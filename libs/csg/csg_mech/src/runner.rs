//! A live sim on its own thread: steps an assembly's [`MechWorld`] at a fixed
//! 240 Hz in real time (or slowed down), takes commands (play, pause, reset,
//! grab, drag, release, pin, dangle, poke, shake), and publishes a snapshot of every part's pose
//! after each step. The UI reads the latest snapshot without ever waiting on the
//! sim (a try-lock that falls back to the snapshot it already has), and drains
//! the events (hard contacts, limit hits) for haptics.

use crate::geom::*;
use crate::sim::{Assembly, BodyAction, JointKind, MechWorld, Pose};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HZ: f64 = 240.0;
/// At most this many steps between two snapshots (two 120 Hz frames' worth). A step that costs more than
/// real time lets the sim fall behind (slow motion) instead of spending ever longer catching up while the
/// view waits for a snapshot.
pub const MAX_CATCH_UP: u64 = 4;

/// A pointer's firm hold (60 Hz against the default 240 Hz * 4 substeps).
pub const POINTER_HZ: f64 = 60.0;
/// A hand's hold: firm enough that the part keeps up with the hand (Hz; well under a quarter of
/// the substep rate, so it stays stable).
pub const HAND_HZ: f64 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    /// Back to the print pose, running; clears grabs and interactive pins.
    Reset,
    /// Sim seconds per wall second (1, 0.25, 0.1).
    Speed(f64),
    /// mm / s^2.
    Gravity(P3),
    /// Start dragging a part by a model-frame point on it (where it is now), pulled by a spring of
    /// `hz` ([`POINTER_HZ`] and [`HAND_HZ`] are firm holds).
    Grab { body: usize, point: P3, hz: f64 },
    /// Toggle a fixed point anchor on this body (model frame).
    Pin { body: usize, point: P3 },
    /// Hold by a soft spring, moved with Drag and ended with Release.
    Dangle { body: usize, point: P3 },
    /// Mass-scaled impulse at a model-frame point; `dv` in mm/s.
    Poke { body: usize, point: P3, dv: P3 },
    /// Move the drag target (model frame).
    Drag { point: P3 },
    /// Let go: the part keeps its velocity, or (a throw) the held point leaves at `velocity`
    /// (model frame, mm/s).
    Release { velocity: Option<P3> },
    /// Shake the held anchors (on/off).
    Shake(bool),
    /// One step while paused.
    Step,
    /// Do something to these bodies only.
    Bodies { bodies: Vec<usize>, action: BodyAction },
}

/// Something the user should feel or hear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// A hard contact: the parts (None: the floor) and the approach speed, mm/s.
    Hit { a: usize, b: Option<usize>, speed: f64 },
    /// A joint ran into a limit.
    Limit { joint: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub frame: u64,
    pub time: f64,
    pub running: bool,
    pub speed: f64,
    pub poses: Vec<Pose>,
    /// The parts' centres of mass as printed (a pose maps model points through it: [`Pose::apply`]).
    pub rest: Vec<P3>,
    /// Joint angles (revolute: angle, ball: cone), radians.
    pub angles: Vec<f64>,
    pub grabbed: Option<usize>,
    pub pinned: Vec<usize>,
    pub frozen: Vec<usize>,
}

impl Snapshot {
    /// Body `i`'s transform as a row-major 3x4 affine of the model frame (as printed -> now).
    pub fn affine(&self, i: usize) -> [[f64; 4]; 3] {
        let p = &self.poses[i];
        let (x, y, z, w) = (p.rot[0], p.rot[1], p.rot[2], p.rot[3]);
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
            [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
        ];
        let c = self.rest[i];
        std::array::from_fn(|k| {
            let t = p.com[k] - (r[k][0] * c[0] + r[k][1] * c[1] + r[k][2] * c[2]);
            [r[k][0], r[k][1], r[k][2], t]
        })
    }
}

