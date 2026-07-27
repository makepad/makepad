//! Shared coordinate conventions (see gps.md): `f64` lon/lat at public API
//! boundaries, normalized web mercator (0..1) internally, and u32 fixed
//! point (norm × 2³²) in the file formats.

pub const EQUATOR_CIRCUMFERENCE_M: f64 = 40_075_016.686;
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_78;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct LonLat {
    pub lon: f64,
    pub lat: f64,
}

impl LonLat {
    pub fn new(lon: f64, lat: f64) -> Self {
        Self { lon, lat }
    }
}

/// Lon/lat to normalized web mercator (x, y) in 0..1, y down.
pub fn lon_lat_to_norm(p: LonLat) -> (f64, f64) {
    let x = (p.lon + 180.0) / 360.0;
    let lat = p.lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
    let sin_lat = lat.to_radians().sin();
    let y = 0.5 - ((1.0 + sin_lat) / (1.0 - sin_lat)).ln() / (4.0 * std::f64::consts::PI);
    (x, y)
}

pub fn norm_to_lon_lat(x: f64, y: f64) -> LonLat {
    let lon = x * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    LonLat::new(lon, lat)
}

/// Normalized 0..1 to u32 fixed point (norm × 2³²).
pub fn norm_to_fixed(v: f64) -> u32 {
    let scaled = (v.clamp(0.0, 1.0) * 4_294_967_296.0) as u64;
    scaled.min(u32::MAX as u64) as u32
}

pub fn fixed_to_norm(v: u32) -> f64 {
    v as f64 / 4_294_967_296.0
}

pub fn lon_lat_to_fixed(p: LonLat) -> (u32, u32) {
    let (x, y) = lon_lat_to_norm(p);
    (norm_to_fixed(x), norm_to_fixed(y))
}

pub fn fixed_to_lon_lat(x: u32, y: u32) -> LonLat {
    norm_to_lon_lat(fixed_to_norm(x), fixed_to_norm(y))
}

/// Great-circle distance in meters.
pub fn haversine_m(a: LonLat, b: LonLat) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_008.8;
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat * 0.5).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon * 0.5).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

/// Initial bearing from `a` to `b` in degrees, 0 = north, clockwise, 0..360.
pub fn bearing_deg(a: LonLat, b: LonLat) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlon = (b.lon - a.lon).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Signed smallest angle from bearing `from` to bearing `to`, in -180..180.
/// Positive = clockwise (right turn).
pub fn bearing_delta_deg(from: f64, to: f64) -> f64 {
    let mut d = (to - from) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

/// Ground meters covered by one normalized-mercator unit at this latitude
/// (locally; mercator is conformal so it holds for both axes).
pub fn meters_per_norm_unit(lat: f64) -> f64 {
    EQUATOR_CIRCUMFERENCE_M * lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians().cos()
}

/// Project point `p` onto segment `a`-`b` (all in one planar space).
/// Returns the projected point and the clamped parameter t in 0..1.
pub fn project_on_segment(
    p: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
) -> ((f64, f64), f64) {
    let ab = (b.0 - a.0, b.1 - a.1);
    let len_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if len_sq <= 0.0 {
        return (a, 0.0);
    }
    let ap = (p.0 - a.0, p.1 - a.1);
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / len_sq).clamp(0.0, 1.0);
    ((a.0 + ab.0 * t, a.1 + ab.1 * t), t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mercator_roundtrip() {
        let p = LonLat::new(4.9041, 52.3676);
        let (x, y) = lon_lat_to_norm(p);
        let back = norm_to_lon_lat(x, y);
        assert!((back.lon - p.lon).abs() < 1e-9);
        assert!((back.lat - p.lat).abs() < 1e-9);
    }

    #[test]
    fn fixed_roundtrip() {
        let p = LonLat::new(4.9041, 52.3676);
        let (fx, fy) = lon_lat_to_fixed(p);
        let back = fixed_to_lon_lat(fx, fy);
        // 2^-32 of the world is ~9mm; expect well under 1e-6 degrees error.
        assert!((back.lon - p.lon).abs() < 1e-6);
        assert!((back.lat - p.lat).abs() < 1e-6);
    }

    #[test]
    fn haversine_known_distance() {
        // Amsterdam Centraal to Dam square is roughly 700m.
        let centraal = LonLat::new(4.9003, 52.3791);
        let dam = LonLat::new(4.8932, 52.3731);
        let d = haversine_m(centraal, dam);
        assert!(d > 600.0 && d < 900.0, "distance {}", d);
    }

    #[test]
    fn bearing_cardinals() {
        let origin = LonLat::new(4.9, 52.37);
        let north = LonLat::new(4.9, 52.38);
        let east = LonLat::new(4.92, 52.37);
        assert!(bearing_deg(origin, north).abs() < 1.0);
        assert!((bearing_deg(origin, east) - 90.0).abs() < 1.5);
        assert!((bearing_delta_deg(350.0, 10.0) - 20.0).abs() < 1e-9);
        assert!((bearing_delta_deg(10.0, 350.0) + 20.0).abs() < 1e-9);
    }

    #[test]
    fn segment_projection() {
        let (p, t) = project_on_segment((0.5, 1.0), (0.0, 0.0), (1.0, 0.0));
        assert_eq!(p, (0.5, 0.0));
        assert_eq!(t, 0.5);
        let (p, t) = project_on_segment((-1.0, 1.0), (0.0, 0.0), (1.0, 0.0));
        assert_eq!(p, (0.0, 0.0));
        assert_eq!(t, 0.0);
    }
}
