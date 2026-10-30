//! A live sim on its own thread: steps an assembly's [`MechWorld`] at a fixed
//! 240 Hz in real time (or slowed down), takes commands (play, pause, reset,
//! grab, drag, release, shake), and publishes a snapshot of every part's pose
//! after each step. The UI reads the latest snapshot without ever waiting on the
//! sim (a try-lock that falls back to the snapshot it already has), and drains
//! the events (hard contacts, limit hits) for haptics.

use crate::geom::*;
use crate::sim::{Assembly, JointKind, MechWorld, Pose};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HZ: f64 = 240.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    /// Back to the print pose, paused.
    Reset,
    /// Sim seconds per wall second (1, 0.25, 0.1).
    Speed(f64),
    /// mm / s^2.
    Gravity(P3),
    /// Start dragging a part by a model-frame point on it (where it is now).
    Grab { body: usize, point: P3 },
    /// Move the drag target (model frame).
    Drag { point: P3 },
    /// Let go: the part keeps its velocity.
    Release,
    /// Shake the held anchors (on/off).
    Shake(bool),
    /// One step while paused.
    Step,
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
        // box3d worlds stay on one thread: this one is only for the first snapshot, the sim builds its own
        let first = Arc::new(snapshot(&mut MechWorld::new(&asm, gravity, floor), &asm, 0, false, 1.0));
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
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
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
                    running = false;
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
                Command::Grab { body, point } => {
                    if body < asm.bodies.len() {
                        w.grab(body, point, 8.0);
                        drag_target = Some(point);
                    }
                }
                Command::Drag { point } => drag_target = Some(point),
                Command::Release => {
                    w.release();
                    drag_target = None;
                }
                Command::Shake(on) => shake = on,
                Command::Step => single = true,
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
