use {
    std::{iter, ops::{Deref, Index}, slice::Iter},
    crate::makepad_live_tokenizer::{
        range::Range,
        delta::{Delta, OperationRange},
        text::Text,
    },
};

#[derive(Clone, Debug,PartialEq)]
pub struct MsgCache {
    lines: Vec<Line>,
}

impl MsgCache {
    pub fn new(text: &Text) -> MsgCache {
        let cache = MsgCache {
            lines: (0..text.as_lines().len()).map(|_| Line::default()).collect::<Vec<_>>(),
        };
        //cache.refresh(text, msg_ranges);
        cache
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.lines[range.start.line] = Line::default();
                    self.lines.splice(
                        range.start.line..range.start.line,
                        iter::repeat(Line::default()).take(range.end.line - range.start.line),
                    );
                }
                OperationRange::Delete(range) => {
                    self.lines.drain(range.start.line..range.end.line);
                    self.lines[range.start.line] = Line::default();
                }
            }
        }
    }

    pub fn clear(&mut self) {
        for line in &mut self.lines{
            line.spans.clear();
        }
    }

    pub fn add_range(&mut self, text:&Text, msg_id: usize, range:Range) {
        // ok so.. we now have to go from line to line
        let start = range.start;
        let end = range.end;
        let lines = text.as_lines();
        if start.line < self.lines.len() && end.line < self.lines.len(){
            if start.line != end.line{
                self.lines[start.line].spans.push(BuilderMsgSpan{
                    start_column: start.column,
                    end_column: lines[start.line].len(),
                    msg_id
                });
                for line in start.line+1..end.line{
                    self.lines[line].spans.push(BuilderMsgSpan{
                        start_column: 0,
                        end_column: lines[start.line].len(),
                        msg_id
                    });
                }
                self.lines[end.line].spans.push(BuilderMsgSpan{
                    start_column: 0,
                    end_column: end.column,
                    msg_id
                });
            }
            else{
                self.lines[start.line].spans.push(BuilderMsgSpan{
                    start_column: start.column,
                    end_column: end.column,
                    msg_id
                });
            }
        }
    }
}

impl Deref for MsgCache {
    type Target = [Line];

    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl Index<usize> for MsgCache {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl<'a> IntoIterator for &'a MsgCache {
    type Item = &'a Line;
    type IntoIter = Iter<'a, Line>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuilderMsgSpan{
    pub start_column: usize,
    pub end_column: usize,
    pub msg_id: usize
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    spans: Vec<BuilderMsgSpan>
}

impl Line {
    pub fn spans(&self) -> &[BuilderMsgSpan] {
        &self.spans
    }
}