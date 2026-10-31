//! Commercial-lane blockers, round 3 (Stage under `makepad run`); each = rustc --test.
//! 1. SILENT (flow graph/mod.rs:2826): a closure that reads a shared reference only through
//!    it (`node.id`) captures a copy of the reference (edition 2021 captures the place
//!    `(*node).id`; rustc truncates it at the deref to a copy of `node`, upvar.rs
//!    truncate_capture_for_optimization). A pointer to the local itself dangled once the
//!    outer closure returned.
//! 2. stage_ui recent_searches.rs:56: `opt.map_or(&[], |(_, l)| l.queries())` returning
//!    `&[String]`: the expected result gives map_or's `U` (rustc
//!    expected_inputs_for_expected_output, per input), so `&[]` is checked against `&[String]`
//!    and unsizes from `&[String; 0]`.
//! 3. widgets grid.rs:1343: `iter.map(fn_item).try_fold(0usize, usize::checked_add)`.
//! 4. Stage (io_err): `File::open(p).map_err(io_err("open"))?` with
//!    `fn io_err(op) -> impl Fn(io::Error) -> ClientError`: map_err's F comes from the opaque's
//!    Fn bound (ClientError) before `?` converts it into the fn's error type.
//! 5. Stage TOOLS table: `concat!` with `macro_rules!` calls among its arguments (rustc expands
//!    them first).
//! 6. Stage studio_api_host: `spawner.spawn_worker(options, body)` with
//!    body: Box<dyn FnOnce() + Send> and `F: FnOnce() -> T`: T = () through the Box impl
//!    (`<Box<F> as FnOnce<A>>::Output = <F as FnOnce<A>>::Output`), plus a generic constructor
//!    whose J/D come only from the closure body's call.
use std::collections::HashMap;

struct Port { name: String }
struct Node { id: String, outputs: Vec<Port> }

#[inline(never)]
fn clobber() -> u64 {
    let a = [7u64; 64];
    a.iter().sum()
}

fn nodes() -> Vec<Node> {
    vec![
        Node { id: "a".into(), outputs: vec![Port { name: "x".into() }, Port { name: "y".into() }] },
        Node { id: "b".into(), outputs: vec![Port { name: "z".into() }] },
    ]
}

#[test]
fn nested_capture_through_reference() {
    let nodes = nodes();
    let m: HashMap<(String, String), usize> = nodes
        .iter()
        .flat_map(|node| node.outputs.iter().map(|port| { clobber(); ((node.id.clone(), port.name.clone()), port.name.len()) }))
        .collect();
    let mut k: Vec<_> = m.keys().cloned().collect();
    k.sort();
    assert_eq!(k, [("a".to_string(), "x".to_string()), ("a".to_string(), "y".to_string()), ("b".to_string(), "z".to_string())]);
    let fs: Vec<_> = nodes.iter().map(|node| move || node.id.len()).collect();
    assert_eq!(fs.iter().map(|g| g()).sum::<usize>(), 2);
    // a non-move inner closure returned from the outer one, reading through the reference
    let gs: Vec<Box<dyn Fn() -> usize + '_>> = nodes.iter().map(|node| Box::new(|| { clobber(); node.outputs.len() }) as Box<dyn Fn() -> usize>).collect();
    assert_eq!(gs.iter().map(|g| g()).collect::<Vec<_>>(), [2, 1]);
}

#[test]
fn reference_captures_that_stay_places() {
    let (a, b) = (1, 2);
    // the captured local itself is reassigned: a capture by mutable reference
    let mut r = &a;
    let mut set = || r = &b;
    set();
    assert_eq!(*r, 2);
    // the address of the captured local: still the same reference value
    let s = String::from("hi");
    let rs = &s;
    let f = || { let rr: &&String = &rs; rr.len() };
    assert_eq!(f(), 2);
    // through a &mut: unchanged
    let mut v = vec![1];
    let vm = &mut v;
    let mut push = || vm.push(2);
    push();
    assert_eq!(v, [1, 2]);
}

struct Recent { q: Vec<String> }
impl Recent {
    fn queries(&self) -> &[String] { &self.q }
}
struct Recalls { lanes: Vec<(String, Recent)> }
impl Recalls {
    fn lane(&self, name: &str) -> &[String] {
        self.lanes.iter().find(|(held, _)| held == name).map_or(&[], |(_, list)| list.queries())
    }
}

#[test]
fn map_or_default_unsizes_to_the_expected_slice() {
    let r = Recalls { lanes: vec![("a".into(), Recent { q: vec!["x".into()] })] };
    assert_eq!(r.lane("a"), ["x".to_string()]);
    assert!(r.lane("b").is_empty());
}

enum Track { One, Many(usize) }
fn count(t: &Track) -> usize {
    match t {
        Track::One => 1,
        Track::Many(n) => *n,
    }
}

