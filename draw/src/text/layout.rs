use {
    super::{
        color::Color,
        font::{Font, GlyphId, RasterizedGlyph},
        font_atlas::{ColorAtlas, GrayscaleAtlas},
        font_family::{FontFamily, FontFamilyId},
        font_loader::{self, FontDefinitions, FontLoader},
        geom::{Point, Rect, Size},
        sdfer,
        selection::{Affinity, Cursor, Position, Selection},
        shape::{self, ShapedText},
        substr::Substr,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        hash::{Hash, Hasher},
        ops::Range,
        rc::Rc,
    },
};

#[derive(Debug)]
pub struct Layouter {
    font_loader: FontLoader,
    cache_size: usize,
    cached_params: VecDeque<LayoutParams>,
    cached_results: HashMap<LayoutParams, Rc<LaidoutText>>,
}

impl Layouter {
    pub fn new(definitions: FontDefinitions, settings: Settings) -> Self {
        Self {
            font_loader: FontLoader::new(definitions, settings.font_loader),
            cache_size: settings.cache_size,
            cached_params: VecDeque::with_capacity(settings.cache_size),
            cached_results: HashMap::with_capacity(settings.cache_size),
        }
    }

    pub fn sdfer(&self) -> &Rc<RefCell<sdfer::Sdfer>> {
        self.font_loader.sdfer()
    }

    pub fn grayscale_atlas(&self) -> &Rc<RefCell<GrayscaleAtlas>> {
        self.font_loader.grayscale_atlas()
    }

    pub fn color_atlas(&self) -> &Rc<RefCell<ColorAtlas>> {
        self.font_loader.color_atlas()
    }

    pub fn get_or_layout(&mut self, params: LayoutParams) -> Rc<LaidoutText> {
        if !self.cached_results.contains_key(&params) {
            if self.cached_params.len() == self.cache_size {
                let params = self.cached_params.pop_front().unwrap();
                self.cached_results.remove(&params);
            }
            let result = self.layout(params.clone());
            self.cached_params.push_back(params.clone());
            self.cached_results.insert(params.clone(), Rc::new(result));
        }
        self.cached_results.get(&params).unwrap().clone()
    }

    fn layout(&mut self, params: LayoutParams) -> LaidoutText {
        use super::num::Zero;

        LayoutContext {
            loader: &mut self.font_loader,
            text: &params.text,
            options: params.options,
            current_point_in_lpxs: Point::ZERO,
            current_row_start: 0,
            current_row_end: 0,
            rows: Vec::new(),
            glyphs: Vec::new(),
        }
        .layout(&params.spans)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub font_loader: font_loader::Settings,
    pub cache_size: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_loader: font_loader::Settings {
                shaper: shape::Settings { cache_size: 4096 },
                sdfer: sdfer::Settings {
                    padding: 4,
                    radius: 8.0,
                    cutoff: 0.25,
                },
                grayscale_atlas_size: Size::new(512, 512),
                color_atlas_size: Size::new(512, 512),
            },
            cache_size: 4096,
        }
    }
}

#[derive(Debug)]
struct LayoutContext<'a> {
    loader: &'a mut FontLoader,
    text: &'a Substr,
    options: LayoutOptions,
    current_point_in_lpxs: Point<f32>,
    current_row_start: usize,
    current_row_end: usize,
    rows: Vec<LaidoutRow>,
    glyphs: Vec<LaidoutGlyph>,
}

impl<'a> LayoutContext<'a> {
    fn remaining_width_on_current_row_in_lpxs(&self) -> Option<f32> {
        self.options
            .max_width_in_lpxs
            .map(|max_width_in_lpxs| max_width_in_lpxs - self.current_point_in_lpxs.x)
    }

