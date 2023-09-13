use crate::{char::CharExt, layout::InlineElement, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .leading_whitespace()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count())
        .sum();
    for inline in line.inline_elements() {
        match inline {
            InlineElement::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string.chars().map(|char| char.column_count()).sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            InlineElement::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte_index = 0;
    let mut column_index = 0;
    let mut wraps = Vec::new();
    for element in line.inline_elements() {
        match element {
            InlineElement::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string.chars().map(|char| char.column_count()).sum();
                    if column_index + column_count > wrap_column {
                        column_index = indent_column_count;
                        wraps.push(byte_index);
                    }
                    column_index += column_count;
                    byte_index += string.len();
                }
            }
            InlineElement::Widget(widget) => {
                if column_index + widget.column_count > wrap_column {
                    column_index = indent_column_count;
                    wraps.push(byte_index);
                }
                column_index += widget.column_count;
                byte_index += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}
