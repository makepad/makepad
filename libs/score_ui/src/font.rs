//! Music-font loading.
//!
//! The engraver needs real SMuFL outlines, not stand-ins. This module resolves
//! one OpenType music font plus its SMuFL metadata, pulls glyph outlines out of
//! the font by canonical name, and exposes the metric surface the engraver
//! needs: glyph bounding boxes, advance widths, stem anchors, and the font's
//! `engravingDefaults`.
//!
//! # Coordinates
//!
//! Outlines stay in the font's own design units (y-up), exactly as
//! [`makepad_score_render::GlyphOutline`] expects; the renderer normalizes them
//! by `units_per_em` and multiplies by a paint item's `em_size`. Everything
//! else here is in staff spaces, y-up, relative to the glyph origin, following
//! SMuFL's rule that one em is four staff spaces.
//!
//! # Availability
//!
//! The font is looked up at runtime (see [`search_paths`]); a checkout without
//! one still starts, falling back to a small set of hand-drawn outlines fitted
//! to Bravura's own bounding boxes.

use makepad_score::{
    smufl::{FontMetadata, GlyphRegistry},
    symbol::{
        Accidental, Articulation, Clef, Digit, Direction, DynamicMark, FermataShape, FlagDuration,
        NoteheadDuration, NoteheadShape, Ornament, Placement, RestDuration, Symbol, TremoloStrokes,
    },
};
use makepad_score_render::{GlyphOutline, GlyphOutlineCommand};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

/// A glyph's ink box in staff spaces, y-up, relative to the glyph origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl GlyphBox {
    pub fn width(self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(self) -> f64 {
        self.max_y - self.min_y
    }
}

/// The font-independent engraving measurements this app consumes, in staff
/// spaces. Values are Bravura's until a font's `engravingDefaults` replaces
/// them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Engraving {
    pub staff_line_thickness: f64,
    pub stem_thickness: f64,
    pub beam_thickness: f64,
    pub beam_spacing: f64,
    pub leger_line_thickness: f64,
    pub leger_line_extension: f64,
    pub thin_barline_thickness: f64,
    pub thick_barline_thickness: f64,
    pub bracket_thickness: f64,
}

