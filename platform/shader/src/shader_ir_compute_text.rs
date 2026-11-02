//! Compute resource declarations and entry points for Metal and D3D11.
use crate::shader_ir::*;
use crate::shader_ir_compute::*;
use crate::shader_ir_text::*;
use std::fmt::Write;

pub fn compute_text(m: &IrModule, entry: FuncId, layout: &ComputeLayout) -> Result<String, String> {
    let metal = layout.target == ComputeTarget::Msl;
    let mut cx = TextCx::new(m, if metal { Dialect::Metal } else { Dialect::Hlsl }, false);
    cx.compute = Some(layout);
    let mut out = String::new();
    if metal {
        out.push_str("#include <metal_stdlib>\nusing namespace metal;\n");
        if layout.precise_math { out.push_str("#pragma clang fp contract(off)\n"); }
    }
    let mut fields = String::from("    uint _mp_iter;\n");
    let mut resources = Vec::new();
    let mut fill = String::new();
    if metal && layout.param_binding.is_some() {
        fields.push_str("    constant uint *kp;\n");
        resources.push(format!("constant uint *kp [[buffer({})]]", layout.param_binding.unwrap()));
        fill.push_str("    _mp.kp = kp;\n");
    } else if !metal {
        if layout.param_binding.is_some() {
            writeln!(out, "cbuffer KernelParams : register(b0) {{ uint4 kp[{}]; }}", layout.param_binding_size / 16).ok();
        }
        out.push_str("static uint _mp_iter;\n");
    }
    for b in &layout.buffers {
        let g = &m.globals[b.global as usize];
        let IrTy::Array(elem, _) = m.get(g.ty) else { unreachable!() };
        let name = cx.plain_global_name(b.global);
        if metal {
            let decl = format!("{}device {} *{name}", if b.writable { "" } else { "const " }, cx.ty_name(*elem));
            writeln!(fields, "    {decl};").ok();
            resources.push(format!("{decl} [[buffer({})]]", b.index));
            writeln!(fill, "    _mp.{name} = {name};").ok();
        } else {
            hlsl_buffer_declaration(&cx, b, &mut out);
        }
    }
    for (i, g) in m.globals.iter().enumerate() {
        if g.kind == IrGlobalKind::Io(IrIo::Workgroup) {
            let name = cx.plain_global_name(i as GlobalId);
            let IrTy::Array(elem, len) = m.get(g.ty) else { unreachable!() };
            if metal {
                writeln!(fields, "    threadgroup {} *{name};", cx.ty_name(*elem)).ok();
                writeln!(fill, "    threadgroup {} {name}[{len}];\n    _mp.{name} = {name};", cx.ty_name(*elem)).ok();
            } else {
                writeln!(out, "groupshared {} {name}[{len}];", cx.ty_name(*elem)).ok();
            }
        }
    }
    for tex in &layout.storage_textures {
        let name = cx.plain_global_name(tex.global);
        let scalar = if tex.format.scalar() == IrScalar::U32 { "uint" } else { "float" };
        if metal {
            let decl = format!("texture2d<{scalar}, access::write> {name}");
            writeln!(fields, "    {decl};").ok();
            resources.push(format!("{decl} [[texture({})]]", tex.index));
            writeln!(fill, "    _mp.{name} = {name};").ok();
        } else {
            let unorm = if matches!(tex.format, StorageTextureFormat::R8unorm | StorageTextureFormat::Rgba8unorm) { "unorm " } else { "" };
            let lanes = if tex.format.channels() == 4 { "4" } else { "" };
            writeln!(out, "RWTexture2D<{unorm}{scalar}{lanes}> {name} : register(u{});", tex.index).ok();
        }
    }
    for (name, ty, attr, semantic) in [
        ("global_id", "uint3", "thread_position_in_grid", "SV_DispatchThreadID"),
        ("local_id", "uint3", "thread_position_in_threadgroup", "SV_GroupThreadID"),
        ("group_id", "uint3", "threadgroup_position_in_grid", "SV_GroupID"),
        ("local_index", "uint", "thread_index_in_threadgroup", "SV_GroupIndex"),
        ("group_count", "uint3", "threadgroups_per_grid", ""),
        ("subgroup_lane", "uint", "thread_index_in_simdgroup", ""),
        ("subgroup_id", "uint", "simdgroup_index_in_threadgroup", ""),
        ("subgroup_size", "uint", "threads_per_simdgroup", ""),
    ] {
        if metal {
            writeln!(fields, "    {ty} _{name};").ok();
            resources.push(format!("{ty} _{name} [[{attr}]]"));
            writeln!(fill, "    _mp._{name} = _{name};").ok();
        } else if !semantic.is_empty() {
            writeln!(out, "static {ty} _mp_{name};").ok();
            resources.push(format!("{ty} _{name} : {semantic}"));
            writeln!(fill, "    _mp_{name} = _{name};").ok();
        }
    }
    if metal {
        writeln!(out, "struct Mp {{\n{fields}}};").ok();
        out.push_str(crate::shader_ir_metal::METAL_HELPERS);
        out.push_str("inline uint _mp_pack_half2(float2 x) { return as_type<uint>(half2(x)); }\n");
    } else {
        out.push_str(crate::shader_ir_hlsl::HLSL_HELPERS);
        out.push_str("uint _mp_pack_half2(float2 x) { return f32tof16(x.x) | (f32tof16(x.y) << 16u); }\n");
    }
    out.push_str("uint _mp_extract_bits(uint x, uint off, uint count) { if (off >= 32u || count == 0u) return 0u; uint n = min(count, 32u - off); return (x >> off) & (0xffffffffu >> (32u - n)); }\n");
    out.push_str("uint _mp_dot4_u8(uint a, uint b) { return (a & 255u)*(b & 255u) + ((a >> 8u)&255u)*((b >> 8u)&255u) + ((a >> 16u)&255u)*((b >> 16u)&255u) + (a >> 24u)*(b >> 24u); }\n");
    for f in callees_first(m, &[entry]) {
        if f != entry { cx.function(f, &mut out); }
    }
    if !metal {
        writeln!(out, "[numthreads({}, {}, {})]", layout.workgroup[0], layout.workgroup[1], layout.workgroup[2]).ok();
    }
    writeln!(out, "{}void kernel_main({}) {{", if metal { "kernel " } else { "" }, resources.join(",\n    ")).ok();
    if metal { out.push_str("    Mp _mp;\n    _mp._mp_iter = 0u;\n"); }
    else { out.push_str("    _mp_iter = 0u;\n"); }
    out.push_str(&fill);
    cx.stmts(&m.functions[entry as usize], &m.functions[entry as usize].body, 1, &mut out);
    out.push_str("}\n");
    Ok(out)
}

