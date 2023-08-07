use {
    crate::{
        arena::Id,
        char::CharExt,
        inlays::{BlockInlay, InlineInlay},
        line::Wrapped,
        selection::Affinity,
        widgets::BlockWidget,
        Arena, Line, Point, Selection, Settings, Token,
    },
    std::{
        collections::{HashMap, HashSet},
        fs::File,
        io,
        io::{BufRead, BufReader},
        mem,
        ops::Range,
        path::{Path, PathBuf},
        slice::Iter,
    },
};

#[derive(Clone, Debug, Default)]
pub struct State {
    settings: Settings,
    sessions: Arena<Session>,
    documents: Arena<Document>,
    document_ids: HashMap<PathBuf, DocumentId>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn width(&self, session_id: SessionId) -> f64 {
        let mut width: f64 = 0.0;
        for line in self.lines(session_id, 0..self.line_count(self.document_id(session_id))) {
            width = width.max(line.width());
        }
        width
    }

    pub fn height(&self, session_id: SessionId) -> f64 {
        let line_count = self.line_count(self.document_id(session_id));
        let line = self.line(session_id, line_count - 1);
        let mut y = line.y() + line.height();
        for block in self.blocks(session_id, line_count..line_count) {
            match block {
                Block::Line {
                    is_inlay: true,
                    line,
                } => y += line.height(),
                Block::Widget(widget) => y += widget.height,
                _ => unreachable!(),
            }
        }
        y
    }

    pub fn document_id(&self, session_id: SessionId) -> DocumentId {
        self.sessions[session_id.0].document_id
    }

    pub fn max_column(&self, session_id: SessionId) -> usize {
        self.sessions[session_id.0].max_column
    }

