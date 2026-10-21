// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// A model of Sandbox's SandboxStore (asset_bridge/config.rs): a Mutex<State> polled every frame through
// `resolved()`, `generation()`, `hosting()` and `connection()`. Every MutexGuard (a local, an if-let / match
// scrutinee temporary, a tail temporary) must be released on every exit, or the next frame's lock() deadlocks.
// Each step checks with try_lock instead of blocking, so a leaked guard prints a differing line, not a hang.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};

static HANDLE_DROPS: AtomicUsize = AtomicUsize::new(0);
static LOCAL_DROPS: AtomicUsize = AtomicUsize::new(0);

pub struct LocalStore { control: SocketAddr, data: SocketAddr, id: [u8; 16], token: String }
impl Drop for LocalStore { fn drop(&mut self) { LOCAL_DROPS.fetch_add(1, Ordering::Relaxed); } }
impl LocalStore {
    fn control_addr(&self) -> SocketAddr { self.control }
    fn data_addr(&self) -> SocketAddr { self.data }
    fn server_id(&self) -> [u8; 16] { self.id }
    fn token(&self) -> &str { &self.token }
}
pub struct Resolved { local: Option<LocalStore>, note: String }
enum ResolveState { Pending(Receiver<Resolved>), Ready(Resolved), Taken }
pub struct ResolveHandle { state: ResolveState, cancelled: Arc<AtomicBool> }
impl Drop for ResolveHandle {
    fn drop(&mut self) { self.cancelled.store(true, Ordering::Release); HANDLE_DROPS.fetch_add(1, Ordering::Relaxed); }
}
impl ResolveHandle {
    fn failed(&self, why: &str) -> Resolved { Resolved { local: None, note: format!("failed: {why}") } }
    #[inline(never)]
    pub fn poll(&mut self) -> Option<Resolved> {
        if let ResolveState::Pending(rx) = &self.state {
            match rx.try_recv() {
                Ok(resolved) => self.state = ResolveState::Ready(resolved),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.state = ResolveState::Ready(self.failed("the store resolver stopped without an answer"));
                }
            }
        }
        match std::mem::replace(&mut self.state, ResolveState::Taken) {
            ResolveState::Ready(resolved) => Some(resolved),
            other => { self.state = other; None }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Endpoints { control: SocketAddr, data: SocketAddr }
enum StorePhase {
    Resolving(ResolveHandle),
    Decided { hosted: Option<(Endpoints, [u8; 16], String)>, _host: Option<LocalStore> },
}
struct StoreState { generation: u64, phase: StorePhase, attached_listen: Option<String> }

pub struct SandboxStore {
    listen: Mutex<Option<String>>,
    state: Mutex<StoreState>,
    resolve: Box<dyn Fn() -> ResolveHandle + Send + Sync>,
}

impl SandboxStore {
    fn new(resolve: impl Fn() -> ResolveHandle + Send + Sync + 'static) -> SandboxStore {
        let first = resolve();
        SandboxStore {
            listen: Mutex::new(None),
            state: Mutex::new(StoreState { generation: 0, phase: StorePhase::Resolving(first), attached_listen: None }),
            resolve: Box::new(resolve),
        }
    }
    fn lock(&self) -> MutexGuard<'_, StoreState> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }
    fn read_listen(&self) -> Option<String> { self.listen.lock().unwrap().clone() }

    #[inline(never)]
    pub fn resolved(&self) -> bool {
        let mut state = self.lock();
        if let StorePhase::Resolving(handle) = &mut state.phase {
            let Some(resolved) = handle.poll() else { return false };
            let _note = resolved.note.len();
            let hosted = resolved.local.as_ref().map(|local| {
                let endpoints = Endpoints { control: local.control_addr(), data: local.data_addr() };
                (endpoints, local.server_id(), local.token().to_string())
            });
            let listen = hosted.is_none().then(|| self.read_listen()).flatten().map(|t| t.trim().to_string());
            let same_host = listen.is_some() && listen == state.attached_listen;
            state.attached_listen = listen;
            state.phase = StorePhase::Decided { hosted, _host: resolved.local };
            if !same_host {
                state.generation += 1;
            }
        }
        true
    }
    #[inline(never)]
    pub fn generation(&self) -> u64 { self.lock().generation }
    #[inline(never)]
    pub fn hosting(&self) -> bool {
        self.resolved();
        matches!(self.lock().phase, StorePhase::Decided { hosted: Some(_), .. })
    }
    #[inline(never)]
    pub fn host_lost(&self, generation: u64) {
        let mut state = self.lock();
        let attached = matches!(state.phase, StorePhase::Decided { hosted: None, .. });
        if attached && state.generation == generation {
            state.phase = StorePhase::Resolving((self.resolve)());
        }
    }
    #[inline(never)]
    pub fn connection(&self) -> Option<(Endpoints, [u8; 16], Option<String>)> {
        if !self.resolved() {
            return None;
        }
        if let StorePhase::Decided { hosted: Some((endpoints, id, token)), .. } = &self.lock().phase {
            return Some((*endpoints, *id, Some(token.clone())));
        }
        let listen = self.read_listen()?;
        let mut parts = listen.split(' ');
        let control: SocketAddr = parts.next()?.parse().ok()?;
        let data: SocketAddr = parts.next()?.parse().ok()?;
        Some((Endpoints { control, data }, [7; 16], None))
    }
    fn free(&self) -> bool { self.state.try_lock().is_ok() }
}

thread_local! { static SENDERS: std::cell::RefCell<Vec<Sender<Resolved>>> = const { std::cell::RefCell::new(Vec::new()) }; }

fn work() {
    HANDLE_DROPS.store(0, Ordering::Relaxed);
    LOCAL_DROPS.store(0, Ordering::Relaxed);
    let resolve = || {
        let (tx, rx) = channel();
        SENDERS.with(|s| s.borrow_mut().push(tx));
        ResolveHandle { state: ResolveState::Pending(rx), cancelled: Arc::new(AtomicBool::new(false)) }
    };
    let store = SandboxStore::new(resolve);
    // frames while the resolver has not answered
    let mut leaked = false;
    let mut step = |what: &str, store: &SandboxStore, f: &dyn Fn(&SandboxStore) -> String| {
        if leaked { d!("{what}: skipped (lock leaked)"); return; }
        let r = f(store);
        let free = store.free();
        d!("{what}: {r} free {free}");
        leaked = !free;
    };
    for frame in 0..3 { step(&format!("pending resolved #{frame}"), &store, &|s| format!("{}", s.resolved())); }
    step("pending generation", &store, &|s| format!("{}", s.generation()));
    step("pending hosting", &store, &|s| format!("{}", s.hosting()));
    step("pending connection", &store, &|s| format!("{:?}", s.connection()));
    // the resolver answers: hosted here
    let local = LocalStore { control: "127.0.0.1:4000".parse().unwrap(), data: "127.0.0.1:4001".parse().unwrap(), id: [3; 16], token: "tok".into() };
    SENDERS.with(|s| s.borrow()[0].send(Resolved { local: Some(local), note: "hosting".into() }).unwrap());
    step("hosted resolved", &store, &|s| format!("{}", s.resolved()));
    step("hosted generation", &store, &|s| format!("{}", s.generation()));
    step("hosted hosting", &store, &|s| format!("{}", s.hosting()));
    step("hosted connection", &store, &|s| format!("{:?}", s.connection().map(|c| (c.0.control.port(), c.1[0], c.2))));
    step("host_lost (hosting: no-op)", &store, &|s| { s.host_lost(1); format!("{}", s.generation()) });
    // a second store that attaches to another host, loses it, re-resolves, attaches again
    let store2 = SandboxStore::new(resolve);
    *store2.listen.lock().unwrap() = Some("10.0.0.1:5000 10.0.0.1:5001 ".into());
    SENDERS.with(|s| s.borrow()[1].send(Resolved { local: None, note: "attached".into() }).unwrap());
    step("attached resolved", &store2, &|s| format!("{}", s.resolved()));
    step("attached connection", &store2, &|s| format!("{:?}", s.connection()));
    step("attached host_lost", &store2, &|s| { s.host_lost(s.generation()); format!("{}", s.resolved()) });
    step("re-resolving connection", &store2, &|s| format!("{:?}", s.connection()));
    SENDERS.with(|s| s.borrow_mut().truncate(2));
    step("resolver died", &store2, &|s| format!("{} gen {}", s.resolved(), s.generation()));
    step("bad listen", &store2, &|s| { *s.listen.lock().unwrap() = Some("nonsense".into()); format!("{:?}", s.connection()) });
    if leaked {
        std::mem::forget(store);
        std::mem::forget(store2);
    } else {
        drop(store);
        drop(store2);
    }
    SENDERS.with(|s| s.borrow_mut().clear());
    d!("handle drops {} local drops {}", HANDLE_DROPS.load(Ordering::Relaxed), LOCAL_DROPS.load(Ordering::Relaxed));
}
