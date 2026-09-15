//! Red Alert archive interpretation on top of the shared tiled-RTS emitter.

use super::{
    positive_unit_cost, rewrite_unit_roles, roster_key, PreviewDot, RoleTable, RtsEmitter,
    SpritePixels, SpriteSpec, SpriteState, UnitSpec, WorldSpec, CELL_M, TILE_PX,
};
use super::ra_templates::{TemplateTable, TEXT as TEMPLATE_TABLE_TEXT};
use crate::classic_import::{ClassicAsset, ConvertStage, ConvertTick};
use crate::cnc_import::{
    aud::Aud,
    ini::Ini,
    map::{MapBounds, RaMap},
    mix::{HashKind, MixFile},
    pal::Pal,
    shp::Shp,
    tmp::Tmp,
    NameTable,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f32::consts::TAU;
use std::path::Path;

const VEHICLE_KEYS: &[&str] = &[
    "1tnk", "2tnk", "3tnk", "4tnk", "v2rl", "arty", "jeep", "apc", "mnly",
    "harv", "mcv", "mrj", "mgg", "ttnk", "ctnk", "dtrk", "stnk", "truk",
];
const VEHICLE_TURRET_KEYS: &[&str] = &["1tnk", "2tnk", "3tnk", "4tnk"];
const AIRCRAFT_KEYS: &[&str] = &["heli", "hind", "yak", "mig", "tran", "badr", "u2"];
const SHIP_KEYS: &[&str] = &["ss", "dd", "ca", "pt", "lst"];
const INFANTRY_KEYS: &[&str] = &[
    "e1", "e2", "e3", "e4", "e6", "e7", "medi", "spy", "thf", "dog", "c1",
    "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "c10", "einstein", "gnrl",
    "delphi", "chan", "shok", "mech",
];
const STRUCTURE_KEYS: &[&str] = &[
    "fact", "powr", "apwr", "proc", "silo", "tent", "barr", "weap", "dome", "fix",
    "hpad", "afld", "atek", "stek", "iron", "pdox", "mslo", "gap", "syrd", "spen",
    "kenn", "pbox", "hbox", "gun", "ftur", "tsla", "sam", "agun", "bio", "hosp",
    "miss", "fcom", "sbag", "cycl", "brik", "barb", "wood", "fenc",
];
const EFFECT_KEYS: &[&str] = &[
    "piff", "piffpiff", "fball1", "fire1", "fire2", "fire3", "napalm1", "napalm2",
    "napalm3", "art-exp1", "frag1", "frag3", "veh-hit1", "veh-hit2", "veh-hit3",
    "smokey", "atomsfx", "ionsfx", "flmspt", "bomb", "bombs",
];
const PROJECTILES: &[(&str, &str)] = &[
    ("120mm", "120mm"),
    ("50cal", "50cal"),
    ("dragon", "dragon"),
    ("bomblet", "bomblet"),
    ("flame", "flame-n"),
    ("missile", "missile"),
    ("patriot", "patriot"),
    ("laser", "laser"),
];

pub(super) const ROLE_TABLE: RoleTable = &[
    ("fact", "conyard"),
    ("powr", "power"),
    ("apwr", "power"),
    ("proc", "refinery"),
    ("silo", "silo"),
    ("tent", "barracks"),
    ("barr", "barracks"),
    ("kenn", "barracks"),
    ("weap", "vehicle_factory"),
    ("hpad", "aircraft_pad"),
    ("afld", "aircraft_pad"),
    ("dome", "radar"),
    ("atek", "tech"),
    ("stek", "tech"),
    ("fix", "repair"),
    ("iron", "superweapon"),
    ("pdox", "superweapon"),
    ("mslo", "superweapon"),
    ("gap", "tech"),
    ("syrd", "naval_yard"),
    ("spen", "naval_yard"),
    ("pbox", "defense"),
    ("hbox", "defense"),
    ("gun", "defense"),
    ("ftur", "defense"),
    ("tsla", "defense"),
    ("sam", "defense"),
    ("agun", "defense"),
    ("sbag", "wall"),
    ("cycl", "wall"),
    ("brik", "wall"),
    ("barb", "wall"),
    ("wood", "wall"),
    ("fenc", "wall"),
];

struct Archives {
    conquer: Vec<u8>,
    general: Vec<u8>,
    sounds: Vec<u8>,
    allies: Vec<u8>,
    russian: Vec<u8>,
    temperat: Vec<u8>,
    snow: Vec<u8>,
    interior: Vec<u8>,
    nested: Vec<Vec<u8>>,
}

impl Archives {
    fn load(pack: &Path) -> Result<Self, String> {
        let redalert = read_pack_file(pack, "redalert.mix")?;
        let outer = MixFile::parse(&redalert).map_err(|e| format!("redalert.mix: {e}"))?;
        let mut nested = Vec::new();
        for entry in outer.entries() {
            let Some(bytes) = outer.by_id(entry.id) else {
                continue;
            };
            if MixFile::parse(bytes).is_ok() {
                nested.push(bytes.to_vec());
            }
        }
        Ok(Self {
            conquer: read_pack_file(pack, "conquer.mix")?,
            general: read_pack_file(pack, "general.mix")?,
            sounds: read_pack_file(pack, "sounds.mix")?,
            allies: read_pack_file(pack, "allies.mix")?,
            russian: read_pack_file(pack, "russian.mix")?,
            temperat: read_pack_file(pack, "temperat.mix")?,
            snow: read_pack_file(pack, "snow.mix")?,
            interior: read_pack_file(pack, "interior.mix")?,
            nested,
        })
    }

    fn core_entry(&self, name: &str) -> Option<&[u8]> {
        mix_entry(&self.conquer, name).or_else(|| {
            self.nested
                .iter()
                .find_map(|archive| mix_entry(archive, name))
        })
    }

    fn shp(&self, stem: &str) -> Option<Shp> {
        Shp::parse(self.core_entry(&format!("{}.SHP", stem.to_ascii_uppercase()))?).ok()
    }

    fn palette(&self, name: &str) -> Option<Pal> {
        let bytes = self
            .nested
            .iter()
            .find_map(|archive| mix_entry(archive, name))
            .or_else(|| mix_entry(&self.temperat, name))
            .or_else(|| mix_entry(&self.snow, name))
            .or_else(|| mix_entry(&self.interior, name))?;
        Pal::parse(bytes).ok()
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
    fn load(
        theater: &str,
        archives: &'a Archives,
        table: &TemplateTable,
        report: &mut ConvertReport,
    ) -> Result<Self, String> {
        let (key, extension, archive, palette_name) =
            match theater.to_ascii_uppercase().as_str() {
                "TEMPERATE" => (
                    "temperat",
                    "TEM",
                    archives.temperat.as_slice(),
                    "TEMPERAT.PAL",
                ),
                "SNOW" => ("snow", "SNO", archives.snow.as_slice(), "SNOW.PAL"),
                "INTERIOR" => (
                    "interior",
                    "INT",
                    archives.interior.as_slice(),
                    "INTERIOR.PAL",
                ),
                other => return Err(format!("unsupported Red Alert theater {other}")),
            };
        let palette = if let Some(palette) = archives.palette(palette_name) {
            palette
        } else {
            report
                .palette_fallbacks
                .insert(format!("{palette_name} missing; used TEMPERAT.PAL"));
            archives
                .palette("TEMPERAT.PAL")
                .ok_or_else(|| format!("{theater}: no theater or temperate palette"))?
        };
        let mut templates = HashMap::new();
        let mut stems = table.stems(key).map(str::to_owned).collect::<BTreeSet<_>>();
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
            .or_else(|| {
                self.templates
                    .get("clear1")
                    .and_then(|tmp| tmp.icon(icon % 16))
            })
            .or_else(|| self.templates.get("clear1").and_then(|tmp| tmp.icon(0)));
        if let Some(indexed) = selected {
            indexed_opaque(indexed, &self.palette, dst);
        } else {
            for pixel in dst.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[24, 24, 24, 255]);
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

#[derive(Default)]
struct ConvertReport {
    maps: Vec<MapSummary>,
    overlay_counts: BTreeMap<u8, usize>,
    roles: BTreeMap<String, usize>,
    resolved_scenery: BTreeSet<String>,
    unresolved_scenery: BTreeSet<String>,
    missing_shapes: BTreeSet<String>,
    missing_icons: BTreeSet<String>,
    missing_audio: BTreeSet<String>,
    palette_fallbacks: BTreeSet<String>,
    structure_halves: Vec<String>,
    remap_note: String,
    e1_frames: usize,
    effect_resolved: usize,
    projectile_resolved: usize,
    side_speech_entries: Vec<String>,
    audio_stems: BTreeSet<String>,
}

struct AudioAsset {
    aud: Aud,
    speech: bool,
}

pub fn convert(
    pack_dir: &Path,
    staged: &Path,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<Vec<ClassicAsset>, String> {
    let archives = Archives::load(pack_dir)?;
    let names = NameTable::with_hash_kind(HashKind::RotateAdd);
    let table = TemplateTable::parse(TEMPLATE_TABLE_TEXT)?;
    let general = MixFile::parse(&archives.general).map_err(|e| e.to_string())?;
    let map_ids = names
        .names()
        .filter_map(|name| name.strip_suffix(".INI"))
        .filter(|stem| stem.to_ascii_uppercase().starts_with("SCM"))
        .filter(|stem| general.by_name(&format!("{stem}.INI")).is_some())
        .filter(|stem| {
            general
                .by_name(&format!("{stem}.INI"))
                .map(|bytes| Ini::parse(&String::from_utf8_lossy(bytes)))
                .is_some_and(|ini| ini.section("MapPack").is_some())
        })
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if map_ids.is_empty() {
        return Err("general.mix contains no resolved packed Red Alert maps".into());
    }

    let mut emitter = RtsEmitter::new(staged, "ra")?;
    let mut report = ConvertReport::default();
    for (index, map_id) in map_ids.iter().enumerate() {
        tick(on_tick, index, map_ids.len(), format!("world {map_id}"), None);
        let bytes = general
            .by_name(&format!("{map_id}.INI"))
            .ok_or_else(|| format!("{map_id}.INI disappeared"))?;
        let ini = Ini::parse(&String::from_utf8_lossy(bytes));
        let map = RaMap::parse(&ini).map_err(|e| format!("{map_id}: {e}"))?;
        let mut theater = TheaterBank::load(&map.theater, &archives, &table, &mut report)?;
        let (world, summary) =
            world_spec(map_id, &ini, &map, &mut theater, &table, &archives, &mut report)?;
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

    let palette = archives
        .palette("TEMPERAT.PAL")
        .ok_or("TEMPERAT.PAL missing from Red Alert pack")?;
    report.remap_note = verify_remap_ramp(&archives, &palette);
    let audio = resolve_audio(&archives, &names, &mut report)?;
    let audio_stems = audio.keys().cloned().collect::<BTreeSet<_>>();
    report.audio_stems = audio_stems.clone();
    emit_sprites(
        &mut emitter,
        &archives,
        &palette,
        &audio_stems,
        &mut report,
        on_tick,
    )?;
    emit_audio(&mut emitter, &audio, on_tick)?;
    write_report(&report)?;
    Ok(emitter.finish())
}

fn world_spec(
    map_id: &str,
    ini: &Ini,
    map: &RaMap,
    theater: &mut TheaterBank<'_>,
    table: &TemplateTable,
    archives: &Archives,
    report: &mut ConvertReport,
) -> Result<(WorldSpec, MapSummary), String> {
    let bounds = map.bounds;
    let mut grid = vec![vec!['.'; bounds.width as usize]; bounds.height as usize];
    let terrain_rgba = RtsEmitter::paint_cell_map(
        (bounds.x, bounds.y, bounds.width, bounds.height),
        |x, y, tile| {
            let (template_id, source_icon) = map.cell(x as usize, y as usize);
            if template_id == 0xffff {
                theater.paint_template(
                    "clear1",
                    (x as usize % 4) + (y as usize % 4) * 4,
                    tile,
                );
            } else if let Some(def) = table.get(theater.key, template_id) {
                if theater.has_template(&def.stem) {
                    theater.paint_template(&def.stem, source_icon as usize, tile);
                } else {
                    theater.paint_template(
                        "clear1",
                        (x as usize % 4) + (y as usize % 4) * 4,
                        tile,
                    );
                }
            } else {
                theater.paint_template(
                    "clear1",
                    (x as usize % 4) + (y as usize % 4) * 4,
                    tile,
                );
            }
        },
    );
    let mut terrain_rgba = terrain_rgba;
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let (template_id, icon) = map.cell((bounds.x + x) as usize, (bounds.y + y) as usize);
            let class = if template_id == 0xffff {
                'c'
            } else if let Some(def) = table.get(theater.key, template_id) {
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
        let Some((x, y)) = local_cell(bounds, smudge.cell) else {
            continue;
        };
        let key = smudge.name.to_ascii_lowercase();
        if !is_smudge(&key) {
            continue;
        }
        if let Some(shp) = theater.shp(&key) {
            if let Some(frame) = shp.frames().first() {
                blit_indexed(
                    &mut terrain_rgba,
                    bounds.width as u32 * TILE_PX,
                    bounds.height as u32 * TILE_PX,
                    x as u32 * TILE_PX,
                    y as u32 * TILE_PX,
                    frame,
                    shp.width() as u32,
                    shp.height() as u32,
                    &theater.palette,
                );
            }
        } else {
            report.unresolved_scenery.insert(smudge.name.clone());
        }
    }
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let source_x = bounds.x + x;
            let source_y = bounds.y + y;
            let Some(id) = map.overlay(source_x as usize, source_y as usize) else {
                continue;
            };
            *report.overlay_counts.entry(id).or_default() += 1;
            let OverlayKind::Bake(stem) = overlay_kind(id) else {
                continue;
            };
            let shp = theater.shp(stem).or_else(|| archives.shp(stem));
            if let Some(shp) = shp {
                if let Some(frame) = shp.frames().first() {
                    blit_indexed(
                        &mut terrain_rgba,
                        bounds.width as u32 * TILE_PX,
                        bounds.height as u32 * TILE_PX,
                        x as u32 * TILE_PX,
                        y as u32 * TILE_PX,
                        frame,
                        shp.width() as u32,
                        shp.height() as u32,
                        &theater.palette,
                    );
                }
            } else {
                report.unresolved_scenery.insert(stem.to_ascii_uppercase());
            }
        }
    }

    let mut houses = BTreeSet::from(["Greece".to_owned(), "USSR".to_owned()]);
    for owner in map
        .units
        .iter()
        .chain(&map.ships)
        .map(|row| row.owner.as_str())
        .chain(map.infantry.iter().map(|row| row.owner.as_str()))
        .chain(map.structures.iter().map(|row| row.owner.as_str()))
    {
        houses.insert(canonical_owner(owner));
    }
    let world_key = format!("worlds/{}", map_id.to_ascii_lowercase());
    let mut place = format!(
        "world-place 1\nsource ra\nworld {world_key}\nmode rts\ncell 6.0\ngrid {world_key}.grid\n"
    );
    let mut ordered_houses = houses.into_iter().collect::<Vec<_>>();
    ordered_houses.sort_by_key(|house| house_order(house));
    for house in &ordered_houses {
        let info = house_info(house);
        place.push_str(&format!(
            "house {house} color={} side={}\n",
            info.color, info.side
        ));
    }

    let mut dots = Vec::new();
    let mut emitted_units = 0usize;
    for unit in &map.units {
        let Some((lx, ly)) = local_cell(bounds, unit.cell) else {
            continue;
        };
        let key = unit.kind.to_ascii_lowercase();
        let owner = canonical_owner(&unit.owner);
        let (x, z) = ra_cell_to_metres(bounds.x, bounds.y, unit.cell);
        let class = mobile_class(&key);
        place.push_str(&format!(
            "place u-{} unit billboards/ra/{key} {x:.4} 0.1000 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.10 class={class}\n",
            unit.number,
            source_yaw(unit.facing),
            health_fraction(unit.health),
        ));
        dots.push(PreviewDot {
            x: lx as f32 + 0.5,
            y: ly as f32 + 0.5,
            rgb: color_rgb(house_info(&owner).color),
        });
        emitted_units += 1;
    }
    for infantry in &map.infantry {
        let Some((lx, ly)) = local_cell(bounds, infantry.cell) else {
            continue;
        };
        let key = infantry.kind.to_ascii_lowercase();
        let owner = canonical_owner(&infantry.owner);
        let (mut x, mut z) = ra_cell_to_metres(bounds.x, bounds.y, infantry.cell);
        let (ox, oz) = subcell_offset(infantry.sub_cell);
        x += ox;
        z += oz;
        place.push_str(&format!(
            "place i-{} unit billboards/ra/{key} {x:.4} 0.1200 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.12 class=infantry\n",
            infantry.number,
            source_yaw(infantry.facing),
            health_fraction(infantry.health),
        ));
        dots.push(PreviewDot {
            x: lx as f32 + 0.5 + ox / CELL_M,
            y: ly as f32 + 0.5 + oz / CELL_M,
            rgb: color_rgb(house_info(&owner).color),
        });
        emitted_units += 1;
    }
    for ship in &map.ships {
        let Some((lx, ly)) = local_cell(bounds, ship.cell) else {
            continue;
        };
        let key = ship.kind.to_ascii_lowercase();
        let owner = canonical_owner(&ship.owner);
        let (x, z) = ra_cell_to_metres(bounds.x, bounds.y, ship.cell);
        place.push_str(&format!(
            "place b-{} unit billboards/ra/{key} {x:.4} 0.1000 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.10 class=boat\n",
            ship.number,
            source_yaw(ship.facing),
            health_fraction(ship.health),
        ));
        dots.push(PreviewDot {
            x: lx as f32 + 0.5,
            y: ly as f32 + 0.5,
            rgb: color_rgb(house_info(&owner).color),
        });
        emitted_units += 1;
    }

    let mut emitted_structures = 0usize;
    for structure in &map.structures {
        let Some((lx, ly)) = local_cell(bounds, structure.cell) else {
            continue;
        };
        let key = structure.kind.to_ascii_lowercase();
        let owner = canonical_owner(&structure.owner);
        let (fw, fh) = structure_footprint(&key);
        block_rect(&mut grid, lx, ly, fw, fh, '#');
        let (x, z) = ra_cell_to_metres(bounds.x, bounds.y, structure.cell);
        let class = if is_defense(&key) {
            "defense"
        } else {
            "structure"
        };
        place.push_str(&format!(
            "place s-{} structure billboards/ra/{key} {x:.4} 0.0600 {z:.4} {:.5} team={owner} hp={:.2} align=floor layer=0.06 class={class} w={} h={}\n",
            structure.number,
            source_yaw(structure.facing),
            health_fraction(structure.health),
            fw as u32 * 6,
            fh as u32 * 6,
        ));
        dots.push(PreviewDot {
            x: lx as f32 + fw as f32 * 0.5,
            y: ly as f32 + fh as f32 * 0.5,
            rgb: color_rgb(house_info(&owner).color),
        });
        emitted_structures += 1;
    }

    let mut scenery_rows = 0usize;
    for (index, terrain) in map.terrain.iter().enumerate() {
        let Some((lx, ly)) = local_cell(bounds, terrain.cell) else {
            continue;
        };
        let key = terrain.name.to_ascii_lowercase();
        let Some(shp) = theater.shp(&key) else {
            report.unresolved_scenery.insert(terrain.name.clone());
            continue;
        };
        report.resolved_scenery.insert(terrain.name.clone());
        let width_cells = (shp.width() as usize).div_ceil(TILE_PX as usize).max(1);
        let height_cells = (shp.height() as usize).div_ceil(TILE_PX as usize).max(1);
        block_rect(&mut grid, lx, ly, width_cells, height_cells, '#');
        let (x, z) = ra_cell_to_metres(bounds.x, bounds.y, terrain.cell);
        let class = if key.starts_with('t') {
            "tree"
        } else if key.starts_with("boxes") {
            "scenery"
        } else {
            "rock"
        };
        place.push_str(&format!(
            "place t-{index} scenery billboards/ra/{key} {x:.4} 0.0600 {z:.4} 0.00000 align=floor layer=0.06 class={class}\n"
        ));
        scenery_rows += 1;
    }

    let mut resource_rows = 0usize;
    let mut overlay_index = 0usize;
    for source_y in bounds.y..bounds.y + bounds.height {
        for source_x in bounds.x..bounds.x + bounds.width {
            let Some(id) = map.overlay(source_x as usize, source_y as usize) else {
                continue;
            };
            let lx = (source_x - bounds.x) as usize;
            let ly = (source_y - bounds.y) as usize;
            let cell = source_y * 128 + source_x;
            let (x, z) = ra_cell_to_metres(bounds.x, bounds.y, cell);
            match overlay_kind(id) {
                OverlayKind::Wall(key) => {
                    grid[ly][lx] = '#';
                    place.push_str(&format!(
                        "place w-{overlay_index} scenery billboards/ra/{key} {x:.4} 0.0600 {z:.4} 0.00000 align=floor layer=0.06 class=wall\n"
                    ));
                    scenery_rows += 1;
                }
                OverlayKind::Resource(stage) => {
                    grid[ly][lx] = 't';
                    place.push_str(&format!(
                        "place r-{overlay_index} resource billboards/ra/ore {x:.4} 0.0400 {z:.4} 0.00000 align=floor layer=0.04 class=resource stage={stage}\n"
                    ));
                    resource_rows += 1;
                }
                OverlayKind::Scenery(key) => {
                    place.push_str(&format!(
                        "place o-{overlay_index} scenery billboards/ra/{key} {x:.4} 0.0600 {z:.4} 0.00000 align=floor layer=0.06 class=scenery\n"
                    ));
                    scenery_rows += 1;
                }
                OverlayKind::Bake(_) | OverlayKind::Unknown => {}
            }
            overlay_index += 1;
        }
    }

    let starts = waypoint_starts(map);
    let mut spawn = String::from("world-spawn 1\n");
    for (name, x, z) in &starts {
        spawn.push_str(&format!(
            "start {name} {x:.4} 0.0000 {z:.4} 0.00000 -1.45000\n"
        ));
    }
    spawn.push_str("floor 0\nstep 0.5\neye 60\n");
    let title = ini.get("Basic", "Name").unwrap_or(map_id).to_owned();
    let summary = MapSummary {
        id: map_id.to_owned(),
        title,
        theater: map.theater.clone(),
        bounds: format!(
            "{},{},{},{}",
            bounds.x, bounds.y, bounds.width, bounds.height
        ),
        units: emitted_units,
        structures: emitted_structures,
        scenery: scenery_rows,
        resources: resource_rows,
        starts: starts.len(),
    };
    Ok((
        WorldSpec {
            key: map_id.to_ascii_lowercase(),
            width: bounds.width,
            height: bounds.height,
            terrain_rgba,
            grid: grid
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            place_text: place,
            roster: pack_roster(),
            spawn_text: spawn,
            preview_dots: dots,
            preview_crop: None,
            tags: vec![
                "ra".into(),
                "world".into(),
                "rts".into(),
                map.theater.to_ascii_lowercase(),
            ],
        },
        summary,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayKind {
    Wall(&'static str),
    Resource(u8),
    Bake(&'static str),
    Scenery(&'static str),
    Unknown,
}

fn overlay_kind(id: u8) -> OverlayKind {
    match id {
        0 => OverlayKind::Wall("sbag"),
        1 => OverlayKind::Wall("cycl"),
        2 => OverlayKind::Wall("brik"),
        3 => OverlayKind::Wall("barb"),
        4 => OverlayKind::Wall("wood"),
        5..=8 => OverlayKind::Resource(id - 5),
        9..=12 => OverlayKind::Resource(id - 5),
        13 => OverlayKind::Bake("v12"),
        14 => OverlayKind::Bake("v13"),
        15 => OverlayKind::Bake("v14"),
        16 => OverlayKind::Bake("v15"),
        17 => OverlayKind::Bake("v16"),
        18 => OverlayKind::Bake("v17"),
        19 => OverlayKind::Bake("v18"),
        20 => OverlayKind::Bake("fpls"),
        21 => OverlayKind::Scenery("wcrate"),
        22 => OverlayKind::Scenery("scrate"),
        23 => OverlayKind::Wall("fenc"),
        // WWCRATE is the water-crate image; expose it through the same
        // scenery contract asset as WCRATE rather than inventing a unit key.
        24 => OverlayKind::Scenery("wcrate"),
        _ => OverlayKind::Unknown,
    }
}

#[derive(Clone, Copy)]
struct UnitManifest {
    key: &'static str,
    line: &'static str,
    weapons: &'static [&'static str],
}

const UNIT_MANIFESTS: &[UnitManifest] = &[
    UnitManifest { key: "e1", line: "unit class=infantry title=\"Rifle Infantry\" cost=100 hp=50 armor=none speed=2.5 sight=12 sides=allies,soviet producer=tent|barr weapon=m1carbine", weapons: &["m1carbine"] },
    UnitManifest { key: "e2", line: "unit class=infantry title=\"Grenadier\" cost=160 hp=50 armor=none speed=3.0 sight=12 sides=allies,soviet producer=tent|barr weapon=grenade", weapons: &["grenade"] },
    UnitManifest { key: "e3", line: "unit class=infantry title=\"Rocket Soldier\" cost=300 hp=45 armor=none speed=2.5 sight=12 sides=allies,soviet producer=tent|barr weapon=redeye", weapons: &["redeye"] },
    UnitManifest { key: "e4", line: "unit class=infantry title=\"Flamethrower\" cost=300 hp=40 armor=none speed=2.5 sight=12 sides=soviet producer=barr weapon=flamer", weapons: &["flamer"] },
    UnitManifest { key: "e6", line: "unit class=infantry title=\"Engineer\" cost=500 hp=25 armor=none speed=2.5 sight=12 sides=allies,soviet producer=tent|barr capture=1", weapons: &[] },
    UnitManifest { key: "e7", line: "unit class=infantry title=\"Tanya\" cost=1200 hp=50 armor=none speed=3.0 sight=18 sides=allies producer=tent prereq=atek weapon=colt45 c4=1", weapons: &["colt45"] },
    UnitManifest { key: "medi", line: "unit class=infantry title=\"Medic\" cost=800 hp=80 armor=none speed=2.5 sight=12 sides=allies producer=tent", weapons: &[] },
    UnitManifest { key: "spy", line: "unit class=infantry title=\"Spy\" cost=500 hp=25 armor=none speed=2.5 sight=18 sides=allies producer=tent prereq=dome", weapons: &[] },
    UnitManifest { key: "dog", line: "unit class=infantry title=\"Attack Dog\" cost=200 hp=12 armor=none speed=8.0 sight=18 sides=soviet producer=kenn weapon=bite", weapons: &["bite"] },
    UnitManifest { key: "shok", line: "unit class=infantry title=\"Shock Trooper\" cost=600 hp=80 armor=none speed=2.5 sight=12 sides=soviet producer=barr prereq=tsla weapon=shock", weapons: &["shock"] },
    UnitManifest { key: "jeep", line: "unit class=vehicle title=\"Ranger\" cost=600 hp=150 armor=light speed=10.0 sight=18 sides=allies producer=weap weapon=m60", weapons: &["m60"] },
    UnitManifest { key: "apc", line: "unit class=vehicle title=\"APC\" cost=800 hp=200 armor=heavy speed=9.0 sight=18 sides=allies,soviet producer=weap prereq=tent|barr weapon=m60", weapons: &["m60"] },
    UnitManifest { key: "1tnk", line: "unit class=vehicle title=\"Light Tank\" cost=700 hp=300 armor=heavy speed=7.0 sight=18 sides=allies producer=weap weapon=75mm turret=billboards/ra/1tnk-turret", weapons: &["75mm"] },
    UnitManifest { key: "2tnk", line: "unit class=vehicle title=\"Medium Tank\" cost=800 hp=400 armor=heavy speed=6.0 sight=18 sides=allies producer=weap weapon=90mm turret=billboards/ra/2tnk-turret", weapons: &["90mm"] },
    UnitManifest { key: "3tnk", line: "unit class=vehicle title=\"Heavy Tank\" cost=950 hp=400 armor=heavy speed=5.0 sight=18 sides=soviet producer=weap weapon=105mm weapon2=105mm turret=billboards/ra/3tnk-turret", weapons: &["105mm"] },
    UnitManifest { key: "4tnk", line: "unit class=vehicle title=\"Mammoth Tank\" cost=1700 hp=600 armor=heavy speed=4.0 sight=18 sides=soviet producer=weap prereq=stek weapon=120mm weapon2=mammoth_tusk turret=billboards/ra/4tnk-turret", weapons: &["120mm", "mammoth_tusk"] },
    UnitManifest { key: "arty", line: "unit class=vehicle title=\"Artillery\" cost=600 hp=75 armor=light speed=6.0 sight=24 sides=allies producer=weap weapon=155mm", weapons: &["155mm"] },
    UnitManifest { key: "v2rl", line: "unit class=vehicle title=\"V2 Rocket\" cost=700 hp=150 armor=light speed=6.0 sight=24 sides=soviet producer=weap weapon=scud", weapons: &["scud"] },
    UnitManifest { key: "mnly", line: "unit class=vehicle title=\"Minelayer\" cost=800 hp=100 armor=heavy speed=7.0 sight=12 sides=allies,soviet producer=weap prereq=fix", weapons: &[] },
    UnitManifest { key: "harv", line: "unit class=vehicle title=\"Ore Truck\" cost=1400 hp=600 armor=heavy speed=5.0 sight=12 sides=allies,soviet producer=weap prereq=proc harvester=1 capacity=700", weapons: &[] },
    UnitManifest { key: "mcv", line: "unit class=vehicle title=\"MCV\" cost=2500 hp=600 armor=heavy speed=4.0 sight=12 sides=allies,soviet producer=weap mcv=1 deploys=fact", weapons: &[] },
    UnitManifest { key: "mrj", line: "unit class=vehicle title=\"Radar Jammer\" cost=600 hp=110 armor=light speed=8.0 sight=24 sides=allies producer=weap prereq=dome", weapons: &[] },
    UnitManifest { key: "mgg", line: "unit class=vehicle title=\"Mobile Gap\" cost=600 hp=110 armor=light speed=8.0 sight=24 sides=allies producer=weap prereq=atek", weapons: &[] },
    UnitManifest { key: "ttnk", line: "unit class=vehicle title=\"Tesla Tank\" cost=1500 hp=300 armor=heavy speed=6.0 sight=18 sides=soviet producer=weap prereq=stek weapon=teslazap", weapons: &["teslazap"] },
    UnitManifest { key: "ctnk", line: "unit class=vehicle title=\"Chrono Tank\" cost=2400 hp=200 armor=light speed=8.0 sight=18 sides=allies producer=weap prereq=pdox weapon=227mm", weapons: &["227mm"] },
    UnitManifest { key: "heli", line: "unit class=aircraft title=\"Longbow\" cost=1200 hp=125 armor=light speed=18.0 sight=24 sides=allies producer=hpad weapon=hellfire", weapons: &["hellfire"] },
    UnitManifest { key: "hind", line: "unit class=aircraft title=\"Hind\" cost=1200 hp=225 armor=light speed=15.0 sight=24 sides=soviet producer=hpad weapon=chaingun", weapons: &["chaingun"] },
    UnitManifest { key: "yak", line: "unit class=aircraft title=\"Yak\" cost=800 hp=80 armor=light speed=20.0 sight=24 sides=soviet producer=afld weapon=chaingun", weapons: &["chaingun"] },
    UnitManifest { key: "mig", line: "unit class=aircraft title=\"MiG\" cost=2000 hp=100 armor=light speed=22.0 sight=24 sides=soviet producer=afld prereq=stek weapon=maverick", weapons: &["maverick"] },
    UnitManifest { key: "fact", line: "unit class=structure title=\"Construction Yard\" cost=2500 hp=1000 armor=concrete speed=0 sight=30 sides=allies,soviet footprint=3x2 power=0 build=1", weapons: &[] },
    UnitManifest { key: "powr", line: "unit class=structure title=\"Power Plant\" cost=300 hp=400 armor=wood speed=0 sight=18 sides=allies,soviet producer=fact footprint=2x2 power=+100 build=1", weapons: &[] },
    UnitManifest { key: "apwr", line: "unit class=structure title=\"Adv. Power Plant\" cost=500 hp=700 armor=wood speed=0 sight=18 sides=allies,soviet producer=fact prereq=powr footprint=3x3 power=+200 build=1", weapons: &[] },
    UnitManifest { key: "proc", line: "unit class=structure title=\"Ore Refinery\" cost=2000 hp=900 armor=wood speed=0 sight=24 sides=allies,soviet producer=fact prereq=powr footprint=3x3 power=-30 refinery=1 build=1", weapons: &[] },
    UnitManifest { key: "silo", line: "unit class=structure title=\"Ore Silo\" cost=150 hp=300 armor=wood speed=0 sight=12 sides=allies,soviet producer=fact prereq=proc footprint=1x1 power=-10 build=1", weapons: &[] },
    UnitManifest { key: "tent", line: "unit class=structure title=\"Allied Barracks\" cost=300 hp=400 armor=wood speed=0 sight=18 sides=allies producer=fact prereq=powr footprint=2x2 power=-20 build=1", weapons: &[] },
    UnitManifest { key: "barr", line: "unit class=structure title=\"Soviet Barracks\" cost=300 hp=400 armor=wood speed=0 sight=18 sides=soviet producer=fact prereq=powr footprint=2x2 power=-20 build=1", weapons: &[] },
    UnitManifest { key: "kenn", line: "unit class=structure title=\"Kennel\" cost=200 hp=400 armor=wood speed=0 sight=18 sides=soviet producer=fact prereq=barr footprint=1x1 power=-10 build=1", weapons: &[] },
    UnitManifest { key: "weap", line: "unit class=structure title=\"War Factory\" cost=2000 hp=900 armor=light speed=0 sight=18 sides=allies,soviet producer=fact prereq=proc footprint=3x2 power=-30 build=1", weapons: &[] },
    UnitManifest { key: "dome", line: "unit class=structure title=\"Radar Dome\" cost=1000 hp=1000 armor=wood speed=0 sight=30 sides=allies,soviet producer=fact prereq=proc footprint=2x2 power=-40 build=1", weapons: &[] },
    UnitManifest { key: "fix", line: "unit class=structure title=\"Service Depot\" cost=1200 hp=800 armor=wood speed=0 sight=18 sides=allies,soviet producer=fact prereq=weap footprint=3x3 power=-30 build=1", weapons: &[] },
    UnitManifest { key: "hpad", line: "unit class=structure title=\"Helipad\" cost=1500 hp=400 armor=wood speed=0 sight=18 sides=allies,soviet producer=fact prereq=dome footprint=2x2 power=-10 build=1", weapons: &[] },
    UnitManifest { key: "afld", line: "unit class=structure title=\"Airfield\" cost=600 hp=600 armor=heavy speed=0 sight=18 sides=soviet producer=fact prereq=dome footprint=3x2 power=-30 build=1", weapons: &[] },
    UnitManifest { key: "atek", line: "unit class=structure title=\"Allied Tech Center\" cost=1500 hp=400 armor=wood speed=0 sight=30 sides=allies producer=fact prereq=dome footprint=2x2 power=-200 build=1", weapons: &[] },
    UnitManifest { key: "stek", line: "unit class=structure title=\"Soviet Tech Center\" cost=1500 hp=400 armor=wood speed=0 sight=30 sides=soviet producer=fact prereq=dome footprint=2x3 power=-100 build=1", weapons: &[] },
    UnitManifest { key: "iron", line: "unit class=structure title=\"Iron Curtain\" cost=2800 hp=400 armor=concrete speed=0 sight=30 sides=soviet producer=fact prereq=stek footprint=2x2 power=-200 build=1", weapons: &[] },
    UnitManifest { key: "pdox", line: "unit class=structure title=\"Chronosphere\" cost=2800 hp=400 armor=concrete speed=0 sight=30 sides=allies producer=fact prereq=atek footprint=2x2 power=-200 build=1", weapons: &[] },
    UnitManifest { key: "mslo", line: "unit class=structure title=\"Missile Silo\" cost=2500 hp=400 armor=concrete speed=0 sight=30 sides=allies,soviet producer=fact prereq=atek|stek footprint=2x1 power=-100 build=1", weapons: &[] },
    UnitManifest { key: "gap", line: "unit class=structure title=\"Gap Generator\" cost=500 hp=400 armor=wood speed=0 sight=30 sides=allies producer=fact prereq=atek footprint=1x1 power=-60 build=1", weapons: &[] },
    UnitManifest { key: "syrd", line: "unit class=structure title=\"Naval Yard\" cost=1000 hp=1000 armor=heavy speed=0 sight=24 sides=allies producer=fact prereq=powr footprint=3x3 power=-30 build=1", weapons: &[] },
    UnitManifest { key: "spen", line: "unit class=structure title=\"Sub Pen\" cost=1000 hp=1000 armor=heavy speed=0 sight=24 sides=soviet producer=fact prereq=powr footprint=3x3 power=-30 build=1", weapons: &[] },
    UnitManifest { key: "pbox", line: "unit class=defense title=\"Pillbox\" cost=400 hp=400 armor=concrete speed=0 sight=24 sides=allies producer=fact prereq=tent footprint=1x1 power=-15 weapon=m60 build=1", weapons: &["m60"] },
    UnitManifest { key: "hbox", line: "unit class=defense title=\"Camo Pillbox\" cost=600 hp=600 armor=concrete speed=0 sight=24 sides=allies producer=fact prereq=tent footprint=1x1 power=-15 weapon=m60 build=1", weapons: &["m60"] },
    UnitManifest { key: "gun", line: "unit class=defense title=\"Turret\" cost=600 hp=400 armor=heavy speed=0 sight=30 sides=allies producer=fact prereq=tent footprint=1x1 power=-40 weapon=105mm turret=billboards/ra/gun-turret build=1", weapons: &["105mm"] },
    UnitManifest { key: "ftur", line: "unit class=defense title=\"Flame Tower\" cost=600 hp=400 armor=wood speed=0 sight=18 sides=soviet producer=fact prereq=barr footprint=1x1 power=-20 weapon=flamer_tower build=1", weapons: &["flamer_tower"] },
    UnitManifest { key: "tsla", line: "unit class=defense title=\"Tesla Coil\" cost=1500 hp=400 armor=concrete speed=0 sight=36 sides=soviet producer=fact prereq=stek footprint=1x1 power=-150 weapon=tesla build=1", weapons: &["tesla"] },
    UnitManifest { key: "sam", line: "unit class=defense title=\"SAM Site\" cost=750 hp=400 armor=heavy speed=0 sight=30 sides=soviet producer=fact prereq=dome footprint=2x1 power=-20 weapon=nike build=1", weapons: &["nike"] },
    UnitManifest { key: "agun", line: "unit class=defense title=\"AA Gun\" cost=600 hp=400 armor=heavy speed=0 sight=30 sides=allies producer=fact prereq=dome footprint=1x1 power=-50 weapon=zsu build=1", weapons: &["zsu"] },
    UnitManifest { key: "sbag", line: "unit class=structure title=\"Sandbag Wall\" cost=50 hp=50 armor=concrete speed=0 sight=0 sides=allies,soviet producer=fact prereq=tent|barr footprint=1x1 wall=1", weapons: &[] },
    UnitManifest { key: "cycl", line: "unit class=structure title=\"Chain Link\" cost=75 hp=75 armor=concrete speed=0 sight=0 sides=allies,soviet producer=fact prereq=tent|barr footprint=1x1 wall=1", weapons: &[] },
    UnitManifest { key: "brik", line: "unit class=structure title=\"Concrete Wall\" cost=100 hp=100 armor=concrete speed=0 sight=0 sides=allies,soviet producer=fact prereq=dome footprint=1x1 wall=1", weapons: &[] },
    UnitManifest { key: "barb", line: "unit class=structure title=\"Barbed Wire\" cost=25 hp=40 armor=wood speed=0 sight=0 sides=allies,soviet producer=fact prereq=tent|barr footprint=1x1 wall=1", weapons: &[] },
    UnitManifest { key: "wood", line: "unit class=structure title=\"Wood Fence\" cost=25 hp=40 armor=wood speed=0 sight=0 sides=allies,soviet producer=fact footprint=1x1 wall=1", weapons: &[] },
    UnitManifest { key: "fenc", line: "unit class=structure title=\"Fence\" cost=25 hp=40 armor=wood speed=0 sight=0 sides=allies,soviet producer=fact footprint=1x1 wall=1", weapons: &[] },
];

fn weapon_line(id: &str) -> Option<&'static str> {
    Some(match id {
        "m1carbine" => "weapon id=m1carbine damage=15 rate=3.0 range=18 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/ra/piff fire=sfx/ra/gun5 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "m60" => "weapon id=m60 damage=15 rate=4.0 range=24 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/ra/piff fire=sfx/ra/gun11 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "chaingun" => "weapon id=chaingun damage=25 rate=5.0 range=24 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/ra/piffpiff fire=sfx/ra/gun13 versus=none:1,wood:.5,light:.4,heavy:.25,concrete:.1",
        "grenade" => "weapon id=grenade damage=50 rate=0.7 range=24 delivery=projectile projectile_speed=10 splash_radius=6 splash_damage=30 projectile_sprite=billboards/ra/bomblet impact=billboards/ra/frag1 fire=sfx/ra/grenade1 versus=none:1,wood:.75,light:.5,heavy:.3,concrete:.5",
        "redeye" => "weapon id=redeye damage=30 rate=0.8 range=27 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/ra/dragon impact=billboards/ra/veh-hit1 fire=sfx/ra/missile1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "flamer" => "weapon id=flamer damage=35 rate=1.5 range=12 delivery=projectile projectile_speed=12 splash_radius=4 splash_damage=20 impact=billboards/ra/fire1 fire=sfx/ra/flamer2 versus=none:1,wood:.9,light:.6,heavy:.3,concrete:.3",
        "colt45" => "weapon id=colt45 damage=125 rate=2.0 range=18 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/ra/piff fire=sfx/ra/gun5 versus=none:1,wood:.1,light:.05,heavy:.02,concrete:.02",
        "bite" => "weapon id=bite damage=25 rate=1.5 range=3 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 melee=1 versus=none:1,wood:0,light:0,heavy:0,concrete:0",
        "shock" => "weapon id=shock damage=60 rate=1.0 range=15 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 fire=sfx/ra/tesla1 versus=none:1,wood:.8,light:.6,heavy:.4,concrete:.5",
        "75mm" => "weapon id=75mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 projectile_sprite=billboards/ra/120mm impact=billboards/ra/veh-hit2 fire=sfx/ra/cannon1 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "90mm" => "weapon id=90mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 projectile_sprite=billboards/ra/120mm impact=billboards/ra/veh-hit2 fire=sfx/ra/cannon1 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "105mm" => "weapon id=105mm damage=30 rate=1.0 range=28 delivery=projectile projectile_speed=30 splash_radius=2 splash_damage=10 projectile_sprite=billboards/ra/120mm impact=billboards/ra/veh-hit2 fire=sfx/ra/cannon1 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "120mm" => "weapon id=120mm damage=40 rate=0.8 range=28 delivery=projectile projectile_speed=30 splash_radius=3 splash_damage=15 projectile_sprite=billboards/ra/120mm impact=billboards/ra/veh-hit3 fire=sfx/ra/cannon2 versus=none:.6,wood:.75,light:.7,heavy:.7,concrete:.5",
        "mammoth_tusk" => "weapon id=mammoth_tusk damage=40 rate=0.5 range=36 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/ra/dragon impact=billboards/ra/veh-hit1 fire=sfx/ra/missile1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "155mm" => "weapon id=155mm damage=60 rate=0.3 range=36 delivery=projectile projectile_speed=18 splash_radius=6 splash_damage=40 projectile_sprite=billboards/ra/120mm impact=billboards/ra/art-exp1 fire=sfx/ra/cannon2 versus=none:.9,wood:.9,light:.6,heavy:.4,concrete:.9",
        "227mm" => "weapon id=227mm damage=60 rate=0.4 range=45 delivery=projectile projectile_speed=20 splash_radius=6 splash_damage=40 projectile_sprite=billboards/ra/dragon impact=billboards/ra/art-exp1 fire=sfx/ra/missile1 versus=none:.9,wood:.9,light:.6,heavy:.4,concrete:.9",
        "scud" => "weapon id=scud damage=180 rate=0.15 range=60 delivery=projectile projectile_speed=14 splash_radius=12 splash_damage=80 projectile_sprite=billboards/ra/missile impact=billboards/ra/napalm1 fire=sfx/ra/missile1 versus=none:1,wood:1,light:.8,heavy:.7,concrete:1",
        "teslazap" => "weapon id=teslazap damage=100 rate=0.5 range=30 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 fire=sfx/ra/tesla1 versus=none:1,wood:1,light:.8,heavy:.6,concrete:.8",
        "tesla" => "weapon id=tesla damage=100 rate=0.5 range=45 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 fire=sfx/ra/tesla1 versus=none:1,wood:1,light:.8,heavy:.6,concrete:.8",
        "hellfire" => "weapon id=hellfire damage=30 rate=0.8 range=27 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/ra/dragon impact=billboards/ra/veh-hit1 fire=sfx/ra/missile1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "maverick" => "weapon id=maverick damage=30 rate=0.8 range=27 delivery=projectile projectile_speed=20 splash_radius=3 splash_damage=15 projectile_sprite=billboards/ra/dragon impact=billboards/ra/veh-hit1 fire=sfx/ra/missile1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5",
        "flamer_tower" => "weapon id=flamer_tower damage=35 rate=1.5 range=18 delivery=projectile projectile_speed=12 splash_radius=4 splash_damage=20 impact=billboards/ra/fire1 fire=sfx/ra/flamer2 versus=none:1,wood:.9,light:.6,heavy:.3,concrete:.3",
        "nike" => "weapon id=nike damage=50 rate=1.0 range=45 delivery=projectile projectile_speed=30 splash_radius=3 splash_damage=15 projectile_sprite=billboards/ra/missile impact=billboards/ra/veh-hit1 fire=sfx/ra/missile1 versus=none:.3,wood:.75,light:.75,heavy:.6,concrete:.5 anti_air=1",
        "zsu" => "weapon id=zsu damage=40 rate=3.0 range=45 delivery=hitscan projectile_speed=0 splash_radius=0 splash_damage=0 impact=billboards/ra/piffpiff fire=sfx/ra/aacanon3 versus=none:1,wood:.5,light:.6,heavy:.4,concrete:.2 anti_air=1",
        _ => return None,
    })
}

fn emit_sprites(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    palette: &Pal,
    audio: &BTreeSet<String>,
    report: &mut ConvertReport,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let remap = remap_line(palette, 80);
    let total = VEHICLE_KEYS.len()
        + AIRCRAFT_KEYS.len()
        + SHIP_KEYS.len()
        + INFANTRY_KEYS.len()
        + STRUCTURE_KEYS.len()
        + 37;
    let mut done = 0usize;
    for &key in VEHICLE_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = archives.shp(key) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        if shp.frames().len() < 32 {
            report.missing_shapes.insert(format!(
                "{}.SHP (needs 32 hull frames)",
                key.to_ascii_uppercase()
            ));
            continue;
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 32,
                frames: frame_range(&shp, 0, 32, palette, facing_rot),
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: 32,
                    looping: true,
                    fps: 8,
                }],
                unit: Some(UnitSpec {
                    manifest_lines: unit_lines(key, &remap, audio),
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ra", "unit", "vehicle"],
            },
        )?;
        if VEHICLE_TURRET_KEYS.contains(&key) {
            if shp.frames().len() >= 64 {
                emit_spec(
                    emitter,
                    report,
                    SpriteSpec {
                        key: format!("{key}-turret"),
                        role: "unit",
                        facings: 32,
                        frames: frame_range(&shp, 32, 64, palette, facing_rot),
                        states: vec![SpriteState {
                            name: "idle",
                            first: 0,
                            last: 32,
                            looping: true,
                            fps: 8,
                        }],
                        unit: None,
                        manifest_lines: vec![remap.clone()],
                        tags: vec!["ra", "unit", "turret"],
                    },
                )?;
            } else {
                report
                    .missing_shapes
                    .insert(format!("{} turret frames", key.to_ascii_uppercase()));
            }
        }
        emit_icon(emitter, archives, palette, key, report)?;
    }

    for &key in AIRCRAFT_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = archives.shp(key) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len().min(32);
        if count == 0 {
            continue;
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: count as u8,
                frames: frame_range(&shp, 0, count, palette, facing_rot),
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: count,
                    looping: true,
                    fps: 8,
                }],
                unit: Some(UnitSpec {
                    manifest_lines: unit_lines(key, &remap, audio),
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ra", "unit", "aircraft"],
            },
        )?;
        emit_icon(emitter, archives, palette, key, report)?;
    }

    for &key in SHIP_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = archives.shp(key) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len().min(32);
        if count == 0 {
            continue;
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: count as u8,
                frames: frame_range(&shp, 0, count, palette, facing_rot),
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: count,
                    looping: true,
                    fps: 8,
                }],
                unit: Some(UnitSpec {
                    manifest_lines: unit_lines(key, &remap, audio),
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ra", "unit", "boat"],
            },
        )?;
        emit_icon(emitter, archives, palette, key, report)?;
    }

    for &key in INFANTRY_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = archives.shp(key) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        if key == "e1" {
            report.e1_frames = shp.frames().len();
        }
        if shp.frames().len() < 120 {
            report.missing_shapes.insert(format!(
                "{}.SHP standard infantry ranges",
                key.to_ascii_uppercase()
            ));
            continue;
        }
        let death_start = if shp.frames().len() >= 304 {
            296
        } else {
            shp.frames().len().saturating_sub(8)
        };
        let mut frames = Vec::new();
        append_frames(&mut frames, &shp, 0, 8, palette, facing_rot);
        let idle_end = frames.len();
        append_frames(&mut frames, &shp, 8, 56, palette, |i| facing_rot(i / 6));
        let walk_end = frames.len();
        append_frames(&mut frames, &shp, 56, 120, palette, |i| facing_rot(i / 8));
        let fire_end = frames.len();
        append_frames(
            &mut frames,
            &shp,
            death_start,
            death_start + 8,
            palette,
            |_| 0,
        );
        let die_end = frames.len();
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "unit",
                facings: 8,
                frames,
                states: vec![
                    SpriteState {
                        name: "idle",
                        first: 0,
                        last: idle_end,
                        looping: true,
                        fps: 8,
                    },
                    SpriteState {
                        name: "walk",
                        first: idle_end,
                        last: walk_end,
                        looping: true,
                        fps: 10,
                    },
                    SpriteState {
                        name: "fire",
                        first: walk_end,
                        last: fire_end,
                        looping: false,
                        fps: 10,
                    },
                    SpriteState {
                        name: "die",
                        first: fire_end,
                        last: die_end,
                        looping: false,
                        fps: 8,
                    },
                ],
                unit: Some(UnitSpec {
                    manifest_lines: unit_lines(key, &remap, audio),
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ra", "unit", "infantry"],
            },
        )?;
        emit_icon(emitter, archives, palette, key, report)?;
    }

    for &key in STRUCTURE_KEYS {
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let Some(shp) = archives.shp(key) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let (fw, fh) = structure_footprint(key);
        if is_wall(key) {
            let count = shp.frames().len();
            if count == 0 {
                continue;
            }
            let mut lines = unit_lines(key, &remap, audio);
            lines.push(format!("footprint {fw} {fh}"));
            emit_spec(
                emitter,
                report,
                SpriteSpec {
                    key: key.into(),
                    role: "structure",
                    facings: 1,
                    frames: frame_range(&shp, 0, count, palette, |_| 0),
                    states: super::wall_states(count),
                    unit: Some(UnitSpec {
                        manifest_lines: lines,
                    }),
                    manifest_lines: Vec::new(),
                    tags: vec!["ra", "structure", "wall"],
                },
            )?;
            emit_icon(emitter, archives, palette, key, report)?;
            continue;
        }
        let base_count = shp.frames().len();
        if base_count == 0 {
            continue;
        }
        let half = base_count.div_ceil(2);
        let mut frames = frame_range(&shp, 0, base_count, palette, |_| 0);
        let build_first = frames.len();
        let make_stem = format!("{key}make");
        if let Some(make) = archives.shp(&make_stem) {
            append_frames(
                &mut frames,
                &make,
                0,
                make.frames().len(),
                palette,
                |_| 0,
            );
        } else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", make_stem.to_ascii_uppercase()));
        }
        let build_last = frames.len();
        let mut states = vec![SpriteState {
            name: "idle",
            first: 0,
            last: half,
            looping: true,
            fps: 6,
        }];
        if half < base_count {
            states.push(SpriteState {
                name: "damaged",
                first: half,
                last: base_count,
                looping: true,
                fps: 6,
            });
        }
        if build_last > build_first {
            states.push(SpriteState {
                name: "build",
                first: build_first,
                last: build_last,
                looping: false,
                fps: 15,
            });
        }
        report
            .structure_halves
            .push(format!("{key}: {half}+{}", base_count - half));
        let mut lines = unit_lines(key, &remap, audio);
        lines.push(format!("footprint {fw} {fh}"));
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "structure",
                facings: 1,
                frames,
                states,
                unit: Some(UnitSpec {
                    manifest_lines: lines,
                }),
                manifest_lines: Vec::new(),
                tags: vec!["ra", "structure"],
            },
        )?;
        if key == "gun" && base_count >= 32 {
            emit_spec(
                emitter,
                report,
                SpriteSpec {
                    key: "gun-turret".into(),
                    role: "unit",
                    facings: 32,
                    frames: frame_range(&shp, 0, 32, palette, facing_rot),
                    states: vec![SpriteState {
                        name: "idle",
                        first: 0,
                        last: 32,
                        looping: true,
                        fps: 8,
                    }],
                    unit: None,
                    manifest_lines: vec![remap.clone()],
                    tags: vec!["ra", "unit", "turret"],
                },
            )?;
        }
        emit_icon(emitter, archives, palette, key, report)?;
    }

    let mut temperate = TheaterBank::load(
        "TEMPERATE",
        archives,
        &TemplateTable::parse(TEMPLATE_TABLE_TEXT)?,
        report,
    )?;
    let snow = TheaterBank::load(
        "SNOW",
        archives,
        &TemplateTable::parse(TEMPLATE_TABLE_TEXT)?,
        report,
    )?;
    let interior = TheaterBank::load(
        "INTERIOR",
        archives,
        &TemplateTable::parse(TEMPLATE_TABLE_TEXT)?,
        report,
    )?;

    for number in 1..=37 {
        let key = format!("v{number:02}");
        done += 1;
        tick(on_tick, done, total, format!("sprite {key}"), None);
        let core = archives.shp(&key);
        let found = core
            .map(|shp| (shp, palette))
            .or_else(|| theater_shape(&temperate, &snow, &interior, &key));
        let Some((shp, sprite_palette)) = found else {
            report
                .missing_shapes
                .insert(format!("{} theater SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len();
        if count == 0 {
            continue;
        }
        let half = count.div_ceil(2);
        let mut states = vec![SpriteState {
            name: "idle",
            first: 0,
            last: half,
            looping: true,
            fps: 6,
        }];
        if half < count {
            states.push(SpriteState {
                name: "damaged",
                first: half,
                last: count,
                looping: true,
                fps: 6,
            });
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key,
                role: "structure",
                facings: 1,
                frames: frame_range(&shp, 0, count, sprite_palette, |_| 0),
                states,
                unit: None,
                manifest_lines: vec![remap.clone(), "footprint 1 1".into()],
                tags: vec!["ra", "structure", "civilian"],
            },
        )?;
    }

    emit_scenery(
        emitter,
        archives,
        &temperate,
        &snow,
        &interior,
        report,
        &remap,
    )?;
    emit_ore(emitter, &mut temperate, palette, report, &remap)?;

    for &key in EFFECT_KEYS {
        let Some(shp) = archives.shp(key) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", key.to_ascii_uppercase()));
            continue;
        };
        let count = shp.frames().len();
        if count == 0 {
            continue;
        }
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "effect",
                facings: 1,
                frames: frame_range(&shp, 0, count, palette, |_| 0),
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: count,
                    looping: false,
                    fps: 15,
                }],
                unit: None,
                manifest_lines: vec![remap.clone()],
                tags: vec!["ra", "effect"],
            },
        )?;
        report.effect_resolved += 1;
    }
    for &(key, source) in PROJECTILES {
        let Some(shp) = archives.shp(source) else {
            report
                .missing_shapes
                .insert(format!("{}.SHP", source.to_ascii_uppercase()));
            continue;
        };
        let count = if shp.frames().len() >= 32 { 32 } else { 1 };
        let facings = if count == 32 { 32 } else { 1 };
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: key.into(),
                role: "effect",
                facings,
                frames: frame_range(&shp, 0, count, palette, |i| {
                    if facings == 32 {
                        facing_rot(i)
                    } else {
                        0
                    }
                }),
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: count,
                    looping: false,
                    fps: 15,
                }],
                unit: None,
                manifest_lines: vec![remap.clone()],
                tags: vec!["ra", "effect", "projectile"],
            },
        )?;
        report.projectile_resolved += 1;
    }
    Ok(())
}

