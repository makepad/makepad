use {
    crate::{
        cx::Cx,
        draw_list::DrawListId,
        draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
        draw_shader::{CxDrawShader, CxDrawShaderCode, CxDrawShaderMapping, DrawShaderId},
        draw_vars::DrawVars,
        geometry::Geometry,
        makepad_objc_sys::objc_block,
        makepad_script::shader::*,
        makepad_script::shader_backend::*,
        makepad_script::*,
        os::{
            apple::apple_sys::*,
            apple::apple_util::{nsstring_to_string, str_to_nsstring},
            shared_framebuf::PresentableDraw,
        },
        script::vm::*,
        studio::{AppToStudio, GPUSample},
        texture::{CxTexture, Texture, TextureAlloc, TextureFormat, TexturePixel},
    },
    makepad_objc_sys::{class, msg_send, sel, sel_impl},
    makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    },
    std::collections::HashMap,
    std::fmt::Write,
    std::sync::atomic::{AtomicUsize, Ordering},
    std::sync::Mutex,
    std::time::Instant,
};

#[derive(Clone, Copy, Debug)]
struct MetalGpuTimelineSync {
    host_to_app_offset: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct GpuSampleCounters {
    draw_calls: u64,
    instances: u64,
    vertices: u64,
    instance_bytes: u64,
    uniform_bytes: u64,
    vertex_buffer_bytes: u64,
    texture_bytes: u64,
}

impl GpuSampleCounters {
    fn accumulate(&mut self, other: Self) {
        self.draw_calls = self.draw_calls.saturating_add(other.draw_calls);
        self.instances = self.instances.saturating_add(other.instances);
        self.vertices = self.vertices.saturating_add(other.vertices);
        self.instance_bytes = self.instance_bytes.saturating_add(other.instance_bytes);
        self.uniform_bytes = self.uniform_bytes.saturating_add(other.uniform_bytes);
        self.vertex_buffer_bytes = self
            .vertex_buffer_bytes
            .saturating_add(other.vertex_buffer_bytes);
        self.texture_bytes = self.texture_bytes.saturating_add(other.texture_bytes);
    }
}

static METAL_GPU_TIMELINE_SYNC: Mutex<Option<MetalGpuTimelineSync>> = Mutex::new(None);
static METAL_GPU_FRAME_RANGES: Mutex<Option<HashMap<u64, (f64, f64)>>> = Mutex::new(None);
static METAL_GPU_FRAME_COUNTERS: Mutex<Option<HashMap<u64, GpuSampleCounters>>> = Mutex::new(None);

fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "metal screenshot size overflow".to_string())?;
    if rgba.len() != expected {
        return Err(format!(
            "metal screenshot expected {} RGBA bytes, got {}",
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
        .map_err(|err| format!("metal screenshot png encode failed: {err:?}"))?;
    Ok(out)
}

fn map_metal_gpu_times_to_app_timeline(
    raw_start: f64,
    raw_end: f64,
    app_now: f64,
    host_now: f64,
) -> Option<(f64, f64)> {
    if !(raw_start.is_finite()
        && raw_end.is_finite()
        && app_now.is_finite()
        && host_now.is_finite())
    {
        return None;
    }
    if raw_start <= 0.0 || raw_end < raw_start {
        return None;
    }

    // Apple documents GPUStartTime/GPUEndTime as host-time seconds. Calibrate that
    // host clock to our app-relative timeline once, then apply the same offset.
    let measured_offset = app_now - host_now;
    let mut sync = METAL_GPU_TIMELINE_SYNC.lock().ok()?;
    let state = sync.get_or_insert(MetalGpuTimelineSync {
        host_to_app_offset: measured_offset,
    });
    if (state.host_to_app_offset - measured_offset).abs() > 0.1 {
        state.host_to_app_offset = measured_offset;
    }

    Some((
        raw_start + state.host_to_app_offset,
        raw_end + state.host_to_app_offset,
    ))
}

// IOSurface-based texture sharing (replaces XPC service approach)
// Uses global IOSurface IDs which work across processes without needing Mach port transfer
#[cfg(target_os = "macos")]
use crate::os::apple::apple_sys::{
    CFRelease, IOSurfaceCreate, IOSurfaceGetID, IOSurfaceID, IOSurfaceLookup, IOSurfaceRef,
};

impl Cx {
    fn total_drawcall_log_enabled() -> bool {
        std::env::var_os("MAKEPAD_TOTAL_DRAWCALLS_DEBUG").is_some()
    }

    fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        encoder: ObjcId,
        metal_cx: &MetalCx,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_order_len = self.draw_lists[draw_list_id].draw_item_order_len();
        let debug_dump_count = self.draw_lists[draw_list_id].debug_dump_count;
        let debug_dump = debug_dump_count > 0;
        if self.draw_lists[draw_list_id].debug_dump {
            self.draw_lists[draw_list_id].debug_dump = false;
            self.draw_lists[draw_list_id].debug_dump_count = 6; // dump 6 consecutive frames
        }
        if debug_dump {
            println!(
                "=== DEBUG DUMP draw_list {:?} ({} items) repaint_id={} frames_left={} ===",
                draw_list_id.index(),
                draw_order_len,
                self.repaint_id,
                debug_dump_count,
            );
            self.draw_lists[draw_list_id].debug_dump_count -= 1;
        }

        for order_index in 0..draw_order_len {
            let Some(draw_item_id) =
                self.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id]
                .kind
                .sub_list()
            {
                self.render_view(
                    draw_pass_id,
                    sub_list_id,
                    zbias,
                    zbias_step,
                    encoder,
                    metal_cx,
                );
            } else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                } else {
                    continue;
                };

                let sh = &self.draw_shaders[draw_call.draw_shader_id.index];
                if sh.os_shader_id.is_none() {
                    // shader didnt compile somehow
                    continue;
                }
                let shp = &self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];

                if sh.mapping.uses_time {
                    self.demo_time_repaint = true;
                }

                if debug_dump {
                    println!(
                        "  [item {}] instance_dirty={} instances_len={}",
                        draw_item_id,
                        draw_call.instance_dirty,
                        draw_item.instances.as_ref().map(|i| i.len()).unwrap_or(0),
                    );
                }

                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    let instance_bytes = (draw_item.instances.as_ref().unwrap().len()
                        * std::mem::size_of::<f32>())
                        as u64;
                    self.os.bytes_written = self
                        .os
                        .bytes_written
                        .saturating_add(instance_bytes as usize);
                    self.os.instance_bytes_uploaded = self
                        .os
                        .instance_bytes_uploaded
                        .saturating_add(instance_bytes);
                    draw_item
                        .os
                        .instance_buffer
                        .update(metal_cx, &draw_item.instances.as_ref().unwrap());
                }

                // update the zbias uniform if we have it.
                draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;

                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }

                // lets verify our instance_offset is not disaligned
                let instances = (draw_item.instances.as_ref().unwrap().len()
                    / sh.mapping.instances.total_slots) as u64;

                if instances == 0 {
                    continue;
                }

                if self.passes[draw_pass_id].depth_texture.is_some() {
                    let depth_state = if draw_call.options.depth_write {
                        self.passes[draw_pass_id].os.mtl_depth_state_write
                    } else {
                        self.passes[draw_pass_id].os.mtl_depth_state_no_write
                    };
                    if let Some(depth_state) = depth_state {
                        let () = unsafe { msg_send![encoder, setDepthStencilState: depth_state] };
                    }
                }

                let render_pipeline_state = shp.render_pipeline_state.as_id();
                unsafe {
                    let () = msg_send![encoder, setRenderPipelineState: render_pipeline_state];
                }

                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {
                    geometry_id
                } else {
                    continue;
                };

                let geometry = &mut self.geometries[geometry_id];

                if geometry.dirty_vertices || geometry.os.vertex_buffer.inner.is_none() {
                    let bytes = (geometry.vertices.len() * std::mem::size_of::<f32>()) as u64;
                    self.os.vertex_buffer_bytes_uploaded =
                        self.os.vertex_buffer_bytes_uploaded.saturating_add(bytes);
                    geometry
                        .os
                        .vertex_buffer
                        .update(metal_cx, &geometry.vertices);
                    geometry.dirty_vertices = false;
                }
                if geometry.dirty_indices || geometry.os.index_buffer.inner.is_none() {
                    let bytes = (geometry.indices.len() * std::mem::size_of::<u32>()) as u64;
                    self.os.vertex_buffer_bytes_uploaded =
                        self.os.vertex_buffer_bytes_uploaded.saturating_add(bytes);
                    geometry.os.index_buffer.update(metal_cx, &geometry.indices);
                    geometry.dirty_indices = false;
                }
                geometry.dirty = geometry.dirty_vertices || geometry.dirty_indices;

                if debug_dump {
                    Self::debug_dump_draw_call(
                        draw_item_id,
                        sh,
                        draw_item.instances.as_ref().unwrap(),
                        draw_call,
                        instances,
                    );
                }

                if let Some(inner) = geometry.os.vertex_buffer.inner.as_ref() {
                    unsafe {
                        msg_send![
                            encoder,
                            setVertexBuffer: inner.buffer.as_id()
                            offset: 0
                            atIndex: 0
                        ]
                    }
                } else {
                    crate::error!("Drawing error: vertex_buffer None")
                }

                if let Some(inner) = draw_item.os.instance_buffer.inner.as_ref() {
                    unsafe {
                        msg_send![
                            encoder,
                            setVertexBuffer: inner.buffer.as_id()
                            offset: 0
                            atIndex: 1
                        ]
                    }
                    // Also bind instance buffer to fragment shader so it can access instance data
                    unsafe {
                        msg_send![
                            encoder,
                            setFragmentBuffer: inner.buffer.as_id()
                            offset: 0
                            atIndex: 1
                        ]
                    }
                } else {
                    crate::error!("Drawing error: instance_buffer None")
                }

                let pass_uniforms = self.passes[draw_pass_id].pass_uniforms.as_slice();
                let draw_list_uniforms = draw_list.draw_list_uniforms.as_slice();
                let draw_call_uniforms = draw_call.draw_call_uniforms.as_slice();
                let mut uniform_bytes_uploaded = 0u64;

                unsafe {
                    //let () = msg_send![encoder, setVertexBytes: sh.mapping.live_uniforms_buf.as_ptr() as *const //std::ffi::c_void length: (sh.mapping.live_uniforms_buf.len() * 4) as u64 atIndex: 2u64];

                    //let () = msg_send![encoder, setFragmentBytes: sh.mapping.live_uniforms_buf.as_ptr() as *const std::ffi::c_void length: (sh.mapping.live_uniforms_buf.len() * 4) as u64 atIndex: 2u64];

                    if let Some(id) = shp.draw_call_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_call_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_call_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call_uniforms.len() * 4) as u64 atIndex: id];
                        uniform_bytes_uploaded = uniform_bytes_uploaded
                            .saturating_add((draw_call_uniforms.len() * 4 * 2) as u64);
                    }
                    if let Some(id) = shp.pass_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: id];
                        uniform_bytes_uploaded = uniform_bytes_uploaded
                            .saturating_add((pass_uniforms.len() * 4 * 2) as u64);
                    }
                    if let Some(id) = shp.draw_list_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_list_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_list_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_list_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_list_uniforms.len() * 4) as u64 atIndex: id];
                        uniform_bytes_uploaded = uniform_bytes_uploaded
                            .saturating_add((draw_list_uniforms.len() * 4 * 2) as u64);
                    }
                    if let Some(id) = shp.dyn_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_call.dyn_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.dyn_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_call.dyn_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.dyn_uniforms.len() * 4) as u64 atIndex: id];
                        uniform_bytes_uploaded = uniform_bytes_uploaded
                            .saturating_add((draw_call.dyn_uniforms.len() * 4 * 2) as u64);
                    }
                    if let Some(id) = shp.scope_uniform_buffer_id {
                        let scope_buf = &sh.mapping.scope_uniforms_buf;
                        if !scope_buf.is_empty() {
                            let () = msg_send![encoder, setVertexBytes: scope_buf.as_ptr() as *const std::ffi::c_void length: (scope_buf.len() * 4) as u64 atIndex: id];
                            let () = msg_send![encoder, setFragmentBytes: scope_buf.as_ptr() as *const std::ffi::c_void length: (scope_buf.len() * 4) as u64 atIndex: id];
                            uniform_bytes_uploaded = uniform_bytes_uploaded
                                .saturating_add((scope_buf.len() * 4 * 2) as u64);
                        }
                    }
                    /*
                    let ct = &sh.mapping.const_table.table;
                    if ct.len()>0 {
                        let () = msg_send![encoder, setVertexBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 3u64];
                        let () = msg_send![encoder, setFragmentBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 3u64];
                    }*/
                }
                self.os.uniform_bytes_uploaded = self
                    .os
                    .uniform_bytes_uploaded
                    .saturating_add(uniform_bytes_uploaded);
                // lets set our textures
                for i in 0..sh.mapping.textures.len() {
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    } else {
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setFragmentTexture: nil
                                atIndex: i as u64
                            ]
                        };
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setVertexTexture: nil
                                atIndex: i as u64
                            ]
                        };
                        continue;
                    };

                    let cxtexture = &mut self.textures[texture_id];

                    if cxtexture.format.is_shared() {
                        #[cfg(target_os = "macos")]
                        cxtexture.update_shared_texture(metal_cx.device);
                    } else if cxtexture.format.is_vec() {
                        let texture_bytes = cxtexture.update_vec_texture(metal_cx);
                        self.os.texture_bytes_uploaded =
                            self.os.texture_bytes_uploaded.saturating_add(texture_bytes);
                    }

                    if let Some(texture) = cxtexture.os.texture.as_ref() {
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setFragmentTexture: texture.as_id()
                                atIndex: i as u64
                            ]
                        };
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setVertexTexture: texture.as_id()
                                atIndex: i as u64
                            ]
                        };
                    }
                }

                // Debug output when shader has debug_draw flag enabled
                if sh.mapping.flags.debug_draw {
                    CxDrawShaderMapping::debug_dump_shader_draw_call(
                        "metal",
                        draw_item_id,
                        sh,
                        draw_call,
                        draw_item.instances.as_ref().unwrap(),
                        instances as usize,
                    );
                }

                self.os.draw_calls_done += 1;
                self.os.instances_done = self.os.instances_done.saturating_add(instances);
                self.os.vertices_done = self
                    .os
                    .vertices_done
                    .saturating_add((geometry.indices.len() as u64).saturating_mul(instances));
                if let Some(inner) = geometry.os.index_buffer.inner.as_ref() {
                    let () = unsafe {
                        msg_send![
                            encoder,
                            drawIndexedPrimitives: MTLPrimitiveType::Triangle
                            indexCount: geometry.indices.len() as u64
                            indexType: MTLIndexType::UInt32
                            indexBuffer: inner.buffer.as_id()
                            indexBufferOffset: 0
                            instanceCount: instances
                        ]
                    };
                } else {
                    crate::error!("Drawing error: index_buffer None")
                }
            }
        }
    }

    /// Debug helper for printing draw call info. Called from draw-list debug dumps.
    fn debug_dump_draw_call(
        draw_item_id: usize,
        sh: &CxDrawShader,
        instance_data: &[f32],
        draw_call: &crate::draw_list::CxDrawCall,
        instances: u64,
    ) {
        let total_slots = sh.mapping.instances.total_slots;
        println!(
            "-- call {} shader:{:?} instances:{} --",
            draw_item_id, sh.debug_id, instances
        );

        // Named dyn_uniforms
        for input in &sh.mapping.dyn_uniforms.inputs {
            let end = (input.offset + input.slots).min(draw_call.dyn_uniforms.len());
            println!(
                "  u {:?}: {:?}",
                input.id,
                &draw_call.dyn_uniforms[input.offset..end]
            );
        }

        // All instances with named values
        for inst_idx in 0..instances as usize {
            let base = inst_idx * total_slots;
            if base + total_slots <= instance_data.len() {
                let mut parts = Vec::new();
                for input in &sh.mapping.instances.inputs {
                    let start = base + input.offset;
                    let end = start + input.slots;
                    if end <= instance_data.len() {
                        let vals = &instance_data[start..end];
                        if input.slots == 1 {
                            parts.push(format!("{:?}={}", input.id, vals[0]));
                        } else {
                            parts.push(format!("{:?}={:?}", input.id, vals));
                        }
                    }
                }
                println!("  i[{}] {}", inst_idx, parts.join(" "));
            }
        }
    }

    pub fn draw_pass(
        &mut self,
        draw_pass_id: DrawPassId,
        metal_cx: &mut MetalCx,
        mode: DrawPassMode,
    ) {
        self.os.bytes_written = 0;
        self.os.draw_calls_done = 0;
        self.os.instances_done = 0;
        self.os.vertices_done = 0;
        self.os.instance_bytes_uploaded = 0;
        self.os.uniform_bytes_uploaded = 0;
        self.os.vertex_buffer_bytes_uploaded = 0;
        self.os.texture_bytes_uploaded = 0;
        let draw_list_id = if let Some(draw_list_id) = self.passes[draw_pass_id].main_draw_list_id {
            draw_list_id
        } else {
            crate::error!("Draw pass has no draw list!");
            return;
        };

        let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };

        let render_pass_descriptor: ObjcId = if let DrawPassMode::MTKView(view) = mode {
            unsafe { msg_send![view, currentRenderPassDescriptor] }
        } else {
            unsafe {
                msg_send![
                    class!(MTLRenderPassDescriptorInternal),
                    renderPassDescriptor
                ]
            }
        };

        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();

        let pass_rect = self
            .get_pass_rect(
                draw_pass_id,
                if mode.is_drawable().is_some() {
                    1.0
                } else {
                    dpi_factor
                },
            )
            .unwrap();

        self.passes[draw_pass_id].set_ortho_matrix(pass_rect.pos, pass_rect.size);

        self.passes[draw_pass_id].paint_dirty = false;

        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            return;
        }

        self.passes[draw_pass_id].set_dpi_factor(dpi_factor);

        if let DrawPassMode::MTKView(_) = mode {
            let color_attachments: ObjcId =
                unsafe { msg_send![render_pass_descriptor, colorAttachments] };
            let color_attachment: ObjcId =
                unsafe { msg_send![color_attachments, objectAtIndexedSubscript: 0] };
            let color = self.passes[draw_pass_id].clear_color;
            unsafe {
                let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                    red: color.x as f64,
                    green: color.y as f64,
                    blue: color.z as f64,
                    alpha: color.w as f64
                }];
            }
        } else if let Some(drawable) = mode.is_drawable() {
            let first_texture: ObjcId = unsafe { msg_send![drawable, texture] };
            let color_attachments: ObjcId =
                unsafe { msg_send![render_pass_descriptor, colorAttachments] };
            let color_attachment: ObjcId =
                unsafe { msg_send![color_attachments, objectAtIndexedSubscript: 0] };

            let () = unsafe {
                msg_send![
                    color_attachment,
                    setTexture: first_texture
                ]
            };
            let color = self.passes[draw_pass_id].clear_color;
            unsafe {
                let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                    red: color.x as f64,
                    green: color.y as f64,
                    blue: color.z as f64,
                    alpha: color.w as f64
                }];
            }
        } else {
            for (index, color_texture) in
                self.passes[draw_pass_id].color_textures.iter().enumerate()
            {
                let color_attachments: ObjcId =
                    unsafe { msg_send![render_pass_descriptor, colorAttachments] };
                let color_attachment: ObjcId =
                    unsafe { msg_send![color_attachments, objectAtIndexedSubscript: index as u64] };

                let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                let size = dpi_factor * pass_rect.size;
                cxtexture.update_render_target(metal_cx, size.x as usize, size.y as usize);

                let is_initial = cxtexture.take_initial();

                if let Some(texture) = cxtexture.os.texture.as_ref() {
                    let () = unsafe {
                        msg_send![
                            color_attachment,
                            setTexture: texture.as_id()
                        ]
                    };
                } else {
                    crate::error!("draw_pass_to_texture invalid render target");
                }

                unsafe { msg_send![color_attachment, setStoreAction: MTLStoreAction::Store] }
                match color_texture.clear_color {
                    DrawPassClearColor::InitWith(color) => {
                        if is_initial {
                            unsafe {
                                let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                                let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                                    red: color.x as f64,
                                    green: color.y as f64,
                                    blue: color.z as f64,
                                    alpha: color.w as f64
                                }];
                            }
                        } else {
                            unsafe {
                                let () =
                                    msg_send![color_attachment, setLoadAction: MTLLoadAction::Load];
                            }
                        }
                    }
                    DrawPassClearColor::ClearWith(color) => unsafe {
                        let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                        let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                            red: color.x as f64,
                            green: color.y as f64,
                            blue: color.z as f64,
                            alpha: color.w as f64
                        }];
                    },
                }
            }
        }
        // attach depth texture
        if let Some(depth_texture) = &self.passes[draw_pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture.texture_id()];
            let size = dpi_factor * pass_rect.size;
            cxtexture.update_depth_stencil(metal_cx, size.x as usize, size.y as usize);
            let is_initial = cxtexture.take_initial();

            let depth_attachment: ObjcId =
                unsafe { msg_send![render_pass_descriptor, depthAttachment] };

            if let Some(texture) = cxtexture.os.texture.as_ref() {
                unsafe { msg_send![depth_attachment, setTexture: texture.as_id()] }
            } else {
                crate::error!("draw_pass_to_texture invalid render target");
            }
            let () = unsafe { msg_send![depth_attachment, setStoreAction: MTLStoreAction::Store] };

            match self.passes[draw_pass_id].clear_depth {
                DrawPassClearDepth::InitWith(depth) => {
                    if is_initial {
                        let () = unsafe {
                            msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]
                        };
                        let () =
                            unsafe { msg_send![depth_attachment, setClearDepth: depth as f64] };
                    } else {
                        let () = unsafe {
                            msg_send![depth_attachment, setLoadAction: MTLLoadAction::Load]
                        };
                    }
                }
                DrawPassClearDepth::ClearWith(depth) => {
                    let () =
                        unsafe { msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear] };
                    let () = unsafe { msg_send![depth_attachment, setClearDepth: depth as f64] };
                }
            }
            // create depth state
            if self.passes[draw_pass_id].os.mtl_depth_state_write.is_none() {
                let desc: ObjcId = unsafe { msg_send![class!(MTLDepthStencilDescriptor), new] };
                let () = unsafe {
                    msg_send![desc, setDepthCompareFunction: MTLCompareFunction::LessEqual]
                };
                let () = unsafe { msg_send![desc, setDepthWriteEnabled: true] };
                let depth_stencil_state: ObjcId =
                    unsafe { msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc] };
                self.passes[draw_pass_id].os.mtl_depth_state_write = Some(depth_stencil_state);
            }
            if self.passes[draw_pass_id]
                .os
                .mtl_depth_state_no_write
                .is_none()
            {
                let desc: ObjcId = unsafe { msg_send![class!(MTLDepthStencilDescriptor), new] };
                let () = unsafe {
                    msg_send![desc, setDepthCompareFunction: MTLCompareFunction::LessEqual]
                };
                let () = unsafe { msg_send![desc, setDepthWriteEnabled: false] };
                let depth_stencil_state: ObjcId =
                    unsafe { msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc] };
                self.passes[draw_pass_id].os.mtl_depth_state_no_write = Some(depth_stencil_state);
            }
        }

        let command_buffer: ObjcId = unsafe { msg_send![metal_cx.command_queue, commandBuffer] };
        let encoder: ObjcId = unsafe {
            msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]
        };

        if let Some(depth_state) = self.passes[draw_pass_id].os.mtl_depth_state_write {
            let () = unsafe { msg_send![encoder, setDepthStencilState: depth_state] };
        }

        let pass_width = dpi_factor * pass_rect.size.x;
        let pass_height = dpi_factor * pass_rect.size.y;

        let () = unsafe {
            msg_send![encoder, setViewport: MTLViewport {
                originX: 0.0,
                originY: 0.0,
                width: pass_width,
                height: pass_height,
                znear: 0.0,
                zfar: 1.0,
            }]
        };

        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;

        self.render_view(
            draw_pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            encoder,
            &metal_cx,
        );
        let gpu_counters = GpuSampleCounters {
            draw_calls: self.os.draw_calls_done as u64,
            instances: self.os.instances_done,
            vertices: self.os.vertices_done,
            instance_bytes: self.os.instance_bytes_uploaded,
            uniform_bytes: self.os.uniform_bytes_uploaded,
            vertex_buffer_bytes: self.os.vertex_buffer_bytes_uploaded,
            texture_bytes: self.os.texture_bytes_uploaded,
        };
        if Self::total_drawcall_log_enabled() {
            static LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
            if LOG_COUNT.fetch_add(1, Ordering::Relaxed) < 200 {
                crate::log!(
                    "total_drawcalls repaint={} pass={:?} draw_list={:?} draw_calls_done={}",
                    self.repaint_id,
                    draw_pass_id,
                    draw_list_id,
                    self.os.draw_calls_done
                );
            }
        }

        let () = unsafe { msg_send![encoder, endEncoding] };
        let gpu_frame_group_key = self.get_pass_window_id(draw_pass_id).map(|window_id| {
            // Group GPU timing by (window, repaint_id) so we don't merge ranges
            // across multiple frames that happen to complete out-of-order.
            (window_id.id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ self.repaint_id
        });

        match mode {
            DrawPassMode::MTKView(view) => {
                let drawable: ObjcId = unsafe { msg_send![view, currentDrawable] };
                let first_texture: ObjcId = unsafe { msg_send![drawable, texture] };
                let () = unsafe { msg_send![command_buffer, presentDrawable: drawable] };
                let screenshot = self.build_screenshot_struct(
                    metal_cx,
                    command_buffer,
                    0,
                    pass_width as usize,
                    pass_height as usize,
                    first_texture,
                    None,
                );
                self.commit_command_buffer(
                    screenshot,
                    None,
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    command_buffer,
                );
            }
            DrawPassMode::Texture => {
                self.commit_command_buffer(
                    None,
                    None,
                    gpu_frame_group_key,
                    false,
                    gpu_counters,
                    command_buffer,
                );
            }
            DrawPassMode::StdinTexture => {
                self.commit_command_buffer(
                    None,
                    None,
                    gpu_frame_group_key,
                    false,
                    gpu_counters,
                    command_buffer,
                );
            }
            DrawPassMode::StdinMain(stdin_frame, kind_id) => {
                let main_texture = &self.passes[draw_pass_id].color_textures[0];
                let tex = &self.textures[main_texture.texture.texture_id()];
                let screenshot = if let Some(texture) = &tex.os.texture {
                    self.build_screenshot_struct(
                        metal_cx,
                        command_buffer,
                        kind_id,
                        pass_width as usize,
                        pass_height as usize,
                        texture.as_id(),
                        tex.alloc.clone(),
                    )
                } else {
                    None
                };
                self.commit_command_buffer(
                    screenshot,
                    Some(stdin_frame),
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    command_buffer,
                );
            }
            DrawPassMode::Drawable(drawable) => {
                let first_texture: ObjcId = unsafe { msg_send![drawable, texture] };
                let () = unsafe { msg_send![command_buffer, presentDrawable: drawable] };
                let screenshot = self.build_screenshot_struct(
                    metal_cx,
                    command_buffer,
                    0,
                    pass_width as usize,
                    pass_height as usize,
                    first_texture,
                    None,
                );
                self.commit_command_buffer(
                    screenshot,
                    None,
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    command_buffer,
                );
            }
            DrawPassMode::Resizing(drawable) => {
                let first_texture: ObjcId = unsafe { msg_send![drawable, texture] };
                let screenshot = self.build_screenshot_struct(
                    metal_cx,
                    command_buffer,
                    0,
                    pass_width as usize,
                    pass_height as usize,
                    first_texture,
                    None,
                );
                self.commit_command_buffer(
                    screenshot,
                    None,
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    command_buffer,
                );
                let () = unsafe { msg_send![command_buffer, waitUntilScheduled] };
                let () = unsafe { msg_send![drawable, present] };
            }
        }
        let () = unsafe { msg_send![pool, release] };
    }

    fn build_screenshot_struct(
        &mut self,
        metal_cx: &MetalCx,
        command_buffer: ObjcId,
        kind_id: usize,
        width: usize,
        height: usize,
        in_texture: ObjcId,
        alloc: Option<TextureAlloc>,
    ) -> Option<ScreenshotInfo> {
        let request_ids = self.take_studio_screenshot_request_ids(kind_id as u32);
        let (tex_width, tex_height) = if let Some(alloc) = alloc {
            (alloc.width, alloc.height)
        } else {
            (width, height)
        };
        if !request_ids.is_empty() {
            let descriptor = RcObjcId::from_owned(
                NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
            );
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: tex_width as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: tex_height as u64] };
            let _: () = unsafe {
                msg_send![descriptor.as_id(), setPixelFormat: MTLPixelFormat::BGRA8Unorm]
            };
            let texture: ObjcId =
                unsafe { msg_send![metal_cx.device, newTextureWithDescriptor: descriptor] };
            unsafe {
                let blit_encoder: ObjcId = msg_send![command_buffer, blitCommandEncoder];
                let () = msg_send![blit_encoder, copyFromTexture: in_texture toTexture:texture];
                let () = msg_send![blit_encoder, synchronizeTexture: texture slice:0 level:0];
                let () = msg_send![blit_encoder, endEncoding];
            };
            return Some(ScreenshotInfo {
                request_ids,
                width: width as _,
                height: height as _,
                texture: texture,
            });
        }
        None
    }

    fn commit_command_buffer(
        &self,
        screenshot_info: Option<ScreenshotInfo>,
        stdin_frame: Option<PresentableDraw>,
        gpu_frame_group_key: Option<u64>,
        flush_gpu_frame_group: bool,
        gpu_counters: GpuSampleCounters,
        command_buffer: ObjcId,
    ) {
        let screenshot_info = Mutex::new(screenshot_info);
        //let present_index = Arc::clone(&self.os.present_index);
        //Self::stdin_send_draw_complete(&present_index);
        let start_time = self.os.start_time.unwrap();
        let () = unsafe {
            msg_send![
                command_buffer,
                addCompletedHandler: &objc_block!(move | command_buffer: ObjcId | {
                    // alright lets grab a texture if need be
                    if let Some(sf) = &*screenshot_info.lock().unwrap(){
                        let mut bgra = vec![0u8; sf.width * sf.height * 4];
                        let region = MTLRegion {
                            origin: MTLOrigin {x: 0, y: 0, z: 0},
                            size: MTLSize {width: sf.width as u64, height: sf.height as u64, depth: 1}
                        };
                        let _:() = unsafe{msg_send![
                            sf.texture,
                            getBytes: bgra.as_mut_ptr()
                            bytesPerRow: sf.width *4
                            bytesPerImage: sf.width * sf.height * 4
                            fromRegion: region
                            mipmapLevel: 0
                            slice: 0
                        ]};
                        let () = msg_send![sf.texture, release];

                        // Metal readback for BGRA8 textures returns BGRA bytes. Convert to RGBA
                        // before PNG encoding so AppToStudio::Screenshot always transports PNG bytes.
                        for px in bgra.chunks_exact_mut(4) {
                            px.swap(0, 2);
                        }
                        let png = match encode_png_rgba(sf.width as u32, sf.height as u32, &bgra) {
                            Ok(png) => png,
                            Err(err) => {
                                crate::error!("{}", err);
                                Vec::new()
                            }
                        };
                        Cx::send_studio_screenshot_response(
                            sf.request_ids.clone(),
                            sf.width as _,
                            sf.height as _,
                            png,
                        );
                    }

                    let raw_start: f64 = unsafe { msg_send![command_buffer, GPUStartTime] };
                    let raw_end: f64 = unsafe { msg_send![command_buffer, GPUEndTime] };
                    if let Some(_stdin_frame) = stdin_frame {
                        #[cfg(target_os = "macos")]
                        Self::stdin_send_draw_complete(_stdin_frame);
                    }

                    let raw_range = if let Some(group_key) = gpu_frame_group_key {
                        // Aggregate all command buffers that belong to one presented frame
                        // (offscreen passes + final present) into one GPU interval.
                        if let Ok(mut frame_ranges) = METAL_GPU_FRAME_RANGES.lock() {
                            let ranges = frame_ranges.get_or_insert_with(HashMap::new);
                            if raw_start.is_finite()
                                && raw_end.is_finite()
                                && raw_start > 0.0
                                && raw_end >= raw_start
                            {
                                if let Some((start, end)) = ranges.get_mut(&group_key) {
                                    *start = start.min(raw_start);
                                    *end = end.max(raw_end);
                                } else {
                                    ranges.insert(group_key, (raw_start, raw_end));
                                }
                                // Safety valve: if a backend path never flushes grouped
                                // ranges, avoid unbounded map growth.
                                if ranges.len() > 1024 {
                                    ranges.clear();
                                }
                            }
                            if flush_gpu_frame_group {
                                ranges.remove(&group_key)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        Some((raw_start, raw_end))
                    };

                    let app_now = Instant::now().duration_since(start_time).as_secs_f64();
                    let host_now = unsafe { CACurrentMediaTime() };
                    let frame_counters = if let Some(group_key) = gpu_frame_group_key {
                        if let Ok(mut grouped) = METAL_GPU_FRAME_COUNTERS.lock() {
                            let counters = grouped.get_or_insert_with(HashMap::new);
                            if let Some(aggregated) = counters.get_mut(&group_key) {
                                aggregated.accumulate(gpu_counters);
                            } else {
                                counters.insert(group_key, gpu_counters);
                            }
                            if counters.len() > 1024 {
                                counters.clear();
                            }
                            if flush_gpu_frame_group {
                                counters.remove(&group_key).unwrap_or_default()
                            } else {
                                GpuSampleCounters::default()
                            }
                        } else {
                            gpu_counters
                        }
                    } else {
                        gpu_counters
                    };
                    if let Some((raw_sample_start, raw_sample_end)) = raw_range {
                        if let Some((start, end)) = map_metal_gpu_times_to_app_timeline(
                            raw_sample_start,
                            raw_sample_end,
                            app_now,
                            host_now,
                        ) {
                            Cx::send_studio_message(AppToStudio::GPUSample(GPUSample {
                                start,
                                end,
                                draw_calls: frame_counters.draw_calls,
                                instances: frame_counters.instances,
                                vertices: frame_counters.vertices,
                                instance_bytes: frame_counters.instance_bytes,
                                uniform_bytes: frame_counters.uniform_bytes,
                                vertex_buffer_bytes: frame_counters.vertex_buffer_bytes,
                                texture_bytes: frame_counters.texture_bytes,
                            }));
                        }
                    }
                })
            ]
        };
        let () = unsafe { msg_send![command_buffer, commit] };
    }

    pub(crate) fn mtl_compile_shaders(&mut self, metal_cx: &MetalCx) {
        for draw_shader_id in self
            .draw_shaders
            .compile_set
            .iter()
            .cloned()
            .collect::<Vec<_>>()
        {
            let cx_shader = &self.draw_shaders.shaders[draw_shader_id];

            let mtlsl = match &cx_shader.mapping.code {
                CxDrawShaderCode::Combined { code } => code.clone(),
                CxDrawShaderCode::Separate { .. } => {
                    crate::error!("Metal does not support separate vertex/fragment sources");
                    continue;
                }
            };

            if cx_shader.mapping.flags.debug_code {
                println!(
                    "=== Generated Metal Shader ===\n{}\n=== End Metal Shader ===",
                    mtlsl
                );
            }

            // Get the uniform buffer bindings from the mapping
            let bindings = cx_shader.mapping.uniform_buffer_bindings.clone();

            // Check if we already have an os_shader with the same source
            let mut found_os_shader_id = None;
            for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                if ds.mtlsl == mtlsl {
                    found_os_shader_id = Some(index);
                    break;
                }
            }

            let cx_shader = &mut self.draw_shaders.shaders[draw_shader_id];
            if let Some(os_shader_id) = found_os_shader_id {
                cx_shader.os_shader_id = Some(os_shader_id);
            } else {
                if let Some(shp) = CxOsDrawShader::new(metal_cx, mtlsl, &bindings) {
                    cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                    self.draw_shaders.os_shaders.push(shp);
                }
            }
        }
        self.draw_shaders.compile_set.clear();
    }

    #[cfg(target_os = "macos")]
    pub fn share_texture_for_presentable_image(&mut self, texture: &Texture) -> u32 {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.metal_device.unwrap())
    }

    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    pub fn share_texture_for_presentable_image(&mut self, _texture: &Texture) -> u32 {
        0
    }

    /// Create an IOSurface-backed texture for embedding Servo's CGL rendering
    /// in Makepad's Metal pipeline. Returns the Makepad Texture handle, the
    /// IOSurfaceRef pointer (for CGL binding), and the IOSurface ID.
    ///
    /// The IOSurface is created by Makepad and owned by the returned Texture.
    /// The caller (Servo's MacosRenderingContext) binds to the same IOSurface
    /// via CGLTexImageIOSurface2D for zero-copy cross-API rendering.
    #[cfg(target_os = "macos")]
    pub fn create_iosurface_render_texture(
        &mut self,
        width: usize,
        height: usize,
    ) -> (Texture, *mut std::ffi::c_void, u32) {
        use crate::shared_framebuf::PresentableImageId;
        use crate::texture::TextureFormat;

        let texture = Texture::new_with_format(
            self,
            TextureFormat::SharedBGRAu8 {
                width,
                height,
                id: PresentableImageId::alloc(),
                initial: true,
            },
        );
        let cxtexture = &mut self.textures[texture.texture_id()];
        let iosurface_id = cxtexture.update_shared_texture(self.os.metal_device.unwrap());
        let iosurface_ref = cxtexture.os.iosurface.unwrap_or(std::ptr::null_mut());
        (texture, iosurface_ref, iosurface_id)
    }
}