impl Default for Engraving {
    fn default() -> Self {
        Self {
            staff_line_thickness: 0.13,
            stem_thickness: 0.12,
            beam_thickness: 0.5,
            beam_spacing: 0.25,
            leger_line_thickness: 0.16,
            leger_line_extension: 0.4,
            thin_barline_thickness: 0.16,
            thick_barline_thickness: 0.5,
            bracket_thickness: 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct GlyphMetrics {
    bbox: Option<GlyphBox>,
    advance: Option<f64>,
    stem_up_se: Option<(f64, f64)>,
    stem_down_nw: Option<(f64, f64)>,
}

/// One resolved music font: outlines by canonical SMuFL name plus metrics.
pub struct MusicFont {
    source: String,
    real: bool,
    units_per_em: u16,
    engraving: Engraving,
    outlines: BTreeMap<String, GlyphOutline>,
    metrics: BTreeMap<String, GlyphMetrics>,
}

impl MusicFont {
    /// A one-line description of where the outlines came from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// False when the hand-drawn fallback is in use.
    pub fn is_real(&self) -> bool {
        self.real
    }

    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    pub fn engraving(&self) -> Engraving {
        self.engraving
    }

    pub fn outlines(&self) -> impl Iterator<Item = (&str, &GlyphOutline)> {
        self.outlines.iter().map(|(name, outline)| (name.as_str(), outline))
    }

    pub fn has(&self, name: &str) -> bool {
        self.outlines.contains_key(name)
    }

    /// Ink box in staff spaces, y-up. Missing glyphs report an empty box at the
    /// origin so callers never have to special-case a font gap.
    pub fn bbox(&self, name: &str) -> GlyphBox {
        self.metrics
            .get(name)
            .and_then(|metrics| metrics.bbox)
            .unwrap_or(GlyphBox {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 0.0,
                max_y: 0.0,
            })
    }

    /// Advance width in staff spaces, falling back to the ink width.
    pub fn advance(&self, name: &str) -> f64 {
        let metrics = self.metrics.get(name);
        metrics
            .and_then(|metrics| metrics.advance)
            .or_else(|| metrics.and_then(|metrics| metrics.bbox).map(GlyphBox::width))
            .unwrap_or(0.0)
    }

    /// Where an up-stem meets this notehead, in staff spaces from the origin.
    pub fn stem_up_se(&self, name: &str) -> (f64, f64) {
        self.metrics
            .get(name)
            .and_then(|metrics| metrics.stem_up_se)
            .unwrap_or_else(|| (self.bbox(name).max_x, 0.168))
    }

    /// Where a down-stem meets this notehead, in staff spaces from the origin.
    pub fn stem_down_nw(&self, name: &str) -> (f64, f64) {
        self.metrics
            .get(name)
            .and_then(|metrics| metrics.stem_down_nw)
            .unwrap_or_else(|| (self.bbox(name).min_x, -0.168))
    }
}

/// The process-wide music font, loaded once on first use.
/// The font's identity without the absolute path — a dialog line, not a log.
pub fn music_font_summary() -> String {
    let source = music_font().source();
    match source.split_once(" from ") {
        Some((name, _path)) => name.to_string(),
        None => source.to_string(),
    }
}

/// A music font compiled into the application, used when no file is found.
///
/// The search paths come first, so a reader who wants a different SMuFL font
/// still gets one by pointing `MAKEPAD_SCORE_MUSIC_FONT` at it. This is the
/// floor: an application that ships its notation font renders notation
/// wherever it is run from, rather than only inside a checkout that happens to
/// have the font lying beside it.
static EMBEDDED: OnceLock<EmbeddedFont> = OnceLock::new();

pub struct EmbeddedFont {
    pub name: &'static str,
    pub otf: &'static [u8],
    pub metadata: Option<&'static [u8]>,
    pub glyphnames: Option<&'static [u8]>,
}

/// Register the font the binary carries. Call before the first draw; later
/// calls are ignored, because the font is resolved once.
pub fn set_embedded_music_font(font: EmbeddedFont) {
    let _ = EMBEDDED.set(font);
}

pub fn music_font() -> &'static MusicFont {
    static FONT: OnceLock<MusicFont> = OnceLock::new();
    FONT.get_or_init(|| {
        let font = load_music_font();
        // One line, whichever way it went: a missing font is a degraded look,
        // never a failed start.
        println!("[score] music font: {}", font.source);
        font
    })
}

/// Where a music font is looked for, in order:
///
/// 1. `$MAKEPAD_SCORE_MUSIC_FONT` — a full path to an `.otf`/`.ttf`.
/// 2. `$MAKEPAD_SCORE_FONT_DIR`, then a `resources/fonts` directory beside the
///    executable (including the macOS `../Resources/fonts` bundle location).
/// 3. `local/score-corpus/fonts` in the development checkout, found by walking
///    up from both the working directory and the executable.
fn search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("MAKEPAD_SCORE_MUSIC_FONT") {
        paths.push(PathBuf::from(path));
    }
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Some(directory) = std::env::var_os("MAKEPAD_SCORE_FONT_DIR") {
        directories.push(PathBuf::from(directory));
    }
    let exe = std::env::current_exe().ok();
    if let Some(beside) = exe.as_ref().and_then(|exe| exe.parent()) {
        directories.push(beside.join("resources/fonts"));
        directories.push(beside.join("../Resources/fonts"));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        roots.extend(current.ancestors().take(6).map(Path::to_path_buf));
    }
    if let Some(beside) = exe.as_ref().and_then(|exe| exe.parent()) {
        roots.extend(beside.ancestors().take(6).map(Path::to_path_buf));
    }
    for root in roots {
        directories.push(root.join("resources/fonts"));
        directories.push(root.join("local/score-corpus/fonts"));
    }
    for directory in directories {
        for name in ["bravura.otf", "Bravura.otf", "bravura.ttf", "Bravura.ttf"] {
            paths.push(directory.join(name));
        }
    }
    paths
}

fn load_music_font() -> MusicFont {
    for path in search_paths() {
        if !path.is_file() {
            continue;
        }
        match load_from_file(&path) {
            Ok(font) => return font,
            Err(reason) => {
                println!("[score] music font at {} unusable: {reason}", path.display());
            }
        }
    }
    if let Some(embedded) = EMBEDDED.get() {
        match load_from_bytes(
            embedded.otf,
            embedded.metadata,
            embedded.glyphnames,
            &format!("{} (built in)", embedded.name),
        ) {
            Ok(font) => return font,
            Err(reason) => println!("[score] built-in music font unusable: {reason}"),
        }
    }
    fallback_font()
}

fn load_from_file(path: &Path) -> Result<MusicFont, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let registry = read_json(&metadata_candidates(path, "glyphnames.json"));
    let metadata = read_json(&metadata_candidates(path, "metadata.json"));
    load_font(
        &bytes,
        metadata.as_deref(),
        registry.as_deref(),
        path.parent().unwrap_or_else(|| Path::new(".")),
        &path.display().to_string(),
    )
}

