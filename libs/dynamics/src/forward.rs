//! Forward dynamics, constraint stages and time integration.
//!
//! Translated from `src/engine/engine_forward.c` and `engine_forward.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::collision_driver::collision;
use crate::core_constraint as constraint;
use crate::core_smooth as smooth;
use crate::core_util;
use crate::data::Data;
use crate::derivative;
use crate::inline::{self, at3, at3_mut, at4, at4_mut, at6};
use crate::inverse;
use crate::io::reset_data;
use crate::island::island;
use crate::metric;
use crate::model::{Model, disabled, enabled};
use crate::passive::passive;
use crate::sensor;
use crate::sleep;
use crate::solver;
use crate::support;
use crate::thread;
use crate::types::*;
use crate::util_blas as blas;
use crate::util_errmem::warning;
use crate::util_misc as misc;
use crate::util_solve as solve;
use crate::util_sparse as sparse;
use crate::util_spatial as spatial;

pub fn check_pos(m: &Model, d: &mut Data) {
    for i in 0..m.nq {
        if misc::is_bad(d.qpos[i]) != 0 {
            warning(d, WARN_BADQPOS, i as i32);
            if !disabled(m, DSBL_AUTORESET) {
                reset_data(m, d);
                d.warning[WARN_BADQPOS as usize].number += 1;
                d.warning[WARN_BADQPOS as usize].lastinfo = i as i32;
            }
            return;
        }
    }
}

pub fn check_vel(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nv = if sleep_filter { d.nv_awake as usize } else { m.nv };
    for j in 0..nv {
        let i = if sleep_filter { d.dof_awake_ind[j] as usize } else { j };
        if misc::is_bad(d.qvel[i]) != 0 {
            warning(d, WARN_BADQVEL, i as i32);
            if !disabled(m, DSBL_AUTORESET) {
                reset_data(m, d);
                d.warning[WARN_BADQVEL as usize].number += 1;
                d.warning[WARN_BADQVEL as usize].lastinfo = i as i32;
            }
            return;
        }
    }
}

pub fn check_acc(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nv = if sleep_filter { d.nv_awake as usize } else { m.nv };
    for j in 0..nv {
        let i = if sleep_filter { d.dof_awake_ind[j] as usize } else { j };
        if misc::is_bad(d.qacc[i]) != 0 {
            warning(d, WARN_BADQACC, i as i32);
            if !disabled(m, DSBL_AUTORESET) {
                reset_data(m, d);
                d.warning[WARN_BADQACC as usize].number += 1;
                d.warning[WARN_BADQACC as usize].lastinfo = i as i32;
                forward(m, d);
            }
            return;
        }
    }
}

pub fn fwd_kinematics(m: &Model, d: &mut Data) {
    smooth::kinematics(m, d);
    smooth::com_pos(m, d);
    smooth::camlight(m, d);
    smooth::flex(m, d);
    smooth::tendon(m, d);
    if sleep::wake_tendon(m, d) != 0 { sleep::update_sleep(m, d); }
}

pub fn fwd_position(m: &Model, d: &mut Data) {
    d.flg_energypos = 0;
    fwd_kinematics(m, d);
    smooth::make_m(m, d);
    smooth::factor_m(m, d);
    collision(m, d);
    if sleep::wake_collision(m, d) != 0 {
        sleep::update_sleep(m, d);
        collision(m, d);
    }
    if sleep::wake_equality(m, d) != 0 { sleep::update_sleep(m, d); }
    constraint::make_constraint(m, d);
    island(m, d);
    constraint::project_constraint(m, d);
    smooth::transmission(m, d);
    metric::eff_build(m, d, core_util::is_metric(m), 1);
}

pub fn fwd_velocity(m: &Model, d: &mut Data) {
    d.flg_subtreevel = 0;
    d.flg_energyvel = 0;
    blas::zero(&mut d.flexedge_velocity, m.nflexedge);
    for f in 0..m.nflex {
        if m.flex_rigid[f] != 0 || m.flex_interp[f] != 0 { continue; }
        let adr = m.flex_edgeadr[f] as usize;
        let num = m.flex_edgenum[f] as usize;
        sparse::mul_mat_vec_sparse(&mut d.flexedge_velocity[adr..], &d.flexedge_j, &d.qvel,
            num, &m.flexedge_j_rownnz[adr..], &m.flexedge_j_rowadr[adr..], &m.flexedge_j_colind, None);
    }
    sparse::mul_mat_vec_sparse(&mut d.ten_velocity, &d.ten_j, &d.qvel, m.ntendon,
        &m.ten_j_rownnz, &m.ten_j_rowadr, &m.ten_j_colind, None);
    if !disabled(m, DSBL_ACTUATION) {
        sparse::mul_mat_vec_sparse(&mut d.actuator_velocity, &d.actuator_moment, &d.qvel,
            m.nout, &d.moment_rownnz, &d.moment_rowadr, &d.moment_colind, None);
    } else { blas::zero(&mut d.actuator_velocity, m.nout); }
    smooth::com_vel(m, d);
    passive(m, d);
    if core_util::is_metric(m) != 0 { constraint::velocity_constraint(m, d); }
    else { constraint::reference_constraint(m, d); }
    // None is the existing API's alias for d.qfrc_bias, including sleep filtering.
    smooth::rne(m, d, 0, None);
    smooth::tendon_bias(m, d, None);
    metric::eff_shift(m, d);
}

fn unpack_servo_inputs(u: &[f64], spec: i32, out: &mut [f64; 4]) {
    let mut adr = 0;
    out[0] = if spec & INPUT_POS != 0 { let v = u[adr]; adr += 1; v } else { 0.0 };
    out[1] = if spec & INPUT_VEL != 0 { let v = u[adr]; adr += 1; v } else { 0.0 };
    out[2] = if spec & INPUT_FF != 0 { let v = u[adr]; adr += 1; v } else { 0.0 };
    out[3] = if spec & INPUT_VOLTAGE != 0 { u[adr] } else { 0.0 };
}

fn dcmotor_voltage(u: &[f64], spec: i32, length: f64, velocity: f64, x_i: f64, gainprm: &[f64]) -> f64 {
    let mut voltage = 0.0;
    let mut u4 = [0.0; 4];
    unpack_servo_inputs(u, spec, &mut u4);
    if spec & (INPUT_POS | INPUT_VEL | INPUT_FF) != 0 {
        let kp = gainprm[4];
        let ki = gainprm[5];
        let kd = gainprm[6];
        let torque = kp*(u4[0] - length) + kd*(u4[1] - velocity) + ki*x_i + u4[2];
        let r = gainprm[0];
        let k = gainprm[1];
        voltage = r/k * torque + k*velocity;
        let vmax = gainprm[7];
        if vmax > 0.0 { voltage = misc::clip(voltage, -vmax, vmax); }
    }
    voltage + u4[3]
}

fn clamp_vec(vec: &mut [f64], range: &[f64], limited: &[u8], n: usize, index: Option<&[i32]>) {
    for i in 0..n {
        let j = index.map_or(i, |index| index[i] as usize);
        if limited[i] != 0 { vec[j] = misc::clip(vec[j], range[2*i], range[2*i+1]); }
    }
}

fn expmap2quat(quat: &mut [f64; 4], v: &[f64; 3]) {
    let angle = blas::norm3(v);
    if angle < MINVAL {
        *quat = [1.0, 0.0, 0.0, 0.0];
    } else {
        let axis = [v[0]/angle, v[1]/angle, v[2]/angle];
        inline::axis_angle2quat(quat, &axis, angle);
    }
}

fn wrap_period(m: &Model, i: usize) -> f64 {
    let dyntype = m.actuator_dyntype[i];
    let servo = m.actuator_gaintype[i] == GAIN_FIXED && m.actuator_biastype[i] == BIAS_AFFINE
        && m.actuator_gainprm[NGAIN*i] == -m.actuator_biasprm[NBIAS*i+1]
        && (dyntype == DYN_NONE || dyntype == DYN_INTEGRATOR || dyntype == DYN_FILTER || dyntype == DYN_FILTEREXACT);
    let pid = m.actuator_gaintype[i] == GAIN_PID;
    if !servo && !pid { return 0.0; }
    let gear = &m.actuator_gear[6*m.actuator_outadr[i] as usize..];
    let trntype = m.actuator_trntype[i];
    if trntype == TRN_SITE && m.actuator_trnid[2*i+1] >= 0 && gear[0] == 0.0 && gear[1] == 0.0 && gear[2] == 0.0 {
        return 2.0*PI * blas::norm3(at3(gear, 3));
    }
    if (trntype == TRN_JOINT || trntype == TRN_JOINTINPARENT) && m.jnt_type[m.actuator_trnid[2*i] as usize] == JNT_BALL {
        return 2.0*PI * blas::norm3(at3(gear, 0));
    }
    0.0
}

