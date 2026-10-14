//! DMA-BUF copies outside any session, on the process's Vulkan device: another process's
//! frame (a browser's paint) into a shared ring slot, CPU pixels into a slot, a slot's pixels
//! back to the CPU (a still, an encoder's picture). Each call records one command buffer,
//! submits it and waits for it: the source may be reused by its owner once the call returns.
//!
//! Images that live as long as their owner says (ring slots) are registered once and named by
//! a process-local handle; a foreign source is imported on its first use and kept while its
//! owner keeps handing out the same buffer (a pool of a few). Every use acquires the image
//! from the foreign queue family and releases it back, in GENERAL layout, as the HAL's own
//! external images are used.

use super::super::device::{error, lock, Device};
use makepad_vulkan::vk;
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

pub const AR24: u32 = u32::from_le_bytes(*b"AR24");
pub const XR24: u32 = u32::from_le_bytes(*b"XR24");
pub const AB24: u32 = u32::from_le_bytes(*b"AB24");
pub const XB24: u32 = u32::from_le_bytes(*b"XB24");

/// How a single-plane DMA-BUF holds its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    /// DRM fourcc: `AR24`/`XR24` (BGRA in memory) or `AB24`/`XB24` (RGBA).
    pub fourcc: u32,
    pub modifier: u64,
    pub offset: u64,
    pub stride: u64,
}

impl Layout {
    /// As six words (width, height, fourcc, modifier, offset, stride), the form that crosses
    /// the `#[relax_api]` ops and the hosted-app channel.
    pub fn to_words(&self) -> [u64; 6] {
        [self.width as u64, self.height as u64, self.fourcc as u64, self.modifier, self.offset, self.stride]
    }

    pub fn from_words(words: &[u64]) -> Option<Layout> {
        let [width, height, fourcc, modifier, offset, stride] = <[u64; 6]>::try_from(words.get(..6)?).ok()?;
        Some(Layout { width: u32::try_from(width).ok()?, height: u32::try_from(height).ok()?, fourcc: u32::try_from(fourcc).ok()?, modifier, offset, stride })
    }

    fn format(&self) -> Result<vk::Format, String> {
        match self.fourcc {
            AR24 | XR24 => Ok(vk::Format::B8G8R8A8_UNORM),
            AB24 | XB24 => Ok(vk::Format::R8G8B8A8_UNORM),
            other => Err(format!("unsupported DMA-BUF fourcc {:?}", other.to_le_bytes().map(char::from))),
        }
    }
}

/// One DMA-BUF bound to a VkImage on the process's device.
struct Imported {
    dev: Arc<Device>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    format: vk::Format,
    layout: Layout,
}

impl Drop for Imported {
    fn drop(&mut self) {
        unsafe {
            self.dev.raw.destroy_image(self.image, None);
            self.dev.raw.free_memory(self.memory, None);
        }
    }
}

