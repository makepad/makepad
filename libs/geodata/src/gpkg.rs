//! GeoPackage feature reading on top of the generic SQLite table access in
//! `makepad-mbtile-reader` (a GeoPackage is just a SQLite file).

use crate::geo;
use crate::wkb::{parse_gpkg_geometry, Geometry};
use makepad_mbtile_reader::{MbtilesReader, Value};
use std::path::Path;

pub struct FeatureTableInfo {
    pub table: String,
    /// Column names in declaration order (matches record value order).
    pub columns: Vec<String>,
    /// Index of the geometry column within `columns`.
    pub geom_col: usize,
    pub srs_id: i64,
}

pub struct Gpkg {
    db: MbtilesReader,
}

impl Gpkg {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = MbtilesReader::open_sqlite(path)
            .map_err(|e| format!("open {}: {e:?}", path.display()))?;
        Ok(Gpkg { db })
    }

    /// Enumerate feature tables via gpkg_contents + gpkg_geometry_columns.
    pub fn feature_tables(&mut self) -> Result<Vec<FeatureTableInfo>, String> {
        let schema = self
            .db
            .schema_entries()
            .map_err(|e| format!("schema: {e:?}"))?;
        let sql_for = |table: &str| -> Option<&str> {
            schema
                .iter()
                .find(|e| e.obj_type == "table" && e.name == table)
                .map(|e| e.sql.as_str())
        };

        let contents_cols = columns_from_sql(
            sql_for("gpkg_contents").ok_or("no gpkg_contents table")?,
        );
        let c_table = col_index(&contents_cols, "table_name")?;
        let c_type = col_index(&contents_cols, "data_type")?;
        let c_srs = col_index(&contents_cols, "srs_id")?;

        let mut feature_tables: Vec<(String, i64)> = Vec::new();
        self.db
            .for_each_row("gpkg_contents", |_rowid, values| {
                let data_type = values.get(c_type).and_then(|v| v.as_text()).unwrap_or("");
                let table = values.get(c_table).and_then(|v| v.as_text()).unwrap_or("");
                let srs = values.get(c_srs).and_then(|v| v.as_integer()).unwrap_or(0);
                if data_type == "features" && !table.is_empty() {
                    feature_tables.push((table.to_string(), srs));
                }
            })
            .map_err(|e| format!("gpkg_contents: {e:?}"))?;

        let geom_cols = columns_from_sql(
            sql_for("gpkg_geometry_columns").ok_or("no gpkg_geometry_columns table")?,
        );
        let g_table = col_index(&geom_cols, "table_name")?;
        let g_col = col_index(&geom_cols, "column_name")?;
        let mut geom_col_names: Vec<(String, String)> = Vec::new();
        self.db
            .for_each_row("gpkg_geometry_columns", |_rowid, values| {
                let table = values.get(g_table).and_then(|v| v.as_text()).unwrap_or("");
                let col = values.get(g_col).and_then(|v| v.as_text()).unwrap_or("");
                if !table.is_empty() {
                    geom_col_names.push((table.to_string(), col.to_string()));
                }
            })
            .map_err(|e| format!("gpkg_geometry_columns: {e:?}"))?;

        let mut infos = Vec::new();
        for (table, srs_id) in feature_tables {
            let Some(sql) = sql_for(&table) else { continue };
            let columns = columns_from_sql(sql);
            let geom_name = geom_col_names
                .iter()
                .find(|(t, _)| *t == table)
                .map(|(_, c)| c.clone())
                .unwrap_or_else(|| "geom".to_string());
            let Some(geom_col) = columns.iter().position(|c| *c == geom_name) else {
                continue;
            };
            infos.push(FeatureTableInfo {
                table,
                columns,
                geom_col,
                srs_id,
            });
        }
        Ok(infos)
    }

    /// Iterate all features of a table. The callback gets the rowid, all
    /// column values (geometry column included, as a blob), and the parsed
    /// geometry already transformed to WGS84 lon/lat.
    pub fn for_each_feature(
        &mut self,
        info: &FeatureTableInfo,
        mut callback: impl FnMut(i64, &[Value], Geometry),
    ) -> Result<u64, String> {
        let srs = info.srs_id;
        let geom_col = info.geom_col;
        let mut skipped = 0u64;
        self.db
            .for_each_row(&info.table, |rowid, values| {
                let Some(blob) = values.get(geom_col).and_then(|v| v.as_blob()) else {
                    skipped += 1;
                    return;
                };
                let Some(geom) = parse_gpkg_geometry(blob) else {
                    skipped += 1;
                    return;
                };
                let Some(geom) = to_wgs84(&geom, srs) else {
                    skipped += 1;
                    return;
                };
                callback(rowid, &values, geom);
            })
            .map_err(|e| format!("scan {}: {e:?}", info.table))?;
        Ok(skipped)
    }
}

/// Transform a geometry from the given EPSG srs to WGS84 lon/lat.
pub fn to_wgs84(geom: &Geometry, srs_id: i64) -> Option<Geometry> {
    match srs_id {
        4326 => Some(geom.clone()),
        28992 => Some(geom.map_coords(&|x, y| geo::rd_to_wgs84(x, y))),
        _ => None,
    }
}

/// Crude but sufficient column-name extraction from a CREATE TABLE statement
/// (GeoPackage SQL is machine-generated and regular).
pub fn columns_from_sql(sql: &str) -> Vec<String> {
    let Some(open) = sql.find('(') else {
        return Vec::new();
    };
    let Some(close) = sql.rfind(')') else {
        return Vec::new();
    };
    let body = &sql[open + 1..close];
    let mut columns = Vec::new();
    let mut depth = 0i32;
    let mut part = String::new();
    let mut parts = Vec::new();
    for ch in body.chars() {
        match ch {
            '(' => {
                depth += 1;
                part.push(ch);
            }
            ')' => {
                depth -= 1;
                part.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut part));
            }
            _ => part.push(ch),
        }
    }
    parts.push(part);

    const CONSTRAINTS: &[&str] = &[
        "PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT",
    ];
    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let first = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']' || c == '\'');
        if first.is_empty()
            || CONSTRAINTS
                .iter()
                .any(|k| first.eq_ignore_ascii_case(k))
        {
            continue;
        }
        columns.push(first.to_string());
    }
    columns
}

fn col_index(columns: &[String], name: &str) -> Result<usize, String> {
    columns
        .iter()
        .position(|c| c.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("column {name} not found in {columns:?}"))
}
