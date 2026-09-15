//! Tiberian Dawn archive interpretation on top of the verified decoders.

use super::{
    cell_to_metres, positive_unit_cost, rewrite_unit_roles, roster_key, PreviewDot, RoleTable,
    RtsEmitter, SpritePixels, SpriteSpec, SpriteState, UnitSpec, WorldSpec, CELL_M, TILE_PX,
};
use crate::classic_import::{ClassicAsset, ConvertStage, ConvertTick};
use crate::cnc_import::{
    aud::Aud, fnt::Fnt, ini::Ini, map::TdMap, mix::MixFile, pal::Pal, shp::Shp,
    tmp::Tmp, NameTable,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f32::consts::TAU;
use std::path::Path;

const TEMPLATE_TABLE_TEXT: &str = include_str!("../cnc-template-table.txt");
const VEHICLE_KEYS: &[&str] = &[
    "mtnk", "htnk", "ltnk", "ftnk", "stnk", "arty", "mlrs", "msam", "apc", "harv",
    "mcv", "jeep", "bggy", "bike",
];
const AIRCRAFT_KEYS: &[&str] = &["orca", "heli", "tran", "a10", "c17"];
const TURRET_KEYS: &[&str] = &["mtnk", "htnk", "ltnk", "mlrs", "msam"];
/// Vehicles whose SECOND 32-frame block is an animation rather than a turret.
/// MTNK's frames 32..64 are its turret; HARV's are its harvesting cycle, and
/// treating every 64-frame vehicle as a turret unit silently threw them away.
const HARVEST_KEYS: &[&str] = &["harv"];
const INFANTRY_KEYS: &[&str] = &[
    "e1", "e2", "e3", "e4", "e5", "e6", "rmbo", "c1", "c2", "c3", "c4", "c5",
    "c6", "c7", "c8", "c9", "moebius", "delphi", "chan",
];
const STRUCTURE_KEYS: &[&str] = &[
    "fact", "powr", "nuke", "proc", "silo", "pyle", "hand", "weap", "fix", "hq",
    "eye", "tmpl", "hpad", "afld", "gtwr", "atwr", "gun", "obli", "sam", "sbag",
    "cycl", "brik", "barb", "wood", "bio", "hosp", "miss", "arco",
];
const SCENERY_KEYS: &[&str] = &[
    "t01", "t02", "t03", "t04", "t05", "t06", "t07", "t08", "t09", "t10",
    "t11", "t12", "t13", "t14", "t15", "t16", "t17", "t18", "tc01", "tc02",
    "tc03", "tc04", "tc05", "rock1", "rock2", "rock3", "rock4", "rock5", "rock6",
    "rock7", "split2", "split3", "scrate", "wcrate", "flag",
];
const EFFECT_KEYS: &[&str] = &[
    "piff", "piffpiff", "fball1", "fire1", "fire2", "fire3", "napalm1", "napalm2",
    "napalm3", "art-exp1", "frag1", "frag3", "veh-hit1", "veh-hit2", "veh-hit3",
    "smokey", "atomsfx", "ionsfx", "flmspt", "bomb", "bombs",
];

const UI_SHAPES: &[(&str, &str)] = &[
    ("HSIDE1", "side_top"),
    ("HSIDE2", "side_bottom"),
    ("HSTRIP", "strip_bg"),
    ("HSTRIPUP", "strip_arrow_up"),
    ("HSTRIPDN", "strip_arrow_down"),
    ("HCLOCK", "build_clock"),
    ("HPIPS", "pips"),
    ("HPOWER", "power_bar"),
    ("HBTN-UP", "button_up"),
    ("HBTN-DN", "button_down"),
    ("HTABS", "tabs"),
    ("HREPAIR", "tab_repair"),
    ("HSELL", "tab_sell"),
    ("HMAP", "tab_map"),
    ("BTEXTURE", "side_texture"),
    ("OPTIONS", "options_button"),
    ("EARTH", "radar_earth"),
    ("POWER", "power_bar_lores"),
    ("BTN-UP", "button_up_lores"),
    ("BTN-DN", "button_down_lores"),
    ("BTN-PL", "button_place_lores"),
    ("MOUSE", "cursor"),
];

const UI_FONTS: &[&str] = &["6POINT", "8POINT", "3POINT", "LED", "VCR", "8FAT"];

// TODO(ui-art): ra/ts/d2k equivalents: RA CLOCK, EARTH, FPOWER, MOUSE,
// PIPS/PIPS2, REPAIR, TABS (this package has no hires.mix); TS GCLOCK2,
// IDLE-SIDE, IDLE-STRIP, MOUSE, PIPS/PIPS2, REPAIR in sidec01/02.mix;
// D2K sidebar artwork lives inside DATA.R8.

pub(super) const ROLE_TABLE: RoleTable = &[
    ("fact", "conyard"),
    ("powr", "power"),
    ("nuke", "power"),
    ("proc", "refinery"),
    ("silo", "silo"),
    ("pyle", "barracks"),
    ("hand", "barracks"),
    ("weap", "vehicle_factory"),
    ("afld", "vehicle_factory"),
    ("hpad", "aircraft_pad"),
    ("fix", "repair"),
    ("hq", "radar"),
    ("eye", "tech"),
    ("tmpl", "tech"),
    ("gtwr", "defense"),
    ("atwr", "defense"),
    ("gun", "defense"),
    ("obli", "defense"),
    ("sam", "defense"),
    ("sbag", "wall"),
    ("cycl", "wall"),
    ("brik", "wall"),
    ("barb", "wall"),
    ("wood", "wall"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateDef {
    theater: String,
    id: u16,
    stem: String,
    width: u8,
    height: u8,
    classes: String,
}

#[derive(Clone, Debug, Default)]
struct TemplateTable {
    by_key: BTreeMap<(String, u16), TemplateDef>,
}

impl TemplateTable {
    fn parse(text: &str) -> Result<Self, String> {
        let mut by_key = BTreeMap::new();
        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 6 {
                return Err(format!("template table line {}: expected 6 fields", line_no + 1));
            }
            let id = fields[1]
                .parse::<u16>()
                .map_err(|_| format!("template table line {}: bad id", line_no + 1))?;
            let width = fields[3]
                .parse::<u8>()
                .map_err(|_| format!("template table line {}: bad width", line_no + 1))?;
            let height = fields[4]
                .parse::<u8>()
                .map_err(|_| format!("template table line {}: bad height", line_no + 1))?;
            if fields[5].len() != width as usize * height as usize {
                return Err(format!("template table line {}: class count", line_no + 1));
            }
            let def = TemplateDef {
                theater: fields[0].to_ascii_lowercase(),
                id,
                stem: fields[2].to_ascii_lowercase(),
                width,
                height,
                classes: fields[5].into(),
            };
            by_key.insert((def.theater.clone(), id), def);
        }
        Ok(Self { by_key })
    }

    fn get(&self, theater: &str, id: u16) -> Option<&TemplateDef> {
        self.by_key.get(&(theater.to_ascii_lowercase(), id))
    }
}

struct Archives {
    conquer: Vec<u8>,
    general: Vec<u8>,
    sounds: Vec<u8>,
    speech: Vec<u8>,
    tempicnh: Vec<u8>,
    temperat: Vec<u8>,
    winter: Vec<u8>,
    desert: Vec<u8>,
    named: BTreeMap<String, IndexedEntry>,
    archive_scan: Vec<String>,
    nested_mix_count: usize,
    unnamed_aud: BTreeMap<String, usize>,
}

#[derive(Clone)]
struct IndexedEntry {
    bytes: Vec<u8>,
    location: String,
}

impl Archives {
    fn load(pack: &Path, names: &NameTable) -> Result<Self, String> {
        let (named, archive_scan, unnamed_aud, nested_mix_count) = index_all_archives(pack, names)?;
        Ok(Self {
            conquer: read_pack_file(pack, "conquer.mix")?,
            general: read_pack_file(pack, "general.mix")?,
            sounds: read_pack_file(pack, "sounds.mix")?,
            speech: read_pack_file(pack, "speech.mix")?,
            tempicnh: read_pack_file(pack, "tempicnh.mix")?,
            temperat: read_pack_file(pack, "temperat.mix")?,
            winter: read_pack_file(pack, "winter.mix")?,
            desert: read_pack_file(pack, "desert.mix")?,
            named,
            archive_scan,
            nested_mix_count,
            unnamed_aud,
        })
    }

    fn named(&self, name: &str) -> Option<&IndexedEntry> {
        self.named.get(&name.to_ascii_uppercase())
    }

    fn aud(&self, stem: &str) -> Option<(Aud, &str)> {
        let entry = self.named(&format!("{}.AUD", stem.to_ascii_uppercase()))?;
        Some((Aud::parse(&entry.bytes).ok()?, entry.location.as_str()))
    }

    fn has_aud(&self, stem: &str) -> bool {
        self.named(&format!("{}.AUD", stem.to_ascii_uppercase())).is_some()
    }
}

struct TheaterBank<'a> {
    key: &'static str,
    extension: &'static str,
    archive: &'a [u8],
    palette: Pal,
    templates: HashMap<String, Tmp>,
}

impl<'a> TheaterBank<'a> {
    fn load(theater: &str, archives: &'a Archives, table: &TemplateTable) -> Result<Self, String> {
        let (key, extension, archive, palette_name) = match theater.to_ascii_uppercase().as_str() {
            "WINTER" => ("winter", "WIN", archives.winter.as_slice(), "WINTER.PAL"),
            "DESERT" => ("desert", "DES", archives.desert.as_slice(), "DESERT.PAL"),
            _ => ("temperat", "TEM", archives.temperat.as_slice(), "TEMPERAT.PAL"),
        };
        let palette_bytes = mix_entry(archive, palette_name)
            .or_else(|| mix_entry(&archives.temperat, "TEMPERAT.PAL"))
            .ok_or_else(|| format!("{theater}: no theater palette"))?;
        let palette = Pal::parse(palette_bytes).map_err(|e| e.to_string())?;
        let mut templates = HashMap::new();
        let mut stems = table
            .by_key
            .values()
            .filter(|def| def.theater == key)
            .map(|def| def.stem.clone())
            .collect::<BTreeSet<_>>();
        stems.insert("clear1".into());
        for stem in stems {
            let name = format!("{}.{}", stem.to_ascii_uppercase(), extension);
            if let Some(bytes) = mix_entry(archive, &name) {
                if let Ok(tmp) = Tmp::parse(bytes) {
                    templates.insert(stem, tmp);
                }
            }
        }
        Ok(Self {
            key,
            extension,
            archive,
            palette,
            templates,
        })
    }

    fn shp(&self, stem: &str) -> Option<Shp> {
        let name = format!("{}.{}", stem.to_ascii_uppercase(), self.extension);
        Shp::parse(mix_entry(self.archive, &name)?).ok()
    }

    fn has_template(&self, stem: &str) -> bool {
        self.templates.contains_key(stem)
    }

    fn paint_template(&self, stem: &str, icon: usize, dst: &mut [u8]) {
        let selected = self
            .templates
            .get(stem)
            .and_then(|tmp| tmp.icon(icon))
            .or_else(|| self.templates.get("clear1").and_then(|tmp| tmp.icon(icon % 16)))
            .or_else(|| self.templates.get("clear1").and_then(|tmp| tmp.icon(0)));
        if let Some(indexed) = selected {
            indexed_opaque(indexed, &self.palette, dst);
        } else {
            for px in dst.chunks_exact_mut(4) {
                px.copy_from_slice(&[24, 24, 24, 255]);
            }
        }
    }
}

#[derive(Clone, Debug)]
struct MapSummary {
    id: String,
    title: String,
    theater: String,
    bounds: String,
    units: usize,
    structures: usize,
    scenery: usize,
    resources: usize,
    starts: usize,
}

#[derive(Clone, Debug)]
struct UiSheetSummary {
    stem: String,
    role: &'static str,
    source: String,
    frames: usize,
    frame_width: u32,
    frame_height: u32,
    cols: u32,
    sheet_width: u32,
    sheet_height: u32,
}

#[derive(Clone, Debug)]
struct UiFontSummary {
    stem: String,
    source: String,
    glyphs: usize,
    line_height: u8,
    max_width: u8,
}

#[derive(Default)]
struct ConvertReport {
    maps: Vec<MapSummary>,
    roles: BTreeMap<String, usize>,
    missing_shapes: BTreeSet<String>,
    missing_audio: BTreeSet<String>,
    audio_fallbacks: Vec<String>,
    audio_sources: BTreeMap<String, String>,
    archive_scan: Vec<String>,
    nested_mix_count: usize,
    unnamed_aud: BTreeMap<String, usize>,
    e1_frames: usize,
    structure_halves: Vec<String>,
    remap_note: String,
    ui_sheets: Vec<UiSheetSummary>,
    ui_fonts: Vec<UiFontSummary>,
    ui_scratch: Vec<String>,
}

pub fn convert(
    pack_dir: &Path,
    staged: &Path,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<Vec<ClassicAsset>, String> {
    let names = NameTable::new();
    let archives = Archives::load(pack_dir, &names)?;
    let table = TemplateTable::parse(TEMPLATE_TABLE_TEXT)?;
    let general = MixFile::parse(&archives.general).map_err(|e| e.to_string())?;
    let map_ids = names
        .names()
        .filter_map(|name| name.strip_suffix(".INI"))
        .filter(|stem| {
            let upper = stem.to_ascii_uppercase();
            upper.starts_with("SCG") || upper.starts_with("SCB") || upper.starts_with("SCM")
        })
        .filter(|stem| {
            general.by_name(&format!("{stem}.INI")).is_some()
                && general.by_name(&format!("{stem}.BIN")).is_some()
        })
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if map_ids.is_empty() {
        return Err("general.mix contains no resolved INI+BIN map pairs".into());
    }

    let mut emitter = RtsEmitter::new(staged, "cnc")?;
    let mut report = ConvertReport {
        archive_scan: archives.archive_scan.clone(),
        nested_mix_count: archives.nested_mix_count,
        unnamed_aud: archives.unnamed_aud.clone(),
        ..ConvertReport::default()
    };
    for (index, map_id) in map_ids.iter().enumerate() {
        tick(on_tick, index, map_ids.len(), format!("world {map_id}"), None);
        let ini_text = String::from_utf8_lossy(
            general
                .by_name(&format!("{map_id}.INI"))
                .ok_or_else(|| format!("{map_id}.INI disappeared"))?,
        );
        let ini = Ini::parse(&ini_text);
        let map = TdMap::parse(
            &ini,
            general
                .by_name(&format!("{map_id}.BIN"))
                .ok_or_else(|| format!("{map_id}.BIN disappeared"))?,
        )
        .map_err(|e| format!("{map_id}: {e}"))?;
        let mut theater = TheaterBank::load(&map.theater, &archives, &table)?;
        let (world, summary) = world_spec(map_id, &ini, &map, &mut theater, &table)?;
        let preview = emitter.emit_world(world)?;
        report.maps.push(summary);
        tick(
            on_tick,
            index + 1,
            map_ids.len(),
            format!("world {map_id}"),
            Some(preview),
        );
    }

    let palette = Pal::parse(
        mix_entry(&archives.temperat, "TEMPERAT.PAL").ok_or("temperat.pal missing")?,
    )
    .map_err(|e| e.to_string())?;
    report.remap_note = verify_remap_ramp(&archives.conquer, &palette);
    emit_sprites(&mut emitter, &archives, &palette, &mut report, on_tick)?;
    emit_ui_art(&mut emitter, &archives, &palette, &mut report)?;
    emit_audio(&mut emitter, &archives, &names, &mut report, on_tick)?;
    write_empirical_contact_sheets(&archives, &palette);
    write_report(&report)?;
    write_ui_art_report(&report)?;
    Ok(emitter.finish())
}

fn world_spec(
    map_id: &str,
    ini: &Ini,
    map: &TdMap,
    theater: &mut TheaterBank<'_>,
    table: &TemplateTable,
) -> Result<(WorldSpec, MapSummary), String> {
    let b = map.bounds;
    let mut grid = vec![vec!['.'; b.width as usize]; b.height as usize];
    let terrain_rgba = RtsEmitter::paint_cell_map((b.x, b.y, b.width, b.height), |x, y, tile| {
        let (template_id, source_icon) = map.cell(x as usize, y as usize);
        if template_id == 0xff {
            let icon = (x as usize % 4) + (y as usize % 4) * 4;
            theater.paint_template("clear1", icon, tile);
        } else if let Some(def) = table.get(theater.key, template_id as u16) {
            if theater.has_template(&def.stem) {
                theater.paint_template(&def.stem, source_icon as usize, tile);
            } else {
                let icon = (x as usize % 4) + (y as usize % 4) * 4;
                theater.paint_template("clear1", icon, tile);
            }
        } else {
            let icon = (x as usize % 4) + (y as usize % 4) * 4;
            theater.paint_template("clear1", icon, tile);
        }
    });
    let mut terrain_rgba = terrain_rgba;
    for y in 0..b.height {
        for x in 0..b.width {
            let (template_id, icon) = map.cell((b.x + x) as usize, (b.y + y) as usize);
            let class = if template_id == 0xff {
                'c'
            } else if let Some(def) = table.get(theater.key, template_id as u16) {
                if theater.has_template(&def.stem) {
                    def.classes
                        .as_bytes()
                        .get(icon as usize)
                        .copied()
                        .map(char::from)
                        .unwrap_or('c')
                } else {
                    'c'
                }
            } else {
                'c'
            };
            grid[y as usize][x as usize] = grid_class(class);
        }
    }

    for smudge in &map.smudges {
        if let Some((x, y)) = local_cell(b, smudge.cell) {
            if let Some(shp) = theater.shp(&smudge.name) {
                if let Some(frame) = shp.frames().first() {
                    blit_indexed(
                        &mut terrain_rgba,
                        b.width as u32 * TILE_PX,
                        b.height as u32 * TILE_PX,
                        x as u32 * TILE_PX,
                        y as u32 * TILE_PX,
                        frame,
                        shp.width() as u32,
                        shp.height() as u32,
                        &theater.palette,
                    );
                }
            }
        }
    }
    for overlay in &map.overlay {
        if !is_static_decal(&overlay.name) {
            continue;
        }
        if let Some((x, y)) = local_cell(b, overlay.cell) {
            if let Some(shp) = theater.shp(&overlay.name) {
                if let Some(frame) = shp.frames().first() {
                    blit_indexed(
                        &mut terrain_rgba,
                        b.width as u32 * TILE_PX,
                        b.height as u32 * TILE_PX,
                        x as u32 * TILE_PX,
                        y as u32 * TILE_PX,
                        frame,
                        shp.width() as u32,
                        shp.height() as u32,
                        &theater.palette,
                    );
                }
            }
        }
    }

    let mut houses = BTreeSet::new();
    houses.insert("GDI".to_string());
    houses.insert("NOD".to_string());
    for owner in map
        .units
        .iter()
        .map(|row| row.owner.as_str())
        .chain(map.infantry.iter().map(|row| row.owner.as_str()))
        .chain(map.structures.iter().map(|row| row.owner.as_str()))
    {
        houses.insert(canonical_owner(owner));
    }
    let world_key = format!("worlds/{}", map_id.to_ascii_lowercase());
    let mut place = format!(
        "world-place 1\nsource cnc\nworld {world_key}\nmode rts\ncell 6.0\ngrid {}.grid\n",
        world_key
    );
    let mut ordered_houses = houses.into_iter().collect::<Vec<_>>();
    ordered_houses.sort_by_key(|name| match name.as_str() {
        "GDI" => (0, name.clone()),
        "NOD" => (1, name.clone()),
        _ => (2, name.clone()),
    });
    for (side, house) in ordered_houses.iter().enumerate() {
        place.push_str(&format!(
            "house {house} color={} side={side}\n",
            house_color(house)
        ));
    }

    let mut dots = Vec::new();
    for unit in &map.units {
        let Some((lx, ly)) = local_cell(b, unit.cell) else { continue };
        let key = unit.kind.to_ascii_lowercase();
        let owner = canonical_owner(&unit.owner);
        let (x, z) = cell_to_metres(b.x, b.y, unit.cell);
        let class = mobile_class(&key);
        place.push_str(&format!(
            "place u-{} unit billboards/cnc/{key} {x:.4} 0.1000 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.10 class={class}\n",
            unit.number,
            source_yaw(unit.facing),
            health_fraction(unit.health),
        ));
        dots.push(PreviewDot {
            x: lx as f32 + 0.5,
            y: ly as f32 + 0.5,
            rgb: color_rgb(house_color(&owner)),
        });
    }
    for infantry in &map.infantry {
        let Some((lx, ly)) = local_cell(b, infantry.cell) else { continue };
        let key = infantry.kind.to_ascii_lowercase();
        let owner = canonical_owner(&infantry.owner);
        let (mut x, mut z) = cell_to_metres(b.x, b.y, infantry.cell);
        let (ox, oz) = subcell_offset(infantry.sub_cell);
        x += ox;
        z += oz;
        place.push_str(&format!(
            "place i-{} unit billboards/cnc/{key} {x:.4} 0.1200 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.12 class=infantry\n",
            infantry.number,
            source_yaw(infantry.facing),
            health_fraction(infantry.health),
        ));
        dots.push(PreviewDot {
            x: lx as f32 + 0.5 + ox / CELL_M,
            y: ly as f32 + 0.5 + oz / CELL_M,
            rgb: color_rgb(house_color(&owner)),
        });
    }
    for structure in &map.structures {
        let Some((lx, ly)) = local_cell(b, structure.cell) else { continue };
        let key = contract_key(&structure.kind);
        let owner = canonical_owner(&structure.owner);
        let (fw, fh) = structure_footprint(&key);
        block_rect(&mut grid, lx, ly, fw, fh, '#');
        let (x, z) = cell_to_metres(b.x, b.y, structure.cell);
        let class = if is_defense(&key) { "defense" } else { "structure" };
        place.push_str(&format!(
            "place s-{} structure billboards/cnc/{key} {x:.4} 0.0600 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.06 class={class} w={} h={}\n",
            structure.number,
            source_yaw(structure.facing),
            health_fraction(structure.health),
            fw as u32 * 6,
            fh as u32 * 6,
        ));
        dots.push(PreviewDot {
            x: lx as f32 + fw as f32 * 0.5,
            y: ly as f32 + fh as f32 * 0.5,
            rgb: color_rgb(house_color(&owner)),
        });
    }
    let mut scenery_rows = 0usize;
    for (index, terrain) in map.terrain.iter().enumerate() {
        let Some((lx, ly)) = local_cell(b, terrain.cell) else { continue };
        let key = terrain.name.to_ascii_lowercase();
        let width_cells = theater
            .shp(&terrain.name)
            .map(|shp| (shp.width() as usize).div_ceil(TILE_PX as usize).max(1))
            .unwrap_or(1);
        block_rect(&mut grid, lx, ly, width_cells, 1, '#');
        let (x, z) = cell_to_metres(b.x, b.y, terrain.cell);
        let class = if key.starts_with('t') || key.starts_with("split") { "tree" } else { "rock" };
        place.push_str(&format!(
            "place t-{index} scenery billboards/cnc/{key} {x:.4} 0.0600 {z:.4} 0.00000 align=floor layer=0.06 class={class}\n"
        ));
        scenery_rows += 1;
    }
    let mut resource_rows = 0usize;
    for (index, overlay) in map.overlay.iter().enumerate() {
        let Some((lx, ly)) = local_cell(b, overlay.cell) else { continue };
        let key = overlay.name.to_ascii_lowercase();
        let (x, z) = cell_to_metres(b.x, b.y, overlay.cell);
        if let Some(stage) = tiberium_stage(&key) {
            grid[ly][lx] = 't';
            place.push_str(&format!(
                "place r-{index} resource billboards/cnc/ti {x:.4} 0.0400 {z:.4} 0.00000 align=floor layer=0.04 class=resource stage={stage}\n"
            ));
            resource_rows += 1;
        } else if is_wall(&key) {
            grid[ly][lx] = '#';
            place.push_str(&format!(
                "place w-{index} scenery billboards/cnc/{key} {x:.4} 0.0600 {z:.4} 0.00000 align=floor layer=0.06 class=wall\n"
            ));
            scenery_rows += 1;
        } else if matches!(key.as_str(), "scrate" | "wcrate" | "flag") {
            place.push_str(&format!(
                "place o-{index} scenery billboards/cnc/{key} {x:.4} 0.0600 {z:.4} 0.00000 align=floor layer=0.06 class=scenery\n"
            ));
            scenery_rows += 1;
        }
    }

    let starts = waypoint_starts(map);
    let mut spawn = String::from("world-spawn 1\n");
    for (name, x, z) in &starts {
        spawn.push_str(&format!("start {name} {x:.4} 0.0000 {z:.4} 0.00000 -1.45000\n"));
    }
    spawn.push_str("floor 0\nstep 0.5\neye 60\n");
    let title = ini.get("Basic", "Name").unwrap_or(map_id).to_string();
    let grid = grid.into_iter().map(|row| row.into_iter().collect()).collect();
    let summary = MapSummary {
        id: map_id.into(),
        title,
        theater: map.theater.clone(),
        bounds: format!("{},{},{},{}", b.x, b.y, b.width, b.height),
        units: map.units.len() + map.infantry.len(),
        structures: map.structures.len(),
        scenery: scenery_rows,
        resources: resource_rows,
        starts: starts.len(),
    };
    Ok((
        WorldSpec {
            key: map_id.to_ascii_lowercase(),
            width: b.width,
            height: b.height,
            terrain_rgba,
            grid,
            place_text: place,
            roster: pack_roster(),
            spawn_text: spawn,
            preview_dots: dots,
            preview_crop: None,
            tags: vec![
                "cnc".into(),
                "world".into(),
                "rts".into(),
                map.theater.to_ascii_lowercase(),
            ],
        },
        summary,
    ))
}

#[derive(Clone, Copy)]
struct UnitManifest {
    key: &'static str,
    line: &'static str,
    weapon: Option<&'static str>,
    weapon2: Option<&'static str>,
}

const UNIT_MANIFESTS: &[UnitManifest] = &[
    UnitManifest { key: "e1", line: "unit class=infantry title=\"Minigunner\" cost=100 hp=50 armor=none speed=2.5 sight=12 sides=GDI,NOD producer=pyle|hand weapon=m16", weapon: Some("m16"), weapon2: None },
    UnitManifest { key: "e2", line: "unit class=infantry title=\"Grenadier\" cost=160 hp=50 armor=none speed=3.0 sight=12 sides=GDI producer=pyle weapon=grenade", weapon: Some("grenade"), weapon2: None },
    UnitManifest { key: "e3", line: "unit class=infantry title=\"Rocket Soldier\" cost=300 hp=25 armor=none speed=2.5 sight=12 sides=GDI,NOD producer=pyle|hand weapon=dragon", weapon: Some("dragon"), weapon2: None },
    UnitManifest { key: "e4", line: "unit class=infantry title=\"Flamethrower\" cost=200 hp=70 armor=none speed=2.5 sight=12 sides=NOD producer=hand weapon=flamer", weapon: Some("flamer"), weapon2: None },
    UnitManifest { key: "e5", line: "unit class=infantry title=\"Chem Warrior\" cost=300 hp=70 armor=none speed=2.5 sight=12 sides=NOD producer=hand prereq=tmpl weapon=chemspray", weapon: Some("chemspray"), weapon2: None },
    UnitManifest { key: "e6", line: "unit class=infantry title=\"Engineer\" cost=500 hp=25 armor=none speed=2.5 sight=12 sides=GDI,NOD producer=pyle|hand capture=1", weapon: None, weapon2: None },
    UnitManifest { key: "rmbo", line: "unit class=infantry title=\"Commando\" cost=1000 hp=80 armor=none speed=3.0 sight=18 sides=GDI,NOD producer=pyle|hand prereq=eye|tmpl weapon=sniper", weapon: Some("sniper"), weapon2: None },
    UnitManifest { key: "jeep", line: "unit class=vehicle title=\"Humvee\" cost=400 hp=150 armor=light speed=10.0 sight=18 sides=GDI producer=weap weapon=m60", weapon: Some("m60"), weapon2: None },
    UnitManifest { key: "bggy", line: "unit class=vehicle title=\"Buggy\" cost=300 hp=140 armor=light speed=10.0 sight=18 sides=NOD producer=afld weapon=m60", weapon: Some("m60"), weapon2: None },
    UnitManifest { key: "bike", line: "unit class=vehicle title=\"Recon Bike\" cost=500 hp=160 armor=light speed=13.0 sight=18 sides=NOD producer=afld weapon=dragon", weapon: Some("dragon"), weapon2: None },
    UnitManifest { key: "ltnk", line: "unit class=vehicle title=\"Light Tank\" cost=600 hp=300 armor=heavy speed=6.5 sight=18 sides=NOD producer=afld weapon=75mm turret=billboards/cnc/ltnk-turret", weapon: Some("75mm"), weapon2: None },
    UnitManifest { key: "mtnk", line: "unit class=vehicle title=\"Medium Tank\" cost=800 hp=400 armor=heavy speed=6.0 sight=18 sides=GDI producer=weap weapon=105mm turret=billboards/cnc/mtnk-turret", weapon: Some("105mm"), weapon2: None },
    UnitManifest { key: "htnk", line: "unit class=vehicle title=\"Mammoth Tank\" cost=1500 hp=600 armor=heavy speed=4.0 sight=18 sides=GDI producer=weap prereq=fix weapon=120mm weapon2=mammoth_tusk turret=billboards/cnc/htnk-turret", weapon: Some("120mm"), weapon2: Some("mammoth_tusk") },
    UnitManifest { key: "ftnk", line: "unit class=vehicle title=\"Flame Tank\" cost=800 hp=300 armor=heavy speed=6.0 sight=18 sides=NOD producer=afld weapon=flamer", weapon: Some("flamer"), weapon2: None },
    UnitManifest { key: "stnk", line: "unit class=vehicle title=\"Stealth Tank\" cost=900 hp=110 armor=light speed=10.0 sight=24 sides=NOD producer=afld prereq=tmpl weapon=dragon", weapon: Some("dragon"), weapon2: None },
    UnitManifest { key: "arty", line: "unit class=vehicle title=\"Artillery\" cost=450 hp=75 armor=light speed=6.0 sight=24 sides=NOD producer=afld weapon=155mm", weapon: Some("155mm"), weapon2: None },
    UnitManifest { key: "mlrs", line: "unit class=vehicle title=\"Rocket Launcher\" cost=800 hp=100 armor=light speed=6.0 sight=24 sides=GDI producer=weap prereq=hq weapon=227mm turret=billboards/cnc/mlrs-turret", weapon: Some("227mm"), weapon2: None },
    UnitManifest { key: "msam", line: "unit class=vehicle title=\"SSM Launcher\" cost=750 hp=120 armor=light speed=5.0 sight=24 sides=NOD producer=afld prereq=tmpl weapon=honest_john turret=billboards/cnc/msam-turret", weapon: Some("honest_john"), weapon2: None },
    UnitManifest { key: "apc", line: "unit class=vehicle title=\"APC\" cost=700 hp=200 armor=heavy speed=9.0 sight=18 sides=GDI producer=weap prereq=pyle weapon=m60", weapon: Some("m60"), weapon2: None },
    UnitManifest { key: "harv", line: "unit class=vehicle title=\"Harvester\" cost=1400 hp=600 armor=heavy speed=5.0 sight=12 sides=GDI,NOD producer=weap|afld prereq=proc harvester=1 capacity=700", weapon: None, weapon2: None },
    UnitManifest { key: "mcv", line: "unit class=vehicle title=\"MCV\" cost=5000 hp=600 armor=heavy speed=4.0 sight=12 sides=GDI,NOD producer=weap|afld mcv=1 deploys=fact", weapon: None, weapon2: None },
    UnitManifest { key: "orca", line: "unit class=aircraft title=\"Orca\" cost=1200 hp=125 armor=light speed=18.0 sight=24 sides=GDI producer=hpad weapon=dragon", weapon: Some("dragon"), weapon2: None },
    UnitManifest { key: "heli", line: "unit class=aircraft title=\"Apache\" cost=1200 hp=125 armor=light speed=16.0 sight=24 sides=NOD producer=hpad weapon=chaingun", weapon: Some("chaingun"), weapon2: None },
    UnitManifest { key: "fact", line: "unit class=structure title=\"Construction Yard\" cost=5000 hp=400 armor=concrete speed=0 sight=30 sides=GDI,NOD footprint=3x2 power=0 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "powr", line: "unit class=structure title=\"Power Plant\" cost=300 hp=200 armor=wood speed=0 sight=18 sides=GDI,NOD producer=fact footprint=2x2 power=+100 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "nuke", line: "unit class=structure title=\"Adv. Power Plant\" cost=700 hp=300 armor=wood speed=0 sight=18 sides=GDI,NOD producer=fact prereq=powr footprint=2x2 power=+200 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "proc", line: "unit class=structure title=\"Tiberium Refinery\" cost=2000 hp=450 armor=wood speed=0 sight=24 sides=GDI,NOD producer=fact prereq=powr footprint=3x2 power=-40 refinery=1 grants=harv build=1", weapon: None, weapon2: None },
    UnitManifest { key: "silo", line: "unit class=structure title=\"Tiberium Silo\" cost=150 hp=150 armor=wood speed=0 sight=12 sides=GDI,NOD producer=fact prereq=proc footprint=2x1 power=-10 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "pyle", line: "unit class=structure title=\"Barracks\" cost=300 hp=400 armor=wood speed=0 sight=18 sides=GDI producer=fact prereq=powr footprint=2x2 power=-20 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "hand", line: "unit class=structure title=\"Hand of Nod\" cost=300 hp=400 armor=wood speed=0 sight=18 sides=NOD producer=fact prereq=powr footprint=2x3 power=-20 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "weap", line: "unit class=structure title=\"Weapons Factory\" cost=2000 hp=200 armor=light speed=0 sight=18 sides=GDI producer=fact prereq=proc footprint=3x3 power=-30 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "afld", line: "unit class=structure title=\"Airstrip\" cost=2000 hp=500 armor=heavy speed=0 sight=18 sides=NOD producer=fact prereq=proc footprint=4x2 power=-30 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "fix", line: "unit class=structure title=\"Repair Facility\" cost=1200 hp=400 armor=wood speed=0 sight=18 sides=GDI,NOD producer=fact prereq=weap|afld footprint=3x3 power=-30 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "hq", line: "unit class=structure title=\"Communications Center\" cost=1000 hp=500 armor=wood speed=0 sight=30 sides=GDI,NOD producer=fact prereq=proc footprint=2x2 power=-40 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "eye", line: "unit class=structure title=\"Adv. Comm Center\" cost=2800 hp=500 armor=concrete speed=0 sight=30 sides=GDI producer=fact prereq=hq footprint=2x2 power=-200 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "tmpl", line: "unit class=structure title=\"Temple of Nod\" cost=3000 hp=1000 armor=concrete speed=0 sight=30 sides=NOD producer=fact prereq=hq footprint=3x3 power=-150 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "hpad", line: "unit class=structure title=\"Helipad\" cost=1500 hp=400 armor=wood speed=0 sight=18 sides=GDI,NOD producer=fact prereq=hq footprint=2x2 power=-10 build=1", weapon: None, weapon2: None },
    UnitManifest { key: "gtwr", line: "unit class=defense title=\"Guard Tower\" cost=500 hp=200 armor=wood speed=0 sight=24 sides=GDI producer=fact prereq=pyle footprint=1x1 power=-10 weapon=m60 build=1", weapon: Some("m60"), weapon2: None },
    UnitManifest { key: "atwr", line: "unit class=defense title=\"Adv. Guard Tower\" cost=1000 hp=300 armor=concrete speed=0 sight=30 sides=GDI producer=fact prereq=hq footprint=1x1 power=-20 weapon=tower_rockets build=1", weapon: Some("tower_rockets"), weapon2: None },
    UnitManifest { key: "gun", line: "unit class=defense title=\"Gun Turret\" cost=600 hp=200 armor=heavy speed=0 sight=24 sides=NOD producer=fact prereq=hand footprint=1x1 power=-20 weapon=75mm turret=billboards/cnc/gun-turret build=1", weapon: Some("75mm"), weapon2: None },
    UnitManifest { key: "obli", line: "unit class=defense title=\"Obelisk of Light\" cost=1500 hp=200 armor=concrete speed=0 sight=36 sides=NOD producer=fact prereq=hq footprint=1x1 power=-150 weapon=laser build=1", weapon: Some("laser"), weapon2: None },
    UnitManifest { key: "sam", line: "unit class=defense title=\"SAM Site\" cost=750 hp=200 armor=heavy speed=0 sight=30 sides=NOD producer=fact prereq=hand footprint=2x1 power=-20 weapon=nike build=1", weapon: Some("nike"), weapon2: None },
    UnitManifest { key: "sbag", line: "unit class=structure title=\"Sandbag Wall\" cost=50 hp=50 armor=concrete speed=0 sight=0 sides=GDI,NOD producer=fact prereq=pyle|hand footprint=1x1 wall=1", weapon: None, weapon2: None },
    UnitManifest { key: "cycl", line: "unit class=structure title=\"Chain Link\" cost=75 hp=75 armor=concrete speed=0 sight=0 sides=GDI,NOD producer=fact prereq=pyle|hand footprint=1x1 wall=1", weapon: None, weapon2: None },
    UnitManifest { key: "brik", line: "unit class=structure title=\"Concrete Wall\" cost=100 hp=100 armor=concrete speed=0 sight=0 sides=GDI,NOD producer=fact prereq=hq footprint=1x1 wall=1", weapon: None, weapon2: None },
];

fn weapon_line(id: &str) -> Option<&'static str> {
    Some(match id {
        "m16" => "weapon id=m16 damage=15 rate=3.0 range=18 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/cnc/piff fire=sfx/cnc/gun8 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "m60" => "weapon id=m60 damage=15 rate=4.0 range=24 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/cnc/piff fire=sfx/cnc/mgun11 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "chaingun" => "weapon id=chaingun damage=25 rate=5.0 range=24 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/cnc/piffpiff fire=sfx/cnc/mgun2 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "grenade" => "weapon id=grenade damage=50 rate=0.7 range=24 delivery=projectile projectile_speed=10 splash_radius=6 splash_damage=30 projectile_sprite=billboards/cnc/bomblet impact=billboards/cnc/frag1 versus=none:1,wood:.75,light:.5,heavy:.3,concrete:.5",
        "dragon" => "weapon id=dragon damage=30 rate=0.8 range=27 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/cnc/dragon impact=billboards/cnc/veh-hit1 fire=sfx/cnc/rocket1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "flamer" => "weapon id=flamer damage=35 rate=1.5 range=12 delivery=projectile projectile_speed=12 splash_radius=4 splash_damage=20 impact=billboards/cnc/fire1 fire=sfx/cnc/flamer2 versus=none:1,wood:.9,light:.6,heavy:.3,concrete:.3",
        "chemspray" => "weapon id=chemspray damage=40 rate=1.5 range=12 delivery=projectile projectile_speed=12 splash_radius=4 splash_damage=20 impact=billboards/cnc/fire1 fire=sfx/cnc/flamer2 versus=none:1,wood:.9,light:.6,heavy:.3,concrete:.3",
        "sniper" => "weapon id=sniper damage=125 rate=0.5 range=30 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/cnc/piff fire=sfx/cnc/ramgun versus=none:1,wood:.1,light:.05,heavy:.02,concrete:.02",
        "75mm" => "weapon id=75mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 projectile_sprite=billboards/cnc/120mm impact=billboards/cnc/veh-hit2 fire=sfx/cnc/tnkfire3 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "105mm" => "weapon id=105mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 projectile_sprite=billboards/cnc/120mm impact=billboards/cnc/veh-hit2 fire=sfx/cnc/tnkfire3 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "120mm" => "weapon id=120mm damage=40 rate=0.8 range=28 delivery=projectile projectile_speed=30 splash_radius=3 splash_damage=15 projectile_sprite=billboards/cnc/120mm impact=billboards/cnc/veh-hit3 fire=sfx/cnc/tnkfire6 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "mammoth_tusk" => "weapon id=mammoth_tusk damage=40 rate=0.5 range=36 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/cnc/dragon impact=billboards/cnc/veh-hit1 fire=sfx/cnc/rocket2 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "155mm" => "weapon id=155mm damage=60 rate=0.3 range=36 delivery=projectile projectile_speed=18 splash_radius=6 splash_damage=40 projectile_sprite=billboards/cnc/120mm impact=billboards/cnc/art-exp1 fire=sfx/cnc/tnkfire4 versus=none:.9,wood:.9,light:.6,heavy:.4,concrete:.9",
        "227mm" => "weapon id=227mm damage=60 rate=0.4 range=45 delivery=projectile projectile_speed=20 splash_radius=6 splash_damage=40 projectile_sprite=billboards/cnc/dragon impact=billboards/cnc/art-exp1 fire=sfx/cnc/rocket2 versus=none:.9,wood:.9,light:.6,heavy:.4,concrete:.9",
        "honest_john" => "weapon id=honest_john damage=120 rate=0.15 range=60 delivery=projectile projectile_speed=14 splash_radius=12 splash_damage=80 projectile_sprite=billboards/cnc/missile impact=billboards/cnc/napalm1 fire=sfx/cnc/rocket2 versus=none:1,wood:1,light:.8,heavy:.7,concrete:1",
        "tower_rockets" => "weapon id=tower_rockets damage=40 rate=1.0 range=36 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/cnc/dragon impact=billboards/cnc/veh-hit1 fire=sfx/cnc/rocket1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "laser" => "weapon id=laser damage=200 rate=0.2 range=45 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 fire=sfx/cnc/laser versus=none:1,wood:1,light:1,heavy:1,concrete:.8",
        "nike" => "weapon id=nike damage=50 rate=1.0 range=45 delivery=projectile projectile_speed=30 splash_radius=3 splash_damage=15 projectile_sprite=billboards/cnc/missile impact=billboards/cnc/veh-hit1 fire=sfx/cnc/rocket1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5 anti_air=1",
        _ => return None,
    })
}

fn emit_sprites(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let remap = remap_line(palette, 176);
    let total = VEHICLE_KEYS.len() + AIRCRAFT_KEYS.len() + INFANTRY_KEYS.len() + STRUCTURE_KEYS.len() + 37;
    let mut done = 0usize;
    for &key in VEHICLE_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = conquer_shp(archives, key) else {
            report.missing_shapes.insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        if shp.frames().len() < 32 {
            report.missing_shapes.insert(format!("{}.SHP (needs 32 hull frames)", key.to_ascii_uppercase()));
            continue;
        }
        let mut frames = frame_range(&shp, 0, 32, palette, facing_rot);
        let mut states =
            vec![SpriteState { name: "idle", first: 0, last: 32, looping: true, fps: 8 }];
        if HARVEST_KEYS.contains(&key) && shp.frames().len() >= 64 {
            let first = frames.len();
            append_harvest_frames(&mut frames, &shp, palette);
            if frames.len() > first {
                states.push(SpriteState {
                    name: "harvest",
                    first,
                    last: frames.len(),
                    looping: true,
                    fps: 8,
                });
            } else {
                report.missing_shapes.insert(format!(
                    "{} harvest frames",
                    key.to_ascii_uppercase()
                ));
            }
        }
        let lines = unit_lines(key, archives, &remap);
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "unit", facings: 32, frames,
            states,
            unit: Some(UnitSpec { manifest_lines: lines }),
            manifest_lines: Vec::new(), tags: vec!["cnc", "unit", "vehicle"],
        })?;
        if TURRET_KEYS.contains(&key) && shp.frames().len() >= 64 {
            let frames = frame_range(&shp, 32, 64, palette, facing_rot);
            emit_spec(emitter, report, SpriteSpec {
                key: format!("{key}-turret"), role: "unit", facings: 32, frames,
                states: vec![SpriteState { name: "idle", first: 0, last: 32, looping: true, fps: 8 }],
                unit: None,
                manifest_lines: vec![remap.clone()], tags: vec!["cnc", "unit", "turret"],
            })?;
        } else if TURRET_KEYS.contains(&key) {
            report.missing_shapes.insert(format!("{} turret frames", key.to_ascii_uppercase()));
        }
        emit_icon(emitter, archives, palette, key, report)?;
    }
    for &key in AIRCRAFT_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = conquer_shp(archives, key) else {
            report.missing_shapes.insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len().min(32);
        let facings = if count == 32 { 32 } else { 1 };
        let frames = frame_range(&shp, 0, count, palette, |i| if facings == 32 { facing_rot(i) } else { 0 });
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "unit", facings, frames,
            states: vec![SpriteState { name: "idle", first: 0, last: count, looping: true, fps: 8 }],
            unit: Some(UnitSpec { manifest_lines: unit_lines(key, archives, &remap) }),
            manifest_lines: Vec::new(), tags: vec!["cnc", "unit", "aircraft"],
        })?;
        emit_icon(emitter, archives, palette, key, report)?;
    }
    for (key, source) in [("lst", "lst"), ("boat", "boat")] {
        if let Some(shp) = conquer_shp(archives, source) {
            let count = shp.frames().len().min(32).max(1);
            let facings = if count == 32 { 32 } else { 1 };
            let frames = frame_range(&shp, 0, count, palette, |i| if facings == 32 { facing_rot(i) } else { 0 });
            emit_spec(emitter, report, SpriteSpec {
                key: key.into(), role: "unit", facings, frames,
                states: vec![SpriteState { name: "idle", first: 0, last: count, looping: true, fps: 8 }],
                unit: None,
                manifest_lines: vec![remap.clone()], tags: vec!["cnc", "unit", "boat"],
            })?;
        }
    }

    for &key in INFANTRY_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = conquer_shp(archives, key) else {
            report.missing_shapes.insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        if key == "e1" {
            report.e1_frames = shp.frames().len();
        }
        let mut frames = Vec::new();
        append_frames(&mut frames, &shp, 0, 8, palette, facing_rot);
        let idle_end = frames.len();
        append_frames(&mut frames, &shp, 16, 64, palette, |i| facing_rot(i / 6));
        let walk_end = frames.len();
        append_frames(&mut frames, &shp, 64, 128, palette, |i| facing_rot(i / 8));
        let fire_end = frames.len();
        let death_start = if shp.frames().len() >= 406 { 398 } else { shp.frames().len().saturating_sub(8) };
        append_frames(&mut frames, &shp, death_start, death_start + 8, palette, |_| 0);
        let die_end = frames.len();
        if idle_end != 8 || walk_end - idle_end != 48 || fire_end - walk_end != 64 || die_end - fire_end != 8 {
            report.missing_shapes.insert(format!("{}.SHP standard infantry ranges", key.to_ascii_uppercase()));
            continue;
        }
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "unit", facings: 8, frames,
            states: vec![
                SpriteState { name: "idle", first: 0, last: idle_end, looping: true, fps: 8 },
                SpriteState { name: "walk", first: idle_end, last: walk_end, looping: true, fps: 10 },
                SpriteState { name: "fire", first: walk_end, last: fire_end, looping: false, fps: 10 },
                SpriteState { name: "die", first: fire_end, last: die_end, looping: false, fps: 8 },
            ],
            unit: Some(UnitSpec { manifest_lines: unit_lines(key, archives, &remap) }),
            manifest_lines: Vec::new(), tags: vec!["cnc", "unit", "infantry"],
        })?;
        emit_icon(emitter, archives, palette, key, report)?;
    }

    for &key in STRUCTURE_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let source = structure_source(key);
        let Some(shp) = conquer_shp(archives, source) else {
            report.missing_shapes.insert(format!("{}.SHP", source.to_ascii_uppercase()));
            continue;
        };
        if is_wall(key) {
            let count = shp.frames().len();
            if count == 0 {
                continue;
            }
            let frames = frame_range(&shp, 0, count, palette, |_| 0);
            let mut lines = unit_lines(key, archives, &remap);
            lines.push("footprint 1 1".into());
            emit_spec(emitter, report, SpriteSpec {
                key: key.into(), role: "structure", facings: 1, frames,
                states: super::wall_states(count),
                unit: Some(UnitSpec { manifest_lines: lines }),
                manifest_lines: Vec::new(), tags: vec!["cnc", "structure", "wall"],
            })?;
            emit_icon(emitter, archives, palette, key, report)?;
            continue;
        }
        let base_count = shp.frames().len();
        let mut frames = frame_range(&shp, 0, base_count, palette, |_| 0);
        let build_first = frames.len();
        let make_stem = format!("{source}make");
        if let Some(make) = conquer_shp(archives, &make_stem) {
            append_frames(&mut frames, &make, 0, make.frames().len(), palette, |_| 0);
        } else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", make_stem.to_ascii_uppercase()));
        }
        let build_last = frames.len();
        let (fw, fh) = structure_footprint(key);
        let (healthy, damaged_end) = structure_split(base_count);
        // A building STANDS STILL. Its extra frames are the animation it plays
        // while it is working — a construction yard building, a refinery taking
        // a load — not a loop it runs forever. Publishing the whole healthy run
        // as `idle` made every structure on the map churn through its animation
        // continuously; the resting picture is frame 0 and the run is its own
        // clip the engine asks for by name.
        let mut states = vec![
            SpriteState { name: "idle", first: 0, last: 1, looping: false, fps: 6 },
            SpriteState { name: "damaged", first: healthy, last: healthy + 1, looping: false, fps: 6 },
        ];
        if healthy > 1 {
            states.push(SpriteState { name: "active", first: 0, last: healthy, looping: true, fps: 6 });
        }
        if damaged_end > healthy + 1 {
            states.push(SpriteState {
                name: "damaged_active",
                first: healthy,
                last: damaged_end,
                looping: true,
                fps: 6,
            });
        }
        if damaged_end < base_count {
            // The rubble frame, under the name the engine already asks for
            // when a piece is destroyed.
            states.push(SpriteState { name: "die", first: damaged_end, last: base_count, looping: false, fps: 6 });
        }
        if build_last > build_first {
            states.push(SpriteState { name: "build", first: build_first, last: build_last, looping: false, fps: 15 });
        }
        report.structure_halves.push(format!(
            "{key}: {healthy}+{}+{}",
            damaged_end - healthy,
            base_count - damaged_end
        ));
        let mut lines = unit_lines(key, archives, &remap);
        lines.push(format!("footprint {fw} {fh}"));
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "structure", facings: 1, frames, states,
            unit: Some(UnitSpec { manifest_lines: lines }),
            manifest_lines: Vec::new(), tags: vec!["cnc", "structure"],
        })?;
        if key == "gun" && base_count >= 32 {
            let frames = frame_range(&shp, 0, 32, palette, facing_rot);
            emit_spec(emitter, report, SpriteSpec {
                key: "gun-turret".into(), role: "unit", facings: 32, frames,
                states: vec![SpriteState { name: "idle", first: 0, last: 32, looping: true, fps: 8 }],
                unit: None,
                manifest_lines: vec![remap.clone()], tags: vec!["cnc", "unit", "turret"],
            })?;
        }
        emit_icon(emitter, archives, palette, key, report)?;
    }

    let table = TemplateTable::parse(TEMPLATE_TABLE_TEXT)?;
    let temperate = TheaterBank::load("TEMPERATE", archives, &table)?;
    let desert = TheaterBank::load("DESERT", archives, &table)?;
    let winter = TheaterBank::load("WINTER", archives, &table)?;
    for number in 1..=37 {
        let key = format!("v{number:02}");
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let shp = temperate.shp(&key).or_else(|| desert.shp(&key)).or_else(|| winter.shp(&key));
        let Some(shp) = shp else {
            report.missing_shapes.insert(format!("{}.TEM", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len();
        let (healthy, damaged_end) = structure_split(count);
        let mut states = vec![
            SpriteState { name: "idle", first: 0, last: 1, looping: false, fps: 6 },
            SpriteState { name: "damaged", first: healthy, last: healthy + 1, looping: false, fps: 6 },
        ];
        if healthy > 1 {
            states.push(SpriteState { name: "active", first: 0, last: healthy, looping: true, fps: 6 });
        }
        if damaged_end < count {
            states.push(SpriteState { name: "die", first: damaged_end, last: count, looping: false, fps: 6 });
        }
        emit_spec(emitter, report, SpriteSpec {
            key: key.clone(), role: "structure", facings: 1,
            frames: frame_range(&shp, 0, count, palette, |_| 0),
            states,
            unit: None,
            manifest_lines: vec![remap.clone(), "footprint 1 1".into()], tags: vec!["cnc", "structure", "civilian"],
        })?;
    }

    for &key in SCENERY_KEYS {
        let shp = temperate
            .shp(key)
            .or_else(|| desert.shp(key))
            .or_else(|| winter.shp(key))
            .or_else(|| conquer_shp(archives, key));
        let Some(shp) = shp else {
            report.missing_shapes.insert(format!("{} theater SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = if matches!(key, "scrate" | "wcrate" | "flag") { shp.frames().len().min(1) } else { shp.frames().len().min(2) };
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "scenery", facings: 1,
            frames: frame_range(&shp, 0, count, palette, |_| 0),
            states: vec![SpriteState { name: "idle", first: 0, last: count, looping: false, fps: 6 }],
            unit: None,
            manifest_lines: vec![remap.clone()], tags: vec!["cnc", "scenery"],
        })?;
    }
    emit_tiberium(emitter, &temperate, palette, report, &remap)?;

    for &key in EFFECT_KEYS {
        let Some(shp) = conquer_shp(archives, key) else {
            report.missing_shapes.insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len();
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "effect", facings: 1,
            frames: frame_range(&shp, 0, count, palette, |_| 0),
            states: vec![SpriteState { name: "idle", first: 0, last: count, looping: false, fps: 15 }],
            unit: None,
            manifest_lines: vec![remap.clone()], tags: vec!["cnc", "effect"],
        })?;
    }
    for (key, source) in [
        ("120mm", "120mm"), ("50cal", "50cal"), ("dragon", "dragon"),
        ("bomblet", "bomblet"), ("flame", "flame-n"), ("missile", "missile"),
        ("patriot", "patriot"), ("laser", "laser"),
    ] {
        let Some(shp) = conquer_shp(archives, source) else {
            report.missing_shapes.insert(format!("{}.SHP", source.to_ascii_uppercase()));
            continue;
        };
        let (count, facings) = if shp.frames().len() >= 32 { (32, 32) } else { (1, 1) };
        emit_spec(emitter, report, SpriteSpec {
            key: key.into(), role: "effect", facings,
            frames: frame_range(&shp, 0, count, palette, |i| if facings == 32 { facing_rot(i) } else { 0 }),
            states: vec![SpriteState { name: "idle", first: 0, last: count, looping: false, fps: 15 }],
            unit: None,
            manifest_lines: vec![remap.clone()], tags: vec!["cnc", "effect", "projectile"],
        })?;
    }
    Ok(())
}

struct UiIndexedFrames {
    width: u32,
    height: u32,
    frames: Vec<Vec<u8>>,
}

fn emit_ui_art(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    report: &mut ConvertReport,
) -> Result<(), String> {
    let mut manifest = String::from("ui-sheets 1\n");
    for &(stem, role) in UI_SHAPES {
        let source_name = format!("{stem}.SHP");
        let entry = archives
            .named(&source_name)
            .ok_or_else(|| format!("required UI shape {source_name} is missing"))?;
        let decoded = if stem == "MOUSE" {
            decode_mouse_shp(&entry.bytes)?
        } else {
            let shp = Shp::parse(&entry.bytes)
                .map_err(|e| format!("{source_name} from {}: {e}", entry.location))?;
            UiIndexedFrames {
                width: shp.width() as u32,
                height: shp.height() as u32,
                frames: shp.frames().to_vec(),
            }
        };
        let cols = (decoded.frames.len() as u32).clamp(1, 16);
        let (rgba, sheet_width, sheet_height) = pack_ui_sheet(
            &decoded.frames,
            decoded.width,
            decoded.height,
            cols,
            palette,
        )?;
        let output_stem = stem.to_ascii_lowercase();
        let key = format!("ui/cnc/{output_stem}");
        emitter.emit_texture(
            &key,
            &rgba,
            sheet_width,
            sheet_height,
            &["cnc", "ui"],
        )?;
        manifest.push_str(&format!(
            "sheet {output_stem} file={key}.png frames={} w={} h={} cols={cols} role={role}\n",
            decoded.frames.len(), decoded.width, decoded.height,
        ));
        report.ui_sheets.push(UiSheetSummary {
            stem: output_stem,
            role,
            source: entry.location.clone(),
            frames: decoded.frames.len(),
            frame_width: decoded.width,
            frame_height: decoded.height,
            cols,
            sheet_width,
            sheet_height,
        });
    }
    emitter.emit_source(
        "ui/cnc/sidebar",
        "ui/cnc/sidebar.ui",
        &manifest,
        &["cnc", "ui"],
    )?;

    let scratch_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../local/agent_state/cnc");
    std::fs::create_dir_all(&scratch_root)
        .map_err(|e| format!("create {}: {e}", scratch_root.display()))?;
    for &stem in UI_FONTS {
        let source_name = format!("{stem}.FNT");
        let entry = archives
            .named(&source_name)
            .ok_or_else(|| format!("required UI font {source_name} is missing"))?;
        let font = Fnt::parse(&entry.bytes)
            .map_err(|e| format!("{source_name} from {}: {e}", entry.location))?;
        if matches!(stem, "8POINT" | "LED") {
            validate_empirical_ascii(stem, &font)?;
        }
        let output_stem = stem.to_ascii_lowercase();
        let key = format!("ui/cnc/font-{output_stem}");
        let (rgba, width, height, font_manifest) = pack_font_sheet(&key, &font)?;
        emitter.emit_source_with_texture(
            &key,
            &format!("{key}.font"),
            &font_manifest,
            &format!("{key}.png"),
            &rgba,
            width,
            height,
            &["cnc", "ui", "font"],
        )?;
        report.ui_fonts.push(UiFontSummary {
            stem: output_stem.clone(),
            source: entry.location.clone(),
            glyphs: font.glyphs().len(),
            line_height: font.line_height(),
            max_width: font.max_width(),
        });
        if matches!(stem, "8POINT" | "LED") {
            let (preview, preview_width, preview_height) =
                render_font_sample(&font, b"0123456789 ABC", 4)?;
            let rel = format!("local/agent_state/cnc/font-{output_stem}-sample.png");
            let png = crate::classic_import::encode_png_rgba(
                &preview,
                preview_width,
                preview_height,
            )?;
            std::fs::write(scratch_root.join(format!("font-{output_stem}-sample.png")), png)
                .map_err(|e| format!("write {rel}: {e}"))?;
            report.ui_scratch.push(rel);
        }
    }
    Ok(())
}

fn pack_ui_sheet(
    frames: &[Vec<u8>],
    frame_width: u32,
    frame_height: u32,
    cols: u32,
    palette: &Pal,
) -> Result<(Vec<u8>, u32, u32), String> {
    if frames.is_empty() || frame_width == 0 || frame_height == 0 || cols == 0 {
        return Err("UI sheet has no frames or dimensions".into());
    }
    let frame_pixels = frame_width
        .checked_mul(frame_height)
        .ok_or("UI frame dimensions overflow")? as usize;
    if frames.iter().any(|frame| frame.len() != frame_pixels) {
        return Err("UI frame dimensions do not match decoded pixels".into());
    }
    let rows = (frames.len() as u32).div_ceil(cols);
    let width = frame_width.checked_mul(cols).ok_or("UI sheet width overflow")?;
    let height = frame_height.checked_mul(rows).ok_or("UI sheet height overflow")?;
    let sheet_pixels = width
        .checked_mul(height)
        .filter(|&pixels| pixels <= 16 * 1024 * 1024)
        .ok_or("UI sheet is too large")?;
    let byte_len = sheet_pixels.checked_mul(4).ok_or("UI sheet byte size overflow")?;
    let mut rgba = vec![0u8; byte_len as usize];
    for (index, frame) in frames.iter().enumerate() {
        let ox = index as u32 % cols * frame_width;
        let oy = index as u32 / cols * frame_height;
        for y in 0..frame_height {
            for x in 0..frame_width {
                let shade = frame[(y * frame_width + x) as usize];
                if shade == 0 {
                    continue;
                }
                let [r, g, b] = palette.rgb(shade);
                let at = (((oy + y) * width + ox + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    Ok((rgba, width, height))
}

fn pack_font_sheet(key: &str, font: &Fnt) -> Result<(Vec<u8>, u32, u32, String), String> {
    let cols = 16u32;
    let rows = (font.glyphs().len() as u32).div_ceil(cols);
    let width = cols
        .checked_mul(font.max_width() as u32)
        .ok_or("FNT sheet width overflow")?;
    let height = rows
        .checked_mul(font.line_height() as u32)
        .ok_or("FNT sheet height overflow")?;
    let pixels = width
        .checked_mul(height)
        .filter(|&pixels| pixels <= 16 * 1024 * 1024)
        .ok_or("FNT sheet is too large")?;
    let mut rgba = vec![0u8; pixels.checked_mul(4).ok_or("FNT byte size overflow")? as usize];
    let mut manifest = format!("ui-font 1\nsheet {key}.png\nline_height {}\n", font.line_height());
    for (code, glyph) in font.glyphs().iter().enumerate() {
        let cell_x = code as u32 % cols * font.max_width() as u32;
        let cell_y = code as u32 / cols * font.line_height() as u32;
        let glyph_y = cell_y + glyph.y_offset() as u32;
        for y in 0..glyph.height() as u32 {
            for x in 0..glyph.width() as u32 {
                let shade = glyph.pixels()[(y * glyph.width() as u32 + x) as usize];
                if shade == 0 {
                    continue;
                }
                let at = (((glyph_y + y) * width + cell_x + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&font_shade(shade));
            }
        }
        manifest.push_str(&format!(
            "glyph {code} x={cell_x} y={glyph_y} w={} h={} advance={} yoff={}\n",
            glyph.width(),
            glyph.height(),
            glyph.width(),
            glyph.y_offset(),
        ));
    }
    Ok((rgba, width, height, manifest))
}

fn font_shade(shade: u8) -> [u8; 4] {
    match shade {
        0 => [0, 0, 0, 0],
        1 => [255, 255, 255, 255],
        other => {
            let grey = other.saturating_mul(17);
            [grey, grey, grey, 255]
        }
    }
}

fn validate_empirical_ascii(stem: &str, font: &Fnt) -> Result<(), String> {
    for code in 0x20usize..=0x7f {
        let glyph = font
            .glyphs()
            .get(code)
            .ok_or_else(|| format!("{stem}.FNT has no ASCII glyph {code}"))?;
        if !(3..=16).contains(&glyph.width()) || glyph.height() > font.line_height() {
            return Err(format!(
                "{stem}.FNT glyph {code} is not empirically plausible: {}x{} line {}",
                glyph.width(),
                glyph.height(),
                font.line_height(),
            ));
        }
    }
    Ok(())
}

fn render_font_sample(font: &Fnt, text: &[u8], scale: u32) -> Result<(Vec<u8>, u32, u32), String> {
    if scale == 0 {
        return Err("FNT preview scale is zero".into());
    }
    let mut text_width = 0u32;
    for &code in text {
        let glyph = font
            .glyphs()
            .get(code as usize)
            .ok_or_else(|| format!("FNT preview has no glyph {code}"))?;
        text_width = text_width
            .checked_add(glyph.width() as u32)
            .ok_or("FNT preview width overflow")?;
    }
    let width = text_width
        .checked_add(4)
        .and_then(|value| value.checked_mul(scale))
        .ok_or("FNT preview width overflow")?;
    let height = (font.line_height() as u32)
        .checked_add(4)
        .and_then(|value| value.checked_mul(scale))
        .ok_or("FNT preview height overflow")?;
    let pixels = width.checked_mul(height).ok_or("FNT preview dimensions overflow")?;
    let mut rgba = vec![0u8; pixels.checked_mul(4).ok_or("FNT preview byte size overflow")? as usize];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[16, 16, 16, 255]);
    }
    let mut pen_x = 2u32;
    for &code in text {
        let glyph = &font.glyphs()[code as usize];
        for y in 0..glyph.height() as u32 {
            for x in 0..glyph.width() as u32 {
                let shade = glyph.pixels()[(y * glyph.width() as u32 + x) as usize];
                if shade == 0 {
                    continue;
                }
                let color = font_shade(shade);
                let px = (pen_x + x) * scale;
                let py = (2 + glyph.y_offset() as u32 + y) * scale;
                for sy in 0..scale {
                    for sx in 0..scale {
                        let at = (((py + sy) * width + px + sx) * 4) as usize;
                        rgba[at..at + 4].copy_from_slice(&color);
                    }
                }
            }
        }
        pen_x += glyph.width() as u32;
    }
    Ok((rgba, width, height))
}

fn decode_mouse_shp(bytes: &[u8]) -> Result<UiIndexedFrames, String> {
    let frame_count = td_u16(bytes, 0).ok_or("MOUSE.SHP is truncated")? as usize;
    if frame_count == 0 || frame_count > 256 {
        return Err("MOUSE.SHP has an invalid frame count".into());
    }
    let table_end = frame_count
        .checked_mul(4)
        .and_then(|size| size.checked_add(2))
        .ok_or("MOUSE.SHP offset table overflow")?;
    if table_end > bytes.len() {
        return Err("MOUSE.SHP offset table is truncated".into());
    }
    let mut offsets = Vec::with_capacity(frame_count);
    for index in 0..frame_count {
        let at = index
            .checked_mul(4)
            .and_then(|value| value.checked_add(2))
            .ok_or("MOUSE.SHP offset overflow")?;
        let offset = td_u32(bytes, at).ok_or("MOUSE.SHP offset table is truncated")? as usize;
        if offset < table_end || offset >= bytes.len() {
            return Err(format!("MOUSE.SHP frame {index} has an invalid offset"));
        }
        offsets.push(offset);
    }
    let mut physical_offsets = offsets.clone();
    physical_offsets.push(bytes.len());
    physical_offsets.sort_unstable();
    physical_offsets.dedup();

    let mut decoded_by_offset = BTreeMap::<usize, Vec<u8>>::new();
    let mut dimensions = None;
    let mut frames = Vec::with_capacity(frame_count);
    for (index, &start) in offsets.iter().enumerate() {
        if let Some(frame) = decoded_by_offset.get(&start) {
            frames.push(frame.clone());
            continue;
        }
        let end = physical_offsets
            .iter()
            .copied()
            .find(|&offset| offset > start)
            .ok_or_else(|| format!("MOUSE.SHP frame {index} has no end offset"))?;
        let frame = bytes
            .get(start..end)
            .filter(|frame| frame.len() >= 12)
            .ok_or_else(|| format!("MOUSE.SHP frame {index} is truncated"))?;
        let width = frame[4] as usize;
        let height = frame[5] as usize;
        if width == 0 || height == 0 || frame[6] != 0 || frame[7] as usize != width {
            return Err(format!("MOUSE.SHP frame {index} has an invalid header"));
        }
        if dimensions.replace((width, height)).is_some_and(|prior| prior != (width, height)) {
            return Err("MOUSE.SHP frames do not share dimensions".into());
        }
        let rle_size = td_u16(frame, 10).ok_or("MOUSE.SHP frame header is truncated")? as usize;
        let rle = decode_lcw_bounded(&frame[12..], rle_size)
            .map_err(|e| format!("MOUSE.SHP frame {index}: {e}"))?;
        let pixel_count = width.checked_mul(height).ok_or("MOUSE.SHP dimensions overflow")?;
        let columns = decode_zero_runs(&rle, pixel_count)
            .map_err(|e| format!("MOUSE.SHP frame {index}: {e}"))?;
        let mut row_major = vec![0u8; pixel_count];
        for x in 0..width {
            for y in 0..height {
                row_major[y * width + x] = columns[x * height + y];
            }
        }
        decoded_by_offset.insert(start, row_major.clone());
        frames.push(row_major);
    }
    let (width, height) = dimensions.ok_or("MOUSE.SHP has no decoded frames")?;
    Ok(UiIndexedFrames {
        width: width as u32,
        height: height as u32,
        frames,
    })
}

fn decode_lcw_bounded(src: &[u8], expected: usize) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::with_capacity(expected);
    let mut at = 0usize;
    loop {
        let cmd = *src.get(at).ok_or("LCW stream has no end command")?;
        at += 1;
        if cmd & 0x80 == 0 {
            let count = ((cmd & 0x70) >> 4) as usize + 3;
            let low = *src.get(at).ok_or("truncated LCW relative copy")? as usize;
            at += 1;
            let relative = ((cmd as usize & 0x0f) << 8) | low;
            if relative == 0 || relative > output.len() {
                return Err("invalid LCW relative copy");
            }
            let from = output.len() - relative;
            copy_lcw_bounded(&mut output, from, count, expected)?;
        } else if cmd & 0x40 == 0 {
            let count = (cmd & 0x3f) as usize;
            if count == 0 {
                return (output.len() == expected)
                    .then_some(output)
                    .ok_or("LCW output size does not match header");
            }
            let end = at.checked_add(count).ok_or("LCW literal overflow")?;
            let literal = src.get(at..end).ok_or("truncated LCW literal")?;
            if output.len().checked_add(count).is_none_or(|size| size > expected) {
                return Err("LCW output exceeds header size");
            }
            output.extend_from_slice(literal);
            at = end;
        } else {
            match cmd & 0x3f {
                0x3e => {
                    let count = take_lcw_u16(src, &mut at)? as usize;
                    let value = *src.get(at).ok_or("truncated LCW fill")?;
                    at += 1;
                    let size = output.len().checked_add(count).ok_or("LCW fill overflow")?;
                    if size > expected {
                        return Err("LCW output exceeds header size");
                    }
                    output.resize(size, value);
                }
                0x3f => {
                    let count = take_lcw_u16(src, &mut at)? as usize;
                    let from = take_lcw_u16(src, &mut at)? as usize;
                    copy_lcw_bounded(&mut output, from, count, expected)?;
                }
                count => {
                    let from = take_lcw_u16(src, &mut at)? as usize;
                    copy_lcw_bounded(&mut output, from, count as usize + 3, expected)?;
                }
            }
        }
    }
}

fn copy_lcw_bounded(
    output: &mut Vec<u8>,
    from: usize,
    count: usize,
    limit: usize,
) -> Result<(), &'static str> {
    if count != 0 && from >= output.len() {
        return Err("invalid LCW copy offset");
    }
    if output.len().checked_add(count).is_none_or(|size| size > limit) {
        return Err("LCW output exceeds header size");
    }
    for index in 0..count {
        let source = from.checked_add(index).ok_or("LCW copy overflow")?;
        let value = *output.get(source).ok_or("invalid LCW copy range")?;
        output.push(value);
    }
    Ok(())
}

fn take_lcw_u16(src: &[u8], at: &mut usize) -> Result<u16, &'static str> {
    let end = at.checked_add(2).ok_or("LCW word overflow")?;
    let pair = src.get(*at..end).ok_or("truncated LCW word")?;
    *at = end;
    Ok(u16::from_le_bytes([pair[0], pair[1]]))
}

fn decode_zero_runs(src: &[u8], expected: usize) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::with_capacity(expected);
    let mut at = 0usize;
    while at < src.len() {
        let value = src[at];
        at += 1;
        if value == 0 {
            let count = *src.get(at).ok_or("truncated zero run")? as usize;
            at += 1;
            let size = output.len().checked_add(count).ok_or("zero run overflow")?;
            if size > expected {
                return Err("zero run exceeds frame dimensions");
            }
            output.resize(size, 0);
        } else {
            if output.len() == expected {
                return Err("pixel stream exceeds frame dimensions");
            }
            output.push(value);
        }
    }
    (output.len() == expected)
        .then_some(output)
        .ok_or("pixel stream does not fill frame dimensions")
}

fn td_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let pair = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([pair[0], pair[1]]))
}

