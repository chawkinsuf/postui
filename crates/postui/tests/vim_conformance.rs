//! Vim conformance (spec §6.5). Every case in the golden file runs through
//! the engine on the body buffer and, when both the input and Vim's result
//! are one line, on a one-line field too, and must end exactly where Vim
//! 9.1 did: text, caret, mode, Visual anchor, unnamed register.
//!
//! `VIM_CASES='objects/brace/*'` narrows the run (`*` is the only
//! wildcard). `VIM_FUZZ=<file>` also replays a file written by
//! `generate.py --fuzz`. Regenerate the golden file with
//! `python3 scripts/vim_oracle/generate.py`; never edit it by hand.

use edtui::{EditorState, Lines};
use postui::components::line_input::LineInput;
use postui::vim::engine::settings::SETTINGS_LINE;
use postui::vim::engine::{
    BodyBuf, BufState, Engine, Mode, OneLineBuf, Outcome, Pos, RegKind, Register, Seat, Shape, Start, Target,
    TextBuf, ViewCtx,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

const GOLDEN: &str = include_str!("vim_conformance/golden.jsonl");
const DIVERGENCES: &str = include_str!("vim_conformance/divergences.toml");
const KEY_NAMES: &str = include_str!("../../../scripts/vim_oracle/keys.toml");
const MAX_DIFFS: usize = 50;

#[derive(Deserialize)]
struct HeaderLine {
    header: Header,
}

#[derive(Deserialize)]
struct Header {
    vim: String,
    settings: String,
    winheight: usize,
    texts: HashMap<String, Vec<String>>,
    groups: HashMap<String, GroupMeta>,
}

#[derive(Deserialize)]
struct GroupMeta {
    status: String,
}

#[derive(Deserialize)]
struct KeyNames {
    names: Vec<String>,
}

/// One golden row. `id` and `status` are not stored: `load` fills them in.
#[derive(Deserialize, Clone)]
struct Case {
    #[serde(skip)]
    id: String,
    #[serde(skip)]
    status: String,
    group: String,
    text: String,
    cursor: [usize; 2],
    keys: String,
    #[serde(default)]
    reg: Option<Preset>,
    expect: Expect,
    #[serde(default)]
    errmsg: String,
}

/// Parses a golden (or fuzz) file: the header, then the cases with their
/// computed ids and their groups' status.
fn load(text: &str) -> (Header, Vec<Case>) {
    let mut lines = text.lines();
    let header = serde_json::from_str::<HeaderLine>(lines.next().expect("a header line")).expect("the header").header;
    let cases = lines
        .map(|l| {
            let mut case: Case = serde_json::from_str(l).expect("a golden row");
            case.id = format!("{}/{}@{}:{}/{}", case.group, case.text, case.cursor[0], case.cursor[1], case.keys);
            case.status = header.groups.get(&case.group).map_or("ship", |g| g.status.as_str()).to_string();
            case
        })
        .collect();
    (header, cases)
}

#[derive(Deserialize, Clone)]
struct Preset {
    text: String,
    #[serde(rename = "type")]
    kind: String,
}

/// Vim's end state. An omitted `lines` means unchanged, an omitted
/// `visual` no anchor, an omitted `top` not recorded.
#[derive(Deserialize, Clone, Debug, PartialEq)]
struct Expect {
    #[serde(default)]
    lines: Option<Vec<String>>,
    cursor: [usize; 2],
    mode: String,
    #[serde(default)]
    visual: Option<[usize; 2]>,
    reg: String,
    regtype: String,
    #[serde(default)]
    top: Option<usize>,
}

#[derive(Deserialize, Default)]
struct DivergenceFile {
    #[serde(default)]
    divergence: Vec<Divergence>,
}

#[derive(Deserialize)]
struct Divergence {
    id: String,
    buffer: String,
    #[serde(default)]
    skip: bool,
    #[serde(default)]
    expect: Option<Patch>,
    who: String,
    why: String,
}

/// What a divergence changes in Vim's expectation; unset fields keep Vim's.
#[derive(Deserialize, Clone, Default)]
struct Patch {
    lines: Option<Vec<String>>,
    cursor: Option<[usize; 2]>,
    mode: Option<String>,
    visual: Option<[usize; 2]>,
    reg: Option<String>,
    regtype: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Buf {
    Body,
    OneLine,
}

fn buf_name(b: Buf) -> &'static str {
    match b {
        Buf::Body => "body",
        Buf::OneLine => "one-line",
    }
}

/// The engine's end state in the golden file's terms (1-based).
#[derive(Debug)]
struct Actual {
    lines: Vec<String>,
    cursor: [usize; 2],
    mode: String,
    visual: Option<[usize; 2]>,
    reg: String,
    regtype: String,
    top: Option<usize>,
}

enum Run {
    Done(Actual),
    /// A one-line field declined an Insert key Vim gives an effect.
    Skipped,
    /// A corpus error, e.g. the case ends half-typed.
    Broken(String),
}

/// Splits Vim notation the way generate.py does: `<Name>` or one char.
fn tokens(keys: &str) -> Vec<String> {
    let chars: Vec<char> = keys.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<'
            && let Some(len) = chars[i + 1..].iter().position(|&c| c == '>')
            && len > 0
            && !chars[i + 1..i + 1 + len].contains(&'<')
        {
            out.push(chars[i..=i + 1 + len].iter().collect());
            i += len + 2;
            continue;
        }
        out.push(chars[i].to_string());
        i += 1;
    }
    out
}

