//! Sets of indices stored as sorted, non-adjacent inclusive intervals: rustc_index/src/interval.rs
//! (`IntervalSet`), the row type of rustc's `SparseIntervalMatrix` that NLL keeps region liveness in.
//! A region is live over a few runs of consecutive points, so a row costs O(runs), not O(points):
//! dense rows made the liveness of a fn with thousands of `else if` arms quadratic in memory.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntervalSet {
    /// (start, end), both inclusive; sorted, and the end of one + 1 < the start of the next
    map: Vec<(u32, u32)>,
    domain: usize,
}

impl IntervalSet {
    pub fn new(domain: usize) -> Self { Self { map: Vec::new(), domain } }

    pub fn new_filled(domain: usize) -> Self {
        let mut set = Self::new(domain);
        set.insert_all();
        set
    }

    pub fn domain_size(&self) -> usize { self.domain }

    pub fn clear(&mut self) { self.map.clear(); }

    /// The points, in order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.iter_intervals().flatten()
    }

    /// The intervals, in order, as half-open ranges.
    pub fn iter_intervals(&self) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        self.map.iter().map(|&(start, end)| start as usize..end as usize + 1)
    }

    /// Returns true if the set grew.
    pub fn insert(&mut self, point: usize) -> bool { self.insert_range(point, point) }

    /// Inserts `start..=end`; returns true if the set grew.
    pub fn insert_range(&mut self, start: usize, end: usize) -> bool {
        if start > end || end >= self.domain { return false; }
        let (start, end) = (start as u32, end as u32);
        // the first range that starts non-adjacently after our end
        let next = self.map.partition_point(|r| r.0 <= end + 1);
        let result = if let Some(right) = next.checked_sub(1) {
            let (prev_start, prev_end) = self.map[right];
            if prev_end + 1 >= start {
                if start < prev_start {
                    // the first range that ends non-adjacently before our start (left <= right)
                    let left = self.map.partition_point(|l| l.1 + 1 < start);
                    let min = self.map[left].0.min(start);
                    let max = prev_end.max(end);
                    self.map[right] = (min, max);
                    if left != right { self.map.drain(left..right); }
                    true
                } else if end > prev_end {
                    self.map[right].1 = end;
                    true
                } else {
                    false
                }
            } else {
                self.map.insert(right + 1, (start, end));
                true
            }
        } else {
            self.map.insert(next, (start, end));
            true
        };
        debug_assert!(self.check_invariants(), "wrong intervals after insert {start}..={end}: {self:?}");
        result
    }

    pub fn contains(&self, needle: usize) -> bool {
        let needle = needle as u32;
        let Some(last) = self.map.partition_point(|r| r.0 <= needle).checked_sub(1) else { return false };
        needle <= self.map[last].1
    }

    /// Does `self` contain every point of `other`?
    pub fn superset(&self, other: &IntervalSet) -> bool {
        let mut sup = self.map.iter().peekable();
        'sub: for &(s, e) in &other.map {
            while let Some(&&(ss, se)) = sup.peek() {
                if se < s { sup.next(); continue; }
                if ss <= s && e <= se { continue 'sub; }
                return false;
            }
            return false;
        }
        true
    }

    pub fn is_empty(&self) -> bool { self.map.is_empty() }

    pub fn insert_all(&mut self) {
        self.clear();
        if let Some(end) = self.domain.checked_sub(1) { self.map.push((0, end as u32)); }
    }

    /// Returns true if the set grew.
    pub fn union(&mut self, other: &IntervalSet) -> bool {
        assert_eq!(self.domain, other.domain);
        if self.superset(other) { return false; }
        // (insert the shorter list into a copy of the longer one, as rustc does)
        if self.map.len() < other.map.len() {
            let mine = std::mem::replace(&mut self.map, other.map.clone());
            for &(s, e) in &mine { self.insert_range(s as usize, e as usize); }
        } else {
            for &(s, e) in &other.map { self.insert_range(s as usize, e as usize); }
        }
        true
    }

    fn check_invariants(&self) -> bool {
        let mut prev: Option<u32> = None;
        for &(s, e) in &self.map {
            if s > e || e as usize >= self.domain { return false; }
            if let Some(p) = prev { if p + 1 >= s { return false; } }
            prev = Some(e);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::IntervalSet;

    #[test]
    fn insert_merge_contains() {
        let mut s = IntervalSet::new(100);
        assert!(s.insert(5));
        assert!(s.insert(7));
        assert!(!s.insert(5));
        assert!(s.insert(6));
        assert_eq!(s.iter_intervals().collect::<Vec<_>>(), vec![5..8]);
        assert!(s.insert_range(20, 30));
        assert!(s.insert_range(2, 21));
        assert_eq!(s.iter_intervals().collect::<Vec<_>>(), vec![2..31]);
        assert!(s.contains(2) && s.contains(30) && !s.contains(31) && !s.contains(1));
        assert!(!s.insert_range(3, 10));
    }

    #[test]
    fn union_and_superset() {
        let mut a = IntervalSet::new(50);
        a.insert_range(0, 3);
        a.insert_range(10, 12);
        let mut b = IntervalSet::new(50);
        b.insert(1);
        b.insert(11);
        assert!(a.superset(&b));
        assert!(!b.superset(&a));
        b.insert(20);
        b.insert(30);
        b.insert(40);
        assert!(!a.superset(&b));
        let mut c = b.clone();
        assert!(c.union(&a));
        assert!(c.superset(&a) && c.superset(&b));
        assert!(!c.union(&a));
        let mut d = a.clone();
        assert!(d.union(&b));
        assert_eq!(d, c);
        let mut e = IntervalSet::new(50);
        e.insert(1);
        let mut f = IntervalSet::new(50);
        f.insert(1);
        f.insert(3);
        f.insert(5);
        let mut g = e.clone();
        assert!(!f.clone().union(&e));
        assert!(g.union(&f));
    }
}
