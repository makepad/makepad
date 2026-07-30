//! Dual-radar volume compositor: KNMI raw polar volumes → hi-res rain image.
//!
//! The 1 km RAD_NL25 composite throws away most of what the radars measure:
//! the low-elevation scans have 223.5 m range bins. This module decodes the
//! raw volume files of both KNMI radars (Herwijnen RAD_NL62, Den Helder
//! RAD_NL61), filters non-meteorological echoes and blends them into a
//! pseudo-CAPPI composite on the same polar-stereographic grid as RAD_NL25
//! but at `scale` subdivisions per km (scale 4 → 250 m pixels, 2800×3060).
//!
//! Output pixels use the RAD_NL25 value convention (dBZ = 0.5·PV − 32,
//! 0 = dry, 255 = outside coverage) so everything downstream — colormaps,
//! Marshall-Palmer sampling — works unchanged.
//!
//! Algorithm validated against the official composite (same timestamp,
//! r = 0.76 on wet pixels; the residual is the ~2.5 min scan-time offset and
//! KNMI's additional clutter processing):
//! - volume rows are north-aligned (row i = azimuth i..i+1°, NOT rotated by
//!   `scan_start_azim`),
//! - pseudo-CAPPI at 800 m: per pixel pick the elevation whose beam center
//!   is nearest 800 m, bilinear-sample in polar space,
//! - echo filter: RhoHV ≥ 0.85 (rain is dual-pol correlated; interference,
//!   clutter and clear-air are not) unless ≥ 45 dBZ, then a polar despeckle,
//! - vertical support: reject echo whose next-higher beam is completely dry
//!   (ships/turbines/RLAN live in one beam, rain paints several),
//! - blend radars weighted by beam-height quality and range taper.

use crate::knmi_hdf5::Hdf5File;
use crate::radar_raster::{Stereo, GRID_COLS, GRID_ROWS};

/// (scan group index, elevation °) — ascending beam height at any range;
/// all use 223.5 m range bins reaching 187 km.
const LADDER: [(usize, f64); 6] = [(15, 0.3), (6, 0.8), (14, 1.2), (5, 2.0), (13, 2.8), (4, 4.5)];
/// Long-range 0.3° scan: 399 m bins reaching 320 km, used beyond the ladder.
const LONG_RANGE: (usize, f64) = (16, 0.3);

const TARGET_H_KM: f64 = 0.8;
const RE_43_KM: f64 = 6371.0 * 4.0 / 3.0;
const DRY_DBZ: f32 = -35.0;
const MAX_RANGE_KM: f64 = 320.0;

pub struct VolumeScan {
    pub elevation_deg: f64,
    pub bin_km: f64,
    pub n_range: usize,
    /// 360 × n_range filtered reflectivity, DRY_DBZ = no echo.
    pub dbz: Vec<f32>,
}

impl VolumeScan {
    fn at(&self, azimuth: usize, range: usize) -> f32 {
        self.dbz[azimuth * self.n_range + range]
    }

    /// Bilinear sample in polar space (azimuth rows centered at i+0.5°,
    /// range bins centered at (j+0.5)·bin). None = beyond the last bin.
    fn sample(&self, bearing_deg: f64, range_km: f64) -> Option<f32> {
        let rf = range_km / self.bin_km - 0.5;
        let r0 = rf.max(0.0).floor() as usize;
        if r0 + 1 >= self.n_range {
            return None;
        }
        let fr = (rf - r0 as f64).clamp(0.0, 1.0) as f32;
        let af = (bearing_deg - 0.5).rem_euclid(360.0);
        let a0 = af.floor() as usize % 360;
        let fa = (af - af.floor()) as f32;
        let a1 = (a0 + 1) % 360;
        let top = self.at(a0, r0) * (1.0 - fr) + self.at(a0, r0 + 1) * fr;
        let bottom = self.at(a1, r0) * (1.0 - fr) + self.at(a1, r0 + 1) * fr;
        Some(top * (1.0 - fa) + bottom * fa)
    }

    fn beam_height_km(&self, range_km: f64) -> f64 {
        range_km * self.elevation_deg.to_radians().sin() + range_km * range_km / (2.0 * RE_43_KM)
    }
}