fn wrap_setpoint(u: f64, length: f64, period: f64) -> f64 {
    let err = u - length;
    u - period * misc::round(err / period) as f64
}

fn slew_limit(mut u: f64, u_prev: f64, slew_s: f64, dt: f64, period: f64, act_dot: &mut f64) -> f64 {
    if period > 0.0 { u = wrap_setpoint(u, u_prev, period); }
    let slew = slew_s * dt;
    let u_eff = misc::clip(u, u_prev - slew, u_prev + slew);
    *act_dot = (u_eff - u_prev) / dt;
    u_eff
}

pub fn fwd_actuation(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nu = m.nu;
    let nactuator = m.nactuator;
    let nout = m.nout;
    let ntendon = m.ntendon;
    blas::zero(&mut d.actuator_force, nout);
    let sleep_filter = enabled(m, ENBL_SLEEP);
    if nactuator == 0 || disabled(m, DSBL_ACTUATION) {
        blas::zero(&mut d.qfrc_actuator, nv);
        return;
    }
    let mut tendon_frclimited = false;
    let mut ctrl = d.scratch.num(nu);
    blas::copy(&mut ctrl, &d.ctrl, nu);
    // delayed actuators read their controls from their history buffers
    for i in 0..nactuator {
        if m.actuator_delay[i] != 0.0 {
            let adr = m.actuator_ctrladr[i] as usize;
            let num = m.actuator_ctrlnum[i] as usize;
            if let Some(p) = support::read_ctrl(m, d, i, d.time, &mut ctrl[adr..adr + num], -1) {
                blas::copy(&mut ctrl[adr..], p, num);
            }
        }
    }
    if !disabled(m, DSBL_CLAMPCTRL) {
        clamp_vec(&mut ctrl, &m.actuator_ctrlrange, &m.actuator_ctrllimited, nu, None);
    }
    for i in 0..nu {
        if misc::is_bad(ctrl[i]) != 0 {
            warning(d, WARN_BADCTRL, i as i32);
            blas::zero(&mut ctrl, nu);
            break;
        }
    }
    for i in 0..nactuator {
        if sleep_filter && sleep::sleep_state(m, d, OBJ_ACTUATOR, i as i32) == S_ASLEEP { continue; }
        let act_first = m.actuator_actadr[i];
        if act_first < 0 { continue; }
        let act_first = act_first as usize;
        let uadr = m.actuator_ctrladr[i] as usize;
        let oadr = m.actuator_outadr[i] as usize;
        let actnum = m.actuator_actnum[i] as usize;
        if actnum != 0 { blas::zero(&mut d.act_dot[act_first..], actnum); }
        let dynprm = &m.actuator_dynprm[i*NDYN..];
        let act_last = act_first + actnum - 1;
        match m.actuator_dyntype[i] {
            DYN_INTEGRATOR => {
                let num = m.actuator_ctrlnum[i] as usize;
                for j in 0..num { d.act_dot[act_last+1-num+j] = ctrl[uadr+j]; }
            }
            DYN_FILTER | DYN_FILTEREXACT => {
                let period = wrap_period(m, i);
                let u = if period > 0.0 { wrap_setpoint(ctrl[uadr], d.act[act_last], period) } else { ctrl[uadr] };
                let tau = misc::max(MINVAL, dynprm[0]);
                d.act_dot[act_last] = (u - d.act[act_last]) / tau;
            }
            DYN_MUSCLE => { d.act_dot[act_last] = misc::muscle_dynamics(ctrl[uadr], d.act[act_last], at3(dynprm, 0)); }
            DYN_PID => {
                let mut adr = act_first;
                let period = wrap_period(m, i);
                let slew_s = dynprm[1];
                if slew_s > 0.0 {
                    ctrl[uadr] = slew_limit(ctrl[uadr], d.act[adr], slew_s, m.opt.timestep, period, &mut d.act_dot[adr]);
                    adr += 1;
                }
                if m.actuator_gainprm[NGAIN*i] > 0.0 {
                    let mut err = ctrl[uadr] - d.actuator_length[oadr];
                    if period > 0.0 { err -= period*misc::round(err/period) as f64; }
                    let imax = dynprm[0];
                    if imax > 0.0 {
                        let z = d.act[adr];
                        if z >= imax { err = misc::min(err, 0.0); }
                        else if z <= -imax { err = misc::max(err, 0.0); }
                    }
                    d.act_dot[adr] = err;
                }
            }
            DYN_DCMOTOR => {
                let gainprm = &m.actuator_gainprm[NGAIN*i..];
                if misc::dcmotor_slots(dynprm, gainprm).num_slots != actnum as i32 {
                    panic!("inconsistent state array dimension in DC motor (actuator {})", i);
                }
                let mut adr = act_first;
                let velocity = d.actuator_velocity[oadr];
                let mut r = gainprm[0];
                let k = gainprm[1];
                let ki = gainprm[5];
                let te = dynprm[0];
                let slew_s = dynprm[7];
                if slew_s > 0.0 {
                    ctrl[uadr] = slew_limit(ctrl[uadr], d.act[adr], slew_s, m.opt.timestep, 0.0, &mut d.act_dot[adr]);
                    adr += 1;
                }
                let mut x_i = 0.0;
                if ki > 0.0 {
                    x_i = d.act[adr];
                    let imax = dynprm[8];
                    let mut act_dot = ctrl[uadr] - d.actuator_length[oadr];
                    if imax > 0.0 {
                        if x_i >= imax { act_dot = misc::min(act_dot, 0.0); }
                        else if x_i <= -imax { act_dot = misc::max(act_dot, 0.0); }
                    }
                    d.act_dot[adr] = act_dot;
                    adr += 1;
                }
                let v = dcmotor_voltage(&ctrl[uadr..], m.actuator_ctrlspec[i], d.actuator_length[oadr], velocity, x_i, gainprm);
                let rt = dynprm[2];
                if rt > 0.0 {
                    let c = dynprm[3];
                    let ta = dynprm[4];
                    let alpha = gainprm[2];
                    let t0 = gainprm[3];
                    let t = d.act[adr];
                    r *= 1.0 + alpha * (t + ta - t0);
                    let current = if te > 0.0 { d.act[act_last] } else { (v - k * velocity) / r };
                    d.act_dot[adr] = (r*current*current - t / rt) / c;
                    adr += 1;
                }
                let sigma0 = dynprm[5];
                if sigma0 > 0.0 {
                    let biasprm = &m.actuator_biasprm[NBIAS*i..];
                    let f_c = biasprm[3];
                    let f_s = biasprm[4];
                    let v_s = biasprm[5];
                    let z = d.act[adr];
                    let g = misc::lugre_stribeck(velocity, f_c, f_s, v_s);
                    let a = -sigma0 * velocity.abs() / misc::max(MINVAL, g);
                    d.act_dot[adr] = a * z + velocity;
                    // C increments adr here; no subsequent access uses it.
                }
                if te > 0.0 {
                    let dimax = dynprm[1];
                    let mut i_dot = (v/r - k/r*velocity - d.act[act_last]) / te;
                    if dimax > 0.0 { i_dot = misc::clip(i_dot, -dimax, dimax); }
                    d.act_dot[act_last] = i_dot;
                }
            }
            _ => { /* mjcb_act_dyn is unset. */ }
        }
    }
    // SDF-only plugins have no actuator_act_dot callback or ACTUATOR capability.
    for i in 0..nactuator {
        if m.actuator_plugin[i] >= 0 { crate::unported!("plugin->actuator_act_dot"); }
    }
    for i in 0..nactuator {
        if sleep_filter && sleep::sleep_state(m, d, OBJ_ACTUATOR, i as i32) == S_ASLEEP { continue; }
        if support::actuator_disabled(m, i) != 0 { continue; }
        if m.actuator_plugin[i] >= 0 { continue; }
        let uadr = m.actuator_ctrladr[i] as usize;
        let oadr = m.actuator_outadr[i] as usize;
        if m.actuator_gaintype[i] == GAIN_SO3 {
            let mut q_tgt = [0.0; 4];
            if m.actuator_ctrlspec[i] == CHART_QUAT {
                blas::copy4(&mut q_tgt, at4(&ctrl, uadr));
                blas::normalize4(&mut q_tgt);
            } else {
                let mut u = [0.0; 3];
                if m.actuator_dyntype[i] == DYN_NONE { blas::copy3(&mut u, at3(&ctrl, uadr)); }
                else {
                    let act_adr = m.actuator_actadr[i] as usize;
                    if m.actuator_actearly[i] != 0 {
                        for k in 0..3 { u[k] = support::next_activation(m, d, i, act_adr+k, d.act_dot[act_adr+k]); }
                    } else { blas::copy3(&mut u, at3(&d.act, act_adr)); }
                }
                expmap2quat(&mut q_tgt, &u);
            }
            let mut q_cur = [0.0; 4];
            let mut e = [0.0; 3];
            expmap2quat(&mut q_cur, at3(&d.actuator_length, oadr));
            inline::sub_quat(&mut e, &q_tgt, &q_cur);
            let kp = m.actuator_gainprm[NGAIN*i];
            let prm = &m.actuator_biasprm[NBIAS*i..];
            for k in 0..3 { d.actuator_force[oadr+k] = kp*e[k] + prm[0] + prm[2]*d.actuator_velocity[oadr+k]; }
            continue;
        }
        if ntendon != 0 && !tendon_frclimited && m.actuator_trntype[i] == TRN_TENDON {
            tendon_frclimited = m.tendon_actfrclimited[m.actuator_trnid[2*i] as usize] != 0;
        }
        let dynprm = &m.actuator_dynprm[NDYN*i..];
        let gainprm = &m.actuator_gainprm[NGAIN*i..];
        let gaintype = m.actuator_gaintype[i];
        let actnum = m.actuator_actnum[i] as usize;
        let gain = match gaintype {
            GAIN_FIXED => gainprm[0],
            GAIN_PID => 0.0,
            GAIN_AFFINE => gainprm[0] + gainprm[1]*d.actuator_length[oadr] + gainprm[2]*d.actuator_velocity[oadr],
            GAIN_MUSCLE => misc::muscle_gain(d.actuator_length[oadr], d.actuator_velocity[oadr],
                m.actuator_lengthrange[2*oadr..2*oadr+2].try_into().unwrap(), m.actuator_acc0[oadr], gainprm[..9].try_into().unwrap()),
            GAIN_DCMOTOR => {
                let r = core_util::dcmotor_resistance(m, d, i);
                let k = gainprm[1];
                let slots = misc::dcmotor_slots(dynprm, gainprm);
                if slots.num_slots != actnum as i32 { panic!("inconsistent state array dimension in DC motor (actuator {})", i); }
                let gain = if dynprm[0] > 0.0 { k } else { k / r };
                if m.actuator_ctrlspec[i] != INPUT_VOLTAGE && m.actuator_ctrlnum[i] > 0 {
                    let x_i = if slots.integral >= 0 { d.act[(m.actuator_actadr[i]+slots.integral) as usize] } else { 0.0 };
                    ctrl[uadr] = dcmotor_voltage(&ctrl[uadr..], m.actuator_ctrlspec[i], d.actuator_length[oadr], d.actuator_velocity[oadr], x_i, gainprm);
                }
                gain
            }
            GAIN_SO3 => panic!("mjGAIN_SO3 reached SISO switch (actuator {})", i),
            _ => 1.0, // mjcb_act_gain is unset.
        };
        let dcmotor_no_current = gaintype == GAIN_DCMOTOR && dynprm[0] <= 0.0;
        if gaintype == GAIN_PID {
            let prm = &m.actuator_biasprm[NBIAS*i..];
            let mut u4 = [0.0; 4];
            unpack_servo_inputs(&ctrl[uadr..], m.actuator_ctrlspec[i], &mut u4);
            let mut qref = u4[0];
            let vref = u4[1];
            let ff = u4[2];
            let period = wrap_period(m, i);
            if period > 0.0 { qref = wrap_setpoint(qref, d.actuator_length[oadr], period); }
            d.actuator_force[oadr] = -prm[1]*qref - prm[2]*vref + ff;
            if actnum != 0 && gainprm[0] > 0.0 {
                let act_adr = m.actuator_actadr[i] as usize + actnum - 1;
                let z = if m.actuator_actearly[i] != 0 { support::next_activation(m, d, i, act_adr, d.act_dot[act_adr]) } else { d.act[act_adr] };
                d.actuator_force[oadr] += gainprm[0]*z;
            }
        } else if actnum == 0 || dcmotor_no_current {
            let mut input = if m.actuator_ctrlnum[i] != 0 { ctrl[uadr] } else { 0.0 };
            let period = wrap_period(m, i);
            if period > 0.0 { input = wrap_setpoint(input, d.actuator_length[oadr], period); }
            d.actuator_force[oadr] = gain * input;
        } else {
            let act_adr = m.actuator_actadr[i] as usize + actnum - 1;
            let mut act = if m.actuator_actearly[i] != 0 { support::next_activation(m, d, i, act_adr, d.act_dot[act_adr]) } else { d.act[act_adr] };
            let period = wrap_period(m, i);
            if period > 0.0 { act = wrap_setpoint(act, d.actuator_length[oadr], period); }
            d.actuator_force[oadr] = gain * act;
        }
        let biasprm = &m.actuator_biasprm[NBIAS*i..];
        let bias = match m.actuator_biastype[i] {
            BIAS_NONE => 0.0,
            BIAS_AFFINE => biasprm[0] + biasprm[1]*d.actuator_length[oadr] + biasprm[2]*d.actuator_velocity[oadr],
            BIAS_MUSCLE => misc::muscle_bias(d.actuator_length[oadr], m.actuator_lengthrange[2*oadr..2*oadr+2].try_into().unwrap(), m.actuator_acc0[oadr], biasprm[..9].try_into().unwrap()),
            BIAS_DCMOTOR => {
                let mut bias = 0.0;
                let te = m.actuator_dynprm[NDYN*i];
                if te <= 0.0 { let k = gainprm[1]; bias -= gain * k * d.actuator_velocity[oadr]; }
                bias
            }
            _ => 0.0, // mjcb_act_bias is unset.
        };
        d.actuator_force[oadr] += bias;
    }
    // SDF-only plugins have no ACTUATOR compute callback.
    for i in 0..nactuator {
        if m.actuator_plugin[i] >= 0 { crate::unported!("plugin->compute (mjPLUGIN_ACTUATOR)"); }
    }
    if tendon_frclimited {
        let mut tendon_total_force = d.scratch.num(ntendon);
        blas::zero(&mut tendon_total_force, ntendon);
        for i in 0..nactuator {
            if m.actuator_trntype[i] == TRN_TENDON {
                let tendon_id = m.actuator_trnid[2*i] as usize;
                if m.tendon_actfrclimited[tendon_id] != 0 { tendon_total_force[tendon_id] += d.actuator_force[m.actuator_outadr[i] as usize]; }
            }
        }
        for i in 0..nactuator {
            if m.actuator_trntype[i] != TRN_TENDON { continue; }
            let tendon_id = m.actuator_trnid[2*i] as usize;
            let tendon_force = tendon_total_force[tendon_id];
            if m.tendon_actfrclimited[tendon_id] != 0 && tendon_force != 0.0 {
                let range = &m.tendon_actfrcrange[2*tendon_id..];
                if tendon_force < range[0] { d.actuator_force[m.actuator_outadr[i] as usize] *= range[0] / tendon_force; }
                else if tendon_force > range[1] { d.actuator_force[m.actuator_outadr[i] as usize] *= range[1] / tendon_force; }
            }
        }
        d.scratch.put_num(tendon_total_force);
    }
    for i in 0..nactuator {
        if m.actuator_forcelimited[i] == 0 { continue; }
        let range = &m.actuator_forcerange[2*i..];
        let f = &mut d.actuator_force[m.actuator_outadr[i] as usize..];
        if m.actuator_gaintype[i] == GAIN_SO3 {
            let norm = blas::norm3(at3(f, 0));
            if norm > range[1] { let old = *at3(f, 0); blas::scl3(at3_mut(f, 0), &old, range[1]/norm); }
        } else {
            for j in 0..m.actuator_outnum[i] as usize { f[j] = misc::clip(f[j], range[0], range[1]); }
        }
    }
    for i in 0..nactuator {
        if m.actuator_biastype[i] != BIAS_DCMOTOR { continue; }
        if sleep_filter && sleep::sleep_state(m, d, OBJ_ACTUATOR, i as i32) == S_ASLEEP { continue; }
        if support::actuator_disabled(m, i) != 0 || m.actuator_plugin[i] >= 0 { continue; }
        let biasprm = &m.actuator_biasprm[NBIAS*i..];
        let dynprm = &m.actuator_dynprm[NDYN*i..];
        let oadr = m.actuator_outadr[i] as usize;
        let a = biasprm[0];
        if a != 0.0 {
            let np = biasprm[1];
            let phi = biasprm[2];
            d.actuator_force[oadr] += a * (np*d.actuator_length[oadr] + phi).sin();
        }
        let sigma0 = dynprm[5];
        if sigma0 > 0.0 {
            let sigma1 = dynprm[6];
            let slots = misc::dcmotor_slots(dynprm, &m.actuator_gainprm[NGAIN*i..]);
            let adr = (m.actuator_actadr[i] + slots.bristle) as usize;
            let z = d.act[adr];
            let z_dot = d.act_dot[adr];
            d.actuator_force[oadr] -= sigma0 * z + sigma1 * z_dot;
        }
    }
    sparse::mul_mat_tvec_sparse(&mut d.qfrc_actuator, &d.actuator_moment, &d.actuator_force, nout, nv,
        &d.moment_rownnz, &d.moment_rowadr, &d.moment_colind);
    if m.flg_gravcomp != 0 && !disabled(m, DSBL_GRAVITY) && blas::norm3(&m.opt.gravity) != 0.0 {
        const JNT_DOFNUM: [usize; 4] = [6, 3, 1, 1];
        for i in 0..m.njnt {
            if m.jnt_actgravcomp[i] == 0 { continue; }
            let dofnum = JNT_DOFNUM[m.jnt_type[i] as usize];
            let dofadr = m.jnt_dofadr[i] as usize;
            blas::add_to(&mut d.qfrc_actuator[dofadr..], &d.qfrc_gravcomp[dofadr..], dofnum);
        }
    }
    clamp_vec(&mut d.qfrc_actuator, &m.jnt_actfrcrange, &m.jnt_actfrclimited, m.njnt, Some(&m.jnt_dofadr));
    d.scratch.put_num(ctrl);
}

