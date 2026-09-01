//! D2K loose-pack interpretation and deterministic Arrakis generation.

use super::{
    positive_unit_cost, rewrite_unit_roles, roster_key, PreviewDot, RoleTable, RtsEmitter,
    SpritePixels, SpriteSpec, SpriteState, UnitSpec, WorldSpec, CELL_M,
};
use crate::classic_import::{ClassicAsset, ConvertStage, ConvertTick};
use crate::cnc_import::{
    aud::Aud,
    d2k_tiles::{D2kTemplate, D2kTemplateTable},
    pal::Pal,
    r8::R8,
    shp::Shp,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const TILE_PX: u32 = 32;
const METRES_PER_PIXEL: f32 = 0.1875;
const MAP_SIZE: usize = 64;
const MASK_N: u8 = 1;
const MASK_E: u8 = 2;
const MASK_S: u8 = 4;
const MASK_W: u8 = 8;

const VEHICLES: &[(&str, &str, &str)] = &[
    ("trike", "trike", "trikeicon"),
    ("raider", "raider", "raidericon"),
    ("quad", "quad", "quadicon"),
    ("combat-a", "combata", "combataicon"),
    ("combat-h", "combath", "combathicon"),
    ("combat-o", "combato", "combatoicon"),
    ("siege", "siegetank", "siegetankicon"),
    ("missile", "missiletank", "missiletankicon"),
    ("sonic", "sonictank", "sonictankicon"),
    ("devastator", "devast", "devasticon"),
    ("deviator", "deviatortank", "deviatortankicon"),
    ("harvester", "harvester", "harvestericon"),
    ("mcv", "dmcv", "mcvicon"),
    ("carryall", "carryall", "carryallicon"),
    ("ornithopter", "orni", "orniicon"),
];

const VEHICLE_TURRETS: &[(&str, &str)] = &[
    ("combat-a-turret", "combataturret"),
    ("combat-h-turret", "combathturret"),
    ("combat-o-turret", "combatoturret"),
    ("siege-turret", "siegeturret"),
    ("missile-turret", "missileturret"),
];

const INFANTRY: &[(&str, &str)] = &[
    ("rifle", "rifle"),
    ("bazooka", "bazooka"),
    ("engineer", "engineer"),
    ("sardaukar", "sardaukar"),
    ("fremen", "fremen"),
    ("saboteur", "saboteur"),
    ("thumper", "thumper"),
];

#[derive(Clone, Copy)]
struct StructureDef {
    key: &'static str,
    stem: &'static str,
    make: Option<&'static str>,
}

const STRUCTURES: &[StructureDef] = &[
    StructureDef { key: "conyard", stem: "conyard", make: Some("conmake") },
    StructureDef { key: "pwr", stem: "pwr", make: Some("wtrpmake") },
    StructureDef { key: "ref", stem: "ref", make: Some("refmake") },
    StructureDef { key: "silo", stem: "silo", make: Some("silomake") },
    StructureDef { key: "barracks", stem: "barr", make: Some("barramake") },
    StructureDef { key: "light", stem: "light", make: Some("lightmake") },
    StructureDef { key: "heavy", stem: "heavy", make: Some("heavymake") },
    StructureDef { key: "hightech", stem: "hightech", make: Some("highmake") },
    StructureDef { key: "research", stem: "research", make: Some("researchmake") },
    StructureDef { key: "outpost", stem: "radar", make: Some("radarmake") },
    StructureDef { key: "repair", stem: "repair", make: Some("repairmake") },
    StructureDef { key: "starport", stem: "starport", make: Some("starportmake") },
    StructureDef { key: "palace", stem: "palace", make: Some("palacemake") },
    StructureDef { key: "guntower", stem: "guntower", make: None },
    StructureDef { key: "rocketturret", stem: "rockettower", make: None },
    StructureDef { key: "wall", stem: "wall", make: None },
];

pub(super) const ROLE_TABLE: RoleTable = &[
    ("conyard", "conyard"),
    ("pwr", "power"),
    ("ref", "refinery"),
    ("silo", "silo"),
    ("barracks", "barracks"),
    ("light", "vehicle_factory"),
    ("heavy", "vehicle_factory"),
    ("hightech", "aircraft_pad"),
    ("outpost", "radar"),
    ("research", "tech"),
    ("palace", "tech"),
    ("starport", "vehicle_factory"),
    ("repair", "repair"),
    ("guntower", "defense"),
    ("rocketturret", "defense"),
    ("wall", "wall"),
];

const MISC_SHAPES: &[(&str, &str)] = &[
    ("crates", "scenery"),
    ("dots", "effect"),
    ("plates", "scenery"),
    ("numbers", "effect"),
    ("overlay", "effect"),
    ("rockcrater1", "scenery"),
    ("rockcrater2", "scenery"),
    ("sandcrater1", "scenery"),
    ("sandcrater2", "scenery"),
    ("spicebloom", "effect"),
    ("stars", "effect"),
    ("missile_launch", "effect"),
    ("deathhandmissile", "effect"),
    ("greenuparrow", "effect"),
    ("wormdust", "effect"),
    ("wormjaw", "effect"),
    ("wormsigns1", "scenery"),
    ("wormsigns2", "scenery"),
    ("wormsigns3", "scenery"),
    ("wormsigns4", "scenery"),
    ("unload", "effect"),
    ("repairing", "effect"),
    ("windtrap_anim", "effect"),
    ("sietch", "scenery"),
    ("frigate", "scenery"),
];

#[derive(Default)]
struct ConvertReport {
    data_root: String,
    tile_banks: Vec<String>,
    worlds: Vec<WorldSummary>,
    roles: BTreeMap<String, usize>,
    sprite_keys: BTreeSet<String>,
    icon_keys: BTreeSet<String>,
    missing_shapes: BTreeSet<String>,
    shape_errors: Vec<String>,
    structure_halves: Vec<String>,
    infantry_layouts: Vec<String>,
    audio_keys: BTreeSet<String>,
    audio_errors: Vec<String>,
    named_sprites: bool,
    spice_note: String,
}

#[derive(Clone)]
struct WorldSummary {
    key: String,
    starts: usize,
    resources: usize,
    multi_templates: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GeneratedMap {
    visual_classes: Vec<u8>,
    grid: Vec<u8>,
    stages: Vec<Option<u8>>,
    starts: Vec<(usize, usize)>,
    blooms: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TileChoice {
    image: String,
    frame: u16,
}

pub fn convert(
    pack_dir: &Path,
    staged: &Path,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<Vec<ClassicAsset>, String> {
    let data_root = detect_data_root(pack_dir)?;
    let palette = Pal::parse(&read_named(&data_root, "PALETTE.BIN")?)
        .map_err(|error| format!("PALETTE.BIN: {error}"))?;
    let table = D2kTemplateTable::embedded().map_err(|error| error.to_string())?;
    let (tile_banks, tile_bank_names) = load_tile_banks(&data_root, &table)?;
    let shape_files = find_named_dir(pack_dir, &data_root, "SHPs")
        .map(|directory| files_by_stem(&directory, "shp"))
        .transpose()?
        .unwrap_or_default();
    let mut report = ConvertReport {
        data_root: data_root
            .strip_prefix(pack_dir)
            .ok()
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| ".".into()),
        tile_banks: tile_bank_names,
        named_sprites: !shape_files.is_empty(),
        ..Default::default()
    };
    if shape_files.is_empty() {
        report.spice_note = "no named sprites; spice uses repeated T-tile art".into();
    }

    let mut emitter = RtsEmitter::new_scaled(staged, "d2k", TILE_PX, METRES_PER_PIXEL)?;
    let available_banks = tile_banks.keys().cloned().collect::<BTreeSet<_>>();
    for seed in 1..=4u32 {
        tick(on_tick, seed as usize - 1, 4, format!("world arrakis-{seed}"), None);
        let generated = generate_map(seed);
        let (choices, multi_templates) = paint_templates(
            &generated.visual_classes,
            &table,
            &available_banks,
            seed ^ 0xa771_5eed,
        )?;
        let terrain_rgba = bake_terrain(&choices, &tile_banks, &palette);
        let world = generated_world(seed, &generated, terrain_rgba, shape_files.contains_key("spicebloom"));
        let preview = emitter.emit_world(world)?;
        report.worlds.push(WorldSummary {
            key: format!("arrakis-{seed}"),
            starts: generated.starts.len(),
            resources: generated.stages.iter().flatten().count(),
            multi_templates,
        });
        tick(on_tick, seed as usize, 4, format!("world arrakis-{seed}"), Some(preview));
    }

    let audio = emit_audio(&mut emitter, pack_dir, &data_root, &mut report, on_tick)?;
    emit_spice(
        &mut emitter,
        &shape_files,
        &tile_banks,
        &palette,
        &mut report,
    )?;
    if !shape_files.is_empty() {
        emit_named_sprites(
            &mut emitter,
            &shape_files,
            &palette,
            &audio,
            &mut report,
            on_tick,
        )?;
    }
    write_report(&report)?;
    Ok(emitter.finish())
}

fn detect_data_root(pack: &Path) -> Result<PathBuf, String> {
    if named_child(pack, "PALETTE.BIN").is_some() {
        return Ok(pack.to_path_buf());
    }
    let v2 = named_child(pack, "v2").filter(|path| path.is_dir());
    if let Some(v2) = v2 {
        if named_child(&v2, "PALETTE.BIN").is_some() {
            return Ok(v2);
        }
    }
    if let Some(orig) = named_child(pack, "orig").filter(|path| path.is_dir()) {
        if let Some(v2) = named_child(&orig, "v2").filter(|path| path.is_dir()) {
            if named_child(&v2, "PALETTE.BIN").is_some() {
                return Ok(v2);
            }
        }
    }
    Err(format!(
        "{} has neither PALETTE.BIN nor v2/PALETTE.BIN",
        pack.display()
    ))
}

fn find_named_dir(pack: &Path, data_root: &Path, name: &str) -> Option<PathBuf> {
    named_child(pack, name)
        .filter(|path| path.is_dir())
        .or_else(|| named_child(data_root, name).filter(|path| path.is_dir()))
        .or_else(|| data_root.parent().and_then(|parent| named_child(parent, name)).filter(|path| path.is_dir()))
}

fn named_child(directory: &Path, wanted: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
        {
            return Some(entry.path());
        }
    }
    None
}

fn read_named(directory: &Path, wanted: &str) -> Result<Vec<u8>, String> {
    let path = named_child(directory, wanted)
        .filter(|path| path.is_file())
        .ok_or_else(|| format!("{} has no {wanted}", directory.display()))?;
    std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn files_by_stem(directory: &Path, extension: &str) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut files = BTreeMap::new();
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_file() || kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else { continue };
        files.entry(stem.to_ascii_lowercase()).or_insert(path);
    }
    Ok(files)
}

fn load_tile_banks(
    root: &Path,
    table: &D2kTemplateTable,
) -> Result<(BTreeMap<String, R8>, Vec<String>), String> {
    let mut names = table
        .templates()
        .iter()
        .map(|template| template.image.to_ascii_uppercase())
        .collect::<BTreeSet<_>>();
    for name in [
        "BLOXBASE",
        "BLOXBAT",
        "BLOXBGBS",
        "BLOXICE",
        "BLOXTREE",
        "BLOXWAST",
        "BLOXXMAS",
    ] {
        names.insert(name.into());
    }
    let mut banks = BTreeMap::new();
    let mut loaded = Vec::new();
    for name in names {
        let file = format!("{name}.R8");
        let Ok(bytes) = read_named(root, &file) else { continue };
        let bank = R8::parse(&bytes).map_err(|error| format!("{file}: {error}"))?;
        if bank.entries().is_empty() {
            return Err(format!("{file}: empty image bank"));
        }
        loaded.push(format!("{name} ({})", bank.entries().len()));
        banks.insert(name, bank);
    }
    if !banks.contains_key("BLOXBASE") {
        return Err("pack has no decodable BLOXBASE.R8".into());
    }
    Ok((banks, loaded))
}

/// The map itself comes from the SHARED generator (`makepad_rtsmap`), which
/// the sandbox also calls at runtime — one algorithm, one set of fairness
/// guarantees, one place a bug gets fixed. What is D2K about this function is
/// only the TRANSLATION: the generator's neutral terrain onto the class
/// letters this pack's tile templates are indexed by, and its scenery onto
/// the shapes this pack actually ships.
fn generate_map(seed: u32) -> GeneratedMap {
    // 2..4 houses, the same seed-driven count this converter always used.
    let players = (2 + seed as usize % 3).clamp(2, 4) as u8;
    let mut map = makepad_rtsmap::generate(&makepad_rtsmap::MapSpec {
        seed,
        width: MAP_SIZE as u16,
        height: MAP_SIZE as u16,
        players,
        style: makepad_rtsmap::Style::Desert,
        resources: 0.65,
        cliffs: 0.7,
        water: 0.0,
        roads: 0.0,
        theater: "arrakis".into(),
        retries: 8,
    });
    // This pack draws its obstacles as ROCK TERRAIN, not as scenery sprites:
    // it has no tree or boulder shape to place. Anything it cannot draw is
    // dropped here, grid included, so no cell is blocked by an invisible
    // thing. The spice bloom stays — that one it has.
    map.retain_props(|prop| prop.kind == makepad_rtsmap::PropKind::Bloom);

    let visual_classes = map.terrain.iter().map(|terrain| class_letter(*terrain)).collect();
    let stages = map.stage_grid();
    let starts = map
        .starts
        .iter()
        .map(|start| (start.x as usize, start.y as usize))
        .collect();
    let blooms = map
        .props
        .iter()
        .filter(|prop| prop.kind == makepad_rtsmap::PropKind::Bloom)
        .map(|prop| (prop.x as usize, prop.y as usize))
        .collect();
    GeneratedMap { visual_classes, grid: map.grid, stages, starts, blooms }
}

/// The template table's class vocabulary: `c` sand, `g` dune, `k` rock,
/// `T` spice. Every neutral terrain lands on one of the four.
fn class_letter(terrain: makepad_rtsmap::Terrain) -> u8 {
    use makepad_rtsmap::Terrain;
    match terrain {
        Terrain::Rough | Terrain::Shore => b'g',
        Terrain::Cliff | Terrain::Plateau => b'k',
        Terrain::Resource => b'T',
        Terrain::Clear | Terrain::Road | Terrain::Water => b'c',
    }
}

/// The inverse of `class_letter`, for painting: rock's rim and its top are
/// ONE class as far as this pack's artwork is concerned, so both come back
/// as the same terrain and a rock shelf is edge-matched as one blob.
fn class_terrain(class: u8) -> Option<makepad_rtsmap::Terrain> {
    use makepad_rtsmap::Terrain;
    match class {
        b'c' => Some(Terrain::Clear),
        b'g' => Some(Terrain::Rough),
        b'k' => Some(Terrain::Cliff),
        b'T' => Some(Terrain::Resource),
        _ => None,
    }
}

/// Choose the artwork for every cell through the SHARED picker.
///
/// This function's job is now purely translation: it turns the pack's
/// template table into a `TileSet` of opaque ids and its class letters into
/// neutral terrain, and the picker in `makepad_rtsmap::tiles` does the
/// edge-matching — the same code that paints a map generated at runtime, so
/// a baked world and a live one cannot drift apart.
fn paint_templates(
    classes: &[u8],
    table: &D2kTemplateTable,
    available_banks: &BTreeSet<String>,
    seed: u32,
) -> Result<(Vec<TileChoice>, usize), String> {
    if classes.len() != MAP_SIZE * MAP_SIZE {
        return Err("generated class map has invalid dimensions".into());
    }
    let valid = table
        .templates()
        .iter()
        .filter(|template| {
            let area = usize::from(template.w) * usize::from(template.h);
            area > 0
                && template.classes.len() == area
                && template.frames.len() >= area
                && available_banks.contains(&template.image.to_ascii_uppercase())
        })
        .collect::<Vec<_>>();

    // Ids are indices into `pieces`: the picker never looks inside one.
    let mut pieces: Vec<TileChoice> = Vec::new();
    let mut set = makepad_rtsmap::TileSet::new();
    for template in valid {
        let width = usize::from(template.w);
        let height = usize::from(template.h);
        let image = template.image.to_ascii_uppercase();
        if width == 1 && height == 1 {
            let Some(terrain) = template.classes.first().copied().and_then(class_terrain) else {
                continue;
            };
            for &frame in &template.frames {
                pieces.push(TileChoice { image: image.clone(), frame });
                set.push_single(terrain, (pieces.len() - 1) as u32);
            }
            continue;
        }
        for (index, &class) in template.classes.iter().enumerate() {
            let Some(&frame) = template.frames.get(index) else { continue };
            let Some(terrain) = class_terrain(class) else { continue };
            pieces.push(TileChoice { image: image.clone(), frame });
            set.push_masked(terrain, template_piece_mask(template, index), (pieces.len() - 1) as u32);
        }
    }

    let terrain = classes
        .iter()
        .map(|class| class_terrain(*class).unwrap_or(makepad_rtsmap::Terrain::Clear))
        .collect::<Vec<_>>();
    let multi_templates = makepad_rtsmap::tiles::masked_pick_count(&terrain, MAP_SIZE, MAP_SIZE, &set);
    let picks = makepad_rtsmap::tiles::pick_tiles_for(&terrain, MAP_SIZE, MAP_SIZE, &set, seed);
    let mut choices = Vec::with_capacity(classes.len());
    for (at, pick) in picks.into_iter().enumerate() {
        let Some(id) = pick else {
            return Err(format!("no template piece for class {}", classes[at] as char));
        };
        choices.push(pieces[id as usize].clone());
    }
    Ok((choices, multi_templates))
}

fn class_neighbour_mask(
    classes: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> u8 {
    if width == 0 || height == 0 || x >= width || y >= height {
        return 0;
    }
    let Some(&class) = classes.get(y.saturating_mul(width).saturating_add(x)) else {
        return 0;
    };
    let mut mask = 0;
    if y > 0 && classes.get((y - 1) * width + x) == Some(&class) {
        mask |= MASK_N;
    }
    if x + 1 < width && classes.get(y * width + x + 1) == Some(&class) {
        mask |= MASK_E;
    }
    if y + 1 < height && classes.get((y + 1) * width + x) == Some(&class) {
        mask |= MASK_S;
    }
    if x > 0 && classes.get(y * width + x - 1) == Some(&class) {
        mask |= MASK_W;
    }
    mask
}

fn template_piece_mask(template: &D2kTemplate, index: usize) -> u8 {
    let width = usize::from(template.w);
    let height = usize::from(template.h);
    if width == 0 || height == 0 || index >= width.saturating_mul(height) {
        return 0;
    }
    let x = index % width;
    let y = index / width;
    class_neighbour_mask(&template.classes, width, height, x, y)
}

fn bake_terrain(choices: &[TileChoice], banks: &BTreeMap<String, R8>, palette: &Pal) -> Vec<u8> {
    RtsEmitter::paint_cell_map_scaled((0, 0, MAP_SIZE as u16, MAP_SIZE as u16), TILE_PX, |x, y, tile| {
        let at = y as usize * MAP_SIZE + x as usize;
        let indexed = choices
            .get(at)
            .and_then(|choice| banks.get(&choice.image).map(|bank| (bank, choice.frame)))
            .and_then(|(bank, frame)| bank.entries().get(frame as usize))
            .filter(|image| image.w == TILE_PX && image.h == TILE_PX)
            .and_then(|image| image.pixels.as_deref());
        if let Some(indexed) = indexed {
            indexed_opaque(indexed, palette, tile);
        } else {
            let [r, g, b] = palette.rgb(0);
            for rgba in tile.chunks_exact_mut(4) {
                rgba.copy_from_slice(&[r, g, b, 255]);
            }
        }
    })
}

fn generated_world(
    seed: u32,
    generated: &GeneratedMap,
    terrain_rgba: Vec<u8>,
    has_bloom_sprite: bool,
) -> WorldSpec {
    let key = format!("arrakis-{seed}");
    let world_key = format!("worlds/{key}");
    let mut place = format!(
        "world-place 1\nsource d2k\nworld {world_key}\nmode rts\ncell 6.0\ntile 32\nmetres_per_pixel 0.1875\ngrid {world_key}.grid\n"
    );
    place.push_str("house Atreides color=3070d0 side=atreides\n");
    place.push_str("house Harkonnen color=d02020 side=harkonnen\n");
    place.push_str("house Ordos color=30b070 side=ordos\n");
    let mut resource = 0usize;
    for y in 0..MAP_SIZE {
        for x in 0..MAP_SIZE {
            let Some(stage) = generated.stages[y * MAP_SIZE + x] else { continue };
            let world_x = (x as f32 + 0.5) * CELL_M;
            let world_z = (y as f32 + 0.5) * CELL_M;
            place.push_str(&format!(
                "place r-{resource} resource billboards/d2k/spice {world_x:.4} 0.0400 {world_z:.4} 0.00000 align=floor layer=0.04 class=resource stage={stage}\n"
            ));
            resource += 1;
        }
    }
    if has_bloom_sprite {
        for (index, &(x, y)) in generated.blooms.iter().enumerate() {
            let world_x = (x as f32 + 0.5) * CELL_M;
            let world_z = (y as f32 + 0.5) * CELL_M;
            place.push_str(&format!(
                "place bloom-{index} scenery billboards/d2k/spicebloom {world_x:.4} 0.0600 {world_z:.4} 0.00000 align=floor layer=0.06 class=scenery\n"
            ));
        }
    }

    let mut spawn = String::from("world-spawn 1\n");
    let colors = [[48, 112, 208], [208, 32, 32], [48, 176, 112]];
    let mut preview_dots = Vec::new();
    for (index, &(x, y)) in generated.starts.iter().enumerate() {
        let world_x = (x as f32 + 0.5) * CELL_M;
        let world_z = (y as f32 + 0.5) * CELL_M;
        spawn.push_str(&format!(
            "start start_{index} {world_x:.4} 0.0000 {world_z:.4} 0.00000 -1.45000\n"
        ));
        preview_dots.push(PreviewDot {
            x: x as f32 + 0.5,
            y: y as f32 + 0.5,
            rgb: colors[index % colors.len()],
        });
    }
    spawn.push_str("floor 0\nstep 0.5\neye 60\n");
    let grid = generated
        .grid
        .chunks_exact(MAP_SIZE)
        .map(|row| String::from_utf8_lossy(row).into_owned())
        .collect();
    WorldSpec {
        key,
        width: MAP_SIZE as u16,
        height: MAP_SIZE as u16,
        terrain_rgba,
        grid,
        place_text: place,
        roster: pack_roster(),
        spawn_text: spawn,
        preview_dots,
        preview_crop: None,
        tags: vec!["d2k".into(), "world".into(), "rts".into(), "arrakis".into()],
    }
}

fn emit_spice(
    emitter: &mut RtsEmitter<'_>,
    shapes: &BTreeMap<String, PathBuf>,
    banks: &BTreeMap<String, R8>,
    palette: &Pal,
    report: &mut ConvertReport,
) -> Result<(), String> {
    let source = decode_shape_optional(shapes, "spice0", report);
    let mut frames = Vec::new();
    if let Some(shp) = source {
        for stage in 0..8 {
            let source = stage % shp.frames().len();
            frames.push(sprite_frame(&shp, source, palette, 0));
        }
        report.spice_note = format!(
            "spice0.shp resolved ({} source frames repeated across stages 0..7); spice1.shp absent; T terrain remains baked",
            shp.frames().len()
        );
    } else {
        let base = banks.get("BLOXBASE").ok_or("BLOXBASE disappeared")?;
        let candidates = [0usize, 8, 9, 48, 49, 50, 51, 52];
        for source in candidates {
            let image = base
                .entries()
                .get(source)
                .filter(|image| image.w == TILE_PX && image.h == TILE_PX)
                .and_then(|image| image.pixels.as_deref())
                .ok_or_else(|| format!("BLOXBASE frame {source} is not a 32px tile"))?;
            frames.push(SpritePixels {
                rgba: indexed_transparent(image, palette),
                width: TILE_PX,
                height: TILE_PX,
                rot: 0,
            });
        }
    }
    emit_spec(
        emitter,
        report,
        SpriteSpec {
            key: "spice".into(),
            role: "resource",
            facings: 1,
            frames,
            states: vec![SpriteState { name: "idle", first: 0, last: 8, looping: false, fps: 1 }],
            unit: Some(UnitSpec { manifest_lines: vec!["unit class=resource title=\"Spice\"".into()] }),
            manifest_lines: Vec::new(),
            tags: vec!["d2k", "resource"],
        },
    )
}

fn emit_named_sprites(
    emitter: &mut RtsEmitter<'_>,
    shapes: &BTreeMap<String, PathBuf>,
    palette: &Pal,
    audio: &BTreeSet<String>,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let remap = remap_line(palette);
    let total = VEHICLES.len() + VEHICLE_TURRETS.len() + INFANTRY.len() + STRUCTURES.len() * 3 + MISC_SHAPES.len();
    let mut done = 0usize;
    for &(key, stem, icon) in VEHICLES {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = decode_shape(shapes, stem, report) else { continue };
        let (count, frames_per_facing) = if key == "ornithopter" && shp.frames().len() >= 96 {
            (96, 3)
        } else {
            (shp.frames().len().min(32), 1)
        };
        if count < 32 {
            report.missing_shapes.insert(format!("{stem}.shp (needs 32 facings)"));
            continue;
        }
        let frames = (0..count)
            .map(|source| sprite_frame(&shp, source, palette, 1 + (source / frames_per_facing) as u8))
            .collect();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 32,
                frames,
                states: vec![SpriteState { name: "idle", first: 0, last: count, looping: true, fps: 8 }],
                unit: Some(UnitSpec { manifest_lines: unit_lines(key, audio) }),
                manifest_lines: vec![remap.clone()],
                tags: vec!["d2k", "unit", if matches!(key, "carryall" | "ornithopter") { "aircraft" } else { "vehicle" }],
            },
        )?;
        emit_icon(emitter, shapes, palette, icon, key, report)?;
    }

    for &(key, stem) in VEHICLE_TURRETS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let shp = if stem == "missileturret" {
            decode_shape_optional(shapes, stem, report)
        } else {
            decode_shape(shapes, stem, report)
        };
        let Some(shp) = shp else { continue };
        if shp.frames().len() < 32 {
            report.missing_shapes.insert(format!("{stem}.shp (needs 32 facings)"));
            continue;
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 32,
                frames: (0..32).map(|source| sprite_frame(&shp, source, palette, 1 + source as u8)).collect(),
                states: vec![SpriteState { name: "idle", first: 0, last: 32, looping: true, fps: 8 }],
                unit: None,
                manifest_lines: vec![remap.clone()],
                tags: vec!["d2k", "unit", "turret"],
            },
        )?;
    }

    for &(key, stem) in INFANTRY {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = decode_shape(shapes, stem, report) else { continue };
        let idle_end = shp.frames().len().min(8);
        if idle_end < 8 {
            report.missing_shapes.insert(format!("{stem}.shp (needs 8 idle facings)"));
            continue;
        }
        let (walk_per_facing, walk_source_end, fire_per_facing, fire_source_end) = match key {
            "saboteur" => (4, 40, 0, 40),
            "thumper" => (6, 56, 5, 96),
            "engineer" => (6, 56, 0, 56),
            _ => (6, 56, 4, 88),
        };
        if shp.frames().len() < walk_source_end || shp.frames().len() < fire_source_end {
            report.missing_shapes.insert(format!("{stem}.shp (incomplete infantry layout)"));
            continue;
        }
        let mut frames = Vec::new();
        append_facing_block(&mut frames, &shp, 0, 8, 1, palette);
        let idle_last = frames.len();
        append_facing_block(&mut frames, &shp, 8, walk_source_end, walk_per_facing, palette);
        let walk_last = frames.len();
        if fire_per_facing > 0 {
            append_facing_block(
                &mut frames,
                &shp,
                walk_source_end,
                fire_source_end,
                fire_per_facing,
                palette,
            );
        } else {
            append_facing_block(&mut frames, &shp, 0, 8, 1, palette);
        }
        let fire_last = frames.len();
        let embedded_death = matches!(key, "bazooka" | "sardaukar" | "fremen")
            && shp.frames().len() >= 236;
        if embedded_death {
            for source in 176..236 {
                frames.push(sprite_frame(&shp, source, palette, 0));
            }
        } else if let Some(death) = decode_shape(shapes, &format!("{stem}death"), report) {
            for source in 0..death.frames().len() {
                frames.push(sprite_frame(&death, source, palette, 0));
            }
        } else {
            frames.push(sprite_frame(&shp, 0, palette, 0));
        }
        let die_last = frames.len();
        report.infantry_layouts.push(format!(
            "{key}: source={} idle=0..8, walk=8..{} ({} per facing), fire={}..{} ({} per facing), die={} ({})",
            shp.frames().len(),
            walk_source_end,
            walk_per_facing,
            walk_source_end,
            fire_source_end,
            fire_per_facing,
            die_last - fire_last,
            if embedded_death { "embedded 176..236" } else { "separate death sheet" },
        ));
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 8,
                frames,
                states: vec![
                    SpriteState { name: "idle", first: 0, last: idle_last, looping: true, fps: 8 },
                    SpriteState { name: "walk", first: idle_last, last: walk_last, looping: true, fps: 10 },
                    SpriteState { name: "fire", first: walk_last, last: fire_last, looping: false, fps: 10 },
                    SpriteState { name: "die", first: fire_last, last: die_last, looping: false, fps: 10 },
                ],
                unit: Some(UnitSpec { manifest_lines: unit_lines(key, audio) }),
                manifest_lines: vec![remap.clone()],
                tags: vec!["d2k", "unit", "infantry"],
            },
        )?;
        emit_icon(emitter, shapes, palette, &format!("{stem}icon"), key, report)?;
    }

    for structure in STRUCTURES {
        for house in ['a', 'h', 'o'] {
            done += 1;
            let Some(side) = house_side(house) else { continue };
            let key = format!("{}-{house}", structure.key);
            tick(on_tick, done, total, format!("sprite {key}"), None);
            let source_stem = format!("{}{house}", structure.stem);
            let Some(shp) = decode_shape(shapes, &source_stem, report) else { continue };
            let base_count = shp.frames().len();
            let half = base_count.div_ceil(2);
            let mut frames = (0..base_count)
                .map(|source| sprite_frame(&shp, source, palette, 0))
                .collect::<Vec<_>>();
            let build_first = frames.len();
            if let Some(make) = structure.make.and_then(|stem| decode_shape(shapes, stem, report)) {
                for source in 0..make.frames().len() {
                    frames.push(sprite_frame(&make, source, palette, 0));
                }
            } else if let Some(make) = structure.make {
                report.missing_shapes.insert(format!("{make}.shp"));
            }
            let build_last = frames.len();
            let mut states = vec![
                SpriteState { name: "idle", first: 0, last: half, looping: true, fps: 6 },
                SpriteState { name: "damaged", first: half, last: base_count, looping: true, fps: 6 },
            ];
            if build_last > build_first {
                states.push(SpriteState { name: "build", first: build_first, last: build_last, looping: false, fps: 15 });
            }
            let (fw, fh) = structure_footprint(structure.key);
            report.structure_halves.push(format!("{key}: {half}+{}", base_count - half));
            let mut unit = structure_unit_lines(structure.key, side, house, audio);
            unit.push(format!("footprint {fw} {fh}"));
            emit_spec(
                emitter,
                report,
                SpriteSpec {
                    key: key.clone(),
                    role: "structure",
                    facings: 1,
                    frames,
                    states,
                    unit: Some(UnitSpec { manifest_lines: unit }),
                    manifest_lines: Vec::new(),
                    tags: vec!["d2k", "structure", side],
                },
            )?;
            let icon_stem = structure_icon_stem(structure, house);
            emit_icon(emitter, shapes, palette, &icon_stem, &key, report)?;

            if let Some(turret_stem) = structure_turret_stem(structure.key, house) {
                if let Some(turret) = decode_shape(shapes, &turret_stem, report) {
                    if turret.frames().len() >= 32 {
                        emit_spec(
                            emitter,
                            report,
                            SpriteSpec {
                                key: format!("{key}-turret"),
                                role: "unit",
                                facings: 32,
                                frames: (0..32)
                                    .map(|source| sprite_frame(&turret, source, palette, 1 + source as u8))
                                    .collect(),
                                states: vec![SpriteState { name: "idle", first: 0, last: 32, looping: true, fps: 8 }],
                                unit: None,
                                manifest_lines: Vec::new(),
                                tags: vec!["d2k", "unit", "turret", side],
                            },
                        )?;
                    }
                }
            }
        }
    }

    for &(stem, role) in MISC_SHAPES {
        done += 1;
        tick(on_tick, done, total, format!("sprite {stem}"), None);
        let Some(shp) = decode_shape(shapes, stem, report) else { continue };
        let count = shp.frames().len();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: stem.into(),
                role,
                facings: 1,
                frames: (0..count).map(|source| sprite_frame(&shp, source, palette, 0)).collect(),
                states: vec![SpriteState { name: "idle", first: 0, last: count, looping: role == "effect", fps: 12 }],
                unit: None,
                manifest_lines: Vec::new(),
                tags: vec!["d2k", role],
            },
        )?;
    }
    Ok(())
}

