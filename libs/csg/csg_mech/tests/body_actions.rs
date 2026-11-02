//! The Mechanics action menu's per-body actions: each changes the chosen body's state.

use makepad_csg_mech::geom::P3;
use makepad_csg_mech::runner::HZ;
use makepad_csg_mech::sim::{Assembly, BodyAction, BodySpec, MechWorld, GRAVITY};

const DT: f64 = 1.0 / HZ;

fn cube(at: P3) -> BodySpec {
    let vertices: Vec<P3> = (0..8).map(|i| std::array::from_fn(|k| at[k] + if i & (1 << k) == 0 { -5.0 } else { 5.0 })).collect();
    let triangles = [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
    makepad_csg_mech::mesh_body("cube", &vertices, &triangles, 0.00124, 1.0, &[])
}

/// Two cubes lying on a floor, at x = 0 and x = 40.
fn world() -> MechWorld {
    let asm = Assembly { bodies: vec![cube([0.0; 3]), cube([40.0, 0.0, 0.0])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(-5.05));
    run(&mut w, 60);
    w
}

fn run(w: &mut MechWorld, steps: usize) {
    for _ in 0..steps {
        w.step(DT);
    }
}

fn z(w: &MechWorld, i: usize) -> f64 {
    w.poses()[i].com[2]
}

#[test]
fn drop_lifts_a_resting_body_and_it_falls_back_alone() {
    let mut w = world();
    let rest = (z(&w, 0), z(&w, 1));
    w.apply(&[0], &BodyAction::Drop { lift: 60.0 });
    assert!((z(&w, 0) - rest.0 - 60.0).abs() < 0.5, "lifted: {}", z(&w, 0));
    assert!((z(&w, 1) - rest.1).abs() < 0.01, "the other body stays");
    run(&mut w, 240);
    assert!((z(&w, 0) - rest.0).abs() < 0.5, "back on the floor: {}", z(&w, 0));
}

#[test]
fn hang_pins_the_body_and_drop_lets_it_go() {
    let mut w = world();
    let rest = z(&w, 0);
    w.apply(&[0], &BodyAction::Hang);
    assert_eq!(w.pinned(), vec![0]);
    run(&mut w, 240);
    // hanging from a pin above it: lifted off the floor, the other body untouched
    assert!(z(&w, 0) > rest + 5.0, "hangs clear of the floor: {} vs {rest}", z(&w, 0));
    w.apply(&[0], &BodyAction::Pin);
    assert_eq!(w.pinned(), vec![0], "pinning twice keeps one pin");
    w.apply(&[0], &BodyAction::Unpin);
    assert!(w.pinned().is_empty());
    w.apply(&[0], &BodyAction::Pin);
    assert_eq!(w.pinned(), vec![0]);
    w.apply(&[0], &BodyAction::Drop { lift: 60.0 });
    assert!(w.pinned().is_empty());
}

#[test]
fn throw_home_and_freeze() {
    let mut w = world();
    w.apply(&[1], &BodyAction::Throw([0.0, 0.0, 2000.0]));
    run(&mut w, 12);
    assert!(z(&w, 1) > z(&w, 0) + 5.0, "thrown up: {}", z(&w, 1));
    w.apply(&[1], &BodyAction::Freeze(true));
    assert_eq!(w.frozen(), vec![1]);
    let held = w.poses()[1];
    run(&mut w, 60);
    assert!((w.poses()[1].com[2] - held.com[2]).abs() < 1.0, "frozen in the air");
    w.apply(&[1], &BodyAction::Freeze(false));
    assert!(w.frozen().is_empty());
    run(&mut w, 240);
    assert!(z(&w, 1) < 1.0, "falls again once thawed: {}", z(&w, 1));
    w.apply(&[1], &BodyAction::Throw([500.0, 0.0, 0.0]));
    run(&mut w, 60);
    assert!(w.poses()[1].com[0] > 45.0);
    w.apply(&[1], &BodyAction::Home);
    let p = w.poses()[1];
    assert!((p.com[0] - 40.0).abs() < 0.01 && p.com[2].abs() < 0.01);
}