pub fn fwd_acceleration(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nv = if sleep_filter { d.nv_awake as usize } else { m.nv };
    if !sleep_filter {
        blas::sub(&mut d.qfrc_smooth, &d.qfrc_passive, &d.qfrc_bias, nv);
        blas::add_to(&mut d.qfrc_smooth, &d.qfrc_applied, nv);
        blas::add_to(&mut d.qfrc_smooth, &d.qfrc_actuator, nv);
    } else {
        blas::sub_ind(&mut d.qfrc_smooth, &d.qfrc_passive, &d.qfrc_bias, &d.dof_awake_ind, nv);
        blas::add_to_ind(&mut d.qfrc_smooth, &d.qfrc_applied, &d.dof_awake_ind, nv);
        blas::add_to_ind(&mut d.qfrc_smooth, &d.qfrc_actuator, &d.dof_awake_ind, nv);
    }
    let mut qfrc_smooth = std::mem::take(&mut d.qfrc_smooth);
    support::xfrc_accumulate(m, d, &mut qfrc_smooth);
    d.qfrc_smooth = qfrc_smooth;
    if d.efm_active != 0 {
        let mut qfrc_eff = d.scratch.num(m.nv);
        if !sleep_filter {
            blas::add(&mut qfrc_eff, &d.qfrc_smooth, &d.efm_c, nv);
            if !d.efm_ca.is_empty() { blas::add_to(&mut qfrc_eff, &d.efm_ca, nv); }
            let mut qacc_smooth = std::mem::take(&mut d.qacc_smooth);
            metric::eff_solve(m, d, &mut qacc_smooth, Some(&qfrc_eff));
            d.qacc_smooth = qacc_smooth;
        } else {
            blas::zero(&mut qfrc_eff, m.nv);
            blas::add_ind(&mut qfrc_eff, &d.qfrc_smooth, &d.efm_c, &d.dof_awake_ind, nv);
            if !d.efm_ca.is_empty() { blas::add_to_ind(&mut qfrc_eff, &d.efm_ca, &d.dof_awake_ind, nv); }
            let mut qacc_eff = d.scratch.num(m.nv);
            metric::eff_solve(m, d, &mut qacc_eff, Some(&qfrc_eff));
            blas::copy_ind(&mut d.qacc_smooth, &qacc_eff, &d.dof_awake_ind, nv);
            d.scratch.put_num(qacc_eff);
        }
        d.scratch.put_num(qfrc_eff);
        return;
    }
    if !sleep_filter { blas::copy(&mut d.qacc_smooth, &d.qfrc_smooth, nv); }
    else { blas::copy_ind(&mut d.qacc_smooth, &d.qfrc_smooth, &d.dof_awake_ind, nv); }
    let index = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
    smooth::solve_ld(&mut d.qacc_smooth, &d.q_ld, &d.q_ldiag_inv, nv, 1, &m.m_rownnz, &m.m_rowadr, &m.m_colind, index);
}

