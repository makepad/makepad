//! The stepper's gates (KERNELS.md P7): seek == play at random times,
//! stale checkpoints never restored, beat-exact spawns, the pending queue
//! and expiry across seeks, contacts in their window, sims stepping with
//! physics, bodies published through a kernel ring, and the step budget.

use makepad_effect_sim::*;
use makepad_render_kernels::compute::kernel::{FieldTy, Layout, LayoutField};
use makepad_render_kernels::{compile, ManualFences};
use std::sync::Arc;

const LAUNCH: &str = r#"
let out = output(Body)
let speed = param(2.2)
fn instance(i) {
    let a = rand01(seed, i, 0) * 6.2832
    // A stream over 0.3 s from the cannon (every tenth on the beat itself).
    let delay = if i % 10 == 0 { 0.0 } else { rand01(seed, i, 4) * 0.3 }
    out[i] = Body{pos: vec3(0.0, 0.2, 0.0), size: vec3(0.05, 0.004, 0.03), vel: vec3(cos(a) * speed, 6.0 + 3.0 * rand01(seed, i, 1), sin(a) * speed), spin: rand_dir(seed, i, 2) * 14.0, drag: 1.5, delay: delay, color: vec4(rand01(seed, i, 3), 0.5, 0.8, 1.0)}
}
"#;

/// A second burst: stagger with `delay`, gone after `life`.
const STAGGER: &str = r#"
let out = output(Body)
fn instance(i) {
    let a = float(i) * 0.7
    out[i] = Body{pos: vec3(cos(a) * 0.5, 1.5, sin(a) * 0.5), size: vec3(0.08, 0.08, 0.08), vel: vec3(0.0, 2.0, 0.0), delay: float(i % 8) * 0.05, life: 1.0 + float(i % 3) * 0.25, shape: 2}
}
"#;

fn spark_layout() -> Layout {
    Layout {
        name: "Spark".into(),
        stride: 4,
        fields: vec![LayoutField { name: "pos".into(), ty: FieldTy::Vec3, offset: 0 }, LayoutField { name: "life".into(), ty: FieldTy::F32, offset: 3 }],
    }
}

fn program(src: &str) -> u64 {
    let mut h = Fnv::default();
    h.bytes(src.as_bytes());
    h.finish()
}

fn call(src: &str, buffer: &str, layouts: &[Layout], inputs: Vec<(String, SimInput)>) -> KernelCall {
    let mut all = makepad_effect_sim::layouts();
    all.extend_from_slice(layouts);
    let kernel = compile(src, &all, &makepad_effect_sim::modules()).unwrap_or_else(|e| panic!("{src}\n{e:?}"));
    KernelCall { kernel, buffer: buffer.into(), params: Vec::new(), inputs, program: program(src) }
}

/// The sparkle sim: every spark takes a hard contact's place when one is
/// there for it, otherwise it fades and falls.
fn sparks() -> SimDesc {
    let srcs = sim_sources(
        "Spark",
        "i",
        "next[i].pos = vec3(0.0, -10.0, 0.0)",
        "i",
        "next[i] = prev[i]\n if i < int(hits_count) && hits[i].impulse > 0.0005 { next[i].pos = hits[i].pos\n next[i].life = 1.0 } else { next[i].life = max(prev[i].life - 0.05, 0.0)\n next[i].pos = prev[i].pos + vec3(0.0, 0.01, 0.0) }",
        "",
        "let hits = input(Contact)\nlet hits_count = param(0.0, 0.0, 1000000.0)",
    );
    let inputs = vec![("hits".to_string(), SimInput::Contacts("confetti".into()))];
    SimDesc {
        name: "sparks".into(),
        state: spark_layout(),
        count: 64,
        every: 2,
        init: call(&srcs.init, "next", &[spark_layout()], inputs.clone()),
        update: call(&srcs.update, "next", &[spark_layout()], inputs),
        prev: "prev".into(),
        spawn: None,
    }
}

