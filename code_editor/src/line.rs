use {
    crate::{char::CharExt, inlays::InlineInlay, str::StrExt, widgets::InlineWidget, Token},
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column_index: usize,
    pub scale: f64,
    pub indent_column_count_after_wrap: usize,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays_by_byte_index: &'a [(usize, InlineInlay)],
    pub wrap_byte_indices: &'a [usize],
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_byte_indices.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_index_to_x(self.column_count())
    }

    pub fn column_index_to_x(&self, column_index: usize) -> f64 {
        let column_count_before_fold = column_index.min(self.fold_column_index);
        let column_count_after_fold = column_index - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column_index(&self) -> usize {
        self.fold_column_index
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn indent_column_count_after_wrap(self) -> usize {
        self.indent_column_count_after_wrap
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inline_elements(&self) -> InlineElements<'a> {
        InlineElements {
            text: self.text,
            inline_inlays_by_byte_index: self.inline_inlays_by_byte_index.iter(),
            byte_index: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut inline_elements = self.inline_elements();
        WrappedElements {
            inline_element: inline_elements.next(),
            inline_elements,
            wrap_byte_indices: self.wrap_byte_indices.iter(),
            byte_index: 0,
        }
    }

    pub(super) fn compute_wrap_data(
        &self,
        max_column_count: usize,
        tab_column_count: usize,
    ) -> (usize, Vec<usize>) {
        let mut indent_column_count_after_wrap: usize = self
            .text
            .indent()
            .unwrap_or("")
            .chars()
            .map(|char| char.column_count(tab_column_count))
            .sum();
        for inline in self.inline_elements() {
            match inline {
                InlineElement::Text { text, .. } => {
                    for string in text.split_whitespace_boundaries() {
                        let column_count: usize = string
                            .chars()
                            .map(|char| char.column_count(tab_column_count))
                            .sum();
                        if indent_column_count_after_wrap + column_count > max_column_count {
                            indent_column_count_after_wrap = 0;
                            break;
                        }
                    }
                }
                InlineElement::Widget(widget) => {
                    if indent_column_count_after_wrap + widget.column_count > max_column_count {
                        indent_column_count_after_wrap = 0;
                        break;
                    }
                }
            }
        }
        let mut byte_index = 0;
        let mut column_index = 0;
        let mut wrap_byte_indices = Vec::new();
        for inline in self.inline_elements() {
            match inline {
                InlineElement::Text { text, .. } => {
                    for string in text.split_whitespace_boundaries() {
                        let column_count: usize = string
                            .chars()
                            .map(|char| char.column_count(tab_column_count))
                            .sum();
                        if column_index + column_count > max_column_count {
                            column_index = indent_column_count_after_wrap;
                            wrap_byte_indices.push(byte_index);
                        } else {
                            column_index += column_count;
                        }
                        byte_index += string.len();
                    }
                }
                InlineElement::Widget(widget) => {
                    if column_index + widget.column_count > max_column_count {
                        column_index = indent_column_count_after_wrap;
                        wrap_byte_indices.push(indent_column_count_after_wrap);
                    } else {
                        column_index += widget.column_count;
                    }
                }
            }
        }
        (indent_column_count_after_wrap, wrap_byte_indices)
    }
}

#[derive(Clone, Debug)]
pub struct InlineElements<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays_by_byte_index: Iter<'a, (usize, InlineInlay)>,
    pub(super) byte_index: usize,
}

impl<'a> Iterator for InlineElements<'a> {
    type Item = InlineElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays_by_byte_index
            .as_slice()
            .first()
            .map_or(false, |&(byte_index, _)| byte_index == self.byte_index)
        {
            let (_, inline_inlay) = self.inline_inlays_by_byte_index.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => InlineElement::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => InlineElement::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut byte_count = self.text.len();
        if let Some(&(byte_index, _)) = self.inline_inlays_by_byte_index.as_slice().first() {
            byte_count = byte_count.min(byte_index - self.byte_index);
        }
        let (text_0, text_1) = self.text.split_at(byte_count);
        self.text = text_1;
        self.byte_index += text_0.len();
        Some(InlineElement::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InlineElement<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    pub(super) inline_element: Option<InlineElement<'a>>,
    pub(super) inline_elements: InlineElements<'a>,
    pub(super) wrap_byte_indices: Iter<'a, usize>,
    pub(super) byte_index: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wrap_byte_indices
            .as_slice()
            .first()
            .map_or(false, |&byte_index| byte_index == self.byte_index)
        {
            self.wrap_byte_indices.next();
            return Some(WrappedElement::Wrap);
        }
        Some(match self.inline_element.take()? {
            InlineElement::Text { is_inlay, text } => {
                let mut byte_count = text.len();
                if let Some(&byte_index) = self.wrap_byte_indices.as_slice().first() {
                    byte_count = byte_count.min(byte_index - self.byte_index);
                }
                let text = if byte_count < text.len() {
                    let (text_0, text_1) = text.split_at(byte_count);
                    self.inline_element = Some(InlineElement::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline_element = self.inline_elements.next();
                    text
                };
                self.byte_index += text.len();
                WrappedElement::Text { is_inlay, text }
            }
            InlineElement::Widget(widget) => {
                self.byte_index += 1;
                WrappedElement::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
