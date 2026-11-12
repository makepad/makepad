//! Analytic derivatives of smooth forces and free-body implicit blocks.
//!
//! Translated from `src/engine/engine_derivative.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::data::Data;
use crate::memory::{stack_bytes, stack_frame_bytes};
use crate::model::Model;
use crate::inline::{self, at3, at3_mut, at6, at9, at10};
use crate::types::*;
use crate::{core_util, passive, support, util_blas as blas, util_spatial as spatial};
use crate::util_misc::{max, min, clip, muscle_gain_length, dcmotor_slots, d_x_poly_force};
use crate::util_sparse::{add_to_scl_sparse_inc, zero_sparse};

// Derivatives of the cross product, with optional 3x3 outputs.
fn d_cross(a: &[f64; 3], b: &[f64; 3], da: Option<&mut [f64; 9]>, db: Option<&mut [f64; 9]>) {
    if let Some(da) = da {
        blas::zero(da, 9);
        da[1] = b[2]; da[2] = -b[1]; da[3] = -b[2];
        da[5] = b[0]; da[6] = b[1]; da[7] = -b[0];
    }
    if let Some(db) = db {
        blas::zero(db, 9);
        db[1] = -a[2]; db[2] = a[1]; db[3] = a[2];
        db[5] = -a[0]; db[6] = -a[1]; db[7] = a[0];
    }
}

fn d_cross_motion_vel(d: &mut [f64; 36], v: &[f64; 6]) {
  blas::zero(d, 36);

  // res[0] = -vel[2]*v[1] + vel[1]*v[2]
  d[0 + 2] = -v[1];
  d[0 + 1] = v[2];

  // res[1] =  vel[2]*v[0] - vel[0]*v[2]
  d[6 + 2] = v[0];
  d[6 + 0] = -v[2];

  // res[2] = -vel[1]*v[0] + vel[0]*v[1]
  d[12 + 1] = -v[0];
  d[12 + 0] = v[1];

  // res[3] = -vel[2]*v[4] + vel[1]*v[5] - vel[5]*v[1] + vel[4]*v[2]
  d[18 + 2] = -v[4];
  d[18 + 1] = v[5];
  d[18 + 5] = -v[1];
  d[18 + 4] = v[2];

  // res[4] =  vel[2]*v[3] - vel[0]*v[5] + vel[5]*v[0] - vel[3]*v[2]
  d[24 + 2] = v[3];
  d[24 + 0] = -v[5];
  d[24 + 5] = v[0];
  d[24 + 3] = -v[2];

  // res[5] = -vel[1]*v[3] + vel[0]*v[4] - vel[4]*v[0] + vel[3]*v[1]
  d[30 + 1] = -v[3];
  d[30 + 0] = v[4];
  d[30 + 4] = -v[0];
  d[30 + 3] = v[1];
}

fn d_cross_force_vel(d: &mut [f64; 36], f: &[f64; 6]) {
  blas::zero(d, 36);

  // res[0] = -vel[2]*f[1] + vel[1]*f[2] - vel[5]*f[4] + vel[4]*f[5]
  d[0 + 2] = -f[1];
  d[0 + 1] = f[2];
  d[0 + 5] = -f[4];
  d[0 + 4] = f[5];

  // res[1] =  vel[2]*f[0] - vel[0]*f[2] + vel[5]*f[3] - vel[3]*f[5]
  d[6 + 2] = f[0];
  d[6 + 0] = -f[2];
  d[6 + 5] = f[3];
  d[6 + 3] = -f[5];

  // res[2] = -vel[1]*f[0] + vel[0]*f[1] - vel[4]*f[3] + vel[3]*f[4]
  d[12 + 1] = -f[0];
  d[12 + 0] = f[1];
  d[12 + 4] = -f[3];
  d[12 + 3] = f[4];

  // res[3] = -vel[2]*f[4] + vel[1]*f[5]
  d[18 + 2] = -f[4];
  d[18 + 1] = f[5];

  // res[4] =  vel[2]*f[3] - vel[0]*f[5]
  d[24 + 2] = f[3];
  d[24 + 0] = -f[5];

  // res[5] = -vel[1]*f[3] + vel[0]*f[4]
  d[30 + 1] = -f[3];
  d[30 + 0] = f[4];
}

fn d_cross_force_frc(d: &mut [f64; 36], vel: &[f64; 6]) {
  blas::zero(d, 36);

  // res[0] = -vel[2]*f[1] + vel[1]*f[2] - vel[5]*f[4] + vel[4]*f[5]
  d[0 + 1] = -vel[2];
  d[0 + 2] = vel[1];
  d[0 + 4] = -vel[5];
  d[0 + 5] = vel[4];

  // res[1] =  vel[2]*f[0] - vel[0]*f[2] + vel[5]*f[3] - vel[3]*f[5]
  d[6 + 0] = vel[2];
  d[6 + 2] = -vel[0];
  d[6 + 3] = vel[5];
  d[6 + 5] = -vel[3];

  // res[2] = -vel[1]*f[0] + vel[0]*f[1] - vel[4]*f[3] + vel[3]*f[4]
  d[12 + 0] = -vel[1];
  d[12 + 1] = vel[0];
  d[12 + 3] = -vel[4];
  d[12 + 4] = vel[3];

  // res[3] = -vel[2]*f[4] + vel[1]*f[5]
  d[18 + 4] = -vel[2];
  d[18 + 5] = vel[1];

  // res[4] =  vel[2]*f[3] - vel[0]*f[5]
  d[24 + 3] = vel[2];
  d[24 + 5] = -vel[0];

  // res[5] = -vel[1]*f[3] + vel[0]*f[4]
  d[30 + 3] = -vel[1];
  d[30 + 4] = vel[0];
}

fn d_mul_inert_vec_vel(d: &mut [f64; 36], i: &[f64; 10]) {
  blas::zero(d, 36);

  // res[0] = i[0]*v[0] + i[3]*v[1] + i[4]*v[2] - i[8]*v[4] + i[7]*v[5]
  d[0 + 0] = i[0];
  d[0 + 1] = i[3];
  d[0 + 2] = i[4];
  d[0 + 4] = -i[8];
  d[0 + 5] = i[7];

  // res[1] = i[3]*v[0] + i[1]*v[1] + i[5]*v[2] + i[8]*v[3] - i[6]*v[5]
  d[6 + 0] = i[3];
  d[6 + 1] = i[1];
  d[6 + 2] = i[5];
  d[6 + 3] = i[8];
  d[6 + 5] = -i[6];

  // res[2] = i[4]*v[0] + i[5]*v[1] + i[2]*v[2] - i[7]*v[3] + i[6]*v[4]
  d[12 + 0] = i[4];
  d[12 + 1] = i[5];
  d[12 + 2] = i[2];
  d[12 + 3] = -i[7];
  d[12 + 4] = i[6];

  // res[3] = i[8]*v[1] - i[7]*v[2] + i[9]*v[3]
  d[18 + 1] = i[8];
  d[18 + 2] = -i[7];
  d[18 + 3] = i[9];

  // res[4] = i[6]*v[2] - i[8]*v[0] + i[9]*v[4]
  d[24 + 2] = i[6];
  d[24 + 0] = -i[8];
  d[24 + 4] = i[9];

  // res[5] = i[7]*v[0] - i[6]*v[1] + i[9]*v[5]
  d[30 + 0] = i[7];
  d[30 + 1] = -i[6];
  d[30 + 5] = i[9];
}

pub fn d_sub_quat(qa: &[f64; 4], qb: &[f64; 4], da: Option<&mut [f64; 9]>, db: Option<&mut [f64; 9]>) {
    if da.is_none() && db.is_none() { return; }
    let mut axis = [0.0; 3];
    spatial::sub_quat(&mut axis, qa, qb);
    let half_angle = 0.5 * blas::normalize3(&mut axis);
    let mut da_tmp = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let k = [0.0, -axis[2], axis[1], axis[2], 0.0, -axis[0], -axis[1], axis[0], 0.0];
    blas::add_to_scl(&mut da_tmp, &k, half_angle, 9);
    let mut kk = [0.0; 9];
    inline::mul_mat_mat3(&mut kk, &k, &k);
    let coef = 1.0 - if half_angle < 6e-8 { 1.0 } else { half_angle / half_angle.tan() };
    blas::add_to_scl(&mut da_tmp, &kk, coef, 9);
    if let Some(da) = da { blas::copy9(da, &da_tmp); }
    if let Some(db) = db {
        blas::transpose(db, &da_tmp, 3, 3);
        let input = *db;
        blas::scl(db, &input, -1.0, 9);
    }
}

pub fn d_quat_integrate(vel: &[f64; 3], scale: f64, mut dquat: Option<&mut [f64; 9]>, mut dvel: Option<&mut [f64; 9]>, dscale: Option<&mut [f64; 3]>) {
    let s = [scale * vel[0], scale * vel[1], scale * vel[2]];
    let eye = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let cross = [0.0, s[2], -s[1], -s[2], 0.0, s[0], s[1], -s[0], 0.0];
    let outer = [s[0]*s[0], s[0]*s[1], s[0]*s[2], s[1]*s[0], s[1]*s[1], s[1]*s[2], s[2]*s[0], s[2]*s[1], s[2]*s[2]];
    let xx = blas::dot3(&s, &s);
    let x = xx.sqrt();
    let a = x.cos();
    let (b, c, d);
    if x.abs() > 1.0/32.0 {
        b = x.sin() / x;
        c = (1.0 - a) / xx;
        d = (1.0 - b) / xx;
    } else {
        b = 1.0 + xx/6.0 * (xx/20.0 * (1.0 - xx/42.0) - 1.0);
        c = (1.0 + xx/12.0 * (xx/30.0 * (1.0 - xx/56.0) - 1.0)) / 2.0;
        d = (1.0 + xx/20.0 * (xx/42.0 * (1.0 - xx/72.0) - 1.0)) / 6.0;
    }
    let mut dvel_ = [0.0; 9];
    for i in 0..9 {
        if let Some(dquat) = dquat.as_deref_mut() { dquat[i] = a*eye[i] + b*cross[i] + c*outer[i]; }
        if dvel.is_some() || dscale.is_some() { dvel_[i] = b*eye[i] + c*cross[i] + d*outer[i]; }
    }
    if let Some(dvel) = dvel.as_deref_mut() { blas::copy9(dvel, &dvel_); }
    if let Some(dscale) = dscale { inline::mul_mat_vec3(dscale, &dvel_, vel); }
}

