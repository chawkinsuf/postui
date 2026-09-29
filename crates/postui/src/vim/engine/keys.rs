//! The Normal/Visual key grammar (spec §3.5): one `Pending` struct turns
//! keys into a `Cmd`, a decline, or nothing yet. Parsing never touches a
//! buffer; executing a `Cmd` is the engine's job. That split is what makes
//! `.` exact: it stores the `Cmd`.

use super::settings::MAX_COUNT;
use super::{Note, Shape};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key as the grammar sees it. SHIFT is part of the char (`$`, `A`), so
/// both terminal spellings of a printable key agree (review focus 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Ctrl(char),
    Esc,
    Enter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// An alt/super chord, a shifted special key, F-keys: never the engine's.
    Other,
}

impl Key {
    pub(crate) fn of(ev: &KeyEvent) -> Key {
        let plain = ev.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        let bare = ev.modifiers.is_empty();
        match ev.code {
            KeyCode::Char('[') if ev.modifiers == KeyModifiers::CONTROL => Key::Esc,
            KeyCode::Char(c) if ev.modifiers == KeyModifiers::CONTROL => Key::Ctrl(c.to_ascii_lowercase()),
            KeyCode::Char(c) if plain => Key::Char(c),
            KeyCode::Esc if bare => Key::Esc,
            KeyCode::Enter if bare => Key::Enter,
            KeyCode::Tab if bare => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Backspace if bare => Key::Backspace,
            KeyCode::Delete if bare => Key::Delete,
            KeyCode::Left if bare => Key::Left,
            KeyCode::Right if bare => Key::Right,
            KeyCode::Up if bare => Key::Up,
            KeyCode::Down if bare => Key::Down,
            KeyCode::Home if bare => Key::Home,
            KeyCode::End if bare => Key::End,
            _ => Key::Other,
        }
    }
}

/// The tier-1 operators. Tier 2 (`gu gU g~ > <` as operators) is plan 3b.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Delete,
    Change,
    Yank,
}

/// What Visual `~` `u` `U` do to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseOp {
    /// `u`
    Lower,
    /// `U`
    Upper,
    /// `~`
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindKind {
    /// `f`
    Find,
    /// `t`
    Till,
    /// `F`
    FindBack,
    /// `T`
    TillBack,
}

impl FindKind {
    #[allow(dead_code)] // used from Task 6
    pub(crate) fn forward(self) -> bool {
        matches!(self, FindKind::Find | FindKind::Till)
    }

    #[allow(dead_code)] // used from Task 6
    pub(crate) fn till(self) -> bool {
        matches!(self, FindKind::Till | FindKind::TillBack)
    }

