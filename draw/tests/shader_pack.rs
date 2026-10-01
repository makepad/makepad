//! Shader packs: the draw shaders here go compile -> pack bytes -> mapping
//! and come back equal to the compile, field by field; their keys do not
//! depend on what a process loaded before them; recording through `Cx`
//! gives entries that install and are found by the same keys.

use makepad_draw::makepad_platform::shader_pack::*;
use makepad_draw::*;

const LIBRARY: [&str; 4] = ["DrawQuad", "DrawColor", "DrawText", "DrawSvg"];

/// A document drawing with a helper from its own scope, a scope value (a
/// scope uniform), a uniform and a texture.
fn document(helper_body: &str) -> String {
    format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw.*\n\
         let STRENGTH = 0.8\n\
         let glow = fn(d: float) -> float {{ {helper_body} }}\n\
         DrawQuad{{\n\
             amount: uniform(0.5)\n\
             image: texture_2d(float)\n\
             pixel: fn() {{\n\
                 let d = length(self.pos - vec2(0.5))\n\
                 let t = self.image.sample(self.pos)\n\
                 return mix(#000, #3080ff, glow(d) * self.amount * STRENGTH) + t * 0.1\n\
             }}\n\
         }}\n"
    )
}

fn eval(vm: &mut ScriptVm, file: &str, code: String) -> ScriptObject {
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.eval(ScriptMod { file: file.into(), code, ..Default::default() });
    let errors = vm.take_errors();
    assert!(errors.is_empty() && !value.is_err(), "{file} did not evaluate: {errors:?}");
    value.as_object().unwrap()
}

fn library_shader(vm: &mut ScriptVm, name: &str) -> ScriptObject {
    eval(vm, &format!("pack://{name}"), format!("use mod.draw.*\n{name}{{}}\n"))
}

#[test]
fn shaders_round_trip_through_a_pack() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        for name in LIBRARY {
            let shader = library_shader(vm, name);
            verify_shader_pack_entry(vm, shader).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let shader = eval(vm, "pack://document", document("return exp(-d * 4.0)"));
        verify_shader_pack_entry(vm, shader).unwrap_or_else(|e| panic!("document: {e}"));
    });
}

/// Every shader's key, in a VM that loaded other code first (shifting heap
/// indices and code positions) and took the shaders in reverse order.
fn keys(other_code_first: bool) -> Vec<u64> {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        if other_code_first {
            for i in 0..3 {
                eval(vm, &format!("pack://other{i}"), format!("use mod.pod.*\nuse mod.math.*\nlet f{i} = fn(x: float) -> float {{ return x * {i}.0 }}\nlet o{i} = {{a: {i}}}\no{i}\n"));
            }
        }
        makepad_draw::script_mod(vm);
        let mut names: Vec<&str> = LIBRARY.to_vec();
        if other_code_first {
            names.reverse();
        }
        let mut keys: Vec<(&str, u64)> = Vec::new();
        let doc = eval(vm, "pack://document", document("return exp(-d * 4.0)"));
        for name in names {
            let shader = library_shader(vm, name);
            keys.push((name, shader_pack_key(vm, shader, false)));
        }
        keys.sort();
        let mut keys: Vec<u64> = keys.into_iter().map(|(_, k)| k).collect();
        keys.push(shader_pack_key(vm, doc, false));
        keys
    })
}

#[test]
fn keys_do_not_depend_on_load_order() {
    let a = keys(false);
    let b = keys(true);
    assert_eq!(a, b);
    let mut unique = a.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), a.len(), "two shaders share a key");
}

#[test]
fn keys_follow_the_code_a_shader_calls() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        let a = eval(vm, "pack://a", document("return exp(-d * 4.0)"));
        let b = eval(vm, "pack://b", document("return exp(-d * 4.0)"));
        let c = eval(vm, "pack://c", document("return exp(-d * 8.0)"));
        // Same text: one key. A helper with another body: another key,
        // though the pixel function is the same text.
        assert_eq!(shader_pack_key(vm, a, false), shader_pack_key(vm, b, false));
        assert_ne!(shader_pack_key(vm, a, false), shader_pack_key(vm, c, false));
        assert_ne!(shader_pack_key(vm, a, false), shader_pack_key(vm, a, true));
    });
}

#[test]
fn a_recorded_pack_installs_by_key() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.record_shader_pack(true);
    let (shaders, keys) = cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        let doc = eval(vm, "pack://document", document("return exp(-d * 4.0)"));
        let quad = library_shader(vm, "DrawQuad");
        let text = library_shader(vm, "DrawText");
        let shaders = (DrawQuad::script_from_value(vm, doc.into()), DrawQuad::script_from_value(vm, quad.into()), DrawText::script_from_value(vm, text.into()));
        let keys = [doc, quad, text].map(|s| shader_pack_key(vm, s, false));
        (shaders, keys)
    });
    let pack = cx.take_shader_pack();
    let entries = read_shader_pack(&pack).unwrap();
    for key in keys {
        assert_eq!(entries.iter().filter(|e| e.key == key).count(), 1, "{key:016x} was not recorded once");
    }
    // The document's scope value is found again by a path of names.
    let doc = entries.iter().find(|e| e.key == keys[0]).unwrap();
    assert!(
        doc.scope_uniforms.iter().any(|su| matches!(&su.source, PackScopeSource::Path { key, .. } if *key == LiveId::from_str("STRENGTH").0)),
        "{:?}",
        doc.scope_uniforms
    );
    assert_eq!(cx.install_shader_pack(pack.clone()).unwrap(), entries.len());
    for entry in &entries {
        assert_eq!(cx.draw_shaders.packs.entry(entry.key).as_ref(), Some(entry));
    }
    // Packs of another version, or not packs, are refused.
    assert!(cx.install_shader_pack(b"MPSP\x63\0\0\0".to_vec()).is_err());
    assert!(cx.install_shader_pack(pack[..pack.len() / 2].to_vec()).is_err());
    drop(shaders);
}
