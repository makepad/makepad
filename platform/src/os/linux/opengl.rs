use {
    self::super::gl_sys,
    crate::{
        cx::{Cx, OsType},
        draw_list::DrawListId,
        draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
        draw_shader::{
            CxDrawShader, CxDrawShaderCode, CxDrawShaderMapping, DrawShaderAttrFormat,
            DrawShaderId, DrawShaderTextureInput,
        },
        draw_vars::DrawVars,
        event::{Event, TextureHandleReadyEvent},
        geometry::Geometry,
        makepad_live_id::*,
        makepad_math::{Vec2d, Vec4f},
        makepad_script::{
            apply::Apply,
            shader::{
                SamplerAddress, SamplerFilter, ShaderFnCompiler, ShaderMode, ShaderOutput,
                ShaderType,
            },
            shader_backend::ShaderBackend,
            shader_output::TextureType,
            trap::NoTrap,
            value::ScriptValue,
            ScriptVm,
        },
        script::vm::ScriptVmCx,
        texture::{CxTexture, Texture, TextureFormat, TexturePixel, TextureUpdated},
    },
    gl_sys::LibGl,
    std::{
        ffi::{c_char, CStr},
        fs::{remove_file, File},
        io::prelude::*,
        mem, ptr,
    },
};

#[cfg(use_vulkan)]
use crate::os::linux::vulkan_naga::CxVulkanShaderBinary;

impl DrawVars {
    pub(crate) fn compile_shader(&mut self, vm: &mut ScriptVm, _apply: &Apply, value: ScriptValue) {
        if let Some(io_self) = value.as_object() {
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&io_self) {
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let fnhash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, io_self);
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_functions_to_shader.get(&fnhash) {
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let mut output = ShaderOutput::default();
            output.backend = ShaderBackend::Glsl;
            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);

            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(vertex).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Vertex;
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

            if output.has_errors {
                return;
            }

            output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

            #[cfg(use_vulkan)]
            let mut compiled_vulkan_shader: Option<CxVulkanShaderBinary> = None;

            #[cfg(use_vulkan)]
            {
                match crate::os::linux::vulkan_naga::compile_draw_shader_wgsl_to_spirv(
                    vm,
                    io_self,
                    &output,
                ) {
                    Ok(vk_shader) => compiled_vulkan_shader = Some(vk_shader),
                    Err(err) => {
                        use std::sync::atomic::{AtomicUsize, Ordering};
                        static ERROR_COUNT: AtomicUsize = AtomicUsize::new(0);
                        const MAX_ERROR_LOGS: usize = 1;
                        let index = ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
                        if index < MAX_ERROR_LOGS {
                            crate::error!("Vulkan WGSL/SPIR-V compilation failed: {}", err);
                        } else if index == MAX_ERROR_LOGS {
                            crate::warning!(
                                "Suppressing further Vulkan WGSL/SPIR-V compilation logs after {} errors",
                                MAX_ERROR_LOGS
                            );
                        }
                    }
                }
            }

            if std::env::var_os("MAKEPAD_DUMP_GLSL_IR").is_some() {
                crate::log!("---- Linux GLSL IR io list ----");
                for io in &output.io {
                    crate::log!("io kind={:?} name={} ty={:?}", io.kind, io.name, io.ty);
                }
                crate::log!("---- Linux GLSL IR functions ----");
                for f in &output.functions {
                    crate::log!("{} {{\n{}\n}}", f.call_sig, f.out);
                }
            }

            let mut shared_defs = String::new();
            output.create_struct_defs(vm, &mut shared_defs);

            let mut vertex = String::new();
            let mut fragment = String::new();
            output.glsl_create_vertex_shader(vm, &shared_defs, &mut vertex);
            output.glsl_create_fragment_shader(vm, &shared_defs, &mut fragment);

            let code = CxDrawShaderCode::Separate {
                vertex: vertex.clone(),
                fragment: fragment.clone(),
            };

            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_code_to_shader.get(&code) {
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

            let geometry_id = if let Some(vb_obj) = output.find_vertex_buffer_object(vm, io_self) {
                let buffer_value =
                    vm.bx
                        .heap
                        .value(vb_obj, id!(buffer).into(), vm.thread().trap.pass());
                if let Some(handle) = buffer_value.as_handle() {
                    vm.bx
                        .heap
                        .handle_ref::<Geometry>(handle)
                        .map(|g: &Geometry| g.geometry_id())
                } else {
                    None
                }
            } else {
                None
            };

            let source = vm.bx.heap.new_object_ref(io_self);
            let mut mapping = CxDrawShaderMapping::from_shader_output(
                source,
                code.clone(),
                &vm.bx.heap,
                &output,
                geometry_id,
            );
            mapping.fill_scope_uniforms_buffer(&vm.bx.heap, &vm.thread().trap.pass());

            let debug_value = vm.bx.heap.value(io_self, id!(debug).into(), NoTrap);
            if let Some(true) = debug_value.as_bool() {
                mapping.flags.debug = true;
            }

            self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
            self.dyn_instance_slots = mapping.instances.total_slots;

            let mut os_shader_id = None;

            #[cfg(use_vulkan)]
            if let Some(vk_shader) = compiled_vulkan_shader.clone() {
                let cx = vm.host.cx_mut();
                for (shader_index, os_shader) in cx.draw_shaders.os_shaders.iter_mut().enumerate() {
                    if os_shader.in_vertex == vertex && os_shader.in_pixel == fragment {
                        os_shader.vulkan_shader = Some(vk_shader.clone());
                        os_shader_id = Some(shader_index);
                        break;
                    }
                }
                if os_shader_id.is_none() {
                    let mut os_shader =
                        CxOsDrawShader::new(cx.os.gl(), &vertex, &fragment, &cx.os_type);
                    os_shader.vulkan_shader = Some(vk_shader);
                    os_shader_id = Some(cx.draw_shaders.os_shaders.len());
                    cx.draw_shaders.os_shaders.push(os_shader);
                }
            }

            let cx = vm.host.cx_mut();
            let index = cx.draw_shaders.shaders.len();
            cx.draw_shaders.shaders.push(CxDrawShader {
                debug_id: LiveId(0),
                os_shader_id,
                mapping,
            });

            let shader_id = DrawShaderId { index };
            cx.draw_shaders
                .cache_object_id_to_shader
                .insert(io_self, shader_id);
            cx.draw_shaders
                .cache_functions_to_shader
                .insert(fnhash, shader_id);
            cx.draw_shaders.cache_code_to_shader.insert(code, shader_id);
            if os_shader_id.is_none() {
                cx.draw_shaders.compile_set.insert(index);
            }

            self.draw_shader_id = Some(shader_id);
            self.geometry_id = geometry_id;
        }
    }
}

impl Cx {
    pub(crate) fn render_view(
        &mut self,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
    ) {
        let mut to_dispatch = Vec::new();
        //self.draw_lists[draw_list_id].draw_list_uniforms.view_transform = Mat4f::identity();
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();

        let draw_list = &mut self.draw_lists[draw_list_id];
        draw_list
            .os
            .draw_list_uniforms
            .update_uniform_buffer(self.os.gl(), draw_list.draw_list_uniforms.as_slice());

        for draw_item_id in 0..draw_items_len {
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id]
                .kind
                .sub_list()
            {
                self.render_view(draw_pass_id, sub_list_id, zbias, zbias_step);
            } else {
                let gl = self.os.gl();

                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];

                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                } else {
                    continue;
                };

