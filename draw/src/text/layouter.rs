use {
    super::{
        font::{AllocatedGlyph, Font, GlyphId},
        font_family::{FontFamily, FontFamilyId},
        font_loader::{FontDefinitions, FontLoader},
        geometry::Point,
        image_atlas::{ColorAtlas, GrayscaleAtlas},
        non_nan::NonNanF32,
        numeric::Zero,
        shaper::ShapedGlyph,
        substr::Substr,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        fmt,
        rc::Rc,
    },
};

const CACHE_SIZE: usize = 1024;

#[derive(Debug)]
pub struct Layouter {
    loader: FontLoader,
    cache_size: usize,
    cached_layout_params: VecDeque<LayoutParams>,
    cached_laidout_rows: HashMap<LayoutParams, Rc<Vec<LaidoutRow>>>,
}

impl Layouter {
    pub fn new(definitions: FontDefinitions) -> Self {
        Self {
            loader: FontLoader::new(definitions),
            cache_size: CACHE_SIZE,
            cached_layout_params: VecDeque::with_capacity(CACHE_SIZE),
            cached_laidout_rows: HashMap::with_capacity(CACHE_SIZE),
        }
    }

    pub fn grayscale_atlas(&self) -> &Rc<RefCell<GrayscaleAtlas>> {
        self.loader.grayscale_atlas()
    }

    pub fn color_atlas(&self) -> &Rc<RefCell<ColorAtlas>> {
        self.loader.color_atlas()
    }

    // TODO: Remove
    pub fn get_or_load_font_family(&mut self, font_family_id: &FontFamilyId) -> &Rc<FontFamily> {
        self.loader.get_or_load_font_family(font_family_id)
    }

    pub fn get_or_layout(&mut self, params: &LayoutParams) -> Rc<Vec<LaidoutRow>> {
        if !self.cached_laidout_rows.contains_key(params) {
            if self.cached_layout_params.len() == self.cache_size {
                let params = self.cached_layout_params.pop_front().unwrap();
                self.cached_laidout_rows.remove(&params);
            }
            let result = self.layout(params);
            self.cached_layout_params.push_back(params.clone());
            self.cached_laidout_rows
                .insert(params.clone(), Rc::new(result));
        }
        self.cached_laidout_rows.get(params).unwrap().clone()
    }

    fn layout(&mut self, params: &LayoutParams) -> Vec<LaidoutRow> {
        let mut rows = Vec::new();
        LayoutContext {
            loader: &mut self.loader,
            settings: &params.settings,
            current_point_in_lpxs: Point::ZERO,
            current_row: LaidoutRow::default(),
            output: &mut rows,
        }
        .layout(&params.spans);
        rows
    }
}

#[derive(Debug)]
struct LayoutContext<'a> {
    loader: &'a mut FontLoader,
    settings: &'a LayoutSettings,
    current_point_in_lpxs: Point<f32>,
    current_row: LaidoutRow,
    output: &'a mut Vec<LaidoutRow>,
}

impl<'a> LayoutContext<'a> {
    fn layout(&mut self, spans: &[LayoutSpan]) {
        for span in spans {
            self.layout_span(span);
        }
    }

    fn layout_span(&mut self, span: &LayoutSpan) {
        let font_family = self
            .loader
            .get_or_load_font_family(&span.style.font_family_id)
            .clone();
        for glyph in &*font_family.get_or_shape(span.text.clone()) {
            self.push_glyph_to_current_row(span.style.font_size_in_lpxs.into_inner(), &glyph);
        }
        self.finish_current_row();
    }

    fn push_glyph_to_current_row(&mut self, font_size_in_lpxs: f32, glyph: &ShapedGlyph) {
        let advance_in_lpxs = glyph.advance_in_ems * font_size_in_lpxs;
        let offset_in_lpxs = glyph.offset_in_ems * font_size_in_lpxs;
        self.current_row.push_glyph(LaidoutGlyph {
            font: glyph.font.clone(),
            font_size_in_lpxs,
            id: glyph.id,
            advance_in_lpxs,
            offset_in_lpxs,
        });
        self.current_point_in_lpxs.x += advance_in_lpxs;
    }

    fn finish_current_row(&mut self) {
        use std::mem;

        let height_in_lpxs = self.current_row.height_in_lpxs();
        self.output.push(mem::take(&mut self.current_row));
        self.current_point_in_lpxs.x = 0.0;
        self.current_point_in_lpxs.y += height_in_lpxs;
    }
}

