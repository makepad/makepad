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
    depth: MTLPixelFormat,
    blend: bool,
    /// samples per pixel of the pass it draws in (an MSAA pass's, `Pass::samples`; else 1)
    samples: u8,
    queued: AtomicBool,
    pipeline: OnceLock<Result<RcObjcId, String>>,
}

impl Variant {
    fn matches(&self, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> bool {
        self.depth == depth && self.blend == blend && self.samples == samples && self.formats == formats
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VariantBuild {
    Draw,
    LayoutProbe,
    PassUpgrade,
}

struct WantedLayout {
    layout: u32,
    /// Single-sample draws in this layout use the pipeline's base states.
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
    variants: Vec<(Vec<MTLPixelFormat>, MTLPixelFormat, bool, u8, RcObjcId)>,
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

/// Formats blendable on at least one Metal GPU. The driver decides support for 32-bit float.
fn supports_add_blend(format: MTLPixelFormat) -> bool {
    matches!(format,
        MTLPixelFormat::BGRA8Unorm | MTLPixelFormat::BGRA8Unorm_sRGB | MTLPixelFormat::RGBA8Unorm
        | MTLPixelFormat::R8Unorm | MTLPixelFormat::RG8Unorm | MTLPixelFormat::RGBA16Float
        | MTLPixelFormat::RG16Float | MTLPixelFormat::R16Float | MTLPixelFormat::R32Float | MTLPixelFormat::RGBA32Float)
}

fn add_blend_error(formats: &[MTLPixelFormat], written: u8) -> Option<String> {
    let bad = formats.iter().enumerate().any(|(i, f)| i < 8 && written & (1 << i) != 0 && !supports_add_blend(*f));
    bad.then(|| format!("blend_op: @Add requires Metal color attachment blending for {formats:?}"))
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
/// their format eligibility, while Add blends every written target that passed its format check.
unsafe fn describe_attachments(descriptor: ObjcId, formats: &[MTLPixelFormat], written: u8, blend: bool, max: bool, add: bool) {
    let color_attachments: ObjcId = msg_send![descriptor, colorAttachments];
    for (index, format) in formats.iter().enumerate() {
        let attachment: ObjcId = msg_send![color_attachments, objectAtIndexedSubscript: index as u64];
        let () = msg_send![attachment, setPixelFormat: *format];
        if index >= 8 || written & (1 << index) == 0 {
            let () = msg_send![attachment, setBlendingEnabled: NO];
            let () = msg_send![attachment, setWriteMask: 0u64];
        } else if blend && (add || blendable(*format)) {
            set_blend(attachment, max, add);
        } else {
            let () = msg_send![attachment, setBlendingEnabled: NO];
        }
    }
}

/// Describes the attachments and shader stages for a base state or a pass-specific variant.
fn build_descriptor(
    vertex: ObjcId, fragment: ObjcId, formats: &[MTLPixelFormat], depth: MTLPixelFormat,
    written: u8, blend: bool, blend_max: bool, blend_add: bool, discards: bool, samples: u8,
) -> RcObjcId {
    let descriptor = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] }).expect("descriptor"));
    unsafe {
        let _: () = msg_send![descriptor.as_id(), setVertexFunction: vertex];
        // A depth-only pass can skip the fragment stage when the shader cannot discard.
        if !formats.is_empty() || discards {
            let _: () = msg_send![descriptor.as_id(), setFragmentFunction: fragment];
        }
        describe_attachments(descriptor.as_id(), formats, written, blend, blend_max, blend_add);
        let _: () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: depth];
        if samples > 1 {
            let _: () = msg_send![descriptor.as_id(), setRasterSampleCount: samples as u64];
        }
    }
    descriptor
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
            .filter_map(|v| match v.pipeline.get() { Some(Ok(p)) => Some((v.formats.clone(), v.depth, v.blend, v.samples, p.clone())), _ => None })
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
                self.answer_layouts(None);
            } else {
                let log = [self.solid.get(), self.blend.get()].into_iter().flatten().find_map(|r| r.as_ref().err().cloned()).unwrap_or_default();
                self.failure(log);
            }
        }
    }

    pub fn can_blend(&self) -> bool { self.can_blend }

    pub fn base_ready(&self) -> bool {
        matches!(self.solid.get(), Some(Ok(_))) && matches!(self.blend.get(), Some(Ok(_)))
    }

    fn base_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire) || matches!(self.solid.get(), Some(Err(_))) || matches!(self.blend.get(), Some(Err(_)))
    }

    fn uses_base(&self, formats: &[MTLPixelFormat], depth: MTLPixelFormat, samples: u8) -> bool {
        samples <= 1 && depth == MTLPixelFormat::Depth32Float && formats == self.declared.as_slice()
    }

    pub(super) fn want_layout(self: &Arc<Self>, dev: &Device, layout: u32, formats: &[MTLPixelFormat], depth: MTLPixelFormat, samples: u8) {
        if self.failed.load(Ordering::Acquire) { return; }
        {
            let mut wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
            if wanted.iter().any(|w| w.layout == layout) { return; }
            let base = self.uses_base(formats, depth, 1);
            let mut variants = Vec::new();
            let mut add_variants = |n| {
                variants.push(self.variant(formats, depth, false, n));
                if self.can_blend { variants.push(self.variant(formats, depth, true, n)); }
            };
            if !base { add_variants(1); }
            if samples > 1 { add_variants(samples); }
            wanted.push(WantedLayout { layout, base, variants, answered: false });
        }
        // Publishing the want before checking the functions pairs with the library callback
        // publishing the functions before checking the wants. Either thread can claim the job.
        // A cached failure answers the whole layout before any sibling variant is claimed.
        self.answer_layouts(None);
        let jobs = self.claim_wanted();
        if !jobs.is_empty() {
            let state = self.clone();
            let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
            dev.compile(Box::new(move || {
                for v in jobs { state.clone().compile_variant(device.as_id(), v, VariantBuild::LayoutProbe); }
            }));
        }
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

    fn answer_layouts(&self, failed_variant: Option<&Variant>) -> bool {
        if self.failed.load(Ordering::Acquire) { return false; }
        let base_ready = self.base_ready();
        let mut matched = false;
        let mut answers = Vec::new();
        {
            let mut wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
            for w in wanted.iter_mut() {
                // Another completion may already have answered this layout since the failure
                // was published. A layout containing a settled Err can only answer failure.
                matched |= failed_variant.is_some_and(|p| w.variants.iter().any(|v| std::ptr::eq(Arc::as_ptr(v), p)));
                if w.answered { continue; }
                let failure = w.variants.iter().find_map(|v| v.pipeline.get()?.as_ref().err());
                let answer = if let Some(log) = failure {
                    Done::PipelineLayoutFailed { sid: self.sid, id: self.id, layout: w.layout, incarnation: self.incarnation.clone(), log: log.clone() }
                } else if (!w.base || base_ready) && w.variants.iter().all(|v| matches!(v.pipeline.get(), Some(Ok(_)))) {
                    Done::PipelineLayoutReady { sid: self.sid, id: self.id, layout: w.layout, incarnation: self.incarnation.clone() }
                } else {
                    continue;
                };
                w.answered = true;
                answers.push(answer);
            }
        }
        for answer in answers { self.mailbox.push(answer); }
        matched
    }

    /// Looks up a ready state without queuing a compile or creating a variant.
    pub fn has_ready(&self, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> bool {
        if self.base_failed() { return false; }
        let want_blend = blend && self.can_blend;
        if self.uses_base(formats, depth, samples) {
            return matches!(if want_blend { self.blend.get() } else { self.solid.get() }, Some(Ok(_)));
        }
        if !self.base_ready() { return false; }
        self.variants.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|v| {
            v.matches(formats, depth, want_blend, samples.max(1)) && matches!(v.pipeline.get(), Some(Ok(_)))
        })
    }

    /// Whether a draw would build this variant now. A cached failure is already settled.
    pub fn lacks(&self, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> bool {
        if self.base_failed() || !self.base_ready() || self.uses_base(formats, depth, samples) { return false; }
        let want_blend = blend && self.can_blend;
        !self.variants.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|v| {
            v.matches(formats, depth, want_blend, samples.max(1)) && v.pipeline.get().is_some()
        })
    }

    /// Once the base is ready, the first draw can use a missing variant in this frame.
    pub fn ensure_variant(&self, device: ObjcId, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> Select {
        if self.base_failed() { return Select::Failed; }
        let want_blend = blend && self.can_blend;
        if self.uses_base(formats, depth, samples) {
            return match if want_blend { self.blend.get() } else { self.solid.get() } {
                Some(Ok(p)) => Select::State(p.as_id()),
                Some(Err(_)) => Select::Failed,
                None => Select::Pending,
            };
        }
        if !self.base_ready() { return Select::Pending; }
        let variant = self.variant(formats, depth, want_blend, samples.max(1));
        if let Some(result) = variant.pipeline.get() {
            return match result { Ok(p) => Select::State(p.as_id()), Err(_) => Select::Failed };
        }
        self.build_variant_sync(device, &variant)
    }

    pub fn select(&self, dev: &Device, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> Select {
        self.ensure_variant(dev.device, formats, depth, blend, samples)
    }

    /// A speculative depth-only or MSAA pass keeps its current mode until the upgrade is ready.
    pub fn ready_for(self: &Arc<Self>, dev: &Device, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> bool {
        if self.base_failed() { return false; }
        let want_blend = blend && self.can_blend;
        if self.uses_base(formats, depth, samples) {
            return matches!(if want_blend { self.blend.get() } else { self.solid.get() }, Some(Ok(_)));
        }
        let variant = self.variant(formats, depth, want_blend, samples.max(1));
        if self.claim_variant(&variant) {
            let state = self.clone();
            let job = variant.clone();
            let device = RcObjcId::from_unowned(NonNull::new(dev.device).expect("device"));
            dev.compile(Box::new(move || state.compile_variant(device.as_id(), job, VariantBuild::PassUpgrade)));
        }
        matches!(variant.pipeline.get(), Some(Ok(_)))
    }

    fn variant(&self, formats: &[MTLPixelFormat], depth: MTLPixelFormat, blend: bool, samples: u8) -> Arc<Variant> {
        let mut variants = self.variants.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = variants.iter().find(|v| v.matches(formats, depth, blend, samples)) {
            return v.clone();
        }
        let v = Arc::new(Variant { formats: formats.to_vec(), depth, blend, samples, queued: AtomicBool::new(false), pipeline: OnceLock::new() });
        variants.push(v.clone());
        v
    }

    fn variant_descriptor(&self, vertex: &RcObjcId, fragment: &RcObjcId, variant: &Variant) -> Result<RcObjcId, String> {
        if self.blend_add && variant.blend {
            if let Some(error) = add_blend_error(&variant.formats, self.written) { return Err(error); }
        }
        Ok(build_descriptor(
            vertex.as_id(), fragment.as_id(), &variant.formats, variant.depth,
            self.written, variant.blend, self.blend_max, self.blend_add, self.discards, variant.samples,
        ))
    }

    fn variant_error(&self, variant: &Variant, error: ObjcId) -> String {
        let log = describe_error(error, "Metal returned no pipeline");
        if self.blend_add && variant.blend {
            format!("blend_op: @Add requires Metal color attachment blending for {:?}: {log}", variant.formats)
        } else { log }
    }

    fn build_variant_sync(&self, device: ObjcId, variant: &Arc<Variant>) -> Select {
        let Some((vertex, fragment)) = self.functions.get() else { return Select::Pending };
        variant.queued.store(true, Ordering::Release);
        let result = self.variant_descriptor(vertex, fragment, variant).and_then(|descriptor| {
            let mut error: ObjcId = nil;
            let pipeline: ObjcId = unsafe { msg_send![device, newRenderPipelineStateWithDescriptor: descriptor.as_id() error: &mut error] };
            NonNull::new(pipeline).map(RcObjcId::from_owned).ok_or_else(|| self.variant_error(variant, error))
        });
        self.settle_variant(variant, result, VariantBuild::Draw)
    }

    /// An in-flight probe and a synchronous draw can build the same key. Publish after the
    /// driver call, notify once, and give the draw whichever state won.
    fn settle_variant(&self, variant: &Arc<Variant>, result: Result<RcObjcId, String>, build: VariantBuild) -> Select {
        let settled_here = variant.pipeline.set(result).is_ok();
        let result = variant.pipeline.get().expect("variant settled");
        if settled_here {
            match result {
                Ok(_) => {
                    if build == VariantBuild::PassUpgrade && self.base_ready() && !self.failed.load(Ordering::Acquire) {
                        self.mailbox.push(Done::PipelineReady { sid: self.sid, id: self.id, incarnation: self.incarnation.clone() });
                    }
                    self.answer_layouts(None);
                }
                Err(log) => {
                    let matched = self.answer_layouts(Some(&**variant));
                    if build != VariantBuild::LayoutProbe && !matched {
                        self.mailbox.push(Done::PipelineLayoutFailed {
                            sid: self.sid, id: self.id, incarnation: self.incarnation.clone(),
                            layout: crate::cmds::PipelineLayout::UNPROBED, log: log.clone(),
                        });
                    }
                }
            }
        }
        match result { Ok(p) => Select::State(p.as_id()), Err(_) => Select::Failed }
    }

    /// Layout probes and speculative pass upgrades submit their builds asynchronously.
    fn compile_variant(self: Arc<Self>, device: ObjcId, variant: Arc<Variant>, build: VariantBuild) {
        let Some((vertex, fragment)) = self.functions.get() else { return };
        let descriptor = match self.variant_descriptor(vertex, fragment, &variant) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                self.settle_variant(&variant, Err(error), build);
                return;
            }
        };
        let completion = objc_block!(move |pipeline: ObjcId, error: ObjcId| {
            let result = NonNull::new(pipeline).map(RcObjcId::from_unowned).ok_or_else(|| self.variant_error(&variant, error));
            self.settle_variant(&variant, result, build);
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
        for (formats, depth, blend, samples, p) in c.variants {
            variants.push(Arc::new(Variant { formats, depth, blend, samples, queued: AtomicBool::new(true), pipeline: OnceLock::from(Ok(p)) }));
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
#[relax_api::relax_api(loader = "a shader test's Metal compile of MSL: such a test runs natively")]
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
    if ready.blend_add && ready.can_blend {
        if let Some(error) = add_blend_error(&ready.declared, ready.written) {
            ready.failure(error);
            return;
        }
    }
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
                ready.clone().compile_variant(callback_device.as_id(), variant, VariantBuild::LayoutProbe);
            }
            // independent descriptors: a driver may consume them after this call returns
            for blend in [true, false] {
                let descriptor = build_descriptor(
                    vertex.as_id(), fragment.as_id(), &ready.declared, MTLPixelFormat::Depth32Float,
                    ready.written, blend && ready.can_blend, ready.blend_max, ready.blend_add, ready.discards, 1,
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Pool;
    use std::sync::mpsc;
    use std::time::Duration;

    const MSL: &str = "#include <metal_stdlib>
using namespace metal;
struct Vertex { float4 pos [[position]]; };
vertex Vertex vertex_main() { return Vertex{float4(0.0, 0.0, 0.0, 1.0)}; }
fragment float4 fragment_main() { return float4(1.0); }";

    fn test_functions(device: ObjcId) -> (RcObjcId, RcObjcId) {
        let options = compile_options(false);
        let mut error: ObjcId = nil;
        let library: ObjcId = unsafe { msg_send![device, newLibraryWithSource: str_to_nsstring(MSL) options: options.as_id() error: &mut error] };
        let library = RcObjcId::from_owned(NonNull::new(library).unwrap_or_else(|| panic!("{}", describe_error(error, "no Metal library"))));
        let function = |name: &str| {
            let f: ObjcId = unsafe { msg_send![library.as_id(), newFunctionWithName: str_to_nsstring(name)] };
            RcObjcId::from_owned(NonNull::new(f).unwrap_or_else(|| panic!("missing {name}")))
        };
        (function("vertex_main"), function("fragment_main"))
    }

    #[test]
    fn descriptor_depth_format_matches_attachment_presence() {
        let _pool = Pool::new();
        let device = RcObjcId::from_owned(NonNull::new(unsafe { MTLCreateSystemDefaultDevice() }).expect("Metal device"));
        let (vertex, fragment) = test_functions(device.as_id());
        for (depth, expected) in [(MTLPixelFormat::Invalid, 0), (MTLPixelFormat::Depth32Float, 252)] {
            let descriptor = build_descriptor(
                vertex.as_id(), fragment.as_id(), &[MTLPixelFormat::RGBA8Unorm], depth,
                1, false, false, false, false, 1,
            );
            let actual: u64 = unsafe { msg_send![descriptor.as_id(), depthAttachmentPixelFormat] };
            assert_eq!(actual, expected, "descriptor for {depth:?}");
        }
    }

    #[test]
    fn depth_and_color_only_passes_select_distinct_psos_and_survive_relaunch() {
        let _pool = Pool::new();
        let (wake, events) = mpsc::channel();
        let mailbox = Mailbox::new(Arc::new(move |_, _| { let _ = wake.send(()); }));
        let dev = Device::new(mailbox).expect("Metal device");
        let desc = PipelineDesc { outputs: 1, color_formats: [format::RGBA8, 0, 0, 0], ..Default::default() };
        let create_state = |taken| create(&dev, 1, Id::new(0, 1), Arc::default(), MSL, &desc, "depth formats", taken);
        let state = create_state(None);
        let formats = [MTLPixelFormat::RGBA8Unorm];
        let depth = MTLPixelFormat::Depth32Float;
        let absent = MTLPixelFormat::Invalid;
        assert!(state.uses_base(&formats, depth, 1), "the declared layout with depth uses the base state");
        assert!(!state.uses_base(&formats, absent, 1), "a colour-only pass needs its own state");

        const LAYOUT: u32 = 1;
        state.want_layout(&dev, LAYOUT, &formats, absent, 1);
        let mut base_ready = false;
        let mut layout_ready = false;
        while !(base_ready && layout_ready) {
            events.recv_timeout(Duration::from_secs(30)).expect("Metal pipeline completion");
            for done in dev.mailbox.take() {
                match done {
                    Done::PipelineReady { .. } => base_ready = true,
                    Done::PipelineLayoutReady { layout: LAYOUT, .. } => layout_ready = true,
                    Done::PipelineFailed { log, .. } | Done::PipelineLayoutFailed { log, .. } => panic!("{log}"),
                    _ => {}
                }
            }
        }
        let selected = |state: &Arc<PipeState>, depth| match state.select(&dev, &formats, depth, false, 1) {
            Select::State(pso) => pso,
            Select::Pending => panic!("state for {depth:?} still pending"),
            Select::Failed => panic!("state for {depth:?} failed"),
        };
        let depth_pso = selected(&state, depth);
        let color_pso = selected(&state, absent);
        assert_ne!(depth_pso, color_pso, "depth presence must select distinct Metal states");
        let compiled = state.compiled(&desc).expect("compiled pipeline");
        assert!(compiled.variants.iter().any(|(colors, d, blend, samples, pso)| {
            colors.as_slice() == formats && *d == absent && !*blend && *samples == 1 && pso.as_id() == color_pso
        }), "snapshot retains the colour-only state under its layout");
        drop(state);
        let restored = create_state(Some(compiled));
        assert_eq!(selected(&restored, absent), color_pso, "relaunch reuses the colour-only state immediately");
        assert_eq!(selected(&restored, depth), depth_pso, "relaunch reuses the depth-attached base state");
    }

    /// With no layout probes, base compilation posts exactly one completion.
    fn compile_base(desc: &PipelineDesc) -> (Device, Arc<PipeState>, Result<(), String>) {
        let (wake, events) = mpsc::channel();
        let mailbox = Mailbox::new(Arc::new(move |_, _| { let _ = wake.send(()); }));
        let dev = Device::new(mailbox).expect("Metal device");
        let state = create(&dev, 1, Id::new(0, 1), Arc::default(), MSL, desc, "base compile", None);
        events.recv_timeout(Duration::from_secs(30)).expect("base compile completion");
        let mut done = dev.mailbox.take();
        assert_eq!(done.len(), 1, "one outcome for the base compile");
        let result = match done.pop().unwrap() {
            Done::PipelineReady { .. } => Ok(()),
            Done::PipelineFailed { log, .. } => Err(log),
            _ => panic!("expected the base pipeline outcome"),
        };
        (dev, state, result)
    }

    #[test]
    fn unprobed_color_only_select_builds_synchronously_after_base_ready_and_survives_relaunch() {
        let _pool = Pool::new();
        let desc = PipelineDesc { outputs: 1, color_formats: [format::RGBA8, 0, 0, 0], ..Default::default() };
        let (dev, state, base) = compile_base(&desc);
        base.expect("base pipeline ready");
        let formats = [MTLPixelFormat::RGBA8Unorm];
        let absent = MTLPixelFormat::Invalid;
        assert!(state.lacks(&formats, absent, false, 1), "first colour-only draw needs a variant");
        let Select::State(base_pso) = state.select(&dev, &formats, MTLPixelFormat::Depth32Float, false, 1) else { panic!("missing base state") };
        let Select::State(color_pso) = state.select(&dev, &formats, absent, false, 1) else { panic!("first draw needs the colour-only state immediately") };
        assert_ne!(base_pso, color_pso, "the colour-only draw needs a different descriptor");
        assert!(!state.lacks(&formats, absent, false, 1));
        assert!(state.has_ready(&formats, absent, false, 1));
        assert!(dev.mailbox.take().is_empty(), "a synchronous draw needs no repeat PipelineReady");

        let compiled = state.compiled(&desc).expect("completed pipeline");
        drop(state);
        let restored = create(&dev, 2, Id::new(0, 1), Arc::default(), MSL, &desc, "relaunch", Some(compiled));
        assert!(!restored.lacks(&formats, absent, false, 1), "relaunch already has the colour-only state");
        assert!(matches!(restored.select(&dev, &formats, absent, false, 1), Select::State(p) if p == color_pso));
    }

    #[test]
    fn select_stays_pending_before_base_ready_for_non_base_layout() {
        let _pool = Pool::new();
        let dev = Device::new(Mailbox::new(Arc::new(|_, _| {}))).expect("Metal device");
        let formats = [MTLPixelFormat::RGBA8Unorm];
        let state = PipeState {
            functions: OnceLock::new(), solid: OnceLock::new(), blend: OnceLock::new(),
            variants: Mutex::new(Vec::new()), wanted: Mutex::new(Vec::new()), written: 1,
            blend_max: false, blend_add: false, can_blend: false, discards: false,
            name: "pending".into(), declared: formats.to_vec(), settled: AtomicU8::new(0), failed: AtomicBool::new(false),
            sid: 1, id: Id::new(0, 1), incarnation: Arc::default(), mailbox: dev.mailbox.clone(), source: MSL.into(),
        };
        let check_pending = || {
            assert!(!state.base_ready());
            assert!(!state.lacks(&formats, MTLPixelFormat::Invalid, false, 1), "work ahead waits for the base");
            assert!(matches!(state.select(&dev, &formats, MTLPixelFormat::Invalid, false, 1), Select::Pending));
            assert!(state.variants.lock().unwrap().is_empty(), "a pending draw creates no variants");
        };
        check_pending();
        let (vertex, fragment) = test_functions(dev.device);
        let descriptor = build_descriptor(vertex.as_id(), fragment.as_id(), &formats, MTLPixelFormat::Depth32Float, 1, false, false, false, false, 1);
        let mut error: ObjcId = nil;
        let solid: ObjcId = unsafe { msg_send![dev.device, newRenderPipelineStateWithDescriptor: descriptor.as_id() error: &mut error] };
        let solid = RcObjcId::from_owned(NonNull::new(solid).unwrap_or_else(|| panic!("{}", describe_error(error, "no solid state"))));
        assert!(state.functions.set((vertex, fragment)).is_ok());
        assert!(state.solid.set(Ok(solid)).is_ok());
        check_pending();
    }

    #[test]
    fn unprobed_sync_variant_failure_caches_and_reports_layout_error() {
        let _pool = Pool::new();
        let desc = PipelineDesc { outputs: 1, color_formats: [format::BGRA8, 0, 0, 0], blend: pipeline_blend::ADD, ..Default::default() };
        let (dev, state, base) = compile_base(&desc);
        base.expect("BGRA8 supports additive blending");
        let formats = [MTLPixelFormat::R32Uint];
        let absent = MTLPixelFormat::Invalid;
        let take_failure = |layout| match dev.mailbox.take().as_slice() {
            [Done::PipelineLayoutFailed { layout: got, log, .. }] if *got == layout => log.clone(),
            _ => panic!("expected one synchronous failure for layout {layout}"),
        };
        assert!(matches!(state.select(&dev, &formats, absent, true, 1), Select::Failed));
        let log = take_failure(crate::cmds::PipelineLayout::UNPROBED);
        assert!(log.contains("blend_op: @Add requires Metal color attachment blending") && log.contains("R32Uint"), "{log}");
        assert!(!state.lacks(&formats, absent, true, 1), "the failure is cached");
        assert!(!state.has_ready(&formats, absent, true, 1), "a cached failure has no PSO");
        assert!(matches!(state.select(&dev, &formats, absent, true, 1), Select::Failed));
        assert!(dev.mailbox.take().is_empty(), "reselecting a failed variant emits nothing");
        state.want_layout(&dev, 7, &formats, absent, 1);
        assert_eq!(take_failure(7), log, "a later layout probe receives the cached error");
        let solid = state.variant(&formats, absent, false, 1);
        assert!(!solid.queued.load(Ordering::Acquire), "the failed layout must not dispatch its solid sibling");
    }

    #[test]
    fn declared_unblendable_add_fails_base_compile_without_metal_validation_error() {
        use MTLPixelFormat::*;
        let _pool = Pool::new();
        assert!(add_blend_error(&[R32Float, RGBA32Float], 0b11).is_none(), "Metal decides 32-bit float blend support");
        assert!(add_blend_error(&[R32Uint], 0).is_none(), "unwritten attachments do not blend");
        for format in [R32Uint, Depth32Float, Invalid, BC5_RGUnorm, BC7_RGBAUnorm, ASTC_4x4_LDR] {
            assert!(add_blend_error(&[format], 1).is_some(), "reject written {format:?} before calling Metal");
        }
        let desc = PipelineDesc { outputs: 1, color_formats: [format::R32UINT, 0, 0, 0], blend: pipeline_blend::ADD, ..Default::default() };
        let (dev, state, base) = compile_base(&desc);
        let log = base.expect_err("an unblendable declaration fails the base compile");
        assert!(log.contains("blend_op: @Add requires Metal color attachment blending") && log.contains("R32Uint"), "{log}");
        assert!(state.functions.get().is_none(), "the error precedes library compilation");
        assert!(!state.base_ready());
        assert!(!state.lacks(&[R32Uint], Invalid, true, 1));
        for depth in [Depth32Float, Invalid] {
            assert!(matches!(state.select(&dev, &[R32Uint], depth, true, 1), Select::Failed));
        }
        state.want_layout(&dev, 1, &[R32Uint], Invalid, 1);
        assert!(dev.mailbox.take().is_empty(), "a failed base emits no further pipeline or layout events");
    }
}
