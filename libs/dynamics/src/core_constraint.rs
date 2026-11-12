//! Constraint construction, Jacobians, impedance, references and projections.
//!
//! Translated from `src/engine/engine_core_constraint.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::inline::{at3, at3_mut, at4, at9};
use crate::{
    core_smooth, core_util,
    data::{Contact, Data},
    inline, memory,
    model::{disabled, enabled, Model},
    sleep,
    sparse_pattern::PatternRef,
    types::*,
    util_blas,
    util_misc::{max, min},
    util_sparse, util_spatial,
};

// C NV and nv collide after snake-case conversion; NV becomes nv_mat.

// Compute cell node Jacobians and combined chain for flex strain constraints.
fn cell_pos_and_jac(
    m: &Model,
    d: &mut Data,
    flex_id: usize,
    npc: usize,
    gindices: &[i32],
    nv: usize,
    xpos_c: &[f64],
    cell_chain: &mut [i32],
    cell_nnz: &mut usize,
) -> Vec<f64> {
    let nstart = m.flex_nodeadr[flex_id] as usize;
    let bodyid = &m.flex_nodebodyid[nstart..];
    *cell_nnz = 0;
    let mut dof_used = d.scratch.int(nv);
    let mut temp_chain = d.scratch.int(nv);
    dof_used[..nv].fill(0);
    for n in 0..npc {
        let temp_nnz =
            core_util::body_chain(m, bodyid[gindices[n] as usize] as usize, &mut temp_chain);
        for k in 0..temp_nnz {
            dof_used[temp_chain[k] as usize] = 1;
        }
    }
    for q in 0..nv {
        if dof_used[q] != 0 {
            cell_chain[*cell_nnz] = q as i32;
            *cell_nnz += 1;
        }
    }
    let mut cell_node_jac = d.scratch.num(3 * npc * *cell_nnz);
    util_blas::zero(&mut cell_node_jac, 3 * npc * *cell_nnz);
    let mut chain_col = d.scratch.int(nv);
    let mut blk_jac = d.scratch.num(3 * nv);
    for n in 0..npc {
        let body = bodyid[gindices[n] as usize] as usize;
        let chain_n = core_util::body_chain(m, body, &mut chain_col);
        util_blas::zero(&mut blk_jac, 3 * chain_n);
        core_util::jac_sparse(
            m,
            d,
            Some(&mut blk_jac),
            None,
            at3(xpos_c, 3 * n),
            body,
            chain_n,
            &chain_col,
            0,
        );
        for r in 0..3 {
            for k in 0..chain_n {
                for cc in 0..*cell_nnz {
                    if cell_chain[cc] == chain_col[k] {
                        cell_node_jac[(3 * n + r) * *cell_nnz + cc] = blk_jac[r * chain_n + k];
                        break;
                    }
                }
            }
        }
    }
    d.scratch.put_int(dof_used);
    d.scratch.put_int(temp_chain);
    d.scratch.put_int(chain_col);
    d.scratch.put_num(blk_jac);
    cell_node_jac
}

fn cell_strain_jacobian(
    npc: usize,
    cell_nnz: usize,
    d_sdx_local: &[f64],
    cell_node_jac: &[f64],
    strain_jac: &mut [f64],
) {
    util_blas::zero(strain_jac, cell_nnz);
    for n in 0..npc {
        for c in 0..3 {
            let w = d_sdx_local[3 * n + c];
            if w == 0.0 {
                continue;
            }
            let row = 3 * n + c;
            for k in 0..cell_nnz {
                strain_jac[k] += w * cell_node_jac[row * cell_nnz + k];
            }
        }
    }
}

fn arena_alloc_efc(m: &Model, d: &mut Data) -> i32 {
    d.parena = d.ncon as usize * std::mem::size_of::<Contact>();
    // Vec storage grows; the C arena-overflow failure cannot happen.
    let nefc = d.nefc as usize;
    let n_j = d.n_j as usize;
    d.efc_type.resize(nefc, 0);
    d.efc_id.resize(nefc, 0);
    d.efc_j_rownnz.resize(nefc, 0);
    d.efc_j_rowadr.resize(nefc, 0);
    d.efc_j_rowsuper.resize(nefc, 0);
    d.efc_j_colind.resize(n_j, 0);
    d.efc_j.resize(n_j, 0.0);
    d.efc_pos.resize(nefc, 0.0);
    d.efc_margin.resize(nefc, 0.0);
    d.efc_frictionloss.resize(nefc, 0.0);
    d.efc_diag_a.resize(nefc, 0.0);
    d.efc_kbip.resize(nefc * 4, 0.0);
    d.efc_d.resize(nefc, 0.0);
    d.efc_r.resize(nefc, 0.0);
    d.tendon_efcadr.resize(m.ntendon, 0);
    d.efc_vel.resize(nefc, 0.0);
    d.efc_aref.resize(nefc, 0.0);
    d.efc_b.resize(nefc, 0.0);
    d.efc_state.resize(nefc, 0);
    d.efc_force.resize(nefc, 0.0);
    1
}

// Compute contact condim, solref, solimp, friction, adhesion.
pub fn contact_param(
    m: &Model,
    condim: &mut i32,
    solref: &mut [f64; 2],
    solimp: &mut [f64; 5],
    friction: &mut [f64; 5],
    adhesion: &mut f64,
    g1: i32,
    g2: i32,
    f1: i32,
    f2: i32,
) {
    use crate::types::{MINVAL, NIMP, NREF};
    use crate::util_misc::{max, min};
    let mut fri = [0.0; 3];
    let priority1 = if f1 < 0 {
        m.geom_priority[g1 as usize]
    } else {
        m.flex_priority[f1 as usize]
    };
    let condim1 = if f1 < 0 {
        m.geom_condim[g1 as usize]
    } else {
        m.flex_condim[f1 as usize]
    };
    let solmix1 = if f1 < 0 {
        m.geom_solmix[g1 as usize]
    } else {
        m.flex_solmix[f1 as usize]
    };
    let solref1 = if f1 < 0 {
        &m.geom_solref[g1 as usize * NREF..]
    } else {
        &m.flex_solref[f1 as usize * NREF..]
    };
    let solimp1 = if f1 < 0 {
        &m.geom_solimp[g1 as usize * NIMP..]
    } else {
        &m.flex_solimp[f1 as usize * NIMP..]
    };
    let friction1 = if f1 < 0 {
        &m.geom_friction[g1 as usize * 3..]
    } else {
        &m.flex_friction[f1 as usize * 3..]
    };
    let adhesion1 = if f1 < 0 {
        m.geom_adhesion[g1 as usize]
    } else {
        0.0
    };
    let priority2 = if f2 < 0 {
        m.geom_priority[g2 as usize]
    } else {
        m.flex_priority[f2 as usize]
    };
    let condim2 = if f2 < 0 {
        m.geom_condim[g2 as usize]
    } else {
        m.flex_condim[f2 as usize]
    };
    let solmix2 = if f2 < 0 {
        m.geom_solmix[g2 as usize]
    } else {
        m.flex_solmix[f2 as usize]
    };
    let solref2 = if f2 < 0 {
        &m.geom_solref[g2 as usize * NREF..]
    } else {
        &m.flex_solref[f2 as usize * NREF..]
    };
    let solimp2 = if f2 < 0 {
        &m.geom_solimp[g2 as usize * NIMP..]
    } else {
        &m.flex_solimp[f2 as usize * NIMP..]
    };
    let friction2 = if f2 < 0 {
        &m.geom_friction[g2 as usize * 3..]
    } else {
        &m.flex_friction[f2 as usize * 3..]
    };
    let adhesion2 = if f2 < 0 {
        m.geom_adhesion[g2 as usize]
    } else {
        0.0
    };
    *adhesion = adhesion1 + adhesion2;
    if priority1 > priority2 {
        *condim = condim1;
        util_blas::copy(solref, solref1, NREF);
        util_blas::copy(solimp, solimp1, NIMP);
        util_blas::copy(&mut fri, friction1, 3);
        *adhesion = adhesion1;
    } else if priority1 < priority2 {
        *condim = condim2;
        util_blas::copy(solref, solref2, NREF);
        util_blas::copy(solimp, solimp2, NIMP);
        util_blas::copy(&mut fri, friction2, 3);
        *adhesion = adhesion2;
    } else {
        *condim = if condim1 > condim2 { condim1 } else { condim2 };
        let mix = if solmix1 >= MINVAL && solmix2 >= MINVAL {
            solmix1 / (solmix1 + solmix2)
        } else if solmix1 < MINVAL && solmix2 < MINVAL {
            0.5
        } else if solmix1 < MINVAL {
            0.0
        } else {
            1.0
        };
        if solref1[0] > 0.0 && solref2[0] > 0.0 {
            for i in 0..NREF {
                solref[i] = mix * solref1[i] + (1.0 - mix) * solref2[i];
            }
        } else {
            for i in 0..NREF {
                solref[i] = min(solref1[i], solref2[i]);
            }
        }
        for i in 0..NIMP {
            solimp[i] = mix * solimp1[i] + (1.0 - mix) * solimp2[i];
        }
        for i in 0..3 {
            fri[i] = max(friction1[i], friction2[i]);
        }
    }
    friction[0] = fri[0];
    friction[1] = fri[0];
    friction[2] = fri[1];
    friction[3] = fri[2];
    friction[4] = fri[2];
    if *condim > 6 || *condim < 1 {
        panic!("Invalid condim value: {}", *condim);
    }
}

pub fn assign_friction(m: &Model, target: &mut [f64; 5], source: &[f64; 5]) {
    if crate::model::enabled(m, crate::types::ENBL_OVERRIDE) {
        for i in 0..5 {
            target[i] = crate::util_misc::max(crate::types::MINMU, m.opt.o_friction[i]);
        }
    } else {
        for i in 0..5 {
            target[i] = crate::util_misc::max(crate::types::MINMU, source[i]);
        }
    }
}

pub fn assign_ref(m: &Model, target: &mut [f64; 2], source: &[f64; 2]) {
    if crate::model::enabled(m, crate::types::ENBL_OVERRIDE) {
        util_blas::copy(target, &m.opt.o_solref, crate::types::NREF);
    } else {
        util_blas::copy(target, source, crate::types::NREF);
    }
}

pub fn assign_imp(m: &Model, target: &mut [f64; 5], source: &[f64; 5]) {
    if crate::model::enabled(m, crate::types::ENBL_OVERRIDE) {
        util_blas::copy(target, &m.opt.o_solimp, crate::types::NIMP);
    } else {
        util_blas::copy(target, source, crate::types::NIMP);
    }
}

pub fn assign_margin(m: &Model, source: f64) -> f64 {
    if crate::model::enabled(m, crate::types::ENBL_OVERRIDE) {
        m.opt.o_margin
    } else {
        source
    }
}

pub fn add_contact(_m: &Model, d: &mut Data, con: &Contact) -> i32 {
    memory::clear_efc(d);
    // Vec storage grows; C's arena-full return is unnecessary.
    let ncon = d.ncon as usize;
    if d.contact.len() <= ncon {
        d.contact.resize(ncon + 1, Contact::default());
    }
    d.contact[ncon] = con.clone();
    d.ncon += 1;
    0
}

fn add_constraint(
    m: &Model,
    d: &mut Data,
    jac: &[f64],
    pos: Option<&[f64]>,
    margin: Option<&[f64]>,
    frictionloss: f64,
    size: usize,
    r#type: i32,
    id: i32,
    nv_mat: i32,
    chain: Option<&[i32]>,
) {
    let nv = m.nv;
    let nefc = d.nefc as usize;
    let mut empty = !matches!(
        r#type,
        CNSTR_CONTACT_FRICTIONLESS | CNSTR_CONTACT_PYRAMIDAL | CNSTR_CONTACT_ELLIPTIC
    );
    if core_util::is_sparse(m) == 0 {
        if empty {
            for i in 0..size * nv {
                if jac[i] != 0.0 {
                    empty = false;
                    break;
                }
            }
        }
        if !empty {
            util_blas::copy(&mut d.efc_j[nefc * nv..], jac, size * nv);
        }
    } else {
        let nv_mat = nv_mat.max(0) as usize;
        if nv_mat != 0 {
            empty = false;
        } else if empty {
            return;
        }
        if nv_mat != 0 && chain.is_none() {
            panic!("called with dense arguments");
        }
        for i in 0..size {
            d.efc_j_rowadr[nefc + i] = if nefc + i != 0 {
                d.efc_j_rowadr[nefc + i - 1] + d.efc_j_rownnz[nefc + i - 1]
            } else {
                0
            };
            d.efc_j_rownnz[nefc + i] = nv_mat as i32;
            if nv_mat != 0 {
                let adr = d.efc_j_rowadr[nefc + i] as usize;
                d.efc_j_colind[adr..adr + nv_mat].copy_from_slice(&chain.unwrap()[..nv_mat]);
                util_blas::copy(&mut d.efc_j[adr..], &jac[i * nv_mat..], nv_mat);
            }
        }
    }
    if empty {
        return;
    }
    for i in 0..size {
        d.efc_pos[nefc + i] = pos.map_or(0.0, |v| v[i]);
        d.efc_margin[nefc + i] = margin.map_or(0.0, |v| v[i]);
        d.efc_frictionloss[nefc + i] = frictionloss;
        d.efc_type[nefc + i] = r#type;
        d.efc_id[nefc + i] = id;
    }
    d.nefc += size as i32;
    if r#type == CNSTR_EQUALITY {
        d.ne += size as i32;
    } else if r#type == CNSTR_FRICTION_DOF || r#type == CNSTR_FRICTION_TENDON {
        d.nf += size as i32;
    } else if r#type == CNSTR_LIMIT_JOINT || r#type == CNSTR_LIMIT_TENDON {
        d.nl += size as i32;
    }
}

pub fn mul_jac_vec(m: &Model, d: &Data, res: &mut [f64], vec: &[f64]) {
    if d.nefc == 0 {
        return;
    }
    if core_util::is_sparse(m) != 0 {
        util_sparse::mul_mat_vec_sparse(
            res,
            &d.efc_j,
            vec,
            d.nefc as usize,
            &d.efc_j_rownnz,
            &d.efc_j_rowadr,
            &d.efc_j_colind,
            Some(&d.efc_j_rowsuper),
        );
    } else {
        util_blas::mul_mat_vec(res, &d.efc_j, vec, d.nefc as usize, m.nv);
    }
}

// Multiply JacobianT by vector.
pub fn mul_jac_tvec(m: &Model, d: &Data, res: &mut [f64], vec: &[f64]) {
    if d.nefc == 0 {
        return;
    }
    if core_util::is_sparse(m) != 0 {
        util_sparse::mul_mat_tvec_sparse(
            res,
            &d.efc_j,
            vec,
            d.nefc as usize,
            m.nv,
            &d.efc_j_rownnz,
            &d.efc_j_rowadr,
            &d.efc_j_colind,
        );
    } else {
        util_blas::mul_mat_tvec(res, &d.efc_j, vec, d.nefc as usize, m.nv);
    }
}

