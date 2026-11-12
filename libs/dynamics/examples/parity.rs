//! Parity traces: step a model file and write what each step computed, or compare two traces
//! bit for bit (this crate's against the reference engine's, CREDITS.md).
//!
//!   parity trace <model file> <steps> <qvel scale> <out> [<ctrl scale> [<integrator>]]
//!   parity diff <a> <b>
//!   parity speed <model file> <steps> <qvel scale> <runs>
//!   parity inverse <model file> <steps> <qvel scale> <out> <integrator> <enable flags>
//!
//! The start state: reset, then qvel[i] = qvel_scale * sin(1 + i); every step is one `step`. With a
//! ctrl scale, before step s: ctrl[i] = ctrl_scale * sin(0.01 s + 1 + i); an integrator replaces the model's.
//! Trace file, little-endian, version 1:
//!   "MPTR", i32 version, i32 nq, i32 nv, i32 na, i32 nsteps
//!   per step: f64 time, f64 qpos[nq], f64 qvel[nv], f64 act[na], f64 qacc[nv],
//!             i32 ncon, i32 nefc, i32 solver_niter[0], i32 nisland,
//!             f64 efc_force[nefc],
//!             per contact: i32 geom[2], f64 dist, f64 pos[3], f64 frame[9]
//! Inverse file (`inverse`: the integrator replaces the model's, the flags are or-ed into its enable flags;
//! after each step: forward, compare_fwd_inv, then inverse), little-endian, version 1:
//!   "MPIV", i32 version, i32 nq, i32 nv, i32 nsensordata, i32 nsteps
//!   per step: f64 time, i32 nefc, f64 solver_fwdinv[2], f64 qfrc_inverse[nv], f64 qfrc_constraint[nv],
//!             f64 efc_force[nefc], f64 sensordata[nsensordata]

use makepad_dynamics::data::Data;
use makepad_dynamics::forward;
use makepad_dynamics::inverse;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_file::load_model_with;
use makepad_dynamics::sdf::SdfPlugin;
use makepad_dynamics::sdf_torus::Torus;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.at + n > self.b.len() {
            return Err(format!("the trace ends at byte {} (wanted {} more)", self.b.len(), n));
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64s(&mut self, n: usize) -> Result<Vec<f64>, String> {
        Ok(self.take(8 * n)?.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
    }
}

struct Contact {
    geom: [i32; 2],
    dist: f64,
    pos: Vec<f64>,
    frame: Vec<f64>,
}

struct Step {
    time: f64,
    qpos: Vec<f64>,
    qvel: Vec<f64>,
    act: Vec<f64>,
    qacc: Vec<f64>,
    ncon: i32,
    nefc: i32,
    niter: i32,
    nisland: i32,
    efc_force: Vec<f64>,
    contacts: Vec<Contact>,
}

struct Trace {
    nq: usize,
    nv: usize,
    na: usize,
    steps: Vec<Step>,
}

fn read_trace(path: &str) -> Result<Trace, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let mut r = Reader { b: &b, at: 0 };
    if r.take(4)? != b"MPTR" {
        return Err(format!("{path}: not a trace"));
    }
    let version = r.i32()?;
    if version != 1 {
        return Err(format!("{path}: trace version {version}, this reads 1"));
    }
    let (nq, nv, na, n) = (r.i32()? as usize, r.i32()? as usize, r.i32()? as usize, r.i32()? as usize);
    let mut steps = Vec::with_capacity(n);
    for _ in 0..n {
        let time = r.f64s(1)?[0];
        let qpos = r.f64s(nq)?;
        let qvel = r.f64s(nv)?;
        let act = r.f64s(na)?;
        let qacc = r.f64s(nv)?;
        let (ncon, nefc, niter, nisland) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
        let efc_force = r.f64s(nefc.max(0) as usize)?;
        let mut contacts = Vec::with_capacity(ncon.max(0) as usize);
        for _ in 0..ncon {
            let geom = [r.i32()?, r.i32()?];
            let dist = r.f64s(1)?[0];
            contacts.push(Contact { geom, dist, pos: r.f64s(3)?, frame: r.f64s(9)? });
        }
        steps.push(Step { time, qpos, qvel, act, qacc, ncon, nefc, niter, nisland, efc_force, contacts });
    }
    Ok(Trace { nq, nv, na, steps })
}

