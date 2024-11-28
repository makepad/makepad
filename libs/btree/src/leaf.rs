use super::Info;

pub trait Leaf {
    const MAX_LEN: usize;

    type Info: Info;

    fn new() -> Self;

    fn is_at_least_half_full(&self) -> bool;

    fn can_split_at(&self, index: usize) -> bool;

    fn len(&self) -> usize;

    fn info_to(&self, end: usize) -> Self::Info;

    fn move_left(&mut self, other: &mut Self, end: usize);

    fn move_right(&mut self, other: &mut Self, start: usize);

    fn remove_from(&mut self, start: usize);

    fn remove_to(&mut self, end: usize);

    fn split_off(&mut self, index: usize) -> Self;

    fn is_full(&self) -> bool {
        self.len() == Self::MAX_LEN
    }

    fn info(&self) -> Self::Info {
        self.info_to(self.len())
    }

    fn prepend(&mut self, other: &mut Self) {
        debug_assert!(self.len() + other.len() <= Self::MAX_LEN);
        other.move_right(self, 0);
    }

    fn append(&mut self, other: &mut Self) {
        debug_assert!(self.len() + other.len() <= Self::MAX_LEN);
        self.move_left(other, other.len());
    }

    fn distribute(&mut self, other: &mut Self) {
        if self.len() < other.len() {
            let mut end = (other.len() - self.len()) / 2;
            while !other.can_split_at(end) {
                end -= 1;
            }
            self.move_left(other, end);
        } else if self.len() > other.len() {
            let mut start = (self.len() + other.len()) / 2;
            while !self.can_split_at(start) {
                start += 1;
            }
            self.move_right(other, start);
        }
    }

    fn prepend_distribute(&mut self, other: &mut Self) -> bool {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.prepend(other);
            true
        } else {
            other.distribute(self);
            false
        }
    }

    fn append_distribute(&mut self, other: &mut Self) -> bool {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.append(other);
            true
        } else {
            self.distribute(other);
            false
        }
    }
}
