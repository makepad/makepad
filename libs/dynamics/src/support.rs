//! State access, inertia products, applied forces, and position integration.
//!
//! Translated from `src/engine/engine_support.c` and `engine_support.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::core_util;
use crate::data::Data;
use crate::inline::{at3, at3_mut, at4, at4_mut, at9};
use crate::model::Model;
use crate::types::*;
use crate::{util_blas, util_misc, util_sparse, util_spatial};

pub const DISABLESTRING: [&str; NDISABLE as usize] = [
    "Constraint", "Equality", "Frictionloss", "Limit", "Contact", "Spring", "Damper",
    "Gravity", "Clampctrl", "Warmstart", "Filterparent", "Actuation", "Refsafe", "Sensor",
    "Midphase", "Eulerdamp", "AutoReset", "NativeCCD", "Island", "MultiCCD",
];
pub const ENABLESTRING: [&str; NENABLE as usize] = [
    "Override", "Energy", "Fwdinv", "InvDiscrete", "Sleep", "DiagExact", "IPC", "Speculative",
];
pub const TIMERSTRING: [&str; NTIMER as usize] = [
    "step", "forward", "inverse", "position", "velocity", "actuation", "constraint", "advance",
    "pos_kinematics", "pos_inertia", "pos_collision", "pos_make", "pos_project", "col_broadphase",
    "col_narrowphase",
];
pub const TOPICSTRING: [&str; NTOPIC as usize] = ["Step timing", "Compile timing", "Sleep/wake"];
pub const CONDATA_SIZE: [i32; NCONDATA as usize] = [1, 3, 3, 1, 3, 3, 3, 3, 3];
pub const RAYDATA_SIZE: [i32; NRAYDATA as usize] = [1, 3, 3, 3, 3, 1];

// Return size of a single state element.
fn state_elem_size(m: &Model, sig: i32) -> usize {
    match sig {
        STATE_TIME => 1,
        STATE_QPOS => m.nq,
        STATE_QVEL => m.nv,
        STATE_ACT => m.na,
        STATE_HISTORY => m.nhistory,
        STATE_WARMSTART => m.nv,
        STATE_CTRL => m.nu,
        STATE_QFRC_APPLIED => m.nv,
        STATE_XFRC_APPLIED => 6 * m.nbody,
        STATE_EQ_ACTIVE => m.neq,
        STATE_MOCAP_POS => 3 * m.nmocap,
        STATE_MOCAP_QUAT => 4 * m.nmocap,
        STATE_USERDATA => m.nuserdata,
        STATE_PLUGIN => m.npluginstate,
        STATE_PLASTIC => m.nplastic,
        _ => panic!("invalid state element {}", sig as u32),
    }
}

// Return a mutable slice to a single state element.
fn state_elem_ptr<'a>(_m: &Model, d: &'a mut Data, sig: i32) -> &'a mut [f64] {
    match sig {
        STATE_TIME => std::slice::from_mut(&mut d.time),
        STATE_QPOS => &mut d.qpos,
        STATE_QVEL => &mut d.qvel,
        STATE_ACT => &mut d.act,
        STATE_HISTORY => &mut d.history,
        STATE_WARMSTART => &mut d.qacc_warmstart,
        STATE_CTRL => &mut d.ctrl,
        STATE_QFRC_APPLIED => &mut d.qfrc_applied,
        STATE_XFRC_APPLIED => &mut d.xfrc_applied,
        STATE_MOCAP_POS => &mut d.mocap_pos,
        STATE_MOCAP_QUAT => &mut d.mocap_quat,
        STATE_USERDATA => &mut d.userdata,
        STATE_PLUGIN => &mut d.plugin_state,
        STATE_PLASTIC => &mut d.plastic,
        _ => panic!("invalid state element {}", sig as u32),
    }
}

