//! Settling raw watcher events into observations a consumer can act on.
//!
//! A path is **settled** when all of these hold: no event for it for
//! `quiescence` (150 ms); two reads at least `confirm_gap` (100 ms) apart
//! returned the same bytes (compared through the 20-byte git blob id and
//! the retained bytes themselves, never through the preliminary FNV
//! fingerprint alone); and identity and metadata (length, modification
//! time, inode / file index where the OS has one) matched across those
//! reads. A path that keeps changing for longer than `give_up` (2 s) is
//! reported as [`Settlement::StillChanging`] with a capped exponential
//! backoff and keeps being probed at that cadence until it settles. The
//! give-up clock runs from the first activity and is independent of new
//! notifications: a path that is notified continuously is still read and
//! reported `StillChanging` at `give_up`.
//!
//! Delete tombstones are held through settlement: a `Removed` event only
//! becomes [`Settlement::Removed`] when the path is still missing after
//! quiescence; a delete-and-recreate inside the window is one update. An
//! atomic replacement (temporary written, then renamed over the target) is
//! one update for the target and nothing for the temporary, unless the
//! temporary had already settled, in which case it is reported removed.
//!
//! The settler remembers the last published blob per path: a re-settlement
//! with the same bytes (a `touch`, a save of identical content) is reported
//! as [`Settlement::Unchanged`] so the consumer can acknowledge the
//! observation without re-publishing content.
//!
//! Reads go through [`probe_file`]: the file is opened without following
//! links (`O_NOFOLLOW`; `FILE_FLAG_OPEN_REPARSE_POINT` with reparse points
//! rejected), identity and metadata are captured from the handle before
//! and after the read and matched against the path, and the bytes are
//! retained immutably in the probe together with their git blob id.
//!
//! The consumer drives the settler: [`Settler::observe`] per event, and
//! [`Settler::poll`] with the current time and a reader whenever
//! [`Settler::next_deadline`] passes. Nothing here touches the filesystem
//! unless the consumer's reader does, so the helper is testable with a
//! scripted clock and scripted reads.

use crate::{FileSystemEvent, FileSystemEventKind};
use makepad_git::oid::{hash_object, ObjectId};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// `probe_file` refuses files larger than this: the bytes are retained in
/// the settlement, and nothing a Live consumer wants to see is bigger.
pub const MAX_PROBE_BYTES: u64 = 64 * 1024 * 1024;
/// Constituent event ids retained per settlement (the first ones plus the
/// latest); the count of coalesced events is always exact.
pub const MAX_RETAINED_EVENT_IDS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettleConfig {
    /// No events for this long before the first confirming read.
    pub quiescence: Duration,
    /// Minimum gap between the two confirming reads.
    pub confirm_gap: Duration,
    /// After this much continuous activity, report `StillChanging`.
    pub give_up: Duration,
    /// First backoff interval after `give_up`; doubles up to `max_backoff`.
    pub min_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for SettleConfig {
    fn default() -> Self {
        Self {
            quiescence: Duration::from_millis(150),
            confirm_gap: Duration::from_millis(100),
            give_up: Duration::from_secs(2),
            min_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(2),
        }
    }
}

/// What one read of a path returned: the bytes, their identities and the
/// metadata that must hold still across the confirming reads.
#[derive(Clone, Debug, Eq)]
pub struct FileProbe {
    /// FNV-1a over the bytes: a preliminary fingerprint only, never the
    /// authority for equality.
    pub content_hash: u64,
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// (device, inode) on Unix, (volume serial, file index) on Windows;
    /// `None` where the OS has no stable identity.
    pub identity: Option<(u64, u64)>,
    /// The git blob id of `bytes` (`hash_object("blob", bytes)`): the
    /// content identity consumers key on.
    pub blob: ObjectId,
    /// The settled bytes, immutable and shared with the consumer.
    pub bytes: Arc<[u8]>,
}

impl PartialEq for FileProbe {
    fn eq(&self, other: &Self) -> bool {
        self.content_hash == other.content_hash
            && self.len == other.len
            && self.modified == other.modified
            && self.identity == other.identity
            && self.blob == other.blob
            && (Arc::ptr_eq(&self.bytes, &other.bytes) || self.bytes == other.bytes)
    }
}

impl FileProbe {
    /// Build a probe from bytes the caller already holds (a scripted read,
    /// an in-memory buffer): hashes and blob id are computed here.
    pub fn from_bytes(
        bytes: Arc<[u8]>,
        modified: Option<SystemTime>,
        identity: Option<(u64, u64)>,
    ) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in bytes.iter() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self {
            content_hash: hash,
            len: bytes.len() as u64,
            modified,
            identity,
            blob: hash_object("blob", &bytes),
            bytes,
        }
    }
}

