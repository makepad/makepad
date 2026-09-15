//! Shared Vulkan images. A frame's Texture handle is its read lease: clones
//! held by previews retain the image until the last GPU reader has completed.
use super::*;
use crate::{
    os::shared_framebuf::{LinuxOwnedImage, LinuxOwnedImagePlane, LinuxOwnedVulkanImage},
    texture::{Texture, WeakTexture},
};
use makepad_studio_protocol::LinuxVulkanSharedImage;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

impl Cx {
    pub(crate) fn export_vulkan_presentable_image(
        &mut self,
        texture: &Texture,
    ) -> Result<LinuxOwnedImage, String> {
        let (width, height) = match texture.get_format(self) {
            TextureFormat::SharedBGRAu8 { width, height, .. } => (*width as u32, *height as u32),
            _ => return Err("expected a shared BGRA render target".into()),
        };
        let gpu = self
            .os
            .vulkan
            .as_mut()
            .ok_or("Vulkan renderer not initialized")?;
        #[cfg(linux_direct)]
        {
            gpu.export_transition_image(texture, width, height)
        }
        #[cfg(not(linux_direct))]
        {
            gpu.export_shared_image(texture, width, height)
        }
    }
}

const INITIAL_WRITE: u64 = 2;

pub(super) struct SharedImage {
    semaphore: vk::Semaphore,
    memory_fd: Option<OwnedFd>,
    semaphore_fd: Option<OwnedFd>,
    info: LinuxVulkanSharedImage,
    producer: bool,
    next_write: u64,
    last_sequence: u64,
    frame: Option<WeakTexture>,
}

struct FrameLease {
    // Pins the owning resource; the alias owns only its separate image view.
    source: Texture,
    sequence: u64,
    acquired: bool,
}

#[derive(Default)]
pub(super) struct SharedState {
    pub(super) enabled: bool,
    frozen: bool,
    images: HashMap<VulkanTextureKey, SharedImage>,
    frames: HashMap<VulkanTextureKey, FrameLease>,
    retired: Vec<FrameLease>,
    waits: Vec<(vk::Semaphore, u64)>,
    signals: Vec<(vk::Semaphore, u64)>,
    trace_last: Option<std::time::Instant>,
    #[cfg(linux_direct)]
    snapshots: Vec<(VulkanTextureKey, VulkanTextureKey, VulkanTextureResource)>,
    #[cfg(linux_direct)]
    quiesce_pending: bool,
}

pub(super) fn device_supports_sharing(
    instance: &ash::Instance,
    device: vk::PhysicalDevice,
) -> bool {
    let Ok(extensions) = (unsafe { instance.enumerate_device_extension_properties(device) }) else {
        return false;
    };
    if ![
        vk::KHR_EXTERNAL_MEMORY_FD_NAME,
        vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME,
        vk::KHR_TIMELINE_SEMAPHORE_NAME,
    ]
    .iter()
    .all(|name| {
        extensions
            .iter()
            .any(|ext| unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) == *name })
    }) {
        return false;
    }
    let mut timeline = vk::PhysicalDeviceTimelineSemaphoreFeatures::default();
    let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut timeline);
    unsafe { instance.get_physical_device_features2(device, &mut features) };
    timeline.timeline_semaphore != 0
}

impl CxVulkan {
    pub(super) fn trace_shared_leases(&mut self, cx: &Cx) {
        if !crate::makepad_error_log::trace_enabled("runview.leases") {
            return;
        }
        let now = std::time::Instant::now();
        if self.desktop.shared.trace_last.is_some_and(|at| now.duration_since(at).as_secs_f64() < 1.0) {
            return;
        }
        self.desktop.shared.trace_last = Some(now);
        for (key, lease) in &self.desktop.shared.frames {
            let mut bindings = Vec::new();
            for (list_index, slot) in cx.draw_lists.0.pool.iter().enumerate() {
                let list = &slot.item;
                for (item_index, item) in list.draw_items.buffer.iter().enumerate() {
                    if item.kind.draw_call().is_some_and(|call| call.texture_slots.iter().flatten().any(|t| t.texture_id() == key.0)) {
                        bindings.push(format!("list={list_index} debug={} free={} item={item_index}/{}", list.debug_id, cx.draw_lists.is_slot_freed(list_index), list.draw_items.len()));
                    }
                }
            }
            crate::trace!("runview.leases", "alias={} source={:?} sequence={} acquired={} free={} bindings={:?}", key, lease.source.texture_id(), lease.sequence, lease.acquired, cx.textures.0.is_free(key.0.0), bindings);
        }
    }

