pub mod arabic;
pub mod hangul;
pub mod hebrew;
pub mod indic;
pub mod khmer;
pub mod myanmar;
pub mod thai;
pub mod universal;
mod arabic_table;
mod indic_machine;
mod indic_table;
mod khmer_machine;
mod myanmar_machine;
mod universal_machine;
mod universal_table;
mod vowel_constraints;

use alloc::boxed::Box;
use core::any::Any;

use crate::{script, Direction, Face, Script, Tag};
use crate::buffer::Buffer;
use crate::common::TagExt;
use crate::normalize::{ShapeNormalizeContext, ShapeNormalizationMode};
use crate::plan::{ShapePlan, ShapePlanner};

pub const MAX_COMBINING_MARKS: usize = 32;

pub const DEFAULT_SHAPER: ComplexShaper = ComplexShaper {
    collect_features: None,
    override_features: None,
    create_data: None,
    preprocess_text: None,
    postprocess_glyphs: None,
    normalization_mode: Some(ShapeNormalizationMode::Auto),
    decompose: None,
    compose: None,
    setup_masks: None,
    gpos_tag: None,
    reorder_marks: None,
    zero_width_marks: Some(ZeroWidthMarksMode::ByGdefLate),
    fallback_position: true,
};

// Same as default but no mark advance zeroing / fallback positioning.
// Dumbest shaper ever, basically.
pub const DUMBER_SHAPER: ComplexShaper = ComplexShaper {
    collect_features: None,
    override_features: None,
    create_data: None,
    preprocess_text: None,
    postprocess_glyphs: None,
    normalization_mode: Some(ShapeNormalizationMode::Auto),
    decompose: None,
    compose: None,
    setup_masks: None,
    gpos_tag: None,
    reorder_marks: None,
    zero_width_marks: None,
    fallback_position: false,
};

pub struct ComplexShaper {
    /// Called during `shape_plan()`.
    /// Shapers should use plan.map to add their features and callbacks.
    pub collect_features: Option<fn(&mut ShapePlanner)>,

    /// Called during `shape_plan()`.
    /// Shapers should use plan.map to override features and add callbacks after
    /// common features are added.
    pub override_features: Option<fn(&mut ShapePlanner)>,

    /// Called at the end of `shape_plan()`.
    /// Whatever shapers return will be accessible through `plan.data()` later.
    pub create_data: Option<fn(&ShapePlan) -> Box<dyn Any>>,

    /// Called during `shape()`.
    /// Shapers can use to modify text before shaping starts.
    pub preprocess_text: Option<fn(&ShapePlan, &Face, &mut Buffer)>,

    /// Called during `shape()`.
    /// Shapers can use to modify text before shaping starts.
    pub postprocess_glyphs: Option<fn(&ShapePlan, &Face, &mut Buffer)>,

    /// How to normalize.
    pub normalization_mode: Option<ShapeNormalizationMode>,

    /// Called during `shape()`'s normalization.
    pub decompose: Option<fn(&ShapeNormalizeContext, char) -> Option<(char, char)>>,

    /// Called during `shape()`'s normalization.
    pub compose: Option<fn(&ShapeNormalizeContext, char, char) -> Option<char>>,

    /// Called during `shape()`.
    /// Shapers should use map to get feature masks and set on buffer.
    /// Shapers may NOT modify characters.
    pub setup_masks: Option<fn(&ShapePlan, &Face, &mut Buffer)>,

    /// If not `None`, then must match found GPOS script tag for
    /// GPOS to be applied.  Otherwise, fallback positioning will be used.
    pub gpos_tag: Option<Tag>,

    /// Called during `shape()`.
    /// Shapers can use to modify ordering of combining marks.
    pub reorder_marks: Option<fn(&ShapePlan, &mut Buffer, usize, usize)>,

    /// If and when to zero-width marks.
    pub zero_width_marks: Option<ZeroWidthMarksMode>,

