//! Native compute resources and per-frame recording. GPU compilation and readback are asynchronous.
use crate::{Cx, Texture, TextureFormat, DrawPassId, makepad_live_id::LiveId, makepad_script::{ScriptVm, ScriptObject}, script::vm::ScriptVmCx};
use crate::makepad_script::shader_ir_compute::{self as compiler, ComputeLayout, ComputeTarget, ComputeSlotKind, CompiledKernel};
use makepad_hal::{self as hal, Id, CmdWriter, StorageBinding, StorageImageBinding};
use hal::codec::ArenaSink;
use std::{cell::Cell, rc::{Rc, Weak}};

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuBufferFlags { pub indirect: bool, pub readback: bool }
#[derive(Debug)]
struct BufferInner { id: Id, size: u64, flags: GpuBufferFlags, alive: Cell<bool> }
/// Clones retain a GPU allocation. Explicit destruction invalidates every clone.
#[derive(Clone, Debug)]
pub struct GpuBuffer(Rc<BufferInner>);
impl PartialEq for GpuBuffer { fn eq(&self, other: &Self) -> bool { Rc::ptr_eq(&self.0, &other.0) } }
impl GpuBuffer {
    /// The ledger names the caller as the buffer's creation site (and the open `gpu_owner` scopes
    /// as its owner), as it does for textures.
    #[track_caller]
    pub fn new(cx: &mut Cx, size: u64, flags: GpuBufferFlags) -> Result<Self, String> {
        if !cx.compute_supported() { return Err("compute is unavailable on this backend".into()); }
        if size == 0 || size % 4 != 0 { return Err("storage buffer size must be a nonzero multiple of four".into()); }
        let kind = hal::buffer_kind::STORAGE | if flags.indirect { hal::buffer_kind::INDIRECT } else { 0 } | if flags.readback { hal::buffer_kind::READBACK } else { 0 };
        let attr = cx.hal.frames.attr(&crate::gpu_ledger::GpuOrigin::here(), 0);
        let id = cx.hal.frames.buffer_create(kind, size, attr);
        let inner = Rc::new(BufferInner { id, size, flags, alive: Cell::new(true) });
        cx.hal.compute.buffers.push((id, Rc::downgrade(&inner)));
        Ok(Self(inner))
    }
    pub fn size(&self) -> u64 { self.0.size }
    pub(crate) fn hal_id(&self) -> Id { if self.0.alive.get() { self.0.id } else { Id::NONE } }
    pub fn update(&self, cx: &mut Cx, offset: u64, bytes: &[u8]) -> Result<(), String> {
        self.check_update(offset, bytes.len())?;
        self.update_owned(cx, offset, bytes.to_vec())
    }
    /// [`GpuBuffer::update`] with bytes it keeps as they are: an upload laid out on another
    /// thread moves in without a copy on this one.
    pub fn update_owned(&self, cx: &mut Cx, offset: u64, bytes: Vec<u8>) -> Result<(), String> {
        self.check_update(offset, bytes.len())?;
        cx.hal.frames.owned_buffer_update(self.0.id, offset, bytes);
        crate::thread::SignalToUI::set_internal_signal();
        Ok(())
    }
    fn check_update(&self, offset: u64, bytes: usize) -> Result<(), String> {
        if !self.0.alive.get() { return Err("GPU buffer upload targets a destroyed buffer".into()); }
        if offset.checked_add(bytes as u64).is_none_or(|end| end > self.size()) {
            return Err(format!("GPU buffer {:?} upload offset={offset} bytes={bytes} exceeds buffer size={}", self.0.id, self.size()));
        }
        Ok(())
    }
    pub fn destroy(&self, cx: &mut Cx) {
        if self.0.alive.replace(false) { cx.hal.frames.buffer_destroy(self.0.id); }
    }
}

