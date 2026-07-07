use {
    super::{
        font::{Font, GlyphId},
        slice::SliceExt,
        substr::Substr,
    },
    fxhash::FxHashMap,
    rustybuzz,
    rustybuzz::UnicodeBuffer,
    std::{
        collections::BTreeMap,
        hash::{Hash, Hasher},
        mem,
        rc::Rc,
    },
    unicode_script::{Script, UnicodeScript},
    unicode_segmentation::UnicodeSegmentation,
};

/// Returns `true` if `text` is guaranteed to contain no right-to-left
/// characters, so that the Unicode Bidirectional Algorithm can be skipped
/// entirely. False negatives (returning `false` for pure-LTR text containing
/// characters in these blocks that happen to be non-RTL) are acceptable —
/// we just run BiDi in that case. False positives would cause mis-rendering,
/// so the ranges below are the full Unicode blocks that contain any strong
/// RTL characters, rather than the exact RTL code points.
fn is_definitely_ltr(text: &str) -> bool {
    // Optimised common case: pure ASCII is always LTR, and `is_ascii` uses
    // a SIMD-accelerated byte scan.
    if text.is_ascii() {
        return true;
    }
    !text.chars().any(|c| {
        let c = c as u32;
        // BMP blocks containing any strong RTL characters: Hebrew, Arabic,
        // Syriac, Thaana, NKo, Samaritan, Mandaic, Syriac Supplement and
        // Arabic Extended-A/B.
        (0x0590..=0x08FF).contains(&c)
            // Alphabetic Presentation Forms (Hebrew ligatures) through
            // Arabic Presentation Forms-A.
            || (0xFB1D..=0xFDFF).contains(&c)
            // Arabic Presentation Forms-B.
            || (0xFE70..=0xFEFF).contains(&c)
            // Ancient RTL scripts (Imperial Aramaic, Phoenician, etc.) in
            // the Supplementary Multilingual Plane.
            || (0x10800..=0x10FFF).contains(&c)
            // More modern SMP RTL blocks (Mende Kikakui, Adlam, Arabic
            // Mathematical Alphabetic Symbols, etc.).
            || (0x1E800..=0x1EFFF).contains(&c)
    })
}

/// Whether `ch` is an emoji (should be rendered by the color emoji font).
///
/// Emoji code points are classified as `Script::Common` by Unicode, so the
/// `collect_missing_scripts` script filter drops them; this predicate lets the
/// shaper surface a separate "missing emoji" signal instead. The ranges cover
/// the main emoji blocks plus the dingbats/misc-symbol ranges that carry emoji
/// presentation; it only needs to be right for characters that fell back to
/// `.notdef`, so a conservative superset is fine (the OS does real coverage
/// matching when we then fetch the emoji font).
fn is_emoji(ch: char) -> bool {
    matches!(ch as u32,
        0x1F300..=0x1FAFF   // Misc symbols & pictographs, emoticons, transport, supplemental, symbols-and-pictographs-extended-A
        | 0x1F000..=0x1F0FF // Mahjong/dominoes/playing cards
        | 0x2600..=0x27BF   // Misc symbols + dingbats
        | 0x2B00..=0x2BFF   // Misc symbols and arrows (stars etc.)
        | 0x1F1E6..=0x1F1FF // Regional indicator symbols (flags)
        | 0xFE00..=0xFE0F   // Variation selectors (VS16 emoji presentation)
        | 0x2190..=0x21FF   // Arrows (some emoji-presented)
        | 0x2300..=0x23FF   // Misc technical (⌚ ⏰ etc.)
    )
}

/// Scan shaped glyphs for any that fell back to `.notdef` (id 0 — no font in
/// the family covered them) and return the de-duplicated Unicode scripts of the
/// characters they came from, plus whether any of them were emoji. Non-emoji
/// `Common`/`Inherited`/`Unknown` are filtered out of the script list: they
/// belong to whitespace, combining marks and shared punctuation, which give no
/// useful signal about which language font to fetch. Emoji are also `Common`
/// but are reported separately via the returned `bool` (an uncovered emoji
/// triggers on-demand loading of the color emoji font).
///
/// `cluster` is the byte offset of the glyph's source character in `text`;
/// we decode the `char` starting there to obtain its script.
///
/// Takes `(glyph_id, cluster)` pairs rather than `&[ShapedGlyph]` so the
/// script-detection logic can be unit-tested without constructing real fonts.
fn collect_missing_scripts(
    text: &str,
    glyphs: impl IntoIterator<Item = (GlyphId, usize)>,
) -> (Vec<Script>, bool) {
    let mut scripts: Vec<Script> = Vec::new();
    let mut missing_emoji = false;
    for (id, cluster) in glyphs {
        if id != 0 {
            continue;
        }
        let Some(rest) = text.get(cluster..) else {
            continue;
        };
        let Some(ch) = rest.chars().next() else {
            continue;
        };
        if is_emoji(ch) {
            missing_emoji = true;
            continue;
        }
        let script = ch.script();
        if matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
            continue;
        }
        if !scripts.contains(&script) {
            scripts.push(script);
        }
    }
    (scripts, missing_emoji)
}

