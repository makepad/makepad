//! SRTM elevation sampling for route height profiles.
//!
//! Tiles come from the AWS Open Data terrain mirror (skadi layout): one
//! 1°×1° `.hgt.gz` per integer lat/lon, 3601×3601 big-endian i16 meters,
//! row 0 at the NORTH edge. Fetched once with curl, cached decompressed
//! under `local/maps/dem/` (~26MB per tile), fully offline afterwards.

use makepad_map_nav::geo::{haversine_m, LonLat};
use makepad_fast_inflate::gzip_decompress_vec;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const HGT_DIM: usize = 3601;
const HGT_VOID: i16 = -32768;

pub struct DemTile {
    data: Vec<i16>,
}

pub struct DemCache {
    dir: PathBuf,
    tiles: HashMap<(i32, i32), Option<Arc<DemTile>>>,
}

fn tile_name(lat_i: i32, lon_i: i32) -> String {
    format!(
        "{}{:02}{}{:03}",
        if lat_i < 0 { "S" } else { "N" },
        lat_i.abs(),
        if lon_i < 0 { "W" } else { "E" },
        lon_i.abs()
    )
}

impl DemCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            tiles: HashMap::new(),
        }
    }

    fn tile(&mut self, lat_i: i32, lon_i: i32) -> Option<Arc<DemTile>> {
        if let Some(cached) = self.tiles.get(&(lat_i, lon_i)) {
            return cached.clone();
        }
        let loaded = self.load_tile(lat_i, lon_i);
        if loaded.is_none() {
            eprintln!("dem: no elevation tile for {}", tile_name(lat_i, lon_i));
        }
        self.tiles.insert((lat_i, lon_i), loaded.clone());
        loaded
    }

    fn load_tile(&self, lat_i: i32, lon_i: i32) -> Option<Arc<DemTile>> {
        let name = tile_name(lat_i, lon_i);
        let hgt_path = self.dir.join(format!("{}.hgt", name));
        if !hgt_path.exists() {
            std::fs::create_dir_all(&self.dir).ok()?;
            let url = format!(
                "https://s3.amazonaws.com/elevation-tiles-prod/skadi/{}{:02}/{}.hgt.gz",
                if lat_i < 0 { "S" } else { "N" },
                lat_i.abs(),
                name
            );
            let gz_path = self.dir.join(format!("{}.hgt.gz.part", name));
            let status = std::process::Command::new("curl")
                .args(["-sf", "--connect-timeout", "10", "--max-time", "120"])
                .arg(&url)
                .arg("-o")
                .arg(&gz_path)
                .status()
                .ok()?;
            if !status.success() {
                let _ = std::fs::remove_file(&gz_path);
                return None;
            }
            let gz = std::fs::read(&gz_path).ok()?;
            let _ = std::fs::remove_file(&gz_path);
            let raw = gzip_decompress_vec(&gz).ok()?;
            if raw.len() != HGT_DIM * HGT_DIM * 2 {
                return None;
            }
            std::fs::write(&hgt_path, &raw).ok()?;
        }
        let raw = std::fs::read(&hgt_path).ok()?;
        if raw.len() != HGT_DIM * HGT_DIM * 2 {
            return None;
        }
        let data: Vec<i16> = raw
            .chunks_exact(2)
            .map(|c| i16::from_be_bytes([c[0], c[1]]))
            .collect();
        Some(Arc::new(DemTile { data }))
    }

    /// Bilinear elevation sample in meters; None outside data / voids.
    pub fn sample_m(&mut self, pos: LonLat) -> Option<f32> {
        let lat_i = pos.lat.floor() as i32;
        let lon_i = pos.lon.floor() as i32;
        let tile = self.tile(lat_i, lon_i)?;
        // Fractional position inside the tile; row 0 = north edge (lat+1).
        let fx = (pos.lon - lon_i as f64) * (HGT_DIM - 1) as f64;
        let fy = (1.0 - (pos.lat - lat_i as f64)) * (HGT_DIM - 1) as f64;
        let x0 = (fx as usize).min(HGT_DIM - 2);
        let y0 = (fy as usize).min(HGT_DIM - 2);
        let tx = (fx - x0 as f64) as f32;
        let ty = (fy - y0 as f64) as f32;
        let at = |x: usize, y: usize| -> Option<f32> {
            let v = tile.data[y * HGT_DIM + x];
            (v != HGT_VOID).then_some(v as f32)
        };
        let (v00, v10, v01, v11) = (
            at(x0, y0)?,
            at(x0 + 1, y0)?,
            at(x0, y0 + 1)?,
            at(x0 + 1, y0 + 1)?,
        );
        let top = v00 + (v10 - v00) * tx;
        let bot = v01 + (v11 - v01) * tx;
        Some(top + (bot - top) * ty)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ElevationProfile {
    /// Evenly spaced samples along the route: (distance m, elevation m).
    pub samples: Vec<(f32, f32)>,
    pub total_m: f32,
    pub min_elev: f32,
    pub max_elev: f32,
    pub ascent_m: f32,
    pub descent_m: f32,
}

/// Sample the route polyline at ~`count` even spacings and derive
/// ascent/descent from a lightly smoothed profile (raw SRTM is noisy at
/// 30m posting — unsmoothed sums badly overstate climb).
pub fn route_profile(
    cache: &mut DemCache,
    points: &[LonLat],
    count: usize,
) -> Option<ElevationProfile> {
    if points.len() < 2 {
        return None;
    }
    let mut cum = Vec::with_capacity(points.len());
    let mut total = 0.0f64;
    cum.push(0.0);
    for pair in points.windows(2) {
        total += haversine_m(pair[0], pair[1]);
        cum.push(total);
    }
    if total <= 0.0 {
        return None;
    }
    let count = count.clamp(16, 512);
    let mut samples = Vec::with_capacity(count);
    let mut seg = 0usize;
    for i in 0..count {
        let d = total * i as f64 / (count - 1) as f64;
        while seg + 2 < cum.len() && cum[seg + 1] < d {
            seg += 1;
        }
        let seg_len = (cum[seg + 1] - cum[seg]).max(1e-9);
        let t = ((d - cum[seg]) / seg_len).clamp(0.0, 1.0);
        let pos = LonLat::new(
            points[seg].lon + (points[seg + 1].lon - points[seg].lon) * t,
            points[seg].lat + (points[seg + 1].lat - points[seg].lat) * t,
        );
        let elev = cache.sample_m(pos)?;
        samples.push((d as f32, elev));
    }
    // SRTM is a radar *surface* model: in cities the samples ride over
    // rooftops and trees (15m spikes over Amsterdam canal houses). Median
    // the spikes out, smooth over ~150m of route, and only count climb
    // through a hysteresis threshold — otherwise a pancake-flat city sums
    // to tens of meters of phantom ascent.
    let median5: Vec<f32> = (0..samples.len())
        .map(|i| {
            let lo = i.saturating_sub(2);
            let hi = (i + 2).min(samples.len() - 1);
            let mut window: Vec<f32> = samples[lo..=hi].iter().map(|s| s.1).collect();
            window.sort_by(|a, b| a.total_cmp(b));
            window[window.len() / 2]
        })
        .collect();
    let spacing = total / (count - 1) as f64;
    let half_window = ((220.0 / spacing).round() as usize).clamp(2, 16);
    let smooth: Vec<f32> = (0..median5.len())
        .map(|i| {
            let lo = i.saturating_sub(half_window);
            let hi = (i + half_window).min(median5.len() - 1);
            let sum: f32 = median5[lo..=hi].iter().sum();
            sum / (hi - lo + 1) as f32
        })
        .collect();
    // Hysteresis: direction changes only commit once they exceed 2m, so
    // meter-scale noise cancels instead of accumulating.
    const CLIMB_THRESHOLD_M: f32 = 2.5;
    let (mut ascent, mut descent) = (0.0f32, 0.0f32);
    let mut anchor = smooth[0];
    for &elev in &smooth[1..] {
        let delta = elev - anchor;
        if delta >= CLIMB_THRESHOLD_M {
            ascent += delta;
            anchor = elev;
        } else if delta <= -CLIMB_THRESHOLD_M {
            descent -= delta;
            anchor = elev;
        }
    }
    // Graph and stats show the smoothed ground line, not the roof spikes.
    let samples: Vec<(f32, f32)> = samples
        .iter()
        .zip(&smooth)
        .map(|(&(d, _), &e)| (d, e))
        .collect();
    let min_elev = samples.iter().map(|s| s.1).fold(f32::MAX, f32::min);
    let max_elev = samples.iter().map(|s| s.1).fold(f32::MIN, f32::max);
    Some(ElevationProfile {
        samples,
        total_m: total as f32,
        min_elev,
        max_elev,
        ascent_m: ascent,
        descent_m: descent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_amsterdam() {
        if !std::path::Path::new("../../local/maps/dem/N52E004.hgt").exists() {
            return;
        }
        let mut cache = DemCache::new("../../local/maps/dem");
        let e = cache.sample_m(LonLat::new(4.88, 52.36));
        println!("elev@ams: {:?}", e);
        assert!(e.is_some());
        let points = vec![LonLat::new(4.88, 52.36), LonLat::new(4.95, 52.37)];
        let profile = route_profile(&mut cache, &points, 200).unwrap();
        println!(
            "profile: n={} min={} max={} up={} down={}",
            profile.samples.len(),
            profile.min_elev,
            profile.max_elev,
            profile.ascent_m,
            profile.descent_m
        );
        // Amsterdam is flat: phantom climb must stay in single digits.
        assert!(profile.ascent_m < 10.0, "ascent {}", profile.ascent_m);
        assert!(profile.max_elev - profile.min_elev < 12.0);
    }
}