#[derive(Clone)]
struct ScreenshotInfo {
    width: usize,
    height: usize,
    request_ids: Vec<u64>,
    texture: ObjcId,
}

pub enum DrawPassMode {
    Texture,
    StdinTexture,
    MTKView(ObjcId),
    StdinMain(PresentableDraw, usize),
    Drawable(ObjcId),
    Resizing(ObjcId),
}

impl DrawPassMode {
    fn is_drawable(&self) -> Option<ObjcId> {
        match self {
            Self::Drawable(obj) | Self::Resizing(obj) => Some(*obj),
            Self::StdinMain(_, _) | Self::Texture | Self::StdinTexture | Self::MTKView(_) => None,
        }
    }
}

pub struct MetalCx {
    pub device: ObjcId,
    command_queue: ObjcId,
}

#[derive(Clone, Default)]
pub struct CxOsDrawList {}

#[derive(Default, Clone)]
pub struct CxOsPass {
    mtl_depth_state_write: Option<ObjcId>,
    mtl_depth_state_no_write: Option<ObjcId>,
}

pub enum PackType {
    Packed,
    Unpacked,
}
/*
pub struct SlErr {
    _msg: String
}*/

impl MetalCx {
    pub(crate) fn new() -> MetalCx {
        let device = get_default_metal_device().expect("Cannot get default metal device");
        MetalCx {
            command_queue: unsafe { msg_send![device, newCommandQueue] },
            device: device,
        }
    }
}

