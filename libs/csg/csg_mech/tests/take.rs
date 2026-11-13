//! A hand takes held parts where the sim has them. A view shows the sim a step or two late: what it
//! saw at the take (a part on its way up as the scene lifts it, say) is not where the sim has it,
//! and a carry that went to the view's pose pulled the part back there, a jerk the hand never made.
//! After a take, each carry moves the parts as the hand has moved since its first.

use makepad_csg_mech::geom::{add, cross, len, norm, scale, sub, P3};
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::runner::{Command, Runner, Snapshot, HZ};
use makepad_csg_mech::sim::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A box of half size `h` about `c`: the hull and mass of a held part.
fn block(name: &str, c: P3, h: P3) -> BodySpec {
    let corner = |k: usize| [c[0] + if k & 1 == 0 { -h[0] } else { h[0] }, c[1] + if k & 2 == 0 { -h[1] } else { h[1] }, c[2] + if k & 4 == 0 { -h[2] } else { h[2] }];
    let volume = 8.0 * h[0] * h[1] * h[2];
    let mass = volume * 1.2e-6;
    let i = |a: f64, b: f64| mass * (a * a + b * b) / 3.0;
    let inertia = [[i(h[1], h[2]), 0.0, 0.0], [0.0, i(h[0], h[2]), 0.0], [0.0, 0.0, i(h[0], h[1])]];
    BodySpec { name: name.into(), mass: MassProps { volume, mass, com: c, inertia }, hulls: vec![(0..8).map(corner).collect()], tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: None }
}

/// A turn (a unit quaternion: w, x, y, z) and then a move.
#[derive(Clone, Copy, Debug)]
struct Motion {
    q: [f64; 4],
    t: P3,
}

impl Motion {
    /// A turn about `axis` through `through`, and then a move.
    fn turn_about(axis: P3, angle: f64, through: P3, then_move: P3) -> Motion {
        let a = norm(axis);
        let (s, c) = (0.5 * angle).sin_cos();
        let turn = Motion { q: [c, a[0] * s, a[1] * s, a[2] * s], t: [0.0; 3] };
        Motion { t: add(sub(through, turn.apply(through)), then_move), ..turn }
    }

    /// A part's pose as a motion from where it was built (`rest`: its centre of mass as built).
    fn of_pose(p: &Pose, rest: P3) -> Motion {
        let turn = Motion { q: [p.rot[3], p.rot[0], p.rot[1], p.rot[2]], t: [0.0; 3] };
        Motion { t: sub(p.com, turn.apply(rest)), ..turn }
    }

    fn apply(&self, p: P3) -> P3 {
        let (w, v) = (self.q[0], [self.q[1], self.q[2], self.q[3]]);
        let u = scale(cross(v, p), 2.0);
        add(add(add(p, scale(u, w)), cross(v, u)), self.t)
    }

    /// `next` done after `self`.
    fn then(&self, next: &Motion) -> Motion {
        let (a, b) = (next.q, self.q);
        let q = [
            a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
            a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
            a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
            a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
        ];
        Motion { q, t: next.apply(self.t) }
    }

    /// The hand at this pose, as a headset sends it.
    fn carry(&self) -> Command {
        let v = [self.q[1], self.q[2], self.q[3]];
        let s = len(v);
        let (axis, angle) = if s == 0.0 { ([0.0, 0.0, 1.0], 0.0) } else { (scale(v, 1.0 / s), 2.0 * s.atan2(self.q[0])) };
        Command::Carry { at: self.t, axis, angle }
    }
}

/// Where the parts' corners are in a snapshot.
fn corners(s: &Snapshot, asm: &Assembly) -> Vec<P3> {
    asm.bodies.iter().enumerate().flat_map(|(i, b)| b.hulls[0].iter().map(move |&c| s.poses[i].apply(s.rest[i], c))).collect()
}

fn furthest(a: &[P3], b: &[P3]) -> f64 {
    a.iter().zip(b).map(|(p, q)| len(sub(*p, *q))).fold(0.0, f64::max)
}

struct Sim {
    r: Runner,
    sent: u64,
}

impl Sim {
    fn send(&mut self, c: Command) {
        self.r.send(c);
        self.sent += 1;
    }

