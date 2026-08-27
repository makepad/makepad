use {
    crate::{
        cx::Cx,
        draw_list::{CxDrawKind, DrawListId},
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
        texture::{
            CxTexture, Texture, TextureAlloc, TextureFormat, TexturePixel, TextureUpdated,
        },
    },
    makepad_objc_sys::{class, msg_send, sel, sel_impl},
    makepad_studio_protocol::{AppToStudio, GPUSample},
    makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    },
    std::cell::RefCell,
    std::collections::{HashMap, VecDeque},
    std::fmt::Write,
    std::sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    std::sync::{Arc, Mutex},
    std::time::{Duration, Instant},
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
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
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
                let child_resets_zbias = self.draw_lists[sub_list_id].reset_zbias;
                let mut child_zbias = 0.0f32;
                self.render_view(
                    draw_pass_id,
                    sub_list_id,
                    if child_resets_zbias {
                        &mut child_zbias
                    } else {
                        zbias
                    },
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
                {
                    // Named in the hang diagnostic / GPU trace for this pass.
                    let mut seen = metal_cx.pass_shaders.borrow_mut();
                    if !seen.contains(&sh.debug_id) {
                        seen.push(sh.debug_id);
                    }
                }

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
                        self.passes[draw_pass_id]
                            .os
                            .mtl_depth_state_write
                            .as_ref()
                    } else {
                        self.passes[draw_pass_id]
                            .os
                            .mtl_depth_state_no_write
                            .as_ref()
                    };
                    if let Some(depth_state) = depth_state {
                        let () = unsafe {
                            msg_send![encoder, setDepthStencilState: depth_state.as_id()]
                        };
                    }
                }

                let cull_mode = if draw_call.options.backface_culling {
                    2u64 // MTLCullModeBack
                } else {
                    0u64 // MTLCullModeNone
                };
                unsafe {
                    let () = msg_send![encoder, setFrontFacingWinding: 1u64]; // MTLWindingCounterClockwise
                    let () = msg_send![encoder, setCullMode: cull_mode];
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

                // Everything bound below belongs to this command buffer
                // until it completes (`MetalBuffer::update` checks).
                geometry.os.vertex_buffer.mark_bound(metal_cx);
                geometry.os.index_buffer.mark_bound(metal_cx);
                draw_item.os.instance_buffer.mark_bound(metal_cx);

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
                    for (slot, id) in shp.custom_uniform_buffer_ids.iter().enumerate() {
                        let Some(uniform_buffer) = draw_call.uniform_buffer_slots[slot].as_ref()
                        else {
                            let () =
                                msg_send![encoder, setVertexBuffer: nil offset: 0 atIndex: *id];
                            let () =
                                msg_send![encoder, setFragmentBuffer: nil offset: 0 atIndex: *id];
                            continue;
                        };
                        let data = &self.uniform_buffers[uniform_buffer.uniform_buffer_id()].data;
                        if data.is_empty() {
                            let () =
                                msg_send![encoder, setVertexBuffer: nil offset: 0 atIndex: *id];
                            let () =
                                msg_send![encoder, setFragmentBuffer: nil offset: 0 atIndex: *id];
                            continue;
                        }
                        let () = msg_send![encoder, setVertexBytes: data.as_ptr() as *const std::ffi::c_void length: data.len() as u64 atIndex: *id];
                        let () = msg_send![encoder, setFragmentBytes: data.as_ptr() as *const std::ffi::c_void length: data.len() as u64 atIndex: *id];
                        uniform_bytes_uploaded =
                            uniform_bytes_uploaded.saturating_add((data.len() * 2) as u64);
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
                                setFragmentTexture: metal_cx.fallback_texture
                                atIndex: i as u64
                            ]
                        };
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setVertexTexture: metal_cx.fallback_texture
                                atIndex: i as u64
                            ]
                        };
                        continue;
                    };

                    let cxtexture = &mut self.textures[texture_id];

                    if cxtexture.format.is_shared() {
                        #[cfg(target_os = "macos")]
                        cxtexture.update_shared_texture(metal_cx.device);
                    }
                    // Vec textures were uploaded (and allocated) by
                    // `encode_vec_texture_uploads` on this pass's command
                    // buffer before the render encoder opened; binding here
                    // never touches their contents.

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
                    } else {
                        // No Metal texture backing yet — bind a 1×1 fallback
                        // texture. On iOS, sampling from nil is a GPU fault
                        // that aborts the command buffer.
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setFragmentTexture: metal_cx.fallback_texture
                                atIndex: i as u64
                            ]
                        };
                        let () = unsafe {
                            msg_send![
                                encoder,
                                setVertexTexture: metal_cx.fallback_texture
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

    /// Returns false if it bailed before presenting, so a caller that already
    /// counted this frame as in flight can undo that; no handler will fire.
    pub fn draw_pass(
        &mut self,
        draw_pass_id: DrawPassId,
        metal_cx: &mut MetalCx,
        mode: DrawPassMode,
    ) -> bool {
        // PerfMonitor "draw" channel: CPU-side pass encode (all passes of a
        // frame sum), separate from the nextDrawable wait timed by the caller.
        let perf_t0 = self
            .perf_monitor
            .enabled()
            .then(std::time::Instant::now);
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
            return false;
        };

        let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };

        let render_pass_descriptor: ObjcId = if let DrawPassMode::MTKView(view) = &mode {
            let descriptor: ObjcId = unsafe { msg_send![*view, currentRenderPassDescriptor] };
            if descriptor == nil {
                let () = unsafe { msg_send![pool, release] };
                return false;
            }
            descriptor
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

        if !self.passes[draw_pass_id].keep_camera_matrix {
            self.passes[draw_pass_id].set_ortho_matrix(pass_rect.pos, pass_rect.size);
        }

        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            if !matches!(&mode, DrawPassMode::MTKView(_)) {
                self.passes[draw_pass_id].paint_dirty = false;
            }
            let () = unsafe { msg_send![pool, release] };
            return false;
        }

        self.passes[draw_pass_id].paint_dirty = false;

        self.passes[draw_pass_id].set_dpi_factor(dpi_factor);

        if matches!(&mode, DrawPassMode::MTKView(_)) {
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
                    if let Some(cube_face) = color_texture.cube_face {
                        let () = unsafe { msg_send![color_attachment, setSlice: cube_face as u64] };
                    }
                    let () = unsafe { msg_send![color_attachment, setLevel: 0u64] };
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
                let desc = RcObjcId::from_owned(
                    NonNull::new(unsafe {
                        msg_send![class!(MTLDepthStencilDescriptor), new]
                    })
                    .unwrap(),
                );
                let () = unsafe {
                    msg_send![desc.as_id(), setDepthCompareFunction: MTLCompareFunction::LessEqual]
                };
                let () = unsafe { msg_send![desc.as_id(), setDepthWriteEnabled: true] };
                let depth_stencil_state: ObjcId =
                    unsafe { msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc.as_id()] };
                self.passes[draw_pass_id].os.mtl_depth_state_write =
                    NonNull::new(depth_stencil_state).map(RcObjcId::from_owned);
            }
            if self.passes[draw_pass_id]
                .os
                .mtl_depth_state_no_write
                .is_none()
            {
                let desc = RcObjcId::from_owned(
                    NonNull::new(unsafe {
                        msg_send![class!(MTLDepthStencilDescriptor), new]
                    })
                    .unwrap(),
                );
                let () = unsafe {
                    msg_send![desc.as_id(), setDepthCompareFunction: MTLCompareFunction::LessEqual]
                };
                let () = unsafe { msg_send![desc.as_id(), setDepthWriteEnabled: false] };
                let depth_stencil_state: ObjcId =
                    unsafe { msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc.as_id()] };
                self.passes[draw_pass_id].os.mtl_depth_state_no_write =
                    NonNull::new(depth_stencil_state).map(RcObjcId::from_owned);
            }
        }

        // Frame batching: offscreen texture passes share one retained
        // command buffer; window modes flush it. Profiling mode keeps the
        // one-buffer-per-pass behavior so per-pass GPU spans stay real.
        // Frame batching regressed badly at large window sizes (3fps —
        // suspicion: hazard-serialized encoders on one buffer defeating
        // per-pass parallelism). Opt-in until understood.
        static BATCH_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let batch_enabled =
            *BATCH_ON.get_or_init(|| std::env::var_os("MAKEPAD_BATCH_PASSES").is_some());
        let batch_this_pass = batch_enabled
            && !Self::gpu_profile_enabled()
            && matches!(mode, DrawPassMode::Texture);
        if !batch_this_pass {
            // Entering a present-bound pass: commit the batched offscreen
            // work NOW so the GPU pipelines it under this pass's CPU encode.
            if let Some(shared) = metal_cx.frame_command_buffer.take() {
                metal_cb_committed(shared);
                let () = unsafe { msg_send![shared, commit] };
                let () = unsafe { msg_send![shared, release] };
            }
        }
        let command_buffer: ObjcId = if batch_this_pass {
            if let Some(buffer) = metal_cx.frame_command_buffer {
                metal_cx.current_cb_seq = metal_cx.frame_command_buffer_seq;
                buffer
            } else {
                let buffer = metal_cx.new_command_buffer();
                let buffer: ObjcId = unsafe { msg_send![buffer, retain] };
                metal_cx.frame_command_buffer = Some(buffer);
                metal_cx.frame_command_buffer_seq = metal_cx.current_cb_seq;
                buffer
            }
        } else {
            metal_cx.new_command_buffer()
        };
        // CPU->GPU uploads for the Vec textures this pass samples go on THIS
        // command buffer, ahead of its render encoder, so the GPU orders them
        // after every earlier reader and before this pass (`VecUploadEncoder`).
        let texture_bytes =
            self.encode_vec_texture_uploads(metal_cx, draw_list_id, command_buffer);
        self.os.texture_bytes_uploaded = self
            .os
            .texture_bytes_uploaded
            .saturating_add(texture_bytes);
        let encoder: ObjcId = unsafe {
            msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]
        };

        if let Some(depth_state) = self.passes[draw_pass_id]
            .os
            .mtl_depth_state_write
            .as_ref()
        {
            let () = unsafe { msg_send![encoder, setDepthStencilState: depth_state.as_id()] };
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
        metal_cx.register_pass(draw_pass_id, &self.passes[draw_pass_id].debug_name);
        let gpu_profile_label = Self::gpu_profile_enabled().then(|| {
            let name = &self.passes[draw_pass_id].debug_name;
            if name.is_empty() {
                format!("pass{:?}", draw_pass_id)
            } else {
                name.clone()
            }
        });
        let gpu_time_query = self.passes[draw_pass_id].gpu_time_query.clone();
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
        // RENDERER-OWNED TEXTURE CAPTURE: a capture requested for a texture
        // THIS pass renders is blitted on this very command buffer — the
        // producing queue — and delivered only from its completion handler,
        // so the bytes provably follow the render (see
        // `Cx::request_render_texture_capture`).
        self.encode_render_texture_captures(metal_cx, draw_pass_id, command_buffer);
        // Which window this pass presents to, so a `--remote` grab can target one
        // window in a multi-window app instead of whichever pass presents first.
        let pass_window_id = self.get_pass_window_id(draw_pass_id).map(|w| w.id());
        let gpu_frame_group_key = self.get_pass_window_id(draw_pass_id).map(|window_id| {
            // Group GPU timing by (window, repaint_id) so we don't merge ranges
            // across multiple frames that happen to complete out-of-order.
            (window_id.id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ self.repaint_id
        });

        // MAKEPAD_GPU_PASS_TRACE=1: log each pass's GPU time to stderr —
        // per-pass breakdown for chasing frame-budget overruns.
        if std::env::var_os("MAKEPAD_GPU_PASS_TRACE").is_some() {
            let name = if self.passes[draw_pass_id].debug_name.is_empty() {
                "main".to_string()
            } else {
                self.passes[draw_pass_id].debug_name.clone()
            };
            let () = unsafe {
                msg_send![
                    command_buffer,
                    addCompletedHandler: &objc_block!(move | command_buffer: ObjcId | {
                        let start: f64 = unsafe { msg_send![command_buffer, GPUStartTime] };
                        let end: f64 = unsafe { msg_send![command_buffer, GPUEndTime] };
                        eprintln!("[gpu-pass] {} {:.3}ms", name, (end - start) * 1000.0);
                    })
                ]
            };
        }
        if let Some(query) = gpu_time_query {
            // The tag identifies what was ENCODED into this buffer; capture
            // it now — by completion time the owner may have retagged the
            // pass for a later frame.
            let tag = query.current_tag();
            let () = unsafe {
                msg_send![
                    command_buffer,
                    addCompletedHandler: &objc_block!(move |command_buffer: ObjcId| {
                        let start: f64 = unsafe { msg_send![command_buffer, GPUStartTime] };
                        let end: f64 = unsafe { msg_send![command_buffer, GPUEndTime] };
                        query.record_seconds_tagged(tag, end - start);
                    })
                ]
            };
        }

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
                    pass_window_id,
                );
                self.commit_command_buffer(
                    screenshot,
                    None,
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    gpu_profile_label.clone(),
                    command_buffer,
                );
            }
            DrawPassMode::Texture => {
                if !batch_this_pass {
                    self.commit_command_buffer(
                        None,
                        None,
                        gpu_frame_group_key,
                        false,
                        gpu_counters,
                        gpu_profile_label.clone(),
                        command_buffer,
                    );
                }
                // Batched: encoder already ended; the shared buffer commits
                // with the window pass.
            }
            DrawPassMode::StdinTexture => {
                self.commit_command_buffer(
                    None,
                    None,
                    gpu_frame_group_key,
                    false,
                    gpu_counters,
                    gpu_profile_label.clone(),
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
                        pass_window_id,
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
                    gpu_profile_label.clone(),
                    command_buffer,
                );
            }
            DrawPassMode::Drawable(drawable, target_presentation_time) => {
                let first_texture: ObjcId = unsafe { msg_send![drawable, texture] };
                if let Some(target_presentation_time) = target_presentation_time {
                    let () = unsafe {
                        msg_send![
                            command_buffer,
                            presentDrawable: drawable
                            atTime: target_presentation_time
                        ]
                    };
                } else {
                    let () = unsafe { msg_send![command_buffer, presentDrawable: drawable] };
                }
                let screenshot = self.build_screenshot_struct(
                    metal_cx,
                    command_buffer,
                    0,
                    pass_width as usize,
                    pass_height as usize,
                    first_texture,
                    None,
                    pass_window_id,
                );
                self.commit_command_buffer(
                    screenshot,
                    None,
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    gpu_profile_label.clone(),
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
                    pass_window_id,
                );
                self.commit_command_buffer(
                    screenshot,
                    None,
                    gpu_frame_group_key,
                    true,
                    gpu_counters,
                    gpu_profile_label.clone(),
                    command_buffer,
                );
                let () = unsafe { msg_send![command_buffer, waitUntilScheduled] };
                let () = unsafe { msg_send![drawable, present] };
            }
        }
        let () = unsafe { msg_send![pool, release] };
        if let Some(t0) = perf_t0 {
            self.perf_monitor.add(
                crate::perf_monitor::PERF_CHANNEL_DRAW,
                t0.elapsed().as_micros() as u64,
            );
        }
        true
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
        window_id: Option<usize>,
    ) -> Option<ScreenshotInfo> {
        let request_ids =
            self.take_studio_screenshot_request_ids_for_window(kind_id as u32, window_id);
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
            let texture = RcObjcId::from_owned(
                NonNull::new(unsafe {
                    msg_send![metal_cx.device, newTextureWithDescriptor: descriptor.as_id()]
                })
                .unwrap(),
            );
            unsafe {
                let blit_encoder: ObjcId = msg_send![command_buffer, blitCommandEncoder];
                let () = msg_send![blit_encoder, copyFromTexture: in_texture toTexture:texture.as_id()];
                let () = msg_send![blit_encoder, synchronizeTexture: texture.as_id() slice:0 level:0];
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

    fn gpu_profile_enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("MAKEPAD_GPU_PROFILE").is_some())
    }

    fn commit_command_buffer(
        &self,
        screenshot_info: Option<ScreenshotInfo>,
        stdin_frame: Option<PresentableDraw>,
        gpu_frame_group_key: Option<u64>,
        flush_gpu_frame_group: bool,
        gpu_counters: GpuSampleCounters,
        gpu_profile_label: Option<String>,
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
                    if let Some(sf) = screenshot_info.lock().unwrap().take(){
                        let mut bgra = vec![0u8; sf.width * sf.height * 4];
                        let region = MTLRegion {
                            origin: MTLOrigin {x: 0, y: 0, z: 0},
                            size: MTLSize {width: sf.width as u64, height: sf.height as u64, depth: 1}
                        };
                        let _:() = unsafe{msg_send![
                            sf.texture.as_id(),
                            getBytes: bgra.as_mut_ptr()
                            bytesPerRow: sf.width *4
                            bytesPerImage: sf.width * sf.height * 4
                            fromRegion: region
                            mipmapLevel: 0
                            slice: 0
                        ]};
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
                            sf.request_ids,
                            sf.width as _,
                            sf.height as _,
                            png,
                        );
                    }

                    let raw_start: f64 = unsafe { msg_send![command_buffer, GPUStartTime] };
                    let raw_end: f64 = unsafe { msg_send![command_buffer, GPUEndTime] };
                    if let Some(label) = &gpu_profile_label {
                        gpu_profile_accumulate(label, raw_end - raw_start, &gpu_counters);
                    }
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
                        // PerfMonitor "gpu" channel: one presented frame's
                        // aggregated GPU interval (no-op unless enabled).
                        crate::perf_monitor::perf_gpu_frame_completed(
                            raw_sample_end - raw_sample_start,
                        );
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
        metal_cb_committed(command_buffer);
        let () = unsafe { msg_send![command_buffer, commit] };
    }

    pub(crate) fn mtl_compile_shaders(&mut self, metal_cx: &MetalCx) {
        let _mp_batch = (
            crate::startup_trace_enabled(),
            self.draw_shaders.compile_set.len(),
            std::time::Instant::now(),
        );
        if _mp_batch.0 && _mp_batch.1 > 0 {
            crate::startup_trace(&format!("mtl_compile_shaders begin ({})", _mp_batch.1));
        }
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
                if let Some(shp) =
                    CxOsDrawShader::new(metal_cx, mtlsl, &cx_shader.mapping, &bindings)
                {
                    cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                    self.draw_shaders.os_shaders.push(shp);
                }
            }
        }
        self.draw_shaders.compile_set.clear();
        if _mp_batch.0 && _mp_batch.1 > 0 {
            crate::startup_trace(&format!(
                "mtl_compile_shaders done ({} in {:.2} ms)",
                _mp_batch.1,
                _mp_batch.2.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }

    #[cfg(target_os = "macos")]
    pub fn share_texture_for_presentable_image(&mut self, texture: &Texture) -> u32 {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.metal_device.unwrap())
    }

    #[cfg(target_os = "ios")]
    pub fn share_texture_for_presentable_image(&mut self, texture: &Texture) -> u32 {
        let cxtexture = &mut self.textures[texture.texture_id()];
        let device = crate::os::apple::ios::ios_app::with_ios_app(|app| app.metal_device());
        cxtexture.update_shared_texture(device)
    }

    #[cfg(target_os = "tvos")]
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

    #[cfg(target_os = "ios")]
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
        let device = crate::os::apple::ios::ios_app::with_ios_app(|app| app.metal_device());
        let iosurface_id = cxtexture.update_shared_texture(device);
        let iosurface_ref = cxtexture.os.iosurface.unwrap_or(std::ptr::null_mut());
        (texture, iosurface_ref, iosurface_id)
    }
}

