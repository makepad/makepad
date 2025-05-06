
use core::cmp;
use core::convert::TryFrom;
use core::ops::Range;

use ttf_parser::GlyphId;

use crate::{ Mask,  GlyphInfo};
use crate::buffer::{ BufferFlags};
use crate::ot::{feature, FeatureFlags, LayoutTable, Map, TableIndex, WouldApply, WouldApplyContext};

use crate::unicode::{CharExt, GeneralCategoryExt, hb_gc};
use super::*;


pub const INDIC_SHAPER: ComplexShaper = ComplexShaper {
    collect_features: Some(collect_features),
    override_features: Some(override_features),
    create_data: Some(|plan| Box::new(IndicShapePlan::new(plan))),
    preprocess_text: Some(preprocess_text),
    postprocess_glyphs: None,
    normalization_mode: Some(ShapeNormalizationMode::ComposedDiacriticsNoShortCircuit),
    decompose: Some(decompose),
    compose: Some(compose),
    setup_masks: Some(setup_masks),
    gpos_tag: None,
    reorder_marks: None,
    zero_width_marks: None,
    fallback_position: false,
};


pub type Category = u8;
pub mod category {
    pub const X: u8 = 0;
    pub const C: u8 = 1;
    pub const V: u8 = 2;
    pub const N: u8 = 3;
    pub const H: u8 = 4;
    pub const ZWNJ: u8 = 5;
    pub const ZWJ: u8 = 6;
    pub const M: u8 = 7;
    pub const SM: u8 = 8;
    // OT_VD = 9, UNUSED; we use OT_A instead.
    pub const A: u8 = 10;
    pub const PLACEHOLDER: u8 = 11;
    pub const DOTTED_CIRCLE: u8 = 12;
    pub const RS: u8 = 13; // Register Shifter, used in Khmer OT spec.
    pub const COENG: u8 = 14; // Khmer-style Virama.
    pub const REPHA: u8 = 15; // Atomically-encoded logical or visual repha.
    pub const RA: u8 = 16;
    pub const CM: u8 = 17; // Consonant-Medial.
    pub const SYMBOL: u8 = 18; // Avagraha, etc that take marks (SM,A,VD).
    pub const CS: u8 = 19;
    pub const ROBATIC: u8 = 20;
    pub const X_GROUP: u8 = 21;
    pub const Y_GROUP: u8 = 22;
    pub const MW: u8 = 23;
    pub const MY: u8 = 24;
    pub const PT: u8 = 25;
    // The following are used by Khmer & Myanmar shapers.  Defined here for them to share.
    pub const V_AVB: u8 = 26;
    pub const V_BLW: u8 = 27;
    pub const V_PRE: u8 = 28;
    pub const V_PST: u8 = 29;
    pub const VS: u8 = 30; // Variation selectors
    pub const P: u8 = 31;  // Punctuation
    pub const D: u8 = 32;  // Digits except zero
}

pub type Position = u8;
pub mod position {
    pub const START: u8 = 0;
    pub const RA_TO_BECOME_REPH: u8 = 1;
    pub const PRE_M: u8 = 2;
    pub const PRE_C: u8 = 3;
    pub const BASE_C: u8 = 4;
    pub const AFTER_MAIN: u8 = 5;
    pub const ABOVE_C: u8 = 6;
    pub const BEFORE_SUB: u8 = 7;
    pub const BELOW_C: u8 = 8;
    pub const AFTER_SUB: u8 = 9;
    pub const BEFORE_POST: u8 = 10;
    pub const POST_C: u8 = 11;
    pub const AFTER_POST: u8 = 12;
    pub const FINAL_C: u8 = 13;
    pub const SMVD: u8 = 14;
    pub const END: u8 = 15;
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub enum SyllabicCategory {
    Other,
    Avagraha,
    Bindu,
    BrahmiJoiningNumber,
    CantillationMark,
    Consonant,
    ConsonantDead,
    ConsonantFinal,
    ConsonantHeadLetter,
    ConsonantInitialPostfixed,
    ConsonantKiller,
    ConsonantMedial,
    ConsonantPlaceholder,
    ConsonantPrecedingRepha,
    ConsonantPrefixed,
    ConsonantSubjoined,
    ConsonantSucceedingRepha,
    ConsonantWithStacker,
    GeminationMark,
    InvisibleStacker,
    Joiner,
    ModifyingLetter,
    NonJoiner,
    Nukta,
    Number,
    NumberJoiner,
    PureKiller,
    RegisterShifter,
    SyllableModifier,
    ToneLetter,
    ToneMark,
    Virama,
    Visarga,
    Vowel,
    VowelDependent,
    VowelIndependent,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub enum MatraCategory {
    NotApplicable,
    Left,
    Top,
    Bottom,
    Right,
    BottomAndLeft,
    BottomAndRight,
    LeftAndRight,
    TopAndBottom,
    TopAndBottomAndRight,
    TopAndLeft,
    TopAndLeftAndRight,
    TopAndRight,
    Overstruck,
    VisualOrderLeft,
}

const INDIC_FEATURES: &[(Tag, FeatureFlags)] = &[
    // Basic features.
    // These features are applied in order, one at a time, after initial_reordering.
    (feature::NUKTA_FORMS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::AKHANDS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::REPH_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::RAKAR_FORMS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::PRE_BASE_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::BELOW_BASE_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::ABOVE_BASE_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::HALF_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::POST_BASE_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::VATTU_VARIANTS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::CONJUNCT_FORMS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    // Other features.
    // These features are applied all at once, after final_reordering
    // but before clearing syllables.
    // Default Bengali font in Windows for example has intermixed
    // lookups for init,pres,abvs,blws features.
    (feature::INITIAL_FORMS, FeatureFlags::MANUAL_JOINERS),
    (feature::PRE_BASE_SUBSTITUTIONS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::ABOVE_BASE_SUBSTITUTIONS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::BELOW_BASE_SUBSTITUTIONS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::POST_BASE_SUBSTITUTIONS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
    (feature::HALANT_FORMS, FeatureFlags::GLOBAL_MANUAL_JOINERS),
];

// Must be in the same order as the INDIC_FEATURES array.
#[allow(dead_code)]
mod indic_feature {
    pub const NUKT: usize = 0;
    pub const AKHN: usize = 1;
    pub const RPHF: usize = 2;
    pub const RKRF: usize = 3;
    pub const PREF: usize = 4;
    pub const BLWF: usize = 5;
    pub const ABVF: usize = 6;
    pub const HALF: usize = 7;
    pub const PSTF: usize = 8;
    pub const VATU: usize = 9;
    pub const CJCT: usize = 10;
    pub const INIT: usize = 11;
    pub const PRES: usize = 12;
    pub const ABVS: usize = 13;
    pub const BLWS: usize = 14;
    pub const PSTS: usize = 15;
    pub const HALN: usize = 16;
}

const fn category_flag(c: Category) -> u32 {
    rb_flag(c as u32)
}

const MEDIAL_FLAGS: u32 = category_flag(category::CM);
// Note:
//
// We treat Vowels and placeholders as if they were consonants.  This is safe because Vowels
// cannot happen in a consonant syllable.  The plus side however is, we can call the
// consonant syllable logic from the vowel syllable function and get it all right!
const CONSONANT_FLAGS: u32 =
    category_flag(category::C) |
    category_flag(category::CS) |
    category_flag(category::RA) |
    MEDIAL_FLAGS |
    category_flag(category::V) |
    category_flag(category::PLACEHOLDER) |
    category_flag(category::DOTTED_CIRCLE)
;
const JOINER_FLAGS: u32 = category_flag(category::ZWJ) | category_flag(category::ZWNJ);

// This is a hack for now.  We should move this data into the main Indic table.
// Or completely remove it and just check in the tables.
const RA_CHARS: &[u32] = &[
    0x0930, // Devanagari
    0x09B0, // Bengali
    0x09F0, // Bengali
    0x0A30, // Gurmukhi. No Reph
    0x0AB0, // Gujarati
    0x0B30, // Oriya
    0x0BB0, // Tamil. No Reph
    0x0C30, // Telugu. Reph formed only with ZWJ
    0x0CB0, // Kannada
    0x0D30, // Malayalam. No Reph, Logical Repha

    0x0DBB, // Sinhala. Reph formed only with ZWJ

    0x179A, // Khmer
];

#[derive(Clone, Copy, PartialEq)]
enum BasePosition {
    LastSinhala,
    Last,
}

#[derive(Clone, Copy, PartialEq)]
enum RephPosition {
    AfterMain = position::AFTER_MAIN as isize,
    BeforeSub = position::BEFORE_SUB as isize,
    AfterSub = position::AFTER_SUB as isize,
    BeforePost = position::BEFORE_POST as isize,
    AfterPost = position::AFTER_POST as isize,
}

#[derive(Clone, Copy, PartialEq)]
enum RephMode {
    /// Reph formed out of initial Ra,H sequence.
    Implicit,
    /// Reph formed out of initial Ra,H,ZWJ sequence.
    Explicit,
    /// Encoded Repha character, needs reordering.
    LogRepha,
}

#[derive(Clone, Copy, PartialEq)]
enum BlwfMode {
    /// Below-forms feature applied to pre-base and post-base.
    PreAndPost,
    /// Below-forms feature applied to post-base only.
    PostOnly,
}

#[derive(Clone, Copy)]
struct IndicConfig {
    script: Option<Script>,
    has_old_spec: bool,
    virama: u32,
    base_pos: BasePosition,
    reph_pos: RephPosition,
    reph_mode: RephMode,
    blwf_mode: BlwfMode,
}

impl IndicConfig {
    const fn new(
        script: Option<Script>,
        has_old_spec: bool,
        virama: u32,
        base_pos: BasePosition,
        reph_pos: RephPosition,
        reph_mode: RephMode,
        blwf_mode: BlwfMode,
    ) -> Self {
        IndicConfig {
            script,
            has_old_spec,
            virama,
            base_pos,
            reph_pos,
            reph_mode,
            blwf_mode,
        }
    }
}

const INDIC_CONFIGS: &[IndicConfig] = &[
    IndicConfig::new(
        None, false, 0, BasePosition::Last,
        RephPosition::BeforePost, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::DEVANAGARI), true, 0x094D, BasePosition::Last,
        RephPosition::BeforePost, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::BENGALI), true, 0x09CD, BasePosition::Last,
        RephPosition::AfterSub, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::GURMUKHI), true, 0x0A4D, BasePosition::Last,
        RephPosition::BeforeSub, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::GUJARATI), true, 0x0ACD, BasePosition::Last,
        RephPosition::BeforePost, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::ORIYA), true, 0x0B4D, BasePosition::Last,
        RephPosition::AfterMain, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::TAMIL), true, 0x0BCD, BasePosition::Last,
        RephPosition::AfterPost, RephMode::Implicit, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::TELUGU), true, 0x0C4D, BasePosition::Last,
        RephPosition::AfterPost, RephMode::Explicit, BlwfMode::PostOnly
    ),
    IndicConfig::new(
        Some(script::KANNADA), true, 0x0CCD, BasePosition::Last,
        RephPosition::AfterPost, RephMode::Implicit, BlwfMode::PostOnly
    ),
    IndicConfig::new(
        Some(script::MALAYALAM), true, 0x0D4D, BasePosition::Last,
        RephPosition::AfterMain, RephMode::LogRepha, BlwfMode::PreAndPost
    ),
    IndicConfig::new(
        Some(script::SINHALA), false, 0x0DCA, BasePosition::LastSinhala,
        RephPosition::AfterPost, RephMode::Explicit, BlwfMode::PreAndPost
    ),
];


