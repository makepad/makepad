//! An AppKit quit held by a session ends through a YES reply, keeps running through NO, ends
//! with 101 when the app's thread panics while it is held, and ends with the code a stop names
//! (`stop_main_loop_with`: a run host's failed program) instead of AppKit's 0. A logout's quit that the app
//! cancelled (NSTerminateCancel) aborted the whole logout. On every platform, a stop with a code
//! and no held quit returns the main loop and the process ends with that code, the way tools/run
//! ends a run (the `returns` case). Each case owns its main loop in a child process; on macOS a
//! hidden Metal window is enough to make the session one AppKit asks. No screen interaction is
//! needed.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

const CHILD: &str = "MAKEPAD_QUIT_LATER_CHILD";
/// The code the `code` and `returns` cases' stops name.
const CODE: i32 = 3;
const RETURNED: &str = "quit_later: the main loop returned";

fn main() {
    match std::env::var(CHILD) {
        Ok(case) if case == "returns" => returns(),
        #[cfg(target_os = "macos")]
        Ok(case) => mac::child(case),
        #[cfg(not(target_os = "macos"))]
        Ok(case) => panic!("quit_later: no child case {case} on this platform"),
        Err(_) => parent(),
    }
}

fn parent() {
    let exe = std::env::current_exe().expect("the test's own path");
    let cases: &[&str] = if cfg!(target_os = "macos") { &["quit", "cancel", "panic", "code", "returns"] } else { &["returns"] };
    for &case in cases {
        let out = Command::new(&exe)
            .env(CHILD, case)
            .env("MAKEPAD_HIDE_WINDOWS", "1")
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .output()
            .unwrap_or_else(|e| panic!("{case}: the child could not run: {e}"));
        let err = String::from_utf8_lossy(&out.stderr);
        eprint!("{err}");
        if case == "returns" {
            assert_eq!(out.status.code(), Some(CODE), "returns: a stop with a code and no held quit ends the process with the code");
            assert!(err.contains(RETURNED), "returns: the main loop did not return after the stop");
            continue;
        }
        check(case, &out, &err);
    }
    // the verdict in libtest's words: what reads a run's tally (devhub) counts it
    println!("test result: ok. {} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out", cases.len());
}

#[cfg(target_os = "macos")]
use mac::check;

#[cfg(not(target_os = "macos"))]
fn check(case: &str, _: &std::process::Output, _: &str) {
    unreachable!("quit_later: {case} runs on macOS only")
}

