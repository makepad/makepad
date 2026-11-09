//! Step rate of models built in code (no model files): `step_rate <model> [steps] [runs]`.
//! Each run starts at qvel = sin(1 + i), takes 100 warm-up steps, then times `steps` steps;
//! prints every run and the median steps/s. Models: hinge_chain (15 capsule links, mm g s, each
//! hinged to the one before about y, z, x in turn, no contacts, at Mechanics' substep
//! rate of 960 Hz).
use makepad_dynamics::builder::compile;
use makepad_dynamics::forward;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::types::*;
use std::f64::consts::PI;

fn hinge_chain() -> ModelSpec {
    let mut s = ModelSpec { modelname: Some("hinge_chain".into()), ..ModelSpec::default() };
    s.option.gravity = [0.0, 0.0, -9810.0];
    s.option.timestep = 1.0 / 960.0;
    // a capsule of radius 2 and length 20 along x, density 0.00124 g/mm^3 (a cylinder and a ball)
    let (r, h, rho) = (2.0, 20.0, 0.00124);
    let (mc, ms) = (rho * PI * r * r * h, rho * 4.0 / 3.0 * PI * r * r * r);
    let along = mc * r * r / 2.0 + ms * 2.0 * r * r / 5.0;
    let across = mc * (3.0 * r * r + h * h) / 12.0 + ms * (2.0 * r * r / 5.0 + h * h / 4.0);
    let axes = [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]];
    for i in 0..15 {
        s.bodies.push(BodySpec {
            name: Some(format!("l{i}")),
            parent: i,
            pos: [if i == 0 { 0.0 } else { h }, 0.0, 0.0],
            mass: mc + ms,
            ipos: [h / 2.0, 0.0, 0.0],
            inertia: [along, across, across],
            joints: vec![JointSpec {
                name: Some(format!("j{i}")),
                r#type: JNT_HINGE,
                axis: axes[i % 3],
                damping: [5.0, 0.0, 0.0],
                ..JointSpec::default()
            }],
            geoms: vec![GeomSpec {
                r#type: GEOM_CAPSULE,
                fromto: [0.0, 0.0, 0.0, h, 0.0, 0.0],
                size: [r, 0.0, 0.0],
                contype: 0,
                conaffinity: 0,
                ..GeomSpec::default()
            }],
            ..BodySpec::default()
        });
    }
    s
}

fn rate(m: &Model, steps: usize, runs: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for _ in 0..runs {
        let mut d = make_data(m);
        for i in 0..m.nv {
            d.qvel[i] = (1.0 + i as f64).sin();
        }
        for _ in 0..100 {
            forward::step(m, &mut d);
        }
        let t0 = std::time::Instant::now();
        for _ in 0..steps {
            forward::step(m, &mut d);
        }
        out.push(steps as f64 / t0.elapsed().as_secs_f64());
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let spec = match args.first().map(String::as_str) {
        Some("hinge_chain") => hinge_chain(),
        _ => {
            eprintln!("usage: step_rate hinge_chain [steps] [runs]");
            std::process::exit(2);
        }
    };
    let steps = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(20000);
    let runs = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(5);
    let m = match compile(&spec) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let mut r = rate(&m, steps, runs);
    for (i, x) in r.iter().enumerate() {
        println!("{}: run {i}: {steps} steps: {x:.0} steps/s", args[0]);
    }
    r.sort_by(f64::total_cmp);
    println!("{}: median {:.0} steps/s (nq {} nv {})", args[0], r[r.len() / 2], m.nq, m.nv);
}
