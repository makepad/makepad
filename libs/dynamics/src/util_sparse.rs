//! Sparse matrix operations and symbolic sparsity utilities.
//!
//! Translated from `src/engine/engine_util_sparse.h` and `src/engine/engine_util_sparse.c`
//! of the engine named in CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

// The scalar #else paths are translated, except where the reference's AVX build
// (src/engine/engine_util_sparse_avx.h) sums in another order: dot_sparse_x3 sums in its four lanes,
// as dot_sparse (whose scalar path shares them) does. mul_mat_vec_sparse follows the AVX build's
// structure: the rows of a supernode in threes, sharing the gathered vector.
use crate::{
    data::Data,
    sparse_pattern::{Pattern, PatternBuf, PatternRef},
    util_blas::*,
    util_misc::{copy_int, gather, zero_int},
};

pub fn popcount64(x: u64) -> i32 {
    x.count_ones() as i32
}
pub fn ctz64(x: u64) -> i32 {
    x.trailing_zeros() as i32
}

// Inlined into every caller, as the C's static inline mju_dotSparse is.
#[inline(always)]
pub fn dot_sparse(vec1: &[f64], vec2: &[f64], nnz1: usize, ind1: &[i32]) -> f64 {
    let mut vec1 = &vec1[..nnz1];
    let mut ind1 = &ind1[..nnz1];
    let (mut res0, mut res1, mut res2, mut res3) = (0.0, 0.0, 0.0, 0.0);
    while vec1.len() >= 4 && ind1.len() >= 4 {
        res0 += vec1[0] * vec2[ind1[0] as usize];
        res1 += vec1[1] * vec2[ind1[1] as usize];
        res2 += vec1[2] * vec2[ind1[2] as usize];
        res3 += vec1[3] * vec2[ind1[3] as usize];
        vec1 = &vec1[4..];
        ind1 = &ind1[4..];
    }
    let mut res = (res0 + res2) + (res1 + res3);
    for i in 0..vec1.len() {
        res += vec1[i] * vec2[ind1[i] as usize];
    }
    res
}

pub fn compare(vec1: &[i32], vec2: &[i32], n: usize) -> i32 {
    (vec1[..n] == vec2[..n]) as i32
}

pub fn merge_sorted(
    merge: &mut [i32],
    chain1: &[i32],
    n1: usize,
    chain2: &[i32],
    n2: usize,
) -> usize {
    if n1 == 0 {
        if n2 == 0 {
            return 0;
        }
        copy_int(merge, chain2, n2);
        return n2;
    } else if n2 == 0 {
        copy_int(merge, chain1, n1);
        return n1;
    }
    if n1 == n2 && compare(chain1, chain2, n1) != 0 {
        copy_int(merge, chain1, n1);
        return n1;
    }
    let (mut i, mut j, mut k) = (0, 0, 0);
    while i < n1 && j < n2 {
        let c1 = chain1[i];
        let c2 = chain2[j];
        if c1 < c2 {
            merge[k] = c1;
            i += 1;
        } else if c1 > c2 {
            merge[k] = c2;
            j += 1;
        } else {
            merge[k] = c1;
            i += 1;
            j += 1;
        }
        k += 1;
    }
    if i < n1 {
        copy_int(&mut merge[k..], &chain1[i..], n1 - i);
        k += n1 - i;
    } else if j < n2 {
        copy_int(&mut merge[k..], &chain2[j..], n2 - j);
        k += n2 - j;
    }
    k
}

#[inline]
pub fn add_to_scl_scl(res: &mut [f64], vec: &[f64], scl1: f64, scl2: f64, n: usize) {
    let res = &mut res[..n];
    let vec = &vec[..n];
    for i in 0..n {
        res[i] = res[i] * scl1 + vec[i] * scl2;
    }
}

pub fn combine_sparse(
    dst: &mut [f64],
    src: &[f64],
    a: f64,
    b: f64,
    dst_nnz: usize,
    src_nnz: usize,
    dst_ind: &mut [i32],
    src_ind: &[i32],
) -> usize {
    if dst_nnz == src_nnz && compare(dst_ind, src_ind, dst_nnz) != 0 {
        add_to_scl_scl(dst, src, a, b, dst_nnz);
        return dst_nnz;
    }
    let nnz = combine_sparse_count(dst_nnz, src_nnz, dst_ind, src_ind);
    let (mut bi, mut si, mut w) = (dst_nnz as i32 - 1, src_nnz as i32 - 1, nnz as i32 - 1);
    while bi >= 0 && si >= 0 {
        let badr = dst_ind[bi as usize];
        let sadr = src_ind[si as usize];
        if badr == sadr {
            dst[w as usize] = a * dst[bi as usize] + b * src[si as usize];
            dst_ind[w as usize] = badr;
            bi -= 1;
            si -= 1;
        } else if badr > sadr {
            dst[w as usize] = a * dst[bi as usize];
            dst_ind[w as usize] = badr;
            bi -= 1;
        } else {
            dst[w as usize] = b * src[si as usize];
            dst_ind[w as usize] = sadr;
            si -= 1;
        }
        w -= 1;
    }
    while si >= 0 {
        dst[w as usize] = b * src[si as usize];
        dst_ind[w as usize] = src_ind[si as usize];
        si -= 1;
        w -= 1;
    }
    if a != 1.0 {
        while bi >= 0 {
            dst[bi as usize] *= a;
            bi -= 1;
        }
    }
    nnz
}

// Three rows of one column pattern times vec2, each summed as dot_sparse sums it (four lanes, then the
// tail), with vec2 gathered once for the three: the reference build's mju_dotSparseX3 (its AVX path; the
// scalar #else sums in one accumulator). Inlined into its callers as the C's static inline is; the rows
// are walked in chunks of four, so only the gather is checked.
#[inline(always)]
pub fn dot_sparse_x3(
    res0: &mut f64,
    res1: &mut f64,
    res2: &mut f64,
    vec10: &[f64],
    vec11: &[f64],
    vec12: &[f64],
    vec2: &[f64],
    nnz1: usize,
    ind1: &[i32],
) {
    let (ind4, ind_tail) = ind1[..nnz1].as_chunks::<4>();
    let (a4, a_tail) = vec10[..nnz1].as_chunks::<4>();
    let (b4, b_tail) = vec11[..nnz1].as_chunks::<4>();
    let (c4, c_tail) = vec12[..nnz1].as_chunks::<4>();
    let mut s0 = [0.0; 4];
    let mut s1 = [0.0; 4];
    let mut s2 = [0.0; 4];
    for (((ix, a), b), c) in ind4.iter().zip(a4).zip(b4).zip(c4) {
        let g = [vec2[ix[0] as usize], vec2[ix[1] as usize], vec2[ix[2] as usize], vec2[ix[3] as usize]];
        for l in 0..4 {
            s0[l] += a[l] * g[l];
            s1[l] += b[l] * g[l];
            s2[l] += c[l] * g[l];
        }
    }
    // C RES0/RES1/RES2 clash with the output names after case conversion.
    let mut res0_mat = (s0[0] + s0[2]) + (s0[1] + s0[3]);
    let mut res1_mat = (s1[0] + s1[2]) + (s1[1] + s1[3]);
    let mut res2_mat = (s2[0] + s2[2]) + (s2[1] + s2[3]);
    for (((&ix, &a), &b), &c) in ind_tail.iter().zip(a_tail).zip(b_tail).zip(c_tail) {
        let v2 = vec2[ix as usize];
        res0_mat += a * v2;
        res1_mat += b * v2;
        res2_mat += c * v2;
    }
    *res0 = res0_mat;
    *res1 = res1_mat;
    *res2 = res2_mat;
}

pub fn dot_sparse2(
    vec1: &[f64],
    ind1: &[i32],
    nnz1: usize,
    vec2: &[f64],
    ind2: &[i32],
    nnz2: usize,
) -> f64 {
    let (mut i1, mut i2, mut res) = (0, 0, 0.0);
    if nnz1 == 0 || nnz2 == 0 {
        return 0.0;
    }
    while i1 < nnz1 && i2 < nnz2 {
        let adr1 = ind1[i1];
        let adr2 = ind2[i2];
        if adr1 == adr2 {
            res += vec1[i1] * vec2[i2];
            i1 += 1;
            i2 += 1;
        } else if adr1 < adr2 {
            i1 += 1;
        } else {
            i2 += 1;
        }
    }
    res
}

