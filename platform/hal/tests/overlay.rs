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
    cmd.window_overlay(HOST_WINDOW, on, PANEL, Id::NONE);
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
    // the composed panel's layer going on asks the app for a complete frame under it (Redraw,
    // exec.rs sync_layer); then the frame beat and the geometry
    assert_eq!(app.iter().map(|(e, _)| e.tag).collect::<Vec<_>>(), [tag::Redraw, tag::FrameReady, tag::Geom]);
    assert!(app.iter().all(|(event, _)| event.window == APP_WINDOW));
    overlay(&hal, HOST, false);
    hal.with_world(|w| w.window_event(sid, text, payload));
    assert!(take(&hal, HOST).is_empty());
    // the dismissed panel asks the app for a frame (its last one stayed under the panel)
    let app = take(&hal, sid);
    assert_eq!(app.iter().map(|(e, _)| e.tag).collect::<Vec<_>>(), [tag::Redraw, tag::Text]);
    assert!(app.iter().all(|(event, _)| event.window == APP_WINDOW));
    assert_eq!(app[1].1, payload);
}

fn shown(hal: &Hal) -> Vec<(u32, Id, u64)> {
    last(hal).shown
}

fn layer_on(hal: &Hal) -> bool {
    hal.with_world(|w| w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val.layer_on)
}

/// The 166 report (2026-10-25): the run's view over a running Sandbox and the game alternated
/// frame by frame, because the backend (Vulkan) composes no panels and both presented. The panel
/// and the app never both reach the window: while the panel is up only its frames do, the app's
/// frames render under it and are not seen; after a dismiss only the app's.
#[test]
fn a_composed_panel_and_the_app_never_both_reach_the_window() {
    let (hal, sid) = setup();
    assert_eq!(shown(&hal), [(sid, APP_WINDOW, 11)]);
    overlay(&hal, HOST, true);
    assert!(layer_on(&hal), "the executor composes the panel through the window's layer");
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    assert_eq!(shown(&hal), [(HOST, HOST_WINDOW, 101)]);
    // the app keeps drawing (a game at 120 fps): its frames finish but do not reach the window
    for serial in 12..16 {
        frame(&hal, sid, APP_WINDOW, serial, target::WINDOW);
        assert_eq!(shown(&hal), [], "app frame {serial} under the panel");
        frame(&hal, HOST, HOST_WINDOW, 90 + serial, target::WINDOW);
        assert_eq!(shown(&hal), [(HOST, HOST_WINDOW, 90 + serial)]);
    }
    assert_eq!(frames_done(&take(&hal, sid)), 4, "the app's frames are answered: it is not stalled");
    // a dismiss: the app owns the window again, and draws a new frame for it
    overlay(&hal, HOST, false);
    assert!(!layer_on(&hal));
    assert!(take(&hal, sid).iter().any(|(e, _)| e.tag == tag::Redraw));
    let before = hal.with_world(|w| w.exec_log.len());
    frame(&hal, HOST, HOST_WINDOW, 120, target::WINDOW);
    assert_eq!(hal.with_world(|w| w.exec_log.len()), before, "late view frames are refused");
    frame(&hal, sid, APP_WINDOW, 16, target::WINDOW);
    assert_eq!(shown(&hal), [(sid, APP_WINDOW, 16)]);
}

#[test]
fn badges_wait_under_a_composed_panel_and_come_back_after_it() {
    let (hal, sid) = setup();
    badges(&hal, 21, true);
    frame(&hal, sid, APP_WINDOW, 12, target::WINDOW);
    assert_eq!(shown(&hal), [(sid, APP_WINDOW, 12), (HOST, HOST_WINDOW, 21)]);
    take(&hal, HOST);
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    assert_eq!(shown(&hal), [(HOST, HOST_WINDOW, 101)]);
    // a new badge frame (the ring turning) is kept and answered, not drawn over the panel
    badges(&hal, 22, true);
    assert_eq!(frames_done(&take(&hal, HOST)), 2);
    frame(&hal, sid, APP_WINDOW, 13, target::WINDOW);
    assert_eq!(shown(&hal), []);
    overlay(&hal, HOST, false);
    assert!(layer_on(&hal), "the badges keep the layer");
    frame(&hal, sid, APP_WINDOW, 14, target::WINDOW);
    assert_eq!(shown(&hal), [(sid, APP_WINDOW, 14), (HOST, HOST_WINDOW, 22)]);
}

