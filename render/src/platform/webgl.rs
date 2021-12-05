
use crate::cx::*;
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
        zbias: &mut f32,
        zbias_step: f32
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
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
                    zbias,
                    zbias_step
                );
            }
            else {
                let cxview = &mut self.views[view_id];
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader.shader_id];

                if draw_call.instance_dirty || draw_call.platform.inst_vb_id.is_none() {
                    draw_call.instance_dirty = false;
                    if draw_call.platform.inst_vb_id.is_none() {
                        draw_call.platform.inst_vb_id = Some(self.platform.vertex_buffers);
                        self.platform.vertex_buffers += 1;
                    }
                    self.platform.from_wasm.alloc_array_buffer(
                        draw_call.platform.inst_vb_id.unwrap(),
                        draw_call.instances.len(),
                        draw_call.instances.as_ptr() as *const f32
                    );
                    draw_call.instance_dirty = false;
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
                
                let geometry_id = if let Some(geometry) = draw_call.geometry{
                    geometry.geometry_id
                }
                else if let Some(geometry) = sh.default_geometry{
                    geometry.geometry_id
                }
                else{
                    continue
                };
                
                // update geometry?
                let geometry = &mut self.geometries[geometry_id];
                
                if geometry.dirty || geometry.platform.vb_id.is_none() || geometry.platform.ib_id.is_none() {
                    if geometry.platform.vb_id.is_none() {
                        geometry.platform.vb_id = Some(self.platform.vertex_buffers);
                        self.platform.vertex_buffers += 1;
                    }
                    if geometry.platform.ib_id.is_none() {
                        geometry.platform.ib_id = Some(self.platform.index_buffers);
                        self.platform.index_buffers += 1;
                    }
                    self.platform.from_wasm.alloc_array_buffer(
                        geometry.platform.vb_id.unwrap(),
                        geometry.vertices.len(),
                        geometry.vertices.as_ptr() as *const f32
                    );

                    self.platform.from_wasm.alloc_index_buffer(
                        geometry.platform.ib_id.unwrap(),
                        geometry.indices.len(),
                        geometry.indices.as_ptr() as *const u32
                    );

                    geometry.dirty = false;
                }
                
                // lets check if our vao is still valid
                if draw_call.platform.vao.is_none() {
                    draw_call.platform.vao = Some(CxPlatformDrawCallVao {
                        vao_id: self.platform.vaos,
                        shader_id: None,
                        inst_vb_id: None,
                        geom_vb_id: None,
                        geom_ib_id: None,
                    });
                    self.platform.vaos += 1;
                }
                let vao = draw_call.platform.vao.as_mut().unwrap();
                if vao.inst_vb_id != draw_call.platform.inst_vb_id
                    || vao.geom_vb_id != geometry.platform.vb_id
                    || vao.geom_ib_id != geometry.platform.ib_id
                    || vao.shader_id != Some(draw_call.shader.shader_id) {
                        
                    vao.shader_id = Some(draw_call.shader.shader_id);
                    vao.inst_vb_id = draw_call.platform.inst_vb_id;
                    vao.geom_vb_id = geometry.platform.vb_id;
                    vao.geom_ib_id = geometry.platform.ib_id;
                    
                    self.platform.from_wasm.alloc_vao(
                        vao.vao_id,
                        vao.shader_id.unwrap(),
                        vao.geom_ib_id.unwrap(),
                        vao.geom_vb_id.unwrap(),
                        draw_call.platform.inst_vb_id.unwrap()
                    );
                }
                
                self.platform.from_wasm.draw_call(
                    draw_call.shader.shader_id,
                    draw_call.platform.vao.as_ref().unwrap().vao_id,
                    self.passes[pass_id].pass_uniforms.as_slice(),
                    cxview.view_uniforms.as_slice(),
                    draw_call.draw_uniforms.as_slice(),
                    &draw_call.user_uniforms,
                    &sh.mapping.live_uniforms_buf,
                    &draw_call.textures_2d,
                    &sh.mapping.const_table
                );
                
                
            }
        }
        if let Some(_) = &self.views[view_id].debug {
            let mut s = String::new();
            self.debug_draw_tree_recur(false, &mut s, view_id, 0);
            console_log(&s);
        }
    }
    
    pub fn setup_render_pass(&mut self, pass_id: usize, inherit_dpi_factor: f32) {
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
    }
    
    pub fn draw_pass_to_canvas(&mut self, pass_id: usize, dpi_factor: f32) {
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        // get the color and depth
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
        self.platform.from_wasm.begin_main_canvas(clear_color, clear_depth as f32);
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        self.platform.from_wasm.set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            &mut zbias,
            zbias_step
        );
    }
    
    pub fn draw_pass_to_texture(&mut self, pass_id: usize, dpi_factor: f32) {
        let pass_size = self.passes[pass_id].pass_size;
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        self.platform.from_wasm.begin_render_targets(pass_id, (pass_size.x * dpi_factor) as usize, (pass_size.y * dpi_factor) as usize);
        
        for color_texture in &self.passes[pass_id].color_textures {
            match color_texture.clear_color {
                ClearColor::InitWith(color) => {
                    self.platform.from_wasm.add_color_target(color_texture.texture_id as usize, true, color);
                },
                ClearColor::ClearWith(color) => {
                    self.platform.from_wasm.add_color_target(color_texture.texture_id as usize, false, color);
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            match self.passes[pass_id].clear_depth {
                ClearDepth::InitWith(depth_clear) => {
                    self.platform.from_wasm.set_depth_target(depth_texture_id as usize, true, depth_clear as f32);
                },
                ClearDepth::ClearWith(depth_clear) => {
                    self.platform.from_wasm.set_depth_target(depth_texture_id as usize, false, depth_clear as f32);
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
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            &mut zbias,
            zbias_step
        );
    }
    
    
    pub fn webgl_compile_all_shaders(&mut self) {
        let options = ShaderCompileOptions {
            gather_all: !self.gpu_info.is_low_on_uniform_vectors(),
            create_const_table: false,
            no_const_collapse: false
        };
        
        for (live_item_id,_shader) in &self.live_styles.shader_alloc{
            match self.live_styles.collect_and_analyse_shader(*live_item_id, options) {
                Err(err) => {
                    self.platform.from_wasm.log(&format!("{}", err))
                },
                Ok((shader_ast, default_geometry)) => {
                    let shader_id = self.live_styles.shader_alloc.get(live_item_id).unwrap().shader_id;
                    Self::webgl_compile_shader(
                        shader_id,
                        self.live_styles.live_item_id_to_string(*live_item_id).unwrap(),
                        &mut self.shaders[shader_id],
                        shader_ast,
                        default_geometry,
                        options,
                        &mut self.platform,
                        &self.live_styles
                    );
                }
            }
        };
        self.live_styles.changed_shaders.clear();
    }
    
    pub fn webgl_update_all_shaders(&mut self, errors:&mut Vec<LiveBodyError>)  {
        let options = ShaderCompileOptions {
            gather_all: !self.gpu_info.is_low_on_uniform_vectors(),
            create_const_table: true,
            no_const_collapse: false
        };
        
        for (live_item_id, change) in &self.live_styles.changed_shaders {
            match change {
                LiveChangeType::Recompile => {
                    match self.live_styles.collect_and_analyse_shader(*live_item_id, options) {
                        Err(err) => {
                            errors.push(err);
                        },
                        Ok((shader_ast, default_geometry)) => {
                            let shader_id = self.live_styles.shader_alloc.get(&live_item_id).unwrap().shader_id;
                            Self::webgl_compile_shader(
                                shader_id,
                                self.live_styles.live_item_id_to_string(*live_item_id).unwrap(),
                                &mut self.shaders[shader_id],
                                shader_ast,
                                default_geometry,
                                options,
                                &mut self.platform,
                                &self.live_styles
                            );
                        }
                    }
                }
                LiveChangeType::UpdateValue => {
                    let shader_id = self.live_styles.shader_alloc.get(&live_item_id).unwrap().shader_id;
                    self.shaders[shader_id].mapping.update_live_uniforms(&self.live_styles);
                }
            }
        }
        self.live_styles.changed_shaders.clear();
    }
    
    pub fn webgl_compile_shader(
        shader_id: usize,
        name: String,
        sh: &mut CxShader,
        shader_ast: ShaderAst,
        default_geometry: Option<Geometry>,
        options: ShaderCompileOptions,
        platform: &mut CxPlatform,
        live_styles: &LiveStyles
    ) -> ShaderCompileResult {
        
        let vertex = generate_glsl::generate_vertex_shader(&shader_ast, live_styles, options);
        let fragment = generate_glsl::generate_fragment_shader(&shader_ast, live_styles, options);
        
        let vertex = format!("
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            mat4 transpose(mat4 m){{return mat4(m[0][0],m[1][0],m[2][0],m[3][0],m[0][1],m[1][1],m[2][1],m[3][1],m[0][2],m[1][2],m[2][2],m[3][3], m[3][0], m[3][1], m[3][2], m[3][3]);}}
            mat3 transpose(mat3 m){{return mat3(m[0][0],m[1][0],m[2][0],m[0][1],m[1][1],m[2][1],m[0][2],m[1][2],m[2][2]);}}
            mat2 transpose(mat2 m){{return mat2(m[0][0],m[1][0],m[0][1],m[1][1]);}}
            {}\0", vertex);
        let fragment = format!("
            #extension GL_OES_standard_derivatives : enable
            precision highp float;
            precision highp int;
            vec4 sample2d(sampler2D sampler, vec2 pos){{return texture2D(sampler, vec2(pos.x, 1.0-pos.y));}}
            mat4 transpose(mat4 m){{return mat4(m[0][0],m[1][0],m[2][0],m[3][0],m[0][1],m[1][1],m[2][1],m[3][1],m[0][2],m[1][2],m[2][2],m[3][3], m[3][0], m[3][1], m[3][2], m[3][3]);}}
            mat3 transpose(mat3 m){{return mat3(m[0][0],m[1][0],m[2][0],m[0][1],m[1][1],m[2][1],m[0][2],m[1][2],m[2][2]);}}
            mat2 transpose(mat2 m){{return mat2(m[0][0],m[1][0],m[0][1],m[1][1]);}}
            {}\0", fragment);
        
        if shader_ast.debug {
            platform.from_wasm.log(&format!(
                "--------------- Vertex shader {} --------------- \n{}\n---------------\n--------------- Fragment shader {} --------------- \n{}\n---------------\n",
                shader_id,
                vertex,
                shader_id,
                fragment
            ));
        }
        
        let mut mapping = CxShaderMapping::from_shader_ast(shader_ast, options, false);
        mapping.update_live_uniforms(live_styles);
        // lets check if we need to recompile the shader at all
        if let Some(sh_platform) = &sh.platform {
            if sh_platform.vertex == vertex && sh_platform.fragment == fragment {
                sh.mapping = mapping;
                return ShaderCompileResult::Nop
            }
        }
        //let shader_id = self.compiled_shaders.len();
        platform.from_wasm.compile_webgl_shader(shader_id, &vertex, &fragment, &mapping);
        sh.name = name;
        sh.default_geometry = default_geometry;
        sh.mapping = mapping;
        sh.platform = Some(CxPlatformShader {
            vertex: vertex,
            fragment: fragment
        });
        
        ShaderCompileResult::Ok 
    }
    
}

#[derive(Default, Clone)]
pub struct CxPlatformPass {
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
}

#[derive(Default, Clone)]
pub struct CxPlatformDrawCallVao {
    pub vao_id: usize,
    pub shader_id: Option<usize>,
    pub inst_vb_id: Option<usize>,
    pub geom_vb_id: Option<usize>,
    pub geom_ib_id: Option<usize>,
}

#[derive(Default, Clone)]
pub struct CxPlatformDrawCall {
    pub vao: Option<CxPlatformDrawCallVao>,
    pub inst_vb_id: Option<usize>,
}

#[derive(Clone)]
pub struct CxPlatformShader {
    pub vertex: String,
    pub fragment: String,
}

#[derive(Clone, Default)]
pub struct CxPlatformTexture {
}

#[derive(Clone, Default)]
pub struct CxPlatformGeometry {
    pub vb_id: Option<usize>,
    pub ib_id: Option<usize>
}

impl CxPlatformDrawCall {
}

use std::process::{Child};
pub fn spawn_process_command(_cmd: &str, _args: &[&str], _current_dir: &str) -> Result<Child, std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, ""))
}
