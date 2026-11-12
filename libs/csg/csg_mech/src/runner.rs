//! A live sim on its own thread: steps an assembly's world (the dynamics engine's [`DynWorld`] for
//! jointed pieces and chains, [`MechWorld`] for the rest) at a fixed 240 Hz in real time (or slowed down), takes commands (play, pause, reset,
//! grab, drag, release, pin, dangle, poke, shake, a motor's setting), and publishes a snapshot of every part's pose
//! after each step. The UI reads the latest snapshot without ever waiting on the
//! sim (a try-lock that falls back to the snapshot it already has), and drains
//! the events (hard contacts, limit hits) for haptics. The sim wakes the UI (the `wake` it was
//! started with) when it publishes news after the UI took the news ([`Runner::take_news`]): a view
//! that goes idle takes it, and no state it believes the sim is in can leave it asleep over news.

use crate::dyn_world::{DynWorld, Reset};
use makepad_dynamics::thread::TaskPool;
use crate::geom::*;
use crate::sim::{AnchorKind, Assembly, BodyAction, JointKind, MechWorld, Pose, StringPath};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HZ: f64 = 240.0;
/// At most this many steps between two snapshots (two 120 Hz frames' worth). A step that costs more than
/// real time lets the sim fall behind (slow motion) instead of spending ever longer catching up while the
/// view waits for a snapshot.
pub const MAX_CATCH_UP: u64 = 4;
/// A shake starts and stops over this long, seconds.
const SHAKE_EASE: f64 = 0.25;
/// [`Assembly::lift`] starts this long after the start (seconds) and rises this fast (mm/s).
const LIFT_AFTER: f64 = 0.6;
const LIFT_SPEED: f64 = 60.0;
/// Hang on a strung piece lifts its held parts (at [`LIFT_SPEED`]) while what hangs from them is less than this
/// far over the floor (mm).
pub const HANG_CLEAR: f64 = 10.0;
/// A hand's frames come slower than the steps: the held ground goes at the speed that gets it to
/// the hand in this long (seconds), so it doesn't jump and wait.
const CARRY_EASE: f64 = 1.0 / 90.0;

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
    /// Carry the held anchors, as a hand does: from their start, this turn about `axis` through
    /// the origin, then this move. They stay where the last one left them and no longer lift until
    /// a reset; a shake shakes them there.
    Carry { at: P3, axis: P3, angle: f64 },
    /// One step while paused.
    Step,
    /// Do something to these bodies only.
    Bodies { bodies: Vec<usize>, action: BodyAction },
    /// These joints take these limits (a check's measured ranges, [`JointKind::measured`]) where
    /// they are now; a Reset keeps them.
    Limits(Vec<(usize, JointKind)>),
    /// Set a joint's motor ([`DynWorld::set_motor`]): a servo's angle (radians) or travel (mm), a drive's speed
    /// (radians or mm per second). A Reset sets every motor back to where its joint printed, or to standing still.
    Motor { joint: usize, target: f64 },
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
    /// The commands the sim had taken up when it published this.
    pub commands: u64,
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
    /// Why there is no sim: its world could not be built (the sim has stopped).
    pub error: Option<String>,
    /// Every time the engine's state blew up and it put the parts back at the print pose.
    pub resets: Vec<Reset>,
    /// The strings as they hang, as the assembly lists them.
    pub strings: Vec<StringPath>,
    /// Each motor's joint and what it is set to.
    pub motors: Vec<(usize, f64)>,
    /// Per motor, in `motors`' order, the torque it pushes with (N mm; a slider's force, N).
    pub efforts: Vec<f64>,
    /// Per assembly joint, the force through it (N); none for a weld, and none on box3d.
    pub loads: Vec<Option<f64>>,
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
    /// The commands sent so far.
    sent: AtomicU64,
    latest: Arc<Mutex<Arc<Snapshot>>>,
    /// The sim has published since the UI last took the news ([`Runner::take_news`]).
    news: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<Event>>>,
    held: Arc<Snapshot>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What the dynamics engine steps: parts on joints or strings, or held by a hand (a jointed piece as
/// Mechanics shows it), weaves (chains and mail), and parts that meet through their fields (exactly, as
/// two distance fields). Balls meet fields on box3d (a support check's pieces against the part). A flex joint's spring is integrated explicitly on
/// the engine, and on a small part it is stiffer than a substep can hold (a jellyfish's needs 35 substeps of
/// a 240 Hz step), so a piece with one stays on box3d, whole, until the engine's position-implicit
/// integrator is ported.
pub fn on_dynamics(asm: &Assembly) -> bool {
    let woven = asm.bodies.iter().any(|b| !b.tubes.is_empty());
    let fields = asm.bodies.iter().any(|b| b.field.is_some());
    let moving = !asm.joints.is_empty() || !asm.strings.is_empty() || asm.anchors.iter().any(|a| a.kind == AnchorKind::Held);
    (moving || woven || fields)
        && asm.joints.iter().all(|j| !matches!(j.kind, JointKind::Flex { .. }))
        && asm.bodies.iter().all(|b| b.balls.is_empty())
}

/// Mail: a weave with a link that holds more than two others (a chain's hold one or two).
fn mail(asm: &Assembly) -> bool {
    let mut held = vec![0usize; asm.bodies.len()];
    for &(a, b) in &asm.interlocks {
        held[a] += 1;
        held[b] += 1;
    }
    held.iter().any(|&n| n > 2)
}

/// A weave on the dynamics engine steps in this long (ms) beside the view's work. A longer step is not
/// refused: the sim runs slower than real time ([`MAX_CATCH_UP`]), every step whole, and the weave says
/// so once ([`WeaveSteps`]).
const WEAVE_STEP_MS: f64 = 2.5;

/// A weave's step times, a second's worth at a time, until one step goes over [`WEAVE_STEP_MS`]: that
/// second is logged (the weave's link count, its 95th percentile and slowest step) and timing stops. The
/// first second is not judged: under the JIT the step's code compiles as it first runs (a 14-link chain's
/// first second has a p95 of 14 ms and one step of 146 ms; its later steps take about 2 ms).
struct WeaveSteps {
    links: usize,
    ms: Vec<f64>,
    over: bool,
    warm: bool,
    logged: bool,
}

impl WeaveSteps {
    fn add(&mut self, ms: f64) {
        if self.logged {
            return;
        }
        self.ms.push(ms);
        self.over |= ms > WEAVE_STEP_MS;
        if self.ms.len() < HZ as usize {
            return;
        }
        if self.over && self.warm {
            self.ms.sort_by(f64::total_cmp);
            let p95 = self.ms[self.ms.len() * 95 / 100];
            let slowest = self.ms[self.ms.len() - 1];
            makepad_dynamics::util_errmem::warning_text(&format!(
                "mech-sim: a weave of {} links steps over {WEAVE_STEP_MS} ms: p95 {p95:.2} ms, slowest {slowest:.2} ms over a second of steps",
                self.links
            ));
            self.logged = true;
        }
        self.ms.clear();
        self.over = false;
        self.warm = true;
    }
}

/// Solver substeps of a 240 Hz step on the dynamics engine. box3d and the joint checks keep the assembly's.
const DYNAMICS_SUBSTEPS: u32 = 8;

/// Substeps of a 240 Hz step for mail: a 41-link sheet keeps every link through its hang and drags at 4 as
/// at 8 and falls asleep as soon, in half the time (natively a drag's step is 5.0 ms at the median and
/// 5.7 at the 95th percentile, against 10.4 and 12.7 at 8, so it runs slower than real time there).
const MAIL_SUBSTEPS: u32 = 4;

/// Substeps of a 240 Hz step for a piece whose parts meet through their fields: finding where two fields
/// meet costs most of a step, and a roller bearing rests and turns at 4 as it does at 8, in half the time.
const FIELD_SUBSTEPS: u32 = 4;

/// A piece's world on the dynamics engine, at its own substeps.
pub fn dynamics_world(asm: &Assembly, gravity: P3, floor: Option<f64>) -> Result<DynWorld, String> {
    let substeps = if asm.bodies.iter().any(|b| b.field.is_some()) {
        FIELD_SUBSTEPS
    } else if mail(asm) {
        MAIL_SUBSTEPS
    } else {
        DYNAMICS_SUBSTEPS
    };
    DynWorld::new(&Assembly { substeps, ..asm.clone() }, gravity, floor)
}

/// The world a live sim steps: jointed pieces, weaves and parts that meet through their fields on the
/// dynamics engine ([`on_dynamics`]), every other assembly (free parts, a support check) on MechWorld. A weave's offline run ([`crate::weave::simulate`]) steps the
/// same world.
pub(crate) enum Engine {
    Mech(MechWorld),
    Dynamics(Box<DynWorld>),
}

/// `on!(world, w => expression with w)`: the expression on whichever world it is.
macro_rules! on {
    ($world:expr, $w:ident => $e:expr) => {
        match $world {
            Engine::Mech($w) => $e,
            Engine::Dynamics($w) => $e,
        }
    };
}

impl Engine {
    fn resets(&self) -> Vec<Reset> {
        match self {
            Engine::Mech(_) => Vec::new(),
            Engine::Dynamics(w) => w.resets().to_vec(),
        }
    }

    fn strings(&self) -> Vec<StringPath> {
        match self {
            Engine::Mech(_) => Vec::new(),
            Engine::Dynamics(w) => w.strings(),
        }
    }

    fn motors(&self) -> Vec<(usize, f64)> {
        match self {
            Engine::Mech(_) => Vec::new(),
            Engine::Dynamics(w) => w.motors(),
        }
    }

    fn efforts(&self) -> Vec<f64> {
        match self {
            Engine::Mech(_) => Vec::new(),
            Engine::Dynamics(w) => w.efforts(),
        }
    }

    fn loads(&self) -> Vec<Option<f64>> {
        match self {
            Engine::Mech(_) => Vec::new(),
            Engine::Dynamics(w) => w.loads(),
        }
    }

    pub(crate) fn step(&mut self, dt: f64) {
        on!(self, w => w.step(dt))
    }

    pub(crate) fn time(&self) -> f64 {
        on!(self, w => w.time())
    }

    pub(crate) fn max_speed(&self) -> f64 {
        on!(self, w => w.max_speed())
    }

    pub(crate) fn poses(&self) -> Vec<Pose> {
        on!(self, w => w.poses())
    }

    /// The dynamics engine's parallel work on `pool` (box3d keeps its own).
    fn bind_pool(&mut self, pool: &Option<Arc<dyn TaskPool>>) {
        if let Engine::Dynamics(w) = self {
            w.bind_pool(pool.clone());
        }
    }

    pub(crate) fn new(asm: &Assembly, gravity: P3, floor: Option<f64>) -> Result<Engine, String> {
        if on_dynamics(asm) {
            dynamics_world(asm, gravity, floor).map(|w| Engine::Dynamics(Box::new(w)))
        } else if !asm.strings.is_empty() || asm.anchors.iter().any(|a| a.kind == AnchorKind::Held) || asm.joints.iter().any(|j| j.motor.is_some()) {
            Err("strings, held parts and motors run only on the dynamics engine, and flex joints run on box3d".into())
        } else {
            Ok(Engine::Mech(MechWorld::new(asm, gravity, floor)))
        }
    }
}

fn snapshot(w: &mut Engine, asm: &Assembly, frame: u64, commands: u64, running: bool, speed: f64) -> Snapshot {
    Snapshot {
        commands,
        frame,
        time: on!(w, w => w.time()),
        running,
        speed,
        poses: on!(w, w => w.poses()),
        rest: (0..asm.bodies.len()).map(|i| on!(&*w, w => w.rest_com(i))).collect(),
        angles: on!(w, w => w.angles(asm)),
        grabbed: on!(w, w => w.grabbed()),
        pinned: on!(w, w => w.pinned()),
        frozen: on!(w, w => w.frozen()),
        error: None,
        resets: w.resets(),
        strings: w.strings(),
        motors: w.motors(),
        efforts: w.efforts(),
        loads: w.loads(),
    }
}

/// Where the sim publishes: the newest snapshot, and the news flag that wakes the UI once per news
/// it has not taken.
struct Outbox {
    latest: Arc<Mutex<Arc<Snapshot>>>,
    news: Arc<AtomicBool>,
    wake: Box<dyn Fn() + Send>,
}

impl Outbox {
    fn store(&self, snap: Arc<Snapshot>) {
        if let Ok(mut l) = self.latest.lock() {
            *l = snap;
        }
        // after the store: a UI that took the news before it either reads this snapshot or is woken for it
        if !self.news.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

/// Publish that the sim stopped because its world could not be built.
fn failed(outbox: &Outbox, error: String, commands: u64) {
    let Ok(last) = outbox.latest.lock().map(|l| l.clone()) else { return };
    outbox.store(Arc::new(Snapshot { frame: last.frame + 1, commands, running: false, error: Some(error), ..(*last).clone() }));
}

impl Runner {
    /// Start the sim thread, paused at the print pose. The sim calls `wake` (on its thread) when it
    /// publishes news the UI has not taken yet: an app passes its signal to the UI thread.
    pub fn start(asm: Assembly, gravity: P3, wake: impl Fn() + Send + 'static) -> Runner {
        Self::start_with_floor(asm, gravity, None, None, wake)
    }

    /// [`Runner::start`] with a floor across gravity (`MechWorld::new`'s level), for a piece that rests on the bed,
    /// and the app's workpool for the dynamics engine's parallel work, which the sim thread helps with.
    pub fn start_with_floor(asm: Assembly, gravity: P3, floor: Option<f64>, pool: Option<Arc<dyn TaskPool>>, wake: impl Fn() + Send + 'static) -> Runner {
        let (tx, rx) = channel();
        // The print pose is known without constructing collision hulls on the caller's UI thread.
        // The physics world is built once, by its owning worker.
        let first = Arc::new(Snapshot {
            commands: 0,
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
            error: None,
            resets: Vec::new(),
            strings: asm.strings.iter().map(|s| s.printed()).collect(),
            motors: asm.joints.iter().enumerate().filter(|(_, j)| j.motor.is_some()).map(|(k, _)| (k, 0.0)).collect(),
            efforts: Vec::new(),
            loads: Vec::new(),
        });
        let latest = Arc::new(Mutex::new(first.clone()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        // the first snapshot is news: the UI shows the print pose before the sim answers
        let news = Arc::new(AtomicBool::new(true));
        let thread = {
            let outbox = Outbox { latest: latest.clone(), news: news.clone(), wake: Box::new(wake) };
            let (events, stop) = (events.clone(), stop.clone());
            std::thread::Builder::new().name("mech-sim".into()).spawn(move || {
                match Engine::new(&asm, gravity, floor) {
                    Ok(mut w) => {
                        w.bind_pool(&pool);
                        run(asm, gravity, floor, pool, w, rx, outbox, events, stop)
                    }
                    Err(e) => failed(&outbox, e, 0),
                }
            }).ok()
        };
        Runner { tx, sent: AtomicU64::new(0), latest, news, events, held: first, stop, thread }
    }

    pub fn send(&self, c: Command) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(c);
    }

    /// The newest snapshot has taken up every command sent, or no sim is left to. Building the
    /// sim's world comes first and can take a long while (a big assembly, a cold start), so a
    /// view that waits for an answer waits for this, not for a number of frames.
    pub fn answered(&mut self) -> bool {
        self.latest().commands == self.sent.load(Ordering::Relaxed) || self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Whether the sim published since the news was last taken, taking it: a view going idle takes
    /// it (news it has not shown is one more frame), and whatever the sim publishes after wakes it.
    /// A view that keeps asking for frames need not take it, and is not woken.
    pub fn take_news(&self) -> bool {
        self.news.swap(false, Ordering::AcqRel)
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

fn run(mut asm: Assembly, gravity: P3, floor: Option<f64>, pool: Option<Arc<dyn TaskPool>>, mut w: Engine, rx: Receiver<Command>, outbox: Outbox, events: Arc<Mutex<Vec<Event>>>, stop: Arc<AtomicBool>) {
    let dt = 1.0 / HZ;
    let mut running = false;
    let mut speed = 1.0;
    let mut gravity = gravity;
    let mut shake = false;
    let mut shaking = 0.0;
    let mut settling = false;
    let mut lift = 0.0;
    // Where the held ground rests (a hand that carried a held part left it there) and whether Hang lifts it.
    let mut base = [0.0; 3];
    let mut hanging = false;
    let strung = !asm.strings.is_empty() && asm.anchors.iter().any(|a| a.kind == AnchorKind::Held);
    let mut frame = 0u64;
    let mut commands = 0u64;
    let mut owed = 0.0;
    let mut last = Instant::now();
    let mut at_limit = vec![false; asm.joints.len()];
    let mut pending: Vec<Event> = Vec::new();
    let mut drag_target: Option<P3> = None;
    let mut carry: Option<(P3, P3, f64)> = None;
    let mut following = false;
    let mut behind = 0.0;
    let mut behind_logged = Instant::now();
    let links = asm.bodies.iter().filter(|b| !b.tubes.is_empty()).count();
    let mut weave_steps = (links > 0 && matches!(w, Engine::Dynamics(_))).then(|| WeaveSteps { links, ms: Vec::new(), over: false, warm: false, logged: false });
    while !stop.load(Ordering::Relaxed) {
        let mut single = false;
        let mut changed = false;
        while let Ok(c) = rx.try_recv() {
            changed = true;
            commands += 1;
            match c {
                Command::Play => running = true,
                Command::Pause => running = false,
                Command::Reset => {
                    w = match Engine::new(&asm, gravity, floor) {
                        Ok(w) => w,
                        Err(e) => return failed(&outbox, e, commands),
                    };
                    w.bind_pool(&pool);
                    running = true;
                    owed = 0.0;
                    at_limit.iter_mut().for_each(|a| *a = false);
                    shake = false;
                    shaking = 0.0;
                    settling = false;
                    lift = 0.0;
                    base = [0.0; 3];
                    hanging = false;
                    carry = None;
                    following = false;
                    drag_target = None;
                }
                Command::Speed(s) => speed = s.clamp(0.01, 4.0),
                Command::Gravity(g) => {
                    gravity = g;
                    on!(&mut w, w => w.set_gravity(g));
                }
                Command::Grab { body, point, hz } => {
                    if body < asm.bodies.len() {
                        hanging = false;
                        on!(&mut w, w => w.grab(body, point, hz));
                        drag_target = Some(point);
                    }
                }
                Command::Pin { body, point } => {
                    if body < asm.bodies.len() {
                        on!(&mut w, w => w.pin(body, point));
                    }
                }
                Command::Dangle { body, point } => {
                    if body < asm.bodies.len() {
                        on!(&mut w, w => w.dangle(body, point));
                        drag_target = Some(point);
                    }
                }
                Command::Poke { body, point, dv } => {
                    if body < asm.bodies.len() {
                        on!(&mut w, w => w.poke(body, point, dv));
                    }
                }
                Command::Drag { point } => drag_target = Some(point),
                Command::Release { velocity } => {
                    // a held part stays where the hand left it
                    if let Engine::Dynamics(d) = &w {
                        if d.carrying() {
                            base = sub(d.ground_at(), scale(norm(asm.up), lift));
                        }
                    }
                    if let Some(v) = velocity {
                        on!(&mut w, w => w.throw(v));
                    }
                    on!(&mut w, w => w.release());
                    drag_target = None;
                }
                Command::Shake(on) => shake = on,
                Command::Carry { at, axis, angle } => {
                    carry = Some((at, axis, angle));
                    following = true;
                }
                Command::Step => single = true,
                // Hang on a strung piece lifts it onto its strings, as a puppeteer lifts the control: nothing is let go.
                Command::Bodies { action: BodyAction::Hang, .. } if strung => {
                    hanging = true;
                    if let Engine::Dynamics(d) = &mut w {
                        d.wake_all();
                    }
                }
                Command::Bodies { bodies, action } => {
                    hanging = false;
                    on!(&mut w, w => w.apply(&bodies, &action));
                }
                Command::Motor { joint, target } => {
                    if let Engine::Dynamics(d) = &mut w {
                        d.set_motor(joint, target);
                    }
                }
                Command::Limits(limits) => {
                    let n = asm.joints.len();
                    for (j, kind) in limits.into_iter().filter(|(j, _)| *j < n) {
                        on!(&mut w, w => w.set_limits(j, &kind));
                        asm.joints[j].kind = kind;
                    }
                }
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
                // Through the engine's log handler, which the app points at its log.
                makepad_dynamics::util_errmem::warning_text(&format!("mech-sim: steps cost more than real time; {:.0} ms of sim time slowed down", behind * 1e3));
                behind = 0.0;
                behind_logged = Instant::now();
            }
        }
        if single && !running {
            steps = 1;
        }
        for _ in 0..steps {
            if let Some(p) = drag_target {
                on!(&mut w, w => w.drag(p, dt));
            }
            let lift_to = if on!(&w, w => w.time()) >= LIFT_AFTER { asm.lift } else { 0.0 };
            let lifting = carry.is_none() && lift != lift_to;
            let rising = match (&w, floor) {
                (Engine::Dynamics(d), Some(f)) => hanging && d.hangs_below(f + HANG_CLEAR),
                _ => false,
            };
            if rising {
                base = madd(base, norm(asm.up), LIFT_SPEED * dt);
                if let Engine::Dynamics(d) = &mut w {
                    d.wake_all();
                }
            }
            if following || shake || shaking > 0.0 || settling || lifting || rising {
                // It eases in and out, and the last move leaves the held ground at rest.
                settling = following || shaking > 0.0 || lifting || rising;
                shaking = (shaking + if shake { dt } else { -dt } / SHAKE_EASE).clamp(0.0, 1.0);
                if lifting {
                    lift += (lift_to - lift).clamp(-LIFT_SPEED * dt, LIFT_SPEED * dt);
                }
                let t = on!(&w, w => w.time());
                let up = norm(asm.up);
                let side = basis(up).0;
                let tempo = if asm.shake_tempo > 0.0 { asm.shake_tempo } else { 1.0 };
                let ang = shaking * 18f64.to_radians() * (std::f64::consts::TAU * 1.6 * t).sin();
                let by = add(scale(side, shaking * 12.0 * (std::f64::consts::TAU * 2.2 * tempo * t).sin()), sub(asm.shake_about, rotate(asm.shake_about, up, ang)));
                match carry {
                    Some(hand) => following = on!(&mut w, w => w.carry_ground(hand, (by, up, ang), CARRY_EASE, dt)),
                    None => on!(&mut w, w => w.move_ground(add(base, add(by, scale(up, lift))), up, ang, dt)),
                }
            }
            let started = Instant::now();
            w.step(dt);
            if let Some(c) = &mut weave_steps {
                c.add(started.elapsed().as_secs_f64() * 1e3);
            }
            frame += 1;
            for (a, b, speed) in on!(&w, w => w.hits()) {
                if speed > 30.0 {
                    pending.push(Event::Hit { a, b, speed });
                }
            }
            let ang = on!(&mut w, w => w.angles(&asm));
            for (j, js) in asm.joints.iter().enumerate() {
                let near = js.kind.near_limit(ang[j]);
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
            outbox.store(Arc::new(snapshot(&mut w, &asm, frame, commands, running, speed)));
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
