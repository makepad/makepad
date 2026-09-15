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
use super::{dma_buf, CxVulkan};
use ash::vk;

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
    fn new(gpu: &CxVulkan, image: dma_buf::Image) -> Result<Self, String> {
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
                .device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(gpu.queue_family_index)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(|e| format!("create GPU bridge command pool: {e:?}"))?;
            endpoint.command = gpu
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(endpoint.pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(|e| format!("allocate GPU bridge command buffer: {e:?}"))?[0];
            endpoint.fence = gpu
                .device
                .create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
                .map_err(|e| format!("create GPU bridge fence: {e:?}"))?;
            endpoint.wait = dma_buf::create_semaphore(&gpu.device)?;
            endpoint.signal = dma_buf::create_semaphore(&gpu.device)?;
            Ok(())
        })();
        if let Err(error) = result {
            endpoint.destroy(gpu);
            return Err(error);
        }
        Ok(endpoint)
    }

    fn idle(&self, gpu: &CxVulkan) -> Result<bool, String> {
        if !self.submitted.get() {
            return Ok(true);
        }
        unsafe { gpu.device.get_fence_status(self.fence) }
            .map_err(|e| format!("poll GPU bridge fence: {e:?}"))
    }

    fn begin(&self, gpu: &CxVulkan) -> Result<(), String> {
        unsafe {
            gpu.device
                .reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset GPU bridge command buffer: {e:?}"))?;
            gpu.device
                .begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin GPU bridge command buffer: {e:?}"))
        }
    }

    fn submit(&self, gpu: &CxVulkan, wait: bool) -> Result<dma_buf::SyncFile, String> {
        self.submit_commands(gpu, wait)?;
        unsafe { dma_buf::export_sync_file(&gpu.instance, &gpu.device, self.signal) }
    }

    fn submit_commands(&self, gpu: &CxVulkan, wait: bool) -> Result<(), String> {
        unsafe {
            gpu.device
                .end_command_buffer(self.command)
                .map_err(|e| format!("end GPU bridge command buffer: {e:?}"))?;
            self.submitted.set(false);
            gpu.device
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
            gpu.device
                .queue_submit(gpu.queue, &[submit], self.fence)
                .map_err(|e| format!("submit GPU bridge: {e:?}"))?;
            self.submitted.set(true);
            Ok(())
        }
    }

    // The caller has retired both devices' use before destruction.
    fn destroy(mut self, gpu: &CxVulkan) {
        unsafe {
            if self.pool != vk::CommandPool::null() {
                gpu.device.destroy_command_pool(self.pool, None);
            }
            if self.fence != vk::Fence::null() {
                gpu.device.destroy_fence(self.fence, None);
            }
            if self.wait != vk::Semaphore::null() {
                gpu.device.destroy_semaphore(self.wait, None);
            }
            if self.signal != vk::Semaphore::null() {
                gpu.device.destroy_semaphore(self.signal, None);
            }
            if let Some(image) = self.image.take() {
                image.destroy(&gpu.device);
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
        source: &CxVulkan,
        destination: &CxVulkan,
        extent: vk::Extent2D,
    ) -> Result<Self, String> {
        let usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        let source_caps =
            unsafe { dma_buf::modifiers(&source.instance, source.physical_device, usage) }?;
        let destination_caps = unsafe {
            dma_buf::modifiers(&destination.instance, destination.physical_device, usage)
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
                        &owner.device,
                        owner.physical_device,
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
                        &reader.device,
                        reader.physical_device,
                        allocated.layout,
                        usage,
                    )
                } {
                    Ok(layout) => layout,
                    Err(error) => {
                        unsafe { allocated.destroy(&owner.device) };
                        drop(fd);
                        errors.push(error);
                        continue;
                    }
                };
                let plane_changed = reader_layout.offset != allocated.layout.offset
                    || reader_layout.row_pitch != allocated.layout.row_pitch;
                if plane_changed && cap.modifier != 0 {
                    unsafe { allocated.destroy(&owner.device) };
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
                    crate::log!(
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
                    unsafe { allocated.destroy(&owner.device) };
                    drop(fd);
                    match unsafe {
                        dma_buf::allocate(
                            &owner.instance,
                            &owner.device,
                            owner.physical_device,
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
                        &reader.device,
                        reader.physical_device,
                        allocated.layout,
                        usage,
                        fd,
                    )
                } {
                    Ok(image) => image,
                    Err(error) => {
                        unsafe { allocated.destroy(&owner.device) };
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
                        unsafe { destination_image.destroy(&destination.device) };
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
                            &owner.device,
                            endpoint.command,
                            endpoint.image.as_ref().unwrap().image,
                            owner.queue_family_index,
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
                    unsafe { dma_buf::identity(&source.instance, source.physical_device) };
                let destination_id = unsafe {
                    dma_buf::identity(&destination.instance, destination.physical_device)
                };
                crate::log!("Vulkan GPU bridge: {:?} -> {:?}, {}x{}, fourcc={:#x} modifier={:#x} features={:?}, allocator={}, GPU transfers + SYNC_FD",
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
        source: &CxVulkan,
        destination: &CxVulkan,
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

    pub fn pending(&self) -> bool {
        matches!(self.handoff, Handoff::Read(_))
    }

    pub unsafe fn poll_initialization(
        &mut self,
        source: &CxVulkan,
        destination: &CxVulkan,
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
        match unsafe { dma_buf::export_sync_file(&owner.instance, &owner.device, endpoint.signal) }
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
    pub unsafe fn idle(&self, source: &CxVulkan, destination: &CxVulkan) -> Result<bool, String> {
        Ok(self.source.idle(source)? && self.destination.idle(destination)?)
    }

    /// Record a migration chunk into the shared byte carrier. The callback
    /// runs with the carrier acquired locally in GENERAL and must write its
    /// full extent. It owns synchronization/layouts of its other resources.
    /// All referenced resources must survive completion of both endpoints.
    pub unsafe fn send_commands(
        &mut self,
        source: &CxVulkan,
        record: impl FnOnce(vk::CommandBuffer, vk::Image) -> Result<(), String>,
    ) -> Result<(), String> {
        let Handoff::Write { sync, .. } = std::mem::replace(
            &mut self.handoff,
            Handoff::Failed("incomplete GPU migration handoff".into()),
        ) else {
            return Err("GPU bridge is not available for a migration chunk".into());
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

    /// Consume one migration chunk with a GPU-only callback. `true` means
    /// submitted, not completed; call `writable` before reusing local scratch
    /// storage, committing migrated resources, or destroying the bridge.
    pub unsafe fn receive_commands(
        &mut self,
        source: &CxVulkan,
        destination: &CxVulkan,
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
            Handoff::Failed("incomplete GPU migration handoff".into()),
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
        gpu: &CxVulkan,
        endpoint: &Endpoint,
        sync: dma_buf::SyncFile,
        write_bridge: bool,
        record: impl FnOnce(vk::CommandBuffer, vk::Image) -> Result<(), String>,
    ) -> Result<(), String> {
        unsafe { dma_buf::import_sync_file(&gpu.instance, &gpu.device, endpoint.wait, sync) }?;
        endpoint.begin(gpu)?;
        let bridge_image = endpoint.image.as_ref().unwrap().image;
        let access = if write_bridge {
            vk::AccessFlags::TRANSFER_WRITE
        } else {
            vk::AccessFlags::TRANSFER_READ
        };
        unsafe {
            dma_buf::ownership_barrier(
                &gpu.device,
                endpoint.command,
                bridge_image,
                gpu.queue_family_index,
                true,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                access,
            );
        }
        record(endpoint.command, bridge_image)?;
        unsafe {
            dma_buf::ownership_barrier(
                &gpu.device,
                endpoint.command,
                bridge_image,
                gpu.queue_family_index,
                false,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                access,
            );
        }
        Ok(())
    }

    /// Source image is local to `source` and ends in SHADER_READ_ONLY_OPTIMAL.
    /// Caller checked writable and submitted its composition before this call.
    pub unsafe fn send(&mut self, source: &CxVulkan, local_image: vk::Image) -> Result<(), String> {
        let Handoff::Write { sync, .. } = std::mem::replace(
            &mut self.handoff,
            Handoff::Failed("incomplete GPU handoff".into()),
        ) else {
            return Err("GPU bridge is not available for a new frame".into());
        };
        let result = self
            .copy(source, &self.source, local_image, sync, true, true)
            .and_then(|()| self.source.submit(source, true));
        match result {
            Ok(sync) => {
                self.handoff = Handoff::Read(sync);
                Ok(())
            }
            Err(error) => self.fail(error),
        }
    }

    /// Copies the pending frame into a destination-local retained image. Its
    /// queue orders earlier display reads before this write, and later output
    /// blits after it. No dependency on a display's next vblank is introduced.
    pub unsafe fn receive(
        &mut self,
        source: &CxVulkan,
        destination: &CxVulkan,
        local_image: vk::Image,
        local_valid: bool,
    ) -> Result<bool, String> {
        if let Handoff::Failed(error) = &self.handoff {
            return Err(error.clone());
        }
        if !self.pending() || !self.source.idle(source)? || !self.destination.idle(destination)? {
            return Ok(false);
        }
        let Handoff::Read(sync) = std::mem::replace(
            &mut self.handoff,
            Handoff::Failed("incomplete GPU handoff".into()),
        ) else {
            unreachable!()
        };
        let result = self
            .copy(
                destination,
                &self.destination,
                local_image,
                sync,
                false,
                local_valid,
            )
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

    fn fail<T>(&mut self, error: String) -> Result<T, String> {
        self.handoff = Handoff::Failed(error.clone());
        Err(error)
    }

    fn copy(
        &self,
        gpu: &CxVulkan,
        endpoint: &Endpoint,
        local_image: vk::Image,
        sync: dma_buf::SyncFile,
        write_bridge: bool,
        local_valid: bool,
    ) -> Result<(), String> {
        unsafe { dma_buf::import_sync_file(&gpu.instance, &gpu.device, endpoint.wait, sync) }?;
        endpoint.begin(gpu)?;
        let bridge_image = endpoint.image.as_ref().unwrap().image;
        let bridge_access = if write_bridge {
            vk::AccessFlags::TRANSFER_WRITE
        } else {
            vk::AccessFlags::TRANSFER_READ
        };
        let local_layout = if write_bridge {
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL
        } else {
            vk::ImageLayout::TRANSFER_DST_OPTIMAL
        };
        let local_access = if write_bridge {
            vk::AccessFlags::TRANSFER_READ
        } else {
            vk::AccessFlags::TRANSFER_WRITE
        };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        unsafe {
            dma_buf::ownership_barrier(
                &gpu.device,
                endpoint.command,
                bridge_image,
                gpu.queue_family_index,
                true,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                bridge_access,
            );
            let barrier = vk::ImageMemoryBarrier::default()
                .image(local_image)
                .subresource_range(range)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .old_layout(if local_valid {
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                } else {
                    vk::ImageLayout::UNDEFINED
                })
                .new_layout(local_layout)
                .src_access_mask(if local_valid {
                    vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                } else {
                    vk::AccessFlags::empty()
                })
                .dst_access_mask(local_access);
            gpu.device.cmd_pipeline_barrier(
                endpoint.command,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            let region = vk::ImageCopy::default()
                .src_subresource(layers)
                .dst_subresource(layers)
                .extent(vk::Extent3D {
                    width: self.extent.width,
                    height: self.extent.height,
                    depth: 1,
                });
            let (source_image, source_layout, destination_image, destination_layout) =
                if write_bridge {
                    (
                        local_image,
                        local_layout,
                        bridge_image,
                        vk::ImageLayout::GENERAL,
                    )
                } else {
                    (
                        bridge_image,
                        vk::ImageLayout::GENERAL,
                        local_image,
                        local_layout,
                    )
                };
            gpu.device.cmd_copy_image(
                endpoint.command,
                source_image,
                source_layout,
                destination_image,
                destination_layout,
                &[region],
            );
            dma_buf::ownership_barrier(
                &gpu.device,
                endpoint.command,
                bridge_image,
                gpu.queue_family_index,
                false,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                bridge_access,
            );
            let barrier = vk::ImageMemoryBarrier::default()
                .image(local_image)
                .subresource_range(range)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .old_layout(local_layout)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(local_access)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
            gpu.device.cmd_pipeline_barrier(
                endpoint.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
        Ok(())
    }

    /// Both devices must have finished every submission using this bridge.
    pub unsafe fn destroy(self, source: &CxVulkan, destination: &CxVulkan) {
        self.source.destroy(source);
        self.destination.destroy(destination);
    }
}
