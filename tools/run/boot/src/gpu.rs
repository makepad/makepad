//! Reflection and packets, using the same shader compiler and HAL ABI as the app.
use makepad_hal::{
    dialect, format, pipeline_binding as binding, Attr, CmdWriter, Draw, Id, Inst, PipelineDesc,
    Slice,
};
use makepad_script::{
    id,
    pod::ScriptPodTy,
    shader_compile::{compile_shader, ShaderCompileOptions},
    shader_reflection::DrawShaderDesc,
    LiveId, NoTrap, ScriptHeap, ScriptObject, ScriptValue, ScriptVm,
};
use makepad_shader::{
    shader_backend::ShaderBackend,
    shader_output::{SamplerAddress, SamplerCoord, SamplerFilter, ShaderIoKind, TextureType},
    uniform_packing::UniformPacking,
};

pub const VERTICES: Id = Id::new(0, 1);
pub const INDICES: Id = Id::new(1, 1);

pub struct Field {
    name: LiveId,
    offset: usize,
    slots: usize,
}

#[derive(Default)]
pub struct Record {
    fields: Vec<Field>,
    pub defaults: Vec<f32>,
}

impl Record {
    fn push(&mut self, name: LiveId, slots: usize, packing: UniformPacking, value: Vec<f32>) {
        let mut end = self.defaults.len();
        let offset = packing.place(&mut end, slots);
        self.defaults.resize(end, 0.0);
        self.defaults[offset..offset + slots].copy_from_slice(&value);
        self.fields.push(Field {
            name,
            offset,
            slots,
        });
    }

    fn align(&mut self) {
        self.defaults
            .resize(self.defaults.len().next_multiple_of(4), 0.0);
    }

    pub fn write(&self, values: &[(LiveId, &[f32])], out: &mut Vec<f32>) {
        let start = out.len();
        out.extend_from_slice(&self.defaults);
        for (name, value) in values {
            if let Some(field) = self.fields.iter().find(|field| field.name == *name) {
                assert_eq!(field.slots, value.len(), "boot shader input {name}");
                out[start + field.offset..start + field.offset + field.slots]
                    .copy_from_slice(value);
            }
        }
    }
}

pub struct Pipeline {
    pub id: Id,
    pub ready: bool,
    pub instances: Record,
    pub uniforms: Record,
    scope: Vec<f32>,
    textures: usize,
    blob: Vec<u8>,
}

pub fn number(value: ScriptValue) -> Option<f64> {
    value.as_number()
}

/// IO markers carry their default in their prototype. No Rust shader source or
/// a second copy of the shader's layout is needed to build a record.
fn floats(heap: &ScriptHeap, mut value: ScriptValue, slots: usize) -> Vec<f32> {
    while let Some(object) = value.as_object() {
        value = heap.proto(object);
    }
    if let Some(color) = value.as_color() {
        return color
            .to_be_bytes()
            .iter()
            .take(slots)
            .map(|v| *v as f32 / 255.0)
            .collect();
    }
    if let Some(value) = number(value) {
        return vec![value as f32; slots];
    }
    if let Some(pod) = value.as_pod() {
        let (_, words) = heap.pod_data(pod);
        if words.len() == slots {
            return words.iter().map(|word| f32::from_bits(*word)).collect();
        }
    }
    vec![0.0; slots]
}

pub fn color(heap: &ScriptHeap, value: ScriptValue) -> [f32; 4] {
    floats(heap, value, 4)
        .try_into()
        .expect("four colour lanes")
}

