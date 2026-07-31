//! mbtiles-merge: combine several single-zoom overlay archives (e.g. the
//! per-strip bridge-dz bakes) into one. Inputs are given west→east; where
//! strips overlap (each strip's east margin duplicates the next strip's
//! core), the LATER input wins, so every overlap tile keeps the version
//! solved at the center of its own strip.

use makepad_mbtile_reader::{MbtilesReader, MbtilesWriter};
use std::collections::HashMap;
use std::path::Path;

pub fn merge(inputs: &[String], output: &str, zoom: u8) -> Result<(), String> {
    if inputs.len() < 2 {
        return Err("mbtiles-merge needs at least two inputs".to_string());
    }
    let axis = 1_i64 << zoom;

    // Later inputs overwrite earlier ones on (x, y) collisions.
    let mut tiles: HashMap<(u32, u32), Vec<u8>> = HashMap::new();
    let mut metadata = HashMap::new();
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for input in inputs {
        let mut reader = MbtilesReader::open(Path::new(input))
            .map_err(|e| format!("open {input}: {e}"))?;
        let meta = reader.get_metadata().unwrap_or_default();
        if let Some(b) = meta.get("bounds") {
            let parts: Vec<f64> = b.split(',').filter_map(|v| v.parse().ok()).collect();
            if parts.len() == 4 {
                bounds.0 = bounds.0.min(parts[0]);
                bounds.1 = bounds.1.min(parts[1]);
                bounds.2 = bounds.2.max(parts[2]);
                bounds.3 = bounds.3.max(parts[3]);
            }
        }
        metadata = meta;
        let mut count = 0usize;
        for tile in reader
            .get_tiles_at_zoom(i64::from(zoom))
            .map_err(|e| format!("read {input}: {e}"))?
        {
            // Rows are stored TMS; flip back to XYZ for ordering + rewrite.
            let x = u32::try_from(tile.tile_column)
                .map_err(|_| format!("{input}: negative tile column"))?;
            let y = u32::try_from(axis - 1 - tile.tile_row)
                .map_err(|_| format!("{input}: tile row out of range"))?;
            tiles.insert((x, y), tile.tile_data);
            count += 1;
        }
        eprintln!("mbtiles-merge: {input}: {count} tiles");
    }

    let mut writer =
        MbtilesWriter::create(Path::new(output)).map_err(|e| format!("create {output}: {e}"))?;
    for (name, value) in &metadata {
        if name != "bounds" {
            writer.set_metadata(name.clone(), value.clone());
        }
    }
    if bounds.0 != f64::MAX {
        writer.set_metadata(
            "bounds",
            format!("{:.7},{:.7},{:.7},{:.7}", bounds.0, bounds.1, bounds.2, bounds.3),
        );
    }

    // MbtilesWriter requires block-major rowid order.
    let mut ordered: Vec<((u32, u32), Vec<u8>)> = tiles.into_iter().collect();
    ordered.sort_by_key(|((x, y), _)| (y >> 8, x >> 8, y & 255, x & 255));
    let total = ordered.len();
    for ((x, y), data) in ordered {
        writer
            .write_tile_xyz(zoom, x, y, &data)
            .map_err(|e| format!("write z{zoom}/{x}/{y}: {e}"))?;
    }
    writer.finish().map_err(|e| format!("finish {output}: {e}"))?;
    eprintln!("mbtiles-merge: wrote {total} tiles to {output}");
    Ok(())
}
