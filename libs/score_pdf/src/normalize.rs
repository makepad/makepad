//! Font-agnostic symbol normalization.
//!
//! All producer-specific knowledge stops in this module. Downstream recovery
//! consumes canonical SMuFL names and semantic symbol classes only.

use crate::confidence::{Estimate, Evidence, Verification};
use crate::display::PdfGlyph;
use makepad_score::smufl::{FontMetadata, GlyphRegistry};
use makepad_score::symbol::Clef;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AccidentalKind {
    TripleFlat,
    DoubleFlat,
    Flat,
    Natural,
    Sharp,
    DoubleSharp,
    TripleSharp,
}

impl AccidentalKind {
    pub const fn semitones(self) -> i8 {
        match self {
            Self::TripleFlat => -3,
            Self::DoubleFlat => -2,
            Self::Flat => -1,
            Self::Natural => 0,
            Self::Sharp => 1,
            Self::DoubleSharp => 2,
            Self::TripleSharp => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BasicDuration {
    DoubleWhole,
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
    OneTwentyEighth,
}

impl BasicDuration {
    pub const fn denominator(self) -> u16 {
        match self {
            Self::DoubleWhole => 0,
            Self::Whole => 1,
            Self::Half => 2,
            Self::Quarter => 4,
            Self::Eighth => 8,
            Self::Sixteenth => 16,
            Self::ThirtySecond => 32,
            Self::SixtyFourth => 64,
            Self::OneTwentyEighth => 128,
        }
    }

    pub const fn from_flag_or_beam_levels(levels: u8) -> Self {
        match levels {
            0 => Self::Quarter,
            1 => Self::Eighth,
            2 => Self::Sixteenth,
            3 => Self::ThirtySecond,
            4 => Self::SixtyFourth,
            _ => Self::OneTwentyEighth,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StemDirection {
    Up,
    Down,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SymbolClass {
    Notehead(BasicDuration),
    Rest(BasicDuration),
    Clef(Clef),
    Accidental(AccidentalKind),
    Flag { levels: u8, direction: StemDirection },
    AugmentationDot,
    TimeSignature,
    OtherMusic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalizationStage {
    SmuflCanonicalName,
    SmuflCodepoint,
    VendorAlias,
    StructuralFallback,
    BravuraGeometry,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedSymbol {
    pub canonical_name: String,
    pub class: SymbolClass,
    pub stage: NormalizationStage,
}

#[derive(Clone, Debug, Default)]
pub struct SymbolNormalizer {
    registry: Option<GlyphRegistry>,
    bravura: Option<FontMetadata>,
}

impl SymbolNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_smufl_registry(mut self, registry: GlyphRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn with_bravura_metadata(mut self, metadata: FontMetadata) -> Self {
        self.bravura = Some(metadata);
        self
    }

    pub fn normalize(
        &self,
        glyph: &PdfGlyph,
        staff_space: Option<f64>,
    ) -> Option<Estimate<NormalizedSymbol>> {
        if let Some(raw_name) = glyph.raw_name.as_deref() {
            if self.is_canonical(raw_name) {
                if let Some(class) = canonical_class(raw_name) {
                    return Some(Estimate::new(
                        NormalizedSymbol {
                            canonical_name: raw_name.to_string(),
                            class,
                            stage: NormalizationStage::SmuflCanonicalName,
                        },
                        0.998,
                        0.99,
                        vec![Evidence::SmuflName(raw_name.to_string())],
                        Verification::Certain,
                    ));
                }
            }
        }

        if let Some(character) = glyph
            .unicode
            .as_deref()
            .and_then(single_character)
            .filter(|character| ('\u{E000}'..='\u{F8FF}').contains(character))
        {
            if let Some(canonical) = self
                .registry
                .as_ref()
                .and_then(|registry| registry.name_for_codepoint(character))
            {
                if let Some(class) = canonical_class(canonical) {
                    return Some(Estimate::new(
                        NormalizedSymbol {
                            canonical_name: canonical.to_string(),
                            class,
                            stage: NormalizationStage::SmuflCodepoint,
                        },
                        0.997,
                        0.985,
                        vec![Evidence::SmuflCodepoint(character)],
                        Verification::Certain,
                    ));
                }
            }
        }

        if let Some(raw_name) = glyph.raw_name.as_deref() {
            if let Some((vendor, canonical)) = vendor_alias(&glyph.font_base_name, raw_name) {
                if let Some(class) = canonical_class(canonical) {
                    return Some(Estimate::new(
                        NormalizedSymbol {
                            canonical_name: canonical.to_string(),
                            class,
                            stage: NormalizationStage::VendorAlias,
                        },
                        0.985,
                        0.96,
                        vec![Evidence::VendorAlias {
                            vendor: vendor.to_string(),
                            source: raw_name.to_string(),
                        }],
                        Verification::Certain,
                    ));
                }
            }
            if let Some(canonical) = structural_alias(raw_name) {
                if let Some(class) = canonical_class(canonical) {
                    return Some(Estimate::new(
                        NormalizedSymbol {
                            canonical_name: canonical.to_string(),
                            class,
                            stage: NormalizationStage::StructuralFallback,
                        },
                        0.82,
                        0.4,
                        vec![Evidence::StructuralName(raw_name.to_string())],
                        Verification::Inferred,
                    ));
                }
            }
        }

        self.geometry_match(glyph, staff_space?)
    }

    fn is_canonical(&self, name: &str) -> bool {
        self.registry
            .as_ref()
            .is_some_and(|registry| registry.glyph(name).is_some())
            || (!name.contains('.') && canonical_class(name).is_some())
    }

    fn geometry_match(
        &self,
        glyph: &PdfGlyph,
        staff_space: f64,
    ) -> Option<Estimate<NormalizedSymbol>> {
        if staff_space <= 0.0 {
            return None;
        }
        let metadata = self.bravura.as_ref()?;
        let observed_width = glyph.bounds.width() / staff_space;
        let observed_height = glyph.bounds.height() / staff_space;
        let mut candidates = Vec::new();
        for canonical in GEOMETRY_CANDIDATES {
            let Some(class) = canonical_class(canonical) else {
                continue;
            };
            let Some(bbox) = metadata.glyph_bboxes.get(*canonical) else {
                continue;
            };
            let expected_width = bbox.width().get().abs();
            let expected_height = bbox.height().get().abs();
            let width_scale = observed_width.max(0.001) / expected_width.max(0.001);
            let height_scale = observed_height.max(0.001) / expected_height.max(0.001);
            let aspect_penalty = (width_scale / height_scale).ln().abs();
            let isotropic_scale = (width_scale * height_scale).sqrt();
            let scale_penalty = isotropic_scale.ln().abs() * 0.15;
            candidates.push((aspect_penalty + scale_penalty, *canonical, class));
        }
        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let (distance, canonical, class) = candidates.first()?.clone();
        let runner_up = candidates.get(1).map_or(distance + 1.0, |value| value.0);
        if distance > 0.32 || runner_up - distance < 0.08 {
            return None;
        }
        let probability = (0.9 - distance as f32).clamp(0.5, 0.9);
        Some(Estimate::new(
            NormalizedSymbol {
                canonical_name: canonical.to_string(),
                class,
                stage: NormalizationStage::BravuraGeometry,
            },
            probability,
            ((runner_up - distance) as f32).clamp(0.0, 1.0),
            vec![Evidence::GeometryMatch {
                canonical: canonical.to_string(),
                distance: distance as f32,
            }],
            Verification::Inferred,
        ))
    }
}

fn single_character(value: &str) -> Option<char> {
    let mut characters = value.chars();
    let first = characters.next()?;
    characters.next().is_none().then_some(first)
}

fn vendor_alias<'a>(font: &str, name: &'a str) -> Option<(&'static str, &'static str)> {
    if name.starts_with("noteheads.")
        || name.starts_with("clefs.")
        || name.starts_with("accidentals.")
        || name.starts_with("rests.")
        || name.starts_with("flags.")
        || name == "dots.dot"
    {
        return dotted_scheme_alias(name).map(|canonical| ("dotted-name scheme", canonical));
    }
    if font.to_ascii_lowercase().contains("opus") || name.starts_with("uniF") {
        return private_use_alias(name).map(|canonical| ("private-use scheme", canonical));
    }
    None
}

fn dotted_scheme_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "noteheads.s0" => "noteheadWhole",
        "noteheads.s1" => "noteheadHalf",
        "noteheads.s2" => "noteheadBlack",
        "clefs.G" | "clefs.G_change" => "gClef",
        "clefs.F" | "clefs.F_change" => "fClef",
        "clefs.C" | "clefs.C_change" => "cClef",
        "accidentals.sharp" => "accidentalSharp",
        "accidentals.flat" => "accidentalFlat",
        "accidentals.natural" => "accidentalNatural",
        "accidentals.doublesharp" => "accidentalDoubleSharp",
        "accidentals.flatflat" => "accidentalDoubleFlat",
        "accidentals.0" => "accidentalNatural",
        "accidentals.2" => "accidentalSharp",
        "accidentals.-2" => "accidentalFlat",
        "dots.dot" => "augmentationDot",
        "rests.0" => "restWhole",
        "rests.1" => "restHalf",
        "rests.2" => "restQuarter",
        "rests.3" => "rest8th",
        "rests.4" => "rest16th",
        "rests.5" => "rest32nd",
        "rests.6" => "rest64th",
        "flags.u3" => "flag8thUp",
        "flags.u4" => "flag16thUp",
        "flags.u5" => "flag32ndUp",
        "flags.u6" => "flag64thUp",
        "flags.d3" => "flag8thDown",
        "flags.d4" => "flag16thDown",
        "flags.d5" => "flag32ndDown",
        "flags.d6" => "flag64thDown",
        "timesig.C44" => "timeSigCommon",
        "timesig.C22" => "timeSigCutCommon",
        _ => return None,
    })
}

/// Explicit aliases for a legacy engraver's private-use encoding found in the
/// corpus, where glyphs are named `uniFxxx` in a font-specific arrangement.
/// These are names emitted by the PDF subset, not Unicode or SMuFL
/// assignments.
fn private_use_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "uniF023" => "accidentalSharp",
        "uniF026" => "gClef",
        "uniF03F" => "fClef",
        "uniF043" => "timeSigCommon",
        "uniF062" => "accidentalFlat",
        "uniF06E" => "accidentalNatural",
        "uniF077" => "noteheadWhole",
        "uniF0CF" => "noteheadHalf",
        "uniF0FA" => "noteheadBlack",
        "uniF04A" => "restQuarter",
        "uniF055" => "flag8thUp",
        "uniF06A" => "flag8thDown",
        "uniF0A1" => "rest8th",
        "uniF0A2" => "rest16th",
        "uniF0AA" => "augmentationDot",
        "uniF0CE" => "restHalf",
        _ => return None,
    })
}

fn structural_alias(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("notehead") || lower.starts_with("head") {
        if lower.contains("whole") || lower.ends_with("s0") {
            Some("noteheadWhole")
        } else if lower.contains("half") || lower.ends_with("s1") {
            Some("noteheadHalf")
        } else {
            Some("noteheadBlack")
        }
    } else if lower.contains("treble") || lower == "gclef" {
        Some("gClef")
    } else if lower.contains("bass") || lower == "fclef" {
        Some("fClef")
    } else if lower.contains("cclef") || lower.contains("alto") || lower.contains("tenorclef") {
        Some("cClef")
    } else if lower.contains("sharp") {
        Some("accidentalSharp")
    } else if lower.contains("flat") {
        Some("accidentalFlat")
    } else if lower.contains("natural") {
        Some("accidentalNatural")
    } else if lower.contains("dot") {
        Some("augmentationDot")
    } else if lower.contains("rest") {
        Some("restQuarter")
    } else if lower.contains("flag") {
        Some("flag8thUp")
    } else {
        None
    }
}

pub fn canonical_class(name: &str) -> Option<SymbolClass> {
    if let Some(duration) = canonical_note_duration(name) {
        return Some(SymbolClass::Notehead(duration));
    }
    let class = match name {
        "noteheadDoubleWhole" | "noteheadDoubleWholeSquare" => {
            SymbolClass::Notehead(BasicDuration::DoubleWhole)
        }
        "noteheadWhole" => SymbolClass::Notehead(BasicDuration::Whole),
        "noteheadHalf" => SymbolClass::Notehead(BasicDuration::Half),
        "noteheadBlack" => SymbolClass::Notehead(BasicDuration::Quarter),
        "gClef" | "gClefChange" | "gClefSmall" => SymbolClass::Clef(Clef::G),
        "fClef" | "fClefChange" | "fClefSmall" => SymbolClass::Clef(Clef::F),
        "cClef" | "cClefChange" | "cClefSmall" => SymbolClass::Clef(Clef::C),
        "accidentalTripleFlat" => SymbolClass::Accidental(AccidentalKind::TripleFlat),
        "accidentalDoubleFlat" => SymbolClass::Accidental(AccidentalKind::DoubleFlat),
        "accidentalFlat" => SymbolClass::Accidental(AccidentalKind::Flat),
        "accidentalNatural" => SymbolClass::Accidental(AccidentalKind::Natural),
        "accidentalSharp" => SymbolClass::Accidental(AccidentalKind::Sharp),
        "accidentalDoubleSharp" => SymbolClass::Accidental(AccidentalKind::DoubleSharp),
        "accidentalTripleSharp" => SymbolClass::Accidental(AccidentalKind::TripleSharp),
        "augmentationDot" => SymbolClass::AugmentationDot,
        "restDoubleWhole" => SymbolClass::Rest(BasicDuration::DoubleWhole),
        "restWhole" => SymbolClass::Rest(BasicDuration::Whole),
        "restHalf" => SymbolClass::Rest(BasicDuration::Half),
        "restQuarter" => SymbolClass::Rest(BasicDuration::Quarter),
        "rest8th" => SymbolClass::Rest(BasicDuration::Eighth),
        "rest16th" => SymbolClass::Rest(BasicDuration::Sixteenth),
        "rest32nd" => SymbolClass::Rest(BasicDuration::ThirtySecond),
        "rest64th" => SymbolClass::Rest(BasicDuration::SixtyFourth),
        "flag8thUp" => SymbolClass::Flag {
            levels: 1,
            direction: StemDirection::Up,
        },
        "flag16thUp" => SymbolClass::Flag {
            levels: 2,
            direction: StemDirection::Up,
        },
        "flag32ndUp" => SymbolClass::Flag {
            levels: 3,
            direction: StemDirection::Up,
        },
        "flag64thUp" => SymbolClass::Flag {
            levels: 4,
            direction: StemDirection::Up,
        },
        "flag8thDown" => SymbolClass::Flag {
            levels: 1,
            direction: StemDirection::Down,
        },
        "flag16thDown" => SymbolClass::Flag {
            levels: 2,
            direction: StemDirection::Down,
        },
        "flag32ndDown" => SymbolClass::Flag {
            levels: 3,
            direction: StemDirection::Down,
        },
        "flag64thDown" => SymbolClass::Flag {
            levels: 4,
            direction: StemDirection::Down,
        },
        name if name.starts_with("timeSig") => SymbolClass::TimeSignature,
        name if name.starts_with("notehead")
            || name.starts_with("rest")
            || name.starts_with("accidental")
            || name.starts_with("flag")
            || name.ends_with("Clef") => SymbolClass::OtherMusic,
        _ => return None,
    };
    Some(class)
}

fn canonical_note_duration(name: &str) -> Option<BasicDuration> {
    let is_notehead = name.starts_with("notehead");
    let is_pitched_note = name.starts_with("note")
        && (name.ends_with("Black")
            || name.ends_with("Half")
            || name.ends_with("Whole")
            || name.starts_with("noteHalf")
            || name.starts_with("noteWhole"));
    if !is_notehead && !is_pitched_note {
        return None;
    }
    if name.contains("DoubleWhole") {
        Some(BasicDuration::DoubleWhole)
    } else if name.ends_with("Whole") || name.contains("WhiteWhole") {
        Some(BasicDuration::Whole)
    } else if name.ends_with("Half")
        || name.starts_with("noteHalf")
        || name.contains("WhiteHalf")
        || name.contains("RoundWhite")
    {
        Some(BasicDuration::Half)
    } else {
        // SMuFL's named clusters, slashes, shapes and pitch-labelled `Black`
        // entries are filled noteheads unless the name explicitly says white.
        Some(BasicDuration::Quarter)
    }
}

const GEOMETRY_CANDIDATES: &[&str] = &[
    "noteheadBlack",
    "noteheadHalf",
    "noteheadWhole",
    "accidentalFlat",
    "accidentalNatural",
    "accidentalSharp",
    "augmentationDot",
    "gClef",
    "fClef",
    "cClef",
    "restQuarter",
    "rest8th",
    "rest16th",
    "flag8thUp",
    "flag8thDown",
];