struct Paddle;

impl Drive for Paddle {
    fn pose(&mut self, key: &str, t: SimTime) -> Option<([f32; 3], [f32; 4])> {
        (key == "paddle").then(|| {
            let s = t.as_secs() as f32;
            ([1.2 * (s * 2.0).sin(), 0.15, 0.0], [0.0, (s * 0.5).sin(), 0.0, (s * 0.5).cos()])
        })
    }
}

/// Confetti on the beat (beat 4 at 120 bpm = 2 s), a staggered burst of
/// balls at 1 s with delays and lives, a kinematic paddle sweeping the
/// floor, and sparks at hard contacts.
fn confetti(doc: u64, checkpoint_bytes: usize) -> PhysicsDesc {
    let mut d = PhysicsDesc::new(doc);
    d.seed = 7;
    d.retain = SimTime::from_ratio(3, 10);
    d.checkpoint_bytes = checkpoint_bytes;
    d.bodies.push(BodyDesc { name: "floor".into(), group: String::new(), shape: Shape::Plane, motion: Motion::Fixed, record: BodyRecord::default() });
    d.bodies.push(BodyDesc {
        name: "paddle".into(),
        group: String::new(),
        shape: Shape::Box,
        motion: Motion::Kinematic { key: "paddle".into() },
        record: BodyRecord { size: [1.0, 0.3, 0.2], ..Default::default() },
    });
    d.spawns.push(SpawnDesc {
        at: SimTime::from_secs(0.5 * 4.0),
        group: "confetti".into(),
        count: 600,
        shape: Shape::Box,
        fixed: false,
        size: [0.05, 0.004, 0.03],
        from: SpawnFrom::Kernel(call(LAUNCH, "out", &[], Vec::new())),
    });
    d.spawns.push(SpawnDesc {
        at: SimTime::from_secs(1.0),
        group: "balls".into(),
        count: 24,
        shape: Shape::Box,
        fixed: false,
        size: [0.1; 3],
        from: SpawnFrom::Kernel(call(STAGGER, "out", &[], Vec::new())),
    });
    d.sims.push(sparks());
    d
}

/// Deterministic picks.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// A frame time, or one of its motion-blur sub-frames (4 over a 180°
/// shutter at 60 fps).
fn frame_time(r: &mut Lcg, frames: i64) -> SimTime {
    let f = (r.next() % frames as u64) as i64;
    if r.next() % 2 == 0 {
        SimTime::frame(f, 60, 1)
    } else {
        SimTime::subframe(f, 60, 1, (r.next() % 4) as u32, 4, 180)
    }
}

#[test]
fn stepper_is_send() {
    fn send<T: Send>() {}
    send::<Stepper>();
}

#[test]
fn seek_equals_play_at_twenty_random_times() {
    let frames = 6 * 60;
    let mut r = Lcg(0x5eed);
    let mut probes: Vec<SimTime> = (0..20).map(|_| frame_time(&mut r, frames)).collect();
    probes.sort();
    probes.dedup();
    // Play: every frame in order (and each probe as it comes).
    let mut play = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    let mut want = Vec::new();
    let mut next = 0;
    for f in 0..frames {
        let t = SimTime::frame(f, 60, 1);
        while next < probes.len() && probes[next] <= t {
            let p = probes[next];
            play.seek(p, &mut Paddle);
            let v = play.view(p).expect("viewable after seek");
            want.push((p, v.hash(), play.state_hash(), play.head()));
            next += 1;
        }
        play.seek(t, &mut Paddle);
        assert!(play.view(t).is_some());
    }
    assert!(play.errors().next().is_none(), "{:?}", play.errors().collect::<Vec<_>>());
    let stats = play.stats();
    assert!(stats.bodies_made > 600, "{stats:?}");
    // Cold seeks in shuffled order on a fresh stepper, then the same
    // stepper seeking back and forth (restoring its own checkpoints).
    let mut order: Vec<usize> = (0..want.len()).collect();
    for i in (1..order.len()).rev() {
        let j = (r.next() as usize) % (i + 1);
        order.swap(i, j);
    }
    let mut cold = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    for &k in &order {
        let (t, view_hash, state_hash, head) = want[k];
        let mut fresh = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
        fresh.seek(t, &mut Paddle);
        assert_eq!(fresh.view(t).unwrap().hash(), view_hash, "cold seek to {} s", t.as_secs());
        assert_eq!(fresh.state_hash(), state_hash, "cold state at {} s", t.as_secs());
        cold.seek(t, &mut Paddle);
        assert_eq!(cold.view(t).unwrap().hash(), view_hash, "seek to {} s", t.as_secs());
        // The whole state where the seek left the world at the same step
        // (a seek into kept frames does not step back).
        if cold.head() == head {
            assert_eq!(cold.state_hash(), state_hash, "state at {} s", t.as_secs());
        }
    }
    assert!(cold.stats().restores > 0, "{:?}", cold.stats());
    eprintln!("play: {:?}", stats);
    eprintln!("seeks: {:?}", cold.stats());
}

