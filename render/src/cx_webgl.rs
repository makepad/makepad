
use crate::cx::*;
use makepad_shader_compiler::generate_glsl;

impl Cx {
    pub fn render_view(
        &mut self,
        pass_id: usize,
        view_id: usize,
        scroll: Vec2,
        clip: (Vec2, Vec2),
        vr_is_presenting: bool,
        zbias: &mut f32,
        zbias_step: f32
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        if vr_is_presenting {
            self.views[view_id].uniform_view_transform(&Mat4::scale_translate(0.0005, -0.0005, 0.001, -0.25, 0., -0.5));
        }
        else {
            self.views[view_id].uniform_view_transform(&Mat4::identity());
        }
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
                    vr_is_presenting,
                    zbias,
                    zbias_step
                );
            }
            else {
                let cxview = &mut self.views[view_id];
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    // update the instance buffer data
                    draw_call.platform.check_attached_vao(draw_call.shader_id, sh, &mut self.platform);
                    
                    self.platform.from_wasm.alloc_array_buffer(
                        draw_call.platform.inst_vb_id,
                        draw_call.instance.len(),
                        draw_call.instance.as_ptr() as *const f32
                    );
                }
                
                draw_call.set_zbias(*zbias);
                draw_call.set_local_scroll(scroll, local_scroll);
                draw_call.set_clip(clip);
                *zbias += zbias_step;
                
                // update/alloc textures?
                for texture_id in &draw_call.textures_2d {
                    let cxtexture = &mut self.textures[*texture_id as usize];
                    if cxtexture.update_image {
                        cxtexture.update_image = false;
                        self.platform.from_wasm.update_texture_image2d(*texture_id as usize, cxtexture);
                        //Self::update_platform_texture_image2d(&mut self.platform);
                    }
                }
                
                let pass_uniforms = self.passes[pass_id].pass_uniforms.as_slice();
                let view_uniforms = cxview.view_uniforms.as_slice();
                let draw_uniforms = draw_call.draw_uniforms.as_slice();
                
                self.platform.from_wasm.draw_call(
                    draw_call.shader_id,
                    draw_call.platform.vao_id,
                    pass_uniforms,
                    view_uniforms,
                    draw_uniforms,
                    &draw_call.uniforms,
                    &draw_call.textures_2d,
                    &sh.mapping.const_table
                );
            }
        }
    }
    
    pub fn setup_render_pass(&mut self, pass_id: usize, inherit_dpi_factor: f32) {
        let pass_size = self.passes[pass_id].pass_size;
        self.passes[pass_id].set_ortho_matrix(Vec2::default(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());
        self.passes[pass_id].paint_dirty = false;
        
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
    }
    
    pub fn draw_pass_to_canvas(&mut self, pass_id: usize, vr_is_presenting: bool, dpi_factor: f32) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        // get the color and depth
        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            Color::default()
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
        self.platform.from_wasm.begin_main_canvas(clear_color, clear_depth as f32);
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        self.platform.from_wasm.set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.platform.from_wasm.mark_begin_canvas_render();
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            vr_is_presenting,
            &mut zbias,
            zbias_step
        );
        self.platform.from_wasm.mark_end_canvas_render();
    }
    
    pub fn draw_pass_to_texture(&mut self, pass_id: usize, dpi_factor: f32) {
        let pass_size = self.passes[pass_id].pass_size;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        self.platform.from_wasm.begin_render_targets(pass_id, (pass_size.x * dpi_factor) as usize, (pass_size.y * dpi_factor) as usize);
        
        for color_texture in &self.passes[pass_id].color_textures {
            match color_texture.clear_color {
                ClearColor::InitWith(color) => {
                    self.platform.from_wasm.add_color_target(color_texture.texture_id, true, color);
                },
                ClearColor::ClearWith(color) => {
                    self.platform.from_wasm.add_color_target(color_texture.texture_id, false, color);
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            match self.passes[pass_id].clear_depth {
                ClearDepth::InitWith(depth_clear) => {
                    self.platform.from_wasm.set_depth_target(depth_texture_id, true, depth_clear as f32);
                },
                ClearDepth::ClearWith(depth_clear) => {
                    self.platform.from_wasm.set_depth_target(depth_texture_id, false, depth_clear as f32);
                }
            }
        }
        
        self.platform.from_wasm.end_render_targets();
        
        // set the default depth and blendmode
        self.platform.from_wasm.set_default_depth_and_blend_mode();
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2{x:-50000.,y:-50000.},Vec2{x:50000.,y:50000.}),
            false,
            &mut zbias,
            zbias_step
        );
    }
    
    
    pub fn webgl_compile_all_shaders(&mut self) {
        for (shader_id, sh) in self.shaders.iter_mut().enumerate() {
            let glsh = Self::webgl_compile_shader(shader_id, false, false, sh, &mut self.platform);
            if let ShaderCompileResult::Fail{err,..} = glsh {
                self.platform.from_wasm.log(&format!("Got GLSL shader compile error: {}", err))
            }
        }
    }
    
    pub fn webgl_compile_shader(shader_id: usize, gather_all_consts:bool, use_const_table: bool, sh: &mut CxShader, platform: &mut CxPlatform) -> ShaderCompileResult{
        
        let shader_ast = sh.shader_gen.lex_parse_analyse(gather_all_consts);
        
        if let Err(err) = shader_ast{
            return ShaderCompileResult::Fail{id:shader_id, err:err}
        } 
        let shader_ast = shader_ast.unwrap();
        let vertex = generate_glsl::generate_vertex_shader(&shader_ast,use_const_table);
        let fragment = generate_glsl::generate_fragment_shader(&shader_ast,use_const_table);
        let mapping = CxShaderMapping::from_shader_gen(&sh.shader_gen, if use_const_table{shader_ast.const_table.borrow_mut().take()} else {None});
    
        let vertex = format!("
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            {}\0", vertex);
        let fragment = format!("
            #extension GL_OES_standard_derivatives : enable
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            {}\0", fragment);

        if shader_ast.debug{
            platform.from_wasm.log(&format!(
                "--------------- Vertex shader {} --------------- \n{}\n---------------\n--------------- Fragment shader {} --------------- \n{}\n---------------\n", shader_id, vertex, shader_id, fragment
            ));
        }

             
        // lets check if we need to recompile the shader at all
        if let Some(sh_platform) = &sh.platform{
            if sh_platform.vertex == vertex && sh_platform.fragment == fragment{
                sh.mapping = mapping;
                return ShaderCompileResult::Nop{id:shader_id}
            }
        } 
        //let shader_id = self.compiled_shaders.len();
        platform.from_wasm.compile_webgl_shader(shader_id, &vertex, &fragment, &mapping);
        
        let geom_ib_id = platform.get_free_index_buffer();
        let geom_vb_id = platform.get_free_index_buffer();
        
        platform.from_wasm.alloc_array_buffer(
            geom_vb_id,
            sh.shader_gen.geometry_vertices.len(),
            sh.shader_gen.geometry_vertices.as_ptr() as *const f32
        );
        
        platform.from_wasm.alloc_index_buffer(
            geom_ib_id,
            sh.shader_gen.geometry_indices.len(),
            sh.shader_gen.geometry_indices.as_ptr() as *const u32
        );
        
        sh.mapping = mapping;
        sh.platform = Some(CxPlatformShader {
            geom_vb_id: geom_vb_id,
            geom_ib_id: geom_ib_id,
            vertex: vertex,
            fragment: fragment
        });
        
        ShaderCompileResult::Ok{id:shader_id}
    }
    
}

#[derive(Default, Clone)]
pub struct CxPlatformPass {
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default, Clone)]
pub struct CxPlatformDrawCall {
    pub resource_shader_id: Option<usize>,
    pub vao_id: usize,
    pub inst_vb_id: usize
}

