//! Admitted, uncached blob loads. Pack indexes stay on disk; the payload
//! reservation lives with the returned bytes.
use std::path::Path;

use crate::bounded_read::{self, Bytes, Scratch};
use crate::memory::MemoryAccount;
use crate::{GitError, ObjectId, ObjectKind};

/// Blob payload whose admission lease is released when this value is dropped.
pub struct BlobBytes {
    inner: Bytes,
}

impl BlobBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.inner.data
    }
}

/// Load one blob through the bounded reader. The returned bytes keep their
/// reservation; the payload is not cloned.
pub fn read(
    common_dir: &Path,
    oid: ObjectId,
    memory: &MemoryAccount,
    budget: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<BlobBytes, GitError> {
    if cancel() {
        return Err(GitError::Cancelled);
    }
    let scratch = Scratch::new(memory, budget);
    let mut bytes_read = 0u64;
    let inner = bounded_read::read(
        common_dir,
        &oid,
        ObjectKind::Blob,
        &scratch,
        &mut bytes_read,
        cancel,
    )?;
    Ok(BlobBytes { inner })
}