struct ScreenshotInfo {
    width: usize,
    height: usize,
    request_ids: Vec<u64>,
    texture: RcObjcId,
}

pub enum DrawPassMode {
    Texture,
    StdinTexture,
    MTKView(ObjcId),
    StdinMain(PresentableDraw, usize),
    /// Optional Core Animation media time is supplied by
    /// CAMetalDisplayLinkUpdate.targetPresentationTimestamp.
    Drawable(ObjcId, Option<f64>),
    Resizing(ObjcId),
}

impl DrawPassMode {
    fn is_drawable(&self) -> Option<ObjcId> {
        match self {
            Self::Drawable(obj, _) | Self::Resizing(obj) => Some(*obj),
            Self::StdinMain(_, _) | Self::Texture | Self::StdinTexture | Self::MTKView(_) => None,
        }
    }
}

pub struct MetalCx {
    pub device: ObjcId,
    command_queue: ObjcId,
    /// 1×1 BGRA fallback texture bound when a texture slot has no backing
    /// MTLTexture. Prevents Metal command-buffer aborts on iOS where sampling
    /// from nil is a GPU fault.
    fallback_texture: ObjcId,
    /// Frame-batched command buffer: offscreen texture passes append their
    /// encoders here instead of committing one buffer each — a 12-pass
    /// blur pyramid was paying ~1ms commit/schedule latency PER PASS. The
    /// final window pass presents and commits it. Retained (see retain in
    /// draw_pass); None outside a frame or when MAKEPAD_GPU_PROFILE=1
    /// (profiling keeps per-pass buffers for per-pass GPU spans).
    pub frame_command_buffer: Option<ObjcId>,
    /// `cb_seq` of `frame_command_buffer`, restored into `current_cb_seq`
    /// when a batched pass appends to it.
    frame_command_buffer_seq: u64,
    /// Monotonic id handed to every command buffer this context creates
    /// (`new_command_buffer`); 0 = none yet. Each buffer's completion
    /// handler publishes its id into `METAL_CB_COMPLETED`, which is how a
    /// CPU-written resource learns "the GPU is done with the last thing
    /// that read me" without a stall.
    cb_seq: u64,
    /// The id of the command buffer the pass being encoded goes into.
    current_cb_seq: u64,
    /// Free-list of Shared staging buffers for Vec-texture uploads
    /// (`VecUploadEncoder`), shared with the completion handlers that hand
    /// buffers back once their blit has executed.
    staging_pool: Arc<Mutex<Vec<StagingBuffer>>>,
    /// Shaders drawn by the pass being encoded (`render_view` collects
    /// them, `draw_pass` hands them to the in-flight registry) — what the
    /// hang diagnostic and `MAKEPAD_GPU_TRACE` name.
    pass_shaders: RefCell<Vec<LiveId>>,
    /// The last command-buffer seq of each recent repaint, oldest first —
    /// the unit of the frame-level GPU backpressure (`frames_in_flight`).
    repaint_tail_seqs: VecDeque<u64>,
    /// Repaints skipped by that backpressure (diagnostics).
    pub(crate) backpressure_skips: u64,
    /// Once-per-second cadence for the opt-in staging/command-buffer counters.
    memory_trace_at: Instant,
}

