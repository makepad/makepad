//! Number-field interaction through the same finger/key path as a window.
use makepad_widgets::makepad_draw::cx_draw::CxDraw;
use makepad_widgets::makepad_draw::event::{ScrollEvent, ScrollPhase};
use makepad_widgets::test_cx::{checkout_test_cx, on_test_cx, PooledCx};
use makepad_widgets::*;
use std::cell::Cell;

const WINDOW: WindowId = WindowId(1, 1);
const SIZE: Vec2d = Vec2d { x: 600.0, y: 300.0 };

struct Scene {
    cx: PooledCx,
    root: WidgetRef,
    pass: DrawPass,
    list: DrawList2d,
    press: Option<Event>,
    time: f64,
}

impl Scene {
    fn new() -> Self {
        let mut cx = checkout_test_cx(makepad_widgets_core::script_mod);
        cx.with_vm(|vm| vm.capture_errors());
        let root = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                View{
                    width: Fill height: Fill flow: Down
                    first := ValueInput{width: 200 min: 0 max: 100 step: 1 precision: 1}
                    second := ValueInput{width: 200 min: 0 max: 100 step: 1 precision: 1}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let errors = cx.with_vm(|vm| vm.take_errors());
        assert!(errors.is_empty(), "{errors:?}");
        let pass = DrawPass::new(&mut cx);
        let list = DrawList2d::new(&mut cx);
        let mut scene = Self { cx, root, pass, list, press: None, time: 0.0 };
        scene.draw();
        scene.field(id!(first)).set_value(&mut scene.cx, 10.0);
        scene.field(id!(second)).set_value(&mut scene.cx, 20.0);
        scene
    }

    fn field(&mut self, id: LiveId) -> ValueInputRef {
        self.root.value_input(&mut self.cx, &[id])
    }

    fn center(&mut self, id: LiveId) -> Vec2d {
        let area = self.field(id).area();
        let face = area.rect(&self.cx);
        face.pos + face.size * 0.5
    }

    fn send(&mut self, event: Event) -> ActionsBuf {
        self.cx.capture_actions(|cx| self.root.handle_event(cx, &event, &mut Scope::empty()))
    }

    fn settle(&mut self) -> ActionsBuf {
        let mut actions = Vec::new();
        loop {
            let prev = self.cx.key_focus();
            self.cx.action(());
            self.cx.handle_actions();
            let focus = self.cx.key_focus();
            if focus == prev { break; }
            actions.extend(self.send(Event::KeyFocus(KeyFocusEvent { prev, focus })));
        }
        actions
    }

    fn draw(&mut self) {
        self.pass.set_size(&mut self.cx, SIZE);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut self.cx, &event);
        let mut cx = Cx2d::new(&mut draw);
        cx.begin_pass(&self.pass, None);
        self.list.begin_always(&mut cx);
        cx.begin_root_turtle(SIZE, Layout::flow_down());
        self.root.draw_all(&mut cx, &mut Scope::empty());
        cx.end_pass_sized_turtle();
        self.list.end(&mut cx);
        cx.end_pass(&self.pass);
        drop(cx);
        drop(draw);
        self.settle();
    }

    fn down(&mut self, abs: Vec2d) -> ActionsBuf {
        self.time += 1.0;
        self.cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let event = Event::MouseDown(MouseDownEvent {
            abs, button: MouseButton::PRIMARY, window_id: WINDOW,
            modifiers: KeyModifiers::default(), handled: Cell::new(Area::Empty), time: self.time,
        });
        let actions = self.cx.capture_actions(|cx| self.root.handle_event(cx, &event, &mut Scope::empty()));
        self.press = Some(event);
        self.settle();
        actions
    }

    fn motion(&mut self, abs: Vec2d, modifiers: KeyModifiers) -> ActionsBuf {
        self.time += 0.1;
        self.send(Event::MouseMove(MouseMoveEvent {
            abs, lock_delta: Vec2d::default(), window_id: WINDOW, modifiers,
            handled: Cell::new(Area::Empty), time: self.time,
        }))
    }

    fn up(&mut self, abs: Vec2d) -> ActionsBuf {
        self.time += 0.1;
        let actions = self.send(Event::MouseUp(MouseUpEvent {
            abs, button: MouseButton::PRIMARY, window_id: WINDOW,
            modifiers: KeyModifiers::default(), time: self.time,
        }));
        if let Some(event) = self.press.take() {
            if let Event::MouseDown(down) = &event {
                event.unhandle(&mut self.cx, &down.handled.get());
            }
        }
        self.cx.fingers.first_mouse_button = None;
        self.settle();
        actions
    }

