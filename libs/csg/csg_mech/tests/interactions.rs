use makepad_box3d::body::{body_get_angular_velocity, body_get_linear_velocity, body_get_local_point, body_get_world_point, body_get_world_point_velocity, body_set_angular_velocity};
use makepad_box3d::math_functions::{to_pos, to_vec3, vec3};
use makepad_csg_mech::geom::{add, len, scale, sub, P3};
use makepad_csg_mech::runner::{HZ, POINTER_HZ};
use makepad_csg_mech::sim::{Assembly, BodySpec, Friction, JointKind, JointSpec, MechWorld, GRAVITY};

const DT: f64 = 1.0 / HZ;

fn cube(at: P3) -> BodySpec {
    let vertices: Vec<P3> = (0..8).map(|i| std::array::from_fn(|k| at[k] + if i & (1 << k) == 0 { -5.0 } else { 5.0 })).collect();
    let triangles = [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
    makepad_csg_mech::mesh_body("cube", &vertices, &triangles, 0.00124, 1.0, &[])
}

fn drag(w: &mut MechWorld) {
    let start = w.poses()[0].com;
    w.grab(0, start, POINTER_HZ);
    for i in 1..=120 {
        w.drag(add(start, [240.0 * i as f64 * DT, 0.0, 0.0]), DT);
        w.step(DT);
    }
}

#[test]
fn dragged_free_body_flies_at_the_release_velocity() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0; 3], None);
    drag(&mut w);
    assert!(w.poses()[0].com[0] > 110.0, "the firm grab must follow the moving target");
    w.throw([480.0, 0.0, 0.0]);
    w.release();
    assert_eq!(w.grabbed(), None);
    let mut last = w.poses()[0].com;
    for _ in 0..2 {
        for _ in 0..24 {
            w.step(DT);
        }
        let now = w.poses()[0].com;
        assert!((now[0] - last[0] - 48.0).abs() < 1.0, "flight: {last:?} -> {now:?}");
        last = now;
    }
}

#[test]
fn throw_carries_transitively_jointed_neighbours_but_not_other_bodies() {
    let mut asm = Assembly { bodies: vec![cube([0.0; 3]), cube([12.0, 0.0, 0.0]), cube([24.0, 0.0, 0.0]), cube([0.0, 100.0, 0.0])], up: [0.0, 0.0, 1.0], ..Default::default() };
    for i in 0..2 {
        asm.joints.push(JointSpec { name: format!("joint {i}"), a: i, b: i + 1, pivot: [6.0 + i as f64 * 12.0, 0.0, 0.0], axis: [0.0, 0.0, 1.0], kind: JointKind::Revolute { lower: -3.0, upper: 3.0 }, friction: Friction::default() });
    }
    let mut w = MechWorld::new(&asm, [0.0; 3], None);
    drag(&mut w);
    let before: Vec<_> = w.bodies.iter().map(|&b| body_get_linear_velocity(&w.world, b)).collect();
    w.throw([480.0, 0.0, 0.0]);
    w.release();
    let dv = 480.0 - before[0].x;
    for (i, b) in w.bodies.iter().enumerate().take(3) {
        let v = body_get_linear_velocity(&w.world, *b);
        assert!((v.x - before[i].x - dv).abs() < 0.05, "body {i}: {v:?}");
    }
    assert_eq!(body_get_linear_velocity(&w.world, w.bodies[3]), before[3]);
    let start = w.poses();
    for _ in 0..48 {
        w.step(DT);
    }
    for (i, p) in w.poses().iter().enumerate().take(3) {
        assert!((p.com[0] - start[i].com[0] - 96.0).abs() < 2.0, "body {i} lost the assembly's throw: {p:?}");
    }
}

#[test]
fn touching_bodies_do_not_share_a_throw() {
    let asm = Assembly { bodies: vec![cube([0.0; 3]), cube([0.0, 9.99, 0.0])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0; 3], None);
    w.step(DT);
    assert!(w.contact_begins().iter().any(|(a, b, _)| *a == 0 && *b == Some(1)));
    let other = body_get_linear_velocity(&w.world, w.bodies[1]);
    w.grab(0, w.poses()[0].com, POINTER_HZ);
    w.throw([300.0, 0.0, 0.0]);
    w.release();
    // No step here: subsequent contact impulses are physical, but the throw itself must not
    // treat a touching body as part of the held body's joint island.
    assert_eq!(body_get_linear_velocity(&w.world, w.bodies[1]), other);
}