fn state_elem_const_ptr<'a>(_m: &Model, d: &'a Data, sig: i32) -> &'a [f64] {
    // The C const-cast wrapper uses the same field selection as state_elem_ptr.
    match sig {
        STATE_TIME => std::slice::from_ref(&d.time),
        STATE_QPOS => &d.qpos,
        STATE_QVEL => &d.qvel,
        STATE_ACT => &d.act,
        STATE_HISTORY => &d.history,
        STATE_WARMSTART => &d.qacc_warmstart,
        STATE_CTRL => &d.ctrl,
        STATE_QFRC_APPLIED => &d.qfrc_applied,
        STATE_XFRC_APPLIED => &d.xfrc_applied,
        STATE_MOCAP_POS => &d.mocap_pos,
        STATE_MOCAP_QUAT => &d.mocap_quat,
        STATE_USERDATA => &d.userdata,
        STATE_PLUGIN => &d.plugin_state,
        STATE_PLASTIC => &d.plastic,
        _ => panic!("invalid state element {}", sig as u32),
    }
}

pub fn state_size(m: &Model, sig: i32) -> usize {
    if sig < 0 {
        panic!("invalid state signature {} < 0", sig);
    }
    if sig >= (1 << NSTATE) {
        panic!("invalid state signature {} >= 2^mjNSTATE", sig);
    }
    let mut size = 0;
    for i in 0..NSTATE {
        let element = 1 << i;
        if element & sig != 0 {
            size += state_elem_size(m, element);
        }
    }
    size
}

pub fn get_state(m: &Model, d: &Data, state: &mut [f64], sig: i32) {
    if sig < 0 {
        panic!("invalid state signature {} < 0", sig);
    }
    if sig >= (1 << NSTATE) {
        panic!("invalid state signature {} >= 2^mjNSTATE", sig);
    }
    let mut adr = 0;
    for i in 0..NSTATE {
        let element = 1 << i;
        if element & sig != 0 {
            let size = state_elem_size(m, element);
            if element == STATE_EQ_ACTIVE {
                let neq = m.neq;
                for j in 0..neq {
                    state[adr] = d.eq_active[j] as f64;
                    adr += 1;
                }
            } else {
                let ptr = state_elem_const_ptr(m, d, element);
                util_blas::copy(&mut state[adr..], ptr, size);
                adr += size;
            }
        }
    }
}

pub fn extract_state(m: &Model, mut src: &[f64], srcsig: i32, mut dst: &mut [f64], dstsig: i32) {
    if srcsig < 0 {
        panic!("invalid srcsig {} < 0", srcsig);
    }
    if srcsig >= (1 << NSTATE) {
        panic!("invalid srcsig {} >= 2^mjNSTATE", srcsig);
    }
    if (srcsig & dstsig) != dstsig {
        panic!("dstsig is not a subset of srcsig");
    }
    for i in 0..NSTATE {
        let element = 1 << i;
        if element & srcsig != 0 {
            let size = state_elem_size(m, element);
            if element & dstsig != 0 {
                util_blas::copy(dst, src, size);
                dst = &mut dst[size..];
            }
            src = &src[size..];
        }
    }
}

pub fn set_state(m: &Model, d: &mut Data, state: &[f64], sig: i32) {
    if sig < 0 {
        panic!("invalid state signature {} < 0", sig);
    }
    if sig >= (1 << NSTATE) {
        panic!("invalid state signature {} >= 2^mjNSTATE", sig);
    }
    let mut adr = 0;
    for i in 0..NSTATE {
        let element = 1 << i;
        if element & sig != 0 {
            let size = state_elem_size(m, element);
            if element == STATE_EQ_ACTIVE {
                let neq = m.neq;
                for j in 0..neq {
                    // C mjtBool assignment converts any nonzero value to true.
                    d.eq_active[j] = u8::from(state[adr] != 0.0);
                    adr += 1;
                }
            } else {
                let ptr = state_elem_ptr(m, d, element);
                util_blas::copy(ptr, &state[adr..], size);
                adr += size;
                if element == STATE_PLASTIC && !d.flex_hessian_valid.is_empty() {
                    for f in 0..m.nflex {
                        if m.flex_plasticadr[f] >= 0 {
                            d.flex_hessian_valid[f] = 0;
                        }
                    }
                }
            }
        }
    }
}

