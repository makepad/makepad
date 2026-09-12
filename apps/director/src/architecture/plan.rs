//! The architecture overview scene (phase 0 of the cached architecture
//! plans, `arch/<crate>/crate.toml`): a plan's lanes, nodes and edges with
//! their overview geometry, typed rather than pretending a memory account
//! is a Rust entity. Built on a worker from a parsed `code_arch::Plan`;
//! the view draws it, picks in it and lights neighbourhoods from it.
use super::layout::{layout, DesignLayout, DesignNodeSpec};
pub use makepad_code_arch::{EdgeKind as DesignEdgeKind, NodeKind as DesignNodeKind, Plan};
use makepad_code_arch::{parse, Limits};
use makepad_widgets::{DVec2, Rect};
use std::collections::HashSet;

/// A code reference of a node: `path[:line][::Symbol]`, with the content
/// hash the plan was generated against when the manifest has the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignRef {
    pub path: String,
    /// 1-based, as written in the plan.
    pub line: Option<u32>,
    pub symbol: Option<String>,
    pub hash: Option<[u8; 20]>,
}

impl DesignRef {
    /// `path::Symbol`, `path:line`, `path:line::Symbol` or `path`.
    pub fn parse(text: &str, files: &[makepad_code_arch::FileHash]) -> DesignRef {
        let (head, symbol) = match text.split_once("::") {
            Some((h, s)) if !s.is_empty() => (h, Some(s.to_string())),
            _ => (text, None),
        };
        let (path, line) = match head.rsplit_once(':') {
            Some((p, l)) if !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()) => (p.to_string(), l.parse::<u32>().ok()),
            _ => (head.to_string(), None),
        };
        let hash = files.iter().find(|f| f.path.to_string_lossy() == path).and_then(|f| hex20(&f.hash));
        DesignRef { path, line, symbol, hash }
    }
    /// The label a person reads: the path's file name, the line, the symbol.
    pub fn label(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        match (self.line, &self.symbol) {
            (Some(l), Some(s)) => format!("{name}:{l} · {s}"),
            (Some(l), None) => format!("{name}:{l}"),
            (None, Some(s)) => format!("{name} · {s}"),
            (None, None) => name.to_string(),
        }
    }
}