    pub(crate) fn import_shared_image(
        &mut self,
        texture: &Texture,
        width: u32,
        height: u32,
        image: LinuxOwnedImage,
    ) -> Result<(), String> {
        #[cfg(linux_direct)]
        if image
            .vulkan
            .as_ref()
            .is_some_and(|image| image.info.device_uuid != self.desktop.gpu.device.uuid)
        {
            return self.import_routed_shared_image(texture, width, height, image);
        }
        let LinuxOwnedImage { plane, vulkan, .. } = image;
        let shared = vulkan.ok_or("Vulkan hosted rendering requires a shared GPU image")?;
        let key = Self::texture_key(texture.texture_id());
        let (resource, info) =
            self.create_shared_resource(width, height, Some((shared.info, plane.dma_buf_fd)))?;
        let mut timeline =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let semaphore = match unsafe {
            self.device.create_semaphore(
                &vk::SemaphoreCreateInfo::default().push_next(&mut timeline),
                None,
            )
        } {
            Ok(semaphore) => semaphore,
            Err(e) => {
                self.destroy_texture_resource(resource);
                return Err(format!("create imported timeline: {e:?}"));
            }
        };
        let loader = ash::khr::external_semaphore_fd::Device::new(&self.instance, &self.device);
        let result = unsafe {
            loader.import_semaphore_fd(
                &vk::ImportSemaphoreFdInfoKHR::default()
                    .semaphore(semaphore)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD)
                    .fd(shared.semaphore_fd.as_raw_fd()),
            )
        };
        if let Err(e) = result {
            unsafe { self.device.destroy_semaphore(semaphore, None) };
            self.destroy_texture_resource(resource);
            return Err(format!("import timeline FD: {e:?}"));
        }
        let _ = shared.semaphore_fd.into_raw_fd();
        // Bootstrap may resend the same swapchain after the child has already
        // presented frames. Resume its timeline instead of reusing sequence 3.
        let timeline_loader =
            ash::khr::timeline_semaphore::Device::new(&self.instance, &self.device);
        let next_write = match unsafe { timeline_loader.get_semaphore_counter_value(semaphore) } {
            Ok(value) if value >= INITIAL_WRITE && value < u64::MAX => value + (value & 1),
            result => {
                unsafe { self.device.destroy_semaphore(semaphore, None) };
                self.destroy_texture_resource(resource);
                return Err(format!("invalid imported timeline state: {result:?}"));
            }
        };
        self.textures.insert(key, resource);
        self.desktop.shared.images.insert(
            key,
            SharedImage {
                semaphore,
                info,
                memory_fd: None,
                semaphore_fd: None,
                producer: true,
                next_write,
                last_sequence: 0,
                frame: None,
            },
        );
        crate::log!(
            "Vulkan shared image: imported {}x{} without pixel copies",
            width,
            height
        );
        Ok(())
    }

    pub(crate) fn shared_write_available(&self, texture: TextureId) -> Result<bool, String> {
        if self.desktop.shared.frozen {
            return Ok(false);
        }
        #[cfg(linux_direct)]
        if self.is_routed_shared_image(texture) {
            return self.routed_shared_write_available(texture);
        }
        let key = Self::texture_key(texture);
        let shared = self
            .desktop
            .shared
            .images
            .get(&key)
            .ok_or("missing shared image")?;
        if !shared.producer {
            return Err("cannot render into a WM-owned read image".into());
        }
        let loader = ash::khr::timeline_semaphore::Device::new(&self.instance, &self.device);
        let value = unsafe { loader.get_semaphore_counter_value(shared.semaphore) }
            .map_err(|e| format!("poll shared image availability: {e:?}"))?;
        if value < shared.next_write {
            crate::trace!("runview.blocked", "image={:?} timeline={} required={}", texture, value, shared.next_write);
        }
        Ok(value >= shared.next_write)
    }

    pub(crate) fn shared_draw_sequence(&self, texture: TextureId) -> u64 {
        self.desktop
            .shared
            .images
            .get(&Self::texture_key(texture))
            .map(|image| image.last_sequence)
            .unwrap_or(0)
    }

    pub(super) fn is_shared_image(&self, texture: TextureId) -> bool {
        #[cfg(linux_direct)]
        if self.is_routed_shared_image(texture) {
            return true;
        }
        self.is_external_shared_image(texture)
    }

    pub(super) fn is_external_shared_image(&self, texture: TextureId) -> bool {
        self.desktop
            .shared
            .images
            .contains_key(&Self::texture_key(texture))
    }

    #[cfg(linux_direct)]
    pub(super) fn shared_sequence_completed(
        &self,
        texture: TextureId,
        sequence: u64,
    ) -> Result<bool, String> {
        let shared = self
            .desktop
            .shared
            .images
            .get(&Self::texture_key(texture))
            .ok_or("missing hosted output timeline")?;
        let loader = ash::khr::timeline_semaphore::Device::new(&self.instance, &self.device);
        unsafe { loader.get_semaphore_counter_value(shared.semaphore) }
            .map(|value| sequence != 0 && value >= sequence)
            .map_err(|e| format!("poll hosted output timeline: {e:?}"))
    }

    /// Caller has drained every submission referencing this producer and
    /// owns its last imported transport handle (no host-side reader aliases).
    #[cfg(linux_direct)]
    pub(super) fn retire_unused_shared_producer(&mut self, key: VulkanTextureKey) {
        if !self
            .desktop
            .shared
            .images
            .get(&key)
            .is_some_and(|image| image.producer)
        {
            return;
        }
        if let Some(image) = self.desktop.shared.images.remove(&key) {
            unsafe { self.device.destroy_semaphore(image.semaphore, None) };
        }
        if let Some(resource) = self.textures.remove(&key) {
            self.destroy_texture_resource(resource);
        }
    }

    pub(crate) fn acquire_shared_draw(
        &mut self,
        cx: &mut Cx,
        source: &Texture,
        sequence: u64,
    ) -> Result<Texture, String> {
        if self.desktop.shared.frozen {
            return Err("GPU transition has paused shared frame admission".into());
        }
        let key = Self::texture_key(source.texture_id());
        let shared = self
            .desktop
            .shared
            .images
            .get(&key)
            .ok_or("presented frame has no shared GPU image")?;
        if shared.producer || sequence < INITIAL_WRITE + 1 || sequence % 2 != 1 {
            return Err("invalid shared frame sequence/owner".into());
        }
        if sequence == shared.last_sequence {
            return shared
                .frame
                .as_ref()
                .and_then(WeakTexture::upgrade)
                .ok_or("shared frame already retired".into());
        }
        if sequence < shared.last_sequence
            || shared
                .frame
                .as_ref()
                .and_then(WeakTexture::upgrade)
                .is_some()
        {
            return Err("producer overwrote a leased frame or sent a stale sequence".into());
        }
        let timeline_loader =
            ash::khr::timeline_semaphore::Device::new(&self.instance, &self.device);
        let completed = unsafe { timeline_loader.get_semaphore_counter_value(shared.semaphore) }
            .map_err(|error| format!("query presented frame completion: {error:?}"))?;
        if completed != sequence {
            return Err(format!(
                "frame sequence {sequence} is not available (timeline {completed})"
            ));
        }
        let resource = self
            .textures
            .get(&key)
            .ok_or("exported Vulkan image disappeared")?;
        let view = self.shared_view(resource.image)?;
        let format = source.get_format(cx).clone();
        let alias = Texture::new_with_format(cx, format);
        let alias_key = Self::texture_key(alias.texture_id());
        let alias_resource = VulkanTextureResource {
            image: resource.image,
            memory: resource.memory,
            view,
            face_views: [vk::ImageView::null(); 6],
            width: resource.width,
            height: resource.height,
            layers: 1,
            is_cube: false,
            format: resource.format,
            layout: vk::ImageLayout::GENERAL,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: false,
        };
        self.textures.insert(alias_key, alias_resource);
        self.desktop.shared.frames.insert(
            alias_key,
            FrameLease {
                source: source.clone(),
                sequence,
                acquired: false,
            },
        );
        let shared = self.desktop.shared.images.get_mut(&key).unwrap();
        shared.last_sequence = sequence;
        shared.frame = Some(alias.downgrade());
        crate::trace!(
            "runview.gpu",
            "shared image {} accepted sequence {} as {}",
            key,
            sequence,
            alias_key
        );
        Ok(alias)
    }

    pub(super) fn retire_shared_texture(&mut self, key: VulkanTextureKey) {
        if let Some(lease) = self.desktop.shared.frames.remove(&key) {
            self.desktop.shared.retired.push(lease);
        }
        if let Some(image) = self.desktop.shared.images.remove(&key) {
            unsafe { self.device.destroy_semaphore(image.semaphore, None) };
        }
    }

    // Called inside a command buffer, before any render pass begins.
    pub(super) fn release_retired_shared_frames(&mut self) {
        for lease in std::mem::take(&mut self.desktop.shared.retired) {
            let key = Self::texture_key(lease.source.texture_id());
            let image = self.textures[&key].image;
            let semaphore = self.desktop.shared.images[&key].semaphore;
            if !lease.acquired {
                self.desktop.shared.waits.push((semaphore, lease.sequence));
                self.shared_barrier(image, true);
            }
            self.shared_barrier(image, false);
            self.desktop
                .shared
                .signals
                .push((semaphore, lease.sequence + 1));
            crate::trace!(
                "runview.gpu",
                "shared image {} released sequence {} for producer {}",
                key,
                lease.sequence,
                lease.sequence + 1
            );
        }
    }

    pub(super) fn acquire_shared_sample(&mut self, texture: TextureId) {
        let key = Self::texture_key(texture);
        let Some(lease) = self.desktop.shared.frames.get_mut(&key) else {
            return;
        };
        if lease.acquired {
            return;
        }
        lease.acquired = true;
        let source_key = Self::texture_key(lease.source.texture_id());
        let sequence = lease.sequence;
        let semaphore = self.desktop.shared.images[&source_key].semaphore;
        self.desktop.shared.waits.push((semaphore, sequence));
        self.shared_barrier(self.textures[&key].image, true);
    }

    pub(super) fn acquire_shared_write(&mut self, texture: TextureId) {
        let key = Self::texture_key(texture);
        let Some(shared) = self.desktop.shared.images.get(&key) else {
            return;
        };
        if !shared.producer {
            return;
        }
        self.desktop
            .shared
            .waits
            .push((shared.semaphore, shared.next_write));
        self.shared_barrier(self.textures[&key].image, true);
    }

    pub(super) fn release_shared_write(&mut self, texture: TextureId) -> bool {
        self.release_shared_write_from(texture, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
    }

    pub(super) fn release_shared_write_from(
        &mut self,
        texture: TextureId,
        layout: vk::ImageLayout,
    ) -> bool {
        let key = Self::texture_key(texture);
        let Some(shared) = self.desktop.shared.images.get_mut(&key) else {
            return false;
        };
        if !shared.producer {
            return false;
        }
        let sequence = shared.next_write + 1;
        shared.next_write += 2;
        shared.last_sequence = sequence;
        self.desktop
            .shared
            .signals
            .push((shared.semaphore, sequence));
        let image = self.textures[&key].image;
        self.transition_image_layout(
            image,
            vk::ImageAspectFlags::COLOR,
            1,
            layout,
            vk::ImageLayout::GENERAL,
        );
        self.shared_barrier(image, false);
        true
    }

    pub(super) fn shared_submit(&mut self, info: &vk::SubmitInfo<'_>) -> Result<(), vk::Result> {
        if self.desktop.shared.waits.is_empty() && self.desktop.shared.signals.is_empty() {
            return unsafe {
                self.device
                    .queue_submit(self.queue, &[*info], self.in_flight_fence)
            };
        }
        // Preserve the binary WSI semaphores alongside the shared timelines.
        let mut waits = if info.wait_semaphore_count == 0 {
            Vec::new()
        } else {
            unsafe {
                std::slice::from_raw_parts(
                    info.p_wait_semaphores,
                    info.wait_semaphore_count as usize,
                )
            }
            .to_vec()
        };
        let mut stages = if info.wait_semaphore_count == 0 {
            Vec::new()
        } else {
            unsafe {
                std::slice::from_raw_parts(
                    info.p_wait_dst_stage_mask,
                    info.wait_semaphore_count as usize,
                )
            }
            .to_vec()
        };
        let mut signals = if info.signal_semaphore_count == 0 {
            Vec::new()
        } else {
            unsafe {
                std::slice::from_raw_parts(
                    info.p_signal_semaphores,
                    info.signal_semaphore_count as usize,
                )
            }
            .to_vec()
        };
        let mut wait_values = vec![0; waits.len()];
        let mut signal_values = vec![0; signals.len()];
        for (semaphore, value) in self.desktop.shared.waits.drain(..) {
            waits.push(semaphore);
            wait_values.push(value);
            stages.push(vk::PipelineStageFlags::ALL_COMMANDS);
        }
        for (semaphore, value) in self.desktop.shared.signals.drain(..) {
            signals.push(semaphore);
            signal_values.push(value);
        }
        let mut timeline = vk::TimelineSemaphoreSubmitInfo::default()
            .wait_semaphore_values(&wait_values)
            .signal_semaphore_values(&signal_values);
        timeline.p_next = info.p_next;
        let mut submit = *info;
        submit.p_next = (&timeline as *const vk::TimelineSemaphoreSubmitInfo).cast();
        submit.wait_semaphore_count = waits.len() as u32;
        submit.p_wait_semaphores = waits.as_ptr();
        submit.p_wait_dst_stage_mask = stages.as_ptr();
        submit.signal_semaphore_count = signals.len() as u32;
        submit.p_signal_semaphores = signals.as_ptr();
        unsafe {
            self.device
                .queue_submit(self.queue, &[submit], self.in_flight_fence)
        }
    }

    pub(super) fn destroy_shared_state(&mut self) {
        #[cfg(linux_direct)]
        for (_, _, resource) in std::mem::take(&mut self.desktop.shared.snapshots) {
            self.destroy_texture_resource(resource);
        }
        for (_, image) in self.desktop.shared.images.drain() {
            unsafe { self.device.destroy_semaphore(image.semaphore, None) };
        }
        self.desktop.shared.frames.clear();
        self.desktop.shared.retired.clear();
    }

    fn shared_identity(&self) -> LinuxVulkanSharedImage {
        let mut ids = vk::PhysicalDeviceIDProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
        unsafe {
            self.instance
                .get_physical_device_properties2(self.physical_device, &mut properties)
        };
        LinuxVulkanSharedImage {
            allocation_size: 0,
            memory_type_index: 0,
            device_uuid: ids.device_uuid,
            driver_uuid: ids.driver_uuid,
        }
    }

    fn shared_view(&self, image: vk::Image) -> Result<vk::ImageView, String> {
        let info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        unsafe { self.device.create_image_view(&info, None) }
            .map_err(|e| format!("create shared image view: {e:?}"))
    }

    fn create_shared_resource(
        &self,
        width: u32,
        height: u32,
        mut import: Option<(LinuxVulkanSharedImage, OwnedFd)>,
    ) -> Result<(VulkanTextureResource, LinuxVulkanSharedImage), String> {
        if !self.desktop.shared.enabled {
            return Err("GPU frame sharing requires external memory FD, external semaphore FD and timeline semaphores".into());
        }
        let mut identity = self.shared_identity();
        let is_imported = import.is_some();
        if let Some((info, _)) = import.as_ref() {
            if info.device_uuid != identity.device_uuid || info.driver_uuid != identity.driver_uuid
            {
                return Err("shared frame comes from a different Vulkan device or driver".into());
            }
        }
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        let mut external_query = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let query = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(vk::Format::B8G8R8A8_UNORM)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .push_next(&mut external_query);
        let mut external_properties = vk::ExternalImageFormatProperties::default();
        let mut properties =
            vk::ImageFormatProperties2::default().push_next(&mut external_properties);
        unsafe {
            self.instance.get_physical_device_image_format_properties2(
                self.physical_device,
                &query,
                &mut properties,
            )
        }
        .map_err(|e| format!("query shared image format: {e:?}"))?;
        if !external_properties
            .external_memory_properties
            .external_memory_features
            .contains(
                vk::ExternalMemoryFeatureFlags::EXPORTABLE
                    | vk::ExternalMemoryFeatureFlags::IMPORTABLE,
            )
        {
            return Err(
                "BGRA render images cannot be exported/imported on this Vulkan device".into(),
            );
        }
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        let image = unsafe { self.device.create_image(&info, None) }
            .map_err(|e| format!("create shared image: {e:?}"))?;
        let result = (|| {
            let requirements = unsafe { self.device.get_image_memory_requirements(image) };
            identity.allocation_size = requirements.size;
            identity.memory_type_index = self.find_memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )?;
            if let Some((info, _)) = import.as_ref() {
                if info.memory_type_index >= 32
                    || requirements.memory_type_bits & (1 << info.memory_type_index) == 0
                    || info.allocation_size < requirements.size
                {
                    return Err("shared image memory requirements do not match the exporter".into());
                }
                identity = *info;
            }
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut export = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let mut import_info = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD)
                .fd(import.as_ref().map(|(_, fd)| fd.as_raw_fd()).unwrap_or(-1));
            let mut allocation = vk::MemoryAllocateInfo::default()
                .allocation_size(identity.allocation_size)
                .memory_type_index(identity.memory_type_index)
                .push_next(&mut dedicated);
            allocation = if import.is_some() {
                allocation.push_next(&mut import_info)
            } else {
                allocation.push_next(&mut export)
            };
            let memory = unsafe { self.device.allocate_memory(&allocation, None) }
                .map_err(|e| format!("allocate shared image: {e:?}"))?;
            // Vulkan consumes an imported FD on successful allocation, even
            // if a later image bind/view step fails.
            if let Some((_, fd)) = import.take() {
                let _ = fd.into_raw_fd();
            }
            let bind = unsafe { self.device.bind_image_memory(image, memory, 0) };
            if let Err(e) = bind {
                unsafe { self.device.free_memory(memory, None) };
                return Err(format!("bind shared image: {e:?}"));
            }
            let view = match self.shared_view(image) {
                Ok(view) => view,
                Err(error) => {
                    unsafe { self.device.free_memory(memory, None) };
                    return Err(error);
                }
            };
            Ok((
                VulkanTextureResource {
                    image,
                    memory,
                    view,
                    face_views: [vk::ImageView::null(); 6],
                    width,
                    height,
                    layers: 1,
                    is_cube: false,
                    format: vk::Format::B8G8R8A8_UNORM,
                    layout: if is_imported {
                        vk::ImageLayout::GENERAL
                    } else {
                        vk::ImageLayout::UNDEFINED
                    },
                    sampler: None,
                    ycbcr_conversion: None,
                    owns_image: true,
                },
                identity,
            ))
        })();
        if result.is_err() {
            unsafe { self.device.destroy_image(image, None) };
        }
        result
    }

    fn shared_barrier(&self, image: vk::Image, acquire: bool) {
        let barrier = vk::ImageMemoryBarrier::default()
            .src_queue_family_index(if acquire {
                vk::QUEUE_FAMILY_EXTERNAL
            } else {
                self.queue_family_index
            })
            .dst_queue_family_index(if acquire {
                self.queue_family_index
            } else {
                vk::QUEUE_FAMILY_EXTERNAL
            })
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_access_mask(if acquire {
                vk::AccessFlags::empty()
            } else {
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
            })
            .dst_access_mask(if acquire {
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
            } else {
                vk::AccessFlags::empty()
            })
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                if acquire {
                    vk::PipelineStageFlags::TOP_OF_PIPE
                } else {
                    vk::PipelineStageFlags::ALL_COMMANDS
                },
                if acquire {
                    vk::PipelineStageFlags::ALL_COMMANDS
                } else {
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE
                },
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }

    pub(super) fn export_shared_image(
        &mut self,
        texture: &Texture,
        width: u32,
        height: u32,
    ) -> Result<LinuxOwnedImage, String> {
        let key = Self::texture_key(texture.texture_id());
        if !self.desktop.shared.images.contains_key(&key) {
            let initialized = (|| -> Result<(), String> {
                let (resource, info) = self.create_shared_resource(width, height, None)?;
                self.textures.insert(key, resource);
                let memory_loader =
                    ash::khr::external_memory_fd::Device::new(&self.instance, &self.device);
                let memory_fd = unsafe {
                    memory_loader.get_memory_fd(
                        &vk::MemoryGetFdInfoKHR::default()
                            .memory(self.textures[&key].memory)
                            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
                    )
                }
                .map_err(|e| format!("export image memory FD: {e:?}"))?;
                let memory_fd = unsafe { OwnedFd::from_raw_fd(memory_fd) };
                let mut export = vk::ExportSemaphoreCreateInfo::default()
                    .handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
                let mut timeline = vk::SemaphoreTypeCreateInfo::default()
                    .semaphore_type(vk::SemaphoreType::TIMELINE);
                let semaphore = unsafe {
                    self.device.create_semaphore(
                        &vk::SemaphoreCreateInfo::default()
                            .push_next(&mut export)
                            .push_next(&mut timeline),
                        None,
                    )
                }
                .map_err(|e| format!("create shared timeline: {e:?}"))?;
                let semaphore_loader =
                    ash::khr::external_semaphore_fd::Device::new(&self.instance, &self.device);
                let semaphore_fd = match unsafe {
                    semaphore_loader.get_semaphore_fd(
                        &vk::SemaphoreGetFdInfoKHR::default()
                            .semaphore(semaphore)
                            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD),
                    )
                } {
                    Ok(fd) => unsafe { OwnedFd::from_raw_fd(fd) },
                    Err(e) => {
                        unsafe { self.device.destroy_semaphore(semaphore, None) };
                        return Err(format!("export timeline FD: {e:?}"));
                    }
                };
                self.desktop.shared.images.insert(
                    key,
                    SharedImage {
                        semaphore,
                        memory_fd: Some(memory_fd),
                        semaphore_fd: Some(semaphore_fd),
                        info,
                        producer: false,
                        next_write: INITIAL_WRITE,
                        last_sequence: 0,
                        frame: None,
                    },
                );
                unsafe {
                    self.device
                        .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                        .map_err(|e| format!("wait before image export: {e:?}"))?;
                }
                self.destroy_frame_resources();
                unsafe {
                    self.device
                        .reset_command_buffer(
                            self.command_buffer,
                            vk::CommandBufferResetFlags::empty(),
                        )
                        .map_err(|e| format!("reset image export commands: {e:?}"))?;
                    self.device
                        .begin_command_buffer(
                            self.command_buffer,
                            &vk::CommandBufferBeginInfo::default(),
                        )
                        .map_err(|e| format!("begin image export commands: {e:?}"))?;
                }
                let image = self.textures[&key].image;
                self.transition_image_layout(
                    image,
                    vk::ImageAspectFlags::COLOR,
                    1,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                );
                self.shared_barrier(image, false);
                self.desktop.shared.signals.push((semaphore, INITIAL_WRITE));
                unsafe { self.device.end_command_buffer(self.command_buffer) }
                    .map_err(|e| format!("finish image export commands: {e:?}"))?;
                self.submit_frame(
                    &vk::SubmitInfo::default().command_buffers(&[self.command_buffer]),
                )?;
                unsafe {
                    self.device
                        .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                }
                .map_err(|e| format!("complete image export: {e:?}"))?;
                self.textures.get_mut(&key).unwrap().layout = vk::ImageLayout::GENERAL;
                crate::log!(
                    "Vulkan shared image: exported {}x{} device-local image and timeline",
                    width,
                    height
                );
                Ok(())
            })();
            if let Err(error) = initialized {
                self.device_wait_idle();
                self.desktop.shared.waits.clear();
                self.desktop.shared.signals.clear();
                self.retire_shared_texture(key);
                if let Some(resource) = self.textures.remove(&key) {
                    self.destroy_texture_resource(resource);
                }
                return Err(error);
            }
        }
        let shared = &self.desktop.shared.images[&key];
        Ok(LinuxOwnedImage {
            drm_format: crate::os::linux::dma_buf::DrmFormat {
                fourcc: 0,
                modifiers: 0,
            },
            plane: LinuxOwnedImagePlane {
                dma_buf_fd: shared
                    .memory_fd
                    .as_ref()
                    .unwrap()
                    .as_fd()
                    .try_clone_to_owned()
                    .map_err(|e| e.to_string())?,
                offset: 0,
                stride: 0,
            },
            vulkan: Some(LinuxOwnedVulkanImage {
                info: shared.info,
                semaphore_fd: shared
                    .semaphore_fd
                    .as_ref()
                    .unwrap()
                    .as_fd()
                    .try_clone_to_owned()
                    .map_err(|e| e.to_string())?,
            }),
        })
    }
}

