//! Stage parity: replay a stages file written by the reference engine (CREDITS.md) through
//! this crate, one function at a time, and compare every array it wrote bit for bit.
//!
//!   stage_parity <model file> <stages file>...
//!
//! A stages file holds the state the reference reached ("input." arrays), then the outputs of
//! each smooth-dynamics function run on that state ("<function>.<array>"), little-endian:
//!   "MPST", i32 version 1, i32 narrays,
//!   per array: u8 namelen, name, u8 kind (0 f64, 1 i32), i32 count, values.
//! Before each function runs, the arrays the previous functions wrote are set to the
//! reference's values, so a difference belongs to the function that shows it.

use makepad_dynamics::{core_smooth, forward, passive};
use makepad_dynamics::data::{Contact, Data};
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_file::load_model_with;
use makepad_dynamics::sdf::SdfPlugin;
use makepad_dynamics::sdf_torus::Torus;
use std::sync::Arc;
use std::path::Path;

enum Values {
    Num(Vec<f64>),
    Int(Vec<i32>),
}

struct Array {
    stage: String,
    name: String,
    values: Values,
}

fn read_stages(path: &Path) -> Result<Vec<Array>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        if at + n > b.len() {
            return Err(format!("{}: ends at byte {}", path.display(), b.len()));
        }
        at += n;
        Ok(&b[at - n..at])
    };
    if take(4)? != b"MPST" {
        return Err(format!("{}: not a stages file", path.display()));
    }
    let word = |s: &[u8]| i32::from_le_bytes(s.try_into().unwrap());
    let version = word(take(4)?);
    if version != 1 {
        return Err(format!("{}: version {version}, this reads 1", path.display()));
    }
    let n = word(take(4)?) as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let len = take(1)?[0] as usize;
        let full = String::from_utf8_lossy(take(len)?).to_string();
        let kind = take(1)?[0];
        let count = word(take(4)?) as usize;
        let values = if kind == 0 {
            Values::Num(take(8 * count)?.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
        } else {
            Values::Int(take(4 * count)?.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect())
        };
        let (stage, name) = full.split_once('.').ok_or_else(|| format!("array name {full} has no stage"))?;
        out.push(Array { stage: stage.to_string(), name: name.to_string(), values });
    }
    Ok(out)
}

