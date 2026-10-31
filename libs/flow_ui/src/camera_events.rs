//! Pointer events mapped through a canvas camera, for widgets laid out in
//! canvas units (graph faces), and the handled flags copied back.

use makepad_flowgraph::Camera;
use makepad_widgets::makepad_platform::event::TweakRayEvent;
use makepad_widgets::*;
use std::cell::RefCell;

/// A pointer event with its positions mapped through the inverse camera, for
/// faces laid out in canvas units; `None` for events without positions.
pub fn remap_event(event: &Event, camera: &Camera) -> Option<Event> {
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
pub fn sync_handled(original: &Event, remapped: &Event, camera: &Camera) {
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
