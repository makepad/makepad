use {
    super::{font_family::FontFamilyId, non_nan::NonNanF32, substr::Substr},
    std::{ops::Deref, slice},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Text {
    spans: Vec<Span>,
}

impl Text {
    pub fn push_span(&mut self, span: Span) {
        self.spans.push(span);
    }
}

impl Deref for Text {
    type Target = [Span];

    fn deref(&self) -> &Self::Target {
        &self.spans
    }
}

impl<'a> IntoIterator for &'a Text {
    type Item = &'a Span;
    type IntoIter = slice::Iter<'a, Span>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
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
    pub font_size_in_lpxs: NonNanF32,
}
