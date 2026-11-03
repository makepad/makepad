//! The git binary, for what devhub does not do through makepad-git yet. Every spawn goes through
//! [`command`], which counts it per call site: a loop must not spawn any (each exec costs a
//! process start and, on macOS, a code assessment of the binary). `devhub health` shows the
//! busiest sites.
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static TOTAL: AtomicU64 = AtomicU64::new(0);
static SITES: Mutex<Option<HashMap<(&'static str, u32), u64>>> = Mutex::new(None);
/// (the wall minute counted now, its count, the previous minute's count)
static MINUTE: Mutex<(u64, u64, u64)> = Mutex::new((0, 0, 0));

thread_local! {
    static HERE: Cell<u64> = const { Cell::new(0) };
}

/// `git`, counted (in its own process group, like every child devhub starts).
#[track_caller]
pub fn command() -> std::process::Command {
    let at = std::panic::Location::caller();
    TOTAL.fetch_add(1, Ordering::Relaxed);
    HERE.with(|n| n.set(n.get() + 1));
    *SITES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry((at.file(), at.line())).or_insert(0) += 1;
    let now = crate::proto::wall_ms() / 60_000;
    let mut m = MINUTE.lock().unwrap_or_else(|e| e.into_inner());
    if m.0 != now {
        *m = (now, 0, if m.0 + 1 == now { m.1 } else { 0 });
    }
    m.1 += 1;
    makepad_remote::group::command("git")
}

/// git processes this process started.
pub fn total() -> u64 {
    TOTAL.load(Ordering::Relaxed)
}

/// git processes started on this thread (a test measures one call with it).
pub fn on_this_thread() -> u64 {
    HERE.with(Cell::get)
}

/// The call sites that started the most git processes: (file:line, count), busiest first.
pub fn top(n: usize) -> Vec<(String, u64)> {
    let sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<(String, u64)> = sites.iter().flatten().map(|((f, l), c)| (format!("{}:{l}", f.rsplit_once("src/").map_or(*f, |x| x.1)), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// git processes started in the last whole wall minute.
pub fn last_minute() -> u64 {
    let now = crate::proto::wall_ms() / 60_000;
    let m = MINUTE.lock().unwrap_or_else(|e| e.into_inner());
    if m.0 == now { m.2 } else if m.0 + 1 == now { m.1 } else { 0 }
}

/// The `gitexecs` control answer: one JSON line.
pub fn answer() -> String {
    let sites: Vec<String> = top(12).into_iter().map(|(s, n)| format!("{{\"site\":{},\"n\":{n}}}", makepad_remote::json_str(&s))).collect();
    format!("{{\"total\":{},\"last_minute\":{},\"sites\":[{}]}}", total(), last_minute(), sites.join(","))
}
