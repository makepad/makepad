use {
    std::{
        fs::{File, remove_file},
        io::prelude::*,
        mem,
        ptr,
        ffi::{c_char, CStr},
    },
    self::super::gl_sys,
    crate::{
        makepad_live_id::*,
        makepad_shader_compiler::generate_glsl,
        cx::{Cx, OsType, OsType::Android},
        texture::{Texture, TextureFormat, TexturePixel, TextureUpdated, CxTexture},
        makepad_math::{Mat4, DVec2, Vec4},
        pass::{PassClearColor, PassClearDepth, PassId},
        draw_list::DrawListId,
        draw_shader::{CxDrawShaderMapping, DrawShaderTextureInput},
        event::{Event, TextureHandleReadyEvent}
    },
};

impl Cx {
    
    pub (crate) fn render_view(
        &mut self,
        pass_id: PassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
    ) {
        let mut to_dispatch = Vec::new();

        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_items_len = self.draw_lists[draw_list_id].draw_items.len();
        //self.views[view_id].set_clipping_uniforms();
        self.draw_lists[draw_list_id].uniform_view_transform(&Mat4::identity());
        
        for draw_item_id in 0..draw_items_len {
            if let Some(sub_list_id) = self.draw_lists[draw_list_id].draw_items[draw_item_id].kind.sub_list() {
                self.render_view(
                    pass_id,
                    sub_list_id,
                    zbias,
                    zbias_step,
                );
            }
            else {
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_call
                }else {
                    continue;
                };
                
                let sh = &self.draw_shaders.shaders[draw_call.draw_shader.draw_shader_id];
                if sh.os_shader_id.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = &mut self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];
                
                if shp.gl_shader.is_none(){
                    shp.gl_shader = Some(GlShader::new(
                        &shp.vertex,
                        &shp.pixel,
                        &sh.mapping,
                        &self.os_type,
                    ));
                }
                let shgl = shp.gl_shader.as_ref().unwrap();
                
                if draw_call.instance_dirty || draw_item.os.inst_vb.gl_buffer.is_none(){
                    draw_call.instance_dirty = false;
                    draw_item.os.inst_vb.update_with_f32_data(draw_item.instances.as_ref().unwrap());
                }
                
                // update the zbias uniform if we have it.
                draw_call.draw_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;
                
                let instances = (draw_item.instances.as_ref().unwrap().len() / sh.mapping.instances.total_slots) as u64;
                
                if instances == 0 {
                    continue;
                }
                
                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {geometry_id}
                else {
                    continue;
                };
                
                let geometry = &mut self.geometries[geometry_id];
                if geometry.dirty || geometry.os.vb.gl_buffer.is_none() || geometry.os.ib.gl_buffer.is_none() {
                    geometry.os.vb.update_with_f32_data(&geometry.vertices);
                    geometry.os.ib.update_with_u32_data(&geometry.indices);
                    geometry.dirty = false;
                }
                
                let indices = geometry.indices.len();
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
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
                    || vao.shader_id != Some(draw_call.draw_shader.draw_shader_id) {
                        
                    if let Some(vao) = vao.vao.take(){
                        unsafe{gl_sys::DeleteVertexArrays(1, &vao)};
                    }
                        
                    vao.vao = Some(unsafe {
                        let mut vao = 0u32;
                        gl_sys::GenVertexArrays(1, &mut vao);
                        vao
                    });    
                    
                    vao.shader_id = Some(draw_call.draw_shader.draw_shader_id);
                    vao.inst_vb = draw_item.os.inst_vb.gl_buffer;
                    vao.geom_vb = geometry.os.vb.gl_buffer;
                    vao.geom_ib = geometry.os.ib.gl_buffer;
                    unsafe {
                        gl_sys::BindVertexArray(vao.vao.unwrap());
                        
                        // bind the vertex and indexbuffers
                        gl_sys::BindBuffer(gl_sys::ARRAY_BUFFER, vao.geom_vb.unwrap());
                        for attr in &shgl.geometries {
                            gl_sys::VertexAttribPointer(attr.loc, attr.size, gl_sys::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                            gl_sys::EnableVertexAttribArray(attr.loc);
                        }
                        
                        gl_sys::BindBuffer(gl_sys::ARRAY_BUFFER, vao.inst_vb.unwrap());
                        
                        for attr in &shgl.instances {
                            gl_sys::VertexAttribPointer(attr.loc, attr.size, gl_sys::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                            gl_sys::EnableVertexAttribArray(attr.loc);
                            gl_sys::VertexAttribDivisor(attr.loc, 1 as gl_sys::GLuint);
                        }
                        
                        // bind the indexbuffer
                        gl_sys::BindBuffer(gl_sys::ELEMENT_ARRAY_BUFFER, vao.geom_ib.unwrap());
                        gl_sys::BindVertexArray(0);
                    }
                }
                
                unsafe {
                    gl_sys::UseProgram(shgl.program);
                    
                    gl_sys::BindVertexArray(draw_item.os.vao.as_ref().unwrap().vao.unwrap());
                    let instances = (draw_item.instances.as_ref().unwrap().len() / sh.mapping.instances.total_slots) as u64;
                    
                    let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                    let draw_list_uniforms = draw_list.draw_list_uniforms.as_slice();
                    let draw_uniforms = draw_call.draw_uniforms.as_slice();
                    
                    GlShader::set_uniform_array(&shgl.pass_uniforms, pass_uniforms);
                    GlShader::set_uniform_array(&shgl.view_uniforms, draw_list_uniforms);
                    GlShader::set_uniform_array(&shgl.draw_uniforms, draw_uniforms);
                    GlShader::set_uniform_array(&shgl.user_uniforms, &draw_call.user_uniforms);
                    GlShader::set_uniform_array(&shgl.live_uniforms, &sh.mapping.live_uniforms_buf);
                    let ct = &sh.mapping.const_table.table;
                    if ct.len()>0 {
                        GlShader::set_uniform_array(&shgl.const_table_uniform, ct);
                    }
                    
                    // lets set our textures
                    for i in 0..sh.mapping.textures.len() {
                        let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                            texture.texture_id()
                        }else {
                            continue;
                        };
                        let cxtexture = &mut self.textures[texture_id];

                        if cxtexture.format.is_vec(){
                            cxtexture.update_vec_texture();
                        } else if cxtexture.format.is_video() {
                            let is_initial_setup = cxtexture.setup_video_texture();
                            if is_initial_setup {
                                let e = Event::TextureHandleReady(
                                    TextureHandleReadyEvent {
                                        texture_id,
                                        handle: cxtexture.os.gl_texture.unwrap()
                                    }
                                );
                                to_dispatch.push(e);
                            }
                        }
                    }
                    for i in 0..sh.mapping.textures.len() {
                        let texture_id = if let Some(texture) = &draw_call.texture_slots[i] {
                            texture.texture_id()
                        }else {
                            continue;
                        };
                        let cxtexture = &mut self.textures[texture_id];
                        // get the loc
                        gl_sys::ActiveTexture(gl_sys::TEXTURE0 + i as u32);
                        if let Some(texture) = cxtexture.os.gl_texture {
                            // Video playback with SurfaceTexture requires TEXTURE_EXTERNAL_OES, for any other format we assume regular 2D textures
                            match cxtexture.format {
                                TextureFormat::VideoRGB => gl_sys::BindTexture(gl_sys::TEXTURE_EXTERNAL_OES, texture),
                                _ => gl_sys::BindTexture(gl_sys::TEXTURE_2D, texture)     
                            }
                        }
                        else {
                            match cxtexture.format {
                                TextureFormat::VideoRGB => gl_sys::BindTexture(gl_sys::TEXTURE_EXTERNAL_OES, 0),
                                _ => gl_sys::BindTexture(gl_sys::TEXTURE_2D, 0)     
                            }
                        }
                        gl_sys::Uniform1i(shgl.textures[i].loc, i as i32);
                    }
                    
                    gl_sys::DrawElementsInstanced(
                        gl_sys::TRIANGLES,
                        indices as i32,
                        gl_sys::UNSIGNED_INT,
                        ptr::null(),
                        instances as i32
                    );
                    
                    //gl_sys::BindVertexArray(0);
                }
                
            }
        }
        for event in to_dispatch.iter() {
            self.call_event_handler(&event);
        }
    }
    
    pub fn set_default_depth_and_blend_mode() {
        unsafe {
            gl_sys::Enable(gl_sys::DEPTH_TEST);
            gl_sys::DepthFunc(gl_sys::LEQUAL);
            gl_sys::BlendEquationSeparate(gl_sys::FUNC_ADD, gl_sys::FUNC_ADD);
            gl_sys::BlendFuncSeparate(gl_sys::ONE, gl_sys::ONE_MINUS_SRC_ALPHA, gl_sys::ONE, gl_sys::ONE_MINUS_SRC_ALPHA);
            gl_sys::Enable(gl_sys::BLEND);
        }
    }
    
    pub fn setup_render_pass(&mut self, pass_id: PassId,) -> Option<DVec2> {
        
        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
        let pass_rect = self.get_pass_rect(pass_id, dpi_factor).unwrap();
        self.passes[pass_id].paint_dirty = false;
        
        if pass_rect.size.x <0.5 || pass_rect.size.y < 0.5 {
            return None
        }
        
        self.passes[pass_id].set_matrix(pass_rect.pos, pass_rect.size);
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        Some(pass_rect.size)
    }

    pub fn draw_pass_to_texture(&mut self, pass_id: PassId, texture: &Texture) {
        self.draw_pass_to_texture_inner(pass_id, Some(texture))
    }

    pub fn draw_pass_to_magic_texture(&mut self, pass_id: PassId) {
        self.draw_pass_to_texture_inner(pass_id, None)
    }

    fn draw_pass_to_texture_inner(
        &mut self,
        pass_id: PassId,
        maybe_texture: Option<&Texture>,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        let pass_size = if let Some(pz) = self.setup_render_pass(pass_id) {
            pz
        }
        else {
            return
        };
        
        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
        
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        let mut clear_color = Vec4::default();
        let mut clear_depth = 1.0;
        let mut clear_flags = 0;
        
        // make a framebuffer
        if self.passes[pass_id].os.gl_framebuffer.is_none() {
            unsafe {
                let mut gl_framebuffer = std::mem::MaybeUninit::uninit();
                gl_sys::GenFramebuffers(1, gl_framebuffer.as_mut_ptr());
                self.passes[pass_id].os.gl_framebuffer = Some(gl_framebuffer.assume_init());
            }
        }
        
        // bind the framebuffer
        unsafe {
            gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, self.passes[pass_id].os.gl_framebuffer.unwrap());
        }

        let color_textures_from_fb_texture = maybe_texture.map(|texture| {
            [crate::pass::CxPassColorTexture {
                clear_color: PassClearColor::ClearWith(self.passes[pass_id].clear_color),
                texture: texture.clone(),
            }]
        });
        let color_textures = color_textures_from_fb_texture
            .as_ref().map_or(&self.passes[pass_id].color_textures[..], |xs| &xs[..]);

        for (index, color_texture) in color_textures.iter().enumerate() {
            match color_texture.clear_color {
                PassClearColor::InitWith(_clear_color) => {
                    let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_render_target(size.x as usize, size.y as usize);
                    if cxtexture.take_initial(){
                       clear_color = _clear_color;
                       clear_flags |= gl_sys::COLOR_BUFFER_BIT;
                    }
                },
                PassClearColor::ClearWith(_clear_color) => {
                    let cxtexture = &mut self.textures[color_texture.texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_render_target(size.x as usize, size.y as usize);
                    clear_color = _clear_color;
                    clear_flags |= gl_sys::COLOR_BUFFER_BIT;
                }
            }
            if let Some(gl_texture) = self.textures[color_texture.texture.texture_id()].os.gl_texture {
                unsafe {
                    gl_sys::FramebufferTexture2D(gl_sys::FRAMEBUFFER, gl_sys::COLOR_ATTACHMENT0 + index as u32, gl_sys::TEXTURE_2D, gl_texture, 0);
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture) = &self.passes[pass_id].depth_texture {
            match self.passes[pass_id].clear_depth {
                PassClearDepth::InitWith(_clear_depth) => {
                    let cxtexture = &mut self.textures[depth_texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_depth_stencil(size.x as usize, size.y as usize);
                    if cxtexture.take_initial(){
                        clear_depth = _clear_depth;
                        clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                    }
                },
                PassClearDepth::ClearWith(_clear_depth) => {
                    let cxtexture = &mut self.textures[depth_texture.texture_id()];
                    let size = dpi_factor * pass_size;
                    cxtexture.update_depth_stencil(size.x as usize, size.y as usize);
                    clear_depth = _clear_depth;
                    clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                }
            }
        }
        else {
            /* unsafe { // BUGFIX. we have to create a depthbuffer for rtt without depthbuffer use otherwise it fails if there is another pass with depth
                if self.passes[pass_id].os.gl_bugfix_depthbuffer.is_none() {
                    let mut gl_renderbuf = std::mem::MaybeUninit::uninit();
                    gl_sys::GenRenderbuffers(1, gl_renderbuf.as_mut_ptr());
                    let gl_renderbuffer = gl_renderbuf.assume_init();
                    gl_sys::BindRenderbuffer(gl_sys::RENDERBUFFER, gl_renderbuffer);
                    gl_sys::RenderbufferStorage(
                        gl_sys::RENDERBUFFER,
                        gl_sys::DEPTH_COMPONENT16,
                        (pass_size.x * dpi_factor) as i32,
                        (pass_size.y * dpi_factor) as i32
                    );
                    gl_sys::BindRenderbuffer(gl_sys::RENDERBUFFER, 0);
                    self.passes[pass_id].os.gl_bugfix_depthbuffer = Some(gl_renderbuffer);
                }
                clear_depth = 1.0;
                clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                gl_sys::Disable(gl_sys::DEPTH_TEST);
                gl_sys::FramebufferRenderbuffer(gl_sys::FRAMEBUFFER, gl_sys::DEPTH_ATTACHMENT, gl_sys::RENDERBUFFER, self.passes[pass_id].os.gl_bugfix_depthbuffer.unwrap());
            }*/
        }

        // HACK(eddyb) drain error queue, so that we can check erors below.
        while unsafe { gl_sys::GetError() } != 0 {}

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

            gl_sys::Viewport(x as i32, y as i32, width as i32, height as i32);
            
            assert_eq!(gl_sys::GetError(), 0, "glViewport({x}, {y}, {width}, {height}) failed");
        }

        if clear_flags != 0 {
            unsafe {
                if clear_flags & gl_sys::DEPTH_BUFFER_BIT != 0 {
                    gl_sys::ClearDepthf(clear_depth);
                }
                gl_sys::ClearColor(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                gl_sys::Clear(clear_flags);
            }
        }
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
        );
        
        unsafe {
            gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, 0);
            //gl_sys::Finish();
        }
    }
    
    pub fn opengl_compile_shaders(&mut self) {
        //let p = profile_start();
        for draw_shader_ptr in &self.draw_shaders.compile_set {
            if let Some(item) = self.draw_shaders.ptr_to_item.get(&draw_shader_ptr) {
                let cx_shader = &mut self.draw_shaders.shaders[item.draw_shader_id];
                let draw_shader_def = self.shader_registry.draw_shader_defs.get(&draw_shader_ptr);
                
                let vertex = generate_glsl::generate_vertex_shader(
                    draw_shader_def.as_ref().unwrap(),
                    &cx_shader.mapping.const_table,
                    &self.shader_registry
                );
                let pixel = generate_glsl::generate_pixel_shader(
                    draw_shader_def.as_ref().unwrap(),
                    &cx_shader.mapping.const_table,
                    &self.shader_registry
                );
                
                if cx_shader.mapping.flags.debug {
                    crate::log!("{}\n{}", vertex, pixel);
                }
                
                // lets see if we have the shader already
                for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                    if ds.vertex == vertex && ds.pixel == pixel {
                        cx_shader.os_shader_id = Some(index);
                        break;
                    }
                }
                
                if cx_shader.os_shader_id.is_none() {
                    let shp = CxOsDrawShader::new(&vertex, &pixel, &self.os_type);
                    cx_shader.os_shader_id = Some(self.draw_shaders.os_shaders.len());
                    self.draw_shaders.os_shaders.push(shp);
                }
            }
        }
        self.draw_shaders.compile_set.clear();
    }

    pub fn maybe_warn_hardware_support(&self) {
        // Temporary warning for Adreno failing at compiling shaders that use samplerExternalOES.
        let gpu_renderer = get_gl_string(gl_sys::RENDERER);
        if gpu_renderer.contains("Adreno") {
            crate::log!("WARNING: This device is using {gpu_renderer} renderer.
            OpenGL external textures (GL_OES_EGL_image_external extension) are currently not working on makepad for most Adreno GPUs.
            This is likely due to a driver bug. External texture support is being disabled, which means you won't be able to use the Video widget on this device.");
        }
    }
}


