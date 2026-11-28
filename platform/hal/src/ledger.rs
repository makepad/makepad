//! The GPU ledger: per session, per handle bytes, when it was created and last used, and who
//! owns it. Report semantics ported from platform/src/gpu_ledger.rs + gpu_inventory.rs: bytes are
//! the backend's; owner = explicit label, else the owner scope, else the creation site; groups
//! sort by bytes; "idle" = a texture-like handle not used for N frames, or never.
//! Reports are plain data (no references into the tables), so any thread can analyse one.

use crate::table::DENSE_MAX;
use crate::types::{Attr, StrId};
use crate::FastMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// The object kinds on a ledger: the four with tables, and the backend's own allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Texture,
    Buffer,
    Pipeline,
    Window,
    /// a backend pool (`Pool`): one row each on the host session's ledger
    Backend,
}

impl Kind {
    /// The kinds with tables (`World::count`).
    pub const ALL: [Kind; 4] = [Kind::Texture, Kind::Buffer, Kind::Pipeline, Kind::Window];

    #[inline]
    fn table_slot(self) -> Option<usize> {
        match self {
            Kind::Texture | Kind::Buffer | Kind::Pipeline | Kind::Window => Some(self as usize),
            Kind::Backend => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Texture => "texture",
            Kind::Buffer => "buffer",
            Kind::Pipeline => "pipeline",
            Kind::Window => "window",
            Kind::Backend => "backend",
        }
    }
}

/// A row whose bytes the graphics API does not report (pipelines, timestamp buffers, command
/// buffers): its bytes are 0 and the row says why.
pub const NO_SIZE: &str = "the API reports no size";

/// What a backend allocates for itself, outside any session's handles. Each pool is one row on
/// the host session's ledger (`Ledger::set_pools`) with its bytes and objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pool {
    /// the arenas frames upload instance data, uniforms and texture data through
    UploadArena,
    /// readback staging (grabs, snapshots, buffer reads)
    Staging,
    /// the blocks compute readback leases are carved from
    Leases,
    /// GPU timestamp sample buffers or query pools
    Timestamps,
    /// command buffers: Vulkan's slots in flight or kept for reuse (each with its command and
    /// timestamp pools); on Metal the most made and not completed at once, which
    /// the queue keeps driver memory for
    CommandBuffers,
    /// the descriptor pools Vulkan's kept descriptor sets come from: the one new sets come from
    /// and older ones batches in flight still hold
    DescriptorPools,
    /// the 1x1 texture bound in an empty slot
    Fallback,
    /// the multisampled images MSAA passes render into
    Multisample,
    /// window images besides the window's own: composed and badged frames, overlays, spares
    WindowCopies,
    /// window capture buffers
    Capture,
    /// copies of instance buffers as textures, for programs that fetch instances by id (GL)
    WordTextures,
    /// the compiled pipelines of an app session that closed, waiting for the next session to take
    /// them (a restart in place, Metal)
    KeptPipelines,
    /// the room in the memory blocks retained buffers take ranges of (Vulkan) that no buffer
    /// holds: the blocks' bytes beyond the handles' own
    BufferBlocks,
}

impl Pool {
    pub const ALL: [Pool; 13] = [
        Pool::UploadArena,
        Pool::Staging,
        Pool::Leases,
        Pool::Timestamps,
        Pool::CommandBuffers,
        Pool::DescriptorPools,
        Pool::Fallback,
        Pool::Multisample,
        Pool::WindowCopies,
        Pool::Capture,
        Pool::WordTextures,
        Pool::KeptPipelines,
        Pool::BufferBlocks,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Pool::UploadArena => "upload arena",
            Pool::Staging => "readback staging",
            Pool::Leases => "compute leases",
            Pool::Timestamps => "timestamps",
            Pool::CommandBuffers => "command buffers",
            Pool::DescriptorPools => "descriptor pools",
            Pool::Fallback => "fallback texture",
            Pool::Multisample => "multisample",
            Pool::WindowCopies => "window copies",
            Pool::Capture => "capture",
            Pool::WordTextures => "instance word textures",
            Pool::KeptPipelines => "kept pipelines",
            Pool::BufferBlocks => "buffer block room",
        }
    }
}

