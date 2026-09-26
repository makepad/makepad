//! Android presentation: repaints in flight.
//!
//! Every pass of one repaint (the captures, their gauss levels, the window)
//! records into ONE command buffer that goes to the queue in ONE submission:
//! the window pass submits and presents it, a repaint that ends without a
//! window pass is submitted by `end_repaint`. Two such slots alternate, so
//! the CPU records repaint N while the GPU still executes N-1.
//!
//! Before this, the Android path waited the single frame fence before every
//! pass and again after every offscreen pass, and freed the packet arena and
//! descriptor pools each time: with the phone shell's captures every frame
//! was a chain of CPU<->GPU round trips. GLES pipelined all of this inside
//! the driver; native Vulkan has to do it here.
//!
//! What a pending slot may still read stays with the slot until its fence
//! proves the submission complete: its packet arena, descriptor sets,
//! staging buffers and retained leases (`FrameResources`), and every texture
//! or geometry buffer replaced while it recorded (`retire_*` in vulkan.rs).

use super::*;

/// Repaints the CPU may run ahead of the GPU. Two overlaps recording with
/// execution; a third only adds a frame of latency behind the Choreographer.
const FRAMES_IN_FLIGHT: usize = 2;

pub(super) struct RepaintSlot {
    serial: u64,
    frame_resources: FrameResources,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    /// Acquire signals it, the submission waits on it. Per slot because the
    /// previous repaint's wait on it may still be pending when this one
    /// acquires.
    image_available_semaphore: vk::Semaphore,
}

struct OpenRepaint {
    slot_index: usize,
    repaint_id: u64,
    /// Passes recorded so far; repainted if the recording is abandoned.
    passes: Vec<DrawPassId>,
    submitted: bool,
}

#[derive(Default)]
pub(super) struct RepaintRing {
    slots: Vec<RepaintSlot>,
    /// The slot the next repaint takes.
    next: usize,
    /// The slot whose command buffer, fence, semaphore and resources are
    /// swapped into `CxVulkan`'s frame fields while a repaint records.
    open: Option<OpenRepaint>,
    /// Passes of an abandoned recording; the next repaint marks them dirty.
    redirty: Vec<DrawPassId>,
}

impl RepaintRing {
    pub(super) fn mark_submitted(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.submitted = true;
        }
    }
}