// Dense derivatives of component functions (retained for parity/tests).
fn d_com_vel_vel_dense(m: &Model, d: &Data, dcvel: &mut [f64], dcdofdot: &mut [f64]) {
    let nv = m.nv;
    let nbody = m.nbody;
    let mut mat = [0.0; 36];
    blas::zero(dcvel, nbody*6*nv);
    for i in 1..nbody {
        let src = m.body_parentid[i] as usize*6*nv;
        dcvel.copy_within(src..src+6*nv, i*6*nv);
        let mut j = m.body_dofadr[i] as usize;
        while j < (m.body_dofadr[i] + m.body_dofnum[i]) as usize {
            let joint_type = m.jnt_type[m.dof_jntid[j] as usize];
            if joint_type == JNT_FREE {
                blas::zero(&mut dcdofdot[j*6*nv..], 18*nv);
                for k in 0..6 {
                    dcvel[i*6*nv+k*nv+j] += d.cdof[j*6+k];
                    dcvel[i*6*nv+k*nv+j+1] += d.cdof[(j+1)*6+k];
                    dcvel[i*6*nv+k*nv+j+2] += d.cdof[(j+2)*6+k];
                }
                j += 3;
            }
            if joint_type == JNT_FREE || joint_type == JNT_BALL {
                for k in 0..3 {
                    d_cross_motion_vel(&mut mat, at6(&d.cdof, 6*(j+k)));
                    blas::mul_mat_mat(&mut dcdofdot[(j+k)*6*nv..], &mat, &dcvel[i*6*nv..], 6, 6, nv);
                }
                for k in 0..6 {
                    dcvel[i*6*nv+k*nv+j] += d.cdof[j*6+k];
                    dcvel[i*6*nv+k*nv+j+1] += d.cdof[(j+1)*6+k];
                    dcvel[i*6*nv+k*nv+j+2] += d.cdof[(j+2)*6+k];
                }
                j += 2;
            } else {
                d_cross_motion_vel(&mut mat, at6(&d.cdof, 6*j));
                blas::mul_mat_mat(&mut dcdofdot[j*6*nv..], &mat, &dcvel[i*6*nv..], 6, 6, nv);
                for k in 0..6 { dcvel[i*6*nv+k*nv+j] += d.cdof[j*6+k]; }
            }
            j += 1;
        }
    }
}

pub fn d_rne_vel_dense(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nbody = m.nbody;
    let mut mat = [0.0; 36];
    let mut mat1 = [0.0; 36];
    let mut mat2 = [0.0; 36];
    let mut dmul = [0.0; 36];
    let mut tmp = [0.0; 6];
    let mut dcvel = d.scratch.num(nbody*6*nv);
    let mut dcdofdot = d.scratch.num(nv*6*nv);
    let mut dcacc = d.scratch.num(nbody*6*nv);
    let mut dcfrcbody = d.scratch.num(nbody*6*nv);
    let mut row = d.scratch.num(nv);
    d_com_vel_vel_dense(m, d, &mut dcvel, &mut dcdofdot);
    blas::zero(&mut dcacc, nbody*6*nv);
    for i in 1..nbody {
        let src = m.body_parentid[i] as usize*6*nv;
        dcacc.copy_within(src..src+6*nv, i*6*nv);
        for j in m.body_dofadr[i] as usize..(m.body_dofadr[i]+m.body_dofnum[i]) as usize {
            for k in 0..6 { dcacc[i*6*nv+k*nv+j] += d.cdof_dot[j*6+k]; }
            blas::add_to_scl(&mut dcacc[i*6*nv..], &dcdofdot[j*6*nv..], d.qvel[j], 6*nv);
        }
        d_mul_inert_vec_vel(&mut dmul, at10(&d.cinert, 10*i));
        blas::mul_mat_mat(&mut dcfrcbody[i*6*nv..], &dmul, &dcacc[i*6*nv..], 6, 6, nv);
        spatial::mul_inert_vec(&mut tmp, at10(&d.cinert, 10*i), at6(&d.cvel, i*6));
        d_cross_force_vel(&mut mat, &tmp);
        d_cross_force_frc(&mut mat1, at6(&d.cvel, i*6));
        blas::mul_mat_mat(&mut mat2, &mat1, &dmul, 6, 6, 6);
        blas::add_to(&mut mat, &mat2, 36);
        blas::mul_mat_mat(&mut dcfrcbody, &mat, &dcvel[i*6*nv..], 6, 6, nv);
        for k in 0..6*nv { let value = dcfrcbody[k]; dcfrcbody[i*6*nv+k] += value; }
    }
    blas::zero(&mut dcfrcbody, 6*nv);
    for i in (1..nbody).rev() {
        if m.body_parentid[i] != 0 {
            let dst = m.body_parentid[i] as usize*6*nv;
            for k in 0..6*nv { let value = dcfrcbody[i*6*nv+k]; dcfrcbody[dst+k] += value; }
        }
    }
    for i in 0..nv {
        for k in 0..6 {
            blas::scl(&mut row, &dcfrcbody[(m.dof_bodyid[i] as usize*6+k)*nv..], d.cdof[i*6+k], nv);
            let end = (m.d_rowadr[i]+m.d_rownnz[i]) as usize;
            for adr in m.d_rowadr[i] as usize..end { d.q_deriv[adr] -= row[m.d_colind[adr] as usize]; }
        }
    }
    d.scratch.put_num(dcvel);
    d.scratch.put_num(dcdofdot);
    d.scratch.put_num(dcacc);
    d.scratch.put_num(dcfrcbody);
    d.scratch.put_num(row);
}

// Sparse format: dense body/dof x sparse dof x 6.
fn copy_from_parent(m: &Model, _d: &Data, mat: &mut [f64], n: usize) {
    if n == 0 || m.body_weldid[m.body_parentid[n] as usize] == 0 { return; }
    let mut ndof = 0;
    let mut np = m.body_weldid[m.body_parentid[n] as usize];
    while np > 0 {
        ndof += m.body_dofnum[np as usize] as usize;
        np = m.body_weldid[m.body_parentid[np as usize] as usize];
    }
    let src = 6*m.b_rowadr[m.body_parentid[n] as usize] as usize;
    mat.copy_within(src..src+6*ndof, 6*m.b_rowadr[n] as usize);
}

fn add_to_parent(m: &Model, _d: &Data, mat: &mut [f64], n: usize) {
    if n == 0 || m.body_weldid[m.body_parentid[n] as usize] == 0 { return; }
    let np = m.body_parentid[n] as usize;
    let (mut i, mut ip) = (0, 0);
    while i < m.b_rownnz[n] as usize && ip < m.b_rownnz[np] as usize {
        let child = m.b_rowadr[n] as usize+i;
        let parent = m.b_rowadr[np] as usize+ip;
        if m.b_colind[child] == m.b_colind[parent] {
            for k in 0..6 { let value = mat[6*child+k]; mat[6*parent+k] += value; }
            i += 1;
            ip += 1;
        } else if m.b_colind[child] > m.b_colind[parent] {
            ip += 1;
        } else { panic!("child nonzeroes must be subset of parent"); }
    }
}

fn d_com_vel_vel(m: &Model, d: &Data, dcvel: &mut [f64], dcdofdot: &mut [f64]) {
    let nv = m.nv;
    let n_m = m.n_m;
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.nbody_awake < m.nbody as i32;
    let nbody = if sleep_filter { d.nbody_awake as usize } else { m.nbody };
    let badr = &m.b_rowadr;
    let dadr = &m.d_rowadr;
    let mut mat = [0.0; 36];
    let mut mat_t = [0.0; 36];
    for b in 1..nbody {
        let i = if sleep_filter { d.body_awake_ind[b] as usize } else { b };
        copy_from_parent(m, d, dcvel, i);
        let doflast = (m.body_dofadr[i]+m.body_dofnum[i]) as usize;
        let mut j = m.body_dofadr[i] as usize;
        while j < doflast {
            let mut jadr = (if j < nv-1 { m.dof_madr[j+1] as usize } else { n_m }) - (m.dof_madr[j] as usize+1);
            let joint_type = m.jnt_type[m.dof_jntid[j] as usize];
            if joint_type == JNT_FREE {
                blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr)..], &d.cdof[6*j..], 6);
                blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr+1)..], &d.cdof[6*(j+1)..], 6);
                blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr+2)..], &d.cdof[6*(j+2)..], 6);
                j += 3;
                jadr += 3;
            }
            match joint_type {
                JNT_FREE | JNT_BALL => {
                    for dj in 0..3 {
                        d_cross_motion_vel(&mut mat, at6(&d.cdof, 6*(j+dj)));
                        blas::transpose(&mut mat_t, &mat, 6, 6);
                        blas::mul_mat_mat(&mut dcdofdot[6*dadr[j+dj] as usize..], &dcvel[6*badr[i] as usize..], &mat_t, jadr+dj, 6, 6);
                    }
                    blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr)..], &d.cdof[6*j..], 6);
                    blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr+1)..], &d.cdof[6*(j+1)..], 6);
                    blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr+2)..], &d.cdof[6*(j+2)..], 6);
                    j += 2;
                }
                JNT_HINGE | JNT_SLIDE => {
                    d_cross_motion_vel(&mut mat, at6(&d.cdof, 6*j));
                    blas::transpose(&mut mat_t, &mat, 6, 6);
                    blas::mul_mat_mat(&mut dcdofdot[6*dadr[j] as usize..], &dcvel[6*badr[i] as usize..], &mat_t, jadr, 6, 6);
                    blas::add_to(&mut dcvel[6*(badr[i] as usize+jadr)..], &d.cdof[6*j..], 6);
                }
                _ => panic!("unknown joint type"),
            }
            j += 1;
        }
    }
}

