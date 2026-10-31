//! The live components the Flow app links. The flow core knows none of them:
//! a component registers its types and node descriptors like any provider,
//! names its live owners to the run scheduler, and hands the live host its
//! owners, its audio device and its frame producers through this module.
//!
//! Flow links no live owner of its own today: its packages run job flows
//! (generation, text, the recipe checks) and show their values on faces. A
//! package that names a live node type fails at evaluation with "node type
//! `X` is not registered", before any live plan; nothing falls back
//! silently.

use makepad_flow::document::Registry;
use makepad_flow::engine::providers::ExecutorRegistry;
use makepad_flow::live::{LiveOwner, OwnerRuntime};
pub use makepad_live_frame::{FrameProducer, ProducerTick};
use makepad_widgets::*;
use std::rc::Rc;
use std::sync::Arc;

/// The types and node descriptors of every linked live owner.
pub fn register(_registry: &mut Registry, _executors: &mut ExecutorRegistry) -> Result<(), String> {
    Ok(())
}

/// The audio device handle an audio owner hands out. No audio owner is
/// linked: there is nothing to install.
pub enum AudioOutput {}

/// Move the engine into the one output callback, once a revision with audio
/// nodes wants a device. Returns whether the callback was installed now.
pub fn install_audio(_cx: &mut Cx, audio: &AudioOutput) -> bool {
    match *audio {}
}

/// What the linked owners give the live host.
pub struct Owners {
    pub owners: Vec<(Arc<dyn LiveOwner>, Box<dyn OwnerRuntime>)>,
    pub audio: Option<AudioOutput>,
    pub producers: Vec<Rc<dyn FrameProducer>>,
}

/// Construct the linked owners.
pub fn owners(_cx: &mut Cx) -> Result<Owners, String> {
    Ok(Owners { owners: Vec::new(), audio: None, producers: Vec::new() })
}
