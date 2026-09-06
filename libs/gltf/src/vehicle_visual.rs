/// Authored limits for the displayed wheel pose, independent of vehicle physics.
///
/// Steering gain changes only the visible wheel angle, never the physical
/// steering, contact forces or turning radius. Steering angles are radians;
/// compression and droop are non-negative distances in authored model metres.
/// Assets without this optional contract retain their original motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualWheelMotion {
    /// Multiplies the physical steering angle, in 0..=1. Zero fixes the axle.
    pub steer_gain: f64,
    /// Symmetric cap on the displayed steering angle, in 0..=1.2 radians.
    pub steer_max: f64,
    /// Maximum displayed upward suspension displacement, in 0..=5 metres.
    pub compression: f64,
    /// Maximum displayed downward suspension displacement, in 0..=5 metres.
    pub droop: f64,
}

impl VisualWheelMotion {
    pub fn is_valid(self) -> bool {
        self.steer_gain.is_finite()
            && (0.0..=1.0).contains(&self.steer_gain)
            && self.steer_max.is_finite()
            && (0.0..=1.2).contains(&self.steer_max)
            && [self.compression, self.droop]
                .iter()
                .all(|v| v.is_finite() && (0.0..=5.0).contains(v))
    }

    /// Map a physical wheel angle and model-space suspension displacement to
    /// their visible equivalents. Positive suspension means compression.
    /// Callers validate authored limits at import, and apply instance scale
    /// after this mapping so a scaled asset retains the same clearance.
    pub fn pose(self, steer: f64, suspension: f64) -> (f64, f64) {
        (
            (steer * self.steer_gain).clamp(-self.steer_max, self.steer_max),
            suspension.clamp(-self.droop, self.compression),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VISUAL: VisualWheelMotion = VisualWheelMotion {
        steer_gain: 0.55,
        steer_max: 0.32,
        compression: 0.08,
        droop: 0.10,
    };

    #[test]
    fn visual_wheel_limits_preserve_sign_and_bound_both_travel_directions() {
        assert!(VISUAL.is_valid());
        assert_eq!(VISUAL.pose(1.0, 0.5), (0.32, 0.08));
        assert_eq!(VISUAL.pose(-1.0, -0.5), (-0.32, -0.10));
        let (steer, travel) = VISUAL.pose(0.2, -0.03);
        assert!((steer - 0.11).abs() < 1e-12);
        assert_eq!(travel, -0.03);
        assert_eq!(VISUAL.pose(0.0, 0.0), (0.0, 0.0));
        let fixed = VisualWheelMotion { steer_gain: 0.0, compression: 0.0, droop: 0.0, ..VISUAL };
        assert_eq!(fixed.pose(1.0, -2.0), (0.0, 0.0));
    }

    #[test]
    fn visual_wheel_limits_remain_in_model_metres_under_instance_scale() {
        for instance_scale in [0.1, 1.0, 10.0] {
            let physical_travel = 0.4 * instance_scale;
            let (_, visible_model_travel) = VISUAL.pose(0.9, physical_travel / instance_scale);
            assert!((visible_model_travel * instance_scale - 0.08 * instance_scale).abs() < 1e-12);
        }
    }

    #[test]
    fn visual_wheel_limits_reject_unbounded_and_nonfinite_metadata() {
        for invalid in [
            VisualWheelMotion { steer_gain: -0.1, ..VISUAL },
            VisualWheelMotion { steer_gain: 1.1, ..VISUAL },
            VisualWheelMotion { steer_max: 1.21, ..VISUAL },
            VisualWheelMotion { steer_max: f64::NAN, ..VISUAL },
            VisualWheelMotion { compression: -0.1, ..VISUAL },
            VisualWheelMotion { compression: 5.1, ..VISUAL },
            VisualWheelMotion { droop: f64::INFINITY, ..VISUAL },
        ] {
            assert!(!invalid.is_valid());
        }
    }
}