    fn layout(mut self, spans: &[Span]) -> LaidoutText {
        for span in spans {
            self.layout_span(span);
        }
        self.finish_current_row();
        LaidoutText {
            text: self.text.clone(),
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

    fn layout_span(&mut self, span: &Span) {
        let font_family = self
            .loader
            .get_or_load_font_family(span.style.font_family_id)
            .clone();
        if self.options.max_width_in_lpxs.is_none() {
            self.append_shaped_text_to_current_row(
                &span.style,
                &font_family.get_or_shape(self.text.substr(span.range.clone())),
            );
        } else {
            self.wrap_by_word(&font_family, &span.style, span.range.clone());
        }
    }

    fn wrap_by_word(&mut self, font_family: &Rc<FontFamily>, style: &Style, range: Range<usize>) {
        let mut fitter = Fitter::new(
            self.text,
            font_family,
            style.font_size_in_lpxs,
            range.clone(),
            SegmentKind::Word,
        );
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_on_current_row_in_lpxs().unwrap()) {
                Some(text) => {
                    self.append_shaped_text_to_current_row(style, &text);
                }
                None => {
                    if self.glyphs.is_empty() {
                        self.wrap_by_grapheme(font_family, style, fitter.pop_front());
                    } else {
                        self.finish_current_row()
                    }
                }
            }
        }
    }

    fn wrap_by_grapheme(
        &mut self,
        font_family: &Rc<FontFamily>,
        style: &Style,
        range: Range<usize>,
    ) {
        let mut fitter = Fitter::new(
            self.text,
            font_family,
            style.font_size_in_lpxs,
            range,
            SegmentKind::Grapheme,
        );
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_on_current_row_in_lpxs().unwrap()) {
                Some(text) => {
                    self.append_shaped_text_to_current_row(style, &text);
                }
                None => {
                    if self.glyphs.is_empty() {
                        self.append_shaped_text_to_current_row(
                            style,
                            &font_family.get_or_shape(self.text.substr(fitter.pop_front())),
                        );
                    } else {
                        self.finish_current_row();
                    }
                }
            }
        }
    }

    fn append_shaped_text_to_current_row(&mut self, style: &Style, text: &ShapedText) {
        use super::num::Zero;

        for glyph in &text.glyphs {
            let mut glyph = LaidoutGlyph {
                origin_in_lpxs: Point::ZERO,
                font: glyph.font.clone(),
                font_size_in_lpxs: style.font_size_in_lpxs,
                color: style.color,
                id: glyph.id,
                cluster: self.current_row_end - self.current_row_start + glyph.cluster,
                advance_in_ems: glyph.advance_in_ems,
                offset_in_ems: glyph.offset_in_ems,
            };
            glyph.origin_in_lpxs.x = self.current_point_in_lpxs.x;
            self.current_point_in_lpxs.x += glyph.advance_in_lpxs();
            self.glyphs.push(glyph);
        }
        self.current_row_end += text.text.len();
    }

    fn finish_current_row(&mut self) {
        use {super::num::Zero, std::mem};

        let glyphs = mem::take(&mut self.glyphs);
        let width_in_lpxs = self.current_point_in_lpxs.x;
        let mut row = LaidoutRow {
            origin_in_lpxs: Point::ZERO,
            text: self
                .text
                .substr(self.current_row_start..self.current_row_end),
            width_in_lpxs,
            ascender_in_lpxs: glyphs
                .iter()
                .map(|glyph| glyph.ascender_in_lpxs())
                .reduce(f32::max)
                .unwrap_or(0.0),
            descender_in_lpxs: glyphs
                .iter()
                .map(|glyph| glyph.descender_in_lpxs())
                .reduce(f32::min)
                .unwrap_or(0.0),
            line_gap_in_lpxs: glyphs
                .iter()
                .map(|glyph| glyph.line_gap_in_lpxs())
                .reduce(f32::max)
                .unwrap_or(0.0),
            line_spacing_scale: self.options.line_spacing_scale,
            glyphs,
        };
        self.current_point_in_lpxs.x = 0.0;
        self.current_point_in_lpxs.y += self.rows.last().map_or(row.ascender_in_lpxs, |prev_row| {
            prev_row.line_spacing_in_lpxs(&row)
        });
        let max_width_in_lpxs = self.options.max_width_in_lpxs.unwrap_or(width_in_lpxs);
        let remaining_width_in_lpxs = max_width_in_lpxs - width_in_lpxs;
        row.origin_in_lpxs.x = self.options.align * remaining_width_in_lpxs;
        row.origin_in_lpxs.y = self.current_point_in_lpxs.y;
        self.current_row_start = self.current_row_end;
        self.rows.push(row);
    }
}

