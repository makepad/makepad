//! The last link of every font chain: the operating system's own fonts.
//!
//! A character no member of a family covers (Devanagari, Arabic, Thai, Hangul
//! in a UI themed with a Latin face) is looked up once, by script, among the
//! installed fonts the OS itself draws that script with
//! ([`makepad_font_core::system_fonts`]). The face found joins the end of every
//! family ([`super::loader::Loader::add_system_fallback`]), memory-mapped, and
//! the text lays out again with real glyphs. Nothing is read before such a
//! miss, and each character is looked up at most once per weight.

use {
    super::{
        font::FontId,
        font_face::FontFace,
        font_family::FontFamilyId,
        layouter::Layouter,
        loader::{font_is_bold, FontDefinition},
        shaper::{mark_missing_glyph_for_warning, missing_glyph_message, WARNED_MISSING_GLYPHS},
    },
    crate::makepad_platform::{log, HostBuf},
    fxhash::FxHashSet,
    makepad_font_core::system_fonts::{self, fallback_script, Script},
    std::hash::{Hash, Hasher},
};

/// Whether a miss of `ch` is reported by the OS font lookup
/// ([`SystemFallback::request`], once, after the OS fonts were asked) rather
/// than by the shaper or layouter, which see it a moment before.
pub(crate) fn reports_miss(ch: char) -> bool {
    !needs_no_glyph(ch)
}

/// Characters a font chain never draws a glyph for: no lookup.
fn needs_no_glyph(ch: char) -> bool {
    ch.is_ascii()
        || ch.is_whitespace()
        || ch.is_control()
        || matches!(ch,
            '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}'..='\u{1160}' | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'
            | '\u{FFA0}' | '\u{FFF0}'..='\u{FFF8}' | '\u{E0000}'..='\u{E0FFF}')
}

#[derive(Default)]
pub(crate) struct SystemFallback {
    /// (character, heavy) pairs looked up already.
    attempted: FxHashSet<(char, bool)>,
    /// The OS comes before the bundled CJK and emoji faces (a `System`
    /// font set): a miss here is not the end of the chain.
    pub(crate) bundled_after: bool,
}

impl SystemFallback {
    /// Looks up the OS fonts for the characters of `text` the family misses.
    /// True when a face joined the families (lay the text out again).
    pub(crate) fn request(&mut self, layouter: &mut Layouter, family_id: FontFamilyId, text: &str) -> bool {
        if text.is_ascii() || !layouter.is_font_family_known(family_id) {
            return false;
        }
        // The installed-font index is still being built in the background
        // (Linux, Windows, Android): asked again on the redraw it brings,
        // never waited for here.
        if !system_fonts::index_ready() {
            return false;
        }
        let family = layouter.get_or_load_font_family(family_id);
        let Some(primary) = family.fonts().first() else {
            return false;
        };
        let bold = font_is_bold(primary);
        // The missing characters, grouped by script (one OS query each); the
        // script-less ones (punctuation, combining marks) go last, so a face
        // found for their neighbours covers them first.
        let mut groups: Vec<(Script, String)> = Vec::new();
        for ch in text.chars() {
            if needs_no_glyph(ch) || self.attempted.contains(&(ch, bold)) {
                continue;
            }
            if family.fonts().iter().any(|font| font.with_ttf_parser_face(|face| face.glyph_index(ch).is_some())) {
                continue;
            }
            self.attempted.insert((ch, bold));
            let script = match fallback_script(ch) {
                Script::Inherited | Script::Unknown => Script::Common,
                script => script,
            };
            match groups.iter_mut().find(|(known, _)| *known == script) {
                Some((_, chars)) => chars.push(ch),
                None => groups.push((script, ch.to_string())),
            }
        }
        if groups.is_empty() {
            return false;
        }
        groups.sort_by_key(|(script, _)| *script == Script::Common);
        let diagnostics = family.diagnostics().clone();

        let mut added: Vec<FontFace> = Vec::new();
        for (script, chars) in groups {
            // Covered by a face this request already added (marks, punctuation).
            let chars: String = chars
                .chars()
                .filter(|&ch| !added.iter().any(|face| face.with_ttf_parser_face(|f| f.glyph_index(ch).is_some())))
                .collect();
            if chars.is_empty() {
                continue;
            }
            let mut found = None;
            for candidate in system_fonts::fallback_candidates(&chars, bold) {
                let id = system_font_id(&candidate, bold);
                if layouter.loader.system_fallbacks().iter().any(|(known, _)| *known == id) {
                    continue;
                }
                let Ok(data) = HostBuf::from_file_mmap_or_read(&candidate.path) else {
                    continue;
                };
                let Some(face) = FontFace::from_data_and_index(data.clone(), candidate.index) else {
                    continue;
                };
                let (covers, has_wght) = face.with_ttf_parser_face(|f| {
                    (
                        chars.chars().any(|ch| f.glyph_index(ch).is_some()),
                        f.variation_axes().into_iter().any(|axis| axis.tag.0 == u32::from_be_bytes(*b"wght")),
                    )
                });
                if !covers {
                    continue;
                }
                log!(
                    "text: {} drawn with the system font {}#{}",
                    script.full_name(),
                    candidate.path.display(),
                    candidate.index
                );
                layouter.add_system_fallback(
                    id,
                    FontDefinition {
                        data,
                        index: candidate.index,
                        ascender_fudge_in_ems: 0.0,
                        descender_fudge_in_ems: 0.0,
                        weight: (bold && has_wght).then_some(700.0),
                        variations: Vec::new(),
                    },
                    bold,
                );
                found = Some(face);
                break;
            }
            for ch in chars.chars() {
                let covered = found.as_ref().is_some_and(|face| face.with_ttf_parser_face(|f| f.glyph_index(ch).is_some()));
                if !covered && mark_missing_glyph_for_warning(&WARNED_MISSING_GLYPHS, ch) {
                    if self.bundled_after {
                        log!("{} (no system font; the bundled faces are next)", missing_glyph_message(ch, &diagnostics));
                    } else {
                        log!("{} (no system font either)", missing_glyph_message(ch, &diagnostics));
                    }
                }
            }
            added.extend(found);
        }
        !added.is_empty()
    }
}

fn system_font_id(font: &system_fonts::SystemFont, bold: bool) -> FontId {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "system_fallback".hash(&mut hasher);
    font.path.hash(&mut hasher);
    font.index.hash(&mut hasher);
    bold.hash(&mut hasher);
    FontId::from(hasher.finish())
}