/// Highest `MetalCx::cb_seq` whose command buffer has COMPLETED. One
/// command queue executes its buffers in enqueue order (Metal: a committed
/// buffer "is executed after any previously enqueued command buffers"), so
/// completion of N implies completion of everything numbered below it.
static METAL_CB_COMPLETED: AtomicU64 = AtomicU64::new(0);

/// One in-flight command buffer as the hang watchdog and `MAKEPAD_GPU_TRACE`
/// see it. Entries are born in `new_command_buffer`, filled by `draw_pass`,
/// stamped at commit, and removed by the buffer's completion handler.
struct InFlightCb {
    seq: u64,
    /// The MTLCommandBuffer, compared only as an address (commit sites find
    /// their entry by it; Metal retains the object until it completes, so
    /// the address cannot be reused while the entry lives).
    buffer: usize,
    committed_at: Option<Instant>,
    passes: Vec<InFlightPass>,
}

struct InFlightPass {
    pass_id: DrawPassId,
    name: String,
    shaders: Vec<LiveId>,
}

static METAL_IN_FLIGHT: Mutex<VecDeque<InFlightCb>> = Mutex::new(VecDeque::new());

fn metal_in_flight() -> std::sync::MutexGuard<'static, VecDeque<InFlightCb>> {
    METAL_IN_FLIGHT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn describe_passes(passes: &[InFlightPass]) -> String {
    if passes.is_empty() {
        return "(no draw pass recorded)".to_string();
    }
    let mut out = String::new();
    for pass in passes {
        let shaders: Vec<String> = pass.shaders.iter().map(|id| format!("{:?}", id)).collect();
        let _ = write!(
            out,
            "{:?} \"{}\" shaders [{}]; ",
            pass.pass_id,
            if pass.name.is_empty() { "main" } else { &pass.name },
            shaders.join(", ")
        );
    }
    out
}

/// Commit-site hook: the watchdog's clock for a buffer starts here. The
/// OLDEST uncompleted buffer has nothing queued ahead of it, so its age is
/// what the GPU is actually spending on it.
pub(crate) fn metal_cb_committed(buffer: ObjcId) {
    let now = Instant::now();
    if let Some(entry) = metal_in_flight()
        .iter_mut()
        .rev()
        .find(|entry| entry.buffer == buffer as usize)
    {
        entry.committed_at = Some(now);
    }
}

/// `MAKEPAD_GPU_MAX_CB_MS`: a command buffer older than this without
/// completing aborts the process (default 1500; 0 disables the watchdog —
/// e.g. for a deliberate multi-second bake).
fn gpu_hang_max_ms() -> u64 {
    static MAX: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("MAKEPAD_GPU_MAX_CB_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(5000)
    })
}

/// `MAKEPAD_GPU_TRACE=1` logs every command buffer whose GPU time exceeds
/// 4 ms (`=N` sets the threshold in ms) with its passes and shaders, plus
/// once-per-second staging, queue, and backpressure counters.
pub(crate) fn gpu_trace_threshold_ms() -> Option<f64> {
    static T: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        let v = std::env::var("MAKEPAD_GPU_TRACE").ok()?;
        let v = v.trim();
        match v.parse::<f64>() {
            Ok(ms) if ms > 1.0 => Some(ms),
            _ => Some(4.0),
        }
    })
}

/// GPU-HANG SELF-TERMINATION. A runaway shader keeps its command buffer
/// from ever completing; macOS's GPU watchdog then resets the DRIVER (the
/// user's whole desktop, tonight: two freezes and a reboot). This thread
/// watches the oldest committed-but-uncompleted command buffer and, once it
/// is older than `MAKEPAD_GPU_MAX_CB_MS`, writes a diagnostic naming the
/// passes and shaders in that buffer (stderr, and the file named by
/// `MAKEPAD_GPU_HANG_DUMP` if set) and aborts THIS process — the kernel
/// tears down our GPU context long before the driver-level watchdog fires.
/// A thread, not a per-frame check: a hung GPU also stalls the display
/// link, so the main loop may never get another beat.
fn metal_hang_watchdog_start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let max_ms = gpu_hang_max_ms();
        if max_ms == 0 {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("metal-hang-watchdog".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(100));
                let completed = METAL_CB_COMPLETED.load(Ordering::Acquire);
                let diagnostic = {
                    let mut queue = metal_in_flight();
                    while queue.front().map_or(false, |entry| entry.seq <= completed) {
                        queue.pop_front();
                    }
                    let Some(oldest) = queue.iter().find(|entry| entry.committed_at.is_some())
                    else {
                        continue;
                    };
                    let age = oldest.committed_at.unwrap().elapsed();
                    if age.as_millis() as u64 <= max_ms {
                        continue;
                    }
                    format!(
                        "[metal-hang] command buffer #{} committed {} ms ago has not completed \
                         (limit MAKEPAD_GPU_MAX_CB_MS={}, {} buffers in flight). A runaway shader \
                         in one of its passes, or the GPU starved by another process. Passes: {} \
                         Aborting this process before the OS GPU watchdog resets the driver.",
                        oldest.seq,
                        age.as_millis(),
                        max_ms,
                        queue.len(),
                        describe_passes(&oldest.passes),
                    )
                };
                eprintln!("{}", diagnostic);
                if let Some(path) = std::env::var_os("MAKEPAD_GPU_HANG_DUMP") {
                    use std::io::Write as _;
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                    {
                        let _ = writeln!(
                            file,
                            "{} pid={} exe={:?}\n{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0),
                            std::process::id(),
                            std::env::current_exe().unwrap_or_default(),
                            diagnostic
                        );
                    }
                }
                // Abort is OPT-IN (`MAKEPAD_GPU_HANG_ABORT=1`): a customer-facing
                // app must never quit itself on a stall it did not cause (a
                // starved GPU shared with another process looks identical).
                // Without it the diagnostic is logged and the stall is
                // re-checked after a pause instead of re-reported every tick.
                if std::env::var("MAKEPAD_GPU_HANG_ABORT").map(|v| v == "1").unwrap_or(false) {
                    std::process::abort();
                } else {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            });
    });
}

/// A Shared `MTLBuffer` carrying one Vec-texture upload from the CPU to a
/// blit. Whoever holds the struct owns the retain.
struct StagingBuffer {
    buffer: ObjcId,
    len: usize,
}
// It travels main thread -> completion handler -> main thread only through
// the pool mutex, and Metal objects are safe to use from any thread.
unsafe impl Send for StagingBuffer {}

static STAGING_LIVE_COUNT: AtomicUsize = AtomicUsize::new(0);
static STAGING_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static STAGING_USED_COUNT: AtomicUsize = AtomicUsize::new(0);
static STAGING_USED_BYTES: AtomicUsize = AtomicUsize::new(0);

impl StagingBuffer {
    fn from_owned(buffer: ObjcId, len: usize) -> Self {
        STAGING_LIVE_COUNT.fetch_add(1, Ordering::Relaxed);
        STAGING_LIVE_BYTES.fetch_add(len, Ordering::Relaxed);
        Self { buffer, len }
    }
}

impl Drop for StagingBuffer {
    fn drop(&mut self) {
        STAGING_LIVE_COUNT.fetch_sub(1, Ordering::Relaxed);
        STAGING_LIVE_BYTES.fetch_sub(self.len, Ordering::Relaxed);
        let () = unsafe { msg_send![self.buffer, release] };
    }
}

/// Staging buffers retained by a command-buffer completion block. Its Drop
/// path is just as important as the normal callback: Metal may discard an
/// uncommitted command buffer, in which case the block is destroyed without
/// ever running and the buffers must still be released.
struct UsedStagingBuffers {
    buffers: Option<Vec<StagingBuffer>>,
    count: usize,
    bytes: usize,
}

impl UsedStagingBuffers {
    fn new(buffers: Vec<StagingBuffer>) -> Self {
        let count = buffers.len();
        let bytes = buffers.iter().map(|buffer| buffer.len).sum();
        STAGING_USED_COUNT.fetch_add(count, Ordering::Relaxed);
        STAGING_USED_BYTES.fetch_add(bytes, Ordering::Relaxed);
        Self {
            buffers: Some(buffers),
            count,
            bytes,
        }
    }

    fn untrack(&mut self) {
        if self.count != 0 {
            STAGING_USED_COUNT.fetch_sub(self.count, Ordering::Relaxed);
            STAGING_USED_BYTES.fetch_sub(self.bytes, Ordering::Relaxed);
            self.count = 0;
            self.bytes = 0;
        }
    }

    fn return_to_pool(mut self, pool: &Mutex<Vec<StagingBuffer>>) {
        self.untrack();
        if let Some(buffers) = self.buffers.take() {
            staging_pool_return(pool, buffers);
        }
    }
}

impl Drop for UsedStagingBuffers {
    fn drop(&mut self) {
        self.untrack();
    }
}

/// Past these the pool releases a returning buffer instead of keeping it:
/// enough for a few frames of NV12 planes + flow fields in flight, not a
/// place for a one-off 16K upload to live forever.
const STAGING_POOL_MAX_COUNT: usize = 32;
const STAGING_POOL_MAX_BYTES: usize = 192 << 20;

fn staging_pool_return(pool: &Mutex<Vec<StagingBuffer>>, used: Vec<StagingBuffer>) {
    let mut pool = pool.lock().unwrap();
    let mut bytes: usize = pool.iter().map(|b| b.len).sum();
    for staging in used {
        if pool.len() < STAGING_POOL_MAX_COUNT && bytes + staging.len <= STAGING_POOL_MAX_BYTES {
            bytes += staging.len;
            pool.push(staging);
        } else {
            drop(staging);
        }
    }
}

/// One pass's Vec-texture uploads. Every `replaceRegion` the backend used
/// to do at bind time — on the main thread, while the render encoder was
/// open, with no knowledge of the in-flight command buffers still sampling
/// the texture — is now a memcpy into a Shared staging buffer plus a
/// `copyFromBuffer:toTexture:` blit on the pass's OWN command buffer,
/// encoded before its render encoder opens. Vec textures are Private, so
/// hazard tracking orders every earlier reader -> this blit -> this pass's
/// reads, with zero CPU stalls (the layout the audit's raw `mtlrace --mode
/// blit` probe proved clean at 3 frames in flight). The blit encoder opens
/// lazily (most passes upload nothing) and the staging buffers return to
/// the pool from the command buffer's completion handler.
struct VecUploadEncoder {
    command_buffer: ObjcId,
    blit: Option<ObjcId>,
    used: Vec<StagingBuffer>,
    bytes: u64,
}

impl VecUploadEncoder {
    fn new(command_buffer: ObjcId) -> Self {
        Self {
            command_buffer,
            blit: None,
            used: Vec::new(),
            bytes: 0,
        }
    }

    fn blit(&mut self) -> ObjcId {
        let command_buffer = self.command_buffer;
        *self
            .blit
            .get_or_insert_with(|| unsafe { msg_send![command_buffer, blitCommandEncoder] })
    }

    /// Ends the blit encoder (a render encoder may open after this) and
    /// arranges for the staging buffers to come back once the GPU has
    /// consumed them. Returns the bytes uploaded.
    fn finish(self, metal_cx: &MetalCx) -> u64 {
        if let Some(blit) = self.blit {
            let () = unsafe { msg_send![blit, endEncoding] };
        }
        if !self.used.is_empty() {
            let pool = metal_cx.staging_pool.clone();
            let used = Mutex::new(Some(UsedStagingBuffers::new(self.used)));
            let () = unsafe {
                msg_send![
                    self.command_buffer,
                    addCompletedHandler: &objc_block!(move |_cb: ObjcId| {
                        if let Some(used) = used.lock().unwrap().take() {
                            used.return_to_pool(&pool);
                        }
                    })
                ]
            };
        }
        self.bytes
    }
}