fn d_rne_vel(m: &Model, d: &mut Data) {
    let n_m = m.n_m;
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.nbody_awake < m.nbody as i32;
    let nbody = if sleep_filter { d.nbody_awake as usize } else { m.nbody };
    let nparent = if sleep_filter { d.nparent_awake as usize } else { m.nbody };
    let mnv = m.nv;
    let nv = if sleep_filter { d.nv_awake as usize } else { mnv };
    let badr = &m.b_rowadr;
    let dadr = &m.d_rowadr;
    let bnnz = &m.b_rownnz;
    let mut mat = [0.0; 36];
    let mut mat1 = [0.0; 36];
    let mut mat2 = [0.0; 36];
    let mut dmul = [0.0; 36];
    let mut tmp = [0.0; 6];
    let mut dcdofdot = d.scratch.num(6*m.n_d);
    let mut dcvel = d.scratch.num(6*m.n_b);
    let mut dcacc = d.scratch.num(6*m.n_b);
    let mut dcfrcbody = d.scratch.num(6*m.n_b);
    let mut row = d.scratch.num(m.nv);
    if !sleep_filter {
        blas::zero(&mut dcdofdot, 6*m.n_d);
        blas::zero(&mut dcvel, 6*m.n_b);
        blas::zero(&mut dcacc, 6*m.n_b);
        blas::zero(&mut dcfrcbody, 6*m.n_b);
    } else {
        for i in 0..nv {
            let dof = d.dof_awake_ind[i] as usize;
            blas::zero(&mut dcdofdot[6*m.d_rowadr[dof] as usize..], 6*m.d_rownnz[dof] as usize);
        }
        for i in 0..nbody {
            let body = d.body_awake_ind[i] as usize;
            let adr = 6*m.b_rowadr[body] as usize;
            let nnz = 6*m.b_rownnz[body] as usize;
            blas::zero(&mut dcvel[adr..], nnz);
            blas::zero(&mut dcacc[adr..], nnz);
            blas::zero(&mut dcfrcbody[adr..], nnz);
        }
    }
    d_com_vel_vel(m, d, &mut dcvel, &mut dcdofdot);
    for b in 1..nbody {
        let i = if sleep_filter { d.body_awake_ind[b] as usize } else { b };
        copy_from_parent(m, d, &mut dcacc, i);
        let doflast = (m.body_dofadr[i]+m.body_dofnum[i]) as usize;
        for j in m.body_dofadr[i] as usize..doflast {
            let jadr = (if j < mnv-1 { m.dof_madr[j+1] as usize } else { n_m }) - (m.dof_madr[j] as usize+1);
            blas::add_to(&mut dcacc[6*(badr[i] as usize+jadr)..], &d.cdof_dot[6*j..], 6);
            blas::add_to_scl(&mut dcacc[6*badr[i] as usize..], &dcdofdot[6*dadr[j] as usize..], d.qvel[j], 6*bnnz[i] as usize);
        }
        d_mul_inert_vec_vel(&mut dmul, at10(&d.cinert, 10*i));
        blas::transpose(&mut mat1, &dmul, 6, 6);
        blas::mul_mat_mat(&mut dcfrcbody[6*badr[i] as usize..], &dcacc[6*badr[i] as usize..], &mat1, bnnz[i] as usize, 6, 6);
        spatial::mul_inert_vec(&mut tmp, at10(&d.cinert, 10*i), at6(&d.cvel, i*6));
        d_cross_force_vel(&mut mat, &tmp);
        d_cross_force_frc(&mut mat1, at6(&d.cvel, i*6));
        blas::mul_mat_mat(&mut mat2, &mat1, &dmul, 6, 6, 6);
        blas::add_to(&mut mat, &mat2, 36);
        blas::transpose(&mut mat1, &mat, 6, 6);
        blas::mul_mat_mat(&mut dcfrcbody, &dcvel[6*badr[i] as usize..], &mat1, bnnz[i] as usize, 6, 6);
        for k in 0..6*bnnz[i] as usize { let value = dcfrcbody[k]; dcfrcbody[6*badr[i] as usize+k] += value; }
    }
    blas::zero(&mut dcfrcbody, 6*bnnz[0] as usize);
    for b in (1..nparent).rev() {
        let i = if sleep_filter { d.parent_awake_ind[b] as usize } else { b };
        add_to_parent(m, d, &mut dcfrcbody, i);
    }
    for v in 0..nv {
        let j = if sleep_filter { d.dof_awake_ind[v] as usize } else { v };
        let i = m.dof_bodyid[j] as usize;
        blas::mul_mat_vec(&mut row, &dcfrcbody[6*badr[i] as usize..], &d.cdof[6*j..], bnnz[i] as usize, 6);
        blas::sub_from(&mut d.q_deriv[dadr[j] as usize..], &row, bnnz[i] as usize);
    }
    d.scratch.put_num(dcdofdot);
    d.scratch.put_num(dcvel);
    d.scratch.put_num(dcacc);
    d.scratch.put_num(dcfrcbody);
    d.scratch.put_num(row);
}

// 3x3 blocks of the free rigid subtree bias Jacobian.
fn free_bias_vel_blocks(mass: f64, r: &[f64; 9], iw: &[f64; 9], s: &[f64; 3], qvel_rot: &[f64; 3], lin: &mut [f64; 9], rot: &mut [f64; 9]) {
    let mut w = [0.0; 3];
    inline::mul_mat_vec3(&mut w, r, qvel_rot);
    let mut ws = [0.0; 3];
    let mut iww = [0.0; 3];
    inline::cross(&mut ws, &w, s);
    inline::mul_mat_vec3(&mut iww, iw, &w);
    let w_dot_s = w[0]*s[0] + w[1]*s[1] + w[2]*s[2];
    let mut k = [0.0; 9];
    k[0] = s[0]*w[0] - w_dot_s;
    k[1] = s[0]*w[1] - ws[2];
    k[2] = s[0]*w[2] + ws[1];
    k[3] = s[1]*w[0] + ws[2];
    k[4] = s[1]*w[1] - w_dot_s;
    k[5] = s[1]*w[2] - ws[0];
    k[6] = s[2]*w[0] - ws[1];
    k[7] = s[2]*w[1] + ws[0];
    k[8] = s[2]*w[2] - w_dot_s;
    inline::mul_mat_mat3(lin, &k, r);
    // C and c collide under snake case: C is c_mat.
    let mut c_mat = [0.0; 9];
    for c in 0..3 {
        let s_x_k_row0 = s[1]*k[6+c] - s[2]*k[3+c];
        let s_x_k_row1 = s[2]*k[c] - s[0]*k[6+c];
        let s_x_k_row2 = s[0]*k[3+c] - s[1]*k[c];
        let w_x_iw_row0 = w[1]*iw[6+c] - w[2]*iw[3+c];
        let w_x_iw_row1 = w[2]*iw[c] - w[0]*iw[6+c];
        let w_x_iw_row2 = w[0]*iw[3+c] - w[1]*iw[c];
        c_mat[c] = -mass * s_x_k_row0 + w_x_iw_row0 + if c == 1 { iww[2] } else if c == 2 { -iww[1] } else { 0.0 };
        c_mat[3+c] = -mass * s_x_k_row1 + w_x_iw_row1 + if c == 0 { -iww[2] } else if c == 2 { iww[0] } else { 0.0 };
        c_mat[6+c] = -mass * s_x_k_row2 + w_x_iw_row2 + if c == 0 { iww[1] } else if c == 1 { -iww[0] } else { 0.0 };
    }
    let mut tmp = [0.0; 9];
    inline::mul_mat_tmat3(&mut tmp, r, &c_mat);
    inline::mul_mat_mat3(rot, &tmp, r);
}

pub fn d_free_bias_vel(m: &Model, d: &Data, jnt: usize, b: &mut [f64; 36]) {
    let body = m.jnt_bodyid[jnt] as usize;
    let adr = m.jnt_dofadr[jnt] as usize;
    let crb = &d.crb[10*body..];
    let mass = crb[9];
    let iw = [crb[0], crb[3], crb[4], crb[3], crb[1], crb[5], crb[4], crb[5], crb[2]];
    let mut s = [0.0; 3];
    let mut lin = [0.0; 9];
    let mut rot = [0.0; 9];
    inline::sub3(&mut s, at3(&d.subtree_com, 3*body), at3(&d.xpos, 3*body));
    free_bias_vel_blocks(mass, at9(&d.xmat, 9*body), &iw, &s, at3(&d.qvel, adr+3), &mut lin, &mut rot);
    blas::zero(b, 36);
    for r in 0..3 {
        for c in 0..3 {
            b[6*r+3+c] = -mass * lin[3*r+c];
            b[6*(r+3)+3+c] = rot[3*r+c];
        }
    }
}

pub fn is_free_body(m: &Model, body: usize) -> bool {
    if m.body_jntnum[body] != 1 || m.jnt_type[m.body_jntadr[body] as usize] != JNT_FREE { return false; }
    let adr = m.jnt_dofadr[m.body_jntadr[body] as usize] as usize;
    m.tree_dofnum[m.dof_treeid[adr] as usize] == 6
}

pub fn d_free_mhat(m: &Model, d: &Data, jnt: usize, h: f64, a: &mut [f64; 36], flg_discrete: i32) -> i32 {
    let body = m.jnt_bodyid[jnt] as usize;
    let adr = m.jnt_dofadr[jnt] as usize;
    if !is_free_body(m, body) || d.tree_awake[m.dof_treeid[adr] as usize] == 0 { return 0; }
    blas::zero(a, 36);
    for r in 0..6 {
        let rowadr = m.m_rowadr[adr+r] as usize;
        let rownnz = m.m_rownnz[adr+r] as usize;
        for k in 0..rownnz {
            let c = (m.m_colind[rowadr+k] - adr as i32) as usize;
            a[6*c+r] = d.m[rowadr+k];
            a[6*r+c] = a[6*c+r];
        }
    }
    if flg_discrete != 0 {
        if !d.efm_diag.is_empty() {
            for r in 0..6 { a[6*r+r] += d.efm_diag[adr+r]; }
        }
        if !d.efm_fluid.is_empty() {
            for r in 0..6 {
                let rowadr = m.m_rowadr[adr+r] as usize;
                let rownnz = m.m_rownnz[adr+r] as usize;
                for k in 0..rownnz {
                    let c = (m.m_colind[rowadr+k] - adr as i32) as usize;
                    a[6*r+c] += d.efm_fluid[rowadr+k];
                    if c != r { a[6*c+r] += d.efm_fluid[rowadr+k]; }
                }
            }
        }
    } else {
        for r in 0..6 {
            let rowadr = m.d_rowadr[adr+r] as usize;
            let rownnz = m.d_rownnz[adr+r] as usize;
            for k in 0..rownnz {
                let c = (m.d_colind[rowadr+k] - adr as i32) as usize;
                a[6*r+c] -= h * d.q_deriv[rowadr+k];
            }
        }
    }
    let mut b = [0.0; 36];
    d_free_bias_vel(m, d, jnt, &mut b);
    blas::add_to_scl(a, &b, h, 36);
    1
}

pub fn d_free_gyro_possible(m: &Model, d: &Data, jnt: usize) -> i32 {
    let adr = m.jnt_dofadr[jnt] as usize;
    if m.jnt_type[jnt] != JNT_FREE || !is_free_body(m, m.jnt_bodyid[jnt] as usize) { return 0; }
    for r in 0..6 {
        if d.nefm_k != 0 && d.efm_k_rownnz[adr+r] != 0 { return 0; }
    }
    // matrix-free flex attachments can couple this free body without any CSR or constraint row
    let tree = m.body_treeid[m.jnt_bodyid[jnt] as usize];
    for f in 0..m.nflex {
        if core_util::eff_flex_possible(m, f) == 0 { continue; }
        let bodies = if m.flex_interp[f] != 0 { &m.flex_nodebodyid } else { &m.flex_vertbodyid };
        let vertadr = if m.flex_interp[f] != 0 { m.flex_nodeadr[f] } else { m.flex_vertadr[f] } as usize;
        let num = if m.flex_interp[f] != 0 { m.flex_nodenum[f] } else { m.flex_vertnum[f] } as usize;
        for v in vertadr..vertadr + num {
            if m.body_treeid[bodies[v] as usize] == tree { return 0; }
        }
    }
    let nefc = d.nefc as usize;
    if core_util::is_sparse(m) != 0 {
        for i in 0..nefc {
            let end = (d.efc_j_rowadr[i]+d.efc_j_rownnz[i]) as usize;
            for j in d.efc_j_rowadr[i] as usize..end {
                let c = d.efc_j_colind[j];
                if c >= (adr+6) as i32 { break; }
                if c >= adr as i32 { return 0; }
            }
        }
    } else {
        let nv = m.nv;
        for i in 0..nefc {
            let row = &d.efc_j[nv*i+adr..];
            for r in 0..6 { if row[r] != 0.0 { return 0; } }
        }
    }
    for t in 0..d.nefm_t as usize {
        if d.efm_ts[t] == 0.0 { continue; }
        let i = d.efm_tid[t] as usize;
        let end = (m.ten_j_rowadr[i]+m.ten_j_rownnz[i]) as usize;
        for j in m.ten_j_rowadr[i] as usize..end {
            let c = m.ten_j_colind[j];
            if c >= adr as i32 && c < (adr+6) as i32 { return 0; }
        }
    }
    for t in 0..d.nefm_a as usize {
        let i = d.efm_aid[t] as usize;
        let oadr = m.actuator_outadr[i] as usize;
        for k in 0..m.actuator_outnum[i] as usize {
            let r = oadr+k;
            let end = (d.moment_rowadr[r]+d.moment_rownnz[r]) as usize;
            for j in d.moment_rowadr[r] as usize..end {
                let c = d.moment_colind[j];
                if c >= adr as i32 && c < (adr+6) as i32 { return 0; }
            }
        }
    }
    1
}

