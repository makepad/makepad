use crate::math::Real;

/// Ensures the given coordinate doesn’t go out of the bounds of spade’s acceptable values.
///
/// Returns 0.0 if the coordinate is smaller than `spade::MIN_ALLOWED_VALUE`.
/// Returns `spade::MAX_ALLOWED_VALUE` the coordinate is larger than `spade::MAX_ALLOWED_VALUE`.
pub fn sanitize_spade_coord(coord: Real) -> Real {
    let abs = coord.abs();

    #[allow(clippy::unnecessary_cast)]
    if abs as f64 <= spade::MIN_ALLOWED_VALUE {
        return 0.0;
    }

    #[cfg(feature = "f64")]
    if abs > spade::MAX_ALLOWED_VALUE {
        // This cannot happen in f32 since the max is 3.40282347E+38.
        return spade::MAX_ALLOWED_VALUE * coord.signum();
    }

    coord
}

/// Ensures the coordinates of the given point don't go out of the bounds of spade's acceptable values.
pub fn sanitize_spade_point(point: spade::Point2<Real>) -> spade::Point2<Real> {
    spade::Point2::new(sanitize_spade_coord(point.x), sanitize_spade_coord(point.y))
}
