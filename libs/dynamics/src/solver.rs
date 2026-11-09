//! Constraint solvers: projected Gauss-Seidel, no-slip, CG and Newton.
//!
//! Translated from `src/engine/engine_solver.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use std::cell::Cell;
use std::mem::{align_of, size_of};

use crate::core_constraint::mul_jac_tvec;
use crate::core_util::is_sparse;
use crate::data::Data;
use crate::metric::eff_solve;
use crate::model::Model;
use crate::types::*;
use crate::util_blas::{add_to, copy, dot, mul_mat_vec, mul_vec_mat_vec, norm, sub, zero};
use crate::util_misc::{clip, fill_int, gather, max};
use crate::util_solve::{qcqp, qcqp2, qcqp3};
use crate::util_sparse::dot_sparse;

// Save solver statistics.
fn save_stats(
    _m: &Model, d: &mut Data, mut island: i32, iter: i32,
    improvement: f64, gradient: f64, lineslope: f64,
    nactive: i32, nchange: i32, neval: i32, nupdate: i32,
) {
    if island >= NISLAND as i32 {
        return;
    }
    island = if 0 > island { 0 } else { island };
    if iter >= NSOLVER as i32 {
        return;
    }
    let stat = &mut d.solver[island as usize * NSOLVER + iter as usize];
    stat.improvement = improvement;
    stat.gradient = gradient;
    stat.lineslope = lineslope;
    stat.nactive = nactive;
    stat.nchange = nchange;
    stat.neval = neval;
    stat.nupdate = nupdate;
}

// C's static dualFinish; _impl distinguishes it from mj_dualFinish.
fn dual_finish_impl(m: &Model, d: &mut Data) {
    // Move outputs out temporarily to express the C's disjoint field borrows.
    let mut qfrc_constraint = std::mem::take(&mut d.qfrc_constraint);
    mul_jac_tvec(m, d, &mut qfrc_constraint, &d.efc_force);
    let mut qacc = std::mem::take(&mut d.qacc);
    eff_solve(m, d, &mut qacc, Some(&qfrc_constraint));
    add_to(&mut qacc, &d.qacc_smooth, m.nv);
    d.qfrc_constraint = qfrc_constraint;
    d.qacc = qacc;
}

// PGS: map efc_force to joint space.
pub fn dual_finish(m: &Model, d: &mut Data) {
    dual_finish_impl(m, d);
}

// Compute 1/diag(AR); efclist is None for sequential iteration.
fn ardiaginv(m: &Model, d: &Data, res: &mut [f64], nefc: usize,
             efclist: Option<&[i32]>, flg_sub_r: i32) {
    let ar = &d.efc_ar;
    let r = &d.efc_r;
    if is_sparse(m) != 0 {
        let rowadr = &d.efc_ar_rowadr;
        let rownnz = &d.efc_ar_rownnz;
        let colind = &d.efc_ar_colind;
        for c in 0..nefc {
            let i = efclist.map_or(c, |list| list[c] as usize);
            let nnz = rownnz[i] as usize;
            for j in 0..nnz {
                let adr = rowadr[i] as usize + j;
                if i == colind[adr] as usize {
                    res[c] = 1.0 / if flg_sub_r != 0 { max(MINVAL, ar[adr] - r[i]) } else { ar[adr] };
                    break;
                }
            }
        }
    } else {
        let d_nefc = d.nefc as usize;
        for c in 0..nefc {
            let i = efclist.map_or(c, |list| list[c] as usize);
            let adr = i * (d_nefc + 1);
            res[c] = 1.0 / if flg_sub_r != 0 { max(MINVAL, ar[adr] - r[i]) } else { ar[adr] };
        }
    }
}

// Extract diagonal block from AR, clamp diag to 1e-10 if flg_sub_r.
fn extract_block(m: &Model, d: &Data, ac: &mut [f64], start: usize, n: usize, flg_sub_r: i32) {
    let nefc = d.nefc as usize;
    let ar = &d.efc_ar;
    if is_sparse(m) != 0 {
        let rownnz = &d.efc_ar_rownnz;
        let rowadr = &d.efc_ar_rowadr;
        let colind = &d.efc_ar_colind;
        // Assume full sub-matrix, find starting k: same for all rows.
        let mut k = 0;
        while k < rownnz[start] as usize {
            if colind[rowadr[start] as usize + k] == start as i32 {
                break;
            }
            k += 1;
        }
        if k >= rownnz[start] as usize {
            panic!("internal error");
        }
        for j in 0..n {
            copy(&mut ac[j*n..], &ar[rowadr[start+j] as usize+k..], n);
        }
    } else {
        for j in 0..n {
            copy(&mut ac[j*n..], &ar[start+(start+j)*nefc..], n);
        }
    }
    if flg_sub_r != 0 {
        let r = &d.efc_r;
        for j in 0..n {
            ac[j*(n+1)] -= r[start+j];
            ac[j*(n+1)] = max(1e-10, ac[j*(n+1)]);
        }
    }
}

fn residual(m: &Model, d: &Data, res: &mut [f64], i: usize, dim: usize, flg_sub_r: i32) {
    let nefc = d.nefc as usize;
    if is_sparse(m) != 0 {
        for j in 0..dim {
            res[j] = d.efc_b[i+j] + dot_sparse(
                &d.efc_ar[d.efc_ar_rowadr[i+j] as usize..], &d.efc_force,
                d.efc_ar_rownnz[i+j] as usize, &d.efc_ar_colind[d.efc_ar_rowadr[i+j] as usize..]);
        }
    } else {
        for j in 0..dim {
            res[j] = d.efc_b[i+j] + dot(&d.efc_ar[(i+j)*nefc..], &d.efc_force, nefc);
        }
    }
    if flg_sub_r != 0 {
        for j in 0..dim {
            res[j] -= d.efc_r[i+j]*d.efc_force[i+j];
        }
    }
}

fn cost_change(a: &[f64], force: &mut [f64], oldforce: &[f64], res: &[f64], dim: usize) -> f64 {
    let mut change;
    if dim == 1 {
        let delta = force[0] - oldforce[0];
        change = 0.5*delta*delta*a[0] + delta*res[0];
    } else {
        let mut delta = [0.0; 6];
        sub(&mut delta, force, oldforce, dim);
        change = 0.5*mul_vec_mat_vec(&delta, a, &delta, dim) + dot(&delta, res, dim);
    }
    if change > 1e-10 {
        copy(force, oldforce, dim);
        change = 0.0;
    }
    change
}

struct Pcg32State {
    state: u64,
    inc: u64,
}

fn pcg32_next(rng: &mut Pcg32State) -> u32 {
    let oldstate = rng.state;
    rng.state = oldstate.wrapping_mul(6364136223846793005).wrapping_add(rng.inc | 1);
    let xorshifted = (((oldstate >> 18) ^ oldstate) >> 27) as u32;
    let rot = (oldstate >> 59) as u32;
    (xorshifted >> rot) | (xorshifted << (rot.wrapping_neg() & 31))
}

fn shuffle_int(array: &mut [i32], n: usize, rng: &mut Pcg32State) {
    let mut i = n as i32 - 1;
    while i > 0 {
        let j = (pcg32_next(rng) % (i + 1) as u32) as usize;
        let temp = array[i as usize];
        array[i as usize] = array[j];
        array[j] = temp;
        i -= 1;
    }
}

fn dual_state(d: &Data, state: &mut [i32], ne: usize, nf: usize, nefc: usize,
              efclist: Option<&[i32]>) -> i32 {
    let force = &d.efc_force;
    let floss = &d.efc_frictionloss;
    let mut nactive = (ne + nf) as i32;
    for c in 0..ne {
        let i = efclist.map_or(c, |list| list[c] as usize);
        state[i] = CNSTRSTATE_QUADRATIC;
    }
    for c in ne..ne+nf {
        let i = efclist.map_or(c, |list| list[c] as usize);
        if force[i] <= -floss[i] {
            state[i] = CNSTRSTATE_LINEARPOS;
        } else if force[i] >= floss[i] {
            state[i] = CNSTRSTATE_LINEARNEG;
        } else {
            state[i] = CNSTRSTATE_QUADRATIC;
        }
    }
    let mut c = ne+nf;
    while c < nefc {
        let i = efclist.map_or(c, |list| list[c] as usize);
        if d.efc_type[i] != CNSTR_CONTACT_ELLIPTIC {
            if force[i] <= 0.0 {
                state[i] = CNSTRSTATE_SATISFIED;
            } else {
                state[i] = CNSTRSTATE_QUADRATIC;
                nactive += 1;
            }
        } else {
            let con = &d.contact[d.efc_id[i] as usize];
            let dim = con.dim as usize;
            let result;
            let mu = con.mu;
            let mut f = [0.0; 6];
            f[0] = force[i]/mu;
            for j in 1..dim {
                f[j] = force[i+j]/con.friction[j-1];
            }
            let n = f[0];
            let t = norm(&f[1..], dim-1);
            if mu*n >= t {
                result = CNSTRSTATE_SATISFIED;
            } else if n+mu*t <= 0.0 {
                result = CNSTRSTATE_QUADRATIC;
                nactive += dim as i32;
            } else {
                result = CNSTRSTATE_CONE;
                nactive += dim as i32;
            }
            fill_int(&mut state[i..], result, dim);
            c += dim-1;
        }
        c += 1;
    }
    nactive
}

fn dual_state_change(d: &Data, state: &mut [i32], oldstate: &mut [i32],
                     ne: usize, nf: usize, nefc: usize, efclist: Option<&[i32]>, nchange: &mut i32) -> i32 {
    for c in 0..nefc {
        let i = efclist.map_or(c, |list| list[c] as usize);
        oldstate[c] = state[i];
    }
    let nactive = dual_state(d, state, ne, nf, nefc, efclist);
    *nchange = 0;
    for c in 0..nefc {
        let i = efclist.map_or(c, |list| list[c] as usize);
        *nchange += (oldstate[c] != state[i]) as i32;
    }
    nactive
}

fn project_ellipsoid(friction: &mut [f64], normal: f64, mu: &[f64], dim: usize, feasible: i32) {
    let mut s = 0.0;
    for j in 0..dim-1 {
        s += friction[j]*friction[j] / (mu[j]*mu[j]);
    }
    let normal2 = normal*normal;
    if feasible == 0 || s > normal2 {
        let scl = (normal2 / max(MINVAL, s)).sqrt();
        for j in 0..dim-1 {
            friction[j] *= scl;
        }
    }
}

fn solve_qcqp(force: &mut [f64], i: usize, dim: usize, ac: &mut [f64], bc: &mut [f64], mu: &[f64]) {
    let flg_active;
    let mut v = [0.0; 6];
    if dim == 3 {
        flg_active = qcqp2(&mut v, ac, bc, mu, force[i]);
    } else if dim == 4 {
        flg_active = qcqp3(&mut v, ac, bc, mu, force[i]);
    } else {
        flg_active = qcqp(&mut v, ac, bc, mu, force[i], dim-1);
    }
    if flg_active != 0 {
        project_ellipsoid(&mut v, force[i], mu, dim, 0);
    }
    copy(&mut force[i+1..], &v, dim-1);
}

fn project_cone(force: &mut [f64], mu: Option<&[f64]>, dim: usize, r#type: i32) {
    if r#type == CNSTR_CONTACT_ELLIPTIC {
        if force[0] < 0.0 {
            zero(force, dim);
        } else {
            let normal = force[0];
            project_ellipsoid(&mut force[1..], normal, mu.unwrap(), dim, 1);
        }
    } else {
        if force[0] < 0.0 {
            force[0] = 0.0;
        }
    }
}

thread_local! {
    /// C's mj_nesterov_momentum, a thread-local benchmark/test toggle.
    pub static NESTEROV_MOMENTUM: Cell<i32> = const { Cell::new(1) };
}

fn inertia_scale(m: &Model, d: &Data, island: i32) -> f64 {
    let nv = if island < 0 { m.nv } else { d.island_nv[island as usize] as usize };
    let dofs = if island < 0 { None } else {
        Some(&d.map_idof2dof[d.island_idofadr[island as usize] as usize..])
    };
    let mut trace = 0.0;
    for i in 0..nv {
        let dof = dofs.map_or(i, |list| list[i] as usize);
        trace += d.m[(m.m_rowadr[dof] + m.m_rownnz[dof] - 1) as usize];
    }
    1.0 / max(MINVAL, trace)
}

// C's static solPGS; _impl distinguishes it from mj_solPGS.
fn sol_pgs_impl(m: &Model, d: &mut Data, island: i32,
                ne: usize, nf: usize, nefc: usize, efclist: Option<&[i32]>, maxiter: i32) {
    let mut arinv = d.scratch.num(nefc);
    let mut oldstate_buffer = d.scratch.int(2*nefc);
    let (oldstate, blockstart) = oldstate_buffer.split_at_mut(nefc);

    let nesterov = NESTEROV_MOMENTUM.get() != 0;
    let mut force_prev = Vec::new();
    let mut force_momentum = Vec::new();
    if nesterov {
        force_prev = d.scratch.num(nefc);
        force_momentum = d.scratch.num(nefc);
        gather(&mut force_prev, &d.efc_force, efclist, nefc);
    }

    let island_stat = if 0 > island { 0 } else { island };
    let scale = inertia_scale(m, d, island);
    ardiaginv(m, d, &mut arinv, nefc, efclist, 0);

    // The state output is disjoint from the fields read by dual_state.
    let mut state = std::mem::take(&mut d.efc_state);
    dual_state(d, &mut state, ne, nf, nefc, efclist);
    d.efc_state = state;

    let mut nblocks = 0;
    let mut c = 0;
    while c < nefc {
        blockstart[nblocks] = c as i32;
        nblocks += 1;
        let i = efclist.map_or(c, |list| list[c] as usize);
        if d.efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
            c += d.contact[d.efc_id[i] as usize].dim as usize;
        } else {
            c += 1;
        }
    }

    let mut rng = Pcg32State { state: 0, inc: 1 };
    pcg32_next(&mut rng);

    let mut iter = 0;
    let mut nesterov_k = 0;
    while iter < maxiter {
        if nesterov {
            let mut beta = 0.0;
            if iter > 0 {
                beta = (nesterov_k - 1) as f64 / (nesterov_k + 2) as f64;
            }
            if beta > 0.0 {
                for c in 0..nefc {
                    let i = efclist.map_or(c, |list| list[c] as usize);
                    let f_save = d.efc_force[i];
                    d.efc_force[i] += beta*(d.efc_force[i] - force_prev[c]);
                    force_prev[c] = f_save;
                }
                for c in ne..ne+nf {
                    let i = efclist.map_or(c, |list| list[c] as usize);
                    d.efc_force[i] = clip(d.efc_force[i], -d.efc_frictionloss[i], d.efc_frictionloss[i]);
                }
                let mut c = ne+nf;
                while c < nefc {
                    let i = efclist.map_or(c, |list| list[c] as usize);
                    let mut dim = 1;
                    let r#type = d.efc_type[i];
                    let mut mu = None;
                    if r#type == CNSTR_CONTACT_ELLIPTIC {
                        dim = d.contact[d.efc_id[i] as usize].dim as usize;
                        mu = Some(d.contact[d.efc_id[i] as usize].friction.as_slice());
                    }
                    project_cone(&mut d.efc_force[i..], mu, dim, r#type);
                    c += dim;
                }
            } else {
                gather(&mut force_prev, &d.efc_force, efclist, nefc);
            }
            gather(&mut force_momentum, &d.efc_force, efclist, nefc);
        }

        let mut improvement = 0.0;
        shuffle_int(blockstart, nblocks, &mut rng);
        for bi in 0..nblocks {
            let c = blockstart[bi] as usize;
            let i = efclist.map_or(c, |list| list[c] as usize);
            let dim;
            if d.efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
                dim = d.contact[d.efc_id[i] as usize].dim as usize;
            } else {
                dim = 1;
            }
            let mut res = [0.0; 6];
            residual(m, d, &mut res, i, dim, 0);
            let mut oldforce = [0.0; 6];
            copy(&mut oldforce, &d.efc_force[i..], dim);
            let mut athis = [0.0; 36];

            if d.efc_type[i] != CNSTR_CONTACT_ELLIPTIC {
                d.efc_force[i] -= res[0]*arinv[c];
                if c >= ne && c < ne+nf {
                    if d.efc_force[i] < -d.efc_frictionloss[i] {
                        d.efc_force[i] = -d.efc_frictionloss[i];
                    } else if d.efc_force[i] > d.efc_frictionloss[i] {
                        d.efc_force[i] = d.efc_frictionloss[i];
                    }
                } else if c >= ne+nf {
                    if d.efc_force[i] < 0.0 {
                        d.efc_force[i] = 0.0;
                    }
                }
            } else {
                let mu = d.contact[d.efc_id[i] as usize].friction;

                // Perform normal or ray update.
                extract_block(m, d, &mut athis, i, dim, 0);
                if d.efc_force[i] < MINVAL {
                    d.efc_force[i] -= res[0]*arinv[c];
                    if d.efc_force[i] < 0.0 {
                        d.efc_force[i] = 0.0;
                    }
                    zero(&mut d.efc_force[i+1..], dim-1);
                } else {
                    let mut v = [0.0; 6];
                    copy(&mut v, &d.efc_force[i..], dim);
                    let mut v1 = [0.0; 6];
                    mul_mat_vec(&mut v1, &athis, &v, dim, dim);
                    let denom = dot(&v, &v1, dim);
                    if denom >= MINVAL {
                        let mut x = -dot(&v, &res, dim) / denom;
                        if d.efc_force[i]+x*v[0] < 0.0 {
                            x = -v[0]/d.efc_force[i];
                        }
                        for j in 0..dim {
                            d.efc_force[i+j] += x*v[j];
                        }
                    }
                }

                // Perform friction update, keep normal fixed.
                let mut bc = [0.0; 5];
                let mut ac = [0.0; 25];
                copy(&mut bc, &res[1..], dim-1);
                for j in 0..dim-1 {
                    copy(&mut ac[j*(dim-1)..], &athis[(j+1)*dim+1..], dim-1);
                    bc[j] -= dot(&ac[j*(dim-1)..], &oldforce[1..], dim-1);
                    bc[j] += athis[(j+1)*dim]*(d.efc_force[i]-oldforce[0]);
                }
                if d.efc_force[i] < MINVAL {
                    zero(&mut d.efc_force[i+1..], dim-1);
                } else {
                    solve_qcqp(&mut d.efc_force, i, dim, &mut ac, &mut bc, &mu);
                }
            }
            if dim == 1 {
                athis[0] = 1.0/arinv[c];
            }
            improvement -= cost_change(&athis, &mut d.efc_force[i..], &oldforce, &res, dim);
        }

        let mut nchange = 0;
        let mut state = std::mem::take(&mut d.efc_state);
        let nactive = dual_state_change(d, &mut state, oldstate, ne, nf, nefc, efclist, &mut nchange);
        d.efc_state = state;
        improvement *= scale;
        save_stats(m, d, island_stat, iter, improvement, 0.0, 0.0, nactive, nchange, 0, 0);

        // Nesterov gradient restart (O'Donoghue-Candes).
        if nesterov {
            let mut restart = false;
            if iter > 0 {
                let mut dot_corr_extr = 0.0;
                for c in 0..nefc {
                    let i = efclist.map_or(c, |list| list[c] as usize);
                    let correction = d.efc_force[i] - force_momentum[c];
                    let extrapolation = force_momentum[c] - force_prev[c];
                    dot_corr_extr += correction * extrapolation;
                }
                restart = dot_corr_extr < 0.0;
            }
            if restart {
                nesterov_k = 0;
            } else {
                nesterov_k += 1;
            }
        }
        iter += 1;
        if improvement < m.opt.tolerance {
            break;
        }
    }

    if island_stat < NISLAND as i32 {
        d.solver_niter[island_stat as usize] += iter;
        if is_sparse(m) != 0 {
            d.solver_nnz[island_stat as usize] = 0;
            for c in 0..nefc {
                d.solver_nnz[island_stat as usize] += d.efc_ar_rownnz[efclist.map_or(c, |list| list[c] as usize)];
            }
        } else {
            d.solver_nnz[island_stat as usize] = (nefc*nefc) as i32;
        }
    }

    d.scratch.put_num(arinv);
    d.scratch.put_int(oldstate_buffer);
    if nesterov {
        d.scratch.put_num(force_prev);
        d.scratch.put_num(force_momentum);
    }
}