pub fn dense2sparse(
    res: &mut [f64],
    mat: &[f64],
    nr: usize,
    nc: usize,
    rownnz: &mut [i32],
    rowadr: &mut [i32],
    colind: &mut [i32],
    nnz: i32,
) -> i32 {
    if nnz <= 0 {
        return 1;
    }
    let mut adr = 0;
    for r in 0..nr {
        rownnz[r] = 0;
        rowadr[r] = adr as i32;
        for c in 0..nc {
            if mat[r * nc + c] != 0.0 {
                if adr >= nnz as usize {
                    return 1;
                }
                colind[adr] = c as i32;
                rownnz[r] += 1;
                res[adr] = mat[r * nc + c];
                adr += 1;
            }
        }
    }
    0
}

pub fn sparse2dense(
    res: &mut [f64],
    mat: &[f64],
    nr: usize,
    nc: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    zero(res, nr * nc);
    for r in 0..nr {
        for i in 0..rownnz[r] as usize {
            let adr = rowadr[r] as usize + i;
            res[r * nc + colind[adr] as usize] = mat[adr];
        }
    }
}

pub fn sym2dense(
    res: &mut [f64],
    mat: &[f64],
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    zero(res, n * n);
    for i in 0..n {
        let adr = rowadr[i] as usize;
        for j in 0..rownnz[i] as usize {
            let col = colind[adr + j] as usize;
            if col <= i {
                res[i * n + col] = mat[adr + j];
                res[col * n + i] = mat[adr + j];
            }
        }
    }
}

pub fn copy_sparse(
    res: &mut [f64],
    mat: &[f64],
    rownnz: &[i32],
    rowadr: &[i32],
    row: &[i32],
    nrow: usize,
) {
    for i in 0..nrow {
        let r = row[i] as usize;
        copy(
            &mut res[rowadr[r] as usize..],
            &mat[rowadr[r] as usize..],
            rownnz[r] as usize,
        );
    }
}
pub fn zero_sparse(res: &mut [f64], rownnz: &[i32], rowadr: &[i32], row: &[i32], nrow: usize) {
    for i in 0..nrow {
        let r = row[i] as usize;
        zero(&mut res[rowadr[r] as usize..], rownnz[r] as usize);
    }
}

pub fn mul_mat_vec_sparse(
    res: &mut [f64],
    mat: &[f64],
    vec: &[f64],
    nr: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    rowsuper: Option<&[i32]>,
) {
    let Some(rowsuper) = rowsuper else {
        for r in 0..nr {
            let adr = rowadr[r] as usize;
            res[r] = dot_sparse(&mat[adr..], vec, rownnz[r] as usize, &colind[adr..]);
        }
        return;
    };
    // regular rows, and supernodes: rows rowsuper[r] + 1 long that share row r's columns
    let mut r = 0;
    while r < nr {
        let mut rs = rowsuper[r] as usize + 1;
        // the supernode's rows in blocks of three
        while rs >= 3 {
            let adr = rowadr[r] as usize;
            let (mut a, mut b, mut c) = (0.0, 0.0, 0.0);
            dot_sparse_x3(&mut a, &mut b, &mut c, &mat[adr..], &mat[rowadr[r + 1] as usize..], &mat[rowadr[r + 2] as usize..],
                vec, rownnz[r] as usize, &colind[adr..]);
            res[r] = a;
            res[r + 1] = b;
            res[r + 2] = c;
            r += 3;
            rs -= 3;
        }
        // the remaining rows
        while rs > 0 {
            let adr = rowadr[r] as usize;
            res[r] = dot_sparse(&mat[adr..], vec, rownnz[r] as usize, &colind[adr..]);
            r += 1;
            rs -= 1;
        }
    }
}

pub fn mul_mat_tvec_sparse(
    res: &mut [f64],
    mat: &[f64],
    vec: &[f64],
    nr: usize,
    nc: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    zero(res, nc);
    for i in 0..nr {
        let scl = vec[i];
        if scl == 0.0 {
            continue;
        }
        let nnz = rownnz[i] as usize;
        let adr = rowadr[i] as usize;
        let ind = &colind[adr..];
        let row = &mat[adr..];
        for j in 0..nnz {
            res[ind[j] as usize] += row[j] * scl;
        }
    }
}

pub fn add_to_mat_sparse(
    dst: &mut [f64],
    rownnz: &mut [i32],
    rowadr: &mut [i32],
    colind: &mut [i32],
    nr: usize,
    m: &[f64],
    m_rownnz: &[i32],
    m_rowadr: &[i32],
    m_colind: &[i32],
) {
    for i in 0..nr {
        let adr = rowadr[i] as usize;
        let m_adr = m_rowadr[i] as usize;
        rownnz[i] = combine_sparse(
            &mut dst[adr..],
            &m[m_adr..],
            1.0,
            1.0,
            rownnz[i] as usize,
            m_rownnz[i] as usize,
            &mut colind[adr..],
            &m_colind[m_adr..],
        ) as i32;
    }
}

pub fn add_to_sym_sparse(
    res: &mut [f64],
    mat: &[f64],
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    flg_upper: i32,
) {
    for i in 0..n {
        let start = rowadr[i] as usize;
        let end = start + rownnz[i] as usize;
        for adr in start..end {
            let val = mat[adr];
            let j = colind[adr] as usize;
            res[i * n + j] += val;
            if flg_upper != 0 && j < i {
                res[j * n + i] += val;
            }
        }
    }
}

pub fn mul_sym_vec_sparse(
    res: &mut [f64],
    mat: &[f64],
    vec: &[f64],
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    zero(res, n);
    for i in 0..n {
        let adr = rowadr[i] as usize;
        let diag = rownnz[i] as usize - 1;
        let row = &mat[adr..];
        res[i] = row[diag] * vec[i];
        let ind = &colind[adr..];
        for k in (0..diag).rev() {
            let j = ind[k] as usize;
            let val = row[k];
            res[i] += val * vec[j];
            res[j] += val * vec[i];
        }
    }
}

pub fn combine_sparse_count(a_nnz: usize, b_nnz: usize, a_ind: &[i32], b_ind: &[i32]) -> usize {
    let (mut a, mut b, mut c_nnz) = (0, 0, 0);
    while a < a_nnz && b < b_nnz {
        if a_ind[a] == b_ind[b] {
            c_nnz += 1;
            a += 1;
            b += 1;
        } else if a_ind[a] < b_ind[b] {
            a += 1;
        } else {
            b += 1;
        }
    }
    a_nnz + b_nnz - c_nnz
}

pub fn combine_sparse_inc(
    dst: &mut [f64],
    src: &[f64],
    n: usize,
    a: f64,
    b: f64,
    dst_nnz: usize,
    src_nnz: usize,
    dst_ind: &[i32],
    src_ind: &[i32],
) {
    if dst_nnz == src_nnz && compare(dst_ind, src_ind, dst_nnz) != 0 {
        add_to_scl_scl(dst, src, a, b, dst_nnz);
        return;
    }
    if a != 1.0 {
        let src_copy = dst[..dst_nnz].to_vec();
        scl(dst, &src_copy, a, dst_nnz);
    }
    let (mut di, mut si) = (0, 0);
    let mut dadr = if di < dst_nnz {
        dst_ind[di]
    } else {
        n as i32 + 1
    };
    let mut sadr = if si < src_nnz {
        src_ind[si]
    } else {
        n as i32 + 1
    };
    while di < dst_nnz {
        if dadr == sadr {
            dst[di] += b * src[si];
            di += 1;
            si += 1;
            dadr = if di < dst_nnz {
                dst_ind[di]
            } else {
                n as i32 + 1
            };
            sadr = if si < src_nnz {
                src_ind[si]
            } else {
                n as i32 + 1
            };
        } else if dadr < sadr {
            di += 1;
            dadr = if di < dst_nnz {
                dst_ind[di]
            } else {
                n as i32 + 1
            };
        } else {
            si += 1;
            sadr = if si < src_nnz {
                src_ind[si]
            } else {
                n as i32 + 1
            };
        }
    }
}

