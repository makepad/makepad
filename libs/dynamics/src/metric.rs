//! The implicit effective metric of the discrete integrator.
//!
//! Translated from `src/engine/engine_metric.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2026 DeepMind Technologies Limited);
//! changed: translated to Rust; the flex parts (the vertex Cholesky factor,
//! the per-island flex lists, rod twist, passive flex contact and the flex
//! stiffness assembly) remain untranslated: models with flexes are refused.

use crate::core_smooth::{factor_i, solve_ld};
use crate::core_util::{
    actuator_damping, eff_actuator_possible, eff_couplings, eff_flex_any, eff_tendon_possible,
    eff_use_qh, flex_body_simple, flex_simple,
};
use crate::data::Data;
use crate::derivative::{
    d_actuator_deriv_skip, d_actuator_len_deriv, d_actuator_vel_deriv, d_ellipsoid_fluid,
    d_flex_interp_assemblable, d_flex_interp_cache_krot, d_flex_interp_mul, d_flex_interp_mul_bytes,
    d_flex_interp_processed, d_flex_passive_contact_any, d_flex_stiff_active_gen, d_flex_stiff_any,
    d_inertia_box_fluid,
};
use crate::inline::{at3, at4, copy4, sub3, sub_quat};
use crate::memory::{stack_bytes, stack_frame_bytes};
use crate::model::Model;
use crate::sleep::sleep_state;
use crate::types::{
    DSBL_DAMPER, DSBL_ISLAND, DSBL_SPRING, ENBL_IPC, ENBL_SLEEP, JNT_BALL, JNT_FREE, JNT_HINGE,
    JNT_SLIDE, MINVAL, NFLUID, NPOLY, OBJ_JOINT, OBJ_TENDON, SOL_CG, SOL_NEWTON, S_AWAKE,
    WARN_INERTIA,
};
use crate::util_blas::{add_to, add_to_scl, copy, dot, norm3, normalize4, zero};
use crate::util_errmem::{error, warning, warning_text};
use crate::util_misc::{d_x_poly_force, gather, is_zero, max, zero_int};
use crate::util_sparse::{dot_sparse, mul_sym_vec_sparse};

// EFMALLOC: the step's metric arrays are Data's Vecs, sized here and cleared by the arena reset.
fn eff_alloc<T: Copy + Default>(v: &mut Vec<T>, n: usize) {
    v.clear();
    v.resize(n, T::default());
}

#[derive(Default)]
pub struct EffRank1<'a> {
    pub val: &'a [f64],
    pub colind: &'a [i32],
    pub nnz: usize,
    pub scale: f64,
}

#[derive(Default)]
pub struct EffRank1Iter {
    pub cls: i32,
    pub i: usize,
    pub k: usize,
    pub colind: [i32; 2],
    pub val: [f64; 2],
}

// Yield the next live rank-1 term of the metric: classes 0 tendons, 1 actuators, 2 passive flex
// contact rows, 3 rod twist hinges.
pub fn eff_rank1_next<'a>(
    m: &'a Model,
    d: &'a Data,
    it: &mut EffRank1Iter,
    e: &mut EffRank1<'a>,
    flg_contact: i32,
) -> i32 {
    // class 0: one entry per tendon with metric terms
    while it.cls == 0 {
        if it.i >= d.nefm_t as usize {
            it.cls = 1;
            it.i = 0;
            break;
        }
        let t = it.i;
        it.i += 1;
        let s = d.efm_ts[t];
        let id = d.efm_tid[t] as usize;
        let nnz = m.ten_j_rownnz[id] as usize;
        if s == 0.0 || nnz == 0 {
            continue;
        }
        let adr = m.ten_j_rowadr[id] as usize;
        e.val = &d.ten_j[adr..];
        e.colind = &m.ten_j_colind[adr..];
        e.nnz = nnz;
        e.scale = s;
        return 1;
    }

    // class 1: one entry per output row of each actuator with metric terms
    while it.cls == 1 {
        if it.i >= d.nefm_a as usize {
            it.cls = 2;
            it.i = 0;
            it.k = 0; // class 2 walks the packed rows by offset
            break;
        }
        let id = d.efm_aid[it.i] as usize;
        if it.k >= m.actuator_outnum[id] as usize {
            it.i += 1;
            it.k = 0;
            continue;
        }
        let s = d.efm_as[it.i];
        let r = m.actuator_outadr[id] as usize + it.k;
        it.k += 1;
        let nnz = d.moment_rownnz[r] as usize;
        if s == 0.0 || nnz == 0 {
            continue;
        }
        let adr = d.moment_rowadr[r] as usize;
        e.val = &d.actuator_moment[adr..];
        e.colind = &d.moment_colind[adr..];
        e.nnz = nnz;
        e.scale = s;
        return 1;
    }

    // class 2: one entry per passive flex contact, published by effContactBuild. Skipped
    // wholesale when the caller accounts for contact separately
    while it.cls == 2 {
        if flg_contact == 0 || it.k >= d.nefmcon as usize {
            it.cls = 3;
            it.i = 0;
            it.k = 0;
            break;
        }
        let adr = it.k; // packed row: header then entries
        let nnz = d.efm_con_ind[adr] as usize;
        e.nnz = nnz;
        e.scale = d.efm_con_val[adr];
        e.colind = &d.efm_con_ind[adr + 2..];
        e.val = &d.efm_con_val[adr + 2..];
        it.k = adr + 2 + nnz;
        return 1;
    }

    // class 3: one entry per torsion hinge of 1D DER rods
    if it.cls == 3 {
        let passive = !(m.opt.disableflags & DSBL_SPRING != 0
            && m.opt.disableflags & DSBL_DAMPER != 0);
        if !passive || eff_couplings(m) == 0 {
            it.cls = 4;
            return 0;
        }
        while it.i < m.nflex {
            let f = it.i;
            if it.k == 0 && (m.flex_rigid[f] != 0 || unported!("mj_rodHasTwist")) {
                it.i += 1;
                continue;
            }
            unported!("mj_effRank1Next (rod twist)");
        }
        it.cls = 4;
    }
    0
}

// return 1 if i starts a 3-dof world-translation root body (its 3 dofs share one welded body at
// the root of its kinematic tree)
fn eff_simple_triple(m: &Model, i: usize) -> bool {
    if i + 3 > m.nv {
        return false;
    }
    let b = m.body_weldid[m.dof_bodyid[i] as usize] as usize;
    m.body_dofadr[b] == i as i32 && m.body_dofnum[b] == 3
        && (m.body_simple[b] == 2 || flex_body_simple(m, b) != 0)
}

// return 1 if every active flex can be assembled by the simple 3x3 vertex fast path
fn efm_all_simple(m: &Model) -> bool {
    if !d_flex_interp_assemblable(m) {
        return false;
    }
    for f in 0..m.nflex {
        if m.flex_interp[f] == 0 && d_flex_stiff_active_gen(m, f, true, true, true)
            && flex_simple(m, f) == 0
        {
            return false;
        }
    }
    true
}

