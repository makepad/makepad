//! `walk_probe` — measure how an autonomous tour actually walks a real map.
//!
//! The tour (`level::LevelWalker` + `player_nav::PlayerNav`) is judged by eye
//! today: "the quake bot is more jittery than the doom one" is a true report
//! with no number behind it. This runs the SAME stack the world previews and
//! the sandbox run — real imported GLB, real `LevelCollision`, real `NavGrid`,
//! real planner — headless at a fixed 60 Hz, and prints per-tick motion plus
//! the summary a person means by "jittery":
//!
//! - `yawrev/s`  — yaw-rate SIGN REVERSALS per second. A body walking a line
//!   or a curve has none; a body oscillating has many. This is the number.
//! - `|dyaw|/s`  — mean absolute turn rate, deg/s.
//! - `stall/s`   — ticks per second with essentially no ground covered while
//!   the tour was not deliberately holding (a door, a cut).
//! - `pop/s`     — floor-height jumps that are neither a fall nor a step.
//! - `straight`  — net displacement / path length over one-second windows: 1
//!   is a straight walk, 0 is walking on the spot.
//! - `stuckmax`  — longest run of seconds with under 0.3 m of net travel: the
//!   "gets stuck on a corner" report, in seconds.
//!
//! Usage:
//! ```text
//! cargo run --release -p makepad-render --example walk_probe -- \
//!     <world.glb> <world.spawn> <doom|quake|duke|none> [secs] [seed] [csv]
//! ```
//! `<world.spawn>` is the importer's sidecar; `-` skips it (no anchors).

use makepad_render::level::{
    surface_kinds_from_glb, BobStyle, LevelCollision, LevelWalker, NavGrid, SurfaceKind, UpAxis,
    WalkerConfig, WalkerEvent,
};
use makepad_render::model::MODEL_VERTEX_FLOATS;
use makepad_render::player_nav::{config_for_world, NavAnchor, PlayerNav};
use makepad_render::StaticModel;
use makepad_draw::*;

