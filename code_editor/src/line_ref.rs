use {
    crate::{char::CharExt, inlays::InlineInlay, str::StrExt, widgets::InlineWidget, Token},
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineRef<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub indent_column_count: usize,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wraps: &'a [usize],
}

impl<'a> LineRef<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn indent_column_count(self) -> usize {
        self.indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            byte: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wraps.iter(),
            byte: 0,
        }
    }

    pub(super) fn compute_wrap_data(
        &self,
        max_column_count: usize,
        tab_column_count: usize,
    ) -> (usize, Vec<usize>) {
        let mut indent_column_count: usize = self
            .text
            .indent()
            .unwrap_or("")
            .chars()
            .map(|char| char.column_count(tab_column_count))
            .sum();
        for inline in self.inlines() {
            match inline {
                Inline::Text { text, .. } => {
                    for string in text.split_whitespace_boundaries() {
                        let column_count: usize = string
                            .chars()
                            .map(|char| char.column_count(tab_column_count))
                            .sum();
                        if indent_column_count + column_count > max_column_count {
                            indent_column_count = 0;
                            break;
                        }
                    }
                }
                Inline::Widget(widget) => {
                    if indent_column_count + widget.column_count > max_column_count {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
        }
        let mut byte_index = 0;
        let mut column_index = 0;
        let mut wraps = Vec::new();
        for inline in self.inlines() {
            match inline {
                Inline::Text { text, .. } => {
                    for string in text.split_whitespace_boundaries() {
                        let column_count: usize = string
                            .chars()
                            .map(|char| char.column_count(tab_column_count))
                            .sum();
                        if column_index + column_count > max_column_count {
                            column_index = indent_column_count;
                            wraps.push(byte_index);
                        } else {
                            column_index += column_count;
                        }
                        byte_index += string.len();
                    }
                }
                Inline::Widget(widget) => {
                    if column_index + widget.column_count > max_column_count {
                        column_index = indent_column_count;
                        wraps.push(indent_column_count);
                    } else {
                        column_index += widget.column_count;
                    }
                }
            }
        }
        (indent_column_count, wraps)
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) byte: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(byte_index, _)| byte_index == self.byte)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.byte);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.byte += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) byte: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&byte| byte == self.byte)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid = text.len();
                if let Some(&byte) = self.wraps.as_slice().first() {
                    mid = mid.min(byte - self.byte);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.byte += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.byte += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