/// Identity and metadata of one look at a file, from a path or a handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    identity: Option<(u64, u64)>,
}

impl Stamp {
    fn agrees(&self, other: &Stamp) -> bool {
        self.len == other.len
            && self.modified == other.modified
            && match (self.identity, other.identity) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

#[cfg(unix)]
fn path_stamp(meta: &std::fs::Metadata) -> Stamp {
    use std::os::unix::fs::MetadataExt;
    Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
        identity: Some((meta.dev(), meta.ino())),
    }
}

#[cfg(not(unix))]
fn path_stamp(meta: &std::fs::Metadata) -> Stamp {
    Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
        identity: None,
    }
}

#[cfg(unix)]
fn handle_stamp(_file: &std::fs::File, meta: &std::fs::Metadata) -> Stamp {
    path_stamp(meta)
}

#[cfg(windows)]
#[repr(C)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        info: *mut ByHandleFileInformation,
    ) -> i32;
}

#[cfg(windows)]
fn handle_stamp(file: &std::fs::File, meta: &std::fs::Metadata) -> Stamp {
    use std::os::windows::io::AsRawHandle;
    let mut info = ByHandleFileInformation {
        file_attributes: 0,
        creation_time: [0; 2],
        last_access_time: [0; 2],
        last_write_time: [0; 2],
        volume_serial_number: 0,
        file_size_high: 0,
        file_size_low: 0,
        number_of_links: 0,
        file_index_high: 0,
        file_index_low: 0,
    };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as *mut _, &mut info) };
    let identity = (ok != 0).then(|| {
        (
            info.volume_serial_number as u64,
            ((info.file_index_high as u64) << 32) | info.file_index_low as u64,
        )
    });
    Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
        identity,
    }
}

#[cfg(not(any(unix, windows)))]
fn handle_stamp(_file: &std::fs::File, meta: &std::fs::Metadata) -> Stamp {
    path_stamp(meta)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(any(target_os = "macos", target_os = "ios"))]
const O_NONBLOCK: i32 = 0x0004;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NOFOLLOW: i32 = 0o400000;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android"))
))]
const O_NOFOLLOW: i32 = 0;
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android"))
))]
const O_NONBLOCK: i32 = 0;

#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Open for reading without following a link at the final component.
/// `O_NONBLOCK` keeps a FIFO that replaced the file from blocking the
/// open; the handle check afterwards rejects it.
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(O_NOFOLLOW | O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

enum ProbeOutcome {
    Missing,
    /// Identity and metadata held still before, during and after the read.
    Stable(FileProbe),
    /// Something moved during the read; the probe carries what the handle
    /// saw last so a confirming read can still disagree with it.
    Torn(FileProbe),
}

fn probe_once(path: &Path, max_bytes: u64) -> Result<ProbeOutcome, String> {
    use std::io::{ErrorKind, Read};
    let before_path = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(ProbeOutcome::Missing),
        Err(err) => return Err(err.to_string()),
    };
    if before_path.file_type().is_symlink() || is_reparse_point(&before_path) {
        return Err("symlink or reparse point: never followed".to_string());
    }
    if !before_path.is_file() {
        return Err("not a regular file".to_string());
    }
    let file = match open_no_follow(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(ProbeOutcome::Missing),
        Err(err) => return Err(err.to_string()),
    };
    let before = file.metadata().map_err(|e| e.to_string())?;
    if is_reparse_point(&before) {
        return Err("symlink or reparse point: never followed".to_string());
    }
    if !before.is_file() {
        return Err("not a regular file".to_string());
    }
    if before.len() > max_bytes {
        return Err(format!("file exceeds the {max_bytes}-byte settle limit"));
    }
    let stamp_path = path_stamp(&before_path);
    let stamp_before = handle_stamp(&file, &before);
    let swapped = !stamp_path.agrees(&stamp_before);
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("file exceeds the {max_bytes}-byte settle limit"));
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    let stamp_after = handle_stamp(&file, &after);
    let path_still = match std::fs::symlink_metadata(path) {
        Ok(meta) => !meta.file_type().is_symlink() && path_stamp(&meta).agrees(&stamp_after),
        Err(_) => false,
    };
    let stable = !swapped
        && stamp_before == stamp_after
        && path_still
        && bytes.len() as u64 == stamp_after.len;
    let probe = FileProbe::from_bytes(bytes.into(), stamp_after.modified, stamp_after.identity);
    Ok(if stable {
        ProbeOutcome::Stable(probe)
    } else {
        ProbeOutcome::Torn(probe)
    })
}

