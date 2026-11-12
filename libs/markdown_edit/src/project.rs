//! The visible text of a parsed slice and the source range behind each
//! visible character.

use pulldown_cmark::{Event, Tag};
use std::ops::Range;

/// The stand-in character of an atomic object in the visible text.
const OBJECT: char = '\u{FFFC}';

/// Which side a position on a boundary between two pieces belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Affinity {
    Left,
    Right,
}

/// An atomic piece: one or a few visible characters for a source range they
/// do not spell byte for byte. The caret stands only at its two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atom {
    /// Text the parser made (an entity, an expanded tab) unlike its source.
    Synthesized,
    SoftBreak,
    HardBreak,
    /// The hidden source between two blocks drawn in one flow.
    BlockBreak,
    Image,
    TaskMarker(bool),
    FootnoteRef,
    Math,
    Rule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    /// Visible text equal to its source, byte for byte.
    Text,
    Atomic(Atom),
}

/// A run of visible text and its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub src: Range<usize>,
    pub visible: Range<usize>,
    pub kind: PieceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineKind {
    Emphasis,
    Strong,
    Strikethrough,
    Code,
    Link,
}

/// An inline container: its open and close markers in the source. Its
/// content lies between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub kind: InlineKind,
    pub open: Range<usize>,
    pub close: Range<usize>,
}

/// The visible text of a parsed slice, with every piece's source range and
/// the inline containers. Offsets are the slice's.
#[derive(Clone, Debug, Default)]
pub struct Projection {
    pub text: String,
    pub pieces: Vec<Piece>,
    pub inlines: Vec<Inline>,
}

struct Frame {
    inline: Option<InlineKind>,
    range: Range<usize>,
    first: Option<usize>,
    last: Option<usize>,
}

