use {
    crate::{log, thread::SignalToUI},
    std::sync::atomic::{AtomicBool, Ordering},
};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }

    if let Err(err) = ctrlc::set_handler(move || {
        REQUESTED.store(true, Ordering::Release);
        SignalToUI::set_ui_signal();
    }) {
        log!("Failed to install termination signal handler: {err}");
    }
}

pub(crate) fn take_requested() -> bool {
    REQUESTED.swap(false, Ordering::AcqRel)
}
