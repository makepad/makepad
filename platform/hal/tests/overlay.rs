//! Composition and ownership only: the headless executor is not a pixel proof.
use makepad_hal::events::event_tag as tag;
use makepad_hal::session::{ExecRecord, HOST};
use makepad_hal::*;

const HOST_WINDOW: Id = Id::new(3, 1);
const APP_WINDOW: Id = Id::new(7, 1);
const PANEL: [f32; 4] = [24.0, 24.0, 720.0, 480.0];

fn setup() -> (std::sync::Arc<Hal>, u32) {
    let hal = Hal::new();
    let req = OpenReq { backend: backend_id::HEADLESS, app_key: 0,
        flags: open_flags::RETAIN_WINDOWS, dpi_hint: 2.0, parent: 0 };
    assert_eq!(hal.open_host(&req).backend, backend_id::HEADLESS);
    let mut host = CmdWriter::new(HOST);
    host.window_create(HOST_WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 1000.0, 700.0], false);
    hal.submit(&host.buf());
    let sid = hal.open(&OpenReq { app_key: 99, ..req }).session;
    assert_ne!(sid, HOST);
    let mut app = CmdWriter::new(sid);
    app.window_create(APP_WINDOW, window_kind::NORMAL, Id::NONE, [0.0; 4], true);
    hal.submit(&app.buf());
    hal.with_world(|w| w.trace_exec = true);
    frame(&hal, sid, APP_WINDOW, 11, target::WINDOW);
    take(&hal, HOST);
    take(&hal, sid);
    (hal, sid)
}

fn frame(hal: &Hal, sid: u32, window: Id, serial: u64, kind: u32) {
    let mut cmd = CmdWriter::new(sid);
    cmd.frame_begin(serial, FrameMeta::default());
    // Two app passes must both finish before the retained host panel is composed.
    for _ in 0..2 {
        cmd.pass(Pass { viewport: [0.0, 0.0, 2000.0, 1400.0],
            load: [pass_load::CLEAR; 5], ..Default::default() });
    }
    cmd.frame_end(window, kind);
    hal.submit(&cmd.buf());
}

fn overlay(hal: &Hal, sid: u32, on: bool) {
    let mut cmd = CmdWriter::new(sid);
    cmd.window_overlay(HOST_WINDOW, on, PANEL);
    hal.submit(&cmd.buf());
}

fn last(hal: &Hal) -> ExecRecord {
    hal.with_world(|w| w.exec_log.last().unwrap().clone())
}

fn take(hal: &Hal, sid: u32) -> Vec<(RawEvent, Vec<u8>)> {
    hal.with_world(|w| {
        let mut result = Vec::new();
        w.session_mut(sid).unwrap().events.drain_with(|event, payload| {
            result.push((event, payload.to_vec()));
        });
        result
    })
}

#[test]
fn host_surface_stays_after_every_app_frame_without_replacing_the_base() {
    let (hal, sid) = setup();
    assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, 11)]);
    overlay(&hal, HOST, true);
    // The app may be idle or parked: a host frame alone reveals the overlay.
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, 11), (HOST, HOST_WINDOW, 101)]);
    for serial in 12..15 {
        frame(&hal, sid, APP_WINDOW, serial, target::WINDOW);
        assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, serial), (HOST, HOST_WINDOW, 101)]);
    }
    frame(&hal, HOST, HOST_WINDOW, 102, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, 14), (HOST, HOST_WINDOW, 102)]);
    hal.with_world(|w| {
        let host = &w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val;
        assert_eq!(host.presenter, sid);
        assert_eq!(host.presentations, 4, "overlay serials do not pass the app's first-frame gate");
        assert!(host.borrower_presented);
        assert_eq!(w.session(sid).unwrap().stats.passes, 8);
        assert_eq!(w.session(HOST).unwrap().stats.passes, 4);
    });
    assert!(take(&hal, HOST).iter().any(|(e, _)| e.tag == tag::FrameDone));
    assert!(take(&hal, sid).iter().any(|(e, _)| e.tag == tag::FrameDone));
}

#[test]
fn dismiss_restores_the_idle_base_and_rejects_late_boot_frames() {
    let (hal, sid) = setup();
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    overlay(&hal, HOST, false);
    hal.with_world(|w| {
        let host = &w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val;
        assert!(host.overlay.is_none());
        assert_eq!(host.base_frame, Some((sid, APP_WINDOW, 11)));
    });
    let before = hal.with_world(|w| w.exec_log.len());
    frame(&hal, HOST, HOST_WINDOW, 102, target::WINDOW);
    assert_eq!(hal.with_world(|w| w.exec_log.len()), before);
    frame(&hal, sid, APP_WINDOW, 12, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, 12)]);
}

