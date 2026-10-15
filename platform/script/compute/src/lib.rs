//! The Splash compute core: a small, statically typed subset of Splash
//! (logic, bounded loops, integers, vectors, fixed arrays and structs,
//! helper functions) compiled ahead of time into AIR, a structured SSA IR,
//! and run by a reference interpreter or native code.
//!
//! ```text
//! Splash source --[parse]--> AST --[lower: types, inlining, math]--> AIR
//!     AIR --> opt              (kernels: forwarding, loop-invariant code
//!                               motion, if-conversion, CSE, dead code,
//!                               strength reduction, multiply-add fusion;
//!                               the same bits, FMA only in math: fast)
//!     AIR --> ir::run          (the reference; also the no-JIT path)
//!     AIR --> arm64            (native scalar code: audio shaders, kernels)
//!     AIR --> neon             (kernels: 4 elements per iteration)
//!     AIR --> wasm             (browsers: scalar, and SIMD128 x4 for kernels)
//!     AIR --> js               (browsers without wasm compilation)
//! ```
//!
//! Front ends decide what a program's entry is and what it can read and
//! write: audio shaders (makepad-script-audio-aot: voices and effects on
//! the audio thread) and geometry kernels ([`geometry`]: per-element
//! kernels over many vertices or instances). Every backend is
//! bit-identical to the interpreter: AIR's ops are total and exactly
//! specified (a fused multiply-add is one rounding everywhere) and math
//! functions are polynomials of those ops (no libm).
use makepad_relax_api::relax_api;

pub mod cost;
pub mod boundary;
pub mod host;
pub mod ir;
pub mod js;
pub mod kernel;
pub mod lower;
pub mod module;
#[cfg(feature = "vm")]
pub mod object;
pub mod opt;
pub mod parse;
pub mod rand;
pub mod work;
mod spmd;
#[cfg(feature = "vm")]
pub mod vm_kernel;
pub mod wasm;

#[cfg(target_arch = "aarch64")]
pub mod arm64;
#[cfg(target_arch = "aarch64")]
pub mod neon;
/// Native code through the shared back end (makepad-codegen): AIR -> RIR -> x64 / arm64.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub mod native;
/// Four elements per iteration through the shared back end (SSE4.1/AVX2, NEON).
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub mod native_vec;
/// The host's scalar native backend: compute's own emitter on AArch64 (until the shared
/// path replaces it, loader-core.md step 6), the shared back end on x86-64.
/// (Feature `shared-backend`: the shared back end on AArch64 too.)
#[cfg(all(target_arch = "aarch64", not(feature = "shared-backend")))]
pub use arm64 as host_native;
#[cfg(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "shared-backend")))]
pub use native as host_native;
/// The host's four-wide backend: NEON x4 on AArch64 (until step 3), the shared back end's
/// SSE4.1/AVX2 vectors on x86-64.
#[cfg(all(target_arch = "aarch64", not(feature = "shared-backend")))]
pub use neon as host_simd;
#[cfg(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "shared-backend")))]
pub use native_vec as host_simd;

/// A compile error with a byte span into the source it came from.
#[derive(Debug)]
pub struct ShaderError {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

impl ShaderError {
    #[inline(always)]
    pub fn new(start: usize, end: usize, message: String) -> Self {
        ShaderError { start, end, message }
    }

    /// 1-based line and column of the error start in `src`.
    #[inline(always)]
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        let at = self.start.min(src.len());
        let before = &src[..at];
        let line = before.matches('\n').count() + 1;
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        (line, col)
    }
}

// These diagnostics are also materialized by caller-side compile wrappers.
// Their String never crosses a bound signature; ErrorView borrows the text.
impl Clone for ShaderError {
    #[inline(always)]
    fn clone(&self) -> Self {
        Self { start: self.start, end: self.end, message: self.message.clone() }
    }
}

impl PartialEq for ShaderError {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.end == other.end && self.message == other.message
    }
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at {}..{})", self.message, self.start, self.end)
    }
}

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Backend {
    /// The AIR interpreter (the reference; the path where executable
    /// memory is unavailable).
    Interp,
    /// Native machine code for the host (ARM64 today).
    Native,
}

/// Parses a prelude (a library every program of a front end sees), with
/// its spans placed after `user_len` bytes of user code so an error's span
/// tells which side it is on.
pub fn parse_prelude(src: &str, user_len: usize) -> Vec<parse::Item> {
    let mut toks = parse::lex(src).expect("the prelude lexes");
    for t in &mut toks {
        t.start += user_len + 1;
        t.end += user_len + 1;
    }
    parse::Parser::new(&toks).items().expect("the prelude parses")
}

/// The prelude's items, then the program's own. The program's names shadow
/// the prelude's for the program only: a prelude item the program redefines
/// (`let fade = param(1.0)` over the prelude's `fade`) stays, as
/// `prelude.<name>`, for the prelude's own bodies and for the library items
/// (modules) among `items`, so a user's name never leaks into a library's
/// bodies. `own` names the program's own items (the rest of `items` came
/// from modules).
pub fn with_prelude(items: &[parse::Item], prelude: Vec<parse::Item>, own: &std::collections::HashSet<String>) -> Vec<parse::Item> {
    let names: std::collections::HashSet<String> = items.iter().map(|i| i.name().to_string()).collect();
    let shadowed: std::collections::HashMap<String, String> =
        prelude.iter().filter(|i| names.contains(i.name())).map(|i| (i.name().to_string(), format!("prelude.{}", i.name()))).collect();
    let mut all: Vec<parse::Item> = prelude;
    all.extend(items.iter().cloned());
    if shadowed.is_empty() {
        return all;
    }
    let n_prelude = all.len() - items.len();
    for (k, it) in all.iter_mut().enumerate() {
        if k < n_prelude {
            module::rename_refs(it, &shadowed);
            if let Some(q) = shadowed.get(it.name()) {
                module::set_name(it, q.clone());
            }
        } else if !own.contains(it.name()) {
            module::rename_refs(it, &shadowed);
        }
    }
    all
}