                let sh = &self.draw_shaders.shaders[draw_call.draw_shader_id.index];
                if sh.os_shader_id.is_none() {
                    // shader didnt compile somehow
                    continue;
                }
                if sh.mapping.uses_time {
                    self.demo_time_repaint = true;
                }
                let shp = &mut self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];

                let shader_variant = self.passes[draw_pass_id].os.shader_variant;

                if shp.gl_shader[shader_variant].is_none() {
                    shp.gl_shader[shader_variant] = Some(GlShader::new(
                        self.os.gl(),
                        &shp.vertex[shader_variant],
                        &shp.pixel[shader_variant],
                        &sh.mapping,
                        &self.os_type,
                    ));
                }
                let shgl = shp.gl_shader[shader_variant].as_ref().unwrap();
                let trace_draw = std::env::var_os("MAKEPAD_GL_DRAW_TRACE").is_some();

                if draw_call.instance_dirty || draw_item.os.inst_vb.gl_buffer.is_none() {
                    draw_call.instance_dirty = false;
                    draw_item
                        .os
                        .inst_vb
                        .update_array_buffer(gl, draw_item.instances.as_ref().unwrap());
                }

                // update the zbias uniform if we have it.
                draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;

                draw_item
                    .os
                    .draw_call_uniforms
                    .update_uniform_buffer(gl, draw_call.draw_call_uniforms.as_slice());

                let instances = (draw_item.instances.as_ref().unwrap().len()
                    / sh.mapping.instances.total_slots) as u64;

                if instances == 0 {
                    continue;
                }

                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {
                    geometry_id
                } else {
                    continue;
                };

                let geometry = &mut self.geometries[geometry_id];
                if geometry.dirty
                    || geometry.os.vb.gl_buffer.is_none()
                    || geometry.os.ib.gl_buffer.is_none()
                {
                    geometry.os.vb.update_array_buffer(gl, &geometry.vertices);
                    geometry.os.ib.update_index_buffer(gl, &geometry.indices);
                    geometry.dirty = false;
                }

                let indices = geometry.indices.len();

                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    draw_item
                        .os
                        .draw_call_uniforms
                        .update_uniform_buffer(gl, draw_call.draw_call_uniforms.as_slice());
                    draw_item
                        .os
                        .user_uniforms
                        .update_uniform_buffer(gl, draw_call.dyn_uniforms.as_slice());
                }

                // update geometry?
                let geometry = &mut self.geometries[geometry_id];

                // lets check if our vao is still valid
                if draw_item.os.vao.is_none() {
                    draw_item.os.vao = Some(CxOsDrawCallVao {
                        vao: None,
                        shader_id: None,
                        inst_vb: None,
                        geom_vb: None,
                        geom_ib: None,
                    });
                }

                let vao = draw_item.os.vao.as_mut().unwrap();
                if vao.inst_vb != draw_item.os.inst_vb.gl_buffer
                    || vao.geom_vb != geometry.os.vb.gl_buffer
                    || vao.geom_ib != geometry.os.ib.gl_buffer
                    || vao.shader_id != Some(draw_call.draw_shader_id.index)
                {
                    if let Some(vao) = vao.vao.take() {
                        unsafe { (gl.glDeleteVertexArrays)(1, &vao) };
                    }

                    vao.vao = Some(unsafe {
                        let mut vao = 0u32;
                        (gl.glGenVertexArrays)(1, &mut vao);
                        vao
                    });

                    vao.shader_id = Some(draw_call.draw_shader_id.index);
                    vao.inst_vb = draw_item.os.inst_vb.gl_buffer;
                    vao.geom_vb = geometry.os.vb.gl_buffer;
                    vao.geom_ib = geometry.os.ib.gl_buffer;
                    unsafe {
                        (gl.glBindVertexArray)(vao.vao.unwrap());
                        // bind the vertex and indexbuffers
                        (gl.glBindBuffer)(gl_sys::ARRAY_BUFFER, vao.geom_vb.unwrap());
                        for attr in &shgl.geometries {
                            if let Some(loc) = attr.loc {
                                match attr.attr_format {
                                    DrawShaderAttrFormat::Float => {
                                        (gl.glVertexAttribPointer)(
                                            loc,
                                            attr.size,
                                            gl_sys::FLOAT,
                                            0,
                                            attr.stride,
                                            attr.offset as *const () as *const _,
                                        );
                                    }
                                    DrawShaderAttrFormat::UInt => {
                                        (gl.glVertexAttribIPointer)(
                                            loc,
                                            attr.size,
                                            gl_sys::UNSIGNED_INT,
                                            attr.stride,
                                            attr.offset as *const () as *const _,
                                        );
                                    }
                                    DrawShaderAttrFormat::SInt => {
                                        (gl.glVertexAttribIPointer)(
                                            loc,
                                            attr.size,
                                            gl_sys::INT,
                                            attr.stride,
                                            attr.offset as *const () as *const _,
                                        );
                                    }
                                }
                                (gl.glEnableVertexAttribArray)(loc);
                            }
                        }
                        (gl.glBindBuffer)(gl_sys::ARRAY_BUFFER, vao.inst_vb.unwrap());
                        for attr in &shgl.instances {
                            if let Some(loc) = attr.loc {
                                match attr.attr_format {
                                    DrawShaderAttrFormat::Float => {
                                        (gl.glVertexAttribPointer)(
                                            loc,
                                            attr.size,
                                            gl_sys::FLOAT,
                                            0,
                                            attr.stride,
                                            attr.offset as *const () as *const _,
                                        );
                                    }
                                    DrawShaderAttrFormat::UInt => {
                                        (gl.glVertexAttribIPointer)(
                                            loc,
                                            attr.size,
                                            gl_sys::UNSIGNED_INT,
                                            attr.stride,
                                            attr.offset as *const () as *const _,
                                        );
                                    }
                                    DrawShaderAttrFormat::SInt => {
                                        (gl.glVertexAttribIPointer)(
                                            loc,
                                            attr.size,
                                            gl_sys::INT,
                                            attr.stride,
                                            attr.offset as *const () as *const _,
                                        );
                                    }
                                }
                                (gl.glEnableVertexAttribArray)(loc);
                                (gl.glVertexAttribDivisor)(loc, 1 as gl_sys::GLuint);
                            }
                        }

                        // bind the indexbuffer
                        (gl.glBindBuffer)(gl_sys::ELEMENT_ARRAY_BUFFER, vao.geom_ib.unwrap());
                        (gl.glBindVertexArray)(0);
                        (gl.glBindBuffer)(gl_sys::ARRAY_BUFFER, 0);
                        (gl.glBindBuffer)(gl_sys::ELEMENT_ARRAY_BUFFER, 0);
                    }
                    if trace_draw {
                        crate::log!(
                            "GL VAO rebuilt shader={} vao={:?} geom_vb={:?} inst_vb={:?} geom_ib={:?}",
                            draw_call.draw_shader_id.index,
                            vao.vao,
                            vao.geom_vb,
                            vao.inst_vb,
                            vao.geom_ib
                        );
                    }
                }
                unsafe {
                    (gl.glUseProgram)(shgl.program);
                    (gl.glBindVertexArray)(draw_item.os.vao.as_ref().unwrap().vao.unwrap());
                    let instances = (draw_item.instances.as_ref().unwrap().len()
                        / sh.mapping.instances.total_slots)
                        as u64;
                    // bind all uniform buffers
                    {
                        shgl.uniforms
                            .pass_uniforms_binding
                            .bind_buffer(gl, &self.passes[draw_pass_id].os.pass_uniforms);
                        shgl.uniforms
                            .draw_list_uniforms_binding
                            .bind_buffer(gl, &draw_list.os.draw_list_uniforms);
                        shgl.uniforms
                            .draw_call_uniforms_binding
                            .bind_buffer(gl, &draw_item.os.draw_call_uniforms);
                        shgl.uniforms
                            .user_uniforms_binding
                            .bind_buffer(gl, &draw_item.os.user_uniforms);
                        shgl.uniforms
                            .live_uniforms_binding
                            .bind_buffer(gl, &shgl.uniforms.live_uniforms);
                    }

                    // give openXR a chance to set its depth texture
                    #[cfg(target_os = "android")]
                    if self.os.in_xr_mode {
                        self.os.openxr.depth_texture_hook(gl, shgl, &sh.mapping)
                    };

                    for i in 0..sh.mapping.textures.len() {
                        let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                            texture.texture_id()
                        } else {
                            continue;
                        };
                        let cxtexture = &mut self.textures[texture_id];

                        if cxtexture.format.is_vec() {
                            cxtexture.update_vec_texture(gl, &self.os_type);
                        } else if cxtexture.format.is_video() {
                            let is_initial_setup = cxtexture.setup_video_texture(gl);
                            if is_initial_setup {
                                let e = Event::TextureHandleReady(TextureHandleReadyEvent {
                                    texture_id,
                                    handle: cxtexture.os.gl_texture.unwrap(),
                                });
                                to_dispatch.push(e);
                            }
                        }
                    }
                    for i in 0..sh.mapping.textures.len() {
                        let gl = self.os.gl();
                        (gl.glActiveTexture)(gl_sys::TEXTURE0 + i as u32);

                        let expected_target =
                            if matches!(
                                sh.mapping.textures[i].tex_type,
                                TextureType::TextureCube | TextureType::TextureCubeArray
                            ) {
                                gl_sys::TEXTURE_CUBE_MAP
                            } else {
                                gl_sys::TEXTURE_2D
                            };

                        if let Some(texture) = &draw_call.texture_slots[i] {
                            let texture_id = texture.texture_id();
                            let cxtexture = &mut self.textures[texture_id];
                            let bind_target = match cxtexture.format {
                                TextureFormat::VideoRGB => gl_sys::TEXTURE_EXTERNAL_OES,
                                TextureFormat::VecCubeBGRAu8_32 { .. } => gl_sys::TEXTURE_CUBE_MAP,
                                _ => gl_sys::TEXTURE_2D,
                            };
                            if let Some(texture) = cxtexture.os.gl_texture {
                                (gl.glBindTexture)(bind_target, texture);
                            } else {
                                (gl.glBindTexture)(bind_target, 0);
                            }
                        } else {
                            (gl.glBindTexture)(expected_target, 0);
                        }

                        if let Some(loc) = shgl.textures[i].loc {
                            (gl.glUniform1i)(loc, i as i32);
                        }
                        if let Some(gl_bind_sampler) = gl.glBindSampler {
                            let sampler = shgl
                                .samplers
                                .get(i)
                                .and_then(|sampler| sampler.sampler)
                                .unwrap_or(0);
                            gl_bind_sampler(i as u32, sampler);
                        }
                    }
                    if trace_draw {
                        crate::log!(
                            "GL draw shader={} variant={} indices={} instances={} textures={}",
                            draw_call.draw_shader_id.index,
                            shader_variant,
                            indices,
                            instances,
                            sh.mapping.textures.len()
                        );
                    }

                    (gl.glDrawElementsInstanced)(
                        gl_sys::TRIANGLES,
                        indices as i32,
                        gl_sys::UNSIGNED_INT,
                        ptr::null(),
                        instances as i32,
                    );

                    (gl.glBindVertexArray)(0);
                    (gl.glUseProgram)(0);
                }
            }
        }
        for event in to_dispatch.iter() {
            self.call_event_handler(&event);
        }
    }

    pub fn set_default_depth_and_blend_mode(gl: &LibGl) {
        unsafe {
            (gl.glEnable)(gl_sys::DEPTH_TEST);
            (gl.glDepthFunc)(gl_sys::LEQUAL);
            (gl.glBlendEquationSeparate)(gl_sys::FUNC_ADD, gl_sys::FUNC_ADD);
            (gl.glBlendFuncSeparate)(
                gl_sys::ONE,
                gl_sys::ONE_MINUS_SRC_ALPHA,
                gl_sys::ONE,
                gl_sys::ONE_MINUS_SRC_ALPHA,
            );
            (gl.glEnable)(gl_sys::BLEND);
        }
    }

    pub fn setup_render_pass(&mut self, draw_pass_id: DrawPassId) -> Option<(Vec2d, f64)> {
        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
        let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
        let pass = &mut self.passes[draw_pass_id];
        pass.paint_dirty = false;

        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            return None;
        }

        pass.set_ortho_matrix(pass_rect.pos, pass_rect.size);
        pass.set_dpi_factor(dpi_factor);

        pass.os
            .pass_uniforms
            .update_uniform_buffer(self.os.gl(), pass.pass_uniforms.as_slice());

        Some((pass_rect.size, dpi_factor))
    }

    pub fn draw_pass_to_texture(
        &mut self,
        draw_pass_id: DrawPassId,
        override_pass_texture: Option<&Texture>,
    ) {
        let draw_list_id = self.passes[draw_pass_id].main_draw_list_id.unwrap();

        let (pass_size, dpi_factor) = if let Some(pz) = self.setup_render_pass(draw_pass_id) {
            pz
        } else {
            return;
        };

        let mut clear_color = Vec4f::default();
        let mut clear_depth = 1.0;
        let mut clear_flags = 0;
        let gl = self.os.gl();
        // make a framebuffer
        if self.passes[draw_pass_id].os.gl_framebuffer.is_none() {
            unsafe {
                let mut gl_framebuffer = std::mem::MaybeUninit::uninit();
                (gl.glGenFramebuffers)(1, gl_framebuffer.as_mut_ptr());
                self.passes[draw_pass_id].os.gl_framebuffer = Some(gl_framebuffer.assume_init());
            }
        }

        // bind the framebuffer
        unsafe {
            (gl.glBindFramebuffer)(
                gl_sys::FRAMEBUFFER,
                self.passes[draw_pass_id].os.gl_framebuffer.unwrap(),
            );
        }

        let color_textures_from_fb_texture = override_pass_texture.map(|texture| {
            [crate::draw_pass::CxDrawPassColorTexture {
                clear_color: DrawPassClearColor::ClearWith(self.passes[draw_pass_id].clear_color),
                texture: texture.clone(),
            }]
        });
        let color_textures = color_textures_from_fb_texture
            .as_ref()
            .map_or(&self.passes[draw_pass_id].color_textures[..], |xs| &xs[..]);

        for (index, color_texture) in color_textures.iter().enumerate() {
            match color_texture.clear_color {
                DrawPassClearColor::InitWith(_clear_color) => {
                    let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_render_target(gl, size.x as usize, size.y as usize);
                    if cxtexture.take_initial() {
                        clear_color = _clear_color;
                        clear_flags |= gl_sys::COLOR_BUFFER_BIT;
                    }
                }
                DrawPassClearColor::ClearWith(_clear_color) => {
                    let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_render_target(gl, size.x as usize, size.y as usize);
                    clear_color = _clear_color;
                    clear_flags |= gl_sys::COLOR_BUFFER_BIT;
                }
            }
            if let Some(gl_texture) = self.textures[color_texture.texture.texture_id()]
                .os
                .gl_texture
            {
                unsafe {
                    (gl.glFramebufferTexture2D)(
                        gl_sys::FRAMEBUFFER,
                        gl_sys::COLOR_ATTACHMENT0 + index as u32,
                        gl_sys::TEXTURE_2D,
                        gl_texture,
                        0,
                    );
                }
            }
        }

        // attach/clear depth buffers, if any
        if let Some(depth_texture) = &self.passes[draw_pass_id].depth_texture {
            match self.passes[draw_pass_id].clear_depth {
                DrawPassClearDepth::InitWith(_clear_depth) => {
                    let cxtexture = &mut self.textures[depth_texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_depth_stencil(gl, size.x as usize, size.y as usize);
                    if cxtexture.take_initial() {
                        clear_depth = _clear_depth;
                        clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                    }
                }
                DrawPassClearDepth::ClearWith(_clear_depth) => {
                    let cxtexture = &mut self.textures[depth_texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_depth_stencil(gl, size.x as usize, size.y as usize);
                    clear_depth = _clear_depth;
                    clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                }
            }
        } else {
            /* unsafe { // BUGFIX. we have to create a depthbuffer for rtt without depthbuffer use otherwise it fails if there is another pass with depth
                if self.passes[draw_pass_id].os.gl_bugfix_depthbuffer.is_none() {
                    let mut gl_renderbuf = std::mem::MaybeUninit::uninit();
                    (gl.glGenRenderbuffers)(1, gl_renderbuf.as_mut_ptr());
                    let gl_renderbuffer = gl_renderbuf.assume_init();
                    (gl.glBindRenderbuffer)(gl_sys::RENDERBUFFER, gl_renderbuffer);
                    (gl.glRenderbufferStorage)(
                        gl_sys::RENDERBUFFER,
                        gl_sys::DEPTH_COMPONENT16,
                        (pass_size.x * dpi_factor) as i32,
                        (pass_size.y * dpi_factor) as i32
                    );
                    (gl.glBindRenderbuffer)(gl_sys::RENDERBUFFER, 0);
                    self.passes[draw_pass_id].os.gl_bugfix_depthbuffer = Some(gl_renderbuffer);
                }
                clear_depth = 1.0;
                clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                (gl.glDisable)(gl_sys::DEPTH_TEST);
                (gl.glFramebufferRenderbuffer)(gl_sys::FRAMEBUFFER, gl_sys::DEPTH_ATTACHMENT, gl_sys::RENDERBUFFER, self.passes[draw_pass_id].os.gl_bugfix_depthbuffer.unwrap());
            }*/
        }

        // HACK(eddyb) drain error queue, so that we can check erors below.
        //while unsafe { (gl.glGetError)() } != 0 {}

        unsafe {
            let (x, mut y) = (0, 0);
            let width = (pass_size.x * dpi_factor) as u32;
            let height = (pass_size.y * dpi_factor) as u32;

            // HACK(eddyb) to try and match DirectX and Metal conventions, we
            // need the viewport to be placed on the other end of the Y axis.
            if let [color_texture] = color_textures {
                let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                if cxtexture.os.gl_texture.is_some() {
                    let alloc_height = cxtexture.alloc.as_ref().unwrap().height as u32;
                    if alloc_height > height {
                        y = alloc_height - height;
                    }
                }
            }

            (gl.glViewport)(x as i32, y as i32, width as i32, height as i32);

            //assert_eq!((gl.glGetError)(), 0, "glViewport({x}, {y}, {width}, {height}) failed");
        }

        if clear_flags != 0 {
            unsafe {
                if clear_flags & gl_sys::DEPTH_BUFFER_BIT != 0 {
                    (gl.glClearDepthf)(clear_depth);
                }
                (gl.glClearColor)(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                (gl.glClear)(clear_flags);
            }
        }
        Self::set_default_depth_and_blend_mode(self.os.gl());

        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;

        self.render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step);

        unsafe {
            (self.os.gl().glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
            //(gl.glFinish)();
        }
    }

    pub fn opengl_compile_shaders(&mut self) {
        let compile_set = std::mem::take(&mut self.draw_shaders.compile_set);

        for shader_index in compile_set {
            let cx_shader = &mut self.draw_shaders.shaders[shader_index];
            if cx_shader.os_shader_id.is_some() {
                continue;
            }

            let (vertex, pixel) = match &cx_shader.mapping.code {
                CxDrawShaderCode::Separate { vertex, fragment } => {
                    (vertex.clone(), fragment.clone())
                }
                CxDrawShaderCode::Combined { code } => (code.clone(), code.clone()),
            };

            if cx_shader.mapping.flags.debug {
                crate::log!("{}\n{}", vertex, pixel);
            }

            for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                if ds.in_vertex == vertex && ds.in_pixel == pixel {
                    cx_shader.os_shader_id = Some(index);
                    break;
                }
            }

            if cx_shader.os_shader_id.is_none() {
                let shp = CxOsDrawShader::new(self.os.gl(), &vertex, &pixel, &self.os_type);
                cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                self.draw_shaders.os_shaders.push(shp);
            }
        }
    }
    /*
    pub fn maybe_warn_hardware_support(&self) {
        // Temporary warning for Adreno failing at compiling shaders that use samplerExternalOES.

    }*/
}

const NUM_SHADER_VARIANTS: usize = 2;

pub struct CxOsDrawShader {
    pub gl_shader: [Option<GlShader>; NUM_SHADER_VARIANTS],
    pub in_vertex: String,
    pub in_pixel: String,
    pub vertex: [String; NUM_SHADER_VARIANTS],
    pub pixel: [String; NUM_SHADER_VARIANTS],
    //pub const_table_uniforms: OpenglBuffer,
    pub live_uniforms: OpenglBuffer,
    #[cfg(use_vulkan)]
    pub vulkan_shader: Option<CxVulkanShaderBinary>,
}

pub struct GlShaderUniforms {
    pub pass_uniforms_binding: OpenglUniformBlockBinding,
    pub draw_list_uniforms_binding: OpenglUniformBlockBinding,
    pub draw_call_uniforms_binding: OpenglUniformBlockBinding,
    pub user_uniforms_binding: OpenglUniformBlockBinding,
    pub live_uniforms_binding: OpenglUniformBlockBinding,
    pub const_table_uniform: OpenglUniform,
    pub live_uniforms: OpenglBuffer,
}
impl GlShaderUniforms {
    fn new(gl: &LibGl, program: u32, mapping: &CxDrawShaderMapping) -> Self {
        let mut live_uniforms = OpenglBuffer::default();
        live_uniforms.update_uniform_buffer(gl, mapping.scope_uniforms_buf.as_ref());

        Self {
            pass_uniforms_binding: GlShader::opengl_get_uniform_block_binding(
                gl,
                program,
                "passUniforms",
            ),
            draw_list_uniforms_binding: GlShader::opengl_get_uniform_block_binding(
                gl,
                program,
                "draw_listUniforms",
            ),
            draw_call_uniforms_binding: GlShader::opengl_get_uniform_block_binding(
                gl,
                program,
                "draw_callUniforms",
            ),
            user_uniforms_binding: GlShader::opengl_get_uniform_block_binding(
                gl,
                program,
                "userUniforms",
            ),
            live_uniforms_binding: GlShader::opengl_get_uniform_block_binding(
                gl,
                program,
                "liveUniforms",
            ),
            const_table_uniform: GlShader::opengl_get_uniform(gl, program, "const_table"),
            live_uniforms,
        }
    }
}

pub struct GlShader {
    pub program: u32,
    pub geometries: Vec<OpenglAttribute>,
    pub instances: Vec<OpenglAttribute>,
    pub textures: Vec<OpenglUniform>,
    pub samplers: Vec<OpenglSampler>,
    pub xr_depth_texture: OpenglUniform,
    // all these things need to be uniform buffers
    pub uniforms: GlShaderUniforms,
}

impl GlShader {
    pub fn new(
        gl: &LibGl,
        vertex: &str,
        pixel: &str,
        mapping: &CxDrawShaderMapping,
        os_type: &OsType,
    ) -> Self {
        // On OpenHarmony, re-using cached shaders doesn't work properly yet.
        #[cfg(ohos_sim)]
        unsafe fn read_cache(
            _gl: &LibOpenGl,
            _vertex: &str,
            _pixel: &str,
            _os_type: &OsType,
        ) -> Option<gl_sys::GLuint> {
            None
        }

        #[cfg(not(ohos_sim))]
        unsafe fn read_cache(
            gl: &LibGl,
            vertex: &str,
            pixel: &str,
            os_type: &OsType,
        ) -> Option<gl_sys::GLuint> {
            if let Some(cache_dir) = os_type.get_cache_dir() {
                let shader_hash = live_id!(shader).str_append(&vertex).str_append(&pixel);
                let mut base_filename = format!("{}/shader_{:08x}", cache_dir, shader_hash.0);

                match os_type {
                    OsType::Android(params) => {
                        base_filename = format!(
                            "{}_av{}_bn{}_kv{}",
                            base_filename,
                            params.android_version,
                            params.build_number,
                            params.kernel_version
                        );
                    }
                    _ => (),
                };

                let filename = format!("{}.bin", base_filename);

                if let Ok(mut cache_file) = File::open(&filename) {
                    let mut binary = Vec::new();
                    let mut format_bytes = [0u8; 4];
                    match cache_file.read(&mut format_bytes) {
                        Ok(_bytes_read) => {
                            let binary_format = u32::from_be_bytes(format_bytes);
                            match cache_file.read_to_end(&mut binary) {
                                Ok(_full_bytes) => {
                                    let mut version_consistency_conflict = false;
                                    // On Android, invalidate the cached file if there have been significant system updates
                                    match os_type {
                                        OsType::Android(params) => {
                                            let current_filename = format!(
                                                "{}/shader_{:08x}_av{}_bn{}_kv{}.bin",
                                                cache_dir,
                                                shader_hash.0,
                                                params.android_version,
                                                params.build_number,
                                                params.kernel_version
                                            );
                                            version_consistency_conflict =
                                                filename != current_filename;
                                        }
                                        _ => (),
                                    };

                                    if !version_consistency_conflict {
                                        let program = (gl.glCreateProgram)();
                                        (gl.glProgramBinary)(
                                            program,
                                            binary_format,
                                            binary.as_ptr() as *const _,
                                            binary.len() as i32,
                                        );
                                        if let Some(error) = GlShader::opengl_has_shader_error(
                                            gl,
                                            false,
                                            program as usize,
                                            "",
                                        ) {
                                            crate::error!(
                                                "ERROR::SHADER::CACHE::PROGRAM_BINARY_FAILED\n{}",
                                                error
                                            );
                                            return None;
                                        }
                                        return Some(program);
                                    } else {
                                        // Version mismatch, delete the old cache file
                                        let _ = remove_file(&filename);
                                    }
                                }
                                Err(e) => {
                                    crate::warning!("Failed to read the full shader cache file {filename}, error: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            crate::warning!("Failed to read format bytes from shader cache file {filename}, error: {e}");
                        }
                    }
                } else {
                    // crate::debug!("File was not in shader cache: {filename}");
                }
            } else {
                //crate::warning!("No cache directory available for shader cache");
            }
            None
        }

        unsafe {
            let program = if let Some(program) = read_cache(gl, &vertex, &pixel, os_type) {
                program
            } else {
                let vs = (gl.glCreateShader)(gl_sys::VERTEX_SHADER);
                (gl.glShaderSource)(vs, 1, [vertex.as_ptr() as *const _].as_ptr(), ptr::null());
                (gl.glCompileShader)(vs);
                Self::opengl_log_shader_info(gl, true, vs as usize, "vertex", vertex);
                if let Some(error) = Self::opengl_has_shader_error(gl, true, vs as usize, &vertex) {
                    panic!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}", error);
                }
                let fs = (gl.glCreateShader)(gl_sys::FRAGMENT_SHADER);
                (gl.glShaderSource)(fs, 1, [pixel.as_ptr() as *const _].as_ptr(), ptr::null());
                (gl.glCompileShader)(fs);
                Self::opengl_log_shader_info(gl, true, fs as usize, "fragment", pixel);
                if let Some(error) = Self::opengl_has_shader_error(gl, true, fs as usize, &pixel) {
                    panic!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}", error);
                }

                let program = (gl.glCreateProgram)();
                (gl.glAttachShader)(program, vs);
                (gl.glAttachShader)(program, fs);
                (gl.glLinkProgram)(program);
                Self::opengl_log_shader_info(gl, false, program as usize, "program", "");
                if let Some(error) = Self::opengl_has_shader_error(gl, false, program as usize, "")
                {
                    panic!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}", error);
                }
                (gl.glDeleteShader)(vs);
                (gl.glDeleteShader)(fs);

                #[cfg(not(ohos_sim))] // caching doesn't work properly on OpenHarmony
                if let Some(cache_dir) = os_type.get_cache_dir() {
                    let mut binary = Vec::new();
                    let mut binary_len = 0;
                    (gl.glGetProgramiv)(program, gl_sys::PROGRAM_BINARY_LENGTH, &mut binary_len);
                    if binary_len != 0 {
                        binary.resize(binary_len as usize, 0u8);
                        let mut return_size = 0i32;
                        let mut binary_format = 0u32;
                        (gl.glGetProgramBinary)(
                            program,
                            binary.len() as i32,
                            &mut return_size as *mut _,
                            &mut binary_format as *mut _,
                            binary.as_mut_ptr() as *mut _,
                        );
                        if return_size != 0 {
                            // crate::log!("GOT FORMAT {}", format);
                            let shader_hash =
                                live_id!(shader).str_append(&vertex).str_append(&pixel);
                            let mut filename =
                                format!("{}/shader_{:08x}", cache_dir, shader_hash.0);

                            match os_type {
                                OsType::Android(params) => {
                                    filename = format!(
                                        "{}_av{}_bn{}_kv{}",
                                        filename,
                                        params.android_version,
                                        params.build_number,
                                        params.kernel_version
                                    );
                                }
                                _ => (),
                            };

                            filename = format!("{}.bin", filename);

                            binary.resize(return_size as usize, 0u8);
                            match File::create(&filename) {
                                Ok(mut cache) => {
                                    let _res1 = cache.write_all(&binary_format.to_be_bytes());
                                    let _res2 = cache.write_all(&binary);
                                    if _res1.is_err() || _res2.is_err() {
                                        crate::error!("Failed to write shader binary to shader cache {filename}");
                                    }
                                }
                                Err(e) => {
                                    crate::error!(
                                        "Failed to write shader cache to {filename}, error: {e}"
                                    );
                                }
                            }
                        }
                    }
                }
                program
            };

            (gl.glUseProgram)(program);

            let uniforms = GlShaderUniforms::new(gl, program, mapping);

            uniforms
                .live_uniforms_binding
                .bind_buffer(gl, &uniforms.live_uniforms);

            if !mapping.dyn_uniforms.inputs.is_empty() {
                let zeros = vec![0.0f32; mapping.dyn_uniforms.total_slots];
                GlShader::set_uniform_array(gl, &uniforms.const_table_uniform, &zeros);
            }

            (gl.glUseProgram)(0);

            let t = Self {
                program,
                geometries: Self::opengl_get_attributes(
                    gl,
                    program,
                    "packed_geometry_",
                    mapping.geometries.total_slots,
                    &mapping.geometries.inputs,
                ),
                instances: Self::opengl_get_attributes(
                    gl,
                    program,
                    "packed_instance_",
                    mapping.instances.total_slots,
                    &mapping.instances.inputs,
                ),
                textures: Self::opengl_get_texture_slots(gl, program, &mapping.textures),
                samplers: Self::opengl_create_samplers(gl, mapping),
                xr_depth_texture: Self::opengl_get_uniform(gl, program, "xr_depth_texture"),
                uniforms,
            };
            t
        }
    }

    pub fn set_uniform_array(gl: &LibGl, loc: &OpenglUniform, array: &[f32]) {
        if let Some(loc) = loc.loc {
            unsafe {
                (gl.glUniform1fv)(loc, array.len() as i32, array.as_ptr());
            }
        }
    }

    pub fn opengl_get_uniform(gl: &LibGl, program: u32, name: &str) -> OpenglUniform {
        unsafe {
            let loc =
                (gl.glGetUniformLocation)(program, std::ffi::CString::new(name).unwrap().as_ptr());
            OpenglUniform {
                loc: if loc < 0 { None } else { Some(loc) },
            }
        }
    }

    pub fn opengl_get_uniform_block_binding(
        gl: &LibGl,
        program: u32,
        name: &str,
    ) -> OpenglUniformBlockBinding {
        unsafe {
            let index = (gl.glGetUniformBlockIndex)(
                program,
                std::ffi::CString::new(name).unwrap().as_ptr(),
            ) as i32;
            if index < 0 {
                return OpenglUniformBlockBinding { index: None };
            }
            // make the binding the same as the index for ease of use
            (gl.glUniformBlockBinding)(program, index as u32, index as u32);

            OpenglUniformBlockBinding {
                index: Some(index as u32),
            }
        }
    }

    pub fn opengl_get_info_log(gl: &LibGl, compile: bool, shader: usize, source: &str) -> String {
        unsafe {
            let mut length = 0;
            if compile {
                (gl.glGetShaderiv)(shader as u32, gl_sys::INFO_LOG_LENGTH, &mut length);
            } else {
                (gl.glGetProgramiv)(shader as u32, gl_sys::INFO_LOG_LENGTH, &mut length);
            }

            let mut log = vec![0u8; length.max(1) as usize];
            if compile {
                (gl.glGetShaderInfoLog)(
                    shader as u32,
                    length,
                    ptr::null_mut(),
                    log.as_mut_ptr() as *mut _,
                );
            } else {
                (gl.glGetProgramInfoLog)(
                    shader as u32,
                    length,
                    ptr::null_mut(),
                    log.as_mut_ptr() as *mut _,
                );
            }
            let c_end = log.iter().position(|b| *b == 0).unwrap_or(log.len());
            let info_log = String::from_utf8_lossy(&log[..c_end]);

            let mut r = String::new();
            r.push_str(info_log.trim_end());
            r.push_str("\n");
            if !source.is_empty() {
                for (line, chunk) in source.split('\n').enumerate() {
                    r.push_str(&(line + 1).to_string());
                    r.push_str(":");
                    r.push_str(chunk);
                    r.push_str("\n");
                }
            }
            r
        }
    }

    fn opengl_log_shader_info(
        gl: &LibGl,
        compile: bool,
        shader: usize,
        stage_name: &str,
        source: &str,
    ) {
        let info = Self::opengl_get_info_log(gl, compile, shader, "");
        let has_errors = info
            .lines()
            .any(|line| line.to_ascii_lowercase().contains("error"));
        let dump_sources = std::env::var_os("MAKEPAD_LOG_GLSL_SOURCES").is_some();

        if has_errors || dump_sources {
            let kind = if compile { "compile" } else { "link" };
            crate::warning!(
                "GLSL {} {} info:\n{}",
                kind,
                stage_name,
                if has_errors {
                    info
                } else {
                    "(no compiler errors)\n".to_string()
                }
            );
            if dump_sources && !source.is_empty() {
                crate::warning!("GLSL {} {} source:\n{}", kind, stage_name, source);
            }
        }
    }

    pub fn opengl_has_shader_error(
        gl: &LibGl,
        compile: bool,
        shader: usize,
        source: &str,
    ) -> Option<String> {
        //None
        unsafe {
            let mut success = gl_sys::TRUE as i32;

            if compile {
                (gl.glGetShaderiv)(shader as u32, gl_sys::COMPILE_STATUS, &mut success);
            } else {
                (gl.glGetProgramiv)(shader as u32, gl_sys::LINK_STATUS, &mut success);
            };

            if success != gl_sys::TRUE as i32 {
                Some(Self::opengl_get_info_log(gl, compile, shader, source))
            } else {
                None
            }
        }
    }

    pub fn opengl_get_attributes(
        gl: &LibGl,
        program: u32,
        prefix: &str,
        slots: usize,
        inputs: &[crate::draw_shader::DrawShaderInput],
    ) -> Vec<OpenglAttribute> {
        let mut attribs = Vec::new();

        fn ceil_div4(base: usize) -> usize {
            let r = base >> 2;
            if base & 3 != 0 {
                return r + 1;
            }
            r
        }

        let stride = (slots * mem::size_of::<f32>()) as i32;
        let num_attr = ceil_div4(slots);
        let mut chunk_formats = vec![DrawShaderAttrFormat::Float; num_attr];
        for input in inputs {
            if input.attr_format == DrawShaderAttrFormat::Float {
                continue;
            }
            for slot in input.offset..(input.offset + input.slots) {
                chunk_formats[slot / 4] = input.attr_format;
            }
        }
        let trace_draw = std::env::var_os("MAKEPAD_GL_DRAW_TRACE").is_some();
        for i in 0..num_attr {
            let mut name0 = prefix.to_string();
            name0.push_str(&i.to_string());
            name0.push_str("\0");

            let mut size = (slots - i * 4) as i32;
            if size > 4 {
                size = 4;
            }
            unsafe {
                let loc = (gl.glGetAttribLocation)(program, name0.as_ptr() as *const _);
                if trace_draw {
                    crate::log!(
                        "GL attrib program={} name={} loc={} size={} stride={} offset={} format={:?}",
                        program,
                        name0.trim_end_matches('\0'),
                        loc,
                        size,
                        stride,
                        (i * 4 * mem::size_of::<f32>()),
                        chunk_formats[i]
                    );
                }
                attribs.push(OpenglAttribute {
                    name: name0.to_string(),
                    loc: if loc < 0 { None } else { Some(loc as u32) },
                    offset: (i * 4 * mem::size_of::<f32>()) as usize,
                    size: size,
                    stride: stride,
                    attr_format: chunk_formats[i],
                })
            }
        }
        attribs
    }

    pub fn opengl_get_texture_slots(
        gl: &LibGl,
        program: u32,
        texture_slots: &Vec<DrawShaderTextureInput>,
    ) -> Vec<OpenglUniform> {
        let mut gl_texture_slots = Vec::new();

        for slot in texture_slots {
            let mut name0 = "tex_".to_string();
            name0.push_str(&slot.id.to_string());
            name0.push_str("\0");
            unsafe {
                let mut loc = (gl.glGetUniformLocation)(program, name0.as_ptr().cast());
                if loc < 0 {
                    let mut fallback_name = "ds_".to_string();
                    fallback_name.push_str(&slot.id.to_string());
                    fallback_name.push_str("\0");
                    loc = (gl.glGetUniformLocation)(program, fallback_name.as_ptr().cast());
                }
                // crate::warning!("opengl_get_texture_slots(): texture slot: ({:?}, {:?}), name0: {:X?}, loc: {loc:#X}", slot.id, slot.ty, name0.as_bytes());
                gl_texture_slots.push(OpenglUniform {
                    loc: if loc < 0 { None } else { Some(loc) },
                });
            }
        }
        gl_texture_slots
    }

    pub fn opengl_create_samplers(gl: &LibGl, mapping: &CxDrawShaderMapping) -> Vec<OpenglSampler> {
        let mut samplers = Vec::with_capacity(mapping.textures.len());
        let Some(gl_gen_samplers) = gl.glGenSamplers else {
            samplers.resize(mapping.textures.len(), OpenglSampler::default());
            return samplers;
        };
        let Some(gl_sampler_parameteri) = gl.glSamplerParameteri else {
            samplers.resize(mapping.textures.len(), OpenglSampler::default());
            return samplers;
        };

        for texture_slot in 0..mapping.textures.len() {
            let sampler_desc = mapping
                .texture_sampler_indices
                .get(texture_slot)
                .and_then(|sampler_idx| mapping.samplers.get(*sampler_idx))
                .copied()
                .unwrap_or_default();

            let mut sampler = 0u32;
            unsafe {
                gl_gen_samplers(1, &mut sampler);
            }
            if sampler == 0 {
                samplers.push(OpenglSampler::default());
                continue;
            }

            let filter = match sampler_desc.filter {
                SamplerFilter::Nearest => gl_sys::NEAREST,
                SamplerFilter::Linear => gl_sys::LINEAR,
            };
            let address = match sampler_desc.address {
                SamplerAddress::Repeat => gl_sys::REPEAT,
                SamplerAddress::ClampToEdge => gl_sys::CLAMP_TO_EDGE,
                // CLAMP_TO_BORDER is not universally available on GLES3, keep it edge-safe.
                SamplerAddress::ClampToZero => gl_sys::CLAMP_TO_EDGE,
                SamplerAddress::MirroredRepeat => gl_sys::MIRRORED_REPEAT,
            };

            unsafe {
                gl_sampler_parameteri(sampler, gl_sys::TEXTURE_MIN_FILTER, filter as i32);
                gl_sampler_parameteri(sampler, gl_sys::TEXTURE_MAG_FILTER, filter as i32);
                gl_sampler_parameteri(sampler, gl_sys::TEXTURE_WRAP_S, address as i32);
                gl_sampler_parameteri(sampler, gl_sys::TEXTURE_WRAP_T, address as i32);
                gl_sampler_parameteri(sampler, gl_sys::TEXTURE_WRAP_R, address as i32);
            }

            samplers.push(OpenglSampler {
                sampler: Some(sampler),
            });
        }

        samplers
    }

    pub fn free_resources(self, gl: &LibGl) {
        if let Some(gl_delete_samplers) = gl.glDeleteSamplers {
            for sampler in &self.samplers {
                if let Some(sampler) = sampler.sampler {
                    unsafe { gl_delete_samplers(1, &sampler) };
                }
            }
        }
        unsafe {
            (gl.glDeleteShader)(self.program);
        }
    }
}

impl CxOsDrawShader {
    pub fn new(gl: &LibGl, in_vertex: &str, in_pixel: &str, os_type: &OsType) -> Self {
        // Check if GL_OES_EGL_image_external extension is available in the current device, otherwise do not attempt to use in the shaders.
        let available_extensions = get_gl_string(gl, gl_sys::EXTENSIONS);
        let is_external_texture_supported = available_extensions
            .split_whitespace()
            .any(|ext| ext == "GL_OES_EGL_image_external");

        // GL_OES_EGL_image_external is not well supported on Android emulators with macOS hosts.
        // Because there's no bullet-proof way to check the emualtor host at runtime, we're currently disabling external texture support on all emulators.
        let is_emulator = match os_type {
            OsType::Android(params) => params.is_emulator,
            OsType::OpenHarmony(_) => true, // TODO FIXME: detect whether we're running on an OHOS emulator
            _ => false,
        };

        // Some Android devices running Adreno GPUs suddenly stopped compiling shaders when passing the samplerExternalOES sampler to texture2D functions.
        // This seems like a driver bug (no confirmation from Qualcomm yet).
        // Therefore we're disabling the external texture support for Adreno until this is fixed.
        let is_vendor_adreno = get_gl_string(gl, gl_sys::RENDERER).contains("Adreno");

        let (tex_ext_import, tex_ext_sampler) = if is_external_texture_supported
            && !is_vendor_adreno
            && !is_emulator
        {
            (
            "#extension GL_OES_EGL_image_external : require\n",
            "vec4 sample2dOES(samplerExternalOES sampler, vec2 pos){ return texture2D(sampler, vec2(pos.x, pos.y));}"
        )
        } else {
            ("", "")
        };

        // Currently, these shaders are only compatible with `#version 100` through `#version 300 es`.
        // Version 310 and later have removed/deprecated some features that we currently use:
        // * error C7616: global function texture2D is removed after version 310
        // * error C1121: transpose: function (builtin) redefinition/overload not allowed
        // * error C5514: 'attribute' is deprecated and removed from this profile, use 'in/out' instead
        // * error C7614: GLSL ES doesn't allow use of reserved word attribute
        // * error C7614: GLSL ES doesn't allow use of reserved word varying

        // check if we are running in XR or not

        //#extension GL_OVR_multiview2 : require
        // layout(num_views=2) in;
        let depth_clip = "
            uniform sampler2DArray xr_depth_texture;
            vec4 depth_clip(vec4 world, vec4 color, float clip){
                vec4 cube_depth_camera_position = pass.depth_projection[VIEW_ID] * pass.depth_view[VIEW_ID] * world;

                vec3 cube_depth_camera_position_hc = cube_depth_camera_position.xyz / cube_depth_camera_position.w;
                cube_depth_camera_position_hc = cube_depth_camera_position_hc*0.5f + 0.5f;

                vec3 depth_view_coord = vec3(cube_depth_camera_position_hc.xy, VIEW_ID);

                gl_FragDepth = cube_depth_camera_position_hc.z;

                float depth_view_eye_z = texture(xr_depth_texture, depth_view_coord).r;
                if(clip  < 0.5 || depth_view_eye_z >= cube_depth_camera_position_hc.z){
                    return color;
                }
                return vec4(0.0,0.0,0.0,0.0);
            }
        ";
        let nop_depth_clip = "
            vec4 depth_clip(vec4 w, vec4 c, float clip){return c;}
        ";
        #[cfg(target_os = "android")]
        let sampler_helpers = "
            vec4 depth_clip(vec4 w, vec4 c, float clip);
            vec4 sample2d(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, pos.y));}
            vec4 sample2d_bgra(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, pos.y));}
            vec4 sample2d_rt(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, 1.0 - pos.y));}
            vec4 samplecube(samplerCube sampler, vec3 dir){return texture(sampler, dir);}
            vec4 samplecube_bgra(samplerCube sampler, vec3 dir){return texture(sampler, dir).zyxw;}
            ";
        #[cfg(not(target_os = "android"))]
        let sampler_helpers = "
            vec4 depth_clip(vec4 w, vec4 c, float clip);
            vec4 sample2d(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, pos.y));}
            vec4 sample2d_bgra(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, pos.y));}
            vec4 sample2d_rt(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, 1.0 - pos.y));}
            vec4 samplecube(samplerCube sampler, vec3 dir){return texture(sampler, dir);}
            vec4 samplecube_bgra(samplerCube sampler, vec3 dir){return texture(sampler, dir);}
            ";

        let (version, vertex_exts, pixel_exts, vertex_defs, pixel_defs, sampler) = if os_type
            .has_xr_mode()
        {
            (
            "#version 300 es",
            // Vertex shader
            "
            #define VIEW_ID 0
            #extension GL_OVR_multiview2 : require
            layout(num_views=2) in;
            ",
            // Pixel shader
            "
            #define VIEW_ID 0
            #extension GL_OVR_multiview2 : require
            ",
            "",
            "",
            sampler_helpers
        )
        } else {
            (
            "#version 300 es",
            "",
            "",
            "",
            "",
            sampler_helpers
        )
        };

        /*
        let transpose_impl = "
        mat4 transpose(mat4 m){{return mat4(m[0][0],m[1][0],m[2][0],m[3][0],m[0][1],m[1][1],m[2][1],m[3][1],m[0][2],m[1][2],m[2][2],m[3][3], m[3][0], m[3][1], m[3][2], m[3][3]);}}
        mat3 transpose(mat3 m){{return mat3(m[0][0],m[1][0],m[2][0],m[0][1],m[1][1],m[2][1],m[0][2],m[1][2],m[2][2]);}}
        mat2 transpose(mat2 m){{return mat2(m[0][0],m[1][0],m[0][1],m[1][1]);}}
        ";
        */
        let vertex = format!(
            "{version}
            {vertex_exts}
            {tex_ext_import}
            precision highp float;
            precision highp int;
            {sampler}
            {tex_ext_sampler}
            {vertex_defs}
            {in_vertex}\0",
        );
        //crate::log!("{}", vertex.replace("int mvo = 0;","int mvo = gl_ViewID_OVR==0?0:16;"));
        let pixel = format!(
            "{version}
            {pixel_exts}
            {tex_ext_import}
            #extension GL_OES_standard_derivatives : enable
            precision highp float;
            precision highp int;
            {sampler}
            {tex_ext_sampler}
            {pixel_defs}
            {in_pixel}
            {nop_depth_clip}
            \0",
        );
        // lets fetch the uniform positions for our uniforms
        CxOsDrawShader {
            in_vertex: in_vertex.to_string(),
            in_pixel: in_pixel.to_string(),
            vertex: [
                vertex.clone(),
                vertex.replace("#define VIEW_ID 0", "#define VIEW_ID gl_ViewID_OVR"),
            ],
            pixel: [
                pixel.clone(),
                pixel
                    .replace("#define VIEW_ID 0", "#define VIEW_ID gl_ViewID_OVR")
                    .replace(nop_depth_clip, depth_clip),
            ],
            gl_shader: [None, None],
            //const_table_uniforms: Default::default(),
            live_uniforms: Default::default(),
            #[cfg(use_vulkan)]
            vulkan_shader: None,
        }
    }

    pub fn free_resources(&mut self, gl: &LibGl) {
        for gl_shader in &mut self.gl_shader {
            if let Some(gl_shader) = gl_shader.take() {
                gl_shader.free_resources(gl);
            }
        }
    }
}