pub fn add_to_scl_sparse_inc(
    dst: &mut [f64],
    src: &[f64],
    nnzdst: usize,
    inddst: &[i32],
    nnzsrc: usize,
    indsrc: &[i32],
    scl: f64,
) {
    if nnzdst == 0 || nnzsrc == 0 {
        return;
    }
    // Cut inputs to their lengths once so the merge indexes without a check per element.
    let (dst, inddst) = (&mut dst[..nnzdst], &inddst[..nnzdst]);
    let (src, indsrc) = (&src[..nnzsrc], &indsrc[..nnzsrc]);
    let (mut adrs, mut adrd) = (0, 0);
    while adrs < nnzsrc && adrd < nnzdst {
        let inds = indsrc[adrs];
        let indd = inddst[adrd];
        if inds == indd {
            dst[adrd] += scl * src[adrs];
            adrs += 1;
            adrd += 1;
        } else if inds < indd {
            adrs += 1;
        } else {
            adrd += 1;
        }
    }
}

pub fn add_to_sparse_mat(
    dst: &mut [f64],
    src: &[f64],
    n: usize,
    nrow: usize,
    scl: f64,
    dst_nnz: usize,
    src_nnz: usize,
    dst_ind: &mut [i32],
    src_ind: &[i32],
    buf: &mut [f64],
    buf_ind: &mut [i32],
) -> usize {
    if dst_nnz == src_nnz {
        if dst_nnz == 0 {
            return 0;
        }
        if compare(dst_ind, src_ind, dst_nnz) != 0 {
            add_to_scl(dst, src, scl, nrow * dst_nnz);
            return dst_nnz;
        }
    }
    let (mut si, mut di, mut nnz) = (0, 0, 0);
    let mut sadr = if src_nnz != 0 {
        src_ind[0]
    } else {
        n as i32 + 1
    };
    let mut dadr = if dst_nnz != 0 {
        dst_ind[0]
    } else {
        n as i32 + 1
    };
    while si < src_nnz || di < dst_nnz {
        if sadr == dadr {
            for k in 0..nrow {
                buf[nrow * nnz + k] = dst[di + k * dst_nnz] + scl * src[si + k * src_nnz];
            }
            buf_ind[nnz] = sadr;
            nnz += 1;
            si += 1;
            di += 1;
            sadr = if si < src_nnz {
                src_ind[si]
            } else {
                n as i32 + 1
            };
            dadr = if di < dst_nnz {
                dst_ind[di]
            } else {
                n as i32 + 1
            };
        } else if dadr < sadr {
            for k in 0..nrow {
                buf[nrow * nnz + k] = dst[di + k * dst_nnz];
            }
            buf_ind[nnz] = dadr;
            nnz += 1;
            di += 1;
            dadr = if di < dst_nnz {
                dst_ind[di]
            } else {
                n as i32 + 1
            };
        } else {
            for k in 0..nrow {
                buf[nrow * nnz + k] = scl * src[si + k * src_nnz];
            }
            buf_ind[nnz] = sadr;
            nnz += 1;
            si += 1;
            sadr = if si < src_nnz {
                src_ind[si]
            } else {
                n as i32 + 1
            };
        }
    }
    transpose(dst, buf, nnz, nrow);
    copy_int(dst_ind, buf_ind, nnz);
    nnz
}

pub fn add_chains(
    res: &mut [i32],
    n: usize,
    nv1: usize,
    nv2: usize,
    chain1: &[i32],
    chain2: &[i32],
) -> usize {
    if nv1 == nv2 {
        if nv1 == 0 {
            return 0;
        }
        if compare(chain1, chain2, nv1) != 0 {
            copy_int(res, chain1, nv1);
            return nv1;
        }
    }
    let (mut i1, mut i2, mut nv) = (0, 0, 0);
    let mut adr1 = if nv1 != 0 { chain1[0] } else { n as i32 + 1 };
    let mut adr2 = if nv2 != 0 { chain2[0] } else { n as i32 + 1 };
    while i1 < nv1 || i2 < nv2 {
        if adr1 == adr2 {
            res[nv] = adr1;
            nv += 1;
            i1 += 1;
            i2 += 1;
            adr1 = if i1 < nv1 { chain1[i1] } else { n as i32 + 1 };
            adr2 = if i2 < nv2 { chain2[i2] } else { n as i32 + 1 };
        } else if adr1 < adr2 {
            res[nv] = adr1;
            nv += 1;
            i1 += 1;
            adr1 = if i1 < nv1 { chain1[i1] } else { n as i32 + 1 };
        } else {
            res[nv] = adr2;
            nv += 1;
            i2 += 1;
            adr2 = if i2 < nv2 { chain2[i2] } else { n as i32 + 1 };
        }
    }
    nv
}

pub fn compress_sparse(
    mat: &mut [f64],
    nr: usize,
    nc: usize,
    rownnz: &mut [i32],
    rowadr: &mut [i32],
    colind: &mut [i32],
    minval: f64,
) -> i32 {
    let _ = nc;
    let remove_small = minval >= 0.0;
    let mut adr = 0;
    for r in 0..nr {
        let rowadr_old = rowadr[r] as usize;
        rowadr[r] = adr as i32;
        let mut nnz = 0;
        let end = rowadr_old + rownnz[r] as usize;
        for adr_old in rowadr_old..end {
            if remove_small && mat[adr_old].abs() <= minval {
                continue;
            }
            if adr != adr_old {
                mat[adr] = mat[adr_old];
                colind[adr] = colind[adr_old];
            }
            adr += 1;
            if remove_small {
                nnz += 1;
            }
        }
        if remove_small {
            rownnz[r] = nnz;
        }
    }
    rowadr[nr - 1] + rownnz[nr - 1]
}

pub fn transpose_sparse(
    mut res: Option<&mut [f64]>,
    mat: &[f64],
    nr: usize,
    nc: usize,
    res_rownnz: &mut [i32],
    res_rowadr: &mut [i32],
    res_colind: &mut [i32],
    mut res_rowsuper: Option<&mut [i32]>,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    if nr == 0 || nc == 0 {
        return;
    }
    zero_int(res_rownnz, nc);
    let row_offset = rowadr[0];
    let res_rownnz = &mut res_rownnz[..nc];
    for r in 0..nr {
        let start = (rowadr[r] - row_offset) as usize;
        let end = start + rownnz[r] as usize;
        for &c in &colind[start..end] {
            res_rownnz[c as usize] += 1;
        }
    }
    if let Some(s) = res_rowsuper.as_deref_mut() {
        for i in 0..nc - 1 {
            s[i] = (res_rownnz[i] == res_rownnz[i + 1]) as i32;
        }
        s[nc - 1] = 0;
    }
    res_rowadr[0] = 0;
    for i in 1..nc {
        res_rowadr[i] = res_rowadr[i - 1] + res_rownnz[i - 1];
    }
    // each row's columns (and values) sliced once; the values and supernode marks as the row asks
    let res_rowadr_nc = &mut res_rowadr[..nc];
    for r in 0..nr {
        let mut c_prev = -1;
        let start = (rowadr[r] - row_offset) as usize;
        let end = start + rownnz[r] as usize;
        let cols = &colind[start..end];
        let mut mark = |c: i32| {
            if let Some(s) = res_rowsuper.as_deref_mut() {
                if c > 0 && c != c_prev + 1 && s[c as usize - 1] != 0 {
                    s[c as usize - 1] = 0;
                }
                c_prev = c;
            }
        };
        match res.as_deref_mut() {
            Some(v) => {
                for (&c, &x) in cols.iter().zip(&mat[start..end]) {
                    let adr = res_rowadr_nc[c as usize] as usize;
                    res_rowadr_nc[c as usize] += 1;
                    res_colind[adr] = r as i32;
                    v[adr] = x;
                    mark(c);
                }
            }
            None => {
                for &c in cols {
                    let adr = res_rowadr_nc[c as usize] as usize;
                    res_rowadr_nc[c as usize] += 1;
                    res_colind[adr] = r as i32;
                    mark(c);
                }
            }
        }
    }
    for i in (1..nc).rev() {
        res_rowadr[i] = res_rowadr[i - 1];
    }
    res_rowadr[0] = 0;
    if let Some(s) = res_rowsuper {
        for i in (0..nc - 1).rev() {
            if s[i] != 0 {
                s[i] += s[i + 1];
            }
        }
    }
}