/**************************************************************************************************/

pub struct CxOsDrawShader {
    _library: RcObjcId,
    render_pipeline_state: RcObjcId,
    draw_call_uniform_buffer_id: Option<u64>,
    pass_uniform_buffer_id: Option<u64>,
    draw_list_uniform_buffer_id: Option<u64>,
    dyn_uniform_buffer_id: Option<u64>,
    scope_uniform_buffer_id: Option<u64>,
    pub mtlsl: String,
}

// alright lets go process this shader
impl DrawVars {
    pub(crate) fn compile_shader(&mut self, vm: &mut ScriptVm, _apply: &Apply, value: ScriptValue) {
        // Shader caching strategy:
        // 1. Check object_id cache (fastest - exact same object)
        // 2. Check function hash cache (same functions even if different object instance)
        // 3. Check code cache (different functions but identical generated code)

        if let Some(io_self) = value.as_object() {
            // Cache 1: Check if this exact object has been compiled before
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&io_self) {
                    // log!("Shader cache HIT (object_id)");
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            // Cache 2: Compute function hash and check if we've seen these functions before
            let fnhash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, io_self);
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_functions_to_shader.get(&fnhash) {
                    // Add to object_id cache for faster lookup next time
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            // Not in function cache, need to compile
            let mut output = ShaderOutput::default();
            output.backend = ShaderBackend::Metal;

            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);

            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(vertex).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Vertex;
                // Entry point shaders don't have script-level arguments to validate, use NoTrap
                ShaderFnCompiler::compile_shader_def(
                    vm,
                    &mut output,
                    NoTrap,
                    id!(vertex),
                    fnobj,
                    ShaderType::IoSelf(io_self),
                    vec![],
                );
            }
            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(fragment).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Fragment;
                // Entry point shaders don't have script-level arguments to validate, use NoTrap
                ShaderFnCompiler::compile_shader_def(
                    vm,
                    &mut output,
                    NoTrap,
                    id!(fragment),
                    fnobj,
                    ShaderType::IoSelf(io_self),
                    vec![],
                );
            }

            // Don't proceed if shader compilation had errors
            if output.has_errors {
                return;
            }

            // Assign buffer indices to uniform buffers before generating Metal code
            // Buffer indices start at 3 (0=vertex buffer, 1=instance buffer, 2=uniform struct)
            output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

            let mut out = String::new();
            write!(out, "#include <metal_stdlib>\nusing namespace metal;\n").ok();
            output.create_struct_defs(vm, &mut out);
            output.metal_create_instance_struct(vm, &mut out);
            output.metal_create_uniform_struct(vm, &mut out);
            output.metal_create_scope_uniform_struct(vm, &mut out);
            output.metal_create_varying_struct(vm, &mut out);
            output.metal_create_vertex_buffer_struct(vm, &mut out);
            output.metal_create_io_struct(vm, &mut out);
            output.metal_create_io_vertex_struct(vm, &mut out);
            output.metal_create_io_framebuffer_struct(vm, &mut out);
            output.metal_create_io_fragment_struct(vm, &mut out);
            output.metal_create_sampler_decls(&mut out);
            output.create_functions(&mut out);
            output.metal_create_vertex_fn(vm, &mut out);
            output.metal_create_fragment_main_fn(vm, &mut out);

            let source = vm.bx.heap.new_object_ref(io_self);

            // Create the shader mapping and allocate CxDrawShader
            let code = CxDrawShaderCode::Combined { code: out };

            // Cache 3: Check if this exact code has been compiled before
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_code_to_shader.get(&code) {
                    // Add to both object_id and function hash caches
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    cx.draw_shaders
                        .cache_functions_to_shader
                        .insert(fnhash, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            // Extract geometry_id from the vertex buffer object before creating the mapping
            let geometry_id = if let Some(vb_obj) = output.find_vertex_buffer_object(vm, io_self) {
                let buffer_value =
                    vm.bx
                        .heap
                        .value(vb_obj, id!(buffer).into(), vm.thread().trap.pass());
                if let Some(handle) = buffer_value.as_handle() {
                    vm.bx
                        .heap
                        .handle_ref::<Geometry>(handle)
                        .map(|g| g.geometry_id())
                } else {
                    None
                }
            } else {
                None
            };

            let mut mapping = CxDrawShaderMapping::from_shader_output(
                source,
                code.clone(),
                &vm.bx.heap,
                &output,
                geometry_id,
            );

            // Fill the scope uniform buffer from current script values
            mapping.fill_scope_uniforms_buffer(&vm.bx.heap, &vm.thread().trap.pass());

            // Set dyn_instance_start and dyn_instance_slots based on mapping
            self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
            self.dyn_instance_slots = mapping.instances.total_slots;

            // Access Cx from the vm host
            let cx = vm.host.cx_mut();

            // Allocate CxDrawShader with os_shader_id set to None
            let index = cx.draw_shaders.shaders.len();
            cx.draw_shaders.shaders.push(CxDrawShader {
                debug_id: LiveId(0),
                os_shader_id: None,
                mapping,
            });

            // Create the shader ID
            let shader_id = DrawShaderId { index };

            // Add to all caches
            cx.draw_shaders
                .cache_object_id_to_shader
                .insert(io_self, shader_id);
            cx.draw_shaders
                .cache_functions_to_shader
                .insert(fnhash, shader_id);
            cx.draw_shaders.cache_code_to_shader.insert(code, shader_id);

            // Add to compile set for later Metal compilation
            cx.draw_shaders.compile_set.insert(index);

            // Set draw_shader on self
            self.draw_shader_id = Some(shader_id);

            // Use the geometry_id stored on the mapping
            self.geometry_id = geometry_id;
        }
    }
}

