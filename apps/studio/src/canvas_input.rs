//! Canvas input follows the established flow-ui face-host coordinate contract.
use crate::canvas::CanvasCamera as Camera;
use makepad_widgets::makepad_platform::event::TweakRayEvent;
use makepad_widgets::*;
use std::cell::RefCell;

/// A pointer event with its positions mapped through the inverse camera, for
/// faces laid out in canvas units; `None` for events without positions.
pub(crate) fn remap_event(event: &Event, camera: &Camera) -> Option<Event> {
    Some(match event {
        Event::MouseDown(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseDown(e)
        }
        Event::MouseMove(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            e.lock_delta /= camera.scale;
            Event::MouseMove(e)
        }
        Event::MouseUp(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseUp(e)
        }
        Event::MouseLeave(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseLeave(e)
        }
        Event::Scroll(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            e.scroll /= camera.scale;
            Event::Scroll(e)
        }
        Event::LongPress(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::LongPress(e)
        }
        Event::TouchUpdate(e) => {
            let mut e = e.clone();
            for touch in &mut e.touches {
                touch.abs = camera.screen_to_local(touch.abs);
                touch.radius /= camera.scale;
            }
            Event::TouchUpdate(e)
        }
        Event::SelectionHandleDrag(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::SelectionHandleDrag(e)
        }
        Event::Drag(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::Drag(e)
        }
        Event::Drop(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::Drop(e)
        }
        Event::TweakRay(e) => Event::TweakRay(TweakRayEvent {
            abs: camera.screen_to_local(e.abs),
            window_id: e.window_id,
            modifiers: e.modifiers,
            time: e.time,
            dpi_factor: e.dpi_factor,
            hit_widget_uids: RefCell::new(e.hit_widget_uids.borrow().clone()),
            hit_rect: std::cell::Cell::new(e.hit_rect.get()),
        }),
        _ => return None,
    })
}