struct IndicWouldSubstituteFeature {
    lookups: Range<usize>,
    zero_context: bool,
}

impl IndicWouldSubstituteFeature {
    pub fn new(map: &Map, feature_tag: Tag, zero_context: bool) -> Self {
        IndicWouldSubstituteFeature {
            lookups: match map.feature_stage(TableIndex::GSUB, feature_tag) {
                Some(stage) => map.stage_lookup_range(TableIndex::GSUB, stage),
                None => 0..0,
            },
            zero_context,
        }
    }

    pub fn would_substitute(&self, map: &Map, face: &Face, glyphs: &[GlyphId]) -> bool {
        for index in self.lookups.clone() {
            let lookup = map.lookup(TableIndex::GSUB, index);
            let ctx = WouldApplyContext { glyphs, zero_context: self.zero_context };
            if face.gsub
                .as_ref()
                .and_then(|table| table.get_lookup(lookup.index))
                .map_or(false, |lookup| lookup.would_apply(&ctx))
            {
                return true;
            }
        }

        false
    }
}


struct IndicShapePlan {
    config: IndicConfig,
    is_old_spec: bool,
    // virama_glyph: Option<u32>,
    rphf: IndicWouldSubstituteFeature,
    pref: IndicWouldSubstituteFeature,
    blwf: IndicWouldSubstituteFeature,
    pstf: IndicWouldSubstituteFeature,
    vatu: IndicWouldSubstituteFeature,
    mask_array: [Mask; INDIC_FEATURES.len()],
}

impl IndicShapePlan {
    fn new(plan: &ShapePlan) -> Self {
        let script = plan.script;
        let config = if let Some(c) = INDIC_CONFIGS.iter().skip(1).find(|c| c.script == script) {
            *c
        } else {
            INDIC_CONFIGS[0]
        };

        let is_old_spec = config.has_old_spec
            && plan.ot_map.chosen_script(TableIndex::GSUB)
                .map_or(false, |tag| tag.to_bytes()[3] != b'2');

        // Use zero-context would_substitute() matching for new-spec of the main
        // Indic scripts, and scripts with one spec only, but not for old-specs.
        // The new-spec for all dual-spec scripts says zero-context matching happens.
        //
        // However, testing with Malayalam shows that old and new spec both allow
        // context.  Testing with Bengali new-spec however shows that it doesn't.
        // So, the heuristic here is the way it is.  It should *only* be changed,
        // as we discover more cases of what Windows does.  DON'T TOUCH OTHERWISE.
        let zero_context = is_old_spec && script != Some(script::MALAYALAM);

        let mut mask_array = [0; INDIC_FEATURES.len()];
        for (i, feature) in INDIC_FEATURES.iter().enumerate() {
            mask_array[i] = if feature.1.contains(FeatureFlags::GLOBAL) {
                0
            } else {
                plan.ot_map.one_mask(feature.0)
            }
        }

        // TODO: what is this?
        // let mut virama_glyph = None;
        // if config.virama != 0 {
        //     if let Some(g) = face.glyph_index(char::try_from(config.virama).unwrap()) {
        //         virama_glyph = Some(g.0 as u32);
        //     }
        // }

        IndicShapePlan {
            config,
            is_old_spec,
            // virama_glyph,
            rphf: IndicWouldSubstituteFeature::new(&plan.ot_map, feature::REPH_FORMS, zero_context),
            pref: IndicWouldSubstituteFeature::new(&plan.ot_map, feature::PRE_BASE_FORMS, zero_context),
            blwf: IndicWouldSubstituteFeature::new(&plan.ot_map, feature::BELOW_BASE_FORMS, zero_context),
            pstf: IndicWouldSubstituteFeature::new(&plan.ot_map, feature::POST_BASE_FORMS, zero_context),
            vatu: IndicWouldSubstituteFeature::new(&plan.ot_map, feature::VATTU_VARIANTS, zero_context),
            mask_array,
        }
    }
}


impl GlyphInfo {
    pub(crate) fn indic_category(&self) -> Category {
        let v: &[u8; 4] = bytemuck::cast_ref(&self.var2);
        v[2]
    }

    pub(crate) fn set_indic_category(&mut self, c: Category) {
        let v: &mut [u8; 4] = bytemuck::cast_mut(&mut self.var2);
        v[2] = c;
    }

    pub(crate) fn indic_position(&self) -> Position {
        let v: &[u8; 4] = bytemuck::cast_ref(&self.var2);
        v[3]
    }

    pub(crate) fn set_indic_position(&mut self, c: Position) {
        let v: &mut [u8; 4] = bytemuck::cast_mut(&mut self.var2);
        v[3] = c;
    }

    fn is_one_of(&self, flags: u32) -> bool {
        // If it ligated, all bets are off.
        if self.is_ligated() {
            return false;
        }

        rb_flag_unsafe(self.indic_category() as u32) & flags != 0
    }

    fn is_joiner(&self) -> bool {
        self.is_one_of(JOINER_FLAGS)
    }

    pub(crate) fn is_consonant(&self) -> bool {
        self.is_one_of(CONSONANT_FLAGS)
    }

    fn is_halant(&self) -> bool {
        self.is_one_of(rb_flag(category::H as u32))
    }

