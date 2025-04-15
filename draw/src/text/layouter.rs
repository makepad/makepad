use {
    super::{
        color::Color,
        font::{Font, FontId, GlyphId},
        font_family::{FontFamily, FontFamilyId},
        geom::{Point, Rect, Size},
        loader::{self, FontDefinition, FontFamilyDefinition, Loader},
        rasterizer::{self, RasterizedGlyph, Rasterizer},
        sdfer,
        selection::{Cursor, CursorPosition, Selection},
        shaper::{self, ShapedText},
        substr::Substr,
    },
    std::{
        borrow::Borrow,
        cell::RefCell,
        collections::{HashMap, VecDeque},
        hash::{Hash, Hasher},
        rc::Rc,
    },
    unicode_segmentation::UnicodeSegmentation,
};

const LPXS_PER_INCH: f32 = 96.0;
const PTS_PER_INCH: f32 = 72.0;

#[derive(Debug)]
pub struct Layouter {
    loader: Loader,
    cache_size: usize,
    cached_params: VecDeque<OwnedLayoutParams>,
    cached_results: HashMap<OwnedLayoutParams, Rc<LaidoutText>>,
}

impl Layouter {
    pub fn new(settings: Settings) -> Self {
        Self {
            loader: Loader::new(settings.loader),
            cache_size: settings.cache_size,
            cached_params: VecDeque::with_capacity(settings.cache_size),
            cached_results: HashMap::with_capacity(settings.cache_size),
        }
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

    pub fn define_font(&mut self, id: FontId, definition: FontDefinition) {
        self.loader.define_font(id, definition);
    }

    pub fn get_or_layout(&mut self, params: impl LayoutParams) -> Rc<LaidoutText> {
        if let Some(result) = self.cached_results.get(&params as &dyn LayoutParams) {
            return result.clone();
        }
        if self.cached_params.len() == self.cache_size {
            let params = self.cached_params.pop_front().unwrap();
            self.cached_results.remove(&params);
        }
        let params = params.to_owned();
        let result = Rc::new(self.layout(params.clone()));
        self.cached_params.push_back(params.clone());
        self.cached_results.insert(params, result.clone());
        result
    }

    fn layout(&mut self, params: OwnedLayoutParams) -> LaidoutText {
        LayoutContext::new(&mut self.loader, params.text, params.options).layout(&params.spans)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub loader: loader::Settings,
    pub cache_size: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            loader: loader::Settings {
                shaper: shaper::Settings { cache_size: 4096 },
                rasterizer: rasterizer::Settings {
                    sdfer: sdfer::Settings {
                        padding: 4,
                        radius: 8.0,
                        cutoff: 0.25,
                    },
                    grayscale_atlas_size: Size::new(4096, 4096),
                    color_atlas_size: Size::new(4096, 4096),
                },
            },
            cache_size: 4096,
        }
    }
}

#[derive(Debug)]
struct LayoutContext<'a> {
    loader: &'a mut Loader,
    text: Substr,
    options: LayoutOptions,
    current_point_in_lpxs: Point<f32>,
    current_row_start: usize,
    current_row_end: usize,
    rows: Vec<LaidoutRow>,
    glyphs: Vec<LaidoutGlyph>,
}

