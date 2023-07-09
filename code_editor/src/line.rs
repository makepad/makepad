use {
    crate::{
        misc::{Bias, BiasedIndex, VirtualPoint},
        token::TokenInfo,
        Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [(BiasedIndex, Widget)],
    wraps: &'a [usize],
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [(BiasedIndex, Widget)],
        wraps: &'a [usize],
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wraps,
        }
    }

    pub fn compute_virtual_width(&self, tab_width: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_width = 0;
        let mut summed_width = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(token) => {
                    summed_width += token.text.width(tab_width);
                }
                WrappedElement::Text(text) => {
                    summed_width += text.width(tab_width);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_width += widget.width;
                }
                WrappedElement::Wrap => {
                    max_summed_width = max_summed_width.max(summed_width);
                    summed_width = 0;
                }
            }
        }
        max_summed_width.max(summed_width)
    }

    pub fn virtual_height(&self) -> usize {
        self.wraps.len() + 1
    }

    pub fn biased_index_to_virtual_point(
        &self,
        index: BiasedIndex,
        tab_width: usize,
    ) -> VirtualPoint {
        use crate::str::StrExt;

        let mut current_index = 0;
        let mut current_point = VirtualPoint::default();
        if current_index == index.index && index.bias == Bias::Before {
            return current_point;
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(token) => {
                    for grapheme in token.text.graphemes() {
                        if current_index == index.index && index.bias == Bias::After {
                            return current_point;
                        }
                        current_index += grapheme.len();
                        current_point.row += grapheme.width(tab_width);
                        if current_index == index.index && index.bias == Bias::Before {
                            return current_point;
                        }
                    }
                }
                WrappedElement::Text(text) => {
                    current_point.row += text.width(tab_width);
                }
                WrappedElement::Widget(_, widget) => {
                    current_point.row += widget.width;
                }
                WrappedElement::Wrap => {
                    current_point.column += 1;
                    current_point.row = 0;
                }
            }
        }
        if current_index == index.index && index.bias == Bias::After {
            return current_point;
        }
        panic!()
    }

    pub fn virtual_point_to_index(&self, point: VirtualPoint, tab_width: usize) -> BiasedIndex {
        use crate::str::StrExt;

        let mut current_index = 0;
        let mut current_point = VirtualPoint::default();
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(token) => {
                    for grapheme in token.text.graphemes() {
                        let width = grapheme.width(tab_width);
                        if (current_point.row..current_point.row + width / 2).contains(&point.row) {
                            return BiasedIndex::new(current_index, Bias::After);
                        }
                        current_index += grapheme.len();
                        current_point.row += width;
                        if (current_point.row - width / 2..current_point.row).contains(&point.row) {
                            return BiasedIndex::new(current_index, Bias::Before);
                        }
                    }
                }
                WrappedElement::Text(text) => {
                    let width = text.width(tab_width);
                    if (current_point.row..current_point.row + width / 2).contains(&point.row) {
                        return BiasedIndex::new(current_index, Bias::After);
                    }
                    current_index += text.len();
                    current_point.row += width;
                    if (current_point.row - width / 2..current_point.row).contains(&point.row) {
                        return BiasedIndex::new(current_index, Bias::Before);
                    }
                }
                WrappedElement::Widget(affinity, widget) => {
                    if (current_point.row..current_point.row + widget.width).contains(&point.row) {
                        return BiasedIndex::new(current_index, affinity);
                    }
                    current_point.row += widget.width;
                }
                WrappedElement::Wrap => {
                    if current_point.column == point.column {
                        return BiasedIndex::new(current_index, Bias::Before);
                    }
                    current_point.column += 1;
                    current_point.row = 0;
                }
            }
        }
        if current_point.column == point.column {
            return BiasedIndex::new(current_index, Bias::After);
        }
        panic!()
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
            wraps: self.wraps,
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
        Some(Token::new(text_0, token_info.kind))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [(BiasedIndex, Widget)],
    index: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.widget_inlays.first().map_or(false, |(index, _)| {
            *index == BiasedIndex::new(self.index, Bias::Before)
        }) {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Bias::Before, *widget));
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
        if self.widget_inlays.first().map_or(false, |(index, _)| {
            *index == BiasedIndex::new(self.index, Bias::After)
        }) {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Bias::After, *widget));
        }
        let token = self.token.take()?;
        let mut len = token.text.len();
        if let Some((index, _)) = self.text_inlays.first() {
            len = len.min(*index - self.index);
        }
        if let Some((index, _)) = self.widget_inlays.first() {
            len = len.min(index.index - self.index);
        }
        let token = if len < token.text.len() {
            let (text_0, text_1) = token.text.split_at(len);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
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
    Widget(Bias, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wraps: &'a [usize],
    index: usize,
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
            .wraps
            .first()
            .map_or(false, |index| *index == self.index)
        {
            self.wraps = &self.wraps[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(token) => {
                let mut len = token.text.len();
                if let Some(index) = self.wraps.first() {
                    len = len.min(*index - self.index);
                }
                let token = if len < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(len);
                    self.element = Some(Element::Token(Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.index += token.text.len();
                WrappedElement::Token(token)
            }
            Element::Text(text) => {
                let mut len = text.len();
                if let Some(index) = self.wraps.first() {
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
    Token(Token<'a>),
    Text(&'a str),
    Widget(Bias, Widget),
    Wrap,
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
