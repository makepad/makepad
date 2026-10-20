//! MAKEPAD_RUN_PROFILE load timeline. Times share a process epoch; nested events are inclusive.
//! Thread ids distinguish pools (including the proc-macro world's own parse pool).
//! Buffer per thread: stderr contention must not become the measured critical path.
use std::sync::Arc;
use relax_session::sync::Mutex;
use std::time::Instant;

pub struct Span {
    start: Instant,
    phase: &'static str,
    subject: String,
    file_cost: Option<(Arc<crate::exec_profile::File>, bool, u64)>,
}
struct Event { start_us: u128, dur_us: u128, phase: &'static str, subject: String }
struct Buffer { thread: std::thread::ThreadId, worker: String, events: Vec<Event> }
static BUFFERS: Mutex<Vec<Arc<Mutex<Buffer>>>> = Mutex::new(Vec::new());

/// Registers the calling thread's new generation of timeline events for flush().
struct LocalBuffer(Arc<Mutex<Buffer>>);
impl Default for LocalBuffer {
    fn default() -> Self {
        let thread = std::thread::current();
        let buffer = Arc::new(Mutex::new(Buffer { thread: thread.id(), worker: thread.name().unwrap_or("unnamed").into(), events: Vec::new() }));
        BUFFERS.lock().unwrap().push(buffer.clone());
        Self(buffer)
    }
}
crate::gen_local! {
    // Exclusive CPU among the selected file spans: nested parse/lower work
    // belongs to its own file, not also to the enclosing parse.
    static FILE_CHILD_CPU: std::cell::RefCell<Vec<u64>>;
    static BUFFER: LocalBuffer;
}

pub fn span(enabled: bool, phase: &'static str, subject: &str) -> Option<Span> {
    enabled.then(|| {
        epoch();
        let file_cost = match phase {
            "lower" | "parse-rs" | "parse-lowered" => Some((crate::exec_profile::file(subject), phase == "lower", relax_backend::cpu_ns())),
            _ => None,
        };
        if file_cost.is_some() { FILE_CHILD_CPU.with(|s| s.borrow_mut().push(0)); }
        Span { start: Instant::now(), phase, subject: subject.into(), file_cost }
    })
}

fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

impl Drop for Span {
    fn drop(&mut self) {
        let end = Instant::now();
        if let Some((file, lower, start)) = &self.file_cost {
            let inclusive = relax_backend::cpu_ns().saturating_sub(*start);
            let own = FILE_CHILD_CPU.with(|s| {
                let mut s = s.borrow_mut();
                let children = s.pop().unwrap_or(0);
                if let Some(parent) = s.last_mut() { *parent += inclusive; }
                inclusive.saturating_sub(children)
            });
            if !self.phase.is_empty() { file.cost(*lower, own); }
        }
        if self.phase.is_empty() { return; }
        BUFFER.with(|buffer| buffer.0.lock().unwrap().events.push(Event {
            start_us: self.start.duration_since(epoch()).as_micros(),
            dur_us: end.duration_since(self.start).as_micros(), phase: self.phase,
            subject: std::mem::take(&mut self.subject),
        }));
    }
}

/// Emit outside the measured load phase, after the worker pool has joined.
pub fn flush() {
    use std::io::Write;
    let buffers = BUFFERS.lock().unwrap().clone();
    let mut out = std::io::BufWriter::new(std::io::stderr().lock());
    for buffer in buffers {
        let mut buffer = buffer.lock().unwrap();
        let thread = buffer.thread;
        let worker = buffer.worker.clone();
        for e in buffer.events.drain(..) {
            let _ = writeln!(out, "load-timeline: start_us={} dur_us={} thread={:?} worker={:?} phase={} subject={:?}",
                e.start_us, e.dur_us, thread, worker, e.phase, e.subject);
        }
    }
}

impl Span {
    pub fn cancel(mut self) { self.phase = ""; }
}

/// A post-present writer may still have live compilation threads around it.
/// Drain only its own events, releasing the buffer before writing stderr.
pub fn flush_current() {
    use std::io::Write;
    let (thread, worker, events) = BUFFER.with(|buffer| {
        let mut buffer = buffer.0.lock().unwrap();
        (buffer.thread, buffer.worker.clone(), std::mem::take(&mut buffer.events))
    });
    let mut out = std::io::BufWriter::new(std::io::stderr().lock());
    for e in events {
        let _ = writeln!(out, "load-timeline: start_us={} dur_us={} thread={:?} worker={:?} phase={} subject={:?}",
            e.start_us, e.dur_us, thread, worker, e.phase, e.subject);
    }
}