pub struct RadarVolume {
    pub lon: f64,
    pub lat: f64,
    pub name: String,
    /// LADDER scans (ascending elevation) followed by the long-range scan.
    pub scans: Vec<VolumeScan>,
}

/// Parse "GEO=0.00193793*PV+-31.5019" → (gain, offset).
fn parse_calibration(formula: &str) -> Result<(f32, f32), String> {
    let rest = formula
        .strip_prefix("GEO=")
        .ok_or_else(|| format!("calibration formula {formula:?}"))?;
    let (gain, offset) = rest
        .split_once("*PV+")
        .ok_or_else(|| format!("calibration formula {formula:?}"))?;
    Ok((
        gain.parse().map_err(|_| format!("gain in {formula:?}"))?,
        offset.parse().map_err(|_| format!("offset in {formula:?}"))?,
    ))
}

fn decode_scan(file: &Hdf5File, index: usize, expect_el: f64) -> Result<VolumeScan, String> {
    let group_name = format!("scan{index}");
    let group = file
        .find_path(&[&group_name])?
        .ok_or_else(|| format!("{group_name} missing"))?;
    let attr_f64 = |name: &str| -> Result<f64, String> {
        file.attr(group, name)?
            .and_then(|v| v.as_f64())
            .ok_or_else(|| format!("{group_name}/{name} missing"))
    };
    let elevation_deg = attr_f64("scan_elevation")?;
    if (elevation_deg - expect_el).abs() > 0.05 {
        return Err(format!(
            "{group_name}: elevation {elevation_deg} (expected {expect_el}) — scan strategy changed"
        ));
    }
    let bin_km = attr_f64("scan_range_bin")?;
    let n_azim = attr_f64("scan_number_azim")? as usize;
    if n_azim != 360 {
        return Err(format!("{group_name}: {n_azim} azimuths"));
    }
    let calibration = file
        .find_path(&[&group_name, "calibration"])?
        .ok_or_else(|| format!("{group_name}/calibration missing"))?;
    let z_formula = file
        .attr(calibration, "calibration_Z_formulas")?
        .and_then(|v| v.as_text().map(str::to_string))
        .ok_or_else(|| format!("{group_name}: Z calibration missing"))?;
    let (z_gain, z_offset) = parse_calibration(&z_formula)?;
    let rho_formula = file
        .attr(calibration, "calibration_RhoHV_formulas")?
        .and_then(|v| v.as_text().map(str::to_string))
        .ok_or_else(|| format!("{group_name}: RhoHV calibration missing"))?;
    let (rho_gain, rho_offset) = parse_calibration(&rho_formula)?;

    let read_u16 = |name: &str| -> Result<(Vec<u16>, usize), String> {
        let ds = file
            .find_path(&[&group_name, name])?
            .ok_or_else(|| format!("{group_name}/{name} missing"))?;
        let info = file.dataset_info(ds)?;
        if info.dims.0 as usize != 360 {
            return Err(format!("{group_name}/{name}: {} rows", info.dims.0));
        }
        Ok((file.read_dataset_u16(&info)?, info.dims.1 as usize))
    };
    let (z_pv, n_range) = read_u16("scan_Z_data")?;
    let (rho_pv, rho_n) = read_u16("scan_RhoHV_data")?;
    if rho_n != n_range {
        return Err(format!("{group_name}: Z/RhoHV range mismatch"));
    }

    // Reflectivity with the non-meteorological echo filter applied.
    let mut dbz = vec![DRY_DBZ; 360 * n_range];
    let mut keep = vec![false; 360 * n_range];
    for i in 0..dbz.len() {
        if z_pv[i] > 0 {
            let value = z_gain * z_pv[i] as f32 + z_offset;
            dbz[i] = value;
            if value > -25.0 {
                let rho = rho_gain * rho_pv[i] as f32 + rho_offset;
                keep[i] = rho >= 0.85 || value >= 45.0;
            }
        }
    }
    // Despeckle: a kept bin needs ≥3 kept neighbors in its 3×3 polar hood
    // (azimuth wraps, range clamps).
    let mut wet_filtered = 0usize;
    let mut out = dbz.clone();
    for azimuth in 0..360usize {
        for range in 0..n_range {
            let i = azimuth * n_range + range;
            if dbz[i] <= -25.0 {
                continue;
            }
            if !keep[i] {
                out[i] = DRY_DBZ;
                wet_filtered += 1;
                continue;
            }
            let mut neighbors = 0;
            for da in -1i64..=1 {
                let a = (azimuth as i64 + da).rem_euclid(360) as usize;
                for dr in -1i64..=1 {
                    if da == 0 && dr == 0 {
                        continue;
                    }
                    let r = range as i64 + dr;
                    if r < 0 || r >= n_range as i64 {
                        continue;
                    }
                    if keep[a * n_range + r as usize] {
                        neighbors += 1;
                    }
                }
            }
            if neighbors < 3 {
                out[i] = DRY_DBZ;
                wet_filtered += 1;
            }
        }
    }
    let _ = wet_filtered;
    Ok(VolumeScan {
        elevation_deg,
        bin_km,
        n_range,
        dbz: out,
    })
}

