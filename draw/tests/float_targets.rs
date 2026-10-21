//! Float render targets on the native HAL GPU backend, end to end through the shader DSL:
//! premultiplied-over blending into RGBA16F and the raw-write data mode, one pass with two
//! blended RGBA16F outputs (MRT), and `Cx::read_render_texture` of RGBA16F and RGBA32F targets
//! with values above 1.0. Run with MAKEPAD_HAL_BACKEND=offscreen and the ignored test selection:
//! `cargo test --release -p makepad-draw --test float_targets -- --ignored --nocapture`.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::makepad_script::pod_heap::f16_to_f32;
use makepad_draw::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Every target is SIZE x SIZE texels; the cases are columns of four texels.
const SIZE: usize = 16;

struct Target {
    pass: DrawPass,
    list: DrawList2d,
    textures: Vec<Texture>,
}

impl Target {
    fn new(cx: &mut Cx, name: &str, format: fn(TextureSize) -> TextureFormat, count: usize, clear: Vec4f) -> Self {
        let pass = DrawPass::new_with_name(cx, name);
        pass.set_size(cx, dvec2(SIZE as f64, SIZE as f64));
        let textures = (0..count)
            .map(|_| {
                let texture = Texture::new_with_format(cx, format(TextureSize::Fixed { width: SIZE, height: SIZE }));
                pass.add_color_texture(cx, &texture, DrawPassClearColor::ClearWith(clear));
                texture
            })
            .collect();
        Self { pass, list: DrawList2d::new(cx), textures }
    }

    fn begin(&mut self, cx: &mut Cx2d) {
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.list.begin_always(cx);
        cx.begin_root_turtle(dvec2(SIZE as f64, SIZE as f64), Layout::flow_overlay());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }
}

fn rgba16f(size: TextureSize) -> TextureFormat {
    TextureFormat::RenderRGBAf16 { size, initial: true }
}

fn rgba32f(size: TextureSize) -> TextureFormat {
    TextureFormat::RenderRGBAf32 { size, initial: true }
}

/// Texels `x0..x1` (all rows) in premultiplied `color`.
fn quad(cx: &mut Cx2d, draw: &mut DrawColor, x0: usize, x1: usize, color: [f32; 4]) {
    draw.color = vec4(color[0], color[1], color[2], color[3]);
    draw.draw_abs(cx, Rect { pos: dvec2(x0 as f64, 0.0), size: dvec2((x1 - x0) as f64, SIZE as f64) });
}

struct Scene {
    _window: WindowHandle,
    window_pass: DrawPass,
    root: DrawList2d,
    blend: Target,
    mrt: Target,
    data32: Target,
    /// the default pipeline: premultiplied over where the target blends
    over: DrawColor,
    /// `alpha_blend: false`: raw writes
    raw: DrawColor,
    /// `color_format: @Rgba16F`, otherwise like `over`
    declared16: DrawColor,
    /// two outputs, `fb1` from the `color1` instance
    two: DrawColor,
    /// `color_format: @Rgba32F`
    declared32: DrawColor,
}

impl Scene {
    fn new(cx: &mut Cx) -> Self {
        let [over, raw, declared16, two, declared32] = cx.with_vm(|vm| {
            makepad_platform::script::script_mod(vm);
            makepad_draw::script_mod(vm);
            let over = vm.eval(script! {
                use mod.draw.*
                DrawColor{ pixel: fn(){ return self.color } }
            });
            let raw = vm.eval(script! {
                use mod.draw.*
                DrawColor{ alpha_blend: false pixel: fn(){ return self.color } }
            });
            let declared16 = vm.eval(script! {
                use mod.draw.*
                DrawColor{ color_format: @Rgba16F pixel: fn(){ return self.color } }
            });
            let two = eval_two_output(vm);
            let declared32 = vm.eval(script! {
                use mod.draw.*
                DrawColor{ color_format: @Rgba32F pixel: fn(){ return self.color } }
            });
            [over, raw, declared16, two, declared32].map(|v| DrawColor::script_from_value(vm, v))
        });
        let mut window = WindowHandle::new(cx);
        window.configure_window(cx, dvec2(64.0, 64.0), dvec2(0.0, 0.0), false, "Float targets".into());
        let window_pass = DrawPass::new(cx);
        window.set_pass(cx, &window_pass);
        window_pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
        let zero = vec4(0.0, 0.0, 0.0, 0.0);
        Self {
            _window: window,
            window_pass,
            root: DrawList2d::new(cx),
            blend: Target::new(cx, "float_blend16", rgba16f, 1, zero),
            mrt: Target::new(cx, "float_mrt16", rgba16f, 2, zero),
            data32: Target::new(cx, "float_data32", rgba32f, 1, vec4(7.5, -1.0, 0.0, 2.0)),
            over,
            raw,
            declared16,
            two,
            declared32,
        }
    }