#[test]
fn release_keeps_the_springs_motion_and_throw_keeps_spin() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0; 3], None);
    drag(&mut w);
    let b = w.bodies[0];
    let v = body_get_linear_velocity(&w.world, b);
    assert!(v.x > 200.0);
    w.release();
    assert_eq!(body_get_linear_velocity(&w.world, b), v);
    let start = w.poses()[0].com;
    w.step(DT);
    assert!(w.poses()[0].com[0] > start[0] + 0.8);

    // The spring target is deliberately away from the actual held point at release.
    let at = add(w.poses()[0].com, [0.0, 4.0, 0.0]);
    let local = body_get_local_point(&w.world, b, to_pos(vec3(at[0] as f32, at[1] as f32, at[2] as f32)));
    w.grab(0, at, POINTER_HZ);
    w.drag(add(at, [30.0, 0.0, 0.0]), DT);
    w.step(DT);
    body_set_angular_velocity(&mut w.world, b, vec3(0.0, 0.0, 2.0));
    let spin = body_get_angular_velocity(&w.world, b);
    w.throw([300.0, 0.0, 0.0]);
    w.release();
    assert_eq!(body_get_angular_velocity(&w.world, b), spin);
    let at = body_get_world_point(&w.world, b, local);
    let v = body_get_world_point_velocity(&w.world, b, at);
    assert!((v.x - 300.0).abs() < 0.01 && v.y.abs() < 0.01 && v.z.abs() < 0.01, "{v:?}");
}

#[test]
fn point_pin_holds_allows_swing_and_toggles_off() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], None);
    let b = w.bodies[0];
    let point = [5.0, 0.0, 5.0];
    let local = body_get_local_point(&w.world, b, to_pos(vec3(5.0, 0.0, 5.0)));
    w.pin(0, point);
    assert_eq!(w.pinned(), vec![0]);
    let mut swung: f64 = 0.0;
    for _ in 0..240 {
        w.step(DT);
        swung = swung.max(len(w.poses()[0].com));
    }
    let p = to_vec3(body_get_world_point(&w.world, b, local));
    assert!(len(sub([p.x as f64, p.y as f64, p.z as f64], point)) < 0.1);
    assert!(swung > 1.0, "the point pin must let the body swing");
    w.pin(0, point);
    assert!(w.pinned().is_empty());
    let v = body_get_linear_velocity(&w.world, b).z;
    for _ in 0..24 {
        w.step(DT);
    }
    assert!(body_get_linear_velocity(&w.world, b).z < v - 900.0);
}

#[test]
fn poke_changes_linear_velocity_and_spins_at_an_offset() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0; 3], None);
    w.poke(0, [0.0, 4.0, 0.0], [120.0, 0.0, 0.0]);
    assert!((body_get_linear_velocity(&w.world, w.bodies[0]).x - 120.0).abs() < 0.01);
    assert!(body_get_angular_velocity(&w.world, w.bodies[0]).z < -1.0);
}

#[test]
fn dangle_sags_and_oscillates_where_a_firm_grab_holds() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut firm = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], None);
    let mut soft = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], None);
    firm.grab(0, [0.0; 3], POINTER_HZ);
    soft.dangle(0, [0.0; 3]);
    for _ in 0..120 {
        firm.drag([0.0; 3], DT);
        soft.drag([0.0; 3], DT);
        firm.step(DT);
        soft.step(DT);
    }
    assert!(firm.poses()[0].com[2].abs() < 1.0);
    assert!(soft.poses()[0].com[2] < -10.0);
    soft.release();
    let from = soft.poses()[0].com;
    let v = body_get_linear_velocity(&soft.world, soft.bodies[0]);
    for _ in 0..24 {
        soft.step(DT);
    }
    let ballistic = add(add(from, scale([v.x as f64, v.y as f64, v.z as f64], 0.1)), [0.0, 0.0, -0.5 * GRAVITY * 0.1 * 0.1]);
    assert!(len(sub(soft.poses()[0].com, ballistic)) < 1.0);
}

/// Mechanical energy per unit mass, with the cube's spin counted (mm and s).
fn energy(w: &MechWorld) -> f64 {
    kinetic(w) + GRAVITY * w.poses()[0].com[2]
}

/// Kinetic energy per unit mass (mm^2 / s^2).
fn kinetic(w: &MechWorld) -> f64 {
    let v = body_get_linear_velocity(&w.world, w.bodies[0]);
    let a = body_get_angular_velocity(&w.world, w.bodies[0]);
    0.5 * (v.x * v.x + v.y * v.y + v.z * v.z) as f64 + 0.5 * 16.7 * (a.x * a.x + a.y * a.y + a.z * a.z) as f64
}

