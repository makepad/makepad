//! Pipelines: the MSL of a `PipelineCreate` compiled off the backend thread, and the pipeline
//! states a draw needs per target layout. Moved from os/apple/metal.rs (`MetalPipelines`,
//! `MetalMrtPipeline`, `describe_mrt_attachments`); the shader compiler (Cx side) made the MSL,
//! nothing is composed here.

use super::device::{Device, Done, Mailbox};
use super::resources::mtl_format;
use crate::types::*;
use makepad_apple_sys::*;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// One pipeline state for a target layout other than the pipeline's own: the pass's attachment
/// formats, blend mode and sample count, built from the retained functions on a want or draw.
pub struct Variant {
    formats: Vec<MTLPixelFormat>,
    blend: bool,
    /// samples per pixel of the pass it draws in (an MSAA pass's, `Pass::samples`; else 1)
    samples: u8,
    queued: AtomicBool,
    pipeline: OnceLock<Result<RcObjcId, String>>,
}

struct WantedLayout {
    layout: u32,
    /// The single-sample draws select the pipeline's declared solid and blend states.
    base: bool,
    variants: Vec<Arc<Variant>>,
    answered: bool,
}

/// What the compile completion handlers fill and the backend thread reads without waiting.
pub struct PipeState {
    functions: OnceLock<(RcObjcId, RcObjcId)>,
    pub solid: OnceLock<Result<RcObjcId, String>>,
    pub blend: OnceLock<Result<RcObjcId, String>>,
    variants: Mutex<Vec<Arc<Variant>>>,
    wanted: Mutex<Vec<WantedLayout>>,
    /// bit `i` set when the shader writes colour output `i`
    written: u8,
    /// `blend_op: @Max`: blending pipelines take the per-channel max
    blend_max: bool,
    /// `blend_op: @Add`: ONE/ONE/ADD, including float attachments.
    blend_add: bool,
    name: String,
    /// whether the declared layout opts into blending (BGRA8 or Add, never RAW)
    can_blend: bool,
    /// the fragment stage can discard: a layout without colour attachments still runs it
    discards: bool,
    declared: Vec<MTLPixelFormat>,
    /// the two base states that finished (ready is announced when both did)
    settled: AtomicU8,
    failed: AtomicBool,
    sid: u32,
    id: Id,
    incarnation: Arc<crate::font::FrameProgress>,
    mailbox: Mailbox,
    /// the MSL it was compiled from (what a later session's pipeline must equal to take its
    /// states, [`Compiled`])
    pub source: Arc<str>,
}

// SAFETY: Metal objects are usable from any thread; the cells are write-once
unsafe impl Send for PipeState {}
unsafe impl Sync for PipeState {}

/// The compiled states of a pipeline whose session closed, for the next session: a restart in
/// place creates the same pipelines again, and one that compiles to these ([`Compiled::is`]: its
/// source equal in full) takes them instead of compiling. Compiling them again is not only slower: with every
/// restart that rebuilt them the driver's own memory grew (on Apple GPUs a few hundred KB a
/// restart of a big app, and none while they are taken).
pub struct Compiled {
    source: Arc<str>,
    desc: PipelineDesc,
    functions: (RcObjcId, RcObjcId),
    solid: RcObjcId,
    blend: RcObjcId,
    variants: Vec<(Vec<MTLPixelFormat>, bool, u8, RcObjcId)>,
}

impl Compiled {
    /// Whether a pipeline of `source` and `desc` compiles to these states: the same source in
    /// full, and the same of what a state is built from besides (its attachments, outputs, blend,
    /// math). The rest of a description names the pipeline, or is read at a draw.
    pub fn is(&self, source: &str, desc: &PipelineDesc) -> bool {
        let d = &self.desc;
        d.color_formats == desc.color_formats && d.outputs == desc.outputs && d.blend == desc.blend && d.flags == desc.flags && *self.source == *source
    }
}

/// The backend's record of a pipeline.
pub struct Pipe {
    pub gen: u32,
    pub state: Arc<PipeState>,
    pub desc: PipelineDesc,
    pub bindings: Arc<[u32]>,
    pub storage: Vec<StorageBinding>,
    /// the session was told the first outcome (variants announce readiness again)
    pub announced: bool,
    /// the GPU profiler's cost hints of the generated source ([`crate::gpu_prof::source_stats`])
    pub stats: Arc<str>,
    /// the generated source, kept for `gpu prof shader` when the profiler keeps sources
    pub source: Option<Arc<str>>,
}