    fn key(&mut self, key_code: KeyCode, modifiers: KeyModifiers) -> ActionsBuf {
        self.send(Event::KeyDown(KeyEvent { key_code, modifiers, is_repeat: false, time: self.time }))
    }

    fn type_text(&mut self, text: &str) -> ActionsBuf {
        self.send(Event::TextInput(TextInputEvent { input: text.to_string(), ..Default::default() }))
    }

    fn edit(&mut self, id: LiveId) {
        let at = self.center(id);
        self.down(at);
        self.up(at);
        self.draw();
        let field = self.field(id);
        assert!(field.is_editing());
        assert!(field.key_focus(&self.cx));
    }
}

fn events(actions: &Actions, field: &ValueInputRef) -> Vec<ValueInputAction> {
    actions.filter_widget_actions_cast::<ValueInputAction>(field.widget_uid()).collect()
}

#[test]
fn scrub_engages_horizontally_reanchors_modifiers_and_ends_once() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        let at = scene.center(id!(first));
        assert!(events(&scene.down(at), &field).is_empty());
        assert!(events(&scene.motion(at + dvec2(0.0, 40.0), Default::default()), &field).is_empty());
        assert!(events(&scene.motion(at + dvec2(3.0, 0.0), Default::default()), &field).is_empty());
        assert_eq!(events(&scene.motion(at + dvec2(13.0, 0.0), Default::default()), &field),
            vec![ValueInputAction::Changed(20.0)]);
        let fine = KeyModifiers { shift: true, ..Default::default() };
        scene.motion(at + dvec2(13.0, 0.0), fine);
        assert_eq!(field.value(), 20.0, "changing the rate must not move the value");
        assert_eq!(events(&scene.motion(at + dvec2(23.0, 0.0), fine), &field),
            vec![ValueInputAction::Changed(21.0)]);
        field.set_value(&mut scene.cx, 80.0);
        assert_eq!(field.value(), 21.0, "external updates yield to the gesture");
        assert_eq!(events(&scene.up(at + dvec2(23.0, 0.0)), &field), vec![ValueInputAction::Ended(21.0)]);
        assert!(events(&scene.up(at), &field).is_empty());
    });
}

#[test]
fn typing_replaces_the_selection_clamps_and_commits_once() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        scene.edit(id!(first));
        assert!(events(&scene.type_text("150.75"), &field).is_empty());
        field.set_value(&mut scene.cx, 80.0);
        assert_eq!(field.value(), 10.0);
        let actions = scene.key(KeyCode::ReturnKey, Default::default());
        assert_eq!(events(&actions, &field), vec![ValueInputAction::Changed(100.0), ValueInputAction::Ended(100.0)]);
        assert_eq!(field.changed(&actions), Some(100.0));
        assert_eq!(field.ended(&actions), Some(100.0));
        assert!(!field.is_editing());
        assert!(events(&scene.key(KeyCode::ReturnKey, Default::default()), &field).is_empty());
    });
}

#[test]
fn opening_and_committing_preserves_full_precision() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        let value = 12.3456789012345;
        field.set_value(&mut scene.cx, value);
        scene.edit(id!(first));
        let actions = scene.key(KeyCode::ReturnKey, Default::default());
        assert_eq!(field.changed(&actions), Some(value));
        assert_eq!(field.ended(&actions), Some(value));
    });
}

#[test]
fn escape_cancels_text_silently_and_restores_a_drag_with_one_end() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        scene.edit(id!(first));
        scene.type_text("75");
        assert!(events(&scene.key(KeyCode::Escape, Default::default()), &field).is_empty());
        assert_eq!(field.value(), 10.0);
        assert!(!field.is_editing());
        scene.draw();
        let at = scene.center(id!(first));
        scene.down(at);
        scene.motion(at + dvec2(3.0, 0.0), Default::default());
        scene.motion(at + dvec2(13.0, 0.0), Default::default());
        assert_eq!(field.value(), 20.0);
        assert_eq!(events(&scene.key(KeyCode::Escape, Default::default()), &field),
            vec![ValueInputAction::Changed(10.0), ValueInputAction::Ended(10.0)]);
        assert!(events(&scene.up(at), &field).is_empty());
    });
}

#[test]
fn right_press_cancels_a_drag() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        let at = scene.center(id!(first));
        scene.down(at);
        scene.motion(at + dvec2(3.0, 0.0), Default::default());
        scene.motion(at + dvec2(13.0, 0.0), Default::default());
        let actions = scene.send(Event::MouseDown(MouseDownEvent {
            abs: at, button: MouseButton::SECONDARY, window_id: WINDOW,
            modifiers: KeyModifiers::default(), handled: Cell::new(Area::Empty), time: 2.0,
        }));
        assert_eq!(events(&actions, &field), vec![ValueInputAction::Changed(10.0), ValueInputAction::Ended(10.0)]);
        assert!(events(&scene.up(at), &field).is_empty());
    });
}