impl Pipeline {
    pub fn compile(
        vm: &mut ScriptVm,
        shader: ScriptObject,
        dialect: u32,
        id: Id,
    ) -> Result<Self, String> {
        let (backend, packing) = match dialect {
            dialect::MSL => (ShaderBackend::Metal, UniformPacking::Metal),
            dialect::SPIRV => (ShaderBackend::Glsl, UniformPacking::Std140),
            // the packing the app's HAL client compiles D3D11 shaders with
            dialect::HLSL => (ShaderBackend::Hlsl, UniformPacking::Std140),
            other => return Err(format!("Unsupported HAL boot shader dialect {other}")),
        };
        let mut options =
            ShaderCompileOptions::new(backend, false, dialect == dialect::SPIRV, false, false);
        options.uniform_packing = packing;
        let output = compile_shader(vm, shader, &options);
        if output.has_errors() {
            return Err(output.error_report().into());
        }
        let desc = DrawShaderDesc::from_output(vm.heap(), &output);
        let heap = vm.heap();
        let mut instances = Record::default();
        let mut uniforms = Record::default();
        let mut scope = Record::default();
        let mut textures = 0;
        let mut geometry = false;
        let mut vertex = false;
        let mut fragment = false;
        for io in desc.io() {
            let slots = io.ty().slots();
            if matches!(
                io.kind(),
                ShaderIoKind::DynInstance | ShaderIoKind::Uniform | ShaderIoKind::ScopeUniform
            ) && !matches!(
                io.ty(),
                ScriptPodTy::F32
                    | ScriptPodTy::Vec(
                        makepad_script::pod::ScriptPodVec::Vec2f
                            | makepad_script::pod::ScriptPodVec::Vec3f
                            | makepad_script::pod::ScriptPodVec::Vec4f
                    )
            ) {
                return Err(format!(
                    "Boot shader input {} must be a float or float vector",
                    io.name()
                ));
            }
            match io.kind() {
                ShaderIoKind::DynInstance | ShaderIoKind::Uniform => {
                    // Reject a malformed override while the previous document
                    // still works, before any frame writes these host inputs.
                    let lanes = match io.kind() {
                        ShaderIoKind::DynInstance => match io.name() {
                            n if n == id!(rect)
                                || n == id!(uv)
                                || n == id!(color)
                                || n == id!(clip_rect) =>
                            {
                                Some(4)
                            }
                            n if [
                                id!(atlas_plane),
                                id!(color_glyph),
                                id!(selected),
                                id!(action),
                                id!(pressed),
                            ]
                            .contains(&n) =>
                            {
                                Some(1)
                            }
                            _ => None,
                        },
                        _ => match io.name() {
                            n if n == id!(viewport) => Some(2),
                            n if [id!(insets), id!(phase_marks)].contains(&n) => Some(4),
                            n if [
                                id!(time),
                                id!(from),
                                id!(previous),
                                id!(changed),
                                id!(completed),
                                id!(weight),
                                id!(total_weight),
                                id!(done),
                                id!(total),
                                id!(phase_elapsed),
                                id!(radius),
                                id!(cutoff),
                                id!(dark),
                                id!(reduce_motion),
                                id!(touch),
                                id!(dpi),
                                id!(overlay),
                                id!(phase_last),
                                id!(phase),
                                id!(paused),
                                id!(elapsed),
                                id!(warm),
                            ]
                            .contains(&n) =>
                            {
                                Some(1)
                            }
                            _ => None,
                        },
                    };
                    if lanes.is_some_and(|lanes| lanes != slots) {
                        return Err(format!(
                            "Boot shader input {} has the wrong vector size",
                            io.name()
                        ));
                    }
                    let value = heap.value(shader, io.name().into(), NoTrap);
                    let (record, packing) = if matches!(io.kind(), ShaderIoKind::DynInstance) {
                        (&mut instances, UniformPacking::Tight)
                    } else {
                        (&mut uniforms, packing)
                    };
                    record.push(io.name(), slots, packing, floats(heap, value, slots));
                }
                ShaderIoKind::VertexBuffer => {
                    if geometry
                        || io.name() != id!(geom)
                        || !matches!(
                            io.ty(),
                            ScriptPodTy::Vec(makepad_script::pod::ScriptPodVec::Vec2f)
                        )
                    {
                        return Err("Boot shaders use one geom: vertex_buffer(vec2f)".into());
                    }
                    geometry = true;
                }
                ShaderIoKind::Texture(TextureType::Texture2d) => {
                    if ![id!(grayscale_texture), id!(image_texture)].contains(&io.name())
                        || textures != 0
                    {
                        return Err(format!(
                            "Boot shader texture {} must be the single glyph atlas or app image",
                            io.name()
                        ));
                    }
                    textures += 1;
                }
                ShaderIoKind::VertexPosition => vertex = true,
                ShaderIoKind::FragmentOutput(0) => fragment = true,
                ShaderIoKind::Varying | ShaderIoKind::ScopeUniform => {}
                _ => return Err(format!("Unsupported boot shader IO {}", io.name())),
            }
        }
        if !geometry
            || !vertex
            || !fragment
            || !instances
                .fields
                .iter()
                .any(|f| f.name == id!(rect) && f.slots == 4)
        {
            return Err("Boot shader must provide a unit quad, rect: instance(vec4), vertex and fragment output".into());
        }
        for field in desc.scope_uniforms() {
            let value = if let Some(object) = field.source_obj() {
                floats(
                    heap,
                    heap.scope_value(object, field.source_key(), NoTrap),
                    field.slots(),
                )
            } else if let Some(index) = field.table_const() {
                let constant = &desc.table_consts()[index];
                constant.color().map_or_else(
                    || vec![constant.value() as f32; field.slots()],
                    |v| v.to_vec(),
                )
            } else {
                field.fixed().to_vec()
            };
            scope.push(field.shader_name(), field.slots(), packing, value);
        }
        uniforms.align();
        scope.align();
        let mut bindings = vec![binding::NONE; binding::BUFFERS];
        bindings[binding::DYN] = if uniforms.defaults.is_empty() {
            binding::NONE
        } else {
            2
        };
        bindings[binding::SCOPE] = desc
            .uniform_buffer_bindings()
            .scope_uniform_buffer_index()
            .map_or(binding::NONE, |i| i as u32);
        bindings[binding::TEXTURES] = textures as u32;
        let code = if dialect == dialect::SPIRV {
            let result = output.spirv().ok_or("Shader compiler returned no SPIR-V")?;
            if let Some(error) = result.error() {
                return Err(error.into());
            }
            let stages = result
                .source(0)
                .ok_or("Shader compiler returned no window SPIR-V")?;
            // The HAL's SPIR-V pipeline payload (makepad_hal::spirv), the same one the
            // platform's HAL client sends: the Vulkan backend decodes exactly this.
            let info = stages.info();
            makepad_hal::spirv::SpirvPipeline {
                vertex: stages.vertex().to_vec(),
                fragment: stages.fragment().to_vec(),
                texture_base: info.texture_binding_base(),
                sampler_base: info.sampler_binding_base(),
                depth_binding: info.xr_depth_binding(),
                instance_binding: info.instance_binding().unwrap_or(u32::MAX),
                pick_binding: info.pick_binding().unwrap_or(u32::MAX),
                pick_texture_base: info.pick_texture_base().unwrap_or(u32::MAX),
                // the boot document samples at most one 2D texture (checked above)
                textures: vec![0; textures],
                samplers: desc
                    .samplers()
                    .iter()
                    .map(|sampler| makepad_hal::spirv::SpirvSampler {
                        filter: match sampler.filter {
                            SamplerFilter::Nearest => 0,
                            SamplerFilter::Linear => 1,
                        },
                        address: match sampler.address {
                            SamplerAddress::Repeat => 0,
                            SamplerAddress::ClampToEdge => 1,
                            SamplerAddress::ClampToZero => 2,
                            SamplerAddress::MirroredRepeat => 3,
                        },
                        compare: sampler.compare,
                        pixel: sampler.coord == SamplerCoord::Pixel,
                    })
                    .collect(),
            }
            .encode()
        } else if dialect == dialect::HLSL {
            // The HAL's HLSL pipeline payload (makepad_hal::hlsl), as the app's HAL client sends
            // it: the one source plus the texture shapes and sampler states D3D11 binds.
            makepad_hal::hlsl::HlslPipeline {
                source: output.combined().to_owned(),
                textures: vec![0; textures],
                samplers: desc
                    .samplers()
                    .iter()
                    .map(|sampler| makepad_hal::spirv::SpirvSampler {
                        filter: match sampler.filter {
                            SamplerFilter::Nearest => 0,
                            SamplerFilter::Linear => 1,
                        },
                        address: match sampler.address {
                            SamplerAddress::Repeat => 0,
                            SamplerAddress::ClampToEdge => 1,
                            SamplerAddress::ClampToZero => 2,
                            SamplerAddress::MirroredRepeat => 3,
                        },
                        compare: sampler.compare,
                        pixel: false,
                    })
                    .collect(),
            }
            .encode()
        } else {
            output.combined().as_bytes().to_vec()
        };
        let blob = CmdWriter::pipeline_blob(
            PipelineDesc {
                color_formats: [format::BGRA8, 0, 0, 0],
                outputs: 1,
                shader: id.index,
                instance_stride: instances.defaults.len() as u32,
                ..Default::default()
            },
            &code,
            &bindings,
            &[8, 0, 1, 2],
        );
        Ok(Self {
            id,
            ready: false,
            instances,
            uniforms,
            scope: scope.defaults,
            textures,
            blob,
        })
    }

    pub fn create(&self, writer: &mut CmdWriter, dialect: u32) {
        writer.pipeline_create(self.id, dialect, Attr::default(), &self.blob);
    }

    pub fn draw(
        &self,
        writer: &mut CmdWriter,
        data: &[f32],
        uniforms: &[f32],
        atlas: Id,
        scissor: [f32; 4],
    ) {
        if data.is_empty() {
            return;
        }
        let textures = if self.textures != 0 { writer.put_ids(&[atlas]) } else { Default::default() };
        writer.draw(Draw {
            pipeline: self.id,
            vb: VERTICES,
            ib: INDICES,
            index: [0, 6],
            inst: Inst::inline(
                Slice::of(data),
                (data.len() / self.instances.defaults.len()) as u32,
            ),
            uniforms: [
                Slice::EMPTY,
                Slice::EMPTY,
                Slice::of(uniforms),
                Slice::of(&self.scope),
            ],
            tex: textures,
            scissor,
            bounds: [f32::NAN; 4],
            flags: Draw::BLEND | Draw::INDEX_U32,
            ..Default::default()
        });
    }
}
