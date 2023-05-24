use std::ops::ControlFlow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elem<'a> {
    pub byte_index: usize,
    pub pos: Pos,
    pub col_count: usize,
    pub kind: ElemKind<'a>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row_index: usize,
    pub col_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElemKind<'a> {
    End,
    LineBreak,
    Grapheme(&'a str),
}

pub fn layout<T>(line: &str, handle_elem: impl FnMut(Elem) -> ControlFlow<T>) -> ControlFlow<T> {
    Layouter {
        byte_index: 0,
        pos: Pos::default(),
        handle_elem,
    }
    .layout(line)
}

pub fn height(line: &str) -> usize {
    match layout(line, |elem| {
        if let ElemKind::End = elem.kind {
            return ControlFlow::Break(elem.pos.row_index + 1);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(row_height) => row_height,
        _ => unreachable!(),
    }
}

pub fn byte_index_to_pos(line: &str, byte_pos: usize) -> Option<Pos> {
    match layout(line, |elem| {
        if elem.byte_index == byte_pos {
            return ControlFlow::Break(elem.pos);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(pos) => Some(pos),
        ControlFlow::Continue(()) => None,
    }
}

pub fn pos_to_byte_index(line: &str, pos: Pos) -> Option<usize> {
    match layout(line, |elem| {
        if elem.pos == pos {
            return ControlFlow::Break(elem.byte_index);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(byte_pos) => Some(byte_pos),
        ControlFlow::Continue(()) => None,
    }
}

#[derive(Debug)]
struct Layouter<F> {
    byte_index: usize,
    pos: Pos,
    handle_elem: F,
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
        self.emit_elem(0, ElemKind::End)?;
        ControlFlow::Continue(())
    }

    fn layout_grapheme(&mut self, grapheme: &str) -> ControlFlow<T> {
        use crate::CharExt;

        let col_count = grapheme.chars().next().unwrap().col_count();
        self.emit_elem(col_count, ElemKind::Grapheme(grapheme))?;
        self.byte_index += grapheme.len();
        self.pos.col_index += col_count;
        ControlFlow::Continue(())
    }

    fn emit_elem(&mut self, width: usize, kind: ElemKind<'_>) -> ControlFlow<T> {
        (self.handle_elem)(Elem {
            byte_index: self.byte_index,
            pos: self.pos,
            col_count: width,
            kind,
        })?;
        ControlFlow::Continue(())
    }
}