fn append_facing_block(
    output: &mut Vec<SpritePixels>,
    shp: &Shp,
    first: usize,
    last: usize,
    frames_per_facing: usize,
    palette: &Pal,
) {
    for source in first..last.min(shp.frames().len()) {
        output.push(sprite_frame(
            shp,
            source,
            palette,
            1 + ((source - first) / frames_per_facing.max(1)) as u8,
        ));
    }
}

fn sprite_frame(shp: &Shp, source: usize, palette: &Pal, rot: u8) -> SpritePixels {
    SpritePixels {
        rgba: indexed_transparent(&shp.frames()[source], palette),
        width: shp.width() as u32,
        height: shp.height() as u32,
        rot,
    }
}

fn decode_shape(
    files: &BTreeMap<String, PathBuf>,
    stem: &str,
    report: &mut ConvertReport,
) -> Option<Shp> {
    let Some(path) = files.get(&stem.to_ascii_lowercase()) else {
        report.missing_shapes.insert(format!("{stem}.shp"));
        return None;
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.shape_errors.push(format!("{}: {error}", path.display()));
            return None;
        }
    };
    match Shp::parse(&bytes) {
        Ok(shp) => Some(shp),
        Err(error) => {
            report.shape_errors.push(format!("{}: {error}", path.display()));
            None
        }
    }
}

