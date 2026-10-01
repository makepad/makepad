use {
    super::*,
    rustybuzz::UnicodeBuffer,
    std::path::{Path, PathBuf},
    ttf_parser::{GlyphId, OutlineBuilder, Tag},
};

/// The film's text sample stand-in.
const SAMPLE: &str = "P(DOOM) 0123456789 %.,:-\u{2014}'\u{2019} ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz";
const CJK_SAMPLE: &str = "\u{5B57}\u{4F53}\u{5B50}\u{96C6}\u{6D4B}\u{8BD5}\u{FF0C}\u{6587}\u{5B57}\u{3002} 2026";

/// The film's fonts, relative to the repository root. The commercial ones
/// are absent when that repository is not cloned; their checks are skipped.
const FONTS: &[&str] = &[
    "widgets/resources/RobotoFlex.ttf",
    "widgets/resources/jetbrains_mono_variable.ttf",
    "widgets/resources/Inter.ttf",
    "widgets/resources/IBMPlexSans-Text.ttf",
    "apps/commercial/engine/motion/resources/fonts/playfair-display/PlayfairDisplay-Variable.ttf",
    "apps/commercial/engine/motion/resources/fonts/playfair-display/PlayfairDisplay-Italic-Variable.ttf",
    "apps/commercial/engine/motion/resources/fonts/cormorant-garamond/CormorantGaramond-Variable.ttf",
];
const CJK_FONT: &str = "widgets/resources/LXGWWenKaiRegular.ttf";

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(relative)
}

fn load(relative: &str) -> Option<Vec<u8>> {
    let data = std::fs::read(repo_path(relative)).ok();
    if data.is_none() {
        eprintln!("skipping {relative}: not present");
    }
    data
}

fn run(text: &str) -> ShapeRun {
    ShapeRun {
        text: text.to_string(),
        ..Default::default()
    }
}

/// The axis settings a check visits: the default, every axis at its minimum
/// and maximum alone, and the heaviest-narrowest and lightest-widest corners.
fn instances(face: &ttf_parser::Face) -> Vec<Vec<(Tag, f32)>> {
    let axes: Vec<_> = face.variation_axes().into_iter().collect();
    let mut instances = vec![Vec::new()];
    for axis in &axes {
        instances.push(vec![(axis.tag, axis.min_value)]);
        instances.push(vec![(axis.tag, axis.max_value)]);
    }
    let find = |tag: &[u8; 4]| axes.iter().find(|a| a.tag == Tag::from_bytes(tag));
    if let (Some(wght), Some(wdth)) = (find(b"wght"), find(b"wdth")) {
        instances.push(vec![(wght.tag, wght.max_value), (wdth.tag, wdth.min_value)]);
        instances.push(vec![(wght.tag, wght.min_value), (wdth.tag, wdth.max_value)]);
    }
    instances
}

fn face_at<'a>(data: &'a [u8], instance: &[(Tag, f32)]) -> ttf_parser::Face<'a> {
    let mut face = ttf_parser::Face::parse(data, 0).unwrap();
    for &(tag, value) in instance {
        face.set_variation(tag, value);
    }
    face
}

#[derive(Default, PartialEq, Debug)]
struct Recorder(Vec<(u8, [u32; 6])>);

impl OutlineBuilder for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push((b'M', [x.to_bits(), y.to_bits(), 0, 0, 0, 0]));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push((b'L', [x.to_bits(), y.to_bits(), 0, 0, 0, 0]));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.push((b'Q', [x1.to_bits(), y1.to_bits(), x.to_bits(), y.to_bits(), 0, 0]));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.push((b'C', [x1.to_bits(), y1.to_bits(), x2.to_bits(), y2.to_bits(), x.to_bits(), y.to_bits()]));
    }
    fn close(&mut self) {
        self.0.push((b'Z', [0; 6]));
    }
}

fn outline(face: &ttf_parser::Face, glyph: u16) -> (Option<ttf_parser::Rect>, Recorder) {
    let mut recorder = Recorder::default();
    let bbox = face.outline_glyph(GlyphId(glyph), &mut recorder);
    (bbox, recorder)
}

