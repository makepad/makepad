//! Splash audio shaders: instruments and effects written as small Splash
//! functions, compiled ahead of time (at edit time, on a worker thread)
//! into realtime-safe code for the audio thread by the Splash compute core
//! (makepad-script-compute).
//!
//! ```text
//! shader source --[parse]--> AST --[lower: types, inlining, math]--> AIR
//!     AIR --> ir::run (the reference interpreter, and the no-JIT path)
//!     AIR --> arm64 (native code: MAP_JIT, W^X)
//!     AIR --> wasm (generated WebAssembly, linked by a browser host: [`wasm`])
//! ```
//!
//! Every backend is bit-identical to [`ir::run`]: AIR's ops are total and
//! exactly specified, and math functions are polynomials made of those ops
//! (no libm on the audio path), so a score renders the same samples on
//! every machine, offline or live.

// thread-locals take no pthread key each on Android (libs/thread_local)
#[macro_use(thread_local)]
extern crate makepad_thread_local;

use relax_api::relax_api;

// Options JIT code reaches in this crate's unmarked items; the binding refuses a tagged Option with no entry.
relax_api::option_layouts! { f32, usize, (f32, f32, f32) }

pub mod align;
pub use align::{align, AlignEdge, AlignError, AlignNode, Alignment, EdgeTiming, NodeTiming};
pub mod boundary;
pub use boundary::{AudioBindings, AudioShaderRef, ParamMetaView, SampleView};
pub mod fuse;
mod guide;
mod selftest;
pub use selftest::{selftest, SelftestBackend, SelftestReport};
pub mod wasm;

// The language, IR, interpreter and native backends are the Splash compute
// core's; audio is one of its front ends.
/// The host's native backend (the shared back end, both targets).
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
use makepad_script_compute::native;
pub use makepad_script_compute::{ir, lower, parse, Backend, ShaderError};
pub use makepad_script_compute::module::Module;

pub use guide::GUIDE;
pub use fuse::{fuse, fuse_with, FuseError, FuseFailure, FuseNode, Port};
pub use lower::{header, AudioPortInfo, Kind, ParamCurve, ParamInfo, ParamMeta, ParamUnit, StateVar, CC_WORDS, CONTROL_BLOCK,
    CTX_BAR_BEATS, CTX_BEAT_AT, CTX_BEAT_FRAME, CTX_BEAT_STEP, CTX_CC, CTX_FRAME, CTX_INSTANCE, CTX_PARAMS, CTX_PLAYING, CTX_RATE, CTX_TEMPO, MAX_FRAMES};

use boundary::forever;
use makepad_script_compute::boundary::ParamInfoView;
use std::sync::{Arc, Weak};

/// Combines two seed words. The constants and order define the render stream.
/// Start with `a ^ (b * 0x9e3779b9)`. XOR with the word shifted right by 16,
/// multiply by 0x85ebca6b, XOR with its shift by 13, multiply by 0xc2b2ae35,
/// then XOR with its shift by 16. All products wrap to 32 bits.
pub fn mix_seed(a: u32, b: u32) -> u32 {
    let x = a ^ b.wrapping_mul(0x9e37_79b9);
    let x = (x ^ (x >> 16)).wrapping_mul(0x85eb_ca6b);
    let x = (x ^ (x >> 13)).wrapping_mul(0xc2b2_ae35);
    x ^ (x >> 16)
}

/// A voice's continuous expression inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expression {
    Bend,
    Pressure,
    Slide,
}

fn unit_value(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, 1.0) } else { fallback }
}

/// A declared audio port. Channel indices are relative to its direction's table.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PortInfoView<'a> {
    pub name: &'a str,
    pub output: bool,
    pub channels: u16,
    pub first: u16,
    pub control: bool,
}

/// A compiled audio shader's plain face: what callers read, laid out so
/// that Relax-compiled code reads it itself instead of calling into the
/// compiler's code. It is the first field of its [`AudioShaderBody`] (the
/// programs and machine code) and derefs to it. Immutable and shareable;
/// all mutable data (ctx, state, scratch) belongs to the caller, allocated
/// off the audio thread.
#[relax_api(plain)]
#[repr(C)]
pub struct AudioShader {
    pub kind: Kind,
    mono: u32,
    latency: u32,
    params: &'static [ParamInfoView<'static>],
    warnings: &'static [boundary::WarningView<'static>],
    ports: &'static [PortInfoView<'static>],
    input_channels: u16,
    output_channels: u16,
    probes: &'static [&'static str],
    samples: &'static [&'static str],
    param_metas: &'static [ParamMetaView<'static>],
    state_vars: &'static [boundary::StateVarView<'static>],
    state_init: &'static [u32],
    /// Address of the body's shared layout, or zero for incomplete coverage.
    state_layout: usize,
    /// A counting build ([`compile_counting`]): the ctx word each run
    /// stores the ops it counted in ([`NO_COUNT_WORD`]: not a counting build).
    count_word: u32,
    /// Words for the interpreter or the compiled frame and aligned channel pointers.
    scratch_words: usize,
    /// The body this face is the first field of.
    body: *const AudioShaderBody,
}

// SAFETY: the face is immutable, and `body` points to the allocation it
// lives in, which is shared across threads the same way.
unsafe impl Send for AudioShader {}
unsafe impl Sync for AudioShader {}

const NO_COUNT_WORD: u32 = u32::MAX;

