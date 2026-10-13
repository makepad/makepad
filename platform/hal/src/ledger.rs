//! The GPU ledger: per session, per handle bytes, when it was created and last used, and who
//! owns it. Report semantics ported from platform/src/gpu_ledger.rs + gpu_inventory.rs: bytes are
//! the backend's; owner = explicit label, else the owner scope, else the creation site; groups
//! sort by bytes; "idle" = a texture-like handle not used for N frames, or never.
//! Reports are plain data (no references into the tables), so any thread can analyse one.

use crate::types::{Attr, StrId};
use std::collections::HashMap;

/// The object kinds that have tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Texture,
    Buffer,
    Pipeline,
    Window,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Texture, Kind::Buffer, Kind::Pipeline, Kind::Window];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Texture => "texture",
            Kind::Buffer => "buffer",
            Kind::Pipeline => "pipeline",
            Kind::Window => "window",
        }
    }
}

/// What the ledger records for one handle. `detail` is the report's kind label
/// ("render target", "depth", "instance buffers", ...), chosen by the executor at create time.
#[derive(Clone, Debug)]
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
}

#[derive(Default, Debug)]
pub struct Ledger {
    entries: HashMap<(Kind, u32), LedgerEntry>,
    /// every create / destroy ever seen, per site (churn)
    sites: HashMap<StrId, SiteCounts>,
    /// What the device says this process holds (set by backends that can ask): the ledger's
    /// total reconciles against it.
    pub device_allocated: Option<u64>,
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
        self.entries.insert((e.kind, e.index), e);
    }

    pub fn remove(&mut self, kind: Kind, index: u32) {
        self.entries.remove(&(kind, index));
    }

    /// Marks the handle used by `frame` (a draw sampled / drew / rendered to it).
    pub fn touch(&mut self, kind: Kind, index: u32, gen: u32, frame: u64, now_ns: u64) {
        if let Some(e) = self.entries.get_mut(&(kind, index)) {
            if e.gen == gen {
                e.last_used_frame = frame;
                e.last_used_ns = now_ns;
            }
        }
    }

    pub fn get(&self, kind: Kind, index: u32) -> Option<&LedgerEntry> {
        self.entries.get(&(kind, index))
    }

    pub fn get_mut(&mut self, kind: Kind, index: u32) -> Option<&mut LedgerEntry> {
        self.entries.get_mut(&(kind, index))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.bytes).sum()
    }

    pub fn entries(&self) -> impl Iterator<Item = &LedgerEntry> {
        self.entries.values()
    }

    /// The report of this session. `strings` resolves the attrs; `frame` is the current frame;
    /// a texture-like handle unused for `idle_frames` frames (or never) is idle.
    pub fn report(&self, session: u32, strings: &HashMap<StrId, String>, frame: u64, idle_frames: u64) -> Report {
        let name = |id: StrId| strings.get(&id).cloned().filter(|_| id != 0);
        let mut items: Vec<ReportItem> = self
            .entries
            .values()
            .map(|e| {
                let site = name(e.attr.site).unwrap_or_default();
                let owner = name(e.attr.label).or_else(|| name(e.attr.owner)).or_else(|| (!site.is_empty()).then(|| site.clone())).unwrap_or_else(|| "unknown".to_string());
                ReportItem {
                    session,
                    kind: e.kind,
                    detail: e.detail,
                    index: e.index,
                    gen: e.gen,
                    bytes: e.bytes,
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
    let mut m: HashMap<String, (u64, usize)> = HashMap::new();
    for i in items {
        let e = m.entry(key(i)).or_default();
        e.0 += i.bytes;
        e.1 += 1;
    }
    let mut v: Vec<Group> = m.into_iter().map(|(key, (bytes, count))| Group { key, bytes, count }).collect();
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
    pub count: usize,
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
