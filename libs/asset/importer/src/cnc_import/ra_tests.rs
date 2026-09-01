use super::{
    ini::Ini,
    map::RaMap,
    mix::{HashKind, MixFile, MixHeaderKind, NameTable},
    pal::Pal,
    rules::Rules,
    shp::{Shp, Sprite},
    shp_ts::ShpTs,
    tmp::Tmp,
    tmp_ts::TmpTs,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn local_packs() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..3 {
        root.pop();
    }
    root.join("local/packs")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn find_in_mix<'a>(
    mix: &MixFile<'a>,
    target: &str,
    names: &NameTable,
    depth: usize,
) -> Option<(String, &'a [u8])> {
    if let Some(bytes) = mix.by_name_with_hash(target, HashKind::Crc32) {
        return Some((target.to_owned(), bytes));
    }
    if depth == 0 {
        return None;
    }
    for entry in mix.entries() {
        let bytes = mix.by_id(entry.id)?;
        let Ok(inner) = MixFile::parse(bytes) else {
            continue;
        };
        let Some((path, found)) = find_in_mix(&inner, target, names, depth - 1) else {
            continue;
        };
        let container = names
            .name_of(entry.id)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{:08x}.mix", entry.id));
        return Some((format!("{container}/{path}"), found));
    }
    None
}

