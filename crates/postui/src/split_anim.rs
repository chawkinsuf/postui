//! The split's three anim values: where a [`SplitState`] puts
//! [`AnimKey::PaneCollapse`], [`AnimKey::ResponseCollapse`] and
//! [`AnimKey::SplitRatio`]. It names both the split model and the anim
//! keys, so it sits in neither.
use crate::anim::{AnimKey, Anims};
use crate::split::SplitState;
use std::time::Instant;

/// The split's three anim values, one field per key, named so each reader
/// takes the one it means rather than a position.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SplitAnims {
    /// [`AnimKey::PaneCollapse`].
    pub editor: f32,
    /// [`AnimKey::ResponseCollapse`].
    pub response: f32,
    /// [`AnimKey::SplitRatio`].
    pub ratio: f32,
}

impl SplitAnims {
    /// Where each anim settles for the split `s`: a collapse at `1.0`
    /// hidden and `0.0` shown, the ratio as is.
    pub fn settled(s: SplitState) -> Self {
        let pole = |hidden: bool| if hidden { 1.0 } else { 0.0 };
        SplitAnims {
            editor: pole(s.editor_minimized),
            response: pole(s.response_minimized),
            ratio: s.ratio,
        }
    }

    /// Each value with its key, in field order.
    pub fn keyed(self) -> [(AnimKey, f32); 3] {
        [
            (AnimKey::PaneCollapse, self.editor),
            (AnimKey::ResponseCollapse, self.response),
            (AnimKey::SplitRatio, self.ratio),
        ]
    }

    /// The values `anims` holds at `now`, each falling back to this one's
    /// when its key is unset.
    pub fn drawn(self, anims: &Anims, now: Instant) -> Self {
        let [editor, response, ratio] = self.keyed().map(|(key, v)| anims.value_or(key, now, v));
        SplitAnims { editor, response, ratio }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task #253: each pane's collapse key settles at 1.0 hidden and 0.0
    /// shown, and the ratio key at the ratio.
    #[test]
    fn split_anims_settle_each_key_at_the_split() {
        let s = SplitState { editor_minimized: true, response_minimized: false, ratio: 0.3 };
        assert_eq!(
            SplitAnims::settled(s).keyed(),
            [
                (AnimKey::PaneCollapse, 1.0),
                (AnimKey::ResponseCollapse, 0.0),
                (AnimKey::SplitRatio, 0.3)
            ]
        );
        let s = SplitState { editor_minimized: false, response_minimized: true, ratio: 0.7 };
        assert_eq!(
            SplitAnims::settled(s).keyed(),
            [
                (AnimKey::PaneCollapse, 0.0),
                (AnimKey::ResponseCollapse, 1.0),
                (AnimKey::SplitRatio, 0.7)
            ]
        );
    }

    /// Task #253: `drawn` reads each field from its own key, and keeps the
    /// settled value for a key that is unset.
    #[test]
    fn split_anims_drawn_reads_each_field_from_its_own_key() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::PaneCollapse, 0.25);
        a.snap(AnimKey::SplitRatio, 0.75);
        let settled = SplitAnims { editor: 1.0, response: 0.5, ratio: 0.0 };
        assert_eq!(settled.drawn(&a, t0), SplitAnims { editor: 0.25, response: 0.5, ratio: 0.75 });
    }
}
