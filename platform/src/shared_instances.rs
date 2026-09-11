//! The generic retained-instance contract (cleanup lane DL-1, additive
//! freeze): a producer publishes an immutable, refcounted block of instance
//! data once; any number of draw items reference `(publication, first,
//! count)`; the platform keeps one GPU buffer per live publication, created
//! when the block is published, freed when the last reference drops and the
//! last command buffer reading it has completed. Readiness, accounting and
//! receipts are explicit; every policy (order, prefetch, eviction, pacing)
//! lives with the producer.
//!
//! This module compiles next to `retained_instances` (the implementation the
//! backends still consume). Nothing here is drawn yet: DL-3 gives each
//! backend a backing per publication and DL-4 moves the code atlas onto it;
//! DL-5 deletes the old machinery. Until then `SharedInstances::retained`
//! adapts a publication onto the old attach path so both can coexist.
//!
//! Laws: no statics (identity comes from a context-owned counter that
//! workers clone), no environment knobs, no constants that are not machine
//! facts, no scene vocabulary.

use crate::texture::FrameSerials;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Weak,
};

/// The context-owned identity source for publications. `Cx` owns one;
/// workers clone it and mint ids without the context. Two contexts mint
/// from two sources, so an id never crosses contexts by accident.
#[derive(Clone, Debug)]
pub struct PublicationIds(Arc<AtomicU64>);

impl PublicationIds {
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
    pub fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for PublicationIds {
    fn default() -> Self {
        Self::new()
    }
}

/// What the producer tells the platform about a publication. None of it
/// changes correctness: `order` and `keep` are the producer's own ranking
/// for the producer's own scheduler to read back; `partial_ok` says the
/// producer may attach a prefix of the block before the whole block is
/// resident (never a partially uploaded buffer: the prefix must be ready).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishHints {
    pub order: u32,
    pub keep: u32,
    pub partial_ok: bool,
}

/// Why a publish was refused. `NoRoom` carries the accounting at the time of
/// refusal so the producer can rank its own victims; the platform evicts
/// nothing on the producer's behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    InvalidStride,
    Empty,
    NoRoom {
        requested: usize,
        charged: usize,
        pending_retirement: usize,
        envelope: usize,
    },
}

/// Where a publication is in its life. Phases only move forward except
/// `Failed`, which is terminal and explicit (a producer sees it and decides).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptPhase {
    /// Published; the backing copy has not landed (asynchronous facility).
    Pending,
    /// Backing resident: drawable.
    Ready,
    /// A draw referencing it was encoded into the command buffer for
    /// `serial`, not yet committed (backends that encode ahead of submit).
    Encoded { serial: u64 },
    /// A draw referencing it was submitted with `serial` (completion still
    /// outstanding).
    Submitted { serial: u64 },
    /// Every submission that read it has completed: no longer in use on the
    /// GPU. Completion never means "rendered" or "presented".
    Complete,
    /// Backing physically released; the charge is gone.
    Released,
    /// The upload or the backing failed; the producer must republish.
    Failed,
}

struct Counters {
    charged: AtomicUsize,
    pending_retirement: AtomicUsize,
    envelope: AtomicUsize,
    live: AtomicUsize,
}

/// One publication's state, shared by the block and its receipt. Every
/// transition is a store on an atomic so a backend completion callback can
/// advance it without the context.
struct State {
    ready: AtomicBool,
    failed: AtomicBool,
    released: AtomicBool,
    /// The greatest frame serial whose command buffer encoded a draw of
    /// this block (ahead of submission on backends that encode early).
    encoded: AtomicU64,
    /// The greatest frame serial of a submission that read this block.
    submitted: AtomicU64,
    /// The binding revision the last submission used (uniform patches).
    binding_revision: AtomicU64,
    charged_bytes: AtomicUsize,
    counters: Arc<Counters>,
}

