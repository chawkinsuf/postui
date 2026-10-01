#!/usr/bin/env python3
"""Expands scripts/vim_oracle/corpus.toml, runs every case through real Vim
9.1, and writes crates/postui/tests/vim_conformance/golden.jsonl.

    python3 scripts/vim_oracle/generate.py                  # everything
    python3 scripts/vim_oracle/generate.py --only 'motions/*'
    python3 scripts/vim_oracle/generate.py --fuzz 500 --seed 7
    python3 scripts/vim_oracle/generate.py --classes        # class_table.rs
    python3 scripts/vim_oracle/generate.py --cases          # case_table.rs

Paths are found from this file, so it runs from any directory.
Design: docs/superpowers/specs/2026-09-27-vim-text-engine-design.md §6.
"""
import argparse
import hashlib
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
CORPUS = os.path.join(HERE, "corpus.toml")
KEYS = os.path.join(HERE, "keys.toml")
ORACLE = os.path.join(HERE, "oracle.vim")
CLASSES_VIM = os.path.join(HERE, "classes.vim")
CASES_VIM = os.path.join(HERE, "cases.vim")
ENGINE = os.path.join(ROOT, "crates", "postui", "src", "vim", "engine")
SETTINGS_RS = os.path.join(ENGINE, "settings.rs")
CLASS_TABLE = os.path.join(ENGINE, "class_table.rs")
CASE_TABLE = os.path.join(ENGINE, "case_table.rs")
GOLDEN = os.path.join(ROOT, "crates", "postui", "tests", "vim_conformance", "golden.jsonl")

# Harness-only settings (spec §3.4 and §6.3 traps 5, 6, 10). Never engine
# behaviour, so they are not part of SETTINGS_LINE.
HARNESS = ("noesckeys notimeout ttimeout ttimeoutlen=0 nomore scrolloff=0 "
           "lines=24 columns=80 nowrap nohlsearch noincsearch")
MAX_EACH_LINES = 12
# Texts longer than this record `top` (the scroll cases, plan 3c).
TOP_MIN_LINES = 21
TOKEN = re.compile(r"<[^<>]+>|.", re.S)
# Keys that start a change, for the `.` lint (§6.2). A heuristic guard.
CHANGE_START = set("dcxXsSDCpPrJiaIAoO~><R") | {"<C-a>", "<C-x>", "<Del>"}
# Keys Vim would run as commands that leave Normal mode or reach outside it
# (`:` and Ex mode, `K`'s keyword program, macros, filters, `:s` repeats) if
# an earlier change fails and the text meant to be typed is read as
# commands. A failed `ci"` once turned a typed `Q` into Ex mode and lost the
# capture (2026-09-29). Typed text uses X, Y and the like instead.
UNSAFE = {":", "Q", "K", "q", "@", "!", "&"}


def die(msg):
    sys.exit(f"generate.py: {msg}")


def settings_line():
    src = open(SETTINGS_RS, encoding="utf-8").read()
    m = re.search(r'pub const SETTINGS_LINE: &str =\s*"([^"]+)";', src)
    if not m:
        die(f"no SETTINGS_LINE in {SETTINGS_RS}")
    # The Rust literal escapes a backslash as `\\`; `:set` and the golden
    # header see the value itself (`paragraphs=IPLPPPQPP\ TPHP…`).
    return m.group(1).replace("\\\\", "\\")


def glob_match(pattern, s):
    """`*` matches any run of chars; everything else is literal (ids hold `[`)."""
    parts = pattern.split("*")
    if len(parts) == 1:
        return pattern == s
    if not s.startswith(parts[0]) or not s.endswith(parts[-1]):
        return False
    pos, end = len(parts[0]), len(s) - len(parts[-1])
    for mid in parts[1:-1]:
        i = s.find(mid, pos, end)
        if i < 0:
            return False
        pos = i + len(mid)
    return pos <= end


def tokens(keys, names, where):
    out = TOKEN.findall(keys)
    for t in out:
        if len(t) > 1 and t[1:-1] not in names:
            die(f"{where}: unknown key name {t} (add it to keys.toml and the Rust key map)")
    for i, t in enumerate(out):
        if t in UNSAFE or (t == "Z" and out[i + 1:i + 2] in (["Z"], ["Q"])):
            die(f"{where}: {t!r} is unsafe in a case (see UNSAFE; §6.2 forbids ':')")
    def starts_change(j):
        return out[j] in CHANGE_START or (out[j] == "g" and out[j + 1:j + 2] in (["~"], ["u"], ["U"]))

    for i, t in enumerate(out):
        if t == "." and not any(starts_change(j) for j in range(i)):
            die(f"{where}: '.' must follow a change in the same case (§6.2, trap 8)")
    return out