#[test]
#[ignore]
fn decode_local_ra_pack_if_present() {
    let pack = local_packs().join("ra");
    if !pack.join("conquer.mix").is_file() {
        return;
    }
    let names = NameTable::with_hash_kind(HashKind::RotateAdd);
    let mut mix_counts = BTreeMap::new();
    for archive in [
        "allies.mix",
        "conquer.mix",
        "general.mix",
        "interior.mix",
        "redalert.mix",
        "russian.mix",
        "scores.mix",
        "snow.mix",
        "sounds.mix",
        "temperat.mix",
    ] {
        let bytes = read(&pack.join(archive));
        let mix = MixFile::parse(&bytes).unwrap_or_else(|error| panic!("{archive}: {error}"));
        let resolved = names
            .names()
            .filter(|name| mix.by_name(name).is_some())
            .count();
        println!(
            "RA_MIX {archive} kind={:?} entries={} resolved={resolved}",
            mix.header_kind(),
            mix.entries().len()
        );
        mix_counts.insert(archive, (mix.entries().len(), resolved));
    }
    assert_eq!(
        MixFile::parse(&read(&pack.join("conquer.mix")))
            .unwrap()
            .header_kind(),
        MixHeaderKind::EncryptedChecksum
    );
    assert!(mix_counts["conquer.mix"].1 > 150);

    let mut index2_histogram = BTreeMap::<u8, usize>::new();
    let mut template_counts = BTreeMap::new();
    for (archive, extension) in [
        ("temperat.mix", ".TEM"),
        ("snow.mix", ".SNO"),
        ("interior.mix", ".INT"),
    ] {
        let bytes = read(&pack.join(archive));
        let mix = MixFile::parse(&bytes).unwrap();
        let mut count = 0usize;
        let mut animated = 0usize;
        for (id, name) in names.resolve_names(&mix) {
            let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(extension)) else {
                continue;
            };
            let data = mix.by_id(id).unwrap();
            let tmp = match Tmp::parse(data) {
                Ok(tmp) => tmp,
                Err(error) if Shp::parse(data).is_ok() => {
                    let _ = error;
                    animated += 1;
                    continue;
                }
                Err(error) => panic!(
                    "{archive}:{name}: {error}; header={:02x?}",
                    &data[..data.len().min(40)]
                ),
            };
            assert!(tmp.blocks().is_some(), "{archive}:{name} parsed as TD TMP");
            for &class in tmp.index2() {
                *index2_histogram.entry(class).or_default() += 1;
            }
            count += 1;
        }
        println!("RA_TMP {archive} decoded={count} animated_shp={animated}");
        template_counts.insert(archive, count);
    }
    assert!(template_counts.values().all(|&count| count != 0));
    println!("RA_TMP_INDEX2 {index2_histogram:?}");

    let general_bytes = read(&pack.join("general.mix"));
    let general = MixFile::parse(&general_bytes).unwrap();
    let mut maps = BTreeSet::new();
    let mut map_template_ids = BTreeSet::new();
    for name in names
        .names()
        .filter(|name| name.to_ascii_uppercase().ends_with(".INI"))
    {
        let Some(bytes) = general.by_name(name) else {
            continue;
        };
        let text = String::from_utf8_lossy(bytes);
        let ini = Ini::parse(&text);
        if ini.section("MapPack").is_none() {
            continue;
        }
        let map = RaMap::parse(&ini).unwrap_or_else(|error| panic!("{name}: {error}"));
        for y in 0..128 {
            for x in 0..128 {
                let template = map.cell(x, y).0;
                assert!(
                    template == 0xffff || template <= 600,
                    "{name}: ({x},{y}) template {template}; first={:?}",
                    (0..8).map(|sample_x| map.cell(sample_x, 0)).collect::<Vec<_>>()
                );
                if template != 0xffff {
                    map_template_ids.insert(template);
                }
            }
        }
        println!(
            "RA_MAP {name} theater={} bounds={},{},{},{} units={} structures={} infantry={} ships={}",
            map.theater,
            map.bounds.x,
            map.bounds.y,
            map.bounds.width,
            map.bounds.height,
            map.units.len(),
            map.structures.len(),
            map.infantry.len(),
            map.ships.len()
        );
        maps.insert(name.to_owned());
    }
    println!("RA_MAP_TOTAL decoded={} names={maps:?}", maps.len());
    println!("RA_MAP_TEMPLATE_IDS {map_template_ids:?}");
    assert!(!maps.is_empty());

    let redalert_bytes = read(&pack.join("redalert.mix"));
    let redalert = MixFile::parse(&redalert_bytes).unwrap();
    let mut rules_location = None;
    let mut rules = None;
    if let Some(bytes) = redalert.by_name("RULES.INI") {
        rules_location = Some("redalert.mix/RULES.INI".to_owned());
        rules = Some(Rules::parse(&Ini::parse(&String::from_utf8_lossy(bytes))).unwrap());
    }
    let mut containers = vec!["LOCAL.MIX", "HIRES.MIX", "LORES.MIX", "SPEECH.MIX"];
    containers.extend(
        names
            .names()
            .filter(|name| name.to_ascii_uppercase().ends_with(".MIX")),
    );
    for container in containers {
        let Some(inner) = redalert
            .mix_by_name(container)
            .unwrap_or_else(|error| panic!("redalert.mix/{container}: {error}"))
        else {
            continue;
        };
        if let Some(bytes) = inner.by_name("RULES.INI") {
            let location = format!("redalert.mix/{container}/RULES.INI");
            let parsed = Rules::parse(&Ini::parse(&String::from_utf8_lossy(bytes)))
                .unwrap_or_else(|error| panic!("{location}: {error}"));
            rules_location = Some(location);
            rules = Some(parsed);
            break;
        }
    }
    if rules.is_none() {
        for outer_entry in redalert.entries() {
            let Some(outer_bytes) = redalert.by_id(outer_entry.id) else {
                continue;
            };
            let Ok(inner) = MixFile::parse(outer_bytes) else {
                continue;
            };
            for inner_entry in inner.entries() {
                let Some(candidate) = inner.by_id(inner_entry.id) else {
                    continue;
                };
                let text = String::from_utf8_lossy(candidate);
                if !text.contains("[1TNK]") {
                    continue;
                }
                let Ok(parsed) = Rules::parse(&Ini::parse(&text)) else {
                    continue;
                };
                if parsed.units.contains_key("1TNK") {
                    rules_location = Some(format!(
                        "redalert.mix/{:08x}/{:08x} (names unresolved)",
                        outer_entry.id, inner_entry.id
                    ));
                    rules = Some(parsed);
                    break;
                }
            }
        }
    }
    if let (Some(rules_location), Some(rules)) = (rules_location, rules) {
        assert!(rules.units.contains_key("1TNK"));
        println!("RA_RULES location={rules_location}");
        for (name, unit) in rules.units.iter().take(10) {
            println!(
                "RA_RULE_UNIT {name} cost={} strength={} speed={} armor={} primary={}",
                unit.cost, unit.strength, unit.speed, unit.armor, unit.primary
            );
        }
    } else {
        println!("RA_RULES absent_from_supplied_archives");
    }
}

