#![cfg(not(target_arch = "wasm32"))]

use makepad_platform::*;
use std::time::{Duration, Instant};

/// Sandbox's stepped launch could time out after 30 s on a loaded box. A glyph answer that
/// arrived after the first present stayed counted until a draw, and the step waited to draw.
#[test]
fn a_queued_glyph_answer_stops_holding_up_the_next_frame() {
    const FONT: u32 = 1;
    const GLYPH: u32 = 36;
    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../widgets/resources/Inter.ttf");

    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    let info = makepad_hal::Hal::global().open(&makepad_hal::OpenReq::from_env(
        0, makepad_hal::open_flags::OFFSCREEN_WINDOWS, 1.0,
    ));
    cx.hal_attach_session(info);

    cx.hal_font_open(FONT, 0, &[], HalFontSource::Path(PATH));

    let mut window = WindowHandle::new(&mut cx);
    let size = dvec2(32.0, 32.0);
    window.configure_window(&mut cx, size, dvec2(0.0, 0.0), false, "Glyph answers".into());
    let mut pass = DrawPass::new(&mut cx);
    let list = DrawList::new(&mut cx);
    window.set_pass(&mut cx, &pass);
    pass.set_size(&mut cx, size);
    pass.set_dpi_factor(&mut cx, 1.0);
    cx.passes[pass.draw_pass_id()].main_draw_list_id = Some(list.id());
    cx.passes[pass.draw_pass_id()].paint_dirty = true;

    let deadline = Instant::now() + Duration::from_secs(30);
    let presented = frame_trace::presented_frames();
    while frame_trace::presented_frames() == presented {
        assert!(Instant::now() < deadline, "first offscreen window present timed out");
        assert!(cx.hal_session_turn(1_000_000, true));
    }

    let received = cx.hal_glyph_answers_received();
    cx.hal_glyph_request(FONT, hal_font_wire::kind::SDF, &[(GLYPH, 16.0)]);
    assert!(cx.pending_async_loads().contains(&("glyph", 1)), "the requested glyph is in flight");
    while cx.hal_glyph_answers_received() == received {
        assert!(Instant::now() < deadline, "HAL glyph answer timed out");
        assert!(cx.hal_session_turn(1_000_000, false));
    }

    assert_eq!(cx.hal_glyph_requests_unanswered(), 0, "an answer that arrived is no longer in flight, taken or not");
    assert_eq!(cx.hal_glyph_answers_queued(), 1, "the answer waits in the queue for the next draw");
    assert!(
        !cx.pending_async_loads().iter().any(|&(kind, _)| kind == "glyph"),
        "the answer is available to the next draw"
    );
    let events = cx.hal_take_font_events();
    assert!(
        events.iter().any(|event| matches!(event, HalFontEvent::Ready { font: FONT, entries }
            if entries.iter().any(|e| e.glyph == GLYPH && e.replaces == 0))),
        "the draw receives the requested glyph"
    );
    assert_eq!(cx.hal_glyph_answers_queued(), 0, "taking the events empties the queue");
    assert_eq!(cx.hal_glyph_answers_received(), received + 1, "one request, one answer received");
    cx.hal_close_session();
}