pub fn copy_state(m: &Model, src: &Data, dst: &mut Data, sig: i32) {
    if sig < 0 {
        panic!("invalid state signature {} < 0", sig);
    }
    if sig >= (1 << NSTATE) {
        panic!("invalid state signature {} >= 2^mjNSTATE", sig);
    }
    for i in 0..NSTATE {
        let element = 1 << i;
        if element & sig != 0 {
            let size = state_elem_size(m, element);
            if element == STATE_EQ_ACTIVE {
                let neq = m.neq;
                for j in 0..neq {
                    dst.eq_active[j] = src.eq_active[j];
                }
            } else {
                let dst_ptr = state_elem_ptr(m, dst, element);
                let src_ptr = state_elem_const_ptr(m, src, element);
                util_blas::copy(dst_ptr, src_ptr, size);
                if element == STATE_PLASTIC && !dst.flex_hessian_valid.is_empty() {
                    for f in 0..m.nflex {
                        if m.flex_plasticadr[f] >= 0 {
                            dst.flex_hessian_valid[f] = 0;
                        }
                    }
                }
            }
        }
    }
}

pub fn set_keyframe(m: &mut Model, d: &Data, k: i32) {
    if k as i64 >= m.nkey as i64 {
        panic!("index must be smaller than {} (keyframes allocated in model)", m.nkey);
    }
    if k < 0 {
        panic!("keyframe index cannot be negative");
    }
    let k = k as usize;
    m.key_time[k] = d.time;
    util_blas::copy(&mut m.key_qpos[k * m.nq..], &d.qpos, m.nq);
    util_blas::copy(&mut m.key_qvel[k * m.nv..], &d.qvel, m.nv);
    util_blas::copy(&mut m.key_act[k * m.na..], &d.act, m.na);
    util_blas::copy(&mut m.key_mpos[k * 3 * m.nmocap..], &d.mocap_pos, 3 * m.nmocap);
    util_blas::copy(&mut m.key_mquat[k * 4 * m.nmocap..], &d.mocap_quat, 4 * m.nmocap);
    util_blas::copy(&mut m.key_ctrl[k * m.nu..], &d.ctrl, m.nu);
    util_blas::copy(&mut m.key_plastic[k * m.nplastic..], &d.plastic, m.nplastic);
}

pub fn full_m(m: &Model, d: &Data, dst: &mut [f64]) {
    util_sparse::sym2dense(dst, &d.m, m.nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind);
}

pub fn mul_m(m: &Model, d: &Data, res: &mut [f64], vec: &[f64]) {
    util_sparse::mul_sym_vec_sparse(res, &d.m, vec, m.nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind);
}

pub fn mul_m2(m: &Model, d: &Data, res: &mut [f64], vec: &[f64]) {
    let nv = m.nv;
    let q_ld = &d.q_ld;
    util_blas::zero(res, nv);
    // res = L * vec.
    for i in 0..nv {
        res[i] = vec[i];
        if m.dof_simplenum[i] == 0 {
            let adr = m.m_rowadr[i] as usize;
            res[i] += util_sparse::dot_sparse(&q_ld[adr..], vec, (m.m_rownnz[i] - 1) as usize, &m.m_colind[adr..]);
        }
    }
    // res *= sqrt(D).
    for i in 0..nv {
        let diag = (m.m_rowadr[i] + m.m_rownnz[i] - 1) as usize;
        res[i] *= q_ld[diag].sqrt();
    }
}

pub fn add_m(m: &Model, d: &mut Data, dst: &mut [f64], rownnz: Option<&mut [i32]>, rowadr: Option<&mut [i32]>, colind: Option<&mut [i32]>) {
    let nv = m.nv;
    if let (Some(rownnz), Some(rowadr), Some(colind)) = (rownnz, rowadr, colind) {
        util_sparse::add_to_mat_sparse(dst, rownnz, rowadr, colind, nv, &d.m, &m.m_rownnz, &m.m_rowadr, &m.m_colind);
    } else {
        util_sparse::add_to_sym_sparse(dst, &d.m, nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind, 0);
    }
}