// Monolithic entry point; caller handles dual_finish.
pub fn sol_pgs(m: &Model, d: &mut Data, maxiter: i32) {
    sol_pgs_impl(m, d, -1, d.ne as usize, d.nf as usize, d.nefc as usize, None, maxiter);
}

pub fn sol_pgs_island(m: &Model, d: &mut Data, island: usize, maxiter: i32) {
    let ne = d.island_ne[island] as usize;
    let nf = d.island_nf[island] as usize;
    let nefc = d.island_nefc[island] as usize;
    let iefcadr = d.island_iefcadr[island] as usize;
    // The core only reads this map through efclist; move it without allocating.
    let map_iefc2efc = std::mem::take(&mut d.map_iefc2efc);
    sol_pgs_impl(m, d, island as i32, ne, nf, nefc, Some(&map_iefc2efc[iefcadr..]), maxiter);
    d.map_iefc2efc = map_iefc2efc;
}

// C's static solNoSlip; _impl distinguishes it from mj_solNoSlip.
fn sol_no_slip_impl(m: &Model, d: &mut Data, island: i32,
                    ne: usize, nf: usize, nefc: usize, efclist: Option<&[i32]>, maxiter: i32) {
    let mut iter = 0;
    let mut ac = [0.0; 25];
    let mut bc = [0.0; 5];
    let mut res = [0.0; 5];
    let mut oldforce = [0.0; 5];
    let mut delta = [0.0; 5];
    let mut arinv = d.scratch.num(nefc);
    let mut oldstate = d.scratch.int(nefc);

    let island_stat = if 0 > island { 0 } else { island };
    let scale = inertia_scale(m, d, island);
    ardiaginv(m, d, &mut arinv, nefc, efclist, 1);
    let mut state = std::mem::take(&mut d.efc_state);
    dual_state(d, &mut state, ne, nf, nefc, efclist);
    d.efc_state = state;

    while iter < maxiter {
        let mut improvement = 0.0;
        if iter == 0 {
            for c in 0..nefc {
                let i = efclist.map_or(c, |list| list[c] as usize);
                improvement += 0.5*d.efc_force[i]*d.efc_force[i]*d.efc_r[i];
            }
        }

        // One sweep: dry friction.
        for c in ne..ne+nf {
            let i = efclist.map_or(c, |list| list[c] as usize);
            residual(m, d, &mut res, i, 1, 1);
            oldforce[0] = d.efc_force[i];
            d.efc_force[i] -= res[0]*arinv[c];
            if d.efc_force[i] < -d.efc_frictionloss[i] {
                d.efc_force[i] = -d.efc_frictionloss[i];
            } else if d.efc_force[i] > d.efc_frictionloss[i] {
                d.efc_force[i] = d.efc_frictionloss[i];
            }
            delta[0] = d.efc_force[i] - oldforce[0];
            improvement -= 0.5*delta[0]*delta[0]/arinv[c] + delta[0]*res[0];
        }

        // One sweep: contact friction.
        let mut c = ne+nf;
        while c < nefc {
            let i = efclist.map_or(c, |list| list[c] as usize);
            if d.efc_type[i] == CNSTR_CONTACT_PYRAMIDAL {
                let con = &d.contact[d.efc_id[i] as usize];
                let dim = con.dim as usize;
                // C's mu assignment is unused in the pyramidal branch.
                let mut j = i;
                while j < i+2*(dim-1) {
                    residual(m, d, &mut res, j, 2, 1);
                    copy(&mut oldforce, &d.efc_force[j..], 2);
                    extract_block(m, d, &mut ac, j, 2, 1);
                    copy(&mut bc, &res, 2);
                    for k in 0..2 {
                        bc[k] -= dot(&ac[k*2..], &oldforce, 2);
                    }
                    let mid = 0.5*(d.efc_force[j]+d.efc_force[j+1]);
                    // Retain C's initial y computation even though the minimum overwrites it.
                    let _y = 0.5*(d.efc_force[j]-d.efc_force[j+1]);
                    let k1 = ac[0] + ac[3] - ac[1] - ac[2];
                    let k0 = mid*(ac[0] - ac[3]) + bc[0] - bc[1];
                    if k1 < MINVAL {
                        d.efc_force[j+1] = mid;
                        d.efc_force[j] = mid;
                    } else {
                        let y = -k0/k1;
                        if y < -mid {
                            d.efc_force[j] = 0.0;
                            d.efc_force[j+1] = 2.0*mid;
                        } else if y > mid {
                            d.efc_force[j] = 2.0*mid;
                            d.efc_force[j+1] = 0.0;
                        } else {
                            d.efc_force[j] = mid+y;
                            d.efc_force[j+1] = mid-y;
                        }
                    }
                    improvement -= cost_change(&ac, &mut d.efc_force[j..], &oldforce, &res, 2);
                    j += 2;
                }
                c += 2*(dim-1)-1;
            } else if d.efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
                let con = &d.contact[d.efc_id[i] as usize];
                let dim = con.dim as usize;
                let mu = con.friction;
                residual(m, d, &mut res, i+1, dim-1, 1);
                copy(&mut oldforce, &d.efc_force[i+1..], dim-1);
                extract_block(m, d, &mut ac, i+1, dim-1, 1);
                copy(&mut bc, &res, dim-1);
                for j in 0..dim-1 {
                    bc[j] -= dot(&ac[j*(dim-1)..], &oldforce, dim-1);
                }
                if d.efc_force[i] < MINVAL {
                    zero(&mut d.efc_force[i+1..], dim-1);
                } else {
                    solve_qcqp(&mut d.efc_force, i, dim, &mut ac, &mut bc, &mu);
                }
                improvement -= cost_change(&ac, &mut d.efc_force[i+1..], &oldforce, &res, dim-1);
                c += dim-1;
            }
            c += 1;
        }

        let mut nchange = 0;
        let mut state = std::mem::take(&mut d.efc_state);
        let nactive = dual_state_change(d, &mut state, &mut oldstate, ne, nf, nefc, efclist, &mut nchange);
        d.efc_state = state;
        improvement *= scale;
        if island_stat < NISLAND as i32 {
            let stats_iter = iter + d.solver_niter[island_stat as usize];
            save_stats(m, d, island_stat, stats_iter, improvement, 0.0, 0.0, nactive, nchange, 0, 0);
        }
        iter += 1;
        if improvement < m.opt.noslip_tolerance {
            break;
        }
    }
    if island_stat < NISLAND as i32 {
        d.solver_niter[island_stat as usize] += iter;
    }
    d.scratch.put_num(arinv);
    d.scratch.put_int(oldstate);
}

// Monolithic entry point; caller handles dual_finish.
pub fn sol_no_slip(m: &Model, d: &mut Data, maxiter: i32) {
    sol_no_slip_impl(m, d, -1, d.ne as usize, d.nf as usize, d.nefc as usize, None, maxiter);
}

pub fn sol_no_slip_island(m: &Model, d: &mut Data, island: usize, maxiter: i32) {
    let ne = d.island_ne[island] as usize;
    let nf = d.island_nf[island] as usize;
    let nefc = d.island_nefc[island] as usize;
    let iefcadr = d.island_iefcadr[island] as usize;
    let map_iefc2efc = std::mem::take(&mut d.map_iefc2efc);
    sol_no_slip_impl(m, d, island as i32, ne, nf, nefc, Some(&map_iefc2efc[iefcadr..]), maxiter);
    d.map_iefc2efc = map_iefc2efc;
}

//------------------------- Primal solvers ---------------------------------------------------------

// The C's Data/Model pointers are represented by island and the two offsets below.
// Each function borrows the corresponding slices at the point of use, so metric
// calls can borrow Data mutably without retaining aliased references. M/J and
// their structures below own scratch only for islands; monolithic calls borrow
// Model/Data. fm/fd are explicit m/d arguments. qfrc_eff owns the optional shifted
// qfrc_smooth. Hessian input views own scratch copies where the sparse kernels
// need mutable Data for scratch; all other vectors are C stack allocations.
#[derive(Default)]
struct PrimalContext {
    is_sparse: i32,  // 1: sparse, 0: dense
    is_elliptic: i32,  // 1: elliptic, 0: pyramidal
    island: i32,  // current island index, -1 if monolithic
    nv: usize,  // number of dofs
    ne: usize,  // number of equalities
    nf: usize,  // number of friction constraints
    nefc: usize,  // number of all constraints
    n_j: usize,  // number of nonzeros in Jacobian
    m_rownnz: Vec<i32>,
    m_rowadr: Vec<i32>,
    m_colind: Vec<i32>,
    m: Vec<f64>,
    q_ld: Vec<f64>,
    q_ldiag_inv: Vec<f64>,
    j_rownnz: Vec<i32>,
    j_rowadr: Vec<i32>,
    j_rowsuper: Vec<i32>,
    j_colind: Vec<i32>,
    j: Vec<f64>,
    jt_rownnz: Vec<i32>,
    jt_rowadr: Vec<i32>,
    jt_rowsuper: Vec<i32>,
    jt_colind: Vec<i32>,
    jt: Vec<f64>,
    jaref: Vec<f64>,  // Jac*qacc - aref                              (nefc x 1)
    jv: Vec<f64>,  // Jac*search                                   (nefc x 1)
    ma: Vec<f64>,  // M*qacc                                       (nv x 1)
    mv: Vec<f64>,  // M*search                                     (nv x 1)
    grad: Vec<f64>,  // gradient of master cost                      (nv x 1)
    mgrad: Vec<f64>,  // M\grad or H\grad                             (nv x 1)
    search: Vec<f64>,  // linesearch vector                            (nv x 1)
    quad: Vec<f64>,  // quadratic polynomials for constraint costs   (nefc x 3)
    oldstate: Vec<i32>,  // previous constraint state                    (nefc x 1)
    gradold: Vec<f64>,  // previous gradient                            (nv x 1)
    mgradold: Vec<f64>,  // previous preconditioned gradient             (nv x 1)
    graddif: Vec<f64>,  // grad - gradold                               (nv x 1)
    mgraddif: Vec<f64>,  // M\(grad - gradold)                           (nv x 1)
    d: Vec<f64>,  // constraint inertia                           (nefc x 1)
    cholupd: Vec<f64>,  // scratch for rank-1 Cholesky updates          (nv x 1)
    cholscratch: Vec<f64>,  // dense accumulator for the sparse kernels     (nv x 1)
    ltj: Vec<f64>,  // L'*J for cone Cholesky updates               (6 x nv)
    h_rowadr: Vec<i32>,  // Hessian row addresses                        (nv x 1)
    h_rownnz: Vec<i32>,  // Hessian row nonzeros                         (nv x 1)
    ht_rownnz: Vec<i32>,  // Hessian transpose row nonzeros               (nv x 1)
    ht_rowadr: Vec<i32>,  // Hessian transpose row addresses              (nv x 1)
    l_rownnz: Vec<i32>,  // Hessian factor row nonzeros                  (nv x 1)
    l_rowadr: Vec<i32>,  // Hessian factor row addresses                 (nv x 1)
    lt_rownnz: Vec<i32>,  // Hessian factor transpose row nonzeros        (nv x 1)
    lt_rowadr: Vec<i32>,  // Hessian factor transpose row addresses       (nv x 1)
    n_h: usize,  // number of nonzeros in Hessian H
    h_colind: Vec<i32>,  // Hessian column indices                       (nH x 1)
    ht_colind: Vec<i32>,  // Hessian transpose column indices             (nH x 1)
    h: Vec<f64>,  // Hessian                                      (nH x 1)
    n_l: usize,  // number of nonzeros in Cholesky factor L
    l_colind: Vec<i32>,  // Cholesky factor column indices               (nL x 1)
    lt_colind: Vec<i32>,  // Cholesky factor transpose column indices     (nL x 1)
    lt_map: Vec<i32>,  // CSC-to-CSR index mapping                     (nL x 1)
    l: Vec<f64>,  // Cholesky factor                              (nL x 1)
    lcone: Vec<f64>,  // Cholesky factor with cone contributions      (nL x 1)
    perm: Vec<i32>,  // DOF permutation old -> new                   (nv x 1)
    hj_colind: Vec<i32>,  // J column indices in Hessian order            (nJ x 1)
    hj: Vec<f64>,  // J values in Hessian order                    (nJ x 1)
    hjt_rownnz: Vec<i32>,  // J' row nonzeros in Hessian order             (nv x 1)
    hjt_rowadr: Vec<i32>,  // J' row addresses in Hessian order            (nv x 1)
    hjt_rowsuper: Vec<i32>,  // J' supernodes in Hessian order               (nv x 1)
    hjt_colind: Vec<i32>,  // J' column indices in Hessian order           (nJ x 1)
    hjt: Vec<f64>,  // J' values in Hessian order                   (nJ x 1)
    hm_rownnz: Vec<i32>,  // M lower-triangle row nonzeros, Hessian order (nv x 1)
    hm_rowadr: Vec<i32>,  // M lower-triangle row addresses, Hessian ord  (nv x 1)
    hm_colind: Vec<i32>,  // M lower-triangle column indices              (nM x 1)
    hm: Vec<f64>,  // M lower-triangle values                      (nM x 1)
    flg_metric: i32,  // effective metric active for this solve
    ep_l: Vec<f64>,  // CG: per-dof diagonal with efc and contact rows folded in
    n_s: usize,  // total nonzeros of the view
    s_tcap: usize,  // capacity of the rank-1 contribution, set by PrimalAllocate
    s_rownnz: Vec<i32>,  // view row nonzeros                            (nv x 1)
    s_rowadr: Vec<i32>,  // view row addresses                           (nv x 1)
    s_colind: Vec<i32>,  // view column indices                          (nS x 1)
    s_val: Vec<f64>,  // view values                                  (nS x 1)
    cost: f64,  // constraint + Gauss cost
    quad_gauss: [f64; 3],  // quadratic polynomial for Gauss cost
    scale: f64,  // scaling factor for improvement and gradient
    nactive: i32,  // number of active constraints
    ncone: i32,  // number of contacts in cone state
    nupdate: i32,  // number of Cholesky updates
    pathcost: Vec<i32>,  // reverse-etree path costs of L rows               (nv x 1)
    lflops: i64,  // sum of squared L row nonzeros (2x factorization MACs)
    cone_fold: i32,  // cached fold decision (-1: recompute)
    flg_lvalid: i32,  // 1 if L holds chol(H), 0 if deferred while folding
    flg_hdirty: i32,  // 1 if H has incremental downdates since full build
    ndowndate: i32,  // accumulated sparse H downdates since full build
    hcone: Vec<f64>,  // cone-inclusive Hessian                           (nH x 1)
    lsiter: i32,  // number of linesearch iterations
    lsresult: i32,  // linesearch result
    lsslope: f64,  // linesearch slope at solution
    idofadr: usize,
    iefcadr: usize,
    qfrc_eff: Vec<f64>,
}

