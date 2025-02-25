use {
    super::{
        color::Color,
        font::{Font, GlyphId, RasterizedGlyph},
        font_atlas::{ColorAtlas, GrayscaleAtlas},
        font_family::{FontFamily, FontFamilyId},
        font_loader::{self, FontDefinitions, FontLoader},
        geom::{Point, Size},
        non_nan::NonNanF32,
        sdfer,
        shape::{self, ShapedText},
        substr::Substr,
    }, std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        ops::Range,
        rc::Rc,
    }
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
            .map(|max_width_in_lpxs| max_width_in_lpxs.into_inner() - self.current_point_in_lpxs.x)
    }

    fn layout(mut self, spans: &[Span]) -> LaidoutText {
        for span in spans {
            self.layout_span(span);
        }
        self.finish_current_row();
        LaidoutText {
            size_in_lpxs: Size::new(
                self.rows
                    .iter()
                    .map(|row| row.width_in_lpxs)
                    .reduce(f32::max)
                    .unwrap_or(0.0),
                self.rows
                    .iter()
                    .scan(None, |state: &mut Option<&LaidoutRow>, row| {
                        let prev_row = *state;
                        *state = Some(row);
                        Some((prev_row, row))
                    })
                    .map(|(prev_row, row)| {
                        let line_spacing_in_lpxs =
                            prev_row.map_or(0.0, |prev_row| prev_row.line_spacing_in_lpxs(row));
                        line_spacing_in_lpxs + row.ascender_in_lpxs - row.descender_in_lpxs
                    })
                    .sum(),
            ),
            rows: self.rows,
        }
    }

    fn layout_span(&mut self, span: &Span) {
        let font_family = self
            .loader
            .get_or_load_font_family(&span.style.font_family_id)
            .clone();
        if self.options.max_width_in_lpxs.is_none() {
            self.append_text_to_current_row(
                &span.style,
                &font_family.get_or_shape(self.text.substr(span.range.clone())),
            );
        } else {
            self.wrap_by_word(&font_family, &span.style, span.range.clone());
        }
    }

    fn wrap_by_word(&mut self, font_family: &Rc<FontFamily>, style: &Style, range: Range<usize>) {
        use unicode_segmentation::UnicodeSegmentation;

        let text = self.text.substr(range.clone());
        let segment_lens = text.split_word_bounds().map(|word| word.len()).collect();
        let mut fitter = Fitter::new(
            font_family,
            style.font_size_in_lpxs.into_inner(),
            text,
            segment_lens,
        );
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_on_current_row_in_lpxs().unwrap()) {
                Some(text) => {
                    self.append_text_to_current_row(style, &text);
                }
                None => {
                    if self.glyphs.is_empty() {
                        self.wrap_by_grapheme(font_family, style, 0..fitter.pop_front());
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
        use unicode_segmentation::UnicodeSegmentation;

        let text = self.text.substr(range.clone());
        let segment_lens = text.split_word_bounds().map(|word| word.len()).collect();
        let mut fitter = Fitter::new(
            font_family,
            style.font_size_in_lpxs.into_inner(),
            text,
            segment_lens,
        );
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_on_current_row_in_lpxs().unwrap()) {
                Some(text) => {
                    self.append_text_to_current_row(style, &text);
                }
                None => {
                    if self.glyphs.is_empty() {
                        self.append_text_to_current_row(
                            style,
                            &font_family.get_or_shape(self.text.substr(0..fitter.pop_front())),
                        );
                    } else {
                        self.finish_current_row();
                    }
                }
            }
        }
    }

    fn append_text_to_current_row(&mut self, style: &Style, text: &ShapedText) {
        use super::num::Zero;

        for glyph in &text.glyphs {
            let mut glyph = LaidoutGlyph {
                origin_in_lpxs: Point::ZERO,
                font: glyph.font.clone(),
                font_size_in_lpxs: style.font_size_in_lpxs.into_inner(),
                color: style.color,
                id: glyph.id,
                cluster: self.current_row_end + glyph.cluster,
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
        use {std::mem, super::num::Zero};

        let glyphs = mem::take(&mut self.glyphs);
        let used_width_in_lpxs = glyphs.iter().map(|glyph| glyph.advance_in_lpxs()).sum();
        let width_in_lpxs = self.options.max_width_in_lpxs.map_or(used_width_in_lpxs, NonNanF32::into_inner);
        let mut row = LaidoutRow {
            origin_in_lpxs: Point::ZERO,
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
            text: self
                .text
                .substr(self.current_row_start..self.current_row_end),
            glyphs,
        };
        let line_spacing_in_lpxs = self.rows.last().map_or(0.0, |prev_row| prev_row.line_spacing_in_lpxs(&row));
        self.current_point_in_lpxs.x = 0.0;
        self.current_point_in_lpxs.y += line_spacing_in_lpxs + row.ascender_in_lpxs;
        row.origin_in_lpxs.y = self.current_point_in_lpxs.y;
        self.current_point_in_lpxs.y -= row.descender_in_lpxs;
        self.current_row_start = self.current_row_end;
        self.rows.push(row);
    }
}

#[derive(Debug)]
struct Fitter<'a> {
    font_family: &'a Rc<FontFamily>,
    font_size_in_lpxs: f32,
    text: Substr,
    segment_lens: Vec<usize>,
    text_width_in_lpxs: f32,
    segment_widths_in_lpxs: Vec<f32>,
}

