//! `devhub live`: a long-running app on a box that edits hot-patch into, so an agent's loop is
//! edit → patch → first result in tens of ms with no relaunch. Built on jobs: a live job is
//! placed like one (the worktree's snapshot shipped on top of its base commits), then holds its
//! box with the app running under the JIT (headless, or visible on a screen box). Every file the
//! master's monitor sees saved in the worktree is shipped to the box as it comes (debounced
//! 30 ms; `live sync` ships changes since the last ship), the box's JIT patches it live, and the job
//! records the outcome: patched, a restart needed or a refusal (with the reason), and the time from
//! the save to the patch and to the first frame after it. `live act` runs any inspect command in
//! the app (ui, key, click, grab, test hooks); `live wait patched|verdict|settled|frame` blocks on the
//! JIT. A parked panic pauses the live job (act and resume through it). It ends on release, when
//! its app exits, after 10 min with no edit or call (a `devhub inspect` of its app is a call; a
//! visible one: nor a person's input at its box), when its lane starts a fourth headless one (the least recently used idle headless one
//! goes; with all of them busy the new one waits for a slot: a visible session is the person's app
//! and never counts), when its lane landed, or to give a waiting whole/timed lease its box once the
//! current call answers. A call (a test session's tests) past three times its class's measured run
//! is said, never ended: an inbox warning, a why-hung sample in `devhub job result <lease>`, and
//! the test's still-running line. Tests keep one job per lane, target and args;
//! their saves only queue, and a test call ships and waits for that patch's test group.
//! Between calls a session is charged only its resident share of its box (one core and its
//! memory); a call takes its full charge back, admitted like a lease (it waits up to 30 s for
//! room, then runs).

use super::leg::{Held, Spec};
use super::Ctx;
use crate::cli::ask;
use crate::lease::{CostClass, Kind, Priority, Src};
use crate::proto::{wall_ms, Record, Sev};
use crate::state::{JobPanic, Live, LivePatch};
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A live job with no edit or call this long ends (tests too: an idle session's box waits for it).
pub const IDLE_MS: u64 = 10 * 60 * 1000;
/// Headless live sessions a lane keeps at once: a new one ends the lane's least recently used idle
/// one, or waits for a slot ([`slot`]; visible ones never count).
pub const LANE_SESSIONS: usize = 3;
/// How long a session has gone unused at `now`: since its last edit or call, and a visible one also
/// since a person's last input at its box (`input_ms`, [`crate::lease::sched::Sched::user_input_ms`]):
/// a visible session is the person's app, in use while they work at its box (the idle end closed
/// a person's visible Fab, 11-09).
fn unused_ms(x: &Live, input_ms: Option<u64>, now: u64) -> u64 {
    let last = if x.visible { x.touched_ms.max(input_ms.unwrap_or(0)) } else { x.touched_ms };
    now.saturating_sub(last)
}

/// Whether a lane's session may ask for its box ([`slot`]).
#[derive(Debug, PartialEq)]
enum Slot<'a> {
    /// the lane has room
    Free,
    /// it takes the place of this one, the lane's least recently used idle headless session
    Takes(&'a Live),
    /// the lane's headless sessions are all busy, or an earlier one waits: it waits for a slot
    Waits,
}

/// Whether a session of a lane (`mine`: the lane's other active sessions; `own`: the session when it
/// waits already, None for a new one) may ask for its box. At most [`LANE_SESSIONS`] headless sessions
/// per lane hold a slot: past them the least recently used idle one goes, and with all of them busy
/// the session waits for one, like a lease, first come first served (a lane's fourth session was
/// refused, 11-09: a bound waits, it never refuses). A waiting session holds no slot. A visible
/// session is the person's app: it never counts, never waits and is never the one ended (the cap
/// ended a person's visible Fab for its lane's next session, 11-09).
fn slot<'a>(mine: &'a [Live], own: Option<&Live>, visible: bool, idle: impl Fn(&Live) -> bool) -> Slot<'a> {
    if visible { return Slot::Free; }
    let key = |x: &Live| (x.slot_wait_ms, x.id.len(), x.id.clone());
    if mine.iter().any(|x| x.slot_wait_ms != 0 && own.is_none_or(|o| key(x) < key(o))) { return Slot::Waits; }
    let counted: Vec<&Live> = mine.iter().filter(|x| !x.visible && x.slot_wait_ms == 0).collect();
    if counted.len() < LANE_SESSIONS { return Slot::Free; }
    counted.into_iter().filter(|x| idle(x)).min_by_key(|x| x.touched_ms).map_or(Slot::Waits, Slot::Takes)
}

/// No call runs in the session and no edit is on its way to it (one still placed has neither).
fn idle_now(x: &Live) -> bool {
    running(&x.id).is_none_or(|r| r.calls.lock().unwrap().count == 0 && !r.inflight.load(std::sync::atomic::Ordering::SeqCst))
}

/// A command routed to an app on `node` (`spec`: its pid or name, as `devhub inspect <node>/<spec>`
/// names it) is a call of the live session that runs it, as `live act` is: a lane that drove its
/// session's app through `devhub inspect` (`ui find`, its AI tools) lost it to the idle end (11-09).
/// `caller`: the routing link's name ([`makepad_remote::hub::RouteSeen`]).
pub fn app_routed(node: &str, spec: &str, service: &str, caller: &str) {
    if !a_lanes_call(service, caller) { return; }
    let Some(ctx) = super::ctx() else { return };
    let ids: Vec<String> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active() && names_app(x, node, spec)).map(|x| x.id.clone()).collect());
    for id in ids {
        let _ = try_set(ctx, &id, |s| s.touched_ms = wall_ms());
    }
}

/// Whether a route to an app is a lane's call of it: not a watch or stream, which only follow the
/// app, and not devhub's own look at it (its `devhub-*` clients: a session's start, its look's checks),
/// which kept no session in use.
fn a_lanes_call(service: &str, caller: &str) -> bool {
    !matches!(service, "watch" | "stream") && !caller.starts_with("devhub-")
}

/// `node`/`spec` names the session's app: its box, and its JIT's pid, its package or the registry
/// name it was started by.
fn names_app(x: &Live, node: &str, spec: &str) -> bool {
    x.node == node && ((x.pid > 0 && x.pid.to_string() == spec) || x.app == spec || makepad_app_registry::find(spec).is_some_and(|e| e.package == x.app))
}

/// What a session that waits for a slot of `lane` says while it waits.
fn slot_note(lane: &str) -> String {
    format!("waits for a slot: lane {lane}'s {LANE_SESSIONS} headless live sessions are busy (or an earlier one waits); it asks for its box when one is idle or ends")
}

/// A session that waits for its lane's slot ([`slot`]) has one once one of its lane's headless
/// sessions is idle (it takes that one's place) or ends, and earlier waiters have theirs. False: it
/// ended meanwhile.
fn wait_slot(ctx: &Ctx, id: &str) -> bool {
    loop {
        let (signal, seen) = {
            let _starting = STARTING.lock().unwrap();
            let Some(x) = get(ctx, id).filter(|x| x.active()) else { return false };
            if x.slot_wait_ms == 0 { return true; }
            let mine: Vec<Live> = ctx.store.read(|d| d.lives.iter().filter(|o| o.active() && o.lane == x.lane && o.id != id).cloned().collect());
            let free = match slot(&mine, Some(&x), x.visible, idle_now) {
                Slot::Waits => false,
                Slot::Free => true,
                Slot::Takes(lru) => {
                    event(ctx, Sev::Info, format!("live {} ({}) of lane {}: released for its lane's waiting session {id} (at most {LANE_SESSIONS} headless per lane)", lru.id, lru.app, x.lane), Some(lru));
                    release(ctx, &lru.id, &format!("released: lane {}'s waiting session {id} took its place (at most {LANE_SESSIONS} headless per lane)", x.lane)).is_ok()
                }
            };
            if free {
                let waited = wall_ms().saturating_sub(x.slot_wait_ms) / 1000;
                let x = set(ctx, id, |s| { s.slot_wait_ms = 0; s.note.clear(); });
                event(ctx, Sev::Info, format!("live {id}: has its lane's slot after {waited} s: asks for a box"), x.as_ref());
                return true;
            }
            // a lane session's change (one ending, this one released) wakes it; one going idle, the next look
            let ids: Vec<&str> = mine.iter().map(|o| o.id.as_str()).chain(std::iter::once(id)).collect();
            let signal = ctx.store.subscribe("live", &ids);
            let seen = signal.generation();
            (signal, seen)
        };
        signal.wait(seen, HOUSEKEEPING);
    }
}

/// How often live jobs are checked for a lane that is gone (a lease beat).
const ORPHAN_LOOK: Duration = Duration::from_secs(30);
/// The ship loop's look at idle and orphaned jobs while no save or ended call wakes it.
const HOUSEKEEPING: Duration = Duration::from_secs(1);

/// A running live job's box and the edits waiting to go.
struct Running {
    held: Held,
    node: String,
    lease: String,
    tree: PathBuf,
    test: bool,
    pending: Pending,
    /// an edit is on its way (shipped, its verdict not in yet)
    inflight: std::sync::atomic::AtomicBool,
    /// Admission and parking share this lock; no call may slip in after the idle check.
    calls: Mutex<Calls>,
    restart: std::sync::atomic::AtomicBool,
    resuming: std::sync::atomic::AtomicBool,
    fault: Mutex<Option<String>>,
    /// the run of the app seen up ([`wait_up`]): an edit's ship asks nothing more
    up_pid: std::sync::atomic::AtomicU32,
}

static RUNNING: Mutex<Option<HashMap<String, Arc<Running>>>> = Mutex::new(None);
/// Wakes the ship loop: a save came, or an edit's call ended (its box takes the next save). The
/// loop shipped a save after a 30 ms quiet window and a 10 ms poll: a third of a design edit's
/// save -> frame (11-04). macOS's file events come at once for a save and 100 ms later for any
/// write after it, so the window joined no save that the first event did not already carry.
static SHIP_WAKE: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(false), std::sync::Condvar::new());

fn wake_ship() {
    *SHIP_WAKE.0.lock().unwrap() = true;
    SHIP_WAKE.1.notify_one();
}

static STARTING: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct Calls {
    count: usize,
    park: Option<String>,
    ended: bool,
}

impl Calls {
    fn ready_to_park(&mut self, inflight: bool) -> Option<String> {
        if self.ended || self.count > 0 || inflight { return None; }
        let note = self.park.clone()?;
        self.ended = true;
        Some(note)
    }
}

struct Inflight<'a>(&'a Ctx, &'a str, &'a Running);
impl Drop for Inflight<'_> {
    fn drop(&mut self) { finish_call(self.0, self.1, self.2); }
}

/// A call's hold on its session, and what it waited for room for, if it waited: its answer says so.
struct Call<'a>(&'a Ctx, &'a str, Arc<Running>, Option<String>);
impl Drop for Call<'_> {
    fn drop(&mut self) {
        self.2.calls.lock().unwrap().count -= 1;
        idle_charge(self.0, &self.2);
        park_ready(self.0, self.1, &self.2);
    }
}

fn call<'a>(ctx: &'a Ctx, id: &'a str) -> Result<Option<Call<'a>>, String> {
    let Some(r) = running(id) else { return Ok(None) };
    {
        let mut calls = r.calls.lock().unwrap();
        if let Some(note) = &calls.park { return Err(format!("{id}: {note}")); }
        calls.count += 1;
    }
    let mut call = Call(ctx, id, r, None);
    call.3 = admit(ctx, id, &call.2, makepad_remote::lines::asked_on())?;
    Ok(Some(call))
}

fn begin_call(r: &Running) -> bool {
    let calls = r.calls.lock().unwrap();
    calls.park.is_none() && !r.inflight.swap(true, std::sync::atomic::Ordering::SeqCst)
}

fn finish_call(ctx: &Ctx, id: &str, r: &Running) {
    {
        let _calls = r.calls.lock().unwrap();
        r.inflight.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    wake_ship();
    idle_charge(ctx, r);
    park_ready(ctx, id, r);
    ctx.store.notify("live", id);
}

/// How often a call that finds no room on its box looks again.
const ADMIT_LOOK: Duration = Duration::from_millis(100);

/// A wait for room this long is a warning: the inbox and the history have it, not only the event
/// stream (a ship waited 220 s for its box's RAM, said only as info, 11-06).
const ROOM_WARN: Duration = Duration::from_secs(30);
/// How often a call that still waits for room says what it waits on now.
const ROOM_SAY_AGAIN: Duration = Duration::from_secs(60);

/// A call takes its session's full charge back, admitted like a lease: it runs once that fits its
/// box, never past the box's cores. Taken anyway (an edit at once, a call after 30 s), calls kept
/// a box over its cores 30% of an hour (11-05). Its box keeps room for one call of its largest
/// idle session (`Sched::call_room`): a call waits beside another call, never for work past its
/// charge, which yields to it.
/// A wait says so as it starts (`live call <id> waits for room on <node>: waiting for N cores (..),
/// held by ..: <why>`), to its caller too (a lane's `live act` sat silent for 6 min, 11-09), each
/// [`ROOM_SAY_AGAIN`] while it lasts, and once it ends (`.. waited <ms> ms ..`, a warning past
/// [`ROOM_WARN`]), and the call's reply carries that line ([`with_room`]). No clock ends it: its session
/// parking or ending does, or the client it answers going (`client`: the request's connection; an
/// edit's ship answers none, its session's state alone ends its wait).
fn admit(ctx: &Ctx, id: &str, r: &Running, client: Option<makepad_remote::Conn>) -> Result<Option<String>, String> {
    let t = Instant::now();
    let ended = || {
        if let Some(note) = &r.calls.lock().unwrap().park { return Some(format!("{id}: {note}")); }
        if get(ctx, id).is_none_or(|x| !x.active()) { return Some(format!("{id}: the session ended while its call waited for room")); }
        client.as_ref().filter(|c| c.is_closed()).map(|_| format!("{id}: this call's client has gone"))
    };
    let waited = wait_for_room(&mut || ctx.leases.live_admit(&r.lease), &ended, ROOM_SAY_AGAIN, &mut |why| {
        let line = format!("live call {id} waits for room on {}: {why}", r.node);
        makepad_remote::lines::say_waiting(&line);
        event(ctx, Sev::Info, line, None);
    }).inspect_err(|_| ctx.leases.live_unwait(&r.lease))?;
    // (said to the caller too: the event reaches only the stream, and a lane's call waited
    // 265 s on a Mac without a word, 11-06)
    Ok(waited.map(|why| {
        makepad_remote::lines::wait_over();
        let waited = t.elapsed();
        let line = format!("live call {id} waited {} ms for room on {}: {why}", waited.as_millis(), r.node);
        event(ctx, if waited >= ROOM_WARN { Sev::Warn } else { Sev::Info }, line.clone(), get(ctx, id).as_ref());
        line
    }))
}

/// A call's JSON reply with its wait for room (`room`: [`admit`]'s line), if it waited: every
/// call's reply says it, as `live act`'s lines do (a look's, a wait's and a sync's said nothing:
/// a lane's look waited 220 s without a word, 11-06).
fn with_room(answer: String, room: Option<&str>) -> String {
    match (room, answer.strip_suffix('}')) {
        (Some(line), Some(head)) => format!("{head}{}\"room\":{}}}", if head.ends_with('{') { "" } else { "," }, json_str(line)),
        _ => answer,
    }
}

fn room_of<'a>(held: &'a Option<Call<'_>>) -> Option<&'a str> {
    held.as_ref().and_then(|c| c.3.as_deref())
}

/// [`admit`]'s wait: `try_admit` until it lets the call in (Ok: the first refusal, if it waited),
/// `ended` saying why the wait ends otherwise (Err), `waits` told the first refusal at once and the
/// refusal then each `again`.
fn wait_for_room(try_admit: &mut dyn FnMut() -> Result<(), String>, ended: &dyn Fn() -> Option<String>, again: Duration, waits: &mut dyn FnMut(&str)) -> Result<Option<String>, String> {
    let mut first: Option<String> = None;
    let mut said = Instant::now();
    loop {
        match try_admit() {
            Ok(()) => return Ok(first),
            Err(why) => {
                if first.is_none() || said.elapsed() >= again {
                    waits(&why);
                    said = Instant::now();
                }
                first.get_or_insert(why);
            }
        }
        if let Some(why) = ended() { return Err(first.map_or(why.clone(), |f| format!("{why}; it waited for {f}"))); }
        std::thread::sleep(ADMIT_LOOK);
    }
}

/// No call runs and no edit is on its way: the session's box charges it its resident share.
fn idle_charge(ctx: &Ctx, r: &Running) {
    let idle = r.calls.lock().unwrap().count == 0 && !r.inflight.load(std::sync::atomic::Ordering::SeqCst);
    if idle {
        ctx.leases.live_idle(&r.lease);
    }
}

/// Why a call finds no running session: an ended one says how it ended (a parked one, with the
/// command that starts it again).
fn not_running(ctx: &Ctx, id: &str) -> String {
    match get(ctx, id) {
        Some(x) if !x.active() && !x.note.is_empty() => format!("{id} {}: {}", x.state, x.note),
        _ => format!("no running live job {id}"),
    }
}

fn accepting(id: &str) -> bool {
    running(id).is_none_or(|r| r.calls.lock().unwrap().park.is_none())
}

/// Called by the lease master only when all blockers of the best box are live jobs.
pub fn park(ctx: &Ctx, leases: &[String], note: &str) {
    let list: Vec<_> = RUNNING.lock().unwrap().as_ref().map(|m| m.iter().filter(|(_, r)| leases.contains(&r.lease)).map(|(id, r)| (id.clone(), r.clone())).collect()).unwrap_or_default();
    for (id, r) in list {
        r.calls.lock().unwrap().park.get_or_insert_with(|| note.to_string());
        park_ready(ctx, &id, &r);
    }
}

fn park_ready(ctx: &Ctx, id: &str, r: &Running) {
    let note = {
        let mut calls = r.calls.lock().unwrap();
        let Some(note) = calls.ready_to_park(r.inflight.load(std::sync::atomic::Ordering::SeqCst)) else { return };
        note
    };
    // the one line its lane gets (its next call on it answers with it; the inbox has it too): why
    // it ended, and the one command that starts it again
    let note = format!("{note}; `devhub live again {id}` starts it again");
    if let Some(x) = set(ctx, id, |s| { s.state = "ended".into(); s.note = note.clone(); s.ended_ms = wall_ms(); }) {
        ctx.store.event(if note.starts_with("parked for ") { "live_parked" } else { "live" }, Sev::Info, format!("live {id} ({}) of lane {}: {note}", x.app, x.lane), Some(crate::state::live_value(&x).to_json()));
    }
    ctx.leases.release_live(&r.lease);
}

/// A session joins the running ones (started, or reattached after a master restart): from now its
/// tree's saves queue for its box. The monitor reports saves only in the trees it follows, so it
/// follows this one: a session reattached after a master restart was followed by nothing, and no
/// save of it ever shipped, without a word (11-03). An app session's ship is due at once: what was
/// saved while no session listened (its box being placed, the master restarting) goes as soon as
/// its app runs; with nothing new, it ships nothing.
fn join(monitor: &crate::monitor::Monitor, id: &str, r: Arc<Running>) {
    follow(monitor, &r.tree);
    RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.to_string(), r.clone());
    if !r.test { due(&r.pending, []); }
}

/// The monitor follows `tree` and each nested repository's tree in it ([`crate::nested`]).
fn follow(monitor: &crate::monitor::Monitor, tree: &Path) {
    let _ = monitor.dirty(tree);
    for name in crate::nested::names(tree) {
        let _ = monitor.dirty(&tree.join(crate::nested::dir(&name)));
    }
}

/// A save the monitor heard in `root` at `rel`, as a path of the session's `tree`: its own files,
/// and each nested repository's from that repository's tree (`<tree>/apps/<name>`; the makepad
/// tree's event for a nested path is that tree's to say). None: not this session's.
fn saved_path(tree: &Path, root: &Path, rel: &str) -> Option<String> {
    if root == tree {
        let nested = rel.starts_with("apps/") && crate::nested::split(rel, crate::nested::names(tree).iter().map(String::as_str)).is_some();
        return (!nested).then(|| rel.to_string());
    }
    let mut parts = root.strip_prefix(tree).ok()?.components().map(|c| c.as_os_str().to_str());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Some("apps")), Some(Some(name)), None) if crate::nested::valid(name) => Some(format!("{}{rel}", crate::nested::prefix(name))),
        _ => None,
    }
}

/// The dirty paths of each nested repository in `tree`, as tree paths.
fn nested_dirty(ctx: &Ctx, tree: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for name in crate::nested::names(tree) {
        let (d, prefix) = (ctx.monitor.dirty(&tree.join(crate::nested::dir(&name)))?, crate::nested::prefix(&name));
        files.extend(d.changed.into_iter().chain(d.deleted).map(|p| format!("{prefix}{p}")));
    }
    Ok(files)
}

/// A session's saves: (paths in the tree, when the first was saved, when the last was).
type Pending = Mutex<(BTreeSet<String>, u64, Option<Instant>)>;

/// A ship of the session's tree is due, with the saved `paths` (the ship loop sends what differs
/// from its sources, at once).
fn due(pending: &Pending, paths: impl IntoIterator<Item = String>) {
    {
        let mut p = pending.lock().unwrap();
        if p.0.is_empty() { p.1 = wall_ms(); }
        p.0.extend(paths);
        p.2 = Some(Instant::now());
    }
    wake_ship();
}

fn running(id: &str) -> Option<Arc<Running>> {
    RUNNING.lock().unwrap().as_ref().and_then(|m| m.get(id).cloned())
}

fn event(ctx: &Ctx, sev: Sev, line: String, x: Option<&Live>) {
    ctx.store.event("live", sev, line, x.map(|x| crate::state::live_value(x).to_json()));
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Live)) -> Option<Live> {
    try_set(ctx, id, f).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); None })
}

fn try_set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Live)) -> Result<Option<Live>, String> {
    ctx.store.update(|d| {
        let x = d.lives.iter_mut().find(|x| x.id == id)?;
        f(x);
        Some(x.clone())
    })
}