fn key_event(token: &str) -> KeyEvent {
    let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        // Terminals report an uppercase letter with SHIFT.
        let mods = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        return KeyEvent::new(KeyCode::Char(c), mods);
    }
    match &token[1..token.len() - 1] {
        "Esc" => plain(KeyCode::Esc),
        "CR" => plain(KeyCode::Enter),
        "BS" => plain(KeyCode::Backspace),
        "Del" => plain(KeyCode::Delete),
        "Tab" => plain(KeyCode::Tab),
        "Space" => plain(KeyCode::Char(' ')),
        "lt" => plain(KeyCode::Char('<')),
        "Left" => plain(KeyCode::Left),
        "Right" => plain(KeyCode::Right),
        "Up" => plain(KeyCode::Up),
        "Down" => plain(KeyCode::Down),
        "Home" => plain(KeyCode::Home),
        "End" => plain(KeyCode::End),
        name if name.starts_with("C-") && name.chars().count() == 3 => {
            KeyEvent::new(KeyCode::Char(name.chars().nth(2).unwrap()), KeyModifiers::CONTROL)
        }
        name => panic!("no key mapping for <{name}>: add it here and to keys.toml"),
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Normal => "n",
        Mode::Insert => "i",
        Mode::Visual(Shape::Char) => "v",
        Mode::Visual(Shape::Line) => "V",
    }
}

fn run_case(case: &Case, input: &[String], buf: Buf, rows: usize) -> Run {
    let mut engine = Engine::new();
    if let Some(p) = &case.reg {
        let kind = if p.kind == "V" { RegKind::Line } else { RegKind::Char };
        engine.registers_mut().set_unnamed(Register { text: p.text.clone(), kind });
    }
    let mut state = BufState::new();
    let at = Pos::new(case.cursor[0] - 1, case.cursor[1] - 1);
    match buf {
        Buf::Body => {
            let mut ed = EditorState::new(Lines::from(input.join("\n")));
            let mut body = BodyBuf::new(&mut ed, Style::default());
            body.set_cursor(at);
            drive(&mut engine, &mut body, &mut state, case, &ViewCtx { viewport_rows: Some(rows) })
        }
        Buf::OneLine => {
            let mut field = LineInput::new(&input[0]);
            let mut one = OneLineBuf::new(&mut field);
            one.set_cursor(at);
            drive(&mut engine, &mut one, &mut state, case, &ViewCtx { viewport_rows: None })
        }
    }
}