#[test]
fn tab_commits_and_opens_the_next_field_with_all_text_selected() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let first = scene.field(id!(first));
        let second = scene.field(id!(second));
        scene.edit(id!(first));
        scene.type_text("35");
        assert_eq!(events(&scene.key(KeyCode::Tab, Default::default()), &first),
            vec![ValueInputAction::Changed(35.0), ValueInputAction::Ended(35.0)]);
        scene.draw();
        assert!(!first.is_editing());
        assert!(second.is_editing() && second.key_focus(&scene.cx));
        scene.type_text("7");
        assert_eq!(events(&scene.key(KeyCode::Tab, KeyModifiers { shift: true, ..Default::default() }), &second),
            vec![ValueInputAction::Changed(7.0), ValueInputAction::Ended(7.0)]);
        scene.draw();
        assert!(first.is_editing() && first.key_focus(&scene.cx));
        scene.type_text("6");
        scene.key(KeyCode::ReturnKey, Default::default());
        assert_eq!(first.value(), 6.0, "Shift+Tab also selects the entire previous value");
    });
}

#[test]
fn losing_focus_commits_the_edit() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        scene.edit(id!(first));
        scene.type_text("32");
        scene.cx.set_key_focus(Area::Empty);
        assert_eq!(events(&scene.settle(), &field), vec![ValueInputAction::Changed(32.0), ValueInputAction::Ended(32.0)]);
        assert!(!field.is_editing());
    });
}

#[test]
fn wheel_requires_key_focus_even_with_control_held() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        let at = scene.center(id!(first));
        for control in [false, true] {
            let actions = scene.send(Event::Scroll(ScrollEvent {
                window_id: WINDOW, scroll: dvec2(0.0, -120.0), abs: at,
                modifiers: KeyModifiers { control, ..Default::default() },
                handled_x: Cell::new(false), handled_y: Cell::new(false), is_mouse: true,
                time: 0.0, phase: ScrollPhase::Changed,
            }));
            assert!(events(&actions, &field).is_empty());
            assert_eq!(field.value(), 10.0);
        }
        scene.cx.set_key_focus(field.area());
        scene.settle();
        let actions = scene.send(Event::Scroll(ScrollEvent {
            window_id: WINDOW, scroll: dvec2(0.0, -120.0), abs: at,
            modifiers: KeyModifiers::default(), handled_x: Cell::new(false),
            handled_y: Cell::new(false), is_mouse: true, time: 0.0, phase: ScrollPhase::Changed,
        }));
        assert_eq!(events(&actions, &field), vec![ValueInputAction::Changed(11.0), ValueInputAction::Ended(11.0)]);
    });
}

#[test]
fn bounded_mapping_clamping_and_wrapping_keep_the_pointer_attached() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        field.borrow_mut().unwrap().show_fill = true;
        let at = scene.center(id!(first));
        scene.down(at);
        scene.motion(at + dvec2(3.0, 0.0), Default::default());
        scene.motion(at + dvec2(23.0, 0.0), Default::default());
        assert_eq!(field.value(), 20.0, "200 points cover the 100-unit soft range");
        scene.motion(at + dvec2(500.0, 0.0), Default::default());
        assert_eq!(field.value(), 100.0);
        scene.motion(at + dvec2(499.0, 0.0), Default::default());
        assert_eq!(field.value(), 99.5, "reversing leaves a clamped limit immediately");
        scene.up(at);
        field.borrow_mut().unwrap().wrap = true;
        field.set_value(&mut scene.cx, 99.0);
        scene.down(at);
        scene.motion(at + dvec2(3.0, 0.0), Default::default());
        scene.motion(at + dvec2(7.0, 0.0), Default::default());
        assert_eq!(field.value(), 1.0);
        scene.up(at);
    });
}

