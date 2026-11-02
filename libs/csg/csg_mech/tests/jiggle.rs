//! A deterministic mouse-flick workload, using Fab's weave scene and worker policy.
//! Run through devhub with --native -- --nocapture; no sleeps or wall-clock-driven input.

use makepad_box3d::physics_world::{world_get_awake_body_count, world_get_counters, world_get_profile};
use makepad_box3d::types::{Counters, Profile as StepProfile};
use makepad_csg_mech::geom::{add, len, sub, P3};
use makepad_csg_mech::joint::Profile;
use makepad_csg_mech::runner::{HZ, POINTER_HZ};
use makepad_csg_mech::sim::MechWorld;
use makepad_csg_mech::weave::{self, Frame, Link, SquareLink, WeaveProfile, WeaveScene};
use std::time::Instant;

const DT: f64 = 1.0 / HZ;
/// The runner steps at 240 Hz in real time: the typical step must fit in its share of a second, and no
/// step may cost more than three (the runner falls behind for that long and catches up; it never stalls
/// the view). A step split over threads that waits on one the OS put aside costs tens of milliseconds.
const MEDIAN_BUDGET_MS: f64 = 1000.0 / HZ;
const MAX_BUDGET_MS: f64 = 3.0 * 1000.0 / HZ;

struct Sample {
    step: usize,
    ms: f64,
    diagnostics_ms: f64,
    counters: Counters,
    profile: StepProfile,
    awake: i32,
    speed: f64,
    deepest: f64,
    target_delta: f64,
}

// Geometry is inspected after timing the step. Reuse the point buffers; neighbours() uses
// the same spatial broad phase as weave_bench's integrity check, including undesigned pairs.
fn sample(w: &mut MechWorld, start: &[Link], posed: &mut [Link], up: P3, step: usize, target_delta: f64) -> Sample {
    let timer = Instant::now();
    w.step(DT);
    let ms = timer.elapsed().as_secs_f64() * 1000.0;
    let diagnostics = Instant::now();
    let counters = world_get_counters(&w.world);
    let profile = world_get_profile(&w.world);
    let awake = world_get_awake_body_count(&w.world);
    let speed = w.max_speed();
    let poses = w.poses();
    for (i, (source, dest)) in start.iter().zip(posed.iter_mut()).enumerate() {
        for (from, to) in source.pts.iter().zip(dest.pts.iter_mut()) {
            *to = poses[i].apply(w.rest_com(i), *from);
        }
    }
    // Zero means no penetration; negative is deepest overlap in mm, as in weave.rs.
    let deepest = weave::neighbours(posed, 0.1).into_iter().fold(0.0_f64, |gap, (a, b)| {
        gap.min(weave::link_gap(&posed[a as usize], &posed[b as usize], up).0)
    });
    Sample { step, ms, diagnostics_ms: diagnostics.elapsed().as_secs_f64() * 1000.0, counters, profile, awake, speed, deepest, target_delta }
}

// Nearest-rank percentiles: p99 selects ceil(0.99 * N), with no interpolation.
fn percentile(sorted: &[f64], percent: usize) -> f64 {
    sorted[(percent * sorted.len()).div_ceil(100).max(1) - 1]
}

fn stats(values: impl Iterator<Item = f64>) -> [f64; 3] {
    let mut values: Vec<_> = values.collect();
    values.sort_by(f64::total_cmp);
    [values[0], percentile(&values, 50), *values.last().unwrap()]
}

fn correlation(samples: &[Sample], metric: impl Fn(&Sample) -> f64) -> f64 {
    let n = samples.len() as f64;
    let mean_ms = samples.iter().map(|s| s.ms).sum::<f64>() / n;
    let mean_value = samples.iter().map(&metric).sum::<f64>() / n;
    let (mut covariance, mut time_variance, mut value_variance) = (0.0, 0.0, 0.0);
    for s in samples {
        let x = s.ms - mean_ms;
        let y = metric(s) - mean_value;
        covariance += x * y;
        time_variance += x * x;
        value_variance += y * y;
    }
    let scale = (time_variance * value_variance).sqrt();
    if scale > 0.0 { covariance / scale } else { 0.0 }
}

