//! `makepad inspect <app> draws`: CPU time of each widget's draw, per frame, recorded only while
//! asked (`draws start` .. `draws`). Off, the widget draw wrappers test one relaxed atomic.
//!
//! While on, every `WidgetRef` draw entry (draw_walk, draw_walk_all, draw, draw_unscoped) times the
//! widget's draw: inclusive (with its children) and self (without the children drawn through a
//! `WidgetRef`). A widget whose draw list is reused from the last frame still counts a call, with
//! almost no time: the answer tells a panel drawn again every frame from one that was skipped.
//! Frames are told apart by `Cx::drawing_id`. The report resolves each uid to its widget path and
//! type once, at the end.

use crate::makepad_draw::*;
use crate::widget::WidgetUid;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

static ON: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Rec {
    calls: u64,
    incl_ns: u64,
    self_ns: u64,
    /// frames the widget drew in
    frames: u32,
    /// self time in the frame it last drew in, and the most of any frame
    frame_self_ns: u64,
    max_frame_self_ns: u64,
    last_frame: u64,
}

struct Recording {
    started: Instant,
    until: Instant,
    /// the drawing being recorded and its widget draw total (top-level inclusive)
    drawing: u64,
    frame_ns: Vec<u64>,
    /// open draws: uid, start, children's inclusive time
    stack: Vec<(u64, Instant, u64)>,
    recs: HashMap<u64, Rec>,
}

thread_local! {
    static REC: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

/// An open widget draw (see [`enter`]); [`exit`] closes it.
pub(crate) struct DrawTimeScope(());

/// A widget draw begins: timed when a recording runs (one relaxed load otherwise).
#[inline]
pub(crate) fn enter(cx: &Cx, uid: WidgetUid) -> Option<DrawTimeScope> {
    if !ON.load(Relaxed) {
        return None;
    }
    enter_slow(cx.drawing_id(), uid.0, Instant::now())
}

#[inline(never)]
fn enter_slow(drawing: u64, uid: u64, now: Instant) -> Option<DrawTimeScope> {
    REC.with(|r| {
        let mut r = r.borrow_mut();
        let rec = r.as_mut()?;
        if rec.drawing != drawing {
            // draws left open by a panic in the last drawing
            rec.stack.clear();
            if now >= rec.until {
                // a forgotten recording ends itself
                ON.store(false, Relaxed);
                return None;
            }
            rec.drawing = drawing;
            rec.frame_ns.push(0);
        }
        rec.stack.push((uid, now, 0));
        Some(DrawTimeScope(()))
    })
}

/// The widget draw opened by [`enter`] ends.
#[inline(never)]
pub(crate) fn exit(scope: DrawTimeScope) {
    exit_at(scope, Instant::now());
}

fn exit_at(_scope: DrawTimeScope, now: Instant) {
    REC.with(|r| {
        let mut r = r.borrow_mut();
        let Some(rec) = r.as_mut() else { return };
        let Some((uid, t0, child_ns)) = rec.stack.pop() else { return };
        let incl = now.saturating_duration_since(t0).as_nanos() as u64;
        let own = incl.saturating_sub(child_ns);
        match rec.stack.last_mut() {
            Some(parent) => parent.2 += incl,
            None => {
                if let Some(f) = rec.frame_ns.last_mut() {
                    *f += incl;
                }
            }
        }
        let drawing = rec.drawing;
        let e = rec.recs.entry(uid).or_default();
        if e.last_frame != drawing || e.frames == 0 {
            e.max_frame_self_ns = e.max_frame_self_ns.max(e.frame_self_ns);
            e.frame_self_ns = 0;
            e.frames += 1;
            e.last_frame = drawing;
        }
        e.calls += 1;
        e.incl_ns += incl;
        e.self_ns += own;
        e.frame_self_ns += own;
    });
}

fn ms(ns: f64) -> String {
    let v = ns / 1e6;
    if v >= 10.0 {
        format!("{v:.1}")
    } else if v >= 0.01 {
        format!("{v:.2}")
    } else {
        format!("{:.0}µs", ns / 1e3)
    }
}

/// `draws start [s=30]` | `draws [n=20] [by=self|incl|calls]` (the report; ends the recording) |
/// `draws stop`
fn query(cx: &mut Cx, words: &[String], kv: &[(String, String)]) -> Vec<String> {
    let get = |k: &str| kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str());
    match words.get(1).map(String::as_str) {
        Some("start") => {
            let s = get("s").and_then(|v| v.parse::<u64>().ok()).unwrap_or(30).clamp(1, 600);
            let now = Instant::now();
            REC.with(|r| {
                *r.borrow_mut() = Some(Recording {
                    started: now,
                    until: now + Duration::from_secs(s),
                    drawing: u64::MAX,
                    frame_ns: Vec::new(),
                    stack: Vec::new(),
                    recs: HashMap::new(),
                })
            });
            ON.store(true, Relaxed);
            vec![format!("{{\"draws\":\"recording\",\"max_s\":{s},\"more\":[\"draws\",\"draws stop\"]}}")]
        }
        Some("stop") => {
            ON.store(false, Relaxed);
            REC.with(|r| *r.borrow_mut() = None);
            vec!["{\"draws\":\"stopped\"}".into()]
        }
        None => report(cx, get("n").and_then(|v| v.parse().ok()).unwrap_or(20), get("by").unwrap_or("self")),
        Some(other) => vec![format!("{{\"err\":\"unknown `draws {other}`: draws start [s=30] | draws [n=20] [by=self|incl|calls] | draws stop\"}}")],
    }
}