pub struct Runner {
    tx: Sender<Command>,
    latest: Arc<Mutex<Arc<Snapshot>>>,
    events: Arc<Mutex<Vec<Event>>>,
    held: Arc<Snapshot>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn snapshot(w: &mut MechWorld, asm: &Assembly, frame: u64, running: bool, speed: f64) -> Snapshot {
    Snapshot {
        frame,
        time: w.time(),
        running,
        speed,
        poses: w.poses(),
        rest: (0..asm.bodies.len()).map(|i| w.rest_com(i)).collect(),
        angles: w.angles(asm),
        grabbed: w.grabbed(),
        pinned: w.pinned(),
        frozen: w.frozen(),
    }
}

impl Runner {
    /// Start the sim thread, paused at the print pose.
    pub fn start(asm: Assembly, gravity: P3) -> Runner {
        Self::start_with_floor(asm, gravity, None)
    }

    /// [`Runner::start`] with a floor across gravity (`MechWorld::new`'s level), for a piece that rests on the bed.
    pub fn start_with_floor(asm: Assembly, gravity: P3, floor: Option<f64>) -> Runner {
        let (tx, rx) = channel();
        // The print pose is known without constructing collision hulls on the caller's UI thread.
        // The physics world is built once, by its owning worker.
        let first = Arc::new(Snapshot {
            frame: 0,
            time: 0.0,
            running: false,
            speed: 1.0,
            poses: asm.bodies.iter().map(|b| Pose { com: b.mass.com, rot: [0.0, 0.0, 0.0, 1.0] }).collect(),
            rest: asm.bodies.iter().map(|b| b.mass.com).collect(),
            angles: vec![0.0; asm.joints.len()],
            grabbed: None,
            pinned: Vec::new(),
            frozen: Vec::new(),
        });
        let latest = Arc::new(Mutex::new(first.clone()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (latest, events, stop) = (latest.clone(), events.clone(), stop.clone());
            std::thread::Builder::new().name("mech-sim".into()).spawn(move || {
                let w = MechWorld::new(&asm, gravity, floor);
                run(asm, gravity, floor, w, rx, latest, events, stop)
            }).ok()
        };
        Runner { tx, latest, events, held: first, stop, thread }
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    /// The newest snapshot; never waits on the sim (keeps the last one when the sim holds the lock).
    pub fn latest(&mut self) -> Arc<Snapshot> {
        if let Ok(s) = self.latest.try_lock() {
            self.held = s.clone();
        }
        self.held.clone()
    }

    /// The events since the last call (never waits; an event stays queued when the lock is busy).
    pub fn take_events(&self) -> Vec<Event> {
        match self.events.try_lock() {
            Ok(mut e) => std::mem::take(&mut *e),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Changing scenes must not wait for hull construction or a solver step. The worker
        // observes stop and disposes of its world itself; dropping its handle detaches it.
        drop(self.thread.take());
    }
}

fn run(asm: Assembly, gravity: P3, floor: Option<f64>, mut w: MechWorld, rx: Receiver<Command>, latest: Arc<Mutex<Arc<Snapshot>>>, events: Arc<Mutex<Vec<Event>>>, stop: Arc<AtomicBool>) {
    let dt = 1.0 / HZ;
    let mut running = false;
    let mut speed = 1.0;
    let mut gravity = gravity;
    let mut shake = false;
    let mut frame = 0u64;
    let mut owed = 0.0;
    let mut last = Instant::now();
    let mut at_limit = vec![false; asm.joints.len()];
    let mut pending: Vec<Event> = Vec::new();
    let mut drag_target: Option<P3> = None;
    let mut behind = 0.0;
    let mut behind_logged = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let mut single = false;
        let mut changed = false;
        while let Ok(c) = rx.try_recv() {
            changed = true;
            match c {
                Command::Play => running = true,
                Command::Pause => running = false,
                Command::Reset => {
                    w = MechWorld::new(&asm, gravity, floor);
                    running = true;
                    owed = 0.0;
                    at_limit.iter_mut().for_each(|a| *a = false);
                    shake = false;
                    drag_target = None;
                }
                Command::Speed(s) => speed = s.clamp(0.01, 4.0),
                Command::Gravity(g) => {
                    gravity = g;
                    makepad_box3d::physics_world::world_set_gravity(&mut w.world, makepad_box3d::math_functions::vec3(g[0] as f32, g[1] as f32, g[2] as f32));
                }
                Command::Grab { body, point, hz } => {
                    if body < asm.bodies.len() {
                        w.grab(body, point, hz);
                        drag_target = Some(point);
                    }
                }
                Command::Pin { body, point } => {
                    if body < asm.bodies.len() {
                        w.pin(body, point);
                    }
                }
                Command::Dangle { body, point } => {
                    if body < asm.bodies.len() {
                        w.dangle(body, point);
                        drag_target = Some(point);
                    }
                }
                Command::Poke { body, point, dv } => {
                    if body < asm.bodies.len() {
                        w.poke(body, point, dv);
                    }
                }
                Command::Drag { point } => drag_target = Some(point),
                Command::Release { velocity } => {
                    if let Some(v) = velocity {
                        w.throw(v);
                    }
                    w.release();
                    drag_target = None;
                }
                Command::Shake(on) => shake = on,
                Command::Step => single = true,
                Command::Bodies { bodies, action } => w.apply(&bodies, &action),
            }
        }
        let now = Instant::now();
        let wall = (now - last).as_secs_f64().min(0.1);
        last = now;
        if running {
            owed += wall * speed;
        }
        let mut steps = (owed / dt).floor() as u64;
        owed -= steps as f64 * dt;
        if steps > MAX_CATCH_UP {
            behind += (steps - MAX_CATCH_UP) as f64 * dt;
            steps = MAX_CATCH_UP;
            if behind_logged.elapsed().as_secs_f64() > 1.0 {
                eprintln!("mech-sim: steps cost more than real time; {:.0} ms of sim time slowed down", behind * 1e3);
                behind = 0.0;
                behind_logged = Instant::now();
            }
        }
        if single && !running {
            steps = 1;
        }
        for _ in 0..steps {
            if let Some(p) = drag_target {
                w.drag(p, dt);
            }
            if shake {
                let t = w.time();
                let up = norm(asm.up);
                let side = basis(up).0;
                w.move_ground(scale(side, 12.0 * (std::f64::consts::TAU * 2.2 * t).sin()), up, 18f64.to_radians() * (std::f64::consts::TAU * 1.6 * t).sin(), dt);
            }
            w.step(dt);
            frame += 1;
            for (a, b, speed) in w.hits() {
                if speed > 30.0 {
                    pending.push(Event::Hit { a, b, speed });
                }
            }
            let ang = w.angles(&asm);
            for (j, js) in asm.joints.iter().enumerate() {
                let near = match js.kind {
                    JointKind::Revolute { lower, upper } => ang[j] < lower + 1f64.to_radians() || ang[j] > upper - 1f64.to_radians(),
                    JointKind::Spherical { cone, .. } => ang[j] > cone - 1f64.to_radians(),
                };
                if near && !at_limit[j] {
                    pending.push(Event::Limit { joint: j });
                }
                at_limit[j] = near;
            }
        }
        if steps > 0 || single || changed {
            // every published snapshot has its own frame number (a view skips a frame it already shows)
            if steps == 0 {
                frame += 1;
            }
            let snap = Arc::new(snapshot(&mut w, &asm, frame, running, speed));
            if let Ok(mut l) = latest.lock() {
                *l = snap;
            }
        }
        if !pending.is_empty() {
            if let Ok(mut e) = events.try_lock() {
                e.append(&mut pending);
                if e.len() > 256 {
                    let n = e.len() - 256;
                    e.drain(0..n);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