impl RadarVolume {
    pub fn decode(data: &[u8]) -> Result<RadarVolume, String> {
        let file = Hdf5File::open(data)?;
        let radar = file
            .find_path(&["radar1"])?
            .ok_or("radar1 group missing")?;
        let location = file
            .attr(radar, "radar_location")?
            .ok_or("radar_location missing")?;
        let (lon, lat) = match &location {
            crate::knmi_hdf5::AttrValue::Floats(v) if v.len() >= 2 => (v[0], v[1]),
            _ => return Err("radar_location not a lon/lat pair".into()),
        };
        let name = file
            .attr(radar, "radar_name")?
            .and_then(|v| v.as_text().map(str::to_string))
            .unwrap_or_default();
        let mut scans = Vec::with_capacity(LADDER.len() + 1);
        for (index, elevation) in LADDER.iter().chain(std::iter::once(&LONG_RANGE)) {
            scans.push(decode_scan(&file, *index, *elevation)?);
        }
        Ok(RadarVolume {
            lon,
            lat,
            name,
            scans,
        })
    }
}

/// Composite output on the RAD_NL25 grid at `scale` subdivisions per km.
pub struct CompositeFrame {
    pub scale: usize,
    pub cols: usize,
    pub rows: usize,
    /// PV values, RAD_NL25 convention: dBZ = 0.5·PV − 32, 0 dry, 255 no coverage.
    pub values: Vec<u8>,
}

/// Great-circle range (km) + bearing (° from north) from the radar site.
fn range_bearing(lon0: f64, lat0: f64, lon: f64, lat: f64) -> (f64, f64) {
    let p0 = lat0.to_radians();
    let p1 = lat.to_radians();
    let dl = (lon - lon0).to_radians();
    let a = ((p1 - p0) / 2.0).sin().powi(2) + p0.cos() * p1.cos() * (dl / 2.0).sin().powi(2);
    let range = 2.0 * 6371.0 * a.sqrt().asin();
    let bearing = (dl.sin() * p1.cos())
        .atan2(p0.cos() * p1.sin() - p0.sin() * p1.cos() * dl.cos())
        .to_degrees()
        .rem_euclid(360.0);
    (range, bearing)
}

/// Pseudo-CAPPI sample of one radar at one ground position.
/// Returns (dbz, weight); weight 0 = no usable coverage there.
fn sample_radar(volume: &RadarVolume, range_km: f64, bearing_deg: f64) -> (f32, f64) {
    if range_km > MAX_RANGE_KM {
        return (DRY_DBZ, 0.0);
    }
    let ladder = &volume.scans[..volume.scans.len() - 1];
    let long_range = &volume.scans[volume.scans.len() - 1];
    let ladder_reach = ladder[0].bin_km * ladder[0].n_range as f64;
    let (scan, support_scan) = if range_km >= ladder_reach {
        (long_range, None)
    } else {
        let mut best = 0;
        let mut best_diff = f64::MAX;
        for (index, scan) in ladder.iter().enumerate() {
            let diff = (scan.beam_height_km(range_km) - TARGET_H_KM).abs();
            if diff < best_diff {
                best_diff = diff;
                best = index;
            }
        }
        (&ladder[best], ladder.get(best + 1))
    };
    let Some(mut dbz) = scan.sample(bearing_deg, range_km) else {
        return (DRY_DBZ, 0.0);
    };
    // Vertical support: echo with a completely dry beam directly above is a
    // ship / turbine / interference spike, not rain.
    if dbz > -25.0 {
        if let Some(up) = support_scan {
            let azimuth = (bearing_deg.floor() as usize).min(359);
            let range = (range_km / up.bin_km) as usize;
            if range < up.n_range && up.at(azimuth, range) < -28.0 {
                dbz = DRY_DBZ;
            }
        }
    }
    let height_diff = (scan.beam_height_km(range_km) - TARGET_H_KM).abs();
    let weight = 1.0 / (1.0 + height_diff * height_diff) / (1.0 + (range_km / 180.0).powi(4));
    (dbz, weight)
}

