//! An on-demand inventory of the GPU allocations a `Cx` knows about: every
//! texture in its pool (format, size, mip levels, allocated bytes, CPU pixels
//! still held, and the passes it is an attachment of or sampled in), the
//! instance and geometry buffers per draw list where the backend reports
//! them, and the totals the backends keep. For memory reports and tools; it
//! walks every pool (O(textures + passes + draw items)) and changes nothing.

use crate::{
    cx::Cx,
    draw_list::CxDrawKind,
    texture::{TextureFormat, TextureId},
};

/// What a texture slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuTextureState {
    /// A live handle names it.
    Live,
    /// Every handle was dropped; the slot waits for reuse and still holds
    /// its GPU storage until then (or until it is released).
    FreeSlotHeld,
}

#[derive(Clone, Debug)]
pub struct GpuTextureEntry {
    pub slot: usize,
    pub generation: u64,
    pub state: GpuTextureState,
    /// Short format name (`RenderRGBAf16`, `VecBGRAu8_32`, ...).
    pub format: &'static str,
    /// Render target (`Render*`), depth buffer, or uploaded data (`Vec*`).
    pub render_target: bool,
    pub depth: bool,
    /// The allocated size (0 x 0 when no GPU storage is allocated).
    pub width: usize,
    pub height: usize,
    pub mip_levels: usize,
    /// Allocated GPU bytes as the backend reports them (Metal `allocatedSize`,
    /// GL/D3D texel storage); `None` where the backend does not report.
    pub gpu_bytes: Option<u64>,
    /// Texel bytes computed from format, size and mip levels.
    pub texel_bytes: u64,
    /// A slot reused for a new texture keeps the previous texture's GPU
    /// storage here until the slot is released.
    pub previous_bytes: u64,
    /// CPU pixels the texture still holds (a `Vec*` format's data).
    pub cpu_bytes: u64,
    /// Passes this texture is attached to (`<pass name>:color<i>`,
    /// `<pass name>:depth`).
    pub attached_to: Vec<String>,
    /// Passes whose draw lists sample it (pass names, deduplicated).
    pub sampled_in: Vec<String>,
}

impl GpuTextureEntry {
    /// The best known GPU size: reported, else computed, plus the previous
    /// allocation the slot still holds.
    pub fn bytes(&self) -> u64 {
        self.gpu_bytes.unwrap_or(self.texel_bytes).saturating_add(self.previous_bytes)
    }
    /// A display name: the first attachment, else the first sampling pass.
    pub fn name(&self) -> String {
        if let Some(a) = self.attached_to.first() {
            a.clone()
        } else if let Some(s) = self.sampled_in.first() {
            format!("data sampled in {s}")
        } else {
            String::from("data")
        }
    }
}

