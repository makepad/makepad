use super::json::{
    array, child_path, codepoint, field, index_path, number, object, optional_string,
    optional_string_array, parse, point, root_object, staff_spaces, string, string_array, Object,
};
use super::SmuflResult;
use crate::units::{StaffPoint, StaffSpaces};
use makepad_micro_serde::JsonValue;
use std::collections::HashMap;

/// Font-independent engraving measurements from `engravingDefaults`.
///
/// Every dimensional value is represented in staff spaces. The field set is
/// the complete set used by the supplied SMuFL metadata, while numeric fields
/// introduced by later revisions are retained in [`Self::other_measurements`].
#[derive(Clone, Debug, PartialEq)]
pub struct EngravingDefaults {
    pub staff_line_thickness: StaffSpaces,
    pub stem_thickness: StaffSpaces,
    pub beam_thickness: StaffSpaces,
    pub beam_spacing: StaffSpaces,
    pub leger_line_thickness: StaffSpaces,
    pub leger_line_extension: StaffSpaces,
    pub slur_endpoint_thickness: StaffSpaces,
    pub slur_midpoint_thickness: StaffSpaces,
    pub tie_endpoint_thickness: StaffSpaces,
    pub tie_midpoint_thickness: StaffSpaces,
    pub thin_barline_thickness: StaffSpaces,
    pub thick_barline_thickness: StaffSpaces,
    pub barline_separation: StaffSpaces,
    pub repeat_barline_dot_separation: StaffSpaces,
    pub bracket_thickness: StaffSpaces,
    pub sub_bracket_thickness: StaffSpaces,
    pub hairpin_thickness: StaffSpaces,
    pub octave_line_thickness: StaffSpaces,
    pub pedal_line_thickness: StaffSpaces,
    pub tuplet_bracket_thickness: StaffSpaces,
    pub text_enclosure_thickness: StaffSpaces,
    pub arrow_shaft_thickness: StaffSpaces,
    pub h_bar_thickness: StaffSpaces,
    pub dashed_barline_thickness: StaffSpaces,
    pub dashed_barline_dash_length: StaffSpaces,
    pub dashed_barline_gap_length: StaffSpaces,
    pub lyric_line_thickness: StaffSpaces,
    pub repeat_ending_line_thickness: StaffSpaces,
    pub thin_thick_barline_separation: StaffSpaces,
    pub text_font_family: Vec<String>,
    pub other_measurements: HashMap<String, StaffSpaces>,
}

impl EngravingDefaults {
    fn parse(value: &JsonValue) -> SmuflResult<Self> {
        const PATH: &str = "fontMetadata.engravingDefaults";
        let object = object(value, PATH)?;

        macro_rules! measure {
            ($name:literal) => {
                staff_spaces(field(object, $name, PATH)?, &child_path(PATH, $name))?
            };
        }

        let mut other_measurements = HashMap::new();
        for (name, value) in object {
            if is_known_engraving_field(name) {
                continue;
            }
            if let Ok(value) = staff_spaces(value, &child_path(PATH, name)) {
                other_measurements.insert(name.clone(), value);
            }
        }

        Ok(Self {
            staff_line_thickness: measure!("staffLineThickness"),
            stem_thickness: measure!("stemThickness"),
            beam_thickness: measure!("beamThickness"),
            beam_spacing: measure!("beamSpacing"),
            leger_line_thickness: measure!("legerLineThickness"),
            leger_line_extension: measure!("legerLineExtension"),
            slur_endpoint_thickness: measure!("slurEndpointThickness"),
            slur_midpoint_thickness: measure!("slurMidpointThickness"),
            tie_endpoint_thickness: measure!("tieEndpointThickness"),
            tie_midpoint_thickness: measure!("tieMidpointThickness"),
            thin_barline_thickness: measure!("thinBarlineThickness"),
            thick_barline_thickness: measure!("thickBarlineThickness"),
            barline_separation: measure!("barlineSeparation"),
            repeat_barline_dot_separation: measure!("repeatBarlineDotSeparation"),
            bracket_thickness: measure!("bracketThickness"),
            sub_bracket_thickness: measure!("subBracketThickness"),
            hairpin_thickness: measure!("hairpinThickness"),
            octave_line_thickness: measure!("octaveLineThickness"),
            pedal_line_thickness: measure!("pedalLineThickness"),
            tuplet_bracket_thickness: measure!("tupletBracketThickness"),
            text_enclosure_thickness: measure!("textEnclosureThickness"),
            arrow_shaft_thickness: measure!("arrowShaftThickness"),
            h_bar_thickness: measure!("hBarThickness"),
            dashed_barline_thickness: measure!("dashedBarlineThickness"),
            dashed_barline_dash_length: measure!("dashedBarlineDashLength"),
            dashed_barline_gap_length: measure!("dashedBarlineGapLength"),
            lyric_line_thickness: measure!("lyricLineThickness"),
            repeat_ending_line_thickness: measure!("repeatEndingLineThickness"),
            thin_thick_barline_separation: measure!("thinThickBarlineSeparation"),
            text_font_family: string_array(
                field(object, "textFontFamily", PATH)?,
                &child_path(PATH, "textFontFamily"),
            )?,
            other_measurements,
        })
    }
}

