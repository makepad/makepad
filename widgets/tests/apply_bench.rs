//! Native timing of the derived `script_apply` on real widgets (rustc, no Relax): (a) the full
//! document apply of a fresh widget, (b) a small animator-sized block (`{height: 0}`) applied
//! to a big widget, (c) a hot-reload re-apply (`Apply::Reload`) of the whole document.
//! Prints the median of 15 batches per case, in microseconds per apply.
//! Run: cargo test --release -p makepad-widgets --test apply_bench -- --ignored --nocapture

use makepad_widgets::makepad_script::script;
use makepad_widgets::*;
use std::time::Instant;

fn median_us(cx: &mut Cx, iters: usize, mut f: impl FnMut(&mut Cx)) -> f64 {
    for _ in 0..iters {
        f(cx);
    }
    let mut v: Vec<f64> = (0..15)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..iters {
                f(cx);
            }
            t.elapsed().as_secs_f64() * 1e6 / iters as f64
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[7]
}

#[test]
#[ignore]
fn apply_bench() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| makepad_widgets::script_mod(vm));
    let docs: Vec<(&str, ScriptValue)> = vec![
        (
            "Button",
            cx.with_vm(|vm| {
                vm.eval(script! {
                    use mod.prelude.widgets.*
                    Button{ text: "Hello" }
                })
            }),
        ),
        (
            "View (big)",
            cx.with_vm(|vm| {
                vm.eval(script! {
                    use mod.prelude.widgets.*
                    View{
                        width: Fill height: Fit flow: Down spacing: 4 padding: 8
                        show_bg: true draw_bg.color: #x334
                        a := Label{ text: "a" } b := Button{ text: "b" } c := Slider{ text: "c" }
                        d := Label{ text: "d" } e := Button{ text: "e" } f := Slider{ text: "f" }
                        g := View{ flow: Right h := Label{ text: "h" } i := Button{ text: "i" } j := CheckBox{ text: "j" } }
                        k := View{ flow: Right l := Label{ text: "l" } m := Button{ text: "m" } n := CheckBox{ text: "n" } }
                        o := TextInput{ text: "o" } p := DropDown{ labels: ["x" "y" "z"] } q := Label{ text: "q" }
                        r := Button{ text: "r" } s := Slider{ text: "s" } t := Label{ text: "t" }
                    }
                })
            }),
        ),
        (
            "Slider",
            cx.with_vm(|vm| {
                vm.eval(script! {
                    use mod.prelude.widgets.*
                    Slider{ text: "Gain" min: 0.0 max: 10.0 }
                })
            }),
        ),
    ];
    let small = cx.with_vm(|vm| {
        vm.eval(script! {
            {height: 0}
        })
    });
    for (name, doc) in docs {
        // (a) full document apply: a fresh widget from the document
        let a = median_us(&mut cx, 200, |cx| {
            let w = cx.with_vm(|vm| WidgetRef::script_from_value(vm, doc));
            drop(w);
        });
        let mut widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, doc));
        // (b) a small state block on the built widget
        let b = median_us(&mut cx, 2000, |cx| {
            cx.with_vm(|vm| widget.script_apply(vm, &Apply::Animate, &mut Scope::empty(), small));
        });
        // (c) hot reload: the whole document re-applied
        let c = median_us(&mut cx, 200, |cx| {
            cx.with_vm(|vm| widget.script_apply(vm, &Apply::Reload, &mut Scope::empty(), doc));
        });
        println!("apply_bench {name:<12} (a) new {a:9.2} us  (b) small block {b:8.3} us  (c) reload {c:9.2} us");
    }
}

/// Native timing of one draw-shader compile as a platform does it: compile_shader for each
/// backend's options, the draw description, and a read of every io row.
/// Run: cargo test --release -p makepad-widgets --test apply_bench shader_bench -- --ignored --nocapture
#[test]
#[ignore]
fn shader_bench() {
    use makepad_widgets::makepad_script::shader_backend::ShaderBackend;
    use makepad_widgets::makepad_script::shader_compile::{compile_shader, ShaderCompileOptions};
    use makepad_widgets::makepad_script::shader_reflection::DrawShaderDesc;
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| makepad_widgets::script_mod(vm));
    let shaders: Vec<(&str, ScriptObject)> = ["Button", "Slider", "CheckBox"]
        .iter()
        .map(|name| {
            let w = cx.with_vm(|vm| match *name {
                "Button" => vm.eval(script! { use mod.prelude.widgets.* Button{} }),
                "Slider" => vm.eval(script! { use mod.prelude.widgets.* Slider{} }),
                _ => vm.eval(script! { use mod.prelude.widgets.* CheckBox{} }),
            });
            let o = w.as_object().unwrap();
            let bg = cx.with_vm(|vm| vm.bx.heap.value(o, id!(draw_bg).into(), NoTrap)).as_object().unwrap();
            (*name, bg)
        })
        .collect();
    for (name, shader) in shaders {
        for backend in [ShaderBackend::Glsl, ShaderBackend::Metal, ShaderBackend::Hlsl] {
            let opts = ShaderCompileOptions::new(backend, false, false, false, false);
            let mut sink = 0usize;
            let t = median_us(&mut cx, 20, |cx| {
                cx.with_vm(|vm| {
                    let out = compile_shader(vm, shader, &opts);
                    let desc = DrawShaderDesc::from_output(vm.heap(), &out);
                    sink += out.combined().len() + out.glsl().map_or(0, |g| g.vertex().len()) + out.table_consts().len();
                    for io in desc.io() {
                        sink += io.name().0 as usize & 1 + io.ty().slots() + io.block_name().len() + io.sampler();
                    }
                    sink += desc.scope_uniforms().len() + desc.samplers().len();
                })
            });
            println!("shader_bench {name:<9} {backend:?}: {t:9.1} us per compile+describe ({sink})");
        }
    }
}
