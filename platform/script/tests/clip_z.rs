//! One clip-z convention on every backend: clip z runs 0..w and a depth
//! buffer holds ndc z (Metal, D3D, Vulkan, WebGPU natively); the OpenGL
//! backend moves its -w..w clip z there in the vertex output. The depth clip
//! compares a fragment's ndc depth with the depth picture in that convention.
use makepad_script::*;

fn compile(backend: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nuse mod.shader.depth_clip\nlet pass = struct{{depth_projection: mat4x4f, depth_view: mat4x4f}}\nlet sh = {{\nvertex_pos: shader.vertex_position(vec4f)\ndraw_pass: shader.uniform_buffer(pass)\npixel: shader.fragment_output(0, vec4f)\n\
         vertex: fn() {{ self.vertex_pos = vec4(0.25, 0.5, 0.75, 1.0) }}\nfragment: fn() {{ self.pixel = depth_clip(vec4(0.25, 0.5, 0.75, 1.0), vec4(1.0), 1.0) }}\n}}\nshader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, &code, &[])));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{backend}: {errors:?}");
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap()
}

#[test]
fn one_clip_z_convention_and_an_empty_depth_texel_never_clips() {
    let glsl = compile("glsl");
    assert!(glsl.contains("((2.0 * vtx_pos.z) - vtx_pos.w)"), "{glsl}");
    for backend in ["metal", "hlsl", "wgsl"] {
        let src = compile(backend);
        assert!(!src.contains("2.0 * vtx_pos.z"), "{backend} keeps its native 0..w clip z:\n{src}");
    }
    // The depth clip (the SPIR-V helper Vulkan runs) keeps a fragment when the depth texel is
    // at least its depth clamped to 1: past the far plane only a nearer texel clips it, never
    // the empty 1.0 (a sky pinned to depth 1 whose world lies past far stays drawn).
    let spirv = compile("spirv");
    let hex = spirv.split("\nf:").nth(1).unwrap_or_else(|| panic!("no fragment words:\n{spirv}"));
    let words: Vec<u32> = (0..hex.len() / 8).map(|i| u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).unwrap()).collect();
    let mut ops = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let n = ((words[i] >> 16) as usize).max(1);
        ops.push((words[i] & 0xffff, &words[i + 1..(i + n).min(words.len())]));
        i += n;
    }
    // OpConstant 1.0, OpExtInst GLSL.std.450 FMin taking it, OpFOrdGreaterThanEqual(texel, that min)
    let one: Vec<u32> = ops.iter().filter(|(op, a)| *op == 43 && a.len() == 3 && a[2] == 1.0f32.to_bits()).map(|(_, a)| a[1]).collect();
    let clamped: Vec<u32> = ops.iter().filter(|(op, a)| *op == 12 && a.len() > 4 && a[3] == 37 && a[4..].iter().any(|x| one.contains(x))).map(|(_, a)| a[1]).collect();
    assert!(
        ops.iter().any(|(op, a)| *op == 190 && a.len() == 4 && clamped.contains(&a[3])),
        "the depth clip compares the texel with the unclamped fragment depth ({} min(_, 1.0) found)",
        clamped.len()
    );
    // WGSL writes the helper out with the module that calls it, with the same compare
    let wgsl = compile("wgsl");
    let helper = &wgsl[wgsl.find("fn depth_clip(").unwrap_or_else(|| panic!("WGSL calls depth_clip without the helper:\n{wgsl}"))..];
    let helper = &helper[..helper.find("\n}").unwrap_or(helper.len())];
    let keep = helper.lines().find(|l| l.contains(">=")).unwrap_or_else(|| panic!("no keep compare in the WGSL depth clip:\n{helper}"));
    assert!(keep.contains("min("), "the WGSL depth clip compares the unclamped fragment depth:\n{helper}");
}