struct Publication {
    id: u64,
    slots: usize,
    data: Arc<[f32]>,
    hints: PublishHints,
    state: Arc<State>,
    serials: Arc<FrameSerials>,
}

impl Drop for Publication {
    /// The last reference is gone: the charge moves from `charged` to
    /// `pending_retirement` until the backend reports the physical release
    /// (`PublishReceipt::released` / `retire_complete`). A block a command
    /// buffer is still reading stays charged as pending, never uncounted.
    fn drop(&mut self) {
        let bytes = self.state.charged_bytes.swap(0, Ordering::AcqRel);
        if bytes != 0 && !self.state.released.load(Ordering::Acquire) {
            self.state.counters.charged.fetch_sub(bytes, Ordering::AcqRel);
            self.state
                .counters
                .pending_retirement
                .fetch_add(bytes, Ordering::AcqRel);
            self.state.charged_bytes.store(bytes, Ordering::Release);
        }
        self.state.counters.live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// An immutable, refcounted block of instance data. Cloning is a reference;
/// the data never moves or copies after `publish`.
#[derive(Clone)]
pub struct SharedInstances(Arc<Publication>);

/// A non-owning reference: reports whether the block is still alive and how
/// many bytes it charges, without keeping it alive.
#[derive(Clone)]
pub struct WeakSharedInstances {
    publication: Weak<Publication>,
    bytes: usize,
}

impl WeakSharedInstances {
    pub fn upgrade(&self) -> Option<SharedInstances> {
        self.publication.upgrade().map(SharedInstances)
    }
    pub fn live_bytes(&self) -> usize {
        if self.publication.strong_count() > 0 {
            self.bytes
        } else {
            0
        }
    }
}

impl SharedInstances {
    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn slots(&self) -> usize {
        self.0.slots
    }
    pub fn data(&self) -> &[f32] {
        &self.0.data
    }
    /// Instances in the block.
    pub fn count(&self) -> usize {
        self.0.data.len() / self.0.slots
    }
    pub fn byte_len(&self) -> usize {
        self.0.data.len() * std::mem::size_of::<f32>()
    }
    pub fn hints(&self) -> PublishHints {
        self.0.hints
    }
    pub fn downgrade(&self) -> WeakSharedInstances {
        WeakSharedInstances {
            publication: Arc::downgrade(&self.0),
            bytes: self.byte_len(),
        }
    }
    /// References held (draw items, the producer, receipts do not count).
    pub fn readers(&self) -> usize {
        Arc::strong_count(&self.0)
    }
    pub fn receipt(&self) -> PublishReceipt {
        PublishReceipt {
            state: self.0.state.clone(),
            serials: self.0.serials.clone(),
        }
    }
    /// A draw references an instance-aligned slice: `(first, count)` is
    /// validated against the block, never against the stride alone.
    pub fn slice(&self, first: usize, count: usize) -> Option<std::ops::Range<usize>> {
        let total = self.count();
        (first <= total && count <= total - first).then(|| first..first + count)
    }
    /// Adapter for the old attach path (`add_retained_instances`): the same
    /// payload as a `RetainedInstances`, without a copy. The old type mints
    /// its own id; the new id is the publication's. DL-5 removes this.
    pub fn retained(&self) -> crate::retained_instances::RetainedInstances {
        crate::retained_instances::RetainedInstances::new(self.0.slots, self.0.data.clone())
            .expect("a published block has a valid stride")
    }
}

/// The receipt of a publication: readiness, submission, completion and
/// release, each answered without the context. Backends advance it; the
/// producer reads it for its settle and liveness proofs.
#[derive(Clone)]
pub struct PublishReceipt {
    state: Arc<State>,
    serials: Arc<FrameSerials>,
}

impl PublishReceipt {
    pub fn upload_ready(&self) -> bool {
        self.state.ready.load(Ordering::Acquire) && !self.state.failed.load(Ordering::Acquire)
    }
    pub fn failed(&self) -> bool {
        self.state.failed.load(Ordering::Acquire)
    }
    pub fn released(&self) -> bool {
        self.state.released.load(Ordering::Acquire)
    }
    /// The greatest frame serial whose command buffer holds an encoded draw
    /// of the block; zero before any draw. Encoded is at least submitted.
    pub fn encoded_serial(&self) -> u64 {
        self.state.encoded.load(Ordering::Acquire)
    }
    /// The greatest frame serial of a submission that read the block; zero
    /// before any draw.
    pub fn submitted_serial(&self) -> u64 {
        self.state.submitted.load(Ordering::Acquire)
    }
    pub fn binding_revision(&self) -> u64 {
        self.state.binding_revision.load(Ordering::Acquire)
    }
    /// Every submission that read the block has completed. Completion is
    /// "no longer in use", not "rendered".
    pub fn draw_complete(&self) -> bool {
        let submitted = self.submitted_serial();
        submitted != 0 && self.serials.completed.load(Ordering::Acquire) >= submitted
    }
    pub fn phase(&self) -> ReceiptPhase {
        if self.failed() {
            ReceiptPhase::Failed
        } else if self.released() {
            ReceiptPhase::Released
        } else if !self.upload_ready() {
            ReceiptPhase::Pending
        } else if self.submitted_serial() == 0 {
            match self.encoded_serial() {
                0 => ReceiptPhase::Ready,
                serial => ReceiptPhase::Encoded { serial },
            }
        } else if self.encoded_serial() > self.submitted_serial() {
            ReceiptPhase::Encoded {
                serial: self.encoded_serial(),
            }
        } else if self.draw_complete() {
            ReceiptPhase::Complete
        } else {
            ReceiptPhase::Submitted {
                serial: self.submitted_serial(),
            }
        }
    }
    // --- backend side (DL-3 calls these; tests exercise them) ---
    /// The backing copy landed: the block is drawable.
    pub fn mark_ready(&self) {
        self.state.ready.store(true, Ordering::Release);
    }
    pub fn mark_failed(&self) {
        self.state.failed.store(true, Ordering::Release);
    }
    /// A draw that reads the block was encoded into the command buffer for
    /// `serial` (before commit). Serials only move forward.
    pub fn mark_encoded(&self, serial: u64) {
        self.state.encoded.fetch_max(serial, Ordering::AcqRel);
    }
    /// A draw that reads the block was submitted with `serial` under
    /// `binding_revision`. Serials only move forward; submission implies
    /// encoding.
    pub fn mark_submitted(&self, serial: u64, binding_revision: u64) {
        self.state.encoded.fetch_max(serial, Ordering::AcqRel);
        self.state.submitted.fetch_max(serial, Ordering::AcqRel);
        self.state
            .binding_revision
            .store(binding_revision, Ordering::Release);
    }
    /// The backing is physically gone. Legal only after the last reference
    /// dropped and `draw_complete()` (or nothing was ever submitted); the
    /// pending-retirement charge is released here and nowhere else.
    pub fn retire_complete(&self) -> bool {
        let submitted = self.submitted_serial();
        let completed = self.serials.completed.load(Ordering::Acquire);
        if submitted != 0 && completed < submitted {
            return false;
        }
        if self.state.released.swap(true, Ordering::AcqRel) {
            return true;
        }
        let bytes = self.state.charged_bytes.swap(0, Ordering::AcqRel);
        if bytes != 0 {
            // Either the block is still alive (charge moves from charged
            // straight to released) or it dropped (charge sits in pending).
            let counters = &self.state.counters;
            let pending = counters.pending_retirement.load(Ordering::Acquire);
            if pending >= bytes {
                counters.pending_retirement.fetch_sub(bytes, Ordering::AcqRel);
            } else {
                counters.charged.fetch_sub(bytes, Ordering::AcqRel);
            }
        }
        true
    }
}

/// A frame's claim on shared GPU storage (the uniform ring, staging): a
/// slot leased under `serial` is reusable only once the serial frontier
/// has completed. Nothing else decides reuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLease {
    pub serial: u64,
}

pub struct FrameLeases {
    serials: Arc<FrameSerials>,
    outstanding: Vec<FrameLease>,
}

impl FrameLeases {
    pub(crate) fn new(serials: Arc<FrameSerials>) -> Self {
        Self {
            serials,
            outstanding: Vec::new(),
        }
    }
    /// Lease storage for the submission `serial` (the frame being encoded).
    pub fn lease(&mut self, serial: u64) -> FrameLease {
        let lease = FrameLease { serial };
        self.outstanding.push(lease);
        lease
    }
    pub fn reusable(&self, lease: FrameLease) -> bool {
        self.serials.completed.load(Ordering::Acquire) >= lease.serial
    }
    /// Drop every lease whose frame completed; the number of slots that may
    /// be reused now.
    pub fn collect(&mut self) -> usize {
        let completed = self.serials.completed.load(Ordering::Acquire);
        let before = self.outstanding.len();
        self.outstanding.retain(|lease| lease.serial > completed);
        before - self.outstanding.len()
    }
    /// The oldest outstanding lease: the frame the ring must not overwrite.
    pub fn tail(&self) -> Option<FrameLease> {
        self.outstanding.iter().copied().min_by_key(|lease| lease.serial)
    }
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }
}

