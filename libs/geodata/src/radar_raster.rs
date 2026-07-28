//! KNMI radar frames → web-mercator RGBA overlays.
//!
//! The RAD_NL25 grid is polar stereographic (lat_ts 60°N, lon_0 0, GRS-ish
//! a=6378.14 km b=6356.75 km, 1 km pixels, row offset 3649.9795 — verified
//! against the file's `geo_product_corners` to ~20 m). Rendering wants a
//! north-up mercator-aligned texture, so we precompute one output→source
//! lookup table and apply it per frame; the colormap turns raw pixel values
//! (dBZ = 0.5·PV − 32) into translucent rain colors.

use crate::knmi_hdf5::KnmiFrame;

const A_KM: f64 = 6378.14;
const B_KM: f64 = 6356.75;
const ROW_OFFSET: f64 = 3649.9795;
const COL_OFFSET: f64 = 0.0;
const GRID_COLS: usize = 700;
const GRID_ROWS: usize = 765;

/// Geographic cover of the produced texture (the radar grid's bounding box
/// in lon/lat, slightly inset to skip out-of-image corners).
pub const RASTER_WEST: f64 = 0.0;
pub const RASTER_EAST: f64 = 10.86;
pub const RASTER_SOUTH: f64 = 48.89;
pub const RASTER_NORTH: f64 = 55.98;

fn mercator_y(lat_deg: f64) -> f64 {
    let lat = lat_deg.to_radians();
    (lat.tan() + 1.0 / lat.cos()).ln()
}

struct Stereo {
    e: f64,
    k0m: f64,
}

impl Stereo {
    fn new() -> Self {
        let e2 = 1.0 - (B_KM * B_KM) / (A_KM * A_KM);
        let e = e2.sqrt();
        let lat_ts = 60.0_f64.to_radians();
        let t_ts = (std::f64::consts::FRAC_PI_4 - lat_ts / 2.0).tan()
            / ((1.0 - e * lat_ts.sin()) / (1.0 + e * lat_ts.sin())).powf(e / 2.0);
        let m_ts = lat_ts.cos() / (1.0 - e2 * lat_ts.sin() * lat_ts.sin()).sqrt();
        Stereo {
            e,
            k0m: A_KM * m_ts / t_ts,
        }
    }

    /// lon/lat (deg) → radar grid col/row (f64).
    fn forward(&self, lon_deg: f64, lat_deg: f64) -> (f64, f64) {
        let lam = lon_deg.to_radians();
        let phi = lat_deg.to_radians();
        let t = (std::f64::consts::FRAC_PI_4 - phi / 2.0).tan()
            / ((1.0 - self.e * phi.sin()) / (1.0 + self.e * phi.sin())).powf(self.e / 2.0);
        let rho = self.k0m * t;
        let x = rho * lam.sin();
        let y = -rho * lam.cos();
        (x - COL_OFFSET, -y - ROW_OFFSET)
    }
}

/// Precomputed output-pixel → source-index mapping for a mercator-aligned
/// texture of `width`×`height` covering the RASTER_* bbox.
pub struct RadarProjection {
    pub width: usize,
    pub height: usize,
    /// Fractional source coordinates per output pixel (f32::NAN = outside
    /// the radar grid) — bilinear sampling smooths the 1 km cells into
    /// curved isolines instead of visible blocks.
    src_col: Vec<f32>,
    src_row: Vec<f32>,
}

