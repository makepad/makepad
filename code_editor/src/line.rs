use {
    crate::{
        token::{TokenInfo, TokenKind},
        Affinity, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap_bytes: &'a [usize],
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap_bytes: &'a [usize],
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap_bytes,
            fold_column,
            scale,
        }
    }

    pub fn compute_column_count(&self, tab_column_count: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_column_count = 0;
        let mut summed_column_count = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    summed_column_count += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_column_count += widget.column_count;
                }
                WrappedElement::Wrap => {
                    max_summed_column_count = max_summed_column_count.max(summed_column_count);
                    summed_column_count = 0;
                }
            }
        }
        max_summed_column_count.max(summed_column_count)
    }

    pub fn row_count(&self) -> usize {
        self.wrap_bytes.len() + 1
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        self.column_to_x(self.compute_column_count(tab_column_count))
    }

    pub fn height(&self) -> f64 {
        self.scale * self.row_count() as f64
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold_column = column.min(self.fold_column);
        let column_count_after_fold_column = column - column_count_before_fold_column;
        column_count_before_fold_column as f64 + self.scale * column_count_after_fold_column as f64
    }

    pub fn byte_affinity_to_row_column(
        &self,
        (byte, affinity): (usize, Affinity),
        tab_column_count: usize,
    ) -> (usize, usize) {
        use crate::str::StrExt;

        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if byte == current_byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(is_inlay, token) => {
                    if is_inlay {
                        column += token.text.column_count(tab_column_count);
                    } else {
                        for grapheme in token.text.graphemes() {
                            if byte == current_byte && affinity == Affinity::After {
                                return (row, column);
                            }
                            current_byte += grapheme.len();
                            column += grapheme.column_count(tab_column_count);
                            if byte == current_byte && affinity == Affinity::Before {
                                return (row, column);
                            }
                        }
                    }
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    column += 1;
                    row = 0;
                }
            }
        }
        if byte == current_byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_column_to_byte_affinity(
        &self,
        (row, column): (usize, usize),
        tab_column_count: usize,
    ) -> (usize, Affinity) {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut current_row = 0;
        let mut current_column = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_column_count);
                        if (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte = byte + grapheme.len();
                        current_column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = column + token.text.column_count(tab_column_count);
                    if (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    current_column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_column == column {
                        return (byte, Affinity::Before);
                    }
                    current_column += 1;
                    current_row = 0;
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn tokens(&self) -> Tokens<'a> {
        Tokens {
            text: self.text,
            token_infos: self.token_infos.iter(),
        }
    }

    pub fn elements(&self) -> Elements<'a> {
        let mut tokens = self.tokens();
        Elements {
            token: tokens.next(),
            tokens,
            text_inlays: self.text_inlays,
            widget_inlays: self.widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap_bytes: self.wrap_bytes,
            byte: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    token_infos: slice::Iter<'a, TokenInfo>,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.token_infos.next() {
            Some(token_info) => {
                let (text_0, text_1) = self.text.split_at(token_info.byte_count);
                self.text = text_1;
                Token::new(text_0, token_info.kind)
            }
            None => {
                if self.text.is_empty() {
                    return None;
                }
                let text = self.text;
                self.text = "";
                Token::new(text, TokenKind::Unknown)
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    byte: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, affinity), _)| {
                *byte == self.byte && *affinity == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, affinity), _)| {
                *byte == self.byte && *affinity == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.widget_inlays.first() {
            byte_count = byte_count.min(byte - self.byte);
        }
        let token = if byte_count < token.text.len() {
            let (text_0, text_1) = token.text.split_at(byte_count);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
        } else {
            self.token = self.tokens.next();
            token
        };
        self.byte += token.text.len();
        Some(Element::Token(false, token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap_bytes: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Affinity::Before, widget));
        }
        if self
            .wrap_bytes
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.wrap_bytes = &self.wrap_bytes[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.wrap_bytes.first() {
                    byte_count = byte_count.min(*byte - self.byte);
                }
                let token = if byte_count < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(byte_count);
                    self.element = Some(Element::Token(is_inlay, Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.byte += token.text.len();
                WrappedElement::Token(is_inlay, token)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Affinity::After, widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub column_count: usize,
}

impl Widget {
    pub fn new(id: usize, column_count: usize) -> Self {
        Self { id, column_count }
    }
}