impl Imported {
    fn new(dev: &Arc<Device>, fd: BorrowedFd<'_>, layout: Layout) -> Result<Imported, String> {
        for ext in [vk::KHR_EXTERNAL_MEMORY_FD_NAME, vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME, vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME, vk::EXT_QUEUE_FAMILY_FOREIGN_NAME] {
            if !dev.supports(ext) {
                return Err(format!("the Vulkan device lacks {}", ext.to_string_lossy()));
            }
        }
        if layout.width == 0 || layout.height == 0 || layout.stride < layout.width as u64 * 4 {
            return Err(format!("invalid DMA-BUF layout {layout:?}"));
        }
        let format = layout.format()?;
        let size = {
            extern "C" { fn lseek(fd: i32, offset: i64, whence: i32) -> i64; }
            let size = unsafe { lseek(fd.as_raw_fd(), 0, 2) };
            if size <= 0 { return Err(format!("cannot size a DMA-BUF: {}", std::io::Error::last_os_error())); }
            size as u64
        };
        let planes = [vk::SubresourceLayout::default().offset(layout.offset).row_pitch(layout.stride)];
        let mut drm = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default().drm_format_modifier(layout.modifier).plane_layouts(&planes);
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default().image_type(vk::ImageType::TYPE_2D).format(format)
            .extent(vk::Extent3D { width: layout.width, height: layout.height, depth: 1 }).mip_levels(1).array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1).tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED)
            .next_external_memory_image_create_info(&mut external).next_image_drm_format_modifier_explicit_create_info_ext(&mut drm);
        let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
        let mut memory = vk::DeviceMemory::null();
        let bound = (|| {
            let req = unsafe { dev.raw.get_image_memory_requirements(image) };
            if req.size > size {
                return Err(format!("DMA-BUF of {size} bytes is smaller than its image needs ({})", req.size));
            }
            let fd = fd.try_clone_to_owned().map_err(|e| e.to_string())?;
            let mut properties = vk::MemoryFdPropertiesKHR::default();
            unsafe { dev.raw.get_memory_fd_properties(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT, fd.as_raw_fd(), &mut properties) }.map_err(error)?;
            let bits = req.memory_type_bits & properties.memory_type_bits;
            let ty = dev.memory_type(bits, vk::MemoryPropertyFlags::DEVICE_LOCAL).or_else(|_| dev.memory_type(bits, vk::MemoryPropertyFlags::empty()))?;
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut import = vk::ImportMemoryFdInfoKHR::default().handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT).fd(fd.as_raw_fd());
            memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(size).memory_type_index(ty)
                .next_memory_dedicated_allocate_info(&mut dedicated).next_import_memory_fd_info_khr(&mut import), None) }.map_err(error)?;
            // the allocation owns the descriptor now
            let _ = fd.into_raw_fd();
            unsafe { dev.raw.bind_image_memory(image, memory, 0) }.map_err(error)
        })();
        if let Err(e) = bound {
            unsafe {
                dev.raw.destroy_image(image, None);
                if memory != vk::DeviceMemory::null() { dev.raw.free_memory(memory, None); }
            }
            return Err(e);
        }
        Ok(Imported { dev: dev.clone(), image, memory, format, layout })
    }
}

struct Registered {
    fd: OwnedFd,
    layout: Layout,
    image: Option<Imported>,
}

/// The command buffer, fence and host-visible staging buffer every copy reuses.
struct Copier {
    dev: Arc<Device>,
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    staging: Option<(vk::Buffer, vk::DeviceMemory, *mut u8, u64)>,
}

// The mapped staging pointer is only touched under the state lock.
unsafe impl Send for Copier {}

impl Drop for Copier {
    fn drop(&mut self) {
        unsafe {
            let _ = self.dev.raw.wait_for_fences(&[self.fence], true, u64::MAX);
            if let Some((buffer, memory, _, _)) = self.staging.take() {
                self.dev.raw.destroy_buffer(buffer, None);
                self.dev.raw.free_memory(memory, None);
            }
            self.dev.raw.destroy_fence(self.fence, None);
            self.dev.raw.destroy_command_pool(self.pool, None);
        }
    }
}