fn shaper_at<'a>(data: &'a [u8], instance: &[(Tag, f32)]) -> rustybuzz::Face<'a> {
    let mut face = rustybuzz::Face::from_slice(data, 0).unwrap();
    let variations: Vec<_> = instance.iter().map(|&(tag, value)| rustybuzz::Variation { tag, value }).collect();
    face.set_variations(&variations);
    face
}

fn shaped(face: &rustybuzz::Face, text: &str) -> Vec<(u32, u32, i32, i32, i32, i32)> {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    let glyphs = rustybuzz::shape(&face, &[], buffer);
    glyphs
        .glyph_infos()
        .iter()
        .zip(glyphs.glyph_positions())
        .map(|(i, p)| (i.glyph_id, i.cluster, p.x_advance, p.y_advance, p.x_offset, p.y_offset))
        .collect()
}

fn subset_text(data: &[u8], text: &str) -> (Vec<u8>, BTreeSet<u16>, SubsetReport) {
    subset_text_with(data, text, &SubsetOptions::default())
}

fn subset_text_with(data: &[u8], text: &str, opts: &SubsetOptions) -> (Vec<u8>, BTreeSet<u16>, SubsetReport) {
    let inputs = ClosureInputs {
        runs: vec![run(text)],
        ..Default::default()
    };
    let keep = closure(data, &inputs).unwrap();
    let (font, report) = subset_with_report(data, &keep, &inputs.chars(), opts).unwrap();
    assert!(report.kept.is_superset(&keep));
    verify(data, &font, &inputs).unwrap();
    let kept = report.kept.clone();
    (font, kept, report)
}

/// The kept glyphs draw, advance and shape as in the original at every
/// checked axis setting; the rest have no outline.
fn check_font(relative: &str, text: &str) {
    check_font_with(relative, text, &SubsetOptions::default());
}