/// The same load, from bytes the binary carries rather than a file.
fn load_from_bytes(
    otf: &[u8],
    metadata: Option<&[u8]>,
    glyphnames: Option<&[u8]>,
    source: &str,
) -> Result<MusicFont, String> {
    load_font(otf, metadata, glyphnames, Path::new("."), source)
}

fn load_font(
    bytes: &[u8],
    metadata_json: Option<&[u8]>,
    glyphnames_json: Option<&[u8]>,
    directory: &Path,
    source: &str,
) -> Result<MusicFont, String> {
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|error| error.to_string())?;
    let units_per_em = face.units_per_em();
    if units_per_em == 0 {
        return Err("font has a zero-sized em square".into());
    }

    let registry = glyphnames_json.and_then(|bytes| GlyphRegistry::from_bytes(bytes).ok());
    let metadata = metadata_json.and_then(|bytes| FontMetadata::from_bytes(bytes).ok());

    let mut outlines = BTreeMap::new();
    let mut metrics: BTreeMap<String, GlyphMetrics> = BTreeMap::new();
    for name in repertoire() {
        let codepoint = registry
            .as_ref()
            .and_then(|registry| registry.codepoint_for_name(&name));
        let glyph = codepoint
            .and_then(|codepoint| face.glyph_index(codepoint))
            .or_else(|| face.glyph_index_by_name(&name));
        let Some(glyph) = glyph else { continue };
        let mut builder = OutlineCollector::default();
        if face.outline_glyph(glyph, &mut builder).is_none() || builder.commands.is_empty() {
            continue;
        }
        let entry = metrics.entry(name.clone()).or_default();
        // Prefer the font metadata's published box; otherwise measure the ink.
        entry.bbox = Some(builder.bounds(units_per_em));
        entry.advance = face
            .glyph_hor_advance(glyph)
            .map(|advance| f64::from(advance) * 4.0 / f64::from(units_per_em));
        outlines.insert(
            name,
            GlyphOutline {
                units_per_em,
                commands: Arc::from(builder.commands),
            },
        );
    }
    if outlines.is_empty() {
        return Err("no SMuFL glyphs found in the font".into());
    }

    let mut engraving = Engraving::default();
    if let Some(metadata) = &metadata {
        let defaults = &metadata.engraving_defaults;
        engraving = Engraving {
            staff_line_thickness: defaults.staff_line_thickness.get(),
            stem_thickness: defaults.stem_thickness.get(),
            beam_thickness: defaults.beam_thickness.get(),
            beam_spacing: defaults.beam_spacing.get(),
            leger_line_thickness: defaults.leger_line_thickness.get(),
            leger_line_extension: defaults.leger_line_extension.get(),
            thin_barline_thickness: defaults.thin_barline_thickness.get(),
            thick_barline_thickness: defaults.thick_barline_thickness.get(),
            bracket_thickness: defaults.bracket_thickness.get(),
        };
        for (name, entry) in metrics.iter_mut() {
            if let Some(bbox) = metadata.glyph_bboxes.get(name) {
                entry.bbox = Some(GlyphBox {
                    min_x: bbox.south_west.x.get(),
                    min_y: bbox.south_west.y.get(),
                    max_x: bbox.north_east.x.get(),
                    max_y: bbox.north_east.y.get(),
                });
            }
            if let Some(advance) = metadata.glyph_advance_widths.get(name) {
                entry.advance = Some(advance.get());
            }
            if let Some(anchors) = metadata.glyphs_with_anchors.get(name) {
                entry.stem_up_se = anchors
                    .stem_up_se
                    .map(|point| (point.x.get(), point.y.get()));
                entry.stem_down_nw = anchors
                    .stem_down_nw
                    .map(|point| (point.x.get(), point.y.get()));
            }
        }
    }

    let font_name = metadata
        .as_ref()
        .and_then(|metadata| metadata.font_name.clone())
        .unwrap_or_else(|| "music font".to_string());
    let _ = directory;
    Ok(MusicFont {
        source: format!(
            "{font_name} ({} glyphs, upem {units_per_em}) from {source}{}",
            outlines.len(),
            if metadata.is_some() {
                ""
            } else {
                " [no metadata json; using built-in engraving defaults]"
            }
        ),
        real: true,
        units_per_em,
        engraving,
        outlines,
        metrics,
    })
}

