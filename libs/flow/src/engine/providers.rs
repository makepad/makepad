//! Job executor factories supplied by an embedding application.
//!
//! The document registry describes a node; this separate registry supplies
//! the implementation for its declared owner. Neither is mutable after an
//! engine starts. Factories run on a run lane and may capture host services,
//! but must create and start executors without blocking that lane.

use super::executors::Executor;
use crate::document::registry::BUILTIN_OWNER;
use crate::document::Registry;
use crate::{Graph, Node};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

pub trait ExecutorFactory: Send + Sync {
    fn create(&self, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String>;
}

impl<F> ExecutorFactory for F
where
    F: Fn(&Node, &Registry) -> Result<Box<dyn Executor>, String> + Send + Sync,
{
    fn create(&self, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String> {
        self(node, registry)
    }
}

#[derive(Clone, Default)]
pub struct ExecutorRegistry {
    factories: BTreeMap<String, Arc<dyn ExecutorFactory>>,
    /// Owners whose nodes run in a live partition (audio, frames, worlds).
    /// A run never executes them; it only has to know they are not jobs.
    live: BTreeSet<String>,
}

impl std::fmt::Debug for ExecutorRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutorRegistry")
            .field("owners", &self.factories.keys().collect::<Vec<_>>())
            .field("live", &self.live)
            .finish()
    }
}

impl ExecutorRegistry {
    /// One factory owns an owner key. Duplicate and builtin keys are errors,
    /// never replacements whose winner depends on registration order.
    pub fn register(
        &mut self,
        owner: impl Into<String>,
        factory: impl ExecutorFactory + 'static,
    ) -> Result<(), String> {
        let owner = owner.into();
        if owner == BUILTIN_OWNER {
            return Err(format!("executor owner `{owner}` is reserved for builtin jobs"));
        }
        if owner.is_empty() || owner.len() > 128 || owner.split('.').any(|part| {
            let mut bytes = part.bytes();
            !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
                || bytes.any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'_')
        }) {
            return Err(format!("executor owner `{owner}` is not a lower-case dotted identifier"));
        }
        if self.factories.contains_key(&owner) {
            return Err(format!("executor owner `{owner}` is already registered"));
        }
        if self.live.contains(&owner) {
            return Err(format!("owner `{owner}` is already registered as a live owner"));
        }
        self.factories.insert(owner, Arc::new(factory));
        Ok(())
    }

    /// Name an owner whose nodes a live host runs. An owner is a job owner
    /// or a live owner, never both: the partition of a graph is by owner.
    pub fn register_live(&mut self, owner: impl Into<String>) -> Result<(), String> {
        let owner = owner.into();
        if owner == BUILTIN_OWNER || self.factories.contains_key(&owner) {
            return Err(format!("owner `{owner}` already runs jobs and cannot also be live"));
        }
        self.live.insert(owner);
        Ok(())
    }

    pub fn is_live(&self, owner: &str) -> bool {
        self.live.contains(owner)
    }

    pub fn contains(&self, owner: &str) -> bool {
        self.factories.contains_key(owner)
    }

    pub(crate) fn create(&self, owner: &str, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String> {
        self.factories
            .get(owner)
            .ok_or_else(|| format!("node `{}` needs unregistered job owner `{owner}`", node.id))?
            .create(node, registry)
    }

    /// Check a lowered graph before any executor can start. The display
    /// domain never selects an executor, and a local/stream/frame value can
    /// never enter the artifact scheduler through an otherwise valid port.
    ///
    /// `selected` is what this run executes. A node of a live owner, and a
    /// boundary that carries a live type through a component, is part of the
    /// same revision but never of a run: it may be present, it may not be
    /// selected. A graph without live owners is checked exactly as before.
    pub(crate) fn validate_graph(&self, graph: &Graph, registry: &Registry, selected: &HashSet<String>) -> Result<(), String> {
        for node in &graph.nodes {
            let mut live = false;
            if node.kind != super::lower::BOUND_KIND {
                let descriptor = registry.descriptor(&node.type_name).ok_or_else(|| {
                    format!("node `{}` has unregistered type `{}`", node.id, node.type_name)
                })?;
                if descriptor.kind != node.kind {
                    return Err(format!("node `{}` does not match registered kind `{}`", node.id, descriptor.kind));
                }
                live = self.is_live(&descriptor.owner);
                if descriptor.owner != BUILTIN_OWNER && !live && !self.contains(&descriptor.owner) {
                    return Err(format!("node `{}` needs unregistered job owner `{}`", node.id, descriptor.owner));
                }
            }
            for (port, ty) in node.inputs.iter().map(|port| (&port.port, &port.ty))
                .chain(node.outputs.iter().map(|port| (&port.name, &port.ty)))
            {
                if let Err(error) = registry.types().require_artifact(ty) {
                    // Only a live node, or the boundary that carries its
                    // type through a component, may hold such a port.
                    if !live && node.kind != super::lower::BOUND_KIND {
                        return Err(format!("node `{}` port `{port}` cannot run as a job: {error}", node.id));
                    }
                    live = true;
                }
            }
            if live && selected.contains(&node.id) {
                return Err(format!(
                    "node `{}` runs in the live partition; a run cannot execute it or read its ports",
                    node.id
                ));
            }
        }
        Ok(())
    }
}
