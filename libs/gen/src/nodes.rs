//! The recipe checks a generation pipeline runs between its jobs, as Flow
//! nodes: `Validate` reads a generated GLB with the runtime's own skin
//! parser and refuses one that is not a skinned character (or has no
//! clips when `clips` is on); `ValidateBrief` refuses an expanded brief too
//! short to paint from or that dropped the identity it was asked to keep.
//! A refusal fails the node, so nothing downstream of a bad stage runs or
//! is published. Hosts register them with [`register`] (the Flow studio's
//! engine and the headless [`crate::host::GenHost`]).

use crate::character::{inspect_character, validate_brief, CharacterMetadata};
use makepad_flow::document::{InputDescriptor, ParamDescriptor, ParamType, Registry, TypeDescriptor};
use makepad_flow::engine::executors::{Executor, Poll};
use makepad_flow::engine::providers::ExecutorRegistry;
use makepad_flow::{Literal, Node, Port, PortType, PortValue, Value};

/// The executor owner of the recipe checks.
pub const OWNER: &str = "gen.validate";

fn input(name: &str, ty: PortType, default: Literal) -> InputDescriptor {
    InputDescriptor { name: name.into(), ty, many: None, default, flexible: false }
}

fn port(name: &str, ty: PortType) -> Port {
    Port { name: name.into(), ty, many: None }
}

/// Add the check node types and their executor to a host's registries.
pub fn register(registry: &mut Registry, executors: &mut ExecutorRegistry) -> Result<(), String> {
    let mut validate = TypeDescriptor::provider("Validate", OWNER,
        vec![input("mesh", PortType::Mesh, Literal::Null)],
        vec![port("mesh", PortType::Mesh), port("facts", PortType::Json)]);
    validate.params = vec![ParamDescriptor::authored("clips", ParamType::Literal, Literal::Bool(false),
        "Also require named animation clips (a motion stage's output).")];
    validate.doc = "Check a generated GLB is a skinned character (read with the runtime's own skin parser), and with `clips` that it carries named animation clips; the mesh passes through, `facts` says what was found (skinned, animated, embedded atlas, clips, playable).".into();
    registry.register(validate).map_err(|error| error.to_string())?;
    let mut brief = TypeDescriptor::provider("ValidateBrief", OWNER,
        vec![input("text", PortType::Text, Literal::Str(String::new())), input("anchor", PortType::Text, Literal::Str(String::new()))],
        vec![port("text", PortType::Text)]);
    brief.doc = "Check an expanded brief is at least 24 words and still names `anchor` (the identity it was asked to keep); the text passes through.".into();
    registry.register(brief).map_err(|error| error.to_string())?;
    executors.register(OWNER, |node: &Node, _: &Registry| -> Result<Box<dyn Executor>, String> {
        match node.type_name.as_str() {
            "Validate" | "ValidateBrief" => Ok(Box::new(Check::default())),
            other => Err(format!("unsupported check node `{other}`")),
        }
    })
}

#[derive(Default)]
struct Check {
    done: Option<Vec<(String, PortValue)>>,
}

fn one<'a>(inputs: &'a [(String, PortValue)], name: &str) -> Option<&'a Value> {
    inputs.iter().find(|(port, _)| port == name).and_then(|(_, value)| value.one())
}

/// The facts a character check reports, as the tools show them.
pub fn facts_json(facts: &CharacterMetadata) -> makepad_strict_json::Value {
    use makepad_strict_json::Value as Json;
    Json::Obj(vec![
        ("skinned".into(), Json::Bool(facts.skinned)),
        ("animated".into(), Json::Bool(facts.animated)),
        ("embedded_atlas".into(), Json::Bool(facts.embedded_atlas)),
        ("playable".into(), Json::Bool(facts.playable)),
        ("clips".into(), Json::Arr(facts.clips.iter().cloned().map(Json::Str).collect())),
    ])
}

impl Executor for Check {
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        if node.type_name == "ValidateBrief" {
            let text = one(inputs, "text").ok_or("ValidateBrief needs its `text`")?.as_text()?.to_string();
            let anchor = one(inputs, "anchor").map(|value| value.as_text().map(str::to_string)).transpose()?.unwrap_or_default();
            validate_brief(&anchor, &text)?;
            self.done = Some(vec![("text".into(), PortValue::One(Value::text(text)))]);
            return Ok(());
        }
        let mesh = one(inputs, "mesh").ok_or("Validate needs its `mesh`")?;
        let clips = node.params.iter().any(|(name, value)| name == "clips" && matches!(value, Literal::Bool(true)));
        let facts = inspect_character(&mesh.bytes)?;
        if !facts.skinned || clips && facts.clips.is_empty() {
            return Err(format!("`{}` did not produce a skinned character{}", node.id, if clips { " with named clips" } else { "" }));
        }
        self.done = Some(vec![
            ("mesh".into(), PortValue::One(mesh.clone())),
            ("facts".into(), PortValue::One(Value::json(facts_json(&facts).to_json()))),
        ]);
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        self.done.take().map(Poll::Done).unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {
        self.done = None;
    }
}