fn equality_anchors(
    m: &Model,
    d: &Data,
    eq_id: usize,
    pos1: &mut [f64; 3],
    pos2: &mut [f64; 3],
    body1: &mut i32,
    body2: &mut i32,
) {
    let r#type = m.eq_type[eq_id];
    let obj1 = m.eq_obj1id[eq_id] as usize;
    let obj2 = m.eq_obj2id[eq_id] as usize;
    if m.eq_objtype[eq_id] == OBJ_BODY {
        let data = &m.eq_data[NEQDATA * eq_id..];
        if r#type == EQ_CONNECT {
            util_blas::mul_mat_vec3(pos1, at9(&d.xmat, 9 * obj1), at3(data, 0));
            util_blas::add_to3(pos1, at3(&d.xpos, 3 * obj1));
            util_blas::mul_mat_vec3(pos2, at9(&d.xmat, 9 * obj2), at3(data, 3));
            util_blas::add_to3(pos2, at3(&d.xpos, 3 * obj2));
        } else {
            util_blas::mul_mat_vec3(pos1, at9(&d.xmat, 9 * obj1), at3(data, 3));
            util_blas::add_to3(pos1, at3(&d.xpos, 3 * obj1));
            util_blas::mul_mat_vec3(pos2, at9(&d.xmat, 9 * obj2), at3(data, 0));
            util_blas::add_to3(pos2, at3(&d.xpos, 3 * obj2));
        }
        *body1 = obj1 as i32;
        *body2 = obj2 as i32;
    } else {
        util_blas::copy3(pos1, at3(&d.site_xpos, 3 * obj1));
        util_blas::copy3(pos2, at3(&d.site_xpos, 3 * obj2));
        *body1 = m.site_bodyid[obj1];
        *body2 = m.site_bodyid[obj2];
    }
}

