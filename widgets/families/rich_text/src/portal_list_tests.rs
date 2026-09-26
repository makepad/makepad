//! The portal list's tests, driven through the log list (a portal list of
//! lines), so they live beside the log list (moved here from
//! portal_list.rs's test module with it).

fn test_cx() -> crate::PooledCx {
    crate::checkout_test_cx()
}
use crate::portal_list::*;
use crate::{
    event::{FingerCancelEvent, ScrollPhase, TouchState, TAP_COUNT_DISTANCE},
    makepad_draw::*,
    widget::*,
};
use crate::event::ScrollEvent;
use crate::log_list::LogList;
use crate::makepad_draw::cx_draw::CxDraw;
use std::cell::Cell;
/// One row of daylight between a recalled row and the edge it came over.
const LEAD: usize = 1;

const PANE: DVec2 = dvec2(300.0, 120.0);

/// One draw of a widget over the whole pane, in a draw list of its own so
/// it overlaps whatever else the same pass drew: two widgets whose rects
/// both contain the press point, the way a slider sits inside a list row.
/// A separate draw list keeps its own redraw id, so neither draw
/// invalidates the other's area.
fn frame_over(
    cx: &mut Cx,
    widget: &mut dyn Widget,
    pass: &DrawPass,
    draw_list: &mut DrawList2d,
) {
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    let mut cx2d = Cx2d::new(&mut draw);
    cx2d.begin_pass(pass, None);
    draw_list.begin_always(&mut cx2d);
    cx2d.begin_root_turtle(PANE, Layout::flow_down());
    let _ = widget.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(PANE.x, PANE.y));
    cx2d.end_pass_sized_turtle();
    draw_list.end(&mut cx2d);
    cx2d.end_pass(pass);
}

/// One draw of the pane, in the pass and draw list the test keeps.
fn frame(cx: &mut Cx, log: &mut LogList, pass: &DrawPass, draw_list: &mut DrawList2d) {
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    let mut cx2d = Cx2d::new(&mut draw);
    cx2d.begin_pass(pass, None);
    draw_list.begin_always(&mut cx2d);
    cx2d.begin_root_turtle(PANE, Layout::flow_down());
    let _ = log.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(PANE.x, PANE.y));
    cx2d.end_pass_sized_turtle();
    draw_list.end(&mut cx2d);
    cx2d.end_pass(pass);
}

/// A wheel rolled over the middle of the pane, `dy` toward the end, and
/// whether the list kept it — which is all a scroll view around the list
/// reads before moving by the same wheel.
fn wheel(cx: &mut Cx, log: &mut LogList, dy: f64, handled: bool) -> bool {
    let event = Event::Scroll(ScrollEvent {
        window_id: WindowId(1, 1),
        scroll: dvec2(0.0, dy),
        abs: PANE * 0.5,
        modifiers: KeyModifiers::default(),
        handled_x: Cell::new(false),
        handled_y: Cell::new(handled),
        is_mouse: true,
        time: 0.0,
        phase: ScrollPhase::None,
    });
    log.handle_event(cx, &event, &mut Scope::empty());
    event.scroll_handled(Vec2Index::Y)
}

/// The first row showing and how far it is scrolled, to a hundredth of a
/// point: a draw re-measures the rows, and that alone moves the scroll by
/// millionths.
fn place(cx: &Cx, log: &LogList) -> (usize, f64) {
    let list = log.portal_list(cx, ids!(list));
    let list = list.borrow().expect("the log holds no portal list");
    (list.first_id(), (list.first_scroll() * 100.0).round() / 100.0)
}

/// How many rows the list last drew.
fn window(cx: &Cx, log: &LogList) -> usize {
    log.portal_list(cx, ids!(list)).visible_items()
}

/// Ask the list to keep `index` on screen.
fn keep(cx: &mut Cx, log: &LogList, index: usize) {
    let list = log.portal_list(cx, ids!(list));
    list.keep_index_visible(cx, index);
}

