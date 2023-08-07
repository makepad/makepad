mod blocks;
mod inlines;
mod line;
mod lines;
mod view;
mod wrappeds;

pub use self::{
    blocks::{Block, Blocks},
    inlines::{Inline, Inlines},
    line::Line,
    lines::Lines,
    view::{View, ViewMut},
    wrappeds::{Wrapped, Wrappeds},
};

use {
    crate::{
        inlays::{BlockInlay, InlineInlay},
        Settings, Token,
    },
    std::{
        collections::{HashMap, HashSet},
        fs::File,
        io,
        io::{BufRead, BufReader},
        path::{Path, PathBuf},
    },
};

#[derive(Clone, Debug, Default)]
pub struct State {
    settings: Settings,
    session_id: usize,
    sessions: HashMap<SessionId, Session>,
    document_id: usize,
    documents: HashMap<DocumentId, Document>,
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

    pub fn view(&self, session_id: SessionId) -> View<'_> {
        let session = &self.sessions[&session_id];
        let document = &self.documents[&session.document_id];
        View {
            settings: &self.settings,
            session,
            document,
        }
    }

    pub fn view_mut(&mut self, session_id: SessionId) -> ViewMut<'_> {
        let session = self.sessions.get_mut(&session_id).unwrap();
        let document = &self.documents[&session.document_id];
        ViewMut {
            settings: &self.settings,
            session,
            document,
        }
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
        let line_count = self.documents[&document_id].text.len();
        let session_id = SessionId(self.session_id);
        self.session_id += 1;
        self.sessions.insert(
            session_id,
            Session {
                document_id,
                max_column: usize::MAX,
                y: Vec::new(),
                column_count: (0..line_count).map(|_| None).collect(),
                fold: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                wraps: (0..line_count).map(|_| Vec::new()).collect(),
                indent: (0..line_count).map(|_| 0).collect(),
            },
        );
        let document = self.documents.get_mut(&document_id).unwrap();
        document.session_ids.insert(session_id);
        let mut view = self.view_mut(session_id);
        view.update_y();
        for index in 0..view.line_count() {
            view.update_column_count(index);
        }
        session_id
    }

    fn destroy_session(&mut self, session_id: SessionId) {
        let document_id = self.sessions[&session_id].document_id;
        let document = self.documents.get_mut(&document_id).unwrap();
        document.session_ids.remove(&session_id);
        if document.session_ids.is_empty() {
            self.destroy_document(document_id);
        }
        self.sessions.remove(&session_id);
    }

    fn create_document(&mut self, path: Option<PathBuf>, text: Vec<String>) -> DocumentId {
        let line_count = text.len();
        let document_id = DocumentId(self.document_id);
        self.document_id += 1;
        self.documents.insert(
            document_id,
            Document {
                path,
                text,
                tokens: (0..line_count).map(|_| Vec::new()).collect(),
                inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
                block_inlays: Vec::new(),
                session_ids: HashSet::new(),
            },
        );
        if let Some(path) = &self.documents[&document_id].path {
            self.document_ids.insert(path.clone(), document_id);
        }
        document_id
    }

    fn destroy_document(&mut self, document_id: DocumentId) {
        if let Some(path) = &self.documents[&document_id].path {
            self.document_ids.remove(path);
        }
        self.documents.remove(&document_id);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(usize);

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