/// Metadata lives beside the font: `bravura.otf` -> `bravura_metadata.json`.
fn metadata_candidates(font: &Path, suffix: &str) -> Vec<PathBuf> {
    let directory = font.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let stem = font
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut candidates = Vec::new();
    if suffix == "metadata.json" {
        candidates.push(directory.join(format!("{stem}_metadata.json")));
        candidates.push(directory.join("metadata.json"));
    } else {
        candidates.push(directory.join(suffix));
    }
    candidates
}

fn read_json(candidates: &[PathBuf]) -> Option<Vec<u8>> {
    candidates
        .iter()
        .find_map(|path| std::fs::read(path).ok())
}

#[derive(Default)]
struct OutlineCollector {
    commands: Vec<GlyphOutlineCommand>,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    started: bool,
}

impl OutlineCollector {
    fn include(&mut self, x: f32, y: f32) {
        if !self.started {
            self.min_x = x;
            self.min_y = y;
            self.max_x = x;
            self.max_y = y;
            self.started = true;
            return;
        }
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    fn bounds(&self, units_per_em: u16) -> GlyphBox {
        let scale = 4.0 / f64::from(units_per_em);
        GlyphBox {
            min_x: f64::from(self.min_x) * scale,
            min_y: f64::from(self.min_y) * scale,
            max_x: f64::from(self.max_x) * scale,
            max_y: f64::from(self.max_y) * scale,
        }
    }
}

impl ttf_parser::OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.include(x, y);
        self.commands.push(GlyphOutlineCommand::MoveTo(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.include(x, y);
        self.commands.push(GlyphOutlineCommand::LineTo(x, y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.include(x, y);
        self.commands.push(GlyphOutlineCommand::QuadTo(cx, cy, x, y));
    }

    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.include(x, y);
        self.commands
            .push(GlyphOutlineCommand::CubicTo(c1x, c1y, c2x, c2y, x, y));
    }

    fn close(&mut self) {
        self.commands.push(GlyphOutlineCommand::Close);
    }
}

/// The working repertoire, as canonical SMuFL names derived from [`Symbol`].
fn repertoire() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut push = |symbol: Symbol| names.push(symbol.canonical_name().to_string());

    const NOTEHEAD_DURATIONS: [NoteheadDuration; 4] = [
        NoteheadDuration::DoubleWhole,
        NoteheadDuration::Whole,
        NoteheadDuration::Half,
        NoteheadDuration::Black,
    ];
    const NOTEHEAD_SHAPES: [NoteheadShape; 4] = [
        NoteheadShape::Normal,
        NoteheadShape::X,
        NoteheadShape::Diamond,
        NoteheadShape::Slash,
    ];
    for shape in NOTEHEAD_SHAPES {
        for duration in NOTEHEAD_DURATIONS {
            push(Symbol::Notehead { duration, shape });
        }
    }
    for duration in [
        RestDuration::Maxima,
        RestDuration::Longa,
        RestDuration::DoubleWhole,
        RestDuration::Whole,
        RestDuration::Half,
        RestDuration::Quarter,
        RestDuration::Eighth,
        RestDuration::Sixteenth,
        RestDuration::ThirtySecond,
        RestDuration::SixtyFourth,
        RestDuration::OneTwentyEighth,
    ] {
        push(Symbol::Rest(duration));
    }
    for accidental in [
        Accidental::TripleFlat,
        Accidental::DoubleFlat,
        Accidental::Flat,
        Accidental::Natural,
        Accidental::Sharp,
        Accidental::DoubleSharp,
        Accidental::TripleSharp,
        Accidental::NaturalFlat,
        Accidental::NaturalSharp,
        Accidental::QuarterToneFlat,
        Accidental::ThreeQuarterTonesFlat,
        Accidental::QuarterToneSharp,
        Accidental::ThreeQuarterTonesSharp,
    ] {
        push(Symbol::Accidental(accidental));
    }
    for clef in [
        Clef::G,
        Clef::G8va,
        Clef::G8vb,
        Clef::G15ma,
        Clef::G15mb,
        Clef::F,
        Clef::F8va,
        Clef::F8vb,
        Clef::F15ma,
        Clef::F15mb,
        Clef::C,
        Clef::Percussion,
        Clef::PercussionAlternate,
        Clef::Tab4String,
        Clef::Tab6String,
    ] {
        push(Symbol::Clef(clef));
    }
    for duration in [
        FlagDuration::Eighth,
        FlagDuration::Sixteenth,
        FlagDuration::ThirtySecond,
        FlagDuration::SixtyFourth,
        FlagDuration::OneTwentyEighth,
    ] {
        for direction in [Direction::Up, Direction::Down] {
            push(Symbol::Flag {
                duration,
                direction,
            });
        }
    }
    for articulation in [
        Articulation::Accent,
        Articulation::Staccato,
        Articulation::Tenuto,
        Articulation::Staccatissimo,
        Articulation::Marcato,
        Articulation::LaissezVibrer,
        Articulation::Stress,
        Articulation::SoftAccent,
        Articulation::AccentStaccato,
        Articulation::TenutoStaccato,
        Articulation::MarcatoStaccato,
        Articulation::MarcatoTenuto,
    ] {
        for placement in [Placement::Above, Placement::Below] {
            push(Symbol::Articulation {
                articulation,
                placement,
            });
        }
    }
    for dynamic in [
        DynamicMark::Piano,
        DynamicMark::Pianissimo,
        DynamicMark::Pianississimo,
        DynamicMark::Pianissississimo,
        DynamicMark::MezzoPiano,
        DynamicMark::MezzoForte,
        DynamicMark::Forte,
        DynamicMark::Fortissimo,
        DynamicMark::Fortississimo,
        DynamicMark::Fortissississimo,
        DynamicMark::FortePiano,
        DynamicMark::Sforzando,
        DynamicMark::SforzandoPiano,
        DynamicMark::Sforzato,
        DynamicMark::Rinforzando,
        DynamicMark::Niente,
        DynamicMark::Mezzo,
        DynamicMark::Z,
    ] {
        push(Symbol::Dynamic(dynamic));
    }
    const DIGITS: [Digit; 10] = [
        Digit::Zero,
        Digit::One,
        Digit::Two,
        Digit::Three,
        Digit::Four,
        Digit::Five,
        Digit::Six,
        Digit::Seven,
        Digit::Eight,
        Digit::Nine,
    ];
    for digit in DIGITS {
        push(Symbol::TimeSignatureDigit(digit));
        push(Symbol::TupletDigit(digit));
    }
    push(Symbol::TimeSignatureCommon);
    push(Symbol::TimeSignatureCutCommon);
    for ornament in [
        Ornament::Trill,
        Ornament::Turn,
        Ornament::InvertedTurn,
        Ornament::TurnWithSlash,
        Ornament::Mordent,
        Ornament::ShortTrill,
        Ornament::Tremblement,
        Ornament::Schleifer,
    ] {
        push(Symbol::Ornament(ornament));
    }
    for shape in [
        FermataShape::Normal,
        FermataShape::Short,
        FermataShape::Long,
        FermataShape::VeryShort,
        FermataShape::VeryLong,
    ] {
        for placement in [Placement::Above, Placement::Below] {
            push(Symbol::Fermata { shape, placement });
        }
    }
    for strokes in [
        TremoloStrokes::One,
        TremoloStrokes::Two,
        TremoloStrokes::Three,
        TremoloStrokes::Four,
        TremoloStrokes::Five,
    ] {
        push(Symbol::Tremolo(strokes));
    }
    push(Symbol::AugmentationDot);
    push(Symbol::RepeatDot);
    push(Symbol::Segno);
    push(Symbol::Coda);
    push(Symbol::BreathMark);
    push(Symbol::Caesura);
    push(Symbol::Arpeggio(Direction::Up));
    push(Symbol::Arpeggio(Direction::Down));
    for extra in ["brace", "bracket", "restHBar", "noteheadWholeFilled"] {
        names.push(extra.to_string());
    }
    names.sort();
    names.dedup();
    names
}

// ---------------------------------------------------------------------------
// Fallback: hand-drawn outlines, fitted to Bravura's published bounding boxes
// so a checkout without a music font still engraves at the right size.
// ---------------------------------------------------------------------------

const FALLBACK_UNITS_PER_EM: u16 = 1000;

fn fallback_font() -> MusicFont {
    let mut outlines = BTreeMap::new();
    let mut metrics = BTreeMap::new();
    for (name, commands, bbox) in [
        (
            "noteheadBlack",
            notehead_shape(),
            GlyphBox {
                min_x: 0.0,
                min_y: -0.5,
                max_x: 1.18,
                max_y: 0.5,
            },
        ),
        (
            "noteheadHalf",
            notehead_shape(),
            GlyphBox {
                min_x: 0.0,
                min_y: -0.5,
                max_x: 1.18,
                max_y: 0.5,
            },
        ),
        (
            "noteheadWhole",
            notehead_shape(),
            GlyphBox {
                min_x: 0.0,
                min_y: -0.5,
                max_x: 1.688,
                max_y: 0.5,
            },
        ),
        (
            "augmentationDot",
            dot_shape(),
            GlyphBox {
                min_x: 0.0,
                min_y: -0.1,
                max_x: 0.2,
                max_y: 0.1,
            },
        ),
        (
            "gClef",
            g_clef_shape(),
            GlyphBox {
                min_x: 0.0,
                min_y: -2.632,
                max_x: 2.684,
                max_y: 4.392,
            },
        ),
        (
            "fClef",
            f_clef_shape(),
            GlyphBox {
                min_x: 0.0,
                min_y: -1.0,
                max_x: 2.736,
                max_y: 2.72,
            },
        ),
    ] {
        outlines.insert(name.to_string(), fit_outline(commands, bbox));
        metrics.insert(
            name.to_string(),
            GlyphMetrics {
                bbox: Some(bbox),
                advance: Some(bbox.width()),
                stem_up_se: Some((bbox.max_x, 0.168)),
                stem_down_nw: Some((bbox.min_x, -0.168)),
            },
        );
    }
    MusicFont {
        source: "hand-drawn fallback outlines (no SMuFL font found; \
                 set MAKEPAD_SCORE_MUSIC_FONT or place bravura.otf in resources/fonts)"
            .to_string(),
        real: false,
        units_per_em: FALLBACK_UNITS_PER_EM,
        engraving: Engraving::default(),
        outlines,
        metrics,
    }
}

/// Maps a hand-drawn path onto a target staff-space box, so the fallback lands
/// at exactly the size the engraver expects of the real glyph.
fn fit_outline(commands: Vec<GlyphOutlineCommand>, target: GlyphBox) -> GlyphOutline {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut visit = |x: f32, y: f32| {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    };
    for command in &commands {
        match *command {
            GlyphOutlineCommand::MoveTo(x, y) | GlyphOutlineCommand::LineTo(x, y) => visit(x, y),
            GlyphOutlineCommand::QuadTo(_, _, x, y) => visit(x, y),
            GlyphOutlineCommand::CubicTo(_, _, _, _, x, y) => visit(x, y),
            GlyphOutlineCommand::Close => {}
        }
    }
    let units = f32::from(FALLBACK_UNITS_PER_EM) / 4.0;
    let scale_x = if max_x > min_x {
        (target.width() as f32) * units / (max_x - min_x)
    } else {
        1.0
    };
    let scale_y = if max_y > min_y {
        (target.height() as f32) * units / (max_y - min_y)
    } else {
        1.0
    };
    let map = |x: f32, y: f32| {
        (
            (x - min_x) * scale_x + target.min_x as f32 * units,
            (y - min_y) * scale_y + target.min_y as f32 * units,
        )
    };
    let mapped = commands
        .into_iter()
        .map(|command| match command {
            GlyphOutlineCommand::MoveTo(x, y) => {
                let (x, y) = map(x, y);
                GlyphOutlineCommand::MoveTo(x, y)
            }
            GlyphOutlineCommand::LineTo(x, y) => {
                let (x, y) = map(x, y);
                GlyphOutlineCommand::LineTo(x, y)
            }
            GlyphOutlineCommand::QuadTo(cx, cy, x, y) => {
                let (cx, cy) = map(cx, cy);
                let (x, y) = map(x, y);
                GlyphOutlineCommand::QuadTo(cx, cy, x, y)
            }
            GlyphOutlineCommand::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                let (c1x, c1y) = map(c1x, c1y);
                let (c2x, c2y) = map(c2x, c2y);
                let (x, y) = map(x, y);
                GlyphOutlineCommand::CubicTo(c1x, c1y, c2x, c2y, x, y)
            }
            GlyphOutlineCommand::Close => GlyphOutlineCommand::Close,
        })
        .collect::<Vec<_>>();
    GlyphOutline {
        units_per_em: FALLBACK_UNITS_PER_EM,
        commands: Arc::from(mapped),
    }
}

