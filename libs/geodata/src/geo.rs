//! Coordinate transforms shared by all layer builders.
//!
//! Conventions: `f64` lon/lat (WGS84, degrees) at module boundaries,
//! normalized web-mercator `(0..1, 0..1)` internally (y grows south, matching
//! tile addressing), matching the map renderer's `center_norm`.

/// RD New (EPSG:28992) -> WGS84, using the published approximation polynomials
/// (Schreutelkamp & Strang van Hees). Accuracy is decimeter-scale across NL,
/// far below overlay pixel size at any zoom we tile.
pub fn rd_to_wgs84(x: f64, y: f64) -> (f64, f64) {
    const X0: f64 = 155_000.0;
    const Y0: f64 = 463_000.0;
    const PHI0: f64 = 52.155_174_40;
    const LAM0: f64 = 5.387_206_21;

    let dx = (x - X0) * 1e-5;
    let dy = (y - Y0) * 1e-5;

    // (p, q, coefficient) terms for phi (seconds of arc)
    const KPQ: &[(i32, i32, f64)] = &[
        (0, 1, 3235.65389),
        (2, 0, -32.58297),
        (0, 2, -0.24750),
        (2, 1, -0.84978),
        (0, 3, -0.06550),
        (2, 2, -0.01709),
        (1, 0, -0.00738),
        (4, 0, 0.00530),
        (2, 3, -0.00039),
        (4, 1, 0.00033),
        (1, 1, -0.00012),
    ];
    // (p, q, coefficient) terms for lambda (seconds of arc)
    const LPQ: &[(i32, i32, f64)] = &[
        (1, 0, 5260.52916),
        (1, 1, 105.94684),
        (1, 2, 2.45656),
        (3, 0, -0.81885),
        (1, 3, 0.05594),
        (3, 1, -0.05607),
        (0, 1, 0.01199),
        (3, 2, -0.00256),
        (1, 4, 0.00128),
        (0, 2, 0.00022),
        (4, 0, -0.00022),
        (5, 0, 0.00026),
    ];

    let mut dphi = 0.0;
    for &(p, q, k) in KPQ {
        dphi += k * dx.powi(p) * dy.powi(q);
    }
    let mut dlam = 0.0;
    for &(p, q, l) in LPQ {
        dlam += l * dx.powi(p) * dy.powi(q);
    }
    let lat = PHI0 + dphi / 3600.0;
    let lon = LAM0 + dlam / 3600.0;
    (lon, lat)
}

/// WGS84 -> RD New (EPSG:28992), the published inverse approximation
/// polynomials. Decimeter-scale accuracy — used to sample RD-referenced
/// rasters (RIVM noise) at map coordinates.
pub fn wgs84_to_rd(lon: f64, lat: f64) -> (f64, f64) {
    const X0: f64 = 155_000.0;
    const Y0: f64 = 463_000.0;
    const PHI0: f64 = 52.155_174_40;
    const LAM0: f64 = 5.387_206_21;

    let dphi = 0.36 * (lat - PHI0);
    let dlam = 0.36 * (lon - LAM0);

    const RPQ: &[(i32, i32, f64)] = &[
        (0, 1, 190_094.945),
        (1, 1, -11_832.228),
        (2, 1, -114.221),
        (0, 3, -32.391),
        (1, 0, -0.705),
        (3, 1, -2.340),
        (1, 3, -0.608),
        (0, 2, -0.008),
        (2, 3, 0.148),
    ];
    const SPQ: &[(i32, i32, f64)] = &[
        (1, 0, 309_056.544),
        (0, 2, 3_638.893),
        (2, 0, 73.077),
        (1, 2, -157.984),
        (3, 0, 59.788),
        (0, 1, 0.433),
        (2, 2, -6.439),
        (1, 1, -0.032),
        (0, 4, 0.092),
        (1, 4, -0.054),
    ];

    let mut x = X0;
    for &(p, q, r) in RPQ {
        x += r * dphi.powi(p) * dlam.powi(q);
    }
    let mut y = Y0;
    for &(p, q, s) in SPQ {
        y += s * dphi.powi(p) * dlam.powi(q);
    }
    (x, y)
}

