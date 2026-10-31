//! AHardwareBuffer import follows native vulkan.rs. The native allocation is retained for
//! the image's whole GPU lifetime; camera YCbCr conversion belongs to the media shell.

use super::*;
use crate::backend::vulkan::device::error;
use crate::backend::vulkan::resources::{image_usage, vk_format};
use crate::types::{format, usage};
use makepad_vulkan::vk;

#[repr(C)]
#[derive(Default)]
struct Description { width: u32, height: u32, layers: u32, format: u32, usage: u64, stride: u32, reserved: u32, reserved64: u64 }
// These API-26 entry points use the same NDK library as platform's android/ndk_sys.rs.
#[link(name = "android")]
extern "C" {
    fn AHardwareBuffer_allocate(desc: *const Description, buffer: *mut *mut vk::AHardwareBuffer) -> i32;
    fn AHardwareBuffer_describe(buffer: *const vk::AHardwareBuffer, desc: *mut Description);
    fn AHardwareBuffer_acquire(buffer: *mut vk::AHardwareBuffer);
    fn AHardwareBuffer_release(buffer: *mut vk::AHardwareBuffer);
}

pub struct HardwareBuffer { pointer: usize }
impl Drop for HardwareBuffer { fn drop(&mut self) { unsafe { AHardwareBuffer_release(self.as_ptr()); } } }
impl HardwareBuffer {
    pub fn as_ptr(&self) -> *mut vk::AHardwareBuffer { self.pointer as *mut vk::AHardwareBuffer }
    /// Retain a live NDK AHardwareBuffer received from the Android shell.
    ///
    /// # Safety
    /// `buffer` must be a live AHardwareBuffer with a reference held by the caller.
    pub unsafe fn retain(buffer: *mut vk::AHardwareBuffer) -> Result<Self, String> {
        if buffer.is_null() { return Err("null AHardwareBuffer".into()); }
        AHardwareBuffer_acquire(buffer); Ok(Self { pointer: buffer as usize })
    }
    pub(super) fn extensions() -> Vec<&'static CStr> { vec![vk::ANDROID_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER_NAME, vk::EXT_QUEUE_FAMILY_FOREIGN_NAME] }