    fn set_indic_properties(&mut self) {
        let u = self.glyph_id;
        let (mut cat, mut pos) = get_category_and_position(u);

        // Re-assign category

        // The following act more like the Bindus.
        match u {
            0x0953..=0x0954 => cat = category::SM,
            // The following act like consonants.
            0x0A72..=0x0A73 | 0x1CF5..=0x1CF6 => cat = category::C,
            // TODO: The following should only be allowed after a Visarga.
            // For now, just treat them like regular tone marks.
            0x1CE2..=0x1CE8 => cat = category::A,
            // TODO: The following should only be allowed after some of
            // the nasalization marks, maybe only for U+1CE9..U+1CF1.
            // For now, just treat them like tone marks.
            0x1CED => cat = category::A,
            // The following take marks in standalone clusters, similar to Avagraha.
            0xA8F2..=0xA8F7 | 0x1CE9..=0x1CEC | 0x1CEE..=0x1CF1 => cat = category::SYMBOL,
            // https://github.com/harfbuzz/harfbuzz/issues/524
            0x0A51 => { cat = category::M; pos = position::BELOW_C; }
            // According to ScriptExtensions.txt, these Grantha marks may also be used in Tamil,
            // so the Indic shaper needs to know their categories.
            0x11301 | 0x11303 => cat = category::SM,
            0x1133B | 0x1133C => cat = category::N,
            // https://github.com/harfbuzz/harfbuzz/issues/552
            0x0AFB => cat = category::N,
            // https://github.com/harfbuzz/harfbuzz/issues/538
            0x0980 => cat = category::PLACEHOLDER,
            // https://github.com/harfbuzz/harfbuzz/issues/1613
            0x09FC => cat = category::PLACEHOLDER,
            // https://github.com/harfbuzz/harfbuzz/issues/623
            0x0C80 => cat = category::PLACEHOLDER,
            0x2010 | 0x2011 => cat = category::PLACEHOLDER,
            0x25CC => cat = category::DOTTED_CIRCLE,
            _ => {}
        }

        // Re-assign position.

        if (rb_flag_unsafe(cat as u32) & CONSONANT_FLAGS) != 0 {
            pos = position::BASE_C;
            if RA_CHARS.contains(&u) {
                cat = category::RA;
            }
        } else if cat == category::M {
            pos = matra_position_indic(u, pos);
        } else if (rb_flag_unsafe(cat as u32) &
            (category_flag(category::SM) |
                category_flag(category::A) |
                category_flag(category::SYMBOL))) != 0
        {
            pos = position::SMVD;
        }

        // Oriya Bindu is BeforeSub in the spec.
        if u == 0x0B01 {
            pos = position::BEFORE_SUB;
        }

        self.set_indic_category(cat);
        self.set_indic_position(pos);
    }
}


fn collect_features(planner: &mut ShapePlanner) {
    // Do this before any lookups have been applied.
    planner.ot_map.add_gsub_pause(Some(setup_syllables));

    planner.ot_map.enable_feature(feature::LOCALIZED_FORMS, FeatureFlags::empty(), 1);
    // The Indic specs do not require ccmp, but we apply it here since if
    // there is a use of it, it's typically at the beginning.
    planner.ot_map.enable_feature(feature::GLYPH_COMPOSITION_DECOMPOSITION, FeatureFlags::empty(), 1);

    planner.ot_map.add_gsub_pause(Some(initial_reordering));

    for feature in INDIC_FEATURES.iter().take(10) {
        planner.ot_map.add_feature(feature.0, feature.1, 1);
        planner.ot_map.add_gsub_pause(None);
    }

    planner.ot_map.add_gsub_pause(Some(final_reordering));

    for feature in INDIC_FEATURES.iter().skip(10) {
        planner.ot_map.add_feature(feature.0, feature.1, 1);
    }

    planner.ot_map.enable_feature(feature::CONTEXTUAL_ALTERNATES, FeatureFlags::empty(), 1);
    planner.ot_map.enable_feature(feature::CONTEXTUAL_LIGATURES, FeatureFlags::empty(), 1);

    planner.ot_map.add_gsub_pause(Some(crate::ot::clear_syllables));
}

fn override_features(planner: &mut ShapePlanner) {
    planner.ot_map.disable_feature(feature::STANDARD_LIGATURES);
}

fn preprocess_text(_: &ShapePlan, _: &Face, buffer: &mut Buffer) {
    super::vowel_constraints::preprocess_text_vowel_constraints(buffer);
}

fn decompose(ctx: &ShapeNormalizeContext, ab: char) -> Option<(char, char)> {
    // Don't decompose these.
    match ab {
        '\u{0931}' |               // DEVANAGARI LETTER RRA
        // https://github.com/harfbuzz/harfbuzz/issues/779
        '\u{09DC}' |               // BENGALI LETTER RRA
        '\u{09DD}' |               // BENGALI LETTER RHA
        '\u{0B94}' => return None, // TAMIL LETTER AU
        _ => {}
    }

    if ab == '\u{0DDA}' || ('\u{0DDC}'..='\u{0DDE}').contains(&ab) {
        // Sinhala split matras...  Let the fun begin.
        //
        // These four characters have Unicode decompositions.  However, Uniscribe
        // decomposes them "Khmer-style", that is, it uses the character itself to
        // get the second half.  The first half of all four decompositions is always
        // U+0DD9.
        //
        // Now, there are buggy fonts, namely, the widely used lklug.ttf, that are
        // broken with Uniscribe.  But we need to support them.  As such, we only
        // do the Uniscribe-style decomposition if the character is transformed into
        // its "sec.half" form by the 'pstf' feature.  Otherwise, we fall back to
        // Unicode decomposition.
        //
        // Note that we can't unconditionally use Unicode decomposition.  That would
        // break some other fonts, that are designed to work with Uniscribe, and
        // don't have positioning features for the Unicode-style decomposition.
        //
        // Argh...
        //
        // The Uniscribe behavior is now documented in the newly published Sinhala
        // spec in 2012:
        //
        //   https://docs.microsoft.com/en-us/typography/script-development/sinhala#shaping

        let mut ok = false;
        if let Some(g) = ctx.face.glyph_index(u32::from(ab)) {
            let indic_plan = ctx.plan.data::<IndicShapePlan>();
            ok = indic_plan.pstf.would_substitute(&ctx.plan.ot_map, ctx.face, &[g]);
        }

        if ok {
            // Ok, safe to use Uniscribe-style decomposition.
            return Some(('\u{0DD9}', ab));
        }
    }

    crate::unicode::decompose(ab)
}

fn compose(_: &ShapeNormalizeContext, a: char, b: char) -> Option<char> {
    // Avoid recomposing split matras.
    if a.general_category().is_mark() {
        return None;
    }

    // Composition-exclusion exceptions that we want to recompose.
    if a == '\u{09AF}' && b == '\u{09BC}' {
        return Some('\u{09DF}');
    }

    crate::unicode::compose(a, b)
}

fn setup_masks(_: &ShapePlan, _: &Face, buffer: &mut Buffer) {
    // We cannot setup masks here.  We save information about characters
    // and setup masks later on in a pause-callback.
    for info in buffer.info_slice_mut() {
        info.set_indic_properties();
    }
}

fn setup_syllables(_: &ShapePlan, _: &Face, buffer: &mut Buffer) {
    super::indic_machine::find_syllables_indic(buffer);

    let mut start = 0;
    let mut end = buffer.next_syllable(0);
    while start < buffer.len {
        buffer.unsafe_to_break(start, end);
        start = end;
        end = buffer.next_syllable(start);
    }
}

fn initial_reordering(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    let indic_plan = plan.data::<IndicShapePlan>();

    update_consonant_positions(plan, indic_plan, face, buffer);
    insert_dotted_circles(face, buffer);

    let mut start = 0;
    let mut end = buffer.next_syllable(0);
    while start < buffer.len {
        initial_reordering_syllable(plan, indic_plan, face, start, end, buffer);
        start = end;
        end = buffer.next_syllable(start);
    }
}

fn update_consonant_positions(
    plan: &ShapePlan,
    indic_plan: &IndicShapePlan,
    face: &Face,
    buffer: &mut Buffer,
) {
    if indic_plan.config.base_pos != BasePosition::Last {
        return;
    }

    let mut virama_glyph = None;
    if indic_plan.config.virama != 0 {
        virama_glyph = face.glyph_index(indic_plan.config.virama);
    }

    if let Some(virama) = virama_glyph {
        for info in buffer.info_slice_mut() {
            if info.indic_position() == position::BASE_C {
                let consonant = info.as_glyph();
                info.set_indic_position(
                    consonant_position_from_face(plan, indic_plan, face, consonant, virama));
            }
        }
    }
}

fn consonant_position_from_face(
    plan: &ShapePlan,
    indic_plan: &IndicShapePlan,
    face: &Face,
    consonant: GlyphId,
    virama: GlyphId,
) -> u8 {
    // For old-spec, the order of glyphs is Consonant,Virama,
    // whereas for new-spec, it's Virama,Consonant.  However,
    // some broken fonts (like Free Sans) simply copied lookups
    // from old-spec to new-spec without modification.
    // And oddly enough, Uniscribe seems to respect those lookups.
    // Eg. in the sequence U+0924,U+094D,U+0930, Uniscribe finds
    // base at 0.  The font however, only has lookups matching
    // 930,94D in 'blwf', not the expected 94D,930 (with new-spec
    // table).  As such, we simply match both sequences.  Seems
    // to work.
    //
    // Vatu is done as well, for:
    // https://github.com/harfbuzz/harfbuzz/issues/1587

    if  indic_plan.blwf.would_substitute(&plan.ot_map, face, &[virama, consonant]) ||
        indic_plan.blwf.would_substitute(&plan.ot_map, face, &[consonant, virama]) ||
        indic_plan.vatu.would_substitute(&plan.ot_map, face, &[virama, consonant]) ||
        indic_plan.vatu.would_substitute(&plan.ot_map, face, &[consonant, virama])
    {
        return position::BELOW_C;
    }

    if indic_plan.pstf.would_substitute(&plan.ot_map, face, &[virama, consonant]) ||
       indic_plan.pstf.would_substitute(&plan.ot_map, face, &[consonant, virama])
    {
        return position::POST_C;
    }

    if indic_plan.pref.would_substitute(&plan.ot_map, face, &[virama, consonant]) ||
       indic_plan.pref.would_substitute(&plan.ot_map, face, &[consonant, virama])
    {
        return position::POST_C;
    }

    position::BASE_C
}

