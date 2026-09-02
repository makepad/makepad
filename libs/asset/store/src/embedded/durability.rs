//! Immutable catalog generations over namespace-bound value storage.

use makepad_asset_data::sha256;
use makepad_platform::{
    Cx, Event, StorageError, StorageHandle, StorageRequestId, StorageResult,
};
use makepad_sqlite::{MemoryStoreSnapshot, StoreKind, MEMORY_DIRTY_PAGE_BYTES};
use std::collections::BTreeMap;

pub const CATALOG_EXTENT_BYTES: usize = 256 * 1024;
const PAGES_PER_EXTENT: u64 = CATALOG_EXTENT_BYTES as u64 / MEMORY_DIRTY_PAGE_BYTES;
const FORMAT_VERSION: u16 = 1;
const NO_GENERATION: u64 = u64::MAX;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurabilityError {
    Storage(StorageError),
    Corrupt(&'static str),
    NoValidGeneration,
    Busy,
}

impl From<StorageError> for DurabilityError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageCommand {
    Get { key: String },
    Set { key: String, value: Vec<u8> },
    Delete { key: String },
}

impl StorageCommand {
    pub fn value_len(&self) -> usize {
        match self {
            Self::Set { value, .. } => value.len(),
            _ => 0,
        }
    }
}

/// Minimal value-store contract used by deterministic recovery and fault
/// tests. Production code submits the same operations asynchronously through
/// [`StorageHandlePump`].
pub trait StorageValues {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), StorageError>;
    fn delete(&mut self, key: &str) -> Result<(), StorageError>;
    fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExtentRef {
    first_page: u64,
    byte_len: u32,
    digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogDescriptor {
    generation: u64,
    previous: Option<u64>,
    logical_len: u64,
    extents: Vec<ExtentRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogHead {
    generation: u64,
    previous: Option<u64>,
    commit_digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredCatalog {
    pub generation: u64,
    pub previous_generation: Option<u64>,
    pub next_generation: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct PendingCommit {
    commands: Vec<StorageCommand>,
    next: usize,
    snapshot: MemoryStoreSnapshot,
    descriptor: CatalogDescriptor,
}

/// The catalog stays synchronous in memory. This coordinator only snapshots
/// Main and advances the external generation protocol one completed storage
/// operation at a time.
pub struct DurabilityCoordinator {
    durable: Option<CatalogDescriptor>,
    pending: Option<PendingCommit>,
    rollback_required: bool,
    last_error: Option<DurabilityError>,
    committed_snapshot: Option<MemoryStoreSnapshot>,
    next_generation: u64,
}

impl Default for DurabilityCoordinator {
    fn default() -> Self {
        Self {
            durable: None,
            pending: None,
            rollback_required: false,
            last_error: None,
            committed_snapshot: None,
            next_generation: 1,
        }
    }
}

impl DurabilityCoordinator {
    pub fn from_restored(restored: &RestoredCatalog) -> Self {
        Self {
            durable: Some(descriptor_for_bytes(
                restored.generation,
                restored.previous_generation,
                &restored.bytes,
            )),
            next_generation: restored.next_generation,
            ..Self::default()
        }
    }

    pub fn durable_generation(&self) -> Option<u64> {
        self.durable.as_ref().map(|descriptor| descriptor.generation)
    }

    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn rollback_required(&self) -> bool {
        self.rollback_required
    }

    pub fn take_error(&mut self) -> Option<DurabilityError> {
        self.last_error.take()
    }

    pub fn begin_commit(&mut self, snapshot: MemoryStoreSnapshot) -> Result<u64, DurabilityError> {
        if self.pending.is_some() || self.rollback_required {
            return Err(DurabilityError::Busy);
        }
        if snapshot.kind != StoreKind::Main {
            return Err(DurabilityError::Corrupt("catalog snapshot is not Main"));
        }
        let previous = self.durable.as_ref().map(|descriptor| descriptor.generation);
        let generation = self.next_generation;
        if generation == NO_GENERATION {
            return Err(DurabilityError::Corrupt("catalog generation overflow"));
        }
        let descriptor = descriptor_for_bytes(generation, previous, &snapshot.bytes);
        let dirty = dirty_extent_set(&snapshot);
        let mut commands = Vec::new();
        for extent in &descriptor.extents {
            let extent_index = extent.first_page / PAGES_PER_EXTENT;
            let unchanged = self
                .durable
                .as_ref()
                .and_then(|old| old.extents.get(extent_index as usize))
                .is_some_and(|old| old == extent);
            if !unchanged || dirty.contains_key(&extent_index) {
                let start = extent_index as usize * CATALOG_EXTENT_BYTES;
                let end = start + extent.byte_len as usize;
                commands.push(StorageCommand::Set {
                    key: extent_key(extent),
                    value: snapshot.bytes[start..end].to_vec(),
                });
            }
        }

        let descriptor_bytes = encode_descriptor(&descriptor);
        let descriptor_digest = sha256(&descriptor_bytes);
        let commit = encode_commit(generation, descriptor_digest);
        let commit_digest = sha256(&commit);
        let head = encode_head(&CatalogHead { generation, previous, commit_digest });
        commands.push(StorageCommand::Set {
            key: generation_key(generation),
            value: descriptor_bytes,
        });
        commands.push(StorageCommand::Set { key: commit_key(generation), value: commit });
        // The only externally visible publication point is deliberately last.
        commands.push(StorageCommand::Set { key: "catalog/head".into(), value: head });
        self.pending = Some(PendingCommit { commands, next: 0, snapshot, descriptor });
        Ok(generation)
    }

    pub fn next_command(&self) -> Option<&StorageCommand> {
        let pending = self.pending.as_ref()?;
        pending.commands.get(pending.next)
    }

    /// Acknowledge the command returned by [`Self::next_command`]. Any error
    /// permanently aborts the pass and requires reopening from the preceding
    /// durable generation before another mutation.
    pub fn complete(&mut self, result: Result<(), StorageError>) {
        let Some(pending) = self.pending.as_mut() else { return };
        if let Err(error) = result {
            self.last_error = Some(error.into());
            self.rollback_required = true;
            self.pending = None;
            return;
        }
        pending.next += 1;
        if pending.next == pending.commands.len() {
            let pending = self.pending.take().expect("pending commit");
            self.durable = Some(pending.descriptor);
            self.next_generation = self
                .durable
                .as_ref()
                .and_then(|descriptor| descriptor.generation.checked_add(1))
                .unwrap_or(NO_GENERATION);
            self.committed_snapshot = Some(pending.snapshot);
            self.last_error = None;
        }
    }

    pub fn take_completed_snapshot(&mut self) -> Option<MemoryStoreSnapshot> {
        self.committed_snapshot.take()
    }

    pub fn execute_commit(
        &mut self,
        storage: &mut dyn StorageValues,
    ) -> Result<(), DurabilityError> {
        while let Some(command) = self.next_command().cloned() {
            let result = execute_command(storage, command);
            self.complete(result);
            if self.rollback_required {
                return Err(self
                    .take_error()
                    .unwrap_or(DurabilityError::Corrupt("catalog commit failed")));
            }
        }
        Ok(())
    }

    pub fn accept_rollback(&mut self, restored: Option<&RestoredCatalog>) {
        self.pending = None;
        self.rollback_required = false;
        self.last_error = None;
        self.committed_snapshot = None;
        self.durable = restored.map(|catalog| {
            descriptor_for_bytes(catalog.generation, catalog.previous_generation, &catalog.bytes)
        });
        self.next_generation = restored.map_or(1, |catalog| catalog.next_generation);
    }

    /// Submit at most one catalog-generation operation through the real
    /// namespace-bound platform handle. Call again after `handle_event`.
    pub fn drive_handle(
        &mut self,
        cx: &mut Cx,
        pump: &mut StorageHandlePump,
    ) -> Result<bool, DurabilityError> {
        if let Some(completion) = pump.take_completion() {
            self.complete(unit_completion(completion));
        }
        if self.rollback_required {
            return Err(self
                .take_error()
                .unwrap_or(DurabilityError::Corrupt("catalog durability failed")));
        }
        let Some(command) = self.next_command() else { return Ok(true) };
        if !pump.is_pending() {
            pump.submit(cx, command)?;
        }
        Ok(false)
    }

    pub fn handle_event(&mut self, pump: &mut StorageHandlePump, event: &Event) -> bool {
        pump.handle_event(event)
    }
}

fn execute_command(
    storage: &mut dyn StorageValues,
    command: StorageCommand,
) -> Result<(), StorageError> {
    match command {
        StorageCommand::Set { key, value } => storage.set(&key, value),
        StorageCommand::Delete { key } => storage.delete(&key),
        StorageCommand::Get { .. } => Err(StorageError::Protocol(
            "get command cannot be acknowledged as unit".into(),
        )),
    }
}

/// Execute the full recovery search against a deterministic value store.
/// The production opener drives equivalent gets through `StorageHandle`.
pub fn restore_catalog(storage: &dyn StorageValues) -> Result<Option<RestoredCatalog>, DurabilityError> {
    let head_bytes = storage.get("catalog/head")?;
    let mut candidates = Vec::new();
    let had_head = head_bytes.is_some();
    if let Some(bytes) = head_bytes {
        if let Ok(head) = decode_head(&bytes) {
            candidates.push((head.generation, Some(head.commit_digest)));
            if let Some(previous) = head.previous {
                candidates.push((previous, None));
            }
        }
    }
    let mut commits = storage.list("catalog/commit/")?;
    let generations = storage.list("catalog/gen/")?;
    let highest_observed = commits
        .iter()
        .chain(generations.iter())
        .filter_map(|key| parse_generation_suffix(key))
        .chain(candidates.iter().map(|candidate| candidate.0))
        .max()
        .unwrap_or(0);
    let next_generation = highest_observed
        .checked_add(1)
        .ok_or(DurabilityError::Corrupt("catalog generation overflow"))?;
    if !had_head {
        return if commits.is_empty() && generations.is_empty() {
            Ok(None)
        } else {
            Err(DurabilityError::NoValidGeneration)
        };
    }
    commits.sort_unstable_by(|a, b| b.cmp(a));
    for key in commits.into_iter().take(32) {
        if let Some(generation) = parse_generation_suffix(&key) {
            if !candidates.iter().any(|candidate| candidate.0 == generation) {
                candidates.push((generation, None));
            }
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    for (generation, expected_commit) in candidates {
        if let Ok(mut restored) = load_generation(storage, generation, expected_commit) {
            restored.next_generation = next_generation;
            return Ok(Some(restored));
        }
    }
    Err(DurabilityError::NoValidGeneration)
}

/// Retain the current and previous catalog generations and their immutable
/// extents. This is maintenance-only; restore itself never lists chunks.
pub fn reclaim_catalog_garbage(
    storage: &mut dyn StorageValues,
) -> Result<u64, DurabilityError> {
    let head_bytes = storage
        .get("catalog/head")?
        .ok_or(DurabilityError::NoValidGeneration)?;
    let head = decode_head(&head_bytes)?;
    let mut keep_generations = BTreeMap::new();
    keep_generations.insert(head.generation, ());
    if let Some(previous) = head.previous {
        keep_generations.insert(previous, ());
    }
    let mut keep_chunks = BTreeMap::new();
    for generation in keep_generations.keys().copied() {
        let descriptor_bytes = storage
            .get(&generation_key(generation))?
            .ok_or(DurabilityError::Corrupt("retained catalog descriptor missing"))?;
        let descriptor = decode_descriptor(&descriptor_bytes)?;
        for extent in descriptor.extents {
            keep_chunks.insert(extent_key(&extent), ());
        }
    }
    let mut removed = 0;
    for prefix in ["catalog/gen/", "catalog/commit/"] {
        for key in storage.list(prefix)? {
            let keep = parse_generation_suffix(&key)
                .is_some_and(|generation| keep_generations.contains_key(&generation));
            if !keep {
                storage.delete(&key)?;
                removed += 1;
            }
        }
    }
    for key in storage.list("catalog/chunk/")? {
        if !keep_chunks.contains_key(&key) {
            storage.delete(&key)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn load_generation(
    storage: &dyn StorageValues,
    generation: u64,
    expected_commit: Option<[u8; 32]>,
) -> Result<RestoredCatalog, DurabilityError> {
    let commit_bytes = storage
        .get(&commit_key(generation))?
        .ok_or(DurabilityError::Corrupt("missing catalog commit"))?;
    if let Some(expected) = expected_commit {
        if sha256(&commit_bytes) != expected {
            return Err(DurabilityError::Corrupt("catalog head commit digest"));
        }
    }
    let descriptor_digest = decode_commit(&commit_bytes, generation)?;
    let descriptor_bytes = storage
        .get(&generation_key(generation))?
        .ok_or(DurabilityError::Corrupt("missing catalog descriptor"))?;
    if sha256(&descriptor_bytes) != descriptor_digest {
        return Err(DurabilityError::Corrupt("catalog descriptor digest"));
    }
    let descriptor = decode_descriptor(&descriptor_bytes)?;
    if descriptor.generation != generation {
        return Err(DurabilityError::Corrupt("catalog descriptor generation"));
    }
    let logical_len = usize::try_from(descriptor.logical_len)
        .map_err(|_| DurabilityError::Corrupt("catalog length overflow"))?;
    let mut bytes = vec![0; logical_len];
    for (index, extent) in descriptor.extents.iter().enumerate() {
        if extent.first_page / PAGES_PER_EXTENT != index as u64 {
            return Err(DurabilityError::Corrupt("catalog extent order"));
        }
        let value = storage
            .get(&extent_key(extent))?
            .ok_or(DurabilityError::Corrupt("missing catalog extent"))?;
        if value.len() != extent.byte_len as usize || sha256(&value) != extent.digest {
            return Err(DurabilityError::Corrupt("catalog extent digest"));
        }
        let start = index * CATALOG_EXTENT_BYTES;
        let end = start + value.len();
        if end > bytes.len() {
            return Err(DurabilityError::Corrupt("catalog extent bounds"));
        }
        bytes[start..end].copy_from_slice(&value);
    }
    Ok(RestoredCatalog {
        generation,
        previous_generation: descriptor.previous,
        next_generation: generation.saturating_add(1),
        bytes,
    })
}

fn descriptor_for_bytes(
    generation: u64,
    previous: Option<u64>,
    bytes: &[u8],
) -> CatalogDescriptor {
    let extents = bytes
        .chunks(CATALOG_EXTENT_BYTES)
        .enumerate()
        .map(|(index, bytes)| ExtentRef {
            first_page: index as u64 * PAGES_PER_EXTENT,
            byte_len: bytes.len() as u32,
            digest: sha256(bytes),
        })
        .collect();
    CatalogDescriptor { generation, previous, logical_len: bytes.len() as u64, extents }
}

fn dirty_extent_set(snapshot: &MemoryStoreSnapshot) -> BTreeMap<u64, ()> {
    snapshot
        .dirty_pages
        .iter()
        .map(|page| (*page / PAGES_PER_EXTENT, ()))
        .collect()
}

fn extent_key(extent: &ExtentRef) -> String {
    format!(
        "catalog/chunk/{:08x}/{}",
        extent.first_page,
        sha256_hex_bytes(extent.digest)
    )
}

fn generation_key(generation: u64) -> String {
    format!("catalog/gen/{generation:020}")
}

fn commit_key(generation: u64) -> String {
    format!("catalog/commit/{generation:020}")
}

fn parse_generation_suffix(key: &str) -> Option<u64> {
    key.rsplit('/').next()?.parse().ok()
}

pub(crate) fn sha256_hex_bytes(digest: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

fn encode_descriptor(descriptor: &CatalogDescriptor) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64 + descriptor.extents.len() * 48);
    bytes.extend_from_slice(b"MPCG");
    put_u16(&mut bytes, FORMAT_VERSION);
    put_u64(&mut bytes, descriptor.generation);
    put_u64(&mut bytes, descriptor.previous.unwrap_or(NO_GENERATION));
    put_u64(&mut bytes, descriptor.logical_len);
    put_u32(&mut bytes, descriptor.extents.len() as u32);
    for extent in &descriptor.extents {
        put_u64(&mut bytes, extent.first_page);
        put_u32(&mut bytes, extent.byte_len);
        bytes.extend_from_slice(&extent.digest);
    }
    append_checksum(&mut bytes);
    bytes
}

fn decode_descriptor(bytes: &[u8]) -> Result<CatalogDescriptor, DurabilityError> {
    let body = checked_body(bytes, b"MPCG")?;
    let mut cursor = 4;
    if take_u16(body, &mut cursor)? != FORMAT_VERSION {
        return Err(DurabilityError::Corrupt("catalog descriptor version"));
    }
    let generation = take_u64(body, &mut cursor)?;
    let previous = generation_option(take_u64(body, &mut cursor)?);
    let logical_len = take_u64(body, &mut cursor)?;
    let count = take_u32(body, &mut cursor)? as usize;
    let expected_count_u64 = if logical_len == 0 {
        0
    } else {
        (logical_len - 1) / CATALOG_EXTENT_BYTES as u64 + 1
    };
    let expected_count = usize::try_from(expected_count_u64)
        .map_err(|_| DurabilityError::Corrupt("catalog extent count overflow"))?;
    if count != expected_count || count > body.len().saturating_sub(cursor) / 44 {
        return Err(DurabilityError::Corrupt("catalog descriptor extent count"));
    }
    let mut extents = Vec::with_capacity(count);
    for index in 0..count {
        let first_page = take_u64(body, &mut cursor)?;
        let byte_len = take_u32(body, &mut cursor)?;
        let digest = take_digest(body, &mut cursor)?;
        let expected_len = (logical_len - index as u64 * CATALOG_EXTENT_BYTES as u64)
            .min(CATALOG_EXTENT_BYTES as u64) as u32;
        if first_page != index as u64 * PAGES_PER_EXTENT || byte_len != expected_len {
            return Err(DurabilityError::Corrupt("catalog descriptor extent shape"));
        }
        extents.push(ExtentRef { first_page, byte_len, digest });
    }
    if cursor != body.len() {
        return Err(DurabilityError::Corrupt("catalog descriptor trailing bytes"));
    }
    Ok(CatalogDescriptor { generation, previous, logical_len, extents })
}

fn encode_commit(generation: u64, descriptor_digest: [u8; 32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(78);
    bytes.extend_from_slice(b"MPCC");
    put_u16(&mut bytes, FORMAT_VERSION);
    put_u64(&mut bytes, generation);
    bytes.extend_from_slice(&descriptor_digest);
    append_checksum(&mut bytes);
    bytes
}

fn decode_commit(bytes: &[u8], generation: u64) -> Result<[u8; 32], DurabilityError> {
    let body = checked_body(bytes, b"MPCC")?;
    let mut cursor = 4;
    if take_u16(body, &mut cursor)? != FORMAT_VERSION
        || take_u64(body, &mut cursor)? != generation
    {
        return Err(DurabilityError::Corrupt("catalog commit marker"));
    }
    let digest = take_digest(body, &mut cursor)?;
    if cursor != body.len() {
        return Err(DurabilityError::Corrupt("catalog commit trailing bytes"));
    }
    Ok(digest)
}

fn encode_head(head: &CatalogHead) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(86);
    bytes.extend_from_slice(b"MPCH");
    put_u16(&mut bytes, FORMAT_VERSION);
    put_u64(&mut bytes, head.generation);
    put_u64(&mut bytes, head.previous.unwrap_or(NO_GENERATION));
    bytes.extend_from_slice(&head.commit_digest);
    append_checksum(&mut bytes);
    bytes
}

fn decode_head(bytes: &[u8]) -> Result<CatalogHead, DurabilityError> {
    let body = checked_body(bytes, b"MPCH")?;
    let mut cursor = 4;
    if take_u16(body, &mut cursor)? != FORMAT_VERSION {
        return Err(DurabilityError::Corrupt("catalog head version"));
    }
    let generation = take_u64(body, &mut cursor)?;
    let previous = generation_option(take_u64(body, &mut cursor)?);
    let commit_digest = take_digest(body, &mut cursor)?;
    if cursor != body.len() {
        return Err(DurabilityError::Corrupt("catalog head trailing bytes"));
    }
    Ok(CatalogHead { generation, previous, commit_digest })
}

fn append_checksum(bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&sha256(bytes));
}

fn checked_body<'a>(bytes: &'a [u8], magic: &[u8; 4]) -> Result<&'a [u8], DurabilityError> {
    if bytes.len() < 36 || &bytes[..4] != magic {
        return Err(DurabilityError::Corrupt("catalog record framing"));
    }
    let split = bytes.len() - 32;
    if sha256(&bytes[..split]) != bytes[split..] {
        return Err(DurabilityError::Corrupt("catalog record checksum"));
    }
    Ok(&bytes[..split])
}

fn generation_option(value: u64) -> Option<u64> {
    (value != NO_GENERATION).then_some(value)
}

pub(crate) fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn take_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, DurabilityError> {
    let value = take_array::<2>(bytes, cursor)?;
    Ok(u16::from_le_bytes(value))
}
pub(crate) fn take_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, DurabilityError> {
    let value = take_array::<4>(bytes, cursor)?;
    Ok(u32::from_le_bytes(value))
}
pub(crate) fn take_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, DurabilityError> {
    let value = take_array::<8>(bytes, cursor)?;
    Ok(u64::from_le_bytes(value))
}
pub(crate) fn take_digest(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 32], DurabilityError> {
    take_array::<32>(bytes, cursor)
}
pub(crate) fn take_array<const N: usize>(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<[u8; N], DurabilityError> {
    let end = cursor
        .checked_add(N)
        .ok_or(DurabilityError::Corrupt("record cursor overflow"))?;
    let source = bytes
        .get(*cursor..end)
        .ok_or(DurabilityError::Corrupt("truncated record"))?;
    let mut value = [0; N];
    value.copy_from_slice(source);
    *cursor = end;
    Ok(value)
}

/// One-in-flight adapter from storage commands to the platform async API.
pub struct StorageHandlePump {
    handle: StorageHandle,
    pending: Option<StorageRequestId>,
    completion: Option<Result<StorageResult, StorageError>>,
}

impl StorageHandlePump {
    pub fn new(handle: StorageHandle) -> Self {
        Self { handle, pending: None, completion: None }
    }

    pub fn namespace(&self) -> &str {
        self.handle.namespace()
    }

    pub fn submit(&mut self, cx: &mut Cx, command: &StorageCommand) -> Result<(), DurabilityError> {
        if self.pending.is_some() {
            return Err(DurabilityError::Busy);
        }
        self.pending = Some(match command {
            StorageCommand::Get { key } => self.handle.get(cx, key),
            StorageCommand::Set { key, value } => self.handle.set(cx, key, value.clone()),
            StorageCommand::Delete { key } => self.handle.delete(cx, key),
        });
        Ok(())
    }

    pub fn handle_event(&mut self, event: &Event) -> bool {
        let Event::Storage(responses) = event else { return false };
        let Some(pending) = self.pending else { return false };
        let Some(response) = responses.iter().find(|response| {
            response.request_id == pending && response.namespace == self.handle.namespace()
        }) else {
            return false;
        };
        self.pending = None;
        self.completion = Some(response.result.clone());
        true
    }

    pub fn take_completion(&mut self) -> Option<Result<StorageResult, StorageError>> {
        self.completion.take()
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

pub fn unit_completion(
    completion: Result<StorageResult, StorageError>,
) -> Result<(), StorageError> {
    match completion? {
        StorageResult::Unit => Ok(()),
        _ => Err(StorageError::Protocol("storage write returned non-unit result".into())),
    }
}
