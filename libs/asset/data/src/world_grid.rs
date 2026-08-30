//! Walkability / terrain-hint grid sidecar (`*.grid`) next to a World GLB.
//!
//! A tiled top-down level's ground is ONE flat textured quad: the picture
//! says nothing about where a vehicle may drive. This sidecar carries that
//! half — one character per map cell, written by the importer, folded by the
//! runtime into the nav map as a static blocked layer.
//!
//! It is deliberately a separate file from [`world_place`](crate::world_place):
//! placements come and go while a level plays (a wall dies, a resource
//! depletes), but the terrain grid is the map's own immutable shape.
//!
//! ```text
//! world-grid 1
//! cell 6.0
//! origin 0.0 0.0
//! size 6 4
//! row 0 ..####
//! row 1 ..#..r
//! row 2 wwbb.r
//! row 3 ...t..
//! ```
//!
//! Cell characters (see [`GridCell`]): `.` clear, `#` blocked, `w` water,
//! `r` road, `b` rough, `t` resource present. Anything else reads as clear,
//! so a newer writer's extra terrain hint never makes an old reader fail.

use std::path::Path;

pub const GRID_VERSION: u32 = 1;
pub const MAGIC: &str = "world-grid";

/// What one map cell is made of. The engine only *needs* to know whether a
/// ground mover may enter; the rest are speed hints and informative flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GridCell {
    /// Open ground.
    #[default]
    Clear,
    /// Rock, cliff, tree, wall, or a structure footprint at import time.
    Blocked,
    /// Impassable to ground movers (boats are a different mover class).
    Water,
    /// Paved: ground movers travel faster.
    Road,
    /// Rough / beach: ground movers travel slower.
    Rough,
    /// A harvestable patch stands here. Passable; the patch itself is a
    /// `.place` row so it can deplete.
    Resource,
}

impl GridCell {
    pub fn as_char(self) -> char {
        match self {
            Self::Clear => '.',
            Self::Blocked => '#',
            Self::Water => 'w',
            Self::Road => 'r',
            Self::Rough => 'b',
            Self::Resource => 't',
        }
    }

    /// Unknown characters read as [`GridCell::Clear`] — an old reader must
    /// survive a newer writer's terrain hint.
    pub fn parse(c: char) -> Self {
        match c {
            '#' => Self::Blocked,
            'w' | 'W' => Self::Water,
            'r' | 'R' => Self::Road,
            'b' | 'B' => Self::Rough,
            't' | 'T' => Self::Resource,
            _ => Self::Clear,
        }
    }

    /// Can a ground mover stand here?
    pub fn passable(self) -> bool {
        !matches!(self, Self::Blocked | Self::Water)
    }

    /// Multiplier on a mover's speed while it crosses this cell.
    pub fn speed_scale(self) -> f32 {
        match self {
            Self::Road => 1.25,
            Self::Rough => 0.8,
            _ => 1.0,
        }
    }
}

/// The parsed sidecar. `cells` is row-major, `size.0` wide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldGrid {
    /// Metres per cell edge.
    pub cell: f32,
    /// World-space (x, z) of the grid's `(0, 0)` corner.
    pub origin: [f32; 2],
    pub width: u32,
    pub height: u32,
    /// `width * height` entries, row-major, row 0 = northmost (`-z`).
    pub cells: Vec<GridCell>,
}

impl WorldGrid {
    /// An all-clear grid of the given shape.
    pub fn new(width: u32, height: u32, cell: f32, origin: [f32; 2]) -> Self {
        Self {
            cell,
            origin,
            width,
            height,
            cells: vec![GridCell::Clear; (width as usize) * (height as usize)],
        }
    }

    fn index(&self, cx: u32, cy: u32) -> Option<usize> {
        if cx >= self.width || cy >= self.height {
            return None;
        }
        Some((cy as usize) * (self.width as usize) + cx as usize)
    }

    pub fn at(&self, cx: u32, cy: u32) -> GridCell {
        self.index(cx, cy)
            .and_then(|i| self.cells.get(i).copied())
            .unwrap_or(GridCell::Clear)
    }

    pub fn set(&mut self, cx: u32, cy: u32, v: GridCell) {
        if let Some(i) = self.index(cx, cy) {
            if let Some(slot) = self.cells.get_mut(i) {
                *slot = v;
            }
        }
    }

    /// World-space centre of cell `(cx, cy)` on the ground plane.
    pub fn cell_centre(&self, cx: u32, cy: u32) -> [f32; 2] {
        [
            self.origin[0] + (cx as f32 + 0.5) * self.cell,
            self.origin[1] + (cy as f32 + 0.5) * self.cell,
        ]
    }

    /// The cell a world-space `(x, z)` point falls in, or `None` outside.
    pub fn cell_of(&self, x: f32, z: f32) -> Option<(u32, u32)> {
        if self.cell <= 0.0 {
            return None;
        }
        let fx = (x - self.origin[0]) / self.cell;
        let fz = (z - self.origin[1]) / self.cell;
        if fx < 0.0 || fz < 0.0 {
            return None;
        }
        let (cx, cy) = (fx as u32, fz as u32);
        if cx >= self.width || cy >= self.height {
            return None;
        }
        Some((cx, cy))
    }

