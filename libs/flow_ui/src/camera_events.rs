//! Window input for graph faces laid out in canvas units.

use makepad_canvas::InputCamera;
use makepad_flowgraph::Camera;
use makepad_widgets::{DVec2, Event};

/// Borrow the graph camera so both input directions use its active render origin.
struct FaceCamera<'a>(&'a Camera);

impl InputCamera for FaceCamera<'_> {
    fn screen_to_local(&self, point: DVec2) -> DVec2 {
        self.0.screen_to_local(point)
    }

    fn local_to_screen(&self, point: DVec2) -> DVec2 {
        self.0.local_to_screen(point)
    }

    fn scale(&self) -> f64 {
        self.0.scale
    }
}

/// Map a pointer event into face coordinates.
pub fn remap_event(event: &Event, camera: &Camera) -> Option<Event> {
    makepad_canvas::remap_event(event, &FaceCamera(camera))
}

/// Propagate a face's input claims back to the window event.
pub fn sync_handled(original: &Event, remapped: &Event, camera: &Camera) {
    makepad_canvas::sync_handled(original, remapped, &FaceCamera(camera));
}
