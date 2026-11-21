//! Verification, optimisation, emission and executable memory for code outside Relax's Rust JIT.

#[cfg(target_arch = "aarch64")]
use crate::arm64 as target;
#[cfg(not(target_arch = "aarch64"))]
use crate::x64 as target;
use crate::{passes, rir};

/// The passes to run before standalone code is emitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Passes {
    /// The tier-0 passes as Relax's baseline runs them: folding, copy propagation, CSE, DCE, SROA
    /// of the frame, LICM, block layout, address modes, loop rotation; after LICM, value numbering
    /// of pure values (passes::gvn_pure). No fusion, no float reassociation. `RELAX_OPT=none` (or `-fold`, `-cse`, `-sroa`, `-licm`, `-imm`,
    /// `-layout`, `-gvn`) turns passes off.
    Kernel,
    /// Many small blocks that must compile fast. No SROA, LICM, value numbering or loop rotation:
    /// compile time matters, and side exits and helper calls leave little to hoist across blocks.
    Blocks,
}

/// Runs the preset with the pass switches in `o` (`RELAX_OPT`).
pub fn optimise(o: &relax_session::Options, f: &mut rir::Func, passes: Passes) {
    let none = o.opt.len() == 1 && o.opt[0] == "none";
    let on = |tok: &str| !none && !crate::stats::opt_off(o, tok);
    let (fold, cse, sroa, licm, imm, layout, gvn) = (on("-fold"), on("-cse"), on("-sroa"), on("-licm"), on("-imm"), on("-layout"), on("-gvn"));
    match passes {
        Passes::Kernel => {
            if fold {
                passes::simplify(o, f, None, imm, cse);
                if sroa && passes::sroa(f) {
                    // the cells' entry zeros that a write always precedes are dead by liveness only
                    passes::live_dce(f);
                    passes::simplify(o, f, None, imm, cse);
                }
                // LICM gathers the loop's invariant lane extracts in its preheader; local CSE's bounded
                // table leaves most repeats there when there are thousands; value numbering merges them
                let hoisted = licm && passes::licm(o, f);
                let numbered = cse && gvn && passes::gvn_pure(f);
                if hoisted || numbered {
                    passes::simplify(o, f, None, imm, cse);
                }
                passes::sink_consts(f);
            }
            if layout {
                passes::relayout(f);
            }
            if fold {
                passes::addr_modes(f);
            }
            if layout {
                passes::rotate_loops(f);
            }
        }
        Passes::Blocks => {
            if fold {
                passes::simplify(o, f, None, imm, cse);
                passes::live_dce(f);
                passes::sink_consts(f);
            }
            if layout {
                passes::relayout(f);
            }
            if fold {
                passes::addr_modes(f);
            }
        }
    }
}

/// The register allocation to use when emitting standalone code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alloc {
    /// One quick allocation round.
    Quick,
    /// Thorough allocation with coalescing and live-range splitting.
    Thorough,
}

/// Standalone code and the layout information kept after linking.
pub struct Emitted {
    /// the linked code, mapped executable
    pub code: crate::exec::Code,
    /// bytes of instructions (a constant pool follows)
    pub text_len: u32,
    /// the relocated fields, as linked
    pub relocs: Vec<rir::RelocAt>,
}

/// Emits optimised RIR, links its relocations through `resolve` and maps the code executable.
/// None means no code generation, an emitter or linker error, a refused relocation, or no
/// executable memory (wasm, a locked-down host); the caller runs another tier then, and this
/// is not an error.
pub fn emit(o: &relax_session::Options, f: &rir::Func, alloc: Alloc, resolve: &dyn Fn(rir::Reloc) -> Option<u64>) -> Option<Emitted> {
    if o.no_codegen {
        return None;
    }
    let mut c = target::compile(o, f, &target::Env::standalone(alloc == Alloc::Thorough)).ok()?;
    if c.relocs.iter().any(|r| resolve(r.target).is_none()) {
        return None;
    }
    // Far symbols use pool words, so linking before mapping needs no base address.
    rir::link(&mut c.code, 0, &c.relocs, &|r| resolve(r).unwrap_or(0)).ok()?;
    let code = crate::exec::Code::new(&c.code)?;
    Some(Emitted { code, text_len: c.text_len, relocs: c.relocs })
}

/// Verifies RIR before and after optimisation, then emits, links and maps it. None lets the
/// caller run another tier, including when verification fails.
pub fn compile(o: &relax_session::Options, f: &mut rir::Func, passes: Passes, alloc: Alloc, resolve: &dyn Fn(rir::Reloc) -> Option<u64>) -> Option<Emitted> {
    f.verify().ok()?;
    optimise(o, f, passes);
    f.verify().ok()?;
    emit(o, f, alloc, resolve)
}

/// A function pointer with the emitter's ABI for standalone entries and host calls: System V
/// on x86-64 on every OS, the platform C ABI on AArch64. Windows' x86-64 C ABI passes arguments
/// in rcx/rdx/r8/r9, so using it would make the emitted code read the wrong arguments.
#[cfg(target_arch = "x86_64")]
#[macro_export]
macro_rules! host_fn_ptr {
    (fn($($arg:ty),* $(,)?) $(-> $ret:ty)?) => {
        unsafe extern "sysv64" fn($($arg),*) $(-> $ret)?
    };
}

/// A function pointer with the emitter's ABI for standalone entries and host calls: System V
/// on x86-64 on every OS, the platform C ABI on AArch64. Windows' x86-64 C ABI passes arguments
/// in rcx/rdx/r8/r9, so using it would make the emitted code read the wrong arguments.
#[cfg(not(target_arch = "x86_64"))]
#[macro_export]
macro_rules! host_fn_ptr {
    (fn($($arg:ty),* $(,)?) $(-> $ret:ty)?) => {
        unsafe extern "C" fn($($arg),*) $(-> $ret)?
    };
}

/// A function with the emitter's ABI for standalone entries and host calls: System V on
/// x86-64 on every OS, the platform C ABI on AArch64. Windows' x86-64 C ABI passes arguments
/// in rcx/rdx/r8/r9, so using it would make the emitted code read the wrong arguments.
#[cfg(target_arch = "x86_64")]
#[macro_export]
macro_rules! host_fn {
    ($(#[$m:meta])* $vis:vis unsafe fn $name:ident($($args:tt)*) $(-> $ret:ty)? $body:block) => {
        $(#[$m])* $vis unsafe extern "sysv64" fn $name($($args)*) $(-> $ret)? $body
    };
}

/// A function with the emitter's ABI for standalone entries and host calls: System V on
/// x86-64 on every OS, the platform C ABI on AArch64. Windows' x86-64 C ABI passes arguments
/// in rcx/rdx/r8/r9, so using it would make the emitted code read the wrong arguments.
#[cfg(not(target_arch = "x86_64"))]
#[macro_export]
macro_rules! host_fn {
    ($(#[$m:meta])* $vis:vis unsafe fn $name:ident($($args:tt)*) $(-> $ret:ty)? $body:block) => {
        $(#[$m])* $vis unsafe extern "C" fn $name($($args)*) $(-> $ret)? $body
    };
}