#[derive(Clone)]
pub struct CxPlatformShader {
    pub vertex: String,
    pub fragment: String,
    pub geom_vb_id: usize,
    pub geom_ib_id: usize,
}

#[derive(Clone, Default)]
pub struct CxPlatformTexture {
}

impl CxPlatformDrawCall {
    
    pub fn check_attached_vao(&mut self, shader_id: usize, sh: &CxShader, platform: &mut CxPlatform) {
        if self.resource_shader_id.is_none() || self.resource_shader_id.unwrap() != shader_id {
            self.free(platform);
            // dont reuse vaos accross shader ids
            
            // create the VAO
            self.resource_shader_id = Some(shader_id);
            
            // get a free vao ID
            self.vao_id = platform.get_free_vao();
            self.inst_vb_id = platform.get_free_index_buffer();
            
            platform.from_wasm.alloc_array_buffer(
                self.inst_vb_id,
                0,
                0 as *const f32
            );
            
            platform.from_wasm.alloc_vao(
                shader_id,
                self.vao_id,
                sh.platform.as_ref().unwrap().geom_ib_id,
                sh.platform.as_ref().unwrap().geom_vb_id,
                self.inst_vb_id,
            );
        }
    }
    
    fn free(&mut self, platform: &mut CxPlatform) {
        
        if self.vao_id != 0 {
            platform.vaos_free.push(self.vao_id);
        }
        if self.inst_vb_id != 0 {
            platform.vertex_buffers_free.push(self.inst_vb_id);
        }
        self.vao_id = 0;
        self.inst_vb_id = 0;
    }
}

use std::process::{Child};
pub fn spawn_process_command(_cmd: &str, _args: &[&str], _current_dir: &str) -> Result<Child, std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, ""))
}
