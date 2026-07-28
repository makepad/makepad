//! The `features` sidecar table: a grid-indexed, JSON-attributed copy of
//! every feature, written into the same .mbtiles as the render tiles.
//!
//! Purpose: the render `tiles` are for pixels; this table is for *questions*.
//! The map app (or an LLM tool call routed through it) asks "what is here /
//! near here" via `query::LayerDb` and gets structured features back without
//! touching MVT.
//!
//! Spatial index: rowid = (z12 grid cell << 24) | seq, so a bbox query turns
//! into a handful of b-tree range scans (`for_each_row_in_range`) — no SQL,
//! no separate index. Polygon layers can opt into storing a simplified
//! exterior ring for exact point-in-polygon answers ("which buurt am I in").

use crate::mvt::AttrVal;
use crate::wkb::Geometry;
use makepad_mbtile_reader::{MbtilesWriter, WriterValue};

/// Grid zoom for the cell index: 4096 x 4096 cells world-wide, ~10 km cells
/// at NL latitude — small enough to prune, big enough to keep ranges few.
pub const CELL_ZOOM: u8 = 12;
pub const CELL_AXIS: u32 = 1 << CELL_ZOOM;

pub const FEATURES_TABLE: &str = "features";
pub const FEATURES_SQL: &str = "CREATE TABLE features (cell INTEGER, layer TEXT, name TEXT, \
     min_lon REAL, min_lat REAL, max_lon REAL, max_lat REAL, attrs TEXT, ring TEXT)";

const NAME_KEYS: &[&str] = &[
    "name", "naam", "naam_n2k", "statnaam", "gemeentenaam", "wijknaam", "buurtnaam", "ref",
];
const MAX_RING_POINTS: usize = 96;
const RING_TOLERANCE_DEG: f64 = 1e-4; // ~8-11 m

pub fn cell_for(lon: f64, lat: f64) -> u32 {
    let (nx, ny) = crate::geo::wgs84_to_norm(lon, lat);
    let cx = ((nx * f64::from(CELL_AXIS)) as i64).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
    let cy = ((ny * f64::from(CELL_AXIS)) as i64).clamp(0, i64::from(CELL_AXIS) - 1) as u32;
    cy * CELL_AXIS + cx
}

pub fn rowid_for(cell: u32, seq: u32) -> i64 {
    (i64::from(cell) << 24) | i64::from(seq & 0x00ff_ffff)
}

struct Record {
    cell: u32,
    layer: String,
    name: Option<String>,
    bbox: (f64, f64, f64, f64),
    attrs_json: String,
    ring_json: Option<String>,
}

#[derive(Default)]
pub struct SidecarBuilder {
    records: Vec<Record>,
}