/// The buffers of one draw list.
#[derive(Clone, Debug, Default)]
pub struct GpuDrawListBuffers {
    pub draw_list: usize,
    /// The pass the list was last recorded into (its name), if any.
    pub pass: String,
    pub draw_calls: usize,
    /// Instance data the list holds on the CPU (recording and retained
    /// publication floats).
    pub instance_cpu_bytes: u64,
    /// GPU instance buffer capacity (Metal: every buffer, pending copy and
    /// spare of each item); `None` where the backend does not report.
    pub instance_gpu_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct GpuInventory {
    pub textures: Vec<GpuTextureEntry>,
    /// Texture allocations released and waiting for the GPU to finish.
    pub retired_texture_bytes: u64,
    pub draw_lists: Vec<GpuDrawListBuffers>,
    /// Geometry vertex + index bytes on the CPU and (where reported) GPU.
    pub geometry_cpu_bytes: u64,
    pub geometry_gpu_bytes: Option<u64>,
    pub geometries: usize,
    /// Physical instance buffer bytes the retained upload budget charges
    /// (every backing allocation incl. spares and pending retirement).
    pub retained_allocation_bytes: u64,
    /// The device's own total (Metal `currentAllocatedSize`): every buffer,
    /// texture and heap the process holds on the GPU, including what no
    /// registry above sees (drawables, staging, pipeline state).
    pub device_allocated_bytes: Option<u64>,
}

impl GpuInventory {
    pub fn texture_bytes(&self) -> u64 {
        self.textures.iter().map(|t| t.bytes()).sum()
    }
    pub fn instance_gpu_bytes(&self) -> u64 {
        self.draw_lists.iter().filter_map(|d| d.instance_gpu_bytes).sum()
    }
}

fn format_name(format: &TextureFormat) -> &'static str {
    match format {
        TextureFormat::Unknown => "Unknown",
        TextureFormat::VecBGRAu8_32 { .. } => "VecBGRAu8_32",
        TextureFormat::VecCubeBGRAu8_32 { .. } => "VecCubeBGRAu8_32",
        TextureFormat::VecMipBGRAu8_32 { .. } => "VecMipBGRAu8_32",
        TextureFormat::VecMipRGBAf32 { .. } => "VecMipRGBAf32",
        TextureFormat::VecMipCompressed { .. } => "VecMipCompressed",
        TextureFormat::VecRGBAf32 { .. } => "VecRGBAf32",
        TextureFormat::VecRu8 { .. } => "VecRu8",
        TextureFormat::VecRGu8 { .. } => "VecRGu8",
        TextureFormat::VecRf32 { .. } => "VecRf32",
        TextureFormat::DepthD32 { .. } => "DepthD32",
        TextureFormat::DepthD32Sampled { .. } => "DepthD32Sampled",
        TextureFormat::RenderBGRAu8 { .. } => "RenderBGRAu8",
        TextureFormat::RenderCubeBGRAu8 { .. } => "RenderCubeBGRAu8",
        TextureFormat::RenderRGBAf16 { .. } => "RenderRGBAf16",
        TextureFormat::RenderRGBAf32 { .. } => "RenderRGBAf32",
        TextureFormat::RenderRf32 { .. } => "RenderRf32",
        TextureFormat::RenderRGf16 { .. } => "RenderRGf16",
        TextureFormat::RenderRu32 { .. } => "RenderRu32",
        TextureFormat::SharedBGRAu8 { .. } => "SharedBGRAu8",
        TextureFormat::VideoYuvPlane => "VideoYuvPlane",
        TextureFormat::VideoExternal => "VideoExternal",
        TextureFormat::VideoGlMemoryRgba => "VideoGlMemoryRgba",
        TextureFormat::VideoRgbaHardwareBuffer => "VideoRgbaHardwareBuffer",
    }
}

/// Bytes per texel (per block for compressed formats, handled apart).
fn texel_size(format: &TextureFormat) -> u64 {
    match format {
        TextureFormat::VecBGRAu8_32 { .. }
        | TextureFormat::VecCubeBGRAu8_32 { .. }
        | TextureFormat::VecMipBGRAu8_32 { .. }
        | TextureFormat::RenderBGRAu8 { .. }
        | TextureFormat::RenderCubeBGRAu8 { .. }
        | TextureFormat::SharedBGRAu8 { .. }
        | TextureFormat::VecRf32 { .. }
        | TextureFormat::RenderRf32 { .. }
        | TextureFormat::RenderRGf16 { .. }
        | TextureFormat::RenderRu32 { .. }
        | TextureFormat::DepthD32 { .. }
        | TextureFormat::DepthD32Sampled { .. } => 4,
        TextureFormat::VecMipRGBAf32 { .. } | TextureFormat::VecRGBAf32 { .. } | TextureFormat::RenderRGBAf32 { .. } => 16,
        TextureFormat::RenderRGBAf16 { .. } => 8,
        TextureFormat::VecRu8 { .. } => 1,
        TextureFormat::VecRGu8 { .. } => 2,
        _ => 0,
    }
}

#[cfg(all(not(gpusim), any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
fn draw_call_gpu_bytes(os: &crate::os::CxOsDrawCall) -> Option<u64> {
    Some(os.inventory_bytes())
}
#[cfg(not(all(not(gpusim), any(target_os = "macos", target_os = "ios", target_os = "tvos"))))]
fn draw_call_gpu_bytes(_os: &crate::os::CxOsDrawCall) -> Option<u64> {
    None
}
#[cfg(all(not(gpusim), any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
fn geometry_gpu_bytes(os: &crate::os::CxOsGeometry) -> Option<u64> {
    Some(os.inventory_bytes())
}
#[cfg(not(all(not(gpusim), any(target_os = "macos", target_os = "ios", target_os = "tvos"))))]
fn geometry_gpu_bytes(_os: &crate::os::CxOsGeometry) -> Option<u64> {
    None
}

