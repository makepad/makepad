//! Shared, lock-free admission accounting. No UI or runtime dependencies.
//!
//! This lives below code-graph so the repository reader can use the very same
//! account as indexing and history. Code-graph and code-atlas re-export it.
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

/// Return allocator-owned free pages after a large worker build/retirement.
/// This never discards live allocations or changes admission charges. Call
/// only on a worker: the system allocator may scan its zones while reclaiming.
/// Unsupported allocators/platforms simply retain their normal free-page policy.
pub fn release_unused_memory() {
    #[cfg(target_os = "macos")]
    unsafe {
        unsafe extern "C" { fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize; }
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        unsafe extern "C" { fn malloc_trim(pad: usize) -> std::ffi::c_int; }
        malloc_trim(0);
    }
}

#[derive(Debug)]
struct Account {
    capacity: usize,
    reserved: AtomicUsize,
    peak: AtomicUsize,
    /// The most the ledger has stood above its capacity.
    overcommitted: AtomicUsize,
}

#[derive(Clone, Debug)]
pub struct MemoryAccount(Arc<Account>);

impl MemoryAccount {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Account { capacity, reserved: AtomicUsize::new(0), peak: AtomicUsize::new(0), overcommitted: AtomicUsize::new(0) }))
    }
    pub fn capacity(&self) -> usize { self.0.capacity }
    pub fn reserved(&self) -> usize { self.0.reserved.load(Ordering::Acquire) }
    pub fn available(&self) -> usize { self.capacity().saturating_sub(self.reserved()) }
    pub fn peak(&self) -> usize { self.0.peak.load(Ordering::Acquire) }
    /// The most `reserve` has taken the ledger above its capacity.
    pub fn overcommitted(&self) -> usize { self.0.overcommitted.load(Ordering::Acquire) }
    pub fn same_account(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
    /// Take `bytes` whether or not the account has room: for work that
    /// cannot do without them. The ledger is a pacing device, not a wall:
    /// `try_reserve` answers callers that can wait or scale down, and this
    /// answers the ones that cannot. The excess shows in `overcommitted`
    /// and, while it stands, `available` is zero, so pacing callers hold
    /// back until it is given back; the machine may swap, the work does
    /// not stop.
    pub fn reserve(&self, bytes: usize) -> Reservation {
        let old = self.0.reserved.fetch_add(bytes, Ordering::AcqRel);
        let next = old.saturating_add(bytes);
        self.0.peak.fetch_max(next, Ordering::Relaxed);
        if next > self.capacity() {
            self.0.overcommitted.fetch_max(next - self.capacity(), Ordering::Relaxed);
        }
        Reservation(Arc::new(Reserved { account: self.clone(), bytes }))
    }
    pub fn try_reserve(&self, bytes: usize) -> Option<Reservation> {
        // Keep fetch_update for older stable toolchains without try_update.
        #[allow(deprecated)]
        let old = self.0.reserved.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= self.capacity())
        }).ok()?;
        self.0.peak.fetch_max(old + bytes, Ordering::Relaxed);
        Some(Reservation(Arc::new(Reserved { account: self.clone(), bytes })))
    }
}

#[derive(Debug)]
struct Reserved { account: MemoryAccount, bytes: usize }
impl Drop for Reserved {
    fn drop(&mut self) { self.account.0.reserved.fetch_sub(self.bytes, Ordering::AcqRel); }
}

/// Cloning transfers shared ownership; bytes are released only by the last
/// owner. Keep the lease alongside external publications and retirement jobs.
#[derive(Clone, Debug)]
pub struct Reservation(Arc<Reserved>);
impl Reservation {
    pub fn bytes(&self) -> usize { self.0.bytes }
    pub fn account(&self) -> &MemoryAccount { &self.0.account }
    /// Transfer part of an exclusive construction lease without releasing and
    /// re-reserving between ownership changes.
    pub fn split_off(&mut self, bytes: usize) -> Option<Reservation> {
        let inner = Arc::get_mut(&mut self.0)?;
        if bytes > inner.bytes { return None; }
        inner.bytes -= bytes;
        Some(Reservation(Arc::new(Reserved { account: inner.account.clone(), bytes })))
    }
    /// Join exclusive leases on the same account without a release/re-reserve
    /// gap. The donor becomes empty; no account total or peak changes.
    pub fn merge(&mut self, donor: &mut Reservation) -> bool {
        if !self.account().same_account(donor.account()) { return false; }
        let Some(to) = Arc::get_mut(&mut self.0) else { return false };
        let Some(from) = Arc::get_mut(&mut donor.0) else { return false };
        to.bytes += from.bytes;
        from.bytes = 0;
        true
    }
    /// Release unused construction space before sharing the lease.
    pub fn shrink_to(&mut self, bytes: usize) -> bool {
        let Some(inner) = Arc::get_mut(&mut self.0) else { return false };
        if bytes > inner.bytes { return false; }
        inner.account.0.reserved.fetch_sub(inner.bytes - bytes, Ordering::AcqRel);
        inner.bytes = bytes;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_leases_release_once_and_refuse_overflow() {
        let account = MemoryAccount::new(64);
        let mut lease = account.try_reserve(64).unwrap();
        assert!(account.try_reserve(1).is_none());
        assert!(account.try_reserve(usize::MAX).is_none());
        assert!(lease.shrink_to(32));
        let other = lease.clone();
        drop(lease);
        assert_eq!(account.reserved(), 32);
        drop(other);
        assert_eq!(account.reserved(), 0);
        assert_eq!(account.peak(), 64);
    }

    /// What must be taken is taken: the ledger stands above its capacity,
    /// says by how much, gives nothing to callers that ask for room until
    /// it is given back, and comes back down when it is.
    #[test]
    fn a_reservation_that_must_be_taken_overcommits_and_is_given_back() {
        let account = MemoryAccount::new(64);
        let held = account.try_reserve(48).unwrap();
        let taken = account.reserve(32);
        assert_eq!(account.reserved(), 80);
        assert_eq!(account.available(), 0);
        assert_eq!(account.overcommitted(), 16);
        assert!(account.try_reserve(1).is_none(), "no room while the ledger stands above its capacity");
        drop(taken);
        assert_eq!(account.reserved(), 48);
        assert!(account.try_reserve(16).is_some());
        drop(held);
        assert_eq!(account.overcommitted(), 16, "the excess is remembered as a peak");
    }
}
