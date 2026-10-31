//! The finished styles and layouts (the names each folder's `EMBED` lists)
//! built into the binary, so an installed bundle (no source tree) carries
//! them. A source checkout also finds every other file on disk at run time.
//! Each table is in name order. The `tests` module fails when a table and
//! its folder's `EMBED` list differ, so a style added to `EMBED` needs its
//! line here.

pub const EMBEDDED: &[(&str, &str)] = &[
    ("artdeco", include_str!("../resources/styles/artdeco.splash")),
    ("bauhaus", include_str!("../resources/styles/bauhaus.splash")),
    ("brushed", include_str!("../resources/styles/brushed.splash")),
    ("brutalist", include_str!("../resources/styles/brutalist.splash")),
    ("cyberpunk", include_str!("../resources/styles/cyberpunk.splash")),
    ("graphite", include_str!("../resources/styles/graphite.splash")),
    ("holo", include_str!("../resources/styles/holo.splash")),
    ("lcd", include_str!("../resources/styles/lcd.splash")),
    ("military", include_str!("../resources/styles/military.splash")),
    ("modern", include_str!("../resources/styles/modern.splash")),
    ("nostromo", include_str!("../resources/styles/nostromo.splash")),
    ("orbital", include_str!("../resources/styles/orbital.splash")),
    ("piano", include_str!("../resources/styles/piano.splash")),
    ("plastic", include_str!("../resources/styles/plastic.splash")),
    ("rack", include_str!("../resources/styles/rack.splash")),
    ("steampunk", include_str!("../resources/styles/steampunk.splash")),
    ("synthwave", include_str!("../resources/styles/synthwave.splash")),
    ("tube", include_str!("../resources/styles/tube.splash")),
    ("walnut", include_str!("../resources/styles/walnut.splash")),
    ("y2k", include_str!("../resources/styles/y2k.splash")),
];

pub const EMBEDDED_LAYOUTS: &[(&str, &str)] = &[
    ("classic", include_str!("../resources/layouts/classic.splash")),
    ("compact", include_str!("../resources/layouts/compact.splash")),
    ("deck", include_str!("../resources/layouts/deck.splash")),
    ("library", include_str!("../resources/layouts/library.splash")),
    ("mini", include_str!("../resources/layouts/mini.splash")),
    ("strip", include_str!("../resources/layouts/strip.splash")),
    ("theatre", include_str!("../resources/layouts/theatre.splash")),
    ("tile", include_str!("../resources/layouts/tile.splash")),
    ("widget", include_str!("../resources/layouts/widget.splash")),
];

#[cfg(test)]
mod tests {
    use super::{EMBEDDED, EMBEDDED_LAYOUTS};

    fn check(table: &[(&str, &str)], folder: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join(folder);
        let list = std::fs::read_to_string(dir.join("EMBED")).unwrap_or_else(|e| panic!("{folder}/EMBED: {e}"));
        let mut wanted: Vec<String> =
            list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect();
        wanted.sort();
        let listed: Vec<String> = table.iter().map(|(n, _)| n.to_string()).collect();
        let missing: Vec<&String> = wanted.iter().filter(|n| !listed.contains(n)).collect();
        let extra: Vec<&String> = listed.iter().filter(|n| !wanted.contains(n)).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "src/embedded_styles.rs is out of date for {folder}/EMBED: names missing from the table {missing:?}, table entries not in EMBED {extra:?}"
        );
        assert!(listed.windows(2).all(|w| w[0] < w[1]), "src/embedded_styles.rs must list {folder} in name order");
        for name in &wanted {
            assert!(dir.join(format!("{name}.splash")).is_file(), "{folder}/EMBED names {name:?}, which has no .splash file");
        }
    }

    #[test]
    fn styles_match_embed_list() {
        check(EMBEDDED, "styles");
    }

    #[test]
    fn layouts_match_embed_list() {
        check(EMBEDDED_LAYOUTS, "layouts");
    }
}
