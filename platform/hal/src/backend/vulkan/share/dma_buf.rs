//! Modifier-preserving DMA-BUF imports/exports. Foreign surfaces need only sampled
//! usage; hosted surfaces also need render/copy usage and an explicit timeline lease.

use super::*;
use crate::backend::vulkan::device::error;
use crate::backend::vulkan::resources::{image_usage, vk_format};
use crate::types::{format, usage};
use makepad_vulkan::vk;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

#[derive(Clone, Copy, Debug)]
pub struct OpaqueMemory {
    pub memory_type_index: u32,
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

#[derive(Clone, Debug)]
pub struct DmaBufSupport {
    pub render_device: u64,
    pub render_node: std::path::PathBuf,
    pub formats: Vec<(u32, u64)>,
}

pub const AR24: u32 = u32::from_le_bytes(*b"AR24");
pub const XR24: u32 = u32::from_le_bytes(*b"XR24");

pub struct DmaBuf {
    pub fd: OwnedFd,
    pub width: u32,
    pub height: u32,
    pub modifier: u64,
    pub offset: u64,
    pub row_pitch: u64,
    pub allocation_size: u64,
    pub fourcc: u32,
    /// Foreign compositor inputs require SAMPLED only (some modifiers cannot render).
    pub sample_only: bool,
    /// OPAQUE_FD memory from an existing native hosted parent. Never treat it as DMA-BUF.
    pub opaque: Option<OpaqueMemory>,
    /// OPAQUE_FD timeline: even values grant a write, odd values publish a frame.
    pub timeline_fd: Option<OwnedFd>,
}

impl DmaBuf {
    pub(super) fn extensions() -> Vec<&'static CStr> { vec![vk::KHR_EXTERNAL_MEMORY_FD_NAME, vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
        vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME, vk::KHR_IMAGE_FORMAT_LIST_NAME, vk::EXT_QUEUE_FAMILY_FOREIGN_NAME] }

    pub(super) fn supported(dev: &Device) -> Result<(), String> {
        if Self::extensions().iter().all(|e| dev.supports(e)) { Ok(()) } else { Err("device lacks DMA-BUF modifier sharing".into()) }
    }

    fn usage(desc: &TextureObj) -> vk::ImageUsageFlags {
        if desc.usage == usage::SAMPLED { vk::ImageUsageFlags::SAMPLED } else { image_usage(desc) }
    }

    fn memory_type(dev: &Device, bits: u32) -> Result<u32, String> {
        let types = &dev.memory.memory_types[..dev.memory.memory_type_count as usize];
        let eligible = |i: usize, ty: &vk::MemoryType| bits & (1 << i) != 0
            && !ty.property_flags.contains(vk::MemoryPropertyFlags::PROTECTED);
        types.iter().enumerate()
            .find(|(i, ty)| eligible(*i, ty) && ty.property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL))
            .or_else(|| types.iter().enumerate().find(|(i, ty)| eligible(*i, ty)))
            .map(|(i, _)| i as u32).ok_or("no unprotected DMA-BUF memory type".into())
    }

    pub(super) fn properties(dev: &Device, desc: &TextureObj, modifier: u64) -> Result<vk::ExternalMemoryProperties, String> {
        let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default().drm_format_modifier(modifier).sharing_mode(vk::SharingMode::EXCLUSIVE);
        let query = vk::PhysicalDeviceImageFormatInfo2::default().format(vk_format(desc.format).unwrap()).ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT).usage(Self::usage(desc))
            .next_physical_device_external_image_format_info(&mut external).next_physical_device_image_drm_format_modifier_info_ext(&mut drm);
        let mut memory = vk::ExternalImageFormatProperties::default();
        let mut result = vk::ImageFormatProperties2::default().next_external_image_format_properties(&mut memory);
        unsafe { dev.instance.get_physical_device_image_format_properties2(dev.physical, &query, &mut result) }.map_err(error)?;
        if desc.w > result.image_format_properties.max_extent.width || desc.h > result.image_format_properties.max_extent.height { return Err("DMA-BUF dimensions exceed device limits".into()); }
        Ok(memory.external_memory_properties)
    }

    fn image_info(desc: &TextureObj) -> vk::ImageCreateInfo {
        vk::ImageCreateInfo::default().image_type(vk::ImageType::TYPE_2D).format(vk::Format::B8G8R8A8_UNORM)
            .extent(vk::Extent3D { width: desc.w, height: desc.h, depth: 1 }).mip_levels(1).array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1).tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(Self::usage(desc)).sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED)
    }

    /// Size a DMA-BUF without mapping its pixels. lseek is the kernel DMA-BUF size API.
    pub fn size(fd: &OwnedFd) -> Result<u64, String> {
        unsafe extern "C" { fn lseek(fd: i32, offset: i64, whence: i32) -> i64; }
        let size = unsafe { lseek(fd.as_raw_fd(), 0, 2) };
        if size <= 0 { return Err(format!("cannot size DMA-BUF: {}", std::io::Error::last_os_error())); }
        Ok(size as u64)
    }

    pub(super) fn modifiers(dev: &Device) -> Vec<vk::DrmFormatModifierPropertiesEXT> {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut props = vk::FormatProperties2::default().next_drm_format_modifier_properties_list_ext(&mut list);
        unsafe { dev.instance.get_physical_device_format_properties2(dev.physical, vk::Format::B8G8R8A8_UNORM, &mut props); }
        let mut items = vec![vk::DrmFormatModifierPropertiesEXT::default(); list.drm_format_modifier_count as usize];
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default().drm_format_modifier_properties(&mut items);
        let mut props = vk::FormatProperties2::default().next_drm_format_modifier_properties_list_ext(&mut list);
        unsafe { dev.instance.get_physical_device_format_properties2(dev.physical, vk::Format::B8G8R8A8_UNORM, &mut props); }
        items.truncate(list.drm_format_modifier_count as usize);
        items
    }

    pub(super) fn support(dev: &Device) -> Option<DmaBufSupport> {
        Self::supported(dev).ok()?;
        if !super::sync::sync_files(dev) { return None; }
        let have = unsafe { dev.instance.enumerate_device_extension_properties(dev.physical) }.ok()?;
        let has = |name| have.iter().any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) } == name);
        let (render_node, render_device) = {
            if !has(vk::EXT_PCI_BUS_INFO_NAME) { return None; }
            let mut pci = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
            let mut props = vk::PhysicalDeviceProperties2::default().next_physical_device_pci_bus_info_properties_ext(&mut pci);
            unsafe { dev.instance.get_physical_device_properties2(dev.physical, &mut props); }
            let dir = format!("/sys/bus/pci/devices/{:04x}:{:02x}:{:02x}.{:x}/drm", pci.pci_domain, pci.pci_bus, pci.pci_device, pci.pci_function);
            std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("renderD") { return None; }
                let value = std::fs::read_to_string(entry.path().join("dev")).ok()?;
                let (major, minor) = value.trim().split_once(':')?;
                Some((std::path::Path::new("/dev/dri").join(name), dev_number(major.parse().ok()?, minor.parse().ok()?)))
            })?
        };
        let desc = TextureObj { format: format::BGRA8, w: 1, h: 1, depth: 1, mips: 1,
            usage: usage::SAMPLED, bytes: 0, imported: true, uploaded: 0, lent: None };
        let mut formats = Vec::new();
        for modifier in Self::modifiers(dev) {
            if modifier.drm_format_modifier_plane_count == 1
                && modifier.drm_format_modifier_tiling_features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
                && Self::properties(dev, &desc, modifier.drm_format_modifier).is_ok_and(|p| p.external_memory_features.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)) {
                formats.extend([(AR24, modifier.drm_format_modifier), (XR24, modifier.drm_format_modifier)]);
            }
        }
        (!formats.is_empty()).then_some(DmaBufSupport { render_node, render_device, formats })
    }

    pub(super) fn allocate(dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Image>, String> {
        Self::supported(&dev)?;
        if desc.w == 0 || desc.h == 0 || desc.format != format::BGRA8 || desc.depth != 1 || desc.mips != 1 { return Err("shared DMA-BUF images are single-layer BGRA8".into()); }
        let modifier = Self::modifiers(&dev).iter().filter(|m| m.drm_format_modifier_plane_count == 1)
            .find(|m| Self::properties(&dev, &desc, m.drm_format_modifier).is_ok_and(|p| p.external_memory_features.contains(
                vk::ExternalMemoryFeatureFlags::EXPORTABLE | vk::ExternalMemoryFeatureFlags::IMPORTABLE)))
            .map(|m| m.drm_format_modifier).ok_or("no exportable/importable single-plane BGRA8 modifier")?;
        let timeline = if super::sync::timeline_supported(&dev) { Some(super::sync::Timeline::create(dev.clone())?) } else { None };
        let choices = [modifier];
        let mut drm = vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&choices);
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = Self::image_info(&desc).next_image_drm_format_modifier_list_create_info_ext(&mut drm).next_external_memory_image_create_info(&mut external);
        let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
        let mut memory = vk::DeviceMemory::null();
        let made = (|| {
            let req = unsafe { dev.raw.get_image_memory_requirements(image) };
            let ty = Self::memory_type(&dev, req.memory_type_bits)?;
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty)
                .next_memory_dedicated_allocate_info(&mut dedicated).next_export_memory_allocate_info(&mut export), None) }.map_err(error)?;
            unsafe { dev.raw.bind_image_memory(image, memory, 0) }.map_err(error)?;
            let layout = unsafe { dev.raw.get_image_subresource_layout(image, &vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT)) };
            let raw = unsafe { dev.raw.get_memory_fd(&vk::MemoryGetFdInfoKHR::default().memory(memory).handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)) }.map_err(error)?;
            if raw < 0 { return Err("driver returned invalid DMA-BUF FD".into()); }
            Ok::<_, String>((req.size, Arc::new(Self { fd: unsafe { OwnedFd::from_raw_fd(raw) }, width: desc.w, height: desc.h,
                modifier, offset: layout.offset, row_pitch: layout.row_pitch, allocation_size: req.size,
                fourcc: AR24, sample_only: false, opaque: None,
                timeline_fd: timeline.as_ref().map(|t| t.export()).transpose()? })))
        })();
        match made {
            Ok((bytes, native)) => {
                let mut image = unsafe { Image::from_bound(dev, desc, image, memory, bytes, vk::ImageLayout::UNDEFINED, Some(native)) }?;
                Arc::get_mut(&mut image).unwrap().timeline = timeline;
                Ok(image)
            },
            Err(e) => { unsafe { dev.raw.destroy_image(image, None); dev.raw.free_memory(memory, None); } Err(e) }
        }
    }

    pub(super) fn import(dev: Arc<Device>, native: Arc<Self>) -> Result<Arc<Image>, String> {
        if native.opaque.is_none() { Self::supported(&dev)?; }
        else if !dev.supports(vk::KHR_EXTERNAL_MEMORY_FD_NAME) { return Err("OPAQUE_FD memory import unavailable".into()); }
        if native.fourcc != AR24 && native.fourcc != XR24 { return Err("unsupported DMA-BUF fourcc".into()); }
        if native.width == 0 || native.height == 0 || (native.opaque.is_none() && native.row_pitch == 0) || native.offset >= native.allocation_size { return Err("invalid DMA-BUF layout".into()); }
        if native.opaque.is_none() && native.modifier == 0 && (native.row_pitch < native.width as u64 * 4 || native.row_pitch.checked_mul((native.height - 1) as u64)
            .and_then(|n| n.checked_add(native.offset)).and_then(|n| n.checked_add(native.width as u64 * 4)).is_none_or(|n| n > native.allocation_size)) { return Err("linear DMA-BUF exceeds allocation".into()); }
        let desc = TextureObj { format: format::BGRA8, w: native.width, h: native.height, depth: 1, mips: 1,
            usage: if native.sample_only { usage::SAMPLED } else { usage::SAMPLED | usage::RENDER_TARGET | usage::SHARED }, bytes: 0, imported: true, uploaded: 0, lent: None };
        if native.opaque.is_none() && !Self::properties(&dev, &desc, native.modifier)?.external_memory_features.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE) { return Err("DMA-BUF modifier not importable".into()); }
        let planes = [vk::SubresourceLayout::default().offset(native.offset).row_pitch(native.row_pitch)];
        let mut drm = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default().drm_format_modifier(native.modifier).plane_layouts(&planes);
        let handle_type = if native.opaque.is_some() { vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD } else { vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT };
        if let Some(opaque) = native.opaque {
            let mut ids = vk::PhysicalDeviceIDProperties::default();
            let mut props = vk::PhysicalDeviceProperties2::default().next_physical_device_id_properties(&mut ids);
            unsafe { dev.instance.get_physical_device_properties2(dev.physical, &mut props); }
            if ids.device_uuid != opaque.device_uuid || ids.driver_uuid != opaque.driver_uuid {
                return Err("OPAQUE_FD hosted image belongs to another Vulkan device/driver; a DMA-BUF route is required".into());
            }
        }
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_type);
        let mut info = Self::image_info(&desc).next_external_memory_image_create_info(&mut external);
        info = if native.opaque.is_some() { info.tiling(vk::ImageTiling::OPTIMAL) }
            else { info.next_image_drm_format_modifier_explicit_create_info_ext(&mut drm) };
        let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
        let mut memory = vk::DeviceMemory::null();
        let made = (|| {
            let req = unsafe { dev.raw.get_image_memory_requirements(image) };
            if req.size > native.allocation_size { return Err("import allocation is too small".into()); }
            let fd = native.fd.try_clone().map_err(|e| e.to_string())?;
            let ty = if let Some(opaque) = native.opaque {
                if opaque.memory_type_index >= dev.memory.memory_type_count || req.memory_type_bits & (1 << opaque.memory_type_index) == 0
                    || dev.memory.memory_types[opaque.memory_type_index as usize].property_flags.contains(vk::MemoryPropertyFlags::PROTECTED) {
                    return Err("OPAQUE_FD memory type does not match exporter".into());
                }
                opaque.memory_type_index
            } else {
                let mut properties = vk::MemoryFdPropertiesKHR::default();
                unsafe { dev.raw.get_memory_fd_properties(handle_type, fd.as_raw_fd(), &mut properties) }.map_err(error)?;
                Self::memory_type(&dev, req.memory_type_bits & properties.memory_type_bits)?
            };
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut import = vk::ImportMemoryFdInfoKHR::default().handle_type(handle_type).fd(fd.as_raw_fd());
            memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(native.allocation_size).memory_type_index(ty)
                .next_memory_dedicated_allocate_info(&mut dedicated).next_import_memory_fd_info_khr(&mut import), None) }.map_err(error)?;
            let _ = fd.into_raw_fd();
            unsafe { dev.raw.bind_image_memory(image, memory, 0) }.map_err(error)?;
            if native.opaque.is_none() {
                let actual = unsafe { dev.raw.get_image_subresource_layout(image, &vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT)) };
                if actual.offset != native.offset || actual.row_pitch != native.row_pitch { return Err("imported DMA-BUF plane layout differs".into()); }
            }
            Ok::<_, String>(())
        })();
        if let Err(e) = made { unsafe { dev.raw.destroy_image(image, None); dev.raw.free_memory(memory, None); } return Err(e); }
        let mut image = unsafe { Image::from_bound(dev.clone(), desc, image, memory, 0, vk::ImageLayout::GENERAL, Some(native.clone())) }?;
        if let Some(fd) = &native.timeline_fd {
            Arc::get_mut(&mut image).unwrap().timeline = Some(super::sync::Timeline::import(dev, fd)?);
        }
        image.external_owner.store(true, Ordering::Release);
        Ok(image)
    }
}

fn dev_number(major: u64, minor: u64) -> u64 {
    ((major & 0xfff) << 8) | (minor & 0xff) | ((major & !0xfff) << 32) | ((minor & !0xff) << 12)
}