fn theater_shape<'a>(
    temperate: &'a TheaterBank<'_>,
    snow: &'a TheaterBank<'_>,
    interior: &'a TheaterBank<'_>,
    key: &str,
) -> Option<(Shp, &'a Pal)> {
    if let Some(shp) = temperate.shp(key) {
        Some((shp, &temperate.palette))
    } else if let Some(shp) = snow.shp(key) {
        Some((shp, &snow.palette))
    } else {
        interior.shp(key).map(|shp| (shp, &interior.palette))
    }
}

fn emit_scenery(
    emitter: &mut RtsEmitter<'_>,
    archives: &Archives,
    temperate: &TheaterBank<'_>,
    snow: &TheaterBank<'_>,
    interior: &TheaterBank<'_>,
    report: &mut ConvertReport,
    remap: &str,
) -> Result<(), String> {
    let mut keys = Vec::new();
    keys.extend((1..=17).map(|number| format!("t{number:02}")));
    keys.extend((1..=5).map(|number| format!("tc{number:02}")));
    keys.extend((1..=9).map(|number| format!("boxes{number:02}")));
    keys.extend((1..=5).map(|number| format!("ice{number:02}")));
    keys.push("mine".into());
    keys.extend((1..=7).map(|number| format!("rock{number}")));
    keys.extend(["wcrate".into(), "scrate".into()]);
    for key in keys {
        let core = archives.shp(&key);
        let found = core
            .map(|shp| (shp, &temperate.palette))
            .or_else(|| theater_shape(temperate, snow, interior, &key));
        let Some((shp, palette)) = found else {
            report
                .unresolved_scenery
                .insert(key.to_ascii_uppercase());
            continue;
        };
        let count = if matches!(key.as_str(), "wcrate" | "scrate") {
            1
        } else {
            shp.frames().len().min(2)
        };
        if count == 0 {
            continue;
        }
        report.resolved_scenery.insert(key.to_ascii_uppercase());
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key,
                role: "scenery",
                facings: 1,
                frames: frame_range(&shp, 0, count, palette, |_| 0),
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: count,
                    looping: false,
                    fps: 6,
                }],
                unit: None,
                manifest_lines: vec![remap.into()],
                tags: vec!["ra", "scenery"],
            },
        )?;
    }
    Ok(())
}

