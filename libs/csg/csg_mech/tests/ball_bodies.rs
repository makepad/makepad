//! Bodies made of balls, loose in a solid given by its distance.

use makepad_box3d::field::DistanceField;
use makepad_box3d::math_functions::{vec3, Vec3, AABB};
use makepad_csg_mech::geom::P3;
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::runner::HZ;
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

impl DistanceField for Cup {
    fn distance(&self, p: Vec3) -> (f32, Vec3) {
        let f = |p: Vec3| sd_box(p, [OUT, OUT], [0.0, RIM]).max(-sd_box(p, [OUT - WALL, OUT - WALL], [WALL, RIM + 10.0]));
        let e = 0.01;
        let d = vec3(f(vec3(p.x + e, p.y, p.z)) - f(vec3(p.x - e, p.y, p.z)), f(vec3(p.x, p.y + e, p.z)) - f(vec3(p.x, p.y - e, p.z)), f(vec3(p.x, p.y, p.z + e)) - f(vec3(p.x, p.y, p.z - e)));
        let l = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-6);
        (f(p), vec3(d.x / l, d.y / l, d.z / l))
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