impl PrimalContext {
    // Counterpart of the caller's mj_freeStack: consume the context and return
    // every owned allocation, including arrays filled by the later Hessian code.
    fn release(self, d: &mut Data) {
        if self.m_rownnz.capacity() != 0 { d.scratch.put_int(self.m_rownnz); }
        if self.m_rowadr.capacity() != 0 { d.scratch.put_int(self.m_rowadr); }
        if self.m_colind.capacity() != 0 { d.scratch.put_int(self.m_colind); }
        if self.m.capacity() != 0 { d.scratch.put_num(self.m); }
        if self.q_ld.capacity() != 0 { d.scratch.put_num(self.q_ld); }
        if self.q_ldiag_inv.capacity() != 0 { d.scratch.put_num(self.q_ldiag_inv); }
        if self.j_rownnz.capacity() != 0 { d.scratch.put_int(self.j_rownnz); }
        if self.j_rowadr.capacity() != 0 { d.scratch.put_int(self.j_rowadr); }
        if self.j_rowsuper.capacity() != 0 { d.scratch.put_int(self.j_rowsuper); }
        if self.j_colind.capacity() != 0 { d.scratch.put_int(self.j_colind); }
        if self.j.capacity() != 0 { d.scratch.put_num(self.j); }
        if self.jt_rownnz.capacity() != 0 { d.scratch.put_int(self.jt_rownnz); }
        if self.jt_rowadr.capacity() != 0 { d.scratch.put_int(self.jt_rowadr); }
        if self.jt_rowsuper.capacity() != 0 { d.scratch.put_int(self.jt_rowsuper); }
        if self.jt_colind.capacity() != 0 { d.scratch.put_int(self.jt_colind); }
        if self.jt.capacity() != 0 { d.scratch.put_num(self.jt); }
        if self.jaref.capacity() != 0 { d.scratch.put_num(self.jaref); }
        if self.jv.capacity() != 0 { d.scratch.put_num(self.jv); }
        if self.ma.capacity() != 0 { d.scratch.put_num(self.ma); }
        if self.mv.capacity() != 0 { d.scratch.put_num(self.mv); }
        if self.grad.capacity() != 0 { d.scratch.put_num(self.grad); }
        if self.mgrad.capacity() != 0 { d.scratch.put_num(self.mgrad); }
        if self.search.capacity() != 0 { d.scratch.put_num(self.search); }
        if self.quad.capacity() != 0 { d.scratch.put_num(self.quad); }
        if self.oldstate.capacity() != 0 { d.scratch.put_int(self.oldstate); }
        if self.gradold.capacity() != 0 { d.scratch.put_num(self.gradold); }
        if self.mgradold.capacity() != 0 { d.scratch.put_num(self.mgradold); }
        if self.graddif.capacity() != 0 { d.scratch.put_num(self.graddif); }
        if self.mgraddif.capacity() != 0 { d.scratch.put_num(self.mgraddif); }
        if self.d.capacity() != 0 { d.scratch.put_num(self.d); }
        if self.cholupd.capacity() != 0 { d.scratch.put_num(self.cholupd); }
        if self.cholscratch.capacity() != 0 { d.scratch.put_num(self.cholscratch); }
        if self.ltj.capacity() != 0 { d.scratch.put_num(self.ltj); }
        if self.h_rowadr.capacity() != 0 { d.scratch.put_int(self.h_rowadr); }
        if self.h_rownnz.capacity() != 0 { d.scratch.put_int(self.h_rownnz); }
        if self.ht_rownnz.capacity() != 0 { d.scratch.put_int(self.ht_rownnz); }
        if self.ht_rowadr.capacity() != 0 { d.scratch.put_int(self.ht_rowadr); }
        if self.l_rownnz.capacity() != 0 { d.scratch.put_int(self.l_rownnz); }
        if self.l_rowadr.capacity() != 0 { d.scratch.put_int(self.l_rowadr); }
        if self.lt_rownnz.capacity() != 0 { d.scratch.put_int(self.lt_rownnz); }
        if self.lt_rowadr.capacity() != 0 { d.scratch.put_int(self.lt_rowadr); }
        if self.h_colind.capacity() != 0 { d.scratch.put_int(self.h_colind); }
        if self.ht_colind.capacity() != 0 { d.scratch.put_int(self.ht_colind); }
        if self.h.capacity() != 0 { d.scratch.put_num(self.h); }
        if self.l_colind.capacity() != 0 { d.scratch.put_int(self.l_colind); }
        if self.lt_colind.capacity() != 0 { d.scratch.put_int(self.lt_colind); }
        if self.lt_map.capacity() != 0 { d.scratch.put_int(self.lt_map); }
        if self.l.capacity() != 0 { d.scratch.put_num(self.l); }
        if self.lcone.capacity() != 0 { d.scratch.put_num(self.lcone); }
        if self.perm.capacity() != 0 { d.scratch.put_int(self.perm); }
        if self.hj_colind.capacity() != 0 { d.scratch.put_int(self.hj_colind); }
        if self.hj.capacity() != 0 { d.scratch.put_num(self.hj); }
        if self.hjt_rownnz.capacity() != 0 { d.scratch.put_int(self.hjt_rownnz); }
        if self.hjt_rowadr.capacity() != 0 { d.scratch.put_int(self.hjt_rowadr); }
        if self.hjt_rowsuper.capacity() != 0 { d.scratch.put_int(self.hjt_rowsuper); }
        if self.hjt_colind.capacity() != 0 { d.scratch.put_int(self.hjt_colind); }
        if self.hjt.capacity() != 0 { d.scratch.put_num(self.hjt); }
        if self.hm_rownnz.capacity() != 0 { d.scratch.put_int(self.hm_rownnz); }
        if self.hm_rowadr.capacity() != 0 { d.scratch.put_int(self.hm_rowadr); }
        if self.hm_colind.capacity() != 0 { d.scratch.put_int(self.hm_colind); }
        if self.hm.capacity() != 0 { d.scratch.put_num(self.hm); }
        if self.ep_l.capacity() != 0 { d.scratch.put_num(self.ep_l); }
        if self.s_rownnz.capacity() != 0 { d.scratch.put_int(self.s_rownnz); }
        if self.s_rowadr.capacity() != 0 { d.scratch.put_int(self.s_rowadr); }
        if self.s_colind.capacity() != 0 { d.scratch.put_int(self.s_colind); }
        if self.s_val.capacity() != 0 { d.scratch.put_num(self.s_val); }
        if self.pathcost.capacity() != 0 { d.scratch.put_int(self.pathcost); }
        if self.hcone.capacity() != 0 { d.scratch.put_num(self.hcone); }
        if self.qfrc_eff.capacity() != 0 { d.scratch.put_num(self.qfrc_eff); }
    }
}

// Set sizes and Data-array offsets, clearing all context state as in C.
// The caller releases an allocated context before rebinding it.
fn primal_pointers(m: &Model, d: &Data, ctx: &mut PrimalContext, island: i32) {
    *ctx = PrimalContext::default();
    ctx.is_sparse = is_sparse(m);
    ctx.is_elliptic = (m.opt.cone == CONE_ELLIPTIC) as i32;
    ctx.island = island;
    ctx.cone_fold = -1;
    if island < 0 {
        ctx.nv = m.nv;
        ctx.ne = d.ne as usize;
        ctx.nf = d.nf as usize;
        ctx.nefc = d.nefc as usize;
        ctx.n_j = d.n_j as usize;
    } else {
        let island = island as usize;
        ctx.nv = d.island_nv[island] as usize;
        ctx.ne = d.island_ne[island] as usize;
        ctx.nf = d.island_nf[island] as usize;
        ctx.nefc = d.island_nefc[island] as usize;
        ctx.idofadr = d.island_idofadr[island] as usize;
        ctx.iefcadr = d.island_iefcadr[island] as usize;
    }
}

// Allocate fixed-size arrays; release the context into d.scratch after the solve.
fn primal_allocate(m: &Model, d: &mut Data, ctx: &mut PrimalContext, flg_newton: i32) {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    let is_sparse = ctx.is_sparse;
    let is_elliptic = ctx.is_elliptic;
    let mut n_j = if is_sparse != 0 { d.n_j as usize } else { 0 };

    let mut n_c = 0;
    if ctx.island >= 0 {
        let island = ctx.island as usize;
        let idofadr = d.island_idofadr[island] as usize;
        for i in 0..nv {
            let dof = d.map_idof2dof[idofadr + i] as usize;
            n_c += m.m_rownnz[dof] as usize;
        }
        if is_sparse != 0 {
            n_j = 0;
            let iefcadr = d.island_iefcadr[island] as usize;
            for i in 0..nefc {
                let efc = d.map_iefc2efc[iefcadr + i] as usize;
                n_j += d.efc_j_rownnz[efc] as usize;
            }
        } else {
            n_j = nefc * nv;
        }
        ctx.n_j = n_j;
    }

    let mut n_scap = 0;
    if d.efm_active != 0 && flg_newton != 0 {
        let mut tcap = 0;
        let mut tlow = 0;
        let mut it = crate::metric::EffRank1Iter::default();
        let mut e = crate::metric::EffRank1::default();
        while crate::metric::eff_rank1_next(m, d, &mut it, &mut e, 1) != 0 {
            if ctx.island >= 0 && d.tree_island[m.dof_treeid[e.colind[0] as usize] as usize] != ctx.island {
                continue;
            }
            tcap += e.nnz * e.nnz;
            tlow += e.nnz * (e.nnz + 1) / 2;
        }
        ctx.s_tcap = tcap;
        let mut kcap = d.nefm_k as usize / 2;
        if ctx.island >= 0 && d.nefm_k != 0 {
            let mut ksum = 0;
            let idofadr = d.island_idofadr[ctx.island as usize] as usize;
            for i in 0..nv {
                ksum += d.efm_k_rownnz[d.map_idof2dof[idofadr + i] as usize] as usize;
            }
            kcap = ksum / 2;
        }
        n_scap = kcap + tlow + nv + if !d.efm_fluid.is_empty() { m.n_c - nv } else { 0 };
    }

    // Keep C's two counting/carving passes and allocation order. Each carved
    // array is an owned scratch vector instead of a pointer into a single block.
    let mut n_num = 0;
    let mut n_int = 0;
    let mut nep = if flg_newton == 0 { crate::metric::eff_fold_diag_size(m, d, ctx.island) } else { 0 };
    for pass in 0..2 {
        if pass != 0 {
            if nep != 0 {
                let prec = crate::metric::eff_prec_scratch(m, d);
                let mul = crate::metric::eff_mul_add_scratch(m, d);
                let scratch = if prec > mul { prec } else { mul };
                let need = crate::memory::stack_bytes(std::mem::size_of::<f64>() * n_num, std::mem::align_of::<f64>())
                    + crate::memory::stack_bytes(std::mem::size_of::<i32>() * n_int, std::mem::align_of::<i32>()) + scratch;
                if need > crate::memory::stack_bytes_available(d) {
                    n_num -= nep;
                    nep = 0;
                    crate::util_errmem::warning(d, WARN_CNSTRFULL, d.narena as i32);
                }
            }
        }
        macro_rules! carve_num {
            ($field:ident, $count:expr) => {
                if pass != 0 { ctx.$field = d.scratch.num($count); } else { n_num += $count; }
            };
        }
        macro_rules! carve_int {
            ($field:ident, $count:expr) => {
                if pass != 0 { ctx.$field = d.scratch.int($count); } else { n_int += $count; }
            };
        }
        if ctx.island >= 0 {
            carve_int!(m_rownnz, nv);
            carve_int!(m_rowadr, nv);
            carve_int!(m_colind, n_c);
            carve_num!(m, n_c);
            carve_num!(q_ld, n_c);
            carve_num!(q_ldiag_inv, nv);
            carve_num!(j, n_j);
            if is_sparse != 0 {
                carve_int!(j_rownnz, nefc);
                carve_int!(j_rowadr, nefc);
                carve_int!(j_rowsuper, nefc);
                carve_int!(j_colind, n_j);
            }
        }
        carve_num!(jaref, nefc);
        carve_num!(jv, nefc);
        carve_num!(ma, nv);
        carve_num!(mv, nv);
        carve_num!(grad, nv);
        carve_num!(mgrad, nv);
        carve_num!(search, nv);
        carve_num!(quad, 3 * nefc);
        if is_sparse != 0 { carve_num!(jt, n_j); }
        if flg_newton != 0 {
            carve_num!(d, nefc);
            carve_num!(cholupd, nv);
            if is_elliptic != 0 { carve_num!(ltj, 6 * nv); }
            if is_sparse != 0 {
                carve_num!(cholscratch, nv);
            } else {
                ctx.n_l = nv * nv;
                carve_num!(l, ctx.n_l);
                if is_elliptic != 0 { carve_num!(lcone, ctx.n_l); }
            }
        } else {
            carve_num!(gradold, nv);
            carve_num!(mgradold, nv);
            carve_num!(graddif, nv);
            carve_num!(mgraddif, nv);
        }
        carve_int!(oldstate, nefc);
        if is_sparse != 0 {
            carve_int!(jt_rownnz, nv);
            carve_int!(jt_rowadr, nv);
            carve_int!(jt_rowsuper, nv);
            carve_int!(jt_colind, n_j);
        }
        if flg_newton != 0 && is_sparse != 0 {
            carve_int!(h_rowadr, nv);
            carve_int!(h_rownnz, nv);
            carve_int!(ht_rownnz, nv);
            carve_int!(ht_rowadr, nv);
            carve_int!(l_rownnz, nv);
            carve_int!(l_rowadr, nv);
            carve_int!(lt_rownnz, nv);
            carve_int!(lt_rowadr, nv);
        }
        if d.efm_active != 0 {
            if flg_newton != 0 {
                carve_int!(s_rownnz, nv);
                carve_int!(s_rowadr, nv);
                carve_int!(s_colind, n_scap);
                carve_num!(s_val, n_scap);
            }
            carve_num!(qfrc_eff, nv);
            ctx.ep_l = Vec::new();
            if nep != 0 { carve_num!(ep_l, nep); }
        }
    }

    if ctx.island >= 0 {
        let island = ctx.island as usize;
        let idofadr = d.island_idofadr[island] as usize;
        let iefcadr = d.island_iefcadr[island] as usize;
        let use_q_h = crate::core_util::eff_use_qh(d);
        let g_ld = if use_q_h != 0 { &d.q_h } else { &d.q_ld };
        let g_ldiag_inv = if use_q_h != 0 { &d.q_hdiag_inv } else { &d.q_ldiag_inv };
        crate::util_sparse::block_sparse(
            &mut ctx.q_ld, &mut ctx.m_rownnz, &mut ctx.m_rowadr, &mut ctx.m_colind,
            g_ld, &m.m_rownnz, &m.m_rowadr, &m.m_colind, nv, &d.map_idof2dof[idofadr..],
            &d.map_dof2idof, d.island_idofadr[island], 0, Some(&mut ctx.m), Some(&d.m));
        gather(&mut ctx.q_ldiag_inv, g_ldiag_inv, Some(&d.map_idof2dof[idofadr..]), nv);
        if is_sparse == 0 {
            crate::util_sparse::block(&mut ctx.j, &d.efc_j, m.nv, nv, nefc,
                &d.map_iefc2efc[iefcadr..], &d.map_idof2dof[idofadr..]);
        } else {
            crate::util_sparse::block_sparse(
                &mut ctx.j, &mut ctx.j_rownnz, &mut ctx.j_rowadr, &mut ctx.j_colind,
                &d.efc_j, &d.efc_j_rownnz, &d.efc_j_rowadr, &d.efc_j_colind,
                nefc, &d.map_iefc2efc[iefcadr..], &d.map_dof2idof,
                d.island_idofadr[island], 0, None, None);
            crate::util_sparse::super_sparse(nefc, &mut ctx.j_rowsuper, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.j_colind);
        }
    }
    if is_sparse != 0 {
        if nefc != 0 {
            let (j, j_rownnz, j_rowadr, j_colind) = if ctx.island < 0 {
                (&d.efc_j, &d.efc_j_rownnz, &d.efc_j_rowadr, &d.efc_j_colind)
            } else { (&ctx.j, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.j_colind) };
            let offset = j_rowadr[0] as usize;
            crate::util_sparse::transpose_sparse(Some(&mut ctx.jt), &j[offset..], nefc, nv,
                &mut ctx.jt_rownnz, &mut ctx.jt_rowadr, &mut ctx.jt_colind, Some(&mut ctx.jt_rowsuper),
                j_rownnz, j_rowadr, &j_colind[offset..]);
        } else {
            crate::util_misc::zero_int(&mut ctx.jt_rownnz, nv);
            crate::util_misc::zero_int(&mut ctx.jt_rowadr, nv);
            crate::util_misc::zero_int(&mut ctx.jt_rowsuper, nv);
        }
    }
    if d.efm_active != 0 {
        ctx.flg_metric = 1;
        // fm/fd are explicit Model/Data arguments to context operations.
        if ctx.island < 0 {
            crate::util_blas::add(&mut ctx.qfrc_eff, &d.qfrc_smooth, &d.efm_c, nv);
            if !d.efm_ca.is_empty() { add_to(&mut ctx.qfrc_eff, &d.efm_ca, nv); }
        } else {
            let idof2dof = &d.map_idof2dof[d.island_idofadr[ctx.island as usize] as usize..];
            for k in 0..nv {
                ctx.qfrc_eff[k] = d.ifrc_smooth[ctx.idofadr + k] + d.efm_c[idof2dof[k] as usize]
                    + if !d.efm_ca.is_empty() { d.efm_ca[idof2dof[k] as usize] } else { 0.0 };
            }
        }
    }
}