#[test]
fn a_dangled_body_loses_energy_and_never_gains_it() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], None);
    // held by an off-centre point so it swings and spins
    w.dangle(0, [8.0, 0.0, 8.0]);
    let mut target = [8.0, 0.0, 8.0];
    // shake the target about, then hold it still
    for i in 0..120 {
        target[0] = 8.0 + 30.0 * (i as f64 * 0.1).sin();
        w.drag(target, DT);
        w.step(DT);
    }
    let mut peaks = Vec::new();
    for _ in 0..6 {
        let mut peak = f64::NEG_INFINITY;
        for _ in 0..240 {
            w.drag(target, DT);
            w.step(DT);
            peak = peak.max(kinetic(&w));
        }
        peaks.push(peak);
    }
    // the motion's energy falls from second to second (the spring's stiffness acts on the held point's
    // effective mass, so its stored energy is not known from outside: the motion is what is measured)
    for k in 1..peaks.len() {
        assert!(peaks[k] <= peaks[k - 1] + 1.0, "energy grew in second {k}: {peaks:?}");
    }
    // a soft dangle, shaken hard, settles within two seconds of holding still: slower than 1 mm/s
    assert!(peaks[2] < 0.5, "still swinging after two seconds: {peaks:?}");
}

#[test]
fn a_firm_grab_and_a_pin_do_not_gain_energy() {
    let asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    for pin in [false, true] {
        let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], None);
        if pin {
            w.pin(0, [8.0, 0.0, 8.0]);
        } else {
            w.grab(0, [8.0, 0.0, 8.0], POINTER_HZ);
        }
        let mut first = 0.0;
        for i in 0..(240 * 6) {
            if !pin {
                w.drag([8.0, 0.0, 8.0], DT);
            }
            w.step(DT);
            if i == 240 * 2 {
                first = (0..240).map(|_| { w.step(DT); energy(&w) }).fold(f64::NEG_INFINITY, f64::max);
            }
        }
        let last = (0..240).map(|_| { w.step(DT); energy(&w) }).fold(f64::NEG_INFINITY, f64::max);
        assert!(last <= first + 50.0, "pin={pin}: energy {first} -> {last}");
    }
}

#[test]
fn a_dense_field_hull_still_collides_with_the_ground() {
    // many points on a vase-like surface of revolution overrun a hull's limits: it must still stand on the floor
    let mut pts = Vec::new();
    for k in 0..40 {
        let z = k as f64 * 1.6;
        let r = 10.0 + 8.0 * (z * 0.12).sin();
        for a in 0..90 {
            let t = a as f64 * std::f64::consts::TAU / 90.0;
            pts.push([r * t.cos(), r * t.sin(), z]);
        }
    }
    let mut asm = Assembly { bodies: vec![cube([0.0; 3])], up: [0.0, 0.0, 1.0], ..Default::default() };
    asm.bodies[0].hulls = vec![pts];
    asm.bodies[0].mass.com = [0.0, 0.0, 30.0];
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(-0.05));
    for _ in 0..480 {
        w.step(DT);
    }
    assert!(w.poses()[0].com[2] > 5.0, "fell through: {:?}", w.poses()[0].com);
}

#[test]
fn a_thrown_field_part_bounces_off_a_mesh_part() {
    // a vase-like dense hull (what a meshed distance field gives) thrown along the floor at a mesh cube
    let mut pts = Vec::new();
    for k in 0..30 {
        let z = k as f64 * 1.2;
        let r = 8.0 + 4.0 * (z * 0.15).sin();
        for a in 0..72 {
            let t = a as f64 * std::f64::consts::TAU / 72.0;
            pts.push([r * t.cos() - 40.0, r * t.sin(), z]);
        }
    }
    let mut vase = cube([0.0; 3]);
    vase.hulls = vec![pts];
    vase.mass.com = [-40.0, 0.0, 18.0];
    let block = cube([0.0, 0.0, 5.0]);
    let asm = Assembly { bodies: vec![vase, block], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(-0.05));
    for _ in 0..240 {
        w.step(DT);
    }
    let block0 = w.poses()[1].com;
    assert!((block0[2] - 5.0).abs() < 1.0, "the cube rests on the floor: {block0:?}");
    let start = w.poses()[0].com;
    assert!(start[2] > 5.0 && start[2] < 40.0, "the field part stands on the floor: {start:?}");
    w.grab(0, start, POINTER_HZ);
    for _ in 0..30 {
        w.drag(start, DT);
        w.step(DT);
    }
    w.throw([800.0, 0.0, 0.0]);
    w.release();
    for _ in 0..240 {
        w.step(DT);
    }
    let after = w.poses();
    assert!(after[1].com[0] > block0[0] + 5.0, "the cube was not hit: {:?} -> {:?}", block0, after[1].com);
    assert!(len(sub(after[0].com, after[1].com)) > 9.0, "the thrown part ended inside the cube: {:?} {:?}", after[0].com, after[1].com);
    assert!(after[1].com[2] > 0.0 && after[0].com[2] > 0.0, "a body fell through the floor");
}