fn decode_shape_optional(
    files: &BTreeMap<String, PathBuf>,
    stem: &str,
    report: &mut ConvertReport,
) -> Option<Shp> {
    if !files.contains_key(&stem.to_ascii_lowercase()) {
        return None;
    }
    decode_shape(files, stem, report)
}

fn emit_icon(
    emitter: &mut RtsEmitter<'_>,
    shapes: &BTreeMap<String, PathBuf>,
    palette: &Pal,
    stem: &str,
    key: &str,
    report: &mut ConvertReport,
) -> Result<(), String> {
    let Some(shp) = decode_shape(shapes, stem, report) else { return Ok(()) };
    let Some(frame) = shp.frames().first() else { return Ok(()) };
    emitter.emit_texture(
        &format!("icons/d2k/{key}"),
        &indexed_transparent(frame, palette),
        shp.width() as u32,
        shp.height() as u32,
        &["d2k", "icon"],
    )?;
    report.icon_keys.insert(format!("icons/d2k/{key}"));
    Ok(())
}

fn structure_icon_stem(structure: &StructureDef, house: char) -> String {
    match structure.key {
        "guntower" => "turreticon".into(),
        "rocketturret" => "rturreticon".into(),
        "wall" => "wallicon".into(),
        _ => format!("{}{house}icon", structure.stem),
    }
}

