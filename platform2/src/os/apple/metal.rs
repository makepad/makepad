use {
    makepad_objc_sys::{
        msg_send,
        sel,
        class,
        sel_impl,
    },
    crate::{
        script::vm::*,
        makepad_objc_sys::objc_block,
        makepad_script::*,
        makepad_script::shader::*,
        makepad_script::shader_backend::*,
        os::{
            apple::apple_sys::*,
            apple::apple_util::{
                nsstring_to_string,
                str_to_nsstring,
            },
            cx_stdin::PresentableDraw,
        },
        draw_list::DrawListId,
        draw_vars::DrawVars,
        draw_shader::{CxDrawShader, CxDrawShaderMapping, CxDrawShaderCode, DrawShaderId},
        geometry::Geometry,
        cx::Cx,
        draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
        studio::{AppToStudio, GPUSample, StudioScreenshotResponse},
        texture::{
            CxTexture,
            Texture,
            TexturePixel,
            TextureAlloc,
            TextureFormat,
        },
    },
    std::time::{Instant},
    std::fmt::Write,
    std::sync::{
        Arc,
        Condvar,
        Mutex,
    },
};

#[cfg(target_os = "macos")]
use crate::{
    metal_xpc::store_xpc_service_texture
};


impl Cx {
    
    
    fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        encoder: ObjcId,
        command_buffer: ObjcId,
        gpu_read_guards: &mut Vec<MetalRwLockGpuReadGuard>,
        metal_cx: &MetalCx,
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();
        //self.views[view_id].set_clipping_uniforms();
        //self.draw_lists[draw_list_id].uniform_view_transform(&Mat4f::identity());
        
