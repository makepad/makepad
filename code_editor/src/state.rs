use {
    crate::{line, view, Bias, Selection, Settings, Text, Tokenizer, View, ViewMut},
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    session_id: usize,
    sessions: HashMap<SessionId, Session>,
    document: usize,
    docs: HashMap<DocumentId, Document>,
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

    pub fn session(&self, session: SessionId) -> View<'_> {
        let session = &self.sessions[&session];
        let doc = &self.docs[&session.document_id];
        View::new(
            &self.settings,
            &doc.text,
            &doc.tokenizer,
            &doc.inline_text_inlays,
            &doc.inline_widget_inlays,
            &session.soft_breaks,
            &session.start_column_after_wrap,
            &session.fold_column,
            &session.scale,
            &doc.block_widget_inlays,
            &session.summed_heights,
            &session.selections,
            session.latest_selection_index,
        )
    }

    pub fn view_mut(&mut self, session: SessionId) -> ViewMut<'_> {
        let view = self.sessions.get_mut(&session).unwrap();
        let doc = self.docs.get_mut(&view.document_id).unwrap();
        ViewMut::new(
            &mut self.settings,
            &mut view.max_column,
            &mut doc.text,
            &mut doc.tokenizer,
            &mut doc.inline_text_inlays,
            &mut doc.inline_widget_inlays,
            &mut view.soft_breaks,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut doc.block_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_session(&mut self, path: impl AsRef<Path>) -> io::Result<SessionId> {
        let document = self.open_document(path)?;
        let session = SessionId(self.session_id);
        self.session_id += 1;
        let line_count = self.docs[&document].text.as_lines().len();
        self.sessions.insert(
            session,
            Session {
                document_id: document,
                max_column: None,
                soft_breaks: (0..line_count).map(|_| [].into()).collect(),
                start_column_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
                latest_selection_index: 0,
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.view_mut(session).update_summed_heights();
        Ok(session)
    }

    fn open_document(&mut self, path: impl AsRef<Path>) -> io::Result<DocumentId> {
        use std::fs;

        let document = DocumentId(self.document);
        self.document += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.docs.insert(
            document,
            Document {
                text,
                tokenizer,
                inline_text_inlays: (0..line_count)
                    .map(|line| {
                        if line % 2 == 0 {
                            [
                                (20, "###".into()),
                                (40, "###".into()),
                                (60, "###".into()),
                                (80, "###".into()),
                            ]
                            .into()
                        } else {
                            [].into()
                        }
                    })
                    .collect(),
                inline_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                block_widget_inlays: [].into(),
            },
        );
        Ok(document)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Session {
    max_column: Option<usize>,
    document_id: DocumentId,
    soft_breaks: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DocumentId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Document {
    text: Text,
    tokenizer: Tokenizer,
    inline_text_inlays: Vec<Vec<(usize, String)>>,
    inline_widget_inlays: Vec<Vec<((usize, Bias), line::Widget)>>,
    block_widget_inlays: Vec<((usize, Bias), view::Widget)>,
}