/// A backend's own allocations by pool, charged and uncharged from any thread (`Charge`).
#[derive(Default, Debug)]
pub struct Pools {
    bytes: [AtomicU64; Pool::ALL.len()],
    objects: [AtomicU64; Pool::ALL.len()],
    /// pools whose objects the API gives no size for (`NO_SIZE`)
    no_size: [AtomicBool; Pool::ALL.len()],
}

impl Pools {
    /// One object of `bytes` in `pool`, held until the `Charge` drops.
    pub fn charge(self: &Arc<Self>, pool: Pool, bytes: u64) -> Charge {
        self.bytes[pool as usize].fetch_add(bytes, Ordering::Relaxed);
        self.objects[pool as usize].fetch_add(1, Ordering::Relaxed);
        Charge { pools: self.clone(), pool, bytes }
    }

    /// One object of `pool` whose size the API does not report.
    pub fn charge_unsized(self: &Arc<Self>, pool: Pool) -> Charge {
        self.no_size[pool as usize].store(true, Ordering::Relaxed);
        self.charge(pool, 0)
    }

    pub fn bytes(&self, pool: Pool) -> u64 {
        self.bytes[pool as usize].load(Ordering::Relaxed)
    }

    pub fn objects(&self, pool: Pool) -> u64 {
        self.objects[pool as usize].load(Ordering::Relaxed)
    }

    /// A pool the backend measures itself: its bytes and objects now.
    pub fn set(&self, pool: Pool, bytes: u64, objects: u64) {
        self.bytes[pool as usize].store(bytes, Ordering::Relaxed);
        self.objects[pool as usize].store(objects, Ordering::Relaxed);
    }

    /// A pool the backend counts itself (objects whose size the API does not report).
    pub fn set_count(&self, pool: Pool, objects: u64) {
        self.no_size[pool as usize].store(true, Ordering::Relaxed);
        self.objects[pool as usize].store(objects, Ordering::Relaxed);
    }
}

/// One object's share of a pool, given back when it drops.
#[derive(Debug)]
pub struct Charge {
    pools: Arc<Pools>,
    pool: Pool,
    bytes: u64,
}

impl Charge {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.pools.bytes[self.pool as usize].fetch_sub(self.bytes, Ordering::Relaxed);
        self.pools.objects[self.pool as usize].fetch_sub(1, Ordering::Relaxed);
    }
}

/// What the ledger records for one handle. `detail` is the report's kind label
/// ("render target", "depth", "instance buffers", ...), chosen by the executor at create time.
#[derive(Clone, Copy, Debug)]
pub struct LedgerEntry {
    pub kind: Kind,
    pub detail: &'static str,
    pub index: u32,
    pub gen: u32,
    pub bytes: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub attr: Attr,
    /// HAL clock ns
    pub created_ns: u64,
    pub created_frame: u64,
    /// 0 = never used
    pub last_used_frame: u64,
    pub last_used_ns: u64,
    /// objects behind the row: 1 for a handle, the pool's count for a backend row
    pub objects: u64,
    /// why `bytes` is not the API's size (`NO_SIZE`), or ""
    pub note: &'static str,
}

/// Only this stamp is read or written when a draw touches a dense handle.
#[derive(Clone, Copy, Debug, Default)]
struct TouchSlot {
    gen: Option<u32>,
    last_used_frame: u64,
    last_used_ns: u64,
}

impl TouchSlot {
    fn merge(&self, entry: LedgerEntry) -> LedgerEntry {
        LedgerEntry { last_used_frame: self.last_used_frame, last_used_ns: self.last_used_ns, ..entry }
    }
}