impl CxOsDrawShader {
    pub(crate) fn new(
        metal_cx: &MetalCx,
        mtlsl: String,
        bindings: &UniformBufferBindings,
    ) -> Option<Self> {
        let options = RcObjcId::from_owned(unsafe { msg_send![class!(MTLCompileOptions), new] });
        unsafe {
            let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
        };

        let mut error: ObjcId = nil;
        let library = RcObjcId::from_owned(
            match NonNull::new(unsafe {
                msg_send![
                    metal_cx.device,
                    newLibraryWithSource: str_to_nsstring(&mtlsl)
                    options: options
                    error: &mut error
                ]
            }) {
                Some(library) => library,
                None => {
                    let description: ObjcId = unsafe { msg_send![error, localizedDescription] };
                    let string = nsstring_to_string(description);
                    let mut out = format!("{}\n", string);
                    for (index, line) in mtlsl.split("\n").enumerate() {
                        out.push_str(&format!("{}: {}\n", index + 1, line));
                    }
                    crate::error!("{}", out);
                    return None;
                }
            },
        );

        let descriptor = RcObjcId::from_owned(
            NonNull::new(unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] }).unwrap(),
        );

        let vertex_function = RcObjcId::from_owned(
            NonNull::new(unsafe {
                msg_send![library.as_id(), newFunctionWithName: str_to_nsstring("vertex_main")]
            })
            .unwrap(),
        );

        let fragment_function = RcObjcId::from_owned(
            NonNull::new(unsafe {
                msg_send![library.as_id(), newFunctionWithName: str_to_nsstring("fragment_main")]
            })
            .unwrap(),
        );

        let render_pipeline_state = RcObjcId::from_owned(NonNull::new(unsafe {
            let _: () = msg_send![descriptor.as_id(), setVertexFunction: vertex_function];
            let _: () = msg_send![descriptor.as_id(), setFragmentFunction: fragment_function];

            let color_attachments: ObjcId = msg_send![descriptor.as_id(), colorAttachments];
            let color_attachment: ObjcId = msg_send![color_attachments, objectAtIndexedSubscript: 0];
            let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![color_attachment, setBlendingEnabled: YES];
            let () = msg_send![color_attachment, setRgbBlendOperation: MTLBlendOperation::Add];
            let () = msg_send![color_attachment, setAlphaBlendOperation: MTLBlendOperation::Add];
            let () = msg_send![color_attachment, setSourceRGBBlendFactor: MTLBlendFactor::One];
            let () = msg_send![color_attachment, setSourceAlphaBlendFactor: MTLBlendFactor::One];
            let () = msg_send![color_attachment, setDestinationRGBBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
            let () = msg_send![color_attachment, setDestinationAlphaBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];

            let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float];

            let mut error: ObjcId = nil;
            msg_send![
                metal_cx.device,
                newRenderPipelineStateWithDescriptor: descriptor
                error: &mut error
            ]
        }).unwrap());

        // Look up buffer IDs from shader output bindings by Pod type name
        let draw_call_uniform_buffer_id = bindings
            .get_by_type_name(id!(DrawCallUniforms))
            .map(|i| i as u64);
        let pass_uniform_buffer_id = bindings
            .get_by_type_name(id!(DrawPassUniforms))
            .map(|i| i as u64);
        let draw_list_uniform_buffer_id = bindings
            .get_by_type_name(id!(DrawListUniforms))
            .map(|i| i as u64);
        // dyn_uniform_buffer_id is not in bindings, it uses the IoUniform struct at buffer(2)
        let dyn_uniform_buffer_id = Some(2);
        // scope_uniform_buffer_id comes from bindings if there are scope uniforms
        let scope_uniform_buffer_id = bindings.scope_uniform_buffer_index.map(|i| i as u64);

        return Some(Self {
            _library: library,
            render_pipeline_state,
            draw_call_uniform_buffer_id,
            pass_uniform_buffer_id,
            draw_list_uniform_buffer_id,
            dyn_uniform_buffer_id,
            scope_uniform_buffer_id,
            mtlsl,
        });
    }
}