/// transpose_sparse of a checked pattern (with its values into `res` when given): the transpose's p.nc() rows
/// over p.nr() columns are written into `out` (each array at least p.nc() long; the column list, and `res`,
/// as long as the pattern's count) and checked as they are made; supernodes when `with_super`. Its writes,
/// addressed by the pattern's columns and by the addresses counted from them, are unchecked.
pub fn transpose_pat(mut res: Option<&mut [f64]>, mat: &[f64], p: PatternRef, out: PatternBuf, with_super: bool) -> Pattern {
    let (nr, nc, rownnz, rowadr, colind) = (p.nr(), p.nc(), p.rownnz(), p.rowadr(), p.colind());
    let mut out = out;
    assert!(out.rownnz.len() >= nc && out.rowadr.len() >= nc && (!with_super || out.rowsuper.len() >= nc),
        "transpose_pat: an output row array is shorter than the column count");
    assert!(res.is_none() || mat.len() >= p.end(), "transpose_pat: the values are shorter than the pattern");
    let res_rownnz = &mut out.rownnz[..nc];
    let res_rowadr = &mut out.rowadr[..nc];
    let mut res_rowsuper = with_super.then(|| &mut out.rowsuper[..nc]);
    zero_int(res_rownnz, nc);
    zero_int(res_rowadr, nc);
    if let Some(s) = res_rowsuper.as_deref_mut() {
        zero_int(s, nc);
    }
    if nr != 0 && nc != 0 {
        // SAFETY: the pattern is checked, so every column c is below nc, the length of the row arrays; the
        // fill writes row c of the transpose at the addresses the count gave it, from res_rowadr[c] up to
        // res_rowadr[c] + res_rownnz[c] (the same walk of the same pattern), all below the total, which is
        // checked against the column list and the values before the fill.
        unsafe {
            for r in 0..nr {
                let adr = rowadr[r] as usize;
                for &c in &colind[adr..adr + rownnz[r] as usize] {
                    *res_rownnz.get_unchecked_mut(c as usize) += 1;
                }
            }
            if let Some(s) = res_rowsuper.as_deref_mut() {
                for i in 0..nc - 1 {
                    s[i] = (res_rownnz[i] == res_rownnz[i + 1]) as i32;
                }
                s[nc - 1] = 0;
            }
            res_rowadr[0] = 0;
            for i in 1..nc {
                res_rowadr[i] = res_rowadr[i - 1] + res_rownnz[i - 1];
            }
            let total = (res_rowadr[nc - 1] + res_rownnz[nc - 1]) as usize;
            assert!(out.colind.len() >= total && res.as_deref().is_none_or(|v| v.len() >= total),
                "transpose_pat: the output column list or values are shorter than the pattern");
            let res_colind = &mut out.colind[..total];
            for r in 0..nr {
                let mut c_prev = -1;
                let start = rowadr[r] as usize;
                let end = start + rownnz[r] as usize;
                let cols = &colind[start..end];
                let mut mark = |c: i32| {
                    if let Some(s) = res_rowsuper.as_deref_mut() {
                        if c > 0 && c != c_prev + 1 && s[c as usize - 1] != 0 {
                            s[c as usize - 1] = 0;
                        }
                        c_prev = c;
                    }
                };
                match res.as_deref_mut() {
                    Some(v) => {
                        for (&c, &x) in cols.iter().zip(&mat[start..end]) {
                            let slot = res_rowadr.get_unchecked_mut(c as usize);
                            let adr = *slot as usize;
                            *slot += 1;
                            *res_colind.get_unchecked_mut(adr) = r as i32;
                            *v.get_unchecked_mut(adr) = x;
                            mark(c);
                        }
                    }
                    None => {
                        for &c in cols {
                            let slot = res_rowadr.get_unchecked_mut(c as usize);
                            let adr = *slot as usize;
                            *slot += 1;
                            *res_colind.get_unchecked_mut(adr) = r as i32;
                            mark(c);
                        }
                    }
                }
            }
        }
        for i in (1..nc).rev() {
            res_rowadr[i] = res_rowadr[i - 1];
        }
        res_rowadr[0] = 0;
        if let Some(s) = res_rowsuper {
            for i in (0..nc - 1).rev() {
                if s[i] != 0 {
                    s[i] += s[i + 1];
                }
            }
        }
    }
    Pattern::new(out, nc, nr, with_super).unwrap_or_else(|(e, _)| panic!("transpose_pat: the transpose was refused: {e:?}"))
}

pub fn super_sparse(
    nr: usize,
    rowsuper: &mut [i32],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
) {
    if nr == 0 {
        return;
    }
    for r in 0..nr - 1 {
        rowsuper[r] = if rownnz[r] != rownnz[r + 1] {
            0
        } else {
            compare(
                &colind[rowadr[r] as usize..],
                &colind[rowadr[r + 1] as usize..],
                rownnz[r] as usize,
            )
        };
    }
    rowsuper[nr - 1] = 0;
    for r in (0..nr - 1).rev() {
        if rowsuper[r] != 0 {
            rowsuper[r] += rowsuper[r + 1];
        }
    }
}

pub fn sqr_mat_tdsparse_count(
    res_rownnz: &mut [i32],
    res_rowadr: &mut [i32],
    nr: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    rownnz_t: &[i32],
    rowadr_t: &[i32],
    colind_t: &[i32],
    rowsuper_t: Option<&[i32]>,
    d: &mut Data,
    flg_upper: i32,
) -> i32 {
    let mut chain = d.scratch.int(2 * nr);
    let mut nchain = 0;
    // The C res_colind points into chain; retain its offset across iterations.
    let mut res_colind = 0;
    for r in 0..nr {
        if rowsuper_t.is_some_and(|s| r > 0 && s[r - 1] > 0) {
            res_rownnz[r] = res_rownnz[r - 1];
            if flg_upper != 0 {
                for j in 0..nchain {
                    res_rownnz[chain[res_colind + j] as usize] += 1;
                }
            }
            if rownnz_t[r] != 0 {
                chain[res_colind + nchain] = r as i32;
                nchain += 1;
                res_rownnz[r] += 1;
            }
        } else {
            let (mut inew, mut iold) = (0, nr);
            nchain = 0;
            for i in 0..rownnz_t[r] as usize {
                let c = colind_t[rowadr_t[r] as usize + i] as usize;
                std::mem::swap(&mut inew, &mut iold);
                let mut nnewchain = 0;
                let mut adr = 0;
                let end = (rowadr[c] + rownnz[c]) as usize;
                for adr1 in rowadr[c] as usize..end {
                    let col_mat = colind[adr1];
                    while adr < nchain
                        && chain[iold + adr] < col_mat
                        && chain[iold + adr] <= r as i32
                    {
                        chain[inew + nnewchain] = chain[iold + adr];
                        nnewchain += 1;
                        adr += 1;
                    }
                    if col_mat > r as i32 {
                        break;
                    }
                    if adr < nchain && chain[iold + adr] == col_mat {
                        adr += 1;
                    }
                    chain[inew + nnewchain] = col_mat;
                    nnewchain += 1;
                }
                while adr < nchain && chain[iold + adr] <= r as i32 {
                    chain[inew + nnewchain] = chain[iold + adr];
                    nnewchain += 1;
                    adr += 1;
                }
                nchain = nnewchain;
            }
            res_rownnz[r] = nchain as i32;
            res_colind = inew;
            if flg_upper != 0 {
                let mut nchain_end = nchain;
                if nchain > 0 && chain[res_colind + nchain - 1] == r as i32 {
                    nchain_end = nchain - 1;
                }
                for j in 0..nchain_end {
                    res_rownnz[chain[res_colind + j] as usize] += 1;
                }
            }
        }
    }
    res_rowadr[0] = 0;
    for r in 1..nr {
        res_rowadr[r] = res_rowadr[r - 1] + res_rownnz[r - 1];
    }
    d.scratch.put_int(chain);
    res_rowadr[nr - 1] + res_rownnz[nr - 1]
}

pub fn sqr_mat_tduncompressed_init(res_rowadr: &mut [i32], nc: usize) {
    for r in 0..nc {
        res_rowadr[r] = (r * nc) as i32;
    }
}