impl Copier {
    fn new() -> Result<Copier, String> {
        let dev = Device::existing().ok_or("no Vulkan device in this process yet")?;
        let pool = unsafe { dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None) }.map_err(error)?;
        let cb = unsafe { dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)) }.map_err(error)?[0];
        let fence = unsafe { dev.raw.create_fence(&vk::FenceCreateInfo::default(), None) }.map_err(error)?;
        Ok(Copier { dev, pool, cb, fence, staging: None })
    }

    /// A mapped host buffer of at least `bytes`.
    fn staging(&mut self, bytes: u64) -> Result<(vk::Buffer, *mut u8), String> {
        if let Some((buffer, _, ptr, size)) = self.staging {
            if size >= bytes {
                return Ok((buffer, ptr));
            }
        }
        if let Some((buffer, memory, _, _)) = self.staging.take() {
            unsafe {
                self.dev.raw.destroy_buffer(buffer, None);
                self.dev.raw.free_memory(memory, None);
            }
        }
        let size = bytes.max(64 * 1024).next_power_of_two();
        let dev = &self.dev;
        let buffer = unsafe { dev.raw.create_buffer(&vk::BufferCreateInfo::default().size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST).sharing_mode(vk::SharingMode::EXCLUSIVE), None) }.map_err(error)?;
        let req = unsafe { dev.raw.get_buffer_memory_requirements(buffer) };
        let made = (|| {
            let ty = dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT | vk::MemoryPropertyFlags::HOST_CACHED)
                .or_else(|_| dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT))?;
            let memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None) }.map_err(error)?;
            let mapped = unsafe { dev.raw.bind_buffer_memory(buffer, memory, 0) }.map_err(error)
                .and_then(|_| unsafe { dev.raw.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) }.map_err(error));
            match mapped {
                Ok(ptr) => Ok((memory, ptr as *mut u8)),
                Err(e) => {
                    unsafe { dev.raw.free_memory(memory, None) };
                    Err(e)
                }
            }
        })();
        match made {
            Ok((memory, ptr)) => {
                self.staging = Some((buffer, memory, ptr, size));
                Ok((buffer, ptr))
            }
            Err(e) => {
                unsafe { dev.raw.destroy_buffer(buffer, None) };
                Err(e)
            }
        }
    }

    fn begin(&self) -> Result<(), String> {
        unsafe {
            self.dev.raw.reset_command_buffer(self.cb, vk::CommandBufferResetFlags::empty()).map_err(error)?;
            self.dev.raw.begin_command_buffer(self.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(error)
        }
    }

    /// Take `image` from the foreign family (keeping its pixels unless `discard`).
    fn acquire(&self, image: vk::Image, discard: bool) {
        let barrier = vk::ImageMemoryBarrier::default().image(image)
            .old_layout(if discard { vk::ImageLayout::UNDEFINED } else { vk::ImageLayout::GENERAL }).new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT).dst_queue_family_index(self.dev.family)
            .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE)
            .subresource_range(color_range());
        unsafe { self.dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(), &[], &[], &[barrier]); }
    }

    /// Give `image` back to the foreign family in GENERAL layout.
    fn release(&self, image: vk::Image) {
        let barrier = vk::ImageMemoryBarrier::default().image(image)
            .old_layout(vk::ImageLayout::GENERAL).new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(self.dev.family).dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::empty())
            .subresource_range(color_range());
        unsafe { self.dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(), &[], &[], &[barrier]); }
    }

    /// The host may read the staging buffer after the transfers.
    fn host_read_barrier(&self) {
        let barrier = vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ);
        unsafe { self.dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(), &[barrier], &[], &[]); }
    }

    /// End, submit and wait.
    fn run(&self) -> Result<(), String> {
        let dev = &self.dev;
        unsafe {
            dev.raw.end_command_buffer(self.cb).map_err(error)?;
            let cbs = [self.cb];
            let info = vk::SubmitInfo::default().command_buffers(&cbs);
            {
                let _queue = lock(&dev.queue_lock);
                dev.raw.queue_submit(dev.queue, &[info], self.fence).map_err(error)?;
            }
            let waited = dev.raw.wait_for_fences(&[self.fence], true, 5_000_000_000).map_err(error);
            dev.raw.reset_fences(&[self.fence]).map_err(error)?;
            waited
        }
    }

    /// Copy `src`'s top-left into `dst` (a blit when the formats differ) and read back the
    /// probes' pixels of `src` (BGRA, one word each; zero outside it).
    fn copy(&mut self, src: &Imported, dst: &Imported, probes: &[[u32; 2]]) -> Result<Vec<[u8; 4]>, String> {
        let (staging, mapped) = self.staging(probes.len().max(1) as u64 * 4)?;
        let w = src.layout.width.min(dst.layout.width);
        let h = src.layout.height.min(dst.layout.height);
        self.begin()?;
        self.acquire(src.image, false);
        self.acquire(dst.image, true);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        unsafe {
            if src.format == dst.format {
                let copy = vk::ImageCopy::default().src_subresource(layers).dst_subresource(layers).extent(vk::Extent3D { width: w, height: h, depth: 1 });
                self.dev.raw.cmd_copy_image(self.cb, src.image, vk::ImageLayout::GENERAL, dst.image, vk::ImageLayout::GENERAL, &[copy]);
            } else {
                let corner = vk::Offset3D { x: w as i32, y: h as i32, z: 1 };
                let blit = vk::ImageBlit::default().src_subresource(layers).dst_subresource(layers)
                    .src_offsets([vk::Offset3D::default(), corner]).dst_offsets([vk::Offset3D::default(), corner]);
                self.dev.raw.cmd_blit_image(self.cb, src.image, vk::ImageLayout::GENERAL, dst.image, vk::ImageLayout::GENERAL, &[blit], vk::Filter::NEAREST);
            }
            let regions: Vec<_> = probes.iter().enumerate().filter(|(_, [x, y])| *x < src.layout.width && *y < src.layout.height)
                .map(|(i, [x, y])| vk::BufferImageCopy::default().buffer_offset(i as u64 * 4).image_subresource(layers)
                    .image_offset(vk::Offset3D { x: *x as i32, y: *y as i32, z: 0 }).image_extent(vk::Extent3D { width: 1, height: 1, depth: 1 }))
                .collect();
            std::ptr::write_bytes(mapped, 0, probes.len() * 4);
            if !regions.is_empty() {
                self.dev.raw.cmd_copy_image_to_buffer(self.cb, src.image, vk::ImageLayout::GENERAL, staging, &regions);
            }
        }
        self.host_read_barrier();
        self.release(src.image);
        self.release(dst.image);
        self.run()?;
        let bgra = src.format == vk::Format::B8G8R8A8_UNORM;
        Ok((0..probes.len()).map(|i| {
            let p = unsafe { std::slice::from_raw_parts(mapped.add(i * 4), 4) };
            if bgra { [p[0], p[1], p[2], p[3]] } else { [p[2], p[1], p[0], p[3]] }
        }).collect())
    }

    /// Write BGRA rows (`stride` bytes apart) into `dst`'s top-left.
    fn upload(&mut self, dst: &Imported, pixels: &[u8], width: u32, height: u32, stride: usize) -> Result<(), String> {
        let (w, h) = (width.min(dst.layout.width), height.min(dst.layout.height));
        if w == 0 || h == 0 || stride < width as usize * 4 || pixels.len() < stride * (height as usize - 1) + width as usize * 4 {
            return Err("upload: pixels do not cover the picture".into());
        }
        let (staging, mapped) = self.staging(w as u64 * h as u64 * 4)?;
        let rgba = dst.format == vk::Format::R8G8B8A8_UNORM;
        for y in 0..h as usize {
            let row = &pixels[y * stride..y * stride + w as usize * 4];
            let out = unsafe { std::slice::from_raw_parts_mut(mapped.add(y * w as usize * 4), w as usize * 4) };
            out.copy_from_slice(row);
            if rgba {
                for px in out.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
        }
        self.begin()?;
        self.acquire(dst.image, true);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let region = vk::BufferImageCopy::default().buffer_row_length(w).image_subresource(layers).image_extent(vk::Extent3D { width: w, height: h, depth: 1 });
        unsafe { self.dev.raw.cmd_copy_buffer_to_image(self.cb, staging, dst.image, vk::ImageLayout::GENERAL, &[region]); }
        self.release(dst.image);
        self.run()
    }

    /// `src`'s pixels in `region` (x, y, width, height), BGRA, tightly packed.
    fn read(&mut self, src: &Imported, region: [u32; 4]) -> Result<Vec<u8>, String> {
        let [x, y, w, h] = region;
        if w == 0 || h == 0 || x.saturating_add(w) > src.layout.width || y.saturating_add(h) > src.layout.height {
            return Err(format!("read: region {region:?} is outside the {}x{} image", src.layout.width, src.layout.height));
        }
        let bytes = w as u64 * h as u64 * 4;
        let (staging, mapped) = self.staging(bytes)?;
        self.begin()?;
        self.acquire(src.image, false);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let copy = vk::BufferImageCopy::default().buffer_row_length(w).image_subresource(layers)
            .image_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 }).image_extent(vk::Extent3D { width: w, height: h, depth: 1 });
        unsafe { self.dev.raw.cmd_copy_image_to_buffer(self.cb, src.image, vk::ImageLayout::GENERAL, staging, &[copy]); }
        self.host_read_barrier();
        self.release(src.image);
        self.run()?;
        let mut out = unsafe { std::slice::from_raw_parts(mapped, bytes as usize) }.to_vec();
        if src.format == vk::Format::R8G8B8A8_UNORM {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(out)
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)
}

