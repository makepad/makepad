use {
    super::{
        color::Color,
        font::{Font, FontId, GlyphId},
        font_family::{FontFamily, FontFamilyId},
        geom::{Point, Rect, Size},
        line_break::{find_first_line_break, find_line_breaks, FollowingContext, PrecedingContext},
        loader::{self, FontDefinition, FontFamilyDefinition, Loader},
        msdfer,
        num::Zero,
        rasterizer::{self, RasterizedGlyph, Rasterizer},
        sdfer,
        selection::{Cursor, CursorPosition, Selection},
        shaper::{self, Direction, ShapedGlyph, ShapedText},
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

/// A layout cache entry, tracked with its estimated size, the tick and frame generation it
/// was last used in, and the tick under which it is registered in `Layouter::cache_lru_order`.
///
/// Hits only update `last_used`, so the registration falls behind until eviction gets to it.
#[derive(Debug)]
struct CachedLayout {
    result: Rc<LaidoutText>,
    weight_in_bytes: usize,
    last_used: u64,
    registered_tick: u64,
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
    pub(crate) font_family_generation: u64,
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
            font_family_generation: 0,
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

    /// Bytes retained by the laid-out text cache.
    pub fn cache_bytes(&self) -> usize {
        self.cache_bytes
    }

    /// Goes up whenever a font family changes, like when a fallback font arrives or gets asked for.
    ///
    /// Anyone who keeps their own text measurements can compare it to tell when they're stale.
    pub fn font_family_generation(&self) -> u64 {
        self.font_family_generation
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
        if self.loader.set_font_family_definition(id, definition) {
            self.font_family_generation += 1;
            self.cached_results.clear();
            self.cache_lru_order.clear();
            self.cache_bytes = 0;
        }
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
            self.cache_tick += 1;
            entry.last_used = self.cache_tick;
            entry.generation = self.cache_generation;
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
                registered_tick: self.cache_tick,
                generation: self.cache_generation,
            },
        ) {
            // Replacing an existing entry: drop its LRU registration and weight
            // so the bookkeeping stays consistent.
            self.cache_lru_order.remove(&old.registered_tick);
            self.cache_bytes = self.cache_bytes.saturating_sub(old.weight_in_bytes);
        }
        self.evict_lru_to_limits();
    }

    /// Evicts least-recently-used entries until both the entry-count cap and the
    /// byte budget are respected, at a cost proportional to the number of entries
    /// evicted or registered again. Entries used in the current frame generation
    /// are never evicted: once the oldest remaining entry is current-generation,
    /// everything newer is too, and eviction stops. The budget is therefore
    /// soft-exceeded while a single frame's visible texts collectively outweigh it
    /// (they would otherwise evict each other and re-layout every frame); the
    /// excess is bounded by the visible working set and drains once scrolling
    /// moves on.
    fn evict_lru_to_limits(&mut self) {
        while self.cached_results.len() > 1
            && (self.cached_results.len() > self.cache_size
                || self.cache_bytes > LAYOUT_CACHE_MAX_BYTES)
        {
            let Some((tick, key)) = self.cache_lru_order.pop_first() else {
                break;
            };
            if let Some(entry) = self.cached_results.get_mut(&key) {
                // It was used since we registered it, so it goes back in under its last use.
                if entry.last_used != tick {
                    entry.registered_tick = entry.last_used;
                    self.cache_lru_order.insert(entry.last_used, key);
                    continue;
                }
                if entry.generation == self.cache_generation {
                    self.cache_lru_order.insert(tick, key);
                    break;
                }
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
    has_break_at_start: bool,
    is_glued_to_next_run: bool,
    is_trailing_glue_reserved: bool,
    // What we draw at the end of a row that wraps right after a soft hyphen.
    hyphen: Option<Rc<ShapedText>>,
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
            has_break_at_start: false,
            is_glued_to_next_run: false,
            is_trailing_glue_reserved: false,
            hyphen: None,
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
        if self.remaining_width_in_lpxs().is_some() && self.text.contains('\u{AD}') {
            self.hyphen = Some(
                self.font_family
                    .get_or_shape(get_hyphen(&self.font_family).into()),
            );
        }

        // Every hard line break ends a line, and we wrap each line at its soft breaks.
        let text = self.text.clone();
        let mut soft_breaks = Vec::new();
        let mut has_break_at_end = false;
        for line_break in find_line_breaks(
            &text,
            self.options.line_break_before,
            self.options.line_break_after,
        ) {
            has_break_at_end = line_break.byte_index == text.len();
            if line_break.byte_index == 0 {
                self.has_break_at_start = true;
            } else if line_break.is_forced {
                if self.is_past_max_rows() {
                    break;
                }
                self.layout_line(line_break.byte_index - line_break.newline_len, &soft_breaks);
                soft_breaks.clear();
                // A line that `max_rows` cut short doesn't get to end with its line break.
                if self.is_past_max_rows() {
                    break;
                }
                self.finish_current_row(RowEnd::ForcedBreak {
                    newline_len: line_break.newline_len,
                });
            } else if line_break.byte_index < text.len() {
                soft_breaks.push(line_break.byte_index);
            }
        }
        if !self.is_past_max_rows() {
            let mut text_end = text.len();
            if !has_break_at_end {
                if text.ends_with('\r') {
                    // This CR starts a CR LF that the next run ends, so we don't draw it.
                    text_end -= 1;
                } else {
                    self.is_glued_to_next_run = true;
                }
            }
            self.layout_line(text_end, &soft_breaks);
        }

        if has_ellipsis_config {
            self.apply_ellipsis_truncation()
        } else {
            self.finish_current_row(RowEnd::TextEnd);
            self.finish_with(false)
        }
    }

    fn is_past_max_rows(&self) -> bool {
        self.options
            .max_rows
            .map_or(false, |max| self.rows.len() >= max)
    }

    /// Whether the row currently being laid out is the last one `max_rows`
    /// permits.
    fn current_row_is_last_allowed(&self) -> bool {
        self.options
            .max_rows
            .map_or(false, |max| self.rows.len() + 1 >= max)
    }

    fn layout_line(&mut self, line_end: usize, soft_breaks: &[usize]) {
        if self.remaining_width_in_lpxs().is_none() {
            self.layout_directly(line_end - self.current_row_end);
        } else {
            self.layout_by_unit(line_end, soft_breaks);
        }
    }

    /// Wraps the units of a line, which are the pieces of text between its break opportunities.
    fn layout_by_unit(&mut self, line_end: usize, soft_breaks: &[usize]) {
        let line_start = self.current_row_end;
        if line_start == line_end {
            return;
        }
        let font_size_in_lpxs = self.style.font_size_in_lpxs();
        let hyphen_width_in_lpxs = self
            .hyphen
            .as_ref()
            .map_or(0.0, |hyphen| hyphen.width_in_ems * font_size_in_lpxs);
        let mut shaped_line = ShapedLine::new(self.text.substr(line_start..line_end));
        let mut unit_start = line_start;
        let units = soft_breaks
            .iter()
            .copied()
            .chain([line_end])
            .enumerate()
            .map(|(index, unit_end)| {
                let unit = self.text.substr(unit_start..unit_end);
                unit_start = unit_end;
                let glue_width_in_lpxs = if self.is_glued_to_next_run && unit_end == self.text.len()
                {
                    self.options.trailing_glue_width_in_lpxs.max(0.0)
                } else {
                    0.0
                };
                // Spaces with glue after them can't hang, so they count toward the unit's width.
                let hanging_len = if glue_width_in_lpxs > 0.0 {
                    0
                } else {
                    unit.len() - unit.trim_end_matches(is_hangable_white_space).len()
                };
                let is_soft_hyphen_break = unit_end < line_end && unit.ends_with('\u{AD}');
                let next_unit_ends = soft_breaks
                    .iter()
                    .chain([&line_end])
                    .skip(index + 1)
                    .map(|&unit_end| unit_end - line_start);
                let shaped_unit =
                    shaped_line.get_or_shape_unit(&self.font_family, unit, next_unit_ends);
                Segment {
                    hyphen_width_in_lpxs: if is_soft_hyphen_break {
                        hyphen_width_in_lpxs
                    } else {
                        0.0
                    },
                    glue_width_in_lpxs,
                    ..Segment::new(&shaped_unit, font_size_in_lpxs, hanging_len)
                }
            })
            .collect();
        let mut fitter = Fitter {
            text: self.text.substr(line_start..line_end),
            shaped_line,
            font_family: self.font_family.clone(),
            font_size_in_lpxs,
            segments: units,
            next_segment_index: 0,
        };
        while !fitter.is_empty() {
            if self.is_past_max_rows() {
                break;
            }
            match fitter.fit(self.remaining_width_in_lpxs().unwrap()) {
                Some(text) => self.append_fitted_text(&text),
                None => {
                    let unit = fitter.next_segment();
                    if unit.hanging_len == unit.len {
                        // Hanging white space never wraps, even on a row that's already full.
                        self.layout_directly(fitter.pop().len);
                    } else if self.current_row_end == 0
                        && self.current_row_is_continuation()
                        && !self.has_break_at_start
                        && self.options.is_start_reserved
                    {
                        // The previous run kept room for this unit on its row.
                        let text = self.font_family.get_or_shape(self.span_text(fitter.pop().len));
                        self.append_fitted_text(&text);
                    } else if self.options.ellipsis
                        && self.current_row_is_last_allowed()
                        && !self.current_row_is_continuation()
                    {
                        // The last permitted row ends in an ellipsis, so word
                        // integrity is moot: fill it to the width limit by
                        // grapheme so the ellipsis truncates at the last glyph
                        // that fits instead of at the last whole word — a word
                        // that wraps away from this row would otherwise leave
                        // it ellipsized far short of the available width.
                        // Continuation rows are excluded: grapheme layout
                        // force-places a grapheme wider than an empty row's
                        // remnant past the width limit, unflagged, whereas
                        // finishing the row truncates within bounds.
                        self.layout_by_grapheme(fitter.pop());
                    } else if self.current_row_is_empty() && !self.current_row_is_continuation() {
                        // A unit that starts a row stays whole if it fits with its hyphen, even if
                        // its glue doesn't, and otherwise breaks between graphemes.
                        let unit = fitter.pop();
                        if unit.width_without_hanging_in_lpxs + unit.hyphen_width_in_lpxs
                            <= self.remaining_width_in_lpxs().unwrap()
                        {
                            self.layout_directly(unit.len);
                        } else {
                            self.layout_by_grapheme(unit);
                        }
                    } else {
                        self.finish_current_row(RowEnd::SoftWrap);
                    }
                }
            }
        }
    }

    fn layout_by_grapheme(&mut self, unit: Segment) {
        let font_size_in_lpxs = self.style.font_size_in_lpxs();
        let text = self.span_text(unit.len);
        // The unit's hanging white space, hyphen and glue stay with its last grapheme.
        let mut grapheme_start = 0;
        let graphemes = text[..unit.len - unit.hanging_len]
            .grapheme_indices(true)
            .skip(1)
            .map(|(grapheme_end, _)| grapheme_end)
            .chain([unit.len])
            .map(|grapheme_end| {
                let grapheme = self
                    .font_family
                    .get_or_shape(text.substr(grapheme_start..grapheme_end));
                grapheme_start = grapheme_end;
                if grapheme_end == unit.len {
                    Segment {
                        hyphen_width_in_lpxs: unit.hyphen_width_in_lpxs,
                        glue_width_in_lpxs: unit.glue_width_in_lpxs,
                        ..Segment::new(&grapheme, font_size_in_lpxs, unit.hanging_len)
                    }
                } else {
                    Segment::new(&grapheme, font_size_in_lpxs, 0)
                }
            })
            .collect();
        let mut fitter = Fitter {
            text: text.clone(),
            shaped_line: ShapedLine::new(text),
            font_family: self.font_family.clone(),
            font_size_in_lpxs,
            segments: graphemes,
            next_segment_index: 0,
        };
        while !fitter.is_empty() {
            if self.is_past_max_rows() {
                break;
            }
            match fitter.fit(self.remaining_width_in_lpxs().unwrap()) {
                Some(text) => self.append_fitted_text(&text),
                None => {
                    if self.current_row_is_empty() {
                        self.layout_directly(fitter.pop().len);
                    } else {
                        self.finish_current_row(RowEnd::EmergencyWrap);
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
        self.append_glyphs(&text.glyphs, self.current_row_len());
        self.current_row_end += text.text.len();
    }

    /// Appends text we made room for.
    ///
    /// When it reaches the end of the text, we made room for the trailing glue after it too.
    fn append_fitted_text(&mut self, text: &ShapedText) {
        self.append_text(text);
        if self.current_row_end == self.text.len() {
            self.is_trailing_glue_reserved = true;
        }
    }

    fn append_glyphs(&mut self, glyphs: &[ShapedGlyph], cluster_start: usize) {
        for glyph in glyphs {
            let mut glyph = LaidoutGlyph {
                origin_in_lpxs: Point::ZERO,
                font: glyph.font.clone(),
                font_size_in_lpxs: self.style.font_size_in_lpxs(),
                color: self.style.color,
                id: glyph.id,
                cluster: cluster_start + glyph.cluster,
                direction: glyph.direction,
                advance_in_ems: glyph.advance_in_ems,
                offset_in_ems: glyph.offset_in_ems,
            };
            glyph.origin_in_lpxs.x = self.current_point_in_lpxs.x;
            self.current_point_in_lpxs.x += glyph.advance_in_lpxs();
            self.glyphs.push(glyph);
        }
    }

    fn finish_current_row(&mut self, row_end: RowEnd) {
        if row_end == RowEnd::SoftWrap
            && self.text[self.current_row_start..self.current_row_end].ends_with('\u{AD}')
        {
            if let Some(hyphen) = self.hyphen.clone() {
                // The hyphen joins its soft hyphen's cluster, so carets see one char there.
                self.append_glyphs(&hyphen.glyphs, self.current_row_len() - '\u{AD}'.len_utf8());
            }
        }
        let font = self.font_family.fonts().first();
        let font_size_in_lpxs = self.style.font_size_in_lpxs();
        let ascender_in_lpxs = font.map_or(0.0, |font| font.ascender_in_ems()) * font_size_in_lpxs;
        let descender_in_lpxs =
            font.map_or(0.0, |font| font.descender_in_ems()) * font_size_in_lpxs;
        let line_gap_in_lpxs = font.map_or(0.0, |font| font.line_gap_in_ems()) * font_size_in_lpxs;
        let cap_height_in_lpxs = font.map_or(0.0, |font| font.cap_height_in_ems()) * font_size_in_lpxs;

        let text = self
            .text
            .substr(self.current_row_start..self.current_row_end);
        let width_in_lpxs = self.current_point_in_lpxs.x;
        let is_followed_by_glue = row_end == RowEnd::TextEnd
            && self.is_glued_to_next_run
            && self.options.trailing_glue_width_in_lpxs > 0.0;
        let hanging_width_in_lpxs = match self.remaining_width_in_lpxs() {
            None => 0.0,
            Some(_) if is_followed_by_glue => 0.0,
            Some(remaining_width_in_lpxs) => {
                // White space at the end of a wrapped row hangs past its edge. At a hard line
                // break or the end of the text, only the part that doesn't fit hangs.
                let hanging_start = text.trim_end_matches(is_hangable_white_space).len();
                let trailing_white_space_width_in_lpxs: f32 = self
                    .glyphs
                    .iter()
                    .filter(|glyph| glyph.cluster >= hanging_start)
                    .map(|glyph| glyph.advance_in_lpxs())
                    .sum();
                let hanging_width_in_lpxs =
                    if matches!(row_end, RowEnd::SoftWrap | RowEnd::EmergencyWrap) {
                        trailing_white_space_width_in_lpxs
                    } else {
                        (-remaining_width_in_lpxs)
                            .min(trailing_white_space_width_in_lpxs)
                            .max(0.0)
                    };
                // A row can start past the edge, after the spaces that hang at the end of the
                // previous run, and if nothing past the edge takes room, all of that hangs too.
                let max_width_in_lpxs = width_in_lpxs + remaining_width_in_lpxs;
                let has_overflowing_content = self.glyphs.iter().any(|glyph| {
                    glyph.advance_in_lpxs() > 0.0
                        && glyph.origin_in_lpxs.x + glyph.advance_in_lpxs() > max_width_in_lpxs
                        && !text[glyph.cluster..].starts_with(is_hangable_white_space)
                });
                if has_overflowing_content {
                    hanging_width_in_lpxs
                } else {
                    hanging_width_in_lpxs.max(-remaining_width_in_lpxs)
                }
            }
        };

        let glyphs = mem::take(&mut self.glyphs);
        let mut row = LaidoutRow {
            origin_in_lpxs: Point::ZERO,
            text,
            newline: matches!(row_end, RowEnd::ForcedBreak { .. }),
            width_in_lpxs,
            hanging_width_in_lpxs,
            ascender_in_lpxs,
            descender_in_lpxs,
            line_gap_in_lpxs,
            cap_height_in_lpxs,
            line_spacing_scale: self.options.line_spacing_scale,
            flow_wrap_spacing_in_lpxs: self.options.flow_wrap_spacing_in_lpxs,
            glyphs,
        };

        self.current_point_in_lpxs.x = 0.0;
        self.current_point_in_lpxs.y += self.rows.last().map_or(row.ascender_in_lpxs, |prev_row| {
            let natural_in_lpxs = prev_row.line_spacing_in_lpxs(&row);
            if self.rows.len() == 1 {
                // The first row can share its visual row with earlier inline
                // content that is taller than the text; the caller passes that
                // row's real height so the second row's top edge clears it.
                // Under the flow rule the wrap gap is ours to add. The floor is
                // a top-to-top distance, so it converts to a baseline advance
                // by adding the change in ascender between the two rows.
                let min_advance_in_lpxs = self.options.first_row_min_line_spacing_below_in_lpxs
                    + prev_row.wrap_spacing_below_in_lpxs().unwrap_or(0.0)
                    + row.ascender_in_lpxs
                    - prev_row.ascender_in_lpxs;
                natural_in_lpxs.max(min_advance_in_lpxs)
            } else {
                natural_in_lpxs
            }
        });
        let max_width_in_lpxs = self
            .options
            .max_width_in_lpxs
            .unwrap_or(row.width_without_hanging_in_lpxs());
        let remaining_width_in_lpxs = max_width_in_lpxs - row.width_without_hanging_in_lpxs();
        row.origin_in_lpxs.x = self.options.align * remaining_width_in_lpxs;
        row.origin_in_lpxs.y = self.current_point_in_lpxs.y;
        self.current_row_start = self.current_row_end;
        if let RowEnd::ForcedBreak { newline_len } = row_end {
            self.current_row_start += newline_len;
            self.current_row_end += newline_len;
        }
        self.rows.push(row);
    }

    fn finish_with(self, is_truncated: bool) -> LaidoutText {
        let last_row = self.rows.last().unwrap();
        let mut line_break_before_next_run = self.options.line_break_before;
        line_break_before_next_run.append(&self.text);
        // An empty text passes its reservation along.
        let is_trailing_glue_reserved = self.is_glued_to_next_run
            && (self.is_trailing_glue_reserved
                || (self.text.is_empty() && self.options.is_start_reserved));
        LaidoutText {
            text: self.text,
            size_in_lpxs: Size::new(
                self.rows
                    .iter()
                    .map(|row| row.width_without_hanging_in_lpxs())
                    .reduce(f32::max)
                    .unwrap_or(0.0),
                last_row.origin_in_lpxs.y - last_row.descender_in_lpxs,
            ),
            has_missing_glyph: self.rows.iter().flat_map(|row| &row.glyphs).any(|glyph| glyph.id == 0),
            rows: self.rows,
            is_truncated,
            line_break_before_next_run,
            is_trailing_glue_reserved,
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
        // Only the text's final hard line break left over still counts as consumed.
        let rest = &self.text[self.current_row_end..];
        let all_text_consumed = find_first_line_break(
            rest,
            PrecedingContext::PARAGRAPH_START,
            FollowingContext::END_OF_TEXT,
        )
        .is_none_or(|line_break| line_break.newline_len == rest.len());

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

        let mut text_was_truncated = self.rows.len() > max_rows || !all_text_consumed;

        self.rows.truncate(max_rows);

        // A non-wrapping layout puts every glyph on a single row, so its
        // overflow shows up as a row wider than the bound rather than as
        // surplus rows. Row counting alone therefore reports "nothing was
        // truncated" for the very case the ellipsis exists to handle.
        // Restricted to non-wrapping layouts because a wrapped row's width
        // includes any first-row indent and may legitimately reach the bound.
        if self.options.ellipsis && !self.options.wrap {
            if let Some(max_width) = self.options.max_width_in_lpxs {
                if self.rows.last().is_some_and(|row| row.width_in_lpxs > max_width) {
                    text_was_truncated = true;
                }
            }
        }

        // What follows us gets cut when the last row we're allowed has no room for it.
        let options = &self.options;
        text_was_truncated |= options.ellipsis
            && is_short_of_following_room(&self.rows, max_rows, options.max_width_in_lpxs, options.following_room_in_lpxs);

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
        last_row.hanging_width_in_lpxs = 0.0;

        // Append ellipsis glyphs
        for glyph in &ellipsis_shaped.glyphs {
            let ellipsis_glyph = LaidoutGlyph {
                origin_in_lpxs: Point::new(last_row.width_in_lpxs, 0.0),
                font: glyph.font.clone(),
                font_size_in_lpxs,
                color: self.style.color,
                id: glyph.id,
                cluster: last_row.text.len(), // beyond the text range
                direction: glyph.direction,
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
            self.finish_current_row(RowEnd::TextEnd);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RowEnd {
    /// A break opportunity, because the next unit doesn't fit.
    SoftWrap,
    /// A grapheme boundary inside a unit that's wider than a row.
    EmergencyWrap,
    ForcedBreak { newline_len: usize },
    TextEnd,
}

#[derive(Debug)]
struct Fitter {
    text: Substr,
    shaped_line: ShapedLine,
    font_family: Rc<FontFamily>,
    font_size_in_lpxs: f32,
    segments: Vec<Segment>,
    next_segment_index: usize,
}

impl Fitter {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn next_segment(&self) -> Segment {
        self.segments[self.next_segment_index]
    }

    /// Takes as many of the next segments as fit on a row, and returns them shaped.
    fn fit(&mut self, wrap_width_in_lpxs: f32) -> Option<Rc<ShapedText>> {
        // We add up each segment's own width, since shaping longer candidates would always miss
        // the shaper's cache. The sum leaves out the last one's hyphen and glue, so it only grows.
        let segments = &self.segments[self.next_segment_index..];
        let mut estimated_width_in_lpxs = 0.0;
        let mut count = segments
            .iter()
            .take_while(|segment| {
                let fits = estimated_width_in_lpxs + segment.width_without_hanging_in_lpxs
                    <= wrap_width_in_lpxs;
                estimated_width_in_lpxs += segment.width_in_lpxs;
                fits
            })
            .count();
        // The estimate misses the kerning between segments, so we shape the row to check it.
        let mut len: usize = segments[..count].iter().map(|segment| segment.len).sum();
        while count > 0 {
            let last_segment = segments[count - 1];
            let text = self
                .shaped_line
                .get_or_shape_row(&self.font_family, self.text.substr(0..len));
            let width_in_lpxs = get_width_in_ems_before(&text, len - last_segment.hanging_len)
                * self.font_size_in_lpxs
                + last_segment.hyphen_width_in_lpxs
                + last_segment.glue_width_in_lpxs;
            if width_in_lpxs <= wrap_width_in_lpxs {
                self.next_segment_index += count;
                self.text = self.text.substr(len..);
                return Some(text);
            }
            len -= last_segment.len;
            count -= 1;
        }
        None
    }

    fn pop(&mut self) -> Segment {
        let segment = self.next_segment();
        self.next_segment_index += 1;
        self.text = self.text.substr(segment.len..);
        segment
    }
}

/// A line's text, which we shape in stretches of consecutive units that aren't cached yet.
///
/// The units and rows inside a stretch then take their glyphs from it where that's exact, instead
/// of each of them getting shaped too.
#[derive(Debug)]
struct ShapedLine {
    text: Substr,
    shaped_stretches: Vec<Rc<ShapedText>>,
    // Where the last stretch we found ends, even if we didn't shape it whole.
    stretch_end: usize,
}

impl ShapedLine {
    fn new(text: Substr) -> Self {
        Self {
            text,
            shaped_stretches: Vec::new(),
            stretch_end: 0,
        }
    }

    /// Returns a unit of the line shaped.
    ///
    /// A unit that isn't cached starts a stretch, which goes on through the units after it (ending
    /// at `next_unit_ends`) until one is cached.
    fn get_or_shape_unit(
        &mut self,
        font_family: &FontFamily,
        unit: Substr,
        next_unit_ends: impl Iterator<Item = usize>,
    ) -> Rc<ShapedText> {
        font_family.get_or_shape_with(unit, |shaper, params| {
            let unit_start = params.text.start_in_parent() - self.text.start_in_parent();
            if unit_start >= self.stretch_end {
                let unit_end = unit_start + params.text.len();
                let mut stretch_params = params.clone();
                self.stretch_end = unit_end;
                for next_unit_end in next_unit_ends {
                    stretch_params.text = self.text.substr(self.stretch_end..next_unit_end);
                    if shaper.is_cached(&stretch_params) {
                        break;
                    }
                    self.stretch_end = next_unit_end;
                }
                if self.stretch_end > unit_end {
                    stretch_params.text = self.text.substr(unit_start..self.stretch_end);
                    if let Some(stretch) = shaper.shape_with_pieces(stretch_params) {
                        self.shaped_stretches.push(Rc::new(stretch));
                    }
                }
            }
            self.get_piece(&params.text)
        })
    }

    /// Returns a row of the line shaped, from the stretch it's in if it isn't cached.
    fn get_or_shape_row(&self, font_family: &FontFamily, row: Substr) -> Rc<ShapedText> {
        font_family.get_or_shape_with(row, |_, params| self.get_piece(&params.text))
    }

    /// Returns `piece` shaped on its own, from the stretch we shaped whole that it's in.
    fn get_piece(&self, piece: &Substr) -> Option<Rc<ShapedText>> {
        let start = piece.start_in_parent();
        let index = self
            .shaped_stretches
            .partition_point(|stretch| stretch.text.start_in_parent() <= start);
        let stretch = self.shaped_stretches[..index].last()?;
        if piece.end_in_parent() > stretch.text.end_in_parent() {
            return None;
        }
        if piece.len() == stretch.text.len() {
            return Some(stretch.clone());
        }
        stretch
            .get_piece(piece, start - stretch.text.start_in_parent())
            .map(Rc::new)
    }
}

/// A piece of text that the fitter puts on a row whole, like a unit or a grapheme.
#[derive(Clone, Copy, Debug)]
struct Segment {
    len: usize,
    // How many bytes of white space at the end hang when a row ends with this segment.
    hanging_len: usize,
    width_in_lpxs: f32,
    width_without_hanging_in_lpxs: f32,
    // The hyphen a row gets when it ends with this segment right after a soft hyphen.
    hyphen_width_in_lpxs: f32,
    // What has to stay on the row right after this segment, which only the text's last unit has.
    glue_width_in_lpxs: f32,
}

impl Segment {
    fn new(shaped_text: &ShapedText, font_size_in_lpxs: f32, hanging_len: usize) -> Self {
        let len = shaped_text.text.len();
        Self {
            len,
            hanging_len,
            width_in_lpxs: shaped_text.width_in_ems * font_size_in_lpxs,
            width_without_hanging_in_lpxs: get_width_in_ems_before(shaped_text, len - hanging_len)
                * font_size_in_lpxs,
            hyphen_width_in_lpxs: 0.0,
            glue_width_in_lpxs: 0.0,
        }
    }
}

/// Returns the advance width of the glyphs for the bytes of `text` before `byte_index`.
fn get_width_in_ems_before(text: &ShapedText, byte_index: usize) -> f32 {
    text.glyphs
        .iter()
        .filter(|glyph| glyph.cluster < byte_index)
        .map(|glyph| glyph.advance_in_ems)
        .sum()
}

/// Returns whether `c` is white space that hangs at the end of a row.
///
/// That's CSS Text's preserved spaces, tabs and other space separators, so `Zs` except U+00A0.
pub fn is_hangable_white_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Whether `rows` end on the last row `max_rows` allows, and it has less room than `following_room_in_lpxs`
/// after it, so it ends with the ellipsis. See [`LayoutOptions::following_room_in_lpxs`].
pub fn is_short_of_following_room(
    rows: &[LaidoutRow],
    max_rows: usize,
    max_width_in_lpxs: Option<f32>,
    following_room_in_lpxs: f32,
) -> bool {
    following_room_in_lpxs > 0.0
        && rows.len() == max_rows
        && rows.last().is_some_and(|row| row.width_in_lpxs + following_room_in_lpxs > max_width_in_lpxs.unwrap_or(f32::MAX))
}

/// Returns the hyphen we draw at the end of a row that wraps right after a soft hyphen.
///
/// Like browsers, we use U+2010 HYPHEN when the family's first font has it.
pub(crate) fn get_hyphen(font_family: &FontFamily) -> &'static str {
    let has_hyphen_glyph = font_family.fonts().first().is_some_and(|font| {
        font.with_ttf_parser_face(|face| face.glyph_index('\u{2010}').is_some())
    });
    if has_hyphen_glyph {
        "\u{2010}"
    } else {
        "-"
    }
}

pub trait LayoutParams {
    fn to_owned(self) -> OwnedLayoutParams;
    fn text(&self) -> &str;
    fn style(&self) -> &Style;
    fn options(&self) -> &LayoutOptions;
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

    fn style(&self) -> &Style {
        &self.style
    }

    fn options(&self) -> &LayoutOptions {
        &self.options
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

    fn style(&self) -> &Style {
        &self.style
    }

    fn options(&self) -> &LayoutOptions {
        &self.options
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
    /// Minimum distance in logical pixels from the first row's top edge to the
    /// second row's top edge. A continuation run's first row can share its
    /// visual row with earlier inline content that is taller than the text;
    /// callers pass that row's real height (plus wrap spacing, unless
    /// `flow_wrap_spacing_in_lpxs` is set and the layouter adds it) so the
    /// second row clears it. Zero keeps pure font-metric spacing. Only the
    /// first row boundary is affected; later rows keep their natural spacing.
    pub first_row_min_line_spacing_below_in_lpxs: f32,
    pub max_width_in_lpxs: Option<f32>,
    pub wrap: bool,
    pub align: f32,
    pub line_spacing_scale: f32,
    /// The enclosing wrap flow's current wrap spacing, when the text draws as part of one. Rows
    /// then advance like the flow's own rows: box height plus the larger of this and the row's
    /// `ascender * (line_spacing_scale - 1)`, line gap ignored. `None` keeps font-metric spacing.
    pub flow_wrap_spacing_in_lpxs: Option<f32>,
    /// Maximum number of rows to display. `None` means unlimited.
    /// When set and the text exceeds this many rows, excess rows are discarded.
    pub max_rows: Option<usize>,
    /// When true and text is truncated (by `max_rows` or by exceeding `max_width_in_lpxs`
    /// on a single non-wrapping line), an ellipsis character (U+2026 "…") is appended
    /// to the last visible row.
    pub ellipsis: bool,
    /// What the line breaking rules see before the text, like the earlier runs on its line.
    pub line_break_before: PrecedingContext,
    /// What the line breaking rules see after the text, like the start of the next run.
    pub line_break_after: FollowingContext,
    /// The width of what has to stay on the row right after the text.
    ///
    /// That's the next run's text up to its first break opportunity, when the text's end isn't one.
    pub trailing_glue_width_in_lpxs: f32,
    /// Whether the previous run kept room on its last row for this text's first unit.
    ///
    /// That unit then stays on our first row even if it doesn't quite fit.
    pub is_start_reserved: bool,
    /// The room that what follows the text needs on the text's last row, or 0 when it needs none.
    ///
    /// When the last row `max_rows` allows doesn't have that room, it ends with the ellipsis.
    pub following_room_in_lpxs: f32,
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
            flow_wrap_spacing_in_lpxs: None,
            max_rows: None,
            ellipsis: false,
            line_break_before: PrecedingContext::PARAGRAPH_START,
            line_break_after: FollowingContext::END_OF_TEXT,
            trailing_glue_width_in_lpxs: 0.0,
            is_start_reserved: false,
            following_room_in_lpxs: 0.0,
        }
    }
}

impl Eq for LayoutOptions {}

impl Hash for LayoutOptions {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        // Keys with the same text mostly differ in where it starts or how wide it gets, so we
        // leave the other options to `eq` instead of hashing them on every lookup.
        self.first_row_indent_in_lpxs.to_bits().hash(hasher);
        self.max_width_in_lpxs.map(f32::to_bits).hash(hasher);
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
            && self.flow_wrap_spacing_in_lpxs.map(f32::to_bits)
                == other.flow_wrap_spacing_in_lpxs.map(f32::to_bits)
            && self.max_rows == other.max_rows
            && self.ellipsis == other.ellipsis
            && self.line_break_before == other.line_break_before
            && self.line_break_after == other.line_break_after
            && self.trailing_glue_width_in_lpxs.to_bits()
                == other.trailing_glue_width_in_lpxs.to_bits()
            && self.is_start_reserved == other.is_start_reserved
            && self.following_room_in_lpxs.to_bits() == other.following_room_in_lpxs.to_bits()
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutText {
    pub text: Substr,
    pub size_in_lpxs: Size<f32>,
    pub rows: Vec<LaidoutRow>,
    /// Whether some glyph is missing from all of the family's fonts.
    pub has_missing_glyph: bool,
    /// True when the text was truncated (e.g., due to `max_rows` or ellipsis).
    pub is_truncated: bool,
    /// The `line_break_before` for the next run on the same line, so ours with this text appended.
    pub line_break_before_next_run: PrecedingContext,
    /// Whether the trailing glue fit right after the text's last unit.
    ///
    /// The next run on the same row gets this as its `is_start_reserved`.
    pub is_trailing_glue_reserved: bool,
}

impl LaidoutText {
    /// How far down the text has to move for its *ink* to sit in the middle of
    /// the box `size_in_lpxs` describes, instead of its line box.
    ///
    /// The box this text occupies runs from the first row's ascender down to
    /// the last row's descender. The ink runs from the first row's cap line
    /// down to the last row's baseline. A text face's ascender reaches further
    /// above the cap line than its descender reaches below the baseline, so
    /// the two centers do not coincide and the ink reads as sitting high. The
    /// difference works out to `(descender + cap_height - ascender) / 2`
    /// whatever the row count, because both boxes share everything in between.
    ///
    /// Returns `0.0` when the metrics are not trustworthy enough to move
    /// anything: a face with no capital (icon and symbol fonts), or one whose
    /// cap height claims to reach past its own ascender.
    pub fn ink_center_offset_in_lpxs(&self) -> f32 {
        let (Some(first), Some(last)) = (self.rows.first(), self.rows.last()) else {
            return 0.0;
        };
        let ascender = first.ascender_in_lpxs;
        let cap_height = first.cap_height_in_lpxs;
        let descender = -last.descender_in_lpxs;
        if cap_height <= 0.0 || cap_height > ascender {
            return 0.0;
        }
        (descender + cap_height - ascender) * 0.5
    }

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
            // An index inside a hard line break, like between its CR and LF, stays on its row.
            if row.newline
                && self
                    .rows
                    .get(row_index + 1)
                    .is_some_and(|next_row| cursor.index < next_row.text.start_in_parent())
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
        for row_index in start_row_index..=end_row_index {
            let row = &self.rows[row_index];
            let mut push_span = |x_in_lpxs: f32, width_in_lpxs: f32| {
                selection_rects.push(SelectionRect {
                    rect_in_lpxs: Rect::new(
                        Point::new(x_in_lpxs, row.origin_in_lpxs.y - row.ascender_in_lpxs),
                        Size::new(width_in_lpxs, row.ascender_in_lpxs - row.descender_in_lpxs),
                    ),
                    ascender_in_lpxs: row.ascender_in_lpxs,
                });
            };
            let is_start_row = row_index == start_row_index;
            let is_end_row = row_index == end_row_index;
            if !is_start_row && !is_end_row {
                push_span(row.origin_in_lpxs.x, row.width_in_lpxs);
                continue;
            }
            let row_start = row.text.start_in_parent();
            let (start, start_x_in_lpxs) = if is_start_row {
                (selection.start().index - row_start, start_x_in_lpxs)
            } else {
                (0, 0.0)
            };
            let (end, end_x_in_lpxs) = if is_end_row {
                (selection.end().index - row_start, end_x_in_lpxs)
            } else {
                (usize::MAX, row.width_in_lpxs)
            };
            if row.is_left_to_right() {
                push_span(start_x_in_lpxs, end_x_in_lpxs - start_x_in_lpxs);
                continue;
            }
            // RTL text, or text the row doesn't draw, can split what's selected on it into several
            // spans.
            let graphemes = row.get_visual_graphemes();
            let mut has_spans = false;
            let mut span_start_x_in_lpxs = None;
            for stop in 0..=graphemes.len() {
                let grapheme = graphemes.get(stop);
                let x_in_lpxs = grapheme.map_or(row.width_in_lpxs, |grapheme| grapheme.x_in_lpxs);
                if grapheme.is_some_and(|grapheme| start <= grapheme.start && grapheme.end <= end) {
                    span_start_x_in_lpxs.get_or_insert(x_in_lpxs);
                } else if let Some(span_start_x_in_lpxs) = span_start_x_in_lpxs.take() {
                    push_span(span_start_x_in_lpxs, x_in_lpxs - span_start_x_in_lpxs);
                    has_spans = true;
                }
            }
            if !has_spans {
                // A row with nothing selected still gets an empty span at the caret.
                push_span(if is_start_row { start_x_in_lpxs } else { end_x_in_lpxs }, 0.0);
            }
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
    /// The width of the white space hanging at the end of the row.
    ///
    /// It's part of `width_in_lpxs`, but doesn't count for fit, alignment or the text's size.
    pub hanging_width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub line_gap_in_lpxs: f32,
    /// Height of a capital above this row's baseline, or `0.0` when the row's
    /// font has no capital to measure (see [`Font::cap_height_in_ems`]).
    pub cap_height_in_lpxs: f32,
    pub line_spacing_scale: f32,
    /// See [`LayoutOptions::flow_wrap_spacing_in_lpxs`].
    pub flow_wrap_spacing_in_lpxs: Option<f32>,
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    /// The row's width without the white space hanging at its end, so what it takes up.
    pub fn width_without_hanging_in_lpxs(&self) -> f32 {
        self.width_in_lpxs - self.hanging_width_in_lpxs
    }

    /// The wrap gap below this row under the flow rule: the flow's spacing or this row's own,
    /// whichever is larger, as the flow itself would use. `None` under font-metric spacing.
    pub fn wrap_spacing_below_in_lpxs(&self) -> Option<f32> {
        self.flow_wrap_spacing_in_lpxs
            .map(|spacing| spacing.max(self.ascender_in_lpxs * (self.line_spacing_scale - 1.0)))
    }

    pub fn line_spacing_in_lpxs(&self, next_row: &LaidoutRow) -> f32 {
        match self.wrap_spacing_below_in_lpxs() {
            // The wrap gap plus this row's box height down to the next row's top, then its ascender.
            Some(wrap_spacing) => {
                wrap_spacing - self.descender_in_lpxs + next_row.ascender_in_lpxs
            }
            None => {
                (self.line_gap_in_lpxs - self.descender_in_lpxs + next_row.ascender_in_lpxs)
                    * next_row.line_spacing_scale
            }
        }
    }

    pub fn x_in_lpxs_to_index(&self, x_in_lpxs: f32) -> usize {
        let is_before_middle = |grapheme: &VisualGrapheme| {
            x_in_lpxs < grapheme.x_in_lpxs + 0.5 * grapheme.width_in_lpxs
        };
        if self.is_left_to_right() {
            return self
                .find_visual_grapheme(None, |grapheme| {
                    is_before_middle(&grapheme).then_some(grapheme.start)
                })
                .unwrap_or(self.text.len());
        }
        let graphemes = self.get_visual_graphemes();
        let caret_indices = self.get_caret_indices(&graphemes);
        let stop = graphemes.iter().position(is_before_middle).unwrap_or(graphemes.len());
        caret_indices[stop]
    }

    pub fn index_to_x_in_lpxs(&self, index: usize) -> f32 {
        if self.is_left_to_right() {
            return self
                .find_visual_grapheme(None, |grapheme| {
                    (grapheme.start == index).then_some(grapheme.x_in_lpxs)
                })
                .unwrap_or(self.width_in_lpxs);
        }
        let graphemes = self.get_visual_graphemes();
        let caret_indices = self.get_caret_indices(&graphemes);
        caret_indices
            .iter()
            .position(|&caret_index| caret_index == index)
            .and_then(|stop| graphemes.get(stop))
            .map_or(self.width_in_lpxs, |grapheme| grapheme.x_in_lpxs)
    }

    /// Returns whether the row draws its text left to right from its first char, if it draws any.
    ///
    /// Each caret is then at the left edge of the grapheme after it, so they go up with the index.
    fn is_left_to_right(&self) -> bool {
        let mut previous_cluster = 0;
        // The ellipsis has no text of its own, so a row of only that draws none.
        self.glyphs
            .first()
            .is_none_or(|glyph| glyph.cluster == 0 || glyph.cluster == self.text.len())
            && self.glyphs.iter().all(|glyph| {
                let is_in_order =
                    glyph.direction == Direction::Ltr && glyph.cluster >= previous_cluster;
                previous_cluster = glyph.cluster;
                is_in_order
            })
    }

    /// Returns the row's graphemes from left to right.
    fn get_visual_graphemes(&self) -> Vec<VisualGrapheme> {
        // Clusters go down from left to right in RTL text.
        let mut clusters: Vec<usize> = self.glyphs.iter().map(|glyph| glyph.cluster).collect();
        clusters.sort_unstable();
        let mut graphemes = Vec::with_capacity(self.glyphs.len());
        self.find_visual_grapheme(Some(&clusters), |grapheme| {
            graphemes.push(grapheme);
            None::<()>
        });
        graphemes
    }

    /// Returns the first thing `f` finds in the row's graphemes, going from left to right.
    ///
    /// A cluster's text ends at the next larger cluster, which we find in `sorted_clusters` unless
    /// the row is left to right. A ligature's width is split evenly between its graphemes.
    fn find_visual_grapheme<T>(
        &self,
        sorted_clusters: Option<&[usize]>,
        mut f: impl FnMut(VisualGrapheme) -> Option<T>,
    ) -> Option<T> {
        use {super::slice::SliceExt, unicode_segmentation::UnicodeSegmentation};

        let mut rtl_graphemes = Vec::new();
        let mut glyph_groups = self
            .glyphs
            .group_by(|glyph_0, glyph_1| glyph_0.cluster == glyph_1.cluster)
            .peekable();
        while let Some(glyph_group) = glyph_groups.next() {
            let start = glyph_group[0].cluster;
            let start_x_in_lpxs = glyph_group[0].origin_in_lpxs.x;
            let next_glyph_group = glyph_groups.peek();
            let end_x_in_lpxs = next_glyph_group.map_or(self.width_in_lpxs, |next_glyph_group| {
                next_glyph_group[0].origin_in_lpxs.x
            });
            let end = match sorted_clusters {
                Some(clusters) => clusters
                    .get(clusters.partition_point(|&cluster| cluster <= start))
                    .copied(),
                None => next_glyph_group.map(|next_glyph_group| next_glyph_group[0].cluster),
            };
            let text = &self.text[start..end.unwrap_or(self.text.len())];
            // The ellipsis has no text of its own.
            if text.is_empty() {
                continue;
            }
            let is_rtl = glyph_group[0].direction == Direction::Rtl;
            // A cluster of one char is one grapheme, so we only segment longer ones.
            let grapheme_count = if text.chars().nth(1).is_none() {
                1
            } else {
                text.graphemes(true).count()
            };
            let grapheme_width_in_lpxs = (end_x_in_lpxs - start_x_in_lpxs) / grapheme_count as f32;
            let mut x_in_lpxs = start_x_in_lpxs;
            let mut visit = |grapheme_start: usize, grapheme: &str| {
                let found = f(VisualGrapheme {
                    start: start + grapheme_start,
                    end: start + grapheme_start + grapheme.len(),
                    x_in_lpxs,
                    width_in_lpxs: grapheme_width_in_lpxs,
                    is_rtl,
                });
                x_in_lpxs += grapheme_width_in_lpxs;
                found
            };
            let found = if grapheme_count == 1 {
                visit(0, text)
            } else if is_rtl {
                rtl_graphemes.clear();
                rtl_graphemes.extend(text.grapheme_indices(true));
                rtl_graphemes.iter().rev().find_map(|&(index, grapheme)| visit(index, grapheme))
            } else {
                text.grapheme_indices(true).find_map(|(index, grapheme)| visit(index, grapheme))
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Returns the text index of the caret at each grapheme's left edge, then at the row's end.
    ///
    /// A caret goes at the leading edge of the grapheme after it or the trailing edge of the one
    /// before it, which differ where the direction changes, so we give each caret its own edge.
    fn get_caret_indices(&self, graphemes: &[VisualGrapheme]) -> Vec<usize> {
        // Without RTL text, each caret is at the left edge of the grapheme after it.
        if !graphemes.iter().any(|grapheme| grapheme.is_rtl) {
            let starts = graphemes.iter().map(|grapheme| grapheme.start);
            return starts.chain([self.text.len()]).collect();
        }
        let count = graphemes.len();
        // The graphemes in text order, and where each one is in that order.
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|&grapheme_index| graphemes[grapheme_index].start);
        let mut rank = vec![0; count];
        for (position, &grapheme_index) in order.iter().enumerate() {
            rank[grapheme_index] = position;
        }
        // Boundary `p` is before the `p`th grapheme in text order, and stop `k` is at the left
        // edge of the `k`th grapheme from the left. After LTR text, we prefer its trailing edge.
        let get_stops = |boundary: usize| {
            let leading = order
                .get(boundary)
                .map(|&k| if graphemes[k].is_rtl { k + 1 } else { k });
            let trailing = boundary.checked_sub(1).map(|p| {
                let k = order[p];
                if graphemes[k].is_rtl { k } else { k + 1 }
            });
            if boundary > 0 && !graphemes[order[boundary - 1]].is_rtl {
                [trailing, leading]
            } else {
                [leading, trailing]
            }
        };
        let get_boundaries = |stop: usize| {
            [
                stop.checked_sub(1)
                    .map(|k| if graphemes[k].is_rtl { rank[k] } else { rank[k] + 1 }),
                graphemes
                    .get(stop)
                    .map(|grapheme| if grapheme.is_rtl { rank[stop] + 1 } else { rank[stop] }),
            ]
        };
        let mut stop_of_boundary = vec![None; count + 1];
        let mut boundary_at_stop = vec![None; count + 1];
        // The text's ends have just one edge each, so they go first.
        for first_boundary in [0, count].into_iter().chain(1..count) {
            let mut next_boundary = stop_of_boundary[first_boundary]
                .is_none()
                .then_some(first_boundary);
            while let Some(boundary) = next_boundary {
                let Some(stop) = get_stops(boundary)
                    .into_iter()
                    .flatten()
                    .find(|&stop| boundary_at_stop[stop].is_none())
                else {
                    break;
                };
                stop_of_boundary[boundary] = Some(stop);
                boundary_at_stop[stop] = Some(boundary);
                // The other boundary that could go at this stop has to go at its other one.
                next_boundary = get_boundaries(stop)
                    .into_iter()
                    .flatten()
                    .find(|&other_boundary| stop_of_boundary[other_boundary].is_none());
            }
        }
        // RTL text drawn in pieces can leave a boundary without an edge of its own, so it gets a
        // stop that's left.
        let free_stops: Vec<usize> = (0..=count)
            .filter(|&stop| boundary_at_stop[stop].is_none())
            .collect();
        let free_boundaries =
            (0..=count).filter(|&boundary| stop_of_boundary[boundary].is_none());
        for (stop, boundary) in free_stops.into_iter().zip(free_boundaries) {
            boundary_at_stop[stop] = Some(boundary);
        }
        boundary_at_stop
            .into_iter()
            .map(|boundary| {
                order
                    .get(boundary.unwrap())
                    .map_or(self.text.len(), |&k| graphemes[k].start)
            })
            .collect()
    }
}

/// A grapheme of a row, with where we draw it.
#[derive(Clone, Copy, Debug)]
struct VisualGrapheme {
    start: usize,
    end: usize,
    x_in_lpxs: f32,
    width_in_lpxs: f32,
    is_rtl: bool,
}

#[derive(Clone, Debug)]
pub struct LaidoutGlyph {
    pub origin_in_lpxs: Point<f32>,
    pub font: Rc<Font>,
    pub font_size_in_lpxs: f32,
    pub color: Option<Color>,
    pub id: GlyphId,
    pub cluster: usize,
    pub direction: Direction,
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
        parse_text_atlas_size_value, LaidoutText, LayoutOptions, Layouter, OwnedLayoutParams,
        Settings, Size, Style, LAYOUT_CACHE_MAX_BYTES,
    };
    use std::rc::Rc;

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
            has_missing_glyph: false,
            is_truncated: false,
            line_break_before_next_run: Default::default(),
            is_trailing_glue_reserved: false,
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

    // -----------------------------------------------------------------------
    // Multiline ellipsis: `max_rows: Some(n)` + `ellipsis: true` + `wrap` must
    // wrap normally up to row n, fill row n to the width limit, and end it in
    // "…" — the contract a two-line card caption stands on. These tests load
    // the bundled fonts (as loader::tests does) because truncation decisions
    // depend on real glyph advances.
    // -----------------------------------------------------------------------

    use super::BorrowedLayoutParams;
    use crate::makepad_platform::SharedBytes;
    use crate::text::font::FontId;
    use crate::text::{font_family::FontDiagnostics, loader::{FontDefinition, FontFamilyDefinition}};
    use std::path::PathBuf;

    const LATIN_FAMILY: u64 = 0xE111_00FA;
    const CJK_FAMILY: u64 = 0xE111_00FB;

    #[test]
    fn async_font_arrival_invalidates_cached_layout() {
        const FAMILY: u64 = 0xE111_00FC;
        let primary_id: FontId = 0xE111_0003_u64.into();
        let fallback_id: FontId = 0xE111_0004_u64.into();
        let resources =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources");
        let mut layouter = Layouter::new(Settings::default());
        let primary = SharedBytes::from_file_mmap_or_read(resources.join("IBMPlexSans-Text.ttf"))
            .expect("primary font bytes should load");
        layouter.define_font(
            primary_id,
            FontDefinition {
                data: primary,
                index: 0,
                ascender_fudge_in_ems: 0.0,
                descender_fudge_in_ems: 0.0,
                weight: None,
                variations: Vec::new(),
            },
        );
        let diagnostics = FontDiagnostics {
            role: "regular".to_string(),
            set: "Latin".to_string(),
            tried: vec!["ibm_plex_text".to_string(), "jetbrains_ui_symbols".to_string()],
        };
        layouter.define_font_family(
            FAMILY.into(),
            FontFamilyDefinition {
                font_ids: vec![primary_id],
                expected_member_count: 2,
                diagnostics: diagnostics.clone(),
            },
        );
        let params = OwnedLayoutParams {
            text: "⌘".into(),
            style: Style {
                font_family_id: FAMILY.into(),
                font_size_in_pts: 12.0,
                color: None,
            },
            options: LayoutOptions::default(),
        };
        let before = layouter.get_or_layout(params.clone());
        assert!(before.rows.iter().flat_map(|row| &row.glyphs).any(|glyph| glyph.id == 0));

        let fallback = SharedBytes::from_file_mmap_or_read(
            resources.join("jetbrains_mono_variable.ttf"),
        )
        .expect("fallback font bytes should arrive");
        layouter.define_font(
            fallback_id,
            FontDefinition {
                data: fallback,
                index: 0,
                ascender_fudge_in_ems: 0.0,
                descender_fudge_in_ems: 0.0,
                weight: None,
                variations: Vec::new(),
            },
        );
        layouter.set_font_family_definition(
            FAMILY.into(),
            FontFamilyDefinition {
                font_ids: vec![primary_id, fallback_id],
                expected_member_count: 2,
                diagnostics,
            },
        );

        let after = layouter.get_or_layout(params);
        assert!(!Rc::ptr_eq(&before, &after), "arrival must evict the old layout");
        assert!(after
            .rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .all(|glyph| glyph.id != 0 && glyph.font.id() == fallback_id));
    }

    #[test]
    fn font_family_generation_goes_up_when_a_family_changes() {
        let mut layouter = real_font_layouter();
        let (latin_font, cjk_font): (FontId, FontId) = (0xE111_0001_u64.into(), 0xE111_0002_u64.into());
        let get_definition = |font_ids: Vec<FontId>| FontFamilyDefinition {
            expected_member_count: font_ids.len(),
            font_ids,
            diagnostics: Default::default(),
        };
        let generation = layouter.font_family_generation();
        layouter.set_font_family_definition(LATIN_FAMILY.into(), get_definition(vec![latin_font]));
        assert_eq!(layouter.font_family_generation(), generation);
        // A fallback font arrives.
        layouter.set_font_family_definition(LATIN_FAMILY.into(), get_definition(vec![latin_font, cjk_font]));
        assert_eq!(layouter.font_family_generation(), generation + 1);
        layout_text(&mut layouter, LATIN_FAMILY, "a\u{4E2D}", LayoutOptions::default());
        layouter.set_font_family_definition(LATIN_FAMILY.into(), get_definition(vec![latin_font, cjk_font]));
        assert_eq!(layouter.font_family_generation(), generation + 1);
    }

    #[test]
    fn layouts_stay_cached_while_a_font_family_waits_for_a_font() {
        let mut layouter = real_font_layouter();
        let (latin_font, cjk_font): (FontId, FontId) = (0xE111_0001_u64.into(), 0xE111_0002_u64.into());
        let get_definition = |font_ids: Vec<FontId>| FontFamilyDefinition {
            font_ids,
            expected_member_count: 2,
            diagnostics: Default::default(),
        };
        // Until its fallback font arrives, every layout through the family sets it again.
        let options = LayoutOptions::default();
        layouter.set_font_family_definition(LATIN_FAMILY.into(), get_definition(vec![latin_font]));
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, "a\u{4E2D}", options);
        layouter.set_font_family_definition(LATIN_FAMILY.into(), get_definition(vec![latin_font]));
        let cached = layout_text(&mut layouter, LATIN_FAMILY, "a\u{4E2D}", options);
        assert!(Rc::ptr_eq(&laidout, &cached));
        let definition = get_definition(vec![latin_font, cjk_font]);
        layouter.set_font_family_definition(LATIN_FAMILY.into(), definition);
        let arrived = layout_text(&mut layouter, LATIN_FAMILY, "a\u{4E2D}", options);
        assert_eq!(arrived.rows[0].glyphs[1].font.id(), cjk_font);
    }

    fn real_font_layouter() -> Layouter {
        let mut layouter = Layouter::new(Settings::default());
        let resources =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources");
        for (family_id, font_id, file) in [
            (LATIN_FAMILY, 0xE111_0001_u64, "IBMPlexSans-Text.ttf"),
            (CJK_FAMILY, 0xE111_0002_u64, "LXGWWenKaiRegular.ttf"),
        ] {
            let data = SharedBytes::from_file_mmap_or_read(resources.join(file))
                .expect("bundled font bytes should load");
            let font_id: FontId = font_id.into();
            layouter.define_font(
                font_id,
                FontDefinition {
                    data,
                    index: 0,
                    ascender_fudge_in_ems: 0.0,
                    descender_fudge_in_ems: 0.0,
                    weight: None,
                    variations: Vec::new(),
                },
            );
            layouter.define_font_family(
                family_id.into(),
                FontFamilyDefinition {
                    font_ids: vec![font_id],
                    expected_member_count: 1,
                    diagnostics: Default::default(),
                },
            );
        }
        layouter
    }

    fn get_test_style(family: u64) -> Style {
        Style {
            font_family_id: family.into(),
            font_size_in_pts: 12.0,
            color: None,
        }
    }

    fn layout_text(
        layouter: &mut Layouter,
        family: u64,
        text: &str,
        options: LayoutOptions,
    ) -> Rc<LaidoutText> {
        layouter.get_or_layout(BorrowedLayoutParams {
            text,
            style: get_test_style(family),
            options,
        })
    }

    fn wrapping_options(max_width: f32) -> LayoutOptions {
        LayoutOptions {
            max_width_in_lpxs: Some(max_width),
            wrap: true,
            ..LayoutOptions::default()
        }
    }

    fn wrapped_layout(
        layouter: &mut Layouter,
        family: u64,
        text: &str,
        max_width: f32,
        max_rows: Option<usize>,
        ellipsis: bool,
    ) -> Rc<LaidoutText> {
        layout_text(
            layouter,
            family,
            text,
            LayoutOptions {
                max_rows,
                ellipsis,
                ..wrapping_options(max_width)
            },
        )
    }

    /// The appended ellipsis glyphs are stamped with `cluster == text.len()`,
    /// past every real grapheme — the one place a row can point beyond its
    /// own text.
    fn ends_with_ellipsis(text: &LaidoutText) -> bool {
        let last_row = text.rows.last().expect("layout always yields a row");
        last_row
            .glyphs
            .last()
            .is_some_and(|glyph| glyph.cluster >= last_row.text.len())
    }

    #[test]
    fn multiline_ellipsis_truncates_at_last_allowed_row() {
        let mut layouter = real_font_layouter();
        let text = "The quick brown fox jumps over the lazy dog and keeps running far beyond the fence";
        let max_width = 120.0;

        // Sanity: unrestricted, this text wraps past two rows at this width.
        let free = wrapped_layout(&mut layouter, LATIN_FAMILY, text, max_width, None, false);
        assert!(free.rows.len() > 2, "test text must overflow two rows");
        assert!(!free.is_truncated);
        assert!(!ends_with_ellipsis(&free));

        let capped = wrapped_layout(&mut layouter, LATIN_FAMILY, text, max_width, Some(2), true);
        assert_eq!(capped.rows.len(), 2, "exactly the allowed rows remain");
        assert!(capped.is_truncated);
        assert!(ends_with_ellipsis(&capped));
        // The ellipsis row obeys the width limit it truncated for.
        let last = capped.rows.last().unwrap();
        assert!(
            last.width_in_lpxs <= max_width + 0.01,
            "last row ({}) must fit the bound ({})",
            last.width_in_lpxs,
            max_width
        );
        // The first row is untouched by truncation: identical to the free layout.
        assert_eq!(capped.rows[0].text, free.rows[0].text);
    }

    #[test]
    fn multiline_ellipsis_leaves_fitting_text_alone() {
        let mut layouter = real_font_layouter();
        let text = "Two short lines";
        let laidout = wrapped_layout(&mut layouter, LATIN_FAMILY, text, 120.0, Some(2), true);
        assert!(laidout.rows.len() <= 2);
        assert!(!laidout.is_truncated, "text that fits must not be marked truncated");
        assert!(!ends_with_ellipsis(&laidout), "no ellipsis on untruncated text");
    }

    #[test]
    fn multiline_ellipsis_one_row_over_gets_ellipsis() {
        let mut layouter = real_font_layouter();
        // Find a prefix of words that lays out to exactly three rows, then cap
        // at two: the minimal "one row over" case, measured in real advances
        // instead of guessed.
        let words: Vec<&str> =
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                .split(' ')
                .collect();
        let max_width = 90.0;
        let mut three_row_text = None;
        for take in 1..=words.len() {
            let candidate = words[..take].join(" ");
            let free =
                wrapped_layout(&mut layouter, LATIN_FAMILY, &candidate, max_width, None, false);
            if free.rows.len() == 3 {
                three_row_text = Some(candidate);
                break;
            }
        }
        let text = three_row_text.expect("some prefix must lay out to three rows");
        let capped =
            wrapped_layout(&mut layouter, LATIN_FAMILY, &text, max_width, Some(2), true);
        assert_eq!(capped.rows.len(), 2);
        assert!(capped.is_truncated);
        assert!(ends_with_ellipsis(&capped));
    }

    #[test]
    fn multiline_ellipsis_fills_last_row_by_grapheme() {
        let mut layouter = real_font_layouter();
        // The word that overflows row 2 is far wider than the row: word-wrap
        // would carry it wholly to a (forbidden) third row and leave row 2
        // ellipsized at a fraction of the width. The layouter instead fills
        // the last allowed row grapheme-by-grapheme before truncating.
        let text = "on and on Supercalifragilisticexpialidocious Supercalifragilisticexpialidocious";
        let max_width = 110.0;
        let capped = wrapped_layout(&mut layouter, LATIN_FAMILY, text, max_width, Some(2), true);
        assert_eq!(capped.rows.len(), 2);
        assert!(ends_with_ellipsis(&capped));
        let last = capped.rows.last().unwrap();
        assert!(
            last.width_in_lpxs > max_width * 0.7,
            "grapheme fill must use the row ({} of {})",
            last.width_in_lpxs,
            max_width
        );
        assert!(last.width_in_lpxs <= max_width + 0.01);
    }

    #[test]
    fn multiline_ellipsis_truncates_cjk_on_grapheme_boundaries() {
        let mut layouter = real_font_layouter();
        // Unspaced CJK can break between any two ideographs, and its last row still fills by
        // grapheme for the ellipsis.
        let text = "这是一个很长的中文标题它会先换到第二行然后在第二行的结尾处出现省略号而不是被裁掉";
        let max_width = 140.0;
        let capped = wrapped_layout(&mut layouter, CJK_FAMILY, text, max_width, Some(2), true);
        assert_eq!(capped.rows.len(), 2);
        assert!(capped.is_truncated);
        assert!(ends_with_ellipsis(&capped));
        // Every surviving glyph must still start on a char boundary of its
        // row's text: truncation pops whole glyphs, never bytes.
        for row in capped.rows.iter() {
            for glyph in row.glyphs.iter().filter(|g| g.cluster < row.text.len()) {
                assert!(
                    row.text.is_char_boundary(glyph.cluster),
                    "glyph cluster {} must be a char boundary",
                    glyph.cluster
                );
            }
        }
    }

    #[test]
    fn single_line_ellipsis_under_wrapping_flow() {
        // `max_lines: 1` on a wrapping Label — the list-row configuration.
        let mut layouter = real_font_layouter();
        let text = "A single line that is much too long for the row it must fit into";
        let capped = wrapped_layout(&mut layouter, LATIN_FAMILY, text, 100.0, Some(1), true);
        assert_eq!(capped.rows.len(), 1);
        assert!(capped.is_truncated);
        assert!(ends_with_ellipsis(&capped));
        assert!(capped.rows[0].width_in_lpxs <= 100.0 + 0.01);
    }

    #[test]
    fn multiline_ellipsis_counts_explicit_newlines() {
        let mut layouter = real_font_layouter();
        let capped =
            wrapped_layout(&mut layouter, LATIN_FAMILY, "one\ntwo\nthree", 200.0, Some(2), true);
        assert_eq!(capped.rows.len(), 2);
        assert!(capped.is_truncated, "a discarded third line is a truncation");
        assert!(ends_with_ellipsis(&capped));
    }

    #[test]
    fn last_allowed_row_without_the_following_room_ends_with_an_ellipsis() {
        let mut layouter = real_font_layouter();
        let text = "aaaa bbbb";
        let max_width = layout_text(&mut layouter, LATIN_FAMILY, text, LayoutOptions::default()).size_in_lpxs.width + 10.0;
        let mut layout_with_room = |max_rows, following_room_in_lpxs| {
            let options = LayoutOptions {
                max_rows: Some(max_rows),
                ellipsis: true,
                following_room_in_lpxs,
                ..wrapping_options(max_width)
            };
            layout_text(&mut layouter, LATIN_FAMILY, text, options)
        };
        assert!(!layout_with_room(1, 5.0).is_truncated);
        let cut = layout_with_room(1, 15.0);
        assert!(cut.is_truncated && ends_with_ellipsis(&cut));
        assert!(cut.rows[0].width_in_lpxs <= max_width);
        // What follows a row before the last one can go on the next row.
        assert!(!layout_with_room(2, 15.0).is_truncated);
    }

    // -----------------------------------------------------------------------
    // Line breaking: rows wrap at the UAX #14 break opportunities that
    // `line_break` finds, measured with the bundled fonts.
    // -----------------------------------------------------------------------

    use super::{
        get_hyphen, is_hangable_white_space,
        shaper::{self, Ems, ShapeParams, ShapedText, Shaper},
        Cursor, Direction, FollowingContext, PrecedingContext, Rect, Selection, Substr,
    };
    use crate::text::line_break::{find_line_breaks, LineBreak};
    use unicode_segmentation::UnicodeSegmentation;

    const START: PrecedingContext = PrecedingContext::PARAGRAPH_START;
    const END: FollowingContext = FollowingContext::END_OF_TEXT;

    fn measure_width(layouter: &mut Layouter, family: u64, text: &str) -> f32 {
        let style = get_test_style(family);
        layouter
            .get_or_load_font_family(style.font_family_id)
            .get_or_shape(text.into())
            .width_in_ems
            * style.font_size_in_lpxs()
    }

    /// Measures the hyphen the layouter draws where a row breaks after a soft hyphen.
    fn measure_hyphen_width(layouter: &mut Layouter, family: u64) -> f32 {
        let hyphen = get_hyphen(&layouter.get_or_load_font_family(family.into()));
        measure_width(layouter, family, hyphen)
    }

    /// Measures the text before `line_break` the way the layouter fits it at the end of a row.
    ///
    /// That leaves out hanging white space, and adds `hyphen_width` right after a soft hyphen.
    fn measure_unit(
        layouter: &mut Layouter,
        family: u64,
        text: &str,
        line_break: LineBreak,
        hyphen_width: f32,
    ) -> f32 {
        let unit = &text[..line_break.byte_index - line_break.newline_len];
        let is_soft_hyphen_break = !line_break.is_forced
            && line_break.byte_index < text.len()
            && unit.ends_with('\u{AD}');
        measure_width(layouter, family, unit.trim_end_matches(is_hangable_white_space))
            + if is_soft_hyphen_break { hyphen_width } else { 0.0 }
    }

    /// Returns where each row after the first starts.
    fn get_row_starts(text: &LaidoutText) -> Vec<usize> {
        text.rows[1..]
            .iter()
            .map(|row| row.text.start_in_parent())
            .collect()
    }

    fn get_row_texts(text: &LaidoutText) -> Vec<&str> {
        text.rows.iter().map(|row| row.text.as_str()).collect()
    }

    /// Checks that no row starts at `indices`, at any width that wraps `text` between units.
    ///
    /// Returns every row start we saw.
    fn check_no_row_starts_at(
        layouter: &mut Layouter,
        family: u64,
        text: &str,
        indices: &[usize],
    ) -> Vec<usize> {
        let full_width_in_lpxs = layout_text(layouter, family, text, LayoutOptions::default())
            .size_in_lpxs
            .width;
        // The widest unit is the narrowest width that wraps the text between units.
        let hyphen_width = measure_hyphen_width(layouter, family);
        let mut unit_start = 0;
        let mut widest_unit_width_in_lpxs = 0.0f32;
        for line_break in find_line_breaks(text, START, END) {
            let line_break_in_unit = LineBreak {
                byte_index: line_break.byte_index - unit_start,
                ..line_break
            };
            let width_in_lpxs = measure_unit(
                layouter,
                family,
                &text[unit_start..],
                line_break_in_unit,
                hyphen_width,
            );
            widest_unit_width_in_lpxs = widest_unit_width_in_lpxs.max(width_in_lpxs);
            unit_start = line_break.byte_index;
        }
        let mut max_width_in_lpxs = widest_unit_width_in_lpxs.ceil();
        let mut seen_row_starts = Vec::new();
        while max_width_in_lpxs < full_width_in_lpxs {
            let laidout = layout_text(layouter, family, text, wrapping_options(max_width_in_lpxs));
            for row_start in get_row_starts(&laidout) {
                assert!(
                    !indices.contains(&row_start),
                    "{text:?} at {max_width_in_lpxs}: {:?}",
                    get_row_texts(&laidout)
                );
                if !seen_row_starts.contains(&row_start) {
                    seen_row_starts.push(row_start);
                }
            }
            max_width_in_lpxs += 0.5;
        }
        assert!(!seen_row_starts.is_empty(), "{text:?} never wrapped");
        seen_row_starts
    }

    #[test]
    fn quotes_never_start_or_end_a_row() {
        let mut layouter = real_font_layouter();
        let text = "He said \"hello world\" to me";
        let indices = [text.find('"').unwrap() + 1, text.rfind('"').unwrap()];
        check_no_row_starts_at(&mut layouter, LATIN_FAMILY, text, &indices);
        let text = "She wrote \u{201C}quite so\u{201D} back to them";
        let indices = [
            text.find('\u{201C}').unwrap() + '\u{201C}'.len_utf8(),
            text.find('\u{201D}').unwrap(),
        ];
        check_no_row_starts_at(&mut layouter, LATIN_FAMILY, text, &indices);
    }

    #[test]
    fn punctuation_after_a_space_stays_attached() {
        let mut layouter = real_font_layouter();
        for text in [
            "Il a dit Bonjour ! et puis il est parti",
            "Hello , world and some more words",
            "Vraiment ? Oui ; bien s\u{FB}r : merci",
        ] {
            let indices: Vec<usize> = text
                .char_indices()
                .filter(|&(_, c)| matches!(c, '!' | '?' | ',' | ';' | ':'))
                .map(|(index, _)| index)
                .collect();
            check_no_row_starts_at(&mut layouter, LATIN_FAMILY, text, &indices);
        }
    }

    #[test]
    fn urls_break_after_slashes() {
        let mut layouter = real_font_layouter();
        let url = "https://example.com/some/long/path?query=value";
        let text = format!("see {url} for details");
        let url_range = 4..4 + url.len();
        let indices: Vec<usize> = (url_range.start + 1..url_range.end)
            .filter(|&index| !text[..index].ends_with(['/', '?']))
            .collect();
        let row_starts = check_no_row_starts_at(&mut layouter, LATIN_FAMILY, &text, &indices);
        assert!(row_starts
            .iter()
            .any(|&index| url_range.contains(&index) && text[..index].ends_with('/')));
    }

    #[test]
    fn dates_and_flags_stay_whole() {
        let mut layouter = real_font_layouter();
        let text = "Due 2026-10-26 or 12/31/2026, see \u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7}\u{1F1EB}\u{1F1F7} flags";
        let mut indices = Vec::new();
        for date in ["2026-10-26", "12/31/2026"] {
            let start = text.find(date).unwrap();
            indices.extend(start + 1..start + date.len());
        }
        // Each flag is two regional indicators of 4 bytes each.
        let flags_start = text.find('\u{1F1FA}').unwrap();
        indices.extend([flags_start + 4, flags_start + 12, flags_start + 20]);
        check_no_row_starts_at(&mut layouter, LATIN_FAMILY, text, &indices);
    }

    #[test]
    fn no_break_spaces_and_word_joiners_glue() {
        let mut layouter = real_font_layouter();
        let text = "Mr.\u{A0}Smith met Mrs.\u{A0}Jones and the no\u{2060}break crew";
        let indices: Vec<usize> = text
            .char_indices()
            .filter(|&(_, c)| matches!(c, '\u{A0}' | '\u{2060}'))
            .flat_map(|(index, c)| [index, index + c.len_utf8()])
            .collect();
        check_no_row_starts_at(&mut layouter, LATIN_FAMILY, text, &indices);
    }

    #[test]
    fn cjk_rows_never_start_with_closing_punctuation_or_small_kana() {
        let mut layouter = real_font_layouter();
        let text = "\u{300C}今日は、とても良い天気ですね。\u{300D}と彼女は言った。ちょっとチャットしましょう、きゃっと笑った。";
        let indices: Vec<usize> = text
            .char_indices()
            .filter_map(|(index, c)| match c {
                '。' | '、' | '\u{300D}' | 'ゃ' | 'ょ' | 'っ' => Some(index),
                '\u{300C}' => Some(index + c.len_utf8()),
                _ => None,
            })
            .collect();
        check_no_row_starts_at(&mut layouter, CJK_FAMILY, text, &indices);
    }

    #[test]
    fn soft_hyphens_show_a_hyphen_that_counts_for_fit() {
        let mut layouter = real_font_layouter();
        let text = "a hyphen\u{AD}ation";
        let soft_hyphen_index = text.find('\u{AD}').unwrap();
        let hyphen_width = measure_hyphen_width(&mut layouter, LATIN_FAMILY);
        let head_width = measure_width(&mut layouter, LATIN_FAMILY, "a hyphen\u{AD}");
        assert!((head_width - measure_width(&mut layouter, LATIN_FAMILY, "a hyphen")).abs() < 0.01);

        // With room for the hyphen, the row breaks after the soft hyphen and ends with a hyphen.
        let max_width = head_width + hyphen_width + 0.5;
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, wrapping_options(max_width));
        assert_eq!(get_row_texts(&laidout), ["a hyphen\u{AD}", "ation"]);
        let row = &laidout.rows[0];
        let hyphen = row.glyphs.last().unwrap();
        assert_eq!(hyphen.cluster, soft_hyphen_index);
        assert!((hyphen.advance_in_lpxs() - hyphen_width).abs() < 0.01);
        assert!(row.width_in_lpxs <= max_width);
        // The hyphen is part of its soft hyphen's cluster, so the caret after that goes after it.
        let row_end = laidout.cursor_to_position(Cursor {
            index: soft_hyphen_index + '\u{AD}'.len_utf8(),
            prefer_next_row: false,
        });
        assert_eq!((row_end.row_index, row_end.x_in_lpxs), (0, row.width_in_lpxs));

        // Without room for it, the unit moves to the next row.
        let max_width = head_width + hyphen_width - 0.5;
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, wrapping_options(max_width));
        assert_eq!(get_row_texts(&laidout)[0], "a ");

        // A unit that starts its row but only fits without the hyphen breaks between graphemes,
        // so the hyphen never overflows the row.
        let max_width = measure_width(&mut layouter, LATIN_FAMILY, "hyphen") + 0.5 * hyphen_width;
        let options = wrapping_options(max_width);
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, &text[2..], options);
        assert_eq!(get_row_texts(&laidout), ["hyphen", "\u{AD}ation"]);
        assert!(laidout.size_in_lpxs.width <= max_width);

        // When the row doesn't break there, there's no hyphen.
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, wrapping_options(500.0));
        assert_eq!(get_row_texts(&laidout), [text]);
        let width = measure_width(&mut layouter, LATIN_FAMILY, text);
        assert!((laidout.rows[0].width_in_lpxs - width).abs() < 0.01);
    }

    #[test]
    fn hanging_spaces_dont_overflow_or_shift_rows() {
        let mut layouter = real_font_layouter();
        let hello_width = measure_width(&mut layouter, LATIN_FAMILY, "hello");
        let max_width = hello_width + 20.0;
        let spaces = " ".repeat(10);
        assert!(measure_width(&mut layouter, LATIN_FAMILY, &spaces) > 20.0);
        for align in [0.0, 0.5, 1.0] {
            let options = LayoutOptions {
                align,
                ..wrapping_options(max_width)
            };
            // At a wrap the spaces hang past the edge, so they don't move the row's text or
            // make the text any wider.
            let text = format!("hello{spaces}world");
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, &text, options);
            assert_eq!(get_row_texts(&laidout), [&text[..15], "world"]);
            let row = &laidout.rows[0];
            assert!(row.width_in_lpxs > max_width);
            assert!((row.width_without_hanging_in_lpxs() - hello_width).abs() < 0.01);
            assert!((row.origin_in_lpxs.x - align * (max_width - hello_width)).abs() < 0.01);
            assert!(laidout.size_in_lpxs.width <= max_width);
            // Zero-width text after them isn't white space, so it wraps like any other unit, and
            // the spaces still hang.
            let text = format!("hello{spaces}\u{AD}");
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, &text, options);
            assert_eq!(get_row_texts(&laidout), [&text[..15], "\u{AD}"]);
            let row = &laidout.rows[0];
            assert!((row.origin_in_lpxs.x - align * (max_width - hello_width)).abs() < 0.01);
            assert!(laidout.size_in_lpxs.width <= max_width);

            // At a hard line break or the end of the text, only the part that overflows hangs.
            for text in [format!("hello{spaces}\nworld"), format!("hello{spaces}")] {
                let laidout = layout_text(&mut layouter, LATIN_FAMILY, &text, options);
                let row = &laidout.rows[0];
                assert!((row.width_without_hanging_in_lpxs() - max_width).abs() < 0.01);
                assert!(row.origin_in_lpxs.x.abs() < 0.01);
                assert!(laidout.size_in_lpxs.width <= max_width + 0.01);
            }
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, "hello ", options);
            let width = measure_width(&mut layouter, LATIN_FAMILY, "hello ");
            assert_eq!(laidout.rows[0].hanging_width_in_lpxs, 0.0);
            assert!((laidout.rows[0].origin_in_lpxs.x - align * (max_width - width)).abs() < 0.01);
        }

        // The next run starts past the edge after those spaces, and doesn't grow the text either.
        let mut line_break_before = START;
        line_break_before.append(&format!("hello{spaces}"));
        let options = LayoutOptions {
            first_row_indent_in_lpxs: max_width + 15.0,
            line_break_before,
            ..wrapping_options(max_width)
        };
        for text in ["world", "  world"] {
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, options);
            assert_eq!(get_row_texts(&laidout), [&text[..text.len() - 5], "world"]);
            assert!((laidout.rows[0].width_without_hanging_in_lpxs() - max_width).abs() < 0.01);
            assert!(laidout.size_in_lpxs.width <= max_width + 0.01);
        }
        // Zero-width text that isn't white space wraps too, here at the start of the run.
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, "\u{200B}world", options);
        assert_eq!(get_row_texts(&laidout), ["", "\u{200B}world"]);
        assert!(laidout.size_in_lpxs.width <= max_width + 0.01);
        // Unless the run before kept room for it, and then it stays past the edge but takes no room.
        let reserved = LayoutOptions {
            is_start_reserved: true,
            ..options
        };
        for text in ["\u{200B}world", "  \u{200B}world"] {
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, reserved);
            assert_eq!(get_row_texts(&laidout), [&text[..text.len() - 5], "world"]);
            assert!(laidout.size_in_lpxs.width <= max_width + 0.01, "{text:?}");
        }
    }

    #[test]
    fn hard_line_breaks_end_rows() {
        let mut layouter = real_font_layouter();
        for options in [LayoutOptions::default(), wrapping_options(500.0)] {
            let text = "a\r\nb\rc\u{2028}d\u{85}e\u{B}f\u{C}g\u{2029}h\ni";
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, options);
            assert_eq!(get_row_texts(&laidout), ["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
            let newlines: Vec<bool> = laidout.rows.iter().map(|row| row.newline).collect();
            assert_eq!(newlines, [true, true, true, true, true, true, true, true, false]);
            // A text that ends with a line break still ends with an empty row.
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, "one\ntwo\n", options);
            assert_eq!(get_row_texts(&laidout), ["one", "two", ""]);
            let newlines: Vec<bool> = laidout.rows.iter().map(|row| row.newline).collect();
            assert_eq!(newlines, [true, true, false]);
        }

        // A CR at the end of a run whose LF starts the next run isn't drawn, and the LF ends
        // the line.
        let mut line_break_after = END;
        line_break_after.prepend("\nnext");
        let options = LayoutOptions {
            line_break_after,
            ..wrapping_options(500.0)
        };
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, "end\r", options);
        assert_eq!(get_row_texts(&laidout), ["end"]);
        assert!(!laidout.rows[0].newline);
        let options = LayoutOptions {
            first_row_indent_in_lpxs: laidout.rows[0].width_in_lpxs,
            line_break_before: laidout.line_break_before_next_run,
            ..wrapping_options(500.0)
        };
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, "\nnext", options);
        assert_eq!(get_row_texts(&laidout), ["", "next"]);
        assert!(laidout.rows[0].newline);

        // A caret between a CR and its LF stays at the end of the row they end.
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, "ab\r\ncd", wrapping_options(500.0));
        let position = laidout.cursor_to_position(Cursor {
            index: 3,
            prefer_next_row: false,
        });
        assert_eq!((position.row_index, position.x_in_lpxs), (0, laidout.rows[0].width_in_lpxs));
    }

    #[test]
    fn over_long_words_break_between_graphemes() {
        let mut layouter = real_font_layouter();
        let text = "a Supercalifragilisticexpialidocious word";
        let max_width = 60.0;
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, wrapping_options(max_width));
        let row_texts = get_row_texts(&laidout);
        // The long word starts a row of its own, and then breaks wherever a row is full.
        assert_eq!(row_texts[0], "a ");
        assert!(row_texts.len() > 4, "{row_texts:?}");
        assert_eq!(row_texts.concat(), text);
        for row in &laidout.rows {
            assert!(row.width_without_hanging_in_lpxs() <= max_width, "{row_texts:?}");
        }
        let word_rows = &laidout.rows[1..laidout.rows.len() - 2];
        for (row, next_row) in word_rows.iter().zip(&laidout.rows[2..]) {
            let next_grapheme = next_row.text.graphemes(true).next().unwrap();
            let full_row = format!("{}{next_grapheme}", row.text.as_str());
            let full_row_width = measure_width(&mut layouter, LATIN_FAMILY, &full_row);
            assert!(full_row_width > max_width, "{row_texts:?}");
        }
    }

    #[test]
    fn trailing_glue_and_reserved_starts_keep_runs_together() {
        let mut layouter = real_font_layouter();
        // A "!" in a run of its own still has to stay with the "Bonjour" before it.
        let (text_0, text_1) = ("Il a dit Bonjour ", "!");
        let mut line_break_after = END;
        line_break_after.prepend(text_1);
        let glue_width = measure_width(&mut layouter, LATIN_FAMILY, text_1);
        let max_width = measure_width(&mut layouter, LATIN_FAMILY, "Il a dit Bonjour !") - 1.0;
        let options = LayoutOptions {
            line_break_after,
            ..wrapping_options(max_width)
        };
        let glued = LayoutOptions {
            trailing_glue_width_in_lpxs: glue_width,
            ..options
        };
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text_0, glued);
        assert_eq!(get_row_texts(&laidout), ["Il a dit ", "Bonjour "]);
        assert!(laidout.is_trailing_glue_reserved);
        let unglued = layout_text(&mut layouter, LATIN_FAMILY, text_0, options);
        assert_eq!(get_row_texts(&unglued), [text_0]);
        // A run that doesn't wrap never checks its glue, so it keeps no room for it.
        let unwrapped = LayoutOptions {
            wrap: false,
            ..glued
        };
        let unwrapped = layout_text(&mut layouter, LATIN_FAMILY, text_0, unwrapped);
        assert_eq!(get_row_texts(&unwrapped), [text_0]);
        assert!(!unwrapped.is_trailing_glue_reserved);

        // The run it reserved room for stays on that row, even if it measures a bit too wide.
        let options = LayoutOptions {
            first_row_indent_in_lpxs: max_width - glue_width + 0.5,
            line_break_before: laidout.line_break_before_next_run,
            ..wrapping_options(max_width)
        };
        let reserved = LayoutOptions {
            is_start_reserved: true,
            ..options
        };
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text_1, reserved);
        assert_eq!(get_row_texts(&laidout), [text_1]);
        // Without the reservation, it wraps at the start of the run.
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, text_1, options);
        assert_eq!(get_row_texts(&laidout), ["", text_1]);

        // A run that's all one reserved unit passes the reservation on to the next run.
        let mut line_break_before = START;
        line_break_before.append("ab");
        let mut line_break_after = END;
        line_break_after.prepend("cd ef");
        let glue_width = measure_width(&mut layouter, LATIN_FAMILY, "cd");
        let options = LayoutOptions {
            first_row_indent_in_lpxs: max_width - 1.0,
            line_break_before,
            line_break_after,
            trailing_glue_width_in_lpxs: glue_width,
            is_start_reserved: true,
            ..wrapping_options(max_width)
        };
        let laidout = layout_text(&mut layouter, LATIN_FAMILY, "xy", options);
        assert_eq!(get_row_texts(&laidout), ["xy"]);
        assert!(laidout.is_trailing_glue_reserved);
    }

    /// Lays out `text` as two runs split at `split`, like two styled runs on one line.
    ///
    /// Returns where each visual row after the first starts.
    fn layout_as_two_runs(
        layouter: &mut Layouter,
        family: u64,
        text: &str,
        split: usize,
        max_width: f32,
    ) -> Vec<usize> {
        let (text_0, text_1) = text.split_at(split);
        let mut line_break_after = END;
        line_break_after.prepend(text_1);
        let mut line_break_before = START;
        line_break_before.append(text_0);
        // The glue is the second run's text up to its first break opportunity. After a soft
        // hyphen, that needs room for the hyphen, or for the text up to the next one if narrower.
        let hyphen_width = measure_hyphen_width(layouter, family);
        let mut line_breaks = find_line_breaks(text_1, line_break_before, END);
        let trailing_glue_width_in_lpxs = match line_breaks.next() {
            Some(line_break) if line_break.byte_index > 0 => {
                let rest_width = line_breaks.next().map_or(0.0, |next_line_break| {
                    let next_line_break_in_rest = LineBreak {
                        byte_index: next_line_break.byte_index - line_break.byte_index,
                        ..next_line_break
                    };
                    let rest = &text_1[line_break.byte_index..];
                    measure_unit(layouter, family, rest, next_line_break_in_rest, hyphen_width)
                });
                measure_unit(layouter, family, text_1, line_break, hyphen_width.min(rest_width))
            }
            _ => 0.0,
        };
        let options = LayoutOptions {
            line_break_after,
            trailing_glue_width_in_lpxs,
            ..wrapping_options(max_width)
        };
        let laidout_0 = layout_text(layouter, family, text_0, options);
        assert_eq!(laidout_0.line_break_before_next_run, line_break_before);
        let options = LayoutOptions {
            first_row_indent_in_lpxs: laidout_0.rows.last().unwrap().width_in_lpxs,
            line_break_before: laidout_0.line_break_before_next_run,
            is_start_reserved: laidout_0.is_trailing_glue_reserved,
            ..wrapping_options(max_width)
        };
        let laidout_1 = layout_text(layouter, family, text_1, options);
        let mut row_starts = get_row_starts(&laidout_0);
        row_starts.extend(
            get_row_starts(&laidout_1)
                .into_iter()
                .map(|row_start| split + row_start),
        );
        row_starts
    }

    #[test]
    fn runs_split_before_a_soft_hyphen_wrap_like_the_whole_text() {
        let mut layouter = real_font_layouter();
        let head = "na\u{EF}ve dog \u{201C}we'l\u{AD}";
        let split = head.len() - '\u{AD}'.len_utf8();
        let head_width = measure_width(&mut layouter, LATIN_FAMILY, head);
        let hyphen_width = measure_hyphen_width(&mut layouter, LATIN_FAMILY);
        let l_width = measure_width(&mut layouter, LATIN_FAMILY, "l");
        assert!(l_width < hyphen_width);
        // The row has room for the unit that ends in the soft hyphen, but not for its hyphen.
        // So the row keeps going past it with the narrow "l", but wraps before it with "lo".
        for (text, max_width, first_row) in [
            (
                format!("{head}l we'll"),
                head_width + 0.5 * (l_width + hyphen_width),
                format!("{head}l "),
            ),
            (
                format!("{head}lo we'll"),
                head_width + 0.5 * hyphen_width,
                "na\u{EF}ve dog ".to_string(),
            ),
        ] {
            let options = wrapping_options(max_width);
            let whole = layout_text(&mut layouter, LATIN_FAMILY, &text, options);
            assert_eq!(get_row_texts(&whole)[0], first_row);
            let row_starts =
                layout_as_two_runs(&mut layouter, LATIN_FAMILY, &text, split, max_width);
            assert_eq!(row_starts, get_row_starts(&whole), "{text:?}");
        }
    }

    #[test]
    fn split_runs_wrap_like_the_whole_text() {
        let mut layouter = real_font_layouter();
        let mut matched_count = 0;
        let mut noise_count = 0;
        let mut too_wide_count = 0;
        for (family, text) in [
            (LATIN_FAMILY, "He said \"hello world\" to me, then left (quietly)."),
            (LATIN_FAMILY, "Bonjour ! \u{C7}a va ? Oui , tr\u{E8}s bien ; merci."),
            (LATIN_FAMILY, "see https://example.com/some/long/path?query=value&x=1 now"),
            (LATIN_FAMILY, "On 2026-10-26 we paid $12.50 (or 50%) for 1,000.00 items."),
            (LATIN_FAMILY, "Mr.\u{A0}Smith met no\u{2060}break and state-of-the-art folks."),
            (LATIN_FAMILY, "a hyphen\u{AD}ation exam\u{AD}ple with soft\u{AD}hyphens\u{AD}everywhere"),
            (LATIN_FAMILY, "spaces   between    words  and\ttabs\there"),
            (LATIN_FAMILY, "lines\nwith\r\nhard\rbreaks\u{2028}of\u{85}every kind"),
            (LATIN_FAMILY, "Supercalifragilisticexpialidocious is long, antidisestablishmentarianism too"),
            (LATIN_FAMILY, "\u{AB}\u{202F}Bonjour\u{202F}\u{BB} dit-il, puis \u{AB} au revoir \u{BB}."),
            (LATIN_FAMILY, "AVA WAW To Ty Yo kerning pairs AVAVAV WAWAWA fifl office"),
            (LATIN_FAMILY, "flags \u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7} and \u{1F44D}\u{1F3FD} plus \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} here"),
            (CJK_FAMILY, "\u{300C}今日は、とても良い天気ですね。\u{300D}と彼女は言った。"),
            (CJK_FAMILY, "ちょっとチャットしましょう、きゃっと言った。中文也可以，对吧？"),
        ] {
            let line_breaks: Vec<LineBreak> = find_line_breaks(text, START, END).collect();
            let hyphen_width = measure_hyphen_width(&mut layouter, family);
            // We try every few pixels, and the exact widths of the first line's prefixes, which
            // leave a row no slack, so the smallest shaping difference can move a break.
            let full_width = layout_text(&mut layouter, family, text, LayoutOptions::default())
                .size_in_lpxs
                .width;
            let mut max_widths: Vec<f32> = (30..full_width as usize + 10)
                .step_by(10)
                .map(|max_width| max_width as f32)
                .collect();
            for &line_break in line_breaks.iter().take_while(|line_break| !line_break.is_forced) {
                let width = measure_unit(&mut layouter, family, text, line_break, hyphen_width);
                max_widths.extend([width - 0.01, width, width + 0.01]);
            }
            for max_width in max_widths {
                let whole = layout_text(&mut layouter, family, text, wrapping_options(max_width));
                let whole_row_starts = get_row_starts(&whole);
                for (split, _) in text.grapheme_indices(true).skip(1) {
                    // The run before can't know whether the next one wraps right at its start, so
                    // it never draws the hyphen of a soft hyphen it ends with.
                    if text[..split].ends_with('\u{AD}') {
                        continue;
                    }
                    // A unit wider than a row breaks between graphemes, and each run can only do
                    // that within itself.
                    let unit_start = line_breaks
                        .iter()
                        .map(|line_break| line_break.byte_index)
                        .take_while(|&index| index <= split)
                        .last()
                        .unwrap_or(0);
                    let unit_break = *line_breaks
                        .iter()
                        .find(|line_break| line_break.byte_index > split)
                        .unwrap();
                    let unit_break_in_unit = LineBreak {
                        byte_index: unit_break.byte_index - unit_start,
                        ..unit_break
                    };
                    let unit_text = &text[unit_start..];
                    let unit_width = measure_unit(
                        &mut layouter,
                        family,
                        unit_text,
                        unit_break_in_unit,
                        hyphen_width,
                    );
                    if unit_start < split && unit_width > max_width {
                        too_wide_count += 1;
                        continue;
                    }
                    let row_starts =
                        layout_as_two_runs(&mut layouter, family, text, split, max_width);
                    if row_starts == whole_row_starts {
                        matched_count += 1;
                        continue;
                    }
                    // The fitter adds up units shaped one by one, and the runs shape apart at
                    // `split`, so a break can only move by the kerning lost or gained there.
                    let row_index = row_starts
                        .iter()
                        .zip(&whole_row_starts)
                        .take_while(|(row_start, whole_row_start)| row_start == whole_row_start)
                        .count();
                    let row = &whole.rows[row_index];
                    let row_start = row.text.start_in_parent();
                    // The row could have grown by the next unit and its hyphen, or by the next
                    // two units, which don't need that hyphen.
                    let next_line_breaks: Vec<LineBreak> = line_breaks
                        .iter()
                        .copied()
                        .filter(|line_break| line_break.byte_index > row.text.end_in_parent())
                        .take(2)
                        .collect();
                    let window_end = next_line_breaks.last().map_or(text.len(), |line_break| {
                        line_break.byte_index - line_break.newline_len
                    });
                    let next_unit_width = next_line_breaks
                        .iter()
                        .map(|&line_break| {
                            let line_break_in_row = LineBreak {
                                byte_index: line_break.byte_index - row_start,
                                ..line_break
                            };
                            let text = &text[row_start..];
                            measure_unit(&mut layouter, family, text, line_break_in_row, hyphen_width)
                        })
                        .reduce(f32::min)
                        .unwrap_or(row.width_without_hanging_in_lpxs());
                    let mut piece_ends = vec![row_start];
                    piece_ends.extend(
                        line_breaks
                            .iter()
                            .map(|line_break| line_break.byte_index)
                            .filter(|&index| row_start < index && index < window_end),
                    );
                    piece_ends.push(window_end);
                    if let Err(position) = piece_ends.binary_search(&split) {
                        piece_ends.insert(position, split);
                    }
                    // We add up the kerning at each boundary on its own, so it can't cancel out.
                    let mut shaping_noise = 0.0;
                    for pieces in piece_ends.windows(3) {
                        let (start, middle, end) = (pieces[0], pieces[1], pieces[2]);
                        let kerning = measure_width(&mut layouter, family, &text[start..end])
                            - measure_width(&mut layouter, family, &text[start..middle])
                            - measure_width(&mut layouter, family, &text[middle..end]);
                        shaping_noise += kerning.abs();
                    }
                    let margin = (row.width_without_hanging_in_lpxs() - max_width)
                        .abs()
                        .min((next_unit_width - max_width).abs());
                    assert!(
                        !row.newline
                            && row_start < split
                            && split < window_end
                            && margin <= shaping_noise + 0.001,
                        "{text:?} split at {split} at {max_width}: rows start at {row_starts:?}, \
                         but at {whole_row_starts:?} unsplit"
                    );
                    noise_count += 1;
                }
            }
        }
        println!(
            "Split runs: {matched_count} wrap like the whole text, {noise_count} differ by \
             shaping noise, {too_wide_count} split a unit wider than a row"
        );
        assert!(matched_count > 0);
    }

    #[test]
    fn carets_move_forward_and_round_trip() {
        let mut layouter = real_font_layouter();
        for (family, text, max_width) in [
            (LATIN_FAMILY, "He said \"hello world\" to me, then left. Again and again.", 80.0),
            (LATIN_FAMILY, "hello          world wraps after its hanging spaces", 70.0),
            (LATIN_FAMILY, "one\r\ntwo\nthree\u{2028}four", 200.0),
            (CJK_FAMILY, "今日は、とても良い天気ですね。", 60.0),
        ] {
            let laidout = layout_text(&mut layouter, family, text, wrapping_options(max_width));
            for row in &laidout.rows {
                let mut previous_x = f32::MIN;
                let indices = row
                    .text
                    .grapheme_indices(true)
                    .map(|(index, _)| index)
                    .chain([row.text.len()]);
                for index in indices {
                    let x = row.index_to_x_in_lpxs(index);
                    assert!(x >= previous_x, "{text:?} at {index} in {:?}", row.text);
                    previous_x = x;
                }
            }
            // Every char boundary maps to a caret and back, except inside a hard line break, where
            // the caret stays at the end of the row it ends.
            let indices = text.char_indices().map(|(index, _)| index).chain([text.len()]);
            for index in indices {
                for prefer_next_row in [false, true] {
                    let cursor = Cursor {
                        index,
                        prefer_next_row,
                    };
                    let position = laidout.cursor_to_position(cursor);
                    let row = &laidout.rows[position.row_index];
                    assert!(row.text.start_in_parent() <= index, "{text:?} at {cursor:?}");
                    let round_trip = laidout.position_to_cursor(position);
                    let index = index.min(row.text.end_in_parent());
                    assert_eq!(round_trip.index, index, "{text:?} at {cursor:?}");
                }
            }
        }
        // The caret after hanging spaces goes past the row's visible text, where its glyphs end.
        let max_width = measure_width(&mut layouter, LATIN_FAMILY, "hello") + 20.0;
        let laidout = layout_text(
            &mut layouter,
            LATIN_FAMILY,
            "hello          world",
            wrapping_options(max_width),
        );
        let row = &laidout.rows[0];
        let position = laidout.cursor_to_position(Cursor {
            index: row.text.len(),
            prefer_next_row: false,
        });
        assert_eq!((position.row_index, position.x_in_lpxs), (0, row.width_in_lpxs));
        assert!(position.x_in_lpxs > row.width_without_hanging_in_lpxs());
    }

    #[test]
    fn carets_and_selections_follow_rtl_text() {
        // Every glyph here is a grapheme, so we know each grapheme's text and edges.
        struct Grapheme {
            start: usize,
            end: usize,
            left: f32,
            right: f32,
            is_rtl: bool,
        }

        let mut layouter = real_font_layouter();
        for (text, max_width) in [
            ("שלום עולם hello עולם שלום world", 1000.0),
            ("שלום עולם hello עולם שלום world", 60.0),
            ("hello שלום", 1000.0),
            ("abc שלו 123 def", 1000.0),
            ("ש abc", 1000.0),
            // HarfBuzz still draws the Hebrew right to left.
            ("\u{202D}שלום\u{202C} abc", 1000.0),
        ] {
            let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, wrapping_options(max_width));
            let rows: Vec<_> = laidout
                .rows
                .iter()
                .map(|row| {
                    let graphemes: Vec<_> = row
                        .glyphs
                        .iter()
                        .enumerate()
                        .map(|(k, glyph)| Grapheme {
                            start: glyph.cluster,
                            end: glyph.cluster
                                + row.text[glyph.cluster..].graphemes(true).next().unwrap().len(),
                            left: glyph.origin_in_lpxs.x,
                            right: row
                                .glyphs
                                .get(k + 1)
                                .map_or(row.width_in_lpxs, |next| next.origin_in_lpxs.x),
                            is_rtl: glyph.direction == Direction::Rtl,
                        })
                        .collect();
                    (row, graphemes)
                })
                .collect();
            for (row, graphemes) in &rows {
                let carets: Vec<_> = row
                    .text
                    .grapheme_indices(true)
                    .map(|(index, _)| index)
                    .chain([row.text.len()])
                    .map(|index| (index, row.index_to_x_in_lpxs(index)))
                    .collect();
                for &(index, x) in &carets {
                    // A click goes back to the caret, unless a zero-width grapheme puts two there.
                    if carets.iter().filter(|caret| caret.1 == x).count() == 1 {
                        assert_eq!(row.x_in_lpxs_to_index(x), index, "{text:?} at {index}");
                    }
                    // The caret is at the leading edge of the grapheme after it or the trailing
                    // edge of the one before it.
                    let leading = graphemes
                        .iter()
                        .find(|grapheme| grapheme.start == index)
                        .map(|grapheme| if grapheme.is_rtl { grapheme.right } else { grapheme.left });
                    let trailing = graphemes
                        .iter()
                        .find(|grapheme| grapheme.end == index)
                        .map(|grapheme| if grapheme.is_rtl { grapheme.left } else { grapheme.right });
                    assert!(leading == Some(x) || trailing == Some(x), "{text:?} at {index}");
                }
                // Between two Hebrew letters, the caret is between their glyphs.
                for pair in graphemes.windows(2) {
                    if pair[0].is_rtl && pair[1].is_rtl && pair[1].end == pair[0].start {
                        assert_eq!(row.index_to_x_in_lpxs(pair[0].start), pair[1].left, "{text:?}");
                    }
                }
            }
            // A selection covers what it selects and nothing else, however that's split up.
            let indices: Vec<usize> = text
                .grapheme_indices(true)
                .map(|(index, _)| index)
                .chain([text.len()])
                .collect();
            for (k, &start) in indices.iter().enumerate() {
                for &end in &indices[k..] {
                    let rects = laidout.selection_rects(Selection {
                        anchor: Cursor { index: start, prefer_next_row: false },
                        cursor: Cursor { index: end, prefer_next_row: false },
                    });
                    for (row, graphemes) in &rows {
                        let row_start = row.text.start_in_parent();
                        for grapheme in graphemes {
                            // A zero-width grapheme, like the LRO, is at the edge of a span anyway.
                            if grapheme.left == grapheme.right {
                                continue;
                            }
                            let is_selected =
                                start <= row_start + grapheme.start && row_start + grapheme.end <= end;
                            let middle = 0.5 * (grapheme.left + grapheme.right);
                            let is_covered = rects.iter().any(|rect| {
                                let Rect { origin, size } = rect.rect_in_lpxs;
                                (origin.y..=origin.y + size.height).contains(&row.origin_in_lpxs.y)
                                    && (origin.x..=origin.x + size.width).contains(&middle)
                            });
                            assert_eq!(is_covered, is_selected, "{text:?} from {start} to {end}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn selections_stay_inside_rows_an_ellipsis_cuts() {
        let mut layouter = real_font_layouter();
        // This draws as `100 ריחמ`, so an ellipsis at the right cuts off the start of the text.
        let text = "מחיר 100";
        let indices: Vec<usize> =
            text.char_indices().map(|(index, _)| index).chain([text.len()]).collect();
        let full_width = measure_width(&mut layouter, LATIN_FAMILY, text);
        for max_width in (1..full_width.ceil() as usize).map(|max_width| max_width as f32) {
            for options in [
                LayoutOptions {
                    max_width_in_lpxs: Some(max_width),
                    ellipsis: true,
                    ..LayoutOptions::default()
                },
                LayoutOptions {
                    max_rows: Some(1),
                    ellipsis: true,
                    ..wrapping_options(max_width)
                },
            ] {
                let laidout = layout_text(&mut layouter, LATIN_FAMILY, text, options);
                for (k, &start) in indices.iter().enumerate() {
                    for &end in &indices[k..] {
                        let rects = laidout.selection_rects(Selection {
                            anchor: Cursor { index: start, prefer_next_row: false },
                            cursor: Cursor { index: end, prefer_next_row: false },
                        });
                        for rect in &rects {
                            let width = rect.rect_in_lpxs.size.width;
                            assert!(width >= 0.0, "at {max_width} from {start} to {end}");
                        }
                        // Selecting all of it covers all the text the row draws.
                        if (start, end) == (0, text.len()) {
                            let row = &laidout.rows[0];
                            let text_glyphs =
                                row.glyphs.iter().filter(|glyph| glyph.cluster < row.text.len());
                            for glyph in text_glyphs {
                                let middle = glyph.origin_in_lpxs.x + 0.5 * glyph.advance_in_lpxs();
                                assert!(
                                    rects.iter().any(|rect| {
                                        let Rect { origin, size } = rect.rect_in_lpxs;
                                        (origin.x..=origin.x + size.width).contains(&middle)
                                    }),
                                    "at {max_width}",
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pieces_of_a_shaped_line_get_the_glyphs_they_get_on_their_own() {
        let mut layouter = real_font_layouter();
        let mut shaper = Shaper::new(shaper::Settings { cache_size: 0 });
        let get_glyphs = |shaped_text: &ShapedText| {
            shaped_text
                .glyphs
                .iter()
                .map(|glyph| {
                    let advance = glyph.advance_in_ems.to_bits();
                    let offsets = (glyph.offset_in_ems.to_bits(), glyph.y_offset_in_ems.to_bits());
                    (glyph.font.id(), glyph.id, glyph.cluster, glyph.direction, advance, offsets)
                })
                .collect::<Vec<_>>()
        };
        let (mut taken_count, mut shaped_count) = (0, 0);
        for (family, text) in [
            (LATIN_FAMILY, "AVATAR WAVE, To Ty: the office affluent fjord flies (off) 1234 finally."),
            (LATIN_FAMILY, "Thanks, that worked! I'll try the new build tonight and report back."),
            (CJK_FAMILY, "我们明天见（下午三点），请带上笔记本。日本語のテキストも"),
        ] {
            let font_family = layouter.get_or_load_font_family(family.into());
            let get_params = |text: Substr| ShapeParams {
                text,
                fonts: font_family.fonts().into(),
                direction: Direction::Ltr,
                letter_spacing: Ems(0.0),
                word_spacing: Ems(0.0),
                features: Rc::new(Vec::new()),
                diagnostics: Rc::new(font_family.diagnostics().clone()),
            };
            let line: Substr = text.into();
            let shaped_line = shaper.shape_with_pieces(get_params(line.clone())).unwrap();
            // Pieces start and end between any two graphemes, so some of them split kerned pairs.
            let boundaries: Vec<usize> =
                text.grapheme_indices(true).map(|(index, _)| index).chain([text.len()]).collect();
            for (k, &start) in boundaries.iter().enumerate() {
                for &end in &boundaries[k + 1..] {
                    let piece = line.substr(start..end);
                    let Some(taken) = shaped_line.get_piece(&piece, start) else {
                        shaped_count += 1;
                        continue;
                    };
                    let shaped = shaper.get_or_shape(get_params(piece.clone()));
                    assert_eq!(get_glyphs(&taken), get_glyphs(&shaped), "{:?}", piece.as_str());
                    taken_count += 1;
                }
            }
        }
        assert!(taken_count > 0 && shaped_count > 0);
    }
}