/// A restart (R in the view, or a crash and relaunch) ends the loan with the panel up: the layer
/// goes with it, and the next borrower's frames reach the window as they are.
#[test]
fn a_restart_under_a_composed_panel_gives_the_next_app_the_window() {
    let (hal, sid) = setup();
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    assert!(layer_on(&hal));
    let mut cmd = CmdWriter::new(sid);
    cmd.window_close(APP_WINDOW);
    hal.submit(&cmd.buf());
    assert!(!layer_on(&hal), "the panel went with the loan");
    // the boot draws its load screen in the meantime, as it is
    frame(&hal, HOST, HOST_WINDOW, 102, target::WINDOW);
    assert_eq!(shown(&hal), [(HOST, HOST_WINDOW, 102)]);
    let next_window = Id::new(APP_WINDOW.index, 2);
    cmd.clear();
    cmd.window_create(next_window, window_kind::NORMAL, Id::NONE, [0.0; 4], true);
    hal.submit(&cmd.buf());
    frame(&hal, sid, next_window, 12, target::WINDOW);
    assert_eq!(shown(&hal), [(sid, next_window, 12)]);
    // and its input is its own once it presented
    take(&hal, HOST);
    take(&hal, sid);
    hal.with_world(|w| w.window_event(sid, RawEvent::drag_query(next_window, 30.0, 40.0), &[]));
    assert!(take(&hal, HOST).is_empty());
    assert_eq!(take(&hal, sid)[0].0.tag, tag::DragQuery);
}

