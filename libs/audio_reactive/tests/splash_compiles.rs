//! The Splash fields a runtime shader includes compile on every backend
//! the shader compiler writes, in a draw that reads every helper.

use makepad_draw::*;

#[test]
fn the_splash_fields_compile_everywhere() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        for backend in ["metal", "hlsl", "glsl", "wgsl"] {
            let code = format!(
                "use mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw\nlet sh = mod.draw.DrawQuad{{\n{}\n        pixel: fn() {{\n            let s = self.audio_fft(self.pos.x, self.pos.y) + self.audio_wave(self.pos.x) + self.audio_wave2(self.pos.y).x\n            return vec4(s, self.audio_hit.x + self.audio_norm.y, self.audio_env.z + self.audio_form.x, 1.0)\n        }}\n}}\nmod.shader.test_compile_draw_source(sh, \"{backend}\", false)\n",
                makepad_audio_reactive::SPLASH
            );
            vm.bx.captured_errors = Some(Vec::new());
            let v = vm.eval(ScriptMod { file: format!("audio/{backend}"), code, ..Default::default() });
            let errors = vm.take_errors();
            assert!(errors.is_empty() && !v.is_err(), "{backend}: {errors:?}");
            let text = vm.bx.heap.string_with(v, |_, s| s.to_string()).unwrap_or_default();
            assert!(!text.starts_with("ERRORS"), "{backend}: {text}");
        }
    });
}
