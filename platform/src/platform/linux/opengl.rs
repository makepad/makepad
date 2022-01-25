use crate::cx::*;
use crate::cx_xlib::*;
use makepad_glx_sys as glx_sys;
use makepad_x11_sys as X11_sys;
use std::ffi::{CStr, CString};
use std::os::raw::{c_ulong, c_void};
use std::ptr;
use std::mem;
use makepad_live_compiler::generate_glsl;
use makepad_live_compiler::analyse::ShaderCompileOptions;
use makepad_live_compiler::shaderast::ShaderAst;

impl Cx {
    
    pub fn render_view(
        &mut self,
        pass_id: usize,
        view_id: usize,
        scroll: Vec2,
        clip: (Vec2, Vec2),
        full_repaint: bool,
        view_rect: &Rect,
        opengl_cx: &OpenglCx,
        zbias: &mut f32,
        zbias_step: f32
    ) {
        
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        if !full_repaint && !view_rect.intersects(self.views[view_id].get_scrolled_rect()) {
            return
        }
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        self.views[view_id].parent_scroll = scroll;
        let local_scroll = self.views[view_id].get_local_scroll();
        let clip = self.views[view_id].intersect_clip(clip);
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(
                    pass_id,
                    sub_view_id,
                    Vec2 {x: local_scroll.x + scroll.x, y: local_scroll.y + scroll.y},
                    clip,
                    full_repaint,
                    view_rect,
                    opengl_cx,
                    zbias,
                    zbias_step
                );
            }
            else {
                let cxview = &mut self.views[view_id];
                //view.platform.uni_vw.update_with_f32_data(device, &view.uniforms);
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    draw_call.platform.inst_vb.update_with_f32_data(opengl_cx, &draw_call.instances);
                }
                
                 let geometry_id = if let Some(geometry) = draw_call.geometry{
                    geometry.geometry_id
                }
                else if let Some(geometry) = sh.default_geometry{
                    geometry.geometry_id
                }
                else{
                    continue
                };
                
                let geometry = &mut self.geometries[geometry_id];
                let indices = geometry.indices.len();
                
