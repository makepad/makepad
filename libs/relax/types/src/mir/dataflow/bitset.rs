//! Dense sets and row-major matrices: rustc_index/src/bit_set.rs:116,1397.
//! One dense representation; indices are concrete usize values rather than Idx parameters.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitSet {
    size: usize,
    words: Vec<u64>,
}

impl BitSet {
    pub fn new_empty(size: usize) -> Self {
        Self { size, words: vec![0; size.div_ceil(64)] }
    }

    pub fn new_filled(size: usize) -> Self {
        let mut set = Self::new_empty(size);
        set.insert_all();
        set
    }

    pub fn domain_size(&self) -> usize { self.size }

    pub fn contains(&self, bit: usize) -> bool {
        assert!(bit < self.size);
        self.words[bit / 64] & (1 << (bit % 64)) != 0
    }

    pub fn insert(&mut self, bit: usize) -> bool {
        assert!(bit < self.size);
        let word = &mut self.words[bit / 64];
        let old = *word;
        *word |= 1 << (bit % 64);
        old != *word
    }

    pub fn remove(&mut self, bit: usize) -> bool {
        assert!(bit < self.size);
        let word = &mut self.words[bit / 64];
        let old = *word;
        *word &= !(1 << (bit % 64));
        old != *word
    }

    pub fn clear(&mut self) { self.words.fill(0); }

    pub fn insert_all(&mut self) {
        self.words.fill(u64::MAX);
        if self.size % 64 != 0 {
            *self.words.last_mut().unwrap() &= (1 << (self.size % 64)) - 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        for word in &self.words {
            if *word != 0 { return false; }
        }
        true
    }

    pub fn count(&self) -> usize {
        let mut count = 0;
        for word in &self.words { count += word.count_ones() as usize; }
        count
    }

    pub fn union(&mut self, other: &Self) -> bool { self.combine(other, SetOp::Union) }
    pub fn intersect(&mut self, other: &Self) -> bool { self.combine(other, SetOp::Intersect) }
    pub fn subtract(&mut self, other: &Self) -> bool { self.combine(other, SetOp::Subtract) }

    fn combine(&mut self, other: &Self, op: SetOp) -> bool {
        assert_eq!(self.size, other.size);
        let mut changed = 0;
        for i in 0..self.words.len() {
            let old = self.words[i];
            self.words[i] = match op {
                SetOp::Union => old | other.words[i],
                SetOp::Intersect => old & other.words[i],
                SetOp::Subtract => old & !other.words[i],
            };
            changed |= old ^ self.words[i];
        }
        changed != 0
    }

    pub fn iter(&self) -> BitIter<'_> { BitIter::new(&self.words) }
}

enum SetOp { Union, Intersect, Subtract }

pub struct BitIter<'a> {
    words: &'a [u64],
    index: usize,
    pending: u64,
}

impl<'a> BitIter<'a> {
    fn new(words: &'a [u64]) -> Self {
        Self { words, index: 0, pending: words.first().copied().unwrap_or(0) }
    }
}

impl Iterator for BitIter<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        if self.index >= self.words.len() { return None; }
        loop {
            if self.pending != 0 {
                let bit = self.pending.trailing_zeros() as usize;
                self.pending &= self.pending - 1;
                return Some(self.index * 64 + bit);
            }
            self.index += 1;
            if self.index >= self.words.len() { return None; }
            self.pending = self.words[self.index];
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitMatrix {
    rows: usize,
    columns: usize,
    words: Vec<u64>,
}

impl BitMatrix {
    pub fn new(rows: usize, columns: usize) -> Self {
        Self { rows, columns, words: vec![0; rows * columns.div_ceil(64)] }
    }

    fn start(&self, row: usize) -> usize {
        assert!(row < self.rows);
        row * self.columns.div_ceil(64)
    }

    pub fn contains(&self, row: usize, column: usize) -> bool {
        assert!(column < self.columns);
        self.words[self.start(row) + column / 64] & (1 << (column % 64)) != 0
    }

    pub fn insert(&mut self, row: usize, column: usize) -> bool {
        assert!(column < self.columns);
        let i = self.start(row) + column / 64;
        let old = self.words[i];
        self.words[i] |= 1 << (column % 64);
        old != self.words[i]
    }

    pub fn union_rows(&mut self, read: usize, write: usize) -> bool {
        let src = self.start(read);
        let dst = self.start(write);
        let mut changed = 0;
        for i in 0..self.columns.div_ceil(64) {
            let old = self.words[dst + i];
            self.words[dst + i] |= self.words[src + i];
            changed |= old ^ self.words[dst + i];
        }
        changed != 0
    }

    pub fn union_row_with(&mut self, read: &BitSet, write: usize) -> bool {
        assert_eq!(read.size, self.columns);
        let dst = self.start(write);
        let mut changed = 0;
        for i in 0..read.words.len() {
            let old = self.words[dst + i];
            self.words[dst + i] |= read.words[i];
            changed |= old ^ self.words[dst + i];
        }
        changed != 0
    }

    pub fn iter(&self, row: usize) -> BitIter<'_> {
        let start = self.start(row);
        BitIter::new(&self.words[start..start + self.columns.div_ceil(64)])
    }
}
