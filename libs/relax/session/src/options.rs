//! Frozen output inputs, owned by the driver and passed to compiler consumers.
//! rustc: compiler/rustc_session/src/session.rs:90-93 (session ownership),
//! compiler/rustc_session/src/options.rs:222-259 (whole-options dependency hash).
use crate::sip::Sip128;

/// The error inline assembly in crate `krate` gets under `--no-asm`.
pub fn asm_blocked(krate: &str) -> String {
    format!("inline assembly (asm!, global_asm!, naked_asm!) in crate `{}` is blocked by `--no-asm` (MAKEPAD_RUN_NO_ASM=1)", krate)
}
/// The error a program whose dependencies compile C gets without `--allow-c`.
pub const C_GATE: &str = "is built only with `--allow-c` (or MAKEPAD_RUN_ALLOW_C=1): cargo and the system C compiler build it, and its library is linked into the run";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Borrowck { Off, Moves, Full }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorrowckAll { Default, Nongeneric, Std, All }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckTarget { Host, MacosArm64, LinuxX64, WindowsX64, AndroidArm64, Web(PtrWidth) }
/// The web target's pointer width, a parameter of the target (`wasm32-unknown-unknown` /
/// `wasm64-unknown-unknown`): usize, isize and pointers are 4 or 8 bytes, every layout is rustc's
/// for that triple, and the trace host runs the program in that mode (32: pointers are offsets into
/// the linear memory's window; 64: native pointers inside it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtrWidth { W32, W64 }
/// Thread profile of a web target. The default wasm target and exported pages are
/// single-threaded; cargo-makepad-style checks opt into atomics explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebThreads { Off, On }
impl PtrWidth {
    /// size_of::<usize>() (and of a thin pointer)
    pub fn bytes(self) -> u32 {
        match self {
            PtrWidth::W32 => 4,
            PtrWidth::W64 => 8,
        }
    }
}
impl CheckTarget {
    /// The targets Relax runs on (and checks for), by triple: `MAKEPAD_RUN_CHECK_TARGET` and `makepad host-gen`.
    pub const TRIPLES: [(&'static str, CheckTarget); 4] = [
        ("aarch64-apple-darwin", CheckTarget::MacosArm64),
        ("x86_64-unknown-linux-gnu", CheckTarget::LinuxX64),
        ("x86_64-pc-windows-gnu", CheckTarget::WindowsX64),
        ("aarch64-linux-android", CheckTarget::AndroidArm64),
    ];
    /// The web targets, by pointer width: the program's crates are read as a web build reads
    /// them (cfg(target_family = "wasm"), the web platform backend), every layout is rustc's for
    /// the triple, and this machine's JIT runs them in the trace host (`makepad-web-trace`). Not
    /// host-gen targets.
    pub const WEB_TRIPLES: [(&'static str, PtrWidth); 2] = [("wasm32-unknown-unknown", PtrWidth::W32), ("wasm64-unknown-unknown", PtrWidth::W64)];
    /// The web target's pointer width, None for the native targets.
    pub fn web(self) -> Option<PtrWidth> {
        match self {
            CheckTarget::Web(w) => Some(w),
            _ => None,
        }
    }
    /// The triple of a web target.
    pub fn web_triple(w: PtrWidth) -> &'static str {
        Self::WEB_TRIPLES.iter().find(|(_, x)| *x == w).unwrap().0
    }
    pub fn from_triple(triple: &str) -> Option<CheckTarget> {
        if let Some((_, w)) = Self::WEB_TRIPLES.iter().find(|(t, _)| *t == triple) {
            return Some(CheckTarget::Web(*w));
        }
        Self::TRIPLES.iter().find(|(t, _)| *t == triple).map(|(_, c)| *c)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctfe { Off, Check, On }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency { Sampled, Static, Blend }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority { Length, References, Sqrt }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerQos { Default, Background, Utility }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier0Baseline { Off, Startup, Always }
/// How much the JIT compiles ahead of the program's calls (`makepad run --ahead=<level>`,
/// `MAKEPAD_RUN_AHEAD`). Every level compiles what runs on its first call and tiers hot code up to
/// O3; they differ in the work done for code that has not run yet (jit::AheadPolicy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AheadLevel {
    /// iteration: only what ran (plus the startup image and O3 for what is hot)
    Minimal,
    /// the default: a bounded, prioritised compile-ahead of what the UI is about to need
    Balanced,
    /// a background AOT pass at worker priority: everything reachable from what ran, on every
    /// worker the policy allows
    AotMax,
    /// the same coverage as AotMax, slowly: few workers, every background compile thread in the
    /// OS's background class (macOS QOS_CLASS_BACKGROUND on efficiency cores, Linux SCHED_IDLE,
    /// Windows EcoQoS); left running a few minutes, the warm cache then holds all of it
    AotMin,
}
impl AheadLevel {
    pub fn name(self) -> &'static str {
        match self { AheadLevel::Minimal => "minimal", AheadLevel::Balanced => "balanced", AheadLevel::AotMax => "aot-max", AheadLevel::AotMin => "aot-min" }
    }
    pub fn parse(v: &str) -> Option<AheadLevel> {
        match v { "minimal" => Some(AheadLevel::Minimal), "balanced" => Some(AheadLevel::Balanced), "aot-max" => Some(AheadLevel::AotMax), "aot-min" => Some(AheadLevel::AotMin), _ => None }
    }
}

/// Effective machine capabilities; detected once by codegen before freezing Options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuFeatures {
    pub avx: bool,
    pub bmi2: bool,
    pub popcnt: bool,
    pub lzcnt: bool,
    pub bmi1: bool,
    /// FMA3 (vfmadd*) next to AVX: the fused multiply-add intrinsics lower to hardware
    pub fma: bool,
    /// AVX2 next to AVX: per-lane variable vector shifts (vpsllv*, vpsrlv*, vpsravd)
    pub avx2: bool,
    /// AVX-512F + AVX-512VL (OS-enabled): EVEX encodings reach xmm16..xmm31, so the register
    /// allocator gets 16 more float/vector registers (x64 reg_config)
    pub avx512: bool,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// `--verbose` / `MAKEPAD_RUN_VERBOSE=1`: informational diagnostics only, not a codegen input.
    pub verbose: bool,
    /// `MAKEPAD_RUN_CHECK_WINDOWS`; default `false`.
    pub check_windows: bool,
    /// `--no-asm` / `MAKEPAD_RUN_NO_ASM=1` (sandboxed uses): `asm!`, `global_asm!` and
    /// `naked_asm!` are an error naming the crate and the flag. Default `false`: inline assembly
    /// is assembled (backend asm).
    pub no_asm: bool,
    /// `--allow-c` / `MAKEPAD_RUN_ALLOW_C=1`: dependencies whose build script compiles C are built
    /// by cargo and the system C compiler and linked in (front native.rs); without it they are an
    /// error naming the flag. Default `false`.
    pub allow_c: bool,
    /// `MAKEPAD_RUN_GATHER`; default `false`.
    pub gather: bool,
    /// `MAKEPAD_RUN_MEMCONST`; default `false`.
    pub memconst: bool,
    /// `MAKEPAD_RUN_NO_AVX`; default `false`.
    pub no_avx: bool,
    /// `MAKEPAD_RUN_NO_AVX512`; default `false`: no xmm16..31 (CpuFeatures::avx512).
    pub no_avx512: bool,
    /// `MAKEPAD_RUN_NO_BITOPS`; default `false`.
    pub no_bitops: bool,
    /// `MAKEPAD_RUN_NO_BLOCK_FREQ`; default `false`.
    pub no_block_freq: bool,
    /// `MAKEPAD_RUN_NO_BMI2`; default `false`.
    pub no_bmi2: bool,
    /// `MAKEPAD_RUN_NO_BOUND_INFER`; default `false`.
    pub no_bound_infer: bool,
    /// `MAKEPAD_RUN_NO_COALESCE`; default `false`.
    pub no_coalesce: bool,
    /// `MAKEPAD_RUN_NO_FULL_UNROLL`; default `false`.
    pub no_full_unroll: bool,
    /// `MAKEPAD_RUN_NO_IV_RANGES`; default `false`.
    pub no_iv_ranges: bool,
    /// `MAKEPAD_RUN_NO_JUMP_TABLES`; default `false`.
    pub no_jump_tables: bool,
    /// `MAKEPAD_RUN_NO_LAYOUT_FN`; default `None`.
    pub no_layout_fn: Option<String>,
    /// `MAKEPAD_RUN_NO_NOALIAS`; default `false`.
    pub no_noalias: bool,
    /// `MAKEPAD_RUN_NO_QUICK_RA`; default `false`: baseline code takes the full register allocator.
    pub no_quick_ra: bool,
    /// `MAKEPAD_RUN_NO_RANGES`; default `false`.
    pub no_ranges: bool,
    /// `MAKEPAD_RUN_NO_RCX`; default `false`.
    pub no_rcx: bool,
    /// `MAKEPAD_RUN_NO_REMAT`; default `false`.
    pub no_remat: bool,
    /// `MAKEPAD_RUN_NO_SIMD`; default `false`.
    pub no_simd: bool,
    /// `MAKEPAD_RUN_NO_SLOT_SHARE`; default `false`.
    pub no_slot_share: bool,
    /// `MAKEPAD_RUN_NO_SPLIT`; default `false`.
    pub no_split: bool,
    /// `MAKEPAD_RUN_NO_STORE_MERGE`; default `false`.
    pub no_store_merge: bool,
    /// `MAKEPAD_RUN_NO_THROUGH`; default `false`.
    pub no_through: bool,
    /// `MAKEPAD_RUN_NO_UNROLL`; default `false`.
    pub no_unroll: bool,
    /// `MAKEPAD_RUN_NO_VECTORIZE`; default `false`.
    pub no_vectorize: bool,
    /// `MAKEPAD_RUN_NO_WINDOWS`; default `false`.
    pub no_windows: bool,
    /// `MAKEPAD_RUN_O3_QUICK_RA`; default `false`.
    pub t1_quick_ra: bool,
    /// `MAKEPAD_RUN_O1`: default startup; `0` disables; `always` exercises
    /// baseline in standalone test/check drivers which have no startup phase.
    pub t0_baseline: Tier0Baseline,
    /// `MAKEPAD_RUN_BORROWCK_INSTANCES`; default `false`.
    pub borrowck_instances: bool,
    /// `MAKEPAD_RUN_LIVE_STREAM`; default `false`.
    pub live_stream: bool,
    /// `MAKEPAD_RUN_GANDALF`; default `true`.
    pub gandalf: bool,
    /// `MAKEPAD_RUN_DERIVES`; default `true`.
    pub derives: bool,
    /// `MAKEPAD_RUN_LAZY_BODIES`; default `true`.
    pub lazy_bodies: bool,
    /// Eager source selection: false for every user-facing mode (run, check, serve). Set only
    /// internally (single-crate loaders without a preload graph, MAKEPAD_RUN_LAZY_VERIFY's eager
    /// replay); its deletion is the next step. Part of the compiler/cache identity.
    pub source_eager: bool,
    /// Outline Rust modules and select them in the demand closure; the post-boot loader brings
    /// in the rest. The one user-facing mode (true).
    pub rs_lazy: bool,
    /// `MAKEPAD_RUN_LAZY_MACRO_OUTPUT`; default `true`.
    pub lazy_macro_output: bool,
    /// `MAKEPAD_RUN_NATIVE_BITFLAGS`; default `true`.
    pub native_bitflags: bool,
    /// `MAKEPAD_RUN_PRE_DERIVES`; default `true`.
    pub pre_derives: bool,
    /// `MAKEPAD_RUN_SANDBOX_FUEL`; default `true`.
    pub sandbox_fuel: bool,
    /// `MAKEPAD_RUN_SPEC`; default `true`.
    pub spec: bool,
    /// `MAKEPAD_RUN_O3`; default `true`.
    pub tier1: bool,
    /// `MAKEPAD_RUN_TYCK_AHEAD`; default `true`.
    pub tyck_ahead: bool,
    /// `MAKEPAD_RUN_UNWIND`; default `true`.
    pub unwind: bool,
    /// `MAKEPAD_RUN_NO_CODEGEN=1`: the process makes no machine code (the platforms without
    /// writable+executable memory, iOS): every function runs in the threaded interpreter
    /// (codegen interp.rs), the runtime glue is the precompiled set (jit/static_glue.rs), and any
    /// write to code memory is an error. Implies no O3; the code cache holds the interpreter's
    /// relocatable form (codegen interp/blob.rs). Default `false`.
    pub no_codegen: bool,
    /// A live session (hot reload, watcher): `makepad run` of a working tree, `makepad live`; never
    /// from the environment. Default `false`.
    pub live: bool,
    /// `MAKEPAD_RUN_MODULES_TRUSTED`; default `false`.
    pub modules_trusted: bool,
    /// `MAKEPAD_RUN_SANDBOX_NOCHECK`; default `false`.
    pub sandbox_nocheck: bool,
    /// `MAKEPAD_RUN_BORROWCK_ONLY`; default `None`.
    pub borrowck_only: Option<String>,
    /// `MAKEPAD_RUN_LAYOUT_ONLY`; default `None`.
    pub layout_only: Option<String>,
    /// `MAKEPAD_RUN_NOHINT`; default `None`.
    pub nohint: Option<String>,
    /// `MAKEPAD_RUN_NOINL`; default `None`.
    pub noinl: Option<String>,
    /// `MAKEPAD_RUN_NOINL_CALLEE`; default `None`.
    pub noinl_callee: Option<String>,
    /// `MAKEPAD_RUN_RELAXED`; default `None`.
    pub relaxed: Option<String>,
    /// `MAKEPAD_RUN_UNROLL_ONLY`; default `None`.
    pub unroll_only: Option<String>,
    /// `MAKEPAD_RUN_AHEAD_HELPERS`; default `None` (jit::ahead_helpers: a few threads, until the
    /// first present).
    pub ahead_helpers: Option<u32>,
    /// `MAKEPAD_RUN_PIPE`; default `1`.
    pub pipe: u32,
    /// `MAKEPAD_RUN_RA_DEPTHCAP`; default `5`.
    pub ra_depthcap: u32,
    /// `MAKEPAD_RUN_PERTURB`; default `0`.
    pub perturb: u64,
    /// `MAKEPAD_RUN_SPEC_DEPTH`; default `3`.
    pub spec_depth: u8,
    /// `MAKEPAD_RUN_SPEC_HOT_FROM`; default `2`.
    pub spec_hot_from: u8,
    /// `MAKEPAD_RUN_SPEC_TTL_MS`; default `2000`.
    pub spec_ttl_ms: u64,
    /// `MAKEPAD_RUN_O3_CALLS`; default `2000`: the O3 threshold as the decayed cost of this many
    /// calls of a small fn (jit.rs Tier1Cfg).
    pub tier1_calls: u64,
    /// `MAKEPAD_RUN_CORE_HINT`; default `None`.
    pub core_hint: Option<bool>,
    /// `MAKEPAD_RUN_SPEC_HOT`; default `None`.
    pub spec_hot: Option<bool>,
    /// `MAKEPAD_RUN_SANDBOX_OPT`; default `None`.
    pub sandbox_opt: Option<bool>,
    /// `MAKEPAD_RUN_INLINE_SRC`; default `None`.
    pub inline_src: Option<u32>,
    /// `MAKEPAD_RUN_SPEC_JOBS`; default `None`.
    pub spec_jobs: Option<usize>,
    /// `MAKEPAD_RUN_INLINE_LEAF`; default `(40, 8)`.
    pub inline_leaf: (usize, usize),
    /// `MAKEPAD_RUN_OPT`; default `Vec::new()`.
    pub opt: Vec<String>,
    /// `MAKEPAD_RUN_A64`; default `Vec::new()`.
    pub a64: Vec<String>,
    /// `MAKEPAD_RUN_BORROWCK`; default `Borrowck::Full`.
    pub borrowck: Borrowck,
    /// `MAKEPAD_RUN_BORROWCK_ALL`; default `BorrowckAll::Default`.
    pub borrowck_all: BorrowckAll,
    /// `MAKEPAD_RUN_CHECK_TARGET`; default `CheckTarget::Host`.
    pub check_target: CheckTarget,
    /// `MAKEPAD_RUN_WEB_THREADS=1` selects atomics; the default is rustc's unthreaded wasm cfg.
    pub web_threads: WebThreads,
    /// `MAKEPAD_RUN_CTFE`; default `Ctfe::On`.
    pub ctfe: Ctfe,
    /// Set by `makepad check` only: loops count their iterations while a constant is evaluated,
    /// so one that never ends is rustc's `long_running_const_eval` error (the interpreter's
    /// terminator limit) instead of a hang. Changes the compiled code.
    pub const_fuel: bool,
    /// `MAKEPAD_RUN_FREQ_MODE`; default `Frequency::Blend`.
    pub freq_mode: Frequency,
    /// `MAKEPAD_RUN_RA_PRIO`; default `Priority::References`.
    pub ra_prio: Priority,
    /// `MAKEPAD_RUN_RA_ORDER`; default `false`.
    pub ra_order: bool,
    /// `MAKEPAD_RUN_RA_QUICK_COALESCE`; default `false`: the quick allocation round (no live-range
    /// splitting) also coalesces copies. Compute kernels set it.
    pub ra_quick_coalesce: bool,
    /// `MAKEPAD_RUN_NAN_EXACT`; default `false`: float identities that would return a NaN operand
    /// unquieted (`x * 1.0`, `x / 1.0`, `x - 0.0`, `x + -0.0` -> x; the hardware op quiets a
    /// signaling NaN) stay ops. Compute kernels set it: their results are bit-specified (the
    /// interpreter's), and Rust leaves the payload unspecified, so Relax's tiers fold them.
    pub nan_exact: bool,
    /// `MAKEPAD_RUN_SPEC_ORDER`; default `false`.
    pub spec_order: bool,
    /// `MAKEPAD_RUN_O3_PROFILE`; default `None`. Deterministic O3: a missing/empty file records the
    /// run's O3 decisions (each installed build's inputs and inline set), a recorded one replays them
    /// strictly (only its slots build O3 code, each once at its first call; its O2 slots get O2 builds);
    /// both runs are check-first and have no baseline tier (MAKEPAD_RUN_O1=0).
    pub tier1_profile: Option<std::ffi::OsString>,
    /// Profile replay contents frozen by the driver, separately from its environment path.
    pub tier1_profile_data: Option<String>,
    /// `--cfg <name>` / `--cfg name="value"` of `makepad run|check` (what `RUSTFLAGS=--cfg` is to
    /// cargo), as `name` / `name=value` entries added to the cfg set the program is read with.
    pub cfgs: Vec<String>,
    /// Effective target triple, finalized by the driver.
    pub target: String,
    pub cpu: CpuFeatures,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            verbose: false,
            check_windows: false,
            no_asm: false,
            allow_c: false,
            gather: false,
            memconst: false,
            no_avx: false,
            no_avx512: false,
            no_bitops: false,
            no_block_freq: false,
            no_bmi2: false,
            no_bound_infer: false,
            no_coalesce: false,
            no_full_unroll: false,
            no_iv_ranges: false,
            no_jump_tables: false,
            no_layout_fn: None,
            no_noalias: false,
            no_ranges: false,
            no_quick_ra: false,
            no_rcx: false,
            no_remat: false,
            no_simd: false,
            no_slot_share: false,
            no_split: false,
            no_store_merge: false,
            no_through: false,
            no_unroll: false,
            no_vectorize: false,
            no_windows: false,
            t1_quick_ra: false,
            t0_baseline: Tier0Baseline::Startup,
            borrowck_instances: false,
            live_stream: false,
            gandalf: true,
            derives: true,
            lazy_bodies: true,
            source_eager: false,
            rs_lazy: true,
            lazy_macro_output: true,
            native_bitflags: true,
            pre_derives: true,
            sandbox_fuel: true,
            spec: true,
            tier1: true,
            tyck_ahead: true,
            unwind: true,
            no_codegen: false,
            live: false,
            modules_trusted: false,
            sandbox_nocheck: false,
            borrowck_only: None,
            layout_only: None,
            nohint: None,
            noinl: None,
            noinl_callee: None,
            relaxed: None,
            unroll_only: None,
            ahead_helpers: None,
            pipe: 1,
            ra_depthcap: 5,
            perturb: 0,
            spec_depth: 3,
            spec_hot_from: 2,
            spec_ttl_ms: 2000,
            tier1_calls: 2000,
            core_hint: None,
            spec_hot: None,
            sandbox_opt: None,
            inline_src: None,
            spec_jobs: None,
            inline_leaf: (40, 8),
            opt: Vec::new(),
            a64: Vec::new(),
            borrowck: Borrowck::Full,
            borrowck_all: BorrowckAll::Default,
            check_target: CheckTarget::Host,
            web_threads: WebThreads::Off,
            ctfe: Ctfe::On,
            const_fuel: false,
            freq_mode: Frequency::Blend,
            ra_prio: Priority::References,
            ra_order: false,
            ra_quick_coalesce: false,
            nan_exact: false,
            spec_order: false,
            tier1_profile: None,
            tier1_profile_data: None,
            cfgs: Vec::new(),
            target: String::new(),
            cpu: CpuFeatures::default(),
        }
    }
}

