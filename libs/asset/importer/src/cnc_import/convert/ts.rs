//! Tiberian Sun archive interpretation and deterministic isometric worlds.

use super::{
    positive_unit_cost, role_for, roster_key, PreviewCrop, PreviewDot, RoleTable, RtsEmitter,
    SpritePixels, SpriteSpec, SpriteState, UnitSpec, WorldSpec,
};
use crate::classic_import::{ClassicAsset, ConvertStage, ConvertTick};
use crate::cnc_import::{
    aud::Aud,
    ini::Ini,
    mix::{HashKind, MixFile, NameTable},
    pal::Pal,
    rules::{Rules, UnitRules},
    shp_ts::ShpTs,
    tmp_ts::{ExtraImage, IsoTile, TmpTs},
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const ISO_SIZE: usize = 48;
const SCREEN_W_CELLS: usize = ISO_SIZE * 2;
const SCREEN_H_CELLS: usize = ISO_SIZE;
const CONTRACT_TILE_PX: u32 = 24;
const ISO_TILE_W: i32 = 48;
const ISO_TILE_H: i32 = 24;
const HEIGHT_STEP_PX: i32 = 12;
const METRES_PER_PIXEL: f32 = 0.125;
const CELL_M: f32 = CONTRACT_TILE_PX as f32 * METRES_PER_PIXEL;
const HASH: HashKind = HashKind::Crc32;
const MASK_N: u8 = 1;
const MASK_E: u8 = 2;
const MASK_S: u8 = 4;
const MASK_W: u8 = 8;

const INFANTRY: &[(&str, &str)] = &[
    ("e1", "e1"),
    ("e2", "e2"),
    ("e3", "e3"),
    ("medic", "medic"),
    ("engineer", "engineer"),
    ("ghost", "ghost"),
    ("jumpjet", "jumpjet"),
    ("cyborg", "cyborg"),
    ("cyc2", "cyc2"),
    ("umagon", "umagon"),
    ("mhijack", "mhijack"),
    ("chamspy", "chamspy"),
    ("civ1", "civ1"),
    ("civ2", "civ2"),
    ("civ3", "civ3"),
];

const VEHICLES: &[(&str, &str, &str)] = &[
    ("titan", "mmch", "MMCH"),
    ("wolverine", "smech", "SMECH"),
];

#[derive(Clone, Copy)]
struct StructureDef {
    key: &'static str,
    source: &'static str,
    rules: &'static str,
    footprint: (u8, u8),
    class: &'static str,
    role: &'static str,
}

const STRUCTURES: &[StructureDef] = &[
    StructureDef { key: "gacnst", source: "gacnst", rules: "GACNST", footprint: (3, 3), class: "structure", role: "conyard" },
    StructureDef { key: "nacnst", source: "gacnst", rules: "NACNST", footprint: (3, 3), class: "structure", role: "conyard" },
    StructureDef { key: "gapowr", source: "gapowr", rules: "GAPOWR", footprint: (2, 2), class: "structure", role: "power" },
    StructureDef { key: "napowr", source: "napowr", rules: "NAPOWR", footprint: (2, 2), class: "structure", role: "power" },
    StructureDef { key: "garefn", source: "narefn", rules: "GAREFN", footprint: (4, 3), class: "structure", role: "refinery" },
    StructureDef { key: "narefn", source: "narefn", rules: "NAREFN", footprint: (4, 3), class: "structure", role: "refinery" },
    StructureDef { key: "gasilo", source: "gasilo", rules: "GASILO", footprint: (1, 1), class: "structure", role: "silo" },
    StructureDef { key: "nasilo", source: "gasilo", rules: "NASILO", footprint: (1, 1), class: "structure", role: "silo" },
    StructureDef { key: "gapile", source: "gapile", rules: "GAPILE", footprint: (2, 2), class: "structure", role: "barracks" },
    StructureDef { key: "nahand", source: "nahand", rules: "NAHAND", footprint: (2, 3), class: "structure", role: "barracks" },
    StructureDef { key: "gaweap", source: "gaweap", rules: "GAWEAP", footprint: (3, 3), class: "structure", role: "vehicle_factory" },
    StructureDef { key: "naweap", source: "naweap", rules: "NAWEAP", footprint: (3, 3), class: "structure", role: "vehicle_factory" },
    StructureDef { key: "garadr", source: "garadr", rules: "GARADR", footprint: (2, 2), class: "structure", role: "radar" },
    StructureDef { key: "naradr", source: "naradr", rules: "NARADR", footprint: (2, 2), class: "structure", role: "radar" },
    StructureDef { key: "gatech", source: "gatech", rules: "GATECH", footprint: (2, 2), class: "structure", role: "tech" },
    StructureDef { key: "natech", source: "natech", rules: "NATECH", footprint: (2, 2), class: "structure", role: "tech" },
    StructureDef { key: "gahpad", source: "gahpad", rules: "GAHPAD", footprint: (2, 2), class: "structure", role: "aircraft_pad" },
    StructureDef { key: "nahpad", source: "nahpad", rules: "NAHPAD", footprint: (2, 2), class: "structure", role: "aircraft_pad" },
    StructureDef { key: "gadept", source: "gadept", rules: "GADEPT", footprint: (3, 2), class: "structure", role: "repair" },
    StructureDef { key: "gactwr", source: "gactwr", rules: "GACTWR", footprint: (1, 1), class: "defense", role: "defense" },
    StructureDef { key: "naobel", source: "naobel", rules: "NAOBEL", footprint: (1, 1), class: "defense", role: "defense" },
    StructureDef { key: "nalasr", source: "nalasr", rules: "NALASR", footprint: (1, 1), class: "defense", role: "defense" },
    StructureDef { key: "nasam", source: "nasam", rules: "NASAM", footprint: (1, 1), class: "defense", role: "defense" },
    StructureDef { key: "natmpl", source: "natmpl", rules: "NATMPL", footprint: (3, 3), class: "structure", role: "tech" },
    StructureDef { key: "gaplug", source: "gaplug", rules: "GAPLUG", footprint: (2, 2), class: "structure", role: "tech" },
    StructureDef { key: "napuls", source: "napuls", rules: "NAPULS", footprint: (2, 2), class: "defense", role: "defense" },
    StructureDef { key: "namisl", source: "namisl", rules: "NAMISL", footprint: (3, 3), class: "structure", role: "superweapon" },
    StructureDef { key: "nastlh", source: "nastlh", rules: "NASTLH", footprint: (2, 2), class: "structure", role: "tech" },
    StructureDef { key: "gawall", source: "gawall", rules: "GAWALL", footprint: (1, 1), class: "structure", role: "wall" },
    StructureDef { key: "nawall", source: "nawall", rules: "NAWALL", footprint: (1, 1), class: "structure", role: "wall" },
];

pub(super) const ROLE_TABLE: RoleTable = &[
    ("gacnst", "conyard"),
    ("nacnst", "conyard"),
    ("gapowr", "power"),
    ("napowr", "power"),
    ("garefn", "refinery"),
    ("narefn", "refinery"),
    ("gasilo", "silo"),
    ("nasilo", "silo"),
    ("gapile", "barracks"),
    ("nahand", "barracks"),
    ("gaweap", "vehicle_factory"),
    ("naweap", "vehicle_factory"),
    ("gahpad", "aircraft_pad"),
    ("nahpad", "aircraft_pad"),
    ("garadr", "radar"),
    ("naradr", "radar"),
    ("gatech", "tech"),
    ("natech", "tech"),
    ("natmpl", "tech"),
    ("gadept", "repair"),
    ("gactwr", "defense"),
    ("naobel", "defense"),
    ("nalasr", "defense"),
    ("nasam", "defense"),
    ("gawall", "wall"),
    ("nawall", "wall"),
    ("gaplug", "tech"),
    ("napuls", "defense"),
    ("namisl", "superweapon"),
    ("nastlh", "tech"),
];

const EFFECTS: &[&str] = &[
    "120mm", "dragon", "piff", "piffpiff", "piffs", "explolrg", "explomed",
    "explosml", "fire1", "fire2", "fire3", "fire4", "smokey", "smokey2",
    "pulse", "pulsefx2", "pulse_explosion", "pulse_explosion_small", "veins",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerrainKind {
    Clear,
    Cliff,
    Road,
    Water,
    Rough,
    Ruin,
}

#[derive(Clone)]
struct TerrainTile {
    name: String,
    pixels: Vec<u8>,
    terrain_type: u8,
    raw_height: u8,
    neighbour_mask: u8,
    extra: Option<ExtraImage>,
}

#[derive(Clone)]
struct TheaterBank {
    name: &'static str,
    palette: Pal,
    clear: Vec<TerrainTile>,
    cliff: Vec<TerrainTile>,
    road: Vec<TerrainTile>,
    water: Vec<TerrainTile>,
    rough: Vec<TerrainTile>,
    ruin: Vec<TerrainTile>,
    overlays: BTreeMap<String, ShpTs>,
    terrain_histograms: BTreeMap<String, BTreeMap<u8, usize>>,
}

impl TheaterBank {
    fn load(
        name: &'static str,
        archive: &[u8],
        overlay_archive: &[u8],
        palette: Pal,
        names: &NameTable,
    ) -> Result<Self, String> {
        let mix = MixFile::parse(archive).map_err(|error| format!("{name}: {error}"))?;
        let extension = if name == "snow" { ".SNO" } else { ".TEM" };
        let mut bank = Self {
            name,
            palette,
            clear: Vec::new(),
            cliff: Vec::new(),
            road: Vec::new(),
            water: Vec::new(),
            rough: Vec::new(),
            ruin: Vec::new(),
            overlays: BTreeMap::new(),
            terrain_histograms: BTreeMap::new(),
        };
        for (id, resolved) in names.resolve_names(&mix) {
            let Some(file) = resolved.filter(|file| file.to_ascii_uppercase().ends_with(extension)) else {
                continue;
            };
            let Some(bytes) = mix.by_id(id) else { continue };
            let stem = file[..file.len().saturating_sub(4)].to_ascii_lowercase();
            if let Ok(template) = TmpTs::parse(bytes) {
                let Some(kind) = terrain_kind(&stem) else { continue };
                let (bw, bh) = template.blocks();
                for by in 0..bh {
                    for bx in 0..bw {
                        let Some(tile) = template.tile(bx, by) else { continue };
                        if template.tile_size() != (ISO_TILE_W, ISO_TILE_H) {
                            continue;
                        }
                        if tile.pixels.iter().all(|&index| index == 0) {
                            continue;
                        }
                        bank.push_tile(
                            kind,
                            stem.clone(),
                            tile,
                            template_slot_mask(&template, bx, by),
                        );
                    }
                }
            } else if let Ok(shp) = ShpTs::parse(bytes) {
                bank.overlays.entry(stem).or_insert(shp);
            }
        }
        let overlay_mix = MixFile::parse(overlay_archive)
            .map_err(|error| format!("{name} overlays: {error}"))?;
        for (id, resolved) in names.resolve_names(&overlay_mix) {
            let Some(file) = resolved.filter(|file| file.to_ascii_uppercase().ends_with(extension))
            else {
                continue;
            };
            let Some(bytes) = overlay_mix.by_id(id) else {
                continue;
            };
            let stem = file[..file.len().saturating_sub(4)].to_ascii_lowercase();
            if let Ok(shp) = ShpTs::parse(bytes) {
                bank.overlays.entry(stem).or_insert(shp);
            }
        }
        if bank.clear.is_empty() {
            return Err(format!("{name}: no 48x24 clear isometric tile resolved"));
        }
        for (label, tiles) in [
            ("cliff", &bank.cliff),
            ("road", &bank.road),
            ("water", &bank.water),
            ("rough", &bank.rough),
        ] {
            if tiles.is_empty() {
                return Err(format!("{name}: no {label} isometric tiles resolved"));
            }
        }
        Ok(bank)
    }

    fn push_tile(
        &mut self,
        kind: TerrainKind,
        name: String,
        tile: &IsoTile,
        neighbour_mask: u8,
    ) {
        let label = terrain_kind_name(kind).to_string();
        *self
            .terrain_histograms
            .entry(label)
            .or_default()
            .entry(tile.terrain_type)
            .or_default() += 1;
        let output = TerrainTile {
            name,
            pixels: tile.pixels.clone(),
            terrain_type: tile.terrain_type,
            raw_height: tile.height,
            neighbour_mask,
            extra: tile.extra.clone(),
        };
        match kind {
            TerrainKind::Clear => self.clear.push(output),
            TerrainKind::Cliff => self.cliff.push(output),
            TerrainKind::Road => self.road.push(output),
            TerrainKind::Water => self.water.push(output),
            TerrainKind::Rough => self.rough.push(output),
            TerrainKind::Ruin => self.ruin.push(output),
        }
    }

    fn tile(&self, kind: TerrainKind, salt: u32, neighbour_mask: u8) -> &TerrainTile {
        let candidates = match kind {
            TerrainKind::Clear => &self.clear,
            TerrainKind::Cliff => &self.cliff,
            TerrainKind::Road => &self.road,
            TerrainKind::Water => &self.water,
            TerrainKind::Rough => &self.rough,
            TerrainKind::Ruin => &self.ruin,
        };
        let candidates = if candidates.is_empty() { &self.clear } else { candidates };
        if matches!(kind, TerrainKind::Cliff | TerrainKind::Road | TerrainKind::Water) {
            if let Some(tile) = choose_masked_tile(candidates, neighbour_mask, salt) {
                return tile;
            }
        }
        &candidates[salt as usize % candidates.len()]
    }

    fn overlay(&self, stem: &str) -> Option<&ShpTs> {
        self.overlays.get(&stem.to_ascii_lowercase())
    }
}

fn template_slot_mask(template: &TmpTs, x: i32, y: i32) -> u8 {
    let mut mask = 0;
    if template.tile(x, y - 1).is_some() {
        mask |= MASK_N;
    }
    if template.tile(x + 1, y).is_some() {
        mask |= MASK_E;
    }
    if template.tile(x, y + 1).is_some() {
        mask |= MASK_S;
    }
    if template.tile(x - 1, y).is_some() {
        mask |= MASK_W;
    }
    mask
}

fn choose_masked_tile(
    candidates: &[TerrainTile],
    neighbour_mask: u8,
    salt: u32,
) -> Option<&TerrainTile> {
    let count = candidates
        .iter()
        .filter(|tile| tile.neighbour_mask == neighbour_mask)
        .count();
    candidates
        .iter()
        .filter(|tile| tile.neighbour_mask == neighbour_mask)
        .nth(salt as usize % count.max(1))
}

fn terrain_kind(stem: &str) -> Option<TerrainKind> {
    let stem = stem.to_ascii_lowercase();
    if stem.starts_with("clear") {
        Some(TerrainKind::Clear)
    } else if stem.contains("cliff") || stem.starts_with("ramp") || stem.starts_with("slope") {
        Some(TerrainKind::Cliff)
    } else if stem.starts_with("water") || stem.starts_with("shore") || stem.starts_with("swamp") {
        Some(TerrainKind::Water)
    } else if stem.contains("road") || stem.starts_with("pave") {
        Some(TerrainKind::Road)
    } else if stem.starts_with("rough") || stem.starts_with("ruff") {
        Some(TerrainKind::Rough)
    } else if stem.starts_with("ruin") {
        Some(TerrainKind::Ruin)
    } else {
        None
    }
}

fn terrain_kind_name(kind: TerrainKind) -> &'static str {
    match kind {
        TerrainKind::Clear => "clear",
        TerrainKind::Cliff => "cliff",
        TerrainKind::Road => "road",
        TerrainKind::Water => "water",
        TerrainKind::Rough => "rough",
        TerrainKind::Ruin => "ruin",
    }
}

struct Archives {
    cache: Vec<u8>,
    conquer: Vec<u8>,
    isotemp: Vec<u8>,
    isosnow: Vec<u8>,
    local: Vec<u8>,
    temperat: Vec<u8>,
    snow: Vec<u8>,
    sounds: Vec<u8>,
    speech01: Vec<u8>,
    speech02: Vec<u8>,
}

impl Archives {
    fn load(pack: &Path) -> Result<Self, String> {
        Ok(Self {
            cache: read_pack_file(pack, "cache.mix")?,
            conquer: read_pack_file(pack, "conquer.mix")?,
            isotemp: read_pack_file(pack, "isotemp.mix")?,
            isosnow: read_pack_file(pack, "isosnow.mix")?,
            local: read_pack_file(pack, "local.mix")?,
            temperat: read_pack_file(pack, "temperat.mix")?,
            snow: read_pack_file(pack, "snow.mix")?,
            sounds: read_pack_file(pack, "sounds.mix")?,
            speech01: read_pack_file(pack, "speech01.mix")?,
            speech02: read_pack_file(pack, "speech02.mix")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IsoCell {
    height: u8,
    class: char,
    kind: TerrainKind,
    variant: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GeneratedIsoMap {
    cells: Vec<IsoCell>,
    starts: Vec<(usize, usize)>,
    resource_stages: Vec<Option<u8>>,
    trees: Vec<(usize, usize, u8)>,
    ruins: Vec<(usize, usize, u8)>,
}

impl GeneratedIsoMap {
    fn cell(&self, x: usize, y: usize) -> IsoCell {
        self.cells[y * ISO_SIZE + x]
    }
}

#[derive(Default)]
struct ConvertReport {
    worlds: Vec<WorldSummary>,
    roles: BTreeMap<String, usize>,
    resolved_shapes: BTreeSet<String>,
    unresolved_shapes: BTreeSet<String>,
    resolved_icons: BTreeSet<String>,
    infantry_layouts: Vec<String>,
    structure_layouts: Vec<String>,
    sound_stems: BTreeSet<String>,
    remap_note: String,
    vehicle_direction: String,
    terrain_histograms: Vec<String>,
    rules_note: String,
}

struct WorldSummary {
    key: String,
    theater: &'static str,
    starts: usize,
    resources: usize,
    trees: usize,
    ruins: usize,
}

pub fn convert(
    pack_dir: &Path,
    staged: &Path,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<Vec<ClassicAsset>, String> {
    let archives = Archives::load(pack_dir)?;
    let names = NameTable::with_hash_kind(HASH);
    let unittem = palette(&archives.cache, "UNITTEM.PAL")?;
    let isotem = palette(&archives.cache, "ISOTEM.PAL")?;
    let isosno = palette(&archives.cache, "ISOSNO.PAL")?;
    let (rules, art, rules_note) = load_rules(&archives.local)?;
    let roster = pack_roster(&rules);
    let temperate = TheaterBank::load(
        "temperate",
        &archives.isotemp,
        &archives.temperat,
        isotem,
        &names,
    )?;
    let snow = TheaterBank::load(
        "snow",
        &archives.isosnow,
        &archives.snow,
        isosno,
        &names,
    )?;

    let mut emitter = RtsEmitter::new_scaled(staged, "ts", CONTRACT_TILE_PX, METRES_PER_PIXEL)?;
    let mut report = ConvertReport {
        rules_note,
        terrain_histograms: theater_histogram_lines(&temperate)
            .into_iter()
            .chain(theater_histogram_lines(&snow))
            .collect(),
        ..Default::default()
    };
    for seed in 1..=4u32 {
        let theater = if seed == 4 { &snow } else { &temperate };
        tick(on_tick, seed as usize - 1, 4, format!("world ts-{seed}"), None);
        let generated = generate_map(seed);
        let terrain = render_terrain(&generated, theater);
        let grid = rasterize_classes(&generated);
        let world = generated_world(
            seed,
            &generated,
            terrain,
            grid,
            theater.name,
            roster.clone(),
        );
        let preview = emitter.emit_world(world)?;
        report.worlds.push(WorldSummary {
            key: format!("ts-{seed}"),
            theater: theater.name,
            starts: generated.starts.len(),
            resources: generated.resource_stages.iter().flatten().count(),
            trees: generated.trees.len(),
            ruins: generated.ruins.len(),
        });
        tick(on_tick, seed as usize, 4, format!("world ts-{seed}"), Some(preview));
    }

    let remap_start = infer_remap_start(&archives.conquer).unwrap_or(16);
    report.remap_note = remap_report(&archives.conquer, remap_start);
    report.vehicle_direction =
        "MMCH frame 0 is north; the 0, 8, 16, 24 silhouettes turn north, west, south, east, so source frames advance counter-clockwise and manifest rot is 1+frame".into();
    let remap = remap_line(&unittem, remap_start);
    let audio = resolved_audio(&archives, &names);
    emit_mobile_sprites(
        &mut emitter,
        &archives,
        &unittem,
        &rules,
        &art,
        &audio,
        &remap,
        &mut report,
        on_tick,
    )?;
    emit_structures(
        &mut emitter,
        &archives,
        &unittem,
        &rules,
        &audio,
        &remap,
        &mut report,
        on_tick,
    )?;
    emit_theater_scenery(&mut emitter, &archives, &temperate, &unittem, &mut report)?;
    emit_effects(&mut emitter, &archives, &unittem, &mut report)?;
    emit_audio(&mut emitter, &archives, &names, &mut report, on_tick)?;
    write_report(&report)?;
    Ok(emitter.finish())
}

fn load_rules(local: &[u8]) -> Result<(Rules, Ini, String), String> {
    let rules_bytes = mix_entry(local, "RULES.INI").ok_or("local.mix has no RULES.INI")?;
    let art_bytes = mix_entry(local, "ART.INI").ok_or("local.mix has no ART.INI")?;
    let rules_ini = Ini::parse(&String::from_utf8_lossy(rules_bytes));
    let art_ini = Ini::parse(&String::from_utf8_lossy(art_bytes));
    let rules = Rules::parse_ts(&rules_ini, &art_ini).map_err(|error| error.to_string())?;
    let note = format!(
        "local.mix/RULES.INI + ART.INI resolved: {} unit/building records, {} weapons, {} warheads",
        rules.units.len(),
        rules.weapons.len(),
        rules.warheads.len()
    );
    Ok((rules, art_ini, note))
}

fn theater_histogram_lines(bank: &TheaterBank) -> Vec<String> {
    bank.terrain_histograms
        .iter()
        .map(|(category, histogram)| {
            let values = histogram
                .iter()
                .map(|(value, count)| format!("{value}:{count}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} {category}: {values}", bank.name)
        })
        .collect()
}

/// The map comes from the SHARED generator (`makepad_rtsmap`) — the same one
/// the sandbox calls at runtime and the same one the other procedural
/// converter uses, so there is ONE algorithm with one set of fairness and
/// reachability guarantees. What is Tiberian-Sun about this function is only
/// the translation: neutral terrain onto this pack's isometric terrain
/// kinds, and neutral scenery onto the shapes this pack ships.
fn generate_map(seed: u32) -> GeneratedIsoMap {
    // The house count this converter has always produced for a seed.
    let players = (2 + (seed as usize).saturating_sub(1) % 3).clamp(2, 4) as u8;
    let mut map = makepad_rtsmap::generate(&makepad_rtsmap::MapSpec {
        seed,
        width: ISO_SIZE as u16,
        height: ISO_SIZE as u16,
        players,
        style: makepad_rtsmap::Style::Temperate,
        resources: 0.7,
        cliffs: 0.7,
        water: 1.0,
        roads: 1.0,
        theater: "temperate".into(),
        retries: 8,
    });
    // Trees and ruins this pack draws; loose boulders it does not, and a
    // prop with no art would block its cell invisibly.
    map.retain_props(|prop| {
        matches!(prop.kind, makepad_rtsmap::PropKind::Tree | makepad_rtsmap::PropKind::Ruin)
    });

    let cells = (0..ISO_SIZE * ISO_SIZE)
        .map(|index| {
            let (x, y) = (index % ISO_SIZE, index / ISO_SIZE);
            let kind = terrain_kind_of(map.terrain[index]);
            let variant = terrain_noise(seed ^ variant_salt(kind), x, y);
            IsoCell {
                height: map.heights[index],
                class: map.grid[index] as char,
                kind,
                variant,
            }
        })
        .collect::<Vec<_>>();
    let mut cells = cells;

    let starts = map
        .starts
        .iter()
        .map(|start| (start.x as usize, start.y as usize))
        .collect::<Vec<_>>();
    let resource_stages = map.stage_grid();
    let mut trees = Vec::new();
    let mut ruins = Vec::new();
    for prop in &map.props {
        let at = prop.y as usize * ISO_SIZE + prop.x as usize;
        match prop.kind {
            makepad_rtsmap::PropKind::Tree => {
                // `tree01`..`tree09` is what this pack has.
                trees.push((prop.x as usize, prop.y as usize, (prop.variant - 1) % 9 + 1));
            }
            makepad_rtsmap::PropKind::Ruin => {
                // A ruin is a BUILDING, so the cell it stands on reads as one.
                cells[at].kind = TerrainKind::Ruin;
                ruins.push((prop.x as usize, prop.y as usize, (prop.variant - 1) % 4 + 1));
            }
            _ => {}
        }
    }
    GeneratedIsoMap { cells, starts, resource_stages, trees, ruins }
}

/// Neutral terrain onto this pack's isometric terrain banks. A plateau TOP
/// is ordinary ground that happens to be a level up; only its rim is cliff.
fn terrain_kind_of(terrain: makepad_rtsmap::Terrain) -> TerrainKind {
    use makepad_rtsmap::Terrain;
    match terrain {
        Terrain::Rough | Terrain::Shore => TerrainKind::Rough,
        Terrain::Road => TerrainKind::Road,
        Terrain::Water => TerrainKind::Water,
        Terrain::Cliff => TerrainKind::Cliff,
        Terrain::Clear | Terrain::Plateau | Terrain::Resource => TerrainKind::Clear,
    }
}

/// One noise stream per terrain bank, so a road's variants and a river's do
/// not march in step down the map.
fn variant_salt(kind: TerrainKind) -> u32 {
    match kind {
        TerrainKind::Water => 0x125a_52d9,
        TerrainKind::Rough => 0x6a31_f2c9,
        TerrainKind::Road => 0x33a1_92e7,
        _ => 0,
    }
}

fn terrain_noise(seed: u32, x: usize, y: usize) -> u32 {
    let mut value = seed
        .wrapping_add((x as u32).wrapping_mul(0x9e37_79b9))
        .wrapping_add((y as u32).wrapping_mul(0x85eb_ca6b));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^ (value >> 15)
}

/// Returns the top-centre anchor of an isometric tile in the generated
/// screen-space image. Height raises the art by one half-diamond per level.
fn iso_to_screen(x: usize, y: usize, screen_width: i32, height: u8) -> (i32, i32) {
    (
        (x as i32 - y as i32) * (ISO_TILE_W / 2) + screen_width / 2,
        (x as i32 + y as i32) * (ISO_TILE_H / 2)
            - i32::from(height) * HEIGHT_STEP_PX,
    )
}

fn render_order() -> Vec<(usize, usize)> {
    let mut order = Vec::with_capacity(ISO_SIZE * ISO_SIZE);
    for diagonal in 0..=2 * (ISO_SIZE - 1) {
        for x in 0..ISO_SIZE {
            let Some(y) = diagonal.checked_sub(x) else { continue };
            if y < ISO_SIZE {
                order.push((x, y));
            }
        }
    }
    order
}

fn render_terrain(map: &GeneratedIsoMap, theater: &TheaterBank) -> Vec<u8> {
    let width = SCREEN_W_CELLS as u32 * CONTRACT_TILE_PX;
    let height = SCREEN_H_CELLS as u32 * CONTRACT_TILE_PX;
    let mut output = vec![0u8; (width * height * 4) as usize];
    for pixel in output.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[10, 10, 10, 255]);
    }
    for (x, y) in render_order() {
        let cell = map.cell(x, y);
        let art = theater.tile(
            cell.kind,
            cell.variant,
            terrain_neighbour_mask(map, x, y, cell.kind),
        );
        let (sx, sy) = iso_to_screen(x, y, width as i32, cell.height);
        blit_indexed(
            &mut output,
            width,
            height,
            sx - ISO_TILE_W / 2,
            sy,
            &art.pixels,
            ISO_TILE_W,
            ISO_TILE_H,
            &theater.palette,
        );
        if let Some(extra) = &art.extra {
            blit_indexed(
                &mut output,
                width,
                height,
                sx + extra.x,
                sy + extra.y,
                &extra.pixels,
                extra.w,
                extra.h,
                &theater.palette,
            );
        }
    }
    output
}

fn terrain_neighbour_mask(
    map: &GeneratedIsoMap,
    x: usize,
    y: usize,
    kind: TerrainKind,
) -> u8 {
    let mut mask = 0;
    if y > 0 && map.cell(x, y - 1).kind == kind {
        mask |= MASK_N;
    }
    if x + 1 < ISO_SIZE && map.cell(x + 1, y).kind == kind {
        mask |= MASK_E;
    }
    if y + 1 < ISO_SIZE && map.cell(x, y + 1).kind == kind {
        mask |= MASK_S;
    }
    if x > 0 && map.cell(x - 1, y).kind == kind {
        mask |= MASK_W;
    }
    mask
}

#[allow(clippy::too_many_arguments)]
fn blit_indexed(
    destination: &mut [u8],
    destination_width: u32,
    destination_height: u32,
    ox: i32,
    oy: i32,
    source: &[u8],
    source_width: i32,
    source_height: i32,
    palette: &Pal,
) {
    if source_width <= 0 || source_height <= 0 {
        return;
    }
    let (Ok(source_width_u), Ok(source_height_u)) = (
        usize::try_from(source_width),
        usize::try_from(source_height),
    ) else {
        return;
    };
    let Some(source_size) = source_width_u.checked_mul(source_height_u) else { return };
    if source.len() < source_size {
        return;
    }
    for sy in 0..source_height_u {
        for sx in 0..source_width_u {
            let dx = ox + sx as i32;
            let dy = oy + sy as i32;
            if dx < 0
                || dy < 0
                || dx >= destination_width as i32
                || dy >= destination_height as i32
            {
                continue;
            }
            let index = source[sy * source_width_u + sx];
            if index == 0 {
                continue;
            }
            let at = ((dy as u32 * destination_width + dx as u32) * 4) as usize;
            let [r, g, b] = palette.rgb(index);
            destination[at..at + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
}

fn rasterize_classes(map: &GeneratedIsoMap) -> Vec<String> {
    rasterize_classes_sized(map, ISO_SIZE, ISO_SIZE)
}

fn rasterize_classes_sized(
    map: &GeneratedIsoMap,
    iso_width: usize,
    iso_height: usize,
) -> Vec<String> {
    if iso_width == 0 || iso_height == 0 || map.cells.len() < iso_width.saturating_mul(iso_height) {
        return Vec::new();
    }
    let grid_width = iso_width.saturating_mul(2);
    let grid_height = iso_height;
    let screen_width = (grid_width as u32).saturating_mul(CONTRACT_TILE_PX) as i32;
    let mut order = (0..iso_height)
        .flat_map(|y| (0..iso_width).map(move |x| (x, y)))
        .collect::<Vec<_>>();
    order.sort_by_key(|&(x, y)| (x + y, x));
    let mut rows = Vec::with_capacity(grid_height);
    for cell_y in 0..grid_height {
        let py = cell_y as i32 * CONTRACT_TILE_PX as i32 + CONTRACT_TILE_PX as i32 / 2;
        let mut row = String::with_capacity(grid_width);
        for cell_x in 0..grid_width {
            let px = cell_x as i32 * CONTRACT_TILE_PX as i32 + CONTRACT_TILE_PX as i32 / 2;
            let mut class = '#';
            for &(x, y) in order.iter().rev() {
                let source = map.cells[y * iso_width + x];
                let (sx, top) = iso_to_screen(x, y, screen_width, source.height);
                let dx = (px - sx).abs();
                let dy = (py - (top + ISO_TILE_H / 2)).abs();
                if dx * (ISO_TILE_H / 2) + dy * (ISO_TILE_W / 2)
                    <= (ISO_TILE_W / 2) * (ISO_TILE_H / 2)
                {
                    class = source.class;
                    break;
                }
            }
            row.push(class);
        }
        rows.push(row);
    }
    rows
}

fn iso_world_position(x: usize, y: usize, height: u8) -> (f32, f32) {
    let screen_width = SCREEN_W_CELLS as i32 * CONTRACT_TILE_PX as i32;
    let (sx, top) = iso_to_screen(x, y, screen_width, height);
    (
        sx as f32 * METRES_PER_PIXEL,
        (top + ISO_TILE_H / 2) as f32 * METRES_PER_PIXEL,
    )
}

fn generated_world(
    seed: u32,
    generated: &GeneratedIsoMap,
    terrain_rgba: Vec<u8>,
    grid: Vec<String>,
    theater: &'static str,
    roster: Vec<String>,
) -> WorldSpec {
    let key = format!("ts-{seed}");
    let world_key = format!("worlds/{key}");
    let mut place = format!(
        "world-place 1\nsource ts\nworld {world_key}\nmode rts\ncell 3.0\ntile 24\nmetres_per_pixel 0.125\ngrid {world_key}.grid\n"
    );
    place.push_str("house GDI color=e8c040 side=gdi\n");
    place.push_str("house NOD color=d02020 side=nod\n");
    let mut resource_index = 0usize;
    for y in 0..ISO_SIZE {
        for x in 0..ISO_SIZE {
            let Some(stage) = generated.resource_stages[y * ISO_SIZE + x] else { continue };
            let (world_x, world_z) = iso_world_position(x, y, generated.cell(x, y).height);
            place.push_str(&format!(
                "place r-{resource_index} resource billboards/ts/tib {world_x:.4} 0.0400 {world_z:.4} 0.00000 align=floor layer=0.04 class=resource stage={stage}\n"
            ));
            resource_index += 1;
        }
    }
    for (index, &(x, y, tree)) in generated.trees.iter().enumerate() {
        let (world_x, world_z) = iso_world_position(x, y, generated.cell(x, y).height);
        place.push_str(&format!(
            "place tree-{index} scenery billboards/ts/tree{tree:02} {world_x:.4} 0.0600 {world_z:.4} 0.00000 align=floor layer=0.06 class=tree\n"
        ));
    }
    for (index, &(x, y, ruin)) in generated.ruins.iter().enumerate() {
        let (world_x, world_z) = iso_world_position(x, y, generated.cell(x, y).height);
        place.push_str(&format!(
            "place ruin-{index} scenery billboards/ts/aban{ruin:02} {world_x:.4} 0.0600 {world_z:.4} 0.00000 align=floor layer=0.06 class=ruin\n"
        ));
    }

    let mut spawn = String::from("world-spawn 1\n");
    let mut preview_dots = Vec::new();
    for (index, &(x, y)) in generated.starts.iter().enumerate() {
        let (world_x, world_z) = iso_world_position(x, y, 0);
        spawn.push_str(&format!(
            "start start_{index} {world_x:.4} 0.0000 {world_z:.4} 0.00000 -1.45000\n"
        ));
        preview_dots.push(PreviewDot {
            x: world_x / CELL_M,
            y: world_z / CELL_M,
            rgb: if index & 1 == 0 { [232, 192, 64] } else { [208, 32, 32] },
        });
    }
    spawn.push_str("floor 0\nstep 0.5\neye 60\n");
    WorldSpec {
        key,
        width: SCREEN_W_CELLS as u16,
        height: SCREEN_H_CELLS as u16,
        terrain_rgba,
        grid,
        place_text: place,
        roster,
        spawn_text: spawn,
        preview_dots,
        preview_crop: Some(PreviewCrop {
            x: 0,
            y: 0,
            width: SCREEN_W_CELLS as u32 * CONTRACT_TILE_PX,
            height: SCREEN_H_CELLS as u32 * CONTRACT_TILE_PX,
        }),
        tags: vec!["ts".into(), "world".into(), "rts".into(), theater.into()],
    }
}

fn emit_mobile_sprites(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    rules: &Rules,
    art: &Ini,
    audio: &BTreeSet<String>,
    remap: &str,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let total = VEHICLES.len() + INFANTRY.len();
    let mut done = 0usize;
    for &(key, source, rules_key) in VEHICLES {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = conquer_shp(archives, source) else {
            report.unresolved_shapes.insert(format!("{key} <- {}.SHP", source.to_ascii_uppercase()));
            continue;
        };
        if shp.frames().len() < 32 {
            report.unresolved_shapes.insert(format!(
                "{key} <- {}.SHP ({} frames, needs 32)",
                source.to_ascii_uppercase(),
                shp.frames().len()
            ));
            continue;
        }
        let frames = (0..32)
            .filter_map(|source_frame| sprite_frame(&shp, source_frame, palette, 1 + source_frame as u8))
            .collect::<Vec<_>>();
        if frames.len() != 32 {
            continue;
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 32,
                frames,
                states: vec![SpriteState { name: "idle", first: 0, last: 32, looping: true, fps: 8 }],
                unit: Some(UnitSpec {
                    manifest_lines: unit_manifest_lines(key, rules_key, "vehicle", None, rules, audio),
                }),
                manifest_lines: vec![remap.into()],
                tags: vec!["ts", "unit", "vehicle"],
            },
        )?;
        report.resolved_shapes.insert(format!(
            "{key} <- {}.SHP ({} frames; hull 0..31)",
            source.to_ascii_uppercase(),
            shp.frames().len()
        ));
        emit_icon(emitter, archives, palette, key, report)?;
    }

    for &(key, source) in INFANTRY {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = conquer_shp(archives, source) else {
            report.unresolved_shapes.insert(format!("{key} <- {}.SHP", source.to_ascii_uppercase()));
            continue;
        };
        let sequence_section = art
            .get(source, "Sequence")
            .or_else(|| art.get(&source.to_ascii_uppercase(), "Sequence"));
        let layout = infantry_layout(
            &shp,
            sequence_section.and_then(|section| art.section(section)),
            palette,
        );
        let Some((frames, states, note)) = layout else {
            report.unresolved_shapes.insert(format!(
                "{key} <- {}.SHP (no safe idle/walk/fire/die layout)",
                source.to_ascii_uppercase()
            ));
            continue;
        };
        let rules_key = if key.starts_with("civ") {
            String::new()
        } else {
            source.to_ascii_uppercase()
        };
        let class = if key == "jumpjet" { "aircraft" } else { "infantry" };
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 8,
                frames,
                states,
                unit: Some(UnitSpec {
                    manifest_lines: unit_manifest_lines(key, &rules_key, class, None, rules, audio),
                }),
                manifest_lines: vec![remap.into()],
                tags: vec!["ts", "unit", class],
            },
        )?;
        report.infantry_layouts.push(format!(
            "{key}: {}.SHP {} source frames; {note}",
            source.to_ascii_uppercase(),
            shp.frames().len()
        ));
        report.resolved_shapes.insert(format!("{key} <- {}.SHP", source.to_ascii_uppercase()));
        emit_icon(emitter, archives, palette, key, report)?;
    }
    audit_requested_mobile_shapes(archives, report);
    Ok(())
}

type InfantryLayout = (Vec<SpritePixels>, Vec<SpriteState>, String);

fn infantry_layout(
    shp: &ShpTs,
    sequence: Option<&[(String, String)]>,
    palette: &Pal,
) -> Option<InfantryLayout> {
    let ready = sequence.and_then(|values| sequence_value(values, &["Ready", "Guard"]));
    let walk = sequence.and_then(|values| sequence_value(values, &["Walk", "Fly"]));
    let fire = sequence.and_then(|values| sequence_value(values, &["FireUp", "FireFly", "FireProne"]));
    let die = sequence.and_then(|values| sequence_value(values, &["Die1", "Die2"]));
    if let (Some(ready), Some(walk), Some(fire), Some(die)) = (ready, walk, fire, die) {
        let mut frames = Vec::new();
        append_sequence(&mut frames, shp, ready, true, palette)?;
        let idle_last = frames.len();
        append_sequence(&mut frames, shp, walk, true, palette)?;
        let walk_last = frames.len();
        append_sequence(&mut frames, shp, fire, true, palette)?;
        let fire_last = frames.len();
        append_sequence(&mut frames, shp, die, false, palette)?;
        let die_last = frames.len();
        let states = infantry_states(idle_last, walk_last, fire_last, die_last);
        let note = format!(
            "ART.INI Ready={},{} stride {}; Walk={},{} stride {}; Fire={},{} stride {}; Die={},{}",
            ready.0, ready.1, ready.2, walk.0, walk.1, walk.2,
            fire.0, fire.1, fire.2, die.0, die.1,
        );
        return Some((frames, states, note));
    }

    if shp.frames().len() < 112 {
        return None;
    }
    let mut frames = Vec::new();
    append_linear_facing(&mut frames, shp, 0, 8, 1, palette)?;
    let idle_last = frames.len();
    append_linear_facing(&mut frames, shp, 8, 56, 6, palette)?;
    let walk_last = frames.len();
    append_linear_facing(&mut frames, shp, 56, 104, 6, palette)?;
    let fire_last = frames.len();
    let die_first = shp.frames().len().saturating_sub(15);
    append_nondirectional(&mut frames, shp, die_first, shp.frames().len(), palette)?;
    let die_last = frames.len();
    Some((
        frames,
        infantry_states(idle_last, walk_last, fire_last, die_last),
        format!(
            "fallback inferred from frame count: idle 0..8, walk 8..56 (6/facing), fire 56..104 (6/facing), die {die_first}..{}",
            shp.frames().len()
        ),
    ))
}

fn infantry_states(
    idle_last: usize,
    walk_last: usize,
    fire_last: usize,
    die_last: usize,
) -> Vec<SpriteState> {
    vec![
        SpriteState { name: "idle", first: 0, last: idle_last, looping: true, fps: 8 },
        SpriteState { name: "walk", first: idle_last, last: walk_last, looping: true, fps: 10 },
        SpriteState { name: "fire", first: walk_last, last: fire_last, looping: false, fps: 10 },
        SpriteState { name: "die", first: fire_last, last: die_last, looping: false, fps: 10 },
    ]
}

fn sequence_value(values: &[(String, String)], keys: &[&str]) -> Option<(usize, usize, usize)> {
    let value = keys.iter().find_map(|wanted| {
        values
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.as_str())
    })?;
    let mut fields = value.split(',').map(str::trim);
    let first = fields.next()?.parse().ok()?;
    let count = fields.next()?.parse().ok()?;
    let stride = fields.next().and_then(|field| field.parse().ok()).unwrap_or(count);
    if count == 0 {
        None
    } else {
        Some((first, count, stride))
    }
}

fn append_sequence(
    output: &mut Vec<SpritePixels>,
    shp: &ShpTs,
    (first, count, stride): (usize, usize, usize),
    directional: bool,
    palette: &Pal,
) -> Option<()> {
    if directional && stride > 0 {
        for facing in 0..8usize {
            let facing_first = first.checked_add(facing.checked_mul(stride)?)?;
            for offset in 0..count {
                output.push(sprite_frame(shp, facing_first.checked_add(offset)?, palette, 1 + facing as u8)?);
            }
        }
    } else {
        for source in first..first.checked_add(count)? {
            output.push(sprite_frame(shp, source, palette, 0)?);
        }
    }
    Some(())
}

fn append_linear_facing(
    output: &mut Vec<SpritePixels>,
    shp: &ShpTs,
    first: usize,
    last: usize,
    frames_per_facing: usize,
    palette: &Pal,
) -> Option<()> {
    for source in first..last {
        output.push(sprite_frame(
            shp,
            source,
            palette,
            1 + ((source - first) / frames_per_facing.max(1)) as u8,
        )?);
    }
    Some(())
}

fn append_nondirectional(
    output: &mut Vec<SpritePixels>,
    shp: &ShpTs,
    first: usize,
    last: usize,
    palette: &Pal,
) -> Option<()> {
    for source in first..last {
        output.push(sprite_frame(shp, source, palette, 0)?);
    }
    Some(())
}

fn emit_structures(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    rules: &Rules,
    audio: &BTreeSet<String>,
    remap: &str,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    for (index, structure) in STRUCTURES.iter().enumerate() {
        tick(
            on_tick,
            index,
            STRUCTURES.len(),
            format!("structure {}", structure.key),
            None,
        );
        let Some(base) = conquer_shp(archives, structure.source) else {
            report.unresolved_shapes.insert(format!(
                "{} <- {}.SHP",
                structure.key,
                structure.source.to_ascii_uppercase()
            ));
            continue;
        };
        if base.frames().is_empty() {
            continue;
        }
        let base_half = base.frames().len().div_ceil(2);
        let mut healthy = Vec::new();
        let mut damaged = Vec::new();
        append_nondirectional(&mut healthy, &base, 0, base_half, palette);
        append_nondirectional(
            &mut damaged,
            &base,
            base_half,
            base.frames().len(),
            palette,
        );
        let mut animation_notes = Vec::new();
        for suffix in ['a', 'b', 'c', 'd', 'e', 'f'] {
            let stem = format!("{}_{}", structure.source, suffix);
            let Some(animation) = conquer_shp(archives, &stem) else { continue };
            if animation.frames().is_empty() {
                continue;
            }
            let half = animation.frames().len().div_ceil(2);
            append_nondirectional(&mut healthy, &animation, 0, half, palette);
            append_nondirectional(
                &mut damaged,
                &animation,
                half,
                animation.frames().len(),
                palette,
            );
            animation_notes.push(format!("_{suffix}={half}+{}", animation.frames().len() - half));
        }
        if healthy.is_empty() {
            continue;
        }
        if damaged.is_empty() {
            damaged.push(healthy[healthy.len() - 1].clone());
        }
        let idle_last = healthy.len();
        let damaged_last = idle_last + damaged.len();
        healthy.extend(damaged);
        let build_first = healthy.len();
        let make_stem = format!("{}mk", structure.source);
        let build_count = conquer_shp(archives, &make_stem).map_or(0, |make| {
            let before = healthy.len();
            append_nondirectional(&mut healthy, &make, 0, make.frames().len(), palette);
            healthy.len() - before
        });
        let mut states = vec![
            SpriteState { name: "idle", first: 0, last: idle_last, looping: true, fps: 6 },
            SpriteState { name: "damaged", first: idle_last, last: damaged_last, looping: true, fps: 6 },
        ];
        if build_count > 0 {
            states.push(SpriteState {
                name: "build",
                first: build_first,
                last: healthy.len(),
                looping: false,
                fps: 15,
            });
        }
        let mut manifest_lines = unit_manifest_lines(
            structure.key,
            structure.rules,
            structure.class,
            Some(structure),
            rules,
            audio,
        );
        manifest_lines.push(format!("footprint {} {}", structure.footprint.0, structure.footprint.1));
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: structure.key.into(),
                role: "structure",
                facings: 1,
                frames: healthy,
                states,
                unit: Some(UnitSpec { manifest_lines }),
                manifest_lines: vec![remap.into()],
                tags: vec!["ts", "structure", structure.class],
            },
        )?;
        report.structure_layouts.push(format!(
            "{} <- {}.SHP base={base_half}+{}, animations [{}], build={} frames",
            structure.key,
            structure.source.to_ascii_uppercase(),
            base.frames().len() - base_half,
            animation_notes.join(", "),
            build_count,
        ));
        report.resolved_shapes.insert(format!(
            "{} <- {}.SHP",
            structure.key,
            structure.source.to_ascii_uppercase()
        ));
        emit_icon(emitter, archives, palette, structure.key, report)?;
        tick(
            on_tick,
            index + 1,
            STRUCTURES.len(),
            format!("structure {}", structure.key),
            None,
        );
    }
    Ok(())
}

fn sprite_frame(shp: &ShpTs, source: usize, palette: &Pal, rot: u8) -> Option<SpritePixels> {
    let frame = shp.frames().get(source)?;
    let (width, height) = shp.canvas();
    let expected = usize::from(width).checked_mul(usize::from(height))?;
    if frame.pixels.len() != expected || width == 0 || height == 0 {
        return None;
    }
    Some(SpritePixels {
        rgba: indexed_transparent(&frame.pixels, palette),
        width: u32::from(width),
        height: u32::from(height),
        rot,
    })
}

fn indexed_transparent(indices: &[u8], palette: &Pal) -> Vec<u8> {
    let mut output = Vec::with_capacity(indices.len() * 4);
    for &index in indices {
        if index == 0 {
            output.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let [r, g, b] = palette.rgb(index);
            output.extend_from_slice(&[r, g, b, 255]);
        }
    }
    output
}

fn conquer_shp(archives: &Archives, stem: &str) -> Option<ShpTs> {
    let name = format!("{}.SHP", stem.to_ascii_uppercase());
    [
        &archives.conquer,
        &archives.isotemp,
        &archives.isosnow,
        &archives.temperat,
        &archives.snow,
        &archives.cache,
        &archives.local,
    ]
    .into_iter()
    .find_map(|archive| ShpTs::parse(mix_entry(archive, &name)?).ok())
}

fn emit_spec(
    emitter: &mut RtsEmitter<'_>,
    report: &mut ConvertReport,
    spec: SpriteSpec,
) -> Result<(), String> {
    *report.roles.entry(spec.role.into()).or_default() += 1;
    emitter.emit_sprite(spec)
}

#[derive(Clone, Copy)]
struct ManualStats {
    title: &'static str,
    cost: i32,
    hp: i32,
    armor: &'static str,
    speed: f32,
    sight: f32,
    sides: &'static str,
    weapon: &'static str,
}

fn unit_manifest_lines(
    key: &str,
    rules_key: &str,
    class: &str,
    structure: Option<&StructureDef>,
    rules: &Rules,
    audio: &BTreeSet<String>,
) -> Vec<String> {
    let fallback = manual_stats(key);
    let source = rules_unit(rules, rules_key);
    let title = fallback.title;
    let cost = source.filter(|unit| unit.cost > 0).map_or(fallback.cost, |unit| unit.cost);
    let hp = source
        .filter(|unit| unit.strength > 0)
        .map_or(fallback.hp, |unit| unit.strength);
    let armor = source
        .map(|unit| unit.armor.trim())
        .filter(|armor| !armor.is_empty())
        .unwrap_or(fallback.armor)
        .to_ascii_lowercase();
    let speed = source.filter(|unit| unit.speed > 0).map_or(fallback.speed, |unit| {
        if class == "infantry" { unit.speed as f32 * 0.5 } else { unit.speed as f32 }
    });
    let sight = source
        .filter(|unit| unit.sight > 0)
        .map_or(fallback.sight, |unit| unit.sight as f32 * CELL_M);
    let sides = source
        .map(|unit| canonical_sides(&unit.owner))
        .filter(|sides| !sides.is_empty())
        .unwrap_or_else(|| fallback.sides.into());
    let primary = source
        .map(|unit| unit.primary.trim())
        .filter(|weapon| !weapon.is_empty() && !weapon.eq_ignore_ascii_case("none"))
        .unwrap_or(fallback.weapon);
    let secondary = source
        .map(|unit| unit.secondary.trim())
        .filter(|weapon| !weapon.is_empty() && !weapon.eq_ignore_ascii_case("none"));
    let weapon_id = sanitize_id(primary);
    let mut unit = format!(
        "unit class={class} title=\"{title}\" cost={cost} hp={hp} armor={armor} speed={speed:.2} sight={sight:.1} sides={sides}"
    );
    if let Some(structure) = structure {
        let role = role_for(ROLE_TABLE, structure.key)
            .unwrap_or_else(|| panic!("missing TS structure role for {}", structure.key));
        debug_assert_eq!(role, structure.role);
        unit.push_str(&format!(
            " role={} footprint={}x{} build=1",
            role, structure.footprint.0, structure.footprint.1
        ));
    } else if class == "aircraft" {
        unit.push_str(" builds_at=aircraft_pad");
    } else if class == "infantry" {
        unit.push_str(" builds_at=barracks");
    } else if class == "vehicle" {
        unit.push_str(" builds_at=vehicle_factory");
    }
    if let Some(source) = source {
        let prerequisites = source
            .prerequisite
            .iter()
            .filter_map(|name| prerequisite_role(name))
            .collect::<BTreeSet<_>>();
        if !prerequisites.is_empty() {
            unit.push_str(&format!(
                " prereq={}",
                prerequisites.into_iter().collect::<Vec<_>>().join(",")
            ));
        }
    }
    if !weapon_id.is_empty() {
        unit.push_str(&format!(" weapon={weapon_id}"));
    }
    if let Some(secondary) = secondary {
        let secondary = sanitize_id(secondary);
        if !secondary.is_empty() {
            unit.push_str(&format!(" weapon2={secondary}"));
        }
    }
    unit.push_str(unit_flags(key));
    let mut lines = vec![unit];
    for weapon in std::iter::once(primary).chain(secondary) {
        if weapon.is_empty() {
            continue;
        }
        lines.push(weapon_manifest_line(weapon, rules));
    }
    if let Some(sound) = source
        .and_then(|unit| rules_weapon(rules, &unit.primary))
        .and_then(|weapon| {
            weapon
                .report
                .iter()
                .map(|stem| sanitize_id(stem))
                .find(|stem| audio.contains(stem))
        })
    {
        lines.push(format!("sound attack=sfx/ts/{sound}"));
    }
    lines
}

fn pack_roster(rules: &Rules) -> Vec<String> {
    let audio = BTreeSet::new();
    let mut roster = Vec::new();
    for &(key, _, rules_key) in VEHICLES {
        let lines = unit_manifest_lines(key, rules_key, "vehicle", None, rules, &audio);
        if lines.first().is_some_and(|line| positive_unit_cost(line)) {
            roster.push(roster_key("ts", key));
        }
    }
    for &(key, source) in INFANTRY {
        let rules_key = if key.starts_with("civ") {
            String::new()
        } else {
            source.to_ascii_uppercase()
        };
        let class = if key == "jumpjet" { "aircraft" } else { "infantry" };
        let lines = unit_manifest_lines(key, &rules_key, class, None, rules, &audio);
        if lines.first().is_some_and(|line| positive_unit_cost(line)) {
            roster.push(roster_key("ts", key));
        }
    }
    for structure in STRUCTURES {
        let lines = unit_manifest_lines(
            structure.key,
            structure.rules,
            structure.class,
            Some(structure),
            rules,
            &audio,
        );
        if lines.first().is_some_and(|line| positive_unit_cost(line)) {
            roster.push(roster_key("ts", structure.key));
        }
    }
    let mcv = roster_key("ts", "mcv");
    if !roster.contains(&mcv) {
        roster.push(mcv);
    }
    roster
}

#[cfg(test)]
pub(super) fn role_test_lines() -> Vec<String> {
    let rules = Rules::default();
    let audio = BTreeSet::new();
    let mut lines = Vec::new();
    for &(key, _, rules_key) in VEHICLES {
        lines.extend(unit_manifest_lines(
            key,
            rules_key,
            "vehicle",
            None,
            &rules,
            &audio,
        ));
    }
    for &(key, source) in INFANTRY {
        let rules_key = if key.starts_with("civ") {
            String::new()
        } else {
            source.to_ascii_uppercase()
        };
        let class = if key == "jumpjet" { "aircraft" } else { "infantry" };
        lines.extend(unit_manifest_lines(
            key,
            &rules_key,
            class,
            None,
            &rules,
            &audio,
        ));
    }
    for structure in STRUCTURES {
        lines.extend(unit_manifest_lines(
            structure.key,
            structure.rules,
            structure.class,
            Some(structure),
            &rules,
            &audio,
        ));
    }
    lines
}

fn manual_stats(key: &str) -> ManualStats {
    let values = match key {
        "e1" => ("Light Infantry", 120, 50, "none", 2.5, 12.0, "GDI,NOD", "m1carbine"),
        "e2" => ("Disc Thrower", 160, 60, "none", 2.5, 12.0, "GDI", "disc"),
        "e3" => ("Rocket Infantry", 300, 40, "none", 2.5, 12.0, "GDI,NOD", "dragon"),
        "medic" => ("Medic", 300, 50, "none", 2.5, 12.0, "GDI", ""),
        "engineer" => ("Engineer", 500, 25, "none", 2.5, 12.0, "GDI,NOD", ""),
        "ghost" => ("Ghost Stalker", 1200, 100, "none", 3.0, 18.0, "GDI", "railgun"),
        "cyborg" => ("Cyborg", 450, 200, "light", 2.0, 12.0, "NOD", "chaingun"),
        "jumpjet" => ("Jump Jet", 600, 60, "none", 6.0, 18.0, "GDI", "m1carbine"),
        "titan" => ("Titan", 800, 300, "heavy", 5.0, 18.0, "GDI", "120mm"),
        "wolverine" => ("Wolverine", 500, 170, "light", 8.0, 18.0, "GDI", "mgun"),
        "gacnst" | "nacnst" => ("Construction Yard", 2500, 1000, "concrete", 0.0, 30.0, if key == "gacnst" { "GDI" } else { "NOD" }, ""),
        "gapowr" | "napowr" => ("Power Plant", 300, 500, "wood", 0.0, 18.0, if key == "gapowr" { "GDI" } else { "NOD" }, ""),
        "garefn" | "narefn" => ("Refinery", 2000, 900, "wood", 0.0, 24.0, if key == "garefn" { "GDI" } else { "NOD" }, ""),
        "gasilo" | "nasilo" => ("Tiberium Silo", 150, 300, "wood", 0.0, 12.0, if key == "gasilo" { "GDI" } else { "NOD" }, ""),
        "gapile" => ("Barracks", 300, 500, "wood", 0.0, 18.0, "GDI", ""),
        "nahand" => ("Hand of Nod", 300, 500, "wood", 0.0, 18.0, "NOD", ""),
        "gaweap" | "naweap" => ("War Factory", 2000, 1000, "light", 0.0, 18.0, if key == "gaweap" { "GDI" } else { "NOD" }, ""),
        "garadr" | "naradr" => ("Radar", 1000, 800, "wood", 0.0, 30.0, if key == "garadr" { "GDI" } else { "NOD" }, ""),
        "gatech" | "natech" => ("Tech Center", 1500, 700, "wood", 0.0, 30.0, if key == "gatech" { "GDI" } else { "NOD" }, ""),
        "gahpad" | "nahpad" => ("Helipad", 1000, 600, "wood", 0.0, 18.0, if key == "gahpad" { "GDI" } else { "NOD" }, ""),
        "gadept" => ("Service Depot", 1200, 700, "wood", 0.0, 18.0, "GDI", ""),
        "gactwr" => ("Component Tower", 800, 400, "concrete", 0.0, 30.0, "GDI", "mgun"),
        "naobel" => ("Obelisk of Light", 1500, 600, "concrete", 0.0, 36.0, "NOD", "laser"),
        "nalasr" => ("Laser Turret", 500, 400, "concrete", 0.0, 24.0, "NOD", "laser_light"),
        "nasam" => ("SAM Site", 700, 400, "heavy", 0.0, 30.0, "NOD", "nike"),
        "natmpl" => ("Temple of Nod", 3000, 1200, "concrete", 0.0, 30.0, "NOD", ""),
        "gawall" | "nawall" => ("Wall", 60, 80, "concrete", 0.0, 0.0, if key == "gawall" { "GDI" } else { "NOD" }, ""),
        _ if key.starts_with("civ") => ("Civilian", 0, 50, "none", 2.0, 9.0, "Neutral", ""),
        _ => ("Tiberian Sun Asset", 0, 100, "none", 0.0, 12.0, "GDI,NOD", ""),
    };
    ManualStats {
        title: values.0,
        cost: values.1,
        hp: values.2,
        armor: values.3,
        speed: values.4,
        sight: values.5,
        sides: values.6,
        weapon: values.7,
    }
}

fn rules_unit<'a>(rules: &'a Rules, name: &str) -> Option<&'a UnitRules> {
    rules
        .units
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, unit)| unit)
}

