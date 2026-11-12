//! Inactive paths of the implicit effective metric.
//!
//! Translated from `src/engine/engine_metric.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2026 DeepMind Technologies Limited);
//! changed: translated to Rust; active metric paths remain untranslated.

use crate::core_smooth::solve_ld;
use crate::core_util::{eff_couplings, eff_flex_any, flex_simple};
use crate::data::Data;
use crate::derivative::{d_flex_interp_mul, d_flex_interp_mul_bytes};
use crate::memory::{stack_bytes, stack_frame_bytes};
use crate::model::Model;
use crate::types::{DSBL_DAMPER, DSBL_ISLAND, DSBL_SPRING, ENBL_IPC, ENBL_SLEEP, SOL_CG};

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

// Yield the next live rank-1 term. Preserve the transitions of an empty iterator.
pub fn eff_rank1_next(
    m: &Model,
    d: &Data,
    it: &mut EffRank1Iter,
    _e: &mut EffRank1<'_>,
    flg_contact: i32,
) -> i32 {
    while it.cls == 0 {
        if it.i >= d.nefm_t as usize {
            it.cls = 1;
            it.i = 0;
            break;
        }
        unported!("mj_effRank1Next (active metric)");
    }
    while it.cls == 1 {
        if it.i >= d.nefm_a as usize {
            it.cls = 2;
            it.i = 0;
            it.k = 0;
            break;
        }
        unported!("mj_effRank1Next (active metric)");
    }
    while it.cls == 2 {
        if flg_contact == 0 || it.k >= d.nefmcon as usize {
            it.cls = 3;
            it.i = 0;
            it.k = 0;
            break;
        }
        unported!("mj_effRank1Next (active metric)");
    }
    if it.cls == 3 {
        let passive = !(m.opt.disableflags & DSBL_SPRING != 0
            && m.opt.disableflags & DSBL_DAMPER != 0);
        if !passive || eff_couplings(m) == 0 {
            it.cls = 4;
            return 0;
        }
        let h_dt = m.opt.timestep;
        let _s1 = if m.opt.disableflags & DSBL_SPRING != 0 { 0.0 } else { h_dt * h_dt };
        let _s2 = if m.opt.disableflags & DSBL_DAMPER != 0 { 0.0 } else { h_dt };
        let _sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
        while it.i < m.nflex {
            let f = it.i;
            if it.k == 0 && (m.flex_rigid[f] != 0 || unported!("mj_rodHasTwist")) {
                it.i += 1;
                continue;
            }
            unported!("mj_effRank1Next (active metric)");
        }
        it.cls = 4;
    }
    0
}

const EFM_ACTIVE_GENERAL: i32 = 2;

fn efm_general_csr(d: &Data) -> bool {
    d.efm_active == EFM_ACTIVE_GENERAL
}