impl MetalCx {
    /// Every command buffer of a frame is born here so it carries a
    /// sequence id and the completion handler that publishes it.
    fn new_command_buffer(&mut self) -> ObjcId {
        metal_hang_watchdog_start();
        let buffer: ObjcId = unsafe { msg_send![self.command_queue, commandBuffer] };
        self.cb_seq += 1;
        let seq = self.cb_seq;
        self.current_cb_seq = seq;
        metal_in_flight().push_back(InFlightCb {
            seq,
            buffer: buffer as usize,
            committed_at: None,
            passes: Vec::new(),
        });
        let () = unsafe {
            msg_send![
                buffer,
                addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                    METAL_CB_COMPLETED.fetch_max(seq, Ordering::AcqRel);
                    let entry = {
                        let mut queue = metal_in_flight();
                        queue
                            .iter()
                            .position(|entry| entry.seq == seq)
                            .and_then(|at| queue.remove(at))
                    };
                    if let (Some(threshold), Some(entry)) = (gpu_trace_threshold_ms(), entry) {
                        let start: f64 = unsafe { msg_send![cb, GPUStartTime] };
                        let end: f64 = unsafe { msg_send![cb, GPUEndTime] };
                        let ms = (end - start) * 1000.0;
                        if ms.is_finite() && ms > threshold {
                            eprintln!(
                                "[gpu-trace] command buffer #{} gpu {:.2} ms: {}",
                                seq,
                                ms,
                                describe_passes(&entry.passes)
                            );
                        }
                    }
                })
            ]
        };
        buffer
    }

    /// Frame-level GPU backpressure, the twin of the per-window present
    /// gate for frames that present nothing (hidden windows, offscreen-only
    /// selftests — which is how those piled up GPU work unboundedly).
    /// Called at the top of every repaint: records the previous repaint's
    /// last command buffer, so `frames_in_flight` counts repaints the GPU
    /// has not finished.
    pub(crate) fn begin_repaint(&mut self) {
        if self.cb_seq > 0 && self.repaint_tail_seqs.back() != Some(&self.cb_seq) {
            self.repaint_tail_seqs.push_back(self.cb_seq);
            while self.repaint_tail_seqs.len() > 16 {
                self.repaint_tail_seqs.pop_front();
            }
        }
    }

    /// Repaints whose command buffers have not all completed.
    pub(crate) fn frames_in_flight(&self) -> usize {
        let completed = METAL_CB_COMPLETED.load(Ordering::Acquire);
        self.repaint_tail_seqs
            .iter()
            .filter(|seq| **seq > completed)
            .count()
    }

    /// `MAKEPAD_GPU_TRACE=1`: report the allocations whose lifetime follows
    /// command-buffer completion. The pool is bounded, while `used` identifies
    /// work waiting on the GPU; growth there is queue growth, not pool growth.
    pub(crate) fn trace_memory_once_per_second(&mut self) {
        if gpu_trace_threshold_ms().is_none() || self.memory_trace_at.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.memory_trace_at = Instant::now();
        let (pool_count, pool_bytes) = {
            let pool = self
                .staging_pool
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (pool.len(), pool.iter().map(|buffer| buffer.len).sum::<usize>())
        };
        let live_count = STAGING_LIVE_COUNT.load(Ordering::Relaxed);
        let live_bytes = STAGING_LIVE_BYTES.load(Ordering::Relaxed);
        let used_count = STAGING_USED_COUNT.load(Ordering::Relaxed);
        let used_bytes = STAGING_USED_BYTES.load(Ordering::Relaxed);
        let encoding_count = live_count.saturating_sub(pool_count.saturating_add(used_count));
        let command_buffers = metal_in_flight().len();
        eprintln!(
            "[gpu-memory] staging_live={} staging_used={} staging_pool={} staging_encoding={} total_bytes={} used_bytes={} pool_bytes={} command_buffers={} frames={} backpressure_skips={}",
            live_count,
            used_count,
            pool_count,
            encoding_count,
            live_bytes,
            used_bytes,
            pool_bytes,
            command_buffers,
            self.frames_in_flight(),
            self.backpressure_skips,
        );
    }

    /// Record a pass just encoded into the buffer being built, for the hang
    /// diagnostic and the GPU trace.
    fn register_pass(&self, pass_id: DrawPassId, name: &str) {
        let shaders = self.pass_shaders.take();
        let seq = self.current_cb_seq;
        if let Some(entry) = metal_in_flight()
            .iter_mut()
            .rev()
            .find(|entry| entry.seq == seq)
        {
            entry.passes.push(InFlightPass {
                pass_id,
                name: name.to_string(),
                shaders,
            });
        }
    }

    /// The smallest pooled staging buffer that fits `len`, or a fresh one.
    /// Fresh buffers round up to 64 KiB so sizes cluster and a returned
    /// buffer fits the next upload of the same texture.
    fn take_staging(&self, len: usize) -> Option<StagingBuffer> {
        {
            let mut pool = self.staging_pool.lock().unwrap();
            let mut best: Option<usize> = None;
            for (index, staging) in pool.iter().enumerate() {
                if staging.len >= len && best.map_or(true, |b| pool[b].len > staging.len) {
                    best = Some(index);
                }
            }
            if let Some(index) = best {
                return Some(pool.swap_remove(index));
            }
        }
        let alloc = (len + 0xffff) & !0xffff;
        let buffer: ObjcId = unsafe {
            msg_send![
                self.device,
                newBufferWithLength: alloc as u64
                options: MTLResourceOptions::StorageModeShared
            ]
        };
        if buffer == nil {
            return None;
        }
        Some(StagingBuffer::from_owned(buffer, alloc))
    }
}

impl Cx {
    /// Walk the draw lists this pass renders and upload every Vec texture
    /// they bind that has pending data (allocating its Private MTLTexture
    /// on first sight). Called from `draw_pass` after the command buffer
    /// exists and before its render encoder opens.
    fn encode_vec_texture_uploads(
        &mut self,
        metal_cx: &MetalCx,
        draw_list_id: DrawListId,
        command_buffer: ObjcId,
    ) -> u64 {
        let mut enc = VecUploadEncoder::new(command_buffer);
        let mut stack: Vec<DrawListId> = vec![draw_list_id];
        while let Some(list_id) = stack.pop() {
            let draw_list = &self.draw_lists[list_id];
            for order_index in 0..draw_list.draw_item_order_len() {
                let Some(item_id) = draw_list.draw_item_id_at_order_index(order_index) else {
                    continue;
                };
                match &draw_list.draw_items[item_id].kind {
                    CxDrawKind::SubList(sub_list_id) => stack.push(*sub_list_id),
                    CxDrawKind::DrawCall(draw_call) => {
                        for texture in draw_call.texture_slots.iter().flatten() {
                            let cxtexture = &mut self.textures[texture.texture_id()];
                            // A size change always arrives with new data, so
                            // "nothing pending and already allocated" is the
                            // whole fast path — no alloc compare per bind.
                            if cxtexture.format.is_vec()
                                && (cxtexture.os.texture.is_none()
                                    || !cxtexture.updated().is_empty())
                            {
                                cxtexture.update_vec_texture(metal_cx, &mut enc);
                            }
                        }
                    }
                    CxDrawKind::Empty => {}
                }
            }
        }
        enc.finish(metal_cx)
    }
}

#[derive(Clone, Default)]
pub struct CxOsDrawList {}

#[derive(Default, Clone)]
pub struct CxOsPass {
    mtl_depth_state_write: Option<RcObjcId>,
    mtl_depth_state_no_write: Option<RcObjcId>,
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
        let fallback_texture = unsafe {
            let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
            let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
            let _: () = msg_send![descriptor, setWidth: 1u64];
            let _: () = msg_send![descriptor, setHeight: 1u64];
            let _: () = msg_send![descriptor, setDepth: 1u64];
            let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
            let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
            let tex: ObjcId = msg_send![device, newTextureWithDescriptor: descriptor];
            let _: () = msg_send![descriptor, release];
            // Write transparent black pixel
            let zero: [u8; 4] = [0, 0, 0, 0];
            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
            };
            let _: () = msg_send![
                tex,
                replaceRegion: region
                mipmapLevel: 0u64
                withBytes: zero.as_ptr() as *const std::ffi::c_void
                bytesPerRow: 4u64
            ];
            tex
        };
        MetalCx {
            command_queue: unsafe { msg_send![device, newCommandQueue] },
            device,
            fallback_texture,
            frame_command_buffer: None,
            frame_command_buffer_seq: 0,
            cb_seq: 0,
            current_cb_seq: 0,
            staging_pool: Arc::new(Mutex::new(Vec::new())),
            pass_shaders: RefCell::new(Vec::new()),
            repaint_tail_seqs: VecDeque::new(),
            backpressure_skips: 0,
            memory_trace_at: Instant::now(),
        }
    }
}

impl Drop for MetalCx {
    fn drop(&mut self) {
        if let Some(buffer) = self.frame_command_buffer.take() {
            metal_in_flight().retain(|entry| entry.buffer != buffer as usize);
            let () = unsafe { msg_send![buffer, release] };
        }
        unsafe {
            let () = msg_send![self.fallback_texture, release];
            let () = msg_send![self.command_queue, release];
            let () = msg_send![self.device, release];
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
    custom_uniform_buffer_ids: Vec<u64>,
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
            output.use_vulkan = false;

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
                DrawVars::log_shader_compile_failure(vm, io_self, &output);
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
            output.metal_create_helpers(&mut out);
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
        mapping: &CxDrawShaderMapping,
        bindings: &UniformBufferBindings,
    ) -> Option<Self> {
        // Generated shader source is what an author — increasingly an AI —
        // actually has to debug, and it is otherwise invisible. Dumping it is
        // opt-in and costs nothing when the var is unset.
        if let Ok(dir) = std::env::var("MAKEPAD_SHADER_DUMP") {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&mtlsl, &mut hasher);
            let name = format!("{}/shader_{:016x}.metal", dir, std::hash::Hasher::finish(&hasher));
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&name, mtlsl.as_bytes());
        }
        let _mp_t0 = std::time::Instant::now();
        let _mp_src_len = mtlsl.len();
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

        let _mp_lib_ms = _mp_t0.elapsed().as_secs_f64() * 1000.0;
        let _mp_t1 = std::time::Instant::now();
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
            match mapping.color_format {
                crate::draw_shader::DrawShaderColorFormat::Bgra8Unorm => {
                    let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                    let () = msg_send![color_attachment, setBlendingEnabled: YES];
                    let () = msg_send![color_attachment, setRgbBlendOperation: MTLBlendOperation::Add];
                    let () = msg_send![color_attachment, setAlphaBlendOperation: MTLBlendOperation::Add];
                    let () = msg_send![color_attachment, setSourceRGBBlendFactor: MTLBlendFactor::One];
                    let () = msg_send![color_attachment, setSourceAlphaBlendFactor: MTLBlendFactor::One];
                    let () = msg_send![color_attachment, setDestinationRGBBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
                    let () = msg_send![color_attachment, setDestinationAlphaBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
                }
                crate::draw_shader::DrawShaderColorFormat::Bgra8NoBlend => {
                    // Raw-write data pass: alpha is payload, and the over
                    // blend can only ever GROW dst alpha.
                    let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                    let () = msg_send![color_attachment, setBlendingEnabled: NO];
                }
                crate::draw_shader::DrawShaderColorFormat::Rf32 => {
                    // Float data target (TextureFormat::RenderRf32): no
                    // blending — these are data passes, and float-target
                    // blending is not universal across GPU families.
                    let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::R32Float];
                    let () = msg_send![color_attachment, setBlendingEnabled: NO];
                }
                crate::draw_shader::DrawShaderColorFormat::Rgba16F => {
                    // Four-channel float sim target (RenderRGBAf16); data
                    // pass, no blending (same law as Rf32).
                    let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::RGBA16Float];
                    let () = msg_send![color_attachment, setBlendingEnabled: NO];
                }
                crate::draw_shader::DrawShaderColorFormat::Rgba32F => {
                    // Four-channel float sim target (RenderRGBAf32); data
                    // pass, no blending. Consumers read with sample_nearest —
                    // Apple GPUs do not filter 32-bit float textures.
                    let () = msg_send![color_attachment, setPixelFormat: MTLPixelFormat::RGBA32Float];
                    let () = msg_send![color_attachment, setBlendingEnabled: NO];
                }
            }

