//! WebGL2-only path: GLSL shader compilation for wasm (`FromWasmCompileWebGLShader` → `web_gl.js`).
//!
//! Shared draw-list encoding and `FromWasm*` buffer/render messages live in [`super::web_render`].

use crate::{
    cx::Cx,
    draw_shader::CxDrawShaderCode,
    os::web::from_wasm::{FromWasmCompileWebGLShader, WTextureInput},
    os::CxOsDrawShader,
};

impl Cx {
    pub fn webgl_compile_shaders(&mut self) {
        let compile_set: Vec<usize> = self.draw_shaders.compile_set.iter().copied().collect();
        for draw_shader_id in compile_set {
            let (vertex, pixel, geometry_slots, instance_slots, textures, debug_code) = {
                let cx_shader = &self.draw_shaders.shaders[draw_shader_id];
                let (vertex, pixel) = match &cx_shader.mapping.code {
                    CxDrawShaderCode::Separate { vertex, fragment } => {
                        (vertex.clone(), fragment.clone())
                    }
                    CxDrawShaderCode::Combined { .. } => {
                        crate::error!("Combined shader code is not supported on wasm webgl");
                        continue;
                    }
                };
                let textures: Vec<WTextureInput> = cx_shader
                    .mapping
                    .textures
                    .iter()
                    .map(|v| v.to_from_wasm_texture_input())
                    .collect();
                (
                    vertex,
                    pixel,
                    cx_shader.mapping.geometries.total_slots,
                    cx_shader.mapping.instances.total_slots,
                    textures,
                    cx_shader.mapping.flags.debug_code,
                )
            };

            if debug_code {
                crate::log!("{}\n{}", vertex, pixel);
            }

            let mut os_shader_id = self.draw_shaders.shaders[draw_shader_id].os_shader_id;
            if os_shader_id.is_none() {
                for (index, ds) in self.draw_shaders.os_shaders.iter().enumerate() {
                    if ds.in_vertex == vertex && ds.in_pixel == pixel {
                        os_shader_id = Some(index);
                        break;
                    }
                }
            }

            if os_shader_id.is_none() {
                let shp = CxOsDrawShader::new(vertex, pixel);
                let shader_id = self.draw_shaders.os_shaders.len();
                self.os.from_wasm(FromWasmCompileWebGLShader {
                    shader_id,
                    vertex: shp.vertex.clone(),
                    pixel: shp.pixel.clone(),
                    geometry_slots,
                    instance_slots,
                    textures,
                });
                self.draw_shaders.os_shaders.push(shp);
                os_shader_id = Some(shader_id);
            }

            self.draw_shaders.shaders[draw_shader_id].os_shader_id = os_shader_id;
        }
        self.draw_shaders.compile_set.clear();
    }
}

impl CxOsDrawShader {
    pub fn new(in_vertex: String, in_pixel: String) -> Self {
        let vertex = format!(
            "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
vec4 sample2d(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){{return textureLod(sampler, vec2(pos.x, pos.y), lod);}}
vec4 sample2d_bgra(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y)).zyxw;}}
vec4 sample2d_rt(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, 1.0 - pos.y));}}
vec4 samplecube(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){{return textureLod(sampler, dir, lod);}}
vec4 samplecube_bgra(samplerCube sampler, vec3 dir){{return texture(sampler, dir).zyxw;}}
vec4 depth_clip(vec4 w, vec4 c, float clip){{return c;}}
{}",
            in_vertex
        );

        let pixel = format!(
            "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
vec4 sample2d(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y));}}
vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){{return textureLod(sampler, vec2(pos.x, pos.y), lod);}}
vec4 sample2d_bgra(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, pos.y)).zyxw;}}
vec4 sample2d_rt(sampler2D sampler, vec2 pos){{return texture(sampler, vec2(pos.x, 1.0 - pos.y));}}
vec4 samplecube(samplerCube sampler, vec3 dir){{return texture(sampler, dir);}}
vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){{return textureLod(sampler, dir, lod);}}
vec4 samplecube_bgra(samplerCube sampler, vec3 dir){{return texture(sampler, dir).zyxw;}}
vec4 depth_clip(vec4 w, vec4 c, float clip){{return c;}}
{}",
            in_pixel
        );

        Self {
            in_vertex,
            in_pixel,
            vertex,
            pixel,
        }
    }
}
