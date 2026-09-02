//! Structured queries against a layer database's `features` sidecar table —
//! the "reason with the map" surface. The map app exposes these as LLM tool
//! calls ("what is at/near this location?") and for tap-to-inspect UI.
//!
//! Queries are b-tree range scans over the grid-indexed rowids; a typical
//! radius query touches a few dozen pages of the .mbtiles file.

use crate::sidecar::{CELL_AXIS, FEATURES_TABLE};
use makepad_mbtile_reader::{MbtilesReader, Value};
use makepad_map_nav::geo::{haversine_m, LonLat};
use std::path::Path;

/// Prevent callers on the interactive app path from accidentally scanning an
/// unbounded dense cell. Services with a whole-request budget use
/// `query_radius_with_budget` directly.
pub const DEFAULT_RADIUS_SCAN_BUDGET: usize = 50_000;
pub const MAX_RADIUS_RESULTS: usize = 50_000;

fn radius_result_capacity(limit: usize, candidate_budget: usize) -> Result<usize, String> {
    if limit > MAX_RADIUS_RESULTS {
        return Err(format!("radius result limit exceeds {MAX_RADIUS_RESULTS}"));
    }
    Ok(limit.min(candidate_budget))
}

#[derive(Debug)]
pub struct FeatureHit {
    pub layer: String,
    pub name: Option<String>,
    /// Feature attributes as parsed JSON.
    pub attrs: serde_json::Value,
    /// Feature bbox center.
    pub center: (f64, f64),
    pub bbox: (f64, f64, f64, f64),
    /// Distance from the query point (radius/point queries).
    pub distance_m: Option<f64>,
    /// For point queries on ring-carrying layers: exact containment.
    pub contains_point: bool,
}

pub struct LayerDb {
    db: MbtilesReader,
}

