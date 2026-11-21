use makepad_csg_mech::geom::{len, P3};
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::runner::{on_dynamics, Clock, Command, Lead, Options, PoseRecord, Probe, ProbeSample, Probes, Runner, HZ};
use makepad_csg_mech::sim::{Assembly, BodySpec, Friction, JointKind, JointSpec, Pose, AIR_ANGULAR_DAMPING, AIR_LINEAR_DAMPING};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, OnceLock};

const G: f64 = 9806.65;
const DOWN: P3 = [0.0, 0.0, -G];
const AXES: [P3; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn block(name: &str, c: P3, h: P3) -> BodySpec {
    let corner = |k: usize| std::array::from_fn(|j| c[j] + if k & (1 << j) == 0 { -h[j] } else { h[j] });
    let volume = 8.0 * h[0] * h[1] * h[2];
    let mass = volume * 1.2e-6;
    let i = |a: f64, b: f64| mass * (a * a + b * b) / 3.0;
    let inertia = [[i(h[1], h[2]), 0.0, 0.0], [0.0, i(h[0], h[2]), 0.0], [0.0, 0.0, i(h[0], h[1])]];
    BodySpec {
        name: name.into(),
        mass: MassProps { volume, mass, com: c, inertia },
        hulls: vec![(0..8).map(corner).collect()],
        tubes: Vec::new(),
        balls: Vec::new(),
        rods: Vec::new(),
        field: None,
    }
}

fn single_block(center: P3) -> Assembly {
    Assembly { bodies: vec![block("block", center, [10.0; 3])], up: AXES[2], ..Assembly::default() }
}

fn record(asm: Assembly, gravity: P3, floor: Option<f64>, probes: Vec<Probe>, commands: &[Command], steps: u64) -> Vec<ProbeSample> {
    let allowed = Arc::new(AtomicU64::new(0));
    let limit = allowed.clone();
    let fault = Arc::new(OnceLock::new());
    let clock_fault = fault.clone();
    let probe_count = probes.len();
    let mut samples = Vec::with_capacity(steps as usize * probe_count);
    let mut previous = 0;
    let mut calls = 0;
    let (done, recorded) = sync_channel(1);
    let probes = Probes {
        probes,
        sink: Box::new(move |step, batch| {
            let permitted = limit.load(Ordering::Acquire);
            if step != previous + 1 || step > permitted {
                let _ = clock_fault.set((previous, step, permitted));
            }
            previous = step;
            calls += 1;
            assert_eq!(batch.len(), probe_count, "probe count at step {step}");
            if calls <= steps {
                samples.extend_from_slice(batch);
                if calls == steps {
                    done.try_send(std::mem::take(&mut samples)).expect("send the completed probe record");
                }
            }
        }),
    };
    let (wake, news) = sync_channel(1);
    let options = Options { floor, clock: Clock::Led(allowed.clone()), probes: Some(probes), ..Options::default() };
    let mut runner = Runner::start_with(asm, gravity, options, move || { let _ = wake.try_send(()); });
    runner.send(Command::Step); // the leader still allows zero
    for command in commands {
        runner.send(command.clone());
    }
    loop {
        runner.take_news();
        if runner.answered() {
            break;
        }
        news.recv().expect("runner stopped before answering commands at step 0");
    }
    let startup = runner.latest();
    assert!(startup.error.is_none(), "world construction at step 0: {:?}", startup.error);
    allowed.store(steps, Ordering::Release);
    let samples = recorded.recv().expect("runner stopped before delivering all probe samples");
    drop(runner);
    // Drop detaches the worker. Wait for its sink to close so a late, unpermitted call is checked too.
    assert!(recorded.recv().is_err(), "probe record delivered twice at step {steps}");
    if let Some((previous, step, permitted)) = fault.get() {
        panic!("led clock at step {step}: previous step {previous}, allowed {permitted}");
    }
    samples
}

fn near(step: usize, property: impl std::fmt::Display, actual: f64, expected: f64, tolerance: f64) {
    assert!((actual - expected).abs() <= tolerance, "{property} at step {step}: got {actual}, expected {expected} within {tolerance}");
}

#[test]
fn rest_measures_gravity_in_the_probe_axes() {
    let center = [0.0, 0.0, 10.0];
    let (sin, cos) = 30.0_f64.to_radians().sin_cos();
    for dynamics in [false, true] {
        let mut asm = single_block(center);
        let mut probes = vec![Probe { body: 0, at: center, axes: AXES }];
        if dynamics {
            asm.bodies.push(block("leaf", [20.0, 0.0, 10.0], [10.0; 3]));
            asm.joints.push(JointSpec {
                name: "hinge".into(), a: 0, b: 1, pivot: [10.0, 0.0, 10.0], axis: AXES[1],
                kind: JointKind::Revolute { lower: -std::f64::consts::PI, upper: std::f64::consts::PI },
                friction: Friction::default(), collide: false, brake: None, motor: None,
            });
            assert!(on_dynamics(&asm), "hinged rest uses DynWorld at step 0");
        } else {
            assert!(!on_dynamics(&asm), "single-block rest uses MechWorld at step 0");
            probes.push(Probe { body: 0, at: center, axes: [[cos, 0.0, -sin], AXES[1], [sin, 0.0, cos]] });
        }
        let count = probes.len();
        let engine = if dynamics { "DynWorld" } else { "MechWorld" };
        let samples = record(asm, DOWN, Some(0.0), probes, &[], 480);
        for (i, batch) in samples.chunks_exact(count).enumerate().skip(240) {
            let step = i + 1;
            for (axis, expected) in [0.0, 0.0, G].into_iter().enumerate() {
                near(step, format_args!("{engine} resting specific force axis {axis}"), batch[0].specific_force[axis], expected, 0.005 * G);
            }
            if !dynamics {
                assert!(len(batch[0].rate) < 1e-3, "resting angular rate at step {step}: {:?}", batch[0].rate);
                for (axis, expected) in [-G * sin, 0.0, G * cos].into_iter().enumerate() {
                    near(step, format_args!("rotated resting specific force axis {axis}"), batch[1].specific_force[axis], expected, 0.005 * G);
                }
            }
        }
    }
}

#[test]
fn free_fall_measures_linear_drag() {
    let center = [0.0; 3];
    let probes = vec![Probe { body: 0, at: center, axes: AXES }];
    let samples = record(single_block(center), [0.0; 3], None, probes, &[Command::Gravity(DOWN)], 120);
    for (i, sample) in samples.iter().enumerate() {
        let step = i + 1;
        let t = step as f64 / HZ;
        near(step, "free-fall x force", sample.specific_force[0], 0.0, 1e-4 * G);
        near(step, "free-fall y force", sample.specific_force[1], 0.0, 1e-4 * G);
        near(step, "free-fall drag along z", sample.specific_force[2], G * (1.0 - (-AIR_LINEAR_DAMPING * t).exp()), 1e-3 * G);
    }
}

#[test]
fn spin_measures_angular_rate_and_off_center_acceleration() {
    let r = 8.0;
    let probes = vec![Probe { body: 0, at: [r, 0.0, 0.0], axes: AXES }];
    let pokes = [
        Command::Poke { body: 0, point: [0.0, 10.0, 0.0], dv: [-60.0, 0.0, 0.0] },
        Command::Poke { body: 0, point: [0.0, -10.0, 0.0], dv: [60.0, 0.0, 0.0] },
    ];
    let samples = record(single_block([0.0; 3]), [0.0; 3], None, probes, &pokes, 240);
    let w3 = samples[2].rate[2];
    let dt = 1.0 / HZ;
    for (i, sample) in samples.iter().enumerate().skip(2) {
        let step = i + 1;
        let w = sample.rate[2];
        assert!(w > 0.0, "spin about +z at step {step}: rate {w}");
        near(step, "spin rate x", sample.rate[0], 0.0, 1e-6 * w);
        near(step, "spin rate y", sample.rate[1], 0.0, 1e-6 * w);
        let expected = w3 * (-AIR_ANGULAR_DAMPING * (step - 3) as f64 * dt).exp();
        near(step, "angular drag", w, expected, 1e-4 * expected);
        let radial = w * w * r;
        let turn_squared = (w * dt).powi(2);
        near(step, "centripetal acceleration", sample.specific_force[0], -radial, (turn_squared / 24.0 + 1e-5) * radial);
        near(step, "tangential acceleration", sample.specific_force[1], -AIR_ANGULAR_DAMPING * w * r, (turn_squared / 8.0 + 1e-5) * radial);
        near(step, "axial acceleration", sample.specific_force[2], 0.0, 1e-5 * radial);
    }
}

/// Says through its channel when the probes' sink that owns it is dropped.
struct Dropped(std::sync::mpsc::SyncSender<()>);

impl Drop for Dropped {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[test]
fn a_lead_handed_to_a_running_runner_counts_from_then_records_its_poses_and_drops_its_probes_when_it_goes() {
    let center = [0.0, 0.0, 10.0];
    let runner = Runner::start_with(single_block(center), DOWN, Options { floor: Some(0.0), ..Options::default() }, || {});
    let leader = runner.leader();
    let mut leads = Vec::new();
    let mut records = Vec::new();
    for round in 0..2 {
        // the second lead's record holds four steps of the one block at full resolution
        let record = Arc::new(PoseRecord::new(if round == 0 { 1 << 20 } else { 4 * (size_of::<(u64, Vec<Pose>)>() + size_of::<Pose>()) }));
        let allowed = Arc::new(AtomicU64::new(0));
        let (steps, seen) = sync_channel(16);
        let (gone, dropped) = sync_channel(1);
        let owner = Dropped(gone);
        let probes = Probes {
            probes: vec![Probe { body: 0, at: center, axes: AXES }],
            sink: Box::new(move |step, batch| {
                let _owner = &owner;
                assert_eq!(batch.len(), 1, "one probe's sample at step {step}");
                let _ = steps.try_send(step);
            }),
        };
        leader.set(Some(Lead { allowed: allowed.clone(), probes, poses: Some(record.clone()) }));
        allowed.store(3, Ordering::Release);
        let got: Vec<u64> = (0..3).map(|_| seen.recv().expect("the led runner takes the steps it is allowed")).collect();
        assert_eq!(got, [1, 2, 3], "lead {round}: the steps count from when the runner took it");
        // a step its sink has seen is recorded; the runner, done with what it was allowed, holds no lock
        let found = |step| record.at(step).map(|(found, poses)| (found, poses.len()));
        assert_eq!([found(2), found(0), found(9)], [Some((2, 1)), Some((1, 1)), Some((3, 1))],
            "lead {round}: a step's poses, the first kept for an earlier one, the newest for a later one");
        records.push(record);
        leads.push((seen, dropped, allowed));
    }
    // past its budget the older half thins: the span stays, the newest steps stay whole; then the
    // floor: the steps under it go as the next is recorded. The first lead's record is as it was.
    let (seen, _, allowed) = &leads[1];
    let found = |record: &PoseRecord, step| record.at(step).map(|(found, _)| found);
    allowed.store(7, Ordering::Release);
    let got: Vec<u64> = (0..4).map(|_| seen.recv().expect("the led runner takes the steps it is allowed")).collect();
    assert_eq!(got, [4, 5, 6, 7], "the second lead goes on");
    assert_eq!([1, 4, 5, 7].map(|step| found(&records[1], step)), [Some(1), Some(1), Some(5), Some(7)],
        "thinned: the first step kept, the older ones every other, the newest whole");
    records[1].keep_from(7);
    allowed.store(8, Ordering::Release);
    assert_eq!(seen.recv().ok(), Some(8), "the second lead goes on");
    assert_eq!([found(&records[1], 1), found(&records[1], 8), found(&records[0], 9)], [Some(7), Some(8), Some(3)],
        "the second lead keeps from its floor; the first lead's record has the steps it took");
    // a record handed on to the next lead goes on from the number it is told; what it had from there goes
    let (steps, seen) = sync_channel(16);
    let allowed = Arc::new(AtomicU64::new(0));
    records[1].number_from(8);
    let probes = Probes { probes: vec![Probe { body: 0, at: center, axes: AXES }], sink: Box::new(move |step, _| { let _ = steps.try_send(step); }) };
    leader.set(Some(Lead { allowed: allowed.clone(), probes, poses: Some(records[1].clone()) }));
    allowed.store(2, Ordering::Release);
    let got: Vec<u64> = (0..2).map(|_| seen.recv().expect("the led runner takes the steps it is allowed")).collect();
    assert_eq!(got, [1, 2], "the third lead counts from when the runner took it");
    assert_eq!([7, 8, 9, 10].map(|step| found(&records[1], step)), [Some(7), Some(8), Some(9), Some(9)],
        "the record goes on at step 8 with the third lead's first step");
    // the second lead took over from the first; the wall clock takes over from the second
    leader.set(None);
    for (round, (seen, dropped, _)) in leads.iter().enumerate() {
        dropped.recv().expect("a lead's probes are dropped when it goes");
        assert!(seen.try_iter().next().is_none(), "lead {round}: no step past the three it allowed");
    }
}
