//! Input record / replay (`input record path=<file>`, `input stop`, `input replay path=<file>`):
//! the person's native pointer, wheel, key and text input written as the remote commands that
//! inject the same input, one per line with its time and drawn-frame number, and played
//! back through those commands -- so a before/after experiment (makepad inspect `ab`, a codegen
//! change, a fix) runs on the same input. A recording is plain text an agent can read, edit and
//! write by hand:
//!
//! ```text
//! # makepad input v1: <ms since record start> <drawn frame since start> <command>
//! 0.0 0 mouse k=move x=412 y=230 w=0
//! 16.7 1 mouse k=down x=412 y=230 b=0 w=0
//! ```
//!
//! Replay paces by time (`sync=time`, default, scaled by `speed=`) or by drawn frames
//! (`sync=frame`: each event waits until the app drew as many frames as when it was
//! recorded, the same input on the same frame however fast the machine is). Injected input is
//! never recorded (a replay does not record itself).

use super::bridge::{err, json_str, resolve, Answer, Params};
use crate::event::Event;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

struct Rec {
    file: std::io::BufWriter<std::fs::File>,
    path: String,
    t0: Instant,
    f0: u64,
    n: u64,
}

static REC: Mutex<Option<Rec>> = Mutex::new(None);
static RECORDING: AtomicBool = AtomicBool::new(false);
/// replays running (a new replay or `input stop` ends the older ones)
static REPLAY_GEN: AtomicU64 = AtomicU64::new(0);
static REPLAY_LEFT: AtomicU64 = AtomicU64::new(0);

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.,:/".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{:02X}", b));
        }
    }
    o
}

fn unesc(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut k = 0;
    while k < b.len() {
        if b[k] == b'%' && k + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[k + 1..k + 3], 16) {
                o.push(v);
                k += 3;
                continue;
            }
        }
        o.push(b[k]);
        k += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

fn mods(m: &crate::event::KeyModifiers) -> String {
    let mut s = String::new();
    if m.shift {
        s.push_str(" shift=1");
    }
    if m.control {
        s.push_str(" ctrl=1");
    }
    if m.alt {
        s.push_str(" alt=1");
    }
    if m.logo {
        s.push_str(" cmd=1");
    }
    s
}

fn n(v: f64) -> String {
    super::bridge::num(v)
}

/// The command that injects the same input as native `event` (None: not recorded).
fn command_of(event: &Event) -> Option<String> {
    Some(match event {
        Event::MouseMove(e) => format!("mouse k=move x={} y={} w={}{}", n(e.abs.x), n(e.abs.y), e.window_id.0, mods(&e.modifiers)),
        Event::MouseDown(e) => format!("mouse k=down x={} y={} b={} w={}{}", n(e.abs.x), n(e.abs.y), e.button.bits().trailing_zeros(), e.window_id.0, mods(&e.modifiers)),
        Event::MouseUp(e) => format!("mouse k=up x={} y={} b={} w={}{}", n(e.abs.x), n(e.abs.y), e.button.bits().trailing_zeros(), e.window_id.0, mods(&e.modifiers)),
        Event::Scroll(e) if e.is_mouse => format!("mouse k=scroll x={} y={} dx={} dy={} w={}{}", n(e.abs.x), n(e.abs.y), n(e.scroll.x), n(e.scroll.y), e.window_id.0, mods(&e.modifiers)),
        Event::KeyDown(e) if !e.is_repeat => format!("key k=down c={:?}{}", e.key_code, mods(&e.modifiers)),
        Event::KeyUp(e) => format!("key k=up c={:?}{}", e.key_code, mods(&e.modifiers)),
        Event::TextInput(e) if !e.replace_last && e.composition.is_none() => format!("text t={}", esc(&e.input)),
        _ => return None,
    })
}