fn insert_dotted_circles(face: &Face, buffer: &mut Buffer) {
    use super::indic_machine::SyllableType;

    if buffer.flags.contains(BufferFlags::DO_NOT_INSERT_DOTTED_CIRCLE) {
        return;
    }

    // Note: This loop is extra overhead, but should not be measurable.
    // TODO Use a buffer scratch flag to remove the loop.
    let has_broken_syllables = buffer.info_slice().iter()
        .any(|info| info.syllable() & 0x0F == SyllableType::BrokenCluster as u8);

    if !has_broken_syllables {
        return;
    }

    let dottedcircle_glyph = match face.glyph_index(0x25CC) {
        Some(g) => g.0 as u32,
        None => return,
    };

    let mut dottedcircle = GlyphInfo {
        glyph_id: 0x25CC,
        ..GlyphInfo::default()
    };
    dottedcircle.set_indic_properties();
    dottedcircle.glyph_id = dottedcircle_glyph;

    buffer.clear_output();

    buffer.idx = 0;
    let mut last_syllable = 0;
    while buffer.idx < buffer.len {
        let syllable = buffer.cur(0).syllable();
        let syllable_type = syllable & 0x0F;
        if last_syllable != syllable && syllable_type == SyllableType::BrokenCluster as u8 {
            last_syllable = syllable;

            let mut ginfo = dottedcircle;
            ginfo.cluster = buffer.cur(0).cluster;
            ginfo.mask = buffer.cur(0).mask;
            ginfo.set_syllable(buffer.cur(0).syllable());

            // Insert dottedcircle after possible Repha.
            while buffer.idx < buffer.len &&
                last_syllable == buffer.cur(0).syllable() &&
                buffer.cur(0).indic_category() == category::REPHA
            {
                buffer.next_glyph();
            }

            buffer.output_info(ginfo);
        } else {
            buffer.next_glyph();
        }
    }

    buffer.swap_buffers();
}

fn initial_reordering_syllable(
    plan: &ShapePlan,
    indic_plan: &IndicShapePlan,
    face: &Face,
    start: usize,
    end: usize,
    buffer: &mut Buffer,
) {
    use super::indic_machine::SyllableType;

    let syllable_type = match buffer.info[start].syllable() & 0x0F {
        0 => SyllableType::ConsonantSyllable,
        1 => SyllableType::VowelSyllable,
        2 => SyllableType::StandaloneCluster,
        3 => SyllableType::SymbolCluster,
        4 => SyllableType::BrokenCluster,
        5 => SyllableType::NonIndicCluster,
        _ => unreachable!(),
    };

    match syllable_type {
        // We made the vowels look like consonants.  So let's call the consonant logic!
        SyllableType::VowelSyllable | SyllableType::ConsonantSyllable => {
            initial_reordering_consonant_syllable(plan, indic_plan, face, start, end, buffer);
        }
        // We already inserted dotted-circles, so just call the standalone_cluster.
        SyllableType::BrokenCluster | SyllableType::StandaloneCluster => {
            initial_reordering_standalone_cluster(plan, indic_plan, face, start, end, buffer);
        }
        SyllableType::SymbolCluster | SyllableType::NonIndicCluster => {}
    }
}

