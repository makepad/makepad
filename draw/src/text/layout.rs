use {
    super::{
        font::{Font, GlyphId, RasterizedGlyph},
        font_atlas::{ColorAtlas, GrayscaleAtlas},
        font_family::FontFamily,
        font_loader::{self, FontDefinitions, FontLoader},
        geom::{Point, Size},
        non_nan::NonNanF32,
        sdfer,
        shape::{self, ShapedText},
        style::{Baseline, Color, Style},
        substr::Substr,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
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
        let mut rows = Vec::new();
        LayoutContext {
            loader: &mut self.font_loader,
            text: &params.text,
            options: params.options,
            start_index: 0,
            current_index: 0,
            current_x_in_lpxs: 0.0,
            glyphs: Vec::new(),
            out_rows: &mut rows,
        }
        .layout(&params.spans);
        LaidoutText { rows }
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
    start_index: usize,
    current_index: usize,
    current_x_in_lpxs: f32,
    glyphs: Vec<LaidoutGlyph>,
    out_rows: &'a mut Vec<LaidoutRow>,
}

impl<'a> LayoutContext<'a> {
    fn max_width_in_lpxs(&self) -> Option<f32> {
        self.options
            .max_width_in_lpxs
            .map(|max_width_in_lpxs| max_width_in_lpxs.into_inner())
    }

    fn remaining_width_on_current_row_in_lpxs(&self) -> Option<f32> {
        self.max_width_in_lpxs()
            .map(|max_width_in_lpxs| max_width_in_lpxs - self.current_x_in_lpxs)
    }

    fn layout(&mut self, spans: &[LayoutSpan]) {
        for span in spans {
            self.layout_span(span);
        }
        self.finish_row();
    }

    fn layout_span(&mut self, span: &LayoutSpan) {
        let font_family = self
            .loader
            .get_or_load_font_family(&span.style.font_family_id)
            .clone();
        if self.options.max_width_in_lpxs.is_some() {
            self.wrap_by_word(
                &font_family,
                span.style.font_size_in_lpxs.into_inner(),
                span.style.color,
                span.style.baseline,
                span.range.clone(),
            );
        } else {
            self.append_text(
                span.style.font_size_in_lpxs.into_inner(),
                span.style.color,
                span.style.baseline,
                &font_family.get_or_shape(self.text.substr(span.range.clone())),
            );
        }
    }

    fn wrap_by_word(
        &mut self,
        font_family: &Rc<FontFamily>,
        font_size_in_lpxs: f32,
        color: Color,
        baseline: Baseline,
        byte_range: Range<usize>,
    ) {
        use unicode_segmentation::UnicodeSegmentation;

        let text = self.text.substr(byte_range.clone());
        let segment_lens = text.split_word_bounds().map(|word| word.len()).collect();
        let mut fitter = Fitter::new(font_family, font_size_in_lpxs, text, segment_lens);
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_on_current_row_in_lpxs().unwrap()) {
                Some(text) => {
                    self.append_text(font_size_in_lpxs, color, baseline, &text);
                }
                None => {
                    if self.glyphs.is_empty() {
                        self.wrap_by_grapheme(
                            font_family,
                            font_size_in_lpxs,
                            color,
                            baseline,
                            0..fitter.pop_front(),
                        );
                    } else {
                        self.finish_row()
                    }
                }
            }
        }
    }

    fn wrap_by_grapheme(
        &mut self,
        font_family: &Rc<FontFamily>,
        font_size_in_lpxs: f32,
        color: Color,
        baseline: Baseline,
        byte_range: Range<usize>,
    ) {
        use unicode_segmentation::UnicodeSegmentation;

        let text = self.text.substr(byte_range.clone());
        let segment_lens = text.split_word_bounds().map(|word| word.len()).collect();
        let mut fitter = Fitter::new(font_family, font_size_in_lpxs, text, segment_lens);
        while !fitter.is_empty() {
            match fitter.fit(self.remaining_width_on_current_row_in_lpxs().unwrap()) {
                Some(text) => {
                    self.append_text(font_size_in_lpxs, color, baseline, &text);
                }
                None => {
                    if self.glyphs.is_empty() {
                        self.append_text(
                            font_size_in_lpxs,
                            color,
                            baseline,
                            &font_family.get_or_shape(self.text.substr(0..fitter.pop_front())),
                        );
                    } else {
                        self.finish_row();
                    }
                }
            }
        }
    }

    fn append_text(
        &mut self,
        font_size_in_lpxs: f32,
        color: Color,
        baseline: Baseline,
        text: &ShapedText,
    ) {
        for glyph in &text.glyphs {
            let glyph = LaidoutGlyph {
                font: glyph.font.clone(),
                font_size_in_lpxs,
                color,
                baseline,
                id: glyph.id,
                cluster: self.current_index + glyph.cluster,
                advance_in_lpxs: glyph.advance_in_ems * font_size_in_lpxs,
                offset_in_lpxs: glyph.offset_in_ems * font_size_in_lpxs,
            };
            self.current_x_in_lpxs += glyph.advance_in_lpxs;
            self.glyphs.push(glyph);
        }
        self.current_index += text.text.len();
    }

    fn finish_row(&mut self) {
        use std::mem;

        let glyphs = mem::take(&mut self.glyphs);
        let row = LaidoutRow {
            text: self.text.substr(self.start_index..self.current_index),
            width_in_lpxs: self.current_x_in_lpxs,
            ascender_in_lpxs: glyphs
                .iter()
                .map(|glyph| glyph.ascender_in_lpxs())
                .reduce(f32::max)
                .unwrap_or(0.0),
            descender_in_lpxs: glyphs
                .iter()
                .map(|glyph| glyph.descender_in_lpxs())
                .reduce(f32::max)
                .unwrap_or(0.0),
            line_gap_in_lpxs: glyphs
                .iter()
                .map(|glyph| glyph.line_gap_in_lpxs())
                .reduce(f32::max)
                .unwrap_or(0.0),
            glyphs,
        };
        self.start_index = self.current_index;
        self.current_x_in_lpxs = 0.0;
        self.out_rows.push(row);
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
    pub spans: Rc<[LayoutSpan]>,
    pub options: LayoutOptions,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutSpan {
    pub style: Style,
    pub range: Range<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct LayoutOptions {
    pub max_width_in_lpxs: Option<NonNanF32>,
}

#[derive(Clone, Debug)]
pub struct LaidoutText {
    pub rows: Vec<LaidoutRow>,
}

impl LaidoutText {
    pub fn walk_rows(
        &self,
        initial_point_in_lpxs: Point<f32>,
        f: impl FnMut(Point<f32>, &LaidoutRow)
    ) {
        let mut current_point_in_lpxs = initial_point_in_lpxs;
        let mut f = f;
        for (row_index, row) in self.rows.iter().enumerate() {
            if row_index != 0 {
                current_point_in_lpxs.y += row.ascender_in_lpxs;
            }
            f(current_point_in_lpxs, row);
            current_point_in_lpxs.y += -row.descender_in_lpxs + row.line_gap_in_lpxs;
        }
    }

    pub fn push_row(&mut self, row: LaidoutRow) {
        self.rows.push(row);
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutRow {
    pub text: Substr,
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub line_gap_in_lpxs: f32,
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    pub fn walk_glyphs(
        &self,
        initial_point_in_lpxs: Point<f32>,
        f: impl FnMut(Point<f32>, &LaidoutGlyph),
    ) {
        let mut current_point_in_lpxs = initial_point_in_lpxs;
        let mut f = f;
        for glyph in &self.glyphs {
            f(current_point_in_lpxs, glyph);
            current_point_in_lpxs.x += glyph.advance_in_lpxs;
        }
    }

    pub fn x_in_lpxs_to_index(&self, x_in_lpxs: f32) -> usize {
        use unicode_segmentation::UnicodeSegmentation;

        fn group_glyphs_by_cluster(
            glyphs: &[LaidoutGlyph],
        ) -> impl Iterator<Item = &[LaidoutGlyph]> + '_ {
            use std::iter;

            let mut start = 0;
            iter::from_fn(move || {
                if start == glyphs.len() {
                    return None;
                }
                let end = glyphs[start..]
                    .windows(2)
                    .position(|window| window[0].cluster != window[1].cluster)
                    .map_or(glyphs.len(), |index| start + index + 1);
                let group = &glyphs[start..end];
                start = end;
                Some(group)
            })
        }

        let mut current_x_in_lpxs = 0.0;
        let mut glyph_groups = group_glyphs_by_cluster(&self.glyphs).peekable();
        while let Some(glyph_group) = glyph_groups.next() {
            let start = glyph_group[0].cluster;
            let end = glyph_groups
                .peek()
                .map_or(self.text.len(), |glyph_group| glyph_group[0].cluster);
            let width_in_lpxs: f32 = glyph_group.iter().map(|glyph| glyph.advance_in_lpxs).sum();
            let grapheme_count = self.text[start..end].graphemes(true).count();
            let width_in_lpxs_per_grapheme = width_in_lpxs / grapheme_count as f32;
            for grapheme_index in 0..grapheme_count {
                if x_in_lpxs < current_x_in_lpxs + width_in_lpxs_per_grapheme {
                    unimplemented!(); // TODO
                }
                current_x_in_lpxs += width_in_lpxs_per_grapheme;
            }
            current_x_in_lpxs += width_in_lpxs_per_grapheme;
        }
        self.text.len()
    }
}

#[derive(Clone, Debug)]
pub struct LaidoutGlyph {
    pub font: Rc<Font>,
    pub font_size_in_lpxs: f32,
    pub color: Color,
    pub baseline: Baseline,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_lpxs: f32,
    pub offset_in_lpxs: f32,
}

impl LaidoutGlyph {
    pub fn ascender_in_lpxs(&self) -> f32 {
        self.font.ascender_in_ems() * self.font_size_in_lpxs
    }

    pub fn descender_in_lpxs(&self) -> f32 {
        self.font.descender_in_ems() * self.font_size_in_lpxs
    }

    pub fn line_gap_in_lpxs(&self) -> f32 {
        self.font.line_gap_in_ems() * self.font_size_in_lpxs
    }

    pub fn baseline_y_in_lpxs(&self) -> f32 {
        match self.baseline {
            Baseline::Alphabetic => 0.0,
            Baseline::Top => self.ascender_in_lpxs(),
            Baseline::Bottom => self.descender_in_lpxs(),
        }
    }

    pub fn rasterize(&self, dpx_per_em: f32) -> Option<RasterizedGlyph> {
        self.font.rasterize_glyph(self.id, dpx_per_em)
    }
}
