use {
    super::{
        color::Color,
        font::{Font, FontId, GlyphId},
        font_family::{FontFamily, FontFamilyId},
        geom::{Point, Rect, Size},
        loader::{self, FontDefinition, FontFamilyDefinition, Loader},
        msdfer,
        num::Zero,
        rasterizer::{self, RasterizedGlyph, Rasterizer},
        sdfer,
        selection::{Cursor, CursorPosition, Selection},
        shaper::{self, ShapedText},
        substr::Substr,
    },
    fxhash::FxHashMap,
    std::{
        borrow::Borrow,
        cell::RefCell,
        collections::BTreeMap,
        env,
        hash::{Hash, Hasher},
        mem,
        rc::Rc,
    },
    unicode_segmentation::UnicodeSegmentation,
};

const LPXS_PER_INCH: f32 = 96.0;
const PTS_PER_INCH: f32 = 72.0;

/// Approximate upper bound, in bytes, on the memory retained by the layout cache.
/// Entry weights are estimates (text bytes plus laid-out row/glyph storage), where a
/// laid-out glyph is ~64 bytes, so a maximal ~60 KB pasted-wall message weighs about
/// 4 MB and a typical 2-10 KB code block 130-650 KB. This budget keeps several such
/// messages warm for scroll-back. Texts drawn in the current frame are never evicted
/// even over budget (see `evict_lru_to_limits`), and the excess is reclaimed at the
/// end of the first frame that no longer draws them (see `advance_cache_generation`),
/// so the true footprint can exceed this only briefly and only by the visible set.
pub const LAYOUT_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// A layout cache entry, tracked with its estimated size, its position in the
/// least-recently-used order (the tick under which it is registered in
/// `Layouter::cache_lru_order`), and the frame generation it was last used in.
#[derive(Debug)]
struct CachedLayout {
    result: Rc<LaidoutText>,
    weight_in_bytes: usize,
    last_used: u64,
    generation: u64,
}

#[derive(Debug)]
pub struct Layouter {
    pub(crate) loader: Loader,
    cache_size: usize,
    cache_tick: u64,
    cache_bytes: usize,
    /// Frame counter for working-set protection: eviction never removes entries
    /// used in the current generation, so the texts visible in one frame cannot
    /// evict each other into a permanent every-frame miss cycle when they
    /// collectively exceed the byte budget. Advanced once per frame via
    /// [`Self::advance_cache_generation`].
    cache_generation: u64,
    cached_results: FxHashMap<OwnedLayoutParams, CachedLayout>,
    cache_lru_order: BTreeMap<u64, OwnedLayoutParams>,
}

impl Layouter {
    pub fn new(settings: Settings) -> Self {
        Self {
            loader: Loader::new(settings.loader),
            cache_size: settings.cache_size,
            cache_tick: 0,
            cache_bytes: 0,
            cache_generation: 0,
            cached_results: FxHashMap::with_capacity_and_hasher(
                settings.cache_size,
                Default::default(),
            ),
            cache_lru_order: BTreeMap::new(),
        }
    }

    /// Marks a frame boundary for the cache's working-set protection; called once
    /// per frame from the font system's per-frame preparation, which runs after the
    /// frame's draws. Eviction runs first, while the finished frame's entries are
    /// still protected: anything older that pushed the cache over its limits (e.g.
    /// a huge message that just scrolled off screen) is reclaimed here, one frame
    /// after it was last drawn, rather than lingering until some later insert.
    pub fn advance_cache_generation(&mut self) {
        self.evict_lru_to_limits();
        self.cache_generation += 1;
    }

    pub fn rasterizer(&self) -> &Rc<RefCell<Rasterizer>> {
        self.loader.rasterizer()
    }

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        self.loader.is_font_family_known(id)
    }

    pub fn is_font_known(&self, id: FontId) -> bool {
        self.loader.is_font_known(id)
    }

    pub fn define_font_family(&mut self, id: FontFamilyId, definition: FontFamilyDefinition) {
        self.loader.define_font_family(id, definition);
    }

    pub fn set_font_family_definition(
        &mut self,
        id: FontFamilyId,
        definition: FontFamilyDefinition,
    ) {
        self.loader.set_font_family_definition(id, definition);
        self.cached_results.clear();
        self.cache_lru_order.clear();
        self.cache_bytes = 0;
    }

    pub fn define_font(&mut self, id: FontId, definition: FontDefinition) {
        self.loader.define_font(id, definition);
    }

    pub fn get_or_load_font_family(&mut self, id: FontFamilyId) -> Rc<FontFamily> {
        self.loader.get_or_load_font_family_rc(id)
    }

    pub fn get_or_layout(&mut self, params: impl LayoutParams) -> Rc<LaidoutText> {
        if self.cache_size == 0 {
            return Rc::new(self.layout(params.to_owned()));
        }
        if let Some(entry) = self.cached_results.get_mut(&params as &dyn LayoutParams) {
            // Refresh recency so texts that are drawn every frame (e.g. all the
            // visible items of a scrolling list) survive eviction.
            if let Some(key) = self.cache_lru_order.remove(&entry.last_used) {
                self.cache_tick += 1;
                entry.last_used = self.cache_tick;
                entry.generation = self.cache_generation;
                self.cache_lru_order.insert(entry.last_used, key);
            }
            return entry.result.clone();
        }
        let params = params.to_owned();
        let cache_key = params.clone();
        let result = Rc::new(self.layout(params));
        self.insert_cached_result(cache_key, result.clone());
        result
    }

    fn insert_cached_result(&mut self, cache_key: OwnedLayoutParams, result: Rc<LaidoutText>) {
        let weight_in_bytes = Self::entry_weight_in_bytes(&cache_key, &result);
        self.cache_tick += 1;
        self.cache_bytes = self.cache_bytes.saturating_add(weight_in_bytes);
        self.cache_lru_order.insert(self.cache_tick, cache_key.clone());
        if let Some(old) = self.cached_results.insert(
            cache_key,
            CachedLayout {
                result,
                weight_in_bytes,
                last_used: self.cache_tick,
                generation: self.cache_generation,
            },
        ) {
            // Replacing an existing entry: drop its LRU registration and weight
            // so the bookkeeping stays consistent.
            self.cache_lru_order.remove(&old.last_used);
            self.cache_bytes = self.cache_bytes.saturating_sub(old.weight_in_bytes);
        }
        self.evict_lru_to_limits();
    }

    /// Evicts least-recently-used entries until both the entry-count cap and the
    /// byte budget are respected, at a cost proportional to the number of entries
    /// evicted. Entries used in the current frame generation are never evicted:
    /// once the oldest remaining entry is current-generation, everything newer is
    /// too, and eviction stops. The budget is therefore soft-exceeded while a
    /// single frame's visible texts collectively outweigh it (they would otherwise
    /// evict each other and re-layout every frame); the excess is bounded by the
    /// visible working set and drains once scrolling moves on.
    fn evict_lru_to_limits(&mut self) {
        while self.cached_results.len() > 1
            && (self.cached_results.len() > self.cache_size
                || self.cache_bytes > LAYOUT_CACHE_MAX_BYTES)
        {
            let Some((tick, key)) = self.cache_lru_order.pop_first() else {
                break;
            };
            if self
                .cached_results
                .get(&key)
                .is_some_and(|entry| entry.generation == self.cache_generation)
            {
                self.cache_lru_order.insert(tick, key);
                break;
            }
            if let Some(entry) = self.cached_results.remove(&key) {
                self.cache_bytes = self.cache_bytes.saturating_sub(entry.weight_in_bytes);
            }
        }
    }

    /// Estimates the memory retained by one cache entry: the text bytes plus the
    /// laid-out row and glyph storage, plus the key stored in both the result map
    /// and the LRU order. This intentionally ignores allocator and hash-map
    /// overhead; the budget is a soft target, not an exact accounting.
    fn entry_weight_in_bytes(params: &OwnedLayoutParams, result: &LaidoutText) -> usize {
        let glyph_count: usize = result.rows.iter().map(|row| row.glyphs.len()).sum();
        params.text.len()
            + 2 * mem::size_of::<OwnedLayoutParams>()
            + mem::size_of::<CachedLayout>()
            + mem::size_of::<LaidoutText>()
            + result.rows.len() * mem::size_of::<LaidoutRow>()
            + glyph_count * mem::size_of::<LaidoutGlyph>()
    }

    fn layout(&mut self, params: OwnedLayoutParams) -> LaidoutText {
        let font_family = self
            .loader
            .get_or_load_font_family(params.style.font_family_id)
            .clone();
        LayoutContext::new(font_family, params.text, params.style, params.options)
            .layout_multiline()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub loader: loader::Settings,
    pub cache_size: usize,
}

