#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event<'a> {
    pub byte: usize,
    pub pos: Pos,
    pub kind: EventKind<'a>
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind<'a> {
    LineStart,
    LineEnd,
    GraphemeStart(&'a str),
    GraphemeEnd(&'a str),
}

pub fn layout(line: &str, handle_event: impl FnMut(Event)) {
    Layouter {
        byte: 0,
        pos: Pos::default(),
        handle_event,
    }
    .layout(line);
}

#[derive(Debug)]
struct Layouter<F> {
    byte: usize,
    pos: Pos,
    handle_event: F,
}

impl<F> Layouter<F>
where
    F: FnMut(Event<'_>)
{
    fn layout(&mut self, line: &str) {
        use crate::StrExt;

        self.emit_event(EventKind::LineStart);
        for grapheme in line.graphemes() {
            self.layout_grapheme(grapheme);
        }
        self.emit_event(EventKind::LineEnd);
    }

    fn layout_grapheme(&mut self, grapheme: &str) {
        self.emit_event(EventKind::GraphemeStart(grapheme));
        self.byte += grapheme.len();
        self.pos.column += grapheme.len();
        self.emit_event(EventKind::GraphemeEnd(grapheme));
    }

    fn emit_event(&mut self, kind: EventKind<'_>) {
        (self.handle_event)(Event {
            byte: self.byte,
            pos: self.pos,
            kind,
        });
    }
}