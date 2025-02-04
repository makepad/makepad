use {
    super::{
        atlas::{ColorAtlas, GrayscaleAtlas},
        font_family::{FontFamily, FontFamilyId},
        loader::{Definitions, Loader},
        non_nan::NonNan,
        substr::Substr,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        rc::Rc,
    },
};

const CACHE_SIZE: usize = 256;

#[derive(Debug)]
pub struct Layouter {
    loader: Loader,
    reusable_word_widths_in_lpxs: Vec<f32>,
    cache_size: usize,
    cached_inputs: VecDeque<LayoutInput>,
    cached_outputs: HashMap<LayoutInput, Rc<LayoutOutput>>,
}

impl Layouter {
    pub fn new(definitions: Definitions) -> Self {
        Self {
            loader: Loader::new(definitions),
            reusable_word_widths_in_lpxs: Vec::new(),
            cache_size: CACHE_SIZE,
            cached_inputs: VecDeque::with_capacity(CACHE_SIZE),
            cached_outputs: HashMap::with_capacity(CACHE_SIZE),
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

    pub fn get_or_layout(&mut self, input: &LayoutInput) -> Rc<LayoutOutput> {
        if !self.cached_outputs.contains_key(input) {
            if self.cached_inputs.len() == self.cache_size {
                let params = self.cached_inputs.pop_front().unwrap();
                self.cached_outputs.remove(&params);
            }
            let result = self.layout(input);
            self.cached_inputs.push_back(input.clone());
            self.cached_outputs.insert(input.clone(), Rc::new(result));
        }
        self.cached_outputs.get(input).unwrap().clone()
    }

    fn layout(&mut self, input: &LayoutInput) -> LayoutOutput {
        LayoutContext {
            layouter: self,
            settings: &input.settings,
        }
        .layout_paragraph(&input.paragraph);
        LayoutOutput {}
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutInput {
    pub settings: LayoutSettings,
    pub paragraph: Paragraph,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutSettings {
    pub max_width_in_lpxs: NonNan<f32>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Paragraph {
    lines: Vec<Line>,
}

impl Paragraph {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    pub fn push_line(&mut self, line: Line) {
        self.lines.push(line);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Line {
    spans: Vec<Span>,
}

impl Line {
    pub fn new() -> Self {
        Self { spans: Vec::new() }
    }

    pub fn push_span(&mut self, span: Span) {
        self.spans.push(span);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub style: Style,
    pub text: Substr,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Style {
    pub font_family_id: FontFamilyId,
    pub font_size_in_lpxs: NonNan<f32>,
}

#[derive(Clone, Debug)]
pub struct LayoutOutput {}

#[derive(Debug)]
struct LayoutContext<'a> {
    layouter: &'a mut Layouter,
    settings: &'a LayoutSettings,
}

impl<'a> LayoutContext<'a> {
    fn layout_paragraph(&mut self, paragraph: &Paragraph) {
        for line in &paragraph.lines {
            self.layout_line(line);
        }
    }

    fn layout_line(&mut self, line: &Line) {
        for span in &line.spans {
            self.layout_span(span);
        }
    }

    fn layout_span(&mut self, span: &Span) {
        use {std::mem, unicode_segmentation::UnicodeSegmentation};

        fn split_words(text: &Substr) -> impl Iterator<Item = Substr> + '_ {
            text.split_word_bound_indices()
                .map(|(word_start, word)| text.substr(word_start..word_start + word.len()))
        }

        fn compute_word_width_in_lpxs(
            font_family: &Rc<FontFamily>,
            font_size_in_lpxs: f32,
            word: Substr,
        ) -> f32 {
            font_family
                .get_or_shape(word)
                .glyphs
                .iter()
                .map(|glyph| glyph.advance_in_ems * font_size_in_lpxs)
                .sum::<f32>()
        }

        let font_family = self
            .layouter
            .loader
            .get_or_load_font_family(&span.style.font_family_id);
        let mut word_widths_in_lpxs = mem::take(&mut self.layouter.reusable_word_widths_in_lpxs);
        word_widths_in_lpxs.extend(split_words(&span.text).map(|word| {
            compute_word_width_in_lpxs(font_family, span.style.font_size_in_lpxs.into_inner(), word)
        }));
        let mut estimated_text_width_in_lpxs = word_widths_in_lpxs
            .iter()
            .map(|word_width| word_width)
            .sum::<f32>();
        // TODO: Shape the entire span, using the word widths in lpxs we computed above to
        // estimate the width of the text up to potential break points
        word_widths_in_lpxs.clear();
        self.layouter.reusable_word_widths_in_lpxs = word_widths_in_lpxs;
    }
}