/// Attachments the premultiplied-over blend applies to: four-channel formats every Metal GPU
/// family blends, whose alpha the shader writes. One- and two-channel targets (normals, velocity,
/// masks) have no alpha to blend by, and 32-bit float and integer targets are data: they all
/// write raw (a float2 output into a blended RG16Float attachment comes out as zeros on Apple GPUs).
fn blendable(format: MTLPixelFormat) -> bool {
    matches!(format, MTLPixelFormat::BGRA8Unorm | MTLPixelFormat::RGBA8Unorm | MTLPixelFormat::RGBA16Float)
}

/// The blend every blendable colour target uses: premultiplied over, or per-channel max for a
/// `blend_op: @Max` shader, or ONE/ONE/ADD for `blend_op: @Add`.
unsafe fn set_blend(color_attachment: ObjcId, max: bool, add: bool) {
    let op = if max { MTLBlendOperation::Max } else { MTLBlendOperation::Add };
    let dest = if add { MTLBlendFactor::One } else { MTLBlendFactor::OneMinusSourceAlpha };
    let () = msg_send![color_attachment, setBlendingEnabled: YES];
    let () = msg_send![color_attachment, setRgbBlendOperation: op];
    let () = msg_send![color_attachment, setAlphaBlendOperation: op];
    let () = msg_send![color_attachment, setSourceRGBBlendFactor: MTLBlendFactor::One];
    let () = msg_send![color_attachment, setSourceAlphaBlendFactor: MTLBlendFactor::One];
    let () = msg_send![color_attachment, setDestinationRGBBlendFactor: dest];
    let () = msg_send![color_attachment, setDestinationAlphaBlendFactor: dest];
}

/// Describes a pipeline's colour attachments for a pass whose attachments have `formats`: each
/// gets its own pixel format and blend state. An attachment the shader does not write gets an
/// empty write mask, so drawing ordinary one-output items into an MRT pass leaves the other
/// attachments intact. Written targets use the selected blend when `blend`; Over/Max keep
/// their format eligibility, while Add asks Metal to validate blending for every written target.
unsafe fn describe_attachments(descriptor: ObjcId, formats: &[MTLPixelFormat], written: u8, blend: bool, max: bool, add: bool) {
    let color_attachments: ObjcId = msg_send![descriptor, colorAttachments];
    for (index, format) in formats.iter().enumerate() {
        let attachment: ObjcId = msg_send![color_attachments, objectAtIndexedSubscript: index as u64];
        let () = msg_send![attachment, setPixelFormat: *format];
        if index >= 8 || written & (1 << index) == 0 {
            let () = msg_send![attachment, setBlendingEnabled: NO];
            let () = msg_send![attachment, setWriteMask: 0u64];
        } else if blend && (add || blendable(*format)) {
            // Metal validates Add support when it builds the pipeline state.
            set_blend(attachment, max, add);
        } else {
            let () = msg_send![attachment, setBlendingEnabled: NO];
        }
    }
}

pub(super) fn describe_error(error: ObjcId, none: &str) -> String {
    if error == nil {
        none.into()
    } else {
        nsstring_to_string(unsafe { msg_send![error, localizedDescription] })
    }
}

impl PipeState {
    /// What a later session's identical pipeline takes instead of compiling: the states this one
    /// finished, its variants among them. None while it compiles, or when it failed.
    pub fn compiled(&self, desc: &PipelineDesc) -> Option<Compiled> {
        let (Some(functions), Some(Ok(solid)), Some(Ok(blend))) = (self.functions.get(), self.solid.get(), self.blend.get()) else { return None };
        let variants = self.variants.lock().unwrap_or_else(|e| e.into_inner()).iter()
            .filter_map(|v| match v.pipeline.get() { Some(Ok(p)) => Some((v.formats.clone(), v.blend, v.samples, p.clone())), _ => None })
            .collect();
        Some(Compiled { source: self.source.clone(), desc: *desc, functions: functions.clone(), solid: solid.clone(), blend: blend.clone(), variants })
    }

