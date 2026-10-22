//! Relax's entry into the machine-code back end (relax-backend): installs Relax's compile
//! profilers as the back end's probe, and on AArch64 sends the functions selected for the
//! threaded interpreter (MAKEPAD_RUN_INTERP / MAKEPAD_RUN_NO_CODEGEN) there instead.

use relax_backend::rir::Func;
#[cfg(target_arch = "aarch64")]
use relax_backend::arm64 as target;
#[cfg(not(target_arch = "aarch64"))]
use relax_backend::x64 as target;
use std::cell::RefCell;

struct RelaxProbe;

thread_local! {
    static PHASES: RefCell<Vec<crate::fncost::Phase>> = const { RefCell::new(Vec::new()) };
    static SCOPES: RefCell<Vec<crate::warm::Scope>> = const { RefCell::new(Vec::new()) };
}

impl relax_backend::probe::Probe for RelaxProbe {
    fn cost_enabled(&self) -> bool {
        crate::fncost::enabled()
    }
    // A function compiled while its thread's locals are being destroyed (a lazily compiled TLS
    // destructor) finds the stacks gone: its guards are then not kept (profiling only).
    fn phase_enter(&self, phase: usize) {
        let mut p = Some(crate::fncost::phase(phase));
        let _ = PHASES.try_with(|s| s.borrow_mut().push(p.take().unwrap()));
        std::mem::forget(p);
    }
    fn phase_leave(&self) {
        let p = PHASES.try_with(|s| s.borrow_mut().pop()).ok().flatten();
        drop(p);
    }
    fn backend_enter(&self, name: &str) {
        let mut s = Some(crate::warm::Scope::backend(name));
        let _ = SCOPES.try_with(|v| v.borrow_mut().push(s.take().unwrap()));
        std::mem::forget(s);
    }
    fn backend_leave(&self) {
        let s = SCOPES.try_with(|v| v.borrow_mut().pop()).ok().flatten();
        drop(s);
    }
    fn size(&self, f: &Func) {
        crate::fncost::size(false, f);
    }
    fn code(&self, bytes: usize) {
        crate::fncost::code(bytes);
    }
}

/// Installs Relax's profilers as the back end's probe (idempotent; one load once installed).
#[inline]
pub fn install_probe() {
    static P: RelaxProbe = RelaxProbe;
    relax_backend::probe::install(&P);
}

pub fn compile(options: &relax_session::Options, f: &Func, env: &target::Env) -> Result<target::Compiled, String> {
    compile_freq(options, f, env, None)
}

/// `compile` with measured block frequencies of `f` (0..=1 of its hottest block, per block), which
/// the register allocator weighs uses by instead of loop depth.
pub fn compile_freq(options: &relax_session::Options, f: &Func, env: &target::Env, freq: Option<&[f32]>) -> Result<target::Compiled, String> {
    let mut c = compile_freq_inner(options, f, env, freq)?;
    // the inline stack goes with the code, in its pc map (rir::inline_pc_entries)
    if !f.inl.is_empty() && !c.pc_map.is_empty() {
        c.pc_map.extend(relax_backend::rir::inline_pc_entries(f));
    }
    Ok(c)
}

fn compile_freq_inner(options: &relax_session::Options, f: &Func, env: &target::Env, freq: Option<&[f32]>) -> Result<target::Compiled, String> {
    install_probe();
    // the wasm32 web target's program (web32.rs): pointers are offsets into its window
    if let Some(c) = crate::web32::applies(env.syms) {
        let mut g = f.clone();
        crate::web32::transform(&mut g, &c, env.sym_class)?;
        return compile_native(options, &g, env, freq);
    }
    compile_native(options, f, env, freq)
}

fn compile_native(options: &relax_session::Options, f: &Func, env: &target::Env, freq: Option<&[f32]>) -> Result<target::Compiled, String> {
    #[cfg(target_arch = "aarch64")]
    {
        // the function runs in the threaded interpreter (interp.rs, MAKEPAD_RUN_INTERP /
        // MAKEPAD_RUN_NO_CODEGEN): its "code" is the interpreter's relocatable form (interp/blob.rs)
        if let Some((code, relocs)) = crate::interp::maybe_compile(f, env.slots, env.syms, env.sym_class, env.tls_slow, env.resume) {
            let n = code.len() as u32;
            return Ok(target::Compiled { code, pc_map: Vec::new(), frame_size: 0, text_len: n, alloc_sig: 0, relocs, rir_map: Vec::new(), rir_emitted: String::new() });
        }
        // research (ios-interp): MAKEPAD_RUN_INTERP_STATS=1 also sums the machine-code backend's
        // time and output for functions compiled natively, to compare with the interpreter's lowering
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_INTERP_STATS").is_some()) {
            let t = std::time::Instant::now();
            let r = target::compile_freq(options, f, env, freq);
            if let Ok(c) = &r {
                crate::interp::note_native(t.elapsed().as_nanos() as u64, c.code.len() as u64, f.blocks.iter().map(|b| b.insts.len() as u64 + 1).sum());
            }
            return r;
        }
    }
    target::compile_freq(options, f, env, freq)
}