fn structure_turret_stem(key: &str, house: char) -> Option<String> {
    match key {
        "guntower" => Some(format!("gunturret{house}")),
        "rocketturret" => Some(format!("rocketturret{house}")),
        _ => None,
    }
}

fn emit_spec(
    emitter: &mut RtsEmitter<'_>,
    report: &mut ConvertReport,
    spec: SpriteSpec,
) -> Result<(), String> {
    *report.roles.entry(spec.role.into()).or_default() += 1;
    report.sprite_keys.insert(format!("billboards/d2k/{}", spec.key));
    emitter.emit_sprite(spec)
}

fn indexed_opaque(indexed: &[u8], palette: &Pal, output: &mut [u8]) {
    for (&index, rgba) in indexed.iter().zip(output.chunks_exact_mut(4)) {
        let [r, g, b] = palette.rgb(index);
        rgba.copy_from_slice(&[r, g, b, 255]);
    }
}

fn indexed_transparent(indexed: &[u8], palette: &Pal) -> Vec<u8> {
    let mut output = Vec::with_capacity(indexed.len() * 4);
    for &index in indexed {
        if index == 0 {
            output.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let [r, g, b] = palette.rgb(index);
            output.extend_from_slice(&[r, g, b, 255]);
        }
    }
    output
}