            let () = msg_send![descriptor.as_id(), setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float];

            let mut error: ObjcId = nil;
            msg_send![
                metal_cx.device,
                newRenderPipelineStateWithDescriptor: descriptor
                error: &mut error
            ]
        }).unwrap());

        // Opt-in: shader compile timing is only interesting when someone is
        // measuring it, and every boot compiles dozens of shaders.
        if std::env::var("MAKEPAD_SHADER_BENCH").is_ok() {
            crate::log!("MPSHADERBENCH src={} bytes lib={:.2}ms pipeline={:.2}ms total={:.2}ms",
                _mp_src_len, _mp_lib_ms, _mp_t1.elapsed().as_secs_f64()*1000.0,
                _mp_t0.elapsed().as_secs_f64()*1000.0);
        }
        crate::startup_acc("metal newLibraryWithSource", _mp_lib_ms);
        crate::startup_acc(
            "metal newRenderPipelineState",
            _mp_t1.elapsed().as_secs_f64() * 1000.0,
        );
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
        let custom_uniform_buffer_ids = mapping
            .uniform_buffers
            .iter()
            .map(|input| input.buffer_index as u64)
            .collect();
        // scope_uniform_buffer_id comes from bindings if there are scope uniforms
        let scope_uniform_buffer_id = bindings.scope_uniform_buffer_index.map(|i| i as u64);

        return Some(Self {
            _library: library,
            render_pipeline_state,
            draw_call_uniform_buffer_id,
            pass_uniform_buffer_id,
            draw_list_uniform_buffer_id,
            dyn_uniform_buffer_id,
            custom_uniform_buffer_ids,
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
pub struct CxOsUniformBuffer {}

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
    /// Bind-time stamp: until the command buffer being encoded completes,
    /// the GPU may be reading this buffer.
    fn mark_bound(&mut self, metal_cx: &MetalCx) {
        if let Some(inner) = self.inner.as_mut() {
            inner.last_bound_seq = metal_cx.current_cb_seq;
        }
    }

    fn update<T>(&mut self, metal_cx: &MetalCx, data: &[T]) {
        let len = data.len() * std::mem::size_of::<T>();
        if len == 0 {
            self.inner = None;
            return;
        }
        if let Some(inner) = self.inner.as_mut() {
            // Writing in place is only safe once every command buffer that
            // bound this buffer has completed: the previous frame's draw may
            // still be reading it (the same law as the Vec textures, and the
            // audit's fix 2). At UI rates the GPU retired the last frame long
            // before the next update, so this is the common path; only a
            // buffer the GPU is still holding gets a fresh allocation.
            let gpu_done =
                inner.last_bound_seq <= METAL_CB_COMPLETED.load(Ordering::Acquire);
            if gpu_done && len <= inner.capacity {
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
                    inner.len = len;
                    return;
                }
            }
        }
        // A fresh buffer. The in-flight command buffers keep their own
        // retain on the old one until they complete, so dropping ours here
        // never pulls memory out from under the GPU.
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
            capacity: len,
            last_bound_seq: 0,
        });
    }
}

struct MetalBufferInner {
    buffer: RcObjcId,
    /// Bytes in use by the last `update`.
    #[allow(dead_code)]
    len: usize,
    /// Bytes allocated: a buffer shrinks in place and grows by reallocation.
    capacity: usize,
    /// `MetalCx::cb_seq` of the last command buffer this buffer was bound
    /// in (see `mark_bound`); 0 = never bound.
    last_bound_seq: u64,
}

#[derive(Default)]
pub struct CxOsTexture {
    pub(crate) texture: Option<RcObjcId>,
    /// `texture` was just (re)created by `update_vec_texture` and holds
    /// nothing yet, so the next upload goes WHOLE no matter how small the
    /// pending dirty rect is. The slug glyph atlas grows by appending rows
    /// and marks only those rows dirty — right for a texture updated in
    /// place, fatal for a fresh one: every earlier row (every earlier
    /// glyph) would never reach the GPU while the CPU believed it had.
    /// Cleared once the full blit is encoded.
    vec_fresh: bool,
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    iosurface: Option<IOSurfaceRef>,
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    iosurface_id: IOSurfaceID,
}

impl Drop for CxOsTexture {
    fn drop(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        if let Some(iosurface) = self.iosurface.take() {
            unsafe { CFRelease(iosurface) };
        }
    }
}
impl Cx {
    /// DEBUG ONLY: synchronous GPU->CPU readback of a render-target texture
    /// (private storage), via a one-off blit to a shared staging texture.
    /// Blocks until the GPU is done — strictly a diagnostics path (atlas
    /// dumps, bake verification), never per-frame work. Returns raw bytes in
    /// the texture's native layout (BGRA8 = 4 bytes/texel, R32F = 4).
    pub fn debug_read_render_texture(
        &mut self,
        texture: &crate::texture::Texture,
    ) -> Option<(usize, usize, Vec<u8>)> {
        let cxtexture = &self.textures[texture.texture_id()];
        let alloc = cxtexture.alloc.as_ref()?;
        let (width, height) = (alloc.width, alloc.height);
        let pixel = alloc.pixel.clone();
        let mtl_tex = cxtexture.os.texture.as_ref()?.as_id();
        unsafe {
            let device: ObjcId = msg_send![mtl_tex, device];
            let descriptor = RcObjcId::from_owned(NonNull::new(msg_send![
                class!(MTLTextureDescriptor),
                new
            ])?);
            let () = msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2];
            let () = msg_send![descriptor.as_id(), setWidth: width as u64];
            let () = msg_send![descriptor.as_id(), setHeight: height as u64];
            let () = msg_send![descriptor.as_id(), setDepth: 1u64];
            let () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared];
            let () = msg_send![
                descriptor.as_id(),
                setPixelFormat: texture_pixel_to_mtl_pixel(&pixel)
            ];
            let staging: ObjcId = msg_send![device, newTextureWithDescriptor: descriptor.as_id()];
            if staging == nil {
                return None;
            }
            let queue: ObjcId = msg_send![device, newCommandQueue];
            let command_buffer: ObjcId = msg_send![queue, commandBuffer];
            let blit: ObjcId = msg_send![command_buffer, blitCommandEncoder];
            let () = msg_send![blit, copyFromTexture: mtl_tex toTexture: staging];
            let () = msg_send![blit, endEncoding];
            let () = msg_send![command_buffer, commit];
            let () = msg_send![command_buffer, waitUntilCompleted];
            // Bytes per pixel from the ALLOCATED format — this reader
            // serves float debug targets too, not just BGRA8.
            let bpp = match &pixel {
                crate::texture::TexturePixel::RGBAf32 => 16,
                crate::texture::TexturePixel::RGBAf16 => 8,
                crate::texture::TexturePixel::Rf32 => 4,
                _ => 4,
            };
            let mut bytes = vec![0u8; width * height * bpp];
            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: width as u64,
                    height: height as u64,
                    depth: 1,
                },
            };
            let () = msg_send![
                staging,
                getBytes: bytes.as_mut_ptr()
                bytesPerRow: width * bpp
                bytesPerImage: width * height * bpp
                fromRegion: region
                mipmapLevel: 0
                slice: 0
            ];
            let () = msg_send![staging, release];
            let () = msg_send![queue, release];
            Some((width, height, bytes))
        }
    }
}

/// Renderer-owned capture requests (texture ids awaiting a pass that
/// renders them) and finished results. Statics rather than Cx state because
/// results are pushed from Metal completion threads (the
/// SCREENSHOT_FILE_SINKS pattern in cx_shared.rs).
static RENDER_TEXTURE_CAPTURE_REQUESTS: Mutex<Vec<crate::texture::TextureId>> =
    Mutex::new(Vec::new());
#[allow(clippy::type_complexity)]
static RENDER_TEXTURE_CAPTURE_RESULTS: Mutex<
    Vec<(crate::texture::TextureId, usize, usize, Vec<u8>)>,
> = Mutex::new(Vec::new());

impl Cx {
    /// RENDERER-OWNED capture of a render-target texture — the race-free
    /// sibling of [`Cx::debug_read_render_texture`], which synchronizes on
    /// a PRIVATE one-off queue with no ordering against in-flight work on
    /// the producing queue (its bytes could be read before the pass that
    /// drew them finished — intermittent half-rendered readbacks).
    ///
    /// Registers the request and returns true; the caller then repaints the
    /// pass that renders into `texture` (`Cx::repaint_pass`) and polls
    /// [`Cx::take_render_texture_captures`]. The next execution of that
    /// pass encodes a blit to a shared staging texture ON ITS OWN COMMAND
    /// BUFFER, and the buffer's completion handler delivers the bytes —
    /// they provably follow the render. Bytes come back in the texture's
    /// native 4-byte layout (BGRA8 = BGRA), full allocated size.
    pub fn request_render_texture_capture(&mut self, texture: &Texture) -> bool {
        let tid = texture.texture_id();
        let mut requests = RENDER_TEXTURE_CAPTURE_REQUESTS.lock().unwrap();
        if !requests.contains(&tid) {
            requests.push(tid);
        }
        true
    }

    /// Drain every finished renderer-owned capture:
    /// `(texture id, width, height, bytes)`.
    #[allow(clippy::type_complexity)]
    pub fn take_render_texture_captures(
        &mut self,
    ) -> Vec<(crate::texture::TextureId, usize, usize, Vec<u8>)> {
        std::mem::take(&mut *RENDER_TEXTURE_CAPTURE_RESULTS.lock().unwrap())
    }