impl<'a> Fitter<'a> {
    fn new(
        font_family: &'a Rc<FontFamily>,
        font_size_in_lpxs: f32,
        text: Substr,
        segment_lens: Vec<usize>,
    ) -> Self {
        let segment_widths_in_lpxs: Vec<_> = segment_lens
            .iter()
            .copied()
            .scan(0, |segment_start, segment_len| {
                let segment_end = *segment_start + segment_len;
                let segment = text.substr(*segment_start..segment_end);
                let segment_width_in_ems = font_family.get_or_shape(segment).width_in_ems;
                let segment_width_in_lpxs = segment_width_in_ems * font_size_in_lpxs;
                *segment_start = segment_end;
                Some(segment_width_in_lpxs)
            })
            .collect();
        Self {
            font_family,
            font_size_in_lpxs,
            text,
            segment_lens,
            text_width_in_lpxs: segment_widths_in_lpxs.iter().sum(),
            segment_widths_in_lpxs,
        }
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn fit(&mut self, max_width_in_lpxs: f32) -> Option<Rc<ShapedText>> {
        let mut remaining_segment_count = self.segment_lens.len();
        let mut remaining_text_len = self.text.len();
        let mut remaining_text_width_in_lpxs = self.text_width_in_lpxs;
        while remaining_segment_count > 0 {
            let remaining_text = self.text.substr(..remaining_text_len);
            if let Some(shaped_text) = self.fit_step(
                max_width_in_lpxs,
                remaining_text,
                remaining_text_width_in_lpxs,
            ) {
                self.text = self.text.substr(remaining_text_len..);
                self.text_width_in_lpxs -= remaining_text_width_in_lpxs;
                self.segment_lens.drain(..remaining_segment_count);
                self.segment_widths_in_lpxs.drain(..remaining_segment_count);
                return Some(shaped_text);
            }
            remaining_segment_count -= 1;
            remaining_text_len -= self.segment_lens[remaining_segment_count];
            remaining_text_width_in_lpxs -= self.segment_widths_in_lpxs[remaining_segment_count];
        }
        None
    }

    fn fit_step(
        &mut self,
        max_width_in_lpxs: f32,
        text: Substr,
        text_width_in_lpxs: f32,
    ) -> Option<Rc<ShapedText>> {
        if 0.5 * text_width_in_lpxs > max_width_in_lpxs {
            return None;
        }
        let shaped_text = self.font_family.get_or_shape(text);
        let shaped_text_width_in_lpxs = shaped_text.width_in_ems * self.font_size_in_lpxs;
        if shaped_text_width_in_lpxs > max_width_in_lpxs {
            return None;
        }
        Some(shaped_text)
    }

    fn pop_front(&mut self) -> usize {
        let segment_len = self.segment_lens[0];
        self.text = self.text.substr(self.segment_lens[0]..);
        self.text_width_in_lpxs -= self.segment_widths_in_lpxs[0];
        self.segment_lens.remove(0);
        self.segment_widths_in_lpxs.remove(0);
        segment_len
    }
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Style {
    pub font_family_id: FontFamilyId,
    pub font_size_in_lpxs: NonNanF32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct LayoutOptions {
    pub max_width_in_lpxs: Option<NonNanF32>,
}

#[derive(Clone, Debug)]
pub struct LaidoutText {
    pub size_in_lpxs: Size<f32>,
    pub rows: Vec<LaidoutRow>,
}

#[derive(Clone, Debug)]
pub struct LaidoutRow {
    pub origin_in_lpxs: Point<f32>,
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub line_gap_in_lpxs: f32,
    pub text: Substr,
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    pub fn line_spacing_in_lpxs(&self, _next_row: &LaidoutRow) -> f32 {
        self.line_gap_in_lpxs
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutGlyph {
    pub origin_in_lpxs: Point<f32>,
    pub font: Rc<Font>,
    pub font_size_in_lpxs: f32,
    pub color: Color,
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
