//! Cx-side shader compilation for the dialect returned by hal_open. Vulkan receives the same
//! SPIR-V stages and descriptor conventions as native Vulkan; Metal receives MSL; D3D11 receives
//! one HLSL source (`vertex_main` / `pixel_main`, registers as shader_ir_hlsl documents); GLES
//! receives the complete GLSL ES 3.00 stages with their reflection (`makepad_hal::glsl`).

use crate::{
    draw_shader::{CxDrawShader, CxDrawShaderCode, CxDrawShaderMapping, DrawShaderId},
    draw_vars::DrawVars,
    geometry::Geometry,
    makepad_live_id::*,
    makepad_script::{apply::Apply, shader_backend::ShaderBackend, value::ScriptValue, ScriptVm},
    script::vm::ScriptVmCx,
};

impl DrawVars {
    pub(crate) fn compile_shader(&mut self, vm: &mut ScriptVm, _apply: &Apply, value: ScriptValue) {
        // caches, cheapest first: this exact object, the same functions, the same generated code
        let Some(io_self) = value.as_object() else { return };
        // the cache is keyed by heap as well as object: a splash isolate has its own heap
        let heap_key = vm.heap_key();
        if let Some(&shader_id) = vm.host.cx().draw_shaders.cache_object_id_to_shader.get(&(heap_key, io_self)) {
            self.finalize_cached_shader(vm, shader_id);
            return;
        }
        let fnhash = crate::makepad_script::shader_compile::shader_functions_hash(vm.heap(), vm.bx.code_ref(), io_self);
        let pipe = DrawVars::pipeline_state_hash(vm.heap(), io_self);
        if let Some(&shader_id) = vm.host.cx().draw_shaders.cache_functions_to_shader.get(&fnhash) {
            vm.host.cx_mut().draw_shaders.cache_object_id_to_shader.insert((heap_key, io_self), shader_id);
            self.finalize_cached_shader(vm, shader_id);
            return;
        }

        // not seen: compile. Uniform buffer indices start at 3 (0 = geometry, 1 = instances,
        // 2 = dyn uniforms)
        let dialect = vm.host.cx().hal.info.map_or(makepad_hal::dialect::NONE, |i| i.dialect);
        let vulkan = dialect == makepad_hal::dialect::SPIRV;
        let hlsl = dialect == makepad_hal::dialect::HLSL;
        let gl = dialect == makepad_hal::dialect::GLSL;
        let backend = match dialect {
            makepad_hal::dialect::SPIRV | makepad_hal::dialect::GLSL => ShaderBackend::Glsl,
            makepad_hal::dialect::HLSL => ShaderBackend::Hlsl,
            _ => ShaderBackend::Metal,
        };
        let mut opts = crate::makepad_script::shader_compile::ShaderCompileOptions::new(
            backend,
            vm.host.cx().shader_const_table_mode(),
            vulkan,
            false,
            true,
        );
        opts.gles_complete = gl;
        opts.pick = vm.host.cx().pick_variants();
        let output = crate::makepad_script::shader_compile::compile_shader(vm, io_self, &opts);
        if output.has_errors() {
            DrawVars::log_shader_compile_failure(vm, io_self, &output);
            return;
        }
        let code = if vulkan || gl {
            let view = output.view();
            CxDrawShaderCode::Separate { vertex: view.glsl_vertex.to_owned(), fragment: view.glsl_fragment.to_owned() }
        } else { CxDrawShaderCode::Combined { code: output.combined().to_owned() } };
        if let Some(shader_id) = vm.host.cx().draw_shaders.code_hit(&code, pipe, output.table_consts()) {
            let cx = vm.host.cx_mut();
            cx.draw_shaders.cache_object_id_to_shader.insert((heap_key, io_self), shader_id);
            cx.draw_shaders.cache_functions_to_shader.insert(fnhash, shader_id);
            self.finalize_cached_shader(vm, shader_id);
            return;
        }

        // the geometry of the vertex buffer object, before the mapping is made
        let geometry_id = output.find_vertex_buffer_object(vm, io_self).and_then(|vb_obj| {
            let buffer_value = vm.value_trap(vb_obj, id!(buffer));
            let handle = buffer_value.as_handle()?;
            vm.heap().handle_ref::<Geometry>(handle).map(|g| g.geometry_id())
        });
        let source = vm.new_object_ref(io_self);
        let mut mapping = CxDrawShaderMapping::from_shader_output(source, code.clone(), vm, &output, geometry_id);
        mapping.pick_code = output.pick_code().map(str::to_owned);
        if vulkan {
            let payload = |result: &crate::makepad_script::shader_ir_draw::IrDrawSpirvResult| -> Result<Vec<u8>, String> {
                use crate::makepad_script::shader::{SamplerAddress, SamplerCoord, SamplerFilter, TextureType};
                let view = result.view();
                if let Some(error) = view.error { return Err(error.to_owned()); }
                let stage = &view.stages[0];
                let textures = mapping.textures.iter().map(|t| match t.tex_type {
                    TextureType::Texture2d | TextureType::TextureVideo => Ok(0), TextureType::TextureCube => Ok(1),
                    TextureType::Texture2dArray => Ok(2), TextureType::TextureDepth => Ok(3), TextureType::TextureDepthArray => Ok(4),
                    _ => Err("HAL Vulkan texture shape is not a 2D/cube/depth image".to_owned()),
                }).collect::<Result<Vec<_>, _>>()?;
                let samplers: Vec<_> = mapping.samplers.iter().map(|s| makepad_hal::spirv::SpirvSampler {
                    filter: if s.filter == SamplerFilter::Nearest { 0 } else { 1 },
                    address: match s.address { SamplerAddress::Repeat => 0, SamplerAddress::ClampToEdge => 1, SamplerAddress::ClampToZero => 2, SamplerAddress::MirroredRepeat => 3 },
                    compare: s.compare, pixel: s.coord == SamplerCoord::Pixel,
                }).collect();
                Ok(makepad_hal::spirv::SpirvPipeline { vertex: stage.vertex.to_vec(), fragment: stage.fragment.to_vec(),
                    texture_base: stage.info.texture_binding_base(), sampler_base: stage.info.sampler_binding_base(),
                    depth_binding: stage.info.xr_depth_binding(), instance_binding: stage.info.instance_binding().unwrap_or(u32::MAX),
                    pick_binding: stage.info.pick_binding().unwrap_or(u32::MAX), pick_texture_base: stage.info.pick_texture_base().unwrap_or(u32::MAX),
                    textures, samplers }.encode())
            };
            let Some(spirv) = output.spirv() else { return };
            let normal = match payload(spirv) { Ok(bytes) => bytes, Err(e) => { crate::error!("Vulkan shader: {e}"); return; } };
            let pick = match output.view().pick_spirv {
                Some(result) => match payload(result) { Ok(bytes) => Some(bytes), Err(e) => { crate::error!("Vulkan pick shader: {e}"); return; } },
                None => None,
            };
            mapping.hal_code = Some(normal); mapping.hal_pick_code = pick;
        }
        if hlsl {
            use crate::makepad_script::shader::{SamplerAddress, SamplerFilter, TextureType};
            let CxDrawShaderCode::Combined { code: source } = &code else { return };
            let textures = match mapping.textures.iter().map(|t| match t.tex_type {
                TextureType::Texture2d | TextureType::TextureVideo => Ok(0), TextureType::TextureCube => Ok(1),
                TextureType::Texture2dArray => Ok(2), TextureType::TextureDepth => Ok(3), TextureType::TextureDepthArray => Ok(4),
                _ => Err(()),
            }).collect::<Result<Vec<_>, _>>() {
                Ok(t) => t,
                Err(()) => { crate::error!("D3D11 shader: a texture shape that is not 2D/cube/array/depth"); return; }
            };
            let samplers: Vec<_> = mapping.samplers.iter().map(|s| makepad_hal::spirv::SpirvSampler {
                filter: if s.filter == SamplerFilter::Nearest { 0 } else { 1 },
                address: match s.address { SamplerAddress::Repeat => 0, SamplerAddress::ClampToEdge => 1, SamplerAddress::ClampToZero => 2, SamplerAddress::MirroredRepeat => 3 },
                compare: s.compare, pixel: false,
            }).collect();
            let payload = |source: &str| makepad_hal::hlsl::HlslPipeline { source: source.to_owned(), textures: textures.clone(), samplers: samplers.clone() }.encode();
            mapping.hal_code = Some(payload(source));
            mapping.hal_pick_code = output.pick_code().map(payload);
        }
        if gl {
            let view = output.view();
            let Some(sources) = view.glsl else { return };
            let payload = |sources: &crate::makepad_script::shader_ir_glsl::GlslDrawSources, pick| {
                gl_payload(&mapping, sources, pick)
            };
            mapping.hal_code = Some(match payload(sources, false) {
                Ok(bytes) => bytes, Err(e) => { crate::error!("GL shader: {e}"); return; }
            });
            mapping.hal_pick_code = match view.pick_glsl {
                Some(sources) => match gl_payload(&mapping, sources, true) {
                    Ok(bytes) => Some(bytes), Err(e) => { crate::error!("GL pick shader: {e}"); return; }
                },
                None => None,
            };
        }
        mapping.fill_scope_uniforms_buffer(vm.heap(), &vm.trap());
        if !self.bind_dyn_instances(vm, io_self, &mapping) {
            return;
        }

        let cx = vm.host.cx_mut();
        mapping.scope_uniforms_gen = cx.next_uniform_gen();
        let index = cx.draw_shaders.shaders.len();
        cx.draw_shaders.shaders.push(CxDrawShader { debug_id: LiveId(0), pipeline_id: None, mapping });
        let shader_id = DrawShaderId { index };
        cx.draw_shaders.cache_object_id_to_shader.insert((heap_key, io_self), shader_id);
        cx.draw_shaders.cache_functions_to_shader.insert(fnhash, shader_id);
        cx.draw_shaders.cache_code_to_shader.insert((code, pipe), shader_id);
        // the frame builder creates the HAL pipeline at the next beat
        cx.draw_shaders.compile_set.insert(index);
        self.draw_shader_id = Some(shader_id);
        self.geometry_id = geometry_id;
    }
}

