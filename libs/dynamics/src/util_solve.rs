//! Dense, sparse, band and LU solvers, updates, and constrained quadratic solvers.
//!
//! Translated from `src/engine/engine_util_solve.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::{
    data::Data,
    memory::Scratch,
    types::MINVAL,
    util_blas::*,
    util_misc::{copy_int, max, min, scatter},
    util_sparse::*,
};

pub fn chol_factor(mat: &mut [f64], n: usize, mindiag: f64) -> i32 {
    let mut rank = n as i32;
    for j in 0..n {
        let mut tmp = mat[j * (n + 1)];
        if j != 0 {
            tmp -= dot(&mat[j * n..], &mat[j * n..], j);
        }
        let deficient = !(tmp >= mindiag);
        if deficient {
            tmp = mindiag;
            rank -= 1;
        }
        mat[j * (n + 1)] = tmp.sqrt();
        if deficient {
            for i in j + 1..n {
                mat[i * n + j] = 0.0;
            }
        } else {
            tmp = 1.0 / mat[j * (n + 1)];
            for i in j + 1..n {
                mat[i * n + j] = (mat[i * n + j] - dot(&mat[i * n..], &mat[j * n..], j)) * tmp;
            }
        }
    }
    rank
}

pub fn chol_solve(res: &mut [f64], mat: &[f64], vec: &[f64], n: usize) {
    copy(res, vec, n);
    for i in 0..n {
        if i != 0 {
            res[i] -= dot(&mat[i * n..], res, i);
        }
        res[i] /= mat[i * (n + 1)];
    }
    for i in (0..n).rev() {
        res[i] /= mat[i * (n + 1)];
        if i != 0 && res[i] != 0.0 {
            let scl = -res[i];
            add_to_scl(res, &mat[i * n..], scl, i);
        }
    }
}

pub fn chol_update(mat: &mut [f64], x: &mut [f64], n: usize, flg_plus: i32) -> i32 {
    let mut rank = n as i32;
    for k in 0..n {
        if x[k] != 0.0 {
            let lkk = mat[k * (n + 1)];
            let mut tmp = lkk * lkk
                + if flg_plus != 0 {
                    x[k] * x[k]
                } else {
                    -x[k] * x[k]
                };
            if !(tmp >= MINVAL) {
                tmp = MINVAL;
                rank -= 1;
            }
            let r = tmp.sqrt();
            let c = r / lkk;
            let cinv = 1.0 / c;
            let s = x[k] / lkk;
            mat[k * (n + 1)] = r;
            let s_signed = if flg_plus != 0 { s } else { -s };
            for i in k + 1..n {
                let m = (mat[i * n + k] + s_signed * x[i]) * cinv;
                mat[i * n + k] = m;
                x[i] = c * x[i] - s * m;
            }
        }
    }
    rank
}

pub fn chol_factor_sparse(
    mat: &mut [f64],
    n: usize,
    mindiag: f64,
    rownnz: &mut [i32],
    rowadr: &[i32],
    colind: &mut [i32],
    d: &mut Data,
) -> i32 {
    let mut rank = n as i32;
    let mut src = d.scratch.num(n);
    let mut src_ind = d.scratch.int(n);
    for r in (0..n).rev() {
        let nnz = rownnz[r] as usize;
        let adr = rowadr[r] as usize;
        let mut tmp = mat[adr + nnz - 1];
        let deficient = !(tmp >= mindiag);
        if deficient {
            tmp = mindiag;
            rank -= 1;
        }
        mat[adr + nnz - 1] = tmp.sqrt();
        if deficient {
            zero(&mut mat[adr..], nnz - 1);
        } else {
            tmp = 1.0 / mat[adr + nnz - 1];
            for i in 0..nnz - 1 {
                mat[adr + i] *= tmp;
            }
        }
        // C passes distinct rows of the same storage. Row r stays unchanged during
        // these updates, so copy its input once into reusable scratch before writing.
        copy(&mut src, &mat[adr..], nnz - 1);
        copy_int(&mut src_ind, &colind[adr..], nnz - 1);
        for i in 0..nnz - 1 {
            let c = colind[adr + i] as usize;
            let scl = -mat[adr + i];
            let nnz_c = combine_sparse(
                &mut mat[rowadr[c] as usize..],
                &src,
                1.0,
                scl,
                rownnz[c] as usize,
                i + 1,
                &mut colind[rowadr[c] as usize..],
                &src_ind,
            );
            rownnz[c] = nnz_c as i32;
        }
    }
    d.scratch.put_num(src);
    d.scratch.put_int(src_ind);
    rank
}