fn warmstart(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nefc = d.nefc as usize;
    if !disabled(m, DSBL_WARMSTART) {
        let mut jar = d.scratch.num(nefc);
        blas::copy(&mut d.qacc, &d.qacc_warmstart, nv);
        constraint::mul_jac_vec(m, d, &mut jar, &d.qacc_warmstart);
        blas::sub_from(&mut jar, &d.efc_aref, nefc);
        let mut cost_warmstart = 0.0;
        constraint::constraint_update(m, d, &jar, Some(&mut cost_warmstart), 0);
        if m.opt.solver == SOL_PGS {
            let mut pgs_warmstart = blas::dot(&d.efc_force, &d.efc_b, nefc);
            let mut arf = d.scratch.num(nefc);
            if core_util::is_sparse(m) != 0 {
                sparse::mul_mat_vec_sparse(&mut arf, &d.efc_ar, &d.efc_force, nefc,
                    &d.efc_ar_rownnz, &d.efc_ar_rowadr, &d.efc_ar_colind, None);
            } else { blas::mul_mat_vec(&mut arf, &d.efc_ar, &d.efc_force, nefc, nefc); }
            pgs_warmstart += 0.5*blas::dot(&d.efc_force, &arf, nefc);
            if pgs_warmstart > 0.0 {
                blas::zero(&mut d.efc_force, nefc);
                blas::zero(&mut d.qfrc_constraint, nv);
            }
            d.scratch.put_num(arf);
        } else {
            let mut da = d.scratch.num(nv);
            let mut mda = d.scratch.num(nv);
            blas::sub(&mut da, &d.qacc_warmstart, &d.qacc_smooth, nv);
            support::mul_m(m, d, &mut mda, &da);
            if d.efm_active != 0 { metric::eff_mul_add(m, d, &mut mda, &da, 1); }
            cost_warmstart += 0.5 * blas::dot(&da, &mda, nv);
            let mut cost_smooth = 0.0;
            // The input aliases a Data field, but constraint_update does not read that field.
            let efc_b = std::mem::take(&mut d.efc_b);
            constraint::constraint_update(m, d, &efc_b, Some(&mut cost_smooth), 0);
            d.efc_b = efc_b;
            if cost_warmstart > cost_smooth { blas::copy(&mut d.qacc, &d.qacc_smooth, nv); }
            d.scratch.put_num(mda);
            d.scratch.put_num(da);
        }
        if d.nisland > 0 {
            for i in d.nidof as usize..nv {
                let dof = d.map_idof2dof[i] as usize;
                d.qacc[dof] = d.qacc_smooth[dof];
            }
        }
        d.scratch.put_num(jar);
    } else {
        blas::copy(&mut d.qacc, &d.qacc_smooth, nv);
        blas::zero(&mut d.efc_force, nefc);
    }
}

fn solve_island_task(m: &Model, d: &mut Data, _arg: &mut (), _thread_id: usize, island: usize) {
    solver::sol_pgs_island(m, d, island, m.opt.iterations);
}

fn discrete_gyro(m: &Model, d: &mut Data) {
    if m.opt.integrator != INT_DISCRETE || d.efm_active == 0 { return; }
    crate::unported!("mj_discreteGyro");
}