/*
    fn remaining_width_in_lpxs(&self) -> f32 {
        self.settings.max_width_in_lpxs.into_inner() - self.current_point_in_lpxs.x
    }

    fn layout_span(&mut self, span: &Span) {
        let font_family = self
            .layouter
            .loader
            .get_or_load_font_family(&span.style.font_family_id)
            .clone();
        self.layout_by_word(
            &font_family,
            span.style.font_size_in_lpxs.into_inner(),
            &span.text,
        );
    }

    fn layout_by_word(
        &mut self,
        font_family: &Rc<FontFamily>,
        font_size_in_lpxs: f32,
        text: &Substr,
    ) {
        let mut fitter = Fitter::new(font_family.clone(), font_size_in_lpxs, text.clone());
        let mut already_failed_before = false;
        let mut count = 0;
        while let Some(shaped_text) = fitter.fit(self.remaining_width_in_lpxs()) {
            count += 1;
            if count == 10 {
                panic!();
            }
            match shaped_text {
                Ok(shaped_text) => {
                    already_failed_before = false;
                    self.extend_current_row(shaped_text);
                }
                Err(FitError) => {
                    assert!(!already_failed_before);
                    already_failed_before = true;
                    self.finish_current_row()
                }
            }
        }
    }

    fn extend_current_row(&mut self, text: Rc<ShapeResult>) {
        self.current_row
            .glyphs
            .extend(text.glyphs.iter().map(|glyph| Glyph { id: glyph.id }));
    }

    fn finish_current_row(&mut self) {
        use std::mem;

        self.output.push(mem::take(&mut self.current_row));
        self.current_point_in_lpxs.x = 0.0;
    }
}

#[derive(Debug)]
struct Fitter {
    font_family: Rc<FontFamily>,
    font_size_in_lpxs: f32,
    text: Substr,
    summed_word_lens: Vec<usize>,
    summed_word_widths_in_lpxs: Vec<f32>,
    first_word_index: usize,
}

impl Fitter {
    fn new(font_family: Rc<FontFamily>, font_size_in_lpxs: f32, text: Substr) -> Self {
        use unicode_segmentation::UnicodeSegmentation;

        let words = text
            .split_word_bound_indices()
            .map(|(word_start, word)| text.substr(word_start..word_start + word.len()));
        let word_lens = words.clone().map(|word| word.len());
        let summed_word_lens: Vec<_> = word_lens
            .scan(0, |summed_word_len, word_len| {
                *summed_word_len += word_len;
                Some(*summed_word_len)
            })
            .collect();
        let word_widths_in_lpxs =
            words.map(|word| font_family.compute_text_width_in_ems(word) * font_size_in_lpxs);
        let summed_word_widths_in_lpxs: Vec<_> = word_widths_in_lpxs
            .scan(0f32, |summed_word_width_in_lpxs, word_width_in_lpxs| {
                *summed_word_width_in_lpxs += word_width_in_lpxs;
                Some(*summed_word_width_in_lpxs)
            })
            .collect();
        Self {
            font_family,
            font_size_in_lpxs,
            text,
            summed_word_lens,
            summed_word_widths_in_lpxs,
            first_word_index: 0,
        }
    }

    fn summed_word_len_before_current_word(&self) -> usize {
        if self.first_word_index == 0 {
            0
        } else {
            self.summed_word_lens[self.first_word_index - 1]
        }
    }

    fn summed_word_width_in_lpxs_before_current_word(&self) -> f32 {
        if self.first_word_index == 0 {
            0.0
        } else {
            self.summed_word_widths_in_lpxs[self.first_word_index - 1]
        }
    }

    fn text_after_current_word(&self) -> Substr {
        self.text
            .substr(self.summed_word_len_before_current_word()..)
    }

    fn summed_word_lens_after_current_word(
        &self,
    ) -> impl DoubleEndedIterator<Item = usize> + ExactSizeIterator<Item = usize> + '_ {
        self.summed_word_lens[self.first_word_index..]
            .iter()
            .copied()
            .map(|summed_word_len| summed_word_len - self.summed_word_len_before_current_word())
    }

    fn summed_word_widths_in_lpxs_after_current_word(
        &self,
    ) -> impl DoubleEndedIterator<Item = f32> + ExactSizeIterator<Item = f32> + '_ {
        self.summed_word_widths_in_lpxs[self.first_word_index..]
            .iter()
            .copied()
            .map(|summed_word_width_in_lpxs| {
                summed_word_width_in_lpxs - self.summed_word_width_in_lpxs_before_current_word()
            })
    }

    fn fit(&mut self, max_width_in_lpxs: f32) -> Option<Result<Rc<ShapeResult>, FitError>> {
        if self.text_after_current_word().is_empty() {
            return None;
        }
        Some(self.fit_inner(max_width_in_lpxs))
    }

    fn fit_inner(&mut self, max_width_in_lpxs: f32) -> Result<Rc<ShapeResult>, FitError> {
        // println!("Max width: {:?}", max_width_in_lpxs);
        let (last_word_index, shape_output) = self
            .summed_word_lens_after_current_word()
            .zip(self.summed_word_widths_in_lpxs_after_current_word())
            .enumerate()
            .rev()
            .find_map(
                |(
                    last_word_index,
                    (
                        summed_word_len_after_current_word,
                        summed_word_width_in_lpxs_after_current_word,
                    ),
                )| {
                    let estimated_width_in_lpxs =
                        0.5 * summed_word_width_in_lpxs_after_current_word;
                    // println!("Estimated width: {:?}", estimated_width_in_lpxs);
                    if estimated_width_in_lpxs > max_width_in_lpxs {
                        return None;
                    }
                    let shaped_text = self.font_family.get_or_shape(
                        self.text_after_current_word()
                            .substr(..summed_word_len_after_current_word),
                    );
                    let actual_width_in_lpxs: f32 =
                        shaped_text.width_in_ems() * self.font_size_in_lpxs;
                    // println!("Actual width: {:?}", actual_width_in_lpxs);
                    if actual_width_in_lpxs > max_width_in_lpxs {
                        return None;f
                    }
                    println!("Found a fit!");
                    println!("First word index: {:?}", self.first_word_index);
                    println!("Last word index: {:?}", last_word_index);
                    Some((last_word_index, shaped_text))
                },
            )
            .ok_or(FitError)?;
        self.first_word_index = last_word_index + 1;
        Ok(shape_output)
    }
}

#[derive(Clone, Debug)]
pub struct FitError;
*/

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutParams {
    pub settings: LayoutSettings,
    pub spans: Vec<LayoutSpan>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutSettings {
    pub max_width_in_lpxs: NonNanF32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutSpan {
    pub style: LayoutStyle,
    pub text: Substr,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutStyle {
    pub font_family_id: FontFamilyId,
    pub font_size_in_lpxs: NonNanF32,
}

#[derive(Clone, Debug)]
pub struct LaidoutRow {
    pub glyphs: Vec<LaidoutGlyph>,
}

impl LaidoutRow {
    pub fn height_in_lpxs(&self) -> f32 {
        self.glyphs
            .iter()
            .map(|glyph| glyph.height_in_lpxs())
            .fold(0.0, |height_in_lpxs, glyph_height_in_lpxs| {
                height_in_lpxs.max(glyph_height_in_lpxs)
            })
    }

    pub fn push_glyph(&mut self, glyph: LaidoutGlyph) {
        self.glyphs.push(glyph);
    }
}

impl Default for LaidoutRow {
    fn default() -> Self {
        Self { glyphs: Vec::new() }
    }
}

#[derive(Clone)]
pub struct LaidoutGlyph {
    pub font: Rc<Font>,
    pub font_size_in_lpxs: f32,
    pub id: GlyphId,
    pub advance_in_lpxs: f32,
    pub offset_in_lpxs: f32,
}

impl LaidoutGlyph {
    pub fn height_in_lpxs(&self) -> f32 {
        self.font.line_height_in_ems() * self.font_size_in_lpxs
    }

    pub fn allocate(&self, dpx_per_em: f32) -> Option<AllocatedGlyph> {
        self.font.allocate_glyph(self.id, dpx_per_em)
    }
}

impl fmt::Debug for LaidoutGlyph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaidoutGlyph")
            .field("font", &self.font.id())
            .field("font_size_in_lpxs", &self.font_size_in_lpxs)
            .field("id", &self.id)
            .field("advance_in_lpxs", &self.advance_in_lpxs)
            .field("offset_in_lpxs", &self.offset_in_lpxs)
            .finish()
    }
}