fn get(ctx: &Ctx, id: &str) -> Option<Live> {
    ctx.store.read(|d| d.lives.iter().find(|x| x.id == id).cloned())
}

fn set_patch(ctx: &Ctx, id: &str, p: &LivePatch) -> Option<Live> {
    let x = set(ctx, id, |s| {
        if s.patch.as_ref().is_none_or(|old| old.edit_ms <= p.edit_ms) { s.patch = Some(p.clone()); }
    }).filter(|x| x.patch.as_ref().is_some_and(|new| new.edit_ms == p.edit_ms));
    if let Some(x) = &x { super::live_shadow::sample(ctx, x); }
    x
}

fn set_test_patch(ctx: &Ctx, id: &str, run: &Live, p: &LivePatch) -> Option<Live> {
    let (mut saved, mut first_verdict) = (false, false);
    let x = set(ctx, id, |s| {
        if s.run_n == run.run_n && s.pid == run.pid && s.state == "running" && s.patch.as_ref().is_some_and(|old| old.edit_ms == p.edit_ms) {
            first_verdict = p.verdict_ms != 0 && s.patch.as_ref().is_some_and(|old| old.verdict_ms == 0);
            s.patch = Some(p.clone());
            saved = true;
        }
    });
    let x = x.filter(|_| saved);
    // an agent's wait for its verdict: shipped (sync), patched (JIT), tests (run); from its call
    // when that came after the save (a test session ships on its call: the time between was its own)
    if first_verdict && p.edit_ms > 0 {
        let from = p.edit_ms.max(p.called_ms).min(p.verdict_ms);
        let at = |t: u64| if t == 0 { p.verdict_ms } else { t.clamp(from, p.verdict_ms) };
        let (shipped, patched) = (at(p.shipped_ms), at(p.outcome_ms).max(at(p.shipped_ms)));
        // the tests' own time is run: a session's first verdict has no patch, and its minutes of
        // tests read as JIT (a start's "jit" 140 s was 135 s of tests, 4 s to main, 11-02)
        let tests_ms = p.tests.as_ref().map_or(0, |t| t.ms.max(0.0) as u64);
        let patched = patched.min(p.verdict_ms.saturating_sub(tests_ms)).max(shipped);
        crate::lease::prof::log_wait(&ctx.home, &format!("{{\"kind\":\"live\",\"lane\":{},\"id\":{},\"node\":{},\"start_ms\":{},\"end_ms\":{},\"sync_ms\":{},\"jit_ms\":{},\"run_ms\":{}}}", json_str(&run.lane), json_str(id), json_str(&run.node), from, p.verdict_ms, shipped - from, patched - shipped, p.verdict_ms.saturating_sub(patched)));
    }
    if let Some(x) = &x { super::live_shadow::sample(ctx, x); }
    x
}

fn wait_tests(ctx: &Ctx, id: &str) {
    let signal = ctx.store.subscribe("live", &[id]);
    loop {
        let seen = signal.generation();
        if !ctx.store.read(|d| d.lives.iter().find(|x| x.id == id).is_some_and(|s| s.state == "running" && s.patch.as_ref().is_some_and(|p| !terminal(p)))) { return; }
        signal.wait(seen, Duration::from_secs(30));
    }
}

fn ctl() -> makepad_remote::link::Client {
    makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-live")
}

/// One inspect command in the live app: its answer lines. Its AI tools (`tools`, `call
/// <service>.<tool> ..`) are its `app` service's ([`makepad_remote::app::app_request_of`]).
fn inspect(node: &str, pid: u32, line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if let Some(request) = makepad_remote::app::app_request_of(line) {
        makepad_remote::hub::ask_app(&ctl(), &format!("{node}:{pid}"), &request?, &mut |l| {
            out.push(l.to_string());
            true
        })?;
        return Ok(out);
    }
    makepad_remote::hub::ask(&ctl(), &format!("{node}:{pid}"), "cmd", "", line, &mut |l| {
        out.push(l.to_string());
        true
    })?;
    Ok(out)
}

/// `live wait patched` is done: the newest edit saved before the call (the OS monitor hears it
/// within ~0.1 s, so `waited` 300 ms covers it) has its verdict; a session that has none after
/// that has no edit to wait for. Before, a session's first edit had no patch yet and `live look`
/// skipped the wait: its grab was the frame from before the edit, drawn with no patch (11-05).
fn patched_done(patch: Option<&LivePatch>, asked: u64, waited: Duration) -> bool {
    let heard = waited > Duration::from_millis(300);
    match patch {
        Some(p) => !p.outcome.is_empty() && (p.edit_ms.saturating_add(300) >= asked || heard),
        None => heard,
    }
}

/// A frame presented: the app is up and drawing.
fn drawing(x: &Live) -> bool {
    x.pid > 0 && inspect(&x.node, x.pid, "live").ok().and_then(|l| frames_of(&l)).is_some_and(|f| f > 0)
}

/// The app is up and its JIT idle.
fn settled(x: &Live) -> bool {
    x.pid > 0 && inspect(&x.node, x.pid, "jit ms=100").is_ok_and(|l| jit_idle(&l))
}

/// The answer of `jit` says idle: its activity line, after the `measured` header.
fn jit_idle(answer: &[String]) -> bool {
    answer.iter().any(|l| l.starts_with("idle"))
}

/// A save during an app's start waits for it to be up, then applies: its first frame, or, for an
/// app that draws nothing or is parked on a compile error only an edit can fix, a JIT gone idle.
/// A patch in the middle of a boot gains nothing, and a save then ended the session (12 min of
/// native build lost, 11-04). False: that run of the app ended meanwhile.
fn wait_up(ctx: &Ctx, id: &str, r: &Running, pid: u32) -> bool {
    let mut quiet = 0;
    loop {
        let Some(x) = get(ctx, id).filter(|x| x.active() && x.pid == pid) else { return false };
        // (two quiet looks: one short window can fall between a boot's steps)
        quiet = if settled(&x) { quiet + 1 } else { 0 };
        if drawing(&x) || quiet >= 2 {
            r.up_pid.store(pid, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// [`inspect`] that `cut` cuts short (None then; the app's `watch` ends with the closed request).
fn inspect_until(node: &str, pid: u32, line: &str, cut: &dyn Fn() -> bool) -> Result<Option<Vec<String>>, String> {
    let mut out = Vec::new();
    let answered = makepad_remote::hub::ask_while(&ctl(), &format!("{node}:{pid}"), "cmd", "", line, &mut |l| {
        out.push(l.to_string());
        true
    }, &|| !cut())?;
    Ok(answered.map(|_| out))
}

/// A ship's error after which no edit reaches the session again: its lease is gone on its box, or
/// its run takes no edits (not a live run, or its live watcher ended).
fn session_over(e: &str, lease: &str) -> Option<String> {
    if e.contains(&format!("no lease {lease} ")) || e.ends_with(&format!("no lease {lease}")) {
        Some(format!("its lease {lease} is gone on its box"))
    } else if e.contains("this run watches no sources") {
        Some("its run takes no edits (not a live run, or its live watcher ended)".into())
    } else {
        None
    }
}

/// Whether a save that ships waits behind this edit's verdict (asked while waiting on it): an
/// edit waiting on its verdict never holds the next one back (a save waited 39 s behind an edit
/// the JIT never answered, 11-03). A file the session shipped before counts, and so does one git
/// sees changed; an ignored file, `local/` and the like never ship, so they cut no wait short.
/// The answer is kept while the waiting saves stay the same.
fn newer_saves<'a>(ctx: &'a Ctx, x: &'a Live, r: Option<&'a Running>) -> impl Fn() -> bool + 'a {
    let seen = std::cell::Cell::new((None::<Instant>, false));
    move || {
        let Some(r) = r else { return false };
        // (a test session's caller ships its saves itself)
        if r.test { return false; }
        let (pending, last): (Vec<String>, Option<Instant>) = {
            let p = r.pending.lock().unwrap();
            if p.0.is_empty() { return false; }
            if seen.get().0 == p.2 { return seen.get().1; }
            (p.0.iter().cloned().collect(), p.2)
        };
        // what the session holds now (this edit's ship among it)
        let sources = get(ctx, &x.id).map(|s| s.sources).unwrap_or_default();
        let ships = a_save_ships(&pending, &sources, |p| source_of(&r.tree, p).map(|s| s.0), || ctx.monitor.dirty(&r.tree));
        seen.set((last, ships));
        ships
    }
}

/// Whether one of the saved paths `pending` ships: a file of the session's overlay (`sources`)
/// whose bytes are not the shipped ones (`hash`: its [`source_of`] hash; unreadable: gone), or one
/// git sees changed or gone (`dirty`, asked only then; unreadable: it may ship). macOS reports a
/// save's rename again ~100 ms after the first event: that cut the edit's verdict wait short for
/// a ship of nothing (11-04).
fn a_save_ships(pending: &[String], sources: &BTreeMap<String, String>, hash: impl Fn(&str) -> Result<String, String>, dirty: impl FnOnce() -> Result<crate::monitor::Dirty, String>) -> bool {
    let (shipped, other): (Vec<&String>, Vec<&String>) = pending.iter().partition(|p| sources.contains_key(*p));
    shipped.iter().any(|p| hash(p).unwrap_or_else(|_| "-".into()) != sources[*p]) || !other.is_empty() && dirty().map_or(true, |d| {
        other.iter().any(|p| !crate::lease::ship::never_ships_named(p) && (d.changed.contains(*p) || d.deleted.contains(*p)))
    })
}

fn num(line: &str, k: &str) -> Option<u64> {
    makepad_strict_json::parse(line.as_bytes()).ok()?.get(k).and_then(crate::lease::num).map(|n| n as u64)
}

pub fn start(ctx: Arc<Ctx>) {
    // a previous master's live jobs go on: one whose lease this master took back (its own exec,
    // the same holder) keeps its app and is followed from where its log stands, and so does the
    // run it had asked for (the lease is noted once shipped): the node may be building its JIT
    // for it. Placed again, that lease went unbeaten until it expired and the build was lost
    // (a lease lost 160 s into a dev JIT build on its box, 11-03). Else it is placed again and its
    // app starts again.
    let lives: Vec<Live> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active()).cloned().collect());
    for x in lives {
        let c = ctx.clone();
        let held = !x.lease.is_empty() && ctx.leases.view().iter().any(|l| l.id == x.lease && !matches!(l.state, crate::lease::sched::State::Ended(_)));
        event(&ctx, Sev::Info, format!("live {} ({}): the master restarted; {}", x.id, x.app, if held { "its app runs on" } else { "placed again, its app starts again" }), Some(&x));
        if held {
            let _ = std::thread::Builder::new().name("devhub-live".into()).spawn(move || reattach(&c, &x));
        } else {
            set(&ctx, &x.id, |s| {
                s.state = "starting".into();
                s.pid = 0;
                s.lease.clear();
                s.run_n = 0;
                s.ask.clear();
                s.patch = None;
                s.touched_ms = wall_ms();
            });
            let id = x.id.clone();
            let _ = std::thread::Builder::new().name("devhub-live".into()).spawn(move || run(&c, &id, false));
        }
    }
    // saves in a live job's worktree queue for its box
    ctx.monitor.listen(Arc::new(|root: &Path, rel: &str| {
        let list: Vec<Arc<Running>> = RUNNING.lock().unwrap().as_ref().map(|m| m.values().cloned().collect()).unwrap_or_default();
        for r in list {
            let Some(path) = saved_path(&r.tree, root, rel) else { continue };
            due(&r.pending, [path]);
        }
    }));
    let c = ctx.clone();
    let mut looked = Instant::now();
    let _ = std::thread::Builder::new().name("devhub-live-ship".into()).spawn(move || loop {
        if looked.elapsed() >= ORPHAN_LOOK {
            looked = Instant::now();
            release_orphans(&c);
        }
        let list: Vec<(String, Arc<Running>)> = RUNNING.lock().unwrap().as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        for (id, r) in list {
            // Only the test caller ships its source state and owns the resulting verdict.
            if r.test { continue; }
            let due = {
                let mut p = r.pending.lock().unwrap();
                match p.2 {
                    Some(_) if begin_call(&r) => {
                        p.2 = None;
                        Some((std::mem::take(&mut p.0), p.1))
                    }
                    _ => None,
                }
            };
            if let Some((files, edit_ms)) = due {
                let (c2, id2) = (c.clone(), id.clone());
                let _ = std::thread::Builder::new().name("devhub-live-patch".into()).spawn(move || {
                    let _inflight = Inflight(&c2, &id2, &r);
                    let called = wall_ms();
                    // the box charges the session as active again before the edit runs: beside the
                    // ship, an edit ran past its box's cores (11-05). A session parking ships no more.
                    if admit(&c2, &id2, &r, None).is_err() { return; }
                    ship_edit(&c2, &id2, &r, files.into_iter().collect(), edit_ms, called, None, None);
                });
            }
        }
        // the idle timeout, from its placement on (a job still queued for a box is not idle)
        let now = wall_ms();
        let idle: Vec<Live> = c.store.read(|d| d.lives.iter().filter(|x| x.active() && !x.lease.is_empty() && now.saturating_sub(x.touched_ms) >= IDLE_MS).cloned().collect());
        for x in idle {
            if x.visible && unused_ms(&x, c.leases.user_input_ms(&x.node), now) < IDLE_MS { continue; }
            let Some(r) = running(&x.id) else { continue };
            // (a test session's saves wait for its next call: one within the timeout keeps it)
            if r.test && r.pending.lock().unwrap().2.is_some_and(|t| t.elapsed() < Duration::from_millis(IDLE_MS)) { continue; }
            {
                let mut calls = r.calls.lock().unwrap();
                if calls.count > 0 || r.inflight.load(std::sync::atomic::Ordering::SeqCst) { continue; }
                calls.park.get_or_insert_with(|| if x.visible { "ended after 10 min with no edit, call or person's input at its box" } else { "ended after 10 min with no edit or call" }.into());
            }
            park_ready(&c, &x.id, &r);
        }
        let mut woke = SHIP_WAKE.0.lock().unwrap();
        if !*woke { woke = SHIP_WAKE.1.wait_timeout(woke, HOUSEKEEPING).unwrap().0; }
        *woke = false;
    });
}

/// Ends a live job at once: it takes no more saves, no longer counts against its lane's next
/// start and is not placed again by a master restart; its lease goes, its app with it. Its run
/// finds it ended whenever that returns. (Ending it was left to the run: one that never returned,
/// its box's hub not closing the run, kept a live run "running (released)" and its lane's next start
/// refused until it was cleared by hand, 11-03.)
fn release(ctx: &Ctx, id: &str, note: &str) -> Result<(), String> {
    get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
    let r = RUNNING.lock().unwrap().as_mut().and_then(|m| m.remove(id));
    try_set(ctx, id, |s| {
        s.note = note.into();
        if s.active() {
            s.state = "ended".into();
            s.ended_ms = wall_ms();
        }
    })?;
    if let Some(r) = r {
        let _ = ask(&ctl(), None, "release", &r.lease);
    }
    Ok(())
}

/// No live job outlives its lane: one whose worktree is gone, or whose pinned commit no longer
/// resolves, is released, with an inbox line.
fn release_orphans(ctx: &Ctx) {
    let mut pass = crate::gitread::Pass::default();
    let lives: Vec<Live> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active()).cloned().collect());
    for x in lives.into_iter().filter(|x| !x.tree.is_empty()) {
        let why = if !Path::new(&x.tree).join(".git").exists() {
            format!("its worktree {} is gone", x.tree)
        } else if let Some(sha) = crate::lease::Pin::of_rev(&x.rev).map(|p| p.makepad).filter(|sha| crate::gitread::lacks_commit(&ctx.repo, sha)) {
            format!("its commit {} no longer resolves", super::short(&sha))
        } else if let Some(head) = lane_landed(ctx, &x, &mut pass) {
            format!("its lane landed ({} in work, its worktree clean) and the lane has been quiet for {} min (no call, lease, job or file change)", super::short(&head), DONE_IDLE_MS / 60_000)
        } else {
            continue;
        };
        if why.starts_with("its lane landed") {
            if let Some(r) = running(&x.id) {
                {
                    let mut calls = r.calls.lock().unwrap();
                    if calls.count > 0 || r.inflight.load(std::sync::atomic::Ordering::SeqCst) { continue; }
                    calls.park.get_or_insert_with(|| format!("released: {why}"));
                }
                park_ready(ctx, &x.id, &r);
                continue;
            }
        }
        event(ctx, Sev::Warn, format!("live {} ({}) of lane {}: released, {why}", x.id, x.app, x.lane), Some(&x));
        let _ = release(ctx, &x.id, if why.starts_with("its lane landed") { "released: its lane landed" } else { "released: its lane is gone" });
    }
}

/// A landed lane quiet this long can give up its sessions ([`lane_landed`]).
const DONE_IDLE_MS: u64 = 5 * 60 * 1000;

/// The whole lane is quiet, and its worktree is clean at a commit of its own (past the
/// session's base) that work holds. Landing alone does not mean its agent stopped working.
pub(super) fn lane_landed(ctx: &Ctx, x: &Live, pass: &mut crate::gitread::Pass) -> Option<String> {
    let now = wall_ms();
    let recent = |at| now.saturating_sub(at) < DONE_IDLE_MS;
    let (lives, busy_job) = ctx.store.read(|d| (
        d.lives.iter().filter(|l| l.lane == x.lane).cloned().collect::<Vec<_>>(),
        d.jobs.iter().any(|j| j.lane == x.lane && (!j.done() || recent(j.ended_ms))),
    ));
    let busy = |l: &Live| recent(l.touched_ms) || running(&l.id).is_some_and(|r| r.calls.lock().unwrap().count > 0 || r.inflight.load(std::sync::atomic::Ordering::SeqCst));
    if busy(x) || lives.iter().any(busy) || busy_job { return None; }
    let leases: Vec<_> = lives.iter().map(|l| l.lease.as_str()).chain(std::iter::once(x.lease.as_str())).collect();
    if !ctx.leases.lane_quiet(&x.lane, &leases, now, DONE_IDLE_MS) { return None; }
    let tree = Path::new(&x.tree);
    if recent(ctx.monitor.last_event_ms(tree)) { return None; }
    let head = pass.rev(tree, "HEAD")?;
    let base = crate::lease::Pin::of_rev(&x.rev).map(|p| p.makepad).unwrap_or_default();
    // Unknown history is not evidence that this lane made and landed a commit.
    if base.is_empty() || pass.is_ancestor(&ctx.repo, &head, &base)? || !pass.is_ancestor(&ctx.repo, &head, "refs/heads/work")? { return None; }
    let (checked_head, clean) = ctx.monitor.tracked_clean(tree).ok()?;
    (clean && checked_head == head).then_some(head)
}

/// The JIT's frame count from its `live` answer (a header line comes first).
fn frames_of(lines: &[String]) -> Option<u64> {
    lines.iter().find_map(|l| num(l, "frames"))
}

/// How long the JIT's watch waits for the edit's verdict.
const WATCH_S: u64 = 60;

/// Why an edit has no verdict: the watch ran out (the edit changed nothing the JIT compiled?), or
/// it came back early without one: what it said (an error, a closed link, another answer).
fn no_verdict(answer: &Result<Vec<String>, String>, waited: Duration) -> String {
    if waited.as_secs() + 1 >= WATCH_S {
        return format!("no patch event in {WATCH_S} s (the edit changed nothing the JIT compiled?)");
    }
    let said = match answer {
        Err(e) => format!("error: {e}"),
        Ok(l) if l.is_empty() => "no answer (the link closed?)".to_string(),
        Ok(l) => format!("answer: {}", l.join(" | ")),
    };
    format!("the JIT's watch came back after {} ms with no patch event ({})", waited.as_millis(), crate::proto::clean(&said, 300))
}

/// How often, 100 ms apart, a closed watch's app is looked at for its exit ([`peer_closed`]): it
/// closes its link as it dies, and its box reaps it a moment after.
const EXIT_LOOKS: u32 = 10;

/// What a session's box says of its app once a link to the app closed.
enum AppSaid {
    Exit(i64),
    Runs,
    /// The box doesn't answer: its hub is away.
    NoAnswer(String),
}

/// Asks a session's box how its app stands, until its exit is known or for [`EXIT_LOOKS`] looks.
fn app_said(r: &Running, x: &Live) -> AppSaid {
    let mut said = AppSaid::Runs;
    for look in 0..EXIT_LOOKS {
        if look > 0 { std::thread::sleep(Duration::from_millis(100)); }
        // (past the log's end: the exit alone, once the box reaped the run)
        let exit = ask(r.held.client(), Some(&r.node), "lease-log", &format!("lease-log {} {} {}", r.lease, x.run_n, u64::MAX >> 1))
            .map(|lines| lines.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok()).and_then(|v| v.get("exit").and_then(Value::as_i64)));
        said = match exit {
            Ok(Some(c)) => return AppSaid::Exit(c),
            Ok(None) => AppSaid::Runs,
            Err(e) => AppSaid::NoAnswer(e),
        };
    }
    said
}

/// [`AppSaid`] in words: the app's exit (a signal's said as 128 + its number), or that it runs on,
/// and its log's last lines.
fn said_of(said: &AppSaid, x: &Live) -> String {
    let said = match said {
        AppSaid::Exit(c) => format!("the app exited ({})", exit_said(*c)),
        AppSaid::Runs => format!("the app runs on (pid {})", x.pid),
        AppSaid::NoAnswer(e) => format!("its box did not say whether the app runs ({})", crate::proto::clean(e, 120)),
    };
    let tail = log_tail(&x.log, LOG_TAIL_LINES);
    if tail.is_empty() { said } else { format!("{said}; its log's last lines: {}", crate::proto::clean(&tail, 600)) }
}

/// Why a watch's link closed before its verdict, as far as the session's box and log say.
/// A closed link said only "connection closed by the peer" (W3683, 11-06): its lane could not tell
/// its own crash from devhub's.
fn peer_closed(r: &Running, x: &Live) -> String {
    said_of(&app_said(r, x), x)
}