#[test]
fn input_payloads_go_to_the_overlay_but_geometry_and_frame_beats_reach_the_app() {
    let (hal, sid) = setup();
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    take(&hal, HOST);
    let payload = "edited text: λ".as_bytes();
    let mut text = RawEvent::text(APP_WINDOW, input::text::TYPED,
        Range { off: 0, len: payload.len() as u32 });
    text.time_ns = 1234;
    let inputs = [
        RawEvent::mouse_down(APP_WINDOW, 30.0, 40.0, 0, 0),
        RawEvent::key_down(APP_WINDOW, 1, false, 0),
        RawEvent::scroll(APP_WINDOW, 30.0, 40.0, 0.0, 12.0, 0, true, 0),
        RawEvent::drag_query(APP_WINDOW, 30.0, 40.0),
    ];
    hal.with_world(|w| {
        for event in inputs { w.window_event(sid, event, &[]); }
        w.window_event(sid, text, payload);
        w.window_event(sid, RawEvent::frame_ready(APP_WINDOW, 77), &[]);
        w.window_event(sid, RawEvent::geom(APP_WINDOW, 2.0, 3.0, 900.0, 650.0, 2.0, 0), &[]);
    });
    let host = take(&hal, HOST);
    assert_eq!(host.len(), inputs.len() + 2);
    assert!(host.iter().all(|(event, _)| event.window == HOST_WINDOW));
    let received = host.iter().find(|(event, _)| event.tag == tag::Text).unwrap();
    assert_eq!(received.0.time_ns, 1234);
    assert_eq!(received.1, payload);
    let app = take(&hal, sid);
    assert_eq!(app.iter().map(|(e, _)| e.tag).collect::<Vec<_>>(), [tag::FrameReady, tag::Geom]);
    assert!(app.iter().all(|(event, _)| event.window == APP_WINDOW));
    overlay(&hal, HOST, false);
    hal.with_world(|w| w.window_event(sid, text, payload));
    assert!(take(&hal, HOST).is_empty());
    let app = take(&hal, sid);
    assert_eq!(app.len(), 1);
    assert_eq!(app[0].0.window, APP_WINDOW);
    assert_eq!(app[0].1, payload);
}

#[test]
fn overlay_is_per_window_and_only_the_host_can_control_it() {
    let (hal, sid) = setup();
    overlay(&hal, sid, true);
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.denied), 1);
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    let other = Id::new(8, 1);
    let mut cmd = CmdWriter::new(sid);
    cmd.window_create(other, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 320.0, 240.0], false);
    hal.submit(&cmd.buf());
    take(&hal, HOST);
    take(&hal, sid);
    hal.with_world(|w| w.window_event(sid, RawEvent::mouse_down(other, 10.0, 10.0, 0, 0), &[]));
    assert!(take(&hal, HOST).is_empty());
    assert_eq!(take(&hal, sid)[0].0.window, other);
    frame(&hal, sid, other, 12, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, other, 12)]);
    // A forged internal target flag cannot make an app frame into a host frame.
    let before = hal.with_world(|w| w.exec_log.len());
    frame(&hal, sid, APP_WINDOW, 13, target::WINDOW | target::OVERLAY);
    assert_eq!(hal.with_world(|w| w.exec_log.len()), before);
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 1);
}

#[test]
fn releasing_the_borrower_clears_overlay_ownership_before_readoption() {
    let (hal, sid) = setup();
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    let mut cmd = CmdWriter::new(sid);
    cmd.window_close(APP_WINDOW);
    hal.submit(&cmd.buf());
    hal.with_world(|w| {
        let host = w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap();
        assert_eq!(host.lent_to, 0);
        assert!(host.val.overlay.is_none());
    });
    frame(&hal, HOST, HOST_WINDOW, 102, target::WINDOW);
    assert_eq!(last(&hal).layers, [(HOST, HOST_WINDOW, 102)]);
    let next_window = Id::new(APP_WINDOW.index, 2);
    cmd.clear();
    cmd.window_create(next_window, window_kind::NORMAL, Id::NONE, [0.0; 4], true);
    hal.submit(&cmd.buf());
    frame(&hal, sid, next_window, 12, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, next_window, 12)]);
}

#[test]
fn invalid_overlay_rectangles_leave_app_input_and_composition_intact() {
    let (hal, sid) = setup();
    let mut cmd = CmdWriter::new(HOST);
    cmd.window_overlay(HOST_WINDOW, true, [0.0, 0.0, 0.0, 40.0]);
    cmd.window_overlay(HOST_WINDOW, true, [f32::NAN, 0.0, 40.0, 40.0]);
    cmd.window_overlay(Id::new(HOST_WINDOW.index, 2), true, PANEL);
    hal.submit(&cmd.buf());
    hal.with_world(|w| {
        let host = w.session(HOST).unwrap();
        assert_eq!(host.stats.bad_args, 2);
        assert_eq!(host.stats.stale_ids, 1);
        assert!(host.windows.get(HOST, HOST_WINDOW).unwrap().val.overlay.is_none());
        w.window_event(sid, RawEvent::key_up(APP_WINDOW, 1, 0), &[]);
    });
    assert!(take(&hal, HOST).is_empty());
    assert_eq!(take(&hal, sid)[0].0.tag, tag::KeyUp);
    frame(&hal, sid, APP_WINDOW, 12, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, 12)]);
}
