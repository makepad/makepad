//! The generated map as the contract's TEXT sidecars.
//!
//! `RtsMap` is values; this is the one place that turns them into
//! `world-grid 1`, `world-place 1` and `world-spawn 1` documents, so the
//! importer's files and the sandbox's in-memory world are the same bytes for
//! the same map. Art keys come in from the producer — the generator has
//! never heard of a sprite.

use crate::{PropKind, RtsMap};

/// A playable house, as the `.place` header declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct House {
    pub name: String,
    /// sRGB hex without the `#`, as `house … color=` wants it.
    pub color: String,
    pub side: String,
}

/// Which billboard key draws each kind of scenery, in variant order. The
/// generator picked a variant number; the producer says what a variant IS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropArt {
    pub trees: Vec<String>,
    pub rocks: Vec<String>,
    pub ruins: Vec<String>,
    pub blooms: Vec<String>,
}

impl PropArt {
    fn keys(&self, kind: PropKind) -> &[String] {
        match kind {
            PropKind::Tree => &self.trees,
            PropKind::Rock => &self.rocks,
            PropKind::Ruin => &self.ruins,
            PropKind::Bloom => &self.blooms,
        }
    }
}

/// Everything the emitter needs that is not the map.
#[derive(Clone, Debug, PartialEq)]
pub struct EmitOpts {
    /// The `source <name>` fact — which importer or verb made this.
    pub source: String,
    /// The world's own key, e.g. `worlds/gen-desert-4p-91`.
    pub world_key: String,
    pub cell_m: f32,
    pub tile_px: u32,
    pub metres_per_pixel: f32,
    pub houses: Vec<House>,
    /// The resource patch's billboard key.
    pub resource_key: String,
    pub props: PropArt,
    /// The keys the level offers for production (`roster` lines).
    pub roster: Vec<String>,
    /// Camera height hint written into `.spawn`.
    pub eye: f32,
}

impl Default for EmitOpts {
    fn default() -> Self {
        Self {
            source: "rtsmap".into(),
            world_key: "worlds/generated".into(),
            cell_m: 6.0,
            tile_px: 24,
            metres_per_pixel: 0.25,
            houses: Vec::new(),
            resource_key: String::new(),
            props: PropArt::default(),
            roster: Vec::new(),
            eye: 60.0,
        }
    }
}

/// One `place` row, before it is a line of text.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceRow {
    pub id: String,
    /// `unit` | `structure` | `scenery` | `resource`.
    pub kind: &'static str,
    pub key: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub extras: Vec<(String, String)>,
}

impl PlaceRow {
    pub fn to_text(&self) -> String {
        let mut line = format!(
            "place {} {} {} {:.4} {:.4} {:.4} {:.5}",
            self.id, self.kind, self.key, self.x, self.y, self.z, self.yaw
        );
        for (key, value) in &self.extras {
            line.push(' ');
            line.push_str(key);
            line.push('=');
            line.push_str(value);
        }
        line
    }
}

/// World-space centre of a cell, per contract section 0.
pub fn cell_centre(x: u16, y: u16, cell_m: f32) -> (f32, f32) {
    ((x as f32 + 0.5) * cell_m, (y as f32 + 0.5) * cell_m)
}

/// Resource cells first, then scenery — the order a `.place` reads best in
/// and the order the layer heights already imply.
pub fn place_rows(map: &RtsMap, opts: &EmitOpts) -> Vec<PlaceRow> {
    let mut rows = Vec::new();
    if !opts.resource_key.is_empty() {
        for (index, cell) in map.resources.iter().enumerate() {
            let (x, z) = cell_centre(cell.x, cell.y, opts.cell_m);
            rows.push(PlaceRow {
                id: format!("r-{index}"),
                kind: "resource",
                key: opts.resource_key.clone(),
                x,
                y: 0.04,
                z,
                yaw: 0.0,
                extras: vec![
                    ("align".into(), "floor".into()),
                    ("layer".into(), "0.04".into()),
                    ("class".into(), "resource".into()),
                    ("stage".into(), cell.stage.to_string()),
                ],
            });
        }
    }
    for (index, prop) in map.props.iter().enumerate() {
        let keys = opts.props.keys(prop.kind);
        if keys.is_empty() {
            continue;
        }
        let key = keys[(prop.variant as usize).saturating_sub(1) % keys.len()].clone();
        let (x, z) = cell_centre(prop.x, prop.y, opts.cell_m);
        rows.push(PlaceRow {
            id: format!("t-{index}"),
            kind: "scenery",
            key,
            x,
            y: 0.06,
            z,
            yaw: 0.0,
            extras: vec![
                ("align".into(), "floor".into()),
                ("layer".into(), "0.06".into()),
                ("class".into(), if prop.kind == PropKind::Tree { "tree".into() } else { "scenery".to_string() }),
            ],
        });
    }
    rows
}

/// The whole `world-place 1` document.
pub fn place_text(map: &RtsMap, opts: &EmitOpts) -> String {
    let mut out = format!(
        "world-place 1\nsource {}\nworld {}\nmode rts\ncell {:.1}\ntile {}\nmetres_per_pixel {}\ngrid {}.grid\n",
        opts.source,
        opts.world_key,
        opts.cell_m,
        opts.tile_px,
        trim_float(opts.metres_per_pixel),
        opts.world_key,
    );
    for house in &opts.houses {
        out.push_str(&format!("house {} color={} side={}\n", house.name, house.color, house.side));
    }
    for keys in opts.roster.chunks(24) {
        out.push_str(&format!("roster {}\n", keys.join(" ")));
    }
    for row in place_rows(map, opts) {
        out.push_str(&row.to_text());
        out.push('\n');
    }
    out
}

/// The `world-grid 1` sidecar.
pub fn grid_text(map: &RtsMap, cell_m: f32) -> String {
    let mut out = format!(
        "world-grid 1\ncell {cell_m:.1}\norigin 0.0 0.0\nsize {} {}\n",
        map.width, map.height
    );
    for (y, row) in map.grid_rows().iter().enumerate() {
        out.push_str(&format!("row {y} {row}\n"));
    }
    out
}

/// The `world-spawn 1` sidecar: one `start_<n>` per house.
pub fn spawn_text(map: &RtsMap, cell_m: f32, eye: f32) -> String {
    let mut out = String::from("world-spawn 1\n");
    for (index, start) in map.starts.iter().enumerate() {
        let (x, z) = cell_centre(start.x, start.y, cell_m);
        out.push_str(&format!("start start_{index} {x:.4} 0.0000 {z:.4} 0.00000 -1.45000\n"));
    }
    out.push_str(&format!("floor 0\nstep 0.5\neye {eye:.0}\n"));
    out
}

fn trim_float(value: f32) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    if text.is_empty() { "0".into() } else { text }
}
