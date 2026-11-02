//! A part picked up, carried and held still settles: its kinetic energy and spin die out,
//! contacts lose energy, and a part put down comes to rest and sleeps.

use makepad_box3d::body::{body_get_angular_velocity, body_get_linear_velocity, body_is_awake};
use makepad_csg_mech::geom::{add, len, sub, P3};
use makepad_csg_mech::runner::{HZ, POINTER_HZ};
use makepad_csg_mech::sim::{Assembly, BodySpec, MechWorld, GRAVITY, PLA_ON_PLATE, SLEEP_SPEED};

const DT: f64 = 1.0 / HZ;
/// PLA, g / mm^3.
const PLA: f64 = 0.00124;

/// A box part (half extents) at `at`.
fn block(at: P3, half: P3) -> BodySpec {
    let vertices: Vec<P3> = (0..8).map(|i| std::array::from_fn(|k| at[k] + if i & (1 << k) == 0 { -half[k] } else { half[k] })).collect();
    let triangles = [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
    makepad_csg_mech::mesh_body("block", &vertices, &triangles, PLA, 1.0, &[])
}

/// Kinetic energy (g mm^2 / s^2: translation plus spin about the centre of mass) and spin (rad/s) of body `i`.
fn motion(w: &MechWorld, spec: &BodySpec, i: usize) -> (f64, f64) {
    let b = w.bodies[i];
    let v = body_get_linear_velocity(&w.world, b);
    let o = body_get_angular_velocity(&w.world, b);
    let o = [o.x as f64, o.y as f64, o.z as f64];
    let ob = w.poses()[i].unrotate(o);
    let io: P3 = std::array::from_fn(|r| (0..3).map(|c| spec.mass.inertia[r][c] * ob[c]).sum());
    let rot = 0.5 * (0..3).map(|k| ob[k] * io[k]).sum::<f64>();
    let lin = 0.5 * spec.mass.mass * (v.x * v.x + v.y * v.y + v.z * v.z) as f64;
    (lin + rot, len(o))
}

/// A 40 x 16 x 6 mm bar resting on the bed, picked up by a point near one end of its top face,
/// lifted, carried sideways and held still. Returns per 0.05 s of the hold: (time, kinetic energy, spin,
/// held point's distance from the cursor).
fn pick_up_and_hold() -> (Vec<(f64, f64, f64, f64)>, f64) {
    let spec = block([0.0, 0.0, 3.0], [20.0, 8.0, 3.0]);
    let asm = Assembly { bodies: vec![spec.clone()], up: [0.0, 0.0, 1.0], ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(-0.05));
    for _ in 0..120 {
        w.step(DT);
    }
    let grip: P3 = [15.0, 4.0, 6.0];
    // the grip in the body's model frame
    let model_grip = {
        let p = w.poses()[0];
        add(w.rest_com(0), p.unrotate(sub(grip, p.com)))
    };
    w.grab(0, grip, POINTER_HZ);
    // lift 40 mm in 0.25 s, carry 80 mm sideways in 0.25 s (about what a quick mouse move does)
    let mut cursor = grip;
    for i in 1..=60 {
        cursor = add(grip, [0.0, 0.0, 40.0 * i as f64 / 60.0]);
        w.drag(cursor, DT);
        w.step(DT);
    }
    let lifted = cursor;
    let mut peak: f64 = 0.0;
    for i in 1..=60 {
        cursor = add(lifted, [80.0 * i as f64 / 60.0, 0.0, 0.0]);
        w.drag(cursor, DT);
        w.step(DT);
        peak = peak.max(motion(&w, &spec, 0).0);
    }
    let mut trace = Vec::new();
    for i in 1..=(HZ as usize * 3) {
        w.drag(cursor, DT);
        w.step(DT);
        if i % 12 == 0 {
            let (ke, spin) = motion(&w, &spec, 0);
            let p = w.poses()[0];
            let at = p.apply(w.rest_com(0), model_grip);
            trace.push((i as f64 * DT, ke, spin, len(sub(at, cursor))));
        }
    }
    (trace, peak)
}

#[test]
fn a_held_part_stops_swinging() {
    let (trace, peak) = pick_up_and_hold();
    for (t, ke, spin, off) in &trace {
        eprintln!("hold t={t:.2} s  KE={ke:10.1} g mm^2/s^2  spin={spin:6.3} rad/s  grip off cursor={off:6.3} mm");
    }
    eprintln!("peak KE while carried: {peak:.1}");
    let at = |s: f64| trace.iter().find(|r| r.0 >= s - 1e-9).copied().unwrap();
    let (_, ke_05, spin_05, _) = at(0.5);
    let (_, ke_1, spin_1, _) = at(1.0);
    // settled within half a second: under 1 % of the carried energy, then the swing is gone
    assert!(ke_05 < peak * 0.01, "still moving 0.5 s after the hand stopped: KE {ke_05:.1} of {peak:.1}");
    assert!(spin_05 < 0.5, "still swinging 0.5 s after the hand stopped: {spin_05:.3} rad/s");
    assert!(ke_1 < peak * 1e-4 && spin_1 < 0.05, "not at rest 1 s after the hand stopped: KE {ke_1:.3}, spin {spin_1:.3}");
    // the energy only falls once the hand holds still (no pumping, no jitter)
    let tail: Vec<f64> = trace.iter().filter(|r| r.0 >= 1.0).map(|r| r.1).collect();
    assert!(tail.iter().all(|ke| *ke < peak * 1e-4), "energy came back: {tail:?}");
}

#[test]
fn a_dropped_part_bounces_lower_and_comes_to_rest_asleep() {
    let spec = block([0.0, 0.0, 53.0], [10.0, 10.0, 3.0]);
    let asm = Assembly { bodies: vec![spec.clone()], up: [0.0, 0.0, 1.0], sleep: SLEEP_SPEED, ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(0.0));
    // falls 50 mm: hits at about 990 mm/s
    let (mut down, mut up, mut was_falling) = (0.0f64, 0.0f64, true);
    let mut rebound_top = 0.0f64;
    for _ in 0..(HZ as usize * 3) {
        w.step(DT);
        let v = body_get_linear_velocity(&w.world, w.bodies[0]).z as f64;
        if was_falling {
            down = down.min(v);
            if v > 0.0 {
                was_falling = false;
            }
        } else if up == 0.0 && v <= 0.0 {
            up = 1.0;
            rebound_top = w.poses()[0].com[2] - 3.0;
        }
    }
    eprintln!("hit at {:.0} mm/s, rebound to {rebound_top:.2} mm", -down);
    assert!(rebound_top > 0.5 && rebound_top < 50.0 * 0.5, "a bounce under 1 loses height: rebound {rebound_top:.2} of 50 mm");
    assert!(!body_is_awake(&w.world, w.bodies[0]), "a part at rest sleeps");
    assert!((w.poses()[0].com[2] - 3.0).abs() < 0.05, "rests on the plate: {:?}", w.poses()[0].com);
}

/// A part on a plate tilted by `deg`: how far it slid in 2 s (mm).
fn slide_on_tilt(deg: f64) -> f64 {
    let spec = block([0.0, 0.0, 3.0], [10.0, 10.0, 3.0]);
    let asm = Assembly { bodies: vec![spec], up: [0.0, 0.0, 1.0], sleep: SLEEP_SPEED, ..Default::default() };
    let t = deg.to_radians();
    // tilting gravity tilts the plate under it (the floor stays across gravity, so the part is placed on it
    // as a plate across z with gravity leaning along x)
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(0.0));
    for _ in 0..60 {
        w.step(DT);
    }
    makepad_box3d::physics_world::world_set_gravity(&mut w.world, makepad_box3d::math_functions::vec3((GRAVITY * t.sin()) as f32, 0.0, (-GRAVITY * t.cos()) as f32));
    makepad_box3d::body::body_set_awake(&mut w.world, w.bodies[0], true);
    let start = w.poses()[0].com;
    for _ in 0..(HZ as usize * 2) {
        w.step(DT);
    }
    len(sub(w.poses()[0].com, start))
}

