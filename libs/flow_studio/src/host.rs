//! What the app hosting the studio gives it: where results go, the node
//! types its engine runs, its own presets and where it says results land.
//!
//! One studio host per process, installed once before the studio first
//! shows ([`install`]). Without one the studio runs on [`default_host`]: a
//! [`FolderStore`] under `~/.makepad/flow/results`, the built-in node types
//! plus the recipe checks, and generic result places.

use crate::presets::Destination;
use makepad_flow::document::Registry;
use makepad_flow::engine::providers::ExecutorRegistry;
use makepad_flow::engine::results::{FolderStore, ResultStore};
use makepad_flow::templates::Template;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// Where a result of one destination shows, and the tags that put it there.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub label: &'static str,
    pub tags: &'static [&'static str],
}

/// What opening the result store gave.
pub enum Opening {
    Open(Arc<dyn ResultStore>),
    /// Not up yet (it may start with the app): asked again shortly.
    Starting(String),
    Failed(String),
}

pub struct StudioHost {
    /// Opens the result store, on the studio worker.
    pub open_results: Box<dyn Fn() -> Opening + Send + Sync>,
    /// The node types and executors the studio engine runs.
    pub registries: fn() -> Result<(Arc<Registry>, Arc<ExecutorRegistry>), String>,
    /// The engine's value folder.
    pub values: PathBuf,
    /// The host's own presets, beside the bundled ones (group `Studio`).
    pub presets: Vec<Template>,
    /// Where each kind of result lands.
    pub place: fn(Destination) -> Place,
    /// What the results land in, for the page's status (`the results folder`).
    pub library: &'static str,
}

static HOST: OnceLock<StudioHost> = OnceLock::new();

/// Install the process's studio host. Fails (handing it back) once one is
/// installed or the studio already started on the default.
pub fn install(host: StudioHost) -> Result<(), StudioHost> {
    HOST.set(host)
}

pub fn host() -> &'static StudioHost {
    HOST.get_or_init(default_host)
}

/// A hidden (test or agent) instance: its folders are its own, never the
/// person's.
pub fn hidden() -> bool {
    std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some_and(|value| !value.is_empty() && value != "0")
}

/// The Flow app's folder `name`: `~/.makepad/flow/<name>`, or a per-process
/// temporary one for a hidden instance.
pub fn flow_dir(name: &str) -> PathBuf {
    if hidden() {
        return std::env::temp_dir().join(format!("flow-hidden-{}", std::process::id())).join(name);
    }
    makepad_ai_hub::home::makepad_home().join("flow").join(name)
}

pub fn default_host() -> StudioHost {
    StudioHost {
        open_results: Box::new(|| match FolderStore::open(flow_dir("results"), "flows") {
            Ok(store) => Opening::Open(Arc::new(store)),
            Err(error) => Opening::Failed(error),
        }),
        registries: default_registries,
        values: flow_dir("values"),
        presets: Vec::new(),
        place: default_place,
        library: "the results folder",
    }
}

/// The node types of a host without components of its own.
pub fn default_registries() -> Result<(Arc<Registry>, Arc<ExecutorRegistry>), String> {
    crate::nodes::registries_with(|_, _| Ok(()))
}

pub fn default_place(destination: Destination) -> Place {
    let (label, tags): (&'static str, &'static [&'static str]) = match destination {
        Destination::Image => ("IMAGES", &["image"]),
        Destination::Video => ("VIDEO", &["video"]),
        Destination::Music => ("MUSIC", &["music"]),
        Destination::Voice => ("VOICE", &["speech", "voiceover"]),
        Destination::Sound => ("AUDIO", &["sfx"]),
        Destination::Stem => ("AUDIO · STEMS", &["stem"]),
        Destination::Mesh => ("3D", &["mesh"]),
        Destination::World => ("WORLDS", &["world"]),
        Destination::Data => ("DATA", &["text"]),
    };
    Place { label, tags }
}
