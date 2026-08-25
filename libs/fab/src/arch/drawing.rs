//! Format-neutral 2D drawing data and pan/zoom editor state.

use crate::document::{CollectionId, ObjectId};

pub type Point = [f64; 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeClass {
    Cut,
    Beyond,
    Opening,
    Crease,
    Dimension,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Line {
        points: [Point; 2],
        class: EdgeClass,
        object: Option<ObjectId>,
    },
    Polyline {
        points: Vec<Point>,
        class: EdgeClass,
        object: Option<ObjectId>,
    },
    Polygon {
        points: Vec<Point>,
        fill: [f32; 4],
        object: Option<ObjectId>,
    },
    Arc {
        center: Point,
        radius: f64,
        start: f64,
        end: f64,
        object: Option<ObjectId>,
    },
    Text {
        position: Point,
        text: String,
        object: Option<ObjectId>,
    },
    Dimension {
        from: Point,
        to: Point,
        offset: f64,
        text: String,
        object: Option<ObjectId>,
    },
    Symbol {
        position: Point,
        rotation: f64,
        kind: String,
        object: Option<ObjectId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraMarker {
    pub position: Point,
    pub heading_radians: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Drawing2D {
    pub name: String,
    pub level: Option<CollectionId>,
    pub cut_height: f64,
    pub primitives: Vec<Primitive>,
    pub camera: Option<CameraMarker>,
    pub hovered_object: Option<ObjectId>,
    pub selected_objects: Vec<ObjectId>,
}

impl Drawing2D {
    pub fn highlight(&mut self, object: Option<ObjectId>) {
        self.hovered_object = object;
    }

    pub fn object_at(&self, point: Point, tolerance: f64) -> Option<ObjectId> {
        self.primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Line { points, object, .. } => {
                    object.map(|object| (distance_to_segment(point, points[0], points[1]), object))
                }
                Primitive::Polyline { points, object, .. } => object.map(|object| {
                    let distance = points
                        .windows(2)
                        .map(|line| distance_to_segment(point, line[0], line[1]))
                        .fold(f64::INFINITY, f64::min);
                    (distance, object)
                }),
                Primitive::Symbol { position, object, .. }
                | Primitive::Text { position, object, .. } => {
                    object.map(|object| (distance(point, *position), object))
                }
                _ => None,
            })
            .filter(|(distance, _)| *distance <= tolerance)
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, object)| object)
    }
}

/// Interaction state for a `Drawing2D` area.
#[derive(Clone, Debug)]
pub struct Drawing2DEditor {
    pub pan: Point,
    pub zoom: f64,
    pub level: Option<CollectionId>,
    pub cut_height: f64,
}

impl Default for Drawing2DEditor {
    fn default() -> Self {
        Self {
            pan: [0.0, 0.0],
            zoom: 64.0,
            level: None,
            cut_height: 1.2,
        }
    }
}

impl Drawing2DEditor {
    pub fn pan_by(&mut self, delta: Point) {
        self.pan[0] += delta[0];
        self.pan[1] += delta[1];
    }

    pub fn zoom_about(&mut self, screen: Point, factor: f64) {
        let before = self.screen_to_world(screen);
        self.zoom = (self.zoom * factor).clamp(1.0, 16_384.0);
        let after = self.world_to_screen(before);
        self.pan[0] += screen[0] - after[0];
        self.pan[1] += screen[1] - after[1];
    }

    pub fn world_to_screen(&self, point: Point) -> Point {
        [point[0] * self.zoom + self.pan[0], point[1] * self.zoom + self.pan[1]]
    }

    pub fn screen_to_world(&self, point: Point) -> Point {
        [(point[0] - self.pan[0]) / self.zoom, (point[1] - self.pan[1]) / self.zoom]
    }

    /// A click in plan space becomes the horizontal part of a walk-mode target.
    pub fn teleport_target(&self, point: Point, level: f64, eye_height: f64) -> [f32; 3] {
        [point[0] as f32, point[1] as f32, (level + eye_height) as f32]
    }
}

fn distance(a: Point, b: Point) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn distance_to_segment(point: Point, from: Point, to: Point) -> f64 {
    let line = [to[0] - from[0], to[1] - from[1]];
    let length_squared = line[0] * line[0] + line[1] * line[1];
    if length_squared <= f64::EPSILON {
        return distance(point, from);
    }
    let t = (((point[0] - from[0]) * line[0] + (point[1] - from[1]) * line[1])
        / length_squared)
        .clamp(0.0, 1.0);
    distance(point, [from[0] + line[0] * t, from[1] + line[1] * t])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut editor = Drawing2DEditor::default();
        let cursor = [320.0, 240.0];
        let before = editor.screen_to_world(cursor);
        editor.zoom_about(cursor, 2.0);
        assert_eq!(editor.screen_to_world(cursor), before);
    }
}