fn td_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

fn emit_tiberium(
    emitter: &mut RtsEmitter<'_>,
    theater: &TheaterBank<'_>,
    palette: &Pal,
    report: &mut ConvertReport,
    remap: &str,
) -> Result<(), String> {
    // TI1..TI12 are twelve CELL VARIANTS of tiberium, and each variant's SHP
    // carries the twelve GROWTH frames — the map's overlay byte picks the
    // variant, the game's growth stage picks the frame. Taking frame 0 of
    // every variant exported twelve kinds of almost-nothing (frame 0 is the
    // sparsest growth), which is why the fields drew as a few grey specks.
    // One variant's full frame run is the honest stage ladder.
    let mut frames = Vec::new();
    for variant in 1..=12 {
        let stem = format!("ti{variant}");
        let Some(shp) = theater.shp(&stem) else {
            report.missing_shapes.insert(format!("{}.TEM", stem.to_ascii_uppercase()));
            continue;
        };
        for indexed in shp.frames() {
            if frames.len() >= 12 {
                break;
            }
            frames.push(SpritePixels {
                rgba: indexed_transparent(indexed, palette),
                width: shp.width() as u32,
                height: shp.height() as u32,
                rot: 0,
            });
        }
        if frames.len() >= 12 {
            break;
        }
    }
    if frames.len() == 12 {
        emit_spec(emitter, report, SpriteSpec {
            key: "ti".into(),
            role: "resource",
            facings: 1,
            states: vec![SpriteState {
                name: "idle",
                first: 0,
                last: 12,
                looping: false,
                fps: 1,
            }],
            frames,
            unit: Some(UnitSpec {
                manifest_lines: vec!["unit class=resource title=\"Tiberium\"".into()],
            }),
            manifest_lines: vec![remap.into()],
            tags: vec!["cnc", "resource"],
        })?;
    }
    Ok(())
}

