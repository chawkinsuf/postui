#!/usr/bin/env python3
"""Runs every case in scripts/vim_oracle/response_corpus.toml through real
Vim 9.1 and writes crates/postui/tests/vim_conformance/response_golden.jsonl,
which `app/response_conformance_tests.rs` replays on the vim profile's
response pane.

    python3 scripts/vim_oracle/response.py

The response is a window over read-only lines: Raw's lines as they are, or
the tree's lines (the body pretty-printed, two spaces) with one fold per
container. Vim runs with the text engine's pinned settings (settings.rs)
and generate.py's harness settings, so its window is the pane's height.
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import generate  # noqa: E402  (its helpers: settings, tokens, the header)

CORPUS = os.path.join(HERE, "response_corpus.toml")
ORACLE = os.path.join(HERE, "response_oracle.vim")
GOLDEN = os.path.join(generate.ROOT, "crates", "postui", "tests", "vim_conformance", "response_golden.jsonl")
# The <Name> keys a response case may use. The Rust replay types each one.
NAMES = {"Esc", "CR", "Space", "C-d", "C-u", "C-f", "C-b", "Up", "Down", "Left", "Right", "Home", "End",
         "PageUp", "PageDown", "C-Home", "C-End"}
# Vim's message numbers are not part of the response's notes.
ENUM = re.compile(r"^E\d+: ")
# Message-history lines that are Vim's or the harness's, not the keys'.
NOISE = re.compile(r"^(Messages maintainer: |Error detected while processing |line +\d+:$|Warning: terminal cannot highlight$)")


def die(msg):
    sys.exit(f"response.py: {msg}")


def pretty_lines(src):
    return json.dumps(json.loads(src), indent=2, ensure_ascii=False).split("\n")


def container_folds(lines):
    """Each container's (first, last) line, 1-based, inner ones first."""
    stack, folds = [], []
    for i, line in enumerate(lines, 1):
        t = line.strip()
        if t[:1] in ("}", "]"):
            folds.append((stack.pop(), i))
        if t.endswith(("{", "[")):
            stack.append(i)
    if stack:
        die("unbalanced pretty lines")
    return sorted(folds, key=lambda f: f[1] - f[0])


def bodies(corpus):
    out = {}
    for name, b in corpus["body"].items():
        if b["view"] == "raw":
            lines = b["lines"]
            out[name] = {"view": "raw", "body": "\n".join(lines), "lines": lines, "folds": []}
        elif b["view"] == "pretty":
            lines = pretty_lines(b["json"])
            out[name] = {"view": "pretty", "body": b["json"], "lines": lines, "folds": container_folds(lines)}
        else:
            die(f"body {name}: view must be raw or pretty")
    return out


def expand(corpus, bods):
    cases = {}
    for g in corpus["group"]:
        body = bods.get(g["body"]) or die(f"group {g['id']}: no body {g['body']}")
        for keys in g["cases"]:
            cid = f"{g['id']}/{keys}"
            if cid in cases:
                die(f"duplicate case {cid}")
            toks = generate.TOKEN.findall(keys)
            for t in toks:
                if len(t) > 1 and t[1:-1] not in NAMES:
                    die(f"{cid}: key {t} is not in NAMES")
                if t in generate.UNSAFE:
                    die(f"{cid}: {t} would leave Normal mode in Vim")
            cases[cid] = {"id": cid, "body": g["body"], "keys": keys, "tokens": toks,
                          "lines": body["lines"], "folds": body["folds"]}
    return cases


def run_vim(cases, settings):
    with tempfile.TemporaryDirectory() as tmp:
        inp, out = os.path.join(tmp, "in.json"), os.path.join(tmp, "out.jsonl")
        with open(inp, "w", encoding="utf-8") as f:
            json.dump(list(cases.values()), f)
        cmd = ["vim", "-Nu", "NONE", "-i", "NONE", "-n", "--not-a-term",
               "--cmd", f"let g:oracle_in = '{inp}'", "--cmd", f"let g:oracle_out = '{out}'",
               "-c", f"set {settings} {generate.HARNESS}", "-S", ORACLE]
        subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=600, check=False)
        if not os.path.exists(out):
            die("Vim wrote no output (run the vim command in run_vim by hand to see why)")
        rows = [json.loads(l) for l in open(out, encoding="utf-8")]
    return rows[0]["header"], rows[1:]


def golden_rows(cases, results):
    rows = []
    for res in results:
        c, cap = cases[res["id"]], res["capture"]
        if cap is None:
            die(f"{c['id']}: no capture (the case ends half-typed)")
        if cap["mode"] not in ("n", "V"):
            die(f"{c['id']}: ends in mode {cap['mode']!r}")
        msgs = [ENUM.sub("", m) for m in cap["msgs"] if m and not NOISE.match(m)]
        expect = {k: cap[k] for k in ("mode", "cursor", "top", "reg", "regtype", "search", "fwd", "closed")}
        # What the message line shows at the end: the last message given.
        expect["msg"] = msgs[-1] if msgs else None
        if cap["visual"] is not None:
            expect["visual"] = cap["visual"]
        rows.append({"id": c["id"], "body": c["body"], "keys": c["keys"], "expect": expect})
    return rows


def main():
    settings = generate.settings_line()
    corpus_bytes = open(CORPUS, "rb").read()
    corpus = tomllib.loads(corpus_bytes.decode("utf-8"))
    bods = bodies(corpus)
    cases = expand(corpus, bods)
    vim_header, results = run_vim(cases, settings)
    rows = golden_rows(cases, results)
    head = generate.header(vim_header, settings, corpus_bytes, {}, {})
    head["header"].pop("texts")
    head["header"].pop("groups")
    head["header"]["bodies"] = {k: {"view": v["view"], "body": v["body"], "lines": v["lines"]} for k, v in bods.items()}
    with open(GOLDEN, "w", encoding="utf-8") as f:
        for obj in [head] + sorted(rows, key=lambda r: r["id"]):
            f.write(json.dumps(obj, ensure_ascii=False, separators=(",", ":")) + "\n")
    print(f"{len(rows)} response cases ({os.path.getsize(GOLDEN) // 1024} KiB)")


if __name__ == "__main__":
    main()