/// [`check_font`] with `opts`; pinned axes stay at their defaults.
fn check_font_with(relative: &str, text: &str, opts: &SubsetOptions) -> Option<SubsetReport> {
    let original = load(relative)?;
    let (subset, keep, report) = subset_text_with(&original, text, opts);
    let original_face = ttf_parser::Face::parse(&original, 0).unwrap();
    let subset_face = ttf_parser::Face::parse(&subset, 0).expect("subset parses with ttf-parser");
    assert!(rustybuzz::Face::from_slice(&subset, 0).is_some(), "subset parses with rustybuzz");
    assert_eq!(subset_face.number_of_glyphs() as usize, report.glyphs_out);
    assert!(keep.iter().all(|&g| (g as usize) < report.glyphs_out));
    assert!(subset.len() < original.len(), "{relative}: {report}");

    let mut varied = false;
    let pinned = |instance: &Vec<(Tag, f32)>| instance.iter().any(|(t, _)| opts.pin_axes.contains(&t.0));
    for instance in instances(&original_face).into_iter().filter(|i| !pinned(i)) {
        let a = face_at(&original, &instance);
        let b = face_at(&subset, &instance);
        for &glyph in &keep {
            assert_eq!(outline(&a, glyph), outline(&b, glyph), "{relative} glyph {glyph} at {instance:?}");
            assert_eq!(a.glyph_hor_advance(GlyphId(glyph)), b.glyph_hor_advance(GlyphId(glyph)));
            assert_eq!(a.glyph_hor_side_bearing(GlyphId(glyph)), b.glyph_hor_side_bearing(GlyphId(glyph)));
        }
        if !instance.is_empty() {
            let default = face_at(&original, &[]);
            varied |= keep.iter().any(|&g| outline(&a, g) != outline(&default, g));
        }
        let (a, b) = (shaper_at(&original, &instance), shaper_at(&subset, &instance));
        assert_eq!(shaped(&a, text), shaped(&b, text), "{relative} at {instance:?}");
    }
    if opts.pin_axes.is_empty() {
        assert_eq!(varied, original_face.is_variable(), "{relative}: the axis settings change the outlines");
    }
    // Every pair of the text's characters kerns as in the original (GPOS
    // pair pruning), at the default and the wght extremes.
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect::<BTreeSet<_>>().into_iter().collect();
    let mut pair_instances = vec![Vec::new()];
    if let Some(wght) = original_face.variation_axes().into_iter().find(|a| a.tag == Tag::from_bytes(b"wght")) {
        pair_instances.push(vec![(wght.tag, wght.min_value)]);
        pair_instances.push(vec![(wght.tag, wght.max_value)]);
    }
    let mut pairs = 0;
    for instance in pair_instances {
        let (a, b) = (shaper_at(&original, &instance), shaper_at(&subset, &instance));
        for &l in &chars {
            for &r in &chars {
                let pair: String = [l, r].into_iter().collect();
                let original_pair = shaped(&a, &pair);
                // A pair can shape to a contextual form the text never
                // used; the subset does not draw those.
                if original_pair.iter().all(|g| keep.contains(&(g.0 as u16))) {
                    assert_eq!(original_pair, shaped(&b, &pair), "{relative}: {pair:?} at {instance:?}");
                    pairs += 1;
                }
            }
        }
    }
    // Legacy `kern` pairs survive (GPOS kerning is in the shaping check).
    if let Some(kern) = original_face.tables().kern {
        let subset_kern = subset_face.tables().kern.unwrap();
        for (a, b) in kern.subtables.into_iter().zip(subset_kern.subtables) {
            for &l in &keep {
                for &r in &keep {
                    assert_eq!(a.glyphs_kerning(GlyphId(l), GlyphId(r)), b.glyphs_kerning(GlyphId(l), GlyphId(r)));
                }
            }
        }
    }
    assert!(pairs > chars.len() * chars.len() / 2, "{relative}: {pairs} pairs checked");
    let mut emptied = 0;
    for glyph in 0..subset_face.number_of_glyphs() {
        if !keep.contains(&glyph) {
            assert!(outline(&subset_face, glyph).0.is_none(), "{relative} glyph {glyph} kept an outline");
            emptied += 1;
        }
    }
    // Characters outside the text no longer map (unless they share a glyph).
    for c in ['\u{416}', '\u{E9}', '#', '\u{4E00}'] {
        if let Some(g) = subset_face.glyph_index(c) {
            assert!(keep.contains(&g.0), "{relative}: {c:?} maps to an emptied glyph");
        }
    }
    // The layout tables were rewritten, not kept by the fallback.
    if opts.prune_layout {
        for table in report.tables.iter().filter(|t| t.tag == "GPOS" && t.before > 4096) {
            assert!(table.after * 2 < table.before, "{relative}: {report}");
        }
    }
    eprintln!("{relative}: kept {} glyphs, emptied {emptied}\n{report}", keep.len());
    Some(report)
}

#[test]
fn latin_fonts_keep_their_outlines_and_shaping() {
    for font in FONTS {
        check_font(font, SAMPLE);
    }
}

#[test]
fn cjk_font_keeps_its_outlines_and_shaping() {
    check_font(CJK_FONT, CJK_SAMPLE);
}

#[test]
fn ligatures_and_features_reach_the_closure() {
    let Some(original) = load("widgets/resources/RobotoFlex.ttf") else { return };
    let tnum = u32::from_be_bytes(*b"tnum");
    let inputs = ClosureInputs {
        runs: vec![ShapeRun {
            text: "office 2026".into(),
            features: vec![(tnum, 1)],
            variations: vec![(u32::from_be_bytes(*b"wght"), 900.0)],
            rtl: false,
        }],
        ..Default::default()
    };
    let keep = closure(&original, &inputs).unwrap();
    let subset = subset(&original, &keep, &inputs.chars(), &SubsetOptions::default()).unwrap();
    let mut a = rustybuzz::Face::from_slice(&original, 0).unwrap();
    let mut b = rustybuzz::Face::from_slice(&subset, 0).unwrap();
    let wght = rustybuzz::Variation { tag: Tag::from_bytes(b"wght"), value: 900.0 };
    a.set_variations(&[wght]);
    b.set_variations(&[wght]);
    let features = [rustybuzz::Feature::new(Tag::from_bytes(b"tnum"), 1, ..)];
    let shape = |face: &rustybuzz::Face| {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str("office 2026");
        let out = rustybuzz::shape(face, &features, buffer);
        out.glyph_infos()
            .iter()
            .zip(out.glyph_positions())
            .map(|(i, p)| (i.glyph_id, p.x_advance))
            .collect::<Vec<_>>()
    };
    let shaped = shape(&a);
    assert_eq!(shaped, shape(&b));
    assert!(shaped.iter().all(|(g, _)| keep.contains(&(*g as u16))));
}

