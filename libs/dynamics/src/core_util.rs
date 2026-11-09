//! Core model predicates, kinematic chains, Jacobians and spatial frame utilities.
//!
//! Translated from `src/engine/engine_core_util.c` and `engine_core_util.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::data::{Contact, Data};
use crate::inline::{at3, at3_mut, at4, at6, at9};
use crate::model::Model;
use crate::types::*;
use crate::{inline, util_blas, util_misc, util_sparse, util_spatial};

// Determine type of constraint Jacobian.
pub fn is_sparse(m: &Model) -> i32 {
    if m.opt.jacobian == JAC_SPARSE || (m.opt.jacobian == JAC_AUTO && m.nv >= 60) {
        1
    } else {
        0
    }
}

// Determine type of friction cone.
pub fn is_pyramidal(m: &Model) -> i32 {
    if m.opt.cone == CONE_PYRAMIDAL {
        1
    } else {
        0
    }
}

// Determine type of solver.
pub fn is_dual(m: &Model) -> i32 {
    if m.opt.solver == SOL_PGS || m.opt.noslip_iterations > 0 {
        1
    } else {
        0
    }
}

// Merge dof chains for two bodies.
pub fn merge_chain(
    m: &Model,
    chain: &mut [i32],
    b1: usize,
    b2: usize,
    flg_skipcommon: i32,
) -> usize {
    let mut nv = 0;
    let b1 = m.body_weldid[b1] as usize;
    let b2 = m.body_weldid[b2] as usize;
    if m.body_dofnum[b1] == 0 && m.body_dofnum[b2] == 0 {
        return 0;
    }
    let mut da1 = m.body_dofadr[b1] + m.body_dofnum[b1] - 1;
    let mut da2 = m.body_dofadr[b2] + m.body_dofnum[b2] - 1;
    while da1 >= 0 || da2 >= 0 {
        let da = if da1 > da2 { da1 } else { da2 };
        if flg_skipcommon != 0 && da1 == da && da2 == da {
            break;
        }
        chain[nv] = da;
        if da1 == da {
            da1 = m.dof_parentid[da1 as usize];
        }
        if da2 == da {
            da2 = m.dof_parentid[da2 as usize];
        }
        nv += 1;
    }
    for i in 0..nv / 2 {
        let tmp = chain[i];
        chain[i] = chain[nv - i - 1];
        chain[nv - i - 1] = tmp;
    }
    nv
}

// Merge dof chains for two different simple bodies.
pub fn merge_chain_simple(m: &Model, chain: &mut [i32], mut b1: usize, mut b2: usize) -> usize {
    if b1 > b2 {
        std::mem::swap(&mut b1, &mut b2);
    }
    let n1 = m.body_dofnum[b1] as usize;
    let n2 = m.body_dofnum[b2] as usize;
    if n1 == 0 && n2 == 0 {
        return 0;
    }
    for i in 0..n1 {
        chain[i] = m.body_dofadr[b1] + i as i32;
    }
    for i in 0..n2 {
        chain[n1 + i] = m.body_dofadr[b2] + i as i32;
    }
    n1 + n2
}

// Get body chain.
pub fn body_chain(m: &Model, body: usize, chain: &mut [i32]) -> usize {
    if m.body_simple[body] != 0 {
        let dofnum = m.body_dofnum[body] as usize;
        for i in 0..dofnum {
            chain[i] = m.body_dofadr[body] + i as i32;
        }
        dofnum
    } else {
        let body = m.body_weldid[body] as usize;
        if m.body_dofnum[body] == 0 {
            return 0;
        }
        let mut da = m.body_dofadr[body] + m.body_dofnum[body] - 1;
        let mut nv = 0;
        while da >= 0 {
            chain[nv] = da;
            nv += 1;
            da = m.dof_parentid[da as usize];
        }
        for i in 0..nv / 2 {
            let tmp = chain[i];
            chain[i] = chain[nv - i - 1];
            chain[nv - i - 1] = tmp;
        }
        nv
    }
}

// Compute 3/6-by-nv Jacobian of global point attached to given body.
pub fn jac(
    m: &Model,
    d: &Data,
    mut jacp: Option<&mut [f64]>,
    mut jacr: Option<&mut [f64]>,
    point: &[f64; 3],
    body: usize,
) {
    let nv = m.nv;
    let mut offset = [0.0; 3];
    if let Some(jacp) = jacp.as_deref_mut() {
        util_blas::zero(jacp, 3 * nv);
        util_blas::sub3(
            &mut offset,
            point,
            at3(&d.subtree_com, 3 * m.body_rootid[body] as usize),
        );
    }
    if let Some(jacr) = jacr.as_deref_mut() {
        util_blas::zero(jacr, 3 * nv);
    }
    let body = m.body_weldid[body] as usize;
    if m.body_dofnum[body] == 0 {
        return;
    }
    let mut i = m.body_dofadr[body] + m.body_dofnum[body] - 1;
    while i >= 0 {
        let cdof = at6(&d.cdof, 6 * i as usize);
        if let Some(jacr) = jacr.as_deref_mut() {
            jacr[i as usize] = cdof[0];
            jacr[i as usize + nv] = cdof[1];
            jacr[i as usize + 2 * nv] = cdof[2];
        }
        if let Some(jacp) = jacp.as_deref_mut() {
            let mut tmp = [0.0; 3];
            inline::cross(&mut tmp, at3(cdof, 0), &offset);
            jacp[i as usize] = cdof[3] + tmp[0];
            jacp[i as usize + nv] = cdof[4] + tmp[1];
            jacp[i as usize + 2 * nv] = cdof[5] + tmp[2];
        }
        i = m.dof_parentid[i as usize];
    }
}

pub fn jac_body(
    m: &Model,
    d: &Data,
    jacp: Option<&mut [f64]>,
    jacr: Option<&mut [f64]>,
    body: usize,
) {
    jac(m, d, jacp, jacr, at3(&d.xpos, 3 * body), body);
}

pub fn jac_body_com(
    m: &Model,
    d: &Data,
    jacp: Option<&mut [f64]>,
    jacr: Option<&mut [f64]>,
    body: usize,
) {
    jac(m, d, jacp, jacr, at3(&d.xipos, 3 * body), body);
}