// J and j collide under snake case: J is j_mat.
fn add_jtbj(m: &Model, d: &mut Data, res: &mut [f64], j_mat: &[f64], b: &[f64], n: usize) {
    let nv = m.nv;
    let mut row = d.scratch.num(nv);
    for i in 0..n {
        for j in 0..n {
            if b[i*n+j] == 0.0 { continue; }
            for k in 0..nv {
                if j_mat[i*nv+k] != 0.0 {
                    blas::scl(&mut row, &j_mat[j*nv..], j_mat[i*nv+k] * b[i*n+j], nv);
                    let rownnz_k = m.d_rownnz[k] as usize;
                    for s in 0..rownnz_k {
                        let adr = m.d_rowadr[k] as usize+s;
                        res[adr] += row[m.d_colind[adr] as usize];
                    }
                }
            }
        }
    }
    d.scratch.put_num(row);
}

fn add_jtbjsparse(m: &Model, _d: &Data, res: &mut [f64], j_mat: &[f64], b: &[f64], n: usize, offset: usize, j_rownnz: &[i32], j_rowadr: &[i32], j_colind: &[i32]) {
    for i in 0..n {
        for j in 0..n {
            if b[i*n+j] == 0.0 { continue; }
            let nnz_i = j_rownnz[offset+i] as usize;
            let adr_i = j_rowadr[offset+i] as usize;
            let nnz_j = j_rownnz[offset+j] as usize;
            let adr_j = j_rowadr[offset+j] as usize;
            for k in 0..nnz_i {
                let ik = adr_i+k;
                let colik = j_colind[ik] as usize;
                add_to_scl_sparse_inc(&mut res[m.d_rowadr[colik] as usize..], &j_mat[adr_j..], m.d_rownnz[colik] as usize, &m.d_colind[m.d_rowadr[colik] as usize..], nnz_j, &j_colind[adr_j..], j_mat[ik]*b[i*n+j]);
            }
        }
    }
}

fn d_muscle_gain_length(length: f64, lmin: f64, lmax: f64) -> f64 {
    if lmin <= length && length <= lmax {
        let a = 0.5*(lmin+1.0);
        let b = 0.5*(1.0+lmax);
        if length <= a {
            let x = (length-lmin) / max(MINVAL, a-lmin);
            return x / max(MINVAL, a-lmin);
        } else if length <= 1.0 {
            let x = (1.0-length) / max(MINVAL, 1.0-a);
            return x / max(MINVAL, 1.0-a);
        } else if length <= b {
            let x = (length-1.0) / max(MINVAL, b-1.0);
            return -x / max(MINVAL, b-1.0);
        } else {
            let x = (lmax-length) / max(MINVAL, lmax-b);
            return -x / max(MINVAL, lmax-b);
        }
    }
    0.0
}

fn d_muscle_gain_len(len: f64, vel: f64, lengthrange: &[f64; 2], acc0: f64, prm: &[f64; 9]) -> f64 {
    let range = [prm[0], prm[1]];
    let mut force = prm[2];
    let scale = prm[3];
    let lmin = prm[4];
    let lmax = prm[5];
    let vmax = prm[6];
    let fvmax = prm[8];
    if force < 0.0 { force = scale / max(MINVAL, acc0); }
    let l0 = (lengthrange[1]-lengthrange[0]) / max(MINVAL, range[1]-range[0]);
    let l = range[0] + (len-lengthrange[0]) / max(MINVAL, l0);
    let v = vel / max(MINVAL, l0*vmax);
    let d_fl = d_muscle_gain_length(l, lmin, lmax);
    let fv;
    let y = fvmax-1.0;
    if v <= -1.0 { fv = 0.0; }
    else if v <= 0.0 { fv = (v+1.0)*(v+1.0); }
    else if v <= y { fv = fvmax - (y-v)*(y-v) / max(MINVAL, y); }
    else { fv = fvmax; }
    -force*d_fl*fv/max(MINVAL, l0)
}

fn d_muscle_bias_len(len: f64, lengthrange: &[f64; 2], acc0: f64, prm: &[f64; 9]) -> f64 {
    let range = [prm[0], prm[1]];
    let mut force = prm[2];
    let scale = prm[3];
    let lmax = prm[5];
    let fpmax = prm[7];
    if force < 0.0 { force = scale / max(MINVAL, acc0); }
    let l0 = (lengthrange[1]-lengthrange[0]) / max(MINVAL, range[1]-range[0]);
    let l = range[0] + (len-lengthrange[0]) / max(MINVAL, l0);
    let b = 0.5*(1.0+lmax);
    let d_fp;
    if l <= 1.0 { d_fp = 0.0; }
    else if l <= b {
        let x = (l-1.0) / max(MINVAL, b-1.0);
        d_fp = x / max(MINVAL, b-1.0);
    } else { d_fp = 1.0 / max(MINVAL, b-1.0); }
    -force*fpmax*d_fp/max(MINVAL, l0)
}

fn d_muscle_gain_vel(len: f64, vel: f64, lengthrange: &[f64; 2], acc0: f64, prm: &[f64; 9]) -> f64 {
    let range = [prm[0], prm[1]];
    let mut force = prm[2];
    let scale = prm[3];
    let lmin = prm[4];
    let lmax = prm[5];
    let vmax = prm[6];
    let fvmax = prm[8];
    if force < 0.0 { force = scale / max(MINVAL, acc0); }
    let l0 = (lengthrange[1]-lengthrange[0]) / max(MINVAL, range[1]-range[0]);
    let l = range[0] + (len-lengthrange[0]) / max(MINVAL, l0);
    let v = vel / max(MINVAL, l0*vmax);
    let fl = muscle_gain_length(l, lmin, lmax);
    let d_fv;
    let y = fvmax-1.0;
    if v <= -1.0 { d_fv = 0.0; }
    else if v <= 0.0 { d_fv = 2.0*v + 2.0; }
    else if v <= y { d_fv = (-2.0*v + 2.0*y) / max(MINVAL, y); }
    else { d_fv = 0.0; }
    -force*fl*d_fv/max(MINVAL, l0*vmax)
}

// addJTBJ_mulSparse is omitted: all its callers are in the excluded flex kernels.
// Flex functions from flexInterpBounds through mjd_flexStiff_assembleGenStack
// remain omitted, except the pre-existing interpolation bounds/kernel/mul below.




fn flex_interp_bounds(m: &Model, max_nodenum: &mut usize, max_npe: &mut usize) {
    *max_nodenum = 0;
    *max_npe = 0;
    for f in 0..m.nflex {
        if m.flex_interp[f] == 0 { continue; }
        if m.flex_rigid[f] != 0 { continue; }
        let mut order = m.flex_interp[f];
        let shell_mode = order < 0;
        order = if order < 0 { -order } else { order };
        let npe = if shell_mode { (order + 1) * (order + 1) } else { (order + 1) * (order + 1) * (order + 1) } as usize;
        if npe > *max_npe { *max_npe = npe; }
        if m.flex_nodenum[f] as usize > *max_nodenum { *max_nodenum = m.flex_nodenum[f] as usize; }
    }
}

pub fn d_flex_interp_mul_bytes(m: &Model) -> usize {
    let mut max_nodenum = 0;
    let mut max_npe = 0;
    flex_interp_bounds(m, &mut max_nodenum, &mut max_npe);
    if max_npe == 0 { return 0; }
    let nv = m.nv;
    let dim = 3 * max_npe;
    let an = align_of::<f64>();
    let ai = align_of::<i32>();
    let sn = size_of::<f64>();
    let si = size_of::<i32>();
    let frame = stack_frame_bytes();
    frame + stack_bytes(sn * 3 * max_nodenum, an) + stack_bytes(sn * 3 * max_npe, an)
        + stack_bytes(sn * dim * dim, an) + 2 * stack_bytes(si * dim, ai)
        + stack_bytes(sn * dim * nv, an) + stack_bytes(si * dim * nv, ai)
        + stack_bytes(si * nv, ai) + stack_bytes(sn * 3 * nv, an) + stack_bytes(sn * dim, an)
        + frame + 2 * stack_bytes(sn * dim, an)
}

fn d_flex_interp_kernel(
    m: &Model, _d: &Data, _res: Option<&mut [f64]>, _vec: Option<&[f64]>,
    _s1: f64, _s2: f64, _k_rot_cache: Option<&[f64]>, _k_rot_out: Option<&mut [f64]>,
    _first: usize, _last: usize,
) {
    let _nv = m.nv;
    let mut max_nodenum = 0;
    let mut max_npe = 0;
    flex_interp_bounds(m, &mut max_nodenum, &mut max_npe);
    if max_npe == 0 { return; }
    crate::unported!("mjd_flexInterp_kernel");
}

pub fn d_flex_interp_mul(
    m: &Model, d: &Data, res: &mut [f64], vec: &[f64], s1: f64, s2: f64,
    k_rot_cache: Option<&[f64]>,
) {
    d_flex_interp_kernel(m, d, Some(res), Some(vec), s1, s2, k_rot_cache, None, 0, m.nflex);
}