#[derive(Clone)]
pub struct CxOsDrawShader {
    pub gl_shader: Option<GlShader>,
    pub vertex: String,
    pub pixel: String,
}

#[derive(Clone)]
pub struct GlShader {
    pub program: u32,
    pub geometries: Vec<OpenglAttribute>,
    pub instances: Vec<OpenglAttribute>,
    pub textures: Vec<OpenglUniform>,
    pub pass_uniforms: OpenglUniform,
    pub view_uniforms: OpenglUniform,
    pub draw_uniforms: OpenglUniform,
    pub user_uniforms: OpenglUniform,
    pub live_uniforms: OpenglUniform,
    pub const_table_uniform: OpenglUniform,
}

impl GlShader{
    pub fn new(vertex: &str, pixel: &str, mapping: &CxDrawShaderMapping, os_type: &OsType)->Self{
        unsafe fn read_cache(vertex: &str, pixel: &str, os_type: &OsType) -> Option<gl_sys::GLuint> {
            if let Some(cache_dir) = os_type.get_cache_dir() {
                let shader_hash = live_id!(shader).str_append(&vertex).str_append(&pixel);
                let mut base_filename = format!("{}/shader_{:08x}", cache_dir, shader_hash.0);

                match os_type {
                    OsType::Android(params) => {
                        base_filename = format!("{}_av{}_bn{}_kv{}", base_filename, params.android_version, params.build_number, params.kernel_version);
                    },
                    _ => (),
                };

                let filename = format!("{}.bin", base_filename);

                if let Ok(mut cache_file) = File::open(&filename) {
                    let mut binary = Vec::new();
                    let mut format_bytes = [0u8; 4];
                    if cache_file.read(&mut format_bytes).is_ok() {
                        let binary_format = u32::from_be_bytes(format_bytes);
                        if cache_file.read_to_end(&mut binary).is_ok() {
                            let mut version_consistency_conflict = false;
                            // On Android, invalidate the cached file if there have been significant system updates
                            match os_type {
                                OsType::Android(params) => {
                                    let current_filename = format!("{}/shader_{:08x}_av{}_bn{}_kv{}.bin", cache_dir, shader_hash.0, params.android_version, params.build_number, params.kernel_version);
                                    version_consistency_conflict = filename != current_filename;
                                },
                                _ => (),
                            };
            
                            if !version_consistency_conflict {
                                let program = gl_sys::CreateProgram();
                                gl_sys::ProgramBinary(program, binary_format, binary.as_ptr() as *const _, binary.len() as i32);
                                return Some(program);
                            } else {
                                // Version mismatch, delete the old cache file
                                let _ = remove_file(&filename);
                            }
                        }
                    }
                }
            }
            None
        }
        
        unsafe {
            let program = if let Some(program) = read_cache(&vertex,&pixel,os_type){
                program
            }
            else{ 
                let vs = gl_sys::CreateShader(gl_sys::VERTEX_SHADER);
                gl_sys::ShaderSource(vs, 1, [vertex.as_ptr() as *const _].as_ptr(), ptr::null());
                gl_sys::CompileShader(vs);
                //println!("{}", Self::opengl_get_info_log(true, vs as usize, &vertex));
                if let Some(error) = Self::opengl_has_shader_error(true, vs as usize, &vertex) {
                    panic!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}", error);
                }
                let fs = gl_sys::CreateShader(gl_sys::FRAGMENT_SHADER);
                gl_sys::ShaderSource(fs, 1, [pixel.as_ptr() as *const _].as_ptr(), ptr::null());
                gl_sys::CompileShader(fs);
                //println!("{}", Self::opengl_get_info_log(true, fs as usize, &fragment));
                if let Some(error) = Self::opengl_has_shader_error(true, fs as usize, &pixel) {
                    panic!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}", error);
                }
                
                let program = gl_sys::CreateProgram();
                gl_sys::AttachShader(program, vs);
                gl_sys::AttachShader(program, fs);
                gl_sys::LinkProgram(program);
                if let Some(error) = Self::opengl_has_shader_error(false, program as usize, "") {
                    panic!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}", error);
                }
                gl_sys::DeleteShader(vs);
                gl_sys::DeleteShader(fs);
                program
            };
            
            if let Some(cache_dir) = os_type.get_cache_dir() {
                let mut binary = Vec::new();
                let mut binary_len = 0;
                gl_sys::GetProgramiv(program, gl_sys::PROGRAM_BINARY_LENGTH, &mut binary_len);
                if binary_len != 0 {
                    binary.resize(binary_len as usize, 0u8);
                    let mut return_size = 0i32;
                    let mut binary_format = 0u32;
                    gl_sys::GetProgramBinary(program, binary.len() as i32, &mut return_size as *mut _, &mut binary_format as *mut _, binary.as_mut_ptr() as *mut _);
                    if return_size != 0 {
                        //log!("GOT FORMAT {}", format);
                        let shader_hash = live_id!(shader).str_append(&vertex).str_append(&pixel);
                        let mut filename = format!("{}/shader_{:08x}", cache_dir, shader_hash.0);

                        match os_type {
                            Android(params) => {
                                filename = format!("{}_av{}_bn{}_kv{}", filename, params.android_version, params.build_number, params.kernel_version);
                            },
                            _ => (),
                        };

                        filename = format!("{}.bin", filename);

                        binary.resize(return_size as usize, 0u8);
                        if let Ok(mut cache) = File::create(filename) {
                            let _ = cache.write_all(&binary_format.to_be_bytes());
                            let _ = cache.write_all(&binary);
                        }
                    }
                } 
            }

            Self{
                program,
                geometries:Self::opengl_get_attributes(program, "packed_geometry_", mapping.geometries.total_slots),
                instances: Self::opengl_get_attributes(program, "packed_instance_", mapping.instances.total_slots),
                textures: Self::opengl_get_texture_slots(program, &mapping.textures),
                pass_uniforms: Self::opengl_get_uniform(program, "pass_table"),
                view_uniforms: Self::opengl_get_uniform(program, "view_table"),
                draw_uniforms: Self::opengl_get_uniform(program, "draw_table"),
                user_uniforms: Self::opengl_get_uniform(program, "user_table"),
                live_uniforms: Self::opengl_get_uniform(program, "live_table"),
                const_table_uniform: Self::opengl_get_uniform(program, "const_table"),
            }
        }
    }

    
    pub fn set_uniform_array(loc: &OpenglUniform, array: &[f32]) {
        unsafe {
            gl_sys::Uniform1fv(loc.loc as i32, array.len() as i32, array.as_ptr());
        }
    }
    
    pub fn opengl_get_uniform(program: u32, name: &str) -> OpenglUniform {
        let mut name0 = String::new();
        name0.push_str(name);
        name0.push_str("\0");
        unsafe {
            OpenglUniform {
                loc: gl_sys::GetUniformLocation(program, name0.as_ptr() as *const _),
                //name: name.to_string(),
            }
        }
    }
    
    pub fn opengl_get_info_log(compile: bool, shader: usize, source: &str) -> String {
        unsafe {
            let mut length = 0;
            if compile {
                gl_sys::GetShaderiv(shader as u32, gl_sys::INFO_LOG_LENGTH, &mut length);
            } else {
                gl_sys::GetProgramiv(shader as u32, gl_sys::INFO_LOG_LENGTH, &mut length);
            }
            let mut log = Vec::with_capacity(length as usize);
            if compile {
                gl_sys::GetShaderInfoLog(shader as u32, length, ptr::null_mut(), log.as_mut_ptr());
            } else {
                gl_sys::GetProgramInfoLog(shader as u32, length, ptr::null_mut(), log.as_mut_ptr());
            }
            log.set_len(length as usize);
            let mut r = "".to_string();
            r.push_str(CStr::from_ptr(log.as_ptr()).to_str().unwrap());
            r.push_str("\n");
            let split = source.split("\n");
            for (line, chunk) in split.enumerate() {
                r.push_str(&(line + 1).to_string());
                r.push_str(":");
                r.push_str(chunk);
                r.push_str("\n");
            }
            r
        }
    }
    
    pub fn opengl_has_shader_error(compile: bool, shader: usize, source: &str) -> Option<String> {
        //None
        unsafe {
            
            let mut success = gl_sys::TRUE as i32;
            
            if compile {
                gl_sys::GetShaderiv(shader as u32, gl_sys::COMPILE_STATUS, &mut success);
            }
            else {
                gl_sys::GetProgramiv(shader as u32, gl_sys::LINK_STATUS, &mut success);
            };
            
            if success != gl_sys::TRUE as i32 {
                Some(Self::opengl_get_info_log(compile, shader, source))
            }
            else {
                None
            }
        }
    }
    
    pub fn opengl_get_attributes(program: u32, prefix: &str, slots: usize) -> Vec<OpenglAttribute> {
        let mut attribs = Vec::new();
        
        fn ceil_div4(base: usize) -> usize {
            let r = base >> 2;
            if base & 3 != 0 {
                return r + 1
            }
            r
        }
        
        let stride = (slots * mem::size_of::<f32>()) as i32;
        let num_attr = ceil_div4(slots);
        for i in 0..num_attr {
            let mut name0 = prefix.to_string();
            name0.push_str(&i.to_string());
            name0.push_str("\0");
            
            let mut size = ((slots - i * 4)) as i32;
            if size > 4 {
                size = 4;
            }
            unsafe {
                attribs.push(
                    OpenglAttribute {
                        loc: {
                            let loc = gl_sys::GetAttribLocation(program, name0.as_ptr() as *const _) as u32;
                            loc
                        },
                        offset: (i * 4 * mem::size_of::<f32>()) as usize,
                        size: size,
                        stride: stride
                    }
                )
            }
        }
        attribs
    }
    
    
    pub fn opengl_get_texture_slots(program: u32, texture_slots: &Vec<DrawShaderTextureInput>) -> Vec<OpenglUniform> {
        let mut gl_texture_slots = Vec::new();
        
        for slot in texture_slots {
            let mut name0 = "ds_".to_string();
            name0.push_str(&slot.id.to_string());
            name0.push_str("\0");
            unsafe {
                gl_texture_slots.push(OpenglUniform {
                    loc: gl_sys::GetUniformLocation(program, name0.as_ptr() as *const _),
                })
            }
        }
        gl_texture_slots
    }

    pub fn free_resources(self){
        unsafe{
            gl_sys::DeleteShader(self.program);
        }
    }
}