pub fn jac_subtree_com(m: &Model, d: &mut Data, jacp: &mut [f64], body: usize) {
    let nv = m.nv;
    let mut jacp_b = d.scratch.num(3 * nv);
    util_blas::zero(jacp, 3 * nv);
    for b in body..m.nbody {
        if b > body && m.body_parentid[b] < body as i32 {
            break;
        }
        jac(m, d, Some(&mut jacp_b), None, at3(&d.xipos, 3 * b), b);
        util_blas::add_to_scl(jacp, &jacp_b, m.body_mass[b], 3 * nv);
    }
    let scl = 1.0 / m.body_subtreemass[body];
    // In-place mju_scl: each input element is read before its output is written.
    for i in 0..3 * nv {
        jacp[i] = jacp[i] * scl;
    }
    d.scratch.put_num(jacp_b);
}

pub fn jac_geom(
    m: &Model,
    d: &Data,
    jacp: Option<&mut [f64]>,
    jacr: Option<&mut [f64]>,
    geom: usize,
) {
    jac(
        m,
        d,
        jacp,
        jacr,
        at3(&d.geom_xpos, 3 * geom),
        m.geom_bodyid[geom] as usize,
    );
}

pub fn jac_site(
    m: &Model,
    d: &Data,
    jacp: Option<&mut [f64]>,
    jacr: Option<&mut [f64]>,
    site: usize,
) {
    jac(
        m,
        d,
        jacp,
        jacr,
        at3(&d.site_xpos, 3 * site),
        m.site_bodyid[site] as usize,
    );
}

pub fn jac_point_axis(
    m: &Model,
    d: &mut Data,
    jac_point: Option<&mut [f64]>,
    jac_axis: Option<&mut [f64]>,
    point: &[f64; 3],
    axis: &[f64; 3],
    body: usize,
) {
    let nv = m.nv;
    let mut scratch_jacp = if jac_point.is_none() {
        Some(d.scratch.num(3 * nv))
    } else {
        None
    };
    let jacp = jac_point.unwrap_or_else(|| scratch_jacp.as_deref_mut().unwrap());
    let mut jacr = d.scratch.num(3 * nv);
    jac(m, d, Some(jacp), Some(&mut jacr), point, body);
    if let Some(jac_axis) = jac_axis {
        for i in 0..nv {
            jac_axis[i] = jacr[nv + i] * axis[2] - jacr[2 * nv + i] * axis[1];
            jac_axis[nv + i] = jacr[2 * nv + i] * axis[0] - jacr[i] * axis[2];
            jac_axis[2 * nv + i] = jacr[i] * axis[1] - jacr[nv + i] * axis[0];
        }
    }
    if let Some(jacp) = scratch_jacp {
        d.scratch.put_num(jacp);
    }
    d.scratch.put_num(jacr);
}

pub fn jac_sparse(
    m: &Model,
    d: &Data,
    mut jacp: Option<&mut [f64]>,
    mut jacr: Option<&mut [f64]>,
    point: &[f64; 3],
    body: usize,
    nv: usize,
    chain: &[i32],
    flg_skipcommon: i32,
) {
    if let Some(jacp) = jacp.as_deref_mut() {
        util_blas::zero(jacp, 3 * nv);
    }
    if let Some(jacr) = jacr.as_deref_mut() {
        util_blas::zero(jacr, 3 * nv);
    }
    let mut offset = [0.0; 3];
    util_blas::sub3(
        &mut offset,
        point,
        at3(&d.subtree_com, 3 * m.body_rootid[body] as usize),
    );
    let body = m.body_weldid[body] as usize;
    if m.body_dofnum[body] == 0 {
        return;
    }
    let mut da = m.body_dofadr[body] + m.body_dofnum[body] - 1;
    let mut ci = nv as i32 - 1;
    while da >= 0 {
        while ci >= 0 && chain[ci as usize] > da {
            ci -= 1;
        }
        if ci < 0 || chain[ci as usize] != da {
            if flg_skipcommon != 0 {
                da = m.dof_parentid[da as usize];
                continue;
            }
            panic!("dof index {} not found in chain", da);
        }
        let cdof = at6(&d.cdof, 6 * da as usize);
        if let Some(jacr) = jacr.as_deref_mut() {
            jacr[ci as usize] = cdof[0];
            jacr[ci as usize + nv] = cdof[1];
            jacr[ci as usize + 2 * nv] = cdof[2];
        }
        if let Some(jacp) = jacp.as_deref_mut() {
            let mut tmp = [0.0; 3];
            inline::cross(&mut tmp, at3(cdof, 0), &offset);
            jacp[ci as usize] = cdof[3] + tmp[0];
            jacp[ci as usize + nv] = cdof[4] + tmp[1];
            jacp[ci as usize + 2 * nv] = cdof[5] + tmp[2];
        }
        da = m.dof_parentid[da as usize];
    }
}

pub fn jac_sparse_simple(
    m: &Model,
    d: &Data,
    mut jacdifp: Option<&mut [f64]>,
    mut jacdifr: Option<&mut [f64]>,
    point: &[f64; 3],
    body: usize,
    flg_second: i32,
    nv: usize,
    start: usize,
) {
    let mut offset = [0.0; 3];
    util_blas::sub3(
        &mut offset,
        point,
        at3(&d.subtree_com, 3 * m.body_rootid[body] as usize),
    );
    if m.body_dofnum[body] == 0 {
        return;
    }
    let mut ci = start;
    let end = m.body_dofadr[body] + m.body_dofnum[body];
    for da in m.body_dofadr[body]..end {
        let cdof = at6(&d.cdof, 6 * da as usize);
        if let Some(jacdifr) = jacdifr.as_deref_mut() {
            if flg_second != 0 {
                jacdifr[ci] = cdof[0];
                jacdifr[ci + nv] = cdof[1];
                jacdifr[ci + 2 * nv] = cdof[2];
            } else {
                jacdifr[ci] = -cdof[0];
                jacdifr[ci + nv] = -cdof[1];
                jacdifr[ci + 2 * nv] = -cdof[2];
            }
        }
        if let Some(jacdifp) = jacdifp.as_deref_mut() {
            let mut tmp = [0.0; 3];
            inline::cross(&mut tmp, at3(cdof, 0), &offset);
            if flg_second != 0 {
                jacdifp[ci] = cdof[3] + tmp[0];
                jacdifp[ci + nv] = cdof[4] + tmp[1];
                jacdifp[ci + 2 * nv] = cdof[5] + tmp[2];
            } else {
                jacdifp[ci] = -(cdof[3] + tmp[0]);
                jacdifp[ci + nv] = -(cdof[4] + tmp[1]);
                jacdifp[ci + 2 * nv] = -(cdof[5] + tmp[2]);
            }
        }
        ci += 1;
    }
}