/// A compiled audio shader's programs and machine code: the compiler's own
/// state, reached through [`AudioShader`]'s deref.
#[relax_api(handle)]
#[repr(C)]
pub struct AudioShaderBody {
    /// First: see [`AudioShader`].
    pub(crate) head: AudioShader,
    params: Vec<ParamInfo>,
    port_info: Vec<AudioPortInfo>,
    _port_views: Box<[PortInfoView<'static>]>,
    _warnings: Vec<lower::ShaderWarning>,
    _warning_views: Box<[boundary::WarningView<'static>]>,
    module_paths: Vec<String>,
    _sample_names: Vec<String>,
    _sample_views: Box<[&'static str]>,
    _probe_names: Vec<String>,
    _probe_views: Box<[&'static str]>,
    param_meta: Vec<ParamMeta>,
    param_views: Box<[ParamInfoView<'static>]>,
    param_meta_views: Box<[ParamMetaView<'static>]>,
    choice_lists: Vec<Box<[&'static str]>>,
    state_init: Box<[u32]>,
    _state_layout: Option<Arc<StateLayout>>,
    state_vars: Vec<StateVar>,
    state_var_views: Box<[boundary::StateVarView<'static>]>,
    shared: Box<[u32]>,
    render: ir::Program,
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    native: Option<native::Code>,
    /// [`AudioShaderBody::program_key`], computed on first use.
    key: std::sync::OnceLock<u64>,
}

/// The interned state layouts ([`StateLayout::intern`]). Its lock keeps its state inline
/// (`relax_api::sync`), and a run's end in place empties it ([`end_run`]).
static LAYOUTS: relax_api::sync::Mutex<Vec<Weak<StateLayout>>> = relax_api::sync::Mutex::new(Vec::new());

/// A run of this process's app ended in place (`makepad run`). The interned layouts belong to that
/// run's programs and go with it, table and all, so the next run starts as a fresh process does.
#[relax_api]
pub fn end_run() {
    wasm::end_run();
    drop(std::mem::take(&mut *LAYOUTS.lock().unwrap_or_else(|e| e.into_inner())));
}

/// Compilers share an identity only after comparing the complete layout.
/// Weak references let retired layouts go with their last program.
#[derive(PartialEq, Eq)]
struct StateLayout {
    kind: Kind,
    params: Vec<String>,
    vars: Vec<(String, String, u32, u32)>,
}

impl StateLayout {
    fn intern(kind: Kind, params: &[ParamInfo], vars: &[StateVar], words: usize) -> Option<Arc<Self>> {
        let mut end = lower::HEADER_WORDS as usize + 2 * params.len();
        for var in vars {
            if var.offset as usize != end { return None; }
            end += var.words as usize;
        }
        if end != words { return None; }
        let layout = Self {
            kind,
            params: params.iter().map(|p| p.name.clone()).collect(),
            vars: vars.iter().map(|v| (v.name.clone(), v.sig.clone(), v.offset, v.words)).collect(),
        };
        let mut layouts = LAYOUTS.lock().unwrap_or_else(|e| e.into_inner());
        layouts.retain(|entry| entry.strong_count() != 0);
        if let Some(found) = layouts.iter().filter_map(Weak::upgrade).find(|other| **other == layout) {
            return Some(found);
        }
        let layout = Arc::new(layout);
        layouts.push(Arc::downgrade(&layout));
        Some(layout)
    }
}

impl std::ops::Deref for AudioShader {
    type Target = AudioShaderBody;
    #[inline]
    fn deref(&self) -> &AudioShaderBody {
        // SAFETY: an AudioShader only exists as the first field of an
        // AudioShaderBody, which `body` points to.
        unsafe { &*self.body }
    }
}

/// Compiles with the fastest backend available on this host.
#[inline(always)]
pub fn compile(code: &str) -> Result<AudioShaderRef, Vec<ShaderError>> {
    compile_with(code, Backend::Native)
}

/// Compiles for a given backend (`Native` falls back to the interpreter
/// when the host has no native backend).
#[inline(always)]
pub fn compile_with(code: &str, backend: Backend) -> Result<AudioShaderRef, Vec<ShaderError>> {
    boundary::compile_audio(code, backend, false).into_result()
}

/// Compiles natively with the render program counting the ops it runs
/// (`makepad_script_compute::work`): the same samples, and
/// [`Instance::ops`] says how much work they took, the same on every
/// machine. For tools ([`check`]); a host plays [`compile`]'s build.
#[inline(always)]
pub fn compile_counting(code: &str) -> Result<AudioShaderRef, Vec<ShaderError>> {
    boundary::compile_audio(code, Backend::Native, true).into_result()
}

/// Query a shader's own entry declarations using the compiler's kind pass.
/// The library editor needs this metadata without generating render code.
#[relax_api]
pub fn declares_audio_kind(code: &str, kind: Kind) -> bool {
    parse::lex(code).and_then(|tokens| parse::Parser::new(&tokens).items())
        .and_then(|items| lower::audio_entry_kind(&items)).is_ok_and(|entry| entry == kind)
}

/// The prelude: DSP building blocks every shader sees (see prelude.splash).
pub const PRELUDE: &str = include_str!("prelude.splash");

struct AudioImports {
    user_len: usize,
    node_ranges: Vec<(usize, usize)>,
    prelude: Vec<parse::Item>,
    paths: Vec<String>,
}

impl AudioImports {
    fn new(user_len: usize) -> Self {
        Self { user_len, node_ranges: Vec::new(), prelude: makepad_script_compute::parse_prelude(PRELUDE, user_len), paths: Vec::new() }
    }

    fn resolve(&mut self, items: Vec<parse::Item>, modules: &[Module]) -> Result<Vec<parse::Item>, ShaderError> {
        use makepad_script_compute::module;
        let roots = items.iter().filter(|i| !matches!(i, parse::Item::Use { .. } | parse::Item::Decl { .. })).map(|i| i.name().to_owned()).collect();
        let items = module::resolve_with(items, self.user_len + PRELUDE.len() + 2, modules, &self.prelude, &mut |path, at| {
            if !self.paths.iter().any(|p| p == path) {
                self.paths.push(path.to_owned());
            }
            at.filter(|span| span.start < self.user_len)
        })?;
        Ok(module::prune(items, &roots))
    }

    fn source(code: &str, bindings: &AudioBindings<'_>) -> Result<(Self, Vec<parse::Item>), ShaderError> {
        let tokens = parse::lex(code)?;
        let items = parse::Parser::new(&tokens).items()?;
        let mut imports = Self::new(code.len());
        let items = imports.resolve(items, bindings.modules)?;
        Ok((imports, items))
    }

    fn with_prelude(&mut self, items: Vec<parse::Item>) -> Vec<parse::Item> {
        // Prelude items the shader does not redefine come first (a shader's
        // state may be a prelude struct), then the shader's own.
        let names: std::collections::HashSet<String> = items.iter().filter(|i| !matches!(i, parse::Item::Decl { .. })).map(|i| i.name().to_string()).collect();
        let mut all: Vec<parse::Item> = std::mem::take(&mut self.prelude).into_iter().filter(|i| !names.contains(i.name())).collect();
        all.extend(items);
        all
    }

    fn lower(&mut self, items: Vec<parse::Item>, bindings: &AudioBindings<'_>, control: lower::CompileTime<'_>) -> Result<lower::Lowered, ShaderError> {
        let all = self.with_prelude(items);
        lower::lower_with_rate(&all, self.user_len + 1, &|name| {
            bindings.samples.iter().find(|s| s.name == name).map(|s| (s.channels, s.frames, s.data))
        }, bindings.oversample, control)
    }
}

/// A declaration query owns its parsed interface or diagnostic on the compiler side.
#[relax_api(handle)]
pub struct AudioDeclarations(Box<Result<lower::AudioInventory, ShaderError>>);

impl AudioBindings<'_> {
    #[inline(always)]
    pub fn declarations(&self, code: &str) -> AudioDeclarations {
        audio_declarations(code, self)
    }
}

#[relax_api]
pub fn audio_declarations(code: &str, bindings: &AudioBindings<'_>) -> AudioDeclarations {
    let result = AudioImports::source(code, bindings).and_then(|(mut imports, items)| {
        lower::audio_inventory(&imports.with_prelude(items))
    });
    AudioDeclarations(Box::new(result))
}

#[relax_api]
impl AudioDeclarations {
    pub fn has_error(&self) -> bool { self.0.is_err() }
    pub fn error(&self) -> makepad_script_compute::boundary::ErrorView<'_> {
        let e = self.0.as_ref().as_ref().unwrap_err();
        makepad_script_compute::boundary::ErrorView { kind: e.kind, start: e.start, end: e.end, message: &e.message }
    }

    pub fn kind(&self) -> Kind { self.inventory().kind }
    pub fn mono(&self) -> bool { self.inventory().mono }
    pub fn sample_count(&self) -> usize { self.inventory().samples.len() }
    pub fn sample(&self, index: usize) -> &str { &self.inventory().samples[index] }
    pub fn param_count(&self) -> usize { self.inventory().params.len() }
    pub fn param(&self, index: usize) -> &str { &self.inventory().params[index].0 }
    pub fn choice_count(&self, param: usize) -> usize { self.inventory().params[param].1.len() }
    pub fn choice(&self, param: usize, index: usize) -> &str { &self.inventory().params[param].1[index] }
    pub fn port_count(&self) -> usize { self.inventory().ports.len() }
    pub fn port(&self, index: usize) -> DeclaredPort<'_> {
        let (name, output, channels) = &self.inventory().ports[index];
        DeclaredPort { name, output: *output, channels: *channels }
    }
}

impl AudioDeclarations {
    fn inventory(&self) -> &lower::AudioInventory { self.0.as_ref().as_ref().expect("checked declaration query") }
}

#[relax_api(plain)]
#[repr(C)]
pub struct DeclaredPort<'a> {
    pub name: &'a str,
    pub output: bool,
    pub channels: u16,
}

/// Preflight a source for differential testing. Price each fold before executing
/// it, then price the fully expanded init and one render frame before backend selection.
pub fn exceeds_static_cost(code: &str, budget: u64) -> Result<bool, ShaderError> {
    use std::sync::atomic::{AtomicU32, Ordering};
    let bindings = AudioBindings::default();
    let (mut imports, items) = AudioImports::source(code, &bindings)?;
    let word = AtomicU32::new(0);
    let observe_cost = |cost| {
        if cost > budget { word.store(1, Ordering::Relaxed); }
    };
    let control = lower::CompileTime { word: Some(&word), observe_cost: Some(&observe_cost), ..bindings.compile_time() };
    let lowered = imports.lower(items, &bindings, control);
    if word.into_inner() != 0 { Ok(true) } else { lowered.map(|_| false) }
}

fn compile_items_with(items: Vec<parse::Item>, imports: AudioImports, backend: Backend, counting: bool, bindings: &AudioBindings<'_>) -> Result<Arc<AudioShaderBody>, Vec<ShaderError>> {
    compile_items(items, imports, backend, counting, bindings, bindings.compile_time())
}

fn compile_items(items: Vec<parse::Item>, mut imports: AudioImports, backend: Backend, counting: bool, bindings: &AudioBindings<'_>, control: lower::CompileTime<'_>) -> Result<Arc<AudioShaderBody>, Vec<ShaderError>> {
    let mut lowered = imports.lower(items, bindings, control).map_err(|e| if e.is_cancelled() { Vec::new() } else { vec![e] })?;
    let mut ctx_words = CTX_PARAMS as usize + lowered.params.len() + 3 * lowered.probes.len();
    let count_word = counting.then(|| {
        makepad_script_compute::work::count_runs(&mut lowered.render, ctx_words as u32);
        ctx_words += 2;
        ctx_words as u32 - 2
    });
    let state_words = lowered.state_init.len();
    let shared_words = lowered.shared_init.len().max(1);
    #[cfg(target_arch = "wasm32")]
    wasm::memory_pages(shared_words as u64 + ctx_words as u64 + state_words as u64, makepad_script_compute::wasm::frame_words(&lowered.render, false) as u64, lowered.render.channel_count() as u64, None).map_err(|e| vec![e])?;
    let regions = |p: &ir::Program, init: bool| ir::Regions {
        ctx: ctx_words as u32,
        state: state_words as u32,
        shared: shared_words as u32,
        frame: p.frame_words,
        shared_writable: init,
        bufs: if init && bindings.control.is_some() { vec![false] } else { Vec::new() },
        io: !init,
    };
    if let Err(e) = ir::validate(&lowered.render, &regions(&lowered.render, false)) {
        return Err(vec![ShaderError::new(0, 1, format!("internal compiler error: {}", e))]);
    }
    let mut shared = lowered.shared_init;
    if shared.is_empty() {
        shared.push(0);
    }
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    let native_ok = backend == Backend::Native;
    // Only the native targets choose between emitted code and the interpreter here.
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    let _ = backend;
    // Tables: run init() once (natively when possible: it can be big).
    if let Some(init) = &lowered.init {
        if let Err(e) = ir::validate(init, &regions(init, true)) {
            return Err(vec![ShaderError::new(0, 1, format!("internal compiler error in init: {}", e))]);
        }
        let mut ctx = vec![0u32; ctx_words];
        let mut state = Vec::new();
        state.try_reserve_exact(state_words.max(1)).map_err(|_| vec![ShaderError::new(0, 1,
            format!("could not allocate {} bytes for shader state", state_words.max(1) as u64 * 4))])?;
        state.resize(state_words.max(1), 0u32);
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        let native_init = (native_ok && control.poll_limit.is_none()).then(|| native::compile_with(init, 0)).flatten();
        if bindings.is_cancelled() { return Err(Vec::new()); }
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        let run = if let Some(code) = native_init {
            let table = bindings.control.map_or([0, 0], |word| [std::ptr::from_ref(word) as u64, 1]);
            // SAFETY: init has no channel I/O. Only a controlled init can read
            // descriptor zero, which then borrows the caller's aligned word.
            lower::executing(bindings.executions, || unsafe { code.run_kernel(ctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_mut_ptr(), table.as_ptr(), 1) });
            Ok(())
        } else {
            run_init_interp(init, &mut ctx, &mut state, &mut shared, control)
        };
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        let run = run_init_interp(init, &mut ctx, &mut state, &mut shared, control);
        if bindings.is_cancelled() { return Err(Vec::new()); }
        run.map_err(|limit| vec![ShaderError::poll_limit(lowered.init_span.expect("init has a declaration or FFT site"), "init()", limit)])?;
    }
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    let native = if native_ok { native::compile_with(&lowered.render, 0) } else { None };
    let (params, param_meta, state_vars) = (lowered.params, lowered.param_meta, lowered.state_vars);
    let state_init = lowered.state_init.into_boxed_slice();
    let layout = StateLayout::intern(lowered.kind, &params, &state_vars, state_words);
    // SAFETY: the strings and boxes borrowed by these views belong to the
    // body built below and are never changed or dropped before it; an
    // AudioShader hands them out only for the length of a borrow of itself.
    let param_views: Box<[ParamInfoView<'static>]> = params.iter().map(|p| ParamInfoView { name: unsafe { forever(p.name.as_str()) }, default: p.default, min: p.min, max: p.max }).collect();
    let ports = lowered.ports;
    let port_views: Box<[PortInfoView<'static>]> = ports.iter().map(|p| PortInfoView {
        name: unsafe { forever(p.name.as_str()) }, output: p.output, channels: p.channels, first: p.first, control: p.control,
    }).collect();
    let sample_names = lowered.sample_names;
    let sample_views: Box<[&'static str]> = sample_names.iter().map(|name| unsafe { forever(name.as_str()) }).collect();
    let probe_names = lowered.probes;
    let probe_views: Box<[&'static str]> = probe_names.iter().map(|name| unsafe { forever(name.as_str()) }).collect();
    let choice_lists: Vec<Box<[&'static str]>> = param_meta.iter().map(|m| {
        m.choices.iter().map(|name| unsafe { forever(name.as_str()) }).collect()
    }).collect();
    let param_meta_views: Box<[ParamMetaView<'static>]> = param_meta.iter().zip(&choice_lists).map(|(m, choices)| ParamMetaView {
        unit: m.unit,
        curve: m.curve,
        step: m.step,
        smooth: m.smooth.unwrap_or(-1.0),
        label: unsafe { forever(m.label.as_str()) },
        group: unsafe { forever(m.group.as_str()) },
        choices: unsafe { forever(choices.as_ref()) },
    }).collect();
    let state_var_views: Box<[boundary::StateVarView<'static>]> = state_vars.iter().map(|v| boundary::StateVarView { name: unsafe { forever(v.name.as_str()) }, sig: unsafe { forever(v.sig.as_str()) }, offset: v.offset, words: v.words }).collect();
    let warnings = lowered.warnings;
    let warning_views: Box<[_]> = warnings.iter().map(|w| {
        let (node, span) = fuse::source_span(w.span, &imports.node_ranges);
        boundary::WarningView { node: node.unwrap_or(boundary::NO_NODE), start: span.start, end: span.end,
            message: unsafe { forever(w.message.as_str()) } }
    }).collect();
    let shader = Arc::new_cyclic(|body| {
        let head = AudioShader {
            warnings: unsafe { forever(&*warning_views) },
            kind: lowered.kind,
            mono: lowered.mono as u32,
            latency: lowered.latency,
            params: unsafe { forever(&*param_views) },
            ports: unsafe { forever(&*port_views) },
            input_channels: lowered.render.input_channels,
            output_channels: lowered.render.output_channels,
            probes: unsafe { forever(&*probe_views) },
            samples: unsafe { forever(&*sample_views) },
            param_metas: unsafe { forever(&*param_meta_views) },
            state_vars: unsafe { forever(&*state_var_views) },
            state_init: unsafe { forever(&*state_init) },
            state_layout: layout.as_ref().map_or(0, |layout| Arc::as_ptr(layout) as usize),
            count_word: count_word.unwrap_or(NO_COUNT_WORD),
            scratch_words: render_scratch_words(&lowered.render),
            body: body.as_ptr(),
        };
        AudioShaderBody {
            head,
            params,
            port_info: ports,
            _port_views: port_views,
            _warning_views: warning_views,
            _warnings: warnings,
            module_paths: imports.paths,
            _sample_names: sample_names,
            _sample_views: sample_views,
            _probe_names: probe_names,
            _probe_views: probe_views,
            param_meta,
            param_views,
            param_meta_views,
            choice_lists,
            state_init,
            _state_layout: layout,
            state_vars,
            state_var_views,
            shared: shared.into_boxed_slice(),
            render: lowered.render,
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            native,
            key: std::sync::OnceLock::new(),
        }
    });
    // The key is what a browser host's audio thread looks the generated
    // code up by: computed here, never first on the audio thread.
    #[cfg(target_arch = "wasm32")]
    shader.program_key();
    if bindings.is_cancelled() { return Err(Vec::new()); }
    wasm::note_compiled(&shader);
    Ok(shader)
}

// Compiled backends share the caller's scratch with the interpreter. Their
// pointer table follows the frame, with room to align a pointer on 64-bit hosts.
fn render_scratch_words(p: &ir::Program) -> usize {
    let ptr_words = std::mem::size_of::<usize>() / std::mem::size_of::<u32>();
    p.scratch_words().max(p.frame_words as usize + p.channel_count() * ptr_words + ptr_words - 1)
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64", target_arch = "wasm32"))]
fn write_channel_table<'a>(storage: &'a mut [u32], io: &mut ir::Io) -> &'a [usize] {
    let count = io.ins.len() + io.outs.len();
    let ptr_words = std::mem::size_of::<usize>() / std::mem::size_of::<u32>();
    // Words to the first usize-aligned one (render_scratch_words reserves them).
    let skip = (storage.as_ptr() as usize).wrapping_neg() % std::mem::align_of::<usize>() / std::mem::size_of::<u32>();
    let words = &mut storage[skip..skip + count * ptr_words];
    // SAFETY: `words` starts usize-aligned, holds `count` usizes, is
    // exclusively borrowed from the instance's scratch, and every bit
    // pattern is a valid usize.
    let table = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<usize>(), count) };
    let (inputs, outputs) = table.split_at_mut(io.ins.len());
    for (ptr, samples) in inputs.iter_mut().zip(io.ins.iter()) {
        *ptr = samples.as_ptr() as usize;
    }
    for (ptr, samples) in outputs.iter_mut().zip(io.outs.iter_mut()) {
        *ptr = samples.as_mut_ptr() as usize;
    }
    table
}

fn run_init_interp(init: &ir::Program, ctx: &mut [u32], state: &mut [u32], shared: &mut [u32], control: lower::CompileTime<'_>) -> Result<(), ir::PollLimitReached> {
    let mut scratch = vec![0u32; init.scratch_words()];
    let buf = control.word.map(|word| ir::RawBuf { ptr: std::ptr::from_ref(word).cast::<u32>().cast_mut(), len: 1, writable: false, elem: ir::Elem::Word });
    let mut mem = ir::Mem { ctx, state, shared: ir::Shared::Write(shared), bufs: buf.as_slice() };
    lower::executing(control.executions, || ir::run_compile_time(init, &mut scratch, &mut mem, &mut ir::Io::default(), buf.is_some(), control.poll_limit)).map(|_| ())
}

impl std::fmt::Debug for AudioShader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AudioShader({:?}, {} params, {} state words)", self.kind, self.params.len(), self.state_init.len())
    }
}