/// Input at the dispatch boundary for the flight recorder (makepad run): native and injected
/// alike (a replay reproduces what the app saw), at its drawn frame.
pub(crate) fn flight(event: &Event, injected: bool) {
    // the window's size and scale: a replay opens its window so (MAKEPAD_WINDOW)
    if let Event::WindowGeomChange(e) = event {
        let g = &e.new_geom;
        crate::makepad_error_log::flight::mark_ab("window.geom", &format!("{}x{}@{}", n(g.inner_size.x), n(g.inner_size.y), n(g.dpi_factor)), e.window_id.0 as u64, 0);
        return;
    }
    let Some(cmd) = command_of(event) else { return };
    crate::makepad_error_log::flight::input(crate::frame_trace::drawn_frames(), injected, &cmd);
}

/// Native input at the dispatch boundary (remote_activity::note_user_event, injected input
/// excluded).
pub(crate) fn note(event: &Event) {
    if !RECORDING.load(Ordering::Relaxed) {
        return;
    }
    let Some(cmd) = command_of(event) else { return };
    let mut g = REC.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = g.as_mut() {
        let ms = r.t0.elapsed().as_secs_f64() * 1000.0;
        let f = crate::frame_trace::drawn_frames().saturating_sub(r.f0);
        let _ = writeln!(r.file, "{:.1} {} {}", ms, f, cmd);
        r.n += 1;
    }
}

fn ok(s: String) -> Answer {
    Answer::Json(s)
}

/// `input record path=<file>`
pub(crate) fn record(p: &Params) -> Answer {
    let Some(path) = p.get(&["path", "p"]) else {
        return err("input record path=<file>");
    };
    let f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => return err(&format!("cannot write {path}: {e}")),
    };
    let mut file = std::io::BufWriter::new(f);
    let _ = writeln!(file, "# makepad input v1: <ms since record start> <drawn frame since start> <command>");
    *REC.lock().unwrap_or_else(|e| e.into_inner()) = Some(Rec { file, path: path.to_string(), t0: Instant::now(), f0: crate::frame_trace::drawn_frames(), n: 0 });
    RECORDING.store(true, Ordering::Relaxed);
    ok(format!("{{\"input\":\"recording\",\"path\":{},\"more\":[\"input stop\",\"input replay path={}\"]}}", json_str(path), esc(path)))
}

/// `input stop`: ends a recording and any replay.
pub(crate) fn stop() -> Answer {
    RECORDING.store(false, Ordering::Relaxed);
    REPLAY_GEN.fetch_add(1, Ordering::Relaxed);
    let r = REC.lock().unwrap_or_else(|e| e.into_inner()).take();
    match r {
        Some(mut r) => {
            let _ = r.file.flush();
            ok(format!("{{\"input\":\"stopped\",\"path\":{},\"events\":{},\"ms\":{:.0}}}", json_str(&r.path), r.n, r.t0.elapsed().as_secs_f64() * 1000.0))
        }
        None => ok("{\"input\":\"stopped\",\"recording\":false}".into()),
    }
}

/// `input status`
pub(crate) fn status() -> Answer {
    let g = REC.lock().unwrap_or_else(|e| e.into_inner());
    ok(format!(
        "{{\"recording\":{},\"events\":{},\"replay_events_left\":{}}}",
        g.is_some(),
        g.as_ref().map_or(0, |r| r.n),
        REPLAY_LEFT.load(Ordering::Relaxed)
    ))
}

/// MAKEPAD_INPUT_REPLAY=<file> (a flight recorder replay, `makepad inspect file=<x.mkpf> rec
/// replay`): the app plays the input back from its start, each event at the drawn frame it
/// was recorded at (frames counted from the app's start).
pub(crate) fn replay_at_start() {
    let Some(path) = std::env::var("MAKEPAD_INPUT_REPLAY").ok().filter(|p| !p.is_empty()) else { return };
    let p = Params(vec![("path".into(), path), ("sync".into(), "frame".into()), ("abs".into(), "1".into())]);
    if let Answer::Json(s) = replay(&p) {
        eprintln!("[makepad-remote] input replay: {s}");
    }
}