// values of d.efm_active: the metric is active with the simple or the general stiffness CSR
const EFM_ACTIVE: i32 = 1;
const EFM_ACTIVE_GENERAL: i32 = 2;

// return 1 if d.efm_K was assembled in general articulated mode
fn efm_general_csr(d: &Data) -> bool {
    d.efm_active == EFM_ACTIVE_GENERAL
}

// res += B*vec, the stiffness part of the metric. The per-dof diagonal classes are applied
// from efm_diag (scales pre-folded); the stretch (and, when assemblable, bending and interp)
// part from the per-step CSR; terms not in the CSR fall back to the matrix-free operators.
pub fn eff_mul_add(m: &Model, d: &Data, res: &mut [f64], vec: &[f64], flg_contact: i32) {
    let h = m.opt.timestep;
    if !d.efm_diag.is_empty() {
        let nv = m.nv;
        for i in 0..nv {
            res[i] += d.efm_diag[i] * vec[i];
        }
    }

    // fluid drag blocks: res += F*vec, F symmetric in M's lower-triangle pattern
    if !d.efm_fluid.is_empty() {
        let nv = m.nv;
        for i in 0..nv {
            let start = m.m_rowadr[i] as usize;
            let diag = start + m.m_rownnz[i] as usize - 1;
            let mut acc = d.efm_fluid[diag] * vec[i];
            for a in start..diag {
                let c = m.m_colind[a] as usize;
                let f = d.efm_fluid[a];
                acc += f * vec[c];
                res[c] += f * vec[i];
            }
            res[i] += acc;
        }
    }

    // rank-1 terms (tendon, actuator): res += scale * row' * (row * vec)
    let mut it = EffRank1Iter::default();
    let mut e = EffRank1::default();
    while eff_rank1_next(m, d, &mut it, &mut e, 0) != 0 {
        let mut dot = 0.0;
        for j in 0..e.nnz {
            dot += e.val[j] * vec[e.colind[j] as usize];
        }
        dot *= e.scale;
        for j in 0..e.nnz {
            res[e.colind[j] as usize] += dot * e.val[j];
        }
    }

    // contact class: the same rank-1 apply over the packed rows, walked inline
    if flg_contact != 0 {
        let ind = &d.efm_con_ind;
        let val = &d.efm_con_val;
        let mut adr = 0;
        while adr < d.nefmcon as usize {
            let nnz = ind[adr] as usize;
            let ci = &ind[adr + 2..];
            let rv = &val[adr + 2..];
            let mut dot = 0.0;
            for j in 0..nnz {
                dot += rv[j] * vec[ci[j] as usize];
            }
            dot *= val[adr];
            for j in 0..nnz {
                res[ci[j] as usize] += dot * rv[j];
            }
            adr += 2 + nnz;
        }
    }

    let gen_csr = efm_general_csr(d);
    if d.nefm_k != 0 && d.nefmdof != 0 && !gen_csr {
        // the stiffness rows come in vertex triples with 3x3 blocks per neighbour
        let rowadr = &d.efm_k_rowadr;
        let rownnz = &d.efm_k_rownnz;
        let colind = &d.efm_k_colind;
        let val = &d.efm_k_val;
        for k in 0..d.nefmdof as usize {
            let i = d.efm_dofid[k] as usize;
            let v0 = &val[rowadr[i] as usize..];
            let v1 = &val[rowadr[i + 1] as usize..];
            let v2 = &val[rowadr[i + 2] as usize..];
            let ci = &colind[rowadr[i] as usize..];
            let nn = (rownnz[i] / 3) as usize;
            let (mut r0, mut r1, mut r2) = (0.0, 0.0, 0.0);
            for j in 0..nn {
                let c = ci[3 * j] as usize;
                let (x0, x1, x2) = (vec[c], vec[c + 1], vec[c + 2]);
                r0 += v0[3 * j] * x0 + v0[3 * j + 1] * x1 + v0[3 * j + 2] * x2;
                r1 += v1[3 * j] * x0 + v1[3 * j + 1] * x1 + v1[3 * j + 2] * x2;
                r2 += v2[3 * j] * x0 + v2[3 * j + 1] * x1 + v2[3 * j + 2] * x2;
            }
            res[i] += r0;
            res[i + 1] += r1;
            res[i + 2] += r2;
        }
    } else if d.nefm_k != 0 {
        let nv = m.nv;
        for i in 0..nv {
            let nnz = d.efm_k_rownnz[i] as usize;
            if nnz == 0 {
                continue;
            }
            let adr = d.efm_k_rowadr[i] as usize;
            res[i] += dot_sparse(&d.efm_k_val[adr..], vec, nnz, &d.efm_k_colind[adr..]);
        }
    }

    // un-assembled flexes dispatch their matrix-free operators individually
    if !gen_csr {
        let s1 = if m.opt.disableflags & DSBL_SPRING != 0 { 0.0 } else { h * h };
        let s2 = if m.opt.disableflags & DSBL_DAMPER != 0 { 0.0 } else { h };
        for f in 0..m.nflex {
            if m.flex_interp[f] != 0 || m.flex_rigid[f] != 0 { continue; }
            if m.flex_dim[f] == 1 && m.flex_bendingadr[f] < 0 { continue; }
            if d.nefm_k == 0 || flex_simple(m, f) == 0 {
                unported!("mjd_flexBend_mulRange");
            }
        }
        if d.nefm_k == 0 || !d_flex_interp_assemblable(m) {
            d_flex_interp_mul(m, d, res, vec, -s1, -s2, Some(&d.flexelem_krot));
        }
    }
}

// Compute the model's minimum-degree vertex ordering for flex stencils.
pub fn eff_chol_set_const(m: &mut Model, _d: &mut Data) {
    let ncap = m.nefm_cvert;
    if ncap == 0 {
        return;
    }
    crate::unported!("mj_effCholSetConst (active metric)");
}

const EFF_CHOL_PENDING: f64 = 2.0;
const EFF_ISLAND_TAG: f64 = -1.0;

fn eff_chol_reserved(d: &Data) -> bool {
    let nb = d.nefmdof;
    nb != 0 && d.nefm_l > 0 && d.efm_l[0] >= 0.0
}

// Only the lists read by the paths translated so far are needed before unpacking.
#[derive(Default)]
struct EffIsland<'a> {
    cov_adr: &'a [i32],
    bend_adr: &'a [i32],
    flex_adr: &'a [i32],
}

fn eff_island_step(d: &Data, _isl: &mut EffIsland<'_>) -> bool {
    if d.nefm_l <= 0 || d.efm_l[0] != EFF_ISLAND_TAG {
        return false;
    }
    // C stores the packed int header immediately after the first mjtNum.
    let bytes = d.efm_l[1].to_ne_bytes();
    let a0 = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
    if a0 != d.nisland {
        return false;
    }
    unported!("effIslandStep (active metric)");
}

// The numeric factorization of the covered flex block runs lazily. Its mutation of d (the
// status entry) is reached only with a reserved factor, which only flexes make.
fn eff_chol_ensure(_m: &Model, d: &Data) {
    if !eff_chol_reserved(d) || d.efm_l[0] != EFF_CHOL_PENDING {
        return;
    }
    unported!("effCholEnsure (active metric)");
}