impl AudioShader {
    #[inline]
    pub fn warnings(&self) -> &[boundary::WarningView<'_>] { self.warnings }

    #[inline]
    pub fn kind(&self) -> Kind { self.kind }

    #[inline]
    pub fn param_count(&self) -> usize { self.params.len() }

    #[inline]
    pub fn port_count(&self) -> usize { self.ports.len() }

    #[inline]
    pub fn port_view(&self, index: usize) -> PortInfoView<'_> { self.ports[index] }

    #[inline]
    pub fn input_channels(&self) -> usize { usize::from(self.input_channels) }

    #[inline]
    pub fn output_channels(&self) -> usize { usize::from(self.output_channels) }

    #[inline]
    pub fn param_view(&self, index: usize) -> ParamInfoView<'_> { self.params[index] }

    #[inline]
    pub fn param_meta(&self, index: usize) -> ParamMetaView<'_> { self.param_metas[index] }

    #[inline]
    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    #[inline]
    pub fn sample_names(&self) -> &[&str] { self.samples }

    #[inline]
    pub fn probe_count(&self) -> usize { self.probes.len() }

    #[inline]
    pub fn probe_name(&self, index: usize) -> &str { self.probes[index] }

    #[inline]
    pub fn probe_base(&self) -> usize { CTX_PARAMS as usize + self.params.len() }

    /// Last, minimum and maximum from the most recent call that filled this context.
    pub fn probe_read(&self, ctx: &[u32], index: usize) -> Option<(f32, f32, f32)> {
        if index >= self.probe_count() {
            return None;
        }
        let word = self.probe_base() + 3 * index;
        let min = f32::from_bits(ctx[word + 1]);
        let max = f32::from_bits(ctx[word + 2]);
        if min > max { None } else { Some((f32::from_bits(ctx[word]), min, max)) }
    }

    #[inline]
    pub fn state_words(&self) -> usize {
        self.state_init.len()
    }

    /// The initial image of a voice or effect state.
    #[inline]
    pub fn state_init(&self) -> &[u32] {
        self.state_init
    }

    #[inline]
    pub fn ctx_words(&self) -> usize {
        self.probe_base() + 3 * self.probe_count() + 2 * (self.count_word != NO_COUNT_WORD) as usize
    }

    /// The first of two ctx words holding the low and high halves of the run’s op count.
    #[inline]
    pub fn count_word(&self) -> Option<u32> {
        (self.count_word != NO_COUNT_WORD).then_some(self.count_word)
    }

    /// Words for the interpreter or the compiled frame and aligned channel pointers.
    #[inline]
    pub fn scratch_words(&self) -> usize {
        self.scratch_words
    }

    /// Fills a caller-owned ctx block of [`AudioShader::ctx_words`] words:
    /// the sample rate and every param at its default. The caller allocates
    /// ctx, state ([`AudioShader::state_words`], then
    /// [`AudioShader::reset_state`]) and scratch ([`AudioShader::scratch_words`],
    /// zeroed) off the audio thread and keeps them for the instance's life.
    pub fn init_ctx(&self, ctx: &mut [u32], sample_rate: f32) {
        assert!(ctx.len() == self.ctx_words(), "ctx is ctx_words() words");
        ctx.fill(0);
        ctx[CTX_RATE as usize] = sample_rate.to_bits();
        self.set_time(ctx, 120.0, 0.0, 0, 0.0, 4.0, false);
        for (k, p) in self.params.iter().enumerate() {
            ctx[CTX_PARAMS as usize + k] = p.default.to_bits();
        }
        for k in 0..self.probe_count() {
            let word = self.probe_base() + 3 * k;
            ctx[word..word + 3].copy_from_slice(&[f32::NAN.to_bits(), f32::INFINITY.to_bits(), f32::NEG_INFINITY.to_bits()]);
        }
    }

    /// Set the transport segment. Beat and slope round once at this boundary.
    pub fn set_time(&self, ctx: &mut [u32], tempo: f32, beat_at: f64, beat_frame: u32, beats_per_frame: f64, beats_per_bar: f32, playing: bool) {
        ctx[CTX_TEMPO as usize] = tempo.to_bits();
        ctx[CTX_BEAT_AT as usize] = (beat_at as f32).to_bits();
        ctx[CTX_BEAT_FRAME as usize] = beat_frame;
        ctx[CTX_BEAT_STEP as usize] = (if playing { beats_per_frame as f32 } else { 0.0 }).to_bits();
        ctx[CTX_BAR_BEATS as usize] = beats_per_bar.to_bits();
        ctx[CTX_PLAYING as usize] = u32::from(playing);
    }

    /// The value a param takes when set to `value`: within its range, on
    /// its step, its default for a value that is not finite.
    pub fn param_value(&self, index: usize, value: f32) -> f32 {
        let p = self.params[index];
        let v = if value.is_finite() { value.clamp(p.min, p.max) } else { p.default };
        let step = self.param_metas[index].step;
        if step <= 0.0 {
            return v;
        }
        let steps = (v - p.min) / step;
        let snapped = p.min + steps.round() * step;
        // Across a wide range even adjacent indices can be the same
        // float. Find the offset from the same grid near v instead.
        let snapped = if steps + 1.0 == steps || !snapped.is_finite() {
            let (v, min, step) = (f64::from(v), f64::from(p.min), f64::from(step));
            let offset = (v % step - min % step).rem_euclid(step);
            (if offset >= step * 0.5 { v + (step - offset) } else { v - offset }) as f32
        } else {
            snapped
        };
        snapped.clamp(p.min, p.max)
    }

    /// Sets a parameter target for the next control block.
    pub fn set_param(&self, ctx: &mut [u32], index: usize, value: f32) {
        if index < self.params.len() {
            ctx[CTX_PARAMS as usize + index] = self.param_value(index, value).to_bits();
        }
    }

    pub fn set_cc(&self, ctx: &mut [u32], cc: u8, value: f32) {
        if u32::from(cc) < CC_WORDS {
            ctx[(CTX_CC + u32::from(cc)) as usize] = unit_value(value, 0.0).to_bits();
        }
    }

    /// Copies the full initial image into a voice or effect state, without allocation.
    pub fn reset_state(&self, state: &mut [u32]) {
        state.copy_from_slice(self.state_init);
    }

    /// Starts a note on a voice state: reset, then note inputs; `frame` is
    /// the absolute frame the note starts at, `seed` seeds `rand()`.
    pub fn note_on(&self, state: &mut [u32], note: f32, velocity: f32, frame: u32, seed: u32, voice: u32) {
        self.reset_state(state);
        state[header::GLIDE_NOTE as usize] = note.to_bits();
        state[header::GLIDE_STEP as usize] = 0;
        state[header::NOTE as usize] = note.to_bits();
        state[header::GLIDE_REMAINDER as usize] = 0;
        state[header::GATE as usize] = 1f32.to_bits();
        state[header::VELOCITY as usize] = velocity.to_bits();
        state[header::TRIGGER as usize] = frame;
        state[header::VOICE as usize] = voice;
        self.seed_state(state, seed);
    }

    pub fn set_instance(&self, ctx: &mut [u32], seed: u32) {
        let instance = (mix_seed(seed, 0x1d) >> 8) as f32 / 16_777_216.0;
        ctx[CTX_INSTANCE as usize] = instance.to_bits();
    }

    pub fn seed_state(&self, state: &mut [u32], seed: u32) {
        state[header::RNG as usize] = seed.wrapping_mul(0x9e37_79b9) | 1;
    }

    /// Hot swap: fills `state` (this shader's layout) from `old_state` (the
    /// `old` shader's): the note inputs, every param's smoothing, and every
    /// state variable whose name and type match; the rest starts from its
    /// initial value. Returns true when everything carried (a seamless
    /// swap); otherwise the host should crossfade. No allocation.
    pub fn carry_state(&self, state: &mut [u32], old: &AudioShader, old_state: &[u32]) -> bool {
        self.reset_state(state);
        self.carry_fresh_state(state, old, old_state)
    }

    /// True when carrying would overwrite every word from the same old offset.
    #[inline]
    pub fn same_state_layout(&self, old: &AudioShader) -> bool {
        self.state_layout != 0 && self.state_layout == old.state_layout
    }

    /// Carries into a destination whose non-header words still hold their
    /// initial values. Seed and note inputs may differ: carry replaces the
    /// entire header. Use `carry_state` for rendered or previously carried state.
    pub fn carry_fresh_state(&self, state: &mut [u32], old: &AudioShader, old_state: &[u32]) -> bool {
        let h = lower::HEADER_WORDS as usize;
        state[..h].copy_from_slice(&old_state[..h]);
        let mut all = self.kind == old.kind;
        for (k, p) in self.params.iter().enumerate() {
            if let Some(j) = old.param_index(p.name) {
                let (a, b) = (h + 2 * k, h + 2 * j);
                state[a] = old_state[b];
                state[a + 1] = old_state[b + 1];
            }
        }
        for v in self.state_vars {
            match old.state_vars.iter().find(|o| o.name == v.name && o.sig == v.sig) {
                Some(o) => {
                    let (a, b) = (v.offset as usize, o.offset as usize);
                    state[a..a + v.words as usize].copy_from_slice(&old_state[b..b + o.words as usize]);
                }
                None => all = false,
            }
        }
        if !all {
            // block() and the param ramps restart on the next frame.
            state[header::FRESH as usize] = 1;
        }
        all
    }

    /// A legato note change: new pitch and velocity without a retrigger
    /// (state, envelopes and `trigger` untouched; slides are the shader's).
    pub fn legato(&self, state: &mut [u32], note: f32, velocity: f32) {
        state[header::GLIDE_NOTE as usize] = note.to_bits();
        state[header::GLIDE_STEP as usize] = 0;
        state[header::NOTE as usize] = note.to_bits();
        state[header::VELOCITY as usize] = velocity.to_bits();
        state[header::GATE as usize] = 1f32.to_bits();
    }

    /// Slide from the currently sounding pitch, including a slide retargeted in flight.
    pub fn glide(&self, state: &mut [u32], note: f32, velocity: f32, secs: f32, rate: f32) {
        let current = f32::from_bits(state[header::GLIDE_NOTE as usize]);
        self.legato(state, note, velocity);
        if secs > 0.0 && note != current {
            let frames = f64::from((secs * rate).max(1.0));
            let distance = f64::from(note) - f64::from(current);
            let step = (distance.abs() / frames) as f32;
            if step > 0.0 {
                let whole = (distance / f64::from(step)).round();
                let correction = (frames.copysign(distance) - whole) as f32;
                state[header::GLIDE_REMAINDER as usize] = correction.to_bits();
                state[header::GLIDE_NOTE as usize] = current.to_bits();
                state[header::GLIDE_STEP as usize] = step.to_bits();
            }
        }
    }

    pub fn set_expression(&self, state: &mut [u32], which: Expression, value: f32) {
        let (word, value) = match which {
            Expression::Bend => (header::BEND, if value.is_finite() { value } else { 0.0 }),
            Expression::Pressure => (header::PRESSURE, unit_value(value, 0.0)),
            Expression::Slide => (header::SLIDE, unit_value(value, 0.0)),
        };
        state[word as usize] = value.to_bits();
    }

    pub fn note_off(&self, state: &mut [u32], release_velocity: f32) {
        state[header::GATE as usize] = 0f32.to_bits();
        state[header::RELEASE_VELOCITY as usize] = unit_value(release_velocity, 0.5).to_bits();
    }

    pub fn stopped(&self, state: &[u32]) -> bool {
        state[header::STOP as usize] != 0
    }
}

impl AudioShaderBody {
    pub fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    pub fn param_metas(&self) -> &[ParamMeta] {
        &self.param_meta
    }

