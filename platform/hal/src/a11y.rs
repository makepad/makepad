//! Semantic trees shared by native adapters, remote inspection and automation.
//! The UI only applies updates and completes requests; waits belong to consumer threads.
use crate::{lock, Hal};
use makepad_a11y::{
    consumer,
    mirror::{Mirror, Tree},
    ActionRequest, ActionResult, Notification, TreeUpdate,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

#[cfg(not(target_arch = "wasm32"))]
pub mod remote;

pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("ui", "[q=QUERY] [node=ID] [depth=N] [w=SESSION:WINDOW] [json=1] [app=NAME_OR_ID] [machine=TAB]", "semantic tree: role, name, state, bounds and actions"),
    ("ui find", "[q=QUERY|role=ROLE name=NAME state=STATE path=PATH] [nth=N] [w=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "matching node lines and count; nth is zero-based"),
    ("ui act", "node=ID|q=QUERY ACTION [v=VALUE] [t=TEXT] [from= to=] [dx= dy=] [name=CUSTOM] [nth=] [w=] [wait=1] [if_user_seq=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "action result and resulting node; wait=1 includes presentation"),
    ("ui type", "node=ID|q=QUERY t=TEXT [from= to=] [nth=] [w=] [if_user_seq=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "focus then replace the selection; raw typing only for nodes without a text action"),
    ("ui wait", "q=QUERY [gone=1|changed=1|count=N] [timeout_ms=5000] [w=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "matching nodes and elapsed ms when the condition holds; timeout is an error"),
    ("ui snap", "name=NAME [w=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "save a named semantic tree snapshot"),
    ("ui diff", "a=NAME [b=now|NAME] [w=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "semantic changes: added, removed and changed nodes"),
    ("ui speak", "[since=N] [node=ID] [w=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "screen-reader announcement text and next sequence number"),
    ("ui lint", "[w=] [json=1] [app=NAME_OR_ID] [machine=TAB]", "accessibility issues and count"),
    ("ui oscheck", "[w=] [act=1] [json=1] [app=NAME_OR_ID] [machine=TAB]", "OS accessibility walk compared with the mirror: mismatches first"),
];

/// The OS adapter's in-process self-check: walks `window_key`'s tree through the OS accessibility API
/// the way an assistive technology would and reports mismatches against the mirror (token-lean text);
/// `act` also performs one action through the OS API. Each adapter registers it when it starts.
pub type OsSelfCheck = fn(window_key: u64, act: bool) -> String;

fn os_selfcheck() -> &'static OnceLock<OsSelfCheck> {
    static CHECK: OnceLock<OsSelfCheck> = OnceLock::new();
    &CHECK
}

pub fn set_os_selfcheck(f: OsSelfCheck) {
    let _ = os_selfcheck().set(f);
}

fn consumer_bits() -> &'static AtomicU32 {
    static BITS: OnceLock<AtomicU32> = OnceLock::new();
    BITS.get_or_init(|| {
        AtomicU32::new(if std::env::var("MAKEPAD_A11Y").as_deref() == Ok("1") {
            consumer::TEST
        } else {
            0
        })
    })
}
pub fn consumers() -> u32 {
    consumer_bits().load(Ordering::Acquire)
}
pub fn window_key(session: u32, window: u32) -> u64 {
    ((session as u64) << 32) | window as u64
}

/// Hosted shells supply desktop placement separately from their input coordinates.
pub fn set_screen_origin(key: u64, origin: Option<(f32, f32)>) {
    #[cfg(target_os = "linux")]
    crate::backend::linux::atspi::geometry(key, origin.filter(|(x,y)| x.is_finite() && y.is_finite()));
    #[cfg(not(target_os = "linux"))]
    let _ = (key, origin);
}
pub fn set_window_active(key: u64, active: bool) {
    #[cfg(target_os = "linux")]
    crate::backend::linux::atspi::focus(key, active);
    #[cfg(not(target_os = "linux"))]
    let _ = (key, active);
}

/// Every session's window trees, keyed by [`window_key`]. Its own short-held lock, apart from the HAL
/// state, so OS accessibility callbacks (any thread, re-entrant from inside the OS's event loop) read it
/// without touching the HAL. Lock order: the HAL state, then this. Never call into the OS or the HAL while
/// holding it.
fn mirrors() -> &'static Mutex<Mirror> {
    static MIRRORS: OnceLock<Mutex<Mirror>> = OnceLock::new();
    MIRRORS.get_or_init(|| Mutex::new(Mirror::default()))
}

/// Applies a window's update; returns its notifications for the backend's adapter.
pub(crate) fn apply_update(session: u32, window: u32, update: TreeUpdate) -> Vec<Notification> {
    lock(mirrors()).apply(window_key(session, window), update)
}

pub(crate) fn drop_window(session: u32, window: u32) {
    lock(mirrors()).drop_window(window_key(session, window));
    #[cfg(target_os = "linux")]
    crate::backend::linux::atspi::notify(window_key(session, window), &[]);
}

fn drop_session(session: u32) {
    lock(mirrors()).trees.retain(|key, _| (key >> 32) as u32 != session);
    #[cfg(target_os = "linux")]
    crate::backend::linux::atspi::session_closed(session);
}

/// Reads one window's tree (`window_key(session, window)`). Keep `f` short.
pub fn with_tree<R>(key: u64, f: impl FnOnce(&Tree) -> R) -> Option<R> {
    lock(mirrors()).tree(key).map(f)
}

/// Reads every tree. Keep `f` short.
pub fn with_mirror<R>(f: impl FnOnce(&Mirror) -> R) -> R {
    f(&lock(mirrors()))
}

pub fn request_consumer(bit: u32, on: bool) {
    request_consumer_in(Hal::global(), bit, on);
}

pub(crate) fn request_consumer_in(hal: &Hal, bit: u32, on: bool) {
    // Serialize transitions with session creation so a newly opened session cannot miss one.
    let mut state = lock(&hal.state);
    let old = consumers();
    let new = if on { old | bit } else { old & !bit };
    if new == old {
        return;
    }
    consumer_bits().store(new, Ordering::Release);
    let sessions = state.world.session_ids();
    for sid in &sessions {
        state
            .world
            .to_session(*sid, |q| q.a11y_state(u32::from(new != 0), new));
    }
    drop(state);
    hal.sig.notify(&sessions);
}

#[derive(Default)]
struct Completions {
    requests: HashMap<u64, (u32, Option<u8>)>,
}
fn completions() -> &'static (Mutex<Completions>, Condvar) {
    static DONE: OnceLock<(Mutex<Completions>, Condvar)> = OnceLock::new();
    DONE.get_or_init(|| (Mutex::new(Completions::default()), Condvar::new()))
}

/// Enqueue for the owning session. A zero request id is replaced with a process-unique id.
pub fn inject_action(window: u64, action: ActionRequest) -> u64 {
    inject_action_in(Hal::global(), window, action)
}

pub(crate) fn inject_action_in(hal: &Hal, window: u64, mut action: ActionRequest) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let sid = (window >> 32) as u32;
    let (done, _) = completions();
    {
        let mut done = lock(done);
        if action.req == 0 || done.requests.contains_key(&action.req) {
            loop {
                action.req = NEXT.fetch_add(1, Ordering::Relaxed);
                if action.req != 0 && !done.requests.contains_key(&action.req) {
                    break;
                }
            }
        }
        done.requests.insert(action.req, (sid, None));
    }
    let mut bytes = Vec::new();
    makepad_a11y::wire::encode_action(&action, &mut bytes);
    let mut state = lock(&hal.state);
    let id = state.world.session(sid).and_then(|s| {
        s.windows
            .keys_of(sid)
            .into_iter()
            .find(|id| id.index == window as u32)
    });
    if let Some(id) = id {
        state.world.to_session(sid, |q| q.a11y_action(id, &bytes));
    } else {
        action_done(sid, action.req, ActionResult::NoSuchNode as u8);
    }
    drop(state);
    hal.sig.notify(&[sid]);
    action.req
}

/// Release a fire-and-forget caller's completion slot. The queued action still runs.
pub fn forget_action(req: u64) {
    lock(&completions().0).requests.remove(&req);
}

pub(crate) fn action_done(session: u32, req: u64, result: u8) {
    let (done, cv) = completions();
    if let Some((owner, value)) = lock(done).requests.get_mut(&req) {
        if *owner == session {
            *value = Some(ActionResult::from_u8(result) as u8);
        }
    }
    cv.notify_all();
}

pub(crate) fn close_session(session: u32) {
    drop_session(session);
    let (done, cv) = completions();
    lock(done)
        .requests
        .retain(|_, (owner, _)| *owner != session);
    cv.notify_all();
}

/// Remote/automation threads only. Timeout leaves the result available for a later wait.
pub fn wait_done(req: u64, timeout: Duration) -> Option<u8> {
    let (done, cv) = completions();
    let deadline = Instant::now() + timeout;
    let mut done = lock(done);
    loop {
        let (_, result) = *done.requests.get(&req)?;
        if let Some(result) = result {
            done.requests.remove(&req);
            return Some(result);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        done = cv
            .wait_timeout(done, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// One long-lived consumer timer, independent of frames and the UI thread. Holding the lease
/// through both transitions prevents an expiring request from switching off a newer request.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remote_activity() {
    static LEASE: OnceLock<std::sync::Arc<(Mutex<Instant>, Condvar)>> = OnceLock::new();
    let lease = LEASE.get_or_init(|| {
        let lease = std::sync::Arc::new((Mutex::new(Instant::now()), Condvar::new()));
        let timer = lease.clone();
        std::thread::Builder::new().name("a11y-remote-lease".into()).spawn(move || {
            let (last, cv) = &*timer;
            let mut last = lock(last);
            loop {
                let left = Duration::from_secs(60).saturating_sub(last.elapsed());
                if left.is_zero() {
                    request_consumer(consumer::REMOTE, false);
                    last = cv.wait(last).unwrap_or_else(|e| e.into_inner());
                } else {
                    last = cv.wait_timeout(last, left).unwrap_or_else(|e| e.into_inner()).0;
                }
            }
        }).expect("remote consumer timer");
        lease
    });
    let mut last = lock(&lease.0);
    *last = Instant::now();
    request_consumer(consumer::REMOTE, true);
    lease.1.notify_one();
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Native;
#[cfg(not(target_arch = "wasm32"))]
impl remote::Backend for Native {
    fn activate(&self) {
        let previous = (consumers() == 0).then(|| remote::versions(&self.trees()));
        remote_activity();
        // Reactivation schedules a full draw. Do not answer from the tree left over
        // before the last consumer went away.
        if let Some(previous) = previous { self.wait_change(&previous, Duration::from_secs(5)); }
    }
    fn trees(&self) -> remote::Trees {
        let mut trees = with_mirror(|m| m.trees.iter().map(|(k,t)| (*k,t.clone())).collect::<Vec<_>>());
        trees.sort_by_key(|(k,_)| *k);
        trees
    }
    fn wait_change(&self, versions: &[(u64,u64)], timeout: Duration) {
        Hal::global().wait_until(timeout, |_| {
            with_mirror(|m| m.trees.len() != versions.len() || versions.iter().any(|(k,v)| m.tree(*k).is_none_or(|t| t.updates != *v)))
        });
    }
    fn action(&self, window: u64, request: ActionRequest, timeout: Duration) -> Result<ActionResult,String> {
        let req = inject_action(window, request);
        wait_done(req, timeout).map(ActionResult::from_u8).ok_or_else(|| format!("action {req} timed out; inspect before retrying"))
    }
    fn present(&self, window: u64, timeout: Duration) -> Result<(), String> {
        let query = format!("a11y_present {}", window as u32);
        let reply = Hal::global().query((window >> 32) as u32, query.as_bytes(), timeout).ok_or("presentation timed out; action may already be applied")?;
        if reply == b"ok" { Ok(()) } else { Err(String::from_utf8_lossy(&reply).into_owned()) }
    }
    fn type_text(&self, window: u64, text: &str) -> Result<(), String> {
        let reply = Hal::global().query((window >> 32) as u32, format!("a11y_type\n{text}").as_bytes(), Duration::from_secs(5)).ok_or("text input timed out")?;
        if reply == b"ok" { Ok(()) } else { Err(String::from_utf8_lossy(&reply).into_owned()) }
    }

    fn user_seq(&self, window: u64) -> Result<u64, String> {
        let reply = Hal::global().query((window >> 32) as u32, b"a11y_user_seq", Duration::from_secs(5)).ok_or("activity query timed out")?;
        String::from_utf8_lossy(&reply).parse().map_err(|_| "invalid activity counter".into())
    }
}

/// Optional host routing, called on the consumer worker before the local mirrors.
/// The callback must release its own registry locks before waiting or dispatching.
#[cfg(not(target_arch = "wasm32"))]
pub type UiRouter = fn(&makepad_remote::Req, &mut makepad_remote::Out) -> bool;
#[cfg(not(target_arch = "wasm32"))]
static UI_ROUTERS: Mutex<Vec<UiRouter>> = Mutex::new(Vec::new());
#[cfg(not(target_arch = "wasm32"))]
pub fn register_ui_router(router: UiRouter) {
    let mut routers = lock(&UI_ROUTERS);
    if !routers.iter().any(|existing| std::ptr::fn_addr_eq(*existing, router)) {
        routers.push(router);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch(req: &makepad_remote::Req, out: &mut makepad_remote::Out) -> bool {
    if req.cmd != "ui" { return false; }
    let routers = lock(&UI_ROUTERS).clone();
    if routers.into_iter().any(|route| route(req, out)) { return true; }
    if req.get("app").is_some() || req.get("machine").is_some() {
        out.err("this endpoint has no matching semantic host route");
        return true;
    }
    dispatch_local(req, out)
}

/// Local HAL sessions only; hosts use this to include their own shell without routing again.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_local(req: &makepad_remote::Req, out: &mut makepad_remote::Out) -> bool {
    if req.cmd != "ui" { return false; }
    static SNAPSHOTS: OnceLock<remote::Snapshots> = OnceLock::new();
    remote::dispatch(req, out, &Native, SNAPSHOTS.get_or_init(Default::default));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend_id, window_kind, CmdWriter, Id, OpenReq};
    use makepad_a11y::{Node, NodeId, Role, TreeUpdate};

    #[test]
    fn commands_apply_to_owning_window_and_teardown_drops_mirrors() {
        let hal = Hal::new();
        let a = hal
            .open(&OpenReq {
                backend: backend_id::HEADLESS,
                ..Default::default()
            })
            .session;
        let b = hal
            .open(&OpenReq {
                backend: backend_id::HEADLESS,
                ..Default::default()
            })
            .session;
        let window = Id::new(0, 1);
        let mut bytes = Vec::new();
        for sid in [a, b] {
            let mut writer = CmdWriter::new(sid);
            writer.window_create(
                window,
                window_kind::NORMAL,
                Id::NONE,
                [0.0, 0.0, 100.0, 100.0],
                false,
            );
            let update = TreeUpdate {
                full: true,
                root: NodeId(1),
                nodes: vec![Node {
                    id: NodeId(1),
                    role: Role::Window,
                    name: sid.to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            makepad_a11y::wire::encode_update(&update, &mut bytes);
            writer.a11y_update(window, &bytes);
            hal.submit(&writer.buf());
        }
        for sid in [a, b] {
            assert_eq!(
                with_tree(window_key(sid, 0), |t| t.node(NodeId(1)).unwrap().name.clone()).unwrap(),
                sid.to_string()
            );
        }
        let mut writer = CmdWriter::new(a);
        writer.window_close(window);
        hal.submit(&writer.buf());
        let has_trees = |sid: u32| with_mirror(|m| m.trees.keys().any(|k| (k >> 32) as u32 == sid));
        assert!(!has_trees(a));
        assert!(has_trees(b));
        assert!(hal.close(b));
        assert!(lock(&hal.state).world.session(b).is_none());
        assert!(!has_trees(b));
        hal.close(a);
    }

    #[test]
    fn injected_action_round_trips_through_the_session_queue_and_completion_command() {
        use crate::{EventStore, HalEvent};
        use makepad_a11y::Actions;
        let hal = Hal::global();
        let sid = hal
            .open(&OpenReq {
                backend: backend_id::HEADLESS,
                ..Default::default()
            })
            .session;
        let window = Id::new(0, 1);
        let mut writer = CmdWriter::new(sid);
        writer.window_create(
            window,
            window_kind::NORMAL,
            Id::NONE,
            [0.0, 0.0, 100.0, 100.0],
            false,
        );
        hal.submit(&writer.buf());
        let req = inject_action(
            window_key(sid, window.index),
            ActionRequest {
                req: 0,
                node: NodeId(123),
                action: Actions::TOGGLE,
                arg: Default::default()
            },
        );
        assert_ne!(req, 0);
        let mut store = EventStore::new(sid, 64, 4096);
        let mut buf = store.buf();
        let count = hal.pump(0, &mut buf);
        let mut received = false;
        for (_, event) in store.reader(count, buf.arena_len) {
            if let HalEvent::A11yAction {
                window: target,
                bytes,
            } = event
            {
                let action = makepad_a11y::wire::decode_action(bytes).unwrap();
                assert_eq!(target, window);
                assert_eq!(action.req, req);
                assert_eq!(action.node, NodeId(123));
                assert_eq!(action.action, Actions::TOGGLE);
                received = true;
            }
        }
        assert!(received);
        writer.clear();
        writer.a11y_action_done(req, ActionResult::Handled as u32);
        hal.submit(&writer.buf());
        assert_eq!(
            wait_done(req, Duration::ZERO),
            Some(ActionResult::Handled as u8)
        );
        let missing = inject_action(window_key(sid, 99), ActionRequest {
            req: 0, node: NodeId(123), action: Actions::TOGGLE, arg: Default::default()
        });
        assert_eq!(
            wait_done(missing, Duration::ZERO),
            Some(ActionResult::NoSuchNode as u8)
        );
        assert!(hal.close(sid));
    }

    #[test]
    fn completion_survives_timeout_until_wait_or_session_end() {
        let sid = u32::MAX;
        let req = u64::MAX;
        lock(&completions().0).requests.insert(req, (sid, None));
        assert_eq!(wait_done(req, Duration::ZERO), None);
        action_done(sid - 1, req, ActionResult::Handled as u8);
        assert_eq!(
            wait_done(req, Duration::ZERO),
            None,
            "another session cannot complete the request"
        );
        action_done(sid, req, ActionResult::Fallback as u8);
        assert_eq!(
            wait_done(req, Duration::ZERO),
            Some(ActionResult::Fallback as u8)
        );
        assert_eq!(wait_done(req, Duration::ZERO), None);
        lock(&completions().0).requests.insert(req, (sid, Some(0)));
        close_session(sid);
        assert_eq!(wait_done(req, Duration::ZERO), None);
    }
}
