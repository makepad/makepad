//! A hosted child's semantic endpoint. Clone `RemoteUi` onto a remote worker and call `dispatch`
//! there. The UI thread only applies messages and drains the outbound queue (via HostedApp).
use makepad_platform::{a11y_tree::{self as a11y, mirror::Mirror, ActionRequest, ActionResult}, makepad_hal::a11y::remote::{self, Backend, Trees}};
use makepad_remote::{Req, Out};
use makepad_studio_protocol::{AppToStudio, StudioToApp};
use std::{collections::HashMap, sync::{Arc, Condvar, Mutex, MutexGuard, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }
#[derive(Default)]
struct State {
    mirror: Mirror,
    outgoing: Vec<StudioToApp>,
    actions: HashMap<u64, Option<u8>>,
    reads: HashMap<u64, Option<u64>>,
    consumers: u32,
    last: Option<Instant>,
    closed: bool,
}
struct Shared { state: Mutex<State>, changed: Condvar, snapshots: remote::Snapshots }
#[derive(Clone)]
pub struct RemoteUi(Arc<Shared>);

impl Default for RemoteUi {
    fn default() -> Self {
        Self(Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new(), snapshots: Mutex::new(HashMap::new()) }))
    }
}
impl RemoteUi {
    /// Consumer thread only: the same ui grammar and answers as a standalone app.
    pub fn dispatch(&self, req: &Req, out: &mut Out) -> bool {
        if req.cmd != "ui" { return false; }
        remote::dispatch(req, out, self, &self.0.snapshots);
        true
    }
    pub fn trees(&self) -> Trees { <Self as Backend>::trees(self) }
    pub fn enable(&self, consumers: u32) {
        let mut state = lock(&self.0.state);
        state.consumers = consumers;
        let consumers = consumers | if state.last.is_some() { a11y::consumer::REMOTE } else { 0 };
        state.outgoing.push(StudioToApp::A11yState { consumers });
        drop(state);
        makepad_platform::SignalToUI::set_ui_signal();
    }
    pub fn send_action(&self, request: &ActionRequest) {
        let mut bytes = Vec::new(); a11y::wire::encode_action(request, &mut bytes);
        self.send(StudioToApp::A11yAction { bytes });
    }
    fn send(&self, msg: StudioToApp) {
        lock(&self.0.state).outgoing.push(msg);
        makepad_platform::SignalToUI::set_ui_signal();
    }
    pub fn drain(&self) -> Vec<StudioToApp> {
        std::mem::take(&mut lock(&self.0.state).outgoing)
    }
    pub fn reset(&mut self) {
        { let mut state = lock(&self.0.state); state.closed = true; }
        self.0.changed.notify_all();
        *self = Self::default();
    }
    pub fn apply(&self, msg: &AppToStudio) -> bool {
        // Decode outside the shared lock: a remote reader never holds up parsing.
        let update = if let AppToStudio::A11yUpdate { bytes, .. } = msg {
            match a11y::wire::decode_update(bytes) {
                Ok(update) => Some(update),
                Err(e) => { makepad_platform::error!("{e}"); return true; }
            }
        } else { None };
        let mut state = lock(&self.0.state);
        match msg {
            AppToStudio::A11yUpdate { window, .. } => { state.mirror.apply(*window as u64, update.unwrap()); }
            AppToStudio::A11yActionDone { req, result } => { if let Some(slot) = state.actions.get_mut(req) { *slot = Some(*result); } }
            AppToStudio::A11yReadDone { req, user_seq } => { if let Some(slot) = state.reads.get_mut(req) { *slot = Some(*user_seq); } }
            _ => return false,
        }
        drop(state); self.0.changed.notify_all(); true
    }
    fn wait<T>(&self, timeout: Duration, mut take: impl FnMut(&mut State) -> Option<T>) -> Result<T,String> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.0.state);
        loop {
            if let Some(value) = take(&mut state) { return Ok(value); }
            if state.closed { return Err("hosted child disconnected".into()); }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err("hosted ui timed out; inspect before retrying".into()); }
            state = self.0.changed.wait_timeout(state,left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
    fn read(&self, window: u64, present: bool, timeout: Duration) -> Result<u64,String> {
        let req = next();
        lock(&self.0.state).reads.insert(req, None);
        self.send(StudioToApp::A11yRead { req, window: window as usize, present });
        let result = self.wait(timeout, |s| s.reads.get(&req).copied().flatten());
        lock(&self.0.state).reads.remove(&req);
        result
    }
}
fn next() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 63);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
impl Backend for RemoteUi {
    fn activate(&self) {
        let start_timer = {
            let mut s = lock(&self.0.state);
            let first = s.last.is_none(); s.last = Some(Instant::now());
            if first {
                if s.consumers == 0 { s.mirror.trees.clear(); }
                let consumers = s.consumers | a11y::consumer::REMOTE;
                s.outgoing.push(StudioToApp::A11yState { consumers });
            }
            first
        };
        if start_timer {
            makepad_platform::SignalToUI::set_ui_signal();
            let weak = Arc::downgrade(&self.0);
            // One sleeping lease worker per live endpoint, not one worker per command.
            std::thread::Builder::new().name("hosted-a11y-lease".into()).spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let Some(shared) = weak.upgrade() else { break };
                    let mut s = lock(&shared.state);
                    if s.closed { break; }
                    if s.last.is_some_and(|t| t.elapsed() >= Duration::from_secs(60)) {
                        s.last = None;
                        let consumers = s.consumers;
                        if consumers == 0 { s.mirror.trees.clear(); }
                        s.outgoing.push(StudioToApp::A11yState { consumers });
                        drop(s);
                        makepad_platform::SignalToUI::set_ui_signal();
                        break;
                    }
                }
            }).expect("hosted accessibility lease timer");
        }
    }
    fn trees(&self) -> Trees {
        let mut trees = lock(&self.0.state).mirror.trees.iter().map(|(k,t)| (*k,t.clone())).collect::<Vec<_>>();
        trees.sort_by_key(|(k,_)| *k); trees
    }
    fn wait_change(&self, versions: &[(u64,u64)], timeout: Duration) {
        let _ = self.wait(timeout, |s| (s.mirror.trees.len() != versions.len() || versions.iter().any(|(k,v)| s.mirror.tree(*k).is_none_or(|t| t.updates != *v))).then_some(()));
    }
    fn action(&self, _window: u64, mut request: ActionRequest, timeout: Duration) -> Result<ActionResult,String> {
        request.req = next();
        lock(&self.0.state).actions.insert(request.req, None);
        self.send_action(&request);
        let result = self.wait(timeout, |s| s.actions.get(&request.req).copied().flatten()).map(ActionResult::from_u8);
        lock(&self.0.state).actions.remove(&request.req);
        result
    }
    fn present(&self, window: u64, timeout: Duration) -> Result<(),String> { self.read(window,true,timeout).map(|_| ()) }
    fn user_seq(&self, window: u64) -> Result<u64,String> { self.read(window,false,Duration::from_secs(5)) }
    fn type_text(&self, _window: u64, text: &str) -> Result<(),String> {
        self.send(StudioToApp::A11yType { text: text.into() });
        Ok(())
    }
}