fn drive<B: TextBuf>(engine: &mut Engine, buf: &mut B, state: &mut BufState, case: &Case, ctx: &ViewCtx) -> Run {
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut *buf, state: &mut *state });
    for token in tokens(&case.keys) {
        // Spec §6.5: the one-line run's first rule (a one-line result) is
        // meant to skip `o` and `O`, whose one-line behaviour (a no-op,
        // §5) S tests pin. `o<BS>` or `oX<Esc>u` bring Vim back to one line
        // and slip through that rule, so a Normal-mode `o`/`O` skips the run
        // outright.
        if !B::MULTILINE && engine.mode() == Mode::Normal && !engine.pending() && matches!(token.as_str(), "o" | "O") {
            return Run::Skipped;
        }
        let inserting = engine.mode() == Mode::Insert;
        let out = engine.handle(key_event(&token), Target { buf: &mut *buf, state: &mut *state }, ctx);
        // A one-line field hands Insert keys it doesn't own (Tab, Up,
        // Down) to the app, where Vim gives them an effect (spec §6.5).
        if inserting && !B::MULTILINE && matches!(out, Outcome::Declined { .. }) {
            return Run::Skipped;
        }
    }
    if engine.pending() {
        return Run::Broken(format!("ends half-typed (echo {:?}): move it to an S test", engine.echo()));
    }
    let caret = buf.cursor();
    let reg = engine.registers().unnamed();
    Run::Done(Actual {
        lines: (0..buf.line_count()).map(|r| buf.line(r).iter().collect()).collect(),
        cursor: [caret.row + 1, caret.col + 1],
        mode: mode_name(engine.mode()).to_string(),
        visual: engine.visual_anchor().map(|p| [p.row + 1, p.col + 1]),
        reg: reg.text.clone(),
        regtype: match reg.kind {
            RegKind::Char => "v",
            RegKind::Line => "V",
        }
        .to_string(),
        top: Some(buf.top() + 1),
    })
}

fn expected(case: &Case, input: &[String], patch: Option<&Patch>) -> Expect {
    let mut e = case.expect.clone();
    if e.lines.is_none() {
        e.lines = Some(input.to_vec());
    }
    if let Some(p) = patch {
        if let Some(v) = &p.lines {
            e.lines = Some(v.clone());
        }
        if let Some(v) = p.cursor {
            e.cursor = v;
        }
        if let Some(v) = &p.mode {
            e.mode = v.clone();
        }
        if let Some(v) = p.visual {
            e.visual = Some(v);
        }
        if let Some(v) = &p.reg {
            e.reg = v.clone();
        }
        if let Some(v) = &p.regtype {
            e.regtype = v.clone();
        }
    }
    e
}

fn agrees(a: &Actual, e: &Expect) -> bool {
    Some(&a.lines) == e.lines.as_ref()
        && a.cursor == e.cursor
        && a.mode == e.mode
        && a.visual == e.visual
        && a.reg == e.reg
        && a.regtype == e.regtype
        && e.top.is_none_or(|t| a.top == Some(t))
}

fn diff(case: &Case, buf: Buf, input: &[String], e: &Expect, a: &Actual) -> String {
    let said = if case.errmsg.is_empty() { String::new() } else { format!("\n    vim said {:?}", case.errmsg) };
    format!(
        "{} [{}] keys {:?}\n    input  {:?} @{:?}\n    vim    {:?} @{:?} {} visual {:?} reg {:?}/{}\n    engine {:?} @{:?} {} visual {:?} reg {:?}/{}{said}",
        case.id, buf_name(buf), case.keys, input, case.cursor,
        e.lines.as_ref().expect("resolved"), e.cursor, e.mode, e.visual, e.reg, e.regtype,
        a.lines, a.cursor, a.mode, a.visual, a.reg, a.regtype,
    )
}

/// `*` matches any run of chars; everything else is literal (ids hold `[`).
fn glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, .., last] = parts.as_slice() else { return pattern == s };
    if !s.starts_with(first) || !s.ends_with(last) || s.len() < first.len() + last.len() {
        return false;
    }
    let (mut pos, end) = (first.len(), s.len() - last.len());
    for mid in &parts[1..parts.len() - 1] {
        match s[pos..end].find(mid) {
            Some(i) => pos += i + mid.len(),
            None => return false,
        }
    }
    true
}

#[derive(Default)]
struct Tally {
    pass: usize,
    fail: usize,
    later_pass: usize,
    later_fail: usize,
    skipped: usize,
}