fn dcmotor_inputs(m: &Model, d: &Data, i: usize, u4: &mut [f64; 4]) {
    let ctrlnum = m.actuator_ctrlnum[i] as usize;
    let mut u = [0.0; 4];
    if ctrlnum > 0 {
        let uadr = m.actuator_ctrladr[i] as usize;
        if m.actuator_delay[i] != 0.0 {
            if let Some(p) = support::read_ctrl(m, d, i, d.time, &mut u, -1) {
                blas::copy(&mut u, p, ctrlnum);
            }
        } else {
            blas::copy(&mut u, &d.ctrl[uadr..], ctrlnum);
        }
        if m.opt.disableflags & DSBL_CLAMPCTRL == 0 {
            for k in 0..ctrlnum {
                if m.actuator_ctrllimited[uadr+k] != 0 {
                    u[k] = clip(u[k], m.actuator_ctrlrange[2*(uadr+k)], m.actuator_ctrlrange[2*(uadr+k)+1]);
                }
            }
        }
        let slew_s = m.actuator_dynprm[NDYN*i+7];
        if slew_s > 0.0 && m.actuator_actadr[i] >= 0 {
            let u_prev = d.act[m.actuator_actadr[i] as usize];
            let slew = slew_s * m.opt.timestep;
            u[0] = clip(u[0], u_prev-slew, u_prev+slew);
        }
    }
    let spec = m.actuator_ctrlspec[i];
    let mut adr = 0;
    u4[0] = if spec & INPUT_POS != 0 { let v = u[adr]; adr += 1; v } else { 0.0 };
    u4[1] = if spec & INPUT_VEL != 0 { let v = u[adr]; adr += 1; v } else { 0.0 };
    u4[2] = if spec & INPUT_FF != 0 { let v = u[adr]; adr += 1; v } else { 0.0 };
    u4[3] = if spec & INPUT_VOLTAGE != 0 { u[adr] } else { 0.0 };
}

fn dcmotor_voltage(m: &Model, d: &Data, i: usize, mut v_clamped: Option<&mut i32>) -> f64 {
    let mut u4 = [0.0; 4];
    dcmotor_inputs(m, d, i, &mut u4);
    if let Some(v_clamped) = v_clamped.as_deref_mut() { *v_clamped = 0; }
    let spec = m.actuator_ctrlspec[i];
    let mut voltage = 0.0;
    if m.actuator_ctrlnum[i] > 0 && spec & (INPUT_POS | INPUT_VEL | INPUT_FF) != 0 {
        let dynprm = &m.actuator_dynprm[NDYN*i..];
        let gainprm = &m.actuator_gainprm[NGAIN*i..];
        let slots = dcmotor_slots(dynprm, gainprm);
        let oadr = m.actuator_outadr[i] as usize;
        let x_i = if slots.integral >= 0 { d.act[(m.actuator_actadr[i]+slots.integral) as usize] } else { 0.0 };
        let torque = gainprm[4]*(u4[0] - d.actuator_length[oadr]) +
            gainprm[6]*(u4[1] - d.actuator_velocity[oadr]) + gainprm[5]*x_i + u4[2];
        voltage = gainprm[0]/gainprm[1] * torque + gainprm[1]*d.actuator_velocity[oadr];
        let vmax = gainprm[7];
        if vmax > 0.0 && voltage.abs() >= vmax {
            if let Some(v_clamped) = v_clamped { *v_clamped = 1; }
            voltage = clip(voltage, -vmax, vmax);
        }
    }
    voltage + u4[3]
}

fn dcmotor_pre_mech_force(m: &Model, d: &Data, i: usize) -> f64 {
    let oadr = m.actuator_outadr[i] as usize;
    let biasprm = &m.actuator_biasprm[NBIAS*i..];
    let dynprm = &m.actuator_dynprm[NDYN*i..];
    let mut f = d.actuator_force[oadr];
    if support::actuator_disabled(m, i) != 0 || m.actuator_plugin[i] >= 0 ||
        (m.opt.enableflags & ENBL_SLEEP != 0 && crate::sleep::sleep_state(m, d, OBJ_ACTUATOR, i as i32) == S_ASLEEP) {
        return f;
    }
    if biasprm[0] != 0.0 {
        f -= biasprm[0] * (biasprm[1]*d.actuator_length[oadr] + biasprm[2]).sin();
    }
    if dynprm[5] > 0.0 {
        let slots = dcmotor_slots(dynprm, &m.actuator_gainprm[NGAIN*i..]);
        let adr = (m.actuator_actadr[i]+slots.bristle) as usize;
        f += dynprm[5]*d.act[adr] + dynprm[6]*d.act_dot[adr];
    }
    f
}

fn tendon_act_frc_clamped(m: &Model, d: &Data, tendon_id: usize) -> i32 {
    if m.tendon_actfrclimited[tendon_id] == 0 { return 0; }
    let range = &m.tendon_actfrcrange[2*tendon_id..];
    let mut tendon_force = 0.0;
    for j in 0..m.nactuator {
        if m.actuator_trntype[j] == TRN_TENDON && m.actuator_trnid[2*j] == tendon_id as i32 {
            tendon_force += if m.actuator_biastype[j] == BIAS_DCMOTOR { dcmotor_pre_mech_force(m, d, j) } else { d.actuator_force[m.actuator_outadr[j] as usize] };
        }
    }
    let tol = 1e-10;
    (tendon_force <= range[0] + tol*range[0].abs() || tendon_force >= range[1] - tol*range[1].abs()) as i32
}

fn dcmotor_saturation(m: &Model, d: &Data, i: usize) -> i32 {
    let mut flags = 0;
    let mut v_clamped = 0;
    let v = dcmotor_voltage(m, d, i, Some(&mut v_clamped));
    if v_clamped != 0 { flags |= 2; }
    let dynprm = &m.actuator_dynprm[NDYN*i..];
    let gainprm = &m.actuator_gainprm[NGAIN*i..];
    let k = gainprm[1];
    let f_elec;
    if dynprm[0] > 0.0 {
        let act_adr = (m.actuator_actadr[i]+m.actuator_actnum[i]-1) as usize;
        let act = if m.actuator_actearly[i] != 0 { support::next_activation(m, d, i, act_adr, d.act_dot[act_adr]) } else { d.act[act_adr] };
        f_elec = k * act;
    } else {
        let oadr = m.actuator_outadr[i] as usize;
        let r = core_util::dcmotor_resistance(m, d, i);
        f_elec = (k / r) * (v - k * d.actuator_velocity[oadr]);
    }
    if m.actuator_forcelimited[i] != 0 {
        let range = &m.actuator_forcerange[2*i..];
        if f_elec <= range[0] || f_elec >= range[1] { flags |= 1; }
    }
    if flags & 1 == 0 && m.actuator_trntype[i] == TRN_TENDON && tendon_act_frc_clamped(m, d, m.actuator_trnid[2*i] as usize) != 0 { flags |= 1; }
    if dynprm[0] > 0.0 && dynprm[1] > 0.0 {
        let act_last = (m.actuator_actadr[i]+m.actuator_actnum[i]-1) as usize;
        if d.act_dot[act_last].abs() >= dynprm[1] { flags |= 4; }
    }
    flags
}

fn dcmotor_elec_clamped(m: &Model, d: &Data, i: usize) -> i32 {
    (dcmotor_saturation(m, d, i) & 1 != 0) as i32
}

pub fn d_actuator_deriv_skip(m: &Model, d: &Data, i: usize, sleep_filter: i32) -> i32 {
    if support::actuator_disabled(m, i) != 0 { return 1; }
    if sleep_filter != 0 && crate::sleep::sleep_state(m, d, OBJ_ACTUATOR, i as i32) == S_ASLEEP { return 1; }
    if m.actuator_biastype[i] == BIAS_DCMOTOR {
        let has_mech = m.actuator_biasprm[NBIAS*i] != 0.0 || m.actuator_dynprm[NDYN*i+6] > 0.0;
        if !has_mech && dcmotor_elec_clamped(m, d, i) != 0 { return 1; }
    } else {
        if m.actuator_forcelimited[i] != 0 {
            let oadr = m.actuator_outadr[i] as usize;
            let range = &m.actuator_forcerange[2*i..];
            if m.actuator_gaintype[i] == GAIN_SO3 {
                if blas::norm3(at3(&d.actuator_force, oadr)) >= range[1] { return 1; }
            } else {
                let force = d.actuator_force[oadr];
                if force <= range[0] || force >= range[1] { return 1; }
            }
        }
        if m.actuator_trntype[i] == TRN_TENDON && tendon_act_frc_clamped(m, d, m.actuator_trnid[2*i] as usize) != 0 { return 1; }
    }
    if m.actuator_trntype[i] == TRN_JOINT || m.actuator_trntype[i] == TRN_JOINTINPARENT {
        let jnt_id = m.actuator_trnid[2*i] as usize;
        if m.jnt_actfrclimited[jnt_id] != 0 {
            let range = &m.jnt_actfrcrange[2*jnt_id..];
            let jnt_force = d.qfrc_actuator[m.jnt_dofadr[jnt_id] as usize];
            if jnt_force <= range[0] || jnt_force >= range[1] { return 1; }
        }
    }
    0
}

fn actuator_input(m: &Model, d: &Data, i: usize) -> f64 {
    if m.actuator_dyntype[i] == DYN_NONE {
        let adr = m.actuator_ctrladr[i] as usize;
        let mut ctrl;
        if m.actuator_delay[i] != 0.0 {
            let mut ctrl_buf = [0.0; 4];
            ctrl = match support::read_ctrl(m, d, i, d.time, &mut ctrl_buf, -1) { Some(p) => p[0], None => ctrl_buf[0] };
        } else {
            ctrl = d.ctrl[adr];
        }
        if m.opt.disableflags & DSBL_CLAMPCTRL == 0 && m.actuator_ctrllimited[adr] != 0 {
            ctrl = clip(ctrl, m.actuator_ctrlrange[2*adr], m.actuator_ctrlrange[2*adr+1]);
        }
        return ctrl;
    }
    let act_adr = (m.actuator_actadr[i]+m.actuator_actnum[i]-1) as usize;
    let mut act = d.act[act_adr];
    if m.actuator_actearly[i] != 0 { act = support::next_activation(m, d, i, act_adr, d.act_dot[act_adr]); }
    act
}

pub fn d_actuator_len_deriv(m: &Model, d: &Data, i: usize) -> f64 {
    let oadr = m.actuator_outadr[i] as usize;
    let mut bias_len = 0.0;
    let mut gain_len = 0.0;
    if m.actuator_biastype[i] == BIAS_AFFINE || m.actuator_biastype[i] == BIAS_SO3 {
        bias_len = m.actuator_biasprm[NBIAS*i+1];
    } else if m.actuator_biastype[i] == BIAS_MUSCLE {
        bias_len = d_muscle_bias_len(d.actuator_length[oadr], m.actuator_lengthrange[2*oadr..2*oadr+2].try_into().unwrap(), m.actuator_acc0[oadr], at9(&m.actuator_biasprm, NBIAS*i));
    } else if m.actuator_biastype[i] == BIAS_DCMOTOR {
        let biasprm = &m.actuator_biasprm[NBIAS*i..];
        let a = biasprm[0];
        if a != 0.0 {
            let np = biasprm[1];
            let phi = biasprm[2];
            bias_len = a * np * (np * d.actuator_length[oadr] + phi).cos();
        }
    }
    if m.actuator_gaintype[i] == GAIN_AFFINE {
        gain_len = m.actuator_gainprm[NGAIN*i+1];
    } else if m.actuator_gaintype[i] == GAIN_MUSCLE {
        gain_len = d_muscle_gain_len(d.actuator_length[oadr], d.actuator_velocity[oadr], m.actuator_lengthrange[2*oadr..2*oadr+2].try_into().unwrap(), m.actuator_acc0[oadr], at9(&m.actuator_gainprm, NGAIN*i));
    } else if m.actuator_gaintype[i] == GAIN_DCMOTOR {
        let sat = dcmotor_saturation(m, d, i);
        if sat & 1 == 0 {
            let dynprm = &m.actuator_dynprm[NDYN*i..];
            let gainprm = &m.actuator_gainprm[NGAIN*i..];
            let te = dynprm[0];
            let v_clamped = sat & 2 != 0;
            let mut d_vdl = 0.0;
            if !v_clamped && m.actuator_ctrlspec[i] & (INPUT_POS | INPUT_VEL | INPUT_FF) != 0 {
                let r0 = max(MINVAL, gainprm[0]);
                let k = gainprm[1];
                d_vdl = -gainprm[4]*r0/k;
            }
            if te > 0.0 {
                if m.actuator_actearly[i] != 0 && d_vdl != 0.0 && sat & 4 == 0 {
                    let r = core_util::dcmotor_resistance(m, d, i);
                    let k = gainprm[1];
                    let s = 1.0 - (-m.opt.timestep / te).exp();
                    bias_len += k * d_vdl * s / r;
                }
            } else if d_vdl != 0.0 {
                let r = core_util::dcmotor_resistance(m, d, i);
                let k = gainprm[1];
                bias_len += k * d_vdl / r;
            }
        }
    }
    if gain_len != 0.0 { bias_len += gain_len * actuator_input(m, d, i); }
    bias_len
}