const MAXSUPER: usize = 8;

pub fn sqr_mat_tdsparse_symbolic(
    res_rownnz: &mut [i32],
    res_rowadr: &mut [i32],
    mut res_colind: Option<&mut [i32]>,
    mut res_diagind: Option<&mut [i32]>,
    nr: usize,
    nc: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    rowsuper: Option<&[i32]>,
    rownnz_t: &[i32],
    rowadr_t: &[i32],
    colind_t: &[i32],
    rowsuper_t: Option<&[i32]>,
    scratch: &mut crate::memory::Scratch,
) -> i32 {
    if nc == 0 || nr == 0 {
        if nc > 0 {
            zero_int(res_rownnz, nc);
            if res_colind.is_none() {
                zero_int(res_rowadr, nc);
            }
            if let Some(v) = res_diagind {
                crate::util_misc::fill_int(v, -1, nc);
            }
        }
        return 0;
    }
    let colnnz = rownnz_t;
    let coladr = rowadr_t;
    let rowind = colind_t;
    let colsuper = rowsuper_t;
    let colnnz_t = rownnz;
    let coladr_t = rowadr;
    let rowind_t = colind;
    let mut marker = scratch.int(nc);
    zero_int(&mut marker, nc);
    let mut buffer_idx = scratch.int(nc);
    let mut rowstart = scratch.int(nr);
    zero_int(&mut rowstart, nr);
    zero_int(res_rownnz, nc);
    let mut c = 0;
    while c < nc {
        let mut buffer_nnz = 0;
        let nnz_c = colnnz[c] as usize;
        let adr_c = coladr[c] as usize;
        let ind_c = &rowind[adr_c..];
        let mut ns = 1;
        if let Some(cs) = colsuper.map(|s| s[c]).filter(|&s| s != 0) {
            ns += (cs as usize).min(MAXSUPER - 1);
        }
        if nnz_c == 0 {
            c += ns;
            continue;
        }
        let mut i = 0;
        while i < nnz_c {
            let r = ind_c[i] as usize;
            let adr_t = coladr_t[r] as usize;
            let nnz_t = colnnz_t[r] as usize;
            let ind_t = &rowind_t[adr_t..];
            rowstart[r] += ns as i32;
            for k in rowstart[r] as usize..nnz_t {
                let j = ind_t[k] as usize;
                if marker[j] == 0 {
                    marker[j] = 1;
                    buffer_idx[buffer_nnz] = j as i32;
                    buffer_nnz += 1;
                }
            }
            if let Some(s) = rowsuper {
                i += s[r] as usize;
            }
            i += 1;
        }
        if let Some(ci) = res_colind.as_deref_mut() {
            if ns == 1 {
                ci[(res_rowadr[c] + res_rownnz[c]) as usize] = c as i32;
                res_rownnz[c] += 1;
                for i in 0..buffer_nnz {
                    let j = buffer_idx[i] as usize;
                    marker[j] = 0;
                    ci[(res_rowadr[j] + res_rownnz[j]) as usize] = c as i32;
                    res_rownnz[j] += 1;
                }
            } else {
                for s in 0..ns {
                    let col = c + s;
                    let adr_col = (res_rowadr[col] + res_rownnz[col]) as usize;
                    for k in 0..=s {
                        ci[adr_col + k] = (c + k) as i32;
                    }
                    res_rownnz[col] += (s + 1) as i32;
                }
                for i in 0..buffer_nnz {
                    let j = buffer_idx[i] as usize;
                    marker[j] = 0;
                    let adr_j = (res_rowadr[j] + res_rownnz[j]) as usize;
                    for s in 0..ns {
                        ci[adr_j + s] = (c + s) as i32;
                    }
                    res_rownnz[j] += ns as i32;
                }
            }
        } else {
            for i in 0..buffer_nnz {
                let j = buffer_idx[i] as usize;
                marker[j] = 0;
                res_rownnz[j] += ns as i32;
            }
            if res_diagind.is_some() {
                for s in 0..ns {
                    res_rownnz[c + s] += (ns + buffer_nnz) as i32;
                }
            } else {
                for s in 0..ns {
                    res_rownnz[c + s] += (s + 1) as i32;
                }
            }
        }
        c += ns;
    }
    if res_colind.is_none() {
        res_rowadr[0] = 0;
        for r in 1..nc {
            res_rowadr[r] = res_rowadr[r - 1] + res_rownnz[r - 1];
        }
    }
    if let (Some(ci), Some(di)) = (res_colind.as_deref_mut(), res_diagind.as_deref_mut()) {
        let mut lower_nnz = scratch.int(nc);
        copy_int(&mut lower_nnz, res_rownnz, nc);
        for r in 0..nc {
            di[r] = if lower_nnz[r] != 0 {
                res_rowadr[r] + lower_nnz[r] - 1
            } else {
                -1
            };
        }
        for r in 0..nc {
            let adr = res_rowadr[r] as usize;
            let nnz = lower_nnz[r] as usize;
            for j in 0..nnz {
                let col = ci[adr + j] as usize;
                if col < r {
                    ci[(res_rowadr[col] + res_rownnz[col]) as usize] = r as i32;
                    res_rownnz[col] += 1;
                }
            }
        }
        scratch.put_int(lower_nnz);
    }
    scratch.put_int(marker);
    scratch.put_int(buffer_idx);
    scratch.put_int(rowstart);
    res_rowadr[nc - 1] + res_rownnz[nc - 1]
}

// W is a constant (4, or MAXSUPER), as the C passes a literal width to its inlined function.
// ind_k and val_k are M's row k, which holds column r.
#[inline(always)]
fn accum_super_row<const W: usize>(
    block: &mut [[f64; MAXSUPER]; MAXSUPER],
    buffer: &mut [f64],
    scale: &[f64; MAXSUPER],
    mat_t_s: &[&[f64]; MAXSUPER],
    ind_k: &[i32],
    val_k: &[f64],
    i: usize,
    r: usize,
    ns: usize,
    any_off: i32,
) {
    for t in 0..ns {
        let v = mat_t_s[t][i];
        for s in 0..W {
            block[t][s] += scale[s] * v;
        }
    }
    if any_off != 0 {
        let buffer = &mut buffer[..r * MAXSUPER];
        for (&c, &v) in ind_k.iter().zip(val_k) {
            let c = c as usize;
            if c >= r {
                break;
            }
            let buf = &mut buffer[c * MAXSUPER..c * MAXSUPER + W];
            for s in 0..W {
                buf[s] += scale[s] * v;
            }
        }
    }
}