// Update efc_force, qfrc_constraint and cost-related quantities.
fn primal_update_constraint(d: &mut Data, ctx: &mut PrimalContext, flg_hessian_cone: i32) {
    let nefc = ctx.nefc;
    let nv = ctx.nv;
    let (efc_d, efc_r, efc_frictionloss, efc_type, efc_id, efc_state, efc_force,
         qfrc_constraint, qfrc_smooth, qacc, qacc_smooth, j) = if ctx.island < 0 {
        (&d.efc_d[..], &d.efc_r[..], &d.efc_frictionloss[..], &d.efc_type[..], &d.efc_id[..],
         &mut d.efc_state[..], &mut d.efc_force[..], &mut d.qfrc_constraint[..],
         &d.qfrc_smooth[..], &d.qacc[..], &d.qacc_smooth[..], &d.efc_j[..])
    } else {
        let a = ctx.iefcadr;
        let b = ctx.idofadr;
        (&d.iefc_d[a..], &d.iefc_r[a..], &d.iefc_frictionloss[a..], &d.iefc_type[a..], &d.iefc_id[a..],
         &mut d.iefc_state[a..], &mut d.iefc_force[a..], &mut d.ifrc_constraint[b..],
         &d.ifrc_smooth[b..], &d.iacc[b..], &d.iacc_smooth[b..], &ctx.j[..])
    };
    let qfrc_smooth = if ctx.flg_metric != 0 { &ctx.qfrc_eff[..] } else { qfrc_smooth };
    crate::core_constraint::constraint_update_impl(ctx.ne, ctx.nf, ctx.nefc, efc_d, efc_r,
        efc_frictionloss, &ctx.jaref, efc_type, efc_id, &mut d.contact, efc_state, efc_force,
        Some(&mut ctx.cost), flg_hessian_cone);
    if ctx.is_sparse == 0 {
        crate::util_blas::mul_mat_tvec(qfrc_constraint, j, efc_force, nefc, nv);
    } else {
        crate::util_sparse::mul_mat_vec_sparse(qfrc_constraint, &ctx.jt, efc_force, nv,
            &ctx.jt_rownnz, &ctx.jt_rowadr, &ctx.jt_colind, Some(&ctx.jt_rowsuper));
    }
    ctx.nactive = 0;
    ctx.ncone = 0;
    for i in 0..nefc {
        ctx.nactive += (efc_state[i] != CNSTRSTATE_SATISFIED) as i32;
        ctx.ncone += (efc_state[i] == CNSTRSTATE_CONE) as i32;
    }
    let mut gauss = 0.0;
    for i in 0..nv {
        gauss += 0.5 * (ctx.ma[i] - qfrc_smooth[i]) * (qacc[i] - qacc_smooth[i]);
    }
    ctx.quad_gauss[0] = gauss;
    ctx.cost += gauss;
}

fn primal_update_grad(d: &Data, ctx: &mut PrimalContext) {
    let nv = ctx.nv;
    let (qfrc_smooth, qfrc_constraint) = if ctx.island < 0 {
        (&d.qfrc_smooth[..], &d.qfrc_constraint[..])
    } else { (&d.ifrc_smooth[ctx.idofadr..], &d.ifrc_constraint[ctx.idofadr..]) };
    let qfrc_smooth = if ctx.flg_metric != 0 { &ctx.qfrc_eff[..] } else { qfrc_smooth };
    for i in 0..nv { ctx.grad[i] = ctx.ma[i] - qfrc_smooth[i] - qfrc_constraint[i]; }
}

fn primal_update_mgrad(m: &Model, d: &mut Data, ctx: &mut PrimalContext, flg_newton: i32) {
    let nv = ctx.nv;
    if flg_newton != 0 {
        let l = if ctx.ncone != 0 { &ctx.lcone } else { &ctx.l };
        if ctx.is_sparse != 0 {
            if !ctx.perm.is_empty() {
                let buf = &mut ctx.cholscratch;
                for i in 0..nv { buf[ctx.perm[i] as usize] = ctx.grad[i]; }
                // C aliases buf as input/output. Mgrad is not live until the
                // unpermutation below, so use it for the required input copy.
                copy(&mut ctx.mgrad, buf, nv);
                crate::util_solve::chol_solve_sparse(buf, l, &ctx.mgrad, nv,
                    &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind);
                for i in 0..nv { ctx.mgrad[i] = buf[ctx.perm[i] as usize]; }
            } else {
                crate::util_solve::chol_solve_sparse(&mut ctx.mgrad, l, &ctx.grad, nv,
                    &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind);
            }
        } else {
            crate::util_solve::chol_solve(&mut ctx.mgrad, l, &ctx.grad, nv);
        }
    } else if ctx.flg_metric != 0 && ctx.island < 0 {
        if !ctx.ep_l.is_empty() {
            crate::metric::eff_prec_blocks(m, d, &mut ctx.mgrad, Some(&ctx.grad), Some(&ctx.ep_l));
        } else {
            crate::metric::eff_prec(m, d, &mut ctx.mgrad, Some(&ctx.grad));
        }
    } else if ctx.flg_metric != 0 {
        crate::metric::eff_prec_island(m, d, &mut ctx.mgrad, Some(&ctx.grad), ctx.island as usize,
            if ctx.ep_l.is_empty() { None } else { Some(&ctx.ep_l) }, &ctx.q_ld, &ctx.q_ldiag_inv,
            &ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind);
    } else {
        copy(&mut ctx.mgrad, &ctx.grad, nv);
        let (q_ld, q_ldiag_inv, m_rownnz, m_rowadr, m_colind) = if ctx.island < 0 {
            (&d.q_ld, &d.q_ldiag_inv, &m.m_rownnz, &m.m_rowadr, &m.m_colind)
        } else { (&ctx.q_ld, &ctx.q_ldiag_inv, &ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind) };
        crate::core_smooth::solve_ld(&mut ctx.mgrad, q_ld, q_ldiag_inv, nv, 1, m_rownnz, m_rowadr, m_colind, None);
    }
}

fn primal_update_gradient(m: &Model, d: &mut Data, ctx: &mut PrimalContext, flg_newton: i32) {
    primal_update_grad(d, ctx);
    primal_update_mgrad(m, d, ctx, flg_newton);
}

// Prepare quadratic polynomials and contact cone quantities.
fn primal_prepare(d: &Data, ctx: &mut PrimalContext) {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    let v = &ctx.search;
    let contact = &d.contact;
    let qfrc_smooth = if ctx.flg_metric != 0 { &ctx.qfrc_eff[..] }
        else if ctx.island < 0 { &d.qfrc_smooth[..] } else { &d.ifrc_smooth[ctx.idofadr..] };
    let (efc_d, efc_type, efc_id) = if ctx.island < 0 {
        (&d.efc_d[..], &d.efc_type[..], &d.efc_id[..])
    } else { (&d.iefc_d[ctx.iefcadr..], &d.iefc_type[ctx.iefcadr..], &d.iefc_id[ctx.iefcadr..]) };
    ctx.quad_gauss[1] = dot(v, &ctx.ma, nv) - dot(qfrc_smooth, v, nv);
    ctx.quad_gauss[2] = 0.5 * dot(v, &ctx.mv, nv);
    let mut i = 0;
    while i < nefc {
        let jv = &ctx.jv[i..];
        let jaref = &ctx.jaref[i..];
        let d = &efc_d[i..];
        let quad = &mut ctx.quad[3 * i..];
        let dj0 = d[0] * jaref[0];
        quad[0] = jaref[0] * dj0;
        quad[1] = jv[0] * dj0;
        quad[2] = jv[0] * d[0] * jv[0];
        if efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
            // d above is C's D pointer; the contact array is borrowed separately.
            let con = &contact[efc_id[i] as usize];
            let dim = con.dim as usize;
            let mut uu = 0.0;
            let mu = con.mu;
            let friction = &con.friction;
            for j in 1..dim {
                let djj = d[j] * jaref[j];
                quad[0] += jaref[j] * djj;
                quad[1] += jv[j] * djj;
                quad[2] += jv[j] * d[j] * jv[j];
                let uj = jaref[j] * friction[j - 1];
                uu += uj * uj;
            }
            quad[3] = jaref[0] * mu;
            quad[4] = jv[0] * mu;
            quad[5] = f64::sqrt(uu);
            quad[6] = d[0] / ((mu * mu) * (1.0 + (mu * mu)));
            i += dim - 1;
        }
        quad[0] *= 0.5;
        quad[2] *= 0.5;
        i += 1;
    }
}

// Linesearch evaluation point.
#[derive(Clone, Copy, Default)]
struct PrimalPnt {
    alpha: f64,
    cost: f64,
    deriv: [f64; 2],
}

// Huber cost of a single friction constraint at x.
fn friction_cost(x: f64, f: f64, rf: f64, d: f64) -> f64 {
    if -rf < x && x < rf { 0.5 * d * x * x }
    else if x <= -rf { f * (-0.5 * rf - x) }
    else { f * (-0.5 * rf + x) }
}

// Huber cost difference: cost(x) - cost(start).
fn friction_cost_dif(start: f64, x: f64, f: f64, rf: f64, d: f64) -> f64 {
    let state_start = if -rf < start && start < rf { 0 } else if start <= -rf { -1 } else { 1 };
    let state_x = if -rf < x && x < rf { 0 } else if x <= -rf { -1 } else { 1 };
    if state_start == 0 && state_x == 0 {
        return 0.5 * d * (x - start) * (x + start);
    }
    if state_start == -1 && state_x == -1 { return f * (start - x); }
    if state_start == 1 && state_x == 1 { return f * (x - start); }
    friction_cost(x, f, rf, d) - friction_cost(start, f, rf, d)
}

// Elliptic cone cost difference: cost(alpha) - cost(0).
fn elliptic_cost_dif(quad: &[f64], alpha: f64, mu: f64, n: f64, t: f64, tsqr_delta: f64) -> f64 {
    let u0 = quad[3];
    let v0 = quad[4];
    let t0 = quad[5];
    let dm = quad[6];
    let zone0;
    if t0 <= 0.0 { zone0 = if u0 < 0.0 { 2 } else { 1 }; }
    else if u0 >= mu * t0 { zone0 = 1; }
    else if mu * u0 + t0 <= 0.0 { zone0 = 2; }
    else { zone0 = 3; }
    let zone_alpha;
    if t <= 0.0 { zone_alpha = if n < 0.0 { 2 } else { 1 }; }
    else if n >= mu * t { zone_alpha = 1; }
    else if mu * n + t <= 0.0 { zone_alpha = 2; }
    else { zone_alpha = 3; }
    if zone0 == 1 && zone_alpha == 1 { return 0.0; }
    if zone0 == 2 && zone_alpha == 2 { return alpha * alpha * quad[2] + alpha * quad[1]; }
    if zone0 == 3 && zone_alpha == 3 {
        let t_delta = tsqr_delta / (t + t0);
        let r_delta = alpha * v0 - mu * t_delta;
        let r0 = u0 - mu * t0;
        return 0.5 * dm * r_delta * (2.0 * r0 + r_delta);
    }
    if zone0 == 3 && zone_alpha == 2 {
        let dq = alpha * (alpha * quad[2] + quad[1]);
        let boundary0 = mu * u0 + t0;
        let gap0 = 0.5 * dm * boundary0 * boundary0;
        return dq + gap0;
    }
    if zone0 == 2 && zone_alpha == 3 {
        let dq = alpha * (alpha * quad[2] + quad[1]);
        let boundary = mu * n + t;
        let gap = 0.5 * dm * boundary * boundary;
        return dq - gap;
    }
    if zone0 == 1 && zone_alpha == 2 { return alpha * alpha * quad[2] + alpha * quad[1] + quad[0]; }
    if zone0 == 1 && zone_alpha == 3 {
        let r = n - mu * t;
        return 0.5 * dm * r * r;
    }
    if zone0 == 3 && zone_alpha == 1 {
        let r0 = u0 - mu * t0;
        return -0.5 * dm * r0 * r0;
    }
    if zone0 == 2 && zone_alpha == 1 { return -quad[0]; }
    0.0
}

// Evaluate shifted linesearch cost and its derivatives.
fn primal_eval(d: &Data, ctx: &mut PrimalContext, p: &mut PrimalPnt) {
    let ne = ctx.ne;
    let nf = ctx.nf;
    let nefc = ctx.nefc;
    let (efc_frictionloss, efc_d, efc_r, efc_type, efc_id) = if ctx.island < 0 {
        (&d.efc_frictionloss[..], &d.efc_d[..], &d.efc_r[..], &d.efc_type[..], &d.efc_id[..])
    } else {
        (&d.iefc_frictionloss[ctx.iefcadr..], &d.iefc_d[ctx.iefcadr..], &d.iefc_r[ctx.iefcadr..],
         &d.iefc_type[ctx.iefcadr..], &d.iefc_id[ctx.iefcadr..])
    };
    let mut cost = 0.0;
    let alpha = p.alpha;
    let mut deriv = [0.0; 2];
    let mut quad_total = [0.0, ctx.quad_gauss[1], ctx.quad_gauss[2]];
    let mut i = 0;
    while i < nefc {
        if i < ne {
            quad_total[1] += ctx.quad[3 * i + 1];
            quad_total[2] += ctx.quad[3 * i + 2];
            i += 1;
            continue;
        }
        if i < ne + nf {
            let start = ctx.jaref[i];
            let dir = ctx.jv[i];
            let x = start + alpha * dir;
            let f = efc_frictionloss[i];
            let d = efc_d[i];
            let rf = efc_r[i] * f;
            cost += friction_cost_dif(start, x, f, rf, d);
            if -rf < x && x < rf {
                deriv[0] += d * x * dir;
                deriv[1] += d * dir * dir;
            } else if x <= -rf {
                deriv[0] += -f * dir;
            } else {
                deriv[0] += f * dir;
            }
            i += 1;
            continue;
        }
        if efc_type[i] == CNSTR_CONTACT_ELLIPTIC {
            let con = &d.contact[efc_id[i] as usize];
            let jaref = &ctx.jaref[i..];
            let jv = &ctx.jv[i..];
            let friction = &con.friction;
            let quad = &ctx.quad[3 * i..];
            let dim = con.dim as usize;
            let mu = con.mu;
            let u0 = quad[3];
            let v0 = quad[4];
            let dm = quad[6];
            let n = u0 + alpha * v0;
            let mut v = [0.0; 5];
            let mut w = [0.0; 5];
            let mut tsqr = 0.0;
            let mut wv = 0.0;
            let mut tsqr_delta = 0.0;
            for j in 0..dim - 1 {
                let uj = jaref[j + 1] * friction[j];
                let vj = jv[j + 1] * friction[j];
                let wj = uj + alpha * vj;
                v[j] = vj;
                w[j] = wj;
                tsqr += wj * wj;
                wv += wj * vj;
                tsqr_delta += alpha * vj * (uj + wj);
            }
            let t = f64::sqrt(tsqr);
            cost += elliptic_cost_dif(quad, alpha, mu, n, t, tsqr_delta);
            if tsqr <= 0.0 {
                if n < 0.0 {
                    deriv[0] += 2.0 * alpha * quad[2] + quad[1];
                    deriv[1] += 2.0 * quad[2];
                }
            } else {
                if n >= mu * t {
                    // Top zone: nothing to do.
                } else if mu * n + t <= 0.0 {
                    deriv[0] += 2.0 * alpha * quad[2] + quad[1];
                    deriv[1] += 2.0 * quad[2];
                } else {
                    let mut what = [0.0; 5];
                    for j in 0..dim - 1 { what[j] = w[j] / t; }
                    let mut det = 0.0;
                    for j in 0..dim - 1 {
                        for k in j + 1..dim - 1 {
                            let djk = what[j] * v[k] - what[k] * v[j];
                            det += djk * djk;
                        }
                    }
                    let n1 = v0;
                    let t1 = wv / t;
                    deriv[0] += dm * (n - mu * t) * (n1 - mu * t1);
                    deriv[1] += dm * ((n1 - mu * t1) * (n1 - mu * t1) + (n / t - mu) * (-mu * det));
                }
            }
            i += dim - 1;
        } else {
            let start = ctx.jaref[i];
            let x = start + alpha * ctx.jv[i];
            let cost0 = if start < 0.0 { ctx.quad[3 * i] } else { 0.0 };
            if x < 0.0 {
                quad_total[0] += ctx.quad[3 * i] - cost0;
                quad_total[1] += ctx.quad[3 * i + 1];
                quad_total[2] += ctx.quad[3 * i + 2];
            } else {
                cost -= cost0;
            }
        }
        i += 1;
    }
    cost += alpha * alpha * quad_total[2] + alpha * quad_total[1] + quad_total[0];
    deriv[0] += 2.0 * alpha * quad_total[2] + quad_total[1];
    deriv[1] += 2.0 * quad_total[2];
    if deriv[1] <= 0.0 {
        crate::util_errmem::warning_text("Linesearch objective is not convex");
        deriv[1] = MINVAL;
    }
    p.cost = cost;
    p.deriv[0] = deriv[0];
    p.deriv[1] = deriv[1];
    ctx.lsiter += 1;
}

// Update bracket point given three candidate points.
fn update_bracket(d: &Data, ctx: &mut PrimalContext, p: &mut PrimalPnt,
                  candidates: &[PrimalPnt; 3], pnext: &mut PrimalPnt) -> i32 {
    let mut flag = 0;
    for i in 0..3 {
        if p.deriv[0] < 0.0 && candidates[i].deriv[0] < 0.0 && p.deriv[0] < candidates[i].deriv[0] {
            *p = candidates[i];
            flag = 1;
        } else if p.deriv[0] > 0.0 && candidates[i].deriv[0] > 0.0 && p.deriv[0] > candidates[i].deriv[0] {
            *p = candidates[i];
            flag = 2;
        }
    }
    if flag != 0 {
        pnext.alpha = p.alpha - p.deriv[0] / p.deriv[1];
        primal_eval(d, ctx, pnext);
    }
    flag
}