impl SidecarBuilder {
    pub fn new() -> Self {
        SidecarBuilder::default()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn add(
        &mut self,
        layer: &str,
        geometry: &Geometry,
        attrs: &[(String, AttrVal)],
        want_ring: bool,
    ) {
        let Some(bbox) = geometry_bbox(geometry) else {
            return;
        };
        let center = ((bbox.0 + bbox.2) / 2.0, (bbox.1 + bbox.3) / 2.0);
        let name = attrs
            .iter()
            .find(|(k, _)| NAME_KEYS.contains(&k.as_str()))
            .and_then(|(_, v)| match v {
                AttrVal::Str(s) => Some(s.clone()),
                _ => None,
            });
        let mut map = serde_json::Map::with_capacity(attrs.len());
        for (key, value) in attrs {
            let json = match value {
                AttrVal::Str(s) => serde_json::Value::String(s.clone()),
                AttrVal::Int(i) => serde_json::Value::from(*i),
                AttrVal::Float(f) => serde_json::Value::from(*f),
                AttrVal::Bool(b) => serde_json::Value::Bool(*b),
            };
            map.insert(key.clone(), json);
        }
        let ring_json = if want_ring {
            exterior_ring(geometry).map(|ring| {
                let simplified = simplify_ring(&ring);
                let pts: Vec<serde_json::Value> = simplified
                    .iter()
                    .map(|&(lon, lat)| {
                        serde_json::json!([
                            (lon * 1e5).round() / 1e5,
                            (lat * 1e5).round() / 1e5
                        ])
                    })
                    .collect();
                serde_json::Value::Array(pts).to_string()
            })
        } else {
            None
        };
        self.records.push(Record {
            cell: cell_for(center.0, center.1),
            layer: layer.to_string(),
            name,
            bbox,
            attrs_json: serde_json::Value::Object(map).to_string(),
            ring_json,
        });
    }

    /// Sort by grid cell and stream into the writer's `features` table.
    pub fn write(mut self, writer: &mut MbtilesWriter) -> Result<u64, String> {
        if self.records.is_empty() {
            return Ok(0);
        }
        writer
            .begin_extra_table(FEATURES_TABLE, FEATURES_SQL)
            .map_err(|e| format!("declare features table: {e:?}"))?;
        self.records.sort_by_key(|r| r.cell);
        let mut count = 0u64;
        let mut seq = 0u32;
        let mut last_cell = u32::MAX;
        for record in &self.records {
            if record.cell != last_cell {
                seq = 0;
                last_cell = record.cell;
            } else {
                seq += 1;
                if seq > 0x00ff_ffff {
                    continue; // absurd density; drop rather than corrupt order
                }
            }
            let rowid = rowid_for(record.cell, seq);
            let values = [
                WriterValue::Integer(i64::from(record.cell)),
                WriterValue::Text(&record.layer),
                match &record.name {
                    Some(name) => WriterValue::Text(name),
                    None => WriterValue::Null,
                },
                WriterValue::Float(record.bbox.0),
                WriterValue::Float(record.bbox.1),
                WriterValue::Float(record.bbox.2),
                WriterValue::Float(record.bbox.3),
                WriterValue::Text(&record.attrs_json),
                match &record.ring_json {
                    Some(ring) => WriterValue::Text(ring),
                    None => WriterValue::Null,
                },
            ];
            writer
                .write_extra_row(FEATURES_TABLE, rowid, &values)
                .map_err(|e| format!("write feature row: {e:?}"))?;
            count += 1;
        }
        Ok(count)
    }
}

fn geometry_bbox(geometry: &Geometry) -> Option<(f64, f64, f64, f64)> {
    let mut bbox = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let mut grow = |lon: f64, lat: f64| {
        bbox.0 = bbox.0.min(lon);
        bbox.1 = bbox.1.min(lat);
        bbox.2 = bbox.2.max(lon);
        bbox.3 = bbox.3.max(lat);
    };
    match geometry {
        Geometry::Point(lon, lat) => grow(*lon, *lat),
        Geometry::MultiPoint(pts) | Geometry::LineString(pts) => {
            pts.iter().for_each(|&(lon, lat)| grow(lon, lat))
        }
        Geometry::MultiLineString(lines) => lines
            .iter()
            .flatten()
            .for_each(|&(lon, lat)| grow(lon, lat)),
        Geometry::Polygon(rings) => rings
            .iter()
            .flatten()
            .for_each(|&(lon, lat)| grow(lon, lat)),
        Geometry::MultiPolygon(polys) => polys
            .iter()
            .flatten()
            .flatten()
            .for_each(|&(lon, lat)| grow(lon, lat)),
    }
    if bbox.0.is_finite() {
        Some(bbox)
    } else {
        None
    }
}

/// Exterior ring of the largest polygon part, by bbox area.
fn exterior_ring(geometry: &Geometry) -> Option<Vec<(f64, f64)>> {
    match geometry {
        Geometry::Polygon(rings) => rings.first().cloned(),
        Geometry::MultiPolygon(polys) => polys
            .iter()
            .filter_map(|rings| rings.first())
            .max_by(|a, b| {
                let area = |ring: &[(f64, f64)]| {
                    let bb = geometry_bbox(&Geometry::LineString(ring.to_vec()))
                        .unwrap_or((0.0, 0.0, 0.0, 0.0));
                    (bb.2 - bb.0) * (bb.3 - bb.1)
                };
                area(a).total_cmp(&area(b))
            })
            .cloned(),
        _ => None,
    }
}

fn simplify_ring(ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let tol2 = RING_TOLERANCE_DEG * RING_TOLERANCE_DEG;
    for &pt in ring {
        if let Some(&last) = out.last() {
            let dx = pt.0 - last.0;
            let dy = pt.1 - last.1;
            if dx * dx + dy * dy < tol2 {
                continue;
            }
        }
        out.push(pt);
    }
    if out.len() > MAX_RING_POINTS {
        let step = out.len().div_ceil(MAX_RING_POINTS);
        let mut thinned: Vec<(f64, f64)> =
            out.iter().step_by(step).copied().collect();
        if thinned.last() != out.last() {
            thinned.push(*out.last().unwrap());
        }
        return thinned;
    }
    out
}
