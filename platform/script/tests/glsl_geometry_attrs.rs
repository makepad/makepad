//! GL ES vertex inputs: a geometry buffer in a compact format (f16 pairs,
//! unorm8x4 colours) is read through typed attributes (`geom_*`) assigned
//! into the buffer's struct; a plain f32 geometry buffer stays packed in
//! vec4 attributes (`packed_geometry_*`).
use makepad_script::*;

fn compile(vertex_struct: &str, vertex_body: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet vd = struct{{\n{vertex_struct}\n}}\nlet sh = {{\ngeom: shader.vertex_buffer(vd)\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n\
         vertex: fn() {{ {vertex_body} }}\nfragment: fn() {{ self.pixel = vec4(1.0) }}\n}}\nshader.test_compile_draw_source(sh, \"glsl\", false)"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, &code, &[])));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap()
}

#[test]
fn compact_geometry_uses_typed_attributes_and_plain_geometry_stays_packed() {
    let src = compile("off: f16x2,\ncolor: unorm8x4,", "self.vertex_pos = vec4(self.geom.off.x, self.geom.off.y, self.geom.color.x, 1.0)");
    assert!(src.contains("in vec2 geom_off;"), "{src}");
    assert!(src.contains("in vec4 geom_color;"), "{src}");
    assert!(!src.contains("packed_geometry_"), "{src}");

    let src = compile("a: vec4f,\nb: vec4f,\nc: vec4f,", "self.vertex_pos = self.geom.a + self.geom.b + self.geom.c");
    for k in 0..3 {
        assert!(src.contains(&format!("in vec4 packed_geometry_{k};")), "{src}");
    }
    assert!(!src.contains("geom_a"), "{src}");
}
