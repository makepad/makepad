//! A test's child processes. A test that must run alone (the gandalf worker, its event bus and the
//! lint sink are process-wide) runs this binary again for itself (`--exact`), and under the JIT each
//! such child is a whole run of the binary, its program loaded: about 1.2 GB. The suite's threads
//! started one each, 15 at once (17.8 GB, 11-08); a slot per child keeps them to [`SLOTS`].

use std::process::{Command, ExitStatus, Output};
use std::sync::{Condvar, Mutex};

/// Children at once.
const SLOTS: usize = 2;

static FREE: Mutex<usize> = Mutex::new(SLOTS);
static FREED: Condvar = Condvar::new();

/// A slot, held while its child runs.
struct Slot;

impl Slot {
    fn take() -> Slot {
        let mut free = FREE.lock().unwrap();
        while *free == 0 {
            free = FREED.wait(free).unwrap();
        }
        *free -= 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        *FREE.lock().unwrap() += 1;
        FREED.notify_one();
    }
}

/// `command` run to its end in a slot, its output kept.
pub(crate) fn output(command: &mut Command) -> Output {
    let _slot = Slot::take();
    command.output().unwrap()
}

/// `command` run to its end in a slot.
pub(crate) fn status(command: &mut Command) -> ExitStatus {
    let _slot = Slot::take();
    command.status().unwrap()
}