pub fn instantiate_equality(m: &Model, d: &mut Data) {
    let issparse = core_util::is_sparse(m);
    let nv = m.nv;
    if disabled(m, DSBL_EQUALITY) || m.nemax == 0 {
        return;
    }
    let sleep_filter = enabled(m, ENBL_SLEEP) && d.ntree_awake < m.ntree as i32;
    let mut jac0 = d.scratch.num(6 * nv);
    let mut jac1 = d.scratch.num(6 * nv);
    let mut jacdif = d.scratch.num(6 * nv);
    let mut chain = if issparse != 0 {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    let mut chain2 = if issparse != 0 {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    for i in 0..m.neq {
        if d.eq_active[i] == 0 {
            continue;
        }
        if sleep_filter && sleep::sleep_state(m, d, OBJ_EQUALITY, i as i32) == S_ASLEEP {
            continue;
        }
        let data = &m.eq_data[NEQDATA * i..];
        let id = [m.eq_obj1id[i], m.eq_obj2id[i]];
        let mut size = 0;
        let mut nv_mat = 0;
        let mut nv2 = 0;
        let mut cpos = [0.0; 6];
        match m.eq_type[i] {
            EQ_CONNECT => {
                let (mut pos0, mut pos1) = ([0.0; 3], [0.0; 3]);
                let (mut body0, mut body1) = (0, 0);
                equality_anchors(m, d, i, &mut pos0, &mut pos1, &mut body0, &mut body1);
                util_blas::sub3(at3_mut(&mut cpos, 0), &pos0, &pos1);
                nv_mat = core_util::jac_dif_pair(
                    m,
                    d,
                    &mut chain,
                    body1 as usize,
                    body0 as usize,
                    &pos1,
                    &pos0,
                    Some(&mut jac1),
                    Some(&mut jac0),
                    Some(&mut jacdif),
                    None,
                    None,
                    None,
                    issparse,
                    0,
                );
                util_blas::copy(&mut jac0, &jacdif, 3 * nv_mat);
                size = 3;
            }
            EQ_WELD => {
                let (mut pos0, mut pos1) = ([0.0; 3], [0.0; 3]);
                let (mut body0, mut body1) = (0, 0);
                equality_anchors(m, d, i, &mut pos0, &mut pos1, &mut body0, &mut body1);
                util_blas::sub3(at3_mut(&mut cpos, 0), &pos0, &pos1);
                let torquescale = data[10];
                let (j0p, j0r) = jac0.split_at_mut(3 * nv);
                let (j1p, j1r) = jac1.split_at_mut(3 * nv);
                let (jdp, jdr) = jacdif.split_at_mut(3 * nv);
                nv_mat = core_util::jac_dif_pair(
                    m,
                    d,
                    &mut chain,
                    body1 as usize,
                    body0 as usize,
                    &pos1,
                    &pos0,
                    Some(j1p),
                    Some(j0p),
                    Some(jdp),
                    Some(j1r),
                    Some(j0r),
                    Some(jdr),
                    issparse,
                    0,
                );
                util_blas::copy(&mut jac0, &jacdif, 3 * nv_mat);
                util_blas::copy(&mut jac0[3 * nv_mat..], &jacdif[3 * nv..], 3 * nv_mat);
                let (mut quat, mut quat1, mut quat2, mut quat3) =
                    ([0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]);
                if m.eq_objtype[i] == OBJ_BODY {
                    inline::mul_quat(&mut quat, at4(&d.xquat, 4 * id[0] as usize), at4(data, 6));
                    inline::neg_quat(&mut quat1, at4(&d.xquat, 4 * id[1] as usize));
                } else {
                    inline::copy4(&mut quat, at4(&d.site_xquat, 4 * id[0] as usize));
                    inline::neg_quat(&mut quat1, at4(&d.site_xquat, 4 * id[1] as usize));
                }
                inline::mul_quat(&mut quat2, &quat1, &quat);
                inline::scl3(at3_mut(&mut cpos, 3), at3(&quat2, 1), torquescale);
                for j in 0..nv_mat {
                    let axis = [
                        jac0[3 * nv_mat + j],
                        jac0[4 * nv_mat + j],
                        jac0[5 * nv_mat + j],
                    ];
                    inline::mul_quat_axis(&mut quat2, &quat1, &axis);
                    inline::mul_quat(&mut quat3, &quat2, &quat);
                    jac0[3 * nv_mat + j] = 0.5 * quat3[1];
                    jac0[4 * nv_mat + j] = 0.5 * quat3[2];
                    jac0[5 * nv_mat + j] = 0.5 * quat3[3];
                }
                for j in 0..3 * nv_mat {
                    jac0[3 * nv_mat + j] *= torquescale;
                }
                size = 6;
            }
            EQ_JOINT | EQ_TENDON => {
                let mut pos = [[0.0; 3]; 2];
                let mut r#ref = [0.0; 2];
                for j in 0..1 + usize::from(id[1] >= 0) {
                    let jac = if j == 0 { &mut jac0 } else { &mut jac1 };
                    let obj = id[j] as usize;
                    if m.eq_type[i] == EQ_JOINT {
                        pos[j][0] = d.qpos[m.jnt_qposadr[obj] as usize];
                        r#ref[j] = m.qpos0[m.jnt_qposadr[obj] as usize];
                        if issparse != 0 {
                            if j == 0 {
                                nv_mat = 1;
                                chain[0] = m.jnt_dofadr[obj];
                                jac[0] = 1.0;
                            } else {
                                nv2 = 1;
                                chain2[0] = m.jnt_dofadr[obj];
                                jac[0] = 1.0;
                            }
                        } else {
                            util_blas::zero(jac, nv);
                            jac[m.jnt_dofadr[obj] as usize] = 1.0;
                        }
                    } else {
                        pos[j][0] = d.ten_length[obj];
                        r#ref[j] = m.tendon_length0[obj];
                        if issparse != 0 {
                            let n = m.ten_j_rownnz[obj] as usize;
                            let adr = m.ten_j_rowadr[obj] as usize;
                            if j == 0 {
                                nv_mat = n;
                                chain[..n].copy_from_slice(&m.ten_j_colind[adr..adr + n]);
                            } else {
                                nv2 = n;
                                chain2[..n].copy_from_slice(&m.ten_j_colind[adr..adr + n]);
                            }
                            util_blas::copy(jac, &d.ten_j[adr..], n);
                        } else {
                            util_sparse::sparse2dense(
                                jac,
                                &d.ten_j,
                                1,
                                nv,
                                &m.ten_j_rownnz[obj..],
                                &m.ten_j_rowadr[obj..],
                                &m.ten_j_colind,
                            );
                        }
                    }
                }
                if id[1] >= 0 {
                    let dif = pos[1][0] - r#ref[1];
                    cpos[0] = pos[0][0]
                        - r#ref[0]
                        - data[0]
                        - (data[1] * dif
                            + data[2] * dif * dif
                            + data[3] * dif * dif * dif
                            + data[4] * dif * dif * dif * dif);
                    let deriv = data[1]
                        + 2.0 * data[2] * dif
                        + 3.0 * data[3] * dif * dif
                        + 4.0 * data[4] * dif * dif * dif;
                    if issparse != 0 {
                        nv_mat = util_sparse::combine_sparse(
                            &mut jac0, &jac1, 1.0, -deriv, nv_mat, nv2, &mut chain, &chain2,
                        );
                    } else {
                        util_blas::add_to_scl(&mut jac0, &jac1, -deriv, nv);
                    }
                } else {
                    cpos[0] = pos[0][0] - r#ref[0] - data[0];
                }
                size = 1;
            }
            EQ_FLEXSTRAIN => {
                let f = id[0] as usize;
                let nodenum = m.flex_nodenum[f];
                let interp = m.flex_interp[f];
                let order = if interp < 0 { -interp } else { interp };
                let shell_mode = interp < 0;
                if order == 0 || nodenum == 0 {
                    continue;
                }
                if order > 2 {
                    panic!(
                        "flex strain constraints only support order 1 and 2, got {}",
                        order
                    );
                }
                let cx = m.flex_cellnum[3 * f];
                let cy = m.flex_cellnum[3 * f + 1];
                let cz = m.flex_cellnum[3 * f + 2];
                let nstart = m.flex_nodeadr[f] as usize;
                let bodyid = &m.flex_nodebodyid[nstart..];
                let (npe, elem_idx) = if shell_mode {
                    (((order + 1) * (order + 1)) as usize, data[0] as i32)
                } else {
                    (
                        ((order + 1) * (order + 1) * (order + 1)) as usize,
                        data[0] as i32 * cy * cz + data[1] as i32 * cz + data[2] as i32,
                    )
                };
                // Typed closures keep the following C code type-checked after the unsupported call.
                let gindices: [i32; 125] = if shell_mode {
                    (|| -> [i32; 125] { crate::unported!("mju_flexGatherFaceState") })()
                } else {
                    (|| -> [i32; 125] { crate::unported!("mju_flexGatherCellState") })()
                };
                let mut xpos_e = d.scratch.num(3 * npe);
                let mut refpos_e = d.scratch.num(3 * npe);
                for n in 0..npe {
                    let gn = gindices[n] as usize;
                    if m.flex_centered[f] != 0
                        || (m.flex_node[3 * (gn + nstart)] == 0.0
                            && m.flex_node[3 * (gn + nstart) + 1] == 0.0
                            && m.flex_node[3 * (gn + nstart) + 2] == 0.0)
                    {
                        util_blas::copy3(
                            at3_mut(&mut xpos_e, 3 * n),
                            at3(&d.xpos, 3 * bodyid[gn] as usize),
                        );
                    } else {
                        util_blas::mul_mat_vec3(
                            at3_mut(&mut xpos_e, 3 * n),
                            at9(&d.xmat, 9 * bodyid[gn] as usize),
                            at3(&m.flex_node, 3 * (gn + nstart)),
                        );
                        util_blas::add_to3(
                            at3_mut(&mut xpos_e, 3 * n),
                            at3(&d.xpos, 3 * bodyid[gn] as usize),
                        );
                    }
                    util_blas::copy3(
                        at3_mut(&mut refpos_e, 3 * n),
                        at3(&m.flex_node0, 3 * (gn + nstart)),
                    );
                }
                let mut elem_quat = [1.0, 0.0, 0.0, 0.0];
                if shell_mode {
                    let face_sizes = [cy * cz, cy * cz, cx * cz, cx * cz, cx * cy, cx * cy];
                    let face_normals = [0, 0, 1, 1, 2, 2];
                    let mut cumul = 0;
                    let mut normal_axis = 0;
                    for ff in 0..6 {
                        if elem_idx < cumul + face_sizes[ff] {
                            normal_axis = face_normals[ff];
                            break;
                        }
                        cumul += face_sizes[ff];
                    }
                    let na0 = (normal_axis + 1) % 3;
                    let na1 = (normal_axis + 2) % 3;
                    let d0 = refpos_e[3 * (npe - 1) + na0] - refpos_e[na0];
                    let d1 = refpos_e[3 * (npe - 1) + na1] - refpos_e[na1];
                    let mut t1 = [0.0; 3];
                    let mut t2 = [0.0; 3];
                    let mut idx = 0;
                    for _l0 in 0..=order {
                        for _l1 in 0..=order {
                            let grad0 = (|| -> f64 { crate::unported!("mju_flexDphi") })()
                                * (|| -> f64 { crate::unported!("mju_flexPhi") })();
                            let grad1 = (|| -> f64 { crate::unported!("mju_flexPhi") })()
                                * (|| -> f64 { crate::unported!("mju_flexDphi") })();
                            for c in 0..3 {
                                t1[c] += xpos_e[3 * idx + c] * grad0;
                                t2[c] += xpos_e[3 * idx + c] * grad1;
                            }
                            idx += 1;
                        }
                    }
                    if d0 > MINVAL {
                        let input = t1;
                        util_blas::scl3(&mut t1, &input, 1.0 / d0);
                    }
                    if d1 > MINVAL {
                        let input = t2;
                        util_blas::scl3(&mut t2, &input, 1.0 / d1);
                    }
                    let mut normal = [0.0; 3];
                    util_spatial::cross(&mut normal, &t1, &t2);
                    let mut mat = [0.0; 9];
                    let mut vecs = [[0.0; 3]; 3];
                    vecs[na0] = t1;
                    vecs[na1] = t2;
                    vecs[normal_axis] = normal;
                    for col in 0..3 {
                        mat[col] = vecs[col][0];
                        mat[3 + col] = vecs[col][1];
                        mat[6 + col] = vecs[col][2];
                    }
                    util_blas::unit4(&mut elem_quat);
                    util_spatial::mat2rot(&mut elem_quat, &mat);
                    let input = elem_quat;
                    util_spatial::neg_quat(&mut elem_quat, &input);
                } else {
                    let mut mat: [f64; 9] =
                        (|| -> [f64; 9] { crate::unported!("mju_defGradient") })();
                    for col in 0..3 {
                        let dc = refpos_e[3 * (npe - 1) + col] - refpos_e[col];
                        if dc > MINVAL {
                            let inv = 1.0 / dc;
                            mat[col] *= inv;
                            mat[3 + col] *= inv;
                            mat[6 + col] *= inv;
                        }
                    }
                    util_blas::unit4(&mut elem_quat);
                    util_spatial::mat2rot(&mut elem_quat, &mat);
                    let input = elem_quat;
                    util_spatial::neg_quat(&mut elem_quat, &input);
                }
                let mut elem_chain = d.scratch.int(nv);
                let mut elem_nnz = 0;
                let elem_node_jac = cell_pos_and_jac(
                    m,
                    d,
                    f,
                    npe,
                    &gindices,
                    nv,
                    &xpos_e,
                    &mut elem_chain,
                    &mut elem_nnz,
                );
                let mut strain_jac = d.scratch.num(elem_nnz);
                let mut d_sdx_local = d.scratch.num(3 * npe);
                let mut dense_jac = if issparse == 0 {
                    let mut v = d.scratch.num(nv);
                    util_blas::zero(&mut v, nv);
                    v
                } else {
                    Vec::new()
                };
                let ndof_elem = 3 * npe;
                let stiffnessadr = m.flex_stiffnessadr[f];
                let mut neig = 0;
                let mut k_elem: &[f64] = &[];
                if stiffnessadr >= 0 {
                    k_elem = &m.flex_stiffness
                        [stiffnessadr as usize + elem_idx as usize * ndof_elem * ndof_elem..];
                    neig = k_elem[0] as usize;
                }
                let mut displ_e = d.scratch.num(ndof_elem);
                for n in 0..npe {
                    let mut xrot = [0.0; 3];
                    util_spatial::rot_vec_quat(&mut xrot, at3(&xpos_e, 3 * n), &elem_quat);
                    displ_e[3 * n] = xrot[0] - refpos_e[3 * n];
                    displ_e[3 * n + 1] = xrot[1] - refpos_e[3 * n + 1];
                    displ_e[3 * n + 2] = xrot[2] - refpos_e[3 * n + 2];
                }
                let mut elem_quat_inv = [0.0; 4];
                util_spatial::neg_quat(&mut elem_quat_inv, &elem_quat);
                for eig in 0..neig {
                    let eigvec = &k_elem[1 + eig * ndof_elem..];
                    let mut residual = 0.0;
                    for j in 0..ndof_elem {
                        residual += eigvec[j] * displ_e[j];
                    }
                    cpos[0] = residual;
                    for n in 0..npe {
                        util_spatial::rot_vec_quat(
                            at3_mut(&mut d_sdx_local, 3 * n),
                            at3(eigvec, 3 * n),
                            &elem_quat_inv,
                        );
                    }
                    cell_strain_jacobian(
                        npe,
                        elem_nnz,
                        &d_sdx_local,
                        &elem_node_jac,
                        &mut strain_jac,
                    );
                    if issparse != 0 {
                        add_constraint(
                            m,
                            d,
                            &strain_jac,
                            Some(&cpos),
                            None,
                            0.0,
                            1,
                            CNSTR_EQUALITY,
                            i as i32,
                            elem_nnz as i32,
                            Some(&elem_chain),
                        );
                    } else {
                        for k in 0..elem_nnz {
                            dense_jac[elem_chain[k] as usize] = strain_jac[k];
                        }
                        add_constraint(
                            m,
                            d,
                            &dense_jac,
                            Some(&cpos),
                            None,
                            0.0,
                            1,
                            CNSTR_EQUALITY,
                            i as i32,
                            0,
                            None,
                        );
                        for k in 0..elem_nnz {
                            dense_jac[elem_chain[k] as usize] = 0.0;
                        }
                    }
                }
                d.scratch.put_num(xpos_e);
                d.scratch.put_num(refpos_e);
                d.scratch.put_int(elem_chain);
                d.scratch.put_num(elem_node_jac);
                d.scratch.put_num(strain_jac);
                d.scratch.put_num(d_sdx_local);
                if issparse == 0 {
                    d.scratch.put_num(dense_jac);
                }
                d.scratch.put_num(displ_e);
            }
            EQ_FLEX => {
                let flex_edgeadr = m.flex_edgeadr[id[0] as usize] as usize;
                let flex_edgenum = m.flex_edgenum[id[0] as usize] as usize;
                for e in flex_edgeadr..flex_edgeadr + flex_edgenum {
                    if m.flexedge_rigid[e] != 0 {
                        continue;
                    }
                    cpos[0] = d.flexedge_length[e] - m.flexedge_length0[e];
                    let rowadr = m.flexedge_j_rowadr[e] as usize;
                    let rownnz = m.flexedge_j_rownnz[e] as usize;
                    if issparse != 0 {
                        // Copy out the input before mutating Data, as required for C aliasing.
                        util_blas::copy(&mut jac0, &d.flexedge_j[rowadr..], rownnz);
                        add_constraint(
                            m,
                            d,
                            &jac0,
                            Some(&cpos),
                            None,
                            0.0,
                            1,
                            CNSTR_EQUALITY,
                            i as i32,
                            rownnz as i32,
                            Some(&m.flexedge_j_colind[rowadr..]),
                        );
                    } else {
                        util_blas::zero(&mut jac0, nv);
                        for k in 0..rownnz {
                            jac0[m.flexedge_j_colind[rowadr + k] as usize] =
                                d.flexedge_j[rowadr + k];
                        }
                        add_constraint(
                            m,
                            d,
                            &jac0,
                            Some(&cpos),
                            None,
                            0.0,
                            1,
                            CNSTR_EQUALITY,
                            i as i32,
                            0,
                            None,
                        );
                    }
                }
            }
            _ => panic!("invalid equality constraint type {}", m.eq_type[i]),
        }
        if size != 0 {
            let efcadr = d.nefc;
            add_constraint(
                m,
                d,
                &jac0,
                Some(&cpos),
                None,
                0.0,
                size,
                CNSTR_EQUALITY,
                i as i32,
                if issparse != 0 { nv_mat as i32 } else { 0 },
                if issparse != 0 { Some(&chain) } else { None },
            );
            if m.eq_type[i] == EQ_TENDON && d.nefc > efcadr {
                if d.tendon_efcadr[id[0] as usize] == -1 {
                    d.tendon_efcadr[id[0] as usize] = efcadr;
                }
                if id[1] >= 0 && d.tendon_efcadr[id[1] as usize] == -1 {
                    d.tendon_efcadr[id[1] as usize] = efcadr;
                }
            }
        }
    }
    d.scratch.put_num(jac0);
    d.scratch.put_num(jac1);
    d.scratch.put_num(jacdif);
    if issparse != 0 {
        d.scratch.put_int(chain);
        d.scratch.put_int(chain2);
    }
}

// Subtract Jdot*v correction for equality constraints.
pub fn jdotv(m: &Model, d: &mut Data, result: &mut [f64]) {
    let nv = m.nv;
    let ne = d.ne as usize;
    if ne == 0 || nv == 0 {
        return;
    }
    let issparse = core_util::is_sparse(m) != 0;
    let mut chain = if issparse {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    let (mut jacdot1, mut jacdot2, mut jacrdot1, mut jacrdot2) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut row = 0;
    while row < ne {
        let eq_id = d.efc_id[row] as usize;
        let r#type = m.eq_type[eq_id];
        if r#type == EQ_CONNECT || r#type == EQ_WELD {
            let data = &m.eq_data[NEQDATA * eq_id..];
            if jacdot1.is_empty() {
                jacdot1 = d.scratch.num(3 * nv);
                jacdot2 = d.scratch.num(3 * nv);
            }
            if r#type == EQ_WELD && jacrdot1.is_empty() {
                jacrdot1 = d.scratch.num(3 * nv);
                jacrdot2 = d.scratch.num(3 * nv);
            }
            let obj1 = m.eq_obj1id[eq_id] as usize;
            let obj2 = m.eq_obj2id[eq_id] as usize;
            let (mut pos1, mut pos2, mut body1, mut body2) = ([0.0; 3], [0.0; 3], 0, 0);
            equality_anchors(m, d, eq_id, &mut pos1, &mut pos2, &mut body1, &mut body2);
            let body1 = body1 as usize;
            let body2 = body2 as usize;
            let (mut jdv1, mut jdv2, mut jrdv1, mut jrdv2) =
                ([0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3]);
            if issparse {
                let nv_mat = core_util::merge_chain(m, &mut chain, body1, body2, 0);
                if nv_mat != 0 {
                    core_util::jac_dot_sparse(
                        m,
                        d,
                        Some(&mut jacdot1),
                        if r#type == EQ_WELD {
                            Some(&mut jacrdot1)
                        } else {
                            None
                        },
                        &pos1,
                        body1,
                        nv_mat,
                        &chain,
                    );
                    core_util::jac_dot_sparse(
                        m,
                        d,
                        Some(&mut jacdot2),
                        if r#type == EQ_WELD {
                            Some(&mut jacrdot2)
                        } else {
                            None
                        },
                        &pos2,
                        body2,
                        nv_mat,
                        &chain,
                    );
                    let [a, b, c] = &mut jdv1;
                    util_sparse::dot_sparse_x3(
                        a,
                        b,
                        c,
                        &jacdot1,
                        &jacdot1[nv_mat..],
                        &jacdot1[2 * nv_mat..],
                        &d.qvel,
                        nv_mat,
                        &chain,
                    );
                    let [a, b, c] = &mut jdv2;
                    util_sparse::dot_sparse_x3(
                        a,
                        b,
                        c,
                        &jacdot2,
                        &jacdot2[nv_mat..],
                        &jacdot2[2 * nv_mat..],
                        &d.qvel,
                        nv_mat,
                        &chain,
                    );
                    if r#type == EQ_WELD {
                        let [a, b, c] = &mut jrdv1;
                        util_sparse::dot_sparse_x3(
                            a,
                            b,
                            c,
                            &jacrdot1,
                            &jacrdot1[nv_mat..],
                            &jacrdot1[2 * nv_mat..],
                            &d.qvel,
                            nv_mat,
                            &chain,
                        );
                        let [a, b, c] = &mut jrdv2;
                        util_sparse::dot_sparse_x3(
                            a,
                            b,
                            c,
                            &jacrdot2,
                            &jacrdot2[nv_mat..],
                            &jacrdot2[2 * nv_mat..],
                            &d.qvel,
                            nv_mat,
                            &chain,
                        );
                    }
                } else {
                    util_blas::zero3(&mut jdv1);
                    util_blas::zero3(&mut jdv2);
                }
            } else {
                core_util::jac_dot(
                    m,
                    d,
                    Some(&mut jacdot1),
                    if r#type == EQ_WELD {
                        Some(&mut jacrdot1)
                    } else {
                        None
                    },
                    &pos1,
                    body1,
                );
                core_util::jac_dot(
                    m,
                    d,
                    Some(&mut jacdot2),
                    if r#type == EQ_WELD {
                        Some(&mut jacrdot2)
                    } else {
                        None
                    },
                    &pos2,
                    body2,
                );
                util_blas::mul_mat_vec(&mut jdv1, &jacdot1, &d.qvel, 3, nv);
                util_blas::mul_mat_vec(&mut jdv2, &jacdot2, &d.qvel, 3, nv);
                if r#type == EQ_WELD {
                    util_blas::mul_mat_vec(&mut jrdv1, &jacrdot1, &d.qvel, 3, nv);
                    util_blas::mul_mat_vec(&mut jrdv2, &jacrdot2, &d.qvel, 3, nv);
                }
            }
            result[row] -= jdv1[0] - jdv2[0];
            result[row + 1] -= jdv1[1] - jdv2[1];
            result[row + 2] -= jdv1[2] - jdv2[2];
            row += 3;
            if r#type == EQ_WELD {
                let torquescale = data[10];
                let (mut q0r, mut negq1) = ([0.0; 4], [0.0; 4]);
                if m.eq_objtype[eq_id] == OBJ_BODY {
                    util_spatial::mul_quat(&mut q0r, at4(&d.xquat, 4 * body1), at4(data, 6));
                    util_spatial::neg_quat(&mut negq1, at4(&d.xquat, 4 * body2));
                } else {
                    util_spatial::mul_quat(
                        &mut q0r,
                        at4(&d.xquat, 4 * body1),
                        at4(&m.site_quat, 4 * obj1),
                    );
                    let mut qsite1 = [0.0; 4];
                    util_spatial::mul_quat(
                        &mut qsite1,
                        at4(&d.xquat, 4 * body2),
                        at4(&m.site_quat, 4 * obj2),
                    );
                    util_spatial::neg_quat(&mut negq1, &qsite1);
                }
                let omega1 = at3(&d.cvel, 6 * body1);
                let omega2 = at3(&d.cvel, 6 * body2);
                let mut domega = [0.0; 3];
                util_blas::sub3(&mut domega, omega1, omega2);
                let mut qdot0 = [0.0; 4];
                if m.eq_objtype[eq_id] == OBJ_BODY {
                    util_spatial::deriv_quat(&mut qdot0, at4(&d.xquat, 4 * body1), omega1);
                } else {
                    let mut qfull0 = [0.0; 4];
                    util_spatial::mul_quat(
                        &mut qfull0,
                        at4(&d.xquat, 4 * body1),
                        at4(&m.site_quat, 4 * obj1),
                    );
                    util_spatial::deriv_quat(&mut qdot0, &qfull0, omega1);
                }
                let mut qdot0r = [0.0; 4];
                if m.eq_objtype[eq_id] == OBJ_BODY {
                    util_spatial::mul_quat(&mut qdot0r, &qdot0, at4(data, 6));
                } else {
                    util_blas::copy4(&mut qdot0r, &qdot0);
                }
                let mut negqdot1 = [0.0; 4];
                if m.eq_objtype[eq_id] == OBJ_BODY {
                    let mut qdot1 = [0.0; 4];
                    util_spatial::deriv_quat(&mut qdot1, at4(&d.xquat, 4 * body2), omega2);
                    util_spatial::neg_quat(&mut negqdot1, &qdot1);
                } else {
                    let (mut qfull1, mut qdot1) = ([0.0; 4], [0.0; 4]);
                    util_spatial::mul_quat(
                        &mut qfull1,
                        at4(&d.xquat, 4 * body2),
                        at4(&m.site_quat, 4 * obj2),
                    );
                    util_spatial::deriv_quat(&mut qdot1, &qfull1, omega2);
                    util_spatial::neg_quat(&mut negqdot1, &qdot1);
                }
                let mut djrdv = [0.0; 3];
                util_blas::sub3(&mut djrdv, &jrdv1, &jrdv2);
                let (mut t1a, mut t1, mut t2a, mut t2, mut t3a, mut t3) =
                    ([0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]);
                util_spatial::mul_quat_axis(&mut t1a, &negqdot1, &domega);
                util_spatial::mul_quat(&mut t1, &t1a, &q0r);
                util_spatial::mul_quat_axis(&mut t2a, &negq1, &djrdv);
                util_spatial::mul_quat(&mut t2, &t2a, &q0r);
                util_spatial::mul_quat_axis(&mut t3a, &negq1, &domega);
                util_spatial::mul_quat(&mut t3, &t3a, &qdot0r);
                result[row] -= 0.5 * (t1[1] + t2[1] + t3[1]) * torquescale;
                result[row + 1] -= 0.5 * (t1[2] + t2[2] + t3[2]) * torquescale;
                result[row + 2] -= 0.5 * (t1[3] + t2[3] + t3[3]) * torquescale;
                row += 3;
            }
        } else {
            while row < ne && d.efc_id[row] == eq_id as i32 {
                row += 1;
            }
        }
    }
    if issparse {
        d.scratch.put_int(chain);
    }
    if !jacdot1.is_empty() {
        d.scratch.put_num(jacdot1);
        d.scratch.put_num(jacdot2);
    }
    if !jacrdot1.is_empty() {
        d.scratch.put_num(jacrdot1);
        d.scratch.put_num(jacrdot2);
    }
}

fn add_constraint_count(m: &Model, size: i32, nv_mat: i32) -> i32 {
    if core_util::is_sparse(m) == 0 {
        return if m.nv != 0 { size } else { 0 };
    }
    if nv_mat.max(0) != 0 {
        size
    } else {
        0
    }
}