fn summary(phase: &str, samples: &[Sample]) -> (f64, f64) {
    let mut times: Vec<_> = samples.iter().map(|s| s.ms).collect();
    times.sort_by(f64::total_cmp);
    let median = percentile(&times, 50);
    let max = *times.last().unwrap();
    eprintln!("jiggle {phase}: {} steps ({:.3} sim s), step ms median={median:.3} p95={:.3} p99={:.3} max={max:.3}; budgets median<{MEDIAN_BUDGET_MS:.3} max<{MAX_BUDGET_MS:.3}",
        samples.len(), samples.len() as f64 * DT, percentile(&times, 95), percentile(&times, 99));
    eprintln!("  min/median/max (all counters are post-step):");
    macro_rules! counters {
        ($($field:ident),+ $(,)?) => {
            eprintln!("    {}", [$( {
                let [min, median, max] = stats(samples.iter().map(|s| s.counters.$field as f64));
                format!("{}={min:.0}/{median:.0}/{max:.0}", stringify!($field))
            } ),+].join(" "));
        };
    }
    counters!(body_count, shape_count, contact_count, awake_contact_count, joint_count, island_count, task_count);
    counters!(static_tree_height, tree_height, sat_call_count, sat_cache_hit_count, recycled_contact_count,
        feature_recycled_contact_count, feature_separated_skip_count);
    counters!(stack_used, arena_capacity, byte_count, distance_iterations, push_back_iterations, root_iterations);
    // Elementwise statistics, not the color vector of the median-time step. The last color
    // is the overflow bucket, whose constraints are solved single-threaded.
    for (name, arrays) in [
        ("color_counts", samples.iter().map(|s| s.counters.color_counts.as_slice()).collect::<Vec<_>>()),
        ("manifold_counts", samples.iter().map(|s| s.counters.manifold_counts.as_slice()).collect::<Vec<_>>()),
    ] {
        let by_bucket: Vec<_> = (0..arrays[0].len()).map(|i| stats(arrays.iter().map(|a| a[i] as f64))).collect();
        for (rank, name_rank) in ["min", "median", "max"].iter().enumerate() {
            eprintln!("    {name}.{name_rank}={:?}", by_bucket.iter().map(|s| s[rank] as i32).collect::<Vec<_>>());
        }
    }
    let metrics: Vec<_> = [
        ("awake_bodies", (|s: &Sample| s.awake as f64) as fn(&Sample) -> f64),
        ("active_colors", |s| s.counters.color_counts.iter().filter(|c| **c > 0).count() as f64),
        ("speed_mm_s", |s| s.speed),
        ("deepest_overlap_mm", |s| s.deepest),
        ("target_delta_mm", |s| s.target_delta),
        ("diagnostics_ms", |s| s.diagnostics_ms),
    ].into_iter().map(|(name, metric)| {
        let [min, median, max] = stats(samples.iter().map(metric));
        format!("{name}={min:.3}/{median:.3}/{max:.3}")
    }).collect();
    eprintln!("    {}", metrics.join(" "));
    macro_rules! profiles {
        ($($field:ident),+ $(,)?) => {
            eprintln!("    profile ms: {}", [$( {
                let [min, median, max] = stats(samples.iter().map(|s| s.profile.$field as f64));
                format!("{}={min:.3}/{median:.3}/{max:.3}", stringify!($field))
            } ),+].join(" "));
        };
    }
    profiles!(step, pairs, collide, solve, solver_setup, constraints);
    profiles!(prepare_constraints, integrate_velocities, warm_start, solve_impulses, integrate_positions, relax_impulses);
    profiles!(apply_restitution, store_impulses, split_islands, transforms, sensor_hits, joint_events);
    profiles!(hit_events, refit, bullets, sleep_islands, sensors);
    eprintln!("  Pearson r with step ms (association, not causation; constant metrics report 0):");
    let correlations: Vec<_> = [
        ("contacts", (|s: &Sample| s.counters.contact_count as f64) as fn(&Sample) -> f64),
        ("awake_contacts", |s| s.counters.awake_contact_count as f64),
        ("islands", |s| s.counters.island_count as f64),
        ("awake_bodies", |s| s.awake as f64),
        ("active_colors", |s| s.counters.color_counts.iter().filter(|c| **c > 0).count() as f64),
        ("overflow", |s| *s.counters.color_counts.last().unwrap() as f64),
        ("speed", |s| s.speed),
        ("overlap_depth", |s| -s.deepest),
        ("pairs_ms", |s| s.profile.pairs as f64),
        ("collide_ms", |s| s.profile.collide as f64),
        ("solve_ms", |s| s.profile.solve as f64),
    ].into_iter().map(|(name, metric)| format!("{name}={:.3}", correlation(samples, metric))).collect();
    eprintln!("    {}", correlations.join(" "));
    let mut slowest: Vec<_> = samples.iter().collect();
    slowest.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    eprintln!("  {phase}: 3 slowest steps (1-based phase step and phase sim time):");
    for s in slowest.iter().take(3) {
        eprintln!("    step={} t={:.4}s ms={:.3} awake={} speed={:.1}mm/s overlap={:.3}mm target_delta={:.1}mm diagnostics={:.3}ms",
            s.step, s.step as f64 * DT, s.ms, s.awake, s.speed, s.deepest, s.target_delta, s.diagnostics_ms);
        eprintln!("      {:?}", s.counters);
        eprintln!("      profile ms world={:.3} pairs={:.3} collide={:.3} solve={:.3} setup={:.3} constraints={:.3} impulses={:.3} relax={:.3} refit={:.3} bullets={:.3}",
            s.profile.step, s.profile.pairs, s.profile.collide, s.profile.solve, s.profile.solver_setup, s.profile.constraints,
            s.profile.solve_impulses, s.profile.relax_impulses, s.profile.refit, s.profile.bullets);
    }
    (median, max)
}