enum LedgerSlot {
    Dense(usize, usize),
    Backend(usize),
    Overflow,
}

#[inline]
fn ledger_slot(kind: Kind, index: u32) -> LedgerSlot {
    match kind.table_slot() {
        Some(k) if index < DENSE_MAX => LedgerSlot::Dense(k, index as usize),
        None if (index as usize) < Pool::ALL.len() => LedgerSlot::Backend(index as usize),
        _ => LedgerSlot::Overflow,
    }
}

#[derive(Default, Debug)]
pub struct Ledger {
    touch: [Vec<TouchSlot>; Kind::ALL.len()],
    meta: [Vec<Option<LedgerEntry>>; Kind::ALL.len()],
    backend: [Option<LedgerEntry>; Pool::ALL.len()],
    overflow: FastMap<(Kind, u32), LedgerEntry>,
    len: usize,
    /// every create / destroy ever seen, per site (churn)
    sites: FastMap<StrId, SiteCounts>,
    /// What the device says this process holds (set by backends that can ask: Metal's
    /// `currentAllocatedSize`, Vulkan's memory budget usage): the ledger's total reconciles
    /// against it.
    pub device_allocated: Option<u64>,
    /// What the OS charges the process for graphics memory (macOS: the task's graphics
    /// footprint, which also holds what the driver allocates outside `device_allocated`).
    pub os_graphics: Option<u64>,
}

#[derive(Default, Clone, Copy, Debug)]
struct SiteCounts {
    handles: u64,
    bytes: u64,
}

impl Ledger {
    pub fn add(&mut self, e: LedgerEntry) {
        let s = self.sites.entry(e.attr.site).or_default();
        s.handles += 1;
        s.bytes += e.bytes;
        let old = match ledger_slot(e.kind, e.index) {
            LedgerSlot::Dense(k, i) => {
                if i >= self.meta[k].len() {
                    self.meta[k].resize(i + 1, None);
                    self.touch[k].resize(i + 1, TouchSlot::default());
                }
                self.touch[k][i] = TouchSlot {
                    gen: Some(e.gen), last_used_frame: e.last_used_frame, last_used_ns: e.last_used_ns,
                };
                self.meta[k][i].replace(e)
            }
            LedgerSlot::Backend(i) => self.backend[i].replace(e),
            LedgerSlot::Overflow => self.overflow.insert((e.kind, e.index), e),
        };
        if old.is_none() {
            self.len += 1;
        }
    }

    pub fn remove(&mut self, kind: Kind, index: u32) {
        let old = match ledger_slot(kind, index) {
            LedgerSlot::Dense(k, i) => {
                let Some(slot) = self.touch[k].get_mut(i) else { return };
                slot.gen = None;
                self.meta[k][i].take()
            }
            LedgerSlot::Backend(i) => self.backend[i].take(),
            LedgerSlot::Overflow => self.overflow.remove(&(kind, index)),
        };
        if old.is_some() {
            self.len -= 1;
        }
    }

    /// Marks the handle used by `frame` (a draw sampled / drew / rendered to it).
    #[inline]
    pub fn touch(&mut self, kind: Kind, index: u32, gen: u32, frame: u64, now_ns: u64) {
        let entry = match ledger_slot(kind, index) {
            LedgerSlot::Dense(k, i) => {
                if let Some(slot) = self.touch[k].get_mut(i) {
                    if slot.gen == Some(gen) {
                        slot.last_used_frame = frame;
                        slot.last_used_ns = now_ns;
                    }
                }
                return;
            }
            LedgerSlot::Backend(i) => self.backend[i].as_mut(),
            LedgerSlot::Overflow => self.overflow.get_mut(&(kind, index)),
        };
        if let Some(e) = entry.filter(|e| e.gen == gen) {
            e.last_used_frame = frame;
            e.last_used_ns = now_ns;
        }
    }