impl<'a> LayoutContext<'a> {
    fn new(loader: &'a mut Loader, text: Substr, options: LayoutOptions) -> Self {
        Self {
            loader,
            text,
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
        self.options
            .wrap_width_in_lpxs
            .map(|wrap_width_in_lpxs| wrap_width_in_lpxs - self.current_point_in_lpxs.x)
    }

    fn layout(mut self, spans: &[Span]) -> LaidoutText {
        for (span_index, span) in spans.iter().enumerate() {
            self.layout_span_multiline(span, span_index == spans.len() - 1);
        }
        self.finish()
    }

    fn layout_span_multiline(&mut self, span: &Span, is_last: bool) {
        let font_family = self
            .loader
            .get_or_load_font_family(span.style.font_family_id)
            .clone();
        for (line_index, len) in self
            .span_text(span.len)
            .split('\n')
            .map(|line| line.len())
            .enumerate()
        {
            if line_index != 0 {
                self.finish_current_row(&font_family, span.style, true);
            }
            self.layout_span(&font_family, span.style, len);
        }
        if is_last {
            self.finish_current_row(&font_family, span.style, false);
        }
    }

    fn layout_span(&mut self, font_family: &Rc<FontFamily>, style: Style, len: usize) {
        if self.remaining_width_in_lpxs().is_none() {
            self.layout_span_directly(font_family, style, len);
        } else {
            self.layout_span_by_word(font_family, style, len);
        }
    }

    fn layout_span_by_word(&mut self, font_family: &Rc<FontFamily>, style: Style, len: usize) {
        let mut fitter = Fitter::new(
            self.span_text(len),
            font_family.clone(),
            style.font_size_in_lpxs(),
            SegmentKind::Word,
        );
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_in_lpxs().unwrap()) {
                Some(text) => self.append_text(style, &text),
                None => {
                    if self.current_row_is_empty() && !self.current_row_is_continuation() {
                        self.layout_span_by_grapheme(font_family, style, fitter.pop());
                    } else {
                        self.finish_current_row(font_family, style, false);
                    }
                }
            }
        }
    }

    fn layout_span_by_grapheme(&mut self, font_family: &Rc<FontFamily>, style: Style, len: usize) {
        let mut fitter = Fitter::new(
            self.span_text(len),
            font_family.clone(),
            style.font_size_in_lpxs(),
            SegmentKind::Grapheme,
        );
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_in_lpxs().unwrap()) {
                Some(text) => self.append_text(style, &text),
                None => {
                    if self.current_row_is_empty() {
                        self.layout_span_directly(font_family, style, fitter.pop());
                    } else {
                        self.finish_current_row(font_family, style, false);
                    }
                }
            }
        }
    }

    fn layout_span_directly(&mut self, font_family: &FontFamily, style: Style, len: usize) {
        self.append_text(
            style,
            &font_family.get_or_shape(
                self.text
                    .substr(self.current_row_end..self.current_row_end + len),
            ),
        );
    }

    fn append_text(&mut self, style: Style, text: &ShapedText) {
        use super::num::Zero;

        for glyph in &text.glyphs {
            let mut glyph = LaidoutGlyph {
                origin_in_lpxs: Point::ZERO,
                font: glyph.font.clone(),
                font_size_in_lpxs: style.font_size_in_lpxs(),
                color: style.color,
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

    fn finish_current_row(
        &mut self,
        fallback_font_family: &FontFamily,
        fallback_style: Style,
        newline: bool,
    ) {
        use {super::num::Zero, std::mem};

        let glyphs = mem::take(&mut self.glyphs);
        let fallback_font = &fallback_font_family.fonts()[0];
        let fallback_font_size_in_lpxs = fallback_style.font_size_in_lpxs();
        let fallback_ascender_in_lpxs =
            fallback_font.ascender_in_ems() * fallback_font_size_in_lpxs;
        let fallback_descender_in_lpxs =
            fallback_font.descender_in_ems() * fallback_font_size_in_lpxs;
        let fallback_line_gap_in_lpxs =
            fallback_font.line_gap_in_ems() * fallback_font_size_in_lpxs;

        let text = self
            .text
            .substr(self.current_row_start..self.current_row_end);
        let width_in_lpxs = self.current_point_in_lpxs.x;
        let ascender_in_lpxs = glyphs
            .iter()
            .map(|glyph| glyph.ascender_in_lpxs())
            .reduce(f32::max)
            .unwrap_or(fallback_ascender_in_lpxs);
        let descender_in_lpxs = glyphs
            .iter()
            .map(|glyph| glyph.descender_in_lpxs())
            .reduce(f32::min)
            .unwrap_or(fallback_descender_in_lpxs);
        let line_gap_in_lpxs = glyphs
            .iter()
            .map(|glyph| glyph.line_gap_in_lpxs())
            .reduce(f32::max)
            .unwrap_or(fallback_line_gap_in_lpxs);
        let line_spacing_scale = self.options.line_spacing_scale;
        let line_spacing_above_in_lpxs = ascender_in_lpxs * line_spacing_scale;
        let line_spacing_below_in_lpxs =
            (-descender_in_lpxs + line_gap_in_lpxs) * line_spacing_scale;
        let line_spacing_below_in_lpxs =
            line_spacing_below_in_lpxs.max(if self.current_row_is_first() {
                self.options.first_row_min_line_spacing_below_in_lpxs
            } else {
                0.0
            });
        let mut row = LaidoutRow {
            origin_in_lpxs: Point::ZERO,
            text,
            newline,
            width_in_lpxs,
            ascender_in_lpxs,
            descender_in_lpxs,
            line_gap_in_lpxs,
            line_spacing_above_in_lpxs,
            line_spacing_below_in_lpxs,
            glyphs,
        };

        self.current_point_in_lpxs.x = 0.0;
        self.current_point_in_lpxs.y += self.rows.last().map_or(row.ascender_in_lpxs, |prev_row| {
            prev_row.line_spacing_in_lpxs(&row)
        });
        let wrap_width_in_lpxs = self.options.wrap_width_in_lpxs.unwrap_or(row.width_in_lpxs);
        let remaining_width_in_lpxs = wrap_width_in_lpxs - row.width_in_lpxs;
        row.origin_in_lpxs.x = self.options.align * remaining_width_in_lpxs;
        row.origin_in_lpxs.y = self.current_point_in_lpxs.y;
        self.current_row_start = self.current_row_end;
        if newline {
            self.current_row_start += 1;
            self.current_row_end += 1;
        }
        self.rows.push(row);
    }

    fn finish(self) -> LaidoutText {
        LaidoutText {
            text: self.text,
            size_in_lpxs: Size::new(
                self.rows
                    .iter()
                    .map(|row| row.width_in_lpxs)
                    .reduce(f32::max)
                    .unwrap_or(0.0),
                self.current_point_in_lpxs.y - self.rows.last().unwrap().descender_in_lpxs,
            ),
            rows: self.rows,
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
        let lens: Vec<_> = match segment_kind {
            SegmentKind::Word => text
                .split_word_bounds()
                .map(|segment| segment.len())
                .collect(),
            SegmentKind::Grapheme => text.graphemes(true).map(|segment| segment.len()).collect(),
        };
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
        if let Some(best_count) = best_count {
            let best_len = self.lens[..best_count].iter().sum();
            let best_text = self.font_family.get_or_shape(self.text.substr(0..best_len));
            self.lens.drain(..best_count);
            self.widths_in_lpxs.drain(..best_count);
            self.text = self.text.substr(best_len..);
            Some(best_text)
        } else {
            None
        }
    }

    fn can_fit(&self, count: usize, wrap_width_in_lpxs: f32) -> bool {
        let len = self.lens[..count].iter().sum();
        let estimated_width_in_lpxs: f32 = self.widths_in_lpxs[..count].iter().sum();
        if 0.5 * estimated_width_in_lpxs > wrap_width_in_lpxs {
            return false;
        }
        let text = self.font_family.get_or_shape(self.text.substr(0..len));
        let actual_width_in_lpxs = text.width_in_ems * self.font_size_in_lpxs;
        if actual_width_in_lpxs > wrap_width_in_lpxs {
            return false;
        }
        true
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

pub trait LayoutParams {
    fn to_owned(self) -> OwnedLayoutParams;
    fn text(&self) -> &str;
    fn spans(&self) -> &[Span];
    fn options(&self) -> LayoutOptions;
}

impl Eq for dyn LayoutParams + '_ {}

impl Hash for dyn LayoutParams + '_ {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.text().hash(hasher);
        self.spans().hash(hasher);
        self.options().hash(hasher);
    }
}

impl PartialEq for dyn LayoutParams + '_ {
    fn eq(&self, other: &Self) -> bool {
        if self.text() != other.text() {
            return false;
        }
        if self.spans() != other.spans() {
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
    pub spans: Rc<[Span]>,
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

    fn spans(&self) -> &[Span] {
        &self.spans
    }

    fn options(&self) -> LayoutOptions {
        self.options
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BorrowedLayoutParams<'a> {
    pub text: &'a str,
    pub spans: &'a [Span],
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
            spans: self.spans.into(),
            options: self.options,
        }
    }

    fn text(&self) -> &str {
        self.text
    }

    fn spans(&self) -> &[Span] {
        self.spans
    }

    fn options(&self) -> LayoutOptions {
        self.options
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub style: Style,
    pub len: usize,
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
    pub first_row_min_line_spacing_below_in_lpxs: f32,
    pub wrap_width_in_lpxs: Option<f32>,
    pub align: f32,
    pub line_spacing_scale: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            first_row_indent_in_lpxs: 0.0,
            first_row_min_line_spacing_below_in_lpxs: 0.0,
            wrap_width_in_lpxs: None,
            align: 0.0,
            line_spacing_scale: 1.0,
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
        self.wrap_width_in_lpxs.map(f32::to_bits).hash(hasher);
        self.align.to_bits().hash(hasher);
        self.line_spacing_scale.to_bits().hash(hasher);
    }
}

impl PartialEq for LayoutOptions {
    fn eq(&self, other: &Self) -> bool {
        if self.first_row_indent_in_lpxs.to_bits() != other.first_row_indent_in_lpxs.to_bits() {
            return false;
        }
        if self.first_row_min_line_spacing_below_in_lpxs.to_bits()
            != other.first_row_min_line_spacing_below_in_lpxs.to_bits()
        {
            return false;
        }
        if self.wrap_width_in_lpxs.map(f32::to_bits) != other.wrap_width_in_lpxs.map(f32::to_bits) {
            return false;
        }
        if self.align != other.align {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutText {
    pub text: Substr,
    pub size_in_lpxs: Size<f32>,
    pub rows: Vec<LaidoutRow>,
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
            if cursor.index == row.text.end_in_parent() {
                if row.newline || !cursor.prefer_next_row {
                    return row_index;
                }
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
            prefer_next_row: if index == 0 { true } else { false },
        }
    }

    pub fn selection_rects_in_lpxs(&self, selection: Selection) -> Vec<Rect<f32>> {
        let CursorPosition {
            row_index: start_row_index,
            x_in_lpxs: start_x_in_lpxs,
        } = self.cursor_to_position(selection.start());
        let CursorPosition {
            row_index: end_row_index,
            x_in_lpxs: end_x_in_lpxs,
        } = self.cursor_to_position(selection.end());
        let mut rects_in_lpxs = Vec::new();
        if start_row_index == end_row_index {
            let row = &self.rows[start_row_index];
            rects_in_lpxs.push(Rect::new(
                Point::new(start_x_in_lpxs, row.origin_in_lpxs.y - row.ascender_in_lpxs),
                Size::new(
                    end_x_in_lpxs - start_x_in_lpxs,
                    row.ascender_in_lpxs - row.descender_in_lpxs,
                ),
            ));
        } else {
            let start_row = &self.rows[start_row_index];
            let end_row = &self.rows[end_row_index];
            rects_in_lpxs.push(Rect::new(
                Point::new(
                    start_x_in_lpxs,
                    start_row.origin_in_lpxs.y - start_row.ascender_in_lpxs,
                ),
                Size::new(
                    start_row.width_in_lpxs - start_x_in_lpxs,
                    start_row.ascender_in_lpxs - start_row.descender_in_lpxs,
                ),
            ));
            for row_index in start_row_index + 1..end_row_index {
                let row = &self.rows[row_index];
                rects_in_lpxs.push(Rect::new(
                    Point::new(
                        row.origin_in_lpxs.x,
                        row.origin_in_lpxs.y - row.ascender_in_lpxs,
                    ),
                    Size::new(
                        row.width_in_lpxs,
                        row.ascender_in_lpxs - row.descender_in_lpxs,
                    ),
                ));
            }
            rects_in_lpxs.push(Rect::new(
                Point::new(0.0, end_row.origin_in_lpxs.y - end_row.ascender_in_lpxs),
                Size::new(
                    end_x_in_lpxs,
                    end_row.ascender_in_lpxs - end_row.descender_in_lpxs,
                ),
            ));
        }
        rects_in_lpxs
    }
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
    pub line_spacing_above_in_lpxs: f32,
    pub line_spacing_below_in_lpxs: f32,
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    pub fn line_spacing_in_lpxs(&self, next_row: &LaidoutRow) -> f32 {
        self.line_spacing_below_in_lpxs + next_row.line_spacing_above_in_lpxs
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