    fn draw(&mut self, cx: &mut Cx2d) {
        cx.begin_pass(&self.window_pass, None);
        self.root.begin_always(cx);
        cx.begin_root_turtle(dvec2(64.0, 64.0), Layout::flow_overlay());

        // RGBA16F, one output. Each column starts from the same half-covering HDR quad.
        self.blend.begin(cx);
        for x in [0, 4, 8] {
            quad(cx, &mut self.over, x, x + 4, [2.0, 0.5, 0.25, 0.5]);
        }
        quad(cx, &mut self.over, 0, 4, [1.0, 1.0, 1.0, 0.5]);
        quad(cx, &mut self.raw, 4, 8, [3.0, 0.25, 0.5, 0.25]);
        quad(cx, &mut self.declared16, 8, 12, [1.0, 1.0, 1.0, 0.5]);
        self.blend.end(cx);

        // Two RGBA16F outputs: overlapping two-output quads, then a one-output quad.
        self.mrt.begin(cx);
        let mut two = |cx: &mut Cx2d, x0, x1, fb0: [f32; 4], fb1: [f32; 4]| {
            self.two.set_dyn_instance(cx, live_id!(color1), &fb1);
            quad(cx, &mut self.two, x0, x1, fb0);
        };
        two(cx, 0, 8, [1.5, 0.0, 0.0, 0.5], [0.0, 4.0, 0.0, 1.0]);
        two(cx, 4, 12, [0.0, 0.0, 2.0, 0.5], [0.0, 0.0, 0.5, 0.25]);
        two(cx, 12, 16, [0.25, 0.25, 0.25, 0.25], [5.0, 0.0, 0.0, 1.0]);
        quad(cx, &mut self.over, 12, 16, [0.0, 1.0, 0.0, 0.5]);
        self.mrt.end(cx);

        // RGBA32F: a value no half float holds, next to the cleared columns.
        self.data32.begin(cx);
        quad(cx, &mut self.declared32, 0, 8, [100000.0, -2.5, 3.25, 0.125]);
        self.data32.end(cx);

        cx.end_pass_sized_turtle();
        self.root.end(cx);
        cx.end_pass(&self.window_pass);
    }
}

/// Writes `color` to output 0 and the `color1` instance to output 1.
fn eval_two_output(vm: &mut ScriptVm) -> ScriptValue {
    vm.eval(script! {
        use mod.pod.*
        use mod.shader.*
        use mod.draw.*
        DrawColor{
            color1: instance(#0000)
            fb1: fragment_output(1, vec4f)
            fragment: fn(){
                self.fb0 = self.color
                self.fb1 = self.color1
            }
        }
    })
}

/// No GPU needed: the generated MSL, HLSL and GLSL ES declare the second colour output.
#[test]
fn a_two_output_shader_declares_both_outputs_for_every_backend() {
    use makepad_draw::makepad_script::{
        shader_backend::ShaderBackend,
        shader_compile::{compile_shader, ShaderCompileOptions},
    };
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        let shader = eval_two_output(vm).as_object().unwrap();
        for (backend, gles, want) in [
            (ShaderBackend::Metal, false, "[[color(1)]]"),
            (ShaderBackend::Hlsl, false, "SV_TARGET1"),
            (ShaderBackend::Glsl, true, "layout(location = 1) out"),
        ] {
            let mut opts = ShaderCompileOptions::new(backend, false, false, false, false);
            opts.gles_complete = gles;
            let output = compile_shader(vm, shader, &opts);
            assert!(!output.has_errors(), "{backend:?}: {}", output.view().error_report);
            let code = if gles { output.view().glsl.unwrap().fragment().to_owned() } else { output.combined().to_owned() };
            assert!(code.contains(want), "{backend:?} code lacks {want}:\n{code}");
        }
    });
}

/// One target read back: its size and texels, or the readback error.
type Texels = Result<(usize, usize, Vec<[f32; 4]>), String>;

fn decode(bytes: &[u8], half: bool) -> Vec<[f32; 4]> {
    if half {
        bytes.as_chunks::<8>().0.iter().map(|t| std::array::from_fn(|c| f16_to_f32(u16::from_le_bytes([t[c * 2], t[c * 2 + 1]])))).collect()
    } else {
        bytes.as_chunks::<16>().0.iter().map(|t| std::array::from_fn(|c| f32::from_le_bytes(t[c * 4..c * 4 + 4].try_into().unwrap()))).collect()
    }
}

/// Every texel of columns `x0..x1` equals `want`.
fn check(name: &str, texels: &[[f32; 4]], x0: usize, x1: usize, want: [f32; 4], failures: &mut Vec<String>) {
    let got = texels[x0];
    let ok = (0..SIZE).all(|y| (x0..x1).all(|x| texels[y * SIZE + x] == want));
    println!("{:<44} {:>9.4?}  {}", format!("{name} [{x0}..{x1}]"), got, if ok { "ok" } else { "MISMATCH" });
    if !ok {
        failures.push(format!("{name} columns {x0}..{x1}: want {want:?}, texel ({x0}, 0) is {got:?}"));
    }
}

