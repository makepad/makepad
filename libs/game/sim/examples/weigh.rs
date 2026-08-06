//! Engine weight harness: per-tick CPU, allocations and bytes, for
//! representative worlds. Run with `--release` for meaningful numbers.
//!
//!   cargo run -p makepad-game-sim --release --example weigh
//!
//! Allocation counting uses a wrapping global allocator, so the numbers
//! include everything the tick touches, not just what this file allocates.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use makepad_game_sim::*;
use makepad_math::*;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(l.size() as u64, Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(new as u64, Relaxed);
        System.realloc(p, l, new)
    }
}

#[global_allocator]
static A: Counting = Counting;

fn ent(id: u64, kind: BodyKind, pos: Vec3f, half: Vec3f) -> Entity {
    Entity {
        id,
        kind,
        pos,
        half,
        collide: true,
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.6,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        speed_mult: 1.0,
        ..Default::default()
    }
}

/// Square heightfield with a gentle hill, the shape a game's ground has.
fn terrain(cells: usize) -> Terrain {
    let cell_size = 1.0;
    let origin = -(cells as f32 * cell_size) * 0.5;
    let mut heights = Vec::with_capacity(cells * cells);
    let mut colors = Vec::with_capacity(cells * cells);
    for z in 0..cells {
        for x in 0..cells {
            let fx = x as f32 / cells as f32 - 0.5;
            let fz = z as f32 / cells as f32 - 0.5;
            heights.push(3.0 * (1.0 - (fx * fx + fz * fz) * 4.0).max(0.0));
            colors.push(vec4f(0.4, 0.6, 0.3, 1.0));
        }
    }
    Terrain { cells, cell_size, origin, heights, colors, revision: 1 }
}

struct Scene {
    name: &'static str,
    world: GameWorld,
}

fn scene(name: &'static str, statics: usize, movers: usize, rigids: usize, cells: usize) -> Scene {
    let mut w = GameWorld::new();
    w.reset_content();
    if cells > 0 {
        w.terrain = Some(terrain(cells));
    }
    let mut id = 0u64;
    id += 1;
    w.push_entity(ent(id, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(60.0, 0.5, 60.0)));
    for i in 0..statics {
        id += 1;
        let a = i as f32 * 0.7;
        w.push_entity(ent(
            id,
            BodyKind::Static,
            vec3f(a.cos() * (8.0 + i as f32 * 0.3), 1.0, a.sin() * (8.0 + i as f32 * 0.3)),
            vec3f(0.5, 1.0, 0.5),
        ));
    }
    for i in 0..movers {
        id += 1;
        let mut e = ent(
            id,
            BodyKind::Mover,
            vec3f((i % 20) as f32 - 10.0, 6.0, (i / 20) as f32 - 5.0),
            vec3f(0.4, 0.8, 0.4),
        );
        e.vel = vec3f(1.0, 0.0, 0.6);
        e.auto_face = true;
        e.turn_rate = 6.0;
        w.push_entity(e);
    }
    for i in 0..rigids {
        id += 1;
        let mut e = ent(
            id,
            BodyKind::Rigid,
            vec3f((i % 8) as f32 * 1.1, 2.0 + (i / 8) as f32 * 1.1, -6.0),
            vec3f(0.5, 0.5, 0.5),
        );
        e.restitution = 0.05;
        w.push_entity(e);
    }
    Scene { name, world: w }
}

fn measure(s: &mut Scene, ticks: usize) {
    // Warm: first ticks build the box3d mirror and any lazy caches.
    for _ in 0..30 {
        step_world(&mut s.world);
    }
    let a0 = ALLOCS.load(Relaxed);
    let b0 = BYTES.load(Relaxed);
    let t0 = Instant::now();
    for _ in 0..ticks {
        step_world(&mut s.world);
    }
    let dt = t0.elapsed();
    let allocs = ALLOCS.load(Relaxed) - a0;
    let bytes = BYTES.load(Relaxed) - b0;
    let statics = s
        .world
        .entities
        .iter()
        .filter(|e| matches!(e.kind, BodyKind::Static | BodyKind::Kinematic))
        .count();
    println!(
        "{:<22} ents {:>5}  static {:>5}  terrain {:>6}  |  {:>8.3} ms/tick  \
         {:>7} allocs/tick  {:>9} B/tick",
        s.name,
        s.world.entities.len(),
        statics,
        s.world.terrain.as_ref().map(|t| t.heights.len()).unwrap_or(0),
        dt.as_secs_f64() * 1000.0 / ticks as f64,
        allocs / ticks as u64,
        bytes / ticks as u64,
    );
}