impl CxVulkan {
    pub(super) fn create_repaint_ring(&self) -> Result<RepaintRing, String> {
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT as u32);
        let command_buffers = unsafe { self.device.allocate_command_buffers(&alloc_info) }
            .map_err(|e| format!("allocate_command_buffers(repaint ring) failed: {e:?}"))?;
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let mut slots: Vec<RepaintSlot> = Vec::with_capacity(command_buffers.len());
        for &command_buffer in &command_buffers {
            let created = unsafe {
                self.device.create_fence(&fence_info, None).and_then(|fence| {
                    self.device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                        .map(|semaphore| (fence, semaphore))
                        .inspect_err(|_| self.device.destroy_fence(fence, None))
                })
            };
            match created {
                Ok((fence, image_available_semaphore)) => slots.push(RepaintSlot {
                    serial: 0,
                    frame_resources: FrameResources::default(),
                    command_buffer,
                    fence,
                    image_available_semaphore,
                }),
                Err(err) => {
                    unsafe {
                        for slot in &slots {
                            self.device.destroy_fence(slot.fence, None);
                            self.device
                                .destroy_semaphore(slot.image_available_semaphore, None);
                        }
                        self.device
                            .free_command_buffers(self.command_pool, &command_buffers);
                    }
                    return Err(format!("repaint ring sync objects: {err:?}"));
                }
            }
        }
        Ok(RepaintRing {
            slots,
            ..RepaintRing::default()
        })
    }

    /// Every window and offscreen pass starts here. The first pass of a
    /// repaint takes the next slot: waits its fence (the repaint before the
    /// previous one), reclaims what that submission owned, and opens its
    /// command buffer; later passes of the same repaint record into it.
    /// `Ok(false)` means the slot is still executing past the bound: the
    /// pass stays dirty and the next vsync retries.
    pub(super) fn begin_repaint_pass(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
    ) -> Result<bool, String> {
        for pass in self.repaints.redirty.drain(..) {
            if !cx.passes.0.is_free(pass.0) {
                cx.passes[pass].paint_dirty = true;
            }
        }
        if let Some(open) = self.repaints.open.as_mut() {
            if open.repaint_id == cx.repaint_id {
                open.passes.push(draw_pass_id);
                return Ok(true);
            }
            // A repaint that ended without a window pass or `end_repaint`
            // (an error path): its recording goes to the queue first.
            self.flush_repaint()?;
        }
        let slot_index = self.repaints.next;
        let fence = self.repaints.slots[slot_index].fence;
        match unsafe { self.device.wait_for_fences(&[fence], true, FRAME_FENCE_WAIT_NS) } {
            Ok(()) => {}
            Err(vk::Result::TIMEOUT) => return Ok(false),
            Err(e) => return Err(format!("wait_for_fences(repaint slot) failed: {e:?}")),
        }
        let slot = &mut self.repaints.slots[slot_index];
        // The fence proved that submission complete: its serial is the
        // completion frontier, and what it owned can go or be reused.
        cx.textures.1.serials.complete(slot.serial);
        Self::recycle_completed_owned_frame_resources(&self.device, &mut slot.frame_resources)?;
        // Retirements made between repaints (the standalone set) follow this
        // slot: an older slot may still be executing with them.
        slot.frame_resources
            .retired_textures
            .append(&mut self.frame_resources.retired_textures);
        slot.frame_resources
            .retired_geometries
            .append(&mut self.frame_resources.retired_geometries);
        slot.frame_resources
            .buffers
            .append(&mut self.frame_resources.buffers);
        slot.frame_resources
            .framebuffers
            .append(&mut self.frame_resources.framebuffers);
        slot.frame_resources
            .sync_semaphores
            .append(&mut self.frame_resources.sync_semaphores);
        self.swap_repaint_slot(slot_index);
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer(repaint) failed: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin_command_buffer(repaint) failed: {e:?}"))?;
        }
        self.repaints.open = Some(OpenRepaint {
            slot_index,
            repaint_id: cx.repaint_id,
            passes: vec![draw_pass_id],
            submitted: false,
        });
        self.repaints.next = (slot_index + 1) % self.repaints.slots.len();
        Ok(true)
    }

    fn swap_repaint_slot(&mut self, slot_index: usize) {
        let slot = &mut self.repaints.slots[slot_index];
        std::mem::swap(&mut self.frame_serial_in_flight, &mut slot.serial);
        std::mem::swap(&mut self.frame_resources, &mut slot.frame_resources);
        std::mem::swap(&mut self.command_buffer, &mut slot.command_buffer);
        std::mem::swap(&mut self.in_flight_fence, &mut slot.fence);
        std::mem::swap(
            &mut self.image_available_semaphore,
            &mut slot.image_available_semaphore,
        );
    }

    /// The recording went to the queue (or was abandoned): give the slot
    /// its handles back. The standalone frame fields return.
    pub(super) fn close_repaint(&mut self) {
        if let Some(open) = self.repaints.open.take() {
            self.swap_repaint_slot(open.slot_index);
        }
    }

    /// A repaint that ends without a window pass still has to reach the
    /// GPU: the captures it rendered are sampled by the next window pass,
    /// and its serial must complete.
    pub(super) fn flush_repaint(&mut self) -> Result<(), String> {
        let Some(open) = self.repaints.open.as_ref() else {
            return Ok(());
        };
        if open.submitted {
            self.close_repaint();
            return Ok(());
        }
        let ended = unsafe { self.device.end_command_buffer(self.command_buffer) };
        if let Err(err) = ended {
            self.abort_repaint();
            return Err(format!("end_command_buffer(repaint flush) failed: {err:?}"));
        }
        let command_buffers = [self.command_buffer];
        let submitted =
            self.submit_frame(&vk::SubmitInfo::default().command_buffers(&command_buffers));
        match submitted {
            Ok(()) => self.close_repaint(),
            Err(_) => self.abort_repaint(),
        }
        submitted
    }

    /// The recording cannot be submitted: the swapchain and its pipelines
    /// are going, or recording failed half way. Nothing reached the GPU, so
    /// the slot's fence is still signaled; its passes repaint next time.
    /// After the submission this only returns the slot's handles.
    pub(super) fn abort_repaint(&mut self) {
        let Some(open) = self.repaints.open.as_ref() else {
            return;
        };
        if !open.submitted {
            unsafe {
                let _ = self
                    .device
                    .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty());
            }
            // Relocations recorded for this repaint never ran: their transfer
            // commands go, and the live-generation check re-copies them.
            self.frame_resources.retained_transfers = None;
            self.frame_resources.retained_updates.clear();
            let passes = std::mem::take(&mut self.repaints.open.as_mut().unwrap().passes);
            self.repaints.redirty.extend(passes);
        }
        self.close_repaint();
    }

    /// The platform loop calls this after the repaint's passes.
    pub fn end_repaint(&mut self) -> Result<(), String> {
        self.flush_repaint()
    }

    /// The device is idle (swapchain teardown): a slot's pooled memory is
    /// released too, so a suspended app holds no packet arenas.
    pub(super) fn release_repaint_ring_resources(&mut self) {
        self.abort_repaint();
        for slot in &mut self.repaints.slots {
            Self::destroy_owned_frame_resources(&self.device, &mut slot.frame_resources);
        }
    }

    pub(super) fn destroy_repaint_ring(&mut self) {
        self.release_repaint_ring_resources();
        let command_buffers: Vec<vk::CommandBuffer> = self
            .repaints
            .slots
            .iter()
            .map(|slot| slot.command_buffer)
            .collect();
        unsafe {
            for slot in self.repaints.slots.drain(..) {
                self.device.destroy_fence(slot.fence, None);
                self.device
                    .destroy_semaphore(slot.image_available_semaphore, None);
            }
            if !command_buffers.is_empty() {
                self.device
                    .free_command_buffers(self.command_pool, &command_buffers);
            }
        }
        self.repaints.next = 0;
    }
}