// Rules from:
// https://docs.microsqoft.com/en-us/typography/script-development/devanagari */
fn initial_reordering_consonant_syllable(
    plan: &ShapePlan,
    indic_plan: &IndicShapePlan,
    face: &Face,
    start: usize,
    end: usize,
    buffer: &mut Buffer,
) {
    // https://github.com/harfbuzz/harfbuzz/issues/435#issuecomment-335560167
    // For compatibility with legacy usage in Kannada,
    // Ra+h+ZWJ must behave like Ra+ZWJ+h...
    if buffer.script == Some(script::KANNADA) &&
        start + 3 <= end &&
        buffer.info[start].is_one_of(category_flag(category::RA)) &&
        buffer.info[start + 1].is_one_of(category_flag(category::H)) &&
        buffer.info[start + 2].is_one_of(category_flag(category::ZWJ))
    {
        buffer.merge_clusters(start + 1, start + 3);
        buffer.info.swap(start + 1, start + 2);
    }

    // 1. Find base consonant:
    //
    // The shaping engine finds the base consonant of the syllable, using the
    // following algorithm: starting from the end of the syllable, move backwards
    // until a consonant is found that does not have a below-base or post-base
    // form (post-base forms have to follow below-base forms), or that is not a
    // pre-base-reordering Ra, or arrive at the first consonant. The consonant
    // stopped at will be the base.
    //
    //   - If the syllable starts with Ra + Halant (in a script that has Reph)
    //     and has more than one consonant, Ra is excluded from candidates for
    //     base consonants.

    let mut base = end;
    let mut has_reph = false;

    {
        // -> If the syllable starts with Ra + Halant (in a script that has Reph)
        //    and has more than one consonant, Ra is excluded from candidates for
        //    base consonants.
        let mut limit = start;
        if indic_plan.mask_array[indic_feature::RPHF] != 0 &&
            start + 3 <= end &&
            ((indic_plan.config.reph_mode == RephMode::Implicit && !buffer.info[start + 2].is_joiner()) ||
                (indic_plan.config.reph_mode == RephMode::Explicit && buffer.info[start + 2].indic_category() == category::ZWJ))
        {
            // See if it matches the 'rphf' feature.
            let glyphs = &[
                buffer.info[start].as_glyph(),
                buffer.info[start + 1].as_glyph(),
                if indic_plan.config.reph_mode == RephMode::Explicit {
                    buffer.info[start + 2].as_glyph()
                } else {
                    GlyphId(0)
                }
            ];
            if indic_plan.rphf.would_substitute(&plan.ot_map, face, &glyphs[0..2]) ||
                (indic_plan.config.reph_mode == RephMode::Explicit &&
                    indic_plan.rphf.would_substitute(&plan.ot_map, face, glyphs))
            {
                limit += 2;
                while limit < end && buffer.info[limit].is_joiner() {
                    limit += 1;
                }
                base = start;
                has_reph = true;
            }
        } else if indic_plan.config.reph_mode == RephMode::LogRepha &&
            buffer.info[start].indic_category() == category::REPHA
        {
            limit += 1;
            while limit < end && buffer.info[limit].is_joiner() {
                limit += 1;
            }
            base = start;
            has_reph = true;
        }

        match indic_plan.config.base_pos {
            BasePosition::Last => {
                // -> starting from the end of the syllable, move backwards
                let mut i = end;
                let mut seen_below = false;
                loop {
                    i -= 1;
                    // -> until a consonant is found
                    if buffer.info[i].is_consonant() {
                        // -> that does not have a below-base or post-base form
                        // (post-base forms have to follow below-base forms),
                        if buffer.info[i].indic_position() != position::BELOW_C &&
                            (buffer.info[i].indic_position() != position::POST_C || seen_below)
                        {
                            base = i;
                            break;
                        }
                        if buffer.info[i].indic_position() == position::BELOW_C {
                            seen_below = true;
                        }

                        // -> or that is not a pre-base-reordering Ra,
                        //
                        // IMPLEMENTATION NOTES:
                        //
                        // Our pre-base-reordering Ra's are marked position::PostC, so will be skipped
                        // by the logic above already.

                        // -> or arrive at the first consonant. The consonant stopped at will
                        // be the base.
                        base = i;
                    } else {
                        // A ZWJ after a Halant stops the base search, and requests an explicit
                        // half form.
                        // A ZWJ before a Halant, requests a subjoined form instead, and hence
                        // search continues.  This is particularly important for Bengali
                        // sequence Ra,H,Ya that should form Ya-Phalaa by subjoining Ya.
                        if start < i && buffer.info[i].indic_category() == category::ZWJ &&
                            buffer.info[i - 1].indic_category() == category::H
                        {
                            break;
                        }
                    }

                    if i <= limit {
                        break;
                    }
                }
            }
            BasePosition::LastSinhala => {
                // Sinhala base positioning is slightly different from main Indic, in that:
                // 1. Its ZWJ behavior is different,
                // 2. We don't need to look into the font for consonant positions.

                if !has_reph {
                    base = limit;
                }

                // Find the last base consonant that is not blocked by ZWJ.  If there is
                // a ZWJ right before a base consonant, that would request a subjoined form.
                for i in limit..end {
                    if buffer.info[i].is_consonant() {
                        if limit < i && buffer.info[i - 1].indic_category() == category::ZWJ {
                            break;
                        } else {
                            base = i;
                        }
                    }
                }

                // Mark all subsequent consonants as below.
                for i in base+1..end {
                    if buffer.info[i].is_consonant() {
                        buffer.info[i].set_indic_position(position::BELOW_C);
                    }
                }
            }
        }

        // -> If the syllable starts with Ra + Halant (in a script that has Reph)
        //    and has more than one consonant, Ra is excluded from candidates for
        //    base consonants.
        //
        //  Only do this for unforced Reph. (ie. not for Ra,H,ZWJ.
        if has_reph && base == start && limit - base <= 2 {
            // Have no other consonant, so Reph is not formed and Ra becomes base.
            has_reph = false;
        }
    }

    // 2. Decompose and reorder Matras:
    //
    // Each matra and any syllable modifier sign in the syllable are moved to the
    // appropriate position relative to the consonant(s) in the syllable. The
    // shaping engine decomposes two- or three-part matras into their constituent
    // parts before any repositioning. Matra characters are classified by which
    // consonant in a conjunct they have affinity for and are reordered to the
    // following positions:
    //
    //   - Before first half form in the syllable
    //   - After subjoined consonants
    //   - After post-form consonant
    //   - After main consonant (for above marks)
    //
    // IMPLEMENTATION NOTES:
    //
    // The normalize() routine has already decomposed matras for us, so we don't
    // need to worry about that.

    // 3.  Reorder marks to canonical order:
    //
    // Adjacent nukta and halant or nukta and vedic sign are always repositioned
    // if necessary, so that the nukta is first.
    //
    // IMPLEMENTATION NOTES:
    //
    // We don't need to do this: the normalize() routine already did this for us.

    // Reorder characters

    for i in start..base {
        let pos = buffer.info[i].indic_position();
        buffer.info[i].set_indic_position(cmp::min(position::PRE_C, pos));
    }

    if base < end {
        buffer.info[base].set_indic_position(position::BASE_C);
    }

    // Mark final consonants.  A final consonant is one appearing after a matra.
    // Happens in Sinhala.
    for i in base+1..end {
        if buffer.info[i].indic_category() == category::M {
            for j in i+1..end {
                if buffer.info[j].is_consonant() {
                    buffer.info[j].set_indic_position(position::FINAL_C);
                    break;
                }
            }

            break;
        }
    }

    // Handle beginning Ra
    if has_reph {
        buffer.info[start].set_indic_position(position::RA_TO_BECOME_REPH);
    }

    // For old-style Indic script tags, move the first post-base Halant after
    // last consonant.
    //
    // Reports suggest that in some scripts Uniscribe does this only if there
    // is *not* a Halant after last consonant already.  We know that is the
    // case for Kannada, while it reorders unconditionally in other scripts,
    // eg. Malayalam, Bengali, and Devanagari.  We don't currently know about
    // other scripts, so we block Kannada.
    //
    // Kannada test case:
    // U+0C9A,U+0CCD,U+0C9A,U+0CCD
    // With some versions of Lohit Kannada.
    // https://bugs.freedesktop.org/show_bug.cgi?id=59118
    //
    // Malayalam test case:
    // U+0D38,U+0D4D,U+0D31,U+0D4D,U+0D31,U+0D4D
    // With lohit-ttf-20121122/Lohit-Malayalam.ttf
    //
    // Bengali test case:
    // U+0998,U+09CD,U+09AF,U+09CD
    // With Windows XP vrinda.ttf
    // https://github.com/harfbuzz/harfbuzz/issues/1073
    //
    // Devanagari test case:
    // U+091F,U+094D,U+0930,U+094D
    // With chandas.ttf
    // https://github.com/harfbuzz/harfbuzz/issues/1071
    if indic_plan.is_old_spec {
        let disallow_double_halants = buffer.script == Some(script::KANNADA);
        for i in base+1..end {
            if buffer.info[i].indic_category() == category::H {
                let mut j = end - 1;
                while j > i {
                    if buffer.info[j].is_consonant() ||
                        (disallow_double_halants && buffer.info[j].indic_category() == category::H)
                    {
                        break;
                    }

                    j -= 1;
                }

                if buffer.info[j].indic_category() != category::H && j > i {
                    // Move Halant to after last consonant.
                    let t = buffer.info[i];
                    for k in 0..j-i {
                        buffer.info[k + i] = buffer.info[k+ i + 1];
                    }
                    buffer.info[j] = t;
                }

                break;
            }
        }
    }

    // Attach misc marks to previous char to move with them.
    {
        let mut last_pos = position::START;
        for i in start..end {
            let ok = rb_flag_unsafe(buffer.info[i].indic_category() as u32) &
                (category_flag(category::ZWJ) | category_flag(category::ZWNJ) |
                    category_flag(category::N) | category_flag(category::RS) |
                    category_flag(category::CM) | category_flag(category::H)
                ) != 0;
            if ok {
                buffer.info[i].set_indic_position(last_pos);

                if buffer.info[i].indic_category() == category::H &&
                    buffer.info[i].indic_position() == position::PRE_M
                {
                    // Uniscribe doesn't move the Halant with Left Matra.
                    // TEST: U+092B,U+093F,U+094DE
                    // We follow.  This is important for the Sinhala
                    // U+0DDA split matra since it decomposes to U+0DD9,U+0DCA
                    // where U+0DD9 is a left matra and U+0DCA is the virama.
                    // We don't want to move the virama with the left matra.
                    // TEST: U+0D9A,U+0DDA
                    for j in (start+1..=i).rev() {
                        if buffer.info[j - 1].indic_position() != position::PRE_M {
                            let pos = buffer.info[j - 1].indic_position();
                            buffer.info[i].set_indic_position(pos);
                            break;
                        }
                    }
                }
            } else if buffer.info[i].indic_position() != position::SMVD {
                last_pos = buffer.info[i].indic_position();
            }
        }
    }
    // For post-base consonants let them own anything before them
    // since the last consonant or matra.
    {
        let mut last = base;
        for i in base+1..end {
            if buffer.info[i].is_consonant() {
                for j in last+1..i {
                    if (buffer.info[j].indic_position() as u8) < (position::SMVD as u8) {
                        let pos = buffer.info[i].indic_position();
                        buffer.info[j].set_indic_position(pos);
                    }
                }

                last = i;
            } else if buffer.info[i].indic_category() == category::M {
                last = i;
            }
        }
    }

    {
        // Use syllable() for sort accounting temporarily.
        let syllable = buffer.info[start].syllable();
        for i in start..end {
            buffer.info[i].set_syllable(u8::try_from(i - start).unwrap());
        }

        buffer.info[start..end].sort_by(|a, b| a.indic_position().cmp(&b.indic_position()));

        // Find base again.
        base = end;
        for i in start..end {
            if buffer.info[i].indic_position() == position::BASE_C {
                base = i;
                break;
            }
        }
        // Things are out-of-control for post base positions, they may shuffle
        // around like crazy.  In old-spec mode, we move halants around, so in
        // that case merge all clusters after base.  Otherwise, check the sort
        // order and merge as needed.
        // For pre-base stuff, we handle cluster issues in final reordering.
        //
        // We could use buffer->sort() for this, if there was no special
        // reordering of pre-base stuff happening later...
        // We don't want to merge_clusters all of that, which buffer->sort()
        // would.
        if indic_plan.is_old_spec || end - start > 127 {
            buffer.merge_clusters(base, end);
        } else {
            // Note! syllable() is a one-byte field.
            for i in base..end {
                if buffer.info[i].syllable() != 255 {
                    let mut max = i;
                    let mut j = start + buffer.info[i].syllable() as usize;
                    while j != i {
                        max = cmp::max(max, j);
                        let next = start + buffer.info[j].syllable() as usize;
                        buffer.info[j].set_syllable(255); // So we don't process j later again.
                        j = next;
                    }

                    if i != max {
                        buffer.merge_clusters(i, max + 1);
                    }
                }
            }
        }

        // Put syllable back in.
        for info in &mut buffer.info[start..end] {
            info.set_syllable(syllable);
        }
    }

    // Setup masks now

    {
        // Reph
        for info in &mut buffer.info[start..end] {
            if info.indic_position() != position::RA_TO_BECOME_REPH {
                break;
            }

            info.mask |= indic_plan.mask_array[indic_feature::RPHF];
        }

        // Pre-base
        let mut mask = indic_plan.mask_array[indic_feature::HALF];
        if !indic_plan.is_old_spec && indic_plan.config.blwf_mode == BlwfMode::PreAndPost {
            mask |= indic_plan.mask_array[indic_feature::BLWF];
        }

        for info in &mut buffer.info[start..base] {
            info.mask |= mask;
        }

        // Base
        mask = 0;
        if base < end {
            buffer.info[base].mask |= mask;
        }

        // Post-base
        mask = indic_plan.mask_array[indic_feature::BLWF] |
            indic_plan.mask_array[indic_feature::ABVF] |
            indic_plan.mask_array[indic_feature::PSTF];
        for i in base+1..end {
            buffer.info[i].mask |= mask;
        }
    }

    if indic_plan.is_old_spec && buffer.script == Some(script::DEVANAGARI) {
        // Old-spec eye-lash Ra needs special handling.  From the
        // spec:
        //
        // "The feature 'below-base form' is applied to consonants
        // having below-base forms and following the base consonant.
        // The exception is vattu, which may appear below half forms
        // as well as below the base glyph. The feature 'below-base
        // form' will be applied to all such occurrences of Ra as well."
        //
        // Test case: U+0924,U+094D,U+0930,U+094d,U+0915
        // with Sanskrit 2003 font.
        //
        // However, note that Ra,Halant,ZWJ is the correct way to
        // request eyelash form of Ra, so we wouldbn't inhibit it
        // in that sequence.
        //
        // Test case: U+0924,U+094D,U+0930,U+094d,U+200D,U+0915
        for i in start..base.saturating_sub(1) {
            if buffer.info[i].indic_category() == category::RA &&
                buffer.info[i + 1].indic_category() == category::H &&
                (i + 2 == base || buffer.info[i + 2].indic_category() != category::ZWJ)
            {
                buffer.info[i].mask |= indic_plan.mask_array[indic_feature::BLWF];
                buffer.info[i + 1].mask |= indic_plan.mask_array[indic_feature::BLWF];
            }
        }
    }

    let pref_len = 2;
    if indic_plan.mask_array[indic_feature::PREF] != 0 && base + pref_len < end {
        // Find a Halant,Ra sequence and mark it for pre-base-reordering processing.
        for i in base+1..end-pref_len+1 {
            let glyphs = &[
                buffer.info[i + 0].as_glyph(),
                buffer.info[i + 1].as_glyph(),
            ];
            if indic_plan.pref.would_substitute(&plan.ot_map, face, glyphs) {
                buffer.info[i + 0].mask = indic_plan.mask_array[indic_feature::PREF];
                buffer.info[i + 1].mask = indic_plan.mask_array[indic_feature::PREF];
                break;
            }
        }
    }

    // Apply ZWJ/ZWNJ effects
    for i in start+1..end {
        if buffer.info[i].is_joiner() {
            let non_joiner = buffer.info[i].indic_category() == category::ZWNJ;
            let mut j = i;

            loop {
                j -= 1;

                // ZWJ/ZWNJ should disable CJCT.  They do that by simply
                // being there, since we don't skip them for the CJCT
                // feature (ie. F_MANUAL_ZWJ)

                // A ZWNJ disables HALF.
                if non_joiner {
                    buffer.info[j].mask &= !indic_plan.mask_array[indic_feature::HALF];
                }

                if j <= start || buffer.info[j].is_consonant() {
                    break;
                }
            }
        }
    }
}

