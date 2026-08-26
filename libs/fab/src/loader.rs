//! Asynchronous loader registry owned by the frontend.

use crate::api::*;
use crate::model::{DemoLoader, Loader};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Registered format loaders plus cancellation state for the active load.
pub struct LoadCoordinator {
    seq: Arc<AtomicU64>,
    loaders: Vec<Arc<dyn Loader>>,
}

impl Default for LoadCoordinator {
    fn default() -> Self {
        let mut coordinator = Self {
            seq: Arc::new(AtomicU64::new(0)),
            loaders: Vec::new(),
        };
        coordinator.register(DemoLoader);
        coordinator
    }
}

impl LoadCoordinator {
    pub fn register<L: Loader + 'static>(&mut self, loader: L) {
        self.loaders.push(Arc::new(loader));
    }

    pub fn extensions(&self) -> Vec<&str> {
        self.loaders
            .iter()
            .flat_map(|loader| loader.extensions().iter().copied())
            .collect()
    }

    fn loader_for(&self, path: &Path) -> Option<Arc<dyn Loader>> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        if let Some(loader) = self.loaders
            .iter()
            .find(|loader| {
                extension.as_deref().is_some_and(|extension| {
                    loader.extensions().iter().any(|candidate| *candidate == extension)
                })
            })
            .cloned()
        {
            return Some(loader);
        }
        let mut prefix = [0_u8; 4096];
        let count = std::fs::File::open(path).ok()?.read(&mut prefix).ok()?;
        self.loaders
            .iter()
            .find(|loader| loader.probe(&prefix[..count]))
            .cloned()
    }

    pub fn open(&mut self, cx: &mut Cx, path: PathBuf) {
        let Some(loader) = self.loader_for(&path) else {
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_string();
            Cx::post_action(ShellAction::LoadFailed {
                path,
                error: format!("no loader registered for .{extension}"),
            });
            return;
        };

        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let seq_ref = self.seq.clone();
        Cx::post_action(ShellAction::LoadStarted(path.clone()));
        cx.spawn_thread(move || {
            let guard_path = path.clone();
            let guard_seq = seq_ref.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                Self::load_on_worker(loader, path, seq, seq_ref)
            }));
            if let Err(payload) = result {
                if guard_seq.load(Ordering::SeqCst) == seq {
                    Cx::post_action(ShellAction::LoadFailed {
                        path: guard_path,
                        error: format!("loader crashed: {}", panic_message(&payload)),
                    });
                }
            }
        });
    }

    fn load_on_worker(
        loader: Arc<dyn Loader>,
        path: PathBuf,
        seq: u64,
        seq_ref: Arc<AtomicU64>,
    ) {
        let still_current = || seq_ref.load(Ordering::SeqCst) == seq;
        let cancelled = || !still_current();
        let mut last = String::new();
        let mut progress = |value: LoadProgress| {
            if !still_current() {
                return;
            }
            let key = format!("{value:?}");
            if key != last {
                last = key;
                Cx::post_action(ShellAction::LoadProgress(value));
            }
        };
        let document = match loader.load_cancellable(&path, &mut progress, &cancelled) {
            Ok(document) => document,
            Err(error) => {
                if still_current() && error != LoadError::Cancelled {
                    Cx::post_action(ShellAction::LoadFailed {
                        path,
                        error: error.to_string(),
                    });
                }
                return;
            }
        };
        if !still_current() {
            return;
        }
        let mut last_stage = "";
        let mut last_step = -1i32;
        let scene = Scene::from_document_with(document, &mut |stage, fraction| {
            if !still_current() {
                return;
            }
            let step = (fraction * 20.0) as i32;
            if stage != last_stage || step != last_step {
                last_stage = stage;
                last_step = step;
                Cx::post_action(ShellAction::LoadProgress(LoadProgress::Building {
                    stage,
                    fraction,
                }));
            }
        });
        if !still_current() {
            return;
        }
        Cx::post_action(ShellAction::LoadProgress(LoadProgress::Building {
            stage: "snapshot",
            fraction: 0.0,
        }));
        let _ = scene.snapshot();
        if !still_current() {
            return;
        }
        // The tour site's voxel/room/portal pass is the authoritative
        // front-door analysis. Pay for it once here, never on the UI thread
        // when Walk is selected and never inside a frame query.
        Cx::post_action(ShellAction::LoadProgress(LoadProgress::Building {
            stage: "entrance analysis",
            fraction: 0.0,
        }));
        let walk_analysis = Arc::new(WalkSceneAnalysis::analyse(
            &scene,
            crate::nav::walk::EYE_HEIGHT,
        ));
        if still_current() {
            let scene = Arc::new(scene);
            Cx::post_action(ShellAction::LoadProgress(LoadProgress::Done));
            // FIFO matters: `apply_core` installs the generation before
            // accepting its cache and tags the latter to scene_revision.
            Cx::post_action(ShellAction::Loaded(scene));
            Cx::post_action(ShellAction::WalkAnalysisReady(walk_analysis));
        }
    }

    pub fn open_demo(&mut self, cx: &mut Cx) {
        self.open(cx, PathBuf::from("house.demo"));
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

pub fn apply(
    cx: &mut Cx,
    loader: &mut LoadCoordinator,
    _state: &mut AppState,
    action: &ShellAction,
) -> bool {
    match action {
        ShellAction::OpenFile(path) => {
            loader.open(cx, path.clone());
            true
        }
        ShellAction::OpenDemo => {
            loader.open_demo(cx);
            true
        }
        ShellAction::Loaded(scene) => {
            let mut message = format!(
                "{} — {} elements, {} tris, {} batches, built in {:.0} ms",
                scene.name,
                scene.stats.elements,
                scene.stats.triangles,
                scene.stats.batches,
                scene.stats.build_ms
            );
            if scene.metadata.iter().any(|(key, _)| key == "Warning") {
                message.push_str("  ·  partial decode");
            }
            if scene.materials_are_derived {
                message.push_str("  ·  materials derived from element class");
            }
            Cx::post_action(ShellAction::StatusMessage(message));
            false
        }
        _ => false,
    }
}