#[derive(Default)]
pub struct CxOsDrawCall {
    instance_buffer: MetalBuffer,
}

#[derive(Default)]
pub struct CxOsGeometry {
    vertex_buffer: MetalBuffer,
    index_buffer: MetalBuffer,
}

#[derive(Default)]
struct MetalBuffer {
    inner: Option<MetalBufferInner>,
}

impl MetalBuffer {
    fn update<T>(&mut self, metal_cx: &MetalCx, data: &[T]) {
        let len = data.len() * std::mem::size_of::<T>();
        if len == 0 {
            self.inner = None;
            return;
        }
        if let Some(inner) = self.inner.as_mut() {
            if inner.len == len {
                let dst = unsafe {
                    let ptr: *mut std::ffi::c_void = msg_send![inner.buffer.as_id(), contents];
                    ptr
                };
                if !dst.is_null() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr() as *const u8,
                            dst as *mut u8,
                            len,
                        );
                    }
                    #[cfg(target_os = "macos")]
                    unsafe {
                        let range = NSRange {
                            location: 0,
                            length: len as u64,
                        };
                        let _: () = msg_send![inner.buffer.as_id(), didModifyRange: range];
                    }
                    return;
                }
            }
        }
        self.inner = Some(MetalBufferInner {
            buffer: RcObjcId::from_owned(
                NonNull::new(unsafe {
                    msg_send![
                        metal_cx.device,
                        newBufferWithBytes: data.as_ptr() as *const std::ffi::c_void
                        length: len as u64
                        options: nil
                    ]
                })
                .unwrap(),
            ),
            len,
        });
    }
}

