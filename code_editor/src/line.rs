use {
    crate::{
        token::{TokenInfo, TokenKind},
        Bias, BiasedUsize, Point, Settings, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    settings: &'a Settings,
    text: &'a str,
    token_infos: &'a [TokenInfo],
    inline_text_inlays: &'a [(usize, String)],
    inline_widget_inlays: &'a [((usize, Bias), Widget)],
    soft_breaks: &'a [usize],
    start_column_after_wrap: usize,
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        settings: &'a Settings,
        text: &'a str,
        token_infos: &'a [TokenInfo],
        inline_text_inlays: &'a [(usize, String)],
        inline_widget_inlays: &'a [((usize, Bias), Widget)],
        soft_breaks: &'a [usize],
        start_column_after_wrap: usize,
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            settings,
            text,
            token_infos,
            inline_text_inlays,
            inline_widget_inlays,
            soft_breaks,
            start_column_after_wrap,
            fold_column,
            scale,
        }
    }

    pub fn compute_width(&self) -> usize {
        use crate::str::StrExt;

        let mut max_width = 0;
        let mut width = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    width += token.text.column_count(self.settings.tab_width);
                }
                WrappedElement::Widget(_, widget) => {
                    width += widget.column_count;
                }
                WrappedElement::SoftBreak => {
                    max_width = max_width.max(width);
                    width = self.start_column_after_wrap();
                }
            }
        }
        max_width.max(width)
    }

    pub fn height(&self) -> usize {
        self.soft_breaks.len() + 1
    }

    pub fn compute_scaled_width(&self) -> f64 {
        self.column_to_x(self.compute_width())
    }

    pub fn scaled_height(&self) -> f64 {
        self.scale * self.height() as f64
    }

    pub fn biased_byte_to_point(&self, biased_byte: BiasedUsize) -> Point {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut point = Point::default();
        if biased_byte.value == byte && biased_byte.bias == Bias::Before {
            return point;
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        if biased_byte.value == byte && biased_byte.bias == Bias::After {
                            return point;
                        }
                        byte += grapheme.len();
                        point.column += grapheme.column_count(self.settings.tab_width);
                        if biased_byte.value == byte && biased_byte.bias == Bias::Before {
                            return point;
                        }
                    }
                }
                WrappedElement::Token(true, token) => {
                    point.column += token.text.column_count(self.settings.tab_width);
                }
                WrappedElement::Widget(_, widget) => {
                    point.column += widget.column_count;
                }
                WrappedElement::SoftBreak => {
                    point.row += 1;
                    point.column = self.start_column_after_wrap();
                }
            }
        }
        if biased_byte.value == byte && biased_byte.bias == Bias::After {
            return point;
        }
        panic!()
    }

    pub fn point_to_biased_byte(&self, point: Point) -> BiasedUsize {
        use crate::str::StrExt;

        let mut row = 0;
        let mut column = 0;
        let mut byte = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = column + grapheme.column_count(self.settings.tab_width);
                        if point.row == row && (column..next_column).contains(&point.column) {
                            return BiasedUsize {
                                value: byte,
                                bias: Bias::After,
                            };
                        }
                        byte = byte + grapheme.len();
                        column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = column + token.text.column_count(self.settings.tab_width);
                    if point.row == row && (column..next_column).contains(&point.column) {
                        return BiasedUsize {
                            value: byte,
                            bias: Bias::Before,
                        };
                    }
                    column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::SoftBreak => {
                    if point.row == row {
                        return BiasedUsize {
                            value: byte,
                            bias: Bias::Before,
                        };
                    }
                    row += 1;
                    column = self.start_column_after_wrap();
                }
            }
        }
        if point.row == row {
            return BiasedUsize {
                value: byte,
                bias: Bias::After,
            };
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let unfolded_columns = column.min(self.fold_column);
        let folded_columns = column - unfolded_columns;
        unfolded_columns as f64 + self.scale * folded_columns as f64
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

    pub fn inline_elements(&self) -> InlineElements<'a> {
        let mut tokens = self.tokens();
        InlineElements {
            token: tokens.next(),
            tokens,
            inline_text_inlays: self.inline_text_inlays,
            inline_widget_inlays: self.inline_widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.inline_elements();
        WrappedElements {
            element: elements.next(),
            elements,
            soft_breaks: self.soft_breaks,
            byte: 0,
        }
    }

    pub fn start_column_after_wrap(&self) -> usize {
        self.start_column_after_wrap
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
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
pub struct InlineElements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    inline_text_inlays: &'a [(usize, String)],
    inline_widget_inlays: &'a [((usize, Bias), Widget)],
    byte: usize,
}

impl<'a> Iterator for InlineElements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Bias::Before
            })
        {
            let ((_, widget), inline_widget_inlays) =
                self.inline_widget_inlays.split_first().unwrap();
            self.inline_widget_inlays = inline_widget_inlays;
            return Some(Element::Widget(Bias::Before, *widget));
        }
        if self
            .inline_text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), inline_text_inlays) = self.inline_text_inlays.split_first().unwrap();
            self.inline_text_inlays = inline_text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .inline_widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Bias::After
            })
        {
            let ((_, widget), inline_widget_inlays) =
                self.inline_widget_inlays.split_first().unwrap();
            self.inline_widget_inlays = inline_widget_inlays;
            return Some(Element::Widget(Bias::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.inline_text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.inline_widget_inlays.first() {
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
    Widget(Bias, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: InlineElements<'a>,
    soft_breaks: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Bias::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Bias::Before, widget));
        }
        if self
            .soft_breaks
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.soft_breaks = &self.soft_breaks[1..];
            return Some(WrappedElement::SoftBreak);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.soft_breaks.first() {
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
            Element::Widget(Bias::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Bias::After, widget)
            }
            Element::Widget(Bias::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Bias, Widget),
    SoftBreak,
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
