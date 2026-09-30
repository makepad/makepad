//! The device-wide GPU admission ledger (KERNELS.md §5.3).
//!
//! Every live document on the device (a VJ deck, a Motion layer, a preview,
//! a thumbnail renderer) reserves the GPU memory its frame plan needs —
//! render targets including depth, history and accumulation, kernel buffers —
//! *before* it allocates anything. The ledger sums every reservation against
//! one device budget and one per-document budget, so a document whose plan
//! does not fit is refused (or queued by its host) with a message, instead
//! of the device running out of memory mid-frame for every document at once.
//!
//! It also caps one submission: a command buffer's draw count and the bytes
//! it touches, so a long export is split into bounded submissions rather than
//! handing the driver one unbounded batch.
//!
//! The ledger is one per process (there is one GPU device per process);
//! reservations are RAII: dropping a [`GpuReservation`] or a
//! [`GpuDocument`] gives its bytes back.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

/// The limits the ledger admits against. Host settings; the device budget
/// is set from the device when the backend knows it (Metal's recommended
/// working set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuBudget {
    /// All documents together.
    pub device_bytes: u64,
    /// One document.
    pub document_bytes: u64,
    /// Bytes one submission may read or write.
    pub submission_bytes: u64,
    /// Draw (or dispatch) calls in one submission.
    pub submission_draws: u64,
}

impl Default for GpuBudget {
    fn default() -> Self {
        Self {
            device_bytes: 4 << 30,
            document_bytes: 1 << 30,
            submission_bytes: 2 << 30,
            submission_draws: 1 << 20,
        }
    }
}

/// Why the ledger refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuRefusal {
    /// A size computation overflowed (a hostile or broken plan).
    Overflow(String),
    /// The document's own budget would be exceeded.
    Document { document: String, wanted: u64, held: u64, limit: u64 },
    /// The device budget would be exceeded (all documents together).
    Device { document: String, wanted: u64, in_use: u64, limit: u64 },
    /// One submission is over its caps: split it.
    Submission { bytes: u64, draws: u64, budget: GpuBudget },
}

impl fmt::Display for GpuRefusal {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mib = |b: &u64| *b as f64 / (1u64 << 20) as f64;
        match self {
            Self::Overflow(m) => write!(f, "GPU size overflow: {m}"),
            Self::Document { document, wanted, held, limit } => write!(
                f,
                "{document}: {:.1} MiB more would bring it to {:.1} MiB, over its {:.1} MiB GPU budget",
                mib(wanted),
                mib(&(held + wanted)),
                mib(limit)
            ),
            Self::Device { document, wanted, in_use, limit } => write!(
                f,
                "{document}: {:.1} MiB does not fit, {:.1} of the device's {:.1} MiB are in use",
                mib(wanted),
                mib(in_use),
                mib(limit)
            ),
            Self::Submission { bytes, draws, budget } => write!(
                f,
                "submission of {draws} draws / {:.1} MiB is over the cap ({} draws / {:.1} MiB): split it",
                mib(bytes),
                budget.submission_draws,
                mib(&budget.submission_bytes)
            ),
        }
    }
}

impl std::error::Error for GpuRefusal {}

/// Bytes of one render target: `width` x `height` x `layers` texels of
/// `bytes_per_texel`, with `mips` levels (1 for none), all checked. The
/// frame plan sums these for every attachment, depth, history and
/// accumulation target before asking the ledger.
pub fn target_bytes(width: u64, height: u64, layers: u64, bytes_per_texel: u64, mips: u32) -> Result<u64, GpuRefusal> {
    let overflow = || GpuRefusal::Overflow(format!("{width}x{height}x{layers} x {bytes_per_texel} B"));
    let base = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(layers))
        .and_then(|v| v.checked_mul(bytes_per_texel))
        .ok_or_else(overflow)?;
    if mips <= 1 {
        return Ok(base);
    }
    // A full chain is at most 4/3 of the base level.
    base.checked_add(base / 3).ok_or_else(overflow)
}

#[derive(Debug)]
struct DocEntry {
    label: String,
    held: u64,
}

