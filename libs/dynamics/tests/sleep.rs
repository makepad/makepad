use makepad_dynamics::builder::compile;
use makepad_dynamics::forward::step;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::sleep::{sleep_cycle, wake_island};
use makepad_dynamics::types::*;

#[test]
fn sleep_cycles_and_wake_islands() {
    // Hand trace: 0 -> 2 -> 1 -> 0; 3 -> 3. The first cycle's minimum is 0.
    let mut tree_asleep = [2, 0, 1, 3];
    for i in 0..3 {
        assert_eq!(
            sleep_cycle(&tree_asleep, 4, i),
            0,
            "three-tree cycle at {i}"
        );
    }
    assert_eq!(sleep_cycle(&tree_asleep, 4, 3), 3, "singleton cycle");
    assert_eq!(
        tree_asleep,
        [2, 0, 1, 3],
        "cycle lookup does not modify links"
    );

    // Starting at 1 writes -11 in order 1, 0, 2, then stops on returning to 1.
    assert_eq!(
        wake_island(&mut tree_asleep, 4, 1, -11, Some("test"), 0.0),
        3,
        "wake entire cycle"
    );
    assert_eq!(tree_asleep, [-11, -11, -11, 3], "singleton remains asleep");
    assert_eq!(
        sleep_cycle(&tree_asleep, 4, 1),
        -1,
        "awake tree has no cycle"
    );

    // Already awake: min(-5, -11) keeps -11; min(-12, -11) resets just tree 0.
    assert_eq!(
        wake_island(&mut tree_asleep, 4, 0, -5, None, 0.0),
        0,
        "already awake count"
    );
    assert_eq!(
        tree_asleep,
        [-11, -11, -11, 3],
        "do not shorten awake countdown"
    );
    assert_eq!(
        wake_island(&mut tree_asleep, 4, 0, -12, None, 0.0),
        0,
        "reset awake countdown"
    );
    assert_eq!(
        tree_asleep,
        [-12, -11, -11, 3],
        "former cycle no longer linked"
    );

    assert_eq!(
        wake_island(&mut tree_asleep, 4, 3, -11, None, 0.0),
        1,
        "wake singleton"
    );
    assert_eq!(tree_asleep, [-12, -11, -11, -11], "all trees awake");
}

#[test]
fn a_box_on_a_still_mocap_sleeps_and_wakes_when_it_moves() {
    // A 20 mm cube dropped 0.5 mm onto a mocap platform (mm, g, s).
    let mut spec = ModelSpec::default();
    spec.option.gravity = [0.0, 0.0, -9810.0];
    spec.option.timestep = 1.0 / 240.0;
    spec.option.enableflags |= ENBL_SLEEP;
    spec.option.sleep_tolerance = 2.0;
    spec.bodies.push(BodySpec {
        name: Some("platform".into()), mocap: true,
        geoms: vec![GeomSpec { r#type: GEOM_BOX, size: [50.0, 50.0, 5.0], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    spec.bodies.push(BodySpec {
        name: Some("box".into()), pos: [0.0, 0.0, 15.5], mass: 8.0, inertia: [533.0; 3],
        joints: vec![JointSpec { r#type: JNT_FREE, limited: 0, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_BOX, size: [10.0; 3], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    let m = compile(&spec).expect("compile the box on a platform");
    let mut d = make_data(&m);
    let slept = (0..960).position(|_| { step(&m, &mut d); d.tree_asleep[0] >= 0 });
    assert!(slept.is_some(), "the box comes to rest on the still platform and sleeps within 4 s");
    for s in 0..240 {
        step(&m, &mut d);
        assert!(d.tree_asleep[0] >= 0, "a still mocap body wakes nothing it touches: the box woke {s} steps after it slept");
    }
    d.mocap_pos[2] += 1.0;
    step(&m, &mut d);
    assert!(d.tree_asleep[0] < 0, "the platform moving wakes the box on it");
}
