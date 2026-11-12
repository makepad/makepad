use super::{device::{Device, error}, resources::Buffer, submit::{Batch, Ticket}, Vulkan};
use crate::{backend::exec::PipeStatus, buffers::{CmdView, PipelineBlob}, compute::{self, KernelLayout}, session::{Session, World}, types::*};
use makepad_vulkan::vk;
use std::sync::{Arc, OnceLock};

pub(super) struct Program {
    dev: Arc<Device>,
    pub layout: KernelLayout,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptors: vk::DescriptorSetLayout,
    /// Set 0, by binding: the storage images, the sampled textures, the samplers, the parameter block.
    bindings: Vec<(u32, vk::DescriptorType)>,
    /// The kernel's samplers, bound after its sampled textures in their order.
    samplers: Vec<vk::Sampler>,
}
impl Drop for Program {
    fn drop(&mut self) { unsafe {
        for s in self.samplers.drain(..) { self.dev.raw.destroy_sampler(s, None); }
        self.dev.raw.destroy_pipeline(self.pipeline, None);
        self.dev.raw.destroy_pipeline_layout(self.pipeline_layout, None);
        self.dev.raw.destroy_descriptor_set_layout(self.descriptors, None);
    } }
}
impl Program {
    fn new(dev: Arc<Device>, layout: KernelLayout, code: &[u32]) -> Result<Arc<Self>, String> {
        let mut bindings: Vec<_> = layout.textures.iter().map(|t| (t.index, vk::DescriptorType::STORAGE_IMAGE)).collect();
        bindings.extend(layout.sampled.iter().map(|t| (t.index, vk::DescriptorType::SAMPLED_IMAGE)));
        let sampler_base = (layout.textures.len() + layout.sampled.len()) as u32;
        bindings.extend((0..layout.samplers.len() as u32).map(|i| (sampler_base + i, vk::DescriptorType::SAMPLER)));
        let block = layout.desc.param_binding != u32::MAX && layout.desc.push_size != 0;
        if block { bindings.push((layout.desc.param_binding, vk::DescriptorType::STORAGE_BUFFER)); }
        bindings.sort_by_key(|b| b.0);
        if bindings.windows(2).any(|b| b[0].0 == b[1].0) { return Err("overlapping Vulkan kernel descriptor bindings".into()); }
        let limits = &dev.properties.limits;
        let count = |ty: vk::DescriptorType| bindings.iter().filter(|b| b.1 == ty).count() as u32;
        for (ty, max, what) in [(vk::DescriptorType::STORAGE_IMAGE, limits.max_per_stage_descriptor_storage_images, "storage images"),
            (vk::DescriptorType::SAMPLED_IMAGE, limits.max_per_stage_descriptor_sampled_images, "sampled images"),
            (vk::DescriptorType::SAMPLER, limits.max_per_stage_descriptor_samplers, "samplers")] {
            if count(ty) > max { return Err(format!("the kernel binds {} {what}, the device allows {max}", count(ty))); }
        }
        if bindings.len() as u32 > limits.max_per_stage_resources { return Err(format!("the kernel binds {} resources, the device allows {}", bindings.len(), limits.max_per_stage_resources)); }
        let set: Vec<_> = bindings.iter().map(|(binding, ty)| vk::DescriptorSetLayoutBinding::default().binding(*binding)
            .descriptor_type(*ty).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE)).collect();
        let mut p = Self { dev, layout, pipeline: vk::Pipeline::null(), pipeline_layout: vk::PipelineLayout::null(), descriptors: vk::DescriptorSetLayout::null(), bindings, samplers: Vec::new() };
        for s in &p.layout.samplers {
            p.samplers.push(super::pipeline::sampler(&p.dev, &crate::spirv::SpirvSampler { filter: s.filter, address: s.address, compare: false, pixel: false, anisotropy: 1 })?);
        }
        unsafe {
            p.descriptors = p.dev.raw.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&set), None).map_err(error)?;
            let sets = [p.descriptors];
            // params in push constants (at most the 128 bytes every device has), else in the block
            let ranges: Vec<_> = (p.layout.desc.push_size != 0 && !block).then(|| vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::COMPUTE).size(p.layout.desc.push_size)).into_iter().collect();
            p.pipeline_layout = p.dev.raw.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&sets).push_constant_ranges(&ranges), None).map_err(error)?;
            let module = p.dev.raw.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(code), None).map_err(error)?;
            let stage = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(module).name(c"kernel_main");
            let result = p.dev.raw.create_compute_pipelines(vk::PipelineCache::null(), &[vk::ComputePipelineCreateInfo::default().stage(stage).layout(p.pipeline_layout)], None).map_err(error);
            p.dev.raw.destroy_shader_module(module, None);
            p.pipeline = result?[0];
        }
        Ok(Arc::new(p))
    }
}
/// `rank`: the compile's place in the queue (`Compilers`), lowered once a pass waits for it.
pub(super) struct Pipe { pub gen: u32, pub state: Arc<OnceLock<Result<Arc<Program>, String>>>, pub announced: bool, pub rank: Arc<std::sync::atomic::AtomicU8> }
pub(super) struct Descriptors { dev: Arc<Device>, pool: vk::DescriptorPool }
impl Drop for Descriptors { fn drop(&mut self) { unsafe { self.dev.raw.destroy_descriptor_pool(self.pool, None); } } }

