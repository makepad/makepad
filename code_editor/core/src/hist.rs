use crate::{Diff, Sel, Text};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hist {
    revs: Vec<Rev>,
    rev: usize,
}

impl Hist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Diff, Sel)> {
        if self.rev == 0 {
            return None;
        }
        let diff = self.revs[self.rev].diff.clone().invert(&text);
        self.rev -= 1;
        let sel = self.revs[self.rev].sel.clone();
        text.apply_diff(diff.clone());
        Some((diff, sel))
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Diff, Sel)> {
        if self.rev == self.revs.len() {
            return None;
        }
        self.rev += 1;
        let diff = self.revs[self.rev].diff.clone();
        let sel = self.revs[self.rev].sel.clone();
        text.apply_diff(diff.clone());
        Some((diff, sel))
    }

    pub fn commit(&mut self, diff: Diff, sel: Sel) {
        self.rev += 1;
        self.revs.truncate(self.rev);
        self.revs.push(Rev { diff, sel });
    }
}

impl Default for Hist {
    fn default() -> Self {
        Self {
            revs: vec![Rev::default()],
            rev: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Rev {
    diff: Diff,
    sel: Sel,
}