/// GPU sync between a host (the WM) and its hosted children, which share
/// frames as AHardwareBuffers: every submission signals a SYNC_FD it exports
/// (`VK_KHR_external_semaphore_fd`). A child sends its frame's fd to the
/// host instead of waiting for its GPU on the CPU; the host waits on those
/// fds inside its next submission, and hands its own latest fd back so a
/// child's next write into a shared image waits for the host's reads of it.
#[derive(Default)]
pub(super) struct HostedSync {
    loader: Option<ash::khr::external_semaphore_fd::Device>,
    role: HostedSyncRole,
    /// Signaled by every submission while a role is set, exported right
    /// after it (a SYNC_FD export resets the semaphore for the next one).
    export_semaphore: vk::Semaphore,
    exported: Option<std::os::fd::OwnedFd>,
    /// Imported fds the next submission waits on.
    waits: Vec<vk::Semaphore>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum HostedSyncRole {
    #[default]
    None,
    Host,
    Child,
}

fn slice<'a, T>(ptr: *const T, len: u32) -> &'a [T] {
    if len == 0 || ptr.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len as usize) } }
}

impl CxVulkan {
    /// Whether the device imports and exports SYNC_FD semaphores; called once
    /// the device exists (the extension is enabled only when it does).
    pub(super) fn init_hosted_sync(&mut self, instance: &ash::Instance, enabled: bool) {
        if !enabled {
            return;
        }
        let info = vk::PhysicalDeviceExternalSemaphoreInfo::default()
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        let mut props = vk::ExternalSemaphoreProperties::default();
        unsafe {
            instance.get_physical_device_external_semaphore_properties(
                self.physical_device,
                &info,
                &mut props,
            )
        };
        let needed = vk::ExternalSemaphoreFeatureFlags::EXPORTABLE
            | vk::ExternalSemaphoreFeatureFlags::IMPORTABLE;
        if !props.external_semaphore_features.contains(needed) {
            crate::log!("hosted sync: SYNC_FD semaphores not supported ({:?})", props.external_semaphore_features);
            return;
        }
        self.hosted_sync.loader = Some(ash::khr::external_semaphore_fd::Device::new(instance, &self.device));
    }

    /// Start exporting a SYNC_FD per submission in `role`. False when the
    /// device cannot: the caller keeps its CPU waits.
    pub fn set_hosted_sync_role(&mut self, role: HostedSyncRole) -> bool {
        if self.hosted_sync.loader.is_none() {
            return false;
        }
        if self.hosted_sync.export_semaphore == vk::Semaphore::null() {
            let mut export = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            let info = vk::SemaphoreCreateInfo::default().push_next(&mut export);
            match unsafe { self.device.create_semaphore(&info, None) } {
                Ok(semaphore) => self.hosted_sync.export_semaphore = semaphore,
                Err(err) => {
                    crate::error!("hosted sync: exportable semaphore: {err:?}");
                    return false;
                }
            }
        }
        self.hosted_sync.role = role;
        true
    }

    pub fn hosted_sync_role(&self) -> HostedSyncRole {
        self.hosted_sync.role
    }