// C's static fwdConstraint differs from its public mj_fwdConstraint wrapper.
fn fwd_constraint_impl(m: &Model, d: &mut Data, solver: i32, flg_island: bool) {
    let nv = m.nv;
    let nefc = d.nefc as usize;
    let nisland = d.nisland as usize;
    blas::zero(&mut d.qfrc_constraint, nv);
    if nefc == 0 {
        blas::copy(&mut d.qacc, &d.qacc_smooth, nv);
        d.solver_niter[..NISLAND].fill(0);
        discrete_gyro(m, d);
        return;
    }
    let mut efc_b = std::mem::take(&mut d.efc_b);
    constraint::mul_jac_vec(m, d, &mut efc_b, &d.qacc_smooth);
    d.efc_b = efc_b;
    blas::sub_from(&mut d.efc_b, &d.efc_aref, nefc);
    warmstart(m, d);
    d.solver_niter[..NISLAND].fill(0);
    let islands_supported = flg_island && nisland > 0 && !enabled(m, ENBL_IPC)
        && metric::eff_cgmonolithic(m, d, solver) == 0;
    if islands_supported {
        match solver {
            SOL_PGS => { thread::dispatch(m, d, &mut solve_island_task, &mut (), nisland); }
            SOL_CG | SOL_NEWTON => {
                let nidof = d.nidof as usize;
                misc::gather(&mut d.ifrc_smooth, &d.qfrc_smooth, Some(&d.map_idof2dof), nidof);
                misc::gather(&mut d.ifrc_constraint, &d.qfrc_constraint, Some(&d.map_idof2dof), nidof);
                misc::gather(&mut d.iacc_smooth, &d.qacc_smooth, Some(&d.map_idof2dof), nidof);
                misc::gather(&mut d.iacc, &d.qacc, Some(&d.map_idof2dof), nidof);
                misc::gather(&mut d.iefc_force, &d.efc_force, Some(&d.map_iefc2efc), nefc);
                misc::gather(&mut d.iefc_aref, &d.efc_aref, Some(&d.map_iefc2efc), nefc);
                solver::sol_primal_islands(m, d, m.opt.iterations, (solver == SOL_NEWTON) as i32);
                misc::scatter(&mut d.qacc, &d.iacc, Some(&d.map_idof2dof), nidof);
                misc::scatter(&mut d.qfrc_constraint, &d.ifrc_constraint, Some(&d.map_idof2dof), nidof);
                misc::gather(&mut d.efc_force, &d.iefc_force, Some(&d.map_efc2iefc), nefc);
            }
            _ => {}
        }
        if m.opt.noslip_iterations > 0 {
            for island in 0..nisland { solver::sol_no_slip_island(m, d, island, m.opt.noslip_iterations); }
        }
    } else {
        match solver {
            SOL_PGS => solver::sol_pgs(m, d, m.opt.iterations),
            SOL_CG => solver::sol_cg(m, d, m.opt.iterations),
            SOL_NEWTON => solver::sol_newton(m, d, m.opt.iterations),
            _ => {}
        }
        if m.opt.noslip_iterations > 0 { solver::sol_no_slip(m, d, m.opt.noslip_iterations); }
    }
    if solver == SOL_PGS || m.opt.noslip_iterations > 0 { solver::dual_finish(m, d); }
    discrete_gyro(m, d);
}

pub fn fwd_constraint(m: &Model, d: &mut Data) {
    if d.nefc != 0 && m.opt.solver != SOL_PGS && m.opt.solver != SOL_CG && m.opt.solver != SOL_NEWTON {
        panic!("unknown solver type {}", m.opt.solver);
    }
    fwd_constraint_impl(m, d, m.opt.solver, !disabled(m, DSBL_ISLAND));
}

// whether sensor i, delayed, takes a history sample at d.time
fn is_delayed_sensor_due(m: &Model, d: &Data, i: usize) -> bool {
    if m.sensor_history[2*i] == 0 || m.sensor_delay[i] <= 0.0 {
        return false;
    }
    let interval = m.sensor_interval[2*i];
    interval <= 0.0 || d.history[m.sensor_historyadr[i] as usize] + interval <= d.time
}

// sensor i computed into its history slot at `slot`, which keeps the values the sensor leaves unwritten
fn compute_sensor_into_history(m: &Model, d: &mut Data, i: usize, slot: usize, dim: usize) {
    let mut value = d.scratch.num(dim);
    blas::copy(&mut value, &d.history[slot..], dim);
    sensor::compute_sensor(m, d, i, &mut value);
    blas::copy(&mut d.history[slot..], &value, dim);
    d.scratch.put_num(value);
}

// The work every integrator does at the pre-step state before it writes the new one: history buffers,
// activations, sleep. `sensordata_delay` holds the due delayed sensors' values when the integrator
// evaluated them before its stages (None: computed here).
fn advance_start(m: &Model, d: &mut Data, act_dot: &[f64], sensordata_delay: Option<&[f64]>) {
    if m.nhistory > 0 {
        // the controls at the current time
        for i in 0..m.nactuator {
            let nsample = m.actuator_history[2*i];
            if nsample == 0 { continue; }
            let dim = m.actuator_ctrlnum[i] as usize;
            let adr = m.actuator_historyadr[i] as usize;
            let slot = adr + misc::history_insert(&mut d.history[adr..], nsample, dim, d.time);
            let ctrladr = m.actuator_ctrladr[i] as usize;
            blas::copy(&mut d.history[slot..], &d.ctrl[ctrladr..], dim);
        }
        // the sensors: sampled on their interval, delayed, or kept as they are
        let mut delay_k = 0;
        for i in 0..m.nsensor {
            let nsample = m.sensor_history[2*i];
            if nsample == 0 { continue; }
            let dim = m.sensor_dim[i] as usize;
            let adr = m.sensor_historyadr[i] as usize;
            let delay = m.sensor_delay[i];
            let interval = m.sensor_interval[2*i];
            let sensor_adr = m.sensor_adr[i] as usize;
            if interval > 0.0 {
                // the first slot holds the previous tick
                let time_prev = d.history[adr];
                if time_prev + interval <= d.time {
                    // advance by the exact interval
                    d.history[adr] += interval;
                    let slot = adr + misc::history_insert(&mut d.history[adr..], nsample, dim, d.time);
                    if delay > 0.0 {
                        if let Some(pre) = sensordata_delay {
                            blas::copy(&mut d.history[slot..], &pre[delay_k..], dim);
                            delay_k += dim;
                        } else {
                            compute_sensor_into_history(m, d, i, slot, dim);
                        }
                    } else {
                        // undelayed: the sensor data already computed
                        blas::copy(&mut d.history[slot..], &d.sensordata[sensor_adr..], dim);
                    }
                }
            } else if delay > 0.0 {
                let slot = adr + misc::history_insert(&mut d.history[adr..], nsample, dim, d.time);
                if let Some(pre) = sensordata_delay {
                    blas::copy(&mut d.history[slot..], &pre[delay_k..], dim);
                    delay_k += dim;
                } else {
                    compute_sensor_into_history(m, d, i, slot, dim);
                }
            } else {
                let slot = adr + misc::history_insert(&mut d.history[adr..], nsample, dim, d.time);
                blas::copy(&mut d.history[slot..], &d.sensordata[sensor_adr..], dim);
            }
        }
    }
    if m.na != 0 && !disabled(m, DSBL_ACTUATION) {
        for i in 0..m.nactuator {
            let actadr = m.actuator_actadr[i];
            let actadr_end = actadr + m.actuator_actnum[i];
            for j in actadr..actadr_end {
                let j = j as usize;
                d.act[j] = support::next_activation(m, d, i, j, if support::actuator_disabled(m, i) != 0 { 0.0 } else { act_dot[j] });
            }
        }
        for i in 0..m.nactuator {
            let dyntype = m.actuator_dyntype[i];
            if dyntype != DYN_INTEGRATOR && dyntype != DYN_FILTER && dyntype != DYN_FILTEREXACT { continue; }
            let period = wrap_period(m, i);
            if period > 0.0 {
                let adr = (m.actuator_actadr[i] + m.actuator_actnum[i] - 1) as usize;
                d.act[adr] = wrap_setpoint(d.act[adr], d.actuator_length[m.actuator_outadr[i] as usize], period);
            } else if m.actuator_gaintype[i] == GAIN_SO3 {
                let adr = m.actuator_actadr[i] as usize;
                let angle = blas::norm3(at3(&d.act, adr));
                if angle > PI {
                    let scale = (angle - 2.0*PI*misc::round(angle/(2.0*PI)) as f64) / angle;
                    for k in 0..3 { d.act[adr+k] *= scale; }
                }
            }
        }
    }
    if sleep::sleep(m, d) != 0 {
        forward_skip(m, d, STAGE_POS, 0);
        sleep::update_sleep(m, d);
    }
}

