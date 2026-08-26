//! Loading. A [`Loader`] turns a path into a [`Document`]; the shell runs it
//! on a worker thread and relays [`LoadProgress`] to the UI. This module owns
//! only the format-neutral seam; implementations live in `libs/loaders/*` and
//! can be registered without changing the shell.

use crate::document::Document;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum LoadProgress {
    /// File opened, container being read.
    Opening,
    /// Format parse, 0..1.
    Parsing(f32),
    /// Meshes decoded so far.
    Meshing { done: usize, total: usize },
    /// Scene build stage (normalise / merge / bvh / snapshot), 0..1.
    Building { stage: &'static str, fraction: f32 },
    Done,
}

/// Asked between (and inside) stages: `true` means a newer open superseded this
/// load and the worker should stop and return [`LoadError::Cancelled`].
/// A reader that ignores it still works — it just finishes work nobody wants.
pub type LoadCancel<'a> = &'a dyn Fn() -> bool;

#[derive(Clone, Debug, PartialEq)]
pub enum LoadError {
    NotFound(String),
    Unsupported(String),
    Corrupt(String),
    Io(String),
    Cancelled,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound(p) => write!(f, "file not found: {p}"),
            LoadError::Unsupported(w) => write!(f, "unsupported: {w}"),
            LoadError::Corrupt(w) => write!(f, "corrupt file: {w}"),
            LoadError::Io(w) => write!(f, "io error: {w}"),
            LoadError::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// A format reader. Implementations must be cheap to construct and `Send`
/// because they run on the loader thread. `progress` is called from that
/// thread; the callback is responsible for forwarding to the UI.
pub trait Loader: Send + Sync {
    /// Lower-case extensions this source handles, without the dot.
    fn extensions(&self) -> &[&str];

    /// Cheap content sniff used when a path has no useful extension.
    fn probe(&self, bytes: &[u8]) -> bool;

    /// Read the file. `cancel` is polled at every stage boundary (and inside
    /// long ones) so opening a second model does not wait for the first to
    /// finish; a reader that ignores it still works.
    fn load_cancellable(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(LoadProgress),
        cancel: LoadCancel,
    ) -> Result<Document, LoadError>;

    /// Read the file with no way to give up — tests and one-shot tools.
    fn load(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(LoadProgress),
    ) -> Result<Document, LoadError> {
        self.load_cancellable(path, progress, &|| false)
    }
}

/// Source that yields the built-in procedural demo house. Handy for the
/// skeleton, tests and the empty-state "Open demo" button.
#[derive(Default)]
pub struct DemoLoader;

impl Loader for DemoLoader {
    fn extensions(&self) -> &[&str] {
        &["demo"]
    }

    fn probe(&self, _bytes: &[u8]) -> bool {
        false
    }

    fn load_cancellable(
        &self,
        _path: &Path,
        progress: &mut dyn FnMut(LoadProgress),
        _cancel: LoadCancel,
    ) -> Result<Document, LoadError> {
        progress(LoadProgress::Opening);
        let model = Document::from_model_data(crate::model::demo::demo_house());
        progress(LoadProgress::Done);
        Ok(model)
    }
}
