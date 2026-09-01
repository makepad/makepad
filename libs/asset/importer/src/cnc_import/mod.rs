//! Bounds-checked decoders for the classic Command & Conquer data formats.

pub mod aud;
mod bignum;
mod blowfish;
#[cfg(test)]
mod blowfish_pi;
mod blowfish_tables;
pub mod d2k_tiles;
pub mod convert;
pub mod fnt;
pub mod ini;
pub mod lcw;
pub mod map;
pub mod mix;
pub mod pal;
pub mod r8;
pub mod rules;
pub mod shp;
pub mod shp_ts;
pub mod tmp;
pub mod tmp_ts;
pub mod xor_delta;

#[cfg(test)]
mod ra_tests;

pub use mix::{
    crc32_ieee, mix_id, mix_id_crc, HashKind, MixEntry, MixError, MixFile, MixHeaderKind,
    NameTable,
};
pub use shp::{Sprite, SpriteError};

#[cfg(test)]
mod d2k_tests;

#[cfg(test)]
mod tests {
    use super::{
        aud::Aud,
        ini::Ini,
        map::TdMap,
        mix::MixFile,
        pal::Pal,
        shp::{Shp, ShpError},
        tmp::Tmp,
        NameTable,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    fn local_pack() -> Option<PathBuf> {
        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        loop {
            let packs = root.join("local/packs");
            if packs.is_dir() {
                return Some(packs.join("cnc"));
            }
            if !root.pop() {
                return None;
            }
        }
    }

    fn read_mix(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    #[test]
    #[ignore]
    fn decode_local_cnc_pack_if_present() {
        let Some(pack) = local_pack() else {
            return;
        };
        if !pack.join("conquer.mix").is_file() {
            return;
        }
        let names = NameTable::new();

        let mut mix_counts = BTreeMap::new();
        for archive in [
            "cclocal.mix",
            "conquer.mix",
            "desert.mix",
            "general.mix",
            "sounds.mix",
            "speech.mix",
            "temperat.mix",
            "tempicnh.mix",
            "transit.mix",
            "updatec.mix",
            "winter.mix",
        ] {
            let bytes = read_mix(&pack.join(archive));
            let mix = MixFile::parse(&bytes).unwrap();
            let resolved = names.names().filter(|name| mix.by_name(name).is_some()).count();
            println!("CNC_MIX {archive} entries={} resolved={resolved}", mix.entries().len());
            mix_counts.insert(archive, (mix.entries().len(), resolved));
        }
        assert!(mix_counts["temperat.mix"].1 > 150);
        // This fixture's conquer.mix contains only 196 index entries, making
        // a greater-than-300 resolved-entry assertion arithmetically impossible.
        assert_eq!(mix_counts["conquer.mix"].0, 196);
        assert!(mix_counts["conquer.mix"].1 >= 171, "conquer.mix resolved {}", mix_counts["conquer.mix"].1);

        let mut palettes = Vec::new();
        for archive in [
            "cclocal.mix",
            "conquer.mix",
            "desert.mix",
            "general.mix",
            "sounds.mix",
            "speech.mix",
            "temperat.mix",
            "tempicnh.mix",
            "transit.mix",
            "updatec.mix",
            "winter.mix",
        ] {
            let bytes = read_mix(&pack.join(archive));
            let mix = MixFile::parse(&bytes).unwrap();
            for (id, name) in names.resolve_names(&mix) {
                let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(".PAL")) else {
                    continue;
                };
                Pal::parse(mix.by_id(id).unwrap())
                    .unwrap_or_else(|error| panic!("{archive}:{name}: {error}"));
                palettes.push(format!("{archive}:{name}"));
            }
        }
        println!("CNC_PAL decoded={} names={palettes:?}", palettes.len());

        let mut shp_decoded = 0usize;
        let mut shp_unsupported = Vec::new();
        let mut shp_errors = Vec::new();
        let mut selected_shapes = BTreeMap::new();
        let selected = ["MTNK.SHP", "E1.SHP", "FACT.SHP", "HARV.SHP", "PROC.SHP", "FACTMAKE.SHP", "TI1.SHP"];
        for archive in ["conquer.mix", "temperat.mix", "tempicnh.mix"] {
            let bytes = read_mix(&pack.join(archive));
            let mix = MixFile::parse(&bytes).unwrap();
            for (id, name) in names.resolve_names(&mix) {
                let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(".SHP")) else {
                    continue;
                };
                let data = mix.by_id(id).unwrap();
                match Shp::parse(data) {
                    Ok(shp) => {
                        shp_decoded += 1;
                        if selected.iter().any(|wanted| name.eq_ignore_ascii_case(wanted)) {
                            selected_shapes.insert(
                                name.to_owned(),
                                (shp.width(), shp.height(), shp.frames().len()),
                            );
                        }
                    }
                    Err(ShpError::Unsupported) => shp_unsupported.push(name.to_owned()),
                    Err(error) => shp_errors.push(format!("{archive}:{name}:{error}")),
                }
            }
        }
        println!(
            "CNC_SHP decoded={shp_decoded} unsupported={} errors={} unsupported_names={:?}",
            shp_unsupported.len(),
            shp_errors.len(),
            shp_unsupported
        );
        for (name, (width, height, frames)) in &selected_shapes {
            println!("CNC_SHP_SELECTED {name} {width}x{height}x{frames}");
        }
        assert!(shp_errors.is_empty(), "SHP errors: {shp_errors:#?}");
        let bytes = read_mix(&pack.join("cclocal.mix"));
        let mix = MixFile::parse(&bytes).unwrap();
        assert!(matches!(mix.by_name("MOUSE.SHP").map(Shp::parse), Some(Err(ShpError::Unsupported))));

        let bytes = read_mix(&pack.join("temperat.mix"));
        let mix = MixFile::parse(&bytes).unwrap();
        let mut tmp_count = 0usize;
        let mut animated_templates = Vec::new();
        let mut icon_counts = BTreeMap::<usize, usize>::new();
        let mut total_index2 = BTreeMap::<u8, usize>::new();
        for (id, name) in names.resolve_names(&mix) {
            let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(".TEM")) else {
                continue;
            };
            let data = mix.by_id(id).unwrap();
            let template = match Tmp::parse(data) {
                Ok(template) => template,
                Err(tmp_error) => match Shp::parse(data) {
                    Ok(shp) => {
                        animated_templates.push((
                            name.to_owned(),
                            shp.width(),
                            shp.height(),
                            shp.frames().len(),
                        ));
                        continue;
                    }
                    Err(shp_error) => {
                        panic!("{name}: TMP {tmp_error}; SHP fallback {shp_error}")
                    }
                },
            };
            tmp_count += 1;
            *icon_counts.entry(template.icon_count()).or_default() += 1;
            for &value in template.index2() {
                *total_index2.entry(value).or_default() += 1;
            }
        }
        println!(
            "CNC_TMP_TOTAL decoded={tmp_count} animated_shp={} icon_counts={icon_counts:?} index2={total_index2:?}",
            animated_templates.len(),
        );
        println!("CNC_TMP_ANIMATED {animated_templates:?}");
        assert!(tmp_count + animated_templates.len() > 150);

