//! The moved crate renders exactly the bands the example rendered before the move: `bands_match_the_pre_move_baseline`
//! compares them with the ones recorded by the example's code (`local/agent_state/hdri/skyline-baseline`, Task 1 step 1; `local/`
//! isn't committed, so a machine without it skips). The baseline names: the defaults (seed 7) and the reference presets props_heavy and offcentre.
//!   `MAKEPAD_HAL_BACKEND=offscreen cargo test --release -p makepad-skyline --test band -- --include-ignored --nocapture`
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{export::Band, params::Params, render};
use makepad_widgets::makepad_draw::{self, *};
use std::{cell::RefCell, rc::Rc, sync::mpsc};

struct Rig {
    _window: WindowHandle, window_pass: DrawPass, root: DrawList2d, sky: render::Skyline, next: NextFrame, more: bool, draws: usize,
}

fn register(vm: &mut ScriptVm) {
    makepad_platform::script::script_mod(vm);
    makepad_draw::script_mod(vm);
    makepad_skyline::script_mod(vm);
}

fn render_band(p: &Params, w: usize) -> Band {
    use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
    let rig: Rc<RefCell<Option<Rig>>> = Rc::new(RefCell::new(None));
    let handler = rig.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let sky = cx.with_vm(|vm| {
                register(vm);
                let v = vm.eval(script! { mod.draw.Skyline{} });
                render::Skyline::script_from_value(vm, v)
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(64.0, 64.0), dvec2(0.0, 0.0), false, "Skyline band".into());
            let window_pass = DrawPass::new(cx);
            window.set_pass(cx, &window_pass);
            *handler.borrow_mut() = Some(Rig { _window: window, window_pass, root: DrawList2d::new(cx), sky, next: NextFrame::default(), more: false, draws: 0 });
            cx.redraw_all();
        }
        let mut rig = handler.borrow_mut();
        let Some(r) = rig.as_mut() else { return };
        r.sky.job_event(cx, event);
        if r.next.is_event(event).is_some() { cx.redraw_all(); }
        if let Event::Draw(event) = event {
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&r.window_pass, None);
            r.root.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(64.0, 64.0), Layout::flow_overlay());
            r.more = r.sky.draw_job(&mut cx);
            r.draws += 1;
            if r.more { r.next = cx.new_next_frame(); }
            cx.end_pass_sized_turtle();
            r.root.end(&mut cx);
            cx.end_pass(&r.window_pass);
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "the band needs a GPU backend");
    cx.borrow_mut().hal_attach_session(info);
    while rig.borrow().is_none() { assert!(cx.borrow_mut().hal_session_turn(1_000_000, true)); }
    // Pipelines compile on first use and the first draws into a new layout can be skipped, so the band is
    // requested again until two successive bands agree (the first ones may fail or come back partial).
    let mut last: Option<Band> = None;
    let mut round = 0;
    let band = loop {
        round += 1;
        assert!(round <= 8, "the band never settled");
        let (tx, rx) = mpsc::channel();
        {
            let mut c = cx.borrow_mut();
            let mut r = rig.borrow_mut();
            r.as_mut().unwrap().sky.request_band(&mut c, p, w, Box::new(move |b| { let _ = tx.send(b); }));
            c.redraw_all();
        }
        let t0 = Cx::monotonic_now();
        let got = 'job: loop {
            // quiet: every submitted frame complete
            loop {
                assert!(cx.borrow_mut().hal_session_turn(1_000_000, true));
                let mut c = cx.borrow_mut();
                let submitted = c.frame_submission_serial();
                if c.frame_completion_serial() == submitted && c.pipelines_pending() == 0 { break; }
            }
            if let Ok(b) = rx.try_recv() { break 'job b; }
            let (draws, after) = (rig.borrow().as_ref().unwrap().draws, cx.borrow_mut().frame_submission_serial());
            cx.borrow_mut().redraw_all();
            loop {
                assert!(cx.borrow_mut().hal_session_turn(1_000_000, true));
                let mut c = cx.borrow_mut();
                let submitted = c.frame_submission_serial();
                if rig.borrow().as_ref().unwrap().draws > draws && submitted > after && c.frame_completion_serial() == submitted && c.pipelines_pending() == 0 { break; }
                drop(c);
                assert!(Cx::monotonic_now() - t0 < 300.0, "band timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
            assert!(Cx::monotonic_now() - t0 < 300.0, "band timed out");
        };
        let Ok(b) = got else { last = None; continue }; // the warm-up rounds can fail
        if last.as_ref().is_some_and(|l| (l.w, l.h) == (b.w, b.h) && l.texels == b.texels) { break b; }
        last = Some(b);
    };
    hal.close(info.session);
    band
}

/// The recorded band and its parameters, or None when `local/` has no baseline `name`.
fn baseline(name: &str) -> Option<(Params, Band)> {
    let dir = format!("{}/../../local/agent_state/hdri/skyline-baseline", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(format!("{dir}/{name}_1024.f32")).ok()?;
    let json = std::fs::read_to_string(format!("{dir}/{name}.json")).ok()?;
    let dim = |key: &str| -> usize {
        let at = json.find(&format!("\"{key}\":")).expect(key) + key.len() + 3;
        json[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().expect(key)
    };
    let (w, h) = (dim("w"), dim("h"));
    let mut p = Params::default();
    p.load_web_json(&json).unwrap();
    assert_eq!(bytes.len(), w * h * 16, "{name}: baseline size");
    let texels = bytes.chunks_exact(16).map(|t| std::array::from_fn(|c| f32::from_le_bytes(t[c * 4..c * 4 + 4].try_into().unwrap()))).collect();
    Some((p, Band { w, h, texels }))
}

/// The largest per-channel difference, relative to max(1, |want|).
fn max_abs_diff(got: &[[f32; 4]], want: &[[f32; 4]]) -> f32 {
    got.iter().zip(want).flat_map(|(g, w)| (0..4).map(move |c| (g[c] - w[c]).abs() / w[c].abs().max(1.0))).fold(0.0, f32::max)
}

/// The moved crate renders exactly the bands the example rendered before
/// the move (local/agent_state/hdri/skyline-baseline, Task 1 step 1).
#[test]
#[ignore = "GPU: MAKEPAD_HAL_BACKEND=offscreen"]
fn bands_match_the_pre_move_baseline() {
    for name in ["default", "props_heavy", "offcentre"] {
        let Some((p, want)) = baseline(name) else { eprintln!("no baseline {name}, skipped"); continue };
        let got = render_band(&p, 1024);
        assert_eq!((got.w, got.h), (want.w, want.h), "{name}");
        let worst = max_abs_diff(&got.texels, &want.texels);
        if let Some(dir) = std::env::var_os("SKYLINE_BAND_DUMP") { // the rendered band as raw f32, for a look by eye
            let bytes: Vec<u8> = got.texels.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}_1024.f32")), bytes).unwrap();
        }
        println!("{name}: {} x {}, worst texel difference {worst}", got.w, got.h);
        assert!(worst <= 1e-3, "{name}: worst texel difference {worst}");
    }
}