struct MetalBufferInner {
    buffer: RcObjcId,
    len: usize,
}

#[derive(Default)]
pub struct CxOsTexture {
    pub(crate) texture: Option<RcObjcId>,
    #[cfg(target_os = "macos")]
    iosurface: Option<IOSurfaceRef>,
    #[cfg(target_os = "macos")]
    iosurface_id: IOSurfaceID,
}
fn texture_pixel_to_mtl_pixel(pix: &TexturePixel) -> MTLPixelFormat {
    match pix {
        TexturePixel::BGRAu8 => MTLPixelFormat::BGRA8Unorm,
        TexturePixel::RGBAf16 => MTLPixelFormat::RGBA16Float,
        TexturePixel::RGBAf32 => MTLPixelFormat::RGBA32Float,
        TexturePixel::Ru8 => MTLPixelFormat::R8Unorm,
        TexturePixel::RGu8 => MTLPixelFormat::RG8Unorm,
        TexturePixel::Rf32 => MTLPixelFormat::R32Float,
        TexturePixel::D32 => MTLPixelFormat::Depth32Float,
        TexturePixel::VideoRGB => MTLPixelFormat::BGRA8Unorm,
    }
}
impl CxTexture {
    /*
    pub fn copy_to_system_ram(
        &self,
        _metal_cx: &MetalCx
    )->Option<Vec<u8>>{
        if let Some(alloc) = &self.alloc{
            if let Some(texture) = &self.os.texture{
                let mut buf = Vec::new();
                buf.resize(alloc.width * alloc.height * 4, 0u8);
                let region = MTLRegion {
                    origin: MTLOrigin {x: 0, y: 0, z: 0},
                    size: MTLSize {width: alloc.width as u64, height: alloc.height as u64, depth: 1}
                };
                let _:() = unsafe{msg_send![
                    texture.as_id(),
                    getBytes: buf.as_ptr()
                    bytesPerRow: alloc.width *4
                    bytesPerImage: alloc.width * alloc.height * 4
                    fromRegion: region
                    mipmapLevel: 0
                    slice: 0
                ]};
                return Some(buf);
            }
        }
        None
    }*/