#[derive(Default)]
struct Report {
    tallies: BTreeMap<String, Tally>,
    diffs: Vec<String>,
    stale: Vec<String>,
    broken: usize,
}

impl Report {
    fn failed(&self) -> usize {
        self.tallies.values().map(|t| t.fail).sum::<usize>() + self.broken + self.stale.len()
    }

    fn merge(&mut self, other: Report) {
        for (group, t) in other.tallies {
            let mine = self.tallies.entry(group).or_default();
            mine.pass += t.pass;
            mine.fail += t.fail;
            mine.later_pass += t.later_pass;
            mine.later_fail += t.later_fail;
            mine.skipped += t.skipped;
        }
        self.diffs.extend(other.diffs);
        self.stale.extend(other.stale);
        self.broken += other.broken;
    }

    fn print(&self, vim: &str) {
        let (mut pass, mut fail) = (0, 0);
        println!("Vim {vim} conformance, per group (ship pass/fail, later pass/fail, skipped):");
        for (group, t) in &self.tallies {
            pass += t.pass;
            fail += t.fail;
            let flag = if t.fail > 0 { "  FAIL" } else { "" };
            println!("  {group:<32} {:>5}/{:<4} {:>5}/{:<4} {:>4}{flag}", t.pass, t.fail, t.later_pass, t.later_fail, t.skipped);
            if t.later_pass > 0 && t.later_fail == 0 && t.fail == 0 && t.pass == 0 {
                println!("    ^ every 'later' run passes: switch this group to status = \"ship\"");
            }
        }
        println!("total ship runs: {pass} pass, {fail} fail; {} broken", self.broken);
        for s in &self.stale {
            println!("STALE {s}");
        }
        for d in self.diffs.iter().take(MAX_DIFFS) {
            println!("{d}");
        }
        if self.diffs.len() > MAX_DIFFS {
            println!("… and {} more (narrow with VIM_CASES=<glob>)", self.diffs.len() - MAX_DIFFS);
        }
    }
}

fn check(
    cases: &[Case],
    texts: &HashMap<String, Vec<String>>,
    rows: usize,
    divs: &[Divergence],
    filter: Option<&str>,
) -> Report {
    let mut report = Report::default();
    let mut used = vec![false; divs.len()];
    for case in cases {
        if filter.is_some_and(|f| !glob(f, &case.id)) {
            continue;
        }
        let input = &texts[&case.text];
        let one_line = input.len() == 1 && case.expect.lines.as_ref().is_none_or(|l| l.len() == 1);
        for buf in [Buf::Body, Buf::OneLine] {
            if buf == Buf::OneLine && !one_line {
                continue;
            }
            let tally = report.tallies.entry(case.group.clone()).or_default();
            let div = divs.iter().position(|d| glob(&d.id, &case.id) && (d.buffer == "both" || d.buffer == buf_name(buf)));
            if let Some(i) = div {
                used[i] = true;
            }
            if div.is_some_and(|i| divs[i].skip) {
                // A skip may exist because the engine panics, so catch that.
                // Still run it: a skip the engine no longer needs is stale.
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_case(case, input, buf, rows)));
                if let Ok(Run::Done(actual)) = run
                    && agrees(&actual, &expected(case, input, None))
                {
                    report.stale.push(format!("{} [{}]: the engine now matches Vim; delete the skip", case.id, buf_name(buf)));
                }
                tally.skipped += 1;
                continue;
            }
            let expect = expected(case, input, div.and_then(|i| divs[i].expect.as_ref()));
            let ok = match run_case(case, input, buf, rows) {
                Run::Skipped => {
                    tally.skipped += 1;
                    continue;
                }
                Run::Broken(why) => {
                    report.broken += 1;
                    report.diffs.push(format!("{} [{}] BROKEN: {why}", case.id, buf_name(buf)));
                    continue;
                }
                Run::Done(actual) => {
                    let ok = agrees(&actual, &expect);
                    if div.is_some() && agrees(&actual, &expected(case, input, None)) {
                        report.stale.push(format!("{} [{}]: the engine now matches Vim; delete the divergence", case.id, buf_name(buf)));
                    }
                    if !ok && case.status == "ship" {
                        report.diffs.push(diff(case, buf, input, &expect, &actual));
                    }
                    ok
                }
            };
            match (case.status.as_str(), ok) {
                ("ship", true) => tally.pass += 1,
                ("ship", false) => tally.fail += 1,
                (_, true) => tally.later_pass += 1,
                (_, false) => tally.later_fail += 1,
            }
        }
    }
    if filter.is_none() {
        for (d, was_used) in divs.iter().zip(&used) {
            if !was_used {
                report.stale.push(format!("divergence {} matches no case", d.id));
            }
        }
    }
    report
}

