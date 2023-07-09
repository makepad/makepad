use std::slice;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap: &'a [usize],
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap: &'a [usize],
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap,
        }
    }

    pub fn index_to_position(&self, index: usize, tab_width: usize) -> Position {
        let mut index = 0;
        let mut position = Position::default();
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(token) => {
                    for grapheme in token.text.graphemes() {
                        // TODO
                        index += grapheme.text.len();
                        position.x += grapheme.width(tab_width);
                    }
                }
                WrappedElement::Text(text) => {
                    position.x += text.width(tab_width);
                }
                WrappedElement::Widget(widget) => {
                    position.x += widget.width;
                }
                WrappedElement::Wrap => {
                    position.y += 1;
                    position.x = 0;
                }
            }
        }
        position
    }

    pub fn position_to_index(&self, position: Position, tab_width: usize) -> usize {
        let mut index = 0;
        let mut position = Position::default();
        for wrapped_element in self.wrapped_elements(){ 
            match wrapped_element {
                WrappedElement::Token(token) => {
                    for grapheme in token.text.graphemes() {
                        let width = grapheme.width(tab_width);
                        // TODO
                        index += grapheme.text.len();
                        position.x += width;
                    }
                }
                WrappedElement::Text(text) => {
                    let width = text.width(tab_width);
                    // TODO
                    position.x += width;
                }
                WrappedElement::Widget(widget) => {
                    let width = widget.width;
                    // TODO
                    position.x += widget.width;
                }
                WrappedElement::Wrap => {
                    position.y += 1;
                    position.x = 0;
                }
            }
        }
        index
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
            index: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap: self.wrap,
            index: 0,
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
        let token_info = self.token_infos.next()?;
        let (text_0, text_1) = self.text.split_at(token_info.len);
        self.text = text_1;
        Some(Token {
            text: text_0,
            kind: token_info.kind,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    text: &'a str,
    kind: TokenKind,
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    index: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |(index, _)| *index == (self.index, Affinity::Before))
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(index, _)| *index == self.index)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Text(text));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |(index, _)| *index == (self.index, Affinity::After))
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut len = token.text.len();
        if let Some((index, _)) = self.text_inlays.first() {
            len = len.min(*index - self.index);
        }
        if let Some(((index, _), _)) = self.widget_inlays.first() {
            len = len.min(*index - self.index);
        }
        let token = if len < token.text.len() {
            let (text_0, text_1) = token.text.split_at(len);
            self.token = Some(Token {
                text: text_1,
                kind: token.kind,
            });
            Token {
                text: text_0,
                kind: token.kind,
            }
        } else {
            self.token = self.tokens.next();
            token
        };
        self.index += token.text.len();
        Some(Element::Token(token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(Token<'a>),
    Text(&'a str),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap: &'a [usize],
    index: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(widget));
        }
        if self
            .wrap
            .first()
            .map_or(false, |index| *index == self.index)
        {
            self.wrap = &self.wrap[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(token) => {
                let mut len = token.text.len();
                if let Some(index) = self.wrap.first() {
                    len = len.min(*index - self.index);
                }
                let token = if len < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(len);
                    self.element = Some(Element::Token(Token {
                        text: text_1,
                        kind: token.kind,
                    }));
                    Token {
                        text: text_0,
                        kind: token.kind,
                    }
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.index += token.text.len();
                WrappedElement::Token(token)
            }
            Element::Text(text) => {
                let mut len = text.len();
                if let Some(index) = self.wrap.first() {
                    len = len.min(*index - self.index);
                }
                let text = if len < text.len() {
                    let (text_0, text_1) = text.split_at(len);
                    self.element = Some(Element::Text(text_1));
                    text_0
                } else {
                    self.element = self.elements.next();
                    text
                };
                self.index += text.len();
                WrappedElement::Text(text)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(Token<'a>),
    Text(&'a str),
    Widget(Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub len: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self { len, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    Whitespace,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: usize,
}

impl Widget {
    pub fn new(id: usize, width: usize) -> Self {
        Self { id, width }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Position {
    pub x: usize,
    pub y: usize,
}

impl Position {
    pub fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }
}