fn remap_line(palette: &Pal) -> String {
    let colors = (240..=255)
        .map(|index| {
            let [r, g, b] = palette.rgb(index);
            format!("{r},{g},{b}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("remap {colors}")
}

fn house_side(house: char) -> Option<&'static str> {
    match house {
        'a' => Some("atreides"),
        'h' => Some("harkonnen"),
        'o' => Some("ordos"),
        _ => None,
    }
}

fn structure_footprint(key: &str) -> (usize, usize) {
    match key {
        "conyard" | "ref" | "heavy" | "hightech" => (3, 2),
        "pwr" | "barracks" | "light" | "research" | "outpost" => (2, 2),
        "repair" | "starport" | "palace" => (3, 3),
        _ => (1, 1),
    }
}

#[derive(Clone, Copy)]
struct UnitDef {
    key: &'static str,
    line: &'static str,
    weapon: Option<&'static str>,
    weapon2: Option<&'static str>,
    voice: &'static str,
}

const UNITS: &[UnitDef] = &[
    UnitDef { key: "rifle", line: "unit class=infantry title=\"Light Infantry\" cost=50 hp=50 armor=none speed=2.5 sight=12 sides=atreides,harkonnen,ordos producer=barracks weapon=lmg", weapon: Some("lmg"), weapon2: None, voice: "I" },
    UnitDef { key: "bazooka", line: "unit class=infantry title=\"Trooper\" cost=90 hp=45 armor=none speed=2.5 sight=12 sides=atreides,harkonnen,ordos producer=barracks weapon=rocket", weapon: Some("rocket"), weapon2: None, voice: "I" },
    UnitDef { key: "engineer", line: "unit class=infantry title=\"Engineer\" cost=200 hp=25 armor=none speed=2.5 sight=12 sides=atreides,harkonnen,ordos producer=barracks capture=1", weapon: None, weapon2: None, voice: "E" },
    UnitDef { key: "sardaukar", line: "unit class=infantry title=\"Sardaukar\" cost=200 hp=100 armor=none speed=2.5 sight=12 sides=harkonnen producer=barracks prereq=palace weapon=lmg weapon2=rocket", weapon: Some("lmg"), weapon2: Some("rocket"), voice: "I" },
    UnitDef { key: "fremen", line: "unit class=infantry title=\"Fremen\" cost=200 hp=80 armor=none speed=3.0 sight=18 sides=atreides producer=barracks prereq=palace weapon=rocket", weapon: Some("rocket"), weapon2: None, voice: "F" },
    UnitDef { key: "saboteur", line: "unit class=infantry title=\"Saboteur\" cost=200 hp=40 armor=none speed=3.0 sight=12 sides=ordos producer=barracks prereq=palace", weapon: None, weapon2: None, voice: "I" },
    UnitDef { key: "thumper", line: "unit class=infantry title=\"Thumper\" cost=200 hp=40 armor=none speed=2.5 sight=12 sides=atreides,harkonnen,ordos producer=barracks", weapon: None, weapon2: None, voice: "I" },
    UnitDef { key: "trike", line: "unit class=vehicle title=\"Trike\" cost=240 hp=100 armor=light speed=12.0 sight=18 sides=atreides,harkonnen producer=light weapon=lmg", weapon: Some("lmg"), weapon2: None, voice: "V" },
    UnitDef { key: "raider", line: "unit class=vehicle title=\"Raider Trike\" cost=260 hp=90 armor=light speed=14.0 sight=18 sides=ordos producer=light weapon=lmg", weapon: Some("lmg"), weapon2: None, voice: "V" },
    UnitDef { key: "quad", line: "unit class=vehicle title=\"Quad\" cost=300 hp=140 armor=light speed=10.0 sight=18 sides=atreides,harkonnen,ordos producer=light weapon=rocket", weapon: Some("rocket"), weapon2: None, voice: "V" },
    UnitDef { key: "combat-a", line: "unit class=vehicle title=\"Combat Tank\" cost=700 hp=400 armor=heavy speed=6.0 sight=18 sides=atreides producer=heavy weapon=90mm turret=billboards/d2k/combat-a-turret", weapon: Some("90mm"), weapon2: None, voice: "V" },
    UnitDef { key: "combat-h", line: "unit class=vehicle title=\"Combat Tank\" cost=700 hp=450 armor=heavy speed=5.5 sight=18 sides=harkonnen producer=heavy weapon=105mm turret=billboards/d2k/combat-h-turret", weapon: Some("105mm"), weapon2: None, voice: "V" },
    UnitDef { key: "combat-o", line: "unit class=vehicle title=\"Combat Tank\" cost=700 hp=350 armor=heavy speed=7.0 sight=18 sides=ordos producer=heavy weapon=75mm turret=billboards/d2k/combat-o-turret", weapon: Some("75mm"), weapon2: None, voice: "V" },
    UnitDef { key: "siege", line: "unit class=vehicle title=\"Siege Tank\" cost=900 hp=300 armor=heavy speed=4.5 sight=24 sides=atreides,harkonnen,ordos producer=heavy prereq=hightech weapon=155mm turret=billboards/d2k/siege-turret", weapon: Some("155mm"), weapon2: None, voice: "V" },
    UnitDef { key: "missile", line: "unit class=vehicle title=\"Missile Tank\" cost=900 hp=200 armor=light speed=6.0 sight=24 sides=atreides,harkonnen,ordos producer=heavy prereq=hightech weapon=227mm", weapon: Some("227mm"), weapon2: None, voice: "V" },
    UnitDef { key: "sonic", line: "unit class=vehicle title=\"Sonic Tank\" cost=1200 hp=300 armor=heavy speed=5.0 sight=24 sides=atreides producer=heavy prereq=research weapon=sonic", weapon: Some("sonic"), weapon2: None, voice: "V" },
    UnitDef { key: "devastator", line: "unit class=vehicle title=\"Devastator\" cost=1500 hp=800 armor=heavy speed=3.5 sight=18 sides=harkonnen producer=heavy prereq=research weapon=plasma weapon2=plasma", weapon: Some("plasma"), weapon2: None, voice: "V" },
    UnitDef { key: "deviator", line: "unit class=vehicle title=\"Deviator\" cost=1000 hp=250 armor=light speed=6.0 sight=24 sides=ordos producer=heavy prereq=research weapon=deviator", weapon: Some("deviator"), weapon2: None, voice: "V" },
    UnitDef { key: "harvester", line: "unit class=vehicle title=\"Harvester\" cost=1500 hp=700 armor=heavy speed=5.0 sight=12 sides=atreides,harkonnen,ordos producer=heavy prereq=ref harvester=1 capacity=700", weapon: None, weapon2: None, voice: "V" },
    UnitDef { key: "mcv", line: "unit class=vehicle title=\"MCV\" cost=2000 hp=600 armor=heavy speed=4.0 sight=12 sides=atreides,harkonnen,ordos producer=heavy mcv=1 deploys=conyard", weapon: None, weapon2: None, voice: "V" },
    UnitDef { key: "carryall", line: "unit class=aircraft title=\"Carryall\" cost=1100 hp=300 armor=light speed=20.0 sight=24 sides=atreides,harkonnen,ordos producer=hightech", weapon: None, weapon2: None, voice: "V" },
    UnitDef { key: "ornithopter", line: "unit class=aircraft title=\"Ornithopter\" cost=900 hp=150 armor=light speed=24.0 sight=24 sides=atreides producer=hightech weapon=bomb", weapon: Some("bomb"), weapon2: None, voice: "V" },
];

fn unit_lines(key: &str, audio: &BTreeSet<String>) -> Vec<String> {
    let Some(unit) = UNITS.iter().find(|unit| unit.key == key) else { return Vec::new() };
    let mut lines = vec![rewrite_unit_roles(key, unit.line, ROLE_TABLE)];
    for weapon in unit.weapon.into_iter().chain(unit.weapon2) {
        if let Some(line) = weapon_line(weapon) {
            lines.push(line.into());
        }
    }
    if let Some(sound) = sound_line(unit.voice, unit.weapon, false, audio) {
        lines.push(sound);
    }
    lines
}

fn structure_unit_lines(key: &str, side: &str, house: char, audio: &BTreeSet<String>) -> Vec<String> {
    let turret = match key {
        "guntower" | "rocketturret" => format!(" turret=billboards/d2k/{key}-{house}-turret"),
        _ => String::new(),
    };
    let line = match key {
        "conyard" => format!("unit class=structure title=\"Construction Yard\" cost=2000 hp=1000 armor=concrete speed=0 sight=30 sides={side} footprint=3x2 power=0 build=1"),
        "pwr" => format!("unit class=structure title=\"Wind Trap\" cost=300 hp=400 armor=wood speed=0 sight=18 sides={side} producer=conyard footprint=2x2 power=100 build=1"),
        "ref" => format!("unit class=structure title=\"Spice Refinery\" cost=1500 hp=900 armor=wood speed=0 sight=24 sides={side} producer=conyard prereq=pwr footprint=3x2 power=-30 refinery=1 build=1"),
        "silo" => format!("unit class=structure title=\"Spice Silo\" cost=120 hp=300 armor=wood speed=0 sight=12 sides={side} producer=conyard prereq=ref footprint=1x1 power=-10 build=1"),
        "barracks" => format!("unit class=structure title=\"Barracks\" cost=300 hp=400 armor=wood speed=0 sight=18 sides={side} producer=conyard prereq=pwr footprint=2x2 power=-20 build=1"),
        "light" => format!("unit class=structure title=\"Light Factory\" cost=500 hp=500 armor=wood speed=0 sight=18 sides={side} producer=conyard prereq=ref footprint=2x2 power=-30 build=1"),
        "heavy" => format!("unit class=structure title=\"Heavy Factory\" cost=900 hp=900 armor=light speed=0 sight=18 sides={side} producer=conyard prereq=ref footprint=3x2 power=-40 build=1"),
        "hightech" => format!("unit class=structure title=\"High Tech Factory\" cost=1000 hp=700 armor=wood speed=0 sight=18 sides={side} producer=conyard prereq=heavy footprint=3x2 power=-40 build=1"),
        "research" => format!("unit class=structure title=\"IX Research Centre\" cost=1000 hp=700 armor=wood speed=0 sight=18 sides={side} producer=conyard prereq=hightech footprint=2x2 power=-100 build=1"),
        "outpost" => format!("unit class=structure title=\"Outpost\" cost=1000 hp=1000 armor=wood speed=0 sight=30 sides={side} producer=conyard prereq=pwr footprint=2x2 power=-40 build=1"),
        "repair" => format!("unit class=structure title=\"Repair Pad\" cost=1000 hp=600 armor=wood speed=0 sight=18 sides={side} producer=conyard prereq=heavy footprint=3x3 power=-30 build=1"),
        "starport" => format!("unit class=structure title=\"Starport\" cost=2000 hp=1200 armor=light speed=0 sight=18 sides={side} producer=conyard prereq=outpost footprint=3x3 power=-50 build=1"),
        "palace" => format!("unit class=structure title=\"Palace\" cost=2000 hp=1500 armor=concrete speed=0 sight=30 sides={side} producer=conyard prereq=research footprint=3x3 power=-80 build=1"),
        "guntower" => format!("unit class=defense title=\"Gun Turret\" cost=500 hp=400 armor=heavy speed=0 sight=24 sides={side} producer=conyard prereq=barracks weapon=105mm footprint=1x1 power=-20 build=1{turret}"),
        "rocketturret" => format!("unit class=defense title=\"Rocket Turret\" cost=900 hp=500 armor=heavy speed=0 sight=30 sides={side} producer=conyard prereq=outpost weapon=rocket footprint=1x1 power=-30 build=1{turret}"),
        "wall" => format!("unit class=structure title=\"Wall\" cost=50 hp=50 armor=concrete speed=0 sight=0 sides={side} producer=conyard prereq=barracks footprint=1x1 wall=1 build=1"),
        _ => return Vec::new(),
    };
    let line = rewrite_unit_roles(key, &line, ROLE_TABLE);
    let weapon = match key {
        "guntower" => Some("105mm"),
        "rocketturret" => Some("rocket"),
        _ => None,
    };
    let mut lines = vec![line];
    if let Some(weapon) = weapon.and_then(weapon_line) {
        lines.push(weapon.into());
    }
    if let Some(sound) = sound_line("S", weapon.map(|_| if key == "guntower" { "105mm" } else { "rocket" }), true, audio) {
        lines.push(sound);
    }
    lines
}

fn pack_roster() -> Vec<String> {
    let mut roster = UNITS
        .iter()
        .filter(|unit| positive_unit_cost(unit.line))
        .map(|unit| roster_key("d2k", unit.key))
        .collect::<Vec<_>>();
    let audio = BTreeSet::new();
    for structure in STRUCTURES {
        for (house, side) in [('a', "atreides"), ('h', "harkonnen"), ('o', "ordos")] {
            let lines = structure_unit_lines(structure.key, side, house, &audio);
            if lines.first().is_some_and(|line| positive_unit_cost(line)) {
                roster.push(roster_key("d2k", &format!("{}-{house}", structure.key)));
            }
        }
    }
    roster
}

#[cfg(test)]
pub(super) fn role_test_lines() -> Vec<String> {
    let mut lines = UNITS
        .iter()
        .map(|unit| rewrite_unit_roles(unit.key, unit.line, ROLE_TABLE))
        .collect::<Vec<_>>();
    let audio = BTreeSet::new();
    for structure in STRUCTURES {
        lines.extend(structure_unit_lines(
            structure.key,
            "atreides",
            'a',
            &audio,
        ));
    }
    lines
}

fn weapon_line(id: &str) -> Option<&'static str> {
    Some(match id {
        "lmg" => "weapon id=lmg damage=15 rate=3.0 range=18 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 fire=sfx/d2k/mgun2 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "rocket" => "weapon id=rocket damage=30 rate=0.8 range=27 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 fire=sfx/d2k/bazook1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "75mm" => "weapon id=75mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 fire=sfx/d2k/medtank1 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "90mm" => "weapon id=90mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 fire=sfx/d2k/medtank1 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "105mm" => "weapon id=105mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 fire=sfx/d2k/turret1 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "155mm" => "weapon id=155mm damage=60 rate=0.3 range=36 delivery=projectile projectile_speed=18 splash_radius=6 splash_damage=40 fire=sfx/d2k/mortar1 versus=none:.9,wood:.9,light:.6,heavy:.4,concrete:.9",
        "227mm" => "weapon id=227mm damage=60 rate=0.4 range=45 delivery=projectile projectile_speed=20 splash_radius=6 splash_damage=40 fire=sfx/d2k/missle1 versus=none:.9,wood:.9,light:.6,heavy:.4,concrete:.9",
        "sonic" => "weapon id=sonic damage=60 rate=0.5 range=30 delivery=projectile projectile_speed=30 splash_radius=0 splash_damage=0 fire=sfx/d2k/sonic1 versus=none:1,wood:1,light:1,heavy:1,concrete:1",
        "plasma" => "weapon id=plasma damage=80 rate=0.5 range=30 delivery=projectile projectile_speed=30 splash_radius=0 splash_damage=0 fire=sfx/d2k/tankhvy1 versus=none:1,wood:1,light:1,heavy:1,concrete:1",
        "deviator" => "weapon id=deviator damage=1 rate=0.3 range=30 delivery=projectile projectile_speed=30 splash_radius=0 splash_damage=0 fire=sfx/d2k/stealth1 versus=none:1,wood:1,light:1,heavy:1,concrete:1 effect_only=1",
        "bomb" => "weapon id=bomb damage=100 rate=0.3 range=12 delivery=projectile projectile_speed=18 splash_radius=6 splash_damage=60 fire=sfx/d2k/napalm1 versus=none:1,wood:1,light:.8,heavy:.7,concrete:1",
        _ => return None,
    })
}

fn weapon_sound(id: &str) -> &'static str {
    match id {
        "lmg" => "mgun2",
        "rocket" => "bazook1",
        "75mm" | "90mm" => "medtank1",
        "105mm" => "turret1",
        "155mm" => "mortar1",
        "227mm" => "missle1",
        "sonic" => "sonic1",
        "plasma" => "tankhvy1",
        "deviator" => "stealth1",
        "bomb" => "napalm1",
        _ => "",
    }
}