    pub fn state_vars(&self) -> &[StateVar] {
        &self.state_vars
    }

    pub fn backend(&self) -> Backend {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if self.native.is_some() {
            return Backend::Native;
        }
        Backend::Interp
    }

    /// The AIR render program (for tests and tools).
    pub fn program(&self) -> &ir::Program {
        &self.render
    }

    /// The shared tables (built by `init()`; the render program reads them).
    pub fn shared_table(&self) -> &[u32] {
        &self.shared
    }

    /// Bytes of native code, if any.
    pub fn native_code_bytes(&self) -> usize {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(code) = &self.native {
            return code.len();
        }
        0
    }

    /// The reference interpreter, whatever the backend.
    #[allow(clippy::too_many_arguments)]
    pub fn run_interp(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], n: usize) {
        let n = n.min(MAX_FRAMES as usize);
        let mut mem = ir::Mem { ctx, state, shared: ir::Shared::Read(&self.shared), bufs: &[] };
        let mut io = ir::Io { ins: &[in_l, in_r], outs: &mut [out_l, out_r] };
        ir::run(&self.render, scratch, &mut mem, &mut io, n as u32);
    }
}

impl AudioShaderBody {
    #[inline(always)]
    fn run_io(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], io: &mut ir::Io, n: usize) {
        let n = n.min(MAX_FRAMES as usize);
        if n == 0 {
            return;
        }
        assert!(ctx.len() == self.head.ctx_words() && state.len() == self.state_init.len());
        assert_eq!(io.ins.len(), usize::from(self.render.input_channels));
        assert_eq!(io.outs.len(), usize::from(self.render.output_channels));
        assert!(io.ins.iter().all(|s| s.len() >= n) && io.outs.iter().all(|s| s.len() >= n));
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(code) = &self.native {
            let table = write_channel_table(&mut scratch[self.render.frame_words as usize..], io);
            // SAFETY-relevant contract: `run` only touches the slices
            // passed here, with every access clamped into them.
            // SAFETY: the render program never stores to shared tables
            // (validated), so handing native code a pointer derived from
            // the immutable table only ever reads it.
            unsafe { code.run_audio(ctx, state, self.shared.as_ptr() as *mut u32, table, n as u32) };
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(slot) = wasm::slot(self.program_key()) {
            wasm::run_slot(&self.head, slot, ctx, state, scratch, io, n);
            return;
        }
        let mut mem = ir::Mem { ctx, state, shared: ir::Shared::Read(&self.shared), bufs: &[] };
        ir::run(&self.render, scratch, &mut mem, io, n as u32);
    }
}

