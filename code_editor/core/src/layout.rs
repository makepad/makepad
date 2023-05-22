use std::ops::ControlFlow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elem<'a> {
    pub byte_pos: usize,
    pub pos: Pos,
    pub column_len: usize,
    pub kind: ElemKind<'a>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElemKind<'a> {
    NewLine,
    Grapheme(&'a str),
}

pub fn layout<T>(line: &str, handle_elem: impl FnMut(Elem) -> ControlFlow<T>) -> ControlFlow<T> {
    Layouter {
        byte_pos: 0,
        pos: Pos::default(),
        handle_event: handle_elem,
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
    F: FnMut(Elem<'_>) -> ControlFlow<T>,
{
    fn layout(&mut self, line: &str) -> ControlFlow<T> {
        use crate::StrExt;

        for grapheme in line.graphemes() {
            self.layout_grapheme(grapheme)?;
        }
        self.emit_elem(0, ElemKind::NewLine)
    }

    fn layout_grapheme(&mut self, grapheme: &str) -> ControlFlow<T> {
        use crate::CharExt;

        let column_len = grapheme.chars().next().unwrap().column_len();
        self.emit_elem(column_len, ElemKind::Grapheme(grapheme))?;
        self.byte_pos += grapheme.len();
        ControlFlow::Continue(())
    }

    fn emit_elem(&mut self, column_len: usize, kind: ElemKind<'_>) -> ControlFlow<T> {
        (self.handle_event)(Elem {
            byte_pos: self.byte_pos,
            pos: self.pos,
            column_len,
            kind,
        })?;
        self.pos.column += column_len;
        ControlFlow::Continue(())
    }
}