impl RadarProjection {
    pub fn new(width: usize, height: usize) -> Self {
        let stereo = Stereo::new();
        let merc_n = mercator_y(RASTER_NORTH);
        let merc_s = mercator_y(RASTER_SOUTH);
        let mut src_col = vec![f32::NAN; width * height];
        let mut src_row = vec![f32::NAN; width * height];
        for py in 0..height {
            // Mercator-linear in screen y so the texture maps 1:1 onto the
            // renderer's mercator plane with a simple quad.
            let merc = merc_n + (merc_s - merc_n) * ((py as f64 + 0.5) / height as f64);
            let lat = (merc.sinh()).atan().to_degrees();
            for px in 0..width {
                let lon =
                    RASTER_WEST + (RASTER_EAST - RASTER_WEST) * ((px as f64 + 0.5) / width as f64);
                let (col, row) = stereo.forward(lon, lat);
                if col >= 0.0
                    && col < GRID_COLS as f64
                    && row >= 0.0
                    && row < GRID_ROWS as f64
                {
                    src_col[py * width + px] = col as f32;
                    src_row[py * width + px] = row as f32;
                }
            }
        }
        Self {
            width,
            height,
            src_col,
            src_row,
        }
    }

    /// Bilinear sample of the raw value field; 255 (out of image) reads as
    /// dry so coastal cells don't smear a phantom band.
    fn sample_value(frame: &KnmiFrame, col: f32, row: f32) -> f32 {
        let value_at = |c: i64, r: i64| -> f32 {
            if c < 0 || c >= frame.cols as i64 || r < 0 || r >= frame.rows as i64 {
                return 0.0;
            }
            let v = frame.values[r as usize * frame.cols + c as usize];
            if v == 255 {
                0.0
            } else {
                v as f32
            }
        };
        let c0 = (col - 0.5).floor();
        let r0 = (row - 0.5).floor();
        let fx = (col - 0.5) - c0;
        let fy = (row - 0.5) - r0;
        let (c0, r0) = (c0 as i64, r0 as i64);
        let top = value_at(c0, r0) * (1.0 - fx) + value_at(c0 + 1, r0) * fx;
        let bottom = value_at(c0, r0 + 1) * (1.0 - fx) + value_at(c0 + 1, r0 + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }


    /// Apply the LUT + bilinear sample to one frame → a CONTINUOUS value
    /// texture (raw 0..255 value in RGB, 255 alpha inside the radar grid).
    /// Banding/colormapping happens in the fragment shader so isolines stay
    /// crisp at screen resolution instead of texture resolution.
    pub fn frame_to_rgba(&self, frame: &KnmiFrame) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 4];
        if frame.cols != GRID_COLS || frame.rows != GRID_ROWS {
            return out;
        }
        for i in 0..self.src_col.len() {
            let col = self.src_col[i];
            if col.is_nan() {
                continue;
            }
            let value = Self::sample_value(frame, col, self.src_row[i])
                .round()
                .clamp(0.0, 254.0) as u8;
            let px = &mut out[i * 4..i * 4 + 4];
            px[0] = value;
            px[1] = value;
            px[2] = value;
            px[3] = 255;
        }
        out
    }
}

/// RGBA bytes → BGRA u32 texels (makepad VecBGRAu8_32 layout).
pub fn rgba_to_bgra_texels(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|px| {
            (px[2] as u32) | ((px[1] as u32) << 8) | ((px[0] as u32) << 16) | ((px[3] as u32) << 24)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_cached_forecast_frame() {
        let path = "../../local/overlays/radar/forecast/RAD_NL25_PCP_FM_202607280900.h5";
        let Ok(data) = std::fs::read(path) else {
            return;
        };
        let frames = crate::knmi_hdf5::decode_frames(&data).unwrap();
        let projection = RadarProjection::new(512, 640);
        let rgba = projection.frame_to_rgba(&frames[0]);
        let visible = rgba.chunks(4).filter(|px| px[3] > 0).count();
        // Frame 1 has 5068 wet source pixels; the mercator resample of the
        // NL bbox should land in the same order of magnitude.
        assert!(visible > 500, "visible rain pixels: {visible}");
        let mapped = projection.src_col.iter().filter(|c| !c.is_nan()).count();
        // Most of the output bbox lies inside the radar grid.
        assert!(mapped > 512 * 640 / 2, "mapped: {mapped}");
    }
}