pub(super) fn usage(kind: u16, compute: bool) -> vk::BufferUsageFlags {
    let mut usage = super::resources::buffer_usage();
    if kind & buffer_kind::KIND_MASK == buffer_kind::STORAGE && compute { usage |= vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS | vk::BufferUsageFlags::STORAGE_BUFFER; }
    if kind & buffer_kind::INDIRECT != 0 { usage |= vk::BufferUsageFlags::INDIRECT_BUFFER; }
    usage
}
pub(super) fn barrier(batch: &Batch) { unsafe {
    batch.dev.raw.cmd_pipeline_barrier(batch.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::ALL_COMMANDS,
        vk::DependencyFlags::empty(), &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::MEMORY_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)], &[], &[]);
} }

impl Vulkan {
    pub(super) fn compute_ready(&self, sid: u32, passes: &[Pass], view: &CmdView) -> bool {
        for pass in passes.iter().filter(|p| p.kind == pass_kind::COMPUTE) {
            let Some(dispatches) = compute::records::<Dispatch>(view.arena, pass.dispatches) else { return false; };
            for d in dispatches {
                let Some(p) = self.compute.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { return false; };
                if !matches!(p.state.get(), Some(Ok(_))) { super::Compilers::waited(&p.rank); return false; }
            }
        }
        true
    }

