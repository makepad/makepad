//! A host's live roots on the studio canvas: the running graph of a root
//! the host runs (an app's audio or picture graph), following every
//! revision the root accepts, and for a root that makes pictures the
//! pictures on the faces of its frame nodes.
//!
//! The host says which roots it runs through [`LiveRoots`]; a host with none
//! ([`NoLiveRoots`]) has no LIVE group. The faces read the host's existing
//! frame provider while they draw; nothing here renders or decodes. They are
//! bound only while the page shows and the root's installed generation runs
//! the accepted revision, and let go of their textures before they are
//! unmounted.

use super::canvas::StudioCanvasRef;
use makepad_flow::document::ident::Generation;
use makepad_flow::Graph;
use makepad_live_frame::{FrameBinding, FrameSource, LiveFrameViewWidgetRefExt};
use makepad_widgets::*;
use std::rc::Rc;
use std::sync::Arc;

/// Where a live root is on its way from source to a running instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// This build links no owner for the root; nothing was loaded.
    Unlinked,
    Loading,
    Instantiating,
    Running,
    Failed(String),
}

/// A live root's state as the LIVE group shows it.
#[derive(Clone, Debug)]
pub struct RootState {
    pub phase: Phase,
    /// The accepted revision.
    pub revision: u64,
    /// The generation of the installed graph, once it runs.
    pub installed: Option<Generation>,
}

/// The live roots a host runs, by index `0..count()`.
pub trait LiveRoots {
    fn count(&self) -> usize;
    /// `Audio`, `Video`, ...
    fn label(&self, root: usize) -> &str;
    /// The root's definition name, its card key.
    fn definition(&self, root: usize) -> &str;
    /// The build links an owner for it.
    fn linked(&self, root: usize) -> bool;
    /// It makes pictures its frame nodes show.
    fn makes_pictures(&self, root: usize) -> bool;
    fn state(&self, root: usize) -> RootState;
    /// The graph it runs.
    fn execution(&self, root: usize) -> Option<Arc<Graph>>;
    fn running_revision(&self, root: usize) -> Option<u64>;
    /// A frame node's picture source.
    fn frame_of(&self, node: &str) -> Option<(Rc<dyn FrameSource>, FrameBinding)>;
    fn is_stopping(&self) -> bool;
}

/// A host without live roots.
pub struct NoLiveRoots;

impl LiveRoots for NoLiveRoots {
    fn count(&self) -> usize { 0 }
    fn label(&self, _: usize) -> &str { "" }
    fn definition(&self, _: usize) -> &str { "" }
    fn linked(&self, _: usize) -> bool { false }
    fn makes_pictures(&self, _: usize) -> bool { false }
    fn state(&self, _: usize) -> RootState { RootState { phase: Phase::Unlinked, revision: 0, installed: None } }
    fn execution(&self, _: usize) -> Option<Arc<Graph>> { None }
    fn running_revision(&self, _: usize) -> Option<u64> { None }
    fn frame_of(&self, _: &str) -> Option<(Rc<dyn FrameSource>, FrameBinding)> { None }
    fn is_stopping(&self) -> bool { false }
}

#[derive(Default)]
pub(super) struct LiveView {
    /// The root on the canvas.
    root: Option<usize>,
    /// Its accepted graph as last put on the canvas.
    graph: Option<Arc<Graph>>,
    /// The generation the faces are bound to.
    generation: Option<Generation>,
    faces: Vec<WidgetRef>,
}