impl Default for Settings {
    fn default() -> Self {
        let atlas_size = default_text_atlas_size();
        Self {
            loader: loader::Settings {
                shaper: shaper::Settings { cache_size: 4096 },
                rasterizer: rasterizer::Settings {
                    sdfer: sdfer::Settings {
                        padding: 4,
                        radius: 8.0,
                        cutoff: 0.25,
                    },
                    msdfer: msdfer::Settings {
                        padding: 4,
                        radius: 8.0,
                        cutoff: 0.25,
                        corner_angle_threshold: 3.0,
                    },
                    msdf_resolution: rasterizer::MsdfResolutionSettings {
                        min_request_dpxs_per_em: 20.0,
                        min_dpxs_per_em: 32.0,
                        base_dpxs_per_em: 64.0,
                        max_dpxs_per_em: 128.0,
                        target_feature_texels: 1.75,
                        dpx_quantum: 8.0,
                        min_feature_floor_ems: 1.0 / 1024.0,
                    },
                    msdf_complexity: rasterizer::MsdfComplexitySettings {
                        max_outline_commands: 180,
                        max_estimated_segments: 1000,
                    },
                    outline_rasterization_mode: rasterizer::OutlineRasterizationMode::Msdf,
                    atlas_size,
                },
            },
            cache_size: 4096,
        }
    }
}

fn default_text_atlas_size() -> Size<usize> {
    atlas_size_override_from_env().unwrap_or_else(|| Size::new(2048, 2048))
}

fn atlas_size_override_from_env() -> Option<Size<usize>> {
    let raw = env::var("MAKEPAD_TEXT_ATLAS_SIZE").ok()?;
    parse_text_atlas_size_value(raw.trim())
}

fn parse_text_atlas_size_value(value: &str) -> Option<Size<usize>> {
    if value.is_empty() {
        return None;
    }

    fn parse_dim(dim: &str) -> Option<usize> {
        let parsed = dim.trim().parse::<usize>().ok()?;
        if (256..=8192).contains(&parsed) {
            Some(parsed)
        } else {
            None
        }
    }

    if let Some((w, h)) = value.split_once('x').or_else(|| value.split_once('X')) {
        return Some(Size::new(parse_dim(w)?, parse_dim(h)?));
    }

    let dim = parse_dim(value)?;
    Some(Size::new(dim, dim))
}

#[derive(Debug)]
struct LayoutContext {
    font_family: Rc<FontFamily>,
    text: Substr,
    style: Style,
    options: LayoutOptions,
    current_point_in_lpxs: Point<f32>,
    current_row_start: usize,
    current_row_end: usize,
    rows: Vec<LaidoutRow>,
    glyphs: Vec<LaidoutGlyph>,
}

impl LayoutContext {
    fn new(
        font_family: Rc<FontFamily>,
        text: Substr,
        style: Style,
        options: LayoutOptions,
    ) -> Self {
        Self {
            font_family,
            text,
            style,
            options,
            current_point_in_lpxs: Point::new(options.first_row_indent_in_lpxs, 0.0),
            current_row_start: 0,
            current_row_end: 0,
            rows: Vec::new(),
            glyphs: Vec::new(),
        }
    }

    fn current_row_is_first(&self) -> bool {
        self.rows.is_empty()
    }

    fn current_row_is_continuation(&self) -> bool {
        self.current_row_is_first() && self.options.first_row_indent_in_lpxs > 0.0
    }

    fn current_row_is_empty(&self) -> bool {
        self.current_row_start == self.current_row_end
    }

    fn current_row_len(&self) -> usize {
        self.current_row_end - self.current_row_start
    }

    fn span_text(&self, len: usize) -> Substr {
        self.text
            .substr(self.current_row_end..self.current_row_end + len)
    }

    fn remaining_width_in_lpxs(&self) -> Option<f32> {
        if self.options.wrap {
            self.options
                .max_width_in_lpxs
                .map(|max_width_in_lpxs| max_width_in_lpxs - self.current_point_in_lpxs.x)
        } else {
            None
        }
    }

    fn layout_multiline(mut self) -> LaidoutText {
        let has_ellipsis_config = self.options.max_rows.is_some() || self.options.ellipsis;

        for (line_index, len) in self
            .text
            .clone()
            .split('\n')
            .map(|line| line.len())
            .enumerate()
        {
            if line_index != 0 {
                self.finish_current_row(true);
            }
            if self.is_past_max_rows() {
                break;
            }
            self.layout(len);
        }

        if has_ellipsis_config {
            self.apply_ellipsis_truncation()
        } else {
            self.finish_current_row(false);
            self.finish_with(false)
        }
    }

