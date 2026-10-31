//! `fragment_depth(@any|@greater|@less)` reaches every backend the shader
//! compiler writes: the depth builtin, its conservative variant where the
//! language has one, and SPIR-V's DepthReplacing execution mode.

use crate::makepad_platform::makepad_script::script_eval;
use crate::makepad_platform::*;

/// A quad writing its own depth in each mode.
fn probe(vm: &mut ScriptVm) {
    crate::script_mod(vm);
    script_eval!(vm, {
        use mod.pod.*
        use mod.math.*
        use mod.shader.*
        mod.draw.DepthProbeAny = mod.draw.DrawQuad{
            depth_out: fragment_depth(@any)
            fragment: fn() {
                self.fb0 = vec4(1.0, 0.0, 0.0, 1.0)
                self.depth_out = self.depth_of(vec4(self.pos.x, self.pos.y, 0.5, 1.0))
            }
        }
        mod.draw.DepthProbeGreater = mod.draw.DepthProbeAny{ depth_out: fragment_depth(@greater) }
        mod.draw.DepthProbeLess = mod.draw.DepthProbeAny{ depth_out: fragment_depth(@less) }
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

fn sources(vm: &mut ScriptVm, mode: &str) -> [String; 5] {
    let v = match mode {
        "any" => [
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeAny, "metal", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeAny, "hlsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeAny, "glsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeAny, "wgsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeAny, "spirv", false)}),
        ],
        "greater" => [
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeGreater, "metal", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeGreater, "hlsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeGreater, "glsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeGreater, "wgsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeGreater, "spirv", false)}),
        ],
        _ => [
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeLess, "metal", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeLess, "hlsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeLess, "glsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeLess, "wgsl", false)}),
            script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DepthProbeLess, "spirv", false)}),
        ],
    };
    v.map(|v| text(vm, v))
}

#[test]
fn fragment_depth_reaches_every_backend() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        probe(vm);
        let cases = [
            ("any", "[[depth(any)]]", "SV_Depth;", None),
            ("greater", "[[depth(greater)]]", "SV_DepthGreaterEqual", Some(14u32)),
            ("less", "[[depth(less)]]", "SV_DepthLessEqual", Some(15u32)),
        ];
        for (mode, metal, hlsl, conservative) in cases {
            let [m, h, g, w, s] = sources(vm, mode);
            for (backend, src) in [("metal", &m), ("hlsl", &h), ("glsl", &g), ("wgsl", &w), ("spirv", &s)] {
                assert!(!src.starts_with("ERRORS"), "{mode}/{backend} did not compile: {src}");
            }
            assert!(m.contains(metal), "{mode}/metal lacks {metal}:\n{m}");
            assert!(h.contains(hlsl), "{mode}/hlsl lacks {hlsl}:\n{h}");
            assert!(g.contains("gl_FragDepth ="), "{mode}/glsl does not write gl_FragDepth:\n{g}");
            // the output starts as the rasterised depth (a path that writes none is a normal draw)
            assert!(g.contains("gl_FragCoord.z"), "{mode}/glsl: no rasterised-depth default:\n{g}");
            assert!(m.contains("position.z"), "{mode}/metal: no rasterised-depth default:\n{m}");
            assert!(!g.contains("in vec4 gl_FragCoord"), "{mode}/glsl declares a builtin input:\n{g}");
            assert!(w.contains("@builtin(frag_depth)"), "{mode}/wgsl lacks frag_depth:\n{w}");
            // OpDecorate (4 words, opcode 71) %v BuiltIn(11) FragDepth(22)
            assert!(spirv_has(&s, &[(4 << 16) | 71, 0, 11, 22]), "{mode}/spirv: no FragDepth builtin");
            // OpExecutionMode (3 words, opcode 16) %main DepthReplacing(12)
            assert!(spirv_has(&s, &[(3 << 16) | 16, 0, 12]), "{mode}/spirv: no DepthReplacing");
            if let Some(c) = conservative {
                assert!(spirv_has(&s, &[(3 << 16) | 16, 0, c]), "{mode}/spirv: no conservative mode {c}");
            }
        }
    });
}