    pub(super) fn allocate(dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Image>, String> {
        Self::allocate_with(dev, desc, 0)
    }

    pub(crate) fn compositor(dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Image>, String> {
        Self::allocate_with(dev, desc, 1 << 11) // AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY
    }

    fn allocate_with(dev: Arc<Device>, desc: TextureObj, extra_usage: u64) -> Result<Arc<Image>, String> {
        if desc.depth != 1 || desc.mips != 1 { return Err("AHardwareBuffer sharing requires one layer and mip".into()); }
        let native_format = match desc.format {
            format::RGBA8 => 1, format::RGBA16F => 0x16, format::DEPTH32F => 0x33, format::DEPTH24_STENCIL8 => 0x32,
            _ => return Err("this HAL format has no portable AHardwareBuffer allocation format".into()),
        };
        let native = Description { width: desc.w, height: desc.h, layers: 1, format: native_format,
            usage: extra_usage | (1 << 8) | if desc.usage & usage::RENDER_TARGET != 0 { 1 << 9 } else { 0 }, ..Default::default() };
        let mut buffer = std::ptr::null_mut();
        let status = unsafe { AHardwareBuffer_allocate(&native, &mut buffer) };
        if status != 0 || buffer.is_null() { return Err(format!("AHardwareBuffer allocation failed: {status}")); }
        let handle = Arc::new(Self { pointer: buffer as usize });
        Self::bind(dev, handle, Some(desc))
    }

    pub(super) fn import(dev: Arc<Device>, native: Arc<Self>) -> Result<Arc<Image>, String> { Self::bind(dev, native, None) }

    fn bind(dev: Arc<Device>, native: Arc<Self>, created: Option<TextureObj>) -> Result<Arc<Image>, String> {
        if !Self::extensions().iter().all(|e| dev.supports(e)) { return Err("device lacks AHardwareBuffer/foreign sharing".into()); }
        let imported = created.is_none();
        let mut description = Description::default();
        unsafe { AHardwareBuffer_describe(native.as_ptr(), &mut description); }
        let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
        let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default().next_android_hardware_buffer_format_properties_android(&mut format_properties);
        unsafe { dev.raw.get_android_hardware_buffer_properties(native.as_ptr(), &mut properties) }.map_err(error)?;
        let fmt = (0..format::COUNT).find(|f| vk_format(*f) == Some(format_properties.format)).ok_or("AHardwareBuffer requires a media/YUV format conversion")?;
        if description.layers != 1 { return Err("AHardwareBuffer import requires one layer".into()); }
        let desc = created.unwrap_or(TextureObj { format: fmt, w: description.width, h: description.height, depth: 1, mips: 1,
            usage: usage::SAMPLED | usage::SHARED | if description.usage & (1 << 9) != 0 { usage::RENDER_TARGET } else { 0 }, bytes: 0,
            imported: true, uploaded: 0, lent: None });
        if desc.format != fmt || desc.w != description.width || desc.h != description.height { return Err("AHardwareBuffer allocation changed format/dimensions".into()); }
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
        let info = vk::ImageCreateInfo::default().next_external_memory_image_create_info(&mut external).image_type(vk::ImageType::TYPE_2D)
            .format(format_properties.format).extent(vk::Extent3D { width: desc.w, height: desc.h, depth: 1 })
            .mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1).tiling(vk::ImageTiling::OPTIMAL)
            .usage(image_usage(&desc)).sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
        let mut memory = vk::DeviceMemory::null();
        let result = (|| {
            // AHardwareBuffer memory requirements come from its properties before binding;
            // vkGetImageMemoryRequirements is not valid for this unbound external image.
            let ty = dev.memory_type(properties.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .or_else(|_| dev.memory_type(properties.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
            let mut import = vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(native.as_ptr());
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(properties.allocation_size).memory_type_index(ty)
                .next_import_android_hardware_buffer_info_android(&mut import).next_memory_dedicated_allocate_info(&mut dedicated), None) }.map_err(error)?;
            unsafe { dev.raw.bind_image_memory(image, memory, 0) }.map_err(error)
        })();
        if let Err(e) = result { unsafe { dev.raw.destroy_image(image, None); dev.raw.free_memory(memory, None); } return Err(e); }
        let image = unsafe { Image::from_bound(dev, desc, image, memory, if imported { 0 } else { properties.allocation_size },
            if imported { vk::ImageLayout::GENERAL } else { vk::ImageLayout::UNDEFINED }, Some(native)) }?;
        image.external_owner.store(imported, Ordering::Release); Ok(image)
    }

    /// A decoder's picture (`native`, a Y'CbCr hardware buffer, typically of a vendor format
    /// Vulkan knows only as an external format) as one sampled video texture over its memory,
    /// through its format's Y'CbCr conversion (`Device::video_sampler`): sampled with that
    /// sampler it gives the raw codes (.g = Y', .b = Cb, .r = Cr). Nothing is copied.
    pub(super) fn import_video(dev: Arc<Device>, native: Arc<Self>) -> Result<Arc<Image>, String> {
        if !Self::extensions().iter().all(|e| dev.supports(e)) { return Err("device lacks AHardwareBuffer/foreign sharing".into()); }
        let mut description = Description::default();
        unsafe { AHardwareBuffer_describe(native.as_ptr(), &mut description); }
        let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
        let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default().next_android_hardware_buffer_format_properties_android(&mut format_properties);
        unsafe { dev.raw.get_android_hardware_buffer_properties(native.as_ptr(), &mut properties) }.map_err(error)?;
        let external_format = format_properties.external_format;
        if external_format == 0 { return Err(format!("the picture's buffer (format {:#x}) has no external format", description.format)); }
        format_properties.p_next = std::ptr::null_mut();
        // (once per external format: what the driver makes of the buffer; a compressed (UBWC)
        // allocation shows as its own external format and a larger allocation)
        #[cfg(target_os = "android")]
        {
            static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            if !seen.contains(&external_format) {
                seen.push(external_format);
                crate::backend::android::xr::log(&format!("video import: buffer format {:#x} usage {:#x} {}x{} stride {} -> external format {:#x}, allocation {} B, features {:#x}, suggested model {} range {} chroma offsets {}/{}",
                    description.format, description.usage, description.width, description.height, description.stride, external_format, properties.allocation_size,
                    format_properties.format_features.as_raw(), format_properties.suggested_ycbcr_model.as_raw(), format_properties.suggested_ycbcr_range.as_raw(),
                    format_properties.suggested_x_chroma_offset.as_raw(), format_properties.suggested_y_chroma_offset.as_raw()));
            }
        }
        let video = dev.video_sampler(external_format, Some(&format_properties))?;
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
        let mut format = vk::ExternalFormatANDROID { external_format, ..Default::default() };
        let info = vk::ImageCreateInfo::default().next_external_memory_image_create_info(&mut external).next_external_format_android(&mut format)
            .image_type(vk::ImageType::TYPE_2D).format(vk::Format::UNDEFINED).extent(vk::Extent3D { width: description.width, height: description.height, depth: 1 })
            .mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1).tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED).sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
        let mut memory = vk::DeviceMemory::null();
        let result = (|| {
            let ty = dev.memory_type(properties.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .or_else(|_| dev.memory_type(properties.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
            let mut import = vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(native.as_ptr());
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(properties.allocation_size).memory_type_index(ty)
                .next_import_android_hardware_buffer_info_android(&mut import).next_memory_dedicated_allocate_info(&mut dedicated), None) }.map_err(error)?;
            unsafe { dev.raw.bind_image_memory(image, memory, 0) }.map_err(error)
        })();
        if let Err(e) = result { unsafe { dev.raw.destroy_image(image, None); dev.raw.free_memory(memory, None); } return Err(e); }
        // (an RGBA8 stand-in: the texture's format for the HAL's bookkeeping; its views are the conversion's)
        let desc = TextureObj { format: format::RGBA8, w: description.width, h: description.height, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: 0, imported: true, uploaded: 0, lent: None };
        let image = match unsafe { Image::external_video(dev.clone(), desc, image, memory, video, Some(native)) } {
            Ok(i) => i,
            Err(e) => { unsafe { dev.raw.destroy_image(image, None); dev.raw.free_memory(memory, None); } return Err(e); }
        };
        image.external_owner.store(true, Ordering::Release);
        Ok(image)
    }
}
