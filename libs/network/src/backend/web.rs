use std::sync::Arc;

use super::{NetworkBackend, UnsupportedBackend};

pub(crate) fn create_backend() -> Arc<dyn NetworkBackend> {
    Arc::new(UnsupportedBackend::new("web backend not migrated yet"))
}
