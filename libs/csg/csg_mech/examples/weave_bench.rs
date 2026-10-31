//! Weave bench: generate chains and chainmail, check them, and run them as free links in box3d (hang from a
//! held row, drape over a ball, drop on the bed) at growing link counts. Prints link count vs step cost and
//! whether every designed pair stays interlocked (linking number) and how deep links press into each other.
//!
//! cargo run --release -p makepad-csg-mech --example weave_bench [-- sizes...]

use makepad_box3d::body::create_body;
use makepad_box3d::math_functions::vec3;
use makepad_box3d::physics_world::{world_get_awake_body_count, world_get_counters, world_get_profile};
use makepad_box3d::shape::create_sphere_shape;
use makepad_box3d::types::{default_body_def, default_shape_def, Sphere};
use makepad_csg_mech::geom::*;
use makepad_csg_mech::joint::Profile;
use makepad_csg_mech::sim::{AnchorKind, AnchorSpec, Assembly, Friction, MechWorld};
use makepad_csg_mech::weave::*;
use std::time::Instant;

const PLA: f64 = 1.24e-3;

fn heart(p: [f64; 2], size: f64) -> f64 {
    // implicit heart (x^2 + y^2 - 1)^3 - x^2 y^3 <= 0, scaled; a sign test is all mail() needs
    let x = p[0] / size;
    let y = p[1] / size + 0.25;
    let a = x * x + y * y - 1.0;
    a * a * a - x * x * y * y * y
}

fn report_check(name: &str, w: &Weave, wp: &WeaveProfile) {
    let t = Instant::now();
    let c = check(w, wp);
    println!(
        "{name}: {} links {} pairs (dropped {}) | gap z {:.3} xy {:.3} | close {} unlinked {} tangled {} | bridge {:.2} leg {:.1} deg | {:?} | check {:.1} ms",
        c.links,
        c.pairs,
        w.dropped,
        c.min_gap_z.0,
        c.min_gap_xy.0,
        c.close.len(),
        c.unlinked.len(),
        c.tangled.len(),
        c.bridge,
        c.leg_deg,
        c.verdict,
        t.elapsed().as_secs_f64() * 1e3
    );
    for (i, j, g, k) in c.close.iter().take(3) {
        println!("   close {i}-{j} gap {g:.3} {k:?}");
    }
}

struct Live {
    links: Vec<Link>,
    rest: Vec<P3>,
}

fn posed(live: &Live, mw: &MechWorld) -> Vec<Link> {
    let poses = mw.poses();
    live.links
        .iter()
        .zip(poses.iter())
        .zip(live.rest.iter())
        .map(|((l, p), r)| Link { pts: l.pts.iter().map(|q| p.apply(*r, *q)).collect(), r: l.r, cell: l.cell, slot: l.slot })
        .collect()
}