/// `input replay path=<file> [speed=1] [sync=time|frame] [abs=1]`: plays a recording back on its
/// own thread through the injecting commands; answers at once. `abs=1`: frames count from the
/// app's start, not from the replay's (a replay that starts with the app).
pub(crate) fn replay(p: &Params) -> Answer {
    let Some(path) = p.get(&["path", "p"]) else {
        return err("input replay path=<file> [speed=1] [sync=time|frame] [abs=1]");
    };
    let abs = p.get(&["abs"]) == Some("1");
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return err(&format!("cannot read {path}: {e}")),
    };
    let speed = p.f64(&["speed"], 1.0).max(0.01);
    let by_frame = p.get(&["sync"]) == Some("frame");
    let mut events: Vec<(f64, u64, Vec<String>)> = Vec::new();
    for (k, l) in text.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let mut w = l.split_whitespace();
        let (Some(t), Some(f)) = (w.next().and_then(|x| x.parse::<f64>().ok()), w.next().and_then(|x| x.parse::<u64>().ok())) else {
            return err(&format!("{path}:{}: want `<ms> <frame> <command ...>`", k + 1));
        };
        let words: Vec<String> = w.map(|x| x.to_string()).collect();
        if words.is_empty() {
            return err(&format!("{path}:{}: no command", k + 1));
        }
        if let Err(e) = resolve(&words) {
            return err(&format!("{path}:{}: {e}", k + 1));
        }
        events.push((t, f, words));
    }
    let gen = REPLAY_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let total = events.len();
    let span_ms = events.last().map_or(0.0, |e| e.0) / speed;
    REPLAY_LEFT.store(total as u64, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("remote-input-replay".into()).spawn(move || {
        let t0 = Instant::now();
        let f0 = if abs { 0 } else { crate::frame_trace::drawn_frames() };
        for (t, f, words) in events {
            if REPLAY_GEN.load(Ordering::Relaxed) != gen {
                break;
            }
            if by_frame {
                // (a replay from the start waits for the app's first frames: a cold start)
                let deadline = Instant::now() + std::time::Duration::from_secs(if abs { 300 } else { 10 });
                while crate::frame_trace::drawn_frames().saturating_sub(f0) < f && Instant::now() < deadline && REPLAY_GEN.load(Ordering::Relaxed) == gen {
                    std::thread::sleep(std::time::Duration::from_micros(500));
                }
            } else {
                let at = std::time::Duration::from_secs_f64(t / speed / 1000.0);
                let el = t0.elapsed();
                if at > el {
                    std::thread::sleep(at - el);
                }
            }
            let Ok(r) = resolve(&words) else { continue };
            let params = Params(words[r.used..].iter().map(|x| match x.split_once('=') {
                Some((k, v)) => (k.to_string(), unesc(v)),
                None => (x.to_string(), "1".to_string()),
            }).collect());
            let _ = super::imp::command(&r, &params);
            REPLAY_LEFT.fetch_sub(1, Ordering::Relaxed);
        }
        if REPLAY_GEN.load(Ordering::Relaxed) == gen {
            REPLAY_LEFT.store(0, Ordering::Relaxed);
            crate::makepad_error_log::flight::mark("replay.done", &format!("input replay of {} events done at frame {}", total, crate::frame_trace::drawn_frames()));
        }
    });
    if spawned.is_err() {
        return err("cannot start the replay thread");
    }
    ok(format!("{{\"input\":\"replaying\",\"events\":{},\"ms\":{:.0},\"sync\":\"{}\",\"more\":[\"input status\",\"input stop\"]}}", total, span_ms, if by_frame { "frame" } else { "time" }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_round_trip() {
        for s in ["hello world", "a=b c%d", "émoji ✓", "x"] {
            assert_eq!(super::unesc(&super::esc(s)), s);
        }
    }
}