fn emit_audio(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    names: &NameTable,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let speech_mix = MixFile::parse(&archives.speech).map_err(|e| e.to_string())?;
    let speech = names
        .resolve_names(&speech_mix)
        .into_iter()
        .filter_map(|(_, name)| name)
        .filter_map(|name| name.strip_suffix(".AUD"))
        .map(|stem| stem.to_ascii_lowercase())
        .filter(|stem| !REQUIRED_SFX.contains(&stem.as_str()))
        .collect::<BTreeSet<_>>();
    let mut done = 0usize;
    let total = speech.len() + REQUIRED_SFX.len();
    for stem in speech {
        done += 1;
        tick(on_tick, done, total, format!("speech {stem}"), None);
        let speech_name = format!("{}.AUD", stem.to_ascii_uppercase());
        if let Some(aud) = mix_entry(&archives.speech, &speech_name).and_then(|bytes| Aud::parse(bytes).ok()) {
            emitter.emit_sfx(
                &format!("sfx/cnc/{stem}"),
                aud.sample_rate(),
                aud.channels(),
                aud.samples(),
                &["cnc", "speech"],
            )?;
        }
    }
    for &stem in REQUIRED_SFX {
        done += 1;
        tick(on_tick, done, total, format!("sfx {stem}"), None);
        let exact = archives.aud(stem);
        if let Some((_, location)) = exact.as_ref() {
            if PREVIOUSLY_UNRESOLVED_SFX.contains(&stem) {
                report.audio_sources.insert(
                    format!("{}.AUD", stem.to_ascii_uppercase()),
                    (*location).to_string(),
                );
            }
        }
        let fallback = if exact.is_none() && stem == "await1" {
            archives.aud("mhello1")
        } else {
            None
        };
        if let Some((aud, _)) = exact.or(fallback) {
            if stem == "await1" && !archives.has_aud(stem) {
                report.missing_audio.insert("AWAIT1.AUD".into());
                report.audio_fallbacks.push("await1.wav uses MHELLO1.AUD, the pack's available select-voice fallback".into());
            }
            emitter.emit_sfx(
                &format!("sfx/cnc/{stem}"),
                aud.sample_rate(),
                aud.channels(),
                aud.samples(),
                &["cnc", "sfx"],
            )?;
        } else {
            report.missing_audio.insert(format!("{}.AUD", stem.to_ascii_uppercase()));
        }
    }
    Ok(())
}

