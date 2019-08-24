
use crate::cx::*;

impl Cx {
    pub fn render_view(&mut self, pass_id: usize, view_id: usize, vr_is_presenting:bool) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        self.views[view_id].set_clipping_uniforms();
        if vr_is_presenting{
            self.views[view_id].uniform_view_transform(&Mat4::scale_translate(0.0005,-0.0005,0.001,-0.3,1.8,-0.4));
        }
        else{
            self.views[view_id].uniform_view_transform(&Mat4::identity());
        }
        for draw_call_id in 0..draw_calls_len {
            
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(pass_id, sub_view_id, vr_is_presenting);
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
                
                // update/alloc textures?
                for texture_id in &draw_call.textures_2d {
                    let cxtexture = &mut self.textures[*texture_id as usize];
                    if cxtexture.update_image {
                        cxtexture.update_image = false;
                        self.platform.from_wasm.update_texture_image2d(*texture_id as usize, cxtexture);
                        //Self::update_platform_texture_image2d(&mut self.platform);
                    }
                }
                let cxuniforms = &self.passes[pass_id].uniforms;
                
                self.platform.from_wasm.draw_call(
                    draw_call.shader_id,
                    draw_call.platform.vao_id,
                    cxuniforms,
                    self.redraw_id as usize,
                    // update once a frame
                    &cxview.uniforms,
                    view_id,
                    // update on drawlist change
                    &draw_call.uniforms,
                    draw_call.draw_call_id,
                    // update on drawcall id change
                    &draw_call.textures_2d
                );
            }
        }
    }
    
    pub fn draw_pass_to_canvas(&mut self, pass_id: usize, vr_is_presenting:bool) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        let pass_size = self.passes[pass_id].pass_size;
        
        // this ortho matrix needs to be a 3D one now
        self.passes[pass_id].set_ortho_matrix(Vec2::zero(), pass_size);
        self.passes[pass_id].uniform_camera_view(&Mat4::identity());

        if self.passes[pass_id].color_textures.len()>0 {
            let color_texture = &self.passes[pass_id].color_textures[0];
            if let Some(color) = color_texture.clear_color {
                self.platform.from_wasm.clear(color.r, color.g, color.b, color.a);
            }
        }
        
        self.platform.from_wasm.begin_frame();
        
        self.render_view(pass_id, view_id, vr_is_presenting);

        self.platform.from_wasm.end_frame();
    }
    
    pub fn webgl_compile_all_shaders(&mut self) {
        for (shader_id, sh) in self.shaders.iter_mut().enumerate() {
            let glsh = Self::webgl_compile_shader(shader_id, sh, &mut self.platform);
            if let Err(err) = glsh {
                self.platform.from_wasm.log(&format!("Got GLSL shader compile error: {}", err.msg))
            }
        }
    }
    
    pub fn webgl_compile_shader(shader_id: usize, sh: &mut CxShader, platform: &mut CxPlatform) -> Result<(), SlErr> {
        let (vertex, fragment, mapping) = Self::gl_assemble_shader(&sh.shader_gen, GLShaderType::WebGL1) ?;
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
        
        Ok(())
    }
    
}

#[derive(Default, Clone, Debug)]
pub struct CxPlatformPass {
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default, Clone)]
pub struct PlatformDrawCall {
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

impl PlatformDrawCall {
    
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