/// Float wrapper that supports Hash and Eq via bit representation.
#[derive(Clone, Copy, Debug)]
pub struct Ems(pub f32);

impl Hash for Ems {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl PartialEq for Ems {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Ems {}

impl Default for Ems {
    fn default() -> Self {
        Ems(0.0)
    }
}

/// Text direction for shaping.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Debug)]
pub struct Shaper {
    reusable_glyphs: Vec<Vec<ShapedGlyph>>,
    reusable_unicode_buffer: UnicodeBuffer,
    // One-slot cache for the converted rustybuzz feature list. `shape_step`
    // is called on every shaping path and would otherwise rebuild a
    // `Vec<rustybuzz::Feature>` on each call. We key the cache on the
    // feature-pair slice contents (not on pointer identity — see note in
    // `shape_step`), which is cheap because `features` is almost always
    // empty in practice and rarely changes across calls when it isn't.
    cached_features_source: Vec<(u32, u32)>,
    cached_rb_features: Vec<rustybuzz::Feature>,
    cache_size: usize,
    cache_tick: u64,
    cached_results: FxHashMap<ShapeParams, CachedShape>,
    cache_lru_order: BTreeMap<u64, ShapeParams>,
}

/// A shaping cache entry, tracked with its position in the least-recently-used
/// order (the tick under which it is registered in `Shaper::cache_lru_order`).
#[derive(Debug)]
struct CachedShape {
    result: Rc<ShapedText>,
    last_used: u64,
}

impl Shaper {
    pub fn new(settings: Settings) -> Self {
        Self {
            reusable_glyphs: Vec::new(),
            reusable_unicode_buffer: UnicodeBuffer::new(),
            cached_features_source: Vec::new(),
            cached_rb_features: Vec::new(),
            cache_size: settings.cache_size,
            cache_tick: 0,
            cached_results: FxHashMap::with_capacity_and_hasher(
                settings.cache_size,
                Default::default(),
            ),
            cache_lru_order: BTreeMap::new(),
        }
    }

    pub fn get_or_shape(&mut self, params: ShapeParams) -> Rc<ShapedText> {
        if self.cache_size == 0 {
            return Rc::new(self.shape(params));
        }
        if let Some(entry) = self.cached_results.get_mut(&params) {
            // Refresh recency so the hot working set of words survives
            // eviction while scrolling through varied text.
            if let Some(key) = self.cache_lru_order.remove(&entry.last_used) {
                self.cache_tick += 1;
                entry.last_used = self.cache_tick;
                self.cache_lru_order.insert(entry.last_used, key);
            }
            return entry.result.clone();
        }
        while self.cached_results.len() >= self.cache_size {
            let Some((_, key)) = self.cache_lru_order.pop_first() else {
                break;
            };
            self.cached_results.remove(&key);
        }
        let cache_key = params.clone();
        let result = Rc::new(self.shape(params));
        self.cache_tick += 1;
        self.cache_lru_order.insert(self.cache_tick, cache_key.clone());
        self.cached_results.insert(
            cache_key,
            CachedShape {
                result: result.clone(),
                last_used: self.cache_tick,
            },
        );
        result
    }

