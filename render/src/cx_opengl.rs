use crate::cx::*;
use crate::cx_xlib::*;
use std::ffi::{CStr, CString};
use std::ptr;
use std::mem;

fn get_egl_error_string(error: i32) -> &'static str {
    match error as u32 {
        EGL_sys::EGL_NOT_INITIALIZED => "EGL_NOT_INITIALIZED",
        EGL_sys::EGL_BAD_ACCESS => "EGL_BAD_ACCESS",
        EGL_sys::EGL_BAD_ALLOC => "EGL_BAD_ALLOC",
        EGL_sys::EGL_BAD_ATTRIBUTE => "EGL_BAD_ATTRIBUTE",
        EGL_sys::EGL_BAD_CONTEXT => "EGL_BAD_CONTEXT",
        EGL_sys::EGL_BAD_CONFIG => "EGL_BAD_CONFIG",
        EGL_sys::EGL_BAD_CURRENT_SURFACE => "EGL_BAD_CURRENT_SURFACE",
        EGL_sys::EGL_BAD_DISPLAY => "EGL_BAD_DISPLAY",
        EGL_sys::EGL_BAD_SURFACE => "EGL_BAD_SURFACE",
        EGL_sys::EGL_BAD_MATCH => "EGL_BAD_MATCH",
        EGL_sys::EGL_BAD_PARAMETER => "EGL_BAD_PARAMETER",
        EGL_sys::EGL_BAD_NATIVE_PIXMAP => "EGL_NATIVE_PIXMAP",
        EGL_sys::EGL_BAD_NATIVE_WINDOW => "EGL_BAD_NATIVE_WINDOW",
        EGL_sys::EGL_CONTEXT_LOST => "EGL_CONTEXT_LOST",
        _ => panic!(),
    }
}

