//! A texture pass paints only when what it feeds can show it. Run with
//! MAKEPAD_HAL_BACKEND=headless (or a native backend).
#![cfg(not(target_arch = "wasm32"))]
use makepad_platform::*;

fn open() -> (Cx, u32) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    let info = makepad_hal::Hal::global().open(&makepad_hal::OpenReq::from_env(
        0, makepad_hal::open_flags::OFFSCREEN_WINDOWS, 1.0));
    cx.hal_attach_session(info);
    (cx, info.session)
}

fn texture_pass(cx: &mut Cx) -> (DrawPass, DrawList, Texture) {
    let texture = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 {
        size: TextureSize::Fixed { width: 64, height: 64 }, initial: true,
    });
    let pass = DrawPass::new(cx);
    let list = DrawList::new(cx);
    pass.add_color_texture(cx, &texture, DrawPassClearColor::ClearWith(vec4(0.1, 0.2, 0.3, 1.0)));
    pass.set_size(cx, dvec2(64.0, 64.0));
    let id = pass.draw_pass_id();
    cx.passes[id].main_draw_list_id = Some(list.id());
    cx.passes[id].dpi_factor = Some(1.0);
    cx.passes[id].paint_dirty = true;
    (pass, list, texture)
}

/// Sandbox hosted in the WM: its scene passes painted on every host Tick while the window had
/// no free image to show them, and the GPU queue grew behind the window's frame (~1 fps). A
/// scene pass of a window that cannot show a frame now waits with it (stays dirty), while a pass
/// that feeds no window -- an offscreen target, a capture, a readback -- still paints.
#[test]
fn a_scene_pass_waits_for_its_window_and_an_offscreen_pass_does_not() {
    let (mut cx, session) = open();
    // a window this session has not got from the HAL: it cannot show a frame
    let (window_pass, _window_list, _window_texture) = texture_pass(&mut cx);
    cx.passes[window_pass.draw_pass_id()].parent = CxDrawPassParent::Window(WindowId(4242, 1));
    let (scene, _scene_list, _scene_texture) = texture_pass(&mut cx);
    cx.passes[scene.draw_pass_id()].parent = CxDrawPassParent::DrawPass(window_pass.draw_pass_id());
    let (offscreen, _offscreen_list, _offscreen_texture) = texture_pass(&mut cx);
    assert!(cx.hal_session_turn(0, true));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while cx.frame_completion_serial() < cx.frame_submission_serial() {
        assert!(std::time::Instant::now() < deadline, "HAL frame completion timed out");
        assert!(cx.hal_session_turn(1_000_000, false));
    }
    assert!(!cx.passes[offscreen.draw_pass_id()].paint_dirty, "a pass that feeds no window paints");
    assert!(cx.frame_submission_serial() > 0, "the offscreen pass was submitted");
    assert!(cx.passes[scene.draw_pass_id()].paint_dirty, "the scene of a window that cannot show it waits, dirty");
    makepad_hal::Hal::global().close(session);
}