pub fn d_actuator_vel_deriv(m: &Model, d: &Data, i: usize) -> f64 {
    let oadr = m.actuator_outadr[i] as usize;
    let mut bias_vel = 0.0;
    let mut gain_vel = 0.0;
    let mut sat_dc = 0;
    if m.actuator_biastype[i] == BIAS_DCMOTOR || m.actuator_gaintype[i] == GAIN_DCMOTOR {
        sat_dc = dcmotor_saturation(m, d, i);
    }
    if m.actuator_biastype[i] == BIAS_AFFINE {
        bias_vel = m.actuator_biasprm[NBIAS*i+2];
    } else if m.actuator_biastype[i] == BIAS_SO3 {
        bias_vel = m.actuator_biasprm[NBIAS*i+2];
    } else if m.actuator_biastype[i] == BIAS_DCMOTOR {
        let dynprm = &m.actuator_dynprm[NDYN*i..];
        let gainprm = &m.actuator_gainprm[NGAIN*i..];
        if dynprm[0] <= 0.0 && sat_dc & 1 == 0 {
            let r = core_util::dcmotor_resistance(m, d, i);
            let k = gainprm[1];
            bias_vel -= k * k / r;
        }
    }
    if m.actuator_gaintype[i] == GAIN_AFFINE {
        gain_vel = m.actuator_gainprm[NGAIN*i+2];
    } else if m.actuator_gaintype[i] == GAIN_MUSCLE {
        gain_vel = d_muscle_gain_vel(d.actuator_length[oadr], d.actuator_velocity[oadr], m.actuator_lengthrange[2*oadr..2*oadr+2].try_into().unwrap(), m.actuator_acc0[oadr], at9(&m.actuator_gainprm, NGAIN*i));
    } else if m.actuator_gaintype[i] == GAIN_DCMOTOR {
        let dynprm = &m.actuator_dynprm[NDYN*i..];
        let gainprm = &m.actuator_gainprm[NGAIN*i..];
        let te = dynprm[0];
        if sat_dc & 1 == 0 {
            let v_clamped = sat_dc & 2 != 0;
            let mut d_vdw = 0.0;
            if !v_clamped && m.actuator_ctrlspec[i] & (INPUT_POS | INPUT_VEL | INPUT_FF) != 0 {
                let r0 = max(MINVAL, gainprm[0]);
                let k = gainprm[1];
                d_vdw = -gainprm[6]*r0/k + k;
            }
            if te > 0.0 {
                if m.actuator_actearly[i] != 0 && sat_dc & 4 == 0 {
                    let r = core_util::dcmotor_resistance(m, d, i);
                    let k = gainprm[1];
                    let s = 1.0 - (-m.opt.timestep / te).exp();
                    bias_vel += k * (d_vdw-k) * s / r;
                }
            } else if d_vdw != 0.0 {
                let r = core_util::dcmotor_resistance(m, d, i);
                let k = gainprm[1];
                bias_vel += k * d_vdw / r;
            }
        }
        let sigma1 = dynprm[6];
        if sigma1 > 0.0 { bias_vel -= sigma1; }
    }
    if gain_vel != 0.0 { bias_vel += gain_vel * actuator_input(m, d, i); }
    bias_vel
}

pub fn d_actuator_vel(m: &Model, d: &mut Data) {
    let nactuator = m.nactuator;
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
    if m.opt.disableflags & DSBL_ACTUATION != 0 { return; }
    for i in 0..nactuator {
        if d_actuator_deriv_skip(m, d, i, sleep_filter as i32) != 0 { continue; }
        let bias_vel = d_actuator_vel_deriv(m, d, i);
        if bias_vel != 0.0 {
            let oadr = m.actuator_outadr[i] as usize;
            let mut q_deriv = std::mem::take(&mut d.q_deriv);
            for k in 0..m.actuator_outnum[i] as usize {
                add_jtbjsparse(m, d, &mut q_deriv, &d.actuator_moment, &[bias_vel], 1, oadr+k, &d.moment_rownnz, &d.moment_rowadr, &d.moment_colind);
            }
            d.q_deriv = q_deriv;
        }
    }
}

fn pow2(val: f64) -> f64 { val*val }

fn ellipsoid_max_moment(size: &[f64; 3], dir: usize) -> f64 {
    let d0 = size[dir];
    let d1 = size[(dir+1)%3];
    let d2 = size[(dir+2)%3];
    8.0/15.0 * PI * d0 * pow2(pow2(max(d1, d2)))
}

fn add_to_quadrant(b: &mut [f64; 36], d: &[f64; 9], col_quad: usize, row_quad: usize) {
    let r = 3*row_quad;
    let c = 3*col_quad;
    b[6*c+r] += d[0];
    b[6*c+r+1] += d[1];
    b[6*c+r+2] += d[2];
    b[6*(c+1)+r] += d[3];
    b[6*(c+1)+r+1] += d[4];
    b[6*(c+1)+r+2] += d[5];
    b[6*(c+2)+r] += d[6];
    b[6*(c+2)+r+1] += d[7];
    b[6*(c+2)+r+2] += d[8];
}

fn d_added_mass_forces(b: &mut [f64; 36], local_vels: &[f64; 6], fluid_density: f64, virtual_mass: &[f64; 3], virtual_inertia: &[f64; 3]) {
    let lin_vel = [local_vels[3], local_vels[4], local_vels[5]];
    let ang_vel = [local_vels[0], local_vels[1], local_vels[2]];
    let virtual_lin_mom = [fluid_density * virtual_mass[0] * lin_vel[0], fluid_density * virtual_mass[1] * lin_vel[1], fluid_density * virtual_mass[2] * lin_vel[2]];
    let virtual_ang_mom = [fluid_density * virtual_inertia[0] * ang_vel[0], fluid_density * virtual_inertia[1] * ang_vel[1], fluid_density * virtual_inertia[2] * ang_vel[2]];
    let mut da = [0.0; 9];
    let mut db = [0.0; 9];
    d_cross(&virtual_ang_mom, &ang_vel, Some(&mut da), Some(&mut db));
    add_to_quadrant(b, &db, 0, 0);
    for i in 0..9 { da[i] *= fluid_density * virtual_inertia[i%3]; }
    add_to_quadrant(b, &da, 0, 0);
    d_cross(&virtual_lin_mom, &lin_vel, Some(&mut da), Some(&mut db));
    add_to_quadrant(b, &db, 0, 1);
    for i in 0..9 { da[i] *= fluid_density * virtual_mass[i%3]; }
    add_to_quadrant(b, &da, 0, 1);
    d_cross(&virtual_lin_mom, &ang_vel, Some(&mut da), Some(&mut db));
    add_to_quadrant(b, &db, 1, 0);
    for i in 0..9 { da[i] *= fluid_density * virtual_mass[i%3]; }
    add_to_quadrant(b, &da, 1, 1);
}

fn d_viscous_torque(d: &mut [f64; 9], lvel: &[f64; 6], fluid_density: f64, fluid_viscosity: f64, size: &[f64; 3], slender_drag_coef: f64, ang_drag_coef: f64) {
    let d_max = max(max(size[0], size[1]), size[2]);
    let d_min = min(min(size[0], size[1]), size[2]);
    let d_mid = size[0]+size[1]+size[2]-d_max-d_min;
    let eq_sphere_d = 2.0/3.0 * (size[0]+size[1]+size[2]);
    let lin_visc_torq_coef = PI * eq_sphere_d*eq_sphere_d*eq_sphere_d;
    let i_max = 8.0/15.0 * PI * d_mid * (d_max*d_max)*(d_max*d_max);
    let ii = [ellipsoid_max_moment(size, 0), ellipsoid_max_moment(size, 1), ellipsoid_max_moment(size, 2)];
    let x = lvel[0]; let y = lvel[1]; let z = lvel[2];
    let mom_coef = [ang_drag_coef*ii[0]+slender_drag_coef*(i_max-ii[0]), ang_drag_coef*ii[1]+slender_drag_coef*(i_max-ii[1]), ang_drag_coef*ii[2]+slender_drag_coef*(i_max-ii[2])];
    let mom_visc = [x*mom_coef[0], y*mom_coef[1], z*mom_coef[2]];
    let density = fluid_density / max(MINVAL, blas::norm3(&mom_visc));
    let mom_sq = [-density*x*mom_coef[0]*mom_coef[0], -density*y*mom_coef[1]*mom_coef[1], -density*z*mom_coef[2]*mom_coef[2]];
    let lin_coef = fluid_viscosity * lin_visc_torq_coef;
    blas::zero(d, 9);
    d[8] = x*mom_sq[0]+y*mom_sq[1]+z*mom_sq[2]-lin_coef;
    d[4] = d[8]; d[0] = d[4];
    blas::add_to_scl3(at3_mut(d, 0), &mom_sq, x);
    blas::add_to_scl3(at3_mut(d, 3), &mom_sq, y);
    blas::add_to_scl3(at3_mut(d, 6), &mom_sq, z);
}