    fn update_vec_texture(&mut self, metal_cx: &MetalCx) -> u64 {
        if self.alloc_vec() {
            let alloc = self.alloc.as_ref().unwrap();

            let descriptor = RcObjcId::from_owned(
                NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
            );
            let texture_type = match &self.format {
                TextureFormat::VecCubeBGRAu8_32 { .. } => MTLTextureType::Cube,
                _ => MTLTextureType::D2,
            };
            let _: () = unsafe { msg_send![descriptor.as_id(), setTextureType: texture_type] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
            let _: () = unsafe {
                msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]
            };
            let texture: ObjcId =
                unsafe { msg_send![metal_cx.device, newTextureWithDescriptor: descriptor] };
            self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
        }
        let update = self.take_updated();
        if update.is_empty() {
            return 0;
        }

        fn update_data(
            texture: &Option<RcObjcId>,
            width: usize,
            height: usize,
            bpp: u64,
            data: *const std::ffi::c_void,
        ) {
            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: width as u64,
                    height: height as u64,
                    depth: 1,
                },
            };

            let () = unsafe {
                msg_send![
                    texture.as_ref().unwrap().as_id(),
                    replaceRegion: region
                    mipmapLevel: 0
                    withBytes: data
                    bytesPerRow: (width as u64) * bpp
                ]
            };
        }

        fn update_cube_data(
            texture: &Option<RcObjcId>,
            width: usize,
            height: usize,
            bpp: u64,
            data: &[u32],
        ) {
            let pixels_per_face = width.saturating_mul(height);
            let words_per_face = pixels_per_face;
            if data.len() < words_per_face.saturating_mul(6) {
                return;
            }
            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: width as u64,
                    height: height as u64,
                    depth: 1,
                },
            };
            let face_bytes = words_per_face.saturating_mul(4);
            for face in 0..6usize {
                let face_offset_words = face.saturating_mul(words_per_face);
                let face_ptr = unsafe { data.as_ptr().add(face_offset_words) };
                let _: () = unsafe {
                    msg_send![
                        texture.as_ref().unwrap().as_id(),
                        replaceRegion: region
                        mipmapLevel: 0
                        slice: face as u64
                        withBytes: face_ptr as *const std::ffi::c_void
                        bytesPerRow: (width as u64) * bpp
                        bytesPerImage: face_bytes as u64
                    ]
                };
            }
        }
        match &self.format {
            TextureFormat::VecBGRAu8_32 {
                width,
                height,
                data,
                ..
            } => {
                update_data(
                    &self.os.texture,
                    *width,
                    *height,
                    4,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                );
                (*width as u64)
                    .saturating_mul(*height as u64)
                    .saturating_mul(4)
            }
            TextureFormat::VecCubeBGRAu8_32 {
                width,
                height,
                data,
                ..
            } => {
                if let Some(data) = data.as_ref() {
                    update_cube_data(&self.os.texture, *width, *height, 4, data);
                }
                (*width as u64)
                    .saturating_mul(*height as u64)
                    .saturating_mul(4)
                    .saturating_mul(6)
            }
            TextureFormat::VecRGBAf32 {
                width,
                height,
                data,
                ..
            } => {
                update_data(
                    &self.os.texture,
                    *width,
                    *height,
                    16,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                );
                (*width as u64)
                    .saturating_mul(*height as u64)
                    .saturating_mul(16)
            }
            TextureFormat::VecRu8 {
                width,
                height,
                data,
                ..
            } => {
                update_data(
                    &self.os.texture,
                    *width,
                    *height,
                    1,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                );
                (*width as u64).saturating_mul(*height as u64)
            }
            TextureFormat::VecRGu8 {
                width,
                height,
                data,
                ..
            } => {
                update_data(
                    &self.os.texture,
                    *width,
                    *height,
                    2,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                );
                (*width as u64)
                    .saturating_mul(*height as u64)
                    .saturating_mul(2)
            }
            TextureFormat::VecRf32 {
                width,
                height,
                data,
                ..
            } => {
                update_data(
                    &self.os.texture,
                    *width,
                    *height,
                    4,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                );
                (*width as u64)
                    .saturating_mul(*height as u64)
                    .saturating_mul(4)
            }
            _ => 0,
        }
    }

    #[cfg(target_os = "macos")]
    fn update_shared_texture(&mut self, metal_device: ObjcId) -> IOSurfaceID {
        // we need a width/height for this one.
        if !self.alloc_shared() {
            return self.os.iosurface_id;
        }
        let alloc = self.alloc.as_ref().unwrap();

        // Create IOSurface properties dictionary
        let iosurface_props: ObjcId = unsafe {
            let dict: ObjcId = msg_send![class!(NSMutableDictionary), new];

            // IOSurfaceWidth
            let width_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfaceWidth");
            let width_val: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInteger: alloc.width as u64];
            let _: () = msg_send![dict, setObject: width_val forKey: width_key];

            // IOSurfaceHeight
            let height_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfaceHeight");
            let height_val: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInteger: alloc.height as u64];
            let _: () = msg_send![dict, setObject: height_val forKey: height_key];

            // IOSurfaceBytesPerElement (4 for BGRA8)
            let bpe_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfaceBytesPerElement");
            let bpe_val: ObjcId = msg_send![class!(NSNumber), numberWithUnsignedInteger: 4u64];
            let _: () = msg_send![dict, setObject: bpe_val forKey: bpe_key];

            // IOSurfacePixelFormat (BGRA = 'BGRA' = 0x42475241)
            let pf_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfacePixelFormat");
            let pf_val: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInteger: 0x42475241u64];
            let _: () = msg_send![dict, setObject: pf_val forKey: pf_key];

            // Mark as global to allow cross-process lookup via IOSurfaceLookup
            let global_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfaceIsGlobal");
            let global_val: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
            let _: () = msg_send![dict, setObject: global_val forKey: global_key];

            dict
        };

        // Create IOSurface
        let iosurface = unsafe { IOSurfaceCreate(iosurface_props) };
        unsafe {
            let _: () = msg_send![iosurface_props, release];
        }

        if iosurface.is_null() {
            crate::error!("Failed to create IOSurface");
            return 0;
        }

        // Get the global IOSurface ID for cross-process sharing
        let iosurface_id = unsafe { IOSurfaceGetID(iosurface) };

        // Create Metal texture descriptor
        let descriptor = RcObjcId::from_owned(
            NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
        );

        let _: () = unsafe { msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2] };
        let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
        let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
        let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
        let _: () =
            unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private] };
        let _: () = unsafe {
            msg_send![descriptor.as_id(), setUsage: (MTLTextureUsage::RenderTarget as u64 | MTLTextureUsage::ShaderRead as u64)]
        };
        let _: () = unsafe {
            msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]
        };

        // Create Metal texture from IOSurface
        let texture: ObjcId = unsafe {
            msg_send![metal_device, newTextureWithDescriptor: descriptor.as_id() iosurface: iosurface plane: 0u64]
        };

        if texture.is_null() {
            crate::error!("Failed to create Metal texture from IOSurface");
            unsafe {
                CFRelease(iosurface);
            }
            return 0;
        }

        // Store the IOSurface and ID (keep IOSurface alive)
        self.os.iosurface = Some(iosurface);
        self.os.iosurface_id = iosurface_id;
        self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));

        iosurface_id
    }

    #[cfg(target_os = "macos")]
    pub fn update_from_shared_handle(
        &mut self,
        metal_cx: &MetalCx,
        iosurface_id: IOSurfaceID,
    ) -> bool {
        // we need a width/height for this one.
        if !self.alloc_shared() {
            return true;
        }
        let alloc = self.alloc.as_ref().unwrap();

        // Look up IOSurface by its global ID (works across processes!)
        let iosurface = unsafe { IOSurfaceLookup(iosurface_id) };
        if iosurface.is_null() {
            crate::error!("Failed to lookup IOSurface with ID {}", iosurface_id);
            return false;
        }

        // Create Metal texture descriptor
        let descriptor = RcObjcId::from_owned(
            NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
        );

        let _: () = unsafe { msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2] };
        let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
        let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
        let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
        let _: () =
            unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private] };
        let _: () = unsafe {
            msg_send![descriptor.as_id(), setUsage: (MTLTextureUsage::RenderTarget as u64 | MTLTextureUsage::ShaderRead as u64)]
        };
        let _: () =
            unsafe { msg_send![descriptor.as_id(), setPixelFormat: MTLPixelFormat::BGRA8Unorm] };

        // Create Metal texture from IOSurface
        let texture: ObjcId = unsafe {
            msg_send![metal_cx.device, newTextureWithDescriptor: descriptor.as_id() iosurface: iosurface plane: 0u64]
        };

        if texture.is_null() {
            crate::error!("Failed to create Metal texture from IOSurface");
            unsafe {
                CFRelease(iosurface);
            }
            return false;
        }

        let width: u64 = unsafe { msg_send![texture, width] };
        let height: u64 = unsafe { msg_send![texture, height] };

        // FIXME(eddyb) can these be an assert now?
        if width != alloc.width as u64 || height != alloc.height as u64 {
            crate::error!(
                "IOSurface size mismatch: expected {}x{}, got {}x{}",
                alloc.width,
                alloc.height,
                width,
                height
            );
            unsafe {
                let _: () = msg_send![texture, release];
                CFRelease(iosurface);
            }
            return false;
        }

        // Store IOSurface and texture
        self.os.iosurface = Some(iosurface);
        self.os.iosurface_id = iosurface_id;
        self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
        true
    }

    fn update_render_target(&mut self, metal_cx: &MetalCx, width: usize, height: usize) {
        if self.alloc_render(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(
                NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
            );

            let _: () =
                unsafe { msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget] };
            let _: () = unsafe {
                msg_send![descriptor.as_id(),setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]
            };
            let texture = RcObjcId::from_owned(
                NonNull::new(unsafe {
                    msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
                })
                .unwrap(),
            );

            self.os.texture = Some(texture);
        }
    }

    fn update_depth_stencil(&mut self, metal_cx: &MetalCx, width: usize, height: usize) {
        if self.alloc_depth(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(
                NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
            );

            let _: () =
                unsafe { msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget] };
            let _: () = unsafe {
                msg_send![
                    descriptor.as_id(),
                    setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)
                ]
            };
            let texture = RcObjcId::from_owned(
                NonNull::new(unsafe {
                    msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
                })
                .unwrap(),
            );
            self.os.texture = Some(texture);
        }
    }
}