/// Keeping a row, through a real list: a row that is showing holds the
/// list exactly where it is, and one that has gone off either edge comes
/// back with a row to spare beyond it.
#[test]
fn the_list_holds_the_row_it_was_asked_to_keep() {
    crate::on_test_cx(|| {
    let mut cx = test_cx();
    let mut log = cx.with_vm(LogList::script_new_with_default);
    log.lines = (0..200).map(|n| format!("log | line {n}")).collect();
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, PANE);
    let mut draw_list = DrawList2d::new(&mut cx);
    for _ in 0..3 {
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    // Off the tail: a tailing list follows its last row and a request to
    // keep another one is dropped rather than fought.
    for _ in 0..2 {
        wheel(&mut cx, &mut log, -600.0, false);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    let (first, _) = place(&cx, &log);
    let rows = window(&cx, &log);
    assert!(
        first > 40 && first + rows + 20 < 200,
        "the log did not settle in its middle: {first} + {rows}"
    );

    // A row that is showing is not a scroll request.
    let before = place(&cx, &log);
    keep(&mut cx, &log, first + 1);
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    assert_eq!(place(&cx, &log), before, "a row already showing moved the list");

    // A row off the top comes back one row short of it.
    keep(&mut cx, &log, first - 20);
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    assert_eq!(
        place(&cx, &log).0,
        first - 21,
        "the recalled row landed pinned to the top edge"
    );

    // And a row off the bottom comes back with a row to spare below it.
    let far = first + 20;
    keep(&mut cx, &log, far);
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    let (now, _) = place(&cx, &log);
    let rows = window(&cx, &log);
    assert!(
        now <= far && far < now + rows,
        "the row the list was asked to keep is off screen: {now} + {rows} for {far}"
    );
    assert!(
        far + LEAD < now + rows,
        "the recalled row landed pinned to the bottom edge: {now} + {rows} for {far}"
    );
    });
}

/// The continuation rows are ground, not layout: turning them on under a
/// list too short to fill its viewport leaves every real row exactly where
/// it was.
#[test]
fn the_continuation_rows_leave_the_real_rows_alone() {
    crate::on_test_cx(|| {
    let mut cx = test_cx();
    let mut log = cx.with_vm(LogList::script_new_with_default);
    log.lines = (0..3).map(|n| format!("log | line {n}")).collect();
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, PANE);
    let mut draw_list = DrawList2d::new(&mut cx);
    for _ in 0..3 {
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    let before = place(&cx, &log);
    {
        let list = log.portal_list(&cx, ids!(list));
        let mut list = list.borrow_mut().expect("the log holds no portal list");
        assert!(
            !list.is_filling_viewport(),
            "three lines filled the pane, so there is no gap to rule"
        );
        list.filler_rows = true;
    }
    for _ in 0..2 {
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    assert_eq!(place(&cx, &log), before, "the ruling moved the rows it fills after");
    });
}

/// The wheel over a list inside a scrolling page: the list keeps every
/// wheel it moves by, so the page stays put, and hands on the one that
/// points past the edge it rests on, so the page moves instead. A log
/// follows its newest line, which puts this one at its bottom to start.
#[test]
fn a_list_keeps_the_wheel_it_moves_by_and_hands_on_the_wheel_past_its_edge() {
    crate::on_test_cx(|| {
    let mut cx = test_cx();
    let mut log = cx.with_vm(LogList::script_new_with_default);
    log.lines = (0..200).map(|n| format!("log | line {n}")).collect();
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, PANE);
    let mut draw_list = DrawList2d::new(&mut cx);
    for _ in 0..3 {
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    let bottom = place(&cx, &log);
    assert!(bottom.0 > 150, "the log did not open on its newest lines: {bottom:?}");

    assert!(!wheel(&mut cx, &mut log, 60.0, false), "a wheel past the bottom was kept");
    // The draw is what pins the list back on its bottom.
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    assert_eq!(place(&cx, &log), bottom, "the list moved past its bottom");
    assert!(wheel(&mut cx, &mut log, -60.0, false), "a wheel up the list was handed on");
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    assert_ne!(place(&cx, &log), bottom, "the kept wheel did not move the list");
    assert!(wheel(&mut cx, &mut log, 30.0, false), "off the bottom, a wheel down is the list's");
    frame(&mut cx, &mut log, &pass, &mut draw_list);

    // A wheel a row's own scroll view already used moves nothing here.
    let before = place(&cx, &log);
    assert!(wheel(&mut cx, &mut log, -60.0, true));
    frame(&mut cx, &mut log, &pass, &mut draw_list);
    assert_eq!(place(&cx, &log), before, "the list moved by a wheel already used");

    for _ in 0..200 {
        if place(&cx, &log) == (0, 0.0) {
            break;
        }
        wheel(&mut cx, &mut log, -600.0, false);
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    assert_eq!(place(&cx, &log), (0, 0.0), "the wheel never reached the top");
    assert!(!wheel(&mut cx, &mut log, -60.0, false), "a wheel past the top was kept");
    assert!(wheel(&mut cx, &mut log, 60.0, false), "a wheel down from the top was handed on");
    });
}

/// A press through the list's real pointer handling, at `at` over a log of
/// `lines` rows. The control is handed the event first, the way a row's own
/// widgets are handled before the list's hits, and it captures the digit;
/// then the list sees the same press through `capture_overload` and has to
/// decide whether that press is its to scroll by. Answers what the list did
/// with it: (entered a drag, stayed stopped).
///
/// The stand-in for the slider is a `Button` drawn over the whole pane:
/// what the list asks is who holds the mouse, not what kind of control it
/// is, and a Button takes a press exactly as a Slider's thumb does.
fn press_over(lines: usize, at: DVec2, touch: bool, control: bool) -> (bool, bool) {
    let mut cx = test_cx();
    let mut log = cx.with_vm(LogList::script_new_with_default);
    log.lines = (0..lines).map(|n| format!("log | line {n}")).collect();
    let mut button = cx.with_vm(crate::button::Button::script_new_with_default);

    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, PANE);
    let mut draw_list = DrawList2d::new(&mut cx);
    let mut button_list = DrawList2d::new(&mut cx);
    for _ in 0..3 {
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    frame_over(&mut cx, &mut button, &pass, &mut button_list);

    // A console keeps out of the way of the app it is embedded in, so its
    // own list drags nothing and leaves its rows their text selection.
    // Turn it into an ordinary drag-to-scroll list — the widget under test.
    {
        let list = log.portal_list(&cx, ids!(list));
        let mut list = list.borrow_mut().expect("the log holds no portal list");
        list.drag_scrolling = true;
        list.capture_overload = true;
        list.selectable = false;
        assert!(
            list.area.is_valid(&cx) && list.area.clipped_rect(&cx).contains(at),
            "the press point is not over the list"
        );
    }
    assert!(
        button.area().is_valid(&cx) && button.area().clipped_rect(&cx).contains(at),
        "the press point is not over the control"
    );

    let event = if touch {
        Event::TouchUpdate(crate::event::TouchUpdateEvent {
            time: 0.0,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            touches: vec![crate::event::TouchPoint {
                state: TouchState::Start,
                abs: at,
                time: 0.0,
                uid: 1,
                rotation_angle: 0.0,
                force: 1.0,
                radius: dvec2(1.0, 1.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty),
            }],
        })
    } else {
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        Event::MouseDown(MouseDownEvent {
            abs: at,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    };
    if control {
        button.handle_event(&mut cx, &event, &mut Scope::empty());
        assert!(
            cx.fingers.any_areas_captured(),
            "the control did not take the press, so this test proves nothing"
        );
    }
    log.handle_event(&mut cx, &event, &mut Scope::empty());
    cx.fingers.first_mouse_button = None;

    let list = log.portal_list(&cx, ids!(list));
    let list = list.borrow().expect("the log holds no portal list");
    (
        matches!(list.scroll_state, ScrollState::Drag { .. }),
        matches!(list.scroll_state, ScrollState::Stopped),
    )
}

/// What the control over the list heard of one finger (see
/// `touch_over_control`).
#[derive(Debug, Default)]
struct Heard {
    clicked: bool,
    /// Its terminal FingerUp said `cancelled`.
    cancelled: bool,
    /// Terminal FingerUps it got (the press must end exactly once).
    ups: usize,
    long_pressed: bool,
    /// After the list took the finger, the control could not claim it.
    second_claim_refused: bool,
}

/// A finger lands on a control drawn over the list (it takes the press
/// first, the way a row's own controls are handled before the list's
/// hits), travels `travel` points down in four moves the list alone sees
/// (a committed drag keeps its children out of the moves), then — with
/// `host_cancel` — the host takes the finger away (`cancel_digit` +
/// `Event::FingerCancel`) before it lifts. Both hear every terminal
/// event, the control first. The list's own interactive detection
/// decides when it commits: nothing is forced.
fn touch_over_control(travel: f64, host_cancel: bool) -> Heard {
    let mut cx = test_cx();
    let mut log = cx.with_vm(LogList::script_new_with_default);
    log.lines = (0..50).map(|n| format!("log | line {n}")).collect();
    let mut button = cx.with_vm(crate::button::Button::script_new_with_default);
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, PANE);
    let mut draw_list = DrawList2d::new(&mut cx);
    let mut button_list = DrawList2d::new(&mut cx);
    for _ in 0..3 {
        frame(&mut cx, &mut log, &pass, &mut draw_list);
    }
    frame_over(&mut cx, &mut button, &pass, &mut button_list);
    {
        let list = log.portal_list(&cx, ids!(list));
        let mut list = list.borrow_mut().expect("the log holds no portal list");
        list.drag_scrolling = true;
        list.capture_overload = true;
        list.selectable = false;
        list.drag_scroll_threshold = TAP_COUNT_DISTANCE;
    }
    let at = |y: f64| dvec2(BARE.x, BARE.y - 60.0 + y);
    let touch = |state: TouchState, y: f64, time: f64| {
        Event::TouchUpdate(crate::event::TouchUpdateEvent {
            time,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            touches: vec![crate::event::TouchPoint {
                state,
                abs: at(y),
                time,
                uid: 1,
                rotation_angle: 0.0,
                force: 1.0,
                radius: dvec2(1.0, 1.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty),
            }],
        })
    };
    let digit: crate::event::DigitId = live_id_num!(touch, 1).into();
    let end = |cx: &mut Cx, event: &Event| {
        if let Event::TouchUpdate(e) = event {
            cx.fingers.process_touch_update_end(&e.touches);
        }
    };
    let mut heard = Heard::default();
    let control = |cx: &mut Cx, button: &mut crate::button::Button, event: &Event, heard: &mut Heard| {
        let area = button.area();
        let actions = cx.capture_actions(|cx| {
            if let Hit::FingerUp(fe) = event.hits(cx, area) {
                heard.ups += 1;
                heard.cancelled |= fe.cancelled;
                assert!(!(fe.cancelled && (fe.is_over || fe.was_tap())), "a cancelled release was over or a tap");
            }
            if let Hit::FingerLongPress(_) = event.hits(cx, area) {
                heard.long_pressed = true;
            }
        });
        let _ = actions;
        // The real widget hears the same event, for its click: every
        // consumer of one event sees the same terminal hit.
        let actions = cx.capture_actions(|cx| button.handle_event(cx, event, &mut Scope::empty()));
        heard.clicked |= actions.iter().any(|a| {
            matches!(
                a.as_widget_action().map(|w| w.cast::<crate::button::ButtonAction>()),
                Some(crate::button::ButtonAction::Clicked(_))
            )
        });
    };
    let down = touch(TouchState::Start, 0.0, 0.0);
    button.handle_event(&mut cx, &down, &mut Scope::empty());
    assert!(cx.fingers.any_areas_captured(), "the control did not take the finger");
    log.handle_event(&mut cx, &down, &mut Scope::empty());
    end(&mut cx, &down);
    for step in 1..=4 {
        let event = touch(TouchState::Move, travel * step as f64 / 4.0, 0.016 * step as f64);
        log.handle_event(&mut cx, &event, &mut Scope::empty());
        end(&mut cx, &event);
    }
    heard.second_claim_refused = travel > 0.0 && !cx.claim_finger_gesture(digit, button.area());
    // A native long press arriving now must not reach a cancelled press.
    let long = Event::LongPress(crate::event::LongPressEvent {
        abs: at(travel),
        uid: 1,
        window_id: WindowId(1, 1),
        time: 0.5,
    });
    control(&mut cx, &mut button, &long, &mut heard);
    if host_cancel {
        cx.fingers.cancel_digit(digit);
        let cancel = Event::FingerCancel(FingerCancelEvent {
            window_id: WindowId(1, 1),
            digit_id: digit,
            device: crate::event::DigitDevice::Touch { uid: 1 },
            abs: at(travel),
            time: 0.07,
            modifiers: KeyModifiers::default(),
        });
        control(&mut cx, &mut button, &cancel, &mut heard);
        log.handle_event(&mut cx, &cancel, &mut Scope::empty());
    }
    let up = touch(TouchState::Stop, travel, 0.08);
    control(&mut cx, &mut button, &up, &mut heard);
    log.handle_event(&mut cx, &up, &mut Scope::empty());
    end(&mut cx, &up);
    assert!(!cx.fingers.any_areas_captured(), "the lift left a capture behind");
    heard
}

/// A finger that scrolled the list never clicks the row it landed on —
/// its press ends once, cancelled, even though the row travelled with
/// the finger and is under it again at the lift — and nothing else can
/// take the finger from the list; a tap that stayed put still clicks.
#[test]
fn a_list_that_takes_the_finger_cancels_the_row_it_landed_on() {
    crate::on_test_cx(|| {
    let tap = touch_over_control(0.0, false);
    assert!(tap.clicked && !tap.cancelled && tap.ups >= 1, "a plain tap no longer clicks: {tap:?}");
    let scroll = touch_over_control(40.0, false);
    assert!(!scroll.clicked, "the row the finger scrolled clicked: {scroll:?}");
    assert!(scroll.cancelled && scroll.ups == 1, "its release was not one cancelled FingerUp: {scroll:?}");
    assert!(scroll.second_claim_refused, "a second claimant took the finger from the list");
    assert!(!scroll.long_pressed, "a cancelled press long-pressed");
    });
}

/// The host takes a finger away (the phone rotated): the press ends at
/// once with a cancelled FingerUp — exactly one, the lift after it finds
/// nothing left — and nothing clicks.
#[test]
fn a_host_cancel_ends_the_press_once_without_a_click() {
    crate::on_test_cx(|| {
    let heard = touch_over_control(0.0, true);
    assert!(!heard.clicked && heard.cancelled, "{heard:?}");
    assert_eq!(heard.ups, 1, "the press ended more than once: {heard:?}");
    });
}

/// One line drawn at the top of the pane leaves the rest of the list bare,
/// so a press down here is over the list and over nothing else of its own.
const BARE: DVec2 = dvec2(150.0, 100.0);

/// The bug, end to end: a mouse press a control holds does not put the list
/// around it into a drag, so dragging a slider's thumb cannot also scroll
/// the rack the slider sits in.
#[test]
fn a_control_holding_the_mouse_keeps_the_list_around_it_still() {
    crate::on_test_cx(|| {
    assert_eq!(
        press_over(1, BARE, false, true),
        (false, true),
        "a press the control holds started the list's drag-to-scroll"
    );
    });
}

/// And the list is not broken while fixing it: the same press with nothing
/// holding the mouse is the list's own, and still starts its drag.
#[test]
fn a_press_on_bare_list_still_starts_its_drag_scroll() {
    crate::on_test_cx(|| {
    assert_eq!(
        press_over(1, BARE, false, false),
        (true, false),
        "the list stopped drag-scrolling from a press nothing else holds"
    );
    });
}

/// The touch exemption, end to end: a finger that lands on a control still
/// scrolls the list under it.
#[test]
fn a_touch_on_a_control_still_scrolls_the_list_under_it() {
    crate::on_test_cx(|| {
    assert_eq!(
        press_over(1, BARE, true, true),
        (true, false),
        "a finger on a control stopped scrolling the list under it"
    );
    });
}

/// The same rule with the control inside the list rather than over it: a
/// log's rows carry selectable text, which takes the press and drags a
/// selection with it. That press is the row's, so the list does not scroll
/// by it — and a finger's still does.
#[test]
fn a_row_holding_the_mouse_keeps_the_list_it_sits_in_still() {
    crate::on_test_cx(|| {
    let over_a_row = PANE * 0.5;
    assert_eq!(
        press_over(200, over_a_row, false, false),
        (false, true),
        "a press a row's own text holds started the list's drag-to-scroll"
    );
    assert_eq!(
        press_over(200, over_a_row, true, false),
        (true, false),
        "a finger on a row stopped scrolling the list under it"
    );
    });
}
/// A log of 200 lines drawn until it settles: a real list, taller than its
/// pane, with a scroll bar showing.
fn drawn_log(cx: &mut Cx) -> (LogList, DrawPass, DrawList2d) {
    let mut log = cx.with_vm(LogList::script_new_with_default);
    log.lines = (0..200).map(|n| format!("log | line {n}")).collect();
    let pass = DrawPass::new(cx);
    pass.set_size(cx, PANE);
    let mut draw_list = DrawList2d::new(cx);
    for _ in 0..3 {
        frame(cx, &mut log, &pass, &mut draw_list);
    }
    (log, pass, draw_list)
}

fn mouse_down(at: DVec2) -> Event {
    Event::MouseDown(MouseDownEvent {
        abs: at,
        button: MouseButton::PRIMARY,
        window_id: WindowId(1, 1),
        modifiers: KeyModifiers::default(),
        handled: Cell::new(Area::Empty),
        time: 1.0,
    })
}

fn mouse_move(at: DVec2) -> Event {
    Event::MouseMove(MouseMoveEvent {
        abs: at,
        lock_delta: dvec2(0.0, 0.0),
        window_id: WindowId(1, 1),
        modifiers: KeyModifiers::default(),
        time: 2.0,
        handled: Cell::new(Area::Empty),
    })
}

/// The areas a list owns for the "whose press is this" question are its
/// own and its bar's. Naming only its own area is how a list comes to read
/// its own scroll bar as an outsider holding the pointer.
#[test]
fn a_lists_own_areas_include_its_scroll_bar() {
    crate::on_test_cx(|| {
    let mut cx = test_cx();
    let (log, _pass, _list) = drawn_log(&mut cx);
    let list_ref = log.portal_list(&cx, ids!(list));
    let list = list_ref.borrow().expect("the log holds no portal list");
    let mine = list.own_press_areas();
    assert_eq!(mine[0], list.area, "a list left itself out of its own areas");
    assert_eq!(
        mine[1],
        list.scroll_bar.area(),
        "a list left its scroll bar out of its own areas"
    );
    assert!(
        mine[1].is_valid(&cx) && mine[0] != mine[1],
        "the bar drew no area of its own, so this test proves nothing"
    );
    });
}

/// A press on the list's OWN scroll bar belongs to the bar and to nothing
/// else. The list starts no drag-to-scroll and no selection from it: the
/// bar is a continuously dragged control holding the pointer, and a list
/// that also drag-scrolled would move itself twice, the two ways at once.
///
/// Today the list never even reaches its own press path for this press —
/// `handle_event` skips its whole hit block while its bar is captured —
/// and this pins that outcome, whichever of the two guards is the one
/// holding it up.
#[test]
fn a_press_on_the_lists_own_bar_is_the_bars_alone() {
    crate::on_test_cx(|| {
    let mut cx = test_cx();
    let (mut log, _pass, _list) = drawn_log(&mut cx);
    let list_ref = log.portal_list(&cx, ids!(list));
    let bar = {
        // A console keeps out of the way of the app around it, so its own
        // list drags nothing. Turn it into an ordinary drag-to-scroll list,
        // selection and all, so both gestures are armed and a press that
        // started either one would show.
        let mut list = list_ref.borrow_mut().expect("the log holds no portal list");
        list.drag_scrolling = true;
        list.capture_overload = true;
        list.selectable = true;
        list.scroll_bar.area()
    };
    assert!(bar.is_valid(&cx), "the list drew no scroll bar to press");
    let rect = bar.clipped_rect(&cx);
    let at = rect.pos + rect.size * 0.5;

    cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
    log.handle_event(&mut cx, &mouse_down(at), &mut Scope::empty());
    assert!(
        cx.fingers.is_area_captured(bar),
        "the bar did not take the press, so this test proves nothing"
    );
    let list = list_ref.borrow().expect("the log holds no portal list");
    assert!(
        !matches!(list.scroll_state, ScrollState::Drag { .. }),
        "the list drag-scrolled itself off a press its own scroll bar is holding"
    );
    assert!(
        !list.is_selecting,
        "the list started a text selection off a press on its own scroll bar"
    );
    drop(list);
    cx.fingers.first_mouse_button = None;
    });
}

/// The other half of the rule for the selection drag: a control can take
/// the pointer AFTER the drag began — the press and the capture land in
/// either order — so the question is re-asked on every move, not only at
/// the press.
///
/// The order is built here by handing the list the press first, while
/// nothing holds the mouse, and the control the same press after. A Button
/// stands in for any continuously dragged control.
#[test]
fn a_selection_drag_stands_down_when_a_control_takes_the_mouse() {
    crate::on_test_cx(|| {
    let mut cx = test_cx();
    let (mut log, pass, _draw_list) = drawn_log(&mut cx);
    let mut button = cx.with_vm(crate::button::Button::script_new_with_default);
    let mut button_list = DrawList2d::new(&mut cx);
    frame_over(&mut cx, &mut button, &pass, &mut button_list);

    let list_ref = log.portal_list(&cx, ids!(list));
    list_ref
        .borrow_mut()
        .expect("the log holds no portal list")
        .selectable = true;

    let at = PANE * 0.5;
    assert!(
        button.area().is_valid(&cx) && button.area().clipped_rect(&cx).contains(at),
        "the control is not over the press point"
    );

    cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
    let press = mouse_down(at);
    log.handle_event(&mut cx, &press, &mut Scope::empty());
    assert!(
        list_ref
            .borrow()
            .expect("the log holds no portal list")
            .is_selecting,
        "no selection drag started, so this test proves nothing"
    );

    // The control takes the pointer after the drag has begun. The press is
    // un-marked first, because the list co-capturing it marked it handled:
    // what this stands in for is any control that takes the pointer
    // without the list's own press having consumed it — an overlay that
    // captures through the overload, a control that captures off a later
    // raw event such as a long press.
    if let Event::MouseDown(e) = &press {
        e.handled.set(Area::Empty);
    }
    button.handle_event(&mut cx, &press, &mut Scope::empty());
    assert!(
        cx.fingers.is_area_captured(button.area()),
        "the control did not take the press, so this test proves nothing"
    );

    log.handle_event(&mut cx, &mouse_move(at + dvec2(0.0, 8.0)), &mut Scope::empty());
    assert!(
        !list_ref
            .borrow()
            .expect("the log holds no portal list")
            .is_selecting,
        "the selection drag kept running while a control held the mouse"
    );
    cx.fingers.first_mouse_button = None;
    });
}