/// Read a regular file for settling with a handle-relative, no-follow
/// open, identity and metadata checks before and after the read, and the
/// bytes retained. `Ok(None)` means the path does not exist. A read that
/// saw movement is retried a few times; the last look is returned so the
/// confirming read can still disagree with it.
pub fn probe_file(path: &Path) -> Result<Option<FileProbe>, String> {
    probe_file_limited(path, MAX_PROBE_BYTES)
}

/// [`probe_file`] with an explicit byte limit; larger files are refused.
pub fn probe_file_limited(path: &Path, max_bytes: u64) -> Result<Option<FileProbe>, String> {
    const ATTEMPTS: usize = 3;
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match probe_once(path, max_bytes)? {
            ProbeOutcome::Missing => return Ok(None),
            ProbeOutcome::Stable(probe) => return Ok(Some(probe)),
            ProbeOutcome::Torn(probe) => last = Some(probe),
        }
    }
    Ok(last)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settlement {
    /// The path holds one stable content version that differs from the
    /// last one published for it.
    Updated {
        path: PathBuf,
        probe: FileProbe,
        epoch: u64,
        /// The first and last raw event sequence numbers folded into this
        /// observation.
        seq: (u64, u64),
        /// How many raw events were coalesced.
        events: u32,
        /// The constituent event ids (bounded; `events` is the exact count).
        event_ids: Vec<u64>,
        /// The old name when a rename delivered this content.
        rename_from: Option<PathBuf>,
    },
    /// The path settled on the same bytes it last published: nothing to
    /// re-publish, but the observation is complete and can be acknowledged.
    Unchanged {
        path: PathBuf,
        blob: ObjectId,
        epoch: u64,
        seq: (u64, u64),
        events: u32,
        event_ids: Vec<u64>,
    },
    /// The path is gone and stayed gone through quiescence.
    Removed {
        path: PathBuf,
        epoch: u64,
        seq: (u64, u64),
        event_ids: Vec<u64>,
    },
    /// The path has not held still for `give_up`; probe again after `retry_in`.
    StillChanging {
        path: PathBuf,
        since: Instant,
        retry_in: Duration,
    },
    /// The reader refused the path (not a regular file, permission).
    Unreadable { path: PathBuf, error: String },
    /// A directory-level or rescan event, passed through unsettled.
    Passthrough(FileSystemEvent),
}

#[derive(Clone, Debug)]
enum Stage {
    /// Activity seen; waiting for quiescence before the first read.
    Pending,
    /// One read done; waiting `confirm_gap` for the confirming read.
    Read { first: Option<FileProbe>, at: Instant },
    /// Reported `StillChanging`; probing at the backoff cadence against
    /// the baseline the last probe saw.
    Backoff {
        retry_at: Instant,
        interval: Duration,
        baseline: Option<FileProbe>,
    },
}

#[derive(Clone, Debug)]
struct Entry {
    stage: Stage,
    tombstone: bool,
    first_seq: u64,
    last_seq: u64,
    events: u32,
    event_ids: Vec<u64>,
    rename_from: Option<PathBuf>,
    first_activity: Instant,
    last_event: Instant,
}

impl Entry {
    fn retain_event_id(&mut self, seq: u64) {
        if self.event_ids.len() < MAX_RETAINED_EVENT_IDS {
            self.event_ids.push(seq);
        } else if let Some(last) = self.event_ids.last_mut() {
            *last = seq;
        }
    }
}

/// Settlement is partitioned by (epoch, path): observations from different
/// root-set generations never merge.
type Key = (u64, PathBuf);

#[derive(Default)]
pub struct Settler {
    config: SettleConfig,
    entries: HashMap<Key, Entry>,
    /// The last blob published per path (`Updated`), until it is reported
    /// removed. Drives `Unchanged` and the removal of known paths.
    known: HashMap<PathBuf, ObjectId>,
}