struct KernelInner { name: String, compiled: CompiledKernel, pipeline: Rc<Cell<Id>> }
#[derive(Clone)]
pub struct ComputeKernel(Rc<KernelInner>);
impl ComputeKernel {
    /// Lowers Splash on the same path as draw shaders. Native pipeline creation is lazy,
    /// and the HAL performs driver compilation on its existing compiler worker.
    pub fn new(vm: &mut ScriptVm, object: ScriptObject, name: &str) -> Result<Self, String> {
        let cx = vm.host.cx();
        if !cx.compute_supported() { return Err("compute is unavailable on this backend".into()); }
        let target = match cx.hal.info.unwrap().dialect {
            hal::dialect::MSL => ComputeTarget::Msl, hal::dialect::HLSL => ComputeTarget::Hlsl,
            hal::dialect::SPIRV => ComputeTarget::Spirv, _ => return Err("unsupported compute dialect".into()),
        };
        let result = compiler::compile_compute_kernel_result(vm, object, target);
        let compiled = result.view().to_kernel()?;
        let inner = Rc::new(KernelInner { name: name.into(), compiled, pipeline: Rc::new(Cell::new(Id::NONE)) });
        vm.host.cx_mut().hal.compute.kernels.push((inner.pipeline.clone(), Rc::downgrade(&inner)));
        Ok(Self(inner))
    }
    pub fn layout(&self) -> &ComputeLayout { &self.0.compiled.layout }
    pub fn params(&self) -> ComputeParams { ComputeParams { kernel: self.clone(), bytes: vec![0; self.layout().push_size as usize] } }
    fn pipeline(&self, fb: &mut crate::frame_build::FrameBuilder) -> Id {
        if !self.0.pipeline.get().is_none() { return self.0.pipeline.get(); }
        let l = self.layout();
        let code = match &self.0.compiled.code {
            compiler::KernelCode::Spirv(words) => words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>(),
            compiler::KernelCode::Msl(text) | compiler::KernelCode::Hlsl(text) => text.as_bytes().to_vec(),
        };
        let buffers: Vec<_> = l.buffers.iter().map(|b| StorageBinding { index: b.index, offset: b.offset, writable: b.writable as u32 }).collect();
        let textures: Vec<_> = l.storage_textures.iter().map(|t| StorageImageBinding { index: t.index, format: storage_format(t.format) }).collect();
        let desc = hal::ComputeDesc { push_size: l.push_size, param_binding: l.param_binding.unwrap_or(u32::MAX), param_binding_size: l.param_binding_size, group_count_offset: l.group_count_offset.unwrap_or(u32::MAX), workgroup: l.workgroup, precise_math: l.precise_math as u32, ..Default::default() };
        let name = fb.intern(&self.0.name);
        let blob = CmdWriter::compute_pipeline_blob(hal::PipelineDesc { name, ..Default::default() }, &code, desc, &buffers, &textures);
        let id = fb.compute_pipeline(&self.0.name, &blob);
        self.0.pipeline.set(id);
        id
    }
}