fn hex20(text: &str) -> Option<[u8; 20]> {
    if text.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesignLane {
    pub id: String,
    pub title: String,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesignNode {
    pub id: String,
    pub kind: DesignNodeKind,
    pub title: String,
    pub summary: String,
    pub story: String,
    pub refs: Vec<DesignRef>,
    pub budget: Option<String>,
    /// Index into `DesignScene::lanes`; none = the unlaned band.
    pub lane: Option<usize>,
    pub child: Option<String>,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesignEdge {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub kind: DesignEdgeKind,
    pub label: Option<String>,
    /// The orthogonal route in world units, source port → target port.
    pub points: Vec<DVec2>,
    pub back: bool,
}

/// One plan laid out for the overview.
#[derive(Clone, Debug, PartialEq)]
pub struct DesignScene {
    pub title: String,
    pub scope: String,
    pub prompt: String,
    pub generator: String,
    pub overview: String,
    pub file_count: usize,
    pub lanes: Vec<DesignLane>,
    /// The band the unlaned nodes share, when any exist.
    pub unlaned: Option<Rect>,
    /// Lanes in drawing order, top → bottom (`lanes.len()` = the unlaned band).
    pub lane_order: Vec<usize>,
    pub nodes: Vec<DesignNode>,
    pub edges: Vec<DesignEdge>,
    pub bounds: Rect,
}

impl DesignScene {
    /// The card under a world point, the topmost by index.
    pub fn node_at(&self, p: DVec2) -> Option<usize> {
        self.nodes.iter().rposition(|n| n.rect.contains(p))
    }
    /// The one-hop neighbourhood of a node: the nodes it touches (itself
    /// included) and the edges between them.
    pub fn neighbourhood(&self, i: usize) -> (HashSet<usize>, HashSet<usize>) {
        let mut nodes = HashSet::from([i]);
        let mut edges = HashSet::new();
        for (ei, e) in self.edges.iter().enumerate() {
            if e.from == i || e.to == i {
                nodes.insert(e.from);
                nodes.insert(e.to);
                edges.insert(ei);
            }
        }
        (nodes, edges)
    }
    /// The edges into and out of a node, as `(edge index, outgoing)`.
    pub fn edges_of(&self, i: usize) -> Vec<(usize, bool)> {
        self.edges.iter().enumerate().filter_map(|(ei, e)| if e.from == i { Some((ei, true)) } else if e.to == i { Some((ei, false)) } else { None }).collect()
    }
    /// The first sentence of the overview (one line for a shelf).
    pub fn overview_first_sentence(&self) -> String {
        first_sentence(&self.overview)
    }
    /// The lane band a node sits in.
    pub fn lane_rect_of(&self, i: usize) -> Option<Rect> {
        match self.nodes.get(i)?.lane {
            Some(l) => self.lanes.get(l).map(|l| l.rect),
            None => self.unlaned,
        }
    }
}

/// The first sentence of a prose block: up to the first `.`, `!` or `?`
/// followed by whitespace or the end, else the first line.
pub fn first_sentence(text: &str) -> String {
    let text = text.trim();
    let bytes = text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if matches!(b, b'.' | b'!' | b'?') && bytes.get(i + 1).is_none_or(|n| n.is_ascii_whitespace()) {
            return text[..=i].to_string();
        }
    }
    text.lines().next().unwrap_or("").to_string()
}

/// Parse a plan and lay it out for the overview (synchronous: call it on a
/// worker, never on the UI thread).
pub fn design_scene(text: &str, limits: &Limits) -> Result<(Plan, DesignScene), String> {
    let plan = parse(text, limits).map_err(|e| e.to_string())?;
    let scene = build_design(&plan);
    Ok((plan, scene))
}

/// Build the overview scene of a parsed plan: the lanes in the plan's id
/// order, the nodes and edges likewise, the layout from `layout`.
pub fn build_design(plan: &Plan) -> DesignScene {
    let lane_ids: Vec<&String> = plan.lanes.keys().collect();
    let lane_index = |id: &str| lane_ids.iter().position(|l| l.as_str() == id);
    let node_ids: Vec<&String> = plan.nodes.keys().collect();
    let node_index = |id: &str| node_ids.iter().position(|n| n.as_str() == id);
    let specs: Vec<DesignNodeSpec> = node_ids.iter().map(|id| DesignNodeSpec { lane: plan.nodes[*id].lane.as_deref().and_then(lane_index) }).collect();
    let edge_pairs: Vec<(usize, usize)> = plan.edges.values().map(|e| (node_index(&e.from).unwrap_or(0), node_index(&e.to).unwrap_or(0))).collect();
    let l: DesignLayout = layout(&specs, lane_ids.len(), &edge_pairs);
    let files = &plan.source.files;
    let nodes: Vec<DesignNode> = node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let n = &plan.nodes[*id];
            DesignNode {
                id: (*id).clone(),
                kind: n.kind,
                title: n.title.clone(),
                summary: n.summary.clone(),
                story: n.story.clone(),
                refs: n.refs.iter().map(|r| DesignRef::parse(r, files)).collect(),
                budget: n.budget.clone().filter(|b| !b.trim().is_empty()),
                lane: specs[i].lane,
                child: n.child.clone().filter(|c| !c.trim().is_empty()),
                rect: l.node_rects[i],
            }
        })
        .collect();
    let lanes: Vec<DesignLane> = lane_ids.iter().enumerate().map(|(i, id)| DesignLane { id: (*id).clone(), title: plan.lanes[*id].title.clone(), rect: l.lane_rects[i] }).collect();
    let unlaned = if specs.iter().any(|s| s.lane.is_none()) { l.lane_rects.get(lane_ids.len()).copied() } else { None };
    // the layout's routes are in `plan.edges` value order (the same
    // iteration as `edge_pairs`), filtered of self-edges by the layout:
    // rebuild the edge list keeping every plan edge, a self-edge with an
    // empty route
    let mut routes = l.routes.iter();
    let edges: Vec<DesignEdge> = plan
        .edges
        .iter()
        .map(|(id, e)| {
            let from = node_index(&e.from).unwrap_or(0);
            let to = node_index(&e.to).unwrap_or(0);
            let route = if from != to { routes.next() } else { None };
            DesignEdge { id: id.clone(), from, to, kind: e.kind, label: e.label.clone().filter(|s| !s.trim().is_empty()), points: route.map(|r| r.points.clone()).unwrap_or_default(), back: route.is_some_and(|r| r.back) }
        })
        .collect();
    DesignScene { title: plan.arch.title.clone(), scope: plan.arch.scope.clone(), prompt: plan.arch.prompt.clone(), generator: plan.arch.generator.clone(), overview: plan.overview.clone(), file_count: files.len(), lanes, unlaned, lane_order: l.lane_order, nodes, edges, bounds: l.bounds }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::dvec2;

    #[test]
    fn refs_parse_their_line_and_symbol() {
        let r = DesignRef::parse("platform/src/cx.rs::Cx", &[]);
        assert_eq!((r.path.as_str(), r.line, r.symbol.as_deref()), ("platform/src/cx.rs", None, Some("Cx")));
        let r = DesignRef::parse("platform/src/event/mod.rs:40", &[]);
        assert_eq!((r.path.as_str(), r.line, r.symbol.as_deref()), ("platform/src/event/mod.rs", Some(40), None));
        let r = DesignRef::parse("a/b.rs:7::T", &[]);
        assert_eq!((r.path.as_str(), r.line, r.symbol.as_deref()), ("a/b.rs", Some(7), Some("T")));
        assert_eq!(r.label(), "b.rs:7 · T");
    }

    #[test]
    fn the_first_sentence_stops_at_a_full_stop() {
        assert_eq!(first_sentence("One thing. Two things."), "One thing.");
        assert_eq!(first_sentence("v1.2 of a thing runs.\nMore."), "v1.2 of a thing runs.");
        assert_eq!(first_sentence("no stop\nsecond"), "no stop");
    }

    #[test]
    fn the_example_plan_builds_with_every_edge_routed() {
        let plan = makepad_code_arch::parse(makepad_code_arch::EXAMPLE, &makepad_code_arch::Limits::default()).expect("the shipped example parses");
        let scene = build_design(&plan);
        assert_eq!(scene.nodes.len(), plan.nodes.len());
        assert_eq!(scene.edges.len(), plan.edges.len());
        for e in &scene.edges {
            if e.from != e.to {
                assert!(e.points.len() >= 2, "edge {} unrouted", e.id);
                let (a, b) = (scene.nodes[e.from].rect, scene.nodes[e.to].rect);
                let first = e.points[0];
                let last = *e.points.last().unwrap();
                assert!(a.add_margin(dvec2(1.0, 1.0)).contains(first), "edge {} does not start at its source", e.id);
                assert!(b.add_margin(dvec2(1.0, 1.0)).contains(last), "edge {} does not end at its target", e.id);
            }
        }
        for (i, a) in scene.nodes.iter().enumerate() {
            for b in scene.nodes.iter().skip(i + 1) {
                assert!(!a.rect.intersects(b.rect), "{} overlaps {}", a.id, b.id);
            }
        }
        let (nodes, edges) = scene.neighbourhood(0);
        assert!(nodes.contains(&0));
        assert_eq!(edges.len(), scene.edges_of(0).len());
    }
}