// res += B*vec; empty metric classes leave res unchanged.
pub fn eff_mul_add(m: &Model, d: &Data, res: &mut [f64], vec: &[f64], flg_contact: i32) {
    let h = m.opt.timestep;
    if !d.efm_diag.is_empty() {
        unported!("mj_effMulAdd (active metric)");
    }
    if !d.efm_fluid.is_empty() {
        unported!("mj_effMulAdd (active metric)");
    }
    let mut it = EffRank1Iter::default();
    let mut e = EffRank1::default();
    while eff_rank1_next(m, d, &mut it, &mut e, 0) != 0 {
        unported!("mj_effMulAdd (active metric)");
    }
    if flg_contact != 0 {
        let _ind = &d.efm_con_ind;
        let _val = &d.efm_con_val;
        let adr = 0;
        while adr < d.nefmcon {
            unported!("mj_effMulAdd (active metric)");
        }
    }
    let gen_csr = efm_general_csr(d);
    if d.nefm_k != 0 && d.nefmdof != 0 && !gen_csr {
        unported!("mj_effMulAdd (active metric)");
    } else if d.nefm_k != 0 {
        unported!("mj_effMulAdd (active metric)");
    }
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
        if d.nefm_k == 0 || unported!("mjd_flexInterpAssemblable") {
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

const EFF_ISLAND_TAG: f64 = -1.0;

fn eff_chol_reserved(d: &Data) -> bool {
    let nb = d.nefmdof;
    nb != 0 && d.nefm_l > 0 && d.efm_l[0] >= 0.0
}

// Only the lists read by the inactive-path tests are needed before unpacking.
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
    let _sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
    for f in 0..m.nflex {
        if m.flex_rigid[f] != 0 || unported!("mj_rodHasTwist") {
            continue;
        }
        unported!("mj_effFoldDiagSize (active metric)");
    }
    0
}

// None represents C's x == b, as in core_smooth::solve_m.
pub fn eff_solve(m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>) {
    if d.efm_active == 0 {
        eff_prec(m, d, x, b);
        return;
    }
    if d.nefm_t == 0 && d.nefm_a == 0 && eff_flex_any(m) == 0 {
        // The backbone-only active metric uses effBlockApply's qH factor.
        unported!("mj_effSolve (active metric)");
    }
    unported!("mj_effSolve (active metric)");
}

pub fn eff_prec(m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>) {
    let nv = m.nv;
    if d.efm_active != 0 {
        unported!("mj_effPrec (active metric)");
    }
    if let Some(b) = b {
        crate::util_blas::copy(x, b, nv);
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

pub fn eff_prec_blocks(m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>, _l: Option<&[f64]>) {
    if d.efm_active != 0 {
        unported!("mj_effPrecBlocks (active metric)");
    }
    eff_prec(m, d, x, b);
}

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
    if d.nefm_k == 0 || unported!("mjd_flexInterpAssemblable") {
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

pub fn eff_shift(_m: &Model, d: &mut Data) {
    if d.efm_active == 0 { return; }
    unported!("mj_effShift (active metric)");
}

pub fn eff_actuation(_m: &Model, d: &mut Data, _flg_factor: i32) {
    if d.efm_active == 0 { return; }
    unported!("mj_effActuation (active metric)");
}

fn flex_matrix_free(m: &Model, _d: &Data, f: usize, interp_asm: bool, gen_csr: bool) -> bool {
    if gen_csr { return false; }
    if m.flex_interp[f] != 0 {
        return !interp_asm && unported!("mjd_flexInterp_processed");
    }
    unported!("mjd_flexStiff_activeGen");
}

pub fn eff_mul_add_island(m: &Model, d: &Data, _res: &mut [f64], _vec: &[f64], island: usize) {
    let _nv = d.island_nv[island];
    let idofadr = d.island_idofadr[island] as usize;
    let _idof2dof = &d.map_idof2dof[idofadr..];
    if !d.efm_diag.is_empty() { unported!("mj_effMulAddIsland (active metric)"); }
    if !d.efm_fluid.is_empty() { unported!("mj_effMulAddIsland (active metric)"); }
    let mut it = EffRank1Iter::default();
    let mut e = EffRank1::default();
    while eff_rank1_next(m, d, &mut it, &mut e, 1) != 0 {
        unported!("mj_effMulAddIsland (active metric)");
    }
    let mut isl = EffIsland::default();
    let has_isl = eff_island_step(d, &mut isl);
    let gen_csr = efm_general_csr(d);
    if d.nefm_k != 0 && d.nefmdof != 0 && has_isl && !gen_csr {
        unported!("mj_effMulAddIsland (active metric)");
    } else if d.nefm_k != 0 {
        unported!("mj_effMulAddIsland (active metric)");
    }
    let has_mf = if !gen_csr && has_isl { isl.flex_adr[island] < isl.flex_adr[island + 1] } else { false };
    let interp_asm = d.nefm_k != 0 && (gen_csr || unported!("mjd_flexInterpAssemblable"));
    if !gen_csr && !has_isl {
        for f in 0..m.nflex {
            if flex_matrix_free(m, d, f, interp_asm, gen_csr) {
                unported!("mj_effMulAddIsland (active metric)");
            }
        }
    }
    if has_mf { unported!("mj_effMulAddIsland (active metric)"); }
}

pub fn eff_prec_island(
    m: &Model, d: &Data, x: &mut [f64], b: Option<&[f64]>, island: usize,
    l: Option<&[f64]>, q_ld: &[f64], q_ldiag_inv: &[f64], m_rownnz: &[i32],
    m_rowadr: &[i32], m_colind: &[i32],
) {
    let nv = d.island_nv[island] as usize;
    let idofadr = d.island_idofadr[island] as usize;
    let _idof2dof = &d.map_idof2dof[idofadr..];
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
        if let Some(b) = b { crate::util_blas::copy(x, b, nv); }
        solve_ld(x, q_ld, q_ldiag_inv, nv, 1, m_rownnz, m_rowadr, m_colind, None);
        return;
    }
    unported!("mj_effPrecIsland (active metric)");
}

pub fn eff_contact_force(d: &Data, _res: &mut [f64]) {
    let _ind = &d.efm_con_ind;
    let _val = &d.efm_con_val;
    let adr = 0;
    while adr < d.nefmcon { unported!("mj_effContactForce (active metric)"); }
}

pub fn eff_build(m: &Model, d: &mut Data, active: i32, _flg_factor: i32) {
    let _nv = m.nv;
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
    if active == 0 { return; }
    unported!("mj_effBuild (active metric)");
}
