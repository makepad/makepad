use std::sync::Arc;

use super::{NetworkBackend, UnsupportedBackend};

pub(crate) fn create_backend() -> Arc<dyn NetworkBackend> {
    Arc::new(UnsupportedBackend::new("android backend not migrated yet"))
}
