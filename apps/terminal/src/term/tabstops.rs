//! Tab stop tracking. Port of ghostty `src/terminal/Tabstops.zig` with a
//! plain bit-vec instead of the prealloc scheme.

pub const TABSTOP_INTERVAL: usize = 8;

#[derive(Clone, Debug)]
pub struct Tabstops {
    cols: usize,
    stops: Vec<bool>,
}

impl Tabstops {
    pub fn new(cols: usize) -> Self {
        let mut t = Self {
            cols,
            stops: vec![false; cols],
        };
        t.reset(TABSTOP_INTERVAL);
        t
    }

    /// Reset to a stop at every `interval` columns (0 = clear all).
    pub fn reset(&mut self, interval: usize) {
        self.stops.iter_mut().for_each(|s| *s = false);
        if interval > 0 {
            let mut i = 0;
            while i < self.cols {
                self.stops[i] = true;
                i += interval;
            }
        }
    }

    pub fn get(&self, col: usize) -> bool {
        self.stops.get(col).copied().unwrap_or(false)
    }

    pub fn set(&mut self, col: usize) {
        if col < self.cols {
            self.stops[col] = true;
        }
    }

    pub fn unset(&mut self, col: usize) {
        if col < self.cols {
            self.stops[col] = false;
        }
    }

    pub fn clear_all(&mut self) {
        self.stops.iter_mut().for_each(|s| *s = false);
    }

    /// Resize, preserving existing stops; new columns get default-interval
    /// stops (matches ghostty resize behavior).
    pub fn resize(&mut self, cols: usize) {
        let old = self.cols;
        self.stops.resize(cols, false);
        self.cols = cols;
        if cols > old {
            for i in old..cols {
                if i % TABSTOP_INTERVAL == 0 {
                    self.stops[i] = true;
                }
            }
        }
    }

    /// Next tab stop strictly after `col`, clamped to the last column.
    pub fn next_after(&self, col: usize) -> usize {
        for i in (col + 1)..self.cols {
            if self.stops[i] {
                return i;
            }
        }
        self.cols.saturating_sub(1)
    }

    /// Previous tab stop strictly before `col`, clamped to column 0 (CBT).
    pub fn prev_before(&self, col: usize) -> usize {
        for i in (0..col).rev() {
            if self.stops[i] {
                return i;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_every_8() {
        let t = Tabstops::new(20);
        assert!(t.get(0) && t.get(8) && t.get(16));
        assert!(!t.get(4));
        assert_eq!(t.next_after(0), 8);
        assert_eq!(t.next_after(8), 16);
        assert_eq!(t.next_after(17), 19);
        assert_eq!(t.prev_before(9), 8);
        assert_eq!(t.prev_before(3), 0);
    }

    #[test]
    fn resize_adds_default_stops() {
        let mut t = Tabstops::new(8);
        t.resize(30);
        assert!(t.get(16) && t.get(24));
    }
}