pub fn get_default_metal_device() -> Option<ObjcId> {
    unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev == nil {
            None
        } else {
            Some(dev)
        }
    }
}

pub fn get_all_metal_devices() -> Vec<ObjcId> {
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    unsafe {
        vec![MTLCreateSystemDefaultDevice()]
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let array = MTLCopyAllDevices();
        let count: u64 = msg_send![array, count];
        let ret = (0..count)
            .map(|i| msg_send![array, objectAtIndex: i])
            // The elements of this array are references---we convert them to owned references
            // (which just means that we increment the reference count here, and it is
            // decremented in the `Drop` impl for `Device`)
            .map(|device: *mut Object| msg_send![device, retain])
            .collect();
        let () = msg_send![array, release];
        ret
    }
}

/// CGL render bridge for macOS. Creates a standalone CGL context (GL 3.2 Core)
/// that shares textures with Metal via IOSurface.
#[cfg(target_os = "macos")]
pub struct CglRenderBridge {
    cgl_context: *mut std::ffi::c_void,
    cgl_pixel_format: *mut std::ffi::c_void,
    opengl_framework: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
impl CglRenderBridge {
    pub fn new() -> Self {
        use std::ffi::c_void;

        // CGL constants
        const K_CGL_PFA_OPENGL_PROFILE: u32 = 99;
        const K_CGL_OGL_PVERSION_3_2_CORE: u32 = 0x3200;
        const K_CGL_PFA_COLOR_SIZE: u32 = 8;
        const K_CGL_PFA_DEPTH_SIZE: u32 = 12;
        const K_CGL_PFA_STENCIL_SIZE: u32 = 13;
        const K_CGL_PFA_ACCELERATED: u32 = 73;
        const K_CGL_PFA_DOUBLE_BUFFER: u32 = 5;

        type CGLPixelFormatObj = *mut c_void;
        type CGLContextObj = *mut c_void;

        extern "C" {
            fn CGLChoosePixelFormat(
                attribs: *const u32,
                pix: *mut CGLPixelFormatObj,
                npix: *mut i32,
            ) -> i32;
            fn CGLCreateContext(
                pix: CGLPixelFormatObj,
                share: CGLContextObj,
                ctx: *mut CGLContextObj,
            ) -> i32;
        }

        unsafe {
            let attribs: &[u32] = &[
                K_CGL_PFA_OPENGL_PROFILE, K_CGL_OGL_PVERSION_3_2_CORE,
                K_CGL_PFA_COLOR_SIZE, 24,
                K_CGL_PFA_DEPTH_SIZE, 24,
                K_CGL_PFA_STENCIL_SIZE, 8,
                K_CGL_PFA_ACCELERATED,
                K_CGL_PFA_DOUBLE_BUFFER,
                0,
            ];

            let mut pix: CGLPixelFormatObj = std::ptr::null_mut();
            let mut npix: i32 = 0;
            let err = CGLChoosePixelFormat(attribs.as_ptr(), &mut pix, &mut npix);
            assert!(err == 0 && !pix.is_null(), "CGLChoosePixelFormat failed: {}", err);

            let mut ctx: CGLContextObj = std::ptr::null_mut();
            let err = CGLCreateContext(pix, std::ptr::null_mut(), &mut ctx);
            assert!(err == 0 && !ctx.is_null(), "CGLCreateContext failed: {}", err);

            // Load OpenGL.framework for dlsym-based proc address lookup
            extern "C" {
                fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
            }
            let framework_path = b"/System/Library/Frameworks/OpenGL.framework/OpenGL\0";
            let opengl_framework = dlopen(framework_path.as_ptr() as *const i8, 1); // RTLD_LAZY
            assert!(!opengl_framework.is_null(), "Failed to load OpenGL.framework");

            CglRenderBridge {
                cgl_context: ctx,
                cgl_pixel_format: pix,
                opengl_framework,
            }
        }
    }

    pub fn make_current(&self) {
        extern "C" {
            fn CGLSetCurrentContext(ctx: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            CGLSetCurrentContext(self.cgl_context);
        }
    }

    pub fn get_proc_address(&self, name: &str) -> *const std::ffi::c_void {
        extern "C" {
            fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
        }
        let c_name = std::ffi::CString::new(name).unwrap();
        unsafe { dlsym(self.opengl_framework, c_name.as_ptr()) }
    }

    pub fn gl_api(&self) -> crate::gl_render_bridge::GlApi {
        crate::gl_render_bridge::GlApi::GL
    }

    pub fn cgl_pixel_format(&self) -> *mut std::ffi::c_void {
        self.cgl_pixel_format
    }

    pub fn cgl_context(&self) -> *mut std::ffi::c_void {
        self.cgl_context
    }

    /// Bind an IOSurface to a GL texture in this CGL context.
    /// Returns the GL texture ID.
    pub fn bind_iosurface_to_gl_texture(
        &self,
        iosurface_ref: *mut std::ffi::c_void,
        width: usize,
        height: usize,
    ) -> u32 {
        use std::ffi::c_void;

        // GL constants for TEXTURE_RECTANGLE (macOS CGL uses rectangle textures for IOSurface)
        const GL_TEXTURE_RECTANGLE: u32 = 0x84F5;
        const GL_RGBA: u32 = 0x1908;
        const GL_BGRA: u32 = 0x80E1;
        const GL_UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;

        type GLuint = u32;
        type GLenum = u32;
        type GLsizei = i32;

        // Load GL functions via dlsym
        type GlGenTexturesFn = unsafe extern "C" fn(GLsizei, *mut GLuint);
        type GlBindTextureFn = unsafe extern "C" fn(GLenum, GLuint);

        extern "C" {
            fn CGLTexImageIOSurface2D(
                ctx: *mut c_void,
                target: GLenum,
                internal_format: GLenum,
                width: GLsizei,
                height: GLsizei,
                format: GLenum,
                ty: GLenum,
                iosurface: *mut c_void,
                plane: GLuint,
            ) -> i32;
        }

        unsafe {
            let gl_gen_textures: GlGenTexturesFn =
                std::mem::transmute(self.get_proc_address("glGenTextures"));
            let gl_bind_texture: GlBindTextureFn =
                std::mem::transmute(self.get_proc_address("glBindTexture"));

            let mut gl_texture: GLuint = 0;
            gl_gen_textures(1, &mut gl_texture);
            gl_bind_texture(GL_TEXTURE_RECTANGLE, gl_texture);

            let err = CGLTexImageIOSurface2D(
                self.cgl_context,
                GL_TEXTURE_RECTANGLE,
                GL_RGBA,
                width as GLsizei,
                height as GLsizei,
                GL_BGRA,
                GL_UNSIGNED_INT_8_8_8_8_REV,
                iosurface_ref,
                0,
            );
            assert!(err == 0, "CGLTexImageIOSurface2D failed: {}", err);

            gl_bind_texture(GL_TEXTURE_RECTANGLE, 0);

            gl_texture
        }
    }
}
