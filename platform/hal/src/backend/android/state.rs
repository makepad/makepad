//! JNI-free surface identity and MotionEvent translation, shared by the device and host tests.
use crate::{input, TouchPoint};

#[derive(Debug, PartialEq, Eq)]
pub enum SurfaceStatus { Lost, Native { generation: u64, size: [u32; 2] } }

#[derive(Debug, Default)]
pub struct SurfaceState {
    pub generation: u64,
    pub window: Option<usize>,
    pub size: [u32; 2],
}
impl SurfaceState {
    pub fn status(&self) -> SurfaceStatus {
        if self.window.is_some() { SurfaceStatus::Native { generation: self.generation, size: self.size } }
        else { SurfaceStatus::Lost }
    }
    pub fn created(&mut self, window: usize, size: [u32; 2]) {
        self.generation += 1;
        self.window = Some(window);
        self.size = size;
    }
    pub fn changed(&mut self, window: usize, size: [u32; 2]) {
        if self.window != Some(window) { self.created(window, size); }
        else { self.size = size; }
    }
    pub fn destroyed(&mut self) { self.window = None; }
}

#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// Android positions are physical pixels; HAL input uses window points. History is passed
/// as ACTION_MOVE, preserving its own uptime-nanosecond timestamp in the enclosing event.
pub fn touches(action: i32, index: usize, pointers: &[Pointer], dpi: f32) -> Vec<TouchPoint> {
    if !matches!(action, 0 | 1 | 2 | 3 | 5 | 6) { return Vec::new(); }
    pointers.iter().enumerate().map(|(i, p)| TouchPoint {
        id: p.id,
        phase: if action == 3 { input::touch::STOP }
            else if action == 2 { input::touch::MOVE }
            else if i != index { input::touch::STABLE }
            else if matches!(action, 0 | 5) { input::touch::START }
            else { input::touch::STOP },
        x: p.x / dpi, y: p.y / dpi, force: p.pressure,
    }).collect()
}

#[derive(Debug, PartialEq)]
pub struct TouchSample { pub time_ns: u64, pub cancelled: bool, pub points: Vec<TouchPoint> }

pub fn sample(action: i32, index: usize, pointers: &[Pointer], dpi: f32, time_ns: u64, historical: bool) -> TouchSample {
    TouchSample { time_ns, cancelled: action == 3 && !historical, points: touches(if historical { 2 } else { action }, index, pointers, dpi) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surface_lifetime() {
        let mut s = SurfaceState::default();
        assert_eq!(s.status(), SurfaceStatus::Lost);
        s.created(7, [100, 200]);
        assert_eq!(s.status(), SurfaceStatus::Native { generation: 1, size: [100, 200] });
        s.changed(7, [200, 100]);
        assert_eq!((s.generation, s.size), (1, [200, 100]));
        s.changed(8, [300, 100]);
        assert_eq!(s.generation, 2);
        s.destroyed();
        assert_eq!(s.status(), SurfaceStatus::Lost);
        s.created(8, [300, 100]);
        assert_eq!(s.generation, 3);
        s.created(8, [300, 100]);
        assert_eq!(s.generation, 4);
    }
    #[test]
    fn motion_pointer_transitions_and_history() {
        let p = [Pointer { id: 42, x: 12., y: 18., pressure: 0.7 }, Pointer { id: 9, x: 20., y: 30., pressure: 1. }];
        let start = touches(5, 1, &p, 2.);
        assert_eq!((start[0].phase, start[1].phase), (input::touch::STABLE, input::touch::START));
        assert_eq!((start[0].id, start[0].x, start[0].y, start[0].force), (42, 6., 9., 0.7));
        assert_eq!(touches(6, 0, &p, 2.)[0].phase, input::touch::STOP);
        assert!(touches(3, 1, &p, 2.).iter().all(|p| p.phase == input::touch::STOP));
        assert!(touches(2, 0, &p, 2.).iter().all(|p| p.phase == input::touch::MOVE));
        assert!(sample(3, 0, &p, 2., 17, false).cancelled);
        assert!(!sample(1, 0, &p, 2., 18, false).cancelled);
        let history = sample(5, 1, &p, 2., 123_456_789, true);
        let current = sample(5, 1, &p, 2., 123_456_799, false);
        assert_eq!((history.time_ns, current.time_ns), (123_456_789, 123_456_799));
        assert!(history.points.iter().all(|p| p.phase == input::touch::MOVE));
        assert_eq!(current.points[1].phase, input::touch::START);
        assert!(touches(8, 0, &p, 2.).is_empty());
        assert!(touches(0, 0, &[], 2.).is_empty());
    }
}
