use crate::{ GlyphInfo};
use crate::buffer::{BufferFlags};
use crate::ot::{feature, FeatureFlags};
use super::indic::{category, position};
use super::*;


pub const MYANMAR_SHAPER: ComplexShaper = ComplexShaper {
    collect_features: Some(collect_features),
    override_features: Some(override_features),
    create_data: None,
    preprocess_text: None,
    postprocess_glyphs: None,
    normalization_mode: Some(ShapeNormalizationMode::ComposedDiacriticsNoShortCircuit),
    decompose: None,
    compose: None,
    setup_masks: Some(setup_masks),
    gpos_tag: None,
    reorder_marks: None,
    zero_width_marks: Some(ZeroWidthMarksMode::ByGdefEarly),
    fallback_position: false,
};

// Ugly Zawgyi encoding.
// Disable all auto processing.
// https://github.com/harfbuzz/harfbuzz/issues/1162
pub const MYANMAR_ZAWGYI_SHAPER: ComplexShaper = ComplexShaper {
    collect_features: None,
    override_features: None,
    create_data: None,
    preprocess_text: None,
    postprocess_glyphs: None,
    normalization_mode: None,
    decompose: None,
    compose: None,
    setup_masks: None,
    gpos_tag: None,
    reorder_marks: None,
    zero_width_marks: None,
    fallback_position: false,
};


const MYANMAR_FEATURES: &[Tag] = &[
    // Basic features.
    // These features are applied in order, one at a time, after reordering.
    feature::REPH_FORMS,
    feature::PRE_BASE_FORMS,
    feature::BELOW_BASE_FORMS,
    feature::POST_BASE_FORMS,
    // Other features.
    // These features are applied all at once after clearing syllables.
    feature::PRE_BASE_SUBSTITUTIONS,
    feature::ABOVE_BASE_SUBSTITUTIONS,
    feature::BELOW_BASE_SUBSTITUTIONS,
    feature::POST_BASE_SUBSTITUTIONS,
];

impl GlyphInfo {
    fn set_myanmar_properties(&mut self) {
        let u = self.glyph_id;
        let (mut cat, mut pos) = super::indic::get_category_and_position(u);

        // Myanmar
        // https://docs.microsoft.com/en-us/typography/script-development/myanmar#analyze

        if (0xFE00..=0xFE0F).contains(&u) {
            cat = category::VS;
        }

        match u {
            // The spec says C, IndicSyllableCategory doesn't have.
            0x104E => cat = category::C,

            0x002D |
            0x00A0 |
            0x00D7 |
            0x2012 |
            0x2013 |
            0x2014 |
            0x2015 |
            0x2022 |
            0x25CC |
            0x25FB |
            0x25FC |
            0x25FD |
            0x25FE => cat = category::PLACEHOLDER,

            0x1004 |
            0x101B |
            0x105A => cat = category::RA,

            0x1032 |
            0x1036 => cat = category::A,

            0x1039 => cat = category::H,

            0x103A => cat = category::SYMBOL,

            0x1041 |
            0x1042 |
            0x1043 |
            0x1044 |
            0x1045 |
            0x1046 |
            0x1047 |
            0x1048 |
            0x1049 |
            0x1090 |
            0x1091 |
            0x1092 |
            0x1093 |
            0x1094 |
            0x1095 |
            0x1096 |
            0x1097 |
            0x1098 |
            0x1099 => cat = category::D,

            // XXX The spec says D0, but Uniscribe doesn't seem to do.
            0x1040 => cat = category::D,

            0x103E |
            0x1060 => cat = category::X_GROUP,

            0x103C => cat = category::Y_GROUP,

            0x103D |
            0x1082 => cat = category::MW,

            0x103B |
            0x105E |
            0x105F => cat = category::MY,

            0x1063 |
            0x1064 |
            0x1069 |
            0x106A |
            0x106B |
            0x106C |
            0x106D |
            0xAA7B => cat = category::PT,

            0x1038 |
            0x1087 |
            0x1088 |
            0x1089 |
            0x108A |
            0x108B |
            0x108C |
            0x108D |
            0x108F |
            0x109A |
            0x109B |
            0x109C => cat = category::SM,

            0x104A |
            0x104B => cat = category::P,

            // https://github.com/harfbuzz/harfbuzz/issues/218
            0xAA74 |
            0xAA75 |
            0xAA76 => cat = category::C,

            _ => {}
        }

        // Re-assign position.

        if cat == category::M {
            match pos {
                position::PRE_C => {
                    cat = category::V_PRE;
                    pos = position::PRE_M;
                }
                position::BELOW_C => cat = category::V_BLW,
                position::ABOVE_C => cat = category::V_AVB,
                position::POST_C => cat = category::V_PST,
                _ => {}
            }
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

    planner.ot_map.add_gsub_pause(Some(reorder));

    for feature in MYANMAR_FEATURES.iter().take(4) {
        planner.ot_map.enable_feature(*feature, FeatureFlags::MANUAL_ZWJ, 1);
        planner.ot_map.add_gsub_pause(None);
    }

    planner.ot_map.add_gsub_pause(Some(crate::ot::clear_syllables));

    for feature in MYANMAR_FEATURES.iter().skip(4) {
        planner.ot_map.enable_feature(*feature, FeatureFlags::MANUAL_ZWJ, 1);
    }
}

fn setup_syllables(_: &ShapePlan, _: &Face, buffer: &mut Buffer) {
    super::myanmar_machine::find_syllables_myanmar(buffer);

    let mut start = 0;
    let mut end = buffer.next_syllable(0);
    while start < buffer.len {
        buffer.unsafe_to_break(start, end);
        start = end;
        end = buffer.next_syllable(start);
    }
}

fn reorder(_: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    insert_dotted_circles(face, buffer);

    let mut start = 0;
    let mut end = buffer.next_syllable(0);
    while start < buffer.len {
        reorder_syllable(start, end, buffer);
        start = end;
        end = buffer.next_syllable(start);
    }
}

fn insert_dotted_circles(face: &Face, buffer: &mut Buffer) {
    use super::myanmar_machine::SyllableType;

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
    dottedcircle.set_myanmar_properties();
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

            buffer.output_info(ginfo);
        } else {
            buffer.next_glyph();
        }
    }

