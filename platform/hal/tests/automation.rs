//! The automation runner against a fake app on the headless HAL: the app answers `widgets`
//! queries and repaints when it is clicked, the way a Cx client does.

use makepad_hal::automation::{Config, Row, Runner, Script};
use makepad_hal::*;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sink {
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap().lines().map(String::from).collect()
    }
}

/// A one-window app with a button and a display; a click on the button makes the display show 15.
fn app(hal: &Hal, session: u32, stop: &AtomicBool) {
    let win = Id::new(0, 1);
    let mut w = CmdWriter::new(session);
    w.window_create(win, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 200.0, 100.0], false);
    let mut frame = 0;
    let paint = |w: &mut CmdWriter, frame: &mut u64| {
        *frame += 1;
        w.frame_begin(*frame, FrameMeta::default());
        w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
        w.frame_end(win, target::WINDOW);
    };
    paint(&mut w, &mut frame);
    hal.submit(&w.buf());
    w.clear();
    let mut shown = "0";
    let mut store = EventStore::new(session, 64, 4096);
    while !stop.load(Ordering::Relaxed) {
        let mut buf = store.buf();
        let n = hal.pump(2_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::MouseUp { x, y, .. } if (10.0..50.0).contains(&x) && (10.0..30.0).contains(&y) => {
                    shown = "15";
                    paint(&mut w, &mut frame);
                }
                HalEvent::Query { req, bytes } if bytes == b"widgets" => {
                    let row = |path: &str, visible, x, y, text: &str| Row { path: path.into(), kind: "Widget".into(), visible, x, y, w: 40.0, h: 20.0, window: Some(win), text: text.into() };
                    let reply = [row("ui.k_add", true, 10.0, 10.0, "+"), row("ui.display", true, 10.0, 50.0, shown), row("ui.hidden", false, 0.0, 0.0, "x")]
                        .iter()
                        .map(|r| r.encode() + "\n")
                        .collect::<String>();
                    w.query_reply(req, reply.as_bytes());
                }
                HalEvent::Query { req, bytes } if bytes == b"poke" => {
                    // an id the HAL never saw: counted as stale
                    w.window_close(Id::new(9, 1));
                    w.query_reply(req, b"poked");
                }
                HalEvent::Query { req, .. } => w.query_reply(req, b"error: unknown request"),
                _ => {}
            }
        }
        if !w.is_empty() {
            hal.submit(&w.buf());
            w.clear();
        }
    }
}

fn run(script: &str, timeout_ms: u64) -> (automation::Outcome, Vec<String>, String) {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::HEADLESS, app_key: 7, flags: 0, dpi_hint: 1.0, parent: 0 });
    let sink = Sink::default();
    let stop = AtomicBool::new(false);
    let mut cfg = Config::new(info.session, info.dpi);
    cfg.step_timeout = Duration::from_millis(timeout_ms);
    let mut runner = Runner::new(&hal, Script::parse(script).unwrap(), cfg, Box::new(sink.clone()));
    let outcome = std::thread::scope(|s| {
        s.spawn(|| app(&hal, info.session, &stop));
        let outcome = runner.run(&hal);
        stop.store(true, Ordering::Relaxed);
        outcome
    });
    hal.close(info.session);
    let summary = outcome.summary_line(&hal);
    (outcome, sink.lines(), summary)
}

#[test]
fn a_script_drives_the_app_and_every_step_reports() {
    let (outcome, lines, summary) = run(
        "wait_frame\nbudget stale_ids == 0\nclick k_add\nwait_frame\nquery widgets\nexpect \"ui.display\\tWidget\\t1\\t10\\t50\\t40\\t20\\t0:1\\t15\"\nsnapshot first\nclock advance 10\nsleep_frames 1\n",
        3000,
    );
    assert!(outcome.ok(), "{lines:?}");
    assert_eq!((outcome.steps, outcome.ran), (9, 9));
    assert_eq!(lines.len(), 9);
    assert!(lines.iter().all(|l| l.contains("\"ok\":true")), "{lines:?}");
    assert!(lines[2].contains("\"cmd\":\"click k_add\"") && lines[2].contains("ui.k_add at (30, 20)"), "{}", lines[2]);
    assert!(summary.starts_with("{\"summary\":true,\"ok\":true,\"steps\":9,\"ran\":9,\"first_failure\":null"), "{summary}");
    assert!(summary.contains("\"ledger_after_close\":{\"textures\":0,\"buffers\":0,\"pipelines\":0,\"windows\":0,\"entries\":0,\"bytes\":0}"), "{summary}");
    assert!(outcome.frames >= 2);
}

#[test]
fn a_failing_expect_is_the_first_failure() {
    let (outcome, lines, summary) = run("query widgets\nexpect \"ui.display\\tWidget\\t1\\t10\\t50\\t40\\t20\\t0:1\\t15\"\nwait_frame\n", 400);
    assert!(!outcome.ok());
    assert_eq!(outcome.ran, 2, "the run stops at the failure");
    let f = outcome.failure.as_ref().unwrap();
    assert_eq!((f.step, f.line), (2, 2));
    assert!(f.why.contains("not in the reply to `widgets`"), "{}", f.why);
    assert_eq!(lines.len(), 2);
    assert!(lines[1].contains("\"ok\":false"));
    assert!(summary.contains("\"ok\":false") && summary.contains("\"first_failure\":{\"step\":2,\"line\":2"), "{summary}");
}

#[test]
fn a_click_on_a_missing_widget_and_an_unknown_request_fail() {
    let (outcome, _, _) = run("click nothing_here\n", 300);
    assert!(outcome.failure.unwrap().why.contains("no visible widget `nothing_here`"));
    let (outcome, _, _) = run("click ui.hidden\n", 300);
    assert!(outcome.failure.is_some(), "invisible widgets cannot be clicked");
    let (outcome, _, _) = run("query bogus\n", 300);
    assert!(outcome.failure.unwrap().why.contains("refused `bogus`"));
    let (outcome, _, _) = run("expect x\n", 300);
    assert!(outcome.failure.unwrap().why.contains("without an earlier query"));
}

#[test]
fn a_frame_that_never_comes_times_out() {
    let (outcome, _, _) = run("wait_frame\nwait_frame\n", 300);
    let f = outcome.failure.unwrap();
    assert_eq!(f.step, 2);
    assert!(f.why.contains("no 1 frame(s)"));
}

#[test]
fn budgets_are_checked_at_every_step() {
    let (outcome, lines, _) = run("wait_frame\nbudget ledger_bytes <= 0\nbudget stale_ids == 0\nclick k_add\n", 2000);
    assert!(outcome.ok(), "{lines:?}");
    assert!(outcome.budgets.iter().all(|b| b.ok) && outcome.budgets.len() == 2);
    let (outcome, _, summary) = run("budget stale_ids == 0\nquery poke\nwait_frame\n", 2000);
    let f = outcome.failure.unwrap();
    assert_eq!(f.step, 2);
    assert!(f.why.contains("budget stale_ids exceeded: 1 > 0"), "{}", f.why);
    assert!(summary.contains("\"budgets\":[{\"name\":\"stale_ids\",\"limit\":0,\"value\":1,\"ok\":false}]"), "{summary}");
}