fn d_viscous_drag(d: &mut [f64; 9], lvel: &[f64; 6], fluid_density: f64, fluid_viscosity: f64, size: &[f64; 3], blunt_drag_coef: f64, slender_drag_coef: f64) {
    let d_max = max(max(size[0], size[1]), size[2]);
    let d_min = min(min(size[0], size[1]), size[2]);
    let d_mid = size[0]+size[1]+size[2]-d_max-d_min;
    let eq_sphere_d = 2.0/3.0 * (size[0]+size[1]+size[2]);
    let a_max = PI * d_max * d_mid;
    let inv_dmax = if d_max > MINVAL { 1.0/d_max } else { 0.0 };
    let s0 = size[0]*inv_dmax;
    let s1 = size[1]*inv_dmax;
    let s2 = size[2]*inv_dmax;
    let norm = blas::norm3(at3(lvel, 3));
    let inv_norm = if norm > MINVAL { 1.0/norm } else { 0.0 };
    let x = lvel[3]*inv_norm;
    let y = lvel[4]*inv_norm;
    let z = lvel[5]*inv_norm;
    let xx = x*x; let yy = y*y; let zz = z*z;
    let xy = x*y; let yz = y*z; let xz = x*z;
    let a_raw = pow2(s1*s2);
    let b_raw = pow2(s2*s0);
    let c_raw = pow2(s0*s1);
    let proj_num = a_raw*xx + b_raw*yy + c_raw*zz;
    let inv_proj_num = if proj_num > MINVAL { 1.0/proj_num } else { 0.0 };
    let a = a_raw*inv_proj_num;
    let b = b_raw*inv_proj_num;
    let c = c_raw*inv_proj_num;
    let aa = a*a; let bb = b*b; let cc = c*c;
    let proj_denom = aa*xx+bb*yy+cc*zz;
    let area_scale = d_max*d_max*proj_num.sqrt();
    let d_a_coef = if proj_denom > MINVAL { PI*area_scale/proj_denom.sqrt() } else { 0.0 };
    let a_proj = PI*area_scale*proj_denom.sqrt();
    let lin_coef = fluid_viscosity*3.0*PI*eq_sphere_d;
    let quad_coef = fluid_density * (a_proj*blunt_drag_coef + slender_drag_coef*(a_max-a_proj));
    let aproj_coef = fluid_density*norm*(blunt_drag_coef-slender_drag_coef);
    let d_aproj_dv = [
        aproj_coef*d_a_coef*a*x*(b*yy*(a-b)+c*zz*(a-c)),
        aproj_coef*d_a_coef*b*y*(a*xx*(b-a)+c*zz*(b-c)),
        aproj_coef*d_a_coef*c*z*(a*xx*(c-a)+b*yy*(c-b))
    ];
    d[0] = xx; d[1] = xy; d[2] = xz;
    d[3] = xy; d[4] = yy; d[5] = yz;
    d[6] = xz; d[7] = yz; d[8] = zz;
    let inner = xx+yy+zz;
    d[0] += inner; d[4] += inner; d[8] += inner;
    let input = *d;
    blas::scl(d, &input, -quad_coef*norm, 9);
    blas::add_to_scl3(at3_mut(d, 0), &d_aproj_dv, -x);
    blas::add_to_scl3(at3_mut(d, 3), &d_aproj_dv, -y);
    blas::add_to_scl3(at3_mut(d, 6), &d_aproj_dv, -z);
    d[0] -= lin_coef; d[4] -= lin_coef; d[8] -= lin_coef;
}

fn d_kutta_lift(d: &mut [f64; 9], lvel: &[f64; 6], fluid_density: f64, size: &[f64; 3], kutta_lift_coef: f64) {
    let d_max = max(max(size[0], size[1]), size[2]);
    let inv_dmax = if d_max > MINVAL { 1.0/d_max } else { 0.0 };
    let s0 = size[0]*inv_dmax;
    let s1 = size[1]*inv_dmax;
    let s2 = size[2]*inv_dmax;
    let norm = blas::norm3(at3(lvel, 3));
    let inv_norm = if norm > MINVAL { 1.0/norm } else { 0.0 };
    let x = lvel[3]*inv_norm;
    let y = lvel[4]*inv_norm;
    let z = lvel[5]*inv_norm;
    let xx = x*x; let yy = y*y; let zz = z*z;
    let xy = x*y; let yz = y*z; let xz = x*z;
    let a_raw = pow2(s1*s2);
    let b_raw = pow2(s2*s0);
    let c_raw = pow2(s0*s1);
    let proj_num = a_raw*xx + b_raw*yy + c_raw*zz;
    let inv_proj_num = if proj_num > MINVAL { 1.0/proj_num } else { 0.0 };
    let a = a_raw*inv_proj_num;
    let b = b_raw*inv_proj_num;
    let c = c_raw*inv_proj_num;
    let aa = a*a; let bb = b*b; let cc = c*c;
    let proj_denom = aa*xx+bb*yy+cc*zz;
    let area_scale = d_max*d_max*proj_num.sqrt();
    let df_denom = if proj_denom > MINVAL { PI*kutta_lift_coef*fluid_density*area_scale*norm/proj_denom.sqrt() } else { 0.0 };
    let dfx_coef = yy*(a-b)+zz*(a-c);
    let dfy_coef = xx*(b-a)+zz*(b-c);
    let dfz_coef = xx*(c-a)+yy*(c-b);
    let proj_term = if proj_denom > MINVAL { 1.0/proj_denom } else { 0.0 };
    let cos_term = 1.0;
    d[0] = a-a; d[1] = b-a; d[2] = c-a;
    d[3] = a-b; d[4] = b-b; d[5] = c-b;
    d[6] = a-c; d[7] = b-c; d[8] = c-c;
    let input = *d;
    blas::scl(d, &input, 2.0, 9);
    let inner_term = [aa*proj_term-a+cos_term, bb*proj_term-b+cos_term, cc*proj_term-c+cos_term];
    blas::add_to_scl3(at3_mut(d, 0), &inner_term, dfx_coef);
    blas::add_to_scl3(at3_mut(d, 3), &inner_term, dfy_coef);
    blas::add_to_scl3(at3_mut(d, 6), &inner_term, dfz_coef);
    d[0] *= xx; d[1] *= xy; d[2] *= xz;
    d[3] *= xy; d[4] *= yy; d[5] *= yz;
    d[6] *= xz; d[7] *= yz; d[8] *= zz;
    d[0] -= dfx_coef; d[4] -= dfy_coef; d[8] -= dfz_coef;
    let input = *d;
    blas::scl(d, &input, df_denom, 9);
}

fn d_magnus_force(b: &mut [f64; 36], lvel: &[f64; 6], fluid_density: f64, size: &[f64; 3], magnus_lift_coef: f64) {
    let volume = 4.0/3.0 * PI * size[0]*size[1]*size[2];
    let magnus_coef = magnus_lift_coef*fluid_density*volume;
    let mut d_lin = [0.0; 9];
    let mut d_ang = [0.0; 9];
    let lin_vel = [magnus_coef*lvel[3], magnus_coef*lvel[4], magnus_coef*lvel[5]];
    let ang_vel = [magnus_coef*lvel[0], magnus_coef*lvel[1], magnus_coef*lvel[2]];
    d_cross(&ang_vel, &lin_vel, Some(&mut d_ang), Some(&mut d_lin));
    add_to_quadrant(b, &d_ang, 1, 0);
    add_to_quadrant(b, &d_lin, 1, 1);
}

pub fn d_ellipsoid_fluid(m: &Model, d: &mut Data, res: &mut [f64], bodyid: usize, flg_dragonly: i32) {
    let nv = m.nv;
    let mut nnz = nv;
    let mut rownnz = [0; 6];
    let mut rowadr = [0; 6];
    // J and j collide under snake case: J is j_mat.
    let mut j_mat = d.scratch.num(6*nv);
    let mut tmp = d.scratch.num(3*nv);
    let mut colind = d.scratch.int(6*nv);
    let mut colind_compressed = d.scratch.int(6*nv);
    let mut lvel = [0.0; 6];
    let mut wind = [0.0; 6];
    let mut lwind = [0.0; 6];
    let mut geom_interaction_coef = 0.0;
    let mut magnus_lift_coef = 0.0;
    let mut kutta_lift_coef = 0.0;
    let mut semiaxes = [0.0; 3];
    let mut virtual_mass = [0.0; 3];
    let mut virtual_inertia = [0.0; 3];
    let mut blunt_drag_coef = 0.0;
    let mut slender_drag_coef = 0.0;
    let mut ang_drag_coef = 0.0;
    if core_util::is_sparse(m) != 0 {
        nnz = core_util::body_chain(m, bodyid, &mut colind);
        for i in 0..6 {
            rownnz[i] = nnz as i32;
            rowadr[i] = if i == 0 { 0 } else { rowadr[i-1]+nnz as i32 };
            for k in 0..nnz { colind_compressed[i*nnz+k] = colind[k]; }
        }
    }
    for j in 0..m.body_geomnum[bodyid] as usize {
        let geomid = m.body_geomadr[bodyid] as usize+j;
        crate::util_misc::geom_semi_axes(&mut semiaxes, at3(&m.geom_size, 3*geomid), m.geom_type[geomid]);
        passive::read_fluid_geom_interaction(&m.geom_fluid[NFLUID*geomid..], &mut geom_interaction_coef, &mut blunt_drag_coef, &mut slender_drag_coef, &mut ang_drag_coef, &mut kutta_lift_coef, &mut magnus_lift_coef, &mut virtual_mass, &mut virtual_inertia);
        if geom_interaction_coef == 0.0 { continue; }
        core_util::object_velocity(m, d, OBJ_GEOM, geomid, &mut lvel, 1);
        blas::zero(&mut wind, 6);
        blas::copy3(at3_mut(&mut wind, 3), &m.opt.wind);
        spatial::transform_spatial(&mut lwind, &wind, 0, at3(&d.geom_xpos, 3*geomid), at3(&d.subtree_com, 3*m.body_rootid[bodyid] as usize), Some(at9(&d.geom_xmat, 9*geomid)));
        blas::sub_from3(at3_mut(&mut lvel, 3), at3(&lwind, 3));
        if core_util::is_sparse(m) != 0 {
            let (jr, jp) = j_mat.split_at_mut(3*nnz);
            core_util::jac_sparse(m, d, Some(jp), Some(jr), at3(&d.geom_xpos, 3*geomid), m.geom_bodyid[geomid] as usize, nnz, &colind, 0);
        } else {
            let (jr, jp) = j_mat.split_at_mut(3*nv);
            core_util::jac_geom(m, d, Some(jp), Some(jr), geomid);
        }
        blas::mul_mat_tmat(&mut tmp, &d.geom_xmat[9*geomid..], &j_mat, 3, 3, nnz);
        blas::copy(&mut j_mat, &tmp, 3*nnz);
        blas::mul_mat_tmat(&mut tmp, &d.geom_xmat[9*geomid..], &j_mat[3*nnz..], 3, 3, nnz);
        blas::copy(&mut j_mat[3*nnz..], &tmp, 3*nnz);
        let mut b = [0.0; 36];
        // C's D matrix collides with Data d: D is d_mat.
        let mut d_mat = [0.0; 9];
        blas::zero(&mut b, 36);
        if flg_dragonly == 0 {
            d_magnus_force(&mut b, &lvel, m.opt.density, &semiaxes, magnus_lift_coef);
            d_kutta_lift(&mut d_mat, &lvel, m.opt.density, &semiaxes, kutta_lift_coef);
            add_to_quadrant(&mut b, &d_mat, 1, 1);
        }
        d_viscous_drag(&mut d_mat, &lvel, m.opt.density, m.opt.viscosity, &semiaxes, blunt_drag_coef, slender_drag_coef);
        add_to_quadrant(&mut b, &d_mat, 1, 1);
        d_viscous_torque(&mut d_mat, &lvel, m.opt.density, m.opt.viscosity, &semiaxes, slender_drag_coef, ang_drag_coef);
        add_to_quadrant(&mut b, &d_mat, 0, 0);
        if flg_dragonly == 0 { d_added_mass_forces(&mut b, &lvel, m.opt.density, &virtual_mass, &virtual_inertia); }
        if flg_dragonly != 0 || (m.opt.integrator == INT_IMPLICITFAST && !is_free_body(m, m.body_rootid[bodyid] as usize)) {
            let input = b;
            blas::symmetrize(&mut b, &input, 6);
        }
        if core_util::is_sparse(m) != 0 {
            add_jtbjsparse(m, d, res, &j_mat, &b, 6, 0, &rownnz, &rowadr, &colind_compressed);
        } else { add_jtbj(m, d, res, &j_mat, &b, 6); }
    }
    d.scratch.put_num(j_mat);
    d.scratch.put_num(tmp);
    d.scratch.put_int(colind);
    d.scratch.put_int(colind_compressed);
}

