use {
    super::{
        font::GlyphId,
        geom::Rect,
        loader::FontData,
    },
    rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
        cmp::Ordering,
        collections::HashMap,
        fmt,
        hash::{Hash, Hasher},
        marker::PhantomPinned,
        mem,
        pin::Pin,
        rc::Rc,
    },
};

#[derive(Clone, Debug)]
pub struct ParsedFontSource {
    data: FontData,
    index: u32,
}

impl ParsedFontSource {
    pub fn from_data_and_index(data: FontData, index: u32) -> Option<Self> {
        ttf_parser::Face::parse(data.as_slice(), index).ok()?;
        Some(Self { data, index })
    }

    pub fn data(&self) -> &FontData {
        &self.data
    }
}

impl PartialEq for ParsedFontSource {
    fn eq(&self, other: &Self) -> bool {
        self.data_ptr() == other.data_ptr() && self.index == other.index
    }
}

impl Eq for ParsedFontSource {}

impl Hash for ParsedFontSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.data_ptr().hash(state);
        self.index.hash(state);
    }
}

impl ParsedFontSource {
    fn data_ptr(&self) -> usize {
        self.data.as_slice().as_ptr() as usize
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ParsedFontInstanceKey {
    pub source: ParsedFontSource,
    pub variations: CanonicalVariations,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct CanonicalVariations(Vec<CanonicalVariation>);

impl CanonicalVariations {
    pub fn new(variations: &[(u32, f32)]) -> Self {
        let mut canonical: Vec<_> = variations
            .iter()
            .map(|&(tag, value)| CanonicalVariation {
                tag,
                value: VariationValueBits::from(value),
            })
            .collect();
        canonical.sort_by(|a, b| match a.tag.cmp(&b.tag) {
            Ordering::Equal => a.value.cmp(&b.value),
            order => order,
        });
        canonical.dedup();
        Self(canonical)
    }

    fn as_rustybuzz_variations(&self) -> Vec<rustybuzz::Variation> {
        self.0
            .iter()
            .map(|variation| rustybuzz::Variation {
                tag: ttf_parser::Tag::from_bytes(&variation.tag.to_be_bytes()),
                value: variation.value.to_f32(),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VariationValueBits(u32);

impl VariationValueBits {
    pub fn to_f32(self) -> f32 {
        f32::from_bits(self.0)
    }
}

impl From<f32> for VariationValueBits {
    fn from(value: f32) -> Self {
        Self(value.to_bits())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CanonicalVariation {
    tag: u32,
    value: VariationValueBits,
}

#[derive(Debug)]
pub struct ParsedFontInstance(Pin<Box<ParsedFontInstanceInner>>);

impl ParsedFontInstance {
    pub fn from_source_and_variations(
        source: ParsedFontSource,
        variations: CanonicalVariations,
    ) -> Self {
        let rustybuzz_variations = variations.as_rustybuzz_variations();
        let mut inner = Box::pin(ParsedFontInstanceInner {
            source,
            variations,
            ttf_parser_face: None,
            rustybuzz_face: None,
            units_per_em: 0.0,
            ascender_in_ems: 0.0,
            descender_in_ems: 0.0,
            line_gap_in_ems: 0.0,
            glyph_outline_bounds_in_ems: RefCell::new(HashMap::new()),
            _pinned: PhantomPinned,
        });
        unsafe {
            let data: &'static [u8] = mem::transmute(inner.source.data.as_slice());
            let ttf_parser_face =
                ttf_parser::Face::parse(data, inner.source.index).expect("validated font source");
            let mut rustybuzz_face = rustybuzz::Face::from_face(ttf_parser_face.clone());
            if !rustybuzz_variations.is_empty() {
                rustybuzz_face.set_variations(&rustybuzz_variations);
            }
            let units_per_em = ttf_parser_face.units_per_em() as f32;
            let ascender_in_ems = ttf_parser_face.ascender() as f32 / units_per_em;
            let descender_in_ems = ttf_parser_face.descender() as f32 / units_per_em;
            let line_gap_in_ems = ttf_parser_face.line_gap() as f32 / units_per_em;
            let inner_ref = Pin::as_mut(&mut inner).get_unchecked_mut();
            inner_ref.ttf_parser_face = Some(ttf_parser_face);
            inner_ref.rustybuzz_face = Some(rustybuzz_face);
            inner_ref.units_per_em = units_per_em;
            inner_ref.ascender_in_ems = ascender_in_ems;
            inner_ref.descender_in_ems = descender_in_ems;
            inner_ref.line_gap_in_ems = line_gap_in_ems;
        }
        Self(inner)
    }

    pub fn key(&self) -> ParsedFontInstanceKey {
        ParsedFontInstanceKey {
            source: self.0.source.clone(),
            variations: self.0.variations.clone(),
        }
    }

    pub fn data(&self) -> &FontData {
        self.0.source.data()
    }

    pub fn variations(&self) -> &CanonicalVariations {
        &self.0.variations
    }

    pub fn ttf_parser_face(&self) -> &ttf_parser::Face<'_> {
        self.0.ttf_parser_face.as_ref().unwrap()
    }

    pub fn rustybuzz_face(&self) -> &rustybuzz::Face<'_> {
        self.0.rustybuzz_face.as_ref().unwrap()
    }

    pub fn units_per_em(&self) -> f32 {
        self.0.units_per_em
    }

    pub fn ascender_in_ems(&self) -> f32 {
        self.0.ascender_in_ems
    }

    pub fn descender_in_ems(&self) -> f32 {
        self.0.descender_in_ems
    }

    pub fn line_gap_in_ems(&self) -> f32 {
        self.0.line_gap_in_ems
    }

    pub fn cached_glyph_outline_bounds_in_ems(&self, glyph_id: GlyphId) -> Option<Option<Rect<f32>>> {
        self.0
            .glyph_outline_bounds_in_ems
            .borrow()
            .get(&glyph_id)
            .copied()
    }

    pub fn cache_glyph_outline_bounds_in_ems(
        &self,
        glyph_id: GlyphId,
        bounds_in_ems: Option<Rect<f32>>,
    ) {
        self.0
            .glyph_outline_bounds_in_ems
            .borrow_mut()
            .insert(glyph_id, bounds_in_ems);
    }
}

struct ParsedFontInstanceInner {
    source: ParsedFontSource,
    variations: CanonicalVariations,
    ttf_parser_face: Option<ttf_parser::Face<'static>>,
    rustybuzz_face: Option<rustybuzz::Face<'static>>,
    units_per_em: f32,
    ascender_in_ems: f32,
    descender_in_ems: f32,
    line_gap_in_ems: f32,
    glyph_outline_bounds_in_ems: RefCell<HashMap<GlyphId, Option<Rect<f32>>>>,
    _pinned: PhantomPinned,
}

impl fmt::Debug for ParsedFontInstanceInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedFontInstanceInner")
            .field("source", &self.source)
            .field("variations", &self.variations)
            .field("units_per_em", &self.units_per_em)
            .field("ascender_in_ems", &self.ascender_in_ems)
            .field("descender_in_ems", &self.descender_in_ems)
            .field("line_gap_in_ems", &self.line_gap_in_ems)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct FontFace {
    source: ParsedFontSource,
    instance: Rc<ParsedFontInstance>,
}

impl FontFace {
    pub fn new(source: ParsedFontSource, instance: Rc<ParsedFontInstance>) -> Self {
        Self { source, instance }
    }

    pub fn instance(&self) -> &Rc<ParsedFontInstance> {
        &self.instance
    }

    pub fn data(&self) -> &FontData {
        self.source.data()
    }
}
