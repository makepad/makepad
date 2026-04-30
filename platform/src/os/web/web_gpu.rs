//! WebGPU-only path: WGSL shader compilation for wasm (`FromWasmCompileWebGPUShader` → `web_gpu.js`).
//!
//! Compare with [`super::web_gl`] (`FromWasmCompileWebGLShader` → `web_gl.js`).

use crate::{
    cx::Cx,
    draw_shader::CxDrawShaderCode,
    os::web::from_wasm::{FromWasmCompileWebGPUShader, WSampler, WTextureInput},
};

impl Cx {
    pub fn webgpu_compile_shaders(&mut self) {
        let compile_set: Vec<usize> = self.draw_shaders.compile_set.iter().copied().collect();
        for draw_shader_id in compile_set {
            let (
                wgsl,
                geometry_slots,
                instance_slots,
                textures,
                debug_code,
                dyn_uniform_binding,
                texture_binding_base,
                sampler_binding_base,
                xr_depth_binding,
                texture_sampler_indices,
                samplers,
            ) = {
                let cx_shader = &self.draw_shaders.shaders[draw_shader_id];
                let (wgsl, dyn_uniform_binding, texture_binding_base, sampler_binding_base, xr_depth_binding) =
                    match &cx_shader.mapping.code {
                        CxDrawShaderCode::Wgsl {
                            wgsl,
                            dyn_uniform_binding,
                            texture_binding_base,
                            sampler_binding_base,
                            xr_depth_binding,
                        } => (
                            wgsl.clone(),
                            *dyn_uniform_binding,
                            *texture_binding_base,
                            *sampler_binding_base,
                            *xr_depth_binding,
                        ),
                        _ => continue,
                    };
                let textures: Vec<WTextureInput> = cx_shader
                    .mapping
                    .textures
                    .iter()
                    .map(|v| v.to_from_wasm_texture_input())
                    .collect();
                let texture_sampler_indices = cx_shader.mapping.texture_sampler_indices.clone();
                let samplers = cx_shader
                    .mapping
                    .samplers
                    .iter()
                    .map(|s| {
                        let filter = match s.filter {
                            makepad_script::shader_output::SamplerFilter::Nearest => 0,
                            makepad_script::shader_output::SamplerFilter::Linear => 1,
                        };
                        let address = match s.address {
                            makepad_script::shader_output::SamplerAddress::Repeat => 0,
                            makepad_script::shader_output::SamplerAddress::ClampToEdge => 1,
                            makepad_script::shader_output::SamplerAddress::ClampToZero => 2,
                            makepad_script::shader_output::SamplerAddress::MirroredRepeat => 3,
                        };
                        let coord = match s.coord {
                            makepad_script::shader_output::SamplerCoord::Normalized => 0,
                            makepad_script::shader_output::SamplerCoord::Pixel => 1,
                        };
                        WSampler {
                            filter,
                            address,
                            coord,
                            is_video: s.is_video,
                        }
                    })
                    .collect::<Vec<_>>();
                (
                    wgsl,
                    cx_shader.mapping.geometries.total_slots,
                    cx_shader.mapping.instances.total_slots,
                    textures,
                    cx_shader.mapping.flags.debug_code,
                    dyn_uniform_binding,
                    texture_binding_base,
                    sampler_binding_base,
                    xr_depth_binding,
                    texture_sampler_indices,
                    samplers,
                )
            };

            if debug_code {
                crate::log!("{}", wgsl);
            }

            let shader_id = self.draw_shaders.os_shaders.len();
            self.os.from_wasm(FromWasmCompileWebGPUShader {
                shader_id,
                wgsl: wgsl.clone(),
                geometry_slots,
                instance_slots,
                textures,
                texture_sampler_indices,
                samplers,
                dyn_uniform_binding,
                texture_binding_base,
                sampler_binding_base,
                xr_depth_binding,
            });

            // Reuse `os_shader_id` as the WebGPU pipeline id.
            self.draw_shaders
                .os_shaders
                .push(crate::os::CxOsDrawShader::new(wgsl, String::new()));
            self.draw_shaders.shaders[draw_shader_id].os_shader_id = Some(shader_id);
        }
        self.draw_shaders.compile_set.clear();
    }
}