impl LayerDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = MbtilesReader::open_sqlite(path)
            .map_err(|e| format!("open {}: {e:?}", path.display()))?;
        Ok(LayerDb { db })
    }

    /// All features whose bbox intersects the query bbox.
    pub fn query_bbox(
        &mut self,
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
        limit: usize,
    ) -> Result<Vec<FeatureHit>, String> {
        let mut hits = Vec::new();
        // Note y grows south in normalized mercator: max_lat -> min cy.
        let (nx0, ny0) = crate::geo::wgs84_to_norm(min_lon, max_lat);
        let (nx1, ny1) = crate::geo::wgs84_to_norm(max_lon, min_lat);
        let clamp = |v: f64| (v * f64::from(CELL_AXIS)) as i64;
        let cx0 = clamp(nx0).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let cx1 = clamp(nx1).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let cy0 = clamp(ny0).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let cy1 = clamp(ny1).clamp(0, i64::from(CELL_AXIS) - 1) as u32;

        'rows: for cy in cy0..=cy1 {
            let lo = (i64::from(cy * CELL_AXIS + cx0)) << 24;
            let hi = ((i64::from(cy * CELL_AXIS + cx1)) << 24) | 0x00ff_ffff;
            let mut scan_err = None;
            let result = self.db.for_each_row_in_range(
                FEATURES_TABLE,
                lo,
                hi,
                |_rowid, values| {
                    if hits.len() >= limit {
                        return;
                    }
                    match parse_hit(&values) {
                        Ok(hit) => {
                            let bb = hit.bbox;
                            if bb.0 <= max_lon && bb.2 >= min_lon && bb.1 <= max_lat && bb.3 >= min_lat {
                                hits.push(hit);
                            }
                        }
                        Err(e) => scan_err = Some(e),
                    }
                },
            );
            result.map_err(|e| format!("range scan: {e:?}"))?;
            if let Some(e) = scan_err {
                return Err(e);
            }
            if hits.len() >= limit {
                break 'rows;
            }
        }
        Ok(hits)
    }

    /// Features within `radius_m` of a point, nearest first.
    pub fn query_radius(
        &mut self,
        lon: f64,
        lat: f64,
        radius_m: f64,
        limit: usize,
    ) -> Result<Vec<FeatureHit>, String> {
        let mut budget = DEFAULT_RADIUS_SCAN_BUDGET;
        self.query_radius_with_budget(lon, lat, radius_m, limit, &mut budget)
    }

    /// Bounded nearest-neighbor selection. At most `candidate_budget` feature
    /// rows are decoded across all calls sharing that mutable budget, and only
    /// the nearest `limit` hits are retained in memory.
    pub fn query_radius_with_budget(
        &mut self,
        lon: f64,
        lat: f64,
        radius_m: f64,
        limit: usize,
        candidate_budget: &mut usize,
    ) -> Result<Vec<FeatureHit>, String> {
        let result_capacity = radius_result_capacity(limit, *candidate_budget)?;
        if result_capacity == 0 {
            return Ok(Vec::new());
        }
        // Convert the radius to a degree bbox (safe overestimate at NL lat).
        let dlat = radius_m / 111_320.0;
        let dlon = radius_m / (111_320.0 * lat.to_radians().cos().max(0.2));
        let min_lon = lon - dlon;
        let min_lat = lat - dlat;
        let max_lon = lon + dlon;
        let max_lat = lat + dlat;
        let (nx0, ny0) = crate::geo::wgs84_to_norm(min_lon, max_lat);
        let (nx1, ny1) = crate::geo::wgs84_to_norm(max_lon, min_lat);
        let clamp = |value: f64| (value * f64::from(CELL_AXIS)) as i64;
        let cx0 = clamp(nx0).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let cx1 = clamp(nx1).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let cy0 = clamp(ny0).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let cy1 = clamp(ny1).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
        let origin = LonLat::new(lon, lat);
        let mut hits: Vec<FeatureHit> = Vec::with_capacity(result_capacity);
        'cells: for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                if *candidate_budget == 0 {
                    break 'cells;
                }
                let lo = (i64::from(cy * CELL_AXIS + cx)) << 24;
                let hi = lo | ((*candidate_budget - 1).min(0x00ff_ffff) as i64);
                let mut scan_error = None;
                self.db
                    .for_each_row_in_range(FEATURES_TABLE, lo, hi, |_rowid, values| {
                        if *candidate_budget == 0 {
                            return;
                        }
                        *candidate_budget -= 1;
                        let mut hit = match parse_hit(&values) {
                            Ok(hit) => hit,
                            Err(error) => {
                                scan_error = Some(error);
                                return;
                            }
                        };
                        let bbox = hit.bbox;
                        if bbox.0 > max_lon
                            || bbox.2 < min_lon
                            || bbox.1 > max_lat
                            || bbox.3 < min_lat
                        {
                            return;
                        }
                        let distance = haversine_m(
                            origin,
                            LonLat::new(hit.center.0, hit.center.1),
                        );
                        if distance > radius_m {
                            return;
                        }
                        hit.distance_m = Some(distance);
                        if hits.len() < limit {
                            hits.push(hit);
                        } else if let Some((farthest, farthest_distance)) = hits
                            .iter()
                            .enumerate()
                            .max_by(|(_, a), (_, b)| {
                                a.distance_m.unwrap().total_cmp(&b.distance_m.unwrap())
                            })
                            .map(|(index, hit)| (index, hit.distance_m.unwrap()))
                        {
                            if distance < farthest_distance {
                                hits[farthest] = hit;
                            }
                        }
                    })
                    .map_err(|error| format!("range scan: {error:?}"))?;
                if let Some(error) = scan_error {
                    return Err(error);
                }
            }
        }
        hits.sort_by(|a, b| a.distance_m.unwrap().total_cmp(&b.distance_m.unwrap()));
        Ok(hits)
    }

    /// Features covering a point. Exact for layers that store rings (and for
    /// grid layers whose bbox equals the cell); bbox containment otherwise.
    pub fn query_point(&mut self, lon: f64, lat: f64, limit: usize) -> Result<Vec<FeatureHit>, String> {
        if limit > MAX_RADIUS_RESULTS {
            return Err(format!("point result limit exceeds {MAX_RADIUS_RESULTS}"));
        }
        let epsilon = 1e-9;
        let mut hits = self.query_bbox(
            lon - epsilon,
            lat - epsilon,
            lon + epsilon,
            lat + epsilon,
            MAX_RADIUS_RESULTS,
        )?;
        for hit in &mut hits {
            hit.contains_point = match hit.attrs.get("__ring") {
                Some(serde_json::Value::Array(ring)) => point_in_ring(lon, lat, ring),
                _ => true, // bbox containment already established
            };
        }
        hits.retain(|h| h.contains_point);
        hits.truncate(limit);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::SidecarBuilder;
    use makepad_mbtile_reader::MbtilesWriter;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn radius_limit_is_capped_before_allocation() {
        assert_eq!(radius_result_capacity(8, 3).unwrap(), 3);
        assert!(radius_result_capacity(usize::MAX, DEFAULT_RADIUS_SCAN_BUDGET).is_err());

        let path = std::env::temp_dir().join(format!(
            "makepad-radius-limit-{}-{}.mbtiles",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut writer = MbtilesWriter::create(&path).unwrap();
        SidecarBuilder::new().write(&mut writer).unwrap();
        writer.finish().unwrap();
        let mut database = LayerDb::open(&path).unwrap();
        assert!(database.query_radius(4.9, 52.3, 1_000.0, usize::MAX).is_err());
        assert!(database.query_point(4.9, 52.3, usize::MAX).is_err());
        std::fs::remove_file(path).unwrap();
    }
}

fn parse_hit(values: &[Value]) -> Result<FeatureHit, String> {
    let text = |i: usize| -> Option<String> {
        values.get(i).and_then(|v| v.as_text()).map(str::to_string)
    };
    let float = |i: usize| -> f64 {
        match values.get(i) {
            Some(Value::Float(f)) => *f,
            Some(Value::Integer(n)) => *n as f64,
            _ => f64::NAN,
        }
    };
    let layer = text(1).unwrap_or_default();
    let name = text(2);
    let bbox = (float(3), float(4), float(5), float(6));
    let mut attrs: serde_json::Value = text(7)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    if let Some(ring_text) = text(8) {
        if let Ok(ring) = serde_json::from_str::<serde_json::Value>(&ring_text) {
            if let Some(map) = attrs.as_object_mut() {
                map.insert("__ring".into(), ring);
            }
        }
    }
    Ok(FeatureHit {
        layer,
        name,
        attrs,
        center: ((bbox.0 + bbox.2) / 2.0, (bbox.1 + bbox.3) / 2.0),
        bbox,
        distance_m: None,
        contains_point: false,
    })
}

/// Ray-cast point-in-polygon on a JSON ring [[lon,lat],...].
fn point_in_ring(lon: f64, lat: f64, ring: &[serde_json::Value]) -> bool {
    let pts: Vec<(f64, f64)> = ring
        .iter()
        .filter_map(|p| {
            let arr = p.as_array()?;
            Some((arr.first()?.as_f64()?, arr.get(1)?.as_f64()?))
        })
        .collect();
    if pts.len() < 3 {
        return true;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if ((yi > lat) != (yj > lat))
            && (lon < (xj - xi) * (lat - yi) / (yj - yi) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}