    /// The encode half of the capture (called from `draw_pass` right after
    /// the render encoder ends): for each of this pass's color textures
    /// with a pending request, blit to shared staging on the SAME command
    /// buffer and hand the bytes over from its completion handler.
    fn encode_render_texture_captures(
        &mut self,
        metal_cx: &MetalCx,
        draw_pass_id: DrawPassId,
        command_buffer: ObjcId,
    ) {
        if RENDER_TEXTURE_CAPTURE_REQUESTS.lock().unwrap().is_empty() {
            return;
        }
        let tids: Vec<crate::texture::TextureId> = self.passes[draw_pass_id]
            .color_textures
            .iter()
            .map(|ct| ct.texture.texture_id())
            .collect();
        for tid in tids {
            let requested = {
                let mut requests = RENDER_TEXTURE_CAPTURE_REQUESTS.lock().unwrap();
                match requests.iter().position(|r| *r == tid) {
                    Some(at) => {
                        requests.remove(at);
                        true
                    }
                    None => false,
                }
            };
            if !requested {
                continue;
            }
            let (width, height, mtl_tex, pixel) = {
                let cxtexture = &self.textures[tid];
                let Some(alloc) = cxtexture.alloc.as_ref() else {
                    crate::error!("render texture capture: texture has no allocation");
                    continue;
                };
                let Some(tex) = cxtexture.os.texture.as_ref() else {
                    crate::error!("render texture capture: texture has no MTLTexture");
                    continue;
                };
                (alloc.width, alloc.height, tex.as_id(), alloc.pixel.clone())
            };
            // Native pixel stride: float targets are wider than 4 bytes — a
            // 4-byte assumption under-allocates the readback and trips the
            // AGX `bytes_per_row` assertion (same law as debug_read above).
            let bpp: usize = match &pixel {
                crate::texture::TexturePixel::RGBAf32 => 16,
                crate::texture::TexturePixel::RGBAf16 => 8,
                crate::texture::TexturePixel::Rf32 => 4,
                _ => 4,
            };
            unsafe {
                let descriptor = RcObjcId::from_owned(
                    NonNull::new(msg_send![class!(MTLTextureDescriptor), new]).unwrap(),
                );
                let () = msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2];
                let () = msg_send![descriptor.as_id(), setDepth: 1u64];
                let () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared];
                let () = msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead];
                let () = msg_send![descriptor.as_id(), setWidth: width as u64];
                let () = msg_send![descriptor.as_id(), setHeight: height as u64];
                let () = msg_send![
                    descriptor.as_id(),
                    setPixelFormat: texture_pixel_to_mtl_pixel(&pixel)
                ];
                let staging = NonNull::new(
                    msg_send![metal_cx.device, newTextureWithDescriptor: descriptor.as_id()],
                )
                .map(RcObjcId::from_owned);
                let Some(staging) = staging else {
                    crate::error!("render texture capture: staging texture alloc failed");
                    continue;
                };
                let blit: ObjcId = msg_send![command_buffer, blitCommandEncoder];
                let () = msg_send![blit, copyFromTexture: mtl_tex toTexture: staging.as_id()];
                let () = msg_send![blit, synchronizeTexture: staging.as_id() slice: 0 level: 0];
                let () = msg_send![blit, endEncoding];
                let capture = Mutex::new(Some((tid, width, height, bpp, staging)));
                let () = msg_send![
                    command_buffer,
                    addCompletedHandler: &objc_block!(move |_cmd: ObjcId| {
                        if let Some((tid, width, height, bpp, staging)) =
                            capture.lock().unwrap().take()
                        {
                            let mut bytes = vec![0u8; width * height * bpp];
                            let region = MTLRegion {
                                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                                size: MTLSize {
                                    width: width as u64,
                                    height: height as u64,
                                    depth: 1,
                                },
                            };
                            let _: () = msg_send![
                                staging.as_id(),
                                getBytes: bytes.as_mut_ptr()
                                bytesPerRow: width * bpp
                                bytesPerImage: width * height * bpp
                                fromRegion: region
                                mipmapLevel: 0
                                slice: 0
                            ];
                            RENDER_TEXTURE_CAPTURE_RESULTS
                                .lock()
                                .unwrap()
                                .push((tid, width, height, bytes));
                        }
                    })
                ];
            }
        }
    }
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
        TexturePixel::VideoYuvPlane => MTLPixelFormat::R8Unorm,
        TexturePixel::VideoExternal => MTLPixelFormat::BGRA8Unorm,
        TexturePixel::VideoGlMemoryRgba => MTLPixelFormat::RGBA8Unorm,
        TexturePixel::VideoRgbaHardwareBuffer => MTLPixelFormat::BGRA8Unorm,
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

    /// Allocate the Private MTLTexture for a Vec texture on first sight (or
    /// size/format change) and, when the CPU side has pending data, stage
    /// it and encode the blit(s) on `enc` — partial rects become sub-region
    /// blits, mip chains one blit per level, cube maps one per face. Never
    /// touches the texture from the CPU: that is what raced the in-flight
    /// readers. Returns the bytes uploaded.
    fn update_vec_texture(&mut self, metal_cx: &MetalCx, enc: &mut VecUploadEncoder) -> u64 {
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
            // Private: only ever written by blits on the command queue, so
            // hazard tracking orders those writes against every reader.
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
            let mip_level_count = match &self.format {
                TextureFormat::VecMipBGRAu8_32 { max_level, .. }
                | TextureFormat::VecMipRGBAf32 { max_level, .. } => {
                    max_level.map(|level| level.saturating_add(1)).unwrap_or(1)
                }
                _ => 1,
            };
            let _: () = unsafe {
                msg_send![descriptor.as_id(), setMipmapLevelCount: mip_level_count as u64]
            };
            let _: () = unsafe {
                msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]
            };
            let texture: ObjcId =
                unsafe { msg_send![metal_cx.device, newTextureWithDescriptor: descriptor] };
            self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
            self.os.vec_fresh = true;
        }
        let Some(texture) = self.os.texture.as_ref().map(|t| t.as_id()) else {
            return 0;
        };

        enum VecLayout {
            Plain,
            Mip { max_level: usize },
            Cube,
        }
        fn as_bytes<T: Copy>(data: &[T]) -> &[u8] {
            // u8/u32/f32 image words: plain old data, no padding.
            unsafe {
                std::slice::from_raw_parts(
                    data.as_ptr() as *const u8,
                    data.len() * std::mem::size_of::<T>(),
                )
            }
        }
        // Data still taken (`take_vec_*` without a `put_back` yet): leave the
        // update pending rather than clearing it against nothing.
        let has_data = match &self.format {
            TextureFormat::VecBGRAu8_32 { data, .. }
            | TextureFormat::VecCubeBGRAu8_32 { data, .. }
            | TextureFormat::VecMipBGRAu8_32 { data, .. } => data.is_some(),
            TextureFormat::VecMipRGBAf32 { data, .. }
            | TextureFormat::VecRGBAf32 { data, .. }
            | TextureFormat::VecRf32 { data, .. } => data.is_some(),
            TextureFormat::VecRu8 { data, .. } | TextureFormat::VecRGu8 { data, .. } => {
                data.is_some()
            }
            _ => false,
        };
        if !has_data {
            return 0;
        }
        let update = self.take_updated();
        // A fresh MTLTexture has no rows worth keeping: whatever the CPU
        // holds goes up whole, whatever the dirty rect said.
        let update = if self.os.vec_fresh {
            TextureUpdated::Full
        } else {
            update
        };
        if update.is_empty() {
            return 0;
        }
        let (width, height, bpp, layout, bytes): (usize, usize, usize, VecLayout, &[u8]) =
            match &self.format {
                TextureFormat::VecBGRAu8_32 {
                    width,
                    height,
                    data,
                    ..
                } => (*width, *height, 4, VecLayout::Plain, as_bytes(data.as_ref().unwrap())),
                TextureFormat::VecCubeBGRAu8_32 {
                    width,
                    height,
                    data,
                    ..
                } => (*width, *height, 4, VecLayout::Cube, as_bytes(data.as_ref().unwrap())),
                TextureFormat::VecMipBGRAu8_32 {
                    width,
                    height,
                    data,
                    max_level,
                    ..
                } => (
                    *width,
                    *height,
                    4,
                    VecLayout::Mip {
                        max_level: max_level.unwrap_or(0),
                    },
                    as_bytes(data.as_ref().unwrap()),
                ),
                TextureFormat::VecMipRGBAf32 {
                    width,
                    height,
                    data,
                    max_level,
                    ..
                } => (
                    *width,
                    *height,
                    16,
                    VecLayout::Mip {
                        max_level: max_level.unwrap_or(0),
                    },
                    as_bytes(data.as_ref().unwrap()),
                ),
                TextureFormat::VecRGBAf32 {
                    width,
                    height,
                    data,
                    ..
                } => (*width, *height, 16, VecLayout::Plain, as_bytes(data.as_ref().unwrap())),
                TextureFormat::VecRu8 {
                    width,
                    height,
                    data,
                    ..
                } => (*width, *height, 1, VecLayout::Plain, as_bytes(data.as_ref().unwrap())),
                TextureFormat::VecRGu8 {
                    width,
                    height,
                    data,
                    ..
                } => (*width, *height, 2, VecLayout::Plain, as_bytes(data.as_ref().unwrap())),
                TextureFormat::VecRf32 {
                    width,
                    height,
                    data,
                    ..
                } => (*width, *height, 4, VecLayout::Plain, as_bytes(data.as_ref().unwrap())),
                _ => return 0,
            };
        if width == 0 || height == 0 {
            return 0;
        }
        let full_len = width.saturating_mul(height).saturating_mul(bpp);

        // The sub-rect worth uploading: 2D textures only; mip chains and
        // cube maps always go whole (as they always did).
        let rect = match (update, &layout) {
            (TextureUpdated::Partial(r), VecLayout::Plain) => {
                let x0 = r.origin.x.min(width);
                let y0 = r.origin.y.min(height);
                let x1 = r.origin.x.saturating_add(r.size.width).min(width);
                let y1 = r.origin.y.saturating_add(r.size.height).min(height);
                if x1 <= x0 || y1 <= y0 {
                    return 0;
                }
                if x0 == 0 && y0 == 0 && x1 == width && y1 == height {
                    None
                } else {
                    Some((x0, y0, x1 - x0, y1 - y0))
                }
            }
            _ => None,
        };

        // How many bytes the staging copy needs, and what the data must hold.
        let (staging_len, required) = match (&layout, rect) {
            (VecLayout::Plain, None) => (full_len, full_len),
            (VecLayout::Plain, Some((_, _, w, h))) => (w * h * bpp, full_len),
            (VecLayout::Cube, _) => (full_len.saturating_mul(6), full_len.saturating_mul(6)),
            // Level 0 must be there; the per-level loop stops where the
            // chain ends, exactly like the old replaceRegion loop.
            (VecLayout::Mip { .. }, _) => (bytes.len(), full_len),
        };
        if bytes.len() < required {
            crate::error!(
                "vec texture upload: {} bytes of data for a {}x{} texture needing {}",
                bytes.len(),
                width,
                height,
                required
            );
            return 0;
        }
        let Some(staging) = metal_cx.take_staging(staging_len) else {
            crate::error!("vec texture upload: staging buffer allocation failed");
            return 0;
        };
        let dst: *mut u8 = unsafe { msg_send![staging.buffer, contents] };
        if dst.is_null() {
            crate::error!("vec texture upload: staging buffer has no contents");
            return 0;
        }
        debug_assert!(staging.len >= staging_len);
        match rect {
            None => unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, staging_len);
            },
            Some((x, y, w, h)) => {
                let row_len = w * bpp;
                for row in 0..h {
                    let src = ((y + row) * width + x) * bpp;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr().add(src),
                            dst.add(row * row_len),
                            row_len,
                        );
                    }
                }
            }
        }

        let blit = enc.blit();
        let copy = |offset: usize, w: usize, h: usize, slice: usize, level: usize, x: usize, y: usize| {
            let bytes_per_row = (w * bpp) as u64;
            let () = unsafe {
                msg_send![
                    blit,
                    copyFromBuffer: staging.buffer
                    sourceOffset: offset as u64
                    sourceBytesPerRow: bytes_per_row
                    sourceBytesPerImage: bytes_per_row * (h as u64)
                    sourceSize: MTLSize { width: w as u64, height: h as u64, depth: 1 }
                    toTexture: texture
                    destinationSlice: slice as u64
                    destinationLevel: level as u64
                    destinationOrigin: MTLOrigin { x: x as u64, y: y as u64, z: 0 }
                ]
            };
        };
        match layout {
            VecLayout::Plain => match rect {
                None => copy(0, width, height, 0, 0, 0, 0),
                Some((x, y, w, h)) => copy(0, w, h, 0, 0, x, y),
            },
            VecLayout::Cube => {
                for face in 0..6usize {
                    copy(face * full_len, width, height, face, 0, 0, 0);
                }
            }
            VecLayout::Mip { max_level } => {
                // Concatenated chain, level 0 first (draw's
                // generate_bgra_mip_chain / the f32 twin).
                let mut offset = 0usize;
                let mut level_width = width.max(1);
                let mut level_height = height.max(1);
                for level in 0..=max_level {
                    let level_len = level_width.saturating_mul(level_height).saturating_mul(bpp);
                    if offset.saturating_add(level_len) > staging_len {
                        break;
                    }
                    copy(offset, level_width, level_height, 0, level, 0, 0);
                    offset += level_len;
                    level_width = (level_width / 2).max(1);
                    level_height = (level_height / 2).max(1);
                }
            }
        }
        self.os.vec_fresh = false;
        enc.used.push(staging);
        enc.bytes = enc.bytes.saturating_add(staging_len as u64);
        staging_len as u64
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
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

            // IOSurfaceBytesPerRow (width * 4, aligned to 16 bytes as required by Metal on iOS)
            let bpr_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfaceBytesPerRow");
            let bytes_per_row = ((alloc.width * 4 + 15) & !15) as u64;
            let bpr_val: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInteger: bytes_per_row];
            let _: () = msg_send![dict, setObject: bpr_val forKey: bpr_key];

            // IOSurfacePixelFormat (BGRA = 'BGRA' = 0x42475241)
            let pf_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfacePixelFormat");
            let pf_val: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInteger: 0x42475241u64];
            let _: () = msg_send![dict, setObject: pf_val forKey: pf_key];

            // Required for CoreVideo + OpenGLES texture cache interop on iOS.
            let gl_tex_compat_key = crate::os::apple::apple_util::str_to_nsstring(
                "IOSurfaceOpenGLESTextureCompatibility",
            );
            let gl_tex_compat_val: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
            let _: () = msg_send![dict, setObject: gl_tex_compat_val forKey: gl_tex_compat_key];

            let gl_fbo_compat_key =
                crate::os::apple::apple_util::str_to_nsstring("IOSurfaceOpenGLESFBOCompatibility");
            let gl_fbo_compat_val: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
            let _: () = msg_send![dict, setObject: gl_fbo_compat_val forKey: gl_fbo_compat_key];

            // IOSurfaceIsGlobal is deprecated since iOS 11 and may cause
            // texImageIOSurface failures on some devices. Only set on macOS.
            #[cfg(target_os = "macos")]
            {
                let global_key = crate::os::apple::apple_util::str_to_nsstring("IOSurfaceIsGlobal");
                let global_val: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
                let _: () = msg_send![dict, setObject: global_val forKey: global_key];
            }

            dict
        };

        // Create IOSurface
        let iosurface = unsafe { IOSurfaceCreate(iosurface_props) };
        unsafe {
            let _: () = msg_send![iosurface_props, release];
        }

        if iosurface.is_null() {
            crate::error!(
                "Failed to create IOSurface {}x{}",
                alloc.width,
                alloc.height
            );
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
        #[cfg(target_os = "ios")]
        let _: () =
            unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared] };
        #[cfg(not(target_os = "ios"))]
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
        if let Some(previous) = self.os.iosurface.replace(iosurface) {
            unsafe { CFRelease(previous) };
        }
        self.os.iosurface_id = iosurface_id;
        self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));

        iosurface_id
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
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
        #[cfg(target_os = "ios")]
        let _: () =
            unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared] };
        #[cfg(not(target_os = "ios"))]
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
        if let Some(previous) = self.os.iosurface.replace(iosurface) {
            unsafe { CFRelease(previous) };
        }
        self.os.iosurface_id = iosurface_id;
        self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
        true
    }

    fn update_render_target(&mut self, metal_cx: &MetalCx, width: usize, height: usize) {
        // Metal forbids zero-size textures. A hosted (--stdin-loop) child
        // can draw its first frame before the host's WindowGeomChange
        // arrives, with 0×0 passes throughout — allocate 1×1 instead of
        // aborting on the MTLTextureDescriptor validation; the target
        // reallocates at the real size the moment geometry lands.
        let width = width.max(1);
        let height = height.max(1);
        if self.alloc_render(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(
                NonNull::new(unsafe { msg_send![class!(MTLTextureDescriptor), new] }).unwrap(),
            );
            let is_cube = matches!(&self.format, TextureFormat::RenderCubeBGRAu8 { .. });

            let _: () = unsafe {
                msg_send![
                    descriptor.as_id(),
                    setTextureType: if is_cube {
                        MTLTextureType::Cube
                    } else {
                        MTLTextureType::D2
                    }
                ]
            };
            let _: () = unsafe { msg_send![descriptor.as_id(), setWidth: alloc.width as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setHeight: alloc.height as u64] };
            let _: () = unsafe { msg_send![descriptor.as_id(), setDepth: 1u64] };
            let _: () =
                unsafe { msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private] };
            let _: () = unsafe {
                msg_send![descriptor.as_id(), setUsage: (MTLTextureUsage::RenderTarget as u64 | MTLTextureUsage::ShaderRead as u64)]
            };
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
        // Same zero-size guard as update_render_target (hosted first frame).
        let width = width.max(1);
        let height = height.max(1);
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

        #[link(name = "OpenGL", kind = "framework")]
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
                K_CGL_PFA_OPENGL_PROFILE,
                K_CGL_OGL_PVERSION_3_2_CORE,
                K_CGL_PFA_COLOR_SIZE,
                24,
                K_CGL_PFA_DEPTH_SIZE,
                24,
                K_CGL_PFA_STENCIL_SIZE,
                8,
                K_CGL_PFA_ACCELERATED,
                K_CGL_PFA_DOUBLE_BUFFER,
                0,
            ];

            let mut pix: CGLPixelFormatObj = std::ptr::null_mut();
            let mut npix: i32 = 0;
            let err = CGLChoosePixelFormat(attribs.as_ptr(), &mut pix, &mut npix);
            assert!(
                err == 0 && !pix.is_null(),
                "CGLChoosePixelFormat failed: {}",
                err
            );

            let mut ctx: CGLContextObj = std::ptr::null_mut();
            let err = CGLCreateContext(pix, std::ptr::null_mut(), &mut ctx);
            assert!(
                err == 0 && !ctx.is_null(),
                "CGLCreateContext failed: {}",
                err
            );

            // Load OpenGL.framework for dlsym-based proc address lookup
            extern "C" {
                fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
            }
            let framework_path = b"/System/Library/Frameworks/OpenGL.framework/OpenGL\0";
            let opengl_framework = dlopen(framework_path.as_ptr() as *const i8, 1); // RTLD_LAZY
            assert!(
                !opengl_framework.is_null(),
                "Failed to load OpenGL.framework"
            );

            CglRenderBridge {
                cgl_context: ctx,
                cgl_pixel_format: pix,
                opengl_framework,
            }
        }
    }

    pub fn make_current(&self) {
        #[link(name = "OpenGL", kind = "framework")]
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

        #[link(name = "OpenGL", kind = "framework")]
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

/// EAGL render bridge for iOS. Creates a standalone EAGL context (GLES 3.0)
/// that shares textures with Metal via IOSurface.
#[cfg(target_os = "ios")]
pub struct EaglRenderBridge {
    pub(crate) eagl_context: ObjcId,
    pub(crate) opengles_framework: *mut std::ffi::c_void,
}

#[cfg(target_os = "ios")]
impl EaglRenderBridge {
    pub fn new() -> Self {
        use std::ffi::c_void;

        // kEAGLRenderingAPIOpenGLES3 = 3
        const K_EAGL_RENDERING_API_OPENGLES3: u64 = 3;

        extern "C" {
            fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
        }

        unsafe {
            let ctx: ObjcId = msg_send![class!(EAGLContext), alloc];
            let ctx: ObjcId = msg_send![ctx, initWithAPI: K_EAGL_RENDERING_API_OPENGLES3];
            assert!(!ctx.is_null(), "Failed to create EAGLContext with GLES 3.0");

            let framework_path = b"/System/Library/Frameworks/OpenGLES.framework/OpenGLES\0";
            let opengles_framework = dlopen(framework_path.as_ptr() as *const i8, 1); // RTLD_LAZY
            assert!(
                !opengles_framework.is_null(),
                "Failed to load OpenGLES.framework"
            );

            EaglRenderBridge {
                eagl_context: ctx,
                opengles_framework,
            }
        }
    }

    pub fn make_current(&self) {
        let success: bool =
            unsafe { msg_send![class!(EAGLContext), setCurrentContext: self.eagl_context] };
        assert!(success, "EAGLContext setCurrentContext failed");
    }

    pub fn get_proc_address(&self, name: &str) -> *const std::ffi::c_void {
        extern "C" {
            fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
        }
        let c_name = std::ffi::CString::new(name).unwrap();
        unsafe { dlsym(self.opengles_framework, c_name.as_ptr()) }
    }

    pub fn gl_api(&self) -> crate::gl_render_bridge::GlApi {
        crate::gl_render_bridge::GlApi::GLES
    }

    /// Create a CVPixelBuffer and derive both a GLES texture and a Metal
    /// texture from it.  This is the standard iOS zero-copy path:
    /// CVPixelBuffer → CVOpenGLESTextureCache (GL side)
    /// CVPixelBuffer → CVMetalTextureCache   (Metal side)
    ///
    /// Returns `(gl_texture_id, metal_texture_objc_id)`.
    /// The caller must keep the returned Metal ObjcId alive (retained).
    pub fn create_shared_texture(
        &self,
        metal_device: ObjcId,
        width: usize,
        height: usize,
    ) -> (u32, ObjcId) {
        use crate::os::apple::apple_sys::{
            kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
            kCVPixelBufferOpenGLESCompatibilityKey, kCVPixelFormatType_32BGRA,
            CVMetalTextureCacheCreate, CVMetalTextureCacheCreateTextureFromImage,
            CVMetalTextureCacheRef, CVMetalTextureGetTexture, CVMetalTextureRef,
            CVPixelBufferCreate, CVPixelBufferRef,
        };

        const GL_TEXTURE_2D: u32 = 0x0DE1;
        const GL_RGBA: u32 = 0x1908;
        const GL_BGRA: u32 = 0x80E1;
        const GL_UNSIGNED_BYTE: u32 = 0x1401;

        type GlBindTextureFn = unsafe extern "C" fn(u32, u32);

        extern "C" {
            fn CVOpenGLESTextureCacheCreate(
                allocator: *const std::ffi::c_void,
                cache_attrs: *const std::ffi::c_void,
                eagl_ctx: ObjcId,
                tex_attrs: *const std::ffi::c_void,
                cache_out: *mut *mut std::ffi::c_void,
            ) -> i32;
            fn CVOpenGLESTextureCacheCreateTextureFromImage(
                allocator: *const std::ffi::c_void,
                cache: *mut std::ffi::c_void,
                pixel_buffer: *mut std::ffi::c_void,
                tex_attrs: *const std::ffi::c_void,
                target: u32,
                internal_format: i32,
                width: i32,
                height: i32,
                format: u32,
                typ: u32,
                plane_index: usize,
                texture_out: *mut *mut std::ffi::c_void,
            ) -> i32;
            fn CVOpenGLESTextureGetName(texture: *mut std::ffi::c_void) -> u32;
        }

        unsafe {
            // -- 1. Create CVPixelBuffer with Metal + GLES compatibility ------
            let pb_attrs: ObjcId = {
                let dict: ObjcId = msg_send![class!(NSMutableDictionary), new];

                let yes_val: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
                let _: () = msg_send![
                    dict, setObject: yes_val
                    forKey: kCVPixelBufferOpenGLESCompatibilityKey as ObjcId
                ];

                let yes_val2: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
                let _: () = msg_send![
                    dict, setObject: yes_val2
                    forKey: kCVPixelBufferMetalCompatibilityKey as ObjcId
                ];

                // IOSurface backing is required for cross-API sharing.
                let io_props: ObjcId = msg_send![class!(NSDictionary), dictionary];
                let _: () = msg_send![
                    dict, setObject: io_props
                    forKey: kCVPixelBufferIOSurfacePropertiesKey as ObjcId
                ];

                dict
            };

            let mut pixel_buffer: CVPixelBufferRef = std::ptr::null_mut();
            let status = CVPixelBufferCreate(
                std::ptr::null(),
                width,
                height,
                kCVPixelFormatType_32BGRA,
                pb_attrs as *const std::ffi::c_void,
                &mut pixel_buffer,
            );
            let _: () = msg_send![pb_attrs, release];
            assert!(
                status == 0 && !pixel_buffer.is_null(),
                "CVPixelBufferCreate failed: {} ({}x{})",
                status,
                width,
                height,
            );

            // -- 2. GL texture from CVPixelBuffer -----------------------------
            let mut gl_cache: *mut std::ffi::c_void = std::ptr::null_mut();
            let status = CVOpenGLESTextureCacheCreate(
                std::ptr::null(),
                std::ptr::null(),
                self.eagl_context,
                std::ptr::null(),
                &mut gl_cache,
            );
            assert!(
                status == 0 && !gl_cache.is_null(),
                "CVOpenGLESTextureCacheCreate failed: {}",
                status,
            );

            let mut cv_gl_tex: *mut std::ffi::c_void = std::ptr::null_mut();
            let status = CVOpenGLESTextureCacheCreateTextureFromImage(
                std::ptr::null(),
                gl_cache,
                pixel_buffer as *mut std::ffi::c_void,
                std::ptr::null(),
                GL_TEXTURE_2D,
                GL_RGBA as i32,
                width as i32,
                height as i32,
                GL_BGRA,
                GL_UNSIGNED_BYTE,
                0,
                &mut cv_gl_tex,
            );
            assert!(
                status == 0 && !cv_gl_tex.is_null(),
                "CVOpenGLESTextureCacheCreateTextureFromImage failed: {} ({}x{})",
                status,
                width,
                height,
            );

            let gl_texture_id = CVOpenGLESTextureGetName(cv_gl_tex);

            let gl_bind_texture: GlBindTextureFn =
                std::mem::transmute(self.get_proc_address("glBindTexture"));
            gl_bind_texture(GL_TEXTURE_2D, gl_texture_id);
            gl_bind_texture(GL_TEXTURE_2D, 0);

            // -- 3. Metal texture from CVPixelBuffer --------------------------
            let mut mtl_cache: CVMetalTextureCacheRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreate(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                metal_device,
                std::ptr::null_mut(),
                &mut mtl_cache,
            );
            assert!(
                status == 0 && !mtl_cache.is_null(),
                "CVMetalTextureCacheCreate failed: {}",
                status,
            );

            let mut cv_mtl_tex: CVMetalTextureRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                mtl_cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::BGRA8Unorm as u64,
                width,
                height,
                0,
                &mut cv_mtl_tex,
            );
            assert!(
                status == 0 && !cv_mtl_tex.is_null(),
                "CVMetalTextureCacheCreateTextureFromImage failed: {} ({}x{})",
                status,
                width,
                height,
            );

            let metal_texture: ObjcId = CVMetalTextureGetTexture(cv_mtl_tex);
            assert!(
                !metal_texture.is_null(),
                "CVMetalTextureGetTexture returned null"
            );
            // Retain — CVMetalTextureGetTexture returns unretained reference.
            let _: () = msg_send![metal_texture, retain];

            // Keep CV wrappers alive: gl_cache, cv_gl_tex, mtl_cache,
            // cv_mtl_tex, pixel_buffer all must outlive the textures.
            // Intentional leak — cleanup belongs with the texture lifecycle.

            (gl_texture_id, metal_texture)
        }
    }
}

