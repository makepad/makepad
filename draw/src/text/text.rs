use super::{font_family::FontFamilyId, non_nan::NonNanF32, substr::Substr};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Text {
    spans: Vec<Span>,
}

impl Text {
    pub fn spans(&self) -> &[Span] {
        &self.spans
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
    pub font_size_in_lpxs: NonNanF32,
}
