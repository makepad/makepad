use {
    std::{
        mem,
        os::raw::{c_ulong, c_void},
        ptr,
        ffi::CStr,
    },
    crate::{
        cx::Cx,
        window::WindowId,
        makepad_math::{Mat4, DVec2, Rect, Vec4},
        pass::{PassClearColor, PassClearDepth, PassId},
        draw_list::DrawListId,
        event::*,
        os::linux::x11_sys,
        os::linux::glx_sys,
        os::linux::gl_sys,
        os::linux::xlib_window::*,
    },
};

impl Cx {
    
    fn render_view(
        &mut self,
        pass_id: PassId,
        draw_list_id: DrawListId,
        zbias: &mut f32,
        zbias_step: f32,
        opengl_cx: &OpenglCx,
    ) {
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
                    opengl_cx,
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
                
                let sh = &self.draw_shaders[draw_call.draw_shader.draw_shader_id];
                if sh.os_shader_id.is_none() { // shader didnt compile somehow
                    continue;
                }
                let shp = &self.draw_shaders.os_shaders[sh.os_shader_id.unwrap()];
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    draw_item.os.inst_vb.update_with_f32_data(opengl_cx, draw_item.instances.as_ref().unwrap());
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
                    geometry.os.vb.update_with_f32_data(opengl_cx, &geometry.vertices);
                    geometry.os.ib.update_with_u32_data(opengl_cx, &geometry.indices);
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
                        vao: unsafe {
                            let mut vao = std::mem::MaybeUninit::uninit();
                            gl_sys::GenVertexArrays(1, vao.as_mut_ptr());
                            vao.assume_init()
                        },
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
                    
                    vao.shader_id = Some(draw_call.draw_shader.draw_shader_id);
                    vao.inst_vb = draw_item.os.inst_vb.gl_buffer;
                    vao.geom_vb = geometry.os.vb.gl_buffer;
                    vao.geom_ib = geometry.os.ib.gl_buffer;
                    
                    unsafe {
                        gl_sys::BindVertexArray(vao.vao);
                        
                        // bind the vertex and indexbuffers
                        gl_sys::BindBuffer(gl_sys::ARRAY_BUFFER, vao.geom_vb.unwrap());
                        for attr in &shp.geometries {
                            gl_sys::VertexAttribPointer(attr.loc, attr.size, gl_sys::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                            gl_sys::EnableVertexAttribArray(attr.loc);
                        }
                        
                        gl_sys::BindBuffer(gl_sys::ARRAY_BUFFER, vao.inst_vb.unwrap());
                        
                        for attr in &shp.instances {
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
                    gl_sys::UseProgram(shp.program);
                    gl_sys::BindVertexArray(draw_item.os.vao.as_ref().unwrap().vao);
                    let instances = (draw_item.instances.as_ref().unwrap().len() 
                        / sh.mapping.instances.total_slots) as u64;
                    
                    let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                    let draw_list_uniforms = draw_list.draw_list_uniforms.as_slice();
                    let draw_uniforms = draw_call.draw_uniforms.as_slice();
                    
                    opengl_cx.set_uniform_array(&shp.pass_uniforms, pass_uniforms);
                    opengl_cx.set_uniform_array(&shp.draw_list_uniforms, draw_list_uniforms);
                    opengl_cx.set_uniform_array(&shp.draw_uniforms, draw_uniforms);
                    opengl_cx.set_uniform_array(&shp.user_uniforms, &draw_call.user_uniforms);
                    opengl_cx.set_uniform_array(&shp.live_uniforms, &sh.mapping.live_uniforms_buf);
                    
                    let ct = &sh.mapping.const_table.table;
                    if ct.len()>0 {
                        opengl_cx.set_uniform_array(&shp.const_table_uniform, ct);
                    }
                    
                    // lets set our textures
                    for i in 0..sh.mapping.textures.len() {
                        
                        let texture_id = if let Some(texture_id) = draw_call.texture_slots[i] {
                            texture_id
                        }else {
                            continue;
                        };
                        
                        let cxtexture = &mut self.textures[texture_id];
                        if cxtexture.update_image {
                            cxtexture.update_image = false;
                            opengl_cx.update_platform_texture_image2d(cxtexture);
                        }
                        // get the loc
                        gl_sys::ActiveTexture(gl_sys::TEXTURE0 + i as u32);
                        if let Some(texture) = cxtexture.os.gl_texture {
                            gl_sys::BindTexture(gl_sys::TEXTURE_2D, texture);
                        }
                        else {
                            gl_sys::BindTexture(gl_sys::TEXTURE_2D, 0);
                        }
                    }
                    
                    gl_sys::DrawElementsInstanced(
                        gl_sys::TRIANGLES,
                        indices as i32,
                        gl_sys::UNSIGNED_INT,
                        ptr::null(),
                        instances as i32
                    );
                    
                    gl_sys::BindVertexArray(0);
                }
            }
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
    
    pub fn setup_render_pass(&mut self, pass_id: PassId, inherit_dpi_factor: f64) {
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_matrix(DVec2::default(), pass_size);
        self.passes[pass_id].paint_dirty = false;
        
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
    }
    
    pub fn draw_pass_to_window(
        &mut self,
        pass_id: PassId,
        dpi_factor: f64,
        opengl_window: &mut OpenglWindow,
        opengl_cx: &OpenglCx,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        let window = opengl_window.xlib_window.window.unwrap();
        
        self.passes[pass_id].paint_dirty = false;
        
        let pix_width = opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor;
        let pix_height = opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor;
        
        unsafe {
            glx_sys::glXMakeCurrent(opengl_cx.display, window, opengl_cx.context);
            gl_sys::Viewport(0, 0, pix_width as i32, pix_height as i32);
        }
        
        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            self.passes[pass_id].clear_color
        }
        else {
            match self.passes[pass_id].color_textures[0].clear_color {
                PassClearColor::InitWith(color) => color,
                PassClearColor::ClearWith(color) => color
            }
        };
        let clear_depth = match self.passes[pass_id].clear_depth {
            PassClearDepth::InitWith(depth) => depth,
            PassClearDepth::ClearWith(depth) => depth
        };
        
        unsafe {
            gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, 0);
            gl_sys::ClearDepth(clear_depth as f64);
            gl_sys::ClearColor(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
            gl_sys::Clear(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
        }
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            &opengl_cx,
        );
        
        unsafe {
            glx_sys::glXSwapBuffers(opengl_cx.display, window);
        }
    }
    
    pub fn draw_pass_to_texture(
        &mut self,
        pass_id: PassId,
        dpi_factor: f64,
        opengl_cx: &OpenglCx,
    ) {
        let pass_size = self.passes[pass_id].pass_size;
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            dpi_factor
        };
        
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
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            match color_texture.clear_color {
                PassClearColor::InitWith(_clear_color) => {
                    if opengl_cx.update_platform_render_target(&mut self.textures[color_texture.texture_id], dpi_factor, pass_size, false) {
                        clear_color = _clear_color;
                        clear_flags |= gl_sys::COLOR_BUFFER_BIT;
                    }
                },
                PassClearColor::ClearWith(_clear_color) => {
                    opengl_cx.update_platform_render_target(&mut self.textures[color_texture.texture_id], dpi_factor, pass_size, false);
                    clear_color = _clear_color;
                    clear_flags |= gl_sys::COLOR_BUFFER_BIT;
                }
            }
            if let Some(gl_texture) = self.textures[color_texture.texture_id].os.gl_texture {
                unsafe {
                    gl_sys::FramebufferTexture2D(gl_sys::FRAMEBUFFER, gl_sys::COLOR_ATTACHMENT0 + index as u32, gl_sys::TEXTURE_2D, gl_texture, 0);
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            match self.passes[pass_id].clear_depth {
                PassClearDepth::InitWith(_clear_depth) => {
                    if opengl_cx.update_platform_render_target(&mut self.textures[depth_texture_id], dpi_factor, pass_size, true) {
                        clear_depth = _clear_depth;
                        clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                    }
                },
                PassClearDepth::ClearWith(_clear_depth) => {
                    opengl_cx.update_platform_render_target(&mut self.textures[depth_texture_id], dpi_factor, pass_size, true);
                    clear_depth = _clear_depth;
                    clear_flags |= gl_sys::DEPTH_BUFFER_BIT;
                }
            }
        }
        else {
            unsafe { // BUGFIX. we have to create a depthbuffer for rtt without depthbuffer use otherwise it fails if there is another pass with depth
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
            }
        }
        
        
        unsafe {
            gl_sys::Viewport(0, 0, (pass_size.x * dpi_factor) as i32, (pass_size.y * dpi_factor) as i32);
        }
        if clear_flags != 0 {
            unsafe {
                gl_sys::ClearDepth(clear_depth as f64);
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
            &opengl_cx,
        );
        unsafe {
            gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, 0);
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
            
            if success != gl_sys::TRUE as i32{
                Some(Self::opengl_get_info_log(compile, shader, source))
            }
            else {
                None
            }
        }
    }
    
    pub fn ceil_div4(base: usize) -> usize {
        let r = base >> 2;
        if base & 3 != 0 {
            return r + 1
        }
        r
    }
    
    pub fn opengl_get_attributes(program: u32, prefix: &str, slots: usize) -> Vec<OpenglAttribute> {
        let mut attribs = Vec::new();
        
        let stride = (slots * mem::size_of::<f32>()) as i32;
        let num_attr = Self::ceil_div4(slots);
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
    /*
    pub fn opengl_get_uniforms(program: u32, unis: &Vec<PropDef>) -> Vec<OpenglUniform> {
        let mut gl_uni = Vec::new();
        for uni in unis {
            gl_uni.push(
                Self::opengl_get_uniform(program, &uni.name, uni.ty.size())
            );
        }
        gl_uni
    }*/
    
    pub fn opengl_get_uniform(program: u32, name: &str, size: usize) -> OpenglUniform {
        let mut name0 = String::new();
        name0.push_str(name);
        name0.push_str("\0");
        unsafe {
            OpenglUniform {
                loc: gl_sys::GetUniformLocation(program, name0.as_ptr() as *const _),
                name: name.to_string(),
                size: size
            }
        }
    }
    
    pub fn opengl_get_texture_slots(program: u32, texture_slots: &Vec<PropDef>) -> Vec<OpenglUniform> {
        let mut gl_texture_slots = Vec::new();
        
        for slot in texture_slots {
            let mut name0 = "".to_string();
            name0.push_str(&slot.name);
            name0.push_str("\0");
            unsafe {
                gl_texture_slots.push(OpenglUniform {
                    loc: gl_sys::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name: slot.name.clone(),
                    size: 0
                    //,sampler:sam.sampler.clone()
                })
            }
        }
        gl_texture_slots
    }
    
    pub fn opengl_compile_all_shaders(&mut self, opengl_cx: &OpenglCx) {
        unsafe {
            glx_sys::glXMakeCurrent(opengl_cx.display, opengl_cx.hidden_window, opengl_cx.context);
        }
        let options = ShaderCompileOptions {
            gather_all: false,
            create_const_table: false,
            no_const_collapse: false
        };
        
        for (live_id, shader) in &self.live_styles.shader_alloc {
            match self.live_styles.collect_and_analyse_shader(*live_id, options) {
                Err(err) => {
                    eprintln!("{}", err);
                    panic!()
                },
                Ok((shader_ast, default_geometry)) => {
                    let shader_id = shader.shader_id;
                    Self::opengl_compile_shader(
                        shader_id,
                        &mut self.shaders[shader_id],
                        shader_ast,
                        default_geometry,
                        options,
                        opengl_cx,
                        &self.live_styles
                    );
                }
            }
        };
        self.live_styles.changed_shaders.clear();
    }
    
    
    pub fn opengl_update_all_shaders(&mut self, opengl_cx: &OpenglCx, errors: &mut Vec<LiveBodyError>) {
        
        // recompile shaders, and update values
        
        let options = ShaderCompileOptions {
            gather_all: true,
            create_const_table: true,
            no_const_collapse: false
        };
        
        for (live_id, change) in &self.live_styles.changed_shaders {
            match change {
                LiveChangeType::Recompile => {
                    match self.live_styles.collect_and_analyse_shader(*live_id, options) {
                        Err(err) => {
                            errors.push(err);
                        },
                        Ok((shader_ast, default_geometry)) => {
                            let shader_id = self.live_styles.shader_alloc.get(&live_id).unwrap().shader_id;
                            Self::opengl_compile_shader(
                                shader_id,
                                &mut self.shaders[shader_id],
                                shader_ast,
                                default_geometry,
                                options,
                                opengl_cx,
                                &self.live_styles
                            );
                        }
                    }
                }
                LiveChangeType::UpdateValue => {
                    let shader_id = self.live_styles.shader_alloc.get(&live_id).unwrap().shader_id;
                    self.shaders[shader_id].mapping.update_live_uniforms(&self.live_styles);
                }
            }
        }
        self.live_styles.changed_shaders.clear();
    }
    
    
    pub fn opengl_compile_shader(
        shader_id: usize,
        sh: &mut CxShader,
        shader_ast: ShaderAst,
        default_geometry: Option<Geometry>,
        options: ShaderCompileOptions,
        _opengl_cx: &OpenglCx,
        live_styles: &LiveStyles
    ) -> ShaderCompileResult {
        
        // lets generate the vertexshader
        let vertex = generate_glsl::generate_vertex_shader(&shader_ast, live_styles, options);
        let fragment = generate_glsl::generate_fragment_shader(&shader_ast, live_styles, options);
        
        let vertex = format!("
            #version 100
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            {}\0", vertex);
        let fragment = format!("
            #version 100
            #extension GL_OES_standard_derivatives : enable
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            {}\0", fragment);
        
        if shader_ast.debug {
            println!("--------------- Vertex shader {} --------------- \n{}\n---------------\n", shader_id, vertex);
            println!("--------------- Fragment shader {} --------------- \n{}\n---------------\n", shader_id, fragment);
        }
        
        let mut mapping = CxShaderMapping::from_shader_ast(shader_ast, options, false);
        mapping.update_live_uniforms(live_styles);
        
        if let Some(sh_platform) = &sh.platform {
            if sh_platform.vertex == vertex && sh_platform.fragment == fragment {
                sh.mapping = mapping;
                return ShaderCompileResult::Nop
            }
        }
        
        //println!("{} {} {}", sh.name, vertex, fragment);
        unsafe {
            
            let vs = gl::CreateShader(gl::VERTEX_SHADER);
            gl::ShaderSource(vs, 1, [vertex.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(vs);
            //println!("{}", Self::opengl_get_info_log(true, vs as usize, &vertex));
            if let Some(error) = Self::opengl_has_shader_error(true, vs as usize, &vertex) {
                if options.create_const_table {
                    println!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}", error);
                    return ShaderCompileResult::Nop
                }
                panic!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}", error);
            }
            let fs = gl::CreateShader(gl::FRAGMENT_SHADER);
            gl::ShaderSource(fs, 1, [fragment.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(fs);
            //println!("{}", Self::opengl_get_info_log(true, fs as usize, &fragment));
            if let Some(error) = Self::opengl_has_shader_error(true, fs as usize, &fragment) {
                if options.create_const_table {
                    println!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}", error);
                    return ShaderCompileResult::Nop
                }
                panic!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}", error);
            }
            
            let program = gl::CreateProgram();
            gl::AttachShader(program, vs);
            gl::AttachShader(program, fs);
            gl::LinkProgram(program);
            if let Some(error) = Self::opengl_has_shader_error(false, program as usize, "") {
                if options.create_const_table {
                    println!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}", error);
                    return ShaderCompileResult::Nop
                }
                panic!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}", error);
            }
            gl::DeleteShader(vs);
            gl::DeleteShader(fs);
            
            let geometries = Self::opengl_get_attributes(program, "mpsc_packed_geometry_", mapping.geometry_props.total_slots);
            let instances = Self::opengl_get_attributes(program, "mpsc_packed_instance_", mapping.instance_props.total_slots);
            
            // lets fetch the uniform positions for our uniforms
            sh.default_geometry = default_geometry;
            sh.platform = Some(CxOsShader {
                program: program,
                geometries,
                instances,
                vertex,
                fragment,
                pass_uniforms: Self::opengl_get_uniforms(program, &mapping.pass_uniforms),
                view_uniforms: Self::opengl_get_uniforms(program, &mapping.view_uniforms),
                draw_uniforms: Self::opengl_get_uniforms(program, &mapping.draw_uniforms),
                live_uniforms: Self::opengl_get_uniforms(program, &mapping.live_uniforms),
                const_table_uniform: Self::opengl_get_uniform(program, "mpsc_const_table", 1),
                user_uniforms: Self::opengl_get_uniforms(program, &mapping.user_uniforms),
            });
            sh.mapping = mapping;
            return ShaderCompileResult::Ok
            
        }
    }
}

pub struct OpenglCx {
    pub display: *mut x11_sys::Display,
    pub context: glx_sys::GLXContext,
    pub visual_info: x11_sys::XVisualInfo,
    pub hidden_window: x11_sys::Window,
}

impl OpenglCx {
    pub fn new(display: *mut x11_sys::Display) -> OpenglCx {
        unsafe {
            // Query GLX version.
            let mut major = 0;
            let mut minor = 0;
            assert!(
                glx_sys::glXQueryVersion(display, &mut major, &mut minor) >= 0,
                "can't query GLX version"
            );
            
            // Check that GLX version number is 1.4 or higher.
            assert!(
                major > 1 || major == 1 && minor >= 4,
                "GLX version must be 1.4 or higher, got {}.{}",
                major,
                minor,
            );
            
            let screen = glx_sys::XDefaultScreen(display);
            
            // Query extensions string
            let supported_extensions = glx_sys::glXQueryExtensionsString(display, screen);
            assert!(
                !supported_extensions.is_null(),
                "can't query GLX extensions string"
            );
            let supported_extensions = CStr::from_ptr(supported_extensions).to_str().unwrap();
            
            // Check that required extensions are supported.
            let required_extensions = &["GLX_ARB_get_proc_address", "GLX_ARB_create_context"];
            for required_extension in required_extensions {
                assert!(
                    supported_extensions.contains(required_extension),
                    "extension {} is required, but not supported",
                    required_extension,
                );
            }
            
            // Load GLX function pointers.
            #[allow(non_snake_case)]
            let glXCreateContextAttribsARB = mem::transmute::<
                _,
                glx_sys::PFNGLXCREATECONTEXTATTRIBSARBPROC,
            >(glx_sys::glXGetProcAddressARB(
                CString::new("glXCreateContextAttribsARB")
                    .unwrap()
                    .to_bytes_with_nul()
                    .as_ptr(),
            ))
                .expect("can't load glXCreateContextAttribsARB function pointer");
            
            // Load GL function pointers.
            gl::load_with( | symbol | {
                glx_sys::glXGetProcAddressARB(
                    CString::new(symbol).unwrap().to_bytes_with_nul().as_ptr(),
                )
                    .map_or(ptr::null(), | ptr | ptr as *const c_void)
            });
            
            // Choose framebuffer configuration.
            let config_attribs = &[
                glx_sys::GLX_DOUBLEBUFFER as i32,
                glx_sys::True as i32,
                glx_sys::GLX_RED_SIZE as i32,
                8,
                glx_sys::GLX_GREEN_SIZE as i32,
                8,
                glx_sys::GLX_BLUE_SIZE as i32,
                8,
                glx_sys::GLX_ALPHA_SIZE as i32,
                8,
                glx_sys::None as i32,
            ];
            let mut config_count = 0;
            let configs = glx_sys::glXChooseFBConfig(
                display,
                glx_sys::XDefaultScreen(display),
                config_attribs.as_ptr(),
                &mut config_count,
            );
            if configs.is_null() {
                panic!("can't choose framebuffer configuration");
            }
            let config = *configs;
            glx_sys::XFree(configs as *mut c_void);
            
            // Create GLX context.
            let context_attribs = &[
                glx_sys::GLX_CONTEXT_MAJOR_VERSION_ARB as i32,
                3,
                glx_sys::GLX_CONTEXT_MINOR_VERSION_ARB as i32,
                0,
                glx_sys::GLX_CONTEXT_PROFILE_MASK_ARB as i32,
                glx_sys::GLX_CONTEXT_ES_PROFILE_BIT_EXT as i32,
                glx_sys::None as i32
            ];
            let context = glXCreateContextAttribsARB(
                display,
                config,
                ptr::null_mut(),
                glx_sys::True as i32,
                context_attribs.as_ptr(),
            );
            
            // Get visual from framebuffer configuration.
            let visual_info_ptr = glx_sys::glXGetVisualFromFBConfig(display, config);
            assert!(
                !visual_info_ptr.is_null(),
                "can't get visual from framebuffer configuration"
            );
            let visual_info = *visual_info_ptr;
            glx_sys::XFree(visual_info_ptr as *mut c_void);
            
            let root_window = glx_sys::XRootWindow(display, screen);
            
            // Create hidden window compatible with visual
            //
            // We need a hidden window because we sometimes want to create OpenGL resources, such as
            // shaders, when Makepad does not have any windows open. In cases such as these, we need
            // *some* window to make the OpenGL context current on.
            let mut attributes = mem::zeroed::<glx_sys::XSetWindowAttributes>();
            
            // We need a color map that is compatible with our visual. Otherwise, the call to
            // XCreateWindow below will fail.
            attributes.colormap = glx_sys::XCreateColormap(
                display,
                root_window,
                visual_info.visual,
                glx_sys::AllocNone as i32
            );
            let hidden_window = glx_sys::XCreateWindow(
                display,
                root_window,
                0,
                0,
                16,
                16,
                0,
                visual_info.depth,
                glx_sys::InputOutput as u32,
                visual_info.visual,
                glx_sys::CWColormap as c_ulong,
                &mut attributes,
            );
            
            // To make sure the window stays hidden, we simply never call XMapWindow on it.
            
            OpenglCx {
                display,
                context,
                visual_info,
                hidden_window,
            }
        }
    }
    
    pub fn set_uniform_array(&self, loc: &OpenglUniform, array: &[f32]) {
        unsafe {
            gl::Uniform1fv(loc.loc as i32, array.len() as i32, array.as_ptr());
        }
    }
    /*
    pub fn set_uniform_buffer(&self, locs: &Vec<OpenglUniform>, uni: &[f32]) {
        
        let mut o = 0;
        for loc in locs {
            if o + loc.size > uni.len() {
                return
            }
            if (o & 3) != 0 && (o & 3) + loc.size > 4 { // goes over the boundary
                o += 4 - (o & 3); // make jump to new slot
            }
            if loc.loc >= 0 {
                unsafe {
                    match loc.size {
                        1 => {
                            gl::Uniform1f(loc.loc as i32, uni[o]);
                        },
                        2 => gl::Uniform2f(loc.loc as i32, uni[o], uni[o + 1]),
                        3 => gl::Uniform3f(loc.loc as i32, uni[o], uni[o + 1], uni[o + 2]),
                        4 => {
                            gl::Uniform4f(loc.loc as i32, uni[o], uni[o + 1], uni[o + 2], uni[o + 3]);
                        },
                        16 => {
                            gl::UniformMatrix4fv(loc.loc as i32, 1, 0, uni.as_ptr().offset((o) as isize));
                        },
                        _ => ()
                    }
                }
            };
            o = o + loc.size;
        }
    }*/
    
    pub fn update_platform_texture_image2d(&self, cxtexture: &mut CxTexture) {
        
        if cxtexture.desc.width.is_none() || cxtexture.desc.height.is_none() {
            println!("update_platform_texture_image2d without width/height");
            return;
        }
        
        let width = cxtexture.desc.width.unwrap();
        let height = cxtexture.desc.height.unwrap();
        
        // allocate new texture if descriptor change
        if cxtexture.platform.alloc_desc != cxtexture.desc {
            
            cxtexture.platform.alloc_desc = cxtexture.desc.clone();
            cxtexture.platform.width = width as u64;
            cxtexture.platform.height = height as u64;
            
            let gl_texture = match cxtexture.platform.gl_texture {
                None => {
                    unsafe {
                        let mut gl_texture = std::mem::MaybeUninit::uninit();
                        gl::GenTextures(1, gl_texture.as_mut_ptr());
                        let gl_texture = gl_texture.assume_init();
                        cxtexture.platform.gl_texture = Some(gl_texture);
                        gl_texture
                    }
                }
                Some(gl_texture_old) => {
                    gl_texture_old
                }
            };
            unsafe {
                gl::BindTexture(gl::TEXTURE_2D, gl_texture);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
                gl::TexImage2D(gl::TEXTURE_2D, 0, gl::RGBA as i32, width as i32, height as i32, 0, gl::RGBA, gl::UNSIGNED_BYTE, cxtexture.image_u32.as_ptr() as *const _);
                gl::BindTexture(gl::TEXTURE_2D, 0);
            }
        }
        
        cxtexture.update_image = false;
    }
    
    pub fn update_platform_render_target(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2, is_depth: bool) -> bool {
        let width = if let Some(width) = cxtexture.desc.width {width as u64} else {(size.x * dpi_factor) as u64};
        let height = if let Some(height) = cxtexture.desc.height {height as u64} else {(size.y * dpi_factor) as u64};
        
        if cxtexture.platform.width == width && cxtexture.platform.height == height && cxtexture.platform.alloc_desc == cxtexture.desc {
            return false
        }
        
        unsafe {
            
            cxtexture.platform.alloc_desc = cxtexture.desc.clone();
            cxtexture.platform.width = width;
            cxtexture.platform.height = height;
            if let Some(gl_texture) = cxtexture.platform.gl_texture {
                gl::DeleteTextures(1, &gl_texture);
            }
            cxtexture.platform.gl_texture = None;
            
            if let Some(gl_renderbuffer) = cxtexture.platform.gl_renderbuffer {
                gl::DeleteTextures(1, &gl_renderbuffer);
            }
            cxtexture.platform.gl_renderbuffer = None;
            
            
            if !is_depth {
                match cxtexture.desc.format {
                    TextureFormat::Default | TextureFormat::RenderBGRA => {
                        
                        let mut gl_texture = std::mem::MaybeUninit::uninit();
                        gl::GenTextures(1, gl_texture.as_mut_ptr());
                        let gl_texture = gl_texture.assume_init();
                        gl::BindTexture(gl::TEXTURE_2D, gl_texture);
                        
                        cxtexture.platform.gl_texture = Some(gl_texture);
                        
                        gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
                        gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
                        gl::TexImage2D(gl::TEXTURE_2D, 0, gl::RGBA as i32, width as i32, height as i32, 0, gl::RGBA, gl::UNSIGNED_BYTE, ptr::null());
                    },
                    _ => {
                        println!("update_platform_render_target unsupported texture format");
                        return false;
                    }
                }
            }
            else {
                match cxtexture.desc.format {
                    TextureFormat::Default | TextureFormat::Depth32Stencil8 => {
                        
                        let mut gl_renderbuf = std::mem::MaybeUninit::uninit();
                        gl::GenRenderbuffers(1, gl_renderbuf.as_mut_ptr());
                        let gl_renderbuffer = gl_renderbuf.assume_init();
                        gl::BindRenderbuffer(gl::RENDERBUFFER, gl_renderbuffer);
                        gl::RenderbufferStorage(
                            gl::RENDERBUFFER,
                            gl::DEPTH_COMPONENT32F,
                            width as i32,
                            height as i32
                        );
                        gl::BindRenderbuffer(gl::RENDERBUFFER, 0);
                        cxtexture.platform.gl_renderbuffer = Some(gl_renderbuffer);
                        // this doesnt work.
                        //gl::TexParameteri(gl::TEXTURE_2D, gl::DEPTH_STENCIL_TEXTURE_MODE, gl::LINEAR as i32);
                        //gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
                        /*
                        gl::TexImage2D(gl::TEXTURE_2D, 0, gl::DEPTH_COMPONENT as i32, width as i32, height as i32, 0, gl::DEPTH_COMPONENT, gl::UNSIGNED_SHORT, ptr::null());
                        loop{
                            let err = gl::GetError();
                            if err == gl::NO_ERROR{
                                break;
                            }
                            println!("GOT ERROR {} {} {:x}", width, height, err);
                        }
                        //println!("Depth stencil texture!");
                        */
                    },
                    _ => {
                        println!("update_platform_render_targete unsupported texture format");
                        return false;
                    }
                }
            }
            
        }
        return true;
    }
}

#[derive(Clone)]
pub struct CxOsDrawShader {
    pub program: u32,
    pub vertex: String,
    pub fragment: String,
    pub geometries: Vec<OpenglAttribute>,
    pub instances: Vec<OpenglAttribute>,
    
    pub pass_uniforms: OpenglUniform,
    pub draw_list_uniforms: OpenglUniform,
    pub draw_uniforms: OpenglUniform,
    pub user_uniforms: OpenglUniform,
    pub live_uniforms: OpenglUniform,
    pub const_table_uniform: OpenglUniform,
}


#[derive(Clone)]
pub struct OpenglWindow {
    pub first_draw: bool,
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    pub opening_repaint_count: u32,
    pub cal_size: Vec2,
    pub xlib_window: XlibWindow,
}

impl OpenglWindow {
    pub fn new(window_id: usize, opengl_cx: &OpenglCx, xlib_app: &mut XlibApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> OpenglWindow {
        
        let mut xlib_window = XlibWindow::new(xlib_app, window_id);
        
        let visual_info = unsafe {mem::transmute(opengl_cx.visual_info)};
        xlib_window.init(title, inner_size, position, visual_info);
        
        OpenglWindow {
            first_draw: true,
            window_id,
            opening_repaint_count: 0,
            cal_size: Vec2::default(),
            window_geom: xlib_window.get_window_geom(),
            xlib_window
        }
    }
    
    pub fn resize_framebuffer(&mut self, _opengl_cx: &OpenglCx) -> bool {
        let cal_size = DVec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            // resize the framebuffer
            true
        }
        else {
            false
        }
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
    pub name: String,
    pub size: usize
}


#[derive(Clone, Default)]
pub struct CxOsGeometry {
    pub vb: OpenglBuffer,
    pub ib: OpenglBuffer,
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
    pub vao: u32,
    pub shader_id: Option<usize>,
    pub inst_vb: Option<u32>,
    pub geom_vb: Option<u32>,
    pub geom_ib: Option<u32>,
}


#[derive(Default, Clone)]
pub struct CxOsDrawCall {
    pub inst_vb: OpenglBuffer,
    pub vao: Option<CxOsDrawCallVao>,
}

impl CxOsDrawCall {
}

#[derive(Default, Clone)]
pub struct CxOsTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub gl_texture: Option<u32>,
    pub gl_renderbuffer: Option<u32>
}

#[derive(Default, Clone)]
pub struct CxOsPass {
    pub gl_framebuffer: Option<u32>,
    pub gl_bugfix_depthbuffer: Option<u32>
}

#[derive(Default, Clone)]
pub struct OpenglBuffer {
    pub gl_buffer: Option<u32>
}

impl OpenglBuffer {
    
    pub fn alloc_gl_buffer(&mut self) {
        unsafe {
            let mut gl_buffer = std::mem::MaybeUninit::uninit();
            gl::GenBuffers(1, gl_buffer.as_mut_ptr());
            self.gl_buffer = Some(gl_buffer.assume_init());
        }
    }
    
    pub fn update_with_f32_data(&mut self, _opengl_cx: &OpenglCx, data: &Vec<f32>) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer();
        }
        unsafe {
            gl::BindBuffer(gl::ARRAY_BUFFER, self.gl_buffer.unwrap());
            gl::BufferData(gl::ARRAY_BUFFER, (data.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr, data.as_ptr() as *const _, gl::STATIC_DRAW);
        }
    }
    
    pub fn update_with_u32_data(&mut self, _opengl_cx: &OpenglCx, data: &Vec<u32>) {
        if self.gl_buffer.is_none() {
            self.alloc_gl_buffer();
        }
        unsafe {
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.gl_buffer.unwrap());
            gl::BufferData(gl::ELEMENT_ARRAY_BUFFER, (data.len() * mem::size_of::<u32>()) as gl::types::GLsizeiptr, data.as_ptr() as *const _, gl::STATIC_DRAW);
        }
    }
}