fn is_known_engraving_field(name: &str) -> bool {
    matches!(
        name,
        "staffLineThickness"
            | "stemThickness"
            | "beamThickness"
            | "beamSpacing"
            | "legerLineThickness"
            | "legerLineExtension"
            | "slurEndpointThickness"
            | "slurMidpointThickness"
            | "tieEndpointThickness"
            | "tieMidpointThickness"
            | "thinBarlineThickness"
            | "thickBarlineThickness"
            | "barlineSeparation"
            | "repeatBarlineDotSeparation"
            | "bracketThickness"
            | "subBracketThickness"
            | "hairpinThickness"
            | "octaveLineThickness"
            | "pedalLineThickness"
            | "tupletBracketThickness"
            | "textEnclosureThickness"
            | "arrowShaftThickness"
            | "hBarThickness"
            | "dashedBarlineThickness"
            | "dashedBarlineDashLength"
            | "dashedBarlineGapLength"
            | "lyricLineThickness"
            | "repeatEndingLineThickness"
            | "thinThickBarlineSeparation"
            | "textFontFamily"
    )
}

/// All standard anchor types carried by one glyph.
///
/// Missing anchors are `None`. Unknown point-valued anchors are retained in
/// `other`, so a new SMuFL revision does not require a parser update merely to
/// preserve data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphAnchors {
    pub stem_up_se: Option<StaffPoint>,
    pub stem_up_nw: Option<StaffPoint>,
    pub stem_down_nw: Option<StaffPoint>,
    pub stem_down_sw: Option<StaffPoint>,
    pub split_stem_up_se: Option<StaffPoint>,
    pub split_stem_up_sw: Option<StaffPoint>,
    pub split_stem_down_ne: Option<StaffPoint>,
    pub split_stem_down_nw: Option<StaffPoint>,
    pub cut_out_ne: Option<StaffPoint>,
    pub cut_out_nw: Option<StaffPoint>,
    pub cut_out_se: Option<StaffPoint>,
    pub cut_out_sw: Option<StaffPoint>,
    pub optical_center: Option<StaffPoint>,
    pub numeral_top: Option<StaffPoint>,
    pub numeral_bottom: Option<StaffPoint>,
    pub grace_note_slash_ne: Option<StaffPoint>,
    pub grace_note_slash_nw: Option<StaffPoint>,
    pub grace_note_slash_se: Option<StaffPoint>,
    pub grace_note_slash_sw: Option<StaffPoint>,
    pub grace_note_slash_stem_up: Option<StaffPoint>,
    pub grace_note_slash_stem_down: Option<StaffPoint>,
    pub repeat_offset: Option<StaffPoint>,
    pub notehead_origin: Option<StaffPoint>,
    pub mark_0: Option<StaffPoint>,
    pub mark_1: Option<StaffPoint>,
    pub mark_2: Option<StaffPoint>,
    pub other: HashMap<String, StaffPoint>,
}

