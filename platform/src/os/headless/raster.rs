use super::virtual_gpu::{
    rasterize_triangle_rows, Framebuffer, RasterScratch, RasterState, TriangleDerivatives,
};
use crate::{
    cx::Cx,
    draw_list::{CxDrawKind, DrawListId},
    draw_pass::{CxDrawPassParent, DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::{CxDrawShaderCode, CxDrawShaderMapping, DrawShaderColorFormat},
    makepad_live_id::*,
    makepad_math::*,
    texture::TextureFormat,
};
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::{collections::HashMap, sync::mpsc};

// ─────────────────────────────────────────────────────────────────────────────
// JIT shader function pointer types
// ─────────────────────────────────────────────────────────────────────────────

type VertexFn = unsafe extern "C" fn(
    geom_ptr: *const f32,
    geom_len: u32,
    inst_ptr: *const f32,
    inst_len: u32,
    uniform_ptrs: *const *const f32,
    uniform_lens: *const u32,
    uniform_count: u32,
    varying_out: *mut f32,
    varying_len: u32,
    out_pos: *mut [f32; 4],
);

/// Fragment entry: takes a pre-filled RenderCx buffer, returns 1 = write pixel, 0 = discard.
/// The host reads frag_fb0 directly from the buffer after the call.
type FragmentFn = unsafe extern "C" fn(rcx_ptr: *mut f32, rcx_f32s: u32) -> u32;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Write a u32 value at a byte offset in the rcx buffer.
#[inline]
fn set_u32(buf: &mut [u8], offset: usize, val: u32) {
    if offset + 4 <= buf.len() {
        buf[offset..offset + 4].copy_from_slice(&val.to_ne_bytes());
    }
}

#[derive(Clone, Copy)]
struct RowChunk {
    start: usize,
    end: usize,
}

fn configured_render_threads(default_threads: usize) -> usize {
    // Efficiency-first default: avoid blasting all cores unless explicitly requested.
    let auto_threads = default_threads.min(4).max(1);
    std::env::var("MAKEPAD_HEADLESS_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(auto_threads)
}

fn configured_parallel_min_tris(default_min: usize) -> usize {
    std::env::var("MAKEPAD_HEADLESS_PARALLEL_MIN_TRIS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default_min)
}

fn compute_index_chunks(
    total: usize,
    desired_chunks: usize,
    min_items_per_chunk: usize,
) -> Vec<RowChunk> {
    if total == 0 {
        return Vec::new();
    }
    let max_chunks = (total / min_items_per_chunk.max(1)).max(1);
    let chunk_count = desired_chunks.max(1).min(max_chunks);
    if chunk_count <= 1 {
        return vec![RowChunk {
            start: 0,
            end: total,
        }];
    }

    let mut chunks = Vec::with_capacity(chunk_count);
    let base = total / chunk_count;
    let rem = total % chunk_count;
    let mut start = 0usize;
    for i in 0..chunk_count {
        let items = base + usize::from(i < rem);
        let end = (start + items).min(total);
        if end > start {
            chunks.push(RowChunk { start, end });
        }
        start = end;
    }
    if chunks.is_empty() {
        chunks.push(RowChunk {
            start: 0,
            end: total,
        });
    }
    chunks
}

fn compute_row_chunks(height: usize, desired_threads: usize) -> Vec<RowChunk> {
    compute_index_chunks(height, desired_threads, 32)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TextureConversionSignature {
    kind: u8,
    width: usize,
    height: usize,
    data_ptr: usize,
    data_len: usize,
}

pub(crate) struct CachedTextureConversion {
    signature: TextureConversionSignature,
    rgba: Vec<f32>,
}

pub(crate) type TextureConversionCache = HashMap<usize, CachedTextureConversion>;

/// Offscreen render targets for the software raster path.
///
/// One [`Framebuffer`] per colour-attachment TEXTURE — the framebuffer *is*
/// the texture, so several passes writing the same attachment accumulate into
/// it the way they do on a GPU, and a sampler resolves a render texture by
/// looking up its own index.
///
/// Kept across frames: a parent that repaints while its child stayed clean must
/// still see the child's last contents, exactly like a GPU texture that was not
/// re-rendered. Buffers are reused in place (see [`Framebuffer::resize`]), so a
/// pane-sized 3D pass costs one allocation, not one per frame.
#[derive(Default)]
pub(crate) struct HeadlessRenderTargets {
    framebuffers: HashMap<usize, Framebuffer>,
}

/// Attachment-level raster state: what the render pass descriptor fixes for
/// every draw call inside the pass.
#[derive(Clone, Copy)]
struct PassRaster {
    /// Clip space maps onto this rect, anchored top-left in the attachment.
    viewport: (usize, usize),
    /// The attachment is 8-bit unorm, so writes clamp to [0,1].
    clamp_unorm: bool,
}

impl HeadlessRenderTargets {
    fn color_target(&self, texture_index: usize) -> Option<&Framebuffer> {
        let fb = self.framebuffers.get(&texture_index)?;
        if fb.width == 0 || fb.height == 0 || fb.color.is_empty() {
            return None;
        }
        Some(fb)
    }
}

fn headless_texture_info(
    texture_index: usize,
    cxtexture: &crate::texture::CxTexture,
    cache: &mut TextureConversionCache,
    render_targets: &HeadlessRenderTargets,
) -> Option<[usize; 4]> {
    match &cxtexture.format {
        // Render-to-texture attachments carry no CPU-side vec: their pixels
        // live in the child pass's framebuffer. Point the sampler straight at
        // it — the child pass always rendered before its parent, and the
        // framebuffer is not touched again while the parent rasterizes.
        // (`[f32;4]` is four contiguous f32 so the colour buffer *is* an RGBA
        // f32 image; a single-channel Rf32 target reads back through .x, which
        // is the component every Rf32 consumer uses.)
        TextureFormat::RenderBGRAu8 { .. }
        | TextureFormat::RenderRGBAf16 { .. }
        | TextureFormat::RenderRGBAf32 { .. }
        | TextureFormat::RenderRf32 { .. } => {
            let fb = render_targets.color_target(texture_index)?;
            Some([
                fb.color.as_ptr() as usize,
                fb.color.len() * 4,
                fb.width,
                fb.height,
            ])
        }
        _ => headless_vec_texture_info(texture_index, cxtexture, cache),
    }
}

fn headless_vec_texture_info(
    texture_index: usize,
    cxtexture: &crate::texture::CxTexture,
    cache: &mut TextureConversionCache,
) -> Option<[usize; 4]> {
    match &cxtexture.format {
        TextureFormat::VecMipRGBAf32 {
            width,
            height,
            data: Some(data),
            ..
        }
        | TextureFormat::VecRGBAf32 {
            width,
            height,
            data: Some(data),
            ..
        } => Some([data.as_ptr() as usize, data.len(), *width, *height]),
        TextureFormat::VecBGRAu8_32 {
            width,
            height,
            data: Some(data),
            updated,
        }
        | TextureFormat::VecMipBGRAu8_32 {
            width,
            height,
            data: Some(data),
            updated,
            ..
        } => {
            let sig = TextureConversionSignature {
                kind: 1,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            };
            let entry = cache
                .entry(texture_index)
                .or_insert_with(|| CachedTextureConversion {
                    signature: sig,
                    rgba: Vec::new(),
                });
            if entry.signature != sig || !updated.is_empty() || entry.rgba.is_empty() {
                entry.signature = sig;
                entry.rgba.clear();
                entry.rgba.reserve(data.len() * 4);
                for &pixel in data {
                    let b = (pixel & 0xFF) as f32 / 255.0;
                    let g = ((pixel >> 8) & 0xFF) as f32 / 255.0;
                    let r = ((pixel >> 16) & 0xFF) as f32 / 255.0;
                    let a = ((pixel >> 24) & 0xFF) as f32 / 255.0;
                    entry.rgba.push(r);
                    entry.rgba.push(g);
                    entry.rgba.push(b);
                    entry.rgba.push(a);
                }
            }
            Some([
                entry.rgba.as_ptr() as usize,
                entry.rgba.len(),
                *width,
                *height,
            ])
        }
        TextureFormat::VecCubeBGRAu8_32 {
            width,
            height,
            data: Some(data),
            updated,
        } => {
            let expected = width.saturating_mul(*height).saturating_mul(6);
            let sig = TextureConversionSignature {
                kind: 4,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            };
            let entry = cache
                .entry(texture_index)
                .or_insert_with(|| CachedTextureConversion {
                    signature: sig,
                    rgba: Vec::new(),
                });
            if entry.signature != sig || !updated.is_empty() || entry.rgba.is_empty() {
                entry.signature = sig;
                entry.rgba.clear();
                entry.rgba.reserve(expected.saturating_mul(4));
                for &pixel in data.iter().take(expected) {
                    let b = (pixel & 0xFF) as f32 / 255.0;
                    let g = ((pixel >> 8) & 0xFF) as f32 / 255.0;
                    let r = ((pixel >> 16) & 0xFF) as f32 / 255.0;
                    let a = ((pixel >> 24) & 0xFF) as f32 / 255.0;
                    entry.rgba.push(r);
                    entry.rgba.push(g);
                    entry.rgba.push(b);
                    entry.rgba.push(a);
                }
            }
            Some([
                entry.rgba.as_ptr() as usize,
                entry.rgba.len(),
                *width,
                *height,
            ])
        }
        TextureFormat::VecRu8 {
            width,
            height,
            data: Some(data),
            updated,
            ..
        } => {
            let expected = width.saturating_mul(*height);
            let sig = TextureConversionSignature {
                kind: 2,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            };
            let entry = cache
                .entry(texture_index)
                .or_insert_with(|| CachedTextureConversion {
                    signature: sig,
                    rgba: Vec::new(),
                });
            if entry.signature != sig || !updated.is_empty() || entry.rgba.is_empty() {
                entry.signature = sig;
                entry.rgba.clear();
                entry.rgba.reserve(expected * 4);
                for &byte in data.iter().take(expected) {
                    let v = byte as f32 / 255.0;
                    entry.rgba.push(v);
                    entry.rgba.push(v);
                    entry.rgba.push(v);
                    entry.rgba.push(v);
                }
            }
            Some([
                entry.rgba.as_ptr() as usize,
                entry.rgba.len(),
                *width,
                *height,
            ])
        }
        TextureFormat::VecRf32 {
            width,
            height,
            data: Some(data),
            updated,
        } => {
            let expected = width.saturating_mul(*height);
            let sig = TextureConversionSignature {
                kind: 3,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            };
            let entry = cache
                .entry(texture_index)
                .or_insert_with(|| CachedTextureConversion {
                    signature: sig,
                    rgba: Vec::new(),
                });
            if entry.signature != sig || !updated.is_empty() || entry.rgba.is_empty() {
                entry.signature = sig;
                entry.rgba.clear();
                entry.rgba.reserve(expected * 4);
                for &v in data.iter().take(expected) {
                    entry.rgba.push(v);
                    entry.rgba.push(v);
                    entry.rgba.push(v);
                    entry.rgba.push(v);
                }
            }
            Some([
                entry.rgba.as_ptr() as usize,
                entry.rgba.len(),
                *width,
                *height,
            ])
        }
        _ => None,
    }
}

#[derive(Default)]
struct RenderProfile {
    draw_calls: usize,
    parallel_draw_calls: usize,
    serial_draw_calls: usize,
    total_instances: usize,
    total_triangles: usize,
    vertex_ms: f64,
    raster_ms: f64,
    texture_ms: f64,
}

#[allow(clippy::too_many_arguments)]
fn rasterize_instances_rows(
    color_chunk: &mut [[f32; 4]],
    depth_chunk: &mut [f32],
    width: usize,
    height: usize,
    viewport: (usize, usize),
    state: RasterState,
    row_start: usize,
    row_end: usize,
    indices: &[u32],
    instance_count: usize,
    vertex_count: usize,
    varying_slots: usize,
    shaded_positions: &[[f32; 4]],
    shaded_varyings: &[f32],
    flat_slots: usize,
    rcx_template: &[u8],
    rcx_size: usize,
    rcx_f32s: usize,
    rcx_vary_offset: usize,
    rcx_quad_mode_offset: usize,
    rcx_frag_offset: usize,
    uses_derivatives: bool,
    fragment_fn: FragmentFn,
    debug_text: bool,
    is_draw_text_shader: bool,
) {
    let mut rcx_buf = rcx_template.to_vec();
    let mut dx_varyings = if uses_derivatives {
        vec![0.0f32; varying_slots]
    } else {
        Vec::new()
    };
    let mut dy_varyings = if uses_derivatives {
        vec![0.0f32; varying_slots]
    } else {
        Vec::new()
    };
    let shift_start = flat_slots.min(varying_slots);
    let tri_count = indices.len() / 3;
    let vary_bytes = varying_slots * std::mem::size_of::<f32>();
    let mut debug_text_prints = 0usize;
    let mut raster_scratch = RasterScratch::default();

    for inst_idx in 0..instance_count {
        let inst_base = inst_idx * vertex_count;
        for tri_idx in 0..tri_count {
            let i0 = indices[tri_idx * 3] as usize;
            let i1 = indices[tri_idx * 3 + 1] as usize;
            let i2 = indices[tri_idx * 3 + 2] as usize;

            if i0 >= vertex_count || i1 >= vertex_count || i2 >= vertex_count {
                continue;
            }

            let v0_idx = inst_base + i0;
            let v1_idx = inst_base + i1;
            let v2_idx = inst_base + i2;

            if v0_idx >= shaded_positions.len()
                || v1_idx >= shaded_positions.len()
                || v2_idx >= shaded_positions.len()
            {
                continue;
            }

            let v0_off = v0_idx * varying_slots;
            let v1_off = v1_idx * varying_slots;
            let v2_off = v2_idx * varying_slots;

            if v0_off + varying_slots > shaded_varyings.len()
                || v1_off + varying_slots > shaded_varyings.len()
                || v2_off + varying_slots > shaded_varyings.len()
            {
                continue;
            }

            let p0 = &shaded_positions[v0_idx];
            let p1 = &shaded_positions[v1_idx];
            let p2 = &shaded_positions[v2_idx];
            let vary0 = &shaded_varyings[v0_off..v0_off + varying_slots];
            let vary1 = &shaded_varyings[v1_off..v1_off + varying_slots];
            let vary2 = &shaded_varyings[v2_off..v2_off + varying_slots];

            if uses_derivatives {
                let mut frag_closure = |varyings: &[f32],
                                        derivs: &TriangleDerivatives,
                                        lane_x: u32,
                                        lane_y: u32,
                                        x: i32,
                                        y: i32|
                 -> Option<[f32; 4]> {
                    for i in 0..varyings.len() {
                        if i < shift_start {
                            dx_varyings[i] = varyings[i];
                            dy_varyings[i] = varyings[i];
                        } else {
                            dx_varyings[i] = varyings[i] + derivs.dvary_dx[i];
                            dy_varyings[i] = varyings[i] + derivs.dvary_dy[i];
                        }
                    }

                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 8, lane_x);
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 12, lane_y);
                    write_varyings(
                        &mut rcx_buf,
                        rcx_vary_offset,
                        &dx_varyings,
                        vary_bytes,
                        rcx_size,
                    );
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset, 0);
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 4, 0);
                    unsafe {
                        fragment_fn(rcx_buf.as_mut_ptr() as *mut f32, rcx_f32s as u32);
                    }

                    write_varyings(
                        &mut rcx_buf,
                        rcx_vary_offset,
                        &dy_varyings,
                        vary_bytes,
                        rcx_size,
                    );
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset, 1);
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 4, 0);
                    unsafe {
                        fragment_fn(rcx_buf.as_mut_ptr() as *mut f32, rcx_f32s as u32);
                    }

                    write_varyings(
                        &mut rcx_buf,
                        rcx_vary_offset,
                        varyings,
                        vary_bytes,
                        rcx_size,
                    );
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset, 2);
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 4, 0);
                    let write_pixel =
                        unsafe { fragment_fn(rcx_buf.as_mut_ptr() as *mut f32, rcx_f32s as u32) };
                    if write_pixel == 0 {
                        return None;
                    }

                    if rcx_frag_offset + 16 <= rcx_size {
                        let color_ptr =
                            unsafe { rcx_buf.as_ptr().add(rcx_frag_offset) as *const [f32; 4] };
                        let color = unsafe { *color_ptr };
                        if debug_text && is_draw_text_shader && debug_text_prints < 120 {
                            let text_t_slot = shift_start + 2;
                            if text_t_slot + 1 < varyings.len() {
                                let a = color[3];
                                if a > 0.0 && a < 1.0 {
                                    eprintln!(
                                        "[headless][draw_text] px=({}, {}) lane=({}, {}) t=({:.6}, {:.6}) dFdx(t)=({:.6}, {:.6}) dFdy(t)=({:.6}, {:.6}) a={:.5}",
                                        x,
                                        y,
                                        lane_x,
                                        lane_y,
                                        varyings[text_t_slot],
                                        varyings[text_t_slot + 1],
                                        derivs.dvary_dx[text_t_slot],
                                        derivs.dvary_dx[text_t_slot + 1],
                                        derivs.dvary_dy[text_t_slot],
                                        derivs.dvary_dy[text_t_slot + 1],
                                        a,
                                    );
                                    debug_text_prints += 1;
                                }
                            }
                        }
                        Some(color)
                    } else {
                        Some([0.0, 0.0, 0.0, 0.0])
                    }
                };

                rasterize_triangle_rows(
                    width,
                    height,
                    viewport,
                    state,
                    row_start,
                    row_end,
                    color_chunk,
                    depth_chunk,
                    p0,
                    vary0,
                    p1,
                    vary1,
                    p2,
                    vary2,
                    flat_slots,
                    true,
                    &mut raster_scratch,
                    &mut frag_closure,
                );
            } else {
                let mut frag_closure = |varyings: &[f32],
                                        _derivs: &TriangleDerivatives,
                                        lane_x: u32,
                                        lane_y: u32,
                                        _x: i32,
                                        _y: i32|
                 -> Option<[f32; 4]> {
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 8, lane_x);
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 12, lane_y);
                    write_varyings(
                        &mut rcx_buf,
                        rcx_vary_offset,
                        varyings,
                        vary_bytes,
                        rcx_size,
                    );
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset, 2);
                    set_u32(&mut rcx_buf, rcx_quad_mode_offset + 4, 0);
                    let write_pixel =
                        unsafe { fragment_fn(rcx_buf.as_mut_ptr() as *mut f32, rcx_f32s as u32) };
                    if write_pixel == 0 {
                        return None;
                    }
                    if rcx_frag_offset + 16 <= rcx_size {
                        let color_ptr =
                            unsafe { rcx_buf.as_ptr().add(rcx_frag_offset) as *const [f32; 4] };
                        Some(unsafe { *color_ptr })
                    } else {
                        Some([0.0, 0.0, 0.0, 0.0])
                    }
                };

                rasterize_triangle_rows(
                    width,
                    height,
                    viewport,
                    state,
                    row_start,
                    row_end,
                    color_chunk,
                    depth_chunk,
                    p0,
                    vary0,
                    p1,
                    vary1,
                    p2,
                    vary2,
                    flat_slots,
                    false,
                    &mut raster_scratch,
                    &mut frag_closure,
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

impl Cx {
    fn headless_render_thread_count(&self) -> usize {
        let cpu_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(self.cpu_cores.max(1));
        configured_render_threads(cpu_threads.max(1))
    }

    fn headless_ensure_render_pool(&mut self, threads: usize) {
        let threads = threads.max(1);
        if threads <= 1 {
            return;
        }
        if self.os.render_pool.is_none() || self.os.render_pool_threads != threads {
            self.os.render_pool = Some(crate::thread::MessageThreadPool::new(self, threads));
            self.os.render_pool_threads = threads;
        }
    }

    /// Render all dirty passes and return framebuffers keyed by window_id.
    pub(crate) fn headless_render_all_passes(&mut self, time: f64) -> Vec<(usize, Framebuffer)> {
        let frame_start = std::time::Instant::now();
        let profile_enabled = std::env::var("MAKEPAD_HEADLESS_PROFILE").is_ok();
        let parallel_min_tris = configured_parallel_min_tris(1);
        let mut profile = RenderProfile::default();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        let render_threads = self.headless_render_thread_count();
        self.headless_ensure_render_pool(render_threads);

        let mut results = Vec::new();
        let mut texture_cache = std::mem::take(&mut self.os.texture_conversions);
        // Taken out of `self.os` so a pass can hold `&mut` its own framebuffer
        // while the sampler reads its already-rendered siblings.
        let mut render_targets = std::mem::take(&mut self.os.render_targets);

        for draw_pass_id in &passes_todo {
            self.passes[*draw_pass_id].paint_dirty = false;

            let parent = self.passes[*draw_pass_id].parent.clone();
            match parent {
                CxDrawPassParent::Window(window_id) => {
                    let window = &self.windows[window_id];
                    let size = window.window_geom.inner_size;
                    let dpi_factor = window.window_geom.dpi_factor;

                    let width = (size.x * dpi_factor).round().max(1.0) as usize;
                    let height = (size.y * dpi_factor).round().max(1.0) as usize;

                    // Set up pass uniforms
                    if !self.passes[*draw_pass_id].keep_camera_matrix {
                        self.passes[*draw_pass_id].set_ortho_matrix(dvec2(0.0, 0.0), size);
                    }
                    self.passes[*draw_pass_id].set_dpi_factor(dpi_factor);
                    self.passes[*draw_pass_id].set_time(time as f32);

                    let mut fb = Framebuffer::new(width, height);
                    let clear = self.passes[*draw_pass_id].clear_color;
                    fb.clear([clear.x, clear.y, clear.z, clear.w], 1.0);

                    self.headless_draw_pass(
                        *draw_pass_id,
                        render_threads,
                        parallel_min_tris,
                        &mut fb,
                        PassRaster {
                            viewport: (width, height),
                            // The window's swapchain image is 8-bit unorm.
                            clamp_unorm: true,
                        },
                        &mut texture_cache,
                        &render_targets,
                        if profile_enabled {
                            Some(&mut profile)
                        } else {
                            None
                        },
                    );
                    results.push((window_id.id(), fb));
                }
                // Render-to-texture. `CxDrawPassParent::None` is a texture pass
                // too (the GPU backends treat both the same way): it just has no
                // parent that composites it back.
                CxDrawPassParent::DrawPass(_) | CxDrawPassParent::None => {
                    self.headless_draw_pass_to_texture(
                        *draw_pass_id,
                        time,
                        render_threads,
                        parallel_min_tris,
                        &mut texture_cache,
                        &mut render_targets,
                        if profile_enabled {
                            Some(&mut profile)
                        } else {
                            None
                        },
                    );
                }
                CxDrawPassParent::Xr => {}
            }
        }

        // Hand the conversions back for the next frame to reuse.
        self.os.texture_conversions = texture_cache;
        self.headless_prune_render_targets(&mut render_targets);
        self.os.render_targets = render_targets;

        let elapsed = frame_start.elapsed();
        if profile_enabled {
            crate::log!(
                "[headless] frame render: {:.1}ms",
                elapsed.as_secs_f64() * 1000.0
            );
        }
        if profile_enabled {
            crate::log!(
                "[headless][profile] draws={} serial={} parallel={} inst={} tris={} vertex={:.1}ms raster={:.1}ms texture={:.1}ms",
                profile.draw_calls,
                profile.serial_draw_calls,
                profile.parallel_draw_calls,
                profile.total_instances,
                profile.total_triangles,
                profile.vertex_ms,
                profile.raster_ms,
                profile.texture_ms
            );
        }

        results
    }

    /// Drop framebuffers whose texture slot is gone or has been recycled into
    /// something that is not a render target, so a churn of short-lived
    /// offscreen targets cannot grow the store without bound.
    fn headless_prune_render_targets(&mut self, render_targets: &mut HeadlessRenderTargets) {
        let pool = &self.textures.0.pool;
        render_targets.framebuffers.retain(|texture_index, _| {
            pool.get(*texture_index)
                .map(|slot| slot.item.format.as_render_alloc(1, 1).is_some())
                .unwrap_or(false)
        });
    }

    /// Render one offscreen pass into the framebuffer that backs its colour
    /// attachment, and publish that framebuffer so parent passes sampling the
    /// texture find the pixels. The GPU backends do exactly this with a render
    /// pass descriptor; here the "texture" is the framebuffer itself.
    #[allow(clippy::too_many_arguments)]
    fn headless_draw_pass_to_texture(
        &mut self,
        draw_pass_id: DrawPassId,
        time: f64,
        render_threads: usize,
        parallel_min_tris: usize,
        texture_cache: &mut TextureConversionCache,
        render_targets: &mut HeadlessRenderTargets,
        profile: Option<&mut RenderProfile>,
    ) {
        if self.passes[draw_pass_id].main_draw_list_id.is_none() {
            return;
        }
        // A pass with no colour attachment has nowhere to render (and nothing
        // could sample it) — the GPU backends log an invalid render target.
        let Some(color_texture) = self.passes[draw_pass_id].color_textures.first().cloned() else {
            return;
        };
        // Cube faces need six framebuffers behind one texture; not modelled.
        if color_texture.cube_face.is_some() {
            return;
        }
        let texture_id = color_texture.texture.texture_id();

        let dpi_factor = match self.passes[draw_pass_id].dpi_factor {
            Some(dpi) => dpi,
            None => self.get_delegated_dpi_factor(draw_pass_id),
        };
        let Some(pass_rect) = self.get_pass_rect(draw_pass_id, dpi_factor) else {
            return;
        };
        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            return;
        }
        // Same arithmetic the GPU backends use: the viewport is dpi * pass rect
        // anchored at the attachment's top-left corner, while the attachment
        // itself may be larger when `TextureSize::Fixed` pins it.
        let viewport_width = (dpi_factor * pass_rect.size.x).max(1.0) as usize;
        let viewport_height = (dpi_factor * pass_rect.size.y).max(1.0) as usize;
        let (width, height) = {
            let cxtexture = &mut self.textures[texture_id];
            cxtexture.alloc_render(viewport_width, viewport_height);
            match cxtexture
                .alloc
                .as_ref()
                .map(|alloc| (alloc.width, alloc.height))
            {
                Some((w, h)) if w > 0 && h > 0 => (w, h),
                _ => (viewport_width, viewport_height),
            }
        };
        let pass_raster = PassRaster {
            viewport: (viewport_width.min(width), viewport_height.min(height)),
            // Only the 8-bit attachments quantize; the float formats exist
            // precisely so a data pass can store a value outside [0,1].
            clamp_unorm: matches!(
                self.textures[texture_id].format,
                TextureFormat::RenderBGRAu8 { .. } | TextureFormat::RenderCubeBGRAu8 { .. }
            ),
        };

        if !self.passes[draw_pass_id].keep_camera_matrix {
            self.passes[draw_pass_id].set_ortho_matrix(pass_rect.pos, pass_rect.size);
        }
        self.passes[draw_pass_id].set_dpi_factor(dpi_factor);
        self.passes[draw_pass_id].set_time(time as f32);

        // Taking the framebuffer out of the store lets the raster below sample
        // every *other* offscreen target while writing into this one.
        let mut fb = render_targets
            .framebuffers
            .remove(&texture_id.0)
            .unwrap_or_else(|| Framebuffer::new(width, height));
        let discarded = fb.resize(width, height);

        // Load actions, mirroring the GPU backends: ClearWith always clears,
        // InitWith clears only the first time the attachment is used. A resize
        // threw the old contents away, so a "load" there has to clear anyway.
        let clear_color = match color_texture.clear_color {
            DrawPassClearColor::ClearWith(color) => Some(color),
            DrawPassClearColor::InitWith(color) => {
                let initial = self.textures[texture_id].take_initial();
                (initial || discarded).then_some(color)
            }
        };
        if let Some(c) = clear_color {
            fb.clear_color([c.x, c.y, c.z, c.w]);
        }
        let clear_depth = match self.passes[draw_pass_id].clear_depth {
            DrawPassClearDepth::ClearWith(depth) => Some(depth),
            DrawPassClearDepth::InitWith(depth) => {
                let initial = match &self.passes[draw_pass_id].depth_texture {
                    Some(texture) => {
                        let texture_id = texture.texture_id();
                        self.textures[texture_id].take_initial()
                    }
                    None => true,
                };
                (initial || discarded).then_some(depth)
            }
        };
        if let Some(depth) = clear_depth {
            fb.clear_depth(depth);
        }

        self.headless_draw_pass(
            draw_pass_id,
            render_threads,
            parallel_min_tris,
            &mut fb,
            pass_raster,
            texture_cache,
            render_targets,
            profile,
        );

        render_targets.framebuffers.insert(texture_id.0, fb);
    }

    #[allow(clippy::too_many_arguments)]
    fn headless_draw_pass(
        &mut self,
        draw_pass_id: DrawPassId,
        render_threads: usize,
        parallel_min_tris: usize,
        fb: &mut Framebuffer,
        pass_raster: PassRaster,
        texture_cache: &mut TextureConversionCache,
        render_targets: &HeadlessRenderTargets,
        mut profile: Option<&mut RenderProfile>,
    ) {
        let draw_list_id = match self.passes[draw_pass_id].main_draw_list_id {
            Some(id) => id,
            None => return,
        };

        let zbias_step = self.passes[draw_pass_id].zbias_step;
        let mut zbias = 0.0f32;

        self.headless_render_view(
            draw_pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            render_threads,
            parallel_min_tris,
            fb,
            pass_raster,
            texture_cache,
            render_targets,
            profile.as_deref_mut(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn headless_render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        render_threads: usize,
        parallel_min_tris: usize,
        fb: &mut Framebuffer,
        pass_raster: PassRaster,
        texture_cache: &mut TextureConversionCache,
        render_targets: &HeadlessRenderTargets,
        mut profile: Option<&mut RenderProfile>,
    ) {
        let only_shader = std::env::var("MAKEPAD_HEADLESS_ONLY_SHADER").ok();
        let debug_text = std::env::var("MAKEPAD_HEADLESS_DEBUG_TEXT").is_ok();
        let draw_order_len = self.draw_lists[draw_list_id].draw_item_order_len();

        for order_index in 0..draw_order_len {
            let Some(draw_item_id) =
                self.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            let kind_tag = match &self.draw_lists[draw_list_id].draw_items[draw_item_id].kind {
                CxDrawKind::SubList(sub_id) => Some(*sub_id),
                CxDrawKind::DrawCall(_) => None,
                CxDrawKind::Empty => continue,
            };

            if let Some(sub_list_id) = kind_tag {
                let child_resets_zbias = self.draw_lists[sub_list_id].reset_zbias;
                let mut child_zbias = 0.0f32;
                self.headless_render_view(
                    draw_pass_id,
                    sub_list_id,
                    if child_resets_zbias {
                        &mut child_zbias
                    } else {
                        zbias
                    },
                    zbias_step,
                    render_threads,
                    parallel_min_tris,
                    fb,
                    pass_raster,
                    texture_cache,
                    render_targets,
                    profile.as_deref_mut(),
                );
                continue;
            }

            let current_zbias = *zbias;
            {
                if let CxDrawKind::DrawCall(dc) =
                    &mut self.draw_lists[draw_list_id].draw_items[draw_item_id].kind
                {
                    dc.draw_call_uniforms.set_zbias(current_zbias);
                }
            }
            *zbias += zbias_step;

            let draw_item = &self.draw_lists[draw_list_id].draw_items[draw_item_id];
            let draw_call = match &draw_item.kind {
                CxDrawKind::DrawCall(dc) => dc,
                _ => continue,
            };

            let shader_id = draw_call.draw_shader_id;
            let depth_write = draw_call.options.depth_write;
            let sh = &self.draw_shaders.shaders[shader_id.index];
            let color_format = sh.mapping.color_format;
            let os_shader_id = match sh.os_shader_id {
                Some(id) => id,
                None => continue,
            };
            let is_draw_text_shader = match &sh.mapping.code {
                CxDrawShaderCode::Combined { code } => code.contains("sample_text_pixel"),
                CxDrawShaderCode::Separate { fragment, .. } => {
                    fragment.contains("sample_text_pixel")
                }
            };
            if let Some(only) = &only_shader {
                let keep = match only.as_str() {
                    "draw_text" => is_draw_text_shader,
                    _ => true,
                };
                if !keep {
                    continue;
                }
            }
            let os_shader = &self.draw_shaders.os_shaders[os_shader_id];
            let module = match &os_shader.module {
                Some(m) => m,
                None => continue,
            };

            // Load function pointers
            let vertex_fn: VertexFn = match module.symbol("makepad_headless_vertex") {
                Ok(f) => f,
                Err(_) => continue,
            };
            let fragment_fn: FragmentFn = match module.symbol("makepad_headless_fragment") {
                Ok(f) => f,
                Err(_) => continue,
            };

            // RenderCx layout info
            let rcx_size = os_shader.rcx_size;
            let rcx_vary_offset = os_shader.rcx_vary_offset;
            let rcx_quad_mode_offset = os_shader.rcx_quad_mode_offset;
            let rcx_frag_offset = os_shader.rcx_frag_offset;

            if rcx_size == 0 {
                continue;
            }

            // Per-draw-call RenderCx template (uniforms + textures) copied per worker.
            let rcx_f32s = rcx_size / std::mem::size_of::<f32>();
            let mut rcx_template = vec![0u8; rcx_size];

            // ── Per-draw-call: build uniform buffer arrays ──
            let draw_call_uniforms_slice = draw_call.draw_call_uniforms.as_slice();
            let pass_uniforms_slice = self.passes[draw_pass_id].pass_uniforms.as_slice();
            let draw_list_uniforms_slice =
                self.draw_lists[draw_list_id].draw_list_uniforms.as_slice();
            let dyn_uniforms = &draw_call.dyn_uniforms;
            let scope_buf = &sh.mapping.scope_uniforms_buf;
            let bindings = &sh.mapping.uniform_buffer_bindings;

            let max_buf_idx = bindings
                .bindings
                .iter()
                .map(|(_, idx)| *idx)
                .max()
                .unwrap_or(0);
            let dyn_buf_idx = max_buf_idx + 1;
            let scope_buf_idx = dyn_buf_idx + 1;
            let has_scope = !scope_buf.is_empty();
            let total_buffers = if has_scope {
                scope_buf_idx + 1
            } else {
                dyn_buf_idx + 1
            };

            const MAX_UNIFORM_BUFS: usize = 16;
            let total_buffers = total_buffers.min(MAX_UNIFORM_BUFS);
            let mut ptrs = [std::ptr::null::<f32>(); MAX_UNIFORM_BUFS];
            let mut lens = [0u32; MAX_UNIFORM_BUFS];

            for (type_name, idx) in &bindings.bindings {
                if *idx >= MAX_UNIFORM_BUFS {
                    continue;
                }
                if *type_name == id!(DrawCallUniforms) {
                    ptrs[*idx] = draw_call_uniforms_slice.as_ptr();
                    lens[*idx] = draw_call_uniforms_slice.len() as u32;
                } else if *type_name == id!(DrawPassUniforms) {
                    ptrs[*idx] = pass_uniforms_slice.as_ptr();
                    lens[*idx] = pass_uniforms_slice.len() as u32;
                } else if *type_name == id!(DrawListUniforms) {
                    ptrs[*idx] = draw_list_uniforms_slice.as_ptr();
                    lens[*idx] = draw_list_uniforms_slice.len() as u32;
                }
            }

            if dyn_buf_idx < MAX_UNIFORM_BUFS {
                ptrs[dyn_buf_idx] = dyn_uniforms.as_ptr();
                lens[dyn_buf_idx] = dyn_uniforms.len() as u32;
            }

            if has_scope && scope_buf_idx < MAX_UNIFORM_BUFS {
                ptrs[scope_buf_idx] = scope_buf.as_ptr();
                lens[scope_buf_idx] = scope_buf.len() as u32;
            }

            let uniform_count = total_buffers as u32;
            let uniform_ptrs = ptrs.as_ptr();
            let uniform_lens = lens.as_ptr();

            // ── Gather texture pointers, converting/caching to RGBA f32 when needed ──
            let mut tex_infos: Vec<[usize; 4]> = Vec::with_capacity(sh.mapping.textures.len());

            for tex_idx in 0..sh.mapping.textures.len() {
                if let Some(texture) = &draw_call.texture_slots[tex_idx] {
                    let texture_id = texture.texture_id();
                    let cxtexture = &self.textures[texture_id];
                    let __tex_t0 = std::time::Instant::now();
                    let __info =
                        headless_texture_info(texture_id.0, cxtexture, texture_cache, render_targets);
                    if let Some(p) = profile.as_deref_mut() {
                        p.texture_ms += __tex_t0.elapsed().as_secs_f64() * 1000.0;
                    }
                    if let Some(info) = __info
                    {
                        tex_infos.push(info);
                    } else {
                        tex_infos.push([0, 0, 0, 0]);
                    }
                } else {
                    tex_infos.push([0, 0, 0, 0]);
                }
            }

            // ── Fill RenderCx buffer: uniforms + textures (per-draw-call, cold path) ──
            type FillUniformsFn = unsafe extern "C" fn(
                rcx_ptr: *mut f32,
                rcx_f32s: u32,
                uniform_ptrs: *const *const f32,
                uniform_lens: *const u32,
                uniform_count: u32,
                tex_infos_ptr: *const [usize; 4],
                tex_count: u32,
            );
            if let Ok(fill_fn) = module.symbol::<FillUniformsFn>("makepad_headless_fill_rcx") {
                unsafe {
                    fill_fn(
                        rcx_template.as_mut_ptr() as *mut f32,
                        rcx_f32s as u32,
                        uniform_ptrs,
                        uniform_lens,
                        uniform_count,
                        tex_infos.as_ptr(),
                        tex_infos.len() as u32,
                    );
                }
            }

            // Get geometry
            let geometry_id = match draw_call.geometry_id {
                Some(id) => id,
                None => continue,
            };
            let geom = &self.geometries[geometry_id];
            let vertices = &geom.vertices;
            let indices = &geom.indices;

            if indices.is_empty() || vertices.is_empty() {
                continue;
            }

            let instances_data = match &draw_item.instances {
                Some(data) => data.as_slice(),
                None => continue,
            };

            let total_instance_slots = sh.mapping.instances.total_slots;
            if total_instance_slots == 0 {
                continue;
            }
            let instance_count = instances_data.len() / total_instance_slots;
            if instance_count == 0 {
                continue;
            }
            if sh.mapping.flags.debug_draw {
                CxDrawShaderMapping::debug_dump_shader_draw_call(
                    "headless",
                    draw_item_id,
                    sh,
                    draw_call,
                    instances_data,
                    instance_count,
                );
            }

            let geom_slots = sh.mapping.geometries.total_slots;
            let varying_slots = sh.mapping.varying_total_slots;

            let vertex_count = if geom_slots > 0 {
                vertices.len() / geom_slots
            } else {
                0
            };
            if vertex_count == 0 {
                continue;
            }
            let tri_count = indices.len() / 3;
            if tri_count == 0 {
                continue;
            }
            if let Some(p) = profile.as_deref_mut() {
                p.draw_calls += 1;
                p.total_instances += instance_count;
                p.total_triangles += tri_count * instance_count;
            }

            let vertex_start = std::time::Instant::now();
            let shaded_vert_count = instance_count * vertex_count;
            let mut shaded_positions = vec![[0.0f32; 4]; shaded_vert_count];
            let mut shaded_varyings = vec![0.0f32; shaded_vert_count * varying_slots];

            for inst_idx in 0..instance_count {
                let inst_offset = inst_idx * total_instance_slots;
                let inst_slice = &instances_data[inst_offset..inst_offset + total_instance_slots];
                let inst_base = inst_idx * vertex_count;

                for vert_idx in 0..vertex_count {
                    let geom_offset = vert_idx * geom_slots;
                    let geom_slice = &vertices[geom_offset..geom_offset + geom_slots];
                    let shaded_idx = inst_base + vert_idx;
                    let vary_offset = shaded_idx * varying_slots;
                    let varying_out = &mut shaded_varyings
                        [vary_offset..vary_offset.saturating_add(varying_slots)];

                    unsafe {
                        vertex_fn(
                            geom_slice.as_ptr(),
                            geom_slice.len() as u32,
                            inst_slice.as_ptr(),
                            inst_slice.len() as u32,
                            uniform_ptrs,
                            uniform_lens,
                            uniform_count,
                            varying_out.as_mut_ptr(),
                            varying_slots as u32,
                            &mut shaded_positions[shaded_idx],
                        );
                    }
                }
            }
            if let Some(p) = profile.as_deref_mut() {
                p.vertex_ms += vertex_start.elapsed().as_secs_f64() * 1000.0;
            }

            let flat_slots = os_shader.flat_varying_slots.min(varying_slots);
            let uses_derivatives = os_shader.uses_derivatives;
            let row_chunks = compute_row_chunks(fb.height, render_threads);
            // Same pipeline state the GPU backends build from the shader: the
            // data-pass colour formats disable blending (their alpha channel is
            // payload — an SDF byte, a depth — and a premultiplied over blend
            // can only ever grow it), and `depth_write: false` shaders must not
            // touch the depth buffer.
            let state = RasterState {
                blend: matches!(color_format, DrawShaderColorFormat::Bgra8Unorm),
                depth_write,
                clamp_unorm: pass_raster.clamp_unorm,
            };
            let viewport = pass_raster.viewport;
            let use_parallel = row_chunks.len() > 1
                && tri_count.saturating_mul(instance_count) >= parallel_min_tris
                && self.os.render_pool.is_some();
            if let Some(p) = profile.as_deref_mut() {
                if use_parallel {
                    p.parallel_draw_calls += 1;
                } else {
                    p.serial_draw_calls += 1;
                }
            }

            let raster_start = std::time::Instant::now();
            if use_parallel {
                let pool = self.os.render_pool.as_ref().unwrap();
                let (done_tx, done_rx) = mpsc::channel::<()>();
                let width = fb.width;
                let height = fb.height;
                let color_ptr = fb.color.as_mut_ptr() as usize;
                let depth_ptr = fb.depth.as_mut_ptr() as usize;
                let indices_ptr = indices.as_ptr() as usize;
                let indices_len = indices.len();
                let shaded_positions_ptr = shaded_positions.as_ptr() as usize;
                let shaded_positions_len = shaded_positions.len();
                let shaded_varyings_ptr = shaded_varyings.as_ptr() as usize;
                let shaded_varyings_len = shaded_varyings.len();
                let rcx_template_ptr = rcx_template.as_ptr() as usize;
                let rcx_template_len = rcx_template.len();

                for chunk in row_chunks.iter().copied() {
                    let done_tx = done_tx.clone();
                    pool.execute(move |_| {
                        let row_start = chunk.start;
                        let row_end = chunk.end;
                        let row_count = row_end.saturating_sub(row_start);
                        if row_count == 0 {
                            let _ = done_tx.send(());
                            return;
                        }

                        let pixel_offset = row_start * width;
                        let pixel_count = row_count * width;
                        let color_chunk = unsafe {
                            std::slice::from_raw_parts_mut(
                                (color_ptr as *mut [f32; 4]).add(pixel_offset),
                                pixel_count,
                            )
                        };
                        let depth_chunk = unsafe {
                            std::slice::from_raw_parts_mut(
                                (depth_ptr as *mut f32).add(pixel_offset),
                                pixel_count,
                            )
                        };
                        let indices = unsafe {
                            std::slice::from_raw_parts(indices_ptr as *const u32, indices_len)
                        };
                        let shaded_positions = unsafe {
                            std::slice::from_raw_parts(
                                shaded_positions_ptr as *const [f32; 4],
                                shaded_positions_len,
                            )
                        };
                        let shaded_varyings = unsafe {
                            std::slice::from_raw_parts(
                                shaded_varyings_ptr as *const f32,
                                shaded_varyings_len,
                            )
                        };
                        let rcx_template = unsafe {
                            std::slice::from_raw_parts(
                                rcx_template_ptr as *const u8,
                                rcx_template_len,
                            )
                        };

                        rasterize_instances_rows(
                            color_chunk,
                            depth_chunk,
                            width,
                            height,
                            viewport,
                            state,
                            row_start,
                            row_end,
                            indices,
                            instance_count,
                            vertex_count,
                            varying_slots,
                            shaded_positions,
                            shaded_varyings,
                            flat_slots,
                            rcx_template,
                            rcx_size,
                            rcx_f32s,
                            rcx_vary_offset,
                            rcx_quad_mode_offset,
                            rcx_frag_offset,
                            uses_derivatives,
                            fragment_fn,
                            debug_text,
                            is_draw_text_shader,
                        );

                        let _ = done_tx.send(());
                    });
                }

                drop(done_tx);
                for _ in 0..row_chunks.len() {
                    if done_rx.recv().is_err() {
                        break;
                    }
                }
            } else {
                rasterize_instances_rows(
                    fb.color.as_mut_slice(),
                    fb.depth.as_mut_slice(),
                    fb.width,
                    fb.height,
                    viewport,
                    state,
                    0,
                    fb.height,
                    indices,
                    instance_count,
                    vertex_count,
                    varying_slots,
                    &shaded_positions,
                    &shaded_varyings,
                    flat_slots,
                    &rcx_template,
                    rcx_size,
                    rcx_f32s,
                    rcx_vary_offset,
                    rcx_quad_mode_offset,
                    rcx_frag_offset,
                    uses_derivatives,
                    fragment_fn,
                    debug_text,
                    is_draw_text_shader,
                );
            }
            if let Some(p) = profile.as_deref_mut() {
                p.raster_ms += raster_start.elapsed().as_secs_f64() * 1000.0;
            }
        }
    }
}

/// Copy varying data into the rcx buffer at the given offset.
#[inline]
fn write_varyings(
    rcx_buf: &mut [u8],
    offset: usize,
    varyings: &[f32],
    vary_bytes: usize,
    rcx_size: usize,
) {
    if offset + vary_bytes <= rcx_size {
        unsafe {
            std::ptr::copy_nonoverlapping(
                varyings.as_ptr() as *const u8,
                rcx_buf.as_mut_ptr().add(offset),
                vary_bytes,
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PNG encoding
// ─────────────────────────────────────────────────────────────────────────────

pub fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "rgba size overflow while encoding png".to_string())?;
    if rgba.len() != expected {
        return Err(format!(
            "encode_png_rgba: expected {} bytes, got {}",
            expected,
            rgba.len()
        ));
    }

    let options = EncoderOptions::default()
        .set_width(width as usize)
        .set_height(height as usize)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);

    let mut encoder = PngEncoder::new(rgba, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| format!("headless png encode failed: {err:?}"))?;
    Ok(out)
}
