use {
    crate::Info,
    std::{ops::Deref, sync::Arc},
};

#[derive(Clone, Debug)]
pub(crate) struct Leaf {
    string: Arc<String>,
}

impl Leaf {
    #[cfg(not(test))]
    pub(crate) const MAX_LEN: usize = 1024;
    #[cfg(test)]
    pub(crate) const MAX_LEN: usize = 8;

    pub(crate) fn new() -> Self {
        Leaf::from(Arc::new(String::new()))
    }

    pub(crate) fn info(&self) -> Info {
        Info::from(self.string.as_str())
    }

    pub(crate) fn as_str(&self) -> &str {
        self.string.as_str()
    }

    pub(crate) fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.append(other);
            None
        } else {
            self.distribute(&mut other);
            Some(other)
        }
    }

    pub(crate) fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        self.shift_right(&mut other, at);
        other
    }

    pub(crate) fn truncate_front(&mut self, start: usize) {
        Arc::make_mut(&mut self.string).drain(..start);
    }

    pub(crate) fn truncate_back(&mut self, end: usize) {
        Arc::make_mut(&mut self.string).truncate(end);
    }

    fn append(&mut self, mut other: Self) {
        let other_len = other.len();
        self.shift_left(&mut other, other_len);
    }

    fn distribute(&mut self, other: &mut Self) {
        use {crate::StrUtils, std::cmp::Ordering};

        match self.len().cmp(&other.len()) {
            Ordering::Less => {
                let mut end = (other.len() - self.len()) / 2;
                while !other.string.can_split_at(end) {
                    end -= 1;
                }
                self.shift_left(other, end);
            }
            Ordering::Greater => {
                let mut start = (self.len() + other.len()) / 2;
                while !self.string.can_split_at(start) {
                    start += 1;
                }
                self.shift_right(other, start);
            }
            _ => {}
        }
    }

    fn shift_left(&mut self, other: &mut Self, end: usize) {
        Arc::make_mut(&mut self.string).push_str(&other[..end]);
        Arc::make_mut(&mut other.string).replace_range(..end, "");
    }

    fn shift_right(&mut self, other: &mut Self, start: usize) {
        Arc::make_mut(&mut other.string).replace_range(..0, &self[start..]);
        Arc::make_mut(&mut self.string).truncate(start);
    }
}

#[cfg(fuzzing)]
impl Leaf {
    pub(crate) fn assert_valid(&self, height: usize) {
        assert!(height == 0);
    }

    pub(crate) fn is_at_least_half_full(&self) -> bool {
        self.len() >= Self::MAX_LEN / 2 - 4
    }
}

impl From<Arc<String>> for Leaf {
    fn from(string: Arc<String>) -> Self {
        Self { string }
    }
}

impl Deref for Leaf {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}
