use crate::{unicode};
use super::*;


pub const HEBREW_SHAPER: ComplexShaper = ComplexShaper {
    collect_features: None,
    override_features: None,
    create_data: None,
    preprocess_text: None,
    postprocess_glyphs: None,
    normalization_mode: Some(ShapeNormalizationMode::Auto),
    decompose: None,
    compose: Some(compose),
    setup_masks: None,
    gpos_tag: Some(Tag::from_bytes(b"hebr")),
    reorder_marks: None,
    zero_width_marks: Some(ZeroWidthMarksMode::ByGdefLate),
    fallback_position: true,
};


const S_DAGESH_FORMS: &[char] = &[
    '\u{FB30}', // ALEF
    '\u{FB31}', // BET
    '\u{FB32}', // GIMEL
    '\u{FB33}', // DALET
    '\u{FB34}', // HE
    '\u{FB35}', // VAV
    '\u{FB36}', // ZAYIN
    '\u{0000}', // HET
    '\u{FB38}', // TET
    '\u{FB39}', // YOD
    '\u{FB3A}', // FINAL KAF
    '\u{FB3B}', // KAF
    '\u{FB3C}', // LAMED
    '\u{0000}', // FINAL MEM
    '\u{FB3E}', // MEM
    '\u{0000}', // FINAL NUN
    '\u{FB40}', // NUN
    '\u{FB41}', // SAMEKH
    '\u{0000}', // AYIN
    '\u{FB43}', // FINAL PE
    '\u{FB44}', // PE
    '\u{0000}', // FINAL TSADI
    '\u{FB46}', // TSADI
    '\u{FB47}', // QOF
    '\u{FB48}', // RESH
    '\u{FB49}', // SHIN
    '\u{FB4A}', // TAV
];

fn compose(ctx: &ShapeNormalizeContext, a: char, b: char) -> Option<char> {
    // Hebrew presentation-form shaping.
    // https://bugzilla.mozilla.org/show_bug.cgi?id=728866
    // Hebrew presentation forms with dagesh, for characters U+05D0..05EA;
    // Note that some letters do not have a dagesh presForm encoded.
    match unicode::compose(a, b) {
        Some(c) => Some(c),
        None if !ctx.plan.has_gpos_mark => {
            // Special-case Hebrew presentation forms that are excluded from
            // standard normalization, but wanted for old fonts.
            let a = a as u32;
            let b = b as u32;
            match b {
                0x05B4 => { // HIRIQ
                    match a {
                        0x05D9 => Some('\u{FB1D}'), // YOD
                        _ => None,
                    }
                }
                0x05B7 => { // patah
                    match a {
                        0x05D9 => Some('\u{FB1F}'), // YIDDISH YOD YOD
                        0x05D0 => Some('\u{FB2E}'), // ALEF
                        _ => None,
                    }
                }
                0x05B8 => { // QAMATS
                    match a {
                        0x05D0 => Some('\u{FB2F}'), // ALEF
                        _ => None,
                    }
                }
                0x05B9 => { // HOLAM
                    match a {
                        0x05D5 => Some('\u{FB4B}'), // VAV
                        _ => None,
                    }
                }
                0x05BC => { // DAGESH
                    match a {
                        0x05D0..=0x05EA => {
                            let c = S_DAGESH_FORMS[a as usize - 0x05D0];
                            if c != '\0' { Some(c) } else { None }
                        }
                        0xFB2A => Some('\u{FB2C}'), // SHIN WITH SHIN DOT
                        0xFB2B => Some('\u{FB2D}'), // SHIN WITH SIN DOT
                        _ => None,
                    }
                }
                0x05BF => { // RAFE
                    match a {
                        0x05D1 => Some('\u{FB4C}'), // BET
                        0x05DB => Some('\u{FB4D}'), // KAF
                        0x05E4 => Some('\u{FB4E}'), // PE
                        _ => None,
                    }
                }
                0x05C1 => { // SHIN DOT
                    match a {
                        0x05E9 => Some('\u{FB2A}'), // SHIN
                        0xFB49 => Some('\u{FB2C}'), // SHIN WITH DAGESH
                        _ => None,
                    }
                }
                0x05C2 => { // SIN DOT
                    match a {
                        0x05E9 => Some('\u{FB2B}'), // SHIN
                        0xFB49 => Some('\u{FB2D}'), // SHIN WITH DAGESH
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        None => None,
    }
}
