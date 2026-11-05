//! Bodies made of balls, loose in a solid given by its distance.

use makepad_box3d::field::DistanceField;
use makepad_box3d::math_functions::{vec3, Vec3, AABB};
use makepad_csg_mech::geom::{add, sub, P3};
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::runner::{HZ, POINTER_HZ};
use makepad_csg_mech::sim::*;
use std::sync::Arc;

const DT: f64 = 1.0 / HZ;
/// The cup's outside half width, wall thickness and rim height (its floor at z = 0).
const OUT: f32 = 20.0;
const WALL: f32 = 3.0;
const RIM: f32 = 30.0;

/// An open-topped box.
#[derive(Debug)]
struct Cup;

fn sd_box(p: Vec3, half: [f32; 2], z: [f32; 2]) -> f32 {
    let q = [p.x.abs() - half[0], p.y.abs() - half[1], (p.z - (z[0] + z[1]) / 2.0).abs() - (z[1] - z[0]) / 2.0];
    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
    outside + q[0].max(q[1]).max(q[2]).min(0.0)
}

/// A distance and the way it grows.
fn with_gradient(f: impl Fn(Vec3) -> f32, p: Vec3) -> (f32, Vec3) {
    let e = 0.01;
    let d = vec3(f(vec3(p.x + e, p.y, p.z)) - f(vec3(p.x - e, p.y, p.z)), f(vec3(p.x, p.y + e, p.z)) - f(vec3(p.x, p.y - e, p.z)), f(vec3(p.x, p.y, p.z + e)) - f(vec3(p.x, p.y, p.z - e)));
    let l = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-6);
    (f(p), vec3(d.x / l, d.y / l, d.z / l))
}

impl DistanceField for Cup {
    fn distance(&self, p: Vec3) -> (f32, Vec3) {
        with_gradient(|p| sd_box(p, [OUT, OUT], [0.0, RIM]).max(-sd_box(p, [OUT - WALL, OUT - WALL], [WALL, RIM + 10.0])), p)
    }
    fn bounds(&self) -> AABB {
        AABB { lower_bound: vec3(-OUT, -OUT, 0.0), upper_bound: vec3(OUT, OUT, RIM) }
    }
}

fn mass(com: P3, m: f64, i: f64) -> MassProps {
    MassProps { volume: 1.0, mass: m, com, inertia: [[i, 0.0, 0.0], [0.0, i, 0.0], [0.0, 0.0, i]] }
}

fn cup() -> BodySpec {
    BodySpec { name: "cup".into(), mass: mass([0.0, 0.0, 15.0], 20.0, 5000.0), hulls: Vec::new(), tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: Some(SolidField(Arc::new(Cup))) }
}

/// A bar of balls lying along x, `half` mm either side of its middle.
fn bar(at: P3, half: f64) -> BodySpec {
    let balls = (-2..=2).map(|k| ([at[0] + half * k as f64 / 2.0, at[1], at[2]], 1.5)).collect();
    BodySpec { name: "bar".into(), mass: mass(at, 0.2, 40.0), hulls: Vec::new(), tubes: Vec::new(), balls, rods: vec![([at[0] - half, at[1], at[2]], [at[0] + half, at[1], at[2]], 1.2)], field: None }
}

fn world(bars: Vec<BodySpec>) -> MechWorld {
    let mut bodies = vec![cup()];
    bodies.extend(bars);
    let asm = Assembly { bodies, anchors: vec![AnchorSpec { body: 0, pivot: [0.0, 0.0, 15.0], kind: AnchorKind::Fixed, friction: Friction::default() }], up: [0.0, 0.0, 1.0], contact_margin: 1.5, max_speed: 400.0, ..Default::default() };
    MechWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(-0.05))
}

fn run(w: &mut MechWorld, steps: usize) {
    for _ in 0..steps {
        w.step(DT);
    }
}

/// A bar dropped in the cup lands on its floor, inside.
#[test]
fn a_ball_body_lands_inside() {
    let mut w = world(vec![bar([0.0, 0.0, 20.0], 8.0)]);
    run(&mut w, 240);
    let c = w.poses()[1].com;
    assert!((c[2] - (WALL as f64 + 1.5)).abs() < 0.3, "at {c:?}");
}