/// The Data array a stages file names (the C name), as a mutable slice.
fn field<'a>(d: &'a mut Data, name: &str) -> Option<Field<'a>> {
    macro_rules! map {
        ($($c:literal => $f:ident: $k:ident,)*) => {
            match name {
                "time" => Some(Field::Num(std::slice::from_mut(&mut d.time))),
                "ncon" => Some(Field::Int(std::slice::from_mut(&mut d.ncon))),
                "nefc" => Some(Field::Int(std::slice::from_mut(&mut d.nefc))),
                "nisland" => Some(Field::Int(std::slice::from_mut(&mut d.nisland))),
                "solver_niter" => Some(Field::Int(&mut d.solver_niter[..1])),
                $($c => Some(Field::$k(&mut d.$f[..])),)*
                _ => None,
            }
        };
    }
    map! {
        "qpos" => qpos: Num, "qvel" => qvel: Num, "act" => act: Num, "ctrl" => ctrl: Num,
        "qacc_warmstart" => qacc_warmstart: Num, "qfrc_applied" => qfrc_applied: Num,
        "xfrc_applied" => xfrc_applied: Num, "mocap_pos" => mocap_pos: Num, "mocap_quat" => mocap_quat: Num,
        "xpos" => xpos: Num, "xquat" => xquat: Num, "xmat" => xmat: Num, "xipos" => xipos: Num,
        "ximat" => ximat: Num, "xanchor" => xanchor: Num, "xaxis" => xaxis: Num,
        "geom_xpos" => geom_xpos: Num, "geom_xmat" => geom_xmat: Num,
        "site_xpos" => site_xpos: Num, "site_xmat" => site_xmat: Num,
        "subtree_com" => subtree_com: Num, "cinert" => cinert: Num, "cdof" => cdof: Num,
        "cam_xpos" => cam_xpos: Num, "cam_xmat" => cam_xmat: Num,
        "light_xpos" => light_xpos: Num, "light_xdir" => light_xdir: Num,
        "ten_wrapadr" => ten_wrapadr: Int, "ten_wrapnum" => ten_wrapnum: Int,
        "ten_J" => ten_j: Num, "ten_length" => ten_length: Num,
        "wrap_obj" => wrap_obj: Int, "wrap_xpos" => wrap_xpos: Num,
        "crb" => crb: Num, "M" => m: Num, "qLD" => q_ld: Num, "qLDiagInv" => q_ldiag_inv: Num,
        "actuator_length" => actuator_length: Num, "moment_rownnz" => moment_rownnz: Int,
        "moment_rowadr" => moment_rowadr: Int, "moment_colind" => moment_colind: Int,
        "actuator_moment" => actuator_moment: Num,
        "cvel" => cvel: Num, "cdof_dot" => cdof_dot: Num,
        "qfrc_spring" => qfrc_spring: Num, "qfrc_damper" => qfrc_damper: Num,
        "qfrc_gravcomp" => qfrc_gravcomp: Num, "qfrc_fluid" => qfrc_fluid: Num,
        "qfrc_adhesion" => qfrc_adhesion: Num, "qfrc_passive" => qfrc_passive: Num,
        "qfrc_bias" => qfrc_bias: Num,
        "efc_type" => efc_type: Int, "efc_id" => efc_id: Int, "efc_J" => efc_j: Num,
        "efc_J_rownnz" => efc_j_rownnz: Int, "efc_J_rowadr" => efc_j_rowadr: Int,
        "efc_J_colind" => efc_j_colind: Int, "efc_island" => efc_island: Int,
        "efc_pos" => efc_pos: Num, "efc_margin" => efc_margin: Num,
        "efc_frictionloss" => efc_frictionloss: Num, "efc_diagA" => efc_diag_a: Num,
        "efc_KBIP" => efc_kbip: Num, "efc_D" => efc_d: Num, "efc_R" => efc_r: Num,
        "efc_vel" => efc_vel: Num, "efc_aref" => efc_aref: Num,
        "efc_force" => efc_force: Num, "efc_state" => efc_state: Int,
        "actuator_force" => actuator_force: Num, "qfrc_actuator" => qfrc_actuator: Num,
        "act_dot" => act_dot: Num, "qfrc_smooth" => qfrc_smooth: Num, "qacc_smooth" => qacc_smooth: Num,
        "qfrc_constraint" => qfrc_constraint: Num, "qacc" => qacc: Num,
    }
}