/// A session ends (the domain's process side is torn down without closing its window): the
/// world takes the loan back; the layer follows on the window's next frame.
#[test]
fn a_borrower_that_ends_under_a_composed_panel_takes_the_layer_on_the_next_frame() {
    let (hal, sid) = setup();
    overlay(&hal, HOST, true);
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    hal.with_world(|w| { w.finish_close(sid, makepad_hal::session::CloseAs::Crash("test".into())); });
    frame(&hal, HOST, HOST_WINDOW, 102, target::WINDOW);
    assert!(!layer_on(&hal));
    assert_eq!(shown(&hal), [(HOST, HOST_WINDOW, 102)]);
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
    cmd.window_overlay(HOST_WINDOW, true, [0.0, 0.0, 0.0, 40.0], Id::NONE);
    cmd.window_overlay(HOST_WINDOW, true, [f32::NAN, 0.0, 40.0, 40.0], Id::NONE);
    cmd.window_overlay(Id::new(HOST_WINDOW.index, 2), true, PANEL, Id::NONE);
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

#[test]
fn a_window_layer_leaves_input_with_the_app_and_needs_its_texture() {
    let (hal, sid) = setup();
    const LAYER: Id = Id::new(20, 1);
    let mut cmd = CmdWriter::new(HOST);
    // no such texture yet: refused
    cmd.window_overlay(HOST_WINDOW, true, [0.0; 4], LAYER);
    cmd.texture_create(LAYER, usage::RENDER_TARGET | usage::SAMPLED, format::BGRA8, 2000, 1400, [1, 1], Attr::default());
    cmd.window_overlay(HOST_WINDOW, true, [0.0; 4], LAYER);
    hal.submit(&cmd.buf());
    hal.with_world(|w| {
        let host = w.session(HOST).unwrap();
        assert_eq!(host.stats.bad_args, 1);
        assert_eq!(host.windows.get(HOST, HOST_WINDOW).unwrap().val.overlay.as_ref().map(|o| o.layer), Some(LAYER));
        w.window_event(sid, RawEvent::key_up(APP_WINDOW, 1, 0), &[]);
    });
    assert!(take(&hal, HOST).is_empty(), "under a window layer input stays with the app");
    assert_eq!(take(&hal, sid)[0].0.tag, tag::KeyUp);
    // session 0's frames go on while the app keeps drawing; neither replaces the other's record
    frame(&hal, HOST, HOST_WINDOW, 101, target::WINDOW);
    frame(&hal, sid, APP_WINDOW, 12, target::WINDOW);
    assert_eq!(last(&hal).layers, [(sid, APP_WINDOW, 12), (HOST, HOST_WINDOW, 101)]);
}

#[test]
fn a_hover_badge_sees_the_pointer_over_it_but_never_takes_input() {
    let (hal, sid) = setup();
    const BADGE: Id = Id::new(9, 1);
    // (the headless backend composites no badges: the routing is the world's, set as a badge
    // with `window_style::BADGE_HOVER` sets it)
    hal.with_world(|w| {
        let host = w.session_mut(HOST).unwrap().windows.get_mut(HOST, HOST_WINDOW).unwrap();
        host.val.badge_hover = Some((BADGE, [960.0, 2.0, 24.0, 24.0]));
    });
    let events = [
        // over the app, onto the badge, a click there, a drag query and a scroll there, off it
        RawEvent::mouse_move(APP_WINDOW, 500.0, 10.0, 0.0, 0.0, 0),
        RawEvent::mouse_move(APP_WINDOW, 970.0, 12.0, 0.0, 0.0, 0),
        RawEvent::drag_query(APP_WINDOW, 970.0, 12.0),
        RawEvent::mouse_down(APP_WINDOW, 970.0, 12.0, 1, 0),
        RawEvent::mouse_up(APP_WINDOW, 970.0, 12.0, 1, 0),
        RawEvent::scroll(APP_WINDOW, 970.0, 12.0, 0.0, 3.0, 0, true, 0),
        RawEvent::mouse_move(APP_WINDOW, 800.0, 12.0, 0.0, 0.0, 0),
    ];
    hal.with_world(|w| for e in events { w.window_event(sid, e, &[]) });
    // session 0 sees the hover only: the move over the badge and the leave
    let host: Vec<_> = take(&hal, HOST).iter().map(|(e, _)| (e.tag, e.window)).collect();
    assert_eq!(host, [(tag::MouseMove, BADGE), (tag::MouseLeave, BADGE)]);
    // the app gets every event, the press, the caption question and the scroll over the badge too
    let app: Vec<_> = take(&hal, sid).iter().map(|(e, _)| (e.tag, e.window)).collect();
    assert_eq!(app, events.iter().map(|e| (e.tag, APP_WINDOW)).collect::<Vec<_>>());
}

#[test]
fn the_apps_caption_is_the_host_windows() {
    let (hal, sid) = setup();
    let mut app = CmdWriter::new(sid);
    app.window_caption(APP_WINDOW, 0.0, 28.0, [7.0, 6.0, 54.0, 16.0], true);
    hal.submit(&app.buf());
    hal.with_world(|w| {
        let host = &w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val;
        assert_eq!(host.app_caption, Some(AppCaption { top: 0.0, height: 28.0, buttons: [7.0, 6.0, 54.0, 16.0], dark: true }));
    });
}

/// Session 0's badge layer frame of the host window: one pass, a draw when `draws` (its uniforms
/// borrowed from a buffer gone after the submit).
fn badges(hal: &Hal, serial: u64, draws: bool) {
    let mut cmd = CmdWriter::new(HOST);
    cmd.frame_begin(serial, FrameMeta::default());
    cmd.pass(Pass { viewport: [0.0, 0.0, 2000.0, 1400.0], ..Default::default() });
    let uniforms: Vec<u8> = (0..32).collect();
    if draws {
        let mut d = Draw::default();
        d.uniforms[0] = Slice::from_bytes(&uniforms);
        cmd.draw(d);
    }
    cmd.frame_end(HOST_WINDOW, target::WINDOW | target::BADGES);
    hal.submit(&cmd.buf());
    drop(uniforms);
}

fn frames_done(events: &[(RawEvent, Vec<u8>)]) -> usize {
    events.iter().filter(|(e, _)| e.tag == tag::FrameDone).count()
}

#[test]
fn a_badge_layer_is_kept_over_every_frame_until_one_without_draws() {
    let (hal, sid) = setup();
    badges(&hal, 21, true);
    let kept = hal.with_world(|w| w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val.badges.clone()).expect("kept");
    // the layer owns what its draw borrowed
    let view = kept.view();
    assert_eq!(unsafe { view.draws[0].uniforms[0].as_bytes() }, (0..32).collect::<Vec<u8>>().as_slice());
    assert_eq!((view.frames[0].passes.off, view.frames[0].draws.off), (0, 0));
    assert_eq!(frames_done(&take(&hal, HOST)), 1);
    // the app's frames are drawn under it: the app hears of them, session 0 does not again
    frame(&hal, sid, APP_WINDOW, 12, target::WINDOW);
    frame(&hal, sid, APP_WINDOW, 13, target::WINDOW);
    assert_eq!(frames_done(&take(&hal, sid)), 2);
    assert_eq!(frames_done(&take(&hal, HOST)), 0);
    assert_eq!(hal.with_world(|w| w.session(HOST).unwrap().frame), 21);
    // a frame without draws ends the layer
    badges(&hal, 22, false);
    assert_eq!(frames_done(&take(&hal, HOST)), 1);
    assert!(hal.with_world(|w| w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val.badges.is_none()));
}

#[test]
fn only_session_0_has_badge_layers_and_a_badge_has_no_frames() {
    let (hal, sid) = setup();
    let bad = |hal: &Hal, sid: u32| hal.with_world(|w| w.session(sid).unwrap().stats.bad_args);
    let before = bad(&hal, sid);
    frame(&hal, sid, APP_WINDOW, 12, target::WINDOW | target::BADGES);
    frame(&hal, sid, APP_WINDOW, 13, target::WINDOW | target::REPLAY);
    assert_eq!(bad(&hal, sid), before + 2);
    const BADGE: Id = Id::new(9, 1);
    let mut host = CmdWriter::new(HOST);
    host.window_create(BADGE, window_kind::BADGE | window_style::BADGE_HOVER, HOST_WINDOW, [960.0, 2.0, 24.0, 24.0], false);
    hal.submit(&host.buf());
    assert!(hal.with_world(|w| w.session(HOST).unwrap().windows.get(HOST, HOST_WINDOW).unwrap().val.badge_hover.is_some()));
    let before = bad(&hal, HOST);
    frame(&hal, HOST, BADGE, 30, target::WINDOW);
    assert_eq!(bad(&hal, HOST), before + 1);
}