/// Resident set size in bytes (macOS `ps`), for the soak check.
fn rss() -> u64 {
    let pid = std::process::id();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

/// 10 simulated minutes of a busy world: RSS must be flat at the end, or the
/// tick path is leaking. Spawns and despawns projectiles throughout so the
/// entity list churns rather than sitting still.
fn soak(minutes: usize) {
    let mut s = scene("soak", 200, 20, 20, 129);
    let ticks = minutes * 60 * 60;
    let mut next_id = s.world.entities.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    let mut samples: Vec<(usize, u64)> = Vec::new();
    for t in 0..ticks {
        // Churn: a short-lived projectile every 30 ticks.
        if t % 30 == 0 {
            let mut e = ent(
                next_id,
                BodyKind::Mover,
                vec3f(0.0, 8.0, 0.0),
                vec3f(0.2, 0.2, 0.2),
            );
            e.vel = vec3f(3.0, 2.0, 1.0);
            e.life = 0.75;
            e.hits = true;
            s.world.push_entity(e);
            next_id += 1;
        }
        step_world(&mut s.world);
        let _ = collect_touches(&s.world);
        if t % (ticks / 10).max(1) == 0 {
            samples.push((t, rss()));
        }
    }
    samples.push((ticks, rss()));
    println!("\nsoak: {minutes} simulated minutes ({ticks} ticks), busy world");
    for (t, r) in &samples {
        println!("  tick {t:>7}  RSS {:>7.1} MB", *r as f64 / 1048576.0);
    }
    let first = samples[1].1 as f64;
    let last = samples.last().unwrap().1 as f64;
    let drift = (last - first) / first * 100.0;
    println!(
        "  drift after warmup: {drift:+.1}%  ({} entities alive at end)",
        s.world.entities.len()
    );
}

/// Movers packed tightly enough that the separation pass actually engages,
/// all walking at a common point — the bench-crowd case. A scene with the same
/// mover count spread out costs far less, so this is the honest worst case.
fn crowd(name: &'static str, movers: usize, statics: usize) -> Scene {
    let mut w = GameWorld::new();
    w.reset_content();
    let mut id = 0u64;
    id += 1;
    w.push_entity(ent(id, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(60.0, 0.5, 60.0)));
    for i in 0..statics {
        id += 1;
        let a = i as f32 * 0.7;
        w.push_entity(ent(
            id,
            BodyKind::Static,
            vec3f(a.cos() * (14.0 + i as f32 * 0.2), 1.0, a.sin() * (14.0 + i as f32 * 0.2)),
            vec3f(0.5, 1.0, 0.5),
        ));
    }
    // A disc of bodies half a metre apart: everyone overlaps a neighbour.
    let per_row = (movers as f32).sqrt().ceil() as usize;
    for i in 0..movers {
        id += 1;
        let (gx, gz) = (i % per_row, i / per_row);
        let mut e = ent(
            id,
            BodyKind::Mover,
            vec3f(
                gx as f32 * 0.5 - per_row as f32 * 0.25,
                0.8,
                gz as f32 * 0.5 - per_row as f32 * 0.25,
            ),
            vec3f(0.4, 0.8, 0.4),
        );
        e.vel = vec3f(0.0, 0.0, 0.0);
        e.auto_face = true;
        e.turn_rate = 6.0;
        w.push_entity(e);
    }
    Scene { name, world: w }
}

fn main() {
    if std::env::args().any(|a| a == "--soak") {
        soak(10);
        return;
    }
    if std::env::args().any(|a| a == "--crowd") {
        println!("mover separation cost (packed crowds, release recommended)\n");
        for s in [
            crowd("crowd 50", 50, 0),
            crowd("crowd 200", 200, 0),
            crowd("crowd 12 (village)", 12, 500),
            crowd("crowd 50 + 500 static", 50, 500),
            crowd("crowd 200 + 500 static", 200, 500),
        ]
        .iter_mut()
        {
            measure(s, 600);
        }
        return;
    }
    let ticks: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    println!("step_world weight, {ticks} ticks each (release recommended)\n");
    let mut scenes = vec![
        scene("demo (arcade)", 20, 2, 8, 0),
        scene("demo + terrain 65", 20, 2, 8, 65),
        scene("racing-ish", 40, 4, 0, 129),
        scene("terrain 129 only", 4, 2, 0, 129),
        scene("terrain 257 only", 4, 2, 0, 257),
        scene("large (500 static)", 500, 20, 20, 129),
        scene("stress (2000 static)", 2000, 50, 50, 257),
    ];
    for s in scenes.iter_mut() {
        measure(s, ticks);
    }
}