impl Cx {
    
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, full_repaint: bool, view_rect: &Rect, opengl_cx: &OpenglCx, zbias: &mut f32, zbias_step: f32) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        if !full_repaint && !view_rect.intersects(self.views[view_id].rect) {
            return
        }
        self.views[view_id].set_clipping_uniforms();
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, full_repaint, view_rect, opengl_cx, zbias, zbias_step);
            }
            else {
                let cxview = &mut self.views[view_id];
                //view.platform.uni_vw.update_with_f32_data(device, &view.uniforms);
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    draw_call.platform.inst_vbuf.update_with_f32_data(opengl_cx, &draw_call.instance);
                }
                
                draw_call.platform.check_vao(draw_call.shader_id, &shp);
                
                if draw_call.uniforms.len() > 0 {
                    if let Some(zbias_offset) = sh.mapping.zbias_uniform_prop {
                        draw_call.uniforms[zbias_offset] = *zbias;
                        *zbias += zbias_step;
                    }
                }
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                
                unsafe {
                    gl::UseProgram(shp.program);
                    gl::BindVertexArray(draw_call.platform.vao.unwrap());
                    let instances = draw_call.instance.len() / sh.mapping.instance_slots;
                    let indices = sh.shader_gen.geometry_indices.len();
                    
                    let cxuniforms = &self.passes[pass_id].uniforms;
                    
                    opengl_cx.set_uniform_buffer(&shp.uniforms_cx, &cxuniforms);
                    opengl_cx.set_uniform_buffer(&shp.uniforms_vw, &cxview.uniforms);
                    opengl_cx.set_uniform_buffer(&shp.uniforms_dr, &draw_call.uniforms);
                    
                    // lets set our textures
                    for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                        let cxtexture = &mut self.textures[*texture_id as usize];
                        if cxtexture.update_image {
                            opengl_cx.update_platform_texture_image2d(cxtexture);
                        }
                        // get the loc
                        gl::ActiveTexture(gl::TEXTURE0 + i as u32);
                        if let Some(texture) = cxtexture.platform.gl_texture {
                            gl::BindTexture(gl::TEXTURE_2D, texture);
                        }
                        else {
                            gl::BindTexture(gl::TEXTURE_2D, 0);
                        }
                    }
                    
                    gl::DrawElementsInstanced(
                        gl::TRIANGLES,
                        indices as i32,
                        gl::UNSIGNED_INT,
                        ptr::null(),
                        instances as i32
                    );
                }
            }
        }
    }
    
    pub fn calc_dirty_bounds(&mut self, pass_id: usize, view_id: usize, view_bounds: &mut ViewBounds) {
        let draw_calls_len = self.views[view_id].draw_calls_len;
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.calc_dirty_bounds(pass_id, sub_view_id, view_bounds)
            }
            else {
                let cxview = &mut self.views[view_id];
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                //let sh = &self.shaders[draw_call.shader_id];
                //let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty || draw_call.uniforms_dirty {
                    view_bounds.add_rect(&cxview.rect);
                }
            }
        }
    }

    pub fn set_default_depth_and_blend_mode(){
        unsafe{
            gl::Enable(gl::DEPTH_TEST);
            gl::DepthFunc(gl::LEQUAL);
            gl::BlendEquationSeparate(gl::FUNC_ADD, gl::FUNC_ADD);
            gl::BlendFuncSeparate(gl::ONE, gl::ONE_MINUS_SRC_ALPHA, gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
            gl::Enable(gl::BLEND);
        }
    }
    
    pub fn draw_pass_to_window(
        &mut self,
        pass_id: usize,
        dpi_factor: f32,
        opengl_window: &mut OpenglWindow,
        opengl_cx: &OpenglCx,
    ) -> bool {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let mut view_bounds = ViewBounds::new();
        let mut init_repaint = false;
        self.calc_dirty_bounds(pass_id, view_id, &mut view_bounds);
        
        let full_repaint = true; // view_bounds.max_x - view_bounds.min_x > opengl_window.window_geom.inner_size.x - 100.
            // && view_bounds.max_y - view_bounds.min_y > opengl_window.window_geom.inner_size.y - 100. ||
        // opengl_window.opening_repaint_count < 10;
        if opengl_window.opening_repaint_count < 10 { // for some reason the first repaint doesn't arrive on the window
            opengl_window.opening_repaint_count += 1;
            init_repaint = true;
        }
        let window;
        let surface;
        let view_rect;
        if full_repaint {
            opengl_window.xlib_window.hide_child_windows();
            
            window = opengl_window.xlib_window.window.unwrap();

            surface = unsafe {
                if opengl_window.xlib_window.surface.is_none() {
                    // Create EGL window surface
                    let surface = EGL_sys::eglCreateWindowSurface(opengl_cx.display, opengl_cx.config, window, ptr::null_mut());
                    if surface.is_null() {
                        panic!("can't create EGL window surface: {}", get_egl_error_string(EGL_sys::eglGetError()));
                    }
                    opengl_window.xlib_window.surface = Some(surface);
                }
                opengl_window.xlib_window.surface.unwrap()
            };
            
            let pass_size = self.passes[pass_id].pass_size;
            self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
            
            let pix_width = opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor;
            let pix_height = opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor;

            unsafe {
                // Make EGL context current
                if EGL_sys::eglMakeCurrent(opengl_cx.display, surface, surface, opengl_cx.context) == EGL_sys::EGL_FALSE {

                    panic!("can't make EGL context current: {}", get_egl_error_string(EGL_sys::eglGetError()));
                }
                gl::Viewport(0, 0, pix_width as i32, pix_height as i32);
            }
            view_rect = Rect::zero();
        }
        else {
            if view_bounds.max_x == std::f32::NEG_INFINITY
                || view_bounds.max_y == std::f32::NEG_INFINITY
                || view_bounds.min_x == std::f32::INFINITY
                || view_bounds.min_x == std::f32::INFINITY
                || view_bounds.min_x == view_bounds.max_x
                || view_bounds.min_y == view_bounds.max_y {
                return false
            }
            /*
            unsafe {
                GLX_sys::glXMakeCurrent(xlib_app.display, opengl_window.xlib_window.window.unwrap(), opengl_cx.context);
                gl::Viewport(
                    0,
                    0,
                    (opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor) as i32,
                    (opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor) as i32
                );
                gl::ClearColor(0.0, 1.0, 0.0, 0.0);
                gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
                GLX_sys::glXSwapBuffers(xlib_app.display, opengl_window.xlib_window.window.unwrap());
            }*/
            
            let pix_width = (view_bounds.max_x - view_bounds.min_x) * opengl_window.window_geom.dpi_factor;
            let pix_height = (view_bounds.max_y - view_bounds.min_y) * opengl_window.window_geom.dpi_factor;
            
            window = opengl_window.xlib_window.alloc_child_window(
                (view_bounds.min_x * opengl_window.window_geom.dpi_factor) as i32,
                (view_bounds.min_y * opengl_window.window_geom.dpi_factor) as i32,
                pix_width as u32,
                pix_height as u32
            ).unwrap();

            surface = unsafe {
                if opengl_window.xlib_window.surface.is_none() {
                    // Create EGL window surface
                    let surface = EGL_sys::eglCreateWindowSurface(opengl_cx.display, opengl_cx.config, window, ptr::null_mut());
                    if surface.is_null() {
                        panic!("can't create EGL window surface: {}", get_egl_error_string(EGL_sys::eglGetError()));
                    }
                    opengl_window.xlib_window.surface = Some(surface);
                }
                opengl_window.xlib_window.surface.unwrap()
            };
            
            //let pass_size = self.passes[pass_id].pass_size;
            self.passes[pass_id].set_ortho_matrix(
                Vec2 {x: view_bounds.min_x, y: view_bounds.min_y},
                Vec2 {x: pix_width / opengl_window.window_geom.dpi_factor, y: pix_height / opengl_window.window_geom.dpi_factor}
            );
            
            unsafe {
                // Make EGL context current
                if EGL_sys::eglMakeCurrent(opengl_cx.display, surface, surface, opengl_cx.context) == EGL_sys::EGL_FALSE {
                    panic!("can't make EGL context current: {}", get_egl_error_string(EGL_sys::eglGetError()));
                }
                gl::Viewport(0, 0, pix_width as i32, pix_height as i32);
            }
            view_rect = Rect {x: view_bounds.min_x, y: view_bounds.min_y, w: view_bounds.max_x - view_bounds.min_x, h: view_bounds.max_y - view_bounds.min_y}
        }
        
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        // set up the
        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            Color::zero()
        }
        else {
            match self.passes[pass_id].color_textures[0].clear_color {
                ClearColor::InitWith(color) => color,
                ClearColor::ClearWith(color) => color
            }
        };
        let clear_depth = match self.passes[pass_id].clear_depth {
            ClearDepth::InitWith(depth) => depth,
            ClearDepth::ClearWith(depth) => depth
        };
        
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
            gl::ClearDepth(clear_depth);
            gl::ClearColor(clear_color.r, clear_color.g, clear_color.b, clear_color.a);
            gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
        }
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(pass_id, view_id, full_repaint, &view_rect, &opengl_cx, &mut zbias, zbias_step);
        
        unsafe {
            EGL_sys::eglSwapBuffers(opengl_cx.display, surface);
        }
        return init_repaint;
    }
    
    pub fn draw_pass_to_texture(
        &mut self,
        pass_id: usize,
        inherit_dpi_factor: f32,
        opengl_cx: &OpenglCx,
    ) {
        
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        self.passes[pass_id].paint_dirty = false;
        
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        let mut clear_color = Color::zero();
        let mut clear_depth = 1.0;
        let mut clear_flags = 0;
        
        // make a framebuffer
        if self.passes[pass_id].platform.gl_framebuffer.is_none() {
            unsafe {
                let mut gl_framebuffer = mem::uninitialized();
                gl::GenFramebuffers(1, &mut gl_framebuffer);
                self.passes[pass_id].platform.gl_framebuffer = Some(gl_framebuffer);
            }
        }
        
        // bind the framebuffer
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.passes[pass_id].platform.gl_framebuffer.unwrap());
        }
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            match color_texture.clear_color {
                ClearColor::InitWith(color) => {
                    if opengl_cx.update_platform_render_target(&mut self.textures[color_texture.texture_id], dpi_factor, pass_size, false) {
                        clear_color = color;
                        clear_flags = gl::COLOR_BUFFER_BIT;
                    }
                },
                ClearColor::ClearWith(color) => {
                    opengl_cx.update_platform_render_target(&mut self.textures[color_texture.texture_id], dpi_factor, pass_size, false);
                    clear_color = color;
                    clear_flags = gl::COLOR_BUFFER_BIT;
                }
            }
            if let Some(gl_texture) = self.textures[color_texture.texture_id].platform.gl_texture {
                unsafe {
                    gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0 + index as u32, gl::TEXTURE_2D, gl_texture, 0);
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            match self.passes[pass_id].clear_depth {
                ClearDepth::InitWith(depth_clear) => {
                    if opengl_cx.update_platform_render_target(&mut self.textures[depth_texture_id], dpi_factor, pass_size, true) {
                        clear_depth = depth_clear;
                        clear_flags = gl::DEPTH_BUFFER_BIT;
                    }
                },
                ClearDepth::ClearWith(depth_clear) => {
                    opengl_cx.update_platform_render_target(&mut self.textures[depth_texture_id], dpi_factor, pass_size, true);
                    clear_depth = depth_clear;
                    clear_flags = gl::COLOR_BUFFER_BIT;
                }
            }
            if let Some(gl_texture) = self.textures[depth_texture_id].platform.gl_texture {
                unsafe {
                    gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::DEPTH_STENCIL_ATTACHMENT, gl::TEXTURE_2D, gl_texture, 0);
                }
            }
        }
        unsafe {
            gl::Viewport(0, 0, (pass_size.x * dpi_factor) as i32, (pass_size.y * dpi_factor) as i32);
        }
        if clear_flags != 0 {
            unsafe {
                gl::ClearDepth(clear_depth);
                gl::ClearColor(clear_color.r, clear_color.g, clear_color.b, clear_color.a);
                gl::Clear(clear_flags);
            }
        }
        
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        self.render_view(pass_id, view_id, true, &Rect::zero(), &opengl_cx, &mut zbias, zbias_step);
        
    }
    
    //let view_id = self.passes[pass_id].main_view_id.unwrap();
    //let _pass_size = self.passes[pass_id].pass_size;
    
    /*
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {

            let cxtexture = &mut self.textures[color_texture.texture_id];

            metal_cx.update_platform_render_target(cxtexture, dpi_factor, pass_size, false);
            let color_attachment = render_pass_descriptor.color_attachments().object_at(index).unwrap();
            if let Some(mtltex) = &cxtexture.platform.mtltexture {
                color_attachment.set_texture(Some(&mtltex));
            }
            else {
                println!("draw_pass_to_texture invalid render target");
            }
            color_attachment.set_store_action(MTLStoreAction::Store);
            if let Some(color) = color_texture.clear_color {
                color_attachment.set_load_action(MTLLoadAction::Clear);
                color_attachment.set_clear_color(MTLClearColor::new(color.r as f64, color.g as f64, color.b as f64, color.a as f64));
            }
            else {
                color_attachment.set_load_action(MTLLoadAction::Load);
            }
        }
        */
    //self.render_view(pass_id, view_id, true, &Rect::zero(), &opengl_cx);
    // commit
    //}
    
    pub fn opengl_compile_all_shaders(&mut self, opengl_cx: &OpenglCx) {
        unsafe {
            // Make EGL context current
            if EGL_sys::eglMakeCurrent(opengl_cx.display, opengl_cx.surface, opengl_cx.surface, opengl_cx.context) == EGL_sys::EGL_FALSE {
                panic!("can't make EGL context current: {}", get_egl_error_string(EGL_sys::eglGetError()));
            }
        }
        for sh in &mut self.shaders {
            let openglsh = Self::opengl_compile_shader(sh, opengl_cx);
            if let Err(err) = openglsh {
                panic!("Got opengl shader compile error:: {}", err.msg);
            }
        };
    }
    
    pub fn opengl_has_shader_error(compile: bool, shader: usize, source: &str) -> Option<String> {
        //None
        unsafe {
            
            let mut success = i32::from(gl::FALSE);
            
            if compile {
                gl::GetShaderiv(shader as u32, gl::COMPILE_STATUS, &mut success);
            }
            else {
                gl::GetProgramiv(shader as u32, gl::LINK_STATUS, &mut success);
            };

            if success != i32::from(gl::TRUE) {
                let mut length = 0;
                if compile {
                    gl::GetShaderiv(shader as u32, gl::INFO_LOG_LENGTH, &mut length);
                } else {
                    gl::GetProgramiv(shader as u32, gl::INFO_LOG_LENGTH, &mut length);
                }
                let mut log = Vec::with_capacity(length as usize);
                if compile {
                    gl::GetShaderInfoLog(shader as u32, length, ptr::null_mut(), log.as_mut_ptr());
                } else {
                    gl::GetProgramInfoLog(shader as u32, length, ptr::null_mut(), log.as_mut_ptr());
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
                Some(r)
            }
            else {
                None
            }
        }
    }
    
    pub fn opengl_get_attributes(program: u32, prefix: &str, slots: usize) -> Vec<OpenglAttribute> {
        let mut attribs = Vec::new();
        
        let stride = (slots * mem::size_of::<f32>()) as i32;
        let num_attr = Self::ceil_div4(slots);
        for i in 0..num_attr {
            let mut name = prefix.to_string();
            name.push_str(&i.to_string());
            name.push_str("\0");
            
            let mut size = ((slots - i * 4)) as i32;
            if size > 4 {
                size = 4;
            }
            unsafe {
                attribs.push(
                    OpenglAttribute {
                        loc: gl::GetAttribLocation(program, name.as_ptr() as *const _) as u32,
                        offset: (i * 4 * mem::size_of::<f32>()) as usize,
                        size: size,
                        stride: stride
                    }
                )
            }
        }
        attribs
    }
    
    pub fn opengl_get_uniforms(program: u32, sg: &ShaderGen, unis: &Vec<ShVar>) -> Vec<OpenglUniform> {
        let mut gl_uni = Vec::new();
        
        for uni in unis {
            let mut name0 = "".to_string();
            name0.push_str(&uni.name);
            name0.push_str("\0");
            unsafe {
                gl_uni.push(OpenglUniform {
                    loc: gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name: uni.name.clone(),
                    size: sg.get_type_slots(&uni.ty)
                })
            }
        }
        gl_uni
    }
    
    pub fn opengl_get_texture_slots(program: u32, texture_slots: &Vec<ShVar>) -> Vec<OpenglUniform> {
        let mut gl_texture_slots = Vec::new();
        
        for slot in texture_slots {
            let mut name0 = "".to_string();
            name0.push_str(&slot.name);
            name0.push_str("\0");
            unsafe {
                gl_texture_slots.push(OpenglUniform {
                    loc: gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name: slot.name.clone(),
                    size: 0
                    //,sampler:sam.sampler.clone()
                })
            }
        }
        gl_texture_slots
    }
    
    pub fn opengl_compile_shader(sh: &mut CxShader, opengl_cx: &OpenglCx) -> Result<(), SlErr> {
        
        let (vertex, fragment, mapping) = Self::gl_assemble_shader(&sh.shader_gen, GLShaderType::OpenGL) ?;
        // now we have a pixel and a vertex shader
        // so lets now pass it to GL
        unsafe {
            let vs = gl::CreateShader(gl::VERTEX_SHADER);
            gl::ShaderSource(vs, 1, [vertex.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(vs);
            if let Some(error) = Self::opengl_has_shader_error(true, vs as usize, &vertex) {
                return Err(SlErr {
                    msg: format!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}", error)
                })
            }
            
            let fs = gl::CreateShader(gl::FRAGMENT_SHADER);
            gl::ShaderSource(fs, 1, [fragment.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(fs);
            if let Some(error) = Self::opengl_has_shader_error(true, fs as usize, &fragment) {
                return Err(SlErr {
                    msg: format!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}", error)
                })
            }
            
            let program = gl::CreateProgram();
            gl::AttachShader(program, vs);
            gl::AttachShader(program, fs);
            gl::LinkProgram(program);
            if let Some(error) = Self::opengl_has_shader_error(false, program as usize, "") {
                return Err(SlErr {
                    msg: format!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}", error)
                })
            }
            gl::DeleteShader(vs);
            gl::DeleteShader(fs);
            
            let geom_attribs = Self::opengl_get_attributes(program, "geomattr", mapping.geometry_slots);
            let inst_attribs = Self::opengl_get_attributes(program, "instattr", mapping.instance_slots);
            
            // lets fetch the uniform positions for our uniforms
            sh.platform = Some(CxPlatformShader {
                program: program,
                geom_ibuf: {
                    let mut buf = OpenglBuffer::default();
                    buf.update_with_u32_data(opengl_cx, &sh.shader_gen.geometry_indices);
                    buf
                },
                geom_vbuf: {
                    let mut buf = OpenglBuffer::default();
                    buf.update_with_f32_data(opengl_cx, &sh.shader_gen.geometry_vertices);
                    buf
                },
                geom_attribs,
                inst_attribs,
                uniforms_cx: Self::opengl_get_uniforms(program, &sh.shader_gen, &mapping.uniforms_cx),
                uniforms_vw: Self::opengl_get_uniforms(program, &sh.shader_gen, &mapping.uniforms_vw),
                uniforms_dr: Self::opengl_get_uniforms(program, &sh.shader_gen, &mapping.uniforms_dr),
            });
            sh.mapping = mapping;
            return Ok(());
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ViewBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32
}

impl ViewBounds {
    fn new() -> ViewBounds {
        ViewBounds {
            min_x: std::f32::INFINITY,
            min_y: std::f32::INFINITY,
            max_x: std::f32::NEG_INFINITY,
            max_y: std::f32::NEG_INFINITY,
        }
    }
    
    fn add_rect(&mut self, rect: &Rect) {
        if rect.x < self.min_x {
            self.min_x = rect.x;
        }
        if rect.x + rect.w > self.max_x {
            self.max_x = rect.x + rect.w;
        }
        if rect.y < self.min_y {
            self.min_y = rect.y;
        }
        if rect.y + rect.h > self.max_y {
            self.max_y = rect.y + rect.h;
        }
    }
}

pub struct OpenglCx {
    pub display: EGL_sys::EGLDisplay,
    pub config: EGL_sys::EGLConfig,
    pub context: EGL_sys::EGLContext,
    pub surface: EGL_sys::EGLSurface,
}

impl OpenglCx {
    pub fn new() -> OpenglCx {
        // Load the gl function pointers
        gl::load_with( | symbol | {
            unsafe {
                EGL_sys::eglGetProcAddress(CString::new(symbol).unwrap().as_ptr()).map_or(ptr::null(), |ptr| ptr as *const _)
            }
        });
        
        // start a cx
        // The default screen of the display
        unsafe {
            // Open EGL display connection
            let display = EGL_sys::eglGetDisplay(ptr::null_mut());
            if display.is_null() {
                panic!("can't open EGL display connection");
            }

            // Initialize EGL display connection
            if EGL_sys::eglInitialize(display, ptr::null_mut(), ptr::null_mut()) == EGL_sys::EGL_FALSE {
                panic!("can't initialize EGL display connection:: {}", get_egl_error_string(EGL_sys::eglGetError()));
            }

            // Choose EGL config
            let attribs = [
                EGL_sys::EGL_RENDERABLE_TYPE as i32,
                EGL_sys::EGL_OPENGL_ES3_BIT as i32,
                EGL_sys::EGL_SURFACE_TYPE as i32,
                EGL_sys::EGL_PBUFFER_BIT as i32 | EGL_sys::EGL_WINDOW_BIT as i32,
                EGL_sys::EGL_RED_SIZE as i32,
                8,
                EGL_sys::EGL_GREEN_SIZE as i32,
                8,
                EGL_sys::EGL_BLUE_SIZE as i32,
                8,
                EGL_sys::EGL_DEPTH_SIZE as i32,
                24,
                EGL_sys::EGL_STENCIL_SIZE as i32,
                8,
                EGL_sys::EGL_NONE as i32,
            ];
            let mut capacity = 0;
            if EGL_sys::eglChooseConfig(display, attribs.as_ptr(), ptr::null_mut(), 0, &mut capacity) == EGL_sys::EGL_FALSE {
                panic!("can't choose EGL config: {}", get_egl_error_string(EGL_sys::eglGetError()));
            }
            let mut configs = Vec::with_capacity(capacity as usize);
            let mut len = 0;
            if EGL_sys::eglChooseConfig(display, attribs.as_ptr(), configs.as_mut_ptr(), capacity, &mut len) == EGL_sys::EGL_FALSE {
                panic!("can't choose EGL config: {}", get_egl_error_string(EGL_sys::eglGetError()));
            }
            configs.set_len(len as usize);
            if configs.is_empty() {
                panic!("can't choose EGL config");
            }
            let config = configs[0];
            
            // Create a gl context
            let attribs = [
                EGL_sys::EGL_CONTEXT_CLIENT_VERSION as i32,
                3,
                EGL_sys::EGL_NONE as i32,
            ];
            let context = EGL_sys::eglCreateContext(display, config, ptr::null_mut(), attribs.as_ptr());
            if context.is_null() {
                panic!("can't create EGL context: {}", get_egl_error_string(EGL_sys::eglGetError()));
            }

            // Create EGL pbuffer surface
            let attribs = [
                EGL_sys::EGL_WIDTH as i32,
                16,
                EGL_sys::EGL_HEIGHT as i32,
                16,
                EGL_sys::EGL_NONE as i32,
            ];
            let surface = EGL_sys::eglCreatePbufferSurface(display, config, attribs.as_ptr());
            if surface.is_null() {
                panic!("can't create EGL pbuffer surface: {}", get_egl_error_string(EGL_sys::eglGetError()));
            }
            
            OpenglCx {
                display,
                config,
                context,
                surface,
            }
        }
    }
    
    pub fn set_uniform_buffer(&self, locs: &Vec<OpenglUniform>, uni: &Vec<f32>) {
        
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
        
    }
    
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
            
            let mut gl_texture;
            match cxtexture.platform.gl_texture {
                None => {
                    unsafe {
                        gl_texture = mem::uninitialized();
                        gl::GenTextures(1, &mut gl_texture);
                        cxtexture.platform.gl_texture = Some(gl_texture);
                    }
                }
                Some(gl_texture_old) => {
                    gl_texture = gl_texture_old
                }
            }
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
            if let Some(gl_texture) = cxtexture.platform.gl_texture {
                gl::DeleteTextures(1, &gl_texture);
            }
            
            let mut gl_texture = mem::uninitialized();
            gl::GenTextures(1, &mut gl_texture);
            gl::BindTexture(gl::TEXTURE_2D, gl_texture);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            
            cxtexture.platform.alloc_desc = cxtexture.desc.clone();
            cxtexture.platform.width = width;
            cxtexture.platform.height = height;
            cxtexture.platform.gl_texture = Some(gl_texture);
            
            if !is_depth {
                match cxtexture.desc.format {
                    TextureFormat::Default | TextureFormat::RenderBGRA => {
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
                        println!("Depth stencil texture!");
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
pub struct CxPlatformShader {
    pub program: u32,
    pub geom_vbuf: OpenglBuffer,
    pub geom_ibuf: OpenglBuffer,
    pub geom_attribs: Vec<OpenglAttribute>,
    pub inst_attribs: Vec<OpenglAttribute>,
    pub uniforms_cx: Vec<OpenglUniform>,
    pub uniforms_vw: Vec<OpenglUniform>,
    pub uniforms_dr: Vec<OpenglUniform>,
}


#[derive(Clone)]
pub struct OpenglWindow {
    pub first_draw:bool,
    pub window_id: usize,
    pub window_geom: WindowGeom,
    pub opening_repaint_count: u32,
    pub cal_size: Vec2,
    pub xlib_window: XlibWindow,
}

impl OpenglWindow {
    pub fn new(window_id: usize, opengl_cx: &OpenglCx, xlib_app: &mut XlibApp, inner_size: Vec2, position: Option<Vec2>, title: &str) -> OpenglWindow {
        
        let mut xlib_window = XlibWindow::new(xlib_app, window_id);
       
        // Get native visual id
        let visual_id = unsafe {
            let mut visual_id = 0;
            if EGL_sys::eglGetConfigAttrib(opengl_cx.display, opengl_cx.config, EGL_sys::EGL_NATIVE_VISUAL_ID as i32, &mut visual_id) == EGL_sys::EGL_FALSE {
                panic!("can't get native visual id");
            }
            visual_id
        };

        xlib_window.init(title, inner_size, position, visual_id as X11_sys::VisualID);
        
        OpenglWindow {
            first_draw: true,
            window_id,
            opening_repaint_count: 0,
            cal_size: Vec2::zero(),
            window_geom: xlib_window.get_window_geom(),
            xlib_window
        }
    }
    
    pub fn resize_framebuffer(&mut self, _opengl_cx: &OpenglCx) -> bool {
        let cal_size = Vec2 {
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

#[derive(Default, Clone)]
pub struct OpenglUniform {
    pub loc: i32,
    pub name: String,
    pub size: usize
}
/*
#[derive(Default, Clone)]
pub struct OpenglTextureSlot {
    pub loc: isize,
    pub name: String
}
*/
#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformDrawCall {
    pub inst_vbuf: OpenglBuffer,
    pub vao_shader_id: Option<usize>,
    pub vao: Option<u32>
}

impl CxPlatformDrawCall {
    
    pub fn check_vao(&mut self, shader_id: usize, shp: &CxPlatformShader) {
        if self.vao_shader_id.is_none() || self.vao_shader_id.unwrap() != shader_id {
            self.free_vao();
            // create the VAO
            unsafe {
                let mut vao = mem::uninitialized();
                gl::GenVertexArrays(1, &mut vao);
                gl::BindVertexArray(vao);
                
                // bind the vertex and indexbuffers
                gl::BindBuffer(gl::ARRAY_BUFFER, shp.geom_vbuf.gl_buffer.unwrap());
                for attr in &shp.geom_attribs {
                    gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                    gl::EnableVertexAttribArray(attr.loc);
                }
                
                gl::BindBuffer(gl::ARRAY_BUFFER, self.inst_vbuf.gl_buffer.unwrap());
                
                for attr in &shp.inst_attribs {
                    gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                    gl::EnableVertexAttribArray(attr.loc);
                    gl::VertexAttribDivisor(attr.loc, 1 as gl::types::GLuint);
                }
                
                // bind the indexbuffer
                gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, shp.geom_ibuf.gl_buffer.unwrap());
                gl::BindVertexArray(0);
                
                self.vao_shader_id = Some(shader_id);
                self.vao = Some(vao);
            }
        }
    }
    
    fn free_vao(&mut self) {
        unsafe {
            if let Some(mut vao) = self.vao {
                gl::DeleteVertexArrays(1, &mut vao);
                self.vao = None;
            }
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub gl_texture: Option<u32>,
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
    pub gl_framebuffer: Option<u32>
}

#[derive(Default, Clone, Debug)]
pub struct OpenglBuffer {
    pub gl_buffer: Option<u32>
}

impl OpenglBuffer {
    
    pub fn alloc_gl_buffer(&mut self) {
        unsafe {
            let mut gl_buffer = mem::uninitialized();
            gl::GenBuffers(1, &mut gl_buffer);
            self.gl_buffer = Some(gl_buffer);
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