    /// Every impassable cell, in row-major order — what a nav map folds in.
    pub fn blocked_cells(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for cy in 0..self.height {
            for cx in 0..self.width {
                if !self.at(cx, cy).passable() {
                    out.push((cx, cy));
                }
            }
        }
        out
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{MAGIC} {GRID_VERSION}\n");
        out.push_str(&format!("cell {}\n", self.cell));
        out.push_str(&format!("origin {} {}\n", self.origin[0], self.origin[1]));
        out.push_str(&format!("size {} {}\n", self.width, self.height));
        for cy in 0..self.height {
            let row: String = (0..self.width).map(|cx| self.at(cx, cy).as_char()).collect();
            out.push_str(&format!("row {cy} {row}\n"));
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut saw = false;
        let mut cell = 0.0f32;
        let mut origin = [0.0f32; 2];
        let mut width = 0u32;
        let mut height = 0u32;
        let mut rows: Vec<(u32, String)> = Vec::new();
        for raw in text.lines() {
            // Every meaningful line starts with a tag word, so a leading
            // '#' is unambiguously a comment (a row's cell characters only
            // ever appear after the `row <index>` tag).
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut it = trimmed.split_whitespace();
            let Some(tag) = it.next() else { continue };
            match tag {
                MAGIC => {
                    let v: u32 = it
                        .next()
                        .ok_or("world-grid version")?
                        .parse()
                        .map_err(|_| "world-grid version")?;
                    if v != GRID_VERSION {
                        return Err(format!("unsupported world-grid {v}"));
                    }
                    saw = true;
                }
                "cell" => cell = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
                "origin" => {
                    origin[0] = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    origin[1] = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                }
                "size" => {
                    width = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                    height = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                "row" => {
                    let Some(cy) = it.next().and_then(|v| v.parse::<u32>().ok()) else {
                        continue;
                    };
                    // The row body is the rest of the line verbatim (cell
                    // characters never contain whitespace, so one token).
                    rows.push((cy, it.next().unwrap_or("").to_string()));
                }
                _ => {}
            }
        }
        if !saw {
            return Err("not a world-grid file".into());
        }
        if width == 0 || height == 0 {
            // Tolerate a missing `size` by deriving it from the rows.
            height = height.max(rows.iter().map(|(cy, _)| cy + 1).max().unwrap_or(0));
            width = width.max(rows.iter().map(|(_, r)| r.chars().count() as u32).max().unwrap_or(0));
        }
        let mut grid = Self::new(width, height, cell, origin);
        for (cy, body) in rows {
            for (cx, c) in body.chars().enumerate() {
                grid.set(cx as u32, cy, GridCell::parse(c));
            }
        }
        Ok(grid)
    }
}

/// Write `<glb>.grid` beside a world GLB.
pub fn write_grid_sidecar(glb: &Path, grid: &WorldGrid) -> Result<(), String> {
    let mut name = glb.file_name().unwrap_or_default().to_string_lossy().to_string();
    name.push_str(".grid");
    std::fs::write(glb.with_file_name(name), grid.to_text()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_round_trips() {
        let mut g = WorldGrid::new(6, 4, 6.0, [-12.0, -6.0]);
        g.set(2, 0, GridCell::Blocked);
        g.set(3, 0, GridCell::Blocked);
        g.set(0, 2, GridCell::Water);
        g.set(5, 1, GridCell::Road);
        g.set(2, 2, GridCell::Rough);
        g.set(3, 3, GridCell::Resource);
        let text = g.to_text();
        assert!(text.starts_with("world-grid 1\n"), "{text}");
        assert!(text.contains("size 6 4\n"), "{text}");
        assert!(text.contains("row 0 ..##..\n"), "{text}");
        let back = WorldGrid::parse(&text).expect("parse");
        assert_eq!(back, g);
    }

    #[test]
    fn cell_geometry_matches_the_contract() {
        let g = WorldGrid::new(6, 6, 6.0, [0.0, 0.0]);
        assert_eq!(g.cell_centre(0, 0), [3.0, 3.0]);
        assert_eq!(g.cell_centre(5, 5), [33.0, 33.0]);
        assert_eq!(g.cell_of(3.0, 3.0), Some((0, 0)));
        assert_eq!(g.cell_of(35.9, 0.1), Some((5, 0)));
        assert_eq!(g.cell_of(-0.1, 0.0), None);
        assert_eq!(g.cell_of(36.0, 0.0), None);
    }

    #[test]
    fn blocked_cells_cover_walls_and_water_only() {
        let mut g = WorldGrid::new(3, 2, 6.0, [0.0, 0.0]);
        g.set(1, 0, GridCell::Blocked);
        g.set(2, 1, GridCell::Water);
        g.set(0, 1, GridCell::Road);
        g.set(0, 0, GridCell::Resource);
        assert_eq!(g.blocked_cells(), vec![(1, 0), (2, 1)]);
        assert!(g.at(0, 0).passable());
        assert_eq!(g.at(0, 1).speed_scale(), 1.25);
    }

    #[test]
    fn unknown_characters_and_short_rows_read_as_clear() {
        let text = "world-grid 1\ncell 6\norigin 0 0\nsize 4 2\nrow 0 .#\nrow 1 ..Z?\n";
        let g = WorldGrid::parse(text).expect("parse");
        assert_eq!(g.width, 4);
        assert_eq!(g.at(1, 0), GridCell::Blocked);
        assert_eq!(g.at(3, 0), GridCell::Clear);
        assert_eq!(g.at(2, 1), GridCell::Clear);
    }

    #[test]
    fn a_non_grid_file_is_an_error() {
        assert!(WorldGrid::parse("world-place 1\n").is_err());
    }
}