impl CxOsDrawShader {
    pub fn new(vertex: &str, pixel: &str, os_type: &OsType) -> Self {
        // Check if GL_OES_EGL_image_external extension is available in the current device, otherwise do not attempt to use in the shaders.
        let available_extensions = get_gl_string(gl_sys::EXTENSIONS);
        let is_external_texture_supported = available_extensions.split_whitespace().any(|ext| ext == "GL_OES_EGL_image_external");

        let mut maybe_ext_tex_extension_import = String::new();
        let mut maybe_ext_tex_extension_sampler = String::new();

        // GL_OES_EGL_image_external is not well supported on Android emulators with macOS hosts.
        // Because there's no bullet-proof way to check the emualtor host at runtime, we're currently disabling external texture support on all emulators.
        let is_emulator = match os_type {
            Android(params) => params.is_emulator,
            _ => false,
        };

        // Some Android devices running Adreno GPUs suddenly stopped compiling shaders when passing the samplerExternalOES sampler to texture2D functions. 
        // This seems like a driver bug (no confirmation from Qualcomm yet).
        // Therefore we're disabling the external texture support for Adreno until this is fixed.
        let is_vendor_adreno = get_gl_string(gl_sys::RENDERER).contains("Adreno"); 
        if is_external_texture_supported && !is_vendor_adreno && !is_emulator {
            maybe_ext_tex_extension_import = "#extension GL_OES_EGL_image_external : require\n".to_string();
            maybe_ext_tex_extension_sampler = "vec4 sample2dOES(samplerExternalOES sampler, vec2 pos){{ return texture2D(sampler, vec2(pos.x, pos.y));}}".to_string();
        }
        
        let vertex = format!("
            #version 100
            {}
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, pos.y));}} 
            vec4 sample2d_rt(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            mat4 transpose(mat4 m){{return mat4(m[0][0],m[1][0],m[2][0],m[3][0],m[0][1],m[1][1],m[2][1],m[3][1],m[0][2],m[1][2],m[2][2],m[3][3], m[3][0], m[3][1], m[3][2], m[3][3]);}}
            mat3 transpose(mat3 m){{return mat3(m[0][0],m[1][0],m[2][0],m[0][1],m[1][1],m[2][1],m[0][2],m[1][2],m[2][2]);}}
            mat2 transpose(mat2 m){{return mat2(m[0][0],m[1][0],m[0][1],m[1][1]);}}
            {}\0", maybe_ext_tex_extension_import, vertex);

        let pixel = format!("
            #version 100
            #extension GL_OES_standard_derivatives : enable
            {}
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, pos.y));}}
            vec4 sample2d_rt(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            {}
            mat4 transpose(mat4 m){{return mat4(m[0][0],m[1][0],m[2][0],m[3][0],m[0][1],m[1][1],m[2][1],m[3][1],m[0][2],m[1][2],m[2][2],m[3][3], m[3][0], m[3][1], m[3][2], m[3][3]);}}
            mat3 transpose(mat3 m){{return mat3(m[0][0],m[1][0],m[2][0],m[0][1],m[1][1],m[2][1],m[0][2],m[1][2],m[2][2]);}}
            mat2 transpose(mat2 m){{return mat2(m[0][0],m[1][0],m[0][1],m[1][1]);}}
            {}\0", maybe_ext_tex_extension_import, maybe_ext_tex_extension_sampler, pixel);
        
            // lets fetch the uniform positions for our uniforms
        CxOsDrawShader {
            vertex,
            pixel,
            gl_shader: None,
        }
    }

    pub fn free_resources(&mut self){
        if let Some(gl_shader) = self.gl_shader.take(){
            gl_shader.free_resources();
        }
    }
}


