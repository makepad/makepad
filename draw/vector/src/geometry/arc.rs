use std::f64::consts::{PI, TAU};

use crate::geometry::{Point, Transform, Transformation};
use crate::internal_iter::InternalIterator;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Arc {
    pub from: Point,
    pub to: Point,
    pub radius: Point,
    pub x_axis_rotation: f64,
    pub large_arc: bool,
    pub sweep: bool,

    center: Point,
    transformed_point: Point,
    transformed_center: Point,
    start_angle: f64,
    sweep_angle: f64,
    end_angle: f64,
    rx: f64,
    ry: f64,
}

impl Arc {
    // See http://www.w3.org/TR/SVG/implnote.html#ArcParameterizationAlternatives
    // for how we calculate center point, start and end angle
    pub fn new(
        from: Point,
        to: Point,
        radius: Point,
        x_axis_rotation: f64,
        large_arc: bool,
        sweep: bool,
    ) -> Arc {
        // If the endpoints are identical, then this is equivalent to omitting the elliptical arc
        // segment entirely.
        if from.x == to.x && from.y == to.y {
            return Arc {
                from,
                to,
                radius,
                x_axis_rotation,
                large_arc,
                sweep,
                ..Default::default()
            };
        }

        let mut rx = radius.x.abs();
        let mut ry = radius.y.abs();
        if rx == 0.0 || ry == 0.0 {
            // Effectively a line
            return Arc {
                from,
                to,
                radius,
                x_axis_rotation,
                large_arc,
                sweep,
                ..Default::default()
            };
        }

        let x_axis_rotation = x_axis_rotation % 360.0;
        let x_axis_rotation_radians = x_axis_rotation * (PI / 180.0);

        // Step #1: Compute transformedPoint
        let dx = (from.x - to.x) / 2.0;
        let dy = (from.y - to.y) / 2.0;
        let transformed_point = Point {
            x: x_axis_rotation_radians.cos() * dx + x_axis_rotation_radians.sin() * dy,
            y: -x_axis_rotation_radians.sin() * dx + x_axis_rotation_radians.cos() * dy,
        };

        // Ensure radii are large enough
        let radii_check =
            transformed_point.x.powi(2) / rx.powi(2) + transformed_point.y.powi(2) / ry.powi(2);
        if radii_check > 1.0 {
            rx *= radii_check.sqrt();
            ry *= radii_check.sqrt();
        }

        // Step #2: Compute transformedCenter
        let c_square_numerator = rx.powi(2) * ry.powi(2)
            - rx.powi(2) * transformed_point.y.powi(2)
            - ry.powi(2) * transformed_point.x.powi(2);
        let c_square_denominator =
            rx.powi(2) * transformed_point.y.powi(2)
            + ry.powi(2) * transformed_point.x.powi(2);
        let c_radicand = (c_square_numerator / c_square_denominator).max(0.0);
        let c_coefficient = if sweep != large_arc {
            1.0
        } else {
            -1.0
        } * c_radicand.sqrt();
        let transformed_center = Point {
            x: c_coefficient * ((rx * transformed_point.y) / ry),
            y: c_coefficient * (-(ry * transformed_point.x) / rx),
        };

        let center = Point {
            x: x_axis_rotation_radians.cos() * transformed_center.x
                - x_axis_rotation_radians.sin() * transformed_center.y
                + ((from.x + to.x) / 2.0),
            y: x_axis_rotation_radians.sin() * transformed_center.x
                + x_axis_rotation_radians.cos() * transformed_center.y
                + ((from.y + to.y) / 2.0),
        };

        // Step #4: Compute start/sweep angles
        // Start angle of the elliptical arc prior to the stretch and rotate operations.
        // Difference between the start and end angles
        let start_vector = Point {
            x: (transformed_point.x - transformed_center.x) / rx,
            y: (transformed_point.y - transformed_center.y) / ry,
        };
        let start_angle = angle_between(Point { x: 1.0, y: 0.0 }, start_vector);
        debug_assert!(!start_angle.is_nan());

        let end_vector = Point {
            x: (-transformed_point.x - transformed_center.x) / rx,
            y: (-transformed_point.y - transformed_center.y) / ry,
        };
        let mut sweep_angle = angle_between(start_vector, end_vector);

        if !sweep && sweep_angle > 0.0 {
            sweep_angle -= TAU;
        } else if sweep && sweep_angle < 0.0 {
            sweep_angle += TAU;
        }
        sweep_angle %= TAU;
        let end_angle = start_angle + sweep_angle;

        Arc {
            from,
            to,
            radius,
            x_axis_rotation,
            large_arc,
            sweep,

            center,
            transformed_point,
            transformed_center,
            start_angle,
            sweep_angle,
            end_angle,
            rx,
            ry,
        }
    }

