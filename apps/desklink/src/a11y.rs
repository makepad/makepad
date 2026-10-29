//! Consumer-thread access to the existing authenticated connections. Network replies wake
//! callers; neither a semantic wait nor remote input runs on the viewer's UI thread.
use makepad_desk_link::ClientMsg;
use makepad_remote::{json_str, Out, Req};
use std::{collections::HashMap, sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc::Sender, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};

#[derive(Default)]
struct State { connected: bool, pending: HashMap<u64, Option<Result<String, String>>> }
pub struct Endpoint { tx: Sender<ClientMsg>, state: Mutex<State>, changed: Condvar }
fn tabs() -> &'static Mutex<Vec<(String, Weak<Endpoint>)>> {
    static TABS: OnceLock<Mutex<Vec<(String, Weak<Endpoint>)>>> = OnceLock::new();
    TABS.get_or_init(Default::default)
}
pub fn register(name: &str, tx: Sender<ClientMsg>) -> Arc<Endpoint> {
    let endpoint = Arc::new(Endpoint { tx, state: Mutex::new(State::default()), changed: Condvar::new() });
    {
        let mut tabs = tabs().lock().unwrap();
        tabs.retain(|(_, endpoint)| endpoint.strong_count() != 0);
        tabs.push((name.into(), Arc::downgrade(&endpoint)));
    }
    makepad_widgets::makepad_platform::makepad_hal::a11y::register_ui_router(dispatch);
    endpoint
}
impl Endpoint {
    pub fn connected(&self, connected: bool) {
        let mut state = self.state.lock().unwrap();
        state.connected = connected;
        if !connected {
            for result in state.pending.values_mut() {
                if result.is_none() { *result = Some(Err("desktop disconnected; action may have applied; inspect before retrying".into())); }
            }
        }
        drop(state);
        self.changed.notify_all();
    }
    pub fn pending(&self, req: u64) -> bool { self.state.lock().unwrap().pending.get(&req).is_some_and(Option::is_none) }
    pub fn reply(&self, req: u64, text: String) {
        if let Some(result) = self.state.lock().unwrap().pending.get_mut(&req) { *result = Some(Ok(text)); }
        self.changed.notify_all();
    }
    fn query(&self, line: String, timeout: Duration, out: &Out) -> Result<String, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let req = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap();
        if !state.connected { return Err("desktop is not connected".into()); }
        state.pending.insert(req, None);
        if self.tx.send(ClientMsg::A11yQuery { req, line }).is_err() {
            state.pending.remove(&req);
            return Err("desktop connection stopped".into());
        }
        let start = Instant::now();
        let result = loop {
            if let Some(result) = state.pending.get_mut(&req).and_then(Option::take) { break result; }
            if out.cancelled() { break Err("semantic caller disconnected".into()); }
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() { break Err("desktop query timed out; action may have applied; inspect before retrying".into()); }
            state = self.changed.wait_timeout(state, left.min(Duration::from_millis(100))).unwrap().0;
        };
        state.pending.remove(&req);
        result
    }
}
fn dispatch(req: &Req, out: &mut Out) -> bool {
    let Some(machine) = req.get("machine") else { return false; };
    let tabs: Vec<_> = tabs().lock().unwrap().iter().filter_map(|(name,weak)| weak.upgrade().map(|e| (name.clone(), e))).collect();
    out.limit = usize::MAX;
    if machine == "list" {
        for (index, (name, _)) in tabs.iter().enumerate() {
            out.line(&format!("{{\"machine\":{index},\"name\":{}}}", json_str(name)));
        }
        return true;
    }
    let matches: Vec<_> = if let Ok(index) = machine.parse::<usize>() {
        tabs.get(index).map(|tab| (index, tab)).into_iter().collect()
    } else {
        tabs.iter().enumerate().filter(|(_, (name,_))| name == machine).collect()
    };
    let [(_, (_, endpoint))] = matches.as_slice() else {
        out.err(if matches.is_empty() { "no such desktop; ui machine=list lists tabs" } else { "ambiguous desktop; use its tab index" });
        return true;
    };
    let command = std::iter::once(req.cmd.as_str()).chain(req.pos.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
    let mut fields = vec![format!("\"cmd\":{}", json_str(&command))];
    fields.extend(req.kv.iter().filter(|(k,_)| k != "machine").map(|(k,v)| format!("{}:{}", json_str(k), json_str(v))));
    let timeout = Duration::from_millis(req.num("timeout_ms", 5000).saturating_add(5000));
    match endpoint.query(format!("{{{}}}", fields.join(",")), timeout, out) {
        Ok(text) => { for line in text.lines() { out.line(line); } }
        Err(error) => out.err(&error),
    }
    true
}
