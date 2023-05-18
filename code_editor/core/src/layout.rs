use std::ops::ControlFlow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event<'a> {
    pub byte_pos: usize,
    pub pos: Pos,
    pub column_len: usize,
    pub kind: EventKind<'a>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind<'a> {
    NewLine,
    Grapheme(&'a str)
}

pub fn layout<T>(line: &str, handle_event: impl FnMut(Event) -> ControlFlow<T>) -> ControlFlow<T> {
    Layouter {
        byte_pos: 0,
        pos: Pos::default(),
        handle_event,
    }
    .layout(line)
}

#[derive(Debug)]
struct Layouter<F> {
    byte_pos: usize,
    pos: Pos,
    handle_event: F,
}

impl<T, F> Layouter<F>
where
    F: FnMut(Event<'_>) -> ControlFlow<T>,
{
    fn layout(&mut self, line: &str) -> ControlFlow<T> {
        use crate::StrExt;

        for grapheme in line.graphemes() {
            self.layout_grapheme(grapheme)?;
        }
        self.emit_event(0, EventKind::NewLine)
    }

    fn layout_grapheme(&mut self, grapheme: &str) -> ControlFlow<T> {
        use crate::CharExt;

        let column_len = grapheme.chars().next().unwrap().column_len();
        self.emit_event(column_len, EventKind::Grapheme(grapheme))?;
        self.byte_pos += grapheme.len();
        ControlFlow::Continue(())
    }

    fn emit_event(&mut self, column_len: usize, kind: EventKind<'_>) -> ControlFlow<T> {
        (self.handle_event)(Event {
            byte_pos: self.byte_pos,
            pos: self.pos,
            column_len,
            kind,
        })?;
        self.pos.column += column_len;
        ControlFlow::Continue(())
    }
}