#[derive(Clone)]
pub struct ComputeParams { kernel: ComputeKernel, bytes: Vec<u8> }
impl ComputeParams {
    /// Packed little-endian scalar/vector lanes, checked against the reflected named slot.
    pub fn set(&mut self, name: LiveId, bytes: &[u8]) -> Result<&mut Self, String> {
        let slot = self.kernel.layout().slots.iter().find(|s| s.name == name && matches!(s.kind, ComputeSlotKind::Param { .. })).ok_or("unknown compute parameter")?;
        if bytes.len() != slot.size as usize { return Err("compute parameter size mismatch".into()); }
        self.bytes[slot.offset as usize..slot.offset as usize + bytes.len()].copy_from_slice(bytes);
        Ok(self)
    }
    pub fn set_u32(&mut self, name: LiveId, value: u32) -> Result<&mut Self, String> { self.set(name, &value.to_le_bytes()) }
    pub fn set_f32(&mut self, name: LiveId, value: f32) -> Result<&mut Self, String> { self.set(name, &value.to_le_bytes()) }
}
struct Recorded { kernel: ComputeKernel, timer: Option<String>, groups: [u32; 3], indirect: Option<(GpuBuffer, u64)>, params: ComputeParams, buffers: Vec<GpuBuffer>, textures: Vec<Texture> }
/// `before` attaches compute to a draw dependency; None progresses independently of paint.
pub struct ComputePass { name: String, before: Option<DrawPassId>, dispatches: Vec<Recorded> }
impl ComputePass {
    pub fn new(name: impl Into<String>, before: Option<DrawPassId>) -> Self { Self { name: name.into(), before, dispatches: Vec::new() } }
    pub fn dispatch(&mut self, kernel: &ComputeKernel, groups: [u32; 3], params: &ComputeParams, buffers: &[GpuBuffer], textures: &[Texture]) -> Result<(), String> {
        if !Rc::ptr_eq(&kernel.0, &params.kernel.0) || buffers.len() != kernel.layout().buffers.len() || textures.len() != kernel.layout().storage_textures.len() { return Err("compute bindings do not match kernel reflection".into()); }
        for (buffer, binding) in buffers.iter().zip(&kernel.layout().buffers) {
            if buffer.hal_id().is_none() || buffer.size() % binding.alignment as u64 != 0 { return Err("invalid storage buffer binding".into()); }
        }
        self.dispatches.push(Recorded { kernel: kernel.clone(), timer: None, groups, indirect: None, params: params.clone(), buffers: buffers.to_vec(), textures: textures.to_vec() });
        Ok(())
    }
    /// Measure this dispatch without splitting the pass or its submission.
    pub fn dispatch_timed(&mut self, name: &str, kernel: &ComputeKernel, groups: [u32; 3], params: &ComputeParams, buffers: &[GpuBuffer], textures: &[Texture]) -> Result<(), String> {
        self.dispatch(kernel, groups, params, buffers, textures)?;
        self.dispatches.last_mut().unwrap().timer = Some(name.to_string());
        crate::gpu_frame_timer::watch(name);
        Ok(())
    }
    pub fn dispatch_indirect(&mut self, kernel: &ComputeKernel, args: &GpuBuffer, offset: u64, params: &ComputeParams, buffers: &[GpuBuffer], textures: &[Texture]) -> Result<(), String> {
        if !args.0.flags.indirect || offset % 4 != 0 || offset.checked_add(12).is_none_or(|n| n > args.size()) { return Err("invalid indirect dispatch buffer".into()); }
        self.dispatch(kernel, [0; 3], params, buffers, textures)?;
        self.dispatches.last_mut().unwrap().indirect = Some((args.clone(), offset));
        Ok(())
    }
    /// Timestamp an indirect job in the same pass as its producer jobs.
    pub fn dispatch_indirect_timed(&mut self, name: &str, kernel: &ComputeKernel, args: &GpuBuffer, offset: u64, params: &ComputeParams, buffers: &[GpuBuffer], textures: &[Texture]) -> Result<(), String> {
        self.dispatch_indirect(kernel, args, offset, params, buffers, textures)?;
        self.dispatches.last_mut().unwrap().timer = Some(name.to_string());
        crate::gpu_frame_timer::watch(name);
        Ok(())
    }
    /// Queue compute independently of paint, or before its attached draw pass.
    pub fn submit(self, cx: &mut Cx) {
        match self.before {
            Some(pass) => cx.repaint_pass(pass),
            None => {
                cx.hal.compute.beat = true;
                crate::thread::SignalToUI::set_internal_signal();
            },
        }
        cx.hal.compute.passes.push(self);
    }
}
#[derive(Default)]
pub(crate) struct ComputeState {
    /// Unattached compute was submitted since the last HAL turn.
    pub(crate) beat: bool,
    buffers: Vec<(Id, Weak<BufferInner>)>,
    passes: Vec<ComputePass>,
    retained: Vec<ComputePass>,
    kernels: Vec<(Rc<Cell<Id>>, Weak<KernelInner>)>,
}
pub(crate) fn storage_format(format: compiler::StorageTextureFormat) -> u32 {
    use compiler::StorageTextureFormat as F;
    match format { F::R16f => hal::format::R16F, F::R8unorm => hal::format::R8, F::R32f => hal::format::R32F, F::R32uint => hal::format::R32UINT, F::Rgba8unorm => hal::format::RGBA8, F::Rgba16f => hal::format::RGBA16F }
}
impl Cx {
    /// Upload a tightly packed subrectangle into the same texture kernels write.
    pub fn update_storage_texture(&mut self, texture: &Texture, rect: [u32; 4], bytes: &[u8]) -> Result<(), String> {
        let tid = texture.texture_id();
        let TextureFormat::Storage { format, width, height } = self.textures[tid].format else { return Err("not a storage texture".into()); };
        if !self.storage_texture_supported(format) || rect[0].checked_add(rect[2]).is_none_or(|n| n as usize > width) || rect[1].checked_add(rect[3]).is_none_or(|n| n as usize > height)
            || (rect[2] as u64).checked_mul(rect[3] as u64).and_then(|n| n.checked_mul(hal::format::texel_bytes(storage_format(format)))) != Some(bytes.len() as u64) { return Err("unsupported storage format or invalid upload rectangle".into()); }
        let mut fb = std::mem::take(&mut self.hal.frames);
        let id = self.fb_attachment(&mut fb, tid, width, height, false);
        fb.owned_texture_update(id, rect, bytes.to_vec());
        self.hal.frames = fb;
        crate::thread::SignalToUI::set_internal_signal();
        Ok(())
    }