fn initial_reordering_standalone_cluster(
    plan: &ShapePlan,
    indic_plan: &IndicShapePlan,
    face: &Face,
    start: usize,
    end: usize,
    buffer: &mut Buffer,
) {
    // We treat placeholder/dotted-circle as if they are consonants, so we
    // should just chain.  Only if not in compatibility mode that is...
    initial_reordering_consonant_syllable(plan, indic_plan, face, start, end, buffer);
}

fn final_reordering(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    if buffer.is_empty() {
        return;
    }

    let indic_plan = plan.data::<IndicShapePlan>();

    let mut virama_glyph = None;
    if indic_plan.config.virama != 0 {
        if let Some(g) = face.glyph_index(indic_plan.config.virama) {
            virama_glyph = Some(g.0 as u32);
        }
    }

    let mut start = 0;
    let mut end = buffer.next_syllable(0);
    while start < buffer.len {
        final_reordering_impl(indic_plan, virama_glyph, start, end, buffer);
        start = end;
        end = buffer.next_syllable(start);
    }
}

fn final_reordering_impl(
    plan: &IndicShapePlan,
    virama_glyph: Option<u32>,
    start: usize,
    end: usize,
    buffer: &mut Buffer,
) {
    // This function relies heavily on halant glyphs.  Lots of ligation
    // and possibly multiple substitutions happened prior to this
    // phase, and that might have messed up our properties.  Recover
    // from a particular case of that where we're fairly sure that a
    // class of OT_H is desired but has been lost.
    //
    // We don't call load_virama_glyph(), since we know it's already loaded.
    if let Some(virama_glyph) = virama_glyph {
        for info in &mut buffer.info[start..end] {
            if info.glyph_id == virama_glyph && info.is_ligated() && info.is_multiplied() {
                // This will make sure that this glyph passes is_halant() test.
                info.set_indic_category(category::H);
                info.clear_ligated_and_multiplied();
            }
        }
    }

    // 4. Final reordering:
    //
    // After the localized forms and basic shaping forms GSUB features have been
    // applied (see below), the shaping engine performs some final glyph
    // reordering before applying all the remaining font features to the entire
    // syllable.

    let mut try_pref = plan.mask_array[indic_feature::PREF] != 0;

    let mut base = start;
    while base < end {
        if buffer.info[base].indic_position() as u32 >= position::BASE_C as u32 {
            if try_pref && base + 1 < end {
                for i in base+1..end {
                    if (buffer.info[i].mask & plan.mask_array[indic_feature::PREF]) != 0 {
                        if !(buffer.info[i].is_substituted() && buffer.info[i].is_ligated_and_didnt_multiply()) {
                            // Ok, this was a 'pref' candidate but didn't form any.
                            // Base is around here...
                            base = i;
                            while base < end && buffer.info[base].is_halant() {
                                base += 1;
                            }

                            buffer.info[base].set_indic_position(position::BASE_C);
                            try_pref = false;
                        }

                        break;
                    }
                }
            }

            // For Malayalam, skip over unformed below- (but NOT post-) forms.
            if buffer.script == Some(script::MALAYALAM) {
                let mut i = base + 1;
                while i < end {
                    while i < end && buffer.info[i].is_joiner() {
                        i += 1;
                    }

                    if i == end || !buffer.info[i].is_halant() {
                        break;
                    }

                    i += 1; // Skip halant.

                    while i < end && buffer.info[i].is_joiner() {
                        i += 1;
                    }

                    if i < end && buffer.info[i].is_consonant() && buffer.info[i].indic_position() == position::BELOW_C {
                        base = i;
                        buffer.info[base].set_indic_position(position::BASE_C);
                    }

                    i += 1;
                }
            }

            if start < base && buffer.info[base].indic_position() as u32 > position::BASE_C as u32 {
                base -= 1;
            }

            break;
        }

        base += 1;
    }

    if base == end && start < base && buffer.info[base - 1].is_one_of(rb_flag(category::ZWJ as u32)) {
        base -= 1;
    }

    if base < end {
        while start < base && buffer.info[base].is_one_of(rb_flag(category::N as u32) | rb_flag(category::H as u32)) {
            base -= 1;
        }
    }

    // - Reorder matras:
    //
    //   If a pre-base matra character had been reordered before applying basic
    //   features, the glyph can be moved closer to the main consonant based on
    //   whether half-forms had been formed. Actual position for the matra is
    //   defined as “after last standalone halant glyph, after initial matra
    //   position and before the main consonant”. If ZWJ or ZWNJ follow this
    //   halant, position is moved after it.
    //
    // IMPLEMENTATION NOTES:
    //
    // It looks like the last sentence is wrong.  Testing, with Windows 7 Uniscribe
    // and Devanagari shows that the behavior is best described as:
    //
    // "If ZWJ follows this halant, matra is NOT repositioned after this halant.
    //  If ZWNJ follows this halant, position is moved after it."
    //
    // Test case, with Adobe Devanagari or Nirmala UI:
    //
    //   U+091F,U+094D,U+200C,U+092F,U+093F
    //   (Matra moves to the middle, after ZWNJ.)
    //
    //   U+091F,U+094D,U+200D,U+092F,U+093F
    //   (Matra does NOT move, stays to the left.)
    //
    // https://github.com/harfbuzz/harfbuzz/issues/1070

    // Otherwise there can't be any pre-base matra characters.
    if start + 1 < end && start < base {
        // If we lost track of base, alas, position before last thingy.
        let mut new_pos = if base == end { base - 2 } else { base - 1 };

        // Malayalam / Tamil do not have "half" forms or explicit virama forms.
        // The glyphs formed by 'half' are Chillus or ligated explicit viramas.
        // We want to position matra after them.
        if buffer.script != Some(script::MALAYALAM) && buffer.script != Some(script::TAMIL) {
            loop {
                while new_pos > start && !buffer.info[new_pos].is_one_of(rb_flag(category::M as u32) | rb_flag(category::H as u32)) {
                    new_pos -= 1;
                }

                // If we found no Halant we are done.
                // Otherwise only proceed if the Halant does
                // not belong to the Matra itself!
                if buffer.info[new_pos].is_halant() && buffer.info[new_pos].indic_position() != position::PRE_M {
                    if new_pos + 1 < end {
                        // -> If ZWJ follows this halant, matra is NOT repositioned after this halant.
                        if buffer.info[new_pos + 1].indic_category() == category::ZWJ {
                            // Keep searching.
                            if new_pos > start {
                                new_pos -= 1;
                                continue;
                            }
                        }

                        // -> If ZWNJ follows this halant, position is moved after it.
                        //
                        // IMPLEMENTATION NOTES:
                        //
                        // This is taken care of by the state-machine. A Halant,ZWNJ is a terminating
                        // sequence for a consonant syllable; any pre-base matras occurring after it
                        // will belong to the subsequent syllable.
                    }
                } else {
                    new_pos = start; // No move.
                }

                break;
            }
        }

        if start < new_pos && buffer.info[new_pos].indic_position() != position::PRE_M {
            // Now go see if there's actually any matras...
            for i in (start+1..=new_pos).rev() {
                if buffer.info[i - 1].indic_position() == position::PRE_M {
                    let old_pos = i - 1;
                    // Shouldn't actually happen.
                    if old_pos < base && base <= new_pos {
                        base -= 1;
                    }

                    let tmp = buffer.info[old_pos];
                    for i in 0..new_pos-old_pos {
                        buffer.info[i + old_pos] = buffer.info[i + old_pos + 1];
                    }
                    buffer.info[new_pos] = tmp;

                    // Note: this merge_clusters() is intentionally *after* the reordering.
                    // Indic matra reordering is special and tricky...
                    buffer.merge_clusters(new_pos, cmp::min(end, base + 1));

                    new_pos -= 1;
                }
            }
        } else {
            for i in start..base {
                if buffer.info[i].indic_position() == position::PRE_M {
                    buffer.merge_clusters(i, cmp::min(end, base + 1));
                    break;
                }
            }
        }
    }

    // - Reorder reph:
    //
    //   Reph’s original position is always at the beginning of the syllable,
    //   (i.e. it is not reordered at the character reordering stage). However,
    //   it will be reordered according to the basic-forms shaping results.
    //   Possible positions for reph, depending on the script, are; after main,
    //   before post-base consonant forms, and after post-base consonant forms.

    // Two cases:
    //
    // - If repha is encoded as a sequence of characters (Ra,H or Ra,H,ZWJ), then
    //   we should only move it if the sequence ligated to the repha form.
    //
    // - If repha is encoded separately and in the logical position, we should only
    //   move it if it did NOT ligate.  If it ligated, it's probably the font trying
    //   to make it work without the reordering.

    if start + 1 < end && buffer.info[start].indic_position() == position::RA_TO_BECOME_REPH &&
        (buffer.info[start].indic_category() == category::REPHA) ^ buffer.info[start].is_ligated_and_didnt_multiply()
    {
        let mut new_reph_pos;
        loop {
            let reph_pos = plan.config.reph_pos;

            // 1. If reph should be positioned after post-base consonant forms,
            //    proceed to step 5.
            if reph_pos != RephPosition::AfterPost {
                // 2. If the reph repositioning class is not after post-base: target
                //    position is after the first explicit halant glyph between the
                //    first post-reph consonant and last main consonant. If ZWJ or ZWNJ
                //    are following this halant, position is moved after it. If such
                //    position is found, this is the target position. Otherwise,
                //    proceed to the next step.
                //
                //    Note: in old-implementation fonts, where classifications were
                //    fixed in shaping engine, there was no case where reph position
                //    will be found on this step.
                {
                    new_reph_pos = start + 1;
                    while new_reph_pos < base && !buffer.info[new_reph_pos].is_halant() {
                        new_reph_pos += 1;
                    }

                    if new_reph_pos < base && buffer.info[new_reph_pos].is_halant() {
                        // ->If ZWJ or ZWNJ are following this halant, position is moved after it.
                        if new_reph_pos + 1 < base && buffer.info[new_reph_pos + 1].is_joiner() {
                            new_reph_pos += 1;
                        }

                        break;
                    }
                }

                // 3. If reph should be repositioned after the main consonant: find the
                //    first consonant not ligated with main, or find the first
                //    consonant that is not a potential pre-base-reordering Ra.
                if reph_pos == RephPosition::AfterMain {
                    new_reph_pos = base;
                    while new_reph_pos + 1 < end && buffer.info[new_reph_pos + 1].indic_position() as u8 <= position::AFTER_MAIN as u8 {
                        new_reph_pos += 1;
                    }

                    if new_reph_pos < end {
                        break;
                    }
                }

                // 4. If reph should be positioned before post-base consonant, find
                //    first post-base classified consonant not ligated with main. If no
                //    consonant is found, the target position should be before the
                //    first matra, syllable modifier sign or vedic sign.
                //
                // This is our take on what step 4 is trying to say (and failing, BADLY).
                if reph_pos == RephPosition::AfterSub {
                    new_reph_pos = base;
                    while new_reph_pos + 1 < end &&
                        (rb_flag_unsafe(buffer.info[new_reph_pos + 1].indic_position() as u32)
                            & (rb_flag(position::POST_C as u32) | rb_flag(position::AFTER_POST as u32) | rb_flag(position::SMVD as u32))) == 0
                    {
                        new_reph_pos += 1;
                    }

                    if new_reph_pos < end {
                        break;
                    }
                }
            }

            // 5. If no consonant is found in steps 3 or 4, move reph to a position
            //    immediately before the first post-base matra, syllable modifier
            //    sign or vedic sign that has a reordering class after the intended
            //    reph position. For example, if the reordering position for reph
            //    is post-main, it will skip above-base matras that also have a
            //    post-main position.
            //
            // Copied from step 2.
            new_reph_pos = start + 1;
            while new_reph_pos < base && !buffer.info[new_reph_pos].is_halant() {
                new_reph_pos += 1;
            }

            if new_reph_pos < base && buffer.info[new_reph_pos].is_halant() {
                /* ->If ZWJ or ZWNJ are following this halant, position is moved after it. */
                if new_reph_pos + 1 < base && buffer.info[new_reph_pos + 1].is_joiner() {
                    new_reph_pos += 1;
                }

                break;
            }
            // See https://github.com/harfbuzz/harfbuzz/issues/2298#issuecomment-615318654

            // 6. Otherwise, reorder reph to the end of the syllable.
            {
                new_reph_pos = end - 1;
                while new_reph_pos > start && buffer.info[new_reph_pos].indic_position() == position::SMVD {
                    new_reph_pos -= 1;
                }

                // If the Reph is to be ending up after a Matra,Halant sequence,
                // position it before that Halant so it can interact with the Matra.
                // However, if it's a plain Consonant,Halant we shouldn't do that.
                // Uniscribe doesn't do this.
                // TEST: U+0930,U+094D,U+0915,U+094B,U+094D
                if buffer.info[new_reph_pos].is_halant() {
                    for info in &buffer.info[base+1..new_reph_pos] {
                        if info.indic_category() == category::M {
                            // Ok, got it.
                            new_reph_pos -= 1;
                        }
                    }
                }
            }

            break;
        }

        // Move
        buffer.merge_clusters(start, new_reph_pos + 1);

        let reph = buffer.info[start];
        for i in 0..new_reph_pos - start {
            buffer.info[i + start] = buffer.info[i + start + 1];
        }
        buffer.info[new_reph_pos] = reph;

        if start < base && base <= new_reph_pos {
            base -= 1;
        }
    }

    // - Reorder pre-base-reordering consonants:
    //
    //   If a pre-base-reordering consonant is found, reorder it according to
    //   the following rules:

    // Otherwise there can't be any pre-base-reordering Ra.
    if try_pref && base + 1 < end {
        for i in base+1..end {
            if (buffer.info[i].mask & plan.mask_array[indic_feature::PREF]) != 0 {
                // 1. Only reorder a glyph produced by substitution during application
                //    of the <pref> feature. (Note that a font may shape a Ra consonant with
                //    the feature generally but block it in certain contexts.)
                //
                // Note: We just check that something got substituted.  We don't check that
                // the <pref> feature actually did it...
                //
                // Reorder pref only if it ligated.
                if buffer.info[i].is_ligated_and_didnt_multiply() {
                    // 2. Try to find a target position the same way as for pre-base matra.
                    //    If it is found, reorder pre-base consonant glyph.
                    //
                    // 3. If position is not found, reorder immediately before main consonant.

                    let mut new_pos = base;
                    // Malayalam / Tamil do not have "half" forms or explicit virama forms.
                    // The glyphs formed by 'half' are Chillus or ligated explicit viramas.
                    // We want to position matra after them.
                    if buffer.script != Some(script::MALAYALAM) && buffer.script != Some(script::TAMIL) {
                        while new_pos > start && !buffer.info[new_pos - 1].is_one_of(rb_flag(category::M as u32) |
                            rb_flag(category::H as u32))
                        {
                            new_pos -= 1;
                        }
                    }

                    if new_pos > start && buffer.info[new_pos - 1].is_halant() {
                        // -> If ZWJ or ZWNJ follow this halant, position is moved after it.
                        if new_pos < end && buffer.info[new_pos].is_joiner() {
                            new_pos += 1;
                        }
                    }

                    {
                        let old_pos = i;

                        buffer.merge_clusters(new_pos, old_pos + 1);
                        let tmp = buffer.info[old_pos];
                        for i in (0..=old_pos-new_pos).rev() {
                            buffer.info[i + new_pos + 1] = buffer.info[i + new_pos];
                        }
                        buffer.info[new_pos] = tmp;

                        if new_pos <= base && base < old_pos {
                            // TODO: investigate
                            #[allow(unused_assignments)]
                            {
                                base += 1;
                            }
                        }
                    }
                }

                break;
            }
        }
    }

    // Apply 'init' to the Left Matra if it's a word start.
    if buffer.info[start].indic_position() == position::PRE_M {
        if start == 0 || (rb_flag_unsafe(buffer.info[start - 1].general_category().to_rb()) &
            rb_flag_range(hb_gc::RB_UNICODE_GENERAL_CATEGORY_FORMAT, hb_gc::RB_UNICODE_GENERAL_CATEGORY_NON_SPACING_MARK)) == 0
        {
            buffer.info[start].mask |= plan.mask_array[indic_feature::INIT];
        } else {
            buffer.unsafe_to_break(start - 1, start + 1);
        }
    }
}

