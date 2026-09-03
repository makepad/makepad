//! 10 m wind field for the particle layer — NOAA GFS via the NOMADS filter
//! CGI, which subsets by variable + bbox SERVER-side: one ~3 KB GRIB2 with
//! the UGRD/VGRD 10 m fields over the region. US public domain, no key.
//!
//! Same app-embeddable contract as [`crate::radar`]: `sync()` polls at most
//! once per `min_poll_secs` through a disk-persisted gate and caches the
//! last good field on disk; `WindField` decode is a minimal GRIB2 reader
//! for exactly what NOMADS serves (template 3.0 lat-lon grid, template 5.0
//! simple packing, no bitmap) — anything else errors instead of guessing.

use std::path::PathBuf;

/// Subset region (lon 2..9 E, lat 48..56 N covers NL + approaches).
pub const WIND_WEST: f64 = 2.0;
pub const WIND_EAST: f64 = 9.0;
pub const WIND_SOUTH: f64 = 48.0;
pub const WIND_NORTH: f64 = 56.0;

#[derive(Debug, Clone)]
pub struct WindField {
    pub nx: usize,
    pub ny: usize,
    /// Row 0 = SOUTH edge (lat1), scanning west→east; u east+, v north+ (m/s).
    pub u: Vec<f32>,
    pub v: Vec<f32>,
    pub west: f64,
    pub east: f64,
    pub south: f64,
    pub north: f64,
}

impl WindField {
    /// Bilinear sample at lon/lat; None outside the grid.
    pub fn sample(&self, lon: f64, lat: f64) -> Option<(f32, f32)> {
        let fx = (lon - self.west) / (self.east - self.west) * (self.nx - 1) as f64;
        let fy = (lat - self.south) / (self.north - self.south) * (self.ny - 1) as f64;
        if fx < 0.0 || fy < 0.0 || fx > (self.nx - 1) as f64 || fy > (self.ny - 1) as f64 {
            return None;
        }
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let (x1, y1) = ((x0 + 1).min(self.nx - 1), (y0 + 1).min(self.ny - 1));
        let at = |x: usize, y: usize| y * self.nx + x;
        let lerp2 = |g: &[f32]| {
            let top = g[at(x0, y0)] * (1.0 - tx) + g[at(x1, y0)] * tx;
            let bottom = g[at(x0, y1)] * (1.0 - tx) + g[at(x1, y1)] * tx;
            top * (1.0 - ty) + bottom * ty
        };
        Some((lerp2(&self.u), lerp2(&self.v)))
    }
}