fn gl_payload(mapping: &CxDrawShaderMapping, sources: &crate::makepad_script::shader_ir_glsl::GlslDrawSources, pick: bool) -> Result<Vec<u8>, String> {
    use crate::makepad_script::shader::{SamplerAddress, SamplerCoord, SamplerFilter, TextureType};
    use makepad_hal::glsl::{Attribute, GlslPipeline, Texture};
    let info = sources.info();
    let mut attributes = Vec::new();
    for (instance, inputs, prefix) in [(false, &mapping.geometries, "geom"), (true, &mapping.instances, "inst")] {
        if info.typed_geometry {
            for input in &inputs.inputs {
                // Matrix columns occupy consecutive attribute locations; the executor splits them.
                for column in 0..input.slots.div_ceil(4) {
                    attributes.push(Attribute { name: format!("{}_{}", prefix, input.id),
                        offset: input.byte_offset as u32 + column as u32 * 16,
                        format: input.attr_format as u32, lanes: (input.slots - column * 4).min(4) as u32, instance });
                }
            }
        } else {
            // the packed vec4 attributes the GL entry points declare; instance vec4s from
            // `instance_fetch_from` on are read from the instance word texture
            let slots = if instance { info.instance_slots } else { info.geometry_slots };
            let vec4s = slots.div_ceil(4);
            let count = if instance { info.instance_fetch_from.map_or(vec4s, |from| from.min(vec4s)) } else { vec4s };
            for i in 0..count {
                attributes.push(Attribute { name: format!("packed_{}_{}", if instance { "instance" } else { "geometry" }, i),
                    offset: i as u32 * 16, format: 3, lanes: (slots - i * 4).min(4) as u32, instance });
            }
        }
    }
    let mut blocks: Vec<String> = ["draw_callUniforms", "passUniforms", "draw_listUniforms", "userUniforms", "liveUniforms"]
        .into_iter().map(str::to_owned).collect();
    blocks.extend(mapping.uniform_buffers.iter().map(|b| b.block_name.clone()));
    let textures = mapping.textures.iter().enumerate().map(|(i, t)| {
        let shape = match t.tex_type {
            TextureType::Texture2d | TextureType::TextureVideo => 0, TextureType::TextureCube => 1,
            TextureType::Texture2dArray => 2, TextureType::TextureDepth => 3, TextureType::TextureDepthArray => 4,
            _ => return Err("unsupported GLES texture shape".to_owned()),
        };
        let sampler = mapping.texture_sampler_indices.get(i).and_then(|i| mapping.samplers.get(*i)).map_or(
            makepad_hal::spirv::SpirvSampler { filter: 1, address: 1, compare: shape >= 3, pixel: false },
            |s| makepad_hal::spirv::SpirvSampler {
                filter: if s.filter == SamplerFilter::Nearest { 0 } else { 1 },
                address: match s.address { SamplerAddress::Repeat => 0, SamplerAddress::ClampToEdge => 1, SamplerAddress::ClampToZero => 2, SamplerAddress::MirroredRepeat => 3 },
                compare: s.compare, pixel: s.coord == SamplerCoord::Pixel,
            });
        Ok(Texture { name: format!("tex_{}", t.id), shape, sampler })
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(GlslPipeline { vertex: sources.vertex().to_owned(), fragment: sources.fragment().to_owned(), blocks, attributes, textures,
        vertex_stride: mapping.geometry_stride_bytes() as u32,
        instance_stride: mapping.instances.total_slots as u32 * 4,
        instance_texture: info.instance_fetch_from.is_some(), pick }.encode())
}
