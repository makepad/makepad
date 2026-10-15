//! A GPU-only image transfer between two Vulkan devices. Each endpoint owns
//! its command pool and fences; neither device ever records the other's image
//! handle. DMA-BUF layouts are negotiated, not inferred from vendor names.
//!
//! Unsafe operations require the exact two live logical devices used by new,
//! with external queue synchronization held by the caller. Local images must
//! belong to the named device, match the negotiated BGRA8 extent, and support
//! TRANSFER_SRC (send) or TRANSFER_DST (receive). Send requires a successful
//! writable check since the previous handoff; destroy requires both devices
//! idle. No image or device referenced here may be destroyed in flight.
use super::dma_buf;
use crate::backend::vulkan::Device;
use makepad_vulkan::vk;

struct Endpoint {
    image: Option<dma_buf::Image>,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    wait: vk::Semaphore,
    signal: vk::Semaphore,
    submitted: std::cell::Cell<bool>,
}

impl Endpoint {
    fn new(gpu: &Device, image: dma_buf::Image) -> Result<Self, String> {
        gpu.charge(image.layout.allocation_size);
        let mut endpoint = Self {
            image: Some(image),
            pool: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            wait: vk::Semaphore::null(),
            signal: vk::Semaphore::null(),
            submitted: std::cell::Cell::new(false),
        };
        let result = (|| unsafe {
            endpoint.pool = gpu
                .raw
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(gpu.family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(|e| format!("create GPU bridge command pool: {e:?}"))?;
            endpoint.command = gpu
                .raw
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(endpoint.pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(|e| format!("allocate GPU bridge command buffer: {e:?}"))?[0];
            endpoint.fence = gpu
                .raw
                .create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
                .map_err(|e| format!("create GPU bridge fence: {e:?}"))?;
            endpoint.wait = dma_buf::create_semaphore(&gpu.raw)?;
            endpoint.signal = dma_buf::create_semaphore(&gpu.raw)?;
            Ok(())
        })();
        if let Err(error) = result {
            endpoint.destroy(gpu);
            return Err(error);
        }
        Ok(endpoint)
    }

    fn idle(&self, gpu: &Device) -> Result<bool, String> {
        if !self.submitted.get() {
            return Ok(true);
        }
        unsafe { gpu.raw.get_fence_status(self.fence) }
            .map_err(|e| format!("poll GPU bridge fence: {e:?}"))
    }

    fn begin(&self, gpu: &Device) -> Result<(), String> {
        unsafe {
            gpu.raw
                .reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset GPU bridge command buffer: {e:?}"))?;
            gpu.raw
                .begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin GPU bridge command buffer: {e:?}"))
        }
    }

    fn submit(&self, gpu: &Device, wait: bool) -> Result<dma_buf::SyncFile, String> {
        self.submit_commands(gpu, wait)?;
        unsafe { dma_buf::export_sync_file(&gpu.raw, self.signal) }
    }

    fn submit_commands(&self, gpu: &Device, wait: bool) -> Result<(), String> {
        let _queue = super::super::lock(&gpu.queue_lock);
        unsafe {
            gpu.raw
                .end_command_buffer(self.command)
                .map_err(|e| format!("end GPU bridge command buffer: {e:?}"))?;
            self.submitted.set(false);
            gpu.raw
                .reset_fences(&[self.fence])
                .map_err(|e| format!("reset GPU bridge fence: {e:?}"))?;
            let commands = [self.command];
            let signals = [self.signal];
            let waits = [self.wait];
            let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
            let mut submit = vk::SubmitInfo::default()
                .command_buffers(&commands)
                .signal_semaphores(&signals);
            if wait {
                submit = submit.wait_semaphores(&waits).wait_dst_stage_mask(&stages);
            }
            gpu.raw
                .queue_submit(gpu.queue, &[submit], self.fence)
                .map_err(|e| format!("submit GPU bridge: {e:?}"))?;
            self.submitted.set(true);
            Ok(())
        }
    }

    // The caller has retired both devices' use before destruction.
    fn destroy(mut self, gpu: &Device) {
        unsafe {
            if self.pool != vk::CommandPool::null() {
                gpu.raw.destroy_command_pool(self.pool, None);
            }
            if self.fence != vk::Fence::null() {
                gpu.raw.destroy_fence(self.fence, None);
            }
            if self.wait != vk::Semaphore::null() {
                gpu.raw.destroy_semaphore(self.wait, None);
            }
            if self.signal != vk::Semaphore::null() {
                gpu.raw.destroy_semaphore(self.signal, None);
            }
            if let Some(image) = self.image.take() {
                gpu.uncharge(image.layout.allocation_size);
                image.destroy(&gpu.raw);
            }
        }
    }
}

enum Handoff {
    /// Initialization is owned before exporting its completion descriptor, so
    /// an export failure can retire asynchronously without blocking a caller.
    Initializing { owner_is_source: bool },
    /// An owner submitted the release making the source's next write legal.
    Write {
        sync: dma_buf::SyncFile,
        owner_is_source: bool,
    },
    /// Source finished a frame; destination has not consumed it yet.
    Read(dma_buf::SyncFile),
    /// Only during an operation, or after an error. Never reuse a failed slot.
    Failed(String),
}

pub(super) struct GpuBridge {
    source: Endpoint,
    destination: Endpoint,
    extent: vk::Extent2D,
    handoff: Handoff,
}

impl GpuBridge {
    pub fn new(
        source: &Device,
        destination: &Device,
        extent: vk::Extent2D,
    ) -> Result<Self, String> {
        let usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        let source_caps =
            unsafe { dma_buf::modifiers(&source.instance, source.physical, usage) }?;
        let destination_caps = unsafe {
            dma_buf::modifiers(&destination.instance, destination.physical, usage)
        }?;
        let mut errors = Vec::new();
        // Try both allocation directions. A GPU may import a layout it cannot
        // export; the compositor GPU does not have to allocate the bridge.
        for owner_is_source in [false, true] {
            let (owner, reader, owner_caps, reader_caps) = if owner_is_source {
                (source, destination, &source_caps, &destination_caps)
            } else {
                (destination, source, &destination_caps, &source_caps)
            };
            for cap in owner_caps {
                if !cap
                    .external
                    .external_memory_features
                    .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
                {
                    continue;
                }
                if !reader_caps.iter().any(|other| {
                    other.modifier == cap.modifier
                        && other
                            .external
                            .external_memory_features
                            .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
                }) {
                    continue;
                }
                let (allocated, fd) = match unsafe {
                    dma_buf::allocate(
                        &owner.instance,
                        &owner.raw,
                        owner.physical,
                        extent.width,
                        extent.height,
                        cap.modifier,
                        usage,
                        0,
                        None,
                    )
                } {
                    Ok(pair) => pair,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
                let reader_layout = match unsafe {
                    dma_buf::import_requirements(
                        &reader.instance,
                        &reader.raw,
                        reader.physical,
                        allocated.layout,
                        usage,
                    )
                } {
                    Ok(layout) => layout,
                    Err(error) => {
                        unsafe { allocated.destroy(&owner.raw) };
                        drop(fd);
                        errors.push(error);
                        continue;
                    }
                };
                let plane_changed = reader_layout.offset != allocated.layout.offset
                    || reader_layout.row_pitch != allocated.layout.row_pitch;
                if plane_changed && cap.modifier != 0 {
                    unsafe { allocated.destroy(&owner.raw) };
                    drop(fd);
                    errors.push("importer changed a non-linear DMA-BUF plane layout".into());
                    continue;
                }
                // Allocation padding cannot repair a differing row pitch. For
                // linear images, ask the exporter to use the reader's reported
                // layout too, then verify both images before publishing either.
                // The retry is bounded; no alignment is guessed from a vendor.
                let reader_minimum = reader_layout.allocation_size;
                let (allocated, fd) = if plane_changed
                    || reader_minimum > allocated.layout.allocation_size
                {
                    let old_bytes = allocated.layout.allocation_size;
                    eprintln!(
                        "Vulkan GPU bridge: negotiating DMA-BUF {} -> {} bytes, {}x{}, offset {} -> {}, pitch {} -> {}",
                        old_bytes,
                        reader_minimum,
                        extent.width,
                        extent.height,
                        allocated.layout.offset,
                        reader_layout.offset,
                        allocated.layout.row_pitch,
                        reader_layout.row_pitch
                    );
                    unsafe { allocated.destroy(&owner.raw) };
                    drop(fd);
                    match unsafe {
                        dma_buf::allocate(
                            &owner.instance,
                            &owner.raw,
                            owner.physical,
                            extent.width,
                            extent.height,
                            cap.modifier,
                            usage,
                            reader_minimum,
                            plane_changed.then_some(reader_layout),
                        )
                    } {
                        Ok(pair) => pair,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    }
                } else {
                    (allocated, fd)
                };
                let imported = match unsafe {
                    dma_buf::import(
                        &reader.instance,
                        &reader.raw,
                        reader.physical,
                        allocated.layout,
                        usage,
                        fd,
                    )
                } {
                    Ok(image) => image,
                    Err(error) => {
                        unsafe { allocated.destroy(&owner.raw) };
                        errors.push(error);
                        continue;
                    }
                };
                let (source_image, destination_image) = if owner_is_source {
                    (allocated, imported)
                } else {
                    (imported, allocated)
                };
                let source_endpoint = match Endpoint::new(source, source_image) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        unsafe { destination_image.destroy(&destination.raw) };
                        return Err(error);
                    }
                };
                let destination_endpoint = match Endpoint::new(destination, destination_image) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        source_endpoint.destroy(source);
                        return Err(error);
                    }
                };
                let mut bridge = Self {
                    source: source_endpoint,
                    destination: destination_endpoint,
                    extent,
                    handoff: Handoff::Failed("bridge has not been initialized".into()),
                };
                let endpoint = if owner_is_source {
                    &bridge.source
                } else {
                    &bridge.destination
                };
                let initialized = (|| {
                    endpoint.begin(owner)?;
                    unsafe {
                        dma_buf::ownership_barrier(
                            &owner.raw,
                            endpoint.command,
                            endpoint.image.as_ref().unwrap().image,
                            owner.family,
                            false,
                            vk::ImageLayout::UNDEFINED,
                            vk::PipelineStageFlags::TOP_OF_PIPE,
                            vk::AccessFlags::empty(),
                        );
                    }
                    endpoint.submit_commands(owner, false)
                })();
                match initialized {
                    Ok(()) => bridge.handoff = Handoff::Initializing { owner_is_source },
                    Err(error) => {
                        // No queue submission succeeded; there is no pending
                        // initialization work to wait for on this path.
                        unsafe { bridge.destroy(source, destination) };
                        return Err(error);
                    }
                }
                let source_id =
                    unsafe { dma_buf::identity(&source.instance, source.physical) };
                let destination_id = unsafe {
                    dma_buf::identity(&destination.instance, destination.physical)
                };
                eprintln!("Vulkan GPU bridge: {:?} -> {:?}, {}x{}, fourcc={:#x} modifier={:#x} features={:?}, allocator={}, GPU transfers + SYNC_FD",
                    source_id, destination_id, extent.width, extent.height,
                    dma_buf::DRM_ARGB8888, cap.modifier, cap.features,
                    if owner_is_source { "source" } else { "destination" });
                return Ok(bridge);
            }
        }
        Err(format!(
            "No usable cross-GPU DMA-BUF layout: {}",
            errors.join("; ")
        ))
    }

    /// CPU polling only checks completion. The imported SYNC_FD still creates
    /// the cross-driver GPU memory dependency; a fence poll cannot replace it.
    pub unsafe fn writable(
        &self,
        source: &Device,
        destination: &Device,
    ) -> Result<bool, String> {
        if let Handoff::Failed(error) = &self.handoff {
            return Err(error.clone());
        }
        let Handoff::Write {
            owner_is_source, ..
        } = self.handoff
        else {
            return Ok(false);
        };
        let owner_done = if owner_is_source {
            self.source.idle(source)?
        } else {
            self.destination.idle(destination)?
        };
        Ok(owner_done && self.source.idle(source)?)
    }

    /// The size of the rectangle each send carries.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    pub fn pending(&self) -> bool {
        matches!(self.handoff, Handoff::Read(_))
    }

    pub unsafe fn poll_initialization(
        &mut self,
        source: &Device,
        destination: &Device,
    ) -> Result<bool, String> {
        let Handoff::Initializing { owner_is_source } = self.handoff else {
            return match &self.handoff {
                Handoff::Failed(error) => Err(error.clone()),
                _ => Ok(true),
            };
        };
        let (owner, endpoint) = if owner_is_source {
            (source, &self.source)
        } else {
            (destination, &self.destination)
        };
        if !endpoint.idle(owner)? {
            return Ok(false);
        }
        match unsafe { dma_buf::export_sync_file(&owner.raw, endpoint.signal) }
        {
            Ok(sync) => {
                self.handoff = Handoff::Write {
                    sync,
                    owner_is_source,
                };
                Ok(true)
            }
            Err(error) => self.fail(error),
        }
    }

    /// Includes initialization and failed handoffs. A reset fence belonging
    /// to a rejected queue submission is not evidence of outstanding work.
    pub unsafe fn idle(&self, source: &Device, destination: &Device) -> Result<bool, String> {
        Ok(self.source.idle(source)? && self.destination.idle(destination)?)
    }

    /// Record a whole display image into the shared byte carrier. The callback
    /// runs with the carrier acquired locally in GENERAL and must write its
    /// full extent. It owns synchronization/layouts of its other resources.
    /// All referenced resources must survive completion of both endpoints.
    pub unsafe fn send_commands(
        &mut self,
        source: &Device,
        record: impl FnOnce(vk::CommandBuffer, vk::Image) -> Result<(), String>,
    ) -> Result<(), String> {
        let Handoff::Write { sync, .. } = std::mem::replace(
            &mut self.handoff,
            Handoff::Failed("incomplete GPU display handoff".into()),
        ) else {
            return Err("GPU bridge is not available for a whole display image".into());
        };
        let result = self
            .record_transfer(source, &self.source, sync, true, record)
            .and_then(|()| self.source.submit(source, true));
        match result {
            Ok(sync) => {
                self.handoff = Handoff::Read(sync);
                Ok(())
            }
            Err(error) => self.fail(error),
        }
    }

    /// Consume one whole display image with a GPU-only callback. `true` means
    /// submitted, not completed; call `writable` before reusing local scratch
    /// storage, committing migrated resources, or destroying the bridge.
    pub unsafe fn receive_commands(
        &mut self,
        source: &Device,
        destination: &Device,
        record: impl FnOnce(vk::CommandBuffer, vk::Image) -> Result<(), String>,
    ) -> Result<bool, String> {
        if let Handoff::Failed(error) = &self.handoff {
            return Err(error.clone());
        }
        if !self.pending() || !self.source.idle(source)? || !self.destination.idle(destination)? {
            return Ok(false);
        }
        let Handoff::Read(sync) = std::mem::replace(
            &mut self.handoff,
            Handoff::Failed("incomplete GPU display handoff".into()),
        ) else {
            unreachable!()
        };
        let result = self
            .record_transfer(destination, &self.destination, sync, false, record)
            .and_then(|()| self.destination.submit(destination, true));
        match result {
            Ok(sync) => {
                self.handoff = Handoff::Write {
                    sync,
                    owner_is_source: false,
                };
                Ok(true)
            }
            Err(error) => self.fail(error),
        }
    }

    fn record_transfer(
        &self,
        gpu: &Device,
        endpoint: &Endpoint,
        sync: dma_buf::SyncFile,
        write_bridge: bool,
        record: impl FnOnce(vk::CommandBuffer, vk::Image) -> Result<(), String>,
    ) -> Result<(), String> {
        unsafe { dma_buf::import_sync_file(&gpu.raw, endpoint.wait, sync) }?;
        endpoint.begin(gpu)?;
        let bridge_image = endpoint.image.as_ref().unwrap().image;
        let access = if write_bridge {
            vk::AccessFlags::TRANSFER_WRITE
        } else {
            vk::AccessFlags::TRANSFER_READ
        };
        unsafe {
            dma_buf::ownership_barrier(
                &gpu.raw,
                endpoint.command,
                bridge_image,
                gpu.family,
                true,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                access,
            );
        }
        record(endpoint.command, bridge_image)?;
        unsafe {
            dma_buf::ownership_barrier(
                &gpu.raw,
                endpoint.command,
                bridge_image,
                gpu.family,
                false,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                access,
            );
        }
        Ok(())
    }

    fn fail<T>(&mut self, error: String) -> Result<T, String> {
        self.handoff = Handoff::Failed(error.clone());
        Err(error)
    }

    /// Both devices must have finished every submission using this bridge.
    pub unsafe fn destroy(self, source: &Device, destination: &Device) {
        self.source.destroy(source);
        self.destination.destroy(destination);
    }
}

