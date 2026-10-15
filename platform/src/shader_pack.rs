//! Shader-pack transport and platform mapping verification. Pack internals live with the compiler.
use crate::cx::Cx;
#[cfg(not(target_arch = "wasm32"))]
use crate::{draw_vars::DrawVars, makepad_script::*, script::vm::ScriptVmCx};
#[cfg(not(target_arch = "wasm32"))]
use crate::makepad_script::shader_pack::{ShaderPacks, shader_pack_key};
use std::borrow::Cow;

impl Cx {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn record_shader_pack(&mut self, on: bool) { self.draw_shaders.packs.record_enabled(on); }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn take_shader_pack(&mut self) -> Vec<u8> { self.draw_shaders.packs.take_recording().bytes().to_vec() }
    pub fn install_shader_pack(&mut self, pack: impl Into<Cow<'static, [u8]>>) -> Result<usize, String> {
        let pack = pack.into();
        let packs = &mut self.draw_shaders.packs;
        if packs.install(&pack) { Ok(packs.installed_count()) } else { Err(packs.error().to_owned()) }
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl DrawVars {
    pub(crate) fn record_shader_pack_entry(vm: &mut ScriptVm, shader: ScriptObject) {
        let mut packs = std::mem::replace(&mut vm.host.cx_mut().draw_shaders.packs, ShaderPacks::new());
        packs.record(vm, shader);
        if !packs.error().is_empty() {
            crate::shader_error::note(packs.error().to_owned());
            crate::error!("shader pack: {}", packs.error());
        }
        vm.host.cx_mut().draw_shaders.packs = packs;
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn verify_shader_pack_entry(vm: &mut ScriptVm, io_self: ScriptObject) -> Result<u64, String> {
    use crate::draw_shader::{CxDrawShaderCode, CxDrawShaderMapping};
    use crate::makepad_script::shader_pack::{record_shader, inspect_shader_pack};
    let key = shader_pack_key(vm, io_self, false);
    let (output, code) = DrawVars::compile_glsl(vm, io_self, false).ok_or("the shader does not compile")?;
    let source = vm.new_object_ref(io_self);
    let live = CxDrawShaderMapping::from_shader_output(source, code.clone(), vm, &output, None);
    let record = record_shader(vm, io_self);
    let entry = record.entry().ok_or_else(|| record.error().unwrap().to_owned())?;
    let bytes = record.bytes();
    let read = inspect_shader_pack(bytes.bytes());
    if let Some(error) = read.error() { return Err(error.to_owned()); }
    if read.len() != 1 { return Err("the pack does not hold one entry".into()); }
    let back = read.entry(0).unwrap();
    if back != entry { return Err("the entry does not read back as written".into()); }
    let back_code = CxDrawShaderCode::Separate { vertex: back.vertex().to_owned(), fragment: back.fragment().to_owned() };
    if back_code != code { return Err("the pack's code differs from the compile's".into()); }
    let source = vm.new_object_ref(io_self);
    let back_desc = back.to_desc(vm.heap(), io_self);
    let mut packed = CxDrawShaderMapping::from_desc(source, back_code, vm, &back_desc, None);
    let mut live = live;
    live.fill_scope_uniforms_buffer(vm.heap(), &NoTrap);
    packed.fill_scope_uniforms_buffer(vm.heap(), &NoTrap);
    mapping_difference(&live, &packed).map_or(Ok(key), Err)
}
/// The first field two mappings differ in (pod types compared by
/// structure, table constants without their source position).
#[cfg(not(target_arch = "wasm32"))]
fn mapping_difference(a: &crate::draw_shader::CxDrawShaderMapping, b: &crate::draw_shader::CxDrawShaderMapping) -> Option<String> {
    fn same<T: std::fmt::Debug>(field: &str, a: T, b: T) -> Result<(), String> {
        let (a, b) = (format!("{:?}", a), format!("{:?}", b));
        if a == b {
            Ok(())
        } else {
            Err(format!("`{}` differs:\n  compile: {}\n  pack:    {}", field, a, b))
        }
    }
    let same_reflection = |a: &[crate::draw_shader_layout::ReflectedIo], b: &[crate::draw_shader_layout::ReflectedIo]| {
        a.len() == b.len() && a.iter().zip(b).all(|(a,b)| a.name == b.name && crate::makepad_script::shader_pack::same_pod_structure(&a.ty, &b.ty))
    };
    let check = || -> Result<(), String> {
        same("code", a.code == b.code, true)?;
        same("flags", a.flags, b.flags)?;
        same("instances", &a.instances, &b.instances)?;
        same("dyn_instances", &a.dyn_instances, &b.dyn_instances)?;
        same("dyn_uniforms", &a.dyn_uniforms, &b.dyn_uniforms)?;
        same("geometries", &a.geometries, &b.geometries)?;
        same(
            "textures",
            a.textures.iter().map(|t| (t.id, t.tex_type)).collect::<Vec<_>>(),
            b.textures.iter().map(|t| (t.id, t.tex_type)).collect::<Vec<_>>(),
        )?;
        same("uniform_buffers", &a.uniform_buffers, &b.uniform_buffers)?;
        same("samplers", &a.samplers, &b.samplers)?;
        same("texture_sampler_indices", &a.texture_sampler_indices, &b.texture_sampler_indices)?;
        same("uses_time", a.uses_time, b.uses_time)?;
        same("rect_pos", a.rect_pos, b.rect_pos)?;
        same("rect_size", a.rect_size, b.rect_size)?;
        same("draw_clip", a.draw_clip, b.draw_clip)?;
        same("draw_depth", a.draw_depth, b.draw_depth)?;
        same("uniform_buffer_bindings", &a.uniform_buffer_bindings, &b.uniform_buffer_bindings)?;
        same("scope_uniforms", &a.scope_uniforms, &b.scope_uniforms)?;
        same("scope_uniform_sources", &a.scope_uniform_sources, &b.scope_uniform_sources)?;
        same("scope_uniforms_buf", &a.scope_uniforms_buf, &b.scope_uniforms_buf)?;
        same(
            "table_consts",
            a.table_consts.iter().map(|t| (t.shader_name, &t.doc, &t.name, t.min, t.max, t.step, t.initial, t.value, t.input)).collect::<Vec<_>>(),
            b.table_consts.iter().map(|t| (t.shader_name, &t.doc, &t.name, t.min, t.max, t.step, t.initial, t.value, t.input)).collect::<Vec<_>>(),
        )?;
        same("geometry_id", a.geometry_id, b.geometry_id)?;
        same("varying_total_slots", a.varying_total_slots, b.varying_total_slots)?;
        same("color_format", a.color_format, b.color_format)?;
        same("blend_op", a.blend_op, b.blend_op)?;
        same("fragment_outputs", a.fragment_outputs, b.fragment_outputs)?;
        same("reflection.instance", same_reflection(&a.reflection.instance, &b.reflection.instance), true)?;
        same("reflection.vertex", same_reflection(&a.reflection.vertex, &b.reflection.vertex), true)?;
        Ok(())
    };
    check().err()
}
