//! Tiled-RTS map generation, as VALUES.
//!
//! One generator, two producers. The asset importer turns the values into
//! `worlds/<map>.glb` + `.grid` + `.place` + `.spawn` files; the sandbox
//! turns the same values into a world it streams from memory. Neither owns
//! the algorithm, so a map an author generates at runtime and a map the
//! importer bakes are the same map.
//!
//! Nothing here knows any game. A style is a shape of terrain (`Temperate`,
//! `Desert`, `Arena`); resources are resources; a house is a colour and an
//! index. Which sprite a resource cell draws with, and what a house is
//! called, are the PRODUCER's business — they come off the art pack.
//!
//! The output vocabulary is contract section 1's `world-grid` letters:
//! `.` clear, `#` blocked, `w` water, `r` road, `b` rough/beach,
//! `t` resource present (passable).

pub mod emit;
pub mod gen;
pub mod math;
pub mod preview;
pub mod rng;
pub mod tiles;
pub mod verify;

pub use emit::{grid_text, place_rows, spawn_text, EmitOpts, House, PlaceRow, PropArt};
pub use tiles::{pick_tiles, TileSet, MASK_ALL, MASK_E, MASK_N, MASK_S, MASK_W};
pub use verify::MapReport;

/// Generation revision. Bump when the output CHANGES for the same seed and
/// spec: a world staged from an older revision is then honestly attributable
/// to the algorithm that made it.
pub const RTSMAP_REV: u32 = 1;

/// What a cell IS, before any art is chosen. Producers map these onto their
/// own tile vocabulary (a template class letter, a learned tile bank).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Terrain {
    /// Open buildable ground.
    Clear,
    /// Broken ground: passable, slower, not the pretty tile.
    Rough,
    /// The band where land meets water.
    Shore,
    /// A laid road: passable and faster.
    Road,
    /// Impassable to ground units.
    Water,
    /// The rim of raised ground — impassable, and the edge tiles must match
    /// their neighbours or the plateau looks like confetti.
    Cliff,
    /// The top of raised ground: passable, one or more levels up.
    Plateau,
    /// A harvestable cell. Passable; the amount lives in `ResourceCell`.
    Resource,
}

impl Terrain {
    /// The `world-grid 1` letter for this cell (contract section 1).
    pub fn grid_letter(self) -> u8 {
        match self {
            Terrain::Clear | Terrain::Plateau => b'.',
            Terrain::Rough | Terrain::Shore => b'b',
            Terrain::Road => b'r',
            Terrain::Water => b'w',
            Terrain::Cliff => b'#',
            Terrain::Resource => b't',
        }
    }

    /// Can a ground unit stand here, ignoring props?
    pub fn passable(self) -> bool {
        !matches!(self, Terrain::Water | Terrain::Cliff)
    }

    /// Can a structure be put down here? Roads and rough count: the engine
    /// only refuses blocked cells. Resource cells do not — building over the
    /// field a house lives off is a trap, not a choice.
    pub fn buildable(self) -> bool {
        matches!(self, Terrain::Clear | Terrain::Plateau | Terrain::Road)
    }

    /// Terrains whose art must edge-match its neighbours. Everything else can
    /// take any variant of its own class.
    pub fn directional(self) -> bool {
        matches!(self, Terrain::Cliff | Terrain::Plateau | Terrain::Water | Terrain::Resource | Terrain::Road)
    }
}

/// A static thing standing on a cell. Trees and rocks block; a bloom is
/// dressing on a resource field and does not.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PropKind {
    Tree,
    Rock,
    Ruin,
    Bloom,
}

impl PropKind {
    pub fn blocks(self) -> bool {
        !matches!(self, PropKind::Bloom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Prop {
    pub x: u16,
    pub y: u16,
    pub kind: PropKind,
    /// Which of the pack's variants of this prop to draw (`t01`..`t17`,
    /// a rock bank, a ruin set). Producers modulo it by what they have.
    pub variant: u8,
}

/// One harvestable cell. `stage` is contract section 1's `stage=0..11`
/// richness, richest at a field's heart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceCell {
    pub x: u16,
    pub y: u16,
    pub stage: u8,
    /// Which field it belongs to — fairness counts per field, and a producer
    /// may want to know that two cells are the same deposit.
    pub field: u8,
}

/// Where a house begins, and what the generator measured about that spot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Start {
    pub x: u16,
    pub y: u16,
    /// Buildable cells inside the start pocket.
    pub pocket: u16,
    /// Resource cells reachable inside `RESOURCE_REACH`.
    pub resource_cells: u16,
    /// Path distance in cells to the nearest resource cell.
    pub resource_distance: u16,
}

/// A playable slot. The generator only knows how many there are and gives
/// each a distinguishable colour; the producer names it off the art pack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HouseSlot {
    pub index: u8,
    pub color: [u8; 3],
}