/// What the last frame's uploads cost, as the backend measured them. The
/// only input to pacing besides the frame's remaining time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadObservation {
    pub bytes: usize,
    pub copy_ns: u64,
}

/// Everything a producer needs to decide how much to publish this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishBackpressure {
    pub charged: usize,
    pub pending_retirement: usize,
    pub envelope: usize,
    pub live: usize,
    /// Frame time still available for copies, measured by the caller.
    pub frame_remaining_ns: u64,
    pub observed: UploadObservation,
}

impl PublishBackpressure {
    pub fn room(&self) -> usize {
        self.envelope
            .saturating_sub(self.charged)
            .saturating_sub(self.pending_retirement)
    }
}

/// Bytes a producer may admit for upload this frame: the room under the
/// envelope, and the bytes the observed copy rate fits into the remaining
/// frame time. No constants: both inputs are measurements. With no
/// observation yet the rate is unknown and the room alone bounds the frame
/// (the first copy becomes the observation).
pub fn upload_pacing(bp: &PublishBackpressure) -> usize {
    let room = bp.room();
    if bp.observed.bytes == 0 || bp.observed.copy_ns == 0 {
        return room;
    }
    let bytes_per_ns = bp.observed.bytes as f64 / bp.observed.copy_ns as f64;
    let fits = (bytes_per_ns * bp.frame_remaining_ns as f64) as usize;
    room.min(fits)
}