enum Field<'a> {
    Num(&'a mut [f64]),
    Int(&'a mut [i32]),
}

/// Run one stage of this crate on `d`; false when this tool has no such stage yet (its
/// arrays are then taken from the reference, so later stages still run on its state).
fn run(stage: &str, m: &Model, d: &mut Data) -> bool {
    match stage {
        "kinematics" => core_smooth::kinematics(m, d),
        "com_pos" => core_smooth::com_pos(m, d),
        "camlight" => core_smooth::camlight(m, d),
        "tendon" => core_smooth::tendon(m, d),
        "make_m" => core_smooth::make_m(m, d),
        "factor_m" => core_smooth::factor_m(m, d),
        "transmission" => core_smooth::transmission(m, d),
        "com_vel" => core_smooth::com_vel(m, d),
        "passive" => passive::passive(m, d),
        "fwd_position" => forward::fwd_position(m, d),
        "fwd_velocity" => forward::fwd_velocity(m, d),
        "fwd_actuation" => forward::fwd_actuation(m, d),
        "fwd_acceleration" => forward::fwd_acceleration(m, d),
        "fwd_constraint" => forward::fwd_constraint(m, d),
        "rne" => {
            let mut bias = std::mem::take(&mut d.qfrc_bias);
            core_smooth::rne(m, d, 0, Some(&mut bias));
            d.qfrc_bias = bias;
        }
        _ => return false,
    }
    true
}

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

/// Print how `ours` differs from `theirs`; true when bit for bit the same.
fn diff_num(stage: &str, name: &str, ours: &[f64], theirs: &[f64]) -> bool {
    let mut differ = 0;
    let mut worst = 0u64;
    let mut first = None;
    for (i, (x, y)) in ours.iter().zip(theirs).enumerate() {
        let u = ulps(*x, *y);
        if u != 0 {
            differ += 1;
            worst = worst.max(u);
            first.get_or_insert((i, *x, *y));
        }
    }
    if let Some((i, x, y)) = first {
        println!("  {stage}.{name}: {differ} of {} differ, max {worst} ulps; first [{i}] ours {x:e} reference {y:e}", theirs.len());
    }
    differ == 0
}

fn diff_int(stage: &str, name: &str, ours: &[i32], theirs: &[i32]) -> bool {
    let differ = ours.iter().zip(theirs).filter(|(x, y)| x != y).count();
    if let Some(i) = ours.iter().zip(theirs).position(|(x, y)| x != y) {
        println!("  {stage}.{name}: {differ} of {} differ; first [{i}] ours {} reference {}", theirs.len(), ours[i], theirs[i]);
    }
    differ == 0
}

/// The contact field a stages file names "contact_<field>", gathered over the contacts
/// (`k` values per contact), and a setter that scatters values back.
fn contact_field(name: &str) -> Option<(usize, fn(&mut Contact) -> ContactField<'_>)> {
    fn num(s: &mut [f64]) -> ContactField<'_> {
        ContactField::Num(s)
    }
    fn int(s: &mut [i32]) -> ContactField<'_> {
        ContactField::Int(s)
    }
    Some(match name {
        "dist" => (1, |c| num(std::slice::from_mut(&mut c.dist))),
        "pos" => (3, |c| num(&mut c.pos)),
        "frame" => (9, |c| num(&mut c.frame)),
        "includemargin" => (1, |c| num(std::slice::from_mut(&mut c.includemargin))),
        "friction" => (5, |c| num(&mut c.friction)),
        "solref" => (2, |c| num(&mut c.solref)),
        "solreffriction" => (2, |c| num(&mut c.solreffriction)),
        "solimp" => (5, |c| num(&mut c.solimp)),
        "mu" => (1, |c| num(std::slice::from_mut(&mut c.mu))),
        "H" => (36, |c| num(&mut c.h)),
        "dim" => (1, |c| int(std::slice::from_mut(&mut c.dim))),
        "geom" => (2, |c| int(&mut c.geom)),
        "exclude" => (1, |c| int(std::slice::from_mut(&mut c.exclude))),
        "efc_address" => (1, |c| int(std::slice::from_mut(&mut c.efc_address))),
        _ => return None,
    })
}