/// The shape of a map. Not a game: a climate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Style {
    /// Green belt: rivers with shores, wooded patches, plateaus, roads.
    Temperate,
    /// Dry: dune fields and rock shelves, no water, wide open sight lines.
    Desert,
    /// Compact and rotationally symmetric — a tournament map: short roads
    /// between starts, little terrain to argue about.
    Arena,
}

impl Style {
    pub fn parse(name: &str) -> Option<Style> {
        match name.trim().to_ascii_lowercase().as_str() {
            "temperate" | "green" | "grass" => Some(Style::Temperate),
            "desert" | "dune" | "sand" | "arid" => Some(Style::Desert),
            "arena" | "tournament" => Some(Style::Arena),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::Temperate => "temperate",
            Style::Desert => "desert",
            Style::Arena => "arena",
        }
    }

    pub const ALL: [Style; 3] = [Style::Temperate, Style::Desert, Style::Arena];
}

/// `none`/`low`/`normal`/`heavy` as a 0..1 amount — what an author types and
/// what the generator scales by. Unknown words fall back to `normal`, and a
/// bare number passes through clamped.
pub fn amount(word: &str) -> f32 {
    match word.trim().to_ascii_lowercase().as_str() {
        "none" | "off" | "0" => 0.0,
        "low" | "sparse" | "light" | "few" => 0.35,
        "normal" | "some" | "medium" | "default" => 0.6,
        "heavy" | "high" | "lots" | "rich" | "many" => 1.0,
        other => other.parse::<f32>().map(|v| v.clamp(0.0, 1.0)).unwrap_or(0.6),
    }
}

/// What to generate.
#[derive(Clone, Debug, PartialEq)]
pub struct MapSpec {
    pub seed: u32,
    pub width: u16,
    pub height: u16,
    pub players: u8,
    pub style: Style,
    /// 0..1 amounts. `resources` scales field size AND count; the others
    /// scale how much of that feature the map gets.
    pub resources: f32,
    pub cliffs: f32,
    pub water: f32,
    pub roads: f32,
    /// An opaque art hint carried through to the producer (`temperate`,
    /// `snow`, `arrakis`…). The generator never reads it.
    pub theater: String,
    /// How many times a failed fairness check may reseed. 0 = take the first.
    pub retries: u8,
}

impl Default for MapSpec {
    fn default() -> Self {
        Self {
            seed: 1,
            width: 64,
            height: 64,
            players: 2,
            style: Style::Temperate,
            resources: 0.6,
            cliffs: 0.6,
            water: 0.6,
            roads: 0.6,
            theater: String::new(),
            retries: 8,
        }
    }
}

/// Bounds. A map smaller than this cannot hold fair pockets; one larger
/// costs more to bake than a round is worth.
pub const MIN_SIZE: u16 = 32;
pub const MAX_SIZE: u16 = 192;
pub const MAX_PLAYERS: u8 = 8;
/// How far a house may be from its field before the map is unfair.
pub const RESOURCE_REACH: u16 = 26;
/// The radius the generator guarantees clear around a start.
pub const POCKET_RADIUS: i32 = 4;

impl MapSpec {
    /// The spec as the generator will actually use it — clamped, with the
    /// player count rounded to something a symmetric layout can serve.
    pub fn resolved(&self) -> MapSpec {
        let mut out = self.clone();
        out.width = self.width.clamp(MIN_SIZE, MAX_SIZE);
        out.height = self.height.clamp(MIN_SIZE, MAX_SIZE);
        out.players = self.players.clamp(2, MAX_PLAYERS);
        out.resources = self.resources.clamp(0.0, 1.0);
        out.cliffs = self.cliffs.clamp(0.0, 1.0);
        out.water = self.water.clamp(0.0, 1.0);
        out.roads = self.roads.clamp(0.0, 1.0);
        out.retries = self.retries.min(32);
        if out.style == Style::Desert {
            // A desert has no rivers by definition; asking for them is an
            // author error, not a lake in the Sahara.
            out.water = 0.0;
        }
        out
    }
}