/// Foreign sources seen lately, by the buffer's identity (device, inode) and layout.
const FOREIGN_KEPT: usize = 8;

#[derive(Default)]
struct State {
    copier: Option<Copier>,
    next: u64,
    registered: HashMap<u64, Registered>,
    foreign: Vec<((u64, u64, Layout), Imported)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> Result<R, String>) -> Result<R, String> {
    let mut state = lock(&STATE);
    f(state.get_or_insert_with(State::default))
}

impl State {
    fn copier(&mut self) -> Result<&mut Copier, String> {
        if self.copier.is_none() {
            self.copier = Some(Copier::new()?);
        }
        Ok(self.copier.as_mut().unwrap())
    }

    /// The registered image `handle`, imported on first use.
    fn registered(&mut self, handle: u64) -> Result<(), String> {
        let dev = self.copier()?.dev.clone();
        let entry = self.registered.get_mut(&handle).ok_or("unknown DMA-BUF handle")?;
        if entry.image.is_none() {
            entry.image = Some(Imported::new(&dev, entry.fd.as_fd(), entry.layout)?);
        }
        Ok(())
    }

    /// The foreign source's import, kept for its next use.
    fn foreign(&mut self, fd: BorrowedFd<'_>, layout: Layout) -> Result<usize, String> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::File::from(fd.try_clone_to_owned().map_err(|e| e.to_string())?).metadata().map_err(|e| e.to_string())?;
        let key = (meta.dev(), meta.ino(), layout);
        if let Some(at) = self.foreign.iter().position(|(k, _)| *k == key) {
            return Ok(at);
        }
        let dev = self.copier()?.dev.clone();
        let image = Imported::new(&dev, fd, layout)?;
        if self.foreign.len() >= FOREIGN_KEPT {
            self.foreign.remove(0);
        }
        self.foreign.push((key, image));
        Ok(self.foreign.len() - 1)
    }
}