        for draw_item_id in 0..draw_items_len {
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id].kind.sub_list() {
                self.render_view(
                    draw_pass_id,
                    sub_list_id,
                    zbias,
                    zbias_step,
                    encoder,
                    command_buffer,
                    gpu_read_guards,
                    metal_cx,
                );
            }
            else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                } else {
                    continue;
                };
                
                let sh = &self.draw_shaders[draw_call.draw_shader_id.index];
                if sh.os_shader_id.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = &self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];
                
                if sh.mapping.uses_time{
                    self.demo_time_repaint = true;
                }
                
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    self.os.bytes_written += draw_item.instances.as_ref().unwrap().len() * 4;
                    draw_item.os.instance_buffer.next();
                    draw_item.os.instance_buffer.get_mut().cpu_write().update(metal_cx, &draw_item.instances.as_ref().unwrap());
                }
                
                // update the zbias uniform if we have it.
                draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                
                // lets verify our instance_offset is not disaligned
                let instances = (draw_item.instances.as_ref().unwrap().len() / sh.mapping.instances.total_slots) as u64;
                
                if instances == 0 {
                    continue;
                }
                let render_pipeline_state = shp.render_pipeline_state.as_id();
                unsafe {let () = msg_send![encoder, setRenderPipelineState: render_pipeline_state];}
                
                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {geometry_id}
                else {
                    continue;
                };
                
                let geometry = &mut self.geometries[geometry_id];
                
                if geometry.dirty {
                    geometry.os.index_buffer.next();
                    geometry.os.index_buffer.get_mut().cpu_write().update(metal_cx, &geometry.indices);
                    geometry.os.vertex_buffer.next();
                    geometry.os.vertex_buffer.get_mut().cpu_write().update(metal_cx, &geometry.vertices);
                    geometry.dirty = false;
                }
                
                // Uncomment to enable draw call debug output:
                //Self::_debug_call_info(sh, draw_item.instances.as_ref().unwrap(), draw_call, instances, geometry);
                
                if let Some(inner) = geometry.os.vertex_buffer.get().cpu_read().inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 0
                    ]}
                }
                else {crate::error!("Drawing error: vertex_buffer None")}
                
                if let Some(inner) = draw_item.os.instance_buffer.get().cpu_read().inner.as_ref() {
                    unsafe {msg_send![
                        encoder,
                        setVertexBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 1
                    ]}
                    // Also bind instance buffer to fragment shader so it can access instance data
                    unsafe {msg_send![
                        encoder,
                        setFragmentBuffer: inner.buffer.as_id()
                        offset: 0
                        atIndex: 1
                    ]}
                }
                else {crate::error!("Drawing error: instance_buffer None")}
                
                let pass_uniforms = self.passes[draw_pass_id].pass_uniforms.as_slice();
                let draw_list_uniforms = draw_list.draw_list_uniforms.as_slice();
                let draw_call_uniforms = draw_call.draw_call_uniforms.as_slice();
                
                unsafe {
                    
                    //let () = msg_send![encoder, setVertexBytes: sh.mapping.live_uniforms_buf.as_ptr() as *const //std::ffi::c_void length: (sh.mapping.live_uniforms_buf.len() * 4) as u64 atIndex: 2u64];
                    
                    //let () = msg_send![encoder, setFragmentBytes: sh.mapping.live_uniforms_buf.as_ptr() as *const std::ffi::c_void length: (sh.mapping.live_uniforms_buf.len() * 4) as u64 atIndex: 2u64];
                    
                    if let Some(id) = shp.draw_call_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_call_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_call_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.pass_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: pass_uniforms.as_ptr() as *const std::ffi::c_void length: (pass_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.draw_list_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_list_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_list_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_list_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_list_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.dyn_uniform_buffer_id {
                        let () = msg_send![encoder, setVertexBytes: draw_call.dyn_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.dyn_uniforms.len() * 4) as u64 atIndex: id];
                        let () = msg_send![encoder, setFragmentBytes: draw_call.dyn_uniforms.as_ptr() as *const std::ffi::c_void length: (draw_call.dyn_uniforms.len() * 4) as u64 atIndex: id];
                    }
                    if let Some(id) = shp.scope_uniform_buffer_id {
                        let scope_buf = &sh.mapping.scope_uniforms_buf;
                        if !scope_buf.is_empty() {
                            let () = msg_send![encoder, setVertexBytes: scope_buf.as_ptr() as *const std::ffi::c_void length: (scope_buf.len() * 4) as u64 atIndex: id];
                            let () = msg_send![encoder, setFragmentBytes: scope_buf.as_ptr() as *const std::ffi::c_void length: (scope_buf.len() * 4) as u64 atIndex: id];
                        }
                    }
                    /*
                    let ct = &sh.mapping.const_table.table;
                    if ct.len()>0 {
                        let () = msg_send![encoder, setVertexBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 3u64];
                        let () = msg_send![encoder, setFragmentBytes: ct.as_ptr() as *const std::ffi::c_void length: (ct.len() * 4) as u64 atIndex: 3u64];
                    }*/
                }
                // lets set our textures
                for i in 0..sh.mapping.textures.len() {
                    
                    let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                        texture.texture_id()
                    }else {
                        let () = unsafe {msg_send![
                            encoder,
                            setFragmentTexture: nil
                            atIndex: i as u64
                        ]};
                        let () = unsafe {msg_send![
                            encoder,
                            setVertexTexture: nil
                            atIndex: i as u64
                        ]};
                        continue
                    };
                    
                    let cxtexture = &mut self.textures[texture_id];
                    
                    if cxtexture.format.is_shared() {
                        #[cfg(target_os = "macos")]
                        cxtexture.update_shared_texture(
                            metal_cx.device,
                        );
                    }
                    else if cxtexture.format.is_vec(){
                        cxtexture.update_vec_texture(
                            metal_cx,
                        );
                    }
                    
                    if let Some(texture) = cxtexture.os.texture.as_ref() {
                        let () = unsafe {msg_send![
                            encoder,
                            setFragmentTexture: texture.as_id()
                            atIndex: i as u64
                        ]};
                        let () = unsafe {msg_send![
                            encoder,
                            setVertexTexture: texture.as_id()
                            atIndex: i as u64
                        ]};
                    }
                    
                }
                self.os.draw_calls_done += 1;
                if let Some(inner) = geometry.os.index_buffer.get().cpu_read().inner.as_ref() {
                    let () = unsafe {msg_send![
                        encoder,
                        drawIndexedPrimitives: MTLPrimitiveType::Triangle
                        indexCount: geometry.indices.len() as u64
                        indexType: MTLIndexType::UInt32
                        indexBuffer: inner.buffer.as_id()
                        indexBufferOffset: 0
                        instanceCount: instances
                    ]};
                }
                else {crate::error!("Drawing error: index_buffer None")}
                
                gpu_read_guards.push(draw_item.os.instance_buffer.get().gpu_read());
                gpu_read_guards.push(geometry.os.vertex_buffer.get().gpu_read());
                gpu_read_guards.push(geometry.os.index_buffer.get().gpu_read());
            }
        }
    }
    
    /// Debug helper for printing draw call info. Uncomment the call in render_view to enable.
    #[allow(dead_code)]
    fn _debug_call_info(
        sh: &CxDrawShader,
        instance_data: &[f32],
        draw_call: &crate::draw_list::CxDrawCall,
        instances: u64,
        geometry: &crate::geometry::CxGeometry,
    ) {
        let total_slots = sh.mapping.instances.total_slots;
        
        println!("=== METAL DRAW CALL DEBUG ===");
        println!("  shader debug_id: {:?}", sh.debug_id);
        println!("  instance_count: {}", instances);
        println!("  total_slots per instance: {}", total_slots);
        println!("  instance_data.len(): {} floats", instance_data.len());
        
        // Print instance layout (all instance fields: dyn + rust)
        println!("  --- Instance Layout ({} inputs, {} total_slots) ---", sh.mapping.instances.inputs.len(), sh.mapping.instances.total_slots);
        for input in &sh.mapping.instances.inputs {
            println!("    {:?}: offset={}, slots={}", input.id, input.offset, input.slots);
        }
        
        // Print dyn_instances layout (just the dynamic portion)
        if !sh.mapping.dyn_instances.inputs.is_empty() {
            println!("  --- Dyn Instance Layout ({} inputs, {} total_slots) ---", sh.mapping.dyn_instances.inputs.len(), sh.mapping.dyn_instances.total_slots);
            for input in &sh.mapping.dyn_instances.inputs {
                println!("    {:?}: offset={}, slots={}", input.id, input.offset, input.slots);
            }
        }
        
        // Print first few instances with named values
        let num_instances_to_print = 3.min(instances as usize);
        for inst_idx in 0..num_instances_to_print {
            let base = inst_idx * total_slots;
            if base + total_slots <= instance_data.len() {
                println!("  --- Instance {} ---", inst_idx);
                for input in &sh.mapping.instances.inputs {
                    let start = base + input.offset;
                    let end = start + input.slots;
                    if end <= instance_data.len() {
                        let values = &instance_data[start..end];
                        println!("    {:?}: {:?}", input.id, values);
                    }
                }
            }
        }
        if instances > 3 {
            println!("  ... ({} more instances)", instances - 3);
        }
        
        // Print uniform info
        let draw_call_uniforms = draw_call.draw_call_uniforms.as_slice();
        println!("    dyn_uniforms ({} floats): {:?}", draw_call.dyn_uniforms.len(), &draw_call.dyn_uniforms[..draw_call.dyn_uniforms.len().min(8)]);
        println!("    draw_call_uniforms ({} floats): {:?}", draw_call_uniforms.len(), draw_call_uniforms);
        
        // Print texture info
        println!("    texture_slots count: {}", sh.mapping.textures.len());
        for (i, slot) in draw_call.texture_slots.iter().enumerate() {
            if let Some(texture) = slot {
                println!("    texture[{}]: Some(id={})", i, texture.texture_id().0);
            } else {
                println!("    texture[{}]: None", i);
            }
        }
        
        // Print geometry info
        println!(">>> METAL drawIndexedPrimitives:");
        println!("    indexCount: {}", geometry.indices.len());
        println!("    instanceCount: {}", instances);
        println!("    geometry vertices: {}", geometry.vertices.len());
        if !geometry.vertices.is_empty() {
            let num_verts = 12.min(geometry.vertices.len());
            println!("    first {} vertex floats: {:?}", num_verts, &geometry.vertices[..num_verts]);
        }
        println!("=============================");
    }
    
    pub fn draw_pass(
        &mut self,
        draw_pass_id: DrawPassId,
        metal_cx: &mut MetalCx,
        mode: DrawPassMode,
    ) {
        self.os.draw_calls_done  = 0;
        let draw_list_id = if let Some(draw_list_id) = self.passes[draw_pass_id].main_draw_list_id{
            draw_list_id
        }
        else{
            crate::error!("Draw pass has no draw list!");
            return
        };
        
        let pool: ObjcId = unsafe {msg_send![class!(NSAutoreleasePool), new]};
        
        let render_pass_descriptor: ObjcId = if let DrawPassMode::MTKView(view) = mode {
            unsafe{msg_send![view, currentRenderPassDescriptor]}
        }
        else{
            unsafe {msg_send![class!(MTLRenderPassDescriptorInternal), renderPassDescriptor]}
        };
        
        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
        
        let pass_rect = self.get_pass_rect(draw_pass_id, if mode.is_drawable().is_some() {1.0}else {dpi_factor}).unwrap();
        
        self.passes[draw_pass_id].set_ortho_matrix(
            pass_rect.pos, 
            pass_rect.size
        );
        
        self.passes[draw_pass_id].paint_dirty = false;

        if pass_rect.size.x <0.5 || pass_rect.size.y < 0.5 {
            return
        }
        
        self.passes[draw_pass_id].set_dpi_factor(dpi_factor);
        
        if let DrawPassMode::MTKView(_) = mode{
            let color_attachments:ObjcId = unsafe{msg_send![render_pass_descriptor, colorAttachments]};
            let color_attachment:ObjcId = unsafe{msg_send![color_attachments, objectAtIndexedSubscript: 0]};
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
        } 
        else if let Some(drawable) = mode.is_drawable() {
            let first_texture: ObjcId = unsafe {msg_send![drawable, texture]};
            let color_attachments: ObjcId = unsafe {msg_send![render_pass_descriptor, colorAttachments]};
            let color_attachment: ObjcId = unsafe {msg_send![color_attachments, objectAtIndexedSubscript: 0]};
            
            let () = unsafe {msg_send![
                color_attachment,
                setTexture: first_texture
            ]};
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
        }
        else {
            for (index, color_texture) in self.passes[draw_pass_id].color_textures.iter().enumerate() {
                let color_attachments: ObjcId = unsafe {msg_send![render_pass_descriptor, colorAttachments]};
                let color_attachment: ObjcId = unsafe {msg_send![color_attachments, objectAtIndexedSubscript: index as u64]};
                
                let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                let size = dpi_factor * pass_rect.size; 
                cxtexture.update_render_target(metal_cx, size.x as usize, size.y as usize);
                
                let is_initial = cxtexture.take_initial();
                
                if let Some(texture) = cxtexture.os.texture.as_ref() {
                    let () = unsafe {msg_send![
                        color_attachment,
                        setTexture: texture.as_id()
                    ]};
                }
                else {
                    crate::error!("draw_pass_to_texture invalid render target");
                }
                
                unsafe {msg_send![color_attachment, setStoreAction: MTLStoreAction::Store]}
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
                        }
                        else {
                            unsafe {let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Load];}
                        }
                    },
                    DrawPassClearColor::ClearWith(color) => {
                        unsafe {
                            let () = msg_send![color_attachment, setLoadAction: MTLLoadAction::Clear];
                            let () = msg_send![color_attachment, setClearColor: MTLClearColor {
                                red: color.x as f64,
                                green: color.y as f64,
                                blue: color.z as f64,
                                alpha: color.w as f64
                            }];
                        }
                    }
                }
            }
        }
        // attach depth texture
        if let Some(depth_texture) = &self.passes[draw_pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture.texture_id()];
            let size = dpi_factor * pass_rect.size;
            cxtexture.update_depth_stencil(metal_cx, size.x as usize, size.y as usize);
            let is_initial = cxtexture.take_initial();
            
            let depth_attachment: ObjcId = unsafe {msg_send![render_pass_descriptor, depthAttachment]};
            
            if let Some(texture) = cxtexture.os.texture.as_ref() {
                unsafe {msg_send![depth_attachment, setTexture: texture.as_id()]}
            }
            else {
                crate::error!("draw_pass_to_texture invalid render target");
            }
            let () = unsafe {msg_send![depth_attachment, setStoreAction: MTLStoreAction::Store]};
            
            match self.passes[draw_pass_id].clear_depth {
                DrawPassClearDepth::InitWith(depth) => {
                    if is_initial {
                        let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]};
                        let () = unsafe {msg_send![depth_attachment, setClearDepth: depth as f64]};
                    }
                    else {
                        let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Load]};
                    }
                },
                DrawPassClearDepth::ClearWith(depth) => {
                    let () = unsafe {msg_send![depth_attachment, setLoadAction: MTLLoadAction::Clear]};
                    let () = unsafe {msg_send![depth_attachment, setClearDepth: depth as f64]};
                }
            }
            // create depth state
            if self.passes[draw_pass_id].os.mtl_depth_state.is_none() {
                
                let desc: ObjcId = unsafe {msg_send![class!(MTLDepthStencilDescriptor), new]};
                let () = unsafe {msg_send![desc, setDepthCompareFunction: MTLCompareFunction::LessEqual]};
                let () = unsafe {msg_send![desc, setDepthWriteEnabled: true]};
                let depth_stencil_state: ObjcId = unsafe {msg_send![metal_cx.device, newDepthStencilStateWithDescriptor: desc]};
                self.passes[draw_pass_id].os.mtl_depth_state = Some(depth_stencil_state);
            }
        }
        
        let command_buffer: ObjcId = unsafe {msg_send![metal_cx.command_queue, commandBuffer]};
        let encoder: ObjcId = unsafe {msg_send![command_buffer, renderCommandEncoderWithDescriptor: render_pass_descriptor]};
        
        if let Some(depth_state) = self.passes[draw_pass_id].os.mtl_depth_state {
            let () = unsafe {msg_send![encoder, setDepthStencilState: depth_state]};
        }
        
        let pass_width = dpi_factor * pass_rect.size.x;
        let pass_height = dpi_factor * pass_rect.size.y;
        
        let () = unsafe {msg_send![encoder, setViewport: MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: pass_width,
            height: pass_height,
            znear: 0.0,
            zfar: 1.0,
        }]};
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;
        let mut gpu_read_guards = Vec::new();
        
        self.render_view(
            draw_pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            encoder,
            command_buffer,
            &mut gpu_read_guards,
            &metal_cx,
        );
        
        let () = unsafe {msg_send![encoder, endEncoding]};
        
        
        match mode {
            DrawPassMode::MTKView(view)=>{
                let drawable:ObjcId = unsafe {msg_send![view, currentDrawable]};
                let first_texture: ObjcId = unsafe {msg_send![drawable, texture]};
                let () = unsafe {msg_send![command_buffer, presentDrawable: drawable]};
                let screenshot = self.build_screenshot_struct(metal_cx, command_buffer, 0, pass_width as usize, pass_height as usize, first_texture, None);
                self.commit_command_buffer(screenshot, None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::Texture => {
                self.commit_command_buffer(None, None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::StdinMain(stdin_frame, kind_id) => {
                let main_texture = &self.passes[draw_pass_id].color_textures[0];
                let tex = &self.textures[main_texture.texture.texture_id()];
                let screenshot = if let Some(texture) = &tex.os.texture{
                    self.build_screenshot_struct(metal_cx, command_buffer, kind_id, pass_width as usize, pass_height as usize, texture.as_id(), tex.alloc.clone())
                }
                else{
                    None
                };
                self.commit_command_buffer(screenshot, Some(stdin_frame), command_buffer, gpu_read_guards);
            }
            DrawPassMode::Drawable(drawable) => {
                let first_texture: ObjcId = unsafe {msg_send![drawable, texture]};
                let () = unsafe {msg_send![command_buffer, presentDrawable: drawable]};
                let screenshot = self.build_screenshot_struct(metal_cx, command_buffer, 0, pass_width as usize, pass_height as usize, first_texture, None);
                self.commit_command_buffer(screenshot, None, command_buffer, gpu_read_guards);
            }
            DrawPassMode::Resizing(drawable) => {
                let first_texture: ObjcId = unsafe {msg_send![drawable, texture]};
                let screenshot = self.build_screenshot_struct(metal_cx, command_buffer, 0, pass_width as usize, pass_height as usize, first_texture, None);
                self.commit_command_buffer(screenshot, None, command_buffer, gpu_read_guards);
                let () = unsafe {msg_send![command_buffer, waitUntilScheduled]};
                let () = unsafe {msg_send![drawable, present]};
            }
        }
        let () = unsafe {msg_send![pool, release]};
    }
    
    fn build_screenshot_struct(&mut self, metal_cx:&MetalCx, command_buffer:ObjcId, kind_id: usize, width: usize, height: usize, in_texture:ObjcId, alloc:Option<TextureAlloc>)->Option<ScreenshotInfo>{
        let mut request_ids = Vec::new();
        self.screenshot_requests.retain(|v|{
            if v.kind_id == kind_id as u32{
                request_ids.push(v.request_id);
                false
            }
            else{
                true
            }
        });
        let (tex_width,tex_height) = if let Some(alloc) = alloc{
            (alloc.width, alloc.height)
        }else{
            (width, height)
        };
        if request_ids.len() > 0{
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
            let _: () = unsafe {msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setWidth: tex_width as u64]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setHeight: tex_height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setPixelFormat: MTLPixelFormat::BGRA8Unorm]};
            let texture:ObjcId = unsafe{msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]};
            unsafe{
                let blit_encoder: ObjcId = msg_send![command_buffer, blitCommandEncoder];
                let () = msg_send![blit_encoder, copyFromTexture: in_texture toTexture:texture];
                let () = msg_send![blit_encoder, synchronizeTexture: texture slice:0 level:0];
                let () = msg_send![blit_encoder, endEncoding];
            };
            return Some(ScreenshotInfo{
                request_ids,
                width: width as _, 
                height: height as _,
                texture: texture
            })
        }
        None
    }
        
    fn commit_command_buffer(&self, screenshot_info: Option<ScreenshotInfo>, stdin_frame: Option<PresentableDraw>, command_buffer: ObjcId, gpu_read_guards: Vec<MetalRwLockGpuReadGuard>) {
        let gpu_read_guards = Mutex::new(Some(gpu_read_guards));
        let screenshot_info =  Mutex::new(screenshot_info);
        //let present_index = Arc::clone(&self.os.present_index);
        //Self::stdin_send_draw_complete(&present_index);
        let start_time = self.os.start_time.unwrap();
        let () = unsafe {msg_send![
            command_buffer,
            addCompletedHandler: &objc_block!(move | command_buffer: ObjcId | {
                // alright lets grab a texture if need be
                if let Some(sf) = &*screenshot_info.lock().unwrap(){
                    let mut buf = Vec::new();
                    buf.resize(sf.width * sf.height * 4, 0u8);
                    let region = MTLRegion {
                        origin: MTLOrigin {x: 0, y: 0, z: 0},
                        size: MTLSize {width: sf.width as u64, height: sf.height as u64, depth: 1}
                    };
                    let _:() = unsafe{msg_send![
                        sf.texture, 
                        getBytes: buf.as_ptr()
                        bytesPerRow: sf.width *4
                        bytesPerImage: sf.width * sf.height * 4
                        fromRegion: region
                        mipmapLevel: 0
                        slice: 0
                    ]};
                    let () = msg_send![sf.texture, release];
                    Self::send_studio_message(AppToStudio::Screenshot(StudioScreenshotResponse{
                        request_ids: sf.request_ids.clone(),
                        image: Some(buf),
                        width: sf.width as _, 
                        height: sf.height as _,
                    }))
                }
                
                let start:f64 = unsafe {msg_send![command_buffer, GPUStartTime]};
                let end:f64 = unsafe {msg_send![command_buffer, GPUEndTime]};
                if let Some(_stdin_frame) = stdin_frame {
                    #[cfg(target_os = "macos")]
                    Self::stdin_send_draw_complete(_stdin_frame);
                }
                // lets send off our gpu time
                let duration = end - start;
                let start = Instant::now().duration_since(start_time).as_secs_f64() - duration;
                let end = start + duration;
                Cx::send_studio_message(AppToStudio::GPUSample(GPUSample{
                    start, end
                }));
                
                drop(gpu_read_guards.lock().unwrap().take().unwrap());
            })
        ]};
        let () = unsafe {msg_send![command_buffer, commit]};
    } 
    
    pub (crate) fn mtl_compile_shaders(&mut self, metal_cx: &MetalCx) {
        for draw_shader_id in self.draw_shaders.compile_set.iter().cloned().collect::<Vec<_>>() {
            let cx_shader = &self.draw_shaders.shaders[draw_shader_id];
            
            let mtlsl = match &cx_shader.mapping.code {
                CxDrawShaderCode::Combined { code } => code.clone(),
                CxDrawShaderCode::Separate { .. } => {
                    crate::error!("Metal does not support separate vertex/fragment sources");
                    continue;
                }
            };
            
            if cx_shader.mapping.flags.debug {
                println!("=== Generated Metal Shader ===\n{}\n=== End Metal Shader ===", mtlsl);
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
    
    #[cfg(target_os="macos")]
    pub fn share_texture_for_presentable_image(
        &mut self,
        texture: &Texture,
    ) -> crate::cx_stdin::SharedPresentableImageOsHandle {
        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.metal_device.unwrap());

        // HACK(eddyb) macOS has no real `SharedPresentableImageOsHandle` because
        // the texture is actually shared through an XPC helper service instead,
        // based entirely on its `PresentableImageId`.
        crate::cx_stdin::SharedPresentableImageOsHandle {
            _dummy_for_macos: None,
        }
    }
    
    #[cfg(any(target_os="ios", target_os="tvos"))]
    pub fn share_texture_for_presentable_image(
        &mut self,
        _texture: &Texture,
    ) -> crate::cx_stdin::SharedPresentableImageOsHandle {
        crate::cx_stdin::SharedPresentableImageOsHandle {
            _dummy_for_unsupported: None,
        }
    }
}

#[derive(Clone)]
struct ScreenshotInfo{
    width: usize,
    height: usize,
    request_ids: Vec<u64>,
    texture: ObjcId,
}

pub enum DrawPassMode {
    Texture,
    MTKView(ObjcId),
    StdinMain(PresentableDraw, usize),
    Drawable(ObjcId),
    Resizing(ObjcId)
}

impl DrawPassMode {
    fn is_drawable(&self) -> Option<ObjcId> {
        match self {
            Self::Drawable(obj) | Self::Resizing(obj) => Some(*obj),
            Self::StdinMain(_,_) | Self::Texture | Self::MTKView(_) => None
        }
    }
}

pub struct MetalCx {
    pub device: ObjcId,
    command_queue: ObjcId
}


#[derive(Clone, Default)]
pub struct CxOsDrawList {
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    mtl_depth_state: Option<ObjcId>
}

pub enum PackType {
    Packed,
    Unpacked
}
/*
pub struct SlErr {
    _msg: String
}*/

impl MetalCx {
    
    pub (crate) fn new() -> MetalCx {
        let device = get_default_metal_device().expect("Cannot get default metal device");
        MetalCx {
            command_queue: unsafe {msg_send![device, newCommandQueue]},
            device: device
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
impl DrawVars{
    pub (crate) fn compile_shader(&mut self, vm:&mut ScriptVm, _apply:&Apply, value:ScriptValue){
        // Shader caching strategy:
        // 1. Check object_id cache (fastest - exact same object)
        // 2. Check function hash cache (same functions even if different object instance)
        // 3. Check code cache (different functions but identical generated code)
        
        if let Some(io_self) = value.as_object(){
            // Cache 1: Check if this exact object has been compiled before
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&io_self) {
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }
            
            // Cache 2: Compute function hash and check if we've seen these functions before
            let fnhash = DrawVars::compute_shader_functions_hash(&vm.heap, io_self);
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_functions_to_shader.get(&fnhash) {
                    // Add to object_id cache for faster lookup next time
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders.cache_object_id_to_shader.insert(io_self, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }
            
            // Not in function cache, need to compile
            let mut output = ShaderOutput::default();
            output.backend = ShaderBackend::Metal;
                                    
            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);
            
            if let Some(fnobj) = vm.heap.object_method(io_self, id!(vertex).into(), vm.thread.trap.pass()).as_object(){
                output.mode = ShaderMode::Vertex;
                ShaderFnCompiler::compile_shader_def(
                    vm, 
                    &mut output, 
                    id!(vertex), 
                    fnobj, 
                    ShaderType::IoSelf(io_self), 
                    vec![],
                );
            }
            if let Some(fnobj) = vm.heap.object_method(io_self, id!(fragment).into(), vm.thread.trap.pass()).as_object(){
                output.mode = ShaderMode::Fragment;
                ShaderFnCompiler::compile_shader_def(
                    vm, 
                    &mut output, 
                    id!(fragment), 
                    fnobj, 
                    ShaderType::IoSelf(io_self), 
                    vec![],
                );
            }
            
            // Assign buffer indices to uniform buffers before generating Metal code
            // Buffer indices start at 3 (0=vertex buffer, 1=instance buffer, 2=uniform struct)
            output.assign_uniform_buffer_indices(vm.heap, 3);
            
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
            
            let source = vm.heap.new_object_ref(io_self);
            
            // Create the shader mapping and allocate CxDrawShader
            let code = CxDrawShaderCode::Combined { code: out };
            
            // Cache 3: Check if this exact code has been compiled before
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_code_to_shader.get(&code) {
                    // Add to both object_id and function hash caches
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders.cache_object_id_to_shader.insert(io_self, shader_id);
                    cx.draw_shaders.cache_functions_to_shader.insert(fnhash, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }
            
            // Extract geometry_id from the vertex buffer object before creating the mapping
            let geometry_id = if let Some(vb_obj) = output.find_vertex_buffer_object(vm, io_self) {
                let buffer_value = vm.heap.value(vb_obj, id!(buffer).into(), vm.thread.trap.pass());
                if let Some(handle) = buffer_value.as_handle() {
                    vm.heap.handle_ref::<Geometry>(handle).map(|g| g.geometry_id())
                } else {
                    None
                }
            } else {
                None
            };
            
            let mut mapping = CxDrawShaderMapping::from_shader_output(source, code.clone(), &vm.heap, &output, geometry_id);
            
            // Fill the scope uniform buffer from current script values
            mapping.fill_scope_uniforms_buffer(
                &vm.heap,
                &vm.thread.trap.pass(),
            );
            
            // Check for debug: true on the shader object
            let debug_value = vm.heap.value(io_self, id!(debug).into(), vm.thread.trap.pass());
            if let Some(true) = debug_value.as_bool() {
                mapping.flags.debug = true;
            }
            
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
            cx.draw_shaders.cache_object_id_to_shader.insert(io_self, shader_id);
            cx.draw_shaders.cache_functions_to_shader.insert(fnhash, shader_id);
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
    pub (crate) fn new(
        metal_cx: &MetalCx,
        mtlsl: String,
        bindings: &UniformBufferBindings,
    ) -> Option<Self> {
        let options = RcObjcId::from_owned(unsafe {msg_send![class!(MTLCompileOptions), new]});
        unsafe {
            let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
        };
        
        let mut error: ObjcId = nil;
        let library = RcObjcId::from_owned(match NonNull::new(unsafe {
            msg_send![
                metal_cx.device,
                newLibraryWithSource: str_to_nsstring(&mtlsl)
                options: options
                error: &mut error
            ]
        }) {
            Some(library) => library,
            None => {
                let description: ObjcId = unsafe {msg_send![error, localizedDescription]};
                let string = nsstring_to_string(description);
                let mut out = format!("{}\n", string);
                for (index, line) in mtlsl.split("\n").enumerate() {
                    out.push_str(&format!("{}: {}\n", index + 1, line));
                }
                crate::error!("{}", out);
                return None;
            }
        });
        
        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![class!(MTLRenderPipelineDescriptor), new]
        }).unwrap());
        
        let vertex_function = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![library.as_id(), newFunctionWithName: str_to_nsstring("vertex_main")]
        }).unwrap());
        
        let fragment_function = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![library.as_id(), newFunctionWithName: str_to_nsstring("fragment_main")]
        }).unwrap());
        
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
        let draw_call_uniform_buffer_id = bindings.get_by_type_name(id!(DrawCallUniforms)).map(|i| i as u64);
        let pass_uniform_buffer_id = bindings.get_by_type_name(id!(DrawPassUniforms)).map(|i| i as u64);
        let draw_list_uniform_buffer_id = bindings.get_by_type_name(id!(DrawListUniforms)).map(|i| i as u64);
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
    //pub uni_dr: MetalBuffer,
    instance_buffer: MetalBufferQueue,
}