#[derive(Debug)]
struct LedgerState {
    budget: GpuBudget,
    next_id: u64,
    docs: BTreeMap<u64, DocEntry>,
}

impl LedgerState {
    fn in_use(&self) -> u64 {
        self.docs.values().map(|d| d.held).sum()
    }
}

/// The ledger. Use [`GpuLedger::global`]; separate ledgers exist only for
/// tests.
#[derive(Debug)]
pub struct GpuLedger {
    state: Arc<Mutex<LedgerState>>,
}

/// One document's registration. Dropping it releases everything the
/// document still holds.
#[derive(Debug)]
pub struct GpuDocument {
    state: Arc<Mutex<LedgerState>>,
    id: u64,
}

/// Bytes admitted for one document. Dropping it gives them back.
#[derive(Debug)]
pub struct GpuReservation {
    state: Arc<Mutex<LedgerState>>,
    doc: u64,
    bytes: u64,
}

/// What the ledger holds right now, for a host's memory panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuLedgerSnapshot {
    pub budget: GpuBudget,
    pub in_use: u64,
    /// (label, bytes held) per live document, in registration order.
    pub documents: Vec<(String, u64)>,
}

impl GpuLedger {
    pub fn new(budget: GpuBudget) -> Self {
        Self { state: Arc::new(Mutex::new(LedgerState { budget, next_id: 0, docs: BTreeMap::new() })) }
    }

    /// The process's ledger (one GPU device per process).
    pub fn global() -> &'static GpuLedger {
        static LEDGER: OnceLock<GpuLedger> = OnceLock::new();
        LEDGER.get_or_init(|| GpuLedger::new(GpuBudget::default()))
    }

    pub fn budget(&self) -> GpuBudget {
        self.state.lock().unwrap().budget
    }

    /// Replace the limits (host settings, or the device's real size once the
    /// backend knows it). Existing reservations stay; new ones see the new
    /// limits.
    pub fn set_budget(&self, budget: GpuBudget) {
        self.state.lock().unwrap().budget = budget;
    }

    /// Set only the device-wide byte budget (the backend's measured size).
    pub fn set_device_bytes(&self, device_bytes: u64) {
        self.state.lock().unwrap().budget.device_bytes = device_bytes;
    }

    /// Register a document under `label` (shown in refusals and snapshots).
    pub fn register(&self, label: impl Into<String>) -> GpuDocument {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;
        state.docs.insert(id, DocEntry { label: label.into(), held: 0 });
        GpuDocument { state: self.state.clone(), id }
    }

    pub fn snapshot(&self) -> GpuLedgerSnapshot {
        let state = self.state.lock().unwrap();
        GpuLedgerSnapshot {
            budget: state.budget,
            in_use: state.in_use(),
            documents: state.docs.values().map(|d| (d.label.clone(), d.held)).collect(),
        }
    }

    /// Whether one submission of `draws` calls touching `bytes` is within
    /// the caps. A caller over them splits the work into several
    /// submissions; it never sends it whole.
    pub fn check_submission(&self, bytes: u64, draws: u64) -> Result<(), GpuRefusal> {
        let budget = self.budget();
        if bytes > budget.submission_bytes || draws > budget.submission_draws {
            return Err(GpuRefusal::Submission { bytes, draws, budget });
        }
        Ok(())
    }
}

impl GpuDocument {
    /// Reserve `bytes` for this document, or refuse without reserving
    /// anything when the document's or the device's budget would be
    /// exceeded.
    pub fn admit(&self, bytes: u64) -> Result<GpuReservation, GpuRefusal> {
        let mut state = self.state.lock().unwrap();
        let budget = state.budget;
        let in_use = state.in_use();
        let doc = state.docs.get(&self.id).expect("registered document");
        let held = doc.held;
        let over_doc = held.checked_add(bytes).is_none_or(|t| t > budget.document_bytes);
        if over_doc {
            return Err(GpuRefusal::Document { document: doc.label.clone(), wanted: bytes, held, limit: budget.document_bytes });
        }
        let over_device = in_use.checked_add(bytes).is_none_or(|t| t > budget.device_bytes);
        if over_device {
            return Err(GpuRefusal::Device { document: doc.label.clone(), wanted: bytes, in_use, limit: budget.device_bytes });
        }
        state.docs.get_mut(&self.id).unwrap().held += bytes;
        Ok(GpuReservation { state: self.state.clone(), doc: self.id, bytes })
    }