fn u16be(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
fn u32be(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i32be_signmag(b: &[u8], o: usize) -> i64 {
    let raw = u32be(b, o);
    if raw & 0x8000_0000 != 0 {
        -((raw & 0x7fff_ffff) as i64)
    } else {
        raw as i64
    }
}
fn i16be_signmag(b: &[u8], o: usize) -> i32 {
    let raw = u16be(b, o);
    if raw & 0x8000 != 0 {
        -((raw & 0x7fff) as i32)
    } else {
        raw as i32
    }
}

struct GribField {
    ni: usize,
    nj: usize,
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    /// GRIB product category/number: (2,2)=UGRD, (2,3)=VGRD.
    param: (u8, u8),
    values: Vec<f32>,
    /// scan flag bit 0x40: +j goes south→north when lat1 < lat2.
    j_positive: bool,
}

/// Decode every message of a (concatenated) GRIB2 file — the fixed NOMADS
/// shape only: grid template 3.0, data representation 5.0, no bitmap.
fn decode_grib2(data: &[u8]) -> Result<Vec<GribField>, String> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 16 <= data.len() {
        if &data[pos..pos + 4] != b"GRIB" {
            return Err(format!("no GRIB magic at {pos}"));
        }
        let total = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap()) as usize;
        let msg = &data[pos..pos + total.min(data.len() - pos)];
        let mut p = 16usize;
        let mut grid: Option<(usize, usize, f64, f64, f64, f64, bool)> = None;
        let mut param = (0u8, 0u8);
        let mut packing: Option<(usize, f32, i32, i32, u8)> = None;
        let mut values: Option<Vec<f32>> = None;
        while p + 5 <= msg.len() {
            if &msg[p..(p + 4).min(msg.len())] == b"7777" {
                break;
            }
            let seclen = u32be(msg, p) as usize;
            let secnum = msg[p + 4];
            let body = &msg[p..(p + seclen).min(msg.len())];
            match secnum {
                3 => {
                    let template = u16be(body, 12);
                    if template != 0 {
                        return Err(format!("unsupported grid template {template}"));
                    }
                    let ni = u32be(body, 30) as usize;
                    let nj = u32be(body, 34) as usize;
                    let lat1 = i32be_signmag(body, 46) as f64 / 1e6;
                    let lon1 = i32be_signmag(body, 50) as f64 / 1e6;
                    let lat2 = i32be_signmag(body, 55) as f64 / 1e6;
                    let lon2 = i32be_signmag(body, 59) as f64 / 1e6;
                    let scan = body[71];
                    grid = Some((ni, nj, lat1, lon1, lat2, lon2, scan & 0x40 != 0));
                }
                4 => {
                    param = (body[9], body[10]);
                }
                5 => {
                    let npts = u32be(body, 5) as usize;
                    let template = u16be(body, 9);
                    if template != 0 {
                        return Err(format!("unsupported packing template {template}"));
                    }
                    let reference = f32::from_be_bytes(body[11..15].try_into().unwrap());
                    let e = i16be_signmag(body, 15);
                    let d = i16be_signmag(body, 17);
                    let nbits = body[19];
                    packing = Some((npts, reference, e, d, nbits));
                }
                6 => {
                    if body[5] != 255 {
                        return Err("bitmapped GRIB2 not supported".into());
                    }
                }
                7 => {
                    let Some((npts, reference, e, d, nbits)) = packing else {
                        return Err("data before packing section".into());
                    };
                    let bits = &body[5..];
                    let mut vals = Vec::with_capacity(npts);
                    let (mut acc, mut nacc, mut i) = (0u64, 0u32, 0usize);
                    let scale = 2f64.powi(e) / 10f64.powi(d);
                    for _ in 0..npts {
                        while nacc < nbits as u32 {
                            if i >= bits.len() {
                                return Err("GRIB2 data section truncated".into());
                            }
                            acc = (acc << 8) | bits[i] as u64;
                            i += 1;
                            nacc += 8;
                        }
                        nacc -= nbits as u32;
                        let raw = (acc >> nacc) & ((1u64 << nbits) - 1);
                        vals.push((reference as f64 + raw as f64 * 2f64.powi(e)) as f32
                            / 10f32.powi(d));
                        let _ = scale;
                    }
                    values = Some(vals);
                }
                _ => {}
            }
            p += seclen.max(1);
        }
        if let (Some((ni, nj, lat1, lon1, lat2, lon2, j_positive)), Some(values)) = (grid, values)
        {
            out.push(GribField {
                ni,
                nj,
                lat1,
                lon1,
                lat2,
                lon2,
                param,
                values,
                j_positive,
            });
        }
        pos += total;
    }
    Ok(out)
}

/// Combine decoded UGRD/VGRD messages into a south-row-first WindField.
pub fn decode_wind(data: &[u8]) -> Result<WindField, String> {
    let fields = decode_grib2(data)?;
    let u = fields
        .iter()
        .find(|f| f.param == (2, 2))
        .ok_or("no UGRD message")?;
    let v = fields
        .iter()
        .find(|f| f.param == (2, 3))
        .ok_or("no VGRD message")?;
    if u.ni != v.ni || u.nj != v.nj {
        return Err("u/v grid mismatch".into());
    }
    let (nx, ny) = (u.ni, u.nj);
    let reorder = |f: &GribField| -> Vec<f32> {
        // Normalize to row 0 = SOUTH regardless of scan direction.
        let south_first = if f.j_positive {
            f.lat1 < f.lat2
        } else {
            f.lat1 > f.lat2
        };
        let mut out = vec![0f32; nx * ny];
        for j in 0..ny {
            let src_row = if south_first { j } else { ny - 1 - j };
            out[j * nx..(j + 1) * nx]
                .copy_from_slice(&f.values[src_row * nx..(src_row + 1) * nx]);
        }
        out
    };
    let (south, north) = (u.lat1.min(u.lat2), u.lat1.max(u.lat2));
    let (west, east) = (u.lon1.min(u.lon2), u.lon1.max(u.lon2));
    Ok(WindField {
        nx,
        ny,
        u: reorder(u),
        v: reorder(v),
        west,
        east,
        south,
        north,
    })
}