#[test]
fn the_engine_matches_vim() {
    let (header, cases) = load(GOLDEN);
    assert_eq!(
        header.settings, SETTINGS_LINE,
        "settings.rs changed without regenerating the golden file: run python3 scripts/vim_oracle/generate.py"
    );
    let names: KeyNames = toml::from_str(KEY_NAMES).expect("keys.toml");
    for name in &names.names {
        key_event(&format!("<{name}>"));
    }
    let divs: DivergenceFile = toml::from_str(DIVERGENCES).expect("divergences.toml");
    for d in &divs.divergence {
        assert!(
            ["you", "mine", "earlier"].contains(&d.who.as_str())
                && !d.why.trim().is_empty()
                && ["body", "one-line", "both"].contains(&d.buffer.as_str()),
            "divergence {} needs who (you|mine|earlier), why and buffer",
            d.id
        );
    }
    let filter = std::env::var("VIM_CASES").ok();
    let mut report = check(&cases, &header.texts, header.winheight, &divs.divergence, filter.as_deref());
    if let Ok(path) = std::env::var("VIM_FUZZ") {
        let text = std::fs::read_to_string(&path).expect("the VIM_FUZZ file");
        let (fh, fuzz) = load(&text);
        report.merge(check(&fuzz, &fh.texts, fh.winheight, &[], None));
    }
    report.print(&header.vim);
    assert_eq!(report.failed(), 0, "Vim conformance failures: see the report above");
}

/// One synthetic <Esc> case, which the engine matches, in a group of the
/// given status.
fn synthetic(status: &str) -> (Header, Vec<Case>) {
    let text = format!(
        concat!(
            r#"{{"header":{{"vim":"9","settings":"","winheight":23,"texts":{{"t":["abc"]}},"groups":{{"g":{{"status":"{}"}}}}}}}}"#,
            "\n",
            r#"{{"group":"g","text":"t","cursor":[1,1],"keys":"<Esc>","expect":{{"cursor":[1,1],"mode":"n","reg":"","regtype":"v"}}}}"#,
        ),
        status
    );
    load(&text)
}

fn divergence(skip: bool, patch: Option<Patch>) -> Divergence {
    Divergence {
        id: "g/t@1:1/<Esc>".into(),
        buffer: "both".into(),
        skip,
        expect: patch,
        who: "mine".into(),
        why: "test".into(),
    }
}

#[test]
fn a_divergence_is_stale_when_the_engine_matches_vim() {
    let real_patch = Patch { cursor: Some([1, 2]), ..Patch::default() };
    for status in ["ship", "later"] {
        let (header, cases) = synthetic(status);
        for div in [divergence(false, Some(real_patch.clone())), divergence(true, None)] {
            let skip = div.skip;
            let report = check(&cases, &header.texts, header.winheight, &[div], None);
            assert!(!report.stale.is_empty(), "{status} skip={skip}: engine matches Vim, so it is stale");
            assert!(report.failed() > 0, "{status} skip={skip}: a stale entry fails the test");
        }
    }
}

#[test]
fn tokens_split_like_the_generator() {
    assert_eq!(tokens("d3w"), ["d", "3", "w"]);
    assert_eq!(tokens("ciwX<Esc>u<C-r>"), ["c", "i", "w", "X", "<Esc>", "u", "<C-r>"]);
    assert_eq!(tokens("<lt><>a<"), ["<lt>", "<", ">", "a", "<"]);
    assert!(glob("objects/brace/*", "objects/brace/json_pretty@2:5/di{"));
    assert!(glob("*/di[", "objects/bracket/x@1:1/di["));
    assert!(!glob("motions/*", "ops/x"));
}
