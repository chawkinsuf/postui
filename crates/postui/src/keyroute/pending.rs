//! What is half-typed (piece 4 spec §3.4; plan M1 Deviation 10): a count
//! and the keys of an unfinished sequence. The transitions live in
//! [`super::Router::resolve`].

use super::notation::Tok;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    count: Option<usize>,
    keys: Vec<Tok>,
}

impl Pending {
    pub fn is_idle(&self) -> bool {
        self.count.is_none() && self.keys.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn count(&self) -> Option<usize> {
        self.count
    }

    /// The keys this state stands for, as typed: the count's digits, then
    /// the buffered keys. Replayed into the engine when the field rows
    /// match nothing (spec §3.7).
    pub fn replay(&self) -> Vec<ratatui::crossterm::event::KeyEvent> {
        let digits = self.count.map(|n| n.to_string()).unwrap_or_default();
        digits
            .chars()
            .map(|c| Tok::Char(c).event())
            .chain(self.keys.iter().map(|t| t.event()))
            .collect()
    }

    pub fn keys(&self) -> &[Tok] {
        &self.keys
    }

    pub fn push(&mut self, t: Tok) {
        self.keys.push(t);
    }

    /// Takes `c` into the count: `1`–`9`, or `0` once a count has
    /// started, while no key is buffered (`g3` is not a count). A leading
    /// `0` is a key. Saturates at the engine's `MAX_COUNT`.
    pub fn digit(&mut self, c: char) -> bool {
        let Some(d) = c.to_digit(10) else {
            return false;
        };
        if !self.keys.is_empty() || (d == 0 && self.count.is_none()) {
            return false;
        }
        self.count = Some(crate::vim::engine::accumulate_count(self.count.unwrap_or(0), d));
        true
    }

    /// The footer's echo (spec §3.8): `3`, `g`, `2g`, `^W`.
    pub fn echo(&self) -> String {
        let mut out = self.count.map_or(String::new(), |n| n.to_string());
        for t in &self.keys {
            match t {
                Tok::Char(c) => out.push(*c),
                Tok::Ctrl(c) => {
                    out.push('^');
                    out.push(c.to_ascii_uppercase());
                }
                Tok::Named(_) | Tok::CtrlNamed(_) | Tok::Any => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vim::engine::settings::MAX_COUNT;

    #[test]
    fn replay_types_the_count_then_the_keys() {
        let mut p = Pending::default();
        p.digit('1');
        p.digit('2');
        p.push(Tok::Char('g'));
        let typed: Vec<Tok> = p.replay().iter().map(|e| Tok::of(e).unwrap()).collect();
        assert_eq!(typed, vec![Tok::Char('1'), Tok::Char('2'), Tok::Char('g')]);
    }

    #[test]
    fn digits_build_a_count_and_a_leading_zero_is_a_key() {
        let mut p = Pending::default();
        assert!(!p.digit('0'), "a leading 0 is a key");
        assert!(p.digit('2') && p.digit('0'));
        assert_eq!((p.count(), p.echo()), (Some(20), "20".to_string()));
        p.push(Tok::Char('g'));
        assert!(!p.digit('3'), "g3 is not a count");
        assert_eq!(p.echo(), "20g");
        p.clear();
        assert!(p.is_idle());
    }

    #[test]
    fn counts_saturate() {
        let mut p = Pending::default();
        for _ in 0..30 {
            assert!(p.digit('9'));
        }
        assert_eq!(p.count(), Some(MAX_COUNT));
    }
}