const DT: f32 = 1.0 / 60.0;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 3 {
        eprintln!("usage: walk_probe <glb> <spawn|-> <style> [secs] [seed] [csv]");
        std::process::exit(2);
    }
    let glb = std::fs::read(&a[0]).expect("read glb");
    let spawn = (a[1] != "-")
        .then(|| std::fs::read_to_string(&a[1]).expect("read spawn"))
        .unwrap_or_default();
    let style = match a[2].as_str() {
        "doom" => BobStyle::Doom,
        "quake" => BobStyle::Quake,
        "duke" => BobStyle::Duke,
        _ => BobStyle::None,
    };
    let secs: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let seed: u64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(7);
    let csv = a.get(5).cloned();

    let model = StaticModel::parse_glb(&glb).expect("parse glb");
    let anchors = parse_spawn(&spawn);
    // `WALK_PROBE_PRESET=1` walks with the style preset alone — the body
    // before this lane read the map's declared step, kept so a before/after
    // is one binary and one run.
    let cfg = match std::env::var_os("WALK_PROBE_PRESET").is_some() {
        true => WalkerConfig::for_style(style),
        false => config_for_world(style, &anchors),
    };
    println!(
        "cfg: eye {:.3} step {:.3} radius {:.3} height {:.3} speed {:.2} gravity {:.1} fall {:.2}",
        cfg.eye_height, cfg.step_up, cfg.radius, cfg.height, cfg.speed, cfg.gravity, cfg.fall_limit
    );

    let level = LevelCollision::from_packed(
        &model.vertices,
        MODEL_VERTEX_FLOATS,
        &model.indices,
        UpAxis::Y,
    )
    .expect("level collision");
    let level = match surface_kinds_from_glb(&glb, model.triangle_count()) {
        Some(kinds) => {
            let hz = kinds.iter().filter(|k| **k == SurfaceKind::Hazard).count();
            println!("kinds: {hz} hazard of {} tris", kinds.len());
            level.with_kinds(kinds)
        }
        None => level,
    };
    let t0 = std::time::Instant::now();
    let mut grid = NavGrid::build(&level, &cfg);
    // Door leaves are animated nodes, not static collision: their cells are
    // walkable and the tour asks for them to be opened.
    let doors: Vec<(String, (Vec3f, Vec3f))> = model
        .anim_parts
        .iter()
        .map(|p| (p.name.clone(), part_box(p)))
        .collect();
    let boxes: Vec<(Vec3f, Vec3f)> = doors.iter().map(|(_, b)| *b).collect();
    if !boxes.is_empty() {
        grid.mark_doors(&boxes);
    }
    println!(
        "grid: {} cells, cell {:.3} m, {} anim parts, build {:?}",
        grid.len(),
        grid.cell_size(),
        doors.len(),
        t0.elapsed()
    );
    if grid.is_empty() {
        println!("NO NAV GRID");
        return;
    }
    let seed = seed ^ level.triangles() as u64;
    let mut player = PlayerNav::new(&grid, &level, &cfg, &anchors, seed).expect("player nav");
    {
        let g = player.graph();
        let band = (0..grid.len() as u32).filter(|c| g.room_at(*c).is_none()).count();
        let corridors = (0..g.rooms()).filter(|r| g.is_corridor(*r as u16)).count();
        println!(
            "rooms: {} ({corridors} corridors), {} portals, {band}/{} cells in a portal band ({:.1}%)",
            g.rooms(),
            g.portals(),
            grid.len(),
            band as f32 * 100.0 / grid.len() as f32
        );
    }
    let (start, start_yaw) = player.start_hint();
    let mut walker = LevelWalker::new(start, start_yaw, cfg, seed);

    // Door drive, standing in for the renderer's declared state machine: a
    // part the tour asks for reads "settled open" after its clip has run.
    let mut door_timer: Vec<f32> = vec![0.0; doors.len()];

    let mut rows: Vec<String> = Vec::new();
    let mut m = Metrics::new();
    let ticks = (secs / DT) as usize;
    let mut prev_yaw = walker.yaw();
    let mut prev_dyaw = 0.0f32;
    let mut prev_feet = walker.feet();
    let mut prev_route = 0usize;
    let mut legs: Vec<(usize, usize)> = Vec::new();
    for i in 0..ticks {
        let t = i as f32 * DT;
        player.steer(DT, &mut walker, &grid, &level);
        let ev = walker.tick_in(DT, &level, Some(&grid));
        if let Some(d) = walker.wanted_door() {
            let k = d as usize;
            if k < door_timer.len() {
                door_timer[k] += DT;
                if door_timer[k] >= 0.6 {
                    walker.set_door_open(d, true);
                }
            } else {
                walker.set_door_open(d, true);
            }
        }
        let feet = walker.feet();
        let yaw = walker.yaw();
        let cam = walker.camera();
        let route_len = walker.route().len();
        if route_len > prev_route {
            legs.push((player.leg_cells().len(), route_len));
        }
        prev_route = route_len;
        let want_door = walker.wanted_door().map(|d| d as i32).unwrap_or(-1);
        let dyaw = wrap_pi(yaw - prev_yaw);
        let step = ((feet.x - prev_feet.x).powi(2) + (feet.z - prev_feet.z).powi(2)).sqrt();
        let dy = feet.y - prev_feet.y;
        m.tick(dyaw, prev_dyaw, step, dy, feet, ev, walker.is_airborne(), cam.roll);
        if csv.is_some() {
            rows.push(format!(
                "{t:.4},{:.4},{:.4},{:.4},{yaw:.5},{dyaw:.6},{step:.5},{dy:.5},{:?},{:.5},{:.5},{:.5},{route_len},{want_door}",
                feet.x, feet.y, feet.z, ev, cam.yaw, cam.roll, cam.eye.y
            ));
        }
        prev_yaw = yaw;
        prev_dyaw = dyaw;
        prev_feet = feet;
    }
    m.finish();
    println!("{}", m.report(secs));
    if !legs.is_empty() {
        let dense: usize = legs.iter().map(|(d, _)| *d).sum();
        let sparse: usize = legs.iter().map(|(_, s)| *s).sum();
        println!(
            "legs: {} installed, {dense} dense cells -> {sparse} waypoints (pull {:.2}x), \
             mean leg {:.1} cells",
            legs.len(),
            dense as f32 / sparse.max(1) as f32,
            dense as f32 / legs.len() as f32
        );
        let careful = legs.iter().filter(|(d, s)| d == s).count();
        println!("  legs planned cell-by-cell (careful, no string-pull): {careful}/{}", legs.len());
    }
    let ns = walker.nav_stats();
    println!(
        "coverage: distinct {}/{} cells, reachable {} ({} unseen), replans {}, cuts {}",
        ns.distinct, ns.cells, ns.reachable, ns.unseen, ns.replans, ns.cuts
    );
    let s = player.stats();
    println!(
        "player: rooms {}/{}, legs {}, doors {}, jams {}, cuts {}, restarts {}",
        s.visited, s.rooms, s.legs, s.doors_opened, s.jammed, s.region_cuts, s.restarts
    );
    if let Some(path) = csv {
        let mut out = String::from("t,x,y,z,yaw,dyaw,step,dy,event,camyaw,roll,eyey,route,door\n");
        out.push_str(&rows.join("\n"));
        std::fs::write(&path, out).expect("write csv");
        println!("csv: {path}");
    }
}