#[test]
#[ignore]
fn decode_local_ts_pack_if_present() {
    let pack = local_packs().join("ts");
    if !pack.join("conquer.mix").is_file() {
        return;
    }
    let names = NameTable::with_hash_kind(HashKind::Crc32);
    let mut counts = BTreeMap::new();
    for archive in [
        "cache.mix",
        "conquer.mix",
        "isosnow.mix",
        "isotemp.mix",
        "local.mix",
        "sidec01.mix",
        "sidec02.mix",
        "sno.mix",
        "snow.mix",
        "sounds.mix",
        "speech01.mix",
        "speech02.mix",
        "tem.mix",
        "temperat.mix",
    ] {
        let bytes = read(&pack.join(archive));
        let mix = MixFile::parse(&bytes).unwrap_or_else(|error| panic!("{archive}: {error}"));
        let resolved = names
            .names()
            .filter(|name| mix.by_name_with_hash(name, HashKind::Crc32).is_some())
            .count();
        println!(
            "TS_MIX {archive} kind={:?} entries={} resolved={resolved}",
            mix.header_kind(),
            mix.entries().len()
        );
        counts.insert(archive, (mix.entries().len(), resolved));
    }
    assert!(counts["conquer.mix"].1 > 200);
    assert!(counts["isotemp.mix"].1 > 100);

    let conquer_bytes = read(&pack.join("conquer.mix"));
    let conquer = MixFile::parse(&conquer_bytes).unwrap();
    let mut shp_count = 0usize;
    let mut shp_frames = 0usize;
    let mut largest = Vec::new();
    for (id, name) in names.resolve_names(&conquer) {
        let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(".SHP")) else {
            continue;
        };
        let data = conquer.by_id(id).unwrap();
        let shp = ShpTs::parse(data).unwrap_or_else(|error| panic!("conquer.mix:{name}: {error}"));
        assert!(matches!(Sprite::parse(data), Ok(Sprite::TiberianSun(_))));
        let (width, height) = shp.canvas();
        let frame_count = shp.frames().len();
        shp_count += 1;
        shp_frames += frame_count;
        largest.push((
            name.to_owned(),
            u64::from(width) * u64::from(height) * frame_count as u64,
            width,
            height,
            frame_count,
        ));
    }
    largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    largest.truncate(10);
    println!("TS_SHP decoded={shp_count} frames={shp_frames}");
    for (name, _, width, height, frame_count) in &largest {
        println!("TS_SHP_LARGEST {name} canvas={width}x{height} frames={frame_count}");
    }
    assert_ne!(shp_count, 0);

    let mut template_counts = BTreeMap::new();
    let mut height_histogram = BTreeMap::<u8, usize>::new();
    let mut terrain_histogram = BTreeMap::<u8, usize>::new();
    let mut ramp_histogram = BTreeMap::<u8, usize>::new();
    for (archive, extension) in [("isotemp.mix", ".TEM"), ("isosnow.mix", ".SNO")] {
        let archive_bytes = read(&pack.join(archive));
        let mix = MixFile::parse(&archive_bytes).unwrap();
        let mut decoded = 0usize;
        let mut sprite_assets = 0usize;
        let mut tiles = 0usize;
        for name in names
            .names()
            .filter(|name| name.to_ascii_uppercase().ends_with(extension))
        {
            let Some(data) = mix.by_name_with_hash(name, HashKind::Crc32) else {
                continue;
            };
            let template = match TmpTs::parse(data) {
                Ok(template) => template,
                Err(error) if ShpTs::parse(data).is_ok() => {
                    // Bridges, tracks, tunnel tops, and vein animations keep
                    // the theater extension but are deliberately TS SHPs.
                    let _ = error;
                    sprite_assets += 1;
                    continue;
                }
                Err(error) => panic!("{archive}:{name}: {error}"),
            };
            let (blocks_x, blocks_y) = template.blocks();
            for by in 0..blocks_y {
                for bx in 0..blocks_x {
                    let Some(tile) = template.tile(bx, by) else {
                        continue;
                    };
                    *height_histogram.entry(tile.height).or_default() += 1;
                    *terrain_histogram.entry(tile.terrain_type).or_default() += 1;
                    *ramp_histogram.entry(tile.ramp_type).or_default() += 1;
                    tiles += 1;
                }
            }
            decoded += 1;
        }
        println!(
            "TS_TMP {archive} decoded={decoded} sprite_assets={sprite_assets} total={} tiles={tiles}",
            decoded + sprite_assets
        );
        template_counts.insert(archive, decoded + sprite_assets);
    }
    println!("TS_TMP_HEIGHT {height_histogram:?}");
    println!("TS_TMP_TERRAIN {terrain_histogram:?}");
    println!("TS_TMP_RAMP {ramp_histogram:?}");
    assert!(template_counts.values().all(|&count| count != 0));

    let palette_names = [
        "UNITTEM.PAL",
        "UNITSNO.PAL",
        "ISOTEM.PAL",
        "ISOSNO.PAL",
        "TEMPERAT.PAL",
        "SNOW.PAL",
    ];
    let archive_names = [
        "cache.mix",
        "conquer.mix",
        "isosnow.mix",
        "isotemp.mix",
        "local.mix",
        "sno.mix",
        "snow.mix",
        "tem.mix",
        "temperat.mix",
    ];
    let mut palette_locations = BTreeMap::<String, Vec<String>>::new();
    for archive in archive_names {
        let archive_bytes = read(&pack.join(archive));
        let mix = MixFile::parse(&archive_bytes).unwrap();
        for palette_name in palette_names {
            let Some(bytes) = mix.by_name_with_hash(palette_name, HashKind::Crc32) else {
                continue;
            };
            Pal::parse(bytes).unwrap_or_else(|error| panic!("{archive}:{palette_name}: {error}"));
            palette_locations
                .entry(palette_name.to_owned())
                .or_default()
                .push(archive.to_owned());
        }
    }
    for palette_name in palette_names {
        println!(
            "TS_PAL {palette_name} locations={:?}",
            palette_locations.get(palette_name).cloned().unwrap_or_default()
        );
    }
    assert!(
        palette_names
            .iter()
            .all(|name| palette_locations.contains_key(*name)),
        "missing TS palettes: {palette_locations:?}"
    );

    let mut rules_source = None;
    let mut art_source = None;
    for archive in ["local.mix", "cache.mix", "conquer.mix"] {
        let archive_bytes = read(&pack.join(archive));
        let mix = MixFile::parse(&archive_bytes).unwrap();
        if rules_source.is_none() {
            if let Some((inner_path, bytes)) = find_in_mix(&mix, "RULES.INI", &names, 2) {
                rules_source = Some((
                    format!("{archive}/{inner_path}"),
                    String::from_utf8_lossy(bytes).into_owned(),
                ));
            }
        }
        if art_source.is_none() {
            if let Some((inner_path, bytes)) = find_in_mix(&mix, "ART.INI", &names, 2) {
                art_source = Some((
                    format!("{archive}/{inner_path}"),
                    String::from_utf8_lossy(bytes).into_owned(),
                ));
            }
        }
    }
    let (rules_location, rules_text) = rules_source.expect("RULES.INI not found in TS mixes");
    let (art_location, art_text) = art_source.expect("ART.INI not found in TS mixes");
    let rules_ini = Ini::parse(&rules_text);
    let art_ini = Ini::parse(&art_text);
    let rules = Rules::parse_ts(&rules_ini, &art_ini).unwrap();
    println!("TS_RULES rules={rules_location} art={art_location}");
    println!(
        "TS_RULES_COUNTS units={} weapons={} warheads={}",
        rules.units.len(),
        rules.weapons.len(),
        rules.warheads.len()
    );
    for name in [
        "4TNK", "BGGY", "BIKE", "E1", "ENGINEER", "HARV", "HVR", "MMCH", "ORCA", "SMECH",
    ] {
        let Some(unit) = rules.units.get(name) else {
            continue;
        };
        println!(
            "TS_RULE_UNIT {name} image={} foundation={} tech_level={} owner={} prerequisite={} primary={} speed={} strength={} armor={} cost={} sight={}",
            unit.image,
            unit.foundation,
            unit.tech_level,
            unit.owner.join(","),
            unit.prerequisite.join(","),
            unit.primary,
            unit.speed,
            unit.strength,
            unit.armor,
            unit.cost,
            unit.sight,
        );
    }
    assert!(!rules.units.is_empty());
}