fn eff_chol_ready(_m: &Model, d: &Data, _is_island: i32) -> bool {
    if !eff_chol_reserved(d) {
        return false;
    }
    unported!("effCholReady (active metric)");
}

fn eff_chol_solve(m: &Model, d: &Data, _x: &mut [f64], _b: &[f64]) -> bool {
    if !eff_chol_ready(m, d, 0) {
        return false;
    }
    unported!("effCholApply (active metric)");
}

pub fn eff_fold_diag_size(m: &Model, d: &Data, _island: i32) -> usize {
    if d.efm_active == 0 {
        return 0;
    }
    if d.nefmdof > 0 {
        unported!("mj_effFoldDiagSize (active metric)");
    }
    let passive = !(m.opt.disableflags & DSBL_SPRING != 0
        && m.opt.disableflags & DSBL_DAMPER != 0);
    if !passive || eff_couplings(m) == 0 {
        return 0;
    }
    for f in 0..m.nflex {
        if m.flex_rigid[f] != 0 || unported!("mj_rodHasTwist") {
            continue;
        }
        unported!("mj_effFoldDiagSize (active metric)");
    }
    0
}

// exact solve of each 1D DER rod twist chain (tridiagonal); every chain is a flex
fn eff_twist_solve(m: &Model, d: &Data, _x: &mut [f64], _rhs: &[f64], _island: i32, _tw_diag: Option<&[f64]>) {
    let passive = !(m.opt.disableflags & DSBL_SPRING != 0
        && m.opt.disableflags & DSBL_DAMPER != 0);
    if !passive || eff_couplings(m) == 0 || d.efm_sdiag.is_empty() {
        return;
    }
    for f in 0..m.nflex {
        if m.flex_rigid[f] != 0 || unported!("mj_rodHasTwist") {
            continue;
        }
        unported!("effTwistSolve (active metric)");
    }
}

// L: folded per-dof diagonal (mj_effPrecFold), or None. b None is C's x == b.
fn eff_block_apply(m: &Model, d: &Data, l: Option<&[f64]>, x: &mut [f64], b: Option<&[f64]>) {
    let nv = m.nv;
    let nbd = m.nefm0dof;
    let flg_bend = nbd != 0 && d.nefmdof == 0;

    // b aliases x, which the sweep below overwrites. No current caller passes it; the copy
    // replaces C's stack buffer (every path below reads an element of b before writing it).
    let buf;
    let rhs: &[f64] = match b {
        Some(b) => b,
        None => {
            buf = x[..nv].to_vec();
            &buf
        }
    };

    // every dof a covered triple: folded diagonal (CG) or sparse factor (1D/2D) is the whole
    // preconditioner
    if 3 * d.nefmdof as usize == nv && !flg_bend {
        if let Some(l) = l {
            let sleep_dof = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
            if sleep_dof {
                for k in 0..d.nefmdof as usize {
                    let i = d.efm_dofid[k] as usize;
                    if d.tree_awake[m.dof_treeid[i] as usize] == 0 {
                        zero(&mut x[i..], 3);
                    } else {
                        x[i] = rhs[i] * l[i];
                        x[i + 1] = rhs[i + 1] * l[i + 1];
                        x[i + 2] = rhs[i + 2] * l[i + 2];
                    }
                }
            } else {
                for r in 0..nv {
                    x[r] = rhs[r] * l[r];
                }
            }
            return;
        }
        if eff_chol_solve(m, d, x, rhs) {
            return;
        }
    }
    let use_chol = l.is_none() && eff_chol_ready(m, d, 0);
    let use_fold = l.is_some() && d.nefmdof > 0;

    // dofs no factor covers: backbone solve, using the qH backbone factor when diagonal
    // terms exist, else qLD. PCG requires symmetry, so dofs covered by L, effChol, or efm0_L are
    // zeroed before the sweep
    copy(x, rhs, nv);
    if use_chol || use_fold {
        for k in 0..d.nefmdof as usize {
            zero(&mut x[d.efm_dofid[k] as usize..], 3);
        }
    }
    if flg_bend {
        for i in 0..nbd {
            x[m.efm0_dofid[i] as usize] = 0.0;
        }
    }
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nv_awake as usize) < nv;
    let n = if sleep_filter { d.nv_awake as usize } else { nv };
    let index = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
    if eff_use_qh(d) != 0 {
        solve_ld(x, &d.q_h, &d.q_hdiag_inv, n, 1, &m.m_rownnz, &m.m_rowadr, &m.m_colind, index);
        if !d.efm_sdiag.is_empty() {
            eff_twist_solve(m, d, x, rhs, -1, l);
        }
    } else {
        solve_ld(x, &d.q_ld, &d.q_ldiag_inv, n, 1, &m.m_rownnz, &m.m_rowadr, &m.m_colind, index);
    }

    // folded per-dof diagonal (CG) or per-step sparse factor (1D rods and 2D flexes)
    if use_fold {
        let l = l.unwrap();
        let sleep_dof = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
        for k in 0..d.nefmdof as usize {
            let i = d.efm_dofid[k] as usize;
            if sleep_dof && d.tree_awake[m.dof_treeid[i] as usize] == 0 {
                zero(&mut x[i..], 3);
            } else {
                x[i] = rhs[i] * l[i];
                x[i + 1] = rhs[i + 1] * l[i + 1];
                x[i + 2] = rhs[i + 2] * l[i + 2];
            }
        }
    } else if use_chol {
        unported!("effCholApply (active metric)");
    }

    // bending-only: exact (M + K_bend)^-1 on the dofs the constant factor covers
    if flg_bend {
        unported!("effBlockApply (bending-only flex factor)");
    }
}