    /// Returns true if `self` is approximately linear with tolerance `epsilon`.
    pub fn is_approximately_linear(&self, epsilon: f64) -> bool {
        let center = self.from.lerp(self.to, 0.5);
        let midpoint = self.midpoint();

        let dx = midpoint.x - center.x;
        let dy = midpoint.y - center.y;
        let sagitta = (dx.powi(2) + dy.powi(2)).sqrt();

        sagitta < epsilon
    }

    pub fn point_on_curve(&self, t: f64) -> Point {
        debug_assert!(t >= 0.0 && t <= 1.0);

        let angle = self.start_angle + self.sweep_angle * t;
        let ellipse_component_x = self.rx * angle.cos();
        let ellipse_component_y = self.ry * angle.sin();
        let x_axis_rotation_radians = self.x_axis_rotation * (PI / 180.0);
        Point {
            x: x_axis_rotation_radians.cos() * ellipse_component_x
                - x_axis_rotation_radians.sin() * ellipse_component_y
                + self.center.x,
            y: x_axis_rotation_radians.sin() * ellipse_component_x
                + x_axis_rotation_radians.cos() * ellipse_component_y
                + self.center.y,
        }
    }

    pub fn midpoint(&self) -> Point {
        self.point_on_curve(0.5)
    }

    // Function to split the arc at parameter `t`
    pub fn split(&self, t: f64) -> (Arc, Arc) {
        let split_at = self.point_on_curve(t);
        let angle = self.start_angle + self.sweep_angle * t;

        (Arc::new(
            self.from,
            split_at,
            self.radius,
            self.x_axis_rotation,
            (angle - self.start_angle) > PI,
            self.sweep,
        ),
        Arc::new(
            split_at,
            self.to,
            self.radius,
            self.x_axis_rotation,
            (self.end_angle - angle) > PI,
            self.sweep,
        ))
    }

    pub fn linearize(self, epsilon: f64) -> Linearize {
        Linearize {
            segment: self,
            epsilon,
        }
    }
}

fn angle_between(v0: Point, v1: Point) -> f64 {
    let adjacent = v0.x * v1.x + v0.y * v1.y;
    let hypotenuse = (
        (v0.x.powi(2) + v0.y.powi(2)) * (v1.x.powi(2) + v1.y.powi(2))
    ).sqrt();
    let sign = if v0.x * v1.y - v0.y * v1.x < 0.0 {
        -1.0
    } else {
        1.0
    };
    sign * (adjacent / hypotenuse).clamp(-1.0, 1.0).acos()
}

impl Transform for Arc {
    fn transform<T>(self, t: &T) -> Arc
    where
        T: Transformation,
    {
        Arc::new(
            self.from.transform(t),
            self.to.transform(t),
            self.radius,
            self.x_axis_rotation,
            self.large_arc,
            self.sweep,
        )
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}

#[derive(Clone, Copy)]
pub struct Linearize {
    segment: Arc,
    epsilon: f64,
}

impl InternalIterator for Linearize {
    type Item = Point;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(Point) -> bool,
    {
        if self.segment.is_approximately_linear(self.epsilon) {
            return f(self.segment.to);
        }
        let (segment_0, segment_1) = self.segment.split(0.5);
        if !segment_0.linearize(self.epsilon).for_each(f) {
            return false;
        }
        segment_1.linearize(self.epsilon).for_each(f)
    }
}