    /// The newest snapshot once the sim has taken up every command sent.
    fn answered(&mut self) -> Arc<Snapshot> {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let s = self.r.latest();
            if s.commands >= self.sent {
                return s;
            }
            assert!(Instant::now() < until, "the sim took up {} of {} commands in a minute", s.commands, self.sent);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The paused sim at `time`, a step at a time (steps sent together make one).
    fn step_to(&mut self, time: f64) -> Arc<Snapshot> {
        let mut s = self.answered();
        while s.time < time {
            self.send(Command::Step);
            s = self.answered();
        }
        s
    }

    /// The hand still at `hand` (sent again each frame, as a headset does) for `seconds` of sim
    /// time: the parts' corners at the end, and the furthest any went from `from` on the way.
    fn hold(&mut self, asm: &Assembly, hand: &Motion, seconds: f64, from: &[P3]) -> (Vec<P3>, f64) {
        self.send(hand.carry());
        let first = self.answered();
        let mut moved = 0.0f64;
        loop {
            let s = self.r.latest();
            let at = corners(&s, asm);
            moved = moved.max(furthest(&at, from));
            if s.time >= first.time + seconds {
                return (at, moved);
            }
            self.send(hand.carry());
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Paused, a hand takes the parts from a view that has them at `shown`, and the sim plays.
    fn take(&mut self, shown: &Motion) {
        self.send(Command::Take);
        self.send(shown.carry());
        self.send(Command::Play);
    }
}

/// Held parts near the origin and about 250 mm out, as the scene lifts them, on both engines (held
/// fixed: box3d; held by a hand: the dynamics engine). Taken mid-lift from a view a few steps
/// behind, they stay where the sim has them while the hand stays still. Taken again from a view that
/// has them turned and moved, they stay too (the take's offset is a whole rigid motion, not a move
/// alone). After each take a turn of the hand about its grip turns them with it, in the world.
#[test]
fn a_take_starts_where_the_sim_has_the_parts() {
    for kind in [AnchorKind::Fixed, AnchorKind::Held] {
        let bodies = vec![block("near", [6.0, -4.0, 8.0], [10.0, 6.0, 8.0]), block("far", [250.0, 30.0, 6.0], [8.0, 12.0, 6.0])];
        let anchors = bodies.iter().enumerate().map(|(body, b)| AnchorSpec { body, pivot: b.mass.com, kind: kind.clone(), friction: Friction::default() }).collect();
        let asm = Assembly { bodies, anchors, up: [0.0, 0.0, 1.0], sleep: SLEEP_SPEED, workers: 1, substeps: JOINT_SUBSTEPS, lift: 20.0, ..Default::default() };
        let mut sim = Sim { r: Runner::start(asm.clone(), [0.0, 0.0, -GRAVITY], || {}), sent: 0 };
        sim.send(Command::Speed(4.0));
        // the scene lifts the parts from 0.6 s at 60 mm/s; the view is a twentieth of a second behind
        let seen = sim.step_to(0.72);
        let rest = seen.rest[0];
        let shown = Motion::of_pose(&seen.poses[0], rest);
        let there = sim.step_to(0.72 + 12.0 / HZ);
        let (from, behind) = (corners(&there, &asm), furthest(&corners(&there, &asm), &corners(&seen, &asm)));
        assert!(behind > 2.0, "{kind:?}: the view must be behind the lift at the take (it saw the parts {behind:.2} mm from where the sim had them)");
        sim.take(&shown);
        let (still, moved) = sim.hold(&asm, &shown, 1.0, &from);
        assert!(moved < 0.01, "{kind:?}: taken mid-lift, a still hand moved the parts {moved:.4} mm (the view was {behind:.2} mm behind)");

        // the hand turns 30 degrees about its grip on the near part and moves 15 mm
        let grip = Motion::of_pose(&there.poses[0], rest).apply(add(rest, [4.0, 2.0, 5.0]));
        let turn = Motion::turn_about([1.0, 1.0, 1.0], 30f64.to_radians(), grip, [15.0, 0.0, 0.0]);
        let (turned, _) = sim.hold(&asm, &shown.then(&turn), 0.25, &still);
        let off = furthest(&turned, &still.iter().map(|&p| turn.apply(p)).collect::<Vec<_>>());
        assert!(off < 0.01, "{kind:?}: the parts went {off:.4} mm from where the hand's turn about its grip takes them");

        // taken again, paused, from a view that has them turned 25 degrees and moved 7 mm from where they are
        sim.send(Command::Pause);
        let now = sim.answered();
        let wrong = Motion::of_pose(&now.poses[0], rest).then(&Motion::turn_about([0.3, -1.0, 0.4], 25f64.to_radians(), [0.0; 3], [0.0, 7.0, 0.0]));
        sim.take(&wrong);
        let (still, moved) = sim.hold(&asm, &wrong, 1.0, &corners(&now, &asm));
        assert!(moved < 0.01, "{kind:?}: taken from a view off by a turn, a still hand moved the parts {moved:.4} mm");
        let turn = Motion::turn_about([0.0, 1.0, 0.2], -40f64.to_radians(), still[0], [0.0, -10.0, 5.0]);
        let (turned, _) = sim.hold(&asm, &wrong.then(&turn), 0.25, &still);
        let off = furthest(&turned, &still.iter().map(|&p| turn.apply(p)).collect::<Vec<_>>());
        assert!(off < 0.01, "{kind:?}: after a take off by a turn, the parts went {off:.4} mm from where the hand's turn takes them");
        println!("{kind:?}: the view {behind:.2} mm behind at the take; the hand's turns followed to {off:.5} mm");
    }
}