pub fn jac_dif_pair(
    m: &Model,
    d: &Data,
    chain: &mut [i32],
    b1: usize,
    b2: usize,
    pos1: &[f64; 3],
    pos2: &[f64; 3],
    mut jac1p: Option<&mut [f64]>,
    mut jac2p: Option<&mut [f64]>,
    mut jacdifp: Option<&mut [f64]>,
    mut jac1r: Option<&mut [f64]>,
    mut jac2r: Option<&mut [f64]>,
    mut jacdifr: Option<&mut [f64]>,
    issparse: i32,
    flg_skipcommon: i32,
) -> usize {
    let issimple = b1 != b2 && m.body_simple[b1] != 0 && m.body_simple[b2] != 0;
    let mut nv = m.nv;
    if nv == 0 {
        return 0;
    }
    if issparse != 0 {
        nv = if issimple {
            merge_chain_simple(m, chain, b1, b2)
        } else {
            merge_chain(m, chain, b1, b2, flg_skipcommon)
        };
    }
    if nv == 0 {
        return 0;
    }
    if jacdifp.is_none() && jacdifr.is_none() && jac1p.is_none() && jac1r.is_none() {
        return nv;
    }
    if issparse != 0 {
        if issimple {
            jac_sparse_simple(
                m,
                d,
                jacdifp.as_deref_mut(),
                jacdifr.as_deref_mut(),
                pos1,
                b1,
                0,
                nv,
                if b1 < b2 {
                    0
                } else {
                    m.body_dofnum[b2] as usize
                },
            );
            jac_sparse_simple(
                m,
                d,
                jacdifp.as_deref_mut(),
                jacdifr.as_deref_mut(),
                pos2,
                b2,
                1,
                nv,
                if b2 < b1 {
                    0
                } else {
                    m.body_dofnum[b1] as usize
                },
            );
        } else {
            jac_sparse(
                m,
                d,
                jac1p.as_deref_mut(),
                jac1r.as_deref_mut(),
                pos1,
                b1,
                nv,
                chain,
                flg_skipcommon,
            );
            jac_sparse(
                m,
                d,
                jac2p.as_deref_mut(),
                jac2r.as_deref_mut(),
                pos2,
                b2,
                nv,
                chain,
                flg_skipcommon,
            );
            if let Some(jacdifp) = jacdifp {
                util_blas::sub(jacdifp, jac2p.unwrap(), jac1p.unwrap(), 3 * nv);
            }
            if let Some(jacdifr) = jacdifr {
                util_blas::sub(jacdifr, jac2r.unwrap(), jac1r.unwrap(), 3 * nv);
            }
        }
    } else {
        jac(m, d, jac1p.as_deref_mut(), jac1r.as_deref_mut(), pos1, b1);
        jac(m, d, jac2p.as_deref_mut(), jac2r.as_deref_mut(), pos2, b2);
        if let Some(jacdifp) = jacdifp {
            util_blas::sub(jacdifp, jac2p.unwrap(), jac1p.unwrap(), 3 * nv);
        }
        if let Some(jacdifr) = jacdifr {
            util_blas::sub(jacdifr, jac2r.unwrap(), jac1r.unwrap(), 3 * nv);
        }
    }
    nv
}

pub fn jac_sum(
    m: &Model,
    d: &mut Data,
    chain: &mut [i32],
    n: usize,
    body: &[i32],
    weight: &[f64],
    point: &[f64; 3],
    jacp: &mut [f64],
    mut jacr: Option<&mut [f64]>,
    flg_rot: i32,
) -> usize {
    let nv = m.nv;
    // C's NV is the packed chain width, distinct from the model's nv.
    let mut nv_chain;
    let mut jtmp = d.scratch.num(if flg_rot != 0 { 6 * nv } else { 3 * nv });
    if is_sparse(m) != 0 {
        let mut jac_mat = d.scratch.num(if flg_rot != 0 { 6 * nv } else { 3 * nv });
        let mut buf = d.scratch.num(if flg_rot != 0 { 6 * nv } else { 3 * nv });
        let mut buf_ind = d.scratch.int(nv);
        let mut bodychain = d.scratch.int(nv);
        nv_chain = body_chain(m, body[0] as usize, chain);
        if nv_chain != 0 {
            let (jp, jr) = jac_mat.split_at_mut(3 * nv_chain);
            let jr = if flg_rot != 0 { Some(jr) } else { None };
            if m.body_simple[body[0] as usize] != 0 {
                jac_sparse_simple(m, d, Some(jp), jr, point, body[0] as usize, 1, nv_chain, 0);
            } else {
                jac_sparse(
                    m,
                    d,
                    Some(jp),
                    jr,
                    point,
                    body[0] as usize,
                    nv_chain,
                    chain,
                    0,
                );
            }
            for k in 0..if flg_rot != 0 {
                6 * nv_chain
            } else {
                3 * nv_chain
            } {
                jac_mat[k] = jac_mat[k] * weight[0];
            }
        }
        for i in 1..n {
            let body_nv = body_chain(m, body[i] as usize, &mut bodychain);
            if body_nv == 0 {
                continue;
            }
            let (jp, jr) = jtmp.split_at_mut(3 * body_nv);
            let jr = if flg_rot != 0 { Some(jr) } else { None };
            if m.body_simple[body[i] as usize] != 0 {
                jac_sparse_simple(m, d, Some(jp), jr, point, body[i] as usize, 1, body_nv, 0);
            } else {
                jac_sparse(
                    m,
                    d,
                    Some(jp),
                    jr,
                    point,
                    body[i] as usize,
                    body_nv,
                    &bodychain,
                    0,
                );
            }
            nv_chain = util_sparse::add_to_sparse_mat(
                &mut jac_mat,
                &jtmp,
                nv,
                if flg_rot != 0 { 6 } else { 3 },
                weight[i],
                nv_chain,
                body_nv,
                chain,
                &bodychain,
                &mut buf,
                &mut buf_ind,
            );
        }
        util_blas::copy(jacp, &jac_mat, 3 * nv_chain);
        if flg_rot != 0 {
            util_blas::copy(jacr.unwrap(), &jac_mat[3 * nv_chain..], 3 * nv_chain);
        }
        d.scratch.put_num(jac_mat);
        d.scratch.put_num(buf);
        d.scratch.put_int(buf_ind);
        d.scratch.put_int(bodychain);
    } else {
        let (jp, jr) = jtmp.split_at_mut(3 * nv);
        jac(
            m,
            d,
            Some(jacp),
            if flg_rot != 0 {
                jacr.as_deref_mut()
            } else {
                None
            },
            point,
            body[0] as usize,
        );
        for k in 0..3 * nv {
            jacp[k] = jacp[k] * weight[0];
        }
        if flg_rot != 0 {
            let jacr = jacr.as_deref_mut().unwrap();
            for k in 0..3 * nv {
                jacr[k] = jacr[k] * weight[0];
            }
        }
        for i in 1..n {
            jac(
                m,
                d,
                Some(jp),
                if flg_rot != 0 { Some(&mut *jr) } else { None },
                point,
                body[i] as usize,
            );
            util_blas::add_to_scl(jacp, jp, weight[i], 3 * nv);
            if flg_rot != 0 {
                util_blas::add_to_scl(jacr.as_deref_mut().unwrap(), jr, weight[i], 3 * nv);
            }
        }
        nv_chain = nv;
    }
    d.scratch.put_num(jtmp);
    nv_chain
}