/// MAKEPAD_GPU_PROFILE=1: per-pass GPU-time + geometry table, printed once
/// a second from the command-buffer completion threads. Names are the
/// passes' debug names; ms are summed GPU intervals over the window.
fn gpu_profile_accumulate(
    label: &str,
    gpu_seconds: f64,
    counters: &GpuSampleCounters,
) {
    use std::collections::HashMap;
    use std::sync::Mutex;
    #[derive(Default, Clone)]
    struct Slot {
        gpu_s: f64,
        buffers: u64,
        draws: u64,
        verts: u64,
        instances: u64,
        instance_bytes: u64,
    }
    static TABLE: Mutex<Option<(std::time::Instant, HashMap<String, Slot>)>> = Mutex::new(None);
    let Ok(mut guard) = TABLE.lock() else { return };
    let (started, table) =
        guard.get_or_insert_with(|| (std::time::Instant::now(), HashMap::new()));
    let slot = table.entry(label.to_string()).or_default();
    if gpu_seconds.is_finite() && gpu_seconds > 0.0 {
        slot.gpu_s += gpu_seconds;
    }
    slot.buffers += 1;
    slot.draws += counters.draw_calls;
    slot.verts += counters.vertices;
    slot.instances += counters.instances;
    slot.instance_bytes += counters.instance_bytes;
    if started.elapsed().as_secs_f64() >= 1.0 {
        let window = started.elapsed().as_secs_f64();
        let mut rows: Vec<(String, Slot)> =
            table.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        rows.sort_by(|a, b| b.1.gpu_s.total_cmp(&a.1.gpu_s));
        let mut out = format!("GPUPROF {:.2}s window\n", window);
        for (name, s) in rows {
            out.push_str(&format!(
                "  {:<24} gpu:{:>7.2}ms/s ({:>5.2}ms/buf) bufs:{:<4} draws:{:<6} verts:{:<9} inst:{:<8} instMB:{:.1}\n",
                name,
                s.gpu_s * 1000.0 / window,
                if s.buffers > 0 { s.gpu_s * 1000.0 / s.buffers as f64 } else { 0.0 },
                s.buffers,
                s.draws,
                s.verts,
                s.instances,
                s.instance_bytes as f64 / 1e6,
            ));
        }
        crate::log!("{}", out);
        *guard = None;
    }
}