fn get_gl_string(gl: &LibGl, key: gl_sys::GLenum) -> String {
    unsafe {
        let string_ptr = (gl.glGetString)(key) as *const c_char;
        if string_ptr == ptr::null() {
            return String::new();
        }
        CStr::from_ptr(string_ptr).to_string_lossy().into_owned()
    }
}

#[derive(Default, Clone, Debug)]
pub struct OpenglAttribute {
    pub name: String,
    pub loc: Option<u32>,
    pub size: i32,
    pub offset: usize,
    pub stride: i32,
    pub attr_format: DrawShaderAttrFormat,
}

#[derive(Debug, Default, Clone)]
pub struct OpenglUniform {
    pub loc: Option<i32>,
    //pub name: String,
}

#[derive(Debug, Default, Clone)]
pub struct OpenglSampler {
    pub sampler: Option<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct OpenglUniformBlockBinding {
    pub index: Option<u32>,
}

impl OpenglUniformBlockBinding {
    #[allow(unused)]
    fn bind_buffer(&self, gl: &LibGl, buf: &OpenglBuffer) {
        if let Some(gl_buf) = buf.gl_buffer {
            if let Some(index) = self.index {
                unsafe { (gl.glBindBufferBase)(gl_sys::UNIFORM_BUFFER, index, gl_buf) };
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct CxOsGeometry {
    pub vb: OpenglBuffer,
    pub ib: OpenglBuffer,
}

impl CxOsGeometry {
    pub fn free_resources(&mut self, gl: &LibGl) {
        self.vb.free_resources(gl);
        self.ib.free_resources(gl);
    }
}

/*
#[derive(Default, Clone)]
pub struct OpenglTextureSlot {
    pub loc: isize,
    pub name: String
}
*/
#[derive(Clone, Default)]
pub struct CxOsDrawList {
    #[allow(unused)]
    draw_list_uniforms: OpenglBuffer,
}

#[derive(Default, Clone)]
pub struct CxOsDrawCallVao {
    pub vao: Option<u32>,
    pub shader_id: Option<usize>,
    pub inst_vb: Option<u32>,
    pub geom_vb: Option<u32>,
    pub geom_ib: Option<u32>,
}

impl CxOsDrawCallVao {
    pub fn free(self, gl: &LibGl) {
        if let Some(vao) = self.vao {
            unsafe { (gl.glDeleteVertexArrays)(1, &vao) };
        }
    }
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub draw_call_uniforms: OpenglBuffer,
    pub user_uniforms: OpenglBuffer,
    pub inst_vb: OpenglBuffer,
    pub vao: Option<CxOsDrawCallVao>,
}

impl CxOsDrawCall {
    pub fn free_resources(&mut self, gl: &LibGl) {
        self.inst_vb.free_resources(gl);
        if let Some(vao) = self.vao.take() {
            vao.free(gl);
        }
    }
}

#[derive(Clone, Default)]
pub struct CxOsTexture {
    pub gl_texture: Option<u32>,
    pub gl_renderbuffer: Option<u32>,
}

impl CxTexture {
    /// Updates or creates a texture based on the current texture format.
    ///
    /// This method optimizes texture management by:
    /// 1. Reusing existing OpenGL textures when possible.
    /// 2. Using `glTexSubImage2D` for updates when dimensions haven't changed.
    /// 3. Falling back to `glTexImage2D` for new textures or when dimensions change.
    ///
    /// Internal workings:
    /// - If a previous platform resource exists, it's reused to avoid unnecessary allocations.
    /// - If no texture exists, a new OpenGL texture is generated.
    /// - The method checks current texture dimensions to decide between `glTexSubImage2D` (update)
    ///   and `glTexImage2D` (new allocation).
    ///
    /// Note: This method assumes that the texture format doesn't change between updates.
    /// This is safe because when allocating textures at the Cx level, there are compatibility checks.
    pub fn update_vec_texture(&mut self, gl: &LibGl, _os_type: &OsType) {
        let mut needs_realloc = false;
        if self.alloc_vec() {
            if let Some(previous) = self.previous_platform_resource.take() {
                self.os = previous;
            }
            if self.os.gl_texture.is_none() {
                unsafe {
                    let mut gl_texture = std::mem::MaybeUninit::uninit();
                    (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                    self.os.gl_texture = Some(gl_texture.assume_init());
                }
            }
            needs_realloc = true;
        }

        let updated = self.take_updated();
        if updated.is_empty() {
            return;
        }

        if let TextureFormat::VecCubeBGRAu8_32 {
            width,
            height,
            data,
            ..
        } = &self.format
        {
            unsafe {
                (gl.glBindTexture)(gl_sys::TEXTURE_CUBE_MAP, self.os.gl_texture.unwrap());
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_CUBE_MAP,
                    gl_sys::TEXTURE_WRAP_S,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_CUBE_MAP,
                    gl_sys::TEXTURE_WRAP_T,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_CUBE_MAP,
                    gl_sys::TEXTURE_WRAP_R,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_CUBE_MAP,
                    gl_sys::TEXTURE_MIN_FILTER,
                    gl_sys::LINEAR as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_CUBE_MAP,
                    gl_sys::TEXTURE_MAG_FILTER,
                    gl_sys::LINEAR as i32,
                );

                let targets = [
                    gl_sys::TEXTURE_CUBE_MAP_POSITIVE_X,
                    gl_sys::TEXTURE_CUBE_MAP_NEGATIVE_X,
                    gl_sys::TEXTURE_CUBE_MAP_POSITIVE_Y,
                    gl_sys::TEXTURE_CUBE_MAP_NEGATIVE_Y,
                    gl_sys::TEXTURE_CUBE_MAP_POSITIVE_Z,
                    gl_sys::TEXTURE_CUBE_MAP_NEGATIVE_Z,
                ];
                let pixels_per_face = width.saturating_mul(*height);
                for (face, target) in targets.iter().enumerate() {
                    let face_ptr = if let Some(data) = data.as_ref() {
                        if data.len() >= pixels_per_face.saturating_mul(6) {
                            data.as_ptr().add(face.saturating_mul(pixels_per_face))
                                as *const std::ffi::c_void
                        } else {
                            std::ptr::null()
                        }
                    } else {
                        std::ptr::null()
                    };
                    (gl.glTexImage2D)(
                        *target,
                        0,
                        gl_sys::BGRA as i32,
                        *width as i32,
                        *height as i32,
                        0,
                        gl_sys::BGRA,
                        gl_sys::UNSIGNED_BYTE,
                        face_ptr,
                    );
                }
                (gl.glBindTexture)(gl_sys::TEXTURE_CUBE_MAP, 0);
            }
            return;
        }

        unsafe {
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, self.os.gl_texture.unwrap());
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_WRAP_S,
                gl_sys::CLAMP_TO_EDGE as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_WRAP_T,
                gl_sys::CLAMP_TO_EDGE as i32,
            );

            // Set texture parameters based on the format
            let (
                width,
                height,
                internal_format,
                format,
                data_type,
                data,
                bytes_per_pixel,
                use_mipmaps,
            ) = match &mut self.format {
                TextureFormat::VecBGRAu8_32 {
                    width,
                    height,
                    data,
                    ..
                } => {
                    let (internal_format, format) = {
                        #[cfg(ohos_sim)]
                        {
                            // The OHOS emulators only support RGBA texture formats, so we swap the `R` and `B` channels.
                            // TODO: test this on *real* OHOS hardware, it may behave differently.
                            for p in data.as_mut().unwrap() {
                                let orig = *p;
                                *p = *p & 0xFF00FF00
                                    | (orig & 0x000000FF) << 16
                                    | (orig & 0x00FF0000) >> 16;
                            }
                            (gl_sys::RGBA, gl_sys::RGBA)
                        }
                        #[cfg(not(ohos_sim))]
                        {
                            // The default for all other devices: use the BGRA texture format
                            (gl_sys::BGRA, gl_sys::BGRA)
                        }
                    };

                    (
                        *width,
                        *height,
                        internal_format,
                        format,
                        gl_sys::UNSIGNED_BYTE,
                        data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                        4,
                        false,
                    )
                }
                TextureFormat::VecMipBGRAu8_32 {
                    width,
                    height,
                    data,
                    max_level: _,
                    ..
                } => (
                    *width,
                    *height,
                    gl_sys::BGRA,
                    gl_sys::BGRA,
                    gl_sys::UNSIGNED_BYTE,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                    4,
                    true,
                ),
                TextureFormat::VecRGBAf32 {
                    width,
                    height,
                    data,
                    ..
                } => (
                    *width,
                    *height,
                    gl_sys::RGBA,
                    gl_sys::RGBA,
                    gl_sys::FLOAT,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                    16,
                    false,
                ),
                TextureFormat::VecRu8 {
                    width,
                    height,
                    data,
                    unpack_row_length,
                    ..
                } => {
                    //(gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 1);
                    if let Some(row_length) = unpack_row_length {
                        (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, *row_length as i32);
                    }
                    (
                        *width,
                        *height,
                        gl_sys::R8,
                        gl_sys::RED,
                        gl_sys::UNSIGNED_BYTE,
                        data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                        1,
                        false,
                    )
                }
                TextureFormat::VecRGu8 {
                    width,
                    height,
                    data,
                    unpack_row_length,
                    ..
                } => {
                    //(gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 1);
                    if let Some(row_length) = unpack_row_length {
                        (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, *row_length as i32);
                    }
                    (
                        *width,
                        *height,
                        gl_sys::RG,
                        gl_sys::RG,
                        gl_sys::UNSIGNED_BYTE,
                        data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                        2,
                        false,
                    )
                }
                TextureFormat::VecRf32 {
                    width,
                    height,
                    data,
                    ..
                } => (
                    *width,
                    *height,
                    gl_sys::RED,
                    gl_sys::RED,
                    gl_sys::FLOAT,
                    data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void,
                    4,
                    false,
                ),
                _ => panic!("Unsupported texture format"),
            };

            // Partial texture updates don't (yet) work on OHOS simulators/emulators.

            // DISABLE PARTIAL TEXTURE UPDATES ENTIRELY. Its broken.
            const DO_PARTIAL_TEXTURE_UPDATES: bool = false; //cfg!(not(ohos_sim));

            match updated {
                TextureUpdated::Partial(rect) if DO_PARTIAL_TEXTURE_UPDATES => {
                    if needs_realloc {
                        (gl.glTexImage2D)(
                            gl_sys::TEXTURE_2D,
                            0,
                            internal_format as i32,
                            width as i32,
                            height as i32,
                            0,
                            format,
                            data_type,
                            0 as *const _,
                        );
                    }

                    (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, bytes_per_pixel);
                    (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, width as _);
                    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, rect.origin.x as i32);
                    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, rect.origin.y as i32);
                    (gl.glTexSubImage2D)(
                        gl_sys::TEXTURE_2D,
                        0,
                        rect.origin.x as i32,
                        rect.origin.y as i32,
                        rect.size.width as i32,
                        rect.size.height as i32,
                        format,
                        data_type,
                        data,
                    );
                }
                // Note: this `Partial(_)` case will only match if `DO_PARTIAL_TEXTURE_UPDATES` is false.
                TextureUpdated::Partial(_) | TextureUpdated::Full => {
                    (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, bytes_per_pixel);
                    (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, width as _);
                    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
                    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);
                    (gl.glTexImage2D)(
                        gl_sys::TEXTURE_2D,
                        0,
                        internal_format as i32,
                        width as i32,
                        height as i32,
                        0,
                        format,
                        data_type,
                        data,
                    );
                }
                TextureUpdated::Empty => panic!("already asserted that updated is not empty"),
            };

            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MIN_FILTER,
                if use_mipmaps {
                    gl_sys::LINEAR_MIPMAP_LINEAR
                } else {
                    gl_sys::LINEAR
                } as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MAG_FILTER,
                gl_sys::LINEAR as i32,
            );