pub fn get_category_and_position(u: u32) -> (Category, Position) {
    let (c1, c2) = super::indic_table::get_categories(u);
    let c2 = if c1 == SyllabicCategory::ConsonantMedial ||
        c1 == SyllabicCategory::GeminationMark ||
        c1 == SyllabicCategory::RegisterShifter ||
        c1 == SyllabicCategory::ConsonantSucceedingRepha ||
        c1 == SyllabicCategory::Virama ||
        c1 == SyllabicCategory::VowelDependent ||
        false
    {
        c2
    } else {
        MatraCategory::NotApplicable
    };

    let c1 = match c1 {
        SyllabicCategory::Other => category::X,
        SyllabicCategory::Avagraha => category::SYMBOL,
        SyllabicCategory::Bindu => category::SM,
        SyllabicCategory::BrahmiJoiningNumber => category::PLACEHOLDER, // Don't care.
        SyllabicCategory::CantillationMark => category::A,
        SyllabicCategory::Consonant => category::C,
        SyllabicCategory::ConsonantDead => category::C,
        SyllabicCategory::ConsonantFinal => category::CM,
        SyllabicCategory::ConsonantHeadLetter => category::C,
        SyllabicCategory::ConsonantInitialPostfixed => category::PLACEHOLDER,
        SyllabicCategory::ConsonantKiller => category::M, // U+17CD only.
        SyllabicCategory::ConsonantMedial => category::CM,
        SyllabicCategory::ConsonantPlaceholder => category::PLACEHOLDER,
        SyllabicCategory::ConsonantPrecedingRepha => category::REPHA,
        SyllabicCategory::ConsonantPrefixed => category::X,
        SyllabicCategory::ConsonantSubjoined => category::CM,
        SyllabicCategory::ConsonantSucceedingRepha => category::CM,
        SyllabicCategory::ConsonantWithStacker => category::CS,
        SyllabicCategory::GeminationMark => category::SM, // https://github.com/harfbuzz/harfbuzz/issues/552
        SyllabicCategory::InvisibleStacker => category::COENG,
        SyllabicCategory::Joiner => category::ZWJ,
        SyllabicCategory::ModifyingLetter => category::X,
        SyllabicCategory::NonJoiner => category::ZWNJ,
        SyllabicCategory::Nukta => category::N,
        SyllabicCategory::Number => category::PLACEHOLDER,
        SyllabicCategory::NumberJoiner => category::PLACEHOLDER, // Don't care.
        SyllabicCategory::PureKiller => category::M,
        SyllabicCategory::RegisterShifter => category::RS,
        SyllabicCategory::SyllableModifier => category::SM,
        SyllabicCategory::ToneLetter => category::X,
        SyllabicCategory::ToneMark => category::N,
        SyllabicCategory::Virama => category::H,
        SyllabicCategory::Visarga => category::SM,
        SyllabicCategory::Vowel => category::V,
        SyllabicCategory::VowelDependent => category::M,
        SyllabicCategory::VowelIndependent => category::V,
    };

    let c2 = match c2 {
        MatraCategory::NotApplicable => position::END,
        MatraCategory::Left => position::PRE_C,
        MatraCategory::Top => position::ABOVE_C,
        MatraCategory::Bottom => position::BELOW_C,
        MatraCategory::Right => position::POST_C,
        MatraCategory::BottomAndLeft => position::POST_C,
        MatraCategory::BottomAndRight => position::POST_C,
        MatraCategory::LeftAndRight => position::POST_C,
        MatraCategory::TopAndBottom => position::BELOW_C,
        MatraCategory::TopAndBottomAndRight => position::POST_C,
        MatraCategory::TopAndLeft => position::ABOVE_C,
        MatraCategory::TopAndLeftAndRight => position::POST_C,
        MatraCategory::TopAndRight => position::POST_C,
        MatraCategory::Overstruck => position::AFTER_MAIN,
        MatraCategory::VisualOrderLeft => position::PRE_M,
    };

    (c1, c2)
}

