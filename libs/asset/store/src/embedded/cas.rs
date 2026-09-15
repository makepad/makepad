//! Storage-backed, chunked content-addressed objects.

use super::durability::{
    put_u16, put_u32, put_u64, sha256_hex_bytes, take_digest, take_u16, take_u32, take_u64,
    DurabilityError, StorageCommand, StorageHandlePump, StorageValues,
};
use crate::error::ServerError;
use makepad_asset_data::{sha256, BlobId, Sha256};
use makepad_platform::{Cx, Event, StorageError};
use std::collections::BTreeMap;

pub const CAS_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const CAS_HASH_SLICE_BYTES: usize = 256 * 1024;
const FORMAT_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageCasError {
    Storage(StorageError),
    Core(ServerError),
    Corrupt(&'static str),
    InvalidUpload,
}

impl From<StorageError> for StorageCasError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<DurabilityError> for StorageCasError {
    fn from(value: DurabilityError) -> Self {
        match value {
            DurabilityError::Storage(error) => Self::Storage(error),
            _ => Self::Corrupt("CAS record decode"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChunkRef {
    digest: [u8; 32],
    len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectManifest {
    blob: BlobId,
    len: u64,
    chunks: Vec<ChunkRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PartialManifest {
    expected: BlobId,
    len: u64,
    updated_ms: u64,
    chunks: Vec<ChunkRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadProgress {
    pub hashed_bytes: u64,
    pub total_bytes: u64,
    pub cpu_bytes: u32,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadProgress {
    pub verified_bytes: u64,
    pub total_bytes: u64,
    pub cpu_bytes: u32,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageBlobCommit {
    pub blob_id: BlobId,
    pub size: u64,
    pub deduped: bool,
}

pub struct StorageCas {
    max_blob_bytes: u64,
    max_partial_uploads: usize,
}

impl StorageCas {
    pub fn new(max_blob_bytes: u64) -> Self {
        Self { max_blob_bytes, max_partial_uploads: 8 }
    }

    pub fn with_partial_limit(mut self, limit: usize) -> Self {
        self.max_partial_uploads = limit.max(1);
        self
    }

    pub fn start_upload(
        &self,
        storage: &dyn StorageValues,
        upload_id: &str,
        bytes: Vec<u8>,
        expected: BlobId,
        now_ms: u64,
    ) -> Result<StorageUpload, StorageCasError> {
        if bytes.len() as u64 > self.max_blob_bytes {
            return Err(StorageCasError::Core(ServerError::OverBudget {
                what: "blob bytes",
                limit: self.max_blob_bytes,
                found: bytes.len() as u64,
            }));
        }
        let partial_key = partial_key(upload_id);
        let partial = storage
            .get(&partial_key)?
            .map(|value| decode_partial(&value))
            .transpose()?;
        if let Some(partial) = &partial {
            if partial.expected != expected || partial.len != bytes.len() as u64 {
                return Err(StorageCasError::InvalidUpload);
            }
        } else if storage.list("cas/partial/")?.len() >= self.max_partial_uploads {
            return Err(StorageCasError::Core(ServerError::OverBudget {
                what: "partial uploads",
                limit: self.max_partial_uploads as u64,
                found: self.max_partial_uploads as u64 + 1,
            }));
        }
        Ok(StorageUpload {
            bytes,
            expected,
            partial_key,
            now_ms,
            offset: 0,
            hasher: Sha256::new(),
            chunk_hasher: Sha256::new(),
            chunks: Vec::new(),
            resumed: partial.map_or_default(|partial| partial.chunks),
            commit: None,
        })
    }

    /// Begin a cooperatively hashed upload whose writes are driven through a
    /// [`StorageHandlePump`]. Existing content-addressed chunks may be safely
    /// replaced; the object manifest remains the last visibility write.
    pub fn start_handle_upload(
        &self,
        upload_id: &str,
        bytes: Vec<u8>,
        expected: BlobId,
        now_ms: u64,
    ) -> Result<HandleStorageUpload, StorageCasError> {
        if bytes.len() as u64 > self.max_blob_bytes {
            return Err(StorageCasError::Core(ServerError::OverBudget {
                what: "blob bytes",
                limit: self.max_blob_bytes,
                found: bytes.len() as u64,
            }));
        }
        Ok(HandleStorageUpload {
            bytes,
            expected,
            partial_key: partial_key(upload_id),
            now_ms,
            offset: 0,
            hasher: Sha256::new(),
            chunk_hasher: Sha256::new(),
            chunks: Vec::new(),
            command: None,
            after: HandleAfter::None,
            commit: None,
        })
    }

    pub fn put(
        &self,
        storage: &mut dyn StorageValues,
        upload_id: &str,
        bytes: Vec<u8>,
        expected: BlobId,
        now_ms: u64,
    ) -> Result<StorageBlobCommit, StorageCasError> {
        let mut upload = self.start_upload(storage, upload_id, bytes, expected, now_ms)?;
        while !upload.step(storage)?.done {}
        upload.commit.ok_or(StorageCasError::Corrupt("missing CAS commit"))
    }

    pub fn contains(
        &self,
        storage: &dyn StorageValues,
        blob: &BlobId,
    ) -> Result<bool, StorageCasError> {
        let Some(bytes) = storage.get(&object_key(blob))? else { return Ok(false) };
        Ok(decode_object(&bytes, Some(*blob)).is_ok())
    }

    /// Fail closed: no bytes are returned until every chunk and the aggregate
    /// object digest have been verified.
    pub fn read_verified(
        &self,
        storage: &dyn StorageValues,
        blob: &BlobId,
    ) -> Result<Vec<u8>, StorageCasError> {
        let mut read = self.start_read(storage, blob)?;
        while !read.step(storage)?.done {}
        read.finish()
            .ok_or(StorageCasError::Corrupt("verified CAS read has no result"))
    }

    pub fn start_read(
        &self,
        storage: &dyn StorageValues,
        blob: &BlobId,
    ) -> Result<StorageBlobRead, StorageCasError> {
        let manifest_bytes = storage
            .get(&object_key(blob))?
            .ok_or(StorageCasError::Core(ServerError::NotFound { what: "CAS object" }))?;
        let manifest = decode_object(&manifest_bytes, Some(*blob))?;
        if manifest.len > self.max_blob_bytes {
            return Err(StorageCasError::Core(ServerError::OverBudget {
                what: "CAS object bytes",
                limit: self.max_blob_bytes,
                found: manifest.len,
            }));
        }
        let capacity = usize::try_from(manifest.len)
            .map_err(|_| StorageCasError::Corrupt("CAS object length overflow"))?;
        Ok(StorageBlobRead {
            manifest,
            chunk_index: 0,
            chunk: None,
            chunk_offset: 0,
            chunk_hasher: Sha256::new(),
            aggregate: Sha256::new(),
            out: Vec::with_capacity(capacity),
            done: false,
        })
    }

    pub fn read_range_verified(
        &self,
        storage: &dyn StorageValues,
        blob: &BlobId,
        offset: u64,
        len: u32,
    ) -> Result<Vec<u8>, StorageCasError> {
        let bytes = self.read_verified(storage, blob)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(bytes.len());
        let end = start.saturating_add(len as usize).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    /// Delete the object manifest first. Chunks shared with any remaining
    /// manifest are retained; orphan chunks are then reclaimed.
    pub fn delete_object(
        &self,
        storage: &mut dyn StorageValues,
        blob: &BlobId,
    ) -> Result<bool, StorageCasError> {
        let existed = storage.get(&object_key(blob))?.is_some();
        storage.delete(&object_key(blob))?;
        self.reclaim_orphan_chunks(storage)?;
        Ok(existed)
    }

    pub fn reclaim_orphan_chunks(
        &self,
        storage: &mut dyn StorageValues,
    ) -> Result<u64, StorageCasError> {
        let mut reachable = BTreeMap::<[u8; 32], ()>::new();
        for key in storage.list("cas/object/")? {
            let Some(bytes) = storage.get(&key)? else { continue };
            let manifest = decode_object(&bytes, None)?;
            for chunk in manifest.chunks {
                reachable.insert(chunk.digest, ());
            }
        }
        for key in storage.list("cas/partial/")? {
            let Some(bytes) = storage.get(&key)? else { continue };
            let partial = decode_partial(&bytes)?;
            for chunk in partial.chunks {
                reachable.insert(chunk.digest, ());
            }
        }
        let mut removed = 0;
        for key in storage.list("cas/chunk/")? {
            let Some(hex) = key.strip_prefix("cas/chunk/") else { continue };
            let Some(digest) = decode_hex_digest(hex) else { continue };
            if !reachable.contains_key(&digest) {
                storage.delete(&key)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn expire_partials(
        &self,
        storage: &mut dyn StorageValues,
        older_than_ms: u64,
    ) -> Result<u64, StorageCasError> {
        let mut removed = 0;
        for key in storage.list("cas/partial/")? {
            let should_remove = storage
                .get(&key)?
                .and_then(|bytes| decode_partial(&bytes).ok())
                .is_none_or(|partial| partial.updated_ms < older_than_ms);
            if should_remove {
                storage.delete(&key)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Cooperative fail-closed reader. No bytes escape until every 256 KiB hash
/// slice, chunk digest, aggregate object digest, and declared length verify.
pub struct StorageBlobRead {
    manifest: ObjectManifest,
    chunk_index: usize,
    chunk: Option<Vec<u8>>,
    chunk_offset: usize,
    chunk_hasher: Sha256,
    aggregate: Sha256,
    out: Vec<u8>,
    done: bool,
}

impl StorageBlobRead {
    pub fn step(
        &mut self,
        storage: &dyn StorageValues,
    ) -> Result<ReadProgress, StorageCasError> {
        if self.done {
            return Ok(self.progress(0));
        }
        if self.chunk.is_none() && self.chunk_index < self.manifest.chunks.len() {
            let chunk_ref = &self.manifest.chunks[self.chunk_index];
            let bytes = storage
                .get(&chunk_key(chunk_ref.digest))?
                .ok_or(StorageCasError::Corrupt("missing CAS chunk"))?;
            if bytes.len() != chunk_ref.len as usize {
                return Err(StorageCasError::Core(ServerError::SizeMismatch {
                    what: "CAS chunk",
                    expected: chunk_ref.len as u64,
                    found: bytes.len() as u64,
                }));
            }
            self.chunk = Some(bytes);
            self.chunk_offset = 0;
        }
        if let Some(bytes) = &self.chunk {
            let end = self
                .chunk_offset
                .saturating_add(CAS_HASH_SLICE_BYTES)
                .min(bytes.len());
            let slice = &bytes[self.chunk_offset..end];
            self.chunk_hasher.update(slice);
            self.aggregate.update(slice);
            self.out.extend_from_slice(slice);
            let cpu_bytes = slice.len() as u32;
            self.chunk_offset = end;
            if end == bytes.len() {
                let expected = self.manifest.chunks[self.chunk_index].digest;
                let found = std::mem::take(&mut self.chunk_hasher).finalize();
                if found != expected {
                    return Err(StorageCasError::Core(ServerError::DigestMismatch {
                        what: "CAS chunk",
                        expected,
                        found,
                    }));
                }
                self.chunk = None;
                self.chunk_index += 1;
            }
            return Ok(self.progress(cpu_bytes));
        }
        if self.out.len() as u64 != self.manifest.len {
            return Err(StorageCasError::Core(ServerError::SizeMismatch {
                what: "CAS object",
                expected: self.manifest.len,
                found: self.out.len() as u64,
            }));
        }
        let found = std::mem::take(&mut self.aggregate).finalize();
        if found != *self.manifest.blob.as_bytes() {
            return Err(StorageCasError::Core(ServerError::DigestMismatch {
                what: "CAS object",
                expected: *self.manifest.blob.as_bytes(),
                found,
            }));
        }
        self.done = true;
        Ok(self.progress(0))
    }

    pub fn finish(mut self) -> Option<Vec<u8>> {
        self.done.then(|| std::mem::take(&mut self.out))
    }

    fn progress(&self, cpu_bytes: u32) -> ReadProgress {
        ReadProgress {
            verified_bytes: self.out.len() as u64,
            total_bytes: self.manifest.len,
            cpu_bytes,
            done: self.done,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleAfter {
    None,
    Chunk,
    Partial,
    Object,
    PartialDelete,
}

pub struct HandleStorageUpload {
    bytes: Vec<u8>,
    expected: BlobId,
    partial_key: String,
    now_ms: u64,
    offset: usize,
    hasher: Sha256,
    chunk_hasher: Sha256,
    chunks: Vec<ChunkRef>,
    command: Option<StorageCommand>,
    after: HandleAfter,
    commit: Option<StorageBlobCommit>,
}

impl HandleStorageUpload {
    pub fn progress(&self) -> UploadProgress {
        UploadProgress {
            hashed_bytes: self.offset as u64,
            total_bytes: self.bytes.len() as u64,
            cpu_bytes: 0,
            done: self.commit.is_some(),
        }
    }

    pub fn next_command(&self) -> Option<&StorageCommand> {
        self.command.as_ref()
    }

    /// Hash at most 256 KiB and prepare at most one storage command.
    pub fn step_cpu(&mut self) -> Result<UploadProgress, StorageCasError> {
        if self.command.is_some() || self.commit.is_some() {
            return Ok(self.progress());
        }
        if self.offset < self.bytes.len() {
            let end = self
                .offset
                .saturating_add(CAS_HASH_SLICE_BYTES)
                .min(self.bytes.len());
            self.hasher.update(&self.bytes[self.offset..end]);
            self.chunk_hasher.update(&self.bytes[self.offset..end]);
            let cpu_bytes = (end - self.offset) as u32;
            self.offset = end;
            if self.offset % CAS_CHUNK_BYTES == 0 || self.offset == self.bytes.len() {
                let start = self.chunks.len() * CAS_CHUNK_BYTES;
                let chunk_bytes = &self.bytes[start..self.offset];
                let chunk = ChunkRef {
                    digest: std::mem::take(&mut self.chunk_hasher).finalize(),
                    len: chunk_bytes.len() as u32,
                };
                self.chunks.push(chunk.clone());
                self.command = Some(StorageCommand::Set {
                    key: chunk_key(chunk.digest),
                    value: chunk_bytes.to_vec(),
                });
                self.after = HandleAfter::Chunk;
            }
            return Ok(UploadProgress {
                cpu_bytes,
                ..self.progress()
            });
        }
        let found = std::mem::take(&mut self.hasher).finalize();
        if found != *self.expected.as_bytes() {
            return Err(StorageCasError::Core(ServerError::DigestMismatch {
                what: "storage CAS upload",
                expected: *self.expected.as_bytes(),
                found,
            }));
        }
        self.command = Some(StorageCommand::Set {
            key: object_key(&self.expected),
            value: encode_object(&ObjectManifest {
                blob: self.expected,
                len: self.bytes.len() as u64,
                chunks: self.chunks.clone(),
            }),
        });
        self.after = HandleAfter::Object;
        Ok(self.progress())
    }

    pub fn complete(&mut self, result: Result<(), StorageError>) -> Result<(), StorageCasError> {
        result?;
        self.command = None;
        match self.after {
            HandleAfter::Chunk => {
                self.command = Some(StorageCommand::Set {
                    key: self.partial_key.clone(),
                    value: encode_partial(&PartialManifest {
                        expected: self.expected,
                        len: self.bytes.len() as u64,
                        updated_ms: self.now_ms,
                        chunks: self.chunks.clone(),
                    }),
                });
                self.after = HandleAfter::Partial;
            }
            HandleAfter::Partial => self.after = HandleAfter::None,
            HandleAfter::Object => {
                self.command = Some(StorageCommand::Delete { key: self.partial_key.clone() });
                self.after = HandleAfter::PartialDelete;
            }
            HandleAfter::PartialDelete => {
                self.after = HandleAfter::None;
                self.commit = Some(StorageBlobCommit {
                    blob_id: self.expected,
                    size: self.bytes.len() as u64,
                    deduped: false,
                });
            }
            HandleAfter::None => return Err(StorageCasError::Corrupt("unexpected CAS completion")),
        }
        Ok(())
    }

    pub fn drive_handle(
        &mut self,
        cx: &mut Cx,
        pump: &mut StorageHandlePump,
    ) -> Result<UploadProgress, StorageCasError> {
        if let Some(completion) = pump.take_completion() {
            self.complete(super::durability::unit_completion(completion))?;
        }
        let mut progress = self.step_cpu()?;
        if let Some(command) = self.next_command() {
            if !pump.is_pending() {
                pump.submit(cx, command)?;
            }
        }
        progress.done = self.commit.is_some();
        Ok(progress)
    }

    pub fn handle_event(&mut self, pump: &mut StorageHandlePump, event: &Event) -> bool {
        pump.handle_event(event)
    }

    pub fn commit(&self) -> Option<StorageBlobCommit> {
        self.commit
    }
}

pub struct StorageUpload {
    bytes: Vec<u8>,
    expected: BlobId,
    partial_key: String,
    now_ms: u64,
    offset: usize,
    hasher: Sha256,
    chunk_hasher: Sha256,
    chunks: Vec<ChunkRef>,
    resumed: Vec<ChunkRef>,
    commit: Option<StorageBlobCommit>,
}

impl StorageUpload {
    pub fn commit(&self) -> Option<StorageBlobCommit> {
        self.commit
    }

    pub fn step(
        &mut self,
        storage: &mut dyn StorageValues,
    ) -> Result<UploadProgress, StorageCasError> {
        if self.commit.is_some() {
            return Ok(self.progress(0, true));
        }
        if self.offset < self.bytes.len() {
            let end = self
                .offset
                .saturating_add(CAS_HASH_SLICE_BYTES)
                .min(self.bytes.len());
            self.hasher.update(&self.bytes[self.offset..end]);
            self.chunk_hasher.update(&self.bytes[self.offset..end]);
            let cpu_bytes = (end - self.offset) as u32;
            self.offset = end;
            if self.offset % CAS_CHUNK_BYTES == 0 || self.offset == self.bytes.len() {
                let chunk_index = self.chunks.len();
                let start = chunk_index * CAS_CHUNK_BYTES;
                let chunk_bytes = &self.bytes[start..self.offset];
                let chunk = ChunkRef {
                    digest: std::mem::take(&mut self.chunk_hasher).finalize(),
                    len: chunk_bytes.len() as u32,
                };
                if self.resumed.get(chunk_index) != Some(&chunk) {
                    storage.set(&chunk_key(chunk.digest), chunk_bytes.to_vec())?;
                } else {
                    let durable = storage
                        .get(&chunk_key(chunk.digest))?
                        .ok_or(StorageCasError::Corrupt("resumed CAS chunk missing"))?;
                    if durable.len() != chunk.len as usize || sha256(&durable) != chunk.digest {
                        return Err(StorageCasError::Corrupt("resumed CAS chunk corrupt"));
                    }
                }
                self.chunks.push(chunk);
                storage.set(
                    &self.partial_key,
                    encode_partial(&PartialManifest {
                        expected: self.expected,
                        len: self.bytes.len() as u64,
                        updated_ms: self.now_ms,
                        chunks: self.chunks.clone(),
                    }),
                )?;
            }
            return Ok(self.progress(cpu_bytes, false));
        }

        let found = std::mem::take(&mut self.hasher).finalize();
        if found != *self.expected.as_bytes() {
            return Err(StorageCasError::Core(ServerError::DigestMismatch {
                what: "storage CAS upload",
                expected: *self.expected.as_bytes(),
                found,
            }));
        }
        let manifest = ObjectManifest {
            blob: self.expected,
            len: self.bytes.len() as u64,
            chunks: self.chunks.clone(),
        };
        let key = object_key(&self.expected);
        let deduped = storage
            .get(&key)?
            .map(|bytes| decode_object(&bytes, Some(self.expected)).map(|_| ()))
            .transpose()?
            .is_some();
        // The manifest is last: its presence is object visibility.
        storage.set(&key, encode_object(&manifest))?;
        storage.delete(&self.partial_key)?;
        self.commit = Some(StorageBlobCommit {
            blob_id: self.expected,
            size: self.bytes.len() as u64,
            deduped,
        });
        Ok(self.progress(0, true))
    }

    fn progress(&self, cpu_bytes: u32, done: bool) -> UploadProgress {
        UploadProgress {
            hashed_bytes: self.offset as u64,
            total_bytes: self.bytes.len() as u64,
            cpu_bytes,
            done,
        }
    }
}

pub fn object_key(blob: &BlobId) -> String {
    format!("cas/object/{}", sha256_hex_bytes(*blob.as_bytes()))
}

pub fn chunk_key(digest: [u8; 32]) -> String {
    format!("cas/chunk/{}", sha256_hex_bytes(digest))
}

fn partial_key(upload_id: &str) -> String {
    format!("cas/partial/{}", sha256_hex_bytes(sha256(upload_id.as_bytes())))
}

fn encode_object(manifest: &ObjectManifest) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(88 + manifest.chunks.len() * 36);
    bytes.extend_from_slice(b"MPOB");
    put_u16(&mut bytes, FORMAT_VERSION);
    bytes.extend_from_slice(manifest.blob.as_bytes());
    put_u64(&mut bytes, manifest.len);
    put_u32(&mut bytes, manifest.chunks.len() as u32);
    for chunk in &manifest.chunks {
        bytes.extend_from_slice(&chunk.digest);
        put_u32(&mut bytes, chunk.len);
    }
    append_checksum(&mut bytes);
    bytes
}

fn decode_object(
    bytes: &[u8],
    expected: Option<BlobId>,
) -> Result<ObjectManifest, StorageCasError> {
    let body = checked_body(bytes, b"MPOB")?;
    let mut cursor = 4;
    if take_u16(body, &mut cursor)? != FORMAT_VERSION {
        return Err(StorageCasError::Corrupt("CAS object version"));
    }
    let blob = BlobId::from_bytes(take_digest(body, &mut cursor)?);
    if expected.is_some_and(|expected| expected != blob) {
        return Err(StorageCasError::Corrupt("CAS object key mismatch"));
    }
    let len = take_u64(body, &mut cursor)?;
    let count = take_u32(body, &mut cursor)? as usize;
    let expected_count_u64 = if len == 0 {
        0
    } else {
        (len - 1) / CAS_CHUNK_BYTES as u64 + 1
    };
    let expected_count = usize::try_from(expected_count_u64)
        .map_err(|_| StorageCasError::Corrupt("CAS chunk count overflow"))?;
    if count != expected_count || count > body.len().saturating_sub(cursor) / 36 {
        return Err(StorageCasError::Corrupt("CAS object chunk count"));
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        chunks.push(ChunkRef {
            digest: take_digest(body, &mut cursor)?,
            len: take_u32(body, &mut cursor)?,
        });
    }
    if cursor != body.len()
        || chunks.iter().any(|chunk| chunk.len as usize > CAS_CHUNK_BYTES)
        || chunks.iter().map(|chunk| chunk.len as u64).sum::<u64>() != len
    {
        return Err(StorageCasError::Corrupt("CAS object shape"));
    }
    Ok(ObjectManifest { blob, len, chunks })
}

fn encode_partial(partial: &PartialManifest) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(96 + partial.chunks.len() * 36);
    bytes.extend_from_slice(b"MPPT");
    put_u16(&mut bytes, FORMAT_VERSION);
    bytes.extend_from_slice(partial.expected.as_bytes());
    put_u64(&mut bytes, partial.len);
    put_u64(&mut bytes, partial.updated_ms);
    put_u32(&mut bytes, partial.chunks.len() as u32);
    for chunk in &partial.chunks {
        bytes.extend_from_slice(&chunk.digest);
        put_u32(&mut bytes, chunk.len);
    }
    append_checksum(&mut bytes);
    bytes
}

fn decode_partial(bytes: &[u8]) -> Result<PartialManifest, StorageCasError> {
    let body = checked_body(bytes, b"MPPT")?;
    let mut cursor = 4;
    if take_u16(body, &mut cursor)? != FORMAT_VERSION {
        return Err(StorageCasError::Corrupt("CAS partial version"));
    }
    let expected = BlobId::from_bytes(take_digest(body, &mut cursor)?);
    let len = take_u64(body, &mut cursor)?;
    let updated_ms = take_u64(body, &mut cursor)?;
    let count = take_u32(body, &mut cursor)? as usize;
    if count > body.len().saturating_sub(cursor) / 36
        || count as u64 > (len / CAS_CHUNK_BYTES as u64).saturating_add(1)
    {
        return Err(StorageCasError::Corrupt("CAS partial chunk count"));
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        chunks.push(ChunkRef {
            digest: take_digest(body, &mut cursor)?,
            len: take_u32(body, &mut cursor)?,
        });
    }
    if cursor != body.len()
        || chunks.iter().any(|chunk| chunk.len as usize > CAS_CHUNK_BYTES)
        || chunks.iter().map(|chunk| chunk.len as u64).sum::<u64>() > len
    {
        return Err(StorageCasError::Corrupt("CAS partial shape"));
    }
    Ok(PartialManifest { expected, len, updated_ms, chunks })
}

fn append_checksum(bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&sha256(bytes));
}

fn checked_body<'a>(bytes: &'a [u8], magic: &[u8; 4]) -> Result<&'a [u8], StorageCasError> {
    if bytes.len() < 36 || &bytes[..4] != magic {
        return Err(StorageCasError::Corrupt("CAS record framing"));
    }
    let split = bytes.len() - 32;
    if sha256(&bytes[..split]) != bytes[split..] {
        return Err(StorageCasError::Corrupt("CAS record checksum"));
    }
    Ok(&bytes[..split])
}

fn decode_hex_digest(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(digest)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