fn emit_ore(
    emitter: &mut RtsEmitter<'_>,
    temperate: &mut TheaterBank<'_>,
    palette: &Pal,
    report: &mut ConvertReport,
    remap: &str,
) -> Result<(), String> {
    let mut frames = Vec::new();
    for stem in [
        "gold01", "gold02", "gold03", "gold04", "gem01", "gem02", "gem03", "gem04",
    ] {
        let Some(shp) = temperate.shp(stem) else {
            report
                .missing_shapes
                .insert(format!("{}.TEM", stem.to_ascii_uppercase()));
            continue;
        };
        let Some(indexed) = shp.frames().first() else {
            continue;
        };
        frames.push(SpritePixels {
            rgba: indexed_transparent(indexed, palette),
            width: shp.width() as u32,
            height: shp.height() as u32,
            rot: 0,
        });
    }
    if frames.len() == 8 {
        emit_spec(
            emitter,
            report,
            SpriteSpec {
                key: "ore".into(),
                role: "resource",
                facings: 1,
                frames,
                states: vec![SpriteState {
                    name: "idle",
                    first: 0,
                    last: 8,
                    looping: false,
                    fps: 1,
                }],
                unit: Some(UnitSpec {
                    manifest_lines: vec!["unit class=resource title=\"Ore and Gems\"".into()],
                }),
                manifest_lines: vec![remap.into()],
                tags: vec!["ra", "resource"],
            },
        )?;
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
    let name = format!("{}ICON.SHP", key.to_ascii_uppercase());
    let Some(shp) = archives.core_entry(&name).and_then(|bytes| Shp::parse(bytes).ok()) else {
        report.missing_icons.insert(name);
        return Ok(());
    };
    let Some(frame) = shp.frames().first() else {
        return Ok(());
    };
    emitter.emit_texture(
        &format!("icons/ra/{key}"),
        &indexed_transparent(frame, palette),
        shp.width() as u32,
        shp.height() as u32,
        &["ra", "icon"],
    )
}

fn unit_lines(key: &str, remap: &str, audio: &BTreeSet<String>) -> Vec<String> {
    let mut lines = vec![remap.into()];
    let Some(unit) = UNIT_MANIFESTS.iter().find(|unit| unit.key == key) else {
        return lines;
    };
    lines.push(rewrite_unit_roles(key, unit.line, ROLE_TABLE));
    for weapon in unit.weapons {
        if let Some(line) = weapon_line(weapon) {
            lines.push(
                line.split_whitespace()
                    .filter(|token| {
                        token
                            .strip_prefix("fire=sfx/ra/")
                            .is_none_or(|stem| audio.contains(stem))
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }
    let mut slots = Vec::new();
    if INFANTRY_KEYS.contains(&key) {
        push_sound_slot(
            &mut slots,
            "select",
            &[
                "await1", "report1", "ready", "mrespon1", "swhat1", "einah1",
                "tuffguy1",
            ],
            &[],
            audio,
        );
        push_sound_slot(
            &mut slots,
            "move",
            &[
                "ackno",
                "affirm1",
                "yessir1",
                "ritaway",
                "roger",
                "ugotit",
                "eaffirm1",
                "eyessir1",
                "emovout1",
                "maffirm1",
                "myessir1",
                "mmovout1",
                "saffirm1",
                "syessir1",
                "smout1",
                "sokay1",
                "syeah1",
                "sindeed1",
                "guyokay1",
                "guyyeah1",
                "gotit1",
                "onit1",
                "yeah1",
                "yes1",
                "yo1",
            ],
            &[],
            audio,
        );
        push_sound_slot(&mut slots, "death", &[], &["nuyell", "dedman"], audio);
    } else if STRUCTURE_KEYS.contains(&key) {
        push_sound_slot(
            &mut slots,
            "death",
            &[
                "crumble",
                "crmble2",
                "xplobig4",
                "kaboom15",
                "kaboom22",
                "kaboom30",
            ],
            &[],
            audio,
        );
    } else {
        push_sound_slot(
            &mut slots,
            "select",
            if key == "harv" {
                &["vehic1", "mrespon1", "tank5"]
            } else {
                &[
                    "vehic1", "await1", "report1", "mrespon1", "rokroll1", "tank5",
                    "tuffguy1",
                ]
            },
            &[],
            audio,
        );
        push_sound_slot(
            &mut slots,
            "move",
            &[
                "ackno",
                "affirm1",
                "roger",
                "maffirm1",
                "myessir1",
                "mmovout1",
                "gotit1",
                "onit1",
                "keepem1",
            ],
            &[],
            audio,
        );
        push_sound_slot(
            &mut slots,
            "death",
            &["xplos", "kaboom15", "kaboom22", "kaboom30"],
            &[],
            audio,
        );
    }
    if let Some(stem) = unit
        .weapons
        .iter()
        .find_map(|weapon| weapon_sound(weapon, audio))
    {
        slots.push(format!("attack=sfx/ra/{stem}"));
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
        .map(|unit| roster_key("ra", unit.key))
        .collect()
}

#[cfg(test)]
pub(super) fn role_test_lines() -> Vec<String> {
    UNIT_MANIFESTS
        .iter()
        .map(|unit| rewrite_unit_roles(unit.key, unit.line, ROLE_TABLE))
        .collect()
}

fn push_sound_slot(
    slots: &mut Vec<String>,
    slot: &str,
    exact: &[&str],
    prefixes: &[&str],
    audio: &BTreeSet<String>,
) {
    let mut choices = exact
        .iter()
        .filter_map(|stem| audio.get(*stem))
        .cloned()
        .collect::<Vec<_>>();
    let prefixed = audio
        .iter()
        .filter(|stem| prefixes.iter().any(|prefix| stem.starts_with(prefix)))
        .filter(|stem| !choices.contains(stem))
        .cloned()
        .collect::<Vec<_>>();
    choices.extend(prefixed);
    if !choices.is_empty() {
        slots.push(format!(
            "{slot}={}",
            choices
                .iter()
                .map(|stem| format!("sfx/ra/{stem}"))
                .collect::<Vec<_>>()
                .join("|")
        ));
    }
}

fn first_audio<'a>(
    audio: &'a BTreeSet<String>,
    exact: &[&str],
    prefixes: &[&str],
) -> Option<&'a str> {
    exact
        .iter()
        .find_map(|stem| audio.get(*stem).map(String::as_str))
        .or_else(|| {
            audio
                .iter()
                .find(|stem| prefixes.iter().any(|prefix| stem.starts_with(prefix)))
                .map(String::as_str)
        })
}

fn weapon_sound<'a>(weapon: &str, audio: &'a BTreeSet<String>) -> Option<&'a str> {
    match weapon {
        "m1carbine" | "colt45" => first_audio(audio, &["gun5"], &["gun", "mgun"]),
        "m60" | "chaingun" | "zsu" => {
            first_audio(audio, &["mgun11", "mgun2", "gun11", "gun13", "aacanon3"], &["mgun", "gun"])
        }
        "grenade" => first_audio(audio, &["grenade1"], &["gun"]),
        "redeye" | "mammoth_tusk" | "227mm" | "scud" | "hellfire" | "maverick"
        | "nike" => first_audio(audio, &[], &["rocket"])
            .or_else(|| first_audio(audio, &["missile1"], &["missile"])),
        "flamer" | "flamer_tower" => {
            first_audio(audio, &["flamer2", "firebl3"], &["flamer"])
        }
        "shock" | "teslazap" | "tesla" => {
            first_audio(audio, &["tesla1"], &["tesla", "shock"])
        }
        "75mm" | "90mm" | "105mm" | "120mm" | "155mm" => {
            first_audio(audio, &[], &["tnkfire"])
                .or_else(|| first_audio(audio, &["cannon1", "cannon2"], &["gun"]))
        }
        _ => None,
    }
}

fn resolve_audio(
    archives: &Archives,
    names: &NameTable,
    report: &mut ConvertReport,
) -> Result<BTreeMap<String, AudioAsset>, String> {
    const EXACT: &[&str] = &[
        "await1", "ackno", "affirm1", "ready", "report1", "yessir1", "ritaway",
        "roger", "ugotit", "vehic1", "xplos", "xplobig4", "crumble", "flamer2",
        "tesla1",
    ];
    const PREFIXES: &[&str] = &[
        "nuyell", "tnkfire", "rocket", "gun", "mgun", "flamer", "tesla", "shock",
    ];
    let sounds = MixFile::parse(&archives.sounds).map_err(|e| format!("sounds.mix: {e}"))?;
    let allies = MixFile::parse(&archives.allies).map_err(|e| format!("allies.mix: {e}"))?;
    let russian = MixFile::parse(&archives.russian).map_err(|e| format!("russian.mix: {e}"))?;
    let mut raw = BTreeMap::<String, (&[u8], bool)>::new();
    for (label, mix, speech) in [
        ("sounds.mix", &sounds, false),
        ("allies.mix", &allies, true),
        ("russian.mix", &russian, true),
    ] {
        let resolved = names
            .resolve_names(mix)
            .into_iter()
            .filter_map(|(id, name)| name.map(|name| (id, name)))
            .filter(|(_, name)| name.to_ascii_uppercase().ends_with(".AUD"))
            .collect::<Vec<_>>();
        if speech {
            report.side_speech_entries.push(format!(
                "{label}: {} entries, {} dictionary-resolved AUD names",
                mix.entries().len(),
                resolved.len()
            ));
        }
        for (id, name) in resolved {
            let Some(bytes) = mix.by_id(id) else { continue };
            let stem = name[..name.len().saturating_sub(4)].to_ascii_lowercase();
            raw.entry(stem).or_insert((bytes, speech));
        }
        // Probe the whole bounded built-in dictionary as well. This catches
        // a hash collision whose canonical dictionary spelling differs from
        // the archive's actual AUD name without guessing arbitrary ids.
        for name in names
            .names()
            .filter(|name| name.to_ascii_uppercase().ends_with(".AUD"))
        {
            let Some(bytes) = mix.by_name(name) else { continue };
            let stem = name[..name.len().saturating_sub(4)].to_ascii_lowercase();
            raw.entry(stem).or_insert((bytes, speech));
        }
    }

    let mut audio = BTreeMap::new();
    for (stem, (bytes, speech)) in raw {
        match Aud::parse(bytes) {
            Ok(aud) => {
                audio.insert(stem, AudioAsset { aud, speech });
            }
            Err(_) => {
                report
                    .missing_audio
                    .insert(format!("{}.AUD (decode)", stem.to_ascii_uppercase()));
            }
        }
    }
    for stem in EXACT {
        if !audio.contains_key(*stem) {
            report
                .missing_audio
                .insert(format!("{}.AUD", stem.to_ascii_uppercase()));
        }
    }
    for prefix in PREFIXES {
        if !audio.keys().any(|stem| stem.starts_with(prefix)) {
            report
                .missing_audio
                .insert(format!("{}*.AUD", prefix.to_ascii_uppercase()));
        }
    }
    Ok(audio)
}

fn emit_audio(
    emitter: &mut RtsEmitter<'_>,
    audio: &BTreeMap<String, AudioAsset>,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<(), String> {
    let total = audio.len();
    for (index, (stem, source)) in audio.iter().enumerate() {
        tick(on_tick, index, total, format!("sfx {stem}"), None);
        emitter.emit_sfx(
            &format!("sfx/ra/{stem}"),
            source.aud.sample_rate(),
            source.aud.channels(),
            source.aud.samples(),
            if source.speech {
                &["ra", "speech"]
            } else {
                &["ra", "sfx"]
            },
        )?;
    }
    Ok(())
}

fn emit_spec(
    emitter: &mut RtsEmitter<'_>,
    report: &mut ConvertReport,
    spec: SpriteSpec,
) -> Result<(), String> {
    *report.roles.entry(spec.role.into()).or_default() += 1;
    emitter.emit_sprite(spec)
}

fn frame_range(
    shp: &Shp,
    first: usize,
    last: usize,
    palette: &Pal,
    rot: impl Fn(usize) -> u8,
) -> Vec<SpritePixels> {
    let mut frames = Vec::new();
    append_frames(&mut frames, shp, first, last, palette, rot);
    frames
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

fn verify_remap_ramp(archives: &Archives, palette: &Pal) -> String {
    let counts = ["1TNK", "2TNK", "E1", "HARV"]
        .into_iter()
        .map(|stem| {
            let count = archives
                .shp(stem)
                .map(|shp| {
                    shp.frames()
                        .iter()
                        .flatten()
                        .filter(|&&index| (80..=95).contains(&index))
                        .count()
                })
                .unwrap_or(0);
            format!("{stem}={count}")
        })
        .collect::<Vec<_>>();
    let ramp = (80..=95).map(|index| palette.rgb(index)).collect::<Vec<_>>();
    let single_hue = ramp.windows(2).all(|pair| {
        let [ar, ag, ab] = pair[0];
        let [br, bg, bb] = pair[1];
        ar >= br && ag >= bg && ab >= bb
    });
    format!(
        "indices 80..95; occurrences {}; gold single-hue luminance ramp {}",
        counts.join(", "),
        if single_hue { "confirmed" } else { "not confirmed" }
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
            let Some(&index) = src.get((y * src_w + x) as usize) else {
                continue;
            };
            if index == 0 {
                continue;
            }
            let at = (((oy + y) * dst_w + ox + x) * 4) as usize;
            let Some(pixel) = dst.get_mut(at..at + 4) else {
                continue;
            };
            if index == 4 {
                for channel in &mut pixel[..3] {
                    *channel = (*channel as u16 / 2) as u8;
                }
            } else {
                let [r, g, b] = palette.rgb(index);
                pixel.copy_from_slice(&[r, g, b, 255]);
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
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
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

fn mix_entry<'a>(archive: &'a [u8], name: &str) -> Option<&'a [u8]> {
    MixFile::parse(archive).ok()?.by_name(name)
}

fn grid_class(class: char) -> char {
    match class {
        'r' | 'B' => 'r',
        'g' | 'b' => 'b',
        'w' | 'v' => 'w',
        'k' | 't' | 'W' => '#',
        'T' | 'G' => 't',
        _ => '.',
    }
}

fn local_cell(bounds: MapBounds, cell: u16) -> Option<(usize, usize)> {
    let x = cell % 128;
    let y = cell / 128;
    if x < bounds.x
        || y < bounds.y
        || x >= bounds.x.checked_add(bounds.width)?
        || y >= bounds.y.checked_add(bounds.height)?
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

fn ra_cell_to_metres(bounds_x: u16, bounds_y: u16, cell: u16) -> (f32, f32) {
    let x = (cell % 128) as f32;
    let y = (cell / 128) as f32;
    (
        (x - bounds_x as f32 + 0.5) * CELL_M,
        (y - bounds_y as f32 + 0.5) * CELL_M,
    )
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
    } else if SHIP_KEYS.contains(&key) {
        "boat"
    } else {
        "vehicle"
    }
}

fn structure_footprint(key: &str) -> (usize, usize) {
    match key {
        "fact" | "weap" | "afld" => (3, 2),
        "apwr" | "proc" | "fix" | "syrd" | "spen" => (3, 3),
        "powr" | "tent" | "barr" | "dome" | "hpad" | "atek" | "iron" | "pdox" => {
            (2, 2)
        }
        "stek" => (2, 3),
        "mslo" | "sam" => (2, 1),
        _ => (1, 1),
    }
}

fn is_defense(key: &str) -> bool {
    matches!(key, "pbox" | "hbox" | "gun" | "ftur" | "tsla" | "sam" | "agun")
}

fn is_wall(key: &str) -> bool {
    matches!(key, "sbag" | "cycl" | "brik" | "barb" | "wood" | "fenc")
}

fn is_smudge(key: &str) -> bool {
    key.get(0..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cr") || prefix.eq_ignore_ascii_case("sc"))
        && key
            .get(2..)
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| (1..=6).contains(&number))
}

#[derive(Clone, Copy)]
struct HouseInfo {
    side: &'static str,
    color: &'static str,
}

fn canonical_owner(owner: &str) -> String {
    match owner.trim().to_ascii_lowercase().as_str() {
        "spain" => "Spain".into(),
        "greece" | "goodguy" => "Greece".into(),
        "england" => "England".into(),
        "germany" => "Germany".into(),
        "france" => "France".into(),
        "turkey" => "Turkey".into(),
        "ussr" | "badguy" => "USSR".into(),
        "ukraine" => "Ukraine".into(),
        "neutral" => "Neutral".into(),
        "special" => "Special".into(),
        lower if lower.len() == 6
            && lower.starts_with("multi")
            && lower.as_bytes()[5].is_ascii_digit() =>
        {
            format!("Multi{}", &lower[5..])
        }
        _ => owner.trim().replace(' ', "_"),
    }
}

fn house_info(house: &str) -> HouseInfo {
    match house {
        "Spain" => HouseInfo { side: "allies", color: "e8c040" },
        "Greece" => HouseInfo { side: "allies", color: "2050d0" },
        "England" => HouseInfo { side: "allies", color: "20a040" },
        "Germany" => HouseInfo { side: "allies", color: "a06030" },
        "France" => HouseInfo { side: "allies", color: "20a0a0" },
        "Turkey" => HouseInfo { side: "allies", color: "e07020" },
        "USSR" => HouseInfo { side: "soviet", color: "d02020" },
        "Ukraine" => HouseInfo { side: "soviet", color: "c060c0" },
        "Neutral" => HouseInfo { side: "neutral", color: "b0b0b0" },
        "Special" => HouseInfo { side: "neutral", color: "9aa0c8" },
        "Multi1" => HouseInfo { side: "multi", color: "e8c040" },
        "Multi2" => HouseInfo { side: "multi", color: "2050d0" },
        "Multi3" => HouseInfo { side: "multi", color: "d02020" },
        "Multi4" => HouseInfo { side: "multi", color: "20a040" },
        "Multi5" => HouseInfo { side: "multi", color: "e07020" },
        "Multi6" => HouseInfo { side: "multi", color: "c060c0" },
        "Multi7" => HouseInfo { side: "multi", color: "20a0a0" },
        "Multi8" => HouseInfo { side: "multi", color: "a06030" },
        _ => HouseInfo { side: "neutral", color: "9aa0c8" },
    }
}

fn house_order(house: &str) -> (u8, String) {
    let rank = match house {
        "Greece" => 0,
        "USSR" => 1,
        _ => 2,
    };
    (rank, house.to_owned())
}

fn color_rgb(hex: &str) -> [u8; 3] {
    let value = u32::from_str_radix(hex, 16).unwrap_or(0x9aa0c8);
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

fn waypoint_starts(map: &RaMap) -> Vec<(String, f32, f32)> {
    let mut starts = map
        .waypoints
        .iter()
        .filter(|waypoint| waypoint.number <= 7 && (0..16_384).contains(&waypoint.cell))
        .filter_map(|waypoint| {
            let cell = waypoint.cell as u16;
            local_cell(map.bounds, cell).map(|_| {
                let (x, z) = ra_cell_to_metres(map.bounds.x, map.bounds.y, cell);
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

fn write_report(report: &ConvertReport) -> Result<(), String> {
    let mut text = String::from(
        "# Red Alert staged conversion report\n\n## Three-line summary\n\n",
    );
    text.push_str(&format!(
        "Converted {} packed multiplayer maps through the shared RTS world emitter.\n",
        report.maps.len()
    ));
    text.push_str(&format!(
        "Emitted {} billboard assets across {} roles; remap {}.\n",
        report.roles.values().sum::<usize>(),
        report.roles.len(),
        report.remap_note
    ));
    text.push_str(&format!(
        "Resolved {} scenery names, {} effects, and {} projectile sprites; unresolved references are itemized below.\n",
        report.resolved_scenery.len(), report.effect_resolved, report.projectile_resolved
    ));
    text.push_str(
        "\n## Worlds\n\n| Map | Title | Theater | Bounds | Units | Structures | Scenery | Resources | Starts |\n|---|---|---|---|---:|---:|---:|---:|---:|\n",
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

    text.push_str("\n## Overlay mapping\n\n");
    text.push_str("The packed byte mapping established against the resolved theater/core names is: `0 SBAG`, `1 CYCL`, `2 BRIK`, `3 BARB`, `4 WOOD`; `5..8 GOLD01..04`; `9..12 GEM01..04`; `13..19 V12..18` baked decals; `20 FPLS` baked flag pole; `21 WCRATE`; `22 SCRATE`; `23 FENC`; and `24 WWCRATE`, exposed through the WCRATE scenery asset. GOLD/GEM become ore stages 0..7.\n\n");
    text.push_str("Observed packed ids across the 24 maps:\n\n");
    for (id, count) in &report.overlay_counts {
        text.push_str(&format!("- `{id}`: {count}\n"));
    }

    text.push_str("\n## Sprite assets\n\n| Role | Count |\n|---|---:|\n");
    for (role, count) in &report.roles {
        text.push_str(&format!("| {role} | {count} |\n"));
    }
    text.push_str(&format!(
        "\n- Remap ramp: {}.\n",
        report.remap_note
    ));
    text.push_str("- Vehicle rotation: 2TNK frame 0 faces north and frame 8 faces west; the intervening frames advance counter-clockwise, so source frame `k` is emitted as `rot 1+k`.\n");
    text.push_str(&format!(
        "- Infantry: E1.SHP has {} frames. Emitted source ranges are idle 0..7, walk 8..55 (8 facings × 6), fire 56..119 (8 × 8), and the first omni death block 296..303. The manifest repacks these as idle 0..8, walk 8..56, fire 56..120, die 120..128 (exclusive ends).\n",
        report.e1_frames
    ));
    text.push_str("- Structures: healthy/damaged split at `ceil(frame_count/2)` and `<key>MAKE.SHP` frames append as build. Observed splits: ");
    text.push_str(&report.structure_halves.join(", "));
    text.push_str(".\n");
    text.push_str(&format!(
        "- Resolution counts: scenery {}/{}, effects {}/{}, projectiles {}/{}.\n",
        report.resolved_scenery.len(),
        report.resolved_scenery.len() + report.unresolved_scenery.len(),
        report.effect_resolved,
        EFFECT_KEYS.len(),
        report.projectile_resolved,
        PROJECTILES.len(),
    ));

    text.push_str("\n## Palette loading\n\n");
    if report.palette_fallbacks.is_empty() {
        text.push_str("TEMPERAT.PAL, SNOW.PAL, and INTERIOR.PAL all resolved from the pack's nested palette archive; no fallback was needed.\n");
    } else {
        for fallback in &report.palette_fallbacks {
            text.push_str(&format!("- {fallback}\n"));
        }
    }

    text.push_str("\n## Speech and SFX\n\n");
    text.push_str(&format!(
        "Decoded and emitted {} uniquely named AUD assets from sounds.mix and any resolved side speech archives.\n\n",
        report.audio_stems.len()
    ));
    for line in &report.side_speech_entries {
        text.push_str(&format!("- {line}\n"));
    }
    text.push_str("Every dictionary-resolved AUD is emitted; the bounded built-in AUD dictionary is also probed directly by rotate-add id for side speech. Unit sound slots reference only emitted stems.\n");
    let classic_exact = [
        "await1", "ackno", "affirm1", "ready", "report1", "yessir1", "ritaway",
        "roger", "ugotit", "vehic1",
    ];
    let absent_exact = classic_exact
        .iter()
        .filter(|stem| !report.audio_stems.contains(**stem))
        .copied()
        .collect::<Vec<_>>();
    text.push_str(&format!(
        "Pack verification: absent classic voice stems: {}.\n",
        if absent_exact.is_empty() {
            "none".to_string()
        } else {
            absent_exact.join(", ")
        }
    ));
    let families = ["nuyell", "tnkfire", "gun", "mgun", "rocket", "flamer", "tesla"];
    text.push_str("Pack weapon/death family counts:");
    for family in families {
        let count = report
            .audio_stems
            .iter()
            .filter(|stem| stem.starts_with(family))
            .count();
        text.push_str(&format!(" {family}*={count}"));
    }
    text.push_str(".\n");

    text.push_str("\n## Unresolved pack references\n\n### Map scenery\n\n");
    write_set(&mut text, &report.unresolved_scenery);
    text.push_str("\n### Shapes\n\n");
    write_set(&mut text, &report.missing_shapes);
    text.push_str("\n### Icons\n\n");
    write_set(&mut text, &report.missing_icons);
    text.push_str("\n### Audio\n\n");
    write_set(&mut text, &report.missing_audio);

    let report_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../cnc-ra-convert-report.md");
    std::fs::write(&report_path, text)
        .map_err(|e| format!("write cnc-ra-convert-report.md: {e}"))
}

fn write_set(text: &mut String, values: &BTreeSet<String>) {
    if values.is_empty() {
        text.push_str("None.\n");
    } else {
        for value in values {
            text.push_str(&format!("- `{value}`\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_ra_overlay_ids_map_to_contract_rows() {
        assert_eq!(overlay_kind(0), OverlayKind::Wall("sbag"));
        assert_eq!(overlay_kind(4), OverlayKind::Wall("wood"));
        assert_eq!(overlay_kind(5), OverlayKind::Resource(0));
        assert_eq!(overlay_kind(8), OverlayKind::Resource(3));
        assert_eq!(overlay_kind(9), OverlayKind::Resource(4));
        assert_eq!(overlay_kind(12), OverlayKind::Resource(7));
        assert_eq!(overlay_kind(13), OverlayKind::Bake("v12"));
        assert_eq!(overlay_kind(20), OverlayKind::Bake("fpls"));
        assert_eq!(overlay_kind(21), OverlayKind::Scenery("wcrate"));
        assert_eq!(overlay_kind(22), OverlayKind::Scenery("scrate"));
        assert_eq!(overlay_kind(23), OverlayKind::Wall("fenc"));
        assert_eq!(overlay_kind(24), OverlayKind::Scenery("wcrate"));
        assert_eq!(overlay_kind(25), OverlayKind::Unknown);
    }

    #[test]
    fn cnc_import_ra_houses_map_to_side_and_colour() {
        let cases = [
            ("Spain", "Spain", "allies", "e8c040"),
            ("GoodGuy", "Greece", "allies", "2050d0"),
            ("England", "England", "allies", "20a040"),
            ("Germany", "Germany", "allies", "a06030"),
            ("France", "France", "allies", "20a0a0"),
            ("Turkey", "Turkey", "allies", "e07020"),
            ("BadGuy", "USSR", "soviet", "d02020"),
            ("Ukraine", "Ukraine", "soviet", "c060c0"),
            ("Neutral", "Neutral", "neutral", "b0b0b0"),
            ("Special", "Special", "neutral", "9aa0c8"),
            ("multi8", "Multi8", "multi", "a06030"),
        ];
        for (source, canonical, side, color) in cases {
            assert_eq!(canonical_owner(source), canonical);
            let info = house_info(canonical);
            assert_eq!((info.side, info.color), (side, color));
        }
    }

    #[test]
    fn cnc_import_ra_template_lookup_is_theater_specific() {
        let table = TemplateTable::parse(TEMPLATE_TABLE_TEXT).unwrap();
        let temperate = table.get("temperat", 1).unwrap();
        assert_eq!(
            (temperate.stem.as_str(), temperate.width, temperate.height),
            ("w1", 1, 1)
        );
        assert_eq!(temperate.classes, "w");
        assert_eq!(table.get("interior", 329).unwrap().stem, "wall0001");
        assert!(table.get("interior", 1).is_none());
    }

    #[test]
    fn cnc_import_ra_cell_metres_use_128_cell_rows() {
        assert_eq!(ra_cell_to_metres(10, 20, 20 * 128 + 10), (3.0, 3.0));
        assert_eq!(ra_cell_to_metres(10, 20, 21 * 128 + 12), (15.0, 9.0));
    }

    #[test]
    fn cnc_import_ra_rotation_is_counter_clockwise_from_north() {
        assert_eq!(facing_rot(0), 1);
        assert_eq!(facing_rot(8), 9);
        assert_eq!(facing_rot(31), 32);
    }

    #[test]
    fn cnc_import_ra_sound_slots_only_choose_emitted_stems() {
        let audio = [
            "await1", "ackno", "nuyell1", "vehic1", "xplos", "tnkfire6",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        let infantry = unit_lines("e1", "remap", &audio);
        let sound = infantry
            .iter()
            .find(|line| line.starts_with("sound "))
            .unwrap();
        assert!(sound.contains("select=sfx/ra/await1"));
        assert!(sound.contains("move=sfx/ra/ackno"));
        assert!(sound.contains("death=sfx/ra/nuyell1"));
        assert!(!infantry.iter().any(|line| line.contains("sfx/ra/gun5")));

        let tank = unit_lines("4tnk", "remap", &audio);
        assert!(tank
            .iter()
            .any(|line| line.contains("attack=sfx/ra/tnkfire6")));
        assert!(!tank.iter().any(|line| line.contains("sfx/ra/cannon2")));

        let pack_audio = [
            "mrespon1", "maffirm1", "dedman1", "tank5", "gotit1", "kaboom15",
            "gun5", "cannon1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        let infantry = unit_lines("e1", "remap", &pack_audio).join("\n");
        assert!(infantry.contains("select=sfx/ra/mrespon1"));
        assert!(infantry.contains("move=sfx/ra/maffirm1"));
        assert!(infantry.contains("death=sfx/ra/dedman1"));
        assert!(infantry.contains("attack=sfx/ra/gun5"));
        let tank = unit_lines("4tnk", "remap", &pack_audio).join("\n");
        assert!(tank.contains("sfx/ra/tank5"));
        assert!(tank.contains("sfx/ra/gotit1"));
        assert!(tank.contains("death=sfx/ra/kaboom15"));
        assert!(tank.contains("attack=sfx/ra/cannon1"));
    }

    #[test]
    #[ignore]
    fn convert_local_ra_pack_if_present() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let pack = manifest.join("../../../local/packs/ra");
        if !pack.join("conquer.mix").is_file() {
            return;
        }
        let staged = std::env::temp_dir().join(format!(
            "makepad-ra-convert-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&staged);
        let assets = convert(&pack, &staged, &mut |_| {}).expect("convert local RA pack");
        let worlds = assets
            .iter()
            .filter(|asset| asset.kind == makepad_asset_data::AssetKind::World)
            .collect::<Vec<_>>();
        assert_eq!(worlds.len(), 24, "worlds={}", worlds.len());
        for world in worlds {
            let glb = staged.join(&world.rel_path);
            for extension in ["glb", "place", "grid", "png"] {
                assert!(
                    glb.with_extension(extension).is_file(),
                    "{}",
                    glb.with_extension(extension).display()
                );
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
            assert_eq!(
                grid.lines().filter(|line| line.starts_with("row ")).count(),
                height
            );
        }
        let manifest = |key: &str| std::fs::read_to_string(staged.join(key)).unwrap();
        assert!(staged.join("billboards/ra/2tnk.billboard").is_file());
        assert!(staged.join("billboards/ra/2tnk-turret.billboard").is_file());
        let e1 = manifest("billboards/ra/e1.billboard");
        for state in ["idle", "walk", "fire", "die"] {
            assert!(e1.contains(&format!("state {state} ")));
        }
        let fact = manifest("billboards/ra/fact.billboard");
        assert!(fact.contains("footprint 3 2"));
        for state in ["idle", "damaged", "build"] {
            assert!(fact.contains(&format!("state {state} ")));
        }
        let ore = manifest("billboards/ra/ore.billboard");
        assert_eq!(
            ore.lines().filter(|line| line.starts_with("frame ")).count(),
            8
        );
        let wav_count = std::fs::read_dir(staged.join("sfx/ra"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wav"))
            .count();
        assert_eq!(wav_count, 107);
        assert!(e1.contains("sound select=sfx/ra/"));
        assert!(e1.contains(" move=sfx/ra/"));
        assert!(e1.contains(" death=sfx/ra/"));
        for asset in &assets {
            if asset.kind == makepad_asset_data::AssetKind::Billboard {
                let text = std::fs::read_to_string(staged.join(&asset.rel_path)).unwrap();
                assert!(!text.contains("producer="), "{}", asset.rel_path);
            }
        }
        let _ = std::fs::remove_dir_all(&staged);
    }
}
