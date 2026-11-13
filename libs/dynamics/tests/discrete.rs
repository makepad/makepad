//! The discrete (position-implicit) integrator: joint springs as stiff as a flex joint on a small part,
//! where an explicit spring diverges; the deflection it settles at; its metric solve with tendon and
//! actuator couplings, which the backbone factor alone does not hold.

use makepad_dynamics::builder::compile;
use makepad_dynamics::forward::{forward, step};
use makepad_dynamics::io::make_data;
use makepad_dynamics::metric::eff_mul_add;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::support::mul_m;
use makepad_dynamics::types::*;

fn poly(v: f64) -> [f64; NPOLY + 1] {
    let mut p = [0.0; NPOLY + 1];
    p[0] = v;
    p
}

fn spec(name: &str) -> ModelSpec {
    let mut s = ModelSpec { modelname: Some(name.into()), ..ModelSpec::default() };
    s.option.timestep = 1.0 / 1920.0;
    s.option.gravity = [0.0, 0.0, -9.81];
    s.option.integrator = INT_DISCRETE;
    s
}

/// Eight 2 g links hung from the world 10 mm apart, each on a hinge or ball spring as stiff as a flex joint
/// on a small part (omega*h about 20 at 1920 Hz).
fn chain(kind: i32) -> Model {
    let mut s = spec("chain");
    for i in 0..8 {
        s.bodies.push(BodySpec {
            name: Some(format!("link{i}")), parent: i, pos: if i == 0 { [0.0; 3] } else { [0.0, 0.0, -0.01] },
            mass: 0.002, ipos: [0.0, 0.0, -0.005], inertia: [2e-8; 3],
            joints: vec![JointSpec { r#type: kind, axis: [0.0, 1.0, 0.0], stiffness: poly(29.5), damping: poly(4.6e-4), ..JointSpec::default() }],
            ..BodySpec::default()
        });
    }
    compile(&s).expect("compile the chain")
}

#[test]
fn stiff_springs_stay_stable_and_settle_where_the_forces_balance() {
    // kicked chains: discrete damps the kick out, explicit Euler diverges on the same springs
    for (kind, name) in [(JNT_HINGE, "hinge"), (JNT_BALL, "ball")] {
        let m = chain(kind);
        let mut d = make_data(&m);
        d.qvel.iter_mut().for_each(|v| *v = 1.0);
        for _ in 0..1920 {
            step(&m, &mut d);
        }
        assert!(d.warning.iter().all(|w| w.number == 0), "{name}: discrete steps without a warning");
        let fastest = d.qvel.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        assert!(fastest < 1e-3, "{name}: discrete damps the kick out within a second, fastest dof {fastest} rad/s");

        let mut e = m.clone();
        e.opt.integrator = INT_EULER;
        let mut d = make_data(&e);
        d.qvel.iter_mut().for_each(|v| *v = 1.0);
        for _ in 0..200 {
            step(&e, &mut d);
        }
        let bad = [WARN_BADQPOS, WARN_BADQVEL, WARN_BADQACC].iter().map(|&w| d.warning[w as usize].number).sum::<i32>();
        assert!(bad > 0, "{name}: an explicit spring this stiff diverges");
    }

    // a 20 mm arm held level by the stiff hinge spring sags until the spring holds its weight,
    // k*theta = m*g*L*cos(theta)
    let mut s = spec("arm");
    let (k, mass, length) = (29.5, 0.002, 0.02);
    s.bodies.push(BodySpec {
        name: Some("arm".into()), mass, ipos: [length, 0.0, 0.0], inertia: [2e-8; 3],
        joints: vec![JointSpec { r#type: JNT_HINGE, axis: [0.0, 1.0, 0.0], stiffness: poly(k), damping: poly(4.6e-4), ..JointSpec::default() }],
        ..BodySpec::default()
    });
    let m = compile(&s).expect("compile the arm");
    let mut d = make_data(&m);
    for _ in 0..3840 {
        step(&m, &mut d);
    }
    let weight = mass * 9.81 * length;
    let mut theta = weight / k;
    for _ in 0..20 {
        theta -= (k * theta - weight * theta.cos()) / (k + weight * theta.sin());
    }
    assert!((d.qpos[0] - theta).abs() < 1e-9 * theta, "the arm sags to {theta} rad, not {}", d.qpos[0]);

    // two 0.1 kg links tied by a stiff, damped fixed tendon, the second on a position servo: the tendon
    // and the servo enter the metric as rank-1 couplings, so the smooth acceleration comes from its
    // iterative solve, which must meet the tolerance against the whole metric
    let mut s = spec("coupled");
    s.option.timestep = 0.002;
    for i in 0..2 {
        s.bodies.push(BodySpec {
            name: Some(format!("link{i}")), parent: i, pos: if i == 0 { [0.0; 3] } else { [0.2, 0.0, 0.0] },
            mass: 0.1, ipos: [0.1, 0.0, 0.0], inertia: [1e-4, 4e-4, 4e-4],
            joints: vec![JointSpec { name: Some(format!("j{i}")), r#type: JNT_HINGE, axis: [0.0, 1.0, 0.0], damping: poly(0.01), ..JointSpec::default() }],
            ..BodySpec::default()
        });
    }
    s.tendons.push(TendonSpec {
        name: Some("tie".into()), stiffness: poly(400.0), damping: poly(2.0), springlength: [0.3, 0.3],
        path: vec![WrapSpec::joint("j0", 1.0), WrapSpec::joint("j1", 2.0)], ..TendonSpec::default()
    });
    let mut servo = ActuatorSpec { name: Some("servo".into()), ..ActuatorSpec::on(TRN_JOINT, "j1") };
    servo.set_to_position(50.0, Some(1.0), None, None, 0.0).expect("a position servo");
    s.actuators.push(servo);
    let m = compile(&s).expect("compile the coupled links");
    let mut d = make_data(&m);
    d.ctrl[0] = 0.4;
    d.qvel[0] = 2.0;
    for _ in 0..50 {
        step(&m, &mut d);
    }
    forward(&m, &mut d);
    assert!(d.nefm_t == 1 && d.nefm_a == 1, "the tendon and the servo are in the metric");
    let nv = m.nv;
    let mut r = vec![0.0; nv];
    mul_m(&m, &d, &mut r, &d.qacc_smooth);
    eff_mul_add(&m, &d, &mut r, &d.qacc_smooth, 1);
    let mut rr = 0.0;
    let mut bb = 0.0;
    for i in 0..nv {
        let b = d.qfrc_smooth[i] + d.efm_c[i] + d.efm_ca[i];
        rr += (r[i] - b) * (r[i] - b);
        bb += b * b;
    }
    assert!(rr.sqrt() <= 10.0 * m.opt.tolerance * bb.sqrt(), "the metric solve meets its tolerance: residual {} of {}", rr.sqrt(), bb.sqrt());
    assert!(d.warning[WARN_INERTIA as usize].number == 0, "the metric solve converges without a warning");
}
