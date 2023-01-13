use std::{cell::Cell, fmt};

thread_local! {
    static ENTERED: Cell<bool> = Cell::new(false);
}

/// An RAII guard used to mark the current thread as no longer being within the scope of an
/// executor when dropped.
#[derive(Debug)]
pub struct Enter {
    _priv: (),
}

impl Drop for Enter {
    fn drop(&mut self) {
        ENTERED.with(|c| {
            assert!(c.get());
            c.set(false);
        });
    }
}

/// An error that occurs when attempting to run an executor within the scope of another executor on
/// the same thread.
#[derive(Debug)]
pub struct EnterError {
    _priv: (),
}

impl fmt::Display for EnterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "can't run an executor within the scope of another executor on the same thread")
    }
}

/// Marks the current thread as being within the scope of an executor.
/// 
/// This is a helper function used to ensure that users don't accidentally run an executor within
/// the scope of another executor on the same thread. Executors should call this function before
/// they begin executing a task, and drop the returned RAII guard after they finished executing the
/// task.
pub fn enter() -> Result<Enter, EnterError> {
    ENTERED.with(|entered| {
        if entered.get() {
            Err(EnterError { _priv: () })
        } else {
            entered.set(true);
            Ok(Enter { _priv: () })
        }
    })
}