    /// The next submission waits (GPU side) for `fd` to signal.
    pub fn wait_sync_fd(&mut self, fd: std::os::fd::OwnedFd) -> Result<(), String> {
        use std::os::fd::IntoRawFd;
        let Some(loader) = self.hosted_sync.loader.as_ref() else {
            return Err("hosted sync: no SYNC_FD support".into());
        };
        let semaphore = unsafe { self.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
            .map_err(|e| format!("hosted sync: wait semaphore: {e:?}"))?;
        let raw = fd.into_raw_fd();
        let import = vk::ImportSemaphoreFdInfoKHR::default()
            .semaphore(semaphore)
            .flags(vk::SemaphoreImportFlags::TEMPORARY)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
            .fd(raw);
        if let Err(err) = unsafe { loader.import_semaphore_fd(&import) } {
            // A failed import leaves the fd with us.
            unsafe {
                self.device.destroy_semaphore(semaphore, None);
                drop(<std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw));
            }
            return Err(format!("hosted sync: import SYNC_FD: {err:?}"));
        }
        self.hosted_sync.waits.push(semaphore);
        Ok(())
    }

    /// The SYNC_FD of the latest submission (it signals when that submission
    /// completes), once.
    pub fn take_exported_submit_fd(&mut self) -> Option<std::os::fd::OwnedFd> {
        self.hosted_sync.exported.take()
    }

    /// `queue_submit` with the hosted waits and the export signal added.
    pub(super) fn hosted_sync_queue_submit(
        &mut self,
        info: &vk::SubmitInfo<'_>,
        fence: vk::Fence,
    ) -> ash::prelude::VkResult<()> {
        let role = self.hosted_sync.role;
        if role == HostedSyncRole::None {
            return unsafe { self.device.queue_submit(self.queue, &[*info], fence) };
        }
        if role == HostedSyncRole::Host {
            for fd in crate::os::linux::android::android_hosted::take_acquire_fds() {
                if let Err(err) = self.wait_sync_fd(fd) {
                    crate::error!("{err}");
                }
            }
        }
        let mut waits: Vec<vk::Semaphore> = slice(info.p_wait_semaphores, info.wait_semaphore_count).to_vec();
        let mut stages: Vec<vk::PipelineStageFlags> =
            slice(info.p_wait_dst_stage_mask, info.wait_semaphore_count).to_vec();
        for &semaphore in &self.hosted_sync.waits {
            waits.push(semaphore);
            stages.push(vk::PipelineStageFlags::ALL_COMMANDS);
        }
        let mut signals: Vec<vk::Semaphore> =
            slice(info.p_signal_semaphores, info.signal_semaphore_count).to_vec();
        signals.push(self.hosted_sync.export_semaphore);
        let command_buffers = slice(info.p_command_buffers, info.command_buffer_count);
        let merged = vk::SubmitInfo::default()
            .wait_semaphores(&waits)
            .wait_dst_stage_mask(&stages)
            .command_buffers(command_buffers)
            .signal_semaphores(&signals);
        unsafe { self.device.queue_submit(self.queue, &[merged], fence) }?;
        // The waited semaphores live until this submission's fence.
        self.frame_resources.sync_semaphores.append(&mut self.hosted_sync.waits);
        let get = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(self.hosted_sync.export_semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        let loader = self.hosted_sync.loader.as_ref().unwrap();
        match unsafe { loader.get_semaphore_fd(&get) } {
            Ok(raw) => {
                let fd = (raw >= 0).then(|| unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw) });
                match role {
                    HostedSyncRole::Host => {
                        if let Some(fd) = fd {
                            crate::os::linux::android::android_hosted::publish_release_fd(fd);
                        }
                    }
                    _ => self.hosted_sync.exported = fd,
                }
            }
            Err(err) => {
                crate::error!("hosted sync: export SYNC_FD: {err:?}");
                // A failed export may leave the payload behind: the next
                // submission signals a fresh semaphore.
                self.frame_resources.sync_semaphores.push(self.hosted_sync.export_semaphore);
                self.hosted_sync.export_semaphore = vk::Semaphore::null();
                let role = self.hosted_sync.role;
                if !self.set_hosted_sync_role(role) {
                    self.hosted_sync.role = HostedSyncRole::None;
                }
            }
        }
        Ok(())
    }

    pub(super) fn destroy_hosted_sync(&mut self) {
        unsafe {
            for semaphore in self.hosted_sync.waits.drain(..) {
                self.device.destroy_semaphore(semaphore, None);
            }
            if self.hosted_sync.export_semaphore != vk::Semaphore::null() {
                self.device.destroy_semaphore(self.hosted_sync.export_semaphore, None);
                self.hosted_sync.export_semaphore = vk::Semaphore::null();
            }
        }
        self.hosted_sync.exported = None;
    }
}
