//! The `prof` service of a native (rustc-built) app with its remote endpoint on (MAKEPAD_REMOTE=1 /
//! MAKEPAD_INSPECT=1): the profiler stream (makepad-prof-stream, the format `makepad run` apps
//! serve from relax inspect) with the UI thread's phases as its main lane. While a viewer is
//! attached, each phase's begin and end go to one log (the UI thread is its only writer; a viewer
//! thread drains it every 16 ms); with none attached a phase costs one relaxed load.

use makepad_prof_stream as ps;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

static VIEWERS: AtomicUsize = AtomicUsize::new(0);

#[inline]
pub fn on() -> bool {
    VIEWERS.load(Relaxed) > 0
}

/// (raw monotonic ns, span kind, phase name)
struct Log {
    base: u64,
    v: std::collections::VecDeque<(u64, u8, &'static str)>,
}

const LOG_MAX: usize = 1 << 16;

static LOG: Mutex<Log> = Mutex::new(Log { base: 0, v: std::collections::VecDeque::new() });

fn push(kind: u8, name: &'static str) {
    let t = ps::mono_ns();
    let mut l = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if l.v.len() >= LOG_MAX {
        l.v.pop_front();
        l.base += 1;
    }
    l.v.push_back((t, kind, name));
}

/// A UI phase begins (thread/ui_hang.rs).
pub fn begin(name: &'static str) {
    push(ps::span::BEGIN, name);
}

/// A UI phase ends.
pub fn end(name: &'static str) {
    push(ps::span::END, name);
}

pub fn service(app: String) -> makepad_remote::endpoint::Factory {
    use makepad_remote::conn::{Event, CLOSE_REFUSED};
    std::sync::Arc::new(move |_conn, open| {
        let app = app.clone();
        std::thread::Builder::new()
            .name("makepad-remote-prof".into())
            .spawn(move || {
                let ch = open.channel;
                VIEWERS.fetch_add(1, Relaxed);
                serve(&app, &|msg| ch.send(msg).is_ok(), &|| matches!(ch.try_recv(), Ok(Event::Closed(..)) | Err(false)));
                VIEWERS.fetch_sub(1, Relaxed);
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the prof thread: {e}")))
    })
}

fn serve(app: &str, send: &dyn Fn(Vec<u8>) -> bool, closed: &dyn Fn() -> bool) {
    const MAIN: u32 = 1;
    let mut strings = ps::Interner::default();
    let _ = strings.id("");
    let mut msg = ps::prelude().to_vec();
    let hello = ps::Hello { app: app.to_string(), host: String::new(), pid: std::process::id() as u64, build: String::new(), device: String::new(), refresh_mhz: 0, flags: 2, start_mono: 0 };
    ps::record(&mut msg, ps::kind::HELLO, &hello.payload());
    let unix_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    ps::record(&mut msg, ps::kind::CLOCK, &ps::clock_payload(ps::mono_ns(), unix_ms()));
    let main_sid = strings.id("main");
    let mut threads = Some(ps::thread_payload(MAIN, 0, 0, ps::role::MAIN, main_sid));
    let mut cursor = {
        let l = LOG.lock().unwrap_or_else(|e| e.into_inner());
        l.base + l.v.len() as u64
    };
    let mut last_clock = std::time::Instant::now();
    loop {
        if closed() {
            return;
        }
        let (recs, lost) = {
            let l = LOG.lock().unwrap_or_else(|e| e.into_inner());
            let lost = l.base.saturating_sub(cursor);
            let from = cursor.max(l.base);
            let v: Vec<(u64, u8, &'static str)> = l.v.iter().skip((from - l.base) as usize).copied().collect();
            cursor = l.base + l.v.len() as u64;
            (v, lost)
        };
        let spans: Vec<ps::SpanRec> = recs.iter().map(|(t, k, n)| ps::SpanRec { kind: *k, t: *t, name: strings.id(n), a: 0 }).collect();
        if let Some(s) = strings.take_new() {
            ps::record(&mut msg, ps::kind::STRINGS, &s);
        }
        if let Some(t) = threads.take() {
            ps::record(&mut msg, ps::kind::THREAD, &t);
        }
        if !spans.is_empty() {
            ps::record(&mut msg, ps::kind::SPANS, &ps::spans_payload(MAIN, &spans));
        }
        if lost > 0 {
            ps::record(&mut msg, ps::kind::DROP, &ps::drop_payload(MAIN, lost));
        }
        if last_clock.elapsed().as_secs() >= 10 {
            last_clock = std::time::Instant::now();
            ps::record(&mut msg, ps::kind::CLOCK, &ps::clock_payload(ps::mono_ns(), unix_ms()));
        }
        if !msg.is_empty() && !send(std::mem::take(&mut msg)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