// C's %.2e: two decimals and a signed exponent of at least two digits
fn fmt_e2(x: f64) -> String {
    let s = format!("{x:.2e}");
    match s.split_once('e') {
        Some((mant, exp)) => {
            let e: i32 = exp.parse().unwrap_or(0);
            format!("{mant}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
        }
        None => s.to_lowercase(),
    }
}

// accurate solve of (M + K) x = b by PCG with the metric preconditioner, converging the
// relative residual to opt.tolerance; used for qacc_smooth. Reaching opt.iterations means the
// metric is too ill-conditioned for the preconditioner: warn (mjWARN_INERTIA, worst-residual dof)
// and return x under-converged. None represents C's x == b, as in core_smooth::solve_m.
pub fn eff_solve(m: &Model, d: &mut Data, x: &mut [f64], b: Option<&[f64]>) {
    if d.efm_active == 0 {
        eff_prec(m, d, x, b);
        return;
    }

    // backbone-only metric (no tendon, actuator or flex couplings): the qH solve inside the
    // block-apply is the exact metric solve, skip the iteration
    if d.nefm_t == 0 && d.nefm_a == 0 && eff_flex_any(m) == 0 {
        eff_block_apply(m, d, None, x, b);
        return;
    }
    let nv = m.nv;
    let mut r = d.scratch.num(nv);
    let mut z = d.scratch.num(nv);
    let mut p = d.scratch.num(nv);
    let mut ap = d.scratch.num(nv);
    // before zeroing x: b may alias x
    match b {
        Some(b) => copy(&mut r, b, nv),
        None => copy(&mut r, x, nv),
    }
    zero(x, nv);
    let bn = dot(&r, &r, nv);
    if bn > MINVAL {
        // converge the relative residual to opt.tolerance; both sides are squared norms
        let tolerance = m.opt.tolerance;
        let tol = tolerance * tolerance * bn;
        let mut capped = true; // cleared by either exit below; still set means the cap was reached
        eff_block_apply(m, d, None, &mut z, Some(&r));
        copy(&mut p, &z, nv);
        let mut rz = dot(&r, &z, nv);
        for _it in 0..m.opt.iterations {
            mul_sym_vec_sparse(&mut ap, &d.m, &p, nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind);
            eff_mul_add(m, d, &mut ap, &p, 1);
            let p_ap = dot(&p, &ap, nv);
            // curvature breakdown: no further progress is possible and x is the best available
            if p_ap <= 0.0 {
                capped = false;
                break;
            }
            let alpha = rz / p_ap;
            add_to_scl(x, &p, alpha, nv);
            add_to_scl(&mut r, &ap, -alpha, nv);
            if dot(&r, &r, nv) < tol {
                capped = false;
                break;
            }
            eff_block_apply(m, d, None, &mut z, Some(&r));
            let rznew = dot(&r, &z, nv);
            // mju_addScl aliases its second input (p = z + p*s); it reads each element first
            let s = rznew / rz;
            for i in 0..nv {
                p[i] = z[i] + p[i] * s;
            }
            rz = rznew;
        }

        // ran out of iterations with the residual still above tolerance: blame the dof carrying
        // the largest residual, saying first that the cause is the stiffness, on the first time
        if capped && dot(&r, &r, nv) >= tol {
            let mut worst = 0;
            for i in 1..nv {
                if r[i].abs() > r[worst].abs() {
                    worst = i;
                }
            }
            if d.warning[WARN_INERTIA as usize].number == 0 {
                warning_text(&format!(
                    "Flex stiffness is too ill-conditioned for the effective-metric block \
                     preconditioner: the M+K solve ran out of iterations at a relative residual \
                     of {} and qacc_smooth is under-converged. Reported as a singular inertia \
                     below, because M+K is the effective inertia.",
                    fmt_e2((dot(&r, &r, nv) / bn).sqrt())
                ));
            }
            warning(d, WARN_INERTIA, worst as i32);
        }
    } else if crate::thread::has_pool(d) {
        eff_chol_ensure(m, d);
    }
    d.scratch.put_num(ap);
    d.scratch.put_num(p);
    d.scratch.put_num(z);
    d.scratch.put_num(r);
}

pub fn eff_prec(m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>) {
    let nv = m.nv;

    // active metric: apply the step's metric preconditioner
    if d.efm_active != 0 {
        eff_block_apply(m, d, None, x, b);
        return;
    }

    // inactive metric: x = M \ b, which is exact
    if let Some(b) = b {
        copy(x, b, nv);
    }
    solve_ld(x, &d.q_ld, &d.q_ldiag_inv, nv, 1, &m.m_rownnz, &m.m_rowadr, &m.m_colind, None);
}

pub fn eff_prec_fold(
    m: &Model, d: &Data, _l: &mut [f64], island: i32, _nefc: usize,
    _efc_d: &[f64], _is_sparse: i32, _j: &[f64], _j_rownnz: &[i32],
    _j_rowadr: &[i32], _j_colind: &[i32],
) -> i32 {
    let nfold = eff_fold_diag_size(m, d, island);
    let ncol = if island >= 0 { d.island_nv[island as usize] as i64 } else { m.nv as i64 };
    if nfold == 0 || d.efm_active == 0 || ncol <= 0 {
        return 0;
    }
    unported!("mj_effPrecFold (active metric)");
}

pub fn eff_prec_blocks(m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>, l: Option<&[f64]>) {
    if d.efm_active != 0 {
        eff_block_apply(m, d, l, x, b);
        return;
    }
    eff_prec(m, d, x, b);
}

// the step's metric buffer holds the sparse factor, which leaves no room for the per-island lists
// (effIslandBuild): a CG solve then runs monolithically, unless sleep needs the island path
pub fn eff_cgmonolithic(m: &Model, d: &Data, solver: i32) -> i32 {
    (solver == SOL_CG && d.efm_active != 0 && m.opt.enableflags & ENBL_SLEEP == 0 && eff_chol_reserved(d)) as i32
}

pub fn eff_mul_add_scratch(m: &Model, d: &Data) -> usize {
    if efm_general_csr(d) { return 0; }
    let frame = stack_frame_bytes();
    let mut bytes = 0;
    for f in 0..m.nflex {
        if m.flex_interp[f] != 0 || m.flex_rigid[f] != 0 { continue; }
        if m.flex_dim[f] == 1 && m.flex_bendingadr[f] < 0 { continue; }
        if d.nefm_k == 0 || flex_simple(m, f) == 0 {
            let b = frame + 2 * stack_bytes(size_of::<f64>() * 3 * m.flex_vertnum[f] as usize, align_of::<f64>());
            bytes = if bytes > b { bytes } else { b };
        }
    }
    if d.nefm_k == 0 || !d_flex_interp_assemblable(m) {
        let b = d_flex_interp_mul_bytes(m);
        bytes = if bytes > b { bytes } else { b };
    }
    if bytes != 0 && m.opt.disableflags & DSBL_ISLAND == 0 && m.opt.enableflags & ENBL_IPC == 0 {
        bytes += frame + 2 * stack_bytes(size_of::<f64>() * m.nv, align_of::<f64>());
    }
    bytes
}

fn eff_chol_apply_scratch(_m: &Model, d: &Data) -> usize {
    if !eff_chol_reserved(d) { return 0; }
    unported!("effCholApplyScratch (active metric)");
}

pub fn eff_prec_scratch(m: &Model, d: &Data) -> usize {
    let sn = size_of::<f64>();
    let an = align_of::<f64>();
    let twist_apply = 0;
    for f in 0..m.nflex {
        if m.flex_rigid[f] == 0 && unported!("mj_rodHasTwist") {
            unported!("mj_effPrecScratch (active metric)");
        }
    }
    let chol_apply = eff_chol_apply_scratch(m, d);
    let mut apply = stack_frame_bytes() + stack_bytes(sn * m.nv, an)
        + if chol_apply > twist_apply { chol_apply } else { twist_apply };
    if m.nefm0dof != 0 && d.nefmdof == 0 {
        apply += 2 * stack_bytes(sn * m.nefm0dof, an);
    }
    apply
}

fn dof_damp_possible(m: &Model, i: usize) -> bool {
    if m.opt.disableflags & DSBL_DAMPER != 0 {
        return false;
    }
    m.dof_damping[i] > 0.0
        || is_zero(&m.dof_dampingpoly[NPOLY * i..], NPOLY) == 0
        || m.jnt_actuatorid[m.dof_jntid[i] as usize] != -1
}

// fill efm_ck with h * d(spring force)/d(displacement) per joint dof, clamped PSD-safe,
// mirroring the joint-spring structure of mj_springdamper; returns any-nonzero
fn eff_diag_stiff(m: &Model, d: &mut Data) -> bool {
    let mut any = false;
    let njnt = m.njnt;
    let h = m.opt.timestep;
    zero(&mut d.efm_ck, m.nv);
    if m.opt.disableflags & DSBL_SPRING != 0 {
        return false;
    }

    for j in 0..njnt {
        let stiffness = m.jnt_stiffness[j];
        let spoly = &m.jnt_stiffnesspoly[NPOLY * j..];
        if stiffness == 0.0 && is_zero(spoly, NPOLY) != 0 {
            continue;
        }

        let mut padr = m.jnt_qposadr[j] as usize;
        let mut dadr = m.jnt_dofadr[j] as usize;
        let mut qspring = if m.jnt_plasticadr[j] >= 0 {
            &d.plastic[m.jnt_plasticadr[j] as usize..]
        } else {
            &m.qpos_spring[padr..]
        };

        // C's free-joint case falls through to the ball-joint rotation.
        if m.jnt_type[j] == JNT_FREE {
            // translation: radial derivative of the spring, isotropic on the 3 slide dofs
            let mut dif = [0.0; 3];
            sub3(&mut dif, at3(&d.qpos, padr), at3(qspring, 0));
            let k = max(0.0, d_x_poly_force(stiffness, spoly, norm3(&dif), NPOLY, 0));
            d.efm_ck[dadr] = h * k;
            d.efm_ck[dadr + 1] = h * k;
            d.efm_ck[dadr + 2] = h * k;
            any = any || k > 0.0;

            // continue with rotations
            dadr += 3;
            padr += 3;
            qspring = &qspring[3..];
        }
        match m.jnt_type[j] {
            JNT_FREE | JNT_BALL => {
                // rotation: small-angle isotropic treatment of the quaternion spring
                let mut dif = [0.0; 3];
                let mut quat = [0.0; 4];
                copy4(&mut quat, at4(&d.qpos, padr));
                normalize4(&mut quat);
                sub_quat(&mut dif, &quat, at4(qspring, 0));
                let k = max(0.0, d_x_poly_force(stiffness, spoly, norm3(&dif), NPOLY, 0));
                d.efm_ck[dadr] = h * k;
                d.efm_ck[dadr + 1] = h * k;
                d.efm_ck[dadr + 2] = h * k;
                any = any || k > 0.0;
            }
            JNT_SLIDE | JNT_HINGE => {
                let x = d.qpos[padr] - qspring[0];
                let k = max(0.0, d_x_poly_force(stiffness, spoly, x, NPOLY, 0));
                d.efm_ck[dadr] = h * k;
                any = any || k > 0.0;
            }
            _ => {}
        }
    }

    any
}

// finalize efm_diag = h*D + h^2*K: the damping derivative is velocity-dependent,
// evaluated here at the velocity stage, matching mj_EulerSkip's coverage but clamped
fn eff_diag_damp(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let h = m.opt.timestep;
    let damper = m.opt.disableflags & DSBL_DAMPER == 0;

    for i in 0..nv {
        let mut damp_deriv = 0.0;
        if damper {
            let mut poly = [0.0; NPOLY];
            copy(&mut poly, &m.dof_dampingpoly[NPOLY * i..], NPOLY);
            let damping = m.dof_damping[i]
                + actuator_damping(m, OBJ_JOINT, m.dof_jntid[i] as usize, &mut poly);
            damp_deriv = max(0.0, d_x_poly_force(damping, &poly, d.qvel[i], NPOLY, 1));
        }
        d.efm_diag[i] = h * damp_deriv + h * d.efm_ck[i];
    }
}

// refresh the velocity-stage values of the active metric: the smooth-force shift
// c = -h*K*qvel and the diagonal (its damping part is velocity-dependent). Called from the
// velocity stage only, after the derived velocities it reads are computed
pub fn eff_shift(m: &Model, d: &mut Data) {
    if d.efm_active == 0 {
        return;
    }
    let h = m.opt.timestep;
    zero(&mut d.efm_c, m.nv);

    // flex stiffness shift, absent when the spring force is disabled
    if m.opt.disableflags & DSBL_SPRING == 0 {
        // the product reads d and writes efm_c, which it never reads
        let mut efm_c = std::mem::take(&mut d.efm_c);
        d_flex_interp_mul(m, d, &mut efm_c, &d.qvel, h, 0.0, Some(&d.flexelem_krot));
        d.efm_c = efm_c;
        if m.nflex != 0 {
            unported!("mjd_flexBend_mul, mjd_flexStretch_mul");
        }

        // 1D DER rod torsional coupling shift
        if eff_couplings(m) != 0 {
            for f in 0..m.nflex {
                if m.flex_rigid[f] != 0 || unported!("mj_rodHasTwist") {
                    continue;
                }
                unported!("mj_effShift (rod twist)");
            }
        }
    }

    // contact: -h*K_contact*v from the packed rank-1 rows; each row's scale already carries
    // the h^2 of the effective stiffness, so the shift is -(1/h) * scale * row'(row.v)
    let mut adr = 0;
    while adr < d.nefmcon as usize {
        let nnz = d.efm_con_ind[adr] as usize;
        let mut dot = 0.0;
        for a in 0..nnz {
            dot += d.efm_con_val[adr + 2 + a] * d.qvel[d.efm_con_ind[adr + 2 + a] as usize];
        }
        dot *= -d.efm_con_val[adr] / h;
        for a in 0..nnz {
            d.efm_c[d.efm_con_ind[adr + 2 + a] as usize] += dot * d.efm_con_val[adr + 2 + a];
        }
        adr += 2 + nnz;
    }

    // per-dof diagonal classes
    if !d.efm_diag.is_empty() {
        let nv = m.nv;
        eff_diag_damp(m, d);

        // stiffness shift
        for i in 0..nv {
            d.efm_c[i] -= d.efm_ck[i] * d.qvel[i];
        }
    }

    // tendon values: stiffness at the current deadband displacement, damping at the current
    // tendon velocity, both clamped PSD-safe, mirroring the passive-force expressions
    for t in 0..d.nefm_t as usize {
        let i = d.efm_tid[t] as usize;

        let mut k = 0.0;
        if m.opt.disableflags & DSBL_SPRING == 0 {
            let length = d.ten_length[i];
            let lengthspring = if m.tendon_plasticadr[i] >= 0 {
                &d.plastic[m.tendon_plasticadr[i] as usize..]
            } else {
                &m.tendon_lengthspring[2 * i..]
            };
            let lower = lengthspring[0];
            let upper = lengthspring[1];
            let x = if length > upper {
                length - upper
            } else if length < lower {
                length - lower
            } else {
                0.0
            };
            if x != 0.0 {
                k = max(0.0, d_x_poly_force(m.tendon_stiffness[i],
                    &m.tendon_stiffnesspoly[NPOLY * i..], x, NPOLY, 0));
            }
        }

        let mut b = 0.0;
        if m.opt.disableflags & DSBL_DAMPER == 0 {
            let mut dpoly = [0.0; NPOLY];
            copy(&mut dpoly, &m.tendon_dampingpoly[NPOLY * i..], NPOLY);
            let damping = m.tendon_damping[i] + actuator_damping(m, OBJ_TENDON, i, &mut dpoly);
            b = max(0.0, d_x_poly_force(damping, &dpoly, d.ten_velocity[i], NPOLY, 1));
        }

        d.efm_ts[t] = h * h * k + h * b;
        d.efm_tk[t] = h * k;

        // stiffness shift: c -= h*k * ten_velocity * J'
        if k != 0.0 {
            let ckv = d.efm_tk[t] * d.ten_velocity[i];
            let start = m.ten_j_rowadr[i] as usize;
            let end = start + m.ten_j_rownnz[i] as usize;
            for j in start..end {
                d.efm_c[m.ten_j_colind[j] as usize] -= ckv * d.ten_j[j];
            }
        }
    }

    // fluid drag blocks: assemble the drag-only passive-fluid velocity derivative into scratch
    // in qDeriv's shape, gather into M's sparsity pattern, scale by -h
    if !d.efm_fluid.is_empty() {
        let mut scratch = d.scratch.num(m.n_d);
        zero(&mut scratch, m.n_d);
        let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
        let nbody = if sleep_filter { d.nbody_awake as usize } else { m.nbody };
        for b in 0..nbody {
            let i = if sleep_filter { d.body_awake_ind[b] as usize } else { b };
            if m.body_mass[i] < MINVAL {
                continue;
            }
            let mut use_ellipsoid_model = 0;
            let geomnum = m.body_geomnum[i] as usize;
            let mut j = 0;
            while j < geomnum && use_ellipsoid_model == 0 {
                let geomid = m.body_geomadr[i] as usize + j;
                use_ellipsoid_model += (m.geom_fluid[NFLUID * geomid] > 0.0) as i32;
                j += 1;
            }
            if use_ellipsoid_model != 0 {
                d_ellipsoid_fluid(m, d, &mut scratch, i, 1);
            } else {
                d_inertia_box_fluid(m, d, &mut scratch, i);
            }
        }
        gather(&mut d.efm_fluid, &scratch, Some(&m.map_d2m), m.n_c);
        // mju_scl aliases its input; it reads each element first
        let s = -m.opt.timestep;
        for v in &mut d.efm_fluid[..m.n_c] {
            *v *= s;
        }
        d.scratch.put_num(scratch);
    }
}

// actuation-stage refresh of the metric: actuator gain scalars, their smooth-force shift, and
// the backbone factor qH = M + diag(h*D + h^2*K) + tendon and actuator diagonals. If flg_factor
// is 0 the backbone is assembled (efm_sdiag reads its diagonal) but not factored
pub fn eff_actuation(m: &Model, d: &mut Data, flg_factor: i32) {
    if d.efm_active == 0 {
        return;
    }
    let nv = m.nv;
    let h = m.opt.timestep;

    // actuator gains, clamped to the stabilizing sign
    d.nefm_a = 0;
    if !d.efm_aid.is_empty() {
        zero(&mut d.efm_ca, nv);
        let sleep_filter = (m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32) as i32;
        for i in 0..m.nactuator {
            if eff_actuator_possible(m, i) == 0 || d_actuator_deriv_skip(m, d, i, sleep_filter) != 0 {
                continue;
            }
            let gv = max(0.0, -d_actuator_vel_deriv(m, d, i));
            let gp = max(0.0, -d_actuator_len_deriv(m, d, i));
            if gv == 0.0 && gp == 0.0 {
                continue;
            }
            let t = d.nefm_a as usize;
            d.nefm_a += 1;
            d.efm_aid[t] = i as i32;
            d.efm_as[t] = h * h * gp + h * gv;
            d.efm_ak[t] = h * gp;

            // stiffness shift: ca -= h*gp * actuator_velocity * moment'
            if gp != 0.0 {
                let oadr = m.actuator_outadr[i] as usize;
                for k in 0..m.actuator_outnum[i] as usize {
                    let r = oadr + k;
                    let ckv = d.efm_ak[t] * d.actuator_velocity[r];
                    let start = d.moment_rowadr[r] as usize;
                    let end = start + d.moment_rownnz[r] as usize;
                    for j in start..end {
                        d.efm_ca[d.moment_colind[j] as usize] -= ckv * d.actuator_moment[j];
                    }
                }
            }
        }
    }

    // factor the backbone qH = M + all metric diagonals
    if eff_use_qh(d) != 0 {
        copy(&mut d.q_h, &d.m, m.n_c);
        if !d.efm_diag.is_empty() {
            for i in 0..nv {
                d.q_h[(m.m_rowadr[i] + m.m_rownnz[i] - 1) as usize] += d.efm_diag[i];
            }
            for t in 0..d.nefm_t as usize {
                let s = d.efm_ts[t];
                if s == 0.0 {
                    continue;
                }
                let i = d.efm_tid[t] as usize;
                let start = m.ten_j_rowadr[i] as usize;
                let end = start + m.ten_j_rownnz[i] as usize;
                for j in start..end {
                    let c = m.ten_j_colind[j] as usize;
                    d.q_h[(m.m_rowadr[c] + m.m_rownnz[c] - 1) as usize] += s * d.ten_j[j] * d.ten_j[j];
                }
            }
            for t in 0..d.nefm_a as usize {
                let s = d.efm_as[t];
                let i = d.efm_aid[t] as usize;
                let oadr = m.actuator_outadr[i] as usize;
                for k in 0..m.actuator_outnum[i] as usize {
                    let r = oadr + k;
                    let start = d.moment_rowadr[r] as usize;
                    let end = start + d.moment_rownnz[r] as usize;
                    for j in start..end {
                        let c = d.moment_colind[j] as usize;
                        d.q_h[(m.m_rowadr[c] + m.m_rownnz[c] - 1) as usize] +=
                            s * d.actuator_moment[j] * d.actuator_moment[j];
                    }
                }
            }
            // 1D DER rod torsional coupling diagonal: class 3 yields only rod twist hinges
            let mut it_tw = EffRank1Iter { cls: 3, ..Default::default() };
            let mut e_tw = EffRank1::default();
            while eff_rank1_next(m, d, &mut it_tw, &mut e_tw, 0) != 0 {
                unported!("mj_effActuation (rod twist diagonal)");
            }
            // fluid drag blocks share M's sparsity: add the full pattern
            if !d.efm_fluid.is_empty() {
                add_to(&mut d.q_h, &d.efm_fluid, m.n_c);
            }

            // record the diagonal additions for metric-consistent regularization
            for i in 0..nv {
                let diag = (m.m_rowadr[i] + m.m_rownnz[i] - 1) as usize;
                d.efm_sdiag[i] = d.q_h[diag] - d.m[diag];
            }
        }

        // flex stiffness CSR diagonal on standalone 3-dof slider trees joins the qH backbone
        // factor, added after recording efm_sdiag
        if d.nefm_k != 0 {
            let mut i = 0;
            while i < nv {
                if eff_simple_triple(m, i) && m.tree_dofnum[m.dof_treeid[i] as usize] == 3 {
                    for r in 0..3 {
                        let row = i + r;
                        let adr = d.efm_k_rowadr[row] as usize;
                        let nnz = d.efm_k_rownnz[row] as usize;
                        for j in adr..adr + nnz {
                            if d.efm_k_colind[j] == row as i32 {
                                d.q_h[(m.m_rowadr[row] + m.m_rownnz[row] - 1) as usize] += d.efm_k_val[j];
                                break;
                            }
                        }
                    }
                    i += 3;
                } else {
                    i += 1;
                }
            }
        }

        if flg_factor != 0 {
            let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nv_awake as usize) < nv;
            let n = if sleep_filter { d.nv_awake as usize } else { nv };
            let index = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
            factor_i(&mut d.q_h, Some(&mut d.q_hdiag_inv), n, &m.m_rownnz, &m.m_rowadr, &m.m_colind, index);
        }
    }
}