/// What an `act` says whose link closed under it (`e`). A box that doesn't answer has lost its hub,
/// as each landing that rolls the hubs does, and the app runs on under the next one: `link lost`,
/// the cut its caller knows how to go on from (`devhub live act`, orch/cli.rs `cut_act`). A long read
/// ended on the bare close, its session running, at the first hub roll a check held one through
/// (11-09). Otherwise the app did it: its exit, or that it runs on.
fn closed_act(id: &str, e: &str, x: &Live, said: AppSaid) -> String {
    match said {
        AppSaid::NoAnswer(_) => format!("link lost: {}'s hub left during this call ({e})", x.node),
        said => format!("{id}: {e}: {}", said_of(&said, x)),
    }
}

/// An exit code as its run's waiter keeps it: a signal's is 128 + its number on Unix, a Windows
/// status (an NTSTATUS such as 0xC0000005) reads in hex.
fn exit_said(c: i64) -> String {
    match c {
        129..=159 => format!("exit {c}: signal {}", c - 128),
        c if c < 0 || c > 0xffff => format!("exit 0x{:08x}", c as u32),
        c => format!("exit {c}"),
    }
}

/// The last lines a closed watch's verdict shows of its session's log.
const LOG_TAIL_LINES: usize = 6;

/// The last `n` lines with text of the session's log (on this machine), from its last 16 KB.
fn log_tail(path: &str, n: usize) -> String {
    use std::io::{Read, Seek};
    let mut text = Vec::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let len = f.metadata().map_or(0, |m| m.len());
        if f.seek(std::io::SeekFrom::Start(len.saturating_sub(16 << 10))).is_ok() { let _ = f.read_to_end(&mut text); }
    }
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

fn event_cursor(v: &Value) -> String {
    let mut v = v.clone();
    if let Value::Obj(fields) = &mut v { fields.retain(|(k, _)| k != "rearm"); }
    v.to_json()
}

/// Bytes of this run, not an asynchronously followed local log: its text from `off` on, and the
/// offset after its last whole line, where the next window starts (the JIT's unbuffered stderr
/// writes a line in pieces; a window cut between them lost "makepad live:   struct ", 11-02).
fn run_log(r: &Running, run: u64, mut off: u64) -> Result<(u64, String), String> {
    let mut bytes = Vec::new();
    loop {
        let lines = ask(r.held.client(), Some(&r.node), "lease-log", &format!("lease-log {} {run} {off}", r.lease))?;
        let v = lines.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok()).ok_or("live: missing run log")?;
        if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(e.to_string()); }
        let chunk = v.get("data").and_then(Value::as_str).and_then(crate::lease::master::b64_decode).ok_or("live: invalid run log")?;
        let end = chunk.is_empty();
        off = v.get("off").and_then(Value::as_u64).unwrap_or(off + chunk.len() as u64);
        bytes.extend(chunk);
        if end { return Ok((whole_lines_end(off, &bytes), String::from_utf8_lossy(&bytes).into_owned())); }
    }
}

/// The offset after the last whole line of `bytes`, which end at `end`.
fn whole_lines_end(end: u64, bytes: &[u8]) -> u64 {
    let partial = bytes.iter().rposition(|b| *b == b'\n').map_or(bytes.len(), |i| bytes.len() - i - 1);
    end.saturating_sub(partial as u64)
}

/// The JIT's own crates and their sources: a panic located there is the JIT's.
const JIT_SOURCES: [(&str, &str); 3] = [("relax-compiler", "/relax/compiler/src/"), ("relax-front", "/relax/front/src/"), ("relax-backend", "/relax/backend/src/")];

/// A fault of the JIT itself in what it reported (a refused patch's reasons).
fn jit_fault(line: &str) -> bool {
    jit_fault_of("", line)
}

/// A fault of the JIT itself in a line of package `app`'s output. A test program of one of the
/// JIT's own crates is the JIT: its tests panic in those sources on purpose and print every line
/// the JIT prints (the ice-scope test's panic, lifeline's "JIT thread .. panicked" of its restart
/// test), so its output says nothing about the session's JIT, whose fault there shows as a run
/// that ended without a verdict. Every live run of relax-compiler --lib was stopped at the first
/// such line and restarted, then failed as a JIT error (11-12).
fn jit_fault_of(app: &str, line: &str) -> bool {
    if JIT_SOURCES.iter().any(|(package, _)| *package == app) {
        return false;
    }
    let l = line.to_ascii_lowercase();
    // This diagnostic invites a cargo comparison; its "makepad run bug" explanation is not
    // an internal compiler fault. Let devhub test check the same target with cargo.
    l.lines().any(|l| {
        if l.trim_start().starts_with("error: could not compile") && l.contains("no test ran") { return false; }
        l.contains("internal compiler error") || l.contains("makepad run bug") || l.starts_with("internal:")
            || l.contains("jit panic") || l.contains("jit ice") || l.contains("restart failed:")
            || (l.contains("jit thread") && l.contains("panicked"))
            || (l.contains("panicked at") && JIT_SOURCES.iter().any(|(_, dir)| l.contains(dir)))
    })
}

fn fault_line(app: &str, line: &str, captured: &mut bool) -> bool {
    if line.starts_with("---- ") && line.ends_with(" stdout ----") { *captured = true; }
    if line == "failures:" || line.starts_with("test result:") { *captured = false; }
    !*captured && jit_fault_of(app, line)
}

fn terminal(p: &LivePatch) -> bool {
    p.tests.is_some() || matches!(p.outcome.as_str(), "refused" | "compile_error" | "failed" | "jit_error" | "none" | "skipped")
}

/// An edit the session's JIT cannot take in place because the JIT itself must be rebuilt (the
/// Splash VM, other provided sources or makepad run's own changed). On a node that JIT is one
/// devhub picked by its inputs' content, the store's or the box's, which cannot rebuild itself
/// through cargo: a new run resolves the JIT again, an overlay's from the published overlay
/// (47 sessions ran on stale code, 11-03..05), a tree's from the lane's tree, and the lane's own
/// is built when its edits changed the JIT's inputs. A tree's relaunch went to its JIT, which
/// rebuilt the box's checkout for 79 s and ran the same binary (W3752, 11-06). A session pinned
/// to a JIT (`--jit`) keeps it: its lane chose that build, and the refusal says so.
fn needs_devhub_rebuild(x: &Live, p: &LivePatch) -> bool {
    x.loader.is_empty() && p.reason.contains("needs a JIT rebuild:")
}

fn rebuild_tests(x: &Live, p: &LivePatch) -> bool {
    x.test && p.tests.is_none() && needs_devhub_rebuild(x, p)
}

/// A relaunch devhub serves itself: an app session on a JIT devhub picked, which its host cannot
/// rebuild ([`needs_devhub_rebuild`]); one pinned to a JIT goes to its host.
fn devhub_relaunch(x: &Live, command: &str) -> bool {
    !x.test && x.loader.is_empty() && command.split_whitespace().eq(["session", "relaunch"])
}

/// One warm retry on the held box, with the same published overlay.
fn recover(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch, stop: bool) {
    let Some(r) = running(id) else { return };
    if get(ctx, id).is_none_or(|s| s.run_n != x.run_n || s.pid != x.pid || s.state != "running") { return; }
    if p.retried {
        let detail = format!("{{\"lane\":{},\"files\":[{}],\"log\":{},\"message\":{}}}", json_str(&x.lane), p.files.iter().map(|f| json_str(f)).collect::<Vec<_>>().join(","), json_str(&x.log), json_str(&p.reason));
        p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, p.reason.clone(), Some(detail));
        p.outcome = "jit_error".into();
        p.verdict_ms = wall_ms();
        if set_test_patch(ctx, id, x, p).is_none() { return; }
        set(ctx, id, |s| { s.state = "jit_error".into(); s.note = format!("JIT error, not your change: {}", p.jit_error); });
    } else {
        p.retried = true;
        return restart_session(ctx, id, x, &r, p, stop);
    }
    if stop { stop_run(ctx, &r, x); }
}

/// A new process on the overlay as shipped; `p.reason` says why. Tests restart at once, apps
/// only on their lane's relaunch. The replacement's verdict belongs to this edit.
fn restart_session(ctx: &Ctx, id: &str, x: &Live, r: &Running, p: &mut LivePatch, stop: bool) {
    p.tests = None;
    p.verdict_ms = 0;
    p.frame_ms = 0;
    p.outcome = "recovering".into();
    if set_test_patch(ctx, id, x, p).is_none() { return; }
    r.restart.store(true, std::sync::atomic::Ordering::SeqCst);
    event(ctx, Sev::Info, format!("live {id}: restarting its JIT warm on {}: {}", x.node, p.reason), Some(x));
    if stop && !stop_run(ctx, r, x) {
        // no restart comes: the edit's answer is that, not a wait for one
        p.outcome = "failed".into();
        p.reason = format!("its JIT could not be restarted: pid {} did not stop ({})", x.pid, p.reason);
        p.verdict_ms = wall_ms();
        r.restart.store(false, std::sync::atomic::Ordering::SeqCst);
        set_test_patch(ctx, id, x, p);
    }
}

/// Own shipping until the replacement is up. In particular, a relaunch immediately after a
/// save includes it even if the file monitor has not delivered that save yet.
fn relaunch(ctx: &Ctx, id: &str) -> Result<Vec<String>, String> {
    let r = running(id).ok_or_else(|| not_running(ctx, id))?;
    let signal = ctx.store.subscribe("live", &[id]);
    loop {
        let seen = signal.generation();
        if get(ctx, id).is_none_or(|x| !x.active()) { return Err(not_running(ctx, id)); }
        if makepad_remote::lines::client_gone() { return Err(format!("{id}: this call's client has gone")); }
        if let Some(note) = &r.calls.lock().unwrap().park { return Err(format!("{id}: {note}")); }
        if begin_call(&r) { break; }
        signal.wait(seen, Duration::from_secs(1));
    }
    let mut restarting = false;
    let result = (|| {
        let x = get(ctx, id).ok_or_else(|| not_running(ctx, id))?;
        let (files, edit_ms, current) = call_saves(ctx, &r, &x)?;
        if !files.is_empty() { ship_edit(ctx, id, &r, files, edit_ms, wall_ms(), Some(current), None); }
        let mut x = get(ctx, id).filter(|x| x.active() && x.pid > 0).ok_or_else(|| not_running(ctx, id))?;
        x.load_text()?;
        let mut p = x.patch.clone().unwrap_or_else(|| LivePatch { edit_ms, shipped_ms: wall_ms(), ..Default::default() });
        if p.outcome == "failed" { return Err(p.reason); }
        // Later unrelated saves may have replaced the patch's display text. The JIT keeps all
        // pending restart reasons. Ordinary relaunches retain its in-process state transfer.
        let session = if needs_devhub_rebuild(&x, &p) { Vec::new() } else { inspect(&x.node, x.pid, "session")? };
        if !needs_devhub_rebuild(&x, &p) && !session.iter().any(|l| l.contains("needs a JIT rebuild:")) {
            let pending = session.iter().find_map(|l| num(l, "restart_pending")).unwrap_or(0);
            let log_from = std::fs::metadata(&x.log).map_or(0, |m| m.len());
            let lines = inspect(&x.node, x.pid, "session relaunch")?;
            if pending == 0 || !lines.iter().any(|l| l.contains("\"relaunch\":\"queued\"")) { return Ok(lines); }
            return follow_relaunch(ctx, id, &x, p, lines, log_from);
        }
        if x.patch.is_none() { set(ctx, id, |s| s.patch = Some(p.clone())); }
        p.reason = "the lane asked for session relaunch".into();
        restart_session(ctx, id, &x, &r, &mut p, true);
        if p.outcome == "failed" { return Err(p.reason); }
        restarting = true;
        Ok(vec![format!("live {id}: relaunch queued through devhub; resolving the JIT for the current sources")])
    })();
    // On success the replacement's watcher releases shipping, as it does on the first start.
    if !restarting { finish_call(ctx, id, &r); }
    result
}

/// How often a relaunch the JIT queued looks at its JIT again.
const RELAUNCH_LOOK: Duration = Duration::from_millis(200);

/// A relaunch its JIT queued in place (the app restarts with its state in its own process, the
/// pid kept) is followed to its end: the new run has loaded the sources (no edit waits for a
/// restart), and the edit is applied; or the app postponed it, and the call fails with the app's
/// reason. It said "queued" and nothing after: the session showed its restart pending, the pid
/// unchanged, though the app had restarted, and the lane released it for a fresh one (11-06).
/// No clock ends it: the session ending does, or the client it answers going.
fn follow_relaunch(ctx: &Ctx, id: &str, x: &Live, mut p: LivePatch, mut lines: Vec<String>, log_from: u64) -> Result<Vec<String>, String> {
    loop {
        if get(ctx, id).is_none_or(|s| !s.active() || s.pid != x.pid) { return Err(not_running(ctx, id)); }
        if makepad_remote::lines::client_gone() { return Err(format!("{id}: this call's client has gone; its relaunch is queued in the app")); }
        // (a look that fails meanwhile is looked at again: the process's end ends the session)
        let live = inspect(&x.node, x.pid, "live").unwrap_or_default();
        let log = log_since(&x.log, log_from);
        match relaunch_end(&live, &log) {
            Some(Ok(())) => {
                (p.outcome, p.reason, p.outcome_ms) = ("patched".into(), "restarted in its process on the current sources".into(), wall_ms());
                set_test_patch(ctx, id, x, &p);
                set(ctx, id, restart_ran);
                lines.push(format!("live {id}: restarted in its process (pid {}) on the current sources", x.pid));
                return Ok(lines);
            }
            Some(Err(why)) => {
                p.reason = format!("the app postponed its restart: {why}");
                set_test_patch(ctx, id, x, &p);
                return Err(format!("{id}: the app postponed its restart: {why} (it restarts when the app allows it)"));
            }
            None => std::thread::sleep(RELAUNCH_LOOK),
        }
    }
}

/// What a relaunch queued in place came to, from its JIT's `live` look and the session's log since
/// the relaunch: done (no edit waits for a restart: the new run loaded the sources), postponed
/// (the app's `restart postponed (<why it restarts>): <why it waits>`), or on its way (None).
fn relaunch_end(live: &[String], log: &str) -> Option<Result<(), String>> {
    if live.iter().find_map(|l| num(l, "restart_pending")) == Some(0) { return Some(Ok(())); }
    log.lines().find_map(|l| l.split_once("restart postponed ").map(|(_, why)| Err(why.trim().to_string())))
}

/// The session log's text from byte `from` on (empty when unreadable).
fn log_since(path: &str, from: u64) -> String {
    use std::io::{Read, Seek};
    let mut text = Vec::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        if f.seek(std::io::SeekFrom::Start(from)).is_ok() { let _ = f.read_to_end(&mut text); }
    }
    String::from_utf8_lossy(&text).into_owned()
}

/// Stops the run's JIT for its restart; false when it could not.
fn stop_run(ctx: &Ctx, r: &Running, x: &Live) -> bool {
    if x.pid == 0 { return true; }
    let _ = inspect(&x.node, x.pid, "quit");
    let asked = Instant::now();
    while asked.elapsed() < Duration::from_secs(2) {
        if get(ctx, &x.id).is_none_or(|s| s.pid != x.pid || s.run_n != x.run_n) { return true; }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The original run is still followed; only its recorded pid is stopped.
    match r.held.kill(x.pid) {
        Ok(()) => true,
        Err(e) => {
            event(ctx, Sev::Warn, format!("live {}: stopping pid {}: {e}", x.id, x.pid), Some(x));
            false
        }
    }
}

/// The failed tests' captured output in this verdict's log interval.
fn failure_output(log: &str, failures: &[String]) -> String {
    // --nocapture can interleave output: keep the interval if it cannot be split by test.
    if failures.iter().any(|name| !log.contains(&format!("---- {name} stdout ----"))) { return log.to_string(); }
    // A hardware fault in test code (a stack overflow, a segfault) is reported by the JIT outside
    // the test's captured output: its report comes first, or the verdict says only "a fault".
    let mut out = String::new();
    let mut report = false;
    for line in log.lines() {
        if line.starts_with("makepad: POISONED:") || line.starts_with("makepad: fault") { report = true; }
        else if !line.starts_with("  ") { report = false; }
        if report { out.push_str(line); out.push('\n'); }
    }
    let mut keep = false;
    for line in log.lines() {
        if let Some(name) = line.strip_prefix("---- ").and_then(|s| s.strip_suffix(" stdout ----")) {
            keep = failures.iter().any(|f| f == name);
        } else if line == "failures:" || line.starts_with("test result:") { keep = false; }
        if keep { out.push_str(line); out.push('\n'); }
    }
    out
}

fn finish_tests(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch) {
    let Some(r) = running(id) else { return };
    if rebuild_tests(x, p) {
        restart_session(ctx, id, x, &r, p, true);
        return;
    }
    if p.outcome == "jit_error" {
        if let Some(x) = set_test_patch(ctx, id, x, p) { event(ctx, Sev::Err, verdict(p).trim_end().to_string(), Some(&x)); }
        return;
    }
    if let Some(reason) = r.fault.lock().unwrap().clone() {
        p.tests = None;
        p.outcome = "jit_fault".into();
        p.reason = reason;
    }
    if matches!(p.outcome.as_str(), "jit_fault" | "restart") && p.tests.is_none() {
        recover(ctx, id, x, p, true);
        return;
    }
    match run_log(&r, x.run_n, p.log_start) {
        Err(e) => p.output = format!("live log {}: {e}\n", x.log),
        Ok((end, log)) => {
            p.log_end = end;
            let mut captured = false;
            if let Some(reason) = log.lines().find(|l| fault_line(&x.app, l, &mut captured)).filter(|_| p.outcome != "refused" && p.reason != "patched after refusal" && !p.tests.as_ref().is_some_and(|t| t.restarted)) {
                p.tests = None;
                p.reason = reason.to_string();
                recover(ctx, id, x, p, true);
                return;
            }
            if let Some(t) = &p.tests { p.output = failure_output(&log, &t.failures); }
            else if p.outcome == "refused" && !log.is_empty() { p.output = log; }
        }
    }
    if p.tests.is_none() && p.outcome == "refused" && !crate::jit_misses::compile_errors(&format!("{}\n{}", p.reason, p.output)).is_empty() {
        p.outcome = "compile_error".into();
        p.verdict_ms = wall_ms();
    }
    if let Some(x) = set_test_patch(ctx, id, x, p) {
        event(ctx, if p.tests.as_ref().is_some_and(|t| t.failed == 0) { Sev::Info } else { Sev::Warn }, format!("live {id}: {}", verdict(p).trim_end()), Some(&x));
    }
}

/// One event, rearming across an exec's closed link and reset event sequence.
fn test_event(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch, on: &str) -> Option<Value> {
    let x_run = x.run_n;
    while running(id).is_some() && get(ctx, id).is_some_and(|x| x.state == "running" && x.pid > 0 && x.run_n == x_run && x.patch.as_ref().is_some_and(|old| old.edit_ms == p.edit_ms)) {
        match inspect(&x.node, x.pid, &format!("watch on={on} since={} timeout_s=1", p.seq)) {
            Ok(lines) => {
                for v in lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
                    let Some(kind) = v.get("event").and_then(Value::as_str) else { continue };
                    if kind == "none" { continue; }
                    p.seq = v.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
                    p.cursor = event_cursor(&v);
                    return Some(v);
                }
                if p.outcome != "hot-recovery" {
                    if let Some(reason) = running(id).and_then(|r| { let fault = r.fault.lock().unwrap().clone(); fault }) {
                        p.outcome = "jit_fault".into(); p.reason = reason; return None;
                    }
                }
                if let Ok(lines) = inspect(&x.node, x.pid, "events") {
                    let events: Vec<Value> = lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).filter(|v| v.get("seq").is_some()).collect();
                    let reset = events.iter().filter_map(|v| v.get("seq").and_then(Value::as_u64)).max().is_some_and(|seq| seq < p.seq)
                        || events.iter().any(|v| v.get("seq").and_then(Value::as_u64) == Some(p.seq) && !p.cursor.is_empty() && event_cursor(v) != p.cursor);
                    if reset { p.seq = 0; p.cursor.clear(); }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    None
}

/// Recovery belongs to the JIT; DEVLOOP still waits for the replacement's test verdict.
fn hot_event(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch, v: &Value) -> bool {
    match v.get("event").and_then(Value::as_str) {
        Some("patchfailed") => p.outcome = if x.test { "hot-recovery" } else { "recovering" }.into(),
        Some("patchrestarted") => {
            p.outcome = if x.test { "restart" } else { "patched" }.into();
            p.reason = "warm restarted".into();
        }
        // the JIT's own error channel (`jiterror {record, reason}`): its warm restart could not
        // get past a JIT failure, never the lane's change. A test run takes devhub's own
        // recovery and verdict (jit_fault); a live session records it as a JIT error.
        Some("jiterror") if x.test => {
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
            p.reason = format!("{} (JIT record {})", s("reason"), s("record"));
            p.outcome = "jit_fault".into();
        }
        Some("jiterror") => {
            let record = v.get("record").and_then(Value::as_str).unwrap_or_default();
            p.reason = makepad_remote::hot::Verdict::JitError { record: record.into() }.line();
            p.outcome = "jit-error".into();
            p.tests = None;
            p.verdict_ms = wall_ms();
            p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, p.reason.clone(), Some(v.to_json()));
        }
        _ => return false,
    }
    p.seq = v.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
    p.cursor = event_cursor(v);
    p.outcome_ms = wall_ms();
    if let Some(r) = running(id) { *r.fault.lock().unwrap() = None; }
    true
}

fn patch_verdict(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch) {
    // the JIT's verdict on the edit
    let asked = Instant::now();
    let r = running(id);
    let newer = newer_saves(ctx, x, r.as_deref());
    let answer = if x.test {
        let mut found = None;
        while let Some(v) = test_event(ctx, id, x, p, "patchverified,patchrefused,patchreverted,patchfailed,patchrestarted,jiterror,testverdict") {
            // A normal verdict belongs to the group of a preceding patchverified. Only an
            // exec's restarted verdict can replace that event (its event queue starts over).
            if v.get("event").and_then(Value::as_str) == Some("testverdict") && !crate::state::live_tests_from(&v).is_some_and(|t| t.restarted) { continue; }
            found = Some(v);
            break;
        }
        found.map(|v| vec![v.to_json()]).ok_or_else(|| "the test JIT's watch ended".to_string())
    } else {
        let watch = format!("watch on=patchverified,patchrefused,patchreverted,patchfailed,patchrestarted,jiterror since={} timeout_s={WATCH_S}", p.seq);
        loop {
            match inspect_until(&x.node, x.pid, &watch, &newer) {
                Ok(Some(lines)) => break Ok(lines),
                // the newer edit ships now; its verdict is the one on both
                Ok(None) => {
                    p.outcome = "superseded".into();
                    p.reason = "a newer save shipped before its verdict came: the newer edit's verdict covers it".into();
                    p.outcome_ms = wall_ms();
                    return;
                }
                // The box's hub restarts (its self-update after a landing): the app runs on and keeps
                // its events, so the watch goes on once the hub answers again. A delivered edit
                // was said "none" in that window (11-04).
                Err(e) if hub_away(&e) && r.as_deref().is_some_and(|r| hub_back(ctx, id, x, r)) => continue,
                Err(e) => break Err(e),
            }
        }
    };
    if p.outcome == "jit_fault" { return; }
    let waited = asked.elapsed();
    // the line that answers (any header line before it is skipped)
    let v = answer.as_ref().ok().and_then(|lines| lines.iter().filter_map(|line| makepad_strict_json::parse(line.as_bytes()).ok()).find(|v| v.get("event").is_some()));
    let s = |k: &str| v.as_ref().and_then(|v| v.get(k)).map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_json())).unwrap_or_default();
    p.edit = s("edit");
    p.group = v.as_ref().and_then(|v| v.get("group")).and_then(Value::as_u64);
    if let Some(v) = &v {
        p.seq = v.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
        p.cursor = event_cursor(v);
    }
    p.outcome_ms = wall_ms();
    if !v.as_ref().is_some_and(|v| hot_event(ctx, id, x, p, v)) {
        match s("event").as_str() {
            "patchverified" => p.outcome = "patched".into(),
            "testverdict" if x.test => {
                p.tests = v.as_ref().and_then(crate::state::live_tests_from);
                p.outcome = if p.tests.as_ref().is_some_and(|t| t.restarted) { "restart" } else { "patched" }.into();
                p.outcome_ms = 0;
                p.verdict_ms = wall_ms();
            }
            "patchrefused" => {
                p.outcome = if s("outcome") == "restart" { "restart" } else { "refused" }.into();
                p.reason = v.as_ref().and_then(|v| v.get("reasons")).and_then(Value::as_arr).unwrap_or(&[]).iter().map(|r| r.as_str().map(str::to_string).unwrap_or_else(|| r.to_json())).collect::<Vec<_>>().join("\n");
                if needs_devhub_rebuild(x, p) {
                    p.outcome = "restart".into();
                } else if x.test && jit_fault(&p.reason) {
                    p.outcome = "hot-recovery".into();
                    if let Some(r) = running(id) { *r.fault.lock().unwrap() = None; }
                }
            }
            "patchreverted" => {
                p.reason = s("error");
                p.outcome = if x.test { "jit_fault" } else { "reverted" }.into();
            }
            _ => {
                p.outcome = "none".into();
                p.reason = no_verdict(&answer, waited);
                // the link closed before its verdict: what the app did then
                if waited.as_secs() + 1 < WATCH_S && answer.as_ref().map_or(true, |l| l.is_empty()) {
                    if let Some(r) = r.as_deref() { p.reason.push_str(&format!("; {}", peer_closed(r, x))); }
                }
            }
        }
    }
    if !x.test && matches!(p.outcome.as_str(), "recovering" | "refused" | "reverted") {
        // A refused partial landing may patch on its quiet retry or next save.
        // (a newer save ends the wait: the JIT's verdict on this edit stands)
        let recovery = inspect_until(&x.node, x.pid, &format!("watch on=patchverified,patchrestarted,jiterror since={} timeout_s={WATCH_S}", p.seq), &newer);
        if let Some(v) = recovery.ok().flatten().and_then(|lines| lines.into_iter().filter_map(|line| makepad_strict_json::parse(line.as_bytes()).ok()).find(|v| matches!(v.get("event").and_then(Value::as_str), Some("patchverified" | "patchrestarted" | "jiterror")))) {
            if !hot_event(ctx, id, x, p, &v) {
                p.outcome = "patched".into();
                p.reason.clear(); p.reason_ref = Default::default();
                p.seq = v.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
                p.cursor = event_cursor(&v);
                p.outcome_ms = wall_ms();
            }
        }
    }
}