#[derive(Default)]
pub struct CxOsGeometry {
    vertex_buffer: MetalBufferQueue,
    index_buffer: MetalBufferQueue,
}

#[derive(Default)]
struct MetalBufferQueue {
    queue: [MetalRwLock<MetalBuffer>; 3],
    index: usize,
}

impl MetalBufferQueue {
    fn get(&self) -> &MetalRwLock<MetalBuffer> {
        &self.queue[self.index]
    }
    
    fn get_mut(&mut self) -> &mut MetalRwLock<MetalBuffer> {
        &mut self.queue[self.index]
    }
    
    fn next(&mut self) {
        self.index = (self.index + 1) % self.queue.len();
    }
}

#[derive(Default)]
struct MetalBuffer {
    inner: Option<MetalBufferInner>,
}

impl MetalBuffer {
    fn update<T>(&mut self, metal_cx: &MetalCx, data: &[T]) where T: std::fmt::Debug {
        let len = data.len() * std::mem::size_of::<T>();
        if len == 0 {
            self.inner = None;
            return;
        }
        if self.inner.as_ref().map_or(0, | inner | inner.len) < len {
            self.inner = Some(MetalBufferInner {
                len,
                buffer: RcObjcId::from_owned(NonNull::new(unsafe {
                    msg_send![
                        metal_cx.device,
                        newBufferWithLength: len as u64
                        options: nil
                    ]
                }).unwrap())
            });
        }
        let inner = self.inner.as_ref().unwrap();
        unsafe {
            let contents: *mut u8 = msg_send![inner.buffer.as_id(), contents];
            
            //println!("Buffer write {} buf {} data {:?}", command_buffer as *const _ as u64, inner.buffer.as_id() as *const _ as u64, data);
            
            std::ptr::copy(data.as_ptr() as *const u8, contents, len);
            /*
            let _: () = msg_send![
                inner.buffer.as_id(),
                didModifyRange: NSRange {
                    location: 0,
                    length: len as u64
                }
            ];*/
        }
    }
}