impl GlyphAnchors {
    fn parse(value: &JsonValue, path: &str) -> SmuflResult<Self> {
        let object = object(value, path)?;
        let mut anchors = Self::default();
        for (name, value) in object {
            let anchor_path = child_path(path, name);
            let destination = match name.as_str() {
                "stemUpSE" => &mut anchors.stem_up_se,
                "stemUpNW" => &mut anchors.stem_up_nw,
                "stemDownNW" => &mut anchors.stem_down_nw,
                "stemDownSW" => &mut anchors.stem_down_sw,
                "splitStemUpSE" => &mut anchors.split_stem_up_se,
                "splitStemUpSW" => &mut anchors.split_stem_up_sw,
                "splitStemDownNE" => &mut anchors.split_stem_down_ne,
                "splitStemDownNW" => &mut anchors.split_stem_down_nw,
                "cutOutNE" => &mut anchors.cut_out_ne,
                "cutOutNW" => &mut anchors.cut_out_nw,
                "cutOutSE" => &mut anchors.cut_out_se,
                "cutOutSW" => &mut anchors.cut_out_sw,
                "opticalCenter" => &mut anchors.optical_center,
                "numeralTop" => &mut anchors.numeral_top,
                "numeralBottom" => &mut anchors.numeral_bottom,
                "graceNoteSlashNE" => &mut anchors.grace_note_slash_ne,
                "graceNoteSlashNW" => &mut anchors.grace_note_slash_nw,
                "graceNoteSlashSE" => &mut anchors.grace_note_slash_se,
                "graceNoteSlashSW" => &mut anchors.grace_note_slash_sw,
                "graceNoteSlashStemUp" => &mut anchors.grace_note_slash_stem_up,
                "graceNoteSlashStemDown" => &mut anchors.grace_note_slash_stem_down,
                "repeatOffset" => &mut anchors.repeat_offset,
                "noteheadOrigin" => &mut anchors.notehead_origin,
                "mark0" => &mut anchors.mark_0,
                "mark1" => &mut anchors.mark_1,
                "mark2" => &mut anchors.mark_2,
                _ => {
                    if let Ok(value) = point(value, &anchor_path) {
                        anchors.other.insert(name.clone(), value);
                    }
                    continue;
                }
            };
            *destination = Some(point(value, &anchor_path)?);
        }
        Ok(anchors)
    }
}

/// The axis-aligned outline bounds of a glyph in staff-space coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphBBox {
    pub north_east: StaffPoint,
    pub south_west: StaffPoint,
}

impl GlyphBBox {
    fn parse(value: &JsonValue, path: &str) -> SmuflResult<Self> {
        let object = object(value, path)?;
        Ok(Self {
            north_east: point(field(object, "bBoxNE", path)?, &child_path(path, "bBoxNE"))?,
            south_west: point(field(object, "bBoxSW", path)?, &child_path(path, "bBoxSW"))?,
        })
    }

    pub fn width(self) -> StaffSpaces {
        self.north_east.x - self.south_west.x
    }