#[derive(Debug)]
struct Fitter<'a> {
    text: &'a Substr,
    font_family: &'a FontFamily,
    font_size_in_lpxs: f32,
    range: Range<usize>,
    segment_lens: Vec<usize>,
    segment_widths_in_lpxs: Vec<f32>,
    width_in_lpxs: f32,
}

impl<'a> Fitter<'a> {
    fn new(
        text: &'a Substr,
        font_family: &'a FontFamily,
        font_size_in_lpxs: f32,
        range: Range<usize>,
        segment_kind: SegmentKind,
    ) -> Self {
        use unicode_segmentation::UnicodeSegmentation;

        let segment_lens: Vec<_> = match segment_kind {
            SegmentKind::Word => text[range.clone()]
                .split_word_bounds()
                .map(|segment| segment.len())
                .collect(),
            SegmentKind::Grapheme => text[range.clone()]
                .graphemes(true)
                .map(|segment| segment.len())
                .collect(),
        };
        let segment_widths_in_lpxs: Vec<_> = segment_lens
            .iter()
            .copied()
            .scan(range.start, |state, segment_len| {
                let segment_start = *state;
                let segment_end = segment_start + segment_len;
                let segment = text.substr(segment_start..segment_end);
                let segment_width_in_ems = font_family.get_or_shape(segment).width_in_ems;
                let segment_width_in_lpxs = segment_width_in_ems * font_size_in_lpxs;
                *state = segment_end;
                Some(segment_width_in_lpxs)
            })
            .collect();
        let width_in_lpxs = segment_widths_in_lpxs.iter().sum();
        Self {
            text,
            font_family,
            font_size_in_lpxs,
            range,
            width_in_lpxs,
            segment_lens,
            segment_widths_in_lpxs,
        }
    }

    fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    fn fit(&mut self, max_width_in_lpxs: f32) -> Option<Rc<ShapedText>> {
        let mut remaining_len = self.range.len();
        let mut remaining_width_in_lpxs = self.width_in_lpxs;
        let mut remaining_segment_count = self.segment_lens.len();
        while remaining_segment_count > 0 {
            if let Some(text) =
                self.fit_step(max_width_in_lpxs, remaining_len, remaining_width_in_lpxs)
            {
                self.range.start += remaining_len;
                self.width_in_lpxs -= remaining_width_in_lpxs;
                self.segment_lens.drain(..remaining_segment_count);
                self.segment_widths_in_lpxs.drain(..remaining_segment_count);
                return Some(text);
            }
            remaining_segment_count -= 1;
            remaining_len -= self.segment_lens[remaining_segment_count];
            remaining_width_in_lpxs -= self.segment_widths_in_lpxs[remaining_segment_count];
        }
        None
    }

    fn fit_step(
        &mut self,
        max_width_in_lpxs: f32,
        len: usize,
        width_in_lpxs: f32,
    ) -> Option<Rc<ShapedText>> {
        if 0.5 * width_in_lpxs > max_width_in_lpxs {
            return None;
        }
        let text = self.text.substr(self.range.start..self.range.start + len);
        let shaped_text = self.font_family.get_or_shape(text);
        let shaped_text_width_in_lpxs = shaped_text.width_in_ems * self.font_size_in_lpxs;
        if shaped_text_width_in_lpxs > max_width_in_lpxs {
            return None;
        }
        Some(shaped_text)
    }