impl Settler {
    pub fn new(config: SettleConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            known: HashMap::new(),
        }
    }

    pub fn config(&self) -> &SettleConfig {
        &self.config
    }

    pub fn pending(&self) -> usize {
        self.entries.len()
    }

    /// The blob last published for `path`, if the path is known.
    pub fn known_blob(&self, path: &Path) -> Option<ObjectId> {
        self.known.get(path).copied()
    }

    /// Seed or correct the published version of a path (from an inventory
    /// the consumer already holds) so an unchanged file settles `Unchanged`.
    pub fn remember(&mut self, path: PathBuf, blob: ObjectId) {
        self.known.insert(path, blob);
    }

    /// Forget a path the consumer no longer tracks: a later settlement is
    /// `Updated` again and a disappearance is silent.
    pub fn forget(&mut self, path: &Path) {
        self.known.remove(path);
    }

    /// Record one raw event. Returns pass-through settlements that need no
    /// settling (rescans, directory events).
    pub fn observe(&mut self, event: &FileSystemEvent) -> Vec<Settlement> {
        match &event.kind {
            FileSystemEventKind::RescanRequired { .. } => {
                return vec![Settlement::Passthrough(event.clone())];
            }
            _ if event.is_dir == Some(true) => {
                return vec![Settlement::Passthrough(event.clone())];
            }
            _ => {}
        }
        let now = event.received_at;
        let mut rename_from = None;
        if let FileSystemEventKind::Renamed { from } = &event.kind {
            // The old name is consumed by the rename: nothing to settle
            // there unless the consumer already knew it.
            let was_pending = self.entries.remove(&(event.epoch, from.clone())).is_some();
            if self.known.contains_key(from) {
                self.touch(from.clone(), event, now, true);
            } else if was_pending {
                // A never-published temporary: gone without a word.
            }
            rename_from = Some(from.clone());
        }
        let tombstone = matches!(event.kind, FileSystemEventKind::Removed);
        self.touch(event.path.clone(), event, now, tombstone);
        if let Some(from) = rename_from {
            if let Some(entry) = self.entries.get_mut(&(event.epoch, event.path.clone())) {
                entry.rename_from = Some(from);
            }
        }
        Vec::new()
    }

    fn touch(&mut self, path: PathBuf, event: &FileSystemEvent, now: Instant, tombstone: bool) {
        let entry = self
            .entries
            .entry((event.epoch, path))
            .or_insert_with(|| Entry {
                stage: Stage::Pending,
                tombstone,
                first_seq: event.seq,
                last_seq: event.seq,
                events: 0,
                event_ids: Vec::new(),
                rename_from: None,
                first_activity: now,
                last_event: now,
            });
        entry.tombstone = tombstone;
        entry.last_seq = entry.last_seq.max(event.seq);
        entry.first_seq = entry.first_seq.min(event.seq);
        entry.events += 1;
        entry.retain_event_id(event.seq);
        entry.last_event = now;
        // New activity restarts confirmation, but keeps the backoff cadence
        // (with its baseline and interval) and the first-activity mark so
        // give_up is measured honestly.
        if !matches!(entry.stage, Stage::Backoff { .. }) {
            entry.stage = Stage::Pending;
        }
    }

    fn give_up_at(&self, entry: &Entry) -> Instant {
        entry.first_activity + self.config.give_up
    }

    fn deadline(&self, entry: &Entry) -> Instant {
        let quiet = entry.last_event + self.config.quiescence;
        match &entry.stage {
            Stage::Pending => quiet.min(self.give_up_at(entry)),
            Stage::Read { at, .. } => (*at + self.config.confirm_gap).max(quiet.min(self.give_up_at(entry))),
            Stage::Backoff { retry_at, .. } => *retry_at,
        }
    }

    /// The earliest time `poll` has something to do, if anything is pending.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.entries.values().map(|entry| self.deadline(entry)).min()
    }

    /// Advance every pending path against `now`, reading through `read`
    /// (`Ok(None)` = the path does not exist).
    pub fn poll(
        &mut self,
        now: Instant,
        read: &mut dyn FnMut(&Path) -> Result<Option<FileProbe>, String>,
    ) -> Vec<Settlement> {
        let mut out = Vec::new();
        // Deterministic order for tests and replay.
        let due: BTreeMap<Key, ()> = self
            .entries
            .iter()
            .filter(|(_, entry)| now >= self.deadline(entry))
            .map(|(key, _)| (key.clone(), ()))
            .collect();
        for (key, _) in due {
            let Some(entry) = self.entries.get_mut(&key) else {
                continue;
            };
            let gave_up = now.saturating_duration_since(entry.first_activity) >= self.config.give_up;
            let current = match read(&key.1) {
                Ok(current) => current,
                Err(error) => {
                    self.entries.remove(&key);
                    out.push(Settlement::Unreadable { path: key.1, error });
                    continue;
                }
            };
            let stage = std::mem::replace(&mut entry.stage, Stage::Pending);
            match stage {
                // First look after quiescence, or the give-up look under
                // continuous activity.
                Stage::Pending => {
                    if gave_up {
                        self.enter_backoff(&key, current, self.config.min_backoff, now, &mut out);
                    } else {
                        entry.stage = Stage::Read {
                            first: current,
                            at: now,
                        };
                    }
                }
                // Confirming look: identical (including "still missing") settles.
                Stage::Read { first, .. } => {
                    if first == current {
                        self.settle(key, current, &mut out);
                    } else if gave_up {
                        self.enter_backoff(&key, current, self.config.min_backoff, now, &mut out);
                    } else {
                        entry.stage = Stage::Read {
                            first: current,
                            at: now,
                        };
                    }
                }
                // Backoff look: the baseline the previous probe saw either
                // held (settle) or moved again (double the interval).
                Stage::Backoff {
                    interval, baseline, ..
                } => {
                    if baseline == current {
                        self.settle(key, current, &mut out);
                    } else {
                        let next = (interval * 2).min(self.config.max_backoff);
                        self.enter_backoff(&key, current, next, now, &mut out);
                    }
                }
            }
        }
        out
    }

    fn enter_backoff(
        &mut self,
        key: &Key,
        baseline: Option<FileProbe>,
        interval: Duration,
        now: Instant,
        out: &mut Vec<Settlement>,
    ) {
        let Some(entry) = self.entries.get_mut(key) else {
            return;
        };
        entry.tombstone = baseline.is_none();
        entry.stage = Stage::Backoff {
            retry_at: now + interval,
            interval,
            baseline,
        };
        out.push(Settlement::StillChanging {
            path: key.1.clone(),
            since: entry.first_activity,
            retry_in: interval,
        });
    }

    fn settle(&mut self, key: Key, current: Option<FileProbe>, out: &mut Vec<Settlement>) {
        let Some(entry) = self.entries.remove(&key) else {
            return;
        };
        let (epoch, path) = key;
        match current {
            Some(probe) => {
                let previous = self.known.insert(path.clone(), probe.blob);
                if previous == Some(probe.blob) && entry.rename_from.is_none() {
                    out.push(Settlement::Unchanged {
                        path,
                        blob: probe.blob,
                        epoch,
                        seq: (entry.first_seq, entry.last_seq),
                        events: entry.events,
                        event_ids: entry.event_ids,
                    });
                } else {
                    out.push(Settlement::Updated {
                        path,
                        probe,
                        epoch,
                        seq: (entry.first_seq, entry.last_seq),
                        events: entry.events,
                        event_ids: entry.event_ids,
                        rename_from: entry.rename_from,
                    });
                }
            }
            None => {
                let was_known = self.known.remove(&path).is_some();
                if was_known || entry.tombstone {
                    out.push(Settlement::Removed {
                        path,
                        epoch,
                        seq: (entry.first_seq, entry.last_seq),
                        event_ids: entry.event_ids,
                    });
                }
                // A never-reported path that vanished before settling was
                // a temporary: nothing to say.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RescanReason;
    use std::collections::VecDeque;

    fn event(path: &str, kind: FileSystemEventKind, seq: u64, at: Instant) -> FileSystemEvent {
        FileSystemEvent {
            mount: "m".into(),
            path: PathBuf::from(path),
            kind,
            seq,
            epoch: 3,
            received_at: at,
            wall_time: SystemTime::UNIX_EPOCH,
            is_dir: Some(false),
        }
    }

    fn probe(hash: u64) -> FileProbe {
        let bytes: Arc<[u8]> = hash.to_le_bytes().to_vec().into();
        FileProbe::from_bytes(bytes, None, Some((1, 42)))
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A scripted reader: pops the next scripted answer per call.
    fn scripted(answers: Vec<Result<Option<FileProbe>, String>>) -> impl FnMut(&Path) -> Result<Option<FileProbe>, String> {
        let mut queue: VecDeque<_> = answers.into();
        move |_| queue.pop_front().unwrap_or(Ok(None))
    }

    #[test]
    fn a_burst_of_writes_settles_into_one_update() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        for i in 0..10u64 {
            let at = t0 + ms(30 * i);
            assert!(settler
                .observe(&event("/w/a.rs", FileSystemEventKind::Changed, 100 + i, at))
                .is_empty());
        }
        let last = t0 + ms(270);
        assert_eq!(settler.next_deadline(), Some(last + ms(150)));
        let mut read = scripted(vec![Ok(Some(probe(7))), Ok(Some(probe(7)))]);
        // Before quiescence nothing happens.
        assert!(settler.poll(last + ms(100), &mut read).is_empty());
        // First read at quiescence, no settlement yet.
        assert!(settler.poll(last + ms(150), &mut read).is_empty());
        // Second read too early (< confirm_gap) does nothing.
        assert!(settler.poll(last + ms(200), &mut read).is_empty());
        let out = settler.poll(last + ms(260), &mut read);
        assert_eq!(
            out,
            vec![Settlement::Updated {
                path: PathBuf::from("/w/a.rs"),
                probe: probe(7),
                epoch: 3,
                seq: (100, 109),
                events: 10,
                event_ids: (100..110).collect(),
                rename_from: None,
            }]
        );
        assert_eq!(settler.pending(), 0);
        assert_eq!(settler.next_deadline(), None);
        assert_eq!(settler.known_blob(Path::new("/w/a.rs")), Some(probe(7).blob));
    }

    #[test]
    fn changing_content_reports_still_changing_with_capped_backoff() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        settler.observe(&event("/w/b.rs", FileSystemEventKind::Changed, 1, t0));
        let mut hash = 0u64;
        let mut read = move |_: &Path| {
            hash += 1;
            Ok(Some(probe(hash)))
        };
        let mut now = t0 + ms(150);
        let mut reports = Vec::new();
        for _ in 0..40 {
            for s in settler.poll(now, &mut read) {
                reports.push(s);
            }
            now += ms(100);
        }
        let intervals: Vec<Duration> = reports
            .iter()
            .filter_map(|s| match s {
                Settlement::StillChanging { retry_in, since, .. } => {
                    assert_eq!(*since, t0);
                    Some(*retry_in)
                }
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert!(!intervals.is_empty(), "no StillChanging after give_up");
        assert_eq!(intervals[0], ms(250));
        // The retry state is retained across probes: doubling, then capped.
        assert_eq!(intervals, vec![ms(250), ms(500), ms(1000), ms(2000)]);
        assert!(reports.iter().all(|s| !matches!(s, Settlement::Updated { .. })));
        // Once the content holds still it settles with the latest probe.
        let stable = probe(999);
        let mut steady = scripted(vec![Ok(Some(stable.clone())); 8]);
        let mut settled = None;
        for _ in 0..60 {
            for s in settler.poll(now, &mut steady) {
                if let Settlement::Updated { probe, .. } = s {
                    settled = Some(probe);
                }
            }
            now += ms(100);
        }
        assert_eq!(settled, Some(stable));
    }

    #[test]
    fn continuous_notifications_still_yield_still_changing_at_give_up() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        let mut reads = 0u32;
        let mut read = |_: &Path| {
            reads += 1;
            Ok(Some(probe(reads as u64)))
        };
        let mut reports = Vec::new();
        // An event every 50 ms, forever: quiescence is never reached.
        let mut now = t0;
        let mut seq = 1;
        while now < t0 + ms(3000) {
            settler.observe(&event("/w/hot.rs", FileSystemEventKind::Changed, seq, now));
            seq += 1;
            reports.extend(settler.poll(now, &mut read));
            now += ms(50);
        }
        let first_still = reports.iter().position(|s| matches!(s, Settlement::StillChanging { .. }));
        let first_still = first_still.expect("StillChanging must be reported despite the event stream");
        assert!(matches!(&reports[first_still], Settlement::StillChanging { retry_in, .. } if *retry_in == ms(250)));
        // It was reported at give_up (2 s after first activity), not later.
        let stills = reports
            .iter()
            .filter(|s| matches!(s, Settlement::StillChanging { .. }))
            .count();
        assert!(stills >= 2, "backoff probes keep running under the stream: {reports:?}");
        assert!(reads >= 2);
        // The give-up deadline is visible before quiescence would be.
        let mut fresh = Settler::new(SettleConfig::default());
        fresh.observe(&event("/w/x.rs", FileSystemEventKind::Changed, 1, t0));
        fresh.observe(&event("/w/x.rs", FileSystemEventKind::Changed, 2, t0 + ms(1990)));
        assert_eq!(fresh.next_deadline(), Some(t0 + ms(2000)));
    }

    #[test]
    fn delete_tombstone_is_held_and_recreate_is_one_update() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        // Known to the consumer first.
        settler.observe(&event("/w/c.rs", FileSystemEventKind::Created, 1, t0));
        let mut read = scripted(vec![Ok(Some(probe(1))), Ok(Some(probe(1)))]);
        settler.poll(t0 + ms(150), &mut read);
        let first = settler.poll(t0 + ms(260), &mut read);
        assert!(matches!(first[0], Settlement::Updated { .. }));

        // Delete then recreate inside the quiescence window: one update.
        let t1 = t0 + ms(1000);
        settler.observe(&event("/w/c.rs", FileSystemEventKind::Removed, 2, t1));
        settler.observe(&event("/w/c.rs", FileSystemEventKind::Created, 3, t1 + ms(50)));
        let mut read = scripted(vec![Ok(Some(probe(2))), Ok(Some(probe(2)))]);
        assert!(settler.poll(t1 + ms(150), &mut read).is_empty());
        settler.poll(t1 + ms(200), &mut read);
        let out = settler.poll(t1 + ms(310), &mut read);
        assert!(
            matches!(&out[0], Settlement::Updated { seq: (2, 3), events: 2, event_ids, .. } if event_ids == &[2, 3]),
            "{out:?}"
        );

        // Delete that stays deleted: Removed after quiescence, and only then.
        let t2 = t0 + ms(3000);
        settler.observe(&event("/w/c.rs", FileSystemEventKind::Removed, 4, t2));
        let mut gone = scripted(vec![Ok(None), Ok(None)]);
        assert!(settler.poll(t2 + ms(100), &mut gone).is_empty());
        settler.poll(t2 + ms(150), &mut gone);
        let out = settler.poll(t2 + ms(260), &mut gone);
        assert_eq!(
            out,
            vec![Settlement::Removed {
                path: PathBuf::from("/w/c.rs"),
                epoch: 3,
                seq: (4, 4),
                event_ids: vec![4],
            }]
        );
        assert_eq!(settler.known_blob(Path::new("/w/c.rs")), None);
    }

    #[test]
    fn unchanged_resettlement_is_acknowledged_not_republished() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        settler.observe(&event("/w/same.rs", FileSystemEventKind::Changed, 1, t0));
        let mut read = scripted(vec![Ok(Some(probe(5))), Ok(Some(probe(5)))]);
        settler.poll(t0 + ms(150), &mut read);
        assert!(matches!(settler.poll(t0 + ms(260), &mut read)[0], Settlement::Updated { .. }));
        // A touch: metadata differs, bytes do not.
        let t1 = t0 + ms(1000);
        settler.observe(&event("/w/same.rs", FileSystemEventKind::Changed, 2, t1));
        let mut touched = probe(5);
        touched.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5));
        let mut read = scripted(vec![Ok(Some(touched.clone())), Ok(Some(touched))]);
        settler.poll(t1 + ms(150), &mut read);
        let out = settler.poll(t1 + ms(260), &mut read);
        assert_eq!(
            out,
            vec![Settlement::Unchanged {
                path: PathBuf::from("/w/same.rs"),
                blob: probe(5).blob,
                epoch: 3,
                seq: (2, 2),
                events: 1,
                event_ids: vec![2],
            }]
        );
        // A consumer-seeded version behaves the same way.
        settler.remember(PathBuf::from("/w/seeded.rs"), probe(8).blob);
        settler.observe(&event("/w/seeded.rs", FileSystemEventKind::Changed, 3, t1));
        let mut read = scripted(vec![Ok(Some(probe(8))), Ok(Some(probe(8)))]);
        settler.poll(t1 + ms(150), &mut read);
        assert!(matches!(settler.poll(t1 + ms(260), &mut read)[0], Settlement::Unchanged { .. }));
        settler.forget(Path::new("/w/seeded.rs"));
        assert_eq!(settler.known_blob(Path::new("/w/seeded.rs")), None);
    }

    #[test]
    fn atomic_replacement_is_one_update_and_the_temporary_is_silent() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        settler.observe(&event("/w/.d.rs.tmp", FileSystemEventKind::Created, 1, t0));
        settler.observe(&event("/w/.d.rs.tmp", FileSystemEventKind::Changed, 2, t0 + ms(20)));
        settler.observe(&event(
            "/w/d.rs",
            FileSystemEventKind::Renamed {
                from: PathBuf::from("/w/.d.rs.tmp"),
            },
            3,
            t0 + ms(40),
        ));
        assert_eq!(settler.pending(), 1, "temporary consumed by the rename");
        let mut read = scripted(vec![Ok(Some(probe(5))), Ok(Some(probe(5)))]);
        settler.poll(t0 + ms(190), &mut read);
        let out = settler.poll(t0 + ms(300), &mut read);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0], Settlement::Updated { path, seq: (3, 3), rename_from: Some(from), .. }
                if path == Path::new("/w/d.rs") && from == Path::new("/w/.d.rs.tmp")),
            "{out:?}"
        );
    }

    #[test]
    fn epochs_partition_settlement() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        settler.observe(&event("/w/e.rs", FileSystemEventKind::Changed, 1, t0));
        let mut later = event("/w/e.rs", FileSystemEventKind::Changed, 2, t0 + ms(10));
        later.epoch = 4;
        settler.observe(&later);
        assert_eq!(settler.pending(), 2, "one entry per (epoch, path)");
        let mut read = scripted(vec![Ok(Some(probe(1))); 4]);
        settler.poll(t0 + ms(160), &mut read);
        let out = settler.poll(t0 + ms(270), &mut read);
        let epochs: Vec<u64> = out
            .iter()
            .map(|s| match s {
                Settlement::Updated { epoch, .. } | Settlement::Unchanged { epoch, .. } => *epoch,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(epochs, vec![3, 4]);
        assert!(matches!(out[0], Settlement::Updated { .. }));
        assert!(matches!(out[1], Settlement::Unchanged { .. }), "{out:?}");
    }

    #[test]
    fn identity_or_metadata_mismatch_between_reads_does_not_settle() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        settler.observe(&event("/w/e.rs", FileSystemEventKind::Changed, 1, t0));
        let mut moved = probe(9);
        moved.identity = Some((1, 43));
        let mut read = scripted(vec![
            Ok(Some(probe(9))),
            Ok(Some(moved)),
            Ok(Some(probe(9))),
            Ok(Some(probe(9))),
        ]);
        settler.poll(t0 + ms(150), &mut read);
        assert!(settler.poll(t0 + ms(260), &mut read).is_empty(), "inode changed: not settled");
        settler.poll(t0 + ms(370), &mut read);
        let out = settler.poll(t0 + ms(480), &mut read);
        assert!(matches!(out[0], Settlement::Updated { .. }));
    }

    #[test]
    fn fnv_collisions_do_not_settle_different_bytes() {
        let a = FileProbe::from_bytes(vec![1, 2, 3].into(), None, None);
        let mut b = FileProbe::from_bytes(vec![4, 5, 6].into(), None, None);
        // Force the preliminary fingerprint and the length to collide.
        b.content_hash = a.content_hash;
        b.len = a.len;
        assert_ne!(a, b, "blob id and bytes decide, not the fingerprint");
        assert_ne!(a.blob, b.blob);
    }

    #[test]
    fn rescans_and_directories_pass_through_and_unreadable_is_reported() {
        let t0 = Instant::now();
        let mut settler = Settler::new(SettleConfig::default());
        let rescan = FileSystemEvent {
            kind: FileSystemEventKind::RescanRequired {
                root: PathBuf::from("/w"),
                reason: RescanReason::Overflow,
                missing: None,
            },
            ..event("/w", FileSystemEventKind::Changed, 1, t0)
        };
        assert_eq!(settler.observe(&rescan), vec![Settlement::Passthrough(rescan.clone())]);
        let dir = FileSystemEvent {
            is_dir: Some(true),
            ..event("/w/sub", FileSystemEventKind::Created, 2, t0)
        };
        assert_eq!(settler.observe(&dir), vec![Settlement::Passthrough(dir.clone())]);
        settler.observe(&event("/w/fifo", FileSystemEventKind::Created, 3, t0));
        let mut read = scripted(vec![Err("not a regular file".into())]);
        let out = settler.poll(t0 + ms(150), &mut read);
        assert_eq!(
            out,
            vec![Settlement::Unreadable {
                path: PathBuf::from("/w/fifo"),
                error: "not a regular file".into()
            }]
        );
        assert_eq!(settler.pending(), 0);
    }

    #[test]
    fn probe_file_retains_bytes_with_blob_identity_and_refuses_links() {
        let dir = std::env::temp_dir().join(format!(
            "fswatch-probe-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.rs");
        std::fs::write(&file, b"fn f() {}\n").unwrap();
        let probe = probe_file(&file).unwrap().expect("exists");
        assert_eq!(&*probe.bytes, b"fn f() {}\n");
        assert_eq!(probe.len, 10);
        assert_eq!(probe.blob, hash_object("blob", b"fn f() {}\n"));
        assert!(probe.modified.is_some());
        #[cfg(any(unix, windows))]
        assert!(probe.identity.is_some());
        assert_eq!(probe_file(&dir.join("missing.rs")).unwrap(), None);
        assert!(probe_file(&dir).is_err(), "directories are not regular files");
        assert!(probe_file_limited(&file, 4).is_err(), "over the byte limit");
        #[cfg(unix)]
        {
            let link = dir.join("link.rs");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            let error = probe_file(&link).unwrap_err();
            assert!(error.contains("never followed"), "{error}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