pub fn apply_ft(m: &Model, d: &mut Data, force: Option<&[f64; 3]>, torque: Option<&[f64; 3]>, point: &[f64; 3], body: i32, qfrc_target: &mut [f64]) {
    let nv = m.nv;
    let mut jacp = force.map(|_| d.scratch.num(3 * nv));
    let mut jacr = torque.map(|_| d.scratch.num(3 * nv));
    let mut qforce = d.scratch.num(nv);
    if body < 0 || body as usize >= m.nbody {
        panic!("invalid body {}", body);
    }
    if core_util::is_sparse(m) != 0 {
        let mut chain = d.scratch.int(nv);
        // C's NV clashes with nv after snake-casing, so it takes the _mat suffix.
        let nv_mat = core_util::body_chain(m, body as usize, &mut chain);
        core_util::jac_sparse(m, d, jacp.as_deref_mut(), jacr.as_deref_mut(), point, body as usize, nv_mat, &chain, 0);
        if let Some(force) = force {
            util_blas::mul_mat_tvec(&mut qforce, jacp.as_deref().unwrap(), force, 3, nv_mat);
            for i in 0..nv_mat {
                qfrc_target[chain[i] as usize] += qforce[i];
            }
        }
        if let Some(torque) = torque {
            util_blas::mul_mat_tvec(&mut qforce, jacr.as_deref().unwrap(), torque, 3, nv_mat);
            for i in 0..nv_mat {
                qfrc_target[chain[i] as usize] += qforce[i];
            }
        }
        d.scratch.put_int(chain);
    } else {
        core_util::jac(m, d, jacp.as_deref_mut(), jacr.as_deref_mut(), point, body as usize);
        if let Some(force) = force {
            util_blas::mul_mat_tvec(&mut qforce, jacp.as_deref().unwrap(), force, 3, nv);
            util_blas::add_to(qfrc_target, &qforce, nv);
        }
        if let Some(torque) = torque {
            util_blas::mul_mat_tvec(&mut qforce, jacr.as_deref().unwrap(), torque, 3, nv);
            util_blas::add_to(qfrc_target, &qforce, nv);
        }
    }
    if let Some(jacp) = jacp {
        d.scratch.put_num(jacp);
    }
    if let Some(jacr) = jacr {
        d.scratch.put_num(jacr);
    }
    d.scratch.put_num(qforce);
}

pub fn xfrc_accumulate(m: &Model, d: &mut Data, qfrc: &mut [f64]) {
    let nbody = m.nbody;
    let xfrc = &d.xfrc_applied;
    // Preserve the C bytewise quick return (negative zero has nonzero bytes).
    // SAFETY: these initialized f64 values may be read as bytes for their exact size.
    let bytes = unsafe {
        std::slice::from_raw_parts(xfrc[6..6 * nbody].as_ptr().cast::<u8>(), 6 * (nbody - 1) * std::mem::size_of::<f64>())
    };
    if util_misc::is_zero_byte(bytes, bytes.len()) != 0 {
        return;
    }
    for i in 1..nbody {
        if util_misc::is_zero(&d.xfrc_applied[6 * i..], 6) == 0 {
            let force = *at3(&d.xfrc_applied, 6 * i);
            let torque = *at3(&d.xfrc_applied, 6 * i + 3);
            let point = *at3(&d.xipos, 3 * i);
            apply_ft(m, d, Some(&force), Some(&torque), &point, i as i32, qfrc);
        }
    }
}