// Omitted simplexWeights through mj_elemVertWeight: flex interpolation and contact weights.

pub fn contact_body_weight(
    m: &Model,
    d: &Data,
    con: &Contact,
    side: usize,
    body: &mut [i32],
    mut weight: Option<&mut [f64]>,
) -> usize {
    if con.geom[side] >= 0 {
        body[0] = m.geom_bodyid[con.geom[side] as usize];
        if let Some(weight) = weight {
            weight[0] = 1.0;
        }
        return 1;
    }
    let _ = d; // Only the omitted mj_elemVertWeight call reads data.
    let f = con.flex[side] as usize;
    let mut vid = [0; 4];
    let mut vweight = [0.0; 4];
    let nw;
    if con.vert[side] >= 0 {
        vid[0] = m.flex_vertadr[f] + con.vert[side];
        vweight[0] = 1.0;
        nw = 1;
    } else {
        if weight.is_none() && m.flex_interp[f] == 0 {
            crate::unported!("mj_rodHasTwist");
        }
        crate::unported!("mj_elemVertWeight");
    }
    if m.flex_interp[f] != 0 {
        if m.flex_nodebodyid.is_empty() {
            return 0;
        }
        let mut coord = [0.0; 3];
        for i in 0..nw {
            util_blas::add_to_scl3(
                &mut coord,
                at3(&m.flex_vert0, 3 * vid[i] as usize),
                vweight[i],
            );
        }
        crate::unported!("mj_flexInterpWeights");
    }
    for k in 0..nw {
        body[k] = m.flex_vertbodyid[vid[k] as usize];
        if let Some(weight) = weight.as_deref_mut() {
            weight[k] = vweight[k];
        }
    }
    crate::unported!("mj_rodHasTwist");
}

pub fn jac_dot(
    m: &Model,
    d: &Data,
    mut jacp: Option<&mut [f64]>,
    mut jacr: Option<&mut [f64]>,
    point: &[f64; 3],
    body: usize,
) {
    let nv = m.nv;
    let mut offset = [0.0; 3];
    let mut pvel = [0.0; 6];
    if let Some(jacp) = jacp.as_deref_mut() {
        util_blas::zero(jacp, 3 * nv);
        let com = at3(&d.subtree_com, 3 * m.body_rootid[body] as usize);
        util_blas::sub3(&mut offset, point, com);
        util_spatial::transform_spatial(&mut pvel, at6(&d.cvel, 6 * body), 0, point, com, None);
    }
    if let Some(jacr) = jacr.as_deref_mut() {
        util_blas::zero(jacr, 3 * nv);
    }
    let body = m.body_weldid[body] as usize;
    if m.body_dofnum[body] == 0 {
        return;
    }
    let mut i = m.body_dofadr[body] + m.body_dofnum[body] - 1;
    while i >= 0 {
        let mut cdof_dot = *at6(&d.cdof_dot, 6 * i as usize);
        let cdof = at6(&d.cdof, 6 * i as usize);
        let r#type = m.jnt_type[m.dof_jntid[i as usize] as usize];
        let dofadr = m.jnt_dofadr[m.dof_jntid[i as usize] as usize];
        let is_quat = r#type == JNT_BALL || (r#type == JNT_FREE && i >= dofadr + 3);
        if is_quat {
            inline::cross_motion(
                &mut cdof_dot,
                at6(&d.cvel, 6 * m.dof_bodyid[i as usize] as usize),
                cdof,
            );
        }
        if let Some(jacr) = jacr.as_deref_mut() {
            jacr[i as usize] += cdof_dot[0];
            jacr[i as usize + nv] += cdof_dot[1];
            jacr[i as usize + 2 * nv] += cdof_dot[2];
        }
        if let Some(jacp) = jacp.as_deref_mut() {
            let mut tmp1 = [0.0; 3];
            inline::cross(&mut tmp1, at3(&cdof_dot, 0), &offset);
            let mut tmp2 = [0.0; 3];
            inline::cross(&mut tmp2, at3(cdof, 0), at3(&pvel, 3));
            jacp[i as usize] += cdof_dot[3] + tmp1[0] + tmp2[0];
            jacp[i as usize + nv] += cdof_dot[4] + tmp1[1] + tmp2[1];
            jacp[i as usize + 2 * nv] += cdof_dot[5] + tmp1[2] + tmp2[2];
        }
        i = m.dof_parentid[i as usize];
    }
}