const REQUIRED_SFX: &[&str] = &[
    "await1", "report1", "ready", "ackno", "affirm1", "yessir1", "ritaway", "roger",
    "ugotit", "nuyell1", "nuyell3", "nuyell5", "vehic1", "xplos", "crumble",
    "xplobig4", "xplode", "sqush2", "keystrok", "gun8", "mgun2", "mgun11", "rocket1",
    "rocket2", "flamer2", "ramgun", "tnkfire3", "tnkfire4", "tnkfire6", "laser", "ionsfx",
];

const PREVIOUSLY_UNRESOLVED_SFX: &[&str] = &[
    "ackno", "affirm1", "await1", "ionsfx", "laser", "ramgun", "ready", "report1",
    "ritaway", "roger", "sqush2", "ugotit", "vehic1", "xplode", "yessir1",
];

fn unit_lines(key: &str, archives: &Archives, remap: &str) -> Vec<String> {
    let mut lines = vec![remap.into()];
    let Some(unit) = UNIT_MANIFESTS.iter().find(|unit| unit.key == key) else {
        return lines;
    };
    lines.push(rewrite_unit_roles(key, unit.line, ROLE_TABLE));
    for weapon in unit.weapon.into_iter().chain(unit.weapon2) {
        if let Some(line) = weapon_line(weapon) {
            let line = match weapon_fire(weapon) {
                Some("laser") if archives.has_aud("laser") => line.into(),
                Some("laser") if archives.has_aud("ionsfx") => {
                    line.replace("fire=sfx/cnc/laser", "fire=sfx/cnc/ionsfx")
                }
                Some(stem) if archives.has_aud(stem) => line.into(),
                Some(_) => line
                    .split_whitespace()
                    .filter(|token| !token.starts_with("fire="))
                    .collect::<Vec<_>>()
                    .join(" "),
                None => line.into(),
            };
            lines.push(line);
        }
    }
    let infantry = INFANTRY_KEYS.contains(&key);
    let structure = STRUCTURE_KEYS.contains(&key);
    let mut slots = Vec::new();
    if infantry {
        push_sound_slot(
            &mut slots,
            "select",
            &["await1", "report1", "ready"],
            archives,
        );
        push_sound_slot(
            &mut slots,
            "move",
            &["ackno", "affirm1", "yessir1", "ritaway", "roger", "ugotit"],
            archives,
        );
        push_sound_slot(
            &mut slots,
            "death",
            &["nuyell1", "nuyell3", "nuyell5"],
            archives,
        );
    } else if structure {
        push_sound_slot(
            &mut slots,
            "death",
            &["crumble", "xplobig4"],
            archives,
        );
    } else {
        let select = if key == "harv" {
            &["vehic1"][..]
        } else {
            &["vehic1", "await1", "report1"][..]
        };
        push_sound_slot(&mut slots, "select", select, archives);
        push_sound_slot(
            &mut slots,
            "move",
            &["ackno", "affirm1", "roger"],
            archives,
        );
        push_sound_slot(&mut slots, "death", &["xplos"], archives);
    }
    if let Some(weapon) = unit.weapon {
        if let Some(stem) = weapon_fire(weapon) {
            let stem = if stem == "laser" && !archives.has_aud(stem) {
                "ionsfx"
            } else {
                stem
            };
            if archives.has_aud(stem) {
                slots.push(format!("attack=sfx/cnc/{stem}"));
            }
        }
    }
    if !slots.is_empty() {
        lines.push(format!("sound {}", slots.join(" ")));
    }
    lines
}