/// The run entry: the one door the audio thread renders through.
#[relax_api]
impl AudioShaderBody {
    /// The render program's identity across hosts (the compute core's
    /// `kernel::program_key`): a build tool compiles a document's shaders
    /// to wasm ahead of time under these keys, and the browser host links
    /// them by key ([`wasm::link`]).
    pub fn program_key(&self) -> u64 {
        *self.key.get_or_init(|| makepad_script_compute::kernel::program_key(&self.render))
    }

    /// Whether the host supplied this module while compiling the shader.
    pub fn uses_module(&self, path: &str) -> bool {
        self.module_paths.iter().any(|p| p == path)
    }

    /// Renders `n` frames (1..=MAX_FRAMES), ADDING into `outs`. `ctx`
    /// holds the absolute frame of the first one (`CTX_FRAME`). Realtime
    /// safe: no allocation, no locks, bounded time.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], n: usize) {
        self.run_io(ctx, state, scratch, &mut ir::Io { ins: &[in_l, in_r], outs: &mut [out_l, out_r] }, n);
    }

    /// Render with the declared channel tables, main stereo pair first in each direction.
    #[allow(clippy::too_many_arguments)]
    pub fn run_ports(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], ins: &[&[f32]], outs: &mut [&mut [f32]], n: usize) {
        self.run_io(ctx, state, scratch, &mut ir::Io { ins, outs }, n)
    }
}