// Line search.
fn primal_search(m: &Model, d: &mut Data, ctx: &mut PrimalContext, tolerance: f64,
                     ls_iterations: f64, improvement: &mut f64) -> f64 {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    let mut p0 = PrimalPnt::default();
    let mut p1 = PrimalPnt::default();
    let mut pmid = PrimalPnt::default();
    let mut p1next = PrimalPnt::default();
    ctx.lsiter = 0;
    ctx.lsresult = 0;
    ctx.lsslope = 1.0;
    *improvement = 0.0;
    let snorm = norm(&ctx.search, nv);
    if snorm < MINVAL {
        ctx.lsresult = 1;
        return 0.0;
    }
    let gtol = tolerance * snorm / ctx.scale;
    let slopescl = ctx.scale / snorm;
    let (m_mat, m_rownnz, m_rowadr, m_colind) = if ctx.island < 0 {
        (&d.m, &m.m_rownnz, &m.m_rowadr, &m.m_colind)
    } else { (&ctx.m, &ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind) };
    crate::util_sparse::mul_sym_vec_sparse(&mut ctx.mv, m_mat, &ctx.search, nv, m_rownnz, m_rowadr, m_colind);
    if ctx.flg_metric != 0 {
        if ctx.island < 0 {
            crate::metric::eff_mul_add(m, d, &mut ctx.mv, &ctx.search, 1);
        } else {
            crate::metric::eff_mul_add_island(m, d, &mut ctx.mv, &ctx.search, ctx.island as usize);
        }
    }
    let (j, j_rownnz, j_rowadr, j_colind, j_rowsuper) = if ctx.island < 0 {
        (&d.efc_j, &d.efc_j_rownnz, &d.efc_j_rowadr, &d.efc_j_colind, &d.efc_j_rowsuper)
    } else { (&ctx.j, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.j_colind, &ctx.j_rowsuper) };
    if ctx.is_sparse == 0 {
        mul_mat_vec(&mut ctx.jv, j, &ctx.search, nefc, nv);
    } else {
        crate::util_sparse::mul_mat_vec_sparse(&mut ctx.jv, j, &ctx.search, nefc,
            j_rownnz, j_rowadr, j_colind, Some(j_rowsuper));
    }
    primal_prepare(d, ctx);
    p0.alpha = 0.0;
    primal_eval(d, ctx, &mut p0);
    p1.alpha = p0.alpha - p0.deriv[0] / p0.deriv[1];
    primal_eval(d, ctx, &mut p1);
    if p1.deriv[0].abs() < gtol && (p1.alpha == 0.0 || p1.cost < 0.0) {
        if p1.alpha == 0.0 { ctx.lsresult = 2; } else { ctx.lsresult = 0; }
        ctx.lsslope = p1.deriv[0].abs() * slopescl;
        *improvement = -p1.cost;
        return p1.alpha;
    }
    let dir = if p1.deriv[0] < 0.0 { 1 } else { -1 };
    // C's commented-out SANITY CHECKS remain inactive.
    let mut p2 = p0;
    let mut p2update = 1;
    while p1.deriv[0] * dir as f64 <= -gtol && (ctx.lsiter as f64) < ls_iterations {
        p2 = p1;
        p2update = 1;
        p1.alpha -= p1.deriv[0] / p1.deriv[1];
        primal_eval(d, ctx, &mut p1);
        if p1.deriv[0].abs() < gtol && p1.cost < 0.0 {
            ctx.lsslope = p1.deriv[0].abs() * slopescl;
            *improvement = -p1.cost;
            return p1.alpha;
        }
    }
    if ctx.lsiter as f64 >= ls_iterations {
        ctx.lsresult = 3;
        ctx.lsslope = p1.deriv[0].abs() * slopescl;
        *improvement = -p1.cost;
        return p1.alpha;
    }
    if p2update == 0 {
        ctx.lsresult = 6;
        ctx.lsslope = p1.deriv[0].abs() * slopescl;
        *improvement = -p1.cost;
        return p1.alpha;
    }
    let mut p2next = p1;
    p1next.alpha = p1.alpha - p1.deriv[0] / p1.deriv[1];
    primal_eval(d, ctx, &mut p1next);
    while (ctx.lsiter as f64) < ls_iterations {
        pmid.alpha = 0.5 * (p1.alpha + p2.alpha);
        primal_eval(d, ctx, &mut pmid);
        let candidates = [p1next, p2next, pmid];
        let mut bestcost = 0.0;
        let mut bestind: i32 = -1;
        for i in 0..3 {
            if candidates[i].deriv[0].abs() < gtol && (bestind == -1 || candidates[i].cost < bestcost) {
                bestcost = candidates[i].cost;
                bestind = i as i32;
            }
        }
        if bestind >= 0 {
            ctx.lsslope = candidates[bestind as usize].deriv[0].abs() * slopescl;
            *improvement = -candidates[bestind as usize].cost;
            return candidates[bestind as usize].alpha;
        }
        let b1 = update_bracket(d, ctx, &mut p1, &candidates, &mut p1next);
        let b2 = update_bracket(d, ctx, &mut p2, &candidates, &mut p2next);
        if b1 == 0 && b2 == 0 {
            if pmid.cost < 0.0 { ctx.lsresult = 0; } else { ctx.lsresult = 7; }
            ctx.lsslope = pmid.deriv[0].abs() * slopescl;
            *improvement = -pmid.cost;
            return pmid.alpha;
        }
    }
    if p1.cost <= p2.cost && p1.cost < 0.0 {
        ctx.lsresult = 4;
        ctx.lsslope = p1.deriv[0].abs() * slopescl;
        *improvement = -p1.cost;
        p1.alpha
    } else if p2.cost <= p1.cost && p2.cost < 0.0 {
        ctx.lsresult = 4;
        ctx.lsslope = p2.deriv[0].abs() * slopescl;
        *improvement = -p2.cost;
        p2.alpha
    } else {
        ctx.lsresult = 5;
        0.0
    }
}





// Metric lower triangle, retained for active-set recomputes.
fn make_metric_lower(m: &Model, d: &mut Data, ctx: &mut PrimalContext) {
    let nv = ctx.nv;
    let island = ctx.island;
    let idofadr = if island >= 0 { d.island_idofadr[island as usize] } else { 0 };
    let tcap = ctx.s_tcap;
    let mut tint = d.scratch.int(2 * nv + if tcap > 0 { tcap } else { 1 });
    let mut tval = d.scratch.num(if tcap > 0 { tcap } else { 1 });
    let (trownnz, rest) = tint.split_at_mut(nv);
    let (trowadr, tcol) = rest.split_at_mut(nv);
    crate::util_misc::zero_int(trownnz, nv);
    let mut it = crate::metric::EffRank1Iter::default();
    let mut e = crate::metric::EffRank1::default();
    while crate::metric::eff_rank1_next(m, d, &mut it, &mut e, 1) != 0 {
        if island >= 0 && d.tree_island[m.dof_treeid[e.colind[0] as usize] as usize] != island { continue; }
        for j in 0..e.nnz {
            let r = e.colind[j];
            let lr = if island >= 0 { d.map_dof2idof[r as usize] - idofadr } else { r };
            if lr < 0 || lr >= nv as i32 { panic!("metric rank-1 term spans islands on dof {}", r); }
            trownnz[lr as usize] += e.nnz as i32;
        }
    }
    let mut adr = 0;
    for i in 0..nv {
        trowadr[i] = adr;
        adr += trownnz[i];
        trownnz[i] = 0;
    }
    it = crate::metric::EffRank1Iter::default();
    while crate::metric::eff_rank1_next(m, d, &mut it, &mut e, 1) != 0 {
        if island >= 0 && d.tree_island[m.dof_treeid[e.colind[0] as usize] as usize] != island { continue; }
        for j in 0..e.nnz {
            let r = e.colind[j];
            let lr = if island >= 0 { d.map_dof2idof[r as usize] - idofadr } else { r } as usize;
            let s_jr = e.scale * e.val[j];
            for a in 0..e.nnz {
                let lc = if island >= 0 { d.map_dof2idof[e.colind[a] as usize] - idofadr } else { e.colind[a] };
                let w = (trowadr[lr] + trownnz[lr]) as usize;
                trownnz[lr] += 1;
                tcol[w] = lc;
                tval[w] = s_jr * e.val[a];
            }
        }
    }
    for i in 0..nv {
        let start = trowadr[i];
        let end = start + trownnz[i];
        for a in start + 1..end {
            let c = tcol[a as usize];
            let v = tval[a as usize];
            let mut b = a - 1;
            while b >= start && tcol[b as usize] > c {
                tcol[(b + 1) as usize] = tcol[b as usize];
                tval[(b + 1) as usize] = tval[b as usize];
                b -= 1;
            }
            tcol[(b + 1) as usize] = c;
            tval[(b + 1) as usize] = v;
        }
    }
    adr = 0;
    for i in 0..nv {
        ctx.s_rowadr[i] = adr;
        let gdof = if island >= 0 { d.map_idof2dof[idofadr as usize + i] as usize } else { i };
        let mut diag = if !d.efm_diag.is_empty() { d.efm_diag[gdof] } else { 0.0 };
        let mut fa = 0;
        let mut fn_ = 0; // C's fn is a Rust keyword.
        if d.nefm_k != 0 {
            fa = d.efm_k_rowadr[gdof] as usize;
            fn_ = d.efm_k_rownnz[gdof] as usize;
        }
        let fe = fa + fn_;
        let mut ta = trowadr[i] as usize;
        let te = ta + trownnz[i] as usize;
        let mut ga = 0;
        let mut ge = 0;
        if !d.efm_fluid.is_empty() {
            ga = m.m_rowadr[gdof] as usize;
            ge = ga + m.m_rownnz[gdof] as usize;
        }
        while fa < fe || ta < te || ga < ge {
            let mut val = 0.0;
            let cf = if fa < fe { if island >= 0 { d.map_dof2idof[d.efm_k_colind[fa] as usize] - idofadr } else { d.efm_k_colind[fa] } } else { nv as i32 };
            let ct = if ta < te { tcol[ta] } else { nv as i32 };
            let cg = if ga < ge { if island >= 0 { d.map_dof2idof[m.m_colind[ga] as usize] - idofadr } else { m.m_colind[ga] } } else { nv as i32 };
            let mut col = if cf < ct { cf } else { ct };
            col = if cg < col { cg } else { col };
            while fa < fe && (if island >= 0 { d.map_dof2idof[d.efm_k_colind[fa] as usize] - idofadr } else { d.efm_k_colind[fa] }) == col {
                val += d.efm_k_val[fa]; fa += 1;
            }
            while ta < te && tcol[ta] == col { val += tval[ta]; ta += 1; }
            while ga < ge && (if island >= 0 { d.map_dof2idof[m.m_colind[ga] as usize] - idofadr } else { m.m_colind[ga] }) == col {
                val += d.efm_fluid[ga]; ga += 1;
            }
            if col < i as i32 {
                ctx.s_colind[adr as usize] = col;
                ctx.s_val[adr as usize] = val;
                adr += 1;
            } else if col == i as i32 {
                diag += val;
                break;
            } else { break; }
        }
        if diag != 0.0 {
            ctx.s_colind[adr as usize] = i as i32;
            ctx.s_val[adr as usize] = diag;
            adr += 1;
        }
        ctx.s_rownnz[i] = adr - ctx.s_rowadr[i];
    }
    ctx.n_s = adr as usize;
    d.scratch.put_int(tint);
    d.scratch.put_num(tval);
}

fn primal_sqr_scratch_bytes(nv: usize, nefc: usize) -> usize {
    use crate::memory::{stack_bytes, stack_frame_bytes};
    let sym_bytes = stack_frame_bytes() + 2 * stack_bytes(nv * size_of::<i32>(), align_of::<i32>()) + stack_bytes(nefc * size_of::<i32>(), align_of::<i32>());
    let num_bytes = stack_frame_bytes() + stack_bytes(8 * nv * size_of::<f64>(), align_of::<f64>());
    if sym_bytes > num_bytes { sym_bytes } else { num_bytes }
}

fn primal_loop_scratch_bytes(m: &Model, ctx: &PrimalContext, d: &Data) -> usize {
    let sqr_bytes = if ctx.is_sparse != 0 { primal_sqr_scratch_bytes(ctx.nv, ctx.nefc) } else { 0 };
    let mul_bytes = if ctx.flg_metric != 0 { crate::metric::eff_mul_add_scratch(m, d) } else { 0 };
    let prec_bytes = if ctx.flg_metric != 0 { crate::metric::eff_prec_scratch(m, d) } else { 0 };
    sqr_bytes.max(mul_bytes.max(prec_bytes))
}

const PRIMAL_MD_MINNV: usize = 64;
const PRIMAL_MD_MAXBLK: usize = 8192;
const PRIMAL_MD_MINFLOP: i64 = 50000;
const PRIMAL_MD_MINSAVE: i64 = 25000;

fn primal_dof_blocks(m: &Model, d: &Data, ctx: &PrimalContext,
                     mut bstart: Option<&mut [i32]>, bsz: &mut [i32], dof2blk: &mut [i32]) -> usize {
    let nv = ctx.nv;
    let idof2dof = if ctx.island >= 0 { Some(&d.map_idof2dof[d.island_idofadr[ctx.island as usize] as usize..]) } else { None };
    let mut nb = 0;
    let mut i = 0;
    while i < nv {
        let gdof = if let Some(map) = idof2dof { map[i] as usize } else { i };
        let body = m.dof_bodyid[gdof];
        let mut s = 1;
        while s < 3 && i + s < nv {
            let gdof_s = if let Some(map) = idof2dof { map[i + s] as usize } else { i + s };
            if m.dof_bodyid[gdof_s] != body { break; }
            s += 1;
        }
        if let Some(bstart) = bstart.as_deref_mut() {
            bstart[nb] = i as i32;
            bsz[nb] = s as i32;
            for a in 0..s { dof2blk[i + a] = nb as i32; }
        }
        nb += 1;
        i += s;
    }
    nb
}

fn primal_block_elim_flops(nb: usize, words: usize, adj: &[u64], bsz: &[i32],
                          bperm: Option<&[i32]>, order: &mut [i32], parent: &mut [i32],
                          flag: &mut [i32], wsum: &mut [i32], n_h_upper_out: Option<&mut usize>,
                          n_l_out: Option<&mut usize>, fill_out: Option<&mut i32>) -> i64 {
    if let Some(bperm) = bperm { for b in 0..nb { order[bperm[b] as usize] = b as i32; } }
    let mut n_h_upper = 0;
    let mut n_l = 0;
    let mut fill_blks = 0;
    for q in (0..nb).rev() {
        let bq = if bperm.is_some() { order[q] as usize } else { q };
        parent[q] = -1;
        flag[q] = q as i32;
        wsum[q] = 0;
        let sq = bsz[bq];
        n_h_upper += (sq * (sq + 1) / 2) as usize;
        let aq = &adj[bq * words..];
        let w0 = if bperm.is_some() { 0 } else { q >> 6 };
        for w in w0..words {
            let mut bits = aq[w];
            if bperm.is_none() && w == w0 { bits &= (!0u64 << (q & 63)) << 1; }
            while bits != 0 {
                let bu = (w << 6) + crate::util_sparse::ctz64(bits) as usize;
                bits &= bits - 1;
                let mut p = if let Some(bperm) = bperm { bperm[bu] as usize } else { bu };
                if p <= q { continue; }
                n_h_upper += (sq * bsz[bu]) as usize;
                let mut hop = 0;
                while flag[p] != q as i32 {
                    flag[p] = q as i32;
                    wsum[p] += sq;
                    fill_blks += hop;
                    hop = 1;
                    let par = parent[p];
                    if par == -1 { parent[p] = q as i32; break; }
                    p = par as usize;
                }
            }
        }
    }
    let mut flops = 0;
    for p in 0..nb {
        let bp = if bperm.is_some() { order[p] as usize } else { p };
        let w = wsum[p];
        let s = bsz[bp];
        for a in 1..=s {
            let lnnz = (w + a) as i64;
            n_l += lnnz as usize;
            flops += lnnz * lnnz;
        }
    }
    if let Some(out) = n_h_upper_out { *out = n_h_upper; }
    if let Some(out) = n_l_out { *out = n_l; }
    if let Some(out) = fill_out { *out = fill_blks; }
    flops
}

fn primal_seed_model_perm(m: &Model, d: &Data, ctx: &PrimalContext, nb: usize,
                          bstart: &[i32], bsz: &[i32], dof2blk: &[i32], bperm: &mut [i32]) -> i32 {
    if m.nefm_cvert == 0 || m.efm_c_perm[0] < 0 { return 0; }
    let nv = ctx.nv;
    let island = ctx.island;
    let idofadr = if island >= 0 { d.island_idofadr[island as usize] } else { 0 };
    for b in 0..nb { bperm[b] = -1; }
    let mut nseeded = 0;
    let mut t = 0;
    while t < m.nefm_cvert && m.efm_c_perm[t] >= 0 {
        let gdof = m.efm_c_perm[t] as usize;
        t += 1;
        if gdof >= m.nv { continue; }
        if island >= 0 && d.tree_island[m.dof_treeid[gdof] as usize] != island { continue; }
        let ldof = if island >= 0 { d.map_dof2idof[gdof] - idofadr } else { gdof as i32 };
        if ldof >= 0 && ldof < nv as i32 {
            let b = dof2blk[ldof as usize] as usize;
            if bstart[b] == ldof && bsz[b] == 3 && bperm[b] < 0 { bperm[b] = nseeded; nseeded += 1; }
        }
    }
    if nseeded < 4 { return 0; }
    let unseeded = nb as i32 - nseeded;
    if unseeded > 0 {
        let mut u = 0;
        for b in 0..nb {
            if bperm[b] >= 0 { bperm[b] += unseeded; } else { bperm[b] = u; u += 1; }
        }
    }
    1
}