fn pack_roster() -> Vec<String> {
    UNIT_MANIFESTS
        .iter()
        .filter(|unit| positive_unit_cost(unit.line))
        .map(|unit| roster_key("cnc", unit.key))
        .collect()
}

#[cfg(test)]
pub(super) fn role_test_lines() -> Vec<String> {
    UNIT_MANIFESTS
        .iter()
        .map(|unit| rewrite_unit_roles(unit.key, unit.line, ROLE_TABLE))
        .collect()
}

fn weapon_fire(weapon: &str) -> Option<&'static str> {
    Some(match weapon {
        "m16" => "gun8",
        "m60" => "mgun11",
        "chaingun" => "mgun2",
        "dragon" | "tower_rockets" | "nike" => "rocket1",
        "flamer" | "chemspray" => "flamer2",
        "sniper" => "ramgun",
        "75mm" | "105mm" => "tnkfire3",
        "120mm" => "tnkfire6",
        "mammoth_tusk" | "227mm" | "honest_john" => "rocket2",
        "155mm" => "tnkfire4",
        "laser" => "laser",
        "grenade" => return None,
        _ => return None,
    })
}

fn push_sound_slot(out: &mut Vec<String>, slot: &str, choices: &[&str], archives: &Archives) {
    let choices = choices
        .iter()
        .filter(|stem| has_slot_sound(archives, stem))
        .map(|stem| format!("sfx/cnc/{stem}"))
        .collect::<Vec<_>>();
    if !choices.is_empty() {
        out.push(format!("{slot}={}", choices.join("|")));
    }
}