/// `cargo test --release -p makepad-hal --lib gpu_bridge -- --ignored --nocapture` on a machine
/// with two GPUs (a hybrid laptop): times each way of moving a 2560x1600 frame across.
#[cfg(test)]
mod bench {
    use super::*;
    use crate::backend::vulkan::{Image, NativeImages, Offscreen, VkExternalImages};
    use crate::{session::TextureObj, types::{format, usage}};
    use std::sync::Arc;
    use std::time::Instant;

    fn range() -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)
    }
    fn barrier(dev: &Device, cb: vk::CommandBuffer, image: vk::Image, old: vk::ImageLayout, new: vk::ImageLayout) {
        let b = vk::ImageMemoryBarrier::default().image(image).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE).dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
            .subresource_range(range());
        unsafe { dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(), &[], &[], &[b]); }
    }
    #[derive(Clone, Copy, Debug)]
    enum Way { ClearBlit, Blit, Copy }
    fn record(dev: &Device, cb: vk::CommandBuffer, way: Way, src: vk::Image, src_layout: vk::ImageLayout,
        dst: vk::Image, dst_layout: vk::ImageLayout, w: u32, h: u32) {
        let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        unsafe {
            if let Way::ClearBlit = way {
                dev.raw.cmd_clear_color_image(cb, dst, dst_layout, &vk::ClearColorValue { float32: [0.0; 4] }, &[range()]);
                barrier(dev, cb, dst, dst_layout, dst_layout);
            }
            match way {
                Way::ClearBlit | Way::Blit => {
                    let o = [vk::Offset3D::default(), vk::Offset3D { x: w as i32, y: h as i32, z: 1 }];
                    let blit = vk::ImageBlit::default().src_subresource(sub).dst_subresource(sub).src_offsets(o).dst_offsets(o);
                    dev.raw.cmd_blit_image(cb, src, src_layout, dst, dst_layout, &[blit], vk::Filter::NEAREST);
                }
                Way::Copy => {
                    let copy = vk::ImageCopy::default().src_subresource(sub).dst_subresource(sub).extent(vk::Extent3D { width: w, height: h, depth: 1 });
                    dev.raw.cmd_copy_image(cb, src, src_layout, dst, dst_layout, &[copy]);
                }
            }
        }
    }
    fn wait(dev: &Device, endpoint: &Endpoint) {
        unsafe { dev.raw.wait_for_fences(&[endpoint.fence], true, u64::MAX).unwrap(); }
    }
    fn image(dev: &Arc<Device>, w: u32, h: u32) -> Arc<Image> {
        Image::new(dev.clone(), TextureObj { format: format::BGRA8, w, h, depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED,
            bytes: 0, imported: false, uploaded: 0, lent: None }).unwrap()
    }
    fn one_shot(dev: &Device, f: impl FnOnce(vk::CommandBuffer)) {
        unsafe {
            let pool = dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family), None).unwrap();
            let cb = dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1)).unwrap()[0];
            dev.raw.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default()).unwrap();
            f(cb);
            dev.raw.end_command_buffer(cb).unwrap();
            let cbs = [cb];
            dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], vk::Fence::null()).unwrap();
            dev.raw.queue_wait_idle(dev.queue).unwrap();
            dev.raw.destroy_command_pool(pool, None);
        }
    }

    #[test]
    #[ignore]
    fn cross_gpu_frame() {
        let (w, h) = (2560, 1600);
        let source = Device::shared(&[], &NativeImages::default().device_extensions(), &mut Offscreen).unwrap();
        let physical = unsafe { source.instance.enumerate_physical_devices() }.unwrap();
        let Some(other) = physical.into_iter().find(|p| *p != source.physical) else { eprintln!("one GPU: nothing to measure"); return };
        let destination = Device::display_peer(source.clone(), other).unwrap();
        let name = |d: &Device| unsafe { std::ffi::CStr::from_ptr(d.properties.device_name.as_ptr()) }.to_string_lossy().into_owned();
        eprintln!("source {} -> destination {}", name(&source), name(&destination));
        let frame = image(&source, w, h);
        let local = image(&destination, w, h);
        one_shot(&source, |cb| barrier(&source, cb, frame.image, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_SRC_OPTIMAL));
        one_shot(&destination, |cb| barrier(&destination, cb, local.image, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL));
        let mut bridge = GpuBridge::new(&source, &destination, vk::Extent2D { width: w, height: h }).unwrap();
        unsafe {
            while !bridge.poll_initialization(&source, &destination).unwrap() {}
            for (send_way, receive_way, gap_ms) in [(Way::ClearBlit, Way::ClearBlit, 0), (Way::Blit, Way::Blit, 0), (Way::Copy, Way::Copy, 0),
                (Way::ClearBlit, Way::ClearBlit, 40), (Way::Copy, Way::Copy, 40), (Way::Copy, Way::Copy, 200)] {
                let (mut send, mut receive) = (Vec::new(), Vec::new());
                for _ in 0..40 {
                    std::thread::sleep(std::time::Duration::from_millis(gap_ms));
                    while !bridge.writable(&source, &destination).unwrap() {}
                    let t = Instant::now();
                    bridge.send_commands(&source, |cb, shared| {
                        record(&source, cb, send_way, frame.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, shared, vk::ImageLayout::GENERAL, w, h); Ok(())
                    }).unwrap();
                    wait(&source, &bridge.source);
                    send.push(t.elapsed().as_secs_f64() * 1e3);
                    let t = Instant::now();
                    assert!(bridge.receive_commands(&source, &destination, |cb, shared| {
                        record(&destination, cb, receive_way, shared, vk::ImageLayout::GENERAL, local.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, w, h); Ok(())
                    }).unwrap());
                    wait(&destination, &bridge.destination);
                    receive.push(t.elapsed().as_secs_f64() * 1e3);
                }
                let stat = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); (v[v.len() / 2], v[v.len() * 9 / 10], v[v.len() - 1]) };
                eprintln!("{send_way:?}/{receive_way:?} gap {gap_ms} ms: send p50/p90/max {:.2?}  receive {:.2?} (ms)", stat(&mut send), stat(&mut receive));
            }
            source.raw.device_wait_idle().unwrap();
            destination.raw.device_wait_idle().unwrap();
            bridge.destroy(&source, &destination);
        }
    }
}