fn target(origin: P3, step: usize) -> P3 {
    let t = step as f64 * DT;
    let mut offset = [4.0, 7.0, 10.0].map(|hz| 80.0 * (std::f64::consts::TAU * hz * t).sin());
    // Five one-frame 80 mm flicks, in alternating directions/axes, independent of wall time.
    // Their returns are abrupt too. Even the 10 Hz sine moves at most 21 mm per step.
    for (at, jump) in [
        (0.50, [80.0, 0.0, 0.0]),
        (1.25, [0.0, -80.0, 0.0]),
        (2.00, [0.0, 0.0, 80.0]),
        (2.75, [-80.0, 0.0, 0.0]),
        (3.50, [0.0, 80.0, 0.0]),
    ] {
        if step == (at * HZ).round() as usize {
            offset = add(offset, jump);
        }
    }
    add(origin, offset)
}

#[test]
fn hard_jiggle_keeps_realtime_headroom() {
    let profile = Profile::default();
    let wp = WeaveProfile::new(&profile, None);
    let frame = Frame::new([0.0; 3], profile.up);
    let woven = weave::mail(&wp, SquareLink::new(&wp, None), frame, [-30.0; 2], [30.0; 2], 0.0, &|_| -1.0);
    let mut setup = weave::scene(&woven, 1.24e-3, WeaveScene::Drop);
    setup.asm.workers = makepad_csg_mech::sim::solver_workers(setup.asm.bodies.len());
    let mut w = MechWorld::new(&setup.asm, setup.gravity, setup.floor);
    // Use the collision assembly's starting points, as weave::simulate does.
    let start: Vec<Link> = woven.links.iter().zip(&setup.asm.bodies).map(|(l, b)| Link {
        pts: b.tubes[0].0.clone(), r: l.r, cell: l.cell, slot: l.slot,
    }).collect();
    let mut posed = start.clone();
    // Pick the link nearest the lower-left design corner, then grab its settled centre.
    let corner = woven.links.iter().enumerate().min_by(|(_, a), (_, b)| {
        let distance = |l: &Link| {
            let c = frame.to_local(l.center());
            (c[0] + 30.0).powi(2) + (c[1] + 30.0).powi(2)
        };
        distance(a).total_cmp(&distance(b))
    }).expect("mail must contain links").0;
    eprintln!("jiggle setup: 60x60mm links={} designed_pairs={} workers={} HZ={HZ} POINTER_HZ={POINTER_HZ} corner={} substeps={} contact_hertz={} margin={} speed_cap={}",
        woven.links.len(), woven.pairs.len(), setup.asm.workers, corner, setup.asm.substeps,
        setup.asm.contact_hertz, setup.asm.contact_margin, setup.asm.max_speed);
    let mut settle = Vec::with_capacity(HZ as usize);
    for step in 1..=HZ as usize {
        settle.push(sample(&mut w, &start, &mut posed, frame.up, step, 0.0));
    }
    let origin = w.poses()[corner].com;
    w.grab(corner, origin, POINTER_HZ);
    let mut previous = origin;
    let mut jiggle = Vec::with_capacity((4.0 * HZ) as usize);
    for step in 1..=(4.0 * HZ) as usize {
        let at = target(origin, step);
        w.drag(at, DT);
        jiggle.push(sample(&mut w, &start, &mut posed, frame.up, step, len(sub(at, previous))));
        previous = at;
    }
    w.release();
    // Print both complete phases before asserting, so a slow settle cannot hide jiggle data.
    let settle_budget = summary("settle", &settle);
    let jiggle_budget = summary("drag", &jiggle);
    assert!(
        [settle_budget, jiggle_budget].iter().all(|&(median, max)| median < MEDIAN_BUDGET_MS && max < MAX_BUDGET_MS),
        "240 Hz needs a median step under {MEDIAN_BUDGET_MS:.3} ms and none over {MAX_BUDGET_MS:.3} ms; settle (median,max)={settle_budget:?}, drag={jiggle_budget:?}"
    );
    // the work stays bounded: no contact blowup, and no link driven through another
    let contacts = jiggle.iter().map(|s| s.counters.contact_count).max().unwrap_or(0) as usize;
    assert!(contacts < 8 * woven.links.len(), "{contacts} contacts for {} links", woven.links.len());
    let deepest = jiggle.iter().map(|s| s.deepest).fold(0.0, f64::min);
    assert!(deepest > -2.0 * woven.links[0].r, "links pressed {deepest:.2} mm into each other (bar {:.2} mm)", 2.0 * woven.links[0].r);
}
