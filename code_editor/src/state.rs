use {
    crate::{
        document, document::LineInlay, line, token::TokenInfo, Affinity, Context, Document,
        Selection, Settings,
    },
    std::{collections::HashMap, io, path::Path},
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
            &editor.text,
            &editor.token_infos,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut editor.text,
            &mut editor.token_infos,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
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
        let text: Vec<_> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| line.into())
            .collect();
        let line_count = text.len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                token_infos: (0..line_count).map(|_| [].into()).collect(),
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
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Vec<String>,
    token_infos: Vec<Vec<TokenInfo>>,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Affinity), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Affinity), document::Widget)>,
}