/// How far apart two doubles are in units in the last place (0: the same bits; u64::MAX: one is NaN).
fn ulps(a: f64, b: f64) -> u64 {
    if a.to_bits() == b.to_bits() {
        return 0;
    }
    if a.is_nan() || b.is_nan() {
        return u64::MAX;
    }
    let key = |x: f64| {
        let i = x.to_bits() as i64;
        if i < 0 {
            i64::MIN - i
        } else {
            i
        }
    };
    key(a).abs_diff(key(b))
}

#[derive(Default)]
struct FieldStat {
    differ: usize,
    max_abs: f64,
    max_ulps: u64,
    first: Option<(usize, usize, f64, f64)>,
}

struct Diff {
    fields: BTreeMap<&'static str, FieldStat>,
    identical_steps: usize,
    structure: Option<String>,
}

impl Diff {
    fn values(&mut self, step: usize, field: &'static str, a: &[f64], b: &[f64]) -> bool {
        let f = self.fields.entry(field).or_default();
        let mut same = true;
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            let u = ulps(*x, *y);
            if u != 0 {
                same = false;
                f.differ += 1;
                f.max_abs = f.max_abs.max((x - y).abs());
                f.max_ulps = f.max_ulps.max(u);
                if f.first.is_none() {
                    f.first = Some((step, i, *x, *y));
                }
            }
        }
        same
    }
}

fn diff(a: &Trace, b: &Trace) -> Diff {
    let mut d = Diff { fields: BTreeMap::new(), identical_steps: 0, structure: None };
    if (a.nq, a.nv, a.na) != (b.nq, b.nv, b.na) {
        d.structure = Some(format!("sizes differ: nq nv na {} {} {} against {} {} {}", a.nq, a.nv, a.na, b.nq, b.nv, b.na));
        return d;
    }
    for (k, (x, y)) in a.steps.iter().zip(&b.steps).enumerate() {
        let mut same = d.values(k, "time", &[x.time], &[y.time]);
        same &= d.values(k, "qpos", &x.qpos, &y.qpos);
        same &= d.values(k, "qvel", &x.qvel, &y.qvel);
        same &= d.values(k, "act", &x.act, &y.act);
        same &= d.values(k, "qacc", &x.qacc, &y.qacc);
        if (x.ncon, x.nefc, x.nisland) != (y.ncon, y.nefc, y.nisland) {
            d.structure = Some(format!("step {k}: ncon nefc nisland {} {} {} against {} {} {}", x.ncon, x.nefc, x.nisland, y.ncon, y.nefc, y.nisland));
            break;
        }
        if x.niter != y.niter {
            same = false;
            let f = d.fields.entry("solver_niter").or_default();
            f.differ += 1;
            if f.first.is_none() {
                f.first = Some((k, 0, x.niter as f64, y.niter as f64));
            }
        }
        same &= d.values(k, "efc_force", &x.efc_force, &y.efc_force);
        for (c, (p, q)) in x.contacts.iter().zip(&y.contacts).enumerate() {
            if p.geom != q.geom {
                d.structure = Some(format!("step {k}: contact {c} is geoms {:?} against {:?}", p.geom, q.geom));
                break;
            }
            same &= d.values(k, "contact.dist", &[p.dist], &[q.dist]);
            same &= d.values(k, "contact.pos", &p.pos, &q.pos);
            same &= d.values(k, "contact.frame", &p.frame, &q.frame);
        }
        if d.structure.is_some() {
            break;
        }
        if same {
            d.identical_steps += 1;
        }
    }
    if d.structure.is_none() && a.steps.len() != b.steps.len() {
        d.structure = Some(format!("{} steps against {}", a.steps.len(), b.steps.len()));
    }
    d
}