fn get_gl_string(key: gl_sys::types::GLenum) -> String {
    unsafe {
        let string_ptr = gl_sys::GetString(key) as *const c_char;
        if string_ptr == ptr::null(){
            return String::new()
        }
        CStr::from_ptr(string_ptr).to_string_lossy().into_owned()
    }
}

#[derive(Default, Clone)]
pub struct OpenglAttribute {
    pub loc: u32,
    pub size: i32,
    pub offset: usize,
    pub stride: i32
}

#[derive(Debug, Default, Clone)]
pub struct OpenglUniform {
    pub loc: i32,
    //pub name: String,
}


#[derive(Clone, Default)]
pub struct CxOsGeometry {
    pub vb: OpenglBuffer,
    pub ib: OpenglBuffer,
}

impl CxOsGeometry{
    pub fn free_resources(&mut self){
        self.vb.free_resources();
        self.ib.free_resources();
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
pub struct CxOsView {
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
    pub fn free(self){
        if let Some(vao) = self.vao{
            unsafe{gl_sys::DeleteVertexArrays(1, &vao)};
        }
    }    
}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub inst_vb: OpenglBuffer,
    pub vao: Option<CxOsDrawCallVao>,
}

impl CxOsDrawCall {
    pub fn free_resources(&mut self){
        self.inst_vb.free_resources();
        if let Some(vao) = self.vao.take(){
            vao.free();
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
    pub fn update_vec_texture(&mut self) {
        let mut newly_allocated = false;
        if self.alloc_vec() {
            if let Some(previous) = self.previous_platform_resource.take() {
                self.os = previous;
            } 
            
            if self.os.gl_texture.is_none() {
                unsafe {
                    let mut gl_texture = std::mem::MaybeUninit::uninit();
                    gl_sys::GenTextures(1, gl_texture.as_mut_ptr());
                    self.os.gl_texture = Some(gl_texture.assume_init());
                }
            }
            newly_allocated = true;
        }
    
        let updated = self.take_updated();
        if updated.is_empty() {
            return;
        }
        
        unsafe {
            gl_sys::BindTexture(gl_sys::TEXTURE_2D, self.os.gl_texture.unwrap());
            gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as i32);
            gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as i32);
    
            // Set texture parameters based on the format
            let (width, height, internal_format, format, data_type, data, size_per_pixel, use_mipmaps) = match &self.format {
                TextureFormat::VecBGRAu8_32{width, height, data, ..} => 
                    (*width, *height, gl_sys::BGRA, gl_sys::BGRA, gl_sys::UNSIGNED_BYTE, data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void, 4, false),
                TextureFormat::VecMipBGRAu8_32{width, height, data, max_level: _, ..} => 
                    (*width, *height, gl_sys::BGRA, gl_sys::BGRA, gl_sys::UNSIGNED_BYTE, data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void, 4, true),
                TextureFormat::VecRGBAf32{width, height, data, ..} => 
                    (*width, *height, gl_sys::RGBA, gl_sys::RGBA, gl_sys::FLOAT, data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void, 16, false),
                TextureFormat::VecRu8{width, height, data, unpack_row_length, ..} => {
                    gl_sys::PixelStorei(gl_sys::UNPACK_ALIGNMENT, 1);
                    if let Some(row_length) = unpack_row_length {
                        gl_sys::PixelStorei(gl_sys::UNPACK_ROW_LENGTH, *row_length as i32);
                    }
                    (*width, *height, gl_sys::R8, gl_sys::RED, gl_sys::UNSIGNED_BYTE, data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void, 1, false)
                },
                TextureFormat::VecRGu8{width, height, data, unpack_row_length, ..} => {
                    gl_sys::PixelStorei(gl_sys::UNPACK_ALIGNMENT, 1);
                    if let Some(row_length) = unpack_row_length {
                        gl_sys::PixelStorei(gl_sys::UNPACK_ROW_LENGTH, *row_length as i32);
                    }
                    (*width, *height, gl_sys::RG, gl_sys::RG, gl_sys::UNSIGNED_BYTE, data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void, 2, false)
                },
                TextureFormat::VecRf32{width, height, data, ..} => 
                    (*width, *height, gl_sys::RED, gl_sys::RED, gl_sys::FLOAT, data.as_ref().unwrap().as_ptr() as *const std::ffi::c_void, 4, false),
                _ => panic!("Unsupported texture format"),
            };
    
            let mut current_width = 0;
            let mut current_height = 0;
            gl_sys::GetTexLevelParameteriv(gl_sys::TEXTURE_2D, 0, gl_sys::TEXTURE_WIDTH, &mut current_width);
            gl_sys::GetTexLevelParameteriv(gl_sys::TEXTURE_2D, 0, gl_sys::TEXTURE_HEIGHT, &mut current_height);
    
            match updated {
                TextureUpdated::Partial(rect) => {
                    /*if newly_allocated {
                        gl_sys::TexImage2D(
                            gl_sys::TEXTURE_2D,
                            0,
                            internal_format as i32,
                            width as i32, height as i32,
                            0,
                            format,
                            data_type,
                            0 as *const _
                        );
                    }
                    gl_sys::PixelStorei(gl_sys::UNPACK_ROW_LENGTH, width as _);
                    gl_sys::TexSubImage2D(
                        gl_sys::TEXTURE_2D,
                        0,
                        rect.origin.x as i32,
                        rect.origin.y as i32,
                        rect.size.width as i32,
                        rect.size.height as i32,
                        format,
                        data_type,
                        (data as *const u8).add((rect.origin.y * width + rect.origin.x) * size_per_pixel) as *const std::ffi::c_void,
                    );*/
                    gl_sys::TexImage2D(
                        gl_sys::TEXTURE_2D,
                        0,
                        internal_format as i32,
                        width as i32, height as i32,
                        0,
                        format,
                        data_type,
                        data
                    );
                },
                TextureUpdated::Full => {
                    gl_sys::TexImage2D(
                        gl_sys::TEXTURE_2D,
                        0,
                        internal_format as i32,
                        width as i32, height as i32,
                        0,
                        format,
                        data_type,
                        data
                    );
                },
                TextureUpdated::Empty => panic!("already asserted that updated is not empty"),
            };
    
            gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, if use_mipmaps { gl_sys::LINEAR_MIPMAP_LINEAR } else { gl_sys::LINEAR } as i32);
            gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
    
            if use_mipmaps {
                if let TextureFormat::VecMipBGRAu8_32{max_level, ..} = &self.format {
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_BASE_LEVEL, 0);
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAX_LEVEL, max_level.unwrap_or(1000) as i32);
                    gl_sys::GenerateMipmap(gl_sys::TEXTURE_2D);
                }
            }
    