/// A generated map, as values.
#[derive(Clone, Debug, PartialEq)]
pub struct RtsMap {
    /// The spec after clamping — what actually produced this map.
    pub spec: MapSpec,
    /// The seed the winning attempt used (differs from `spec.seed` when a
    /// fairness retry fired).
    pub seed: u32,
    pub width: u16,
    pub height: u16,
    /// Row-major, `y * width + x`.
    pub terrain: Vec<Terrain>,
    /// Height level per cell (0 = ground). Producers that draw elevation
    /// (isometric packs) read it; flat packs ignore it.
    pub heights: Vec<u8>,
    /// The `world-grid` letter per cell, props already folded in.
    pub grid: Vec<u8>,
    pub resources: Vec<ResourceCell>,
    pub props: Vec<Prop>,
    pub starts: Vec<Start>,
    pub houses: Vec<HouseSlot>,
    pub report: MapReport,
}

impl RtsMap {
    #[inline]
    pub fn at(&self, x: u16, y: u16) -> usize {
        y as usize * self.width as usize + x as usize
    }

    #[inline]
    pub fn terrain_at(&self, x: i32, y: i32) -> Option<Terrain> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        self.terrain.get(y as usize * self.width as usize + x as usize).copied()
    }

    /// The `.grid` sidecar's rows.
    pub fn grid_rows(&self) -> Vec<String> {
        self.grid
            .chunks_exact(self.width as usize)
            .map(|row| String::from_utf8_lossy(row).into_owned())
            .collect()
    }

    /// Drop the props a producer has no art for, and take their blocked
    /// cells back with them.
    ///
    /// A prop that is not drawn but still marks its cell `#` is an INVISIBLE
    /// WALL — the worst bug a generated map can ship — so removing scenery
    /// and re-deriving the grid are one operation, never two.
    pub fn retain_props(&mut self, keep: impl Fn(&Prop) -> bool) {
        self.props.retain(&keep);
        self.rebuild_grid();
    }

    /// Re-derive `grid` from `terrain` plus the props that block.
    pub fn rebuild_grid(&mut self) {
        self.grid = self.terrain.iter().map(|t| t.grid_letter()).collect();
        for prop in &self.props {
            if !prop.kind.blocks() {
                continue;
            }
            let at = prop.y as usize * self.width as usize + prop.x as usize;
            if at < self.grid.len() {
                self.grid[at] = b'#';
            }
        }
    }

    /// Resource stage per cell, `None` where there is none — the shape the
    /// classic converters already carried.
    pub fn stage_grid(&self) -> Vec<Option<u8>> {
        let mut out = vec![None; self.terrain.len()];
        for cell in &self.resources {
            let at = self.at(cell.x, cell.y);
            if at < out.len() {
                out[at] = Some(cell.stage);
            }
        }
        out
    }
}

/// Generate, retrying a failed fairness check with a derived seed up to
/// `spec.retries` times, and returning the best attempt either way. Read
/// `map.report.ok` to find out whether the map that came back is fair.
pub fn generate(spec: &MapSpec) -> RtsMap {
    let spec = spec.resolved();
    let mut best: Option<RtsMap> = None;
    for attempt in 0..=u32::from(spec.retries) {
        let seed = if attempt == 0 {
            spec.seed
        } else {
            rng::hash2(spec.seed, attempt as i32, 0x5eed)
        };
        let mut map = gen::build(&spec, seed);
        let mut report = verify::verify(&mut map);
        report.attempts = attempt + 1;
        map.report = report;
        if map.report.ok {
            return map;
        }
        let better = best
            .as_ref()
            .map(|prev| map.report.score() > prev.report.score())
            .unwrap_or(true);
        if better {
            best = Some(map);
        }
    }
    // Every attempt failed: hand back the least-bad one with its report
    // saying so, rather than a hard error the caller has no better answer to.
    best.expect("at least one attempt")
}

/// `generate`, but a fairness failure is an error rather than a report.
pub fn generate_checked(spec: &MapSpec) -> Result<RtsMap, MapReport> {
    let map = generate(spec);
    if map.report.ok { Ok(map) } else { Err(map.report) }
}