#[cfg(linux_direct)]
impl CxVulkan {
    pub(super) fn shared_readers_drained(&self) -> bool {
        self.desktop.shared.frames.is_empty()
            && self.desktop.shared.retired.is_empty()
            && self.desktop.shared.snapshots.is_empty()
            && !self.desktop.shared.quiesce_pending
            && self.desktop.shared.waits.is_empty()
            && self.desktop.shared.signals.is_empty()
    }

    pub(super) fn resume_shared_frames(&mut self) -> Result<(), String> {
        if self.desktop.shared.quiesce_pending || self.in_flight_fence == vk::Fence::null() {
            return Err("cannot resume while shared GPU retirement is incomplete".into());
        }
        self.desktop.shared.frozen = false;
        Ok(())
    }

    /// Cancel only work already submitted. Allocation failure before a
    /// snapshot submission must not require retrying that same allocation.
    pub(super) fn poll_shared_cancel(&mut self, cx: &mut Cx) -> Result<bool, String> {
        if self.desktop.shared.quiesce_pending {
            return self.quiesce_shared_frames(cx);
        }
        if self.in_flight_fence == vk::Fence::null() {
            return Err("cannot resume a renderer with no submission fence".into());
        }
        unsafe { self.device.get_fence_status(self.in_flight_fence) }
            .map_err(|error| format!("poll cancelled renderer: {error:?}"))
    }

