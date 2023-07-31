use {
    crate::{
        line, view, view::LineInlay, Bias, ViewMut, Sel, Settings, Text, Tokenizer, View,
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
    views: HashMap<ViewId, Session>,
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

    pub fn document(&self, view_id: ViewId) -> View<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        View::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.inline_text_inlays,
            &editor.inline_widget_inlays,
            &view.soft_breaks,
            &view.start_col_after_wrap,
            &view.fold_col,
            &view.scale,
            &editor.line_inlays,
            &editor.block_widget_inlays,
            &view.summed_heights,
            &view.sels,
            view.latest_sel_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> ViewMut<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        ViewMut::new(
            &mut self.settings,
            &mut view.max_col,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.inline_text_inlays,
            &mut editor.inline_widget_inlays,
            &mut view.soft_breaks,
            &mut view.start_col_after_wrap,
            &mut view.fold_col,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.block_widget_inlays,
            &mut view.summed_heights,
            &mut view.sels,
            &mut view.latest_sel_index,
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
            Session {
                editor_id,
                max_col: None,
                soft_breaks: (0..line_count).map(|_| [].into()).collect(),
                start_col_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_col: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                sels: [Sel::default()].into(),
                latest_sel_index: 0,
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
                inline_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                block_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Session {
    max_col: Option<usize>,
    editor_id: EditorId,
    fold_col: Vec<usize>,
    scale: Vec<f64>,
    soft_breaks: Vec<Vec<usize>>,
    start_col_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    sels: Vec<Sel>,
    latest_sel_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    inline_text_inlays: Vec<Vec<(usize, String)>>,
    inline_widget_inlays: Vec<Vec<((usize, Bias), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    block_widget_inlays: Vec<((usize, Bias), view::Widget)>,
}
