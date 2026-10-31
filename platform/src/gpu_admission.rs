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
//! Cx owns the admission client. The HAL reports actual device usage after submitted
//! allocations; new reservations remain charged locally until that report acknowledges
//! their epoch. Admission reads this cached state, never the HAL pipe. The reported
//! bytes include other sessions and unowned allocations. Like any asynchronous device
//! snapshot, this is planning admission, not a driver-enforced allocation guarantee.
//! Reservations are RAII; dropping a document or reservation returns its planned bytes.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

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
    Document {
        document: String,
        wanted: u64,
        held: u64,
        limit: u64,
    },
    /// The device budget would be exceeded (all documents together).
    Device {
        document: String,
        wanted: u64,
        in_use: u64,
        limit: u64,
    },
    /// One submission is over its caps: split it.
    Submission {
        bytes: u64,
        draws: u64,
        budget: GpuBudget,
    },
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
                mib(&held.saturating_add(*wanted)),
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
pub fn target_bytes(
    width: u64,
    height: u64,
    layers: u64,
    bytes_per_texel: u64,
    mips: u32,
) -> Result<u64, GpuRefusal> {
    let overflow =
        || GpuRefusal::Overflow(format!("{width}x{height}x{layers} x {bytes_per_texel} B"));
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
    reserved: u64,
    device_used: u64,
    epoch: u64,
    #[cfg(not(target_arch = "wasm32"))]
    submitted_epoch: u64,
    pending: BTreeMap<u64, (u64, u64)>, // epoch -> (document, bytes)
    pending_bytes: u64,
    #[cfg(not(target_arch = "wasm32"))]
    refused: bool,
}

impl LedgerState {
    fn in_use(&self) -> u64 {
        self.device_used
            .saturating_add(self.pending_bytes)
            .max(self.reserved)
    }
}

/// The admission state of one Cx, accessed through `Cx::gpu_admission`.
#[derive(Debug)]
pub struct GpuLedger {
    state: Rc<RefCell<LedgerState>>,
}

/// One document's registration. Dropping it releases everything the
/// document still holds.
#[derive(Debug)]
pub struct GpuDocument {
    state: Rc<RefCell<LedgerState>>,
    id: u64,
}

/// Bytes admitted for one document. Dropping it gives them back.
#[derive(Debug)]
pub struct GpuReservation {
    state: Rc<RefCell<LedgerState>>,
    doc: u64,
    bytes: u64,
    epoch: u64,
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
        Self {
            state: Rc::new(RefCell::new(LedgerState {
                budget,
                next_id: 0,
                docs: BTreeMap::new(),
                reserved: 0,
                device_used: 0,
                epoch: 0,
                pending: BTreeMap::new(),
                pending_bytes: 0,
                #[cfg(not(target_arch = "wasm32"))]
                submitted_epoch: 0,
                #[cfg(not(target_arch = "wasm32"))]
                refused: false,
            })),
        }
    }

    /// Mark reservations whose allocations are included in this batch. Control-only batches
    /// may refresh usage, but must not acknowledge plans that have not been drawn yet.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn submitted(&self, has_frames: bool) -> u64 {
        let mut state = self.state.borrow_mut();
        if has_frames {
            state.submitted_epoch = state.epoch;
        }
        state.submitted_epoch
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn update(&self, epoch: u64, device_bytes: u64, used_bytes: u64) -> bool {
        let mut state = self.state.borrow_mut();
        let old_room = state.budget.device_bytes.saturating_sub(state.in_use());
        if device_bytes != 0 {
            state.budget.device_bytes = device_bytes;
        }
        state.device_used = used_bytes;
        while state
            .pending
            .first_key_value()
            .is_some_and(|(key, _)| *key <= epoch)
        {
            let (_, (_, bytes)) = state.pending.pop_first().unwrap();
            state.pending_bytes -= bytes;
        }
        let retry =
            state.refused && state.budget.device_bytes.saturating_sub(state.in_use()) > old_room;
        if retry {
            state.refused = false;
        }
        retry
    }

    pub fn budget(&self) -> GpuBudget {
        self.state.borrow().budget
    }

    /// Replace the limits (host settings, or the device's real size once the
    /// backend knows it). Existing reservations stay; new ones see the new
    /// limits.
    pub fn set_budget(&self, budget: GpuBudget) {
        self.state.borrow_mut().budget = budget;
    }

    /// Set only the device-wide byte budget (the backend's measured size).
    pub fn set_device_bytes(&self, device_bytes: u64) {
        self.state.borrow_mut().budget.device_bytes = device_bytes;
    }

    /// Register a document under `label` (shown in refusals and snapshots).
    pub fn register(&self, label: impl Into<String>) -> GpuDocument {
        let mut state = self.state.borrow_mut();
        let id = state.next_id;
        state.next_id += 1;
        state.docs.insert(
            id,
            DocEntry {
                label: label.into(),
                held: 0,
            },
        );
        GpuDocument {
            state: self.state.clone(),
            id,
        }
    }

    pub fn snapshot(&self) -> GpuLedgerSnapshot {
        let state = self.state.borrow();
        GpuLedgerSnapshot {
            budget: state.budget,
            in_use: state.in_use(),
            documents: state
                .docs
                .values()
                .map(|d| (d.label.clone(), d.held))
                .collect(),
        }
    }

    /// Whether one submission of `draws` calls touching `bytes` is within
    /// the caps. A caller over them splits the work into several
    /// submissions; it never sends it whole.
    pub fn check_submission(&self, bytes: u64, draws: u64) -> Result<(), GpuRefusal> {
        let budget = self.budget();
        if bytes > budget.submission_bytes || draws > budget.submission_draws {
            return Err(GpuRefusal::Submission {
                bytes,
                draws,
                budget,
            });
        }
        Ok(())
    }
}

