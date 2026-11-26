//! The score as a graph: nodes, ports and wires for the Graph view, and the
//! wiring gestures written back into the source. The graph is the document
//! read another way: instruments feed loops (`play:`), loops and FX feed FX
//! (`in: [...]`, `key:`), note streams (MIDI in through Arp, Chord,
//! Transpose, Velocity) feed each other and loops (`in: keys`), and
//! whatever nothing consumes feeds the master.

use crate::model::ScoreDoc;
use crate::source;
use makepad_edit_values::Splice;

#[derive(Clone, Debug, PartialEq)]
pub struct GPort {
    pub name: String,
    /// `inst`, `audio`, `notes` or `clock`: what may connect to what.
    pub kind: &'static str,
    pub many: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GNode {
    pub id: String,
    pub type_name: String,
    /// `clock`, `inst`, `stream`, `loop`, `fx`, `section`, `master`.
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
        let (role, mut inputs, mut outputs) = match ty.as_str() {
            "Clock" => ("clock", vec![], vec![port("clock", "clock", false)]),
            t if crate::instruments::INSTRUMENT_TYPES.contains(&t) => ("inst", vec![], vec![port("inst", "inst", false)]),
            "Loop" | "Stinger" => ("loop", vec![port("play", "inst", false), port("in", "notes", false)], vec![port("audio", "audio", false)]),
            "Section" => ("section", vec![], vec![]),
            "MidiIn" => ("stream", vec![], vec![port("notes", "notes", false)]),
            t if crate::model::STREAM_TYPES.contains(&t) => ("stream", vec![port("in", "notes", false)], vec![port("notes", "notes", false)]),
            _ => {
                let kind = makepad_score_player::FxKind::parse(ty);
                let duck = matches!(kind, Some(makepad_score_player::FxKind::Duck | makepad_score_player::FxKind::Comp));
                let mut inputs = if ty == "ShaderControl" { vec![] } else { vec![port("in", "audio", true)] };
                if duck {
                    inputs.push(port("key", "audio", false));
                }
                if let Some(kind) = kind {
                    inputs.extend(kind.params().iter().map(|(name, ..)| port(name, "audio", true)));
                }
                let outputs = if ty == "ShaderControl" { vec![] } else { vec![port("audio", "audio", false)] };
                ("fx", inputs, outputs)
            }
        };
        let playing = doc.loops.iter().find(|l| &l.name == name).and_then(|l| l.play.as_deref());
        if let Some(shader) = doc.shader(playing.unwrap_or(name)) {
            if playing.is_none() {
                inputs.extend((0..shader.shader.param_count()).map(|i| port(shader.shader.param_view(i).name, "audio", true)));
            }
            for i in 0..shader.shader.port_count() {
                let p = shader.shader.port_view(i);
                if p.output && role != "inst" {
                    let name = if playing.is_some() { makepad_audio_engine::nodes::instrument_output(p.name) } else { p.name };
                    outputs.push(port(name, "audio", false));
                }
                if !p.output && playing.is_none() { inputs.push(port(p.name, "audio", true)); }
            }
        }
        graph.nodes.push(GNode { id: name.clone(), type_name: ty.clone(), role, inputs, outputs, at: at(name) });
    }
    for l in &doc.loops {
        if let Some(play) = &l.play {
            graph.edges.push(GEdge { from: play.clone(), from_port: "inst".into(), to: l.name.clone(), to_port: "play".into(), key: None });
        }
        if let Some(input) = &l.input {
            graph.edges.push(GEdge { from: input.clone(), from_port: "notes".into(), to: l.name.clone(), to_port: "in".into(), key: None });
        }
    }
    for inst in &doc.insts {
        if let Some(shader) = doc.shader_def(&inst.name) {
            for (field, sources) in &shader.wires {
                for (i, (node, port)) in sources.graph_ports(doc).enumerate() {
                    graph.edges.push(GEdge { from: node.into(), from_port: port.into(), to: inst.name.clone(), to_port: field.clone(), key: Some(i as u64) });
                }
            }
        }
    }
    for s in &doc.streams {
        if let Some(input) = &s.input {
            graph.edges.push(GEdge { from: input.clone(), from_port: "notes".into(), to: s.name.clone(), to_port: "in".into(), key: None });
        }
    }
    for fx in &doc.fx {
        for (i, (node, port)) in fx.inputs.graph_ports(doc).enumerate() {
            graph.edges.push(GEdge { from: node.into(), from_port: port.into(), to: fx.name.clone(), to_port: "in".into(), key: Some(i as u64) });
            if port == "audio" { consumed.push(node); }
        }
        for (field, sources) in &fx.sides {
            for (i, (node, port)) in sources.graph_ports(doc).enumerate() {
                graph.edges.push(GEdge { from: node.into(), from_port: port.into(), to: fx.name.clone(), to_port: field.clone(), key: (fx.shader.is_some() || field != "key").then_some(i as u64) });
            }
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
    for name in doc.loops.iter().map(|l| &l.name).chain(doc.fx.iter().filter(|f| !doc.is_control(f)).map(|f| &f.name)) {
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

/// Connect `from` into `to`'s port, as a source change.
pub fn connect(src: &str, from: &str, to: &str, to_port: &str, control: &crate::PrepareControl) -> Result<String, String> {
    connect_port(src, from, "audio", to, to_port, control)
}

/// Connect a particular output, spelling named audio outputs as Port objects.
pub fn connect_port(src: &str, from: &str, from_port: &str, to: &str, to_port: &str, control: &crate::PrepareControl) -> Result<String, String> {
    let outline = source::outline(src);
    let node = outline.node(to).ok_or_else(|| format!("no node `{to}`"))?;
    // A loop's or a stream node's `in` takes one stream.
    let shader = matches!(node.type_name.as_str(), "Shader" | "ShaderFx" | "ShaderControl");
    let native_param = crate::model::native_param(&node.type_name, to_port);
    let one = matches!(node.type_name.as_str(), "Loop" | "Stinger") || crate::model::STREAM_TYPES.contains(&node.type_name.as_str());
    let named = from_port != "audio" && (!matches!(from_port, "inst" | "clock" | "notes")
        || outline.node(from).is_some_and(|n| matches!(n.type_name.as_str(), "Loop" | "Stinger" | "ShaderFx" | "ShaderControl")));
    let name = from_port.strip_prefix(':').unwrap_or(from_port);
    let wire = if named { format!("Port{{node: {from} name: @{name}}}") } else { from.to_owned() };
    let from = wire.as_str();
    let splice: Splice = match to_port {
        "play" if !shader => source::set_field(src, to, to_port, from)?,
        "key" if !shader => source::set_field(src, to, "key", from)?,
        "in" if one => source::set_field(src, to, "in", from)?,
        field if field == "in" || shader || native_param => {
            let value = node.field(field).map_or("[]", |f| f.value.text(src));
            let entries = source::wire_entries(value);
            if entries.iter().any(|entry| entry.text(value) == from) {
                return Err(format!("`{from}` already feeds `{to}`"));
            }
            let list = if let Some(body) = value.strip_suffix(']').filter(|_| value.starts_with('[')) {
                let space = if entries.is_empty() || body.ends_with(char::is_whitespace) { "" } else { " " };
                format!("{body}{space}{from}]")
            } else if (shader || native_param) && outline.node(value).is_none() && !value.split_once('{').is_some_and(|(name, _)| name.trim() == "Port") {
                // A new wire replaces the authored scalar or choice.
                format!("[{from}]")
            } else { format!("[{value} {from}]") };
            source::set_field(src, to, field, &list)?
        }
        other => return Err(format!("`{to}` has no input `{other}`")),
    };
    let edited = makepad_edit_values::apply_splices(src, 0, &[splice])?;
    if named || shader || native_param {
        ScoreDoc::load_controlled(&edited, control).map_err(|e| e.to_string())?;
    }
    Ok(edited)
}

/// Remove a wire: the entry `key` of an ordered input, or the one input.
pub fn disconnect(src: &str, to: &str, to_port: &str, key: Option<u64>) -> Result<String, String> {
    let outline = source::outline(src);
    let node = outline.node(to).ok_or_else(|| format!("no node `{to}`"))?;
    let shader = matches!(node.type_name.as_str(), "Shader" | "ShaderFx" | "ShaderControl");
    let one = matches!(node.type_name.as_str(), "Loop" | "Stinger") || crate::model::STREAM_TYPES.contains(&node.type_name.as_str());
    let splice = match to_port {
        "play" if !shader => source::remove_field(src, to, to_port)?,
        "key" if !shader => source::remove_field(src, to, "key")?,
        "in" if one => source::remove_field(src, to, "in")?,
        field if field == "in" || shader || crate::model::native_param(&node.type_name, field) => {
            let value = node.field(field).map_or("[]", |f| f.value.text(src));
            let entries = source::wire_entries(value);
            let index = key.map_or_else(|| entries.len().saturating_sub(1), |k| k as usize);
            let entry = entries.get(index).ok_or_else(|| format!("`{to}.{field}` has no entry {index}"))?;
            let mut list = value.to_owned();
            if value.starts_with('[') { list.replace_range(source::wire_removal(value, *entry), ""); }
            else { list = "[]".to_owned(); }
            source::set_field(src, to, field, &list)?
        }
        other => return Err(format!("`{to}` has no input `{other}`")),
    };
    makepad_edit_values::apply_splices(src, 0, &[splice])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 4 code: fn(i) { play(60) sleep(1) }}\nlet b = Loop{hits: \"x.x.\"}\nlet verb = Reverb{in: [a] mix: 0.3}\nScore{clock s a b verb}\n";

    #[test]
    fn the_graph_is_the_document_and_wiring_writes_it() {
        let doc = crate::model::ScoreDoc::test_load(SRC).unwrap();
        let g = project(&doc);
        let edges: Vec<_> = g.edges.iter().map(|e| format!("{}.{}>{}.{}", e.from, e.from_port, e.to, e.to_port)).collect();
        assert_eq!(edges, ["s.inst>a.play", "a.audio>verb.in", "b.audio>master.in", "verb.audio>master.in"]);
        let s2 = connect(SRC, "b", "verb", "in", &crate::PrepareControl::default()).unwrap();
        assert!(s2.contains("Reverb{in: [a b] mix: 0.3}"), "{s2}");
        let g2 = project(&crate::model::ScoreDoc::test_load(&s2).unwrap());
        assert!(!g2.edges.iter().any(|e| e.from == "b" && e.to == MASTER));
        let s3 = disconnect(&s2, "verb", "in", Some(0)).unwrap();
        assert!(s3.contains("Reverb{in: [b] mix: 0.3}"), "{s3}");
        let s4 = disconnect(SRC, "a", "play", None).unwrap();
        assert!(s4.contains("Loop{beats: 4") || s4.contains("Loop{ beats: 4"), "{s4}");
        assert!(compatible(&g).iter().any(|((n, p), t)| n == "s" && p == "inst" && t.contains(&("b".into(), "play".into()))));

        let src = SRC.replace("let verb", "let cv = ShaderFx{code: \"latency(7) let amount = output(1) fn effect(l,r) { amount = 0.2; 0.0 }\"}\nlet verb")
            .replace("a b verb}", "a b cv verb}");
        let wired = connect_port(&src, "cv", "amount", "verb", "mix", &crate::PrepareControl::default()).unwrap();
        let wired = connect(&wired, "b", "verb", "mix", &crate::PrepareControl::default()).unwrap();
        let doc = ScoreDoc::test_load(&wired).unwrap();
        let g = project(&doc);
        assert!(g.nodes.iter().find(|n| n.id == "verb").unwrap().inputs.contains(&port("mix", "audio", true)));
        let edges: Vec<_> = g.edges.iter().filter(|e| e.to_port == "mix").map(|e| (e.from.as_str(), e.from_port.as_str(), e.key)).collect();
        assert_eq!(edges, [("cv", "amount", Some(0)), ("b", "audio", Some(1))]);
        let player = crate::render::instantiate(&doc, 8000.0, &crate::PrepareControl::default()).unwrap();
        assert!(player.layout.plan_notes.is_empty());
        for input in ["audio", "mix"] {
            assert!(player.layout.delays.iter().any(|(key, delay)| key.consumer.ends_with("/verb") && key.port == input && delay.frames == 7),
                "shorter {input} source must align to the delayed parameter source");
        }
        makepad_audio_engine::feed::forget_instance(&player.name);
        let with_comment = wired.replace("@amount} b]", "@amount} // keep\n b]");
        let removed = disconnect(&with_comment, "verb", "mix", Some(0)).unwrap();
        assert!(removed.contains("// keep\n"), "{removed}");
        let remaining = ScoreDoc::test_load(&removed).unwrap();
        let verb = remaining.fx.iter().find(|f| f.name == "verb").unwrap();
        assert!(verb.sides[0].1.ports().eq([("b", "audio")]));
        let deleted = source::remove_node(&wired, "cv").unwrap();
        assert!(deleted.contains("mix: [b]"), "{deleted}");
        let bare = deleted.replace("mix: [b]", "mix: b");
        let deleted = source::remove_node(&bare, "b").unwrap();
        assert!(ScoreDoc::test_load(&deleted).unwrap().fx[0].sides.iter().all(|(_, sources)| sources.is_empty()));
        for separator in [", ", " "] {
            let mixed = wired.replace("@amount} b]", &format!("@amount}}{separator}{{param: @level from: 0 to: 1}}]"));
            assert!(ScoreDoc::test_load(&mixed).unwrap_err().iter().any(|d| d.message.contains("either automation or source wires")));
        }
    }
}
