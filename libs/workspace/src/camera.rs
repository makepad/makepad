//! The camera every zoomable presentation projects through, and the
//! world-space rectangle it projects.

use makepad_widgets::makepad_micro_serde::*;

pub const MIN_ZOOM: f64 = 0.00001;
pub const MAX_ZOOM: f64 = 8.0;
const MAX_COORD: f64 = 10_000_000.0;

/// Pan is in viewport pixels; world coordinates are in points.
/// screen = viewport_origin + pan + world * zoom.
#[derive(Clone, Copy, Debug, PartialEq, SerRon, DeRon)]
pub struct Camera {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan_x: 32.0,
            pan_y: 32.0,
            zoom: 0.7,
        }
    }
}

impl Camera {
    pub fn is_valid(self) -> bool {
        self.pan_x.is_finite()
            && self.pan_y.is_finite()
            && self.zoom.is_finite()
            && self.pan_x.abs() <= MAX_COORD
            && self.pan_y.abs() <= MAX_COORD
            && (MIN_ZOOM..=MAX_ZOOM).contains(&self.zoom)
    }
}

/// A world-space rectangle a zoomable presentation projects through its camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