    fn is_past_max_rows(&self) -> bool {
        self.options
            .max_rows
            .map_or(false, |max| self.rows.len() >= max)
    }

    fn layout(&mut self, len: usize) {
        if self.remaining_width_in_lpxs().is_none() {
            self.layout_directly(len);
        } else {
            self.layout_by_word(len);
        }
    }

    fn layout_by_word(&mut self, len: usize) {
        let mut fitter = Fitter::new(
            self.span_text(len),
            self.font_family.clone(),
            self.style.font_size_in_lpxs(),
            SegmentKind::Word,
        );
        while !fitter.is_empty() {
            if self.is_past_max_rows() {
                break;
            }
            match fitter.fit(self.remaining_width_in_lpxs().unwrap()) {
                Some(text) => self.append_text(&text),
                None => {
                    let next_word = &self.text[self.current_row_end..][..fitter.next_len()];
                    if next_word.chars().all(|char| char.is_whitespace()) {
                        self.layout_directly(fitter.pop());
                    } else if self.current_row_is_empty() && !self.current_row_is_continuation() {
                        self.layout_by_grapheme(fitter.pop());
                    } else {
                        self.finish_current_row(false);
                    }
                }
            }
        }
    }

    fn layout_by_grapheme(&mut self, len: usize) {
        let mut fitter = Fitter::new(
            self.span_text(len),
            self.font_family.clone(),
            self.style.font_size_in_lpxs(),
            SegmentKind::Grapheme,
        );
        while !fitter.is_empty() {
            if self.is_past_max_rows() {
                break;
            }
            match fitter.fit(self.remaining_width_in_lpxs().unwrap()) {
                Some(text) => self.append_text(&text),
                None => {
                    if self.current_row_is_empty() {
                        self.layout_directly(fitter.pop());
                    } else {
                        self.finish_current_row(false);
                    }
                }
            }
        }
    }

    fn layout_directly(&mut self, len: usize) {
        self.append_text(
            &self.font_family.get_or_shape(
                self.text
                    .substr(self.current_row_end..self.current_row_end + len),
            ),
        );
    }

    fn append_text(&mut self, text: &ShapedText) {
        for glyph in &text.glyphs {
            let mut glyph = LaidoutGlyph {
                origin_in_lpxs: Point::ZERO,
                font: glyph.font.clone(),
                font_size_in_lpxs: self.style.font_size_in_lpxs(),
                color: self.style.color,
                id: glyph.id,
                cluster: self.current_row_len() + glyph.cluster,
                advance_in_ems: glyph.advance_in_ems,
                offset_in_ems: glyph.offset_in_ems,
            };
            glyph.origin_in_lpxs.x = self.current_point_in_lpxs.x;
            self.current_point_in_lpxs.x += glyph.advance_in_lpxs();
            self.glyphs.push(glyph);
        }
        self.current_row_end += text.text.len();
    }

    fn finish_current_row(&mut self, newline: bool) {
        let font = self.font_family.fonts().first();
        let font_size_in_lpxs = self.style.font_size_in_lpxs();
        let ascender_in_lpxs = font.map_or(0.0, |font| font.ascender_in_ems()) * font_size_in_lpxs;
        let descender_in_lpxs =
            font.map_or(0.0, |font| font.descender_in_ems()) * font_size_in_lpxs;
        let line_gap_in_lpxs = font.map_or(0.0, |font| font.line_gap_in_ems()) * font_size_in_lpxs;

        let text = self
            .text
            .substr(self.current_row_start..self.current_row_end);
        let width_in_lpxs = self.current_point_in_lpxs.x;

        let glyphs = mem::take(&mut self.glyphs);
        let mut row = LaidoutRow {
            origin_in_lpxs: Point::ZERO,
            text,
            newline,
            width_in_lpxs,
            ascender_in_lpxs,
            descender_in_lpxs,
            line_gap_in_lpxs,
            line_spacing_scale: self.options.line_spacing_scale,
            glyphs,
        };

        self.current_point_in_lpxs.x = 0.0;
        self.current_point_in_lpxs.y += self.rows.last().map_or(row.ascender_in_lpxs, |prev_row| {
            prev_row.line_spacing_in_lpxs(&row)
        });
        let max_width_in_lpxs = self.options.max_width_in_lpxs.unwrap_or(row.width_in_lpxs);
        let remaining_width_in_lpxs = max_width_in_lpxs - row.width_in_lpxs;
        row.origin_in_lpxs.x = self.options.align * remaining_width_in_lpxs;
        row.origin_in_lpxs.y = self.current_point_in_lpxs.y;
        self.current_row_start = self.current_row_end;
        if newline {
            self.current_row_start += 1;
            self.current_row_end += 1;
        }
        self.rows.push(row);
    }

    fn finish_with(self, is_truncated: bool) -> LaidoutText {
        let last_row = self.rows.last().unwrap();
        LaidoutText {
            text: self.text,
            size_in_lpxs: Size::new(
                self.rows
                    .iter()
                    .map(|row| row.width_in_lpxs)
                    .reduce(f32::max)
                    .unwrap_or(0.0),
                last_row.origin_in_lpxs.y - last_row.descender_in_lpxs,
            ),
            rows: self.rows,
            is_truncated,
        }
    }

    /// Applies ellipsis truncation as a post-processing step after layout.
    ///
    /// Truncation is triggered when:
    /// 1. `max_rows` is set and more text exists than fits in that many rows.
    /// 2. `ellipsis` is true, wrap is false, and a single row exceeds `max_width_in_lpxs`.
    fn apply_ellipsis_truncation(mut self) -> LaidoutText {
        self.finish_current_row_if_pending();

        // Detect whether all text was consumed during layout.
        // If not, text was truncated by the max_rows early-exit.
        let all_text_consumed = self.current_row_end >= self.text.len()
            || (self.current_row_end + 1 == self.text.len()
                && self.text.as_bytes()[self.current_row_end] == b'\n');

        let max_rows = match self.options.max_rows {
            Some(max) if max > 0 => max,
            _ => {
                // No max_rows constraint: check single-line non-wrapping overflow
                if self.options.ellipsis && !self.options.wrap {
                    if let Some(max_width) = self.options.max_width_in_lpxs {
                        if self.rows.len() == 1 && self.rows[0].width_in_lpxs > max_width {
                            self.truncate_last_row_with_ellipsis(max_width);
                            return self.finish_with(true);
                        }
                    }
                }
                return self.finish_with(false);
            }
        };

        let text_was_truncated = self.rows.len() > max_rows || !all_text_consumed;

        self.rows.truncate(max_rows);

        if !text_was_truncated {
            return self.finish_with(false);
        }

        if self.options.ellipsis {
            let max_width = self.options.max_width_in_lpxs.unwrap_or(f32::MAX);
            self.truncate_last_row_with_ellipsis(max_width);
        }

        self.finish_with(true)
    }