/// Tests finish independently of the caller's wait timeout.
fn test_verdict(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch) {
    // No verdict can come from this binary for the edit. Let finish_tests start the new JIT.
    if rebuild_tests(x, p) { return; }
    while let Some(v) = test_event(ctx, id, x, p, "testverdict,patchverified,patchrefused,patchreverted,patchfailed,patchrestarted,jiterror") {
        if v.get("event").and_then(Value::as_str) == Some("patchverified") {
            if p.outcome == "hot-recovery" {
                p.outcome = "patched".into();
                p.reason = "patched after refusal".into();
                p.group = v.get("group").and_then(Value::as_u64);
                p.outcome_ms = wall_ms();
                if let Some(r) = running(id) { *r.fault.lock().unwrap() = None; }
            }
            continue;
        }
        if hot_event(ctx, id, x, p, &v) {
            if p.outcome == "jit_fault" { return; }
            continue;
        }
        if v.get("event").and_then(Value::as_str) == Some("patchreverted") {
            p.reason = v.get("error").and_then(Value::as_str).unwrap_or("").to_string();
            p.outcome = "jit_fault".into();
            return;
        }
        if v.get("event").and_then(Value::as_str) == Some("patchrefused") {
            p.reason = v.get("reasons").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
            p.outcome = if jit_fault(&p.reason) { "hot-recovery" } else if v.get("outcome").and_then(Value::as_str) == Some("restart") { "restart" } else { "refused" }.into();
            if let Some(r) = running(id) { *r.fault.lock().unwrap() = None; }
            if p.outcome == "hot-recovery" { continue; }
            return;
        }
        let Some(t) = crate::state::live_tests_from(&v) else { continue };
        let matches = match p.outcome.as_str() {
            "initial" => t.group == 0,
            "restart" => t.restarted,
            // The patch's edit is display text, the harness's edit is a path array. The
            // group is their shared identity; an older group's verdict never completes it.
            _ => t.restarted || Some(t.group) == p.group,
        };
        if !matches { continue; }
        p.tests = Some(t);
        p.verdict_ms = wall_ms();
        return;
    }
}

/// The first run, or a verdict still pending when the master restarted.
fn resume_tests(ctx: &Ctx, id: &str, r: &Running) {
    let Some(mut x) = get(ctx, id) else { return };
    if let Err(e) = x.load_text() { eprintln!("live log: {e}"); return; }
    let Some(mut p) = x.patch.clone() else { return };
    if p.outcome == "recovering" { return; }
    if p.outcome.is_empty() { patch_verdict(ctx, id, &x, &mut p); }
    if p.tests.is_none() && matches!(p.outcome.as_str(), "initial" | "patched" | "restart" | "hot-recovery") {
        test_verdict(ctx, id, &x, &mut p);
    }
    if get(ctx, id).is_none_or(|s| s.run_n != x.run_n || s.pid != x.pid || s.state != "running") { return; }
    finish_tests(ctx, id, &x, &mut p);
    let done = ctx.store.read(|d| d.lives.iter().any(|s| s.id == id && s.run_n == x.run_n && s.pid == x.pid && s.patch.as_ref().is_some_and(terminal)));
    if done && r.resuming.swap(false, std::sync::atomic::Ordering::SeqCst) { finish_call(ctx, id, r); }
}

/// Copies changed bytes before shipping; the manifest still names the complete overlay.
/// A tree file as a session ships it: its source hash (symlink or not, exec bit, sha256), its
/// bytes (a symlink's target) and its entry.
fn source_of(tree: &Path, path: &str) -> Result<(String, Vec<u8>, crate::lease::ship::Entry), String> {
    let src = tree.join(path);
    let mut entries = Vec::new();
    crate::lease::ship::entries(tree, path, &mut entries)?;
    let entry = entries.into_iter().next().filter(|e| e.path == path).ok_or_else(|| format!("live: {path} is not a file"))?;
    let bytes = match &entry.link {
        Some(link) => link.as_bytes().to_vec(),
        None => crate::lease::ship::read_stable(&src).map_err(|e| format!("{}: {e}", src.display()))?.0,
    };
    let hash = format!("{}:{}:{}", entry.link.is_some(), entry.exec, makepad_network::digest::sha256_hash(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>());
    Ok((hash, bytes, entry))
}

fn capture(tree: &Path, changed: &[String], deleted: &[String], old: &BTreeMap<String, String>, dir: &Path) -> Result<BTreeMap<String, String>, String> {
    if dir.exists() { std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?; }
    let mut sources = BTreeMap::new();
    for path in changed {
        let src = tree.join(path);
        // gone since the save was heard (a file that came and went, renamed away): a deletion when
        // it shipped before, else nothing of it ships
        if std::fs::symlink_metadata(&src).is_err() {
            if old.get(path).is_some_and(|h| h != "-") {
                sources.insert(path.clone(), "-".into());
            }
            continue;
        }
        let (hash, bytes, entry) = source_of(tree, path)?;
        if old.get(path) != Some(&hash) {
            let dst = dir.join(path);
            std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
            if let Some(link) = &entry.link {
                #[cfg(unix)]
                std::os::unix::fs::symlink(link, &dst).map_err(|e| e.to_string())?;
                #[cfg(not(unix))]
                return Err(format!("live: cannot snapshot symlink {path} to {link}"));
            } else {
                std::fs::write(&dst, &bytes).map_err(|e| e.to_string())?;
                std::fs::set_permissions(&dst, std::fs::metadata(&src).map_err(|e| e.to_string())?.permissions()).map_err(|e| e.to_string())?;
            }
        }
        sources.insert(path.clone(), hash);
    }
    sources.extend(deleted.iter().map(|p| (p.clone(), "-".into())));
    Ok(sources)
}

/// Ships saved files to the box; the JIT's verdict and the first frame after it are the result.
/// Test callers supply the manifest they just read, including saves the monitor has not heard.
/// Returns a note when the shipped edit cannot reach the target; tests keep their last verdict.
/// The files under a run's `DEVHUB_ALSO` paths: they ship although `local/` never does.
fn also_files(tree: &Path, also: &[String]) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    for p in also {
        crate::lease::ship::entries(tree, p, &mut entries)?;
    }
    Ok(entries.into_iter().map(|e| e.path).filter(|p| !crate::lease::ship::never_ships_named(p)).collect())
}

/// After a node call failed on its hub not answering: waits, while the session and its lease
/// live, until the hub answers again (true), else false.
/// A node call's error that is its hub away for a moment (restarting for its self-update, or not
/// reachable): [`hub_back`] waits for it.
fn hub_away(e: &str) -> bool {
    crate::lease::master::unreachable(e) || e.contains("restarting: ")
}

fn hub_back(ctx: &Ctx, id: &str, x: &Live, r: &Running) -> bool {
    loop {
        if get(ctx, id).is_none_or(|s| !s.active() || s.pid != x.pid || s.run_n != x.run_n) || !super::leg::lease_held(ctx, &r.lease) { return false; }
        std::thread::sleep(Duration::from_secs(1));
        if ask(r.held.client(), Some(&r.node), "caps", "caps").is_ok() { return true; }
    }
}

/// Where a ship's time went, step by step ("admit 0 dirty 2 capture 1 ...", ms each): the
/// ledger's save -> shipped split for the live loop's measurements.
#[derive(Default)]
struct Steps {
    at: u64,
    said: String,
}

impl Steps {
    fn lap(&mut self, name: &str) {
        let now = wall_ms();
        if !self.said.is_empty() { self.said.push(' '); }
        self.said.push_str(&format!("{name} {}", now.saturating_sub(self.at)));
        self.at = now;
    }
}