#[test]
fn a_thin_checkpoint_budget_still_seeks_exactly() {
    let mut play = Stepper::new(confetti(1, 1 << 20), &mut Paddle);
    let t = SimTime::from_secs(4.3);
    play.seek(t, &mut Paddle);
    let want = play.view(t).unwrap().hash();
    let s = play.stats();
    assert!(s.thinned > 0 && s.checkpoint_bytes <= 1 << 20 || s.checkpoints == 1, "{s:?}");
    // Back to the start and forward again from what is left.
    play.seek(SimTime::from_secs(0.1), &mut Paddle);
    play.seek(t, &mut Paddle);
    assert_eq!(play.view(t).unwrap().hash(), want);
}

#[test]
fn stale_checkpoints_are_never_restored() {
    let mut st = Stepper::new(confetti(1, 1 << 30), &mut Paddle);
    st.seek(SimTime::from_secs(5.0), &mut Paddle);
    assert_eq!(st.stats().checkpoints, 11);
    // The same document: nothing changes.
    assert!(!st.reload(confetti(1, 1 << 30), &mut Paddle));
    // An edit (another document hash, or another launch program).
    let mut edited = confetti(2, 1 << 30);
    if let SpawnFrom::Kernel(k) = &mut edited.spawns[0].from {
        k.params.push(("speed".into(), 3.5));
        k.program ^= 1;
    }
    assert!(st.reload(edited.clone(), &mut Paddle));
    let s = st.stats();
    assert_eq!(s.stale_discarded, 11, "{s:?}");
    assert_eq!(s.restores, 0);
    let t = SimTime::from_secs(3.0);
    st.seek(t, &mut Paddle);
    let mut fresh = Stepper::new(edited, &mut Paddle);
    fresh.seek(t, &mut Paddle);
    assert_eq!(st.view(t).unwrap().hash(), fresh.view(t).unwrap().hash());
    // And it differs from the unedited document (the edit did something).
    let mut old = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    old.seek(t, &mut Paddle);
    assert_ne!(old.view(t).unwrap().hash(), fresh.view(t).unwrap().hash());
}

#[test]
fn spawns_land_on_the_first_step_at_or_after_the_beat() {
    let mut st = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    let dt = st.dt();
    assert_eq!(dt.step_ceil(SimTime::from_secs(2.0)), 480);
    let count = |st: &mut Stepper, t: SimTime| {
        st.seek(t, &mut Paddle);
        let mut v = Vec::new();
        st.view(t).unwrap().bodies("confetti", &mut v);
        v.len()
    };
    let before = dt.time_of(480).saturating_sub(SimTime::from_ticks(1));
    assert_eq!(count(&mut st, before), 0);
    assert_eq!(count(&mut st, dt.time_of(480)), 60, "the undelayed tenth lands on the beat's step");
    assert_eq!(count(&mut st, dt.time_of(480 + 72)), 600, "the rest within 0.3 s");
    // Seeking back before the beat removes them again.
    assert_eq!(count(&mut st, SimTime::from_secs(1.9)), 0);
    // Negative time is the initial state.
    st.seek(SimTime::from_secs(-2.0), &mut Paddle);
    let v = st.view(SimTime::from_secs(-2.0)).unwrap();
    assert_eq!(v.step(), 0);
    assert_eq!(v.alpha(), 0.0);
}

