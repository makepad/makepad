//! Portable content-addressed storage boundary.
//!
//! Native builds use [`crate::FsCas`], whose implementation lives in the
//! filesystem-only module. Embedded builds start with [`MemoryCas`]; E2 will
//! replace its backing map with storage-managed objects without changing the
//! catalog-facing interface.

use crate::budget::Budgets;
use crate::error::{ServerError, ServerResult};
use makepad_asset_data::BlobId;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobCommit {
    pub blob_id: BlobId,
    pub size: u64,
    pub deduped: bool,
}

/// Synchronous CAS operations needed by the portable catalog façade.
///
/// Durable embedded implementations perform their asynchronous storage work
/// in the request state machine before reporting completion; catalog code
/// never waits on browser storage.
pub trait Cas {
    fn recover(&self) -> ServerResult<u64>;
    fn contains(&self, id: &BlobId) -> bool;
    fn put(&self, bytes: &[u8], expected: Option<BlobId>) -> ServerResult<BlobCommit>;
    fn read_verified(&self, id: &BlobId) -> ServerResult<Vec<u8>>;
    fn remove_object(&self, id: &BlobId) -> ServerResult<bool>;
}

/// E1's portable CAS. It deliberately provides no durability claim: E2 will
/// retain this API while placing object chunks and manifests in `cx.storage`.
pub struct MemoryCas {
    objects: RefCell<BTreeMap<BlobId, Vec<u8>>>,
    max_blob_bytes: u64,
}

impl MemoryCas {
    pub fn new(budgets: &Budgets) -> Self {
        Self {
            objects: RefCell::new(BTreeMap::new()),
            max_blob_bytes: budgets.max_blob_bytes,
        }
    }
}

impl Cas for MemoryCas {
    fn recover(&self) -> ServerResult<u64> {
        Ok(0)
    }

    fn contains(&self, id: &BlobId) -> bool {
        self.objects.borrow().contains_key(id)
    }

    fn put(&self, bytes: &[u8], expected: Option<BlobId>) -> ServerResult<BlobCommit> {
        if bytes.len() as u64 > self.max_blob_bytes {
            return Err(ServerError::OverBudget {
                what: "blob bytes",
                limit: self.max_blob_bytes,
                found: bytes.len() as u64,
            });
        }
        let blob_id = BlobId::hash_of(bytes);
        let digest = *blob_id.as_bytes();
        if let Some(expected) = expected {
            if expected != blob_id {
                return Err(ServerError::DigestMismatch {
                    what: "memory cas commit",
                    expected: *expected.as_bytes(),
                    found: digest,
                });
            }
        }
        let mut objects = self.objects.borrow_mut();
        let deduped = objects.contains_key(&blob_id);
        objects.entry(blob_id).or_insert_with(|| bytes.to_vec());
        Ok(BlobCommit { blob_id, size: bytes.len() as u64, deduped })
    }

    fn read_verified(&self, id: &BlobId) -> ServerResult<Vec<u8>> {
        let bytes = self
            .objects
            .borrow()
            .get(id)
            .cloned()
            .ok_or(ServerError::NotFound { what: "cas object" })?;
        let found_id = BlobId::hash_of(&bytes);
        if &found_id != id {
            return Err(ServerError::DigestMismatch {
                what: "memory cas object",
                expected: *id.as_bytes(),
                found: *found_id.as_bytes(),
            });
        }
        Ok(bytes)
    }

    fn remove_object(&self, id: &BlobId) -> ServerResult<bool> {
        Ok(self.objects.borrow_mut().remove(id).is_some())
    }
}
