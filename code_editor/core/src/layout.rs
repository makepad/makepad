use std::ops::ControlFlow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elem<'a> {
    pub byte: usize,
    pub pos: Pos,
    pub width: usize,
    pub kind: ElemKind<'a>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElemKind<'a> {
    NewRow,
    Grapheme(&'a str),
}

pub fn layout<T>(line: &str, handle_elem: impl FnMut(Elem) -> ControlFlow<T>) -> ControlFlow<T> {
    LayoutContext {
        byte: 0,
        pos: Pos::default(),
        handle_elem,
    }
    .layout(line)
}

pub fn row_height(line: &str) -> usize {
    let mut row_height = 0;
    layout(line, |elem| {
        match elem.kind {
            ElemKind::NewRow => row_height += 1,
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
    row_height
}

pub fn byte_to_pos(line: &str, byte: usize) -> Option<Pos> {
    match layout(line, |elem| {
        if elem.byte == byte {
            return ControlFlow::Break(elem.pos);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(pos) => Some(pos),
        ControlFlow::Continue(()) => None,
    }
}

pub fn pos_to_byte(line: &str, pos: Pos) -> Option<usize> {
    match layout(line, |elem| {
        if elem.pos == pos {
            return ControlFlow::Break(elem.byte);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(byte) => Some(byte),
        ControlFlow::Continue(()) => None,
    }
}

#[derive(Debug)]
struct LayoutContext<F> {
    byte: usize,
    pos: Pos,
    handle_elem: F,
}

impl<T, F> LayoutContext<F>
where
    F: FnMut(Elem<'_>) -> ControlFlow<T>,
{
    fn layout(&mut self, line: &str) -> ControlFlow<T> {
        use crate::StrExt;

        for grapheme in line.graphemes() {
            self.layout_grapheme(grapheme)?;
        }
        self.emit_elem(0, ElemKind::NewRow)?;
        self.pos.row += 1;
        self.pos.column = 0;
        ControlFlow::Continue(())
    }

    fn layout_grapheme(&mut self, grapheme: &str) -> ControlFlow<T> {
        use crate::CharExt;

        let column_len = grapheme.chars().next().unwrap().column_width();
        self.emit_elem(column_len, ElemKind::Grapheme(grapheme))?;
        self.byte += grapheme.len();
        self.pos.column += column_len;
        ControlFlow::Continue(())
    }

    fn emit_elem(&mut self, width: usize, kind: ElemKind<'_>) -> ControlFlow<T> {
        (self.handle_elem)(Elem {
            byte: self.byte,
            pos: self.pos,
            width,
            kind,
        })?;
        ControlFlow::Continue(())
    }
}