    fn failure(&self, error: String) {
        let error = format!("shader `{}`: {error}", self.name);
        let _ = self.blend.set(Err(error.clone()));
        let _ = self.solid.set(Err(error.clone()));
        if !self.failed.swap(true, Ordering::AcqRel) {
            self.mailbox.push(Done::PipelineFailed { sid: self.sid, id: self.id, incarnation: self.incarnation.clone(), log: error });
        }
    }

    fn settle(&self) {
        if self.settled.fetch_add(1, Ordering::AcqRel) == 1 && !self.failed.load(Ordering::Acquire) {
            if self.base_ready() {
                self.mailbox.push(Done::PipelineReady { sid: self.sid, id: self.id, incarnation: self.incarnation.clone() });
                self.answer_layouts();
            } else {
                let log = [self.solid.get(), self.blend.get()].into_iter().flatten().find_map(|r| r.as_ref().err().cloned()).unwrap_or_default();
                self.failure(log);
            }
        }
    }

    fn base_ready(&self) -> bool {
        matches!(self.solid.get(), Some(Ok(_))) && matches!(self.blend.get(), Some(Ok(_)))
    }

    fn uses_base(&self, formats: &[MTLPixelFormat], samples: u8) -> bool {
        samples <= 1 && formats == self.declared.as_slice()
    }

    pub(super) fn want_layout(self: &Arc<Self>, dev: &Device, layout: u32, formats: &[MTLPixelFormat], samples: u8) {
        if self.failed.load(Ordering::Acquire) { return; }
        {
            let mut wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
            if wanted.iter().any(|w| w.layout == layout) { return; }
            let base = self.uses_base(formats, 1);
            let mut variants = Vec::new();
            let mut add_variants = |n| {
                variants.push(self.variant(formats, false, n));
                if self.can_blend { variants.push(self.variant(formats, true, n)); }
            };
            if !base { add_variants(1); }
            if samples > 1 { add_variants(samples); }
            wanted.push(WantedLayout { layout, base, variants, answered: false });
        }
        // Publishing the want before checking the functions pairs with the library callback
        // publishing the functions before checking the wants. Either thread can claim the job.
        let jobs = self.claim_wanted();
        if !jobs.is_empty() {
            let state = self.clone();
            let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
            // The same worker as a first draw's state (`select`): the build call is
            // asynchronous, so even its inline run on a full queue does not wait.
            dev.compile(Box::new(move || {
                for v in jobs { state.clone().compile_variant(device.as_id(), v, true); }
            }));
        }
        self.answer_layouts();
    }

    fn claim_variant(&self, v: &Variant) -> bool {
        !v.queued.load(Ordering::Relaxed) && self.functions.get().is_some() && !v.queued.swap(true, Ordering::AcqRel)
    }