fn primal_try_reorder(m: &Model, d: &mut Data, ctx: &mut PrimalContext, cg_bytes: usize, loop_bytes: usize) {
    use crate::memory::{stack_bytes, stack_bytes_available, stack_frame_bytes};
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    if m.nflex == 0 || nv < PRIMAL_MD_MINNV || nv > 3 * PRIMAL_MD_MAXBLK || (ctx.n_s == 0 && nefc < 16) { return; }
    let nb = primal_dof_blocks(m, d, ctx, None, &mut [], &mut []);
    if nb <= 4 || nb > PRIMAL_MD_MAXBLK { return; }
    let words = (nb + 63) / 64;
    let perm_bytes = stack_bytes(nv * size_of::<i32>(), align_of::<i32>());
    let md_int_cnt = 10 * nb + words + nv;
    let md_bit_cnt = (nb + 2) * words;
    let md_scratch = stack_frame_bytes() + stack_bytes(md_int_cnt * size_of::<i32>(), align_of::<i32>())
        + stack_bytes(md_bit_cnt * size_of::<u64>(), align_of::<u64>());
    if stack_bytes_available(d) < perm_bytes + md_scratch.max(cg_bytes + loop_bytes) { return; }
    let mut perm = d.scratch.int(nv);
    let mut n_h_blk_upper = 0;
    let mut n_l_perm = 0;
    let lflops_nat;
    let mut lflops_perm = 0;
    let mut md_ok = 0;
    let mut md_ints = d.scratch.int(md_int_cnt);
    // The bitset is scalar scratch, not a C struct. Retain it in the byte pool,
    // with enough alignment padding for a u64 slice, and return the original Vec.
    let mut md_bits = d.scratch.byte(md_bit_cnt * size_of::<u64>() + align_of::<u64>() - 1);
    {
        // SAFETY: every u64 bit pattern is valid; align_to_mut provides alignment,
        // and no other reference to the byte storage exists while the view is live.
        let (_, aligned, _) = unsafe { md_bits.align_to_mut::<u64>() };
        let adj_all = &mut aligned[..md_bit_cnt];
        let (adj, rest) = adj_all.split_at_mut(nb * words);
        let (row_bits, nz_bits) = rest.split_at_mut(words);
        let (bstart, rest) = md_ints.split_at_mut(nb);
        let (bsz, rest) = rest.split_at_mut(nb);
        let (bperm, rest) = rest.split_at_mut(nb);
        let (order, rest) = rest.split_at_mut(nb);
        let (row_blks, rest) = rest.split_at_mut(nb);
        let (deg_scratch, dof2blk) = rest.split_at_mut(5 * nb + words);
        primal_dof_blocks(m, d, ctx, Some(bstart), bsz, dof2blk);
        adj.fill(0);
        row_bits.fill(0);
        nz_bits.fill(0);
        for r in 0..nv {
            let br = dof2blk[r] as usize;
            let end = (ctx.hm_rowadr[r] + ctx.hm_rownnz[r]) as usize;
            for j in ctx.hm_rowadr[r] as usize..end {
                let c = ctx.hm_colind[j] as usize;
                if c >= r { break; }
                let bc = dof2blk[c] as usize;
                if bc != br {
                    adj[br * words + (bc >> 6)] |= 1u64 << (bc & 63);
                    adj[bc * words + (br >> 6)] |= 1u64 << (br & 63);
                }
            }
        }
        if ctx.n_s != 0 {
            for r in 0..nv {
                let br = dof2blk[r] as usize;
                let end = (ctx.s_rowadr[r] + ctx.s_rownnz[r]) as usize;
                for j in ctx.s_rowadr[r] as usize..end {
                    let c = ctx.s_colind[j] as usize;
                    if c >= r { break; }
                    let bc = dof2blk[c] as usize;
                    if bc != br {
                        adj[br * words + (bc >> 6)] |= 1u64 << (bc & 63);
                        adj[bc * words + (br >> 6)] |= 1u64 << (br & 63);
                    }
                }
            }
        }
        let mut has_dyn_edges = 0;
        let mut i = 0;
        while i < nefc {
            let adr = ctx.j_rowadr[i] as usize;
            let nnz = ctx.j_rownnz[i] as usize;
            let mut k = 0;
            let mut prev = -1;
            for j in 0..nnz {
                let b = dof2blk[ctx.hj_colind[adr + j] as usize];
                if b != prev { row_blks[k] = b; k += 1; prev = b; }
            }
            if k > 1 {
                let check_new = i >= ctx.ne || k > 2;
                if k <= 16 {
                    for a in 0..k {
                        let ba = row_blks[a] as usize;
                        for c in a + 1..k {
                            let bc = row_blks[c] as usize;
                            let mask = 1u64 << (bc & 63);
                            if check_new && adj[ba * words + (bc >> 6)] & mask == 0 { has_dyn_edges = 1; }
                            adj[ba * words + (bc >> 6)] |= mask;
                            adj[bc * words + (ba >> 6)] |= 1u64 << (ba & 63);
                        }
                    }
                } else {
                    if check_new { has_dyn_edges = 1; }
                    row_bits.fill(0);
                    for a in 0..k {
                        let ba = row_blks[a] as usize;
                        row_bits[ba >> 6] |= 1u64 << (ba & 63);
                    }
                    for a in 0..k {
                        let ba = row_blks[a] as usize;
                        let aa = &mut adj[ba * words..];
                        for w in 0..words { aa[w] |= row_bits[w]; }
                        aa[ba >> 6] &= !(1u64 << (ba & 63));
                    }
                }
            }
            if !ctx.j_rowsuper.is_empty() { i += ctx.j_rowsuper[i] as usize; }
            i += 1;
        }
        let mut fill_blks = 0;
        {
            let (parent, rest) = deg_scratch.split_at_mut(nb);
            let (flag, wsum) = rest.split_at_mut(nb);
            lflops_nat = primal_block_elim_flops(nb, words, adj, bsz, None, &mut [], parent, flag, wsum,
                                                Some(&mut n_h_blk_upper), None, Some(&mut fill_blks));
        }
        if fill_blks != 0 && lflops_nat >= PRIMAL_MD_MINFLOP {
            if has_dyn_edges == 0 && primal_seed_model_perm(m, d, ctx, nb, bstart, bsz, dof2blk, bperm) != 0 {
                let (parent, rest) = deg_scratch.split_at_mut(nb);
                let (flag, wsum) = rest.split_at_mut(nb);
                lflops_perm = primal_block_elim_flops(nb, words, adj, bsz, Some(bperm), order, parent, flag, wsum,
                                                     None, Some(&mut n_l_perm), None);
                md_ok = (5 * lflops_perm < 4 * lflops_nat) as i32;
            }
            if md_ok == 0 {
                md_ok = crate::util_sparse::min_degree(bperm, adj, nb, Some(bsz), Some(&mut n_l_perm),
                    Some(&mut lflops_perm), lflops_nat, deg_scratch, nz_bits);
            }
            if md_ok != 0 {
                for b in 0..nb { order[bperm[b] as usize] = b as i32; }
                let mut cursor = 0;
                for pos in 0..nb {
                    let b = order[pos] as usize;
                    let s0 = bstart[b] as usize;
                    let sv = bsz[b] as usize;
                    for a in 0..sv { perm[s0 + a] = cursor; cursor += 1; }
                }
            }
        }
    }
    d.scratch.put_int(md_ints);
    d.scratch.put_byte(md_bits);
    let n_m = (ctx.hm_rowadr[nv - 1] + ctx.hm_rownnz[nv - 1]) as usize;
    let n_j = ctx.n_j;
    let saved = lflops_nat - lflops_perm;
    if md_ok == 0 || saved < PRIMAL_MD_MINSAVE || saved <= 16 * (n_j + n_m + ctx.n_s) as i64 {
        d.scratch.put_int(perm);
        return;
    }
    let n_p_int = 2 * n_j + 5 * nv + n_m;
    let n_p_num = 2 * n_j + n_m;
    let p_bytes = stack_bytes(n_p_int * size_of::<i32>(), align_of::<i32>()) + stack_bytes(n_p_num * size_of::<f64>(), align_of::<f64>());
    let s_scratch = if ctx.n_s != 0 { stack_frame_bytes()
        + stack_bytes((2 * nv + ctx.n_s) * size_of::<i32>(), align_of::<i32>())
        + stack_bytes(ctx.n_s * size_of::<f64>(), align_of::<f64>()) } else { 0 };
    let j_scratch = if nefc != 0 { stack_frame_bytes() + stack_bytes((nv + nefc) * size_of::<i32>(), align_of::<i32>()) } else { 0 };
    let n_h_cap = n_h_blk_upper + n_m + ctx.n_s;
    let h_cap_bytes = stack_bytes(n_h_cap * size_of::<f64>(), align_of::<f64>()) + stack_bytes(2 * n_h_cap * size_of::<i32>(), align_of::<i32>());
    let n_lp_int = 3 * n_l_perm + nv;
    let n_lp_num = if ctx.is_elliptic != 0 { 2 * n_l_perm } else { n_l_perm };
    let lp_bytes = stack_bytes(n_lp_int * size_of::<i32>(), align_of::<i32>()) + stack_bytes(n_lp_num * size_of::<f64>(), align_of::<f64>());
    let chol_count_bytes = stack_frame_bytes() + 2 * stack_bytes(nv * size_of::<i32>(), align_of::<i32>());
    let chol_fill_bytes = stack_frame_bytes() + 3 * stack_bytes(nv * size_of::<i32>(), align_of::<i32>());
    let after_h = (cg_bytes + chol_count_bytes.max(loop_bytes)).max(lp_bytes + chol_fill_bytes.max(loop_bytes));
    let after_p = s_scratch.max(j_scratch).max(h_cap_bytes + after_h);
    if stack_bytes_available(d) < p_bytes + after_p.max(cg_bytes + loop_bytes) {
        d.scratch.put_int(perm);
        return;
    }
    ctx.perm = perm;
    if nefc != 0 {
        let mut scratch = d.scratch.int(nv + nefc);
        let (invperm, hj_pos) = scratch.split_at_mut(nv);
        for r in 0..nv { invperm[ctx.perm[r] as usize] = r as i32; }
        crate::util_misc::copy_int(hj_pos, &ctx.j_rowadr, nefc);
        let mut hjt_adr = 0;
        for r in 0..nv {
            let r_old = invperm[r] as usize;
            let nnz = ctx.jt_rownnz[r_old] as usize;
            let src_adr = ctx.jt_rowadr[r_old] as usize;
            ctx.hjt_rownnz[r] = nnz as i32;
            ctx.hjt_rowadr[r] = hjt_adr as i32;
            let src_col = &ctx.jt_colind[src_adr..];
            let src_val = &ctx.jt[src_adr..];
            let dst_col = &mut ctx.hjt_colind[hjt_adr..];
            let dst_val = &mut ctx.hjt[hjt_adr..];
            for k in 0..nnz {
                let i = src_col[k] as usize;
                let v = src_val[k];
                dst_col[k] = i as i32;
                dst_val[k] = v;
                let w = hj_pos[i] as usize;
                hj_pos[i] += 1;
                ctx.hj_colind[w] = r as i32;
                ctx.hj[w] = v;
            }
            hjt_adr += nnz;
        }
        ctx.hjt_rowsuper[nv - 1] = 0;
        for r in (0..nv - 1).rev() {
            let mut matches = 0; // C's match is a Rust keyword.
            let r_old = invperm[r] as usize;
            if invperm[r + 1] == r_old as i32 + 1 { matches = (ctx.jt_rowsuper[r_old] > 0) as i32; }
            else if ctx.hjt_rownnz[r] == ctx.hjt_rownnz[r + 1] {
                let nnz = ctx.hjt_rownnz[r] as usize;
                let c0 = &ctx.hjt_colind[ctx.hjt_rowadr[r] as usize..];
                let c1 = &ctx.hjt_colind[ctx.hjt_rowadr[r + 1] as usize..];
                matches = 1;
                for k in 0..nnz { if c0[k] != c1[k] { matches = 0; break; } }
            }
            ctx.hjt_rowsuper[r] = if matches != 0 { ctx.hjt_rowsuper[r + 1] + 1 } else { 0 };
        }
        d.scratch.put_int(scratch);
    } else {
        crate::util_misc::zero_int(&mut ctx.hjt_rownnz, nv);
        crate::util_misc::zero_int(&mut ctx.hjt_rowadr, nv);
        crate::util_misc::zero_int(&mut ctx.hjt_rowsuper, nv);
    }
    let (m_rownnz, m_rowadr, m_colind, m_mat) = if ctx.island < 0 {
        (&m.m_rownnz, &m.m_rowadr, &m.m_colind, &d.m)
    } else { (&ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind, &ctx.m) };
    crate::util_sparse::permute_lower_csr(nv, &ctx.perm, &mut ctx.hm_rownnz, &mut ctx.hm_rowadr,
        &mut ctx.hm_colind, &mut ctx.hm, m_rownnz, m_rowadr, m_colind, m_mat);
    if ctx.n_s != 0 {
        let n_sp_int = 2 * nv + ctx.n_s;
        let mut sp_int = d.scratch.int(n_sp_int);
        let mut sp_val = d.scratch.num(ctx.n_s);
        let (sp_rownnz, rest) = sp_int.split_at_mut(nv);
        let (sp_rowadr, sp_colind) = rest.split_at_mut(nv);
        crate::util_sparse::permute_lower_csr(nv, &ctx.perm, sp_rownnz, sp_rowadr, sp_colind, &mut sp_val,
            &ctx.s_rownnz, &ctx.s_rowadr, &ctx.s_colind, &ctx.s_val);
        crate::util_misc::copy_int(&mut ctx.s_rownnz, sp_rownnz, nv);
        crate::util_misc::copy_int(&mut ctx.s_rowadr, sp_rowadr, nv);
        crate::util_misc::copy_int(&mut ctx.s_colind, sp_colind, ctx.n_s);
        copy(&mut ctx.s_val, &sp_val, ctx.n_s);
        d.scratch.put_int(sp_int);
        d.scratch.put_num(sp_val);
    }
}