    /// Device replacements so far (`Event::GpuReset`): what was uploaded on an older
    /// generation is gone, and caches keyed by it upload again.
    pub fn gpu_device_generation(&self) -> u64 { self.hal.device_generation }
    /// The device is lost and not replaced yet: nothing submitted now runs.
    pub fn gpu_device_lost(&self) -> bool { self.hal.device_lost }
    /// The backend replaces a lost device (`Event::GpuReset` follows `Event::GpuLost`). False:
    /// a lost device stays lost, and work waiting for it is to be answered as failed.
    pub fn gpu_device_recovers(&self) -> bool { self.hal.info.is_some_and(|i| i.flags & hal::info_flags::DEVICE_RESET != 0) }
    pub fn compute_supported(&self) -> bool { self.hal.info.is_some_and(|i| i.flags & hal::info_flags::COMPUTE != 0) }
    pub fn storage_texture_supported(&self, format: compiler::StorageTextureFormat) -> bool { self.hal.info.is_some_and(|i| i.storage_formats & (1 << storage_format(format)) != 0) }
    /// Whether the compute passes encoded before `before` (None: the unattached ones, ahead of
    /// every draw pass) have their pipelines. Unattached passes still queued when a draw pass
    /// prepares were held for a pipeline: a pass attached behind one of them waits too, so
    /// compute keeps its submission order.
    pub(crate) fn compute_prepare(&mut self, fb: &mut crate::frame_build::FrameBuilder, before: Option<DrawPassId>) -> bool {
        let mut ready = true;
        let mut held_before = false;
        for pass in &self.hal.compute.passes {
            if pass.before != before {
                held_before |= pass.before.is_none();
                continue;
            }
            ready &= !held_before;
            for d in &pass.dispatches {
                let id = d.kernel.pipeline(fb);
                if fb.pipeline_waited_for(id.index as usize) { ready = false; }
            }
        }
        ready
    }
    pub(crate) fn compute_encode(&mut self, fb: &mut crate::frame_build::FrameBuilder, before: Option<DrawPassId>) -> Vec<u64> {
        let mut built = Vec::new();
        let queued = std::mem::take(&mut self.hal.compute.passes);
        for pass in queued {
            if pass.before != before { self.hal.compute.passes.push(pass); continue; }
            // A packet per pass gives the existing FrameDone timing path its own
            // span and name. Submission order still places it before its draw dependency.
            let serial = self.textures.1.serials.submit();
            fb.w.frame_begin(serial, hal::FrameMeta::default());
            let mut dispatches = Vec::new();
            for d in &pass.dispatches {
                let mut textures = Vec::new();
                for texture in &d.textures {
                    let tid = texture.texture_id();
                    let (width, height) = match self.textures[tid].format { TextureFormat::Storage { width, height, .. } => (width, height), _ => (0, 0) };
                    textures.push(self.fb_attachment(fb, tid, width, height, false));
                }
                let buffers: Vec<_> = d.buffers.iter().map(GpuBuffer::hal_id).collect();
                let b = fb.w.put_ids(&buffers); let t = fb.w.put_ids(&textures);
                let params = fb.w.put_bytes(&d.params.bytes);
                let timer = d.timer.as_ref().map_or(0, |name| fb.intern(name));
                dispatches.push(hal::Dispatch { pipeline: d.kernel.0.pipeline.get(), timer, groups: d.groups, indirect_buffer: d.indirect.as_ref().map_or(Id::NONE, |a| a.0.hal_id()), indirect_offset: d.indirect.as_ref().map_or(0, |a| a.1), params, buffers: hal::Range { off: b.off * 8, len: b.len * 8 }, textures: hal::Range { off: t.off * 8, len: t.len * 8 } });
            }
            let name = fb.intern(&pass.name);
            fb.w.compute_pass(name, name, &dispatches);
            fb.w.frame_end(Id::NONE, hal::target::COMPUTE);
            if crate::gpu_frame_timer::enabled() {
                let group = before.and_then(|p| self.get_pass_window_id(p)).map(|window|
                    (window.id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ self.repaint_id);
                fb.timings.insert(serial, (group.unwrap_or(serial), pass.name.clone(), group.is_none()));
            }
            fb.stats.frames += 1;
            fb.stats.passes += 1;
            built.push(serial);
            self.hal.compute.retained.push(pass);
        }
        built
    }
    pub(crate) fn compute_after_submit(&mut self) {
        self.hal.compute.retained.clear();
        self.hal.compute.kernels.retain(|(id, weak)| {
            if weak.strong_count() != 0 { return true; }
            if !id.get().is_none() { self.hal.frames.w.pipeline_destroy(id.get()); }
            false
        });
        self.hal.compute.buffers.retain(|(id, weak)| {
            match weak.upgrade() {
                Some(b) => b.alive.get(),
                None => { self.hal.frames.buffer_destroy(*id); false }
            }
        });
    }
}

impl ComputeState {
    pub(crate) fn has_unattached(&self) -> bool { self.passes.iter().any(|p| p.before.is_none()) }
    /// A queued pass binds this buffer: a read of it follows that pass, and only such a pass.
    pub(crate) fn binds_buffer(&self, id: Id) -> bool {
        self.passes.iter().flat_map(|p| &p.dispatches).any(|d| d.buffers.iter().chain(d.indirect.as_ref().map(|i| &i.0)).any(|b| b.hal_id() == id))
    }
    /// A queued pass binds this storage texture.
    pub(crate) fn binds_texture(&self, id: crate::TextureId) -> bool {
        self.passes.iter().flat_map(|p| &p.dispatches).any(|d| d.textures.iter().any(|t| t.texture_id() == id))
    }
}