impl Projection {
    /// Projects the events of a parse of `source`.
    pub fn new(source: &str, events: &[(Event<'_>, Range<usize>)]) -> Self {
        let mut out = Projection::default();
        let mut stack: Vec<Frame> = Vec::new();
        let mut image_depth = 0usize;
        let mut pending_break = false;
        for (event, range) in events {
            let range = range.clone();
            if image_depth > 0 {
                match event {
                    Event::Start(_) => image_depth += 1,
                    Event::End(_) => image_depth -= 1,
                    _ => {}
                }
                if image_depth > 0 {
                    continue;
                }
                // The image's own end closes its frame below.
            }
            match event {
                Event::Start(tag) => {
                    if let Some(top) = stack.last_mut() {
                        top.first.get_or_insert(range.start);
                    }
                    let inline = match tag {
                        Tag::Emphasis => Some(InlineKind::Emphasis),
                        Tag::Strong => Some(InlineKind::Strong),
                        Tag::Strikethrough => Some(InlineKind::Strikethrough),
                        Tag::Link { .. } => Some(InlineKind::Link),
                        _ => None,
                    };
                    let is_block = !matches!(
                        tag,
                        Tag::Emphasis
                            | Tag::Strong
                            | Tag::Strikethrough
                            | Tag::Link { .. }
                            | Tag::Image { .. }
                    );
                    if is_block && !out.pieces.is_empty() {
                        pending_break = true;
                    }
                    if let Tag::Image { .. } = tag {
                        out.push(
                            &mut pending_break,
                            range.clone(),
                            &OBJECT.to_string(),
                            PieceKind::Atomic(Atom::Image),
                        );
                        image_depth = 1;
                    }
                    stack.push(Frame {
                        inline,
                        range,
                        first: None,
                        last: None,
                    });
                }
                Event::End(_) => {
                    let Some(frame) = stack.pop() else { continue };
                    if let Some(kind) = frame.inline {
                        let first = frame.first.unwrap_or(frame.range.start + 1);
                        let last = frame.last.unwrap_or(first);
                        out.inlines.push(Inline {
                            kind,
                            open: frame.range.start..first,
                            close: last..frame.range.end,
                        });
                    }
                    if let Some(top) = stack.last_mut() {
                        top.first.get_or_insert(frame.range.start);
                        top.last = Some(frame.range.end);
                    }
                }
                _ => {
                    if let Some(top) = stack.last_mut() {
                        top.first.get_or_insert(range.start);
                        top.last = Some(range.end);
                    }
                    out.leaf(source, event, range, &mut pending_break);
                }
            }
        }
        out.inlines.sort_by_key(|inline| inline.open.start);
        out
    }

    fn leaf(
        &mut self,
        source: &str,
        event: &Event<'_>,
        range: Range<usize>,
        pending_break: &mut bool,
    ) {
        let atomic = |atom| PieceKind::Atomic(atom);
        let object = OBJECT.to_string();
        match event {
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                if source.get(range.clone()) == Some(&**text) {
                    self.push(pending_break, range, text, PieceKind::Text);
                } else if !text.is_empty() {
                    self.push(pending_break, range, text, atomic(Atom::Synthesized));
                }
            }
            Event::Code(text) => {
                let span = &source[range.clone()];
                let ticks = span.bytes().take_while(|&b| b == b'`').count();
                let inner = span.len().saturating_sub(ticks * 2);
                // One space of padding goes on each side when both are there.
                let pad = inner.saturating_sub(text.len()) / 2;
                let content = range.start + ticks + pad..range.start + ticks + pad + text.len();
                // A line ending inside a code span shows as a space: one byte
                // for one byte, so the offsets still map one to one. Container
                // prefixes inside a span that crosses lines do not.
                let identity = source.get(content.clone()).is_some_and(|src| {
                    src.bytes()
                        .zip(text.bytes())
                        .all(|(a, b)| a == b || (a == b'\n' && b == b' '))
                });
                let (content, kind) = if identity {
                    (content, PieceKind::Text)
                } else {
                    let inner = range.start + ticks..range.end.saturating_sub(ticks).max(range.start + ticks);
                    (inner, atomic(Atom::Synthesized))
                };
                self.inlines.push(Inline {
                    kind: InlineKind::Code,
                    open: range.start..content.start,
                    close: content.end..range.end,
                });
                if text.is_empty() {
                    return;
                }
                self.push(pending_break, content, text, kind);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                self.push(pending_break, range, &object, atomic(Atom::Math))
            }
            Event::FootnoteReference(_) => {
                self.push(pending_break, range, &object, atomic(Atom::FootnoteRef))
            }
            Event::SoftBreak => self.push(pending_break, range, " ", atomic(Atom::SoftBreak)),
            Event::HardBreak => self.push(pending_break, range, "\n", atomic(Atom::HardBreak)),
            Event::Rule => self.push(pending_break, range, &object, atomic(Atom::Rule)),
            Event::TaskListMarker(checked) => {
                self.push(pending_break, range, &object, atomic(Atom::TaskMarker(*checked)))
            }
            Event::Start(_) | Event::End(_) => {}
        }
    }

    fn push(&mut self, pending_break: &mut bool, src: Range<usize>, text: &str, kind: PieceKind) {
        if text.is_empty() {
            return;
        }
        if std::mem::take(pending_break) {
            if let Some(last) = self.pieces.last() {
                let gap = last.src.end..src.start.max(last.src.end);
                let at = self.text.len();
                self.text.push('\n');
                self.pieces.push(Piece {
                    src: gap,
                    visible: at..at + 1,
                    kind: PieceKind::Atomic(Atom::BlockBreak),
                });
            }
        }
        let at = self.text.len();
        self.text.push_str(text);
        self.pieces.push(Piece {
            src,
            visible: at..self.text.len(),
            kind,
        });
    }

    /// The source offset of visible position `at` (a char boundary of
    /// `text`). On a boundary between two pieces, `affinity` picks the side;
    /// at the very start both sides are the first piece's start.
    pub fn to_source(&self, at: usize, affinity: Affinity) -> usize {
        let Some(last) = self.pieces.last() else { return 0 };
        if at >= last.visible.end {
            return last.src.end;
        }
        let i = self.pieces.partition_point(|piece| piece.visible.end < at);
        let piece = &self.pieces[i];
        if at <= piece.visible.start {
            return piece.src.start;
        }
        if at < piece.visible.end {
            return match piece.kind {
                PieceKind::Text => piece.src.start + (at - piece.visible.start),
                PieceKind::Atomic(_) => match affinity {
                    Affinity::Left => piece.src.start,
                    Affinity::Right => piece.src.end,
                },
            };
        }
        // `at` ends this piece and starts the next.
        match affinity {
            Affinity::Left => piece.src.end,
            Affinity::Right => self.pieces.get(i + 1).map_or(piece.src.end, |next| next.src.start),
        }
    }

    /// The visible position of source offset `src`. Hidden source (markers,
    /// prefixes) maps to the visible boundary it sits at; a position inside
    /// an atomic piece to the piece's start.
    pub fn to_visible(&self, src: usize) -> usize {
        let i = self.pieces.partition_point(|piece| piece.src.end <= src);
        let Some(piece) = self.pieces.get(i) else {
            return self.text.len();
        };
        if src <= piece.src.start {
            return piece.visible.start;
        }
        match piece.kind {
            PieceKind::Text => piece.visible.start + (src - piece.src.start),
            PieceKind::Atomic(_) => piece.visible.start,
        }
    }

    /// Where typing at visible position `at` inserts: the left side of the
    /// boundary, inside emphasis, strong, strike and code that end there, but
    /// after a link that ends there (typing after a link does not extend it).
    /// At the very start, the first character's containers.
    pub fn caret_source(&self, at: usize) -> usize {
        let mut src = self.to_source(at, Affinity::Left);
        if at == 0 {
            return src;
        }
        loop {
            let after_link = self
                .inlines
                .iter()
                .find(|inline| inline.kind == InlineKind::Link && inline.close.start == src);
            match after_link {
                Some(link) => src = link.close.end,
                None => return src,
            }
        }
    }
}