    #[allow(dead_code)] // used from Task 6
    pub(crate) fn reversed(self) -> Self {
        match self {
            FindKind::Find => FindKind::FindBack,
            FindKind::Till => FindKind::TillBack,
            FindKind::FindBack => FindKind::Find,
            FindKind::TillBack => FindKind::Till,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Motion {
    /// `h`, `<Left>`
    Left,
    /// `l`, `<Right>`
    Right,
    /// `k`, `<Up>`
    Up,
    /// `j`, `<Down>`
    Down,
    /// `<BS>`: `h` that wraps to the previous line (`whichwrap=b`)
    BackWrap,
    /// `<Space>`: `l` that wraps to the next line (`whichwrap=s`)
    ForwardWrap,
    /// `0`, `<Home>`
    LineStart,
    /// `^`
    FirstNonBlank,
    /// `$`, `<End>`
    LineEnd,
    /// `gg`
    FirstLine,
    /// `G`
    LastLine,
    WordFwd { big: bool },
    WordBack { big: bool },
    WordEnd { big: bool },
    Find(FindKind, char),
    /// `;` (`reverse: false`) and `,`
    RepeatFind { reverse: bool },
    /// `%`
    MatchPair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Object {
    Word { big: bool },
    /// The quote char: `"`, `'` or `` ` ``.
    Quote(char),
    /// The opening char: `(`, `[` or `{`.
    Block(char),
}

/// What an operator acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    Motion(Motion),
    Object { obj: Object, inner: bool },
    /// The doubled operator (`dd`, `cc`, `yy`) and its shorthands.
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertHow {
    /// `i`
    Before,
    /// `a`
    After,
    /// `I`
    LineStart,
    /// `A`
    LineEnd,
    /// `o`
    OpenBelow,
    /// `O`
    OpenAbove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisualOp {
    Delete,
    DeleteLines,
    Yank,
    YankLines,
    Change,
    ChangeLines,
    Put { before: bool },
    Replace(char),
    Join,
    /// `~` `u` `U`.
    Case(CaseOp),
    Shift { right: bool },
}

/// A complete command. Counts are 0 when none was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cmd {
    Move { motion: Motion, count: usize },
    /// `count` is the two counts combined (`2d3w` is 6), as Vim stores it.
    Operate { op: Op, reach: Reach, count: usize, reg: Option<char> },
    Put { before: bool, count: usize, reg: Option<char> },
    Replace { ch: char, count: usize },
    Join { count: usize },
    Insert { how: InsertHow, count: usize },
    Undo(usize),
    Redo(usize),
    Repeat(usize),
    VisualStart(Shape),
    VisualSwap,
    VisualExit,
    VisualObject { obj: Object, inner: bool, count: usize },
    VisualOp { op: VisualOp, count: usize, reg: Option<char> },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ParseCx {
    pub visual: bool,
    pub multiline: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    /// One more key of a half-typed command.
    More,
    Cmd(Cmd),
    /// Not the engine's key (spec §4.3).
    Decline { count: Option<usize>, keys: Vec<KeyEvent> },
    /// Consumed with no effect, maybe with a footer note.
    Inert(Option<Note>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prefix {
    G,
    Register,
    Object { inner: bool },
    Find(FindKind),
    Replace,
}

/// The half-typed command (spec §3.5).
#[derive(Debug, Default)]
pub(crate) struct Pending {
    reg: Option<char>,
    count1: usize,
    op: Option<Op>,
    count2: usize,
    prefix: Option<Prefix>,
    /// Every key swallowed after the count, for `Step::Decline`.
    keys: Vec<KeyEvent>,
}

/// Folds one more digit into a count, saturating at [`MAX_COUNT`].
pub(crate) fn accumulate_count(count: usize, digit: u32) -> usize {
    count.saturating_mul(10).saturating_add(digit as usize).min(MAX_COUNT)
}

/// Two counts multiply (`2d3w` is 6), capped. 0 when neither was typed.
pub(crate) fn combine_counts(outer: usize, inner: usize) -> usize {
    if outer == 0 && inner == 0 {
        0
    } else {
        outer.max(1).saturating_mul(inner.max(1)).min(MAX_COUNT)
    }
}

pub(crate) fn op_name(op: Op) -> &'static str {
    match op {
        Op::Delete => "d",
        Op::Change => "c",
        Op::Yank => "y",
    }
}

fn find_char(kind: FindKind) -> char {
    match kind {
        FindKind::Find => 'f',
        FindKind::Till => 't',
        FindKind::FindBack => 'F',
        FindKind::TillBack => 'T',
    }
}

fn find_kind(c: char) -> FindKind {
    match c {
        'f' => FindKind::Find,
        't' => FindKind::Till,
        'F' => FindKind::FindBack,
        _ => FindKind::TillBack,
    }
}

fn motion_of(key: Key) -> Option<Motion> {
    Some(match key {
        Key::Char('h') | Key::Left => Motion::Left,
        Key::Char('l') | Key::Right => Motion::Right,
        Key::Char('j') | Key::Down => Motion::Down,
        Key::Char('k') | Key::Up => Motion::Up,
        Key::Backspace => Motion::BackWrap,
        Key::Char(' ') => Motion::ForwardWrap,
        Key::Char('0') | Key::Home => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonBlank,
        Key::Char('$') | Key::End => Motion::LineEnd,
        Key::Char('G') => Motion::LastLine,
        Key::Char('w') => Motion::WordFwd { big: false },
        Key::Char('W') => Motion::WordFwd { big: true },
        Key::Char('b') => Motion::WordBack { big: false },
        Key::Char('B') => Motion::WordBack { big: true },
        Key::Char('e') => Motion::WordEnd { big: false },
        Key::Char('E') => Motion::WordEnd { big: true },
        Key::Char(';') => Motion::RepeatFind { reverse: false },
        Key::Char(',') => Motion::RepeatFind { reverse: true },
        Key::Char('%') => Motion::MatchPair,
        _ => return None,
    })
}

fn object_of(key: Key) -> Option<Object> {
    let Key::Char(c) = key else { return None };
    Some(match c {
        'w' => Object::Word { big: false },
        'W' => Object::Word { big: true },
        '"' | '\'' | '`' => Object::Quote(c),
        '(' | ')' | 'b' => Object::Block('('),
        '[' | ']' => Object::Block('['),
        '{' | '}' | 'B' => Object::Block('{'),
        _ => return None,
    })
}

const SEARCH_KEYS: [char; 6] = ['/', '?', 'n', 'N', '*', '#'];

impl Pending {
    pub(crate) fn is_empty(&self) -> bool {
        self.reg.is_none() && self.count1 == 0 && self.op.is_none() && self.count2 == 0 && self.prefix.is_none()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The footer echo: register, count, operator, count, prefix
    /// (`"02d3`, `di`, `f`, `r`). Empty when nothing is half-typed.
    pub(crate) fn echo(&self) -> String {
        let mut s = String::new();
        if let Some(r) = self.reg {
            s.push('"');
            s.push(r);
        }
        if self.count1 > 0 {
            s += &self.count1.to_string();
        }
        if let Some(op) = self.op {
            s += op_name(op);
        }
        if self.count2 > 0 {
            s += &self.count2.to_string();
        }
        match self.prefix {
            None => {}
            Some(Prefix::G) => s.push('g'),
            Some(Prefix::Register) => s.push('"'),
            Some(Prefix::Object { inner }) => s.push(if inner { 'i' } else { 'a' }),
            Some(Prefix::Find(k)) => s.push(find_char(k)),
            Some(Prefix::Replace) => s.push('r'),
        }
        s
    }

    /// One key. See the grammar table in the plan (Task 5) and spec §4.3.
    pub(crate) fn feed(&mut self, ev: KeyEvent, cx: ParseCx) -> Step {
        let key = Key::of(&ev);
        if matches!(key, Key::Other) || (matches!(key, Key::Ctrl(_)) && key != Key::Ctrl('r')) {
            return self.decline_alone(ev);
        }
        // A pending `r f t F T` takes `Tab` as its argument, even in a
        // one-line field (deviation 17).
        if let Some(prefix) = self.prefix.take() {
            return self.after_prefix(prefix, key, ev, cx);
        }
        if matches!(key, Key::Enter | Key::Tab | Key::BackTab) && !cx.multiline {
            return self.decline_alone(ev);
        }
        let count = if self.op.is_some() { &mut self.count2 } else { &mut self.count1 };
        if let Key::Char(c @ '0'..='9') = key
            && (c != '0' || *count > 0)
        {
            *count = accumulate_count(*count, c.to_digit(10).expect("a digit"));
            return Step::More;
        }
        // `<Del>` in the middle of a count drops its last digit (Vim); only
        // a bare `<Del>` is `x`.
        if key == Key::Delete && *count > 0 {
            *count /= 10;
            return Step::More;
        }
        match key {
            Key::Esc if cx.visual => {
                self.clear();
                Step::Cmd(Cmd::VisualExit)
            }
            Key::Esc if self.is_empty() => self.decline(ev),
            Key::Esc => self.inert(None),
            Key::Ctrl(_) if cx.visual || self.op.is_some() => self.decline_alone(ev),
            Key::Ctrl(_) => self.cmd(|count, _| Cmd::Redo(count)),
            _ if self.op.is_some() => self.operator_arg(key, ev),
            _ if cx.visual => self.visual_key(key, ev),
            _ => self.normal_key(key, ev, cx),
        }
    }

    fn take(&mut self) -> (usize, Option<char>) {
        let count = combine_counts(self.count1, self.count2);
        let reg = self.reg;
        self.clear();
        (count, reg)
    }

    fn cmd(&mut self, f: impl FnOnce(usize, Option<char>) -> Cmd) -> Step {
        let (count, reg) = self.take();
        Step::Cmd(f(count, reg))
    }

    /// Completes whatever waits for a motion: an operator, or a bare move.
    fn motion_done(&mut self, motion: Motion) -> Step {
        match self.op {
            Some(op) => self.cmd(|count, reg| Cmd::Operate { op, reach: Reach::Motion(motion), count, reg }),
            None => self.cmd(|count, _| Cmd::Move { motion, count }),
        }
    }

    fn arm(&mut self, ev: KeyEvent, prefix: Prefix) -> Step {
        self.keys.push(ev);
        self.prefix = Some(prefix);
        Step::More
    }

    /// Declines with every key swallowed since the count.
    fn decline(&mut self, ev: KeyEvent) -> Step {
        let count = (self.count1 > 0).then_some(self.count1);
        let mut keys = std::mem::take(&mut self.keys);
        keys.push(ev);
        self.clear();
        Step::Decline { count, keys }
    }

    /// A chord or special key: whatever was pending is discarded and the
    /// key goes to the app alone, with the count when only a count was
    /// typed (field.rs's rule, which keeps ctrl+c reaching the app).
    fn decline_alone(&mut self, ev: KeyEvent) -> Step {
        let only_count = self.op.is_none() && self.prefix.is_none() && self.reg.is_none();
        let count = (only_count && self.count1 > 0).then_some(self.count1);
        self.clear();
        Step::Decline { count, keys: vec![ev] }
    }

    fn inert(&mut self, note: Option<String>) -> Step {
        self.clear();
        Step::Inert(note.map(Note::Unsupported))
    }

    fn normal_key(&mut self, key: Key, ev: KeyEvent, cx: ParseCx) -> Step {
        if let Some(motion) = motion_of(key) {
            if matches!(motion, Motion::Up | Motion::Down) && !cx.multiline {
                return self.decline(ev);
            }
            return self.motion_done(motion);
        }
        let op = |op: Op, reach: Reach| move |count, reg| Cmd::Operate { op, reach, count, reg };
        let ch = match key {
            Key::Char(c) => c,
            Key::Delete => return self.cmd(op(Op::Delete, Reach::Motion(Motion::Right))),
            _ => return self.inert(None),
        };
        match ch {
            '"' => self.arm(ev, Prefix::Register),
            'd' | 'c' | 'y' => {
                self.keys.push(ev);
                self.op = Some(match ch {
                    'd' => Op::Delete,
                    'c' => Op::Change,
                    _ => Op::Yank,
                });
                Step::More
            }
            'g' => self.arm(ev, Prefix::G),
            'f' | 't' | 'F' | 'T' => self.arm(ev, Prefix::Find(find_kind(ch))),
            'r' => self.arm(ev, Prefix::Replace),
            'x' => self.cmd(op(Op::Delete, Reach::Motion(Motion::Right))),
            'X' => self.cmd(op(Op::Delete, Reach::Motion(Motion::Left))),
            'D' => self.cmd(op(Op::Delete, Reach::Motion(Motion::LineEnd))),
            'C' => self.cmd(op(Op::Change, Reach::Motion(Motion::LineEnd))),
            's' => self.cmd(op(Op::Change, Reach::Motion(Motion::Right))),
            'S' => self.cmd(op(Op::Change, Reach::Line)),
            'Y' => self.cmd(op(Op::Yank, Reach::Line)),
            'p' | 'P' => self.cmd(|count, reg| Cmd::Put { before: ch == 'P', count, reg }),
            'J' => self.cmd(|count, _| Cmd::Join { count }),
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' => {
                let how = match ch {
                    'i' => InsertHow::Before,
                    'a' => InsertHow::After,
                    'I' => InsertHow::LineStart,
                    'A' => InsertHow::LineEnd,
                    'o' => InsertHow::OpenBelow,
                    _ => InsertHow::OpenAbove,
                };
                self.cmd(|count, _| Cmd::Insert { how, count })
            }
            'v' => self.cmd(|_, _| Cmd::VisualStart(Shape::Char)),
            'V' => self.cmd(|_, _| Cmd::VisualStart(Shape::Line)),
            'u' => self.cmd(|count, _| Cmd::Undo(count)),
            '.' => self.cmd(|count, _| Cmd::Repeat(count)),
            ':' | 'Z' | 'q' | '@' | 'm' | '\'' | '`' => self.decline(ev),
            'U' | 'K' | 'Q' | '&' => self.inert(Some(format!("{ch} not supported"))),
            c if SEARCH_KEYS.contains(&c) => self.inert(Some(format!("{c} not supported yet"))),
            _ => self.inert(None),
        }
    }

    fn operator_arg(&mut self, key: Key, ev: KeyEvent) -> Step {
        let op = self.op.expect("an operator is pending");
        if let Some(motion) = motion_of(key) {
            return self.motion_done(motion);
        }
        let Key::Char(ch) = key else { return self.inert(None) };
        match ch {
            _ if op_name(op).starts_with(ch) => self.cmd(|count, reg| Cmd::Operate { op, reach: Reach::Line, count, reg }),
            'i' | 'a' => self.arm(ev, Prefix::Object { inner: ch == 'i' }),
            'f' | 't' | 'F' | 'T' => self.arm(ev, Prefix::Find(find_kind(ch))),
            'g' => self.arm(ev, Prefix::G),
            ':' => self.inert(Some(format!("{}: not supported", op_name(op)))),
            c if SEARCH_KEYS.contains(&c) => self.inert(Some(format!("{}{c} not supported yet", op_name(op)))),
            _ => self.inert(None),
        }
    }

    fn visual_key(&mut self, key: Key, ev: KeyEvent) -> Step {
        if let Some(motion) = motion_of(key) {
            return self.cmd(|count, _| Cmd::Move { motion, count });
        }
        let Key::Char(ch) = key else { return self.inert(None) };
        let vop = |op: VisualOp| move |count, reg| Cmd::VisualOp { op, count, reg };
        match ch {
            '"' => self.arm(ev, Prefix::Register),
            'i' | 'a' => self.arm(ev, Prefix::Object { inner: ch == 'i' }),
            'g' => self.arm(ev, Prefix::G),
            'f' | 't' | 'F' | 'T' => self.arm(ev, Prefix::Find(find_kind(ch))),
            'r' => self.arm(ev, Prefix::Replace),
            'd' | 'x' => self.cmd(vop(VisualOp::Delete)),
            'X' | 'D' => self.cmd(vop(VisualOp::DeleteLines)),
            'y' => self.cmd(vop(VisualOp::Yank)),
            'Y' => self.cmd(vop(VisualOp::YankLines)),
            'c' | 's' => self.cmd(vop(VisualOp::Change)),
            'C' | 'S' | 'R' => self.cmd(vop(VisualOp::ChangeLines)),
            'p' | 'P' => self.cmd(vop(VisualOp::Put { before: ch == 'P' })),
            'J' => self.cmd(vop(VisualOp::Join)),
            '~' => self.cmd(vop(VisualOp::Case(CaseOp::Toggle))),
            'u' => self.cmd(vop(VisualOp::Case(CaseOp::Lower))),
            'U' => self.cmd(vop(VisualOp::Case(CaseOp::Upper))),
            '>' => self.cmd(vop(VisualOp::Shift { right: true })),
            '<' => self.cmd(vop(VisualOp::Shift { right: false })),
            'o' => self.cmd(|_, _| Cmd::VisualSwap),
            'v' => self.cmd(|_, _| Cmd::VisualStart(Shape::Char)),
            'V' => self.cmd(|_, _| Cmd::VisualStart(Shape::Line)),
            c if SEARCH_KEYS.contains(&c) => self.inert(Some(format!("{c} not supported yet"))),
            _ => self.inert(None),
        }
    }

    fn after_prefix(&mut self, prefix: Prefix, key: Key, ev: KeyEvent, cx: ParseCx) -> Step {
        match prefix {
            Prefix::Register => match key {
                Key::Char('"') => {
                    self.keys.push(ev);
                    self.reg = Some('"');
                    Step::More
                }
                Key::Char(c) => self.inert(Some(format!("register \"{c} not supported"))),
                _ => self.inert(None),
            },
            Prefix::G => match key {
                Key::Char('g') => self.motion_done(Motion::FirstLine),
                Key::Char('J') => self.inert(Some("gJ not supported".to_string())),
                Key::Esc => self.inert(None),
                _ if self.op.is_none() && !cx.visual => self.decline(ev),
                _ => self.inert(None),
            },
            Prefix::Find(kind) => match key {
                Key::Char(c) => self.motion_done(Motion::Find(kind, c)),
                Key::Tab => self.motion_done(Motion::Find(kind, '\t')),
                _ => self.inert(None),
            },
            Prefix::Object { inner } => match object_of(key) {
                Some(obj) => match self.op {
                    Some(op) => self.cmd(|count, reg| Cmd::Operate { op, reach: Reach::Object { obj, inner }, count, reg }),
                    None => self.cmd(|count, _| Cmd::VisualObject { obj, inner, count }),
                },
                None => self.inert(None),
            },
            Prefix::Replace => {
                let ch = match key {
                    Key::Char(c) => c,
                    Key::Tab => '\t',
                    // `r<CR>` is not supported (deviation 16).
                    _ => return self.inert(None),
                };
                if cx.visual {
                    self.cmd(|count, reg| Cmd::VisualOp { op: VisualOp::Replace(ch), count, reg })
                } else {
                    self.cmd(|count, _| Cmd::Replace { ch, count })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE })
    }

    const NORMAL: ParseCx = ParseCx { visual: false, multiline: true };
    const VISUAL: ParseCx = ParseCx { visual: true, multiline: true };
    const ONE_LINE: ParseCx = ParseCx { visual: false, multiline: false };

    /// Feeds `keys` (plain chars) and returns the last step.
    fn feed(p: &mut Pending, keys: &str, cx: ParseCx) -> Step {
        let mut last = Step::More;
        for c in keys.chars() {
            last = p.feed(ev(c), cx);
        }
        last
    }

    fn cmd(keys: &str, cx: ParseCx) -> Cmd {
        match feed(&mut Pending::default(), keys, cx) {
            Step::Cmd(c) => c,
            other => panic!("{keys:?} gave {other:?}, not a command"),
        }
    }

    #[test]
    fn operators_motions_and_counts() {
        use Motion::*;
        let op = |op, reach, count| Cmd::Operate { op, reach, count, reg: None };
        assert_eq!(cmd("d3w", NORMAL), op(Op::Delete, Reach::Motion(WordFwd { big: false }), 3));
        assert_eq!(cmd("2d3w", NORMAL), op(Op::Delete, Reach::Motion(WordFwd { big: false }), 6), "counts multiply");
        assert_eq!(cmd("dd", NORMAL), op(Op::Delete, Reach::Line, 0));
        assert_eq!(cmd("3yy", NORMAL), op(Op::Yank, Reach::Line, 3));
        assert_eq!(cmd("cW", NORMAL), op(Op::Change, Reach::Motion(WordFwd { big: true }), 0));
        assert_eq!(cmd("dgg", NORMAL), op(Op::Delete, Reach::Motion(FirstLine), 0));
        assert_eq!(cmd("d2fa", NORMAL), op(Op::Delete, Reach::Motion(Find(FindKind::Find, 'a')), 2));
        assert_eq!(cmd("dT,", NORMAL), op(Op::Delete, Reach::Motion(Find(FindKind::TillBack, ',')), 0));
        assert_eq!(cmd("d;", NORMAL), op(Op::Delete, Reach::Motion(RepeatFind { reverse: false }), 0));
        assert_eq!(cmd("di{", NORMAL), op(Op::Delete, Reach::Object { obj: Object::Block('{'), inner: true }, 0));
        assert_eq!(cmd("ca\"", NORMAL), op(Op::Change, Reach::Object { obj: Object::Quote('"'), inner: false }, 0));
        assert_eq!(cmd("y2ab", NORMAL), op(Op::Yank, Reach::Object { obj: Object::Block('('), inner: false }, 2));
        assert_eq!(cmd("diW", NORMAL), op(Op::Delete, Reach::Object { obj: Object::Word { big: true }, inner: true }, 0));
        assert_eq!(cmd("3x", NORMAL), op(Op::Delete, Reach::Motion(Right), 3));
        assert_eq!(cmd("X", NORMAL), op(Op::Delete, Reach::Motion(Left), 0));
        assert_eq!(cmd("D", NORMAL), op(Op::Delete, Reach::Motion(LineEnd), 0));
        assert_eq!(cmd("C", NORMAL), op(Op::Change, Reach::Motion(LineEnd), 0));
        assert_eq!(cmd("s", NORMAL), op(Op::Change, Reach::Motion(Right), 0));
        assert_eq!(cmd("2S", NORMAL), op(Op::Change, Reach::Line, 2));
        assert_eq!(cmd("Y", NORMAL), op(Op::Yank, Reach::Line, 0));
        assert_eq!(cmd("0", NORMAL), Cmd::Move { motion: LineStart, count: 0 });
        assert_eq!(cmd("10l", NORMAL), Cmd::Move { motion: Right, count: 10 });
        assert_eq!(cmd("gg", NORMAL), Cmd::Move { motion: FirstLine, count: 0 });
        assert_eq!(cmd("5G", NORMAL), Cmd::Move { motion: LastLine, count: 5 });
        assert_eq!(cmd("f\t", NORMAL), Cmd::Move { motion: Find(FindKind::Find, '\t'), count: 0 });
    }

    #[test]
    fn simple_commands() {
        assert_eq!(cmd("\"\"P", NORMAL), Cmd::Put { before: true, count: 0, reg: Some('"') });
        assert_eq!(cmd("3rX", NORMAL), Cmd::Replace { ch: 'X', count: 3 });
        assert_eq!(cmd("J", NORMAL), Cmd::Join { count: 0 });
        assert_eq!(cmd("A", NORMAL), Cmd::Insert { how: InsertHow::LineEnd, count: 0 });
        assert_eq!(cmd("O", NORMAL), Cmd::Insert { how: InsertHow::OpenAbove, count: 0 });
        assert_eq!(cmd("V", NORMAL), Cmd::VisualStart(Shape::Line));
        assert_eq!(cmd("3u", NORMAL), Cmd::Undo(3));
        assert_eq!(cmd("4.", NORMAL), Cmd::Repeat(4));
        let mut p = Pending::default();
        assert!(matches!(p.feed(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), NORMAL), Step::Cmd(Cmd::Redo(0))));
    }

    #[test]
    fn delete_key_is_x_but_drops_a_typed_count_digit() {
        let del = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
        let x = Cmd::Operate { op: Op::Delete, reach: Reach::Motion(Motion::Right), count: 0, reg: None };
        assert_eq!(Pending::default().feed(del, NORMAL), Step::Cmd(x));
        let mut p = Pending::default();
        feed(&mut p, "34", NORMAL);
        assert_eq!(p.feed(del, NORMAL), Step::More);
        assert_eq!(p.echo(), "3");
        assert_eq!(p.feed(del, NORMAL), Step::More);
        assert!(p.is_empty(), "the last digit is gone");
        assert_eq!(p.feed(del, NORMAL), Step::Cmd(x), "then it is x again");
        feed(&mut p, "d25", NORMAL);
        assert_eq!(p.feed(del, NORMAL), Step::More);
        assert_eq!(p.echo(), "d2");
    }

    #[test]
    fn visual_commands() {
        let vop = |op, count| Cmd::VisualOp { op, count, reg: None };
        assert_eq!(cmd("d", VISUAL), vop(VisualOp::Delete, 0));
        assert_eq!(cmd("x", VISUAL), vop(VisualOp::Delete, 0));
        assert_eq!(cmd("D", VISUAL), vop(VisualOp::DeleteLines, 0));
        assert_eq!(cmd("Y", VISUAL), vop(VisualOp::YankLines, 0));
        assert_eq!(cmd("s", VISUAL), vop(VisualOp::Change, 0));
        assert_eq!(cmd("R", VISUAL), vop(VisualOp::ChangeLines, 0));
        assert_eq!(cmd("u", VISUAL), vop(VisualOp::Case(CaseOp::Lower), 0), "u in Visual lowercases, it does not undo");
        assert_eq!(cmd("~", VISUAL), vop(VisualOp::Case(CaseOp::Toggle), 0));
        assert_eq!(cmd("U", VISUAL), vop(VisualOp::Case(CaseOp::Upper), 0));
        assert_eq!(cmd("3>", VISUAL), vop(VisualOp::Shift { right: true }, 3));
        assert_eq!(cmd("rX", VISUAL), vop(VisualOp::Replace('X'), 0));
        assert_eq!(cmd("P", VISUAL), vop(VisualOp::Put { before: true }, 0));
        assert_eq!(cmd("o", VISUAL), Cmd::VisualSwap);
        assert_eq!(cmd("2i(", VISUAL), Cmd::VisualObject { obj: Object::Block('('), inner: true, count: 2 });
        assert_eq!(cmd("$", VISUAL), Cmd::Move { motion: Motion::LineEnd, count: 0 });
        let mut p = Pending::default();
        assert!(matches!(p.feed(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), VISUAL), Step::Cmd(Cmd::VisualExit)));
        assert!(
            matches!(feed(&mut Pending::default(), "j", ParseCx { visual: true, multiline: false }), Step::Cmd(Cmd::Move { .. })),
            "a one-line buffer consumes Visual j as a failed motion"
        );
    }

    #[test]
    fn declines_carry_the_swallowed_keys_and_the_count() {
        let decline = |s: Step| match s {
            Step::Decline { count, keys } => (
                count,
                keys.iter()
                    .map(|k| match k.code {
                        KeyCode::Char(c) => c,
                        _ => '?',
                    })
                    .collect::<String>(),
            ),
            other => panic!("not a decline: {other:?}"),
        };
        assert_eq!(decline(feed(&mut Pending::default(), "gt", NORMAL)), (None, "gt".into()));
        assert_eq!(decline(feed(&mut Pending::default(), "3gT", NORMAL)), (Some(3), "gT".into()));
        assert_eq!(decline(feed(&mut Pending::default(), "2:", NORMAL)), (Some(2), ":".into()));
        assert_eq!(decline(feed(&mut Pending::default(), "5j", ONE_LINE)), (Some(5), "j".into()));
        for c in [':', 'Z', 'q', '@', 'm', '\'', '`'] {
            assert!(matches!(Pending::default().feed(ev(c), NORMAL), Step::Decline { .. }), "{c}");
        }
        assert!(matches!(feed(&mut Pending::default(), "j", NORMAL), Step::Cmd(_)), "the body keeps j");
    }

    #[test]
    fn a_chord_discards_whatever_is_pending_and_declines_alone() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut p = Pending::default();
        feed(&mut p, "2d", NORMAL);
        assert_eq!(p.feed(ctrl_c, NORMAL), Step::Decline { count: None, keys: vec![ctrl_c] });
        assert!(p.is_empty());
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        feed(&mut p, "3", NORMAL);
        assert_eq!(p.feed(ctrl_o, NORMAL), Step::Decline { count: Some(3), keys: vec![ctrl_o] });
        let mut p = Pending::default();
        feed(&mut p, "f", NORMAL);
        assert!(matches!(p.feed(ctrl_c, NORMAL), Step::Decline { .. }), "never read as the find's target");
        assert!(p.is_empty());
    }

    #[test]
    fn notes_and_cancels() {
        let note = |s: Step| match s {
            Step::Inert(Some(Note::Unsupported(n))) => n,
            other => panic!("no note: {other:?}"),
        };
        assert_eq!(note(feed(&mut Pending::default(), "U", NORMAL)), "U not supported");
        assert_eq!(note(feed(&mut Pending::default(), "gJ", NORMAL)), "gJ not supported");
        assert_eq!(note(feed(&mut Pending::default(), "d:", NORMAL)), "d: not supported");
        assert_eq!(note(feed(&mut Pending::default(), "d/", NORMAL)), "d/ not supported yet");
        assert_eq!(note(feed(&mut Pending::default(), "n", NORMAL)), "n not supported yet");
        let mut p = Pending::default();
        assert_eq!(note(feed(&mut p, "\"a", NORMAL)), "register \"a not supported");
        assert!(p.is_empty(), "nothing is armed after an unsupported register");
        assert!(matches!(feed(&mut p, "dx", NORMAL), Step::Inert(None)), "an operator with a bad argument cancels");
        assert!(p.is_empty());
        feed(&mut p, "g", NORMAL);
        assert_eq!(p.feed(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), NORMAL), Step::Inert(None), "g<Esc> cancels");
        assert!(p.is_empty());
    }

    #[test]
    fn echo_shows_the_half_typed_command() {
        for (keys, echo) in [
            ("3", "3"),
            ("2d", "2d"),
            ("d3", "d3"),
            ("\"\"", "\"\""),
            ("\"\"2d3", "\"\"2d3"),
            ("di", "di"),
            ("2d3a", "2d3a"),
            ("f", "f"),
            ("dt", "dt"),
            ("r", "r"),
            ("g", "g"),
            ("\"", "\""),
        ] {
            let mut p = Pending::default();
            feed(&mut p, keys, NORMAL);
            assert_eq!(p.echo(), echo, "{keys:?}");
            assert!(!p.is_empty());
        }
        assert_eq!(Pending::default().echo(), "");
    }

    #[test]
    fn counts_saturate() {
        let mut p = Pending::default();
        feed(&mut p, "99999999999999999999", NORMAL);
        assert_eq!(p.echo(), MAX_COUNT.to_string());
        assert_eq!(combine_counts(MAX_COUNT, MAX_COUNT), MAX_COUNT);
        assert_eq!(combine_counts(0, 0), 0);
        assert_eq!(combine_counts(0, 3), 3);
        assert_eq!(accumulate_count(MAX_COUNT, 9), MAX_COUNT);
    }

    /// Review focus 5: terminals spell printable keys differently.
    #[test]
    fn key_spellings_normalise() {
        let m = |code, mods| Key::of(&KeyEvent::new(code, mods));
        assert_eq!(m(KeyCode::Char('$'), KeyModifiers::SHIFT), Key::Char('$'));
        assert_eq!(m(KeyCode::Char('A'), KeyModifiers::NONE), Key::Char('A'));
        assert_eq!(m(KeyCode::Char('['), KeyModifiers::CONTROL), Key::Esc);
        assert_eq!(m(KeyCode::Char('R'), KeyModifiers::CONTROL), Key::Ctrl('r'));
        assert_eq!(m(KeyCode::Char('x'), KeyModifiers::ALT), Key::Other);
        assert_eq!(m(KeyCode::Char('a'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), Key::Other);
        assert_eq!(m(KeyCode::Left, KeyModifiers::SHIFT), Key::Other);
        assert_eq!(m(KeyCode::BackTab, KeyModifiers::SHIFT), Key::BackTab);
        assert_eq!(m(KeyCode::F(2), KeyModifiers::NONE), Key::Other);
    }
}
