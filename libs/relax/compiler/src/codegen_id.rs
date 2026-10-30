//! The codegen identity: the key under which every stored build, code store, front store and
//! kernel store lives (cache.rs, store.rs, kernel_store.rs; the Android host's cache wipe).
//!
//! It is a hash of exactly what decides the code Relax generates: the source stamp of every crate
//! of relax-compiler's library closure (the compiler, RIR, codegen, regalloc and backends, the
//! runtime ABI and its rt_* functions, the std image and its shims, the native macros), each with
//! the rustc version, target, CPU features, profile and flags it was compiled with
//! (libs/relax/self_stamp.rs; build.rs codegen_parts for the closure's non-Relax packages).
//! Each stamp is compiled into the rlib of the crate it describes, so a crate cargo did not
//! rebuild keeps its old stamp with its old code: the identity cannot be newer than the code.
//!
//! Anything else linked into the binary (devhub, platform glue, Desk Link, the loader's UI) is
//! not in it: changing it keeps every stored build. Provided crates (provided.rs) are keyed by
//! their binding identity (provided.rs binding_identity; cache/items.rs apply_bound), their source
//! is checked against the loader's at bind time. Stored
//! entries stay keyed per function by their own inputs (body, inline tree, verdicts).
use crate::sip::Sip128;

include!(concat!(env!("OUT_DIR"), "/codegen_parts.rs"));

// Every part names a stamp of its own: a crate whose SRC_STAMP resolves to another crate's (a glob
// re-export standing in for a missing include of its own stamp) would leave its source out.
const _: () = {
    let mut i = 0;
    while i < CODEGEN_PARTS.len() {
        let mut j = i + 1;
        while j < CODEGEN_PARTS.len() {
            assert!(CODEGEN_PARTS[i].1 != CODEGEN_PARTS[j].1, "two codegen identity parts share one stamp");
            j += 1;
        }
        i += 1;
    }
};

/// The target a stored code entry runs on: `<arch>-<os>`, and on arm64 the atomics the backend
/// emits (`lse` on ARMv8.1+, `llsc` exclusive-access loops without it): code stored on one CPU
/// class must never be loaded on the other (a cache seed shipped in an APK, a copied home).
pub fn target_tag() -> String {
    let mut t = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
    #[cfg(target_arch = "aarch64")]
    t.push_str(if relax_backend::arm64::lse_atomics() { "-lse" } else { "-llsc" });
    t
}

/// This build's codegen identity.
pub fn id() -> u128 {
    static ID: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let mut h = Sip128::new();
        h.str("relax-codegen-id");
        for (name, stamp) in CODEGEN_PARTS {
            h.str(name);
            h.u128(*stamp);
        }
        h.finish128()
    })
}