    /// Truncates the last row to fit within `max_width` and appends an ellipsis glyph.
    fn truncate_last_row_with_ellipsis(&mut self, max_width: f32) {
        let ellipsis_shaped = self.font_family.get_or_shape("…".into());
        let font_size_in_lpxs = self.style.font_size_in_lpxs();
        let ellipsis_width: f32 = ellipsis_shaped
            .glyphs
            .iter()
            .map(|g| g.advance_in_ems * font_size_in_lpxs)
            .sum();

        let last_row = match self.rows.last_mut() {
            Some(row) => row,
            None => return,
        };

        // Remove glyphs from the end until remaining + ellipsis fits
        while last_row.width_in_lpxs + ellipsis_width > max_width && !last_row.glyphs.is_empty() {
            let removed = last_row.glyphs.pop().unwrap();
            last_row.width_in_lpxs -= removed.advance_in_lpxs();
        }

        // Trim trailing whitespace glyphs for a cleaner look before the ellipsis.
        while last_row.glyphs.last().map_or(false, |g| {
            last_row.text[g.cluster..]
                .chars()
                .next()
                .map_or(false, |c| c.is_whitespace())
        }) {
            let removed = last_row.glyphs.pop().unwrap();
            last_row.width_in_lpxs -= removed.advance_in_lpxs();
        }

        // Append ellipsis glyphs
        for glyph in &ellipsis_shaped.glyphs {
            let ellipsis_glyph = LaidoutGlyph {
                origin_in_lpxs: Point::new(last_row.width_in_lpxs, 0.0),
                font: glyph.font.clone(),
                font_size_in_lpxs,
                color: self.style.color,
                id: glyph.id,
                cluster: last_row.text.len(), // beyond the text range
                advance_in_ems: glyph.advance_in_ems,
                offset_in_ems: glyph.offset_in_ems,
            };
            last_row.width_in_lpxs += ellipsis_glyph.advance_in_lpxs();
            last_row.glyphs.push(ellipsis_glyph);
        }
    }

    /// Finishes any pending glyphs into a row (without a newline).
    /// Always ensures at least one row exists.
    fn finish_current_row_if_pending(&mut self) {
        let has_pending_content =
            self.current_row_start != self.current_row_end || !self.glyphs.is_empty();
        if has_pending_content || self.rows.is_empty() {
            self.finish_current_row(false);
        }
    }
}

#[derive(Debug)]
struct Fitter {
    text: Substr,
    font_family: Rc<FontFamily>,
    font_size_in_lpxs: f32,
    lens: Vec<usize>,
    widths_in_lpxs: Vec<f32>,
}

impl Fitter {
    fn new(
        text: Substr,
        font_family: Rc<FontFamily>,
        font_size_in_lpxs: f32,
        segment_kind: SegmentKind,
    ) -> Self {
        let mut lens: Vec<_> = match segment_kind {
            SegmentKind::Word => text
                .split_word_bounds()
                .map(|segment| segment.len())
                .collect(),
            SegmentKind::Grapheme => text.graphemes(true).map(|segment| segment.len()).collect(),
        };
        if matches!(segment_kind, SegmentKind::Word) {
            merge_segments_for_line_breaking(&text, &mut lens);
        }
        let widths_in_lpxs: Vec<_> = lens
            .iter()
            .copied()
            .scan(0, |state, len| {
                let start = *state;
                let end = start + len;
                let segment = font_family.get_or_shape(text.substr(start..end));
                let width_in_lpxs = segment.width_in_ems * font_size_in_lpxs;
                *state = end;
                Some(width_in_lpxs)
            })
            .collect();
        Self {
            text,
            font_family,
            font_size_in_lpxs,
            lens,
            widths_in_lpxs,
        }
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn next_len(&self) -> usize {
        self.lens[0]
    }

    fn fit(&mut self, wrap_width_in_lpxs: f32) -> Option<Rc<ShapedText>> {
        let mut min_count = 1;
        let mut max_count = self.lens.len() + 1;
        let mut best_count = None;
        while min_count < max_count {
            let mid_count = (min_count + max_count) / 2;
            if self.can_fit(mid_count, wrap_width_in_lpxs) {
                best_count = Some(mid_count);
                min_count = mid_count + 1;
            } else {
                max_count = mid_count;
            }
        }
        if let Some(mut best_count) = best_count {
            while best_count > 0 {
                let best_len = self.lens[..best_count].iter().sum();
                let best_text = self.font_family.get_or_shape(self.text.substr(0..best_len));
                if best_text.width_in_ems * self.font_size_in_lpxs <= wrap_width_in_lpxs {
                    self.lens.drain(..best_count);
                    self.widths_in_lpxs.drain(..best_count);
                    self.text = self.text.substr(best_len..);
                    return Some(best_text);
                }
                best_count -= 1;
            }
        }
        None
    }

    fn can_fit(&self, count: usize, wrap_width_in_lpxs: f32) -> bool {
        // Use the pre-computed per-segment widths to estimate whether `count`
        // segments fit within the wrap width. This avoids calling get_or_shape()
        // on progressively longer substrings during the binary search — those
        // cumulative substrings are unique and always miss the shaper cache,
        // making each call a full HarfBuzz shape operation.
        //
        // The final candidate is shaped and checked exactly in `fit()` before it
        // is accepted, so this estimate can never allow an overflowing row.
        let estimated_width_in_lpxs: f32 = self.widths_in_lpxs[..count].iter().sum();
        estimated_width_in_lpxs <= wrap_width_in_lpxs
    }

    fn pop(&mut self) -> usize {
        let len = self.lens.remove(0);
        self.widths_in_lpxs.remove(0);
        self.text = self.text.substr(len..);
        len
    }
}

#[derive(Clone, Copy, Debug)]
enum SegmentKind {
    Word,
    Grapheme,
}

/// Merges word-boundary segments to prevent line breaks at typographically
/// incorrect positions, following standard line-breaking conventions
/// (UAX#14 / CSS Text Module Level 3).
///
/// Trailing/closing punctuation (`.` `,` `;` `)` etc.) is merged into the
/// preceding segment so it won't wrap to a new line by itself. Opening
/// punctuation (`(` `[` etc.) is merged into the following segment so it
/// won't be stranded at the end of a line.
fn merge_segments_for_line_breaking(text: &str, lens: &mut Vec<usize>) {
    // Pass 1: Merge "no-break-before" segments (trailing/closing punctuation)
    // into the preceding segment.
    if lens.len() >= 2 {
        let mut i = 1;
        let mut byte_offset = lens[0];
        while i < lens.len() {
            let seg_end = byte_offset + lens[i];
            let seg_text = &text[byte_offset..seg_end];
            if seg_text.chars().all(is_no_break_before_char) {
                lens[i - 1] += lens[i];
                lens.remove(i);
            } else {
                i += 1;
            }
            byte_offset = seg_end;
        }
    }

    // Pass 2: Merge "no-break-after" segments (opening punctuation)
    // into the following segment.
    if lens.len() >= 2 {
        let mut i = 0;
        let mut byte_offset = 0;
        while i + 1 < lens.len() {
            let seg_end = byte_offset + lens[i];
            let seg_text = &text[byte_offset..seg_end];
            if seg_text.chars().all(is_no_break_after_char) {
                lens[i + 1] = lens[i] + lens[i + 1];
                lens.remove(i);
            } else {
                byte_offset = seg_end;
                i += 1;
            }
        }
    }
}

/// Characters before which a line break should not occur (UAX#14 classes
/// CL, CP, EX, IS and common typographic conventions).
fn is_no_break_before_char(c: char) -> bool {
    matches!(
        c,
        // IS: Infix Numeric Separator
        '.' | ',' | ':' | ';'
        // EX: Exclamation / Interrogation
        | '!' | '?'
        // CP: Close Parenthesis, CL: Close Punctuation
        | ')' | ']' | '}'
        // CL: Closing quotation marks
        | '\u{2019}' // '
        | '\u{201D}' // \u{201d}
        | '\u{203A}' // ›
        | '\u{00BB}' // »
        // Other common no-break-before characters
        | '\u{2026}' // …
        | '%'
        | '\u{00B0}' // °
    )
}

/// Characters after which a line break should not occur (UAX#14 class OP).
fn is_no_break_after_char(c: char) -> bool {
    matches!(
        c,
        '(' | '[' | '{'
        // Opening quotation marks
        | '\u{2018}' // '
        | '\u{201C}' // \u{201c}
        | '\u{2039}' // ‹
        | '\u{00AB}' // «
    )
}

pub trait LayoutParams {
    fn to_owned(self) -> OwnedLayoutParams;
    fn text(&self) -> &str;
    fn style(&self) -> Style;
    fn options(&self) -> LayoutOptions;
}

impl Eq for dyn LayoutParams + '_ {}

impl Hash for dyn LayoutParams + '_ {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.text().hash(hasher);
        self.style().hash(hasher);
        self.options().hash(hasher);
    }
}