// gramDet3D is omitted with flex plastic return mapping.
fn advance_finish(m: &Model, d: &mut Data, qvel: Option<&[f64]>) {
    d.time += m.opt.timestep;
    // SDF-only plugins have no advance callback; mutable plugin states are untranslated.
    if m.npluginstate != 0 { crate::unported!("plugin->advance"); }
    if m.nplastic != 0 {
        let sleep_filter = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
        for f in 0..m.nflex {
            if m.flex_dim[f] != 1 || m.flex_plasticadr[f] < 0 { continue; }
            if sleep_filter && sleep::sleep_state(m, d, OBJ_FLEX, f as i32) != S_AWAKE { continue; }
            crate::unported!("mj_rodChain");
        }
    }
    if m.nplastic != 0 && !disabled(m, DSBL_SPRING) {
        let sleep_filter = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
        for j in 0..m.njnt {
            if m.jnt_plasticadr[j] < 0 { continue; }
            if sleep_filter && d.body_awake[m.jnt_bodyid[j] as usize] != S_AWAKE { continue; }
            let adr = m.jnt_plasticadr[j] as usize;
            let padr = m.jnt_qposadr[j] as usize;
            let yield_ = misc::max(0.0, m.jnt_springyield[j]);
            let type_ = m.jnt_type[j];
            if type_ == JNT_SLIDE || type_ == JNT_HINGE {
                let x = d.qpos[padr] - d.plastic[adr];
                if x > yield_ { d.plastic[adr] = d.qpos[padr] - yield_; }
                else if x < -yield_ { d.plastic[adr] = d.qpos[padr] + yield_; }
            } else if type_ == JNT_BALL {
                let mut dif = [0.0; 3];
                let mut quat = *at4(&d.qpos, padr);
                blas::normalize4(&mut quat);
                spatial::sub_quat(&mut dif, &quat, at4(&d.plastic, adr));
                let r = blas::norm3(&dif);
                if r > yield_ {
                    spatial::quat_integrate(at4_mut(&mut d.plastic, adr), &dif, (r - yield_) / r);
                    blas::normalize4(at4_mut(&mut d.plastic, adr));
                }
            }
        }
        for i in 0..m.ntendon {
            if m.tendon_plasticadr[i] < 0 { continue; }
            if sleep_filter && sleep::sleep_state(m, d, OBJ_TENDON, i as i32) != S_AWAKE { continue; }
            let padr = m.tendon_plasticadr[i] as usize;
            let yield_ = misc::max(0.0, m.tendon_springyield[i]);
            let mut l_next = 0.0;
            let adr = m.tendon_adr[i] as usize;
            if m.wrap_type[adr] == WRAP_JOINT {
                let num = m.tendon_num[i] as usize;
                for j in 0..num {
                    let jntid = m.wrap_objid[adr+j] as usize;
                    l_next += m.wrap_prm[adr+j] * d.qpos[m.jnt_qposadr[jntid] as usize];
                }
            } else {
                let rownnz = m.ten_j_rownnz[i] as usize;
                let rowadr = m.ten_j_rowadr[i] as usize;
                l_next = d.ten_length[i] + m.opt.timestep * sparse::dot_sparse(&d.ten_j[rowadr..], qvel.unwrap_or(&d.qvel), rownnz, &m.ten_j_colind[rowadr..]);
            }
            let lower = d.plastic[padr];
            let upper = d.plastic[padr+1];
            let x = if l_next > upper { l_next - upper } else if l_next < lower { l_next - lower } else { 0.0 };
            if x > yield_ {
                let delta = x - yield_;
                if lower != 0.0 || upper <= 0.0 { d.plastic[padr] += delta; }
                d.plastic[padr+1] += delta;
            } else if x < -yield_ {
                let delta = x + yield_;
                d.plastic[padr] += delta;
                d.plastic[padr+1] += delta;
            }
        }
        for f in 0..m.nflex {
            if m.flex_plasticadr[f] < 0 { continue; }
            if sleep_filter && sleep::sleep_state(m, d, OBJ_FLEX, f as i32) != S_AWAKE { continue; }
            crate::unported!("mj_advance flex plastic return mapping");
        }
    }
}

// C's static advance carries the optional sensor snapshot; mj_advance is the public wrapper.
fn advance_impl(m: &Model, d: &mut Data, act_dot: &[f64], qacc: &[f64], qvel: Option<&[f64]>, sensordata_delay: Option<&[f64]>) {
    advance_start(m, d, act_dot, sensordata_delay);
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
    if sleep_filter { blas::add_to_scl_ind(&mut d.qvel, qacc, &d.dof_awake_ind, m.opt.timestep, d.nv_awake as usize); }
    else { blas::add_to_scl(&mut d.qvel, qacc, m.opt.timestep, m.nv); }
    let index = if sleep_filter { Some(d.body_awake_ind.as_slice()) } else { None };
    let nbody = if sleep_filter { d.nbody_awake as usize } else { m.nbody };
    support::integrate_pos_ind(m, &mut d.qpos, qvel.unwrap_or(&d.qvel), m.opt.timestep, index, nbody);
    advance_finish(m, d, qvel);
    blas::copy(&mut d.qacc_warmstart, &d.qacc, m.nv);
}

pub fn advance(m: &Model, d: &mut Data, act_dot: &[f64], qacc: &[f64], qvel: Option<&[f64]>) {
    advance_impl(m, d, act_dot, qacc, qvel, None);
}

pub fn commit(m: &Model, d: &mut Data, act_dot: &[f64], qpos: &[f64], qvel: &[f64], qacc: &[f64]) {
    blas::copy(&mut d.qacc, qacc, m.nv);
    advance_start(m, d, act_dot, None);
    blas::copy(&mut d.qpos, qpos, m.nq);
    blas::copy(&mut d.qvel, qvel, m.nv);
    advance_finish(m, d, None);
}

pub fn euler_skip(m: &Model, d: &mut Data, skipfactor: i32) {
    let mut qfrc = d.scratch.num(m.nv);
    let mut qacc = d.scratch.num(m.nv);
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nv = if sleep_filter { d.nv_awake as usize } else { m.nv };
    let dof_awake_ind = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
    let mut dof_damping = false;
    if !disabled(m, DSBL_EULERDAMP) && !disabled(m, DSBL_DAMPER) {
        for v in 0..nv {
            let i = dof_awake_ind.map_or(v, |ind| ind[v] as usize);
            if m.dof_damping[i] > 0.0 || misc::is_zero(&m.dof_dampingpoly[NPOLY*i..], NPOLY) == 0
                || m.jnt_actuatorid[m.dof_jntid[i] as usize] != -1 {
                dof_damping = true;
                break;
            }
        }
    }
    if !dof_damping {
        if let Some(ind) = dof_awake_ind { blas::copy_ind(&mut qacc, &d.qacc, ind, nv); }
        else { blas::copy(&mut qacc, &d.qacc, nv); }
    } else {
        if skipfactor == 0 {
            if let Some(ind) = dof_awake_ind { sparse::copy_sparse(&mut d.q_h, &d.m, &m.m_rownnz, &m.m_rowadr, ind, d.nv_awake as usize); }
            else { blas::copy(&mut d.q_h, &d.m, m.n_c); }
            for v in 0..nv {
                let i = dof_awake_ind.map_or(v, |ind| ind[v] as usize);
                let qv = d.qvel[i];
                let mut poly = [0.0; NPOLY];
                blas::copy(&mut poly, &m.dof_dampingpoly[NPOLY*i..], NPOLY);
                let damping = m.dof_damping[i] + core_util::actuator_damping(m, OBJ_JOINT, m.dof_jntid[i] as usize, &mut poly);
                let damp_deriv = misc::d_x_poly_force(damping, &poly, qv, NPOLY, 1);
                d.q_h[(m.m_rowadr[i] + m.m_rownnz[i] - 1) as usize] += m.opt.timestep * damp_deriv;
            }
            let clamped = smooth::factor_i(&mut d.q_h, Some(&mut d.q_hdiag_inv), nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind, dof_awake_ind);
            if clamped >= 0 { warning(d, WARN_INERTIA, clamped); }
        }
        let dof_awake_ind = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
        if let Some(ind) = dof_awake_ind {
            blas::add_ind(&mut qfrc, &d.qfrc_smooth, &d.qfrc_constraint, ind, nv);
            blas::copy_ind(&mut qacc, &qfrc, ind, nv);
        } else {
            blas::add(&mut qfrc, &d.qfrc_smooth, &d.qfrc_constraint, nv);
            blas::copy(&mut qacc, &qfrc, nv);
        }
        smooth::solve_ld(&mut qacc, &d.q_h, &d.q_hdiag_inv, nv, 1, &m.m_rownnz, &m.m_rowadr, &m.m_colind, dof_awake_ind);
    }
    // Preserve d.act_dot for next_activation and any sleep-triggered forward call.
    let mut act_dot = d.scratch.num(m.na);
    blas::copy(&mut act_dot, &d.act_dot, m.na);
    advance(m, d, &act_dot, &qacc, None);
    d.scratch.put_num(act_dot);
    d.scratch.put_num(qacc);
    d.scratch.put_num(qfrc);
}

