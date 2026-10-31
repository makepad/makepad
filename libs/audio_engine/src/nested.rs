//! Players inside a schedule: a node type registered at runtime whose
//! state is a whole player of its own (a score, with its loops and FX),
//! run as one op that follows the node's clock input and writes the node's
//! stereo span.
//!
//! - A crate registers the type's factory with [`register`] (and its
//!   descriptor in the flow registry, as any node type). `compile` asks
//!   the factory for the node's state, off the audio thread.
//! - Every pass the op hands the node the clock chunk its `clock` input
//!   reads (silence-stopped when unwired) and the span to fill.
//! - Across revisions a nested node of the same id and type carries over
//!   whole (it keeps playing what it plays), unless its [`NestedNode::key`]
//!   changed: then the new revision's takes over.
//!
//! Every nested node has a `clock` input, an `audio` output and a `peak`
//! probe; its own settings are its descriptor's parameters, read by its
//! factory.

use crate::schedule::{ClockChunk, Layout};
use makepad_flow::live::plan::{LiveNode, PlanError};
use std::sync::{Mutex, OnceLock};

/// A nested player's state as the audio thread runs it. `run` and
/// `running` are called on the audio thread: they never allocate, lock or
/// free.
pub trait NestedNode: Send {
    /// What it plays (its settings): a carried-over node whose key differs
    /// from the new revision's gives way to the new one.
    fn key(&self) -> u64;
    /// One pass: `left` and `right` (the pass's frames) filled following
    /// `clock`.
    fn run(&mut self, clock: &ClockChunk, left: &mut [f32], right: &mut [f32]);
    /// Whether it has something to play or take (a schedule holding one
    /// that runs does not rest).
    fn running(&self) -> bool;
}

/// Makes a node's state at prepare time: the node as planned, the device
/// rate, the revision's generation and the layout it is prepared against.
pub type Factory = fn(&LiveNode, u32, u64, Option<&Layout>) -> Result<Box<dyn NestedNode>, PlanError>;

fn factories() -> &'static Mutex<Vec<(String, Factory)>> {
    static FACTORIES: OnceLock<Mutex<Vec<(String, Factory)>>> = OnceLock::new();
    FACTORIES.get_or_init(Mutex::default)
}

/// Register `type_name`'s factory (again: the newest wins).
pub fn register(type_name: &str, factory: Factory) {
    let mut list = factories().lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|(name, _)| name != type_name);
    list.push((type_name.to_string(), factory));
}

/// The factory of a registered nested type.
pub fn factory(type_name: &str) -> Option<Factory> {
    factories().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(name, _)| name == type_name).map(|(_, factory)| *factory)
}