#[derive(Default)]
struct ChannelTables<'a> {
    ins: Vec<&'a [f32]>,
    outs: Vec<&'a mut [f32]>,
}

impl ChannelTables<'_> {
    fn empty<'call>(mut self) -> ChannelTables<'call> {
        self.ins.clear();
        self.outs.clear();
        let mut tables = std::mem::ManuallyDrop::new(self);
        // SAFETY: both allocations are empty. Only the reference lifetimes change.
        // Each Vec takes back its own allocation with the original capacity.
        unsafe {
            ChannelTables {
                ins: Vec::from_raw_parts(tables.ins.as_mut_ptr().cast(), 0, tables.ins.capacity()),
                outs: Vec::from_raw_parts(tables.outs.as_mut_ptr().cast(), 0, tables.outs.capacity()),
            }
        }
    }
}

#[derive(Default)]
struct UnconnectedPorts {
    tables: ChannelTables<'static>,
    discard: Vec<f32>,
}

impl UnconnectedPorts {
    fn new(shader: &AudioShader) -> Self {
        if shader.port_count() == 0 {
            return Self::default();
        }
        Self {
            tables: ChannelTables {
                ins: Vec::with_capacity(shader.input_channels()),
                outs: Vec::with_capacity(shader.output_channels()),
            },
            discard: vec![0.0; (shader.output_channels() - if shader.kind == Kind::Control { 0 } else { 2 }) * (MAX_FRAMES as usize)],
        }
    }

    fn clear(&mut self, n: usize) {
        for channel in self.discard.chunks_exact_mut(MAX_FRAMES as usize) {
            channel[..n].fill(0.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, shader: &AudioShader, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32],
        ins: [&[f32]; 2], outs: [&mut [f32]; 2], silence: &[f32], n: usize) {
        if shader.port_count() == 0 {
            let [l, r] = outs;
            shader.run(ctx, state, scratch, ins[0], ins[1], l, r, n);
            return;
        }
        let mut tables = std::mem::take(&mut self.tables).empty();
        if shader.kind != Kind::Control {
            tables.ins.extend(ins);
            tables.ins.resize(shader.input_channels(), &silence[..n]);
            tables.outs.extend(outs);
        }
        tables.outs.extend(self.discard.chunks_exact_mut(MAX_FRAMES as usize).map(|ch| &mut ch[..n]));
        shader.run_ports(ctx, state, scratch, &tables.ins, &mut tables.outs, n);
        self.tables = tables.empty();
    }
}

/// One playable instance (a voice or an effect) with its own frame
/// counter: the simple host for tests, tools and offline renders.
#[relax_api(plain)]
pub struct Instance {
    pub shader: AudioShaderRef,
    pub ctx: Box<[u32]>,
    pub state: Box<[u32]>,
    scratch: Box<[u32]>,
    frame: u32,
    voice: u32,
    zeros: Box<[f32]>,
    ports: UnconnectedPorts,
    ops: u64,
}

impl Instance {
    pub fn new(shader: AudioShaderRef, sample_rate: f32) -> Self {
        let mut ctx = vec![0u32; shader.ctx_words()].into_boxed_slice();
        shader.init_ctx(&mut ctx, sample_rate);
        Instance {
            ctx,
            state: shader.state_init().into(),
            scratch: vec![0u32; shader.scratch_words()].into_boxed_slice(),
            frame: 0,
            voice: 0,
            zeros: vec![0.0; MAX_FRAMES as usize].into_boxed_slice(),
            ports: UnconnectedPorts::new(&shader),
            ops: 0,
            shader,
        }
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn set_time(&mut self, tempo: f32, beat_at: f64, beat_frame: u32, beats_per_frame: f64, beats_per_bar: f32, playing: bool) {
        self.shader.set_time(&mut self.ctx, tempo, beat_at, beat_frame, beats_per_frame, beats_per_bar, playing);
    }

    /// The ops this instance's runs counted so far (a [`compile_counting`]
    /// build; 0 otherwise).
    pub fn ops(&self) -> u64 {
        self.ops
    }

    fn counted(&mut self) {
        if let Some(w) = self.shader.count_word() {
            self.ops = self.ops.saturating_add(makepad_script_compute::work::read_count(&self.ctx, w as usize));
        }
    }

    pub fn set_param(&mut self, name: &str, value: f32) -> bool {
        match self.shader.param_index(name) {
            Some(k) => {
                self.shader.set_param(&mut self.ctx, k, value);
                true
            }
            None => false,
        }
    }

    pub fn seed(&mut self, seed: u32) {
        self.shader.seed_state(&mut self.state, seed);
        self.shader.set_instance(&mut self.ctx, seed);
    }

    pub fn set_voice(&mut self, voice: u32) {
        self.voice = voice;
    }

    pub fn note_on(&mut self, note: f32, velocity: f32, seed: u32) {
        self.shader.note_on(&mut self.state, note, velocity, self.frame, seed, self.voice);
    }

    pub fn set_expression(&mut self, which: Expression, value: f32) {
        self.shader.set_expression(&mut self.state, which, value);
    }

    pub fn set_cc(&mut self, cc: u8, value: f32) {
        self.shader.set_cc(&mut self.ctx, cc, value);
    }

    pub fn note_off(&mut self, release_velocity: f32) {
        self.shader.note_off(&mut self.state, release_velocity);
    }

    /// Instrument render: adds `l.len()` frames into l/r.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let mut done = 0;
        while done < n {
            let k = (n - done).min(MAX_FRAMES as usize);
            self.ctx[CTX_FRAME as usize] = self.frame;
            let zeros = &self.zeros[..k];
            self.ports.clear(k);
            self.ports.run(&self.shader, &mut self.ctx, &mut self.state, &mut self.scratch, [zeros, zeros],
                [&mut l[done..done + k], &mut r[done..done + k]], zeros, k);
            self.counted();
            self.frame = self.frame.wrapping_add(k as u32);
            done += k;
        }
    }

    /// Effect render: reads in_l/in_r, adds into out_l/out_r.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut done = 0;
        while done < n {
            let k = (n - done).min(MAX_FRAMES as usize);
            self.ctx[CTX_FRAME as usize] = self.frame;
            self.ports.clear(k);
            self.ports.run(&self.shader, &mut self.ctx, &mut self.state, &mut self.scratch,
                [&in_l[done..done + k], &in_r[done..done + k]], [&mut out_l[done..done + k], &mut out_r[done..done + k]], &self.zeros, k);
            self.counted();
            self.frame = self.frame.wrapping_add(k as u32);
            done += k;
        }
    }
}

/// Share of realtime allocated to score shaders, reserving the other half for the engine, mixing, the host and scheduling jitter.
const SHADER_REALTIME_SHARE: f64 = 0.5;
/// The mean budget lets this many instances share the score's shader time equally.
const MEAN_INSTANCES: u32 = 32;
/// The peak budget allows this many instances to do their block-boundary work together.
const PEAK_INSTANCES: u32 = 8;
/// Nanoseconds per counted op on wasm in CEF on a Linux node, the library audition's mix, 2026-11-25.
/// Measured on drained timed leases by dividing total render time by total counted ops.
const NS_PER_COUNTED_OP: f64 = 0.0839;
/// Sample rate in Hz used by the `check` audition and its budget calculations.
const AUDITION_RATE: f32 = 48_000.0;
/// The web audio worklet's render quantum is the shortest callback used for the peak budget.
const SHORTEST_CALLBACK_FRAMES: u32 = 128;