#[test]
fn gsub_chars_keep_every_alternate() {
    let Some(original) = load("widgets/resources/RobotoFlex.ttf") else { return };
    let digits: BTreeSet<char> = ('0'..='9').collect();
    let shaped_only = closure(&original, &ClosureInputs { runs: vec![run("0123456789")], ..Default::default() }).unwrap();
    let inputs = ClosureInputs { gsub_chars: digits, ..Default::default() };
    let conservative = closure(&original, &inputs).unwrap();
    assert!(conservative.is_superset(&shaped_only));
    assert!(conservative.len() > shaped_only.len(), "tnum/onum/sups digits are reachable through GSUB");
    // Every feature's digits shape the same in the subset.
    let subset = subset(&original, &conservative, &inputs.chars(), &SubsetOptions::default()).unwrap();
    for feature in [*b"tnum", *b"onum", *b"pnum", *b"sups", *b"sinf", *b"frac", *b"zero"] {
        let features = [rustybuzz::Feature::new(Tag::from_bytes(&feature), 1, ..)];
        let shape = |data: &[u8]| {
            let face = rustybuzz::Face::from_slice(data, 0).unwrap();
            let mut buffer = UnicodeBuffer::new();
            buffer.push_str("0123456789");
            let out = rustybuzz::shape(&face, &features, buffer);
            out.glyph_infos().iter().map(|i| i.glyph_id).collect::<Vec<_>>()
        };
        assert_eq!(shape(&original), shape(&subset), "{}", String::from_utf8_lossy(&feature));
    }
}

#[test]
fn checksums_are_consistent() {
    let Some(original) = load("widgets/resources/jetbrains_mono_variable.ttf") else { return };
    let (subset, _, _) = subset_text(&original, SAMPLE);
    let mut sum = 0u32;
    for chunk in subset.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    assert_eq!(sum, 0xB1B0_AFBA);
    let tables = Tables::read(&subset).unwrap();
    assert!(tables.get(b"DSIG").is_none() && tables.get(b"STAT").is_none());
    assert_eq!(u16_at(tables.get(b"post").unwrap(), 0).unwrap(), 3);
}

#[test]
fn reserved_font_names_from_licences() {
    assert_eq!(reserved_font_names("Copyright \u{a9} 2017 IBM Corp. with Reserved Font Name \"Plex\"\n\nThis Font Software is licensed under the SIL Open Font License, Version 1.1.\n\"Reserved Font Name\" refers to any names specified as such after the\ncopyright statement(s)."), ["Plex"]);
    assert_eq!(reserved_font_names("(https://github.com/googlefonts/lexend), with Reserved Font Name \u{201c}RevReading Lexend\u{201d}."), ["RevReading Lexend"]);
    assert_eq!(reserved_font_names("Copyright 2010 Adobe, with Reserved Font Names 'Source' and \"Source Sans\", \"Source Code\"."), ["Source", "Source Sans", "Source Code"]);
    assert_eq!(reserved_font_names("Copyright 2011 The X Authors, with Reserved Font Name Fira Sans.\n"), ["Fira Sans"]);
    assert!(reserved_font_names("Copyright 2020 The Archivo Project Authors\n\"Reserved Font Name\" refers to any names specified as such").is_empty());
    for (licence, expected) in [
        ("apps/commercial/engine/motion/resources/fonts/playfair-display/OFL.txt", vec!["Playfair Display"]),
        ("apps/commercial/engine/motion/resources/fonts/anton/OFL.txt", vec![]),
        ("apps/commercial/engine/motion/src/fonts/ibm-plex-mono-OFL.txt", vec!["Plex"]),
    ] {
        if let Some(text) = load(licence) {
            assert_eq!(reserved_font_names(&String::from_utf8(text).unwrap()), expected, "{licence}");
        }
    }
}

