//! The native WM shell's studio semantic transport. It exports the HAL mirror and
//! uses the same action/presentation backend as a local remote consumer.
use super::{Hub, Key};
use crate::a11y::{self, remote::Backend};
use makepad_a11y::{consumer, TreeUpdate};
use makepad_studio_protocol::{AppToStudio, StudioToApp};
use std::{collections::{HashMap, VecDeque}, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}, mpsc::{sync_channel, SyncSender, TrySendError}}, time::Duration};

enum Work {
    Request(StudioToApp),
    Update(u64, u64, Vec<u8>),
    Window(usize, Option<Key>),
    Hub(Arc<Hub>),
    Detached,
    Stop,
}
struct Queue { tx: SyncSender<Work>, overflow: VecDeque<Work> }
struct Bridge { queue: Mutex<Queue>, active: AtomicBool }
static BRIDGE: OnceLock<Bridge> = OnceLock::new();
fn post(work: Work) {
    let Some(bridge) = BRIDGE.get() else { return; };
    let mut queue = bridge.queue.lock().unwrap();
    if !queue.overflow.is_empty() { queue.overflow.push_back(work); }
    else if let Err(TrySendError::Full(work)) = queue.tx.try_send(work) {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| eprintln!("[hal-wm] semantic channel full; retaining queued work"));
        queue.overflow.push_back(work);
    }
}
pub(super) fn start(hub: Arc<Hub>) {
    BRIDGE.get_or_init(|| {
        let (tx, rx) = sync_channel(1);
        std::thread::Builder::new().name("wm-semantics".into()).spawn(move || {
            let mut hub: Option<Arc<Hub>> = None;
            let mut windows = HashMap::<usize, u64>::new();
            let mut versions = HashMap::<u64, u64>::new();
            let mut active = false;
            while let Ok(first) = rx.recv() {
                let mut work = VecDeque::from([first]);
                if let Some(bridge) = BRIDGE.get() { work.append(&mut std::mem::take(&mut bridge.queue.lock().unwrap().overflow)); }
                for work in work {
                    match work {
                        Work::Stop => { a11y::request_consumer(consumer::HOST, false); return; }
                        Work::Detached => {
                            hub = None; active = false; versions.clear();
                            a11y::request_consumer(consumer::HOST, false);
                        }
                        Work::Hub(new) => {
                            hub = Some(new); active = false; versions.clear();
                            a11y::request_consumer(consumer::HOST, false);
                        }
                        Work::Window(window, key) => {
                            if let Some(key) = key {
                                windows.insert(window, a11y::window_key(key.0, key.1.index));
                                if active { snapshot(&hub, &windows, &mut versions); }
                            }
                            else if let Some(key) = windows.remove(&window) {
                                versions.remove(&key);
                                if active {
                                    let mut bytes = Vec::new();
                                    makepad_a11y::wire::encode_update(&TreeUpdate { full: true, ..Default::default() }, &mut bytes);
                                    if let Some(hub) = &hub { hub.send(vec![AppToStudio::A11yUpdate { window, bytes }]); }
                                }
                            }
                        }
                        Work::Update(key, version, bytes) => {
                            if !active || versions.get(&key).is_some_and(|seen| *seen >= version) { continue; }
                            if let Some((&window, _)) = windows.iter().find(|(_, k)| **k == key) {
                                versions.insert(key, version);
                                if let Some(hub) = &hub { hub.send(vec![AppToStudio::A11yUpdate { window, bytes }]); }
                            }
                        }
                        Work::Request(StudioToApp::A11yState { consumers }) => {
                            active = consumers != 0;
                            if let Some(bridge) = BRIDGE.get() { bridge.active.store(active, Ordering::Release); }
                            a11y::request_consumer(consumer::HOST, active);
                            versions.clear();
                            // An OS consumer may already have a complete tree. Send it before
                            // deltas, skipping queued versions already covered by this snapshot.
                            if active {
                                snapshot(&hub, &windows, &mut versions);
                            }
                        }
                        Work::Request(StudioToApp::A11yAction { bytes }) => {
                            if let Ok(action) = makepad_a11y::wire::decode_action(&bytes) {
                                let req = action.req;
                                let key = windows.values().copied().find(|k| a11y::with_tree(*k, |t| t.node(action.node).is_some()) == Some(true));
                                let result = key.and_then(|key| a11y::Native.action(key, action, Duration::from_secs(5)).ok()).unwrap_or(makepad_a11y::ActionResult::NoSuchNode);
                                if let Some(hub) = &hub { hub.send(vec![AppToStudio::A11yActionDone { req, result: result as u8 }]); }
                            }
                        }
                        Work::Request(StudioToApp::A11yRead { req, window, present }) => {
                            if let Some(&key) = windows.get(&window) {
                                let presented = !present || a11y::Native.present(key, Duration::from_secs(5)).is_ok();
                                if presented {
                                    if present { snapshot(&hub, &windows, &mut versions); }
                                    if let Ok(user_seq) = a11y::Native.user_seq(key) {
                                        if let Some(hub) = &hub { hub.send(vec![AppToStudio::A11yReadDone { req, user_seq }]); }
                                    }
                                }
                            }
                        }
                        Work::Request(StudioToApp::A11yType { text }) => {
                            if let Some((_, &key)) = windows.iter().min_by_key(|(window,_)| *window) { let _ = a11y::Native.type_text(key, &text); }
                        }
                        Work::Request(_) => (),
                    }
                }
            }
        }).expect("WM semantic worker");
        Bridge { queue: Mutex::new(Queue { tx, overflow: VecDeque::new() }), active: AtomicBool::new(false) }
    });
    if let Some(bridge) = BRIDGE.get() { bridge.active.store(false, Ordering::Release); }
    post(Work::Hub(hub));
}
pub(super) fn request(message: &StudioToApp) -> bool {
    if !matches!(message, StudioToApp::A11yState { .. } | StudioToApp::A11yAction { .. } | StudioToApp::A11yRead { .. } | StudioToApp::A11yType { .. }) { return false; }
    post(Work::Request(message.clone())); true
}
pub(super) fn window(window: usize, key: Option<Key>) { post(Work::Window(window, key)); }
pub(super) fn detached() {
    if let Some(bridge) = BRIDGE.get() { bridge.active.store(false, Ordering::Release); }
    post(Work::Detached);
}
pub(super) fn stop() {
    if let Some(bridge) = BRIDGE.get() { bridge.active.store(false, Ordering::Release); }
    post(Work::Stop);
}
pub(crate) fn update(key: u64, bytes: &[u8]) {
    if BRIDGE.get().is_some_and(|b| b.active.load(Ordering::Acquire)) {
        let version = a11y::with_tree(key, |tree| tree.updates).unwrap_or(0);
        post(Work::Update(key, version, bytes.to_vec()));
    }
}

fn snapshot(hub: &Option<Arc<Hub>>, windows: &HashMap<usize, u64>, versions: &mut HashMap<u64, u64>) {
    for (key, tree) in a11y::Native.trees() {
        let Some((&window, _)) = windows.iter().find(|(_, k)| **k == key) else { continue; };
        let update = TreeUpdate { full: true, root: tree.root, focus: tree.focus, nodes: tree.nodes.values().cloned().collect(), ..Default::default() };
        let mut bytes = Vec::new(); makepad_a11y::wire::encode_update(&update, &mut bytes);
        versions.insert(key, tree.updates);
        if let Some(hub) = hub { hub.send(vec![AppToStudio::A11yUpdate { window, bytes }]); }
    }
}