impl PartialEq for dyn LayoutParams + '_ {
    fn eq(&self, other: &Self) -> bool {
        if self.text() != other.text() {
            return false;
        }
        if self.style() != other.style() {
            return false;
        }
        if self.options() != other.options() {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OwnedLayoutParams {
    pub text: Substr,
    pub style: Style,
    pub options: LayoutOptions,
}

impl<'a> Borrow<dyn LayoutParams + 'a> for OwnedLayoutParams {
    fn borrow(&self) -> &(dyn LayoutParams + 'a) {
        self
    }
}

impl LayoutParams for OwnedLayoutParams {
    fn to_owned(self) -> Self {
        self
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn style(&self) -> Style {
        self.style
    }

    fn options(&self) -> LayoutOptions {
        self.options
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BorrowedLayoutParams<'a> {
    pub text: &'a str,
    pub style: Style,
    pub options: LayoutOptions,
}

impl<'a> Borrow<dyn LayoutParams + 'a> for BorrowedLayoutParams<'a> {
    fn borrow(&self) -> &(dyn LayoutParams + 'a) {
        self
    }
}

impl<'a> LayoutParams for BorrowedLayoutParams<'a> {
    fn to_owned(self) -> OwnedLayoutParams {
        OwnedLayoutParams {
            text: self.text.into(),
            style: self.style,
            options: self.options,
        }
    }

    fn text(&self) -> &str {
        self.text
    }

    fn style(&self) -> Style {
        self.style
    }

    fn options(&self) -> LayoutOptions {
        self.options
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub font_family_id: FontFamilyId,
    pub font_size_in_pts: f32,
    pub color: Option<Color>,
}

impl Style {
    fn font_size_in_lpxs(&self) -> f32 {
        self.font_size_in_pts * LPXS_PER_INCH / PTS_PER_INCH
    }
}

impl Eq for Style {}

impl Hash for Style {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.font_family_id.hash(hasher);
        self.font_size_in_pts.to_bits().hash(hasher);
        self.color.hash(hasher);
    }
}

impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        if self.font_family_id != other.font_family_id {
            return false;
        }
        if self.font_size_in_lpxs().to_bits() != other.font_size_in_lpxs().to_bits() {
            return false;
        }
        if self.color != other.color {
            return false;
        }
        true
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutOptions {
    pub first_row_indent_in_lpxs: f32,
    // Note: currently does nothing. Only used by `TextFlow`. Should be removed once `TextFlow` is
    // replaced with `TextFlow2`.
    pub first_row_min_line_spacing_below_in_lpxs: f32,
    pub max_width_in_lpxs: Option<f32>,
    pub wrap: bool,
    pub align: f32,
    pub line_spacing_scale: f32,
    /// Maximum number of rows to display. `None` means unlimited.
    /// When set and the text exceeds this many rows, excess rows are discarded.
    pub max_rows: Option<usize>,
    /// When true and text is truncated (by `max_rows` or by exceeding `max_width_in_lpxs`
    /// on a single non-wrapping line), an ellipsis character (U+2026 "…") is appended
    /// to the last visible row.
    pub ellipsis: bool,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            first_row_indent_in_lpxs: 0.0,
            first_row_min_line_spacing_below_in_lpxs: 0.0,
            max_width_in_lpxs: None,
            wrap: false,
            align: 0.0,
            line_spacing_scale: 1.0,
            max_rows: None,
            ellipsis: false,
        }
    }
}

impl Eq for LayoutOptions {}

impl Hash for LayoutOptions {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.first_row_indent_in_lpxs.to_bits().hash(hasher);
        self.first_row_min_line_spacing_below_in_lpxs
            .to_bits()
            .hash(hasher);
        self.max_width_in_lpxs.map(f32::to_bits).hash(hasher);
        self.wrap.hash(hasher);
        self.align.to_bits().hash(hasher);
        self.line_spacing_scale.to_bits().hash(hasher);
        self.max_rows.hash(hasher);
        self.ellipsis.hash(hasher);
    }
}