fn emit_icon(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    key: &str,
    report: &mut ConvertReport,
) -> Result<(), String> {
    let upper = structure_source(key).to_ascii_uppercase();
    let names = [format!("{upper}ICNH.SHP"), format!("{upper}ICNH.TEM")];
    let bytes = names.iter().find_map(|name| {
        mix_entry(&archives.tempicnh, name)
            .or_else(|| mix_entry(&archives.conquer, name))
            .or_else(|| mix_entry(&archives.temperat, name))
    });
    let Some(shp) = bytes.and_then(|bytes| Shp::parse(bytes).ok()) else {
        report.missing_shapes.insert(format!("{upper}ICNH.SHP"));
        return Ok(());
    };
    let Some(frame) = shp.frames().first() else { return Ok(()) };
    let rgba = indexed_transparent(frame, palette);
    emitter.emit_texture(
        &format!("icons/cnc/{key}"),
        &rgba,
        shp.width() as u32,
        shp.height() as u32,
        &["cnc", "icon"],
    )
}

fn emit_spec(
    emitter: &mut RtsEmitter<'_>,
    report: &mut ConvertReport,
    spec: SpriteSpec,
) -> Result<(), String> {
    *report.roles.entry(spec.role.into()).or_default() += 1;
    emitter.emit_sprite(spec)
}

/// How a TD building's SHP divides into intact art, damaged art and rubble.
///
/// TD stores a building's animation TWICE — once intact, once damaged — with a
/// single rubble frame after it, so the frame count is `2n + 1`. Measured
/// straight out of `conquer.mix` by comparing each frame with frame 0: NUKE
/// 9 = 4+4+1, OBLI 9 = 4+4+1, SILO 11 = 5+5+1 (five tiberium fill stages, each
/// with a damaged twin), TMPL 11, HPAD/FIX 15 = 7+7+1, PYLE 21 = 10+10+1,
/// HQ/EYE/AFLD 33 = 16+16+1, FACT 49 = 24+24+1, PROC 61 = 30+30+1, SAM
/// 129 = 64+64+1, and WEAP/HAND/GTWR/ATWR/BIO/MISS 3 = 1+1+1. An even count
/// (GUN's 128 facing frames) has no rubble frame and splits in half.
///
/// What this replaces: `count.div_ceil(2)`, which put the FIRST DAMAGED frame
/// at the end of the intact loop. Every animated building therefore cycled
/// intact→intact→intact→DAMAGED at 6fps — reported live on the power plant,
/// where 9 frames split 5/4 instead of 4/4/1.
fn structure_split(count: usize) -> (usize, usize) {
    if count <= 1 {
        return (count, count);
    }
    let paired = if count % 2 == 1 { count - 1 } else { count };
    let healthy = (paired / 2).max(1);
    (healthy, (healthy * 2).min(count))
}

/// Harvest facings stored in HARV.SHP, against the 32 the hull is drawn at.
const HARVEST_ANIM_FACINGS: usize = 8;
/// Steps in one harvesting cycle.
const HARVEST_ANIM_STEPS: usize = 4;

/// HARV.SHP frames 32..64: the harvesting cycle, 8 facings × 4 steps,
/// facing-major, where harvest facing `f` is the hull's driving facings
/// `4f..4f+4` (frame 32 is 2% different from frame 0 — the same facing at the
/// start of its cycle — and the run repeats in groups of four across eight
/// facings).
///
/// Emitted at ALL 32 rots, each source step repeated across the four driving
/// facings it covers, because a clip stored at only 8 of the sheet's 32 rots
/// has no frame for the other 24 and the renderer falls back to rot 1 — a
/// harvester that snaps to facing north the moment it starts working.
fn harvest_source(rot: usize, step: usize) -> usize {
    let per_facing = 32 / HARVEST_ANIM_FACINGS;
    32 + (rot / per_facing) * HARVEST_ANIM_STEPS + step
}

fn append_harvest_frames(out: &mut Vec<SpritePixels>, shp: &Shp, palette: &Pal) {
    for rot in 0..32usize {
        for step in 0..HARVEST_ANIM_STEPS {
            let source = harvest_source(rot, step);
            let Some(pixels) = shp.frames().get(source) else {
                return;
            };
            out.push(SpritePixels {
                rgba: indexed_transparent(pixels, palette),
                width: shp.width() as u32,
                height: shp.height() as u32,
                rot: facing_rot(rot),
            });
        }
    }
}

fn frame_range(
    shp: &Shp,
    first: usize,
    last: usize,
    palette: &Pal,
    rot: impl Fn(usize) -> u8,
) -> Vec<SpritePixels> {
    let mut out = Vec::new();
    append_frames(&mut out, shp, first, last, palette, rot);
    out
}

fn append_frames(
    out: &mut Vec<SpritePixels>,
    shp: &Shp,
    first: usize,
    last: usize,
    palette: &Pal,
    rot: impl Fn(usize) -> u8,
) {
    for source in first..last.min(shp.frames().len()) {
        out.push(SpritePixels {
            rgba: indexed_transparent(&shp.frames()[source], palette),
            width: shp.width() as u32,
            height: shp.height() as u32,
            rot: rot(source - first),
        });
    }
}

fn conquer_shp(archives: &Archives, stem: &str) -> Option<Shp> {
    Shp::parse(mix_entry(
        &archives.conquer,
        &format!("{}.SHP", stem.to_ascii_uppercase()),
    )?)
    .ok()
}