    pub fn get(&self, kind: Kind, index: u32) -> Option<LedgerEntry> {
        match ledger_slot(kind, index) {
            LedgerSlot::Dense(k, i) => self.meta[k].get(i)?.map(|e| self.touch[k][i].merge(e)),
            LedgerSlot::Backend(i) => self.backend[i],
            LedgerSlot::Overflow => self.overflow.get(&(kind, index)).copied(),
        }
    }

    /// Returns the cold entry. Dense handles keep their current `last_used_frame` and
    /// `last_used_ns` in the touch table. Use `get` to read them and `touch` to update them.
    pub fn get_mut(&mut self, kind: Kind, index: u32) -> Option<&mut LedgerEntry> {
        match ledger_slot(kind, index) {
            LedgerSlot::Dense(k, i) => self.meta[k].get_mut(i)?.as_mut(),
            LedgerSlot::Backend(i) => self.backend[i].as_mut(),
            LedgerSlot::Overflow => self.overflow.get_mut(&(kind, index)),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn total_bytes(&self) -> u64 {
        self.meta.iter().flatten().flatten()
            .chain(self.backend.iter().flatten())
            .chain(self.overflow.values())
            .map(|e| e.bytes)
            .sum()
    }

    pub fn entries(&self) -> impl Iterator<Item = LedgerEntry> + '_ {
        self.meta.iter().zip(&self.touch)
            .flat_map(|(meta, touch)| meta.iter().zip(touch))
            .filter_map(|(entry, stamp)| entry.map(|e| stamp.merge(e)))
            .chain(self.backend.iter().flatten().copied())
            .chain(self.overflow.values().copied())
    }

    /// The backend's pool rows (`Kind::Backend`), one per pool with objects in it.
    pub fn set_pools(&mut self, pools: &Pools, frame: u64, now_ns: u64) {
        for pool in Pool::ALL {
            let (bytes, objects) = (pools.bytes(pool), pools.objects(pool));
            let index = pool as u32;
            if objects == 0 {
                self.remove(Kind::Backend, index);
                continue;
            }
            let note = if pools.no_size[pool as usize].load(Ordering::Relaxed) { NO_SIZE } else { "" };
            match &mut self.backend[pool as usize] {
                Some(e) => {
                    e.bytes = bytes;
                    e.objects = objects;
                    e.note = note;
                    e.last_used_frame = frame;
                    e.last_used_ns = now_ns;
                }
                slot @ None => {
                    *slot = Some(LedgerEntry {
                        kind: Kind::Backend,
                        detail: pool.name(),
                        index,
                        gen: 0,
                        bytes,
                        format: 0,
                        width: 0,
                        height: 0,
                        mips: 0,
                        attr: Attr::default(),
                        created_ns: now_ns,
                        created_frame: frame,
                        last_used_frame: frame,
                        last_used_ns: now_ns,
                        objects,
                        note,
                    });
                    self.len += 1;
                }
            }
        }
    }

    /// The report of this session. `strings` resolves the attrs; `frame` is the current frame;
    /// a texture-like handle unused for `idle_frames` frames (or never) is idle.
    pub fn report(&self, session: u32, strings: &FastMap<StrId, String>, frame: u64, idle_frames: u64) -> Report {
        let name = |id: StrId| strings.get(&id).cloned().filter(|_| id != 0);
        let mut items: Vec<ReportItem> = self
            .entries()
            .map(|e| {
                let site = name(e.attr.site).unwrap_or_default();
                let owner = name(e.attr.label)
                    .or_else(|| name(e.attr.owner))
                    .or_else(|| (!site.is_empty()).then(|| site.clone()))
                    .unwrap_or_else(|| match e.kind {
                        Kind::Window => e.detail.to_string(),
                        Kind::Backend => "hal".to_string(),
                        _ => "unknown".to_string(),
                    });
                ReportItem {
                    session,
                    kind: e.kind,
                    detail: e.detail,
                    index: e.index,
                    gen: e.gen,
                    bytes: e.bytes,
                    objects: e.objects,
                    note: e.note,
                    owner,
                    site,
                    created_frame: e.created_frame,
                    last_used_frame: e.last_used_frame,
                    idle_frames: (e.last_used_frame != 0).then(|| frame.saturating_sub(e.last_used_frame)),
                }
            })
            .collect();
        items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.kind.cmp(&b.kind)).then(a.index.cmp(&b.index)));
        let idle = items.iter().filter(|i| matches!(i.kind, Kind::Texture) && i.idle_frames.map_or(true, |f| f >= idle_frames)).cloned().collect();
        let total_bytes = items.iter().map(|i| i.bytes).sum();
        Report {
            session,
            frame,
            total_bytes,
            device_allocated: self.device_allocated,
            os_graphics: self.os_graphics,
            by_session: vec![SessionTotal { session, frame, bytes: total_bytes, count: items.len() }],
            by_kind: group(&items, |i| i.detail.to_string()),
            by_owner: group(&items, |i| i.owner.clone()),
            by_site: group(&items, |i| i.site.clone()),
            churn: {
                let mut v: Vec<(String, u64, u64)> = self.sites.iter().map(|(s, c)| (name(*s).unwrap_or_default(), c.handles, c.bytes)).collect();
                v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
                v
            },
            idle,
            items,
        }
    }
}