// Returns the smallest distance between two geoms using native CCD.
fn geom_distance_ccd(m: &Model, d: &mut Data, g1: i32, g2: i32, distmax: f64,
    fromto: Option<&mut [f64; 6]>) -> f64 {
    use crate::collision_convex::init_ccd_obj;
    use crate::collision_gjk::{ccd, ccd_size, CcdConfig, CcdObj, CcdStatus};

    let size = ccd_size(0, 0, m.opt.ccd_iterations as usize);
    let mut buffer = d.scratch.ccd_buffer(size);
    let mut config = CcdConfig {
        max_iterations: m.opt.ccd_iterations as usize,
        tolerance: m.opt.ccd_tolerance,
        npolygonmax: 0,
        nmeshdegmax: 0,
        max_contacts: 1,
        dist_cutoff: distmax,
        buffer: Some(&mut buffer),
    };
    let mut status = CcdStatus::default();
    let mut obj1 = CcdObj::default();
    let mut obj2 = CcdObj::default();
    init_ccd_obj(&mut obj1, m, Some(d), g1, 0.0);
    init_ccd_obj(&mut obj2, m, Some(d), g2, 0.0);
    let dist = ccd(&mut config, &mut status, &mut obj1, &mut obj2);
    d.scratch.put_ccd_buffer(buffer, size);
    if let Some(fromto) = fromto {
        if status.nx > 0 {
            util_blas::copy3(at3_mut(fromto, 0), at3(&status.x1, 0));
            util_blas::copy3(at3_mut(fromto, 3), at3(&status.x2, 0));
        }
    }
    if dist < distmax { dist } else { distmax }
}

pub fn geom_distance(m: &Model, d: &mut Data, geom1: i32, geom2: i32, distmax: f64,
    mut fromto: Option<&mut [f64; 6]>) -> f64 {
    use crate::collision_driver::{CollisionFn, COLLISIONFUNC};

    let mut con = d.scratch.pre_contact(MAXCONPAIR);
    let mut dist = distmax;
    if let Some(fromto) = fromto.as_deref_mut() { util_blas::zero(fromto, 6); }
    let flip = m.geom_type[geom1 as usize] > m.geom_type[geom2 as usize];
    let g1 = if flip { geom2 } else { geom1 };
    let g2 = if flip { geom1 } else { geom2 };
    let type1 = m.geom_type[g1 as usize];
    let type2 = m.geom_type[g2 as usize];
    let Some(func) = COLLISIONFUNC[type1 as usize][type2 as usize] else {
        d.scratch.put_pre_contact(con);
        return dist;
    };
    if m.opt.disableflags & DSBL_NATIVECCD == 0 {
        if func == CollisionFn::Convex || func == CollisionFn::BoxBox {
            let dist = geom_distance_ccd(m, d, geom1, geom2, distmax, fromto);
            d.scratch.put_pre_contact(con);
            return dist;
        }
    }
    let num = func.call(m, d, &mut con, g1, g2, distmax);
    let mut smallest = -1;
    for i in 0..num {
        let dist_i = con[i].dist;
        if dist_i < dist || (dist_i == dist && smallest < 0) {
            dist = dist_i;
            smallest = i as i32;
        }
    }
    if let Some(fromto) = fromto {
        if smallest >= 0 {
            let sign = if flip { -1.0 } else { 1.0 };
            let smallest = smallest as usize;
            util_blas::add_scl3(at3_mut(fromto, 0), &con[smallest].pos, &con[smallest].normal, -0.5*sign*dist);
            util_blas::add_scl3(at3_mut(fromto, 3), &con[smallest].pos, &con[smallest].normal, 0.5*sign*dist);
        }
    }
    d.scratch.put_pre_contact(con);
    dist
}

pub fn inside_site(m: &Model, d: &Data, siteid: usize, point: &[f64; 3]) -> i32 {
    let r#type = m.site_type[siteid];
    let pos = at3(&d.site_xpos, 3 * siteid);
    let mat = at9(&d.site_xmat, 9 * siteid);
    let size = at3(&m.site_size, 3 * siteid);
    if r#type == GEOM_MESH {
        let meshid = m.site_dataid[siteid];
        if meshid < 0 {
            return 0;
        }
        let meshid = meshid as usize;
        let pn = m.mesh_polynum[meshid];
        if pn <= 0 {
            return 0;
        }
        let mut vec = [0.0; 3];
        let mut plocal = [0.0; 3];
        let mut meshmat = [0.0; 9];
        util_blas::sub3(&mut vec, point, pos);
        util_blas::mul_mat_tvec3(&mut plocal, mat, &vec);
        util_blas::sub3(&mut vec, &plocal, at3(&m.mesh_pos, 3 * meshid));
        util_spatial::quat2mat(&mut meshmat, at4(&m.mesh_quat, 4 * meshid));
        util_blas::mul_mat_tvec3(&mut plocal, &meshmat, &vec);
        if plocal[0].abs() > size[0] || plocal[1].abs() > size[1] || plocal[2].abs() > size[2] {
            return 0;
        }
        let vbase = &m.mesh_vert[3 * m.mesh_vertadr[meshid] as usize..];
        let polyadr = m.mesh_polyadr[meshid] as usize;
        for p in 0..pn as usize {
            let pnl = at3(&m.mesh_polynormal, 3 * (polyadr + p));
            let v0 = &vbase[3 * m.mesh_polyvert[m.mesh_polyvertadr[polyadr + p] as usize] as usize..];
            let c = pnl[0] * v0[0] as f64 + pnl[1] * v0[1] as f64 + pnl[2] * v0[2] as f64;
            let dd = pnl[0] * plocal[0] + pnl[1] * plocal[1] + pnl[2] * plocal[2] - c;
            if dd > 0.0 {
                return 0;
            }
        }
        return 1;
    }
    util_misc::inside_primitive(pos, mat, size, r#type, point)
}