    pub(super) fn create_compute(&mut self, s: &mut Session, id: Id, blob: &PipelineBlob) -> PipeStatus {
        if !self.dev.compute { return PipeStatus::Failed("Vulkan compute needs bufferDeviceAddress".into()); }
        let layout = match KernelLayout::parse(blob) { Ok(l) => l, Err(e) => return PipeStatus::Failed(e) };
        if let Err(e) = layout.validate_device(&self.info) { return PipeStatus::Failed(e); }
        if blob.code.len() % 4 != 0 { return PipeStatus::Failed("invalid SPIR-V bytes".into()); }
        let code: Vec<u32> = blob.code.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        let state = Arc::new(OnceLock::new());
        let rank = super::Compilers::rank(super::compile_rank::KERNEL);
        self.compute.insert((s.id, id.index), Pipe { gen: id.gen, state: state.clone(), announced: false, rank: rank.clone() });
        let dev = self.dev.clone(); let notify = self.mailbox.notify.clone(); let sid = s.id;
        self.compiler.push(rank, move || { let _ = state.set(Program::new(dev, layout, &code)); notify(sid, false); });
        PipeStatus::Pending
    }
    pub(super) fn announce_compute(&mut self, world: &mut World) {
        for ((sid, index), pipe) in &mut self.compute {
            if pipe.announced { continue; }
            let Some(result) = pipe.state.get() else { continue; };
            let Some(s) = world.session_mut(*sid) else { continue; };
            pipe.announced = true;
            s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
            let id = Id::new(*index, pipe.gen);
            match result { Ok(_) => s.events.pipeline_ready(id), Err(e) => s.events.pipeline_failed(id, e.as_bytes()) }
        }
    }
    pub(super) fn encode_compute(&mut self, s: &mut Session, pass: &Pass, view: &CmdView, batch: &mut Batch) -> Result<(), String> {
        let dispatches = compute::records::<Dispatch>(view.arena, pass.dispatches).ok_or("invalid dispatch records")?;
        for d in &dispatches {
            let pipe = self.compute.get(&(s.id, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen).ok_or("stale compute pipeline")?;
            let p = pipe.state.get().ok_or("compute pipeline pending")?.as_ref().map_err(|e| e.clone())?.clone();
            p.layout.validate_dispatch(s, d, view.arena)?;
            let params = compute::bytes(view.arena, d.params).ok_or("invalid params")?;
            let ids = compute::ids(view.arena, d.buffers).ok_or("invalid buffers")?;
            let textures = compute::ids(view.arena, d.textures).ok_or("invalid images")?;
            let sampled = compute::ids(view.arena, d.sampled).ok_or("invalid sampled textures")?;
            if params.len() != p.layout.desc.push_size as usize || ids.len() != p.layout.buffers.len() || textures.len() != p.layout.textures.len()
                || sampled.len() != p.layout.sampled.len() { return Err("compute resource count mismatch".into()); }
            let mut params = params.to_vec();
            for (id, binding) in ids.iter().zip(&p.layout.buffers) {
                let buffer = self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).ok_or("stale compute buffer")?.buffer.clone();
                if !s.buffers.get(s.id, *id).is_ok_and(|b| b.val.kind & buffer_kind::KIND_MASK == buffer_kind::STORAGE) { return Err("compute buffer lacks STORAGE".into()); }
                let address = unsafe { self.dev.raw.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buffer.buffer)) };
                let off = binding.offset as usize;
                params[off..off + 8].copy_from_slice(&address.to_le_bytes());
                batch.buffers.push(buffer);
            }
            unsafe { self.dev.raw.cmd_bind_pipeline(batch.cb, vk::PipelineBindPoint::COMPUTE, p.pipeline); }
            let block = p.layout.desc.param_binding != u32::MAX && !params.is_empty();
            if !p.bindings.is_empty() {
                let mut images = Vec::new();
                for (id, binding) in textures.iter().zip(&p.layout.textures) {
                    let image = self.resolve(s, *id).ok_or("stale storage image")?;
                    if image.desc.format != binding.format || image.desc.usage & crate::usage::STORAGE == 0 { return Err("storage image format/usage mismatch".into()); }
                    image.transition(batch.cb, vk::ImageLayout::GENERAL);
                    images.push((binding.index, vk::DescriptorType::STORAGE_IMAGE, vk::DescriptorImageInfo::default().image_view(image.view).image_layout(vk::ImageLayout::GENERAL)));
                    batch.images.push(image);
                }
                // what a draw samples is readied the same way: its layout changes only after a
                // write, and that barrier orders the write before this dispatch's reads
                for (id, binding) in sampled.iter().zip(&p.layout.sampled) {
                    let image = self.resolve(s, *id).ok_or("missing sampled texture")?;
                    let view = image.sample_view(0)?;
                    image.to_sampled(batch.cb);
                    images.push((binding.index, vk::DescriptorType::SAMPLED_IMAGE, vk::DescriptorImageInfo::default().image_view(view).image_layout(image.sampled_layout())));
                    batch.images.push(image);
                }
                let sampler_base = (p.layout.textures.len() + p.layout.sampled.len()) as u32;
                for (i, sampler) in p.samplers.iter().enumerate() {
                    images.push((sampler_base + i as u32, vk::DescriptorType::SAMPLER, vk::DescriptorImageInfo::default().sampler(*sampler)));
                }
                let mut blocks = Vec::new();
                if block {
                    let placed = batch.put(&params)?;
                    blocks.push((p.layout.desc.param_binding, vk::DescriptorBufferInfo::default().buffer(placed.buffer).offset(placed.at).range(params.len() as u64)));
                }
                let mut sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
                for (_, ty) in &p.bindings {
                    match sizes.iter_mut().find(|s| s.ty == *ty) { Some(s) => s.descriptor_count += 1, None => sizes.push(vk::DescriptorPoolSize::default().ty(*ty).descriptor_count(1)) }
                }
                let pool = unsafe { self.dev.raw.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&sizes), None) }.map_err(error)?;
                let pool = Descriptors { dev: self.dev.clone(), pool };
                let layouts = [p.descriptors];
                let sets = unsafe { self.dev.raw.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool.pool).set_layouts(&layouts)) }.map_err(error)?;
                let mut writes: Vec<_> = images.iter().map(|(binding, ty, info)| vk::WriteDescriptorSet::default().dst_set(sets[0]).dst_binding(*binding)
                    .descriptor_type(*ty).image_info(std::slice::from_ref(info))).collect();
                writes.extend(blocks.iter().map(|(binding, info)| vk::WriteDescriptorSet::default().dst_set(sets[0]).dst_binding(*binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(std::slice::from_ref(info))));
                unsafe { self.dev.raw.update_descriptor_sets(&writes, &[]); self.dev.raw.cmd_bind_descriptor_sets(batch.cb, vk::PipelineBindPoint::COMPUTE, p.pipeline_layout, 0, &sets, &[]); }
                batch.compute_descriptors.push(pool);
            }
            let timer = if d.timer != 0 {
                let q = batch.dispatch_query;
                batch.dispatch_query += 2;
                batch.dispatch_times.push((Self::token(s), s.string(d.timer).unwrap_or("compute").to_string(), q));
                batch.stamp(q);
                Some(q)
            } else { None };
            unsafe {
                if !params.is_empty() && !block { self.dev.raw.cmd_push_constants(batch.cb, p.pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, &params); }
                if d.indirect_buffer.is_none() { self.dev.raw.cmd_dispatch(batch.cb, d.groups[0], d.groups[1], d.groups[2]); }
                else {
                    let buffer = self.buf.get(&(s.id, d.indirect_buffer.index)).filter(|b| b.gen == d.indirect_buffer.gen).ok_or("stale indirect buffer")?.buffer.clone();
                    if d.indirect_offset % 4 != 0 || d.indirect_offset.checked_add(12).is_none_or(|end| end > buffer.size) { return Err("invalid indirect offset".into()); }
                    self.dev.raw.cmd_dispatch_indirect(batch.cb, buffer.buffer, d.indirect_offset); batch.buffers.push(buffer);
                }
            }
            barrier(batch);
            if let Some(q) = timer { batch.stamp(q + 1); }
            batch.compute_pipelines.push(p);
        }
        Ok(())
    }
    pub(super) fn read_compute_buffers(&mut self, s: &mut Session, req: u32, ranges: &[BufferReadRange]) {
        let result = (|| {
            let mut reads = Vec::with_capacity(ranges.len());
            let dev = self.dev.clone();
            let align = dev.properties.limits.non_coherent_atom_size.max(256);
            // Reserve the entire request before encoding any copy.
            for (order, range) in ranges.iter().enumerate() {
                let source = self.buf.get(&(s.id, range.id.index)).filter(|b| b.gen == range.id.gen).ok_or("stale buffer")?.buffer.clone();
                let (block, offset) = self.leases.alloc(range.len, align, |size| Buffer::scratch(dev.clone(), crate::ledger::Pool::Leases, size, vk::BufferUsageFlags::TRANSFER_DST))?;
                let len = usize::try_from(range.len).map_err(|_| "lease exceeds address space")?;
                // SAFETY: the guard retains the mapped block; the range is disjoint from all
                // live allocations and is delivered only after the transfer fence and invalidate.
                let lease = unsafe { crate::lease::Lease::new(block.clone(), block.lease_ptr(offset), len, order as u32) }.in_block(block.lease_ptr(0), block.size);
                reads.push((source, range.offset, block, offset, range.len, lease));
            }
            let batch = self.ensure_batch()?;
            barrier(batch);
            let mut completed = Vec::with_capacity(reads.len());
            for (source, source_offset, block, offset, len, lease) in reads {
                if len != 0 { unsafe { batch.dev.raw.cmd_copy_buffer(batch.cb, source.buffer, block.buffer,
                    &[vk::BufferCopy { src_offset: source_offset, dst_offset: offset, size: len }]); } }
                batch.buffers.push(source);
                completed.push((block, offset, len, lease));
            }
            unsafe { batch.dev.raw.cmd_pipeline_barrier(batch.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ)], &[], &[]); }
            batch.tickets.push(Ticket::BufferLeases { token: Self::token(s), req, ranges: completed });
            Ok::<_, String>(())
        })();
        if let Err(e) = result { eprintln!("[hal-vulkan] buffer readback: {e}"); s.events.buffer_leases(req, readback::FAILED, Vec::new()); }
    }

}