    /// Whether to use fallback mark positioning.
    pub fallback_position: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroWidthMarksMode {
    ByGdefEarly,
    ByGdefLate,
}

pub fn complex_categorize(
    script: Script,
    direction: Direction,
    chosen_gsub_script: Option<Tag>,
) -> &'static ComplexShaper {
    match script {
        // Unicode-1.1 additions
        script::ARABIC

        // Unicode-3.0 additions
        | script::MONGOLIAN
        | script::SYRIAC

        // Unicode-5.0 additions
        | script::NKO
        | script::PHAGS_PA

        // Unicode-6.0 additions
        | script::MANDAIC

        // Unicode-7.0 additions
        | script::MANICHAEAN
        | script::PSALTER_PAHLAVI

        // Unicode-9.0 additions
        | script::ADLAM

        // Unicode-11.0 additions
        | script::HANIFI_ROHINGYA
        | script::SOGDIAN => {
            // For Arabic script, use the Arabic shaper even if no OT script tag was found.
            // This is because we do fallback shaping for Arabic script (and not others).
            // But note that Arabic shaping is applicable only to horizontal layout; for
            // vertical text, just use the generic shaper instead.
            //
            // TODO: Does this still apply? Arabic fallback shaping was removed.
            if (chosen_gsub_script != Some(Tag::default_script()) || script == script::ARABIC)
                && direction.is_horizontal()
            {
                &arabic::ARABIC_SHAPER
            } else {
                &DEFAULT_SHAPER
            }
        }

        // Unicode-1.1 additions
        script::THAI
        | script::LAO => &thai::THAI_SHAPER,

        // Unicode-1.1 additions
        script::HANGUL => &hangul::HANGUL_SHAPER,

        // Unicode-1.1 additions
        script::HEBREW => &hebrew::HEBREW_SHAPER,

        // Unicode-1.1 additions
        script::BENGALI
        | script::DEVANAGARI
        | script::GUJARATI
        | script::GURMUKHI
        | script::KANNADA
        | script::MALAYALAM
        | script::ORIYA
        | script::TAMIL
        | script::TELUGU

        // Unicode-3.0 additions
        | script::SINHALA => {
            // If the designer designed the font for the 'DFLT' script,
            // (or we ended up arbitrarily pick 'latn'), use the default shaper.
            // Otherwise, use the specific shaper.
            //
            // If it's indy3 tag, send to USE.
            if chosen_gsub_script == Some(Tag::default_script()) ||
               chosen_gsub_script == Some(Tag::from_bytes(b"latn")) {
                &DEFAULT_SHAPER
            } else if chosen_gsub_script.map_or(false, |tag| tag.to_bytes()[3] == b'3') {
                &universal::UNIVERSAL_SHAPER
            } else {
                &indic::INDIC_SHAPER
            }
        }

        script::KHMER => &khmer::KHMER_SHAPER,

        script::MYANMAR => {
            // If the designer designed the font for the 'DFLT' script,
            // (or we ended up arbitrarily pick 'latn'), use the default shaper.
            // Otherwise, use the specific shaper.
            //
            // If designer designed for 'mymr' tag, also send to default
            // shaper.  That's tag used from before Myanmar shaping spec
            // was developed.  The shaping spec uses 'mym2' tag.
            if chosen_gsub_script == Some(Tag::default_script()) ||
               chosen_gsub_script == Some(Tag::from_bytes(b"latn")) ||
               chosen_gsub_script == Some(Tag::from_bytes(b"mymr"))
            {
                &DEFAULT_SHAPER
            } else {
                &myanmar::MYANMAR_SHAPER
            }
        }

        // https://github.com/harfbuzz/harfbuzz/issues/1162
        script::MYANMAR_ZAWGYI => &myanmar::MYANMAR_ZAWGYI_SHAPER,

        // Unicode-2.0 additions
        script::TIBETAN

        // Unicode-3.0 additions
        // | script::MONGOLIAN
        // | script::SINHALA

        // Unicode-3.2 additions
        | script::BUHID
        | script::HANUNOO
        | script::TAGALOG
        | script::TAGBANWA

        // Unicode-4.0 additions
        | script::LIMBU
        | script::TAI_LE

        // Unicode-4.1 additions
        | script::BUGINESE
        | script::KHAROSHTHI
        | script::SYLOTI_NAGRI
        | script::TIFINAGH

        // Unicode-5.0 additions
        | script::BALINESE
        // | script::NKO
        // | script::PHAGS_PA

        // Unicode-5.1 additions
        | script::CHAM
        | script::KAYAH_LI
        | script::LEPCHA
        | script::REJANG
        | script::SAURASHTRA
        | script::SUNDANESE

        // Unicode-5.2 additions
        | script::EGYPTIAN_HIEROGLYPHS
        | script::JAVANESE
        | script::KAITHI
        | script::MEETEI_MAYEK
        | script::TAI_THAM
        | script::TAI_VIET

        // Unicode-6.0 additions
        | script::BATAK
        | script::BRAHMI
        // | script::MANDAIC

        // Unicode-6.1 additions
        | script::CHAKMA
        | script::SHARADA
        | script::TAKRI

        // Unicode-7.0 additions
        | script::DUPLOYAN
        | script::GRANTHA
        | script::KHOJKI
        | script::KHUDAWADI
        | script::MAHAJANI
        // | script::MANICHAEAN
        | script::MODI
        | script::PAHAWH_HMONG
        // | script::PSALTER_PAHLAVI
        | script::SIDDHAM
        | script::TIRHUTA

        // Unicode-8.0 additions
        | script::AHOM

        // Unicode-9.0 additions
        // | script::ADLAM
        | script::BHAIKSUKI
        | script::MARCHEN
        | script::NEWA

        // Unicode-10.0 additions
        | script::MASARAM_GONDI
        | script::SOYOMBO
        | script::ZANABAZAR_SQUARE

        // Unicode-11.0 additions
        | script::DOGRA
        | script::GUNJALA_GONDI
        // | script::HANIFI_ROHINGYA
        | script::MAKASAR
        // | script::SOGDIAN

        // Unicode-12.0 additions
        | script::NANDINAGARI

        // Unicode-13.0 additions
        | script::CHORASMIAN
        | script::DIVES_AKURU => {
            // If the designer designed the font for the 'DFLT' script,
            // (or we ended up arbitrarily pick 'latn'), use the default shaper.
            // Otherwise, use the specific shaper.
            // Note that for some simple scripts, there may not be *any*
            // GSUB/GPOS needed, so there may be no scripts found!
            if chosen_gsub_script == Some(Tag::default_script()) ||
               chosen_gsub_script == Some(Tag::from_bytes(b"latn")) {
                &DEFAULT_SHAPER
            } else {
                &universal::UNIVERSAL_SHAPER
            }
        }

        _ => &DEFAULT_SHAPER
    }
}

// TODO: find a better name
#[inline]
pub const fn rb_flag(x: u32) -> u32 {
    1 << x
}

#[inline]
pub fn rb_flag_unsafe(x: u32) -> u32 {
    if x < 32 { 1 << x } else { 0 }
}

#[inline]
pub fn rb_flag_range(x: u32, y: u32) -> u32 {
    (x < y) as u32 + rb_flag(y + 1) - rb_flag(x)
}

#[inline]
pub const fn rb_flag64(x: u32) -> u64 {
    1 << x as u64
}

#[inline]
pub fn rb_flag64_unsafe(x: u32) -> u64 {
    if x < 64 { 1 << (x as u64) } else { 0 }
}