    /// Stop new shared reads/writes, retain every visible/preview frame as an
    /// owned GPU image, and return the external leases after its copy completes.
    /// A controller must stop ordinary render submissions while this polls.
    /// The alias Texture IDs remain valid, so CPU-held previews need no reset.
    pub(super) fn quiesce_shared_frames(&mut self, cx: &mut Cx) -> Result<bool, String> {
        self.desktop.shared.frozen = true;
        if self.in_flight_fence == vk::Fence::null() {
            return Err("shared GPU quiescence cannot recover a missing submission fence".into());
        }
        if !unsafe { self.device.get_fence_status(self.in_flight_fence) }
            .map_err(|error| format!("poll GPU quiescence: {error:?}"))?
        {
            return Ok(false);
        }
        if self.desktop.shared.quiesce_pending {
            for (key, source_key, resource) in std::mem::take(&mut self.desktop.shared.snapshots) {
                if let Some(old) = self.textures.remove(&key) {
                    self.destroy_texture_resource(old);
                }
                self.desktop.shared.frames.remove(&key);
                if let Some(shared) = self.desktop.shared.images.get_mut(&source_key) {
                    shared.frame = None;
                }
                let live = cx.textures.0.pool.get(key.0 .0).is_some_and(|slot| {
                    TextureId::from_pool_slot(key.0 .0, slot.generation) == key.0
                        && !cx.textures.0.is_free(key.0 .0)
                });
                if !live {
                    self.destroy_texture_resource(resource);
                    continue;
                }
                let texture = &mut cx.textures[key.0];
                if matches!(texture.format, TextureFormat::SharedBGRAu8 { .. }) {
                    texture.format = TextureFormat::RenderBGRAu8 {
                        size: crate::texture::TextureSize::Fixed {
                            width: resource.width as usize,
                            height: resource.height as usize,
                        },
                        initial: false,
                    };
                }
                texture.alloc = Some(crate::texture::TextureAlloc {
                    category: TextureCategory::Render,
                    pixel: TexturePixel::BGRAu8,
                    width: resource.width as usize,
                    height: resource.height as usize,
                });
                self.textures.insert(key, resource);
            }
            self.desktop.shared.retired.clear();
            self.desktop.shared.quiesce_pending = false;
            return Ok(self.shared_readers_drained());
        }
        if !self.desktop.shared.waits.is_empty() || !self.desktop.shared.signals.is_empty() {
            return Err("cannot quiesce an unsubmitted shared-frame transaction".into());
        }
        self.prune_stale_geometry_resources(cx);
        if self.shared_readers_drained() {
            return Ok(true);
        }

        let mut snapshots = Vec::new();
        let mut releases = Vec::new();
        let prepared = (|| -> Result<(), String> {
            for (&key, lease) in &self.desktop.shared.frames {
                let source_key = Self::texture_key(lease.source.texture_id());
                let original = self
                    .textures
                    .get(&key)
                    .ok_or("shared alias has no Vulkan resource")?;
                let shared = self
                    .desktop
                    .shared
                    .images
                    .get(&source_key)
                    .ok_or("shared alias has no owner")?;
                let mut resource = self.create_color_target_resource(
                    original.width,
                    original.height,
                    original.format,
                    false,
                )?;
                resource.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
                releases.push((
                    original.image,
                    shared.semaphore,
                    lease.sequence,
                    lease.acquired,
                    Some(resource.image),
                ));
                snapshots.push((key, source_key, resource));
            }
            for lease in &self.desktop.shared.retired {
                let key = Self::texture_key(lease.source.texture_id());
                let resource = self
                    .textures
                    .get(&key)
                    .ok_or("retired shared frame has no resource")?;
                let shared = self
                    .desktop
                    .shared
                    .images
                    .get(&key)
                    .ok_or("retired shared frame has no owner")?;
                releases.push((
                    resource.image,
                    shared.semaphore,
                    lease.sequence,
                    lease.acquired,
                    None,
                ));
            }
            let mut semaphores = HashSet::new();
            if releases
                .iter()
                .any(|(_, semaphore, _, _, _)| !semaphores.insert(*semaphore))
            {
                return Err("multiple outstanding leases name the same shared image".into());
            }
            unsafe {
                self.device
                    .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                    .map_err(|error| format!("reset shared snapshot commands: {error:?}"))?;
                self.device
                    .begin_command_buffer(
                        self.command_buffer,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )
                    .map_err(|error| format!("begin shared snapshot commands: {error:?}"))?;
            }
            let mut waits = Vec::new();
            let mut wait_values = Vec::new();
            let mut signals = Vec::new();
            let mut signal_values = Vec::new();
            for (image, semaphore, sequence, acquired, destination) in &releases {
                if !acquired {
                    waits.push(*semaphore);
                    wait_values.push(*sequence);
                    self.shared_barrier(*image, true);
                }
                if let Some(destination) = destination {
                    let resource = snapshots
                        .iter()
                        .find(|(_, _, resource)| resource.image == *destination)
                        .map(|(_, _, resource)| resource)
                        .ok_or("shared snapshot target disappeared")?;
                    self.transition_image_layout(
                        *destination,
                        vk::ImageAspectFlags::COLOR,
                        1,
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    );
                    let layers = vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1);
                    unsafe {
                        self.device.cmd_copy_image(
                            self.command_buffer,
                            *image,
                            vk::ImageLayout::GENERAL,
                            *destination,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[vk::ImageCopy::default()
                                .src_subresource(layers)
                                .dst_subresource(layers)
                                .extent(vk::Extent3D {
                                    width: resource.width,
                                    height: resource.height,
                                    depth: 1,
                                })],
                        )
                    };
                    self.transition_image_layout(
                        *destination,
                        vk::ImageAspectFlags::COLOR,
                        1,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    );
                }
                self.shared_barrier(*image, false);
                signals.push(*semaphore);
                signal_values.push(*sequence + 1);
            }
            unsafe { self.device.end_command_buffer(self.command_buffer) }
                .map_err(|error| format!("end shared snapshot commands: {error:?}"))?;
            let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
            let mut timeline = vk::TimelineSemaphoreSubmitInfo::default()
                .wait_semaphore_values(&wait_values)
                .signal_semaphore_values(&signal_values);
            let command_buffers = [self.command_buffer];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&waits)
                .wait_dst_stage_mask(&stages)
                .signal_semaphores(&signals)
                .command_buffers(&command_buffers)
                .push_next(&mut timeline);
            unsafe { self.device.reset_fences(&[self.in_flight_fence]) }
                .map_err(|error| format!("reset shared snapshot fence: {error:?}"))?;
            if let Err(error) = unsafe {
                self.device
                    .queue_submit(self.queue, &[submit], self.in_flight_fence)
            } {
                self.device_wait_idle();
                unsafe {
                    self.device.destroy_fence(self.in_flight_fence, None);
                    self.in_flight_fence = vk::Fence::null();
                    self.in_flight_fence = self
                        .device
                        .create_fence(
                            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                            None,
                        )
                        .map_err(|recovery| {
                            format!("shared snapshot submit {error:?}; fence recovery {recovery:?}")
                        })?;
                }
                return Err(format!("submit shared snapshots: {error:?}"));
            }
            Ok(())
        })();
        if let Err(error) = prepared {
            for (_, _, resource) in snapshots {
                self.destroy_texture_resource(resource);
            }
            return Err(error);
        }
        self.desktop.shared.snapshots = snapshots;
        // Retirement-only batches also retain their source pins until the
        // fence completes; an empty snapshot list is not a completion signal.
        self.desktop.shared.quiesce_pending = true;
        Ok(false)
    }
}
