use makepad_dynamics::builder::compile;
use makepad_dynamics::data::Data;
use makepad_dynamics::forward::{forward, step};
use makepad_dynamics::inverse::compare_fwd_inv;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::support::{get_state, set_state, state_size};
use makepad_dynamics::types::*;

/// A spinning box on a plane (a standalone free body in frictional contact) beside a damped two-link
/// hinge chain whose joints are tied by a joint equality: contacts, an equality, damping and the free
/// body's local gyroscopic block, as in the reference engine's inverse tests.
fn fixture() -> Model {
    let mut s = ModelSpec { modelname: Some("inverse".into()), ..ModelSpec::default() };
    s.option.timestep = 0.002;
    s.option.gravity = [0.0, 0.0, -9.81];
    s.bodies[0].geoms.push(GeomSpec { r#type: GEOM_PLANE, size: [2.0, 2.0, 0.1], friction: [0.2, 0.005, 0.0001], ..GeomSpec::default() });
    s.bodies.push(BodySpec {
        name: Some("box".into()), pos: [0.0, 0.0, 0.1],
        mass: 2.0, ipos: [0.02, -0.01, 0.03], inertia: [0.0216667, 0.0333333, 0.0416667],
        joints: vec![JointSpec { r#type: JNT_FREE, damping: { let mut b = [0.0; NPOLY + 1]; b[0] = 0.01; b }, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_BOX, size: [0.2, 0.15, 0.1], mass: 2.0, pos: [0.02, -0.01, 0.03], friction: [0.2, 0.005, 0.0001], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    for (i, (parent, pos)) in [(0, [1.0, 0.0, 1.0]), (2, [0.0, 0.2, 0.0])].into_iter().enumerate() {
        s.bodies.push(BodySpec {
            name: Some(format!("link{i}")), parent, pos, mass: 1.0, ipos: [0.0; 3], inertia: [0.0141, 0.0141, 0.00125],
            joints: vec![JointSpec {
                name: Some(format!("j{i}")), r#type: JNT_HINGE, axis: [1.0, 0.0, 0.0],
                damping: { let mut b = [0.0; NPOLY + 1]; b[0] = 0.5; b }, ..JointSpec::default()
            }],
            geoms: vec![GeomSpec { r#type: GEOM_CAPSULE, size: [0.05, 0.2, 0.0], mass: 1.0, ..GeomSpec::default() }],
            ..BodySpec::default()
        });
    }
    s.equalities.push(EqualitySpec { r#type: EQ_JOINT, name1: Some("j0".into()), name2: Some("j1".into()), ..EqualitySpec::default() });
    compile(&s).expect("compile the inverse fixture")
}

fn start(m: &Model) -> Data {
    let mut d = make_data(m);
    // spin about the vertical with small tumble components; swing the chain
    d.qvel[3] = 0.5;
    d.qvel[4] = -0.3;
    d.qvel[5] = 20.0;
    d.qvel[6] = 1.0;
    d
}

/// Step at least 70 steps, then on until the box touches the plane (it lands and bounces before it rests).
fn settle(m: &Model, d: &mut Data) {
    for k in 0..1000 {
        step(m, d);
        if k + 1 >= 70 {
            forward(m, d);
            if d.ncon > 0 {
                return;
            }
        }
    }
    panic!("the box never touches the plane");
}

#[test]
fn forward_and_inverse_agree_for_every_solver() {
    let mut m = fixture();
    // a small tolerance and enough iterations for every solver to converge
    m.opt.iterations = 500;
    m.opt.tolerance = 0.0;
    for diagexact in [false, true] {
        if diagexact { m.opt.enableflags |= ENBL_DIAGEXACT } else { m.opt.enableflags &= !ENBL_DIAGEXACT }
        for (solver, name, epsilon) in [(SOL_PGS, "PGS", 1e-6), (SOL_CG, "CG", 1e-9), (SOL_NEWTON, "Newton", 1e-10)] {
            m.opt.solver = solver;
            let mut d = start(&m);
            settle(&m, &mut d);
            compare_fwd_inv(&m, &mut d);
            assert!(d.solver_fwdinv[0] < epsilon, "{name} diagexact {diagexact}: constraint force residual {}", d.solver_fwdinv[0]);
            assert!(d.solver_fwdinv[1] < epsilon, "{name} diagexact {diagexact}: applied force residual {}", d.solver_fwdinv[1]);
        }
    }
}

#[test]
fn discrete_inverse_recovers_the_step_and_keeps_the_factorizations() {
    let base = fixture();
    let nv = base.nv;
    for (integrator, name) in [(INT_EULER, "Euler"), (INT_IMPLICIT, "implicit"), (INT_IMPLICITFAST, "implicitfast")] {
        let mut residual = [0.0; 2];
        for invdiscrete in [false, true] {
            let mut m = base.clone();
            m.opt.integrator = integrator;
            if invdiscrete { m.opt.enableflags |= ENBL_INVDISCRETE }
            let mut d = start(&m);
            settle(&m, &mut d);
            // the step's finite-differenced acceleration
            let mut state = vec![0.0; state_size(&m, STATE_INTEGRATION)];
            get_state(&m, &d, &mut state, STATE_INTEGRATION);
            step(&m, &mut d);
            let qvel_next = d.qvel.clone();
            set_state(&m, &mut d, &state, STATE_INTEGRATION);
            let qacc_fd: Vec<f64> = (0..nv).map(|i| (qvel_next[i] - d.qvel[i]) / m.opt.timestep).collect();
            forward(&m, &mut d);
            assert!(d.ncon > 0 && d.qvel[5].abs() > 1.0, "{name}: the box spins on the plane");
            // the forward pass's factorizations, which the inverse must leave as they are
            let (q_lu, q_h) = (d.q_lu.clone(), d.q_h.clone());
            d.qacc.copy_from_slice(&qacc_fd);
            compare_fwd_inv(&m, &mut d);
            assert!(d.q_lu == q_lu && d.q_h == q_h, "{name} invdiscrete {invdiscrete}: the forward factorizations are unchanged");
            residual[invdiscrete as usize] = d.solver_fwdinv[0].max(d.solver_fwdinv[1]);
        }
        assert!(residual[1] < 5e-7, "{name}: the discrete inverse recovers the step's forces, residual {}", residual[1]);
        assert!(residual[0] > 1e3 * residual[1], "{name}: the continuous inverse does not ({} against {})", residual[0], residual[1]);
    }
}

#[test]
fn discrete_inverse_adds_no_damping_when_the_damper_is_off() {
    // a damped slide held by a joint equality under Euler with the damper disabled: Euler integrates
    // without the implicit damping term, so the discrete inverse must not add it either
    let mut s = ModelSpec { modelname: Some("slide".into()), ..ModelSpec::default() };
    s.option.timestep = 0.01;
    s.option.integrator = INT_EULER;
    s.option.enableflags |= ENBL_INVDISCRETE;
    s.option.disableflags |= DSBL_DAMPER;
    s.bodies.push(BodySpec {
        name: Some("ball".into()), mass: 1.0, ipos: [0.0; 3], inertia: [0.004; 3],
        joints: vec![JointSpec { name: Some("slide".into()), r#type: JNT_SLIDE, damping: { let mut b = [0.0; NPOLY + 1]; b[0] = 5.0; b }, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_SPHERE, size: [0.1, 0.0, 0.0], mass: 1.0, ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    s.equalities.push(EqualitySpec { r#type: EQ_JOINT, name1: Some("slide".into()), ..EqualitySpec::default() });
    let m = compile(&s).expect("compile the slide");
    let mut d = make_data(&m);
    d.qvel[0] = 1.0;
    d.qfrc_applied[0] = 3.0;
    let mut state = vec![0.0; state_size(&m, STATE_INTEGRATION)];
    get_state(&m, &d, &mut state, STATE_INTEGRATION);
    step(&m, &mut d);
    let qacc_fd = (d.qvel[0] - 1.0) / m.opt.timestep;
    set_state(&m, &mut d, &state, STATE_INTEGRATION);
    forward(&m, &mut d);
    assert!(d.nefc > 0, "the equality holds the slide");
    d.qacc[0] = qacc_fd;
    compare_fwd_inv(&m, &mut d);
    assert!(d.solver_fwdinv[0] < 1e-9 && d.solver_fwdinv[1] < 1e-9, "residuals {:?}", d.solver_fwdinv);
}