// does flex f require the matrix-free stiffness path (not covered by the CSR)?
fn flex_matrix_free(m: &Model, d: &Data, f: usize, interp_asm: bool, gen_csr: bool) -> bool {
    if gen_csr {
        return false;
    }
    if m.flex_interp[f] != 0 {
        return !interp_asm && d_flex_interp_processed(m, f);
    }
    d_flex_stiff_active_gen(m, f, true, true, true) && (d.nefm_k == 0 || flex_simple(m, f) == 0)
}

pub fn eff_mul_add_island(m: &Model, d: &Data, res: &mut [f64], vec: &[f64], island: usize) {
    let nv = d.island_nv[island] as usize;
    let idofadr = d.island_idofadr[island];
    let idof2dof = &d.map_idof2dof[idofadr as usize..];

    if !d.efm_diag.is_empty() {
        for k in 0..nv {
            res[k] += d.efm_diag[idof2dof[k] as usize] * vec[k];
        }
    }

    // fluid drag blocks: M-row columns are ancestor dofs, guaranteed in the same island
    if !d.efm_fluid.is_empty() {
        for k in 0..nv {
            let i = idof2dof[k] as usize;
            let start = m.m_rowadr[i] as usize;
            let diag = start + m.m_rownnz[i] as usize - 1;
            let mut acc = d.efm_fluid[diag] * vec[k];
            for a in start..diag {
                let kc = (d.map_dof2idof[m.m_colind[a] as usize] - idofadr) as usize;
                let f = d.efm_fluid[a];
                acc += f * vec[kc];
                res[kc] += f * vec[k];
            }
            res[k] += acc;
        }
    }

    // the island's rank-1 terms: every tree an entry touches shares the island, so the first
    // column decides membership
    let mut it = EffRank1Iter::default();
    let mut e = EffRank1::default();
    while eff_rank1_next(m, d, &mut it, &mut e, 1) != 0 {
        if d.tree_island[m.dof_treeid[e.colind[0] as usize] as usize] != island as i32 {
            continue;
        }
        let mut dot = 0.0;
        for j in 0..e.nnz {
            dot += e.val[j] * vec[(d.map_dof2idof[e.colind[j] as usize] - idofadr) as usize];
        }
        dot *= e.scale;
        for j in 0..e.nnz {
            res[(d.map_dof2idof[e.colind[j] as usize] - idofadr) as usize] += dot * e.val[j];
        }
    }

    let mut isl = EffIsland::default();
    let has_isl = eff_island_step(d, &mut isl);
    let gen_csr = efm_general_csr(d);

    // flex stiffness CSR restricted to the island's covered triples
    if d.nefm_k != 0 && d.nefmdof != 0 && has_isl && !gen_csr {
        unported!("mj_effMulAddIsland (covered flex triples)");
    } else if d.nefm_k != 0 {
        for k in 0..nv {
            let i = idof2dof[k] as usize;
            let nnz = d.efm_k_rownnz[i] as usize;
            if nnz == 0 {
                continue;
            }
            let adr = d.efm_k_rowadr[i] as usize;
            let ci = &d.efm_k_colind[adr..];
            let v = &d.efm_k_val[adr..];
            let mut acc = 0.0;
            for j in 0..nnz {
                let lc = d.map_dof2idof[ci[j] as usize] - idofadr;
                if lc < 0 || lc >= nv as i32 {
                    error(&format!("mj_effMulAddIsland: flex stiffness column outside island {island}"));
                }
                acc += v[j] * vec[lc as usize];
            }
            res[k] += acc;
        }
    }

    // matrix-free fallback for flexes in this island not covered by the CSR
    let has_mf = if !gen_csr && has_isl { isl.flex_adr[island] < isl.flex_adr[island + 1] } else { false };
    let interp_asm = d.nefm_k != 0 && (gen_csr || d_flex_interp_assemblable(m));
    if !gen_csr && !has_isl {
        for f in 0..m.nflex {
            if flex_matrix_free(m, d, f, interp_asm, gen_csr) {
                unported!("flexIsland");
            }
        }
    }
    if has_mf {
        unported!("mj_effMulAddIsland (matrix-free flex operators)");
    }
}

