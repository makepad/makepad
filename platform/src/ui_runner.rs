use crate::*;
use std::marker::PhantomData;
use std::sync::Mutex;

/// Run code on the UI thread from another thread.
///
/// Allows you to mix non-blocking threaded code, with code that reads and updates
/// your widget in the UI thread.
///
/// This can be copied and passed around.
pub struct UiRunner<T> {
    /// Trick to later distinguish actions sent globally thru `Cx::post_action`.
    key: usize,
    /// Enforce a consistent `target` type across `handle` and `defer`.
    ///
    /// `fn() -> W` is used instead of `W` because, in summary, these:
    /// - https://stackoverflow.com/a/50201389
    /// - https://doc.rust-lang.org/nomicon/phantom-data.html#table-of-phantomdata-patterns
    /// - https://doc.rust-lang.org/std/marker/struct.PhantomData.html
    target: PhantomData<fn() -> T>,
}

impl<T> Copy for UiRunner<T> {}

impl<T> Clone for UiRunner<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            target: PhantomData,
        }
    }
}

impl<T> std::fmt::Debug for UiRunner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiRunner").field("key", &self.key).finish()
    }
}
impl<T> PartialEq for UiRunner<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<T: 'static> UiRunner<T> {
    /// Create a new `UiRunner` that dispatches functions as global actions but
    /// differentiates them by the provided `key`.
    ///
    /// If used in a widget, prefer using `your_widget.ui_runner()`.
    /// If used in your app main, prefer using `your_app_main.ui_runner()`.
    pub fn new(key: usize) -> Self {
        Self {
            key,
            target: PhantomData,
        }
    }

    /// Handle all functions scheduled with the `key` of this `UiRunner`.
    ///
    /// You should call this once from your `handle_event` method, like:
    ///
    /// ```rust
    /// fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
    ///    // ... handle other stuff ...
    ///    self.ui_runner().handle(cx, event, scope, self);
    /// }
    /// ```
    ///
    /// Once a function has been handled, it will never run again.
    pub fn handle(self, cx: &mut Cx, event: &Event, scope: &mut Scope, target: &mut T) {
        if let Event::Actions(actions) = event {
            for action in actions {
                if let Some(action) = action.downcast_ref::<UiRunnerAction<T>>() {
                    if action.key != self.key {
                        continue;
                    }

                    if let Some(f) = action.f.lock().unwrap().take() {
                        (f)(target, cx, scope);
                    }
                }
            }
        }
    }

    /// Schedule the provided closure to run on the UI thread.
    pub fn defer(self, f: impl DeferCallback<T>) {
        let action = UiRunnerAction {
            f: Mutex::new(Some(Box::new(f))),
            key: self.key,
        };

        Cx::post_action(action);
    }

    /// Like `defer`, but blocks the current thread until the UI awakes, processes
    /// the closure, and returns the result.
    ///
    /// Generally, you should prefer to use `defer` if you don't need to communicate
    /// a value back. This method may wait a long time if the UI thread is busy so you
    /// should not use it in tight loops.
    pub fn block_on<R: Send + 'static>(
        self,
        f: impl FnOnce(&mut T, &mut Cx, &mut Scope) -> R + Send + 'static,
    ) -> R {
        let (tx, rx) = std::sync::mpsc::channel();
        self.defer(move |target, cx, scope| {
            tx.send(f(target, cx, scope)).unwrap();
        });
        rx.recv().unwrap()
    }
}

/// Private message that is sent to the ui thread with the closure to run.
struct UiRunnerAction<T> {
    f: Mutex<Option<Box<dyn DeferCallback<T>>>>,
    key: usize,
}

impl<T> std::fmt::Debug for UiRunnerAction<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiRunnerAction")
            .field("key", &self.key)
            .field("f", &"...")
            .finish()
    }
}

pub trait DeferCallback<T>: FnOnce(&mut T, &mut Cx, &mut Scope) + Send + 'static {}
impl<T, F: FnOnce(&mut T, &mut Cx, &mut Scope) + Send + 'static> DeferCallback<T> for F {}