    fn pop_front(&mut self) -> Range<usize> {
        let len = self.segment_lens[0];
        let range = self.range.start..self.range.start + len;
        self.range.start += len;
        self.width_in_lpxs -= self.segment_widths_in_lpxs[0];
        self.segment_lens.remove(0);
        self.segment_widths_in_lpxs.remove(0);
        range
    }
}

#[derive(Clone, Copy, Debug)]
enum SegmentKind {
    Word,
    Grapheme,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutParams {
    pub text: Substr,
    pub spans: Rc<[Span]>,
    pub options: LayoutOptions,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub style: Style,
    pub range: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Style {
    pub font_family_id: FontFamilyId,
    pub font_size_in_lpxs: f32,
    pub color: Option<Color>,
}

impl Eq for Style {}

impl Hash for Style {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.font_family_id.hash(hasher);
        self.font_size_in_lpxs.to_bits().hash(hasher);
        self.color.hash(hasher);
    }
}

impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        if self.font_family_id != other.font_family_id {
            return false;
        }
        if self.font_size_in_lpxs.to_bits() != other.font_size_in_lpxs.to_bits() {
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
    pub max_width_in_lpxs: Option<f32>,
    pub align: f32,
    pub line_spacing_scale: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            max_width_in_lpxs: None,
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
        self.max_width_in_lpxs.map(f32::to_bits).hash(hasher);
        self.align.to_bits().hash(hasher);
        self.line_spacing_scale.to_bits().hash(hasher);
    }
}

impl PartialEq for LayoutOptions {
    fn eq(&self, other: &Self) -> bool {
        if self.max_width_in_lpxs.map(f32::to_bits) != other.max_width_in_lpxs.map(f32::to_bits) {
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
    pub fn cursor_to_position(&self, cursor: Cursor) -> Position {
        let row_index = self.cursor_to_row_index(cursor);
        let row = &self.rows[row_index];
        let x_in_lpxs = row.index_to_x_in_lpxs(cursor.index - row.text.start_in_parent());
        Position {
            row_index,
            x_in_lpxs,
        }
    }

    fn cursor_to_row_index(&self, cursor: Cursor) -> usize {
        for (row_index, row) in self.rows.iter().enumerate() {
            let row_end = row.text.start_in_parent() + row.text.as_str().len();
            if match cursor.affinity {
                Affinity::Before => cursor.index <= row_end,
                Affinity::After => cursor.index < row_end,
            } {
                return row_index;
            }
        }
        self.rows.len() - 1
    }

    pub fn point_in_lpxs_to_cursor(&self, point_in_lpxs: Point<f32>) -> Cursor {
        let row_index = self.y_in_lpxs_to_row_index(point_in_lpxs.y);
        self.position_to_cursor(Position {
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

    pub fn position_to_cursor(&self, position: Position) -> Cursor {
        let row = &self.rows[position.row_index];
        let index = row.x_in_lpxs_to_index(position.x_in_lpxs);
        Cursor {
            index: row.text.start_in_parent() + index,
            affinity: if index == 0 {
                Affinity::After
            } else {
                Affinity::Before
            },
        }
    }

    pub fn selection_rects_in_lpxs(&self, selection: Selection) -> Vec<Rect<f32>> {
        let Position {
            row_index: start_row_index,
            x_in_lpxs: start_x_in_lpxs,
        } = self.cursor_to_position(selection.start());
        let Position {
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
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub line_gap_in_lpxs: f32,
    pub line_spacing_scale: f32,
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    pub fn line_spacing_in_lpxs(&self, next_row: &LaidoutRow) -> f32 {
        self.line_spacing_below_in_lpxs() + next_row.line_spacing_above_in_lpxs()
    }

    pub fn line_spacing_above_in_lpxs(&self) -> f32 {
        self.ascender_in_lpxs * self.line_spacing_scale
    }

    pub fn line_spacing_below_in_lpxs(&self) -> f32 {
        (-self.descender_in_lpxs + self.line_gap_in_lpxs) * self.line_spacing_scale
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