fn instantiate_friction(
    m: &Model,
    d: &mut Data,
    count_only: bool,
    mut nnz: Option<&mut i32>,
) -> i32 {
    let nv = m.nv;
    let issparse = core_util::is_sparse(m) != 0;
    let mut nf = 0;
    if disabled(m, DSBL_FRICTIONLOSS) {
        return 0;
    }
    let sleep_filter = enabled(m, ENBL_SLEEP) && d.ntree_awake < m.ntree as i32;
    let mut jac = if !count_only {
        d.scratch.num(nv)
    } else {
        Vec::new()
    };
    for i in 0..nv {
        if m.dof_frictionloss[i] == 0.0 {
            continue;
        }
        if sleep_filter && sleep::sleep_state(m, d, OBJ_DOF, i as i32) == S_ASLEEP {
            continue;
        }
        if count_only {
            nf += add_constraint_count(m, 1, 1);
            if let Some(nnz) = nnz.as_deref_mut() {
                *nnz += 1;
            }
        } else {
            if issparse {
                jac[0] = 1.0;
            } else {
                util_blas::zero(&mut jac, nv);
                jac[i] = 1.0;
            }
            let dof_chain = [i as i32];
            add_constraint(
                m,
                d,
                &jac,
                None,
                None,
                m.dof_frictionloss[i],
                1,
                CNSTR_FRICTION_DOF,
                i as i32,
                if issparse { 1 } else { 0 },
                if issparse { Some(&dof_chain) } else { None },
            );
        }
    }
    for i in 0..m.ntendon {
        if m.tendon_frictionloss[i] > 0.0 {
            if sleep_filter && sleep::sleep_state(m, d, OBJ_TENDON, i as i32) == S_ASLEEP {
                continue;
            }
            if count_only {
                nf += add_constraint_count(m, 1, m.ten_j_rownnz[i]);
                if let Some(nnz) = nnz.as_deref_mut() {
                    *nnz += m.ten_j_rownnz[i];
                }
            } else {
                let efcadr = d.nefc;
                let adr = m.ten_j_rowadr[i] as usize;
                let n = m.ten_j_rownnz[i] as usize;
                if issparse {
                    util_blas::copy(&mut jac, &d.ten_j[adr..], n);
                    add_constraint(
                        m,
                        d,
                        &jac,
                        None,
                        None,
                        m.tendon_frictionloss[i],
                        1,
                        CNSTR_FRICTION_TENDON,
                        i as i32,
                        n as i32,
                        Some(&m.ten_j_colind[adr..]),
                    );
                } else {
                    util_sparse::sparse2dense(
                        &mut jac,
                        &d.ten_j,
                        1,
                        nv,
                        &m.ten_j_rownnz[i..],
                        &m.ten_j_rowadr[i..],
                        &m.ten_j_colind,
                    );
                    add_constraint(
                        m,
                        d,
                        &jac,
                        None,
                        None,
                        m.tendon_frictionloss[i],
                        1,
                        CNSTR_FRICTION_TENDON,
                        i as i32,
                        0,
                        None,
                    );
                }
                if d.nefc > efcadr && d.tendon_efcadr[i] == -1 {
                    d.tendon_efcadr[i] = efcadr;
                }
            }
        }
    }
    if !count_only {
        d.scratch.put_num(jac);
    }
    nf
}

fn instantiate_limit(m: &Model, d: &mut Data, count_only: bool, mut nnz: Option<&mut i32>) -> i32 {
    let nv = m.nv;
    let issparse = core_util::is_sparse(m) != 0;
    let mut nl = 0;
    if disabled(m, DSBL_LIMIT) {
        return 0;
    }
    let sleep_filter = enabled(m, ENBL_SLEEP) && d.ntree_awake < m.ntree as i32;
    let mut jac = if !count_only {
        d.scratch.num(nv)
    } else {
        Vec::new()
    };
    for i in 0..m.njnt {
        if m.jnt_limited[i] == 0 {
            continue;
        }
        if sleep_filter && sleep::sleep_state(m, d, OBJ_JOINT, i as i32) == S_ASLEEP {
            continue;
        }
        let margin = m.jnt_margin[i];
        if m.jnt_type[i] == JNT_SLIDE || m.jnt_type[i] == JNT_HINGE {
            let value = d.qpos[m.jnt_qposadr[i] as usize];
            for side in [-1, 1] {
                let dist = side as f64 * (m.jnt_range[2 * i + ((side + 1) / 2) as usize] - value);
                if dist < margin {
                    if count_only {
                        nl += add_constraint_count(m, 1, 1);
                        if let Some(nnz) = nnz.as_deref_mut() {
                            *nnz += 1;
                        }
                    } else {
                        if issparse {
                            jac[0] = -(side as f64);
                        } else {
                            util_blas::zero(&mut jac, nv);
                            jac[m.jnt_dofadr[i] as usize] = -(side as f64);
                        }
                        add_constraint(
                            m,
                            d,
                            &jac,
                            Some(&[dist]),
                            Some(&[margin]),
                            0.0,
                            1,
                            CNSTR_LIMIT_JOINT,
                            i as i32,
                            if issparse { 1 } else { 0 },
                            if issparse {
                                Some(&m.jnt_dofadr[i..])
                            } else {
                                None
                            },
                        );
                    }
                }
            }
        } else if m.jnt_type[i] == JNT_BALL {
            let adr = m.jnt_qposadr[i] as usize;
            let mut quat = *at4(&d.qpos, adr);
            util_blas::normalize4(&mut quat);
            let mut angle_axis = [0.0; 3];
            util_spatial::quat2vel(&mut angle_axis, &quat, 1.0);
            let value = util_blas::normalize3(&mut angle_axis);
            let dist = max(m.jnt_range[2 * i], m.jnt_range[2 * i + 1]) - value;
            if dist < margin {
                if count_only {
                    nl += add_constraint_count(m, 1, 3);
                    if let Some(nnz) = nnz.as_deref_mut() {
                        *nnz += 3;
                    }
                } else if issparse {
                    let chain = [m.jnt_dofadr[i], m.jnt_dofadr[i] + 1, m.jnt_dofadr[i] + 2];
                    util_blas::scl3(at3_mut(&mut jac, 0), &angle_axis, -1.0);
                    add_constraint(
                        m,
                        d,
                        &jac,
                        Some(&[dist]),
                        Some(&[margin]),
                        0.0,
                        1,
                        CNSTR_LIMIT_JOINT,
                        i as i32,
                        3,
                        Some(&chain),
                    );
                } else {
                    util_blas::zero(&mut jac, nv);
                    util_blas::scl3(
                        at3_mut(&mut jac, m.jnt_dofadr[i] as usize),
                        &angle_axis,
                        -1.0,
                    );
                    add_constraint(
                        m,
                        d,
                        &jac,
                        Some(&[dist]),
                        Some(&[margin]),
                        0.0,
                        1,
                        CNSTR_LIMIT_JOINT,
                        i as i32,
                        0,
                        None,
                    );
                }
            }
        }
    }
    for i in 0..m.ntendon {
        if m.tendon_limited[i] == 0 {
            continue;
        }
        if sleep_filter && sleep::sleep_state(m, d, OBJ_TENDON, i as i32) == S_ASLEEP {
            continue;
        }
        let value = d.ten_length[i];
        let margin = m.tendon_margin[i];
        for side in [-1, 1] {
            let dist = side as f64 * (m.tendon_range[2 * i + ((side + 1) / 2) as usize] - value);
            if dist < margin {
                if count_only {
                    nl += add_constraint_count(m, 1, m.ten_j_rownnz[i]);
                    if let Some(nnz) = nnz.as_deref_mut() {
                        *nnz += m.ten_j_rownnz[i];
                    }
                } else {
                    let efcadr = d.nefc;
                    let adr = m.ten_j_rowadr[i] as usize;
                    let n = m.ten_j_rownnz[i] as usize;
                    if issparse {
                        util_blas::scl(&mut jac, &d.ten_j[adr..], (-side) as f64, n);
                        add_constraint(
                            m,
                            d,
                            &jac,
                            Some(&[dist]),
                            Some(&[margin]),
                            0.0,
                            1,
                            CNSTR_LIMIT_TENDON,
                            i as i32,
                            n as i32,
                            Some(&m.ten_j_colind[adr..]),
                        );
                    } else {
                        util_sparse::sparse2dense(
                            &mut jac,
                            &d.ten_j,
                            1,
                            nv,
                            &m.ten_j_rownnz[i..],
                            &m.ten_j_rowadr[i..],
                            &m.ten_j_colind,
                        );
                        for k in 0..nv {
                            jac[k] *= (-side) as f64;
                        }
                        add_constraint(
                            m,
                            d,
                            &jac,
                            Some(&[dist]),
                            Some(&[margin]),
                            0.0,
                            1,
                            CNSTR_LIMIT_TENDON,
                            i as i32,
                            0,
                            None,
                        );
                    }
                    if d.nefc > efcadr && d.tendon_efcadr[i] == -1 {
                        d.tendon_efcadr[i] = efcadr;
                    }
                }
            }
        }
    }
    if !count_only {
        d.scratch.put_num(jac);
    }
    nl
}

// Return 1 if all bodies are welded together or have no dofs, 0 otherwise.
fn no_dofs(m: &Model, body: &[i32], n: usize) -> i32 {
    let wid0 = if m.body_treeid[body[0] as usize] >= 0 {
        m.body_weldid[body[0] as usize]
    } else {
        0
    };
    for i in 1..n {
        let wid = if m.body_treeid[body[i] as usize] >= 0 {
            m.body_weldid[body[i] as usize]
        } else {
            0
        };
        if wid != wid0 {
            return 0;
        }
    }
    1
}

// Compute Jacobian for contact, return number of DOFs affected.
pub fn contact_jacobian(
    m: &Model,
    d: &mut Data,
    i: usize,
    dim: i32,
    jacdifp: &mut [f64],
    jacdifr: Option<&mut [f64]>,
    jac1p: &mut [f64],
    jac2p: &mut [f64],
    jac1r: Option<&mut [f64]>,
    jac2r: Option<&mut [f64]>,
    chain: &mut [i32],
) -> usize {
    let con = &d.contact[i];
    if (con.geom[0] >= 0
        || (con.vert[0] >= 0 && m.flex_interp[con.flex[0] as usize] == 0 && {
            crate::unported!("mj_rodHasTwist");
        }))
        && (con.geom[1] >= 0
            || (con.vert[1] >= 0 && m.flex_interp[con.flex[1] as usize] == 0 && {
                crate::unported!("mj_rodHasTwist");
            }))
    {
        let mut bid = [0; 2];
        for side in 0..2 {
            bid[side] = if con.geom[side] >= 0 {
                m.geom_bodyid[con.geom[side] as usize]
            } else {
                m.flex_vertbodyid
                    [(m.flex_vertadr[con.flex[side] as usize] + con.vert[side]) as usize]
            };
        }
        if no_dofs(m, &bid, 2) != 0 {
            return 0;
        }
        if dim > 3 {
            core_util::jac_dif_pair(
                m,
                d,
                chain,
                bid[0] as usize,
                bid[1] as usize,
                &con.pos,
                &con.pos,
                Some(jac1p),
                Some(jac2p),
                Some(jacdifp),
                jac1r,
                jac2r,
                jacdifr,
                core_util::is_sparse(m),
                1,
            )
        } else {
            core_util::jac_dif_pair(
                m,
                d,
                chain,
                bid[0] as usize,
                bid[1] as usize,
                &con.pos,
                &con.pos,
                Some(jac1p),
                Some(jac2p),
                Some(jacdifp),
                None,
                None,
                None,
                core_util::is_sparse(m),
                1,
            )
        }
    } else {
        // mjMAXFLEXINTERPW from engine_core_util.h.
        const MAXFLEXINTERPW: usize = 98;
        let mut bid = [0; 2 * MAXFLEXINTERPW];
        let mut bweight = [0.0; 2 * MAXFLEXINTERPW];
        let (pos, flex) = (con.pos, con.flex);
        let mut nb = core_util::contact_body_weight(m, d, con, 0, &mut bid, Some(&mut bweight));
        let input = bweight;
        util_blas::scl(&mut bweight, &input, -1.0, nb);
        nb +=
            core_util::contact_body_weight(m, d, con, 1, &mut bid[nb..], Some(&mut bweight[nb..]));
        if no_dofs(m, &bid, nb) != 0 {
            return 0;
        }
        let nv = core_util::jac_sum(
            m,
            d,
            chain,
            nb,
            &bid,
            &bweight,
            &pos,
            jacdifp,
            jacdifr,
            (dim > 3) as i32,
        );
        if nv > 0 {
            for side in 0..2 {
                if flex[side] >= 0 {
                    // mj_rodContactJac follows if the untranslated twist predicate is true.
                    crate::unported!("mj_rodHasTwist");
                }
            }
        }
        nv
    }
}

