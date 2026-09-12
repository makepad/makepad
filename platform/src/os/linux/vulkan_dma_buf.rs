//! Cross-device image primitives. Unlike OPAQUE_FD, these handles may cross
//! Vulkan drivers. Callers negotiate a modifier and usages before allocating,
//! and pair every foreign ownership transfer with a one-shot sync file.
//! No function in this module maps or copies pixels through the CPU.
//!
//! # Safety
//! All Vulkan-taking functions require live handles from the specified instance
//! and device, with REQUIRED_EXTENSIONS enabled on logical devices. The caller
//! owns Vulkan external synchronization. Command recording requires a recording
//! buffer from the named queue family; image/semaphore use and destruction must
//! obey their pending GPU-use lifetimes. Sync-file export requires a submitted
//! signal, and import requires a semaphore with no pending use.
use ash::{vk, Device, Instance};
use std::{
    ffi::CStr,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
};

pub(super) const FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;
pub(super) const DRM_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
pub(super) const REQUIRED_EXTENSIONS: [&CStr; 6] = [
    vk::KHR_EXTERNAL_MEMORY_FD_NAME,
    vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
    vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
    // DesktopInit requests Vulkan 1.1. This modifier-extension dependency
    // is only core in 1.2, even when the physical device supports newer APIs.
    vk::KHR_IMAGE_FORMAT_LIST_NAME,
    vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
    vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DeviceIdentity {
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ModifierCaps {
    pub modifier: u64,
    pub features: vk::FormatFeatureFlags,
    pub external: vk::ExternalMemoryProperties,
    pub limits: vk::ImageFormatProperties,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ImageLayout {
    pub width: u32,
    pub height: u32,
    pub modifier: u64,
    pub offset: u64,
    pub row_pitch: u64,
    pub allocation_size: u64,
}

impl ImageLayout {
    fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.allocation_size == 0
            || self.row_pitch == 0
            || self.offset >= self.allocation_size
        {
            return Err("invalid DMA-BUF dimensions or memory layout".into());
        }
        if self.modifier == 0 {
            let row_bytes = u64::from(self.width) * 4;
            let end = self
                .row_pitch
                .checked_mul(u64::from(self.height - 1))
                .and_then(|size| size.checked_add(self.offset))
                .and_then(|size| size.checked_add(row_bytes));
            if self.row_pitch < row_bytes || end.is_none_or(|end| end > self.allocation_size) {
                return Err("linear DMA-BUF exceeds its allocation".into());
            }
        }
        Ok(())
    }
}

/// Explicit destruction follows completion of ALL GPU work using this image.
/// Views are owned by the caller and must be destroyed first. Keeping this
/// separate from a Texture permits transfer-only images without sampled views.
pub(super) struct Image {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub layout: ImageLayout,
}

impl Image {
    /// The owning device must still be live, with no pending use or views.
    pub unsafe fn destroy(self, device: &Device) {
        unsafe {
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}

/// `None` is Vulkan's already-signaled sync_file sentinel (-1), not a Unix FD.
pub(super) struct SyncFile(pub Option<OwnedFd>);

pub(super) unsafe fn identity(instance: &Instance, physical: vk::PhysicalDevice) -> DeviceIdentity {
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
    unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
    DeviceIdentity {
        device_uuid: id.device_uuid,
        driver_uuid: id.driver_uuid,
    }
}

pub(super) unsafe fn supports_extensions(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> bool {
    let Ok(extensions) = (unsafe { instance.enumerate_device_extension_properties(physical) })
    else {
        return false;
    };
    REQUIRED_EXTENSIONS.iter().all(|name| {
        extensions
            .iter()
            .any(|extension| unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) == *name })
    })
}

pub(super) unsafe fn supports_sync_files(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> bool {
    let query = vk::PhysicalDeviceExternalSemaphoreInfo::default()
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalSemaphoreProperties::default();
    unsafe {
        instance.get_physical_device_external_semaphore_properties(
            physical,
            &query,
            &mut properties,
        )
    };
    properties.external_semaphore_features.contains(
        vk::ExternalSemaphoreFeatureFlags::IMPORTABLE
            | vk::ExternalSemaphoreFeatureFlags::EXPORTABLE,
    )
}

pub(super) unsafe fn modifiers(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    usage: vk::ImageUsageFlags,
) -> Result<Vec<ModifierCaps>, String> {
    if !unsafe {
        supports_extensions(instance, physical) && supports_sync_files(instance, physical)
    } {
        return Err(
            "GPU lacks DMA-BUF/modifier/foreign ownership or binary sync-file support".into(),
        );
    }
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut format = vk::FormatProperties2::default().push_next(&mut list);
    unsafe { instance.get_physical_device_format_properties2(physical, FORMAT, &mut format) };
    let mut items = vec![
        vk::DrmFormatModifierPropertiesEXT::default();
        list.drm_format_modifier_count as usize
    ];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
        .drm_format_modifier_properties(&mut items);
    let mut format = vk::FormatProperties2::default().push_next(&mut list);
    unsafe { instance.get_physical_device_format_properties2(physical, FORMAT, &mut format) };
    let count = (list.drm_format_modifier_count as usize).min(items.len());
    let mut result = Vec::new();
    for item in &items[..count] {
        if item.drm_format_modifier_plane_count != 1 {
            continue;
        }
        let mut external_query = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut modifier_query = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(item.drm_format_modifier)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let query = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(FORMAT)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .push_next(&mut external_query)
            .push_next(&mut modifier_query);
        let mut external = vk::ExternalImageFormatProperties::default();
        let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
        match unsafe {
            instance.get_physical_device_image_format_properties2(physical, &query, &mut properties)
        } {
            Ok(()) => result.push(ModifierCaps {
                modifier: item.drm_format_modifier,
                features: item.drm_format_modifier_tiling_features,
                limits: properties.image_format_properties,
                external: external.external_memory_properties,
            }),
            Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED) => {}
            Err(error) => {
                return Err(format!(
                    "query DMA-BUF modifier {:#x}: {error:?}",
                    item.drm_format_modifier
                ))
            }
        }
    }
    Ok(result)
}

fn image_info<'a>(width: u32, height: u32, usage: vk::ImageUsageFlags) -> vk::ImageCreateInfo<'a> {
    vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(FORMAT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
}

unsafe fn memory_type(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    bits: u32,
) -> Result<u32, String> {
    let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
    let types = &memory.memory_types[..memory.memory_type_count as usize];
    let suitable = |index: usize, ty: &vk::MemoryType| {
        bits & (1 << index) != 0
            && !ty
                .property_flags
                .contains(vk::MemoryPropertyFlags::PROTECTED)
    };
    types
        .iter()
        .enumerate()
        .find(|(index, ty)| {
            suitable(*index, ty)
                && ty
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .or_else(|| {
            types
                .iter()
                .enumerate()
                .find(|(index, ty)| suitable(*index, ty))
        })
        .map(|(index, _)| index as u32)
        .ok_or_else(|| "DMA-BUF has no compatible local memory type".into())
}

/// Allocate on the exporting GPU. The minimum size can include requirements
/// negotiated with the importing device; no memory-type index crosses GPUs.
pub(super) unsafe fn allocate(
    instance: &Instance,
    device: &Device,
    physical: vk::PhysicalDevice,
    width: u32,
    height: u32,
    modifier: u64,
    usage: vk::ImageUsageFlags,
    minimum_allocation_size: u64,
    required_layout: Option<ImageLayout>,
) -> Result<(Image, OwnedFd), String> {
    if let Some(layout) = required_layout {
        layout.validate()?;
        if layout.width != width || layout.height != height || layout.modifier != modifier {
            return Err("explicit DMA-BUF allocation changed its image identity".into());
        }
    }
    let cap = unsafe { modifiers(instance, physical, usage) }?
        .into_iter()
        .find(|cap| {
            cap.modifier == modifier
                && cap
                    .external
                    .external_memory_features
                    .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
        })
        .ok_or_else(|| format!("modifier {modifier:#x} is not exportable for {usage:?}"))?;
    if width == 0
        || height == 0
        || width > cap.limits.max_extent.width
        || height > cap.limits.max_extent.height
    {
        return Err("DMA-BUF dimensions exceed the exporting GPU's limits".into());
    }
    let choices = [modifier];
    let mut modifier_info =
        vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&choices);
    let planes = [vk::SubresourceLayout::default()
        .offset(required_layout.map_or(0, |layout| layout.offset))
        .row_pitch(required_layout.map_or(0, |layout| layout.row_pitch))];
    let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(modifier)
        .plane_layouts(&planes);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = image_info(width, height, usage).push_next(&mut external);
    let info = if required_layout.is_some() {
        info.push_next(&mut explicit)
    } else {
        info.push_next(&mut modifier_info)
    };
    let image = unsafe { device.create_image(&info, None) }
        .map_err(|e| format!("create exported DMA-BUF image: {e:?}"))?;
    let mut memory = vk::DeviceMemory::null();
    let result = (|| {
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let size = requirements.size.max(minimum_allocation_size);
        let index = unsafe { memory_type(instance, physical, requirements.memory_type_bits) }?;
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let allocation = vk::MemoryAllocateInfo::default()
            .allocation_size(size)
            .memory_type_index(index)
            .push_next(&mut dedicated)
            .push_next(&mut export);
        memory = unsafe { device.allocate_memory(&allocation, None) }
            .map_err(|e| format!("allocate exported DMA-BUF: {e:?}"))?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .map_err(|e| format!("bind exported DMA-BUF: {e:?}"))?;
        let loader = ash::ext::image_drm_format_modifier::Device::new(instance, device);
        let mut selected = vk::ImageDrmFormatModifierPropertiesEXT::default();
        unsafe { loader.get_image_drm_format_modifier_properties(image, &mut selected) }
            .map_err(|e| format!("read allocated DRM modifier: {e:?}"))?;
        if selected.drm_format_modifier != modifier {
            return Err("driver selected an unnegotiated DRM modifier".into());
        }
        let plane = unsafe {
            device.get_image_subresource_layout(
                image,
                vk::ImageSubresource::default()
                    .aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT),
            )
        };
        let layout = ImageLayout {
            width,
            height,
            modifier,
            offset: plane.offset,
            row_pitch: plane.row_pitch,
            allocation_size: size,
        };
        layout.validate()?;
        if let Some(required) = required_layout {
            check_plane_layout(layout, required, "exporter")?;
        }
        let loader = ash::khr::external_memory_fd::Device::new(instance, device);
        let raw = unsafe {
            loader.get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
        }
        .map_err(|e| format!("export DMA-BUF FD: {e:?}"))?;
        if raw < 0 {
            return Err("driver returned an invalid DMA-BUF FD".into());
        }
        Ok((
            Image {
                image,
                memory,
                layout,
            },
            unsafe { OwnedFd::from_raw_fd(raw) },
        ))
    })();
    if result.is_err() {
        unsafe {
            device.destroy_image(image, None);
            if memory != vk::DeviceMemory::null() {
                device.free_memory(memory, None);
            }
        }
    }
    result
}

/// Explicit-layout import image only. Does not allocate, import an FD, or
/// compare `VkMemoryRequirements.size` to `layout.allocation_size`.
unsafe fn create_import_image(
    instance: &Instance,
    device: &Device,
    physical: vk::PhysicalDevice,
    layout: ImageLayout,
    usage: vk::ImageUsageFlags,
) -> Result<vk::Image, String> {
    layout.validate()?;
    let cap = unsafe { modifiers(instance, physical, usage) }?
        .into_iter()
        .find(|cap| {
            cap.modifier == layout.modifier
                && cap
                    .external
                    .external_memory_features
                    .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
        })
        .ok_or_else(|| {
            format!(
                "modifier {:#x} is not importable for {usage:?}",
                layout.modifier
            )
        })?;
    if layout.width > cap.limits.max_extent.width || layout.height > cap.limits.max_extent.height {
        return Err("DMA-BUF dimensions exceed the importing GPU's limits".into());
    }
    let planes = [vk::SubresourceLayout::default()
        .offset(layout.offset)
        .row_pitch(layout.row_pitch)];
    let mut modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(layout.modifier)
        .plane_layouts(&planes);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = image_info(layout.width, layout.height, usage)
        .push_next(&mut modifier)
        .push_next(&mut external);
    unsafe { device.create_image(&info, None) }
        .map_err(|e| format!("create imported DMA-BUF image: {e:?}"))
}

fn check_plane_layout(
    actual: ImageLayout,
    expected: ImageLayout,
    role: &str,
) -> Result<(), String> {
    if actual.offset != expected.offset || actual.row_pitch != expected.row_pitch {
        return Err(format!(
            "{role} DMA-BUF layout mismatch: requested offset={} pitch={}, reported offset={} pitch={} for {}x{}",
            expected.offset, expected.row_pitch, actual.offset, actual.row_pitch,
            expected.width, expected.height
        ));
    }
    Ok(())
}

/// The image's reported layout, with its own memory requirement as the size.
/// Modifier image layouts can be queried before binding and are invariant.
unsafe fn import_image_layout(
    device: &Device,
    image: vk::Image,
    expected: ImageLayout,
) -> Result<ImageLayout, String> {
    let plane = unsafe {
        device.get_image_subresource_layout(
            image,
            vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT),
        )
    };
    let layout = ImageLayout {
        offset: plane.offset,
        row_pitch: plane.row_pitch,
        allocation_size: unsafe { device.get_image_memory_requirements(image) }.size,
        ..expected
    };
    layout.validate()?;
    Ok(layout)
}

/// Probe the importing driver's actual plane layout and allocation size.
/// Creates and destroys an unbound image; no FD or GPU submission is involved.
pub(super) unsafe fn import_requirements(
    instance: &Instance,
    device: &Device,
    physical: vk::PhysicalDevice,
    layout: ImageLayout,
    usage: vk::ImageUsageFlags,
) -> Result<ImageLayout, String> {
    let image = unsafe { create_import_image(instance, device, physical, layout, usage) }?;
    let reported = unsafe { import_image_layout(device, image, layout) };
    unsafe { device.destroy_image(image, None) };
    if let Ok(actual) = reported.as_ref() {
        crate::log!(
            "Vulkan DMA-BUF import probe: {}x{} modifier={:#x}, exported offset={} pitch={} bytes={}, reader offset={} pitch={} bytes={}",
            layout.width, layout.height, layout.modifier, layout.offset, layout.row_pitch,
            layout.allocation_size, actual.offset, actual.row_pitch, actual.allocation_size
        );
    }
    reported
}

pub(super) unsafe fn import(
    instance: &Instance,
    device: &Device,
    physical: vk::PhysicalDevice,
    layout: ImageLayout,
    usage: vk::ImageUsageFlags,
    fd: OwnedFd,
) -> Result<Image, String> {
    let image = unsafe { create_import_image(instance, device, physical, layout, usage) }?;
    let mut memory = vk::DeviceMemory::null();
    let result = (|| {
        let reported = unsafe { import_image_layout(device, image, layout) }?;
        check_plane_layout(reported, layout, "unbound importer")?;
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        if requirements.size > layout.allocation_size {
            return Err(format!(
                "import needs {} bytes, exporter allocated {}",
                requirements.size, layout.allocation_size
            ));
        }
        let loader = ash::khr::external_memory_fd::Device::new(instance, device);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        unsafe {
            loader.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut fd_properties,
            )
        }
        .map_err(|e| format!("query foreign DMA-BUF memory types: {e:?}"))?;
        let index = unsafe {
            memory_type(
                instance,
                physical,
                requirements.memory_type_bits & fd_properties.memory_type_bits,
            )
        }?;
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd.as_raw_fd());
        let allocation = vk::MemoryAllocateInfo::default()
            .allocation_size(layout.allocation_size)
            .memory_type_index(index)
            .push_next(&mut dedicated)
            .push_next(&mut import);
        memory = unsafe { device.allocate_memory(&allocation, None) }
            .map_err(|e| format!("import DMA-BUF memory: {e:?}"))?;
        // Successful import transfers ownership, including if binding fails.
        let _ = fd.into_raw_fd();
        unsafe { device.bind_image_memory(image, memory, 0) }
            .map_err(|e| format!("bind imported DMA-BUF: {e:?}"))?;
        let bound_layout = unsafe { import_image_layout(device, image, layout) }?;
        check_plane_layout(bound_layout, layout, "bound importer")?;
        Ok(Image {
            image,
            memory,
            layout,
        })
    })();
    if result.is_err() {
        unsafe {
            device.destroy_image(image, None);
            if memory != vk::DeviceMemory::null() {
                device.free_memory(memory, None);
            }
        }
    }
    result
}

pub(super) unsafe fn create_semaphore(device: &Device) -> Result<vk::Semaphore, String> {
    let mut export = vk::ExportSemaphoreCreateInfo::default()
        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    unsafe {
        device.create_semaphore(
            &vk::SemaphoreCreateInfo::default().push_next(&mut export),
            None,
        )
    }
    .map_err(|e| format!("create binary sync-file semaphore: {e:?}"))
}

/// Export only after submitting a signal operation. Export transfers the
/// payload out of this binary semaphore; do not also wait on it locally.
pub(super) unsafe fn export_sync_file(
    instance: &Instance,
    device: &Device,
    semaphore: vk::Semaphore,
) -> Result<SyncFile, String> {
    let loader = ash::khr::external_semaphore_fd::Device::new(instance, device);
    let fd = unsafe {
        loader.get_semaphore_fd(
            &vk::SemaphoreGetFdInfoKHR::default()
                .semaphore(semaphore)
                .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
        )
    }
    .map_err(|e| format!("export sync file: {e:?}"))?;
    match fd {
        -1 => Ok(SyncFile(None)),
        0.. => Ok(SyncFile(Some(unsafe { OwnedFd::from_raw_fd(fd) }))),
        _ => Err("driver returned an invalid sync-file descriptor".into()),
    }
}

/// The semaphore must have no pending use. Import temporarily replaces its
/// binary payload, restored after the queue consumes the imported wait.
pub(super) unsafe fn import_sync_file(
    instance: &Instance,
    device: &Device,
    semaphore: vk::Semaphore,
    sync: SyncFile,
) -> Result<(), String> {
    let loader = ash::khr::external_semaphore_fd::Device::new(instance, device);
    unsafe {
        loader.import_semaphore_fd(
            &vk::ImportSemaphoreFdInfoKHR::default()
                .semaphore(semaphore)
                .flags(vk::SemaphoreImportFlags::TEMPORARY)
                .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                .fd(sync.0.as_ref().map_or(-1, AsRawFd::as_raw_fd)),
        )
    }
    .map_err(|e| format!("import sync file: {e:?}"))?;
    if let Some(fd) = sync.0 {
        let _ = fd.into_raw_fd();
    }
    Ok(())
}

/// Call inside a command buffer with a submitted sync-file wait/signal.
/// The shared external layout is always GENERAL. These barriers alone do
/// not synchronize with the other GPU; the caller owns that dependency.
pub(super) unsafe fn ownership_barrier(
    device: &Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    queue_family: u32,
    acquire: bool,
    local_layout: vk::ImageLayout,
    local_stage: vk::PipelineStageFlags,
    local_access: vk::AccessFlags,
) {
    let mut barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        );
    let (source_stage, destination_stage) = if acquire {
        barrier = barrier
            .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .dst_queue_family_index(queue_family)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(local_layout)
            .dst_access_mask(local_access);
        (vk::PipelineStageFlags::TOP_OF_PIPE, local_stage)
    } else {
        barrier = barrier
            .src_queue_family_index(queue_family)
            .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .old_layout(local_layout)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_access_mask(local_access);
        (local_stage, vk::PipelineStageFlags::BOTTOM_OF_PIPE)
    };
    unsafe {
        device.cmd_pipeline_barrier(
            command,
            source_stage,
            destination_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        )
    };
}