impl PartialEq for LayoutOptions {
    fn eq(&self, other: &Self) -> bool {
        self.first_row_indent_in_lpxs.to_bits() == other.first_row_indent_in_lpxs.to_bits()
            && self.first_row_min_line_spacing_below_in_lpxs.to_bits()
                == other.first_row_min_line_spacing_below_in_lpxs.to_bits()
            && self.max_width_in_lpxs.map(f32::to_bits) == other.max_width_in_lpxs.map(f32::to_bits)
            && self.wrap == other.wrap
            && self.align.to_bits() == other.align.to_bits()
            && self.line_spacing_scale.to_bits() == other.line_spacing_scale.to_bits()
            && self.max_rows == other.max_rows
            && self.ellipsis == other.ellipsis
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutText {
    pub text: Substr,
    pub size_in_lpxs: Size<f32>,
    pub rows: Vec<LaidoutRow>,
    /// True when the text was truncated (e.g., due to `max_rows` or ellipsis).
    pub is_truncated: bool,
}

impl LaidoutText {
    pub fn cursor_to_position(&self, cursor: Cursor) -> CursorPosition {
        let row_index = self.cursor_to_row_index(cursor);
        let row = &self.rows[row_index];
        let x_in_lpxs = row.index_to_x_in_lpxs(cursor.index - row.text.start_in_parent());
        CursorPosition {
            row_index,
            x_in_lpxs,
        }
    }

    fn cursor_to_row_index(&self, cursor: Cursor) -> usize {
        for (row_index, row) in self.rows.iter().enumerate() {
            if cursor.index < row.text.end_in_parent() {
                return row_index;
            }
            if cursor.index == row.text.end_in_parent() && (row.newline || !cursor.prefer_next_row)
            {
                return row_index;
            }
        }
        self.rows.len() - 1
    }

    pub fn point_in_lpxs_to_cursor(&self, point_in_lpxs: Point<f32>) -> Cursor {
        let row_index = self.y_in_lpxs_to_row_index(point_in_lpxs.y);
        self.position_to_cursor(CursorPosition {
            row_index,
            x_in_lpxs: point_in_lpxs.x,
        })
    }

    fn y_in_lpxs_to_row_index(&self, y_in_lpxs: f32) -> usize {
        if y_in_lpxs < 0.0 {
            return 0;
        }
        for (row_index, row) in self.rows.iter().enumerate() {
            let line_spacing_in_lpxs = self
                .rows
                .get(row_index + 1)
                .map_or(0.0, |next_row| row.line_spacing_in_lpxs(next_row));
            if y_in_lpxs < row.origin_in_lpxs.y + 0.5 * line_spacing_in_lpxs {
                return row_index;
            }
        }
        self.rows.len() - 1
    }

    pub fn position_to_cursor(&self, position: CursorPosition) -> Cursor {
        let row = &self.rows[position.row_index];
        let index = row.x_in_lpxs_to_index(position.x_in_lpxs);
        Cursor {
            index: row.text.start_in_parent() + index,
            prefer_next_row: index == 0,
        }
    }

    pub fn selection_rects(&self, selection: Selection) -> Vec<SelectionRect> {
        let CursorPosition {
            row_index: start_row_index,
            x_in_lpxs: start_x_in_lpxs,
        } = self.cursor_to_position(selection.start());
        let CursorPosition {
            row_index: end_row_index,
            x_in_lpxs: end_x_in_lpxs,
        } = self.cursor_to_position(selection.end());
        let mut selection_rects = Vec::new();
        if start_row_index == end_row_index {
            let row = &self.rows[start_row_index];
            selection_rects.push(SelectionRect {
                rect_in_lpxs: Rect::new(
                    Point::new(start_x_in_lpxs, row.origin_in_lpxs.y - row.ascender_in_lpxs),
                    Size::new(
                        end_x_in_lpxs - start_x_in_lpxs,
                        row.ascender_in_lpxs - row.descender_in_lpxs,
                    ),
                ),
                ascender_in_lpxs: row.ascender_in_lpxs,
            });
        } else {
            let start_row = &self.rows[start_row_index];
            let end_row = &self.rows[end_row_index];
            selection_rects.push(SelectionRect {
                rect_in_lpxs: Rect::new(
                    Point::new(
                        start_x_in_lpxs,
                        start_row.origin_in_lpxs.y - start_row.ascender_in_lpxs,
                    ),
                    Size::new(
                        start_row.width_in_lpxs - start_x_in_lpxs,
                        start_row.ascender_in_lpxs - start_row.descender_in_lpxs,
                    ),
                ),
                ascender_in_lpxs: start_row.ascender_in_lpxs,
            });
            for row_index in start_row_index + 1..end_row_index {
                let row = &self.rows[row_index];
                selection_rects.push(SelectionRect {
                    rect_in_lpxs: Rect::new(
                        Point::new(
                            row.origin_in_lpxs.x,
                            row.origin_in_lpxs.y - row.ascender_in_lpxs,
                        ),
                        Size::new(
                            row.width_in_lpxs,
                            row.ascender_in_lpxs - row.descender_in_lpxs,
                        ),
                    ),
                    ascender_in_lpxs: row.ascender_in_lpxs,
                });
            }
            selection_rects.push(SelectionRect {
                rect_in_lpxs: Rect::new(
                    Point::new(0.0, end_row.origin_in_lpxs.y - end_row.ascender_in_lpxs),
                    Size::new(
                        end_x_in_lpxs,
                        end_row.ascender_in_lpxs - end_row.descender_in_lpxs,
                    ),
                ),
                ascender_in_lpxs: end_row.ascender_in_lpxs,
            });
        }
        selection_rects
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionRect {
    pub rect_in_lpxs: Rect<f32>,
    pub ascender_in_lpxs: f32,
}

#[derive(Clone, Debug)]
pub struct LaidoutRow {
    pub origin_in_lpxs: Point<f32>,
    pub text: Substr,
    pub newline: bool,
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub line_gap_in_lpxs: f32,
    pub line_spacing_scale: f32,
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    pub fn line_spacing_in_lpxs(&self, next_row: &LaidoutRow) -> f32 {
        (self.line_gap_in_lpxs - self.descender_in_lpxs + next_row.ascender_in_lpxs)
            * next_row.line_spacing_scale
    }

    pub fn x_in_lpxs_to_index(&self, x_in_lpxs: f32) -> usize {
        use {super::slice::SliceExt, unicode_segmentation::UnicodeSegmentation};

        let mut glyph_groups = self
            .glyphs
            .group_by(|glyph_0, glyph_1| glyph_0.cluster == glyph_1.cluster)
            .peekable();
        while let Some(glyph_group) = glyph_groups.next() {
            let start = glyph_group[0].cluster;
            let start_x_in_lpxs = glyph_group[0].origin_in_lpxs.x;
            let next_glyph_group = glyph_groups.peek();
            let end = next_glyph_group.map_or(self.text.len(), |next_glyph_group| {
                next_glyph_group[0].cluster
            });
            let end_x_in_lpxs = next_glyph_group.map_or(self.width_in_lpxs, |next_glyph_group| {
                next_glyph_group[0].origin_in_lpxs.x
            });
            let width_in_lpxs = end_x_in_lpxs - start_x_in_lpxs;
            let grapheme_count = self.text[start..end].graphemes(true).count();
            let grapheme_width_in_lpxs = width_in_lpxs / grapheme_count as f32;
            let mut current_x_in_lpxs = start_x_in_lpxs;
            for (grapheme_start, _) in self.text[start..end].grapheme_indices(true) {
                if x_in_lpxs < current_x_in_lpxs + 0.5 * grapheme_width_in_lpxs {
                    return start + grapheme_start;
                }
                current_x_in_lpxs += grapheme_width_in_lpxs;
            }
        }
        self.text.len()
    }

    pub fn index_to_x_in_lpxs(&self, index: usize) -> f32 {
        use {super::slice::SliceExt, unicode_segmentation::UnicodeSegmentation};

        let mut glyph_groups = self
            .glyphs
            .group_by(|glyph_0, glyph_1| glyph_0.cluster == glyph_1.cluster)
            .peekable();
        while let Some(glyph_group) = glyph_groups.next() {
            let start = glyph_group[0].cluster;
            let start_x_in_lpxs = glyph_group[0].origin_in_lpxs.x;
            let end = glyph_groups
                .peek()
                .map_or(self.text.len(), |next_glyph_group| {
                    next_glyph_group[0].cluster
                });
            let end_x_in_lpxs = glyph_groups
                .peek()
                .map_or(self.width_in_lpxs, |next_glyph_group| {
                    next_glyph_group[0].origin_in_lpxs.x
                });
            let width_in_lpxs = end_x_in_lpxs - start_x_in_lpxs;
            let grapheme_count = self.text[start..end].graphemes(true).count();
            let grapheme_width_in_lpxs = width_in_lpxs / grapheme_count as f32;
            let mut current_x_in_lpxs = start_x_in_lpxs;
            for (grapheme_start, _) in self.text[start..end].grapheme_indices(true) {
                let grapheme_start = start + grapheme_start;
                if index == grapheme_start {
                    return current_x_in_lpxs;
                }
                current_x_in_lpxs += grapheme_width_in_lpxs;
            }
        }
        self.width_in_lpxs
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutGlyph {
    pub origin_in_lpxs: Point<f32>,
    pub font: Rc<Font>,
    pub font_size_in_lpxs: f32,
    pub color: Option<Color>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
}

impl LaidoutGlyph {
    pub fn advance_in_lpxs(&self) -> f32 {
        self.advance_in_ems * self.font_size_in_lpxs
    }

    pub fn offset_in_lpxs(&self) -> f32 {
        self.offset_in_ems * self.font_size_in_lpxs
    }

    pub fn ascender_in_lpxs(&self) -> f32 {
        self.font.ascender_in_ems() * self.font_size_in_lpxs
    }

    pub fn descender_in_lpxs(&self) -> f32 {
        self.font.descender_in_ems() * self.font_size_in_lpxs
    }

    pub fn line_gap_in_lpxs(&self) -> f32 {
        self.font.line_gap_in_ems() * self.font_size_in_lpxs
    }

    pub fn rasterize(&self, dpx_per_em: f32) -> Option<RasterizedGlyph> {
        self.font.rasterize_glyph(self.id, dpx_per_em)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        merge_segments_for_line_breaking, parse_text_atlas_size_value, LaidoutText, LayoutOptions,
        Layouter, OwnedLayoutParams, Settings, Size, Style, LAYOUT_CACHE_MAX_BYTES,
    };
    use std::rc::Rc;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn parses_text_atlas_size_from_env_value() {
        assert_eq!(
            parse_text_atlas_size_value("1024"),
            Some(Size::new(1024, 1024))
        );
        assert_eq!(
            parse_text_atlas_size_value("1024x2048"),
            Some(Size::new(1024, 2048))
        );
        assert_eq!(
            parse_text_atlas_size_value("1024X2048"),
            Some(Size::new(1024, 2048))
        );
        assert_eq!(parse_text_atlas_size_value(""), None);
        assert_eq!(parse_text_atlas_size_value("64"), None);
        assert_eq!(parse_text_atlas_size_value("bogus"), None);
    }

    /// Helper: split text by word bounds and return segment lengths,
    /// then apply merging, then reconstruct the segment strings.
    fn merged_segments(text: &str) -> Vec<String> {
        let mut lens: Vec<usize> = text.split_word_bounds().map(|s| s.len()).collect();
        merge_segments_for_line_breaking(text, &mut lens);
        let mut result = Vec::new();
        let mut offset = 0;
        for len in &lens {
            result.push(text[offset..offset + len].to_string());
            offset += len;
        }
        result
    }

    #[test]
    fn trailing_punctuation_merges_into_preceding_word() {
        assert_eq!(merged_segments("Hello."), vec!["Hello."]);
        assert_eq!(merged_segments("Hello,"), vec!["Hello,"]);
        assert_eq!(merged_segments("Hello;"), vec!["Hello;"]);
        assert_eq!(merged_segments("Hello!"), vec!["Hello!"]);
        assert_eq!(merged_segments("Hello?"), vec!["Hello?"]);
    }

    #[test]
    fn multiple_trailing_punctuation_marks_merge() {
        assert_eq!(merged_segments("end.)"), vec!["end.)"]);
        assert_eq!(merged_segments("end...)"), vec!["end...)"]);
    }

    #[test]
    fn punctuation_between_words_merges_correctly() {
        assert_eq!(
            merged_segments("Hello, world."),
            vec!["Hello,", " ", "world."]
        );
    }

    #[test]
    fn opening_punctuation_merges_into_following_word() {
        assert_eq!(
            merged_segments("say (hello) now"),
            vec!["say", " ", "(hello)", " ", "now"]
        );
    }

    #[test]
    fn consecutive_trailing_punctuation_chains_merge() {
        // ")" and ":" are both no-break-before, so they chain-merge
        // into the preceding word.
        assert_eq!(
            merged_segments("(News Hello): world"),
            vec!["(News", " ", "Hello):", " ", "world"]
        );
    }

    #[test]
    fn no_merge_for_plain_words() {
        assert_eq!(merged_segments("hello world"), vec!["hello", " ", "world"]);
    }

    #[test]
    fn single_word_no_change() {
        assert_eq!(merged_segments("hello"), vec!["hello"]);
    }

    #[test]
    fn empty_text() {
        assert_eq!(merged_segments(""), Vec::<String>::new());
    }

    fn cache_test_params(text: &str) -> OwnedLayoutParams {
        OwnedLayoutParams {
            text: text.into(),
            style: Style {
                font_family_id: 0u64.into(),
                font_size_in_pts: 12.0,
                color: None,
            },
            options: LayoutOptions::default(),
        }
    }

    /// Builds a synthetic cache entry so cache behavior can be exercised
    /// without loading real fonts.
    fn cache_test_result(text: &str) -> Rc<LaidoutText> {
        Rc::new(LaidoutText {
            text: text.into(),
            size_in_lpxs: Size::new(0.0, 0.0),
            rows: Vec::new(),
            is_truncated: false,
        })
    }

    #[test]
    fn layout_cache_admits_long_texts_and_hits_refresh_recency() {
        let mut settings = Settings::default();
        settings.cache_size = 3;
        let mut layouter = Layouter::new(settings);

        // Texts well beyond the old 512-byte admission limit must be cacheable.
        // Each insert happens in its own frame generation so the working-set
        // protection doesn't suppress eviction.
        let long_a = "a".repeat(16 * 1024);
        let long_b = "b".repeat(16 * 1024);
        let long_c = "c".repeat(16 * 1024);
        let long_d = "d".repeat(16 * 1024);
        layouter.insert_cached_result(cache_test_params(&long_a), cache_test_result(&long_a));
        layouter.advance_cache_generation();
        layouter.insert_cached_result(cache_test_params(&long_b), cache_test_result(&long_b));
        layouter.advance_cache_generation();
        layouter.insert_cached_result(cache_test_params(&long_c), cache_test_result(&long_c));
        layouter.advance_cache_generation();

        // A cache hit must not re-layout (which would require loading fonts)
        // and must refresh the entry's LRU position.
        let hit = layouter.get_or_layout(cache_test_params(&long_a));
        assert!(Rc::ptr_eq(
            &hit,
            &layouter
                .cached_results
                .get(&cache_test_params(&long_a))
                .unwrap()
                .result
        ));

        // The cache is full, so inserting a fourth entry (in a later frame)
        // evicts the least recently used one, which is now `long_b` rather
        // than `long_a`.
        layouter.advance_cache_generation();
        layouter.insert_cached_result(cache_test_params(&long_d), cache_test_result(&long_d));
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&long_a)));
        assert!(!layouter
            .cached_results
            .contains_key(&cache_test_params(&long_b)));
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&long_c)));
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&long_d)));
        assert_eq!(
            layouter.cached_results.len(),
            layouter.cache_lru_order.len()
        );
    }

    #[test]
    fn layout_cache_evicts_to_byte_budget() {
        let mut layouter = Layouter::new(Settings::default());

        // Three entries of ~40% of the budget each, inserted in separate frame
        // generations: the third insert must push the total over the budget and
        // evict the oldest entry.
        let weight = LAYOUT_CACHE_MAX_BYTES * 2 / 5;
        let text_a = "a".repeat(weight);
        let text_b = "b".repeat(weight);
        let text_c = "c".repeat(weight);
        layouter.insert_cached_result(cache_test_params(&text_a), cache_test_result(&text_a));
        layouter.advance_cache_generation();
        layouter.insert_cached_result(cache_test_params(&text_b), cache_test_result(&text_b));
        layouter.advance_cache_generation();
        layouter.insert_cached_result(cache_test_params(&text_c), cache_test_result(&text_c));

        assert!(!layouter
            .cached_results
            .contains_key(&cache_test_params(&text_a)));
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&text_b)));
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&text_c)));
        assert!(layouter.cache_bytes <= LAYOUT_CACHE_MAX_BYTES);

        // An entry heavier than the whole budget is still kept (as the sole
        // survivor), so oversized texts don't re-layout on every draw.
        let huge = "h".repeat(LAYOUT_CACHE_MAX_BYTES + 1);
        layouter.advance_cache_generation();
        layouter.insert_cached_result(cache_test_params(&huge), cache_test_result(&huge));
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&huge)));
        assert_eq!(layouter.cached_results.len(), 1);
    }

    #[test]
    fn layout_cache_protects_current_frame_working_set() {
        let mut layouter = Layouter::new(Settings::default());

        // A single frame whose visible texts collectively exceed the budget must
        // keep them all cached; evicting them would make each one a guaranteed
        // miss on every subsequent frame.
        let weight = LAYOUT_CACHE_MAX_BYTES * 2 / 5;
        let texts: Vec<String> = (0..4)
            .map(|i| char::from(b'a' + i as u8).to_string().repeat(weight))
            .collect();
        for text in &texts {
            layouter.insert_cached_result(cache_test_params(text), cache_test_result(text));
        }
        for text in &texts {
            assert!(layouter.cached_results.contains_key(&cache_test_params(text)));
        }

        // Once a new frame starts without touching them, they become evictable
        // and the budget is enforced again.
        layouter.advance_cache_generation();
        let fresh = "z".repeat(weight);
        layouter.insert_cached_result(cache_test_params(&fresh), cache_test_result(&fresh));
        assert!(layouter.cache_bytes <= LAYOUT_CACHE_MAX_BYTES);
        assert!(layouter
            .cached_results
            .contains_key(&cache_test_params(&fresh)));
    }

    #[test]
    fn layout_cache_reclaims_over_budget_memory_at_frame_boundaries() {
        let mut layouter = Layouter::new(Settings::default());

        // A frame draws texts that collectively exceed the budget; they are all
        // kept for that frame.
        let weight = LAYOUT_CACHE_MAX_BYTES * 2 / 5;
        let texts: Vec<String> = (0..4)
            .map(|i| char::from(b'a' + i as u8).to_string().repeat(weight))
            .collect();
        for text in &texts {
            layouter.insert_cached_result(cache_test_params(text), cache_test_result(text));
        }
        assert!(layouter.cache_bytes > LAYOUT_CACHE_MAX_BYTES);

        // The frame boundary right after that frame still protects its entries.
        layouter.advance_cache_generation();
        assert!(layouter.cache_bytes > LAYOUT_CACHE_MAX_BYTES);

        // The boundary after the first frame that no longer draws them reclaims
        // the excess without waiting for a new insert.
        layouter.advance_cache_generation();
        assert!(layouter.cache_bytes <= LAYOUT_CACHE_MAX_BYTES);
    }
}