/// The run host's way (tools/run): the run's code kept in a static, the stop from another thread
/// with it, then the main loop's caller ends the process with it.
fn returns() {
    static EXIT: AtomicI32 = AtomicI32::new(0);
    makepad_hal::expect_main_loop();
    let stopper = std::thread::Builder::new().name("quit-later-stop".into()).spawn(|| {
        // (the main loop is usually waiting by now; a stop before it starts returns it at once)
        std::thread::sleep(Duration::from_millis(50));
        EXIT.store(CODE, Ordering::Release);
        makepad_hal::stop_main_loop_with(EXIT.load(Ordering::Acquire));
    }).expect("the stopping thread");
    makepad_hal::run_main_loop();
    eprintln!("{RETURNED}");
    stopper.join().expect("the stopping thread");
    makepad_hal::Hal::global().before_exit();
    std::process::exit(EXIT.load(Ordering::Acquire));
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{CODE, RETURNED};
    use makepad_apple_sys::*;
    use makepad_hal::*;
    use std::process::Output;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const WIN: Id = Id::new(0, 1);
    const DEADLINE: Duration = Duration::from_secs(30);
    /// The child's own word that its quit was held (AppKit answered NSTerminateLater): the app ran on
    /// while its quit waited for it, a frame presented and its main thread ran a block queued after
    /// the ask. A quit answered NOW exits from inside AppKit's delegate and never runs that block (a
    /// frame alone can present before that exit: a HAL that answered NOW passed on it, 11-22). (The
    /// HAL's own lines for each answer went with the push review of 11-16.)
    const HELD: &str = "quit_later: the app ran on while its quit was held (a frame, a main-thread block)";
    /// The child's own word that `terminate:` returned: only a held quit answered NO returns it.
    const CANCELLED: &str = "quit_later: terminate: returned after the session's QuitCancel";

    /// A held-quit case's child ended as it should. How each answer shows without the HAL saying it:
    /// YES ends the process with 0 and the main loop never returns; a stop with a code (a panicking
    /// thread's 101) ends it with that code, the main loop never returning; NO returns `terminate:`
    /// and the app runs on until its main loop returns.
    pub fn check(case: &str, out: &Output, err: &str) {
        assert!(err.contains(HELD), "{case}: AppKit was not answered NSTerminateLater (the app did not run on while its quit was held)");
        if case == "panic" {
            assert_eq!(out.status.code(), Some(101), "panic: a held quit ended by a panicking thread keeps the failure code (not YES's 0)");
            assert!(!err.contains(RETURNED), "panic: the main loop returned instead of the stop path ending the app with 101 itself");
            return;
        }
        if case == "code" {
            assert_eq!(out.status.code(), Some(CODE), "code: a held quit ended by a stop with a code keeps that code (not YES's 0)");
            assert!(!err.contains(RETURNED), "code: the main loop returned instead of the stop ending the app");
            return;
        }
        assert!(out.status.success(), "{case}: the child failed: {}", out.status);
        if case == "quit" {
            assert!(!err.contains(RETURNED), "quit: the main loop returned instead of the held termination's YES ending the app");
            assert!(!err.contains(CANCELLED), "quit: the held termination was answered NO");
        } else {
            assert!(err.contains(CANCELLED), "cancel: the held termination was not answered NO");
            assert!(err.contains(RETURNED), "cancel: the main loop did not return after the session closed (the cancelled termination was answered YES too)");
        }
    }

    /// Ends the main loop when the session's thread is done, with its code when not 0.
    struct Stop(i32);

    impl Drop for Stop {
        fn drop(&mut self) {
            match self.0 {
                0 => stop_main_loop(),
                code => stop_main_loop_with(code),
            }
        }
    }

    pub fn child(case: String) {
        assert!(matches!(case.as_str(), "quit" | "cancel" | "panic" | "code"), "the child case must be quit, cancel, panic or code");
        expect_main_loop();
        let window = std::thread::Builder::new().name("quit-later".into()).spawn(move || {
            let _stop = Stop(if case == "code" { CODE } else { 0 });
            if case == "panic" {
                // unwinds through Stop: the stop path ends the held quit with the failure code
                answer_quit(&case);
                return;
            }
            // A failed quit case must not become AppKit's successful exit when Stop replies YES.
            if std::panic::catch_unwind(|| answer_quit(&case)).is_err() {
                std::process::exit(101);
            }
        }).expect("the session's thread");
        run_main_loop();
        eprintln!("{RETURNED}");
        let failed = window.join().is_err();
        Hal::global().before_exit();
        if failed {
            std::process::exit(101);
        }
    }

    fn answer_quit(case: &str) {
        let hal = Hal::global();
        let info = hal.open(&OpenReq { backend: backend_id::METAL, ..Default::default() });
        assert_eq!(info.backend, backend_id::METAL, "{case}: Metal HAL with windows");
        let sid = info.session;
        let mut w = CmdWriter::new(sid);
        let mut store = EventStore::new(sid, 64, 1 << 16);
        w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 400.0, 300.0], false);
        present(hal, &mut w, 1);
        wait_for(hal, &mut store, case, "the first frame did not present", |e| matches!(e, HalEvent::FrameDone { frame: 1, .. }));

        let (tx, rx) = mpsc::sync_channel(1);
        if case == "quit" {
            // the automation entry (`quit native=1`), the HAL's op
            assert!(makepad_hal::ask_native_quit(), "quit: AppKit's app runs for a session with windows");
        } else {
            let queued = dispatch_main(move || unsafe {
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                let () = msg_send![ns_app, terminate: nil];
                let _ = tx.try_send(());
            });
            assert!(queued, "{case}: the main loop did not accept the termination request");
        }
        wait_for(hal, &mut store, case, "the session did not receive QUIT_REQUESTED", |e| {
            matches!(e, HalEvent::Lifecycle { kind: lifecycle::QUIT_REQUESTED })
        });
        // held while the app finishes: its frames go on until it answers
        w.clear();
        present(hal, &mut w, 2);
        wait_for(hal, &mut store, case, "no frame presented while the quit was held", |e| matches!(e, HalEvent::FrameDone { frame: 2, .. }));
        // and the main thread runs on: AppKit waits for the answer in its run loop
        let (ran_tx, ran_rx) = mpsc::sync_channel(1);
        assert!(dispatch_main(move || { let _ = ran_tx.try_send(()); }), "{case}: the main loop did not accept a block while the quit was held");
        let deadline = Instant::now() + DEADLINE;
        while ran_rx.try_recv().is_err() {
            assert!(Instant::now() < deadline, "{case}: the main thread did not run on while the quit was held");
            let mut buf = store.buf();
            hal.pump(10_000_000, &mut buf);
        }
        eprintln!("{HELD}");

        if case == "panic" {
            panic!("quit_later: the app panics while its quit is held");
        }
        w.clear();
        if case == "quit" || case == "code" {
            w.quit();
            hal.submit(&w.buf());
            // Quit normally closes the session during submit.
            hal.close(sid);
            return;
        }

        w.quit_cancel();
        hal.submit(&w.buf());
        // The modal terminate: call returns after NO; the next frame must be after that reply.
        let deadline = Instant::now() + DEADLINE;
        while rx.try_recv().is_err() {
            assert!(Instant::now() < deadline, "cancel: terminate: did not return after NO");
            let mut buf = store.buf();
            hal.pump(10_000_000, &mut buf);
        }
        eprintln!("{CANCELLED}");
        w.clear();
        present(hal, &mut w, 3);
        wait_for(hal, &mut store, case, "a frame did not present after NO", |e| matches!(e, HalEvent::FrameDone { frame: 3, .. }));
        w.clear();
        w.window_close(WIN);
        hal.submit(&w.buf());
        assert!(hal.close(sid), "cancel: the session did not stay open until explicitly closed");
    }

    fn present(hal: &Hal, w: &mut CmdWriter, frame: u64) {
        w.frame_begin(frame, FrameMeta::default());
        w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.2, 0.2, 0.2, 1.0]; 4], ..Default::default() });
        w.frame_end(WIN, target::WINDOW);
        hal.submit(&w.buf());
    }

    fn wait_for(hal: &Hal, store: &mut EventStore, case: &str, failure: &str, accept: impl Fn(HalEvent<'_>) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            assert!(Instant::now() < deadline, "{case}: {failure}");
            let mut buf = store.buf();
            let n = hal.pump(10_000_000, &mut buf);
            if store.reader(n, buf.arena_len).any(|(_, e)| accept(e)) {
                return;
            }
        }
    }
}
