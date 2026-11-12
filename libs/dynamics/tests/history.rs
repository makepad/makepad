use makepad_dynamics::builder::compile;
use makepad_dynamics::forward::step;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_file::{load_model_buffer, save_model};
use makepad_dynamics::model_spec::*;
use makepad_dynamics::support::{init_ctrl_history, read_ctrl};
use makepad_dynamics::types::*;

/// The reference's delay test model: a slide driven by a motor whose control arrives `delay` seconds
/// late, through a history buffer of `nsample` samples, at a timestep of 0.01; built, then saved and
/// loaded as a model file.
fn delayed_slide(delay: f64, nsample: i32, interp: i32, integrator: i32) -> Model {
    let mut s = ModelSpec::default();
    s.option.timestep = 0.01;
    s.option.integrator = integrator;
    s.bodies.push(BodySpec {
        name: Some("slider".into()), mass: 1.0, ipos: [0.0; 3], inertia: [0.004; 3],
        joints: vec![JointSpec { name: Some("slide".into()), r#type: JNT_SLIDE, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_SPHERE, size: [0.1, 0.0, 0.0], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    let mut motor = ActuatorSpec { delay, nsample, interp, ..ActuatorSpec::on(TRN_JOINT, "slide") };
    motor.set_to_motor().expect("a motor");
    s.actuators.push(motor);
    let built = compile(&s).expect("compile the delayed slide");
    load_model_buffer(&save_model(&built)).expect("a model with a history buffer loads")
}

#[test]
fn delayed_controls_arrive_through_the_history_buffer() {
    // zero-order hold: a control set before the first step arrives on the third, two steps late
    for (integrator, name) in [(INT_EULER, "Euler"), (INT_IMPLICITFAST, "implicitfast")] {
        let m = delayed_slide(0.02, 2, 0, integrator);
        assert_eq!((m.actuator_history[0], m.nhistory), (2, 2 + 2 + 2), "{name}: two samples of one control");
        let mut d = make_data(&m);
        d.ctrl[0] = 10.0;
        let forces: Vec<f64> = (0..3).map(|_| { step(&m, &mut d); d.actuator_force[0] }).collect();
        assert_eq!(forces, [0.0, 0.0, 10.0], "{name}: the control arrives 0.02 s late");
    }

    // linear interpolation between samples: a delay of a step and a half reads halfway between two
    let m = delayed_slide(0.015, 3, 1, INT_EULER);
    let mut d = make_data(&m);
    let mut forces = Vec::new();
    for ctrl in [10.0, 20.0, 30.0] {
        d.ctrl[0] = ctrl;
        step(&m, &mut d);
        forces.push(d.actuator_force[0]);
    }
    for (force, want) in forces.iter().zip([0.0, 5.0, 15.0]) {
        assert!((force - want).abs() < 1e-10, "linear: forces {forces:?}, expected [0, 5, 15]");
    }

    // a history set by hand, read at any time and order: held at the ends, linear and cubic between
    let m = delayed_slide(0.01, 4, 0, INT_EULER);
    let mut d = make_data(&m);
    init_ctrl_history(&m, &mut d, 0, Some(&[0.0, 1.0, 2.0, 3.0]), Some(&[0.0, 1.0, 4.0, 9.0]));
    let read = |d: &_, t: f64, interp: i32| {
        let mut res = [f64::NAN];
        // the delay is subtracted from the time read
        read_ctrl(&m, d, 0, t + 0.01, &mut res, interp).map_or(res[0], |v| v[0])
    };
    assert_eq!((read(&d, -1.0, 2), read(&d, 5.0, 2)), (0.0, 9.0), "held before the oldest and after the newest sample");
    assert_eq!(read(&d, 1.5, 0), 1.0, "zero-order hold reads the sample before");
    assert!((read(&d, 1.5, 1) - 2.5).abs() < 1e-12, "linear halfway between 1 and 4");
    // Catmull-Rom slopes (4 - 0) / 2 and (9 - 1) / 2 at the bracket's ends
    assert!((read(&d, 1.5, 2) - (0.5 * 1.0 + 0.125 * 2.0 + 0.5 * 4.0 - 0.125 * 4.0)).abs() < 1e-12, "cubic between 1 and 4");
    assert!((read(&d, 1.5, -1) - 1.0).abs() < 1e-12, "-1 reads with the actuator's own interpolation");
}

#[test]
fn a_delay_needs_a_history_buffer_and_an_input() {
    let mut s = ModelSpec::default();
    s.bodies.push(BodySpec {
        mass: 1.0, ipos: [0.0; 3], inertia: [0.004; 3],
        joints: vec![JointSpec { name: Some("slide".into()), r#type: JNT_SLIDE, ..JointSpec::default() }],
        ..BodySpec::default()
    });
    let mut motor = ActuatorSpec { delay: 0.02, ..ActuatorSpec::on(TRN_JOINT, "slide") };
    motor.set_to_motor().expect("a motor");
    s.actuators.push(motor);
    let error = compile(&s).expect_err("a delay without samples");
    assert!(error.contains("without a history buffer"), "{error}");
    s.actuators[0].nsample = 2;
    s.option.timestep = 0.0;
    let error = compile(&s).expect_err("a history at timestep 0");
    assert!(error.contains("positive timestep"), "{error}");
}
