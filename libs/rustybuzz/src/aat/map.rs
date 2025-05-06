use alloc::vec::Vec;

use crate::{Face, Tag, Mask};
use super::feature_mappings::FEATURE_MAPPINGS;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum FeatureType {
    Ligatures = 1,
    LetterCase = 3,
    VerticalSubstitution = 4,
    NumberSpacing = 6,
    VerticalPosition = 10,
    Fractions = 11,
    TypographicExtras = 14,
    MathematicalExtras = 15,
    StyleOptions = 19,
    CharacterShape = 20,
    NumberCase = 21,
    TextSpacing = 22,
    Transliteration = 23,
    RubyKana = 28,
    ItalicCjkRoman = 32,
    CaseSensitiveLayout = 33,
    AlternateKana = 34,
    StylisticAlternatives = 35,
    ContextualAlternatives = 36,
    LowerCase = 37,
    UpperCase = 38,
}


#[derive(Default)]
pub struct Map {
    pub chain_flags: Vec<Mask>,
}


#[derive(Copy, Clone)]
pub struct FeatureInfo {
    pub kind: u16,
    pub setting: u16,
    pub is_exclusive: bool,
}


#[derive(Default)]
pub struct MapBuilder {
    pub features: Vec<FeatureInfo>,
}

impl MapBuilder {
    pub fn add_feature(&mut self, face: &Face, tag: Tag, value: u32) -> Option<()> {
        const FEATURE_TYPE_CHARACTER_ALTERNATIVES: u16 = 17;

        let feat = face.tables().feat?;

        if tag == Tag::from_bytes(b"aalt") {
            let exposes_feature = feat.names.find(FEATURE_TYPE_CHARACTER_ALTERNATIVES)
                .map(|f| f.setting_names.len() != 0)
                .unwrap_or(false);

            if !exposes_feature {
                return Some(());
            }

            self.features.push(FeatureInfo {
                kind: FEATURE_TYPE_CHARACTER_ALTERNATIVES,
                setting: value as u16,
                is_exclusive: true,
            });
        }

        let idx = FEATURE_MAPPINGS.binary_search_by(|map| map.ot_feature_tag.cmp(&tag)).ok()?;
        let mapping = &FEATURE_MAPPINGS[idx];

        let mut feature = feat.names.find(mapping.aat_feature_type as u16);

        match feature {
            Some(feature) if feature.setting_names.len() != 0 => {}
            _ => {
                // Special case: Chain::compile_flags will fall back to the deprecated version of
                // small-caps if necessary, so we need to check for that possibility.
                // https://github.com/harfbuzz/harfbuzz/issues/2307
                if  mapping.aat_feature_type == FeatureType::LowerCase &&
                    mapping.selector_to_enable == super::feature_selector::LOWER_CASE_SMALL_CAPS
                {
                    feature = feat.names.find(FeatureType::LetterCase as u16);
                }
            }
        }

        match feature {
            Some(feature) if feature.setting_names.len() != 0 => {
                let setting = if value != 0 {
                    mapping.selector_to_enable
                } else {
                    mapping.selector_to_disable
                } as u16;

                self.features.push(FeatureInfo {
                    kind: mapping.aat_feature_type as u16,
                    setting,
                    is_exclusive: feature.exclusive,
                });
            }
            _ => {}
        }

        Some(())
    }

    pub fn has_feature(&self, kind: u16, setting: u16) -> bool {
        self.features.binary_search_by(|probe| {
            if probe.kind != kind {
                probe.kind.cmp(&kind)
            } else {
                probe.setting.cmp(&setting)
            }
        }).is_ok()
    }

    pub fn compile(&mut self, face: &Face) -> Map {
        // Sort features and merge duplicates.
        self.features.sort_by(|a, b| {
            if a.kind != b.kind {
                a.kind.cmp(&b.kind)
            } else if !a.is_exclusive && (a.setting & !1) != (b.setting & !1) {
                a.setting.cmp(&b.setting)
            } else {
                core::cmp::Ordering::Equal
            }
        });

        let mut j = 0;
        for i in 0..self.features.len() {
            // Nonexclusive feature selectors come in even/odd pairs to turn a setting on/off
            // respectively, so we mask out the low-order bit when checking for "duplicates"
            // (selectors referring to the same feature setting) here.
            let non_exclusive = !self.features[i].is_exclusive &&
                (self.features[i].setting & !1) != (self.features[j].setting & !1);

            if self.features[i].kind != self.features[j].kind || non_exclusive {
                j += 1;
                self.features[j] = self.features[i];
            }
        }
        self.features.truncate(j + 1);

        super::metamorphosis::compile_flags(face, self).unwrap_or_default()
    }
}