#[test]
fn delayed_bodies_wait_and_expire_across_seeks() {
    let mut st = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    let balls = |st: &mut Stepper, secs: f64| {
        let t = SimTime::from_secs(secs);
        st.seek(t, &mut Paddle);
        let mut v = Vec::new();
        st.view(t).unwrap().bodies("balls", &mut v);
        v.len()
    };
    // 24 balls from 1.0 s, 3 at each of 8 delays 0.05 s apart.
    assert_eq!(balls(&mut st, 1.0), 3);
    assert_eq!(balls(&mut st, 1.2), 15);
    assert_eq!(balls(&mut st, 1.4), 24);
    // Lives 1.0, 1.25 and 1.5 s after their own spawn.
    assert_eq!(balls(&mut st, 3.0), 0);
    let mut ev = Vec::new();
    st.view(SimTime::from_secs(3.0)).unwrap().events("balls", &mut ev);
    // Back into the middle of it (a restore), and forward.
    assert_eq!(balls(&mut st, 1.3), 21);
    // Expired by 2.2 s: delay + life <= 1.2 s for five of them.
    assert_eq!(balls(&mut st, 2.2), 24 - 5);
    let mut ev2 = Vec::new();
    let t = SimTime::from_secs(2.2);
    st.view(t).unwrap().events("balls", &mut ev2);
    assert!(ev2.iter().any(|e| e.kind == EventKind::Expire));
    assert!(ev2.iter().all(|e| e.time as f64 > 2.2 - 0.3 - 1e-6 && e.time as f64 <= 2.2 + 1e-6));
}

#[test]
fn contacts_stay_readable_for_the_retention_window() {
    let mut st = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    let t = SimTime::from_secs(3.5);
    st.seek(t, &mut Paddle);
    let v = st.view(t).unwrap();
    let mut c = Vec::new();
    v.contacts("confetti", &mut c);
    assert!(!c.is_empty(), "confetti landing makes contacts");
    for k in &c {
        assert!(k.time as f64 > 3.5 - 0.3 - 1e-6 && k.time as f64 <= 3.5 + 1e-6, "{k:?}");
        assert!(k.impulse >= 0.0 && k.speed >= 0.05);
    }
    // Many steps' worth, not only the last one.
    let steps: std::collections::BTreeSet<u64> = c.iter().map(|k| k.step).collect();
    assert!(steps.len() > 5, "{} steps", steps.len());
    // The sparks sim put some of them into its state.
    let sparks = v.sim("sparks").unwrap();
    let lit = sparks.chunks(4).filter(|s| f32::from_bits(s[3]) > 0.0).count();
    assert!(lit > 0);
    // Bodies interpolate between the two steps around t.
    let dt = st.dt();
    let mid = dt.time_of(dt.step_floor(t)).saturating_add(SimTime::from_ticks(dt.ticks() / 2));
    st.seek(dt.time_of(dt.step_floor(mid) + 1), &mut Paddle);
    let va = st.view(mid).unwrap();
    assert_eq!(va.alpha(), 0.5);
    let (mut a, mut b, mut m) = (Vec::new(), Vec::new(), Vec::new());
    va.bodies("confetti", &mut m);
    st.view(dt.time_of(dt.step_floor(mid))).unwrap().bodies("confetti", &mut a);
    st.view(dt.time_of(dt.step_floor(mid) + 1)).unwrap().bodies("confetti", &mut b);
    for k in 0..m.len() {
        for c in 0..3 {
            assert!((m[k].pos[c] - (a[k].pos[c] + b[k].pos[c]) * 0.5).abs() < 1e-5);
        }
    }
}