// Allocate and compute Hessian given efc_state.
fn make_hessian(m: &Model, d: &mut Data, ctx: &mut PrimalContext) -> i32 {
    use crate::memory::{stack_bytes, stack_bytes_available, stack_frame_bytes};
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    // C aliases J, JT and M here. Own the sparse views so the existing sparse
    // kernels can borrow Data mutably for their scratch without aliasing its
    // monolithic inputs. Copies preserve values; every vector returns to Scratch.
    if ctx.is_sparse != 0 {
        macro_rules! copy_int_view {
            ($field:ident, $src:expr, $n:expr) => {{
                ctx.$field = d.scratch.int($n);
                crate::util_misc::copy_int(&mut ctx.$field, $src, $n);
            }};
        }
        macro_rules! copy_num_view {
            ($field:ident, $src:expr, $n:expr) => {{
                ctx.$field = d.scratch.num($n);
                copy(&mut ctx.$field, $src, $n);
            }};
        }
        if ctx.island < 0 {
            copy_int_view!(j_rownnz, &d.efc_j_rownnz, nefc);
            copy_int_view!(j_rowadr, &d.efc_j_rowadr, nefc);
            copy_int_view!(j_rowsuper, &d.efc_j_rowsuper, nefc);
            copy_int_view!(hj_colind, &d.efc_j_colind, ctx.n_j);
            copy_num_view!(hj, &d.efc_j, ctx.n_j);
            let n_m = (m.m_rowadr[nv - 1] + m.m_rownnz[nv - 1]) as usize;
            copy_int_view!(hm_rownnz, &m.m_rownnz, nv);
            copy_int_view!(hm_rowadr, &m.m_rowadr, nv);
            copy_int_view!(hm_colind, &m.m_colind, n_m);
            copy_num_view!(hm, &d.m, n_m);
        } else {
            copy_int_view!(hj_colind, &ctx.j_colind, ctx.n_j);
            copy_num_view!(hj, &ctx.j, ctx.n_j);
            let n_m = (ctx.m_rowadr[nv - 1] + ctx.m_rownnz[nv - 1]) as usize;
            copy_int_view!(hm_rownnz, &ctx.m_rownnz, nv);
            copy_int_view!(hm_rowadr, &ctx.m_rowadr, nv);
            copy_int_view!(hm_colind, &ctx.m_colind, n_m);
            copy_num_view!(hm, &ctx.m, n_m);
        }
        copy_int_view!(hjt_rownnz, &ctx.jt_rownnz, nv);
        copy_int_view!(hjt_rowadr, &ctx.jt_rowadr, nv);
        copy_int_view!(hjt_rowsuper, &ctx.jt_rowsuper, nv);
        copy_int_view!(hjt_colind, &ctx.jt_colind, ctx.n_j);
        copy_num_view!(hjt, &ctx.jt, ctx.n_j);
    }
    let cg_bytes = if !ctx.ltj.is_empty() { 0 } else { stack_bytes(2 * nv * size_of::<f64>(), align_of::<f64>()) };
    let loop_bytes = primal_loop_scratch_bytes(m, ctx, d);
    let (efc_state, efc_d) = if ctx.island < 0 { (&d.efc_state[..], &d.efc_d[..]) }
        else { (&d.iefc_state[ctx.iefcadr..], &d.iefc_d[ctx.iefcadr..]) };
    for i in 0..nefc { ctx.d[i] = if efc_state[i] == CNSTRSTATE_QUADRATIC { efc_d[i] } else { 0.0 }; }
    ctx.n_s = 0;
    if ctx.flg_metric != 0 {
        let tcap = if ctx.s_tcap > 0 { ctx.s_tcap } else { 1 };
        let metric_bytes = stack_frame_bytes() + stack_bytes((2 * nv + tcap) * size_of::<i32>(), align_of::<i32>())
            + stack_bytes(tcap * size_of::<f64>(), align_of::<f64>());
        if stack_bytes_available(d) < metric_bytes.max(cg_bytes + loop_bytes) {
            crate::util_errmem::warning(d, WARN_CNSTRFULL, d.narena as i32); return 0;
        }
        make_metric_lower(m, d, ctx);
    }
    if ctx.is_sparse != 0 {
        if stack_bytes_available(d) < cg_bytes + loop_bytes {
            crate::util_errmem::warning(d, WARN_CNSTRFULL, d.narena as i32); return 0;
        }
        primal_try_reorder(m, d, ctx, cg_bytes, loop_bytes);
        ctx.n_h = crate::util_sparse::sqr_mat_tdsparse_symbolic(
            &mut ctx.h_rownnz, &mut ctx.h_rowadr, None, None, nefc, nv,
            &ctx.j_rownnz, &ctx.j_rowadr, &ctx.hj_colind, Some(&ctx.j_rowsuper),
            &ctx.hjt_rownnz, &ctx.hjt_rowadr, &ctx.hjt_colind, Some(&ctx.hjt_rowsuper), d) as usize;
        ctx.n_h += (ctx.hm_rowadr[nv - 1] + ctx.hm_rownnz[nv - 1]) as usize;
        ctx.n_h += ctx.n_s;
        let h_bytes = stack_bytes(ctx.n_h * size_of::<f64>(), align_of::<f64>()) + stack_bytes(2 * ctx.n_h * size_of::<i32>(), align_of::<i32>());
        let chol_count_bytes = stack_frame_bytes() + 2 * stack_bytes(nv * size_of::<i32>(), align_of::<i32>());
        if stack_bytes_available(d) < h_bytes + cg_bytes + chol_count_bytes.max(loop_bytes) {
            crate::util_errmem::warning(d, WARN_CNSTRFULL, d.narena as i32); return 0;
        }
        ctx.h = d.scratch.num(ctx.n_h);
        ctx.h_colind = d.scratch.int(ctx.n_h);
        ctx.ht_colind = d.scratch.int(ctx.n_h);
        let mut shift = 0;
        for r in 0..nv - 1 {
            shift += ctx.hm_rownnz[r] + if ctx.n_s != 0 { ctx.s_rownnz[r] } else { 0 };
            ctx.h_rowadr[r + 1] += shift;
        }
        crate::util_sparse::sqr_mat_tdsparse_symbolic(
            &mut ctx.h_rownnz, &mut ctx.h_rowadr, Some(&mut ctx.h_colind), None, nefc, nv,
            &ctx.j_rownnz, &ctx.j_rowadr, &ctx.hj_colind, Some(&ctx.j_rowsuper),
            &ctx.hjt_rownnz, &ctx.hjt_rowadr, &ctx.hjt_colind, Some(&ctx.hjt_rowsuper), d);
        crate::util_sparse::sqr_mat_tdsparse_numeric(
            &mut ctx.h, nv, &ctx.h_rownnz, &ctx.h_rowadr, &ctx.h_colind, None,
            &ctx.hj, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.hj_colind,
            &ctx.hjt, &ctx.hjt_rownnz, &ctx.hjt_rowadr, &ctx.hjt_colind, Some(&ctx.hjt_rowsuper), Some(&ctx.d), d);
        crate::util_sparse::add_to_mat_sparse(&mut ctx.h, &mut ctx.h_rownnz, &mut ctx.h_rowadr, &mut ctx.h_colind, nv,
            &ctx.hm, &ctx.hm_rownnz, &ctx.hm_rowadr, &ctx.hm_colind);
        if ctx.n_s != 0 {
            crate::util_sparse::add_to_mat_sparse(&mut ctx.h, &mut ctx.h_rownnz, &mut ctx.h_rowadr, &mut ctx.h_colind, nv,
                &ctx.s_val, &ctx.s_rownnz, &ctx.s_rowadr, &ctx.s_colind);
        }
        crate::util_sparse::transpose_sparse(None, &[], nv, nv, &mut ctx.ht_rownnz, &mut ctx.ht_rowadr, &mut ctx.ht_colind, None,
            &ctx.h_rownnz, &ctx.h_rowadr, &ctx.h_colind);
        ctx.n_l = crate::util_solve::chol_factor_symbolic(None, &mut ctx.l_rownnz, &mut ctx.l_rowadr, &mut [],
            &mut ctx.lt_rownnz, &mut ctx.lt_rowadr, &mut [], &ctx.ht_rownnz, &ctx.ht_rowadr, &ctx.ht_colind, nv, Some(d), &mut []) as usize;
        let n_l_int = 3 * ctx.n_l + nv;
        let n_l_num = if ctx.is_elliptic != 0 { 2 * ctx.n_l } else { ctx.n_l };
        let l_bytes = stack_bytes(n_l_int * size_of::<i32>(), align_of::<i32>()) + stack_bytes(n_l_num * size_of::<f64>(), align_of::<f64>());
        let chol_fill_bytes = stack_frame_bytes() + 3 * stack_bytes(nv * size_of::<i32>(), align_of::<i32>());
        if stack_bytes_available(d) < l_bytes + chol_fill_bytes.max(loop_bytes) {
            crate::util_errmem::warning(d, WARN_CNSTRFULL, d.narena as i32); return 0;
        }
        ctx.l_colind = d.scratch.int(ctx.n_l);
        ctx.lt_colind = d.scratch.int(ctx.n_l);
        ctx.lt_map = d.scratch.int(ctx.n_l);
        ctx.pathcost = d.scratch.int(nv);
        ctx.l = d.scratch.num(ctx.n_l);
        if ctx.is_elliptic != 0 { ctx.lcone = d.scratch.num(ctx.n_l); }
        crate::util_solve::chol_factor_symbolic(Some(&mut ctx.l_colind), &mut ctx.l_rownnz, &mut ctx.l_rowadr, &mut ctx.lt_colind,
            &mut ctx.lt_rownnz, &mut ctx.lt_rowadr, &mut ctx.lt_map, &ctx.ht_rownnz, &ctx.ht_rowadr, &ctx.ht_colind, nv, Some(d), &mut []);
        ctx.lflops = 0;
        for r in 0..nv {
            let nnz = ctx.l_rownnz[r];
            let parent = if nnz >= 2 { ctx.l_colind[(ctx.l_rowadr[r] + nnz - 2) as usize] } else { -1 };
            ctx.pathcost[r] = nnz + if parent >= 0 { ctx.pathcost[parent as usize] } else { 0 };
            ctx.lflops += nnz as i64 * nnz as i64;
        }
    } else {
        let (j, m_mat, m_rownnz, m_rowadr, m_colind) = if ctx.island < 0 {
            (&d.efc_j, &d.m, &m.m_rownnz, &m.m_rowadr, &m.m_colind)
        } else { (&ctx.j, &ctx.m, &ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind) };
        crate::util_blas::sqr_mat_td_impl(&mut ctx.l, j, Some(&ctx.d), nefc, nv, 0);
        crate::util_sparse::add_to_sym_sparse(&mut ctx.l, m_mat, nv, m_rownnz, m_rowadr, m_colind, 0);
        if ctx.n_s != 0 {
            crate::util_sparse::add_to_sym_sparse(&mut ctx.l, &ctx.s_val, nv, &ctx.s_rownnz, &ctx.s_rowadr, &ctx.s_colind, 0);
        }
    }
    1
}

// Add scl * val' * val (lower triangle) into sparse symmetric H.
// The row's columns and values are sliced to nnz once, and each H row is walked as one merge with the
// row's leading columns: the same additions in the same order, without a check per element.
fn primal_add_row_outer_sparse(h: &mut [f64], h_rowadr: &[i32], h_colind: &[i32], nnz: usize,
                               cols: &[i32], val: &[f64], scl: f64) {
    let (cols, val) = (&cols[..nnz], &val[..nnz]);
    for a in 0..nnz {
        let va = scl * val[a];
        if va == 0.0 { continue; }
        let adr = h_rowadr[cols[a] as usize] as usize;
        let mut row = h_colind[adr..].iter().zip(h[adr..].iter_mut());
        for (&cb, &vb) in cols[..=a].iter().zip(&val[..=a]) {
            let hval = loop {
                let (&c, v) = row.next().expect("H holds every column of the row's outer product");
                if c >= cb { break v; }
            };
            *hval += va * vb;
        }
    }
}

// Add LTJ' * LTJ (dim x nnz, lower triangle) into sparse symmetric H.
// Sliced and merged as primal_add_row_outer_sparse.
fn primal_add_cone_outer_sparse(h: &mut [f64], h_rowadr: &[i32], h_colind: &[i32], dim: usize,
                                nnz: usize, cols: &[i32], ltj: &[f64]) {
    let cols = &cols[..nnz];
    let ltj = &ltj[..dim * nnz];
    for a in 0..nnz {
        let adr = h_rowadr[cols[a] as usize] as usize;
        let mut row = h_colind[adr..].iter().zip(h[adr..].iter_mut());
        let mut next = |cb: i32| loop {
            let (&c, v) = row.next().expect("H holds every column of the cone's outer product");
            if c >= cb { break v; }
        };
        if dim == 3 {
            let (l0, rest) = ltj.split_at(nnz);
            let (l1, l2) = rest.split_at(nnz);
            let (la0, la1, la2) = (l0[a], l1[a], l2[a]);
            for (b, &cb) in cols[..=a].iter().enumerate() {
                *next(cb) += la0 * l0[b] + la1 * l1[b] + la2 * l2[b];
            }
        } else {
            let mut la = [0.0; 6];
            for c in 0..dim { la[c] = ltj[c * nnz + a]; }
            for (b, &cb) in cols[..=a].iter().enumerate() {
                let hval = next(cb);
                let mut s = 0.0;
                for c in 0..dim { s += la[c] * ltj[c * nnz + b]; }
                *hval += s;
            }
        }
    }
}

fn factorize_hessian(m: &Model, d: &mut Data, ctx: &mut PrimalContext, flg_recompute: i32) {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    if flg_recompute != 0 {
        let (efc_state, efc_d) = if ctx.island < 0 { (&d.efc_state[..], &d.efc_d[..]) }
            else { (&d.iefc_state[ctx.iefcadr..], &d.iefc_d[ctx.iefcadr..]) };
        for i in 0..nefc { ctx.d[i] = if efc_state[i] == CNSTRSTATE_QUADRATIC { efc_d[i] } else { 0.0 }; }
    }
    if ctx.is_sparse != 0 {
        if flg_recompute != 0 {
            crate::util_sparse::sqr_mat_tdsparse_numeric(
                &mut ctx.h, nv, &ctx.h_rownnz, &ctx.h_rowadr, &ctx.h_colind, None,
                &ctx.hj, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.hj_colind,
                &ctx.hjt, &ctx.hjt_rownnz, &ctx.hjt_rowadr, &ctx.hjt_colind, Some(&ctx.hjt_rowsuper), Some(&ctx.d), d);
            crate::util_sparse::add_to_mat_sparse(&mut ctx.h, &mut ctx.h_rownnz, &mut ctx.h_rowadr, &mut ctx.h_colind, nv,
                &ctx.hm, &ctx.hm_rownnz, &ctx.hm_rowadr, &ctx.hm_colind);
            if ctx.n_s != 0 {
                crate::util_sparse::add_to_mat_sparse(&mut ctx.h, &mut ctx.h_rownnz, &mut ctx.h_rowadr, &mut ctx.h_colind, nv,
                    &ctx.s_val, &ctx.s_rownnz, &ctx.s_rowadr, &ctx.s_colind);
            }
            ctx.flg_hdirty = 0;
            ctx.ndowndate = 0;
        }
        ctx.cone_fold = -1;
        ctx.flg_lvalid = 0;
        if hessian_cone_fold(m, d, ctx, 0) == 0 {
            crate::util_solve::chol_factor_numeric(&mut ctx.l, nv, MINVAL, &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind,
                &ctx.lt_rownnz, &ctx.lt_rowadr, &ctx.lt_colind, &ctx.lt_map, &ctx.h, &ctx.h_rownnz, &ctx.h_rowadr, &ctx.h_colind, &mut ctx.cholscratch);
            ctx.flg_lvalid = 1;
        }
    } else {
        if flg_recompute != 0 {
            let (j, m_mat, m_rownnz, m_rowadr, m_colind) = if ctx.island < 0 {
                (&d.efc_j, &d.m, &m.m_rownnz, &m.m_rowadr, &m.m_colind)
            } else { (&ctx.j, &ctx.m, &ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind) };
            crate::util_blas::sqr_mat_td_impl(&mut ctx.l, j, Some(&ctx.d), nefc, nv, 0);
            crate::util_sparse::add_to_sym_sparse(&mut ctx.l, m_mat, nv, m_rownnz, m_rowadr, m_colind, 0);
            if ctx.n_s != 0 { crate::util_sparse::add_to_sym_sparse(&mut ctx.l, &ctx.s_val, nv, &ctx.s_rownnz, &ctx.s_rowadr, &ctx.s_colind, 0); }
        }
        crate::util_solve::chol_factor(&mut ctx.l, nv, MINVAL);
    }
    if ctx.ncone != 0 { hessian_cone(m, d, ctx); }
    ctx.nupdate = nefc as i32;
}

fn hessian_cone_folded(m: &Model, d: &mut Data, ctx: &mut PrimalContext) {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    let mut local = [0.0; 36];
    copy(&mut ctx.hcone, &ctx.h, ctx.n_h);
    let (efc_state, efc_id) = if ctx.island < 0 { (&d.efc_state[..], &d.efc_id[..]) }
        else { (&d.iefc_state[ctx.iefcadr..], &d.iefc_id[ctx.iefcadr..]) };
    let mut i = 0;
    while i < nefc {
        if efc_state[i] == CNSTRSTATE_CONE {
            let dim = d.contact[efc_id[i] as usize].dim as usize;
            copy(&mut local, &d.contact[efc_id[i] as usize].h, dim * dim);
            crate::util_solve::chol_factor(&mut local, dim, MINVAL);
            let nnz = ctx.j_rownnz[i] as usize;
            zero(&mut ctx.ltj, dim * nnz);
            for r in 0..dim {
                for c in 0..=r {
                    crate::util_blas::add_to_scl(&mut ctx.ltj[c * nnz..], &ctx.hj[ctx.j_rowadr[i + r] as usize..], local[r * dim + c], nnz);
                }
            }
            primal_add_cone_outer_sparse(&mut ctx.hcone, &ctx.h_rowadr, &ctx.h_colind, dim, nnz,
                &ctx.hj_colind[ctx.j_rowadr[i] as usize..], &ctx.ltj);
            ctx.nupdate += dim as i32;
            i += dim - 1;
        }
        i += 1;
    }
    let rank = crate::util_solve::chol_factor_numeric(&mut ctx.lcone, nv, MINVAL, &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind,
        &ctx.lt_rownnz, &ctx.lt_rowadr, &ctx.lt_colind, &ctx.lt_map, &ctx.hcone, &ctx.h_rownnz, &ctx.h_rowadr, &ctx.h_colind, &mut ctx.cholscratch);
    if rank < nv as i32 && ctx.flg_hdirty != 0 { factorize_hessian(m, d, ctx, 1); }
}

fn hessian_cone_update(d: &Data, ctx: &mut PrimalContext) {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    let mut local = [0.0; 36];
    copy(&mut ctx.lcone, &ctx.l, ctx.n_l);
    let (efc_state, efc_id, j) = if ctx.island < 0 { (&d.efc_state[..], &d.efc_id[..], &d.efc_j[..]) }
        else { (&d.iefc_state[ctx.iefcadr..], &d.iefc_id[ctx.iefcadr..], &ctx.j[..]) };
    let mut i = 0;
    while i < nefc {
        if efc_state[i] == CNSTRSTATE_CONE {
            let con = &d.contact[efc_id[i] as usize];
            let dim = con.dim as usize;
            copy(&mut local, &con.h, dim * dim);
            crate::util_solve::chol_factor(&mut local, dim, MINVAL);
            if ctx.is_sparse != 0 {
                let nnz = ctx.j_rownnz[i] as usize;
                zero(&mut ctx.ltj, dim * nnz);
                for r in 0..dim {
                    for c in 0..=r {
                        crate::util_blas::add_to_scl(&mut ctx.ltj[c * nnz..], &ctx.hj[ctx.j_rowadr[i + r] as usize..], local[r * dim + c], nnz);
                    }
                }
                for r in 0..dim {
                    crate::util_solve::chol_update_sparse(&mut ctx.lcone, &ctx.ltj[r * nnz..], nv, 1,
                        &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind, nnz, &ctx.hj_colind[ctx.j_rowadr[i + r] as usize..], &mut ctx.cholscratch);
                }
            } else {
                zero(&mut ctx.ltj, dim * nv);
                for r in 0..dim {
                    for c in 0..=r { crate::util_blas::add_to_scl(&mut ctx.ltj[c * nv..], &j[(i + r) * nv..], local[r * dim + c], nv); }
                }
                for r in 0..dim { crate::util_solve::chol_update(&mut ctx.lcone, &mut ctx.ltj[r * nv..], nv, 1); }
            }
            ctx.nupdate += dim as i32;
            i += dim - 1;
        }
        i += 1;
    }
}

fn hessian_cone_fold_faster(d: &Data, ctx: &PrimalContext, cost_l: i64) -> i32 {
    let nefc = ctx.nefc;
    let mut u = 0i64;
    let mut s = 0i64;
    let (efc_state, efc_id) = if ctx.island < 0 { (&d.efc_state[..], &d.efc_id[..]) }
        else { (&d.iefc_state[ctx.iefcadr..], &d.iefc_id[ctx.iefcadr..]) };
    let mut i = 0;
    while i < nefc {
        if efc_state[i] == CNSTRSTATE_CONE {
            let nnz = ctx.j_rownnz[i];
            let dim = d.contact[efc_id[i] as usize].dim;
            let lastcol = ctx.hj_colind[(ctx.j_rowadr[i] + nnz - 1) as usize] as usize;
            u += dim as i64 * ctx.pathcost[lastcol] as i64;
            s += dim as i64 * nnz as i64 * nnz as i64;
            i += dim as usize - 1;
        }
        i += 1;
    }
    (cost_l + 10 * u > 3 * (ctx.lflops + 2 * s)) as i32
}