/// Counted ops available to all score shaders per audition frame at the measured rate.
const SHADER_OPS_PER_FRAME: f64 = SHADER_REALTIME_SHARE * 1e9 / (AUDITION_RATE as f64 * NS_PER_COUNTED_OP);

/// At the measured wasm rate, 32 instances share half of realtime at 48 kHz.
/// The mean warning line is one instance's share in whole counted ops per frame, rounded down.
pub const EXPENSIVE_OPS_PER_FRAME: f64 = (SHADER_OPS_PER_FRAME / MEAN_INSTANCES as f64) as u64 as f64;
/// Eight coincident peaks share half of the 128-frame worklet callback at the measured wasm rate.
/// The peak warning line expresses one instance's share over its busiest 64-frame block in whole ops per frame, rounded down.
pub const EXPENSIVE_PEAK_OPS_PER_FRAME: f64 =
    (SHADER_OPS_PER_FRAME * SHORTEST_CALLBACK_FRAMES as f64 / PEAK_INSTANCES as f64 / CONTROL_BLOCK as f64) as u64 as f64;

/// What [`check`] found: a compile plus an offline audition of a test
/// phrase, for the editor and the AI composer's shader tools.
#[derive(Clone, Debug)]
pub struct CheckReport {
    pub kind: Kind,
    /// Frames of latency the shader reports to its host.
    pub latency: u32,
    pub params: Vec<ParamInfo>,
    pub ports: Vec<AudioPortInfo>,
    /// Minimum and maximum over the audition. An unreached probe has min > max.
    pub probes: Vec<(String, f32, f32)>,
    pub param_meta: Vec<ParamMeta>,
    pub state_words: usize,
    pub backend: Backend,
    /// Largest absolute sample.
    pub peak: f32,
    pub rms: f32,
    /// Mean of the output (a DC offset shows here).
    pub dc: f32,
    /// NaN or infinite samples.
    pub nonfinite: usize,
    /// Share of samples at or above full scale.
    pub clipped: f32,
    /// What one instance runs a frame, in counted ops (the same on every
    /// machine; see [`EXPENSIVE_OPS_PER_FRAME`]).
    pub ops_per_frame: f64,
    /// Counted ops per frame in the busiest 64-frame call on any one voice,
    /// effect or control node, including the fixed cost of that call.
    pub peak_ops_per_block: f64,
    /// Instruments: seconds from the last release until the voice stopped
    /// or went quiet (-80 dB); None if it never did within 8 s.
    pub tail_secs: Option<f32>,
    /// Plain-language problems found (empty when it looks healthy).
    pub warnings: Vec<String>,
}

fn audition_signal(frames: usize, rate: f32) -> impl Iterator<Item = (f32, f32)> {
    let mut phases = [0.0f32; 3];
    (0..frames).map(move |i| {
        let mut x = 0.0;
        for (k, f) in [130.8f32, 164.8, 196.0].iter().enumerate() {
            phases[k] = (phases[k] + f / rate).fract();
            x += (2.0 * phases[k] - 1.0) * 0.15;
        }
        if i % 12000 == 0 {
            x += 0.5;
        }
        (x, x * 0.9)
    })
}

const AUDITION_FRAMES: usize = 3 * AUDITION_RATE as usize;

fn compile_audition(code: &str, counting: bool, bindings: &AudioBindings<'_>) -> Result<AudioShaderRef, Vec<ShaderError>> {
    with_audition_recording(bindings, |b| boundary::compile_audio_with(code, Backend::Native, counting, b).into_result())
}