    /// Take shared references under the lock. Metal can call a completion as soon as a
    /// build is submitted, so every driver call runs after releasing it.
    fn claim_wanted(&self) -> Vec<Arc<Variant>> {
        self.wanted.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|w| !w.answered)
            .flat_map(|w| &w.variants).filter(|v| self.claim_variant(v)).cloned().collect()
    }

    fn answer_layouts(&self) {
        if self.failed.load(Ordering::Acquire) { return; }
        let base_ready = self.base_ready();
        let mut answers = Vec::new();
        {
            let mut wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
            for w in wanted.iter_mut() {
                if !w.answered && (!w.base || base_ready) && w.variants.iter().all(|v| matches!(v.pipeline.get(), Some(Ok(_)))) {
                    w.answered = true;
                    answers.push(w.layout);
                }
            }
        }
        for layout in answers {
            self.mailbox.push(Done::PipelineLayoutReady { sid: self.sid, id: self.id, layout, incarnation: self.incarnation.clone() });
        }
    }

    /// The state for a draw into `formats` with `samples` per pixel (an MSAA pass's; else 1);
    /// never waits. The first draw of an unprobed state queues it and skips until it is ready.
    pub fn select(self: &Arc<Self>, dev: &Device, formats: &[MTLPixelFormat], blend: bool, samples: u8) -> Select {
        if self.failed.load(Ordering::Acquire) { return Select::Failed; }
        let want_blend = blend && self.can_blend;
        if self.uses_base(formats, samples) {
            return match if want_blend { self.blend.get() } else { self.solid.get() } {
                Some(Ok(p)) => Select::State(p.as_id()),
                Some(Err(_)) => Select::Failed,
                None => Select::Pending,
            };
        }
        if matches!(self.blend.get(), Some(Err(_))) {
            return Select::Failed;
        }
        let variant = self.variant(formats, want_blend, samples.max(1));
        if self.claim_variant(&variant) {
            let state = self.clone();
            let job = variant.clone();
            let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
            dev.compile(Box::new(move || state.compile_variant(device.as_id(), job, false)));
        }
        match variant.pipeline.get() {
            Some(Ok(p)) => Select::State(p.as_id()),
            Some(Err(e)) => {
                // A probed Add state can fail while the app still uses its fallback.
                // Selecting that failed state reports the same pipeline failure as a lazy build.
                if self.blend_add { self.failure(e.clone()); }
                Select::Failed
            }
            None => Select::Pending,
        }
    }

    /// Whether the state for a draw into `formats` with `samples` per pixel is ready; queues it
    /// when it is not (never waits). False while it compiles, or when it failed.
    pub fn ready_for(self: &Arc<Self>, dev: &Device, formats: &[MTLPixelFormat], blend: bool, samples: u8) -> bool {
        matches!(self.select(dev, formats, blend, samples), Select::State(_))
    }

    fn variant(&self, formats: &[MTLPixelFormat], blend: bool, samples: u8) -> Arc<Variant> {
        let mut variants = self.variants.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = variants.iter().find(|v| v.blend == blend && v.samples == samples && v.formats == formats) {
            return v.clone();
        }
        let v = Arc::new(Variant { formats: formats.to_vec(), blend, samples, queued: AtomicBool::new(false), pipeline: OnceLock::new() });
        variants.push(v.clone());
        v
    }

    /// Builds one variant's state from the retained functions.
    fn compile_variant(self: Arc<Self>, device: ObjcId, variant: Arc<Variant>, probed: bool) {
        let Some((vertex, fragment)) = self.functions.get() else { return };
        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] }).expect("descriptor"));
        unsafe {
            let _: () = msg_send![descriptor.as_id(), setVertexFunction: vertex.as_id()];
            // Depth only (a scratch-colour pass drawn without its colour, see `depth_only`):
            // the fragment stage writes nothing anyone reads, so a shader that cannot discard
            // has none; the GPU then only rasterizes depth.
            if !variant.formats.is_empty() || self.discards {
                let _: () = msg_send![descriptor.as_id(), setFragmentFunction: fragment.as_id()];
            }
            describe_attachments(descriptor.as_id(), &variant.formats, self.written, variant.blend, self.blend_max, self.blend_add);
            let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float];
            if variant.samples > 1 {
                let () = msg_send![descriptor.as_id(), setRasterSampleCount: variant.samples as u64];
            }
        }
        let state = self.clone();
        let completion = objc_block!(move |pipeline: ObjcId, error: ObjcId| {
            let result = match NonNull::new(pipeline) {
                Some(p) => Ok(RcObjcId::from_unowned(p)),
                None => Err(if state.blend_add && variant.blend {
                    format!("blend_op: @Add requires Metal color attachment blending for {:?}: {}", variant.formats, describe_error(error, "Metal returned no pipeline"))
                } else { describe_error(error, "Metal returned no pipeline") }),
            };
            if let Err(e) = &result {
                if state.blend_add && !probed {
                    state.failure(e.clone());
                    let _ = variant.pipeline.set(result);
                    return;
                }
                eprintln!("[metal] pipeline variant {:?} at {} samples: {e}", variant.formats, variant.samples);
            }
            let _ = variant.pipeline.set(result);
            // A lazy draw still gets its repaint. An early speculative build must not
            // announce a base that is still compiling. Its settle will repaint in that case.
            if !probed || (state.base_ready() && !state.failed.load(Ordering::Acquire)) {
                state.mailbox.push(Done::PipelineReady { sid: state.sid, id: state.id, incarnation: state.incarnation.clone() });
            }
            state.answer_layouts();
        });
        unsafe {
            let _: () = msg_send![device, newRenderPipelineStateWithDescriptor: descriptor.as_id() completionHandler: &completion];
        }
    }
}

pub enum Select {
    State(ObjcId),
    Pending,
    Failed,
}