fn rules_weapon<'a>(rules: &'a Rules, name: &str) -> Option<&'a crate::cnc_import::rules::WeaponRules> {
    rules
        .weapons
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, weapon)| weapon)
}

fn weapon_manifest_line(name: &str, rules: &Rules) -> String {
    let id = sanitize_id(name);
    if let Some(weapon) = rules_weapon(rules, name) {
        let rate = if weapon.rof > 0 { 60.0 / weapon.rof as f32 } else { 1.0 };
        let range = (weapon.range * CELL_M).max(0.0);
        let hitscan = weapon.projectile.eq_ignore_ascii_case("Invisible")
            || id.contains("rail")
            || id.contains("laser");
        let mut line = format!(
            "weapon id={id} damage={} rate={rate:.3} range={range:.2} delivery={} projectile_speed={}",
            weapon.damage,
            if hitscan { "hitscan" } else { "projectile" },
            if hitscan { 0 } else { 18 },
        );
        if let Some((_, warhead)) = rules
            .warheads
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(&weapon.warhead))
        {
            let armor = ["none", "wood", "light", "heavy", "concrete"];
            let versus = armor
                .iter()
                .zip(warhead.verses)
                .map(|(armor, value)| format!("{armor}:{:.2}", value as f32 / 100.0))
                .collect::<Vec<_>>()
                .join(",");
            line.push_str(&format!(" versus={versus}"));
        }
        return line;
    }
    match id.as_str() {
        "railgun" => "weapon id=railgun damage=200 rate=0.5 range=36 delivery=hitscan projectile_speed=0".into(),
        "laser" => "weapon id=laser damage=200 rate=0.4 range=36 delivery=hitscan projectile_speed=0".into(),
        "laser_light" => "weapon id=laser_light damage=60 rate=1.0 range=24 delivery=hitscan projectile_speed=0".into(),
        "disc" => "weapon id=disc damage=50 rate=0.75 range=18 delivery=projectile projectile_speed=12 splash_radius=3 splash_damage=25".into(),
        "mgun" | "m1carbine" => format!("weapon id={id} damage=15 rate=5.0 range=18 delivery=hitscan projectile_speed=0"),
        _ => format!("weapon id={id} damage=30 rate=1.0 range=18 delivery=projectile projectile_speed=18"),
    }
}

