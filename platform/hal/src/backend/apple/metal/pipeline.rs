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
/// formats and whether the draw blends, built from the retained functions on first use.
pub struct Variant {
    formats: Vec<MTLPixelFormat>,
    blend: bool,
    queued: AtomicBool,
    pipeline: OnceLock<Result<RcObjcId, String>>,
}

/// What the compile completion handlers fill and the backend thread reads without waiting.
pub struct PipeState {
    functions: OnceLock<(RcObjcId, RcObjcId)>,
    pub solid: OnceLock<Result<RcObjcId, String>>,
    pub blend: OnceLock<Result<RcObjcId, String>>,
    variants: Mutex<Vec<Arc<Variant>>>,
    /// bit `i` set when the shader writes colour output `i`
    written: u8,
    /// `blend_op: @Max`: blending pipelines take the per-channel max
    blend_max: bool,
    /// whether the pipeline's own layout blends at all (BGRA8, not a raw-write data pass)
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
}

// SAFETY: Metal objects are usable from any thread; the cells are write-once
unsafe impl Send for PipeState {}
unsafe impl Sync for PipeState {}

/// The backend's record of a pipeline.
pub struct Pipe {
    pub gen: u32,
    pub state: Arc<PipeState>,
    pub desc: PipelineDesc,
    pub bindings: Arc<[u32]>,
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
/// `blend_op: @Max` shader (factors are ignored by the max operation).
unsafe fn set_over_blend(color_attachment: ObjcId, max: bool) {
    let op = if max { MTLBlendOperation::Max } else { MTLBlendOperation::Add };
    let () = msg_send![color_attachment, setBlendingEnabled: YES];
    let () = msg_send![color_attachment, setRgbBlendOperation: op];
    let () = msg_send![color_attachment, setAlphaBlendOperation: op];
    let () = msg_send![color_attachment, setSourceRGBBlendFactor: MTLBlendFactor::One];
    let () = msg_send![color_attachment, setSourceAlphaBlendFactor: MTLBlendFactor::One];
    let () = msg_send![color_attachment, setDestinationRGBBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
    let () = msg_send![color_attachment, setDestinationAlphaBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
}

/// Describes a pipeline's colour attachments for a pass whose attachments have `formats`: each
/// gets its own pixel format and blend state. An attachment the shader does not write gets an
/// empty write mask, so drawing ordinary one-output items into an MRT pass leaves the other
/// attachments intact; a written, blendable one gets the over blend when `blend`; everything
/// else writes raw.
unsafe fn describe_attachments(descriptor: ObjcId, formats: &[MTLPixelFormat], written: u8, blend: bool, max: bool) {
    let color_attachments: ObjcId = msg_send![descriptor, colorAttachments];
    for (index, format) in formats.iter().enumerate() {
        let attachment: ObjcId = msg_send![color_attachments, objectAtIndexedSubscript: index as u64];
        let () = msg_send![attachment, setPixelFormat: *format];
        if index >= 8 || written & (1 << index) == 0 {
            let () = msg_send![attachment, setBlendingEnabled: NO];
            let () = msg_send![attachment, setWriteMask: 0u64];
        } else if blend && blendable(*format) {
            set_over_blend(attachment, max);
        } else {
            let () = msg_send![attachment, setBlendingEnabled: NO];
        }
    }
}

fn describe_error(error: ObjcId, none: &str) -> String {
    if error == nil {
        none.into()
    } else {
        nsstring_to_string(unsafe { msg_send![error, localizedDescription] })
    }
}

impl PipeState {
    fn failure(&self, error: String) {
        let _ = self.blend.set(Err(error.clone()));
        let _ = self.solid.set(Err(error.clone()));
        if !self.failed.swap(true, Ordering::AcqRel) {
            self.mailbox.push(Done::PipelineFailed { sid: self.sid, id: self.id, incarnation: self.incarnation.clone(), log: error });
        }
    }

    fn settle(&self) {
        if self.settled.fetch_add(1, Ordering::AcqRel) == 1 && !self.failed.load(Ordering::Acquire) {
            let ok = matches!(self.solid.get(), Some(Ok(_))) && matches!(self.blend.get(), Some(Ok(_)));
            if ok {
                self.mailbox.push(Done::PipelineReady { sid: self.sid, id: self.id, incarnation: self.incarnation.clone() });
            } else if !self.failed.swap(true, Ordering::AcqRel) {
                let log = [self.solid.get(), self.blend.get()].into_iter().flatten().find_map(|r| r.as_ref().err().cloned()).unwrap_or_default();
                self.mailbox.push(Done::PipelineFailed { sid: self.sid, id: self.id, incarnation: self.incarnation.clone(), log });
            }
        }
    }

    /// The state for a draw into `formats`; never waits. A target layout of its own is built
    /// from the retained functions on first use and the draw is skipped until it is ready.
    pub fn select(self: &Arc<Self>, dev: &Device, formats: &[MTLPixelFormat], blend: bool) -> Select {
        let want_blend = blend && self.can_blend;
        if formats == self.declared.as_slice() {
            return match if want_blend { self.blend.get() } else { self.solid.get() } {
                Some(Ok(p)) => Select::State(p.as_id()),
                Some(Err(_)) => Select::Failed,
                None => Select::Pending,
            };
        }
        if matches!(self.blend.get(), Some(Err(_))) {
            return Select::Failed;
        }
        let variant = self.variant(formats, want_blend);
        if !variant.queued.load(Ordering::Relaxed) && self.enqueue_variant(dev, &variant) {
            variant.queued.store(true, Ordering::Relaxed);
        }
        match variant.pipeline.get() {
            Some(Ok(p)) => Select::State(p.as_id()),
            Some(Err(_)) => Select::Failed,
            None => Select::Pending,
        }
    }

    /// Whether the state for a draw into `formats` is ready; queues it when it is not (never
    /// waits). False while it compiles, or when it failed.
    pub fn ready_for(self: &Arc<Self>, dev: &Device, formats: &[MTLPixelFormat], blend: bool) -> bool {
        matches!(self.select(dev, formats, blend), Select::State(_))
    }

    fn variant(&self, formats: &[MTLPixelFormat], blend: bool) -> Arc<Variant> {
        let mut variants = self.variants.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = variants.iter().find(|v| v.blend == blend && v.formats == formats) {
            return v.clone();
        }
        let v = Arc::new(Variant { formats: formats.to_vec(), blend, queued: AtomicBool::new(false), pipeline: OnceLock::new() });
        variants.push(v.clone());
        v
    }

    /// Queues a variant's state. False while the library is still compiling (the caller retries
    /// on a later draw).
    fn enqueue_variant(self: &Arc<Self>, dev: &Device, variant: &Arc<Variant>) -> bool {
        if self.functions.get().is_none() {
            return false;
        }
        let state = self.clone();
        let variant = variant.clone();
        let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
        dev.compile(Box::new(move || state.compile_variant(device.as_id(), variant)));
        true
    }

    /// Builds one variant's state from the retained functions.
    fn compile_variant(self: Arc<Self>, device: ObjcId, variant: Arc<Variant>) {
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
            describe_attachments(descriptor.as_id(), &variant.formats, self.written, variant.blend, self.blend_max);
            let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float];
        }
        let state = self.clone();
        let completion = objc_block!(move |pipeline: ObjcId, error: ObjcId| {
            let result = match NonNull::new(pipeline) {
                Some(p) => Ok(RcObjcId::from_unowned(p)),
                None => Err(describe_error(error, "Metal returned no pipeline")),
            };
            if let Err(e) = &result {
                eprintln!("[metal] pipeline variant {:?}: {e}", variant.formats);
            }
            let _ = variant.pipeline.set(result);
            // a draw into this layout was skipped: the app paints again
            state.mailbox.push(Done::PipelineReady { sid: state.sid, id: state.id, incarnation: state.incarnation.clone() });
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
/// backend reads; `PipelineReady` / `PipelineFailed` arrive through the mailbox.
pub(super) fn create(dev: &Device, sid: u32, id: Id, incarnation: Arc<crate::font::FrameProgress>, source: &str, desc: &PipelineDesc) -> Arc<PipeState> {
    let declared = mtl_format(desc.color_formats[0]).unwrap_or(MTLPixelFormat::BGRA8Unorm);
    let raw = desc.blend & pipeline_blend::RAW != 0;
    let state = Arc::new(PipeState {
        functions: OnceLock::new(),
        solid: OnceLock::new(),
        blend: OnceLock::new(),
        variants: Mutex::new(Vec::new()),
        written: desc.outputs as u8,
        blend_max: desc.blend & pipeline_blend::MAX != 0,
        can_blend: declared == MTLPixelFormat::BGRA8Unorm && !raw,
        discards: source.contains("discard_fragment"),
        declared: desc.color_formats.iter().copied().take_while(|f| *f != 0).filter_map(mtl_format).collect(),
        settled: AtomicU8::new(0),
        failed: AtomicBool::new(false),
        sid,
        id,
        incarnation,
        mailbox: dev.mailbox.clone(),
    });
    let source = source.to_owned();
    let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
    let job = state.clone();
    dev.compile(Box::new(move || compile(device, source, job)));
    state
}

/// The options every pipeline's MSL is compiled with.
fn compile_options() -> RcObjcId {
    let options = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![class!(MTLCompileOptions), new] }).expect("options"));
    unsafe {
        let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
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
    let options = compile_options();
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

fn compile(device: RcObjcId, source: String, ready: Arc<PipeState>) {
    let options = compile_options();
    let callback_device = device.clone();
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
            // kept for the variants built on first use: the passes of other attachment layouts
            // (an HDR target, MRT)
            let _ = ready.functions.set((vertex.clone(), fragment.clone()));
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
                        if blend && ready.can_blend && blendable(*format) && ready.written & (1 << index) != 0 {
                            set_over_blend(attachment, ready.blend_max);
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
                        None => Err(describe_error(error, "Metal returned no pipeline")),
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
