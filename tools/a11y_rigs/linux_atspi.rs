#![cfg(target_os = "linux")]
//! Adapter protocol coverage independent of how far individual widgets have been ported.
use makepad_a11y::{
    ActionArg, ActionResult, Actions, LineBox, Live, Node, NodeId, Notification, RangeValue, Rect,
    Role, States, TableInfo, TextInfo, TreeUpdate,
};
use makepad_atspi::*;
use makepad_dbus_wire::{Body, Connection, Message, MessageType, Value};
use makepad_hal::{
    a11y, backend_id, window_kind, CmdWriter, EventStore, Hal, HalEvent, Id, OpenReq,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(5);
const ROOT: &str = "/org/a11y/atspi/accessible/root";
fn fixture() -> Vec<Node> {
    let bounds = Rect {
        x: 10.,
        y: 20.,
        w: 100.,
        h: 30.,
    };
    vec![
        Node {
            id: NodeId(1),
            role: Role::Window,
            name: "Protocol rig".into(),
            bounds: Rect {
                x: 0.,
                y: 0.,
                w: 500.,
                h: 500.,
            },
            children: vec![
                NodeId(2),
                NodeId(3),
                NodeId(4),
                NodeId(5),
                NodeId(7),
                NodeId(10),
            ],
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            role: Role::Button,
            name: "Press".into(),
            bounds,
            states: States::FOCUSABLE,
            actions: Actions::PRESS | Actions::FOCUS | Actions::SCROLL_INTO_VIEW | Actions::CUSTOM,
            custom_actions: vec!["refresh".into()],
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            role: Role::TextField,
            name: "Unicode".into(),
            bounds,
            states: States::EDITABLE | States::FOCUSABLE | States::MULTILINE,
            actions: Actions::FOCUS | Actions::REPLACE_TEXT | Actions::SET_TEXT_SELECTION,
            text: Some(TextInfo {
                content: "aé😀\nword two".into(),
                selection: (1, 7),
                lines: vec![
                    LineBox {
                        start: 0,
                        rect: bounds,
                        baseline: 40.,
                    },
                    LineBox {
                        start: 8,
                        rect: Rect { y: 50., ..bounds },
                        baseline: 70.,
                    },
                ],
                glyph_x: vec![
                    10., 20., 30., 40., 50., 10., 20., 30., 40., 50., 60., 70., 80., 90.,
                ],
            }),
            ..Default::default()
        },
        Node {
            id: NodeId(4),
            role: Role::Slider,
            name: "Level".into(),
            bounds,
            range: Some(RangeValue {
                min: 0.,
                max: 10.,
                now: 2.,
                step: 0.5,
            }),
            actions: Actions::SET_VALUE,
            ..Default::default()
        },
        Node {
            id: NodeId(5),
            role: Role::ListBox,
            name: "Choices".into(),
            bounds,
            children: vec![NodeId(6)],
            states: States::MULTISELECTABLE,
            ..Default::default()
        },
        Node {
            id: NodeId(6),
            role: Role::Option,
            name: "Choice".into(),
            bounds,
            states: States::SELECTABLE,
            actions: Actions::SELECT | Actions::ADD_SELECTION | Actions::REMOVE_SELECTION,
            ..Default::default()
        },
        Node {
            id: NodeId(7),
            role: Role::Table,
            name: "Table".into(),
            bounds,
            table: Some(TableInfo {
                rows: 1,
                cols: 2,
                ..Default::default()
            }),
            children: vec![NodeId(8)],
            ..Default::default()
        },
        Node {
            id: NodeId(8),
            role: Role::Cell,
            name: "Cell".into(),
            bounds,
            table: Some(TableInfo {
                rows: 1,
                cols: 2,
                row: 0,
                col: 0,
                row_span: 1,
                col_span: 2,
            }),
            ..Default::default()
        },
        Node {
            id: NodeId(10),
            role: Role::Status,
            name: "Waiting".into(),
            bounds,
            live: Live::Polite,
            ..Default::default()
        },
    ]
}
fn update(
    hal: &Hal,
    sid: u32,
    window: Id,
    nodes: Vec<Node>,
    focus: NodeId,
    notifications: Vec<Notification>,
) {
    let mut bytes = Vec::new();
    makepad_a11y::wire::encode_update(
        &TreeUpdate {
            full: true,
            root: NodeId(1),
            focus,
            nodes,
            notifications,
            ..Default::default()
        },
        &mut bytes,
    );
    let mut w = CmdWriter::new(sid);
    w.a11y_update(window, &bytes);
    hal.submit(&w.buf());
}
struct Finish(Arc<AtomicBool>);
impl Drop for Finish {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
#[test]
fn real_bus_interfaces_actions_events_and_teardown() {
    if std::env::var("DBUS_WIRE_LIVE").as_deref() != Ok("1") {
        eprintln!("run tools/a11y_rigs/linux_atspi.sh for the private real-bus rig");
        return;
    }
    let hal = Hal::global();
    let sid = hal
        .open(&OpenReq {
            backend: backend_id::HEADLESS,
            ..Default::default()
        })
        .session;
    let window = Id::new(0, 1);
    let second = Id::new(1, 1);
    let mut w = CmdWriter::new(sid);
    for id in [window, second] {
        w.window_create(
            id,
            window_kind::NORMAL,
            Id::NONE,
            [0., 0., 500., 500.],
            false,
        );
    }
    hal.submit(&w.buf());
    let mut nodes = fixture();
    let mut focus = NodeId::NONE;
    update(hal, sid, window, nodes.clone(), focus, vec![]);
    update(
        hal,
        sid,
        second,
        vec![Node {
            id: NodeId(1),
            role: Role::Window,
            name: "Second window".into(),
            ..Default::default()
        }],
        focus,
        vec![],
    );
    let mut bytes = Vec::new();
    makepad_a11y::wire::encode_update(
        &TreeUpdate {
            root: NodeId(1),
            ..Default::default()
        },
        &mut bytes,
    );
    let mut timing = CmdWriter::new(sid);
    timing.a11y_update(window, &bytes);
    let started = Instant::now();
    for _ in 0..1000 {
        hal.submit(&timing.buf());
    }
    println!(
        "HAL mirror + AT-SPI enqueue: 1000 empty diffs, {:.2} us/submit",
        started.elapsed().as_secs_f64() * 1000.
    );
    let done = Arc::new(AtomicBool::new(false));
    let client_done = done.clone();
    let client = std::thread::spawn(move || {
        let _finish = Finish(client_done);
        client_checks(sid);
    });
    let mut store = EventStore::new(sid, 64, 65536);
    while !done.load(Ordering::Acquire) {
        let mut buf = store.buf();
        let count = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(count, buf.arena_len) {
            if let HalEvent::A11yAction {
                window: target,
                bytes,
            } = event
            {
                let action = makepad_a11y::wire::decode_action(bytes).unwrap();
                assert_eq!(target, window);
                let mut notices = vec![];
                if action.action == Actions::PRESS {
                    nodes.iter_mut().find(|n| n.id == action.node).unwrap().name = "Pressed".into();
                    nodes.retain(|n| n.id != NodeId(10));
                    let root = nodes.iter_mut().find(|n| n.id == NodeId(1)).unwrap();
                    root.children.retain(|n| *n != NodeId(10));
                    root.children.push(NodeId(11));
                    nodes.push(Node {
                        id: NodeId(11),
                        role: Role::Status,
                        name: "Ready".into(),
                        live: Live::Polite,
                        ..Default::default()
                    });
                    notices.push(Notification::Announce {
                        text: "Ready".into(),
                        live: Live::Polite,
                    });
                } else if action.action == Actions::FOCUS {
                    focus = action.node;
                } else {
                    let node = nodes.iter_mut().find(|n| n.id == action.node).unwrap();
                    match action.arg {
                        ActionArg::Number(value) => node.range.as_mut().unwrap().now = value,
                        ActionArg::Range(a, b) => node.text.as_mut().unwrap().selection = (a, b),
                        ActionArg::Replace(a, b, text) => {
                            let t = node.text.as_mut().unwrap();
                            t.content.replace_range(a as usize..b as usize, &text);
                            t.selection = (a + text.len() as u32, a + text.len() as u32);
                        }
                        _ if action.action.intersects(
                            Actions::SELECT | Actions::ADD_SELECTION | Actions::REMOVE_SELECTION,
                        ) =>
                        {
                            node.states
                                .set(States::SELECTED, action.action != Actions::REMOVE_SELECTION)
                        }
                        _ => (),
                    }
                }
                update(hal, sid, window, nodes.clone(), focus, notices);
                let mut w = CmdWriter::new(sid);
                w.a11y_action_done(action.req, ActionResult::Handled as u32);
                hal.submit(&w.buf());
            }
        }
    }
    let result = client.join();
    hal.close(sid);
    result.unwrap();
}
fn client_checks(sid: u32) {
    let mut session = Connection::session(TIMEOUT).unwrap();
    let address =
        call_bus_get_address(&mut session, "org.a11y.Bus", "/org/a11y/bus", TIMEOUT).unwrap();
    let mut c = Connection::connect(&address, TIMEOUT).unwrap();
    let deadline = Instant::now() + TIMEOUT;
    let app = loop {
        let apps =
            call_accessible_get_children(&mut c, "org.a11y.atspi.Registry", ROOT, TIMEOUT).unwrap();
        if let Some(app) = apps.into_iter().find(|a| {
            call_accessible_get_name(&mut c, &a.bus, &a.path, TIMEOUT)
                .unwrap_or_default()
                .contains("linux_atspi")
        }) {
            break app;
        }
        assert!(
            Instant::now() < deadline,
            "application registration timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let bus = &app.bus;
    let p = |id| format!("/org/a11y/atspi/accessible/{sid:x}_0_{id:x}");
    macro_rules! call { ($f:ident,$id:expr $(,$a:expr)*) => { $f(&mut c,bus,&p($id),$($a,)* TIMEOUT).unwrap() }; }
    macro_rules! eventually {
        ($condition:expr) => {{
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if $condition {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "condition timed out: {}",
                    stringify!($condition)
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }};
    }
    assert_ne!(a11y::consumers() & makepad_a11y::consumer::OS, 0);
    let children = call_accessible_get_children(&mut c, bus, ROOT, TIMEOUT).unwrap();
    assert_eq!(children.len(), 2);
    assert_ne!(children[0].path, children[1].path);
    assert_eq!(call!(call_accessible_get_role, 1), AtspiRole::Frame as u32);
    let started = Instant::now();
    for _ in 0..100 {
        assert_eq!(call!(call_accessible_get_name, 2), "Press");
    }
    println!(
        "AT-SPI real-bus Name: 100 calls, {:.1} us/call",
        started.elapsed().as_secs_f64() * 10_000.
    );
    assert_eq!(
        call!(call_component_get_extents, 2, AtspiCoordType::Window as u32),
        (10, 20, 100, 30)
    );
    assert!(call!(
        call_component_contains,
        2,
        15,
        25,
        AtspiCoordType::Window as u32
    ));
    assert!(!call!(
        call_component_contains,
        2,
        1,
        1,
        AtspiCoordType::Window as u32
    ));
    assert_eq!(
        call!(
            call_component_get_accessible_at_point,
            2,
            15,
            25,
            AtspiCoordType::Window as u32
        )
        .path,
        "/org/a11y/atspi/null"
    ); // later sibling overlays this button
    assert!(
        call_component_get_extents(&mut c, bus, &p(2), AtspiCoordType::Screen as u32, TIMEOUT)
            .is_err()
    );
    let a = call!(call_action_get_actions, 2);
    let press = a.iter().position(|a| a.0 == "press").unwrap() as i32;
    assert!(a.iter().any(|a| a.0 == "refresh"));
    assert_eq!(call!(call_text_get_character_count, 3), 12);
    assert_eq!(call!(call_text_get_text, 3, 1, 3), "é😀");
    assert_eq!(call!(call_text_get_selection, 3, 0), (1, 3));
    assert_eq!(call!(call_text_get_caret_offset, 3), 3);
    assert_eq!(
        call!(
            call_text_get_string_at_offset,
            3,
            1,
            AtspiTextGranularity::Char as u32
        ),
        ("é".into(), 1, 2)
    );
    assert_eq!(
        call!(
            call_text_get_string_at_offset,
            3,
            5,
            AtspiTextGranularity::Word as u32
        ),
        ("word".into(), 4, 8)
    );
    assert_eq!(
        call!(
            call_text_get_string_at_offset,
            3,
            2,
            AtspiTextGranularity::Line as u32
        ),
        ("aé😀\n".into(), 0, 4)
    );
    assert_eq!(
        call!(
            call_text_get_character_extents,
            3,
            1,
            AtspiCoordType::Window as u32
        ),
        (20, 20, 10, 30)
    );
    assert_eq!(
        call!(
            call_text_get_offset_at_point,
            3,
            30,
            21,
            AtspiCoordType::Window as u32
        ),
        2
    );
    assert_eq!(call!(call_table_get_n_rows, 7), 1);
    assert_eq!(call!(call_table_get_n_columns, 7), 2);
    assert_eq!(call!(call_table_get_accessible_at, 7, 0, 1).path, p(8));
    assert_eq!(
        call!(call_table_cell_get_row_column_span, 8),
        (true, 0, 0, 1, 2)
    );
    assert_eq!(call!(call_table_cell_get_table, 8).path, p(7));
    c.add_match(&format!("type='signal',sender='{bus}'"), TIMEOUT)
        .unwrap();
    assert!(call!(call_component_grab_focus, 2));
    eventually!(
        call!(call_accessible_get_state, 2)[0] & (1 << AtspiStateType::Focused as u32) != 0
    );
    call!(call_value_set_current_value, 4, 7.5);
    eventually!(call!(call_value_get_current_value, 4) == 7.5);
    assert!(call!(call_selection_select_child, 5, 0));
    eventually!(call!(call_selection_get_n_selected_children, 5) == 1);
    assert_eq!(call!(call_selection_get_selected_child, 5, 0).path, p(6));
    assert!(call!(call_selection_deselect_child, 5, 0));
    eventually!(call!(call_selection_get_n_selected_children, 5) == 0);
    assert!(call!(call_text_set_selection, 3, 0, 0, 2));
    eventually!(call!(call_text_get_selection, 3, 0) == (0, 2));
    assert!(call!(call_text_set_caret_offset, 3, 1));
    eventually!(call!(call_text_get_caret_offset, 3) == 1);
    assert!(call!(call_editable_text_insert_text, 3, 1, "🦀", 4));
    eventually!(call!(call_text_get_text, 3, 0, -1) == "a🦀é😀\nword two");
    assert!(call!(call_editable_text_delete_text, 3, 1, 2));
    eventually!(call!(call_text_get_text, 3, 0, -1) == "aé😀\nword two");
    assert!(call!(call_editable_text_set_text_contents, 3, "é😀"));
    eventually!(call!(call_text_get_text, 3, 0, -1) == "é😀");
    assert!(call!(call_action_do_action, 2, press));
    eventually!(call!(call_accessible_get_name, 2) == "Pressed");
    let cache = call_cache_get_items(&mut c, bus, "/org/a11y/atspi/cache", TIMEOUT).unwrap();
    assert!(cache.iter().any(|i| i.0.path == p(11)));
    assert!(!cache.iter().any(|i| i.0.path == p(10)));
    let mut signals = Vec::new();
    let expected = [
        "Focus",
        "StateChanged",
        "PropertyChange",
        "ChildrenChanged",
        "TextChanged",
        "TextCaretMoved",
        "Announcement",
        "AddAccessible",
        "RemoveAccessible",
    ];
    eventually!({
        c.read_ready().unwrap();
        while let Some(m) = c.pop_message() {
            if m.header.kind == MessageType::Signal {
                signals.push(m);
            }
        }
        expected.iter().all(|member| {
            signals
                .iter()
                .any(|m| m.header.member.as_deref() == Some(member))
        })
    });
    let insert = signals
        .iter()
        .find(|m| {
            m.header.member.as_deref() == Some("TextChanged")
                && m.body.values().unwrap().get(3)
                    == Some(&Value::Variant(Box::new(Value::Str("🦀".into()))))
        })
        .unwrap();
    assert_eq!(
        &insert.body.values().unwrap()[..3],
        &[Value::Str("insert".into()), Value::I32(1), Value::I32(1)]
    );
    let mut w = CmdWriter::new(sid);
    w.window_close(Id::new(1, 1));
    Hal::global().submit(&w.buf());
    eventually!(
        call_accessible_get_children(&mut c, bus, ROOT, TIMEOUT)
            .unwrap()
            .len()
            == 1
    );
    let mut status = |name: &str, on: bool| {
        session
            .call(
                Message::method_call(
                    "org.a11y.Bus",
                    "/org/a11y/bus",
                    "org.freedesktop.DBus.Properties",
                    "Set",
                    Body::from_values(&[
                        Value::Str(STATUS_INTERFACE.into()),
                        Value::Str(name.into()),
                        Value::Variant(Box::new(Value::Bool(on))),
                    ])
                    .unwrap(),
                ),
                TIMEOUT,
            )
            .unwrap();
    };
    let wait_inactive = || {
        let deadline = Instant::now() + Duration::from_secs(35);
        while a11y::consumers() & makepad_a11y::consumer::OS != 0 {
            assert!(
                Instant::now() < deadline,
                "OS consumer did not deactivate after the idle grace period"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    status("IsEnabled", false);
    status("ScreenReaderEnabled", false);
    status("IsEnabled", false);
    wait_inactive();
    status("ScreenReaderEnabled", true);
    eventually!(a11y::consumers() & makepad_a11y::consumer::OS != 0);
    status("ScreenReaderEnabled", false);
    status("IsEnabled", false);
    wait_inactive();
    assert!(
        call_accessible_get_children(&mut c, "org.a11y.atspi.Registry", ROOT, TIMEOUT)
            .unwrap()
            .iter()
            .any(|registered| registered.bus == *bus),
        "registry liveness probes must keep the app registered without enabling semantics"
    );
    assert_eq!(call!(call_accessible_get_name, 2), "Pressed");
    eventually!(a11y::consumers() & makepad_a11y::consumer::OS != 0);
    println!("AT-SPI activation: IsEnabled, ScreenReaderEnabled, idle-off, query-on: passed");
    println!("AT-SPI protocol rig: two windows, Unicode text/geometry/editing, range, selection, table/cell, focus, actions, cache/events, teardown: mismatches=0 ({} signals)",signals.len());
}
