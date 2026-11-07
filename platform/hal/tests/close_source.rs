//! A synthetic click on a window's close button names its poster in the close line (macOS). Four
//! apps closed within 40 s on clicks on their close buttons that no person may have made (11-06),
//! and the line could not tell a person's click from one a process posted.
//! The test shows a window and posts a click on its close button into its own event queue: the
//! Quartz event behind it carries the pid of the process that made it, as the window server's does
//! for a click another process posts, and the line must name this process by it. (A post through
//! the window server, `CGEventPostToPid` even to itself, needs the post-event access a person grants
//! in a TCC prompt: without it nothing arrives.) Needs a screen: a hidden window
//! (`MAKEPAD_HIDE_WINDOWS`) is never on it to be hit, so the window shows in a child of the test,
//! which says the line on its stderr.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    if std::env::var_os(mac::CHILD).is_some() {
        mac::child()
    } else {
        mac::parent()
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use makepad_apple_sys::*;
    use makepad_hal::*;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    pub const CHILD: &str = "MAKEPAD_CLOSE_SOURCE_CHILD";
    const WIN: Id = Id::new(0, 1);
    /// How long the window may take to show, and the close request to come after the click.
    const DEADLINE: Duration = Duration::from_secs(30);
    /// A click posted before the window takes it is clicked again after this.
    const CLICK_AGAIN: Duration = Duration::from_secs(2);

    /// The test: its child shows the window and clicks; its close line names the child.
    pub fn parent() {
        let exe = std::env::current_exe().expect("the test's own path");
        let run = Command::new(exe).env(CHILD, "1").env_remove("MAKEPAD_HIDE_WINDOWS").stdout(Stdio::inherit()).stderr(Stdio::piped()).spawn().expect("the child");
        let pid = run.id();
        let out = run.wait_with_output().expect("the child's end");
        let err = String::from_utf8_lossy(&out.stderr);
        eprint!("{err}");
        assert!(out.status.success(), "the child failed: {}", out.status);
        let line = err.lines().find(|l| l.contains("asked to close (windowShouldClose:)")).expect("the child's window was asked to close");
        let want = format!("on its close button, from this app itself, pid {pid} ");
        assert!(line.contains(&want), "the close line names its poster ({want:?}): {line}");
        // the verdict in libtest's words: what reads a run's tally (devhub) counts it
        println!("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out");
    }

    /// Ends the main loop when the window's thread is done, however it ends.
    struct Stop;

    impl Drop for Stop {
        fn drop(&mut self) {
            stop_main_loop();
        }
    }

    extern "C" {
        fn pthread_main_np() -> i32;
    }

    pub fn child() {
        if unsafe { pthread_main_np() } == 0 {
            // a host runs the window system on the main thread (the JIT's runner) and `main` on a
            // thread of its own
            click_close();
            return;
        }
        expect_main_loop();
        let window = std::thread::Builder::new().name("close-source".into()).spawn(|| {
            let _stop = Stop;
            click_close();
        });
        let window = window.expect("the window's thread");
        run_main_loop();
        let failed = window.join().is_err();
        Hal::global().before_exit();
        if failed {
            std::process::exit(101);
        }
    }

    /// Shows the window, clicks its close button until the app is asked to close it.
    fn click_close() {
        let hal = Hal::global();
        let info = hal.open(&OpenReq { backend: backend_id::METAL, ..Default::default() });
        assert_eq!(info.backend, backend_id::METAL, "Metal HAL with windows");
        let sid = info.session;
        let mut w = CmdWriter::new(sid);
        w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 400.0, 300.0], false);
        w.frame_begin(1, FrameMeta::default());
        w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.2, 0.2, 0.2, 1.0]; 4], ..Default::default() });
        w.frame_end(WIN, target::WINDOW);
        hal.submit(&w.buf());
        let mut store = EventStore::new(sid, 64, 1 << 16);
        let mut shown = false;
        let mut asked = false;
        let mut clicked: Option<Instant> = None;
        let deadline = Instant::now() + DEADLINE;
        while !asked {
            assert!(Instant::now() < deadline, "the window {}", if shown { "was not asked to close after the clicks" } else { "did not show" });
            let mut buf = store.buf();
            let n = hal.pump(10_000_000, &mut buf);
            for (_, e) in store.reader(n, buf.arena_len) {
                match e {
                    HalEvent::FrameDone { frame: 1, .. } => shown = true,
                    HalEvent::CloseRequested { window } if window == WIN => asked = true,
                    _ => {}
                }
            }
            if shown && !asked && clicked.is_none_or(|at| at.elapsed() > CLICK_AGAIN) {
                clicked = Some(Instant::now());
                click_close_button();
            }
        }
        w.clear();
        w.window_close(WIN);
        hal.submit(&w.buf());
        assert!(hal.close(sid));
    }

    /// A left click on the shown window's close button, posted into this app's event queue on the
    /// main thread (none while the window is not on screen). The click is the app's own: no field
    /// of its Quartz event is set here.
    fn click_close_button() {
        let (tx, rx) = mpsc::channel();
        let queued = dispatch_main(move || unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let windows: ObjcId = msg_send![ns_app, windows];
            let count: u64 = msg_send![windows, count];
            for i in 0..count {
                let win: ObjcId = msg_send![windows, objectAtIndex: i];
                let visible: bool = msg_send![win, isVisible];
                let close: ObjcId = msg_send![win, standardWindowButton: 0u64];
                if !visible || close == nil {
                    continue;
                }
                let bounds: NSRect = msg_send![close, bounds];
                let in_window: NSRect = msg_send![close, convertRect: bounds toView: nil];
                let at = NSPoint { x: in_window.origin.x + in_window.size.width / 2.0, y: in_window.origin.y + in_window.size.height / 2.0 };
                let number: isize = msg_send![win, windowNumber];
                eprintln!("close_source: clicking ({:.0}, {:.0}) in window {number}", at.x, at.y);
                for kind in [NSEventType::NSLeftMouseDown, NSEventType::NSLeftMouseUp] {
                    let info: ObjcId = msg_send![class!(NSProcessInfo), processInfo];
                    let time: f64 = msg_send![info, systemUptime];
                    let event: ObjcId = msg_send![
                        class!(NSEvent),
                        mouseEventWithType: kind
                        location: at
                        modifierFlags: 0u64
                        timestamp: time
                        windowNumber: number
                        context: nil
                        eventNumber: 1isize
                        clickCount: 1isize
                        pressure: 1.0f32
                    ];
                    let () = msg_send![ns_app, postEvent: event atStart: false];
                }
                break;
            }
            let _ = tx.send(());
        });
        assert!(queued, "the main loop runs");
        rx.recv().expect("the main thread's answer");
    }
}