/// res = mat_t * diag * mat' over checked patterns: each row r of `mat_t` (pattern `pt`) picks rows k of `mat`
/// (pattern `p`) by its columns, into row r of `res` (pattern `res_p`, nc x nc with nc = res_p.nr()); with
/// `res_diagind`, the lower triangle up to each row's diagonal entry, mirrored into the upper after.
pub fn sqr_mat_tdsparse_numeric(
    res: &mut [f64],
    res_p: PatternRef,
    res_diagind: Option<&[i32]>,
    mat: &[f64],
    p: PatternRef,
    mat_t: &[f64],
    pt: PatternRef,
    diag: Option<&[f64]>,
    scratch: &mut crate::memory::Scratch,
) {
    let nc = res_p.nr();
    let (res_rownnz, res_rowadr, res_colind) = (res_p.rownnz(), res_p.rowadr(), res_p.colind());
    let (rownnz, rowadr, colind) = (p.rownnz(), p.rowadr(), p.colind());
    let (rownnz_t, rowadr_t, colind_t, rowsuper_t) = (pt.rownnz(), pt.rowadr(), pt.colind(), pt.rowsuper());
    assert!(res_p.nc() == nc && pt.nr() == nc && p.nc() == nc && pt.nc() == p.nr(),
        "sqr_mat_tdsparse_numeric: the patterns' shapes disagree");
    if nc == 0 {
        return;
    }
    let mut buffer = scratch.num(nc * MAXSUPER);
    let mut buf_zeroed = 0;
    let mut r = 0;
    while r < nc {
        let mut ns = 1;
        if let Some(s) = rowsuper_t {
            ns = (s[r] + 1) as usize;
            if ns > MAXSUPER {
                ns = MAXSUPER;
            }
        }
        let nnz_t_r = rownnz_t[r] as usize;
        if nnz_t_r == 0 {
            for s in 0..ns {
                let row = r + s;
                let adr = res_rowadr[row];
                let nnz = res_diagind.map_or(res_rownnz[row], |di| di[row] - adr + 1);
                if nnz > 0 {
                    zero(&mut res[adr as usize..], nnz as usize);
                }
            }
            r += ns;
            continue;
        }
        let adr_r = rowadr_t[r] as usize;
        if ns == 1 {
            let res_adr = res_rowadr[r] as usize;
            let res_nnz = res_diagind.map_or(res_rownnz[r], |di| di[r] - res_adr as i32 + 1);
            let off_nnz = res_nnz - 1;
            let mut diag_r = 0.0;
            if off_nnz > 0 && r > buf_zeroed {
                zero(&mut buffer[buf_zeroed..], r - buf_zeroed);
                buf_zeroed = r;
            }
            for i in 0..nnz_t_r {
                let k = colind_t[adr_r + i] as usize;
                let val_t = mat_t[adr_r + i];
                let scale = diag.map_or(val_t, |di| di[k] * val_t);
                if scale == 0.0 {
                    continue;
                }
                diag_r += scale * val_t;
                if off_nnz > 0 {
                    // M's row k, sliced to its nonzeros, up to column r (which it holds); the accumulator to r
                    let adr_k = rowadr[k] as usize;
                    let end_k = adr_k + rownnz[k] as usize;
                    let buf = &mut buffer[..r];
                    for (&c, &v) in colind[adr_k..end_k].iter().zip(&mat[adr_k..end_k]) {
                        let c = c as usize;
                        if c >= r {
                            break;
                        }
                        buf[c] += scale * v;
                    }
                }
            }
            let res_ind = &res_colind[res_adr..];
            let res_val = &mut res[res_adr..];
            for j in 0..off_nnz as usize {
                let c = res_ind[j] as usize;
                res_val[j] = buffer[c];
                buffer[c] = 0.0;
            }
            res_val[off_nnz as usize] = diag_r;
        } else {
            let mut res_adr_s = [0; MAXSUPER];
            let mut off_nnz_s = [0; MAXSUPER];
            let mut any_off = 0;
            let mut mat_t_s: [&[f64]; MAXSUPER] = [&[]; MAXSUPER];
            for s in 0..ns {
                let row = r + s;
                let adr = res_rowadr[row];
                let nnz = res_diagind.map_or(res_rownnz[row], |di| di[row] - adr + 1);
                res_adr_s[s] = adr as usize;
                off_nnz_s[s] = nnz - (s + 1) as i32;
                any_off |= off_nnz_s[s];
                mat_t_s[s] = &mat_t[rowadr_t[row] as usize..];
            }
            if any_off != 0 && r * MAXSUPER > buf_zeroed {
                zero(&mut buffer[buf_zeroed..], r * MAXSUPER - buf_zeroed);
                buf_zeroed = r * MAXSUPER;
            }
            let mut block = [[0.0; MAXSUPER]; MAXSUPER];
            let mut scale = [0.0; MAXSUPER];
            for i in 0..nnz_t_r {
                let k = colind_t[adr_r + i] as usize;
                if let Some(di) = diag {
                    let dk = di[k];
                    if dk == 0.0 {
                        continue;
                    }
                    for s in 0..ns {
                        scale[s] = dk * mat_t_s[s][i];
                    }
                } else {
                    for s in 0..ns {
                        scale[s] = mat_t_s[s][i];
                    }
                }
                let adr_k = rowadr[k] as usize;
                let end_k = adr_k + rownnz[k] as usize;
                let (ind_k, val_k) = (&colind[adr_k..end_k], &mat[adr_k..end_k]);
                if ns <= 4 {
                    accum_super_row::<4>(&mut block, &mut buffer, &scale, &mat_t_s, ind_k, val_k, i, r, ns, any_off);
                } else {
                    accum_super_row::<MAXSUPER>(&mut block, &mut buffer, &scale, &mat_t_s, ind_k, val_k, i, r, ns, any_off);
                }
            }
            for s in 0..ns {
                let res_ind = &res_colind[res_adr_s[s]..];
                let res_val = &mut res[res_adr_s[s]..];
                let off_s = off_nnz_s[s] as usize;
                for j in 0..off_s {
                    let c = res_ind[j] as usize;
                    res_val[j] = buffer[c * MAXSUPER + s];
                    buffer[c * MAXSUPER + s] = 0.0;
                }
                for t in 0..=s {
                    res_val[off_s + t] = block[t][s];
                }
            }
        }
        r += ns;
    }
    if let Some(di) = res_diagind {
        let mut upper_pos = scratch.int(nc);
        for r in 0..nc {
            upper_pos[r] = di[r] + 1;
        }
        for r in 0..nc {
            let adr = res_rowadr[r];
            let lower_nnz = di[r] - adr + 1;
            for j in 0..lower_nnz {
                let c = res_colind[(adr + j) as usize] as usize;
                if c < r {
                    res[upper_pos[c] as usize] = res[(adr + j) as usize];
                    upper_pos[c] += 1;
                }
            }
        }
        scratch.put_int(upper_pos);
    }
    scratch.put_num(buffer);
}

pub fn sqr_mat_tdsparse(
    res: &mut [f64],
    mat: &[f64],
    mat_t: &[f64],
    diag: Option<&[f64]>,
    nr: usize,
    nc: usize,
    res_rownnz: &mut [i32],
    res_rowadr: &[i32],
    res_colind: &mut [i32],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    rowsuper: Option<&[i32]>,
    rownnz_t: &[i32],
    rowadr_t: &[i32],
    colind_t: &[i32],
    rowsuper_t: Option<&[i32]>,
    d: &mut Data,
    diagind: Option<&mut [i32]>,
) {
    let _ = rowsuper;
    let mat_csc = mat_t;
    let colnnz = rownnz_t;
    let coladr = rowadr_t;
    let rowind = colind_t;
    let colsuper = rowsuper_t;
    let mat_t_csc = mat;
    let colnnz_t = rownnz;
    let coladr_t = rowadr;
    let rowind_t = colind;
    let mut marker = d.scratch.int(nc);
    zero_int(&mut marker, nc);
    let mut buffer = d.scratch.num(nc * MAXSUPER);
    let mut buffer_idx = d.scratch.int(nc);
    let mut rowstart = d.scratch.int(nr);
    zero_int(&mut rowstart, nr);
    zero_int(res_rownnz, nc);
    let mut c = 0;
    while c < nc {
        let mut buffer_nnz = 0;
        let nnz = colnnz[c] as usize;
        let adr = coladr[c] as usize;
        let ind = &rowind[adr..];
        let mut val: [&[f64]; MAXSUPER] = [&[]; MAXSUPER];
        let mut ns = 1;
        val[0] = &mat_csc[adr..];
        if let Some(cs) = colsuper.map(|s| s[c]).filter(|&s| s != 0) {
            ns += (cs as usize).min(MAXSUPER - 1);
            for s in 1..ns {
                val[s] = &mat_csc[coladr[c + s] as usize..];
            }
        }
        let mut diag_c = [0.0; MAXSUPER];
        if let Some(di) = diag {
            for s in 0..ns {
                let mut ds = 0.0;
                for k in 0..nnz {
                    ds += (val[s][k] * val[s][k]) * di[ind[k] as usize];
                }
                diag_c[s] = ds;
            }
        } else {
            for s in 0..ns {
                diag_c[s] = dot(val[s], val[s], nnz);
            }
        }
        for i in 0..nnz {
            let r = ind[i] as usize;
            let adr_t = coladr_t[r] as usize;
            let nnz_t = colnnz_t[r] as usize;
            let ind_t = &rowind_t[adr_t..];
            let val_t = &mat_t_csc[adr_t..];
            let mut v = [0.0; MAXSUPER];
            if let Some(di) = diag {
                let diag_r = di[r];
                for s in 0..ns {
                    v[s] = diag_r * val[s][i];
                }
            } else {
                for s in 0..ns {
                    v[s] = val[s][i];
                }
            }
            for k in rowstart[r] as usize..nnz_t {
                let j = ind_t[k] as usize;
                if j <= c {
                    rowstart[r] += 1;
                    continue;
                }
                if marker[j] == 0 {
                    marker[j] = 1;
                    buffer_idx[buffer_nnz] = j as i32;
                    buffer_nnz += 1;
                    let vk = val_t[k];
                    for s in 0..ns {
                        buffer[s * nc + j] = vk * v[s];
                    }
                } else {
                    let vk = val_t[k];
                    for s in 0..ns {
                        buffer[s * nc + j] += vk * v[s];
                    }
                }
            }
        }
        for i in 0..buffer_nnz {
            let j = buffer_idx[i] as usize;
            marker[j] = 0;
            let adr_j = (res_rowadr[j] + res_rownnz[j]) as usize;
            let lower = j - c;
            let nm = ns.min(lower);
            res_rownnz[j] += nm as i32;
            for s in 0..nm {
                res[adr_j + s] = buffer[s * nc + j];
            }
            for s in 0..nm {
                res_colind[adr_j + s] = (c + s) as i32;
            }
        }
        for s in 0..ns {
            let adr_s = (res_rowadr[c + s] + res_rownnz[c + s]) as usize;
            res_rownnz[c + s] += 1;
            res_colind[adr_s] = (c + s) as i32;
            res[adr_s] = diag_c[s];
        }
        c += ns;
    }
    if let Some(di) = diagind {
        for i in 0..nc {
            di[i] = res_rowadr[i] + res_rownnz[i] - 1;
        }
        for i in 0..nc {
            let start = res_rowadr[i];
            let end = start + res_rownnz[i] - 1;
            for j in start..end {
                let j = j as usize;
                let col = res_colind[j] as usize;
                let adr = (res_rowadr[col] + res_rownnz[col]) as usize;
                res_rownnz[col] += 1;
                res[adr] = res[j];
                res_colind[adr] = i as i32;
            }
        }
    }
    d.scratch.put_int(marker);
    d.scratch.put_num(buffer);
    d.scratch.put_int(buffer_idx);
    d.scratch.put_int(rowstart);
}