pub struct WindSync {
    pub cache_dir: PathBuf,
    /// GFS publishes 4 runs/day with multi-hour latency; 30 min polls are
    /// plenty and the gate persists on disk.
    pub min_poll_secs: u64,
}

impl WindSync {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            min_poll_secs: 1800,
        }
    }

    fn now_unix() -> u64 {
        crate::clock::now_unix()
    }

    /// Cached field without any network contact.
    pub fn cached(&self) -> Option<WindField> {
        let data = std::fs::read(self.cache_dir.join("wind_nl.grib2")).ok()?;
        decode_wind(&data).ok()
    }

    pub fn cached_with_stamp(&self) -> Option<(u64, WindField)> {
        let field = self.cached()?;
        let stamp = std::fs::read_to_string(self.cache_dir.join("wind_updated_unix"))
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .or_else(|| {
                std::fs::metadata(self.cache_dir.join("wind_nl.grib2"))
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
            })?;
        Some((stamp, field))
    }

    /// Poll-gated fetch: tries the newest plausible GFS runs (each run
    /// appears ~4 h after its nominal time) until one returns a GRIB.
    pub fn sync(&self) -> Result<Option<WindField>, String> {
        std::fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("create {}: {e}", self.cache_dir.display()))?;
        let gate = self.cache_dir.join("last_poll_unix");
        let now = Self::now_unix();
        let last = std::fs::read_to_string(&gate)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if now.saturating_sub(last) < self.min_poll_secs {
            return Ok(self.cached());
        }
        // Arm the gate BEFORE network contact (see radar.rs).
        let _ = std::fs::write(&gate, now.to_string());

        // Candidate runs: step back in 6 h increments from now-5h.
        for step in 0..4u64 {
            let t = now - 5 * 3600 - step * 6 * 3600;
            let days = t / 86400;
            let (y, m, d) = civil_from_days(days as i64);
            let hour = (t % 86400) / 3600 / 6 * 6;
            let date = format!("{y:04}{m:02}{d:02}");
            let run = format!("{hour:02}");
            let url = format!(
                "https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl?\
                 dir=%2Fgfs.{date}%2F{run}%2Fatmos&file=gfs.t{run}z.pgrb2.0p25.f000\
                 &var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on&subregion=\
                 &toplat={}&leftlon={}&rightlon={}&bottomlat={}",
                WIND_NORTH, WIND_WEST, WIND_EAST, WIND_SOUTH
            );
            let tmp = self.cache_dir.join("wind_nl.grib2.part");
            let Ok(response) = crate::http_fetch::get(&url, None, 1024 * 1024) else {
                continue;
            };
            if !(200..300).contains(&response.status) {
                continue;
            }
            if decode_wind(&response.body).is_ok() {
                std::fs::write(&tmp, &response.body)
                    .map_err(|e| format!("write wind download: {e}"))?;
                let final_path = self.cache_dir.join("wind_nl.grib2");
                std::fs::rename(&tmp, &final_path)
                    .map_err(|e| format!("rename: {e}"))?;
                std::fs::write(self.cache_dir.join("wind_updated_unix"), now.to_string())
                    .map_err(|e| format!("write wind update time: {e}"))?;
                return Ok(self.cached());
            }
        }
        Ok(self.cached())
    }
}

/// days-since-epoch → (y, m, d) — civil calendar, no chrono dependency.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_nomads_probe() {
        let path = concat!(
            "/private/tmp/claude-501/-Users-admin-makepad-makepad/",
            "2360ceda-2c5b-45d8-91f7-072799a0d3d9/scratchpad/wind_probe.grib2"
        );
        let Ok(data) = std::fs::read(path) else {
            return;
        };
        let field = decode_wind(&data).unwrap();
        assert_eq!((field.nx, field.ny), (29, 33));
        assert_eq!((field.west, field.east, field.south, field.north), (2.0, 9.0, 48.0, 56.0));
        // Reference values from the python decode: u first row starts
        // -2.15, -2.05... (lat1=48 south, scan +j → row 0 is already south).
        assert!((field.u[0] - -2.15).abs() < 0.01, "u0 {}", field.u[0]);
        assert!((field.v[3] - 0.03).abs() < 0.01, "v3 {}", field.v[3]);
        let amsterdam = field.sample(4.9, 52.37).unwrap();
        assert!(amsterdam.0.abs() < 30.0 && amsterdam.1.abs() < 30.0);
    }
}
