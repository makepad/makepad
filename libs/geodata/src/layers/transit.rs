//! Public transport from the OVapi static GTFS bundle (CC0): all NL stops as
//! points plus rail/tram/metro/ferry route shapes as lines (bus shapes are
//! skipped — they follow the roads that are already on the map and triple the
//! archive size). Output: nl-transit.mbtiles, MVT layers `stops` (z10-z14)
//! and `routes` (z7-z12).
//!
//! Live vehicle positions (GTFS-RT) are deliberately NOT here: that is a
//! runtime concern for the map app.

use super::{BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::mvt::AttrVal;
use crate::tiler::{Tileset, TilesetConfig};
use crate::wkb::Geometry;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

const OVAPI_GTFS: SourceSpec = SourceSpec {
    id: "ovapi-gtfs",
    url: "https://gtfs.openov.nl/gtfs-rt/gtfs-openov-nl.zip",
    filename: "gtfs-openov-nl.zip",
    license: "CC0",
    attribution: "OVapi / NDOV loket",
    // Refreshed daily upstream; weekly is plenty for stop/route geometry.
    recheck_days: 7,
    limit_rate: None,
};

pub struct TransitLayer;

impl Layer for TransitLayer {
    fn id(&self) -> &'static str {
        "transit"
    }
    fn description(&self) -> &'static str {
        "All NL transit stops + rail/tram/metro/ferry route lines (OVapi GTFS, CC0)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![OVAPI_GTFS]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let zip = ctx.cached(&OVAPI_GTFS);
        if !zip.exists() {
            return Err("source not fetched yet (run: geodata fetch transit)".into());
        }

        // routes.txt: route_id -> (route_type, name)
        let mut route_info: HashMap<String, (i64, String)> = HashMap::new();
        stream_gtfs_csv(&zip, "routes.txt", |row| {
            let (Some(id), Some(rtype)) = (row.get("route_id"), row.get("route_type")) else {
                return;
            };
            let name = row
                .get("route_short_name")
                .filter(|s| !s.is_empty())
                .or_else(|| row.get("route_long_name"))
                .cloned()
                .unwrap_or_default();
            route_info.insert(id.clone(), (rtype.parse().unwrap_or(3), name));
        })?;

        // trips.txt: shape_id -> route_id (first trip wins; direction variants
        // produce distinct shape_ids so nothing is lost).
        let mut shape_route: HashMap<String, String> = HashMap::new();
        stream_gtfs_csv(&zip, "trips.txt", |row| {
            let (Some(shape), Some(route)) = (row.get("shape_id"), row.get("route_id")) else {
                return;
            };
            if shape.is_empty() {
                return;
            }
            shape_route
                .entry(shape.clone())
                .or_insert_with(|| route.clone());
        })?;

        // Which shapes do we keep? Everything except bus (route_type 3 and the
        // extended 700-series bus codes).
        let keep_shape: HashMap<&String, (&i64, &String)> = shape_route
            .iter()
            .filter_map(|(shape, route)| {
                let (rtype, name) = route_info.get(route)?;
                let is_bus = *rtype == 3 || (700..800).contains(rtype);
                if is_bus {
                    None
                } else {
                    Some((shape, (rtype, name)))
                }
            })
            .collect();

        let mut tileset = Tileset::new();
        let mut features = 0u64;

        // stops.txt -> points
        stream_gtfs_csv(&zip, "stops.txt", |row| {
            let (Some(lat), Some(lon)) = (row.get("stop_lat"), row.get("stop_lon")) else {
                return;
            };
            let (Ok(lat), Ok(lon)) = (lat.parse::<f64>(), lon.parse::<f64>()) else {
                return;
            };
            if !(50.0..54.2).contains(&lat) || !(2.5..7.5).contains(&lon) {
                return;
            }
            let mut attrs: Vec<(String, AttrVal)> = Vec::new();
            if let Some(name) = row.get("stop_name") {
                if !name.is_empty() {
                    attrs.push(("name".into(), AttrVal::Str(name.clone())));
                }
            }
            let is_station = row.get("location_type").map(|s| s.as_str()) == Some("1");
            if is_station {
                attrs.push(("station".into(), AttrVal::Bool(true)));
            }
            // Stations get a wider zoom range than local stops.
            let zmin = if is_station { 8 } else { 10 };
            tileset.add("stops", zmin, 14, &Geometry::Point(lon, lat), &attrs);
            features += 1;
        })?;

        // shapes.txt -> route lines (streamed; shapes arrive grouped by id,
        // but don't rely on it — accumulate per shape id, flush at the end).
        let mut shapes: HashMap<String, Vec<(f64, f64, i64)>> = HashMap::new();
        stream_gtfs_csv(&zip, "shapes.txt", |row| {
            let Some(id) = row.get("shape_id") else { return };
            if !keep_shape.contains_key(id) {
                return;
            }
            let (Some(lat), Some(lon), Some(seq)) = (
                row.get("shape_pt_lat"),
                row.get("shape_pt_lon"),
                row.get("shape_pt_sequence"),
            ) else {
                return;
            };
            let (Ok(lat), Ok(lon), Ok(seq)) =
                (lat.parse::<f64>(), lon.parse::<f64>(), seq.parse::<i64>())
            else {
                return;
            };
            shapes.entry(id.clone()).or_default().push((lon, lat, seq));
        })?;
        let shape_count = shapes.len();
        for (shape_id, mut pts) in shapes {
            let Some((rtype, name)) = keep_shape.get(&shape_id) else {
                continue;
            };
            pts.sort_by_key(|&(_, _, seq)| seq);
            let line: Vec<(f64, f64)> = pts.iter().map(|&(lon, lat, _)| (lon, lat)).collect();
            if line.len() < 2 {
                continue;
            }
            let mode = match **rtype {
                0 | 900..=906 => "tram",
                1 | 400..=404 => "metro",
                2 | 100..=117 => "rail",
                4 | 1000 | 1200 => "ferry",
                _ => "other",
            };
            let mut attrs = vec![("mode".to_string(), AttrVal::Str(mode.into()))];
            if !name.is_empty() {
                attrs.push(("ref".into(), AttrVal::Str((*name).clone())));
            }
            tileset.add("routes", 7, 14, &Geometry::LineString(line), &attrs);
            features += 1;
        }
        eprintln!("  transit: {features} features ({shape_count} non-bus shapes)");

        let out = ctx.out_file(self.id());
        let stats = tileset.finish(
            &out,
            &TilesetConfig {
                name: "nl-transit".into(),
                description: "NL transit stops + non-bus route shapes (OVapi GTFS)".into(),
                attribution: "OVapi / NDOV".into(),
                license: "CC0".into(),
                minzoom: 7,
                maxzoom: 14,
            },
        )?;
        Ok(BuildReport {
            out_path: out,
            features,
            tiles: stats.tiles,
            bytes: stats.bytes,
        })
    }
}

/// Stream one CSV member of a zip through a row callback without extracting
/// to disk. Handles quoted fields and the UTF-8 BOM.
fn stream_gtfs_csv(
    zip: &Path,
    member: &str,
    mut callback: impl FnMut(&HashMap<String, String>),
) -> Result<(), String> {
    let mut child = Command::new("unzip")
        .arg("-p")
        .arg(zip)
        .arg(member)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("unzip -p {member}: {e}"))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = BufReader::with_capacity(1 << 20, stdout);

    let mut header: Vec<String> = Vec::new();
    let mut row: HashMap<String, String> = HashMap::new();
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read {member}: {e}"))?;
        let fields = parse_csv_line(&line);
        if header.is_empty() {
            header = fields
                .into_iter()
                .map(|f| f.trim_start_matches('\u{feff}').to_string())
                .collect();
            continue;
        }
        row.clear();
        for (i, field) in fields.into_iter().enumerate() {
            if let Some(key) = header.get(i) {
                row.insert(key.clone(), field);
            }
        }
        callback(&row);
    }
    let status = child.wait().map_err(|e| format!("unzip wait: {e}"))?;
    if !status.success() {
        return Err(format!("unzip -p {member} failed (missing member?)"));
    }
    Ok(())
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}
