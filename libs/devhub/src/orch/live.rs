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
//! its app exits, after 10 min with no edit, call or person's input in its app (a `devhub inspect`
//! of its app is a call; never a visible one: it stays until its owner ends it), when its lane
//! landed (not a visible one, nor one a person used in the last 30 min), or to give a waiting
//! whole/timed lease its box once the current call answers. A lane's sessions are not counted:
//! each asks for its box and waits in the queue for room. A call (a test session's tests) past
//! three times its class's measured run is said, never ended: an inbox warning, a why-hung sample
//! in `devhub job result <lease>`, and the test's still-running line. Tests keep one job per lane,
//! target and args;
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
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) const SET_USAGE: &str = "--set needs <name>/<n>: a lane-local name and a member count of 1 or more";

/// A hidden live job with no edit or call this long ends (tests too: an idle session's box waits
/// for it); a visible one stays until its owner ends it ([`idle_kept`]).
pub const IDLE_MS: u64 = 10 * 60 * 1000;
/// A test session whose starter left (its last call's client went before the answer: the lane's
/// `devhub test --live` stopped) ends after this long with no call or save, one still queued for
/// its box too: stopped clients left a lane 13 sessions, 3 holding leases (11-16). The lane's next
/// call starts it again.
pub const LEFT_MS: u64 = 3 * 60 * 1000;
/// A session its app's builds alone kept this long past its idle end is said, once
/// ([`keep_for_build`]): a WM that rebuilds a failing app in a loop holds its box, and nothing
/// would say so. Only said; the session stays while the builds run.
const KEPT_SAID_MS: u64 = 30 * 60 * 1000;

/// What keeps `x` past its idle end, its edits and calls [`IDLE_MS`] behind: compilers at work
/// in its tree within [`IDLE_MS`] (`built`: when its box last said so, and the crate the busiest
/// built, [`crate::lease::master::Master::built`]). A WM session builds the apps it hosts with
/// cargo, minutes each, and no call says so: its idle end would cut it mid-build. Only
/// compilers count: an app that animates or spins a core is no build, and ends as before.
fn kept_by_build(x: &Live, built: Option<(u64, String)>, now: u64) -> Option<String> {
    built.filter(|(at, _)| now.saturating_sub(*at) < IDLE_MS && now.saturating_sub(x.touched_ms) >= IDLE_MS).map(|(_, what)| what)
}

/// `x` kept by its app's builds ([`kept_by_build`]): `live list` says so, and once it is
/// [`KEPT_SAID_MS`] past its idle end the master's log says it, with what builds.
fn keep_for_build(ctx: &Ctx, x: &Live, r: &Running, what: &str, now: u64) {
    let x = set(ctx, &x.id, |s| (s.kept_by, s.kept_ms) = (what.to_string(), now)).unwrap_or_else(|| x.clone());
    let past = now.saturating_sub(x.touched_ms.saturating_add(IDLE_MS));
    if past >= KEPT_SAID_MS && !r.kept_said.swap(true, std::sync::atomic::Ordering::SeqCst) {
        event(ctx, Sev::Warn, format!("live {} ({}) of lane {}: builds alone kept it {} past its idle end (no edit or call for {}); its tree builds `{what}` now; `devhub live release {}` ends it", x.id, x.app, x.lane, crate::state::ago(past), crate::state::ago(now.saturating_sub(x.touched_ms)), x.id), Some(&x));
    }
}

/// `live list`'s line for a session its app's builds keep past its idle end ([`keep_for_build`]),
/// while no edit or call came since.
fn kept_line(x: &Live, now: u64) -> Option<String> {
    (x.active() && !x.kept_by.is_empty() && x.kept_ms > x.touched_ms).then(|| format!("  kept by a build ({}): no edit or call for {}, past its idle end of {} min\n", x.kept_by, crate::state::ago(now.saturating_sub(x.touched_ms)), IDLE_MS / 60_000))
}

/// The client of `id`'s call went before its answer: its starter left ([`LEFT_MS`]).
fn starter_left(ctx: &Ctx, id: &str) {
    let mut first = false;
    let x = set(ctx, id, |s| if s.left_ms == 0 { s.left_ms = wall_ms(); first = true; });
    if first { event(ctx, Sev::Info, format!("live {id}: its call's client went before the answer; it ends {} min after with no call or save", LEFT_MS / 60_000), x.as_ref()); }
}

/// Whether `x`'s starter left [`LEFT_MS`] ago or more, and nothing touched it since (a call, or a
/// verdict for nobody: the time counts from it). A save on its box keeps it ([`start`]'s sweep).
fn left_long(x: &Live, now: u64) -> bool {
    x.held.is_empty() && x.active() && x.left_ms > 0 && now.saturating_sub(x.left_ms.max(x.touched_ms)) >= LEFT_MS
}
/// How long a session has gone unused at `now`: since its last edit or call, and a visible one also
/// since a person's last input at its box (`input_ms`, [`crate::lease::sched::Sched::user_input_ms`]):
/// a visible session is the person's app, in use while they work at its box (the idle end closed
/// a person's visible Fab, 11-09); past its idle end it stays all the same ([`idle_kept`]).
/// `app_input_ms`: when a person last gave its app input ([`person_input_at`]), visible or not
/// (through Desk Link, which the box's idle time does not see).
fn unused_ms(x: &Live, input_ms: Option<u64>, app_input_ms: Option<u64>, now: u64) -> u64 {
    let last = if x.visible { x.touched_ms.max(input_ms.unwrap_or(0)) } else { x.touched_ms };
    now.saturating_sub(last.max(app_input_ms.unwrap_or(0)))
}

/// How long a person's input in a session's app keeps it from its landed lane's end
/// ([`kept_from_landed`]).
const PERSON_KEEPS_MS: u64 = 30 * 60 * 1000;
/// How long an app's `activity` answer is waited for ([`person_input_at`]), and how long it is
/// reused: the idle sweep looks every second.
const ACTIVITY_LOOK: Duration = Duration::from_secs(2);
const ACTIVITY_EVERY_MS: u64 = 30_000;
/// Per session: when its app was asked for its activity, and its person's last input then.
static PERSON_SEEN: Mutex<Option<HashMap<String, (u64, Option<u64>)>>> = Mutex::new(None);

/// When a person last gave the session's app input, by its `activity` answer (`idle_ms`: input a
/// lane drives through devhub is no person's), or None: none yet, no app, or no answer within
/// [`ACTIVITY_LOOK`] (an app busy building or drawing is asked again at the next look). Asked at
/// most once every [`ACTIVITY_EVERY_MS`] per session.
fn person_input_at(x: &Live, now: u64) -> Option<u64> {
    if x.pid == 0 { return None; }
    if let Some(&(asked, at)) = PERSON_SEEN.lock().unwrap().as_ref().and_then(|m| m.get(&x.id)) {
        if now.saturating_sub(asked) < ACTIVITY_EVERY_MS { return at; }
    }
    let t = Instant::now();
    let at = inspect_until(&x.node, x.pid, "activity", &|| t.elapsed() > ACTIVITY_LOOK).ok().flatten()
        .and_then(|lines| lines.iter().find_map(|l| num(l, "idle_ms")))
        .map(|idle| now.saturating_sub(idle));
    PERSON_SEEN.lock().unwrap().get_or_insert_with(HashMap::new).insert(x.id.clone(), (now, at));
    at
}

/// Why a session whose lane landed and went quiet stays: it is visible (a person's window: a
/// person looked at a two-machine demo through Desk Link as the landed lane's end closed it,
/// W22615, 11-26), or a person gave its app input within [`PERSON_KEEPS_MS`]. Its owner ends it
/// (`devhub live release`); a quiet hidden one goes.
fn kept_from_landed(x: &Live, app_input_ms: Option<u64>, now: u64) -> Option<String> {
    if x.visible {
        return Some(format!("it is visible, a person's window: it stays until its owner ends it (`devhub live release {}`)", x.id));
    }
    let ago = now.saturating_sub(app_input_ms?);
    (ago < PERSON_KEEPS_MS).then(|| format!("a person gave its app input {} ago: it stays while they use it (to {} min after their last input), or until its owner ends it (`devhub live release {}`)", crate::lease::sched::dur(ago), PERSON_KEEPS_MS / 60_000, x.id))
}

/// Why a visible session past its idle end ([`idle_now`]) stays: a person's window ends only by its
/// owner's release, used or not (a person's demo on a box was ended twice so in a morning, 11-26).
/// Only a hidden one ends by its idle time.
fn idle_kept(x: &Live) -> Option<String> {
    x.visible.then(|| format!("it is visible, a person's window: it stays until its owner ends it (`devhub live release {}`)", x.id))
}

/// A session's keep ([`kept_from_landed`], [`idle_kept`]) is said once per session and kind.
static KEPT_SAID: Mutex<Option<std::collections::HashSet<(String, &'static str)>>> = Mutex::new(None);

fn said_once(id: &str, kind: &'static str) -> bool {
    KEPT_SAID.lock().unwrap().get_or_insert_with(Default::default).insert((id.to_string(), kind))
}

/// The sessions past their idle end at `now`: active, not held, placed, joined by none, and unused
/// for [`IDLE_MS`] ([`unused_ms`]); a run or a build may keep them still ([`sweep_end`],
/// [`kept_by_build`]). Only on a box this master has asked for its caps: a person's input there
/// and its builds come with them, and a restarted master has none before its first ask. A restart
/// is no call: its sessions keep their own touch through it ([`take_back`]). Its touch had kept
/// W19220, with no save or call, on its box through four restarts in most of an hour (11-25).
fn idle_now(ctx: &Ctx, now: u64) -> Vec<Live> {
    let past: Vec<Live> = ctx.store.read(|d| {
        let mut touched = HashMap::new();
        for x in d.lives.iter().filter(|x| x.active() && !x.set.is_empty()) {
            let last = touched.entry((&x.lane, &x.set)).or_insert(x.touched_ms);
            *last = (*last).max(x.touched_ms);
        }
        d.lives.iter().filter(|x| x.active() && x.held.is_empty() && !x.lease.is_empty()
            && now.saturating_sub(*touched.get(&(&x.lane, &x.set)).unwrap_or(&x.touched_ms)) >= IDLE_MS
            && !joined(&d.lives, &x.id)).cloned().collect()
    });
    past.into_iter().filter(|x| ctx.leases.caps_asked(&x.node) && unused_ms(x, ctx.leases.user_input_ms(&x.node), None, now) >= IDLE_MS)
        // (its app's own person's input, asked last: only of a session its other use left idle)
        .filter(|x| unused_ms(x, ctx.leases.user_input_ms(&x.node), person_input_at(x, now), now) >= IDLE_MS)
        .collect()
}

/// A test session's saves wait for its next call and keep it meanwhile, as a call does: the last
/// one is noted as its touch at the ship loop's next look, so that it keeps it through a master's
/// restart too, which forgets the saves its runs had queued. (To within the look's second: read
/// back from its instant, the save's time moves with the clocks' drift at every look.)
fn note_save(ctx: &Ctx, id: &str, r: &Running) {
    let Some(t) = r.pending.lock().unwrap().2 else { return };
    let at = wall_ms().saturating_sub(t.elapsed().as_millis() as u64);
    let look = HOUSEKEEPING.as_millis() as u64;
    if ctx.store.read(|d| d.lives.iter().any(|x| x.id == id && x.active() && x.touched_ms.saturating_add(look) <= at)) {
        let _ = try_set(ctx, id, |s| s.touched_ms = s.touched_ms.max(at));
    }
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
        acted(ctx, &id, format!("`devhub inspect {node}/{spec}` ({service}, from {caller})"));
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
    recover_cut: AtomicBool,
    resume_active: AtomicBool,
    supersede: AtomicBool,
    active_wait: Mutex<Option<Arc<AtomicBool>>>,
    fault: Mutex<Option<String>>,
    /// the run of the app seen up ([`wait_up`]): an edit's ship asks nothing more
    up_pid: std::sync::atomic::AtomicU32,
    /// the callers waiting for its run's answer: a test call, a start that waits for its first
    /// verdict while its client stays ([`Waiting`]); a run in flight that none waits for holds
    /// its session only while it writes ([`run_holds`])
    waiting: std::sync::atomic::AtomicUsize,
    /// a `session relaunch` call waits for the edit in flight ([`relaunch`]): a refused edit's wait
    /// for its recovery ends for it (the relaunch restarts on the current sources whatever that
    /// wait would see)
    relaunch_waits: std::sync::atomic::AtomicBool,
    /// its builds kept it [`KEPT_SAID_MS`] past its idle end, and that was said
    kept_said: std::sync::atomic::AtomicBool,
}

/// A caller's wait for its session's run ([`Running::waiting`]), for as long as it is held.
struct Waiting {
    r: Arc<Running>,
    released: Arc<AtomicBool>,
}
impl Waiting {
    fn on(r: Arc<Running>) -> Waiting {
        r.waiting.fetch_add(1, SeqCst);
        Waiting { r, released: Default::default() }
    }

    fn bind_active(&self) {
        *self.r.active_wait.lock().unwrap() = Some(self.released.clone());
    }

    fn release_once(&self) {
        if !self.released.swap(true, SeqCst) { self.r.waiting.fetch_sub(1, SeqCst); }
    }
}
impl Drop for Waiting {
    fn drop(&mut self) { self.release_once(); }
}

impl Running {
    fn release_active_wait(&self) {
        if let Some(released) = self.active_wait.lock().unwrap().take() {
            if !released.swap(true, SeqCst) { self.waiting.fetch_sub(1, SeqCst); }
        }
    }
}

fn try_supersede_orphan(r: &Running, fresh: bool, changed: bool) -> bool {
    let _calls = r.calls.lock().unwrap();
    if r.inflight.load(SeqCst)
        && r.waiting.load(SeqCst) == 0
        && !r.restart.load(SeqCst)
        && (fresh || changed)
    {
        r.supersede.store(true, SeqCst);
        return true;
    }
    false
}

fn try_supersede_running(ctx: &Ctx, id: &str, r: &Running, fresh: bool, changed: bool) -> bool {
    ctx.store.read(|d| {
        d.lives.iter().any(|s| s.id == id && s.state == "running" && s.pid > 0)
            && try_supersede_orphan(r, fresh, changed)
    })
}

fn watch_superseded(r: &Running) -> bool { r.supersede.load(SeqCst) }

/// True once a supersede request may release `inflight`: no restart is pending and no
/// `devhub-live-pid` watcher is actively running `resume_tests`.
fn superseded_idle(r: &Running) -> bool {
    let _calls = r.calls.lock().unwrap();
    watch_superseded(r) && !r.resume_active.load(SeqCst) && !r.restart.load(SeqCst)
}

/// True when a superseded caller (`initial_call`) may drop `inflight` without racing a restart
/// or a booting replacement process (`s.pid == 0`).
fn can_yield_superseded(ctx: &Ctx, id: &str, r: &Running) -> bool {
    ctx.store.read(|d| {
        d.lives.iter().any(|s| s.id == id && s.state == "running" && s.pid > 0)
            && superseded_idle(r)
    })
}

/// Consumes a pending warm restart atomically with any stale `supersede` flag under `r.calls`
/// so no waiter can observe `(!restart && supersede)` between the two stores, and a replacement
/// process's `resume_tests` never aborts on a flag left by a disconnected earlier caller.
fn take_restart(r: &Running) -> bool {
    let _calls = r.calls.lock().unwrap();
    if !r.restart.load(SeqCst) { return false; }
    r.supersede.store(false, SeqCst);
    r.restart.store(false, SeqCst);
    true
}

struct ResumeGuard<'a>(&'a Ctx, &'a str, &'a Running);

impl Drop for ResumeGuard<'_> {
    fn drop(&mut self) {
        self.2.resume_active.store(false, SeqCst);
        self.0.store.notify("live", self.1);
    }
}

/// Whether the run in flight keeps a session from its idle, left or landed end: while a caller
/// waits for its answer (a hung run is said, never ended), or while it still writes (its log
/// changed within [`IDLE_MS`]). A run that none waits for and that went silent is orphaned:
/// W11710's first run stopped at `running 4 tests`, no call waited for it (a `live start
/// --test`), and it held its box 4.7 h after its lane landed (11-17).
fn run_holds(inflight: bool, waiting: usize, quiet_ms: Option<u64>) -> bool {
    inflight && (waiting > 0 || quiet_ms.is_none_or(|q| q < IDLE_MS))
}

/// A session's end by a sweep: when no call is open on it and its run in flight does not hold it
/// ([`run_holds`]), its park note is set (no call or edit starts after it) and it ends now, its
/// orphaned run with it. Whether it ended.
fn sweep_end(ctx: &Ctx, x: &Live, r: &Running, now: u64, note: impl FnOnce() -> String) -> bool {
    let quiet = quiet_ms(x, now);
    let parked = ctx.store.read(|d| {
        let Some(current) = d.lives.iter().find(|s| s.id == x.id && s.active()) else { return false };
        let mut calls = r.calls.lock().unwrap();
        if calls.keeps_refusal(current) { return false; }
        let inflight = r.inflight.load(std::sync::atomic::Ordering::SeqCst);
        if calls.count > 0 || run_holds(inflight, r.waiting.load(std::sync::atomic::Ordering::SeqCst), quiet) { return false; }
        let mut note = note();
        if inflight {
            note += &format!("; its run wrote nothing for {} min and no call waited for its answer", quiet.unwrap_or(0) / 60_000);
        }
        calls.park.get_or_insert(note);
        true
    });
    if !parked { return false; }
    end_parked(ctx, &x.id, r, false);
    true
}

static RUNNING: Mutex<Option<HashMap<String, Arc<Running>>>> = Mutex::new(None);
/// The ask a starting session's start waits on, by session, from the first answer that names it
/// until its run is joined ([`RUNNING`] names its lease from then on): a release gives the ask up
/// at once ([`release`]), queued, or granted while its sources ship. Left to the start's next look at its session, a released
/// session's queued timed ask stood for up to a beat, and its lane's next start was refused as a
/// second timed lease of the lane (11-23).
static ASKED: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
/// [`ASKED`]'s entry of a start, until its run is joined.
struct Asking<'a>(&'a str);
impl Asking<'_> {
    fn names(&self, lease: &str) { ASKED.lock().unwrap().insert(self.0.to_string(), lease.to_string()); }
}
impl Drop for Asking<'_> {
    fn drop(&mut self) { ASKED.lock().unwrap().remove(self.0); }
}
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

/// The control listener can accept live calls while the master reconciles its saved sessions.
/// The atomic keeps ordinary calls off the gate's mutex once startup is complete.
static START_GATE_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static START_GATE: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(false), std::sync::Condvar::new());

pub(crate) struct StartupGate;

impl StartupGate {
    pub(crate) fn arm() -> Self {
        let mut armed = START_GATE.0.lock().unwrap_or_else(|e| e.into_inner());
        *armed = true;
        START_GATE_ARMED.store(true, std::sync::atomic::Ordering::Release);
        Self
    }

    pub(crate) fn release() {
        let mut armed = START_GATE.0.lock().unwrap_or_else(|e| e.into_inner());
        if *armed {
            *armed = false;
            START_GATE_ARMED.store(false, std::sync::atomic::Ordering::Release);
            START_GATE.1.notify_all();
        }
    }

    fn wait_ready() {
        if !START_GATE_ARMED.load(std::sync::atomic::Ordering::Acquire) { return; }
        let mut armed = START_GATE.0.lock().unwrap_or_else(|e| e.into_inner());
        while *armed {
            armed = START_GATE.1.wait(armed).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for StartupGate {
    fn drop(&mut self) { Self::release(); }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchPath {
    Serve,
    Restarted,
    Reattach,
    ResumeParked,
}

impl LaunchPath {
    fn as_str(self) -> &'static str {
        match self {
            Self::Serve => "serve",
            Self::Restarted => "restarted",
            Self::Reattach => "reattach",
            Self::ResumeParked => "resume_parked",
        }
    }
}

/// A leaf lock. Each worker owns its session from the launch decision through cleanup.
static CLAIMED: Mutex<BTreeMap<String, LaunchPath>> = Mutex::new(BTreeMap::new());

struct Claim {
    id: String,
    by: LaunchPath,
}

impl Claim {
    fn acquire(id: &str, by: LaunchPath) -> Result<Self, LaunchPath> {
        let mut map = CLAIMED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&owner) = map.get(id) { return Err(owner); }
        map.insert(id.to_string(), by);
        Ok(Self { id: id.to_string(), by })
    }

    fn id(&self) -> &str { &self.id }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut map = CLAIMED.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(&self.id) == Some(&self.by) { map.remove(&self.id); }
    }
}

fn claimed_by(id: &str) -> Option<LaunchPath> {
    CLAIMED.lock().unwrap_or_else(|e| e.into_inner()).get(id).copied()
}

fn warn_duplicate(ctx: &Ctx, id: &str, by: LaunchPath, owner: LaunchPath) {
    event(ctx, Sev::Warn, format!("live {id}: refused duplicate start from {} (already owned by {})", by.as_str(), owner.as_str()), get(ctx, id).as_ref());
}

fn try_claim_or_warn(ctx: &Ctx, id: &str, by: LaunchPath) -> Option<Claim> {
    match Claim::acquire(id, by) {
        Ok(claim) => Some(claim),
        Err(owner) => {
            warn_duplicate(ctx, id, by, owner);
            None
        }
    }
}

#[derive(Default)]
struct Calls {
    count: usize,
    checks: Vec<CargoCheck>,
    park: Option<String>,
    ended: bool,
}

/// The test client keeps its connection open through cargo. A check protects this refusal
/// from time sweeps without admitting a live call or preventing the scheduler from parking it.
struct CargoCheck {
    caller: String,
    refusal: String,
    diagnostic: String,
    gone: Box<dyn Fn() -> bool + Send + Sync>,
}

fn test_reply(x: &Live, r: Option<&Running>, caller: &str) -> String {
    let value = crate::state::live_value(x);
    if !caller.is_empty() && !x.offline && x.held.is_empty() {
        if let (Some(refusal), Some(r), Some(conn)) = (x.refusal_key(), r, makepad_remote::lines::asked_on()) {
            r.calls.lock().unwrap().checking(&crate::state::live_from(&value), caller, refusal, Box::new(move || conn.is_closed()));
        }
    }
    value.to_json()
}

/// Consume only this caller's check. A newer edit or run invalidates both possible verdicts.
/// End the refusal in the same store transaction that checks its identity, before freeing the
/// lease. Concurrent checks on the same refusal may still confirm it.
fn checked(ctx: &Ctx, id: &str, calls: &Mutex<Calls>, caller: &str, refusal: &str, confirmed: bool) -> Result<String, String> {
    let mut released = None;
    let mut consumed = None;
    let mut diagnostic = String::new();
    // An armed check carries its diagnostic. Without one (below), read it from the record with
    // its spilled text loaded, as the call's reply had it, outside the store's lock.
    let unarmed = (confirmed && !calls.lock().unwrap().checks.iter().any(|c| c.caller == caller && c.refusal == refusal))
        .then(|| get(ctx, id).and_then(|x| crate::state::live_from(&crate::state::live_value(&x)).compile_errors().first().map(|e| crate::proto::clean(e, 240))))
        .flatten();
    let progress = end_progress(ctx, id);
    let result = try_set(ctx, id, |x| {
        let mut calls = calls.lock().unwrap();
        let Some(i) = calls.checks.iter().position(|c| c.caller == caller && c.refusal == refusal) else {
            // No check of this caller: the master restarted since the call armed it (its checks
            // live in memory). The caller is still here and cargo confirmed this very refusal,
            // so it holds as an armed check would.
            if confirmed && x.refusal_key().as_deref() == Some(refusal) && calls.park.is_none() {
                if let (true, Some(first)) = (x.held.is_empty(), unarmed.as_ref()) { x.held.clone_from(first); }
                diagnostic.clone_from(&x.held);
            }
            return;
        };
        let check = consumed.insert(calls.checks.remove(i));
        if x.refusal_key().as_deref() != Some(refusal) || calls.park.is_some() { return; }
        if confirmed {
            x.held.clone_from(&check.diagnostic);
            diagnostic = x.held.clone();
        } else if x.held.is_empty() && !calls.checks.iter().any(|c| c.refusal == refusal) {
            x.state = "ended".into();
            x.ended_ms = wall_ms();
            x.note = "released: cargo did not confirm the live refusal".into();
            settle_open_patch(x, x.ended_ms, progress.as_ref());
            released = Some(x.lease.clone());
        }
    });
    if let Err(e) = result {
        if let Some(check) = consumed { calls.lock().unwrap().checks.push(check); }
        return Err(e);
    }
    if let Some(lease) = released { free_lease(ctx, id, lease); }
    Ok(format!("{{\"held\":{}}}", json_str(&diagnostic)))
}

fn departed_checks(ctx: &Ctx, id: &str, calls: &Mutex<Calls>) {
    let gone: Vec<_> = calls.lock().unwrap().checks.iter().filter(|c| (c.gone)()).map(|c| (c.caller.clone(), c.refusal.clone())).collect();
    for (caller, refusal) in gone {
        if let Err(e) = checked(ctx, id, calls, &caller, &refusal, false) { eprintln!("live {id}: {e}"); }
    }
}

impl Calls {
    fn checking(&mut self, x: &Live, caller: &str, refusal: String, gone: Box<dyn Fn() -> bool + Send + Sync>) {
        if let Some(first) = x.compile_errors().first() {
            self.checks.retain(|c| c.refusal == refusal && c.caller != caller);
            self.checks.push(CargoCheck { caller: caller.into(), refusal, diagnostic: crate::proto::clean(first, 240), gone });
        }
    }

    fn keeps_refusal(&self, x: &Live) -> bool {
        !x.held.is_empty() || (!self.checks.is_empty() && x.refusal_key().is_some_and(|key| self.checks.iter().any(|c| c.refusal == key)))
    }

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
    call_until(ctx, id, None)
}

/// [`call`], its wait for its session's box ([`rejoined`]) ending at `until`: a wait's own end
/// ([`wait`]), which a restarted master that never took the box back held past (WORKBOARD, 11-27).
fn call_until<'a>(ctx: &'a Ctx, id: &'a str, until: Option<Instant>) -> Result<Option<Call<'a>>, String> {
    let client = makepad_remote::lines::asked_on();
    let waits = || waits_on_box(get(ctx, id).as_ref(), client.as_ref().is_some_and(|c| c.is_closed()), until, Instant::now());
    let Some(r) = rejoined(&mut || running(id), &waits, &mut || {
        let line = format!("live call {id} waits for its session's box: the master restarted and takes it back now");
        makepad_remote::lines::say_waiting(&line);
        event(ctx, Sev::Info, line, None);
    }) else { return Ok(None) };
    {
        let mut calls = r.calls.lock().unwrap();
        if let Some(note) = &calls.park { return Err(format!("{id}: {note}")); }
        calls.count += 1;
    }
    let mut call = Call(ctx, id, r, None);
    call.3 = admit(ctx, id, &call.2, makepad_remote::lines::asked_on())?;
    Ok(Some(call))
}

/// Whether a call still waits for its session's box ([`rejoined`]): its session runs an app (a
/// pid), its client stays, and its own end (`until`, a wait's) has not come.
fn waits_on_box(x: Option<&Live>, client_gone: bool, until: Option<Instant>, now: Instant) -> bool {
    x.is_some_and(|x| x.active() && x.pid != 0) && !client_gone && until.is_none_or(|u| now < u)
}

/// Wakes a call that waits for its session's box ([`rejoined`]): a reattach joined it.
static JOINED: (Mutex<()>, std::sync::Condvar) = (Mutex::new(()), std::sync::Condvar::new());

/// A call's session box (`find`), waited for while `waits`: a session whose app runs (it has a
/// pid) and that this master holds no box for is a previous master's, which its reattach takes
/// back on a thread of its own once the master serves. A call there ran unheld, and the idle sweep
/// ended its session under it: W16863's call, asked again into the master's restart as a cut
/// call is, waited on its app while its session ended "after 10 min with no edit or call" (11-23).
/// `say` once as it starts waiting. None at once, or once the session ends, has no app (placed
/// again: its call says so) or its client goes.
fn rejoined<T>(find: &mut dyn FnMut() -> Option<T>, waits: &dyn Fn() -> bool, say: &mut dyn FnMut()) -> Option<T> {
    let mut said = false;
    loop {
        if let Some(r) = find() { return Some(r); }
        if !waits() { return None; }
        if !said {
            say();
            said = true;
        }
        // (a join between the look and this wait is seen at the next look)
        let g = JOINED.0.lock().unwrap();
        let _ = JOINED.1.wait_timeout(g, ADMIT_LOOK);
    }
}

fn begin_call(r: &Running) -> bool {
    let calls = r.calls.lock().unwrap();
    calls.park.is_none() && !r.inflight.swap(true, std::sync::atomic::Ordering::SeqCst)
}

fn finish_call(ctx: &Ctx, id: &str, r: &Running) {
    {
        let _calls = r.calls.lock().unwrap();
        *r.active_wait.lock().unwrap() = None;
        r.inflight.store(false, SeqCst);
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

/// A live test's selection before its libtest `--` arguments: its target (none, `--lib`, `--bin B`
/// or `--test T`), then its package's feature flags (`--no-default-features`, `--features F`),
/// which the session builds with and is keyed by.
pub(crate) fn test_select_ok(select: &[String]) -> bool {
    let mut i = match select {
        [f, ..] if f == "--lib" => 1,
        [f, t, ..] if (f == "--bin" || f == "--test") && !t.is_empty() && !t.starts_with('-') => 2,
        _ => 0,
    };
    while i < select.len() {
        match select[i].as_str() {
            "--no-default-features" => i += 1,
            "--features" if select.get(i + 1).is_some_and(|v| !v.is_empty() && !v.starts_with('-')) => i += 2,
            _ => return false,
        }
    }
    true
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

/// The lane's other test sessions of `new`'s package on its system, which `new` replaces: one live
/// test session per lane, package and system. A whole-package `test --live` left a session per
/// target up after its verdict, each holding a lease (35 of one package on one box, idle, 11-15);
/// another target, filter or feature set of the package is a new session, and the old ones end.
/// Another system's session stays: a lane's Windows `test --live` ended its Linux one while that
/// one's JIT still built, and the lane lost two test rounds (11-26).
fn replaced<'a>(lives: &'a [Live], new: &Live) -> Vec<&'a str> {
    lives.iter().filter(|x| new.test && x.test && new.set.is_empty() && x.set.is_empty() && x.id != new.id && x.active() && x.lane == new.lane && x.app == new.app && system(x) == system(new)).map(|x| x.id.as_str()).collect()
}

/// The system a test session asks for, which keys it with its lane and package ([`replaced`]):
/// the OS its `--needs` names, and the box its `--node` names (that box's OS is not known here: a
/// session pinned to a box is keyed by the box). A session that names neither runs where the
/// fleet puts unnamed work, the Windows box never among them.
fn system(x: &Live) -> (String, &str) {
    let os = crate::lease::sched::parse_needs(&x.needs).into_iter().find(|(k, _)| k == "os").map(|(_, v)| v.to_ascii_lowercase()).unwrap_or_default();
    (os, x.pin.as_str())
}

/// The lane's newest test session of `x`'s package and system asked after `x` and still active: `x` ends
/// once its call does, as one [`replaced`] does. [`retire_replaced`] retires only what runs, and a
/// session still waiting for its box is left to its call: several `test --live` calls of one
/// package at once each started a session while the others waited for theirs, none was
/// replaced, and six sessions of one lane held leases on three boxes for 25 min (W22126-W22157,
/// 11-26).
fn superseded_by<'a>(lives: &'a [Live], x: &Live) -> Option<&'a str> {
    let seq = |id: &str| id.trim_start_matches('W').parse::<u64>().unwrap_or(0);
    lives.iter().filter(|o| x.test && o.test && x.set.is_empty() && o.set.is_empty() && o.active() && o.lane == x.lane && o.app == x.app && system(o) == system(x) && seq(&o.id) > seq(&x.id)).max_by_key(|o| seq(&o.id)).map(|o| o.id.as_str())
}

/// A session that has its box ends after its first call when a newer one of its lane, package
/// and system was asked meanwhile ([`superseded_by`]).
fn retire_if_superseded(ctx: &Ctx, id: &str, r: &Running) {
    let Some(newer) = ctx.store.read(|d| d.lives.iter().find(|x| x.id == id).and_then(|x| superseded_by(&d.lives, x)).map(str::to_string)) else { return };
    let note = format!("replaced by {newer}, asked while this one waited for its box: one live test session per lane, package and system");
    r.calls.lock().unwrap().park.get_or_insert_with(|| note);
    park_ready(ctx, id, r);
}

/// Ends the sessions `new` replaces ([`replaced`]), as a park does: an idle one now, one in a call
/// when its call and edit end (its caller's verdict comes first). One still waiting for its box is
/// in its first call, its caller waiting on it: it is left to that call.
fn retire_replaced(ctx: &Ctx, new: &Live) {
    let ids: Vec<String> = ctx.store.read(|d| replaced(&d.lives, new).into_iter().map(str::to_string).collect());
    let note = format!("replaced by {} ({}{}): one live test session per lane, package and system", new.id, new.app, new.test_args.iter().map(|a| format!(" {a}")).collect::<String>());
    for id in ids {
        let Some(r) = running(&id) else { continue };
        r.calls.lock().unwrap().park.get_or_insert_with(|| note.clone());
        park_ready(ctx, &id, &r);
    }
}

/// Called by the lease master only when all blockers of the best box are live jobs. With
/// `resume_after` (the timed lease they park for while in use) each starts again by itself once
/// that lease ends ([`resume_parked`]), and its line says the work it interrupted: its last act
/// ([`park_note`]).
pub fn park(ctx: &Ctx, leases: &[String], note: &str, resume_after: Option<&str>) {
    let list: Vec<_> = RUNNING.lock().unwrap().as_ref().map(|m| m.iter().filter(|(_, r)| leases.contains(&r.lease)).map(|(id, r)| (id.clone(), r.clone())).collect()).unwrap_or_default();
    let now = wall_ms();
    for (id, r) in list {
        // (what a call refused meanwhile answers too)
        let note = park_note(note, get(ctx, &id).as_ref(), resume_after.is_some(), now);
        let first = {
            let mut calls = r.calls.lock().unwrap();
            let first = calls.park.is_none();
            calls.park.get_or_insert_with(|| note.clone());
            first
        };
        if let (true, Some(after)) = (first, resume_after) {
            set(ctx, &id, |s| s.resume_after = after.to_string());
        }
        park_ready(ctx, &id, &r);
    }
}

/// What a session parked for a timed lease while in use says of its end ([`park`]).
const RESTARTS_BY_ITSELF: &str = "restarts by itself after it, same app, args, tree and env; in-memory state is not kept";

/// Starts again, under its id, each session parked for a timed lease while in use once that
/// lease has ended ([`crate::lease::sched::Sched::park_live`]), as a master restart places one
/// again: same app, args, tree and env; what its app held in memory is gone. Its saves since
/// are in its tree, and its app starts from that. One its lane has started anew meanwhile
/// (the same app or package, active) stays ended.
fn resume_parked(ctx: &Arc<Ctx>) {
    for claim in resume_due_claims(ctx) {
        if let Err((e, claim)) = spawn_run(ctx, claim, false) {
            failed_to_spawn_run(ctx, claim, e);
        }
    }
}

/// Claims the sessions due to start again and publishes their starting state together.
fn resume_due_claims(ctx: &Ctx) -> Vec<Claim> {
    let waiting: Vec<String> = ctx.store.read(|d| d.lives.iter().filter(|x| x.state == "ended" && !x.resume_after.is_empty()).map(|x| x.resume_after.clone()).collect());
    if waiting.is_empty() { return Vec::new(); }
    let open: std::collections::HashSet<String> = ctx.leases.view().into_iter().filter(|l| !matches!(l.state, crate::lease::sched::State::Ended(_))).map(|l| l.id).collect();
    if waiting.iter().all(|after| open.contains(after)) { return Vec::new(); }

    // Publish every member's next start together. A joiner must never wake to a partner
    // still marked ended, including if the master restarts between this update and the launches.
    let changed = ctx.store.update(|d| {
        let mut changed = Vec::new();
        let mut due = Vec::new();
        let mut refused = Vec::new();
        for i in 0..d.lives.len() {
            let x = &d.lives[i];
            if x.state != "ended" || x.resume_after.is_empty() || open.contains(&x.resume_after) { continue; }
            let after = x.resume_after.clone();
            let members = if x.set.is_empty() { vec![i] } else {
                d.lives.iter().enumerate().filter(|(_, o)| o.set == x.set && o.lane == x.lane && o.resume_after == after).map(|(i, _)| i).collect()
            };
            if members.iter().any(|&i| d.lives[i].state != "ended" || running(&d.lives[i].id).is_some() || claimed_by(&d.lives[i].id).is_some()) { continue; }
            let anew = members.iter().find_map(|&i| {
                let x = &d.lives[i];
                d.lives.iter().find(|o| o.active() && o.lane == x.lane && o.app == x.app && o.test == x.test && (x.set.is_empty() || x.set != o.set)).map(|o| o.id.clone())
            });
            if anew.is_none() {
                let claims: Result<Vec<_>, _> = members.iter().map(|&i| {
                    let id = &d.lives[i].id;
                    Claim::acquire(id, LaunchPath::ResumeParked).map_err(|owner| (id.clone(), owner))
                }).collect();
                match claims {
                    Ok(group) => due.extend(group),
                    Err(conflict) => {
                        refused.push(conflict);
                        continue;
                    }
                }
            }
            for i in members {
                let s = &mut d.lives[i];
                s.resume_after.clear();
                if anew.is_none() {
                    s.state = "starting".into();
                    (s.pid, s.run_pid) = (0, 0);
                    s.lease.clear();
                    s.node.clear();
                    s.run_n = 0;
                    s.ask.clear();
                    s.patch = None;
                    (s.touched_ms, s.left_ms, s.ended_ms) = (wall_ms(), 0, 0);
                    s.note = format!("started again by itself: timed {after}, which it was parked for, ended");
                }
                changed.push((s.clone(), after.clone(), anew.clone()));
            }
        }
        (changed, due, refused)
    });
    let (changed, due, refused) = match changed {
        Ok(changed) => changed,
        Err(e) => { eprintln!("devhub state: {e}"); return Vec::new(); }
    };
    for (id, owner) in refused { warn_duplicate(ctx, &id, LaunchPath::ResumeParked, owner); }
    for (x, after, anew) in changed {
        say_in_log(&x, "ended");
        if let Some(other) = anew {
            event(ctx, Sev::Info, format!("live {} ({}) of lane {}: timed {after} ended; not started again, its lane runs {other} now", x.id, x.app, x.lane), Some(&x));
            continue;
        }
        event(ctx, Sev::Info, format!("live {} ({}) of lane {}: timed {after} ended, so it starts again as it was asked; what its app held in memory is not kept", x.id, x.app, x.lane), Some(&x));
    }
    due
}

#[cfg(test)]
fn resume_due(ctx: &Ctx) -> Vec<String> {
    resume_due_claims(ctx).into_iter().map(|claim| claim.id().to_string()).collect()
}

fn park_ready(ctx: &Ctx, id: &str, r: &Running) {
    end_parked(ctx, id, r, true)
}

/// Ends a session whose park note is set once no call is open on it and, with `wait_run`, no run
/// is in flight (a sweep's end of an orphaned run waits for none: [`sweep_end`]).
fn end_parked(ctx: &Ctx, id: &str, r: &Running, wait_run: bool) {
    let note = {
        let mut calls = r.calls.lock().unwrap();
        let Some(note) = calls.ready_to_park(wait_run && r.inflight.load(std::sync::atomic::Ordering::SeqCst)) else { return };
        note
    };
    // the one line its lane gets (its next call on it answers with it; the inbox has it too): why
    // it ended, and the one command that starts it again, or that it starts again by itself
    let progress = end_progress(ctx, id);
    if let Some(x) = set(ctx, id, |s| record_park_with(s, &note, wall_ms(), progress.as_ref())) {
        ctx.store.event(if x.note.starts_with("parked for ") { "live_parked" } else { "live" }, Sev::Info, format!("live {id} ({}) of lane {}: {}", x.app, x.lane, x.note), Some(crate::state::live_value(&x).to_json()));
    }
    ctx.leases.release_live(&r.lease);
}

#[cfg(test)]
fn record_park(x: &mut Live, note: &str, now: u64) {
    record_park_with(x, note, now, None);
}

fn record_park_with(x: &mut Live, note: &str, now: u64, progress: Option<&RunProgress>) {
    x.state = "ended".into();
    x.ended_ms = now;
    if !x.held.is_empty() && note.starts_with("parked") {
        x.resume_after.clear();
        x.note = note.trim_end_matches(RESTARTS_BY_ITSELF).trim_end_matches("; ").into();
    } else {
        x.note = if note.ends_with(RESTARTS_BY_ITSELF) { note.into() } else { format!("{note}; `devhub live again {}` starts it again", x.id) };
    }
    settle_open_patch(x, now, progress);
}

/// Consume the previous parked hold when placing its replacement. The notice precedes even
/// the new session's first queue line.
fn cold_start(previous: &mut Live, new: &Live) -> Option<String> {
    if previous.active() || previous.held.is_empty() { return None; }
    previous.held.clear();
    (previous.note.starts_with("parked") && previous.tree == new.tree && previous.src == new.src && previous.timed == new.timed && previous.also == new.also
        && answers(previous, &new.lane, &new.app, true, new.offline, &new.test_args, &new.needs, &new.pin, &new.avoid, &new.loader, &new.env))
        .then(|| format!("the held session {} was {} at {}; this run started cold", previous.id, previous.note, hhmm(previous.ended_ms)))
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
    JOINED.1.notify_all();
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

/// A session's record names the box it runs on, not the master that files it: two Fab edits on
/// pool boxes read as the user's Fab on the Mac (11-16).
fn event(ctx: &Ctx, sev: Sev, line: String, x: Option<&Live>) {
    ctx.store.event_on(x.map_or("", |x| x.node.as_str()), "live", sev, line, x.map(|x| crate::state::live_value(x).to_json()));
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Live)) -> Option<Live> {
    try_set(ctx, id, f).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); None })
}

fn try_set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Live)) -> Result<Option<Live>, String> {
    let mut was = None;
    let x = ctx.store.update(|d| {
        let x = d.lives.iter_mut().find(|x| x.id == id)?;
        was = Some(x.state.clone());
        let held = (!x.held.is_empty() && x.active()).then(|| x.refusal_key());
        f(x);
        // The saved diagnostic belongs to one refusal. Preserve it on a scheduler park so the
        // next caller can explain its cold start, including after a master restart.
        if let Some(key) = held {
            if x.refusal_key() != key && !(x.state == "ended" && x.note.starts_with("parked")) { x.held.clear(); }
        }
        Some(x.clone())
    })?;
    if let (Some(x), Some(was)) = (&x, was) { say_in_log(x, &was); }
    Ok(x)
}

/// A session's log hears each change of its state that its note explains (parked, failed,
/// ended, started again), in a `[devhub]` line after its app's last: a lane that follows its
/// app's output learns there that the app is gone, and why. A session parked, started again and
/// denied its box said so only in its record, and its lane's script watched the app's output
/// for over an hour (W12708, 11-18).
fn say_in_log(x: &Live, was: &str) {
    if x.state == was || x.state == "running" || x.note.is_empty() || x.log.is_empty() { return; }
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&x.log) {
        use std::io::Write;
        let _ = writeln!(f, "[devhub] {} {}: {}", x.id, x.state, x.note);
    }
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
            if p.outcome == "recovering" && run.jit_restart.is_some() { s.jit_restart = run.jit_restart.clone(); }
            else if !p.jit_notice.is_empty() { s.jit_restart = None; }
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
    wait_tests_with(ctx, id, makepad_remote::lines::client_gone, Duration::from_secs(1));
}

fn wait_tests_with(ctx: &Ctx, id: &str, mut client_gone: impl FnMut() -> bool, poll: Duration) {
    let mut left = false;
    let signal = ctx.store.subscribe("live", &[id]);
    loop {
        let seen = signal.generation();
        if !left && client_gone() {
            left = true;
            if let Some(r) = running(id) { r.release_active_wait(); }
            starter_left(ctx, id);
        }
        let r = running(id);
        let (open, yield_now) = ctx.store.read(|d| {
            let Some(s) = d.lives.iter().find(|x| x.id == id && x.state == "running") else {
                return (false, false);
            };
            let open = s.patch.as_ref().is_some_and(|p| !terminal(p));
            let yield_now = open && s.pid > 0 && r.as_deref().is_some_and(superseded_idle);
            (open, yield_now)
        });
        if !open || yield_now { return; }
        signal.wait(seen, poll);
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

/// The app has no run and waits for a fix: its build or app check failed, and the next save or
/// `session relaunch` starts its next run. Its `live` says `held`; its `inputs` answers "held: ..."
/// (relax's reload::HELD_NO_RUN) while no run's watcher can start.
fn held_app(live: &[String], inputs: &[String]) -> bool {
    live.iter().any(|l| num(l, "held") == Some(1)) || inputs.iter().any(|l| l.contains("\"err\":\"held: "))
}

/// A save while the app has no run and waits for a fix ([`held_app`]): shipped, its files are the
/// next run's, which `session relaunch` starts, followed as a relaunch is ([`follow_relaunch`]):
/// its end is the edit's verdict (that run up, or its errors). A `session relaunch` call ships its
/// saves first and relaunches itself ([`relaunch`]). The save read the app's `inputs` as the
/// session's end before: a session started on a broken tree ended at the save that fixed it
/// (W15724, 11-21).
fn held_relaunch(ctx: &Ctx, id: &str, x: &Live, r: &Running, p: LivePatch, count: usize) {
    if r.relaunch_waits.load(std::sync::atomic::Ordering::SeqCst) { return; }
    let log_from = std::fs::metadata(&x.log).map_or(0, |m| m.len());
    let said = inspect(&x.node, x.pid, "session relaunch").and_then(|lines| {
        if lines.iter().any(|l| l.contains("\"relaunch\":\"queued\"")) { follow_relaunch(ctx, id, x, p, lines, log_from, None) } else { Err(lines.join(" ")) }
    });
    let current = get(ctx, id);
    match said {
        Ok(_) => event(ctx, Sev::Info, format!("live {id}: {count} file(s): the app had no run (the last did not compile); the next one started on them"), current.as_ref()),
        Err(e) => event(ctx, Sev::Warn, format!("live {id}: {count} file(s): the app had no run (the last did not compile); its relaunch: {e}"), current.as_ref()),
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

/// A line of a watch's answer that carries an edit's verdict: an event, other than the watch's own
/// answer when it ran out (`{"event":"none","waited_s":..}`). That answer counted as one, so a
/// watch that ran out never asked the JIT where the edit was: an edit installed in 3.6 s whose
/// check had not reported was said "edit not applied", its `live` "gave no answer", though it was
/// never asked (W11955, 11-17).
fn verdict_line(line: &str) -> bool {
    makepad_strict_json::parse(line.as_bytes()).is_ok_and(|v| v.get("event").and_then(Value::as_str).is_some_and(|e| e != "none"))
}

pub fn start(ctx: Arc<Ctx>) {
    // a previous master's live jobs go on: one whose lease this master took back (its own exec,
    // the same holder) keeps its app and is followed from where its log stands, and so does the
    // run it had asked for (the lease is noted once shipped): the node may be building its JIT
    // for it. Placed again, that lease went unbeaten until it expired and the build was lost
    // (a lease lost 160 s into a dev JIT build on its box, 11-03). Else it is placed again and its
    // app starts again. A restart is no call: each keeps its own touch ([`restarted_to_launch`]).
    let to_launch = {
        let _starting = STARTING.lock().unwrap();
        restarted_to_launch(&ctx)
    };
    StartupGate::release();
    for (x, held, claim) in to_launch {
        event(&ctx, Sev::Info, format!("live {} ({}): the master restarted; {}", x.id, x.app, if held { "its app runs on" } else { "placed again, its app starts again" }), Some(&x));
        if held {
            if let Err((e, claim)) = spawn_reattach(&ctx, x, claim) {
                let note = format!("failed to spawn reattach thread: {e}");
                event(&ctx, Sev::Warn, format!("live {}: {note}", claim.id()), get(&ctx, claim.id()).as_ref());
                let _ = release(&ctx, claim.id(), &note);
                drop(claim);
            }
        } else if let Err((e, claim)) = spawn_run(&ctx, claim, false) {
            failed_to_spawn_run(&ctx, claim, e);
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
            resume_parked(&c);
        }
        let list: Vec<(String, Arc<Running>)> = RUNNING.lock().unwrap().as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        for (id, r) in list {
            // Only the test caller ships its source state and owns the resulting verdict.
            if r.test {
                departed_checks(&c, &id, &r.calls);
                note_save(&c, &id, &r);
                continue;
            }
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
                    let _ = ship_edit(&c2, &id2, &r, files.into_iter().collect(), edit_ms, called, None, None);
                });
            }
        }
        // the idle timeout, from its placement on (a job still queued for a box is not idle)
        let now = wall_ms();
        for x in idle_now(&c, now) {
            let Some(r) = running(&x.id) else { continue };
            // (a test session's saves wait for its next call: one within the timeout keeps it)
            if r.test && r.pending.lock().unwrap().2.is_some_and(|t| t.elapsed() < Duration::from_millis(IDLE_MS)) { continue; }
            if let Some(kept) = idle_kept(&x) {
                if said_once(&x.id, "idle") {
                    event(&c, Sev::Info, format!("live {} ({}) of lane {}: kept past its idle end ({} min with no edit, call or person's input): {kept}", x.id, x.app, x.lane, IDLE_MS / 60_000), Some(&x));
                }
                continue;
            }
            if let Some(what) = kept_by_build(&x, c.leases.built(&x.lease), now) {
                keep_for_build(&c, &x, &r, &what, now);
                continue;
            }
            sweep_end(&c, &x, &r, now, || "ended after 10 min with no edit, call or person's input in its app".into());
        }
        // a session whose starter left, with no call or save since for LEFT_MS (its verdict for
        // nobody counts from when it came): one queued for its box ends, one on its box parks
        // once its run in flight ends
        let left: Vec<Live> = c.store.read(|d| d.lives.iter().filter(|x| left_long(x, now) && !joined(&d.lives, &x.id)).cloned().collect());
        for x in left {
            let note = format!("ended {} min after its starter's call went, with no call or save since", LEFT_MS / 60_000);
            let Some(r) = running(&x.id) else {
                if x.state == "starting" {
                    let note = format!("{note}; the lane's next call starts it again");
                    event(&c, Sev::Info, format!("live {} ({}) of lane {}: {note}", x.id, x.app, x.lane), Some(&x));
                    let _ = release(&c, &x.id, &note);
                }
                continue;
            };
            if r.pending.lock().unwrap().2.is_some_and(|t| t.elapsed() < Duration::from_millis(LEFT_MS)) { continue; }
            sweep_end(&c, &x, &r, now, || note);
        }
        let mut woke = SHIP_WAKE.0.lock().unwrap();
        if !*woke { woke = SHIP_WAKE.1.wait_timeout(woke, HOUSEKEEPING).unwrap().0; }
        *woke = false;
    });
}

/// The master's start squares the sessions with the leases, both ways, a line each: an ended
/// session whose lease still runs frees it, and a running one whose lease is gone ends (its app
/// went with it). The rest go on: each active session, and whether its lease is held; one not held
/// is set to be placed again. A restart is no call: each keeps its own touch, and its idle end with
/// it ([`idle_now`]); a queued one whose starter left ends as the left sweep would end it
/// ([`left_long`]), not placed again. (Every
/// string of the store names a lease the restart keeps, an ended session's too: W9068's lease
/// outlived its release, 11-15.)
fn restarted_to_launch(ctx: &Ctx) -> Vec<(Live, bool, Claim)> {
    let open: std::collections::HashSet<String> = ctx.leases.view().into_iter().filter(|l| !matches!(l.state, crate::lease::sched::State::Ended(_))).map(|l| l.id).collect();
    let (active, leaked): (Vec<Live>, Vec<Live>) = ctx.store.read(|d| d.lives.iter().filter(|x| x.active() || open.contains(&x.lease)).cloned().partition(|x| x.active()));
    for x in leaked.iter().filter(|x| claimed_by(&x.id).is_none() && !active.iter().any(|a| a.lease == x.lease)) {
        if ctx.leases.release_live(&x.lease) {
            event(ctx, Sev::Warn, format!("live {} ({}): {} ({}), but its lease {} ran on: freed", x.id, x.app, x.state, x.note, x.lease), Some(x));
        }
    }
    let mut on = Vec::new();
    let now = wall_ms();
    let joins: std::collections::HashSet<String> = active.iter().map(|x| x.joins.clone()).collect();
    for x in active {
        if open.contains(&x.lease) {
            if let Some(claim) = try_claim_or_warn(ctx, &x.id, LaunchPath::Reattach) {
                on.push((x, true, claim));
            }
            continue;
        }
        // (a session still starting waits for its box, or builds on one: placed again)
        if x.state == "starting" {
            if let Some(owner) = claimed_by(&x.id) {
                warn_duplicate(ctx, &x.id, LaunchPath::Restarted, owner);
                continue;
            }
            if left_long(&x, now) && !joins.contains(&x.id) {
                let note = format!("ended {} min after its starter's call went, with no call or save since: the master's restart did not place it again; the lane's next call starts it again", LEFT_MS / 60_000);
                event(ctx, Sev::Info, format!("live {} ({}) of lane {}: {note}", x.id, x.app, x.lane), Some(&x));
                let _ = release(ctx, &x.id, &note);
                continue;
            }
            let Some(claim) = try_claim_or_warn(ctx, &x.id, LaunchPath::Restarted) else { continue };
            let again = set(ctx, &x.id, |s| {
                s.state = "starting".into();
                (s.pid, s.run_pid) = (0, 0);
                s.lease.clear();
                s.run_n = 0;
                s.ask.clear();
                s.patch = None;
            });
            on.push((again.unwrap_or(x), false, claim));
            continue;
        }
        if claimed_by(&x.id).is_some() { continue; }
        let gone = if x.lease.is_empty() { "it held no lease".to_string() } else { format!("its lease {} had ended", x.lease) };
        let note = format!("it was {}, but {gone} when the master restarted; `devhub live again {}` starts it again", x.state, x.id);
        let progress = open_progress(&x);
        let now = set(ctx, &x.id, |s| {
            s.state = "ended".into();
            s.ended_ms = wall_ms();
            s.note = note.clone();
            settle_open_patch(s, s.ended_ms, progress.as_ref());
        });
        event(ctx, Sev::Warn, format!("live {} ({}) ends: {note}", x.id, x.app), now.as_ref());
    }
    on
}

#[cfg(test)]
fn restarted(ctx: &Ctx) -> Vec<(Live, bool)> {
    restarted_to_launch(ctx).into_iter().map(|(x, held, _claim)| (x, held)).collect()
}

/// Why a release or cancel of `lease` by its id is refused: it is an active session's, and a
/// session's lease ends by its own release or its end only. (A frame sequence's answer prints the
/// session's lease to copy its frames from, an agent released that lease as the sequence's own and
/// its session W9349 ended under it, 11-15.)
pub(crate) fn owned_lease(lives: &[Live], lease: &str) -> Option<String> {
    lives.iter().find(|x| x.active() && !x.lease.is_empty() && x.lease == lease)
        .map(|x| format!("{lease} is live session {}'s lease, and ends with it: `devhub live release {}`", x.id, x.id))
}

/// Ends a live job at once: it takes no more saves, no longer counts against its lane's next
/// start and is not placed again by a master restart; its lease goes, its app with it. Its run
/// finds it ended whenever that returns. (Ending it was left to the run: one that never returned,
/// its box's hub not closing the run, kept a live run "running (released)" and its lane's next start
/// refused until it was cleared by hand, 11-03.) Its lease is the one the store records, followed
/// here or not: a session a master restart had not taken back yet released nothing, and said
/// nothing (W9068, 11-15). What became of its lease.
fn release(ctx: &Ctx, id: &str, note: &str) -> Result<String, String> {
    let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
    let progress = open_progress(&x);
    try_set(ctx, id, |s| {
        s.note = note.into();
        s.held.clear();
        // (released by its lane: it no longer starts again by itself)
        s.resume_after.clear();
        if s.active() {
            s.state = "ended".into();
            s.ended_ms = wall_ms();
        }
        settle_open_patch(s, wall_ms(), progress.as_ref());
    })?;
    Ok(free_lease(ctx, id, x.lease))
}

fn free_lease(ctx: &Ctx, id: &str, lease: String) -> String {
    let r = RUNNING.lock().unwrap().as_mut().and_then(|m| m.remove(id));
    let asked = ASKED.lock().unwrap().remove(id);
    let lease = r.map_or(lease, |r| r.lease.clone());
    match (lease.as_str(), asked) {
        // (still asked, or granted while its sources ship: it goes with the session, before the
        // release answers)
        ("", Some(q)) if ctx.leases.release_live(&q) => format!("its asked lease {q} given up"),
        ("", Some(q)) => format!("its lease {q} had ended"),
        ("", None) => "it held no lease".into(),
        (l, _) if ctx.leases.release_live(l) => format!("its lease {l} freed"),
        (l, _) => format!("its lease {l} had ended"),
    }
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
        } else if !x.held.is_empty() {
            continue;
        } else if let Some(head) = lane_landed(ctx, &x, &mut pass) {
            let landed = format!("its lane landed ({} in work, its worktree clean) and the lane has been quiet for {} min (no call, lease, job or file change)", super::short(&head), DONE_IDLE_MS / 60_000);
            let now = wall_ms();
            if let Some(kept) = kept_from_landed(&x, person_input_at(&x, now), now) {
                if said_once(&x.id, if x.visible { "landed" } else { "landed, used" }) { event(ctx, Sev::Info, format!("live {} ({}) of lane {}: kept, though {landed}: {kept}", x.id, x.app, x.lane), Some(&x)); }
                continue;
            }
            landed
        } else {
            continue;
        };
        if why.starts_with("its lane landed") {
            if let Some(r) = running(&x.id) {
                sweep_end(ctx, &x, &r, wall_ms(), || format!("released: {why}"));
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

/// Why an edit has no verdict: the watch ran out and the JIT's `live` gave no answer (its app is
/// gone?), or it came back early without one: what it said (an error, a closed link, another
/// answer).
fn no_verdict(answer: &Result<Vec<String>, String>, waited: Duration) -> String {
    if waited.as_secs() + 1 >= WATCH_S {
        return format!("no patch event in {WATCH_S} s, and the JIT did not say where the edit is (its `live` gave no answer)");
    }
    let said = match answer {
        Err(e) => format!("error: {e}"),
        Ok(l) if l.is_empty() => "no answer (the link closed?)".to_string(),
        Ok(l) => format!("answer: {}", l.join(" | ")),
    };
    format!("the JIT's watch came back after {} ms with no patch event ({})", waited.as_millis(), crate::proto::clean(&said, 300))
}

/// Where the JIT is with an edit whose watch ran out with no verdict ([`edit_state`]).
#[derive(Debug, PartialEq)]
enum EditState {
    /// still on it: what it does
    On(String),
    /// installed (the patch count rose after the ship), its verdict event not seen: how many patch
    /// groups run while their check has not reported, and what the oldest one waits on (the JIT's
    /// `unverified_why`; "" from a JIT that does not say)
    Installed(u64, String),
    /// done with it, and nothing installed
    Unchanged,
}

/// The JIT's `live` answer read against the patches it had at the ship: `editing` (its watcher
/// round, the save to its verdict), `patches` and `unverified` (relax inspect cmd_live). None: no
/// answer, or one from a JIT that does not say `editing`.
fn edit_state(live: &Result<Vec<String>, String>, at_ship: Option<u64>) -> Option<EditState> {
    let lines = live.as_ref().ok()?;
    let field = |k: &str| lines.iter().find_map(|l| num(l, k));
    let unverified = field("unverified").unwrap_or(0);
    if at_ship.zip(field("patches")).is_some_and(|(n, now)| now > n) {
        let why = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("unverified_why")?.as_str().map(str::to_string)).unwrap_or_default();
        return Some(EditState::Installed(unverified, why));
    }
    if field("editing")? > 0 { return Some(EditState::On("its watcher round runs: the edit's read, check and compile".into())); }
    // (no count at the ship: whether it installed one is not known)
    at_ship.and(field("patches")).map(|_| EditState::Unchanged)
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
/// writes a line in pieces; a window cut between them lost "relax live:   struct ", 11-02).
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
pub(crate) fn jit_fault(line: &str) -> bool {
    jit_fault_of("", line)
}

/// A fault of the JIT itself in a line of package `app`'s output. A test program of one of the
/// JIT's own crates is the JIT: its tests panic in those sources on purpose and print every line
/// the JIT prints (the ice-scope test's panic, lifeline's "JIT thread .. panicked" of its restart
/// test), so its output says nothing about the session's JIT, whose fault there shows as a run
/// that ended without a verdict. Every live run of relax-compiler --lib was stopped at the first
/// such line and restarted, then failed as a JIT error (11-12).
fn jit_fault_of(app: &str, line: &str) -> bool {
    !jit_crate(app) && jit_words(line)
}

/// A test program of one of the JIT's own crates ([`jit_fault_of`]).
fn jit_crate(app: &str) -> bool {
    JIT_SOURCES.iter().any(|(package, _)| *package == app)
}

/// A place in the JIT's own sources.
fn in_jit(location: &str) -> bool {
    JIT_SOURCES.iter().any(|(_, dir)| location.contains(dir))
}

/// A panic of package `app`'s run, by its structured frame (its event, or its `report:` line):
/// the JIT's fault when it is located in the JIT's sources, or when the JIT made it (a failed
/// update, a compile error in a call: any kind but a program's own `panic`), whose message is the
/// JIT's words. A program's own panic is judged by where it is, never by what it says: a lane's
/// test whose assertion quoted an internal compiler error was said to be a JIT error, "not your
/// change" (11-23), and lanes carry on past those.
fn panic_fault(app: &str, kind: &str, message: &str, location: &str) -> bool {
    !jit_crate(app) && (in_jit(location) || kind != "panic" && jit_words(message))
}

/// The JIT's words for its own faults in a line.
fn jit_words(line: &str) -> bool {
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

/// Where [`fault_line`] is in a run's output: in a test's captured output, or in a panic's
/// message (the lines after its `panicked at` header, up to its note or `report:` frame).
#[derive(Clone, Copy, Default)]
struct Scan {
    captured: bool,
    message: bool,
}

/// A fault of the JIT in a line of package `app`'s run output. A panic's header is judged by where
/// it panicked, its `report:` frame by its structured fields ([`panic_fault`]); the lines between
/// are its message, the program's words, and judged not at all.
fn fault_line(app: &str, line: &str, scan: &mut Scan) -> bool {
    if line.starts_with("---- ") && line.ends_with(" stdout ----") { scan.captured = true; }
    if line == "failures:" || line.starts_with("test result:") { scan.captured = false; }
    if scan.captured { return false; }
    let t = line.trim_start();
    if let Some(at) = t.strip_prefix("thread '").and_then(|r| r.split_once(" panicked at ")).map(|(_, at)| at) {
        scan.message = true;
        return !jit_crate(app) && in_jit(at);
    }
    if let Some(frame) = t.find("report: {").map(|i| &t[i + "report: ".len()..]) {
        scan.message = false;
        let v = makepad_strict_json::parse(frame.as_bytes()).ok();
        let f = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
        return v.is_some() && panic_fault(app, &f("kind"), &f("message"), &f("location"));
    }
    // (its message ends at its note, a blank line or the harness's verdict line: whatever its words,
    // a quoted JIT line among them)
    if scan.message && (t.is_empty() || t.starts_with("note: run with") || t.starts_with("test ") && t.contains(" ... ")) { scan.message = false; }
    !scan.message && jit_fault_of(app, line)
}

fn terminal(p: &LivePatch) -> bool {
    p.tests.is_some() || matches!(p.outcome.as_str(), "refused" | "compile_error" | "failed" | "jit_error" | "none" | "skipped" | "unchanged" | "told" | "ended")
}

/// How long a save of files the app read waits for its JIT to tell the app ([`told_verdict`]):
/// the watcher's round after the ship's ledger, well under a second, and the frame after it. Past
/// it the verdict says so, and the app takes the file in at its next read (a JIT from before
/// resources tells it nothing).
const TOLD_WAIT: Duration = Duration::from_secs(10);

/// A save of files the session's app read at run time that are none of its sources (a kit, a
/// level, a data file): its JIT tells the app with the save's patch, or alone (relax's
/// `Patched::resources`, platform's `Event::ResourcesChanged`). The verdict waits for that
/// count and the first frame drawn after it, then asks whether the app read the files again
/// (relax inspect `inputs`' `told` lines): "told" either way, the second saying the app has no
/// reload for it. Before, such a save said "edit does not reach" the app and nothing reached a
/// running game until a commit and a fresh session (a Sandbox kit, 11-22).
fn told_verdict(ctx: &Ctx, id: &str, x: &Live, r: &Running, p: &mut LivePatch, files: &[String], count: usize) {
    let t = Instant::now();
    let newer = newer_saves(ctx, x, Some(r));
    let mut told_at = None;
    while t.elapsed() < TOLD_WAIT && !newer() {
        let live = inspect(&r.node, x.pid, "live").unwrap_or_default();
        let field = |k: &str| live.iter().find_map(|l| num(l, k));
        if told_at.is_none() { told_at = field("patches").filter(|n| p.patches_at_ship.is_none_or(|s| *n > s)); }
        if told_at.is_some_and(|n| field("frame_patch").is_some_and(|f| f >= n)) {
            p.frame_ms = wall_ms();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let of = format!("{{\"cmd\":\"inputs\",\"of\":{}}}", json_str(&files.join("\n")));
    let told: Vec<(String, bool)> = inspect(&r.node, x.pid, &of).unwrap_or_default().iter().filter_map(|l| {
        let v = makepad_strict_json::parse(l.as_bytes()).ok()?;
        Some((v.get("told")?.as_str()?.to_string(), v.get("again")?.as_bool()?))
    }).collect();
    let (again, not): (Vec<&(String, bool)>, Vec<&(String, bool)>) = told.iter().partition(|(_, a)| *a);
    let names = |v: &[&(String, bool)]| v.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>().join(", ");
    p.outcome_ms = wall_ms();
    if told.is_empty() {
        p.outcome = "skipped".into();
        p.reason = format!("{} read {}, and its JIT did not tell it of the change within {} s (a JIT from before resources, or its watcher holds the save): the app takes it in at its next read", x.app, files.join(", "), TOLD_WAIT.as_secs());
    } else {
        p.outcome = "told".into();
        let mut said = Vec::new();
        if !again.is_empty() { said.push(format!("{} changed; it read the file again", names(&again))); }
        if !not.is_empty() { said.push(format!("{} changed; it did not read it again (no reload for it in the app)", names(&not))); }
        p.reason = format!("told {}: {}", x.app, said.join("; "));
    }
    let Some(current) = set_patch(ctx, id, p) else { return };
    let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
    let sev = if p.outcome == "told" && not.is_empty() { Sev::Warn } else { Sev::Info };
    event(ctx, sev, format!("live {id}: {count} file(s) {} (save → shipped {}, → told {}, → frame {}): {}", p.outcome, after(p.shipped_ms), after(p.outcome_ms), after(p.frame_ms), p.reason), Some(&current));
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
    x.loader.is_empty() && (p.reason.contains("needs a JIT rebuild:") || p.reason.contains(MACROS_DIFFER))
}

/// A run's refusal when the tree's proc macros are not those its JIT has built in (relax
/// native_macros.rs `Mismatch`): only a JIT built from the tree runs it, as "needs a JIT rebuild"
/// says of its other inputs. A held test harness restarted on a save of 57 files that changed
/// them, on the same binary, and held again on this (W25685, W25686, 11-27).
const MACROS_DIFFER: &str = "the proc macros in this tree differ from those built into";

fn rebuild_tests(x: &Live, p: &LivePatch) -> bool {
    x.test && p.tests.is_none() && needs_devhub_rebuild(x, p)
}

/// A relaunch devhub serves itself: an app session on a JIT devhub picked, which its host cannot
/// rebuild ([`needs_devhub_rebuild`]); one pinned to a JIT goes to its host.
fn devhub_relaunch(x: &Live, command: &str) -> bool {
    !x.test && x.loader.is_empty() && relaunch_anyway(command).is_some()
}

/// Whether `command` is a plain `session relaunch` (Some(false)) or one that does not let the app
/// postpone it (Some(true): devhub's `--anyway`, the JIT's `anyway=1`); None: any other call.
fn relaunch_anyway(command: &str) -> Option<bool> {
    match command.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["session", "relaunch"] => Some(false),
        ["session", "relaunch", "--anyway" | "anyway=1"] => Some(true),
        _ => None,
    }
}

/// Whether `command` is `session relaunch --fresh` (`--anyway` beside it changes nothing): a real
/// quit of the app and a new process on the session's home and lease ([`relaunch`]), devhub's own
/// for any app session. A walk that tests going offline and linking again from disk needs the app
/// to quit for real; a relaunch in place keeps its process and state (CREATE, 11-27).
fn fresh_relaunch(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.starts_with(&["session", "relaunch"]) && words.contains(&"--fresh") && words[2..].iter().all(|w| matches!(*w, "--fresh" | "--anyway" | "anyway=1"))
}

/// A `session relaunch` call with devhub's `--anyway` put in the JIT's words (`anyway=1`), for a
/// call its JIT answers itself (`process=1`, a test's, a pinned JIT's); None: no `--anyway` in it.
fn anyway_in_jit_words(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    (words.starts_with(&["session", "relaunch"]) && words.contains(&"--anyway"))
        .then(|| words.iter().map(|w| if *w == "--anyway" { "anyway=1" } else { *w }).collect::<Vec<_>>().join(" "))
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
    if stop { let _ = stop_run(ctx, &r, x); }
}

/// A new process on the overlay as shipped; `p.reason` says why. Tests restart at once, apps
/// only on their lane's relaunch. The replacement's verdict belongs to this edit.
fn restart_session(ctx: &Ctx, id: &str, x: &Live, r: &Running, p: &mut LivePatch, stop: bool) {
    restart_session_in(ctx, id, x, r, p, stop.then_some(QUIT_GRACE));
}

/// [`restart_session`], its old process given `quit` (None: not stopped here) to exit on its own
/// quit; how it stopped (None: not stopped, or it could not be and the session ends).
fn restart_session_in(ctx: &Ctx, id: &str, x: &Live, r: &Running, p: &mut LivePatch, quit: Option<Duration>) -> Option<Stopped> {
    p.tests = None;
    p.jit_notice.clear();
    p.verdict_ms = 0;
    p.frame_ms = 0;
    p.outcome = "recovering".into();
    set_test_patch(ctx, id, x, p)?;
    {
        let _calls = r.calls.lock().unwrap();
        r.restart.store(true, SeqCst);
    }
    event(ctx, Sev::Info, format!("live {id}: restarting its JIT warm on {}: {}", x.node, p.reason), Some(x));
    let stopped = stop_run_in(ctx, r, x, quit?);
    if let Err(e) = &stopped {
        // no restart comes: the edit's answer is that, and the session ends (its lease's end stops
        // what carries the lease's mark). Kept running beside a JIT it could not stop, it tried
        // again on each later edit and failed the same way (W7246, 11-14). A lease that ended
        // under it as the restart began (its hung check, a cancel) took the JIT with it: that end
        // is the answer. It read "pid did not stop" for a pid already gone with its lease (W7372).
        let why = if super::leg::lease_held(ctx, &r.lease) {
            format!("its JIT could not be restarted: pid {} did not stop ({e})", x.pid)
        } else {
            let how = lease_end(ctx, &r.lease).map_or(String::new(), |h| format!(" ({h})"));
            format!("its lease {} ended{how} as its JIT restarted, and pid {} stopped with it", r.lease, x.pid)
        };
        p.outcome = "failed".into();
        p.reason = format!("{why}: the session ends ({})", p.reason);
        p.verdict_ms = wall_ms();
        r.restart.store(false, std::sync::atomic::Ordering::SeqCst);
        set_test_patch(ctx, id, x, p);
        event(ctx, Sev::Warn, format!("live {id}: {why}: the session ends"), Some(x));
        if let Err(e) = release(ctx, id, &why) {
            event(ctx, Sev::Warn, format!("live {id}: ending it: {e}"), Some(x));
        }
    }
    stopped.ok()
}

/// How long a restart's old process has to exit on its own quit before it is stopped.
const QUIT_GRACE: Duration = Duration::from_secs(2);
/// How long a fresh relaunch's app has to exit on its quit, as a real quit: saving what it keeps
/// and closing its links take longer than a restart's quit. One that has not exited by then is
/// stopped, and the answer says its quit did not finish.
const FRESH_QUIT: Duration = Duration::from_secs(10);

/// The answer of a fresh relaunch of `old` ([`relaunch`]) at the session as it is now (`now`): None
/// until its new process is up, a process of a later run than the one it quit.
fn fresh_said(old: &Live, now: &Live, stopped: Stopped) -> Option<String> {
    (now.pid != 0 && now.run_n > old.run_n && now.pid != old.pid).then(|| {
        let how = match stopped {
            Stopped::Quit(ms) => format!("quit and exited in {:.1} s", ms as f64 / 1000.0),
            Stopped::Killed => format!("did not exit within {} s of its quit, so it was stopped (its quit did not finish)", FRESH_QUIT.as_secs()),
            Stopped::Ended => "had ended already".into(),
        };
        let home = if !now.home_dir.is_empty() { now.home_dir.clone() }
            else if let Some((_, h)) = now.env.iter().find(|(k, _)| k == "MAKEPAD_TEST_HOME") { h.clone() }
            else { format!("{} (the box's own)", if now.box_home.is_empty() { "-" } else { &now.box_home }) };
        format!("live {}: pid {} {how}; pid {} runs on the same home {home}, lease {} kept", now.id, old.pid, now.pid, now.lease)
    })
}

/// A fresh relaunch's answer once its old process has exited: its new process up, or the
/// session's end. While the app builds and starts again the call waits; a client that leaves is
/// told how to wait for it.
fn fresh_up(ctx: &Ctx, id: &str, old: &Live, stopped: Stopped) -> Result<Vec<String>, String> {
    let signal = ctx.store.subscribe("live", &[id]);
    loop {
        let seen = signal.generation();
        let now = get(ctx, id).ok_or_else(|| not_running(ctx, id))?;
        if let Some(line) = fresh_said(old, &now, stopped) { return Ok(vec![line]); }
        if !now.active() { return Err(format!("{id}: pid {} exited, and its new process did not start: {}", old.pid, now.note)); }
        if makepad_remote::lines::client_gone() { return Err(format!("{id}: this call's client has gone; pid {} exited and its new process starts (`devhub live wait {id} frame`)", old.pid)); }
        signal.wait(seen, Duration::from_secs(1));
    }
}

/// Own shipping until the replacement is up. In particular, a relaunch immediately after a
/// save includes it even if the file monitor has not delivered that save yet.
/// `fresh`: a real quit and a new process on the same home and lease ([`fresh_relaunch`]).
fn relaunch(ctx: &Ctx, id: &str, resend: bool, anyway: bool, fresh: bool) -> Result<Vec<String>, String> {
    let r = running(id).ok_or_else(|| not_running(ctx, id))?;
    let signal = ctx.store.subscribe("live", &[id]);
    // (a refused edit's wait for its recovery, up to WATCH_S, ends for this call: patch_verdict)
    struct Asked<'a>(&'a Running);
    impl Drop for Asked<'_> {
        fn drop(&mut self) { self.0.relaunch_waits.store(false, std::sync::atomic::Ordering::SeqCst); }
    }
    r.relaunch_waits.store(true, std::sync::atomic::Ordering::SeqCst);
    let _asked = Asked(&r);
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
        if !files.is_empty() { ship_edit(ctx, id, &r, files, edit_ms, wall_ms(), Some(current), None)?; }
        let mut x = get(ctx, id).filter(|x| x.active() && x.pid > 0).ok_or_else(|| not_running(ctx, id))?;
        x.load_text()?;
        let mut p = x.patch.clone().unwrap_or_else(|| LivePatch { edit_ms, shipped_ms: wall_ms(), ..Default::default() });
        if p.outcome == "failed" { return Err(p.reason); }
        // Later unrelated saves may have replaced the patch's display text. The JIT keeps all
        // pending restart reasons. Ordinary relaunches retain its in-process state transfer.
        let by_devhub = fresh || needs_devhub_rebuild(&x, &p);
        let session = if by_devhub { Vec::new() } else { inspect(&x.node, x.pid, "session")? };
        if !by_devhub && !session.iter().any(|l| l.contains("needs a JIT rebuild:")) {
            let pending = session.iter().find_map(|l| num(l, "restart_pending")).unwrap_or(0);
            if let Some(done) = resend.then(|| relaunch_applied(id, x.pid, pending)).flatten() { return Ok(vec![done]); }
            // (an app that postponed a restart earlier in this run says nothing new to this one)
            let earlier = if anyway { None } else { postponed_before(&log_since(&x.log, x.run_log_start)) };
            let log_from = std::fs::metadata(&x.log).map_or(0, |m| m.len());
            let lines = inspect(&x.node, x.pid, if anyway { "session relaunch anyway=1" } else { "session relaunch" })?;
            // (followed also with no restart pending: the run after it may fail its check, and a
            // relaunch after a refused save answered only "queued", 11-21)
            if !lines.iter().any(|l| l.contains("\"relaunch\":\"queued\"")) { return Ok(lines); }
            return follow_relaunch(ctx, id, &x, p, lines, log_from, earlier);
        }
        if x.patch.is_none() { set(ctx, id, |s| s.patch = Some(p.clone())); }
        p.reason = if fresh { "the lane asked for session relaunch --fresh: a real quit, then a new process on the same home" } else { "the lane asked for session relaunch" }.into();
        let stopped = restart_session_in(ctx, id, &x, &r, &mut p, Some(if fresh { FRESH_QUIT } else { QUIT_GRACE }));
        if p.outcome == "failed" { return Err(p.reason); }
        if fresh {
            let Some(stopped) = stopped else { return Err(format!("{id}: the session changed as its relaunch began (a newer edit or run): nothing was quit; ask again")) };
            restarting = true;
            return fresh_up(ctx, id, &x, stopped);
        }
        restarting = true;
        Ok(vec![format!("live {id}: relaunch queued through devhub; resolving the JIT for the current sources")])
    })();
    // On success the replacement's watcher releases shipping, as it does on the first start.
    if !restarting { finish_call(ctx, id, &r); }
    result
}

/// The word a client adds to an observed call it sends again after a master or hub restart cut
/// it ([`RESENT`]): devhub looks whether the cut call's effect is there and does it only if not.
pub(crate) const RESEND: &str = "--resend";

/// The calls whose effect devhub looks at itself when a cut one is sent again ([`RESEND`]): a
/// relaunch (whether an edit still waits for a restart). Any other act cut by a restart may have
/// reached the app, and is not sent twice. A lane's relaunch was cut before it reached the app,
/// the call said only that this wasn't known, and its script went on to grab the old code
/// (W11772, 11-17).
pub(crate) const RESENT: [&[&str]; 1] = [&["session", "relaunch"]];

/// A resent relaunch's answer when its devhub relaunch goes on (the cut call began it): the
/// restart's new run is asked or starting, and it is not done twice.
fn relaunch_under_way(x: &Live) -> Option<String> {
    let recovering = x.patch.as_ref().is_some_and(|p| p.outcome == "recovering");
    (x.active() && (recovering || x.pid == 0 && x.run_n > 0)).then(|| format!("live {}: the cut relaunch goes on through devhub: its new run starts on the current sources (`devhub live wait {} frame` waits for it)", x.id, x.id))
}

/// A resent relaunch's answer when no edit waits for a restart (`pending`): the cut one ran, or
/// had nothing to apply. None: it is sent.
fn relaunch_applied(id: &str, pid: u32, pending: u64) -> Option<String> {
    (pending == 0).then(|| format!("live {id}: no edit waits for a restart: the app (pid {pid}) runs the current sources; the cut relaunch ran or had nothing to apply"))
}

/// The edits the session's JIT holds for a restart.
fn restarts_waiting(x: &Live) -> Result<u64, String> {
    Ok(inspect(&x.node, x.pid, "session")?.iter().find_map(|l| num(l, "restart_pending")).unwrap_or(0))
}

/// How a session rejoins its runs after a master restart: the run it noted, or its last ask
/// ([`Live::ask_after`]) asked again under its name. Its box answers that from the run of that
/// name whatever its number, from its JIT resolution going on, or starts it if it never got the
/// ask. A run's number is no guess: a lease's runs share their numbers with its looks and gets.
/// A relaunch's new run started as the master restarted, and the next master took the run after
/// the noted one for it: the lane's first look, a grab long ended with exit 0. It followed that,
/// and the user's demo session ended "the app exited (0)" after its JIT rebuilt (W25972, 11-27).
#[derive(Debug, PartialEq)]
enum Rejoin {
    Follow(u64),
    Ask,
}

fn rejoin(x: &Live) -> Rejoin {
    let asked = !x.ask.is_empty() && x.ask_after == x.run_n;
    if x.run_n > 0 && !asked { Rejoin::Follow(x.run_n) } else { Rejoin::Ask }
}

/// Every path of the session's tree that may differ from what it shipped: its saves vs HEAD,
/// nested trees' too, and every path it shipped before (a restore, a deleted untracked file). The
/// ship sends only what differs from its sources.
fn saved_paths(ctx: &Ctx, tree: &Path, x: &Live) -> Result<Vec<String>, String> {
    let mut files: BTreeSet<String> = x.sources.keys().cloned().collect();
    let d = ctx.monitor.dirty(tree)?;
    files.extend(d.changed.into_iter().chain(d.deleted));
    files.extend(nested_dirty(ctx, tree)?);
    Ok(files.into_iter().collect())
}

/// How often a relaunch the JIT queued looks at its JIT again.
const RELAUNCH_LOOK: Duration = Duration::from_millis(200);

/// A relaunch its JIT queued in place (the app restarts with its state in its own process, the
/// pid kept) is followed to its end: the new run has loaded the sources (no edit waits for a
/// restart), and the edit is applied; or the app postponed it, and the call fails with the app's
/// reason. It said "queued" and nothing after: the session showed its restart pending, the pid
/// unchanged, though the app had restarted, and the lane released it for a fresh one (11-06).
/// No clock ends it: the session ending does, or the client it answers going. `earlier`: the app's
/// postponement of a restart earlier in this run ([`postponed_before`]), which this one meets too.
fn follow_relaunch(ctx: &Ctx, id: &str, x: &Live, mut p: LivePatch, mut lines: Vec<String>, log_from: u64, earlier: Option<String>) -> Result<Vec<String>, String> {
    loop {
        if get(ctx, id).is_none_or(|s| !s.active() || s.pid != x.pid) { return Err(not_running(ctx, id)); }
        if makepad_remote::lines::client_gone() { return Err(format!("{id}: this call's client has gone; its relaunch is queued in the app")); }
        // (a look that fails meanwhile is looked at again: the process's end ends the session)
        let live = inspect(&x.node, x.pid, "live").unwrap_or_default();
        let log = log_since(&x.log, log_from);
        match relaunch_end(&live, &log).or_else(|| earlier.clone().map(RelaunchEnd::Postponed)) {
            Some(RelaunchEnd::Done) => {
                (p.outcome, p.reason, p.outcome_ms) = ("patched".into(), "restarted in its process on the current sources".into(), wall_ms());
                set_test_patch(ctx, id, x, &p);
                set(ctx, id, restart_ran);
                lines.push(format!("live {id}: restarted in its process (pid {}) on the current sources", x.pid));
                return Ok(lines);
            }
            Some(RelaunchEnd::Postponed(why)) => {
                p.reason = format!("the app postponed its restart: {why}");
                set_test_patch(ctx, id, x, &p);
                return Err(postponed_said(id, &why));
            }
            Some(RelaunchEnd::Refused(why)) => {
                (p.outcome, p.reason, p.outcome_ms) = ("refused".into(), format!("the restarted run did not compile: {why}"), wall_ms());
                set_test_patch(ctx, id, x, &p);
                return Err(format!("{id}: the restarted run did not compile: {why} (the app waits with no run, its state kept: the next save starts it)"));
            }
            None => std::thread::sleep(RELAUNCH_LOOK),
        }
    }
}

/// What a relaunch queued in place came to (`relaunch_end`).
#[derive(Debug, PartialEq)]
enum RelaunchEnd {
    /// no edit waits for a restart: the new run loaded the sources
    Done,
    /// the app's `restart postponed (<why it restarts>): <why it waits>`
    Postponed(String),
    /// the new run failed its app check: it ended in place, and the JIT's `run ends in place: <why>`
    /// waits with no run for the next save (its errors are the session's diagnostics)
    Refused(String),
}

/// What a relaunch queued in place came to, from its JIT's `live` look and the session's log since
/// the relaunch, or on its way (None).
fn relaunch_end(live: &[String], log: &str) -> Option<RelaunchEnd> {
    if let Some(why) = log.lines().find_map(|l| l.split_once("relax live: the run ends in place: ")) {
        return Some(RelaunchEnd::Refused(why.1.split("; the next save").next().unwrap_or("").trim().to_string()));
    }
    // (a restart whose next run still boots or checks is not done: that run may end in place; and
    // none is done before its run's end: a relaunch with nothing pending has no restart_pending
    // to see fall)
    let ended = log.contains("run's threads stayed");
    let unfinished = live.iter().find_map(|l| num(l, "restart_unfinished")) == Some(1);
    if ended && !unfinished && live.iter().find_map(|l| num(l, "restart_pending")) == Some(0) { return Some(RelaunchEnd::Done); }
    log.lines().find_map(|l| l.split_once("restart postponed ").map(|(_, why)| RelaunchEnd::Postponed(why.trim().to_string())))
}

/// A relaunch the app postponed: its reason (what a restart now would cost: unsaved work, a turn
/// in flight), and how to restart it now anyway. devhub never does that by itself.
fn postponed_said(id: &str, why: &str) -> String {
    format!("{id}: the app postponed its restart: {why} (it restarts when the app allows it; pass --anyway to restart now: `makepad devhub live act {id} session relaunch --anyway` does not ask the app, as the window caption's restart does not)")
}

/// The app's postponement of a restart earlier in its current run, from the session log of its
/// JIT process before a relaunch: the app says `restart postponed` at its first ask in a run only,
/// and while it postpones the restart stays asked, so a later relaunch hears nothing new from it
/// and waited as long as the app's blocker lasted. Its run's end (`run's threads stayed`), or its
/// restart point, after the postponement ends it.
fn postponed_before(log: &str) -> Option<String> {
    let run = ["run's threads stayed", "the app reached a restart point"].iter()
        .filter_map(|end| log.rfind(end).map(|at| at + end.len())).max().map_or(log, |at| &log[at..]);
    run.lines().filter_map(|l| l.split_once("restart postponed ")).last().map(|(_, why)| why.trim().to_string())
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
/// How the lease `id` ended, as the master still keeps it (None while it lives, or once it is
/// no longer kept).
fn lease_end(ctx: &Ctx, id: &str) -> Option<String> {
    ctx.leases.view().into_iter().find(|l| l.id == id).and_then(|l| match l.state { crate::lease::sched::State::Ended(how) => Some(how), _ => None })
}

fn stop_run(ctx: &Ctx, r: &Running, x: &Live) -> Result<(), String> {
    stop_run_in(ctx, r, x, QUIT_GRACE).map(|_| ())
}

/// How a run's process stopped ([`stop_run_in`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stopped {
    /// it exited on its quit, this long after it was asked
    Quit(u64),
    /// it had not exited when its grace ran out, and was stopped
    Killed,
    /// its run had ended before it was asked
    Ended,
}

/// [`stop_run`], the process given `grace` to exit on its quit: the run's end is its exit.
fn stop_run_in(ctx: &Ctx, r: &Running, x: &Live, grace: Duration) -> Result<Stopped, String> {
    let Some(mut pid) = run_to_stop(x, Some(x)) else { return Ok(Stopped::Ended) };
    let first = Instant::now();
    loop {
        let _ = inspect(&x.node, pid, "quit force=1");
        let (asked, mut moved) = (Instant::now(), None);
        while asked.elapsed() < grace {
            match run_to_stop(x, get(ctx, &x.id).as_ref()) {
                None => return Ok(Stopped::Quit(first.elapsed().as_millis() as u64)),
                Some(now) if now != pid => { moved = Some(now); break; }
                Some(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let Some(now) = moved else { break };
        event(ctx, Sev::Info, format!("live {}: its JIT restarted in place as its restart began (pid {pid}, now {now}): the run goes on, so pid {now} stops too", x.id), Some(x));
        pid = now;
    }
    // The original run is still followed; only its recorded pid is stopped. (A lease that ended
    // stopped it already: its caller says that end.)
    r.held.kill(pid).inspect_err(|e| if super::leg::lease_held(ctx, &r.lease) { event(ctx, Sev::Warn, format!("live {}: stopping pid {pid}: {e}", x.id), Some(x)) }).map(|()| Stopped::Killed)
}

/// The process a stop of `x`'s run quits, at the session as it is now (`now`); none once its run
/// has ended (no session, no process, another run). A pid that moved within the run is a restart
/// in place behind the restart parent, and the run goes on: its new process is the one to stop.
/// Read as stopped, a restart devhub asked (a file the tests read but do not compile) met the
/// JIT's own (statics an earlier run left) and waited 10 min on a run whose new process had given
/// its verdict and waited in the harness for its next run, until its lease ended (W18336, 11-24).
fn run_to_stop(x: &Live, now: Option<&Live>) -> Option<u32> {
    now.filter(|s| s.pid != 0 && s.run_n == x.run_n).map(|s| s.pid)
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
        if crate::run_names::says(line, "", "POISONED:") || crate::run_names::says(line, "", "fault") { report = true; }
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

// W7458 reran tests on old code after an include! change only a restart applied (11-14).
fn restart_offered(log: &str) -> Option<String> {
    log.lines().find_map(|line| line.strip_prefix("relax live: ").and_then(|s| s.split_once(": a restart applies it; offered")).map(|(why, _)| crate::proto::clean(why, 300)))
}

fn run_jit_key(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        let key = line.strip_prefix("devhub: the JIT is this box's dev binary ")
            .or_else(|| line.strip_prefix("devhub: the JIT is this lane's own build "))
            .or_else(|| line.strip_prefix("devhub: the JIT is this box's binary ").and_then(|s| s.strip_suffix(", no cargo")))?;
        (!key.is_empty() && !key.chars().any(char::is_whitespace)).then(|| key.to_string())
    })
}

#[derive(Debug, PartialEq)]
enum JitRestart {
    Restart(String, String),
    Verdict(String),
    Terminal(String),
    Unchanged,
}

// A live test session on an outdated JIT must answer from its replacement, never loop when
// the key missed an input: a fresh harness forgets that the old one restarted (11-15).
fn jit_restart(log: &str, pending: Option<&(String, String)>) -> JitRestart {
    let why = restart_offered(log).filter(|why| why.starts_with("The JIT itself changed"));
    if why.is_none() && pending.is_none() { return JitRestart::Unchanged; }
    let selected = run_jit_key(log);
    let key = selected.as_deref().unwrap_or("unknown");
    if let Some((old, previous)) = pending {
        // (a run that names no key, as a tree lease's "this box's binary built by its lease"
        // line does, is no repeat: only the same key or the offer again is)
        if why.is_some() || selected.as_deref() == Some(old.as_str()) {
            return JitRestart::Terminal(format!("the current JIT {key} still says: {}: devhub's JIT key misses what changed (a devhub bug)", why.as_ref().unwrap_or(previous)));
        }
        let current = selected.map_or_else(|| "the current JIT".to_string(), |k| format!("the current JIT {k}"));
        return JitRestart::Verdict(format!("restarted on {current}: {previous}"));
    }
    match why {
        Some(why) => JitRestart::Restart(key.to_string(), why),
        None => JitRestart::Unchanged,
    }
}

fn finish_tests(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch) {
    let Some(r) = running(id) else { return };
    if rebuild_tests(x, p) {
        restart_session(ctx, id, x, &r, p, true);
        return;
    }
    if p.outcome == "jit_error" && x.jit_restart.is_none() {
        let (sev, line) = verdict_said(id, x, p, Sev::Err, verdict(p).trim_end().to_string());
        if let Some(x) = set_test_patch(ctx, id, x, p) { event(ctx, sev, line, Some(&x)); }
        return;
    }
    if p.outcome != "jit_error" {
        if let Some(reason) = r.fault.lock().unwrap().clone() {
            p.tests = None;
            p.outcome = "jit_fault".into();
            p.reason = reason;
        }
    }
    if matches!(p.outcome.as_str(), "jit_fault" | "restart") && p.tests.is_none() {
        recover(ctx, id, x, p, true);
        return;
    }
    let mut jit = JitRestart::Unchanged;
    match run_log(&r, x.run_n, p.log_start) {
        Err(e) => p.output = format!("live log {}: {e}\n", x.log),
        Ok((end, log)) => {
            p.log_end = end;
            if x.test {
                jit = jit_restart(&log, x.jit_restart.as_ref());
                match &jit {
                    JitRestart::Restart(key, why) => {
                        let mut run = x.clone();
                        run.jit_restart = Some((key.clone(), why.clone()));
                        p.tests = None;
                        p.reason = format!("{why}: the tests start again on it");
                        restart_session(ctx, id, &run, &r, p, true);
                        return;
                    }
                    JitRestart::Terminal(line) => {
                        p.tests = None;
                        p.outcome = "jit_error".into();
                        p.reason = line.clone();
                        p.jit_notice = format!("{line}\n");
                        p.verdict_ms = wall_ms();
                        p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, line.clone(), None);
                    }
                    _ => {}
                }
            }
            if x.test && p.outcome != "jit_error" && !p.tests.as_ref().is_some_and(|t| t.restarted) {
                if let Some(why) = restart_offered(&log) {
                    p.tests = None;
                    p.reason = format!("{why}: the tests start again on it");
                    restart_session(ctx, id, x, &r, p, true);
                    return;
                }
            }
            let mut scan = Scan::default();
            if let Some(reason) = log.lines().find(|l| fault_line(&x.app, l, &mut scan)).filter(|_| p.outcome != "jit_error" && p.outcome != "refused" && p.reason != "patched after refusal" && !p.tests.as_ref().is_some_and(|t| t.restarted)) {
                p.tests = None;
                p.reason = reason.to_string();
                recover(ctx, id, x, p, true);
                return;
            }
            if let Some(t) = &mut p.tests {
                t.warnings = round_warnings(&log, Path::new(&x.tree));
                p.output = failure_output(&log, &t.failures);
            }
            else if p.outcome == "refused" && !log.is_empty() { p.output = log; }
        }
    }
    if let JitRestart::Verdict(line) = jit { p.jit_notice = format!("{line}\n"); }
    if p.outcome == "jit_error" {
        let (sev, line) = verdict_said(id, x, p, Sev::Err, verdict(p).trim_end().to_string());
        if let Some(x) = set_test_patch(ctx, id, x, p) { event(ctx, sev, line, Some(&x)); }
        return;
    }
    if p.tests.is_none() && p.outcome == "refused" && !crate::jit_misses::compile_errors(&format!("{}\n{}", p.reason, p.output)).is_empty() {
        p.outcome = "compile_error".into();
        p.verdict_ms = wall_ms();
    }
    let (sev, line) = verdict_said(id, x, p, if p.tests.as_ref().is_some_and(|t| t.failed == 0) { Sev::Info } else { Sev::Warn }, format!("live {id}: {}", verdict(p).trim_end()));
    if let Some(x) = set_test_patch(ctx, id, x, p) { event(ctx, sev, line, Some(&x)); }
}

/// The event of a test verdict, `sev` and `line` as a new one says them. One the session already
/// holds ([`resume_tests`] after a master's restart reads its run's log again) is said as a
/// replay, with when that run ended, and as info: the inbox has it. W9372's second "verdict 9/10
/// passed", three seconds after a restart, was read as a run of an edit that never shipped (11-15).
fn verdict_said(id: &str, x: &Live, p: &LivePatch, sev: Sev, line: String) -> (Sev, String) {
    let replayed = x.patch.as_ref().is_some_and(|old| old.edit_ms == p.edit_ms && old.verdict_ms != 0 && old.verdict_ms == p.verdict_ms);
    if !replayed { return (sev, line); }
    (Sev::Info, format!("live {id}: replayed after a master restart, the verdict of its run that ended {}: {}", hhmmss(p.verdict_ms), verdict(p).trim_end()))
}

/// Whether a test watch on `x`'s edit `p` goes on, at the session as it is now (`now`): its run
/// and its edit, still running. A process restart behind the restart parent (the harness's fresh
/// process for statics an earlier run left, or a warm restart) moves the session's pid: the watch
/// moves with it, from the first of the new process's events, as its verdict comes from there.
/// Watching the old pid, a test call waited 6 min for the verdict its restarted tests gave in 3 s
/// (W12859, 11-18).
fn watch_on(x: &mut Live, now: Option<&Live>, p: &mut LivePatch) -> bool {
    let Some(now) = now.filter(|s| s.state == "running" && s.pid > 0 && s.run_n == x.run_n && s.patch.as_ref().is_some_and(|old| old.edit_ms == p.edit_ms)) else { return false };
    if now.pid != x.pid {
        x.pid = now.pid;
        p.seq = 0;
        p.cursor.clear();
    }
    true
}

/// The first of a run's queued `events` past `since` of a kind in `on` (a watch's `on=` list), as
/// a watch answers it ([`test_event`]).
fn queued_event<'a>(events: &'a [Value], on: &str, since: u64) -> Option<&'a Value> {
    let kinds: Vec<&str> = on.split(',').collect();
    events.iter()
        .filter(|v| v.get("seq").and_then(Value::as_u64).is_some_and(|seq| seq > since))
        .filter(|v| v.get("event").and_then(Value::as_str).is_some_and(|k| kinds.contains(&k)))
        .min_by_key(|v| v.get("seq").and_then(Value::as_u64))
}

/// One event, rearming across an exec's closed link and reset event sequence, and following the
/// session's process across a restart ([`watch_on`]).
fn test_event(ctx: &Ctx, id: &str, x: &mut Live, p: &mut LivePatch, on: &str) -> Option<Value> {
    let mut left = false;
    while let Some(r) = running(id) {
        if watch_superseded(&r) || !watch_on(x, get(ctx, id).as_ref(), p) { return None; }
        if !left && makepad_remote::lines::client_gone() {
            left = true;
            r.release_active_wait();
            starter_left(ctx, id);
        }
        let answer = inspect(&x.node, x.pid, &format!("watch on={on} since={} timeout_s=1", p.seq));
        if watch_superseded(&r) { return None; }
        match answer {
            Ok(lines) => {
                let mut refused = false;
                for v in lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
                    refused |= v.get("err").is_some();
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
                    if watch_superseded(&r) { return None; }
                    let events: Vec<Value> = lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).filter(|v| v.get("seq").is_some()).collect();
                    let reset = events.iter().filter_map(|v| v.get("seq").and_then(Value::as_u64)).max().is_some_and(|seq| seq < p.seq)
                        || events.iter().any(|v| v.get("seq").and_then(Value::as_u64) == Some(p.seq) && !p.cursor.is_empty() && event_cursor(v) != p.cursor);
                    if reset { p.seq = 0; p.cursor.clear(); }
                    // A run held before its watcher exists (its resolve failed: a crate
                    // substitution, a manifest) answers every watch "not a live session", and its
                    // refusal is in its event queue: read there. Asked again and again, two test
                    // calls waited 102 min at "restarting JIT" for the verdict their restarted run
                    // had given in its first second (W25685, W25686, 11-27).
                    if let Some(v) = refused.then(|| queued_event(&events, on, p.seq)).flatten() {
                        p.seq = v.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
                        p.cursor = event_cursor(v);
                        return Some(v.clone());
                    }
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

/// Test verdict `p.tests` is on a round that took none of `taken` (the shipped files the session's
/// JIT has as sources): the call's answer is a JIT error on them, its tests dropped. Returns them.
fn jit_miss(p: &mut LivePatch, taken: &[String]) -> Option<Vec<String>> {
    let t = p.tests.as_ref()?;
    let missed = untaken(t, taken)?;
    p.reason = format!("the JIT did not take {}: its tests ran for {}", missed.join(", "), t.edit);
    p.tests = None;
    p.outcome = "jit_error".into();
    p.verdict_ms = wall_ms();
    Some(missed)
}

/// The files of `taken` (shipped, sources of the session's JIT) that test verdict `t` was not about:
/// all of them when the round it ran for named none (its `edit`: the paths the JIT took), else
/// None. A restarted run loaded every source, and the first run follows no edit: both answer.
fn untaken(t: &crate::state::LiveTests, taken: &[String]) -> Option<Vec<String>> {
    if t.restarted || taken.is_empty() {
        return None;
    }
    let edit = makepad_strict_json::parse(t.edit.as_bytes()).ok()?;
    let paths: Vec<&str> = edit.as_arr()?.iter().filter_map(Value::as_str).collect();
    // (the box names a file by its own root: a tree path is its tail)
    let named = |f: &String| paths.iter().any(|p| *p == f.as_str() || p.strip_suffix(f.as_str()).is_some_and(|head| head.ends_with('/')));
    (!taken.iter().any(named)).then(|| taken.to_vec())
}

/// A round's relax warnings (`<file>:<line>:<col>: warning: ..`), once each, the file as the tree
/// names it (the box's path read as its longest tail that is a file of the tree), and whether the
/// lane changed that file ([`lane_changed`]). The tests' verdict said nothing of them: a lane's
/// unused const reached the fast bucket as a warning of unknown origin (11-16).
pub(crate) fn round_warnings(log: &str, tree: &Path) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for line in log.lines().map(str::trim_end) {
        let Some((path, rest)) = crate::bucket::location(line) else { continue };
        if !rest.starts_with("warning: ") { continue; }
        let rel = tree_file(tree, path);
        let said = rel.as_ref().map_or_else(|| line.to_string(), |rel| format!("{rel}{}", &line[path.len()..]));
        if out.iter().any(|(l, _)| *l == said) { continue; }
        let mine = rel.is_some_and(|rel| lane_changed(tree, &rel));
        out.push((said, mine));
    }
    out
}

/// `path`, as a box or a tool names a file, as the tree names it: its longest tail that is a file
/// of `tree`.
fn tree_file(tree: &Path, path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    (0..parts.len()).map(|i| parts[i..].join("/")).find(|rel| tree.join(rel).is_file())
}

/// Whether the lane changed file `rel` of its tree: it differs from work's (the nested commercial
/// checkout's from its main), or work has none. The merge bot keeps a lane merged with the tip,
/// so a file that differs is the lane's. Unreadable: not said to be the lane's.
fn lane_changed(tree: &Path, rel: &str) -> bool {
    let (dir, base, at) = match rel.strip_prefix(&crate::nested::prefix(crate::state::PAIRED)) {
        Some(at) => (tree.join(crate::nested::dir(crate::state::PAIRED)), "main", at),
        None => (tree.to_path_buf(), "work", rel),
    };
    let Ok(disk) = std::fs::read(tree.join(rel)) else { return false };
    match crate::gitread::entries_at(&dir, base, &[at]).and_then(|mut oids| oids.pop()) {
        Some(oid) if oid.is_empty() => true,
        Some(oid) => crate::gitread::blob(&dir, &oid).is_ok_and(|b| b != disk),
        None => false,
    }
}

/// A verified patch's outcome: "patched", or "restart" when the JIT kept items of the save at their
/// running code until a restart (its `held` lines: a struct the app holds changed its layout, and
/// the fns that need its new form), with what waits as the reason. A save that read "patched"
/// while the app ran the old code of the fns it edited sent a lane chasing a probe that never
/// printed (11-17).
fn verified_outcome(v: Option<&Value>) -> (&'static str, String) {
    let held: Vec<String> = v.and_then(|v| v.get("held")).and_then(Value::as_arr).unwrap_or(&[]).iter().map(|r| r.as_str().map(str::to_string).unwrap_or_else(|| r.to_json())).collect();
    if held.is_empty() {
        return ("patched", String::new());
    }
    ("restart", format!("restart needed: patched in part, and what is held runs its old code until a restart:\n{}", held.join("\n")))
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

fn patch_verdict(ctx: &Ctx, id: &str, x: &mut Live, p: &mut LivePatch) {
    // the JIT's verdict on the edit
    let asked = Instant::now();
    let r = running(id);
    // (of the session as the call began: the watch below may move `x` to a restarted process)
    let began = x.clone();
    let newer = newer_saves(ctx, &began, r.as_deref());
    let mut ran_out = None;
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
                // A watch with no verdict asks the JIT where the edit is: still on it, the watch
                // goes on, however long; installed, or nothing to install, it is said so. A fixed
                // 60 s said "edit not applied" of a two-file Fab edit the JIT patched at 70 s,
                // and of one it patched at 7 s whose check had not reported (11-16).
                Ok(Some(lines)) if !lines.iter().any(|l| verdict_line(l)) => {
                    match edit_state(&inspect(&x.node, x.pid, "live"), p.patches_at_ship) {
                        Some(EditState::On(why)) => {
                            event(ctx, Sev::Info, format!("live {id}: no verdict yet after {} s: the JIT is still on the edit ({why}); the watch goes on", asked.elapsed().as_secs()), Some(x));
                            continue;
                        }
                        state => { ran_out = state; break Ok(lines) }
                    }
                }
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
    if p.outcome == "jit_fault" || r.as_deref().is_some_and(watch_superseded) { return; }
    let installed = match &ran_out { Some(EditState::Installed(n, why)) => Some((*n, why.clone())), _ => None };
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
            "patchverified" => {
                let (outcome, reason) = verified_outcome(v.as_ref());
                p.outcome = outcome.into();
                if !reason.is_empty() {
                    p.reason = reason;
                }
            }
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
            _ if installed.is_some() => {
                let (unverified, why) = installed.unwrap_or_default();
                p.outcome = "patched".into();
                p.reason = format!("installed: the JIT's patch count rose after the ship; its check has not reported yet ({unverified} patch group(s) run unverified){}", if why.is_empty() { String::new() } else { format!(": {why}") });
            }
            _ if matches!(ran_out, Some(EditState::Unchanged)) => {
                p.outcome = "unchanged".into();
                p.reason = "the edit changed nothing the JIT compiles: its watcher is done with it and installed no patch (a comment, or code no build of the app reaches)".into();
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
        // (a newer save ends the wait, and so does a relaunch asked meanwhile: the JIT's verdict on
        // this edit stands. A relaunch after a refused save waited the whole WATCH_S, 11-21)
        let relaunch_waits = || r.as_deref().is_some_and(|r| r.relaunch_waits.load(std::sync::atomic::Ordering::SeqCst));
        let recovery = inspect_until(&x.node, x.pid, &format!("watch on=patchverified,patchrestarted,jiterror since={} timeout_s={WATCH_S}", p.seq), &|| newer() || relaunch_waits());
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

/// A JIT run the session's app started on its box: a WM's hosted app (the session's lease, another
/// pid), as it stood before a ship: whether the edit's files are among its sources, its event
/// cursor and its installed patches, for its verdict on an edit the session's app does not read
/// ([`hosted_verdict`]). Since its launch keeps the WM's edits (app_registry launch.rs), a hosted
/// app patches the lane's saves in; the session's own app is not one of them.
#[derive(Debug, Default)]
struct Hosted {
    pid: u32,
    app: String,
    reads: bool,
    seq: u64,
    patches: Option<u64>,
}

/// The runs of the session's lease on its box other than its app ([`Hosted`]), each asked about
/// the edit's `files` (`inputs_of`); none when its hub's `ps` does not answer.
fn hosted_runs(r: &Running, x: &Live, inputs_of: &str, files: &[String]) -> Vec<Hosted> {
    let Ok(lines) = ask(r.held.client(), Some(&r.node), "ps", "ps") else { return Vec::new() };
    let mine = |pid: u32| pid == x.pid || (x.run_pid > 0 && pid == x.run_pid);
    // (a run's lease in `ps` is its mark, the hub's ledger dir and the lease id: [`lease_of_mark`])
    crate::cli::whose_of_ps(&lines).into_iter().filter(|w| crate::cli::lease_of_mark(&w.lease) == r.lease && !mine(w.pid)).filter_map(|w| {
        let inputs = inspect(&r.node, w.pid, inputs_of).ok()?;
        let reads = inputs.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
            .any(|v| v.get("path").and_then(Value::as_str).is_some_and(|path| files.iter().any(|f| f == path)));
        if !reads { return Some(Hosted { pid: w.pid, app: w.app, ..Default::default() }); }
        let seq = inspect(&r.node, w.pid, "events").ok()?.iter().filter_map(|l| num(l, "seq")).max().unwrap_or(0);
        let patches = inspect(&r.node, w.pid, "live").ok().and_then(|l| l.iter().find_map(|l| num(l, "patches")));
        Some(Hosted { pid: w.pid, app: w.app, reads, seq, patches })
    }).collect()
}

/// What hosted run `h` ([`Hosted`]) did with the edit, as the session's own verdict is read
/// ([`patch_verdict`]), without the session's restarts and frames (the app is its host's): its
/// outcome and what it said.
fn hosted_verdict(node: &str, h: &Hosted, newer: &dyn Fn() -> bool) -> (String, String) {
    let watch = format!("watch on=patchverified,patchrefused,patchreverted,patchfailed,patchrestarted,jiterror since={} timeout_s={WATCH_S}", h.seq);
    loop {
        let lines = match inspect_until(node, h.pid, &watch, newer) {
            Ok(Some(lines)) => lines,
            Ok(None) => return ("superseded".into(), "a newer save shipped before its verdict came".into()),
            Err(e) => return ("none".into(), e),
        };
        let Some(v) = lines.iter().filter(|l| verdict_line(l)).find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) else {
            return match edit_state(&inspect(node, h.pid, "live"), h.patches) {
                Some(EditState::On(_)) => continue,
                Some(EditState::Installed(..)) => ("patched".into(), "installed: its patch count rose after the ship; its check has not reported yet".into()),
                Some(EditState::Unchanged) => ("unchanged".into(), "it changed nothing that run compiles".into()),
                _ => ("none".into(), format!("no verdict from it in {WATCH_S} s")),
            };
        };
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let reasons = || v.get("reasons").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|r| r.as_str().map(str::to_string).unwrap_or_else(|| r.to_json())).collect::<Vec<_>>().join("; ");
        return match s("event").as_str() {
            "patchverified" => { let (o, why) = verified_outcome(Some(&v)); (o.into(), why) }
            "patchrefused" => ((if s("outcome") == "restart" { "restart" } else { "refused" }).into(), reasons()),
            "patchreverted" => ("reverted".into(), s("error")),
            "patchrestarted" => ("restart".into(), "it restarted on it".into()),
            other => ("failed".into(), format!("{other}: {}", if s("error").is_empty() { s("message") } else { s("error") })),
        };
    }
}

/// Tests finish independently of the caller's wait timeout.
fn test_verdict(ctx: &Ctx, id: &str, x: &mut Live, p: &mut LivePatch) {
    collect_test_verdict(ctx, id, x, p, |x, p| test_event(ctx, id, x, p, "testverdict,patchverified,patchrefused,patchreverted,patchfailed,patchrestarted,jiterror"));
}

/// The edit of the refusal a test run held at its start says ([`collect_test_verdict`]): relax's
/// `harness_live::hold`, which waits for the next save.
const HELD_AT_START: &str = "the tests did not compile";

fn collect_test_verdict(ctx: &Ctx, id: &str, x: &mut Live, p: &mut LivePatch, mut next: impl FnMut(&mut Live, &mut LivePatch) -> Option<Value>) {
    // No verdict can come from this binary for the edit. Let finish_tests start the new JIT.
    if rebuild_tests(x, p) { return; }
    while let Some(v) = next(x, p) {
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
            // A run held at its start (the tests did not compile: relax's harness_live::hold) waits
            // for the next save, and no hot recovery comes: a JIT fault among its errors (an
            // internal compiler error) is its verdict, a JIT error. Taken as one to recover from,
            // the call waited on until the box's idle end ended its lease as hung and the client
            // said "no lease": nine sessions of four lanes, 10 min each, on a relax ICE (11-27).
            let held = p.outcome == "initial" || v.get("edit").and_then(Value::as_str) == Some(HELD_AT_START);
            p.outcome = if jit_fault(&p.reason) { if held { "jit_error" } else { "hot-recovery" } } else if v.get("outcome").and_then(Value::as_str) == Some("restart") { "restart" } else { "refused" }.into();
            if let Some(r) = running(id) { *r.fault.lock().unwrap() = None; }
            if p.outcome == "hot-recovery" { continue; }
            if p.outcome == "jit_error" {
                p.tests = None;
                p.verdict_ms = wall_ms();
                p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, format!("live {id}: the tests did not compile under the JIT: {}", p.reason.lines().find(|l| jit_fault(l)).unwrap_or("")), None);
            }
            return;
        }
        // a run on the code from before patches the JIT undid is no verdict on the saved source
        if v.get("reverted").is_some() { continue; }
        // nor is one whose patched code faulted, whatever its tests say (a thread no test waits for)
        if let Some(groups) = v.get("faulted") {
            p.reason = format!("the patched code faulted (patch groups {}): the JIT undid it; no verdict on the saved source", groups.to_json());
            p.outcome = "jit_fault".into();
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
fn resume_tests(ctx: &Ctx, id: &str, r: &Running, guard: Option<ResumeGuard<'_>>) {
    let Some(mut x) = get(ctx, id) else { return };
    if watch_superseded(r) || !x.held.is_empty() {
        // A confirmed refusal is complete. Reattaching restores the idle charge without
        // reinterpreting the failed run's log or requesting a second verdict.
        drop(guard);
        if r.resuming.swap(false, std::sync::atomic::Ordering::SeqCst) { finish_call(ctx, id, r); }
        return;
    }
    if let Err(e) = x.load_text() { eprintln!("live log: {e}"); return; }
    let Some(mut p) = x.patch.clone() else { return };
    if p.outcome == "recovering" { return; }
    let settled = r.recover_cut.swap(false, SeqCst) && recover_cut_verdict(ctx, id, r, &mut x, &mut p);
    if !settled && !watch_superseded(r) && !terminal(&p) && p.outcome.is_empty() { patch_verdict(ctx, id, &mut x, &mut p); }
    if !settled && !watch_superseded(r) && !terminal(&p) && matches!(p.outcome.as_str(), "initial" | "patched" | "restart" | "hot-recovery") {
        test_verdict(ctx, id, &mut x, &mut p);
    }
    if settled || watch_superseded(r) {
        drop(guard);
        if r.resuming.swap(false, SeqCst) { finish_call(ctx, id, r); }
        return;
    }
    if get(ctx, id).is_none_or(|s| s.run_n != x.run_n || s.pid != x.pid || s.state != "running") { return; }
    finish_tests(ctx, id, &x, &mut p);
    drop(guard);
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

/// The files a session shipped (`shipped`) that its tree's dirty set (`changed`, `deleted`) no
/// longer names: back to the pin's bytes (a revert, a checkout), so no longer dirty, while the box
/// runs the bytes shipped before. Each ships again as the tree has it, or as gone when the tree no
/// longer has it (a new file removed again). Such a revert went nowhere, and a lane added a blank
/// line to get its revert shipped (11-20).
fn back_to_pin(tree: &Path, shipped: &BTreeMap<String, String>, changed: &mut Vec<String>, deleted: &mut Vec<String>) {
    let clean: Vec<String> = shipped.keys().filter(|p| !changed.contains(*p) && !deleted.contains(*p)).cloned().collect();
    for path in clean {
        match std::fs::symlink_metadata(tree.join(&path)) {
            Ok(m) if !m.is_dir() => changed.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => deleted.push(path),
            _ => {}
        }
    }
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

/// Of a test session's edited `files`, those no patch takes that its tests still see: a file in a
/// package that is none of its `inputs` (a data file beside the sources), or one in no package
/// (`outside`) that the run opened, probed or listed the folder of (`read`: a fixture its tests load
/// from the workspace). Their tests start again on them. A file outside every package the run never
/// read keeps the last verdict (a JIT that names no reads keeps it for every such file, as before).
fn rerun_on(files: &[String], inputs: &BTreeSet<String>, outside: &BTreeSet<String>, read: &BTreeSet<String>) -> Vec<String> {
    files.iter().filter(|f| !inputs.contains(*f) && (!outside.contains(*f) || read.contains(*f))).cloned().collect()
}

/// Whether the session's JIT took a change after verdict `old`, by its `events`: a patch event
/// after that verdict's cursor, or a queue that started over since (an exec's restart). The
/// verdict is then no longer what the tests run.
fn taken_since(events: &[String], old: &LivePatch) -> bool {
    let events: Vec<Value> = events.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).filter(|v| v.get("seq").is_some()).collect();
    let seq = |v: &Value| v.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let over = events.iter().map(seq).max().is_some_and(|m| m < old.seq)
        || events.iter().any(|v| seq(v) == old.seq && !old.cursor.is_empty() && event_cursor(v) != old.cursor);
    over || events.iter().any(|v| seq(v) > old.seq && matches!(v.get("event").and_then(Value::as_str), Some("patchverified" | "patchrefused" | "patchreverted" | "patchfailed" | "patchrestarted" | "jiterror")))
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

/// Capture the overlay once so the event cursor can be sampled before publishing it.
fn capture_overlay(ctx: &Ctx, id: &str, r: &Running, x: &Live, files: &[String], current: Option<(Vec<String>, Vec<String>)>, steps: &mut Steps) -> Result<BTreeMap<String, String>, String> {
    let pin = crate::lease::Pin::of_rev(&x.rev).ok_or("live: missing pin")?;
    let (mut changed, mut deleted) = match current {
        Some(paths) => paths,
        None => crate::lease::ship::uncommitted_seen(&ctx.monitor, &r.tree, &pin)?,
    };
    back_to_pin(&r.tree, &x.sources, &mut changed, &mut deleted);
    changed.extend(also_files(&r.tree, &x.also)?);
    changed.sort();
    changed.dedup();
    deleted.sort();
    steps.lap("dirty");
    let selected: BTreeSet<_> = files.iter().collect();
    if x.test {
        changed.retain(|p| selected.contains(p));
        deleted.retain(|p| selected.contains(p));
    }
    let dir = ctx.logs("live").join(format!("{id}.ship"));
    let mut sources = capture(&r.tree, &changed, &deleted, &x.sources, &dir)?;
    if x.test { sources.extend(x.sources.iter().filter(|(p, _)| !selected.contains(p)).map(|(p, h)| (p.clone(), h.clone()))); }
    steps.lap("capture");
    Ok(sources)
}

fn overlay_changes(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<String> {
    old.keys().chain(new.keys()).filter(|k| old.get(*k) != new.get(*k)).cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

fn publish_overlay(ctx: &Ctx, id: &str, r: &Running, x: &Live, files: &[String], current: Option<(Vec<String>, Vec<String>)>) -> Result<Option<(Vec<String>, BTreeMap<String, String>)>, String> {
    let mut steps = Steps::default();
    let sources = capture_overlay(ctx, id, r, x, files, current, &mut steps)?;
    if sources == x.sources { return Ok(None); }
    let changes = overlay_changes(&x.sources, &sources);
    send_overlay(ctx, id, r, x, &sources, &mut steps)?;
    Ok(Some((changes, sources)))
}

fn send_overlay(ctx: &Ctx, id: &str, r: &Running, x: &Live, sources: &BTreeMap<String, String>, steps: &mut Steps) -> Result<(), String> {
    let dir = ctx.logs("live").join(format!("{id}.ship"));
    let pin = crate::lease::Pin::of_rev(&x.rev).ok_or("live: missing pin")?;
    let mut entries = Vec::new();
    for (path, hash) in sources {
        if hash == "-" || x.sources.get(path) == Some(hash) { continue; }
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
    steps.lap("done");
    try_set(ctx, id, |s| s.sources.clone_from(sources))?.ok_or("live job disappeared")?;
    Ok(())
}

/// `called_ms`: when the call came (a test session's `live sync`, before it waits for its box's
/// room or an earlier call): what follows it is devhub's, what comes before it the caller's.
/// `queued`: how long a test call waited for the previous call's tests, and behind what.
fn ship_edit(ctx: &Ctx, id: &str, r: &Running, files: Vec<String>, edit_ms: u64, called_ms: u64, current: Option<(Vec<String>, Vec<String>)>, queued: Option<(u64, String)>) -> Result<Option<String>, String> {
    let mut room = Ok(None);
    let Some(mut x) = get(ctx, id).filter(|x| x.active() && x.pid > 0) else { return room };
    let mut needs_room = x.test && !x.held.is_empty();
    let mut note = None;
    // the saved files the app read at run time that are none of its sources ([`told_verdict`])
    let mut resources: Vec<String> = Vec::new();
    let mut hosted: Vec<Hosted> = Vec::new();
    let mut unread = Vec::new();
    // the shipped files the JIT has as sources: its test verdict must be about one of them
    let mut taken: Vec<String> = Vec::new();
    // the app has no run and waits for a fix ([`held_app`]): the files are the next run's
    let mut held = false;
    let (queued_ms, behind) = queued.unwrap_or_default();
    let mut p = LivePatch { edit_ms: edit_ms.max(x.patch.as_ref().map_or(0, |p| p.edit_ms + 1)), called_ms, queued_ms, behind, ..Default::default() };
    let mut steps = Steps::default();
    let mut ship = || -> Result<bool, String> {
        steps = Steps { at: called_ms + queued_ms, said: if queued_ms > 0 { format!("queued {queued_ms}") } else { String::new() } };
        steps.lap("admit");
        let sources = capture_overlay(ctx, id, r, &x, &files, current.clone(), &mut steps)?;
        let unchanged = sources == x.sources;
        if unchanged {
            let Some(old) = x.patch.as_ref().filter(|old| x.test && terminal(old)) else { return Ok(false) };
            if !taken_since(&inspect(&r.node, x.pid, "events")?, old) { return Ok(false); }
        }
        // A confirmed refusal with the same manifest and event cursor has already answered.
        // Take back its full lease charge only when there really is a round to run.
        if std::mem::take(&mut needs_room) {
            room = admit(ctx, id, r, makepad_remote::lines::asked_on());
            if room.is_err() { return Ok(false); }
        }
        if unchanged {
            let old = x.patch.as_ref().unwrap();
            p.files = old.files.clone();
            (p.seq, p.cursor, p.log_start) = (old.seq, old.cursor.clone(), old.log_end);
            held = !x.held.is_empty();
            set(ctx, id, |s| { s.patch = Some(p.clone()); s.touched_ms = wall_ms(); });
            return Ok(true);
        }
        if !x.test && r.up_pid.load(std::sync::atomic::Ordering::Relaxed) != x.pid {
            if !wait_up(ctx, id, r, x.pid) { return Ok(false); }
            steps.lap("up");
        }
        p.files = overlay_changes(&x.sources, &sources);
        // the event cursor and the inputs at once, one round trip: both are asked before the
        // ship, so no answer can hold anything the JIT did with this edit. The inputs are asked
        // about the edit's files only (`of`): the whole list cost the JIT 25-40 ms.
        let inputs_of = format!("{{\"cmd\":\"inputs\",\"of\":{}}}", json_str(&p.files.join("\n")));
        // (and the patches it installed so far: one more after the ship is this edit's)
        // (and an app's: the runs it started on its box, as they stood before the ship, [`Hosted`])
        let (events, inputs, live, runs) = std::thread::scope(|sc| {
            let inputs = sc.spawn(|| inspect(&r.node, x.pid, &inputs_of));
            let live = sc.spawn(|| inspect(&r.node, x.pid, "live"));
            let runs = (!x.test).then(|| sc.spawn(|| hosted_runs(r, &x, &inputs_of, &p.files)));
            let events = inspect(&r.node, x.pid, "events");
            let panicked = |_| Err("live: an inspect call panicked".to_string());
            (events, inputs.join().unwrap_or_else(panicked), live.join().unwrap_or_else(panicked), runs.map(|h| h.join().unwrap_or_default()).unwrap_or_default())
        });
        hosted = runs;
        held = live.as_ref().is_ok_and(|l| held_app(l, &[]));
        p.patches_at_ship = live.ok().and_then(|l| l.iter().find_map(|l| num(l, "patches")));
        let cursor = events?.into_iter().filter(|l| num(l, "seq").is_some()).max_by_key(|l| num(l, "seq").unwrap_or(0)).unwrap_or_default();
        p.seq = num(&cursor, "seq").unwrap_or(0);
        p.cursor = makepad_strict_json::parse(cursor.as_bytes()).ok().map(|v| event_cursor(&v)).unwrap_or_default();
        steps.lap("cursor");
        p.log_start = if x.test { run_log(r, x.run_n, x.patch.as_ref().map_or(0, |p| p.log_end))?.0 } else { 0 };
        set(ctx, id, |s| { s.patch = Some(p.clone()); s.touched_ms = wall_ms(); });
        send_overlay(ctx, id, r, &x, &sources, &mut steps)?;
        p.shipped_ms = wall_ms();
        // One query for the exact shipped set. Unknown or incomplete inputs cannot
        // justify waiting for a patch event (nor claiming this edit missed the target).
        if held { return Ok(true); }
        let lines = inputs?;
        // (its inputs answer says so too, when its wait began after the look at `live`)
        held |= held_app(&[], &lines);
        if held { return Ok(true); }
        let mut inputs = BTreeSet::new();
        let mut outside = BTreeSet::new();
        let mut read = BTreeSet::new();
        let mut count = None;
        for line in &lines {
            let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
            if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(format!("live: inputs: {e}")); }
            if let Some(n) = v.get("count").and_then(Value::as_u64) { count = Some(n); }
            if let Some(path) = v.get("path").and_then(Value::as_str) { inputs.insert(path.to_string()); }
            if let Some(path) = v.get("outside").and_then(Value::as_str) { outside.insert(path.to_string()); }
            if let Some(path) = v.get("read").and_then(Value::as_str) { read.insert(path.to_string()); }
        }
        if count != Some(inputs.len() as u64) { return Err("live: inputs did not return the complete source list".into()); }
        if x.test {
            // (what the run names in DEVHUB_ALSO is its own, wherever it lies)
            outside.retain(|f| !x.also.iter().any(|a| Path::new(f).starts_with(a)));
            unread = rerun_on(&p.files, &inputs, &outside, &read);
            taken = p.files.iter().filter(|f| inputs.contains(*f)).cloned().collect();
            if unread.is_empty() && !p.files.iter().any(|f| inputs.contains(f)) {
                if x.patch.as_ref().is_some_and(terminal) {
                    note = Some(format!("{} is in no package of the tests' graph and they did not read it: the last verdict stands", p.files[0]));
                } else {
                    unread = p.files.clone();
                }
            }
        } else if !p.files.iter().any(|p| inputs.contains(p)) {
            // (unless an app it started reads it: [`hosted_verdict`] answers)
            hosted.retain(|h| h.reads);
            if hosted.is_empty() {
                // a file the app read at run time (a kit, a level): its JIT tells the app
                resources = p.files.iter().filter(|f| read.contains(*f)).cloned().collect();
                if resources.is_empty() {
                    note = Some(format!("edit does not reach {}: {} is not one of its sources, nor of an app it started, nor a file it read", x.app, p.files[0]));
                }
            }
        } else {
            hosted.clear();
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
    if watch_superseded(r) { return room; }
    if p.shipped_ms == 0 { p.shipped_ms = wall_ms(); }
    if matches!(shipped, Ok(false)) { return room; }
    if p.files.is_empty() { p.files = files; }
    let count = p.files.len();
    if let Err(e) = shipped {
        // released meanwhile: its release said so, the ship it cut short says nothing
        if !get(ctx, id).is_some_and(|s| s.active()) { return room; }
        // no edit can reach it again: it ends, said once (before, each save failed, 11-04)
        if let Some(why) = session_over(&e, &r.lease) {
            event(ctx, Sev::Warn, format!("live {id} ({}): ended, {why}; `makepad devhub live start {}` starts a new session", x.app, x.app), Some(&x));
            let _ = release(ctx, id, &format!("ended: {why}"));
            return room;
        }
        p.outcome = "failed".into();
        p.outcome_ms = wall_ms();
        p.reason = e.clone();
        let x = set(ctx, id, |s| s.patch = Some(p.clone()));
        event(ctx, Sev::Warn, format!("live {id}: shipping {count} file(s) failed: {e}"), x.as_ref());
        if let Some(x) = &x { unapplied(ctx, x, &p); }
        return room;
    }
    if held {
        if x.test {
            // The harness restarts itself on publication. Follow its pre-ship cursor even if
            // the new PID was already announced before ship-done answered.
            p.outcome = "restart".into();
            set_patch(ctx, id, &p);
            loop {
                test_verdict(ctx, id, &mut x, &mut p);
                if watch_superseded(r) { return room; }
                finish_tests(ctx, id, &x, &mut p);
                let now = get(ctx, id);
                if now.as_ref().is_none_or(|s| s.pid == x.pid || s.patch.as_ref().is_some_and(terminal)) || !watch_on(&mut x, now.as_ref(), &mut p) { break; }
                p.tests = None;
                p.outcome = "restart".into();
                p.reason.clear();
                p.verdict_ms = 0;
            }
            wait_tests(ctx, id);
        } else {
            held_relaunch(ctx, id, &x, r, p, count);
        }
        return room;
    }
    // An edit the session's app does not read and an app it started does (a WM's hosted app):
    // that app's verdict, named. Read literally, "edit does not reach makepad-app-wm" sent a lane
    // after an edit its hosted Calculator had patched in (11-20).
    if !hosted.is_empty() {
        let newer = newer_saves(ctx, &x, Some(r));
        let said: Vec<(String, String)> = hosted.iter().map(|h| {
            let (outcome, why) = hosted_verdict(&r.node, h, &newer);
            let line = format!("{outcome} in {} (pid {}), which {} started{}", h.app, h.pid, x.app, if why.is_empty() { String::new() } else { format!(": {why}") });
            (outcome, line)
        }).collect();
        // the worst of them is the edit's (one app, as a rule)
        let rank = |o: &str| match o { "patched" | "unchanged" => 0, "superseded" => 1, "restart" => 2, _ => 3 };
        p.outcome = said.iter().map(|(o, _)| o.clone()).max_by_key(|o| rank(o)).unwrap_or_default();
        p.reason = format!("{}; {} itself does not read {}", said.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join("; "), x.app, p.files[0]);
        p.outcome_ms = wall_ms();
        let Some(current) = set_patch(ctx, id, &p) else { return room };
        if matches!(p.outcome.as_str(), "none" | "failed") { unapplied(ctx, &current, &p); }
        let sev = if matches!(p.outcome.as_str(), "patched" | "unchanged" | "superseded") { Sev::Info } else { Sev::Warn };
        event(ctx, sev, format!("live {id}: {count} file(s) {}: {}", p.outcome, p.reason), Some(&current));
        return room;
    }
    if !resources.is_empty() {
        told_verdict(ctx, id, &x, r, &mut p, &resources, count);
        return room;
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
            return room;
        }
        p.outcome = "skipped".into();
        p.reason = note.clone();
        p.outcome_ms = wall_ms();
        let current = set_patch(ctx, id, &p);
        event(ctx, Sev::Info, format!("live {id}: {note}"), current.as_ref());
        return room;
    }
    // It is none of the running program's sources, so no patch and no selection can say what it
    // does to the tests, yet they read it or can: a process they start, a data file. The last verdict is
    // about the bytes before it (a broken spritelab main.rs still said 1/1 passed, 11-04).
    if let Some(file) = unread.first() {
        p.reason = format!("{file} is not a source of the running tests: they start again on it");
        restart_session(ctx, id, &x, r, &mut p, true);
        wait_tests(ctx, id);
        return room;
    }
    set(ctx, id, |s| {
        if let Some(current) = &mut s.patch {
            if current.edit_ms == p.edit_ms { current.shipped_ms = p.shipped_ms; }
        }
    });
    if get(ctx, id).is_some_and(|s| s.pid == x.pid && s.run_n == x.run_n && s.patch.as_ref().is_some_and(|p| p.outcome.is_empty())) {
        patch_verdict(ctx, id, &mut x, &mut p);
    } else {
        wait_tests(ctx, id);
        return room;
    }
    if x.test {
        if watch_superseded(r) { return room; }
        if p.tests.is_none() && set_test_patch(ctx, id, &x, &p).is_none() { wait_tests(ctx, id); return room; }
        if p.tests.is_none() && matches!(p.outcome.as_str(), "patched" | "restart" | "hot-recovery") {
            test_verdict(ctx, id, &mut x, &mut p);
        }
        if watch_superseded(r) { return room; }
        // A verdict on a round that took none of them is not about this edit: a JIT miss, never
        // a pass (a ship's folder answered for the file in it, 11-17)
        if let Some(missed) = jit_miss(&mut p, &taken) {
            let detail = format!("{{\"lane\":{},\"files\":[{}],\"log\":{},\"message\":{}}}", json_str(&x.lane), missed.iter().map(|f| json_str(f)).collect::<Vec<_>>().join(","), json_str(&x.log), json_str(&p.reason));
            p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, p.reason.clone(), Some(detail));
        }
        if get(ctx, id).is_some_and(|s| s.run_n == x.run_n && s.pid == x.pid && s.state == "running") { finish_tests(ctx, id, &x, &mut p); }
        wait_tests(ctx, id);
        return room;
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
    let Some(mut x) = set_patch(ctx, id, &p) else { return room };
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
        if matches!(p.outcome.as_str(), "patched" | "recovering" | "jit-error" | "superseded" | "unchanged") { Sev::Info } else { Sev::Warn },
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
    room
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
        // the hot record as it stands: its node, app and pid ([`crate::proto::from_hot`])
        Ok(true) => { ctx.store.publish_record(crate::proto::from_hot(&r)); }
        Ok(false) => {}
        Err(e) => event(ctx, Sev::Warn, format!("live {}: hot ledger: {e}", x.id), Some(x)),
    }
}

/// The output of the live app's run: the log, and the JIT's pid from its announce line.
struct RunOut {
    file: std::fs::File,
    buf: Vec<u8>,
    /// The app pid and the pid that stays across its restarts.
    pids: Option<(u32, u32)>,
    /// the package the job runs: whose announce names the program's pid
    app: String,
    r: Arc<Running>,
    scan: Scan,
    /// the process wrote a line: it started, whatever it said
    wrote: bool,
    /// cargo builds the tree's JIT ([`jit_build_line`]): the run is busy, not silent
    building: bool,
    /// its lease's run started: its JIT was selected, built, or another lease's build of it waited
    /// for ([`startup_due`])
    started: bool,
}

/// Whether a run's silence can be its missing startup line ([`no_startup_said`]): it wrote output
/// and announced no pid, its lease's run started, and no JIT build shows in its output. Until its
/// run starts the session waits for its JIT, however long that takes, and its caller hears what it
/// waits for; only a failed build (with its error) or a cancel ends that wait. A lane's Windows
/// test session was failed at 2 min while its box built the dev JIT for 192 s, whose lines only
/// come with the run, and its tests then passed (W22551, 11-26).
fn startup_due(pids: Option<(u32, u32)>, wrote: bool, started: bool, building: bool) -> bool {
    pids.is_none() && wrote && started && !building
}

/// What a session says while its process wrote output and none of it was a startup line.
const NO_STARTUP_LINE: &str = "no startup line read from the JIT; if the lane's tree is older than the master's, merge the tip (`devhub lanes merge rc-";

/// Why a run that wrote output has no pid: the session's reason, when its process has been quiet
/// past `bound_ms` (the previous test's measured time plus [`crate::state::STARTUP_MARGIN_MS`], the
/// margin the client's own phase notice waits) with no startup line read. A process still writing
/// is compiling or booting: that is not a failure. Nor is a run whose box's cargo still builds its
/// JIT (`building`): its bound starts once that build ends. A lane's live test waited out its
/// 2m45s JIT build silent, was failed at 2 min, and its JIT then started and passed 36 of 36
/// (W21875, 11-26).
fn no_startup_line(pid: u32, wrote: bool, building: bool, quiet_ms: Option<u64>, bound_ms: u64, lane: &str) -> Option<String> {
    (pid == 0 && wrote && !building && quiet_ms.is_some_and(|q| q >= bound_ms)).then(|| format!("{NO_STARTUP_LINE}{lane}`)"))
}

/// A run's output line as it says cargo starts and ends building its tree's JIT
/// ([`crate::lease::JIT_BUILD_PENDING`], [`crate::lease::JIT_BUILT`]).
fn jit_build_line(line: &str, building: &mut bool) {
    if line.contains(crate::lease::JIT_BUILD_PENDING) { *building = true; }
    else if line.contains(crate::lease::JIT_BUILT) { *building = false; }
}

/// A pending test verdict failed for a run's missing startup line ([`no_startup_said`]), keeping
/// the outcome it replaces for a startup line read late ([`startup_read`]).
fn no_startup_failed(p: &mut LivePatch, why: String) {
    p.startup_outcome = Some(std::mem::take(&mut p.outcome));
    (p.tests, p.outcome, p.reason, p.verdict_ms) = (None, "failed".into(), why, wall_ms());
}

/// The session's run announced its pid. A failure its missing startup line said goes: the note,
/// and the verdict it failed, which is pending again, so its run's own verdict answers the call
/// and a retry waits for it (W21875's retry was answered "no startup line" by the stored failure).
/// Whether it withdrew such a failure.
fn startup_read(s: &mut Live) -> bool {
    if s.note.starts_with(NO_STARTUP_LINE) { s.note.clear(); }
    let Some(p) = s.patch.as_mut().filter(|p| p.startup_outcome.is_some()) else { return false };
    p.outcome = p.startup_outcome.take().unwrap_or_default();
    (p.tests, p.verdict_ms, p.reason_ref) = (None, 0, Default::default());
    p.reason.clear();
    true
}

/// A run that wrote output and read no startup line, quiet past its bound, is said once: in its
/// session's note (so every call and the client's phase say it), in the inbox, and, for a test
/// session, as the failed answer of its call, which otherwise waits on a pid that never comes.
fn no_startup_said(ctx: &Ctx, id: &str) {
    let Some(x) = get(ctx, id).filter(|x| x.state == "running" && !x.note.starts_with(NO_STARTUP_LINE)) else { return };
    let bound = x.patch.as_ref().and_then(|p| p.tests.as_ref()).map_or(0, |t| t.ms.max(0.0) as u64).saturating_add(crate::state::STARTUP_MARGIN_MS);
    let Some(why) = no_startup_line(x.pid, true, false, quiet_ms(&x, wall_ms()), bound, &x.lane) else { return };
    let x = set(ctx, id, |s| s.note = why.clone());
    event(ctx, Sev::Warn, format!("live {id}: {why}"), x.as_ref());
    if let Some(x) = x.filter(|x| x.test) {
        let mut p = x.patch.clone().unwrap_or_default();
        if p.outcome.is_empty() || matches!(p.outcome.as_str(), "initial" | "patched" | "restart") {
            no_startup_failed(&mut p, why);
            set_patch(ctx, id, &p);
        }
    }
}

/// Each startup of this package replaces both pids. Its parent may be named in a later
/// write, interleaved with output from other processes in the same job.
fn announce(line: &str, app: &str, pids: &mut Option<(u32, u32)>) {
    if let Some((pid, name, _)) = makepad_remote::parse_startup_line(line) {
        if name == app { *pids = Some((pid, pid)); }
        return;
    }
    let Some((app_pid, run_pid)) = pids else { return };
    let Some((tag, fields)) = line.strip_prefix('[').and_then(|s| s.split_once(']')) else { return };
    if !tag.ends_with("-remote") { return; }
    let Some((run, pid)) = fields.strip_prefix(" run=").and_then(|s| s.split_once(" pid=")) else { return };
    if pid.trim_end_matches('\r').parse() == Ok(*app_pid) {
        if let Ok(parent) = run.parse() { *run_pid = parent; }
    }
}

impl std::io::Write for RunOut {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(b)?;
        self.wrote |= !b.is_empty();
        self.buf.extend_from_slice(b);
        let mut consumed = 0;
        while let Some(end) = self.buf[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + end;
            let line = String::from_utf8_lossy(&self.buf[consumed..end]);
            announce(&line, &self.app, &mut self.pids);
            jit_build_line(&line, &mut self.building);
            if fault_line(&self.app, &line, &mut self.scan) { *self.r.fault.lock().unwrap() = Some(line.into_owned()); }
            consumed = end + 1;
        }
        self.buf.drain(..consumed);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// An edit the previous master took and gave no verdict, its ship or its wait for the verdict
/// cut by the master's restart: each look then waited out its bound on a patch nothing would
/// conclude (W7598, W7635, 11-15). Shipped: the app keeps its events, so its verdict is watched
/// again. Not shipped: an app session's ships again, and that ship's verdict covers it; a test
/// session's ends said, as its saves ship on its next call.
enum Cut {
    Watch,
    Ship(LivePatch, Vec<String>),
    Said(LivePatch),
    RecoverTest,
}

#[derive(Debug, PartialEq)]
enum CutProbe {
    Finished { patch: Option<Value>, verdict: Value },
    RunningPatch(Value),
    StillRunning,
    FromLog(crate::state::LiveTests),
    Lost(String),
}

fn log_test_verdict(log: &str, p: &LivePatch, allow_group_zero: bool) -> Option<crate::state::LiveTests> {
    let mut end = log.len();
    for raw in log.split_inclusive('\n').rev() {
        let start = end - raw.len();
        end = start;
        let Some(rest) = raw.trim_end().strip_prefix("testverdict ") else { continue };
        let fields: BTreeMap<_, _> = verdict_fields(rest).collect();
        if fields.contains_key("faulted") || fields.contains_key("reverted") { continue; }
        let number = |k| fields.get(k).and_then(|s| s.parse::<u64>().ok());
        let (Some(group), Some(ran), Some(passed), Some(failed), Some(ms)) = (number("group"), number("ran"), number("passed"), number("failed"), fields.get("ms").and_then(|s| s.parse().ok())) else { continue };
        let restarted = fields.get("restarted") == Some(&"true");
        let matches = match p.outcome.as_str() {
            "initial" => group == 0,
            "restart" => restarted || group == 0,
            _ if p.group.is_some() => restarted || p.group == Some(group),
            "" => restarted || group > 0 || allow_group_zero,
            _ => false,
        };
        if !matches { continue; }
        let round = parse_rounds(&log[..start]).pop().unwrap_or_default();
        let mut failures = round.failed;
        if fields.get("borrowck") == Some(&"failed") {
            failures.push(format!("borrow check failed: {}", fields.get("first").copied().unwrap_or("see run log")));
        }
        return Some(crate::state::LiveTests { group, ran, passed, failed, failures, ms, restarted,
            ignored: round.ignored.len() as u64, skipped: number("skipped").unwrap_or_default(),
            selected: fields.get("selected").copied().unwrap_or_default().into(), ..Default::default() });
    }
    None
}

fn probe_cut_events(events: &[Value], live_lines: &[String], log_tail: &str, p: &LivePatch, allow_group_zero: bool) -> CutProbe {
    let seq = |v: &Value| v.get("seq").and_then(Value::as_u64);
    let reset = events.iter().filter_map(seq).max().is_some_and(|n| n < p.seq)
        || events.iter().any(|v| seq(v) == Some(p.seq) && !p.cursor.is_empty() && event_cursor(v) != p.cursor);
    let since = if reset { 0 } else { p.seq };
    let mut newer: Vec<_> = events.iter().filter(|v| seq(v).is_some_and(|n| n > since)).collect();
    newer.sort_by_key(|v| seq(v));
    let mut group = p.group;
    let mut patch = None;
    let mut terminal_event = None;
    let mut restarted = reset || p.outcome == "restart";
    for v in newer {
        match v.get("event").and_then(Value::as_str) {
            Some("patchverified" | "patchrestarted" | "patchfailed") => {
                restarted |= v.get("event").and_then(Value::as_str) == Some("patchrestarted");
                group = v.get("group").and_then(Value::as_u64).or(group);
                patch = Some(v.clone());
                terminal_event = None;
            }
            Some("testverdict") if v.get("reverted").is_none() => {
                if v.get("faulted").is_some() || crate::state::live_tests_from(v).is_some_and(|t| t.restarted || Some(t.group) == group
                    || (t.group == 0 && (p.outcome == "initial" || (restarted && p.outcome.is_empty())))) {
                    return CutProbe::Finished { patch, verdict: v.clone() };
                }
            }
            Some("patchrefused" | "patchreverted" | "jiterror") => terminal_event = Some(v.clone()),
            _ => {}
        }
    }
    if let Some(verdict) = terminal_event { return CutProbe::Finished { patch, verdict }; }
    if let Some(t) = log_test_verdict(log_tail, p, allow_group_zero) { return CutProbe::FromLog(t); }
    if let Some(v) = patch { return CutProbe::RunningPatch(v); }
    if matches!(edit_state(&Ok(live_lines.to_vec()), p.patches_at_ship), Some(EditState::On(_))) ||
        parse_rounds(log_tail).last().is_some_and(|r| !r.finished && r.total > 0) { return CutProbe::StillRunning; }
    CutProbe::Lost(if live_lines.is_empty() { "the JIT's inspect socket could not be reached and its run log has no verdict" }
        else { "the JIT is idle with no matching verdict in its event queue or run log" }.into())
}

fn apply_cut_patch(ctx: &Ctx, id: &str, x: &Live, p: &mut LivePatch, v: &Value) {
    if let Some(edit) = v.get("edit") { p.edit = edit.as_str().map(str::to_string).unwrap_or_else(|| edit.to_json()); }
    p.group = v.get("group").and_then(Value::as_u64).or(p.group);
    if !hot_event(ctx, id, x, p, v) {
        let (outcome, reason) = verified_outcome(Some(v));
        p.outcome = outcome.into();
        p.reason = reason;
        p.reason_ref = Default::default();
        p.outcome_ms = wall_ms();
        p.seq = v.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
        p.cursor = event_cursor(v);
    }
}

fn allow_group_zero(x: &Live, p: &LivePatch) -> bool {
    p.outcome == "initial" || p.log_start > 0 || p.edit_ms <= x.started_ms
}

fn recover_cut_verdict(ctx: &Ctx, id: &str, r: &Running, x: &mut Live, p: &mut LivePatch) -> bool {
    let events: Vec<_> = inspect(&x.node, x.pid, "events").unwrap_or_default().iter()
        .filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let live = inspect(&x.node, x.pid, "live").unwrap_or_default();
    let log = run_log(r, x.run_n, p.log_start).map(|(_, log)| log).unwrap_or_default();
    let probe = probe_cut_events(&events, &live, &log, p, allow_group_zero(x, p));
    if watch_superseded(r) { return false; }
    match probe {
        CutProbe::Finished { patch, verdict } => {
            if let Some(v) = patch { apply_cut_patch(ctx, id, x, p, &v); }
            if verdict.get("event").and_then(Value::as_str) == Some("testverdict") {
                if let Some(t) = crate::state::live_tests_from(&verdict) {
                    // The probe matched the round across the restart gap. Preserve that
                    // identity when the ordinary collector applies its group check.
                    if t.restarted { p.outcome = "restart".into(); }
                    else if t.group == 0 && p.group.is_none() { p.outcome = "initial".into(); }
                    else if p.group == Some(t.group) { p.outcome = "patched".into(); }
                }
            }
            p.seq = verdict.get("seq").and_then(Value::as_u64).unwrap_or(p.seq);
            p.cursor = event_cursor(&verdict);
            let mut event = Some(verdict);
            collect_test_verdict(ctx, id, x, p, |_, _| event.take());
            if let Some(t) = &p.tests {
                if p.outcome.is_empty() { p.outcome = if t.restarted { "restart" } else { "patched" }.into(); }
            }
        }
        CutProbe::FromLog(t) => {
            p.outcome = if t.restarted { "restart" } else { "patched" }.into();
            p.tests = Some(t);
            p.verdict_ms = wall_ms();
            p.reason = "recovered from run log after master restart".into();
        }
        CutProbe::RunningPatch(v) => {
            apply_cut_patch(ctx, id, x, p, &v);
            set_test_patch(ctx, id, x, p);
        }
        CutProbe::StillRunning => {
            if p.outcome.is_empty() && parse_rounds(&log).last().is_some_and(|r| !r.finished && r.total > 0) {
                p.outcome = if p.group.is_some() { "patched" } else { "restart" }.into();
                set_test_patch(ctx, id, x, p);
            }
        }
        CutProbe::Lost(why) => {
            *p = lost_verdict(p, wall_ms(), format!("verdict lost: {why}; {NEXT_CALL_RERUNS}"));
            if let Some(x) = set_test_patch(ctx, id, x, p) { event(ctx, Sev::Warn, format!("live {id}: {}", p.reason), Some(&x)); }
            return true;
        }
    }
    false
}

fn lost_verdict(p: &LivePatch, now: u64, reason: String) -> LivePatch {
    LivePatch { tests: None, outcome: "failed".into(), outcome_ms: now, verdict_ms: now, reason, reason_ref: Default::default(), ..p.clone() }
}

fn cut_by_restart(x: &Live, now: u64, following: bool) -> Option<Cut> {
    if x.test {
        let p = x.patch.as_ref().filter(|p| !terminal(p) && p.outcome != "recovering")?;
        if p.outcome.is_empty() && p.shipped_ms == 0 || !following {
            let reason = if p.shipped_ms > 0 { CUT_LOST_NO_RUN } else { CUT_LOST_UNSHIPPED };
            return Some(Cut::Said(lost_verdict(p, now, reason.into())));
        }
        return Some(Cut::RecoverTest);
    }
    let p = x.patch.as_ref().filter(|p| p.outcome.is_empty())?;
    if p.shipped_ms > 0 { return Some(Cut::Watch); }
    let mut cut = LivePatch { outcome_ms: now, ..p.clone() };
    if p.files.is_empty() {
        cut.outcome = "failed".into();
        cut.reason = "the master restarted before this edit's verdict: the session's next call ships its saves again".into();
        return Some(Cut::Said(cut));
    }
    cut.outcome = "superseded".into();
    cut.reason = format!("the master restarted before this edit shipped: its {} file(s) ship again, and that verdict covers it", p.files.len());
    Some(Cut::Ship(cut, p.files.clone()))
}

/// A test session's edit its node took before the master's restart cut its verdict: nothing is
/// left to ship, and the next call answered that cut verdict again, call after call, until the
/// session ended (W18256, 11-24). That call runs its tests again on its JIT restarted warm.
const CUT_RERUN: &str = "the master restarted before this edit's verdict: the session's next call runs its tests again";
const CUT_LOST_UNSHIPPED: &str = "verdict lost: the master restarted before this edit finished shipping; the session's next call ships its saves again";
const CUT_LOST_NO_RUN: &str = "verdict lost: the master restarted and the run for this edit is gone from its box; the session's next call runs its tests again";
const NEXT_CALL_RERUNS: &str = "the session's next call runs its tests again";

/// Whether `p` is such a cut verdict ([`CUT_RERUN`]).
fn rerun_cut(p: &LivePatch) -> bool {
    if p.outcome != "failed" || p.shipped_ms == 0 { return false; }
    let why = reason_text(p);
    why == CUT_RERUN || why.ends_with(NEXT_CALL_RERUNS)
}

fn reason_text(p: &LivePatch) -> String {
    if p.reason.is_empty() { p.reason_ref.read().unwrap_or_default() } else { p.reason.clone() }
}

/// Holds a live job whose lease and app outlived the previous master, until it ends.
/// The run of a session the restarted master took back on its held lease, followed again: its
/// saves are heard ([`join`]). A restart is no call: the session keeps its own touch, and its idle
/// end with it ([`idle_now`]).
fn take_back(ctx: &Ctx, x: &Live) -> Arc<Running> {
    let held = Held::attach(&x.lease, &x.node);
    let tree = PathBuf::from(&x.tree);
    // an app's ships wait for its pid, as a started session's do (the run's watcher frees them):
    // one shipped before it found no app and dropped its files
    let r = Arc::new(Running { node: x.node.clone(), lease: x.lease.clone(), held, tree, test: x.test, pending: Mutex::new((BTreeSet::new(), 0, None)), up_pid: Default::default(), calls: Mutex::new(Calls::default()), inflight: std::sync::atomic::AtomicBool::new(true), restart: std::sync::atomic::AtomicBool::new(false), recover_cut: Default::default(), resume_active: Default::default(), supersede: Default::default(), active_wait: Default::default(), resuming: std::sync::atomic::AtomicBool::new(x.test), fault: Mutex::new(None), waiting: Default::default(), relaunch_waits: Default::default(), kept_said: Default::default() });
    join(&ctx.monitor, &x.id, r.clone());
    r
}

fn reattach_claimed(ctx: &Arc<Ctx>, x: &Live, claim: Claim) {
    let r = take_back(ctx, x);
    let len = std::fs::metadata(&x.log).map_or(0, |m| m.len());
    // a run asked for and not noted is asked again under its name ([`rejoin`]): the node answers
    // from it, or from its JIT resolution going on, or starts it if it never got the ask
    let rejoined = rejoin(x);
    let follow = match rejoined {
        Rejoin::Ask => None,
        Rejoin::Follow(n) => Some((n, len.saturating_sub(x.run_log_start))),
    };
    let found = lost_pid(x, follow).map(|(pid, run)| {
        set(ctx, &x.id, |s| if (s.pid, s.run_pid) == (x.pid, x.run_pid) { (s.pid, s.run_pid) = (pid, run); });
        event(ctx, Sev::Info, format!("live {}: restored app pid {pid} and run pid {run} from its log", x.id), Some(x));
        Live { pid, run_pid: run, ..x.clone() }
    });
    let x = found.as_ref().unwrap_or(x);
    if rejoined == Rejoin::Ask {
        // the restart's new run itself, asked: nothing of the old one is left to stop, and a
        // restart marked now would start the app once more when that new run ends
    } else if x.patch.as_ref().is_some_and(|p| p.outcome == "recovering") {
        r.restart.store(true, std::sync::atomic::Ordering::SeqCst);
        if stop_run(ctx, &r, x).is_err() { r.restart.store(false, std::sync::atomic::Ordering::SeqCst); }
    }
    match cut_by_restart(x, wall_ms(), follow.is_some()) {
        Some(Cut::RecoverTest) => r.recover_cut.store(true, SeqCst),
        Some(Cut::Watch) => {
            let (c, id, r2, mut x2) = (ctx.clone(), x.id.clone(), r.clone(), x.clone());
            let _ = std::thread::Builder::new().name("devhub-live-rewatch".into()).spawn(move || {
                let Some(mut p) = x2.patch.clone() else { return };
                if !wait_up(&c, &id, &r2, x2.pid) { return; }
                patch_verdict(&c, &id, &mut x2, &mut p);
                set_patch(&c, &id, &p);
                event(&c, Sev::Info, format!("live {id}: the edit the master's restart cut is {} ({} file(s))", p.outcome, p.files.len()), None);
            });
        }
        Some(Cut::Ship(cut, files)) => {
            let now = set_patch(ctx, &x.id, &cut);
            due(&r.pending, files);
            event(ctx, Sev::Info, format!("live {}: {}", x.id, cut.reason), now.as_ref());
        }
        Some(Cut::Said(cut)) => {
            let now = set_patch(ctx, &x.id, &cut);
            event(ctx, Sev::Info, format!("live {}: {}", x.id, cut.reason), now.as_ref());
        }
        None => {}
    }
    // saves the previous master took and had not shipped (its queue, which `live sync` answered
    // "queued", was its memory) ship now: what differs from the session's sources
    if !x.test {
        match saved_paths(ctx, &r.tree, x) {
            Ok(files) => due(&r.pending, files),
            Err(e) => event(ctx, Sev::Warn, format!("live {}: its saves since the master's restart were not read: {e}", x.id), Some(x)),
        }
    }
    // no run of it on the node: the previous master's ask goes on under its name
    let asked = (follow.is_none() && !x.ask.is_empty()).then(|| x.ask.clone());
    let res = run_processes(ctx, &x.id, &r, follow, asked);
    finished(ctx, &x.id, &r, res);
    drop(claim);
}

/// The pids of a session's app that its state lost while its run went on, read back from its log:
/// a run followed from where its log stands (`follow`: the run and the offset into its output)
/// skips its announce. 11-17: a master that panicked mid-follow left W12160 "running" with pid 0;
/// the next one followed its run from the log's end, never saw the pid, and refused every act and
/// relaunch as "its JIT is not up yet", and `live again` as "still runs", until the lane released it.
fn lost_pid(x: &Live, follow: Option<(u64, u64)>) -> Option<(u32, u32)> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    if !follow.is_some_and(|(n, off)| n == x.run_n && off > 0) {
        return None;
    }
    if x.pid != 0 && x.run_pid != 0 && x.run_pid != x.pid {
        return None;
    }
    let mut log = BufReader::new(std::fs::File::open(&x.log).ok()?);
    log.seek(SeekFrom::Start(x.run_log_start)).ok()?;
    let mut pids = None;
    let mut line = Vec::new();
    while log.read_until(b'\n', &mut line).ok()? != 0 {
        announce(String::from_utf8_lossy(&line).trim_end_matches('\n'), &x.app, &mut pids);
        line.clear();
    }
    // The old master may have saved the app pid just before the parent line reached the log.
    pids.filter(|&(pid, run)| x.pid == 0 || (x.pid == pid && run != pid && x.run_pid != run))
}

/// Whether a run followed from where its log stands ([`lost_pid`]'s `follow`) is still in its
/// box's JIT build, by its part of the log ([`jit_build_line`]): a master that restarted while
/// cargo built it reads the build as silence otherwise ([`no_startup_line`]).
fn building_in_log(x: &Live, follow: Option<(u64, u64)>) -> bool {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    if !follow.is_some_and(|(n, off)| n == x.run_n && off > 0) { return false; }
    let Ok(file) = std::fs::File::open(&x.log) else { return false };
    let mut log = BufReader::new(file);
    if log.seek(SeekFrom::Start(x.run_log_start)).is_err() { return false; }
    let (mut building, mut line) = (false, Vec::new());
    while log.read_until(b'\n', &mut line).is_ok_and(|n| n != 0) {
        jit_build_line(&String::from_utf8_lossy(&line), &mut building);
        line.clear();
    }
    building
}

/// How often a running session's process is looked at on its box ([`alive_look`]): a lease's beat.
const ALIVE_EVERY: Duration = Duration::from_secs(2);

/// What the looks at a session's process saw ([`alive_look`]).
#[derive(Default)]
struct AliveLook {
    pid: u32,
    /// its start, as its box said it (a pid used again is another process)
    start: Option<u64>,
    /// when a look last saw it run (or its pid was first looked at)
    seen_ms: u64,
    /// looks in a row that found it gone
    gone: u32,
}

/// A look at a running session's process on its box (`lease-pid`): why the session ends, once two
/// looks in a row find it gone while its run goes on. A run that ends with its process (a crash
/// its log says) is answered by that end, which comes first. A JIT test process gone that way
/// left its run on, and its session read "still running: tests" for 52 and 37 min, its caller
/// waiting (W11754, W11740, 11-17). Nothing is said while it restarts, before its pid is known, or
/// without its box's answer (the box away, an older hub).
fn alive_look(ctx: &Ctx, id: &str, lease: &str, restarting: bool, look: &mut AliveLook) -> Option<String> {
    let x = get(ctx, id)?;
    if x.state != "running" || x.pid == 0 || restarting || lease.is_empty() {
        look.gone = 0;
        return None;
    }
    // behind a restart parent, the run's pid: it stays while the app's process restarts, and
    // the app's pid is gone from its end until the new process announces
    let pid = if x.run_pid > 0 { x.run_pid } else { x.pid };
    if pid != look.pid { *look = AliveLook { pid, start: None, seen_ms: wall_ms(), gone: 0 }; }
    match ctx.leases.lease_pid(&x.node, lease, pid, look.start) {
        Ok(Some(start)) => {
            look.start = look.start.or(Some(start).filter(|s| *s > 0));
            (look.seen_ms, look.gone) = (wall_ms(), 0);
            None
        }
        Ok(None) => {
            look.gone += 1;
            (look.gone >= 2).then(|| format!("the process is gone (last seen {}): pid {pid} ended on {} while its run went on", hhmmss(look.seen_ms), x.node))
        }
        Err(_) => None,
    }
}

/// Ends a session whose process is gone ([`alive_look`]): its edit's answer says so, its lease
/// goes (what is left of its run with it), and the call that waits on it answers with its end.
fn process_gone(ctx: &Ctx, id: &str, note: &str) {
    let Some(x) = get(ctx, id).filter(|x| x.active()) else { return };
    event(ctx, Sev::Warn, format!("live {id} ({}) of lane {} ends: {note}", x.app, x.lane), Some(&x));
    if let Err(e) = release(ctx, id, note) {
        event(ctx, Sev::Warn, format!("live {id}: ending it: {e}"), Some(&x));
    }
}

/// How long ago a running session's run last wrote (its log's last change here), None when it is
/// not running: a hung run says how long it has been silent ([`render`], the caller's status).
pub(crate) fn quiet_ms(x: &Live, now: u64) -> Option<u64> {
    if x.state != "running" || x.log.is_empty() { return None; }
    let at = std::fs::metadata(&x.log).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(now.saturating_sub(at))
}

/// The live job's app ended: its lease goes and the job is over.
fn finished(ctx: &Ctx, id: &str, r: &Running, res: Result<i32, String>) {
    RUNNING.lock().unwrap().as_mut().map(|m| m.remove(id));
    // (in the master's own scheduler: a release it asks of itself refuses an active session's lease)
    ctx.leases.release_live(&r.lease);
    let jit_verdict = if matches!(res, Ok(makepad_remote::hot::JIT_ERROR_EXIT)) {
        get(ctx, id).and_then(|s| std::fs::read_to_string(s.log).ok())
            .and_then(|text| makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &text)).map(|v| v.line())
    } else { None };
    let progress = end_progress(ctx, id);
    let x = set(ctx, id, |s| {
        if s.active() {
            s.state = "ended".into();
            s.ended_ms = wall_ms();
        }
        if let Some(verdict) = &jit_verdict {
            s.note = verdict.clone();
            s.panic = None;
        } else if s.note.is_empty() {
            s.note = match &res {
                Ok(c) => format!("the app exited ({c})"),
                Err(e) => crate::proto::clean(e, 200),
            };
        }
        settle_open_patch(s, wall_ms(), progress.as_ref());
    });
    let _ = std::fs::remove_dir_all(ctx.logs("live").join(format!("{id}.ship")));
    event(ctx, Sev::Info, format!("live {id} ended: {}", x.as_ref().map_or(String::new(), |x| x.note.clone())), x.as_ref());
}

/// The partner's current ask, including a queued ask: the store records its lease only after
/// its sources ship. Waiting for that record alone would leave a set waiting for its joiner.
fn partner_ask(ctx: &Ctx, joins: &str) -> Result<String, String> {
    let asked = ASKED.lock().unwrap().get(joins).cloned();
    let lease = ctx.store.read(|d| d.lives.iter().find(|j| j.id == joins && j.active()).map(|j| j.lease.clone()))
        .ok_or_else(|| format!("the session it joins ({joins}) ended before it got its box"))?;
    Ok(asked.unwrap_or(lease))
}

/// Return ownership to the caller if the OS cannot start the thread. It can then finish
/// the session's store transition while still holding the claim.
fn spawn_claimed(claim: Claim, worker: impl FnOnce(Claim) + Send + 'static) -> Result<(), (std::io::Error, Claim)> {
    let slot = Arc::new(Mutex::new(Some(claim)));
    let worker_slot = slot.clone();
    let spawned = std::thread::Builder::new().name("devhub-live".into()).spawn(move || {
        let claim = worker_slot.lock().unwrap().take().expect("worker owns the claim");
        worker(claim);
    });
    match spawned {
        Ok(_) => Ok(()),
        Err(e) => Err((e, slot.lock().unwrap().take().expect("spawn failed before thread ran"))),
    }
}

fn spawn_run(ctx: &Arc<Ctx>, claim: Claim, first_call: bool) -> Result<(), (std::io::Error, Claim)> {
    let c = ctx.clone();
    spawn_claimed(claim, move |claim| run_claimed(&c, claim, first_call))
}

fn spawn_reattach(ctx: &Arc<Ctx>, x: Live, claim: Claim) -> Result<(), (std::io::Error, Claim)> {
    let c = ctx.clone();
    spawn_claimed(claim, move |claim| reattach_claimed(&c, &x, claim))
}

fn failed_to_spawn_run(ctx: &Ctx, claim: Claim, e: std::io::Error) {
    let note = format!("failed to spawn live thread: {e}");
    let progress = end_progress(ctx, claim.id());
    let x = set(ctx, claim.id(), |s| {
        s.state = "failed".into();
        s.ended_ms = wall_ms();
        s.note = note.clone();
        settle_open_patch(s, s.ended_ms, progress.as_ref());
    });
    event(ctx, Sev::Warn, format!("live {}: {note}", claim.id()), x.as_ref());
}

#[cfg(test)]
fn try_run(ctx: &Arc<Ctx>, id: &str, first_call: bool, by: LaunchPath) -> bool {
    let Some(claim) = try_claim_or_warn(ctx, id, by) else { return false };
    run_claimed(ctx, claim, first_call);
    true
}

/// Places the live job, starts its app, runs until it ends.
fn run_claimed(ctx: &Arc<Ctx>, claim: Claim, first_call: bool) {
    let id = claim.id();
    let Some(x) = get(ctx, id) else { return };
    // (a job released meanwhile stays released: its lease wait gives up because of that)
    let fail = |e: String| {
        let mut failed = false;
        let progress = end_progress(ctx, id);
        let x = set(ctx, id, |s| {
            if !s.active() { return; }
            failed = true;
            s.state = "failed".into();
            s.ended_ms = wall_ms();
            s.note = crate::proto::clean(&e, 300);
            settle_open_patch(s, s.ended_ms, progress.as_ref());
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
        // each followed one the worktree lacks at its landed tip, as the lane's runs pin it
        rev.push_str(&crate::nested::absent_sides(&tree));
        let pin = crate::lease::Pin::of_rev(&rev).ok_or("live: missing pin")?;
        let (mut changed, deleted) = crate::lease::ship::uncommitted(&tree, &pin)?;
        changed.extend(also_files(&tree, &x.also)?);
        changed.sort();
        changed.dedup();
        if changed.is_empty() && deleted.is_empty() { Ok((rev, String::new(), 0)) }
        else { super::job::copy_snapshot(ctx, &tree, rev, changed, deleted) }
    })();
    let (rev, snap, _) = match snapshot.and_then(|s| super::job::whole_pair(ctx, &s.0).map(|()| s)) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    set(ctx, id, |x| x.rev = rev.clone());
    // Overlays are keyed by lease lane: each target needs its own source state.
    let (kind, needs) = lease_kind(&x);
    let spec = Spec { since_ms: None, lane: if x.test { format!("live-{id}") } else { x.lane.clone() }, agent: x.agent.clone(), kind, needs: needs.clone(), prio: crate::lease::sched::DEFAULT_PRIO, rev, repo_dir: x.tree.clone(), node: (!x.pin.is_empty()).then(|| x.pin.clone()) };
    let argv: Vec<String> = ["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run"].iter().map(|s| s.to_string()).chain(std::iter::once(x.app.clone())).collect();
    let src = Src::for_run(spec.kind, &argv, x.src == "tree", false);
    let configure = |r: &mut crate::lease::sched::Req, joined_lease: Option<String>| {
        r.run_options(src, CostClass::parse(&x.cost), x.est_min, &x.class, &argv);
        r.set = (!x.set.is_empty()).then(|| x.set.clone());
        r.joins = joined_lease;
        r.cores = x.cores;
        r.ram_gb = x.ram_gb;
        r.live = true;
        // (its lane's cap counts it, not its overlay's own lane's: [`Req::for_lane`])
        if x.test { r.for_lane = Some(x.lane.clone()); }
        r.priority = Priority::parse(&x.priority).unwrap_or(Priority::Interactive);
        r.loader = (!x.loader.is_empty()).then(|| x.loader.clone());
        // its box as a lease's flags place it: `--node` (or the box of the session it joins),
        // `--needs`, `--avoid`, `--series`, `--pin-why`
        r.place(&x.pin, &needs, &x.avoid, &x.series, &x.pin_why);
    };
    // A box that goes away while the sources ship is no verdict on the session (its hub restarts
    // for its self-update, most often): the lease goes back and it is placed again, as a job is.
    // So is an ask for its box whose answer was lost ([`super::leg::exit_lost`]): a live test's
    // session queued 9 min for its box failed on "beat: no answer", its test never run (W4322, 11-08).
    // It waits for its box as any lease does, in the queue by what the boxes have free; how many
    // sessions its lane has decides only its order (past its lane's cap it is overflow,
    // [`Req::for_lane`]), never refused or ended. Its wait is its note, and said once in the log. At most
    // three headless sessions per lane, a lane's next one ended its least recently used one
    // between two calls, and a lane's WM screenshot tests ran again (11-13).
    let waited: std::cell::Cell<Option<u64>> = std::cell::Cell::new(None);
    let asking = Asking(id);
    let mut on_wait = |line: &str| {
        if let Some(lease) = super::leg::asked_lease(line) { asking.names(lease); }
        if line.contains(" granted") { return; }
        let first = waited.get().is_none();
        if first { waited.set(Some(wall_ms())); }
        let note = format!("waits for its box: {line}");
        let x = set(ctx, id, |s| s.note = note.clone());
        if first { event(ctx, Sev::Info, format!("live {id}: {note}"), x.as_ref()); }
    };
    let mut placed = 0;
    let (held, ship_started, home_dir) = loop {
        let joined_lease = if x.joins.is_empty() { None } else {
            loop {
                if get(ctx, id).is_none_or(|x| !x.active()) { return; }
                let lease = match partner_ask(ctx, &x.joins) {
                    Ok(lease) => lease,
                    Err(note) => {
                        let ended = set(ctx, id, |s| if s.active() { s.state = "ended".into(); s.ended_ms = wall_ms(); s.note = note.clone(); settle_open_patch(s, s.ended_ms, None); });
                        event(ctx, Sev::Info, format!("live {id}: {note}"), ended.as_ref());
                        return;
                    }
                };
                if !lease.is_empty() && ctx.leases.view().iter().any(|l| l.id == lease && !matches!(l.state, crate::lease::sched::State::Ended(_))) { break Some(lease); }
                std::thread::sleep(Duration::from_secs(1));
            }
        };
        // A partner can end or ask again while this ask is queued. Give the old join up and
        // resolve it again before asking for another box.
        let partner_changed = || joined_lease.as_ref().is_some_and(|lease| partner_ask(ctx, &x.joins).as_ref().ok() != Some(lease));
        let stop = || get(ctx, id).is_none_or(|x| !x.active()) || partner_changed();
        let held = match Held::acquire_with(&spec, &|r| configure(r, joined_lease.clone()), &mut on_wait, &stop) {
            Ok(h) if partner_changed() => { h.release(None); continue; }
            Ok(h) => h,
            Err(_) if partner_changed() => continue,
            // placed again however often, after a pause that grows ([`super::job::again_after`])
            Err(e) if super::leg::exit_lost(&e) => {
                placed += 1;
                let why = format!("no box ({}); placed again in {} s", crate::proto::clean(&e, 160), super::job::again_after(placed).as_secs());
                let x = set(ctx, id, |s| s.note = why.clone());
                event(ctx, Sev::Warn, format!("live {id}: {why}"), x.as_ref());
                if !super::job::pause_again(placed, &|| get(ctx, id).is_none_or(|x| !x.active())) { return; }
                continue;
            }
            Err(e) => return fail(e),
        };
        placed += 1;
        let ship_started = wall_ms();
        let shipped = if !snap.is_empty() || src == Src::Overlay { super::job::ship_snap(ctx, &held, &snap).map(drop).map_err(|e| (format!("shipping {snap}"), e)) } else { Ok(()) };
        // its home's files go to the box once (by their hash) and the app gets its own copy
        let shipped = shipped.and_then(|()| match home_archive(ctx, &x) {
            Ok(None) => Ok(String::new()),
            Ok(Some(a)) => crate::lease::ship::send_home(held.client(), &held.node, &held.id, &a).map_err(|e| (format!("shipping its home {}", a.display()), e)),
            Err(e) => Err(("making its fresh home".to_string(), e)),
        });
        let (what, e) = match shipped {
            Ok(home_dir) => {
                if let Some(since) = waited.take() {
                    let x = set(ctx, id, |s| s.note.clear());
                    event(ctx, Sev::Info, format!("live {id}: has its box {} after {} s in the queue", held.node, wall_ms().saturating_sub(since) / 1000), x.as_ref());
                }
                break (held, ship_started, home_dir);
            }
            Err(e) => e,
        };
        let node = held.node.clone();
        held.release(None);
        if !super::leg::exit_lost(&e) {
            return fail(format!("{what}: {e}"));
        }
        event(ctx, Sev::Warn, format!("live {id}: {what} to {node} failed ({}); placed again in {} s", crate::proto::clean(&e, 160), super::job::again_after(placed).as_secs()), Some(&x));
        if !super::job::pause_again(placed, &|| get(ctx, id).is_none_or(|x| !x.active())) { return; }
    };
    // (one ended while its box was granted or its sources shipped starts no app: its lease goes)
    if get(ctx, id).is_none_or(|x| !x.active()) {
        held.release(None);
        return;
    }
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
    let r = Arc::new(Running { node: held.node.clone(), lease: held.id.clone(), held, tree: tree.clone(), test: x.test, pending: Mutex::new((BTreeSet::new(), 0, None)), up_pid: Default::default(), calls: Mutex::new(Calls::default()), inflight: std::sync::atomic::AtomicBool::new(true), restart: std::sync::atomic::AtomicBool::new(false), recover_cut: Default::default(), resume_active: Default::default(), supersede: Default::default(), active_wait: Default::default(), resuming: std::sync::atomic::AtomicBool::new(x.test && !first_call), fault: Mutex::new(None), waiting: Default::default(), relaunch_waits: Default::default(), kept_said: Default::default() });
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
        s.home_dir = home_dir;
        (s.work_dir, s.box_home) = (r.held.work.clone(), r.held.home.clone());
        if s.test {
            s.patch = Some(LivePatch { files: s.sources.keys().cloned().collect(), edit_ms: ship_started, shipped_ms: s.started_ms, outcome: "initial".into(), ..Default::default() });
        }
    });
    drop(asking);
    event(ctx, Sev::Info, format!("live {id}: {} starting on {} ({}, {})", x.as_ref().map_or("", |x| &x.app), r.node, r.lease, if x.as_ref().is_some_and(|x| x.visible) { "visible" } else { "headless" }), x.as_ref());
    retire_if_superseded(ctx, id, &r);
    drop(file);
    let res = run_processes(ctx, id, &r, None, None);
    finished(ctx, id, &r, res);
}

/// The CEF distribution a live app's run is given: where its box's checkout keeps one
/// ([`crate::lease::Caps::cef_dist`]): the overlay it runs from carries no `local/`, and a browser
/// host's bootstrap found none and panicked (W11267, 11-16). None for a test run, a run that names
/// its own, a box with none or a hub too old to say: the app starts all the same, and only a
/// browser view it opens needs one. Refusing a start on a box with none, by an app's dependencies,
/// stopped every Fab session on two boxes it had run on (11-16).
fn cef_env(test: bool, env: &[(String, String)], dist: &str) -> Option<String> {
    (!test && !dist.is_empty() && !env.iter().any(|(k, _)| k == "MAKEPAD_CEF_DIST_DIR")).then(|| format!("MAKEPAD_CEF_DIST_DIR={dist}"))
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
        let live: &[&str] = if x.test { &[] } else { &["RELAX_STORE_LIVE=1"] };
        let dist = crate::orch::leases().and_then(|lm| lm.nodes().into_iter().find(|n| n.name == r.node)).and_then(|n| n.caps).map(|c| c.cef_dist).unwrap_or_default();
        let mut argv = vec!["env".to_string()];
        argv.extend(x.env.iter().map(|(k, v)| format!("{k}={v}")));
        argv.extend(cef_env(x.test, &x.env, &dist));
        if !x.home_dir.is_empty() { argv.push(format!("MAKEPAD_TEST_HOME={}", x.home_dir)); }
        // Marks the app as a test run (its lane and session) for anything it shows to other
        // devices, so a lane's instance is never mistaken for the user's own.
        if !x.env.iter().any(|(k, _)| k == "MAKEPAD_TEST_RUN") { argv.push(format!("MAKEPAD_TEST_RUN={} {}", x.lane, x.id)); }
        // An agent drives every session: it asks for the app's remote endpoint itself, whatever
        // its box's setting (`~/.makepad/remote`, which a box's hub writes at its own start: a
        // hub not yet rolled onto that code had none, and sessions there opened no endpoint).
        if !x.env.iter().any(|(k, _)| k == "MAKEPAD_REMOTE") { argv.push("MAKEPAD_REMOTE=1".into()); }
        argv.extend(hide.iter().chain(live).chain(&["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--"]).map(|s| s.to_string()));
        argv.extend(if x.test { ["test", "--live", &x.app].into_iter().chain(crate::lease::JIT_TEST_PROFILE).map(String::from).collect() } else { vec!["run".into(), x.app.clone()] });
        argv.extend(x.test_args.iter().cloned());
        let file = std::fs::OpenOptions::new().append(true).create(true).open(&x.log).map_err(|e| e.to_string())?;
        let log_start = file.metadata().map_err(|e| e.to_string())?.len();
        let pids = follow.filter(|_| x.pid > 0).map(|_| (x.pid, if x.run_pid > 0 { x.run_pid } else { x.pid }));
        let building = pids.is_none() && building_in_log(&x, follow);
        // (a run followed again has started)
        let out = RunOut { file, buf: Vec::new(), pids, app: x.app.clone(), r: r.clone(), scan: Scan::default(), wrote: false, building, started: follow.is_some() };
        let shared = Arc::new(Mutex::new(out));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (c2, id2, s2, d2, r2) = (ctx.clone(), id.to_string(), shared.clone(), done.clone(), r.clone());
        // the session follows the newest announce for the run's whole life: its first starts the
        // session's calls, and each later one (a process restart behind a restart parent) moves
        // the pid that inspect, verdicts and the alive look address
        let watcher = std::thread::Builder::new().name("devhub-live-pid".into()).spawn(move || {
            let mut seen = None;
            while !d2.load(std::sync::atomic::Ordering::SeqCst) && running(&id2).is_some() {
                let (pids, wrote, started, building) = { let o = s2.lock().unwrap(); (o.pids, o.wrote, o.started, o.building) };
                if startup_due(pids, wrote, started, building) { no_startup_said(&c2, &id2); }
                if let Some((pid, run_pid)) = pids.filter(|pair| Some(*pair) != seen) {
                    let first = seen.is_none();
                    let guard = (first && r2.test).then(|| {
                        r2.resume_active.store(true, SeqCst);
                        ResumeGuard(&c2, &id2, &r2)
                    });
                    seen = pids;
                    // a new process loads the sources as they are: no save waits for a restart
                    let mut withdrew = false;
                    let x = set(&c2, &id2, |s| { if !d2.load(std::sync::atomic::Ordering::SeqCst) { if s.pid != pid { restart_ran(s); } s.pid = pid; s.run_pid = run_pid; withdrew = startup_read(s); } });
                    if withdrew { event(&c2, Sev::Info, format!("live {id2}: its startup line came after all (pid {pid}): the no-startup-line failure is withdrawn, and its run's verdict answers"), x.as_ref()); }
                    if d2.load(std::sync::atomic::Ordering::SeqCst) { return; }
                    if !first { continue; }
                    if x.as_ref().is_some_and(|x| x.test) { resume_tests(&c2, &id2, &r2, guard); }
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
                }
                std::thread::sleep(Duration::from_millis(if seen.is_some() { 100 } else { 20 }));
            }
        }).map_err(|e| e.to_string())?;
        // its process looked at on its box each beat while its run goes on ([`alive_look`]); the
        // thread ends with the run (a look in flight then is not waited for)
        let (c3, id3, d3, r3) = (ctx.clone(), id.to_string(), done.clone(), r.clone());
        std::thread::Builder::new().name("devhub-live-alive".into()).spawn(move || {
            let mut look = AliveLook::default();
            loop {
                let t = Instant::now();
                while t.elapsed() < ALIVE_EVERY {
                    if d3.load(std::sync::atomic::Ordering::SeqCst) || running(&id3).is_none() { return; }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let restarting = r3.restart.load(std::sync::atomic::Ordering::SeqCst);
                if let Some(note) = alive_look(&c3, &id3, &r3.lease, restarting, &mut look) {
                    // (the run's own end, come meanwhile, answers instead)
                    if !d3.load(std::sync::atomic::Ordering::SeqCst) { process_gone(&c3, &id3, &note); }
                    return;
                }
            }
        }).map_err(|e| e.to_string())?;
        let s4 = shared.clone();
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
                    set(ctx, id, |s| { s.ask = ask.clone(); s.ask_after = s.run_n; });
                    ask
                });
                r.held.run_live(&ask, &argv, x.offline, &mut Shared(shared), &mut |n| {
                    set(ctx, id, |s| { s.run_n = n; s.run_log_start = log_start; });
                    s4.lock().unwrap().started = true;
                })
            }
        };
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        set(ctx, id, |s| (s.pid, s.run_pid) = (0, 0));
        let _ = watcher.join();
        let Some(mut x) = get(ctx, id) else { return res };
        x.load_text()?;
        if x.test && x.active() && !r.restart.load(std::sync::atomic::Ordering::SeqCst) {
            let mut p = x.patch.clone().unwrap_or_default();
            let (log_end, log) = run_log(r, x.run_n, p.log_start).unwrap_or_else(|_| (p.log_end, String::new()));
            let mut scan = Scan::default();
            let fault = r.fault.lock().unwrap().take().or_else(|| log.lines().find(|l| fault_line(&x.app, l, &mut scan)).map(str::to_string));
            if let Some(verdict) = makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &log) {
                p.tests = None;
                p.reason = verdict.line();
                p.outcome = "jit_error".into();
                p.verdict_ms = wall_ms();
                p.jit_error = ctx.store.event_seq("jit_error", Sev::Err, p.reason.clone(), None);
                set_patch(ctx, id, &p);
            } else if let Some(reason) = fault {
                let own = begin_call(r);
                if own {
                    r.resuming.store(true, SeqCst);
                    // a session parking meanwhile restarts nothing: the fault is its edit's answer
                    if admit(ctx, id, r, None).is_err() {
                        r.resuming.store(false, SeqCst);
                        (p.tests, p.outcome, p.reason, p.verdict_ms) = (None, "failed".into(), reason, wall_ms());
                        set_patch(ctx, id, &p);
                        finish_call(ctx, id, r);
                        return res;
                    }
                }
                if own || !watch_superseded(r) {
                    p.tests = None;
                    p.reason = reason;
                    recover(ctx, id, &x, &mut p, false);
                } else {
                    (p.tests, p.outcome, p.reason, p.verdict_ms) = (None, "failed".into(), reason, wall_ms());
                    set_patch(ctx, id, &p);
                }
            } else if !terminal(&p) {
                if let Some(mut tests) = log_test_verdict(&log, &p, allow_group_zero(&x, &p)) {
                    tests.warnings = round_warnings(&log, Path::new(&x.tree));
                    p.log_end = log_end;
                    p.output = failure_output(&log, &tests.failures);
                    p.tests = Some(tests);
                    p.outcome = "recovered".into();
                    p.reason = "recovered from run log after the test process exited".into();
                    p.reason_ref = Default::default();
                } else {
                    p.outcome = if !crate::jit_misses::compile_errors(&log).is_empty() { "compile_error" } else { "failed" }.into();
                    p.reason = format!("live test ended without a verdict: {res:?}");
                    if !log.is_empty() { p.reason.push('\n'); p.reason.push_str(&log); }
                }
                p.verdict_ms = wall_ms();
                set_patch(ctx, id, &p);
            }
        }
        if !x.active() || !take_restart(r) { return res; }
        *r.fault.lock().unwrap() = None;
        set(ctx, id, |s| {
            (s.pid, s.run_pid) = (0, 0);
            if let Some(p) = &mut s.patch {
                p.seq = 0; p.cursor.clear(); p.tests = None; p.log_start = 0; p.log_end = 0;
                p.jit_notice.clear();
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
    if x.test && panic_fault(&x.app, &f("kind"), &f("message"), &f("location")) {
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
mod verified_outcome_tests {
    /// A verified patch that held items for a restart is no "patched": its outcome is a restart's,
    /// naming what is held. One that held nothing is patched.
    #[test]
    fn a_patch_that_held_items_for_a_restart_reads_as_a_restart() {
        let held = makepad_strict_json::parse(br#"{"event":"patchverified","seq":7,"edit":"closed_box.rs","group":3,"verified_events":[],"held":["held for a restart: closed_box.rs:2:1: struct `ClosedBox` changed its layout -- restart the app","held, as it needs a held item: closed_box.rs:7:40: E0609: no field `5`"]}"#).unwrap();
        let (outcome, reason) = super::verified_outcome(Some(&held));
        assert_eq!(outcome, "restart", "a patch in part asks for its restart");
        assert!(reason.contains("struct `ClosedBox` changed its layout") && reason.contains("no field `5`"), "the reason names what is held: {reason}");
        let whole = makepad_strict_json::parse(br#"{"event":"patchverified","seq":8,"edit":"a.rs","group":4,"verified_events":[]}"#).unwrap();
        assert_eq!(super::verified_outcome(Some(&whole)), ("patched", String::new()), "a patch that held nothing is patched");
    }

    /// A test watch follows its session's process: a restart behind the restart parent moves the
    /// pid, and the watch reads the new process's events from its first (W12859 waited 6 min on
    /// the old pid for the verdict its restarted tests gave in 3 s, 11-18). Another run, a newer
    /// edit, an ended session or one with no process yet ends it. A stop of the run follows it
    /// too: the moved pid is the run going on, its new process the one to stop (W18336 read it as
    /// stopped and waited 10 min for a run that never ended, 11-24); no process or another run is
    /// the run's end.
    #[test]
    fn a_test_watch_follows_its_sessions_restarted_process() {
        use crate::state::{Live, LivePatch};
        let edit = LivePatch { edit_ms: 5, seq: 2, cursor: "{\"seq\":2,\"pid\":7}".into(), ..Default::default() };
        let session = Live { state: "running".into(), pid: 7, run_n: 1, patch: Some(edit.clone()), ..Default::default() };
        let (mut x, mut p) = (session.clone(), edit.clone());
        assert!(super::watch_on(&mut x, Some(&session), &mut p));
        assert_eq!((x.pid, p.seq, p.cursor.as_str()), (7, 2, edit.cursor.as_str()), "the same process: the watch goes on where it was");
        let moved = Live { pid: 9, ..session.clone() };
        assert!(super::watch_on(&mut x, Some(&moved), &mut p));
        assert_eq!((x.pid, p.seq, p.cursor.as_str()), (9, 0, ""), "a restarted process: its events from the first");
        let newer = LivePatch { edit_ms: 6, ..edit.clone() };
        for (now, why) in [(Live { run_n: 2, ..moved.clone() }, "another run"), (Live { patch: Some(newer), ..moved.clone() }, "a newer edit"), (Live { state: "ended".into(), ..moved.clone() }, "an ended session"), (Live { pid: 0, ..moved.clone() }, "no process yet")] {
            assert!(!super::watch_on(&mut x.clone(), Some(&now), &mut p.clone()), "{why} ends the watch");
        }
        assert!(!super::watch_on(&mut x, None, &mut p), "a session that is gone ends it");
        assert_eq!(super::run_to_stop(&session, Some(&session)), Some(7), "a stop quits the run's process");
        assert_eq!(super::run_to_stop(&session, Some(&moved)), Some(9), "a restart in place as the stop began: the run goes on, and its new process is stopped");
        for (now, why) in [(Some(Live { pid: 0, ..session.clone() }), "no process"), (Some(Live { run_n: 2, ..moved.clone() }), "another run"), (None, "no session")] {
            assert_eq!(super::run_to_stop(&session, now.as_ref()), None, "{why}: the run has ended");
        }
    }
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

    /// A round's relax warnings, once each, by the tree's path of their file, the lane's own marked:
    /// a file that differs from work's or that work has none of.
    #[test]
    fn a_rounds_warnings_say_which_files_the_lane_changed() {
        let base = std::env::temp_dir().join(format!("devhub-live-warn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("lane");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        git(&tree, &["init", "-q", "-b", "work"]);
        git(&tree, &["config", "user.email", "t@t"]);
        git(&tree, &["config", "user.name", "t"]);
        std::fs::write(tree.join("src/a.rs"), "a").unwrap();
        std::fs::write(tree.join("src/b.rs"), "b").unwrap();
        git(&tree, &["add", "."]);
        git(&tree, &["commit", "-q", "-m", "x"]);
        std::fs::write(tree.join("src/a.rs"), "a2").unwrap();
        std::fs::write(tree.join("src/c.rs"), "c").unwrap();
        let log = ["/box/tree/src/a.rs:1:1: warning: constant `X` is never used [dead_code]", "/box/tree/src/a.rs:1:1: warning: constant `X` is never used [dead_code]",
            "/box/tree/src/b.rs:2:3: warning: unused import: `y` [unused_imports]", "/box/tree/src/c.rs:4:5: warning: unused variable: `z`",
            "/elsewhere/std/lib.rs:9:9: warning: use of deprecated function", "/box/tree/src/a.rs:3:1: error: not a warning", "relax test: 4 warnings"].join("\n");
        let got = super::round_warnings(&log, &tree);
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(got, vec![
            ("src/a.rs:1:1: warning: constant `X` is never used [dead_code]".to_string(), true),
            ("src/b.rs:2:3: warning: unused import: `y` [unused_imports]".to_string(), false),
            ("src/c.rs:4:5: warning: unused variable: `z`".to_string(), true),
            ("/elsewhere/std/lib.rs:9:9: warning: use of deprecated function".to_string(), false),
        ], "a changed and a new file are the lane's, an unchanged one and one outside the tree are not; errors and summaries are no warnings");
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

#[cfg(all(test, unix))]
mod gone_tests {
    /// A session whose process is killed while its run goes on ends at the second look (a beat
    /// each), said "the process is gone (last seen ..)": its edit's answer says so, its lease ends
    /// on its box, and the call waiting on its verdict answers with that end. The box's `lease-pid`
    /// here is the node's own answer, on this machine's processes (W11754 read "still running:
    /// tests" for 52 min, its process gone, 11-17).
    #[test]
    fn a_session_whose_process_is_killed_ends_and_tells_its_caller() {
        use super::*;
        use crate::lease::sched::{Req, State};
        let home = std::env::temp_dir().join(format!("devhub-live-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let ended = Arc::new(Mutex::new(Vec::<String>::new()));
        let ends = ended.clone();
        let leases = Arc::new(crate::lease::master::LeaseMaster::new("", home.clone(), Arc::new(move |_: &str, service: &str, line: &str| {
            let w: Vec<&str> = line.split_whitespace().collect();
            match service {
                "lease-pid" => {
                    let start = crate::lease::exec::pid_runs(w[2].parse().unwrap(), w.get(3).map(|s| s.parse().unwrap()));
                    Ok(vec![format!("{{\"alive\":{},\"start\":{}}}", start.is_some(), start.unwrap_or(0))])
                }
                "lease-end" => {
                    ends.lock().unwrap().push(w[1].to_string());
                    Ok(vec![format!("{{\"ended\":{},\"leak\":[]}}", json_str(w[1]))])
                }
                _ => Err(format!("no {service} here")),
            }
        }), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let store = crate::state::Store::open(&home, "", Arc::new(|_: Record| 0)).unwrap();
        let ctx = Arc::new(Ctx { store, leases, machine: String::new(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home: home.clone() });
        let mut req = Req::new("lane", Kind::Test);
        req.live = true;
        ctx.leases.test_sched(|s| { s.restore(&format!("{{\"id\":\"L1\",\"node\":\"node\",\"req\":{}}}", req.to_json()), 1); });
        let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        let patch = LivePatch { outcome: "initial".into(), ..Default::default() };
        ctx.store.update(|d| d.lives.push(Live { id: "WG1".into(), app: "app".into(), lane: "lane".into(), lease: "L1".into(), node: "node".into(), state: "running".into(), test: true, pid, patch: Some(patch), ..Default::default() })).unwrap();
        // its caller waits for the first run's verdict
        let c = ctx.clone();
        let caller = std::thread::spawn(move || initial_call(&c, "WG1", ""));
        let mut look = AliveLook::default();
        assert_eq!(alive_look(&ctx, "WG1", "L1", false, &mut look), None, "it runs");
        assert!(look.start.is_some() && look.gone == 0, "its start is kept: a pid used again is another process");
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(alive_look(&ctx, "WG1", "L1", true, &mut look), None, "no look while it restarts");
        assert_eq!(alive_look(&ctx, "WG1", "L1", false, &mut look), None, "gone at one look: its run's own end may come yet");
        let note = alive_look(&ctx, "WG1", "L1", false, &mut look).expect("gone at two looks in a row");
        assert!(note.starts_with("the process is gone (last seen ") && note.ends_with(&format!("Z): pid {pid} ended on node while its run went on")), "{note}");
        process_gone(&ctx, "WG1", &note);
        let answer = caller.join().unwrap().unwrap();
        let told = crate::state::live_from(&makepad_strict_json::parse(answer.as_bytes()).unwrap());
        assert_eq!((told.state.as_str(), told.note.as_str()), ("ended", note.as_str()), "the waiting call answers with its end");
        let p = told.patch.unwrap();
        assert!(p.outcome == "ended" && terminal(&p), "its edit finishes with its session");
        assert!(p.reason.contains(&note) && p.reason.contains("ended before its verdict"), "{}", p.reason);
        assert_eq!(*ended.lock().unwrap(), ["L1"], "its lease ends on its box");
        assert!(!ctx.leases.view().iter().any(|l| l.id == "L1" && !matches!(l.state, State::Ended(_))), "and in the scheduler");
        assert_eq!(quiet_ms(&get(&ctx, "WG1").unwrap(), wall_ms()), None, "an ended session says no silence");
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[cfg(all(test, unix))]
mod relaunch_cut_tests {
    /// The master restarts between accepting a relaunch and applying it. A relaunch through devhub
    /// goes on under the next master: its new run, asked and not yet noted, is asked again under its
    /// name (its box answers from that run, whatever its number), never a second ask beside it nor
    /// another run taken for it; and the cut call, sent again, answers from that relaunch without starting it
    /// twice. A relaunch its JIT serves is sent again only while an edit still waits for it
    /// (W11772's was cut before it reached the app, and the app ran the old code, 11-17).
    #[test]
    fn a_relaunch_cut_by_a_master_restart_is_applied_once_after_it() {
        use super::*;
        let home = std::env::temp_dir().join(format!("devhub-live-relaunch-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // the master that took the relaunch: its old run (1) ended, the new one asked
        let before = crate::state::Store::open(&home, "", Arc::new(|_: Record| 0)).unwrap();
        before.update(|d| d.lives.push(Live { id: "W1".into(), app: "app".into(), lane: "lane".into(), lease: "L1".into(), node: "node".into(), state: "running".into(), run_n: 1, pid: 0, ask: "a-2".into(), ask_after: 1, patch: Some(LivePatch { outcome: "initial".into(), ..Default::default() }), ..Default::default() })).unwrap();
        drop(before);
        // the next master, on that state
        let store = crate::state::Store::open(&home, "", Arc::new(|_: Record| 0)).unwrap();
        let x = store.read(|d| d.lives[0].clone());
        assert_eq!((x.ask.as_str(), x.ask_after), ("a-2", 1), "the ask and the run it follows outlive the master");
        // (its box answers the ask from the run of that name, whatever its number: the run after
        // the noted one was a look's grab, W25972)
        assert_eq!(rejoin(&x), Rejoin::Ask, "the new run, asked and not noted, is asked again under its name, not guessed by its number");
        assert_eq!(rejoin(&Live { run_n: 2, pid: 9, ..x.clone() }), Rejoin::Follow(2), "a noted run is followed");
        assert_eq!(rejoin(&Live { run_n: 0, ask_after: 0, ..x.clone() }), Rejoin::Ask, "a first run as well");
        assert_eq!(rejoin(&Live { ask_after: 0, pid: 9, ..x.clone() }), Rejoin::Follow(1), "a state with no ask_after follows its run");
        // the cut call sent again: answered from the relaunch under way, its box not asked (there is none here)
        let leases = Arc::new(crate::lease::master::LeaseMaster::new("", home.clone(), Arc::new(|_: &str, service: &str, _: &str| Err(format!("no {service} here"))), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Arc::new(Ctx { store, leases, machine: String::new(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home: home.clone() });
        let answer = act(&ctx, "W1", &format!("session relaunch {RESEND}"), None).unwrap();
        assert!(answer.contains("the cut relaunch goes on through devhub"), "{answer}");
        ctx.store.update(|d| { d.lives[0].pid = 9; d.lives[0].run_n = 2; d.lives[0].patch = Some(LivePatch { outcome: "recovering".into(), ..Default::default() }); }).unwrap();
        assert!(act(&ctx, "W1", &format!("session relaunch {RESEND}"), None).unwrap().contains("goes on through devhub"), "one stopping its old run goes on too");
        let e = act(&ctx, "W1", &format!("ui act q=Save {RESEND}"), None).unwrap_err();
        assert!(e.contains("is for a call devhub can look at: session relaunch"), "only a call devhub can look at is resent: {e}");
        // a relaunch its JIT serves: sent again only while an edit waits for it
        assert!(relaunch_applied("W1", 9, 0).is_some_and(|l| l.contains("runs the current sources")));
        assert_eq!(relaunch_applied("W1", 9, 2), None);
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[cfg(test)]
mod announce_tests {
    /// The session follows the newest announce of its own app: a process restart behind a
    /// restart parent announces a new pid (and the run's, which stays), and another package's
    /// announce in the same output is never the app's (11-17).
    #[test]
    fn the_newest_announce_of_the_sessions_app_names_its_pids() {
        let lines = [
            ("[makepad-remote] pid=41 app=makepad-fab socket=/t/a", (41, 41)),
            ("[makepad-remote] run=40 pid=41", (41, 40)),
            ("[makepad-remote] pid=77 app=makepad-scope socket=/t/s", (41, 40)),
            ("[makepad-remote] run=76 pid=77", (41, 40)),
            ("[makepad-remote] pid=52 app=makepad-fab socket=/t/b", (52, 52)),
            ("[makepad-remote] run=40 pid=41", (52, 52)),
            ("[makepad-remote] run=40 pid=52", (52, 40)),
        ];
        let mut pids = None;
        for (line, expected) in lines {
            super::announce(line, "makepad-fab", &mut pids);
            assert_eq!(pids, Some(expected), "the app and run pids after {line}");
        }
        // a JIT of an older tree prints the old startup form: no pid is read from it, and the
        // session says why once the process has been quiet past its bound
        let mut old_form = None;
        super::announce("makepad-remote: pid 41 app makepad-fab run 40 socket /t/a", "makepad-fab", &mut old_form);
        assert_eq!(old_form, None, "the old startup line names no pid");
        let why = super::no_startup_line(0, true, false, Some(200_000), 120_000, "fab").expect("quiet past its bound with output and no pid");
        assert!(why.starts_with("no startup line read from the JIT") && why.contains("devhub lanes merge rc-fab"), "{why}");
        for (pid, wrote, quiet, what) in [(7, true, Some(200_000), "a pid was read"), (0, false, Some(200_000), "no output yet"), (0, true, Some(1_000), "still writing: compiling or booting"), (0, true, None, "not running")] {
            assert_eq!(super::no_startup_line(pid, wrote, false, quiet, 120_000, "fab"), None, "{what}");
        }
        // A run whose box's cargo builds its JIT is busy, not silent, past any bound; the bound
        // starts as the build ends (W21875: its 2m45s build was failed at 2 min, then its JIT
        // started and passed 36 of 36, 11-26). Its lines as the run wrote them:
        let mut building = false;
        for line in ["L1 n1: cargo builds this tree's JIT first; the command runs on it once built", "     Locking 58 packages to highest compatible versions"] {
            super::jit_build_line(line, &mut building);
            assert!(building, "cargo builds the JIT after {line:?}");
        }
        assert_eq!(super::no_startup_line(0, true, building, Some(165_000), 120_000, "lamgap"), None, "a JIT build that outlasts the margin fails nothing");
        for line in ["    Finished `host` profile [optimized] target(s) in 2m 45s", "devhub: cargo built this tree's JIT in 166 s"] { super::jit_build_line(line, &mut building); }
        assert!(!building, "the build ended");
        assert!(super::no_startup_line(0, true, building, Some(120_000), 120_000, "lamgap").is_some(), "quiet past its bound after the build: said");
        // Before its lease's run starts the session waits for its JIT, whatever that takes and
        // whatever its pending lines say: a dev JIT's store build says itself only once the run
        // starts, and W22551 was failed at 2 min of its 192 s build (11-26)
        assert!(!super::startup_due(None, true, false, false), "its run not started: its JIT is still resolved or built");
        assert!(super::startup_due(None, true, true, false), "started, with output and no pid: its silence counts");
        assert!(!super::startup_due(Some((7, 7)), true, true, false) && !super::startup_due(None, false, true, false) && !super::startup_due(None, true, true, true), "a pid read, no output yet, or a JIT build in its output");
        // Its startup line read late withdraws the failure: the verdict it failed is pending again
        // as it was, so its run's verdict answers and a retry waits for that; a verdict failed for
        // anything else stays failed
        for pending in ["initial", "restart", ""] {
            let mut p = crate::state::LivePatch { outcome: pending.into(), edit_ms: 5, ..Default::default() };
            super::no_startup_failed(&mut p, why.clone());
            assert!(p.outcome == "failed" && super::terminal(&p) && p.reason == why, "the missing line fails the call: {p:?}");
            let mut s = crate::state::Live { note: why.clone(), patch: Some(p), ..Default::default() };
            assert!(super::startup_read(&mut s), "{pending:?}: withdrawn");
            let p = s.patch.as_ref().unwrap();
            assert!(s.note.is_empty() && p.outcome == pending && !super::terminal(p) && p.reason.is_empty() && p.verdict_ms == 0 && p.startup_outcome.is_none(), "{pending:?} pending again: {s:?}");
        }
        let failed = crate::state::LivePatch { outcome: "failed".into(), reason: "a test failed".into(), ..Default::default() };
        let mut s = crate::state::Live { patch: Some(failed.clone()), ..Default::default() };
        assert!(!super::startup_read(&mut s) && s.patch == Some(failed), "another failure stays");
        // a run followed from where its log stands, its pid lost: read back from this run's part
        // of the log (W12160, 11-17), never from an earlier run's
        let log = std::env::temp_dir().join(format!("devhub-lost-pid-{}.log", std::process::id()));
        let old = "[makepad-remote] pid=11 app=makepad-fab socket=/t/a\n";
        let current = lines.map(|(line, _)| line).join("\n");
        std::fs::write(&log, format!("{old}{current}\nrelax live: restart the app\n")).unwrap();
        let x = crate::state::Live { app: "makepad-fab".into(), log: log.to_string_lossy().into(), run_n: 2, run_log_start: old.len() as u64, ..Default::default() };
        assert_eq!(super::lost_pid(&x, Some((2, 300))), Some((52, 40)), "the newest announce of the run it follows");
        assert_eq!(super::lost_pid(&crate::state::Live { pid: 52, run_pid: 52, ..x.clone() }, Some((2, 300))), Some((52, 40)), "a parent logged after the app pid was saved");
        assert_eq!(super::lost_pid(&crate::state::Live { run_log_start: 0, ..x.clone() }, Some((2, 300))), Some((52, 40)));
        assert_eq!(super::lost_pid(&crate::state::Live { pid: 7, ..x.clone() }, Some((2, 300))), None, "a pid it has stays");
        assert_eq!(super::lost_pid(&x, Some((2, 0))), None, "followed from its start, the run says its announce again");
        assert_eq!(super::lost_pid(&x, Some((3, 0))), None, "a new run announces itself");
        assert_eq!(super::lost_pid(&x, None), None, "a run asked anew too");
        // and whether cargo still builds its JIT there: a master restarted mid-build reads it on
        let build = format!("{old}L1 n1: cargo builds this tree's JIT first; the command runs on it once built\n   Compiling relax-compiler v0.1.0\n");
        std::fs::write(&log, &build).unwrap();
        assert!(super::building_in_log(&x, Some((2, 300))), "a run followed mid-build is building");
        assert!(!super::building_in_log(&x, None), "a run asked anew reads its own output");
        std::fs::write(&log, format!("{build}devhub: cargo built this tree's JIT in 166 s\n")).unwrap();
        assert!(!super::building_in_log(&x, Some((2, 300))), "built: its bound runs");
        let _ = std::fs::remove_file(&log);
    }
}

#[cfg(test)]
mod look_tests {
    use crate::state::Live;

    /// A look on a session parked for another ask's RAM answered no frame and no word, and every
    /// look of a lane's proof after the park came back empty (E3D, 11-15): a look that finds no
    /// running session fails, saying how it ended and how to start it again.
    #[test]
    fn a_look_at_a_parked_session_says_parked_never_an_empty_frame() {
        let x = |state: &str, note: &str| Live { id: "W1".into(), state: state.into(), note: note.into(), ..Default::default() };
        let parked = super::no_frame(&x("ended", "parked for L9 (test of rc-a): it waited for 8.0 GB of RAM on n1; `devhub live again W1` starts it again"));
        assert!(parked.starts_with("W1 ended: parked for L9"), "a parked session says it was parked, and for what: {parked}");
        assert_eq!(parked.matches("live again W1").count(), 1, "it names the command that starts it again, once: {parked}");
        let bare = super::no_frame(&x("ended", ""));
        assert!(bare.starts_with("W1 ended") && bare.contains("`devhub live again W1` starts it again"), "an end with no note still names the way back: {bare}");
        let starting = super::no_frame(&x("starting", "placed again: waits for n2"));
        assert!(starting.contains("is starting: placed again: waits for n2") && starting.contains("no frame yet"), "a session not up yet says what it waits on: {starting}");
        // a wait for the frame of an app whose sources do not build ends at once, with the error: a
        // lane's wait for its Town's first frame sat 21 min on a failed build and a refused save
        // (WORKBOARD, 11-27); a build on its way, or one that patched, waits on
        let error = "/n/src/apps/town/src/main.rs:1216:17: E0027: pattern does not mention field `reason`";
        let built = |outcome: &str, reason: &str, edit: &str| super::unbuilt(&Live { patch: Some(crate::state::LivePatch { outcome: outcome.into(), reason: reason.into(), edit: edit.into(), ..Default::default() }), ..x("running", "") });
        assert_eq!(built("refused", &format!("{error}\nmore"), ""), Some(format!("W1 draws no frame before its next save: its sources do not build: {error}")), "a refused build names its first error");
        assert!(built("refused", "", &format!("its app check found 1 error: {error}")).is_some_and(|w| w.ends_with(&format!("its app check found 1 error: {error}"))), "a refusal kept with no reason says its edit's");
        assert!(built("compile_error", error, "").is_some(), "a compile error too");
        for outcome in ["patched", "shipped", "initial", ""] { assert_eq!(built(outcome, "", ""), None, "{outcome:?}: it may draw yet"); }
        assert_eq!(super::unbuilt(&x("running", "")), None, "no build yet");
        // the call's wait for its box after a master restart ends at the wait's end
        let now = std::time::Instant::now();
        let app = Live { pid: 7, ..x("running", "") };
        assert!(super::waits_on_box(Some(&app), false, None, now) && super::waits_on_box(Some(&app), false, Some(now + std::time::Duration::from_secs(1)), now), "before its end it waits");
        assert!(!super::waits_on_box(Some(&app), false, Some(now), now), "at the wait's end it stops waiting for the box");
        assert!(!super::waits_on_box(Some(&app), true, None, now) && !super::waits_on_box(None, false, None, now) && !super::waits_on_box(Some(&x("running", "")), false, None, now), "a gone client, an unknown session, one with no app: at once");
    }
}

#[cfg(test)]
mod verdict_recovery_tests {
    use super::*;
    use std::{cell::RefCell, fs};

    struct Fixture(Ctx);

    impl Fixture {
        fn new(id: &str) -> Self {
            let home = std::env::temp_dir().join(format!("devhub-verdict-{id}-{}", std::process::id()));
            fs::create_dir_all(&home).unwrap();
            let store = crate::state::Store::open_quiet(&home, "box-a").unwrap();
            let leases = Arc::new(crate::lease::master::LeaseMaster::new("", home.clone(),
                Arc::new(|_, _, _| Err("no box".into())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
            Self(Ctx { store, leases, monitor: crate::monitor::Monitor::start(&[]), machine: "box-a".into(), repo: home.clone(), home })
        }

        fn save(&self, x: Live) -> Live {
            let id = x.id.clone();
            self.0.store.update(|d| { d.lives.retain(|s| s.id != id); d.lives.push(x); }).unwrap();
            get(&self.0, &id).unwrap()
        }

        fn rounds(&self, id: &str, missing: &str, ms: &str) -> Live {
            let mut x = session(id);
            x.log = self.0.home.join("run.log").to_str().unwrap().into();
            let first = format!("running 3 tests\ntest a::one ... ok\ntest a::two - should panic ... ok <0.012s>\ntest {missing} ... ok\ntest result: ok. 3 passed\ntestverdict group=0 ran=3 passed=3 failed=0 ms={ms}\n");
            x.run_log_start = first.len() as u64;
            fs::write(&x.log, format!("{first}relax live: restarting (changes)\nrunning 3 tests\ntest a::one ... ok\nrunning 1 test\ntest a::two - should panic ... FAILED <0.123s>\n")).unwrap();
            x
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0.home); }
    }

    fn session(id: &str) -> Live {
        Live { id: id.into(), node: "box-a".into(), test: true, state: "running".into(), pid: 77,
            patch: Some(LivePatch { edit_ms: 10, shipped_ms: 20, seq: 5, ..Default::default() }), ..Default::default() }
    }

    fn json(line: &str) -> Value { makepad_strict_json::parse(line.as_bytes()).unwrap() }

    struct RemoveRunning<'a>(&'a str);
    impl Drop for RemoveRunning<'_> {
        fn drop(&mut self) {
            RUNNING.lock().unwrap().as_mut().unwrap().remove(self.0);
        }
    }

    struct EndOnDrop<'a>(&'a Ctx, &'a str);
    impl Drop for EndOnDrop<'_> {
        fn drop(&mut self) {
            set(self.0, self.1, |s| s.state = "ended".into());
        }
    }

    #[test]
    fn warm_restart_blocks_supersede_and_keeps_inflight_until_replacement_starts() {
        let f = Fixture::new("restart-race-a");
        let x = f.save(Live { lease: "L-vr-ra".into(), tree: f.0.home.to_string_lossy().into_owned(), ..session("WVRRestartA") });
        let r = take_back(&f.0, &x);
        let _remove = RemoveRunning(&x.id);
        let caller = Waiting::on(r.clone());
        caller.bind_active();
        r.restart.store(true, SeqCst);
        let signal = f.0.store.subscribe("live", &[&x.id]);
        std::thread::scope(|scope| {
            let _end = EndOnDrop(&f.0, &x.id);
            let waiter = scope.spawn(|| wait_tests_with(&f.0, &x.id, || true, Duration::from_millis(1)));
            loop {
                let seen = signal.generation();
                if get(&f.0, &x.id).unwrap().left_ms > 0 { break; }
                signal.wait(seen, Duration::from_millis(1));
            }
            assert_eq!(r.waiting.load(SeqCst), 0);
            assert!(!try_supersede_orphan(&r, true, true), "a pending restart protects the old pid");
            assert!(!try_supersede_running(&f.0, &x.id, &r, true, true));
            assert!(!watch_superseded(&r));
            let assert_waiting = || {
                f.0.store.notify("live", &x.id);
                std::thread::sleep(Duration::from_millis(15));
                assert!(!can_yield_superseded(&f.0, &x.id, &r));
                assert!(!waiter.is_finished(), "the disconnected request must still own inflight");
            };
            r.supersede.store(true, SeqCst);
            assert_waiting();

            set(&f.0, &x.id, |s| s.pid = 0);
            assert!(take_restart(&r));
            assert!(!r.restart.load(SeqCst) && !watch_superseded(&r));
            assert!(!try_supersede_running(&f.0, &x.id, &r, true, true), "the replacement has no pid yet");
            r.supersede.store(true, SeqCst);
            assert_waiting();
            r.supersede.store(false, SeqCst);

            r.resume_active.store(true, SeqCst);
            set(&f.0, &x.id, |s| s.pid = 88);
            assert!(try_supersede_running(&f.0, &x.id, &r, true, false) && watch_superseded(&r));
            assert_waiting();
            r.resume_active.store(false, SeqCst);
            f.0.store.notify("live", &x.id);
            assert!(can_yield_superseded(&f.0, &x.id, &r));
            waiter.join().unwrap();
            assert!(r.inflight.load(SeqCst), "the request releases inflight after the wait returns");
            assert_eq!(r.waiting.load(SeqCst), 0);
        });
    }

    #[test]
    fn restart_clears_stale_supersede_so_replacement_resume_tests_does_not_abort() {
        let f = Fixture::new("restart-race-b");
        let x = f.save(Live { lease: "L-vr-rb".into(), tree: f.0.home.to_string_lossy().into_owned(), ..session("WVRRestartB") });
        let r = take_back(&f.0, &x);
        let _remove = RemoveRunning(&x.id);
        assert!(try_supersede_orphan(&r, true, false));
        finish_call(&f.0, &x.id, &r);
        assert!(!r.inflight.load(SeqCst) && watch_superseded(&r));
        assert!(!take_restart(&r) && watch_superseded(&r), "an ordinary exit preserves the new caller's request");
        assert!(begin_call(&r));
        r.resuming.store(true, SeqCst);
        r.restart.store(true, SeqCst);
        assert!(take_restart(&r));
        assert!(!r.restart.load(SeqCst));
        assert!(!watch_superseded(&r), "a restart must clear the disconnected caller's supersede");
        set(&f.0, &x.id, |s| s.patch.as_mut().unwrap().outcome = "recovering".into());
        resume_tests(&f.0, &x.id, &r, None);
        assert!(r.resuming.load(SeqCst) && r.inflight.load(SeqCst), "recovery keeps ownership");
        r.supersede.store(true, SeqCst);
        resume_tests(&f.0, &x.id, &r, None);
        assert!(!r.resuming.load(SeqCst) && !r.inflight.load(SeqCst), "a new supersede still aborts recovery");
    }

    #[derive(Default)]
    struct ProbeGate(Mutex<bool>, std::sync::Condvar);
    impl ProbeGate {
        fn open(&self) {
            *self.0.lock().unwrap() = true;
            self.1.notify_all();
        }

        fn wait(&self, timeout: Duration) -> bool {
            let (open, _) = self.1.wait_timeout_while(self.0.lock().unwrap(), timeout, |open| !*open).unwrap();
            *open
        }
    }

    struct OpenGateOnDrop<'a>(&'a ProbeGate);
    impl Drop for OpenGateOnDrop<'_> {
        fn drop(&mut self) { self.0.open(); }
    }

    fn probe_row(name: &str, state: &str, cpu_ms: u64, waits: &str) -> RunningThread {
        RunningThread { name: name.into(), state: state.into(), cpu_ms, waits: waits.into() }
    }

    #[test]
    fn slow_thread_probe_never_stalls_live_list_past_budget_and_deduplicates_single_flight() {
        use std::sync::atomic::AtomicUsize;
        let gate = Arc::new(ProbeGate::default());
        let slow_calls = Arc::new(AtomicUsize::new(0));
        let slow_done = Arc::new(AtomicUsize::new(0));
        let fast_calls = Arc::new(AtomicUsize::new(0));
        let probe = {
            let (gate, slow_calls, slow_done, fast_calls) = (gate.clone(), slow_calls.clone(), slow_done.clone(), fast_calls.clone());
            Arc::new(move |node: &str, pid: u32| match (node, pid) {
                ("box-warmup", 1) => Vec::new(),
                ("box-fast", 20) => {
                    fast_calls.fetch_add(1, SeqCst);
                    vec![probe_row("suite::fast_test", "running", 9, "")]
                }
                ("box-slow", 10) | ("box-slow-2", 11) => {
                    slow_calls.fetch_add(1, SeqCst);
                    gate.wait(Duration::from_millis(400));
                    slow_done.fetch_add(1, SeqCst);
                    vec![probe_row("suite::slow_test", "waiting", 5, "futex")]
                }
                _ => panic!("unexpected probe {node}:{pid}"),
            })
        };
        let cache = ThreadCache::new(THREAD_PROBE_WORKERS, Duration::from_secs(5), Duration::from_secs(10), probe);
        let _open = OpenGateOnDrop(&gate);
        assert!(cache.lookup_one("box-warmup", 1, Duration::from_secs(2), false).is_empty());
        let keys = [("box-slow".into(), 10), ("box-slow-2".into(), 11), ("box-fast".into(), 20)];
        let wait = Duration::from_millis(60);
        let start = Instant::now();
        let rows = cache.lookup_batch(&keys, wait, false);
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(350) && slow_done.load(SeqCst) == 0, "cold batch took {elapsed:?}, rows {rows:?}");
        assert!(rows[&keys[0]].is_empty() && rows[&keys[1]].is_empty());
        assert_eq!(rows[&keys[2]], [probe_row("suite::fast_test", "running", 9, "")]);
        let progress = RunProgress { current: Some(RoundLog { total: 3, ..Default::default() }), ..Default::default() };
        for key in [&keys[0], &keys[2]] {
            let x = Live { node: key.0.clone(), pid: key.1, ..session("WVRProbe") };
            let value = progress_value_with(&x, &progress, &rows[key], Some(crate::state::QUIET_SAID_MS), 60_020);
            let summary = value.get("no_progress").unwrap().as_str().unwrap();
            assert!(summary.contains("0/3 finished"), "{summary}");
            let threads = value.get("running_tests").unwrap().get("threads").unwrap().as_arr().unwrap();
            if key == &keys[0] {
                assert!(threads.is_empty() && !summary.contains("still running thread"), "{summary}");
            } else {
                assert!(threads.len() == 1 && summary.contains("still running thread suite::fast_test [running]"), "{summary}");
            }
        }
        let start = Instant::now();
        for _ in 0..5 {
            let again = cache.lookup_batch(&keys[..2], wait, false);
            assert!(again[&keys[0]].is_empty() && again[&keys[1]].is_empty());
        }
        assert!(start.elapsed() < Duration::from_millis(200), "follow-up looks must share the first deadline");
        assert_eq!(slow_done.load(SeqCst), 0);
        assert!(slow_calls.load(SeqCst) <= 2);
        gate.open();
        for (node, pid) in &keys[..2] {
            assert_eq!(cache.lookup_one(node, *pid, Duration::from_secs(2), false), [probe_row("suite::slow_test", "waiting", 5, "futex")]);
        }
        assert_eq!(slow_calls.load(SeqCst), 2);
        assert_eq!(fast_calls.load(SeqCst), 1);
    }

    #[test]
    fn thread_cache_stamps_entry_on_completion_and_serves_stale_while_revalidating() {
        use std::sync::atomic::AtomicUsize;
        let gate = Arc::new(ProbeGate::default());
        let started = Arc::new(ProbeGate::default());
        let done = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = {
            let (gate, started, done, calls) = (gate.clone(), started.clone(), done.clone(), calls.clone());
            Arc::new(move |_: &str, _: u32| match calls.fetch_add(1, SeqCst) {
                0 => {
                    std::thread::sleep(Duration::from_millis(250));
                    vec![probe_row("suite::round1", "waiting", 1, "")]
                }
                1 => {
                    started.open();
                    gate.wait(Duration::from_millis(400));
                    done.store(true, SeqCst);
                    vec![probe_row("suite::round2", "waiting", 2, "")]
                }
                _ => panic!("duplicate cache refresh"),
            })
        };
        let cache = ThreadCache::new(2, Duration::from_millis(200), Duration::from_secs(10), probe);
        let _open = OpenGateOnDrop(&gate);
        let look = |wait, stale| cache.lookup_one("box-a", 42, wait, stale);
        let round1 = [probe_row("suite::round1", "waiting", 1, "")];
        assert_eq!(look(Duration::from_secs(2), false), round1);
        assert_eq!(calls.load(SeqCst), 1);
        let start = Instant::now();
        let second = look(Duration::from_millis(500), false);
        assert_eq!(calls.load(SeqCst), 1, "immediate lookup re-probed, returning {second:?} after {:?}", start.elapsed());
        assert_eq!(second, round1);
        assert!(start.elapsed() < Duration::from_millis(250));
        std::thread::sleep(Duration::from_millis(210));
        let start = Instant::now();
        assert_eq!(look(Duration::from_millis(500), false), round1);
        assert!(start.elapsed() < Duration::from_millis(250) && !done.load(SeqCst), "serve stale rows while refreshing");
        assert!(started.wait(Duration::from_secs(2)), "the stale list lookup starts the refresh");
        assert_eq!(calls.load(SeqCst), 2);
        gate.open();
        assert_eq!(look(Duration::from_secs(2), true), [probe_row("suite::round2", "waiting", 2, "")]);
        assert_eq!(calls.load(SeqCst), 2);
    }

    #[test]
    fn mid_suite_child_running_banners_and_child_results_do_not_reset_an_outer_round() {
        let f = Fixture::new("children");
        let path = f.0.home.join("rounds.log");
        let log = path.to_str().unwrap();
        let old = "running 5 tests\ntest old ... ok\n";
        fs::write(&path, old).unwrap();
        assert!(log_progress(log, old.len() as u64).current.is_none(), "the new process has not written yet");
        fs::write(&path, format!("{old}running 3 tests\ntest one ... ok\n     Running unittests src/lib.rs\nrunning 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out\ntest two ... ok\n")).unwrap();
        let current = log_progress(log, old.len() as u64).current.expect("the outer suite is still running");
        assert_eq!(current.total, 3);
        assert_eq!(current.passed, ["one", "two"]);
    }

    #[test]
    fn an_exited_runs_log_recovered_verdict_passes_and_fails_honestly_after_finished() {
        let f = Fixture::new("exit");
        let mut x = session("WVRExited");
        x.lease = "L-vr-exit".into();
        x.tree = f.0.home.to_str().unwrap().into();
        for fails in [false, true] {
            let p = x.patch.as_mut().unwrap();
            p.outcome = "recovered".into();
            p.reason = "recovered from run log after the test process exited".into();
            p.tests = Some(crate::state::LiveTests { ran: 1, passed: u64::from(!fails), failed: u64::from(fails),
                failures: if fails { vec!["suite::broken".into()] } else { Vec::new() }, ..Default::default() });
            f.save(x.clone());
            let r = take_back(&f.0, &x);
            finished(&f.0, &x.id, &r, Ok(i32::from(fails)));
            let mut ended = get(&f.0, &x.id).unwrap();
            assert_eq!(ended.state, "ended");
            let p = ended.patch.as_ref().unwrap();
            assert!(p.outcome == "recovered" && p.tests.is_some());
            assert_eq!(ended.test_code(), i32::from(fails), "an exited run answers with its recovered verdict");
            let shown = ended.test_result(1.0);
            assert!(shown.contains("recovered from run log") && shown.contains(if fails { "suite::broken" } else { "1/1 ran" }), "{shown}");
            ended.patch.as_mut().unwrap().outcome = "patched".into();
            assert!(ended.test_code() == 1 && ended.test_result(1.0).contains("no verdict on this source state"));
        }
    }

    #[test]
    fn warm_restart_wait_tests_detects_client_disconnect_and_yields_after_watcher_exits() {
        let f = Fixture::new("disconnect");
        let mut x = session("WVRDisconnect");
        x.lease = "L-vr-disconnect".into();
        x.tree = f.0.home.to_str().unwrap().into();
        f.save(x.clone());
        let r = take_back(&f.0, &x);
        let caller = Waiting::on(r.clone());
        caller.bind_active();
        r.resume_active.store(true, SeqCst);
        let signal = f.0.store.subscribe("live", &[&x.id]);
        let (released, superseded, stayed) = std::thread::scope(|scope| {
            let waiter = scope.spawn(|| wait_tests_with(&f.0, &x.id, || true, Duration::from_millis(1)));
            loop {
                let seen = signal.generation();
                if get(&f.0, &x.id).unwrap().left_ms > 0 { break; }
                signal.wait(seen, Duration::from_millis(1));
            }
            let released = r.waiting.load(SeqCst) == 0;
            let superseded = try_supersede_orphan(&r, true, false);
            f.0.store.notify("live", &x.id);
            let stayed = !waiter.is_finished();
            r.resume_active.store(false, SeqCst);
            f.0.store.notify("live", &x.id);
            if !superseded { set(&f.0, &x.id, |s| s.state = "ended".into()); }
            waiter.join().unwrap();
            (released, superseded, stayed)
        });
        RUNNING.lock().unwrap().as_mut().unwrap().remove(&x.id);
        drop(caller);
        assert!(released, "the disconnected request releases its wait token during warm restart");
        assert!(superseded && stayed && r.inflight.load(SeqCst), "the request owns inflight until the watcher exits");
        assert_eq!(r.waiting.load(SeqCst), 0);
    }

    #[test]
    fn a_verdict_that_finished_during_a_master_restart_is_delivered_from_events() {
        let x = session("WVREvents");
        assert!(matches!(cut_by_restart(&x, 50, true), Some(Cut::RecoverTest)), "follow the shipped test edit for its verdict");
        let events = [
            json(r#"{"seq":6,"event":"patchverified","group":2}"#),
            json(r#"{"seq":7,"event":"testverdict","group":2,"ran":40,"passed":39,"failed":1,"failures":["a::b"],"ms":1200.0}"#),
        ];
        let probe = probe_cut_events(&events, &[], "", x.patch.as_ref().unwrap(), false);
        let CutProbe::Finished { patch: Some(patch), verdict } = probe else { panic!("missing finished verdict: {probe:?}") };
        assert_eq!(patch, events[0]);
        let t = crate::state::live_tests_from(&verdict).unwrap();
        assert_eq!((t.ran, t.passed, t.failed, t.ms), (40, 39, 1, 1200.0));
        assert_eq!(t.failures, ["a::b"]);
    }

    #[test]
    fn a_verdict_that_finished_during_a_master_restart_is_delivered_from_run_log_when_events_wrapped_or_ended() {
        let p = session("WVRLog").patch.unwrap();
        let log = "running 10 tests\ntest foo::bar ... FAILED\ntest result: FAILED\ntestverdict group=2 ran=10 passed=9 failed=1 ms=4321.0 selected=all skipped=0 borrowck=clean";
        let probe = probe_cut_events(&[], &[], log, &p, false);
        let CutProbe::FromLog(t) = probe else { panic!("missing verdict from the run log: {probe:?}") };
        assert_eq!((t.ran, t.passed, t.failed, t.ms), (10, 9, 1, 4321.0));
        assert_eq!(t.failures, ["foo::bar"]);
        assert!(matches!(probe_cut_events(&[], &[], &format!("{log} reverted=[1]"), &p, false), CutProbe::Lost(_)), "reverted code cannot answer this edit");
    }

    #[test]
    fn a_verdict_that_cannot_be_recovered_after_restart_says_lost_why_and_what_to_do() {
        let mut x = session("WVRLost");
        for (shipped, why, reruns) in [(0, CUT_LOST_UNSHIPPED, false), (20, CUT_LOST_NO_RUN, true)] {
            x.patch.as_mut().unwrap().shipped_ms = shipped;
            let Some(Cut::Said(p)) = cut_by_restart(&x, 50, false) else { panic!("the caller needs the lost verdict") };
            assert!(terminal(&p));
            assert_eq!(verdict(&p), format!("{why}\n"), "say why this verdict was lost and what the next call does");
            assert_eq!(rerun_cut(&p), reruns);
        }
        let p = x.patch.as_ref().unwrap();
        let CutProbe::Lost(why) = probe_cut_events(&[], &[r#"{"editing":0}"#.into()], "", p, false) else { panic!("the idle run has no verdict") };
        assert!(why.contains("idle") && why.contains("verdict"), "{why}");
        let cut = lost_verdict(p, 50, format!("verdict lost: {why}; {NEXT_CALL_RERUNS}"));
        assert!(verdict(&cut).starts_with("verdict lost:") && rerun_cut(&cut));
        assert_eq!((cut.outcome.as_str(), cut.verdict_ms), ("failed", 50));
        x.patch = Some(cut);
        assert_eq!(x.test_code(), 2);
    }

    fn spilled(f: &Fixture, why: &str, shipped_ms: u64) -> Live {
        let mut x = session("WVRSpilled");
        x.patch = Some(LivePatch { reason: why.into(), outcome: "failed".into(), shipped_ms, retried: true, ..x.patch.take().unwrap() });
        let x = f.save(x);
        let p = x.patch.as_ref().unwrap();
        assert!(p.reason.is_empty() && p.reason_ref.len > 0, "the store must spill the reason");
        x
    }

    #[test]
    fn rerun_cut_still_triggers_after_store_spills_patch_reason_to_reason_ref() {
        let f = Fixture::new("spill");
        for (why, shipped, rerun) in [(CUT_RERUN, 20, true), (CUT_LOST_NO_RUN, 20, true), ("3 tests failed", 20, false), (CUT_LOST_UNSHIPPED, 0, false)] {
            let x = spilled(&f, why, shipped);
            assert_eq!(rerun_cut(x.patch.as_ref().unwrap()), rerun, "rerun decision after spilling {why}");
        }
    }

    #[test]
    fn next_edit_after_cut_ships_new_saves_before_single_warm_restart() {
        let f = Fixture::new("publish");
        let x = spilled(&f, CUT_RERUN, 20);
        let files = vec!["changed.rs".to_string()];
        fs::write(f.0.home.join(&files[0]), "new bytes").unwrap();
        let snapshot = f.0.home.join("overlay");
        let trace = RefCell::new(Vec::new());
        let (before, result) = run_test_call_step(x, false, None,
            |x| {
                let sources = capture(&f.0.home, &files, &[], &x.sources, &snapshot)?;
                try_set(&f.0, &x.id, |s| s.sources = sources)?;
                trace.borrow_mut().push("publish");
                Ok((get(&f.0, &x.id).unwrap(), LivePatch { files: files.clone(), edit_ms: 30, ..Default::default() }))
            },
            |x, p| {
                assert_eq!(fs::read(snapshot.join(&files[0])).unwrap(), b"new bytes");
                assert!(x.sources.contains_key(&files[0]) && !p.retried);
                assert_eq!(p.files, files);
                trace.borrow_mut().push("restart");
            },
            || { trace.borrow_mut().push("wait"); Ok(get(&f.0, "WVRSpilled").unwrap()) },
            |_| { trace.borrow_mut().push("ship_edit"); Ok(Ok(None)) },
        ).unwrap();
        result.unwrap();
        assert_eq!(*trace.borrow(), ["publish", "restart", "wait"], "publish this edit before the single warm restart");
        assert!(before.is_none(), "the restarted run has its own fresh verdict");
    }

    #[test]
    fn next_edit_supersedes_orphaned_hung_test_run_without_interrupting_active_caller() {
        let f = Fixture::new("orphan");
        let mut x = session("WVROrphan");
        x.lease = "L-vr-orphan".into();
        x.tree = f.0.home.to_str().unwrap().into();
        let x = f.save(x);
        let r = take_back(&f.0, &x);
        RUNNING.lock().unwrap().as_mut().unwrap().remove(&x.id);
        let caller = Waiting::on(r.clone());
        caller.bind_active();
        assert!(!try_supersede_orphan(&r, false, true) && !watch_superseded(&r), "a connected caller keeps its run");
        drop(caller);
        assert!(!try_supersede_orphan(&r, false, false) && !watch_superseded(&r), "a reconnect waits for the same bytes");
        let caller = Waiting::on(r.clone());
        caller.bind_active();
        r.release_active_wait();
        assert!(try_supersede_orphan(&r, false, true) && watch_superseded(&r), "a new save must supersede the disconnected caller's hung run");
        assert!(r.supersede.load(SeqCst));
        assert_eq!(get(&f.0, &x.id).unwrap().patch, x.patch, "the old watcher leaves its patch alone");
        drop(caller);
        assert_eq!(r.waiting.load(SeqCst), 0, "disconnect and drop release the caller only once");
        r.supersede.store(false, SeqCst);
        assert!(try_supersede_orphan(&r, true, false) && watch_superseded(&r), "fresh also supersedes an orphan");
    }

    #[test]
    fn ending_a_session_settles_open_verdict_with_tests_that_did_finish() {
        let f = Fixture::new("ends");
        let base = f.rounds("WVREnd", "a::three", "5000.0");
        for end in ["release", "park", "finished"] {
            let mut x = base.clone();
            x.id.push_str(end);
            x.lease = format!("L-vr-{end}");
            x.tree = f.0.home.to_str().unwrap().into();
            f.save(x.clone());
            match end {
                "release" => { release(&f.0, &x.id, "released").unwrap(); }
                "park" => {
                    let progress = log_progress(&x.log, x.run_log_start);
                    set(&f.0, &x.id, |s| record_park_with(s, "parked", 50, Some(&progress))).unwrap();
                }
                _ => { let r = take_back(&f.0, &x); finished(&f.0, &x.id, &r, Ok(0)); }
            }
            let x = get(&f.0, &x.id).unwrap();
            let p = x.patch.as_ref().unwrap();
            assert_eq!(p.outcome, "ended", "{end} must settle the edit");
            assert!(terminal(p) && p.verdict_ms > 0);
            let shown = render(&x);
            assert!(!shown.contains("verdict pending") && shown.contains("ended before its verdict") && shown.contains("2/3 finished: 1 passed, 1 failed") && shown.contains("a::three"), "{end}: {shown}");
        }
    }

    #[test]
    fn render_never_says_verdict_pending_for_an_ended_session_even_on_historical_records() {
        let f = Fixture::new("historical");
        let mut x = f.rounds("WVRHistory", "a::three", "5000.0");
        x.state = "ended".into();
        x.note = "released".into();
        x.patch.as_mut().unwrap().log_start = fs::metadata(&x.log).unwrap().len();
        let shown = render(&x);
        assert!(!shown.contains("verdict pending"), "historical ended session: {shown}");
        assert!(shown.contains("verdict ended before its verdict (released; 2/3 finished: 1 passed, 1 failed; still running: a::three)"), "{shown}");
    }

    #[test]
    fn running_test_look_names_hung_test_and_elapsed_vs_measured_without_killing_it() {
        let f = Fixture::new("running");
        let mut x = f.rounds("WVRRunning", "master::tests::hung_one", "73503.2");
        let t0 = wall_ms().saturating_sub(192_000);
        let p = x.patch.as_mut().unwrap();
        (p.edit_ms, p.shipped_ms) = (t0, t0);
        let progress = log_progress(&x.log, x.run_log_start);
        let shown = render(&x);
        for detail in ["master::tests::hung_one", "2/3 finished", "3m12s", "1m13s measured"] {
            assert!(shown.contains(detail), "live get is missing {detail}: {shown}");
        }
        let summary = running_test_summary(&x, &progress, &[], Some(185_000), t0 + 192_000).unwrap();
        for detail in ["master::tests::hung_one", "2/3 finished", "3m12s elapsed", "1m13s measured"] {
            assert!(summary.contains(detail), "live list is missing {detail}: {summary}");
        }
        assert_eq!(x.state, "running");
    }

    #[test]
    fn round_one_hung_test_look_uses_program_threads_when_no_prior_round_exists() {
        let f = Fixture::new("threads");
        let mut x = session("WVRThreads");
        x.log = f.0.home.join("first.log").to_str().unwrap().into();
        fs::write(&x.log, "running 4 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n").unwrap();
        let threads = parse_program_threads(&[
            r#"{"role":"jit","name":"worker"}"#.into(), r#"{"role":"program","name":"main"}"#.into(),
            r#"{"role":"program","name":"relax-watcher"}"#.into(),
            r#"{"id":"t5","name":"master::tests::","role":"program","state":"waiting","cpu_ms":12,"waits":"futex"}"#.into(),
        ]);
        assert_eq!(threads, [RunningThread { name: "master::tests::".into(), state: "waiting".into(), cpu_ms: 12, waits: "futex".into() }], "show the program test thread");
        let summary = running_test_summary(&x, &log_progress(&x.log, 0), &threads, Some(185_000), 192_020).unwrap();
        for detail in ["master::tests::", "waiting", "3/4 finished", "3m12s elapsed", "no prior run measured"] { assert!(summary.contains(detail), "{summary}"); }
    }
}

#[cfg(test)]
mod cut_tests {
    use super::{cut_by_restart, Cut};
    use crate::state::{Live, LivePatch};
    use std::time::Duration;

    /// The master restarted between an edit's call and its verdict: the patch kept its call and no
    /// outcome, and every look waited out its bound on it (W7598, W7635, 11-15). On rejoin no
    /// patch is left without an outcome a wait could hang on, and the edit is not lost.
    #[test]
    fn an_edit_cut_by_a_master_restart_ships_again_is_watched_again_or_is_said() {
        let edit = LivePatch { files: vec!["src/a.rs".into()], edit_ms: 10, called_ms: 11, ..Default::default() };
        let app = Live { id: "W1".into(), state: "running".into(), pid: 7, patch: Some(edit.clone()), ..Default::default() };
        let Some(Cut::Ship(cut, files)) = cut_by_restart(&app, 50, false) else { panic!("an app session's edit that never shipped must ship again") };
        assert_eq!(files, edit.files, "the cut edit's own files ship again");
        assert_eq!((cut.edit_ms, cut.called_ms, cut.outcome_ms), (10, 11, 50), "the cut patch keeps its edit and call, and is concluded now");
        assert!(cut.reason.contains("master restarted"), "the outcome says why: {}", cut.reason);
        assert!(super::patched_done(Some(&cut), 10, Duration::from_millis(400)), "a wait for the patch no longer hangs on the cut edit");
        let pending: super::Pending = Default::default();
        super::due(&pending, files);
        assert!(pending.lock().unwrap().0.contains("src/a.rs"), "its files are due, so `live wait patched` waits for their new ship, not for the cut one");
        let shipped = Live { patch: Some(LivePatch { shipped_ms: 20, ..edit.clone() }), ..app.clone() };
        assert!(matches!(cut_by_restart(&shipped, 50, false), Some(Cut::Watch)), "an edit the node has is watched for its verdict again, never shipped twice");
        let test = Live { test: true, ..shipped.clone() };
        let Some(Cut::Said(cut)) = cut_by_restart(&test, 50, false) else { panic!("a test session's cut edit must be said") };
        assert!(super::terminal(&cut) && cut.reason.contains("next call"), "`live wait verdict` answers at once and names the call that ships it again: {} {}", cut.outcome, cut.reason);
        // its node has it, so the next call has nothing to ship: that call runs its tests again,
        // where every call answered the cut verdict again until the session ended (W18256, 11-24)
        assert!(super::rerun_cut(&cut), "a shipped test edit's cut is rerun by the next call: {}", cut.reason);
        let Some(Cut::Said(unshipped)) = cut_by_restart(&Live { test: true, ..app.clone() }, 50, false) else { panic!("a test session's cut edit must be said") };
        assert!(!super::rerun_cut(&unshipped) && unshipped.reason.contains("ships its saves again"), "one its node never had ships on the next call: {}", unshipped.reason);
        assert!(!super::rerun_cut(&LivePatch { outcome: "failed".into(), shipped_ms: 20, reason: "3 tests failed".into(), ..Default::default() }), "a failed verdict of the tests is no cut");
        let done = Live { patch: Some(LivePatch { outcome: "patched".into(), ..edit }), ..app.clone() };
        assert!(cut_by_restart(&done, 50, false).is_none(), "an edit with its verdict is left as it is");
        assert!(cut_by_restart(&Live { patch: None, ..app }, 50, false).is_none(), "a session with no edit has nothing cut");
    }
}

#[cfg(test)]
mod dup_launch_tests {
    use super::*;
    use crate::lease::sched::{Req, State};
    use std::sync::mpsc;

    struct Fixture {
        ctx: Arc<Ctx>,
        events: Arc<Mutex<Vec<(Sev, String)>>>,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let home = std::env::temp_dir().join(format!("devhub-live-dup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(home.join(".git")).unwrap();
            let leases = Arc::new(crate::lease::master::LeaseMaster::new("", home.clone(),
                Arc::new(|_, _, _| Err("no box".into())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
            let events = Arc::new(Mutex::new(Vec::new()));
            let heard = events.clone();
            let store = crate::state::Store::open(&home, "", Arc::new(move |r| {
                heard.lock().unwrap().push((r.sev, r.line));
                0
            })).unwrap();
            let ctx = Arc::new(Ctx { store, leases, machine: String::new(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home });
            Self { ctx, events }
        }

        fn request(&self, verb: &str) -> String {
            format!(r#"live {verb} {{"app":"app","lane":"lane","repo_dir":{}}}"#, json_str(self.ctx.repo.to_str().unwrap()))
        }

        fn queue(&self, kind: Kind) -> String {
            let mut req = Req::new("lane", kind);
            req.live = kind == Kind::Test;
            let mut queued = String::new();
            self.ctx.leases.test_sched(|s| queued = s.request(req, 1));
            queued
        }

        fn assert_still_queued(&self, id: &str, queued: &str) {
            assert!(self.ctx.leases.view().iter().any(|l| l.id == queued && l.state == State::Queued), "{queued} must still be queued");
            assert_eq!(ASKED.lock().unwrap().get(id).map(String::as_str), Some(queued), "the first worker still owns its ask");
        }

        fn assert_refused(&self, id: &str, path: &str) {
            let line = format!("live {id}: refused duplicate start from {path} (already owned by serve)");
            assert!(self.events.lock().unwrap().contains(&(Sev::Warn, line.clone())), "missing warning: {line}");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.ctx.home); }
    }

    #[test]
    fn serve_created_session_is_not_relaunched_by_restarted() {
        let f = Fixture::new("serve");
        f.ctx.store.update(|d| d.lives_next = 900_100).unwrap();
        let (launch_tx, launch_rx) = mpsc::channel();
        let ctx = f.ctx.clone();
        let request = f.request("test");
        let caller = std::thread::spawn(move || {
            serve_with(&ctx, &request, &|_| {}, &|_, claim, first_call| {
                assert!(first_call);
                launch_tx.send(claim).unwrap();
                Ok(())
            })
        });
        let claim = launch_rx.recv().unwrap();
        let id = claim.id().to_string();
        let asking = Asking(claim.id());
        let queued = f.queue(Kind::Test);
        asking.names(&queued);
        let before = set(&f.ctx, &id, |s| s.note = format!("waiting for {queued}")).unwrap();
        assert_eq!(before.state, "starting");
        assert!(restarted_to_launch(&f.ctx).is_empty(), "restart must skip serve's pending worker");
        assert_eq!(get(&f.ctx, &id).unwrap(), before, "reconciliation must preserve the pending start");
        f.assert_refused(&id, "restarted");
        f.assert_still_queued(&id, &queued);

        set(&f.ctx, &id, |s| {
            s.state = "running".into();
            s.patch = Some(LivePatch { outcome: "initial".into(),
                tests: Some(crate::state::LiveTests { ran: 1, passed: 1, ..Default::default() }), ..Default::default() });
        }).unwrap();
        let reply = caller.join().unwrap().unwrap();
        let answer = crate::state::live_from(&makepad_strict_json::parse(reply.as_bytes()).unwrap());
        assert_eq!(answer.id, id);
        assert_eq!(answer.test_code(), 0, "the caller receives the first worker's passing verdict");
        set(&f.ctx, &id, |s| { s.state = "ended".into(); s.ended_ms = wall_ms(); }).unwrap();
        drop(asking);
        drop(claim);
        assert_eq!(claimed_by(&id), None);
    }

    #[test]
    fn startup_gate_holds_serve_until_restart_reconciliation_completes() {
        let f = Fixture::new("gate");
        f.ctx.store.update(|d| {
            d.lives_next = 900_200;
            d.lives.push(Live { id: "WDUP-dead".into(), app: "app".into(), lane: "lane".into(),
                state: "running".into(), lease: "L-dead".into(), ..Default::default() });
            d.lives.push(Live { id: "WDUP-start".into(), app: "other".into(), lane: "lane".into(),
                state: "starting".into(), ..Default::default() });
        }).unwrap();

        let gate = StartupGate::arm();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (launch_tx, launch_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let ctx = f.ctx.clone();
        let request = f.request("start");
        let caller = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            let reply = serve_with(&ctx, &request, &|_| {}, &|_, claim, _| {
                launch_tx.send(claim).unwrap();
                Ok(())
            });
            done_tx.send(reply).unwrap();
        });
        entered_rx.recv().unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_millis(30)), Err(mpsc::RecvTimeoutError::Timeout));
        assert!(launch_rx.try_recv().is_err());
        let to_launch = restarted_to_launch(&f.ctx);
        drop(gate);
        let reply = done_rx.recv().unwrap().unwrap();
        caller.join().unwrap();

        assert_eq!(get(&f.ctx, "WDUP-dead").unwrap().state, "ended");
        assert_eq!(to_launch.len(), 1);
        let (x, held, restarted) = &to_launch[0];
        assert_eq!(x.id, "WDUP-start");
        assert!(!held);
        assert_eq!(restarted.id(), x.id);
        assert_eq!(claimed_by(restarted.id()), Some(LaunchPath::Restarted));
        let served = launch_rx.try_recv().unwrap();
        assert_eq!(served.id(), "W900201");
        assert_eq!(crate::state::live_from(&makepad_strict_json::parse(reply.as_bytes()).unwrap()).id, served.id());
        assert_eq!(claimed_by(served.id()), Some(LaunchPath::Serve));
        drop(to_launch);
        drop(served);
    }

    #[test]
    fn second_claim_from_any_path_is_refused_and_names_both_paths() {
        let f = Fixture::new("claim");
        let id = "WDUP-claim";
        let first = Claim::acquire(id, LaunchPath::Serve).unwrap();
        let asking = Asking(id);
        let queued = f.queue(Kind::Test);
        asking.names(&queued);

        assert!(!try_run(&f.ctx, id, false, LaunchPath::Restarted));
        assert!(try_claim_or_warn(&f.ctx, id, LaunchPath::Reattach).is_none());
        assert!(try_claim_or_warn(&f.ctx, id, LaunchPath::ResumeParked).is_none());
        for path in ["restarted", "reattach", "resume_parked"] { f.assert_refused(id, path); }
        f.assert_still_queued(id, &queued);
        drop(asking);
        drop(first);
        let next = Claim::acquire(id, LaunchPath::ResumeParked).unwrap();
        assert_eq!(claimed_by(next.id()), Some(LaunchPath::ResumeParked));
    }

    #[test]
    fn resume_parked_skips_claimed_session_without_flipping_state() {
        let f = Fixture::new("park");
        let id = "WDUP-park";
        let timed = f.queue(Kind::Timed);
        f.ctx.leases.test_sched(|s| assert!(s.release(&timed, "released", 2)));
        let parked = Live { id: id.into(), state: "ended".into(), resume_after: timed, ..Default::default() };
        f.ctx.store.update(|d| d.lives.push(parked.clone())).unwrap();
        let first = Claim::acquire(id, LaunchPath::Serve).unwrap();
        assert!(resume_due_claims(&f.ctx).is_empty());
        assert_eq!(get(&f.ctx, id).unwrap(), parked, "a claimed session stays parked for its timed lease");
        drop(first);

        let due = resume_due_claims(&f.ctx);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id(), id);
        assert_eq!(claimed_by(id), Some(LaunchPath::ResumeParked));
        let resumed = get(&f.ctx, id).unwrap();
        assert_eq!(resumed.state, "starting");
        assert!(resumed.resume_after.is_empty());
    }
}

#[cfg(test)]
mod owner_tests {
    /// A session's lease ends by its own release or its end, and by nothing else. `live release
    /// <id>` frees the lease the store records though no run follows it (a restart had not taken
    /// W9068 back, and its release freed nothing and said nothing, 11-15). A release by the lease's
    /// id is refused while its session runs, naming the session's own (an agent released the lease a
    /// frame sequence printed, and W9349 ended, 11-15). A master's start frees an ended session's
    /// lease and ends a running one whose lease is gone, a line each, once.
    #[test]
    fn a_sessions_lease_ends_by_its_own_release_or_its_end() {
        use super::*;
        use crate::lease::sched::{Req, State};
        let home = std::env::temp_dir().join(format!("devhub-live-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let leases = Arc::new(crate::lease::master::LeaseMaster::new("", home.clone(), Arc::new(|_: &str, service: &str, line: &str| match (service, line.split_whitespace().nth(1)) {
            ("lease-end", Some(id)) => Ok(vec![format!("{{\"ended\":{},\"leak\":[]}}", json_str(id))]),
            _ => Err(format!("no {service} here")),
        }), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let heard = said.clone();
        let store = crate::state::Store::open(&home, "", Arc::new(move |r: Record| { heard.lock().unwrap().push(r.line); 0 })).unwrap();
        let ctx = Arc::new(Ctx { store, leases, machine: String::new(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home: home.clone() });
        let mut req = Req::new("lane", Kind::Test);
        req.live = true;
        for id in ["L1", "L2", "L3", "L4", "L5"] {
            ctx.leases.test_sched(|s| { s.restore(&format!("{{\"id\":{},\"node\":\"node\",\"req\":{}}}", json_str(id), req.to_json()), 1); });
        }
        let open = |id: &str| ctx.leases.view().iter().any(|l| l.id == id && !matches!(l.state, State::Ended(_)));
        assert!(["L1", "L2", "L3", "L4", "L5"].iter().all(|id| open(id)), "the fixture's leases run");
        let live = |id: &str, lease: &str, state: &str| Live { id: id.into(), app: "app".into(), lane: "lane".into(), lease: lease.into(), state: state.into(), ..Default::default() };
        ctx.store.update(|d| d.lives.extend([live("WR1", "L1", "running"), live("WR2", "L2", "running"), live("WR3", "L4", "ended"), live("WR4", "L6", "running"), live("WR5", "", "starting"), live("WR6", "L5", "paused")])).unwrap();
        let field = |reply: &str, k: &str| makepad_strict_json::parse(reply.as_bytes()).unwrap().get(k).and_then(Value::as_str).unwrap_or("").to_string();

        let refused = |id: &str, lease: &str| Live {
            test: true, pid: 7, run_n: 2, patch: Some(LivePatch { edit_ms: 42, outcome: "compile_error".into(), reason: "error: missing item\nerror: second error".into(), ..Default::default() }),
            ..live(id, lease, "running")
        };
        let calls = Mutex::new(Calls::default());
        let arm = |x: &Live, caller: &str, gone: bool| calls.lock().unwrap().checking(x, caller, x.refusal_key().unwrap(), Box::new(move || gone));
        let mut x = refused("WR1", "L1");
        set(&ctx, "WR1", |s| *s = x.clone());
        arm(&x, "first", false);
        let old = x.refusal_key().unwrap();
        x.run_n += 1;
        set(&ctx, "WR1", |s| s.run_n = x.run_n);
        checked(&ctx, "WR1", &calls, "first", &old, true).unwrap();
        assert!(get(&ctx, "WR1").unwrap().held.is_empty() && open("L1"), "a confirmation for the previous run neither holds nor releases");
        arm(&x, "second", false);
        assert!(calls.lock().unwrap().keeps_refusal(&x), "the cargo caller protects its current refusal from the master's sweeps");
        checked(&ctx, "WR1", &calls, "second", &x.refusal_key().unwrap(), true).unwrap();
        let held = get(&ctx, "WR1").unwrap();
        assert_eq!(held.held, "error: missing item", "only the first compile diagnostic is kept");
        assert!(calls.lock().unwrap().keeps_refusal(&held), "the confirmed refusal survives the sweeps after the client goes");
        // A master restart between the call and its cargo answer loses the armed check: the
        // caller's confirmation still holds, its non-confirmation changes nothing.
        let unarmed = refused("WR-restart", "L-restart");
        ctx.store.update(|d| d.lives.push(unarmed.clone())).unwrap();
        let fresh = Mutex::new(Calls::default());
        checked(&ctx, "WR-restart", &fresh, "after", &unarmed.refusal_key().unwrap(), false).unwrap();
        assert!(get(&ctx, "WR-restart").is_some_and(|x| x.active() && x.held.is_empty()), "an unarmed non-confirmation neither holds nor releases");
        let reply = checked(&ctx, "WR-restart", &fresh, "after", &unarmed.refusal_key().unwrap(), true).unwrap();
        assert_eq!((get(&ctx, "WR-restart").unwrap().held, field(&reply, "held")), ("error: missing item".to_string(), "error: missing item".to_string()), "a confirmation after a master restart holds");

        assert!(render(&held).contains("  held: error: missing item\n"), "live list and live get render the diagnostic");
        assert_eq!(field(&serve(&ctx, "live list held-leases").unwrap(), "L1"), held.held, "the lease diagnostic is keyed by lease ID");
        assert!(!left_long(&Live { left_ms: 1, ..held.clone() }, LEFT_MS + IDLE_MS), "a departed starter does not end a confirmed hold");

        // A held edit needs no second check until the node takes a change. On a publication
        // the node can already have moved to a fresh PID with its own event sequence.
        let refusal_event = r#"{"event":"patchrefused","seq":8,"reasons":["error: missing item"]}"#;
        let mut old_patch = held.patch.clone().unwrap();
        old_patch.seq = 8;
        old_patch.cursor = event_cursor(&makepad_strict_json::parse(refusal_event.as_bytes()).unwrap());
        assert!(!taken_since(&[refusal_event.into()], &old_patch), "unchanged manifest and refusal cursor reuse the held answer");
        let answer = crate::state::live_from(&makepad_strict_json::parse(test_reply(&held, None, "again").as_bytes()).unwrap());
        assert_eq!((answer.test_code(), answer.held.as_str()), (1, "error: missing item"), "the unchanged call answers with the confirmed refusal");
        // a call that ran no round answers its last verdict, marked cached with that verdict's time
        let passed = LivePatch { seq: 3, edit_ms: 10_000, shipped_ms: 11_000, verdict_ms: 12_000, outcome: "patched".into(), tests: Some(Default::default()), answered_ms: 12_500, ..Default::default() };
        let cached_ms = |before: Option<&LivePatch>, after: &LivePatch| makepad_strict_json::parse(cached_reply(test_reply(&held, None, "again"), before, Some(after)).as_bytes()).unwrap().get("cached_ms").and_then(Value::as_u64);
        assert_eq!(cached_ms(Some(&passed), &passed), Some(12_000), "the last round's verdict answers, cached");
        let unanswered = LivePatch { answered_ms: 0, ..passed.clone() };
        assert_eq!(cached_ms(Some(&unanswered), &unanswered), None, "a round no caller heard (its call's link was cut) answers the call that asks again as its own, not cached");
        let saved = crate::state::live_from(&crate::state::live_value(&Live { patch: Some(passed.clone()), ..held.clone() }));
        assert_eq!(saved.patch.map(|p| p.answered_ms), Some(12_500), "a heard round stays heard across the master's restart");
        assert_eq!(cached_ms(Some(&passed), &LivePatch { verdict_ms: 13_000, ..passed.clone() }), None, "a round that ran is no cached verdict");
        assert_eq!(cached_ms(None, &passed), None, "nor is the session's first");
        assert_eq!(cached_ms(Some(&LivePatch { tests: None, ..passed.clone() }), &LivePatch { tests: None, ..passed.clone() }), None, "a held refusal is no test verdict");
        let repaired = Live { id: "WR-edit".into(), lease: "L-edit".into(), ..held.clone() };
        ctx.store.update(|d| d.lives.push(repaired.clone())).unwrap();
        for (failed, expected) in [(0, 0), (1, 1)] {
            let mut watch = set(&ctx, &repaired.id, |s| *s = repaired.clone()).unwrap();
            let mut patch = LivePatch { edit_ms: old_patch.edit_ms + 1, outcome: "restart".into(), seq: old_patch.seq, cursor: old_patch.cursor.clone(), ..Default::default() };
            let id = watch.id.clone();
            set_patch(&ctx, &id, &patch);
            let moved = set(&ctx, &id, |s| s.pid += 1).unwrap();
            let mut events = [false, true].into_iter();
            collect_test_verdict(&ctx, &id, &mut watch, &mut patch, |x, p| {
                assert!(watch_on(x, Some(&moved), p));
                assert_eq!((x.pid, p.seq), (moved.pid, 0), "the watch follows the fast PID change from the new queue's first event");
                let restarted = events.next().expect("the restarted verdict ends the call");
                let event = format!(r#"{{"event":"testverdict","seq":1,"group":0,"ran":1,"passed":{},"failed":{failed},"ms":1,"restarted":{restarted}}}"#, 1-failed);
                makepad_strict_json::parse(event.as_bytes()).ok()
            });
            let saved = set_test_patch(&ctx, &id, &watch, &patch).unwrap();
            assert!(saved.held.is_empty() && saved.patch.as_ref().unwrap().tests.as_ref().unwrap().restarted, "the restarted round supersedes the held refusal");
            assert_eq!((saved.lease.as_str(), saved.test_code()), ("L-edit", expected), "the fixed edit's verdict on the same lease");
        }
        // a run held at its start (the tests did not compile) has that as its verdict at once: a
        // JIT fault among its errors a JIT error, a plain compile error a refusal. Nine sessions
        // waited out their box's idle end on a relax ICE and ended "no lease" (11-27).
        for (reason, outcome, code, says) in [
            ("core.rs:761:5: internal compiler error while checking the borrows of f: MIR bug", "jit_error", 3, "JIT error, not your change: core.rs:761:5: internal compiler error"),
            ("error[E0425]: cannot find value `x`", "refused", 1, "cannot find value `x`"),
        ] {
            let mut watch = set(&ctx, &repaired.id, |s| *s = Live { held: String::new(), ..repaired.clone() }).unwrap();
            let mut patch = LivePatch { edit_ms: old_patch.edit_ms + 2, outcome: "initial".into(), ..Default::default() };
            let id = watch.id.clone();
            set_patch(&ctx, &id, &patch);
            let mut events = std::iter::once(format!(r#"{{"event":"patchrefused","seq":1,"edit":"{HELD_AT_START}","outcome":"refused","reasons":["{reason}"]}}"#));
            collect_test_verdict(&ctx, &id, &mut watch, &mut patch, |_, _| events.next().and_then(|e| makepad_strict_json::parse(e.as_bytes()).ok()));
            assert!(terminal(&patch) && patch.outcome == outcome, "a run held at its start on {reason:?} has its verdict at once, {outcome}: {patch:?}");
            assert!(verdict(&patch).contains(says), "its verdict says what did not compile: {}", verdict(&patch));
            let saved = set_test_patch(&ctx, &id, &watch, &patch).unwrap();
            assert_eq!(saved.test_code(), code, "the client's exit for a held {outcome}");
        }
        // a run held before its watcher exists answers every watch "not a live session": its
        // refusal is read from its event queue, the first of the watched kinds past the cursor
        // (two calls waited 102 min for it, W25685, W25686, 11-27)
        let queue: Vec<Value> = [r#"{"event":"panic","seq":1}"#, r#"{"event":"patchrefused","seq":3,"edit":"the tests did not compile"}"#, r#"{"event":"patchrefused","seq":2,"edit":"the tests did not compile"}"#]
            .iter().map(|e| makepad_strict_json::parse(e.as_bytes()).unwrap()).collect();
        let seq = |v: Option<&Value>| v.and_then(|v| v.get("seq")).and_then(Value::as_u64);
        let on = "testverdict,patchverified,patchrefused";
        assert_eq!((seq(queued_event(&queue, on, 0)), seq(queued_event(&queue, on, 2))), (Some(2), Some(3)), "the next watched event past the cursor, in order");
        assert!(queued_event(&queue, on, 3).is_none() && queued_event(&queue, "testverdict", 0).is_none(), "none past the last, and only the kinds watched");
        release(&ctx, &repaired.id, "done").unwrap();

        let orphan = refused("WR-check", "L11");
        ctx.leases.test_sched(|s| { s.restore(&format!("{{\"id\":\"L11\",\"node\":\"node\",\"req\":{}}}", req.to_json()), 1); });
        ctx.store.update(|d| d.lives.push(orphan.clone())).unwrap();
        assert!(open("L11"), "the cargo caller owns a granted lease");
        arm(&orphan, "gone", true);
        departed_checks(&ctx, &orphan.id, &calls);
        assert!(!get(&ctx, &orphan.id).unwrap().active() && !open("L11"), "a departed cargo caller releases its unconfirmed session and lease");

        // released by its id alone, with no run of it followed here
        assert!(running("WR1").is_none());
        let log = home.join("WR1.log");
        std::fs::write(&log, "app line\n").unwrap();
        ctx.store.update(|d| d.lives.iter_mut().find(|x| x.id == "WR1").unwrap().log = log.to_string_lossy().into_owned()).unwrap();
        let reply = serve(&ctx, "live release WR1").unwrap();
        assert_eq!(field(&reply, "lease"), "its lease L1 freed", "{reply}");
        assert!(!open("L1"), "the session's release frees the lease its record names");
        assert_eq!(get(&ctx, "WR1").map(|x| (x.state, x.note)), Some(("ended".into(), "released".into())));
        assert_eq!(field(&serve(&ctx, "live release WR1").unwrap(), "lease"), "its lease L1 had ended", "a second release says what became of it");
        // its end is said in its log after its app's last line, once, and a wait on it fails at
        // once with how it ended (W12708, 11-18)
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "app line\n[devhub] WR1 ended: released\n", "its log says it ended, and why");
        let e = serve(&ctx, "live wait WR1 settled 1").unwrap_err();
        assert!(e.starts_with("WR1 ended: released") && e.contains("live again WR1"), "a wait on an ended session fails, saying how it ended: {e}");
        // a lane's wait on one of its sessions is its use of the lane's other running ones, not of
        // another lane's nor of an ended one (a walk's two Towns ended while it waited for its
        // Fab's first frame, W24524, 11-26)
        let of = |id: &str, lane: &str, state: &str| Live { lane: lane.into(), ..live(id, "", state) };
        ctx.store.update(|d| d.lives.extend([of("WT1", "walk", "starting"), of("WT2", "walk", "running"), of("WT3", "other", "running"), of("WT4", "walk", "ended")])).unwrap();
        let e = serve(&ctx, "live wait WT1 frame 0").unwrap_err();
        assert!(e.contains("not within 0 s"), "the wait on a session not up yet times out: {e}");
        let touched = |id: &str| get(&ctx, id).unwrap().touched_ms;
        assert!(touched("WT2") > 0, "the lane's other running session is in use while its lane waits");
        assert_eq!((touched("WT3"), touched("WT4")), (0, 0), "another lane's session and an ended one are not");
        // each one's last act, which a park for timed work while it is in use says it interrupted
        // (a walk's owner saw only that its run broke, 11-26)
        let act = |id: &str| get(&ctx, id).unwrap().last_act;
        assert_eq!([act("WT1"), act("WT2"), act("WT3"), act("WT4")], ["`live wait WT1 frame 0`", "its lane's wait on WT1", "", ""], "the wait is WT1's act and its lane's other running session's");
        let wt2 = get(&ctx, "WT2").unwrap();
        let at = wt2.last_act_ms + 40_000;
        assert_eq!(park_note("parked for timed L9 (bench)", Some(&wt2), true, at), format!("parked for timed L9 (bench); in use: its last act was its lane's wait on WT1, 40 s before; {RESTARTS_BY_ITSELF}"), "parked in use for timed work: what it interrupted");
        assert_eq!(park_note("parked for timed L9 (bench)", Some(&get(&ctx, "WT3").unwrap()), true, at), format!("parked for timed L9 (bench); {RESTARTS_BY_ITSELF}"), "with no act, the line as before");
        assert_eq!(park_note("parked for L9 (test of x): ..", Some(&wt2), false, at), "parked for L9 (test of x): ..", "an idle park says no act");
        ctx.store.update(|d| d.lives.retain(|x| !x.id.starts_with("WT"))).unwrap();
        // an app session's home: fresh by default (an empty archive), the box's own by name, its
        // `--home` archive, the harness's own for a test session or an --env test home (a lane's
        // Town signed a name into a box's own Town home, 11-27)
        let app = Live { id: "WH1".into(), app: "makepad-app-x".into(), ..Default::default() };
        let fresh = home_archive(&ctx, &app).unwrap().expect("an app session without --home gets a fresh home");
        assert_eq!(std::fs::read(&fresh).unwrap(), vec![0u8; 10240], "the fresh home's seed is an empty tar archive");
        assert_eq!(home_archive(&ctx, &app).unwrap(), Some(fresh), "the seed is made once and kept");
        for (x, why) in [
            (Live { home: BOX_HOME.into(), ..app.clone() }, "--home box runs on the box's own home"),
            (Live { test: true, ..app.clone() }, "a test session's harness gives each app its own home"),
            (Live { env: vec![("MAKEPAD_TEST_HOME".into(), "/x".into())], ..app.clone() }, "an --env test home is the session's own choice"),
        ] {
            assert_eq!(home_archive(&ctx, &x).unwrap(), None, "{why}");
        }
        assert_eq!(home_archive(&ctx, &Live { home: "/a/scan.tar".into(), ..app }).unwrap(), Some(PathBuf::from("/a/scan.tar")), "a --home archive seeds it");
        assert!(serve(&ctx, "live release WR-gone").unwrap_err().contains("no live job WR-gone"), "a release names what is missing");

        // a release by a lease's id frees what it names; a running session's lease is refused
        let owner = |lease: &str| ctx.store.read(|d| owned_lease(&d.lives, lease));
        let refused = owner("L2").expect("a running session's lease is refused to a release by its id");
        assert!(refused.contains("devhub live release WR2"), "the refusal names the session's own release: {refused}");
        assert!(owner("L5").is_some_and(|e| e.contains("WR6")), "a paused session holds its lease too");
        assert_eq!((owner("L1"), owner("L3"), owner("")), (None, None, None), "an ended session's lease, another lease, and no lease are anyone's to release");
        ctx.leases.test_sched(|s| { assert!(s.release("L3", "released", 2)); });
        assert!(!open("L3") && open("L2") && open("L5"), "a release ends the one lease it names");

        // the master's start: square the sessions with the leases both ways
        let on: Vec<(String, bool)> = restarted(&ctx).into_iter().map(|(x, held)| (x.id, held)).collect();
        assert_eq!(on, [("WR2".to_string(), true), ("WR5".to_string(), false), ("WR6".to_string(), true)], "held sessions go on, a starting one is placed again");
        assert!(!open("L4") && open("L2") && open("L5"), "the ended session's lease is freed, the running ones' kept");
        let x = get(&ctx, "WR4").unwrap();
        assert_eq!(x.state, "ended", "a running session whose lease is gone ends");
        assert!(x.note.contains("its lease L6 had ended") && x.note.contains("devhub live again WR4"), "{}", x.note);
        let lines = |id: &str| said.lock().unwrap().iter().filter(|l| l.contains(&format!("live {id} "))).count();
        assert_eq!((lines("WR3"), lines("WR4")), (1, 1), "one line each: {:?}", said.lock().unwrap());
        let again: Vec<String> = restarted(&ctx).into_iter().map(|(x, _)| x.id).collect();
        assert_eq!(again, ["WR2", "WR5", "WR6"], "a second start finds them squared");
        assert_eq!((lines("WR3"), lines("WR4")), (1, 1), "and says nothing more");
        let reply = serve(&ctx, "live release WR2").unwrap();
        assert!(field(&reply, "lease") == "its lease L2 freed" && !open("L2") && owner("L2").is_none(), "the session's own release ends its lease: {reply}");

        // a restart is no call: a session keeps its own touch through it, and its idle end with it.
        // That end waits for the restarted master's first ask of its box's caps, which say a
        // person's input there; a queued session whose starter left ends, not placed again (W19220
        // held its box through four restarts with no save or call, 11-25)
        let (now, min) = (wall_ms(), 60_000);
        for id in ["L21", "L22", "L23", "L24"] {
            ctx.leases.test_sched(|s| { s.restore(&format!("{{\"id\":{},\"node\":\"idle-box\",\"req\":{}}}", json_str(id), req.to_json()), 1); });
        }
        let tree = home.join("idle");
        std::fs::create_dir_all(&tree).unwrap();
        let on_box = |id: &str, lease: &str, touched_ms: u64| Live { app: "idle-app".into(), node: "idle-box".into(), tree: tree.to_string_lossy().into_owned(), touched_ms, ..live(id, lease, "running") };
        let quiet = on_box("WR11", "L21", now - 30 * min);
        let called = on_box("WR12", "L22", now - 2 * min);
        let saving = Live { test: true, ..on_box("WR13", "L23", now - 30 * min) };
        let watched = Live { visible: true, ..on_box("WR14", "L24", now - 30 * min) };
        let left = Live { app: "idle-app".into(), left_ms: now - 20 * min, touched_ms: now - 30 * min, ..live("WR15", "", "starting") };
        ctx.store.update(|d| d.lives.extend([quiet.clone(), called.clone(), saving.clone(), watched.clone(), left])).unwrap();
        let ours: Vec<(String, bool)> = restarted(&ctx).into_iter().filter(|(x, _)| x.app == "idle-app").map(|(x, held)| (x.id, held)).collect();
        assert_eq!(ours, [("WR11".to_string(), true), ("WR12".to_string(), true), ("WR13".to_string(), true), ("WR14".to_string(), true)], "the held ones go on");
        let x = get(&ctx, "WR15").unwrap();
        assert!(x.state == "ended" && x.note.contains("starter's call went") && x.note.contains("did not place it again"), "a queued session whose starter left ends, not placed again: {x:?}");
        assert_eq!(get(&ctx, "WR5").unwrap().touched_ms, 0, "one placed again keeps its own touch");
        let runs: Vec<Arc<Running>> = [&quiet, &called, &saving, &watched].into_iter().map(|x| take_back(&ctx, x)).collect();
        let touched = |id: &str| get(&ctx, id).unwrap().touched_ms;
        assert_eq!([touched("WR11"), touched("WR12"), touched("WR13"), touched("WR14")], [quiet.touched_ms, called.touched_ms, saving.touched_ms, watched.touched_ms], "taken back, each keeps its own touch");
        // a test session's save waits for its next call: noted as its touch, a restart keeps it
        due(&runs[2].pending, ["src/a.rs".to_string()]);
        note_save(&ctx, "WR13", &runs[2]);
        assert!(touched("WR13") >= now, "a test session's save is its touch");
        let idle = |at: u64| idle_now(&ctx, at).into_iter().filter(|x| x.app == "idle-app").map(|x| x.id).collect::<Vec<_>>();
        assert!(idle(now).is_empty(), "the restarted master has not asked its box yet: a person's input there and its builds are not known");
        ctx.leases.test_sched(|s| { s.set_caps("idle-box", crate::lease::Caps { input_idle_ms: Some(min), ..Default::default() }, now); });
        assert_eq!(idle(now), ["WR11"], "once asked, the session past its idle end ends; one with a call or a save inside the window stays, and so does a visible one its person works at");
        set(&ctx, "WR11", |s| s.set = "idle/2".into());
        set(&ctx, "WR12", |s| s.set = "idle/2".into());
        assert!(idle(now).is_empty(), "the other active set member's touch keeps the quiet session from its idle end");
        assert!(idle(called.touched_ms + IDLE_MS).contains(&quiet.id), "the quiet member is idle when its set's latest touch reaches the idle end");
        for id in ["WR11", "WR12", "WR13", "WR14"] {
            release(&ctx, id, "done").unwrap();
        }

        // sessions parked in use for a timed lease start again by themselves once it ends, under
        // their id; one whose lane runs the same app anew meanwhile, or released it, stays ended
        ctx.leases.test_sched(|s| { s.restore(&format!("{{\"id\":\"L7\",\"node\":\"node\",\"req\":{}}}", req.to_json()), 1); });
        let parked = |id: &str, app: &str, after: &str| Live { id: id.into(), app: app.into(), lane: "lane".into(), state: "ended".into(), resume_after: after.into(), ..Default::default() };
        ctx.store.update(|d| d.lives.extend([parked("WR7", "app7", "L7"), parked("WR8", "app", "L8"), parked("WR9", "app9", "L8")])).unwrap();
        let town = |id| Live { set: "walk/2".into(), ..parked(id, "town", "L7") };
        ctx.store.update(|d| d.lives.extend([town("WR-town-a"), town("WR-town-b")])).unwrap();
        serve(&ctx, "live release WR9").unwrap();
        assert!(resume_due(&ctx).is_empty(), "its timed lease runs: none is due");
        let x = get(&ctx, "WR8").unwrap();
        assert!(x.state == "ended" && x.resume_after.is_empty(), "the lane runs the same app anew (WR5, WR6): it stays ended");
        assert!(said.lock().unwrap().iter().any(|l| l.contains("live WR8 ") && l.contains("not started again, its lane runs WR5 now")), "{:?}", said.lock().unwrap());
        assert!(get(&ctx, "WR9").is_some_and(|x| x.state == "ended" && x.resume_after.is_empty()), "released by its lane: it stays ended");
        assert_eq!(resume_due(&ctx), Vec::<String>::new(), "WR7's timed lease still runs");
        ctx.leases.test_sched(|s| { assert!(s.release("L7", "released", 3)); });
        assert_eq!(resume_due(&ctx), ["WR7", "WR-town-a", "WR-town-b"], "both parked Towns of the same lane and set start together once the timed lease ends");
        for id in ["WR-town-a", "WR-town-b"] {
            assert!(get(&ctx, id).is_some_and(|x| x.state == "starting" && x.set == "walk/2" && x.resume_after.is_empty()), "resumed {id} starts as a member of its parked set");
        }
        let x = get(&ctx, "WR7").unwrap();
        assert!(x.state == "starting" && x.resume_after.is_empty() && x.lease.is_empty() && x.note.contains("timed L7"), "{x:?}");
        assert!(resume_due(&ctx).is_empty(), "once");

        // a starting session's queued ask goes with its release, before the release answers: left
        // to its start's next look, it stood a beat, and its lane's next timed start was refused as
        // a second timed lease of the lane (11-23)
        let mut timed = Req::new("lane", Kind::Timed);
        timed.live = true;
        let mut queued = String::new();
        ctx.leases.test_sched(|s| { queued = s.request(timed.clone(), 4); });
        ctx.store.update(|d| d.lives.push(live("WR10", "", "starting"))).unwrap();
        let asking = Asking("WR10");
        asking.names(super::super::leg::asked_lease(&format!("{queued} queued waits for a drained box")).unwrap());
        assert_eq!(partner_ask(&ctx, "WR10"), Ok(queued.clone()), "a joiner can use its partner's queued lease before it has a box");
        assert!(open(&queued), "{queued} waits in the queue");
        let reply = serve(&ctx, "live release WR10").unwrap();
        assert_eq!(field(&reply, "lease"), format!("its asked lease {queued} given up"), "{reply}");
        assert!(partner_ask(&ctx, "WR10").is_err_and(|e| e.contains("WR10") && e.contains("ended")), "a joiner learns that its queued partner ended");
        let timed_left = ctx.leases.view().iter().filter(|l| l.req.kind == Kind::Timed && l.req.lane == "lane" && !matches!(l.state, State::Ended(_))).map(|l| l.id.clone()).collect::<Vec<_>>();
        assert!(timed_left.is_empty(), "the release gave the ask up at once: no timed lease of the lane is left to refuse its next start: {timed_left:?}");
        // The held session's resident RAM keeps another lane's ask queued until the call-end grace passes.
        req.src = Src::Overlay;
        req.rev = Some(format!("work@{}", "a".repeat(40)));
        ctx.leases.test_sched(|s| { s.restore(&format!("{{\"id\":\"L31\",\"node\":\"ram-box\",\"req\":{}}}", req.to_json()), 1); });
        let parked_hold = Live { id: "WR-park".into(), lease: "L31".into(), tree: home.to_string_lossy().into_owned(), src: req.src.as_str().into(), ..held.clone() };
        ctx.store.update(|d| d.lives.push(parked_hold.clone())).unwrap();
        ctx.leases.live_idle(&parked_hold.lease);
        assert!(open("L31"));
        let mut park_note = None;
        ctx.leases.test_sched(|s| {
            let after_grace = 2 + crate::lease::sched::LIVE_IN_USE_MS;
            s.add_node("ram-box", "192.0.2.1", None);
            s.set_caps("ram-box", crate::lease::Caps {
                cores: 16, ram_gb: 16.0, ram_avail_gb: 16.0, disk_free_gb: 100.0, role: Some(crate::lease::Role::Pool),
                features: crate::lease::FEATURES.iter().map(|f| f.to_string()).collect(), ..Default::default()
            }, after_grace);
            s.beat("L31", after_grace);
            s.set_info("L31", r#"{"running":"makepad","held_mb":8192,"idle_s":30}"#, after_grace);
            assert!(s.live_idle.contains("L31"), "the lease charges its idle share");
            let mut waiting = Req::new("waiting", Kind::Test);
            waiting.src = req.src;
            waiting.rev = req.rev.clone();
            waiting.ram_gb = Some(8.0);
            let ask = s.request(waiting, after_grace);
            let actions = s.tick(after_grace);
            park_note = actions.iter().find_map(|action| match action {
                crate::lease::sched::Action::ParkLive { leases, note, .. } if leases.iter().any(|id| id == "L31") && note.starts_with(&format!("parked for {ask} ")) => Some(note.clone()),
                _ => None,
            });
            assert!(park_note.is_some(), "the RAM ask parks the held lease: {actions:?}; ask: {:?}", s.get(&ask));
        });
        let note = park_note.unwrap();
        let parked = set(&ctx, &parked_hold.id, |s| record_park(s, &note, 3_660_000)).unwrap();
        ctx.leases.release_live(&parked.lease);
        assert!(!parked.active() && !open("L31"), "parking frees the box");
        std::fs::create_dir(home.join(".git")).unwrap();
        let request = format!(r#"live test {{"app":"app","lane":"lane","repo_dir":{}}}"#, json_str(&parked.tree));
        let notices = std::cell::RefCell::new(Vec::new());
        let reply = serve_with(&ctx, &request, &|line| notices.borrow_mut().push(line.to_string()), &|ctx, claim, first_call| {
            let id = claim.id();
            assert_eq!(*notices.borrow(), [format!("the held session WR-park was {note} at 01:01Z; this run started cold")], "the caller hears why before the new run starts");
            assert!(first_call && id != parked.id, "the call starts a fresh session");
            set(ctx, id, |s| {
                s.state = "running".into();
                s.patch = Some(LivePatch { outcome: "initial".into(), tests: Some(crate::state::LiveTests { ran: 1, passed: 1, ..Default::default() }), ..Default::default() });
            });
            Ok(())
        }).expect("the call on the parked session starts cold");
        let fresh = crate::state::live_from(&makepad_strict_json::parse(reply.as_bytes()).unwrap());
        assert!(fresh.id != parked.id && fresh.test_code() == 0, "the fresh run's passing verdict reaches the caller: {reply}");
        assert!(get(&ctx, &parked.id).unwrap().held.is_empty(), "the cold-start notice is consumed once");
        // A call that names other DEVHUB_ALSO paths than its session's ships those from that call on.
        set(&ctx, &fresh.id, |s| s.state = "paused".into());
        let also_request = format!(r#"live test {{"app":"app","lane":"lane","repo_dir":{},"also":["local/later"]}}"#, json_str(&parked.tree));
        serve_with(&ctx, &also_request, &|_| {}, &|_, claim, _| panic!("a call naming other DEVHUB_ALSO paths starts no new session ({})", claim.id())).expect("the call answers from its session");
        assert_eq!(get(&ctx, &fresh.id).unwrap().also, ["local/later"], "the session ships the paths the call names");
        set(&ctx, &fresh.id, |s| s.state = "running".into());

        // A lane may never call again. Its parked records share the ordinary history bound.
        ctx.store.update(|d| {
            for i in 0..=crate::state::KEEP {
                let lane = if i == 1 { "finished" } else { "continuing" };
                d.lives.push(Live { id: format!("WH{i}"), lane: lane.into(), state: "ended".into(), held: held.held.clone(), ..Default::default() });
            }
            d.lives.push(live("WR-plain", "", "ended"));
        }).unwrap();
        assert_eq!(ctx.store.read(|d| d.lives.iter().filter(|s| !s.active()).count()), crate::state::KEEP, "held and plain records share KEEP");
        assert!(get(&ctx, "WR-plain").is_none() && get(&ctx, "WH0").is_none(), "plain ended records go first, then the oldest parked record");
        assert_eq!(get(&ctx, "WH1").unwrap().held, held.held, "the next parked note survives trimming");
        assert!(get(&ctx, &fresh.id).unwrap().active(), "active sessions survive history trimming");
        serve(&ctx, "live end-lane finished").unwrap();
        assert!(get(&ctx, "WH1").unwrap().held.is_empty(), "lane cleanup clears the ended record's diagnostic");
        assert_eq!(get(&ctx, "WH2").unwrap().held, held.held, "another lane keeps its cold-start note");
        drop(asking);
        assert_eq!(super::super::leg::asked_lease("held back: lane work waits"), None, "a line before the ask names none");
        drop(ctx);
        let _ = std::fs::remove_dir_all(&home);
    }

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
        // a file back to the pin's bytes is no longer dirty: it ships again as the tree has it, and
        // one shipped that is gone again ships gone (a revert shipped nothing, 11-20)
        let shipped = std::collections::BTreeMap::from([
            ("src/a.rs".to_string(), "false:false:edited".to_string()),
            ("src/new.rs".to_string(), "false:false:added".to_string()),
            ("src/del.rs".to_string(), "-".to_string()),
        ]);
        let (mut changed, mut deleted) = (Vec::new(), Vec::new());
        super::back_to_pin(&tree, &shipped, &mut changed, &mut deleted);
        assert_eq!((changed.as_slice(), deleted.as_slice()), (&["src/a.rs".to_string()][..], &["src/del.rs".to_string(), "src/new.rs".to_string()][..]));
        let sources = super::capture(&tree, &changed, &deleted, &shipped, &base.join("ship")).unwrap();
        assert!(sources["src/a.rs"] != shipped["src/a.rs"] && base.join("ship/src/a.rs").exists(), "the revert ships: {sources:?}");
        assert_eq!((sources["src/new.rs"].as_str(), sources["src/del.rs"].as_str()), ("-", "-"));
        let (mut again, mut gone) = (vec!["src/a.rs".to_string()], Vec::new());
        super::back_to_pin(&tree, &shipped, &mut again, &mut gone);
        assert_eq!(again, ["src/a.rs"], "a dirty file is named once");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A test session's edit that is none of its sources: a data file its tests read (a race
    /// fixture's game.splash in no package of the graph) starts them again; the last verdict
    /// stood on such a file and replayed a pass from the bytes before it (RACECARS, FOLIAGE, 11-12).
    #[test]
    fn a_data_file_the_tests_read_runs_them_again_and_one_they_never_read_keeps_the_verdict() {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<String>>();
        let files = ["res/game.splash", "res/notes.md", "res/new/level.splash", "src/data.json", "src/lib.rs"].map(String::from);
        let inputs = set(&["src/lib.rs"]);
        let outside = set(&["res/game.splash", "res/notes.md", "res/new/level.splash"]);
        let rerun = |read: &[&str]| super::rerun_on(&files, &inputs, &outside, &set(read));
        let fresh = rerun(&["res/game.splash", "res/new/level.splash"]);
        assert!(fresh.contains(&"res/game.splash".to_string()), "a file outside every package the tests read runs them again: {fresh:?}");
        assert!(fresh.contains(&"res/new/level.splash".to_string()), "so does one the JIT names as read (in a folder the tests listed): {fresh:?}");
        assert!(!fresh.contains(&"res/notes.md".to_string()), "a file outside every package the tests never read keeps the verdict: {fresh:?}");
        assert!(fresh.contains(&"src/data.json".to_string()), "a file in a package that is no source runs them again, read or not: {fresh:?}");
        assert!(!fresh.contains(&"src/lib.rs".to_string()), "a source is patched, not run again: {fresh:?}");
        assert_eq!(rerun(&[]), ["src/data.json"], "a JIT that names no reads keeps the verdict for every file outside the packages, as before");
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
        assert!(super::restart_rows(&[x.clone()], t0 + 18 * 60_000).is_empty());
        // a test call whose edit has no verdict and whose JIT wrote nothing for 10 min makes no
        // progress, said with that evidence (W25685 waited 102 min at "restarting JIT", 11-27)
        let call = Live { test: true, pid: 41, patch: Some(LivePatch { edit_ms: t0, outcome: "restart".into(), ..Default::default() }), ..x };
        let quiet = |min: u64| Some(min * 60_000);
        assert_eq!(super::no_progress(&call, quiet(9), t0 + 9 * 60_000), None, "not past 10 min yet");
        assert_eq!(super::no_progress(&call, quiet(102), t0 + 103 * 60_000).as_deref(), Some("no progress for 102 min through its JIT's restart: the edit saved at 13:49:00Z has no verdict, and its JIT (pid 41) has written nothing since 13:50:00Z"));
        let verdict = Live { patch: Some(LivePatch { outcome: "refused".into(), ..call.patch.clone().unwrap() }), ..call.clone() };
        let building = Live { pid: 0, ..call.clone() };
        let app = Live { test: false, ..call.clone() };
        assert!([verdict, building, app].iter().all(|x| super::no_progress(x, quiet(30), t0 + 30 * 60_000).is_none()), "an edit with its verdict, a run not started yet (its JIT builds: its phase says so) and an app session are none");
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

    /// A session joins one of its lane's that runs placed, on that one's box (`--node` naming the
    /// same box or none), and keeps it from its idle end while it is active (11-16: a hidden test
    /// WM parked under the visible app that joined it left the app a dead socket). And the lease a
    /// session asks for: a timed one's is timed (`--timed`), a visible one's needs a display.
    #[test]
    fn a_session_joins_a_placed_session_of_its_lane_on_its_box() {
        let live = |id: &str, lane: &str, state: &str, node: &str, joins: &str| crate::state::Live { id: id.into(), lane: lane.into(), state: state.into(), node: node.into(), joins: joins.into(), note: if state == "ended" { "parked for L9".into() } else { String::new() }, ..Default::default() };
        let lives = [live("W1", "a", "running", "n1", ""), live("W2", "b", "running", "n2", ""), live("W3", "a", "ended", "n1", ""), live("W4", "a", "starting", "", "")];
        assert_eq!(super::joined_box(&lives, "W1", "a", "", ""), Ok("n1".to_string()), "a plain join takes its placed partner's box");
        assert_eq!(super::joined_box(&lives, "W1", "a", "n1", ""), Ok("n1".to_string()), "--node naming its box");
        for (joins, asked, said) in [("W1", "n2", "name two boxes: W1 runs on n1"), ("W2", "", "it is lane b's session"), ("W3", "", "it ended: parked for L9"), ("W4", "", "still waits for its box"), ("W9", "", "no live session W9")] {
            assert!(super::joined_box(&lives, joins, "a", asked, "").is_err_and(|e| e.contains(said)), "{joins}: {:?}", super::joined_box(&lives, joins, "a", asked, ""));
        }
        let member = |mut x: crate::state::Live| { x.set = "walk/2".into(); x };
        let with = [member(live("W1", "a", "starting", "", "")), member(live("W5", "a", "starting", "", "W1"))];
        assert_eq!(super::joined_box(&with, "W1", "a", "", "walk/2"), Ok(String::new()), "a queued set partner leaves the box for the scheduler to choose");
        for (records, set) in [(&with[..], "other/2"), (&with[..], ""), (&lives[..], "walk/2")] {
            assert!(super::joined_box(records, "W1", "a", "", set).is_err_and(|e| e.contains("a member joins a member of its own set")), "a join cannot cross set membership ({set:?})");
        }
        assert!(super::joined(&with, "W1") && !super::joined(&with, "W5"), "W1 is joined while W5 runs");
        let ended = [live("W1", "a", "running", "n1", ""), live("W5", "a", "ended", "n1", "W1")];
        assert!(!super::joined(&ended, "W1"), "an ended session joins nothing");
        let back = crate::state::live_from(&crate::state::live_value(&with[1]));
        assert_eq!(back.joins, "W1", "a session's join survives the store");
        assert_eq!(back.set, "walk/2", "a session's set name and count survive the store");
        // its lease's kind and needs: a timed one's is timed, and visible it needs a display there
        use crate::lease::Kind;
        let x = |visible: bool, timed: bool, needs: &str| super::lease_kind(&crate::state::Live { visible, timed, needs: needs.into(), ..Default::default() });
        assert_eq!(x(false, false, "os=linux"), (Kind::Test, "os=linux".to_string()));
        assert_eq!(x(true, false, ""), (Kind::Screen, String::new()));
        assert_eq!(x(false, true, "os=macos"), (Kind::Timed, "os=macos".to_string()), "a headless timed session");
        assert_eq!(x(true, true, "os=macos"), (Kind::Timed, "os=macos,gui=1".to_string()), "a visible timed session needs a display");
        assert_eq!(x(true, true, "display=1"), (Kind::Timed, "display=1".to_string()), "one that names a display keeps it");
    }

    /// A test session whose starter left ends after LEFT_MS with nothing since: queued or on its
    /// box, counted from its last touch (11-16).
    #[test]
    fn a_session_whose_starter_left_ends_after_a_quiet_spell() {
        let min = 60_000;
        let now = 100 * min;
        let x = |state: &str, left_ms: u64, touched_ms: u64| crate::state::Live { id: "W1".into(), state: state.into(), left_ms, touched_ms, ..Default::default() };
        assert!(super::left_long(&x("starting", now - 3 * min, now - 5 * min), now), "queued for its box, its starter gone 3 min: it ends");
        assert!(super::left_long(&x("running", now - 4 * min, now - 3 * min), now), "its verdict for nobody 3 min ago: it parks");
        assert!(!super::left_long(&x("running", now - 4 * min, now - min), now), "a verdict a minute ago: the spell counts from it");
        assert!(!super::left_long(&x("running", now - 2 * min, now - 5 * min), now), "its starter gone 2 min: it waits");
        assert!(!super::left_long(&x("running", 0, now - 30 * min), now), "a session whose last call's client stayed waits for the idle end");
        assert!(!super::left_long(&x("ended", now - 9 * min, now - 9 * min), now), "an ended one has nothing to end");
    }

    /// A run in flight keeps its session from the idle, left and landed ends only while a caller
    /// waits for its answer or it still writes to its log: W11710's first run hung at `running 4
    /// tests`, no call waited for it (a `live start --test`), and it held its box 4.7 h (11-17).
    #[test]
    fn a_silent_run_nobody_waits_for_holds_no_session() {
        let min = 60_000u64;
        let dir = std::env::temp_dir().join(format!("devhub-live-orphan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("W1.log");
        std::fs::write(&log, "running 4 tests\n").unwrap();
        let now = super::wall_ms();
        let f = std::fs::File::options().write(true).open(&log).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(now - 284 * min)).unwrap();
        let x = crate::state::Live { id: "W1".into(), state: "running".into(), log: log.to_string_lossy().into_owned(), ..Default::default() };
        let quiet = super::quiet_ms(&x, now);
        assert!(quiet.is_some_and(|q| q >= 284 * min), "the run's silence is its log's age: {quiet:?}");
        assert!(!super::run_holds(true, 0, quiet), "a run silent 284 min that no call waits for is orphaned: its session ends");
        assert!(super::run_holds(true, 1, quiet), "one a call waits for is said, never ended");
        assert!(super::run_holds(true, 0, Some(2 * min)), "one that wrote 2 min ago still works");
        assert!(super::run_holds(true, 0, super::quiet_ms(&crate::state::Live { state: "paused".into(), ..x.clone() }, now)), "a paused run's silence is not counted");
        assert!(!super::run_holds(false, 0, None), "no run in flight: nothing holds it");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One live test session per lane, package and system: a new one replaces the lane's other
    /// sessions of its package on its system, whatever they select, and nothing of another lane,
    /// package or system, no app session and no ended one (11-15). A Windows session ended the
    /// lane's Linux one while its JIT still built (11-26).
    #[test]
    fn a_new_test_session_replaces_its_lanes_sessions_of_the_package() {
        let live = |id: &str, lane: &str, app: &str, test: bool, args: &[&str], state: &str| crate::state::Live { id: id.into(), lane: lane.into(), app: app.into(), test, test_args: args.iter().map(|a| a.to_string()).collect(), state: state.into(), ..Default::default() };
        let lives = [
            live("W1", "rc-a", "pkg", true, &["--lib"], "running"),
            live("W2", "rc-a", "pkg", true, &["--test", "t", "--", "only"], "starting"),
            live("W3", "rc-b", "pkg", true, &["--lib"], "running"),
            live("W4", "rc-a", "other", true, &["--lib"], "running"),
            live("W5", "rc-a", "pkg", false, &[], "running"),
            live("W6", "rc-a", "pkg", true, &["--lib"], "ended"),
            live("W7", "rc-a", "pkg", true, &["--lib", "--", "filter"], "starting"),
            crate::state::Live { needs: "gpu=1,os=Windows".into(), ..live("W8", "rc-a", "pkg", true, &["--lib"], "starting") },
            crate::state::Live { pin: "box-w".into(), ..live("W9", "rc-a", "pkg", true, &["--lib"], "starting") },
            crate::state::Live { needs: "os=windows".into(), ..live("W10", "rc-a", "pkg", true, &["--test", "t"], "starting") },
        ];
        assert_eq!(super::replaced(&lives, &lives[6]), ["W1", "W2"], "another target or filter of the lane's package is replaced; another lane's, another package's, another system's, an app session and an ended one are not");
        assert_eq!(super::replaced(&lives, &lives[9]), ["W8"], "a Windows session replaces only the lane's Windows session of the package");
        assert!(super::replaced(&lives, &lives[8]).is_empty(), "nor does one pinned to a box replace another system's");
        assert!(super::replaced(&lives, &lives[4]).is_empty(), "an app session replaces no test session");
        // one asked while another waited for its box: the older ends once it has its box and its
        // call is done, the newest of them stays (several calls at once, W22126-W22157, 11-26)
        let by = |i: usize| super::superseded_by(&lives, &lives[i]);
        assert_eq!([by(0), by(1), by(2), by(3), by(4), by(6), by(7), by(8)], [Some("W7"), Some("W7"), None, None, None, None, Some("W10"), None]);
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

    /// A grab names its file on every OS, and a gseq every frame in order, each with what it was
    /// drawn at; they come here named as a look's frames, numbered on after the session's own.
    #[test]
    fn a_grab_names_its_file_on_every_os() {
        let lines = |l: &str| vec!["{\"frame\":3}".to_string(), l.to_string()];
        let first = |l: &str| super::grab_files(&lines(l), "L7").first().map(|(rel, _)| rel.clone());
        assert_eq!(first(r#"{"png":"/h/devhub/tmp/L7/g/a.png"}"#).as_deref(), Some("g/a.png"));
        assert_eq!(first(r#"{"png":"C:\\Users\\u\\devhub\\tmp\\L7\\g\\a.png"}"#).as_deref(), Some("g/a.png"));
        assert_eq!(first(r#"{"png":"/h/devhub/tmp/L70/g/a.png"}"#), None);
        assert_eq!(first(r#"{"png":"/h/devhub/tmp/L7/g/a.png","capture_ms":NaN}"#).as_deref(), Some("g/a.png"), "a line no JSON reader takes still names its file");
        assert_eq!(super::grab_files(&lines(r#"{"png":"/h/devhub/tmp/L7/g/a.png","frame":9,"patch":2}"#), "L7"), [("g/a.png".to_string(), Some((9, 2)))]);
        let seq = r#"{"png":["/h/devhub/tmp/L7/g/1.png","/h/devhub/tmp/L7/g/2.png","/h/devhub/tmp/L7/g/3.png"],"n":3,"every_ms":20,"frames":[{"png":"/h/devhub/tmp/L7/g/1.png","frame":40,"patch":1},{"png":"/h/devhub/tmp/L7/g/2.png","frame":41,"patch":1},{"png":"/h/devhub/tmp/L7/g/3.png"}]}"#;
        let files = super::grab_files(&lines(seq), "L7");
        assert_eq!(files, [("g/1.png".to_string(), Some((40, 1))), ("g/2.png".into(), Some((41, 1))), ("g/3.png".into(), None)], "a gseq names every frame, in order");
        let drawn: Vec<_> = files.iter().map(|(_, d)| *d).collect();
        let dir = std::path::Path::new("/l/screencap/lane");
        assert_eq!(super::frame_paths(dir, "W5", 2, &drawn), [dir.join("W5-3-f40-p1.png"), dir.join("W5-4-f41-p1.png"), dir.join("W5-5.png")], "named as a look's, numbered on");
        assert_eq!(super::frames_said(&super::frame_paths(dir, "W5", 2, &drawn)), "its 3 frames: W5-3-f40-p1.png .. W5-5.png in /l/screencap/lane");
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
        // a call that comes before the restarted master took its session's box back waits for it,
        // and holds the session there: W16863's call, asked again into the restart, ran unheld and
        // the idle sweep ended its session under it (11-23)
        let joined: std::sync::Arc<std::sync::Mutex<Option<u32>>> = Default::default();
        let j = joined.clone();
        let reattach = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            *j.lock().unwrap() = Some(7);
            super::JOINED.1.notify_all();
        });
        let mut said = 0;
        assert_eq!(super::rejoined(&mut || *joined.lock().unwrap(), &|| true, &mut || said += 1), Some(7), "the box its reattach joined");
        reattach.join().unwrap();
        assert_eq!(said, 1, "said once, as it starts to wait");
        let mut said = 0;
        assert_eq!(super::rejoined(&mut || None::<u32>, &|| false, &mut || said += 1), None, "no app or no session: at once");
        assert_eq!(said, 0);
        let gone = std::sync::Arc::new(AtomicBool::new(false));
        let leaves = gone.clone();
        let client = std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(200)); leaves.store(true, Ordering::SeqCst); });
        assert_eq!(super::rejoined(&mut || None::<u32>, &|| !gone.load(Ordering::SeqCst), &mut || {}), None, "its client went");
        client.join().unwrap();
    }

    /// A watch with no verdict asks the JIT's `live` where the edit is: still on it, installed (and
    /// what its check waits on), or done with nothing installed. A fixed 60 s said "edit not
    /// applied" of two Fab edits the JIT had patched, at 70 s and at 7 s with its check still out
    /// (11-16). The watch's own answer when it ran out counts as no verdict: it did, and the JIT was
    /// never asked (11-17).
    #[test]
    fn a_watch_with_no_verdict_asks_where_the_jit_is_with_the_edit() {
        use super::{edit_state, terminal, EditState};
        use crate::state::LivePatch;
        let live = |editing: u64, patches: u64, unverified: u64| -> Result<Vec<String>, String> {
            Ok(vec![r#"{"measured":"live counters"}"#.to_string(), format!(r#"{{"now_ms":1.0,"frames":40,"patches":{patches},"frame_patch":3,"restart_pending":0,"editing":{editing},"unverified":{unverified}}}"#)])
        };
        assert!(matches!(edit_state(&live(1, 3, 0), Some(3)), Some(EditState::On(_))), "its round runs: the watch goes on");
        assert_eq!(edit_state(&live(0, 4, 2), Some(3)), Some(EditState::Installed(2, String::new())), "installed after the ship, its check out");
        assert_eq!(edit_state(&live(1, 4, 0), Some(3)), Some(EditState::Installed(0, String::new())), "installed, whatever round runs now");
        let waits = Ok(vec![r#"{"now_ms":1.0,"frames":40,"patches":4,"frame_patch":3,"restart_pending":0,"editing":0,"unverified":1,"unverified_why":"lib.rs: patch group 4 has run unverified 61 s; its check: not queued"}"#.to_string()]);
        assert_eq!(edit_state(&waits, Some(3)), Some(EditState::Installed(1, "lib.rs: patch group 4 has run unverified 61 s; its check: not queued".into())), "installed, and what its check waits on");
        // the watch's own answer when it ran out is no verdict: the JIT is asked where the edit is
        assert!(!super::verdict_line(r#"{"event":"none","waited_s":60,"rearm":"watch since=11337"}"#), "a watch that ran out");
        assert!(super::verdict_line(r#"{"event":"patchverified","seq":11338,"edit":"lib.rs","group":3}"#), "a verdict");
        assert_eq!(edit_state(&live(0, 3, 1), Some(3)), Some(EditState::Unchanged), "done, nothing installed: another group's check is not this edit's");
        assert_eq!(edit_state(&live(0, 4, 0), None), None, "no count at the ship: not known");
        assert_eq!(edit_state(&Err("pid 7 did not answer".into()), Some(3)), None, "no answer: its app gone?");
        let older = Ok(vec![r#"{"now_ms":1.0,"frames":40,"patches":3,"frame_patch":3,"restart_pending":0}"#.to_string()]);
        assert_eq!(edit_state(&older, Some(3)), None, "a JIT that does not say editing");
        let p = LivePatch { outcome: "unchanged".into(), ..Default::default() };
        assert!(terminal(&p), "an edit that changed nothing compiled has its verdict");
    }

    /// A test call with nothing new to ship answers with the last verdict only while the JIT
    /// took no change after it. A two-file ship the JIT patched as two groups had group 3's
    /// verdict answered again while group 4's tests ran (11-17).
    #[test]
    fn a_verdict_is_never_reused_across_a_change_the_jit_took_after_it() {
        use crate::state::LivePatch;
        let ev = |kind: &str, seq: u64, group: u64| format!(r#"{{"event":"{kind}","seq":{seq},"pid":7,"app":"t","at_ms":1,"verified":true,"group":{group}}}"#);
        let verdict = ev("testverdict", 7, 3);
        let old = LivePatch { seq: 7, cursor: super::event_cursor(&makepad_strict_json::parse(verdict.as_bytes()).unwrap()), ..Default::default() };
        let taken = |events: &[String]| super::taken_since(events, &old);
        assert!(!taken(&[ev("patchverified", 6, 3), verdict.clone()]), "nothing after the verdict: it stands");
        assert!(!taken(&[verdict.clone(), ev("panic", 8, 0)]), "an event that changed no code: it stands");
        assert!(taken(&[verdict.clone(), ev("patchverified", 8, 4)]), "a patch taken after it: its verdict is waited for");
        assert!(taken(&[verdict.clone(), ev("patchrefused", 8, 4)]), "a refused one too: the tests no longer run what the verdict ran");
        assert!(taken(&[ev("testverdict", 2, 0)]), "a queue that started over (an exec's restart): a newer run");
        assert!(taken(&[ev("patchverified", 7, 9)]), "another event at its sequence number: the queue started over");
    }

    /// A test verdict on a round that took none of the shipped files is a JIT miss, never a pass:
    /// a ship's folder answered for the file in it (11-17). A round that took the file, a
    /// restarted run, the first run and a ship of nothing the JIT has as a source keep theirs.
    #[test]
    fn a_verdict_on_a_round_that_took_none_of_the_shipped_files_is_a_jit_miss() {
        use crate::state::{LivePatch, LiveTests};
        let file = "apps/x/src/canvas/draw.rs".to_string();
        let verdict = |edit: &str, restarted: bool| -> LiveTests {
            let line = format!(r#"{{"event":"testverdict","seq":9,"edit":{edit},"group":0,"ran":1,"passed":1,"failed":0,"ms":188.8,"selected":"all","skipped":0,"restarted":{restarted}}}"#);
            crate::state::live_tests_from(&makepad_strict_json::parse(line.as_bytes()).unwrap()).unwrap()
        };
        let folder = verdict(r#"["/box/dev/src/apps/x/src/canvas"]"#, false);
        let mut p = LivePatch { files: vec![file.clone()], tests: Some(folder.clone()), outcome: "patched".into(), ..Default::default() };
        assert_eq!(super::jit_miss(&mut p, &[file.clone()]), Some(vec![file.clone()]), "a folder-only verdict for the shipped file: a miss");
        assert!(p.tests.is_none() && p.outcome == "jit_error", "its tests are no verdict on the edit: {}", p.outcome);
        let said = crate::state::jit_error_verdict(&p);
        assert!(said.starts_with("JIT error, not your change: the JIT did not take apps/x/src/canvas/draw.rs"), "the call's answer: {said}");
        let keeps = |t: LiveTests, taken: &[String]| {
            let mut p = LivePatch { tests: Some(t), outcome: "patched".into(), ..Default::default() };
            super::jit_miss(&mut p, taken).is_none() && p.tests.is_some() && p.outcome == "patched"
        };
        assert!(keeps(verdict(r#"["/box/dev/src/apps/x/src/canvas/draw.rs"]"#, false), &[file.clone()]), "a round that took the file: its verdict");
        assert!(keeps(verdict(r#"["/box/dev/src/apps/x/src/canvas"]"#, true), &[file.clone()]), "a restarted run loaded every source");
        assert!(keeps(verdict(r#""initial""#, false), &[file.clone()]), "the first run follows no edit");
        assert!(keeps(folder, &[]), "a ship of nothing the JIT has as a source: not the JIT's to take");
        assert!(!keeps(verdict(r#"["/box/myapps/x/src/canvas/draw.rs"]"#, false), &[file]), "a path that only ends like the file names another file");
    }

    /// A relaunch its JIT queued in place ends when no edit waits for a restart any more (the new
    /// run loaded the sources, in the same process), or fails with the app's reason when it
    /// postponed it, or with the new run's check errors when it did not compile (it ended in place;
    /// its run loaded the sources, so `restart_pending` alone read as restarted); until then it is
    /// on its way. The session said "queued" and stayed "restart pending" over a restart that had
    /// happened (11-06).
    #[test]
    fn a_relaunch_in_place_ends_when_the_new_run_has_the_sources() {
        use super::RelaunchEnd;
        let live = |pending: u32| vec![format!(r#"{{"now_ms":1.0,"frames":40,"patches":3,"frame_patch":3,"restart_pending":{pending}}}"#)];
        let postponed = "[I] platform/src/restart.rs:433:17 - restart postponed (struct `Plate` changed): unsaved: plate 1\n";
        assert_eq!(super::relaunch_end(&live(2), "relax live: restarting (struct `Plate` changed)\n"), None, "on its way");
        assert_eq!(super::relaunch_end(&live(2), postponed), Some(RelaunchEnd::Postponed("(struct `Plate` changed): unsaved: plate 1".to_string())));
        let end = "relax: run end: 60 threads, 148.2 ms; run's threads stayed: []; 1289 ms in all\n";
        assert_eq!(super::relaunch_end(&live(0), &format!("{postponed}{end}")), Some(RelaunchEnd::Done), "the app allowed it after all");
        assert_eq!(super::relaunch_end(&live(0), "relax live: restarting (session relaunch)\n"), None,
            "a relaunch with nothing pending: not done before its run's end");
        assert_eq!(super::relaunch_end(&live(0), end), Some(RelaunchEnd::Done), "its run ended and the next one runs");
        let checking = vec![r#"{"now_ms":1.0,"restart_pending":0,"restart_unfinished":1}"#.to_string()];
        assert_eq!(super::relaunch_end(&checking, ""), None, "the new run still boots or checks: on its way");
        assert_eq!(super::relaunch_end(&[], ""), None, "a look that failed: looked at again");
        let refused = "relax live: restarting (struct `Plate` changed)\nrelax live: the run ends in place: its app check found 1 error: src/a.rs:3:8: E0277: the trait bound `f32: Eq` is not satisfied; the next save or `session relaunch` starts the next one\n";
        assert_eq!(super::relaunch_end(&live(0), refused), Some(RelaunchEnd::Refused("its app check found 1 error: src/a.rs:3:8: E0277: the trait bound `f32: Eq` is not satisfied".to_string())),
            "a new run that failed its check: its errors, though it loaded the sources");
        // the app says `restart postponed` at its first ask in a run only: a later relaunch in that
        // run meets the same postponement, and waited as long as the app's blocker lasted
        let why = "(struct `Plate` changed): unsaved: plate 1".to_string();
        assert_eq!(super::postponed_before(postponed), Some(why.clone()), "postponed earlier in this run");
        assert_eq!(super::postponed_before(&format!("{postponed}{end}")), None, "a run that ended since asks afresh");
        assert_eq!(super::postponed_before(&format!("{postponed}relax live: the app reached a restart point; the rebuilt makepad waits for `session relaunch`\n")), None,
            "the app allowed a restart since");
        assert_eq!(super::postponed_before(&format!("{end}{postponed}")), Some(why.clone()), "postponed in the run after the last end");
        assert_eq!(super::postponed_before("relax live: restarting (session relaunch)\n"), None, "never postponed");
        let said = super::postponed_said("W3", &why);
        assert!(said.starts_with("W3: the app postponed its restart: (struct `Plate` changed): unsaved: plate 1"), "the app's reason, what a restart now costs: {said}");
        assert!(said.contains("pass --anyway to restart now: `makepad devhub live act W3 session relaunch --anyway`"), "how to restart it now: {said}");
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

    /// A call runs in the lane's open session of its package when that session suits the call's
    /// placement ([`super::suits`]): one that names none takes it wherever it runs, `--node` a
    /// session on that box, `--avoid` one on none of those boxes, `--needs` one asked with the same.
    /// An app session answers only a start placed exactly as it is.
    #[test]
    fn a_session_answers_a_call_its_placement_suits() {
        let args = vec!["--lib".to_string()];
        let x = crate::state::Live { id: "W1".into(), state: "running".into(), lane: "lane-x".into(), app: "pkg-x".into(), test: true, test_args: args.clone(), needs: "os=linux,gpu=1".into(), node: "box-a".into(), ..Default::default() };
        let call = |needs: &str, pin: &str, avoid: &str| super::answers(&x, "lane-x", "pkg-x", true, false, &args, needs, pin, avoid, "", &[]);
        assert!(call("gpu=1, os=linux", "", ""), "the same needs in another order");
        assert!(!call("os=macos", "", ""), "another OS starts its own session");
        assert!(call("", "", ""), "no placement: the lane's session, wherever it runs");
        assert!(!call("os=linux,gpu=1", "box-b", ""), "another box");
        assert!(call("", "box-a", ""), "the box it runs on");
        assert!(!call("", "", "box-c,box-a"), "a box the call avoids");
        assert!(call("", "", "box-c"), "it runs on none of the avoided");
        let other = |lane: &str, app: &str, test_args: &[String]| super::answers(&x, lane, app, true, false, test_args, "", "", "", "", &[]);
        assert!(!other("lane-y", "pkg-x", &args) && !other("lane-x", "pkg-y", &args) && !other("lane-x", "pkg-x", &["--test".into(), "t".into()]), "another lane, package or target");
        // started with --node box-p, still waiting for it: a call that names none takes it, one that avoids box-p does not
        let waiting = crate::state::Live { pin: "box-p".into(), node: String::new(), needs: String::new(), ..x.clone() };
        assert!(super::answers(&waiting, "lane-x", "pkg-x", true, false, &args, "", "", "", "", &[]));
        assert!(super::answers(&waiting, "lane-x", "pkg-x", true, false, &args, "", "box-p", "", "", &[]));
        assert!(!super::answers(&waiting, "lane-x", "pkg-x", true, false, &args, "", "", "box-p", "", &[]));
        let unplaced = crate::state::Live { node: String::new(), avoid: "box-c".into(), ..x.clone() };
        assert!(super::answers(&unplaced, "lane-x", "pkg-x", true, false, &args, "", "", "box-c", "", &[]), "not placed yet, it avoids the same");
        assert!(!super::answers(&unplaced, "lane-x", "pkg-x", true, false, &args, "", "", "box-d", "", &[]), "not placed yet: it could still land there");
        let app = crate::state::Live { test: false, test_args: Vec::new(), ..x.clone() };
        let start = |needs: &str, pin: &str, avoid: &str| super::answers(&app, "lane-x", "pkg-x", false, false, &[], needs, pin, avoid, "", &[]);
        assert!(start("os=linux,gpu=1", "", "") && !start("", "", "") && !start("os=linux,gpu=1", "box-a", "") && !start("os=linux,gpu=1", "", "box-c"), "an app: placed exactly as it is");
        // another JIT or other env is another session: a saved JIT state replays in its own
        let jit = crate::state::Live { loader: "0123456789abcdef01234567".into(), env: vec![("RELAX_X".into(), "1".into())], ..x.clone() };
        let env = [("RELAX_X".to_string(), "1".to_string())];
        assert!(super::answers(&jit, "lane-x", "pkg-x", true, false, &args, "", "", "", "0123456789abcdef01234567", &env), "the same JIT and env");
        assert!(!super::answers(&jit, "lane-x", "pkg-x", true, false, &args, "", "", "", "", &env), "the default JIT is another session");
        assert!(!super::answers(&jit, "lane-x", "pkg-x", true, false, &args, "", "", "", "0123456789abcdef01234567", &[]), "without its env");
        assert!(!super::answers(&x, "lane-x", "pkg-x", true, false, &args, "", "", "", "0123456789abcdef01234567", &[]), "a JIT asked of a session on the default one");
    }

    #[test]
    fn another_lanes_session_is_refused_with_the_owners_name() {
        let e = super::refuse("W257", "fablook3", "smallfix").unwrap_err();
        assert!(e.contains("W257") && e.contains("fablook3") && e.contains("--lane fablook3"), "{e}");
        assert!(super::refuse("W257", "fablook3", "FabLook3").is_ok());
        assert!(super::refuse("W1", "", "x").is_ok());
    }

    #[test]
    fn a_verdict_said_again_after_a_master_restart_is_marked_as_a_replay() {
        use crate::proto::Sev;
        use crate::state::{Live, LivePatch, LiveTests};
        let p = LivePatch { edit_ms: 1_794_748_617_828, verdict_ms: 1_794_748_621_977, outcome: "initial".into(), tests: Some(LiveTests { ran: 10, passed: 9, failed: 1, failures: vec!["keys_name_one_property".into()], ..Default::default() }), ..Default::default() };
        let line = format!("live W9: {}", super::verdict(&p).trim_end());
        let running = Live { id: "W9".into(), patch: Some(LivePatch { verdict_ms: 0, tests: None, ..p.clone() }), ..Default::default() };
        assert_eq!(super::verdict_said("W9", &running, &p, Sev::Warn, line.clone()), (Sev::Warn, line.clone()), "a run's first verdict is said as it is");
        let held = Live { patch: Some(p.clone()), ..running.clone() };
        let (sev, said) = super::verdict_said("W9", &held, &p, Sev::Warn, line.clone());
        assert_eq!(sev, Sev::Info, "a replay is no news: the inbox had the verdict");
        assert_eq!(said, format!("live W9: replayed after a master restart, the verdict of its run that ended 13:17:01Z: {}", super::verdict(&p).trim_end()), "the replay says so, and when its run ended");
        let jit_error = LivePatch { outcome: "jit_error".into(), tests: None, reason: "the JIT crashed".into(), ..p.clone() };
        let held = Live { patch: Some(jit_error.clone()), ..running.clone() };
        assert_eq!(super::verdict_said("W9", &held, &jit_error, Sev::Err, "JIT error".into()).0, Sev::Info, "a JIT error said again is a replay too");
        let next = LivePatch { edit_ms: p.edit_ms + 1, verdict_ms: p.verdict_ms + 5_000, ..p.clone() };
        assert_eq!(super::verdict_said("W9", &held, &next, Sev::Warn, line.clone()), (Sev::Warn, line), "a later edit's verdict is new");
    }

    #[test]
    fn an_edit_that_waits_for_a_restart_says_what_restarts_it() {
        let mut x = crate::state::Live { id: "W7".into(), node: "box-a".into(), src: "overlay".into(), state: "running".into(), lane: "lane-x".into(), agent: "AGENT-X".into(), app: "app-x".into(), patch: Some(crate::state::LivePatch { files: vec!["src/a.rs".into()], outcome: "restart".into(), reason: "2 files changed: the Splash VM or other provided sources changed; needs a JIT rebuild: restart it through cargo; keeping the running code".into(), ..Default::default() }), ..Default::default() };
        let text = super::render(&x);
        assert!(text.contains("`makepad devhub live act W7 session relaunch`"), "{text}");
        assert!(text.starts_with("W7 running app-x of lane-x (AGENT-X) on box-a pid 0"), "a session's line names whose it is: {text}");
        assert!(!text.contains("  on box-a: work"), "not placed: no work dir said: {text}");
        // placed: where it runs on its box, its home and pid, and how to copy its files back with no
        // lease of its own (a helper's probes each cost about 10 min of queue, 11-26)
        let placed = crate::state::Live { work_dir: "/w/lane/lane-x/run".into(), box_home: "/home/someone".into(), pid: 41, ..x.clone() };
        assert!(super::render(&placed).contains("  on box-a: work /w/lane/lane-x/run, home /home/someone, pid 41; its files: `devhub live get W7 <rel> <local-dir>`"), "{}", super::render(&placed));
        let own_home = crate::state::Live { home: "/a/scan.tar".into(), home_dir: "/w/tmp/L1/app-home".into(), ..placed.clone() };
        assert!(super::render(&own_home).contains(", home /w/tmp/L1/app-home (MAKEPAD_TEST_HOME, from --home), pid 41;"), "{}", super::render(&own_home));
        // an app session without --home has a fresh one; `--home box` says plainly it is the box's own
        // (a lane's Town opened a box's demo copy and signed a name into it, 11-27)
        let fresh = crate::state::Live { home_dir: "/w/tmp/L1/app-home".into(), ..placed.clone() };
        assert!(super::render(&fresh).contains(", home /w/tmp/L1/app-home (MAKEPAD_TEST_HOME, fresh: gone at its release), pid 41;"), "{}", super::render(&fresh));
        let own_box = crate::state::Live { home: super::BOX_HOME.into(), ..placed };
        let text = super::render(&own_box);
        assert!(text.contains(", home /home/someone (the box's own home, from --home box: its real app data), pid 41;") && text.contains("  home: the box's own home (--home box): its real app data\n"), "{text}");
        let p = x.patch.as_ref().unwrap();
        assert!(super::needs_devhub_rebuild(&x, p));
        assert!(!super::rebuild_tests(&x, p), "an app's state stays until its lane asks");
        assert!(super::devhub_relaunch(&x, "session relaunch"), "the node's host cannot rebuild its store binary");
        assert!(!super::devhub_relaunch(&x, "session"), "inspection does not restart");
        // --anyway: devhub's relaunch too, sent as the JIT's anyway=1; never without it
        assert!(super::devhub_relaunch(&x, "session relaunch --anyway") && super::devhub_relaunch(&x, "session relaunch anyway=1"));
        assert_eq!((super::relaunch_anyway("session relaunch"), super::relaunch_anyway("session relaunch --anyway"), super::relaunch_anyway("session  relaunch anyway=1")), (Some(false), Some(true), Some(true)));
        assert_eq!(super::relaunch_anyway("session relaunch process=1"), None, "a process=1 relaunch is its JIT's");
        assert_eq!(super::anyway_in_jit_words("session relaunch process=1 --anyway").as_deref(), Some("session relaunch process=1 anyway=1"), "one the JIT answers gets its words");
        assert_eq!(super::anyway_in_jit_words("session relaunch process=1"), None);
        assert_eq!(super::anyway_in_jit_words("ui find q=--anyway"), None, "only a relaunch's");
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
        // the tree's proc macros are not those the JIT has built in: a JIT built from the tree runs
        // it (a held harness restarted on the same binary and held again, W25685, 11-27)
        x.patch.as_mut().unwrap().reason = "crate substitution: the proc macros in this tree differ from those built into makepad: rebuild makepad by starting it through cargo".into();
        assert!(super::rebuild_tests(&x, x.patch.as_ref().unwrap()), "a run on stale built-in proc macros restarts on a rebuilt JIT");
        x.loader = "0123456789abcdef01234567".into();
        assert!(!super::rebuild_tests(&x, x.patch.as_ref().unwrap()), "a pinned JIT's mismatch is its refusal");
        x.loader.clear();
        // --fresh: a real quit, then a new process on the same home and lease, devhub's for any app
        // session; never the JIT's, never sent twice after a cut (CREATE's offline walks, 11-27)
        assert!(["session relaunch --fresh", "session relaunch --fresh anyway=1", "session  relaunch --anyway --fresh"].iter().all(|c| super::fresh_relaunch(c)), "--fresh, with or without --anyway");
        assert!(["session relaunch", "session relaunch --fresh process=1", "ui find q=--fresh"].iter().all(|c| !super::fresh_relaunch(c)), "only a relaunch's --fresh, with nothing its JIT reads beside it");
        assert_eq!(super::relaunch_anyway("session relaunch --fresh"), None, "a fresh relaunch is no relaunch in place");
        assert!(!super::RESENT.iter().any(|c| "session relaunch --fresh".split_whitespace().eq(c.iter().copied())), "a cut fresh relaunch may have quit the app: it is never sent again");
        let old = crate::state::Live { lease: "L1".into(), run_n: 3, ..fresh.clone() };
        let up = crate::state::Live { pid: 57, run_n: 4, ..old.clone() };
        for (now, why) in [(crate::state::Live { pid: 0, ..old.clone() }, "the old process gone, no new one yet"), (old.clone(), "the old process still up"), (crate::state::Live { run_n: 4, ..old.clone() }, "a new run with no pid of its own yet")] {
            assert_eq!(super::fresh_said(&old, &now, super::Stopped::Quit(1_400)), None, "{why}: it waits");
        }
        assert_eq!(super::fresh_said(&old, &up, super::Stopped::Quit(1_400)).as_deref(), Some("live W7: pid 41 quit and exited in 1.4 s; pid 57 runs on the same home /w/tmp/L1/app-home, lease L1 kept"), "a new pid on the same home and lease");
        let said = super::fresh_said(&old, &up, super::Stopped::Killed).unwrap();
        assert!(said.contains("pid 41 did not exit within 10 s of its quit, so it was stopped (its quit did not finish)"), "a quit that did not finish is said: {said}");
        let env_home = crate::state::Live { home_dir: String::new(), env: vec![("MAKEPAD_TEST_HOME".into(), "/w/own".into())], ..up.clone() };
        assert!(super::fresh_said(&old, &env_home, super::Stopped::Ended).unwrap().contains("pid 41 had ended already; pid 57 runs on the same home /w/own,"), "an --env home is the one it keeps");
    }

    #[test]
    fn a_test_round_labels_earlier_failures_and_recognises_a_restart_offer() {
        use crate::state::{LivePatch, LiveTests};
        let why = "src_stamp.rs: changed, and the code that includes it makes its path from other tokens (no item's text or expansion names it): which code read it is not known";
        let log = format!("relax live: {why}: a restart applies it; offered (Update ready on the JIT ring, the WM's toast, or `makepad inspect <app> session relaunch`); the app keeps running meanwhile\nrelax live:   no loaded source items changed\nrelax live: .../out/src_stamp.rs: patched (0 item(s) changed, 0 fn(s) installed) in 0.0 ms\n");
        assert_eq!(super::restart_offered(&log).as_deref(), Some(why));
        assert_eq!(super::restart_offered("relax live: .../out/src_stamp.rs: patched (0 item(s) changed, 0 fn(s) installed) in 0.0 ms\n"), None);
        assert_eq!(super::restart_offered("relax live: the tests crashed: Restart runs them; offered (Update ready on the JIT ring)\n"), None);
        let failures = ["a", "b", "c", "d"].map(String::from).to_vec();
        let p = LivePatch { tests: Some(LiveTests { ran: 4, passed: 196, failed: 4, failures: failures.clone(), stale: failures, ..Default::default() }), ..Default::default() };
        let out = super::verdict(&p);
        assert!(out.starts_with("verdict 4/4 passed, 0 failed ("), "{out}");
        assert!(out.contains("  4 earlier failure(s), not rerun for this edit (its changes reach none of them): a, b, c, d\n"), "{out}");
        for name in ["a", "b", "c", "d"] { assert!(!out.contains(&format!("\n  {name}\n")), "an earlier failure is not fresh: {out}"); }
    }

    #[test]
    fn a_session_started_on_an_outdated_jit_gives_its_first_verdict_from_the_current_one() {
        use super::{jit_restart, run_jit_key, verdict, JitRestart};
        use crate::state::{Doc, Live, LivePatch};
        let why = "The JIT itself changed (makepad_hal) -- Restart to update";
        let offer = format!("relax live: {why}: a restart applies it; offered (Update ready on the JIT ring)\n");
        let old = format!("devhub: the JIT is this box's dev binary K-old\n{offer}");
        let pending = match jit_restart(&old, None) {
            JitRestart::Restart(key, why) => Some((key, why)),
            other => panic!("the outdated JIT must give no verdict and request a restart: {other:?}"),
        };
        assert_eq!(pending, Some(("K-old".into(), why.into())), "the pending restart keeps the selected key and why");
        let saved = Doc { lives: vec![Live { jit_restart: pending.clone(), ..Default::default() }], ..Default::default() };
        let restored = Doc::from_value(&saved.to_value());
        let pending = restored.lives[0].jit_restart.as_ref();
        assert_eq!(pending, saved.lives[0].jit_restart.as_ref(), "a master restart keeps the pending JIT restart");

        let current = "devhub: the JIT is this lane's own build K-new\n{\"group\":0,\"ran\":1,\"passed\":1,\"failed\":0,\"ms\":0}\n";
        let prefix = match jit_restart(current, pending) {
            JitRestart::Verdict(line) => line,
            other => panic!("the replacement JIT must give the first verdict: {other:?}"),
        };
        let tests = current.lines().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).find_map(|v| crate::state::live_tests_from(&v));
        assert!(tests.is_some(), "the current run supplies the passing test result");
        let p = LivePatch { tests, jit_notice: format!("{prefix}\n"), ..Default::default() };
        let first = format!("restarted on the current JIT K-new: {why}");
        let out = verdict(&p);
        assert_eq!(out.lines().next(), Some(first.as_str()), "the first verdict starts with the current key and original why");
        assert_eq!(out.matches(&first).count(), 1, "the verdict says why it restarted exactly once");
        assert!(out.contains("verdict 1/1 passed, 0 failed"), "the verdict is the replacement's passing result: {out}");
        let saved = Doc { lives: vec![Live { state: "running".into(), patch: Some(p), ..Default::default() }], ..Default::default() };
        let restored = Doc::from_value(&saved.to_value());
        assert_eq!(restored.lives[0].test_result(0.0).lines().next(), Some(first.as_str()), "the caller sees the same prefix after persistence");
        assert_eq!(jit_restart(current, None), JitRestart::Unchanged, "a consumed restart adds no notice to later verdicts");

        for log in [&old, &old.replace("K-old", "K-new"), &current.replace("K-new", "K-old")] {
            let line = match jit_restart(log, pending) {
                JitRestart::Terminal(line) => line,
                other => panic!("the same key or a repeated offer must end in a JIT error without another restart: {other:?}"),
            };
            let key = run_jit_key(log).unwrap();
            assert_eq!(line, format!("the current JIT {key} still says: {why}: devhub's JIT key misses what changed (a devhub bug)"), "the terminal verdict identifies the key bug");
            let p = LivePatch { outcome: "jit_error".into(), reason: line.clone(), jit_notice: format!("{line}\n"), ..Default::default() };
            assert_eq!(verdict(&p).lines().next(), Some(line.as_str()), "the JIT error formatter exposes the diagnosis");
            let x = Live { state: "running".into(), patch: Some(p), ..Default::default() };
            assert_eq!(x.test_code(), 3, "the terminal result is a JIT error, never a pass");
        }
        let other = "relax live: an include changed: a restart applies it; offered (Update ready on the JIT ring)\n";
        assert_eq!(jit_restart(other, None), JitRestart::Unchanged, "other restart offers keep their existing handling");
    }

    #[test]
    fn a_log_window_ends_after_its_last_whole_line() {
        // read from 100 to 140: the next window starts where the half-written line does
        let bytes = b"test a ... ok\nverdict\nrelax live:   struct ";
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
        // an app that waits for a fix takes the save as its next run's: never the session's end
        let held = "{\"err\":\"held: no run: the last one did not compile; the next save or `session relaunch` starts the next one\"}".to_string();
        assert!(super::session_over(&format!("live: inputs: {held}"), "L1").is_none(), "a held app's answer ends no session");
        assert!(super::held_app(&[], &[held]), "the inputs answer says held");
        assert!(super::held_app(&["{\"now_ms\":1.0,\"held\":1,\"editing\":0}".into()], &[]), "the live look says held");
        assert!(!super::held_app(&["{\"now_ms\":1.0,\"held\":0,\"editing\":0}".into()], &["{\"count\":0}".into()]), "a running app is not held");
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
    let (arg, resend) = match arg.strip_suffix(RESEND) {
        Some(a) if RESENT.iter().any(|c| a.split_whitespace().eq(c.iter().copied())) => (a.trim_end(), true),
        Some(_) => return Err(format!("{id}: {RESEND} is for a call devhub can look at: {}", RESENT.iter().map(|c| c.join(" ")).collect::<Vec<_>>().join(", "))),
        None => (arg, false),
    };
    // (`--anyway` is devhub's word for the JIT's `anyway=1`)
    let jit_words = anyway_in_jit_words(arg);
    let arg = jit_words.as_deref().unwrap_or(arg);
    if let Some(line) = resend.then(|| relaunch_under_way(&x)).flatten() {
        return Ok(format!("{{\"ms\":0,\"png\":null,\"lines\":[{}]}}", json_str(&line)));
    }
    if x.pid == 0 {
        // (and why, when it starts again: what its app held is gone)
        let why = if x.note.is_empty() { String::new() } else { format!(": {}", x.note) };
        return Err(format!("{id}: its JIT is not up yet{why} (live wait {id} frame)"));
    }
    try_set(ctx, id, |s| s.touched_ms = wall_ms())?;
    let t = Instant::now();
    // a grab's file: (node, lease, path under the lease's dir), and every frame the call drew, here
    let (mut png, mut paths) = (None, Vec::new());
    let mut lines = match arg.split_whitespace().next() {
        _ if fresh_relaunch(arg) && x.test => return Err(format!("{id}: --fresh relaunches an app session: a test session's runs restart on their own")),
        _ if fresh_relaunch(arg) => relaunch(ctx, id, false, true, true)?,
        _ if devhub_relaunch(&x, arg) => relaunch(ctx, id, resend, relaunch_anyway(arg) == Some(true), false)?,
        // a resent relaunch its JIT serves: sent only while an edit still waits for it
        _ if resend && restarts_waiting(&x)? == 0 => relaunch_applied(id, x.pid, 0).into_iter().collect(),
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
            let files = grab_files(&lines, &r.lease);
            if let Some((rel, _)) = files.first() {
                let drawn = drawn.map_or(String::new(), |(frame, patch)| format!(",\"frame\":{frame},\"patch\":{patch}"));
                png = Some(format!("{{\"node\":{},\"lease\":{},\"rel\":{}{drawn}}}", json_str(&x.node), json_str(&r.lease), json_str(&format!("tmp/{rel}"))));
            }
            // the frames come here, as a look's does: left on the box, a gseq's were gone with
            // the session (a path camera's pop could not be shown, 11-23)
            if !files.is_empty() {
                match fetch_frames(ctx, &x, &r.lease, &files) {
                    Ok(got) => {
                        lines.push(frames_said(&got));
                        paths = got;
                    }
                    Err(e) => lines.push(format!("its {} frame(s) stay on {}: {e}; copy one: makepad devhub get {} tmp/{} <dir> --node {} --lease {}", files.len(), x.node, x.lane, files[0].0, x.node, r.lease)),
                }
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
    let paths: Vec<String> = paths.iter().map(|p| json_str(&p.to_string_lossy())).collect();
    Ok(format!("{{\"ms\":{},\"png\":{},\"paths\":[{}],\"lines\":[{}]}}", t.elapsed().as_millis(), png.as_deref().unwrap_or("null"), paths.join(","), lines.iter().map(|l| json_str(l)).collect::<Vec<_>>().join(",")))
}

/// Copies the frames a call drew ([`grab_files`]) off the lease's tmp dir into the lane's
/// `local/screencap/<lane>/`, named as a look's frames are ([`frame_paths`]): their paths here.
fn fetch_frames(ctx: &Ctx, x: &Live, lease: &str, files: &[(String, Option<(u64, u64)>)]) -> Result<Vec<PathBuf>, String> {
    let dir = ctx.repo.join("local/screencap").join(&x.lane);
    let had = std::fs::read_dir(&dir).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{}-", x.id))).count()).unwrap_or(0);
    let drawn: Vec<Option<(u64, u64)>> = files.iter().map(|(_, d)| *d).collect();
    let mut got = Vec::new();
    for ((rel, _), path) in files.iter().zip(frame_paths(&dir, &x.id, had, &drawn)) {
        let fetched = ctx.leases.fetch(&x.node, &x.lane, lease, &format!("tmp/{rel}"), &dir)?;
        std::fs::rename(&fetched[0].0, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        got.push(path);
    }
    Ok(got)
}

/// Where a call's frames landed, for its caller.
fn frames_said(paths: &[PathBuf]) -> String {
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match paths {
        [one] => format!("its frame: {}", one.display()),
        [first, .., last] => format!("its {} frames: {} .. {} in {}", paths.len(), name(first), name(last), first.parent().map(|d| d.display().to_string()).unwrap_or_default()),
        [] => String::new(),
    }
}

/// A look's naming of session `id`'s frames in `dir`, after the `had` of its there:
/// `<id>-<n>[-f<frame>-p<patch>].png`, numbered on, each stamped with what it was drawn at.
fn frame_paths(dir: &Path, id: &str, had: usize, drawn: &[Option<(u64, u64)>]) -> Vec<PathBuf> {
    drawn.iter().enumerate().map(|(i, d)| dir.join(format!("{id}-{}{}.png", had + i + 1, d.map_or(String::new(), |(f, p)| format!("-f{f}-p{p}"))))).collect()
}

/// A `live wait`'s timeout when it names none, s.
pub(crate) const WAIT_S: u64 = 120;

/// `live wait`'s words `<what> [timeout_s] [since=<ms>]`: what it waits for, its timeout, what is
/// left of it at `now` and when it was asked. `since` is when the client first asked
/// ([`crate::cli::retry_master_until`]): an ask again after a restart cut the call counts from
/// it. Each ask again got the whole timeout anew, and a `live wait <id> verdict 900` ran 29 min
/// past it through restarts (11-17).
pub(crate) fn wait_args(arg: &str, now: u64) -> (&str, Duration, Duration, u64) {
    let mut words = arg.split_whitespace();
    let what = words.next().unwrap_or("patched");
    let (mut timeout, mut since) = (WAIT_S, None);
    for w in words {
        match w.strip_prefix("since=") {
            Some(ms) => since = ms.parse::<u64>().ok().filter(|ms| *ms <= now),
            None => timeout = w.parse().unwrap_or(timeout),
        }
    }
    let asked = since.unwrap_or(now);
    let timeout = Duration::from_secs(timeout);
    (what, timeout, timeout.saturating_sub(Duration::from_millis(now - asked)), asked)
}

/// How often a wait touches its lane's other running sessions ([`touch_lane`]).
const LANE_TOUCH_EVERY: Duration = Duration::from_secs(30);

/// A lane's wait on one of its sessions is its use of the lane's other running sessions: a walk
/// that waited 10 min for its Fab's first frame lost its two Towns to the idle end before it could
/// drive them again (W24524, W24525, 11-26). The wait ends ([`wait_args`]), so a session its lane
/// forgot stays only while the lane waits on another. It is their last act ([`acted`]).
fn touch_lane(ctx: &Ctx, x: &Live) {
    if x.lane.is_empty() { return; }
    let now = wall_ms();
    let _ = ctx.store.update(|d| {
        for o in d.lives.iter_mut().filter(|o| o.id != x.id && o.lane == x.lane && o.active()) {
            o.touched_ms = o.touched_ms.max(now);
            (o.last_act, o.last_act_ms) = (format!("its lane's wait on {}", x.id), now);
        }
    });
}

/// What a lane did with a session, as its last act and when: what a park for timed work while it
/// is in use says it interrupted ([`park`]), so a walk's owner sees why its run broke.
fn acted(ctx: &Ctx, id: &str, what: String) {
    let now = wall_ms();
    let _ = try_set(ctx, id, |s| if s.active() { (s.last_act, s.last_act_ms) = (what, now); });
}

/// A park's line for session `x` at `now`: one parked for timed work while in use (`resumes`) says
/// the work it interrupted, its last act ([`acted`]), then that it starts again by itself.
fn park_note(note: &str, x: Option<&Live>, resumes: bool, now: u64) -> String {
    if !resumes { return note.to_string(); }
    let act = x.filter(|x| !x.last_act.is_empty())
        .map(|x| format!("; in use: its last act was {}, {} before", x.last_act, crate::lease::sched::dur(now.saturating_sub(x.last_act_ms))))
        .unwrap_or_default();
    format!("{note}{act}; {RESTARTS_BY_ITSELF}")
}

fn wait<'a>(ctx: &'a Arc<Ctx>, id: &'a str, arg: &str, held: &mut Option<Call<'a>>) -> Result<String, String> {
    let (what, timeout, left, asked) = wait_args(arg, wall_ms());
    let t = Instant::now();
    let x = try_set(ctx, id, |s| s.touched_ms = wall_ms())?.ok_or_else(|| format!("no live job {id}"))?;
    if what == "verdict" && !x.test { return Err(format!("{id}: verdict needs a live test job (--test)")); }
    let signal = ctx.store.subscribe("live", &[id]);
    let mut lane_touched: Option<Instant> = None;
    loop {
        let seen = signal.generation();
        if held.is_none() { *held = call_until(ctx, id, Some(t + left))?; }
        // a test session ships its saves on its next call (`live sync`), never on a wait:
        // waiting for them waited to the timeout; the answer says they are there
        let (quiet, unshipped) = if let Some(r) = running(id) {
            let pending = r.pending.lock().unwrap();
            let inflight = r.inflight.load(std::sync::atomic::Ordering::SeqCst);
            if r.test { (!inflight, pending.0.len()) } else { (pending.0.is_empty() && !inflight, 0) }
        } else { (true, 0) };
        let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
        // a session that ended or failed answers its waits at once, with why: a test session's
        // verdict wait has its answer in the record; any other wait has none, and answered as met
        // (exit 0) a lane's script waited on as if its app ran (W12708, 11-18)
        if !x.active() {
            return if what == "verdict" { Ok(crate::state::live_value(&x).to_json()) } else { Err(no_frame(&x)) };
        }
        if lane_touched.is_none_or(|t| t.elapsed() >= LANE_TOUCH_EVERY) {
            touch_lane(ctx, &x);
            lane_touched = Some(Instant::now());
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
        // an app whose sources do not build draws nothing before its next save: its wait for the
        // app's frame ends with why at once ([`unbuilt`])
        if let Some(why) = matches!(what, "frame" | "settled").then(|| unbuilt(&x)).flatten() {
            return Err(format!("live wait {id} {what}: {why}"));
        }
        if t.elapsed() > left {
            let box_note = if held.is_none() && x.pid != 0 { "; the restarted master has not taken its box back" } else { "" };
            return Err(format!("live wait {id} {what}: not within {} s{box_note}", timeout.as_secs()));
        }
        // Observe frame/idle transitions on the session's inspection deadline;
        // document and admission changes wake just this session immediately.
        let deadline = if t.elapsed() < Duration::from_millis(300) { Duration::from_millis(300) - t.elapsed().min(Duration::from_millis(300)) } else { Duration::from_millis(250) };
        signal.wait(seen, deadline.min(left.saturating_sub(t.elapsed())));
    }
}

/// Why a session's app draws no frame before its next save: the newest build of its sources was
/// refused, its first (the session's build failed) or a later save's. A lane's wait for its Town's
/// first frame sat 21 min on a build that failed (E0027) and a save refused on the same error,
/// through a master restart (WORKBOARD, 11-27). An app that draws already has its frame: this is
/// asked only of one that does not.
fn unbuilt(x: &Live) -> Option<String> {
    let p = x.patch.as_ref().filter(|p| matches!(p.outcome.as_str(), "refused" | "compile_error"))?;
    let reason = if p.reason.is_empty() { p.reason_ref.read().unwrap_or_default() } else { p.reason.clone() };
    let first = |t: &str| t.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string);
    let why = first(&reason).or_else(|| first(&p.edit)).unwrap_or_else(|| p.outcome.clone());
    Some(format!("{} draws no frame before its next save: its sources do not build: {}", x.id, crate::proto::clean(&why, 300)))
}

/// Hold the session through both waits, the grab and its copy off the lease's scratch dir.
/// Why a look has no frame: a session that ended (parked for another ask's RAM, idle, released)
/// says how and the command that starts it again; one not up yet, what it waits on. A look
/// answered a parked session with no frame and no word, so every look of a proof after the park
/// came back empty (E3D, 11-15).
fn no_frame(x: &Live) -> String {
    let note = if x.note.is_empty() { String::new() } else { format!(": {}", x.note) };
    if x.active() { return format!("{} is {}{note}: no frame yet; look again once it runs", x.id, x.state); }
    let again = if x.note.contains("live again") { String::new() } else { format!("; `devhub live again {}` starts it again", x.id) };
    format!("{} {}{note}{again}", x.id, x.state)
}

fn look(ctx: &Arc<Ctx>, id: &str, timeout: &str) -> Result<String, String> {
    let mut held = call(ctx, id)?;
    let mut x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
    if x.test { return Err(format!("{id} runs tests: live wait {id} verdict")); }
    for what in ["patched", "frame"] {
        let answer = wait(ctx, id, &format!("{what} {timeout}"), &mut held)?;
        x = crate::state::live_from(&makepad_strict_json::parse(answer.as_bytes()).map_err(|e| e.to_string())?);
        if x.state != "running" { return Err(no_frame(&x)); }
    }
    // (the act copies its frame here: [`fetch_frames`])
    let answer = act(ctx, id, "grab", held.as_ref())?;
    let v = makepad_strict_json::parse(answer.as_bytes()).map_err(|e| e.to_string())?;
    let lines = v.get("lines").cloned().unwrap_or(Value::Arr(Vec::new()));
    let Some(path) = v.get("paths").and_then(Value::as_arr).and_then(|p| p.first()).and_then(Value::as_str).map(PathBuf::from) else {
        return Err(format!("the grab brought no file here: {}", lines.as_arr().unwrap_or(&[]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | ")));
    };
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
    if matches!(k, "CARGO_TARGET_DIR" | "MAKEPAD_HIDE_WINDOWS") || ["VFS", "WORKSPACE", "EDITS", "HOME", "STORE_LIVE"].iter().any(|v| crate::run_names::is(k, v)) {
        return Err(format!("live start: --env {k} is managed by devhub"));
    }
    Ok(())
}

/// Whether running session `x` is what a `live start`/`live test` asks for, and the call goes to
/// it: the same lane, program, test selection and placement (its `--needs`, in any order, and its
/// box). One that differs leaves it be and starts its own. A `--needs os=macos` test went to the
/// lane's session on a Linux box, and --live takes no --node (W7272, 11-14).
/// Offline and networked requests never share a session (11-15).
/// A join runs on its partner's box. Inside a set the scheduler places them together, so the
/// partner may still be queued and have no box yet.
fn joined_box(lives: &[Live], joins: &str, lane: &str, asked: &str, set: &str) -> Result<String, String> {
    let j = lives.iter().find(|x| x.id == joins).ok_or_else(|| format!("live: --joins {joins}: no live session {joins}"))?;
    if j.lane != lane { return Err(format!("live: --joins {joins}: it is lane {}'s session; a session joins one of its own lane's", j.lane)); }
    if j.set != set { return Err(format!("live: --joins {joins}: a member joins a member of its own set")); }
    if !j.active() { return Err(format!("live: --joins {joins}: it {}{}; join the session that starts it again", j.state, if j.note.is_empty() { String::new() } else { format!(": {}", j.note) })); }
    if j.node.is_empty() && !set.is_empty() { return Ok(asked.into()); }
    if j.node.is_empty() { return Err(format!("live: --joins {joins}: it still waits for its box; join it once it runs there")); }
    if !asked.is_empty() && asked != j.node { return Err(format!("live: --node {asked} and --joins {joins} name two boxes: {joins} runs on {}", j.node)); }
    Ok(j.node.clone())
}

/// Whether an active session joins `id` ([`Live::joins`]): `id` does not end idle meanwhile.
fn joined(lives: &[Live], id: &str) -> bool {
    lives.iter().any(|x| x.active() && x.joins == id)
}

/// A session asked with another JIT (`--jit`) or other `--env` is not one either: its call ran in
/// the lane's session under its old JIT and environment, the asked ones dropped unsaid, and a saved
/// JIT state could not be replayed (11-18).
/// The kind and needs of session `x`'s lease. A timed one's (`--timed`) is timed: it waits for a
/// drained box and holds it while it lives, as a timed lease does ([`crate::lease::sched`]'s
/// `parks_for`), and a visible one needs a display there (`gui=1`), as a screen lease has one.
/// Else a visible one's is a screen lease, a headless one's a test lease.
pub(crate) fn lease_kind(x: &Live) -> (Kind, String) {
    let needs = if x.timed && x.visible {
        let mut needs = crate::lease::sched::parse_needs(&x.needs);
        if !needs.iter().any(|(k, _)| k == "gui" || k == "display") { needs.push(("gui".to_string(), "1".to_string())); }
        needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
    } else {
        x.needs.clone()
    };
    (if x.timed { Kind::Timed } else if x.visible { Kind::Screen } else { Kind::Test }, needs)
}

fn answers(x: &Live, lane: &str, app: &str, test: bool, offline: bool, test_args: &[String], needs: &str, pin: &str, avoid: &str, loader: &str, env: &[(String, String)]) -> bool {
    let placed = |s: &str| {
        let mut n = crate::lease::sched::parse_needs(s);
        n.sort();
        n
    };
    let avoided = |s: &str| {
        let mut n = crate::lease::sched::avoid_list(s);
        n.sort();
        n
    };
    x.lane == lane && x.app == app && x.test == test && x.offline == offline && x.test_args == test_args && x.loader == loader && x.env == env
        && if test { suits(x, needs, pin, avoid) } else { placed(&x.needs) == placed(needs) && x.pin == pin && avoided(&x.avoid) == avoided(avoid) }
}

/// Whether the lane's live test session `x` suits a call's placement, so the call runs in it
/// rather than in a new session that replaces it. A call that names no placement takes the
/// session wherever it runs. One that names a box takes a session on it (or, not placed yet,
/// asked for it). One that avoids boxes takes a session on none of them (not placed yet: one that
/// avoids them too). One with needs takes a session asked with the same. (A session started with
/// `--node` was replaced by the lane's next `test --live`, which named none, 11-17.)
fn suits(x: &Live, needs: &str, pin: &str, avoid: &str) -> bool {
    let set = |s: &str| {
        let mut n = crate::lease::sched::parse_needs(s);
        n.sort();
        n
    };
    let at = if x.node.is_empty() { x.pin.as_str() } else { x.node.as_str() };
    let own = crate::lease::sched::avoid_list(&x.avoid);
    (needs.is_empty() || set(&x.needs) == set(needs))
        && (pin.is_empty() || at == pin)
        && crate::lease::sched::avoid_list(avoid).iter().all(|a| a != at && (!x.node.is_empty() || own.contains(a)))
}

/// `live start {"app","repo_dir","tree"?,"lane","agent","needs","node"?,"avoid"?,"series"?,"pin_why"?,"visible","test","offline"?,"test_args","env"?}`, `live act <id> <cmd>`,
/// `live wait <id> patched|verdict|settled|frame [timeout_s]`, `live sync|release|get <id>`, `live list [all]`.
pub fn serve(ctx: &Arc<Ctx>, line: &str) -> Result<String, String> {
    serve_with(ctx, line, &makepad_remote::lines::say_waiting, &spawn_run)
}

fn serve_with(ctx: &Arc<Ctx>, line: &str, say: &dyn Fn(&str), launch: &dyn Fn(&Arc<Ctx>, Claim, bool) -> Result<(), (std::io::Error, Claim)>) -> Result<String, String> {
    StartupGate::wait_ready();
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
    if matches!(verb, "act" | "wait" | "look" | "sync") {
        let arg: String = arg.chars().take(80).collect();
        acted(ctx, id, format!("`live {verb} {id}{}{arg}`", if arg.is_empty() { "" } else { " " }));
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
            let timed = v.get("timed").map(|t| t.as_bool().ok_or("live: timed must be a bool")).transpose()?.unwrap_or(false);
            let src = if v.get("tree").and_then(Value::as_bool).unwrap_or(false) { Src::Tree } else { Src::Overlay };
            let test = verb == "test" || v.get("test").and_then(Value::as_bool).unwrap_or(false);
            let offline = v.get("offline").map(|v| v.as_bool().ok_or("live: offline must be a bool")).transpose()?.unwrap_or(false);
            let mut test_args: Vec<String> = v.get("test_args").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|a| a.as_str().map(str::to_string).ok_or("live: test_args must be strings")).collect::<Result<_, _>>()?;
            {
                let select = &test_args[..test_args.iter().position(|a| a == "--").unwrap_or(test_args.len())];
                if test {
                    if !test_select_ok(select) {
                        return Err("live: select --lib, --bin B or --test T, then any --no-default-features or --features F, before the libtest -- arguments".into());
                    }
                } else if !select.is_empty() && !matches!(select, [f, t] if f == "--bin" && !t.is_empty() && !t.starts_with('-')) {
                    return Err("live: a program takes --bin B before its -- arguments; a test selection needs test=true".into());
                }
            }
            if test_args.last().is_some_and(|a| a == "--") { test_args.pop(); }
            // `--home`: files the app finds in a home of its own on its box (a capture to look at
            // was put into the box user's home by a job pinned to the session's box, 11-13)
            let home = s("home");
            if home == BOX_HOME {
                if test { return Err("live: --home box runs an app on its box's own home; a test's app has its harness's own".into()); }
            } else if !home.is_empty() {
                if test { return Err("live: --home seeds an app's home; a test's app has its harness's own".into()); }
                if !Path::new(&home).is_absolute() || !Path::new(&home).is_file() { return Err(format!("live: --home {home} is not an archive on this machine")); }
                if env.iter().any(|(k, _)| k == "MAKEPAD_TEST_HOME") { return Err("live: --home is the app's MAKEPAD_TEST_HOME; drop the --env one".into()); }
            }
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
            let visible = v.get("visible").and_then(|b| b.as_bool()).unwrap_or(false);
            // `--joins W<n>`: on that session's box, which a visible one must take
            let joins = s("joins");
            let set_name = s("set");
            if v.get("set").is_some_and(|v| v.as_str().is_none()) || !set_name.is_empty() && crate::lease::sched::parse_set(&set_name).is_none() {
                return Err(format!("live: {SET_USAGE}"));
            }
            let pin = if joins.is_empty() { s("node") } else {
                let node = ctx.store.read(|d| joined_box(&d.lives, &joins, &lane, &s("node"), &set_name))?;
                if let Some(why) = ctx.leases.screen_refusal(&node).filter(|_| visible && !node.is_empty()) {
                    return Err(format!("live: --joins {joins}: it runs on {node}, where a visible session would wait with no end: {why}; release {joins} and start it again on a box with a display (`--node <box>`), then join it"));
                }
                if set_name.is_empty() { node } else { s("node") }
            };
            let needs = s("needs");
            // placed as a lease is, by a lease's flags and rules ([`Req::place`])
            let (avoid, series, pin_why) = (s("avoid"), s("series"), s("pin_why"));
            let mut asked = crate::lease::sched::Req::new(&lane, Kind::Test);
            asked.place(&pin, &needs, &avoid, &series, &pin_why);
            asked.check_series().map_err(|e| format!("live: {e}"))?;
            // Each explicit set start creates a member, even for two copies of the same app.
            if let Some(old) = ctx.store.read(|d| d.lives.iter().find(|x| x.active() && set_name.is_empty() && x.set.is_empty() && answers(x, &lane, &app, test, offline, &test_args, &needs, &pin, &avoid, &s("loader"), &env) && (accepting(&x.id) || verb == "test" && !x.held.is_empty())).cloned()) {
                if verb == "test" && !old.held.is_empty() && !accepting(&old.id) {
                    drop(starting);
                    let signal = ctx.store.subscribe("live", &[&old.id]);
                    loop {
                        let seen = signal.generation();
                        if get(ctx, &old.id).is_none_or(|x| !x.active()) || accepting(&old.id) { break; }
                        if makepad_remote::lines::client_gone() { return Err("the test caller went away".into()); }
                        signal.wait(seen, Duration::from_secs(1));
                    }
                    return serve_with(ctx, line, say, launch);
                }
                if verb == "test" {
                    // (a call from another checkout read the session's log, its run before the
                    // edit, as this call's verdict: ELANG took W9372's 221 for stale code, 11-15)
                    if old.tree != tree { return Err(format!("{} belongs to worktree {}, and this was asked from {tree}: run it from {}; nothing shipped or ran for this call (the session's log still ends with its last run)", old.id, old.tree, old.tree)); }
                    if old.src != src.as_str() { return Err(format!("{} uses {}; live release {} before changing its source mode", old.id, old.src, old.id)); }
                    if old.timed != timed { return Err(format!("{} is {}a timed session; live release {} before changing that", old.id, if old.timed { "" } else { "not " }, old.id)); }
                    // (other DEVHUB_ALSO paths than its session's ship from this call on, as changed
                    // sources do: a file named after the session started never reached its box, 11-26)
                    if old.also != also { try_set(ctx, &old.id, |s| s.also = also.clone())?; }
                    drop(starting);
                    let fresh = v.get("fresh").map(|f| f.as_bool().ok_or("live: fresh must be a bool")).transpose()?.unwrap_or(false);
                    return match test_call(ctx, &old.id, &s("check"), fresh)? { Some(answer) => Ok(answer), None => serve_with(ctx, line, say, launch) };
                }
                return Err(format!("{} already runs {app} for lane {lane} on {} (live release {} first)", old.id, old.node, old.id));
            }
            // (a lane's sessions are not counted: each asks for its box and waits in the queue for
            // room, [`run_claimed`])
            let priority = match s("priority").as_str() { "" => Priority::Interactive, p => Priority::parse(p).ok_or("priority is interactive or background")? };
            let cost = s("cost");
            if !cost.is_empty() && CostClass::parse(&cost).is_none() { return Err("cost is small, medium or large".into()); }
            let est_min = v.get("est_min").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|m| m.is_finite() && *m >= 0.0).ok_or("est_min is a nonnegative number")).transpose()?;
            let cores = v.get("cores").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("cores is a positive number")).transpose()?;
            let ram_gb = v.get("ram_gb").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("ram_gb is a positive number")).transpose()?;
            let (x, cold) = ctx.store.update(|d| {
                d.lives_next += 1;
                let x = Live { id: format!("W{}", d.lives_next), priority: priority.as_str().into(), loader: s("loader"), src: src.as_str().into(), cost: cost.as_str().into(), est_min, cores, ram_gb, class: s("class"), app: app.clone(), test, offline, test_args, env, also, home, lane: lane.clone(), agent: s("agent"), tree: tree.clone(), needs: s("needs"), pin: pin.clone(), avoid: avoid.clone(), series: series.clone(), pin_why: pin_why.clone(), joins, set: set_name, visible, timed, state: "starting".into(), touched_ms: wall_ms(), ..Default::default() };
                let cold = if test {
                    d.lives.iter_mut().rev().find(|p| p.test && p.app == app && p.lane == lane && p.set == x.set).and_then(|previous| cold_start(previous, &x))
                } else { None };
                d.lives.push(x.clone());
                (x, cold)
            })?;
            if let Some(line) = cold { say(&line); }
            // (its caller hears which of its lane's sessions of the package it ends, and when)
            let others: Vec<String> = ctx.store.read(|d| replaced(&d.lives, &x).into_iter().map(str::to_string).collect());
            if !others.is_empty() {
                say(&format!("live {}: lane {lane} has {} other live session{} of {app} ({}): each ends once its call does, one live test session per lane, package and system", x.id, others.len(), if others.len() == 1 { "" } else { "s" }, others.join(", ")));
            }
            retire_replaced(ctx, &x);
            let Some(claim) = try_claim_or_warn(ctx, &x.id, LaunchPath::Serve) else {
                return Err(format!("live {}: already started by another path", x.id));
            };
            drop(starting);
            event(ctx, Sev::Info, format!("live {}: {app} for lane {lane} asks for a box", x.id), Some(&x));
            let first_call = verb == "test";
            if let Err((e, claim)) = launch(ctx, claim, first_call) {
                let progress = end_progress(ctx, claim.id());
                try_set(ctx, claim.id(), |s| {
                    s.state = "failed".into();
                    s.ended_ms = wall_ms();
                    s.note = e.to_string();
                    settle_open_patch(s, s.ended_ms, progress.as_ref());
                })?;
                drop(claim);
            }
            if first_call { initial_call(ctx, &x.id, &s("check")) }
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
            if x.test { return test_call(ctx, id, "", false)?.ok_or_else(|| format!("{id}: parked; take a new live job")); }
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
        "checked" => {
            let words: Vec<_> = arg.split_whitespace().collect();
            let [caller, refusal, verdict @ ("confirmed" | "release")] = words.as_slice() else { return Err("live checked: invalid cargo verdict".into()) };
            let Some(r) = running(id) else { return Ok("{}".into()) };
            checked(ctx, id, &r.calls, caller, refusal, *verdict == "confirmed")
        }
        "release" => {
            let lease = release(ctx, id, "released")?;
            Ok(format!("{{\"id\":{},\"state\":\"released\",\"lease\":{}}}", json_str(id), json_str(&lease)))
        }
        // An ended session (parked for another ask's RAM, idle, released) starts again as it was
        // asked: its app or tests, box, env, source mode and priority, as a new session.
        "again" => {
            let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
            if x.active() { return Err(format!("{id} still runs ({}): nothing to start again", x.state)); }
            let strs = |v: &[String]| format!("[{}]", v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","));
            let env = x.env.iter().map(|(k, v)| format!("[{},{}]", json_str(k), json_str(v))).collect::<Vec<_>>().join(",");
            let req = format!(
                "{{\"app\":{},\"repo_dir\":{},\"tree\":{},\"lane\":{},\"agent\":{},\"needs\":{},\"node\":{},\"avoid\":{},\"series\":{},\"pin_why\":{},\"visible\":{},\"timed\":{},\"test\":{},\"offline\":{},\"test_args\":{},\"env\":[{env}],\"also\":{},\"home\":{},\"priority\":{},\"cost\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"class\":{},\"loader\":{},\"joins\":{},\"set\":{}}}",
                json_str(&x.app), json_str(&x.tree), x.src == Src::Tree.as_str(), json_str(&x.lane), json_str(&x.agent), json_str(&x.needs), json_str(if x.joins.is_empty() || !x.set.is_empty() { &x.pin } else { "" }), json_str(&x.avoid), json_str(&x.series), json_str(&x.pin_why), x.visible, x.timed, x.test, x.offline,
                strs(&x.test_args), strs(&x.also), json_str(&x.home), json_str(&x.priority), json_str(&x.cost), x.est_min.map_or("null".to_string(), |m| m.to_string()), x.cores.map_or("null".to_string(), |n| n.to_string()), x.ram_gb.map_or("null".to_string(), |n| n.to_string()), json_str(&x.class), json_str(&x.loader), json_str(&x.joins), json_str(&x.set),
            );
            serve_with(ctx, &format!("live {} {req}", if x.test { "test" } else { "start" }), say, launch)
        }
        // `devhub clean <lane>`: the lane is done, so are its live jobs
        "end-lane" => {
            if !makepad_devhub_conf::valid_name(rest) { return Err(format!("live end-lane: `{rest}` is not a lane")); }
            let ids: Vec<String> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active() && x.lane == rest).map(|x| x.id.clone()).collect());
            for id in &ids {
                event(ctx, Sev::Info, format!("live {id} of lane {rest}: released, the lane was cleaned"), get(ctx, id).as_ref());
                release(ctx, id, "released: its lane was cleaned")?;
            }
            ctx.store.update(|d| {
                for x in d.lives.iter_mut().filter(|x| x.lane == rest && !x.active()) { x.held.clear(); }
            })?;
            Ok(format!("{{\"released\":[{}]}}", ids.iter().map(|i| json_str(i)).collect::<Vec<_>>().join(",")))
        }
        "get" => get(ctx, id).map(|x| { let now = wall_ms(); progress_value(&x, quiet_ms(&x, now), now).to_json() }).ok_or_else(|| format!("no live job {id}")),
        "list" if rest == "held-leases" => Ok(ctx.store.read(|d| Value::Obj(d.lives.iter().filter(|x| x.active() && !x.held.is_empty() && !x.lease.is_empty())
            .map(|x| (x.lease.clone(), Value::Str(x.held.clone()))).collect()).to_json())),
        "list" => {
            let all = rest == "all";
            let (now, lives) = ctx.store.read(|d| (wall_ms(), d.lives.iter().filter(|x| all || x.active()).cloned().collect::<Vec<_>>()));
            let sessions: Vec<_> = lives.into_iter().map(|x| {
                let quiet = quiet_ms(&x, now);
                let progress = awaits_verdict(&x).map(|_| log_progress(&x.log, x.run_log_start));
                let key = (progress.as_ref().is_some_and(|p| p.still_running.is_empty()) && quiet >= Some(crate::state::QUIET_SAID_MS))
                    .then(|| (x.node.clone(), x.pid));
                (x, quiet, progress, key)
            }).collect();
            let keys: Vec<ThreadKey> = sessions.iter().filter_map(|(_, _, _, key)| key.clone()).collect();
            let threads = if keys.is_empty() { HashMap::new() } else { global_thread_cache().lookup_batch(&keys, LIST_THREADS_WAIT, false) };
            // Each session includes what devhub tells its lease owner.
            let list: Vec<String> = sessions.into_iter().map(|(x, quiet, progress, key)| {
                let mut v = match progress {
                    Some(progress) => {
                        let rows = key.as_ref().and_then(|k| threads.get(k)).map_or(&[][..], Vec::as_slice);
                        progress_value_with(&x, &progress, rows, quiet, now)
                    }
                    None => crate::state::live_value(&x),
                };
                if let (Value::Obj(fields), Some(word)) = (&mut v, (!x.lease.is_empty()).then(|| ctx.leases.owner_word(&x.lease)).flatten()) {
                    fields.push(("word".into(), Value::Str(word)));
                }
                // (how long its run has been silent: a hung one says it while its caller waits, and
                // a test call's edit with no progress says that, [`progress_value`])
                if let (Value::Obj(fields), Some(ms)) = (&mut v, quiet) {
                    fields.push(("quiet_ms".into(), Value::Int(ms as i64)));
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
/// Every frame a grab, gq or gseq answer names under lease `lease`'s tmp dir, in order: its path
/// there and the frame and patch it was drawn at, when stamped (a grab's one, each of a sequence's).
fn grab_files(lines: &[String], lease: &str) -> Vec<(String, Option<(u64, u64)>)> {
    let tmp = format!("/tmp/{lease}/");
    let under = |p: &str| { let p = p.replace('\\', "/"); p.ends_with(".png").then(|| p.split(&tmp).nth(1).map(str::to_string)).flatten() };
    let stamp = |f: &Value| Some((crate::lease::num(f.get("frame")?)? as u64, crate::lease::num(f.get("patch")?)? as u64));
    let mut out = Vec::new();
    for l in lines {
        let Ok(v) = makepad_strict_json::parse_depth(l.as_bytes(), 16) else {
            // (a line no JSON reader takes: its paths as words, unstamped)
            let l = l.replace("\\\\", "/");
            out.extend(l.split('"').filter(|w| w.ends_with(".png")).filter_map(|w| w.split(&tmp).nth(1)).map(|rel| (rel.to_string(), None)));
            continue;
        };
        let frames: Vec<&Value> = v.get("frames").and_then(Value::as_arr).map_or_else(|| vec![&v], |f| f.iter().collect());
        out.extend(frames.into_iter().filter_map(|f| Some((under(f.get("png")?.as_str()?)?, stamp(f)))));
    }
    out
}

fn initial_call(ctx: &Ctx, id: &str, check: &str) -> Result<String, String> {
    let signal = ctx.store.subscribe("live", &[id]);
    // what its session waits for (its box: its lease's queue line), said to the caller as it
    // changes: a lane's tests waited behind its own screen lease, said only in the queue (11-15)
    let mut told = String::new();
    let mut left = false;
    let mut waiting: Option<Waiting> = None;
    loop {
        let seen = signal.generation();
        // (one whose caller went waits on for its first verdict, its run in flight ending with it,
        // or for its end: [`LEFT_MS`]; nobody waits for that run any more)
        if !left && makepad_remote::lines::client_gone() {
            left = true;
            waiting = None;
            starter_left(ctx, id);
        }
        let r = running(id);
        if left {
            if let Some(r) = r.as_ref().filter(|r| can_yield_superseded(ctx, id, r)) {
                finish_call(ctx, id, r);
                return Err(format!("{id}: this call's client has gone"));
            }
        }
        if !left && waiting.is_none() { waiting = r.map(Waiting::on); }
        let x = get(ctx, id).ok_or("live job disappeared")?;
        let waits = if x.state == "starting" { x.note.as_str() } else { "" };
        if waits != told {
            if waits.is_empty() { makepad_remote::lines::wait_over(); } else { makepad_remote::lines::say_waiting(&format!("live {id} {waits}")); }
            told = waits.to_string();
        }
        if !x.active() || x.state == "paused" || x.patch.as_ref().is_some_and(terminal) {
            let answer = test_reply(&x, running(id).as_deref(), check);
            try_set(ctx, id, |s| s.touched_ms = wall_ms())?;
            if let Some(r) = running(id) { finish_call(ctx, id, &r); }
            return Ok(answer);
        }
        signal.wait(seen, Duration::from_secs(1));
    }
}

/// Compare with the published overlay without consuming saves or writing into the active ship directory.
fn saves_differ_from_sources(r: &Running, x: &Live) -> Result<bool, String> {
    let pin = crate::lease::Pin::of_rev(&x.rev).ok_or("live: missing pin")?;
    let (mut changed, mut deleted) = crate::lease::ship::uncommitted(&r.tree, &pin)?;
    back_to_pin(&r.tree, &x.sources, &mut changed, &mut deleted);
    changed.extend(also_files(&r.tree, &x.also)?);
    for path in changed {
        if crate::lease::ship::never_ships(&path) { continue; }
        match std::fs::symlink_metadata(r.tree.join(&path)) {
            Ok(_) if x.sources.get(&path) != Some(&source_of(&r.tree, &path)?.0) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => deleted.push(path),
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
    }
    Ok(deleted.iter().any(|p| !crate::lease::ship::never_ships(p) && x.sources.get(p).is_none_or(|h| h != "-")))
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

/// The admitted caller owns the edit until shipping and any warm restart have answered.
#[cfg(test)]
fn run_test_call_step(
    x: Live, fresh: bool, fault: Option<String>,
    publish: impl FnOnce(&Live) -> Result<(Live, LivePatch), String>,
    recover: impl FnOnce(&Live, &mut LivePatch),
    wait_tests: impl FnOnce() -> Result<Live, String>,
    ship_edit: impl FnOnce(&Live) -> Result<Result<Option<String>, String>, String>,
) -> Result<(Option<LivePatch>, Result<Option<String>, String>), String> {
    run_test_call_step_with(x, fresh, false, fault, publish, recover, wait_tests, ship_edit)
}

fn run_test_call_step_with(
    x: Live, fresh: bool, superseded: bool, fault: Option<String>,
    publish: impl FnOnce(&Live) -> Result<(Live, LivePatch), String>,
    recover: impl FnOnce(&Live, &mut LivePatch),
    wait_tests: impl FnOnce() -> Result<Live, String>,
    ship_edit: impl FnOnce(&Live) -> Result<Result<Option<String>, String>, String>,
) -> Result<(Option<LivePatch>, Result<Option<String>, String>), String> {
    let cut = fault.is_none() && !superseded && x.patch.as_ref().is_some_and(rerun_cut);
    let again = fresh && !superseded && !cut && x.patch.as_ref().is_some_and(|p| p.tests.is_some());
    let retried = fault.is_some() && x.patch.as_ref().is_some_and(|p| p.retried);
    let reason = fault.or_else(|| superseded.then(|| "its tests run again on the new saves after the earlier caller disconnected".into()))
        .or_else(|| cut.then(|| "its tests run again: the master's restart cut their verdict".into()))
        .or_else(|| again.then(|| "its tests run again: asked with --fresh".into()));
    if let Some(reason) = reason {
        let (published, mut p) = publish(&x)?;
        p.reason = reason;
        p.retried = retried;
        recover(&published, &mut p);
        wait_tests()?;
        Ok((None, Ok(None)))
    } else {
        Ok((x.patch.clone(), ship_edit(&x)?))
    }
}

/// Holds shipping until this call has copied the verdict for its exact source state.
/// `fresh`: its tests run again though nothing changed since their last round (`test --live
/// --fresh`), as after a fault; without it such a call answers that round's verdict, and its reply
/// says it is cached ([`cached_reply`]).
fn test_call(ctx: &Arc<Ctx>, id: &str, check: &str, fresh: bool) -> Result<Option<String>, String> {
    let called = wall_ms();
    try_set(ctx, id, |s| { s.touched_ms = called; s.left_ms = 0; })?;
    let signal = ctx.store.subscribe("live", &[id]);
    // the session's previous call, whose tests run: this one ships when their verdict comes
    // (the JIT applies an edit only after the running tests end)
    let mut behind = None;
    let mut _waiting: Option<Waiting> = None;
    loop {
        // a call still queued whose client has gone ships nothing: nobody asked for that edit any
        // more (a stopped client's call shipped a save made after it stopped, 11-05)
        if makepad_remote::lines::client_gone() {
            starter_left(ctx, id);
            return Err(format!("{id}: this call's client has gone"));
        }
        let seen = signal.generation();
        let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
        // (a session that ended has no verdict on what the caller has now: a new one does)
        if !accepting(id) || x.note.starts_with("parked for ") || x.state == "ended" { return Ok(None); }
        if !x.active() || x.state == "paused" { return Ok(Some(crate::state::live_value(&x).to_json())); }
        if let Some(r) = running(id) {
            // Register the owner while admission is locked, so another call cannot mistake the
            // interval between acquiring inflight and binding its wait token for an orphan.
            let admitted = {
                let calls = r.calls.lock().unwrap();
                let acquired = calls.park.is_none() && !r.inflight.swap(true, SeqCst);
                if acquired { _waiting.get_or_insert_with(|| Waiting::on(r.clone())).bind_active(); }
                acquired
            };
            if !admitted {
                if _waiting.is_none() {
                    if watch_superseded(&r) {
                        signal.wait(seen, Duration::from_secs(1));
                        continue;
                    }
                    if x.pid > 0 && r.waiting.load(SeqCst) == 0 && !r.restart.load(SeqCst) {
                        let changed = !fresh && saves_differ_from_sources(&r, &x)?;
                        if try_supersede_running(ctx, id, &r, fresh, changed) {
                            ctx.store.notify("live", id);
                            continue;
                        }
                    }
                    _waiting = Some(Waiting::on(r.clone()));
                }
                if behind.is_none() {
                    behind = Some(x.patch.as_ref().map_or_else(|| "the session's first run".to_string(), |p| {
                        let at = p.edit_ms / 1000 % 86_400;
                        format!("the tests of the edit saved at {:02}:{:02}:{:02}Z{}", at / 3600, at / 60 % 60, at % 60, p.group.map_or(String::new(), |g| format!(" (group {g})")))
                    }));
                }
            } else {
                let queued = behind.take().map(|b| (wall_ms().saturating_sub(called), b));
                let _inflight = Inflight(ctx, id, &r);
                let superseded = r.supersede.swap(false, SeqCst);
                let mut x = get(ctx, id).ok_or("live job disappeared")?;
                x.load_text()?;
                let fault = r.fault.lock().unwrap().clone();
                let held = !x.held.is_empty();
                let room = if held && fault.is_none() { None } else {
                    let admitted = admit(ctx, id, &r, makepad_remote::lines::asked_on());
                    if held && !accepting(id) { return Ok(None); }
                    x = get(ctx, id).ok_or("live job disappeared")?;
                    x.load_text()?;
                    admitted.inspect_err(|_| if makepad_remote::lines::client_gone() { starter_left(ctx, id) })?
                };
                let (before, shipped) = run_test_call_step_with(x, fresh, superseded, fault,
                    |x| {
                        let (files, edit_ms, current) = call_saves(ctx, &r, x)?;
                        let published = loop {
                            match publish_overlay(ctx, id, &r, x, &files, Some(current.clone())) {
                                Err(e) if hub_away(&e) && hub_back(ctx, id, x, &r) => continue,
                                result => break result?,
                            }
                        };
                        let x = get(ctx, id).filter(|s| s.active() && s.run_n == x.run_n && s.pid == x.pid).ok_or("the live run ended during publication")?;
                        let old = x.patch.as_ref().ok_or("live: no test state")?;
                        let files = published.map_or_else(|| old.files.clone(), |(changes, _)| changes);
                        let (queued_ms, behind) = queued.clone().unwrap_or_default();
                        let p = LivePatch { files, edit_ms: edit_ms.max(old.edit_ms.saturating_add(1)), called_ms: called, shipped_ms: wall_ms(), queued_ms, behind, ..Default::default() };
                        Ok((x, p))
                    },
                    |x, p| {
                        if set(ctx, id, |s| {
                            if s.state == "running" && s.pid == x.pid && s.run_n == x.run_n { s.patch = Some(p.clone()); }
                        }).is_some_and(|s| s.patch.as_ref().is_some_and(|old| old.edit_ms == p.edit_ms)) { recover(ctx, id, x, p, true); }
                    },
                    || {
                        wait_tests(ctx, id);
                        if watch_superseded(&r) { return Err(format!("{id}: this call's client has gone")); }
                        get(ctx, id).ok_or_else(|| "live job disappeared".into())
                    },
                    |x| {
                        let (files, edit_ms, current) = call_saves(ctx, &r, x)?;
                        let shipped = ship_edit(ctx, id, &r, files, edit_ms, called, Some(current), queued.clone());
                        if watch_superseded(&r) { return Err(format!("{id}: this call's client has gone")); }
                        Ok(shipped)
                    },
                )?;
                if held && !accepting(id) { return Ok(None); }
                let room = shipped.inspect_err(|_| if makepad_remote::lines::client_gone() { starter_left(ctx, id) })?.or(room);
                let x = try_set(ctx, id, |s| s.touched_ms = wall_ms())?.ok_or("live job disappeared")?;
                let gone = makepad_remote::lines::client_gone();
                if gone { starter_left(ctx, id); }
                let reply = cached_reply(test_reply(&x, Some(&r), check), before.as_ref(), x.patch.as_ref());
                // (its round's verdict reached a caller: the next call with nothing changed is cached)
                if !gone && x.patch.as_ref().is_some_and(|p| p.tests.is_some() && p.answered_ms == 0) {
                    let at = wall_ms();
                    let round = x.patch.as_ref().map(round_key);
                    let _ = try_set(ctx, id, |s| if let Some(p) = s.patch.as_mut().filter(|p| Some(round_key(p)) == round) { p.answered_ms = at; });
                }
                return Ok(Some(with_room(reply, room.as_deref())));
            }
        }
        signal.wait(seen, Duration::from_secs(1));
    }
}

/// A test call's reply whose verdict is the session's last round's (its patch before the call,
/// `before`, is the one after it, `after`: no round ran) with `cached_ms`, that verdict's time: the
/// client says the verdict is cached and that `--fresh` runs the tests again. Four of a lane's
/// five reruns in a flake hunt read their session's last pass as a fresh one (11-26). Only once
/// an earlier call carried that verdict to its caller (`answered_ms`): a call whose link was cut
/// mid-round asked again and read its own edit's round as cached (W24157, 11-26).
fn cached_reply(reply: String, before: Option<&LivePatch>, after: Option<&LivePatch>) -> String {
    let cached = match (before, after) {
        (Some(b), Some(a)) if a.tests.is_some() && b.answered_ms > 0 && round_key(a) == round_key(b) => a.verdict_ms.max(a.shipped_ms).max(a.edit_ms),
        _ => return reply,
    };
    match reply.strip_suffix('}') {
        Some(head) => format!("{head},\"cached_ms\":{cached}}}"),
        None => reply,
    }
}

/// What makes a session's round that round: its edit, ship, verdict and outcome.
fn round_key(p: &LivePatch) -> (u64, u64, u64, u64, String) {
    (p.seq, p.edit_ms, p.shipped_ms, p.verdict_ms, p.outcome.clone())
}

pub fn verdict(p: &LivePatch) -> String {
    let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
    if p.outcome == "jit_error" { return crate::state::jit_error_verdict(p); }
    let Some(t) = &p.tests else {
        if p.outcome == "ended" {
            let reason = if p.reason.is_empty() { "ended before its verdict" } else { &p.reason };
            return format!("{}verdict {reason}\n", p.jit_notice);
        }
        if p.reason.starts_with("verdict lost:") { return format!("{}{}\n", p.jit_notice, p.reason); }
        return format!("{}verdict {}{}\n", p.jit_notice, if p.outcome.is_empty() { "pending" } else { &p.outcome }, if p.reason.is_empty() { String::new() } else { format!(": {}", p.reason) });
    };
    // a save made long before its call (a test session ships on the call) says so: that wait was
    // the caller's, not the ship's (162 s "save -> shipped" was an agent's 161 s before its sync, 11-02)
    let called = if p.called_ms > p.edit_ms + 1000 { format!("save -> call {}, ", after(p.called_ms)) } else { String::new() };
    // a call that waited for the previous call's tests says so: that wait was neither the ship's nor this edit's
    let called = if p.queued_ms > 0 { format!("{called}queued {} ms behind {}, ", p.queued_ms, p.behind) } else { called };
    let failures = t.fresh_failures();
    let mut out = format!("{}verdict {}/{} passed, {} failed ({called}save -> shipped {}, -> patched {}, -> verdict {})\n", p.jit_notice, t.fresh_passed(), t.ran, failures.len(), after(p.shipped_ms), after(p.outcome_ms), after(p.verdict_ms));
    if !p.reason.is_empty() { out.push_str(&format!("  {}\n", p.reason)); }
    for name in failures { out.push_str(&format!("  {name}\n")); }
    if !t.stale.is_empty() { out.push_str(&format!("  {} earlier failure(s), not rerun for this edit (its changes reach none of them): {}\n", t.stale.len(), t.stale.join(", "))); }
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

/// `HH:MM:SSZ` of a wall time.
fn hhmmss(ms: u64) -> String {
    let s = ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
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
    let (mut rows, waiting) = ctx.store.read(|d| (restart_rows(&d.lives, now), d.lives.iter().filter(|x| awaits_verdict(x).is_some()).cloned().collect::<Vec<_>>()));
    let stalled: Vec<_> = waiting.into_iter().filter_map(|x| {
        let quiet = quiet_ms(&x, now).filter(|q| *q >= crate::lease::sched::QUIET_STALL_MS)?;
        let progress = log_progress(&x.log, x.run_log_start);
        Some((x, quiet, progress))
    }).collect();
    let keys: Vec<ThreadKey> = stalled.iter().filter(|(_, _, progress)| {
        (progress.current.is_some() || progress.measured_ms.is_some()) && progress.still_running.is_empty()
    }).map(|(x, _, _)| (x.node.clone(), x.pid)).collect();
    let threads = if keys.is_empty() { HashMap::new() } else { global_thread_cache().lookup_batch(&keys, GET_THREADS_WAIT, true) };
    rows.extend(stalled.into_iter().filter_map(|(x, quiet, progress)| {
        let why = if progress.current.is_none() && progress.measured_ms.is_none() {
            no_progress(&x, Some(quiet), now)
        } else {
            let rows = threads.get(&(x.node.clone(), x.pid)).map_or(&[][..], Vec::as_slice);
            running_test_summary(&x, &progress, rows, Some(quiet), now)
        }?;
        Some(format!("{}: {} {}@{} {why}: `makepad devhub live {}`", x.lane, x.id, x.app, x.node, x.id))
    }));
    rows
}

/// A running test session's edit whose verdict its call waits for: none yet, and its run started.
fn awaits_verdict(x: &Live) -> Option<&LivePatch> {
    x.patch.as_ref().filter(|p| x.test && x.state == "running" && x.pid > 0 && p.edit_ms > 0 && !terminal(p))
}

/// A test session's edit with no verdict ([`awaits_verdict`]) whose run has written nothing for
/// [`crate::lease::sched::QUIET_STALL_MS`] or more (`quiet`: its log's last change) makes no
/// progress: said on its caller's still-running line and in the manager's check-in
/// ([`restarts_pending`]), with that evidence, and never a reason to end it. Two test calls waited
/// 102 min at "restarting JIT" before their lane noticed by chance (W25685, W25686, 11-27).
fn no_progress(x: &Live, quiet: Option<u64>, now: u64) -> Option<String> {
    let p = awaits_verdict(x)?;
    let quiet = quiet.filter(|q| *q >= crate::lease::sched::QUIET_STALL_MS)?;
    let through = match p.outcome.as_str() {
        "restart" | "recovering" => " through its JIT's restart",
        "initial" => " in its first run",
        _ => "",
    };
    Some(format!("no progress for {} min{through}: the edit saved at {} has no verdict, and its JIT (pid {}) has written nothing since {}", quiet / 60_000, hhmmss(p.edit_ms), x.pid, hhmmss(now.saturating_sub(quiet))))
}

#[derive(Debug, Default, Clone, PartialEq)]
struct RoundLog {
    total: usize,
    passed: Vec<String>,
    failed: Vec<String>,
    ignored: Vec<String>,
    over_60s: Vec<String>,
    finished: bool,
    verdict_ms: Option<u64>,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct RunProgress {
    current: Option<RoundLog>,
    measured_ms: Option<u64>,
    still_running: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct RunningThread {
    name: String,
    state: String,
    cpu_ms: u64,
    waits: String,
}

impl RoundLog {
    fn done(&self) -> usize { self.passed.len() + self.failed.len() + self.ignored.len() }

    fn names(&self) -> impl Iterator<Item = &String> { self.passed.iter().chain(&self.failed).chain(&self.ignored) }
}

/// Tokens of a harness verdict. A quoted why or diagnostic can contain spaces and even "ms=".
fn verdict_fields(line: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut quoted = false;
    let mut escaped = false;
    line.split(move |c: char| {
        if escaped { escaped = false; }
        else if quoted && c == '\\' { escaped = true; }
        else if c == '"' { quoted = !quoted; }
        !quoted && c.is_ascii_whitespace()
    }).filter_map(|field| field.split_once('='))
}

fn test_result_count(rest: &str) -> Option<usize> {
    rest.split([';', '.']).filter_map(|part| {
        let (n, kind) = part.trim().split_once(' ')?;
        matches!(kind, "passed" | "failed" | "ignored").then(|| n.parse::<usize>().ok()).flatten()
    }).reduce(usize::saturating_add)
}

fn parse_rounds(log: &str) -> Vec<RoundLog> {
    let mut rounds = Vec::new();
    let mut cur = RoundLog::default();
    let mut done = std::collections::HashSet::new();
    for line in log.lines() {
        let header = line.strip_prefix("running ").and_then(|s| s.split_once(' '))
            .filter(|(_, tail)| matches!(*tail, "test" | "tests")).and_then(|(n, _)| n.parse::<usize>().ok());
        let boot = line.starts_with("relax live: restarting (") || line.starts_with("relax live: restarting:") || line.starts_with("devhub: the JIT is ");
        if boot || (cur.finished && (line.starts_with("     Running ") || header.is_some())) {
            if cur.total > 0 || cur.done() > 0 || cur.finished {
                rounds.push(std::mem::take(&mut cur));
                done.clear();
            }
        }
        if let Some(n) = header {
            if cur.total == 0 { cur.total = n; }
        } else if let Some(rest) = line.strip_prefix("test result: ") {
            if !test_result_count(rest).is_some_and(|n| n < cur.total) { cur.finished = true; }
        } else if let Some(rest) = line.strip_prefix("testverdict ") {
            cur.finished = true;
            let fields: BTreeMap<_, _> = verdict_fields(rest).collect();
            if !fields.contains_key("reverted") && !fields.contains_key("faulted") {
                cur.verdict_ms = fields.get("ms").and_then(|s| s.parse::<f64>().ok()).map(|ms| ms as u64);
            }
        } else if let Some(test) = line.strip_prefix("test ").filter(|_| !cur.finished) {
            if let Some(name) = test.strip_suffix(" has been running for over 60 seconds") {
                let name = name.strip_suffix(" - should panic").unwrap_or(name);
                if !name.is_empty() && !name.contains(char::is_whitespace) && !done.contains(name) && !cur.over_60s.iter().any(|n| n == name) {
                    cur.over_60s.push(name.into());
                }
                continue;
            }
            let Some((name, outcome)) = test.split_once(" ... ") else { continue };
            let name = name.strip_suffix(" - should panic").unwrap_or(name);
            let outcome = match outcome.rsplit_once(" <") {
                Some((result, time)) if time.strip_suffix("s>").is_some_and(|s| s.parse::<f64>().is_ok()) => result,
                _ => outcome,
            };
            let list = match outcome { "ok" => &mut cur.passed, "FAILED" => &mut cur.failed, "ignored" => &mut cur.ignored, _ => continue };
            if !name.is_empty() && !name.contains(char::is_whitespace) && done.insert(name) {
                list.push(name.into());
                cur.over_60s.retain(|n| n != name);
            }
        }
    }
    rounds.push(cur);
    rounds
}

fn log_progress(log_path: &str, run_log_start: u64) -> RunProgress {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 8 << 20;
    let Ok(mut log) = std::fs::File::open(log_path) else { return RunProgress::default() };
    let Ok(len) = log.seek(SeekFrom::End(0)) else { return RunProgress::default() };
    let start = len.saturating_sub(TAIL);
    let mut bytes = Vec::new();
    if log.seek(SeekFrom::Start(start)).is_err() || log.take(len - start).read_to_end(&mut bytes).is_err() { return RunProgress::default(); }
    let split = run_log_start.saturating_sub(start).min(bytes.len() as u64) as usize;
    let mut rounds = parse_rounds(&String::from_utf8_lossy(&bytes[..split]));
    rounds.extend(parse_rounds(&String::from_utf8_lossy(&bytes[split..])));
    let measured_ms = rounds.iter().rev().find_map(|r| r.verdict_ms);
    let current = rounds.pop().filter(|r| !r.finished && (r.total > 0 || r.done() > 0));
    let mut still_running = Vec::new();
    if let Some(cur) = &current {
        still_running.clone_from(&cur.over_60s);
        if still_running.is_empty() && cur.total > 0 {
            if let Some(prev) = rounds.iter().rev().find(|r| r.finished && r.total == cur.total) {
                let done: std::collections::HashSet<_> = cur.names().collect();
                let missing: Vec<_> = prev.names().filter(|n| !done.contains(n)).cloned().collect();
                if missing.len() <= cur.total.saturating_sub(done.len()).max(1) { still_running = missing; }
            }
        }
    }
    RunProgress { current, measured_ms, still_running }
}

fn parse_program_threads(lines: &[String]) -> Vec<RunningThread> {
    lines.iter().filter_map(|line| {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let name = v.get("name")?.as_str()?;
        if v.get("role")?.as_str()? != "program" || matches!(name, "" | "main" | "<unnamed>") ||
            ["makepad-", "relax-", "devhub-"].iter().any(|s| name.starts_with(s)) { return None; }
        Some(RunningThread { name: name.into(), state: v.get("state").and_then(Value::as_str).unwrap_or_default().into(),
            cpu_ms: v.get("cpu_ms").and_then(Value::as_u64).unwrap_or_default(), waits: v.get("waits").and_then(Value::as_str).unwrap_or_default().into() })
    }).collect()
}

const THREADS_LOOK: Duration = Duration::from_secs(2);
const THREADS_CACHE_TTL: Duration = Duration::from_secs(10);
const THREADS_CACHE_EVICT: Duration = Duration::from_secs(60);
const THREAD_PROBE_WORKERS: usize = 4;
const LIST_THREADS_WAIT: Duration = Duration::from_millis(250);
const GET_THREADS_WAIT: Duration = Duration::from_millis(500);

type ThreadKey = (String, u32);
type ThreadProbeFn = Arc<dyn Fn(&str, u32) -> Vec<RunningThread> + Send + Sync>;

#[derive(Default)]
struct ThreadEntry {
    rows: Vec<RunningThread>,
    /// Stamped when the probe finishes (`None` until the first probe completes).
    updated_at: Option<Instant>,
    inflight: bool,
    /// When the current in-flight probe was enqueued (caps follow-up callers' wait).
    started_at: Option<Instant>,
    logged_wait: bool,
}

#[derive(Default)]
struct ThreadPoolState {
    entries: HashMap<ThreadKey, ThreadEntry>,
    queue: std::collections::VecDeque<ThreadKey>,
    closed: bool,
}

struct ThreadCacheShared {
    state: Mutex<ThreadPoolState>,
    work_cv: std::sync::Condvar,
    done_cv: std::sync::Condvar,
    ttl: Duration,
    evict: Duration,
}

struct ThreadCache {
    shared: Arc<ThreadCacheShared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl ThreadCache {
    fn new(worker_count: usize, ttl: Duration, evict: Duration, probe: ThreadProbeFn) -> Self {
        let shared = Arc::new(ThreadCacheShared {
            state: Mutex::new(ThreadPoolState::default()),
            work_cv: std::sync::Condvar::new(),
            done_cv: std::sync::Condvar::new(),
            ttl,
            evict,
        });
        let mut workers = Vec::with_capacity(worker_count);
        for i in 0..worker_count {
            let s = shared.clone();
            let p = probe.clone();
            match std::thread::Builder::new()
                .name(format!("devhub-live-threads-{i}"))
                .spawn(move || thread_probe_worker(s, p))
            {
                Ok(h) => workers.push(h),
                Err(e) => eprintln!("live threads: failed to spawn worker {i}: {e}"),
            }
        }
        Self { shared, workers }
    }

    fn lookup_one(&self, node: &str, pid: u32, wait: Duration, wait_on_stale: bool) -> Vec<RunningThread> {
        let key = (node.to_string(), pid);
        self.lookup_batch(std::slice::from_ref(&key), wait, wait_on_stale)
            .remove(&key)
            .unwrap_or_default()
    }

    fn lookup_batch(&self, keys: &[ThreadKey], wait: Duration, wait_on_stale: bool) -> HashMap<ThreadKey, Vec<RunningThread>> {
        if keys.is_empty() { return HashMap::new(); }
        let call_start = Instant::now();
        let call_deadline = call_start.checked_add(wait).unwrap_or(call_start);
        let mut slow_to_log: Vec<ThreadKey> = Vec::new();
        let out = {
            let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            st.entries.retain(|_, e| e.inflight || e.updated_at.is_some_and(|at| call_start.saturating_duration_since(at) < self.shared.evict));
            let mut enqueued = 0usize;
            for key in keys {
                let entry = st.entries.entry(key.clone()).or_default();
                let fresh = entry.updated_at.is_some_and(|at| call_start.saturating_duration_since(at) < self.shared.ttl);
                if !fresh && !entry.inflight && !self.workers.is_empty() {
                    entry.inflight = true;
                    entry.started_at = Some(call_start);
                    entry.logged_wait = false;
                    st.queue.push_back(key.clone());
                    enqueued += 1;
                }
            }
            if enqueued == 1 { self.shared.work_cv.notify_one(); }
            else if enqueued > 1 { self.shared.work_cv.notify_all(); }
            loop {
                let now = Instant::now();
                if now >= call_deadline { break; }
                let wake_at = keys.iter()
                    .filter_map(|k| st.entries.get(k))
                    .filter_map(|e| thread_wait_until(e, now, call_start, wait, wait_on_stale))
                    .max()
                    .map(|cap| cap.min(call_deadline));
                let Some(until) = wake_at else { break };
                let Some(rem) = until.checked_duration_since(now).filter(|d| !d.is_zero()) else { break };
                st = self.shared.done_cv.wait_timeout(st, rem).unwrap_or_else(|e| e.into_inner()).0;
            }
            let mut out = HashMap::with_capacity(keys.len());
            for key in keys {
                if let Some(entry) = st.entries.get_mut(key) {
                    let still_waiting = !wait.is_zero()
                        && entry.inflight
                        && (entry.updated_at.is_none() || (wait_on_stale && entry.updated_at.is_some_and(|at| at < call_start)));
                    if still_waiting && !entry.logged_wait {
                        entry.logged_wait = true;
                        slow_to_log.push(key.clone());
                    }
                    out.insert(key.clone(), entry.rows.clone());
                }
            }
            out
        };
        for (node, pid) in slow_to_log {
            eprintln!("live threads {node}:{pid}: probe still running past {} ms budget; returning without fresh thread rows", wait.as_millis());
        }
        out
    }
}

impl Drop for ThreadCache {
    fn drop(&mut self) {
        {
            let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            st.closed = true;
            st.queue.clear();
        }
        self.shared.work_cv.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn thread_wait_until(entry: &ThreadEntry, now: Instant, call_start: Instant, wait: Duration, wait_on_stale: bool) -> Option<Instant> {
    if !entry.inflight || wait.is_zero() { return None; }
    if let Some(at) = entry.updated_at {
        if !wait_on_stale || at >= call_start { return None; }
    }
    let cap = entry.started_at?.checked_add(wait)?;
    (cap > now).then_some(cap)
}

fn thread_probe_worker(shared: Arc<ThreadCacheShared>, probe: ThreadProbeFn) {
    loop {
        let key = {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(key) = st.queue.pop_front() { break key; }
                if st.closed { return; }
                st = shared.work_cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        let rows = probe(&key.0, key.1);
        let finished_at = Instant::now();
        {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = st.entries.get_mut(&key) {
                entry.rows = rows;
                entry.updated_at = Some(finished_at);
                entry.inflight = false;
                entry.started_at = None;
                entry.logged_wait = false;
            }
        }
        shared.done_cv.notify_all();
    }
}

fn probe_program_threads(node: &str, pid: u32) -> Vec<RunningThread> {
    let started = Instant::now();
    let lines = inspect_until(node, pid, "threads", &|| started.elapsed() > THREADS_LOOK)
        .ok()
        .flatten()
        .unwrap_or_default();
    parse_program_threads(&lines)
}

fn global_thread_cache() -> &'static ThreadCache {
    static CACHE: std::sync::LazyLock<ThreadCache> = std::sync::LazyLock::new(|| {
        ThreadCache::new(
            THREAD_PROBE_WORKERS,
            THREADS_CACHE_TTL,
            THREADS_CACHE_EVICT,
            Arc::new(probe_program_threads),
        )
    });
    &CACHE
}

fn cached_program_threads(node: &str, pid: u32, wait: Duration, wait_on_stale: bool) -> Vec<RunningThread> {
    global_thread_cache().lookup_one(node, pid, wait, wait_on_stale)
}

fn ended_verdict_reason(note: &str, progress: &RunProgress) -> String {
    let Some(cur) = &progress.current else { return format!("ended before its verdict ({note}; 0 tests finished)") };
    let ignored = if cur.ignored.is_empty() { String::new() } else { format!(", {} ignored", cur.ignored.len()) };
    let still = if progress.still_running.is_empty() { String::new() } else { format!("; still running: {}", progress.still_running.join(", ")) };
    format!("ended before its verdict ({note}; {}/{} finished: {} passed, {} failed{ignored}{still})", cur.done(), cur.total, cur.passed.len(), cur.failed.len())
}

fn settle_open_patch(s: &mut Live, now: u64, progress: Option<&RunProgress>) {
    if !s.test { return; }
    let Some(p) = s.patch.as_mut().filter(|p| !terminal(p)) else { return };
    p.tests = None;
    p.outcome = "ended".into();
    p.verdict_ms = now;
    if p.outcome_ms == 0 { p.outcome_ms = now; }
    p.reason = ended_verdict_reason(&s.note, progress.unwrap_or(&RunProgress::default()));
    p.reason_ref = Default::default();
}

fn end_progress(ctx: &Ctx, id: &str) -> Option<RunProgress> {
    get(ctx, id).as_ref().and_then(open_progress)
}

fn open_progress(x: &Live) -> Option<RunProgress> {
    (x.test && x.patch.as_ref().is_some_and(|p| !terminal(p))).then(|| log_progress(&x.log, x.run_log_start))
}

fn running_test_summary(x: &Live, progress: &RunProgress, threads: &[RunningThread], quiet: Option<u64>, now: u64) -> Option<String> {
    use crate::lease::sched::dur;
    let p = x.patch.as_ref().filter(|p| x.test && x.active() && !terminal(p))?;
    let elapsed = dur(now.saturating_sub(p.edit_ms.max(p.shipped_ms).max(x.started_ms)));
    let measured = progress.measured_ms.map_or_else(|| ", no prior run measured".into(), |ms| format!(" vs {} measured", dur(ms)));
    let mut time = format!("{elapsed} elapsed{measured}");
    if let Some(ms) = quiet { time.push_str(&format!("; quiet {}", dur(ms))); }
    let mut summary = progress.current.as_ref().map_or_else(|| "waiting for tests to start".into(), |r| format!("{}/{} finished", r.done(), r.total));
    let (noun, names, count) = if progress.still_running.is_empty() {
        let names: Vec<_> = threads.iter().take(3).map(|t| format!("{} [{}{}{}]", t.name, t.state, if t.waits.is_empty() { "" } else { ", " }, t.waits)).collect();
        ("thread ", names, threads.len())
    } else {
        ("", progress.still_running.iter().take(3).cloned().collect(), progress.still_running.len())
    };
    if count > 0 {
        summary.push_str(&format!("; still running {noun}{}", names.join(", ")));
        if count > names.len() { summary.push_str(&format!(" + {} more", count - names.len())); }
    } else if progress.current.is_none() {
        time.push_str(&format!("; edit saved at {} on pid {}", hhmmss(p.edit_ms), x.pid));
    }
    Some(format!("{summary} ({time})"))
}

fn running_tests_json(x: &Live, progress: &RunProgress, threads: &[RunningThread], quiet: Option<u64>, now: u64) -> Value {
    use makepad_strict_json::{obj, s};
    let round = progress.current.as_ref();
    let names = progress.still_running.iter().map(s).collect();
    let thread_rows = threads.iter().map(|t| obj(vec![("name", s(&t.name)), ("state", s(&t.state)), ("waits", s(&t.waits)), ("cpu_ms", Value::Int(t.cpu_ms as i64))])).collect();
    obj(vec![("summary", s(running_test_summary(x, progress, threads, quiet, now).unwrap_or_default())),
        ("total", Value::Int(round.map_or(0, |r| r.total) as i64)), ("finished", Value::Int(round.map_or(0, RoundLog::done) as i64)),
        ("measured_ms", progress.measured_ms.map_or(Value::Null, |n| Value::Int(n as i64))),
        ("still_running", Value::Arr(names)), ("threads", Value::Arr(thread_rows))])
}

fn progress_value(x: &Live, quiet: Option<u64>, now: u64) -> Value {
    if awaits_verdict(x).is_none() { return crate::state::live_value(x); }
    let progress = log_progress(&x.log, x.run_log_start);
    let threads = if progress.still_running.is_empty() && quiet >= Some(crate::state::QUIET_SAID_MS) {
        cached_program_threads(&x.node, x.pid, GET_THREADS_WAIT, true)
    } else { Vec::new() };
    progress_value_with(x, &progress, &threads, quiet, now)
}

fn progress_value_with(x: &Live, progress: &RunProgress, threads: &[RunningThread], quiet: Option<u64>, now: u64) -> Value {
    let Some(p) = awaits_verdict(x) else { return crate::state::live_value(x) };
    let info = running_tests_json(x, progress, threads, quiet, now);
    let summary = info.get("summary").and_then(Value::as_str).unwrap_or_default();
    let mut value = crate::state::live_value(x);
    if let Value::Obj(fields) = &mut value {
        if !threads.is_empty() && p.reason.is_empty() && p.reason_ref.len == 0 {
            if let Some((_, Value::Obj(patch))) = fields.iter_mut().find(|(k, _)| k == "patch") {
                if let Some((_, reason)) = patch.iter_mut().find(|(k, _)| k == "reason") { *reason = Value::Str(summary.into()); }
            }
        }
        let elapsed = now.saturating_sub(p.edit_ms.max(p.shipped_ms).max(x.started_ms));
        if quiet >= Some(crate::state::QUIET_SAID_MS) || progress.measured_ms.is_some_and(|ms| elapsed > ms.saturating_add(crate::state::STARTUP_MARGIN_MS)) {
            fields.push(("no_progress".into(), Value::Str(summary.into())));
        }
        fields.push(("running_tests".into(), info));
    }
    value
}

fn restart_rows(lives: &[Live], now: u64) -> Vec<String> {
    lives.iter().filter(|x| x.active() && x.restart_since_ms > 0 && now.saturating_sub(x.restart_since_ms) > RESTART_LISTED_MS).map(|x| format!(
        "{}: {} {}@{} restart pending {} min, {} save(s) since {} not running ({}): `makepad devhub live act {} session relaunch`",
        x.lane, x.id, x.app, x.node, now.saturating_sub(x.restart_since_ms) / 60_000, x.restart_saves, hhmm(x.restart_since_ms),
        if x.restart_why.is_empty() { "no reason given" } else { &x.restart_why }, x.id,
    )).collect()
}

/// `live start --home box`: the session's app runs on its box's own home, the person's app data there.
pub const BOX_HOME: &str = "box";

/// What a session's app home starts from ([`crate::lease::ship::send_home`]): its `--home` archive;
/// for an app session without one, an empty archive, so its app finds a fresh home of its own
/// (`MAKEPAD_TEST_HOME`), gone with its lease. None on the box's own home (`--home box`), for a test
/// session (its harness gives each app its own), or when its `--env` names a test home. A lane's live
/// Town opened a box's own Town home, a persistent demo copy linked to the person's Mac, and signed
/// a name into it (11-27). A fresh home is a test home: Town takes the test driver's input to its
/// call and sharing controls there, never in the box's own.
fn home_archive(ctx: &Ctx, x: &Live) -> Result<Option<PathBuf>, String> {
    match x.home.as_str() {
        BOX_HOME => Ok(None),
        "" if x.test || x.env.iter().any(|(k, _)| k == "MAKEPAD_TEST_HOME") => Ok(None),
        "" => empty_home(&ctx.home).map(Some),
        a => Ok(Some(PathBuf::from(a))),
    }
}

/// An empty tar archive (one record of zero blocks) in devhub's home: a fresh app home's seed,
/// shipped to a box once by its hash.
fn empty_home(home: &Path) -> Result<PathBuf, String> {
    const RECORD: usize = 10240;
    let p = home.join("empty-home.tar");
    if std::fs::metadata(&p).is_ok_and(|m| m.len() == RECORD as u64) { return Ok(p); }
    let tmp = home.join(format!("empty-home.tar.{}", std::process::id()));
    std::fs::write(&tmp, vec![0u8; RECORD]).and_then(|()| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

/// Where a placed session runs on its box: its work dir, the home its app finds (`MAKEPAD_TEST_HOME`
/// from `--home` or fresh, or the box's `HOME`), its pid, and how to copy its files back (`live get`) with no
/// lease of its own: a helper's `run --node` job on the same box could not find them, and each probe
/// cost it about 10 min of queue (11-26). None before it is placed.
fn on_box_line(x: &Live) -> Option<String> {
    if x.work_dir.is_empty() { return None; }
    let home = if x.home == BOX_HOME {
        format!("{} (the box's own home, from --home box: its real app data)", if x.box_home.is_empty() { "-" } else { &x.box_home })
    } else if x.home_dir.is_empty() {
        if x.box_home.is_empty() { "-".into() } else { x.box_home.clone() }
    } else if x.home.is_empty() {
        format!("{} (MAKEPAD_TEST_HOME, fresh: gone at its release)", x.home_dir)
    } else {
        format!("{} (MAKEPAD_TEST_HOME, from --home)", x.home_dir)
    };
    Some(format!("  on {}: work {}, home {home}, pid {}; its files: `devhub live get {} <rel> <local-dir>` (rel in its lease: its work, `target/..`, `tmp/..`)\n", x.node, x.work_dir, x.pid, x.id))
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
    if x.active() && !x.held.is_empty() { out.push_str(&format!("  held: {}\n", x.held)); }
    if let Some(ms) = quiet_ms(x, wall_ms()).filter(|ms| *ms >= crate::state::QUIET_SAID_MS) { out.push_str(&format!("  last output {} ago\n", crate::state::ago(ms))); }
    if let Some(line) = kept_line(x, wall_ms()) { out.push_str(&line); }
    if !x.env.is_empty() { out.push_str(&format!("  env: {}\n", x.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(","))); }
    if let Some(line) = on_box_line(x) { out.push_str(&line); }
    if x.home == BOX_HOME { out.push_str("  home: the box's own home (--home box): its real app data\n"); } else if !x.home.is_empty() { out.push_str(&format!("  home: {}{}\n", x.home, if x.home_dir.is_empty() { String::new() } else { format!(" (the app's copy on its box: {})", x.home_dir) })); }
    if let Some(p) = &x.patch {
        let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
        if x.test && !terminal(p) {
            let progress = log_progress(&x.log, x.run_log_start);
            if !x.active() {
                out.push_str(&format!("{}verdict {}\n", p.jit_notice, ended_verdict_reason(&x.note, &progress)));
            } else {
                let now = wall_ms();
                let summary = running_test_summary(x, &progress, &[], quiet_ms(x, now), now).unwrap_or_default();
                let phase = if p.outcome.is_empty() { "pending" } else { &p.outcome };
                let hint = progress.still_running.is_empty() && p.reason.contains("; still running thread ");
                out.push_str(&format!("{}verdict {phase}: {}\n", p.jit_notice, if hint { &p.reason } else { &summary }));
                if !hint && !p.reason.is_empty() { out.push_str(&format!("  {}\n", p.reason)); }
            }
        } else if x.test {
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

    /// A visible session is the person's app: in use while they work at its box, and a headless one
    /// by its edits and calls alone (the idle end closed a person's visible Fab, 11-09).
    #[test]
    fn a_visible_session_is_in_use_while_its_person_works_at_its_box() {
        let t0 = 1_793_713_740_000u64;
        let min = 60_000;
        let live = |id: &str, visible: bool, touched_ms: u64| Live { id: id.into(), lane: "a-lane".into(), node: "box-a".into(), visible, touched_ms, state: "running".into(), ..Default::default() };
        let mine = [live("W1", true, t0), live("W2", false, t0 + min)];
        let now = t0 + 30 * min;
        assert_eq!(super::unused_ms(&mine[0], Some(now - 2 * min), None, now), 2 * min, "idle: a person's input at its box uses a visible session");
        assert_eq!(super::unused_ms(&mine[1], Some(now - 2 * min), None, now), 29 * min, "idle: a headless session on that box goes by its edits and calls");
        assert_eq!(super::unused_ms(&mine[0], None, None, now), 30 * min, "idle: no input seen at its box");
        assert!(super::unused_ms(&mine[0], Some(now - 12 * min), None, now) >= super::IDLE_MS, "idle: it ends 10 min after the person's last input");
        // a person's input in the app itself (through Desk Link: the box's idle time does not see
        // it) uses any session, headless too
        assert_eq!(super::unused_ms(&mine[1], None, Some(now - 3 * min), now), 3 * min, "idle: a person's input in its app");
        // Its lane landed and went quiet: a visible session stays until its owner ends it, and a
        // headless one while a person uses its app; a quiet headless one goes (W22615, a visible
        // session a person watched through Desk Link, was ended so, 11-26)
        let kept = super::kept_from_landed(&mine[0], None, now).expect("visible");
        assert!(kept.starts_with("it is visible, a person's window") && kept.contains("devhub live release W1"), "{kept}");
        let used = super::kept_from_landed(&mine[1], Some(now - 5 * min), now).expect("a person's input 5 min ago");
        assert!(used.starts_with("a person gave its app input 5m00s ago"), "{used}");
        assert_eq!(super::kept_from_landed(&mine[1], Some(now - 31 * min), now), None, "their last input past the keep");
        assert_eq!(super::kept_from_landed(&mine[1], None, now), None, "no person's input: a quiet hidden session goes");
        // Past its idle end a visible session stays too, used or not, until its owner ends it: a
        // person's demo was ended twice so in a morning (11-26); a hidden one ends
        let idle = super::idle_kept(&mine[0]).expect("visible");
        assert!(idle.starts_with("it is visible, a person's window: it stays until its owner ends it") && idle.contains("devhub live release W1"), "{idle}");
        assert_eq!(super::idle_kept(&mine[1]), None, "a hidden session ends by its idle time");
        assert!(super::said_once("W1", "idle") && !super::said_once("W1", "idle") && super::said_once("W1", "landed"), "said once per session and kind");
        // its app's builds keep it past its idle end: a WM's cargo builds of the apps it hosts
        let wm = live("W3", false, t0);
        let built = |ago: u64| Some((now - ago, "makepad_widgets".to_string()));
        assert_eq!(super::kept_by_build(&wm, built(min), now).as_deref(), Some("makepad_widgets"), "kept: its compilers worked a minute ago");
        assert_eq!(super::kept_by_build(&wm, built(11 * min), now), None, "ends: its builds ended past its idle time ago");
        assert_eq!(super::kept_by_build(&wm, None, now), None, "ends: no build, whatever else its app does");
        assert_eq!(super::kept_by_build(&live("W4", false, now - 2 * min), built(min), now), None, "not idle yet: nothing to keep");
        let kept = Live { kept_by: "makepad_widgets".into(), kept_ms: now, ..wm.clone() };
        assert_eq!(super::kept_line(&kept, now).as_deref(), Some("  kept by a build (makepad_widgets): no edit or call for 30 min, past its idle end of 10 min\n"), "live list says it");
        assert_eq!(super::kept_line(&Live { touched_ms: now + 1, ..kept.clone() }, now + 1), None, "a call since: no longer kept by a build");
        assert_eq!(super::kept_line(&wm, now), None);
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
    /// live runs were stopped at the first one and failed as JIT errors (11-12). A program's own
    /// panic is judged by its frame, where it panicked and what kind it is, never by its message: a
    /// lane's test whose assertion quoted an internal compiler error was said to be a JIT error,
    /// "not your change" (11-23).
    #[test]
    fn the_jits_own_crates_tests_print_the_jits_lines_as_their_own() {
        use super::Scan;
        let panic = "thread '<unnamed>' (2729363) panicked at libs/relax/compiler/src/jit.rs:2200:49:";
        let restart = "relax: JIT thread `lifeline-test-restart` panicked: lifeline test panic (at libs/relax/compiler/src/lifeline.rs:271) -- restarted (1 of 16)";
        for line in [panic, restart] {
            assert!(super::fault_line("makepad-fab", line, &mut Scan::default()), "an app's run: the JIT's fault: {line}");
            assert!(!super::fault_line("relax-compiler", line, &mut Scan::default()), "relax-compiler's tests: its own line: {line}");
        }
        assert!(!super::fault_line("relax-front", "thread 't' panicked at libs/relax/front/src/parse.rs:1:1:", &mut Scan::default()), "relax-front's tests: its own panic");
        assert!(super::jit_fault(panic), "a refused patch's reasons are the JIT's");
        let ice = "compile error in x::f: internal compiler error: RELAX_TEST_HOT_BUILD_ERROR: forced for `f` -- a Relax bug, report it";
        let quoted = format!("assertion `left == right` failed: a forced failure is no red: panic: The live update failed in this call: {ice}");
        let report = format!(r#"  report: {{"kind":"panic","message":{},"location":"libs/devhub/src/proto.rs:330:9"}}"#, makepad_remote::json_str(&quoted));
        let failed = [
            "thread 'proto::tests::panic_event' (1008411) panicked at libs/devhub/src/proto.rs:330:9:",
            quoted.as_str(), "  left: Info", " right: Err",
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
            report.as_str(), "test proto::tests::panic_event ... FAILED",
        ];
        let mut scan = Scan::default();
        for line in failed {
            assert!(!super::fault_line("makepad-devhub", line, &mut scan), "a lane's test that quotes an internal compiler error fails as the lane's: {line}");
        }
        let mut scan = Scan::default();
        for line in &failed[..2] { super::fault_line("makepad-devhub", line, &mut scan); }
        assert!(!super::fault_line("makepad-devhub", "relax: JIT thread `x` panicked: quoted -- restarted (1 of 16)", &mut scan), "a JIT line quoted in its message is its message");
        let jits = [
            "error: internal compiler error while checking x::f -- Relax bug, report it".to_string(),
            format!(r#"  report: {{"kind":"update failed","message":"The live update failed in this call: {}","location":""}}"#, ice),
        ];
        let mut scan = Scan::default();
        for line in failed.iter().map(|l| l.to_string()).chain(jits.iter().cloned()) {
            let fault = super::fault_line("makepad-devhub", &line, &mut scan);
            assert_eq!(fault, jits.contains(&line), "after the test's failure, a real internal compiler error is still the JIT's: {line}");
        }
        assert!(!super::panic_fault("makepad-devhub", "panic", &quoted, "libs/devhub/src/proto.rs:330:9"), "a lane's panic event that quotes one is the lane's");
        assert!(super::panic_fault("makepad-devhub", "update failed", &format!("The live update failed in this call: {ice}"), ""), "a failed update's is the JIT's");
    }
}

#[cfg(test)]
mod cef_tests {
    /// A live app on a box whose checkout holds CEF is given it; on a box with none, or whose hub
    /// is too old to report the field, it starts with nothing added, whatever it links (a Fab
    /// session, 11-16). A test run, or a run that names its own, keeps its own.
    #[test]
    fn a_live_app_gets_its_boxs_cef_where_it_has_one_and_starts_without() {
        let none: Vec<(String, String)> = Vec::new();
        assert_eq!(super::cef_env(false, &none, "/srv/cef/dist").as_deref(), Some("MAKEPAD_CEF_DIST_DIR=/srv/cef/dist"));
        assert_eq!(super::cef_env(false, &none, ""), None, "a box without CEF: the app starts, nothing added");
        let old_hub = crate::lease::Caps::parse(r#"{"node":"box-a","os":"linux","cores":16}"#).unwrap();
        assert_eq!(super::cef_env(false, &none, &old_hub.cef_dist), None, "a hub that does not report the field: the app starts");
        assert_eq!(super::cef_env(true, &none, "/srv/cef/dist"), None, "a test run is left alone");
        let own = vec![("MAKEPAD_CEF_DIST_DIR".to_string(), "/mine".to_string())];
        assert_eq!(super::cef_env(false, &own, "/srv/cef/dist"), None, "its own wins");
    }
}
