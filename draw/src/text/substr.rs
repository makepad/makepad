use std::{
    ops::{Deref, RangeBounds},
    rc::Rc,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Substr {
    string: Rc<str>,
    start: usize,
    end: usize,
}

impl Substr {
    pub fn substr(&self, range: impl RangeBounds<usize>) -> Substr {
        use std::ops::Bound;

        let start = match range.start_bound() {
            Bound::Included(&start) => start,
            Bound::Excluded(&start) => start + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&end) => end + 1,
            Bound::Excluded(&end) => end,
            Bound::Unbounded => self.len(),
        };
        assert!(start <= end);
        assert!(end <= self.len());
        Substr {
            string: self.string.clone(),
            start: self.start + start,
            end: self.start + end,
        }
    }
}

impl Deref for Substr {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.string[self.start..self.end]
    }
}

impl From<&str> for Substr {
    fn from(string: &str) -> Self {
        Substr::from(Rc::from(string))
    }
}

impl From<Rc<str>> for Substr {
    fn from(string: Rc<str>) -> Self {
        let len = string.len();
        Self {
            string,
            start: 0,
            end: len,
        }
    }
}