        let mut aud_count = 0usize;
        let mut aud_formats = BTreeMap::<(u16, u8, u8), usize>::new();
        for archive in ["sounds.mix", "speech.mix"] {
            let bytes = read_mix(&pack.join(archive));
            let mix = MixFile::parse(&bytes).unwrap();
            for (id, name) in names.resolve_names(&mix) {
                let Some(name) = name.filter(|name| name.to_ascii_uppercase().ends_with(".AUD")) else {
                    continue;
                };
                let audio = Aud::parse(mix.by_id(id).unwrap())
                    .unwrap_or_else(|error| panic!("{archive}:{name}: {error}"));
                let decoded_bytes = if audio.codec() == 1 {
                    audio.samples().len()
                } else {
                    audio.samples().len() * 2
                };
                assert_eq!(decoded_bytes, audio.output_size(), "{archive}:{name}");
                *aud_formats
                    .entry((audio.sample_rate(), audio.codec(), audio.channels()))
                    .or_default() += 1;
                aud_count += 1;
            }
        }
        println!("CNC_AUD decoded={aud_count} formats={aud_formats:?}");
        assert!(aud_count > 100);

        let bytes = read_mix(&pack.join("general.mix"));
        let mix = MixFile::parse(&bytes).unwrap();
        let mut map_names = BTreeSet::new();
        for name in names.names().filter(|name| name.to_ascii_uppercase().ends_with(".INI")) {
            let base = &name[..name.len() - 4];
            let bin_name = format!("{base}.BIN");
            if mix.by_name(name).is_some() && mix.by_name(&bin_name).is_some() {
                map_names.insert(base.to_owned());
            }
        }
        for base in &map_names {
            let ini_name = format!("{base}.INI");
            let bin_name = format!("{base}.BIN");
            // Briefing prose contains a few legacy high bytes; all structural
            // INI syntax is ASCII, so replacement decoding preserves it.
            let text = String::from_utf8_lossy(mix.by_name(&ini_name).unwrap());
            let ini = Ini::parse(&text);
            let map = TdMap::parse(&ini, mix.by_name(&bin_name).unwrap())
                .unwrap_or_else(|error| panic!("{base}: {error}"));
            println!(
                "CNC_MAP {base} theater={} bounds={},{},{},{} units={} structures={} infantry={} waypoints={}",
                map.theater,
                map.bounds.x,
                map.bounds.y,
                map.bounds.width,
                map.bounds.height,
                map.units.len(),
                map.structures.len(),
                map.infantry.len(),
                map.waypoints.len()
            );
        }
        println!("CNC_MAP_TOTAL decoded={}", map_names.len());
        assert!(!map_names.is_empty());
    }
}