fn sound_line(
    voice: &str,
    weapon: Option<&str>,
    structure: bool,
    audio: &BTreeSet<String>,
) -> Option<String> {
    let upper_select = format!("_{voice}SEL");
    let upper_confirm = format!("_{voice}CONF");
    let mut select = Vec::new();
    let mut confirm = Vec::new();
    for stem in audio {
        let upper = stem.to_ascii_uppercase();
        if upper.contains(&upper_select) {
            select.push(format!("sfx/d2k/{stem}"));
        }
        if upper.contains(&upper_confirm) {
            confirm.push(format!("sfx/d2k/{stem}"));
        }
    }
    let mut slots = Vec::new();
    if !select.is_empty() {
        slots.push(format!("select={}", select.join("|")));
    }
    if !confirm.is_empty() {
        slots.push(format!("move={}", confirm.join("|")));
    }
    if let Some(weapon) = weapon {
        let stem = weapon_sound(weapon);
        if audio.contains(stem) {
            slots.push(format!("attack=sfx/d2k/{stem}"));
        }
    }
    let death = if structure { "expllg2" } else if voice == "V" { "explmd2" } else { "killguy1" };
    if audio.contains(death) {
        slots.push(format!("death=sfx/d2k/{death}"));
    }
    if slots.is_empty() { None } else { Some(format!("sound {}", slots.join(" "))) }
}