/// The process's Vulkan device was replaced after a loss: the copier and every import made on
/// the old one are dropped; the next copy makes them again on the new device (the registered
/// descriptors are kept).
pub fn device_replaced() {
    let mut state = lock(&STATE);
    if let Some(state) = state.as_mut() {
        state.foreign.clear();
        for entry in state.registered.values_mut() { entry.image = None; }
        state.copier = None;
    }
}

/// Name an image for the copies below (the descriptor is kept until `release`).
pub fn register(fd: OwnedFd, layout: Layout) -> u64 {
    let mut state = lock(&STATE);
    let state = state.get_or_insert_with(State::default);
    state.next += 1;
    let handle = state.next;
    state.registered.insert(handle, Registered { fd, layout, image: None });
    handle
}

pub fn release(handle: u64) {
    if let Some(state) = lock(&STATE).as_mut() {
        state.registered.remove(&handle);
    }
}

pub fn layout(handle: u64) -> Option<Layout> {
    lock(&STATE).as_ref()?.registered.get(&handle).map(|r| r.layout)
}

/// Copy the foreign image `src` into the registered image `dst` (its top-left; the smaller
/// size of the two) and read back the probes' pixels of `src`. Waits for the GPU: `src` may be
/// reused by its owner as soon as this returns.
pub fn copy(src: BorrowedFd<'_>, src_layout: Layout, dst: u64, probes: &[[u32; 2]]) -> Result<Vec<[u8; 4]>, String> {
    with_state(|state| {
        state.registered(dst)?;
        let at = state.foreign(src, src_layout)?;
        let State { copier, registered, foreign, .. } = state;
        let dst = registered.get(&dst).and_then(|r| r.image.as_ref()).ok_or("unknown DMA-BUF handle")?;
        copier.as_mut().unwrap().copy(&foreign[at].1, dst, probes)
    })
}

/// Write BGRA rows (`stride` bytes apart) of a `width` x `height` picture into the registered
/// image `dst` (its top-left).
pub fn upload(dst: u64, pixels: &[u8], width: u32, height: u32, stride: usize) -> Result<(), String> {
    with_state(|state| {
        state.registered(dst)?;
        let State { copier, registered, .. } = state;
        let dst = registered.get(&dst).and_then(|r| r.image.as_ref()).ok_or("unknown DMA-BUF handle")?;
        copier.as_mut().unwrap().upload(dst, pixels, width, height, stride)
    })
}

/// The registered image's pixels in `region` (x, y, width, height), BGRA, tightly packed.
pub fn read(handle: u64, region: [u32; 4]) -> Result<Vec<u8>, String> {
    with_state(|state| {
        state.registered(handle)?;
        let State { copier, registered, .. } = state;
        let src = registered.get(&handle).and_then(|r| r.image.as_ref()).ok_or("unknown DMA-BUF handle")?;
        copier.as_mut().unwrap().read(src, region)
    })
}