fn group(items: &[ReportItem], key: impl Fn(&ReportItem) -> String) -> Vec<Group> {
    let mut m: FastMap<String, (u64, usize, Option<&'static str>)> = FastMap::default();
    for i in items {
        let e = m.entry(key(i)).or_default();
        e.0 += i.bytes;
        e.1 += i.objects as usize;
        // a note every item of the group shares
        e.2 = Some(match e.2 {
            None => i.note,
            Some(n) if n == i.note => n,
            Some(_) => "",
        });
    }
    let mut v: Vec<Group> = m.into_iter().map(|(key, (bytes, count, note))| Group { key, bytes, count, note: note.unwrap_or("") }).collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.key.cmp(&b.key)));
    v
}

/// One handle in a report.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportItem {
    pub session: u32,
    pub kind: Kind,
    pub detail: &'static str,
    pub index: u32,
    pub gen: u32,
    pub bytes: u64,
    /// objects behind the row (a backend pool's count)
    pub objects: u64,
    /// why `bytes` is not the API's size, or ""
    pub note: &'static str,
    pub owner: String,
    pub site: String,
    pub created_frame: u64,
    pub last_used_frame: u64,
    /// frames since last use; `None` = never used
    pub idle_frames: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub key: String,
    pub bytes: u64,
    /// objects in the group
    pub count: usize,
    /// a note all its items share (`NO_SIZE`), or ""
    pub note: &'static str,
}

/// A session's totals, retained in the process report even when it has no allocations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTotal {
    pub session: u32,
    pub frame: u64,
    pub bytes: u64,
    pub count: usize,
}

/// A session's ledger as plain data: totals by kind / owner / site, the idle handles, churn
/// per creation site (site, handles created, bytes created).
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub session: u32,
    pub frame: u64,
    pub total_bytes: u64,
    /// the device's own number, when the backend has one
    pub device_allocated: Option<u64>,
    /// the OS's graphics memory charge for the process, where it has one (`Ledger::os_graphics`)
    pub os_graphics: Option<u64>,
    pub by_session: Vec<SessionTotal>,
    pub by_kind: Vec<Group>,
    pub by_owner: Vec<Group>,
    pub by_site: Vec<Group>,
    pub churn: Vec<(String, u64, u64)>,
    pub idle: Vec<ReportItem>,
    pub items: Vec<ReportItem>,
}