pub fn sqr_mat_tdsparse_row(
    res: &mut [f64],
    mat: &[f64],
    mat_t: &[f64],
    diag: Option<&[f64]>,
    nr: usize,
    nc: usize,
    res_rownnz: &mut [i32],
    res_rowadr: &[i32],
    res_colind: &mut [i32],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    rowsuper: Option<&[i32]>,
    rownnz_t: &[i32],
    rowadr_t: &[i32],
    colind_t: &[i32],
    rowsuper_t: Option<&[i32]>,
    d: &mut Data,
    diagind: Option<&mut [i32]>,
) {
    let _ = (nr, rowsuper);
    let mut buffer = d.scratch.num(nc);
    let mut markers = d.scratch.int(nc);
    for i in 0..nc {
        let rowadr_i = res_rowadr[i] as usize;
        res_rownnz[i] = 0;
        buffer[i] = 0.0;
        markers[i] = 0;
        if rowsuper_t.is_some_and(|s| i > 0 && s[i - 1] != 0) {
            res_rownnz[i] = res_rownnz[i - 1];
            // C copies between rows in one array: preserve the input before writing.
            let cols_copy = res_colind
                [res_rowadr[i - 1] as usize..(res_rowadr[i - 1] + res_rownnz[i]) as usize]
                .to_vec();
            copy_int(
                &mut res_colind[rowadr_i..],
                &cols_copy,
                res_rownnz[i] as usize,
            );
        }
        let cols = &mut res_colind[rowadr_i..];
        let adr_t = rowadr_t[i] as usize;
        let end_r = adr_t + rownnz_t[i] as usize;
        for r in adr_t..end_r {
            let t = colind_t[r] as usize;
            let adr = rowadr[t] as usize;
            let end_c = adr + rownnz[t] as usize;
            for c in adr..end_c {
                let cc = colind[c] as usize;
                if cc > i {
                    break;
                }
                if let Some(di) = diag {
                    buffer[cc] += mat_t[r] * di[t] * mat[c];
                } else {
                    buffer[cc] += mat_t[r] * mat[c];
                }
                if markers[cc] == 0 {
                    markers[cc] = 1;
                    if cc == i {
                        cols[res_rownnz[i] as usize] = cc as i32;
                        res_rownnz[i] += 1;
                        continue;
                    }
                    let mut l = 0;
                    let mut h = res_rownnz[i] as usize;
                    while l < h {
                        let m = (l + h) >> 1;
                        if cols[m] < cc as i32 {
                            l = m + 1;
                        } else {
                            h = m;
                        }
                    }
                    if l == res_rownnz[i] as usize {
                        cols[l] = cc as i32;
                        res_rownnz[i] += 1;
                        continue;
                    }
                    h = res_rownnz[i] as usize;
                    while l < h {
                        cols[h] = cols[h - 1];
                        h -= 1;
                    }
                    cols[l] = cc as i32;
                    res_rownnz[i] += 1;
                }
            }
        }
        let end_r = res_rownnz[i] as usize;
        if rowsuper_t.is_some_and(|s| s[i] != 0) {
            for r in 0..end_r {
                let c = cols[r] as usize;
                res[rowadr_i + r] = buffer[c];
                buffer[c] = 0.0;
            }
        } else {
            for r in 0..end_r {
                let c = cols[r] as usize;
                let adr = rowadr_i + r;
                res[adr] = buffer[c];
                cols[r] = c as i32;
                buffer[c] = 0.0;
                markers[c] = 0;
            }
        }
    }
    if let Some(di) = diagind {
        for i in 0..nc {
            di[i] = res_rowadr[i] + res_rownnz[i] - 1;
        }
        for i in 0..nc {
            let start = res_rowadr[i];
            let end = start + res_rownnz[i] - 1;
            for j in start..end {
                let j = j as usize;
                let col = res_colind[j] as usize;
                let adr = (res_rowadr[col] + res_rownnz[col]) as usize;
                res_rownnz[col] += 1;
                res[adr] = res[j];
                res_colind[adr] = i as i32;
            }
        }
    }
    d.scratch.put_num(buffer);
    d.scratch.put_int(markers);
}

pub fn block(
    res: &mut [f64],
    mat: &[f64],
    nc_mat: usize,
    nc_res: usize,
    nr: usize,
    perm_r: &[i32],
    perm_c: &[i32],
) {
    for r in 0..nr {
        let res_r = &mut res[r * nc_res..];
        let mat_r = &mat[perm_r[r] as usize * nc_mat..];
        gather(res_r, mat_r, Some(perm_c), nc_res);
    }
}

pub fn block_diag(
    res: &mut [f64],
    mat: &[f64],
    nc_mat: usize,
    nc_res: usize,
    nb: usize,
    perm_r: &[i32],
    perm_c: &[i32],
    block_nr: &[i32],
    block_nc: &[i32],
    block_r: &[i32],
    block_c: &[i32],
) {
    for b in 0..nb {
        let adr = nc_res * block_r[b] as usize;
        block(
            &mut res[adr..],
            mat,
            nc_mat,
            block_nc[b] as usize,
            block_nr[b] as usize,
            &perm_r[block_r[b] as usize..],
            &perm_c[block_c[b] as usize..],
        );
    }
}

pub fn block_sparse(
    res: &mut [f64],
    res_rownnz: &mut [i32],
    res_rowadr: &mut [i32],
    res_colind: &mut [i32],
    mat: &[f64],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    nr: usize,
    perm_r: &[i32],
    perm_c: &[i32],
    col_offset: i32,
    res_offset: i32,
    mut res2: Option<&mut [f64]>,
    mat2: Option<&[f64]>,
) {
    for r in 0..nr {
        let k = perm_r[r] as usize;
        let nnz = rownnz[k];
        res_rownnz[r] = nnz;
        let res_adr = if r == 0 {
            res_offset
        } else {
            res_rowadr[r - 1] + res_rownnz[r - 1]
        };
        res_rowadr[r] = res_adr;
        let res_colind_r = &mut res_colind[(res_adr - res_offset) as usize..];
        let mat_adr = rowadr[k] as usize;
        let colind_k = &colind[mat_adr..];
        for j in 0..nnz as usize {
            res_colind_r[j] = perm_c[colind_k[j] as usize] - col_offset;
        }
        copy(
            &mut res[(res_adr - res_offset) as usize..],
            &mat[mat_adr..],
            nnz as usize,
        );
        if let (Some(m), Some(v)) = (mat2, res2.as_deref_mut()) {
            copy(
                &mut v[(res_adr - res_offset) as usize..],
                &m[mat_adr..],
                nnz as usize,
            );
        }
    }
}