#[cfg(test)]
mod vec_upload_tests {
    //! The slug glyph atlas grows by appending rows and marks ONLY those rows
    //! dirty. On Metal a size change means a fresh Private MTLTexture, and a
    //! fresh texture that only ever receives the dirty rect keeps garbage in
    //! every earlier row — every glyph cached before the growth vanished for
    //! the rest of the process (sandbox Doom HUD, model-viewer labels, 2026-08-24).
    //! This drives `update_vec_texture` through that exact sequence on the
    //! real device and reads the texture back after every step.
    use super::*;
    use crate::makepad_math::{PointUsize, RectUsize, SizeUsize};
    use crate::texture::{Texture, TextureFormat, TextureUpdated};

    /// Texel (x, y) = [x, y, tag, 1]: a dropped or misplaced row is obvious.
    fn image(width: usize, height: usize, tag: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&[x as f32, y as f32, tag, 1.0]);
            }
        }
        out
    }

    fn set_image(cx: &mut Cx, texture: &Texture, data: Vec<f32>, width: usize, updated: TextureUpdated) {
        let height = data.len() / (width * 4);
        cx.textures[texture.texture_id()].format = TextureFormat::VecRGBAf32 {
            width,
            height,
            data: Some(data),
            updated,
        };
    }

    fn upload(cx: &mut Cx, metal_cx: &mut MetalCx, texture: &Texture) -> u64 {
        let command_buffer = metal_cx.new_command_buffer();
        let mut enc = VecUploadEncoder::new(command_buffer);
        let bytes = cx.textures[texture.texture_id()].update_vec_texture(metal_cx, &mut enc);
        enc.finish(metal_cx);
        unsafe {
            let () = msg_send![command_buffer, commit];
            let () = msg_send![command_buffer, waitUntilCompleted];
        }
        bytes
    }

    fn read_back(cx: &mut Cx, texture: &Texture) -> (usize, usize, Vec<f32>) {
        let (width, height, bytes) = cx
            .debug_read_render_texture(texture)
            .expect("the vec texture should be allocated and readable");
        let floats = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        (width, height, floats)
    }

    fn rows(width: usize, dirty_rows: std::ops::Range<usize>) -> TextureUpdated {
        TextureUpdated::Partial(RectUsize::new(
            PointUsize::new(0, dirty_rows.start),
            SizeUsize::new(width, dirty_rows.end - dirty_rows.start),
        ))
    }

    #[test]
    fn a_grown_vec_texture_keeps_every_earlier_row() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut metal_cx = MetalCx::new();
        let texture = Texture::new_with_format(
            &mut cx,
            TextureFormat::VecRGBAf32 {
                width: 1,
                height: 1,
                data: None,
                updated: TextureUpdated::Empty,
            },
        );
        const W: usize = 8;

        // Frame 1: first sight, one row, marked Full.
        set_image(&mut cx, &texture, image(W, 1, 7.0), W, TextureUpdated::Full);
        assert!(upload(&mut cx, &mut metal_cx, &texture) > 0);
        assert_eq!(read_back(&mut cx, &texture), (W, 1, image(W, 1, 7.0)));

        // Frame 2: the atlas appended a row; only that row is dirty. The
        // MTLTexture is reallocated — row 0 must come along.
        set_image(&mut cx, &texture, image(W, 2, 7.0), W, rows(W, 1..2));
        assert_eq!(upload(&mut cx, &mut metal_cx, &texture), (W * 2 * 16) as u64, "a fresh texture uploads whole");
        assert_eq!(read_back(&mut cx, &texture), (W, 2, image(W, 2, 7.0)), "row 0 lost on growth");

        // Frame 3: another row, another reallocation; rows 0 and 1 must survive.
        set_image(&mut cx, &texture, image(W, 3, 7.0), W, rows(W, 2..3));
        upload(&mut cx, &mut metal_cx, &texture);
        assert_eq!(read_back(&mut cx, &texture), (W, 3, image(W, 3, 7.0)), "rows 0-1 lost on growth");

        // Frame 4: no growth — an in-place partial rewrite of row 1 touches
        // exactly row 1 (the sub-region blit path stays a sub-region blit).
        let mut partial = image(W, 3, 7.0);
        partial[W * 4..W * 8].copy_from_slice(&image(W, 1, 9.0));
        let expected = partial.clone();
        set_image(&mut cx, &texture, partial, W, rows(W, 1..2));
        assert_eq!(upload(&mut cx, &mut metal_cx, &texture), (W * 16) as u64, "an in-place row goes up as a sub-rect");
        assert_eq!(read_back(&mut cx, &texture), (W, 3, expected));

        // Frame 5: a stale Partial (data taken and put back the same size)
        // with nothing pending uploads nothing and changes nothing.
        assert_eq!(upload(&mut cx, &mut metal_cx, &texture), 0);
    }
}