fn report(cx: &mut Cx, n: usize, by: &str) -> Vec<String> {
    ON.store(false, Relaxed);
    let Some(mut rec) = REC.with(|r| r.borrow_mut().take()) else {
        return vec!["{\"err\":\"no recording: draws start, then do the interaction, then draws\"}".into()];
    };
    let frames = rec.frame_ns.len();
    if frames == 0 {
        return vec!["{\"err\":\"no widget drew since draws start (nothing redrew)\"}".into()];
    }
    let secs = rec.started.elapsed().as_secs_f64();
    let mut per_frame = rec.frame_ns.clone();
    per_frame.sort_unstable();
    let at = |q: f64| per_frame[((per_frame.len() - 1) as f64 * q).round() as usize] as f64;
    let mut out = vec![format!(
        "draws: {frames} frames over {secs:.1} s; widget draw per frame: median {} ms, p95 {} ms, max {} ms; {} widgets drew",
        ms(at(0.5)),
        ms(at(0.95)),
        ms(at(1.0)),
        rec.recs.len()
    )];
    for e in rec.recs.values_mut() {
        e.max_frame_self_ns = e.max_frame_self_ns.max(e.frame_self_ns);
    }
    let mut rows: Vec<(u64, &Rec)> = rec.recs.iter().map(|(u, e)| (*u, e)).collect();
    match by {
        "incl" => rows.sort_by(|a, b| b.1.incl_ns.cmp(&a.1.incl_ns)),
        "calls" => rows.sort_by(|a, b| b.1.calls.cmp(&a.1.calls)),
        _ => rows.sort_by(|a, b| b.1.self_ns.cmp(&a.1.self_ns)),
    }
    let shown: Vec<(u64, &Rec)> = rows.iter().take(n).copied().collect();
    let uids: Vec<u64> = shown.iter().map(|x| x.0).collect();
    let names: HashMap<u64, (String, String)> = match cx.widget_uid_paths_callback {
        Some(f) => f(cx, &uids).into_iter().map(|(u, p, t)| (u, (p, t))).collect(),
        None => HashMap::new(),
    };
    let f = frames as f64;
    out.push(String::new());
    out.push("| widget | type | frames drawn | calls/frame | self ms/frame | incl ms/frame | worst frame self ms |".into());
    out.push("|---|---|---:|---:|---:|---:|---:|".into());
    for (uid, e) in &shown {
        let (path, ty) = names.get(uid).cloned().unwrap_or_else(|| (format!("uid {uid}"), "-".into()));
        out.push(format!(
            "| {} | {} | {}/{} | {:.1} | {} | {} | {} |",
            if path.is_empty() { "(window)".into() } else { path },
            ty,
            e.frames,
            frames,
            e.calls as f64 / f,
            ms(e.self_ns as f64 / f),
            ms(e.incl_ns as f64 / f),
            ms(e.max_frame_self_ns as f64)
        ));
    }
    let rest: u64 = rows.iter().skip(n).map(|x| x.1.self_ns).sum();
    if rows.len() > n {
        out.push(format!("+{} more widgets: {} ms/frame self in all", rows.len() - n, ms(rest as f64 / f)));
    }
    out.push("more: draws start · draws by=incl|calls n=40 · prof view=tree focus=<Type>::draw_walk".into());
    out
}

/// Registers the `draws` inspect command (once per process; later calls replace it).
pub(crate) fn register() {
    crate::makepad_platform::remote::register_app_query(
        "draws",
        "start [s=30] | [n=20] [by=self|incl|calls] | stop",
        "CPU time of each widget's draw per frame since `draws start`: frames drawn, calls, self and inclusive ms per frame, worst frame",
        query,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn a_recording_splits_self_from_children_per_frame() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        query(&mut cx, &words("draws start"), &[]);
        // two frames on a clock of their own: the parent (uid 1) draws for 1 ms, its child (uid 2)
        // for 1 ms, the parent 1 ms more; in the second frame the child draws twice
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut clock = 0;
        for (drawing, children) in [(10, 1), (11, 2)] {
            let parent = enter_slow(drawing, 1, at(clock)).unwrap();
            clock += 1;
            for _ in 0..children {
                let child = enter_slow(drawing, 2, at(clock)).unwrap();
                clock += 1;
                exit_at(child, at(clock));
            }
            clock += 1;
            exit_at(parent, at(clock));
        }
        let out = query(&mut cx, &words("draws"), &[("by".into(), "calls".into())]);
        assert!(out[0].starts_with("draws: 2 frames over"), "{}", out[0]);
        assert!(out[0].ends_with("2 widgets drew"), "{}", out[0]);
        // by calls: the child first, 3 calls over 2 frames: 3 ms in all, 2 ms in its worst
        assert_eq!(out[4], "| uid 2 | - | 2/2 | 1.5 | 1.50 | 1.50 | 2.00 |");
        // the parent: 3 + 4 ms inclusive, 2 ms of its own in each frame
        assert_eq!(out[5], "| uid 1 | - | 2/2 | 1.0 | 2.00 | 3.50 | 2.00 |");
        // the report ended the recording
        assert!(!ON.load(Relaxed));
        assert!(query(&mut cx, &words("draws"), &[])[0].contains("no recording"));
    }
}
