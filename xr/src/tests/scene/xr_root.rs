mod tests {
    use super::*;

    fn pointer_move(x: f64, y: f64) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs: dvec2(x, y),
            lock_delta: Vec2d::default(),
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: Cell::new(Area::Empty),
        })
    }

    /// A move away from the camera's viewport keeps the cursor a control
    /// set in the same move, and the move that leaves gives back the grab.
    #[test]
    fn a_camera_keeps_its_cursor_to_its_own_viewport() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut camera = XrCamera::default();
        camera.set_desktop_viewport_rect(Rect { pos: dvec2(300.0, 10.0), size: dvec2(4.0, 4.0) });

        cx.set_cursor(MouseCursor::ColResize);
        camera.handle_desktop_interaction(&mut cx, &pointer_move(700.0, 580.0));
        assert_eq!(cx.mouse_cursor(), MouseCursor::ColResize, "a move elsewhere took the cursor");

        camera.handle_desktop_interaction(&mut cx, &pointer_move(302.0, 12.0));
        assert_eq!(cx.mouse_cursor(), MouseCursor::Grab);

        camera.handle_desktop_interaction(&mut cx, &pointer_move(320.0, 12.0));
        assert_eq!(cx.mouse_cursor(), MouseCursor::Default, "leaving kept the grab");

        cx.set_cursor(MouseCursor::Grab);
        camera.handle_desktop_interaction(&mut cx, &pointer_move(302.0, 12.0));
        cx.set_cursor(MouseCursor::Hand);
        camera.handle_desktop_interaction(&mut cx, &pointer_move(320.0, 12.0));
        assert_eq!(cx.mouse_cursor(), MouseCursor::Hand, "leaving took a control's cursor");
    }

    fn sample(captured_at: f64, vertical_split: f32) -> BoxSyncPoseSample {
        BoxSyncPoseSample {
            anchor: XrAnchor {
                left: vec3f(-0.1, vertical_split * 0.5, -0.4),
                right: vec3f(0.1, -vertical_split * 0.5, -0.4),
            },
            captured_at,
            vertical_split,
        }
    }

    #[test]
    fn box_sync_detector_emits_every_full_reversal_extrema() {
        let mut runtime = XrSyncAnchorRuntime::default();
        let mut emitted = Vec::new();
        let samples = [
            sample(0.00, 0.00),
            sample(0.10, 0.03),
            sample(0.20, 0.06),
            sample(0.30, 0.03),
            sample(0.40, 0.00),
            sample(0.50, -0.03),
            sample(0.60, -0.06),
            sample(0.70, -0.03),
            sample(0.80, 0.00),
            sample(0.90, 0.03),
            sample(1.00, 0.06),
        ];

        for sample in samples {
            if let Some(sync_anchor) = runtime.update_detector_sample(Some(sample)) {
                emitted.push(sync_anchor);
            }
        }

        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[0].extrema, XrSyncAnchorExtrema::High);
        assert_eq!(emitted[0].captured_at, 0.20);
        assert_eq!(emitted[1].extrema, XrSyncAnchorExtrema::Low);
        assert_eq!(emitted[1].captured_at, 0.60);
    }
}