#[test]
fn control_snap_and_finer_snap_reanchor_without_a_modifier_jump() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        let at = scene.center(id!(first));
        scene.down(at);
        scene.motion(at + dvec2(3.0, 0.0), Default::default());
        scene.motion(at + dvec2(7.0, 0.0), Default::default());
        assert_eq!(field.value(), 14.0);
        let snap = KeyModifiers { control: true, ..Default::default() };
        scene.motion(at + dvec2(7.0, 0.0), snap);
        assert_eq!(field.value(), 14.0);
        scene.motion(at + dvec2(9.0, 0.0), snap);
        assert_eq!(field.value(), 20.0);
        let fine = KeyModifiers { control: true, shift: true, ..Default::default() };
        scene.motion(at + dvec2(9.0, 0.0), fine);
        assert_eq!(field.value(), 20.0);
        scene.motion(at + dvec2(19.0, 0.0), fine);
        assert_eq!(field.value(), 21.0);
        scene.motion(at + dvec2(19.0, 0.0), Default::default());
        assert_eq!(field.value(), 21.0);
        scene.motion(at + dvec2(20.0, 0.0), Default::default());
        assert_eq!(field.value(), 22.0);
        scene.up(at);
    });
}

#[test]
fn bounded_shift_is_twenty_times_finer() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        field.borrow_mut().unwrap().show_fill = true;
        let at = scene.center(id!(first));
        scene.down(at);
        scene.motion(at + dvec2(3.0, 0.0), Default::default());
        let fine = KeyModifiers { shift: true, ..Default::default() };
        scene.motion(at + dvec2(3.0, 0.0), fine);
        scene.motion(at + dvec2(43.0, 0.0), fine);
        assert_eq!(field.value(), 11.0);
        scene.up(at);
    });
}

#[test]
fn step_click_and_focused_keys_each_end_once() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        let face = field.area().rect(&scene.cx);
        let left = dvec2(face.pos.x + 5.0, face.pos.y + face.size.y * 0.5);
        scene.down(left);
        assert_eq!(events(&scene.up(left), &field), vec![ValueInputAction::Changed(9.0), ValueInputAction::Ended(9.0)]);
        assert_eq!(events(&scene.key(KeyCode::ArrowUp, Default::default()), &field),
            vec![ValueInputAction::Changed(10.0), ValueInputAction::Ended(10.0)]);
        assert_eq!(events(&scene.key(KeyCode::ArrowDown, Default::default()), &field),
            vec![ValueInputAction::Changed(9.0), ValueInputAction::Ended(9.0)]);
    });
}

#[test]
fn read_only_fields_keep_their_value_and_mixed_fields_show_a_dash() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        let field = scene.field(id!(first));
        {
            let mut value = field.borrow_mut().unwrap();
            value.read_only = true;
            value.mixed = true;
        }
        scene.draw();
        let at = scene.center(id!(first));
        assert!(events(&scene.down(at), &field).is_empty());
        assert!(events(&scene.motion(at + dvec2(30.0, 0.0), Default::default()), &field).is_empty());
        assert!(events(&scene.up(at), &field).is_empty());
        assert!(!field.is_editing());
        assert_eq!(field.value(), 10.0);
        assert_eq!(field.text(), "—");
    });
}

#[test]
fn stacked_rows_align_columns_reserve_trails_and_join_only_outer_corners() {
    on_test_cx(|| {
        let mut scene = Scene::new();
        scene.cx.with_vm(|vm| vm.capture_errors());
        scene.root = scene.cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                PropStack{
                    width: 400
                    x := PropNumberRow{label +: {text: "Location X"}}
                    y := PropNumberRow{label +: {text: "Y"} trail +: {reset := PropTrailButton{}}}
                    z := PropNumberRow{label +: {text: "Z"}}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let errors = scene.cx.with_vm(|vm| vm.take_errors());
        assert!(errors.is_empty(), "{errors:?}");
        scene.draw();
        let mut positions = Vec::new();
        for (id, join) in [(id!(x), ValueInputJoin::Top), (id!(y), ValueInputJoin::None), (id!(z), ValueInputJoin::Bottom)] {
            let row = scene.root.widget(&scene.cx, &[id]);
            let label = row.widget(&scene.cx, ids!(label)).area().rect(&scene.cx);
            let number = row.value_input(&scene.cx, ids!(value.num));
            let value = number.area().rect(&scene.cx);
            let trail = row.widget(&scene.cx, ids!(trail)).area().rect(&scene.cx);
            assert_eq!(label.size.x, 120.0, "a wide row gets the full label column");
            assert_eq!(value.pos.x - label.pos.x - label.size.x, 6.0);
            assert_eq!(trail.size.x, 16.0);
            assert_eq!(value.size.y, 20.0);
            assert_eq!(number.borrow().unwrap().join, join);
            positions.push((value.pos.x, value.size.x, value.pos.y));
        }
        assert_eq!(positions[0].0, positions[1].0);
        assert_eq!(positions[0].1, positions[1].1, "the reset button cannot move the value column");
        assert_eq!(positions[1].2 - positions[0].2, 21.0);
        assert_eq!(positions[2].2 - positions[1].2, 21.0);
    });
}
