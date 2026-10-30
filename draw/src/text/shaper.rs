use {
    super::{
        font::{Font, GlyphId},
        font_family::FontDiagnostics,
        slice::SliceExt,
        substr::Substr,
    },
    fxhash::FxHashMap,
    rustybuzz,
    rustybuzz::UnicodeBuffer,
    std::{
        collections::{BTreeMap, HashSet},
        hash::{Hash, Hasher},
        mem,
        rc::Rc,
        sync::Mutex,
    },
    unicode_script::{Script, UnicodeScript},
    unicode_segmentation::UnicodeSegmentation,
};

static WARNED_MISSING_GLYPHS: Mutex<Option<HashSet<char>>> = Mutex::new(None);

fn mark_missing_glyph_for_warning(warned: &Mutex<Option<HashSet<char>>>, ch: char) -> bool {
    let Ok(mut warned) = warned.try_lock() else {
        return false;
    };
    warned.get_or_insert_with(HashSet::new).insert(ch)
}

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

/// A shaping cache entry, tracked with the tick it was last used at.
///
/// Hits only update `last_used`, so its registration in `Shaper::cache_lru_order` can be older.
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
        self.get_or_shape_with(params, |_, _| None)
    }

    /// Like [`Self::get_or_shape`], but `get_shaped_text` gets the first try at what isn't cached.
    pub(crate) fn get_or_shape_with(
        &mut self,
        params: ShapeParams,
        get_shaped_text: impl FnOnce(&mut Self, &ShapeParams) -> Option<Rc<ShapedText>>,
    ) -> Rc<ShapedText> {
        if self.cache_size == 0 {
            return get_shaped_text(self, &params).unwrap_or_else(|| Rc::new(self.shape(params)));
        }
        if let Some(entry) = self.cached_results.get_mut(&params) {
            // Refresh recency so the hot working set of words survives
            // eviction while scrolling through varied text.
            self.cache_tick += 1;
            entry.last_used = self.cache_tick;
            return entry.result.clone();
        }
        while self.cached_results.len() >= self.cache_size {
            let Some((tick, key)) = self.cache_lru_order.pop_first() else {
                break;
            };
            if let Some(entry) = self.cached_results.get(&key) {
                // It was used since we registered it, so it goes back in under its last use.
                if entry.last_used != tick {
                    self.cache_lru_order.insert(entry.last_used, key);
                    continue;
                }
            }
            self.cached_results.remove(&key);
        }
        let cache_key = params.clone();
        let result = get_shaped_text(self, &params).unwrap_or_else(|| Rc::new(self.shape(params)));
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

    /// Returns whether `params` is in the cache, without counting that as a use.
    pub(crate) fn is_cached(&self, params: &ShapeParams) -> bool {
        self.cached_results.contains_key(params)
    }

    /// Shapes `params.text` so its pieces can take their glyphs from it, unless they can't.
    ///
    /// They can't if the text might go right to left or needs a fallback font, since a piece could
    /// then shape differently on its own.
    pub(crate) fn shape_with_pieces(&mut self, params: ShapeParams) -> Option<ShapedText> {
        let font = params.fonts.first()?.clone();
        let text: &str = &params.text;
        // Text fonts have ASCII, and the check after shaping catches one that doesn't.
        let has_glyphs = || {
            font.with_ttf_parser_face(|face| {
                text.chars().all(|c| {
                    c.is_ascii() || face.glyph_index(c).is_some_and(|glyph_id| glyph_id.0 != 0)
                })
            })
        };
        if !is_definitely_ltr(text) || !has_glyphs() {
            return None;
        }
        let shaped_text = self.shape(params);
        shaped_text
            .glyphs
            .iter()
            .all(|glyph| glyph.id != 0 && Rc::ptr_eq(&glyph.font, &font))
            .then_some(shaped_text)
    }

    fn shape(&mut self, params: ShapeParams) -> ShapedText {
        let mut glyphs = Vec::new();
        if params.fonts.is_empty() {
            println!("WARNING: encountered empty font family");
        } else {
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
                    &params.diagnostics,
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
                        &params.diagnostics,
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
                            &params.diagnostics,
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

        ShapedText {
            text: params.text,
            width_in_ems: glyphs.iter().map(|glyph| glyph.advance_in_ems).sum(),
            glyphs,
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Logs one warning per unique codepoint that no loaded font can render,
    /// so ".notdef" boxes in the UI are explained in the log instead of being
    /// silently drawn (a missing arrow/symbol glyph is otherwise very hard to
    /// distinguish from a layout bug).
    fn warn_missing_glyph_once(
        text: &str,
        cluster: usize,
        diagnostics: &FontDiagnostics,
    ) {
        let Some(ch) = text.get(cluster..).and_then(|s| s.chars().next()) else {
            return;
        };
        // These misses are expected while their deferred policy face is in
        // flight. The family arrival has one aggregate log line instead.
        if crate::makepad_platform::LazyFontFamily::Cjk.contains(ch)
            || crate::makepad_platform::LazyFontFamily::Emoji.contains(ch)
        {
            return;
        }
        if mark_missing_glyph_for_warning(&WARNED_MISSING_GLYPHS, ch) {
            crate::makepad_platform::log!("{}", missing_glyph_message(ch, diagnostics));
        }
    }

    fn shape_recursive(
        &mut self,
        text: &str,
        primary_font: &Rc<Font>,
        fonts: &[Rc<Font>],
        features: &[(u32, u32)],
        diagnostics: &FontDiagnostics,
        direction: Direction,
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        let (font, remaining_fonts) = fonts.split_first().unwrap();
        let mut glyphs = self.reusable_glyphs.pop().unwrap_or_default();
        self.shape_step(text, font, features, direction, start, end, &mut glyphs);
        if !glyphs.iter().any(|glyph| glyph.id == 0) {
            out_glyphs.append(&mut glyphs);
            self.reusable_glyphs.push(glyphs);
            return;
        }

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
                        diagnostics,
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
                if remaining_fonts.is_empty() {
                    for glyph in glyph_group.iter().filter(|glyph| glyph.id == 0) {
                        Self::warn_missing_glyph_once(text, glyph.cluster, diagnostics);
                    }
                }
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
        let glyph_buffer = font.shape(&self.cached_rb_features, unicode_buffer);
        let units_per_em = font.units_per_em();
        // HarfBuzz keeps a script in its own direction, even under an override like LRO before
        // Hebrew, so the order of the clusters it returns tells us which way the glyphs go.
        let glyph_infos = glyph_buffer.glyph_infos();
        let direction = match (glyph_infos.first(), glyph_infos.last()) {
            (Some(first), Some(last)) if first.cluster > last.cluster => Direction::Rtl,
            (Some(first), Some(last)) if first.cluster < last.cluster => Direction::Ltr,
            _ => direction,
        };
        out_glyphs.extend(
            glyph_infos
                .iter()
                .zip(glyph_buffer.glyph_positions())
                .map(|(glyph_info, glyph_position)| ShapedGlyph {
                    font: font.clone(),
                    id: glyph_info.glyph_id as u16,
                    cluster: glyph_info.cluster as usize,
                    direction,
                    advance_in_ems: glyph_position.x_advance as f32 / units_per_em,
                    offset_in_ems: glyph_position.x_offset as f32 / units_per_em,
                    y_offset_in_ems: glyph_position.y_offset as f32 / units_per_em,
                    is_unsafe_to_break: glyph_info.unsafe_to_break(),
                }),
        );

        self.reusable_unicode_buffer = glyph_buffer.clear();
    }
}

fn missing_glyph_message(ch: char, diagnostics: &FontDiagnostics) -> String {
    format!(
        "font miss U+{:04X} role={} set={} tried=[{}]",
        ch as u32,
        diagnostics.role,
        diagnostics.set,
        diagnostics.tried.join(",")
    )
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub cache_size: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeParams {
    pub text: Substr,
    pub fonts: Rc<[Rc<Font>]>,
    pub direction: Direction,
    pub letter_spacing: Ems,
    pub word_spacing: Ems,
    /// OpenType feature tag/value pairs for shaping.
    pub features: Rc<Vec<(u32, u32)>>,
    pub diagnostics: Rc<FontDiagnostics>,
}

// The text alone tells keys apart well enough, and the rest takes longer to hash than it saves.
impl Hash for ShapeParams {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct ShapedText {
    pub text: Substr,
    pub width_in_ems: f32,
    pub glyphs: Vec<ShapedGlyph>,
}

impl ShapedText {
    /// Returns `piece`, at `start` in a text from [`Shaper::shape_with_pieces`], shaped on its own.
    ///
    /// That's our glyphs for it where HarfBuzz says breaking the text at both of its ends is safe
    /// and it has our script. Elsewhere it returns `None`.
    pub(crate) fn get_piece(&self, piece: &Substr, start: usize) -> Option<ShapedText> {
        let end = start + piece.len();
        let start_index = self.glyphs.partition_point(|glyph| glyph.cluster < start);
        let end_index = self.glyphs.partition_point(|glyph| glyph.cluster < end);
        let is_safe_to_break_at = |index: usize, cluster: usize| {
            cluster == 0
                || cluster == self.text.len()
                || self.glyphs.get(index).is_some_and(|glyph| {
                    glyph.cluster == cluster && !glyph.is_unsafe_to_break
                })
        };
        if !is_safe_to_break_at(start_index, start)
            || !is_safe_to_break_at(end_index, end)
            || guess_script(piece) != guess_script(&self.text)
        {
            return None;
        }
        let glyphs: Vec<ShapedGlyph> = self.glyphs[start_index..end_index]
            .iter()
            .map(|glyph| ShapedGlyph {
                cluster: glyph.cluster - start,
                ..glyph.clone()
            })
            .collect();
        Some(ShapedText {
            text: piece.clone(),
            width_in_ems: glyphs.iter().map(|glyph| glyph.advance_in_ems).sum(),
            glyphs,
        })
    }
}

/// Returns the script HarfBuzz picks for `text`, from the first char that has one of its own.
fn guess_script(text: &str) -> Option<Script> {
    text.chars()
        .map(|c| c.script())
        .find(|script| !matches!(script, Script::Common | Script::Inherited | Script::Unknown))
}

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub direction: Direction,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
    pub y_offset_in_ems: f32,
    /// Whether the text before and after this glyph's cluster could shape differently on their own.
    pub is_unsafe_to_break: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_glyph_diagnostic_has_frozen_contract_fields() {
        let diagnostics = FontDiagnostics {
            role: "regular".to_string(),
            set: "Latin".to_string(),
            tried: vec!["ibm_plex_text".to_string()],
        };
        assert_eq!(
            missing_glyph_message('中', &diagnostics),
            "font miss U+4E2D role=regular set=Latin tried=[ibm_plex_text]"
        );
    }

    #[test]
    fn missing_glyph_warning_is_emitted_once_per_codepoint() {
        let warned = Mutex::new(None);
        assert!(mark_missing_glyph_for_warning(&warned, '⌘'));
        assert!(!mark_missing_glyph_for_warning(&warned, '⌘'));
        assert!(mark_missing_glyph_for_warning(&warned, '⇧'));
        assert_eq!(warned.lock().unwrap().as_ref().unwrap().len(), 2);
    }
}