/// `called_ms`: when the call came (a test session's `live sync`, before it waits for its box's
/// room or an earlier call): what follows it is devhub's, what comes before it the caller's.
/// `queued`: how long a test call waited for the previous call's tests, and behind what.
fn ship_edit(ctx: &Ctx, id: &str, r: &Running, files: Vec<String>, edit_ms: u64, called_ms: u64, current: Option<(Vec<String>, Vec<String>)>, queued: Option<(u64, String)>) {
    let Some(x) = get(ctx, id).filter(|x| x.active() && x.pid > 0) else { return };
    let mut note = None;
    let mut unread = Vec::new();
    let (queued_ms, behind) = queued.unwrap_or_default();
    let mut p = LivePatch { edit_ms: edit_ms.max(x.patch.as_ref().map_or(0, |p| p.edit_ms + 1)), called_ms, queued_ms, behind, ..Default::default() };
    let mut steps = Steps::default();
    let mut ship = || -> Result<bool, String> {
        steps = Steps { at: called_ms + queued_ms, said: if queued_ms > 0 { format!("queued {queued_ms}") } else { String::new() } };
        steps.lap("admit");
        let pin = crate::lease::Pin::of_rev(&x.rev).ok_or("live: missing pin")?;
        let (mut changed, mut deleted) = match current.clone() {
            Some(paths) => paths,
            // the save came from the monitor: its dirty set has it, no status of the tree
            None => crate::lease::ship::uncommitted_seen(&ctx.monitor, &r.tree, &pin)?,
        };
        changed.extend(also_files(&r.tree, &x.also)?);
        changed.sort();
        changed.dedup();
        steps.lap("dirty");
        let selected: BTreeSet<&String> = files.iter().collect();
        if x.test {
            changed.retain(|p| selected.contains(p));
            deleted.retain(|p| selected.contains(p));
        }
        let dir = ctx.logs("live").join(format!("{id}.ship"));
        let mut sources = capture(&r.tree, &changed, &deleted, &x.sources, &dir)?;
        steps.lap("capture");
        if x.test {
            sources.extend(x.sources.iter().filter(|(p, _)| !selected.contains(p)).map(|(p, h)| (p.clone(), h.clone())));
        }
        if sources == x.sources { return Ok(false); }
        if !x.test && r.up_pid.load(std::sync::atomic::Ordering::Relaxed) != x.pid {
            if !wait_up(ctx, id, r, x.pid) { return Ok(false); }
            steps.lap("up");
        }
        p.files = sources.keys().chain(x.sources.keys()).filter(|k| sources.get(*k) != x.sources.get(*k)).cloned().collect::<BTreeSet<_>>().into_iter().collect();
        // the event cursor and the inputs at once, one round trip: both are asked before the
        // ship, so no answer can hold anything the JIT did with this edit. The inputs are asked
        // about the edit's files only (`of`): the whole list cost the JIT 25-40 ms.
        let inputs_of = format!("{{\"cmd\":\"inputs\",\"of\":{}}}", json_str(&p.files.join("\n")));
        let (events, inputs) = std::thread::scope(|sc| {
            let inputs = sc.spawn(|| inspect(&r.node, x.pid, &inputs_of));
            let events = inspect(&r.node, x.pid, "events");
            (events, inputs.join().unwrap_or_else(|_| Err("live: an inspect call panicked".into())))
        });
        let cursor = events?.into_iter().filter(|l| num(l, "seq").is_some()).max_by_key(|l| num(l, "seq").unwrap_or(0)).unwrap_or_default();
        p.seq = num(&cursor, "seq").unwrap_or(0);
        p.cursor = makepad_strict_json::parse(cursor.as_bytes()).ok().map(|v| event_cursor(&v)).unwrap_or_default();
        steps.lap("cursor");
        p.log_start = if x.test { run_log(r, x.run_n, x.patch.as_ref().map_or(0, |p| p.log_end))?.0 } else { 0 };
        set(ctx, id, |s| { s.patch = Some(p.clone()); s.touched_ms = wall_ms(); });
        let mut entries = Vec::new();
        for path in changed.iter().filter(|p| sources.get(*p) != x.sources.get(*p)) {
            crate::lease::ship::entries(&dir, path, &mut entries)?;
        }
        // files the session shipped before have their place on the node: no ship-have asks
        let placed = entries.iter().all(|e| x.sources.get(&e.path).is_some_and(|h| h.as_str() != "-"));
        let sent = if entries.is_empty() { Default::default() } else { crate::lease::ship::send(r.held.client(), &r.node, &r.lease, &dir, &entries, placed)? };
        steps.lap("send");
        steps.said.push_str(&format!(" (node {})", sent.node_ms));
        // ship-done publishes the complete overlay, retaining paths this call did not send.
        let changed: Vec<String> = sources.iter().filter(|(_, h)| h.as_str() != "-").map(|(p, _)| p.clone()).collect();
        let deleted: Vec<String> = sources.iter().filter(|(_, h)| h.as_str() == "-").map(|(p, _)| p.clone()).collect();
        let list = |v: &[String]| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
        let bases = crate::lease::ship::bases_json(&r.tree, &pin, &changed.iter().chain(&deleted).cloned().collect::<Vec<_>>())?;
        steps.lap("bases");
        let body = format!("{{\"shipped\":[{}],\"deleted\":[{}],\"bases\":{bases}}}", list(&changed), list(&deleted));
        let a = ask(r.held.client(), Some(&r.node), "ship-done", &format!("ship-done {} {}", r.lease, makepad_network::digest::base64_encode(body.as_bytes())))?;
        if let Some(e) = a.first().filter(|e| e.starts_with("{\"err\":")) { return Err(e.clone()); }
        p.shipped_ms = wall_ms();
        steps.lap("done");
        set(ctx, id, |s| s.sources = sources);
        // One query for the exact shipped set. Unknown or incomplete inputs cannot
        // justify waiting for a patch event (nor claiming this edit missed the target).
        let lines = inputs?;
        let mut inputs = BTreeSet::new();
        let mut outside = BTreeSet::new();
        let mut count = None;
        for line in &lines {
            let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
            if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(format!("live: inputs: {e}")); }
            if let Some(n) = v.get("count").and_then(Value::as_u64) { count = Some(n); }
            if let Some(path) = v.get("path").and_then(Value::as_str) { inputs.insert(path.to_string()); }
            if let Some(path) = v.get("outside").and_then(Value::as_str) { outside.insert(path.to_string()); }
        }
        if count != Some(inputs.len() as u64) { return Err("live: inputs did not return the complete source list".into()); }
        if x.test {
            // (what the run names in DEVHUB_ALSO is its own, wherever it lies)
            outside.retain(|f| !x.also.iter().any(|a| Path::new(f).starts_with(a)));
            unread = p.files.iter().filter(|f| !inputs.contains(*f) && !outside.contains(*f)).cloned().collect();
            if unread.is_empty() && !p.files.iter().any(|f| inputs.contains(f)) {
                if x.patch.as_ref().is_some_and(terminal) {
                    note = Some(format!("{} is in no package of the tests' graph: the last verdict stands", p.files[0]));
                } else {
                    unread = p.files.clone();
                }
            }
        } else if !p.files.iter().any(|p| inputs.contains(p)) {
            note = Some(format!("edit does not reach {}: {} is not one of its sources", x.app, p.files[0]));
        }
        Ok(true)
    };
    // a hub that restarts for its update (seconds; its runs and leases go on) is waited for and the
    // edit shipped once it answers: a save never fails on a roll (one did, refused mid-roll, 11-02)
    let shipped = loop {
        let shipped = ship();
        match &shipped {
            Err(e) if hub_away(e) && hub_back(ctx, id, &x, r) => continue,
            _ => break shipped,
        }
    };
    if p.shipped_ms == 0 { p.shipped_ms = wall_ms(); }
    if matches!(shipped, Ok(false)) { return; }
    if p.files.is_empty() { p.files = files; }
    let count = p.files.len();
    if let Err(e) = shipped {
        // released meanwhile: its release said so, the ship it cut short says nothing
        if !get(ctx, id).is_some_and(|s| s.active()) { return; }
        // no edit can reach it again: it ends, said once (before, each save failed, 11-04)
        if let Some(why) = session_over(&e, &r.lease) {
            event(ctx, Sev::Warn, format!("live {id} ({}): ended, {why}; `makepad devhub live start {}` starts a new session", x.app, x.app), Some(&x));
            let _ = release(ctx, id, &format!("ended: {why}"));
            return;
        }
        p.outcome = "failed".into();
        p.outcome_ms = wall_ms();
        p.reason = e.clone();
        let x = set(ctx, id, |s| s.patch = Some(p.clone()));
        event(ctx, Sev::Warn, format!("live {id}: shipping {count} file(s) failed: {e}"), x.as_ref());
        if let Some(x) = &x { unapplied(ctx, x, &p); }
        return;
    }
    if let Some(note) = note {
        // A lane's scratch file beside the packages: the verdict is still about everything the
        // tests can read. It says so; a file its program does not read in a package does not land here.
        if x.test {
            let current = set(ctx, id, |s| {
                if s.run_n == x.run_n && s.pid == x.pid && s.state == "running" && s.patch.as_ref().is_some_and(|now| now.edit_ms == p.edit_ms) {
                    s.patch = x.patch.clone().map(|old| LivePatch { reason: note.clone(), reason_ref: Default::default(), ..old });
                }
            });
            event(ctx, Sev::Info, format!("live {id}: {note}"), current.as_ref());
            return;
        }
        p.outcome = "skipped".into();
        p.reason = note.clone();
        p.outcome_ms = wall_ms();
        let current = set_patch(ctx, id, &p);
        event(ctx, Sev::Info, format!("live {id}: {note}"), current.as_ref());
        return;
    }
    // The running program does not read it, so no patch and no selection can say what it does to
    // the tests, yet they can read it: a process they start, a data file. The last verdict is
    // about the bytes before it (a broken spritelab main.rs still said 1/1 passed, 11-04).
    if let Some(file) = unread.first() {
        p.reason = format!("{file} is not a source of the running tests: they start again on it");
        restart_session(ctx, id, &x, r, &mut p, true);
        wait_tests(ctx, id);
        return;
    }
    set(ctx, id, |s| {
        if let Some(current) = &mut s.patch {
            if current.edit_ms == p.edit_ms { current.shipped_ms = p.shipped_ms; }
        }
    });
    if get(ctx, id).is_some_and(|s| s.pid == x.pid && s.run_n == x.run_n && s.patch.as_ref().is_some_and(|p| p.outcome.is_empty())) {
        patch_verdict(ctx, id, &x, &mut p);
    } else {
        wait_tests(ctx, id);
        return;
    }
    if x.test {
        if p.tests.is_none() && set_test_patch(ctx, id, &x, &p).is_none() { wait_tests(ctx, id); return; }
        if p.tests.is_none() && matches!(p.outcome.as_str(), "patched" | "restart" | "hot-recovery") {
            test_verdict(ctx, id, &x, &mut p);
        }
        if get(ctx, id).is_some_and(|s| s.run_n == x.run_n && s.pid == x.pid && s.state == "running") { finish_tests(ctx, id, &x, &mut p); }
        wait_tests(ctx, id);
        return;
    }
    // the first frame on the glass built with it: the JIT's `live` says the patches it installed
    // (this one among them, its verdict came) and the ones the newest presented window frame was
    // built with (`frame_patch`). A frame count after the ship counted an animating app's next
    // frame, drawn before the edit's design re-run (a third of Fab's edits said "frame" at 0 ms).
    if p.outcome == "patched" && !x.test {
        let t = Instant::now();
        // (a newer save ships now: its frame is the one that counts)
        let newer = newer_saves(ctx, &x, Some(r));
        let mut installed = None;
        while t.elapsed() < Duration::from_secs(5) && !newer() {
            let live = inspect(&r.node, x.pid, "live").unwrap_or_default();
            let field = |k: &str| live.iter().find_map(|l| num(l, k));
            if let Some(n) = field("patches") { installed.get_or_insert(n); }
            if installed.is_some_and(|n| field("frame_patch").is_some_and(|f| f >= n)) {
                p.frame_ms = wall_ms();
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let Some(mut x) = set_patch(ctx, id, &p) else { return };
    if matches!(p.outcome.as_str(), "none" | "failed") { unapplied(ctx, &x, &p); }
    // an app's save that waits for a restart is counted until the app restarts; a later one that
    // patched asks whether the app restarted meanwhile (a postponed relaunch, the person's click)
    if !x.test && p.outcome == "restart" {
        x = set(ctx, id, |s| restart_waits(s, &p, wall_ms())).unwrap_or(x);
    } else if !x.test && x.restart_since_ms > 0 && p.outcome == "patched" && inspect(&x.node, x.pid, "session").is_ok_and(|l| l.iter().find_map(|l| num(l, "restart_pending")) == Some(0)) {
        x = set(ctx, id, restart_ran).unwrap_or(x);
    }
    // an agent's save -> patched (-> frame), beside the test sessions' save -> verdict
    if p.edit_ms > 0 && p.outcome_ms >= p.edit_ms {
        let from = p.edit_ms.max(p.called_ms).min(p.outcome_ms);
        let (shipped, patched, end) = (p.shipped_ms.clamp(from, p.outcome_ms), p.outcome_ms, p.frame_ms.max(p.outcome_ms));
        crate::lease::prof::log_wait(&ctx.home, &format!("{{\"kind\":\"live\",\"lane\":{},\"id\":{},\"node\":{},\"start_ms\":{from},\"end_ms\":{end},\"sync_ms\":{},\"jit_ms\":{},\"run_ms\":{},\"steps\":{}}}", json_str(&x.lane), json_str(id), json_str(&r.node), shipped - from, patched - shipped, end - patched, json_str(&steps.said)));
    }
    let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
    event(
        ctx,
        if matches!(p.outcome.as_str(), "patched" | "recovering" | "jit-error" | "superseded") { Sev::Info } else { Sev::Warn },
        {
            let line = format!("live {id}: {count} file(s) {} (save → shipped {}, → verdict {}, → frame {}){}", p.outcome, after(p.shipped_ms), after(p.outcome_ms), after(p.frame_ms), if p.reason.is_empty() { String::new() } else { format!(": {}", p.reason) });
            match (p.outcome == "restart", x.restart_saves >= LOUD_SAVES) {
                (false, _) => line,
                (true, false) => format!("{line}\n{}", restart_hint(&x)),
                // said first, where it is read
                (true, true) => format!("live {id}: {}\n{line}", restart_hint(&x)),
            }
        },
        Some(&x),
    );
}

/// An edit that reached no JIT verdict (not shipped, not watched, no answer) is a hot failure:
/// it goes into the hot ledger like the JIT's own records, never only into the job's state.
/// (Refusals the JIT decides record themselves; one for a source error is the lane's change.)
fn unapplied(ctx: &Ctx, x: &Live, p: &LivePatch) {
    use makepad_remote::hot::{Outcome, Record};
    let load = format!("hot.devhub.{}.{}.{}", x.id, x.run_n, p.edit_ms);
    let r = Record {
        id: format!("{load}.1"), load, at_ms: wall_ms(), app: x.app.clone(), node: x.node.clone(), pid: x.pid,
        outcome: Outcome::Refused, reason: format!("edit not applied: {}", p.reason), failure: String::new(),
        units: Vec::new(), logs: p.files.iter().map(|f| format!("edited: {f}")).collect(),
    };
    match crate::hot::ingest(r.clone()) {
        Ok(true) => ctx.store.event("patch_failed", Sev::Warn, format!("patch_failed: {} {}: {}", r.app, r.outcome.name(), crate::proto::clean(&r.reason, 300)), Some(r.to_json())),
        Ok(false) => {}
        Err(e) => event(ctx, Sev::Warn, format!("live {}: hot ledger: {e}", x.id), Some(x)),
    }
}

/// The output of the live app's run: the log, and the JIT's pid from its announce line.
struct RunOut {
    file: std::fs::File,
    buf: Vec<u8>,
    pid: Option<u32>,
    /// the package the job runs: whose announce names the program's pid
    app: String,
    r: Arc<Running>,
    captured: bool,
}

impl std::io::Write for RunOut {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(b)?;
        self.buf.extend_from_slice(b);
        let mut consumed = 0;
        while let Some(end) = self.buf[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + end;
            let line = String::from_utf8_lossy(&self.buf[consumed..end]);
            if self.pid.is_none() {
                // `makepad-remote: pid <n> app <package> socket ..`: the program's own announce. Any
                // other package's announce never answers for the program (its verdicts and
                // patches, its crash).
                let mut words = line.split("makepad-remote: pid ").nth(1).unwrap_or("").split_whitespace();
                let pid = words.next().and_then(|n| n.parse().ok());
                if words.next() == Some("app") && words.next() == Some(self.app.as_str()) { self.pid = pid; }
            }
            if fault_line(&self.app, &line, &mut self.captured) { *self.r.fault.lock().unwrap() = Some(line.into_owned()); }
            consumed = end + 1;
        }
        self.buf.drain(..consumed);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Holds a live job whose lease and app outlived the previous master, until it ends.
fn reattach(ctx: &Arc<Ctx>, x: &Live) {
    let held = Held::attach(&x.lease, &x.node);
    let tree = PathBuf::from(&x.tree);
    // an app's ships wait for its pid, as a started session's do (the run's watcher frees them):
    // one shipped before it found no app and dropped its files
    let r = Arc::new(Running { node: x.node.clone(), lease: x.lease.clone(), held, tree, test: x.test, pending: Mutex::new((BTreeSet::new(), 0, None)), up_pid: Default::default(), calls: Mutex::new(Calls::default()), inflight: std::sync::atomic::AtomicBool::new(true), restart: std::sync::atomic::AtomicBool::new(false), resuming: std::sync::atomic::AtomicBool::new(x.test), fault: Mutex::new(None) });
    join(&ctx.monitor, &x.id, r.clone());
    set(ctx, &x.id, |s| s.touched_ms = wall_ms());
    let len = std::fs::metadata(&x.log).map_or(0, |m| m.len());
    // a run asked for and not noted: the node has it (followed), or is still getting it ready (the
    // same ask answers from that: an overlay's JIT resolution goes on), or never got the ask
    let follow = match x.run_n {
        0 => match r.held.has_run(1) {
            Ok(true) => {
                set(ctx, &x.id, |s| { s.run_n = 1; s.run_log_start = len; });
                Some((1, 0))
            }
            Ok(false) => None,
            Err(e) => return finished(ctx, &x.id, &r, Err(e)),
        },
        n => Some((n, len.saturating_sub(x.run_log_start))),
    };
    if x.patch.as_ref().is_some_and(|p| p.outcome == "recovering") {
        r.restart.store(true, std::sync::atomic::Ordering::SeqCst);
        if !stop_run(ctx, &r, x) { r.restart.store(false, std::sync::atomic::Ordering::SeqCst); }
    }
    // no run of it on the node: the previous master's ask goes on under its name
    let asked = (follow.is_none() && !x.ask.is_empty()).then(|| x.ask.clone());
    let res = run_processes(ctx, &x.id, &r, follow, asked);
    finished(ctx, &x.id, &r, res);
}

/// The live job's app ended: its lease goes and the job is over.
fn finished(ctx: &Ctx, id: &str, r: &Running, res: Result<i32, String>) {
    RUNNING.lock().unwrap().as_mut().map(|m| m.remove(id));
    let _ = ask(&ctl(), None, "release", &r.lease);
    let jit_verdict = if matches!(res, Ok(makepad_remote::hot::JIT_ERROR_EXIT)) {
        get(ctx, id).and_then(|s| std::fs::read_to_string(s.log).ok())
            .and_then(|text| makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &text)).map(|v| v.line())
    } else { None };
    let x = set(ctx, id, |s| {
        if s.active() {
            s.state = "ended".into();
        }
        s.ended_ms = wall_ms();
        if let Some(verdict) = &jit_verdict {
            s.note = verdict.clone();
            s.panic = None;
        } else if s.note.is_empty() {
            s.note = match &res {
                Ok(c) => format!("the app exited ({c})"),
                Err(e) => crate::proto::clean(e, 200),
            };
        }
    });
    let _ = std::fs::remove_dir_all(ctx.logs("live").join(format!("{id}.ship")));
    event(ctx, Sev::Info, format!("live {id} ended: {}", x.as_ref().map_or(String::new(), |x| x.note.clone())), x.as_ref());
}

/// Places the live job, starts its app, runs until it ends.
fn run(ctx: &Arc<Ctx>, id: &str, first_call: bool) {
    if !wait_slot(ctx, id) { return; }
    let Some(x) = get(ctx, id) else { return };
    // (a job released meanwhile stays released: its lease wait gives up because of that)
    let fail = |e: String| {
        let mut failed = false;
        let x = set(ctx, id, |s| {
            if !s.active() { return; }
            failed = true;
            s.state = "failed".into();
            s.ended_ms = wall_ms();
            s.note = crate::proto::clean(&e, 300);
        });
        if failed { event(ctx, Sev::Warn, format!("live {id} ({}) failed: {e}", x.as_ref().map_or("", |x| &x.app)), x.as_ref()); }
    };
    let tree = PathBuf::from(&x.tree);
    // every file saved in the tree ships to the box: a person's checkout never
    if crate::lease::ship::is_main_checkout(&tree) {
        return fail(format!("{} is a main checkout, not a lane's worktree: live ships every saved file", tree.display()));
    }
    let snapshot = (|| -> Result<_, String> {
        let head = ctx.monitor.dirty(&tree)?.head;
        let mut rev = format!("{}@{head}", x.lane);
        for name in crate::nested::names(&tree) {
            rev.push_str(&format!(",{name}={}@{}", x.lane, ctx.monitor.dirty(&tree.join(crate::nested::dir(&name)))?.head));
        }
        let pin = crate::lease::Pin::of_rev(&rev).ok_or("live: missing pin")?;
        let (mut changed, deleted) = crate::lease::ship::uncommitted(&tree, &pin)?;
        changed.extend(also_files(&tree, &x.also)?);
        changed.sort();
        changed.dedup();
        if changed.is_empty() && deleted.is_empty() { Ok((rev, String::new(), 0)) }
        else { super::job::copy_snapshot(ctx, &tree, rev, changed, deleted) }
    })();
    let (rev, snap, _) = match snapshot {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    set(ctx, id, |x| x.rev = rev.clone());
    // Overlays are keyed by lease lane: each target needs its own source state.
    let spec = Spec { since_ms: None, lane: if x.test { format!("live-{id}") } else { x.lane.clone() }, agent: x.agent.clone(), kind: if x.visible { Kind::Screen } else { Kind::Test }, needs: x.needs.clone(), prio: crate::lease::sched::DEFAULT_PRIO, rev, repo_dir: x.tree.clone(), node: (!x.pin.is_empty()).then(|| x.pin.clone()) };
    let argv: Vec<String> = ["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run"].iter().map(|s| s.to_string()).chain(std::iter::once(x.app.clone())).collect();
    let src = Src::for_run(spec.kind, &argv, x.src == "tree");
    let configure = |r: &mut crate::lease::sched::Req| {
        r.run_options(src, CostClass::parse(&x.cost), x.est_min, &x.class, &argv);
        r.cores = x.cores;
        r.ram_gb = x.ram_gb;
        r.live = true;
        r.priority = Priority::parse(&x.priority).unwrap_or(Priority::Interactive);
        r.loader = (!x.loader.is_empty()).then(|| x.loader.clone());
    };
    // A box that goes away while the sources ship is no verdict on the session (its hub restarts
    // for its self-update, most often): the lease goes back and it is placed again, as a job is.
    // So is an ask for its box whose answer was lost ([`super::leg::exit_lost`]): a live test's
    // session queued 9 min for its box failed on "beat: no answer", its test never run (W4322, 11-08).
    let mut placed = 0;
    let (held, ship_started) = loop {
        let held = match Held::acquire_with(&spec, &configure, &mut |_| {}, &|| get(ctx, id).is_none_or(|x| !x.active())) {
            Ok(h) => h,
            Err(e) if placed + 1 < super::job::ATTEMPTS && super::leg::exit_lost(&e) => {
                placed += 1;
                let why = format!("no box ({}); placed again", crate::proto::clean(&e, 160));
                let x = set(ctx, id, |s| s.note = why.clone());
                event(ctx, Sev::Warn, format!("live {id}: {why}"), x.as_ref());
                continue;
            }
            Err(e) => return fail(e),
        };
        placed += 1;
        let ship_started = wall_ms();
        let shipped = if !snap.is_empty() || src == Src::Overlay { super::job::ship_snap(ctx, &held, &snap).map(drop) } else { Ok(()) };
        let Err(e) = shipped else { break (held, ship_started) };
        let node = held.node.clone();
        held.release(None);
        if placed >= super::job::ATTEMPTS || !super::leg::exit_lost(&e) {
            return fail(format!("shipping {snap}: {e}"));
        }
        event(ctx, Sev::Warn, format!("live {id}: shipping to {node} failed ({}); placed again", crate::proto::clean(&e, 160)), Some(&x));
    };
    let sources = if snap.is_empty() { BTreeMap::new() } else {
        let dir = super::job::snap_dir(ctx, &snap);
        let captured = (|| -> Result<_, String> {
            let bytes = std::fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?;
            let v = makepad_strict_json::parse(&bytes).map_err(|e| format!("snapshot: {e:?}"))?;
            let changed = v.get("changed").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|v| v.get("path").and_then(Value::as_str).map(str::to_string)).collect::<Vec<_>>();
            let deleted = v.get("deleted").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>();
            capture(&dir.join("files"), &changed, &deleted, &BTreeMap::new(), &ctx.logs("live").join(format!("{id}.ship")))
        })();
        match captured { Ok(s) => s, Err(e) => return fail(e) }
    };
    let log = ctx.logs("live").join(format!("{id}.log"));
    let _ = std::fs::create_dir_all(ctx.logs("live"));
    let file = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return fail(format!("{}: {e}", log.display())),
    };
    let r = Arc::new(Running { node: held.node.clone(), lease: held.id.clone(), held, tree: tree.clone(), test: x.test, pending: Mutex::new((BTreeSet::new(), 0, None)), up_pid: Default::default(), calls: Mutex::new(Calls::default()), inflight: std::sync::atomic::AtomicBool::new(true), restart: std::sync::atomic::AtomicBool::new(false), resuming: std::sync::atomic::AtomicBool::new(x.test && !first_call), fault: Mutex::new(None) });
    join(&ctx.monitor, id, r.clone());
    let x = set(ctx, id, |s| {
        s.state = "running".into();
        // (a placement's "placed again" note says how it waited, not how it runs)
        s.note.clear();
        s.lease = r.lease.clone();
        s.node = r.node.clone();
        s.started_ms = wall_ms();
        s.touched_ms = wall_ms();
        s.log = log.to_string_lossy().into_owned();
        s.sources = sources;
        if s.test {
            s.patch = Some(LivePatch { files: s.sources.keys().cloned().collect(), edit_ms: ship_started, shipped_ms: s.started_ms, outcome: "initial".into(), ..Default::default() });
        }
    });
    event(ctx, Sev::Info, format!("live {id}: {} starting on {} ({}, {})", x.as_ref().map_or("", |x| &x.app), r.node, r.lease, if x.as_ref().is_some_and(|x| x.visible) { "visible" } else { "headless" }), x.as_ref());
    drop(file);
    let res = run_processes(ctx, id, &r, None, None);
    finished(ctx, id, &r, res);
}

/// `follow`: a run of the lease to follow first (number, log offset); `asked`: the name of an ask
/// a previous master sent that has no run yet, asked again under it.
fn run_processes(ctx: &Arc<Ctx>, id: &str, r: &Arc<Running>, mut follow: Option<(u64, u64)>, mut asked: Option<String>) -> Result<i32, String> {
    loop {
        let x = get(ctx, id).ok_or("live job disappeared")?;
        if !x.active() { return Ok(0); }
        let hide: &[&str] = if x.visible { &["-u", "MAKEPAD_HIDE_WINDOWS"] } else { &["MAKEPAD_HIDE_WINDOWS=1"] };
        // An overlay run's sources are a store with an edits folder, and devhub ships every save
        // into that folder: such a run is live only when asked (run.rs store_live). Without it
        // the app never watches its sources and no edit patches in.
        let live: &[&str] = if x.test { &[] } else { &["MAKEPAD_RUN_STORE_LIVE=1"] };
        let mut argv = vec!["env".to_string()];
        argv.extend(x.env.iter().map(|(k, v)| format!("{k}={v}")));
        argv.extend(hide.iter().chain(live).chain(&["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--"]).map(|s| s.to_string()));
        argv.extend(if x.test { vec!["test".into(), "--live".into(), x.app.clone()] } else { vec!["run".into(), x.app.clone()] });
        argv.extend(x.test_args.iter().cloned());
        let file = std::fs::OpenOptions::new().append(true).create(true).open(&x.log).map_err(|e| e.to_string())?;
        let log_start = file.metadata().map_err(|e| e.to_string())?.len();
        let out = RunOut { file, buf: Vec::new(), pid: follow.map(|_| x.pid).filter(|p| *p > 0), app: x.app.clone(), r: r.clone(), captured: false };
        let shared = Arc::new(Mutex::new(out));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (c2, id2, s2, d2, r2) = (ctx.clone(), id.to_string(), shared.clone(), done.clone(), r.clone());
        let watcher = std::thread::Builder::new().name("devhub-live-pid".into()).spawn(move || {
            while !d2.load(std::sync::atomic::Ordering::SeqCst) && running(&id2).is_some() {
                let pid = s2.lock().unwrap().pid;
                if let Some(pid) = pid {
                    // a new process loads the sources as they are: no save waits for a restart
                    let x = set(&c2, &id2, |s| { if !d2.load(std::sync::atomic::Ordering::SeqCst) { if s.pid != pid { restart_ran(s); } s.pid = pid; } });
                    if d2.load(std::sync::atomic::Ordering::SeqCst) { return; }
                    if x.as_ref().is_some_and(|x| x.test) { resume_tests(&c2, &id2, &r2); }
                    else {
                        if let Some(x) = x.filter(|x| x.patch.as_ref().is_some_and(|p| p.outcome == "initial")) {
                            if wait_up(&c2, &id2, &r2, pid) {
                                let mut p = x.patch.clone().unwrap();
                                p.outcome = "patched".into();
                                p.reason = "restarted on the current sources".into();
                                p.outcome_ms = wall_ms();
                                if drawing(&x) { p.frame_ms = p.outcome_ms; }
                                set_test_patch(&c2, &id2, &x, &p);
                            }
                        }
                        finish_call(&c2, &id2, &r2);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }).map_err(|e| e.to_string())?;
        struct Shared(Arc<Mutex<RunOut>>);
        impl std::io::Write for Shared {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().write(b) }
            fn flush(&mut self) -> std::io::Result<()> { self.0.lock().unwrap().flush() }
        }
        let res = match follow.take() {
            Some((n, off)) => r.held.follow(n, off, &mut Shared(shared)),
            None => {
                let ask = asked.take().unwrap_or_else(|| {
                    let ask = crate::proto::request_id();
                    set(ctx, id, |s| s.ask = ask.clone());
                    ask
                });
                r.held.run_live(&ask, &argv, &mut Shared(shared), &mut |n| { set(ctx, id, |s| { s.run_n = n; s.run_log_start = log_start; }); })
            }
        };
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        set(ctx, id, |s| s.pid = 0);
        let _ = watcher.join();
        let Some(mut x) = get(ctx, id) else { return res };
        x.load_text()?;
        if x.test && x.active() && !r.restart.load(std::sync::atomic::Ordering::SeqCst) {
            let mut p = x.patch.clone().unwrap_or_default();
            let log = run_log(r, x.run_n, p.log_start).map(|(_, log)| log).unwrap_or_default();
            let mut captured = false;
            let fault = r.fault.lock().unwrap().take().or_else(|| log.lines().find(|l| fault_line(&x.app, l, &mut captured)).map(str::to_string));
            if let Some(verdict) = makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &log) {
                p.tests = None;
                p.reason = verdict.line();
                p.outcome = "jit_error".into();
                p.verdict_ms = wall_ms();
                p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, p.reason.clone(), None);
                set_patch(ctx, id, &p);
            } else if let Some(reason) = fault {
                if begin_call(r) {
                    r.resuming.store(true, std::sync::atomic::Ordering::SeqCst);
                    // a session parking meanwhile restarts nothing: the fault is its edit's answer
                    if admit(ctx, id, r, None).is_err() {
                        r.resuming.store(false, std::sync::atomic::Ordering::SeqCst);
                        (p.tests, p.outcome, p.reason, p.verdict_ms) = (None, "failed".into(), reason, wall_ms());
                        set_patch(ctx, id, &p);
                        finish_call(ctx, id, r);
                        return res;
                    }
                }
                p.tests = None;
                p.reason = reason;
                recover(ctx, id, &x, &mut p, false);
            } else if !terminal(&p) {
                p.outcome = if !crate::jit_misses::compile_errors(&log).is_empty() { "compile_error" } else { "failed" }.into();
                p.reason = format!("live test ended without a verdict: {res:?}");
                if !log.is_empty() { p.reason.push('\n'); p.reason.push_str(&log); }
                p.verdict_ms = wall_ms();
                set_patch(ctx, id, &p);
            }
        }
        if !x.active() || !r.restart.swap(false, std::sync::atomic::Ordering::SeqCst) { return res; }
        *r.fault.lock().unwrap() = None;
        set(ctx, id, |s| {
            s.pid = 0;
            if let Some(p) = &mut s.patch {
                p.seq = 0; p.cursor.clear(); p.tests = None; p.log_start = 0; p.log_end = 0;
                p.outcome = "initial".into(); p.reason.clear(); p.reason_ref = Default::default(); p.output.clear(); p.output_ref = Default::default();
            }
        });
    }
}

/// A parked panic of a live job's app pauses it (act and resume through it).
pub fn on_record(ctx: &Ctx, r: &Record) {
    if r.kind != "panic" { return; }
    let Some(x) = ctx.store.read(|d| d.lives.iter().find(|x| x.state == "running" && x.node == r.machine && x.pid == r.pid).cloned()) else { return };
    let d = makepad_strict_json::parse(r.detail.as_bytes()).ok();
    let f = |k: &str| d.as_ref().and_then(|d| d.get(k)).and_then(|v| v.as_str()).map(str::to_string).unwrap_or_default();
    if x.test && jit_fault_of(&x.app, &format!("{} panicked at {}", f("message"), f("location"))) {
        if let Some(r) = running(&x.id) { *r.fault.lock().unwrap() = Some(format!("{}: {}", f("location"), f("message"))); }
        return;
    }
    let p = JobPanic { id: f("id"), pid: r.pid, message: f("message"), thread: f("thread"), location: f("location"), at_ms: wall_ms() };
    let x = set(ctx, &x.id, |s| {
        s.state = "paused".into();
        s.panic = Some(p.clone());
        if s.test {
            if let Some(last) = &mut s.patch { last.tests = None; last.outcome = "refused".into(); last.reason = format!("{}: {}", p.location, p.message); }
        }
        s.touched_ms = wall_ms();
    });
    if let Some(x) = x {
        event(ctx, Sev::Err, format!("live {} PAUSED on a panic: {} at {} [{}] ({}): live act {} stacks | live act {} resume {} retry", x.id, p.message, p.location, p.thread, p.id, x.id, x.id, p.id), Some(&x));
    }
}

/// A live session belongs to the lane that started it: another lane's inspect, drive, sync and
/// release calls are refused with the owner's name, unless the caller says it acts as the owner
/// (`--lane <owner>` on the CLI). A call that names no lane (an old client) passes.
fn owned(ctx: &Ctx, id: &str, caller: Option<&str>) -> Result<(), String> {
    let Some(caller) = caller else { return Ok(()) };
    match get(ctx, id) {
        Some(x) => refuse(id, &x.lane, caller),
        None => Ok(()),
    }
}

fn refuse(id: &str, owner: &str, caller: &str) -> Result<(), String> {
    if owner.is_empty() || owner.eq_ignore_ascii_case(caller) { return Ok(()); }
    Err(format!("{id} belongs to lane {owner} (you are {caller}): start your own with `devhub live start`, or pass --lane {owner} to act as its owner"))
}

#[cfg(test)]
mod join_tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    /// A master that restarted reattached its sessions, and their trees, followed by nothing,
    /// reported no save: an edit shipped only when some other call happened to follow the tree.
    #[test]
    fn a_session_that_joins_a_running_monitor_hears_its_saves_and_ships_what_it_missed() {
        let base = std::env::temp_dir().join(format!("devhub-live-join-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("lane");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        git(&tree, &["init", "-q", "-b", "rc-x"]);
        git(&tree, &["config", "user.email", "t@t"]);
        git(&tree, &["config", "user.name", "t"]);
        std::fs::write(tree.join("src/a.rs"), "a").unwrap();
        git(&tree, &["add", "."]);
        git(&tree, &["commit", "-q", "-m", "x"]);
        // the monitor ran before the session joined (the master restarted under it)
        let m = crate::monitor::Monitor::start(&[base.clone()]);
        let heard = Arc::new(Mutex::new(Vec::<String>::new()));
        let h = heard.clone();
        m.listen(Arc::new(move |_: &Path, rel: &str| h.lock().unwrap().push(rel.to_string())));
        let pending: super::Pending = Default::default();
        super::follow(&m, &tree);
        super::due(&pending, []);
        assert!(pending.lock().unwrap().2.is_some(), "a joining app session ships what was saved while none listened");
        std::fs::write(tree.join("src/a.rs"), "a2").unwrap();
        let mut got = false;
        for _ in 0..50 {
            got = heard.lock().unwrap().iter().any(|p| p == "src/a.rs");
            if got { break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&base);
        assert!(got, "a save in a joined session's tree reaches the live sessions");
    }

    /// A save in any nested repository's tree is the session's, at its path in the tree; the
    /// makepad tree's own event for it is left to that tree.
    #[test]
    fn a_save_in_a_nested_repository_is_the_sessions_at_its_tree_path() {
        let base = std::env::temp_dir().join(format!("devhub-live-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("lane");
        for d in ["apps/source-library/.git", "apps/commercial/.git", "apps/plain/src"] {
            std::fs::create_dir_all(tree.join(d)).unwrap();
        }
        let saved = |root: &Path, rel: &str| super::saved_path(&tree, root, rel);
        assert_eq!(saved(&tree, "src/a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(saved(&tree, "apps/plain/src/a.rs").as_deref(), Some("apps/plain/src/a.rs"), "a makepad app");
        assert_eq!(saved(&tree, "apps/source-library/src/a.rs"), None, "its own tree says it");
        assert_eq!(saved(&tree.join("apps/source-library"), "src/a.rs").as_deref(), Some("apps/source-library/src/a.rs"));
        assert_eq!(saved(&tree.join("apps/commercial"), "fab/a.rs").as_deref(), Some("apps/commercial/fab/a.rs"));
        assert_eq!(saved(&base.join("other"), "src/a.rs"), None, "another tree's");
        assert_eq!(saved(&tree.join("apps/source-library/src"), "a.rs"), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod owner_tests {
    /// A save whose file is gone by the ship (an editor's temp file, renamed onto the final one)
    /// ships nothing; one that shipped before and is gone ships as a deletion (W3766 shipped a
    /// renamed temp file and its edit failed, 11-06).
    #[test]
    fn a_file_gone_since_its_save_ships_nothing_new() {
        let base = std::env::temp_dir().join(format!("devhub-live-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("tree");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        std::fs::write(tree.join("src/a.rs"), "fn a() {}\n").unwrap();
        let old = std::collections::BTreeMap::from([("src/was.rs".to_string(), "false:false:00".to_string())]);
        let changed = ["src/a.rs", "src/a.rs.swp", "src/was.rs"].map(String::from);
        let sources = super::capture(&tree, &changed, &[], &old, &base.join("ship")).unwrap();
        assert_eq!(sources.keys().map(String::as_str).collect::<Vec<_>>(), ["src/a.rs", "src/was.rs"]);
        assert_eq!(sources["src/was.rs"], "-");
        assert!(base.join("ship/src/a.rs").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Saves that wait for a restart are counted from the first; from the third the hint leads
    /// with their count, past 5 min the check-in lists the session, and a restart clears it all (a
    /// lane saved into stale code for 20 min, 11-06).
    #[test]
    fn saves_waiting_for_a_restart_are_counted_and_listed() {
        use crate::state::{Live, LivePatch};
        let t0 = 1_793_713_740_000u64; // 13:49:00Z
        let mut x = Live { id: "W1".into(), app: "an-app".into(), lane: "a-lane".into(), node: "box-a".into(), state: "running".into(), ..Default::default() };
        let p = LivePatch { outcome: "restart".into(), reason: "a struct's layout changed\nmore".into(), ..Default::default() };
        super::restart_waits(&mut x, &p, t0);
        super::restart_waits(&mut x, &p, t0 + 60_000);
        assert!(super::restart_hint(&x).starts_with("restart pending: "), "{}", super::restart_hint(&x));
        super::restart_waits(&mut x, &p, t0 + 120_000);
        assert_eq!((x.restart_since_ms, x.restart_saves), (t0, 3));
        assert_eq!(super::restart_hint(&x), "3 saves since 13:49Z are not running: relaunch (`makepad devhub live act W1 session relaunch` restarts the app with all of them)");
        assert!(super::restart_rows(std::slice::from_ref(&x), t0 + 5 * 60_000).is_empty(), "not past 5 min yet");
        assert_eq!(super::restart_rows(std::slice::from_ref(&x), t0 + 18 * 60_000), ["a-lane: W1 an-app@box-a restart pending 18 min, 3 save(s) since 13:49Z not running (a struct's layout changed): `makepad devhub live act W1 session relaunch`"]);
        let ended = Live { state: "ended".into(), ..x.clone() };
        assert!(super::restart_rows(&[ended], t0 + 18 * 60_000).is_empty(), "an ended session runs nothing");
        super::restart_ran(&mut x);
        assert_eq!((x.restart_since_ms, x.restart_saves, x.restart_why.as_str()), (0, 0, ""));
        assert!(super::restart_rows(&[x], t0 + 18 * 60_000).is_empty());
    }

    /// A closed watch's verdict says the app's exit as its waiter kept it, and its log's last
    /// lines ([`super::peer_closed`]).
    #[test]
    fn a_closed_watch_says_the_apps_exit_and_last_lines() {
        assert_eq!(super::exit_said(101), "exit 101");
        assert_eq!(super::exit_said(139), "exit 139: signal 11");
        assert_eq!(super::exit_said(0xC000_0005u32 as i32 as i64), "exit 0xc0000005");
        let log = std::env::temp_dir().join(format!("devhub-live-tail-{}.log", std::process::id()));
        let lines: String = (1..=9).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&log, format!("{lines}\n   \nthread 'main' panicked at src/apply.rs:3:1\n")).unwrap();
        assert_eq!(super::log_tail(&log.to_string_lossy(), 3), "line 8 | line 9 | thread 'main' panicked at src/apply.rs:3:1");
        assert_eq!(super::log_tail("/nonexistent/devhub-live.log", 3), "");
        let _ = std::fs::remove_file(&log);
    }

    /// A call closed with its box away is a cut its caller goes on from; with its box answering it
    /// is the app's, and says how the app stands.
    #[test]
    fn a_call_closed_by_a_hub_leaving_is_a_cut_and_one_closed_by_the_app_says_so() {
        let x = super::Live { node: "boxa".into(), pid: 42, log: "/nonexistent/devhub-live.log".into(), ..Default::default() };
        let closed = "connection closed by the peer";
        let gone = super::closed_act("W1", closed, &x, super::AppSaid::NoAnswer("link lost: host `boxa` is not connected".into()));
        assert!(crate::cli::link_cut(&gone) && gone.contains("boxa's hub"), "{gone}");
        let exited = super::closed_act("W1", closed, &x, super::AppSaid::Exit(139));
        assert!(!crate::cli::link_cut(&exited) && exited.contains("the app exited (exit 139: signal 11)"), "{exited}");
        let runs = super::closed_act("W1", closed, &x, super::AppSaid::Runs);
        assert!(!crate::cli::link_cut(&runs) && runs.contains("the app runs on (pid 42)"), "{runs}");
    }

    #[test]
    fn a_park_waits_for_the_whole_call_and_any_inflight_edit() {
        let mut calls = super::Calls { count: 1, ..Default::default() };
        calls.park = Some("parked for a gate".into());
        assert_eq!(calls.ready_to_park(false), None, "a look still copying its grab holds the session");
        assert!(!calls.ended);
        calls.count -= 1;
        assert_eq!(calls.ready_to_park(true), None, "an edit in flight still holds it");
        assert_eq!(calls.ready_to_park(false).as_deref(), Some("parked for a gate"));
        assert!(calls.ended, "the session ends once the call and edit finish");
        assert_eq!(calls.ready_to_park(false), None, "release happens once");
    }

    #[test]
    fn a_grab_names_its_file_on_every_os() {
        let lines = |l: &str| vec!["{\"frame\":3}".to_string(), l.to_string()];
        assert_eq!(super::grab_file(&lines(r#"{"png":"/h/devhub/tmp/L7/g/a.png"}"#), "L7").as_deref(), Some("g/a.png"));
        assert_eq!(super::grab_file(&lines(r#"{"png":"C:\\Users\\u\\devhub\\tmp\\L7\\g\\a.png"}"#), "L7").as_deref(), Some("g/a.png"));
        assert_eq!(super::grab_file(&lines(r#"{"png":"/h/devhub/tmp/L70/g/a.png"}"#), "L7"), None);
    }

    /// A call's wait for room ends when its client leaves, however long the box stays full: no
    /// clock ends it, and it says at once what it waits for and who holds it. The wait read its
    /// client from a thread-local that an edit's ship thread never has, and said nothing (11-05).
    #[test]
    fn a_call_waiting_for_room_ends_when_its_client_leaves() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let gone = std::sync::Arc::new(AtomicBool::new(false));
        let leaves = gone.clone();
        let client = std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(300)); leaves.store(true, Ordering::SeqCst); });
        let full = "a: waiting for 6 cores (14 of 16 in use), held by L1 (test, 8), W2 (live, in a call, 6): beside the call of W2";
        let (mut tries, mut said) = (0, Vec::new());
        let r = super::wait_for_room(&mut || { tries += 1; Err(full.to_string()) },
            &|| gone.load(Ordering::SeqCst).then(|| "W1: this call's client has gone".to_string()), super::ROOM_SAY_AGAIN, &mut |why| said.push(why.to_string()));
        client.join().unwrap();
        assert_eq!(r, Err(format!("W1: this call's client has gone; it waited for {full}")), "the caller hears what it waited for");
        assert_eq!(said, [full], "said as it started, not again before its time");
        assert!(tries > 1, "it asked again until its client left");
        // room at once: in, nothing said; room after refusals: the first is what it waited for
        assert_eq!(super::wait_for_room(&mut || Ok(()), &|| None, super::ROOM_SAY_AGAIN, &mut |_| panic!("no wait")), Ok(None));
        let mut n = 0;
        assert_eq!(super::wait_for_room(&mut || { n += 1; if n < 3 { Err(format!("full {n}")) } else { Ok(()) } }, &|| None, super::ROOM_SAY_AGAIN, &mut |_| {}), Ok(Some("full 1".to_string())));
        // past its time it says what it waits on then, and still answers with the first
        let (mut n, mut said) = (0, Vec::new());
        assert_eq!(super::wait_for_room(&mut || { n += 1; if n < 3 { Err(format!("full {n}")) } else { Ok(()) } }, &|| None, std::time::Duration::ZERO, &mut |why| said.push(why.to_string())), Ok(Some("full 1".to_string())));
        assert_eq!(said, ["full 1", "full 2"]);
    }

    /// A relaunch its JIT queued in place ends when no edit waits for a restart any more (the new
    /// run loaded the sources, in the same process), or fails with the app's reason when it
    /// postponed it; until then it is on its way. The session said "queued" and stayed "restart
    /// pending" over a restart that had happened (11-06).
    #[test]
    fn a_relaunch_in_place_ends_when_the_new_run_has_the_sources() {
        let live = |pending: u32| vec![format!(r#"{{"now_ms":1.0,"frames":40,"patches":3,"frame_patch":3,"restart_pending":{pending}}}"#)];
        let postponed = "[I] platform/src/restart.rs:433:17 - restart postponed (struct `Plate` changed): unsaved: plate 1\n";
        assert_eq!(super::relaunch_end(&live(2), "makepad live: restarting (struct `Plate` changed)\n"), None, "on its way");
        assert_eq!(super::relaunch_end(&live(2), postponed), Some(Err("(struct `Plate` changed): unsaved: plate 1".to_string())));
        assert_eq!(super::relaunch_end(&live(0), postponed), Some(Ok(())), "the app allowed it after all");
        assert_eq!(super::relaunch_end(&[], ""), None, "a look that failed: looked at again");
    }

    /// Every call's reply says its wait for room: a look's, a wait's and a sync's said nothing
    /// (a lane's look waited 220 s without a word, 11-06). One that did not wait is as it was.
    #[test]
    fn every_live_reply_says_its_wait_for_room() {
        let line = "live call W1 waited 41000 ms for room on node-a: node-a: budget ram 21.6 GB, in use 8.1 GB, asked 19.4 GB";
        let sync = super::with_room(r#"{"id":"W1","queued":2}"#.to_string(), Some(line));
        let v = makepad_strict_json::parse(sync.as_bytes()).unwrap();
        assert_eq!(v.get("room").and_then(super::Value::as_str), Some(line), "{sync}");
        assert_eq!(v.get("queued").and_then(super::Value::as_u64), Some(2));
        assert_eq!(super::with_room("{}".to_string(), Some("x")), r#"{"room":"x"}"#);
        assert_eq!(super::with_room(r#"{"id":"W1"}"#.to_string(), None), r#"{"id":"W1"}"#);
    }

    #[test]
    fn a_wait_for_the_patch_waits_out_a_save_the_monitor_has_not_heard_yet() {
        use std::time::Duration;
        let (soon, heard) = (Duration::from_millis(100), Duration::from_millis(301));
        // a session's first edit: no patch yet when the wait starts
        assert!(!super::patched_done(None, 1_000, soon));
        assert!(super::patched_done(None, 1_000, heard), "no save at all: nothing to wait for");
        let patch = |edit_ms: u64, outcome: &str| crate::state::LivePatch { edit_ms, outcome: outcome.into(), ..Default::default() };
        assert!(!super::patched_done(Some(&patch(900, "")), 1_000, heard), "no verdict yet");
        assert!(super::patched_done(Some(&patch(900, "patched")), 1_000, soon), "the save just before the call");
        assert!(!super::patched_done(Some(&patch(100, "patched")), 1_000, soon), "an older edit: a newer save may come");
        assert!(super::patched_done(Some(&patch(100, "patched")), 1_000, heard));
    }

    #[test]
    fn the_jit_is_idle_by_its_activity_line_not_its_header() {
        let answer = |line: &str| vec![r#"{"measured":"JIT compile activity","at_ms":1,"source":"two looks","partial":[]}"#.to_string(), line.to_string(), "(101 ms)".to_string()];
        assert!(super::jit_idle(&answer("idle; running: nothing; queued: ahead 0, O2 0, O3 0")));
        assert!(!super::jit_idle(&answer("busy (background); running: 1 O2 builds")));
    }

    #[test]
    fn another_lanes_session_is_refused_with_the_owners_name() {
        let e = super::refuse("W257", "fablook3", "smallfix").unwrap_err();
        assert!(e.contains("W257") && e.contains("fablook3") && e.contains("--lane fablook3"), "{e}");
        assert!(super::refuse("W257", "fablook3", "FabLook3").is_ok());
        assert!(super::refuse("W1", "", "x").is_ok());
    }

    #[test]
    fn an_edit_that_waits_for_a_restart_says_what_restarts_it() {
        let mut x = crate::state::Live { id: "W7".into(), node: "box-a".into(), src: "overlay".into(), state: "running".into(), lane: "lane-x".into(), agent: "AGENT-X".into(), app: "app-x".into(), patch: Some(crate::state::LivePatch { files: vec!["src/a.rs".into()], outcome: "restart".into(), reason: "2 files changed: the Splash VM or other provided sources changed; needs a JIT rebuild: restart it through cargo; keeping the running code".into(), ..Default::default() }), ..Default::default() };
        let text = super::render(&x);
        assert!(text.contains("`makepad devhub live act W7 session relaunch`"), "{text}");
        assert!(text.starts_with("W7 running app-x of lane-x (AGENT-X) on box-a pid 0"), "a session's line names whose it is: {text}");
        let p = x.patch.as_ref().unwrap();
        assert!(super::needs_devhub_rebuild(&x, p));
        assert!(!super::rebuild_tests(&x, p), "an app's state stays until its lane asks");
        assert!(super::devhub_relaunch(&x, "session relaunch"), "the node's host cannot rebuild its store binary");
        assert!(!super::devhub_relaunch(&x, "session"), "inspection does not restart");
        x.test = true;
        x.patch.as_mut().unwrap().outcome = "refused".into();
        assert!(super::rebuild_tests(&x, x.patch.as_ref().unwrap()), "a refused provided-source edit restarts tests without waiting for a verdict from stale code");
        // a tree's JIT is the box's binary for the lane tree's inputs: its own rebuild built the
        // box's checkout, never the lane's tree
        x.src = "tree".into();
        assert!(super::rebuild_tests(&x, x.patch.as_ref().unwrap()), "a tree's picked JIT is resolved again for the lane's tree");
        x.test = false;
        assert!(super::devhub_relaunch(&x, "session relaunch"), "a lane tree's relaunch resolves its JIT for the tree");
        x.test = true;
        x.src = "overlay".into();
        x.loader = "0123456789abcdef01234567".into();
        assert!(!super::rebuild_tests(&x, x.patch.as_ref().unwrap()), "a session pinned to a JIT keeps it");
        x.test = false;
        assert!(!super::devhub_relaunch(&x, "session relaunch"), "its relaunch goes to its host");
        (x.test, x.loader) = (true, String::new());
        x.patch.as_mut().unwrap().reason = "type mismatch".into();
        assert!(!super::rebuild_tests(&x, x.patch.as_ref().unwrap()), "a source error is still a refusal");
    }

    #[test]
    fn a_log_window_ends_after_its_last_whole_line() {
        // read from 100 to 140: the next window starts where the half-written line does
        let bytes = b"test a ... ok\nverdict\nmakepad live:   struct ";
        assert_eq!(super::whole_lines_end(100 + bytes.len() as u64, bytes), 100 + 22);
        assert_eq!(super::whole_lines_end(120, b"no newline yet"), 106);
        assert_eq!(super::whole_lines_end(120, b"done\n"), 120);
    }

    #[test]
    fn a_ship_error_that_no_edit_gets_past_ends_the_session() {
        assert!(super::session_over("box ship-have: no lease L303488 on box", "L303488").is_some());
        assert!(super::session_over("box ship-have: no lease L3034881 on box", "L303488").is_none());
        assert!(super::session_over("live: inputs: this run watches no sources (not a live session, or its watcher ended)", "L1").is_some());
        assert!(super::session_over("live: inputs did not return the complete source list", "L1").is_none());
    }

    #[test]
    fn only_a_save_that_ships_cuts_an_older_edits_verdict_wait_short() {
        let pending = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let sources: std::collections::BTreeMap<String, String> = [("src/a.rs".to_string(), "h".to_string())].into();
        let dirty = |changed: &[&str], deleted: &[&str]| -> Result<crate::monitor::Dirty, String> {
            Ok(crate::monitor::Dirty { changed: pending(changed), deleted: pending(deleted), ..Default::default() })
        };
        let hash = |h: &'static str| move |_: &str| -> Result<String, String> { Ok(h.into()) };
        // a file the session shipped before, saved with other bytes: no need to ask git
        assert!(super::a_save_ships(&pending(&["src/a.rs"]), &sources, hash("h2"), || panic!("asked git")));
        // ... or gone
        assert!(super::a_save_ships(&pending(&["src/a.rs"]), &sources, |_| Err("gone".into()), || panic!("asked git")));
        // the same bytes again (macOS repeats a save's event ~100 ms later): no ship
        assert!(!super::a_save_ships(&pending(&["src/a.rs"]), &sources, hash("h"), || panic!("asked git")));
        // a new change git sees, or a tracked file gone
        assert!(super::a_save_ships(&pending(&["src/b.rs"]), &sources, hash("h"), || dirty(&["src/b.rs"], &[])));
        assert!(super::a_save_ships(&pending(&["src/c.rs"]), &sources, hash("h"), || dirty(&[], &["src/c.rs"])));
        // an ignored write (git does not list it) or a path that never ships
        assert!(!super::a_save_ships(&pending(&["local/x.py"]), &sources, hash("h"), || dirty(&["src/b.rs"], &[])));
        assert!(!super::a_save_ships(&pending(&["app/target/x"]), &sources, hash("h"), || dirty(&["app/target/x"], &[])));
        // git unreadable: it may ship
        assert!(super::a_save_ships(&pending(&["src/d.rs"]), &sources, hash("h"), || Err("no tree".into())));
    }
}

fn act(ctx: &Arc<Ctx>, id: &str, arg: &str, held: Option<&Call<'_>>) -> Result<String, String> {
    let x = get(ctx, id).filter(|x| x.active()).ok_or_else(|| not_running(ctx, id))?;
    if x.pid == 0 {
        return Err(format!("{id}: its JIT is not up yet (live wait {id} frame)"));
    }
    try_set(ctx, id, |s| s.touched_ms = wall_ms())?;
    let t = Instant::now();
    // a grab's file: (node, lease, path under the lease's dir), for `live look` to fetch
    let mut png = None;
    let mut lines = match arg.split_whitespace().next() {
        _ if devhub_relaunch(&x, arg) => relaunch(ctx, id)?,
        // images come from the app's frames, answered by a client on its box (the `cmd`
        // service has nothing for them): the file stays there, in the lease's tmp
        Some(verb @ ("grab" | "gseq" | "gq" | "rec")) if !arg.contains(" save") && !arg.contains(" status") => {
            let r = running(id).ok_or_else(|| format!("{id}: not running here"))?;
            let pid = x.pid.to_string();
            // the patches installed before the grab is asked for: its frame must show them all
            let live = inspect(&x.node, x.pid, "live").ok();
            let installed = live.as_ref().and_then(|l| l.iter().find_map(|l| num(l, "patches")));
            // saved edits that wait for a restart: the frame shows the code before them
            let waiting = live.as_ref().and_then(|l| l.iter().find_map(|l| num(l, "restart_pending"))).unwrap_or(0);
            // a save not shipped yet or without its verdict: nor does it show that one
            let unpatched = !r.pending.lock().unwrap().0.is_empty() || r.inflight.load(std::sync::atomic::Ordering::SeqCst) || x.patch.as_ref().is_some_and(|p| p.outcome.is_empty());
            let mut argv = vec!["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "inspect", &pid];
            argv.extend(arg.split_whitespace());
            let (code, text) = r.held.output(&argv)?;
            if code != 0 {
                return Err(format!("{id}: {arg} on {}: exit {code}: {}", x.node, crate::proto::clean(&text, 300)));
            }
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            // a grab after a patch is a frame drawn with it: one drawn before is no answer
            // (a stale look sent an agent after a wrong cause for hours, 11-04)
            let drawn = lines.iter().find(|l| l.contains("\"png\"")).and_then(|l| Some((num(l, "frame")?, num(l, "patch")?)));
            if verb == "grab" {
                match (drawn, installed) {
                    (Some((frame, patch)), Some(installed)) if patch < installed => {
                        return Err(format!("{id}: the grab's frame {frame} was drawn with {patch} live patch(es) of {installed}: no frame after the newest patch"));
                    }
                    (Some(_), Some(_)) => {}
                    // a session built before grabs were stamped (its app or JIT says nothing)
                    _ => lines.push(format!("unverified: this session's build does not stamp a grab's frame and patches: it may show a frame from before the newest patch (start a new session)")),
                }
            }
            if waiting > 0 {
                lines.insert(0, format!("STALE: {waiting} edit(s) wait for a restart: this frame shows the code before them (`makepad devhub live act {id} session relaunch` applies them)"));
            }
            if unpatched {
                lines.insert(0, format!("STALE: a save waits to ship or for its verdict: this frame shows the code before it (`makepad devhub live look {id}` waits for it)"));
            }
            if let Some(rel) = grab_file(&lines, &r.lease) {
                lines.push(format!("copy it here: makepad devhub get {} tmp/{rel} <dir> --node {} --lease {} (or `live look {id}`)", x.lane, x.node, r.lease));
                let drawn = drawn.map_or(String::new(), |(frame, patch)| format!(",\"frame\":{frame},\"patch\":{patch}"));
                png = Some(format!("{{\"node\":{},\"lease\":{},\"rel\":{}{drawn}}}", json_str(&x.node), json_str(&r.lease), json_str(&format!("tmp/{rel}"))));
            }
            lines
        }
        // the app's AI tools: its JSON arguments as typed
        Some("call" | "tools") => inspect(&x.node, x.pid, arg)?,
        // code (`eval <expr>`, `probe add <hook> <code>`) goes as JSON, as `makepad
        // inspect` sends it: typed, its `=` and `=>` were taken for `key=value` words
        _ if arg.trim_start().starts_with('{') => inspect(&x.node, x.pid, arg)?,
        _ => inspect(&x.node, x.pid, &makepad_remote::app::typed_line(&arg.split_whitespace().map(str::to_string).collect::<Vec<_>>()))?,
    };
    if let Some(waited) = held.and_then(|c| c.3.clone()) { lines.insert(0, waited); }
    if arg.starts_with("resume ") {
        try_set(ctx, id, |s| {
            if s.state == "paused" {
                s.state = "running".into();
            }
        })?;
    }
    Ok(format!("{{\"ms\":{},\"png\":{},\"lines\":[{}]}}", t.elapsed().as_millis(), png.as_deref().unwrap_or("null"), lines.iter().map(|l| json_str(l)).collect::<Vec<_>>().join(",")))
}

fn wait<'a>(ctx: &'a Arc<Ctx>, id: &'a str, arg: &str, held: &mut Option<Call<'a>>) -> Result<String, String> {
    let mut a = arg.split_whitespace();
    let what = a.next().unwrap_or("patched");
    let timeout = Duration::from_secs(a.next().and_then(|t| t.parse().ok()).unwrap_or(120));
    let t = Instant::now();
    let asked = wall_ms();
    let x = try_set(ctx, id, |s| s.touched_ms = wall_ms())?.ok_or_else(|| format!("no live job {id}"))?;
    if what == "verdict" && !x.test { return Err(format!("{id}: verdict needs a live test job (--test)")); }
    let signal = ctx.store.subscribe("live", &[id]);
    loop {
        let seen = signal.generation();
        if held.is_none() { *held = call(ctx, id)?; }
        // a test session ships its saves on its next call (`live sync`), never on a wait:
        // waiting for them waited to the timeout; the answer says they are there
        let (quiet, unshipped) = if let Some(r) = running(id) {
            let pending = r.pending.lock().unwrap();
            let inflight = r.inflight.load(std::sync::atomic::Ordering::SeqCst);
            if r.test { (!inflight, pending.0.len()) } else { (pending.0.is_empty() && !inflight, 0) }
        } else { (true, 0) };
        let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
        if !x.active() {
            return Ok(crate::state::live_value(&x).to_json());
        }
        let done = match what {
            // the newest edit has its verdict: nothing waits to ship or for a verdict, and an
            // edit saved before this call (the OS monitor hears it within ~0.1 s) is in
            "patched" => quiet && patched_done(x.patch.as_ref(), asked, t.elapsed()),
            "verdict" => quiet && x.patch.as_ref().is_some_and(|p| terminal(p)) && t.elapsed() >= Duration::from_millis(300),
            "settled" => settled(&x),
            "frame" => drawing(&x),
            w => return Err(format!("live wait: `{w}` (patched verdict settled frame)")),
        };
        if done || x.state == "paused" {
            let answer = crate::state::live_value(&x).to_json();
            return Ok(match answer.strip_suffix('}') { Some(head) if unshipped > 0 => format!("{head},\"unshipped\":{unshipped}}}"), _ => answer });
        }
        if t.elapsed() > timeout {
            return Err(format!("live wait {id} {what}: not within {} s", timeout.as_secs()));
        }
        // Observe frame/idle transitions on the session's inspection deadline;
        // document and admission changes wake just this session immediately.
        let deadline = if t.elapsed() < Duration::from_millis(300) { Duration::from_millis(300) - t.elapsed().min(Duration::from_millis(300)) } else { Duration::from_millis(250) };
        signal.wait(seen, deadline.min(timeout.saturating_sub(t.elapsed())));
    }
}

/// Hold the session through both waits, the grab and its copy off the lease's scratch dir.
fn look(ctx: &Arc<Ctx>, id: &str, timeout: &str) -> Result<String, String> {
    let mut held = call(ctx, id)?;
    let mut x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
    if x.test { return Err(format!("{id} runs tests: live wait {id} verdict")); }
    for what in ["patched", "frame"] {
        let answer = wait(ctx, id, &format!("{what} {timeout}"), &mut held)?;
        x = crate::state::live_from(&makepad_strict_json::parse(answer.as_bytes()).map_err(|e| e.to_string())?);
        if x.state != "running" {
            return Ok(with_room(format!("{{\"live\":{answer},\"path\":null,\"lines\":[]}}"), room_of(&held)));
        }
    }
    let answer = act(ctx, id, "grab", held.as_ref())?;
    let v = makepad_strict_json::parse(answer.as_bytes()).map_err(|e| e.to_string())?;
    let png = v.get("png");
    let s = |k: &str| png.and_then(|p| p.get(k)).and_then(Value::as_str).unwrap_or("");
    let lines = v.get("lines").cloned().unwrap_or(Value::Arr(Vec::new()));
    if s("rel").is_empty() {
        return Err(format!("the grab named no file: {}", lines.as_arr().unwrap_or(&[]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | ")));
    }
    let dir = ctx.repo.join("local/screencap").join(&x.lane);
    let got = ctx.leases.fetch(s("node"), &x.lane, s("lease"), s("rel"), &dir)?;
    let n = std::fs::read_dir(&dir).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{id}-"))).count()).unwrap_or(0);
    let drawn = png.and_then(|p| Some(format!("-f{}-p{}", crate::lease::num(p.get("frame")?)? as u64, crate::lease::num(p.get("patch")?)? as u64))).unwrap_or_default();
    let path = dir.join(format!("{id}-{}{drawn}.png", n + 1));
    std::fs::rename(&got[0].0, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(with_room(format!("{{\"live\":{},\"path\":{},\"lines\":{}}}", crate::state::live_value(&x).to_json(), json_str(&path.to_string_lossy()), lines.to_json()), room_of(&held)))
}

/// Shared by the CLI and the master; source and visibility settings belong to devhub.
pub(super) fn validate_env(k: &str, v: &str) -> Result<(), String> {
    if k.is_empty() || k.as_bytes()[0].is_ascii_digit() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("live start: --env name must use ASCII letters, digits and _, and not start with a digit".into());
    }
    if v.contains(['\n', '\r']) { return Err("live start: --env value must not contain a newline".into()); }
    // Lease argv is NUL-separated; a value must remain one argument.
    if v.contains('\0') { return Err("live start: --env value must not contain NUL".into()); }
    if matches!(k, "MAKEPAD_RUN_VFS" | "MAKEPAD_RUN_WORKSPACE" | "MAKEPAD_RUN_EDITS" | "MAKEPAD_RUN_HOME" | "CARGO_TARGET_DIR" | "MAKEPAD_HIDE_WINDOWS" | "MAKEPAD_RUN_STORE_LIVE") {
        return Err(format!("live start: --env {k} is managed by devhub"));
    }
    Ok(())
}

/// `live start {"app","repo_dir","tree"?,"lane","agent","needs","node"?,"visible","test","test_args","env"?}`, `live act <id> <cmd>`,
/// `live wait <id> patched|verdict|settled|frame [timeout_s]`, `live sync|release|get <id>`, `live list [all]`.
pub fn serve(ctx: &Arc<Ctx>, line: &str) -> Result<String, String> {
    // `live --as <lane> <verb> ..`: the calling lane (see [`owned`])
    let (line, caller) = match line.strip_prefix("live --as ").and_then(|r| r.split_once(' ')) {
        Some((lane, rest)) => (format!("live {rest}"), Some(lane.to_string())),
        None => (line.to_string(), None),
    };
    let line = line.as_str();
    let mut w = line.splitn(3, ' ');
    let _ = w.next();
    let verb = w.next().unwrap_or("list");
    let rest = w.next().unwrap_or("").trim();
    let (id, arg) = rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b.trim()));
    if matches!(verb, "act" | "wait" | "look" | "sync" | "release" | "again") {
        owned(ctx, id, caller.as_deref())?;
    }
    match verb {
        "start" | "test" => {
            let starting = STARTING.lock().unwrap();
            let v = makepad_strict_json::parse(rest.as_bytes()).map_err(|_| "live start: bad request")?;
            let mut env = Vec::new();
            if let Some(value) = v.get("env") {
                let malformed = "live start: env must be an array of [key, value] string pairs";
                for pair in value.as_arr().ok_or(malformed)? {
                    let [k, v] = pair.as_arr().ok_or(malformed)? else { return Err(malformed.into()) };
                    let (k, v) = (k.as_str().ok_or(malformed)?, v.as_str().ok_or(malformed)?);
                    validate_env(k, v)?;
                    env.push((k.to_string(), v.to_string()));
                }
            }
            // `DEVHUB_ALSO`: named paths ship with every patch, `local/` included
            let also: Vec<String> = v.get("also").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|a| a.as_str().map(str::to_string).ok_or("live: also must be strings")).collect::<Result<_, _>>()?;
            if let Some(p) = also.iter().find(|p| !crate::lease::ship::inside(p) || crate::lease::ship::never_ships_named(p)) {
                return Err(format!("DEVHUB_ALSO path never ships: {p}"));
            }
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            // Older clients sent the path as `tree`; new clients reserve it for the opt-out.
            let tree = v.get("repo_dir").and_then(Value::as_str).or_else(|| v.get("tree").and_then(Value::as_str)).unwrap_or("").to_string();
            let (app, lane) = (s("app"), makepad_devhub_conf::lane_name(&s("lane")));
            let src = if v.get("tree").and_then(Value::as_bool).unwrap_or(false) { Src::Tree } else { Src::Overlay };
            let test = verb == "test" || v.get("test").and_then(Value::as_bool).unwrap_or(false);
            let mut test_args: Vec<String> = v.get("test_args").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|a| a.as_str().map(str::to_string).ok_or("live: test_args must be strings")).collect::<Result<_, _>>()?;
            {
                let select = &test_args[..test_args.iter().position(|a| a == "--").unwrap_or(test_args.len())];
                if test {
                    if !select.is_empty() && !matches!(select, [f] if f == "--lib") && !matches!(select, [f, t] if (f == "--bin" || f == "--test") && !t.is_empty() && !t.starts_with('-')) {
                        return Err("live: select --lib, --bin B or --test T before the libtest -- arguments".into());
                    }
                } else if !select.is_empty() && !matches!(select, [f, t] if f == "--bin" && !t.is_empty() && !t.starts_with('-')) {
                    return Err("live: a program takes --bin B before its -- arguments; a test selection needs test=true".into());
                }
            }
            if test_args.last().is_some_and(|a| a == "--") { test_args.pop(); }
            // a registry name (`calculator`) runs as its package (`makepad-app-calculator`)
            let app = if test { app } else { makepad_app_registry::find(&app).map_or(app, |e| e.package.to_string()) };
            if app.is_empty() || app.starts_with('-') || !app.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Err(format!("live: `{app}` is not a package name"));
            }
            if !Path::new(&tree).join(".git").exists() {
                return Err(format!("live: `{tree}` is not a worktree"));
            }
            if !makepad_devhub_conf::valid_name(&lane) {
                return Err(format!("live: lane `{lane}` is not a name"));
            }
            if let Some(old) = ctx.store.read(|d| d.lives.iter().find(|x| x.active() && x.lane == lane && x.app == app && x.test == test && x.test_args == test_args && accepting(&x.id)).cloned()) {
                if verb == "test" {
                    if old.tree != tree { return Err(format!("{} belongs to worktree {}", old.id, old.tree)); }
                    if old.src != src.as_str() { return Err(format!("{} uses {}; live release {} before changing its source mode", old.id, old.src, old.id)); }
                    if old.also != also { return Err(format!("{} ships DEVHUB_ALSO `{}`; live release {} before changing it", old.id, old.also.join(" "), old.id)); }
                    drop(starting);
                    return match test_call(ctx, &old.id)? { Some(answer) => Ok(answer), None => serve(ctx, line) };
                }
                return Err(format!("{} already runs {app} for lane {lane} on {} (live release {} first)", old.id, old.node, old.id));
            }
            // a lane keeps LANE_SESSIONS headless: the new one ends its least recently used idle one,
            // or waits for a slot (lanes held 7 and 8 sessions past the lane cap, 11-02); a visible
            // one is the person's
            let visible = v.get("visible").and_then(|b| b.as_bool()).unwrap_or(false);
            let mine: Vec<Live> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active() && x.lane == lane).cloned().collect());
            let waits = match slot(&mine, None, visible, idle_now) {
                Slot::Free => false,
                Slot::Takes(lru) => {
                    event(ctx, Sev::Info, format!("live {} ({}) of lane {lane}: released for its lane's new session (at most {LANE_SESSIONS} headless per lane)", lru.id, lru.app), Some(lru));
                    release(ctx, &lru.id, &format!("released: lane {lane}'s new session took its place (at most {LANE_SESSIONS} headless per lane)"))?;
                    false
                }
                Slot::Waits => true,
            };
            let priority = match s("priority").as_str() { "" => Priority::Interactive, p => Priority::parse(p).ok_or("priority is interactive or background")? };
            let cost = s("cost");
            if !cost.is_empty() && CostClass::parse(&cost).is_none() { return Err("cost is small, medium or large".into()); }
            let est_min = v.get("est_min").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|m| m.is_finite() && *m >= 0.0).ok_or("est_min is a nonnegative number")).transpose()?;
            if verb == "test" && ((!cost.is_empty() && cost != "small") || est_min.is_some()) { return Err("test --live uses small cost on a hot box".into()); }
            let cores = v.get("cores").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("cores is a positive number")).transpose()?;
            let ram_gb = v.get("ram_gb").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("ram_gb is a positive number")).transpose()?;
            let x = ctx.store.update(|d| {
                d.lives_next += 1;
                let x = Live { id: format!("W{}", d.lives_next), priority: priority.as_str().into(), loader: s("loader"), src: src.as_str().into(), cost: cost.as_str().into(), est_min, cores, ram_gb, class: s("class"), app: app.clone(), test, test_args, env, also, lane: lane.clone(), agent: s("agent"), tree: tree.clone(), needs: s("needs"), pin: s("node"), visible, state: "starting".into(), touched_ms: wall_ms(), slot_wait_ms: if waits { wall_ms() } else { 0 }, note: if waits { slot_note(&lane) } else { String::new() }, ..Default::default() };
                d.lives.push(x.clone());
                x
            })?;
            drop(starting);
            event(ctx, Sev::Info, if waits { format!("live {}: {app} for lane {lane} {}", x.id, x.note) } else { format!("live {}: {app} for lane {lane} asks for a box", x.id) }, Some(&x));
            let (c, id, first_call) = (ctx.clone(), x.id.clone(), verb == "test");
            if let Err(e) = std::thread::Builder::new().name("devhub-live".into()).spawn(move || run(&c, &id, first_call)) {
                try_set(ctx, &x.id, |s| { s.state = "failed".into(); s.note = e.to_string(); })?;
            }
            if first_call { initial_call(ctx, &x.id) }
            else { Ok(format!("{{\"id\":{},\"app\":{},\"state\":\"starting\",\"note\":{}}}", json_str(&x.id), json_str(&app), json_str(&x.note))) }
        }
        "act" => {
            let held = call(ctx, id)?;
            act(ctx, id, arg, held.as_ref()).map_err(|e| match (crate::lease::master::unreachable(&e) && !crate::cli::link_cut(&e), running(id), get(ctx, id)) {
                (true, Some(r), Some(x)) => closed_act(id, &e, &x, app_said(&r, &x)),
                _ => e,
            })
        }
        "wait" => {
            let mut held = call(ctx, id)?;
            let answer = wait(ctx, id, arg, &mut held)?;
            Ok(with_room(answer, room_of(&held)))
        }
        "look" => look(ctx, id, arg),
        "sync" => {
            let x = try_set(ctx, id, |s| s.touched_ms = wall_ms())?.filter(|x| x.active()).ok_or_else(|| not_running(ctx, id))?;
            if x.test { return test_call(ctx, id)?.ok_or_else(|| format!("{id}: parked; take a new live job")); }
            let held = call(ctx, id)?;
            let r = running(id).ok_or_else(|| format!("{id} has no box yet"))?;
            let tree = PathBuf::from(&x.tree);
            let mut files = Vec::new();
            let d = ctx.monitor.dirty(&tree)?;
            files.extend(d.changed.into_iter().chain(d.deleted));
            files.extend(nested_dirty(ctx, &tree)?);
            let n = files.len();
            due(&r.pending, files);
            Ok(with_room(format!("{{\"id\":{},\"queued\":{n}}}", json_str(id)), room_of(&held)))
        }
        "release" => {
            release(ctx, id, "released")?;
            Ok(format!("{{\"id\":{},\"state\":\"released\"}}", json_str(id)))
        }
        // An ended session (parked for another ask's RAM, idle, released) starts again as it was
        // asked: its app or tests, box, env, source mode and priority, as a new session.
        "again" => {
            let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
            if x.active() { return Err(format!("{id} still runs ({}): nothing to start again", x.state)); }
            let strs = |v: &[String]| format!("[{}]", v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","));
            let env = x.env.iter().map(|(k, v)| format!("[{},{}]", json_str(k), json_str(v))).collect::<Vec<_>>().join(",");
            let req = format!(
                "{{\"app\":{},\"repo_dir\":{},\"tree\":{},\"lane\":{},\"agent\":{},\"needs\":{},\"node\":{},\"visible\":{},\"test\":{},\"test_args\":{},\"env\":[{env}],\"also\":{},\"priority\":{},\"cost\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"class\":{},\"loader\":{}}}",
                json_str(&x.app), json_str(&x.tree), x.src == Src::Tree.as_str(), json_str(&x.lane), json_str(&x.agent), json_str(&x.needs), json_str(&x.pin), x.visible, x.test,
                strs(&x.test_args), strs(&x.also), json_str(&x.priority), json_str(&x.cost), x.est_min.map_or("null".to_string(), |m| m.to_string()), x.cores.map_or("null".to_string(), |n| n.to_string()), x.ram_gb.map_or("null".to_string(), |n| n.to_string()), json_str(&x.class), json_str(&x.loader),
            );
            serve(ctx, &format!("live {} {req}", if x.test { "test" } else { "start" }))
        }
        // `devhub clean <lane>`: the lane is done, so are its live jobs
        "end-lane" => {
            if !makepad_devhub_conf::valid_name(rest) { return Err(format!("live end-lane: `{rest}` is not a lane")); }
            let ids: Vec<String> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active() && x.lane == rest).map(|x| x.id.clone()).collect());
            for id in &ids {
                event(ctx, Sev::Info, format!("live {id} of lane {rest}: released, the lane was cleaned"), get(ctx, id).as_ref());
                release(ctx, id, "released: its lane was cleaned")?;
            }
            Ok(format!("{{\"released\":[{}]}}", ids.iter().map(|i| json_str(i)).collect::<Vec<_>>().join(",")))
        }
        "get" => get(ctx, id).map(|x| crate::state::live_value(&x).to_json()).ok_or_else(|| format!("no live job {id}")),
        "list" => {
            let all = rest == "all";
            // each with what devhub tells its owner of its lease (a call far past its measure)
            let list: Vec<String> = ctx.store.read(|d| d.lives.iter().filter(|x| all || x.active()).cloned().collect::<Vec<_>>()).iter().map(|x| {
                let mut v = crate::state::live_value(x);
                if let (Value::Obj(fields), Some(word)) = (&mut v, (!x.lease.is_empty()).then(|| ctx.leases.owner_word(&x.lease)).flatten()) {
                    fields.push(("word".into(), Value::Str(word)));
                }
                v.to_json()
            }).collect();
            Ok(format!("[{}]", list.join(",")))
        }
        _ => Err(format!("live: unknown word `{verb}` (start act wait sync release again end-lane get list)")),
    }
}

/// The first caller owns the initial run until its verdict is copied.
/// A grab's png under its lease's tmp dir, from the lines that name it (a JSON string; Windows
/// paths come with escaped backslashes).
fn grab_file(lines: &[String], lease: &str) -> Option<String> {
    let tmp = format!("/tmp/{lease}/");
    lines.iter().find_map(|l| {
        let l = l.replace("\\\\", "/");
        let path = l.split('"').find(|w| w.contains(&tmp) && w.ends_with(".png"))?;
        path.split(&tmp).nth(1).map(str::to_string)
    })
}

fn initial_call(ctx: &Ctx, id: &str) -> Result<String, String> {
    let signal = ctx.store.subscribe("live", &[id]);
    loop {
        let seen = signal.generation();
        let x = get(ctx, id).ok_or("live job disappeared")?;
        if !x.active() || x.state == "paused" || x.patch.as_ref().is_some_and(terminal) {
            let answer = crate::state::live_value(&x).to_json();
            try_set(ctx, id, |s| s.touched_ms = wall_ms())?;
            if let Some(r) = running(id) { finish_call(ctx, id, &r); }
            return Ok(answer);
        }
        signal.wait(seen, Duration::from_secs(1));
    }
}

/// What a call ships, with its edit time and the tree's uncommitted state: every save the
/// session has not run, the pending ones taken.
fn call_saves(ctx: &Ctx, r: &Running, x: &Live) -> Result<(Vec<String>, u64, (Vec<String>, Vec<String>)), String> {
    // The monitor's dirty set is relative to HEAD. Previously shipped paths also catch restores
    // to HEAD and deleted untracked files; capture compares their content with sources, so
    // unchanged files need no ship or verdict.
    let mut files: BTreeSet<String> = x.sources.keys().cloned().collect();
    let d = ctx.monitor.dirty(&r.tree)?;
    files.extend(d.changed.into_iter().chain(d.deleted));
    files.extend(nested_dirty(ctx, &r.tree)?);
    let edit_ms = {
        let mut pending = r.pending.lock().unwrap();
        let edit_ms = if pending.0.is_empty() { wall_ms() } else { pending.1 };
        files.extend(std::mem::take(&mut pending.0));
        pending.2 = None;
        edit_ms
    };
    // Reuse the ship manifest's fixed-pin scan here: HEAD may have moved, or the OS may not have
    // delivered a save yet. Neither makes a call a no-op.
    let pin = crate::lease::Pin::of_rev(&x.rev).ok_or("live: missing pin")?;
    let current = crate::lease::ship::uncommitted(&r.tree, &pin)?;
    files.extend(current.0.iter().chain(&current.1).cloned());
    files.retain(|p| !crate::lease::ship::never_ships(p));
    files.extend(also_files(&r.tree, &x.also)?);
    Ok((files.into_iter().collect(), edit_ms, current))
}

/// Holds shipping until this call has copied the verdict for its exact source state.
fn test_call(ctx: &Arc<Ctx>, id: &str) -> Result<Option<String>, String> {
    let called = wall_ms();
    try_set(ctx, id, |s| s.touched_ms = called)?;
    let signal = ctx.store.subscribe("live", &[id]);
    // the session's previous call, whose tests run: this one ships when their verdict comes
    // (the JIT applies an edit only after the running tests end)
    let mut behind = None;
    loop {
        // a call still queued whose client has gone ships nothing: nobody asked for that edit any
        // more (a stopped client's call shipped a save made after it stopped, 11-05)
        if makepad_remote::lines::client_gone() { return Err(format!("{id}: this call's client has gone")); }
        let seen = signal.generation();
        let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
        // (a session that ended has no verdict on what the caller has now: a new one does)
        if !accepting(id) || x.note.starts_with("parked for ") || x.state == "ended" { return Ok(None); }
        if !x.active() || x.state == "paused" { return Ok(Some(crate::state::live_value(&x).to_json())); }
        if let Some(r) = running(id) {
            if !begin_call(&r) {
                if behind.is_none() {
                    behind = Some(x.patch.as_ref().map_or_else(|| "the session's first run".to_string(), |p| {
                        let at = p.edit_ms / 1000 % 86_400;
                        format!("the tests of the edit saved at {:02}:{:02}:{:02}Z{}", at / 3600, at / 60 % 60, at % 60, p.group.map_or(String::new(), |g| format!(" (group {g})")))
                    }));
                }
            } else {
                let queued = behind.take().map(|b| (wall_ms().saturating_sub(called), b));
                let _inflight = Inflight(ctx, id, &r);
                let room = admit(ctx, id, &r, makepad_remote::lines::asked_on())?;
                let mut x = get(ctx, id).ok_or("live job disappeared")?;
                let fault = r.fault.lock().unwrap().clone();
                if let Some(reason) = fault {
                    let mut p = x.patch.clone().ok_or("live: no test state")?;
                    p.tests = None;
                    p.reason = reason;
                    recover(ctx, id, &x, &mut p, true);
                    wait_tests(ctx, id);
                    // (that run had the bytes of the last ship, not of this call)
                    x = get(ctx, id).ok_or("live job disappeared")?;
                }
                let (files, edit_ms, current) = call_saves(ctx, &r, &x)?;
                if !files.is_empty() { ship_edit(ctx, id, &r, files, edit_ms, called, Some(current), queued); }
                let x = try_set(ctx, id, |s| s.touched_ms = wall_ms())?.ok_or("live job disappeared")?;
                return Ok(Some(with_room(crate::state::live_value(&x).to_json(), room.as_deref())));
            }
        }
        signal.wait(seen, Duration::from_secs(1));
    }
}

pub fn verdict(p: &LivePatch) -> String {
    let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
    if p.outcome == "jit_error" { return crate::state::jit_error_verdict(p); }
    let Some(t) = &p.tests else {
        return format!("verdict {}{}\n", if p.outcome.is_empty() { "pending" } else { &p.outcome }, if p.reason.is_empty() { String::new() } else { format!(": {}", p.reason) });
    };
    // a save made long before its call (a test session ships on the call) says so: that wait was
    // the caller's, not the ship's (162 s "save -> shipped" was an agent's 161 s before its sync, 11-02)
    let called = if p.called_ms > p.edit_ms + 1000 { format!("save -> call {}, ", after(p.called_ms)) } else { String::new() };
    // a call that waited for the previous call's tests says so: that wait was neither the ship's nor this edit's
    let called = if p.queued_ms > 0 { format!("{called}queued {} ms behind {}, ", p.queued_ms, p.behind) } else { called };
    let mut out = format!("verdict {}/{} passed, {} failed ({called}save -> shipped {}, -> patched {}, -> verdict {})\n", t.passed, t.ran, t.failed, after(p.shipped_ms), after(p.outcome_ms), after(p.verdict_ms));
    if !p.reason.is_empty() { out.push_str(&format!("  {}\n", p.reason)); }
    for name in &t.failures { out.push_str(&format!("  {name}\n")); }
    out.push_str(&p.output);
    out
}

/// What an agent whose edit waits for a restart runs: the app keeps the previous code, and later
/// saves queue with the edit, until it is relaunched (the JIT offers it to a person on its ring
/// and the WM's toast; a session on a box has neither).
/// From this many saves waiting for one restart its hint leads the verdict with their count (a lane
/// saved five times in 8 min into an app that ran none of them, 11-06).
const LOUD_SAVES: u64 = 3;

/// How long a session's restart may wait before the manager's check-in lists it ([`restarts_pending`]).
const RESTART_LISTED_MS: u64 = 5 * 60_000;

fn restart_hint(x: &Live) -> String {
    let relaunch = format!("`makepad devhub live act {} session relaunch`", x.id);
    if x.restart_saves >= LOUD_SAVES {
        return format!("{} saves since {} are not running: relaunch ({relaunch} restarts the app with all of them)", x.restart_saves, hhmm(x.restart_since_ms));
    }
    format!("restart pending: the app runs the previous code until {relaunch} restarts it with this edit and every save after it (devhub rebuilds a node's JIT first when its own sources changed)")
}

/// `HH:MMZ` of a wall time.
fn hhmm(ms: u64) -> String {
    let s = ms / 1000 % 86_400;
    format!("{:02}:{:02}Z", s / 3600, s / 60 % 60)
}

/// A save of `s`'s app that waits for a restart, at `now`: since when the first one has, how many,
/// and why the last.
fn restart_waits(s: &mut Live, p: &LivePatch, now: u64) {
    if s.restart_since_ms == 0 { s.restart_since_ms = now; }
    s.restart_saves += 1;
    s.restart_why = crate::proto::clean(p.reason.lines().next().unwrap_or(""), 200);
}

/// The app restarted on the sources as they are: no save waits for it any more.
fn restart_ran(s: &mut Live) {
    (s.restart_since_ms, s.restart_saves) = (0, 0);
    s.restart_why.clear();
}

/// For the manager's check-in (`lane quiet`): the live sessions whose restart has waited longer
/// than [`RESTART_LISTED_MS`], their lane measuring code the app does not run.
pub(super) fn restarts_pending(ctx: &Ctx, now: u64) -> Vec<String> {
    ctx.store.read(|d| restart_rows(&d.lives, now))
}

fn restart_rows(lives: &[Live], now: u64) -> Vec<String> {
    lives.iter().filter(|x| x.active() && x.restart_since_ms > 0 && now.saturating_sub(x.restart_since_ms) > RESTART_LISTED_MS).map(|x| format!(
        "{}: {} {}@{} restart pending {} min, {} save(s) since {} not running ({}): `makepad devhub live act {} session relaunch`",
        x.lane, x.id, x.app, x.node, now.saturating_sub(x.restart_since_ms) / 60_000, x.restart_saves, hhmm(x.restart_since_ms),
        if x.restart_why.is_empty() { "no reason given" } else { &x.restart_why }, x.id,
    )).collect()
}

/// One live job, answer first: `W3 running makepad-app-calculator on a node pid 4412; last edit: 2
/// file(s) patched, save → frame 38 ms`.
pub fn render(x: &Live) -> String {
    let mut loaded = x.clone();
    if let Err(e) = loaded.load_text() { return format!("live log: {e}\n"); }
    let x = &loaded;
    // whose session it is: a test session's lease is `live-<id>`, so its lane and agent show here
    let owner = match (x.lane.is_empty(), x.agent.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!(" of {}", x.lane),
        (true, false) => format!(" of {}", x.agent),
        (false, false) => format!(" of {} ({})", x.lane, x.agent),
    };
    let mut out = format!("{} {} {}{owner} on {} pid {}{}\n", x.id, x.state, x.app, if x.node.is_empty() { "-" } else { &x.node }, x.pid, if x.note.is_empty() { String::new() } else { format!(" ({})", x.note) });
    if !x.env.is_empty() { out.push_str(&format!("  env: {}\n", x.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(","))); }
    if let Some(p) = &x.patch {
        let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
        if x.test {
            out.push_str(&verdict(p));
        } else {
            out.push_str(&format!("  last edit: {} file(s) {}; save → shipped {}, → verdict {}, → frame {}{}\n", p.files.len(), if p.outcome.is_empty() { "shipping" } else { &p.outcome }, after(p.shipped_ms), after(p.outcome_ms), after(p.frame_ms), if p.reason.is_empty() { String::new() } else { format!(": {}", p.reason) }));
            if p.outcome == "restart" || x.restart_since_ms > 0 { out.push_str(&format!("  {}\n", restart_hint(x))); }
        }
    }
    if let Some(p) = x.panic.as_ref().filter(|_| x.state == "paused") {
        out.push_str(&format!("  panic {}: {} at {} [{}] -> live act {} stacks | live act {} resume {} retry|restart|panic\n", p.id, p.message, p.location, p.thread, x.id, x.id, p.id));
    }
    out
}

#[cfg(test)]
mod person_tests {
    use crate::state::Live;

    /// A lane holds [`super::LANE_SESSIONS`] headless sessions: past them the least recently used
    /// idle one goes, and with all of them busy the new one waits for a slot, first come first served,
    /// never refused. A visible session is the person's app: the cap never ends it, counts it or
    /// makes it wait (it closed a person's visible Fab for its lane's next session, and refused a
    /// lane's fourth session, 11-09).
    #[test]
    fn a_lanes_sessions_past_its_cap_wait_and_its_visible_one_is_the_persons() {
        use super::Slot;
        let t0 = 1_793_713_740_000u64;
        let min = 60_000;
        let live = |id: &str, visible: bool, touched_ms: u64| Live { id: id.into(), lane: "a-lane".into(), node: "box-a".into(), visible, touched_ms, state: "running".into(), ..Default::default() };
        let waiting = |id: &str, since: u64| Live { slot_wait_ms: since, state: "starting".into(), ..live(id, false, since) };
        let pick = |s: Slot| match s { Slot::Free => "free".to_string(), Slot::Takes(x) => x.id.clone(), Slot::Waits => "waits".to_string() };
        // the visible session is the least recently used one, and idle
        let mine = [live("W1", true, t0), live("W2", false, t0 + min), live("W3", false, t0 + 2 * min), live("W4", false, t0 + 3 * min)];
        assert_eq!(pick(super::slot(&mine, None, false, |_| true)), "W2", "cap: the least recently used idle headless session goes, never the visible one");
        assert_eq!(pick(super::slot(&mine, None, true, |_| true)), "free", "cap: a new visible session ends none");
        assert_eq!(pick(super::slot(&mine[..3], None, false, |_| true)), "free", "cap: a visible session does not count toward it");
        assert_eq!(pick(super::slot(&mine, None, false, |x| x.visible)), "waits", "cap: with its headless sessions busy the new one waits, the visible one stays");
        // W5 and W6 wait, in that order; W2 ended
        let (w5, w6) = (waiting("W5", t0 + 4 * min), waiting("W6", t0 + 5 * min));
        let lane = [mine[0].clone(), mine[2].clone(), mine[3].clone(), w5.clone(), w6.clone()];
        let others = |id: &str| lane.iter().filter(|x| x.id != id).cloned().collect::<Vec<_>>();
        assert_eq!(pick(super::slot(&others("W5"), Some(&w5), false, |_| false)), "free", "wait: a slot freed goes to the first waiter; waiting sessions hold none");
        assert_eq!(pick(super::slot(&others("W6"), Some(&w6), false, |_| true)), "waits", "wait: a later waiter waits behind an earlier one");
        assert_eq!(pick(super::slot(&lane, None, false, |_| true)), "waits", "wait: a new session queues behind the waiters");
        assert_eq!(pick(super::slot(&lane, None, true, |_| false)), "free", "wait: a visible session never waits");
        let now = t0 + 30 * min;
        assert_eq!(super::unused_ms(&mine[0], Some(now - 2 * min), now), 2 * min, "idle: a person's input at its box uses a visible session");
        assert_eq!(super::unused_ms(&mine[1], Some(now - 2 * min), now), 29 * min, "idle: a headless session on that box goes by its edits and calls");
        assert_eq!(super::unused_ms(&mine[0], None, now), 30 * min, "idle: no input seen at its box");
        assert!(super::unused_ms(&mine[0], Some(now - 12 * min), now) >= super::IDLE_MS, "idle: it ends 10 min after the person's last input");
    }

    /// `devhub inspect <box>/<pid|app>` of a session's app is a call of the session's: its box and its
    /// pid, package or registry name name it (FABSTRENGTHEN's `ui find` and tool calls went that way
    /// and its sessions idled out, 11-09).
    #[test]
    fn a_routed_command_names_its_sessions_app() {
        let x = Live { id: "W1".into(), node: "box-a".into(), pid: 4242, app: "makepad-app-calculator".into(), state: "running".into(), ..Default::default() };
        for spec in ["4242", "makepad-app-calculator", "calculator"] {
            assert!(super::names_app(&x, "box-a", spec), "{spec} names it");
        }
        assert!(!super::names_app(&x, "box-b", "4242"), "another box's pid");
        assert!(!super::names_app(&x, "box-a", "4243"), "another pid");
        assert!(!super::names_app(&Live { pid: 0, ..x.clone() }, "box-a", "0"), "a session whose app is not up yet has no pid");
        assert!(super::a_lanes_call("cmd", "observe") && super::a_lanes_call("app", "devhub"), "a lane's inspect and tool call");
        assert!(!super::a_lanes_call("watch", "observe") && !super::a_lanes_call("stream", "observe"), "a watch only follows");
        assert!(!super::a_lanes_call("cmd", "devhub-live"), "devhub's own look at the app");
    }
}

#[cfg(test)]
mod fault_tests {
    /// In an app's output a panic in the JIT's sources and the JIT's own lines are the JIT's
    /// faults. A test program of the JIT's own crates is the JIT and prints both on purpose: its
    /// live runs were stopped at the first one and failed as JIT errors (11-12).
    #[test]
    fn the_jits_own_crates_tests_print_the_jits_lines_as_their_own() {
        let panic = "thread '<unnamed>' (2729363) panicked at libs/relax/compiler/src/jit.rs:2200:49:";
        let restart = "makepad-run: JIT thread `lifeline-test-restart` panicked: lifeline test panic (at libs/relax/compiler/src/lifeline.rs:271) -- restarted (1 of 16)";
        for line in [panic, restart] {
            assert!(super::fault_line("makepad-fab", line, &mut false), "an app's run: the JIT's fault: {line}");
            assert!(!super::fault_line("relax-compiler", line, &mut false), "relax-compiler's tests: its own line: {line}");
        }
        assert!(!super::fault_line("relax-front", "thread 't' panicked at libs/relax/front/src/parse.rs:1:1:", &mut false), "relax-front's tests: its own panic");
        assert!(super::jit_fault(panic), "a refused patch's reasons are the JIT's");
    }
}