#[test]
fn rename_replaces_the_reserved_family() {
    let path = "apps/commercial/engine/motion/resources/fonts/playfair-display/PlayfairDisplay-Variable.ttf";
    let Some(original) = load(path) else { return };
    let inputs = ClosureInputs { runs: vec![run("Title")], ..Default::default() };
    let keep = closure(&original, &inputs).unwrap();
    let opts = SubsetOptions {
        rename: Some(Rename { reserved: vec!["Playfair Display".into()], family: "Film Serif".into() }),
        ..Default::default()
    };
    let subset = subset(&original, &keep, &inputs.chars(), &opts).unwrap();
    let face = ttf_parser::Face::parse(&subset, 0).unwrap();
    let mut saw_family = false;
    for name in face.names() {
        let Some(text) = name.to_string() else { continue };
        if [0, 7, 13].contains(&name.name_id) {
            continue;
        }
        assert!(!text.contains("Playfair Display") && !text.contains("PlayfairDisplay"), "name {}: {text}", name.name_id);
        saw_family |= name.name_id == 1 && text.contains("Film Serif");
        if name.name_id == 6 {
            assert!(text.starts_with("FilmSerif"), "{text}");
        }
    }
    assert!(saw_family);
}

#[test]
fn layout_kept_in_place_when_not_pruned() {
    let opts = SubsetOptions { prune_layout: false, ..Default::default() };
    for font in ["widgets/resources/Inter.ttf", "widgets/resources/RobotoFlex.ttf"] {
        if let Some(report) = check_font_with(font, SAMPLE, &opts) {
            assert_eq!(report.glyphs_out, report.glyphs_total, "no trimming without the layout rewrite");
        }
    }
}

#[test]
fn trimmed_fonts_end_at_the_last_kept_glyph() {
    let Some(report) = check_font_with("widgets/resources/IBMPlexSans-Text.ttf", SAMPLE, &SubsetOptions::default()) else { return };
    assert_eq!(report.glyphs_out, *report.kept.last().unwrap() as usize + 1);
    assert!(report.glyphs_out < report.glyphs_total);
}

#[test]
fn pinned_axes_drop_their_variations() {
    let path = "widgets/resources/RobotoFlex.ttf";
    let Some(original) = load(path) else { return };
    let wght = u32::from_be_bytes(*b"wght");
    let wdth = u32::from_be_bytes(*b"wdth");
    // The text was laid out at two weights and the default width: every
    // other axis (and wdth) is unvaried.
    let unvaried = unvaried_axes(&original, &[vec![(wght, 300.0)], vec![(wght, 800.0), (wdth, 100.0)]]).unwrap();
    assert!(!unvaried.contains(&wght) && unvaried.contains(&wdth) && unvaried.len() > 8, "{unvaried:?}");
    let pin_axes: Vec<u32> = unvaried.into_iter().filter(|&t| t != wdth).collect();
    let pinned = check_font_with(path, SAMPLE, &SubsetOptions { pin_axes, ..Default::default() }).unwrap();
    let full = check_font_with(path, SAMPLE, &SubsetOptions::default()).unwrap();
    let gvar = |r: &SubsetReport| r.tables.iter().find(|t| t.tag == "gvar").unwrap().after;
    assert!(gvar(&pinned) * 2 < gvar(&full), "{} vs {}", gvar(&pinned), gvar(&full));
    // At a pinned axis's non-default value the subset no longer varies.
    let (subset, keep, _) = subset_text_with(&original, SAMPLE, &SubsetOptions { pin_axes: vec![wght], ..Default::default() });
    let heavy = [(Tag::from_bytes(b"wght"), 900.0)];
    let g = *keep.iter().find(|&&g| outline(&face_at(&original, &heavy), g) != outline(&face_at(&original, &[]), g)).unwrap();
    assert_eq!(outline(&face_at(&subset, &heavy), g), outline(&face_at(&subset, &[]), g));
}