fn with_audition_recording(
    bindings: &AudioBindings<'_>,
    compile: impl Fn(&AudioBindings<'_>) -> Result<AudioShaderRef, Vec<ShaderError>>,
) -> Result<AudioShaderRef, Vec<ShaderError>> {
    let mut shader = compile(bindings)?;
    let missing: Vec<_> = shader.sample_names().iter().copied().filter(|name| !bindings.samples.iter().any(|s| s.name == *name)).collect();
    if !missing.is_empty() {
        let frames = AUDITION_RATE as u32;
        let recording: Vec<_> = audition_signal(frames as usize, AUDITION_RATE).flat_map(|(l, r)| [l, r]).collect();
        let mut samples = bindings.samples.to_vec();
        samples.extend(missing.iter().map(|name| SampleView { name, channels: 2, frames, data: &recording }));
        shader = compile(&AudioBindings { samples: &samples, ..*bindings })?;
    }
    Ok(shader)
}

struct Audition {
    outputs: Vec<Vec<f32>>,
    tail_secs: Option<f32>,
    probes: Vec<(String, f32, f32)>,
    ops_per_frame: f64,
}

fn set_audition_time(instance: &mut Instance, start: usize) {
    const TEMPO: f32 = 120.0;
    let step = TEMPO as f64 / 60.0 / AUDITION_RATE as f64;
    instance.set_time(TEMPO, start as f64 * step, 0, step, 4.0, true);
}

fn audition_input(kind: Kind) -> [Vec<f32>; 2] {
    if kind != Kind::Effect {
        return Default::default();
    }
    let (left, right) = audition_signal(AUDITION_FRAMES, AUDITION_RATE).unzip();
    [left, right]
}

fn audition(shader: &AudioShaderRef, input: &[Vec<f32>; 2]) -> Audition {
    let n = AUDITION_FRAMES;
    let channels = if shader.kind == Kind::Control { shader.output_channels() } else { 2 };
    let mut outputs = vec![vec![0.0f32; n]; channels];
    let mut tail_secs = None;
    let mut instance_frames = 0usize;
    let mut probes: Vec<_> = (0..shader.probe_count()).map(|k| (shader.probe_name(k).to_owned(), f32::INFINITY, f32::NEG_INFINITY)).collect();
    let mut observe = |ctx: &[u32]| {
        for (k, (_, lo, hi)) in probes.iter_mut().enumerate() {
            if let Some((_, min, max)) = shader.probe_read(ctx, k) {
                *lo = lo.min(min);
                *hi = hi.max(max);
            }
        }
    };
    let ops: u64;
    match shader.kind {
        Kind::Instrument => {
            let notes = [48.0f32, 52.0, 55.0];
            let release_at = AUDITION_RATE as usize;
            let start_of = |i: usize| i * release_at / 4;
            // A voice's frame 0 is its note's start on the song's clock.
            let mut voices: Vec<Instance> = (0..if shader.mono() { 1 } else { notes.len() })
                .map(|i| {
                    let mut v = Instance::new(shader.clone(), AUDITION_RATE);
                    v.set_voice(i as u32);
                    set_audition_time(&mut v, start_of(i));
                    v
                })
                .collect();
            let mut last_loud = 0usize;
            let mut stopped_at = None;
            let (mut vl, mut vr) = (vec![0.0f32; 128], vec![0.0f32; 128]);
            let mut at = 0;
            while at < n {
                // A block ends at the next event (a note start, the
                // release), so each note starts on its own frame.
                let next = (0..notes.len()).map(start_of).chain([release_at]).filter(|&e| e > at).min().unwrap_or(n);
                let k = 128.min(n - at).min(next - at);
                let mut block_peak = 0.0f32;
                let mut all_stopped = at >= release_at;
                for (i, v) in voices.iter_mut().enumerate() {
                    let start = start_of(i);
                    if at == start {
                        v.note_on(notes[i], 0.8, i as u32 + 1);
                    }
                    if shader.mono() {
                        for j in 1..notes.len() {
                            if at == start_of(j) {
                                shader.legato(&mut v.state, notes[j], 0.8);
                            }
                        }
                    }
                    if at == release_at {
                        v.note_off(0.5);
                    }
                    if at < start || shader.stopped(&v.state) {
                        continue;
                    }
                    all_stopped = false;
                    vl[..k].fill(0.0);
                    vr[..k].fill(0.0);
                    v.render(&mut vl[..k], &mut vr[..k]);
                    observe(&v.ctx);
                    instance_frames += k;
                    for j in 0..k {
                        outputs[0][at + j] += vl[j];
                        outputs[1][at + j] += vr[j];
                        block_peak = block_peak.max(vl[j].abs()).max(vr[j].abs());
                    }
                }
                if block_peak > 1e-4 {
                    last_loud = at + k;
                }
                if all_stopped && stopped_at.is_none() {
                    stopped_at = Some(at);
                }
                at += k;
            }
            ops = voices.iter().map(Instance::ops).sum();
            let end = stopped_at.unwrap_or(last_loud).max(release_at);
            if end < n - 128 {
                tail_secs = Some((end - release_at) as f32 / AUDITION_RATE);
            }
        }
        Kind::Effect => {
            let mut inst = Instance::new(shader.clone(), AUDITION_RATE);
            set_audition_time(&mut inst, 0);
            let [il, ir] = input;
            let (left, right) = outputs.split_at_mut(1);
            let (l, r) = (&mut left[0], &mut right[0]);
            for at in (0..n).step_by(MAX_FRAMES as usize) {
                let end = (at + MAX_FRAMES as usize).min(n);
                inst.process(&il[at..end], &ir[at..end], &mut l[at..end], &mut r[at..end]);
                observe(&inst.ctx);
            }
            instance_frames = n;
            ops = inst.ops();
        }
        Kind::Control => {
            let mut inst = Instance::new(shader.clone(), AUDITION_RATE);
            set_audition_time(&mut inst, 0);
            let mut tables = ChannelTables { ins: Vec::new(), outs: Vec::with_capacity(channels) };
            for at in (0..n).step_by(MAX_FRAMES as usize) {
                let end = (at + MAX_FRAMES as usize).min(n);
                inst.ctx[CTX_FRAME as usize] = at as u32;
                let mut call = tables.empty();
                call.outs.extend(outputs.iter_mut().map(|ch| &mut ch[at..end]));
                shader.run_ports(&mut inst.ctx, &mut inst.state, &mut inst.scratch, &[], &mut call.outs, end - at);
                tables = call.empty();
                observe(&inst.ctx);
                inst.counted();
            }
            instance_frames = n;
            ops = inst.ops();
        }
    }
    let ops_per_frame = ops as f64 / instance_frames.max(1) as f64;
    Audition { outputs, tail_secs, probes, ops_per_frame }
}

/// Replay the audition for cost alone. Each voice can play its whole phrase
/// independently, using only one control block of output storage.
fn audition_peak_ops(shader: &AudioShaderRef, input: &[Vec<f32>; 2]) -> f64 {
    const BLOCK: usize = CONTROL_BLOCK as usize;
    let word = shader.count_word().expect("check compiles a counting shader") as usize;
    let mut peak = 0;
    let mut run = |inst: &mut Instance, start: usize, end: usize| {
        for at in (start..end).step_by(BLOCK) {
            let n = BLOCK.min(end - at);
            let (mut l, mut r) = ([0.0; BLOCK], [0.0; BLOCK]);
            match shader.kind {
                Kind::Effect => inst.process(&input[0][at..at + n], &input[1][at..at + n], &mut l[..n], &mut r[..n]),
                // A control node reads no input; the stereo arguments give the call its length.
                Kind::Control => inst.process(&[0.0; BLOCK][..n], &[0.0; BLOCK][..n], &mut l[..n], &mut r[..n]),
                Kind::Instrument => {
                    if shader.stopped(&inst.state) {
                        break;
                    }
                    inst.render(&mut l[..n], &mut r[..n]);
                }
            }
            peak = peak.max(makepad_script_compute::work::read_count(&inst.ctx, word));
        }
    };
    match shader.kind {
        Kind::Effect | Kind::Control => {
            let mut inst = Instance::new(shader.clone(), AUDITION_RATE);
            set_audition_time(&mut inst, 0);
            run(&mut inst, 0, AUDITION_FRAMES);
        }
        Kind::Instrument => {
            let notes = [48.0, 52.0, 55.0];
            let release = AUDITION_RATE as usize;
            let start_of = |i| i * release / 4;
            let count = if shader.mono() { 1 } else { notes.len() };
            for (i, &note) in notes[..count].iter().enumerate() {
                let mut at = start_of(i);
                let mut voice = Instance::new(shader.clone(), AUDITION_RATE);
                voice.set_voice(i as u32);
                set_audition_time(&mut voice, at);
                voice.note_on(note, 0.8, i as u32 + 1);
                if shader.mono() {
                    for (j, &pitch) in notes.iter().enumerate().skip(1) {
                        run(&mut voice, at, start_of(j));
                        shader.legato(&mut voice.state, pitch, 0.8);
                        at = start_of(j);
                    }
                }
                run(&mut voice, at, release);
                voice.note_off(0.5);
                run(&mut voice, release, AUDITION_FRAMES);
            }
        }
    }
    peak as f64 / f64::from(CONTROL_BLOCK)
}

/// Compiles `code` and auditions it at 48 kHz with the song playing (120
/// bpm, 4 beats a bar, from beat 0): an instrument plays a short phrase (C3
/// E3 G3, held, then released), an effect processes a test signal (a saw
/// chord with clicks). Off the audio thread.
pub fn check(code: &str) -> Result<CheckReport, Vec<ShaderError>> {
    check_with(code, &AudioBindings::default())
}

/// Auditions the shader with the host's modules and recordings.
pub fn check_with(code: &str, bindings: &AudioBindings<'_>) -> Result<CheckReport, Vec<ShaderError>> {
    let shader = compile_audition(code, true, bindings)?;
    let input = audition_input(shader.kind);
    let Audition { outputs, tail_secs, probes, ops_per_frame } = audition(&shader, &input);
    let peak_ops_per_block = audition_peak_ops(&shader, &input);
    let all = outputs.iter().flatten();
    let nonfinite = all.clone().filter(|x| !x.is_finite()).count();
    let finite: Vec<f32> = all.filter(|x| x.is_finite()).copied().collect();
    let peak = finite.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let rms = (finite.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / finite.len().max(1) as f64).sqrt() as f32;
    let dc = (finite.iter().map(|x| *x as f64).sum::<f64>() / finite.len().max(1) as f64) as f32;
    let clipped = finite.iter().filter(|x| x.abs() >= 1.0).count() as f32 / finite.len().max(1) as f32;
    let mut warnings: Vec<String> = shader.warnings().iter().map(|w| w.message.to_string()).collect();
    if nonfinite > 0 {
        warnings.push(format!("{} NaN/infinite samples: look for a division by zero, log of 0 or an unstable filter", nonfinite));
    }
    if shader.kind != Kind::Control {
        if peak < 1e-4 {
            warnings.push("silent: nothing reaches the output (check the return value and the envelope)".into());
        } else if clipped > 0.001 {
            warnings.push(format!("{:.1}% of samples at or above full scale: lower the gain", clipped * 100.0));
        } else if peak > 1.0 {
            warnings.push(format!("peaks at {:.2}: lower the gain", peak));
        }
        if dc.abs() > 0.05 * rms.max(1e-6) && dc.abs() > 0.01 {
            warnings.push(format!("DC offset {:.3}: add a dc_block() (asymmetric shapers make DC)", dc));
        }
    }
    if shader.kind == Kind::Instrument && tail_secs.is_none() {
        warnings.push("voices never end after release: fade out on `gate == 0.0` (an adsr() does), or call stop()".into());
    }
    if ops_per_frame > EXPENSIVE_OPS_PER_FRAME {
        warnings.push(format!(
            "expensive: {ops_per_frame:.0} ops per frame per voice, over the {EXPENSIVE_OPS_PER_FRAME:.0} line (1/{:.0} of realtime on the slowest backend)",
            MEAN_INSTANCES as f64 / SHADER_REALTIME_SHARE,
        ));
    }
    if peak_ops_per_block > EXPENSIVE_PEAK_OPS_PER_FRAME {
        warnings.push(format!(
            "peak {peak_ops_per_block:.0} ops per frame in one {CONTROL_BLOCK}-frame block (mean {ops_per_frame:.0}), \
             over the {EXPENSIVE_PEAK_OPS_PER_FRAME:.0} line (1/{:.0} of a {SHORTEST_CALLBACK_FRAMES}-frame callback on the slowest backend): \
             spread the work over blocks or precompute it in init()",
            PEAK_INSTANCES as f64 / SHADER_REALTIME_SHARE,
        ));
    }
    Ok(CheckReport {
        kind: shader.kind,
        latency: shader.latency(),
        params: shader.params().to_vec(),
        ports: shader.port_info.clone(),
        probes,
        param_meta: shader.param_metas().to_vec(),
        state_words: shader.state_words(),
        backend: shader.backend(),
        peak,
        rms,
        dc,
        nonfinite,
        clipped,
        ops_per_frame,
        peak_ops_per_block,
        tail_secs,
        warnings,
    })
}