struct MetalBufferInner {
    len: usize,
    buffer: RcObjcId,
}

#[derive(Default)]
pub struct CxOsTexture {
    texture: Option<RcObjcId>
}
fn texture_pixel_to_mtl_pixel(pix:&TexturePixel)-> MTLPixelFormat {
     match pix{
         TexturePixel::BGRAu8 => MTLPixelFormat::BGRA8Unorm,
         TexturePixel::RGBAf16 => MTLPixelFormat::RGBA16Float,
         TexturePixel::RGBAf32 => MTLPixelFormat::RGBA32Float,
         TexturePixel::Ru8  => MTLPixelFormat::R8Unorm,
         TexturePixel::RGu8  => MTLPixelFormat::RG8Unorm,
         TexturePixel::Rf32  => MTLPixelFormat::R32Float,
         TexturePixel::D32 => MTLPixelFormat::Depth32Float,
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
    
    fn update_vec_texture(
        &mut self,
        metal_cx: &MetalCx,
    ) {
        if self.alloc_vec() {
            let alloc = self.alloc.as_ref().unwrap();
            
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
            let _: () = unsafe {msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
            let _: () = unsafe {msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]};
            let texture:ObjcId = unsafe{msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]};
            self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
        }
        let update = self.take_updated();
        if update.is_empty(){
            return;
        }
        fn update_data(texture:&Option<RcObjcId>, width: usize, height: usize, bpp: u64, data: *const std::ffi::c_void){

            let region = MTLRegion {
                origin: MTLOrigin {x: 0, y: 0, z: 0},
                size: MTLSize {width: width as u64, height: height as u64, depth: 1}
            };
                                            
            let () = unsafe {msg_send![
                texture.as_ref().unwrap().as_id(),
                replaceRegion: region
                mipmapLevel: 0
                withBytes: data
                bytesPerRow: (width as u64) * bpp
            ]};
        }
        match &self.format{
            TextureFormat::VecBGRAu8_32{width, height, data, ..}=>{
                update_data(&self.os.texture, *width, *height, 4,  data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void);
            }
            TextureFormat::VecRGBAf32{width, height, data, ..}=>{
                update_data(&self.os.texture, *width, *height, 16,  data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void);
            }
            TextureFormat::VecRu8{width, height, data, ..}=>{
                update_data(&self.os.texture, *width, *height, 1,  data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void);
            }
            TextureFormat::VecRGu8{width, height, data, ..}=>{
                update_data(&self.os.texture, *width, *height, 2,  data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void);
            }
            TextureFormat::VecRf32{width, height, data, ..}=>{
                update_data(&self.os.texture, *width, *height, 4,  data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void);
            }
            _=>panic!()
        }
    }
    
    #[cfg(target_os = "macos")]
    fn update_shared_texture(
        &mut self,
        metal_device: ObjcId,
    ) {
        // we need a width/height for this one.
        if !self.alloc_shared(){
            return
        }
        let alloc = self.alloc.as_ref().unwrap();
        let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![class!(MTLTextureDescriptor), new]
        }).unwrap());
            
        let _: () = unsafe{msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setDepth: 1u64]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget]};
        let _: () = unsafe{msg_send![descriptor.as_id(), setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]};
        match &self.format {
            TextureFormat::SharedBGRAu8{id, ..} => {
                let texture: ObjcId = unsafe{msg_send![metal_device, newSharedTextureWithDescriptor: descriptor]};
                let shared: ObjcId = unsafe{msg_send![texture, newSharedTextureHandle]};
                store_xpc_service_texture(*id, shared);
                let _: () = unsafe{msg_send![shared, release]};
                self.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
            }
            _ => panic!(),
        }
    }
    
    #[cfg(target_os = "macos")]
    pub fn update_from_shared_handle(
        &mut self,
        metal_cx: &MetalCx,
        shared_handle: ObjcId,
    ) -> bool {
        // we need a width/height for this one.
        if !self.alloc_shared(){
            return true
        }
        let alloc = self.alloc.as_ref().unwrap();
    
        let texture = RcObjcId::from_owned(NonNull::new(unsafe {
            msg_send![metal_cx.device, newSharedTextureWithHandle: shared_handle]
        }).unwrap());
        let width: u64 = unsafe{msg_send![texture.as_id(), width]};
        let height: u64 = unsafe{msg_send![texture.as_id(), height]};
        // FIXME(eddyb) can these be an assert now?
        if width != alloc.width as u64|| height != alloc.height as u64{
            return false
        }
        self.os.texture = Some(texture);
        true
    }
    
    fn update_render_target(
        &mut self,
        metal_cx: &MetalCx,
        width: usize,
        height: usize
    ) {
        if self.alloc_render(width, height){
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
            
            let _: () = unsafe{msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget]};
            let _: () = unsafe{msg_send![descriptor.as_id(),setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)]};
            let texture = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
            }).unwrap());
            
            self.os.texture = Some(texture); 
        }
    }
    
    
    fn update_depth_stencil(
        &mut self,
        metal_cx: &MetalCx,
        width: usize,
        height: usize
    ) {
        if self.alloc_depth(width, height){
       
            let alloc = self.alloc.as_ref().unwrap();
            let descriptor = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![class!(MTLTextureDescriptor), new]
            }).unwrap());
                        
            let _: () = unsafe{msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setWidth: alloc.width as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setHeight: alloc.height as u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setDepth: 1u64]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Private]};
            let _: () = unsafe{msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::RenderTarget]};
            let _: () = unsafe{msg_send![
                descriptor.as_id(),
                setPixelFormat: texture_pixel_to_mtl_pixel(&alloc.pixel)
            ]};
            let texture = RcObjcId::from_owned(NonNull::new(unsafe {
                msg_send![metal_cx.device, newTextureWithDescriptor: descriptor]
            }).unwrap());
            self.os.texture = Some(texture);
        }
    }    
}