            if use_mipmaps {
                if let TextureFormat::VecMipBGRAu8_32 { max_level, .. } = &self.format {
                    (gl.glTexParameteri)(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_BASE_LEVEL, 0);
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MAX_LEVEL,
                        max_level.unwrap_or(1000) as i32,
                    );
                    (gl.glGenerateMipmap)(gl_sys::TEXTURE_2D);
                }
            }

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
        }
    }

    pub fn setup_video_texture(&mut self, gl: &LibGl) -> bool {
        while unsafe { (gl.glGetError)() } != 0 {}

        if self.alloc_video() {
            self.free_previous_resources(gl);
            if self.os.gl_texture.is_none() {
                unsafe {
                    let mut gl_texture = std::mem::MaybeUninit::uninit();
                    (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                    self.os.gl_texture = Some(gl_texture.assume_init());
                }
            }
        }
        if self.take_initial() {
            unsafe {
                let gpu_renderer = get_gl_string(gl, gl_sys::RENDERER);
                if gpu_renderer.contains("Adreno") {
                    crate::warning!("WARNING: This device is using {gpu_renderer} renderer.
                    OpenGL external textures (GL_OES_EGL_image_external extension) are currently not working on makepad for most Adreno GPUs.
                    This is likely due to a driver bug. External texture support is being disabled, which means you won't be able to use the Video widget on this device.");
                }

                (gl.glBindTexture)(gl_sys::TEXTURE_EXTERNAL_OES, self.os.gl_texture.unwrap());

                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_EXTERNAL_OES,
                    gl_sys::TEXTURE_WRAP_S,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_EXTERNAL_OES,
                    gl_sys::TEXTURE_WRAP_T,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );

                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_EXTERNAL_OES,
                    gl_sys::TEXTURE_MIN_FILTER,
                    gl_sys::LINEAR as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_EXTERNAL_OES,
                    gl_sys::TEXTURE_MAG_FILTER,
                    gl_sys::LINEAR as i32,
                );

                (gl.glBindTexture)(gl_sys::TEXTURE_EXTERNAL_OES, 0);

                assert_eq!(
                    (gl.glGetError)(),
                    0,
                    "UPDATE VIDEO TEXTURE ERROR {}",
                    self.os.gl_texture.unwrap()
                );
            }
            return true;
        }
        false
    }

    pub fn update_render_target(&mut self, gl: &LibGl, width: usize, height: usize) {
        if self.alloc_render(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            if self.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                unsafe {
                    (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                    self.os.gl_texture = Some(gl_texture.assume_init());
                }
            }
            unsafe { (gl.glBindTexture)(gl_sys::TEXTURE_2D, self.os.gl_texture.unwrap()) };
            match &alloc.pixel {
                TexturePixel::BGRAu8 => unsafe {
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MIN_FILTER,
                        gl_sys::NEAREST as i32,
                    );
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MAG_FILTER,
                        gl_sys::NEAREST as i32,
                    );
                    (gl.glTexImage2D)(
                        gl_sys::TEXTURE_2D,
                        0,
                        gl_sys::RGBA as i32,
                        width as i32,
                        height as i32,
                        0,
                        gl_sys::RGBA,
                        gl_sys::UNSIGNED_BYTE,
                        ptr::null(),
                    );
                },
                TexturePixel::RGBAf16 => unsafe {
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MIN_FILTER,
                        gl_sys::NEAREST as i32,
                    );
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MAG_FILTER,
                        gl_sys::NEAREST as i32,
                    );
                    (gl.glTexImage2D)(
                        gl_sys::TEXTURE_2D,
                        0,
                        gl_sys::RGBA as i32,
                        width as i32,
                        height as i32,
                        0,
                        gl_sys::RGBA,
                        gl_sys::HALF_FLOAT,
                        ptr::null(),
                    );
                },
                TexturePixel::RGBAf32 => unsafe {
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MIN_FILTER,
                        gl_sys::NEAREST as i32,
                    );
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D,
                        gl_sys::TEXTURE_MAG_FILTER,
                        gl_sys::NEAREST as i32,
                    );
                    (gl.glTexImage2D)(
                        gl_sys::TEXTURE_2D,
                        0,
                        gl_sys::RGBA as i32,
                        width as i32,
                        height as i32,
                        0,
                        gl_sys::RGBA,
                        gl_sys::FLOAT,
                        ptr::null(),
                    );
                },
                _ => panic!(),
            }
            unsafe {
                (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
            }
        }
    }

    fn update_depth_stencil(&mut self, gl: &LibGl, width: usize, height: usize) {
        if self.alloc_depth(width, height) {
            let alloc = self.alloc.as_ref().unwrap();
            match &alloc.pixel {
                TexturePixel::D32 => unsafe {
                    if self.os.gl_renderbuffer.is_none() {
                        let mut gl_renderbuf = std::mem::MaybeUninit::uninit();
                        (gl.glGenRenderbuffers)(1, gl_renderbuf.as_mut_ptr());
                        let gl_renderbuffer = gl_renderbuf.assume_init();
                        self.os.gl_renderbuffer = Some(gl_renderbuffer);
                    }

                    (gl.glBindRenderbuffer)(gl_sys::RENDERBUFFER, self.os.gl_renderbuffer.unwrap());
                    (gl.glRenderbufferStorage)(
                        gl_sys::RENDERBUFFER,
                        gl_sys::DEPTH_COMPONENT32F,
                        width as i32,
                        height as i32,
                    );
                    (gl.glBindRenderbuffer)(gl_sys::RENDERBUFFER, 0);
                },
                _ => {
                    println!("update_platform_render_targete unsupported texture format");
                }
            }
        }
    }

    pub fn free_previous_resources(&mut self, gl: &LibGl) {
        if let Some(mut old_os) = self.previous_platform_resource.take() {
            if let Some(gl_texture) = old_os.gl_texture.take() {
                unsafe { (gl.glDeleteTextures)(1, &gl_texture) };
                crate::log!("Deleted texture: {}", gl_texture);
            }
            if let Some(gl_renderbuffer) = old_os.gl_renderbuffer.take() {
                unsafe { (gl.glDeleteRenderbuffers)(1, &gl_renderbuffer) };
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    pub shader_variant: usize,
    pub pass_uniforms: OpenglBuffer,
    pub gl_framebuffer: Option<u32>,
}

impl CxOsPass {
    pub fn free_resources(&mut self, gl: &LibGl) {
        if let Some(gl_framebuffer) = self.gl_framebuffer.take() {
            unsafe { (gl.glDeleteFramebuffers)(1, &gl_framebuffer) };
        }
    }
}

#[derive(Default, Clone)]
pub struct OpenglBuffer {
    pub gl_buffer: Option<u32>,
}

impl OpenglBuffer {
    pub fn alloc_gl_buffer(&mut self, gl: &LibGl) {
        unsafe {
            let mut gl_buffer = 0;
            (gl.glGenBuffers)(1, &mut gl_buffer);
            self.gl_buffer = Some(gl_buffer);
        }
    }

    pub fn update_array_buffer(&mut self, gl: &LibGl, data: &[f32]) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer(gl);
        }
        unsafe {
            (gl.glBindBuffer)(gl_sys::ARRAY_BUFFER, self.gl_buffer.unwrap());
            (gl.glBufferData)(
                gl_sys::ARRAY_BUFFER,
                (data.len() * mem::size_of::<f32>()) as gl_sys::GLsizeiptr,
                data.as_ptr() as *const _,
                gl_sys::STATIC_DRAW,
            );
            (gl.glBindBuffer)(gl_sys::ARRAY_BUFFER, 0);
        }
    }

    pub fn update_uniform_buffer(&mut self, gl: &LibGl, data: &[f32]) {
        if data.is_empty() {
            return;
        }
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer(gl);
        }
        unsafe {
            (gl.glBindBuffer)(gl_sys::UNIFORM_BUFFER, self.gl_buffer.unwrap());
            (gl.glBufferData)(
                gl_sys::UNIFORM_BUFFER,
                (data.len() * mem::size_of::<f32>()) as gl_sys::GLsizeiptr,
                data.as_ptr() as *const _,
                gl_sys::STATIC_DRAW,
            );
            (gl.glBindBuffer)(gl_sys::UNIFORM_BUFFER, 0);
            #[cfg(debug_assertions)]
            crate::gl_log_error!(gl);
        }
    }

    pub fn update_index_buffer(&mut self, gl: &LibGl, data: &[u32]) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer(gl);
        }
        unsafe {
            (gl.glBindBuffer)(gl_sys::ELEMENT_ARRAY_BUFFER, self.gl_buffer.unwrap());
            (gl.glBufferData)(
                gl_sys::ELEMENT_ARRAY_BUFFER,
                (data.len() * mem::size_of::<u32>()) as gl_sys::GLsizeiptr,
                data.as_ptr() as *const _,
                gl_sys::STATIC_DRAW,
            );
            (gl.glBindBuffer)(gl_sys::ELEMENT_ARRAY_BUFFER, 0);
        }
    }

    pub fn free_resources(&mut self, gl: &LibGl) {
        if let Some(gl_buffer) = self.gl_buffer.take() {
            unsafe { (gl.glDeleteBuffers)(1, &gl_buffer) };
        }
    }
}