pub fn eff_prec_island(
    m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>, island: usize,
    l: Option<&[f64]>, q_ld: &[f64], q_ldiag_inv: &[f64], m_rownnz: &[i32],
    m_rowadr: &[i32], m_colind: &[i32],
) {
    let nv = d.island_nv[island] as usize;
    let nbd = m.nefm0dof;
    let flg_bend = nbd != 0 && d.nefmdof == 0;
    let mut isl = EffIsland::default();
    let has_isl = eff_island_step(d, &mut isl);
    let has_cov = has_isl && isl.cov_adr[island] < isl.cov_adr[island + 1];
    if !has_isl && d.nefmdof != 0 && (l.is_some() || eff_chol_reserved(d)) {
        unported!("mj_effPrecIsland (active metric)");
    }
    let use_chol = l.is_none() && has_cov && unported!("mj_effPrecIsland (active metric)");
    let use_fold = l.is_some() && has_cov;
    let has_bend = flg_bend && has_isl && isl.bend_adr[island] < isl.bend_adr[island + 1];
    let has_twist = false;
    if !d.efm_sdiag.is_empty() {
        for f in 0..m.nflex {
            if m.flex_rigid[f] == 0 && unported!("mj_rodHasTwist") {
                unported!("mj_effPrecIsland (active metric)");
            }
        }
    }
    if !use_chol && !use_fold && !has_bend && !has_twist {
        if let Some(b) = b { copy(x, b, nv); }
        solve_ld(x, q_ld, q_ldiag_inv, nv, 1, m_rownnz, m_rowadr, m_colind, None);
        return;
    }
    unported!("mj_effPrecIsland (active metric)");
}