/// Shaken hard, it stays in; dragged sideways, the wall stops it; dragged up, it comes out.
#[test]
fn the_wall_holds_and_the_way_out_lets_go() {
    let mut w = world(vec![bar([0.0, 0.0, 20.0], 8.0)]);
    for k in 0..720 {
        let t = k as f64 * DT;
        w.move_ground([12.0 * (std::f64::consts::TAU * 2.2 * t).sin(), 0.0, 0.0], [0.0, 0.0, 1.0], 0.0, DT);
        w.step(DT);
        let c = w.poses()[1].com;
        assert!(c[2] < RIM as f64 && (c[0] - w.poses()[0].com[0]).abs() < (OUT - WALL) as f64, "out of the cup at step {k}: {c:?}");
    }
    for _ in 0..240 {
        w.move_ground([0.0; 3], [0.0, 0.0, 1.0], 0.0, DT);
        w.step(DT);
    }
    let c = w.poses()[1].com;
    w.grab(1, c, 8.0);
    for k in 0..480 {
        w.drag([c[0] + 60.0 * (k as f64 / 480.0), c[1], c[2]], DT);
        w.step(DT);
    }
    let held = w.poses()[1].com;
    assert!(held[0] < (OUT + 10.0) as f64, "the wall let it through: {held:?}");
    let now = w.poses()[1].com;
    for k in 0..480 {
        let z = now[2] + 60.0 * (k as f64 / 480.0);
        w.drag([now[0].min(5.0), now[1], z], DT);
        w.step(DT);
    }
    let out = w.poses()[1].com;
    assert!(out[2] > RIM as f64, "straight up leaves the cup: {out:?}");
}

/// Two ball bodies pass through each other's balls but not their rods.
#[test]
fn rods_keep_ball_bodies_apart() {
    let mut w = world(vec![bar([0.0, 0.0, 12.0], 8.0), bar([0.0, 0.0, 20.0], 8.0)]);
    run(&mut w, 480);
    let (low, high) = (w.poses()[1].com, w.poses()[2].com);
    assert!(high[2] - low[2] > 1.5, "they sank into each other: {low:?} {high:?}");
}

/// A closed box's inside half width and its walls' thickness (its floor's underside at z = 0).
const IN: f32 = 10.0;
const SKIN: f32 = 2.0;
/// A ball's middle is out of a solid to within this (mm): where it is moved back out it may stand
/// just outside, and the box is far from the origin.
const THROUGH: f64 = 1e-3;

/// A closed box, `IN` either side of its middle inside.
#[derive(Debug)]
struct Closed;

impl DistanceField for Closed {
    fn distance(&self, p: Vec3) -> (f32, Vec3) {
        let (o, mid) = (IN + SKIN, IN + SKIN);
        with_gradient(|p| sd_box(p, [o, o], [0.0, 2.0 * o]).max(-sd_box(p, [IN, IN], [mid - IN, mid + IN])), p)
    }
    fn bounds(&self) -> AABB {
        let o = IN + SKIN;
        AABB { lower_bound: vec3(-o, -o, 0.0), upper_bound: vec3(o, o, 2.0 * o) }
    }
}

/// A short support tip of balls `r` across, lying along x `at[2]` above the box's floor.
fn tip(at: P3, r: f64) -> BodySpec {
    let z = (SKIN as f64) + r + at[2];
    let balls: Vec<(P3, f64)> = (-2..=2).map(|k| ([at[0] + 1.5 * r * k as f64, at[1], z], r)).collect();
    let m = 0.0124 * r * r * r;
    BodySpec { name: "tip".into(), mass: mass([at[0], at[1], z], m, m * 9.0 * r * r), hulls: Vec::new(), tubes: Vec::new(), balls, rods: Vec::new(), field: None }
}

/// The closed box held by a hand, the tips loose in it, a moment after they were let go.
fn in_a_box(tips: Vec<BodySpec>, gravity: f64) -> (MechWorld, Vec<Vec<(P3, f64)>>) {
    let o = (IN + SKIN) as f64;
    let part = BodySpec { name: "box".into(), mass: mass([0.0, 0.0, o], 10.0, 1500.0), hulls: Vec::new(), tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: Some(SolidField(Arc::new(Closed))) };
    let balls = tips.iter().map(|t| t.balls.clone()).collect();
    let mut bodies = vec![part];
    bodies.extend(tips);
    let asm = Assembly { bodies, anchors: vec![AnchorSpec { body: 0, pivot: [0.0, 0.0, o], kind: AnchorKind::Fixed, friction: Friction::default() }], up: [0.0, 0.0, 1.0], contact_margin: 1.5, max_speed: 2000.0, ..Default::default() };
    let mut w = MechWorld::new(&asm, [0.0, 0.0, -gravity], None);
    run(&mut w, 120);
    (w, balls)
}