    pub fn height(self) -> StaffSpaces {
        self.north_east.y - self.south_west.y
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphAlternate {
    pub name: String,
    pub codepoint: char,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphAlternates {
    pub alternates: Vec<GlyphAlternate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphLigature {
    pub codepoint: char,
    pub component_glyphs: Vec<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionalGlyph {
    pub codepoint: char,
    pub description: Option<String>,
    pub classes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetGlyph {
    pub name: String,
    pub codepoint: char,
    pub alternate_for: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphSet {
    pub description: Option<String>,
    pub glyphs: Vec<SetGlyph>,
}

/// Typed, font-agnostic contents of a SMuFL font metadata document.
#[derive(Clone, Debug, PartialEq)]
pub struct FontMetadata {
    pub font_name: Option<String>,
    pub font_version: Option<f64>,
    pub engraving_defaults: EngravingDefaults,
    pub glyph_advance_widths: HashMap<String, StaffSpaces>,
    pub glyph_bboxes: HashMap<String, GlyphBBox>,
    pub glyphs_with_anchors: HashMap<String, GlyphAnchors>,
    pub glyphs_with_alternates: HashMap<String, GlyphAlternates>,
    pub ligatures: HashMap<String, GlyphLigature>,
    pub optional_glyphs: HashMap<String, OptionalGlyph>,
    pub sets: HashMap<String, GlyphSet>,
}

impl FontMetadata {
    /// Loads a SMuFL font metadata document from caller-owned bytes.
    pub fn from_bytes(bytes: &[u8]) -> SmuflResult<Self> {
        const ROOT: &str = "fontMetadata";
        let value = parse(bytes)?;
        let root = root_object(&value, ROOT)?;
        let engraving_defaults = EngravingDefaults::parse(field(root, "engravingDefaults", ROOT)?)?;
        let font_name = optional_string(root, "fontName", ROOT)?;
        let font_version = root
            .get("fontVersion")
            .filter(|value| !matches!(value, JsonValue::Null))
            .map(|value| number(value, &child_path(ROOT, "fontVersion")))
            .transpose()?;

        Ok(Self {
            font_name,
            font_version,
            engraving_defaults,
            glyph_advance_widths: parse_measurement_map(root, "glyphAdvanceWidths", ROOT)?,
            glyph_bboxes: parse_bboxes(root, ROOT)?,
            glyphs_with_anchors: parse_anchors(root, ROOT)?,
            glyphs_with_alternates: parse_alternates(root, ROOT)?,
            ligatures: parse_ligatures(root, ROOT)?,
            optional_glyphs: parse_optional_glyphs(root, ROOT)?,
            sets: parse_sets(root, ROOT)?,
        })
    }
}

fn optional_object<'a>(root: &'a Object, key: &str, parent: &str) -> SmuflResult<Option<&'a Object>> {
    let Some(value) = root.get(key) else {
        return Ok(None);
    };
    if matches!(value, JsonValue::Null) {
        return Ok(None);
    }
    object(value, &child_path(parent, key)).map(Some)
}

fn parse_measurement_map(
    root: &Object,
    key: &str,
    parent: &str,
) -> SmuflResult<HashMap<String, StaffSpaces>> {
    let Some(values) = optional_object(root, key, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, key);
    values
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                staff_spaces(value, &child_path(&path, name))?,
            ))
        })
        .collect()
}

fn parse_bboxes(root: &Object, parent: &str) -> SmuflResult<HashMap<String, GlyphBBox>> {
    const KEY: &str = "glyphBBoxes";
    let Some(values) = optional_object(root, KEY, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, KEY);
    values
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                GlyphBBox::parse(value, &child_path(&path, name))?,
            ))
        })
        .collect()
}

fn parse_anchors(root: &Object, parent: &str) -> SmuflResult<HashMap<String, GlyphAnchors>> {
    const KEY: &str = "glyphsWithAnchors";
    let Some(values) = optional_object(root, KEY, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, KEY);
    values
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                GlyphAnchors::parse(value, &child_path(&path, name))?,
            ))
        })
        .collect()
}