impl Options {
    /// One `--cfg` value as a cfg set entry: `name` or `name="value"` (-> `name=value`).
    pub fn cfg_entry(v: &str) -> String {
        match v.split_once('=') {
            Some((k, val)) => format!("{}={}", k.trim(), val.trim().trim_matches('"')),
            None => v.trim().to_string(),
        }
    }

    pub fn from_env() -> Self {
        Self::read(&|name| std::env::var_os(name))
    }

    fn read(get: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        Self {
            verbose: get("MAKEPAD_RUN_VERBOSE").is_some_and(|v| v == "1"),
            check_windows: { let os = get("MAKEPAD_RUN_CHECK_WINDOWS"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_asm: get("MAKEPAD_RUN_NO_ASM").is_some_and(|v| v == "1"),
            allow_c: get("MAKEPAD_RUN_ALLOW_C").is_some_and(|v| v == "1"),
            gather: { let os = get("MAKEPAD_RUN_GATHER"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            memconst: { let os = get("MAKEPAD_RUN_MEMCONST"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_avx: { let os = get("MAKEPAD_RUN_NO_AVX"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_avx512: { let os = get("MAKEPAD_RUN_NO_AVX512"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_bitops: { let os = get("MAKEPAD_RUN_NO_BITOPS"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_block_freq: { let os = get("MAKEPAD_RUN_NO_BLOCK_FREQ"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_bmi2: { let os = get("MAKEPAD_RUN_NO_BMI2"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_bound_infer: { let os = get("MAKEPAD_RUN_NO_BOUND_INFER"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_coalesce: { let os = get("MAKEPAD_RUN_NO_COALESCE"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_full_unroll: { let os = get("MAKEPAD_RUN_NO_FULL_UNROLL"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_iv_ranges: { let os = get("MAKEPAD_RUN_NO_IV_RANGES"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_jump_tables: { let os = get("MAKEPAD_RUN_NO_JUMP_TABLES"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_layout_fn: { let os = get("MAKEPAD_RUN_NO_LAYOUT_FN"); let v = os.and_then(|v| v.into_string().ok()); v },
            no_noalias: { let os = get("MAKEPAD_RUN_NO_NOALIAS"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_ranges: { let os = get("MAKEPAD_RUN_NO_RANGES"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_quick_ra: { let os = get("MAKEPAD_RUN_NO_QUICK_RA"); os.is_some() },
            no_rcx: { let os = get("MAKEPAD_RUN_NO_RCX"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_remat: { let os = get("MAKEPAD_RUN_NO_REMAT"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_simd: { let os = get("MAKEPAD_RUN_NO_SIMD"); os.is_some() },
            no_slot_share: { let os = get("MAKEPAD_RUN_NO_SLOT_SHARE"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_split: { let os = get("MAKEPAD_RUN_NO_SPLIT"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_store_merge: { let os = get("MAKEPAD_RUN_NO_STORE_MERGE"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_through: { let os = get("MAKEPAD_RUN_NO_THROUGH"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_unroll: { let os = get("MAKEPAD_RUN_NO_UNROLL"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_vectorize: { let os = get("MAKEPAD_RUN_NO_VECTORIZE"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            no_windows: { let os = get("MAKEPAD_RUN_NO_WINDOWS"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            // no-codegen: no baseline startup tier -- its O2 upgrade is driven by the sampler
            // finding baseline machine code, and interpreted code has none, so it would stay
            // unoptimised for good
            t0_baseline: match get("MAKEPAD_RUN_O1").and_then(|v| v.into_string().ok()).as_deref() {
                _ if get("MAKEPAD_RUN_NO_CODEGEN").is_some_and(|v| !v.is_empty() && v != "0") => Tier0Baseline::Off,
                Some("0") => Tier0Baseline::Off,
                Some("always") => Tier0Baseline::Always,
                _ => Tier0Baseline::Startup,
            },
            t1_quick_ra: { let os = get("MAKEPAD_RUN_O3_QUICK_RA"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            borrowck_instances: { let os = get("MAKEPAD_RUN_BORROWCK_INSTANCES"); os.is_some() },
            live_stream: { let os = get("MAKEPAD_RUN_LIVE_STREAM"); os.is_some() },
            gandalf: { let os = get("MAKEPAD_RUN_GANDALF"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            derives: { let os = get("MAKEPAD_RUN_DERIVES"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            lazy_bodies: { let os = get("MAKEPAD_RUN_LAZY_BODIES"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            // one mode: lazy Rust selection (source_eager is internal only)
            source_eager: false,
            rs_lazy: true,
            lazy_macro_output: { let os = get("MAKEPAD_RUN_LAZY_MACRO_OUTPUT"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            native_bitflags: { let os = get("MAKEPAD_RUN_NATIVE_BITFLAGS"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            pre_derives: { let os = get("MAKEPAD_RUN_PRE_DERIVES"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            sandbox_fuel: { let os = get("MAKEPAD_RUN_SANDBOX_FUEL"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            spec: { let os = get("MAKEPAD_RUN_SPEC"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            tier1: { let os = get("MAKEPAD_RUN_O3"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") && !get("MAKEPAD_RUN_NO_CODEGEN").is_some_and(|v| !v.is_empty() && v != "0") },
            tyck_ahead: { let os = get("MAKEPAD_RUN_TYCK_AHEAD"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            unwind: { let os = get("MAKEPAD_RUN_UNWIND"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            no_codegen: { let os = get("MAKEPAD_RUN_NO_CODEGEN"); let v = os.and_then(|v| v.into_string().ok()); v.is_some_and(|v| !v.is_empty() && v != "0") },
            live: false,
            modules_trusted: { let os = get("MAKEPAD_RUN_MODULES_TRUSTED"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() == Some("1") },
            sandbox_nocheck: { let os = get("MAKEPAD_RUN_SANDBOX_NOCHECK"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() == Some("1") },
            borrowck_only: { let os = get("MAKEPAD_RUN_BORROWCK_ONLY"); let v = os.and_then(|v| v.into_string().ok()); v },
            layout_only: { let os = get("MAKEPAD_RUN_LAYOUT_ONLY"); let v = os.and_then(|v| v.into_string().ok()); v },
            nohint: { let os = get("MAKEPAD_RUN_NOHINT"); let v = os.and_then(|v| v.into_string().ok()); v },
            noinl: { let os = get("MAKEPAD_RUN_NOINL"); let v = os.and_then(|v| v.into_string().ok()); v },
            noinl_callee: { let os = get("MAKEPAD_RUN_NOINL_CALLEE"); let v = os.and_then(|v| v.into_string().ok()); v },
            relaxed: { let os = get("MAKEPAD_RUN_RELAXED"); let v = os.and_then(|v| v.into_string().ok()); v },
            unroll_only: { let os = get("MAKEPAD_RUN_UNROLL_ONLY"); let v = os.and_then(|v| v.into_string().ok()); v },
            ahead_helpers: { let os = get("MAKEPAD_RUN_AHEAD_HELPERS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()) },
            pipe: { let os = get("MAKEPAD_RUN_PIPE"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(1) },
            ra_depthcap: { let os = get("MAKEPAD_RUN_RA_DEPTHCAP"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(5) },
            perturb: { let os = get("MAKEPAD_RUN_PERTURB"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(0) },
            spec_depth: { let os = get("MAKEPAD_RUN_SPEC_DEPTH"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(3) },
            spec_hot_from: { let os = get("MAKEPAD_RUN_SPEC_HOT_FROM"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(2) },
            spec_ttl_ms: { let os = get("MAKEPAD_RUN_SPEC_TTL_MS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(2000) },
            tier1_calls: { let os = get("MAKEPAD_RUN_O3_CALLS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(2000) },
            core_hint: { let os = get("MAKEPAD_RUN_CORE_HINT"); let v = os.and_then(|v| v.into_string().ok()); v.map(|v| v == "1") },
            spec_hot: { let os = get("MAKEPAD_RUN_SPEC_HOT"); let v = os.and_then(|v| v.into_string().ok()); v.map(|v| v == "1") },
            sandbox_opt: { let os = get("MAKEPAD_RUN_SANDBOX_OPT"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("0") => Some(false), Some("1") => Some(true), _ => None } },
            inline_src: { let os = get("MAKEPAD_RUN_INLINE_SRC"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()) },
            spec_jobs: { let os = get("MAKEPAD_RUN_SPEC_JOBS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()) },
            inline_leaf: { let os = get("MAKEPAD_RUN_INLINE_LEAF"); let v = os.and_then(|v| v.into_string().ok()); parse_leaf(v.as_deref()) },
            opt: { let os = get("MAKEPAD_RUN_OPT"); let v = os.and_then(|v| v.into_string().ok()); split(v) },
            a64: { let os = get("MAKEPAD_RUN_A64"); let v = os.and_then(|v| v.into_string().ok()); split(v) },
            borrowck: { let os = get("MAKEPAD_RUN_BORROWCK"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("off") => Borrowck::Off, Some("moves") => Borrowck::Moves, _ => Borrowck::Full } },
            borrowck_all: { let os = get("MAKEPAD_RUN_BORROWCK_ALL"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("nongeneric") => BorrowckAll::Nongeneric, None => BorrowckAll::Default, Some("std") => BorrowckAll::Std, _ => BorrowckAll::All } },
            check_target: { let os = get("MAKEPAD_RUN_CHECK_TARGET"); let v = os.and_then(|v| v.into_string().ok()); CheckTarget::from_triple(v.as_deref().unwrap_or("")).unwrap_or(CheckTarget::Host) },
            web_threads: if get("MAKEPAD_RUN_WEB_THREADS").as_deref() == Some(std::ffi::OsStr::new("1")) { WebThreads::On } else { WebThreads::Off },
            const_fuel: false,
            ctfe: { let os = get("MAKEPAD_RUN_CTFE"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("0") => Ctfe::Off, Some("check") => Ctfe::Check, _ => Ctfe::On } },
            freq_mode: { let os = get("MAKEPAD_RUN_FREQ_MODE"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("sampled") => Frequency::Sampled, Some("static") => Frequency::Static, _ => Frequency::Blend } },
            ra_prio: { let os = get("MAKEPAD_RUN_RA_PRIO"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("len") => Priority::Length, Some("sqrt") => Priority::Sqrt, _ => Priority::References } },
            ra_order: { let os = get("MAKEPAD_RUN_RA_ORDER"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() == Some("start") },
            ra_quick_coalesce: { let os = get("MAKEPAD_RUN_RA_QUICK_COALESCE"); os.is_some() },
            nan_exact: { let os = get("MAKEPAD_RUN_NAN_EXACT"); os.is_some() },
            spec_order: { let os = get("MAKEPAD_RUN_SPEC_ORDER"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() == Some("lifo") },
            tier1_profile: { let os = get("MAKEPAD_RUN_O3_PROFILE"); os },
            tier1_profile_data: None,
            cfgs: Vec::new(),
            target: String::new(),
            cpu: CpuFeatures::default(),
        }
    }

    /// Exhaustive destructuring makes adding an unhashed field a compile error.
    pub fn fingerprint(&self) -> u128 {
        let Self {
            // Logging does not change compiled code or invalidate warm caches.
            verbose: _,
            // which dependencies cargo builds does not change the code Relax compiles from them
            allow_c: _,
            check_windows,
            no_asm,
            gather,
            memconst,
            no_avx,
            no_avx512,
            no_bitops,
            no_block_freq,
            no_bmi2,
            no_bound_infer,
            no_coalesce,
            no_full_unroll,
            no_iv_ranges,
            no_jump_tables,
            no_layout_fn,
            no_noalias,
            no_ranges,
            no_quick_ra,
            no_rcx,
            no_remat,
            no_simd,
            no_slot_share,
            no_split,
            no_store_merge,
            no_through,
            no_unroll,
            no_vectorize,
            no_windows,
            t1_quick_ra,
            t0_baseline,
            borrowck_instances,
            live_stream,
            gandalf,
            derives,
            lazy_bodies,
            source_eager,
            rs_lazy,
            lazy_macro_output,
            native_bitflags,
            pre_derives,
            sandbox_fuel,
            spec,
            tier1,
            tyck_ahead,
            unwind,
            no_codegen,
            live,
            modules_trusted,
            sandbox_nocheck,
            borrowck_only,
            layout_only,
            nohint,
            noinl,
            noinl_callee,
            relaxed,
            unroll_only,
            ahead_helpers,
            pipe,
            ra_depthcap,
            perturb,
            spec_depth,
            spec_hot_from,
            spec_ttl_ms,
            tier1_calls,
            core_hint,
            // run-time scheduling knobs, filled in from the machine's state at session start
            // (lib.rs session_from_env: on battery, online cores): they decide when bodies are
            // compiled, not what code they compile to -- hashing them made a cache written on
            // mains, or with other cores online, a stranger to the next launch (every record
            // rejected: "identity: parser options")
            spec_hot: _,
            sandbox_opt,
            inline_src,
            spec_jobs: _,
            inline_leaf,
            opt,
            a64,
            borrowck,
            borrowck_all,
            check_target,
            web_threads,
            ctfe,
            const_fuel,
            freq_mode,
            ra_prio,
            ra_order,
            ra_quick_coalesce,
            nan_exact,
            spec_order,
            tier1_profile,
            tier1_profile_data, cfgs, target, cpu,
        } = self;
        let mut h = Sip128::new();
        h.str("relax-options-v1");
        h.str("check_windows");
        h.u8(*check_windows as u8);
        h.str("no_asm");
        h.u8(*no_asm as u8);
        h.str("gather");
        h.u8(*gather as u8);
        h.str("memconst");
        h.u8(*memconst as u8);
        h.str("no_avx");
        h.u8(*no_avx as u8);
        h.str("no_avx512");
        h.u8(*no_avx512 as u8);
        h.str("no_bitops");
        h.u8(*no_bitops as u8);
        h.str("no_block_freq");
        h.u8(*no_block_freq as u8);
        h.str("no_bmi2");
        h.u8(*no_bmi2 as u8);
        h.str("no_bound_infer");
        h.u8(*no_bound_infer as u8);
        h.str("no_coalesce");
        h.u8(*no_coalesce as u8);
        h.str("no_full_unroll");
        h.u8(*no_full_unroll as u8);
        h.str("no_iv_ranges");
        h.u8(*no_iv_ranges as u8);
        h.str("no_jump_tables");
        h.u8(*no_jump_tables as u8);
        h.str("no_layout_fn");
        match no_layout_fn { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("no_noalias");
        h.u8(*no_noalias as u8);
        h.str("no_ranges");
        h.u8(*no_ranges as u8);
        h.str("no_quick_ra");
        h.u8(*no_quick_ra as u8);
        h.str("no_rcx");
        h.u8(*no_rcx as u8);
        h.str("no_remat");
        h.u8(*no_remat as u8);
        h.str("no_simd");
        h.u8(*no_simd as u8);
        h.str("no_slot_share");
        h.u8(*no_slot_share as u8);
        h.str("no_split");
        h.u8(*no_split as u8);
        h.str("no_store_merge");
        h.u8(*no_store_merge as u8);
        h.str("no_through");
        h.u8(*no_through as u8);
        h.str("no_unroll");
        h.u8(*no_unroll as u8);
        h.str("no_vectorize");
        h.u8(*no_vectorize as u8);
        h.str("no_windows");
        h.u8(*no_windows as u8);
        h.str("o1");
        h.u8(*t0_baseline as u8);
        h.str("o3_quick_ra");
        h.u8(*t1_quick_ra as u8);
        h.str("borrowck_instances");
        h.u8(*borrowck_instances as u8);
        h.str("live_stream");
        h.u8(*live_stream as u8);
        h.str("gandalf");
        h.u8(*gandalf as u8);
        h.str("derives");
        h.u8(*derives as u8);
        h.str("lazy_bodies");
        h.u8(*lazy_bodies as u8);
        h.str("source_eager");
        h.u8(*source_eager as u8);
        h.str("rs_lazy");
        h.u8(*rs_lazy as u8);
        h.str("lazy_macro_output");
        h.u8(*lazy_macro_output as u8);
        h.str("native_bitflags");
        h.u8(*native_bitflags as u8);
        h.str("pre_derives");
        h.u8(*pre_derives as u8);
        h.str("sandbox_fuel");
        h.u8(*sandbox_fuel as u8);
        h.str("spec");
        h.u8(*spec as u8);
        h.str("o3");
        h.u8(*tier1 as u8);
        h.str("tyck_ahead");
        h.u8(*tyck_ahead as u8);
        h.str("unwind");
        h.u8(*unwind as u8);
        h.str("no_codegen");
        h.u8(*no_codegen as u8);
        h.str("live");
        h.u8(*live as u8);
        h.str("modules_trusted");
        h.u8(*modules_trusted as u8);
        h.str("sandbox_nocheck");
        h.u8(*sandbox_nocheck as u8);
        h.str("borrowck_only");
        match borrowck_only { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("layout_only");
        match layout_only { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("nohint");
        match nohint { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("noinl");
        match noinl { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("noinl_callee");
        match noinl_callee { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("relaxed");
        match relaxed { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("unroll_only");
        match unroll_only { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str("ahead_helpers");
        match ahead_helpers { Some(v) => { h.u8(1); h.u64(*v as u64); }, None => h.u8(0) }
        h.str("pipe");
        h.u64(*pipe as u64);
        h.str("ra_depthcap");
        h.u64(*ra_depthcap as u64);
        h.str("perturb");
        h.u64(*perturb as u64);
        h.str("spec_depth");
        h.u64(*spec_depth as u64);
        h.str("spec_hot_from");
        h.u64(*spec_hot_from as u64);
        h.str("spec_ttl_ms");
        h.u64(*spec_ttl_ms as u64);
        h.str("o3_calls");
        h.u64(*tier1_calls as u64);
        h.str("core_hint");
        match core_hint { Some(v) => { h.u8(1); h.u64(*v as u64); }, None => h.u8(0) }
        h.str("sandbox_opt");
        match sandbox_opt { Some(v) => { h.u8(1); h.u64(*v as u64); }, None => h.u8(0) }
        h.str("inline_src");
        match inline_src { Some(v) => { h.u8(1); h.u64(*v as u64); }, None => h.u8(0) }
        h.str("inline_leaf");
        h.u64(inline_leaf.0 as u64); h.u64(inline_leaf.1 as u64);
        h.str("opt");
        h.u64(opt.len() as u64); for v in opt { h.str(v); }
        h.str("cfgs");
        h.u64(cfgs.len() as u64); for v in cfgs { h.str(v); }
        h.str("a64");
        h.u64(a64.len() as u64); for v in a64 { h.str(v); }
        h.str("borrowck");
        h.u8(*borrowck as u8);
        h.str("borrowck_all");
        h.u8(*borrowck_all as u8);
        h.str("web_threads");
        h.u8(if *web_threads == WebThreads::On { 1 } else { 0 });
        h.str("check_target");
        h.u8(match *check_target {
            CheckTarget::Host => 0,
            CheckTarget::MacosArm64 => 1,
            CheckTarget::LinuxX64 => 2,
            CheckTarget::WindowsX64 => 3,
            CheckTarget::AndroidArm64 => 4,
            CheckTarget::Web(PtrWidth::W32) => 5,
            CheckTarget::Web(PtrWidth::W64) => 6,
        });
        h.str("ctfe");
        h.u8(*ctfe as u8);
        h.str("const_fuel");
        h.u8(*const_fuel as u8);
        h.str("freq_mode");
        h.u8(*freq_mode as u8);
        h.str("ra_prio");
        h.u8(*ra_prio as u8);
        h.str("ra_order");
        h.u8(*ra_order as u8);
        h.str("ra_quick_coalesce");
        h.u8(*ra_quick_coalesce as u8);
        // (only when set: the default key stays the key it was before the option existed)
        if *nan_exact {
            h.str("nan_exact");
            h.u8(1);
        }
        h.str("spec_order");
        h.u8(*spec_order as u8);
        h.str("o3_profile");
        match tier1_profile { Some(v) => { h.u8(1); h.u64(v.as_encoded_bytes().len() as u64); h.write(v.as_encoded_bytes()); }, None => h.u8(0) }
        match tier1_profile_data { Some(v) => { h.u8(1); h.str(v); }, None => h.u8(0) }
        h.str(target);
        let CpuFeatures { avx, bmi2, popcnt, lzcnt, bmi1, fma, avx2, avx512 } = cpu;
        for b in [avx, bmi2, popcnt, lzcnt, bmi1, fma, avx2, avx512] { h.u8(*b as u8); }
        h.finish128()
    }
}

/// The cold Rust load+parse sweep favors eight workers on a 16-CPU Mac.
/// Keep the front-end pool and the core's module index on the same default;
/// explicit MAKEPAD_RUN_JOBS values continue to override both.
pub fn default_parse_jobs() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).min(8)
}

#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    /// Legacy switch retained to diagnose MAKEPAD_RUN_NATIVE_MACROS=0 as a policy error.
    pub native_macros: bool,
    /// Legacy differential switch; MAKEPAD_RUN_NATIVE_VERIFY=1 conflicts with native-only macros.
    pub native_verify: bool,
    /// Reparse accepted worker files in collection scope and compare their complete ASTs.
    pub parse_verify: bool,
    /// Frozen execution options for the source lowering core.
    pub profile: bool,
    pub parse_jobs: usize,
    /// Check indexed declarations and complete lazy/eager program equivalence.
    pub lazy_verify: bool,
    /// `MAKEPAD_RUN_BOOT_ONLY`; default `false`.
    pub boot_only: bool,
    /// `MAKEPAD_RUN_MEM_AT_TRIM`; default `false`.
    pub mem_at_trim: bool,
    /// `MAKEPAD_RUN_MEM_TRIM`; default `true`.
    pub mem_trim: bool,
    /// `MAKEPAD_RUN_SETTLE_DROP`; default `true`.
    pub settle_drop: bool,
    /// `MAKEPAD_RUN_BURST_ARENA`; default `true`.
    pub burst_arena: bool,
    /// `MAKEPAD_RUN_O3_BURST` (`0`: off, for A/Bs); default `true`: after the first present, while the O3
    /// queue is not trivial, more O3 workers run (every performance core but one) until it drains.
    pub tier1_burst: bool,
    /// `MAKEPAD_RUN_MODULE_FUEL`; default `4_000_000_000`.
    pub module_fuel: i64,
    /// `MAKEPAD_RUN_MODULE_MEM_MB`; default `256`.
    pub module_mem_mb: u64,
    /// `MAKEPAD_RUN_REPEAT`; default `1`.
    pub repeat: usize,
    /// `MAKEPAD_RUN_TEST_DOMAINS`: `makepad test` runs each test at the same time in this many domains
    /// (each its own thread and static block); default `1`.
    pub test_domains: usize,
    /// `MAKEPAD_RUN_WATCHDOG_MS`; default `20_000`.
    pub watchdog_ms: u64,
    /// `MAKEPAD_RUN_O3_THREADS`; default `None`.
    pub tier1_threads: Option<usize>,
    /// `MAKEPAD_RUN_RESTORE`; default `None`.
    pub restore: Option<String>,
    /// `MAKEPAD_RUN_WORKER_QOS`; default `WorkerQos::Background`.
    pub worker_qos: WorkerQos,
    /// `MAKEPAD_RUN_AHEAD` (`--ahead=<level>` sets it); default `AheadLevel::Balanced`. Not part of
    /// the code identity: every level makes the same code for a slot, so they share warm caches.
    pub ahead: AheadLevel,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            native_macros: true,
            native_verify: false,
            parse_verify: false,
            profile: false,
            parse_jobs: default_parse_jobs(),
            lazy_verify: false,
            boot_only: false,
            mem_at_trim: false,
            mem_trim: true,
            settle_drop: true,
            burst_arena: true,
            tier1_burst: true,
            module_fuel: 4_000_000_000,
            module_mem_mb: 256,
            repeat: 1,
            test_domains: 1,
            watchdog_ms: 20_000,
            tier1_threads: None,
            restore: None,
            worker_qos: WorkerQos::Background,
            ahead: AheadLevel::Balanced,
        }
    }
}

impl RuntimeOptions {
    pub fn validate_native_macros(&self) -> Result<(), String> {
        if !self.native_macros {
            return Err("MAKEPAD_RUN_NATIVE_MACROS=0 conflicts with native-only Makepad proc macros; unset it and rebuild makepad to use changed macro sources".into());
        }
        if self.native_verify {
            return Err("MAKEPAD_RUN_NATIVE_VERIFY=1 conflicts with native-only Makepad proc macros; unset it (hosted differential expansion is not supported)".into());
        }
        Ok(())
    }

    pub fn from_env() -> Self {
        Self::read(&|name| std::env::var_os(name))
    }

    fn read(get: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        Self {
            native_macros: get("MAKEPAD_RUN_NATIVE_MACROS").as_deref() != Some(std::ffi::OsStr::new("0")),
            native_verify: get("MAKEPAD_RUN_NATIVE_VERIFY").as_deref() == Some(std::ffi::OsStr::new("1")),
            parse_verify: get("MAKEPAD_RUN_PARSE_VERIFY").as_deref() == Some(std::ffi::OsStr::new("1")),
            profile: get("MAKEPAD_RUN_PROFILE").is_some(),
            parse_jobs: get("MAKEPAD_RUN_JOBS").and_then(|v| v.into_string().ok()).and_then(|v| v.parse::<usize>().ok()).unwrap_or_else(default_parse_jobs).clamp(1, 32),
            lazy_verify: get("MAKEPAD_RUN_LAZY_VERIFY").as_deref() == Some(std::ffi::OsStr::new("1")),
            boot_only: { let os = get("MAKEPAD_RUN_BOOT_ONLY"); let v = os.and_then(|v| v.into_string().ok()); v.is_some() },
            mem_at_trim: { let os = get("MAKEPAD_RUN_MEM_AT_TRIM"); os.is_some() },
            mem_trim: { let os = get("MAKEPAD_RUN_MEM_TRIM"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            settle_drop: { let os = get("MAKEPAD_RUN_SETTLE_DROP"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            burst_arena: { let os = get("MAKEPAD_RUN_BURST_ARENA"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            tier1_burst: { let os = get("MAKEPAD_RUN_O3_BURST"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref() != Some("0") },
            module_fuel: { let os = get("MAKEPAD_RUN_MODULE_FUEL"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(4_000_000_000) },
            module_mem_mb: { let os = get("MAKEPAD_RUN_MODULE_MEM_MB"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(256) },
            repeat: { let os = get("MAKEPAD_RUN_REPEAT"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(1) },
            test_domains: { let os = get("MAKEPAD_RUN_TEST_DOMAINS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).filter(|n: &usize| *n >= 1).unwrap_or(1) },
            watchdog_ms: { let os = get("MAKEPAD_RUN_WATCHDOG_MS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()).unwrap_or(20_000) },
            tier1_threads: { let os = get("MAKEPAD_RUN_O3_THREADS"); let v = os.and_then(|v| v.into_string().ok()); v.and_then(|v| v.parse().ok()) },
            restore: { let os = get("MAKEPAD_RUN_RESTORE"); let v = os.and_then(|v| v.into_string().ok()); v },
            worker_qos: { let os = get("MAKEPAD_RUN_WORKER_QOS"); let v = os.and_then(|v| v.into_string().ok()); match v.as_deref() { Some("0") => WorkerQos::Default, Some("utility") => WorkerQos::Utility, _ => WorkerQos::Background } },
            ahead: { let os = get("MAKEPAD_RUN_AHEAD"); let v = os.and_then(|v| v.into_string().ok()); v.as_deref().and_then(AheadLevel::parse).unwrap_or(AheadLevel::Balanced) },
        }
    }
}

fn split(v: Option<String>) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(v) = v {
        for p in v.split(',') { parts.push(p.to_string()); }
    }
    parts
}

fn parse_leaf(v: Option<&str>) -> (usize, usize) {
    let mut parts = v.unwrap_or("").split(',');
    let insts = parts.next().and_then(|v| v.parse().ok()).unwrap_or(40);
    let blocks = parts.next().and_then(|v| v.parse().ok()).unwrap_or(8);
    (insts, blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn q2_options_preserve_parsing_and_presence() {
        let empty = Options::read(&|_| None);
        assert_eq!(empty.fingerprint(), Options::default().fingerprint());
        let opts = Options::read(&|name| match name {
            "MAKEPAD_RUN_INLINE_LEAF" => Some("bad,17".into()),
            "MAKEPAD_RUN_INLINE_SRC" => Some("bad".into()),
            "MAKEPAD_RUN_OPT" => Some(" -thread,-ifconv".into()),
            "MAKEPAD_RUN_BORROWCK_ALL" => Some("0".into()),
            "MAKEPAD_RUN_CORE_HINT" => Some("0".into()),
            "MAKEPAD_RUN_NO_BOUND_INFER" => Some("0".into()),
            _ => None,
        });
        assert_eq!(opts.inline_leaf, (40, 17));
        assert_eq!(opts.inline_src, None);
        assert_eq!(opts.core_hint, Some(false));
        assert!(opts.no_bound_infer);
        assert_eq!(opts.borrowck_all, BorrowckAll::All);
        assert_eq!(opts.opt, [" -thread", "-ifconv"]);
        assert_ne!(empty.fingerprint(), opts.fingerprint());
    }

    #[test]
    fn q2_options_hash_effective_cpu_target_and_replay_contents() {
        let base = Options::default();
        let fingerprint = base.fingerprint();
        let mut changed = base.clone();
        changed.no_bound_infer = true;
        assert_ne!(fingerprint, changed.fingerprint());
        changed = base.clone();
        changed.live = true;
        assert_ne!(fingerprint, changed.fingerprint());
        changed = base.clone();
        changed.cpu.avx = true;
        assert_ne!(fingerprint, changed.fingerprint());
        changed = base.clone();
        changed.target = "x86_64-pc-windows-gnu".to_string();
        assert_ne!(fingerprint, changed.fingerprint());
        changed = base.clone();
        changed.tier1_profile_data = Some("record".to_string());
        assert_ne!(fingerprint, changed.fingerprint());
        changed.tier1_profile_data = Some("other record".to_string());
        assert_ne!(fingerprint, changed.fingerprint());
        assert_eq!(base.fingerprint(), base.clone().fingerprint());
    }

    #[cfg(unix)]
    #[test]
    fn q2_options_non_utf8_matches_var_and_var_os() {
        use std::os::unix::ffi::OsStringExt;
        let opts = Options::read(&|_| Some(OsString::from_vec(vec![0xff])));
        assert!(!opts.no_avx);
        assert!(opts.no_simd);
        assert!(opts.live_stream);
        assert!(opts.tier1_profile.is_some());
        assert!(opts.derives);
        assert_eq!(opts.core_hint, None);
    }

    #[test]
    fn q2_runtime_policy_is_separate() {
        let runtime = RuntimeOptions::read(&|name| match name {
            "MAKEPAD_RUN_REPEAT" => Some("3".into()),
            "MAKEPAD_RUN_WATCHDOG_MS" => Some("123".into()),
            "MAKEPAD_RUN_BURST_ARENA" => Some("0".into()),
            _ => None,
        });
        assert_eq!(runtime.repeat, 3);
        assert_eq!(runtime.watchdog_ms, 123);
        assert!(!runtime.burst_arena);
        let options = Options::read(&|name| match name {
            "MAKEPAD_RUN_REPEAT" | "MAKEPAD_RUN_WATCHDOG_MS" | "MAKEPAD_RUN_BURST_ARENA" => Some("123".into()),
            _ => None,
        });
        assert_eq!(Options::default().fingerprint(), options.fingerprint());
    }

    #[test]
    fn selection_is_lazy_and_runtime_verify_reads() {
        let options = Options::read(&|_| None);
        assert!(options.rs_lazy && !options.source_eager);
        assert!(RuntimeOptions::read(&|name| (name == "MAKEPAD_RUN_LAZY_VERIFY").then(|| "1".into())).lazy_verify);
    }

    #[test]
    fn parse_pool_and_core_defaults_agree_and_explicit_jobs_still_override() {
        let read = |jobs: Option<&str>| RuntimeOptions::read(&|name| {
            (name == "MAKEPAD_RUN_JOBS").then(|| jobs.map(OsString::from)).flatten()
        }).parse_jobs;
        assert_eq!(read(None), default_parse_jobs());
        assert_eq!(read(None), RuntimeOptions::default().parse_jobs);
        assert!((1..=8).contains(&read(None)));
        assert_eq!(read(Some("16")), 16);
        assert_eq!(read(Some("0")), 1);
        assert_eq!(read(Some("99")), 32);
        assert_eq!(read(Some("invalid")), read(None));
    }
}
