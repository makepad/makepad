use crate::{char::CharExt, inlays::InlineInlay, str::StrExt, Settings, Token};

mod inlines;
mod wrappeds;

pub use self::{
    inlines::{Inline, Inlines},
    wrappeds::{Wrapped, Wrappeds},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub settings: &'a Settings,
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold: usize,
    pub scale: f64,
    pub indent: usize,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wraps: &'a [usize],
}

impl<'a> Line<'a> {
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
        let before_fold = column.min(self.fold);
        let after_fold = column - before_fold;
        before_fold as f64 + after_fold as f64 * self.scale
    }

    pub fn fold(&self) -> usize {
        self.fold
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn indent(self) -> usize {
        self.indent
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
            index: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wraps.iter(),
            index: 0,
        }
    }

    pub(super) fn compute_indent_and_wraps(&self, max_column: usize) -> (usize, Vec<usize>) {
        let mut indent: usize = self
            .text
            .indent()
            .unwrap_or("")
            .chars()
            .map(|char| char.column_count(self.settings.tab_column_count))
            .sum();
        for inline in self.inlines() {
            match inline {
                Inline::Text { text, .. } => {
                    for string in text.split_whitespace_boundaries() {
                        let column_count: usize = string
                            .chars()
                            .map(|char| char.column_count(self.settings.tab_column_count))
                            .sum();
                        if indent + column_count > max_column {
                            indent = 0;
                            break;
                        }
                    }
                }
                Inline::Widget(widget) => {
                    if indent + widget.column_count > max_column {
                        indent = 0;
                        break;
                    }
                }
            }
        }
        let mut index = 0;
        let mut column = 0;
        let mut wraps = Vec::new();
        for inline in self.inlines() {
            match inline {
                Inline::Text { text, .. } => {
                    for string in text.split_whitespace_boundaries() {
                        let column_count: usize = string
                            .chars()
                            .map(|char| char.column_count(self.settings.tab_column_count))
                            .sum();
                        if column + column_count > max_column {
                            column = indent;
                            wraps.push(index);
                        } else {
                            column += column_count;
                        }
                        index += string.len();
                    }
                }
                Inline::Widget(widget) => {
                    if column + widget.column_count > max_column {
                        column = indent;
                        wraps.push(indent);
                    } else {
                        column += widget.column_count;
                    }
                }
            }
        }
        (indent, wraps)
    }
}
