use {
    crate::Node,
    std::ops::{Add, AddAssign, Sub, SubAssign},
};

#[derive(Clone, Copy)]
pub(crate) struct Info {
    pub(crate) byte_count: usize,
    pub(crate) char_count: usize,
}

impl Info {
    pub(crate) fn new() -> Self {
        Self {
            byte_count: 0,
            char_count: 0,
        }
    }
}

impl<'a> From<&'a str> for Info {
    fn from(string: &str) -> Self {
        Self {
            byte_count: string.len(),
            char_count: string.chars().count(),
        }
    }
}

impl<'a> From<&'a [Node]> for Info {
    fn from(nodes: &[Node]) -> Self {
        let mut info = Info::new();
        for node in nodes {
            info += node.info();
        }
        info
    }
}

impl Add for Info {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self {
            byte_count: self.byte_count + other.byte_count,
            char_count: self.char_count + other.char_count,
        }
    }
}

impl AddAssign for Info {
    fn add_assign(&mut self, other: Info) {
        *self = *self + other;
    }
}

impl Sub for Info {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self {
            byte_count: self.byte_count - other.byte_count,
            char_count: self.char_count - other.char_count,
        }
    }
}

impl SubAssign for Info {
    fn sub_assign(&mut self, other: Info) {
        *self = *self - other;
    }
}