pub fn euler(m: &Model, d: &mut Data) { euler_skip(m, d, 0); }

pub fn discrete(_m: &Model, _d: &mut Data) { crate::unported!("mj_discrete"); }

const RK4_A: [f64; 9] = [0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 1.0];
const RK4_B: [f64; 4] = [1.0/6.0, 1.0/3.0, 1.0/3.0, 1.0/6.0];

fn rk_correct_vel(m: &Model, vel: &mut [f64], dqvel: &[f64], h: f64) {
    for j in 0..m.njnt {
        let mut vadr = m.jnt_dofadr[j] as usize;
        let type_ = m.jnt_type[j];
        if type_ == JNT_FREE { vadr += 3; }
        else if type_ != JNT_BALL { continue; }
        let rot = [h*dqvel[vadr], h*dqvel[vadr+1], h*dqvel[vadr+2]];
        let old = *at3(vel, vadr);
        spatial::deriv_expmap(at3_mut(vel, vadr), &rot, &old);
    }
}

pub fn runge_kutta(m: &Model, d: &mut Data, n: i32) {
    let nv = m.nv;
    let nq = m.nq;
    let na = m.na;
    let h = m.opt.timestep;
    let time = d.time;
    let mut c = [0.0; 9];
    let mut t = [0.0; 9];
    if n != 4 { panic!("supported RK orders: N=4"); }
    let n = n as usize;
    let a = &RK4_A;
    let b = &RK4_B;
    let mut dx = d.scratch.num(2*nv+na);
    let mut x: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
    let mut f: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
    for i in 0..n {
        x[i] = d.scratch.num(nq+nv+na);
        f[i] = d.scratch.num(nv+na);
    }
    for i in 1..n {
        c[i-1] = 0.0;
        for j in 0..i { c[i-1] += a[(i-1)*(n-1)+j]; }
        t[i-1] = d.time + c[i-1]*h;
    }
    blas::copy(&mut x[0], &d.qpos, nq);
    blas::copy(&mut x[0][nq..], &d.qvel, nv);
    blas::copy(&mut f[0], &d.qacc, nv);
    if na != 0 {
        blas::copy(&mut x[0][nq+nv..], &d.act, na);
        blas::copy(&mut f[0][nv..], &d.act_dot, na);
    }
    let mut actuator_length0 = None;
    let mut actuator_velocity0 = None;
    if na != 0 && m.nout != 0 && !disabled(m, DSBL_ACTUATION) {
        let mut save_len = false;
        let mut save_vel = false;
        for i in 0..m.nactuator {
            let dyntype = m.actuator_dyntype[i];
            if (dyntype == DYN_INTEGRATOR || dyntype == DYN_FILTER || dyntype == DYN_FILTEREXACT) && wrap_period(m, i) > 0.0 { save_len = true; }
            else if dyntype == DYN_DCMOTOR { save_vel = true; }
        }
        if save_len {
            let mut saved = d.scratch.num(m.nout);
            blas::copy(&mut saved, &d.actuator_length, m.nout);
            actuator_length0 = Some(saved);
        }
        if save_vel {
            let mut saved = d.scratch.num(m.nout);
            blas::copy(&mut saved, &d.actuator_velocity, m.nout);
            actuator_velocity0 = Some(saved);
        }
    }
    let mut ten_length0 = None;
    let mut ten_j0 = None;
    if m.nplastic != 0 && m.ntendon != 0 {
        let mut saved = d.scratch.num(m.ntendon);
        blas::copy(&mut saved, &d.ten_length, m.ntendon);
        ten_length0 = Some(saved);
        if m.n_jten != 0 {
            let mut saved = d.scratch.num(m.n_jten);
            blas::copy(&mut saved, &d.ten_j, m.n_jten);
            ten_j0 = Some(saved);
        }
    }
    // Flex plasticity and its stage-zero kinematic snapshots are untranslated together.
    if m.nplastic != 0 && m.nflexvert != 0 { crate::unported!("mj_RungeKutta flex plasticity"); }
    // the due delayed sensors evaluated at stage 0, before the stages overwrite the data
    let mut sensordata_delay0 = None;
    if m.nhistory > 0 {
        let ndelaydata: usize = (0..m.nsensor).filter(|&i| is_delayed_sensor_due(m, d, i)).map(|i| m.sensor_dim[i] as usize).sum();
        if ndelaydata > 0 {
            let mut values = d.scratch.num(ndelaydata);
            let mut k = 0;
            for i in 0..m.nsensor {
                if is_delayed_sensor_due(m, d, i) {
                    let dim = m.sensor_dim[i] as usize;
                    sensor::compute_sensor(m, d, i, &mut values[k..k + dim]);
                    k += dim;
                }
            }
            sensordata_delay0 = Some(values);
        }
    }
    for i in 1..n {
        blas::zero(&mut dx, 2*nv+na);
        for j in 0..i {
            blas::add_to_scl(&mut dx, &x[j][nq..], a[(i-1)*(n-1)+j], nv);
            blas::add_to_scl(&mut dx[nv..], &f[j], a[(i-1)*(n-1)+j], nv+na);
        }
        let (previous, next) = x.split_at_mut(i);
        blas::copy(&mut next[0], &previous[0], nq+nv+na);
        support::integrate_pos(m, &mut x[i], &dx, h);
        blas::add_to_scl(&mut x[i][nq..], &dx[nv..], h, nv+na);
        blas::copy(&mut d.qpos, &x[i], nq);
        blas::copy(&mut d.qvel, &x[i][nq..], nv);
        if na != 0 { blas::copy(&mut d.act, &x[i][nq+nv..], na); }
        d.time = t[i-1];
        rk_correct_vel(m, &mut x[i][nq..], &dx, h);
        forward_skip(m, d, STAGE_NONE, 1);
        blas::copy(&mut f[i], &d.qacc, nv);
        if na != 0 { blas::copy(&mut f[i][nv..], &d.act_dot, na); }
    }
    blas::zero(&mut dx, 2*nv+na);
    for j in 0..n {
        blas::add_to_scl(&mut dx, &x[j][nq..], b[j], nv);
        blas::add_to_scl(&mut dx[nv..], &f[j], b[j], nv+na);
    }
    d.time = time;
    blas::copy(&mut d.qpos, &x[0], nq);
    blas::copy(&mut d.qvel, &x[0][nq..], nv);
    blas::copy(&mut d.act, &x[0][nq+nv..], na);
    if let Some(saved) = &actuator_length0 { blas::copy(&mut d.actuator_length, saved, m.nout); }
    if let Some(saved) = &actuator_velocity0 { blas::copy(&mut d.actuator_velocity, saved, m.nout); }
    if let Some(saved) = &ten_length0 {
        blas::copy(&mut d.ten_length, saved, m.ntendon);
        if let Some(saved) = &ten_j0 { blas::copy(&mut d.ten_j, saved, m.n_jten); }
    }
    advance_impl(m, d, &dx[2*nv..], &dx[nv..], Some(&dx), sensordata_delay0.as_deref());
    blas::copy(&mut d.qacc, &dx[nv..], nv);
    blas::copy(&mut d.qacc_warmstart, &f[n-1], nv);
    if let Some(values) = sensordata_delay0 { d.scratch.put_num(values); }
    if let Some(saved) = ten_j0 { d.scratch.put_num(saved); }
    if let Some(saved) = ten_length0 { d.scratch.put_num(saved); }
    if let Some(saved) = actuator_velocity0 { d.scratch.put_num(saved); }
    if let Some(saved) = actuator_length0 { d.scratch.put_num(saved); }
    for (x, f) in x.into_iter().zip(f) { d.scratch.put_num(f); d.scratch.put_num(x); }
    d.scratch.put_num(dx);
}

pub fn check_discrete(m: &Model) {
    // Supported models take the non-metric early return; IPC/flex/discrete validation is out.
    if !enabled(m, ENBL_IPC) && core_util::is_metric(m) == 0 && m.nflex == 0 { return; }
    crate::unported!("mj_checkDiscrete");
}

