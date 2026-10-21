//! Compiled-kernel store: machine code of a kernel program kept on disk between runs by the
//! embedder (`makepad run` installs the hook; plain builds have none and compile every time).
//!
//! The key names everything the emitted bytes depend on: the program (its whole AIR, the host
//! buffer write mask), the back end (scalar or four-wide), the optimiser switches and the CPU
//! features the emitter reads. The embedder adds the loader's build id (the store it hands out
//! is that build's), so another build, another program or another CPU never meets old code.
//! The code is position independent except for the host-call address (the trampoline into the
//! interpreter): the back end's pool words for it, recorded as `host_refs` and written at load.

use crate::ir::Program;
use std::fmt::Write;
use std::sync::OnceLock;

/// The embedder's store: `get` returns a stored payload, `put` offers a new one (the embedder
/// decides when it reaches the disk).
#[derive(Clone, Copy)]
pub struct Hook {
    pub get: fn(u128) -> Option<Vec<u8>>,
    pub put: fn(u128, Vec<u8>),
}

static HOOK: OnceLock<Hook> = OnceLock::new();

/// Installs the store (once per process); false when one is installed already.
pub fn install(hook: Hook) -> bool {
    HOOK.set(hook).is_ok()
}

/// `MAKEPAD_RUN_KERNEL_CACHE=verify`: every kernel found in the store is also compiled and the two
/// machine codes compared (they must be the same bytes; the process ends on a difference).
pub(crate) fn verify() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("MAKEPAD_RUN_KERNEL_CACHE").is_ok_and(|v| v == "verify"))
}

pub(crate) fn same(back_end: &str, stored: &[u8], fresh: &[u8]) {
    static CHECKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if stored != fresh {
        eprintln!("kernel cache VERIFY FAILED: {back_end} kernel: stored {} bytes, compiled {} bytes, first difference at {:?}", stored.len(), fresh.len(), stored.iter().zip(fresh).position(|(a, b)| a != b));
        std::process::exit(86);
    }
    let n = CHECKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n % 20 == 0 || n == 1 {
        eprintln!("kernel cache verify: {n} stored kernels equal their compile");
    }
}

pub(crate) fn hook() -> Option<&'static Hook> {
    HOOK.get()
}

const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"KC01";

/// 128 bits of two independent 64-bit streams (the files are private: corruption and
/// accidental collision, not an adversary).
struct Hash128 {
    a: u64,
    b: u64,
}

impl Hash128 {
    fn new() -> Self {
        Hash128 { a: 0xcbf2_9ce4_8422_2325, b: 0x9e37_79b9_7f4a_7c15 }
    }
    fn bytes(&mut self, s: &[u8]) {
        for &x in s {
            self.a = (self.a ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3);
            self.b = (self.b ^ x as u64).wrapping_mul(0xff51_afd7_ed55_8ccd).rotate_left(23);
        }
    }
    fn finish(self) -> u128 {
        let mix = |mut x: u64| {
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
            x ^= x >> 33;
            x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
            x ^ (x >> 33)
        };
        ((mix(self.a ^ self.b.rotate_left(17)) as u128) << 64) | mix(self.b ^ self.a.rotate_left(41)) as u128
    }
}

impl Write for Hash128 {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

/// The identity of the machine code `back_end` makes of `p` with host buffer write mask
/// `writable`. None without a store (nothing to look up or keep).
pub(crate) fn key(back_end: &str, p: &Program, writable: u64) -> Option<u128> {
    hook()?;
    let mut h = Hash128::new();
    h.bytes(b"kernel-code");
    h.bytes(&FORMAT.to_le_bytes());
    h.bytes(back_end.as_bytes());
    h.bytes(std::env::consts::ARCH.as_bytes());
    h.bytes(&writable.to_le_bytes());
    // what the emitter and the passes read besides the program: the pass switches, the
    // instruction set limits, the CPU's features
    let o = crate::native::options();
    for t in &o.opt {
        h.bytes(t.as_bytes());
        h.bytes(&[0]);
    }
    h.bytes(&[o.no_avx as u8, o.no_bmi2 as u8, o.no_bitops as u8, o.memconst as u8]);
    h.bytes(&cpu_features().to_le_bytes());
    // (Debug of the program is a plain tree of vectors: deterministic)
    let _ = write!(h, "{:?}", p);
    Some(h.finish())
}

#[cfg(target_arch = "x86_64")]
fn cpu_features() -> u64 {
    let mut m = 0u64;
    macro_rules! f { ($($n:tt),*) => { $( m = (m << 1) | std::is_x86_feature_detected!($n) as u64; )* }; }
    f!("sse4.1", "sse4.2", "avx", "avx2", "fma", "bmi1", "bmi2", "popcnt", "lzcnt");
    m
}
#[cfg(not(target_arch = "x86_64"))]
fn cpu_features() -> u64 {
    0
}

fn check(b: &[u8]) -> u64 {
    let mut h = Hash128::new();
    h.bytes(b);
    let v = h.finish();
    (v >> 64) as u64 ^ v as u64
}

/// Payload: magic, host-ref count, refs, code length, code, checksum of all before it.
pub(crate) fn encode(code: &[u8], host_refs: &[u32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(code.len() + 4 * host_refs.len() + 24);
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&(host_refs.len() as u32).to_le_bytes());
    for r in host_refs {
        v.extend_from_slice(&r.to_le_bytes());
    }
    v.extend_from_slice(&(code.len() as u32).to_le_bytes());
    v.extend_from_slice(code);
    let sum = check(&v);
    v.extend_from_slice(&sum.to_le_bytes());
    v
}

/// The code and its host refs; None for anything torn, truncated or not ours.
pub(crate) fn decode(b: &[u8]) -> Option<(Vec<u8>, Vec<u32>)> {
    let body = b.get(..b.len().checked_sub(8)?)?;
    if u64::from_le_bytes(b[b.len() - 8..].try_into().ok()?) != check(body) || body.get(..4)? != MAGIC {
        return None;
    }
    let n = u32::from_le_bytes(body.get(4..8)?.try_into().ok()?) as usize;
    let refs_end = 8usize.checked_add(n.checked_mul(4)?)?;
    let refs: Vec<u32> = body.get(8..refs_end)?.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let len = u32::from_le_bytes(body.get(refs_end..refs_end + 4)?.try_into().ok()?) as usize;
    let code = body.get(refs_end + 4..)?;
    (code.len() == len && refs.iter().all(|&r| (r as usize).checked_add(8).is_some_and(|e| e <= len))).then(|| (code.to_vec(), refs))
}

/// The address host calls in stored code are patched to.
pub(crate) fn host_addr() -> u64 {
    let o = HOST_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if o != 0 {
        return o;
    }
    crate::host::trampoline as *const () as usize as u64
}

static HOST_OVERRIDE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tests: stored code is re-targeted at `addr` (0: the real trampoline), as when the loader
/// is mapped elsewhere in the next run.
#[doc(hidden)]
pub fn override_host_addr(addr: u64) {
    HOST_OVERRIDE.store(addr, std::sync::atomic::Ordering::Relaxed);
}