/// Starts the compile of a pipeline. Returns the state the completion handlers fill and the
/// backend reads; `PipelineReady` / `PipelineFailed` arrive through the mailbox. With `taken`
/// (the states of an identical pipeline, [`Compiled::is`]) nothing compiles: the state is ready
/// as it returns and no message follows.
pub(super) fn create(dev: &Device, sid: u32, id: Id, incarnation: Arc<crate::font::FrameProgress>, source: &str, desc: &PipelineDesc, name: &str, taken: Option<Compiled>) -> Arc<PipeState> {
    let declared = mtl_format(desc.color_formats[0]).unwrap_or(MTLPixelFormat::BGRA8Unorm);
    let raw = desc.blend & pipeline_blend::RAW != 0;
    let state = Arc::new(PipeState {
        functions: OnceLock::new(),
        solid: OnceLock::new(),
        blend: OnceLock::new(),
        variants: Mutex::new(Vec::new()),
        wanted: Mutex::new(Vec::new()),
        written: desc.outputs as u8,
        blend_max: desc.blend & pipeline_blend::MAX != 0,
        blend_add: desc.blend & pipeline_blend::ADD != 0,
        name: name.to_owned(),
        can_blend: (declared == MTLPixelFormat::BGRA8Unorm || desc.blend & pipeline_blend::ADD != 0) && !raw,
        discards: source.contains("discard_fragment"),
        // RGBA8 is format zero, so the output mask determines the attachment span.
        declared: desc.color_formats.iter().copied().take((u32::BITS - desc.outputs.leading_zeros()) as usize).filter_map(mtl_format).collect(),
        settled: AtomicU8::new(0),
        failed: AtomicBool::new(false),
        sid,
        id,
        incarnation,
        mailbox: dev.mailbox.clone(),
        source: taken.as_ref().map_or_else(|| source.into(), |c| c.source.clone()),
    });
    if let Some(c) = taken {
        let _ = state.functions.set(c.functions);
        let _ = state.solid.set(Ok(c.solid));
        let _ = state.blend.set(Ok(c.blend));
        state.settled.store(2, Ordering::Release);
        let mut variants = state.variants.lock().unwrap_or_else(|e| e.into_inner());
        for (formats, blend, samples, p) in c.variants {
            variants.push(Arc::new(Variant { formats, blend, samples, queued: AtomicBool::new(true), pipeline: OnceLock::from(Ok(p)) }));
        }
        // The new session can only want a layout after create returns. It will find these
        // completed states and can queue missing variants immediately from the restored functions.
        drop(variants);
        return state;
    }
    let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
    let job = state.clone();
    let precise = desc.flags & pipeline_flags::PRECISE != 0;
    dev.compile(Box::new(move || compile(device, job, precise)));
    state
}

/// The options a pipeline's MSL is compiled with: fast math, or for a precise-math pipeline
/// (`pipeline_flags::PRECISE`) none (IEEE division and sqrt; the source also turns fp
/// contraction off), with the safe math mode and precise fp32 functions where the OS has them.
pub(super) fn compile_options(precise: bool) -> RcObjcId {
    let options = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![class!(MTLCompileOptions), new] }).expect("options"));
    unsafe {
        let _: () = msg_send![options.as_id(), setFastMathEnabled: if precise { NO } else { YES }];
        if precise {
            // MTLMathModeSafe = 0, MTLMathFloatingPointFunctionsPrecise = 1 (macOS 15 / iOS 18)
            let mode: BOOL = msg_send![options.as_id(), respondsToSelector: sel!(setMathMode:)];
            if mode == YES {
                let _: () = msg_send![options.as_id(), setMathMode: 0i64];
            }
            let functions: BOOL = msg_send![options.as_id(), respondsToSelector: sel!(setMathFloatingPointFunctions:)];
            if functions == YES {
                let _: () = msg_send![options.as_id(), setMathFloatingPointFunctions: 1i64];
            }
        }
    }
    options
}