/// Raw-buffer helpers also serve draw shaders; half stores preserve the
/// neighbouring half with compare/exchange (two invocations may share a word).
pub fn hlsl_buffer_declaration(cx: &TextCx, b: &ComputeBufferBinding, out: &mut String) {
    let name = cx.plain_global_name(b.global);
    writeln!(out, "{}ByteAddressBuffer {name} : register({}{});", if b.writable { "RW" } else { "" }, if b.writable { "u" } else { "t" }, b.index).ok();
    if b.scalar == IrScalar::F16 {
        writeln!(out, "float _mp_load_half_{name}(uint off) {{ return f16tof32(({name}.Load(off & ~3u) >> ((off & 2u)*8u)) & 65535u); }}").ok();
        if b.writable {
            writeln!(out, "void _mp_store_half_{name}(uint off, float value) {{ uint shift = (off & 2u)*8u; uint mask = 65535u << shift; uint bits = f32tof16(value) << shift; uint expected = {name}.Load(off & ~3u); uint old; [allow_uav_condition] while (true) {{ {name}.InterlockedCompareExchange(off & ~3u, expected, (expected & ~mask) | bits, old); if (old == expected) break; expected = old; }} }}").ok();
        }
    }
    if b.writable && matches!(b.scalar, IrScalar::U32 | IrScalar::I32) {
        let ty = if b.scalar == IrScalar::I32 { "int" } else { "uint" };
        let result = if b.scalar == IrScalar::I32 { "asint(old)" } else { "old" };
        writeln!(out, "{ty} _mp_atomic_{name}(uint off, {ty} value) {{ uint old; {name}.InterlockedAdd(off, asuint(value), old); return {result}; }}").ok();
    }
}
