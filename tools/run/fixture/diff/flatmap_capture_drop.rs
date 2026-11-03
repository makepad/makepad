// A FlatMap whose inner Map's `move` closure owns a captured value with heap fields and clones it
// per item (the shape of fab_arms' planner, whose double free under the JIT came from a move rustc
// rejects, E0507, now refused at lowering: lower.rs illegal_move_tests). Every Task made is
// dropped exactly once: LIVE counts them, and a drop of a dead one is reported.
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering::SeqCst};

static LIVE: AtomicIsize = AtomicIsize::new(0);
static BAD: AtomicUsize = AtomicUsize::new(0);

macro_rules! d { ($($t:tt)*) => { println!("D {}", format!($($t)*)) } }

#[derive(Clone, Debug, PartialEq)]
enum Kind { Scan, Pick }

#[derive(Clone, Debug)]
struct Way { p: [f64; 3], x: Option<f64> }

#[derive(Debug)]
struct Task { path: Vec<Way>, kind: Kind, name: String, alive: u64 }

const ALIVE: u64 = 0x5eed_7a5c;

impl Task {
    fn new(n: usize, kind: Kind) -> Task {
        LIVE.fetch_add(1, SeqCst);
        Task { path: (0..n).map(|i| Way { p: [i as f64, 1.0, 2.0], x: Some(i as f64 * 0.5) }).collect(), kind, name: format!("task{n}"), alive: ALIVE }
    }
}

impl Clone for Task {
    fn clone(&self) -> Task {
        if self.alive != ALIVE { BAD.fetch_add(1, SeqCst); }
        LIVE.fetch_add(1, SeqCst);
        Task { path: self.path.clone(), kind: self.kind.clone(), name: self.name.clone(), alive: ALIVE }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if self.alive != ALIVE { BAD.fetch_add(1, SeqCst); }
        self.alive = 0;
        LIVE.fetch_sub(1, SeqCst);
    }
}

type Q = [f64; 6];

fn ik(task: &Task, s: &Q) -> Result<Q, ()> {
    if s[0] < -0.5 { return Err(()); }
    let mut q = *s;
    q[1] += task.path[0].p[0];
    Ok(q)
}

fn trace(path: &[Way], q0: &Q) -> (u32, Option<[f64; 3]>, Q) {
    let misses = path.iter().filter(|w| w.x.is_some_and(|x| x > q0[0])).count() as u32;
    (misses, path.first().map(|w| w.p), *q0)
}

// the planner's shape: options by value into flat_map, the inner Map's move closure owns `task`
fn plan(task: Task, seeds: [Option<Q>; 3], from: Q) -> Option<(u32, f64, Q, Option<[f64; 3]>, Task, Q)> {
    let mut options = vec![task];
    if options[0].kind == Kind::Scan {
        let mut flipped = options[0].clone();
        for w in flipped.path.iter_mut() { w.x = w.x.map(|x| -x); }
        options.push(flipped);
    }
    let traced = options.into_iter().flat_map(|task| {
        let starts: Vec<Q> = seeds.iter().flatten().filter_map(|s| ik(&task, s).ok()).collect();
        starts.into_iter().map(move |q0| {
            let (misses, at, end) = trace(&task.path, &q0);
            let travel: f64 = (0..6).map(|j| (q0[j] - from[j]).abs()).sum();
            (misses, travel, q0, at, task.clone(), end)
        })
    });
    traced.min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
}

// stops in the middle of an inner iterator: the FlatMap drops a live front Map (and its task)
fn first_far(tasks: Vec<Task>, seeds: Vec<Q>) -> Option<(usize, String)> {
    tasks.into_iter().flat_map(|task| {
        let seeds = seeds.clone();
        seeds.into_iter().enumerate().map(move |(i, q)| (i, q[0] + task.path.len() as f64, task.name.clone()))
    }).find(|x| x.1 > 3.5).map(|x| (x.0, x.2))
}

// both ends: next_back takes the back inner iterator, next the front one, then the rest is dropped
fn both_ends(tasks: Vec<Task>) -> (Option<String>, Option<String>) {
    let mut it = tasks.into_iter().flat_map(|task| (0..2).map(move |k| format!("{}#{k}", task.name)));
    let front = it.next();
    let back = it.next_back();
    (front, back)
}

// flatten of Options holding tasks, and a fused flat_map exhausted then polled again
fn flatten_and_fuse(tasks: Vec<Option<Task>>) -> (usize, Option<usize>) {
    let n = tasks.into_iter().flatten().map(|t| t.path.len()).sum();
    let mut f = vec![Task::new(1, Kind::Pick)].into_iter().flat_map(|t| std::iter::once(t.path.len()).map(move |k| k + t.name.len())).fuse();
    let a = f.next();
    let _ = f.next();
    (n, a.or(f.next()))
}

fn check(label: &str) {
    d!("{label}: live={} bad={}", LIVE.load(SeqCst), BAD.load(SeqCst));
}

#[test]
fn flatmap_capture_drop() {
    let seeds = [Some([0.0; 6]), None, Some([2.0, 0.0, 0.0, 0.0, 0.0, 1.0])];
    for (n, kind) in [(3, Kind::Scan), (1, Kind::Pick), (5, Kind::Scan)] {
        let best = plan(Task::new(n, kind), seeds, [0.5; 6]);
        let r = best.as_ref().map(|b| (b.0, b.1, b.4.name.clone(), b.4.path.len(), b.4.path[0].x));
        d!("plan {n}: {r:?}");
        drop(best);
        check("after plan");
    }
    // no start reaches: every inner iterator is empty
    let none = plan(Task::new(2, Kind::Scan), [Some([-1.0; 6]), None, None], [0.0; 6]);
    d!("plan unreachable: {}", none.is_none());
    check("after unreachable");
    let r = first_far(vec![Task::new(1, Kind::Pick), Task::new(3, Kind::Scan), Task::new(4, Kind::Pick)], vec![[0.0; 6], [1.0; 6], [2.0; 6]]);
    d!("first_far: {r:?}");
    check("after first_far");
    let r = both_ends(vec![Task::new(1, Kind::Pick), Task::new(2, Kind::Pick), Task::new(3, Kind::Pick)]);
    d!("both_ends: {r:?}");
    check("after both_ends");
    let r = flatten_and_fuse(vec![Some(Task::new(2, Kind::Pick)), None, Some(Task::new(3, Kind::Scan))]);
    d!("flatten_and_fuse: {r:?}");
    check("end");
    assert_eq!((LIVE.load(SeqCst), BAD.load(SeqCst)), (0, 0));
}