impl Report {
    /// Adds another session's report to this one (the process-wide view).
    pub fn merge(&mut self, o: Report) {
        self.total_bytes += o.total_bytes;
        // Sessions share the HAL's device: its allocation total must only be counted once.
        self.device_allocated = self.device_allocated.or(o.device_allocated);
        self.os_graphics = self.os_graphics.or(o.os_graphics);
        self.by_session.extend(o.by_session);
        self.items.extend(o.items);
        self.idle.extend(o.idle);
        self.items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.session.cmp(&b.session)).then(a.index.cmp(&b.index)));
        self.by_kind = group(&self.items, |i| i.detail.to_string());
        self.by_owner = group(&self.items, |i| i.owner.clone());
        self.by_site = group(&self.items, |i| i.site.clone());
        self.churn.extend(o.churn);
    }
}

#[cfg(test)]
#[test]
fn ledger_inline_touch_syncs_report_and_ignores_stale_gen() {
    let mut ledger = Ledger::default();
    let texture = LedgerEntry {
        kind: Kind::Texture, detail: "texture", index: 5, gen: 2, bytes: 4,
        format: 0, width: 1, height: 1, mips: 1, attr: Attr::default(),
        created_ns: 10, created_frame: 1, last_used_frame: 2, last_used_ns: 20, objects: 1, note: "",
    };
    let rows = [
        texture,
        LedgerEntry { index: crate::types::LENT_BASE + 1, gen: 1, ..texture },
        LedgerEntry { kind: Kind::Window, detail: "window", gen: 0, ..texture },
    ];
    ledger.add(LedgerEntry { gen: 1, last_used_frame: 1, last_used_ns: 10, ..texture });
    for row in rows {
        ledger.add(row);
        ledger.touch(row.kind, row.index, row.gen.wrapping_sub(1), 75, 8_000);
        let entry = ledger.get(row.kind, row.index).unwrap();
        assert_eq!((entry.last_used_frame, entry.last_used_ns), (2, 20), "stale touch changed {row:?}");
        ledger.touch(row.kind, row.index, row.gen, 99, 12_345);
    }
    assert_eq!(ledger.len(), rows.len(), "replacing a generation added a row");

    let report = ledger.report(1, &FastMap::default(), 103, 5);
    assert_eq!(report.items.len(), rows.len());
    assert!(report.idle.is_empty(), "the touched textures are only four frames idle");
    for row in rows {
        let item = report.items.iter().find(|i| (i.kind, i.index) == (row.kind, row.index)).unwrap();
        assert_eq!((item.last_used_frame, item.idle_frames), (99, Some(4)), "report for {row:?}");
        let entry = ledger.get(row.kind, row.index).unwrap();
        assert_eq!((entry.last_used_frame, entry.last_used_ns), (99, 12_345), "get for {row:?}");
        let entry = ledger.entries().find(|e| (e.kind, e.index) == (row.kind, row.index)).unwrap();
        assert_eq!((entry.last_used_frame, entry.last_used_ns), (99, 12_345), "entries for {row:?}");
    }
    for row in rows {
        ledger.remove(row.kind, row.index);
        ledger.touch(row.kind, row.index, row.gen, 100, 13_000);
        assert!(ledger.get(row.kind, row.index).is_none(), "touch revived {row:?}");
    }
    assert_eq!(ledger.len(), 0);
    assert_eq!(ledger.entries().count(), 0);

    let pools = Pools::default();
    pools.set(Pool::Staging, 8, 1);
    ledger.set_pools(&pools, 101, 14_000);
    pools.set(Pool::Staging, 16, 2);
    ledger.set_pools(&pools, 102, 15_000);
    assert_eq!((ledger.len(), ledger.total_bytes()), (1, 16), "pool refresh replaces its row");
    let pool = ledger.get(Kind::Backend, Pool::Staging as u32).unwrap();
    assert_eq!((pool.objects, pool.created_frame, pool.last_used_ns), (2, 101, 15_000));
    pools.set(Pool::Staging, 0, 0);
    ledger.set_pools(&pools, 103, 16_000);
    assert!(ledger.is_empty(), "empty pool still counted");
}
