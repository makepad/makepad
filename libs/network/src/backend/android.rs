use std::sync::{Arc, Mutex, OnceLock};

use super::{NetworkBackend, UnsupportedBackend};

fn backend_slot() -> &'static Mutex<Option<Arc<dyn NetworkBackend>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn NetworkBackend>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn register_platform_backend(backend: Arc<dyn NetworkBackend>) {
    if let Ok(mut slot) = backend_slot().lock() {
        *slot = Some(backend);
    }
}

pub fn clear_platform_backend() {
    if let Ok(mut slot) = backend_slot().lock() {
        *slot = None;
    }
}

pub(crate) fn create_backend() -> Arc<dyn NetworkBackend> {
    if let Ok(slot) = backend_slot().lock() {
        if let Some(backend) = slot.as_ref() {
            return Arc::clone(backend);
        }
    }
    Arc::new(UnsupportedBackend::new(
        "android backend shim not registered by makepad-platform",
    ))
}