impl LiveView {
    /// Follow `root` on `canvas`: its graph whenever the accepted one
    /// changes, and its pictures while `shown`.
    pub fn sync(&mut self, cx: &mut Cx, canvas: &StudioCanvasRef, runtime: &dyn LiveRoots, root: usize, shown: bool) {
        if self.root != Some(root) {
            self.forget(cx, canvas);
            self.root = Some(root);
        }
        let graph = runtime.execution(root);
        if !same_graph(self.graph.as_ref(), graph.as_ref()) {
            self.release(cx, canvas);
            canvas.clear_run(cx);
            // Fitted when it first appears, left where the hand put it after.
            canvas.set_graph(cx, graph.as_deref(), self.graph.is_none());
            self.graph = graph;
        }
        let state = runtime.state(root);
        let wanted = pictures_wanted(
            runtime.makes_pictures(root), shown && !runtime.is_stopping(), &state.phase, state.installed,
            self.graph.as_ref().map(|graph| graph.revision), runtime.running_revision(root),
        );
        if wanted == self.generation {
            return;
        }
        self.release(cx, canvas);
        let (Some(generation), Some(graph)) = (wanted, self.graph.clone()) else { return };
        for node in frame_nodes(&graph) {
            let Some((source, binding)) = runtime.frame_of(&node) else { continue };
            let widget = cx.with_vm(|vm| {
                let value = script_eval!(vm, {
                    use mod.prelude.widgets.*
                    use mod.widgets.*
                    StageDebugFrameFace{width: Fill height: 112}
                });
                WidgetRef::script_from_value(vm, value)
            });
            widget.live_frame_view(cx, ids!(frame)).set_frame(cx, source, binding);
            canvas.set_face_widget(cx, &node, widget.clone());
            self.faces.push(widget);
        }
        self.generation = Some(generation);
    }

    /// Take the pictures down: each face lets go of its frame first, while
    /// its widget and recorded draw call are still mounted, which releases
    /// the texture that call holds even if the canvas never draws again.
    pub fn release(&mut self, cx: &mut Cx, canvas: &StudioCanvasRef) {
        self.generation = None;
        if self.faces.is_empty() {
            return;
        }
        for face in self.faces.drain(..) {
            face.live_frame_view(cx, ids!(frame)).clear_frame(cx);
        }
        canvas.clear_faces(cx);
    }

    /// No live root on the canvas any more (a preset or a run takes it).
    pub fn forget(&mut self, cx: &mut Cx, canvas: &StudioCanvasRef) {
        self.release(cx, canvas);
        self.root = None;
        self.graph = None;
    }
}

fn same_graph(a: Option<&Arc<Graph>>, b: Option<&Arc<Graph>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The generation whose pictures the faces may show: a picture root's,
/// while it is shown, running, installed, and running the revision of the
/// graph on the canvas. An accepted edit not yet installed has none, so an
/// older generation's pictures never sit on a newer graph.
fn pictures_wanted(pictures: bool, shown: bool, phase: &Phase, installed: Option<Generation>, revision: Option<u64>, running: Option<u64>) -> Option<Generation> {
    (pictures && shown && *phase == Phase::Running && revision.is_some() && running == revision).then_some(installed).flatten()
}

/// The nodes that make pictures: those with a `frame` output.
fn frame_nodes(graph: &Graph) -> Vec<String> {
    graph.nodes.iter()
        .filter(|node| node.outputs.iter().any(|port| port.name == "frame"))
        .map(|node| node.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_only_for_a_running_picture_root() {
        let generation = Some(Generation(3));
        assert_eq!(pictures_wanted(true, true, &Phase::Running, generation, Some(2), Some(2)), generation);
        // Hidden, another root, not running, nothing installed, no graph,
        // or an accepted edit still installing: no pictures.
        assert_eq!(pictures_wanted(true, false, &Phase::Running, generation, Some(2), Some(2)), None);
        assert_eq!(pictures_wanted(false, true, &Phase::Running, generation, Some(2), Some(2)), None);
        assert_eq!(pictures_wanted(true, true, &Phase::Instantiating, generation, Some(2), Some(2)), None);
        assert_eq!(pictures_wanted(true, true, &Phase::Running, None, Some(2), Some(2)), None);
        assert_eq!(pictures_wanted(true, true, &Phase::Running, generation, None, None), None);
        assert_eq!(pictures_wanted(true, true, &Phase::Running, generation, Some(3), Some(2)), None);
    }
}