            gl_sys::BindTexture(gl_sys::TEXTURE_2D, 0);
        }
    }

    pub fn setup_video_texture(&mut self) -> bool {
        while unsafe { gl_sys::GetError() } != 0 {}

        if self.alloc_video() {
            self.free_previous_resources();
            if self.os.gl_texture.is_none() { 
                unsafe {
                    let mut gl_texture = std::mem::MaybeUninit::uninit();
                    gl_sys::GenTextures(1, gl_texture.as_mut_ptr());
                    self.os.gl_texture = Some(gl_texture.assume_init());
                }
            }
        }
        if self.take_initial() {
            unsafe{
                gl_sys::BindTexture(gl_sys::TEXTURE_EXTERNAL_OES, self.os.gl_texture.unwrap());
        
                gl_sys::TexParameteri(gl_sys::TEXTURE_EXTERNAL_OES, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as i32);
                gl_sys::TexParameteri(gl_sys::TEXTURE_EXTERNAL_OES, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as i32);

                gl_sys::TexParameteri(gl_sys::TEXTURE_EXTERNAL_OES, gl_sys::TEXTURE_MIN_FILTER, gl_sys::LINEAR as i32);
                gl_sys::TexParameteri(gl_sys::TEXTURE_EXTERNAL_OES, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
        
                gl_sys::BindTexture(gl_sys::TEXTURE_EXTERNAL_OES, 0);

                assert_eq!(gl_sys::GetError(), 0, "UPDATE VIDEO TEXTURE ERROR {}", self.os.gl_texture.unwrap());
            }
            return true;
        }
        false
    }
    
    pub fn update_render_target(&mut self, width: usize, height: usize) {
        if self.alloc_render(width, height){
            let alloc = self.alloc.as_ref().unwrap();
            if self.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                unsafe{
                    gl_sys::GenTextures(1, gl_texture.as_mut_ptr());
                    self.os.gl_texture = Some(gl_texture.assume_init());
                }
            }
            unsafe{gl_sys::BindTexture(gl_sys::TEXTURE_2D, self.os.gl_texture.unwrap())};
            match &alloc.pixel {
                TexturePixel::BGRAu8 => unsafe{
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, gl_sys::NEAREST as i32);
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::NEAREST as i32);
                    gl_sys::TexImage2D(
                        gl_sys::TEXTURE_2D,
                        0,
                        gl_sys::RGBA as i32,
                        width as i32,
                        height as i32,
                        0,
                        gl_sys::RGBA,
                        gl_sys::UNSIGNED_BYTE,
                        ptr::null()
                    );
                },
                TexturePixel::RGBAf16 => unsafe{
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, gl_sys::NEAREST as i32);
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::NEAREST as i32);
                    gl_sys::TexImage2D(
                        gl_sys::TEXTURE_2D,
                        0,
                        gl_sys::RGBA as i32,
                        width as i32,
                        height as i32,
                        0,
                        gl_sys::RGBA,
                        gl_sys::HALF_FLOAT,
                        ptr::null()
                    );
                }
                TexturePixel::RGBAf32 => unsafe{
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MIN_FILTER, gl_sys::NEAREST as i32);
                    gl_sys::TexParameteri(gl_sys::TEXTURE_2D, gl_sys::TEXTURE_MAG_FILTER, gl_sys::NEAREST as i32);
                    gl_sys::TexImage2D(
                        gl_sys::TEXTURE_2D,
                        0,
                        gl_sys::RGBA as i32,
                        width as i32,
                        height as i32,
                        0,
                        gl_sys::RGBA,
                        gl_sys::FLOAT,
                        ptr::null()
                    );
                }
                _ => panic!()
            }
            unsafe{
                gl_sys::BindTexture(gl_sys::TEXTURE_2D, 0);
            }
        }
    }
    
    fn update_depth_stencil(
        &mut self,
        width: usize,
        height: usize
    ) {
        if self.alloc_depth(width, height){
                   
            let alloc = self.alloc.as_ref().unwrap();
            match &alloc.pixel {
                TexturePixel::D32 => unsafe{
                    if self.os.gl_renderbuffer.is_none() {
                        let mut gl_renderbuf = std::mem::MaybeUninit::uninit();
                        gl_sys::GenRenderbuffers(1, gl_renderbuf.as_mut_ptr());
                        let gl_renderbuffer = gl_renderbuf.assume_init();
                        self.os.gl_renderbuffer = Some(gl_renderbuffer);
                    }
                        
                    gl_sys::BindRenderbuffer(gl_sys::RENDERBUFFER, self.os.gl_renderbuffer.unwrap());
                    gl_sys::RenderbufferStorage(
                        gl_sys::RENDERBUFFER,
                        gl_sys::DEPTH_COMPONENT32F,
                        width as i32,
                        height as i32
                    );
                    gl_sys::BindRenderbuffer(gl_sys::RENDERBUFFER, 0);
                },
                _ => {
                    println!("update_platform_render_targete unsupported texture format");
                }
            }
        }
    }
    
    pub fn free_previous_resources(&mut self){
        if let Some(mut old_os) = self.previous_platform_resource.take(){
            if let Some(gl_texture) = old_os.gl_texture.take(){
                unsafe{gl_sys::DeleteTextures(1, &gl_texture)};
                crate::log!("Deleted texture: {}", gl_texture);
            }
            if let Some(gl_renderbuffer) = old_os.gl_renderbuffer.take(){
                unsafe{gl_sys::DeleteRenderbuffers(1, &gl_renderbuffer)};
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    pub gl_framebuffer: Option<u32>,
}

impl CxOsPass{
    
    pub fn free_resources(&mut self){
        if let Some(gl_framebuffer) = self.gl_framebuffer.take(){
            unsafe{gl_sys::DeleteFramebuffers(1, &gl_framebuffer)};
        }
    }    
}

#[derive(Default, Clone)]
pub struct OpenglBuffer {
    pub gl_buffer: Option<u32>
}

impl OpenglBuffer {
    
    pub fn alloc_gl_buffer(&mut self) {
        unsafe {
            let mut gl_buffer = std::mem::MaybeUninit::uninit();
            gl_sys::GenBuffers(1, gl_buffer.as_mut_ptr());
            self.gl_buffer = Some(gl_buffer.assume_init());
        }
    }
    
    pub fn update_with_f32_data(&mut self, data: &Vec<f32>) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer();
        }
        unsafe {
            gl_sys::BindBuffer(gl_sys::ARRAY_BUFFER, self.gl_buffer.unwrap());
            gl_sys::BufferData(
                gl_sys::ARRAY_BUFFER,
                (data.len() * mem::size_of::<f32>()) as gl_sys::types::GLsizeiptr,
                data.as_ptr() as *const _,
                gl_sys::STATIC_DRAW
            );
        }
    }
    
    pub fn update_with_u32_data(&mut self, data: &Vec<u32>) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer();
        }
        unsafe {
            gl_sys::BindBuffer(gl_sys::ELEMENT_ARRAY_BUFFER, self.gl_buffer.unwrap());
            gl_sys::BufferData(
                gl_sys::ELEMENT_ARRAY_BUFFER,
                (data.len() * mem::size_of::<u32>()) as gl_sys::types::GLsizeiptr,
                data.as_ptr() as *const _,
                gl_sys::STATIC_DRAW
            );
        }
    }
    
    pub fn free_resources(&mut self){
        if let Some(gl_buffer) = self.gl_buffer.take(){
            unsafe{gl_sys::DeleteBuffers(1, &gl_buffer)};
        }
    }

}
