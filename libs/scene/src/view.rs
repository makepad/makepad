//! The view a world is drawn from: a physical camera and the viewport it
//! fills. Pure data plus the projection maths every consumer shares (the
//! renderer, anchors, depth of field, motion blur), so no two of them derive
//! a lens differently.
use makepad_math::*;

/// How the camera maps view space to clip space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// A physical lens: the vertical field of view follows from the focal
    /// length over the sensor height (a 50 mm lens on the 24 mm full-frame
    /// height is about 27 degrees).
    Perspective { focal_length_mm: f32, sensor_height_mm: f32 },
    /// Parallel projection; `height` is the visible world height.
    Ortho { height: f32 },
}

impl Default for Projection {
    fn default() -> Self {
        // 40 degrees vertical on a full-frame sensor: CameraState's stock fov.
        Projection::Perspective { focal_length_mm: Camera::focal_for_fov(40.0, 24.0), sensor_height_mm: 24.0 }
    }
}

/// A physical camera in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Camera to world (the camera looks down its local -Z, +Y up).
    pub transform: Mat4f,
    pub projection: Projection,
    pub near: f32,
    pub far: f32,
    /// Distance to the plane in focus, metres. Depth of field reads it.
    pub focus_distance: f32,
    /// Aperture as an f-number; 0 or infinite means no depth of field.
    pub f_stop: f32,
    /// Shutter angle in degrees (180 = half the frame interval open);
    /// 0 means no motion blur.
    pub shutter_deg: f32,
    /// Last frame's camera to world, for realtime velocity. Locked time
    /// leaves it `None`: its motion blur accumulates sub-frames instead.
    pub prev_transform: Option<Mat4f>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            transform: Mat4f::identity(),
            projection: Projection::default(),
            near: 0.15,
            far: 500.0,
            focus_distance: 10.0,
            f_stop: 0.0,
            shutter_deg: 180.0,
            prev_transform: None,
        }
    }
}

impl Camera {
    /// The focal length that gives `fov_deg` vertically on a sensor of this height.
    pub fn focal_for_fov(fov_deg: f32, sensor_height_mm: f32) -> f32 {
        sensor_height_mm * 0.5 / (fov_deg.to_radians() * 0.5).tan()
    }

    /// A camera at `eye` looking at `target`, with a vertical field of view in degrees.
    pub fn look_at(eye: Vec3f, target: Vec3f, fov_deg: f32) -> Self {
        let view = Mat4f::look_at(eye, target, vec3f(0.0, 1.0, 0.0));
        Self {
            transform: view.invert(),
            projection: Projection::Perspective { focal_length_mm: Self::focal_for_fov(fov_deg, 24.0), sensor_height_mm: 24.0 },
            focus_distance: (target - eye).length(),
            ..Self::default()
        }
    }

    /// Vertical field of view in degrees; 0 for an orthographic camera.
    pub fn fov_deg(&self) -> f32 {
        match self.projection {
            Projection::Perspective { focal_length_mm, sensor_height_mm } => {
                (2.0 * (sensor_height_mm * 0.5 / focal_length_mm.max(1.0e-3)).atan()).to_degrees()
            }
            Projection::Ortho { .. } => 0.0,
        }
    }

    pub fn eye(&self) -> Vec3f {
        vec3f(self.transform.v[12], self.transform.v[13], self.transform.v[14])
    }

    /// World to camera.
    pub fn view_matrix(&self) -> Mat4f {
        self.transform.invert()
    }