pub fn jac_dot_sparse(
    m: &Model,
    d: &Data,
    mut jacp: Option<&mut [f64]>,
    mut jacr: Option<&mut [f64]>,
    point: &[f64; 3],
    body: usize,
    nv: usize,
    chain: &[i32],
) {
    let mut offset = [0.0; 3];
    let mut pvel = [0.0; 6];
    if let Some(jacp) = jacp.as_deref_mut() {
        util_blas::zero(jacp, 3 * nv);
        let com = at3(&d.subtree_com, 3 * m.body_rootid[body] as usize);
        util_blas::sub3(&mut offset, point, com);
        util_spatial::transform_spatial(&mut pvel, at6(&d.cvel, 6 * body), 0, point, com, None);
    }
    if let Some(jacr) = jacr.as_deref_mut() {
        util_blas::zero(jacr, 3 * nv);
    }
    let body = m.body_weldid[body] as usize;
    if m.body_dofnum[body] == 0 {
        return;
    }
    let mut da = m.body_dofadr[body] + m.body_dofnum[body] - 1;
    let mut ci = nv as i32 - 1;
    while da >= 0 {
        while ci >= 0 && chain[ci as usize] > da {
            ci -= 1;
        }
        if ci < 0 || chain[ci as usize] != da {
            panic!("dof index {} not found in chain", da);
        }
        let mut cdof_dot = *at6(&d.cdof_dot, 6 * da as usize);
        let cdof = at6(&d.cdof, 6 * da as usize);
        let r#type = m.jnt_type[m.dof_jntid[da as usize] as usize];
        let dofadr = m.jnt_dofadr[m.dof_jntid[da as usize] as usize];
        let is_quat = r#type == JNT_BALL || (r#type == JNT_FREE && da >= dofadr + 3);
        if is_quat {
            inline::cross_motion(
                &mut cdof_dot,
                at6(&d.cvel, 6 * m.dof_bodyid[da as usize] as usize),
                cdof,
            );
        }
        if let Some(jacr) = jacr.as_deref_mut() {
            jacr[ci as usize] += cdof_dot[0];
            jacr[ci as usize + nv] += cdof_dot[1];
            jacr[ci as usize + 2 * nv] += cdof_dot[2];
        }
        if let Some(jacp) = jacp.as_deref_mut() {
            let mut tmp1 = [0.0; 3];
            inline::cross(&mut tmp1, at3(&cdof_dot, 0), &offset);
            let mut tmp2 = [0.0; 3];
            inline::cross(&mut tmp2, at3(cdof, 0), at3(&pvel, 3));
            jacp[ci as usize] += cdof_dot[3] + tmp1[0] + tmp2[0];
            jacp[ci as usize + nv] += cdof_dot[4] + tmp1[1] + tmp2[1];
            jacp[ci as usize + 2 * nv] += cdof_dot[5] + tmp1[2] + tmp2[2];
        }
        da = m.dof_parentid[da as usize];
    }
}

pub fn angmom_mat(m: &Model, d: &mut Data, mat: &mut [f64], body: usize) {
    let nv = m.nv;
    let mut jacp = d.scratch.num(3 * nv);
    let mut jacr = d.scratch.num(3 * nv);
    let mut term1 = d.scratch.num(3 * nv);
    let mut term2 = d.scratch.num(3 * nv);
    util_blas::zero(mat, 3 * nv);
    let subtree_com = *at3(&d.subtree_com, 3 * body);
    for b in body..m.nbody {
        if b > body && m.body_parentid[b] < body as i32 {
            break;
        }
        jac_body_com(m, d, Some(&mut jacp), Some(&mut jacr), b);
        let ximat = *at9(&d.ximat, 9 * b);
        let mut inertia = [0.0; 9];
        inertia[0] = m.body_inertia[3 * b];
        inertia[4] = m.body_inertia[3 * b + 1];
        inertia[8] = m.body_inertia[3 * b + 2];
        let mut tmp1 = [0.0; 9];
        let mut tmp2 = [0.0; 9];
        inline::mul_mat_mat3(&mut tmp1, &ximat, &inertia);
        util_blas::mul_mat_mat_t3(&mut tmp2, &tmp1, &ximat);
        util_blas::mul_mat_mat(&mut term1, &tmp2, &jacr, 3, 3, nv);
        let mut com = [0.0; 3];
        inline::sub3(&mut com, at3(&d.xipos, 3 * b), &subtree_com);
        let mut com_mat = [0.0; 9];
        com_mat[1] = -com[2];
        com_mat[2] = com[1];
        com_mat[3] = com[2];
        com_mat[5] = -com[0];
        com_mat[6] = -com[1];
        com_mat[7] = com[0];
        util_blas::mul_mat_mat(&mut term2, &com_mat, &jacp, 3, 3, nv);
        for i in 0..3 * nv {
            term2[i] = term2[i] * m.body_mass[b];
        }
        util_blas::add_to(mat, &term1, 3 * nv);
        util_blas::add_to(mat, &term2, 3 * nv);
    }
    d.scratch.put_num(jacp);
    d.scratch.put_num(jacr);
    d.scratch.put_num(term1);
    d.scratch.put_num(term2);
}

