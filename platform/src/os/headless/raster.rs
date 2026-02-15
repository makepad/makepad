use super::virtual_gpu::{rasterize_triangle, Framebuffer, ShadedVertex, TriangleDerivatives};
use crate::{
    cx::Cx,
    draw_list::{CxDrawKind, DrawListId},
    draw_pass::{CxDrawPassParent, DrawPassId},
    makepad_live_id::*,
    makepad_math::*,
    texture::TextureFormat,
};
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};

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
// Public API
// ─────────────────────────────────────────────────────────────────────────────

impl Cx {
    /// Render all dirty passes and return framebuffers keyed by window_id.
    pub(crate) fn headless_render_all_passes(&mut self, time: f64) -> Vec<(usize, Framebuffer)> {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);

        let mut results = Vec::new();

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
                    self.passes[*draw_pass_id].set_ortho_matrix(dvec2(0.0, 0.0), size);
                    self.passes[*draw_pass_id].set_dpi_factor(dpi_factor);
                    self.passes[*draw_pass_id].set_time(time as f32);

                    let mut fb = Framebuffer::new(width, height);
                    let clear = self.passes[*draw_pass_id].clear_color;
                    fb.clear([clear.x, clear.y, clear.z, clear.w], 1.0);

                    self.headless_draw_pass(*draw_pass_id, &mut fb);
                    results.push((window_id.id(), fb));
                }
                CxDrawPassParent::DrawPass(_dep_pass_id) => {
                    // TODO: render-to-texture passes
                }
                _ => {}
            }
        }

        results
    }

    fn headless_draw_pass(&mut self, draw_pass_id: DrawPassId, fb: &mut Framebuffer) {
        let draw_list_id = match self.passes[draw_pass_id].main_draw_list_id {
            Some(id) => id,
            None => return,
        };

        let zbias_step = self.passes[draw_pass_id].zbias_step;
        let mut zbias = 0.0f32;

        self.headless_render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step, fb);
    }

    fn headless_render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        fb: &mut Framebuffer,
    ) {
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();

        for draw_item_id in 0..draw_items_len {
            let kind_tag = match &self.draw_lists[draw_list_id].draw_items[draw_item_id].kind {
                CxDrawKind::SubList(sub_id) => Some(*sub_id),
                CxDrawKind::DrawCall(_) => None,
                CxDrawKind::Empty => continue,
            };

            if let Some(sub_list_id) = kind_tag {
                self.headless_render_view(draw_pass_id, sub_list_id, zbias, zbias_step, fb);
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
            let sh = &self.draw_shaders.shaders[shader_id.index];
            let os_shader_id = match sh.os_shader_id {
                Some(id) => id,
                None => continue,
            };
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
            let rcx_dfdx_offset = os_shader.rcx_dfdx_offset;
            let rcx_dfdy_offset = os_shader.rcx_dfdy_offset;
            let rcx_frag_offset = os_shader.rcx_frag_offset;

            if rcx_size == 0 {
                continue;
            }

            // Allocate RenderCx buffer (reused across all instances/triangles/pixels)
            let rcx_f32s = rcx_size / std::mem::size_of::<f32>();
            let mut rcx_buf = vec![0u8; rcx_size];

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

            // ── Convert texture data to RGBA f32, store pointers ──
            // tex_rgba_bufs must live through rendering so the pointers in rcx_buf stay valid.
            let mut tex_rgba_bufs: Vec<Vec<f32>> = Vec::new();

            // Collect texture (data_ptr, data_len, width, height) for each texture slot
            struct TexInfo {
                data_ptr: usize, // *const f32 as usize
                data_len: usize,
                width: usize,
                height: usize,
            }
            let mut tex_infos: Vec<TexInfo> = Vec::new();

            for tex_idx in 0..sh.mapping.textures.len() {
                if let Some(texture) = &draw_call.texture_slots[tex_idx] {
                    let cxtexture = &self.textures[texture.texture_id()];
                    match &cxtexture.format {
                        TextureFormat::VecRGBAf32 {
                            width,
                            height,
                            data: Some(data),
                            ..
                        } => {
                            tex_infos.push(TexInfo {
                                data_ptr: data.as_ptr() as usize,
                                data_len: data.len(),
                                width: *width,
                                height: *height,
                            });
                        }
                        TextureFormat::VecBGRAu8_32 {
                            width,
                            height,
                            data: Some(data),
                            ..
                        } => {
                            let mut rgba = Vec::with_capacity(data.len() * 4);
                            for &pixel in data.iter() {
                                let b = (pixel & 0xFF) as f32 / 255.0;
                                let g = ((pixel >> 8) & 0xFF) as f32 / 255.0;
                                let r = ((pixel >> 16) & 0xFF) as f32 / 255.0;
                                let a = ((pixel >> 24) & 0xFF) as f32 / 255.0;
                                rgba.push(r);
                                rgba.push(g);
                                rgba.push(b);
                                rgba.push(a);
                            }
                            tex_infos.push(TexInfo {
                                data_ptr: rgba.as_ptr() as usize,
                                data_len: rgba.len(),
                                width: *width,
                                height: *height,
                            });
                            tex_rgba_bufs.push(rgba);
                        }
                        TextureFormat::VecRu8 {
                            width,
                            height,
                            data: Some(data),
                            ..
                        } => {
                            let mut rgba = Vec::with_capacity(width * height * 4);
                            for &byte in data.iter().take(width * height) {
                                let v = byte as f32 / 255.0;
                                rgba.push(v);
                                rgba.push(v);
                                rgba.push(v);
                                rgba.push(v);
                            }
                            tex_infos.push(TexInfo {
                                data_ptr: rgba.as_ptr() as usize,
                                data_len: rgba.len(),
                                width: *width,
                                height: *height,
                            });
                            tex_rgba_bufs.push(rgba);
                        }
                        TextureFormat::VecRf32 {
                            width,
                            height,
                            data: Some(data),
                            ..
                        } => {
                            let mut rgba = Vec::with_capacity(width * height * 4);
                            for &v in data.iter().take(width * height) {
                                rgba.push(v);
                                rgba.push(v);
                                rgba.push(v);
                                rgba.push(v);
                            }
                            tex_infos.push(TexInfo {
                                data_ptr: rgba.as_ptr() as usize,
                                data_len: rgba.len(),
                                width: *width,
                                height: *height,
                            });
                            tex_rgba_bufs.push(rgba);
                        }
                        _ => {
                            tex_infos.push(TexInfo {
                                data_ptr: 0,
                                data_len: 0,
                                width: 0,
                                height: 0,
                            });
                        }
                    }
                } else {
                    tex_infos.push(TexInfo {
                        data_ptr: 0,
                        data_len: 0,
                        width: 0,
                        height: 0,
                    });
                }
            }

            // ── Fill RenderCx buffer: uniforms + textures (per-draw-call, cold path) ──
            // Use the exported fill_rcx_uniforms entry if available
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
                // Pack tex_infos into [data_ptr, data_len, width, height] tuples
                let tex_tuples: Vec<[usize; 4]> = tex_infos
                    .iter()
                    .map(|t| [t.data_ptr, t.data_len, t.width, t.height])
                    .collect();
                unsafe {
                    fill_fn(
                        rcx_buf.as_mut_ptr() as *mut f32,
                        rcx_f32s as u32,
                        uniform_ptrs,
                        uniform_lens,
                        uniform_count,
                        tex_tuples.as_ptr(),
                        tex_tuples.len() as u32,
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

            let geom_slots = sh.mapping.geometries.total_slots;
            let varying_slots = sh.mapping.varying_total_slots;

            let vertex_count = if geom_slots > 0 {
                vertices.len() / geom_slots
            } else {
                0
            };

            for inst_idx in 0..instance_count {
                let inst_offset = inst_idx * total_instance_slots;
                let inst_slice = &instances_data[inst_offset..inst_offset + total_instance_slots];

                let mut shaded_vertices = Vec::with_capacity(vertex_count);

                for vert_idx in 0..vertex_count {
                    let geom_offset = vert_idx * geom_slots;
                    let geom_slice = &vertices[geom_offset..geom_offset + geom_slots];

                    let mut out_pos = [0.0f32; 4];
                    let mut varying_out = vec![0.0f32; varying_slots];

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
                            varying_out.len() as u32,
                            &mut out_pos,
                        );
                    }

                    shaded_vertices.push(ShadedVertex {
                        pos: out_pos,
                        varyings: varying_out,
                    });
                }

                // Rasterize triangles
                let tri_count = indices.len() / 3;
                for tri_idx in 0..tri_count {
                    let i0 = indices[tri_idx * 3] as usize;
                    let i1 = indices[tri_idx * 3 + 1] as usize;
                    let i2 = indices[tri_idx * 3 + 2] as usize;

                    if i0 >= shaded_vertices.len()
                        || i1 >= shaded_vertices.len()
                        || i2 >= shaded_vertices.len()
                    {
                        continue;
                    }

                    let v0 = &shaded_vertices[i0];
                    let v1 = &shaded_vertices[i1];
                    let v2 = &shaded_vertices[i2];

                    // Fragment closure: writes varyings + derivatives into rcx_buf,
                    // calls fragment, reads back color. Zero-copy for the call itself.
                    let mut frag_closure = |varyings: &[f32],
                                            derivs: &TriangleDerivatives|
                     -> Option<[f32; 4]> {
                        // Write interpolated varyings at the start of rcx_buf (group 1, offset = rcx_vary_offset)
                        let vary_bytes = varyings.len() * std::mem::size_of::<f32>();
                        if rcx_vary_offset + vary_bytes <= rcx_size {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    varyings.as_ptr() as *const u8,
                                    rcx_buf.as_mut_ptr().add(rcx_vary_offset),
                                    vary_bytes,
                                );
                            }
                        }

                        // Write derivatives (group 2)
                        let deriv_bytes = derivs.dvary_dx.len() * std::mem::size_of::<f32>();
                        if rcx_dfdx_offset + deriv_bytes <= rcx_size {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    derivs.dvary_dx.as_ptr() as *const u8,
                                    rcx_buf.as_mut_ptr().add(rcx_dfdx_offset),
                                    deriv_bytes,
                                );
                            }
                        }
                        if rcx_dfdy_offset + deriv_bytes <= rcx_size {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    derivs.dvary_dy.as_ptr() as *const u8,
                                    rcx_buf.as_mut_ptr().add(rcx_dfdy_offset),
                                    deriv_bytes,
                                );
                            }
                        }

                        // Call fragment shader
                        let write_pixel = unsafe {
                            fragment_fn(rcx_buf.as_mut_ptr() as *mut f32, rcx_f32s as u32)
                        };

                        if write_pixel == 0 {
                            return None; // discard
                        }

                        // Read frag_fb0 (Vec4f = 4 f32s at rcx_frag_offset)
                        if rcx_frag_offset + 16 <= rcx_size {
                            let color_ptr =
                                unsafe { rcx_buf.as_ptr().add(rcx_frag_offset) as *const [f32; 4] };
                            Some(unsafe { *color_ptr })
                        } else {
                            Some([0.0, 0.0, 0.0, 0.0])
                        }
                    };

                    rasterize_triangle(fb, v0, v1, v2, &mut frag_closure);
                }
            }

            // tex_rgba_bufs dropped here — pointers in rcx_buf are no longer valid
            // but that's fine since we're done rendering this draw call
            let _ = &tex_rgba_bufs;
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
            "invalid rgba buffer size for png encode: got {}, expected {}",
            rgba.len(),
            expected
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