    /// Camera to clip space for a viewport of this aspect (width / height),
    /// in the -1..1 depth convention `Mat4f::perspective` uses.
    pub fn projection_matrix(&self, aspect: f32) -> Mat4f {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let near = self.near.max(1.0e-4);
        let far = self.far.max(near * 1.0001);
        match self.projection {
            Projection::Perspective { .. } => Mat4f::perspective(self.fov_deg(), aspect, near, far),
            Projection::Ortho { height } => {
                let h = height.abs().max(1.0e-4) * 0.5;
                let w = h * aspect;
                let nf = 1.0 / (near - far);
                Mat4f { v: [
                    1.0 / w, 0.0, 0.0, 0.0,
                    0.0, 1.0 / h, 0.0, 0.0,
                    0.0, 0.0, 2.0 * nf, 0.0,
                    0.0, 0.0, (far + near) * nf, 1.0,
                ] }
            }
        }
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4f {
        Mat4f::mul(&self.projection_matrix(aspect), &self.view_matrix())
    }

    /// Last frame's view-projection, when the camera carries its previous
    /// transform (realtime velocity).
    pub fn prev_view_projection(&self, aspect: f32) -> Option<Mat4f> {
        let prev = self.prev_transform?;
        Some(Mat4f::mul(&self.projection_matrix(aspect), &prev.invert()))
    }

    /// Circle-of-confusion diameter on the sensor (mm) for a point `depth`
    /// metres in front of the lens: 0 when depth of field is off.
    pub fn circle_of_confusion_mm(&self, depth: f32) -> f32 {
        let Projection::Perspective { focal_length_mm: f, .. } = self.projection else { return 0.0 };
        if !(self.f_stop.is_finite() && self.f_stop > 0.0) || depth <= 0.0 {
            return 0.0;
        }
        let s = (self.focus_distance.max(self.near) * 1000.0).max(f * 1.001);
        let d = depth * 1000.0;
        let aperture = f / self.f_stop;
        (aperture * (d - s).abs() / d * f / (s - f)).abs()
    }

    /// The fraction of the frame interval the shutter is open (0..1).
    pub fn shutter_fraction(&self) -> f32 {
        if self.shutter_deg.is_finite() { (self.shutter_deg / 360.0).clamp(0.0, 1.0) } else { 0.0 }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.transform.v.iter().any(|v| !v.is_finite()) {
            return Err("camera transform must be finite");
        }
        if let Some(prev) = &self.prev_transform {
            if prev.v.iter().any(|v| !v.is_finite()) {
                return Err("camera prev_transform must be finite");
            }
        }
        if !(self.near.is_finite() && self.far.is_finite() && self.near > 0.0 && self.far > self.near) {
            return Err("camera needs 0 < near < far");
        }
        match self.projection {
            Projection::Perspective { focal_length_mm, sensor_height_mm } => {
                if !(focal_length_mm.is_finite() && focal_length_mm > 0.0 && sensor_height_mm.is_finite() && sensor_height_mm > 0.0) {
                    return Err("lens focal length and sensor height must be positive");
                }
            }
            Projection::Ortho { height } => {
                if !(height.is_finite() && height > 0.0) {
                    return Err("orthographic height must be positive");
                }
            }
        }
        if !self.focus_distance.is_finite() || self.focus_distance < 0.0 || self.f_stop.is_nan() || self.f_stop < 0.0 {
            return Err("focus distance and f-stop must be non-negative");
        }
        if !self.shutter_deg.is_finite() || !(0.0..=360.0).contains(&self.shutter_deg) {
            return Err("shutter angle must be in 0..360 degrees");
        }
        Ok(())
    }
}

/// What a world is drawn from. `None` camera = the host's own camera
/// (Sandbox's `CameraState` path).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub camera: Option<Camera>,
    /// Viewport size in pixels, for anchors and pixel-sized effects; 0 = the
    /// target's own size.
    pub size: Vec2f,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_50mm_lens_on_full_frame_is_27_degrees_and_round_trips() {
        let c = Camera { projection: Projection::Perspective { focal_length_mm: 50.0, sensor_height_mm: 24.0 }, ..Camera::default() };
        assert!((c.fov_deg() - 26.99).abs() < 0.05, "{}", c.fov_deg());
        let f = Camera::focal_for_fov(c.fov_deg(), 24.0);
        assert!((f - 50.0).abs() < 1.0e-3);
        assert!((Camera::default().fov_deg() - 40.0).abs() < 1.0e-3);
    }

    #[test]
    fn look_at_projects_the_target_to_the_centre() {
        let c = Camera::look_at(vec3f(0.0, 2.0, 10.0), vec3f(0.0, 1.0, 0.0), 40.0);
        let p = c.view_projection(16.0 / 9.0).transform_vec4(vec4(0.0, 1.0, 0.0, 1.0));
        assert!((p.x / p.w).abs() < 1.0e-4 && (p.y / p.w).abs() < 1.0e-4);
        assert!(p.w > 0.0);
        assert!((c.eye() - vec3f(0.0, 2.0, 10.0)).length() < 1.0e-4);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn ortho_maps_half_height_to_the_top_edge() {
        let mut c = Camera { projection: Projection::Ortho { height: 4.0 }, ..Camera::default() };
        c.transform.v[14] = 5.0;
        let p = c.view_projection(1.0).transform_vec4(vec4(0.0, 2.0, 0.0, 1.0));
        assert!((p.y / p.w - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn prev_view_projection_needs_a_previous_transform() {
        let mut c = Camera::look_at(vec3f(0.0, 0.0, 5.0), vec3f(0.0, 0.0, 0.0), 40.0);
        assert!(c.prev_view_projection(1.0).is_none());
        c.prev_transform = Some(c.transform);
        assert_eq!(c.prev_view_projection(1.0).unwrap().v, c.view_projection(1.0).v);
    }

    #[test]
    fn circle_of_confusion_is_zero_in_focus_and_without_an_aperture() {
        let mut c = Camera { projection: Projection::Perspective { focal_length_mm: 50.0, sensor_height_mm: 24.0 }, focus_distance: 5.0, f_stop: 2.0, ..Camera::default() };
        assert!(c.circle_of_confusion_mm(5.0) < 1.0e-5);
        assert!(c.circle_of_confusion_mm(1.0) > c.circle_of_confusion_mm(3.0));
        assert!(c.circle_of_confusion_mm(50.0) > 0.0);
        c.f_stop = 0.0;
        assert_eq!(c.circle_of_confusion_mm(1.0), 0.0);
    }

    #[test]
    fn validation_refuses_broken_lenses() {
        assert!(Camera { near: 0.0, ..Camera::default() }.validate().is_err());
        assert!(Camera { shutter_deg: 400.0, ..Camera::default() }.validate().is_err());
        assert!(Camera { projection: Projection::Ortho { height: f32::NAN }, ..Camera::default() }.validate().is_err());
        let mut t = Mat4f::identity();
        t.v[3] = f32::INFINITY;
        assert!(Camera { prev_transform: Some(t), ..Camera::default() }.validate().is_err());
    }
}
