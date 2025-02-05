use std::{
    hash::{Hash, Hasher},
    ops::{Deref, RangeBounds},
    rc::Rc,
};

#[derive(Clone, Debug)]
pub struct Substr {
    string: Rc<str>,
    start: usize,
    end: usize,
}

impl Substr {
    pub fn split_at(self, index: usize) -> (Substr, Substr) {
        (
            Self {
                string: self.string.clone(),
                start: self.start,
                end: index,
            },
            Self {
                string: self.string,
                start: index,
                end: self.end,
            },
        )
    }

    pub fn as_str(&self) -> &str {
        &self.string[self.start..self.end]
    }

    pub fn shallow_eq(&self, other: &Self) -> bool {
        if !Rc::ptr_eq(&self.string, &other.string) {
            return false;
        }
        if self.start != other.start {
            return false;
        }
        if self.end != other.end {
            return false;
        }
        true
    }

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
        self.as_str()
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

impl Eq for Substr {}

impl Hash for Substr {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.as_str().hash(hasher)
    }
}

impl Ord for Substr {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialEq for Substr {
    fn eq(&self, other: &Self) -> bool {
        self.shallow_eq(other) || self.as_str().eq(other.as_str())
    }
}

impl PartialOrd for Substr {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.as_str().partial_cmp(other.as_str())
    }
}