pub fn implicit_skip(m: &Model, d: &mut Data, skipfactor: i32) {
    let n_d = m.n_d;
    let n_c = m.n_c;
    let njnt = m.njnt;
    let mut qfrc = d.scratch.num(m.nv);
    let mut qacc = d.scratch.num(m.nv);
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nv = if sleep_filter { d.nv_awake as usize } else { m.nv };
    if sleep_filter { blas::add_ind(&mut qfrc, &d.qfrc_smooth, &d.qfrc_constraint, &d.dof_awake_ind, nv); }
    else { blas::add(&mut qfrc, &d.qfrc_smooth, &d.qfrc_constraint, nv); }
    if skipfactor == 0 {
        if m.opt.integrator == INT_IMPLICIT {
            derivative::d_smooth_vel(m, d, 1);
            misc::gather_masked(&mut d.q_lu, &d.m, &m.map_m2d, n_d);
            blas::add_to_scl(&mut d.q_lu, &d.q_deriv, -m.opt.timestep, n_d);
        } else if m.opt.integrator == INT_IMPLICITFAST {
            derivative::d_smooth_vel(m, d, 0);
            misc::gather(&mut d.q_h, &d.q_deriv, Some(&m.map_d2m), n_c);
            // mju_addScl aliases its second input; it reads/writes each element in order.
            for i in 0..n_c { d.q_h[i] = d.m[i] + d.q_h[i] * -m.opt.timestep; }
            for j in 0..njnt {
                if m.jnt_type[j] != JNT_FREE || !derivative::is_free_body(m, m.jnt_bodyid[j] as usize) { continue; }
                let adr = m.jnt_dofadr[j] as usize;
                for r in 0..6 {
                    let rowadr = m.m_rowadr[adr+r] as usize;
                    blas::copy(&mut d.q_h[rowadr..], &d.m[rowadr..], m.m_rownnz[adr+r] as usize);
                }
            }
        } else { panic!("integrator must be implicit or implicitfast"); }
        let dof_awake_ind = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
        let clamped = if m.opt.integrator == INT_IMPLICIT {
            let mut scratch = d.scratch.int(nv);
            let clamped = solve::factor_lusparse(&mut d.q_lu, nv, &mut scratch, &m.d_rownnz, &m.d_rowadr, &m.d_colind, dof_awake_ind);
            d.scratch.put_int(scratch);
            clamped
        } else {
            smooth::factor_i(&mut d.q_h, Some(&mut d.q_hdiag_inv), nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind, dof_awake_ind)
        };
        if clamped >= 0 { warning(d, WARN_INERTIA, clamped); }
    }
    let dof_awake_ind = if sleep_filter { Some(d.dof_awake_ind.as_slice()) } else { None };
    if m.opt.integrator == INT_IMPLICIT {
        solve::solve_lusparse(&mut qacc, &d.q_lu, &qfrc, nv, &m.d_rownnz, &m.d_rowadr, &m.d_diag, &m.d_colind, dof_awake_ind);
    } else {
        if let Some(ind) = dof_awake_ind { blas::copy_ind(&mut qacc, &qfrc, ind, nv); }
        else { blas::copy(&mut qacc, &qfrc, nv); }
        smooth::solve_ld(&mut qacc, &d.q_h, &d.q_hdiag_inv, nv, 1, &m.m_rownnz, &m.m_rowadr, &m.m_colind, dof_awake_ind);
    }
    if m.opt.integrator == INT_IMPLICITFAST {
        for j in 0..m.njnt {
            let mut a = [0.0; 36];
            if derivative::d_free_mhat(m, d, j, m.opt.timestep, &mut a, 0) == 0 { continue; }
            let adr = m.jnt_dofadr[j] as usize;
            let mut pivot = [0; 6];
            if solve::factor_lu6(&mut a, &mut pivot) != 0 {
                let mut x = [0.0; 6];
                solve::solve_lu6(&mut x, &a, at6(&qfrc, adr), &pivot);
                inline::copy6(inline::at6_mut(&mut qacc, adr), &x);
            }
        }
    }
    let mut act_dot = d.scratch.num(m.na);
    blas::copy(&mut act_dot, &d.act_dot, m.na);
    advance(m, d, &act_dot, &qacc, None);
    d.scratch.put_num(act_dot);
    d.scratch.put_num(qacc);
    d.scratch.put_num(qfrc);
}

pub fn implicit(m: &Model, d: &mut Data) { implicit_skip(m, d, 0); }

fn ipc_skip_constraint(m: &Model) -> bool {
    if !enabled(m, ENBL_IPC) { return false; }
    crate::unported!("ipcSkipConstraint");
}

fn fwd_constraint_stage(m: &Model, d: &mut Data) {
    if ipc_skip_constraint(m) {
        blas::copy(&mut d.qacc, &d.qacc_smooth, m.nv);
        d.solver_niter[..NISLAND].fill(0);
    } else { fwd_constraint(m, d); }
}

pub fn forward_skip(m: &Model, d: &mut Data, skipstage: i32, skipsensor: i32) {
    check_discrete(m);
    if skipstage < STAGE_POS {
        fwd_position(m, d);
        if skipsensor == 0 { sensor::sensor_pos(m, d); }
        if d.flg_energypos == 0 {
            if enabled(m, ENBL_ENERGY) { sensor::energy_pos(m, d); }
            else { d.energy[0] = 0.0; d.energy[1] = 0.0; }
        }
    }
    if skipstage < STAGE_VEL {
        fwd_velocity(m, d);
        if skipsensor == 0 { sensor::sensor_vel(m, d); }
        if enabled(m, ENBL_ENERGY) && d.flg_energyvel == 0 { sensor::energy_vel(m, d); }
    }
    // mjcb_control is unset.
    fwd_actuation(m, d);
    metric::eff_actuation(m, d, 1);
    if core_util::is_metric(m) != 0 {
        constraint::regularize_constraint(m, d, 1);
        constraint::reference_constraint(m, d);
    }
    fwd_acceleration(m, d);
    fwd_constraint_stage(m, d);
    if skipsensor == 0 {
        d.flg_rnepost = 0;
        sensor::sensor_acc(m, d);
    }
}

pub fn forward(m: &Model, d: &mut Data) { forward_skip(m, d, STAGE_NONE, 0); }

pub fn step(m: &Model, d: &mut Data) {
    check_pos(m, d);
    check_vel(m, d);
    forward(m, d);
    check_acc(m, d);
    if enabled(m, ENBL_FWDINV) { inverse::compare_fwd_inv(m, d); }
    match m.opt.integrator {
        INT_EULER => euler(m, d),
        INT_RK4 => runge_kutta(m, d, 4),
        INT_IMPLICIT | INT_IMPLICITFAST => implicit(m, d),
        INT_DISCRETE => {
            if enabled(m, ENBL_IPC) { crate::unported!("mj_ipc"); }
            else { crate::unported!("mj_discrete"); }
        }
        _ => panic!("invalid integrator"),
    }
}

pub fn step1(m: &Model, d: &mut Data) {
    check_discrete(m);
    check_pos(m, d);
    check_vel(m, d);
    fwd_position(m, d);
    sensor::sensor_pos(m, d);
    if d.flg_energypos == 0 {
        if enabled(m, ENBL_ENERGY) { sensor::energy_pos(m, d); }
        else { d.energy[0] = 0.0; d.energy[1] = 0.0; }
    }
    fwd_velocity(m, d);
    sensor::sensor_vel(m, d);
    if enabled(m, ENBL_ENERGY) && d.flg_energyvel == 0 { sensor::energy_vel(m, d); }
    // mjcb_control is unset.
}

pub fn step2(m: &Model, d: &mut Data) {
    fwd_actuation(m, d);
    metric::eff_actuation(m, d, 1);
    if core_util::is_metric(m) != 0 {
        constraint::regularize_constraint(m, d, 1);
        constraint::reference_constraint(m, d);
    }
    fwd_acceleration(m, d);
    fwd_constraint_stage(m, d);
    d.flg_rnepost = 0;
    sensor::sensor_acc(m, d);
    check_acc(m, d);
    if enabled(m, ENBL_FWDINV) { inverse::compare_fwd_inv(m, d); }
    if m.opt.integrator == INT_DISCRETE {
        if enabled(m, ENBL_IPC) { crate::unported!("mj_ipc"); }
        else { crate::unported!("mj_discrete"); }
    } else if m.opt.integrator == INT_IMPLICIT || m.opt.integrator == INT_IMPLICITFAST {
        implicit(m, d);
    } else { euler(m, d); }
    // Timer bookkeeping, including mjTIMER_STEP.number--, is omitted with TM_*.
}
