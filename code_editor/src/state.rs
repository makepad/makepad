use {
    crate::{
        document, document::LineInlay, line, Context, Document, Bias, Selection, Settings, Text,
        Tokenizer,
    },
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    view_id: usize,
    views: HashMap<ViewId, View>,
    editor_id: usize,
    editors: HashMap<EditorId, Editor>,
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

    pub fn document(&self, view_id: ViewId) -> Document<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        Document::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.start_column_after_wrap,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
            view.latest_selection_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut self.settings,
            &mut view.max_column,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.as_lines().len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                max_column: None,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
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
        self.context(view_id).update_summed_heights();
        Ok(view_id)
    }

    fn open_editor(&mut self, path: impl AsRef<Path>) -> io::Result<EditorId> {
        use std::fs;

        let editor_id = EditorId(self.editor_id);
        self.editor_id += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                tokenizer,
                text_inlays: (0..line_count)
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
                line_inlays: [
                    (
                        10,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        20,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        30,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        40,
                        LineInlay::new("##################################################".into()),
                    ),
                ]
                .into(),
                line_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                document_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct View {
    max_column: Option<usize>,
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Bias), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Bias), document::Widget)>,
}