fn matra_position_indic(u: u32, side: u8) -> u8 {
    #[inline] fn in_half_block(u: u32, base: u32) -> bool { u & !0x7F == base }
    #[inline] fn is_deva(u: u32) -> bool { in_half_block(u, 0x0900) }
    #[inline] fn is_beng(u: u32) -> bool { in_half_block(u, 0x0980) }
    #[inline] fn is_guru(u: u32) -> bool { in_half_block(u, 0x0A00) }
    #[inline] fn is_gujr(u: u32) -> bool { in_half_block(u, 0x0A80) }
    #[inline] fn is_orya(u: u32) -> bool { in_half_block(u, 0x0B00) }
    #[inline] fn is_taml(u: u32) -> bool { in_half_block(u, 0x0B80) }
    #[inline] fn is_telu(u: u32) -> bool { in_half_block(u, 0x0C00) }
    #[inline] fn is_knda(u: u32) -> bool { in_half_block(u, 0x0C80) }
    #[inline] fn is_mlym(u: u32) -> bool { in_half_block(u, 0x0D00) }
    #[inline] fn is_sinh(u: u32) -> bool { in_half_block(u, 0x0D80) }

    #[inline]
    fn matra_pos_right(u: u32) -> Position {
        if is_deva(u) {
            position::AFTER_SUB
        } else if is_beng(u) {
            position::AFTER_POST
        } else if is_guru(u) {
            position::AFTER_POST
        } else if is_gujr(u) {
            position::AFTER_POST
        } else if is_orya(u) {
            position::AFTER_POST
        } else if is_taml(u) {
            position::AFTER_POST
        } else if is_telu(u) {
            if u <= 0x0C42 { position::BEFORE_SUB } else { position::AFTER_SUB }
        } else if is_knda(u) {
            if u < 0x0CC3 || u > 0xCD6 { position::BEFORE_SUB } else { position::AFTER_SUB }
        } else if is_mlym(u) {
            position::AFTER_POST
        } else if is_sinh(u) {
            position::AFTER_SUB
        } else {
            position::AFTER_SUB
        }
    }

    // BENG and MLYM don't have top matras.
    #[inline]
    fn matra_pos_top(u: u32) -> Position {
        if is_deva(u)      {
            position::AFTER_SUB
        } else if is_guru(u) {
            // Deviate from spec
            position::AFTER_POST
        } else if is_gujr(u) {
            position::AFTER_SUB
        } else if is_orya(u) {
            position::AFTER_MAIN
        } else if is_taml(u) {
            position::AFTER_SUB
        } else if is_telu(u) {
            position::BEFORE_SUB
        } else if is_knda(u) {
            position::BEFORE_SUB
        } else if is_sinh(u) {
            position::AFTER_SUB
        } else {
            position::AFTER_SUB
        }
    }

    #[inline]
    fn matra_pos_bottom(u: u32) -> Position {
        if is_deva(u) {
            position::AFTER_SUB
        } else if is_beng(u) {
            position::AFTER_SUB
        } else if is_guru(u) {
            position::AFTER_POST
        } else if is_gujr(u) {
            position::AFTER_POST
        } else if is_orya(u) {
            position::AFTER_SUB
        } else if is_taml(u) {
            position::AFTER_POST
        } else if is_telu(u) {
            position::BEFORE_SUB
        } else if is_knda(u) {
            position::BEFORE_SUB
        } else if is_mlym(u) {
            position::AFTER_POST
        } else if is_sinh(u) {
            position::AFTER_SUB
        } else {
            position::AFTER_SUB
        }
    }

    match side {
        position::PRE_C => position::PRE_M,
        position::POST_C => matra_pos_right(u),
        position::ABOVE_C => matra_pos_top(u),
        position::BELOW_C => matra_pos_bottom(u),
        _ => side,
    }
}