pub fn instantiate_contact(m: &Model, d: &mut Data) {
    let ispyramid = core_util::is_pyramidal(m) != 0;
    let issparse = core_util::is_sparse(m) != 0;
    let ncon = d.ncon as usize;
    let nv = m.nv;
    if disabled(m, DSBL_CONTACT) || ncon == 0 || nv == 0 {
        return;
    }
    let mut jac = d.scratch.num(6 * nv);
    let mut jacdifp = d.scratch.num(3 * nv);
    let mut jacdifr = d.scratch.num(3 * nv);
    let mut jac1p = d.scratch.num(3 * nv);
    let mut jac2p = d.scratch.num(3 * nv);
    let mut jac1r = d.scratch.num(3 * nv);
    let mut jac2r = d.scratch.num(3 * nv);
    let mut chain = if issparse {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    for i in 0..ncon {
        if d.contact[i].exclude != 0 {
            continue;
        }
        let dim = d.contact[i].dim as usize;
        d.contact[i].efc_address = d.nefc;
        let nv_mat = contact_jacobian(
            m,
            d,
            i,
            dim as i32,
            &mut jacdifp,
            Some(&mut jacdifr),
            &mut jac1p,
            &mut jac2p,
            Some(&mut jac1r),
            Some(&mut jac2r),
            &mut chain,
        );
        if nv_mat == 0 {
            d.contact[i].efc_address = -1;
            d.contact[i].exclude = 3;
            continue;
        }
        let con = &d.contact[i];
        let (dist, includemargin, friction) = (con.dist, con.includemargin, con.friction);
        util_blas::mul_mat_mat(
            &mut jac,
            &con.frame,
            &jacdifp,
            if dim > 1 { 3 } else { 1 },
            3,
            nv_mat,
        );
        if dim > 3 {
            util_blas::mul_mat_mat(
                &mut jac[3 * nv_mat..],
                &con.frame,
                &jacdifr,
                dim - 3,
                3,
                nv_mat,
            );
        }
        if dim == 1 {
            add_constraint(
                m,
                d,
                &jac,
                Some(&[dist]),
                Some(&[includemargin]),
                0.0,
                1,
                CNSTR_CONTACT_FRICTIONLESS,
                i as i32,
                if issparse { nv_mat as i32 } else { 0 },
                if issparse { Some(&chain) } else { None },
            );
        } else if ispyramid {
            let cpos = [dist; 2];
            let cmargin = [includemargin; 2];
            for k in 1..dim {
                util_blas::add_scl(
                    &mut jacdifp,
                    &jac,
                    &jac[k * nv_mat..],
                    friction[k - 1],
                    nv_mat,
                );
                util_blas::add_scl(
                    &mut jacdifp[nv_mat..],
                    &jac,
                    &jac[k * nv_mat..],
                    -friction[k - 1],
                    nv_mat,
                );
                add_constraint(
                    m,
                    d,
                    &jacdifp,
                    Some(&cpos),
                    Some(&cmargin),
                    0.0,
                    2,
                    CNSTR_CONTACT_PYRAMIDAL,
                    i as i32,
                    if issparse { nv_mat as i32 } else { 0 },
                    if issparse { Some(&chain) } else { None },
                );
            }
        } else {
            let mut cpos = [0.0; 6];
            let mut cmargin = [0.0; 6];
            util_blas::zero(&mut cpos, dim);
            util_blas::zero(&mut cmargin, dim);
            cpos[0] = dist;
            cmargin[0] = includemargin;
            add_constraint(
                m,
                d,
                &jac,
                Some(&cpos),
                Some(&cmargin),
                0.0,
                dim,
                CNSTR_CONTACT_ELLIPTIC,
                i as i32,
                if issparse { nv_mat as i32 } else { 0 },
                if issparse { Some(&chain) } else { None },
            );
        }
    }
    d.scratch.put_num(jac);
    d.scratch.put_num(jacdifp);
    d.scratch.put_num(jacdifr);
    d.scratch.put_num(jac1p);
    d.scratch.put_num(jac2p);
    d.scratch.put_num(jac1r);
    d.scratch.put_num(jac2r);
    if issparse {
        d.scratch.put_int(chain);
    }
}

pub fn diag_approx(m: &Model, d: &mut Data) {
    let mut weldcnt = 0;
    let nefc = d.nefc as usize;
    // a contact side's bodies and weights: written before they are read, so filled once here
    let mut bid = [0; 98];
    let mut bweight = [0.0; 98];
    let mut i = 0;
    while i < nefc {
        let id = d.efc_id[i] as usize;
        match d.efc_type[i] {
            CNSTR_EQUALITY => match m.eq_type[id] {
                EQ_CONNECT => {
                    let mut b1 = m.eq_obj1id[id] as usize;
                    let mut b2 = m.eq_obj2id[id] as usize;
                    if m.eq_objtype[id] == OBJ_SITE {
                        b1 = m.site_bodyid[b1] as usize;
                        b2 = m.site_bodyid[b2] as usize;
                    }
                    d.efc_diag_a[i] = m.body_invweight0[2 * b1] + m.body_invweight0[2 * b2];
                }
                EQ_WELD => {
                    let mut b1 = m.eq_obj1id[id] as usize;
                    let mut b2 = m.eq_obj2id[id] as usize;
                    if m.eq_objtype[id] == OBJ_SITE {
                        b1 = m.site_bodyid[b1] as usize;
                        b2 = m.site_bodyid[b2] as usize;
                    }
                    d.efc_diag_a[i] = m.body_invweight0[2 * b1 + usize::from(weldcnt > 2)]
                        + m.body_invweight0[2 * b2 + usize::from(weldcnt > 2)];
                    weldcnt = (weldcnt + 1) % 6;
                }
                EQ_JOINT | EQ_TENDON => {
                    let obj1 = m.eq_obj1id[id] as usize;
                    d.efc_diag_a[i] = if m.eq_type[id] == EQ_JOINT {
                        m.dof_invweight0[m.jnt_dofadr[obj1] as usize]
                    } else {
                        m.tendon_invweight0[obj1]
                    };
                    if m.eq_obj2id[id] >= 0 {
                        let obj2 = m.eq_obj2id[id] as usize;
                        d.efc_diag_a[i] += if m.eq_type[id] == EQ_JOINT {
                            m.dof_invweight0[m.jnt_dofadr[obj2] as usize]
                        } else {
                            m.tendon_invweight0[obj2]
                        };
                    }
                }
                EQ_FLEX => {
                    let f = m.eq_obj1id[id] as usize;
                    let flex_edgeadr = m.flex_edgeadr[f] as usize;
                    let flex_edgenum = m.flex_edgenum[f] as usize;
                    for e in flex_edgeadr..flex_edgeadr + flex_edgenum {
                        if m.flexedge_rigid[e] == 0 {
                            d.efc_diag_a[i] = m.flexedge_invweight0[e];
                            i += 1;
                        }
                    }
                    // C decrements i here and the for-loop increments it again.
                    continue;
                }
                EQ_FLEXSTRAIN => {
                    let flex_id = m.eq_obj1id[id] as usize;
                    let nstart = m.flex_nodeadr[flex_id] as usize;
                    let interp = m.flex_interp[flex_id];
                    let order = if interp < 0 { -interp } else { interp };
                    let is_shell = interp < 0;
                    let cy = m.flex_cellnum[3 * flex_id + 1];
                    let cz = m.flex_cellnum[3 * flex_id + 2];
                    let (npe, elem_idx) = if is_shell {
                        (
                            ((order + 1) * (order + 1)) as usize,
                            m.eq_data[NEQDATA * id] as i32,
                        )
                    } else {
                        (
                            ((order + 1) * (order + 1) * (order + 1)) as usize,
                            m.eq_data[NEQDATA * id] as i32 * cy * cz
                                + m.eq_data[NEQDATA * id + 1] as i32 * cz
                                + m.eq_data[NEQDATA * id + 2] as i32,
                        )
                    };
                    let ndof_elem = 3 * npe;
                    let k_elem = &m.flex_stiffness[m.flex_stiffnessadr[flex_id] as usize
                        + elem_idx as usize * ndof_elem * ndof_elem..];
                    let nconstraint = k_elem[0] as usize;
                    let gindices: [i32; 125] = if is_shell {
                        (|| -> [i32; 125] { crate::unported!("mju_flexGatherFaceState") })()
                    } else {
                        (|| -> [i32; 125] { crate::unported!("mju_flexGatherCellState") })()
                    };
                    let mut avg_invweight = 0.0;
                    for n in 0..npe {
                        let bodyid = m.flex_nodebodyid[nstart + gindices[n] as usize] as usize;
                        avg_invweight += m.body_invweight0[2 * bodyid];
                    }
                    avg_invweight /= npe as f64;
                    for _c in 0..nconstraint {
                        d.efc_diag_a[i] = avg_invweight;
                        i += 1;
                    }
                    continue;
                }
                _ => panic!("unknown constraint type {}", d.efc_type[i]),
            },
            CNSTR_FRICTION_DOF => d.efc_diag_a[i] = m.dof_invweight0[id],
            CNSTR_LIMIT_JOINT => d.efc_diag_a[i] = m.dof_invweight0[m.jnt_dofadr[id] as usize],
            CNSTR_FRICTION_TENDON | CNSTR_LIMIT_TENDON => d.efc_diag_a[i] = m.tendon_invweight0[id],
            CNSTR_CONTACT_FRICTIONLESS | CNSTR_CONTACT_PYRAMIDAL | CNSTR_CONTACT_ELLIPTIC => {
                let con = &d.contact[id];
                let dim = con.dim as usize;
                let mut tran = 0.0;
                let mut rot = 0.0;
                for side in 0..2 {
                    let nb = core_util::contact_body_weight(
                        m,
                        d,
                        con,
                        side,
                        &mut bid,
                        Some(&mut bweight),
                    );
                    for k in 0..nb {
                        tran += m.body_invweight0[2 * bid[k] as usize] * bweight[k];
                        rot += m.body_invweight0[2 * bid[k] as usize + 1] * bweight[k];
                    }
                }
                if d.efc_type[i] == CNSTR_CONTACT_FRICTIONLESS {
                    d.efc_diag_a[i] = tran;
                } else if d.efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
                    for j in 0..dim {
                        d.efc_diag_a[i + j] = if j < 3 { tran } else { rot };
                    }
                    i += dim - 1;
                } else {
                    for j in 0..dim - 1 {
                        let fri = con.friction[j];
                        let value = tran + fri * fri * if j < 2 { tran } else { rot };
                        d.efc_diag_a[i + 2 * j + 1] = value;
                        d.efc_diag_a[i + 2 * j] = value;
                    }
                    i += 2 * dim - 3;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn getsolparam(
    m: &Model,
    d: &Data,
    i: usize,
    solref: &mut [f64; 2],
    solreffriction: &mut [f64; 2],
    solimp: &mut [f64; 5],
) {
    let id = d.efc_id[i] as usize;
    util_blas::zero(solreffriction, NREF);
    match d.efc_type[i] {
        CNSTR_EQUALITY => {
            util_blas::copy(solref, &m.eq_solref[NREF * id..], NREF);
            util_blas::copy(solimp, &m.eq_solimp[NIMP * id..], NIMP);
        }
        CNSTR_LIMIT_JOINT => {
            util_blas::copy(solref, &m.jnt_solref[NREF * id..], NREF);
            util_blas::copy(solimp, &m.jnt_solimp[NIMP * id..], NIMP);
        }
        CNSTR_FRICTION_DOF => {
            util_blas::copy(solref, &m.dof_solref[NREF * id..], NREF);
            util_blas::copy(solimp, &m.dof_solimp[NIMP * id..], NIMP);
        }
        CNSTR_LIMIT_TENDON => {
            util_blas::copy(solref, &m.tendon_solref_lim[NREF * id..], NREF);
            util_blas::copy(solimp, &m.tendon_solimp_lim[NIMP * id..], NIMP);
        }
        CNSTR_FRICTION_TENDON => {
            util_blas::copy(solref, &m.tendon_solref_fri[NREF * id..], NREF);
            util_blas::copy(solimp, &m.tendon_solimp_fri[NIMP * id..], NIMP);
        }
        CNSTR_CONTACT_FRICTIONLESS | CNSTR_CONTACT_PYRAMIDAL | CNSTR_CONTACT_ELLIPTIC => {
            util_blas::copy(solref, &d.contact[id].solref, NREF);
            util_blas::copy(solreffriction, &d.contact[id].solreffriction, NREF);
            util_blas::copy(solimp, &d.contact[id].solimp, NIMP);
        }
        _ => {}
    }
    if (solref[0] > 0.0) ^ (solref[1] > 0.0) {
        crate::util_errmem::warning_text("mixed solref format, replacing with default");
        crate::model::default_sol_ref_imp(Some(solref), None);
    }
    let metric = core_util::is_metric(m) != 0;
    if !disabled(m, DSBL_REFSAFE) && solref[0] > 0.0 && !metric {
        solref[0] = max(solref[0], 2.0 * m.opt.timestep);
    }
    if (solreffriction[0] > 0.0) ^ (solreffriction[1] > 0.0) {
        crate::util_errmem::warning_text("solreffriction values should have the same sign, replacing with default");
        util_blas::zero(solreffriction, NREF);
    }
    if !disabled(m, DSBL_REFSAFE) && solreffriction[0] > 0.0 && !metric {
        solreffriction[0] = max(solreffriction[0], 2.0 * m.opt.timestep);
    }
    solimp[0] = min(MAXIMP, max(MINIMP, solimp[0]));
    solimp[1] = min(MAXIMP, max(MINIMP, solimp[1]));
    solimp[2] = max(0.0, solimp[2]);
    solimp[3] = min(MAXIMP, max(MINIMP, solimp[3]));
    solimp[4] = max(1.0, solimp[4]);
}

fn getposdim(m: &Model, d: &Data, i: usize, pos: &mut f64, dim: &mut usize) {
    let id = d.efc_id[i] as usize;
    *dim = 1;
    *pos = d.efc_pos[i];
    match d.efc_type[i] {
        CNSTR_CONTACT_ELLIPTIC => *dim = d.contact[id].dim as usize,
        CNSTR_CONTACT_PYRAMIDAL => *dim = 2 * (d.contact[id].dim as usize - 1),
        CNSTR_EQUALITY => {
            if m.eq_type[id] == EQ_WELD {
                *dim = 6;
                *pos = util_blas::norm(&d.efc_pos[i..], 6);
            } else if m.eq_type[id] == EQ_CONNECT {
                *dim = 3;
                *pos = util_blas::norm(&d.efc_pos[i..], 3);
            }
        }
        _ => {}
    }
}

// Public for the integration test of C's scalar impedance formula.
pub fn power(a: f64, b: f64) -> f64 {
    if b == 1.0 {
        return a;
    } else if b == 2.0 {
        return a * a;
    }
    a.powf(b)
}

pub fn getimpedance(solimp: &[f64; 5], pos: f64, margin: f64, imp: &mut f64, imp_p: &mut f64) {
    if solimp[0] == solimp[1] || solimp[2] <= MINVAL {
        *imp = 0.5 * (solimp[0] + solimp[1]);
        *imp_p = 0.0;
        return;
    }
    let mut x = (pos - margin) / solimp[2];
    let mut sgn = 1.0;
    if x < 0.0 {
        x = -x;
        sgn = -1.0;
    }
    if x >= 1.0 || x <= 0.0 {
        *imp = if x >= 1.0 { solimp[1] } else { solimp[0] };
        *imp_p = 0.0;
        return;
    }
    let (y, y_p);
    if solimp[4] == 1.0 {
        y = x;
        y_p = 1.0;
    } else if x <= solimp[3] {
        let a = 1.0 / power(solimp[3], solimp[4] - 1.0);
        y = a * power(x, solimp[4]);
        y_p = solimp[4] * a * power(x, solimp[4] - 1.0);
    } else {
        let b = 1.0 / power(1.0 - solimp[3], solimp[4] - 1.0);
        y = 1.0 - b * power(1.0 - x, solimp[4]);
        y_p = solimp[4] * b * power(1.0 - x, solimp[4] - 1.0);
    }
    *imp = solimp[0] + y * (solimp[1] - solimp[0]);
    *imp_p = y_p * sgn * (solimp[1] - solimp[0]) / solimp[2];
}

fn implicit_factor(kbip: &[f64], h: f64) -> f64 {
    1.0 + h * kbip[1] + h * h * kbip[0] * kbip[2]
}

pub fn make_impedance(m: &Model, d: &mut Data) {
    let nefc = d.nefc as usize;
    let metric = core_util::is_metric(m) != 0;
    let h = m.opt.timestep;
    let mut i = 0;
    while i < nefc {
        let (mut solref, mut solreffriction, mut solimp) = ([0.0; 2], [0.0; 2], [0.0; 5]);
        getsolparam(m, d, i, &mut solref, &mut solreffriction, &mut solimp);
        let (mut pos, mut dim, mut imp, mut imp_p) = (0.0, 0, 0.0, 0.0);
        getposdim(m, d, i, &mut pos, &mut dim);
        getimpedance(&solimp, pos, d.efc_margin[i], &mut imp, &mut imp_p);
        // This constraint's rows as slices: a store below reloads none of the other arrays.
        let efc_r = &mut d.efc_r[i..i + dim];
        let efc_kbip = &mut d.efc_kbip[4 * i..4 * (i + dim)];
        let efc_diag_a = &d.efc_diag_a[i..i + dim];
        let efc_type = &d.efc_type[i..i + dim];
        for j in 0..dim {
            efc_r[j] = max(MINVAL, (1.0 - imp) * efc_diag_a[j] / imp);
            let tp = efc_type[j];
            let elliptic_friction = tp == CNSTR_CONTACT_ELLIPTIC && j > 0;
            let r#ref =
                if elliptic_friction && (solreffriction[0] != 0.0 || solreffriction[1] != 0.0) {
                    &solreffriction
                } else {
                    &solref
                };
            let kbip = &mut efc_kbip[4 * j..4 * j + 4];
            if tp == CNSTR_FRICTION_DOF || tp == CNSTR_FRICTION_TENDON || elliptic_friction {
                kbip[0] = 0.0;
            } else if r#ref[0] > 0.0 {
                kbip[0] = 1.0
                    / max(
                        MINVAL,
                        solimp[1] * solimp[1] * r#ref[0] * r#ref[0] * r#ref[1] * r#ref[1],
                    );
            } else {
                kbip[0] = -r#ref[0] / max(MINVAL, solimp[1] * solimp[1]);
            }
            if r#ref[1] > 0.0 {
                kbip[1] = 2.0 / max(MINVAL, solimp[1] * r#ref[0]);
            } else {
                kbip[1] = -r#ref[1] / max(MINVAL, solimp[1]);
            }
            kbip[2] = imp;
            kbip[3] = imp_p;
            if metric {
                if !disabled(m, DSBL_REFSAFE)
                    && r#ref[0] > 0.0
                    && kbip[0] > 0.0
                    && matches!(
                        tp,
                        CNSTR_LIMIT_JOINT
                            | CNSTR_LIMIT_TENDON
                            | CNSTR_CONTACT_FRICTIONLESS
                            | CNSTR_CONTACT_PYRAMIDAL
                            | CNSTR_CONTACT_ELLIPTIC
                    )
                {
                    let excess = (h * h) * kbip[0] * kbip[2];
                    if excess > 1.0 {
                        let fmax = MAXIMP * (1.0 - kbip[2]) / max(MINVAL, kbip[2] * (1.0 - MAXIMP));
                        kbip[0] = 1.0 / (h * h * kbip[2]);
                        kbip[1] = min(kbip[1] / excess.sqrt(), max(0.0, fmax - 2.0) / h);
                    }
                }
                let f = implicit_factor(kbip, h);
                let rmin = max(MINVAL, (1.0 - MAXIMP) * efc_diag_a[j] / MAXIMP);
                efc_r[j] = max(efc_r[j] / f, rmin);
            }
        }
        i += dim;
    }
    // The efc arrays cut to nefc once for the passes below.
    let efc_r = &mut d.efc_r[..nefc];
    let efc_type = &d.efc_type[..nefc];
    let efc_id = &d.efc_id[..nefc];
    let contact = &mut d.contact[..];
    i = (d.ne + d.nf) as usize;
    while i < nefc {
        if efc_type[i] == CNSTR_CONTACT_PYRAMIDAL || efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
            let con = &mut contact[efc_id[i] as usize];
            let dim = con.dim as usize;
            let friction = con.friction;
            efc_r[i + 1] = efc_r[i] / max(MINVAL, m.opt.impratio);
            con.mu = friction[0] * (efc_r[i + 1] / efc_r[i]).sqrt();
            if efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
                for j in 1..dim - 1 {
                    efc_r[i + j + 1] =
                        efc_r[i + 1] * friction[0] * friction[0] / (friction[j] * friction[j]);
                }
                i += dim - 1;
            } else {
                let rpy = 2.0 * con.mu * con.mu * efc_r[i];
                for j in 0..2 * (dim - 1) {
                    efc_r[i + j] = rpy;
                }
                i += 2 * (dim - 1) - 1;
            }
        }
        i += 1;
    }
    let efc_d = &mut d.efc_d[..nefc];
    for i in 0..nefc {
        efc_d[i] = 1.0 / efc_r[i];
    }
    let efc_kbip = &d.efc_kbip[..4 * nefc];
    let efc_diag_a = &mut d.efc_diag_a[..nefc];
    for i in 0..nefc {
        efc_diag_a[i] = efc_r[i] * efc_kbip[4 * i + 2] / (1.0 - efc_kbip[4 * i + 2]);
    }
}

pub fn regularize_constraint(m: &Model, d: &mut Data, flg_ar: i32) {
    let nefc = d.nefc as usize;
    let nv = m.nv;
    if nefc == 0 {
        return;
    }
    let is_dual = core_util::is_dual(m) != 0;
    if enabled(m, ENBL_DIAGEXACT) || (is_dual && flg_ar != 0) {
        make_ynumeric(m, d, enabled(m, ENBL_DIAGEXACT) as i32);
    }
    if !enabled(m, ENBL_DIAGEXACT) {
        diag_approx(m, d);
        if core_util::is_metric(m) != 0 && !d.efm_sdiag.is_empty() {
            let is_sparse = core_util::is_sparse(m) != 0;
            for r in 0..nefc {
                let mut num = 0.0;
                let mut den = 0.0;
                let rownnz = if is_sparse {
                    d.efc_j_rownnz[r] as usize
                } else {
                    nv
                };
                let rowadr = if is_sparse {
                    d.efc_j_rowadr[r] as usize
                } else {
                    r * nv
                };
                for a in 0..rownnz {
                    let c = if is_sparse {
                        d.efc_j_colind[rowadr + a] as usize
                    } else {
                        a
                    };
                    let j = d.efc_j[rowadr + a];
                    if j == 0.0 {
                        continue;
                    }
                    let mdiag = d.m[(m.m_rowadr[c] + m.m_rownnz[c] - 1) as usize];
                    num += j * j / max(MINVAL, mdiag + d.efm_sdiag[c]);
                    den += j * j / max(MINVAL, mdiag);
                }
                if den > MINVAL {
                    d.efc_diag_a[r] *= num / den;
                }
            }
        }
    }
    make_impedance(m, d);
    if is_dual && flg_ar != 0 {
        make_arnumeric(m, d);
    }
    if d.nisland != 0 {
        crate::util_misc::gather(&mut d.iefc_d, &d.efc_d, Some(&d.map_iefc2efc), nefc);
        crate::util_misc::gather(&mut d.iefc_r, &d.efc_r, Some(&d.map_iefc2efc), nefc);
    }
}

fn jac_sum_count(m: &Model, d: &mut Data, chain: &mut [i32], n: usize, body: &[i32]) -> usize {
    let nv = m.nv;
    let mut bodychain = d.scratch.int(nv);
    let mut tempchain = d.scratch.int(nv);
    let mut nv_mat = core_util::body_chain(m, body[0] as usize, chain);
    for i in 1..n {
        let body_nv = core_util::body_chain(m, body[i] as usize, &mut bodychain);
        if body_nv == 0 {
            continue;
        }
        nv_mat = util_sparse::add_chains(&mut tempchain, nv, nv_mat, body_nv, chain, &bodychain);
        if nv_mat != 0 {
            chain[..nv_mat].copy_from_slice(&tempchain[..nv_mat]);
        }
    }
    d.scratch.put_int(bodychain);
    d.scratch.put_int(tempchain);
    nv_mat
}

fn ne(m: &Model, d: &mut Data, nnz: Option<&mut i32>) -> i32 {
    let mut ne = 0;
    let mut nnze = 0;
    let nv = m.nv;
    let issparse = nnz.is_some();
    if disabled(m, DSBL_EQUALITY) || m.nemax == 0 {
        return 0;
    }
    let sleep_filter = enabled(m, ENBL_SLEEP) && d.ntree_awake < m.ntree as i32;
    let mut chain = if issparse {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    let mut chain2 = if issparse {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    let mut cell_bodies = if issparse {
        d.scratch.int(125)
    } else {
        Vec::new()
    };
    for i in 0..m.neq {
        if d.eq_active[i] == 0 {
            continue;
        }
        if sleep_filter && sleep::sleep_state(m, d, OBJ_EQUALITY, i as i32) == S_ASLEEP {
            continue;
        }
        let mut id = [m.eq_obj1id[i], m.eq_obj2id[i]];
        let mut size = 0;
        let mut nv_mat = 0;
        let mut nv2 = 0;
        match m.eq_type[i] {
            EQ_CONNECT | EQ_WELD => {
                size = if m.eq_type[i] == EQ_CONNECT { 3 } else { 6 };
                if issparse {
                    if m.eq_objtype[i] == OBJ_SITE {
                        id[0] = m.site_bodyid[id[0] as usize];
                        id[1] = m.site_bodyid[id[1] as usize];
                    }
                    // All outputs are absent: jac_dif_pair returns the count before reading positions or Data.
                    nv_mat = core_util::jac_dif_pair(
                        m,
                        d,
                        &mut chain,
                        id[1] as usize,
                        id[0] as usize,
                        &[0.0; 3],
                        &[0.0; 3],
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        1,
                        0,
                    ) as i32;
                }
            }
            EQ_JOINT | EQ_TENDON => {
                size = 1;
                if issparse {
                    for j in 0..1 + usize::from(id[1] >= 0) {
                        let obj = id[j] as usize;
                        if m.eq_type[i] == EQ_JOINT {
                            if j == 0 {
                                nv_mat = 1;
                                chain[0] = m.jnt_dofadr[obj];
                            } else {
                                nv2 = 1;
                                chain2[0] = m.jnt_dofadr[obj];
                            }
                        } else {
                            let n = m.ten_j_rownnz[obj] as usize;
                            let adr = m.ten_j_rowadr[obj] as usize;
                            if j == 0 {
                                nv_mat = n as i32;
                                chain[..n].copy_from_slice(&m.ten_j_colind[adr..adr + n]);
                            } else {
                                nv2 = n as i32;
                                chain2[..n].copy_from_slice(&m.ten_j_colind[adr..adr + n]);
                            }
                        }
                    }
                    if id[1] >= 0 {
                        nv_mat = util_sparse::combine_sparse_count(
                            nv_mat as usize,
                            nv2 as usize,
                            &chain,
                            &chain2,
                        ) as i32;
                    }
                }
            }
            EQ_FLEX => {
                let f = id[0] as usize;
                let flex_edgeadr = m.flex_edgeadr[f] as usize;
                let flex_edgenum = m.flex_edgenum[f] as usize;
                size = flex_edgenum as i32;
                for e in flex_edgeadr..flex_edgeadr + flex_edgenum {
                    if m.flexedge_rigid[e] != 0 {
                        size -= 1;
                        continue;
                    }
                    if issparse {
                        let b1 =
                            m.flex_vertbodyid[(m.flex_vertadr[f] + m.flex_edge[2 * e]) as usize];
                        let b2 = m.flex_vertbodyid
                            [(m.flex_vertadr[f] + m.flex_edge[2 * e + 1]) as usize];
                        nv_mat += core_util::jac_dif_pair(
                            m,
                            d,
                            &mut chain,
                            b1 as usize,
                            b2 as usize,
                            &[0.0; 3],
                            &[0.0; 3],
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            1,
                            0,
                        ) as i32;
                    }
                }
            }
            EQ_FLEXSTRAIN => {
                let f = id[0] as usize;
                let interp = m.flex_interp[f];
                let order = if interp < 0 { -interp } else { interp };
                let is_shell = interp < 0;
                if order != 0 && m.flex_nodenum[f] != 0 {
                    let cy = m.flex_cellnum[3 * f + 1];
                    let cz = m.flex_cellnum[3 * f + 2];
                    let (npe, elem_idx) = if is_shell {
                        (
                            ((order + 1) * (order + 1)) as usize,
                            m.eq_data[NEQDATA * i] as i32,
                        )
                    } else {
                        (
                            ((order + 1) * (order + 1) * (order + 1)) as usize,
                            m.eq_data[NEQDATA * i] as i32 * cy * cz
                                + m.eq_data[NEQDATA * i + 1] as i32 * cz
                                + m.eq_data[NEQDATA * i + 2] as i32,
                        )
                    };
                    let ndof_elem = 3 * npe;
                    if m.flex_stiffnessadr[f] >= 0 {
                        let k_elem = &m.flex_stiffness[m.flex_stiffnessadr[f] as usize
                            + elem_idx as usize * ndof_elem * ndof_elem..];
                        size = k_elem[0] as i32;
                    }
                    if issparse {
                        let gindices: [i32; 125] = if is_shell {
                            (|| -> [i32; 125] { crate::unported!("mju_flexGatherFaceState") })()
                        } else {
                            (|| -> [i32; 125] { crate::unported!("mju_flexGatherCellState") })()
                        };
                        let nstart = m.flex_nodeadr[f] as usize;
                        for n in 0..npe {
                            cell_bodies[n] = m.flex_nodebodyid[nstart + gindices[n] as usize];
                        }
                        nv_mat = jac_sum_count(m, d, &mut chain, npe, &cell_bodies) as i32;
                        nv_mat = size * nv_mat;
                    }
                }
            }
            _ => panic!("unknown constraint type {}", m.eq_type[i]),
        }
        ne += add_constraint_count(m, size, nv_mat);
        if m.eq_type[i] == EQ_FLEX || m.eq_type[i] == EQ_FLEXSTRAIN {
            nnze += nv_mat;
        } else {
            nnze += size * nv_mat;
        }
    }
    if let Some(nnz) = nnz {
        *nnz += nnze;
    }
    if issparse {
        d.scratch.put_int(chain);
        d.scratch.put_int(chain2);
        d.scratch.put_int(cell_bodies);
    }
    ne
}

fn nc(m: &Model, d: &mut Data, nnz: Option<&mut i32>) -> i32 {
    let mut nnzc = 0;
    let mut nc = 0;
    let ispyramid = core_util::is_pyramidal(m) != 0;
    let ncon = d.ncon as usize;
    if disabled(m, DSBL_CONTACT) || ncon == 0 {
        return 0;
    }
    let sleep_filter = enabled(m, ENBL_SLEEP) && d.ntree_awake < m.ntree as i32;
    let mut chain = d.scratch.int(m.nv);
    for i in 0..ncon {
        let Contact { geom, flex, vert, dim, .. } = d.contact[i];
        let f0 = flex[0];
        let f1 = flex[1];
        let wants = (f0 > -1 && core_util::eff_flex_contact_possible(m, f0 as usize) != 0)
            || (f1 > -1 && core_util::eff_flex_contact_possible(m, f1 as usize) != 0);
        let mut ok = f0 > -1 && f1 > -1;
        let mut s = 0;
        while s < 2 && !ok {
            if flex[s] < 0 && geom[s] > -1 {
                ok = m.body_weldid[m.geom_bodyid[geom[s] as usize] as usize] == 0;
            }
            s += 1;
        }
        if wants && ok {
            d.contact[i].efc_address = -1;
            d.contact[i].exclude = 4;
        }
        if d.contact[i].exclude != 0 {
            continue;
        }
        if sleep_filter {
            let g1 = geom[0];
            let g2 = geom[1];
            if g1 >= 0 && g2 >= 0 {
                let b1 = m.body_weldid[m.geom_bodyid[g1 as usize] as usize] as usize;
                let b2 = m.body_weldid[m.geom_bodyid[g2 as usize] as usize] as usize;
                let asleep1 = d.body_awake[b1] as i32 == S_ASLEEP;
                let asleep2 = d.body_awake[b2] as i32 == S_ASLEEP;
                if asleep1 || asleep2 {
                    panic!(
                        "contact {} involves sleeping geom {}",
                        i,
                        if asleep1 { g1 } else { g2 }
                    );
                }
            }
            for side in 0..2 {
                if geom[side] >= 0 {
                    continue;
                }
                let b = sleep::flex_body(m, &d.contact[i], side) as usize;
                if d.body_awake[m.body_weldid[b] as usize] as i32 == S_ASLEEP {
                    panic!("contact {} involves sleeping flex {}", i, flex[side]);
                }
            }
        }
        let mut nv_mat = 0;
        if nnz.is_some() {
            if (geom[0] >= 0
                || (vert[0] >= 0 && m.flex_interp[flex[0] as usize] == 0 && {
                    crate::unported!("mj_rodHasTwist");
                }))
                && (geom[1] >= 0
                    || (vert[1] >= 0 && m.flex_interp[flex[1] as usize] == 0 && {
                        crate::unported!("mj_rodHasTwist");
                    }))
            {
                let mut bid = [0; 2];
                for side in 0..2 {
                    bid[side] = if geom[side] >= 0 {
                        m.geom_bodyid[geom[side] as usize]
                    } else {
                        m.flex_vertbodyid
                            [(m.flex_vertadr[flex[side] as usize] + vert[side]) as usize]
                    };
                }
                if no_dofs(m, &bid, 2) == 0 {
                    nv_mat = core_util::jac_dif_pair(
                        m,
                        d,
                        &mut chain,
                        bid[0] as usize,
                        bid[1] as usize,
                        &[0.0; 3],
                        &[0.0; 3],
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        core_util::is_sparse(m),
                        1,
                    ) as i32;
                }
            } else {
                let mut bid = [0; 196];
                let mut nb = core_util::contact_body_weight(m, d, &d.contact[i], 0, &mut bid, None);
                nb += core_util::contact_body_weight(m, d, &d.contact[i], 1, &mut bid[nb..], None);
                if no_dofs(m, &bid, nb) == 0 {
                    nv_mat = jac_sum_count(m, d, &mut chain, nb, &bid) as i32;
                }
            }
            if nv_mat == 0 {
                continue;
            }
        }
        if dim == 1 {
            nc += 1;
            nnzc += nv_mat;
        } else if ispyramid {
            nc += 2 * (dim - 1);
            nnzc += 2 * (dim - 1) * nv_mat;
        } else {
            nc += dim;
            nnzc += dim * nv_mat;
        }
    }
    if let Some(nnz) = nnz {
        *nnz += nnzc;
    }
    d.scratch.put_int(chain);
    nc
}

fn compute_y_precount(
    y_rownnz: &mut [i32],
    y_rowadr: &mut [i32],
    nefc: usize,
    nv: usize,
    j_rownnz: &[i32],
    j_rowadr: &[i32],
    j_colind: &[i32],
    m_rownnz: &[i32],
    m_rowadr: &[i32],
    m_colind: &[i32],
    marker: &mut [i32],
) -> i32 {
    marker[..nv].fill(-1);
    y_rowadr[0] = 0;
    for r in 0..nefc {
        let mut nnz = 0;
        let start = j_rowadr[r] as usize;
        let end = start + j_rownnz[r] as usize;
        for i in (start..end).rev() {
            let j = j_colind[i] as usize;
            if marker[j] == r as i32 {
                continue;
            }
            let nnz_m = m_rownnz[j] as usize;
            let adr_m = m_rowadr[j] as usize;
            for k in 0..nnz_m {
                let c = m_colind[adr_m + k] as usize;
                if marker[c] != r as i32 {
                    marker[c] = r as i32;
                    nnz += 1;
                }
            }
        }
        y_rownnz[r] = nnz;
        if r < nefc - 1 {
            y_rowadr[r + 1] = y_rowadr[r] + nnz;
        }
    }
    y_rowadr[nefc - 1] + y_rownnz[nefc - 1]
}

fn compute_y_fill(
    mut y: Option<&mut [f64]>,
    y_colind: &mut [i32],
    y_rownnz: &[i32],
    y_rowadr: &[i32],
    nefc: usize,
    j_mat: &[f64],
    j_rownnz: &[i32],
    j_rowadr: &[i32],
    j_colind: &[i32],
    m_rownnz: &[i32],
    m_rowadr: &[i32],
    m_colind: &[i32],
) {
    // C J and j collide after case conversion; J becomes j_mat.
    for r in 0..nefc {
        let end = (y_rowadr[r] + y_rownnz[r]) as usize;
        let adr_j = j_rowadr[r] as usize;
        let mut remain_j = j_rownnz[r] as usize;
        let mut nnz_y = 0;
        loop {
            let prev_src = if remain_j > 0 {
                j_colind[adr_j + remain_j - 1]
            } else {
                -1
            };
            let mut prev_dst = -1;
            if nnz_y > 0 {
                let j = y_colind[end - nnz_y] as usize;
                if m_rownnz[j] > 1 {
                    prev_dst = m_colind[(m_rowadr[j] + m_rownnz[j] - 2) as usize];
                }
            }
            if prev_src < 0 && prev_dst < 0 {
                break;
            } else if prev_src >= prev_dst {
                nnz_y += 1;
                remain_j -= 1;
                y_colind[end - nnz_y] = prev_src;
                if let Some(y) = y.as_deref_mut() {
                    y[end - nnz_y] = j_mat[adr_j + remain_j];
                }
            } else {
                nnz_y += 1;
                y_colind[end - nnz_y] = prev_dst;
                if let Some(y) = y.as_deref_mut() {
                    y[end - nnz_y] = 0.0;
                }
            }
        }
        if nnz_y != y_rownnz[r] as usize {
            panic!("pre and post-count of Y_rownnz are not equal on row {}", r);
        }
    }
}

fn compute_y_backsub(
    y: &mut [f64],
    y_rownnz: &[i32],
    y_rowadr: &[i32],
    y_colind: &[i32],
    nefc: usize,
    q_ld: &[f64],
    m_rownnz: &[i32],
    m_rowadr: &[i32],
    m_colind: &[i32],
    sqrt_inv_d: &[f64],
) {
    for r in 0..nefc {
        let nnz_y = y_rownnz[r] as usize;
        let adr_y = y_rowadr[r] as usize;
        for i in (adr_y..adr_y + nnz_y).rev() {
            let val = y[i];
            if val == 0.0 {
                continue;
            }
            let j = y_colind[i] as usize;
            let adr_m = m_rowadr[j] as usize;
            util_sparse::add_to_scl_sparse_inc(
                &mut y[adr_y..],
                &q_ld[adr_m..],
                nnz_y,
                &y_colind[adr_y..],
                (m_rownnz[j] - 1) as usize,
                &m_colind[adr_m..],
                -val,
            );
        }
        for i in adr_y..adr_y + nnz_y {
            let j = y_colind[i] as usize;
            y[i] *= sqrt_inv_d[j];
        }
    }
}

pub fn make_constraint(m: &Model, d: &mut Data) {
    d.ne = 0;
    d.nf = 0;
    d.nl = 0;
    d.nefc = 0;
    d.n_j = 0;
    d.n_a = 0;
    d.n_y = 0;
    if disabled(m, DSBL_CONSTRAINT) {
        return;
    }
    let issparse = core_util::is_sparse(m) != 0;
    // Copy out nJ while callees borrow Data, then restore the accumulated count.
    let mut n_j = d.n_j;
    let ne_allocated = ne(m, d, if issparse { Some(&mut n_j) } else { None });
    let nf_allocated =
        instantiate_friction(m, d, true, if issparse { Some(&mut n_j) } else { None });
    let nl_allocated = instantiate_limit(m, d, true, if issparse { Some(&mut n_j) } else { None });
    let nc_allocated = nc(m, d, if issparse { Some(&mut n_j) } else { None });
    let nefc_allocated = ne_allocated + nf_allocated + nl_allocated + nc_allocated;
    d.n_j = n_j;
    if !issparse {
        d.n_j = nefc_allocated * m.nv as i32;
    }
    d.nefc = nefc_allocated;
    if arena_alloc_efc(m, d) == 0 {
        return;
    }
    d.tendon_efcadr[..m.ntendon].fill(-1);
    d.nefc = 0;
    instantiate_equality(m, d);
    instantiate_friction(m, d, false, None);
    instantiate_limit(m, d, false, None);
    instantiate_contact(m, d);
    if issparse {
        if d.ne != ne_allocated {
            panic!(
                "ne mis-allocation: found ne={} but allocated {}",
                d.ne, ne_allocated
            );
        }
        if d.nf != nf_allocated {
            panic!(
                "nf mis-allocation: found nf={} but allocated {}",
                d.nf, nf_allocated
            );
        }
        if d.nl != nl_allocated {
            panic!(
                "nl mis-allocation: found nl={} but allocated {}",
                d.nl, nl_allocated
            );
        }
        if d.nefc != nefc_allocated {
            panic!(
                "nefc mis-allocation: found nefc={} but allocated {}",
                d.nefc, nefc_allocated
            );
        }
        if d.nefc > 0 {
            let n_j = d.efc_j_rownnz[d.nefc as usize - 1] + d.efc_j_rowadr[d.nefc as usize - 1];
            if d.n_j != n_j {
                panic!(
                    "constraint Jacobian mis-allocation: found nJ={} but allocated {}",
                    n_j, d.n_j
                );
            }
        }
    } else if d.nefc > nefc_allocated {
        panic!(
            "nefc under-allocation: found nefc={} but allocated only {}",
            d.nefc, nefc_allocated
        );
    }
    d.maxuse_con = d.maxuse_con.max(d.ncon);
    d.maxuse_efc = d.maxuse_efc.max(d.nefc);
    if d.nefc == 0 {
        return;
    }
    if issparse {
        util_sparse::super_sparse(
            d.nefc as usize,
            &mut d.efc_j_rowsuper,
            &d.efc_j_rownnz,
            &d.efc_j_rowadr,
            &d.efc_j_colind,
        );
    }
    if core_util::is_metric(m) == 0 {
        diag_approx(m, d);
        make_impedance(m, d);
    }
}

fn make_ysymbolic(m: &Model, d: &mut Data) {
    let nefc = d.nefc as usize;
    let nv = m.nv;
    // All arena allocations grow Vecs; C's arena-overflow branches cannot occur.
    if core_util::is_sparse(m) != 0 {
        d.efc_y_rownnz.resize(nefc, 0);
        d.efc_y_rowadr.resize(nefc, 0);
        let mut marker = d.scratch.int(nv);
        d.n_y = compute_y_precount(
            &mut d.efc_y_rownnz,
            &mut d.efc_y_rowadr,
            nefc,
            nv,
            &d.efc_j_rownnz,
            &d.efc_j_rowadr,
            &d.efc_j_colind,
            &m.m_rownnz,
            &m.m_rowadr,
            &m.m_colind,
            &mut marker,
        );
        d.scratch.put_int(marker);
        d.efc_y.resize(d.n_y as usize, 0.0);
        d.efc_y_colind.resize(d.n_y as usize, 0);
        if core_util::is_metric(m) != 0 {
            compute_y_fill(
                None,
                &mut d.efc_y_colind,
                &d.efc_y_rownnz,
                &d.efc_y_rowadr,
                nefc,
                &[],
                &d.efc_j_rownnz,
                &d.efc_j_rowadr,
                &d.efc_j_colind,
                &m.m_rownnz,
                &m.m_rowadr,
                &m.m_colind,
            );
        }
    } else {
        d.n_y = (nefc * nv) as i32;
        d.efc_y.resize(d.n_y as usize, 0.0);
    }
}

fn make_ynumeric(m: &Model, d: &mut Data, flg_diagexact: i32) {
    let nefc = d.nefc as usize;
    let nv = m.nv;
    if d.efc_y.is_empty() {
        return;
    }
    let factor = if core_util::eff_use_qh(d) != 0 {
        &d.q_h
    } else {
        &d.q_ld
    };
    let mut sqrt_inv_d = d.scratch.num(nv);
    for i in 0..nv {
        let diag = (m.m_rowadr[i] + m.m_rownnz[i] - 1) as usize;
        sqrt_inv_d[i] = 1.0 / factor[diag].sqrt();
    }
    if core_util::is_sparse(m) != 0 {
        compute_y_fill(
            Some(&mut d.efc_y),
            &mut d.efc_y_colind,
            &d.efc_y_rownnz,
            &d.efc_y_rowadr,
            nefc,
            &d.efc_j,
            &d.efc_j_rownnz,
            &d.efc_j_rowadr,
            &d.efc_j_colind,
            &m.m_rownnz,
            &m.m_rowadr,
            &m.m_colind,
        );
        compute_y_backsub(
            &mut d.efc_y,
            &d.efc_y_rownnz,
            &d.efc_y_rowadr,
            &d.efc_y_colind,
            nefc,
            factor,
            &m.m_rownnz,
            &m.m_rowadr,
            &m.m_colind,
            &sqrt_inv_d,
        );
        if flg_diagexact != 0 {
            for i in 0..nefc {
                let adr = d.efc_y_rowadr[i] as usize;
                let nnz = d.efc_y_rownnz[i] as usize;
                d.efc_diag_a[i] = util_blas::dot(&d.efc_y[adr..], &d.efc_y[adr..], nnz);
            }
        }
    } else {
        core_smooth::solve_m2_impl(
            &mut d.efc_y,
            &d.efc_j,
            &sqrt_inv_d,
            factor,
            nv,
            nefc,
            &m.m_rownnz,
            &m.m_rowadr,
            &m.m_colind,
            &m.dof_simplenum,
        );
        if flg_diagexact != 0 {
            for i in 0..nefc {
                d.efc_diag_a[i] = util_blas::dot(&d.efc_y[i * nv..], &d.efc_y[i * nv..], nv);
            }
        }
    }
    d.scratch.put_num(sqrt_inv_d);
}

fn make_arsymbolic(m: &Model, d: &mut Data) {
    let nefc = d.nefc as usize;
    let nv = m.nv;
    if core_util::is_sparse(m) != 0 {
        let mut yt_rownnz = d.scratch.int(nv);
        let mut yt_rowadr = d.scratch.int(nv);
        let mut yt_colind = d.scratch.int(d.n_y as usize);
        util_sparse::transpose_sparse(
            None,
            &[],
            nefc,
            nv,
            &mut yt_rownnz,
            &mut yt_rowadr,
            &mut yt_colind,
            None,
            &d.efc_y_rownnz,
            &d.efc_y_rowadr,
            &d.efc_y_colind,
        );
        d.efc_ar_rownnz.resize(nefc, 0);
        d.efc_ar_rowadr.resize(nefc, 0);
        let mut diagind = d.scratch.int(nefc);
        // The sparse routines use Data only for scratch. Move the field buffers out
        // temporarily so their input/output borrows do not alias that scratch borrow.
        let mut ar_rownnz = std::mem::take(&mut d.efc_ar_rownnz);
        let mut ar_rowadr = std::mem::take(&mut d.efc_ar_rowadr);
        let y_rownnz = std::mem::take(&mut d.efc_y_rownnz);
        let y_rowadr = std::mem::take(&mut d.efc_y_rowadr);
        let y_colind = std::mem::take(&mut d.efc_y_colind);
        let y_rowsuper = std::mem::take(&mut d.efc_j_rowsuper);
        d.n_a = util_sparse::sqr_mat_tdsparse_symbolic(
            &mut ar_rownnz,
            &mut ar_rowadr,
            None,
            Some(&mut diagind),
            nv,
            nefc,
            &yt_rownnz,
            &yt_rowadr,
            &yt_colind,
            None,
            &y_rownnz,
            &y_rowadr,
            &y_colind,
            Some(&y_rowsuper),
            &mut d.scratch,
        );
        d.efc_ar.resize(d.n_a as usize, 0.0);
        d.efc_ar_colind.resize(d.n_a as usize, 0);
        let mut ar_colind = std::mem::take(&mut d.efc_ar_colind);
        util_sparse::sqr_mat_tdsparse_symbolic(
            &mut ar_rownnz,
            &mut ar_rowadr,
            Some(&mut ar_colind),
            Some(&mut diagind),
            nv,
            nefc,
            &yt_rownnz,
            &yt_rowadr,
            &yt_colind,
            None,
            &y_rownnz,
            &y_rowadr,
            &y_colind,
            Some(&y_rowsuper),
            &mut d.scratch,
        );
        d.efc_ar_rownnz = ar_rownnz;
        d.efc_ar_rowadr = ar_rowadr;
        d.efc_ar_colind = ar_colind;
        d.efc_y_rownnz = y_rownnz;
        d.efc_y_rowadr = y_rowadr;
        d.efc_y_colind = y_colind;
        d.efc_j_rowsuper = y_rowsuper;
        d.scratch.put_int(yt_rownnz);
        d.scratch.put_int(yt_rowadr);
        d.scratch.put_int(yt_colind);
        d.scratch.put_int(diagind);
    } else {
        d.n_a = (nefc * nefc) as i32;
        d.efc_ar.resize(d.n_a as usize, 0.0);
    }
}

fn make_arnumeric(m: &Model, d: &mut Data) {
    let nefc = d.nefc as usize;
    let nv = m.nv;
    if d.efc_ar.is_empty() {
        return;
    }
    if core_util::is_sparse(m) != 0 {
        let mut yt_rownnz = d.scratch.int(nv);
        let mut yt_rowadr = d.scratch.int(nv);
        let mut yt_colind = d.scratch.int(d.n_y as usize);
        let mut yt = d.scratch.num(d.n_y as usize);
        util_sparse::transpose_sparse(
            Some(&mut yt),
            &d.efc_y,
            nefc,
            nv,
            &mut yt_rownnz,
            &mut yt_rowadr,
            &mut yt_colind,
            None,
            &d.efc_y_rownnz,
            &d.efc_y_rowadr,
            &d.efc_y_colind,
        );
        let mut diagind = d.scratch.int(nefc);
        for i in 0..nefc {
            let mut adr = d.efc_ar_rowadr[i];
            let end = adr + d.efc_ar_rownnz[i];
            while adr < end && d.efc_ar_colind[adr as usize] < i as i32 {
                adr += 1;
            }
            diagind[i] = adr;
        }
        let mut ar = std::mem::take(&mut d.efc_ar);
        let ar_rownnz = std::mem::take(&mut d.efc_ar_rownnz);
        let ar_rowadr = std::mem::take(&mut d.efc_ar_rowadr);
        let ar_colind = std::mem::take(&mut d.efc_ar_colind);
        let y = std::mem::take(&mut d.efc_y);
        let y_rownnz = std::mem::take(&mut d.efc_y_rownnz);
        let y_rowadr = std::mem::take(&mut d.efc_y_rowadr);
        let y_colind = std::mem::take(&mut d.efc_y_colind);
        let y_rowsuper = std::mem::take(&mut d.efc_j_rowsuper);
        // the product's three patterns, checked where they are taken
        let ar_p = PatternRef::check(&ar_rownnz, &ar_rowadr, &ar_colind, None, nefc, nefc)
            .expect("make_arnumeric: the AR pattern is malformed");
        let yt_p = PatternRef::check(&yt_rownnz, &yt_rowadr, &yt_colind, None, nv, nefc)
            .expect("make_arnumeric: the Y' pattern is malformed");
        let y_p = PatternRef::check(&y_rownnz, &y_rowadr, &y_colind, Some(&y_rowsuper), nefc, nv)
            .expect("make_arnumeric: the Y pattern is malformed");
        util_sparse::sqr_mat_tdsparse_numeric(&mut ar, ar_p, Some(&diagind), &yt, yt_p, &y, y_p, None, &mut d.scratch);
        d.efc_ar = ar;
        d.efc_ar_rownnz = ar_rownnz;
        d.efc_ar_rowadr = ar_rowadr;
        d.efc_ar_colind = ar_colind;
        d.efc_y = y;
        d.efc_y_rownnz = y_rownnz;
        d.efc_y_rowadr = y_rowadr;
        d.efc_y_colind = y_colind;
        d.efc_j_rowsuper = y_rowsuper;
        for i in 0..nefc {
            d.efc_ar[diagind[i] as usize] += d.efc_r[i];
        }
        d.scratch.put_int(yt_rownnz);
        d.scratch.put_int(yt_rowadr);
        d.scratch.put_int(yt_colind);
        d.scratch.put_num(yt);
        d.scratch.put_int(diagind);
    } else {
        let mut yt = d.scratch.num(nv * nefc);
        util_blas::transpose(&mut yt, &d.efc_y, nefc, nv);
        util_blas::sqr_mat_td(&mut d.efc_ar, &yt, None, nv, nefc);
        for r in 0..nefc {
            d.efc_ar[r * (nefc + 1)] += d.efc_r[r];
        }
        d.scratch.put_num(yt);
    }
}

pub fn project_constraint(m: &Model, d: &mut Data) {
    let nefc = d.nefc;
    if nefc == 0 {
        return;
    }
    let is_dual = core_util::is_dual(m) != 0;
    let diagexact = enabled(m, ENBL_DIAGEXACT);
    if core_util::is_metric(m) != 0 {
        if is_dual || diagexact {
            make_ysymbolic(m, d);
            if is_dual && d.nefc != 0 {
                make_arsymbolic(m, d);
            }
        }
        return;
    }
    if is_dual || diagexact {
        make_ysymbolic(m, d);
        if d.nefc != 0 {
            make_ynumeric(m, d, diagexact as i32);
        }
    }
    if diagexact && d.nefc != 0 {
        make_impedance(m, d);
        if d.nisland != 0 {
            crate::util_misc::gather(
                &mut d.iefc_d,
                &d.efc_d,
                Some(&d.map_iefc2efc),
                d.nefc as usize,
            );
            crate::util_misc::gather(
                &mut d.iefc_r,
                &d.efc_r,
                Some(&d.map_iefc2efc),
                d.nefc as usize,
            );
        }
    }
    if is_dual && d.nefc != 0 {
        make_arsymbolic(m, d);
        if d.nefc != 0 {
            make_arnumeric(m, d);
        }
    }
}

fn add_surface_vel(m: &Model, d: &mut Data) {
    if m.flg_surfacevel == 0 {
        return;
    }
    let ispyramid = core_util::is_pyramidal(m) != 0;
    for i in 0..d.ncon as usize {
        let con = &d.contact[i];
        if con.efc_address < 0 || con.dim <= 1 {
            continue;
        }
        let sv = &con.surfacevel;
        if sv[0] == 0.0 && sv[1] == 0.0 && sv[2] == 0.0 {
            continue;
        }
        let adr = con.efc_address as usize;
        let n = con.dim.min(4) as usize;
        if !ispyramid {
            for k in 1..n {
                d.efc_vel[adr + k] += sv[k - 1];
            }
        } else {
            for k in 1..n {
                let mu = con.friction[k - 1];
                d.efc_vel[adr + 2 * (k - 1)] += mu * sv[k - 1];
                d.efc_vel[adr + 2 * (k - 1) + 1] -= mu * sv[k - 1];
            }
        }
    }
}

fn adhesion_ref(m: &Model, d: &mut Data) {
    if m.flg_adhesion == 0 {
        return;
    }
    let ispyramid = core_util::is_pyramidal(m) != 0;
    for i in 0..d.ncon as usize {
        let con = &d.contact[i];
        if con.adhesion == 0.0 || con.efc_address < 0 {
            continue;
        }
        let adr = con.efc_address as usize;
        let dim = con.dim as usize;
        if dim == 1 || !ispyramid {
            d.efc_aref[adr] += d.efc_r[adr] * con.adhesion;
        } else {
            let edge = con.adhesion / (2 * (dim - 1)) as f64;
            for j in 0..2 * (dim - 1) {
                d.efc_aref[adr + j] += d.efc_r[adr + j] * edge;
            }
        }
    }
}

pub fn velocity_constraint(m: &Model, d: &mut Data) {
    let mut efc_vel = std::mem::take(&mut d.efc_vel);
    mul_jac_vec(m, d, &mut efc_vel, &d.qvel);
    d.efc_vel = efc_vel;
    add_surface_vel(m, d);
}

pub fn reference_constraint(m: &Model, d: &mut Data) {
    let nefc = d.nefc as usize;
    let metric = core_util::is_metric(m) != 0;
    let h = m.opt.timestep;
    velocity_constraint(m, d);
    let shift = if metric { h } else { 0.0 };
    for i in 0..nefc {
        let kbip = &d.efc_kbip[4 * i..];
        d.efc_aref[i] = -kbip[1] * d.efc_vel[i]
            - kbip[0] * kbip[2] * (d.efc_pos[i] - d.efc_margin[i] + shift * d.efc_vel[i]);
    }
    if d.ne > 0 {
        let mut efc_aref = std::mem::take(&mut d.efc_aref);
        jdotv(m, d, &mut efc_aref);
        d.efc_aref = efc_aref;
    }
    if metric {
        for i in 0..nefc {
            d.efc_aref[i] /= implicit_factor(&d.efc_kbip[4 * i..], h);
        }
    }
    adhesion_ref(m, d);
}

/// Where an elliptic solve keeps the cone Hessians (`Contact::h`) it computes: in the contacts themselves, or
/// in a solve's own buffer (`solver::ConeBuf`). A cone is the contact `id` whose rows start at `row`.
pub trait ConeHessians {
    /// The contact a cone belongs to.
    fn contact(&self, id: usize) -> &Contact;
    /// The 36 values the cone's Hessian goes in.
    fn h(&mut self, row: usize, id: usize) -> &mut [f64; 36];
}

impl ConeHessians for [Contact] {
    fn contact(&self, id: usize) -> &Contact {
        &self[id]
    }
    fn h(&mut self, _row: usize, id: usize) -> &mut [f64; 36] {
        &mut self[id].h
    }
}

pub fn constraint_update_impl<C: ConeHessians + ?Sized>(
    ne: usize,
    nf: usize,
    nefc: usize,
    d: &[f64],
    r: &[f64],
    floss: &[f64],
    jar: &[f64],
    r#type: &[i32],
    id: &[i32],
    cones: &mut C,
    state: &mut [i32],
    force: &mut [f64],
    cost: Option<&mut f64>,
    flg_cone_hessian: i32,
) {
    let mut s = 0.0;
    if nefc == 0 {
        if let Some(cost) = cost {
            *cost = 0.0;
        }
        return;
    }
    for i in 0..nefc {
        force[i] = -d[i] * jar[i];
    }
    let mut i = 0;
    while i < nefc {
        if i < ne {
            if cost.is_some() {
                s += 0.5 * d[i] * jar[i] * jar[i];
            }
            state[i] = CNSTRSTATE_QUADRATIC;
            i += 1;
            continue;
        }
        if i < ne + nf {
            if jar[i] <= -r[i] * floss[i] {
                if cost.is_some() {
                    s += -0.5 * r[i] * floss[i] * floss[i] - floss[i] * jar[i];
                }
                force[i] = floss[i];
                state[i] = CNSTRSTATE_LINEARNEG;
            } else if jar[i] >= r[i] * floss[i] {
                if cost.is_some() {
                    s += -0.5 * r[i] * floss[i] * floss[i] + floss[i] * jar[i];
                }
                force[i] = -floss[i];
                state[i] = CNSTRSTATE_LINEARPOS;
            } else {
                if cost.is_some() {
                    s += 0.5 * d[i] * jar[i] * jar[i];
                }
                state[i] = CNSTRSTATE_QUADRATIC;
            }
            i += 1;
            continue;
        }
        if r#type[i] != CNSTR_CONTACT_ELLIPTIC {
            if jar[i] >= 0.0 {
                force[i] = 0.0;
                state[i] = CNSTRSTATE_SATISFIED;
            } else {
                if cost.is_some() {
                    s += 0.5 * d[i] * jar[i] * jar[i];
                }
                state[i] = CNSTRSTATE_QUADRATIC;
            }
        } else {
            let con = cones.contact(id[i] as usize);
            let mu = con.mu;
            let friction = con.friction;
            let dim = con.dim as usize;
            let mut u = [0.0; 6];
            u[0] = jar[i] * mu;
            for j in 1..dim {
                u[j] = jar[i + j] * friction[j - 1];
            }
            let n = u[0];
            let t = util_blas::norm(&u[1..], dim - 1);
            if n >= mu * t || (t <= 0.0 && n >= 0.0) {
                util_blas::zero(&mut force[i..], dim);
                state[i] = CNSTRSTATE_SATISFIED;
            } else if mu * n + t <= 0.0 || (t <= 0.0 && n < 0.0) {
                if cost.is_some() {
                    for j in 0..dim {
                        s += 0.5 * d[i + j] * jar[i + j] * jar[i + j];
                    }
                }
                state[i] = CNSTRSTATE_QUADRATIC;
            } else {
                let dm = d[i] / (mu * mu * (1.0 + mu * mu));
                let nm_t = n - mu * t;
                if cost.is_some() {
                    s += 0.5 * dm * nm_t * nm_t;
                }
                let mut uhat = [0.0; 5];
                for j in 1..dim {
                    uhat[j - 1] = u[j] / t;
                }
                force[i] = -dm * nm_t * mu;
                for j in 1..dim {
                    force[i + j] = -force[i] * uhat[j - 1] * friction[j - 1];
                }
                state[i] = CNSTRSTATE_CONE;
                if flg_cone_hessian != 0 {
                    let h = cones.h(i, id[i] as usize);
                    h[0] = 1.0;
                    for j in 1..dim {
                        h[j] = -mu * uhat[j - 1];
                    }
                    let mut scl = mu * (n / t);
                    for k in 1..dim {
                        for j in k..dim {
                            h[k * dim + j] = scl * uhat[j - 1] * uhat[k - 1];
                        }
                    }
                    scl = mu * mu - scl;
                    for j in 1..dim {
                        h[j * (dim + 1)] += scl;
                    }
                    for k in 0..dim {
                        scl = dm * if k == 0 { mu } else { friction[k - 1] };
                        for j in k..dim {
                            h[k * dim + j] *= scl * if j == 0 { mu } else { friction[j - 1] };
                        }
                    }
                    for k in 0..dim {
                        for j in k + 1..dim {
                            h[j * dim + k] = h[k * dim + j];
                        }
                    }
                }
            }
            for j in 1..dim {
                state[i + j] = state[i];
            }
            i += dim - 1;
        }
        i += 1;
    }
    if let Some(cost) = cost {
        *cost = s;
    }
}

pub fn constraint_update(
    m: &Model,
    d: &mut Data,
    jar: &[f64],
    cost: Option<&mut f64>,
    flg_cone_hessian: i32,
) {
    constraint_update_impl(
        d.ne as usize,
        d.nf as usize,
        d.nefc as usize,
        &d.efc_d,
        &d.efc_r,
        &d.efc_frictionloss,
        jar,
        &d.efc_type,
        &d.efc_id,
        &mut d.contact[..],
        &mut d.efc_state,
        &mut d.efc_force,
        cost,
        flg_cone_hessian,
    );
    let mut qfrc_constraint = std::mem::take(&mut d.qfrc_constraint);
    mul_jac_tvec(m, d, &mut qfrc_constraint, &d.efc_force);
    d.qfrc_constraint = qfrc_constraint;
}