fn emit_audio(
    emitter: &mut RtsEmitter<'_>,
    pack: &Path,
    data_root: &Path,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<BTreeSet<String>, String> {
    let gamesfx = find_named_dir(pack, data_root, "GAMESFX");
    let aud_files = gamesfx
        .as_deref()
        .map(|directory| files_by_stem(directory, "aud"))
        .transpose()?
        .unwrap_or_default();
    let total = aud_files.len().max(1);
    for (index, (stem, path)) in aud_files.iter().enumerate() {
        tick(on_tick, index, total, format!("sfx {stem}"), None);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                report.audio_errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        match Aud::parse(&bytes) {
            Ok(audio) => {
                emitter.emit_sfx(
                    &format!("sfx/d2k/{stem}"),
                    audio.sample_rate(),
                    audio.channels(),
                    audio.samples(),
                    &["d2k", "sfx"],
                )?;
                report.audio_keys.insert(stem.clone());
            }
            Err(error) => report.audio_errors.push(format!("{}: {error}", path.display())),
        }
    }

    if let Ok(sound) = read_named(data_root, "SOUND.RS") {
        for (name, wav) in sound_rs_entries(&sound)? {
            let stem = name
                .strip_suffix(".wav")
                .unwrap_or(&name)
                .to_ascii_lowercase();
            if report.audio_keys.contains(&stem) {
                continue;
            }
            emitter.emit_wav(&format!("sfx/d2k/{stem}"), wav, &["d2k", "sfx"])?;
            report.audio_keys.insert(stem);
        }
    }
    Ok(report.audio_keys.clone())
}

fn sound_rs_entries(bytes: &[u8]) -> Result<Vec<(String, &[u8])>, String> {
    let table_size = read_u32(bytes, 0).ok_or("SOUND.RS: truncated table size")? as usize;
    let table_end = 4usize
        .checked_add(table_size)
        .filter(|&end| end <= bytes.len())
        .ok_or("SOUND.RS: invalid table size")?;
    let mut at = 4usize;
    let mut entries = Vec::new();
    while at < table_end {
        let relative_end = bytes[at..table_end]
            .iter()
            .position(|&byte| byte == 0)
            .ok_or("SOUND.RS: unterminated name")?;
        let name_end = at + relative_end;
        let name = std::str::from_utf8(&bytes[at..name_end])
            .map_err(|_| "SOUND.RS: non-UTF8 name")?
            .to_ascii_lowercase();
        at = name_end + 1;
        let offset = read_u32(bytes, at).ok_or("SOUND.RS: truncated offset")? as usize;
        let length = read_u32(bytes, at + 4).ok_or("SOUND.RS: truncated length")? as usize;
        at = at.checked_add(8).ok_or("SOUND.RS: invalid table offset")?;
        if at > table_end {
            return Err("SOUND.RS: entry crosses table end".into());
        }
        let end = offset.checked_add(length).ok_or("SOUND.RS: invalid WAVE extent")?;
        let wav = bytes.get(offset..end).ok_or("SOUND.RS: WAVE outside file")?;
        if wav.get(..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
            return Err(format!("SOUND.RS: {name} is not WAVE"));
        }
        entries.push((name, wav));
    }
    if at != table_end {
        return Err("SOUND.RS: table length mismatch".into());
    }
    Ok(entries)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let value = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(value.try_into().ok()?))
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
    let mut text = String::from("# D2K staged conversion report\n\n");
    text.push_str(&format!("- Pack data root: `{}`.\n", report.data_root));
    text.push_str(&format!("- Tile banks: {}.\n", report.tile_banks.join(", ")));
    if report.named_sprites {
        text.push_str("- Named sprite directory resolved.\n");
    } else {
        text.push_str("- no named sprites.\n");
    }
    text.push_str(&format!("- Spice overlays: {}.\n", report.spice_note));
    text.push_str("\n## Generated worlds\n\n| Key | Size | Starts | Resources | Neighbour-mask pieces |\n|---|---:|---:|---:|---:|\n");
    for world in &report.worlds {
        text.push_str(&format!(
            "| {} | 64x64 | {} | {} | {} |\n",
            world.key, world.starts, world.resources, world.multi_templates
        ));
    }
    text.push_str("\nThe generator uses xorshift seeds 1 through 4. Rock and spice template pieces are classified by their N/E/S/W class neighbours, with one seeded interior and edge/corner choice per connected plateau or field; an absent authored rim mask falls back to that component's one-cell class art. Plateau interiors use buildable `.` grid cells, their authored rims and outcrops use `#`, dunes use `b`, and every emitted spice cell has both `t` and a stage 0..7 resource row. Terrain is 32 px per 6 m cell (`metres_per_pixel 0.1875`).\n");
    text.push_str("\n## Sprite assets\n\n");
    for (role, count) in &report.roles {
        text.push_str(&format!("- {role}: {count}\n"));
    }
    text.push_str("\nFull billboard key list:\n\n");
    for key in &report.sprite_keys {
        text.push_str(&format!("- `{key}`\n"));
    }
    text.push_str("\nFull icon key list:\n\n");
    for key in &report.icon_keys {
        text.push_str(&format!("- `{key}`\n"));
    }
    text.push_str("\n## Empirical findings\n\n");
    text.push_str("- Unit remap is palette indices 240..255. Vehicle source frame `k` is emitted as `rot 1+k`; the `combata` frames 0..8 progress from north toward west (counter-clockwise screen-space facings).\n");
    text.push_str("- Infantry state ranges follow the common facing-major layout: idle 0..7, six walk frames per facing at 8..55, and four fire frames per facing at 56..87. The remainder of the 176-frame sheets is prone/crawl art. The 236-frame bazooka/fremen/sardaukar sheets add 60 embedded death frames at 176..235; rifle, engineer, saboteur and thumper append their separate death sheets. Engineer and saboteur have no source fire block, so their idle block is reused for the required fire state; thumper uses five action frames per facing at 56..95.\n");
    for layout in &report.infantry_layouts {
        text.push_str(&format!("  - {layout}\n"));
    }
    text.push_str("- Structures use `ceil(frame_count/2)` healthy frames followed by the damaged remainder; common MAKE sheets are appended as `build`. Observed splits include: ");
    text.push_str(&report.structure_halves.join(", "));
    text.push_str(".\n");
    text.push_str("\n## Sound mapping\n\nSelection and confirmation slots use every available A/H/O `*SEL*` and `*CONF*` alternative for the unit voice class. Embedded WAVE mappings are: `lmg→mgun2`, `rocket→bazook1`, `75mm/90mm→medtank1`, `105mm→turret1`, `155mm→mortar1`, `227mm→missle1`, `sonic→sonic1`, `plasma→tankhvy1`, `deviator→stealth1`, `bomb→napalm1`; `explmd2`/`expllg2` and `killguy1` supply vehicle/structure and infantry deaths.\n");
    text.push_str(&format!("\nDecoded audio assets: {}.\n", report.audio_keys.len()));
    text.push_str("\n## Unresolved pack references\n\n### Shapes\n\n");
    if report.missing_shapes.is_empty() {
        text.push_str("None.\n");
    } else {
        for missing in &report.missing_shapes {
            text.push_str(&format!("- `{missing}`\n"));
        }
    }
    if !report.shape_errors.is_empty() {
        text.push_str("\n### Shape decode errors\n\n");
        for error in &report.shape_errors {
            text.push_str(&format!("- {error}\n"));
        }
    }
    if !report.audio_errors.is_empty() {
        text.push_str("\n### Audio decode errors\n\n");
        for error in &report.audio_errors {
            text.push_str(&format!("- {error}\n"));
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../cnc-d2k-convert-report.md");
    std::fs::write(&path, text)
        .map_err(|error| format!("write cnc-d2k-convert-report.md: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_d2k_generator_is_deterministic() {
        let first = generate_map(3);
        let second = generate_map(3);
        assert_eq!(first.grid, second.grid);
        assert_eq!(first, second);
        assert!(first.stages.iter().flatten().count() >= 40);
        assert!((2..=4).contains(&first.starts.len()));
    }

    #[test]
    fn cnc_import_d2k_class_painter_uses_matching_templates() {
        let table = D2kTemplateTable::embedded().unwrap();
        let available = table
            .templates()
            .iter()
            .map(|template| template.image.to_ascii_uppercase())
            .collect::<BTreeSet<_>>();
        let generated = generate_map(1);
        let (choices, multi) = paint_templates(&generated.visual_classes, &table, &available, 7).unwrap();
        assert!(multi > 0);
        for (class, choice) in generated.visual_classes.iter().zip(choices) {
            assert!(table.templates().iter().any(|template| {
                template.image.eq_ignore_ascii_case(&choice.image)
                    && if template.w == 1 && template.h == 1 {
                        template.classes.first() == Some(class)
                            && template.frames.contains(&choice.frame)
                    } else {
                        template.frames.iter().enumerate().any(|(index, &frame)| {
                            frame == choice.frame && template.classes.get(index) == Some(class)
                        })
                    }
            }));
        }
    }

    #[test]
    fn cnc_import_d2k_neighbour_mask_picks_authored_edge_and_interior_frames() {
        let table = D2kTemplateTable::parse(
            "arrakis 1 TEST 3 3 kkkkkkkkk 0,1,2,3,4,5,6,7,8\n\
             arrakis 2 TEST 3 3 kkkkkkkkk 100,101,102,103,104,105,106,107,108\n\
             arrakis 3 TEST 1 1 k 20,21\n",
        )
        .unwrap();
        let available = BTreeSet::from(["TEST".to_string()]);
        let classes = vec![b'k'; MAP_SIZE * MAP_SIZE];
        let (choices, directional) = paint_templates(&classes, &table, &available, 1).unwrap();
        assert!(directional > 0);
        assert_eq!(
            class_neighbour_mask(&classes, MAP_SIZE, MAP_SIZE, 0, 0),
            MASK_E | MASK_S
        );
        assert!(matches!(choices[0].frame, 0 | 100), "north-west corner piece");
        assert!(matches!(choices[1].frame, 1 | 101), "north edge piece");
        assert_eq!(choices[1].frame, choices[2].frame, "one edge style per plateau");
        assert!(matches!(choices[MAP_SIZE + 1].frame, 20 | 21), "one-cell interior piece");
        assert_eq!(
            choices[MAP_SIZE + 1].frame,
            choices[MAP_SIZE + 2].frame,
            "one interior style per plateau"
        );
    }

    #[test]
    fn cnc_import_d2k_house_codes_map_to_manifest_sides() {
        assert_eq!(house_side('a'), Some("atreides"));
        assert_eq!(house_side('h'), Some("harkonnen"));
        assert_eq!(house_side('o'), Some("ordos"));
        assert_eq!(house_side('x'), None);
    }

    #[test]
    fn cnc_import_d2k_sound_rs_rejects_hostile_extents() {
        assert!(sound_rs_entries(&[]).is_err());
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&9u32.to_le_bytes());
        bytes.extend_from_slice(b"x.wav\0");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        assert!(sound_rs_entries(&bytes).is_err());
    }

    #[test]
    #[ignore]
    fn convert_local_d2k_pack_if_present() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let pack = manifest.join("../../../local/packs/d2k");
        if !pack.join("orig/v2/PALETTE.BIN").is_file() {
            return;
        }
        let staged = std::env::temp_dir().join(format!("makepad-d2k-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staged);
        let assets = convert(&pack, &staged, &mut |_| {}).expect("convert local d2k pack");
        let worlds = assets
            .iter()
            .filter(|asset| asset.kind == makepad_asset_data::AssetKind::World)
            .collect::<Vec<_>>();
        assert_eq!(worlds.len(), 4);
        for world in worlds {
            let glb = staged.join(&world.rel_path);
            for extension in ["glb", "place", "grid", "png"] {
                assert!(glb.with_extension(extension).is_file(), "{}", glb.with_extension(extension).display());
            }
            let place = std::fs::read_to_string(glb.with_extension("place")).unwrap();
            let spawn = std::fs::read_to_string(glb.with_extension("spawn")).unwrap();
            assert!(place.contains("tile 32"));
            let roster_keys = place
                .lines()
                .filter_map(|line| line.strip_prefix("roster "))
                .flat_map(str::split_whitespace)
                .count();
            assert!(roster_keys >= 20, "roster keys={roster_keys}");
            assert!(place.lines().filter(|line| line.contains(" resource ")).count() >= 40);
            assert!(spawn.lines().filter(|line| line.starts_with("start start_")).count() >= 2);
        }
        assert!(staged.join("billboards/d2k/combat-a.billboard").is_file());
        assert!(staged.join("billboards/d2k/combat-a-turret.billboard").is_file());
        let rifle = std::fs::read_to_string(staged.join("billboards/d2k/rifle.billboard")).unwrap();
        for state in ["idle", "walk", "fire", "die"] {
            assert!(rifle.contains(&format!("state {state} ")));
        }
        let conyard = std::fs::read_to_string(staged.join("billboards/d2k/conyard-a.billboard")).unwrap();
        assert!(conyard.contains("footprint 3 2"));
        assert!(conyard.contains("sides=atreides"));
        assert!(staged.join("billboards/d2k/pwr-h.billboard").is_file());
        let harvester = std::fs::read_to_string(staged.join("billboards/d2k/harvester.billboard")).unwrap();
        assert!(harvester.lines().any(|line| line.starts_with("unit ") && line.contains("harvester=1")));
        for asset in &assets {
            if asset.kind == makepad_asset_data::AssetKind::Billboard {
                let text = std::fs::read_to_string(staged.join(&asset.rel_path)).unwrap();
                assert!(!text.contains("producer="), "{}", asset.rel_path);
            }
        }
        let _ = std::fs::remove_dir_all(&staged);
    }
}