enum ContactField<'a> {
    Num(&'a mut [f64]),
    Int(&'a mut [i32]),
}

/// Compare a contact array over the first ncon contacts, then set ours to the reference's.
fn compare_contacts(stage: &str, a: &Array, d: &mut Data, quiet: bool) -> Result<bool, String> {
    let name = &a.name["contact_".len()..];
    let (k, get) = contact_field(name).ok_or_else(|| format!("no contact field for {}", a.name))?;
    let ncon = d.ncon as usize;
    if d.contact.len() < ncon {
        return Err(format!("{stage}.{}: {} contacts, ncon {ncon}", a.name, d.contact.len()));
    }
    let contacts = &mut d.contact[..ncon];
    let same = match &a.values {
        Values::Num(theirs) if theirs.len() == k * ncon => {
            let mut ours = Vec::with_capacity(k * ncon);
            for c in contacts.iter_mut() {
                if let ContactField::Num(s) = get(c) {
                    ours.extend_from_slice(s);
                }
            }
            let same = quiet || ours.len() == theirs.len() && diff_num(stage, &a.name, &ours, theirs);
            for (c, v) in contacts.iter_mut().zip(theirs.chunks_exact(k)) {
                if let ContactField::Num(s) = get(c) {
                    s.copy_from_slice(v);
                }
            }
            same
        }
        Values::Int(theirs) if theirs.len() == k * ncon => {
            let mut ours = Vec::with_capacity(k * ncon);
            for c in contacts.iter_mut() {
                if let ContactField::Int(s) = get(c) {
                    ours.extend_from_slice(s);
                }
            }
            let same = quiet || ours.len() == theirs.len() && diff_int(stage, &a.name, &ours, theirs);
            for (c, v) in contacts.iter_mut().zip(theirs.chunks_exact(k)) {
                if let ContactField::Int(s) = get(c) {
                    s.copy_from_slice(v);
                }
            }
            same
        }
        _ => return Err(format!("{stage}.{}: kind or length differs", a.name)),
    };
    Ok(same)
}

/// Compare one array; on a difference print it. Then set ours to the reference's values.
/// Constraint arrays are compared over the first nefc rows (the buffers only grow).
fn compare(stage: &str, a: &Array, d: &mut Data, quiet: bool) -> Result<bool, String> {
    if a.name.starts_with("contact_") {
        return compare_contacts(stage, a, d, quiet);
    }
    let prefix = a.name.starts_with("efc_");
    let f = field(d, &a.name).ok_or_else(|| format!("no data field for {}", a.name))?;
    let fits = |ours: usize, theirs: usize| if prefix { ours >= theirs } else { ours == theirs };
    let same = match (f, &a.values) {
        (Field::Num(ours), Values::Num(theirs)) if fits(ours.len(), theirs.len()) => {
            let ours = &mut ours[..theirs.len()];
            let same = quiet || diff_num(stage, &a.name, ours, theirs);
            ours.copy_from_slice(theirs);
            same
        }
        (Field::Int(ours), Values::Int(theirs)) if fits(ours.len(), theirs.len()) => {
            let ours = &mut ours[..theirs.len()];
            let same = quiet || diff_int(stage, &a.name, ours, theirs);
            ours.copy_from_slice(theirs);
            same
        }
        (Field::Num(ours), Values::Num(theirs)) => return Err(format!("{stage}.{}: length {} against {}", a.name, ours.len(), theirs.len())),
        (Field::Int(ours), Values::Int(theirs)) => return Err(format!("{stage}.{}: length {} against {}", a.name, ours.len(), theirs.len())),
        _ => return Err(format!("{stage}.{}: kind differs", a.name)),
    };
    // the counts come first: size the buffers the arrays after them land in
    if a.name == "ncon" {
        let ncon = d.ncon as usize;
        if d.contact.len() < ncon {
            d.contact.resize(ncon, Contact::default());
        }
    }
    Ok(same)
}

/// Attach the fields a parity model's plugins name: the reference's SDF parity case is a torus.
fn attach(m: &mut Model) -> Result<(), String> {
    for i in 0..m.nplugin {
        let torus = Torus::from_plugin(m, i)?;
        m.sdf_plugins.push(SdfPlugin(Arc::new(torus)));
    }
    Ok(())
}

fn replay(model: &Path, stages: &Path) -> Result<bool, String> {
    let m = load_model_with(model, attach)?;
    let mut d = make_data(&m);
    let arrays = read_stages(stages)?;
    let mut all_same = true;
    let mut i = 0;
    while i < arrays.len() {
        let stage = arrays[i].stage.clone();
        let end = i + arrays[i..].iter().take_while(|a| a.stage == stage).count();
        let ran = stage != "input" && run(&stage, &m, &mut d);
        let mut same = true;
        for a in &arrays[i..end] {
            same &= compare(&stage, a, &mut d, !ran)?;
        }
        if stage != "input" {
            let verdict = if !ran { "skipped (not in this tool yet)" } else if same { "identical" } else { "DIFFERS" };
            println!("{}: {stage}: {verdict}", stages.display());
        }
        all_same &= same;
        i = end;
    }
    Ok(all_same)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: stage_parity <model file> <stages file>...");
        std::process::exit(2);
    }
    let mut ok = true;
    for s in &args[1..] {
        match replay(Path::new(&args[0]), Path::new(s)) {
            Ok(same) => ok &= same,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }
    if !ok {
        std::process::exit(1);
    }
}
