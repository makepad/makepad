use {super::loader::FontData, rustybuzz, rustybuzz::ttf_parser};

#[derive(Clone, Debug)]
pub struct FontFace {
    data: FontData,
    index: u32,
    variations: Vec<rustybuzz::Variation>,
}

impl FontFace {
    pub fn from_data_and_index(data: FontData, index: u32) -> Option<Self> {
        // Validate upfront so subsequent parse calls are expected to succeed.
        ttf_parser::Face::parse(data.as_slice(), index).ok()?;
        Some(Self {
            data,
            index,
            variations: Vec::new(),
        })
    }

    pub fn with_ttf_parser_face<R>(&self, f: impl FnOnce(&ttf_parser::Face<'_>) -> R) -> R {
        let face = ttf_parser::Face::parse(self.data.as_slice(), self.index)
            .expect("font face became invalid after initial validation");
        f(&face)
    }

    pub fn with_rustybuzz_face<R>(&self, f: impl FnOnce(&rustybuzz::Face<'_>) -> R) -> R {
        self.with_ttf_parser_face(|ttf_face| {
            let mut rb_face = rustybuzz::Face::from_face(ttf_face.clone());
            if !self.variations.is_empty() {
                rb_face.set_variations(&self.variations);
            }
            f(&rb_face)
        })
    }

    pub fn data(&self) -> &FontData {
        &self.data
    }

    pub fn set_variations(&mut self, variations: &[(u32, f32)]) {
        self.variations.clear();
        self.variations
            .extend(variations.iter().map(|&(tag, value)| rustybuzz::Variation {
                tag: ttf_parser::Tag::from_bytes(&tag.to_be_bytes()),
                value,
            }));
    }
}