fn upper_start(colind: &[i32], start: usize, end: usize, r: usize) -> usize {
    if start < end && colind[start] <= r as i32 {
        if colind[start] == r as i32 {
            return start + 1;
        }
        let mut lo = start + 1;
        let mut hi = end;
        while lo < hi {
            let mid = (lo + hi) >> 1;
            if colind[mid] <= r as i32 {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        return lo;
    }
    start
}

/// With no pool, scratch has 2*n integers for counting or 3*n for filling.
pub fn chol_factor_symbolic(
    mut l_colind: Option<&mut [i32]>,
    l_rownnz: &mut [i32],
    l_rowadr: &mut [i32],
    lt_colind: &mut [i32],
    lt_rownnz: &mut [i32],
    lt_rowadr: &mut [i32],
    lt_map: &mut [i32],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    n: usize,
    mut pool: Option<&mut Scratch>,
    scratch: &mut [i32],
) -> i32 {
    let size = if l_colind.is_some() { 3 * n } else { 2 * n };
    let mut owned = pool.as_deref_mut().map(|pool| pool.int(size));
    let work = owned.as_deref_mut().unwrap_or(scratch);
    let (parent, rest) = work.split_at_mut(n);
    let (flag, cursor) = rest.split_at_mut(n);
    if l_colind.is_none() {
        for r in (0..n).rev() {
            parent[r] = -1;
            flag[r] = r as i32;
            l_rownnz[r] = 1;
            let mut lt_nnz = 1;
            let end = (rowadr[r] + rownnz[r]) as usize;
            for c in upper_start(colind, rowadr[r] as usize, end, r)..end {
                let mut i = colind[c] as usize;
                while flag[i] != r as i32 {
                    l_rownnz[i] += 1;
                    lt_nnz += 1;
                    flag[i] = r as i32;
                    let p = parent[i];
                    if p == -1 {
                        parent[i] = r as i32;
                        break;
                    }
                    i = p as usize;
                }
            }
            lt_rownnz[r] = lt_nnz;
        }
        if let (Some(pool), Some(v)) = (pool, owned) {
            pool.put_int(v);
        }
        let mut nnz = l_rownnz[0];
        l_rowadr[0] = 0;
        lt_rowadr[0] = 0;
        for r in 1..n {
            l_rowadr[r] = l_rowadr[r - 1] + l_rownnz[r - 1];
            lt_rowadr[r] = lt_rowadr[r - 1] + lt_rownnz[r - 1];
            nnz += l_rownnz[r];
        }
        return nnz;
    }
    let l_colind = l_colind.as_deref_mut().unwrap();
    for r in (0..n).rev() {
        parent[r] = -1;
        flag[r] = r as i32;
        let diag_idx = l_rowadr[r] + l_rownnz[r] - 1;
        cursor[r] = diag_idx - 1;
        l_colind[diag_idx as usize] = r as i32;
        let mut lt_write = lt_rowadr[r] as usize;
        lt_colind[lt_write] = r as i32;
        lt_map[lt_write] = diag_idx;
        lt_write += 1;
        let end = (rowadr[r] + rownnz[r]) as usize;
        for c in upper_start(colind, rowadr[r] as usize, end, r)..end {
            let mut i = colind[c] as usize;
            while flag[i] != r as i32 {
                let l_idx = cursor[i];
                cursor[i] -= 1;
                l_colind[l_idx as usize] = r as i32;
                lt_colind[lt_write] = i as i32;
                lt_map[lt_write] = l_idx;
                lt_write += 1;
                flag[i] = r as i32;
                let p = parent[i];
                if p == -1 {
                    parent[i] = r as i32;
                    break;
                }
                i = p as usize;
            }
        }
    }
    if let (Some(pool), Some(v)) = (pool, owned) {
        pool.put_int(v);
    }
    // chol_factor_numeric indexes its dense accumulator by these columns unchecked: each is a row below n
    // by construction, checked here once per pattern (over the whole slice, so no stale entry escapes).
    assert!(l_colind.iter().all(|&c| (c as u32 as usize) < n), "chol_factor_symbolic: a column of L is not below n");
    0
}

pub fn chol_factor_symbolic_blocked(
    l_colind: Option<&mut [i32]>,
    l_rownnz: &mut [i32],
    l_rowadr: &mut [i32],
    bt_adr: &mut [i32],
    bt_row: &mut [i32],
    bt_off: &mut [i32],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    nb: usize,
    bs: usize,
    scratch: &mut [i32],
) -> i32 {
    let (parent, rest) = scratch.split_at_mut(nb);
    let (flag, count) = rest.split_at_mut(nb);
    let Some(l_colind) = l_colind else {
        for q in (0..nb).rev() {
            parent[q] = -1;
            flag[q] = q as i32;
            count[q] = 0;
            let mut ncol = 0;
            let end = (rowadr[q] + rownnz[q]) as usize;
            for j in upper_start(colind, rowadr[q] as usize, end, q)..end {
                let mut p = colind[j] as usize;
                while flag[p] != q as i32 {
                    flag[p] = q as i32;
                    count[p] += 1;
                    ncol += 1;
                    let par = parent[p];
                    if par == -1 {
                        parent[p] = q as i32;
                        break;
                    }
                    p = par as usize;
                }
            }
            bt_adr[q] = ncol;
        }
        let (mut nnz, mut nnz_t) = (0, 0);
        for p in 0..nb {
            let ncol = bt_adr[p];
            bt_adr[p] = nnz_t;
            nnz_t += ncol;
            for a in 0..bs {
                let r = bs * p + a;
                l_rownnz[r] = bs as i32 * count[p] + a as i32 + 1;
                l_rowadr[r] = nnz;
                nnz += l_rownnz[r];
            }
        }
        bt_adr[nb] = nnz_t;
        return nnz;
    };
    for q in (0..nb).rev() {
        let mut x = bt_adr[q] as usize;
        parent[q] = -1;
        flag[q] = q as i32;
        count[q] = (l_rownnz[bs * q] - 1) / bs as i32;
        let off = bs * count[q] as usize;
        for c in 0..bs {
            for b in 0..=c {
                l_colind[l_rowadr[bs * q + c] as usize + off + b] = (bs * q + b) as i32;
            }
        }
        let end = (rowadr[q] + rownnz[q]) as usize;
        for j in upper_start(colind, rowadr[q] as usize, end, q)..end {
            let mut p = colind[j] as usize;
            while flag[p] != q as i32 {
                flag[p] = q as i32;
                count[p] -= 1;
                let ip = count[p] as usize;
                for c in 0..bs {
                    let col = &mut l_colind[l_rowadr[bs * p + c] as usize + bs * ip..];
                    for b in 0..bs {
                        col[b] = (bs * q + b) as i32;
                    }
                }
                bt_row[x] = p as i32;
                bt_off[x] = (bs * ip) as i32;
                x += 1;
                let par = parent[p];
                if par == -1 {
                    parent[p] = q as i32;
                    break;
                }
                p = par as usize;
            }
        }
    }
    0
}

// The C targets and descendants are disjoint rows of L. Offsets retain that storage and
// the exact read/write order without constructing overlapping Rust slice references.
// Suffix update of the NS target rows from BATCH descendant rows (NS and BATCH are constants, as the C
// passes them as literals to its forced-inline helper). The target rows d0..d2 and the descendant rows
// p0..p3 are separate slices, as the C's restrict pointers are, each cut to the length it is read at.
#[inline(always)]
fn chol_suffix_update<const NS: usize, const BATCH: usize>(
    nlead: usize,
    d0: &mut [f64],
    d1: &mut [f64],
    d2: &mut [f64],
    p0: &[f64],
    p1: &[f64],
    p2: &[f64],
    p3: &[f64],
    s: &[[f64; 4]; 3],
) {
    let num_cols = nlead + NS - 1;
    let (p0, p1, p2, p3) = (
        &p0[..num_cols],
        if BATCH >= 2 { &p1[..num_cols] } else { &p1[..0] },
        if BATCH >= 3 { &p2[..num_cols] } else { &p2[..0] },
        if BATCH >= 4 { &p3[..num_cols] } else { &p3[..0] },
    );
    let d0 = &mut d0[..num_cols];
    // leading columns shared by all NS rows
    {
        let (q0, q1, q2, q3) = (
            &p0[..nlead],
            if BATCH >= 2 { &p1[..nlead] } else { p1 },
            if BATCH >= 3 { &p2[..nlead] } else { p2 },
            if BATCH >= 4 { &p3[..nlead] } else { p3 },
        );
        let e0 = &mut d0[..nlead];
        let e1 = if NS >= 2 { &mut d1[..nlead] } else { &mut d1[..0] };
        let e2 = if NS >= 3 { &mut d2[..nlead] } else { &mut d2[..0] };
        for i in 0..nlead {
            let v = [
                q0[i],
                if BATCH >= 2 { q1[i] } else { 0.0 },
                if BATCH >= 3 { q2[i] } else { 0.0 },
                if BATCH >= 4 { q3[i] } else { 0.0 },
            ];
            for b in 0..BATCH {
                e0[i] -= s[0][b] * v[b];
            }
            if NS >= 2 {
                for b in 0..BATCH {
                    e1[i] -= s[1][b] * v[b];
                }
            }
            if NS >= 3 {
                for b in 0..BATCH {
                    e2[i] -= s[2][b] * v[b];
                }
            }
        }
    }
    // triangular tail: column nlead+u is present in rows t <= NS-2-u
    for u in 0..NS - 1 {
        let idx = nlead + u;
        let v = [
            p0[idx],
            if BATCH >= 2 { p1[idx] } else { 0.0 },
            if BATCH >= 3 { p2[idx] } else { 0.0 },
            if BATCH >= 4 { p3[idx] } else { 0.0 },
        ];
        for b in 0..BATCH {
            d0[idx] -= s[0][b] * v[b];
        }
        if u == 0 && NS >= 3 {
            for b in 0..BATCH {
                d1[idx] -= s[1][b] * v[b];
            }
        }
    }
}

// The supernode's loops through the dense accumulator, each a small function of its own: inlined
// into the supernode, these loops reloaded spilled pointers on every element. Those that index dense
// by columns of L do so unchecked, as the C's loops do (a check per element was a fifth of the
// factorization): the columns are below n (checked once per pattern by chol_factor_symbolic) and dense
// is at least n long (checked by chol_factor_numeric).

// Scatter the lower part of a row of H (its columns up to `row`) into dense.
#[inline(never)]
fn chol_scatter_row(dense: &mut [f64], cols: &[i32], vals: &[f64], row: usize) {
    let dense = &mut dense[..=row];
    for (&c, &v) in cols.iter().zip(vals) {
        let c = c as usize;
        if c > row {
            break;
        }
        dense[c] = v;
    }
}

// Gather dense into a row of L at its columns (`add`: added to the row), clearing dense there.
/// # Safety
/// Every entry of `cols` is below `dense.len()`.
#[inline(never)]
unsafe fn chol_gather_row(row: &mut [f64], cols: &[i32], dense: &mut [f64], add: bool) {
    // SAFETY (each access): c < dense.len(), the caller's contract.
    if add {
        for (x, &c) in row.iter_mut().zip(cols) {
            let c = c as usize;
            unsafe {
                *x += *dense.get_unchecked(c);
                *dense.get_unchecked_mut(c) = 0.0;
            }
        }
    } else {
        for (x, &c) in row.iter_mut().zip(cols) {
            let c = c as usize;
            unsafe {
                *x = *dense.get_unchecked(c);
                *dense.get_unchecked_mut(c) = 0.0;
            }
        }
    }
}

// dense[cols[i]] -= scale * vals[i]: a non-suffix descendant's update of a later supernode row.
/// # Safety
/// Every entry of `cols` is below `dense.len()`.
#[inline(never)]
unsafe fn chol_scatter_sub(dense: &mut [f64], cols: &[i32], vals: &[f64], scale: f64) {
    for (&c, &v) in cols.iter().zip(vals) {
        // SAFETY: c < dense.len(), the caller's contract.
        unsafe { *dense.get_unchecked_mut(c as usize) -= scale * v };
    }
}

// Non-suffix scatter of BATCH descendant rows into dense (BATCH is a constant, as in the C).
// The rows a batch reads are sliced to num_cols once (the same operations, no check per element).
/// # Safety
/// Every entry of `colptr[..num_cols]` is below `dense.len()`.
#[inline(never)]
unsafe fn chol_scatter_update<const BATCH: usize>(
    num_cols: usize,
    dense: &mut [f64],
    colptr: &[i32],
    p0: &[f64],
    p1: &[f64],
    p2: &[f64],
    p3: &[f64],
    s: &[f64; 4],
) {
    let (colptr, p0) = (&colptr[..num_cols], &p0[..num_cols]);
    let p1 = if BATCH >= 2 { &p1[..num_cols] } else { &p1[..0] };
    let p2 = if BATCH >= 3 { &p2[..num_cols] } else { &p2[..0] };
    let p3 = if BATCH >= 4 { &p3[..num_cols] } else { &p3[..0] };
    for i in 0..num_cols {
        let col = colptr[i] as usize;
        // SAFETY (this read and the write below): col < dense.len(), the caller's contract.
        let mut acc = unsafe { *dense.get_unchecked(col) } - s[0] * p0[i];
        if BATCH >= 2 {
            acc -= s[1] * p1[i];
        }
        if BATCH >= 3 {
            acc -= s[2] * p2[i];
        }
        if BATCH >= 4 {
            acc -= s[3] * p3[i];
        }
        unsafe { *dense.get_unchecked_mut(col) = acc };
    }
}

// Factor supernode of size NS (1, 2 or 3) starting at row r; return rank decrement. NS is a constant, as
// the C passes it as a literal to its forced-inline function so that each size is its own copy.
// L stores its rows in order, so the supernode's rows r, r-1, r-2 lie before every descendant row
// c > r: L is split there, and the target rows (the C's Lptr0..Lptr2) and the descendant rows are
// separate slices, as the C's restrict pointers are. `base` is where the descendant rows start.
// Not inlined into chol_factor_numeric: each size is a function of its own (called once per supernode).
/// # Safety
/// Every entry of `l_colind` is below `dense.len()`.
#[inline(never)]
unsafe fn chol_factor_supernode<const NS: usize>(
    r: usize,
    mindiag: f64,
    l: &mut [f64],
    l_rownnz: &[i32],
    l_rowadr: &[i32],
    l_colind: &[i32],
    lt_rownnz: &[i32],
    lt_rowadr: &[i32],
    lt_colind: &[i32],
    lt_map: &[i32],
    h: &[f64],
    h_rownnz: &[i32],
    h_rowadr: &[i32],
    h_colind: &[i32],
    dense: &mut [f64],
) -> i32 {
    let l_adr = l_rowadr[r] as usize;
    let l_nnz = l_rownnz[r] as usize;
    let l_colptr = &l_colind[l_adr..l_adr + l_nnz];
    let lt_adr = lt_rowadr[r] as usize;
    let lt_nnz = lt_rownnz[r] as usize;
    let lt_cols = &lt_colind[lt_adr..lt_adr + lt_nnz];
    let lt_idx = &lt_map[lt_adr..lt_adr + lt_nnz];

    let base = l_adr + l_nnz;
    let (head, desc) = l.split_at_mut(base);
    let desc: &[f64] = desc;
    let (head, row0) = head.split_at_mut(l_adr);
    let (head, row1): (&mut [f64], &mut [f64]) = if NS >= 2 {
        let (rest, row1) = head.split_at_mut(l_rowadr[r - 1] as usize);
        (rest, &mut row1[..l_nnz - 1])
    } else {
        (head, &mut [])
    };
    let row2: &mut [f64] = if NS >= 3 {
        let adr = l_rowadr[r - 2] as usize;
        &mut head[adr..adr + l_nnz - 2]
    } else {
        &mut []
    };
    let mut rows = [row0, row1, row2];

    // initialize L rows for the supernode from H (copy if no fill-in, else scatter/gather)
    let mut used_dense = false;
    for s in 0..NS {
        let row = r - s;
        let nnz_s = l_nnz - s;
        let h_adr = h_rowadr[row] as usize;
        let h_nnz = h_rownnz[row] as usize;
        let (h_cols, h_vals) = (&h_colind[h_adr..h_adr + h_nnz], &h[h_adr..h_adr + h_nnz]);
        let row_s = &mut rows[s][..nnz_s];
        if h_nnz >= nnz_s && h_cols[nnz_s - 1] == row as i32 {
            copy(row_s, h_vals, nnz_s);
        } else if NS == 1 {
            zero(row_s, nnz_s);
            chol_scatter_row(dense, h_cols, h_vals, row);
            used_dense = true;
        } else {
            chol_scatter_row(dense, h_cols, h_vals, row);
            // SAFETY: columns of L, below dense.len() (this function's contract).
            unsafe { chol_gather_row(row_s, &l_colptr[..nnz_s], dense, false) };
        }
    }

    // accumulate updates from descendant rows c > r in LT[r] (skip k=0 diagonal entry)
    let max_batch = if NS == 3 { 3 } else { 4 };
    let mut k = 1;
    while k < lt_nnz {
        let c0 = lt_cols[k] as usize;
        let idx0 = lt_idx[k] as usize;
        let c0_adr = l_rowadr[c0] as usize;
        let num_cols = idx0 - c0_adr + 1;
        let colptr = &l_colind[c0_adr..c0_adr + num_cols];
        let mut p = [c0_adr, 0, 0, 0];
        let mut idx = [idx0, 0, 0, 0];
        let col0 = colptr[0];
        let is_suffix = l_colptr[l_nnz - num_cols] == col0;
        let mut batch = 1;
        if is_suffix || NS == 1 {
            let mut prev_c = c0;
            let mut prev_adr = c0_adr;
            while k + batch < lt_nnz && batch < max_batch {
                let next_c = lt_cols[k + batch] as usize;
                let next_idx = lt_idx[k + batch] as usize;
                let next_adr = l_rowadr[next_c] as usize;
                if next_idx - next_adr + 1 != num_cols {
                    break;
                }
                if is_suffix {
                    if l_colind[next_adr] != col0 {
                        break;
                    }
                } else {
                    let hi = if next_c > prev_c { next_c } else { prev_c };
                    let lo = if next_c > prev_c { prev_c } else { next_c };
                    let hi_adr = if next_c > prev_c { next_adr } else { prev_adr };
                    let lo_nnz = l_rownnz[lo] as usize;
                    if lo_nnz >= l_rownnz[hi] as usize || l_colind[hi_adr + lo_nnz - 1] != lo as i32
                    {
                        break;
                    }
                    prev_c = next_c;
                    prev_adr = next_adr;
                }
                p[batch] = next_adr;
                idx[batch] = next_idx;
                batch += 1;
            }
        }
        let src = |b: usize| if b < batch { &desc[p[b] - base..] } else { &desc[..0] };
        let (q0, q1, q2, q3) = (src(0), src(1), src(2), src(3));
        if is_suffix {
            // suffix fast path: update target supernode rows contiguously in L
            let off = l_nnz - num_cols;
            let nlead = num_cols - (NS - 1);
            let mut s = [[0.0; 4]; 3];
            for t in 0..NS {
                for b in 0..batch {
                    s[t][b] = desc[idx[b] - t - base];
                }
            }
            let [r0, r1, r2] = &mut rows;
            let d0 = &mut r0[off..];
            let d1 = if NS >= 2 { &mut r1[off..] } else { &mut r1[..0] };
            let d2 = if NS >= 3 { &mut r2[off..] } else { &mut r2[..0] };
            match batch {
                1 => chol_suffix_update::<NS, 1>(nlead, d0, d1, d2, q0, q1, q2, q3, &s),
                2 => chol_suffix_update::<NS, 2>(nlead, d0, d1, d2, q0, q1, q2, q3, &s),
                3 => chol_suffix_update::<NS, 3>(nlead, d0, d1, d2, q0, q1, q2, q3, &s),
                4 => chol_suffix_update::<NS, 4>(nlead, d0, d1, d2, q0, q1, q2, q3, &s),
                _ => unreachable!(),
            }
        } else {
            // non-suffix subset: scatter row r update into dense; rows r-1, r-2 are revisited below
            used_dense = true;
            let mut s = [0.0; 4];
            for b in 0..batch {
                s[b] = desc[idx[b] - base];
            }
            // SAFETY: colptr holds columns of L, below dense.len() (this function's contract).
            unsafe {
                match batch {
                    1 => chol_scatter_update::<1>(num_cols, dense, colptr, q0, q1, q2, q3, &s),
                    2 => chol_scatter_update::<2>(num_cols, dense, colptr, q0, q1, q2, q3, &s),
                    3 => chol_scatter_update::<3>(num_cols, dense, colptr, q0, q1, q2, q3, &s),
                    4 => chol_scatter_update::<4>(num_cols, dense, colptr, q0, q1, q2, q3, &s),
                    _ => unreachable!(),
                }
            }
        }
        k += batch;
    }

    // flush non-suffix updates from dense for s = 0 .. NS-1
    if used_dense {
        for s in 0..NS {
            if s > 0 {
                for j in 1..lt_nnz {
                    let c0 = lt_cols[j] as usize;
                    let l_cr0_idx = lt_idx[j] as usize;
                    let c0_adr = l_rowadr[c0] as usize;
                    let num_cols = l_cr0_idx - c0_adr + 1;
                    if l_colptr[l_nnz - num_cols] != l_colind[c0_adr] {
                        let scale = desc[l_cr0_idx - s - base];
                        let n = num_cols - s;
                        // SAFETY: columns of L, below dense.len() (this function's contract).
                        unsafe { chol_scatter_sub(dense, &l_colind[c0_adr..c0_adr + n], &desc[c0_adr - base..][..n], scale) };
                    }
                }
            }
            let nnz_s = l_nnz - s;
            // SAFETY: columns of L, below dense.len() (this function's contract).
            unsafe { chol_gather_row(&mut rows[s][..nnz_s], &l_colptr[..nnz_s], dense, true) };
        }
    }

    // factor supernode rows r, r-1, ... and fuse scaling with intra-supernode elimination
    let mut rank_dec = 0;
    let n_off = l_nnz - NS;
    for s in 0..NS {
        let nnz_s = l_nnz - s;
        let (cur, next) = rows.split_at_mut(s + 1);
        let row_s = &mut *cur[s];
        let mut diag = row_s[nnz_s - 1];
        let deficient = !(diag >= mindiag);
        if deficient {
            diag = mindiag;
            rank_dec += 1;
        }
        let l_rr = diag.sqrt();
        if deficient {
            zero(row_s, nnz_s - 1);
        } else {
            let l_rr_inv = 1.0 / l_rr;
            let mut s_st = [0.0; 3];
            for t in s + 1..NS {
                row_s[l_nnz - 1 - t] *= l_rr_inv;
                s_st[t] = row_s[l_nnz - 1 - t];
            }
            let lead = &mut row_s[..n_off];
            if s + 2 < NS {
                let (a, b) = next.split_at_mut(1);
                let (a, b) = (&mut a[0][..n_off], &mut b[0][..n_off]);
                for i in 0..n_off {
                    lead[i] *= l_rr_inv;
                    let l_si = lead[i];
                    a[i] -= s_st[s + 1] * l_si;
                    b[i] -= s_st[s + 2] * l_si;
                }
            } else if s + 1 < NS {
                let a = &mut next[0][..n_off];
                for i in 0..n_off {
                    lead[i] *= l_rr_inv;
                    let l_si = lead[i];
                    a[i] -= s_st[s + 1] * l_si;
                }
            } else {
                for x in lead.iter_mut() {
                    *x *= l_rr_inv;
                }
            }
            for t in s + 1..NS {
                for u in t..NS {
                    next[t - s - 1][l_nnz - 1 - u] -= s_st[t] * s_st[u];
                }
            }
        }
        row_s[nnz_s - 1] = l_rr;
    }
    rank_dec
}

/// Factor H into the pattern of L, the C's mju_cholFactorNumeric.
/// # Safety
/// Every entry of `l_colind` is below `n`: true of the pattern chol_factor_symbolic builds, which checks it
/// once per pattern. The dense accumulator (`scratch`, at least n long, checked here) is indexed by those
/// columns without checks.
pub unsafe fn chol_factor_numeric(
    l: &mut [f64],
    n: usize,
    mindiag: f64,
    l_rownnz: &[i32],
    l_rowadr: &[i32],
    l_colind: &[i32],
    lt_rownnz: &[i32],
    lt_rowadr: &[i32],
    lt_colind: &[i32],
    lt_map: &[i32],
    h: &[f64],
    h_rownnz: &[i32],
    h_rowadr: &[i32],
    h_colind: &[i32],
    scratch: &mut [f64],
) -> i32 {
    let mut rank = n as i32;
    assert!(scratch.len() >= n, "chol_factor_numeric: the dense accumulator is shorter than n");
    let dense = scratch;
    zero(dense, n);
    let mut r = n as i32 - 1;
    while r >= 0 {
        let row = r as usize;
        let l_adr = l_rowadr[row] as usize;
        let l_nnz = l_rownnz[row] as usize;
        let l_colptr = &l_colind[l_adr..];
        let lt_nnz = lt_rownnz[row];
        let h_adr = h_rowadr[row] as usize;
        let h_nnz = h_rownnz[row] as usize;
        let h_nofill = h_nnz >= l_nnz && h_colind[h_adr + l_nnz - 1] == r;
        if lt_nnz == 1 && h_nofill {
            let hptr = &h[h_adr..h_adr + l_nnz];
            let mut diag = hptr[l_nnz - 1];
            let deficient = !(diag >= mindiag);
            if deficient {
                diag = mindiag;
                rank -= 1;
            }
            let l_rr = diag.sqrt();
            let lptr = &mut l[l_adr..l_adr + l_nnz];
            if deficient {
                zero(lptr, l_nnz - 1);
            } else {
                let l_rr_inv = 1.0 / l_rr;
                for i in 0..l_nnz - 1 {
                    lptr[i] = hptr[i] * l_rr_inv;
                }
            }
            lptr[l_nnz - 1] = l_rr;
            r -= 1;
            continue;
        }
        let mut ns = 1;
        while ns < 3
            && r - ns as i32 >= 0
            && l_nnz > ns
            && l_colptr[l_nnz - 1 - ns] == r - ns as i32
            && l_rownnz[row - ns] == (l_nnz - ns) as i32
            && lt_rownnz[row - ns] == lt_nnz + ns as i32
        {
            ns += 1;
        }
        if ns == 3 {
            // SAFETY: l_colind is below n (this function's contract), and n <= dense.len() (checked above).
            rank -= unsafe {
                chol_factor_supernode::<3>(
                    row, mindiag, l, l_rownnz, l_rowadr, l_colind, lt_rownnz, lt_rowadr, lt_colind,
                    lt_map, h, h_rownnz, h_rowadr, h_colind, dense,
                )
            };
        } else if ns == 2 {
            // SAFETY: l_colind is below n (this function's contract), and n <= dense.len() (checked above).
            rank -= unsafe {
                chol_factor_supernode::<2>(
                    row, mindiag, l, l_rownnz, l_rowadr, l_colind, lt_rownnz, lt_rowadr, lt_colind,
                    lt_map, h, h_rownnz, h_rowadr, h_colind, dense,
                )
            };
        } else {
            // SAFETY: l_colind is below n (this function's contract), and n <= dense.len() (checked above).
            rank -= unsafe {
                chol_factor_supernode::<1>(
                    row, mindiag, l, l_rownnz, l_rowadr, l_colind, lt_rownnz, lt_rowadr, lt_colind,
                    lt_map, h, h_rownnz, h_rowadr, h_colind, dense,
                )
            };
        }
        r -= ns as i32;
    }
    rank
}

pub fn chol_solve_sparse(
    res: &mut [f64],
    mat: &[f64],
    vec: &[f64],
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    copy(res, vec, n);
    let mut i = n as i32 - 1;
    while i >= 0 {
        let row = i as usize;
        let adr = rowadr[row] as usize;
        let nnz = rownnz[row] as usize;
        if i > 0
            && nnz >= 2
            && rownnz[row - 1] == (nnz - 1) as i32
            && colind[adr + nnz - 2] == i - 1
        {
            let row0 = &mat[adr..];
            let row1 = &mat[rowadr[row - 1] as usize..];
            let ci = &colind[adr..];
            if i > 1
                && nnz >= 3
                && rownnz[row - 2] == (nnz - 2) as i32
                && colind[adr + nnz - 3] == i - 2
            {
                let nob = nnz - 3;
                let row2 = &mat[rowadr[row - 2] as usize..];
                res[row] /= row0[nob + 2];
                let x0 = res[row];
                res[row - 1] = (res[row - 1] - row0[nob + 1] * x0) / row1[nob + 1];
                let x1 = res[row - 1];
                res[row - 2] = (res[row - 2] - row0[nob] * x0 - row1[nob] * x1) / row2[nob];
                let x2 = res[row - 2];
                if x0 != 0.0 || x1 != 0.0 || x2 != 0.0 {
                    for (((&c, &a0), &a1), &a2) in ci[..nob].iter().zip(&row0[..nob]).zip(&row1[..nob]).zip(&row2[..nob]) {
                        let c = c as usize;
                        res[c] = res[c] - a0 * x0 - a1 * x1 - a2 * x2;
                    }
                }
                i -= 3;
                continue;
            }
            let nob = nnz - 2;
            res[row] /= row0[nob + 1];
            let x0 = res[row];
            res[row - 1] = (res[row - 1] - row0[nob] * x0) / row1[nob];
            let x1 = res[row - 1];
            if x0 != 0.0 || x1 != 0.0 {
                for ((&c, &a0), &a1) in ci[..nob].iter().zip(&row0[..nob]).zip(&row1[..nob]) {
                    let c = c as usize;
                    res[c] = res[c] - a0 * x0 - a1 * x1;
                }
            }
            i -= 2;
            continue;
        }
        if res[row] != 0.0 {
            res[row] /= mat[adr + nnz - 1];
            let tmp = res[row];
            let row_mat = &mat[adr..adr + nnz - 1]; // C row pointer vs row index.
            for (&c, &a) in colind[adr..adr + nnz - 1].iter().zip(row_mat) {
                res[c as usize] -= a * tmp;
            }
        }
        i -= 1;
    }
    let mut i = 0;
    while i < n {
        let adr = rowadr[i] as usize;
        let nnz = rownnz[i] as usize;
        if i + 1 < n
            && rownnz[i + 1] == (nnz + 1) as i32
            && colind[rowadr[i + 1] as usize + nnz - 1] == i as i32
        {
            let nob = nnz - 1;
            let row0 = &mat[adr..];
            let row1 = &mat[rowadr[i + 1] as usize..];
            let ci = &colind[adr..];
            if i + 2 < n
                && rownnz[i + 2] == (nnz + 2) as i32
                && colind[rowadr[i + 2] as usize + nnz - 1] == i as i32
            {
                let row2 = &mat[rowadr[i + 2] as usize..];
                let (mut s0, mut s1, mut s2) = (0.0, 0.0, 0.0);
                for (((&c, &a0), &a1), &a2) in ci[..nob].iter().zip(&row0[..nob]).zip(&row1[..nob]).zip(&row2[..nob]) {
                    let y = res[c as usize];
                    s0 += a0 * y;
                    s1 += a1 * y;
                    s2 += a2 * y;
                }
                res[i] = (res[i] - s0) / row0[nob];
                res[i + 1] = (res[i + 1] - s1 - row1[nob] * res[i]) / row1[nob + 1];
                res[i + 2] = (res[i + 2] - s2 - row2[nob] * res[i] - row2[nob + 1] * res[i + 1])
                    / row2[nob + 2];
                i += 3;
                continue;
            }
            let (mut s0, mut s1) = (0.0, 0.0);
            for ((&c, &a0), &a1) in ci[..nob].iter().zip(&row0[..nob]).zip(&row1[..nob]) {
                let y = res[c as usize];
                s0 += a0 * y;
                s1 += a1 * y;
            }
            res[i] = (res[i] - s0) / row0[nob];
            res[i + 1] = (res[i + 1] - s1 - row1[nob] * res[i]) / row1[nob + 1];
            i += 2;
            continue;
        }
        if nnz > 1 {
            res[i] -= dot_sparse(&mat[adr..], res, nnz - 1, &colind[adr..]);
        }
        res[i] /= mat[adr + nnz - 1];
        i += 1;
    }
}

pub fn chol_update_sparse(
    mat: &mut [f64],
    x: &[f64],
    n: usize,
    flg_plus: i32,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    x_nnz: usize,
    x_ind: &[i32],
    scratch: &mut [f64],
) -> i32 {
    if x_nnz == 0 {
        return n as i32;
    }
    let start = x_ind[x_nnz - 1];
    let mut min_row = x_ind[0];
    let dense = scratch;
    zero(
        &mut dense[min_row as usize..],
        (start - min_row + 1) as usize,
    );
    scatter(dense, x, Some(x_ind), x_nnz);
    let mut rank = n as i32;
    let mut row = start;
    while row >= min_row {
        let x_row = dense[row as usize];
        if x_row == 0.0 {
            row -= 1;
            continue;
        }
        let nnz = rownnz[row as usize] as usize - 1;
        let adr = rowadr[row as usize] as usize;
        let diag = mat[adr + nnz];
        let mut tmp = diag * diag
            + if flg_plus != 0 {
                x_row * x_row
            } else {
                -x_row * x_row
            };
        if !(tmp >= MINVAL) {
            tmp = MINVAL;
            rank -= 1;
        }
        let r = tmp.sqrt();
        mat[adr + nnz] = r;
        if nnz == 0 {
            row -= 1;
            continue;
        }
        let c = diag / r;
        let s = -x_row / r;
        let s_signed = if flg_plus != 0 { -s } else { s };
        let ci = &colind[adr..];
        let col0 = ci[0];
        if col0 < min_row {
            zero(&mut dense[col0 as usize..], (min_row - col0) as usize);
            min_row = col0;
        }
        // Each operand sliced to its length once: the same operations, without a check per element.
        let mrow = &mut mat[adr..adr + nnz];
        let ci = &ci[..nnz];
        if nnz as i32 == ci[nnz - 1] - col0 + 1 {
            let dptr = &mut dense[col0 as usize..col0 as usize + nnz];
            for (m, d) in mrow.iter_mut().zip(dptr.iter_mut()) {
                let dense_j = *d;
                let mat_val = *m;
                *m = c * mat_val + s_signed * dense_j;
                *d = s * mat_val + c * dense_j;
            }
        } else {
            for (m, &j) in mrow.iter_mut().zip(ci) {
                let j = j as usize;
                let dense_j = dense[j];
                let mat_val = *m;
                *m = c * mat_val + s_signed * dense_j;
                dense[j] = s * mat_val + c * dense_j;
            }
        }
        row -= 1;
    }
    rank
}

fn min_int(a: i32, b: i32) -> i32 {
    if a < b {
        a
    } else {
        b
    }
}
fn max_int(a: i32, b: i32) -> i32 {
    if a > b {
        a
    } else {
        b
    }
}

pub fn band_diag(i: usize, ntotal: usize, nband: usize, ndense: usize) -> usize {
    let nsparse = ntotal - ndense;
    if i < nsparse {
        i * nband + nband - 1
    } else {
        nsparse * nband + (i - nsparse) * ntotal + i
    }
}

pub fn chol_factor_band(
    mat: &mut [f64],
    ntotal: usize,
    nband: usize,
    ndense: usize,
    diagadd: f64,
    diagmul: f64,
) -> f64 {
    let nsparse = ntotal - ndense;
    let mut mindiag = -1.0;
    for j in 0..nsparse {
        let width_jj = min_int(j as i32, nband as i32 - 1) as usize;
        let height = min_int((nsparse - j - 1) as i32, nband as i32 - 1) as usize;
        let adr_jj = (j + 1) * nband - 1;
        let left_jj = if width_jj > 1 {
            dot(
                &mat[adr_jj - width_jj..],
                &mat[adr_jj - width_jj..],
                width_jj,
            )
        } else if width_jj == 1 {
            mat[adr_jj - 1] * mat[adr_jj - 1]
        } else {
            0.0
        };
        let mut ljj = diagadd + diagmul * mat[adr_jj] + mat[adr_jj] - left_jj;
        if !(ljj >= MINVAL) {
            return 0.0;
        }
        ljj = ljj.sqrt();
        let scale = 1.0 / ljj;
        if ljj < mindiag || mindiag < 0.0 {
            mindiag = ljj;
        }
        let mut adr_ij = adr_jj + nband - 1;
        for i in j + 1..=j + height {
            let width_ij = min_int(j as i32, nband as i32 - 1 - i as i32 + j as i32) as usize;
            let left_ij = if width_ij > 1 {
                dot(
                    &mat[adr_jj - width_ij..],
                    &mat[adr_ij - width_ij..],
                    width_ij,
                )
            } else if width_ij == 1 {
                mat[adr_jj - 1] * mat[adr_ij - 1]
            } else {
                0.0
            };
            mat[adr_ij] = scale * (mat[adr_ij] - left_ij);
            adr_ij += nband - 1;
        }
        adr_ij = nsparse * nband + j;
        for _i in nsparse..ntotal {
            let left_ij = if width_jj > 1 {
                dot(
                    &mat[adr_jj - width_jj..],
                    &mat[adr_ij - width_jj..],
                    width_jj,
                )
            } else if width_jj == 1 {
                mat[adr_jj - 1] * mat[adr_ij - 1]
            } else {
                0.0
            };
            mat[adr_ij] = scale * (mat[adr_ij] - left_ij);
            adr_ij += ntotal;
        }
        mat[adr_jj] = ljj;
    }
    for j in nsparse..ntotal {
        let adr_jj = nsparse * nband + (j - nsparse) * ntotal + j;
        let mut ljj = diagadd + diagmul * mat[adr_jj] + mat[adr_jj]
            - dot(&mat[adr_jj - j..], &mat[adr_jj - j..], j);
        if !(ljj >= MINVAL) {
            return 0.0;
        }
        ljj = ljj.sqrt();
        let scale = 1.0 / ljj;
        if ljj < mindiag || mindiag < 0.0 {
            mindiag = ljj;
        }
        let mut adr_ij = adr_jj + ntotal;
        for _i in j + 1..ntotal {
            mat[adr_ij] = scale * (mat[adr_ij] - dot(&mat[adr_jj - j..], &mat[adr_ij - j..], j));
            adr_ij += ntotal;
        }
        mat[adr_jj] = ljj;
    }
    mindiag
}

pub fn chol_solve_band(
    res: &mut [f64],
    mat: &[f64],
    vec: &[f64],
    ntotal: usize,
    nband: usize,
    ndense: usize,
) {
    let nsparse = ntotal - ndense;
    copy(res, vec, ntotal);
    let mut adr_ii = nband as i32 - 1;
    for i in 0..nsparse {
        let width = min_int(i as i32, nband as i32 - 1) as usize;
        if width > 1 {
            res[i] -= dot(&mat[adr_ii as usize - width..], &res[i - width..], width);
        } else if width == 1 {
            res[i] -= mat[adr_ii as usize - 1] * res[i - 1];
        }
        res[i] /= mat[adr_ii as usize];
        adr_ii += nband as i32;
    }
    let mut row_i = nsparse * nband;
    for i in nsparse..ntotal {
        if i != 0 {
            res[i] -= dot(&mat[row_i..], res, i);
        }
        res[i] /= mat[row_i + i];
        row_i += ntotal;
    }
    let mut row_i = (nsparse * nband) as i32 + (ndense as i32 - 1) * ntotal as i32;
    for i in (nsparse..ntotal).rev() {
        res[i] /= mat[row_i as usize + i];
        if i != 0 && res[i] != 0.0 {
            let scl = -res[i];
            add_to_scl(res, &mat[row_i as usize..], scl, i);
        }
        row_i -= ntotal as i32;
    }
    let mut adr_ii = (nsparse * nband) as i32 - 1;
    for i in (0..nsparse).rev() {
        let width = min_int(i as i32, nband as i32 - 1) as usize;
        res[i] /= mat[adr_ii as usize];
        if width > 1 && res[i] != 0.0 {
            let scl = -res[i];
            add_to_scl(
                &mut res[i - width..],
                &mat[adr_ii as usize - width..],
                scl,
                width,
            );
        } else if width == 1 {
            res[i - 1] -= mat[adr_ii as usize - 1] * res[i];
        }
        adr_ii -= nband as i32;
    }
}

pub fn band2dense(
    res: &mut [f64],
    mat: &[f64],
    ntotal: usize,
    nband: usize,
    ndense: usize,
    flg_sym: bool,
) {
    let nsparse = ntotal - ndense;
    zero(res, ntotal * ntotal);
    for i in 0..nsparse {
        let width = min_int(i as i32, nband as i32 - 1) as usize;
        copy(
            &mut res[i * ntotal + i - width..],
            &mat[(i + 1) * nband - (width + 1)..],
            width + 1,
        );
    }
    for i in nsparse..ntotal {
        copy(
            &mut res[i * ntotal..],
            &mat[nsparse * nband + (i - nsparse) * ntotal..],
            i + 1,
        );
    }
    if flg_sym {
        for i in 0..ntotal {
            for j in i + 1..ntotal {
                res[i * ntotal + j] = res[j * ntotal + i];
            }
        }
    }
}

pub fn dense2band(res: &mut [f64], mat: &[f64], ntotal: usize, nband: usize, ndense: usize) {
    let nsparse = ntotal - ndense;
    for i in 0..nsparse {
        let width = min_int(i as i32, nband as i32 - 1) as usize;
        copy(
            &mut res[(i + 1) * nband - (width + 1)..],
            &mat[i * ntotal + i - width..],
            width + 1,
        );
    }
    for i in nsparse..ntotal {
        copy(
            &mut res[nsparse * nband + (i - nsparse) * ntotal..],
            &mat[i * ntotal..],
            i + 1,
        );
    }
}

pub fn band_mul_mat_vec(
    res: &mut [f64],
    mat: &[f64],
    vec: &[f64],
    ntotal: usize,
    nband: usize,
    ndense: usize,
    nvec: usize,
    flg_sym: bool,
) {
    let nsparse = ntotal - ndense;
    for j in 0..nvec {
        let vec_j = &vec[ntotal * j..];
        let res_j = &mut res[ntotal * j..];
        let mut row_end = nband;
        for i in 0..nsparse {
            let width = min_int((i + 1) as i32, nband as i32) as usize;
            let adr = row_end - width;
            let offset = i + 1 - width;
            res_j[i] = dot(&mat[adr..], &vec_j[offset..], width);
            if flg_sym && width > 1 {
                add_to_scl(&mut res_j[offset..], &mat[adr..], vec_j[i], width - 1);
            }
            row_end += nband;
        }
        let mut adr = nsparse * nband;
        for i in nsparse..ntotal {
            res_j[i] = dot(&mat[adr..], vec_j, i + 1);
            if flg_sym && i > 0 {
                add_to_scl(res_j, &mat[adr..], vec_j[i], i);
            }
            adr += ntotal;
        }
    }
}

pub fn factor_lu(a: &mut [f64], n: usize, pivot: &mut [i32]) -> i32 {
    for k in 0..n {
        pivot[k] = k as i32;
        let mut maxval = a[k * n + k].abs();
        let mut maxrow = k;
        for i in k + 1..n {
            let val = a[i * n + k].abs();
            if val > maxval {
                maxval = val;
                maxrow = i;
            }
        }
        if maxval < MINVAL {
            return 0;
        }
        if maxrow != k {
            pivot[k] = maxrow as i32;
            for j in 0..n {
                let tmp = a[k * n + j];
                a[k * n + j] = a[maxrow * n + j];
                a[maxrow * n + j] = tmp;
            }
        }
        let diaginv = 1.0 / a[k * n + k];
        for i in k + 1..n {
            a[i * n + k] *= diaginv;
            let aik = a[i * n + k];
            for j in k + 1..n {
                a[i * n + j] -= aik * a[k * n + j];
            }
        }
    }
    1
}

pub fn solve_lu(x: &mut [f64], lu: &[f64], b: &[f64], pivot: &[i32], n: usize) {
    copy(x, b, n);
    for i in 0..n {
        if pivot[i] != i as i32 {
            let tmp = x[i];
            x[i] = x[pivot[i] as usize];
            x[pivot[i] as usize] = tmp;
        }
        for j in 0..i {
            x[i] -= lu[i * n + j] * x[j];
        }
    }
    for i in (0..n).rev() {
        for j in i + 1..n {
            x[i] -= lu[i * n + j] * x[j];
        }
        x[i] /= lu[i * n + i];
    }
}

pub fn factor_lu6(a: &mut [f64; 36], pivot: &mut [i32; 6]) -> i32 {
    for k in 0..6 {
        pivot[k] = k as i32;
        let mut maxval = a[k * 6 + k].abs();
        let mut maxrow = k;
        for i in k + 1..6 {
            let val = a[i * 6 + k].abs();
            if val > maxval {
                maxval = val;
                maxrow = i;
            }
        }
        if maxval < MINVAL {
            return 0;
        }
        if maxrow != k {
            pivot[k] = maxrow as i32;
            for j in 0..6 {
                let tmp = a[k * 6 + j];
                a[k * 6 + j] = a[maxrow * 6 + j];
                a[maxrow * 6 + j] = tmp;
            }
        }
        let diaginv = 1.0 / a[k * 6 + k];
        for i in k + 1..6 {
            a[i * 6 + k] *= diaginv;
            let aik = a[i * 6 + k];
            for j in k + 1..6 {
                a[i * 6 + j] -= aik * a[k * 6 + j];
            }
        }
    }
    1
}

pub fn solve_lu6(x: &mut [f64; 6], lu: &[f64; 36], b: &[f64; 6], pivot: &[i32; 6]) {
    for i in 0..6 {
        x[i] = b[i];
    }
    for i in 0..6 {
        if pivot[i] != i as i32 {
            let tmp = x[i];
            x[i] = x[pivot[i] as usize];
            x[pivot[i] as usize] = tmp;
        }
        for j in 0..i {
            x[i] -= lu[i * 6 + j] * x[j];
        }
    }
    for i in (0..6).rev() {
        for j in i + 1..6 {
            x[i] -= lu[i * 6 + j] * x[j];
        }
        x[i] /= lu[i * 6 + i];
    }
}

pub fn factor_lusparse(
    lu: &mut [f64],
    n: usize,
    scratch: &mut [i32],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    index: Option<&[i32]>,
) -> i32 {
    let remaining = scratch;
    let mut clamped = -1;
    if let Some(idx) = index {
        for i in 0..n {
            remaining[i] = rownnz[idx[i] as usize];
        }
    } else {
        copy_int(remaining, rownnz, n);
    }
    for r in (0..n).rev() {
        let i = index.map_or(r, |idx| idx[r] as usize);
        let ii = (rowadr[i] + remaining[r] - 1) as usize;
        remaining[r] -= 1;
        if colind[ii] != i as i32 {
            panic!("missing diagonal element");
        }
        if lu[ii].abs() < MINVAL {
            lu[ii] = if lu[ii] < 0.0 { -MINVAL } else { MINVAL };
            if clamped < 0 {
                clamped = i as i32;
            }
        }
        for c in (0..r).rev() {
            let j = index.map_or(c, |idx| idx[c] as usize);
            let ji = (rowadr[j] + remaining[c] - 1) as usize;
            if colind[ji] == i as i32 {
                remaining[c] -= 1;
                lu[ji] = lu[ji] / lu[ii];
                let luji = lu[ji];
                let mut icnt = rowadr[i] as usize;
                let mut jcnt = rowadr[j] as usize;
                while jcnt < (rowadr[j] + remaining[c]) as usize {
                    if colind[icnt] == colind[jcnt] {
                        lu[jcnt] -= lu[icnt] * luji;
                        jcnt += 1;
                        icnt += 1;
                    } else if colind[icnt] > colind[jcnt] {
                        jcnt += 1;
                    } else {
                        panic!("requires fill-in");
                    }
                }
                if icnt != (rowadr[i] + remaining[r]) as usize
                    || jcnt != (rowadr[j] + remaining[c]) as usize
                {
                    panic!("row processing incomplete");
                }
            }
        }
    }
    for r in 0..n {
        let i = index.map_or(r, |idx| idx[r] as usize);
        if remaining[r] < 0 || colind[(rowadr[i] + remaining[r]) as usize] != i as i32 {
            panic!("unexpected sparse matrix structure");
        }
    }
    clamped
}

pub fn solve_lusparse(
    res: &mut [f64],
    lu: &[f64],
    vec: &[f64],
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    diag: &[i32],
    colind: &[i32],
    index: Option<&[i32]>,
) {
    for k in (0..n).rev() {
        let i = index.map_or(k, |idx| idx[k] as usize);
        res[i] = vec[i];
        let d1 = diag[i] + 1;
        let nnz = rownnz[i] - d1;
        if nnz > 0 {
            let adr = (rowadr[i] + d1) as usize;
            res[i] -= dot_sparse(&lu[adr..], res, nnz as usize, &colind[adr..]);
        }
    }
    for k in 0..n {
        let i = index.map_or(k, |idx| idx[k] as usize);
        let d = diag[i] as usize;
        let adr = rowadr[i] as usize;
        if d > 0 {
            res[i] -= dot_sparse(&lu[adr..], res, d, &colind[adr..]);
        }
        res[i] /= lu[adr + d];
    }
}

pub fn solve3(x: &mut [f64; 3], a: &[f64; 9], b: &[f64; 3]) {
    let mut m = [
        [a[0], a[1], a[2], b[0]],
        [a[3], a[4], a[5], b[1]],
        [a[6], a[7], a[8], b[2]],
    ];
    for i in 0..3 {
        let pivot = m[i][i];
        for j in i..4 {
            m[i][j] /= pivot;
        }
        for k in 0..3 {
            if k != i {
                let factor = m[k][i];
                for j in i..4 {
                    m[k][j] -= factor * m[i][j];
                }
            }
        }
    }
    x[0] = m[0][3];
    x[1] = m[1][3];
    x[2] = m[2][3];
}

pub fn qcqp2(res: &mut [f64], ain: &[f64], bin: &[f64], d: &[f64], r: f64) -> i32 {
    let b1 = bin[0] * d[0];
    let b2 = bin[1] * d[1];
    let a11 = ain[0] * d[0] * d[0];
    let a22 = ain[3] * d[1] * d[1];
    let a12 = ain[1] * d[0] * d[1];
    let mut la = 0.0;
    let (mut v1, mut v2) = (0.0, 0.0);
    for _iter in 0..20 {
        let det = (a11 + la) * (a22 + la) - a12 * a12;
        if det < 1e-10 {
            res[0] = 0.0;
            res[1] = 0.0;
            return 0;
        }
        let detinv = 1.0 / det;
        let p11 = (a22 + la) * detinv;
        let p22 = (a11 + la) * detinv;
        let p12 = -a12 * detinv;
        v1 = -p11 * b1 - p12 * b2;
        v2 = -p12 * b1 - p22 * b2;
        let val = v1 * v1 + v2 * v2 - r * r;
        if val < 1e-10 {
            break;
        }
        let deriv = -2.0 * (p11 * v1 * v1 + 2.0 * p12 * v1 * v2 + p22 * v2 * v2);
        let delta = -val / deriv;
        if delta < 1e-10 {
            break;
        }
        la += delta;
    }
    res[0] = v1 * d[0];
    res[1] = v2 * d[1];
    (la != 0.0) as i32
}

pub fn qcqp3(res: &mut [f64], ain: &[f64], bin: &[f64], d: &[f64], r: f64) -> i32 {
    let b1 = bin[0] * d[0];
    let b2 = bin[1] * d[1];
    let b3 = bin[2] * d[2];
    let a11 = ain[0] * d[0] * d[0];
    let a22 = ain[4] * d[1] * d[1];
    let a33 = ain[8] * d[2] * d[2];
    let a12 = ain[1] * d[0] * d[1];
    let a13 = ain[2] * d[0] * d[2];
    let a23 = ain[5] * d[1] * d[2];
    let mut la = 0.0;
    let (mut v1, mut v2, mut v3) = (0.0, 0.0, 0.0);
    for _iter in 0..20 {
        let mut p11 = (a22 + la) * (a33 + la) - a23 * a23;
        let mut p22 = (a11 + la) * (a33 + la) - a13 * a13;
        let mut p33 = (a11 + la) * (a22 + la) - a12 * a12;
        let mut p12 = a13 * a23 - a12 * (a33 + la);
        let mut p13 = a12 * a23 - a13 * (a22 + la);
        let mut p23 = a12 * a13 - a23 * (a11 + la);
        let det = (a11 + la) * p11 + a12 * p12 + a13 * p13;
        if det < 1e-10 {
            res[0] = 0.0;
            res[1] = 0.0;
            res[2] = 0.0;
            return 0;
        }
        let detinv = 1.0 / det;
        p11 *= detinv;
        p22 *= detinv;
        p33 *= detinv;
        p12 *= detinv;
        p13 *= detinv;
        p23 *= detinv;
        v1 = -p11 * b1 - p12 * b2 - p13 * b3;
        v2 = -p12 * b1 - p22 * b2 - p23 * b3;
        v3 = -p13 * b1 - p23 * b2 - p33 * b3;
        let val = v1 * v1 + v2 * v2 + v3 * v3 - r * r;
        if val < 1e-10 {
            break;
        }
        let deriv = -2.0 * (p11 * v1 * v1 + p22 * v2 * v2 + p33 * v3 * v3)
            - 4.0 * (p12 * v1 * v2 + p13 * v1 * v3 + p23 * v2 * v3);
        let delta = -val / deriv;
        if delta < 1e-10 {
            break;
        }
        la += delta;
    }
    res[0] = v1 * d[0];
    res[1] = v2 * d[1];
    res[2] = v3 * d[2];
    (la != 0.0) as i32
}

pub fn qcqp(res: &mut [f64], ain: &[f64], bin: &[f64], d: &[f64], r: f64, n: usize) -> i32 {
    let mut a = [0.0; 25];
    let mut ala = [0.0; 25];
    let mut b = [0.0; 5];
    let mut tmp = [0.0; 5];
    if n > 5 {
        panic!("n is only supported up to 5");
    }
    for i in 0..n {
        b[i] = bin[i] * d[i];
        for j in 0..n {
            a[j + i * n] = ain[j + i * n] * d[i] * d[j];
        }
    }
    let mut la = 0.0;
    for _iter in 0..20 {
        copy(&mut ala, &a, n * n);
        for i in 0..n {
            ala[i * (n + 1)] += la;
        }
        if chol_factor(&mut ala, n, 1e-10) < n as i32 {
            zero(res, n);
            return 0;
        }
        chol_solve(res, &ala, &b, n);
        // C scl aliases its input and output; the fixed-size copy needs no allocation.
        copy(&mut tmp, res, n);
        scl(res, &tmp, -1.0, n);
        let val = dot(res, res, n) - r * r;
        if val < 1e-10 {
            break;
        }
        chol_solve(&mut tmp, &ala, res, n);
        let deriv = -2.0 * dot(res, &tmp, n);
        let delta = -val / deriv;
        if delta < 1e-10 {
            break;
        }
        la += delta;
    }
    for i in 0..n {
        res[i] = res[i] * d[i];
    }
    (la != 0.0) as i32
}

/// Minimize 0.5*x'*H*x + x'*g with optional bounds and a warm start in res.
/// mju_boxQPmalloc is omitted: callers size res/g/lower/upper/index to n, H to n*n,
/// and R to n*(n+7). Integer work has 3*n entries (clamped, oldclamped, optional index),
/// replacing the C int-pointer casts into R's final three scratch vectors.
pub fn box_qp(
    res: &mut [f64],
    r: &mut [f64],
    index: Option<&mut [i32]>,
    h: &[f64],
    g: &[f64],
    n: usize,
    lower: Option<&[f64]>,
    upper: Option<&[f64]>,
    work: &mut [i32],
) -> i32 {
    let maxiter = 100;
    let mingrad = 1e-16;
    let backtrack = 0.5;
    let minstep = 1e-22;
    let armijo = 0.1;
    box_qpoption(
        res, r, index, h, g, n, lower, upper, maxiter, mingrad, backtrack, minstep, armijo, None,
        0, work,
    )
}

const BOXQP_NOT_SPD: i32 = -1;
const BOXQP_NO_DESCENT: i32 = 0;
const BOXQP_MAX_ITER: i32 = 1;
const BOXQP_MAX_LS_ITER: i32 = 2;
const BOXQP_TOL_GRAD: i32 = 3;
const BOXQP_UNBOUNDED: i32 = 4;
const BOXQP_ALL_CLAMPED: i32 = 5;
const NBOXQP: usize = 7;

fn mul_sym_vec(res: &mut [f64], mat: &[f64], vec: &[f64], n: usize) {
    for i in 0..n {
        res[i] = mat[n * i + i] * vec[i] + dot(&mat[n * i..], vec, i);
        if vec[i] != 0.0 {
            add_to_scl(res, &mat[n * i..], vec[i], i);
        }
    }
}

fn chol_delete(r: &mut [f64], temp: &mut [f64], m: usize, p: usize) -> i32 {
    for i in 1..p {
        for j in 0..=i {
            r[i * (m - 1) + j] = r[i * m + j];
        }
    }
    zero(temp, p);
    for i in p + 1..m {
        for j in 0..p {
            r[(i - 1) * (m - 1) + j] = r[i * m + j];
        }
        temp[i - 1] = r[i * m + p];
        for j in p + 1..=i {
            r[(i - 1) * (m - 1) + (j - 1)] = r[i * m + j];
        }
    }
    chol_update(r, temp, m - 1, 1)
}

fn chol_insert(
    r: &mut [f64],
    temp: &mut [f64],
    rowp: &mut [f64],
    h: &[f64],
    n: usize,
    index: &[i32],
    m: usize,
    p: usize,
    idx: usize,
) -> i32 {
    for j in 0..p {
        let mut s = h[idx * n + index[j] as usize];
        if j != 0 {
            s -= dot(&r[j * m..], rowp, j);
        }
        rowp[j] = s / r[j * (m + 1)];
    }
    let diag = h[idx * (n + 1)] - dot(rowp, rowp, p);
    if !(diag >= MINVAL) {
        return -1;
    }
    let l22 = diag.sqrt();
    rowp[p] = l22;
    zero(temp, p + 1);
    let inv_l22 = 1.0 / l22;
    for i in p..m {
        temp[i + 1] = (h[index[i] as usize * n + idx] - dot(&r[i * m..], rowp, p)) * inv_l22;
    }
    for i in (p..m).rev() {
        for j in (p..=i).rev() {
            r[(i + 1) * (m + 1) + (j + 1)] = r[i * m + j];
        }
        r[(i + 1) * (m + 1) + p] = temp[i + 1];
        for j in (0..p).rev() {
            r[(i + 1) * (m + 1) + j] = r[i * m + j];
        }
    }
    for j in 0..=p {
        r[p * (m + 1) + j] = rowp[j];
    }
    for i in (1..p).rev() {
        for j in (0..=i).rev() {
            r[i * (m + 1) + j] = r[i * m + j];
        }
    }
    chol_update(r, temp, m + 1, 0)
}

// Use the platform C formatter to retain snprintf's %g spelling, width, truncation,
// NUL termination and return count. No numerical work is delegated to C.
unsafe extern "C" {
    fn snprintf(
        s: *mut std::ffi::c_char,
        n: usize,
        format: *const std::ffi::c_char,
        ...
    ) -> std::ffi::c_int;
}

/// Explicit box-QP options; work has 3*n integers, as documented on box_qp.
pub fn box_qpoption(
    res: &mut [f64],
    r: &mut [f64],
    index: Option<&mut [i32]>,
    h: &[f64],
    g: &[f64],
    n: usize,
    lower: Option<&[f64]>,
    upper: Option<&[f64]>,
    maxiter: i32,
    mingrad: f64,
    backtrack: f64,
    minstep: f64,
    armijo: f64,
    mut log: Option<&mut [u8]>,
    logsz: usize,
    work: &mut [i32],
) -> i32 {
    let mut status = BOXQP_NO_DESCENT;
    let mut nfree = n;
    let mut nfactor = 0i32;
    let mut value = 0.0;
    let mut norm2: f64 = 0.0;
    if n == 0 {
        panic!("problem size n must be positive");
    }
    if let (Some(upper), Some(lower)) = (upper, lower) {
        for i in 0..n {
            if lower[i] >= upper[i] {
                panic!("upper bounds must be strictly larger than lower bounds");
            }
        }
    }
    let (r, scratch) = r.split_at_mut(n * n);
    let (grad, scratch) = scratch.split_at_mut(n);
    let (search, scratch) = scratch.split_at_mut(n);
    let (candidate, scratch) = scratch.split_at_mut(n);
    let (temp, _) = scratch.split_at_mut(n);
    let (clamped, work) = work.split_at_mut(n);
    let (oldclamped, work) = work.split_at_mut(n);
    let index = index.unwrap_or(&mut work[..n]);
    const STATUS_STRING: [&std::ffi::CStr; NBOXQP] = [
        c"Hessian is not positive definite",
        c"No descent direction found",
        c"Maximum main iterations exceeded",
        c"Maximum line-search iterations exceeded",
        c"Gradient norm smaller than tolerance",
        c"No dimensions clamped, returning Newton point",
        c"All dimensions clamped",
    ];
    if lower.is_none() && upper.is_none() {
        for i in 0..n {
            for j in 0..=i {
                r[i * n + j] = h[i * n + j];
            }
        }
        let rank = chol_factor(r, n, MINVAL);
        if rank == n as i32 {
            chol_solve(res, r, g, n);
            copy(temp, res, n);
            scl(res, temp, -1.0, n);
            value = 0.5 * dot(res, g, n);
            nfactor = 1;
            status = BOXQP_UNBOUNDED;
        } else {
            status = BOXQP_NOT_SPD;
        }
        for i in 0..n {
            index[i] = i as i32;
        }
    } else {
        for i in 0..n {
            if let Some(lo) = lower {
                res[i] = max(res[i], lo[i]);
            }
            if let Some(up) = upper {
                res[i] = min(res[i], up[i]);
            }
        }
    }
    let mut iter = 0;
    let mut logptr = 0;
    while iter < maxiter {
        if status != BOXQP_NO_DESCENT {
            break;
        }
        mul_sym_vec(grad, h, res, n);
        if iter == 0 {
            value = 0.5 * dot(res, grad, n) + dot(res, g, n);
        }
        add_to(grad, g, n);
        let oldvalue = value;
        let oldnfree = nfree;
        nfree = 0;
        for i in 0..n {
            clamped[i] = (lower.is_some_and(|lo| res[i] == lo[i] && grad[i] > 0.0)
                || upper.is_some_and(|up| res[i] == up[i] && grad[i] < 0.0))
                as i32;
            if clamped[i] == 0 {
                nfree += 1;
            }
        }
        if nfree == 0 {
            status = BOXQP_ALL_CLAMPED;
            break;
        }
        let mut rank = nfree as i32;
        let mut factorize = iter == 0;
        if iter != 0 {
            let (mut nclamp, mut nunclamp) = (0, 0);
            for i in 0..n {
                if clamped[i] != 0 && oldclamped[i] == 0 {
                    nclamp += 1;
                } else if clamped[i] == 0 && oldclamped[i] != 0 {
                    nunclamp += 1;
                }
            }
            if nclamp + nunclamp > 0 {
                if nclamp < oldnfree
                    && nclamp + 2 * nunclamp <= max_int(2, (nfree / 8) as i32) as usize
                {
                    let mut m = oldnfree;
                    let mut rank_ok = true;
                    for p in (0..m).rev() {
                        if clamped[index[p] as usize] != 0 {
                            if chol_delete(r, temp, m, p) != (m - 1) as i32 {
                                rank_ok = false;
                                break;
                            }
                            for j in p..m - 1 {
                                index[j] = index[j + 1];
                            }
                            m -= 1;
                        }
                    }
                    if nunclamp != 0 && rank_ok {
                        let mut p = 0;
                        for i in 0..n {
                            if clamped[i] == 0 {
                                if oldclamped[i] != 0 {
                                    if chol_insert(r, temp, search, h, n, index, m, p, i)
                                        != (m + 1) as i32
                                    {
                                        rank_ok = false;
                                        break;
                                    }
                                    for j in (p + 1..=m).rev() {
                                        index[j] = index[j - 1];
                                    }
                                    index[p] = i as i32;
                                    m += 1;
                                }
                                p += 1;
                            }
                        }
                    }
                    if !rank_ok {
                        factorize = true;
                    }
                } else {
                    factorize = true;
                }
            }
        }
        if factorize {
            let mut j = 0;
            for i in 0..n {
                if clamped[i] == 0 {
                    index[j] = i as i32;
                    j += 1;
                }
            }
            for i in 0..nfree {
                for k in 0..i + 1 {
                    r[i * nfree + k] = h[index[i] as usize * n + index[k] as usize];
                }
            }
            rank = chol_factor(r, nfree, MINVAL);
            nfactor += 1;
        }
        for i in 0..n {
            oldclamped[i] = clamped[i];
        }
        if rank != nfree as i32 {
            status = BOXQP_NOT_SPD;
            break;
        }
        norm2 = 0.0;
        for i in 0..nfree {
            let grad_i = grad[index[i] as usize];
            temp[i] = grad_i;
            norm2 += grad_i * grad_i;
        }
        if norm2 < mingrad {
            status = if nfree == n {
                BOXQP_UNBOUNDED
            } else {
                BOXQP_TOL_GRAD
            };
            break;
        }
        // Preserve the aliased C input using the candidate scratch, not yet live this iteration.
        copy(candidate, temp, nfree);
        chol_solve(temp, r, candidate, nfree);
        let mut sdotg = 0.0;
        for i in 0..nfree {
            search[i] = -temp[i];
            sdotg -= temp[i] * grad[index[i] as usize];
        }
        if sdotg >= 0.0 {
            break;
        }
        let mut step = 1.0;
        let mut improvement;
        let mut reduction;
        let mut nstep = 0i32;
        let mut clipped;
        loop {
            clipped = false;
            for i in 0..nfree {
                let idx = index[i] as usize;
                let mut c = res[idx] + step * search[i];
                if lower.is_some_and(|lo| c < lo[idx]) {
                    c = lower.unwrap()[idx];
                    clipped = true;
                } else if upper.is_some_and(|up| c > up[idx]) {
                    c = upper.unwrap()[idx];
                    clipped = true;
                }
                candidate[i] = c;
            }
            if !clipped {
                reduction = -step * (1.0 - 0.5 * step) * sdotg;
            } else {
                zero(temp, nfree);
                let mut ddotg = 0.0;
                for i in 0..nfree {
                    let idx = index[i] as usize;
                    let di = candidate[i] - res[idx];
                    if di != 0.0 {
                        ddotg += di * grad[idx];
                        add_to_scl(temp, &r[i * nfree..], di, i + 1);
                    }
                }
                reduction = -ddotg - 0.5 * dot(temp, temp, nfree);
            }
            value = oldvalue - reduction;
            nstep += 1;
            improvement = -reduction / (step * sdotg);
            if improvement < armijo {
                step = step * backtrack;
                if step < minstep {
                    status = BOXQP_MAX_LS_ITER;
                    break;
                }
            }
            if !(improvement < armijo) {
                break;
            }
        }
        if let Some(buf) = log.as_deref_mut().filter(|_| logptr < logsz) {
            assert!(logsz <= buf.len());
            // SAFETY: the destination is bounded by logsz; C string and all variadic types
            // match the format (f64 for double, i32 for int).
            let written = unsafe {
                snprintf(buf[logptr..].as_mut_ptr().cast(),logsz-logptr,
                c"iter %-3d:  |grad|: %-8.2g  reduction: %-8.2g  improvement: %-8.4g  linesearch: %g^%-2d  factorized: %d  nfree: %d\n".as_ptr(),
                iter+1,norm2.sqrt(),reduction,improvement,backtrack,nstep-1,factorize as i32,nfree as i32)
            };
            if written > 0 {
                logptr = min_int(logptr as i32 + written, logsz as i32) as usize;
            }
        }
        if status == BOXQP_MAX_LS_ITER {
            value = oldvalue;
            break;
        }
        for i in 0..nfree {
            res[index[i] as usize] = candidate[i];
        }
        if !clipped && value >= oldvalue {
            status = if nfree == n {
                BOXQP_UNBOUNDED
            } else {
                BOXQP_TOL_GRAD
            };
            break;
        }
        iter += 1;
    }
    if iter == maxiter {
        status = BOXQP_MAX_ITER;
    }
    if let Some(buf) = log.filter(|_| logptr < logsz) {
        assert!(logsz <= buf.len());
        // SAFETY: bounded writable slice, static C strings, and format-matched arguments.
        unsafe {
            snprintf(buf[logptr..].as_mut_ptr().cast(),logsz-logptr,
            c"BOXQP: %s.\niterations= %d,  factorizations= %d,  |grad|= %-12.6g, final value= %-12.6g\n".as_ptr(),
            STATUS_STRING[(status+1) as usize].as_ptr(),iter,nfactor,norm2.sqrt(),value);
        }
    }
    if status == BOXQP_NO_DESCENT || status == BOXQP_NOT_SPD {
        -1
    } else {
        nfree as i32
    }
}