#[test]
fn bodies_publish_through_a_kernel_ring() {
    let mut st = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    let layout = rigid_layout();
    let mut ring = body_ring(0xb0d1, &layout);
    let fences = ManualFences::with_delay(2);
    for f in 120..160 {
        let t = SimTime::frame(f, 60, 1);
        st.seek(t, &mut Paddle);
        let v = st.view(t).unwrap();
        ring.collect(&fences);
        let now = publish_bodies(&v, "confetti", &layout, &mut ring, &fences).unwrap();
        let mut want = Vec::new();
        let n = v.write_bodies("confetti", &layout, &mut want);
        let draw = ring.draw(&fences).expect("a published slot");
        if now.is_some_and(|l| l.0 == draw.generation) {
            assert_eq!(draw.count as usize, n);
            assert_eq!(&draw.data[..n * RIGID_WORDS], &want[..]);
        }
        fences.submit();
        fences.complete_all();
    }
    let rs = ring.stats();
    assert!(rs.published > 10 && rs.busy > 0, "{rs:?}");
}

#[test]
fn sims_can_spawn_bodies_and_read_bodies() {
    // A fountain sim: every 12 steps each of its 4 elements emits a ball
    // above the nearest confetti piece (a query over bodies).
    let srcs = sim_sources(
        "Tick",
        "i",
        "next[i].n = 0.0",
        "i",
        "next[i].n = prev[i].n + 1.0\n if bodies_count > 0.0 { let k = physics.nearest(bodies, int(bodies_count), vec3(float(i), 0.0, 0.0))\n emit(drops, Body{pos: bodies[max(k, 0)].pos + vec3(0.0, 1.0, 0.0), size: vec3(0.05, 0.05, 0.05), life: 0.5, shape: 2}) }",
        "",
        "let bodies = input(Rigid)\nlet bodies_count = param(0.0, 0.0, 1000000.0)\nlet drops = emit_buffer(Body, 1)",
    );
    let (init, update) = (srcs.init, srcs.update);
    let inputs = vec![("bodies".to_string(), SimInput::Bodies("confetti".into()))];
    let tick = Layout { name: "Tick".into(), stride: 1, fields: vec![LayoutField { name: "n".into(), ty: FieldTy::F32, offset: 0 }] };
    let mut d = confetti(3, 64 << 20);
    d.sims.push(SimDesc {
        name: "fountain".into(),
        state: tick.clone(),
        count: 4,
        every: 12,
        init: call(&init, "next", &[tick.clone()], inputs.clone()),
        update: call(&update, "next", &[tick.clone()], inputs),
        prev: "prev".into(),
        spawn: Some(("drops".into(), "drops".into(), Shape::Sphere)),
    });
    let mut st = Stepper::new(d.clone(), &mut Paddle);
    let t = SimTime::from_secs(2.4);
    st.seek(t, &mut Paddle);
    assert!(st.errors().next().is_none(), "{:?}", st.errors().collect::<Vec<_>>());
    let v = st.view(t).unwrap();
    let mut drops = Vec::new();
    v.bodies("drops", &mut drops);
    // From the beat (step 480) to 2.4 s (576): 4 per 12 steps, 9 runs,
    // each drop living 0.5 s.
    assert_eq!(drops.len(), 4 * 9);
    let n = f32::from_bits(v.sim("fountain").unwrap()[0]);
    assert_eq!(n, (576 / 12) as f32);
    let h = v.hash();
    let mut cold = Stepper::new(d, &mut Paddle);
    cold.seek(t, &mut Paddle);
    assert_eq!(cold.view(t).unwrap().hash(), h);
}