/// How far each ball's surface is out of the box's solid (less than zero: into it), and its
/// centre's; the least of each over all balls.
fn clearances(w: &MechWorld, balls: &[Vec<(P3, f64)>]) -> (f64, f64) {
    let poses = w.poses();
    let (box_at, box_rest) = (poses[0], w.rest_com(0));
    let mut least = (f64::MAX, f64::MAX);
    for (i, tip) in balls.iter().enumerate() {
        for &(c, r) in tip {
            let at = poses[i + 1].apply(w.rest_com(i + 1), c);
            let local = add(box_rest, box_at.unrotate(sub(at, box_at.com)));
            let d = Closed.distance(vec3(local[0] as f32, local[1] as f32, local[2] as f32)).0 as f64;
            least = (least.0.min(d - r), least.1.min(d));
        }
    }
    least
}

/// Whether every tip's middle is inside the box.
fn all_inside(w: &MechWorld, n: usize) -> bool {
    let poses = w.poses();
    (1..=n).all(|i| {
        let d = poses[0].unrotate(sub(poses[i].com, poses[0].com));
        d.iter().all(|x| x.abs() < IN as f64)
    })
}

/// Small tips in a closed box flung to and fro, up and down and round as fast as the hand may
/// take it, reversing each time at full speed: the walls stop them, as contacts, every step.
#[test]
fn small_balls_flung_at_the_hands_top_speed_stay_in() {
    let (mut w, balls) = in_a_box(vec![tip([-4.0, 0.0, 0.0], 0.2), tip([4.0, 3.0, 0.0], 0.2), tip([0.0, -5.0, 0.0], 0.5)], GRAVITY);
    let up = [0.0, 0.0, 1.0];
    let mut hand = ([0.0; 3], up, 0.0);
    let (mut fastest, mut was) = (0.0f64, w.poses()[0].com);
    for step in 0..960 {
        if step % 3 == 0 {
            let t = step as f64 * DT;
            let to = |hz: f64, phase: f64| if (std::f64::consts::TAU * hz * t + phase).sin() >= 0.0 { 1.0 } else { -1.0 };
            hand = ([80.0 * to(4.0, 0.0), 60.0 * to(3.0, 1.0), 80.0 * to(5.0, 2.0)], [0.3, 1.0, 0.5], 0.8 * to(2.0, 0.5));
        }
        w.carry_ground(hand, ([0.0; 3], up, 0.0), 1.0 / 90.0, DT);
        w.step(DT);
        let c = clearances(&w, &balls).1;
        assert!(c > -THROUGH, "a ball's middle went {} mm into a wall at step {step}", -c);
        let now = w.poses()[0].com;
        fastest = fastest.max(makepad_csg_mech::geom::len(sub(now, was)) / DT);
        was = now;
    }
    assert!(fastest > 1900.0, "the box went no faster than {fastest} mm/s");
    assert!(all_inside(&w, balls.len()), "a tip left the box");
}

/// Tips tossed up, about to come down, as the hand jerks the box up at full speed from still:
/// its floor comes up at them faster than a step, and stops them at its face.
#[test]
fn a_jerk_into_a_floor_sinks_nothing() {
    let (mut w, balls) = in_a_box(vec![tip([-4.0, -4.0, 1.0], 0.5), tip([4.0, 3.0, 3.0], 0.5), tip([0.0, 5.0, 6.0], 1.0)], 0.0);
    let up = [0.0, 0.0, 1.0];
    let mut surface = f64::MAX;
    for _ in 0..12 {
        w.carry_ground(([0.0, 0.0, 200.0], up, 0.0), ([0.0; 3], up, 0.0), 1.0 / 90.0, DT);
        w.step(DT);
        surface = surface.min(clearances(&w, &balls).0);
    }
    assert!(all_inside(&w, balls.len()), "a tip went through the floor");
    assert!(surface > -0.2, "a tip went {} mm into the floor", -surface);
}

/// A tip pulled through the side wall by a hand far stronger than the contact: pressed past the
/// wall's middle, it is still pushed back the way it came, never out of the far side.
#[test]
fn a_tip_pulled_through_a_wall_stays_on_its_side() {
    let (mut w, balls) = in_a_box(vec![tip([0.0, 0.0, 0.0], 0.2)], GRAVITY);
    let at = w.poses()[1].com;
    w.grab(1, at, POINTER_HZ);
    for step in 0..480 {
        w.drag([at[0] + 60.0 * (step as f64 / 120.0).min(1.0), at[1], at[2]], DT);
        w.step(DT);
        let c = clearances(&w, &balls).1;
        assert!(c > -THROUGH, "a ball's middle went {} mm into the wall at step {step}", -c);
    }
    assert!(all_inside(&w, 1), "the tip went through the wall: {:?}", w.poses()[1].com);
}