fn run_diff(pa: &str, pb: &str) -> Result<bool, String> {
    let (a, b) = (read_trace(pa)?, read_trace(pb)?);
    let d = diff(&a, &b);
    let n = a.steps.len().min(b.steps.len());
    let bad: Vec<_> = d.fields.iter().filter(|(_, f)| f.differ > 0).collect();
    if bad.is_empty() && d.structure.is_none() {
        println!("{pa} vs {pb}: bit-identical over {n} steps (nq {} nv {} na {})", a.nq, a.nv, a.na);
        return Ok(true);
    }
    println!("{pa} vs {pb}: {} of {n} steps identical", d.identical_steps);
    for (name, f) in bad {
        let (s, i, x, y) = f.first.unwrap();
        println!("  {name}: {} values differ, max |a-b| {:.3e}, max {} ulps; first at step {s} index {i}: {x:e} against {y:e}", f.differ, f.max_abs, f.max_ulps);
    }
    if let Some(s) = &d.structure {
        println!("  structure: {s}");
    }
    Ok(false)
}


/// Attach the fields a parity model's plugins name: the reference's SDF parity case is a torus.
fn attach(m: &mut Model) -> Result<(), String> {
    for i in 0..m.nplugin {
        let torus = Torus::from_plugin(m, i)?;
        m.sdf_plugins.push(SdfPlugin(Arc::new(torus)));
    }
    Ok(())
}