#[test]
fn try_fold_over_a_fn_item_map() {
    let v = vec![Track::One, Track::Many(3)];
    assert_eq!(v.iter().map(count).try_fold(0usize, usize::checked_add).unwrap_or(99), 4);
    let big = vec![Track::Many(usize::MAX), Track::One];
    assert_eq!(big.iter().map(count).try_fold(0usize, usize::checked_add), None);
}

#[derive(Debug, PartialEq)]
enum ClientError { Io { op: &'static str, kind: std::io::ErrorKind } }
#[derive(Debug, PartialEq)]
enum Attempt { Client(ClientError), Stale }
impl From<ClientError> for Attempt {
    fn from(e: ClientError) -> Self { Attempt::Client(e) }
}
fn io_err(op: &'static str) -> impl Fn(std::io::Error) -> ClientError {
    move |e| ClientError::Io { op, kind: e.kind() }
}
fn open_len(p: &str) -> Result<u64, Attempt> {
    let f = std::fs::File::open(p).map_err(io_err("open"))?;
    let n = f.metadata().map_err(io_err("meta"))?.len();
    if n == 12345 { return Err(Attempt::Stale); }
    Ok(n)
}

#[test]
fn map_err_with_an_opaque_fn() {
    assert_eq!(open_len("/nonexistent/x"), Err(Attempt::Client(ClientError::Io { op: "open", kind: std::io::ErrorKind::NotFound })));
}

macro_rules! asset_ref {
    ($what:literal) => { concat!(r#"{"d": ""#, $what, r#""}"#) };
}
macro_rules! no_args { () => { "{}" }; }
macro_rules! nested { ($x:literal) => { concat!("<", asset_ref!($x), ">") }; }
pub static TOOLS: &[&str] = &[
    concat!(r#"{"edit": "#, asset_ref!("the edit"), "}"),
    concat!("a", no_args!(), "b", 7, true),
    concat!("n", nested!("q")),
];

#[test]
fn concat_with_macro_rules_arguments() {
    assert_eq!(TOOLS, [r#"{"edit": {"d": "the edit"}}"#, "a{}b7true", r#"n<{"d": "q"}>"#]);
}

mod worker {
    use std::marker::PhantomData;
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
    pub struct TaskHandle<T>(PhantomData<T>);
    impl<T> TaskHandle<T> {
        pub fn detach(self) {}
    }
    pub struct Spawner;
    impl Spawner {
        pub fn spawn_worker<F, T>(&self, f: F) -> Result<TaskHandle<T>, String>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            std::thread::spawn(move || {
                let _ = f();
            });
            Ok(TaskHandle(PhantomData))
        }
    }
    pub struct Worker<J, D> {
        pub jobs: Option<SyncSender<J>>,
        pub done: Receiver<D>,
    }
    impl<J: Send + 'static, D: Send + 'static> Worker<J, D> {
        pub fn new(sp: &Spawner, name: &'static str, mut run: impl FnMut(J) -> D + Send + 'static) -> Result<Self, String> {
            let (job_tx, job_rx) = sync_channel::<J>(4);
            let (done_tx, done) = sync_channel::<D>(4);
            let handle = sp
                .spawn_worker(move || {
                    while let Ok(job) = job_rx.recv() {
                        if done_tx.send(run(job)).is_err() {
                            break;
                        }
                    }
                })
                .map_err(|e| format!("{name}: {e:?}"))?;
            handle.detach();
            Ok(Self { jobs: Some(job_tx), done })
        }
    }
    pub struct SlowJob(pub u32);
    pub struct SlowDone(pub u32);
    pub struct Client(u8);
    fn run_slow(client: &mut Option<Client>, job: (u64, SlowJob)) -> SlowDone {
        client.get_or_insert(Client(1));
        SlowDone(job.1 .0 + job.0 as u32)
    }
    pub struct Host {
        pub slow: Option<Worker<(u64, SlowJob), SlowDone>>,
        pub runs: Option<Worker<(u64, SlowJob), SlowDone>>,
        pub error: Option<String>,
    }
    impl Host {
        pub fn start(&mut self, sp: &Spawner) -> Result<(), String> {
            for (slot, name) in [(&mut self.slow, "a"), (&mut self.runs, "b")] {
                let mut client = None;
                match Worker::new(sp, name, move |job| run_slow(&mut client, job)) {
                    Ok(worker) => *slot = Some(worker),
                    Err(error) => self.error = Some(error),
                }
            }
            let spawn = move |name: &'static str, body: Box<dyn FnOnce() + Send>| -> Result<(), String> {
                sp.spawn_worker(body).map(|h| h.detach()).map_err(|e| format!("{name}: {e:?}"))
            };
            spawn("x", Box::new(|| {}))
        }
    }
}

#[test]
fn spawn_worker_with_a_boxed_fn_once() {
    use worker::*;
    let mut h = Host { slow: None, runs: None, error: None };
    assert_eq!(h.start(&Spawner), Ok(()));
    assert!(h.error.is_none());
    let w = h.runs.as_ref().unwrap();
    w.jobs.as_ref().unwrap().send((2, SlowJob(3))).unwrap();
    assert_eq!(w.done.recv().unwrap().0, 5);
}
