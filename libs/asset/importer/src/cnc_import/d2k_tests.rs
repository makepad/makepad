use super::{
    aud::Aud,
    d2k_tiles::D2kTemplateTable,
    pal::Pal,
    r8::R8,
    shp::{Shp, ShpError},
    tmp::Tmp,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn local_d2k_pack() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../local/packs/d2k")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn files_with_extension(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
        .collect();
    paths.sort();
    paths
}

fn named_shp(directory: &Path, name: &str) -> Shp {
    let path = directory.join(format!("{name}.shp"));
    Shp::parse(&read(&path)).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn ranges(values: &BTreeSet<u8>) -> String {
    let mut output = Vec::new();
    let mut values = values.iter().copied();
    let Some(mut start) = values.next() else {
        return String::new();
    };
    let mut end = start;
    for value in values {
        if end.checked_add(1) == Some(value) {
            end = value;
        } else {
            output.push(if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            });
            start = value;
            end = value;
        }
    }
    output.push(if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    });
    output.join(",")
}

fn report_house_remap(directory: &Path, names: [&str; 3]) {
    let shapes = names.map(|name| named_shp(directory, name));
    assert_eq!(shapes[0].width(), shapes[1].width());
    assert_eq!(shapes[0].width(), shapes[2].width());
    assert_eq!(shapes[0].height(), shapes[1].height());
    assert_eq!(shapes[0].height(), shapes[2].height());
    assert_eq!(shapes[0].frames().len(), shapes[1].frames().len());
    assert_eq!(shapes[0].frames().len(), shapes[2].frames().len());

    let mut house_indices = [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
    let mut mappings = BTreeMap::<u8, BTreeSet<(u8, u8)>>::new();
    let mut differences = 0usize;
    for frame_index in 0..shapes[0].frames().len() {
        for ((&a, &h), &o) in shapes[0].frames()[frame_index]
            .iter()
            .zip(&shapes[1].frames()[frame_index])
            .zip(&shapes[2].frames()[frame_index])
        {
            if a != h || a != o {
                differences += 1;
                house_indices[0].insert(a);
                house_indices[1].insert(h);
                house_indices[2].insert(o);
                mappings.entry(a).or_default().insert((h, o));
            }
        }
    }
    let consistent_mappings = mappings
        .values()
        .filter(|targets| targets.len() == 1)
        .count();
    println!(
        "D2K_REMAP {} differences={differences} ranges={}/{}/{} consistent_mappings={consistent_mappings}/{}",
        names.join("/"),
        ranges(&house_indices[0]),
        ranges(&house_indices[1]),
        ranges(&house_indices[2]),
        mappings.len(),
    );
}

fn report_sprite_histogram(directory: &Path, name: &str) {
    let shp = named_shp(directory, name);
    let mut histogram = BTreeMap::<u8, usize>::new();
    for frame in shp.frames() {
        for &index in frame {
            *histogram.entry(index).or_default() += 1;
        }
    }
    println!("D2K_UNIT_HISTOGRAM {name} {histogram:?}");
}

#[test]
#[ignore]
fn decode_local_d2k_pack_if_present() {
    let pack = local_d2k_pack();
    if !pack.is_dir() {
        return;
    }
    let v2 = pack.join("orig/v2");
    if !v2.join("DATA.R8").is_file() {
        return;
    }

    let mut r8_counts = BTreeMap::new();
    for name in [
        "BLOXBASE.R8",
        "BLOXBAT.R8",
        "BLOXBGBS.R8",
        "BLOXICE.R8",
        "BLOXTREE.R8",
        "BLOXWAST.R8",
        "BLOXXMAS.R8",
        "DATA.R8",
        "MOUSE.R8",
    ] {
        let bundle = R8::parse(&read(&v2.join(name)))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        r8_counts.insert(name, bundle.entries().len());
        if name.starts_with("BLOX") {
            assert_eq!(bundle.entries().len(), 800, "{name}");
        }
        if name == "DATA.R8" {
            let mut histogram = BTreeMap::<(u32, u32), usize>::new();
            for entry in bundle.entries() {
                *histogram.entry((entry.w, entry.h)).or_default() += 1;
            }
            println!("D2K_DATA_R8_HISTOGRAM {histogram:?}");
            assert_eq!(bundle.entries().len(), 6_555);
        }
    }
    println!("D2K_R8 {r8_counts:?}");

    let base = R8::parse(&read(&v2.join("BLOXBASE.R8"))).unwrap();
    let base00 = Tmp::parse(&read(&pack.join("Tilesets/BASE00.bas"))).unwrap();
    assert_eq!(base00.icon_size(), (32, 32));
    assert_eq!(base00.icon_count(), 1);
    assert_eq!(base.entries()[0].pixels.as_deref(), base00.icon(0));
    println!("D2K_TILE_CROSSCHECK BLOXBASE[0]=BASE00.bas icon[0]");

    let palette = Pal::parse(&read(&v2.join("PALETTE.BIN"))).unwrap();
    assert_eq!(palette.rgb(0), [0, 0, 0]);
    println!("D2K_PALETTE index0={:?}", palette.rgb(0));

    let shp_directory = pack.join("SHPs");
    let mut decoded = Vec::new();
    let mut unsupported = Vec::new();
    let mut errors = Vec::new();
    for path in files_with_extension(&shp_directory, "shp") {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        match Shp::parse(&read(&path)) {
            Ok(shp) => decoded.push((name, shp.frames().len(), shp.width(), shp.height())),
            Err(ShpError::Unsupported) => unsupported.push(name),
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    decoded.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let selected: BTreeMap<_, _> = decoded
        .iter()
        .filter(|(name, _, _, _)| {
            matches!(
                name.as_str(),
                "rifle" | "mcvicon" | "lighto" | "conyarda" | "harvester"
            )
        })
        .map(|(name, frames, w, h)| (name.clone(), (*frames, *w, *h)))
        .collect();
    println!(
        "D2K_SHP decoded={} unsupported={} errors={} selected={selected:?}",
        decoded.len(),
        unsupported.len(),
        errors.len()
    );
    println!("D2K_SHP_UNSUPPORTED {unsupported:?}");
    println!("D2K_SHP_LARGEST {:?}", &decoded[..decoded.len().min(20)]);
    assert!(errors.is_empty(), "D2K SHP errors: {errors:#?}");

    report_house_remap(&shp_directory, ["lighta", "lighth", "lighto"]);
    report_house_remap(&shp_directory, ["conyarda", "conyardh", "conyardo"]);
    report_sprite_histogram(&shp_directory, "harvester");
    report_sprite_histogram(&shp_directory, "combata");

    let table = D2kTemplateTable::embedded().unwrap();
    let mut dimension_exceptions = Vec::new();
    for template in table.templates() {
        assert!(template.frames.iter().all(|&frame| frame < 800));
        let expected = usize::from(template.w) * usize::from(template.h);
        if template.frames.len() != expected {
            dimension_exceptions.push((template.id, expected, template.frames.len()));
        }
    }
    println!(
        "D2K_TEMPLATES count={} dimension_exceptions={dimension_exceptions:?}",
        table.templates().len()
    );

    let mut aud_count = 0usize;
    let mut sample_rates = BTreeMap::<u16, usize>::new();
    for path in files_with_extension(&pack.join("GAMESFX"), "aud") {
        let audio = Aud::parse(&read(&path))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        *sample_rates.entry(audio.sample_rate()).or_default() += 1;
        aud_count += 1;
    }
    println!("D2K_AUD decoded={aud_count} sample_rates={sample_rates:?}");

    let sound = read(&v2.join("SOUND.RS"));
    let first_64 = sound
        .get(..64)
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let table_size = u32::from_le_bytes(sound[..4].try_into().unwrap()) as usize;
    let table_end = 4usize.checked_add(table_size).unwrap();
    assert!(table_end <= sound.len());
    let mut at = 4usize;
    let mut sound_entries = Vec::new();
    while at < table_end {
        let name_end = sound[at..table_end]
            .iter()
            .position(|&byte| byte == 0)
            .map(|offset| at + offset)
            .expect("unterminated SOUND.RS name");
        let name = String::from_utf8_lossy(&sound[at..name_end]).into_owned();
        at = name_end + 1;
        let fields = sound.get(at..at + 8).expect("truncated SOUND.RS table");
        let offset = u32::from_le_bytes(fields[..4].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(fields[4..].try_into().unwrap()) as usize;
        at += 8;
        sound_entries.push((name, offset, length));
    }
    assert_eq!(at, table_end);
    assert_eq!(sound_entries.first().unwrap().1, table_end);
    for pair in sound_entries.windows(2) {
        assert_eq!(pair[0].1 + pair[0].2, pair[1].1);
    }
    let last = sound_entries.last().unwrap();
    assert_eq!(last.1 + last.2, sound.len());
    assert!(sound_entries.iter().all(|(_, offset, length)| {
        sound.get(*offset..offset.saturating_add(*length))
            .is_some_and(|data| data.starts_with(b"RIFF"))
    }));
    println!("D2K_SOUND_RS first64={first_64}");
    println!(
        "D2K_SOUND_RS table_bytes={table_size} entries={} first={:?} last={:?}",
        sound_entries.len(),
        sound_entries.first(),
        sound_entries.last()
    );
}
