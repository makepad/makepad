//! Ownership of readback memory, from GPU completion until the last application view drops.
use std::{collections::HashMap, sync::{Mutex, OnceLock, atomic::{AtomicU64, Ordering}}};
use makepad_relax_api::relax_api;

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
use std::sync::Arc;

pub const BLOCK_BYTES: u64 = 64 << 20;

/// The wire carries ownership tokens and addresses, never readback payload bytes.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Record {
    pub ptr: u64,
    pub len: u64,
    pub token: u64,
    pub order: u32,
    pub reserved: u32,
}

/// An undelivered lease. Discarding a completion releases its storage automatically.
pub struct Lease {
    record: Record,
    _owner: Box<dyn Send + Sync>,
    block: Option<(u64, u64)>,
}

impl Lease {
    /// # Safety
    /// `owner` must keep this range mapped and readable until dropped. Before delivery the
    /// producer must finish all writes; no writer may touch it until the owner is released.
    pub unsafe fn new(owner: impl Send + Sync + 'static, ptr: *const u8, len: usize, order: u32) -> Self {
        assert!(!ptr.is_null() && len <= isize::MAX as usize);
        Self { record: Record { ptr: ptr as u64, len: len as u64, token: 0, order, reserved: 0 }, _owner: Box::new(owner), block: None }
    }

    /// Adopt received socket bytes or a driver's heap destination without another copy.
    pub fn from_vec(bytes: Vec<u8>, order: u32) -> Self {
        let ptr = bytes.as_ptr();
        let len = bytes.len();
        // SAFETY: the Vec is moved into this guard and never mutated again.
        unsafe { Self::new(bytes, ptr, len, order) }
    }

    pub fn record(&self) -> Record { self.record }

    /// Record the mapped block that owns this suballocation, for readback provenance checks.
    pub fn in_block(mut self, base: *const u8, bytes: u64) -> Self {
        let offset = self.record.ptr.checked_sub(base as u64).expect("lease precedes its block");
        assert!(offset.checked_add(self.record.len).is_some_and(|end| end <= bytes));
        self.block = Some((base as u64, bytes));
        self
    }

    /// Transfer exactly one release obligation to the receiver.
    pub fn into_record(mut self) -> Record {
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        assert_ne!(token, 0, "lease token space exhausted");
        self.record.token = token;
        let record = self.record;
        registry().lock().unwrap_or_else(|e| e.into_inner()).insert(token, self);
        record
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);
static LEASES: OnceLock<Mutex<HashMap<u64, Lease>>> = OnceLock::new();
fn registry() -> &'static Mutex<HashMap<u64, Lease>> { LEASES.get_or_init(Default::default) }

/// (`hal::end_run`) Every handle the ended run was given and did not release: their storage goes
/// now, outside the registry lock. The only receiver of a handle is the app.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn end_run() {
    let leases = std::mem::take(&mut *registry().lock().unwrap_or_else(|e| e.into_inner()));
    drop(leases);
}

/// Release a delivered handle on any thread and return its byte length (zero if already released).
/// Destruction runs outside the registry lock.
#[relax_api]
pub fn hal_lease_release(token: u64) -> u64 {
    let lease = registry().lock().unwrap_or_else(|e| e.into_inner()).remove(&token);
    let len = lease.as_ref().map_or(0, |lease| lease.record.len);
    drop(lease);
    len
}

/// Whether this range belongs to a currently delivered mapped HAL block.
#[relax_api]
pub fn hal_lease_in_block(ptr: u64, len: u64) -> bool {
    registry().lock().unwrap_or_else(|e| e.into_inner()).values().any(|lease| {
        lease.record.ptr == ptr && lease.record.len == len && lease.block.is_some()
    })
}

/// Number of CPU copies of buffer readback payloads (driver writes and socket I/O excluded).
static COPIES: AtomicU64 = AtomicU64::new(0);
#[relax_api]
pub fn readback_copy_count() -> u64 { COPIES.load(Ordering::Relaxed) }
#[relax_api]
pub fn count_readback_copy() { COPIES.fetch_add(1, Ordering::Relaxed); }

/// A free block nothing leased for this long is released (`Pool::trim`).
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
const BLOCK_IDLE_NS: u64 = 5_000_000_000;

/// A backend owns its pool on the encode thread. A block resets only after *all* GPU and
/// application references are gone. Dropping the pool leaves old-device blocks readable.
/// Blocks: (block, cursor, capacity, last leased at in HAL clock ns).
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(crate) struct Pool<T> { blocks: Vec<(Arc<T>, u64, u64, u64)> }
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
impl<T> Default for Pool<T> { fn default() -> Self { Self { blocks: Vec::new() } } }
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
impl<T> Pool<T> {
    pub fn alloc(&mut self, len: u64, align: u64, mut create: impl FnMut(u64) -> Result<Arc<T>, String>) -> Result<(Arc<T>, u64), String> {
        let round = |n: u64| n.checked_add(align - 1).map(|n| n / align * align).ok_or_else(|| "lease size overflow".to_string());
        let size = round(len.max(1))?;
        let now = crate::clock_ns();
        for (block, cursor, capacity, used) in &mut self.blocks {
            if Arc::strong_count(block) == 1 { *cursor = 0; }
            if size <= *capacity - *cursor {
                let offset = *cursor;
                *cursor += size;
                *used = now;
                return Ok((block.clone(), offset));
            }
        }
        // A large contiguous range uses a block rounded up to the normal block size.
        let capacity = size.checked_add(BLOCK_BYTES - 1).ok_or("lease size overflow")? / BLOCK_BYTES * BLOCK_BYTES;
        let block = create(capacity)?;
        self.blocks.push((block.clone(), size, capacity, now));
        Ok((block, 0))
    }

    /// Releases the blocks no lease or GPU work holds that went unleased for `BLOCK_IDLE_NS`:
    /// a burst of readbacks leaves no 64 MB blocks behind for the life of the process.
    pub fn trim(&mut self, now_ns: u64) {
        self.blocks.retain(|(block, _, _, used)| Arc::strong_count(block) > 1 || now_ns.saturating_sub(*used) < BLOCK_IDLE_NS);
    }
}