/// A hit the faces claimed on the mapped clone is a hit on the original.
pub(crate) fn sync_handled(original: &Event, remapped: &Event, camera: &Camera) {
    match (original, remapped) {
        (Event::MouseDown(a), Event::MouseDown(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::MouseMove(a), Event::MouseMove(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::MouseLeave(a), Event::MouseLeave(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::Scroll(a), Event::Scroll(b)) => {
            if b.handled_x.get() {
                a.handled_x.set(true);
            }
            if b.handled_y.get() {
                a.handled_y.set(true);
            }
        }
        (Event::TouchUpdate(a), Event::TouchUpdate(b)) => {
            for (x, y) in a.touches.iter().zip(b.touches.iter()) {
                if x.handled.get().is_empty() {
                    x.handled.set(y.handled.get());
                }
                if x.sweep_lock.get().is_empty() {
                    x.sweep_lock.set(y.sweep_lock.get());
                }
            }
        }
        (Event::TweakRay(a), Event::TweakRay(b)) => {
            *a.hit_widget_uids.borrow_mut() = b.hit_widget_uids.borrow().clone();
            a.hit_rect.set(b.hit_rect.get().map(|rect| Rect {
                pos: camera.local_to_screen(rect.pos),
                size: rect.size * camera.scale,
            }));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::event::{
        ScrollEvent, ScrollPhase, TouchPoint, TouchState, TouchUpdateEvent,
    };
    use std::cell::Cell;

    fn camera(scale: f64) -> Camera {
        let mut camera = Camera::default();
        camera.view = Rect {
            pos: dvec2(100.0, 60.0),
            size: dvec2(900.0, 600.0),
        };
        camera.pan = dvec2(40.0, -20.0);
        camera.scale = scale;
        camera
    }

    fn claimed_areas() -> (Area, Area) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = makepad_widgets::makepad_platform::DrawList::new(&mut cx);
        // Only equality/propagation is tested; no GPU or hit-testing is needed.
        let area = |rect_id| {
            Area::Rect(makepad_widgets::makepad_platform::RectArea {
                draw_list_id: list.id(),
                rect_id,
                redraw_id: 1,
            })
        };
        (area(0), area(1))
    }

    #[test]
    fn mouse_press_and_release_share_the_inverse_camera_at_nonunit_zoom() {
        for scale in [0.25, 2.0, 8.0] {
            let camera = camera(scale);
            let original = Event::MouseDown(MouseDownEvent {
                abs: dvec2(340.0, 240.0),
                button: MouseButton::PRIMARY,
                window_id: WindowId(7, 1),
                modifiers: KeyModifiers::default(),
                handled: Cell::new(Area::Empty),
                time: 3.0,
            });
            let Event::MouseDown(mapped) = remap_event(&original, &camera).unwrap() else {
                panic!("wrong event");
            };
            assert_eq!(
                mapped.abs,
                dvec2(32768.0 + 200.0 / scale, 32768.0 + 200.0 / scale)
            );
            assert_eq!(mapped.window_id, WindowId(7, 1));
            let release = Event::MouseUp(MouseUpEvent {
                abs: dvec2(350.0, 260.0),
                button: MouseButton::PRIMARY,
                window_id: WindowId(7, 1),
                modifiers: KeyModifiers::default(),
                time: 4.0,
            });
            let Event::MouseUp(mapped) = remap_event(&release, &camera).unwrap() else {
                panic!("wrong event");
            };
            assert_eq!(
                mapped.abs,
                dvec2(32768.0 + 210.0 / scale, 32768.0 + 220.0 / scale)
            );
            assert_eq!(mapped.time, 4.0);
        }
    }

    #[test]
    fn scroll_scales_position_and_delta_and_propagates_both_handled_axes() {
        let camera = camera(0.5);
        let original = Event::Scroll(ScrollEvent {
            window_id: WindowId(7, 1),
            abs: dvec2(340.0, 240.0),
            scroll: dvec2(10.0, -20.0),
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(true),
            handled_y: Cell::new(false),
            is_mouse: false,
            time: 3.0,
            phase: ScrollPhase::Changed,
        });
        let mapped = remap_event(&original, &camera).unwrap();
        let Event::Scroll(scroll) = &mapped else {
            panic!("wrong event");
        };
        assert_eq!(scroll.abs, dvec2(33168.0, 33168.0));
        assert_eq!(scroll.scroll, dvec2(20.0, -40.0));
        assert_eq!(scroll.phase, ScrollPhase::Changed);
        scroll.handled_x.set(false);
        scroll.handled_y.set(true);
        sync_handled(&original, &mapped, &camera);
        let Event::Scroll(original) = original else {
            unreachable!()
        };
        assert!(
            original.handled_x.get(),
            "a previous handler must be preserved"
        );
        assert!(
            original.handled_y.get(),
            "a child claim must reach the screen event"
        );
    }

    #[test]
    fn mapped_mouse_claim_reaches_the_original_without_overriding_an_owner() {
        let camera = camera(2.0);
        let (child, previous) = claimed_areas();
        let original = Event::MouseDown(MouseDownEvent {
            abs: dvec2(340.0, 240.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(7, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 3.0,
        });
        let mapped = remap_event(&original, &camera).unwrap();
        let Event::MouseDown(down) = &mapped else {
            panic!("wrong event");
        };
        down.handled.set(child);
        sync_handled(&original, &mapped, &camera);
        let Event::MouseDown(down) = &original else {
            unreachable!()
        };
        assert_eq!(down.handled.get(), child);
        down.handled.set(previous);
        sync_handled(&original, &mapped, &camera);
        assert_eq!(down.handled.get(), previous);
    }

    #[test]
    fn touch_radius_position_and_capture_claim_use_the_same_camera() {
        let camera = camera(2.0);
        let (child, sweep) = claimed_areas();
        let original = Event::TouchUpdate(TouchUpdateEvent {
            window_id: WindowId(7, 1),
            time: 3.0,
            modifiers: KeyModifiers::default(),
            touches: vec![TouchPoint {
                state: TouchState::Start,
                abs: dvec2(340.0, 240.0),
                time: 3.0,
                uid: 42,
                rotation_angle: 0.0,
                force: 1.0,
                radius: dvec2(10.0, 14.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty),
            }],
        });
        let mapped = remap_event(&original, &camera).unwrap();
        let Event::TouchUpdate(touch) = &mapped else {
            panic!("wrong event");
        };
        assert_eq!(touch.touches[0].abs, dvec2(32868.0, 32868.0));
        assert_eq!(touch.touches[0].radius, dvec2(5.0, 7.0));
        touch.touches[0].handled.set(child);
        touch.touches[0].sweep_lock.set(sweep);
        sync_handled(&original, &mapped, &camera);
        let Event::TouchUpdate(touch) = &original else {
            unreachable!()
        };
        assert_eq!(touch.touches[0].handled.get(), child);
        assert_eq!(touch.touches[0].sweep_lock.get(), sweep);
    }
}