    fn shape(&mut self, params: ShapeParams) -> ShapedText {
        let mut glyphs = Vec::new();

        // An empty font family is a normal state, not an error: on platforms
        // without a system-font API (e.g. wasm) UI text may resolve to no fonts
        // at all. Shape to zero glyphs (renders nothing) instead of warning.
        if !params.fonts.is_empty() {
            let text: &str = &params.text;
            // Fast path: when the text is guaranteed to contain no RTL
            // characters, skip the Unicode Bidirectional Algorithm entirely
            // and shape as a single LTR run. This avoids BiDi's classification
            // pass and vec allocations for the common case of ASCII / Latin /
            // Greek / Cyrillic / CJK / emoji text.
            if is_definitely_ltr(text) {
                self.shape_recursive(
                    text,
                    &params.fonts[0],
                    &params.fonts,
                    &params.features,
                    Direction::Ltr,
                    0,
                    text.len(),
                    &mut glyphs,
                );
            } else {
                // The text contains at least one possibly-RTL character, so
                // run the Unicode Bidirectional Algorithm to resolve embedding
                // levels and segment the text into visual runs. Shaping each
                // run in its resolved direction keeps mixed LTR/RTL strings
                // from stomping on each other visually.
                let default_level = match params.direction {
                    Direction::Ltr => Some(unicode_bidi::Level::ltr()),
                    Direction::Rtl => Some(unicode_bidi::Level::rtl()),
                };
                let bidi = unicode_bidi::ParagraphBidiInfo::new(text, default_level);
                if bidi.is_pure_ltr {
                    // BiDi confirmed everything resolved to LTR after all
                    // (e.g. isolated presentation-form characters), so a
                    // single LTR shape call is still correct and avoids the
                    // visual_runs allocation.
                    self.shape_recursive(
                        text,
                        &params.fonts[0],
                        &params.fonts,
                        &params.features,
                        Direction::Ltr,
                        0,
                        text.len(),
                        &mut glyphs,
                    );
                } else {
                    // `visual_runs` returns level runs in visual (left-to-right)
                    // order, so appending each run's shaped glyphs in iteration
                    // order yields glyphs in the final visual order.
                    let (levels, runs) = bidi.visual_runs(0..text.len());
                    for run in &runs {
                        let direction = if levels[run.start].is_rtl() {
                            Direction::Rtl
                        } else {
                            Direction::Ltr
                        };
                        self.shape_recursive(
                            text,
                            &params.fonts[0],
                            &params.fonts,
                            &params.features,
                            direction,
                            run.start,
                            run.end,
                            &mut glyphs,
                        );
                    }
                }
            }
        }

        // Post-process: apply letter-spacing and word-spacing
        let letter_spacing = params.letter_spacing.0;
        let word_spacing = params.word_spacing.0;
        if letter_spacing != 0.0 || word_spacing != 0.0 {
            let text = params.text.as_bytes();
            for glyph in glyphs.iter_mut() {
                glyph.advance_in_ems += letter_spacing;
                if glyph.cluster < text.len() && text[glyph.cluster] == b' ' {
                    glyph.advance_in_ems += word_spacing;
                }
            }
        }

        let (missing_scripts, missing_emoji) =
            collect_missing_scripts(&params.text, glyphs.iter().map(|g| (g.id, g.cluster)));

        ShapedText {
            text: params.text,
            width_in_ems: glyphs.iter().map(|glyph| glyph.advance_in_ems).sum(),
            glyphs,
            missing_scripts,
            missing_emoji,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_recursive(
        &mut self,
        text: &str,
        primary_font: &Rc<Font>,
        fonts: &[Rc<Font>],
        features: &[(u32, u32)],
        direction: Direction,
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        let (font, remaining_fonts) = fonts.split_first().unwrap();
        let mut glyphs = self.reusable_glyphs.pop().unwrap_or_default();
        self.shape_step(text, font, features, direction, start, end, &mut glyphs);

        // Collect glyph groups (runs of glyphs sharing a cluster) in the order
        // produced by the shaper, which is the visual order for `direction`.
        // We use an indexable Vec so we can look both forward and backward to
        // compute the logical byte range a glyph group covers — necessary
        // because HarfBuzz emits clusters monotonically in the shaping
        // direction (non-decreasing for LTR, non-increasing for RTL).
        let glyph_groups: Vec<&[ShapedGlyph]> = glyphs
            .group_by(|glyph_0, glyph_1| glyph_0.cluster == glyph_1.cluster)
            .collect();

        let mut i = 0;
        while i < glyph_groups.len() {
            if glyph_groups[i].iter().any(|glyph| glyph.id == 0) && !remaining_fonts.is_empty() {
                // Extend the run to cover every adjacent glyph group that
                // still has an unmapped glyph, so we can reshape the whole
                // span with the next fallback font in one call.
                let run_start = i;
                while i < glyph_groups.len() && glyph_groups[i].iter().any(|glyph| glyph.id == 0) {
                    i += 1;
                }
                let run_end = i;

                // Compute the logical byte range [missing_start, missing_end)
                // in `text` that this missing run covers. The "logically next"
                // cluster after a group sits visually to the right in LTR and
                // visually to the left in RTL. If the group is at the visual
                // edge of the run, the range extends to `end` (LTR) or to the
                // group just before the run (RTL).
                let (missing_start, missing_end) = match direction {
                    Direction::Ltr => {
                        let lo = glyph_groups[run_start][0].cluster;
                        let hi = glyph_groups
                            .get(run_end)
                            .map_or(end, |next| next[0].cluster);
                        (lo, hi)
                    }
                    Direction::Rtl => {
                        let lo = glyph_groups[run_end - 1][0].cluster;
                        let hi = if run_start == 0 {
                            end
                        } else {
                            glyph_groups[run_start - 1][0].cluster
                        };
                        (lo, hi)
                    }
                };

                // Defensive: if the computed range is invalid for any reason
                // (e.g. HarfBuzz produced unexpected non-monotonic clusters
                // for a complex script shaped in the "wrong" direction), fall
                // back to rendering the missing groups as the primary font's
                // .notdef glyph rather than panicking inside a recursive
                // shape_step call on an inverted byte range.
                if missing_start >= start && missing_end <= end && missing_start < missing_end {
                    self.shape_recursive(
                        text,
                        primary_font,
                        remaining_fonts,
                        features,
                        direction,
                        missing_start,
                        missing_end,
                        out_glyphs,
                    );
                } else {
                    for group in &glyph_groups[run_start..run_end] {
                        out_glyphs.extend(group.iter().map(|glyph| {
                            let mut g = glyph.clone();
                            g.id = 0;
                            g.font = primary_font.clone();
                            g
                        }));
                    }
                }
            } else {
                let glyph_group = glyph_groups[i];
                // If we've exhausted all fallback fonts and still have
                // unmapped glyphs (id == 0), use the primary font's .notdef
                // so a visible placeholder is rendered instead of nothing.
                if glyph_group.iter().any(|glyph| glyph.id == 0) && !Rc::ptr_eq(font, primary_font)
                {
                    out_glyphs.extend(glyph_group.iter().map(|glyph| {
                        let mut g = glyph.clone();
                        if g.id == 0 {
                            g.font = primary_font.clone();
                        }
                        g
                    }));
                } else {
                    out_glyphs.extend(glyph_group.iter().cloned());
                }
                i += 1;
            }
        }
        drop(glyph_groups);
        glyphs.clear();
        self.reusable_glyphs.push(glyphs);
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_step(
        &mut self,
        text: &str,
        font: &Rc<Font>,
        features: &[(u32, u32)],
        direction: Direction,
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        let mut unicode_buffer = mem::take(&mut self.reusable_unicode_buffer);
        match direction {
            Direction::Ltr => unicode_buffer.set_direction(rustybuzz::Direction::LeftToRight),
            Direction::Rtl => unicode_buffer.set_direction(rustybuzz::Direction::RightToLeft),
        }
        for (index, grapheme) in text[start..end].grapheme_indices(true) {
            let cluster = start + index;
            for char in grapheme.chars() {
                unicode_buffer.add(char, cluster as u32);
            }
        }
        // Convert the caller's `(tag, value)` feature pairs into rustybuzz's
        // `Feature` type, reusing the previous conversion when the feature
        // set hasn't changed. We compare contents rather than pointers: a
        // pointer-based cache would be unsound if the caller's `Rc<Vec<_>>`
        // backing the slice were dropped and a fresh allocation happened
        // to reuse the same address. Content comparison is O(n), but `n`
        // is almost always zero, and the saved work on a hit (allocation
        // + Feature construction) dwarfs the comparison itself.
        if features != self.cached_features_source.as_slice() {
            self.cached_features_source.clear();
            self.cached_features_source.extend_from_slice(features);
            self.cached_rb_features.clear();
            self.cached_rb_features
                .extend(features.iter().map(|&(tag, value)| {
                    rustybuzz::Feature::new(
                        rustybuzz::ttf_parser::Tag::from_bytes(&tag.to_be_bytes()),
                        value,
                        ..,
                    )
                }));
        }
        let rb_features = &self.cached_rb_features;
        let glyph_buffer =
            font.with_rustybuzz_face(|face| rustybuzz::shape(face, rb_features, unicode_buffer));
        let units_per_em = font.units_per_em();

        out_glyphs.extend(
            glyph_buffer
                .glyph_infos()
                .iter()
                .zip(glyph_buffer.glyph_positions())
                .map(|(glyph_info, glyph_position)| ShapedGlyph {
                    font: font.clone(),
                    id: glyph_info.glyph_id as u16,
                    cluster: glyph_info.cluster as usize,
                    advance_in_ems: glyph_position.x_advance as f32 / units_per_em,
                    offset_in_ems: glyph_position.x_offset as f32 / units_per_em,
                    y_offset_in_ems: glyph_position.y_offset as f32 / units_per_em,
                }),
        );

        self.reusable_unicode_buffer = glyph_buffer.clear();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub cache_size: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ShapeParams {
    pub text: Substr,
    pub fonts: Rc<[Rc<Font>]>,
    pub direction: Direction,
    pub letter_spacing: Ems,
    pub word_spacing: Ems,
    /// OpenType feature tag/value pairs for shaping.
    pub features: Rc<Vec<(u32, u32)>>,
}

#[derive(Clone, Debug)]
pub struct ShapedText {
    pub text: Substr,
    pub width_in_ems: f32,
    pub glyphs: Vec<ShapedGlyph>,
    /// Unicode scripts of characters that no font in the family could cover
    /// (they shaped to `.notdef` / id 0). De-duplicated and free of the
    /// `Common`/`Inherited`/`Unknown` scripts (whitespace, combining marks,
    /// punctuation), which carry no useful fallback signal. Higher layers use
    /// this to ask the OS for a font covering each script (see
    /// `DrawText::layout` per-glyph system-font fallback).
    pub missing_scripts: Vec<Script>,
    /// Whether any uncovered glyph was an emoji code point. Emoji are Unicode
    /// `Script::Common` (so they never appear in `missing_scripts`); this flag
    /// is the separate signal that triggers on-demand loading of the color
    /// emoji font, which is otherwise never resident (it is huge — ~180MB).
    pub missing_emoji: bool,
}

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
    pub y_offset_in_ems: f32,
}

#[cfg(test)]
mod tests {
    use super::{collect_missing_scripts, GlyphId, Script};

    // Helper: byte offset of the nth char in `s`.
    fn byte_at(s: &str, n: usize) -> usize {
        s.char_indices().nth(n).unwrap().0
    }

    #[test]
    fn missing_thai_with_latin_only_reports_thai() {
        // Simulates the family covering Latin but not Thai: the Latin chars
        // shaped to real glyph ids, the Thai chars fell back to `.notdef` (0).
        let text = "Hi สวัสดี";
        let glyphs: Vec<(GlyphId, usize)> = vec![
            (10, byte_at(text, 0)), // H
            (11, byte_at(text, 1)), // i
            (0, byte_at(text, 3)),  // ส  -> notdef
            (0, byte_at(text, 4)),  // ว  -> notdef
        ];
        assert_eq!(collect_missing_scripts(text, glyphs), (vec![Script::Thai], false));
    }

    #[test]
    fn covered_text_reports_nothing() {
        // No `.notdef` glyphs -> no missing scripts.
        let text = "Hello";
        let glyphs: Vec<(GlyphId, usize)> =
            vec![(10, 0), (11, 1), (12, 2), (13, 3), (14, 4)];
        let (scripts, emoji) = collect_missing_scripts(text, glyphs);
        assert!(scripts.is_empty());
        assert!(!emoji);
    }

    #[test]
    fn deduplicates_and_preserves_first_seen_order() {
        let text = "नमस्ते สวัสดี";
        let deva0 = byte_at(text, 0); // न Devanagari
        let deva1 = byte_at(text, 1); // म Devanagari
        let thai = byte_at(text, 7); // ส Thai
        let glyphs: Vec<(GlyphId, usize)> =
            vec![(0, deva0), (0, deva1), (0, thai)];
        assert_eq!(
            collect_missing_scripts(text, glyphs),
            (vec![Script::Devanagari, Script::Thai], false)
        );
    }

    #[test]
    fn filters_common_inherited_unknown() {
        // A space (Common) and a digit (Common) that fell back to notdef carry
        // no useful fallback signal and must be filtered out.
        let text = "a 1";
        let glyphs: Vec<(GlyphId, usize)> = vec![
            (0, byte_at(text, 1)), // ' ' Common
            (0, byte_at(text, 2)), // '1' Common
        ];
        let (scripts, emoji) = collect_missing_scripts(text, glyphs);
        assert!(scripts.is_empty());
        assert!(!emoji);
    }

    #[test]
    fn detects_emoji_via_separate_flag() {
        // Emoji are Script::Common, so they must NOT appear in the script list,
        // but must set the missing_emoji flag when uncovered.
        let text = "a😀";
        let glyphs: Vec<(GlyphId, usize)> = vec![
            (10, byte_at(text, 0)), // 'a'
            (0, byte_at(text, 1)),  // 😀 -> notdef
        ];
        let (scripts, emoji) = collect_missing_scripts(text, glyphs);
        assert!(scripts.is_empty());
        assert!(emoji);
    }
}