    pub fn find_first_line_ending_after_y(&self, session_id: SessionId, y: f64) -> usize {
        match self.sessions[session_id.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, session_id: SessionId, y: f64) -> usize {
        match self.sessions[session_id.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    pub fn line(&self, session_id: SessionId, index: usize) -> Line<'_> {
        let document_id = self.document_id(session_id);
        Line {
            y: self.sessions[session_id.0].y.get(index).copied(),
            column_count: self.sessions[session_id.0].column_count[index],
            fold: self.sessions[session_id.0].fold[index],
            scale: self.sessions[session_id.0].scale[index],
            text: &self.documents[document_id.0].text[index],
            tokens: &self.documents[document_id.0].tokens[index],
            inline_inlays: &self.documents[document_id.0].inline_inlays[index],
            wraps: &self.sessions[session_id.0].wraps[index],
            indent: self.sessions[session_id.0].indent[index],
        }
    }

    pub fn lines(&self, session_id: SessionId, range: Range<usize>) -> Lines<'_> {
        let document_id = self.document_id(session_id);
        let y_count = self.sessions[session_id.0].y.len();
        Lines {
            y: self.sessions[session_id.0].y[range.start.min(y_count)..range.end.min(y_count)]
                .iter(),
            column_count: self.sessions[session_id.0].column_count[range.start..range.end].iter(),
            fold: self.sessions[session_id.0].fold[range.start..range.end].iter(),
            scale: self.sessions[session_id.0].scale[range.start..range.end].iter(),
            indent: self.sessions[session_id.0].indent[range.start..range.end].iter(),
            text: self.documents[document_id.0].text[range.start..range.end].iter(),
            tokens: self.documents[document_id.0].tokens[range.start..range.end].iter(),
            inline_inlays: self.documents[document_id.0].inline_inlays[range.start..range.end]
                .iter(),
            wraps: self.sessions[session_id.0].wraps[range.start..range.end].iter(),
        }
    }

    pub fn blocks(&self, session_id: SessionId, range: Range<usize>) -> Blocks<'_> {
        let document_id = self.document_id(session_id);
        let mut block_inlays = self.documents[document_id.0].block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(index, _)| index < range.start)
        {
            block_inlays.next();
        }
        Blocks {
            lines: self.lines(session_id, range.start..range.end),
            block_inlays,
            index: range.start,
        }
    }

    pub fn selections(&self, session_id: SessionId) -> &[Selection] {
        &self.sessions[session_id.0].selections
    }

    pub fn line_count(&self, document_id: DocumentId) -> usize {
        self.documents[document_id.0].text.len()
    }

    pub fn new_file(&mut self, text: Vec<String>) -> SessionId {
        let document_id = self.create_document(None, text);
        self.create_session(document_id)
    }

    pub fn open_file(&mut self, path: impl AsRef<Path> + Into<PathBuf>) -> io::Result<SessionId> {
        let document_id = match self.document_ids.get(path.as_ref()) {
            Some(&document_id) => document_id,
            None => {
                let file = File::open(path.as_ref())?;
                self.create_document(
                    Some(path.into()),
                    BufReader::new(file).lines().collect::<Result<_, _>>()?,
                )
            }
        };
        Ok(self.create_session(document_id))
    }

    pub fn close_file(&mut self, session_id: SessionId) {
        self.destroy_session(session_id);
    }

    fn create_session(&mut self, document_id: DocumentId) -> SessionId {
        let line_count = self.documents[document_id.0].text.len();
        let session_id = SessionId(self.sessions.insert(Session {
            document_id,
            max_column: usize::MAX,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wraps: (0..line_count).map(|_| Vec::new()).collect(),
            indent: (0..line_count).map(|_| 0).collect(),
            selections: vec![Selection {
                cursor: Point { line: 0, byte: 0 },
                anchor: Point { line: 7, byte: 28 },
                affinity: Affinity::Before,
            }],
        }));
        self.documents[document_id.0].session_ids.insert(session_id);
        self.update_y(session_id);
        for index in 0..self.line_count(document_id) {
            self.update_column_count(session_id, index);
        }
        session_id
    }

    pub fn set_max_column(&mut self, session_id: SessionId, max_column: usize) {
        if self.sessions[session_id.0].max_column == max_column {
            return;
        }
        self.sessions[session_id.0].max_column = max_column;
        for index in 0..self.line_count(self.document_id(session_id)) {
            self.update_indent_and_wraps(session_id, index);
        }
        self.update_y(session_id);
    }

    fn update_y(&mut self, session_id: SessionId) {
        let start = self.sessions[session_id.0].y.len();
        let line_count = self.line_count(self.document_id(session_id));
        if start == line_count + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            let line = self.line(session_id, start - 1);
            line.y() + line.height()
        };
        let mut ys = mem::take(&mut self.sessions[session_id.0].y);
        for block in self.blocks(session_id, start..line_count) {
            match block {
                Block::Line { is_inlay, line } => {
                    if !is_inlay {
                        ys.push(y);
                    }
                    y += line.height();
                }
                Block::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        ys.push(y);
        self.sessions[session_id.0].y = ys;
    }

    fn update_column_count(&mut self, session_id: SessionId, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        let line = self.line(session_id, index);
        for wrapped in line.wrappeds() {
            match wrapped {
                Wrapped::Text { text, .. } => {
                    column += text
                        .chars()
                        .map(|char| char.column_count(self.settings.tab_column_count))
                        .sum::<usize>();
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    column_count = column_count.max(column);
                    column = line.indent();
                }
            }
        }
        self.sessions[session_id.0].column_count[index] = Some(column_count.max(column));
    }

    fn update_indent_and_wraps(&mut self, session_id: SessionId, index: usize) {
        let (indent, wraps) = self.line(session_id, index).compute_indent_and_wraps(
            self.sessions[session_id.0].max_column,
            self.settings.tab_column_count,
        );
        self.sessions[session_id.0].wraps[index] = wraps;
        self.sessions[session_id.0].indent[index] = indent;
        self.update_column_count(session_id, index);
        self.sessions[session_id.0].y.truncate(index + 1);
    }

    fn destroy_session(&mut self, session_id: SessionId) {
        let document_id = self.document_id(session_id);
        self.documents[document_id.0]
            .session_ids
            .remove(&session_id);
        if self.documents[document_id.0].session_ids.is_empty() {
            self.destroy_document(document_id);
        }
        self.sessions.remove(session_id.0);
    }

    fn create_document(&mut self, path: Option<PathBuf>, text: Vec<String>) -> DocumentId {
        let line_count = text.len();
        let document_id = DocumentId(self.documents.insert(Document {
            path,
            text,
            tokens: (0..line_count).map(|_| Vec::new()).collect(),
            inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
            block_inlays: Vec::new(),
            session_ids: HashSet::new(),
        }));
        if let Some(path) = &self.documents[document_id.0].path {
            self.document_ids.insert(path.clone(), document_id);
        }
        document_id
    }

    fn destroy_document(&mut self, document_id: DocumentId) {
        if let Some(path) = &self.documents[document_id.0].path {
            self.document_ids.remove(path);
        }
        self.documents.remove(document_id.0);
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    pub(super) lines: Lines<'a>,
    pub(super) block_inlays: Iter<'a, (usize, BlockInlay)>,
    pub(super) index: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(index, _)| index == self.index)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.index += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub indent: Iter<'a, usize>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wraps: Iter<'a, Vec<usize>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold: *self.fold.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            indent: *self.indent.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wraps: self.wraps.next().unwrap(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(Id<Session>);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(Id<Document>);

#[derive(Clone, Debug)]
struct Session {
    document_id: DocumentId,
    max_column: usize,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold: Vec<usize>,
    scale: Vec<f64>,
    wraps: Vec<Vec<usize>>,
    indent: Vec<usize>,
    selections: Vec<Selection>,
}

#[derive(Clone, Debug)]
struct Document {
    path: Option<PathBuf>,
    text: Vec<String>,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    session_ids: HashSet<SessionId>,
}