/// Blend the radars into one PV image. `scale` 1 → 700×765 (1 km, the
/// RAD_NL25 layout), 4 → 2800×3060 (250 m).
pub fn composite_volumes(volumes: &[RadarVolume], scale: usize) -> CompositeFrame {
    let cols = GRID_COLS * scale;
    let rows = GRID_ROWS * scale;
    let mut values = vec![255u8; cols * rows];
    let stereo = Stereo::new();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);
    let band = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        for (band_index, chunk) in values.chunks_mut(band * cols).enumerate() {
            let stereo = &stereo;
            scope.spawn(move || {
                let row0 = band_index * band;
                for (local_row, out_row) in chunk.chunks_mut(cols).enumerate() {
                    let row_km = (row0 + local_row) as f64 + 0.5;
                    for (col, out) in out_row.iter_mut().enumerate() {
                        let col_km = (col as f64 + 0.5) / scale as f64;
                        let (lon, lat) = stereo.inverse(col_km, row_km / scale as f64);
                        let mut z_sum = 0.0f64;
                        let mut w_sum = 0.0f64;
                        for volume in volumes {
                            let (range, bearing) = range_bearing(volume.lon, volume.lat, lon, lat);
                            let (dbz, weight) = sample_radar(volume, range, bearing);
                            if weight > 0.0 {
                                let z = if dbz > -34.0 {
                                    10.0f64.powf(dbz as f64 / 10.0)
                                } else {
                                    0.0
                                };
                                z_sum += weight * z;
                                w_sum += weight;
                            }
                        }
                        if w_sum <= 1e-6 {
                            continue; // stays 255: outside coverage
                        }
                        let z = z_sum / w_sum;
                        let dbz = 10.0 * z.max(1e-10).log10();
                        *out = if dbz < -31.0 {
                            0
                        } else {
                            ((dbz + 32.0) * 2.0).round().clamp(1.0, 254.0) as u8
                        };
                    }
                }
            });
        }
    });
    CompositeFrame {
        scale,
        cols,
        rows,
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(path: &str) -> Option<RadarVolume> {
        let data = std::fs::read(path).ok()?;
        Some(RadarVolume::decode(&data).unwrap())
    }

    #[test]
    fn composites_cached_volumes() {
        let Some(herwijnen) =
            load("../../local/overlays/radar_test/RAD_NL62_VOL_NA_202607301810.h5")
        else {
            return;
        };
        let Some(den_helder) =
            load("../../local/overlays/radar_test/RAD_NL61_VOL_NA_202607301810.h5")
        else {
            return;
        };
        assert_eq!(herwijnen.name, "Herwijnen");
        assert!((herwijnen.lon - 5.1381).abs() < 1e-3);
        assert!((den_helder.lat - 52.9528).abs() < 1e-3);
        assert_eq!(herwijnen.scans.len(), 7);
        assert_eq!(herwijnen.scans[1].n_range, 838);

        let frame = composite_volumes(&[herwijnen, den_helder], 1);
        assert_eq!((frame.cols, frame.rows), (700, 765));
        let coverage = frame.values.iter().filter(|&&v| v != 255).count();
        let wet = frame.values.iter().filter(|&&v| v > 0 && v != 255).count();
        let pv_sum: u64 = frame
            .values
            .iter()
            .filter(|&&v| v > 0 && v != 255)
            .map(|&v| v as u64)
            .sum();
        // Reference values from the validated python prototype over the same
        // files (coverage exact — pure geometry; echo counts tolerant to
        // f32 ordering differences).
        assert_eq!(coverage, 431050);
        assert!((wet as i64 - 58857).abs() < 1200, "wet={wet}");
        assert!(
            (pv_sum as i64 - 5_019_928).abs() < 110_000,
            "pv_sum={pv_sum}"
        );
    }
}
