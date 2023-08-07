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
    documents_by_path: HashMap<PathBuf, DocumentId>,
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

    pub fn width(&self, session: SessionId) -> f64 {
        let mut width: f64 = 0.0;
        for line in self.lines(session, 0..self.line_count(self.document(session))) {
            width = width.max(line.width());
        }
        width
    }

    pub fn height(&self, session: SessionId) -> f64 {
        let line_count = self.line_count(self.document(session));
        let line = self.line(session, line_count - 1);
        let mut y = line.y() + line.height();
        for block in self.blocks(session, line_count..line_count) {
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

    pub fn document(&self, session: SessionId) -> DocumentId {
        self.sessions[session.0].document
    }

    pub fn max_column(&self, session: SessionId) -> usize {
        self.sessions[session.0].max_column
    }

    pub fn find_first_line_ending_after_y(&self, session: SessionId, y: f64) -> usize {
        match self.sessions[session.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, session: SessionId, y: f64) -> usize {
        match self.sessions[session.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line(&self, session: SessionId, line: usize) -> Line<'_> {
        let document = self.document(session);
        Line {
            y: self.sessions[session.0].y.get(line).copied(),
            column_count: self.sessions[session.0].column_count[line],
            fold: self.sessions[session.0].fold[line],
            scale: self.sessions[session.0].scale[line],
            text: &self.documents[document.0].text[line],
            tokens: &self.documents[document.0].tokens[line],
            inline_inlays: &self.documents[document.0].inline_inlays[line],
            wraps: &self.sessions[session.0].wraps[line],
            indent: self.sessions[session.0].indent[line],
        }
    }

    pub fn lines(&self, session: SessionId, line_range: Range<usize>) -> Lines<'_> {
        let document = self.document(session);
        let y_count = self.sessions[session.0].y.len();
        Lines {
            y: self.sessions[session.0].y[line_range.start.min(y_count)..line_range.end.min(y_count)].iter(),
            column_count: self.sessions[session.0].column_count[line_range.start..line_range.end].iter(),
            fold: self.sessions[session.0].fold[line_range.start..line_range.end].iter(),
            scale: self.sessions[session.0].scale[line_range.start..line_range.end].iter(),
            indent: self.sessions[session.0].indent[line_range.start..line_range.end].iter(),
            text: self.documents[document.0].text[line_range.start..line_range.end].iter(),
            tokens: self.documents[document.0].tokens[line_range.start..line_range.end].iter(),
            inline_inlays: self.documents[document.0].inline_inlays[line_range.start..line_range.end].iter(),
            wraps: self.sessions[session.0].wraps[line_range.start..line_range.end].iter(),
        }
    }

    pub fn blocks(&self, session: SessionId, line_range: Range<usize>) -> Blocks<'_> {
        let document = self.document(session);
        let mut block_inlays = self.documents[document.0].block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line < line_range.start)
        {
            block_inlays.next();
        }
        Blocks {
            lines: self.lines(session, line_range.start..line_range.end),
            block_inlays,
            line: line_range.start,
        }
    }

    pub fn selections(&self, session: SessionId) -> &[Selection] {
        &self.sessions[session.0].selections
    }

    pub fn line_count(&self, document: DocumentId) -> usize {
        self.documents[document.0].text.len()
    }

    pub fn new_file(&mut self, text: Vec<String>) -> SessionId {
        let document = self.create_document(None, text);
        self.create_session(document)
    }

    pub fn open_file(&mut self, path: impl AsRef<Path> + Into<PathBuf>) -> io::Result<SessionId> {
        let document = match self.documents_by_path.get(path.as_ref()) {
            Some(&document) => document,
            None => {
                let file = File::open(path.as_ref())?;
                self.create_document(
                    Some(path.into()),
                    BufReader::new(file).lines().collect::<Result<_, _>>()?,
                )
            }
        };
        Ok(self.create_session(document))
    }

    pub fn close_file(&mut self, session: SessionId) {
        self.destroy_session(session);
    }

    pub fn set_max_column(&mut self, session: SessionId, max_column: usize) {
        if self.sessions[session.0].max_column == max_column {
            return;
        }
        self.sessions[session.0].max_column = max_column;
        for line in 0..self.line_count(self.document(session)) {
            self.update_indent_and_wraps(session, line);
        }
        self.update_y(session);
    }

    pub fn set_cursor(&mut self, session: SessionId, cursor: Point, affinity: Affinity) {
        self.sessions[session.0].selections.clear();
        self.sessions[session.0].selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
        });
        self.sessions[session.0].pending_selection = Some(0);
    }

    pub fn move_to(
        &mut self,
        session: SessionId,
        cursor: Point,
        affinity: Affinity,
    ) {
        let mut pending_selection = self.sessions[session.0].pending_selection.unwrap();
        self.sessions[session.0].selections[pending_selection].cursor = cursor;
        self.sessions[session.0].selections[pending_selection].affinity = affinity;
        while pending_selection > 0 {
            let prev_selection_index = pending_selection - 1;
            if self.sessions[session.0].selections[prev_selection_index]
                .should_merge(self.sessions[session.0].selections[pending_selection])
            {
                break;
            }
            self.sessions[session.0]
                .selections
                .remove(prev_selection_index);
            pending_selection -= 1;
        }
        while pending_selection + 1 < self.sessions[session.0].selections.len() {
            let next_selection_index = pending_selection + 1;
            if self.sessions[session.0].selections[pending_selection]
                .should_merge(self.sessions[session.0].selections[next_selection_index])
            {
                break;
            }
            self.sessions[session.0]
                .selections
                .remove(next_selection_index);
        }
        self.sessions[session.0].pending_selection = Some(pending_selection);
    }

    fn create_session(&mut self, document: DocumentId) -> SessionId {
        let line_count = self.documents[document.0].text.len();
        let session = SessionId(self.sessions.insert(Session {
            document,
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
            pending_selection: None,
        }));
        self.documents[document.0].sessions.insert(session);
        self.update_y(session);
        for line in 0..self.line_count(document) {
            self.update_column_count(session, line);
        }
        session
    }

    fn update_y(&mut self, session: SessionId) {
        let start = self.sessions[session.0].y.len();
        let line_count = self.line_count(self.document(session));
        if start == line_count + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            let line = self.line(session, start - 1);
            line.y() + line.height()
        };
        let mut ys = mem::take(&mut self.sessions[session.0].y);
        for block in self.blocks(session, start..line_count) {
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
        self.sessions[session.0].y = ys;
    }

    fn update_column_count(&mut self, session: SessionId, line: usize) {
        let mut column_count = 0;
        let mut column = 0;
        let line_ref = self.line(session, line);
        for wrapped in line_ref.wrappeds() {
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
                    column = line_ref.indent();
                }
            }
        }
        self.sessions[session.0].column_count[line] = Some(column_count.max(column));
    }

    fn update_indent_and_wraps(&mut self, session: SessionId, line: usize) {
        let (indent, wraps) = self.line(session, line).compute_indent_and_wraps(
            self.sessions[session.0].max_column,
            self.settings.tab_column_count,
        );
        self.sessions[session.0].wraps[line] = wraps;
        self.sessions[session.0].indent[line] = indent;
        self.update_column_count(session, line);
        self.sessions[session.0].y.truncate(line + 1);
    }

    fn destroy_session(&mut self, session: SessionId) {
        let document = self.document(session);
        self.documents[document.0].sessions.remove(&session);
        if self.documents[document.0].sessions.is_empty() {
            self.destroy_document(document);
        }
        self.sessions.remove(session.0);
    }

    fn create_document(&mut self, path: Option<PathBuf>, text: Vec<String>) -> DocumentId {
        let line_count = text.len();
        let document = DocumentId(self.documents.insert(Document {
            path,
            text,
            tokens: (0..line_count).map(|_| Vec::new()).collect(),
            inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
            block_inlays: Vec::new(),
            sessions: HashSet::new(),
        }));
        if let Some(path) = &self.documents[document.0].path {
            self.documents_by_path.insert(path.clone(), document);
        }
        document
    }

    fn destroy_document(&mut self, document: DocumentId) {
        if let Some(path) = &self.documents[document.0].path {
            self.documents_by_path.remove(path);
        }
        self.documents.remove(document.0);
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    pub(super) lines: Lines<'a>,
    pub(super) block_inlays: Iter<'a, (usize, BlockInlay)>,
    pub(super) line: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.line)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.line += 1;
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
    document: DocumentId,
    max_column: usize,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold: Vec<usize>,
    scale: Vec<f64>,
    wraps: Vec<Vec<usize>>,
    indent: Vec<usize>,
    selections: Vec<Selection>,
    pending_selection: Option<usize>,
}

#[derive(Clone, Debug)]
struct Document {
    path: Option<PathBuf>,
    text: Vec<String>,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    sessions: HashSet<SessionId>,
}
