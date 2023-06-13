use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    slice,
};

#[derive(Debug)]
pub struct State {
    view_id: usize,
    views: HashMap<ViewId, View>,
    model_id: usize,
    models: HashMap<ModelId, Model>,
    model_ids_by_path: HashMap<PathBuf, ModelId>,
}

impl State {
    pub fn new() -> Self {
        Self {
            view_id: 0,
            views: HashMap::new(),
            model_id: 0,
            models: HashMap::new(),
            model_ids_by_path: HashMap::new(),
        }
    }

    pub fn context(&self, view_id: ViewId) -> Context<'_> {
        let view = &self.views[&view_id];
        let model = &self.models[&view.model_id];
        Context { text: &model.text }
    }

    pub fn create_view<P>(&mut self, path: Option<P>) -> Result<ViewId, io::Error>
    where
        P: AsRef<Path> + Into<PathBuf>,
    {
        let model_id = if let Some(path) = path {
            if let Some(model_id) = self.model_ids_by_path.get(path.as_ref()).copied() {
                model_id
            } else {
                self.create_model(Some(path.into()))?
            }
        } else {
            self.create_model(None)?
        };
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        self.views.insert(view_id, View { model_id });
        self.models
            .get_mut(&model_id)
            .unwrap()
            .view_ids
            .insert(view_id);
        Ok(view_id)
    }

    pub fn destroy_view(&mut self, view_id: ViewId) {
        let model_id = self.views[&view_id].model_id;
        let view_ids = &mut self.models.get_mut(&model_id).unwrap().view_ids;
        view_ids.remove(&view_id);
        if view_ids.is_empty() {
            self.destroy_model(model_id);
        }
        self.views.remove(&view_id);
    }

    fn create_model(&mut self, path: Option<PathBuf>) -> Result<ModelId, io::Error> {
        use std::fs;

        let mut text: Vec<_> = if let Some(path) = &path {
            String::from_utf8_lossy(&fs::read(path)?).into_owned()
        } else {
            String::new()
        }
        .lines()
        .map(|string| string.to_string())
        .collect();
        if text.is_empty() {
            text.push(String::new());
        }
        let model_id = ModelId(self.model_id);
        self.model_id += 1;
        self.models.insert(
            model_id,
            Model {
                view_ids: HashSet::new(),
                path: path.clone(),
                text,
            },
        );
        if let Some(path) = path {
            self.model_ids_by_path.insert(path, model_id);
        }
        Ok(model_id)
    }

    fn destroy_model(&mut self, model_id: ModelId) {
        if let Some(path) = &self.models[&model_id].path {
            self.model_ids_by_path.remove(path);
        }
        self.models.remove(&model_id);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Debug)]
struct View {
    model_id: ModelId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ModelId(usize);

#[derive(Debug)]
struct Model {
    view_ids: HashSet<ViewId>,
    path: Option<PathBuf>,
    text: Vec<String>,
}

pub struct Context<'a> {
    text: &'a [String],
}

impl<'a> Context<'a> {
    pub fn lines(&self) -> Lines<'a> {
        Lines {
            text: self.text.iter(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.text.next()?)
    }
}
