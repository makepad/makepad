//! `fragment_position()` reaches every backend the shader compiler writes in one convention:
//! pixels from the target's top-left, window depth, 1 / clip w (frag_position.rs runs it on a
//! GPU). It costs no varying, and a vertex stage cannot read it.

use crate::makepad_platform::makepad_script::script_eval;
use crate::makepad_platform::*;

/// A quad that shows its fragment position, and one that reads it in its vertex stage.
fn probe(vm: &mut ScriptVm) {
    crate::script_mod(vm);
    script_eval!(vm, {
        use mod.pod.*
        use mod.math.*
        use mod.shader.*
        mod.draw.PositionProbe = mod.draw.DrawQuad{
            at: fragment_position()
            pixel: fn() {
                return vec4(self.at.x / 64.0, self.at.y / 64.0, self.at.z, self.at.w)
            }
        }
        mod.draw.PositionPlain = mod.draw.DrawQuad{
            pixel: fn() {
                return vec4(self.pos.x, self.pos.y, 0.0, 1.0)
            }
        }
        mod.draw.PositionInVertex = mod.draw.DrawQuad{
            at: fragment_position()
            vertex: fn() {
                self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size) + self.at
            }
        }
    });
}

fn text(vm: &mut ScriptVm, value: ScriptValue) -> String {
    vm.heap().string_with(value, |_heap, t| t.to_string()).expect("the compiler answers with source")
}

/// The fragment module's words contain `words` (`words[1]`, an id, matches anything).
fn spirv_has(hex: &str, words: &[u32]) -> bool {
    let f = hex.split("\nf:").nth(1).unwrap_or("");
    let w: Vec<u32> = (0..f.len() / 8).map(|i| u32::from_str_radix(&f[i * 8..i * 8 + 8], 16).unwrap_or(0)).collect();
    w.windows(words.len()).any(|s| s[0] == words[0] && s[2..] == words[2..])
}

/// The probe's sources: metal, hlsl, glsl, wgsl, spirv, wgsl_xr, spirv_xr.
fn sources(vm: &mut ScriptVm) -> [String; 7] {
    [
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "metal", false)}),
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "hlsl", false)}),
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "glsl", false)}),
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "wgsl", false)}),
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "spirv", false)}),
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "wgsl_xr", false)}),
        script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "spirv_xr", false)}),
    ]
    .map(|v| text(vm, v))
}

/// OpDecorate (4 words, opcode 71) %v BuiltIn(11) FragCoord(15)
const SPIRV_FRAG_COORD: [u32; 4] = [(4 << 16) | 71, 0, 11, 15];
/// OpDecorate %v BuiltIn(11) ViewIndex(4440)
const SPIRV_VIEW_INDEX: [u32; 4] = [(4 << 16) | 71, 0, 11, 4440];

#[test]
fn fragment_position_reaches_every_backend() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        probe(vm);
        let [m, h, g, w, s, wx, sx] = sources(vm);
        for (backend, src) in [("metal", &m), ("hlsl", &h), ("glsl", &g), ("wgsl", &w), ("spirv", &s), ("wgsl_xr", &wx), ("spirv_xr", &sx)] {
            assert!(!src.starts_with("ERRORS"), "{backend} did not compile: {src}");
        }
        // the builtin as it is (Metal, WebGPU, Vulkan), into the shader's own global
        assert!(m.contains("_mp.frag_position = input.position;"), "metal: no fragment position:\n{m}");
        assert!(w.contains("@builtin(position)") && w.contains("frag_position = in.position;"), "wgsl: no fragment position:\n{w}");
        assert!(spirv_has(&s, &SPIRV_FRAG_COORD), "spirv: no FragCoord builtin");
        // D3D's SV_Position.w is the clip w: the shader reads its reciprocal
        assert!(h.contains("frag_position = input.position;"), "hlsl: no fragment position:\n{h}");
        assert!(h.contains("frag_position.w = (1.0 / frag_position.w);"), "hlsl: w is not the reciprocal of SV_Position.w:\n{h}");
        // GL counts rows from the bottom: the host's row origin turns them top-down
        assert!(g.contains("uniform vec2 mp_frag_y;"), "glsl: no row origin:\n{g}");
        assert!(g.contains("frag_position = gl_FragCoord;"), "glsl: no fragment position:\n{g}");
        assert!(g.contains("frag_position.y = (mp_frag_y.x + (mp_frag_y.y * frag_position.y));"), "glsl: the row is not turned:\n{g}");
        assert!(!g.contains("in vec4 gl_FragCoord"), "glsl declares a builtin input:\n{g}");
        // per eye in XR: the multiview variants read the position beside the view index
        assert!(wx.contains("@builtin(view_index)") && wx.contains("frag_position = in.position;"), "wgsl_xr: no per-eye position:\n{wx}");
        assert!(spirv_has(&sx, &SPIRV_FRAG_COORD) && spirv_has(&sx, &SPIRV_VIEW_INDEX), "spirv_xr: no per-eye FragCoord");
        // the CPU path's host fills it
        assert!(
            script_eval!(vm, {mod.shader.test_compile_draw_rust_contains(mod.draw.PositionProbe, "rcx.frag_position")}).as_bool() == Some(true),
            "rust: the fragment does not read rcx.frag_position"
        );
    });
}

#[test]
fn fragment_position_costs_no_varying() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        probe(vm);
        // the same quad without it has the same varyings: Metal's VertexOut
        let with = { let v = script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionProbe, "metal", false)}); text(vm, v) };
        let without = { let v = script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionPlain, "metal", false)}); text(vm, v) };
        let vertex_out = |src: &str| src.split("struct VertexOut {").nth(1).and_then(|s| s.split("};").next()).unwrap_or("").to_string();
        assert!(!vertex_out(&with).is_empty(), "metal: no VertexOut:\n{with}");
        assert_eq!(vertex_out(&with), vertex_out(&without), "fragment_position() added a varying");
        // GL only reads gl_FragCoord for a shader that asks
        let plain = { let v = script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionPlain, "glsl", false)}); text(vm, v) };
        assert!(!plain.contains("mp_frag_y"), "glsl: a row origin without fragment_position():\n{plain}");
    });
}

#[test]
fn a_vertex_stage_cannot_read_fragment_position() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        probe(vm);
        let src = { let v = script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.PositionInVertex, "metal", false)}); text(vm, v) };
        assert!(src.starts_with("ERRORS") && src.contains("fragment_position()"), "a vertex read of fragment_position() compiled:\n{src}");
    });
}