    /// Bytes this document holds across its live reservations.
    pub fn held(&self) -> u64 {
        self.state.lock().unwrap().docs.get(&self.id).map_or(0, |d| d.held)
    }
}

impl Drop for GpuDocument {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.docs.remove(&self.id);
        }
    }
}

impl GpuReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for GpuReservation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            // The document may be gone already (dropped first): its entry,
            // with these bytes, went with it.
            if let Some(doc) = state.docs.get_mut(&self.doc) {
                doc.held = doc.held.saturating_sub(self.bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> GpuLedger {
        GpuLedger::new(GpuBudget { device_bytes: 1000, document_bytes: 600, submission_bytes: 500, submission_draws: 10 })
    }

    #[test]
    fn documents_are_admitted_against_their_own_and_the_device_budget() {
        let ledger = small();
        let a = ledger.register("deck A");
        let b = ledger.register("deck B");
        let ra = a.admit(500).unwrap();
        // A's own budget: 500 + 200 > 600.
        assert!(matches!(a.admit(200), Err(GpuRefusal::Document { .. })));
        let rb = b.admit(450).unwrap();
        // The device: 950 in use, 100 more is over 1000 even though B has room.
        let refused = b.admit(100).unwrap_err();
        assert!(matches!(refused, GpuRefusal::Device { in_use: 950, .. }), "{refused}");
        assert!(refused.to_string().contains("deck B"));
        // Refusals reserve nothing.
        assert_eq!(ledger.snapshot().in_use, 950);
        drop(ra);
        assert_eq!(a.held(), 0);
        let rb2 = b.admit(100).unwrap();
        assert_eq!(ledger.snapshot().documents, vec![("deck A".to_string(), 0), ("deck B".to_string(), 550)]);
        drop((rb, rb2));
        assert_eq!(ledger.snapshot().in_use, 0);
    }

    #[test]
    fn dropping_a_document_releases_everything_it_held() {
        let ledger = small();
        let a = ledger.register("preview");
        let r = a.admit(600).unwrap();
        drop(a);
        assert_eq!(ledger.snapshot().in_use, 0);
        assert!(ledger.snapshot().documents.is_empty());
        // The orphaned reservation's drop is harmless.
        drop(r);
        let b = ledger.register("next");
        assert!(b.admit(600).is_ok());
    }

    #[test]
    fn target_sizes_are_checked() {
        // One 8192^2 RGBA16F target is 512 MiB, as §3.4.1 says.
        assert_eq!(target_bytes(8192, 8192, 1, 8, 1).unwrap(), 512 << 20);
        assert_eq!(target_bytes(4, 4, 6, 4, 1).unwrap(), 384);
        assert_eq!(target_bytes(3, 3, 1, 1, 4).unwrap(), 12);
        assert!(matches!(target_bytes(u64::MAX, 2, 1, 1, 1), Err(GpuRefusal::Overflow(_))));
        assert!(matches!(target_bytes(1 << 32, 1 << 32, 1, 1, 1), Err(GpuRefusal::Overflow(_))));
        // An overflowing request is refused, never wrapped into a small one.
        let ledger = small();
        let doc = ledger.register("x");
        assert!(doc.admit(u64::MAX).is_err());
    }

    #[test]
    fn submissions_over_the_caps_must_be_split() {
        let ledger = small();
        assert!(ledger.check_submission(500, 10).is_ok());
        assert!(matches!(ledger.check_submission(501, 1), Err(GpuRefusal::Submission { .. })));
        assert!(matches!(ledger.check_submission(1, 11), Err(GpuRefusal::Submission { .. })));
        ledger.set_budget(GpuBudget { submission_draws: 100, ..ledger.budget() });
        assert!(ledger.check_submission(1, 11).is_ok());
    }
}
