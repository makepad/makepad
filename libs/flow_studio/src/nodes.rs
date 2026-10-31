//! The node types every Flow host runs: the built-ins, the `TextLines`
//! provider and the recipe checks, plus whatever components the host app
//! links (its live owners and providers). The engine knows only their
//! descriptors and owner factories; adding a component requires no engine
//! enum change.
use makepad_flow::document::types::{Carrier, Rate, SerialCodecId, TypeKey, TypeSchema};
use makepad_flow::document::{InputDescriptor, Registry, TypeDescriptor};
use makepad_flow::engine::executors::{Executor, Poll};
use makepad_flow::engine::providers::ExecutorRegistry;
use makepad_flow::{Literal, Node, Port, PortType, PortValue, Value};
use makepad_strict_json::Value as JsonValue;
use std::sync::Arc;

const LINES: &str = "flow.text.lines";
const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_LINES: usize = 65_536;

/// The registries with the host's own `components` (live owners name
/// themselves to the run scheduler there, so it knows their nodes are not
/// jobs).
pub fn registries_with(components: fn(&mut Registry, &mut ExecutorRegistry) -> Result<(), String>) -> Result<(Arc<Registry>, Arc<ExecutorRegistry>), String> {
    let mut registry = Registry::with_builtins();
    registry.register_type(TypeSchema {
        key: TypeKey::new(LINES), schema_rev: 1,
        carrier: Carrier::Artifact, rate: Rate::Job, local_only: false,
        serial: Some(SerialCodecId(Arc::from("core.value"))), audio_lowering: None,
        doc: "An ordered collection of UTF-8 text lines, encoded as a JSON string array.".into(),
    }).map_err(|error| error.to_string())?;
    let mut descriptor = TypeDescriptor::provider(
        "TextLines", "flow.text",
        vec![InputDescriptor {
            name: "text".into(), ty: PortType::Text, many: None,
            default: Literal::Str(String::new()), flexible: false,
        }],
        vec![Port { name: "lines".into(), ty: PortType::Registered(TypeKey::new(LINES)), many: None }],
    );
    descriptor.doc = "Split text into an ordered collection of lines. Empty input yields an empty collection; a trailing newline does not add a line. Limited to 8 MiB input, 65536 lines and 16 MiB encoded output.".into();
    registry.register(descriptor).map_err(|error| error.to_string())?;
    let mut executors = ExecutorRegistry::default();
    components(&mut registry, &mut executors)?;
    // The recipe checks the shared presets gate on (a character's skin and
    // clips, an expanded brief).
    makepad_gen::nodes::register(&mut registry, &mut executors)?;
    let registry = Arc::new(registry);
    executors.register("flow.text", |node: &Node, registry: &Registry| -> Result<Box<dyn Executor>, String> {
        if node.type_name != "TextLines" { return Err(format!("unsupported text node `{}`", node.type_name)); }
        Ok(Box::new(TextLines { registry: registry.clone(), ready: None }))
    })?;
    Ok((registry, Arc::new(executors)))
}

struct TextLines {
    registry: Registry,
    ready: Option<Vec<(String, PortValue)>>,
}

impl Executor for TextLines {
    fn start(&mut self, _: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        let input = inputs.iter().find(|(name, _)| name == "text")
            .and_then(|(_, value)| value.one()).ok_or("TextLines requires one text input")?;
        if input.ty != PortType::Text { return Err("TextLines requires text".into()); }
        if input.bytes.len() > MAX_INPUT_BYTES { return Err("TextLines input exceeds 8 MiB".into()); }
        let text = input.as_text()?;
        let mut upper_bytes = 2usize;
        for (index, line) in text.lines().enumerate() {
            if index >= MAX_LINES { return Err("TextLines exceeds 65536 lines".into()); }
            // An ASCII control can need six JSON bytes; every UTF-8 byte is
            // otherwise preserved except quotes/backslashes. Bound expansion
            // before creating even the first encoded line.
            upper_bytes += 3 + line.bytes().map(|byte| match byte {
                0..=31 => 6, b'"' | b'\\' => 2, _ => 1,
            }).sum::<usize>();
            if upper_bytes > MAX_OUTPUT_BYTES { return Err("TextLines encoded output exceeds 16 MiB".into()); }
        }
        let mut encoded = String::with_capacity(upper_bytes);
        encoded.push('[');
        for (index, line) in text.lines().enumerate() {
            if index != 0 { encoded.push(','); }
            encoded.push_str(&JsonValue::Str(line.to_owned()).to_json());
        }
        encoded.push(']');
        let value = Value::artifact(PortType::Registered(TypeKey::new(LINES)),
            "application/json", encoded.into_bytes(), self.registry.types())?;
        self.ready = Some(vec![("lines".into(), PortValue::One(value))]);
        Ok(())
    }
    fn poll(&mut self) -> Poll {
        self.ready.take().map(Poll::Done).unwrap_or(Poll::Pending)
    }
    fn cancel(&mut self) { self.ready = None; }
}