                draw_call.set_zbias(*zbias);
                draw_call.set_local_scroll(scroll, local_scroll);
                draw_call.set_clip(clip);
                *zbias += zbias_step;
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                }
                
                // update geometry?
                let geometry = &mut self.geometries[geometry_id];
                if geometry.dirty || geometry.platform.vb.gl_buffer.is_none() || geometry.platform.ib.gl_buffer.is_none() {
                    geometry.platform.vb.update_with_f32_data(opengl_cx, &geometry.vertices);
                    geometry.platform.ib.update_with_u32_data(opengl_cx, &geometry.indices);
                    geometry.dirty = false;
                }
                
                // lets check if our vao is still valid
                if draw_call.platform.vao.is_none() {
                    draw_call.platform.vao = Some(CxPlatformDrawCallVao {
                        vao: unsafe {
                            let mut vao = std::mem::MaybeUninit::uninit();
                            gl::GenVertexArrays(1, vao.as_mut_ptr());
                            vao.assume_init()
                        },
                        shader_id: None,
                        inst_vb: None,
                        geom_vb: None,
                        geom_ib: None,
                    });
                }
                
                let vao = draw_call.platform.vao.as_mut().unwrap();
                if vao.inst_vb != draw_call.platform.inst_vb.gl_buffer
                    || vao.geom_vb != geometry.platform.vb.gl_buffer
                    || vao.geom_ib != geometry.platform.ib.gl_buffer
                    || vao.shader_id != Some(draw_call.shader.shader_id) {
                    vao.shader_id = Some(draw_call.shader.shader_id);
                    vao.inst_vb = draw_call.platform.inst_vb.gl_buffer;
                    vao.geom_vb = geometry.platform.vb.gl_buffer;
                    vao.geom_ib = geometry.platform.ib.gl_buffer;
                    
                    unsafe {
                        gl::BindVertexArray(vao.vao);
                        
                        // bind the vertex and indexbuffers
                        gl::BindBuffer(gl::ARRAY_BUFFER, vao.geom_vb.unwrap());
                        for attr in &shp.geometries {
                            gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                            gl::EnableVertexAttribArray(attr.loc);
                        }
                        
                        gl::BindBuffer(gl::ARRAY_BUFFER, vao.inst_vb.unwrap());
                        
                        for attr in &shp.instances {
                            gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                            gl::EnableVertexAttribArray(attr.loc);
                            gl::VertexAttribDivisor(attr.loc, 1 as gl::types::GLuint);
                        }
                        
                        // bind the indexbuffer
                        gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, vao.geom_ib.unwrap());
                        gl::BindVertexArray(0);
                    }
                }
                
                unsafe {
                    gl::UseProgram(shp.program);
                    gl::BindVertexArray(draw_call.platform.vao.as_ref().unwrap().vao);
                    let instances = draw_call.instances.len() / sh.mapping.instance_props.total_slots;
                    
                    let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                    let view_uniforms = cxview.view_uniforms.as_slice();
                    let draw_uniforms = draw_call.draw_uniforms.as_slice();
                    
                    opengl_cx.set_uniform_buffer(&shp.pass_uniforms, pass_uniforms);
                    opengl_cx.set_uniform_buffer(&shp.view_uniforms, view_uniforms);
                    opengl_cx.set_uniform_buffer(&shp.draw_uniforms, draw_uniforms);
                    opengl_cx.set_uniform_buffer(&shp.user_uniforms, &draw_call.user_uniforms);
                    opengl_cx.set_uniform_buffer(&shp.live_uniforms, &sh.mapping.live_uniforms_buf);
                    
                    if let Some(ct) = &sh.mapping.const_table {
                        opengl_cx.set_uniform_array(&shp.const_table_uniform, ct);
                    }
                    
                    // lets set our textures
                    for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                        let cxtexture = &mut self.textures[*texture_id as usize];
                        if cxtexture.update_image {
                            cxtexture.update_image = false;
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
                    
                    gl::BindVertexArray(0);
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
                    view_bounds.add_rect(&cxview.get_inverse_scrolled_rect());
                }
            }
        }
    }
    
    pub fn set_default_depth_and_blend_mode() {
        unsafe {
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
        _force_full_repaint: bool,
    ) -> bool {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        let mut view_bounds = ViewBounds::new();
        let mut init_repaint = false;
        self.calc_dirty_bounds(pass_id, view_id, &mut view_bounds);
        
        let full_repaint = true;/*force_full_repaint || view_bounds.max_x - view_bounds.min_x > opengl_window.window_geom.inner_size.x - 100.
         && view_bounds.max_y - view_bounds.min_y > opengl_window.window_geom.inner_size.y - 100. ||
         opengl_window.opening_repaint_count < 10;*/
        if opengl_window.opening_repaint_count < 10 { // for some reason the first repaint doesn't arrive on the window
            opengl_window.opening_repaint_count += 1;
            init_repaint = true;
        }
        let window;
        let view_rect;
        
        opengl_window.xlib_window.hide_child_windows();
        
        window = opengl_window.xlib_window.window.unwrap();
        
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_matrix(Vec2::default(), pass_size);
        
        let pix_width = opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor;
        let pix_height = opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor;
        
        unsafe {
            glx_sys::glXMakeCurrent(opengl_cx.display, window, opengl_cx.context);
            gl::Viewport(0, 0, pix_width as i32, pix_height as i32);
        }
        view_rect = Rect::default();
        
        //self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        // set up the
        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            Vec4::default()
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
            gl::ClearColor(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
            gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
        }
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            full_repaint,
            &view_rect,
            &opengl_cx,
            &mut zbias,
            zbias_step
        );
        
        unsafe {
            glx_sys::glXSwapBuffers(opengl_cx.display, window);
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
        self.passes[pass_id].set_matrix(Vec2::default(), pass_size);
        self.passes[pass_id].paint_dirty = false;
        
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        let mut clear_color = Vec4::default();
        let mut clear_depth = 1.0;
        let mut clear_flags = 0;
        
        // make a framebuffer
        if self.passes[pass_id].platform.gl_framebuffer.is_none() {
            unsafe {
                let mut gl_framebuffer = std::mem::MaybeUninit::uninit();
                gl::GenFramebuffers(1, gl_framebuffer.as_mut_ptr());
                self.passes[pass_id].platform.gl_framebuffer = Some(gl_framebuffer.assume_init());
            }
        }
        
        // bind the framebuffer
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.passes[pass_id].platform.gl_framebuffer.unwrap());
        }
        
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            // ok lets do ugly shit here.
            match self.passes[pass_id].clear_depth {
                ClearDepth::InitWith(depth_clear) => {
                    if opengl_cx.update_platform_render_target(&mut self.textures[depth_texture_id as usize], dpi_factor, pass_size, true) {
                        clear_depth = depth_clear;
                        clear_flags |= gl::DEPTH_BUFFER_BIT;
                    }
                },
                ClearDepth::ClearWith(depth_clear) => {
                    opengl_cx.update_platform_render_target(&mut self.textures[depth_texture_id as usize], dpi_factor, pass_size, true);
                    clear_depth = depth_clear;
                    clear_flags |= gl::DEPTH_BUFFER_BIT;
                }
            }
            if let Some(gl_renderbuffer) = self.textures[depth_texture_id as usize].platform.gl_renderbuffer {
                unsafe {
                    gl::FramebufferRenderbuffer( gl::FRAMEBUFFER, gl::DEPTH_ATTACHMENT, gl::RENDERBUFFER, gl_renderbuffer );
                }
            }            
        }
        else{
            unsafe{ // BUGFIX. we have to create a depthbuffer for rtt without depthbuffer use otherwise it fails if there is another pass with depth
                if self.passes[pass_id].platform.gl_bugfix_depthbuffer.is_none(){
                    let mut gl_renderbuf = std::mem::MaybeUninit::uninit();
                    gl::GenRenderbuffers(1, gl_renderbuf.as_mut_ptr());
                    let gl_renderbuffer = gl_renderbuf.assume_init();
                    gl::BindRenderbuffer(gl::RENDERBUFFER, gl_renderbuffer);
                    gl::RenderbufferStorage(
                        gl::RENDERBUFFER, 
                        gl::DEPTH_COMPONENT16,
                        (pass_size.x * dpi_factor) as i32, (pass_size.y * dpi_factor) as i32
                    );
                    gl::BindRenderbuffer(gl::RENDERBUFFER, 0);
                    self.passes[pass_id].platform.gl_bugfix_depthbuffer = Some(gl_renderbuffer);
                }
                clear_depth = 1.0;
                clear_flags |= gl::DEPTH_BUFFER_BIT;
                gl::Disable(gl::DEPTH_TEST);
                gl::FramebufferRenderbuffer( gl::FRAMEBUFFER, gl::DEPTH_ATTACHMENT, gl::RENDERBUFFER, self.passes[pass_id].platform.gl_bugfix_depthbuffer.unwrap() );
            }
        }
        
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            match color_texture.clear_color {
                ClearColor::InitWith(color) => {
                    if opengl_cx.update_platform_render_target(&mut self.textures[color_texture.texture_id as usize], dpi_factor, pass_size, false) {
                        clear_color = color;
                        clear_flags |= gl::COLOR_BUFFER_BIT;
                    }
                },
                ClearColor::ClearWith(color) => {
                    opengl_cx.update_platform_render_target(&mut self.textures[color_texture.texture_id as usize], dpi_factor, pass_size, false);
                    clear_color = color;
                    clear_flags |= gl::COLOR_BUFFER_BIT;
                }
            }
            if let Some(gl_texture) = self.textures[color_texture.texture_id as usize].platform.gl_texture {
                unsafe {
                    gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0 + index as u32, gl::TEXTURE_2D, gl_texture, 0);
                }
            }
        }
        
        unsafe {
            gl::Viewport(0, 0, (pass_size.x * dpi_factor) as i32, (pass_size.y * dpi_factor) as i32);
        }
        if clear_flags != 0 {
            unsafe {
                gl::ClearDepth(clear_depth);
                gl::ClearColor(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                gl::Clear(clear_flags);
            }
        }
        
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            true,
            &Rect::default(),
            &opengl_cx,
            &mut zbias,
            zbias_step
        );
        unsafe{
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        }
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
    
    
    
    pub fn opengl_get_info_log(compile: bool, shader: usize, source: &str) -> String {
        unsafe {
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
            r
        }
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
                            let loc = gl::GetAttribLocation(program, name0.as_ptr() as *const _) as u32;
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
    
    pub fn opengl_get_uniforms(program: u32, unis: &Vec<PropDef>) -> Vec<OpenglUniform> {
        let mut gl_uni = Vec::new();
        for uni in unis {
            gl_uni.push(
                Self::opengl_get_uniform(program, &uni.name, uni.ty.size())
            );
        }
        gl_uni
    }
    
    pub fn opengl_get_uniform(program: u32, name: &str, size: usize) -> OpenglUniform {
        let mut name0 = String::new();
        name0.push_str(name);
        name0.push_str("\0");
        unsafe {
            OpenglUniform {
                loc: gl::GetUniformLocation(program, name0.as_ptr() as *const _),
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
                    loc: gl::GetUniformLocation(program, name0.as_ptr() as *const _),
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
            sh.platform = Some(CxPlatformShader {
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
        if rect.pos.x < self.min_x {
            self.min_x = rect.pos.x;
        }
        if rect.pos.x + rect.size.x > self.max_x {
            self.max_x = rect.pos.x + rect.size.x;
        }
        if rect.pos.y < self.min_y {
            self.min_y = rect.pos.y;
        }
        if rect.pos.y + rect.size.y > self.max_y {
            self.max_y = rect.pos.y + rect.size.y;
        }
    }
}
pub struct OpenglCx {
    pub display: *mut glx_sys::Display, 
    pub context: glx_sys::GLXContext,
    pub visual_info: glx_sys::XVisualInfo,
    pub hidden_window: glx_sys::Window,
}

impl OpenglCx {
    pub fn new(display: *mut X11_sys::Display) -> OpenglCx {
        unsafe {
            let display = display as *mut glx_sys::Display;
            
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
pub struct CxPlatformShader {
    pub program: u32,
    pub vertex: String,
    pub fragment: String,
    pub geometries: Vec<OpenglAttribute>,
    pub instances: Vec<OpenglAttribute>,
    pub pass_uniforms: Vec<OpenglUniform>,
    pub view_uniforms: Vec<OpenglUniform>,
    pub draw_uniforms: Vec<OpenglUniform>,
    pub user_uniforms: Vec<OpenglUniform>,
    pub live_uniforms: Vec<OpenglUniform>,
    pub const_table_uniform: OpenglUniform,
}


#[derive(Clone)]
pub struct OpenglWindow {
    pub first_draw: bool,
    pub window_id: usize,
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

#[derive(Debug, Default, Clone)]
pub struct OpenglUniform {
    pub loc: i32,
    pub name: String,
    pub size: usize
}


#[derive(Clone, Default)]
pub struct CxPlatformGeometry {
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
pub struct CxPlatformView {
}

#[derive(Default, Clone)]
pub struct CxPlatformDrawCallVao {
    pub vao: u32,
    pub shader_id: Option<usize>,
    pub inst_vb: Option<u32>,
    pub geom_vb: Option<u32>,
    pub geom_ib: Option<u32>,
}


#[derive(Default, Clone)]
pub struct CxPlatformDrawCall {
    pub inst_vb: OpenglBuffer,
    pub vao: Option<CxPlatformDrawCallVao>,
}

impl CxPlatformDrawCall {
}

#[derive(Default, Clone)]
pub struct CxPlatformTexture {
    pub alloc_desc: TextureDesc,
    pub width: u64,
    pub height: u64,
    pub gl_texture: Option<u32>,
    pub gl_renderbuffer: Option<u32>
}

#[derive(Default, Clone)]
pub struct CxPlatformPass {
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