#[test]
fn a_part_sticks_on_a_gentle_slope_and_slides_on_a_steep_one() {
    // PLA on the plate: sticks below atan(stick), and once moving slides on the lower sliding friction
    let below = (PLA_ON_PLATE.stick.atan().to_degrees() - 3.0).max(1.0);
    let above = PLA_ON_PLATE.stick.atan().to_degrees() + 3.0;
    let (held, slid) = (slide_on_tilt(below), slide_on_tilt(above));
    eprintln!("{below:.1} deg: {held:.3} mm, {above:.1} deg: {slid:.1} mm");
    assert!(held < 0.2, "static friction must hold a part on a {below:.1} degree slope: slid {held:.3} mm");
    // sliding: a = g (sin - mu_slide cos) over 2 s
    let t = above.to_radians();
    let expect = 0.5 * GRAVITY * (t.sin() - PLA_ON_PLATE.slide * t.cos()) * 4.0;
    assert!(slid > 0.7 * expect && slid < 1.3 * expect, "sliding friction: slid {slid:.1} mm, Coulomb says {expect:.1}");
}

#[test]
fn a_resting_stack_stays_still_and_sleeps() {
    let bodies: Vec<BodySpec> = (0..4).map(|k| block([0.0, 0.0, 3.0 + 6.02 * k as f64], [10.0, 10.0, 3.0])).collect();
    let asm = Assembly { bodies, up: [0.0, 0.0, 1.0], sleep: SLEEP_SPEED, ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(0.0));
    for _ in 0..(HZ as usize) {
        w.step(DT);
    }
    let settled = w.poses();
    for _ in 0..(HZ as usize * 2) {
        w.step(DT);
    }
    let fin = w.poses();
    for i in 0..4 {
        let drift = len(sub(fin[i].com, settled[i].com));
        assert!(drift < 0.01, "block {i} crept {drift:.4} mm while resting");
        assert!(!body_is_awake(&w.world, w.bodies[i]), "block {i} is still awake");
        assert!((fin[i].com[2] - (3.0 + 6.0 * i as f64)).abs() < 0.1, "block {i} sank or jumped: {:?}", fin[i].com);
    }
}