impl GpuDocument {
    /// Reserve `bytes` for this document, or refuse without reserving
    /// anything when the document's or the device's budget would be
    /// exceeded.
    pub fn admit(&self, bytes: u64) -> Result<GpuReservation, GpuRefusal> {
        let mut state = self.state.borrow_mut();
        let budget = state.budget;
        let in_use = state.in_use();
        let doc = state.docs.get(&self.id).expect("registered document");
        let held = doc.held;
        let over_doc = held
            .checked_add(bytes)
            .is_none_or(|t| t > budget.document_bytes);
        if over_doc {
            return Err(GpuRefusal::Document {
                document: doc.label.clone(),
                wanted: bytes,
                held,
                limit: budget.document_bytes,
            });
        }
        let over_device = in_use
            .checked_add(bytes)
            .is_none_or(|t| t > budget.device_bytes);
        if over_device {
            let refusal = GpuRefusal::Device {
                document: doc.label.clone(),
                wanted: bytes,
                in_use,
                limit: budget.device_bytes,
            };
            #[cfg(not(target_arch = "wasm32"))]
            {
                state.refused = true;
            }
            return Err(refusal);
        }
        state.docs.get_mut(&self.id).unwrap().held += bytes;
        state.reserved += bytes;
        state.epoch += 1;
        let epoch = state.epoch;
        state.pending.insert(epoch, (self.id, bytes));
        state.pending_bytes += bytes;
        Ok(GpuReservation {
            state: self.state.clone(),
            doc: self.id,
            bytes,
            epoch,
        })
    }

    /// Bytes this document holds across its live reservations.
    pub fn held(&self) -> u64 {
        self.state.borrow().docs.get(&self.id).map_or(0, |d| d.held)
    }
}

impl Drop for GpuDocument {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        if let Some(doc) = state.docs.remove(&self.id) {
            state.reserved -= doc.held;
        }
        let mut released = 0;
        state.pending.retain(|_, (doc, bytes)| {
            if *doc == self.id {
                released += *bytes;
                false
            } else {
                true
            }
        });
        state.pending_bytes -= released;
    }
}

impl GpuReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for GpuReservation {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        if let Some(doc) = state.docs.get_mut(&self.doc) {
            doc.held -= self.bytes;
            state.reserved -= self.bytes;
        }
        if let Some((_, bytes)) = state.pending.remove(&self.epoch) {
            state.pending_bytes -= bytes;
        }
    }
}

impl Default for GpuLedger {
    fn default() -> Self {
        Self::new(GpuBudget::default())
    }
}

impl crate::cx::Cx {
    pub fn gpu_admission(&self) -> &GpuLedger {
        let bytes = crate::gpu_info::gpu_working_set_bytes();
        if bytes != 0 {
            self.gpu_admission.set_device_bytes(bytes);
        }
        &self.gpu_admission
    }
}