pub fn differentiate_pos(m: &Model, qvel: &mut [f64], dt: f64, qpos1: &[f64], qpos2: &[f64]) {
    for j in 0..m.njnt {
        let mut padr = m.jnt_qposadr[j] as usize;
        let mut vadr = m.jnt_dofadr[j] as usize;
        match m.jnt_type[j] {
            JNT_FREE | JNT_BALL => {
                if m.jnt_type[j] == JNT_FREE {
                    for i in 0..3 {
                        qvel[vadr + i] = (qpos2[padr + i] - qpos1[padr + i]) / dt;
                    }
                    vadr += 3;
                    padr += 3;
                }
                // C's free joint falls through to the ball joint rotation.
                util_spatial::sub_quat(at3_mut(qvel, vadr), at4(qpos2, padr), at4(qpos1, padr));
                let vel = *at3(qvel, vadr);
                util_blas::scl3(at3_mut(qvel, vadr), &vel, 1.0 / dt);
            }
            JNT_HINGE | JNT_SLIDE => {
                qvel[vadr] = (qpos2[padr] - qpos1[padr]) / dt;
            }
            _ => {}
        }
    }
}

pub fn integrate_pos_ind(m: &Model, qpos: &mut [f64], qvel: &[f64], dt: f64, index: Option<&[i32]>, nbody: usize) {
    for b in 1..nbody {
        let k = index.map_or(b, |index| index[b] as usize);
        let start = m.body_jntadr[k];
        let end = start + m.body_jntnum[k];
        for j in start..end {
            let j = j as usize;
            let mut padr = m.jnt_qposadr[j] as usize;
            let mut vadr = m.jnt_dofadr[j] as usize;
            match m.jnt_type[j] {
                JNT_FREE | JNT_BALL => {
                    if m.jnt_type[j] == JNT_FREE {
                        for i in 0..3 {
                            qpos[padr + i] += dt * qvel[vadr + i];
                        }
                        padr += 3;
                        vadr += 3;
                    }
                    // C's free joint falls through to the ball joint rotation.
                    util_spatial::quat_integrate(at4_mut(qpos, padr), at3(qvel, vadr), dt);
                }
                JNT_HINGE | JNT_SLIDE => {
                    qpos[padr] += dt * qvel[vadr];
                }
                _ => {}
            }
        }
    }
}

pub fn integrate_pos(m: &Model, qpos: &mut [f64], qvel: &[f64], dt: f64) {
    integrate_pos_ind(m, qpos, qvel, dt, None, m.nbody);
}

pub fn normalize_quat(m: &Model, qpos: &mut [f64]) {
    for i in 0..m.njnt {
        if m.jnt_type[i] == JNT_BALL || m.jnt_type[i] == JNT_FREE {
            util_blas::normalize4(at4_mut(qpos, m.jnt_qposadr[i] as usize + 3 * usize::from(m.jnt_type[i] == JNT_FREE)));
        }
    }
}

pub fn actuator_disabled(m: &Model, i: usize) -> i32 {
    let group = m.actuator_group[i];
    if group < 0 || group > 30 {
        0
    } else {
        i32::from(m.opt.disableactuator & (1 << group) != 0)
    }
}

