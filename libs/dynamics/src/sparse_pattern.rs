//! Row-compressed sparsity patterns checked once when they are built.
//!
//! The solver's sparse kernels walk a pattern's columns many times a step. A pattern checked here (every
//! column below the column count, the row addresses in order and inside the column list, a supernode's rows
//! as many as there are and holding its first row's count) lets a kernel index by its columns unchecked where
//! that measurably pays (the transpose, the symbolic factor, the numeric factor's dense accumulator): such a
//! kernel checks only, in O(1) at its entry, that its value and output slices are as long as the pattern needs. The fields are private and nothing hands out a mutable reference, so a checked pattern
//! cannot change; to rebuild one, its arrays go back into a [`PatternBuf`] and are checked again.

/// Why a pattern was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternError {
    /// `rownnz`, `rowadr` or `rowsuper` is shorter than the row count.
    Lengths,
    /// The row's count is negative, its address is negative or before the row above's, or its columns run past
    /// the column list.
    Row(usize),
    /// A column of the row is negative or not below the column count.
    Column(usize),
    /// The row's supernode runs past the last row, or a row of it differs in its count or in the rows that
    /// follow it.
    Supernode(usize),
}

/// A checked pattern borrowed from where it was built: `nr` rows, row `r` holding `rownnz[r]` columns at
/// `colind[rowadr[r]..]`, each below `nc`; `rowsuper[r]`, when there is one, the number of rows after `r` in
/// its supernode (rows that share its count).
#[derive(Clone, Copy, Debug)]
pub struct PatternRef<'a> {
    rownnz: &'a [i32],
    rowadr: &'a [i32],
    colind: &'a [i32],
    rowsuper: Option<&'a [i32]>,
    nr: usize,
    nc: usize,
    end: usize,
    contiguous: bool,
}

impl<'a> PatternRef<'a> {
    /// Checks the pattern: O(nr + nnz), once where it is built.
    pub fn check(
        rownnz: &'a [i32],
        rowadr: &'a [i32],
        colind: &'a [i32],
        rowsuper: Option<&'a [i32]>,
        nr: usize,
        nc: usize,
    ) -> Result<Self, PatternError> {
        if rownnz.len() < nr || rowadr.len() < nr || rowsuper.is_some_and(|s| s.len() < nr) {
            return Err(PatternError::Lengths);
        }
        let (rownnz, rowadr) = (&rownnz[..nr], &rowadr[..nr]);
        let rowsuper = rowsuper.map(|s| &s[..nr]);
        // rows: in order and inside the column list; contiguous when each starts where the one above ends
        let mut end = 0;
        let mut prev_adr = 0;
        let mut contiguous = true;
        for r in 0..nr {
            let (nnz, adr) = (rownnz[r], rowadr[r]);
            if nnz < 0 || adr < prev_adr || adr as usize + nnz as usize > colind.len() {
                return Err(PatternError::Row(r));
            }
            contiguous &= adr as usize == end;
            prev_adr = adr;
            end = end.max(adr as usize + nnz as usize);
        }
        // columns: one sweep of the whole list when the rows tile it, else row by row (a gap between rows holds
        // whatever was there before and is never read as a column)
        let below = |cols: &[i32]| cols.iter().fold(0, |m: u32, &c| m.max(c as u32)) < nc as u32 || cols.is_empty();
        if !(contiguous && below(&colind[..end])) {
            for r in 0..nr {
                let adr = rowadr[r] as usize;
                if !below(&colind[adr..adr + rownnz[r] as usize]) {
                    return Err(PatternError::Column(r));
                }
            }
        }
        if let Some(s) = rowsuper {
            // a supernode of k rows after r: the next row's is one shorter and holds as many columns, so by
            // induction every row of it does
            for r in 0..nr {
                let k = s[r];
                if k < 0 || r + k as usize >= nr || (k > 0 && (s[r + 1] != k - 1 || rownnz[r + 1] != rownnz[r])) {
                    return Err(PatternError::Supernode(r));
                }
            }
        }
        Ok(PatternRef { rownnz, rowadr, colind: &colind[..end], rowsuper, nr, nc, end, contiguous })
    }
    #[inline]
    pub fn nr(&self) -> usize {
        self.nr
    }
    #[inline]
    pub fn nc(&self) -> usize {
        self.nc
    }
    /// One past the last address any row uses: a value slice of this length holds every row.
    #[inline]
    pub fn end(&self) -> usize {
        self.end
    }
    /// The rows follow one another from address 0, so every address below `end` is a column of a row.
    #[inline]
    pub fn contiguous(&self) -> bool {
        self.contiguous
    }
    #[inline]
    pub fn rownnz(&self) -> &'a [i32] {
        self.rownnz
    }
    #[inline]
    pub fn rowadr(&self) -> &'a [i32] {
        self.rowadr
    }
    /// The column list up to `end`.
    #[inline]
    pub fn colind(&self) -> &'a [i32] {
        self.colind
    }
    #[inline]
    pub fn rowsuper(&self) -> Option<&'a [i32]> {
        self.rowsuper
    }
}