/// Compiles the MSL of a pipeline as the backend does (the system's default Metal device,
/// the same options, `vertex_main` and `fragment_main` looked up), synchronously and with no
/// window: what a shader test checks the generated MSL with, so it needs no offline Metal
/// toolchain. Err when the compile fails, an entry point is missing, or there is no Metal
/// device at all.
pub fn compile_msl(source: &str) -> Result<(), String> {
    let device = unsafe { MTLCreateSystemDefaultDevice() };
    let Some(device) = NonNull::new(device).map(RcObjcId::from_owned) else {
        return Err("no Metal device".into());
    };
    let options = compile_options(source.contains(PRECISE_PRAGMA));
    let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
    let mut error: ObjcId = nil;
    let library: ObjcId = unsafe { msg_send![device.as_id(), newLibraryWithSource: str_to_nsstring(source) options: options.as_id() error: &mut error] };
    let result = match NonNull::new(library).map(RcObjcId::from_owned) {
        None => Err(describe_error(error, "Metal returned no library")),
        Some(library) => {
            let has = |name: &str| NonNull::new(unsafe { msg_send![library.as_id(), newFunctionWithName: str_to_nsstring(name)] }).map(RcObjcId::from_owned).is_some();
            match (has("vertex_main"), has("fragment_main")) {
                (true, true) => Ok(()),
                _ => Err("Metal library is missing vertex_main or fragment_main".into()),
            }
        }
    };
    unsafe {
        let _: () = msg_send![pool, release];
    }
    result
}

/// The line a precise-math MSL source starts with (`shader_ir_metal`).
const PRECISE_PRAGMA: &str = "#pragma clang fp contract(off)";

fn compile(device: RcObjcId, ready: Arc<PipeState>, precise: bool) {
    let options = compile_options(precise);
    let callback_device = device.clone();
    let source = ready.source.clone();
    let callback_source = source.clone();
    let completion = objc_block!(move |library: ObjcId, error: ObjcId| {
        if library == nil {
            let description = describe_error(error, "Metal returned no library");
            ready.failure(format!("{description}\n{callback_source}"));
            return;
        }
        let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
        let function = |name: &str| NonNull::new(unsafe { msg_send![library, newFunctionWithName: str_to_nsstring(name)] }).map(RcObjcId::from_owned);
        if let (Some(vertex), Some(fragment)) = (function("vertex_main"), function("fragment_main")) {
            // Both layout wants and first draws into other targets use these functions.
            let _ = ready.functions.set((vertex.clone(), fragment.clone()));
            for variant in ready.claim_wanted() {
                ready.clone().compile_variant(callback_device.as_id(), variant, true);
            }
            // independent descriptors: a driver may consume them after this call returns
            for blend in [true, false] {
                let descriptor = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] }).expect("descriptor"));
                unsafe {
                    let _: () = msg_send![descriptor.as_id(), setVertexFunction: vertex.as_id()];
                    let _: () = msg_send![descriptor.as_id(), setFragmentFunction: fragment.as_id()];
                    let attachments: ObjcId = msg_send![descriptor.as_id(), colorAttachments];
                    for (index, format) in ready.declared.iter().enumerate() {
                        let attachment: ObjcId = msg_send![attachments, objectAtIndexedSubscript: index as u64];
                        let () = msg_send![attachment, setPixelFormat: *format];
                        if blend && ready.can_blend && (ready.blend_add || blendable(*format)) && ready.written & (1 << index) != 0 {
                            set_blend(attachment, ready.blend_max, ready.blend_add);
                        } else {
                            let () = msg_send![attachment, setBlendingEnabled: NO];
                        }
                    }
                    let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float];
                }
                let ready = ready.clone();
                let completion = objc_block!(move |pipeline: ObjcId, error: ObjcId| {
                    let result = match NonNull::new(pipeline) {
                        Some(p) => Ok(RcObjcId::from_unowned(p)),
                        None => Err(if ready.blend_add && blend {
                            format!("blend_op: @Add requires Metal color attachment blending for {:?}: {}", ready.declared, describe_error(error, "Metal returned no pipeline"))
                        } else { describe_error(error, "Metal returned no pipeline") }),
                    };
                    let _ = if blend { ready.blend.set(result) } else { ready.solid.set(result) };
                    ready.settle();
                });
                unsafe {
                    let _: () = msg_send![callback_device.as_id(), newRenderPipelineStateWithDescriptor: descriptor.as_id() completionHandler: &completion];
                }
            }
        } else {
            ready.failure("Metal library is missing vertex_main or fragment_main".into());
        }
        unsafe {
            let _: () = msg_send![pool, release];
        }
    });
    unsafe {
        let _: () = msg_send![device.as_id(), newLibraryWithSource: str_to_nsstring(&source) options: options.as_id() completionHandler: &completion];
    }
}
