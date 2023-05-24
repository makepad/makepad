use std::ops::ControlFlow;

#[derive(Debug)]
pub struct Context<'a> {
    pub line: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elem<'a> {
    pub byte_pos: usize,
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
    End,
    LineBreak,
    Grapheme(&'a str),
}

pub fn layout<T>(
    context: &Context<'_>,
    handle_elem: impl FnMut(Elem) -> ControlFlow<T>,
) -> ControlFlow<T> {
    Layouter {
        byte_pos: 0,
        pos: Pos::default(),
        handle_elem,
    }
    .layout(context.line)
}

pub fn height(context: &Context<'_>) -> usize {
    match layout(context, |elem| {
        if let ElemKind::End = elem.kind {
            return ControlFlow::Break(elem.pos.row + 1);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(row_height) => row_height,
        _ => unreachable!(),
    }
}

pub fn byte_pos_to_pos(context: &Context<'_>, byte_pos: usize) -> Option<Pos> {
    match layout(context, |elem| {
        if elem.byte_pos == byte_pos {
            return ControlFlow::Break(elem.pos);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(pos) => Some(pos),
        ControlFlow::Continue(()) => None,
    }
}

pub fn pos_to_byte_pos(context: &Context<'_>, pos: Pos) -> Option<usize> {
    match layout(context, |elem| {
        if elem.pos == pos {
            return ControlFlow::Break(elem.byte_pos);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(byte_pos) => Some(byte_pos),
        ControlFlow::Continue(()) => None,
    }
}

#[derive(Debug)]
struct Layouter<F> {
    byte_pos: usize,
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

        let column_len = grapheme.chars().next().unwrap().width();
        self.emit_elem(column_len, ElemKind::Grapheme(grapheme))?;
        self.byte_pos += grapheme.len();
        self.pos.column += column_len;
        ControlFlow::Continue(())
    }

    fn emit_elem(&mut self, width: usize, kind: ElemKind<'_>) -> ControlFlow<T> {
        (self.handle_elem)(Elem {
            byte_pos: self.byte_pos,
            pos: self.pos,
            width,
            kind,
        })?;
        ControlFlow::Continue(())
    }
}
