use crate::{char::CharExt, line::Inline, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize, tab_column_count: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .indent()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count(tab_column_count))
        .sum();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            Inline::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte = 0;
    let mut column = 0;
    let mut wraps = Vec::new();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if column + column_count > wrap_column {
                        column = indent_column_count;
                        wraps.push(byte);
                    }
                    column += column_count;
                    byte += string.len();
                }
            }
            Inline::Widget(widget) => {
                if column + widget.column_count > wrap_column {
                    column = indent_column_count;
                    wraps.push(byte);
                }
                column += widget.column_count;
                byte += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}