pub fn object_velocity(
    m: &Model,
    d: &Data,
    objtype: i32,
    objid: usize,
    res: &mut [f64; 6],
    flg_local: i32,
) {
    let bodyid;
    let pos;
    let rot;
    if objtype == OBJ_BODY {
        bodyid = objid;
        pos = at3(&d.xipos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.ximat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_XBODY {
        bodyid = objid;
        pos = at3(&d.xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.xmat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_GEOM {
        bodyid = m.geom_bodyid[objid] as usize;
        pos = at3(&d.geom_xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.geom_xmat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_SITE {
        bodyid = m.site_bodyid[objid] as usize;
        pos = at3(&d.site_xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.site_xmat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_CAMERA {
        bodyid = m.cam_bodyid[objid] as usize;
        pos = at3(&d.cam_xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.cam_xmat, 9 * objid))
        } else {
            None
        };
    } else {
        panic!("invalid object type {}", objtype);
    }
    if m.body_dofnum[m.body_weldid[bodyid] as usize] == 0 {
        util_blas::zero(res, 6);
        return;
    }
    util_spatial::transform_spatial(
        res,
        at6(&d.cvel, 6 * bodyid),
        0,
        pos,
        at3(&d.subtree_com, 3 * m.body_rootid[bodyid] as usize),
        rot,
    );
}

pub fn geom_surface_velocity(
    m: &Model,
    d: &Data,
    geomid: usize,
    point: &[f64; 3],
    linear: &mut [f64; 3],
    angular: &mut [f64; 3],
) {
    let sv = &m.geom_surfacevel[6 * geomid..];
    inline::mul_mat_vec3(linear, at9(&d.geom_xmat, 9 * geomid), at3(sv, 0));
    inline::mul_mat_vec3(angular, at9(&d.geom_xmat, 9 * geomid), at3(sv, 3));
    let mut arm = [0.0; 3];
    let mut wxr = [0.0; 3];
    inline::sub3(&mut arm, point, at3(&d.geom_xpos, 3 * geomid));
    inline::cross(&mut wxr, angular, &arm);
    inline::add_to3(linear, &wxr);
}

pub fn object_acceleration(
    m: &Model,
    d: &Data,
    objtype: i32,
    objid: usize,
    res: &mut [f64; 6],
    flg_local: i32,
) {
    let bodyid;
    let pos;
    let rot;
    if objtype == OBJ_BODY {
        bodyid = objid;
        pos = at3(&d.xipos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.ximat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_XBODY {
        bodyid = objid;
        pos = at3(&d.xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.xmat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_GEOM {
        bodyid = m.geom_bodyid[objid] as usize;
        pos = at3(&d.geom_xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.geom_xmat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_SITE {
        bodyid = m.site_bodyid[objid] as usize;
        pos = at3(&d.site_xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.site_xmat, 9 * objid))
        } else {
            None
        };
    } else if objtype == OBJ_CAMERA {
        bodyid = m.cam_bodyid[objid] as usize;
        pos = at3(&d.cam_xpos, 3 * objid);
        rot = if flg_local != 0 {
            Some(at9(&d.cam_xmat, 9 * objid))
        } else {
            None
        };
    } else {
        panic!("invalid object type {}", objtype);
    }
    util_spatial::transform_spatial(
        res,
        at6(&d.cacc, 6 * bodyid),
        0,
        pos,
        at3(&d.subtree_com, 3 * m.body_rootid[bodyid] as usize),
        rot,
    );
    if m.body_dofnum[m.body_weldid[bodyid] as usize] == 0 {
        return;
    }
    let mut vel = [0.0; 6];
    util_spatial::transform_spatial(
        &mut vel,
        at6(&d.cvel, 6 * bodyid),
        0,
        pos,
        at3(&d.subtree_com, 3 * m.body_rootid[bodyid] as usize),
        rot,
    );
    let mut correction = [0.0; 3];
    inline::cross(&mut correction, at3(&vel, 0), at3(&vel, 3));
    inline::add_to3(at3_mut(res, 3), &correction);
}

pub fn local2global_quat(
    d: &mut Data,
    xpos: Option<&mut [f64; 3]>,
    mut xquat: Option<&mut [f64; 4]>,
    xmat: Option<&mut [f64; 9]>,
    pos: Option<&[f64; 3]>,
    quat: Option<&[f64; 4]>,
    body: usize,
    sameframe: u8,
) {
    let sf = sameframe as i32;
    if let (Some(xpos), Some(pos)) = (xpos, pos) {
        match sf {
            SAMEFRAME_NONE | SAMEFRAME_BODYROT | SAMEFRAME_INERTIAROT => {
                inline::mul_mat_vec3(xpos, at9(&d.xmat, 9 * body), pos);
                inline::add_to3(xpos, at3(&d.xpos, 3 * body));
            }
            SAMEFRAME_BODY => inline::copy3(xpos, at3(&d.xpos, 3 * body)),
            SAMEFRAME_INERTIA => inline::copy3(xpos, at3(&d.xipos, 3 * body)),
            _ => {}
        }
    }
    if xquat.is_some() || xmat.is_some() {
        if let Some(quat) = quat {
            let mut tmp = [0.0; 4];
            let q = xquat.as_deref_mut().unwrap_or(&mut tmp);
            match sf {
                SAMEFRAME_NONE => {
                    inline::mul_quat(q, at4(&d.xquat, 4 * body), quat);
                    if let Some(xmat) = xmat {
                        inline::quat2mat(xmat, q);
                    }
                }
                SAMEFRAME_BODY | SAMEFRAME_BODYROT => {
                    if let Some(xquat) = xquat {
                        inline::copy4(xquat, at4(&d.xquat, 4 * body));
                    }
                    if let Some(xmat) = xmat {
                        inline::copy9(xmat, at9(&d.xmat, 9 * body));
                    }
                }
                SAMEFRAME_INERTIA | SAMEFRAME_INERTIAROT => {
                    if let Some(xquat) = xquat {
                        inline::copy4(xquat, at4(&d.xiquat, 4 * body));
                    }
                    if let Some(xmat) = xmat {
                        inline::copy9(xmat, at9(&d.ximat, 9 * body));
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn local2global(
    d: &mut Data,
    xpos: Option<&mut [f64; 3]>,
    xmat: Option<&mut [f64; 9]>,
    pos: Option<&[f64; 3]>,
    quat: Option<&[f64; 4]>,
    body: usize,
    sameframe: u8,
) {
    local2global_quat(d, xpos, None, xmat, pos, quat, body, sameframe);
}

pub fn flex_body_simple(m: &Model, body: usize) -> i32 {
    let body = m.body_weldid[body] as usize;
    if m.body_dofnum[body] != 3
        || m.body_jntnum[body] != 3
        || m.body_dofnum[m.body_weldid[m.body_parentid[body] as usize] as usize] != 0
    {
        return 0;
    }
    let jadr = m.body_jntadr[body] as usize;
    for j in 0..3 {
        if m.jnt_type[jadr + j] != JNT_SLIDE {
            return 0;
        }
        for k in 0..3 {
            if (m.jnt_axis[3 * (jadr + j) + k] - if j == k { 1.0 } else { 0.0 }).abs() > MINVAL {
                return 0;
            }
        }
    }
    1
}

pub fn flex_simple(m: &Model, f: usize) -> i32 {
    if m.flex_interp[f] != 0 {
        return 0;
    }
    for v in m.flex_vertadr[f]..m.flex_vertadr[f] + m.flex_vertnum[f] {
        let body = m.body_weldid[m.flex_vertbodyid[v as usize] as usize] as usize;
        if m.body_dofnum[body] != 0 && flex_body_simple(m, body) == 0 {
            return 0;
        }
    }
    1
}

// Omitted flexPointMap, flexMap, mj_flexGather, mj_flexScatter and mju_flexGatherState: flex-only mappings.

pub fn contact_force(m: &Model, d: &Data, id: i32, result: &mut [f64; 6]) {
    util_blas::zero(result, 6);
    if id >= 0 && id < d.ncon && d.contact[id as usize].efc_address >= 0 {
        let con = &d.contact[id as usize];
        if is_pyramidal(m) != 0 {
            util_misc::decode_pyramid(
                result,
                &d.efc_force[con.efc_address as usize..],
                &con.friction,
                con.dim as usize,
            );
        } else {
            util_blas::copy(
                result,
                &d.efc_force[con.efc_address as usize..],
                con.dim as usize,
            );
        }
        result[0] -= con.adhesion;
    }
}

pub fn tendon_limit(m: &Model, ten_length: &[f64], i: usize) -> i32 {
    if m.tendon_limited[i] == 0 {
        return 0;
    }
    let mut nl = 0;
    let value = ten_length[i];
    let margin = m.tendon_margin[i];
    let mut side = -1;
    while side <= 1 {
        let dist = side as f64 * (m.tendon_range[2 * i + ((side + 1) / 2) as usize] - value);
        if dist < margin {
            nl += 1;
        }
        side += 2;
    }
    nl
}

pub fn tendon_spring_damper(
    m: &Model,
    d: &Data,
    i: usize,
    frc_spring: &mut f64,
    frc_damper: &mut f64,
) {
    *frc_spring = 0.0;
    *frc_damper = 0.0;
    if m.opt.disableflags & DSBL_SPRING == 0 {
        let stiffness = m.tendon_stiffness[i];
        let spoly = &m.tendon_stiffnesspoly[NPOLY * i..];
        if stiffness != 0.0 || util_misc::is_zero(spoly, NPOLY) == 0 {
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
            *frc_spring = -x * util_misc::poly_force(stiffness, spoly, x, NPOLY, 0);
        }
    }
    if m.opt.disableflags & DSBL_DAMPER == 0 {
        let mut dpoly = [0.0; NPOLY];
        util_blas::copy(&mut dpoly, &m.tendon_dampingpoly[NPOLY * i..], NPOLY);
        let damping = m.tendon_damping[i] + actuator_damping(m, OBJ_TENDON, i, &mut dpoly);
        if damping != 0.0 || util_misc::is_zero(&dpoly, NPOLY) == 0 {
            let v = d.ten_velocity[i];
            *frc_damper = -v * util_misc::poly_force(damping, &dpoly, v, NPOLY, 1);
        }
    }
}

pub fn actuator_damping(m: &Model, r#type: i32, id: usize, poly: &mut [f64; NPOLY]) -> f64 {
    if r#type != OBJ_TENDON && r#type != OBJ_JOINT {
        panic!("only joint and tendon objects can inherit damping from actuators");
    }
    let actuatorid = if r#type == OBJ_JOINT {
        m.jnt_actuatorid[id]
    } else {
        m.tendon_actuatorid[id]
    };
    if actuatorid == -1 {
        return 0.0;
    }
    let mut damping = 0.0;
    if actuatorid >= 0 {
        let actuatorid = actuatorid as usize;
        let gear2 = m.actuator_gear[6 * m.actuator_outadr[actuatorid] as usize]
            * m.actuator_gear[6 * m.actuator_outadr[actuatorid] as usize];
        damping = m.actuator_damping[actuatorid] * gear2;
        for k in 0..NPOLY {
            poly[k] += m.actuator_dampingpoly[NPOLY * actuatorid + k] * gear2;
        }
    } else {
        for k in 0..m.nactuator {
            if m.actuator_trnid[2 * k] != id as i32 {
                continue;
            }
            if r#type == OBJ_JOINT
                && m.actuator_trntype[k] != TRN_JOINT
                && m.actuator_trntype[k] != TRN_JOINTINPARENT
            {
                continue;
            }
            if r#type == OBJ_TENDON && m.actuator_trntype[k] != TRN_TENDON {
                continue;
            }
            let gear2 = m.actuator_gear[6 * m.actuator_outadr[k] as usize]
                * m.actuator_gear[6 * m.actuator_outadr[k] as usize];
            damping += m.actuator_damping[k] * gear2;
            for j in 0..NPOLY {
                poly[j] += m.actuator_dampingpoly[NPOLY * k + j] * gear2;
            }
        }
    }
    damping
}

pub fn actuator_armature(m: &Model, r#type: i32, id: usize) -> f64 {
    if r#type != OBJ_TENDON && r#type != OBJ_JOINT {
        panic!("only joint and tendon objects can inherit armature from actuators");
    }
    let actuatorid = if r#type == OBJ_JOINT {
        m.jnt_actuatorid[id]
    } else {
        m.tendon_actuatorid[id]
    };
    if actuatorid == -1 {
        return 0.0;
    }
    let mut armature = 0.0;
    if actuatorid >= 0 {
        let actuatorid = actuatorid as usize;
        let gear2 = m.actuator_gear[6 * m.actuator_outadr[actuatorid] as usize]
            * m.actuator_gear[6 * m.actuator_outadr[actuatorid] as usize];
        armature = m.actuator_armature[actuatorid] * gear2;
    } else {
        for k in 0..m.nactuator {
            if m.actuator_trnid[2 * k] != id as i32 {
                continue;
            }
            if r#type == OBJ_JOINT
                && m.actuator_trntype[k] != TRN_JOINT
                && m.actuator_trntype[k] != TRN_JOINTINPARENT
            {
                continue;
            }
            if r#type == OBJ_TENDON && m.actuator_trntype[k] != TRN_TENDON {
                continue;
            }
            let gear2 = m.actuator_gear[6 * m.actuator_outadr[k] as usize]
                * m.actuator_gear[6 * m.actuator_outadr[k] as usize];
            armature += m.actuator_armature[k] * gear2;
        }
    }
    armature
}

pub fn dcmotor_resistance(m: &Model, d: &Data, id: usize) -> f64 {
    let dynprm = &m.actuator_dynprm[NDYN * id..];
    let gainprm = &m.actuator_gainprm[NGAIN * id..];
    let mut r = gainprm[0];
    let slots = util_misc::dcmotor_slots(dynprm, gainprm);
    if slots.temperature >= 0 {
        let t = d.act[(m.actuator_actadr[id] + slots.temperature) as usize];
        let alpha = gainprm[2];
        let t0 = gainprm[3];
        let ta = dynprm[4];
        r *= 1.0 + alpha * (t + ta - t0);
    }
    util_misc::max(MINVAL, r)
}

// mj_warning is translated in util_errmem.rs.

pub fn is_metric(m: &Model) -> i32 {
    (m.opt.integrator == INT_DISCRETE) as i32
}

pub fn eff_use_qh(d: &Data) -> i32 {
    (d.efm_active != 0 && (!d.efm_diag.is_empty() || d.nefm_k > 0)) as i32
}

pub fn eff_couplings(m: &Model) -> i32 {
    (is_metric(m) != 0 && m.opt.solver != SOL_PGS) as i32
}

pub fn tendon_has_stiffness(m: &Model, i: usize) -> i32 {
    (m.tendon_stiffness[i] != 0.0
        || util_misc::is_zero(&m.tendon_stiffnesspoly[NPOLY * i..], NPOLY) == 0) as i32
}

pub fn tendon_has_damping(m: &Model, i: usize) -> i32 {
    (m.tendon_damping[i] != 0.0
        || util_misc::is_zero(&m.tendon_dampingpoly[NPOLY * i..], NPOLY) == 0
        || m.tendon_actuatorid[i] != -1) as i32
}

pub fn flex_has_stiffness(m: &Model, f: usize) -> i32 {
    let adr = m.flex_stiffnessadr[f];
    if adr < 0 {
        return 0;
    }
    let k = &m.flex_stiffness[adr as usize..];
    let order = m.flex_interp[f];
    if order == 0 {
        return (k[0] != 0.0) as i32;
    }
    if order < 0 {
        return 1;
    }
    let cx = m.flex_cellnum[3 * f];
    let cy = m.flex_cellnum[3 * f + 1];
    let cz = m.flex_cellnum[3 * f + 2];
    let npe = (order + 1) * (order + 1) * (order + 1);
    let nelem = cx * cy * cz;
    let stride = 9 * npe * npe;
    for e in 0..nelem {
        if k[e as usize * stride as usize] != 0.0 {
            return 1;
        }
    }
    0
}

pub fn eff_flex_contact_possible(m: &Model, f: usize) -> i32 {
    (m.flex_passive[f] != 0
        && m.flex_rigid[f] == 0
        && m.flex_dim[f] >= 2
        && m.opt.enableflags & ENBL_IPC == 0) as i32
}

pub fn eff_flex_stiff_possible(m: &Model, f: usize) -> i32 {
    if m.flex_rigid[f] != 0 {
        return 0;
    }
    if m.flex_dim[f] == 1 {
        return (m.flex_bendingadr[f] >= 0) as i32;
    }
    if m.flex_edgeequality[f] != 3 && flex_has_stiffness(m, f) != 0 {
        return 1;
    }
    let badr = m.flex_bendingadr[f];
    if badr < 0 {
        return 0;
    }
    let mut end = m.nflexbending;
    for g in f + 1..m.nflex {
        if m.flex_bendingadr[g] >= 0 {
            end = m.flex_bendingadr[g] as usize;
            break;
        }
    }
    (util_misc::is_zero(&m.flex_bending[badr as usize..], end - badr as usize) == 0) as i32
}

pub fn eff_flex_possible(m: &Model, f: usize) -> i32 {
    (eff_flex_stiff_possible(m, f) != 0 || eff_flex_contact_possible(m, f) != 0) as i32
}

pub fn eff_flex_any(m: &Model) -> i32 {
    for f in 0..m.nflex {
        if eff_flex_possible(m, f) != 0 {
            return 1;
        }
    }
    0
}

pub fn eff_tendon_possible(m: &Model, i: usize) -> i32 {
    ((m.opt.disableflags & DSBL_SPRING == 0 && tendon_has_stiffness(m, i) != 0)
        || (m.opt.disableflags & DSBL_DAMPER == 0 && tendon_has_damping(m, i) != 0)) as i32
}

pub fn eff_actuator_possible(m: &Model, i: usize) -> i32 {
    if m.opt.disableflags & DSBL_ACTUATION != 0 {
        return 0;
    }
    (m.actuator_biastype[i] == BIAS_AFFINE
        || m.actuator_biastype[i] == BIAS_SO3
        || m.actuator_biastype[i] == BIAS_DCMOTOR
        || m.actuator_biastype[i] == BIAS_MUSCLE
        || m.actuator_gaintype[i] == GAIN_AFFINE
        || m.actuator_gaintype[i] == GAIN_SO3
        || m.actuator_gaintype[i] == GAIN_MUSCLE
        || m.actuator_gaintype[i] == GAIN_DCMOTOR) as i32
}

// Omitted snhProject declarations, mj_flexHessian/mj_flexHessianMul and rod functions: flex/rod/SNH-only.

pub fn rod_state_size(m: &Model, f: usize) -> i32 {
    if m.flex_dim[f] != 1 || m.flex_plasticadr[f] < 0 || m.flex_bendingadr[f] < 0 {
        return 0;
    }
    let nhinge = m.flex_bending[m.flex_bendingadr[f] as usize] as i32;
    6 * (nhinge + 1) + nhinge
}

// Omitted remaining snh* functions, mj_snhCubic and header stretch inlines: flex/SNH-only.