#[test]
fn stubs_are_tiny_and_keep_the_metrics() {
    let mut fonts: Vec<&str> = FONTS.to_vec();
    fonts.push(CJK_FONT);
    for path in fonts {
        let Some(original) = load(path) else { continue };
        let stub = stub(&original, &SubsetOptions::default()).unwrap();
        assert!(stub.len() < 2048, "{path}: stub of {} bytes", stub.len());
        let a = ttf_parser::Face::parse(&original, 0).unwrap();
        let b = ttf_parser::Face::parse(&stub, 0).expect("stub parses");
        assert_eq!(b.number_of_glyphs(), 1);
        assert_eq!((a.ascender(), a.descender(), a.line_gap(), a.units_per_em()), (b.ascender(), b.descender(), b.line_gap(), b.units_per_em()));
        assert_eq!(a.glyph_hor_advance(GlyphId(0)), b.glyph_hor_advance(GlyphId(0)));
        assert_eq!(outline(&a, 0), outline(&b, 0));
        assert!(b.glyph_index('A').is_none());
        let shaped = shaped(&rustybuzz::Face::from_slice(&stub, 0).expect("the shaper reads the stub"), "Ab 1");
        assert!(shaped.iter().all(|g| g.0 == 0));
        eprintln!("{path}: stub {} bytes, brotli {}", stub.len(), brotli(&stub));
    }
}

#[test]
fn apple_layout_dropped_shapes_the_same() {
    let Some(original) = load(CJK_FONT) else { return };
    let opts = SubsetOptions { drop_aat: true, ..Default::default() };
    let (subset, _, report) = subset_text_with(&original, CJK_SAMPLE, &opts);
    let tables = Tables::read(&subset).unwrap();
    assert!(tables.get(b"morx").is_none() && tables.get(b"prop").is_none());
    // Without Apple's tables nothing stops the glyph count from being cut.
    assert_eq!(report.glyphs_out, *report.kept.last().unwrap() as usize + 1);
}

#[test]
fn verify_finds_a_missing_glyph() {
    let Some(original) = load("widgets/resources/Inter.ttf") else { return };
    let inputs = ClosureInputs { runs: vec![run("Hello")], ..Default::default() };
    let mut keep = closure(&original, &inputs).unwrap();
    let face = ttf_parser::Face::parse(&original, 0).unwrap();
    keep.remove(&face.glyph_index('l').unwrap().0);
    let chars: BTreeSet<char> = "Heo".chars().collect();
    let subset = subset(&original, &keep, &chars, &SubsetOptions::default()).unwrap();
    assert!(verify(&original, &subset, &inputs).is_err());
}

fn brotli(data: &[u8]) -> usize {
    use std::io::Write;
    let mut out = Vec::new();
    {
        let mut writer = ::brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        writer.write_all(data).unwrap();
    }
    out.len()
}

/// Sizes raw and brotli (quality 11): the original, the default subset,
/// the subset with every axis but wght and wdth pinned (variable fonts),
/// with Apple's layout tables dropped (fonts that have them) and the stub.
/// A measurement, not a check: run with `--ignored --nocapture`.
#[test]
#[ignore]
fn size_report() {
    let mut fonts: Vec<(&str, &str)> = FONTS.iter().map(|f| (*f, SAMPLE)).collect();
    fonts.push((CJK_FONT, CJK_SAMPLE));
    fonts.push(("widgets/resources/LXGWWenKaiBold.ttf", CJK_SAMPLE));
    for (path, text) in fonts {
        let Some(original) = load(path) else { continue };
        let line = |name: &str, data: &[u8]| eprintln!("  {name:<28} {:>9} raw {:>9} br", data.len(), brotli(data));
        eprintln!("{path}");
        line("original", &original);
        line("subset", &subset_text(&original, text).0);
        let face = ttf_parser::Face::parse(&original, 0).unwrap();
        if face.is_variable() {
            let pin_axes = face
                .variation_axes()
                .into_iter()
                .map(|a| a.tag.0)
                .filter(|t| !matches!(&t.to_be_bytes(), b"wght" | b"wdth"))
                .collect();
            line("subset, wght+wdth only", &subset_text_with(&original, text, &SubsetOptions { pin_axes, ..Default::default() }).0);
        }
        if Tables::read(&original).unwrap().get(b"morx").is_some() {
            line("subset, Apple layout dropped", &subset_text_with(&original, text, &SubsetOptions { drop_aat: true, ..Default::default() }).0);
        }
        line("stub", &stub(&original, &SubsetOptions::default()).unwrap());
    }
}
