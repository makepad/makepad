use {
    crate::{inlays::InlineInlay, Line, Settings, Token},
    std::slice::Iter,
};

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub settings: &'a Settings,
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub indent: Iter<'a, usize>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wraps: Iter<'a, Vec<usize>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            settings: self.settings,
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold: *self.fold.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            indent: *self.indent.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wraps: self.wraps.next().unwrap(),
        })
    }
}