fn hessian_cone_fold(m: &Model, d: &mut Data, ctx: &mut PrimalContext, cost_l: i64) -> i32 {
    if ctx.is_sparse == 0 || ctx.ncone == 0 { return 0; }
    if ctx.cone_fold < 0 {
        ctx.cone_fold = hessian_cone_fold_faster(d, ctx, cost_l);
        if ctx.cone_fold != 0 && ctx.hcone.is_empty() {
            let fold_bytes = crate::memory::stack_bytes(ctx.n_h * size_of::<f64>(), align_of::<f64>());
            let loop_bytes = primal_loop_scratch_bytes(m, ctx, d);
            if crate::memory::stack_bytes_available(d) < fold_bytes + loop_bytes {
                crate::util_errmem::warning(d, WARN_CNSTRFULL, d.narena as i32);
                ctx.cone_fold = 0;
            } else { ctx.hcone = d.scratch.num(ctx.n_h); }
        }
    }
    ctx.cone_fold
}

fn hessian_cone(m: &Model, d: &mut Data, ctx: &mut PrimalContext) {
    if hessian_cone_fold(m, d, ctx, 0) != 0 { hessian_cone_folded(m, d, ctx); return; }
    hessian_cone_update(d, ctx);
}

const PRIMAL_MAX_DOWNDATE: i32 = 32;

fn hessian_incremental(m: &Model, d: &mut Data, ctx: &mut PrimalContext, oldstate: &[i32]) {
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    ctx.nupdate = 0;
    let mut u_quad = 0i64;
    let mut ndowndate_step = 0;
    let efc_state = if ctx.island < 0 { &d.efc_state[..] } else { &d.iefc_state[ctx.iefcadr..] };
    for i in 0..nefc {
        if oldstate[i] != efc_state[i] {
            ctx.cone_fold = -1;
            if ctx.is_sparse != 0 && ((oldstate[i] == CNSTRSTATE_QUADRATIC) != (efc_state[i] == CNSTRSTATE_QUADRATIC)) {
                if oldstate[i] == CNSTRSTATE_QUADRATIC { ndowndate_step += 1; }
                let nnz = ctx.j_rownnz[i];
                if nnz > 0 {
                    let lastcol = ctx.hj_colind[(ctx.j_rowadr[i] + nnz - 1) as usize] as usize;
                    u_quad += ctx.pathcost[lastcol] as i64;
                }
            }
        }
    }
    if ctx.is_sparse != 0 && ndowndate_step > 0 && ctx.ndowndate + ndowndate_step >= PRIMAL_MAX_DOWNDATE {
        factorize_hessian(m, d, ctx, 1); return;
    }
    let refactor_l = ctx.is_sparse != 0 && (ctx.flg_lvalid == 0 || (ctx.lflops > 4096 && 10 * u_quad > 3 * ctx.lflops));
    let cost_l = if refactor_l { 3 * ctx.lflops } else { 10 * u_quad };
    let use_fold = hessian_cone_fold(m, d, ctx, cost_l);
    if refactor_l || use_fold != 0 { ctx.flg_lvalid = 0; }
    let (efc_state, efc_d, j) = if ctx.island < 0 { (&d.efc_state[..], &d.efc_d[..], &d.efc_j[..]) }
        else { (&d.iefc_state[ctx.iefcadr..], &d.iefc_d[ctx.iefcadr..], &ctx.j[..]) };
    for i in 0..nefc {
        let mut flag_update = -1;
        if oldstate[i] != CNSTRSTATE_QUADRATIC && efc_state[i] == CNSTRSTATE_QUADRATIC { flag_update = 1; }
        else if oldstate[i] == CNSTRSTATE_QUADRATIC && efc_state[i] != CNSTRSTATE_QUADRATIC { flag_update = 0; }
        if flag_update != -1 {
            let rank;
            if ctx.is_sparse != 0 {
                let nnz = ctx.j_rownnz[i] as usize;
                let adr = ctx.j_rowadr[i] as usize;
                let scl = if flag_update != 0 { efc_d[i] } else { -efc_d[i] };
                primal_add_row_outer_sparse(&mut ctx.h, &ctx.h_rowadr, &ctx.h_colind, nnz, &ctx.hj_colind[adr..], &ctx.hj[adr..], scl);
                if flag_update == 0 { ctx.flg_hdirty = 1; ctx.ndowndate += 1; }
                if ctx.flg_lvalid == 0 { ctx.nupdate += 1; continue; }
                crate::util_blas::scl(&mut ctx.cholupd, &ctx.hj[adr..], efc_d[i].sqrt(), nnz);
                rank = crate::util_solve::chol_update_sparse(&mut ctx.l, &ctx.cholupd, nv, flag_update,
                    &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind, nnz, &ctx.hj_colind[adr..], &mut ctx.cholscratch);
            } else {
                crate::util_blas::scl(&mut ctx.cholupd, &j[i * nv..], efc_d[i].sqrt(), nv);
                rank = crate::util_solve::chol_update(&mut ctx.l, &mut ctx.cholupd, nv, flag_update);
            }
            ctx.nupdate += 1;
            if rank < nv as i32 { factorize_hessian(m, d, ctx, 1); return; }
        }
    }
    if use_fold == 0 && ctx.is_sparse != 0 && ctx.flg_lvalid == 0 {
        let rank = crate::util_solve::chol_factor_numeric(&mut ctx.l, nv, MINVAL, &ctx.l_rownnz, &ctx.l_rowadr, &ctx.l_colind,
            &ctx.lt_rownnz, &ctx.lt_rowadr, &ctx.lt_colind, &ctx.lt_map, &ctx.h, &ctx.h_rownnz, &ctx.h_rowadr, &ctx.h_colind, &mut ctx.cholscratch);
        ctx.flg_lvalid = 1;
        if rank < nv as i32 && ctx.flg_hdirty != 0 { factorize_hessian(m, d, ctx, 1); return; }
    }
    if ctx.ncone != 0 { hessian_cone(m, d, ctx); }
}

fn sol_primal(m: &Model, d: &mut Data, island: i32, maxiter: i32, mut flg_newton: i32) {
    let mut iter = 0;
    let mut ctx = PrimalContext::default();
    primal_pointers(m, d, &mut ctx, island);
    if island >= 0 && ctx.nefc == 0 {
        let b = ctx.idofadr;
        copy(&mut d.iacc[b..], &d.iacc_smooth[b..], ctx.nv);
        zero(&mut d.ifrc_constraint[b..], ctx.nv);
        if island < NISLAND as i32 { d.solver_niter[island as usize] = 0; }
        ctx.release(d);
        return;
    }
    primal_allocate(m, d, &mut ctx, flg_newton);
    let nv = ctx.nv;
    let nefc = ctx.nefc;
    let mut oldstate = std::mem::take(&mut ctx.oldstate);
    let (m_mat, m_rownnz, m_rowadr, m_colind, qacc) = if ctx.island < 0 {
        (&d.m, &m.m_rownnz, &m.m_rowadr, &m.m_colind, &d.qacc[..])
    } else { (&ctx.m, &ctx.m_rownnz, &ctx.m_rowadr, &ctx.m_colind, &d.iacc[ctx.idofadr..]) };
    crate::util_sparse::mul_sym_vec_sparse(&mut ctx.ma, m_mat, qacc, nv, m_rownnz, m_rowadr, m_colind);
    if ctx.flg_metric != 0 {
        // The metric callee mutably borrows Data for scratch, so copy its input
        // out of Data first, keeping the C call's read-before-write semantics.
        let mut qacc_copy = d.scratch.num(nv);
        let qacc = if ctx.island < 0 { &d.qacc[..] } else { &d.iacc[ctx.idofadr..] };
        copy(&mut qacc_copy, qacc, nv);
        if ctx.island < 0 { crate::metric::eff_mul_add(m, d, &mut ctx.ma, &qacc_copy, 1); }
        else { crate::metric::eff_mul_add_island(m, d, &mut ctx.ma, &qacc_copy, ctx.island as usize); }
        d.scratch.put_num(qacc_copy);
    }
    let (j, j_rownnz, j_rowadr, j_colind, j_rowsuper, qacc, efc_aref) = if ctx.island < 0 {
        (&d.efc_j, &d.efc_j_rownnz, &d.efc_j_rowadr, &d.efc_j_colind, &d.efc_j_rowsuper, &d.qacc[..], &d.efc_aref[..])
    } else { (&ctx.j, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.j_colind, &ctx.j_rowsuper, &d.iacc[ctx.idofadr..], &d.iefc_aref[ctx.iefcadr..]) };
    if ctx.is_sparse == 0 { mul_mat_vec(&mut ctx.jaref, j, qacc, nefc, nv); }
    else { crate::util_sparse::mul_mat_vec_sparse(&mut ctx.jaref, j, qacc, nefc, j_rownnz, j_rowadr, j_colind, Some(j_rowsuper)); }
    crate::util_blas::sub_from(&mut ctx.jaref, efc_aref, nefc);
    primal_update_constraint(d, &mut ctx, flg_newton & (m.opt.cone == CONE_ELLIPTIC) as i32);
    primal_update_grad(d, &mut ctx);
    let scale = inertia_scale(m, d, island);
    ctx.scale = scale;
    if !ctx.ep_l.is_empty() {
        let mut efc_d = d.scratch.num(nefc);
        let src_d = if ctx.island < 0 { &d.efc_d[..] } else { &d.iefc_d[ctx.iefcadr..] };
        copy(&mut efc_d, src_d, nefc);
        // Move the monolithic arrays out just for this call: the callee uses
        // them through its explicit arguments and Data only for metric state.
        let mut j = Vec::new();
        let mut j_rownnz = Vec::new();
        let mut j_rowadr = Vec::new();
        let mut j_colind = Vec::new();
        if ctx.island < 0 {
            j = std::mem::take(&mut d.efc_j);
            j_rownnz = std::mem::take(&mut d.efc_j_rownnz);
            j_rowadr = std::mem::take(&mut d.efc_j_rowadr);
            j_colind = std::mem::take(&mut d.efc_j_colind);
        }
        let (j_in, nnz_in, adr_in, col_in) = if ctx.island < 0 { (&j, &j_rownnz, &j_rowadr, &j_colind) }
            else { (&ctx.j, &ctx.j_rownnz, &ctx.j_rowadr, &ctx.j_colind) };
        let folded = crate::metric::eff_prec_fold(m, d, &mut ctx.ep_l, island, nefc, &efc_d, ctx.is_sparse, j_in, nnz_in, adr_in, col_in);
        if ctx.island < 0 {
            d.efc_j = j;
            d.efc_j_rownnz = j_rownnz;
            d.efc_j_rowadr = j_rowadr;
            d.efc_j_colind = j_colind;
        }
        d.scratch.put_num(efc_d);
        if folded == 0 { d.scratch.put_num(std::mem::take(&mut ctx.ep_l)); }
    }
    primal_update_mgrad(m, d, &mut ctx, 0);
    let flg_gap = max(0.0, 0.5 * scale * dot(&ctx.grad, &ctx.mgrad, nv)) < m.opt.tolerance;
    let flg_gradient = scale * norm(&ctx.grad, nv) < m.opt.tolerance;
    let flg_certificate = flg_gap && (!(flg_newton != 0 || ctx.flg_metric != 0) || flg_gradient);
    let mut flg_done = flg_certificate;
    if !flg_done && flg_newton != 0 {
        if make_hessian(m, d, &mut ctx) != 0 {
            factorize_hessian(m, d, &mut ctx, 0);
            primal_update_mgrad(m, d, &mut ctx, 1);
            flg_done = flg_gradient && max(0.0, 0.5 * scale * dot(&ctx.grad, &ctx.mgrad, nv)) < m.opt.tolerance;
        } else {
            flg_newton = 0;
            flg_done = flg_gap && (ctx.flg_metric == 0 || flg_gradient);
            if !flg_done {
                ctx.gradold = std::mem::take(&mut ctx.cholupd);
                ctx.mgradold = if ctx.is_sparse != 0 { std::mem::take(&mut ctx.cholscratch) } else { std::mem::take(&mut ctx.l) };
                // C carves two vectors from LTJ. Owned Vecs cannot share that
                // allocation, so recycle it and take the two views from Scratch.
                if !ctx.ltj.is_empty() { d.scratch.put_num(std::mem::take(&mut ctx.ltj)); }
                ctx.graddif = d.scratch.num(nv);
                ctx.mgraddif = d.scratch.num(nv);
            }
        }
    }
    if !flg_done { crate::util_blas::scl(&mut ctx.search, &ctx.mgrad, -1.0, nv); }
    while !flg_done && iter < maxiter {
        let mut ls_improvement = 0.0;
        let alpha = primal_search(m, d, &mut ctx, m.opt.tolerance * m.opt.ls_tolerance, m.opt.ls_iterations as f64, &mut ls_improvement);
        if alpha == 0.0 { break; }
        let qacc = if ctx.island < 0 { &mut d.qacc[..] } else { &mut d.iacc[ctx.idofadr..] };
        crate::util_blas::add_to_scl(qacc, &ctx.search, alpha, nv);
        crate::util_blas::add_to_scl(&mut ctx.ma, &ctx.mv, alpha, nv);
        crate::util_blas::add_to_scl(&mut ctx.jaref, &ctx.jv, alpha, nefc);
        if flg_newton == 0 {
            copy(&mut ctx.gradold, &ctx.grad, nv);
            copy(&mut ctx.mgradold, &ctx.mgrad, nv);
        }
        let efc_state = if ctx.island < 0 { &d.efc_state[..] } else { &d.iefc_state[ctx.iefcadr..] };
        crate::util_misc::copy_int(&mut oldstate, efc_state, nefc);
        primal_update_constraint(d, &mut ctx, flg_newton & (m.opt.cone == CONE_ELLIPTIC) as i32);
        if flg_newton != 0 { hessian_incremental(m, d, &mut ctx, &oldstate); }
        primal_update_gradient(m, d, &mut ctx, flg_newton);
        let mut nchange = 0;
        let efc_state = if ctx.island < 0 { &d.efc_state[..] } else { &d.iefc_state[ctx.iefcadr..] };
        for i in 0..nefc { nchange += (efc_state[i] != oldstate[i]) as i32; }
        let improvement = scale * ls_improvement;
        let gradient = scale * norm(&ctx.grad, nv);
        save_stats(m, d, island, iter, improvement, gradient, ctx.lsslope, ctx.nactive, nchange, ctx.lsiter, ctx.nupdate);
        let decrement = if flg_newton != 0 { max(0.0, 0.5 * scale * dot(&ctx.grad, &ctx.mgrad, nv)) } else { 0.0 };
        iter += 1;
        if (improvement > 0.0 && improvement < m.opt.tolerance) || gradient < m.opt.tolerance || (flg_newton != 0 && decrement < m.opt.tolerance) { break; }
        if flg_newton != 0 { crate::util_blas::scl(&mut ctx.search, &ctx.mgrad, -1.0, nv); }
        else {
            // Default C branch (mjCG_PRP is not defined): Hager-Zhang.
            let eta = 0.01;
            sub(&mut ctx.graddif, &ctx.grad, &ctx.gradold, nv);
            sub(&mut ctx.mgraddif, &ctx.mgrad, &ctx.mgradold, nv);
            let d_dot_y = dot(&ctx.search, &ctx.graddif, nv);
            let beta;
            if d_dot_y < MINVAL { beta = 0.0; }
            else {
                let y_dot_my = dot(&ctx.graddif, &ctx.mgraddif, nv);
                let y_dot_mgrad = dot(&ctx.graddif, &ctx.mgrad, nv);
                let d_dot_grad = dot(&ctx.search, &ctx.grad, nv);
                let beta_hz = (y_dot_mgrad - 2.0 * (y_dot_my / d_dot_y) * d_dot_grad) / d_dot_y;
                let d_norm = norm(&ctx.search, nv);
                let grad_norm = norm(&ctx.grad, nv);
                let eta_k = -1.0 / max(MINVAL, d_norm * crate::util_misc::min(eta, grad_norm));
                beta = max(eta_k, beta_hz);
            }
            for i in 0..nv { ctx.search[i] = -ctx.mgrad[i] + beta * ctx.search[i]; }
        }
    }
    if island < NISLAND as i32 {
        let island_stat = if island < 0 { 0 } else { island as usize };
        d.solver_niter[island_stat] += iter;
        if flg_newton != 0 && !flg_certificate {
            if is_sparse(m) != 0 {
                let num_factors = 1 + (!ctx.lcone.is_empty()) as usize;
                d.solver_nnz[island_stat] = (num_factors * ctx.n_l + ctx.n_h) as i32;
            } else { d.solver_nnz[island_stat] = (nv * nv) as i32; }
        } else { d.solver_nnz[island_stat] = ctx.n_j as i32; }
    }
    ctx.oldstate = oldstate;
    ctx.release(d);
}

pub fn sol_cg(m: &Model, d: &mut Data, maxiter: i32) {
    sol_primal(m, d, -1, maxiter, 0);
}

pub fn sol_cg_island(m: &Model, d: &mut Data, island: usize, maxiter: i32) {
    sol_primal(m, d, island as i32, maxiter, 0);
}

pub fn sol_newton(m: &Model, d: &mut Data, maxiter: i32) {
    sol_primal(m, d, -1, maxiter, 1);
}

pub fn sol_newton_island(m: &Model, d: &mut Data, island: usize, maxiter: i32) {
    sol_primal(m, d, island as i32, maxiter, 1);
}

#[cfg(test)]
#[path = "../tests/solver.rs"]
mod test_source;

#[cfg(test)]
mod tests {
    crate::solver_tests!();
}