/// WGS84 lon/lat -> normalized web mercator (0..1, 0..1), y growing south.
/// Thin wrapper over the shared projection in `makepad-map-nav` so the math
/// lives in exactly one place across the map stack.
pub fn wgs84_to_norm(lon: f64, lat: f64) -> (f64, f64) {
    makepad_map_nav::geo::lon_lat_to_norm(makepad_map_nav::geo::LonLat::new(lon, lat))
}

/// Axis-aligned bbox in normalized mercator.
#[derive(Debug, Clone, Copy)]
pub struct NormBBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl NormBBox {
    pub fn empty() -> Self {
        NormBBox {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }
    pub fn add(&mut self, x: f64, y: f64) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }
    pub fn is_empty(&self) -> bool {
        self.min_x > self.max_x
    }
}

/// Deterministic tile ordering key matching the mbtiles writer's rowid scheme
/// (zoom ascending, then 256x256 block row-major, then local row-major).
/// Sorting tile keys by this value yields the exact order `MbtilesWriter`
/// requires.
pub fn tile_order_key(zoom: u8, x: u32, y: u32) -> u128 {
    let zoom_capacity = 1_u128 << (u32::from(zoom) * 2);
    let prefix = (zoom_capacity - 1) / 3;
    let axis = 1_u128 << zoom;
    let within = if zoom <= 8 {
        u128::from(y) * axis + u128::from(x)
    } else {
        let blocks_per_axis = 1_u128 << (zoom - 8);
        let block_x = u128::from(x >> 8);
        let block_y = u128::from(y >> 8);
        let local_x = u128::from(x & 255);
        let local_y = u128::from(y & 255);
        ((block_y * blocks_per_axis + block_x) << 16) + (local_y << 8) + local_x
    };
    prefix + within + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rd_origin_is_amersfoort() {
        let (lon, lat) = rd_to_wgs84(155_000.0, 463_000.0);
        assert!((lat - 52.155_174_40).abs() < 1e-9);
        assert!((lon - 5.387_206_21).abs() < 1e-9);
    }

    #[test]
    fn rd_scale_is_sane() {
        // 1 km east of the origin is ~0.0146 degrees of longitude at 52N.
        let (lon, _lat) = rd_to_wgs84(156_000.0, 463_000.0);
        let dlon = lon - 5.387_206_21;
        assert!((dlon - 0.01464).abs() < 0.0005, "dlon = {dlon}");
        // 1 km north is ~0.0090 degrees of latitude.
        let (_lon, lat) = rd_to_wgs84(155_000.0, 464_000.0);
        let dlat = lat - 52.155_174_40;
        assert!((dlat - 0.00899).abs() < 0.0005, "dlat = {dlat}");
    }

    #[test]
    fn rd_round_trip_within_a_meter() {
        for &(x, y) in &[
            (121_861.0, 487_981.0), // Amsterdam
            (92_565.0, 437_428.0),  // Rotterdam
            (176_500.0, 317_500.0), // Maastricht area
            (233_883.0, 582_065.0), // Groningen area
        ] {
            let (lon, lat) = rd_to_wgs84(x, y);
            let (x2, y2) = wgs84_to_rd(lon, lat);
            assert!(
                (x - x2).abs() < 1.0 && (y - y2).abs() < 1.0,
                "round trip drift: ({x},{y}) -> ({x2:.2},{y2:.2})"
            );
        }
    }

    #[test]
    fn amsterdam_lands_in_amsterdam() {
        // Amsterdam Centraal is around RD (121861, 487981).
        let (lon, lat) = rd_to_wgs84(121_861.0, 487_981.0);
        assert!((52.36..52.40).contains(&lat), "lat = {lat}");
        assert!((4.88..4.92).contains(&lon), "lon = {lon}");
    }

    #[test]
    fn tile_order_matches_block_major() {
        let mut last = 0u128;
        for z in [6u8, 9] {
            for y in 0..40u32 {
                for x in 0..40u32 {
                    let k = tile_order_key(z, x, y);
                    assert!(k > last || (x == 0 && y == 0), "order broken at z{z} {x},{y}");
                    if x > 0 || y > 0 {
                        last = k;
                    } else {
                        last = k;
                    }
                }
            }
        }
    }
}