fn remap_line(palette: &Pal, start: u8) -> String {
    let colors = (start..start + 16)
        .map(|index| {
            let [r, g, b] = palette.rgb(index);
            format!("{r},{g},{b}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("remap {colors}")
}

fn verify_remap_ramp(conquer: &[u8], palette: &Pal) -> String {
    let mut counts = Vec::new();
    for stem in ["MTNK", "E1", "HARV"] {
        let count = mix_entry(conquer, &format!("{stem}.SHP"))
            .and_then(|bytes| Shp::parse(bytes).ok())
            .map(|shp| {
                shp.frames()
                    .iter()
                    .flatten()
                    .filter(|&&index| (176..=191).contains(&index))
                    .count()
            })
            .unwrap_or(0);
        counts.push(format!("{stem}={count}"));
    }
    let ramp = (176..=191).map(|i| palette.rgb(i)).collect::<Vec<_>>();
    let coloured = ramp.iter().filter(|rgb| rgb[0] != rgb[1] || rgb[1] != rgb[2]).count();
    format!(
        "indices 176..191; occurrences {}; single-hue luminance ramp {}",
        counts.join(", "),
        if coloured >= 12 { "confirmed" } else { "not confirmed" }
    )
}

fn indexed_opaque(indexed: &[u8], palette: &Pal, out: &mut [u8]) {
    for (&index, rgba) in indexed.iter().zip(out.chunks_exact_mut(4)) {
        let [r, g, b] = palette.rgb(index);
        rgba.copy_from_slice(&[r, g, b, 255]);
    }
}

fn indexed_transparent(indexed: &[u8], palette: &Pal) -> Vec<u8> {
    let mut out = Vec::with_capacity(indexed.len() * 4);
    for &index in indexed {
        match index {
            0 => out.extend_from_slice(&[0, 0, 0, 0]),
            4 => out.extend_from_slice(&[0, 0, 0, 128]),
            _ => {
                let [r, g, b] = palette.rgb(index);
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn blit_indexed(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    ox: u32,
    oy: u32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    palette: &Pal,
) {
    for y in 0..src_h {
        for x in 0..src_w {
            if ox + x >= dst_w || oy + y >= dst_h {
                continue;
            }
            let index = src[(y * src_w + x) as usize];
            if index == 0 {
                continue;
            }
            let at = (((oy + y) * dst_w + ox + x) * 4) as usize;
            if index == 4 {
                for channel in 0..3 {
                    dst[at + channel] = (dst[at + channel] as u16 / 2) as u8;
                }
            } else {
                let [r, g, b] = palette.rgb(index);
                dst[at..at + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
}

fn read_pack_file(pack: &Path, wanted: &str) -> Result<Vec<u8>, String> {
    let mut stack = vec![(pack.to_path_buf(), 0usize)];
    let mut matches = Vec::new();
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
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
    std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn index_all_archives(
    pack: &Path,
    names: &NameTable,
) -> Result<
    (
        BTreeMap<String, IndexedEntry>,
        Vec<String>,
        BTreeMap<String, usize>,
        usize,
    ),
    String,
> {
    let mut paths = Vec::new();
    let mut stack = vec![(pack.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push((path, depth + 1));
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mix"))
            {
                paths.push(path);
            }
        }
    }
    paths.sort();

    let top_level = paths.len();
    let mut named = BTreeMap::new();
    let mut archive_scan = Vec::new();
    let mut unnamed_aud = BTreeMap::new();
    for path in paths {
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let location = path
            .strip_prefix(pack)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        index_mix(
            &bytes,
            &location,
            names,
            &mut named,
            &mut archive_scan,
            &mut unnamed_aud,
        )?;
    }
    let nested_mix_count = archive_scan.len().saturating_sub(top_level);
    Ok((named, archive_scan, unnamed_aud, nested_mix_count))
}

fn index_mix(
    bytes: &[u8],
    location: &str,
    names: &NameTable,
    named: &mut BTreeMap<String, IndexedEntry>,
    archive_scan: &mut Vec<String>,
    unnamed_aud: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    let mix = MixFile::parse(bytes).map_err(|e| format!("{location}: {e}"))?;
    let resolved = mix
        .entries()
        .iter()
        .filter(|entry| names.name_of(entry.id).is_some())
        .count();
    archive_scan.push(format!(
        "{location}: {} entries, {resolved} names resolved",
        mix.entries().len()
    ));
    let mut unnamed = 0usize;
    for entry in mix.entries() {
        let Some(payload) = mix.by_id(entry.id) else { continue };
        let resolved_name = names.name_of(entry.id);
        let child = match resolved_name {
            Some(name) => {
                let child = format!("{location}/{name}");
                named
                    .entry(name.to_ascii_uppercase())
                    .or_insert_with(|| IndexedEntry {
                        bytes: payload.to_vec(),
                        location: child.clone(),
                    });
                child
            }
            None => {
                if has_aud_header(payload) {
                    unnamed += 1;
                }
                format!("{location}/{:08x}", entry.id)
            }
        };
        if MixFile::parse(payload).is_ok() {
            index_mix(payload, &child, names, named, archive_scan, unnamed_aud)?;
        }
    }
    unnamed_aud.insert(location.to_string(), unnamed);
    Ok(())
}

fn has_aud_header(bytes: &[u8]) -> bool {
    if bytes.len() < 20 {
        return false;
    }
    let rate = u16::from_le_bytes([bytes[0], bytes[1]]);
    let marker = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    (8000..=48000).contains(&rate) && matches!(bytes[11], 1 | 99) && marker == 0x0000_deaf
}

fn mix_entry<'a>(archive: &'a [u8], name: &str) -> Option<&'a [u8]> {
    MixFile::parse(archive).ok()?.by_name(name)
}

fn has_slot_sound(archives: &Archives, stem: &str) -> bool {
    archives.has_aud(stem) || (stem == "await1" && archives.has_aud("mhello1"))
}

fn grid_class(class: char) -> char {
    match class {
        'r' | 'B' => 'r',
        'g' | 'b' => 'b',
        'w' | 'v' => 'w',
        'k' | 't' | 'W' => '#',
        'T' => 't',
        _ => '.',
    }
}

fn local_cell(bounds: crate::cnc_import::map::MapBounds, cell: u16) -> Option<(usize, usize)> {
    let x = cell % 64;
    let y = cell / 64;
    if x < bounds.x
        || y < bounds.y
        || x >= bounds.x + bounds.width
        || y >= bounds.y + bounds.height
    {
        return None;
    }
    Some(((x - bounds.x) as usize, (y - bounds.y) as usize))
}

fn block_rect(
    grid: &mut [Vec<char>],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    value: char,
) {
    for row in grid.iter_mut().skip(y).take(height) {
        for cell in row.iter_mut().skip(x).take(width) {
            *cell = value;
        }
    }
}

fn source_yaw(facing: i32) -> f32 {
    -(facing as f32 / 256.0) * TAU
}

fn facing_rot(source_facing: usize) -> u8 {
    1 + source_facing as u8
}

fn health_fraction(health: i32) -> f32 {
    (health as f32 / 256.0).clamp(0.0, 1.0)
}

fn subcell_offset(subcell: i32) -> (f32, f32) {
    match subcell {
        1 => (-1.5, -1.5),
        2 => (1.5, -1.5),
        3 => (-1.5, 1.5),
        4 => (1.5, 1.5),
        _ => (0.0, 0.0),
    }
}

fn mobile_class(key: &str) -> &'static str {
    if AIRCRAFT_KEYS.contains(&key) {
        "aircraft"
    } else if matches!(key, "lst" | "boat") {
        "boat"
    } else {
        "vehicle"
    }
}

fn structure_footprint(key: &str) -> (usize, usize) {
    match key {
        "fact" | "proc" => (3, 2),
        "powr" | "nuke" | "pyle" | "hq" | "eye" | "hpad" => (2, 2),
        "silo" | "sam" => (2, 1),
        "hand" => (2, 3),
        "weap" | "fix" | "tmpl" => (3, 3),
        "afld" => (4, 2),
        _ => (1, 1),
    }
}

fn structure_source(key: &str) -> &str {
    match key {
        "powr" => "nuke",
        "nuke" => "nuk2",
        _ => key,
    }
}

fn contract_key(source: &str) -> String {
    match source.to_ascii_uppercase().as_str() {
        "NUKE" => "powr".into(),
        "NUK2" => "nuke".into(),
        _ => source.to_ascii_lowercase(),
    }
}

fn is_defense(key: &str) -> bool {
    matches!(key, "gun" | "gtwr" | "atwr" | "obli" | "sam")
}

fn is_wall(key: &str) -> bool {
    matches!(key, "sbag" | "cycl" | "brik" | "barb" | "wood")
}

fn is_static_decal(key: &str) -> bool {
    if key.eq_ignore_ascii_case("FPLS") {
        return true;
    }
    key.get(1..)
        .filter(|_| key.starts_with('V') || key.starts_with('v'))
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (12..=18).contains(&number))
}

fn tiberium_stage(key: &str) -> Option<u8> {
    key.strip_prefix("ti")?
        .parse::<u8>()
        .ok()
        .filter(|stage| (1..=12).contains(stage))
        .map(|stage| stage - 1)
}

fn canonical_owner(owner: &str) -> String {
    if owner.eq_ignore_ascii_case("GoodGuy") || owner.eq_ignore_ascii_case("GDI") {
        "GDI".into()
    } else if owner.eq_ignore_ascii_case("BadGuy") || owner.eq_ignore_ascii_case("NOD") {
        "NOD".into()
    } else if owner.eq_ignore_ascii_case("Neutral") {
        "Neutral".into()
    } else if owner.eq_ignore_ascii_case("Special") {
        "Special".into()
    } else if owner.len() == 6
        && owner[..5].eq_ignore_ascii_case("Multi")
        && owner.as_bytes()[5].is_ascii_digit()
    {
        format!("Multi{}", &owner[5..])
    } else {
        owner.replace(' ', "_")
    }
}

fn house_color(house: &str) -> &'static str {
    match house {
        "GDI" => "e8c040",
        "NOD" => "d02020",
        "Neutral" => "b0b0b0",
        "Special" => "9aa0c8",
        "Multi1" => "e8c040",
        "Multi2" => "d02020",
        "Multi3" => "4090e8",
        "Multi4" => "48b850",
        "Multi5" => "d060c8",
        "Multi6" => "e08030",
        _ => "9aa0c8",
    }
}

fn color_rgb(hex: &str) -> [u8; 3] {
    let value = u32::from_str_radix(hex, 16).unwrap_or(0x9aa0c8);
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

fn waypoint_starts(map: &TdMap) -> Vec<(String, f32, f32)> {
    let mut starts = map
        .waypoints
        .iter()
        .filter(|waypoint| waypoint.number <= 7 && (0..4096).contains(&waypoint.cell))
        .filter_map(|waypoint| {
            let cell = waypoint.cell as u16;
            local_cell(map.bounds, cell).map(|_| {
                let (x, z) = cell_to_metres(map.bounds.x, map.bounds.y, cell);
                (format!("start_{}", waypoint.number), x, z)
            })
        })
        .collect::<Vec<_>>();
    starts.sort_by(|a, b| a.0.cmp(&b.0));
    if starts.is_empty() {
        starts.push((
            "start_0".into(),
            map.bounds.width as f32 * CELL_M * 0.5,
            map.bounds.height as f32 * CELL_M * 0.5,
        ));
    }
    starts
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

fn write_empirical_contact_sheets(archives: &Archives, palette: &Pal) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../local/agent_state/cnc");
    if std::fs::create_dir_all(&root).is_err() {
        return;
    }
    for (stem, first, last, cols, file) in [
        ("mtnk", 0usize, 9usize, 3u32, "mtnk-frames-0-8.png"),
        ("e1", 0usize, 64usize, 8u32, "e1-frames-0-63.png"),
    ] {
        let Some(shp) = conquer_shp(archives, stem) else { continue };
        let frames = frame_range(&shp, first, last, palette, |_| 0);
        if let Some((rgba, width, height)) = contact_sheet(&frames, cols) {
            if let Ok(png) = crate::classic_import::encode_png_rgba(&rgba, width, height) {
                let _ = std::fs::write(root.join(file), png);
            }
        }
    }
}

fn contact_sheet(frames: &[SpritePixels], cols: u32) -> Option<(Vec<u8>, u32, u32)> {
    let cell_w = frames.iter().map(|frame| frame.width).max()?;
    let cell_h = frames.iter().map(|frame| frame.height).max()?;
    let cols = cols.max(1);
    let rows = (frames.len() as u32).div_ceil(cols);
    let width = cols * cell_w;
    let height = rows * cell_h;
    let mut out = vec![0u8; (width * height * 4) as usize];
    for (index, frame) in frames.iter().enumerate() {
        let ox = index as u32 % cols * cell_w;
        let oy = index as u32 / cols * cell_h;
        for y in 0..frame.height {
            let src = (y * frame.width * 4) as usize;
            let dst = (((oy + y) * width + ox) * 4) as usize;
            out[dst..dst + (frame.width * 4) as usize]
                .copy_from_slice(&frame.rgba[src..src + (frame.width * 4) as usize]);
        }
    }
    Some((out, width, height))
}

fn write_report(report: &ConvertReport) -> Result<(), String> {
    let mut text = String::from(
        "# Command & Conquer staged conversion report\n\n## Worlds\n\n| Map | Title | Theater | Bounds | Units | Structures | Scenery | Resources | Starts |\n|---|---|---|---|---:|---:|---:|---:|---:|\n",
    );
    for map in &report.maps {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            map.id,
            map.title.replace('|', "\\|"),
            map.theater,
            map.bounds,
            map.units,
            map.structures,
            map.scenery,
            map.resources,
            map.starts,
        ));
    }
    text.push_str("\n## Sprite assets\n\n| Role | Count |\n|---|---:|\n");
    for (role, count) in &report.roles {
        text.push_str(&format!("| {role} | {count} |\n"));
    }
    text.push_str("\n## Empirical findings\n\n");
    text.push_str(&format!("- Remap ramp: {}.\n", report.remap_note));
    text.push_str("- Vehicle rotation: source frames advance counter-clockwise in screen space; MTNK frame 0 faces north and frame 8 faces west, so source frame `k` is manifest `rot 1+k`.\n");
    text.push_str(&format!(
        "- Infantry: E1.SHP has {} frames. Stand is 0..7, guard 8..15, walk 16..63 (8 facings × 6, facing-major), fire 64..127 (8 × 8), and the emitted gun death is 398..405. The following omni deaths are explosion 406..413, grenade 414..425, and fire 426..443. Facing blocks advance in the same counter-clockwise direction as vehicles.\n",
        report.e1_frames
    ));
    text.push_str("- Structures: the source frame count is divided at `ceil(count/2)`; the first half is healthy and the second damaged. MAKE frames are appended as build. Observed splits: ");
    text.push_str(&report.structure_halves.join(", "));
    text.push_str(".\n");
    text.push_str("- TMP `index2`: observed values remain 0 and 1, but the payloads do not establish a gameplay/terrain meaning. The converter therefore follows the contract table and does not use index2 for class assignment.\n");
    text.push_str("\n## MIX archive index\n\n");
    text.push_str(&format!(
        "- Nested MIX entries discovered: {}.\n",
        report.nested_mix_count
    ));
    for archive in &report.archive_scan {
        text.push_str(&format!("- {archive}\n"));
    }
    text.push_str("\n### Unnamed AUD payloads\n\n");
    for (archive, count) in &report.unnamed_aud {
        text.push_str(&format!("- `{archive}`: {count}\n"));
    }
    text.push_str("\n### Recovered audio sources\n\n");
    if report.audio_sources.is_empty() {
        text.push_str("None.\n");
    } else {
        for (name, location) in &report.audio_sources {
            text.push_str(&format!("- `{name}` from `{location}`\n"));
        }
    }
    text.push_str("\n## Unresolved pack references\n\n### Shapes\n\n");
    if report.missing_shapes.is_empty() {
        text.push_str("None.\n");
    } else {
        for missing in &report.missing_shapes {
            text.push_str(&format!("- `{missing}`\n"));
        }
    }
    text.push_str("\n### Audio\n\n");
    if report.missing_audio.is_empty() {
        text.push_str("None.\n");
    } else {
        for missing in &report.missing_audio {
            text.push_str(&format!("- `{missing}`\n"));
        }
    }
    if !report.audio_fallbacks.is_empty() {
        text.push_str("\n### Audio fallbacks\n\n");
        for fallback in &report.audio_fallbacks {
            text.push_str(&format!("- {fallback}\n"));
        }
    }
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../cnc-convert-report.md");
    std::fs::write(&report_path, text)
        .map_err(|e| format!("write cnc-convert-report.md: {e}"))
}

fn write_ui_art_report(report: &ConvertReport) -> Result<(), String> {
    let mut text = String::from(
        "# Tiberian Dawn UI art emission report\n\n## UI sheets\n\n| Stem | Role | MIX source | Frames | Frame size | Columns | Sheet size |\n|---|---|---|---:|---:|---:|---:|\n",
    );
    for sheet in &report.ui_sheets {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {}x{} | {} | {}x{} |\n",
            sheet.stem,
            sheet.role,
            sheet.source.replace('|', "\\|"),
            sheet.frames,
            sheet.frame_width,
            sheet.frame_height,
            sheet.cols,
            sheet.sheet_width,
            sheet.sheet_height,
        ));
    }
    text.push_str(
        "\n## FNT layout found\n\nThe checked files use a 14-byte little-endian header: `u16 file_size`, `u8 zero`, `u8 kind`, then five `u16` absolute offsets for metadata, glyph offsets, widths, nibble data, and height pairs. Metadata is six bytes (`12 10 00`, last character, maximum height, maximum width). The glyph count is `last + 1`; each absolute glyph offset points into low-nibble-first row-major pixels, and the final table stores `(y_offset, glyph_height)`. Nibble 0 is transparent, nibble 1 is opaque white, and the remaining nibbles are opaque grey levels.\n\n| Font | MIX source | Glyphs | Line height | Maximum width |\n|---|---|---:|---:|---:|\n",
    );
    for font in &report.ui_fonts {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            font.stem,
            font.source.replace('|', "\\|"),
            font.glyphs,
            font.line_height,
            font.max_width,
        ));
    }
    text.push_str("\n`8POINT.FNT` and `LED.FNT` both passed the empirical ASCII 0x20..0x7f width range of 3..16 pixels and the declared line-height bound.\n\n## Scratch previews\n\n");
    for path in &report.ui_scratch {
        text.push_str(&format!("- `{path}` (`0123456789 ABC`)\n"));
    }
    text.push_str("\n## Deferred equivalent packs\n\n- RA: `CLOCK`, `EARTH`, `FPOWER`, `MOUSE`, `PIPS`/`PIPS2`, `REPAIR`, and `TABS`; this package has no `hires.mix`.\n- TS: `GCLOCK2`, `IDLE-SIDE`, `IDLE-STRIP`, `MOUSE`, `PIPS`/`PIPS2`, and `REPAIR` in `sidec01.mix` / `sidec02.mix`.\n- D2K: sidebar artwork is inside `DATA.R8`.\n");
    let report_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../cnc-ui-art-report.md");
    std::fs::write(&report_path, text)
        .map_err(|e| format!("write cnc-ui-art-report.md: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_mix(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let data_size = entries.iter().map(|(_, bytes)| bytes.len()).sum::<usize>();
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(data_size as u32).to_le_bytes());
        let mut offset = 0u32;
        for (id, bytes) in entries {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            offset += bytes.len() as u32;
        }
        for (_, bytes) in entries {
            out.extend_from_slice(bytes);
        }
        out
    }

    #[test]
    fn cnc_import_nested_mix_names_and_unnamed_aud_are_indexed() {
        let mut unnamed = vec![0u8; 20];
        unnamed[0..2].copy_from_slice(&22_050u16.to_le_bytes());
        unnamed[11] = 99;
        unnamed[16..20].copy_from_slice(&0x0000_deafu32.to_le_bytes());
        let inner = test_mix(&[
            (crate::cnc_import::mix_id("ACKNO.AUD"), vec![1, 2, 3]),
            (0x1234_5678, unnamed),
        ]);
        let outer = test_mix(&[(0xdead_beef, inner)]);
        let mut named = BTreeMap::new();
        let mut scan = Vec::new();
        let mut unnamed_aud = BTreeMap::new();
        index_mix(
            &outer,
            "outer.mix",
            &NameTable::new(),
            &mut named,
            &mut scan,
            &mut unnamed_aud,
        )
        .unwrap();
        let ackno = named.get("ACKNO.AUD").expect("nested name");
        assert_eq!(ackno.bytes, [1, 2, 3]);
        assert_eq!(ackno.location, "outer.mix/deadbeef/ACKNO.AUD");
        assert_eq!(scan.len(), 2);
        assert_eq!(unnamed_aud["outer.mix/deadbeef"], 1);
    }

    #[test]
    fn cnc_import_template_table_lookup_is_theater_specific() {
        let table = TemplateTable::parse(TEMPLATE_TABLE_TEXT).unwrap();
        let temperate = table.get("temperat", 1).unwrap();
        assert_eq!((temperate.stem.as_str(), temperate.width, temperate.height), ("w1", 1, 1));
        assert_eq!(temperate.classes, "w");
        assert_eq!(table.get("desert", 57).unwrap().stem, "br1");
        assert!(table.get("winter", 57).is_none());
    }

    #[test]
    fn cnc_import_facing_to_rot_is_counter_clockwise_from_north() {
        let rots = (0..32).map(facing_rot).collect::<Vec<_>>();
        assert_eq!(rots[0], 1);
        assert_eq!(rots[8], 9, "quarter turn left is west");
        assert_eq!(rots[31], 32);
    }

    #[test]
    fn cnc_import_power_structure_names_follow_contract_keys() {
        assert_eq!(contract_key("NUKE"), "powr");
        assert_eq!(contract_key("NUK2"), "nuke");
        assert_eq!(structure_source("powr"), "nuke");
        assert_eq!(structure_source("nuke"), "nuk2");
    }

    #[test]
    fn cnc_import_grid_letters_follow_contract_classes() {
        assert_eq!(grid_class('c'), '.');
        assert_eq!(grid_class('r'), 'r');
        assert_eq!(grid_class('g'), 'b');
        assert_eq!(grid_class('b'), 'b');
        assert_eq!(grid_class('w'), 'w');
        assert_eq!(grid_class('v'), 'w');
        assert_eq!(grid_class('k'), '#');
        assert_eq!(grid_class('t'), '#');
        assert_eq!(grid_class('T'), 't');
    }

    #[test]
    fn cnc_import_remap_line_contains_exactly_sixteen_palette_entries() {
        let mut bytes = [0u8; 768];
        for index in 176..192 {
            bytes[index * 3..index * 3 + 3].copy_from_slice(&[63, 32, 1]);
        }
        let line = remap_line(&Pal::parse(&bytes).unwrap(), 176);
        assert_eq!(
            line,
            format!("remap {}", vec!["255,129,4"; 16].join(" "))
        );
    }

    #[test]
    #[ignore]
    fn convert_local_cnc_pack_if_present() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let pack = manifest.join("../../../local/packs/cnc");
        if !pack.join("conquer.mix").is_file() {
            return;
        }
        let staged = manifest.join(format!(
            "../../../local/agent_state/cnc/td-convert-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&staged);
        let assets = convert(&pack, &staged, &mut |_| {}).expect("convert local CNC pack");
        let worlds = assets
            .iter()
            .filter(|asset| asset.kind == makepad_asset_data::AssetKind::World)
            .collect::<Vec<_>>();
        assert!(worlds.len() >= 20, "worlds={}", worlds.len());
        for world in worlds {
            let glb = staged.join(&world.rel_path);
            for extension in ["glb", "place", "grid", "png"] {
                assert!(glb.with_extension(extension).is_file(), "{}", glb.with_extension(extension).display());
            }
            let grid = std::fs::read_to_string(glb.with_extension("grid")).unwrap();
            let place = std::fs::read_to_string(glb.with_extension("place")).unwrap();
            let roster_keys = place
                .lines()
                .filter_map(|line| line.strip_prefix("roster "))
                .flat_map(str::split_whitespace)
                .count();
            assert!(roster_keys >= 20, "roster keys={roster_keys}");
            let height = grid
                .lines()
                .find_map(|line| line.strip_prefix("size "))
                .and_then(|size| size.split_whitespace().nth(1))
                .and_then(|height| height.parse::<usize>().ok())
                .unwrap();
            assert_eq!(grid.lines().filter(|line| line.starts_with("row ")).count(), height);
        }
        let manifest = |key: &str| std::fs::read_to_string(staged.join(key)).unwrap();
        assert!(staged.join("billboards/cnc/mtnk.billboard").is_file());
        assert!(staged.join("billboards/cnc/mtnk-turret.billboard").is_file());
        let e1 = manifest("billboards/cnc/e1.billboard");
        for state in ["idle", "walk", "fire", "die"] {
            assert!(e1.contains(&format!("state {state} ")));
        }
        let fact = manifest("billboards/cnc/fact.billboard");
        assert!(fact.contains("footprint 3 2"));
        for state in ["idle", "damaged", "build"] {
            assert!(fact.contains(&format!("state {state} ")));
        }
        let ti = manifest("billboards/cnc/ti.billboard");
        assert_eq!(ti.lines().filter(|line| line.starts_with("frame ")).count(), 12);
        let sidebar = manifest("ui/cnc/sidebar.ui");
        assert!(sidebar.starts_with("ui-sheets 1\n"));
        assert!(
            sidebar
                .lines()
                .filter(|line| line.starts_with("sheet "))
                .count()
                >= 15
        );
        let hside1 = assets
            .iter()
            .find(|asset| asset.key == "ui/cnc/hside1")
            .expect("hside1 texture asset");
        assert_eq!(hside1.kind, makepad_asset_data::AssetKind::Texture);
        assert!(hside1.tags.iter().any(|tag| tag == "cnc"));
        assert!(hside1.tags.iter().any(|tag| tag == "ui"));
        let led_asset = assets
            .iter()
            .find(|asset| asset.key == "ui/cnc/font-led")
            .expect("LED font asset");
        assert_eq!(led_asset.kind, makepad_asset_data::AssetKind::Data);
        assert_eq!(led_asset.rel_path, "ui/cnc/font-led.font");
        let led = manifest("ui/cnc/font-led.font");
        let digit_glyphs = led
            .lines()
            .filter_map(|line| line.strip_prefix("glyph "))
            .filter_map(|line| line.split_whitespace().next())
            .filter_map(|code| code.parse::<u16>().ok())
            .filter(|code| (48..=57).contains(code))
            .count();
        assert!(digit_glyphs >= 10, "LED digit glyphs={digit_glyphs}");
        assert!(staged.join("sfx/cnc/await1.wav").is_file());
        for asset in &assets {
            if asset.kind == makepad_asset_data::AssetKind::Billboard {
                let text = std::fs::read_to_string(staged.join(&asset.rel_path)).unwrap();
                assert!(!text.contains("producer="), "{}", asset.rel_path);
            }
        }
        let _ = std::fs::remove_dir_all(&staged);
    }

    /// The split measured out of conquer.mix, frame by frame. The old
    /// `div_ceil(2)` answers are spelled out beside the right ones because
    /// every one of them put a damaged frame inside the intact loop.
    #[test]
    fn structure_split_keeps_damaged_art_out_of_the_intact_loop() {
        // NUKE (the power plant that flickered): 4 intact + 4 damaged + rubble.
        assert_eq!(structure_split(9), (4, 8));
        assert_ne!(structure_split(9).0, 9usize.div_ceil(2));
        // SILO: five tiberium fill stages, each with a damaged twin.
        assert_eq!(structure_split(11), (5, 10));
        // The three-frame buildings: one of each, then rubble.
        assert_eq!(structure_split(3), (1, 2));
        // The big animated ones.
        assert_eq!(structure_split(21), (10, 20));
        assert_eq!(structure_split(33), (16, 32));
        assert_eq!(structure_split(49), (24, 48));
        assert_eq!(structure_split(61), (30, 60));
        assert_eq!(structure_split(129), (64, 128));
        // Even counts (GUN's 128 facing frames) carry no rubble frame.
        assert_eq!(structure_split(128), (64, 128));
        assert_eq!(structure_split(2), (1, 2));
        // Degenerate counts must not produce an empty or inverted intact clip.
        assert_eq!(structure_split(1), (1, 1));
        assert_eq!(structure_split(0), (0, 0));
        for count in 1..200usize {
            let (healthy, damaged_end) = structure_split(count);
            assert!(healthy >= 1, "count={count}");
            assert!(healthy <= damaged_end, "count={count}");
            assert!(damaged_end <= count, "count={count}");
        }
    }

    /// The harvesting cycle is stored at 8 facings; the hull is drawn at 32.
    /// Every rot must map onto a whole four-step cycle, and neighbouring rots
    /// must share the facing they actually came from, or a working harvester
    /// snaps to one direction.
    #[test]
    fn harvest_source_covers_every_rot_with_a_whole_cycle() {
        // Every rot resolves inside HARV.SHP's second 32-frame block.
        for rot in 0..32usize {
            for step in 0..HARVEST_ANIM_STEPS {
                let source = harvest_source(rot, step);
                assert!((32..64).contains(&source), "rot {rot} step {step} -> {source}");
            }
        }
        // Rot 0 is the start of the first facing's cycle, which sits directly
        // after the 32 hull frames.
        assert_eq!(harvest_source(0, 0), 32);
        assert_eq!(harvest_source(0, 3), 35);
        // Four rots share a source facing; the fifth moves on to the next.
        for rot in 0..4 {
            assert_eq!(harvest_source(rot, 0), 32, "rot {rot}");
        }
        assert_eq!(harvest_source(4, 0), 36);
        assert_eq!(harvest_source(31, 3), 63);
        // Exactly eight distinct cycles are drawn from.
        let facings: std::collections::BTreeSet<usize> =
            (0..32).map(|rot| harvest_source(rot, 0)).collect();
        assert_eq!(facings.len(), HARVEST_ANIM_FACINGS);
    }
}
