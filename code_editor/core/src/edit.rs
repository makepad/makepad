use crate::{cursor_set, Diff, Text};

#[derive(Debug)]
pub struct Context<'a> {
    pub text: &'a Text,
}

impl<'a> Context<'a> {
    pub fn insert(
        &self,
        spans: impl Iterator<Item = cursor_set::Span>,
        replace_with: &Text,
    ) -> Diff {
        use crate::diff::Builder;

        let mut builder = Builder::new();
        for span in spans {
            if span.is_sel {
                builder.delete(span.len);
                builder.insert(replace_with.clone());
            } else {
                builder.retain(span.len);
            }
        }
        builder.finish()
    }

    /*
    pub fn delete(
        &self,
        spans: impl Iterator<Item = cursor_set::Span>,
    ) -> Diff {
        use crate::diff::Builder;

        let mut builder = Builder::new();
        let mut pos = Pos::default();
        for span in spans {
            if span.is_sel {
                if span.len == Len::default() {
                    if pos.byte > 0 {
                        pos = Pos {
                            line: pos.line,
                            byte: self.text.as_lines()[pos.line].prev_grapheme_boundary(pos.byte).unwrap(),
                        };
                    } else if pos.line > 0 {
                        let prev_line = pos.line - 1;
                        Pos {
                            line: prev_line,
                            byte: self.text.as_lines()[prev_line].len,
                        }
                    };
                } else {
                    builder.delete(span.len);
                }
            } else {
                pos += span.len;
            }
        }
        builder.finish()
    }
    */
}