fn integrity(w: &Weave, links: &[Link]) -> (usize, f64) {
    let unlinked = w.pairs.iter().filter(|(a, b)| linking_number(&links[*a as usize].pts, &links[*b as usize].pts) == 0).count();
    let mut deepest = f64::MAX;
    for (i, j) in neighbours(links, 0.1) {
        let (g, _, _) = link_gap(&links[i as usize], &links[j as usize], w.frame.up);
        deepest = deepest.min(g);
    }
    (unlinked, deepest)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Scene {
    Hang,
    Drape,
    Drop,
}

fn run(scene: Scene, w: &Weave, seconds: f64, workers: u32, margin: f64, cap: bool, hertz: f64, substeps: u32) {
    let base = assembly(w, PLA, 240.0);
    let bodies = base.bodies;
    let rest: Vec<P3> = bodies.iter().map(|b| b.mass.com).collect();
    let up = w.frame.up;
    let mut anchors = Vec::new();
    let gravity;
    let mut floor = None;
    match scene {
        Scene::Hang => {
            // hold the top row (largest local y), hang down along -y
            let ys: Vec<f64> = w.links.iter().map(|l| w.frame.to_local(l.center())[1]).collect();
            let top = ys.iter().cloned().fold(f64::MIN, f64::max);
            for (i, y) in ys.iter().enumerate() {
                if *y > top - 1e-6 {
                    anchors.push(AnchorSpec { body: i, pivot: rest[i], kind: AnchorKind::Fixed, friction: Friction::default() });
                }
            }
            gravity = scale(w.frame.y, -9810.0);
        }
        Scene::Drape | Scene::Drop => {
            gravity = scale(up, -9810.0);
            floor = Some(if scene == Scene::Drop { -20.0 } else { -60.0 });
        }
    }
    let asm = Assembly { bodies, joints: Vec::new(), anchors, up, sleep: 2.0, workers, contact_margin: margin, max_speed: if cap { base.max_speed } else { 0.0 }, contact_hertz: hertz, substeps, ball: None };
    let mut mw = MechWorld::new(&asm, gravity, floor);
    if scene == Scene::Drape {
        // a static ball under the middle of the cloth
        let c = w.links.iter().fold([0.0; 3], |s, l| add(s, l.center()));
        let c = scale(c, 1.0 / w.links.len() as f64);
        let r = 25.0;
        let at = madd(c, up, -r - 2.0);
        let mut bd = default_body_def();
        bd.position = makepad_box3d::math_functions::to_pos(vec3(at[0] as f32, at[1] as f32, at[2] as f32));
        let id = create_body(&mut mw.world, &bd);
        let mut sd = default_shape_def();
        sd.base_material.friction = 0.4;
        create_sphere_shape(&mut mw.world, id, &sd, &Sphere { center: vec3(0.0, 0.0, 0.0), radius: r as f32 });
    }
    let live = Live { links: w.links.clone(), rest };
    let dt = 1.0 / 240.0;
    let steps = (seconds / dt) as usize;
    let mut worst_ms: f64 = 0.0;
    let mut sum_ms = 0.0;
    let mut last_sec_ms = 0.0;
    let mut min_unlinked_seen = 0usize;
    let mut deepest_seen = f64::MAX;
    let t_all = Instant::now();
    let mut prof = [0.0f64; 4];
    for k in 0..steps {
        let t = Instant::now();
        mw.step(dt);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let pr = world_get_profile(&mw.world);
        prof[0] += pr.pairs as f64;
        prof[1] += pr.collide as f64;
        prof[2] += pr.solve as f64;
        prof[3] += world_get_counters(&mw.world).contact_count as f64;
        worst_ms = worst_ms.max(ms);
        sum_ms += ms;
        if k + 240 >= steps {
            last_sec_ms += ms;
        }
        if k % 60 == 59 {
            let (u, d) = integrity(w, &posed(&live, &mw));
            min_unlinked_seen = min_unlinked_seen.max(u);
            deepest_seen = deepest_seen.min(d);
        }
    }
    let (u, d) = integrity(w, &posed(&live, &mw));
    let awake = world_get_awake_body_count(&mw.world);
    let n = w.links.len();
    println!(
        "  {scene:?} {n} links {:.1} s, workers {workers} margin {margin} cap {cap} hz {hertz} sub {substeps}: step avg {:.2} ms (max {:.1}, last 1 s avg {:.2}) => {:.0} sim-fps | awake {awake} | unlinked now {u} worst {} | deepest {:.3} mm (worst {:.3}) | wall {:.1} s",
        seconds,
        sum_ms / steps as f64,
        worst_ms,
        last_sec_ms / 240.0,
        1000.0 / (sum_ms / steps as f64),
        min_unlinked_seen,
        d,
        deepest_seen,
        t_all.elapsed().as_secs_f64()
    );
    let k = steps as f64;
    let cc = world_get_counters(&mw.world).color_counts;
    let (mut cs, mut ms, mut ps) = (0usize, 0usize, 0usize);
    for c in &mw.world.contacts {
        let m = c.manifold_count() as usize;
        if m > 0 {
            cs += 1;
            ms += m;
            ps += c.manifolds.iter().map(|x| x.point_count as usize).sum::<usize>();
        }
    }
    println!("    touching contacts {cs}, manifolds {ms} ({:.1} per contact), points {ps}", ms as f64 / cs.max(1) as f64);
    println!("    colors {:?} (last = overflow)", cc);
    println!("    per step: pairs {:.2} collide {:.2} solve {:.2} ms, contacts {:.0}", prof[0] / k, prof[1] / k, prof[2] / k, prof[3] / k);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a.as_str()) == Some("scale") {
        let prof = Profile::default();
        let wp = WeaveProfile::new(&prof, None);
        let link = SquareLink::new(&wp, None);
        let frame = Frame::new([0.0; 3], prof.up);
        for side in args[1..].iter().filter_map(|a| a.parse::<f64>().ok()) {
            let h = side * 0.5;
            let w = mail(&wp, link, frame, [-h, -h], [h, h], 0.0, &|_| -1.0);
            for (hz, sub) in [(240.0, 8)] {
                for scene in [Scene::Hang, Scene::Drape, Scene::Drop] {
                    for workers in [8] {
                        run(scene, &w, 3.0, workers, 1.6, true, hz, sub);
                    }
                }
            }
        }
        return;
    }
    let sizes: Vec<f64> = if args.is_empty() { vec![40.0, 80.0, 120.0, 180.0] } else { args.iter().filter_map(|a| a.parse().ok()).collect() };
    let prof = Profile::default();
    let wp = WeaveProfile::new(&prof, None);
    let link = SquareLink::new(&wp, None);
    let frame = Frame::new([0.0; 3], prof.up);
    println!(
        "profile: r {:.2} c_xy {:.2} c_z {:.2} | link a {:.3} s {:.3} lo {:.2} hi {:.2} bridge {:.2} thick {:.2} chain pitch {:.2}",
        wp.r,
        wp.c_xy,
        wp.c_z,
        link.a,
        link.s,
        link.lo,
        link.hi,
        link.bridge(),
        link.thickness(),
        link.chain_pitch()
    );
    // a pair alone
    let two = chain(&wp, link, frame, &[[0.0, 0.0], [link.chain_pitch(), 0.0]], false);
    println!("pair linking number {}", linking_number(&two.links[0].pts, &two.links[1].pts));
    report_check("pair", &two, &wp);
    let straight = chain(&wp, link, frame, &[[0.0, 0.0], [200.0, 0.0]], false);
    report_check("chain straight 200 mm", &straight, &wp);
    for radius in [15.0, 20.0, 30.0, 50.0] {
        let circle: Vec<[f64; 2]> = (0..256).map(|k| {
            let t = k as f64 / 256.0 * std::f64::consts::TAU;
            [radius * t.cos(), radius * t.sin()]
        }).collect();
        let ring = chain(&wp, link, frame, &circle, true);
        report_check(&format!("bracelet r {radius}"), &ring, &wp);
    }
    let wave: Vec<[f64; 2]> = (0..400).map(|k| {
        let x = k as f64 * 0.5;
        [x, 15.0 * (x / 25.0).sin()]
    }).collect();
    report_check("chain sine 15 mm / 157 mm", &chain(&wp, link, frame, &wave, false), &wp);
    let hw = 60.0;
    let mut hearts = mail(&wp, link, frame, [-hw, -hw], [hw, hw], 0.0, &|p| heart(p, 55.0));
    paint(&mut hearts, &|p| if p[1] > 0.0 { 1 } else { 0 });
    report_check("heart mail 110 mm", &hearts, &wp);
    let turned = mail(&wp, link, frame, [-40.0, -40.0], [40.0, 40.0], 0.3, &|_| -1.0);
    report_check("square mail turned 0.3 rad", &turned, &wp);
    let mut domed = mail(&wp, link, frame, [-40.0, -40.0], [40.0, 40.0], 0.0, &|_| -1.0);
    drape(&mut domed, &|p| {
        let r2 = p[0] * p[0] + p[1] * p[1];
        let big = 120.0f64;
        if r2 >= big * big {
            return None;
        }
        let h = (big * big - r2).sqrt() - big;
        Some((h + 20.0, [p[0], p[1], h + big]))
    });
    report_check("square mail draped on a 120 mm dome", &domed, &wp);

    println!("sim (240 Hz, 4 substeps, sleep 2 mm/s):");
    for side in &sizes {
        let h = side * 0.5;
        let w = mail(&wp, link, frame, [-h, -h], [h, h], 0.0, &|_| -1.0);
        for (margin, hertz, sub) in [(0.0, 120.0, 4), (1.6, 120.0, 4), (1.6, 240.0, 8)] {
            for scene in [Scene::Hang, Scene::Drape, Scene::Drop] {
                run(scene, &w, 4.0, 1, margin, true, hertz, sub);
            }
        }
        run(Scene::Drape, &w, 4.0, 4, 1.6, true, 120.0, 4);
    }
    let ch = chain(&wp, link, frame, &[[0.0, 0.0], [0.0, 200.0]], false);
    run(Scene::Hang, &ch, 4.0, 1, 1.6, true, 120.0, 4);
    run(Scene::Hang, &ch, 4.0, 1, 1.6, true, 240.0, 8);
}