/// A pattern's arrays while it is built: written freely, then checked into a [`Pattern`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatternBuf {
    pub rownnz: Vec<i32>,
    pub rowadr: Vec<i32>,
    pub colind: Vec<i32>,
    pub rowsuper: Vec<i32>,
}

impl PatternBuf {
    /// The four arrays, to give back to a pool.
    pub fn into_parts(self) -> [Vec<i32>; 4] {
        [self.rownnz, self.rowadr, self.colind, self.rowsuper]
    }
}

/// A checked pattern that owns its arrays (pooled vectors, given back with [`Pattern::into_buf`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pattern {
    buf: PatternBuf,
    has_super: bool,
    nr: usize,
    nc: usize,
    end: usize,
    contiguous: bool,
}

impl Pattern {
    /// Checks the pattern (as [`PatternRef::check`], with `rowsuper` when `has_super`) and keeps it. A refused
    /// pattern hands its arrays back.
    pub fn new(buf: PatternBuf, nr: usize, nc: usize, has_super: bool) -> Result<Self, (PatternError, PatternBuf)> {
        let checked = PatternRef::check(&buf.rownnz, &buf.rowadr, &buf.colind, has_super.then_some(&buf.rowsuper[..]), nr, nc)
            .map(|p| (p.end, p.contiguous));
        match checked {
            Ok((end, contiguous)) => Ok(Pattern { buf, has_super, nr, nc, end, contiguous }),
            Err(e) => Err((e, buf)),
        }
    }
    /// The checked view the kernels take; no check again.
    #[inline]
    pub fn view(&self) -> PatternRef<'_> {
        PatternRef {
            rownnz: &self.buf.rownnz[..self.nr],
            rowadr: &self.buf.rowadr[..self.nr],
            colind: &self.buf.colind[..self.end],
            rowsuper: self.has_super.then(|| &self.buf.rowsuper[..self.nr]),
            nr: self.nr,
            nc: self.nc,
            end: self.end,
            contiguous: self.contiguous,
        }
    }
    #[inline]
    pub fn nr(&self) -> usize {
        self.nr
    }
    #[inline]
    pub fn nc(&self) -> usize {
        self.nc
    }
    #[inline]
    pub fn rownnz(&self) -> &[i32] {
        &self.buf.rownnz[..self.nr]
    }
    #[inline]
    pub fn rowadr(&self) -> &[i32] {
        &self.buf.rowadr[..self.nr]
    }
    /// The column list up to the end of the last row.
    #[inline]
    pub fn colind(&self) -> &[i32] {
        &self.buf.colind[..self.end]
    }
    #[inline]
    pub fn rowsuper(&self) -> Option<&[i32]> {
        self.has_super.then(|| &self.buf.rowsuper[..self.nr])
    }
    /// The arrays, to rebuild the pattern or give them back to a pool.
    pub fn into_buf(self) -> PatternBuf {
        self.buf
    }
}