    buffer.swap_buffers();
}

fn reorder_syllable(start: usize, end: usize, buffer: &mut Buffer) {
    use super::myanmar_machine::SyllableType;

    let syllable_type = match buffer.info[start].syllable() & 0x0F {
        0 => SyllableType::ConsonantSyllable,
        1 => SyllableType::PunctuationCluster,
        2 => SyllableType::BrokenCluster,
        3 => SyllableType::NonMyanmarCluster,
        _ => unreachable!(),
    };

    match syllable_type {
        // We already inserted dotted-circles, so just call the consonant_syllable.
        SyllableType::ConsonantSyllable | SyllableType::BrokenCluster => {
            initial_reordering_consonant_syllable(start, end, buffer);
        }
        SyllableType::PunctuationCluster | SyllableType::NonMyanmarCluster => {}
    }
}

// Rules from:
// https://docs.microsoft.com/en-us/typography/script-development/myanmar
fn initial_reordering_consonant_syllable(start: usize, end: usize, buffer: &mut Buffer) {
    let mut base = end;
    let mut has_reph = false;

    {
        let mut limit = start;
        if start + 3 <= end &&
            buffer.info[start + 0].indic_category() == category::RA &&
            buffer.info[start + 1].indic_category() == category::SYMBOL &&
            buffer.info[start + 2].indic_category() == category::H
        {
            limit += 3;
            base = start;
            has_reph = true;
        }

        {
            if !has_reph {
                base = limit;
            }

            for i in limit..end {
                if buffer.info[i].is_consonant() {
                    base = i;
                    break;
                }
            }
        }
    }

    // Reorder!
    {
        let mut i = start;
        while i < start + if has_reph { 3 } else { 0 } {
            buffer.info[i].set_indic_position(position::AFTER_MAIN);
            i += 1;
        }

        while i < base {
            buffer.info[i].set_indic_position(position::PRE_C);
            i += 1;
        }

        if i < end {
            buffer.info[i].set_indic_position(position::BASE_C);
            i += 1;
        }

        let mut pos = position::AFTER_MAIN;
        // The following loop may be ugly, but it implements all of
        // Myanmar reordering!
        for i in i..end {
            // Pre-base reordering
            if buffer.info[i].indic_category() == category::Y_GROUP {
                buffer.info[i].set_indic_position(position::PRE_C);
                continue;
            }

            // Left matra
            if buffer.info[i].indic_position() < position::BASE_C {
                continue;
            }

            if buffer.info[i].indic_category() == category::VS {
                let t = buffer.info[i - 1].indic_position();
                buffer.info[i].set_indic_position(t);
                continue;
            }

            if pos == position::AFTER_MAIN && buffer.info[i].indic_category() == category::V_BLW {
                pos = position::BELOW_C;
                buffer.info[i].set_indic_position(pos);
                continue;
            }

            if pos == position::BELOW_C && buffer.info[i].indic_category() == category::A {
                buffer.info[i].set_indic_position(position::BEFORE_SUB);
                continue;
            }

            if pos == position::BELOW_C && buffer.info[i].indic_category() == category::V_BLW {
                buffer.info[i].set_indic_position(pos);
                continue;
            }

            if pos == position::BELOW_C && buffer.info[i].indic_category() != category::A {
                pos = position::AFTER_SUB;
                buffer.info[i].set_indic_position(pos);
                continue;
            }

            buffer.info[i].set_indic_position(pos);
        }
    }

    buffer.sort(start, end, |a, b| a.indic_position().cmp(&b.indic_position()) == core::cmp::Ordering::Greater);
}

fn override_features(planner: &mut ShapePlanner) {
    planner.ot_map.disable_feature(feature::STANDARD_LIGATURES);
}

fn setup_masks(_: &ShapePlan, _: &Face, buffer: &mut Buffer) {
    // We cannot setup masks here.  We save information about characters
    // and setup masks later on in a pause-callback.
    for info in buffer.info_slice_mut() {
        info.set_myanmar_properties();
    }
}