fn canonical_sides(owners: &[String]) -> String {
    owners
        .iter()
        .filter_map(|owner| {
            if owner.eq_ignore_ascii_case("GDI") {
                Some("GDI")
            } else if owner.eq_ignore_ascii_case("Nod") {
                Some("NOD")
            } else if owner.eq_ignore_ascii_case("Neutral") {
                Some("Neutral")
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",")
}

fn prerequisite_role(name: &str) -> Option<&'static str> {
    let upper = name.trim().to_ascii_uppercase();
    role_for(ROLE_TABLE, &upper).or_else(|| Some(match upper.as_str() {
        "BARRACKS" | "GAPILE" | "NAHAND" | "PILE" | "HAND" => "barracks",
        "FACTORY" | "GAWEAP" | "NAWEAP" | "WEAP" => "vehicle_factory",
        "PROC" | "GAREFN" | "NAREFN" | "REFN" => "refinery",
        "GACNST" | "NACNST" | "CNST" => "conyard",
        "GAPOWR" | "NAPOWR" | "POWR" => "power",
        "GARADR" | "NARADR" | "RADR" => "radar",
        "GATECH" | "NATECH" | "TECH" => "tech",
        "GAHPAD" | "NAHPAD" | "HPAD" => "aircraft_pad",
        _ => return None,
    }))
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| if character.is_ascii_alphanumeric() || character == '_' { character } else { '_' })
        .collect()
}