fn put(out: &mut Vec<u8>, v: &[f64]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn step_record(m: &Model, d: &Data, out: &mut Vec<u8>) {
    put(out, &[d.time]);
    put(out, &d.qpos[..m.nq]);
    put(out, &d.qvel[..m.nv]);
    put(out, &d.act[..m.na]);
    put(out, &d.qacc[..m.nv]);
    put_i32(out, d.ncon as i32);
    put_i32(out, d.nefc as i32);
    put_i32(out, d.solver_niter[0]);
    put_i32(out, d.nisland as i32);
    put(out, &d.efc_force[..d.nefc as usize]);
    for c in &d.contact[..d.ncon as usize] {
        put_i32(out, c.geom[0]);
        put_i32(out, c.geom[1]);
        put(out, &[c.dist]);
        put(out, &c.pos);
        put(out, &c.frame);
    }
}

/// The reference driver's start state: reset, then qvel[i] = scale * sin(1 + i).
fn start(m: &Model, scale: f64) -> Data {
    let mut d = make_data(m);
    for i in 0..m.nv {
        d.qvel[i] = scale * (1.0 + i as f64).sin();
    }
    d
}

fn run_trace(model: &str, steps: usize, scale: f64, out_path: &str, ctrl: Option<f64>, integrator: Option<i32>) -> Result<bool, String> {
    let mut m = load_model_with(Path::new(model), attach)?;
    if let Some(i) = integrator {
        m.opt.integrator = i;
    }
    let mut d = start(&m, scale);
    let mut out = Vec::new();
    out.extend_from_slice(b"MPTR");
    for v in [1, m.nq as i32, m.nv as i32, m.na as i32, steps as i32] {
        put_i32(&mut out, v);
    }
    for s in 0..steps {
        if let Some(c) = ctrl {
            for i in 0..m.nu {
                d.ctrl[i] = c * (0.01 * s as f64 + 1.0 + i as f64).sin();
            }
        }
        forward::step(&m, &mut d);
        step_record(&m, &d, &mut out);
    }
    std::fs::File::create(out_path).and_then(|mut f| f.write_all(&out)).map_err(|e| format!("{out_path}: {e}"))?;
    println!("{model}: {steps} steps, last ncon {} nefc {} -> {out_path}", d.ncon, d.nefc);
    Ok(true)
}

/// Inverse dynamics after each step: forward, the forward/inverse comparison, then the inverse itself.
fn run_inverse(model: &str, steps: usize, scale: f64, out_path: &str, integrator: i32, flags: i32) -> Result<bool, String> {
    let mut m = load_model_with(Path::new(model), attach)?;
    m.opt.integrator = integrator;
    m.opt.enableflags |= flags;
    let mut d = start(&m, scale);
    let mut out = Vec::new();
    out.extend_from_slice(b"MPIV");
    for v in [1, m.nq as i32, m.nv as i32, m.nsensordata as i32, steps as i32] {
        put_i32(&mut out, v);
    }
    for _ in 0..steps {
        forward::step(&m, &mut d);
        forward::forward(&m, &mut d);
        inverse::compare_fwd_inv(&m, &mut d);
        put(&mut out, &[d.time]);
        put_i32(&mut out, d.nefc as i32);
        put(&mut out, &d.solver_fwdinv);
        inverse::inverse(&m, &mut d);
        put(&mut out, &d.qfrc_inverse[..m.nv]);
        put(&mut out, &d.qfrc_constraint[..m.nv]);
        put(&mut out, &d.efc_force[..d.nefc as usize]);
        put(&mut out, &d.sensordata[..m.nsensordata]);
    }
    std::fs::File::create(out_path).and_then(|mut f| f.write_all(&out)).map_err(|e| format!("{out_path}: {e}"))?;
    println!("{model}: {steps} steps, last nefc {} fwdinv {:.3e} {:.3e} -> {out_path}", d.nefc, d.solver_fwdinv[0], d.solver_fwdinv[1]);
    Ok(true)
}

/// The ray casts after each step: the sensors every step, and every `every` steps, from three points
/// (two about the model's centre, one beside its first body), a lattice of 342 directions cast one by
/// one (all geoms with normals; a group, static and body filter without) and all at once (the same,
/// the filtered cast within the model's extent).
fn run_rays(model: &str, steps: usize, every: usize, scale: f64, out_path: &str) -> Result<bool, String> {
    use makepad_dynamics::ray::{multi_ray, ray, NGROUP};
    use makepad_dynamics::types::MAXVAL;
    let m = load_model_with(Path::new(model), attach)?;
    let mut d = start(&m, scale);
    let mut vec = Vec::new();
    for a in -3..=3 {
        for b in -3..=3 {
            for c in -3..=3 {
                if (a, b, c) != (0, 0, 0) {
                    vec.extend([a as f64, b as f64, c as f64]);
                }
            }
        }
    }
    let nray = vec.len() / 3;
    let groups: [u8; NGROUP] = [1, 0, 1, 1, 0, 1];
    let body = if m.nbody > 1 { 1 } else { 0 };
    let bodyexclude = if m.nbody > 1 { 1 } else { -1 };
    let mut out = Vec::new();
    out.extend_from_slice(b"MPRY");
    for v in [1, m.nq as i32, m.nv as i32, m.ngeom as i32, m.nsensordata as i32, steps as i32, every as i32, nray as i32] {
        put_i32(&mut out, v);
    }
    let (mut dist, mut geomid, mut normal) = (vec![0.0; nray], vec![0; nray], vec![0.0; 3 * nray]);
    for s in 0..steps {
        forward::step(&m, &mut d);
        forward::forward(&m, &mut d);
        put(&mut out, &[d.time]);
        put(&mut out, &d.sensordata[..m.nsensordata]);
        if s % every != 0 {
            continue;
        }
        let (c, e) = (m.stat.center, m.stat.extent);
        let x = &d.xpos[3 * body..3 * body + 3];
        let origins = [
            [c[0] + e * 0.05, c[1] - e * 1.5, c[2] + e * 0.8],
            [c[0] + e * 0.7, c[1] + e * 0.3, c[2] + e * 1.2],
            [x[0] + e * 0.01, x[1] + e * 0.02, x[2] + e * 0.03],
        ];
        for pnt in &origins {
            put(&mut out, pnt);
            for k in 0..nray {
                let v: &[f64; 3] = vec[3 * k..3 * k + 3].try_into().unwrap();
                let (mut g, mut n) = (0, [0.0; 3]);
                let x = ray(&m, &d, pnt, v, None, true, -1, Some(&mut g), Some(&mut n));
                put(&mut out, &[x]);
                put_i32(&mut out, g);
                put(&mut out, &n);
            }
            for k in 0..nray {
                let v: &[f64; 3] = vec[3 * k..3 * k + 3].try_into().unwrap();
                let mut g = 0;
                let x = ray(&m, &d, pnt, v, Some(&groups), false, bodyexclude, Some(&mut g), None);
                put(&mut out, &[x]);
                put_i32(&mut out, g);
            }
            multi_ray(&m, &mut d, pnt, &vec, None, true, -1, Some(&mut geomid), &mut dist, Some(&mut normal), nray, MAXVAL);
            put(&mut out, &dist);
            geomid.iter().for_each(|&g| put_i32(&mut out, g));
            put(&mut out, &normal);
            multi_ray(&m, &mut d, pnt, &vec, Some(&groups), false, bodyexclude, Some(&mut geomid), &mut dist, None, nray, e);
            put(&mut out, &dist);
            geomid.iter().for_each(|&g| put_i32(&mut out, g));
        }
    }
    std::fs::File::create(out_path).and_then(|mut f| f.write_all(&out)).map_err(|e| format!("{out_path}: {e}"))?;
    println!("{model}: {steps} steps, {nray} rays from 3 points every {every} steps -> {out_path}");
    Ok(true)
}

/// Step rate: 100 warm-up steps, then `steps` timed, `runs` times, as the reference driver does.
fn run_speed(model: &str, steps: usize, scale: f64, runs: usize) -> Result<bool, String> {
    let m = load_model_with(Path::new(model), attach)?;
    for r in 0..runs {
        let mut d = start(&m, scale);
        for _ in 0..100 {
            forward::step(&m, &mut d);
        }
        let t0 = std::time::Instant::now();
        for _ in 0..steps {
            forward::step(&m, &mut d);
        }
        let t = t0.elapsed().as_secs_f64();
        println!("{model}: run {r}: {steps} steps in {t:.4} s: {:.0} steps/s", steps as f64 / t);
    }
    Ok(true)
}

/// The step's stages timed one by one (sensors left out), after 100 warm-up steps.
fn run_stage_speed(model: &str, steps: usize, scale: f64) -> Result<bool, String> {
    use makepad_dynamics::types::{INT_EULER, INT_RK4};
    let m = load_model_with(Path::new(model), attach)?;
    let mut d = start(&m, scale);
    for _ in 0..100 {
        forward::step(&m, &mut d);
    }
    let mut t = [0.0f64; 6];
    for _ in 0..steps {
        let t0 = std::time::Instant::now();
        forward::check_pos(&m, &mut d);
        forward::check_vel(&m, &mut d);
        forward::fwd_position(&m, &mut d);
        let t1 = std::time::Instant::now();
        forward::fwd_velocity(&m, &mut d);
        let t2 = std::time::Instant::now();
        forward::fwd_actuation(&m, &mut d);
        let t3 = std::time::Instant::now();
        forward::fwd_acceleration(&m, &mut d);
        let t4 = std::time::Instant::now();
        forward::fwd_constraint(&m, &mut d);
        forward::check_acc(&m, &mut d);
        let t5 = std::time::Instant::now();
        match m.opt.integrator {
            INT_EULER => forward::euler(&m, &mut d),
            INT_RK4 => forward::runge_kutta(&m, &mut d, 4),
            _ => forward::implicit(&m, &mut d),
        }
        let t6 = std::time::Instant::now();
        for (i, (a, b)) in [(t0, t1), (t1, t2), (t2, t3), (t3, t4), (t4, t5), (t5, t6)].iter().enumerate() {
            t[i] += (*b - *a).as_secs_f64();
        }
    }
    let us = |x: f64| 1e6 * x / steps as f64;
    println!("{model}: integrator {}: us/step position {:.3} velocity {:.3} actuation {:.3} acceleration {:.3} constraint {:.3} integrate {:.3}",
        m.opt.integrator, us(t[0]), us(t[1]), us(t[2]), us(t[3]), us(t[4]), us(t[5]));
    Ok(true)
}

/// Time individual position/velocity kernels at the warmed state. Repeating each
/// kernel separately isolates its cost without adding timers to production code.
fn run_kernel_speed(model: &str, steps: usize, scale: f64) -> Result<bool, String> {
    use makepad_dynamics::{collision_driver, core_constraint, core_smooth, passive};
    let m = load_model_with(Path::new(model), attach)?;
    let mut warm = start(&m, scale);
    for _ in 0..100 {
        forward::step(&m, &mut warm);
    }
    forward::forward(&m, &mut warm);
    let kernels: &[(&str, fn(&Model, &mut Data))] = &[
        ("kinematics", core_smooth::kinematics),
        ("com_pos", core_smooth::com_pos),
        ("camlight", core_smooth::camlight),
        ("tendon", core_smooth::tendon),
        ("crb", core_smooth::crb),
        ("factor_m", core_smooth::factor_m),
        ("collision", collision_driver::collision),
        ("make_constraint", core_constraint::make_constraint),
        ("make_m", core_smooth::make_m),
        ("island", makepad_dynamics::island::island),
        ("transmission", core_smooth::transmission),
        ("project_constraint", core_constraint::project_constraint),
        ("com_vel", core_smooth::com_vel),
        ("passive", passive::passive),
        ("rne", |m, d| core_smooth::rne(m, d, 0, None)),
    ];
    for &(name, kernel) in kernels {
        let mut d = warm.clone();
        kernel(&m, &mut d);
        let t0 = std::time::Instant::now();
        for _ in 0..steps {
            kernel(&m, &mut d);
        }
        let us = t0.elapsed().as_secs_f64() * 1e6 / steps as f64;
        println!("{model}: kernel {name}: {us:.3} us/call");
    }
    Ok(true)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["diff", a, b] => run_diff(a, b),
        ["trace", model, steps, scale, out] => match (steps.parse(), scale.parse()) {
            (Ok(n), Ok(q)) => run_trace(model, n, q, out, None, None),
            _ => Err("trace: steps and qvel scale are numbers".to_string()),
        },
        ["trace", model, steps, scale, out, ctrl] => match (steps.parse(), scale.parse(), ctrl.parse()) {
            (Ok(n), Ok(q), Ok(c)) => run_trace(model, n, q, out, Some(c), None),
            _ => Err("trace: steps, qvel scale and ctrl scale are numbers".to_string()),
        },
        ["trace", model, steps, scale, out, ctrl, integrator] => match (steps.parse(), scale.parse(), ctrl.parse(), integrator.parse()) {
            (Ok(n), Ok(q), Ok(c), Ok(i)) => run_trace(model, n, q, out, Some(c), Some(i)),
            _ => Err("trace: steps, qvel scale, ctrl scale and integrator are numbers".to_string()),
        },
        ["inverse", model, steps, scale, out, integrator, flags] => match (steps.parse(), scale.parse(), integrator.parse(), flags.parse()) {
            (Ok(n), Ok(q), Ok(i), Ok(f)) => run_inverse(model, n, q, out, i, f),
            _ => Err("inverse: steps, qvel scale, integrator and enable flags are numbers".to_string()),
        },
        ["rays", model, steps, every, scale, out] => match (steps.parse(), every.parse(), scale.parse()) {
            (Ok(n), Ok(k), Ok(q)) if k > 0 => run_rays(model, n, k, q, out),
            _ => Err("rays: steps, a positive ray interval and qvel scale are numbers".to_string()),
        },
        ["speed", model, steps, scale, runs] => match (steps.parse(), scale.parse(), runs.parse()) {
            (Ok(n), Ok(q), Ok(r)) => run_speed(model, n, q, r),
            _ => Err("speed: steps, qvel scale and runs are numbers".to_string()),
        },
        ["kernelspeed", model, steps, scale] => match (steps.parse(), scale.parse()) {
            (Ok(n), Ok(q)) => run_kernel_speed(model, n, q),
            _ => Err("kernelspeed: steps and qvel scale are numbers".to_string()),
        },
        ["stagespeed", model, steps, scale] => match (steps.parse(), scale.parse()) {
            (Ok(n), Ok(q)) => run_stage_speed(model, n, q),
            _ => Err("stagespeed: steps and qvel scale are numbers".to_string()),
        },
        _ => Err("usage: parity trace <model file> <steps> <qvel scale> <out> [<ctrl scale> [<integrator>]] | parity diff <a> <b> | parity inverse <model file> <steps> <qvel scale> <out> <integrator> <enable flags> | parity rays <model file> <steps> <ray interval> <qvel scale> <out> | parity speed <model file> <steps> <qvel scale> <runs> | parity stagespeed <model file> <steps> <qvel scale> | parity kernelspeed <model file> <steps> <qvel scale>".to_string()),
    };
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
}