/// The context's view of publication memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublicationAccounting {
    pub charged: usize,
    pub pending_retirement: usize,
    pub envelope: usize,
    pub live: usize,
}

/// The context-owned registry: mints ids, charges and releases bytes, and
/// refuses with `NoRoom` at the envelope. It never orders, evicts or
/// schedules anything.
pub struct Publications {
    ids: PublicationIds,
    serials: Arc<FrameSerials>,
    counters: Arc<Counters>,
}

impl Publications {
    pub(crate) fn new(serials: Arc<FrameSerials>) -> Self {
        Self {
            ids: PublicationIds::new(),
            serials,
            counters: Arc::new(Counters {
                charged: AtomicUsize::new(0),
                pending_retirement: AtomicUsize::new(0),
                envelope: AtomicUsize::new(0),
                live: AtomicUsize::new(0),
            }),
        }
    }
    /// The identity source for workers.
    pub fn ids(&self) -> PublicationIds {
        self.ids.clone()
    }
    /// The device envelope publications may charge in total: derived once
    /// from the machine (`retained_instances::retained_device_envelope`),
    /// never a constant. Zero means "not yet known": nothing is refused.
    pub fn set_envelope(&self, bytes: usize) {
        self.counters.envelope.store(bytes, Ordering::Release);
    }
    pub fn envelope(&self) -> usize {
        self.counters.envelope.load(Ordering::Acquire)
    }
    pub fn accounting(&self) -> PublicationAccounting {
        PublicationAccounting {
            charged: self.counters.charged.load(Ordering::Acquire),
            pending_retirement: self.counters.pending_retirement.load(Ordering::Acquire),
            envelope: self.envelope(),
            live: self.counters.live.load(Ordering::Acquire),
        }
    }
    pub fn backpressure(
        &self,
        frame_remaining_ns: u64,
        observed: UploadObservation,
    ) -> PublishBackpressure {
        let a = self.accounting();
        PublishBackpressure {
            charged: a.charged,
            pending_retirement: a.pending_retirement,
            envelope: a.envelope,
            live: a.live,
            frame_remaining_ns,
            observed,
        }
    }
    /// Publish an immutable block. Validates the stride, charges the bytes
    /// against the envelope (refusing with the accounting when they do not
    /// fit), and returns the block whose receipt starts `Pending`.
    pub fn publish(
        &self,
        slots: usize,
        data: Arc<[f32]>,
        hints: PublishHints,
    ) -> Result<SharedInstances, PublishError> {
        if slots == 0 || data.len() % slots != 0 {
            return Err(PublishError::InvalidStride);
        }
        if data.is_empty() {
            return Err(PublishError::Empty);
        }
        let bytes = data.len() * std::mem::size_of::<f32>();
        let envelope = self.envelope();
        let charged = self.counters.charged.load(Ordering::Acquire);
        let pending = self.counters.pending_retirement.load(Ordering::Acquire);
        if envelope != 0 && charged.saturating_add(pending).saturating_add(bytes) > envelope {
            return Err(PublishError::NoRoom {
                requested: bytes,
                charged,
                pending_retirement: pending,
                envelope,
            });
        }
        self.counters.charged.fetch_add(bytes, Ordering::AcqRel);
        self.counters.live.fetch_add(1, Ordering::AcqRel);
        Ok(SharedInstances(Arc::new(Publication {
            id: self.ids.next(),
            slots,
            data,
            hints,
            state: Arc::new(State {
                ready: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                released: AtomicBool::new(false),
                encoded: AtomicU64::new(0),
                submitted: AtomicU64::new(0),
                binding_revision: AtomicU64::new(0),
                charged_bytes: AtomicUsize::new(bytes),
                counters: self.counters.clone(),
            }),
            serials: self.serials.clone(),
        })))
    }
    /// Publish a block the caller already holds resident on the CPU and the
    /// backend copies synchronously (whole copy): ready on return.
    pub fn publish_ready(
        &self,
        slots: usize,
        data: Arc<[f32]>,
        hints: PublishHints,
    ) -> Result<SharedInstances, PublishError> {
        let block = self.publish(slots, data, hints)?;
        block.receipt().mark_ready();
        Ok(block)
    }
    /// Frame leases over this context's serials (one set per ring the
    /// backend keeps: uniforms, staging).
    pub fn frame_leases(&self) -> FrameLeases {
        FrameLeases::new(self.serials.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> (Publications, Arc<FrameSerials>) {
        let serials = Arc::new(FrameSerials::default());
        (Publications::new(serials.clone()), serials)
    }

    #[test]
    fn ids_are_context_owned_and_never_static() {
        let (a, _) = registry();
        let (b, _) = registry();
        let pa = a.publish(2, vec![0.0; 4].into(), PublishHints::default()).unwrap();
        let pb = b.publish(2, vec![0.0; 4].into(), PublishHints::default()).unwrap();
        assert_eq!(pa.id(), 1);
        assert_eq!(pb.id(), 1, "a second context starts its own id space");
        assert!(!a.ids().same_source(&b.ids()));
        let worker_ids = a.ids();
        assert_eq!(worker_ids.next(), 2, "workers mint from the context's source");
        assert_eq!(a.publish(2, vec![0.0; 2].into(), PublishHints::default()).unwrap().id(), 3);
    }

    #[test]
    fn stride_and_slice_are_validated_at_publish() {
        let (r, _) = registry();
        assert_eq!(
            r.publish(3, vec![0.0; 4].into(), PublishHints::default()).err(),
            Some(PublishError::InvalidStride)
        );
        assert_eq!(
            r.publish(0, vec![0.0; 4].into(), PublishHints::default()).err(),
            Some(PublishError::InvalidStride)
        );
        assert_eq!(
            r.publish(2, Vec::<f32>::new().into(), PublishHints::default()).err(),
            Some(PublishError::Empty)
        );
        let block = r.publish(2, vec![0.0; 8].into(), PublishHints::default()).unwrap();
        assert_eq!(block.count(), 4);
        assert_eq!(block.slice(1, 3), Some(1..4));
        assert_eq!(block.slice(4, 0), Some(4..4));
        assert_eq!(block.slice(3, 2), None);
        assert_eq!(block.byte_len(), 32);
    }

    #[test]
    fn receipt_phases_only_move_forward() {
        let (r, serials) = registry();
        let block = r.publish(1, vec![1.0; 3].into(), PublishHints::default()).unwrap();
        let receipt = block.receipt();
        assert_eq!(receipt.phase(), ReceiptPhase::Pending);
        assert!(!receipt.draw_complete());
        receipt.mark_ready();
        assert_eq!(receipt.phase(), ReceiptPhase::Ready);
        let s1 = serials.submit();
        receipt.mark_encoded(s1);
        assert_eq!(receipt.phase(), ReceiptPhase::Encoded { serial: s1 });
        assert!(!receipt.draw_complete(), "encoded is not submitted");
        receipt.mark_submitted(s1, 7);
        assert_eq!(receipt.encoded_serial(), s1);
        assert_eq!(receipt.phase(), ReceiptPhase::Submitted { serial: s1 });
        assert_eq!(receipt.binding_revision(), 7);
        let s2 = serials.submit();
        receipt.mark_submitted(s2, 8);
        receipt.mark_submitted(s1, 9); // a late, older submission never regresses the serial
        assert_eq!(receipt.submitted_serial(), s2);
        serials.complete(s1);
        assert!(!receipt.draw_complete(), "the newer submission is still in flight");
        serials.complete(s2);
        assert!(receipt.draw_complete());
        assert_eq!(receipt.phase(), ReceiptPhase::Complete);
        assert!(!receipt.retire_complete() || receipt.released());
    }

    #[test]
    fn retirement_waits_for_completion_and_releases_the_charge_once() {
        let (r, serials) = registry();
        r.set_envelope(1024);
        let block = r.publish(1, vec![0.0; 16].into(), PublishHints::default()).unwrap();
        let receipt = block.receipt();
        assert_eq!(r.accounting().charged, 64);
        receipt.mark_ready();
        let s = serials.submit();
        receipt.mark_submitted(s, 1);
        drop(block);
        let a = r.accounting();
        assert_eq!((a.charged, a.pending_retirement, a.live), (0, 64, 0), "a dropped block stays charged as pending until released");
        assert!(!receipt.retire_complete(), "still read by an incomplete submission");
        assert_eq!(r.accounting().pending_retirement, 64);
        serials.complete(s);
        assert!(receipt.retire_complete());
        assert!(receipt.retire_complete(), "idempotent");
        let a = r.accounting();
        assert_eq!((a.charged, a.pending_retirement), (0, 0));
        assert_eq!(receipt.phase(), ReceiptPhase::Released);
    }

    #[test]
    fn no_room_refuses_with_the_accounting_and_never_evicts() {
        let (r, _) = registry();
        r.set_envelope(100);
        let a = r.publish(1, vec![0.0; 20].into(), PublishHints::default()).unwrap(); // 80 B
        let err = r.publish(1, vec![0.0; 10].into(), PublishHints::default()).err(); // 40 B
        assert_eq!(
            err,
            Some(PublishError::NoRoom {
                requested: 40,
                charged: 80,
                pending_retirement: 0,
                envelope: 100
            })
        );
        drop(a);
        // The dropped block's charge is pending until the backend releases it:
        // room does not appear by dropping alone.
        assert!(matches!(
            r.publish(1, vec![0.0; 10].into(), PublishHints::default()).err(),
            Some(PublishError::NoRoom { pending_retirement: 80, .. })
        ));
        let weak_alive = r.publish(1, vec![0.0; 5].into(), PublishHints::default()).unwrap();
        let weak = weak_alive.downgrade();
        assert_eq!(weak.live_bytes(), 20);
        drop(weak_alive);
        assert_eq!(weak.live_bytes(), 0);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn unknown_envelope_refuses_nothing() {
        let (r, _) = registry();
        assert_eq!(r.envelope(), 0);
        let big = r.publish(1, vec![0.0; 1 << 20].into(), PublishHints::default());
        assert!(big.is_ok());
        assert_eq!(r.accounting().charged, 4 << 20);
    }

    #[test]
    fn leases_reuse_only_after_completion() {
        let (_, serials) = registry();
        let mut leases = FrameLeases::new(serials.clone());
        let l1 = leases.lease(serials.submit());
        let l2 = leases.lease(serials.submit());
        assert_eq!(leases.tail(), Some(l1));
        assert!(!leases.reusable(l1) && !leases.reusable(l2));
        serials.complete(l1.serial);
        assert!(leases.reusable(l1) && !leases.reusable(l2));
        assert_eq!(leases.collect(), 1);
        assert_eq!(leases.tail(), Some(l2));
        serials.complete(l2.serial);
        assert_eq!(leases.collect(), 1);
        assert_eq!((leases.outstanding(), leases.tail()), (0, None));
    }

    #[test]
    fn pacing_is_derived_from_room_and_the_observed_copy_rate() {
        let (r, _) = registry();
        r.set_envelope(1000);
        let _held = r.publish(1, vec![0.0; 100].into(), PublishHints::default()).unwrap(); // 400 B
        let bp = r.backpressure(1_000_000, UploadObservation::default());
        assert_eq!(bp.room(), 600);
        assert_eq!(upload_pacing(&bp), 600, "no observation: the room bounds the frame");
        // 1 MiB copied in 1 ms; 100 µs remaining fits 1 MiB / 10, capped by the room.
        let observed = UploadObservation { bytes: 1 << 20, copy_ns: 1_000_000 };
        let bp = r.backpressure(100_000, observed);
        assert_eq!(upload_pacing(&bp), 600);
        r.set_envelope(1 << 30);
        let bp = r.backpressure(100_000, observed);
        let fits = (1usize << 20) / 10;
        assert!((fits - 2..=fits + 2).contains(&upload_pacing(&bp)), "{}", upload_pacing(&bp));
        let bp = r.backpressure(0, observed);
        assert_eq!(upload_pacing(&bp), 0, "no frame time left: nothing is admitted");
    }

    #[test]
    fn the_adapter_keeps_the_payload_and_stride() {
        let (r, _) = registry();
        let block = r.publish_ready(2, vec![1.0, 2.0, 3.0, 4.0].into(), PublishHints::default()).unwrap();
        assert!(block.receipt().upload_ready());
        let old = block.retained();
        assert_eq!(old.slots(), 2);
        assert_eq!(old.data(), block.data());
        assert_eq!(old.count(), block.count());
    }
}