fn notehead_shape() -> Vec<GlyphOutlineCommand> {
    use GlyphOutlineCommand::*;
    vec![
        MoveTo(-520.0, -90.0),
        CubicTo(-430.0, 260.0, 180.0, 430.0, 470.0, 190.0),
        CubicTo(760.0, -50.0, 470.0, -390.0, 20.0, -420.0),
        CubicTo(-410.0, -450.0, -610.0, -280.0, -520.0, -90.0),
        Close,
    ]
}

fn dot_shape() -> Vec<GlyphOutlineCommand> {
    use GlyphOutlineCommand::*;
    vec![
        MoveTo(-180.0, 0.0),
        CubicTo(-180.0, 110.0, -100.0, 180.0, 0.0, 180.0),
        CubicTo(110.0, 180.0, 180.0, 100.0, 180.0, 0.0),
        CubicTo(180.0, -110.0, 100.0, -180.0, 0.0, -180.0),
        CubicTo(-110.0, -180.0, -180.0, -100.0, -180.0, 0.0),
        Close,
    ]
}

fn g_clef_shape() -> Vec<GlyphOutlineCommand> {
    use GlyphOutlineCommand::*;
    vec![
        MoveTo(80.0, 660.0),
        CubicTo(-300.0, 450.0, -360.0, 80.0, -80.0, -110.0),
        CubicTo(210.0, -305.0, 490.0, -100.0, 335.0, 125.0),
        CubicTo(215.0, 300.0, -30.0, 230.0, -35.0, 75.0),
        CubicTo(-35.0, -20.0, 85.0, -55.0, 145.0, 15.0),
        CubicTo(280.0, 175.0, 70.0, 305.0, -95.0, 220.0),
        CubicTo(-360.0, 85.0, -270.0, -300.0, 65.0, -350.0),
        LineTo(115.0, -800.0),
        LineTo(245.0, -790.0),
        LineTo(180.0, -335.0),
        CubicTo(540.0, -210.0, 560.0, 250.0, 230.0, 430.0),
        CubicTo(145.0, 480.0, 120.0, 590.0, 80.0, 660.0),
        Close,
    ]
}

