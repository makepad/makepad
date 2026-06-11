use {
    crate::{log, thread::SignalToUI},
    std::sync::atomic::{AtomicBool, Ordering},
};
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicUsize;

static INSTALLED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "linux")]
static SIGNAL_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }

    if let Err(err) = ctrlc::set_handler(move || {
        #[cfg(target_os = "linux")]
        if SIGNAL_COUNT.fetch_add(1, Ordering::AcqRel) > 0 {
            std::process::exit(130);
        }
        REQUESTED.store(true, Ordering::Release);
        SignalToUI::set_ui_signal();
    }) {
        log!("Failed to install termination signal handler: {err}");
    }
}

pub(crate) fn take_requested() -> bool {
    REQUESTED.swap(false, Ordering::AcqRel)
}