pub fn d_inertia_box_fluid(m: &Model, d: &mut Data, res: &mut [f64], i: usize) {
    let nv = m.nv;
    let mut rownnz = [0; 6];
    let mut rowadr = [0; 6];
    // J and j collide under snake case: J is j_mat.
    let mut j_mat = d.scratch.num(6*nv);
    let mut tmp = d.scratch.num(3*nv);
    let mut colind = d.scratch.int(6*nv);
    let mut lvel = [0.0; 6];
    let mut wind = [0.0; 6];
    let mut lwind = [0.0; 6];
    let mut box_ = [0.0; 3]; // box is a Rust keyword.
    let mut b;
    let inertia = &m.body_inertia[3*i..];
    box_[0] = (max(MINVAL, inertia[1]+inertia[2]-inertia[0]) / m.body_mass[i] * 6.0).sqrt();
    box_[1] = (max(MINVAL, inertia[0]+inertia[2]-inertia[1]) / m.body_mass[i] * 6.0).sqrt();
    box_[2] = (max(MINVAL, inertia[0]+inertia[1]-inertia[2]) / m.body_mass[i] * 6.0).sqrt();
    core_util::object_velocity(m, d, OBJ_BODY, i, &mut lvel, 1);
    blas::zero(&mut wind, 6);
    blas::copy3(at3_mut(&mut wind, 3), &m.opt.wind);
    spatial::transform_spatial(&mut lwind, &wind, 0, at3(&d.xipos, 3*i), at3(&d.subtree_com, 3*m.body_rootid[i] as usize), Some(at9(&d.ximat, 9*i)));
    blas::sub_from3(at3_mut(&mut lvel, 3), at3(&lwind, 3));
    let mut nnz = nv;
    if core_util::is_sparse(m) != 0 {
        nnz = core_util::body_chain(m, i, &mut colind);
        let (jr, jp) = j_mat.split_at_mut(3*nnz);
        core_util::jac_sparse(m, d, Some(jp), Some(jr), at3(&d.xipos, 3*i), i, nnz, &colind, 0);
        rownnz[0] = nnz as i32;
        rowadr[0] = 0;
        for j in 1..6 {
            rownnz[j] = nnz as i32;
            rowadr[j] = rowadr[j-1]+nnz as i32;
            for k in 0..nnz { colind[j*nnz+k] = colind[k]; }
        }
    } else {
        let (jr, jp) = j_mat.split_at_mut(3*nv);
        core_util::jac_body_com(m, d, Some(jp), Some(jr), i);
    }
    blas::mul_mat_tmat(&mut tmp, &d.ximat[9*i..], &j_mat, 3, 3, nnz);
    blas::copy(&mut j_mat, &tmp, 3*nnz);
    blas::mul_mat_tmat(&mut tmp, &d.ximat[9*i..], &j_mat[3*nnz..], 3, 3, nnz);
    blas::copy(&mut j_mat[3*nnz..], &tmp, 3*nnz);
    if m.opt.viscosity > 0.0 {
        let diam = (box_[0]+box_[1]+box_[2])/3.0;
        b = -PI*diam*diam*diam*m.opt.viscosity;
        for j in 0..3 {
            if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, j, &rownnz, &rowadr, &colind); }
            else { add_jtbj(m, d, res, &j_mat[j*nv..], &[b], 1); }
        }
        b = -3.0*PI*diam*m.opt.viscosity;
        for j in 0..3 {
            if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 3+j, &rownnz, &rowadr, &colind); }
            else { add_jtbj(m, d, res, &j_mat[3*nv+j*nv..], &[b], 1); }
        }
    }
    if m.opt.density > 0.0 {
        b = -m.opt.density*box_[0]*(box_[1]*box_[1]*box_[1]*box_[1]+box_[2]*box_[2]*box_[2]*box_[2])*2.0*lvel[0].abs()/64.0;
        if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 0, &rownnz, &rowadr, &colind); }
        else { add_jtbj(m, d, res, &j_mat, &[b], 1); }
        b = -m.opt.density*box_[1]*(box_[0]*box_[0]*box_[0]*box_[0]+box_[2]*box_[2]*box_[2]*box_[2])*2.0*lvel[1].abs()/64.0;
        if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 1, &rownnz, &rowadr, &colind); }
        else { add_jtbj(m, d, res, &j_mat[nv..], &[b], 1); }
        b = -m.opt.density*box_[2]*(box_[0]*box_[0]*box_[0]*box_[0]+box_[1]*box_[1]*box_[1]*box_[1])*2.0*lvel[2].abs()/64.0;
        if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 2, &rownnz, &rowadr, &colind); }
        else { add_jtbj(m, d, res, &j_mat[2*nv..], &[b], 1); }
        b = -0.5*m.opt.density*box_[1]*box_[2]*2.0*lvel[3].abs();
        if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 3, &rownnz, &rowadr, &colind); }
        else { add_jtbj(m, d, res, &j_mat[3*nv..], &[b], 1); }
        b = -0.5*m.opt.density*box_[0]*box_[2]*2.0*lvel[4].abs();
        if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 4, &rownnz, &rowadr, &colind); }
        else { add_jtbj(m, d, res, &j_mat[4*nv..], &[b], 1); }
        b = -0.5*m.opt.density*box_[0]*box_[1]*2.0*lvel[5].abs();
        if core_util::is_sparse(m) != 0 { add_jtbjsparse(m, d, res, &j_mat, &[b], 1, 5, &rownnz, &rowadr, &colind); }
        else { add_jtbj(m, d, res, &j_mat[5*nv..], &[b], 1); }
    }
    d.scratch.put_num(j_mat);
    d.scratch.put_num(tmp);
    d.scratch.put_int(colind);
}

pub fn d_passive_vel(m: &Model, d: &mut Data) {
    if m.opt.disableflags & DSBL_SPRING != 0 && m.opt.disableflags & DSBL_DAMPER != 0 { return; }
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.ntree_awake < m.ntree as i32;
    let nbody = if sleep_filter { d.nbody_awake as usize } else { m.nbody };
    if m.opt.viscosity > 0.0 || m.opt.density > 0.0 {
        for b in 0..nbody {
            let i = if sleep_filter { d.body_awake_ind[b] as usize } else { b };
            if m.body_mass[i] < MINVAL { continue; }
            let mut use_ellipsoid_model = 0;
            let mut j = 0;
            while j < m.body_geomnum[i] as usize && use_ellipsoid_model == 0 {
                let geomid = m.body_geomadr[i] as usize+j;
                use_ellipsoid_model += (m.geom_fluid[NFLUID*geomid] > 0.0) as i32;
                j += 1;
            }
            // Move the output Vec out while Data's scratch is used; no allocation or copy.
            let mut q_deriv = std::mem::take(&mut d.q_deriv);
            if use_ellipsoid_model != 0 { d_ellipsoid_fluid(m, d, &mut q_deriv, i, 0); }
            else { d_inertia_box_fluid(m, d, &mut q_deriv, i); }
            d.q_deriv = q_deriv;
        }
    }
    if m.opt.disableflags & DSBL_DAMPER != 0 { return; }
    let nv = m.nv;
    let nv_awake = if sleep_filter { d.nv_awake as usize } else { nv };
    for j in 0..nv_awake {
        let i = if sleep_filter { d.dof_awake_ind[j] as usize } else { j };
        let v = d.qvel[i];
        let mut poly = [0.0; NPOLY];
        blas::copy(&mut poly, &m.dof_dampingpoly[NPOLY*i..], NPOLY);
        let damping = m.dof_damping[i] + core_util::actuator_damping(m, OBJ_JOINT, m.dof_jntid[i] as usize, &mut poly);
        let adr = (m.d_rowadr[i]+m.d_diag[i]) as usize;
        d.q_deriv[adr] -= d_x_poly_force(damping, &poly, v, NPOLY, 1);
    }
    // flex_edgedamping is intentionally omitted in C: tree-only D sparsity would
    // drop off-diagonal blocks between flexcomp trees, breaking translational invariance.
    let ntendon = m.ntendon;
    for i in 0..ntendon {
        if sleep_filter {
            let treenum = m.tendon_treenum[i];
            let id1 = m.tendon_treeid[2*i];
            if treenum == 1 && d.tree_awake[id1 as usize] == 0 { continue; }
            let id2 = m.tendon_treeid[2*i+1];
            if treenum == 2 && d.tree_awake[id1 as usize] == 0 && d.tree_awake[id2 as usize] == 0 { continue; }
        }
        let v = d.ten_velocity[i];
        let mut poly = [0.0; NPOLY];
        blas::copy(&mut poly, &m.tendon_dampingpoly[NPOLY*i..], NPOLY);
        let damping = m.tendon_damping[i] + core_util::actuator_damping(m, OBJ_TENDON, i, &mut poly);
        let b = -d_x_poly_force(damping, &poly, v, NPOLY, 1);
        if b == 0.0 { continue; }
        let mut q_deriv = std::mem::take(&mut d.q_deriv);
        add_jtbjsparse(m, d, &mut q_deriv, &d.ten_j, &[b], 1, i, &m.ten_j_rownnz, &m.ten_j_rowadr, &m.ten_j_colind);
        d.q_deriv = q_deriv;
    }
}

// d(qfrc_actuator + qfrc_passive - [qfrc_bias]) / d(qvel).
pub fn d_smooth_vel(m: &Model, d: &mut Data, flg_bias: i32) {
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && d.nv_awake < m.nv as i32;
    if !sleep_filter { blas::zero(&mut d.q_deriv, m.n_d); }
    else { zero_sparse(&mut d.q_deriv, &m.d_rownnz, &m.d_rowadr, &d.dof_awake_ind, d.nv_awake as usize); }
    d_actuator_vel(m, d);
    d_passive_vel(m, d);
    if flg_bias != 0 { d_rne_vel(m, d); }
}