def seats(lines, at, where):
    """1-based (row, col) seats for one text (§6.2)."""
    rows = list(range(1, len(lines) + 1))
    if len(lines) > MAX_EACH_LINES:
        rows = sorted({1, (len(lines) + 1) // 2, len(lines)})
    out = []
    for seat in at:
        if isinstance(seat, list):
            r, c = seat
            if r <= len(lines) and c <= max(1, len(lines[r - 1])):
                out.append((r, c))
        elif seat == "each":
            if len(lines) > MAX_EACH_LINES:
                die(f"{where}: seat 'each' on a text longer than {MAX_EACH_LINES} lines")
            for r in rows:
                out += [(r, c) for c in range(1, max(1, len(lines[r - 1])) + 1)]
        elif seat == "empty":
            out += [(r, 1) for r in rows if lines[r - 1] == ""][:1]
        elif seat in ("bol", "fnb", "mid", "eol"):
            for r in rows:
                line = lines[r - 1]
                n = max(1, len(line))
                fnb = next((i + 1 for i, ch in enumerate(line) if ch not in " \t"), n)
                out.append({"bol": (r, 1), "fnb": (r, fnb), "mid": (r, n // 2 + 1), "eol": (r, n)}[seat])
        else:
            die(f"{where}: unknown seat {seat!r}")
    seen = set()
    return [s for s in out if not (s in seen or seen.add(s))]


def expand(corpus, names):
    texts = {name: t["lines"] for name, t in corpus["text"].items()}
    cases = {}
    for g in corpus["group"]:
        gid = g["id"]
        status = g.get("status", "ship")
        if status not in ("ship", "later"):
            die(f"{gid}: status must be ship or later")
        for text in g["on"]:
            if text not in texts:
                die(f"{gid}: unknown text {text}")
            for base in g["keys"]:
                for suffix in g.get("suffixes", [""]):
                    keys = base + suffix
                    toks = tokens(keys, names, f"{gid} {keys!r}")
                    for (r, c) in seats(texts[text], g.get("at", ["bol", "mid", "eol"]), gid):
                        cid = f"{gid}/{text}@{r}:{c}/{keys}"
                        cases[cid] = {
                            "id": cid, "group": gid, "tier": g["tier"], "status": status,
                            "text": text, "lines": texts[text], "cursor": [r, c], "keys": keys,
                            "tokens": toks, "reg": g.get("reg"),
                        }
    return texts, cases


def run_vim(cases, settings):
    with tempfile.TemporaryDirectory() as tmp:
        inp = os.path.join(tmp, "in.json")
        out = os.path.join(tmp, "out.jsonl")
        with open(inp, "w", encoding="utf-8") as f:
            json.dump(list(cases.values()), f)
        cmd = ["vim", "-Nu", "NONE", "-i", "NONE", "-n", "--not-a-term",
               "--cmd", f"let g:oracle_in = '{inp}'", "--cmd", f"let g:oracle_out = '{out}'",
               "-c", f"set {settings} {HARNESS}", "-S", ORACLE]
        subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=900, check=False)
        if not os.path.exists(out):
            die("Vim wrote no output (run the vim command in run_vim by hand to see why)")
        rows = [json.loads(l) for l in open(out, encoding="utf-8")]
    return rows[0]["header"], rows[1:]


def case_id(row):
    """A case's id, computed rather than stored: group/text@row:col/keys."""
    return f"{row['group']}/{row['text']}@{row['cursor'][0]}:{row['cursor'][1]}/{row['keys']}"


def golden_rows(cases, results):
    """Compact rows: an omitted `expect` field means the text did not change
    (`lines`), no Visual anchor (`visual`), or not recorded (`top`)."""
    rows = []
    for res in results:
        c = cases[res["id"]]
        cap = res["capture"]
        if cap is None:
            die(f"{c['id']}: no capture. The case ends half-typed (§6.2 lint) or the harness broke")
        if cap["mode"].startswith("no"):
            die(f"{c['id']}: ends with an operator pending (§6.2 lint, trap 9)")
        if cap["mode"] == "c":
            die(f"{c['id']}: ends in the search prompt (§6.2 lint, trap 9)")
        expect = {"cursor": cap["cursor"], "mode": cap["mode"], "reg": cap["reg"], "regtype": cap["regtype"]}
        if cap["lines"] != c["lines"]:
            expect["lines"] = cap["lines"]
        if cap["visual"] is not None:
            expect["visual"] = cap["visual"]
        if len(c["lines"]) >= TOP_MIN_LINES:
            expect["top"] = cap["top"]
        row = {"group": c["group"], "text": c["text"], "cursor": c["cursor"], "keys": c["keys"]}
        if c["reg"] is not None:
            row["reg"] = c["reg"]
        row["expect"] = expect
        if res["errmsg"]:
            row["errmsg"] = res["errmsg"]
        rows.append(row)
    return rows


def header(vim_header, settings, corpus_bytes, texts, groups):
    vl = vim_header["versionlong"]
    if vl < 9010000:
        die(f"Vim {vl} is older than 9.1")
    version = subprocess.run(["vim", "--version"], capture_output=True, text=True).stdout
    patches = next((l.split(":", 1)[1].strip() for l in version.splitlines()
                    if l.startswith("Included patches")), "")
    return {"header": {
        "vim": f"{vl // 1000000}.{vl // 10000 % 100}.{vl % 10000:04d}",
        "patches": patches,
        "settings": settings,
        "winheight": vim_header["winheight"],
        "corpus_sha256": hashlib.sha256(corpus_bytes).hexdigest(),
        "texts": texts,
        "groups": groups,
    }}


def write_jsonl(path, head, rows):
    def dump(obj):
        return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
    with open(path, "w", encoding="utf-8") as f:
        f.write(dump(head) + "\n")
        for r in sorted(rows, key=case_id):
            f.write(dump(r) + "\n")


def read_golden():
    if not os.path.exists(GOLDEN):
        return {}
    lines = open(GOLDEN, encoding="utf-8").read().splitlines()
    return {case_id(r): r for r in map(json.loads, lines[1:])}


def fuzz_keys(atoms, rng):
    """One to six atoms and a closing <Esc>. An atom that starts with `O`
    never follows one that ends with <Esc>: Vim reads `<Esc>O` plus the next
    key as a keypad termcode (`<Esc>Ox`), so the case would test the
    terminal, not the editor. Such an atom is drawn again. So is one whose
    join would read differently from its atoms: `<<`, `yy`, `k`, `e`, `>j`
    spell the key name `<yyke>`."""
    def joins(keys, atom):
        return TOKEN.findall(keys + atom) == TOKEN.findall(keys) + TOKEN.findall(atom)

    keys = ""
    for _ in range(rng.randint(1, 6)):
        atom = rng.choice(atoms)
        while (keys.endswith("<Esc>") and atom.startswith("O")) or not joins(keys, atom):
            atom = rng.choice(atoms)
        keys += atom
    return keys + "<Esc>"


def fuzz(corpus, names, n, seed, settings):
    atoms = corpus["fuzz"]["atoms"]
    rng = random.Random(seed)
    texts = {k: t["lines"] for k, t in corpus["text"].items() if len(t["lines"]) <= MAX_EACH_LINES}
    cases = {}
    for i in range(n):
        name = rng.choice(sorted(texts))
        lines = texts[name]
        r = rng.randrange(len(lines)) + 1
        c = rng.randrange(max(1, len(lines[r - 1]))) + 1
        keys = fuzz_keys(atoms, rng)
        cid = f"fuzz/{seed}/{i}/{name}@{r}:{c}/{keys}"
        cases[cid] = {"id": cid, "group": "fuzz", "tier": 1, "status": "ship", "text": name,
                      "lines": lines, "cursor": [r, c], "keys": keys,
                      "tokens": tokens(keys, names, cid), "reg": None}
    vim_header, results = run_vim(cases, settings)
    path = os.path.join(os.environ.get("TMPDIR", tempfile.gettempdir()), "vim_fuzz.jsonl")
    # The fuzz file carries its own header, so it replays against any golden.
    groups = {"fuzz": {"tier": 1, "status": "ship"}}
    write_jsonl(path, header(vim_header, settings, b"fuzz", texts, groups), golden_rows(cases, results))
    print(f"wrote {n} fuzz cases to {path}\n"
          f"replay: /usr/bin/env VIM_FUZZ={path} cargo test -p postui --test vim_conformance")


def write_classes(settings):
    """class_table.rs from Vim's own charclass() (spec §3.13)."""
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "classes.txt")
        subprocess.run(["vim", "-Nu", "NONE", "-i", "NONE", "-n", "--not-a-term",
                        "--cmd", f"let g:out = '{out}'", "-c", f"set {settings}", "-S", CLASSES_VIM],
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=600, check=False)
        ranges = [tuple(map(int, l.split())) for l in open(out, encoding="utf-8")]
    rows = [f"    ({a:#x}, {b:#x}, {k})," for a, b, k in ranges if k >= 0]
    with open(CLASS_TABLE, "w", encoding="utf-8") as f:
        f.write("//! Generated by `python3 scripts/vim_oracle/generate.py --classes` from\n"
                "//! Vim's own `charclass()` under the engine's `iskeyword`. Do not edit.\n"
                "//! Each row is (first, last, class): 0 blank, 1 punctuation, 2 word,\n"
                "//! 3 emoji, anything larger a script's own word class (spec §3.13).\n\n"
                "pub(super) const TABLE: &[(u32, u32, u32)] = &[\n")
        f.write("\n".join(rows) + "\n];\n")
    print(f"wrote {len(rows)} ranges to {CLASS_TABLE}")


def case_runs(pairs):
    """Sorted (code point, mapped) pairs -> runs of (first, last, step,
    delta): every code point from first to last in steps of step (1 or 2)
    maps to itself plus delta."""
    runs = []
    for c, m in pairs:
        d = m - c
        if runs:
            first, last, step, rd = runs[-1]
            s = c - last
            if rd == d and ((first == last and s in (1, 2)) or (first != last and s == step)):
                runs[-1] = (first, c, s, rd)
                continue
        runs.append((c, c, 1, d))
    return runs


def write_cases(settings):
    """case_table.rs from Vim's own toupper()/tolower() (plan 3b)."""
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "cases.txt")
        subprocess.run(["vim", "-Nu", "NONE", "-i", "NONE", "-n", "--not-a-term",
                        "--cmd", f"let g:out = '{out}'", "-c", f"set {settings}", "-S", CASES_VIM],
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=900, check=False)
        if not os.path.exists(out):
            die("Vim wrote no case table (run the vim command in write_cases by hand to see why)")
        rows = [tuple(map(int, l.split())) for l in open(out, encoding="utf-8")]
    upper = case_runs([(c, u) for c, u, _ in rows if u != c])
    lower = case_runs([(c, l) for c, _, l in rows if l != c])

    def table(name, runs):
        body = "\n".join(f"    ({a:#x}, {b:#x}, {s}, {d})," for a, b, s, d in runs)
        return f"pub(super) const {name}: &[(u32, u32, u32, i32)] = &[\n{body}\n];\n"

    with open(CASE_TABLE, "w", encoding="utf-8") as f:
        f.write("//! Generated by `python3 scripts/vim_oracle/generate.py --cases` from\n"
                "//! Vim's own `toupper()`/`tolower()` under the engine's `casemap`. Do not\n"
                "//! edit. Each row is (first, last, step, delta): every code point from\n"
                "//! `first` to `last` in steps of `step` maps to itself plus `delta`.\n\n")
        f.write(table("UPPER", upper) + "\n" + table("LOWER", lower))
    print(f"wrote {len(upper)} upper and {len(lower)} lower runs to {CASE_TABLE}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", help="regenerate the cases whose id matches this glob; keep the rest")
    ap.add_argument("--fuzz", type=int, help="write N random cases to $TMPDIR/vim_fuzz.jsonl")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--classes", action="store_true", help="regenerate class_table.rs")
    ap.add_argument("--cases", action="store_true", help="regenerate case_table.rs")
    args = ap.parse_args()

    settings = settings_line()
    if args.classes:
        return write_classes(settings)
    if args.cases:
        return write_cases(settings)
    corpus_bytes = open(CORPUS, "rb").read()
    corpus = tomllib.loads(corpus_bytes.decode("utf-8"))
    names = set(tomllib.load(open(KEYS, "rb"))["names"])
    if args.fuzz:
        return fuzz(corpus, names, args.fuzz, args.seed, settings)

    texts, cases = expand(corpus, names)
    old = read_golden()
    run = {k: v for k, v in cases.items() if not args.only or glob_match(args.only, k)}
    vim_header, results = run_vim(run, settings)
    new = {case_id(r): r for r in golden_rows(run, results)}
    merged = {k: v for k, v in old.items() if k in cases and args.only and not glob_match(args.only, k)}
    merged.update(new)
    groups = {g["id"]: {"tier": g["tier"], "status": g.get("status", "ship")} for g in corpus["group"]}
    write_jsonl(GOLDEN, header(vim_header, settings, corpus_bytes, texts, groups), list(merged.values()))
    added = len(set(merged) - set(old))
    removed = len(set(old) - set(merged))
    changed = sum(1 for k in set(merged) & set(old) if merged[k] != old[k])
    size = os.path.getsize(GOLDEN)
    print(f"{len(merged)} cases ({size // 1024} KiB): {added} added, {removed} removed, {changed} changed")


if __name__ == "__main__":
    main()