pub fn next_activation(m: &Model, d: &Data, actuator_id: usize, act_adr: usize, act_dot: f64) -> f64 {
    let mut act = d.act[act_adr];
    let dyntype = m.actuator_dyntype[actuator_id];
    if dyntype == DYN_FILTEREXACT {
        let tau = util_misc::max(MINVAL, m.actuator_dynprm[actuator_id * NDYN]);
        act = act + act_dot * tau * (1.0 - (-m.opt.timestep / tau).exp());
    } else if dyntype == DYN_DCMOTOR {
        let dynprm = &m.actuator_dynprm[actuator_id * NDYN..];
        let gainprm = &m.actuator_gainprm[actuator_id * NGAIN..];
        let slots = util_misc::dcmotor_slots(dynprm, gainprm);
        let offset = act_adr as i32 - m.actuator_actadr[actuator_id];
        if offset == slots.current {
            let te = util_misc::max(MINVAL, dynprm[0]);
            act = act + act_dot * te * (1.0 - (-m.opt.timestep / te).exp());
        } else if offset == slots.bristle {
            let biasprm = &m.actuator_biasprm[NBIAS * actuator_id..];
            let f_c = biasprm[3];
            let f_s = biasprm[4];
            let v_s = biasprm[5];
            let sigma0 = dynprm[5];
            let velocity = d.actuator_velocity[m.actuator_outadr[actuator_id] as usize];
            let g = util_misc::lugre_stribeck(velocity, f_c, f_s, v_s);
            let a = -sigma0 * velocity.abs() / util_misc::max(MINVAL, g);
            let h = m.opt.timestep;
            let exp_ah = (a * h).exp();
            let int_h = if a.abs() > MINVAL { (exp_ah - 1.0) / a } else { h };
            act = exp_ah * act + int_h * velocity;
        } else if offset == slots.integral {
            act = act + act_dot * m.opt.timestep;
            let imax = dynprm[8];
            if imax > 0.0 {
                act = util_misc::clip(act, -imax, imax);
            }
        } else {
            act = act + act_dot * m.opt.timestep;
        }
    } else {
        act = act + act_dot * m.opt.timestep;
    }
    if dyntype != DYN_DCMOTOR && m.actuator_actlimited[actuator_id] != 0 {
        let actrange = &m.actuator_actrange[2 * actuator_id..];
        act = util_misc::clip(act, actrange[0], actrange[1]);
    }
    act
}

pub fn get_totalmass(m: &Model) -> f64 {
    let mut res = 0.0;
    for i in 1..m.nbody {
        res += m.body_mass[i];
    }
    res
}

pub fn set_totalmass(m: &mut Model, newmass: f64) {
    let scale = util_misc::max(MINVAL, newmass / util_misc::max(MINVAL, get_totalmass(m)));
    for i in 1..m.nbody {
        m.body_mass[i] *= scale;
        m.body_inertia[3 * i] *= scale;
        m.body_inertia[3 * i + 1] *= scale;
        m.body_inertia[3 * i + 2] *= scale;
    }
    // Don't forget to call set0 after changing masses.
}

pub fn version() -> i32 {
    VERSION_HEADER as i32
}

// mj_versionString: omitted as requested; version() returns VERSION_HEADER.

pub fn condata_size(dataspec: i32) -> i32 {
    let mut size = 0;
    for i in 0..NCONDATA as usize {
        if dataspec & (1 << i) != 0 {
            size += CONDATA_SIZE[i];
        }
    }
    size
}

pub fn raydata_size(dataspec: i32) -> i32 {
    let mut size = 0;
    for i in 0..NRAYDATA as usize {
        if dataspec & (1 << i) != 0 {
            size += RAYDATA_SIZE[i];
        }
    }
    size
}

// mju_camIntrinsics: omitted as requested (camera support).
// mj_readCtrl: omitted as requested (history).
// mj_readSensor: omitted as requested (history).
// mj_initCtrlHistory: omitted as requested (history).
// mj_initSensorHistory: omitted as requested (history).
