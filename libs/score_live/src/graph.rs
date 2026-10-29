//! The score as a graph: nodes, ports and wires for the Graph view, and the
//! wiring gestures written back into the source. The graph is the document
//! read another way: instruments feed loops (`play:`), loops and FX feed FX
//! (`in: [...]`, `key:`), and whatever nothing consumes feeds the master.

use crate::model::ScoreDoc;
use crate::source::{self, Splice};

#[derive(Clone, Debug, PartialEq)]
pub struct GPort {
    pub name: String,
    /// `inst`, `audio` or `clock`: what may connect to what.
    pub kind: &'static str,
    pub many: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GNode {
    pub id: String,
    pub type_name: String,
    /// `clock`, `inst`, `loop`, `fx`, `section`, `master`.
    pub role: &'static str,
    pub inputs: Vec<GPort>,
    pub outputs: Vec<GPort>,
    pub at: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GEdge {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
    /// The entry's place in an ordered input.
    pub key: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreGraph {
    pub nodes: Vec<GNode>,
    pub edges: Vec<GEdge>,
}

/// The id of the implicit master node.
pub const MASTER: &str = "master";

fn port(name: &str, kind: &'static str, many: bool) -> GPort {
    GPort { name: name.into(), kind, many }
}

pub fn project(doc: &ScoreDoc) -> ScoreGraph {
    let mut graph = ScoreGraph::default();
    let at = |name: &str| doc.positions.iter().find(|(n, _)| n == name).map(|(_, p)| *p);
    let mut consumed: Vec<&str> = Vec::new();
    for (name, ty) in &doc.order {
        let (role, inputs, outputs) = match ty.as_str() {
            "Clock" => ("clock", vec![], vec![port("clock", "clock", false)]),
            "Synth" | "Drums" | "Sampler" | "Ironfish" | "Morph" | "Shader" | "Voice" | "Instrument" => ("inst", vec![], vec![port("inst", "inst", false)]),
            "Loop" | "Stinger" => ("loop", vec![port("play", "inst", false)], vec![port("audio", "audio", false)]),
            "Section" => ("section", vec![], vec![]),
            _ => {
                let duck = ty == "Duck" || ty == "Sidechain";
                let mut inputs = vec![port("in", "audio", true)];
                if duck {
                    inputs.push(port("key", "audio", false));
                }
                ("fx", inputs, vec![port("audio", "audio", false)])
            }
        };
        graph.nodes.push(GNode { id: name.clone(), type_name: ty.clone(), role, inputs, outputs, at: at(name) });
    }
    for l in &doc.loops {
        if let Some(play) = &l.play {
            graph.edges.push(GEdge { from: play.clone(), from_port: "inst".into(), to: l.name.clone(), to_port: "play".into(), key: None });
        }
    }
    for fx in &doc.fx {
        for (i, input) in fx.inputs.iter().enumerate() {
            graph.edges.push(GEdge { from: input.clone(), from_port: "audio".into(), to: fx.name.clone(), to_port: "in".into(), key: Some(i as u64) });
            consumed.push(input);
        }
        if let Some(side) = &fx.side {
            graph.edges.push(GEdge { from: side.clone(), from_port: "audio".into(), to: fx.name.clone(), to_port: "key".into(), key: None });
        }
    }
    // Aux sends: an edge into the FX that leaves the loop on the master too.
    for l in &doc.loops {
        for (i, (fx, _)) in l.sends.iter().enumerate() {
            graph.edges.push(GEdge { from: l.name.clone(), from_port: "audio".into(), to: fx.clone(), to_port: "in".into(), key: Some(1000 + i as u64) });
        }
    }
    let mut master = GNode { id: MASTER.into(), type_name: "Master".into(), role: "master", inputs: vec![port("in", "audio", true)], outputs: vec![], at: None };
    let mut key = 0;
    for name in doc.loops.iter().map(|l| &l.name).chain(doc.fx.iter().map(|f| &f.name)) {
        if !consumed.contains(&name.as_str()) {
            graph.edges.push(GEdge { from: name.clone(), from_port: "audio".into(), to: MASTER.into(), to_port: "in".into(), key: Some(key) });
            key += 1;
        }
    }
    master.inputs[0].many = true;
    graph.nodes.push(master);
    graph
}

/// Which input ports an output may connect to.
pub fn compatible(graph: &ScoreGraph) -> Vec<((String, String), Vec<(String, String)>)> {
    let mut out = Vec::new();
    for from in &graph.nodes {
        for output in &from.outputs {
            let targets = graph.nodes.iter().filter(|to| to.id != from.id && to.role != "master")
                .flat_map(|to| to.inputs.iter().filter(|input| input.kind == output.kind).map(|input| (to.id.clone(), input.name.clone())))
                .collect();
            out.push(((from.id.clone(), output.name.clone()), targets));
        }
    }
    out
}

/// The names in a list field's value (`[a b]`), or the one bare name.
fn names_in(text: &str) -> Vec<String> {
    text.trim().trim_start_matches('[').trim_end_matches(']').split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn list_text(names: &[String]) -> String {
    format!("[{}]", names.join(" "))
}

/// Connect `from` into `to`'s port, as a source change.
pub fn connect(src: &str, from: &str, to: &str, to_port: &str) -> Result<String, String> {
    let outline = source::outline(src);
    let node = outline.node(to).ok_or_else(|| format!("no node `{to}`"))?;
    let splice: Splice = match to_port {
        "play" | "key" => source::set_field(src, to, to_port, from)?,
        "in" => {
            let mut names = node.field("in").map_or_else(Vec::new, |f| names_in(f.value.text(src)));
            if names.iter().any(|n| n == from) {
                return Err(format!("`{from}` already feeds `{to}`"));
            }
            names.push(from.to_string());
            source::set_field(src, to, "in", &list_text(&names))?
        }
        other => return Err(format!("`{to}` has no input `{other}`")),
    };
    Ok(source::apply(src, &splice))
}

/// Remove a wire: the entry `key` of an ordered input, or the one input.
pub fn disconnect(src: &str, to: &str, to_port: &str, key: Option<u64>) -> Result<String, String> {
    let outline = source::outline(src);
    let node = outline.node(to).ok_or_else(|| format!("no node `{to}`"))?;
    let splice = match to_port {
        "play" | "key" => source::remove_field(src, to, to_port)?,
        "in" => {
            let mut names = node.field("in").map_or_else(Vec::new, |f| names_in(f.value.text(src)));
            let index = key.map(|k| k as usize).unwrap_or(names.len().saturating_sub(1));
            if index >= names.len() {
                return Err(format!("`{to}.in` has no entry {index}"));
            }
            names.remove(index);
            source::set_field(src, to, "in", &list_text(&names))?
        }
        other => return Err(format!("`{to}` has no input `{other}`")),
    };
    Ok(source::apply(src, &splice))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 4 code: fn(i) { play(60) sleep(1) }}\nlet b = Loop{hits: \"x.x.\"}\nlet verb = Reverb{in: [a] mix: 0.3}\nScore{clock s a b verb}\n";

    #[test]
    fn the_graph_is_the_document_and_wiring_writes_it() {
        let doc = crate::model::ScoreDoc::load(SRC).unwrap();
        let g = project(&doc);
        let edges: Vec<_> = g.edges.iter().map(|e| format!("{}.{}>{}.{}", e.from, e.from_port, e.to, e.to_port)).collect();
        assert_eq!(edges, ["s.inst>a.play", "a.audio>verb.in", "b.audio>master.in", "verb.audio>master.in"]);
        let s2 = connect(SRC, "b", "verb", "in").unwrap();
        assert!(s2.contains("Reverb{in: [a b] mix: 0.3}"), "{s2}");
        let g2 = project(&crate::model::ScoreDoc::load(&s2).unwrap());
        assert!(!g2.edges.iter().any(|e| e.from == "b" && e.to == MASTER));
        let s3 = disconnect(&s2, "verb", "in", Some(0)).unwrap();
        assert!(s3.contains("Reverb{in: [b] mix: 0.3}"), "{s3}");
        let s4 = disconnect(SRC, "a", "play", None).unwrap();
        assert!(s4.contains("Loop{beats: 4") || s4.contains("Loop{ beats: 4"), "{s4}");
        assert!(compatible(&g).iter().any(|((n, p), t)| n == "s" && p == "inst" && t.contains(&("b".into(), "play".into()))));
    }
}