pub fn block_diag_sparse(
    res: &mut [f64],
    res_rownnz: &mut [i32],
    res_rowadr: &mut [i32],
    res_colind: &mut [i32],
    mat: &[f64],
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    nr: usize,
    nb: usize,
    perm_r: &[i32],
    perm_c: &[i32],
    block_r: &[i32],
    block_c: &[i32],
    mut res2: Option<&mut [f64]>,
    mat2: Option<&[f64]>,
) {
    for b in 0..nb {
        let nr_block = (if b + 1 < nb {
            block_r[b + 1]
        } else {
            nr as i32
        }) - block_r[b];
        let r = block_r[b] as usize;
        let res_adr = if r == 0 {
            0
        } else {
            res_rowadr[r - 1] + res_rownnz[r - 1]
        };
        block_sparse(
            &mut res[res_adr as usize..],
            &mut res_rownnz[r..],
            &mut res_rowadr[r..],
            &mut res_colind[res_adr as usize..],
            mat,
            rownnz,
            rowadr,
            colind,
            nr_block as usize,
            &perm_r[r..],
            perm_c,
            block_c[b],
            res_adr,
            res2.as_deref_mut().map(|v| &mut v[res_adr as usize..]),
            mat2,
        );
    }
}

pub fn sort_sparse_row(nnz: usize, colind: &mut [i32], val: &mut [f64]) {
    let gap0 = if nnz > 24 { nnz >> 1 } else { 1 };
    let mut gap = gap0;
    while gap > 0 {
        for a in gap..nnz {
            let c = colind[a];
            let v = val[a];
            let mut b = a as i32 - gap as i32;
            while b >= 0 && colind[b as usize] > c {
                colind[(b + gap as i32) as usize] = colind[b as usize];
                val[(b + gap as i32) as usize] = val[b as usize];
                b -= gap as i32;
            }
            colind[(b + gap as i32) as usize] = c;
            val[(b + gap as i32) as usize] = v;
        }
        gap >>= 1;
    }
}

pub fn permute_lower_csr(
    nv: usize,
    perm: &[i32],
    dst_rownnz: &mut [i32],
    dst_rowadr: &mut [i32],
    dst_colind: &mut [i32],
    dst_val: &mut [f64],
    src_rownnz: &[i32],
    src_rowadr: &[i32],
    src_colind: &[i32],
    src_val: &[f64],
) {
    zero_int(dst_rownnz, nv);
    for r in 0..nv {
        let pr = perm[r];
        let end = (src_rowadr[r] + src_rownnz[r]) as usize;
        for j in src_rowadr[r] as usize..end {
            let pc = perm[src_colind[j] as usize];
            dst_rownnz[if pr >= pc { pr } else { pc } as usize] += 1;
        }
    }
    let mut adr = 0;
    for r in 0..nv {
        dst_rowadr[r] = adr;
        adr += dst_rownnz[r];
        dst_rownnz[r] = 0;
    }
    for r in 0..nv {
        let pr = perm[r];
        let end = (src_rowadr[r] + src_rownnz[r]) as usize;
        for j in src_rowadr[r] as usize..end {
            let pc = perm[src_colind[j] as usize];
            let row = if pr >= pc { pr } else { pc } as usize;
            let col = if pr >= pc { pc } else { pr };
            let w = (dst_rowadr[row] + dst_rownnz[row]) as usize;
            dst_rownnz[row] += 1;
            dst_colind[w] = col;
            dst_val[w] = src_val[j];
        }
    }
    for r in 0..nv {
        sort_sparse_row(
            dst_rownnz[r] as usize,
            &mut dst_colind[dst_rowadr[r] as usize..],
            &mut dst_val[dst_rowadr[r] as usize..],
        );
    }
}

pub fn min_degree(
    bperm: &mut [i32],
    adj: &mut [u64],
    nb: usize,
    bsz: Option<&[i32]>,
    n_l_out: Option<&mut usize>,
    flops_out: Option<&mut i64>,
    max_flops: i64,
    deg_scratch: &mut [i32],
    nz_bits: &mut [u64],
) -> i32 {
    let words = (nb + 63) / 64;
    let (deg, rest) = deg_scratch.split_at_mut(nb);
    let (head, rest) = rest.split_at_mut(nb);
    let (next, rest) = rest.split_at_mut(nb);
    let (prev, rest) = rest.split_at_mut(nb);
    let (nbr, nz_w) = rest.split_at_mut(nb);
    for d in 0..nb {
        head[d] = -1;
    }
    let mut mindeg = nb;
    for k in (0..nb).rev() {
        let ak = &mut adj[k * words..];
        ak[k >> 6] &= !(1u64 << (k & 63));
        let mut dg = 0;
        for w in 0..words {
            dg += popcount64(ak[w]);
        }
        deg[k] = dg;
        let h = head[dg as usize];
        prev[k] = -1;
        next[k] = h;
        if h >= 0 {
            prev[h as usize] = k as i32;
        }
        head[dg as usize] = k as i32;
        if (dg as usize) < mindeg {
            mindeg = dg as usize;
        }
    }
    let mut n_l = 0usize;
    let mut flops = 0i64;
    for step in 0..nb {
        while head[mindeg] < 0 {
            mindeg += 1;
        }
        let v = head[mindeg] as usize;
        let nxt = next[v];
        head[mindeg] = nxt;
        if nxt >= 0 {
            prev[nxt as usize] = -1;
        }
        deg[v] = -1;
        let av = &adj[v * words..];
        let (mut nw, mut nnbr, mut w_off) = (0, 0, 0);
        for w in 0..words {
            let mut bits = av[w];
            if bits != 0 {
                nz_w[nw] = w as i32;
                nz_bits[nw] = bits;
                nw += 1;
                while bits != 0 {
                    let u = (w << 6) + ctz64(bits) as usize;
                    bits &= bits - 1;
                    nbr[nnbr] = u as i32;
                    nnbr += 1;
                    if let Some(sz) = bsz {
                        w_off += sz[u];
                    }
                }
            }
        }
        if let Some(sz) = bsz {
            let sv = sz[v];
            for a in 1..=sv {
                let lnnz = (w_off + a) as i64;
                n_l += lnnz as usize;
                flops += lnnz * lnnz;
            }
            if max_flops != 0 && 5 * flops >= 4 * max_flops {
                if let Some(out) = n_l_out {
                    *out = n_l;
                }
                if let Some(out) = flops_out {
                    *out = flops;
                }
                return 0;
            }
        }
        bperm[v] = (nb - 1 - step) as i32;
        let vw = v >> 6;
        let vmask = !(1u64 << (v & 63));
        for idx in 0..nnbr {
            let u = nbr[idx] as usize;
            let old_dg = deg[u];
            let p = prev[u];
            let n = next[u];
            if p >= 0 {
                next[p as usize] = n;
            } else {
                head[old_dg as usize] = n;
            }
            if n >= 0 {
                prev[n as usize] = p;
            }
            let au = &mut adj[u * words..];
            au[vw] &= vmask;
            let mut dg = old_dg - 1;
            let uw = u >> 6;
            let ubit = 1u64 << (u & 63);
            for i in 0..nw {
                let w = nz_w[i] as usize;
                let mut add = nz_bits[i] & !au[w];
                if w == uw {
                    add &= !ubit;
                }
                if add != 0 {
                    au[w] |= add;
                    dg += popcount64(add);
                }
            }
            deg[u] = dg;
            let h = head[dg as usize];
            prev[u] = -1;
            next[u] = h;
            if h >= 0 {
                prev[h as usize] = u as i32;
            }
            head[dg as usize] = u as i32;
            if (dg as usize) < mindeg {
                mindeg = dg as usize;
            }
        }
    }
    if let Some(out) = n_l_out {
        *out = n_l;
    }
    if let Some(out) = flops_out {
        *out = flops;
    }
    1
}