fn unit_flags(key: &str) -> &'static str {
    match key {
        "engineer" => " capture=1",
        "harv" => " harvester=1 capacity=700",
        "mcv" => " mcv=1 deploys=conyard",
        "hover" => " crushes=0",
        "garefn" | "narefn" => " power=-30 refinery=1",
        "gapowr" | "napowr" => " power=+100",
        "gasilo" | "nasilo" => " power=-10",
        "gapile" | "nahand" | "gactwr" | "nasam" => " power=-20",
        "gaweap" | "naweap" | "gadept" | "nalasr" => " power=-30",
        "garadr" | "naradr" => " power=-40",
        "gatech" | "natech" => " power=-100",
        "gahpad" | "nahpad" => " power=-10",
        "naobel" => " power=-150",
        "natmpl" => " power=-200",
        _ => "",
    }
}

fn emit_theater_scenery(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    theater: &TheaterBank,
    unit_palette: &Pal,
    report: &mut ConvertReport,
) -> Result<(), String> {
    let mut tiberium = Vec::new();
    for stage in 1..=19u8 {
        let stem = format!("tib{stage:02}");
        let Some(shp) = theater.overlay(&stem) else {
            report.unresolved_shapes.insert(format!("{stem}.TEM"));
            continue;
        };
        if let Some(frame) = sprite_frame(shp, 0, &theater.palette, 0) {
            tiberium.push(frame);
        }
    }
    if !tiberium.is_empty() {
        let count = tiberium.len();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: "tib".into(),
                role: "resource",
                facings: 1,
                frames: tiberium,
                states: vec![SpriteState { name: "idle", first: 0, last: count, looping: false, fps: 1 }],
                unit: Some(UnitSpec {
                    manifest_lines: vec!["unit class=resource title=\"Tiberium\"".into()],
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ts", "resource", "tiberium"],
            },
        )?;
        report.resolved_shapes.insert(format!("tib <- TIB01..TIB{count:02}.TEM"));
    }

    for number in 1..=9u8 {
        let key = format!("tree{number:02}");
        let Some(shp) = theater.overlay(&key) else {
            report.unresolved_shapes.insert(format!("{}.TEM", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len().min(2);
        let frames = (0..count)
            .filter_map(|source| sprite_frame(shp, source, &theater.palette, 0))
            .collect::<Vec<_>>();
        if frames.is_empty() {
            continue;
        }
        let frame_count = frames.len();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.clone(),
                role: "scenery",
                facings: 1,
                frames,
                states: vec![SpriteState { name: "idle", first: 0, last: frame_count, looping: false, fps: 6 }],
                unit: Some(UnitSpec {
                    manifest_lines: vec![format!("unit class=scenery title=\"Tree {number}\"")],
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ts", "scenery", "tree"],
            },
        )?;
        report.resolved_shapes.insert(format!("{key} <- {}.TEM", key.to_ascii_uppercase()));
    }

    for number in 1..=4u8 {
        let key = format!("aban{number:02}");
        let Some(shp) = conquer_shp(archives, &key) else {
            report.unresolved_shapes.insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len().min(4);
        let frames = (0..count)
            .filter_map(|source| sprite_frame(&shp, source, unit_palette, 0))
            .collect::<Vec<_>>();
        if frames.is_empty() {
            continue;
        }
        let frame_count = frames.len();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.clone(),
                role: "scenery",
                facings: 1,
                frames,
                states: vec![SpriteState { name: "idle", first: 0, last: frame_count, looping: true, fps: 4 }],
                unit: Some(UnitSpec {
                    manifest_lines: vec![format!("unit class=scenery title=\"Civilian Ruin {number}\"")],
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ts", "scenery", "ruin"],
            },
        )?;
        report.resolved_shapes.insert(format!("{key} <- {}.SHP", key.to_ascii_uppercase()));
    }
    Ok(())
}

fn emit_effects(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    report: &mut ConvertReport,
) -> Result<(), String> {
    for &key in EFFECTS {
        let Some(shp) = conquer_shp(archives, key) else {
            report.unresolved_shapes.insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len().min(96);
        let frames = (0..count)
            .filter_map(|source| sprite_frame(&shp, source, palette, 0))
            .collect::<Vec<_>>();
        if frames.is_empty() {
            continue;
        }
        let frame_count = frames.len();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "effect",
                facings: 1,
                frames,
                states: vec![SpriteState { name: "idle", first: 0, last: frame_count, looping: false, fps: 15 }],
                unit: None,
                manifest_lines: Vec::new(),
                tags: vec!["ts", "effect"],
            },
        )?;
        report.resolved_shapes.insert(format!("{key} <- {}.SHP", key.to_ascii_uppercase()));
    }
    Ok(())
}

fn emit_icon(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    key: &str,
    report: &mut ConvertReport,
) -> Result<(), String> {
    let Some(stem) = icon_stem(key) else { return Ok(()) };
    let Some(shp) = conquer_shp(archives, stem) else { return Ok(()) };
    let Some(frame) = shp.frames().first() else { return Ok(()) };
    let (width, height) = shp.canvas();
    if width == 0 || height == 0 {
        return Ok(());
    }
    emitter.emit_texture(
        &format!("icons/ts/{key}"),
        &indexed_transparent(&frame.pixels, palette),
        u32::from(width),
        u32::from(height),
        &["ts", "icon"],
    )?;
    report.resolved_icons.insert(format!("{key} <- {stem}.SHP"));
    Ok(())
}

fn icon_stem(key: &str) -> Option<&'static str> {
    Some(match key {
        "titan" => "MMCHICON",
        "wolverine" => "SMCHICON",
        "e1" => "E1ICON",
        "e2" => "E2ICON",
        "engineer" => "ENGNICON",
        "ghost" => "GOSTICON",
        "jumpjet" => "JJETICON",
        "cyborg" => "CYBCICON",
        "umagon" => "UMAGICON",
        "medic" => "MEDIICON",
        "gacnst" | "nacnst" => "FACTICON",
        "gapowr" => "POWRICON",
        "napowr" => "NPWRICON",
        "garefn" | "narefn" => "REFICON",
        "gasilo" | "nasilo" => "SILOICON",
        "gapile" => "BRRKICON",
        "nahand" => "HANDICON",
        "gaweap" | "naweap" => "WEAPICON",
        "garadr" | "naradr" => "RADRICON",
        "gatech" => "TECHICON",
        "natech" => "NTCHICON",
        "gahpad" => "HELIICON",
        "nahpad" => "NHPDICON",
        "gadept" => "FIXICON",
        "gactwr" => "TOWRICON",
        "naobel" => "OBLIICON",
        "nalasr" => "LASRICON",
        "nasam" => "SAMICON",
        "natmpl" => "TMPLICON",
        "gaplug" => "PLUGICON",
        "napuls" => "PULSICON",
        "gawall" | "nawall" => "WALLICON",
        _ => return None,
    })
}

fn infer_remap_start(conquer: &[u8]) -> Option<u8> {
    let shapes = ["MMCH", "SMECH", "E1", "CYBORG"]
        .into_iter()
        .filter_map(|stem| ShpTs::parse(mix_entry(conquer, &format!("{stem}.SHP"))?).ok())
        .collect::<Vec<_>>();
    if shapes.len() < 3 {
        return None;
    }
    let count_range = |shp: &ShpTs, start: u8| {
        shp.frames()
            .iter()
            .flat_map(|frame| frame.pixels.iter())
            .filter(|&&index| index >= start && index < start.saturating_add(16))
            .count()
    };
    if shapes.iter().all(|shp| count_range(shp, 16) > 0) {
        return Some(16);
    }
    (1..=239u8)
        .map(|start| {
            let score = shapes.iter().map(|shp| count_range(shp, start)).min().unwrap_or(0);
            (score, start)
        })
        .max()
        .filter(|(score, _)| *score > 0)
        .map(|(_, start)| start)
}

fn remap_report(conquer: &[u8], start: u8) -> String {
    let counts = ["MMCH", "SMECH", "E1", "CYBORG"]
        .into_iter()
        .map(|stem| {
            let count = ShpTs::parse(mix_entry(conquer, &format!("{stem}.SHP")).unwrap_or_default())
                .ok()
                .map(|shp| {
                    shp.frames()
                        .iter()
                        .flat_map(|frame| frame.pixels.iter())
                        .filter(|&&index| index >= start && index < start.saturating_add(16))
                        .count()
                })
                .unwrap_or(0);
            format!("{stem}={count}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("histogram intersection selected indices {start}..{}; occurrences {counts}", start + 15)
}

fn remap_line(palette: &Pal, start: u8) -> String {
    let colors = (start..start.saturating_add(16))
        .map(|index| {
            let [r, g, b] = palette.rgb(index);
            format!("{r},{g},{b}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("remap {colors}")
}

fn resolved_audio(archives: &Archives, names: &NameTable) -> BTreeSet<String> {
    let mut output = BTreeSet::new();
    for bytes in [&archives.sounds, &archives.speech01, &archives.speech02] {
        let Ok(mix) = MixFile::parse(bytes) else { continue };
        for (id, name) in names.resolve_names(&mix) {
            let Some(stem) = name
                .filter(|name| name.to_ascii_uppercase().ends_with(".AUD"))
                .map(|name| sanitize_id(&name[..name.len().saturating_sub(4)]))
            else {
                continue;
            };
            if mix.by_id(id).is_some_and(|data| Aud::parse(data).is_ok()) {
                output.insert(stem);
            }
        }
    }
    output
}

fn emit_audio(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    names: &NameTable,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let resolved = resolved_audio(archives, names);
    let mut emitted = BTreeSet::new();
    let total = resolved.len().max(1);
    for (archive_index, bytes) in [&archives.sounds, &archives.speech01, &archives.speech02]
        .into_iter()
        .enumerate()
    {
        let mix = MixFile::parse(bytes).map_err(|error| error.to_string())?;
        for (id, name) in names.resolve_names(&mix) {
            let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(".AUD")) else {
                continue;
            };
            let stem = sanitize_id(&name[..name.len().saturating_sub(4)]);
            if !resolved.contains(&stem) || !emitted.insert(stem.clone()) {
                continue;
            }
            let Some(data) = mix.by_id(id) else { continue };
            let Ok(audio) = Aud::parse(data) else { continue };
            tick(
                on_tick,
                emitted.len() - 1,
                total,
                format!("sfx {stem}"),
                None,
            );
            emitter.emit_sfx(
                &format!("sfx/ts/{stem}"),
                audio.sample_rate(),
                audio.channels(),
                audio.samples(),
                if archive_index == 0 { &["ts", "sfx"] } else { &["ts", "speech"] },
            )?;
            report.sound_stems.insert(stem);
        }
    }
    Ok(())
}

fn audit_requested_mobile_shapes(archives: &Archives, report: &mut ConvertReport) {
    const REQUESTED: &[(&str, &str)] = &[
        ("titan", "MMCH"), ("wolverine", "SMECH"), ("hover", "HVR"),
        ("mlrs", "MLRS"), ("hmec", "HMEC"), ("smech", "SMECH"),
        ("sonic", "SONIC"), ("disr", "DISR"), ("jugg", "JUGG"),
        ("apc", "APC"), ("harv", "HARV"), ("mcv", "MCV"),
        ("art2", "ART2"), ("stnk", "STNK"), ("bggy", "BGGY"),
        ("bike", "BIKE"), ("ttnk", "TTNK"), ("sapc", "SAPC"),
        ("weed", "WEED"), ("subt", "SUBT"), ("lasr", "LASR"),
        ("repair", "REPAIR"), ("orca", "ORCA"), ("orcab", "ORCAB"),
        ("orcatran", "ORCATRAN"), ("dshp", "DSHP"), ("scrin", "SCRIN"),
        ("apache", "APACHE"), ("harpy", "HARPY"),
    ];
    for &(key, stem) in REQUESTED {
        let label = format!("requested {key} -> {stem}.SHP");
        if conquer_shp(archives, stem).is_some() {
            report.resolved_shapes.insert(label);
        } else {
            report.unresolved_shapes.insert(label);
        }
    }
}

fn palette(archive: &[u8], name: &str) -> Result<Pal, String> {
    let bytes = mix_entry(archive, name).ok_or_else(|| format!("cache.mix has no {name}"))?;
    Pal::parse(bytes).map_err(|error| format!("{name}: {error}"))
}

fn mix_entry<'a>(archive: &'a [u8], name: &str) -> Option<&'a [u8]> {
    MixFile::parse(archive).ok()?.by_name_with_hash(name, HASH)
}

fn read_pack_file(pack: &Path, wanted: &str) -> Result<Vec<u8>, String> {
    let mut stack = vec![(pack.to_path_buf(), 0usize)];
    let mut matches = Vec::new();
    while let Some((directory, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("read {}: {error}", directory.display()))?;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                stack.push((path, depth + 1));
            } else if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
            {
                matches.push(path);
            }
        }
    }
    matches.sort();
    let path = matches
        .into_iter()
        .next()
        .ok_or_else(|| format!("pack has no {wanted}"))?;
    std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn tick(
    on_tick: &mut dyn FnMut(ConvertTick),
    done: usize,
    total: usize,
    current: String,
    preview_png: Option<Vec<u8>>,
) {
    on_tick(ConvertTick {
        stage: ConvertStage::Convert,
        done,
        total: total.max(1),
        current,
        preview_png,
    });
}

fn write_report(report: &ConvertReport) -> Result<(), String> {
    let mut text = String::from(
        "# Tiberian Sun staged conversion report\n\n## Three-line summary\n\n",
    );
    text.push_str("Four deterministic 48×48 isometric maps are rendered onto the original screen plane, then sampled into 96×48 flat 3 m contract grids.\n");
    text.push_str("TS SHP infantry, walkers, multipart structures, icons, tiberium, trees, effects, and every dictionary-resolved AUD are emitted under the `ts` namespace.\n");
    text.push_str("RULES.INI and ART.INI drive stats and infantry clips; unresolved voxel-only vehicles and unresolved speech hashes are reported rather than guessed.\n");
    text.push_str("\n## Generated worlds\n\n| Key | Theater | Iso grid | Flat grid | Starts | Tiberium | Trees | Ruins |\n|---|---|---:|---:|---:|---:|---:|---:|\n");
    for world in &report.worlds {
        text.push_str(&format!(
            "| {} | {} | 48×48 | 96×48 | {} | {} | {} | {} |\n",
            world.key, world.theater, world.starts, world.resources, world.trees, world.ruins,
        ));
    }
    text.push_str("\nThe world plane is the source screen plane: `sx=(x-y)*24+W/2`, `sy=(x+y)*12-height*12`; each flat cell is 24 px at 0.125 m/px (`cell 3.0`). Outside the isometric diamond is opaque `#0a0a0a` and blocked as `#`; the preview keeps the diamond's 2:1 bounding-box aspect without square letterboxing. Genuine `CLEAR*.TEM` slots supply clear ground, while cliff, road, and water pieces are selected by exact N/E/S/W neighbour mask. Cliff and water diamonds are impassable; roads are `r`, rough is `b`, and every `t` cell has a `billboards/ts/tib` resource row. Seeds 1–3 use temperate art and seed 4 snow.\n");
    text.push_str("\n## Terrain classification evidence\n\nNames classify only `clear*` as clear, plus `cliff*`/`ramp*`, `water*`/`shore*`, `*road*`/`pave*`, and `rough*`/`ruff*`. `GREEN*.TEM` and `SNOW*.TEM` are transition templates, not interchangeable clear cells. Empty TMP slots and all-zero decoded slots are skipped. The raw per-tile `terrain_type` histograms retained for the named groups are:\n\n");
    for line in &report.terrain_histograms {
        text.push_str(&format!("- {line}\n"));
    }
    text.push_str("\n## Rules and sprite layout\n\n");
    text.push_str(&format!("- {}.\n", report.rules_note));
    text.push_str(&format!("- Remap ramp: {}.\n", report.remap_note));
    text.push_str(&format!("- Vehicle direction: {}.\n", report.vehicle_direction));
    for layout in &report.infantry_layouts {
        text.push_str(&format!("- Infantry {layout}.\n"));
    }
    for layout in &report.structure_layouts {
        text.push_str(&format!("- Structure {layout}.\n"));
    }
    text.push_str("\n## Emitted sprite roles\n\n| Role | Count |\n|---|---:|\n");
    for (role, count) in &report.roles {
        text.push_str(&format!("| {role} | {count} |\n"));
    }
    text.push_str("\n## Resolved shapes\n\n");
    for value in &report.resolved_shapes {
        text.push_str(&format!("- `{value}`\n"));
    }
    text.push_str("\n## Unresolved requested or optional shapes\n\n");
    for value in &report.unresolved_shapes {
        text.push_str(&format!("- `{value}`\n"));
    }
    text.push_str("\n## Icons\n\n");
    for value in &report.resolved_icons {
        text.push_str(&format!("- `{value}`\n"));
    }
    text.push_str("\n## Resolved audio stems\n\n");
    if report.sound_stems.is_empty() {
        text.push_str("None. The supplied speech MIX hashes do not resolve through the dictionary.\n");
    } else {
        text.push_str(&report.sound_stems.iter().cloned().collect::<Vec<_>>().join(", "));
        text.push('\n');
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../cnc-ts-convert-report.md");
    std::fs::write(&path, text).map_err(|error| format!("write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cnc_import_ts_iso_to_screen_mapping_is_pinned() {
        assert_eq!(iso_to_screen(0, 0, 2304, 0), (1152, 0));
        assert_eq!(iso_to_screen(1, 0, 2304, 0), (1176, 12));
        assert_eq!(iso_to_screen(0, 1, 2304, 0), (1128, 12));
        assert_eq!(iso_to_screen(3, 2, 2304, 2), (1176, 36));
    }

    #[test]
    fn cnc_import_ts_two_by_two_diamond_raster_is_expected_square_cells() {
        let cell = |class| IsoCell {
            height: 0,
            class,
            kind: TerrainKind::Clear,
            variant: 0,
        };
        let map = GeneratedIsoMap {
            cells: vec![cell('a'), cell('b'), cell('c'), cell('d')],
            starts: Vec::new(),
            resource_stages: vec![None; 4],
            trees: Vec::new(),
            ruins: Vec::new(),
        };
        assert_eq!(
            rasterize_classes_sized(&map, 2, 2),
            vec!["#aa#".to_string(), "#dd#".to_string()]
        );
    }

    #[test]
    fn cnc_import_ts_generator_is_deterministic() {
        for seed in 1..=4 {
            let first = generate_map(seed);
            let second = generate_map(seed);
            assert_eq!(first, second);
            assert_eq!(first.cells.len(), ISO_SIZE * ISO_SIZE);
            assert_eq!(first.starts.len(), 2 + (seed as usize - 1) % 3);
            assert!(first.resource_stages.iter().flatten().count() >= 20);
            assert!(first.cells.iter().any(|cell| cell.height > 0));
            assert!(first.cells.iter().any(|cell| cell.class == 'r'));
            assert!(first.cells.iter().any(|cell| cell.class == 'w'));
            assert!(first.cells.iter().any(|cell| cell.class == 'b'));
        }
    }

    #[test]
    fn cnc_import_ts_directional_tiles_follow_four_neighbour_mask() {
        let tile = |name: &str, neighbour_mask| TerrainTile {
            name: name.into(),
            pixels: vec![1],
            terrain_type: 0,
            raw_height: 0,
            neighbour_mask,
            extra: None,
        };
        let candidates = vec![
            tile("north-edge", MASK_E | MASK_S | MASK_W),
            tile("interior", MASK_N | MASK_E | MASK_S | MASK_W),
        ];
        assert_eq!(
            choose_masked_tile(&candidates, MASK_E | MASK_S | MASK_W, 99)
                .map(|tile| tile.name.as_str()),
            Some("north-edge")
        );

        let clear = IsoCell {
            height: 0,
            class: '.',
            kind: TerrainKind::Clear,
            variant: 0,
        };
        let cliff = IsoCell { kind: TerrainKind::Cliff, class: '#', ..clear };
        let mut cells = vec![clear; ISO_SIZE * ISO_SIZE];
        let (x, y) = (20, 20);
        for (nx, ny) in [(x, y), (x, y - 1), (x + 1, y), (x, y + 1), (x - 1, y)] {
            cells[ny * ISO_SIZE + nx] = cliff;
        }
        let map = GeneratedIsoMap {
            cells,
            starts: Vec::new(),
            resource_stages: vec![None; ISO_SIZE * ISO_SIZE],
            trees: Vec::new(),
            ruins: Vec::new(),
        };
        assert_eq!(
            terrain_neighbour_mask(&map, x, y, TerrainKind::Cliff),
            MASK_N | MASK_E | MASK_S | MASK_W
        );
    }

    #[test]
    fn cnc_import_ts_clear_bank_excludes_snow_transition_templates() {
        assert_eq!(terrain_kind("clear01"), Some(TerrainKind::Clear));
        assert_eq!(terrain_kind("snow01"), None);
        assert_eq!(terrain_kind("green01"), None);
    }

    #[test]
    fn cnc_import_ts_remap_line_has_sixteen_entries() {
        let mut bytes = [0u8; 768];
        for index in 16..32 {
            bytes[index * 3..index * 3 + 3].copy_from_slice(&[63, 32, 1]);
        }
        let line = remap_line(&Pal::parse(&bytes).unwrap(), 16);
        assert_eq!(line.split_whitespace().count(), 17);
    }

    #[test]
    #[ignore]
    fn convert_local_ts_pack_if_present() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let pack = manifest.join("../../../local/packs/ts");
        if !pack.join("conquer.mix").is_file() {
            return;
        }
        let staged = std::env::temp_dir().join(format!(
            "makepad-ts-convert-{}",
            std::process::id()
        ));
        if staged.is_dir() {
            std::fs::remove_dir_all(&staged).expect("clear prior TS staging");
        }
        let assets = convert(&pack, &staged, &mut |_| {}).expect("convert local TS pack");
        let worlds = assets
            .iter()
            .filter(|asset| asset.kind == makepad_asset_data::AssetKind::World)
            .collect::<Vec<_>>();
        assert_eq!(worlds.len(), 4);
        for world in worlds {
            let glb = staged.join(&world.rel_path);
            for extension in ["glb", "place", "grid", "png"] {
                assert!(
                    glb.with_extension(extension).is_file(),
                    "{}",
                    glb.with_extension(extension).display()
                );
            }
            let place = std::fs::read_to_string(glb.with_extension("place")).unwrap();
            assert!(place.contains("\ncell 3.0\n"));
            assert!(place.contains("\ntile 24\n"));
            let roster_keys = place
                .lines()
                .filter_map(|line| line.strip_prefix("roster "))
                .flat_map(str::split_whitespace)
                .count();
            assert!(roster_keys >= 20, "roster keys={roster_keys}");
            let grid = std::fs::read_to_string(glb.with_extension("grid")).unwrap();
            assert!(grid.contains("\ncell 3.0\n"));
            let height = grid
                .lines()
                .find_map(|line| line.strip_prefix("size "))
                .and_then(|size| size.split_whitespace().nth(1))
                .and_then(|height| height.parse::<usize>().ok())
                .unwrap();
            assert_eq!(grid.lines().filter(|line| line.starts_with("row ")).count(), height);
        }
        let read = |relative: &str| std::fs::read_to_string(staged.join(relative)).unwrap();
        let tank = read("billboards/ts/titan.billboard");
        assert!(tank.contains("facings 32"));
        assert_eq!(tank.lines().filter(|line| line.starts_with("frame ")).count(), 32);
        let infantry = read("billboards/ts/e1.billboard");
        for state in ["idle", "walk", "fire", "die"] {
            assert!(infantry.contains(&format!("state {state} ")));
        }
        let conyard = read("billboards/ts/gacnst.billboard");
        assert!(conyard.contains("footprint 3 3"));
        let tiberium = read("billboards/ts/tib.billboard");
        assert!(tiberium.lines().filter(|line| line.starts_with("frame ")).count() >= 12);
        for asset in &assets {
            if asset.kind == makepad_asset_data::AssetKind::Billboard {
                let text = std::fs::read_to_string(staged.join(&asset.rel_path)).unwrap();
                assert!(!text.contains("producer="), "{}", asset.rel_path);
            }
        }
        std::fs::remove_dir_all(&staged).expect("remove TS staging");
    }
}