fn f_clef_shape() -> Vec<GlyphOutlineCommand> {
    use GlyphOutlineCommand::*;
    vec![
        MoveTo(-430.0, 240.0),
        CubicTo(-210.0, 570.0, 330.0, 470.0, 390.0, 90.0),
        CubicTo(450.0, -300.0, 120.0, -550.0, -260.0, -430.0),
        CubicTo(20.0, -300.0, 160.0, -90.0, 105.0, 120.0),
        CubicTo(45.0, 340.0, -190.0, 390.0, -430.0, 240.0),
        Close,
        MoveTo(560.0, 210.0),
        LineTo(760.0, 210.0),
        LineTo(760.0, 410.0),
        LineTo(560.0, 410.0),
        Close,
        MoveTo(560.0, -190.0),
        LineTo(760.0, -190.0),
        LineTo(760.0, 10.0),
        LineTo(560.0, 10.0),
        Close,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repertoire_covers_the_working_symbols() {
        let names = repertoire();
        for expected in [
            "noteheadBlack",
            "noteheadHalf",
            "noteheadWhole",
            "restQuarter",
            "rest8th",
            "accidentalSharp",
            "accidentalFlat",
            "accidentalNatural",
            "accidentalDoubleSharp",
            "gClef",
            "fClef",
            "cClef",
            "gClef8vb",
            "flag8thUp",
            "flag32ndDown",
            "augmentationDot",
            "timeSig4",
            "articStaccatoAbove",
            "articAccentBelow",
            "fermataAbove",
        ] {
            assert!(names.iter().any(|name| name == expected), "missing {expected}");
        }
    }

    #[test]
    fn fallback_notehead_is_one_staff_space_tall() {
        let font = fallback_font();
        let bbox = font.bbox("noteheadBlack");
        assert!((bbox.height() - 1.0).abs() < 1e-9);
        assert!((bbox.width() - 1.18).abs() < 1e-9);
        let outline = font.outlines.get("noteheadBlack").unwrap();
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for command in outline.commands.iter() {
            if let GlyphOutlineCommand::CubicTo(_, _, _, _, _, y) | GlyphOutlineCommand::MoveTo(_, y) =
                command
            {
                min_y = min_y.min(*y);
                max_y = max_y.max(*y);
            }
        }
        // 1000 units per em, four staff spaces to the em: 250 units tall.
        assert!((max_y - min_y - 250.0).abs() < 1.0, "{min_y}..{max_y}");
    }

    #[test]
    fn the_shipped_font_loads_when_present() {
        let font = load_music_font();
        println!("music font source: {}", font.source());
        if !font.is_real() {
            return;
        }
        let bbox = font.bbox("noteheadBlack");
        assert!((bbox.height() - 1.0).abs() < 0.02, "{bbox:?}");
        assert!((bbox.width() - 1.18).abs() < 0.05, "{bbox:?}");
        assert!(font.has("restQuarter"));
        assert!(font.has("accidentalSharp"));
        assert!(font.has("flag8thUp"));
        assert!(font.engraving().beam_thickness > 0.0);
    }
}