/// Ground covered under which a tick counts as a stall (a hair of numerical
/// creep is not walking).
const STALL_STEP: f32 = 0.002;

struct Metrics {
    reversals: usize,
    turn_sum: f32,
    stalls: usize,
    pops: usize,
    blocked: usize,
    path: f32,
    /// One-second windows: (start feet, path in window).
    win_start: Option<Vec3f>,
    win_path: f32,
    win_ticks: usize,
    straights: Vec<f32>,
    /// Seconds of the current under-0.3 m run, and the worst seen.
    stuck_now: f32,
    stuck_max: f32,
    ticks: usize,
    roll_rev: usize,
    roll_sum: f32,
    prev_roll: f32,
    prev_droll: f32,
}

impl Metrics {
    fn new() -> Metrics {
        Metrics {
            reversals: 0,
            turn_sum: 0.0,
            stalls: 0,
            pops: 0,
            blocked: 0,
            path: 0.0,
            win_start: None,
            win_path: 0.0,
            win_ticks: 0,
            straights: Vec::new(),
            stuck_now: 0.0,
            stuck_max: 0.0,
            ticks: 0,
            roll_rev: 0,
            roll_sum: 0.0,
            prev_roll: 0.0,
            prev_droll: 0.0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tick(
        &mut self,
        dyaw: f32,
        prev_dyaw: f32,
        step: f32,
        dy: f32,
        feet: Vec3f,
        ev: WalkerEvent,
        airborne: bool,
        roll: f32,
    ) {
        self.ticks += 1;
        // Camera roll is the instantaneous turn rate (Quake's `cl_rollangle`):
        // it is not smoothed anywhere, so it is the most direct read-out of a
        // heading that keeps changing its mind.
        let droll = roll - self.prev_roll;
        if droll.abs() > 1e-5 && self.prev_droll.abs() > 1e-5 && droll * self.prev_droll < 0.0 {
            self.roll_rev += 1;
        }
        self.roll_sum += droll.abs();
        self.prev_droll = droll;
        self.prev_roll = roll;
        // A reversal only counts when both turns are real motion, or every
        // float wobble at a standstill would read as a flip.
        const TURN_EPS: f32 = 0.0008; // ~0.05 deg / tick
        if dyaw.abs() > TURN_EPS && prev_dyaw.abs() > TURN_EPS && dyaw * prev_dyaw < 0.0 {
            self.reversals += 1;
        }
        self.turn_sum += dyaw.abs();
        if step < STALL_STEP {
            self.stalls += 1;
        }
        if ev == WalkerEvent::Blocked {
            self.blocked += 1;
        }
        // A height jump while on the ground and not stepping over ground the
        // body covered: the floor probe changed its mind under the feet.
        if !airborne && dy.abs() > 0.02 && dy.abs() > step * 2.0 {
            self.pops += 1;
        }
        self.path += step;
        self.win_path += step;
        let start = *self.win_start.get_or_insert(feet);
        self.win_ticks += 1;
        if self.win_ticks >= 60 {
            let net = ((feet.x - start.x).powi(2) + (feet.z - start.z).powi(2)).sqrt();
            if self.win_path > 0.05 {
                self.straights.push(net / self.win_path);
            } else {
                self.straights.push(0.0);
            }
            if net < 0.3 {
                self.stuck_now += 1.0;
                self.stuck_max = self.stuck_max.max(self.stuck_now);
            } else {
                self.stuck_now = 0.0;
            }
            self.win_start = Some(feet);
            self.win_path = 0.0;
            self.win_ticks = 0;
        }
    }

    fn finish(&mut self) {}

    fn report(&self, secs: f32) -> String {
        let straight = if self.straights.is_empty() {
            0.0
        } else {
            self.straights.iter().sum::<f32>() / self.straights.len() as f32
        };
        format!(
            "yawrev/s {:.2}  |dyaw|/s {:.1}deg  stall/s {:.2}  blocked/s {:.2}  pop/s {:.2}  \
             straight {:.3}  stuckmax {:.0}s  path {:.1}m  rollrev/s {:.2}  |droll|/s {:.1}deg",
            self.reversals as f32 / secs,
            self.turn_sum.to_degrees() / secs,
            self.stalls as f32 / secs,
            self.blocked as f32 / secs,
            self.pops as f32 / secs,
            straight,
            self.stuck_max,
            self.path,
            self.roll_rev as f32 / secs,
            self.roll_sum.to_degrees() / secs,
        )
    }
}

fn wrap_pi(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut a = a % tau;
    if a > std::f32::consts::PI {
        a -= tau;
    } else if a < -std::f32::consts::PI {
        a += tau;
    }
    a
}

fn part_box(p: &makepad_render::model::AnimPart) -> (Vec3f, Vec3f) {
    let m = p.rest_transform();
    let (mut lo, mut hi) = (
        vec3f(f32::MAX, f32::MAX, f32::MAX),
        vec3f(f32::MIN, f32::MIN, f32::MIN),
    );
    for x in [p.min.x, p.max.x] {
        for y in [p.min.y, p.max.y] {
            for z in [p.min.z, p.max.z] {
                let q = m.transform_vec4(Vec4f { x, y, z, w: 1.0 }).to_vec3f();
                lo.x = lo.x.min(q.x);
                lo.y = lo.y.min(q.y);
                lo.z = lo.z.min(q.z);
                hi.x = hi.x.max(q.x);
                hi.y = hi.y.max(q.y);
                hi.z = hi.z.max(q.z);
            }
        }
    }
    (lo, hi)
}

/// The importer's `.spawn` sidecar, in the shape `PlayerNav` reads. Same
/// fields the catalog publishes as anchors (`world_nav::WorldNav::anchors`).
fn parse_spawn(text: &str) -> Vec<NavAnchor> {
    let mut out = Vec::new();
    for line in text.lines().skip(3) {
        let mut p = line.split_whitespace();
        let kind = p.next().unwrap_or("");
        match kind {
            "floor" | "step" | "eye" => {
                let v: f32 = match p.next().and_then(|s| s.parse().ok()) {
                    Some(v) => v,
                    None => continue,
                };
                let name = match kind {
                    "floor" => "floor_height",
                    "step" => "step_height",
                    _ => "eye_height",
                };
                out.push(NavAnchor {
                    name: name.into(),
                    pos: vec3f(0.0, v, 0.0),
                    yaw: 0.0,
                    scale: vec3f(1.0, 1.0, 1.0),
                });
            }
            "start" | "marker" => {
                let name = p.next().unwrap_or("").to_string();
                let n: Vec<f32> = p.filter_map(|s| s.parse().ok()).collect();
                if name.is_empty() || n.len() < 4 || name.starts_with("deathmatch") {
                    continue;
                }
                out.push(NavAnchor {
                    name,
                    pos: vec3f(n[0], n[1], n[2]),
                    yaw: n[3],
                    scale: vec3f(1.0, 1.0, 1.0),
                });
            }
            _ => {}
        }
    }
    out
}
