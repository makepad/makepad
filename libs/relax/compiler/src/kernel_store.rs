//! Compiled compute kernels in the Relax disk store: one more entry type (KIND_KERNEL) in a
//! store file of the cache folder (`kernels-<16 hex>.rcs`, the hex = the codegen identity, the compute crate's source identity, target
//! and the entry format), so the store's commit protocol (a torn or partial append reads as
//! absent), its generations, its prune and its cap apply unchanged. The key is the kernel
//! program's identity (makepad_script_compute::kcache::key: the AIR, the back end, the
//! optimiser switches and CPU features); the identities are in the file name, so a build with
//! another kernel compiler never opens this build's code. `makepad run` installs `get` / `put` into the
//! compute crate (tools/run). A load is lazy: a kernel's entry is read when that kernel is built.
//!
//! Offered entries are queued and appended in batches (a store append rewrites the small index
//! and flips the header, so one per kernel would be a commit each): when a batch is large
//! enough or old enough, and at the quit write.

use crate::pack::NewEntry;
use crate::store::Store;
use relax_session::sync::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

const KIND_KERNEL: u32 = 15;
const FORMAT: u32 = 1;
/// A batch of this many entries, or this many payload bytes, is appended at once.
const BATCH_ENTRIES: usize = 16;
const BATCH_BYTES: usize = 8 << 20;
/// A queued entry is appended at the next offer after this long.
const BATCH_AGE_MS: u128 = 1500;

struct State {
    open: Option<Arc<Store>>,
    pending: Vec<NewEntry>,
    bytes: usize,
    oldest: Option<std::time::Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { open: None, pending: Vec::new(), bytes: 0, oldest: None });

/// The kernel compiler's identity: the source identity of the compute crate this binary was built
/// with (the loader sets it with the hook: tools/run). The kernels' code comes from that crate, not
/// from Relax's own code generator, so it keys the store next to the codegen identity.
static KERNEL_COMPILER: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

pub fn set_kernel_compiler(id: u64) {
    let _ = KERNEL_COMPILER.set(id);
}

fn enabled() -> bool {
    KERNEL_COMPILER.get().is_some()
        && !matches!(std::env::var("MAKEPAD_RUN_CACHE").ok().as_deref(), Some("off") | Some("0"))
        && !matches!(std::env::var("MAKEPAD_RUN_KERNEL_CACHE").ok().as_deref(), Some("off") | Some("0"))
}

fn path() -> PathBuf {
    let mut h = crate::sip::Sip128::new();
    h.str("kernel-store");
    h.u128(crate::codegen_id::id());
    h.u64(*KERNEL_COMPILER.get().expect("kernel compiler identity"));
    h.str(&crate::codegen_id::target_tag());
    h.u32(FORMAT);
    crate::dirs::relax_dir().join("cache").join(format!("kernels-{:016x}.rcs", h.finish128() as u64))
}

/// The stored payload of kernel `key` (None: absent, or torn).
pub fn get(key: u128) -> Option<Vec<u8>> {
    if !enabled() {
        return None;
    }
    let _kept = crate::zone_alloc::kept();
    let store = {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if s.open.is_none() {
            s.open = Store::open(&path()).map(Arc::new);
        }
        s.open.clone()?
    };
    let (kind, bytes) = store.get(key)?;
    (kind == KIND_KERNEL).then(|| bytes.to_vec())
}

/// Offers a freshly compiled kernel's payload for the store.
pub fn put(key: u128, payload: Vec<u8>) {
    if !enabled() {
        return;
    }
    let _kept = crate::zone_alloc::kept();
    let batch = {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        s.bytes += payload.len();
        s.pending.push(NewEntry { key, kind: KIND_KERNEL, payload });
        s.oldest.get_or_insert_with(std::time::Instant::now);
        let due = s.pending.len() >= BATCH_ENTRIES || s.bytes >= BATCH_BYTES || s.oldest.is_some_and(|t| t.elapsed().as_millis() >= BATCH_AGE_MS);
        due.then(|| take(&mut s))
    };
    if let Some(batch) = batch {
        append(batch);
    }
}

/// Appends what is queued (the quit write).
pub fn flush() {
    let _kept = crate::zone_alloc::kept();
    let batch = take(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()));
    append(batch);
}

fn take(s: &mut State) -> Vec<NewEntry> {
    s.bytes = 0;
    s.oldest = None;
    std::mem::take(&mut s.pending)
}

fn append(batch: Vec<NewEntry>) {
    if batch.is_empty() {
        return;
    }
    let path = path();
    if let Err(e) = crate::store::append(&path, batch, false) {
        if std::env::var_os("MAKEPAD_RUN_VERBOSE").is_some() {
            eprintln!("makepad-run cache: kernel store {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Offered kernels reach the store file on a flush, read back byte for byte, an unknown key
    /// is absent, and a file cut short (a torn write) is absent, not an error.
    #[test]
    fn kernels_round_trip_through_the_store_and_a_torn_file_is_absent() {
        isolated_test!(kernels_round_trip_through_the_store_and_a_torn_file_is_absent);
        let dir = std::env::temp_dir().join(format!("relax-kernel-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("MAKEPAD_RUN_CACHE_DIR", &dir);
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        for k in 1..=20u128 {
            put(k, vec![k as u8; 1000 + k as usize]);
        }
        flush();
        for k in 1..=20u128 {
            assert_eq!(get(k).unwrap(), vec![k as u8; 1000 + k as usize]);
        }
        assert!(get(99).is_none());
        // cut the file inside its payloads: the header it commits no longer fits
        let file = path();
        let len = std::fs::metadata(&file).unwrap().len();
        STATE.lock().unwrap().open = None;
        std::fs::OpenOptions::new().write(true).open(&file).unwrap().set_len(len / 2).unwrap();
        assert!(get(5).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