#[derive(Default)]
struct MetalRwLock<T> {
    inner: Arc<MetalRwLockInner>,
    value: T
}

impl<T> MetalRwLock<T> {
    fn cpu_read(&self) -> &T {
        &self.value
    }
    
    fn gpu_read(&self) -> MetalRwLockGpuReadGuard {
        let mut reader_count = self.inner.reader_count.lock().unwrap();
        *reader_count += 1;
        MetalRwLockGpuReadGuard {
            inner: self.inner.clone()
        }
    }
    
    fn cpu_write(&mut self) -> &mut T {
        let mut reader_count = self.inner.reader_count.lock().unwrap();
        while *reader_count != 0 {
            reader_count = self.inner.condvar.wait(reader_count).unwrap();
        }
        &mut self.value
    }
}

#[derive(Default)]
struct MetalRwLockInner {
    reader_count: Mutex<usize>,
    condvar: Condvar,
}

struct MetalRwLockGpuReadGuard {
    inner: Arc<MetalRwLockInner>
}

impl Drop for MetalRwLockGpuReadGuard {
    fn drop(&mut self) {
        let mut reader_count = self.inner.reader_count.lock().unwrap();
        *reader_count -= 1;
        if *reader_count == 0 {
            self.inner.condvar.notify_one();
        }
    }
}

pub fn get_default_metal_device() -> Option<ObjcId> {
    unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev == nil {None} else {Some(dev)}
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
            .map( | i | msg_send![array, objectAtIndex: i])
        // The elements of this array are references---we convert them to owned references
        // (which just means that we increment the reference count here, and it is
        // decremented in the `Drop` impl for `Device`)
            .map( | device: *mut Object | msg_send![device, retain])
            .collect();
        let () = msg_send![array, release];
        ret
    }
}