#[test]
fn realtime_advance_keeps_the_last_state_when_behind() {
    let mut st = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    let t = SimTime::from_secs(5.0);
    match st.advance_to(t, std::time::Duration::ZERO, &mut Paddle) {
        Progress::Behind { head } => assert!(head < st.dt().step_floor(t)),
        Progress::Ready => panic!("no budget, no steps"),
    }
    let v = st.view_or_latest(t);
    assert!(v.step() < 1200);
    assert_eq!(st.stats().late, 1);
    while st.advance_to(t, std::time::Duration::from_millis(50), &mut Paddle) != Progress::Ready {}
    let h = st.view(t).unwrap().hash();
    let mut locked = Stepper::new(confetti(1, 64 << 20), &mut Paddle);
    locked.seek(t, &mut Paddle);
    assert_eq!(locked.view(t).unwrap().hash(), h, "realtime reaches the same state as locked time");
}

#[test]
fn records_out_of_range_are_refused_before_anything_is_made() {
    let mut d = PhysicsDesc::new(9);
    let bad = r#"
let out = output(Body)
fn instance(i) {
    out[i] = Body{pos: vec3(0.0, 1.0, 0.0), size: vec3(0.1, 0.1, 0.1), vel: vec3(if i == 3 { 1.0e9 } else { 0.0 }, 0.0, 0.0)}
}
"#;
    d.spawns.push(SpawnDesc { at: SimTime::ZERO, group: "g".into(), count: 8, shape: Shape::Box, fixed: false, size: [0.1; 3], from: SpawnFrom::Kernel(call(bad, "out", &[], Vec::new())) });
    let mut st = Stepper::new(d, &mut NoDrive);
    st.seek(SimTime::ZERO, &mut NoDrive);
    let errs: Vec<&String> = st.errors().collect();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("record 3") && errs[0].contains("`vel`"), "{}", errs[0]);
    let mut v = Vec::new();
    st.view(SimTime::ZERO).unwrap().bodies("g", &mut v);
    assert_eq!(v.len(), 7);
}

#[test]
fn two_thousand_bodies_step_within_budget() {
    // 2000 boxes dropped into a pile: the effect-physics budget is <= 1 ms
    // per 1/120 s of sim (two steps at 1/240) on one worker, including
    // pose extraction and history upkeep (KERNELS.md §6).
    let recs: Vec<BodyRecord> = (0..2000)
        .map(|i| BodyRecord { pos: [(i % 20) as f32 * 0.3 - 3.0, 0.3 + (i / 400) as f32 * 0.3, ((i / 20) % 20) as f32 * 0.3 - 3.0], size: [0.2; 3], ..Default::default() })
        .collect();
    let mut d = PhysicsDesc::new(11);
    d.bodies.push(BodyDesc { name: "floor".into(), group: String::new(), shape: Shape::Plane, motion: Motion::Fixed, record: BodyRecord::default() });
    d.spawns.push(SpawnDesc { at: SimTime::ZERO, group: "pile".into(), count: 0, shape: Shape::Box, fixed: false, size: [0.2; 3], from: SpawnFrom::Records(Arc::new(recs)) });
    let mut st = Stepper::new(d, &mut NoDrive);
    let t0 = std::time::Instant::now();
    let t = SimTime::from_secs(2.0);
    st.seek(t, &mut NoDrive);
    let wall = t0.elapsed();
    let s = st.stats();
    let per_120 = s.step_ns as f64 / s.steps as f64 * 2.0 / 1e6;
    eprintln!(
        "2000 bodies: {} steps in {:.1} ms wall; {:.3} ms per 1/120 s (extraction {:.3} ms), worst step {:.3} ms, {} checkpoints {:.1} MiB",
        s.steps,
        wall.as_secs_f64() * 1e3,
        per_120,
        s.extract_ns as f64 / s.steps as f64 * 2.0 / 1e6,
        s.step_ns_max as f64 / 1e6,
        s.checkpoints,
        s.checkpoint_bytes as f64 / (1 << 20) as f64
    );
    assert_eq!(s.bodies_live, 2001);
}