// passive flex contact rows: published only for flexes on the passive contact path
fn eff_contact_build(m: &Model, d: &mut Data, _scale: f64) {
    d.nefmcon = 0;
    if d.ncon == 0 || !d_flex_passive_contact_any(m) {
        return;
    }
    unported!("effContactBuild (passive flex contact)");
}

// apply the published rows' forces, res += force * row: the passive stage takes its contact
// force from the rows the metric build published
pub fn eff_contact_force(d: &Data, res: &mut [f64]) {
    let ind = &d.efm_con_ind;
    let val = &d.efm_con_val;
    let mut adr = 0;
    while adr < d.nefmcon as usize {
        let nnz = ind[adr] as usize;
        let f = val[adr + 1];
        let colind = &ind[adr + 2..];
        let row = &val[adr + 2..];
        for a in 0..nnz {
            res[colind[a] as usize] += f * row[a];
        }
        adr += 2 + nnz;
    }
}

// build the per-step implicit effective metric, or deactivate it. The gate decision
// (integrator=discrete) is the caller's
pub fn eff_build(m: &Model, d: &mut Data, active: i32, _flg_factor: i32) {
    let nv = m.nv;
    d.efm_active = 0;
    d.nefm_k = 0;
    d.nefmcon = 0;
    d.nefm_t = 0;
    d.nefmdof = 0;
    d.nefm_l = 0;
    d.nefm_a = 0;
    d.efm_diag.clear();
    d.efm_ck.clear();
    d.efm_sdiag.clear();
    d.efm_fluid.clear();
    d.efm_tid.clear();
    d.efm_ts.clear();
    d.efm_tk.clear();
    d.efm_aid.clear();
    d.efm_as.clear();
    d.efm_ak.clear();
    d.efm_ca.clear();
    d.efm_k_colind.clear();
    d.efm_k_val.clear();
    d.efm_dofid.clear();
    d.efm_con_ind.clear();
    d.efm_con_val.clear();
    d.efm_l.clear();
    if active == 0 {
        return;
    }
    let h = m.opt.timestep;

    // corotated element stiffness cache (used by the shift and the matrix-free fallback)
    zero(&mut d.flexelem_krot, m.nflexstiffness);
    let mut krot = std::mem::take(&mut d.flexelem_krot);
    d_flex_interp_cache_krot(m, d, &mut krot);
    d.flexelem_krot = krot;

    // smooth-force shift c = h*K*qvel (values refreshed by mj_effShift in the velocity stage)
    eff_alloc(&mut d.efm_c, nv);

    // per-dof diagonal classes: position-dependent stiffness assembled here; the
    // velocity-dependent damping part and the qH backbone factor are refreshed later
    eff_alloc(&mut d.efm_ck, nv);
    let mut any_diag = eff_diag_stiff(m, d);
    let mut i = 0;
    while !any_diag && i < nv {
        any_diag = dof_damp_possible(m, i);
        i += 1;
    }

    // tendons with metric terms: id list here, values (velocity-dependent) in mj_effShift.
    // Sleeping tendons are excluded, matching the passive-force filter
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
    if m.ntendon != 0 && eff_couplings(m) != 0 {
        eff_alloc(&mut d.efm_tid, m.ntendon);
        eff_alloc(&mut d.efm_ts, m.ntendon);
        eff_alloc(&mut d.efm_tk, m.ntendon);
        for i in 0..m.ntendon {
            if eff_tendon_possible(m, i) == 0 {
                continue;
            }
            if sleep_filter && sleep_state(m, d, OBJ_TENDON, i as i32) != S_AWAKE {
                continue;
            }
            let t = d.nefm_t as usize;
            d.efm_ts[t] = 0.0;
            d.efm_tid[t] = i as i32;
            d.nefm_t += 1;
        }
    }

    // metric-possible actuators: allocation here, values in mj_effActuation
    let mut any_act = false;
    if eff_couplings(m) != 0 {
        for i in 0..m.nactuator {
            if eff_actuator_possible(m, i) != 0 {
                any_act = true;
                break;
            }
        }
    }
    if any_act {
        eff_alloc(&mut d.efm_aid, m.nactuator);
        eff_alloc(&mut d.efm_as, m.nactuator);
        eff_alloc(&mut d.efm_ak, m.nactuator);
        eff_alloc(&mut d.efm_ca, nv);
        zero(&mut d.efm_ca, nv);
    }

    // the tendon and actuator diagonals join the qH backbone: force the diagonal machinery on.
    // Fluid drag and passive flex contact enter only while their forces are applied
    let passive = !(m.opt.disableflags & DSBL_SPRING != 0
        && m.opt.disableflags & DSBL_DAMPER != 0);
    let any_fluid = passive && (m.opt.viscosity > 0.0 || m.opt.density > 0.0);
    let any_twist = false;
    if passive && eff_couplings(m) != 0 {
        for f in 0..m.nflex {
            if m.flex_rigid[f] == 0 {
                unported!("mj_rodHasTwist");
            }
        }
    }
    if any_diag || d.nefm_t != 0 || any_act || any_fluid || any_twist {
        eff_alloc(&mut d.efm_diag, nv);
    }
    if !d.efm_diag.is_empty() {
        eff_alloc(&mut d.efm_sdiag, nv);
    }
    if any_fluid {
        eff_alloc(&mut d.efm_fluid, m.n_c);
    }

    // Newton needs every flex in its explicit Hessian: once any active flex is not simple, the
    // whole stiffness goes through the general (Jacobian pull-back) assembly
    let gen_csr = m.opt.solver == SOL_NEWTON && !efm_all_simple(m);
    d.efm_active = if gen_csr { EFM_ACTIVE_GENERAL } else { EFM_ACTIVE };
    let krot = gen_csr || d_flex_interp_assemblable(m);
    eff_alloc(&mut d.efm_k_rownnz, nv);
    eff_alloc(&mut d.efm_k_rowadr, nv);

    // Newton consumes the metric as an explicit sparse matrix inside its Hessian: force full
    // CSR assembly, including bending-only flexes and general articulated flex attachments
    let mut assemble_any = d_flex_stiff_any(m, krot) || d_flex_passive_contact_any(m);
    if m.opt.solver == SOL_NEWTON {
        let mut f = 0;
        while !assemble_any && f < m.nflex {
            assemble_any = d_flex_stiff_active_gen(m, f, true, true, gen_csr)
                || (gen_csr && d_flex_interp_processed(m, f));
            f += 1;
        }
    }
    if assemble_any {
        unported!("mjd_flexStiff_assemble");
    }

    // passive flex contact is a rank-1 class, not CSR entries (see effContactBuild)
    if passive {
        eff_contact_build(m, d, h * h);
    }
    if d.nefm_k != 0 {
        // the flex stiffness CSR values and, when flg_factor, the per-step factor (effBlocks)
        unported!("mjd_flexStiff_assemble");
    } else {
        zero_int(&mut d.efm_k_rownnz, nv);
        zero_int(&mut d.efm_k_rowadr, nv);
    }

    // per-island flex/triple lists for mj_effMulAddIsland and mj_effPrecIsland
    if d.nisland > 0 && d.nefc > 0 && m.nflex > 0
        && m.opt.enableflags & ENBL_IPC == 0 && !eff_chol_reserved(d)
    {
        unported!("effIslandBuild");
    }
}