fn full_chain(width: usize, height: usize) -> usize {
    (usize::BITS - width.max(height).max(1).leading_zeros()) as usize
}

impl Cx {
    /// Every GPU allocation this `Cx`'s registries know, measured now.
    /// Call `frame_completion_serial` first to collect finished retirements.
    pub fn gpu_inventory(&self) -> GpuInventory {
        let mut inv = GpuInventory::default();

        // Pass attachments and the passes whose draw lists sample a texture.
        let slots = self.textures.0.pool.len();
        let mut attached: Vec<Vec<String>> = vec![Vec::new(); slots];
        let mut sampled: Vec<Vec<String>> = vec![Vec::new(); slots];
        let mut pass_names: Vec<String> = Vec::with_capacity(self.passes.0.pool.len());
        for (index, slot) in self.passes.0.pool.iter().enumerate() {
            let pass = &slot.item;
            let name = if pass.debug_name.is_empty() { format!("pass#{index}") } else { pass.debug_name.clone() };
            pass_names.push(name.clone());
            if self.passes.0.is_free(index) {
                continue;
            }
            for (i, color) in pass.color_textures.iter().enumerate() {
                let id = color.texture.texture_id();
                if let Some(list) = attached.get_mut(id.0) {
                    list.push(format!("{name}:color{i}"));
                }
            }
            if let Some(depth) = &pass.depth_texture {
                if let Some(list) = attached.get_mut(depth.texture_id().0) {
                    list.push(format!("{name}:depth"));
                }
            }
        }

        for (index, slot) in self.draw_lists.0.pool.iter().enumerate() {
            if self.draw_lists.0.is_free(index) {
                continue;
            }
            let list = &slot.item;
            let pass = list
                .draw_pass_id
                .and_then(|p| pass_names.get(p.0).cloned())
                .unwrap_or_default();
            let mut buffers = GpuDrawListBuffers { draw_list: index, pass: pass.clone(), ..Default::default() };
            let mut gpu = 0u64;
            let mut gpu_known = false;
            for item in &list.draw_items.buffer {
                if let Some(instances) = &item.instances {
                    buffers.instance_cpu_bytes += (instances.capacity() * 4) as u64;
                }
                if let Some(retained) = &item.retained_instances {
                    buffers.instance_cpu_bytes += (retained.float_len() * 4) as u64;
                }
                if let Some(bytes) = draw_call_gpu_bytes(&item.os) {
                    gpu += bytes;
                    gpu_known = true;
                }
                if let CxDrawKind::DrawCall(call) = &item.kind {
                    buffers.draw_calls += 1;
                    for texture in call.texture_slots.iter().flatten() {
                        if let Some(list) = sampled.get_mut(texture.texture_id().0) {
                            if !pass.is_empty() && !list.contains(&pass) {
                                list.push(pass.clone());
                            }
                        }
                    }
                }
            }
            buffers.instance_gpu_bytes = gpu_known.then_some(gpu);
            if buffers.instance_cpu_bytes > 0 || gpu > 0 {
                inv.draw_lists.push(buffers);
            }
        }

        for (index, slot) in self.textures.0.pool.iter().enumerate() {
            let texture = &slot.item;
            let free = self.textures.0.is_free(index);
            let id = TextureId::from_pool_slot(index, slot.generation);
            let gpu_bytes = self.texture_allocation_bytes(id);
            let previous_bytes = texture
                .previous_platform_resource
                .as_ref()
                .and_then(|os| os.allocated_bytes(self))
                .unwrap_or(0);
            let (width, height) = texture.alloc.as_ref().map_or((0, 0), |a| (a.width, a.height));
            let cpu_bytes = texture.format.cpu_data_bytes() as u64;
            if free && gpu_bytes.unwrap_or(0) == 0 && previous_bytes == 0 && texture.alloc.is_none() {
                continue;
            }
            if !free && texture.alloc.is_none() && gpu_bytes.unwrap_or(0) == 0 && previous_bytes == 0 && cpu_bytes == 0 {
                continue;
            }
            let mip_levels = match &texture.format {
                TextureFormat::VecMipBGRAu8_32 { max_level, .. }
                | TextureFormat::VecMipRGBAf32 { max_level, .. }
                | TextureFormat::VecMipCompressed { max_level, .. } => {
                    max_level.map_or(full_chain(width, height), |m| (m + 1).min(full_chain(width, height)))
                }
                f if f.is_render() && texture.render_mips => full_chain(width, height),
                _ => 1,
            };
            let faces: u64 = if matches!(texture.format, TextureFormat::VecCubeBGRAu8_32 { .. } | TextureFormat::RenderCubeBGRAu8 { .. }) { 6 } else { 1 };
            let mut texel_bytes = 0u64;
            for level in 0..mip_levels.max(1) {
                let (w, h) = ((width >> level).max(1), (height >> level).max(1));
                texel_bytes += match &texture.format {
                    TextureFormat::VecMipCompressed { .. } => crate::texture::CompressedTextureFormat::level_bytes(w, h) as u64,
                    f => (w * h) as u64 * texel_size(f),
                };
            }
            if width == 0 || height == 0 {
                texel_bytes = 0;
            }
            inv.textures.push(GpuTextureEntry {
                slot: index,
                generation: slot.generation,
                state: if free { GpuTextureState::FreeSlotHeld } else { GpuTextureState::Live },
                format: format_name(&texture.format),
                render_target: texture.format.is_render(),
                depth: texture.format.is_depth(),
                width,
                height,
                mip_levels,
                gpu_bytes,
                texel_bytes: texel_bytes * faces,
                previous_bytes,
                cpu_bytes,
                attached_to: if free { Vec::new() } else { std::mem::take(&mut attached[index]) },
                sampled_in: if free { Vec::new() } else { std::mem::take(&mut sampled[index]) },
            });
        }
        inv.retired_texture_bytes = self
            .textures
            .1
            .retired
            .iter()
            .map(|r| r.os.allocated_bytes(self).unwrap_or(r.bytes))
            .sum();

        let mut geometry_gpu = 0u64;
        let mut geometry_gpu_known = false;
        for (index, slot) in self.geometries.0.pool.iter().enumerate() {
            if self.geometries.0.is_free(index) {
                continue;
            }
            let g = &slot.item;
            inv.geometries += 1;
            inv.geometry_cpu_bytes += g.vertices.byte_len() as u64 + g.indices.capacity_bytes() as u64;
            if let Some(bytes) = geometry_gpu_bytes(&g.os) {
                geometry_gpu += bytes;
                geometry_gpu_known = true;
            }
        }
        inv.geometry_gpu_bytes = geometry_gpu_known.then_some(geometry_gpu);
        inv.retained_allocation_bytes = self.draw_lists.1.allocations.bytes() as u64;
        inv.device_allocated_bytes = self.device_allocated_bytes();
        inv
    }

    /// The live texture slots as (index, generation), cheaply: compare the
    /// lists before and after some code ran to learn which textures it made
    /// (the entries of [`GpuInventory::textures`] carry the same pair).
    pub fn live_texture_slots(&self) -> Vec<(usize, u64)> {
        self.textures
            .0
            .pool
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.textures.0.is_free(*index))
            .map(|(index, slot)| (index, slot.generation))
            .collect()
    }

    #[cfg(all(not(gpusim), target_os = "macos"))]
    fn device_allocated_bytes(&self) -> Option<u64> {
        use crate::makepad_objc_sys::{msg_send, sel, sel_impl};
        let device = self.os.metal_device?;
        Some(unsafe { msg_send![device, currentAllocatedSize] })
    }

    #[cfg(not(all(not(gpusim), target_os = "macos")))]
    fn device_allocated_bytes(&self) -> Option<u64> {
        None
    }
}
