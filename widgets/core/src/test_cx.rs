//! Pooled test contexts, shared by the core's tests and the family crates'.
//!
//! libtest starts a thread for every test and joins it, so a thread-local
//! `Cx` dies with the test. Registering the widget library on it measured
//! 45 ms (release, one test); the data-grid module's 51 cases then took 1.7 s.
//! `on_test_cx` runs those cases on a few threads that stay up and keep one
//! registered `Cx` each. Finger and key state is cleared on checkout. A
//! style hotload must not use this: a reload would change the next case.
//!
//! Each crate's tests pass the registration they need (the core alone, or
//! the core with a family); one test binary always passes the same one.
#![doc(hidden)]

use crate::{makepad_draw::*, makepad_platform, overlay_place, widget_tree};

thread_local! {
    static TEST_CX_POOL: std::cell::RefCell<Option<Cx>> = const { std::cell::RefCell::new(None) };
    static ON_TEST_CX_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub struct PooledCx {
    cx: Option<Cx>,
}

impl Drop for PooledCx {
    fn drop(&mut self) {
        if let Some(cx) = self.cx.take() {
            TEST_CX_POOL.with(|slot| *slot.borrow_mut() = Some(cx));
        }
    }
}

impl std::ops::Deref for PooledCx {
    type Target = Cx;
    fn deref(&self) -> &Cx {
        self.cx.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for PooledCx {
    fn deref_mut(&mut self) -> &mut Cx {
        self.cx.as_mut().unwrap()
    }
}

/// This thread's pooled `Cx`, registered with `register` the first time.
pub fn checkout_test_cx(register: fn(&mut ScriptVm)) -> PooledCx {
    TEST_CX_POOL.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.is_none() {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.init_cx_os();
            cx.with_vm(register);
            let _ = makepad_platform::shader_error::take();
            *guard = Some(cx);
        }
        let mut cx = guard.take().unwrap();
        cx.fingers = Default::default();
        cx.keyboard = Default::default();
        cx.passes = Default::default();
        cx.draw_lists = Default::default();
        cx.windows = Default::default();
        cx.new_draw_event = Default::default();
        overlay_place::reset_for_test(&mut cx);
        widget_tree::reset_for_test(&mut cx);
        let _ = makepad_platform::shader_error::take();
        PooledCx { cx: Some(cx) }
    })
}

/// Run `f` on one of the threads that keep a pooled `Cx`.
pub fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    if ON_TEST_CX_THREAD.with(|flag| flag.get()) {
        f();
        return;
    }
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    test_cx_jobs()
        .send(TestCxJob {
            run: Box::new(f),
            done: done_tx,
        })
        .expect("widget test Cx worker is gone");
    match done_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(payload)) => std::panic::resume_unwind(payload),
        Err(_) => panic!("widget test Cx worker exited before the test finished"),
    }
}

struct TestCxJob {
    run: Box<dyn FnOnce() + Send>,
    done: std::sync::mpsc::Sender<std::thread::Result<()>>,
}

fn test_cx_jobs() -> &'static std::sync::mpsc::Sender<TestCxJob> {
    static JOBS: std::sync::OnceLock<std::sync::mpsc::Sender<TestCxJob>> = std::sync::OnceLock::new();
    JOBS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<TestCxJob>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        for i in 0..4 {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("widget-test-cx-{i}"))
                .spawn(move || {
                    ON_TEST_CX_THREAD.with(|flag| flag.set(true));
                    loop {
                        let job = {
                            let guard = rx.lock().unwrap_or_else(|err| err.into_inner());
                            guard.recv()
                        };
                        let Ok(job) = job else { break };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.run));
                        if let Err(payload) = &result {
                            let msg = payload
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                                .unwrap_or_else(|| "non-string panic".to_string());
                            eprintln!("widget test cx panic: {msg}");
                        }
                        let _ = job.done.send(result);
                    }
                })
                .expect("spawn widget test Cx worker");
        }
        tx
    })
}
