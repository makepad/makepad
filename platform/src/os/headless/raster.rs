use super::virtual_gpu::{
    quantize_color_unorm8, rasterize_setup_rows, setup_triangle, Framebuffer, RasterScratch,
    RasterState, TriSetup, TriangleDerivatives,
};
use crate::{
    cx::Cx,
    draw_list::{CxDrawKind, DrawListId},
    draw_pass::{CxDrawPassParent, DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::{CxDrawShaderCode, CxDrawShaderMapping, DrawShaderColorFormat},
    makepad_live_id::*,
    makepad_math::*,
    texture::{TextureFormat, TextureUpdated},
};
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::collections::HashMap;

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

fn configured_render_threads(default_threads: usize) -> usize {
    std::env::var("MAKEPAD_HEADLESS_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default_threads.max(1))
}

fn configured_parallel_min_tris(default_min: usize) -> usize {
    std::env::var("MAKEPAD_HEADLESS_PARALLEL_MIN_TRIS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default_min)
}

/// Per-frame raster knobs, read from the environment once instead of on every
/// draw list. `headless_render_view` recurses per sub-list, so the three
/// `getenv` calls it used to make ran hundreds of times a frame.
#[derive(Clone)]
struct RenderOptions {
    threads: usize,
    /// Minimum triangles in a draw call before it is worth splitting.
    parallel_min_tris: usize,
    /// Minimum estimated covered pixels before it is worth splitting.
    parallel_min_pixels: usize,
    /// `MAKEPAD_HEADLESS_ONLY_SHADER` — draw only the named shader class.
    only_shader: Option<String>,
    /// `MAKEPAD_HEADLESS_DEBUG_TEXT` — dump per-fragment text shader state.
    debug_text: bool,
}

impl RenderOptions {
    fn from_env(threads: usize) -> Self {
        Self {
            threads,
            parallel_min_tris: configured_parallel_min_tris(1),
            parallel_min_pixels: parallel_min_pixels(),
            only_shader: std::env::var("MAKEPAD_HEADLESS_ONLY_SHADER").ok(),
            debug_text: std::env::var("MAKEPAD_HEADLESS_DEBUG_TEXT").is_ok(),
        }
    }
}

/// Rows a band must have before splitting one off is worth a thread hand-off.
const MIN_BAND_ROWS: usize = 8;

/// Estimated covered pixels below which a draw call stays on the calling
/// thread. Spinning threads up for a button-sized quad costs more than the
/// quad does, and a UI frame is mostly button-sized quads.
fn parallel_min_pixels() -> usize {
    const DEFAULT: usize = 2048;
    std::env::var("MAKEPAD_HEADLESS_PARALLEL_MIN_PIXELS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT)
}

/// Split the rows a draw call actually touches into bands for the workers.
///
/// Deliberately more bands than threads: a draw call's cost is concentrated in
/// whichever rows its expensive fragments land on, so an even row split leaves
/// most workers idle while one finishes. Workers pull the next band when they
/// free up, which turns that into a balanced queue.
fn compute_row_bands(
    band_lo: usize,
    band_hi: usize,
    threads: usize,
    covered_px: usize,
    min_pixels: usize,
) -> Vec<(usize, usize)> {
    let total_rows = band_hi.saturating_sub(band_lo);
    if total_rows == 0 {
        return Vec::new();
    }
    if threads <= 1 || covered_px < min_pixels || total_rows < MIN_BAND_ROWS * 2 {
        return vec![(band_lo, band_hi)];
    }
    // Scale the split with the work: handing a four-thousand-pixel quad to
    // sixteen threads spends more on starting them than they save. One thread
    // per `min_pixels` of estimated coverage, capped at the pool size.
    let useful_threads = (covered_px / min_pixels.max(1)).clamp(1, threads);
    let want = (useful_threads * 4).max(1);
    let max_bands = (total_rows / MIN_BAND_ROWS).max(1);
    let count = want.min(max_bands);
    let base = total_rows / count;
    let rem = total_rows % count;
    let mut bands = Vec::with_capacity(count);
    let mut start = band_lo;
    for i in 0..count {
        let rows = base + usize::from(i < rem);
        let end = (start + rows).min(band_hi);
        if end > start {
            bands.push((start, end));
        }
        start = end;
    }
    bands
}

/// One band's exclusive view of the framebuffer.
struct RowBand<'a> {
    row_start: usize,
    row_end: usize,
    color: &'a mut [[f32; 4]],
    depth: &'a mut [f32],
}

/// Carve the framebuffer into the given bands as disjoint `&mut` slices.
///
/// `bands` must be ordered and non-overlapping, which is what
/// [`compute_row_bands`] produces; the borrow checker then guarantees the
/// workers cannot touch each other's pixels.
fn split_bands<'a>(
    fb: &'a mut Framebuffer,
    band_lo: usize,
    bands: &[(usize, usize)],
) -> Vec<RowBand<'a>> {
    let width = fb.width;
    let has_depth = !fb.depth.is_empty();
    let mut color_rest = &mut fb.color[band_lo * width..];
    let mut depth_rest: &mut [f32] = if has_depth {
        &mut fb.depth[band_lo * width..]
    } else {
        &mut []
    };
    let mut out = Vec::with_capacity(bands.len());
    let mut cursor = band_lo;
    for &(start, end) in bands {
        let skip = start.saturating_sub(cursor) * width;
        let take = end.saturating_sub(start) * width;
        let (_, rest) = std::mem::take(&mut color_rest).split_at_mut(skip);
        let (color, rest) = rest.split_at_mut(take);
        color_rest = rest;
        let depth: &mut [f32] = if has_depth {
            let (_, rest) = std::mem::take(&mut depth_rest).split_at_mut(skip);
            let (depth, rest) = rest.split_at_mut(take);
            depth_rest = rest;
            depth
        } else {
            &mut []
        };
        out.push(RowBand {
            row_start: start,
            row_end: end,
            color,
            depth,
        });
        cursor = end;
    }
    out
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
    /// Frame each target was last written or sampled, for eviction. A `Cell`
    /// because sampling happens behind `&self`, deep inside the draw loop.
    last_used: HashMap<usize, std::cell::Cell<u64>>,
    frame: u64,
    /// Textures already reported as unsupported cube-face targets, so the
    /// warning is one line per texture rather than one per frame forever.
    warned_cube_faces: std::collections::HashSet<usize>,
}

/// Frames a render target may go completely untouched — neither rendered into
/// nor sampled — before its framebuffer is released. A GPU keeps these in VRAM
/// for free; here each one is host RAM, and a lightmap bake leaves a dozen
/// large scratch targets behind after it has run once. Long enough that a
/// target used once every few seconds survives.
const RENDER_TARGET_IDLE_FRAMES: u64 = 120;

/// Retained render-target budget in MB, over which the least recently used
/// targets are released even if they are not idle yet. Anything dropped is
/// rebuilt (cleared) the next time a pass renders into it, so this only ever
/// costs work, never correctness — a bake chain's scratch targets are all
/// written before they are read within the same frame. Tunable through
/// `MAKEPAD_HEADLESS_RT_BUDGET_MB`; 0 disables the cap.
fn render_target_budget_bytes() -> usize {
    const DEFAULT_MB: usize = 512;
    std::env::var("MAKEPAD_HEADLESS_RT_BUDGET_MB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MB)
        .saturating_mul(1024 * 1024)
}

/// Attachment-level raster state: what the render pass descriptor fixes for
/// every draw call inside the pass.
#[derive(Clone, Copy)]
struct PassRaster {
    /// Clip space maps onto this rect, anchored top-left in the attachment.
    viewport: (usize, usize),
    /// The attachment is 8-bit unorm, so writes clamp and quantize.
    unorm8: bool,
    /// The pass has a depth attachment (so fragments depth-test).
    has_depth: bool,
}

impl HeadlessRenderTargets {
    fn color_target(&self, texture_index: usize) -> Option<&Framebuffer> {
        let fb = self.framebuffers.get(&texture_index)?;
        if fb.width == 0 || fb.height == 0 || fb.color.is_empty() {
            return None;
        }
        if let Some(used) = self.last_used.get(&texture_index) {
            used.set(self.frame);
        }
        Some(fb)
    }

    /// CPU readback of a color render target — the headless twin of the GPU
    /// backends' `debug_read_render_texture`. Returns the framebuffer the
    /// raster last rendered for this texture as packed BGRA8 bytes, origin
    /// top-left (the byte layout the Metal readback returns; alpha
    /// unpremultiplied exactly like the window frame writer). `None` when
    /// the target was never rendered or has been evicted.
    pub(crate) fn read_color_bgra8(
        &self,
        texture_id: crate::texture::TextureId,
    ) -> Option<(usize, usize, Vec<u8>)> {
        let fb = self.color_target(texture_id.0)?;
        let mut out = vec![0u8; fb.width * fb.height * 4];
        for (i, c) in fb.color.iter().enumerate() {
            let a = c[3].clamp(0.0, 1.0);
            let inv_a = if a > 0.0 { 1.0 / a } else { 0.0 };
            let base = i * 4;
            out[base] = ((c[2] * inv_a).clamp(0.0, 1.0) * 255.0).round() as u8;
            out[base + 1] = ((c[1] * inv_a).clamp(0.0, 1.0) * 255.0).round() as u8;
            out[base + 2] = ((c[0] * inv_a).clamp(0.0, 1.0) * 255.0).round() as u8;
            out[base + 3] = (a * 255.0).round() as u8;
        }
        Some((fb.width, fb.height, out))
    }

    fn touch(&mut self, texture_index: usize) {
        let frame = self.frame;
        self.last_used
            .entry(texture_index)
            .or_insert_with(|| std::cell::Cell::new(frame))
            .set(frame);
    }

    fn bytes(&self) -> usize {
        self.framebuffers.values().map(|fb| fb.bytes()).sum()
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

/// Re-convert one axis-aligned region of a texture into its RGBA-f32 mirror.
fn convert_rect<T: Copy>(
    dst: &mut [f32],
    src: &[T],
    width: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    texel: &impl Fn(T) -> [f32; 4],
) {
    for y in y0..y1 {
        let row = y * width;
        let (Some(src_row), Some(dst_row)) = (
            src.get(row + x0..row + x1),
            dst.get_mut((row + x0) * 4..(row + x1) * 4),
        ) else {
            continue;
        };
        for (out, &pixel) in dst_row.chunks_exact_mut(4).zip(src_row) {
            out.copy_from_slice(&texel(pixel));
        }
    }
}

/// Keep a texture's RGBA-f32 mirror up to date and hand the sampler a view of it.
///
/// Only the region the texture reports as dirty is re-converted. A glyph atlas
/// is several megabytes and grows by one glyph at a time; re-expanding the whole
/// thing on every update cost more than rasterizing the frame that needed the
/// glyph. `count` is how many texels of `src` the mirror covers — the whole
/// buffer for a mip chain (so the levels are converted once), the base image
/// otherwise.
fn cached_conversion<T: Copy>(
    cache: &mut TextureConversionCache,
    texture_index: usize,
    sig: TextureConversionSignature,
    width: usize,
    height: usize,
    count: usize,
    src: &[T],
    updated: &TextureUpdated,
    texel: impl Fn(T) -> [f32; 4],
) -> Option<[usize; 4]> {
    if width == 0 || height == 0 {
        return None;
    }
    let entry = cache
        .entry(texture_index)
        .or_insert_with(|| CachedTextureConversion {
            signature: sig,
            rgba: Vec::new(),
        });

    // A different buffer, a different size, or a full invalidation means the
    // mirror has to be rebuilt end to end; anything else is a patch.
    let stale = entry.signature != sig || entry.rgba.len() != count * 4;
    if stale || matches!(updated, TextureUpdated::Full) {
        entry.signature = sig;
        entry.rgba.clear();
        entry.rgba.resize(count * 4, 0.0);
        let rows = count / width.max(1);
        convert_rect(&mut entry.rgba, src, width, 0, 0, width, rows, &texel);
    } else if let TextureUpdated::Partial(rect) = updated {
        let x0 = rect.origin.x.min(width);
        let y0 = rect.origin.y.min(height);
        let x1 = rect.origin.x.saturating_add(rect.size.width).min(width);
        let y1 = rect.origin.y.saturating_add(rect.size.height).min(height);
        convert_rect(&mut entry.rgba, src, width, x0, y0, x1, y1, &texel);
    }

    Some([
        entry.rgba.as_ptr() as usize,
        entry.rgba.len(),
        width,
        height,
    ])
}

#[inline]
fn bgra_u32_to_rgba(pixel: u32) -> [f32; 4] {
    const INV: f32 = 1.0 / 255.0;
    [
        ((pixel >> 16) & 0xFF) as f32 * INV,
        ((pixel >> 8) & 0xFF) as f32 * INV,
        (pixel & 0xFF) as f32 * INV,
        ((pixel >> 24) & 0xFF) as f32 * INV,
    ]
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
        } => cached_conversion(
            cache,
            texture_index,
            TextureConversionSignature {
                kind: 1,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            },
            *width,
            *height,
            data.len(),
            data,
            updated,
            bgra_u32_to_rgba,
        ),
        TextureFormat::VecCubeBGRAu8_32 {
            width,
            height,
            data: Some(data),
            updated,
        } => cached_conversion(
            cache,
            texture_index,
            TextureConversionSignature {
                kind: 4,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            },
            *width,
            *height,
            width.saturating_mul(*height).saturating_mul(6).min(data.len()),
            data,
            updated,
            bgra_u32_to_rgba,
        ),
        TextureFormat::VecRu8 {
            width,
            height,
            data: Some(data),
            updated,
            ..
        } => cached_conversion(
            cache,
            texture_index,
            TextureConversionSignature {
                kind: 2,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            },
            *width,
            *height,
            width.saturating_mul(*height).min(data.len()),
            data,
            updated,
            |byte: u8| {
                let v = byte as f32 / 255.0;
                [v, v, v, v]
            },
        ),
        TextureFormat::VecRf32 {
            width,
            height,
            data: Some(data),
            updated,
        } => cached_conversion(
            cache,
            texture_index,
            TextureConversionSignature {
                kind: 3,
                width: *width,
                height: *height,
                data_ptr: data.as_ptr() as usize,
                data_len: data.len(),
            },
            *width,
            *height,
            width.saturating_mul(*height).min(data.len()),
            data,
            updated,
            |v: f32| [v, v, v, v],
        ),
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

/// Everything about a draw call that does not vary per row band, so a band
/// worker can be handed one shared reference instead of a dozen arguments.
struct DrawJob<'a> {
    setups: &'a [TriSetup],
    shaded_varyings: &'a [f32],
    varying_slots: usize,
    flat_slots: usize,
    rcx_template: &'a [u8],
    rcx_size: usize,
    rcx_f32s: usize,
    rcx_vary_offset: usize,
    rcx_quad_mode_offset: usize,
    rcx_frag_offset: usize,
    uses_derivatives: bool,
    fragment_fn: FragmentFn,
    debug_text: bool,
    is_draw_text_shader: bool,
}

/// Rasterize the draw call's prepared triangles into one row band.
///
/// The band only visits triangles whose bounding box reaches it — the setup
/// itself was done once, before the split.
fn rasterize_band(
    job: &DrawJob<'_>,
    color_chunk: &mut [[f32; 4]],
    depth_chunk: &mut [f32],
    width: usize,
    state: RasterState,
    row_start: usize,
    row_end: usize,
) {
    let varying_slots = job.varying_slots;
    let mut rcx_buf = job.rcx_template.to_vec();
    let mut dx_varyings = if job.uses_derivatives {
        vec![0.0f32; varying_slots]
    } else {
        Vec::new()
    };
    let mut dy_varyings = if job.uses_derivatives {
        vec![0.0f32; varying_slots]
    } else {
        Vec::new()
    };
    let shift_start = job.flat_slots.min(varying_slots);
    let vary_bytes = varying_slots * std::mem::size_of::<f32>();
    let mut debug_text_prints = 0usize;
    let mut raster_scratch = RasterScratch::default();
    let (rcx_size, rcx_f32s) = (job.rcx_size, job.rcx_f32s);
    let (rcx_vary_offset, rcx_quad_mode_offset, rcx_frag_offset) = (
        job.rcx_vary_offset,
        job.rcx_quad_mode_offset,
        job.rcx_frag_offset,
    );
    let fragment_fn = job.fragment_fn;

    let row_lo = row_start as i32;
    let row_hi = row_end as i32 - 1;

    for tri in job.setups {
        if tri.max_y < row_lo || tri.min_y > row_hi {
            continue;
        }
        if job.uses_derivatives {
            let mut frag_closure = |varyings: &[f32],
                                    derivs: &TriangleDerivatives,
                                    lane_x: u32,
                                    lane_y: u32,
                                    x: i32,
                                    y: i32|
             -> Option<[f32; 4]> {
                // The flat head is identical in all three taps; only the
                // interpolated tail is shifted by a derivative.
                dx_varyings[..shift_start].copy_from_slice(&varyings[..shift_start]);
                dy_varyings[..shift_start].copy_from_slice(&varyings[..shift_start]);
                for i in shift_start..varyings.len() {
                    dx_varyings[i] = varyings[i] + derivs.dvary_dx[i];
                    dy_varyings[i] = varyings[i] + derivs.dvary_dy[i];
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
                    if job.debug_text && job.is_draw_text_shader && debug_text_prints < 120 {
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

            rasterize_setup_rows(
                tri,
                width,
                state,
                row_start,
                row_end,
                color_chunk,
                depth_chunk,
                job.shaded_varyings,
                varying_slots,
                job.flat_slots,
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

            rasterize_setup_rows(
                tri,
                width,
                state,
                row_start,
                row_end,
                color_chunk,
                depth_chunk,
                job.shaded_varyings,
                varying_slots,
                job.flat_slots,
                false,
                &mut raster_scratch,
                &mut frag_closure,
            );
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

    /// Render all dirty passes; returns the window ids whose framebuffer was
    /// repainted. The pixels stay in `os.window_framebuffers`, which the caller
    /// reads — a 2400x1520 window is 73 MB of colour plus depth, and building
    /// that mapping fresh every frame cost more in page faults than clearing it
    /// does.
    pub(crate) fn headless_render_all_passes(&mut self, time: f64) -> Vec<usize> {
        let frame_start = std::time::Instant::now();
        let profile_enabled = std::env::var("MAKEPAD_HEADLESS_PROFILE").is_ok();

        let mut profile = RenderProfile::default();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        let options = RenderOptions::from_env(self.headless_render_thread_count());

        let mut results = Vec::new();
        let mut window_framebuffers = std::mem::take(&mut self.os.window_framebuffers);
        let mut texture_cache = std::mem::take(&mut self.os.texture_conversions);
        // Taken out of `self.os` so a pass can hold `&mut` its own framebuffer
        // while the sampler reads its already-rendered siblings.
        let mut render_targets = std::mem::take(&mut self.os.render_targets);
        render_targets.frame = render_targets.frame.wrapping_add(1);

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

                    let mut fb = window_framebuffers
                        .remove(&window_id.id())
                        .unwrap_or_else(|| Framebuffer::new(width, height));
                    fb.resize(width, height);
                    fb.set_has_depth(true);
                    let clear = self.passes[*draw_pass_id].clear_color;
                    // The window's swapchain image is 8-bit unorm.
                    fb.clear(
                        quantize_color_unorm8([clear.x, clear.y, clear.z, clear.w]),
                        1.0,
                    );

                    self.headless_draw_pass(
                        *draw_pass_id,
                        &options,
                        &mut fb,
                        PassRaster {
                            viewport: (width, height),
                            // The window's swapchain image is 8-bit unorm.
                            unorm8: true,
                            has_depth: true,
                        },
                        &mut texture_cache,
                        &render_targets,
                        if profile_enabled {
                            Some(&mut profile)
                        } else {
                            None
                        },
                    );
                    window_framebuffers.insert(window_id.id(), fb);
                    results.push(window_id.id());
                }
                // Render-to-texture. `CxDrawPassParent::None` is a texture pass
                // too (the GPU backends treat both the same way): it just has no
                // parent that composites it back.
                CxDrawPassParent::DrawPass(_) | CxDrawPassParent::None => {
                    self.headless_draw_pass_to_texture(
                        *draw_pass_id,
                        time,
                        &options,
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

        // Hand the conversions and window buffers back for the next frame.
        self.os.texture_conversions = texture_cache;
        self.os.window_framebuffers = window_framebuffers;
        self.headless_prune_render_targets(&mut render_targets, profile_enabled);
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
    fn headless_prune_render_targets(
        &mut self,
        render_targets: &mut HeadlessRenderTargets,
        profile_enabled: bool,
    ) {
        let pool = &self.textures.0.pool;
        let frame = render_targets.frame;
        let last_used = &render_targets.last_used;
        render_targets.framebuffers.retain(|texture_index, _| {
            let still_a_render_target = pool
                .get(*texture_index)
                .map(|slot| slot.item.format.as_render_alloc(1, 1).is_some())
                .unwrap_or(false);
            let idle = last_used
                .get(texture_index)
                .map(|used| frame.saturating_sub(used.get()))
                .unwrap_or(u64::MAX);
            still_a_render_target && idle < RENDER_TARGET_IDLE_FRAMES
        });
        // Over budget: release least-recently-used targets — but never one
        // touched within the last few frames. "Same frame" alone is not
        // safe: a target can legitimately be READ one or two frames after
        // its last render (the VJ's thumbnail sheet accumulates cells over
        // many paints and is read back a frame after the last one), and an
        // eviction in that window hands the reader nothing. Recent targets
        // may keep the store over budget for a beat; that costs memory
        // briefly, never pixels.
        const RENDER_TARGET_EVICT_GUARD_FRAMES: u64 = 3;
        let budget = render_target_budget_bytes();
        if budget > 0 && render_targets.bytes() > budget {
            let mut by_age: Vec<(u64, usize)> = render_targets
                .framebuffers
                .keys()
                .map(|texture_index| {
                    let used = render_targets
                        .last_used
                        .get(texture_index)
                        .map(|used| used.get())
                        .unwrap_or(0);
                    (used, *texture_index)
                })
                .collect();
            by_age.sort_unstable();
            let mut bytes = render_targets.bytes();
            for (used, texture_index) in by_age {
                if bytes <= budget
                    || used.saturating_add(RENDER_TARGET_EVICT_GUARD_FRAMES) >= frame
                {
                    break;
                }
                if let Some(fb) = render_targets.framebuffers.remove(&texture_index) {
                    bytes = bytes.saturating_sub(fb.bytes());
                }
            }
        }
        let framebuffers = &render_targets.framebuffers;
        render_targets
            .last_used
            .retain(|texture_index, _| framebuffers.contains_key(texture_index));
        if profile_enabled {
            crate::log!(
                "[headless][profile] render targets: {} live, {:.1} MB",
                render_targets.framebuffers.len(),
                render_targets.bytes() as f64 / (1024.0 * 1024.0)
            );
        }
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
        options: &RenderOptions,
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
        let texture_id = color_texture.texture.texture_id();
        // Cube-face targets (`add_color_texture_face`, which is how XR
        // passthrough and cube probes render) are not modelled: one texture
        // needs six attachments and the sampler wants them contiguous, six
        // faces of `width*height*4` floats in +X/-X/+Y/-Y/+Z/-Z order (see
        // `Texture2D::sample_face_from_data`).
        //
        // The shape a fix would take, if it is ever wanted: give the texture ONE
        // framebuffer of `height*6` rows, so face f occupies rows
        // `f*height..(f+1)*height` and the stacked buffer already IS the layout
        // the sampler expects — no assembly step, and `headless_texture_info`
        // reports the per-face height. That needs a y-origin on the viewport
        // (`setup_triangle` currently anchors at row 0) and a row-scoped clear,
        // since each face carries its own load action and clearing the whole
        // buffer would wipe its five neighbours.
        //
        // Until then, say so rather than rendering nothing quietly: a silently
        // skipped pass is how child passes used to come out as a flat colour,
        // and that cost a day to find.
        if color_texture.cube_face.is_some() {
            if render_targets.warned_cube_faces.insert(texture_id.0) {
                crate::error!(
                    "headless: pass renders to cube face {:?} of texture {} — \
                     cube-face render targets are not implemented, so anything \
                     sampling this texture will read whatever was there before",
                    color_texture.cube_face,
                    texture_id.0,
                );
            }
            return;
        }

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
            unorm8: matches!(
                self.textures[texture_id].format,
                TextureFormat::RenderBGRAu8 { .. } | TextureFormat::RenderCubeBGRAu8 { .. }
            ),
            has_depth: self.passes[draw_pass_id].depth_texture.is_some(),
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
            .unwrap_or_else(|| Framebuffer::with_depth(width, height, pass_raster.has_depth));
        let discarded = fb.resize(width, height);
        fb.set_has_depth(pass_raster.has_depth);

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
            let c = [c.x, c.y, c.z, c.w];
            fb.clear_color(if pass_raster.unorm8 {
                quantize_color_unorm8(c)
            } else {
                c
            });
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
            if fb.has_depth() {
                fb.clear_depth(depth);
            }
        }

        self.headless_draw_pass(
            draw_pass_id,
            options,
            &mut fb,
            pass_raster,
            texture_cache,
            render_targets,
            profile,
        );

        render_targets.framebuffers.insert(texture_id.0, fb);
        render_targets.touch(texture_id.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn headless_draw_pass(
        &mut self,
        draw_pass_id: DrawPassId,
        options: &RenderOptions,
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
            options,
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
        options: &RenderOptions,
        fb: &mut Framebuffer,
        pass_raster: PassRaster,
        texture_cache: &mut TextureConversionCache,
        render_targets: &HeadlessRenderTargets,
        mut profile: Option<&mut RenderProfile>,
    ) {
        let draw_order_len = self.draw_lists[draw_list_id].draw_item_order_len();
        // Exploded z-layer view: z is the call's nesting depth, not paint order.
        let sploded = self.passes[draw_pass_id].sploded.is_some();

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
                    options,
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
                    dc.resolve_zbias(current_zbias, sploded);
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
            if let Some(only) = &options.only_shader {
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
                    if let Some(info) = __info {
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
            // Same pipeline state the GPU backends build from the shader: the
            // data-pass colour formats disable blending (their alpha channel is
            // payload — an SDF byte, a depth — and a premultiplied over blend
            // can only ever grow it), and `depth_write: false` shaders must not
            // touch the depth buffer.
            let state = RasterState {
                blend: matches!(color_format, DrawShaderColorFormat::Bgra8Unorm),
                depth_write,
                unorm8: pass_raster.unorm8,
                has_depth: pass_raster.has_depth,
            };
            let viewport = pass_raster.viewport;

            // ── Triangle setup, once for the whole draw call ──
            // Projection, winding fix and bounding box do not depend on which
            // rows a worker owns, so they happen here rather than inside every
            // band. That also yields the draw call's real screen extent, which
            // is what the row split below is sized against — splitting the whole
            // framebuffer meant a draw covering forty rows handed every band but
            // one an empty range.
            let raster_start = std::time::Instant::now();
            let mut setups: Vec<TriSetup> =
                Vec::with_capacity(tri_count.saturating_mul(instance_count));
            let mut covered_px = 0usize;
            let (mut band_lo, mut band_hi) = (usize::MAX, 0usize);
            for inst_idx in 0..instance_count {
                let inst_base = (inst_idx * vertex_count) as u32;
                for tri_idx in 0..tri_count {
                    let i0 = indices[tri_idx * 3];
                    let i1 = indices[tri_idx * 3 + 1];
                    let i2 = indices[tri_idx * 3 + 2];
                    if i0 as usize >= vertex_count
                        || i1 as usize >= vertex_count
                        || i2 as usize >= vertex_count
                    {
                        continue;
                    }
                    let Some(setup) = setup_triangle(
                        fb.width,
                        fb.height,
                        viewport,
                        &shaded_positions,
                        inst_base + i0,
                        inst_base + i1,
                        inst_base + i2,
                    ) else {
                        continue;
                    };
                    band_lo = band_lo.min(setup.min_y.max(0) as usize);
                    band_hi = band_hi.max(setup.max_y.max(0) as usize);
                    // Half the bounding box is a fair estimate of a triangle's
                    // covered pixels, and this only has to be good enough to
                    // decide whether spreading the draw over threads pays.
                    covered_px += ((setup.max_x - setup.min_x + 1) as usize
                        * (setup.max_y - setup.min_y + 1) as usize)
                        / 2;
                    setups.push(setup);
                }
            }

            if setups.is_empty() {
                if let Some(p) = profile.as_deref_mut() {
                    p.raster_ms += raster_start.elapsed().as_secs_f64() * 1000.0;
                }
                continue;
            }
            let band_lo = band_lo.min(fb.height);
            let band_hi = (band_hi + 1).min(fb.height);
            if band_lo >= band_hi {
                if let Some(p) = profile.as_deref_mut() {
                    p.raster_ms += raster_start.elapsed().as_secs_f64() * 1000.0;
                }
                continue;
            }

            let job = DrawJob {
                setups: &setups,
                shaded_varyings: &shaded_varyings,
                varying_slots,
                flat_slots,
                rcx_template: &rcx_template,
                rcx_size,
                rcx_f32s,
                rcx_vary_offset,
                rcx_quad_mode_offset,
                rcx_frag_offset,
                uses_derivatives,
                fragment_fn,
                debug_text: options.debug_text,
                is_draw_text_shader,
            };

            let width = fb.width;
            let bands = compute_row_bands(
                band_lo,
                band_hi,
                options.threads,
                covered_px,
                options.parallel_min_pixels,
            );
            let use_parallel = bands.len() > 1
                && tri_count.saturating_mul(instance_count) >= options.parallel_min_tris;
            if let Some(p) = profile.as_deref_mut() {
                if use_parallel {
                    p.parallel_draw_calls += 1;
                } else {
                    p.serial_draw_calls += 1;
                }
            }

            if use_parallel {
                // Bands are disjoint row ranges, so the colour and depth buffers
                // split into non-overlapping `&mut` slices and the workers need
                // no shared mutable state at all — no raw pointers, no aliasing
                // to argue about. Workers pull bands off a shared queue instead
                // of owning a fixed share, because a draw call's cost is spread
                // very unevenly over the rows it touches.
                let threads = options.threads.min(bands.len());
                let queue = std::sync::Mutex::new(split_bands(fb, band_lo, &bands));
                std::thread::scope(|scope| {
                    for _ in 0..threads {
                        scope.spawn(|| loop {
                            let Some(band) = queue.lock().ok().and_then(|mut q| q.pop()) else {
                                break;
                            };
                            let RowBand {
                                row_start,
                                row_end,
                                color,
                                depth,
                            } = band;
                            rasterize_band(&job, color, depth, width, state, row_start, row_end);
                        });
                    }
                });
            } else {
                let color = &mut fb.color[band_lo * width..band_hi * width];
                let depth = if fb.depth.is_empty() {
                    &mut [][..]
                } else {
                    &mut fb.depth[band_lo * width..band_hi * width]
                };
                rasterize_band(&job, color, depth, width, state, band_lo, band_hi);
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