#[test]
#[ignore = "requires a native HAL GPU release build"]
fn float_targets_blend_write_two_outputs_and_read_back() {
    use makepad_draw::makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));
    let handler_scene = scene.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            *handler_scene.borrow_mut() = Some(Scene::new(cx));
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            handler_scene.borrow_mut().as_mut().unwrap().draw(&mut cx);
        }
    }))));
    cx.borrow_mut().init_cx_os();

    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "float targets need a GPU backend");
    cx.borrow_mut().hal_attach_session(info);
    println!("backend {} float16 blend targets {}", info.backend, cx.borrow().gpu_info().float16_blend_targets);

    // Paint until no pipeline is compiling, read the four targets, and again until two reads
    // agree: a backend that builds a pipeline for a new target layout on first use (Metal:
    // RGBA16F, two outputs) leaves that draw out until it is ready, then Cx paints again.
    // a turn waits up to 1 ms for the HAL: a minute or more in all
    let mut turns = 0;
    let mut turn = |cx: &mut Cx| {
        turns += 1;
        assert!(turns < 60_000, "timed out");
        assert!(cx.hal_session_turn(1_000_000, true));
    };
    let mut last: Option<Vec<Texels>> = None;
    let mut rounds = 0;
    let read = loop {
        rounds += 1;
        assert!(rounds <= 8, "the targets did not settle");
        let mut cx = cx.borrow_mut();
        let after = cx.frame_submission_serial();
        cx.redraw_all();
        loop {
            turn(&mut cx);
            let submitted = cx.frame_submission_serial();
            if submitted > after && cx.frame_completion_serial() == submitted && cx.pipelines_pending() == 0 {
                break;
            }
        }
        let slots: Arc<Mutex<Vec<Option<Texels>>>> = Arc::new(Mutex::new(vec![None; 4]));
        {
            let scene = scene.borrow();
            let scene = scene.as_ref().unwrap();
            let targets = [(&scene.blend.textures[0], true), (&scene.mrt.textures[0], true), (&scene.mrt.textures[1], true), (&scene.data32.textures[0], false)];
            for (slot, (texture, half)) in targets.into_iter().enumerate() {
                let slots = slots.clone();
                cx.read_render_texture(texture, move |result| {
                    let result = result.map(|(w, h, bytes)| (w, h, decode(&bytes, half))).map_err(|e| format!("{e:?}"));
                    slots.lock().unwrap()[slot] = Some(result);
                })
                .expect("readback request");
            }
        }
        while slots.lock().unwrap().iter().any(Option::is_none) {
            turn(&mut cx);
        }
        let now: Vec<Texels> = slots.lock().unwrap().iter().map(|t| t.clone().unwrap()).collect();
        if last.as_ref() == Some(&now) {
            break now;
        }
        last = Some(now);
    };
    hal.close(info.session);
    println!("two equal reads after {rounds} paints");

    let texels = |slot: usize| -> &Vec<[f32; 4]> {
        let (w, h, texels) = read[slot].as_ref().expect("readback");
        assert_eq!((*w, *h), (SIZE, SIZE));
        texels
    };
    let mut failures = Vec::new();
    let f = &mut failures;
    let blend = texels(0);
    check("RGBA16F over, over (premultiplied over)", blend, 0, 4, [2.0, 1.25, 1.125, 0.75], f);
    check("RGBA16F over, then alpha_blend: false (raw)", blend, 4, 8, [3.0, 0.25, 0.5, 0.25], f);
    let declared = blend[8];
    let declared_blends = declared == [2.0, 1.25, 1.125, 0.75];
    let declared_raw = declared == [1.0, 1.0, 1.0, 0.5];
    println!("{:<44} {:>9.4?}  {}", "RGBA16F over, then color_format: @Rgba16F", declared,
        if declared_blends { "blends" } else if declared_raw { "writes raw" } else { "neither" });
    check("RGBA16F untouched (clear)", blend, 12, 16, [0.0; 4], f);
    let (fb0, fb1) = (texels(1), texels(2));
    check("MRT fb0 first quad", fb0, 0, 4, [1.5, 0.0, 0.0, 0.5], f);
    check("MRT fb0 overlap (blended)", fb0, 4, 8, [0.75, 0.0, 2.0, 0.75], f);
    check("MRT fb0 second quad", fb0, 8, 12, [0.0, 0.0, 2.0, 0.5], f);
    check("MRT fb0 one-output quad over two-output", fb0, 12, 16, [0.125, 1.125, 0.125, 0.625], f);
    check("MRT fb1 first quad", fb1, 0, 4, [0.0, 4.0, 0.0, 1.0], f);
    check("MRT fb1 overlap (blended)", fb1, 4, 8, [0.0, 3.0, 0.5, 1.0], f);
    check("MRT fb1 second quad", fb1, 8, 12, [0.0, 0.0, 0.5, 0.25], f);
    check("MRT fb1 kept under a one-output quad", fb1, 12, 16, [5.0, 0.0, 0.0, 1.0], f);
    let data = texels(3);
    check("RGBA32F @Rgba32F write", data, 0, 8, [100000.0, -2.5, 3.25, 0.125], f);
    check("RGBA32F clear", data, 8, 16, [7.5, -1.0, 0.0, 2.0], f);
    assert!(failures.is_empty(), "{failures:#?}");
}