fn parse_alternates(
    root: &Object,
    parent: &str,
) -> SmuflResult<HashMap<String, GlyphAlternates>> {
    const KEY: &str = "glyphsWithAlternates";
    let Some(values) = optional_object(root, KEY, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, KEY);
    values
        .iter()
        .map(|(name, value)| {
            let glyph_path = child_path(&path, name);
            let glyph = object(value, &glyph_path)?;
            let alternates_path = child_path(&glyph_path, "alternates");
            let alternates = array(field(glyph, "alternates", &glyph_path)?, &alternates_path)?
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let item_path = index_path(&alternates_path, index);
                    let item = object(value, &item_path)?;
                    Ok(GlyphAlternate {
                        name: string(field(item, "name", &item_path)?, &child_path(&item_path, "name"))?
                            .to_string(),
                        codepoint: codepoint(
                            field(item, "codepoint", &item_path)?,
                            &child_path(&item_path, "codepoint"),
                        )?,
                    })
                })
                .collect::<SmuflResult<Vec<_>>>()?;
            Ok((name.clone(), GlyphAlternates { alternates }))
        })
        .collect()
}

fn parse_ligatures(root: &Object, parent: &str) -> SmuflResult<HashMap<String, GlyphLigature>> {
    const KEY: &str = "ligatures";
    let Some(values) = optional_object(root, KEY, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, KEY);
    values
        .iter()
        .map(|(name, value)| {
            let item_path = child_path(&path, name);
            let item = object(value, &item_path)?;
            Ok((
                name.clone(),
                GlyphLigature {
                    codepoint: codepoint(
                        field(item, "codepoint", &item_path)?,
                        &child_path(&item_path, "codepoint"),
                    )?,
                    component_glyphs: string_array(
                        field(item, "componentGlyphs", &item_path)?,
                        &child_path(&item_path, "componentGlyphs"),
                    )?,
                    description: optional_string(item, "description", &item_path)?,
                },
            ))
        })
        .collect()
}

fn parse_optional_glyphs(
    root: &Object,
    parent: &str,
) -> SmuflResult<HashMap<String, OptionalGlyph>> {
    const KEY: &str = "optionalGlyphs";
    let Some(values) = optional_object(root, KEY, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, KEY);
    values
        .iter()
        .map(|(name, value)| {
            let item_path = child_path(&path, name);
            let item = object(value, &item_path)?;
            Ok((
                name.clone(),
                OptionalGlyph {
                    codepoint: codepoint(
                        field(item, "codepoint", &item_path)?,
                        &child_path(&item_path, "codepoint"),
                    )?,
                    description: optional_string(item, "description", &item_path)?,
                    classes: optional_string_array(item, "classes", &item_path)?,
                },
            ))
        })
        .collect()
}

fn parse_sets(root: &Object, parent: &str) -> SmuflResult<HashMap<String, GlyphSet>> {
    const KEY: &str = "sets";
    let Some(values) = optional_object(root, KEY, parent)? else {
        return Ok(HashMap::new());
    };
    let path = child_path(parent, KEY);
    values
        .iter()
        .map(|(name, value)| {
            let set_path = child_path(&path, name);
            let set = object(value, &set_path)?;
            let glyphs = match set.get("glyphs") {
                None | Some(JsonValue::Null) => Vec::new(),
                Some(value) => {
                    let glyphs_path = child_path(&set_path, "glyphs");
                    array(value, &glyphs_path)?
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let glyph_path = index_path(&glyphs_path, index);
                            let glyph = object(value, &glyph_path)?;
                            Ok(SetGlyph {
                                name: string(
                                    field(glyph, "name", &glyph_path)?,
                                    &child_path(&glyph_path, "name"),
                                )?
                                .to_string(),
                                codepoint: codepoint(
                                    field(glyph, "codepoint", &glyph_path)?,
                                    &child_path(&glyph_path, "codepoint"),
                                )?,
                                alternate_for: optional_string(glyph, "alternateFor", &glyph_path)?,
                                description: optional_string(glyph, "description", &glyph_path)?,
                            })
                        })
                        .collect::<SmuflResult<Vec<_>>>()?
                }
            };
            Ok((
                name.clone(),
                GlyphSet {
                    description: optional_string(set, "description", &set_path)?,
                    glyphs,
                },
            ))
        })
        .collect()
}
