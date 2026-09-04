use crate::wire::*;
use crate::Value;
use makepad_micro_serde::*;
use makepad_script::*;
use std::collections::{HashMap, HashSet};

const INSTRUCTION_LIMIT: usize = 5_000_000;
const HEAP_LIMIT: usize = 64 * 1024 * 1024;
const PRELUDE_FILE: &str = "<makepad-flow-prelude>";
const RECIPE_PRELUDE_FILE: &str = "<makepad-flow-recipe-prelude>";
const RECIPE_PRELUDE: &str = include_str!("../../recipes/prelude_recipes.splash");
const FN_INSTRUCTION_LIMIT: usize = 200_000;
pub const MAX_SOURCE_BYTES: usize = 192 * 1024;

/// A loaded flow and the splash heap that owns its run-time closures.
pub struct FlowVm {
    host: Box<ScriptVmHost<i32, ()>>,
    bx: Option<Box<ScriptVmBase>>,
    flow: ScriptObjectRef,
    graph: Graph,
}

impl FlowVm {
    pub fn load(source: &str, file_name: &str) -> Result<(Self, Graph), EvalError> {
        validate_source(source, file_name)?;
        let mut host = Box::new(ScriptVmHost::new(0i32, ()));
        let mut vm = ScriptVm {
            host: host.as_mut(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        let (result, allocation) = vm.with_heap_allocation_limit(HEAP_LIMIT, |vm| {
            vm.with_instruction_limit(INSTRUCTION_LIMIT, |vm| {
                vm.new_module(id!(flow));
                vm.eval(make_mod(PRELUDE_FILE, crate::PRELUDE));
                let prelude_errors = vm.take_errors();
                if !prelude_errors.is_empty() {
                    return Err(error_from_vm(&prelude_errors[0], PRELUDE_FILE));
                }
                vm.bx.captured_errors = Some(Vec::new());
                vm.eval(make_mod(RECIPE_PRELUDE_FILE, RECIPE_PRELUDE));
                let recipe_errors = vm.take_errors();
                if !recipe_errors.is_empty() {
                    return Err(error_from_vm(&recipe_errors[0], RECIPE_PRELUDE_FILE));
                }
                vm.bx.captured_errors = Some(Vec::new());
                let eval_source = source_for_eval(source);
                let value = vm.eval(make_mod(file_name, &eval_source));
                let errors = vm.take_errors();
                if !errors.is_empty() {
                    return Err(error_from_vm(&errors[0], file_name));
                }
                let graph = extract(vm, value, source, file_name)?;
                let flow = value
                    .as_object()
                    .ok_or_else(|| at(file_name, 1, 1, "last expression is not a Flow{}"))?;
                Ok((graph, vm.bx.heap.new_object_ref(flow)))
            })
        });
        if allocation.exceeded {
            return Err(at(
                file_name,
                1,
                1,
                "flow exceeded the 64 MiB heap allocation limit",
            ));
        }
        let (graph, flow) = result?;
        let bx = vm.bx;
        let loaded = Self {
            host,
            bx: Some(bx),
            flow,
            graph: graph.clone(),
        };
        Ok((loaded, graph))
    }

    pub fn call_fn(
        &mut self,
        node_id: &str,
        inputs: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, String> {
        let node = self
            .graph
            .nodes
            .iter()
            .find(|node| node.id == node_id && node.kind == "fn")
            .cloned()
            .ok_or_else(|| format!("Fn node `{node_id}` is not declared"))?;
        let media: HashMap<_, _> = inputs
            .iter()
            .filter(|(_, value)| value.ty.is_media())
            .map(|(_, value)| (value.digest_hex(), value.clone()))
            .collect();
        for (port, value) in inputs {
            let declared = node
                .inputs
                .iter()
                .find(|input| input.port == *port)
                .ok_or_else(|| format!("Fn node `{node_id}` has no input port `{port}`"))?;
            if declared.ty != value.ty {
                return Err(format!(
                    "Fn node `{node_id}` input `{port}` expected {}, got {}",
                    declared.ty.as_str(),
                    value.ty.as_str()
                ));
            }
        }
        let flow = self.flow.as_object();
        self.with_vm(|vm| {
            let node_value = own_value(vm, flow, node_id)
                .ok_or_else(|| format!("Fn node `{node_id}` is missing from run VM"))?;
            let node_obj = node_value
                .as_object()
                .ok_or_else(|| format!("Fn node `{node_id}` is not an object"))?;
            let run = deep_value(vm, node_obj, "run")
                .ok_or_else(|| format!("Fn node `{node_id}` has no run closure"))?;
            let args = vm.bx.heap.new_object();
            for (port, value) in inputs {
                let script_value = value_to_script(vm, value)?;
                vm.bx.heap.set_value_def(
                    args,
                    LiveId::from_str(port).into(),
                    script_value,
                );
            }
            vm.bx.captured_errors = Some(Vec::new());
            let returned = vm.with_instruction_limit(FN_INSTRUCTION_LIMIT, |vm| {
                vm.call(run, &[args.into()])
            });
            let errors = vm.take_errors();
            if let Some(error) = errors.first() {
                return Err(trim_origin(error).to_string());
            }
            if returned.is_err() {
                return Err("Fn closure failed".to_string());
            }
            let object = returned
                .as_object()
                .ok_or_else(|| format!("Fn node `{node_id}` must return an object"))?;
            let mut outputs = Vec::with_capacity(node.outputs.len());
            for output in &node.outputs {
                let value = own_value(vm, object, &output.name).ok_or_else(|| {
                    format!(
                        "Fn node `{node_id}` returned no declared output key `{}`",
                        output.name
                    )
                })?;
                outputs.push((
                    output.name.clone(),
                    script_to_value(vm, value, output.ty, &media).map_err(|error| {
                        format!("Fn node `{node_id}` output `{}`: {error}", output.name)
                    })?,
                ));
            }
            Ok(outputs)
        })
    }

    fn with_vm<R>(&mut self, f: impl FnOnce(&mut ScriptVm<'_>) -> R) -> R {
        let bx = self.bx.take().expect("FlowVm re-entered");
        let mut vm = ScriptVm {
            host: self.host.as_mut(),
            bx,
        };
        let result = f(&mut vm);
        self.bx = Some(vm.bx);
        result
    }
}

#[derive(Clone)]
struct TypeSpec {
    type_name: &'static str,
    kind: &'static str,
    params: &'static [ParamSpec],
    inputs: &'static [InputSpec],
}

#[derive(Clone, Copy)]
struct ParamSpec {
    name: &'static str,
    expected: ParamType,
    default: DefaultValue,
}

#[derive(Clone, Copy)]
struct InputSpec {
    name: &'static str,
    ty: PortType,
    default: DefaultValue,
    flexible: bool,
}

#[derive(Clone, Copy)]
enum ParamType {
    String,
    Literal,
    Number,
    NonNegativeInteger,
    PortTypeNoBytes,
    PortTypeWithBytes,
    HttpOut,
    HttpMethod,
    LiteralArray,
}

#[derive(Clone, Copy)]
enum DefaultValue {
    Null,
    Num(f64),
    Str(&'static str),
    Id(&'static str),
    Arr,
    Obj,
}

const INPUT_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("type", ParamType::PortTypeNoBytes, DefaultValue::Id("text")),
    ParamSpec::new("value", ParamType::Literal, DefaultValue::Str("")),
];
const OUTPUT_PARAMS: &[ParamSpec] = &[ParamSpec::new(
    "type",
    ParamType::PortTypeWithBytes,
    DefaultValue::Id("text"),
)];
const PUBLISH_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("title", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("namespace", ParamType::String, DefaultValue::Str("flows")),
    ParamSpec::new("tags", ParamType::LiteralArray, DefaultValue::Arr),
    ParamSpec::new("description", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("alias", ParamType::String, DefaultValue::Str("")),
];
const LLM_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("system", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("model", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("temperature", ParamType::Number, DefaultValue::Num(0.7)),
    ParamSpec::new(
        "max_tokens",
        ParamType::NonNegativeInteger,
        DefaultValue::Num(0.0),
    ),
];
const FN_PARAMS: &[ParamSpec] = &[ParamSpec::new(
    "out",
    ParamType::LiteralArray,
    DefaultValue::Arr,
)];
const HTTP_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("method", ParamType::HttpMethod, DefaultValue::Id("get")),
    ParamSpec::new("content_type", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new(
        "out",
        ParamType::HttpOut,
        DefaultValue::Id("text"),
    ),
    ParamSpec::new("accept", ParamType::LiteralArray, DefaultValue::Arr),
];
const ASK_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("question", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("type", ParamType::PortTypeNoBytes, DefaultValue::Id("text")),
    ParamSpec::new("options", ParamType::LiteralArray, DefaultValue::Arr),
    ParamSpec::new("default", ParamType::Literal, DefaultValue::Str("")),
    ParamSpec::new(
        "timeout",
        ParamType::NonNegativeInteger,
        DefaultValue::Num(0.0),
    ),
];
const IMAGE_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("width", ParamType::Number, DefaultValue::Num(1024.0)),
    ParamSpec::new("height", ParamType::Number, DefaultValue::Num(1024.0)),
    ParamSpec::new("steps", ParamType::Number, DefaultValue::Num(8.0)),
    ParamSpec::new("seed", ParamType::Number, DefaultValue::Num(0.0)),
    ParamSpec::new("negative", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("model", ParamType::String, DefaultValue::Str("")),
];
const UPSCALE_PARAMS: &[ParamSpec] = &[ParamSpec::new(
    "factor",
    ParamType::Number,
    DefaultValue::Num(2.0),
)];

const LLM_INPUTS: &[InputSpec] = &[InputSpec::new(
    "prompt",
    PortType::Text,
    DefaultValue::Str(""),
)];
const OUTPUT_INPUTS: &[InputSpec] = &[InputSpec::new(
    "value",
    PortType::Text,
    DefaultValue::Null,
)];
const PUBLISH_INPUTS: &[InputSpec] = &[InputSpec::flexible(
    "value",
    PortType::Image,
    DefaultValue::Null,
)];
const HTTP_INPUTS: &[InputSpec] = &[
    InputSpec::new("url", PortType::Text, DefaultValue::Str("")),
    InputSpec::new("headers", PortType::Json, DefaultValue::Obj),
    InputSpec::flexible("body", PortType::Json, DefaultValue::Null),
];
const IMAGE_INPUTS: &[InputSpec] = &[
    InputSpec::new("prompt", PortType::Text, DefaultValue::Str("")),
    InputSpec::new("image", PortType::Image, DefaultValue::Null),
];
const UPSCALE_INPUTS: &[InputSpec] = &[InputSpec::new(
    "image",
    PortType::Image,
    DefaultValue::Null,
)];

impl ParamSpec {
    const fn new(name: &'static str, expected: ParamType, default: DefaultValue) -> Self {
        Self {
            name,
            expected,
            default,
        }
    }
}

impl InputSpec {
    const fn new(name: &'static str, ty: PortType, default: DefaultValue) -> Self {
        Self {
            name,
            ty,
            default,
            flexible: false,
        }
    }

    const fn flexible(name: &'static str, ty: PortType, default: DefaultValue) -> Self {
        Self {
            name,
            ty,
            default,
            flexible: true,
        }
    }
}

fn type_spec(type_name: &str) -> Option<TypeSpec> {
    Some(match type_name {
        "Input" | "Text" => TypeSpec {
            type_name: if type_name == "Text" { "Text" } else { "Input" },
            kind: "input",
            params: INPUT_PARAMS,
            inputs: &[],
        },
        "Output" => TypeSpec {
            type_name: "Output",
            kind: "output",
            params: OUTPUT_PARAMS,
            inputs: OUTPUT_INPUTS,
        },
        "Publish" => TypeSpec {
            type_name: "Publish",
            kind: "publish",
            params: PUBLISH_PARAMS,
            inputs: PUBLISH_INPUTS,
        },
        "Llm" => TypeSpec {
            type_name: "Llm",
            kind: "chat",
            params: LLM_PARAMS,
            inputs: LLM_INPUTS,
        },
        "Fn" => TypeSpec {
            type_name: "Fn",
            kind: "fn",
            params: FN_PARAMS,
            inputs: &[],
        },
        "Http" => TypeSpec {
            type_name: "Http",
            kind: "http",
            params: HTTP_PARAMS,
            inputs: HTTP_INPUTS,
        },
        "Ask" => TypeSpec {
            type_name: "Ask",
            kind: "ask",
            params: ASK_PARAMS,
            inputs: &[],
        },
        "Image" => TypeSpec {
            type_name: "Image",
            kind: "gen",
            params: IMAGE_PARAMS,
            inputs: IMAGE_INPUTS,
        },
        "Upscale" => TypeSpec {
            type_name: "Upscale",
            kind: "gen",
            params: UPSCALE_PARAMS,
            inputs: UPSCALE_INPUTS,
        },
        "Gen" => TypeSpec {
            type_name: "Gen",
            kind: "gen",
            params: &[],
            inputs: &[],
        },
        _ => return None,
    })
}

fn make_mod(file: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: file.to_string(),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

/// Evaluate one flow source in an isolated, budgeted splash VM.
pub fn evaluate(source: &str, file_name: &str) -> Result<Graph, EvalError> {
    FlowVm::load(source, file_name).map(|(_, graph)| graph)
}

fn value_to_script(vm: &mut ScriptVm<'_>, value: &Value) -> Result<ScriptValue, String> {
    if value.ty == PortType::Text {
        return Ok(vm.bx.heap.new_string_from_str(value.as_text()?).into());
    }
    if matches!(value.ty, PortType::Json | PortType::List) {
        let text = value.as_text()?;
        let parsed = makepad_strict_json::parse(text.as_bytes())
            .map_err(|error| format!("invalid JSON value: {error}"))?;
        if value.ty == PortType::List
            && !matches!(parsed, makepad_strict_json::Value::Arr(_))
        {
            return Err("list value must be a JSON array".to_string());
        }
        return Ok(json_to_script(vm, &parsed));
    }
    let object = vm.bx.heap.new_object();
    let digest = vm.bx.heap.new_string_from_str(&value.digest_hex());
    let content_type = vm.bx.heap.new_string_from_str(&value.content_type);
    vm.bx
        .heap
        .set_value_def(object, id!(digest).into(), digest);
    vm.bx
        .heap
        .set_value_def(object, id!(content_type).into(), content_type);
    vm.bx.heap.set_value_def(
        object,
        id!(bytes).into(),
        ScriptValue::from_f64(value.bytes.len() as f64),
    );
    Ok(object.into())
}

fn json_to_script(vm: &mut ScriptVm<'_>, value: &makepad_strict_json::Value) -> ScriptValue {
    use makepad_strict_json::Value as Json;
    match value {
        Json::Null => NIL,
        Json::Bool(value) => ScriptValue::from_bool(*value),
        Json::Int(value) => ScriptValue::from_f64(*value as f64),
        Json::F64(value) => ScriptValue::from_f64(*value),
        Json::Str(value) => vm.bx.heap.new_string_from_str(value),
        Json::Arr(values) => {
            let array = vm.bx.heap.new_array();
            for value in values {
                let value = json_to_script(vm, value);
                vm.bx.heap.array_push_unchecked(array, value);
            }
            array.into()
        }
        Json::Obj(values) => {
            let object = vm.bx.heap.new_object();
            vm.bx.heap.set_string_keys(object);
            for (name, value) in values {
                let key = vm.bx.heap.new_string_from_str(name);
                let value = json_to_script(vm, value);
                vm.bx.heap.set_value_def(object, key, value);
            }
            object.into()
        }
    }
}

fn script_to_value(
    vm: &mut ScriptVm<'_>,
    value: ScriptValue,
    ty: PortType,
    media: &HashMap<String, Value>,
) -> Result<Value, String> {
    match ty {
        PortType::Text => value_string(vm, value)
            .map(Value::text)
            .ok_or_else(|| "expected a string".to_string()),
        PortType::Json | PortType::List => {
            if ty == PortType::List && value.as_array().is_none() {
                return Err("expected an array".to_string());
            }
            let json = vm.bx.heap.to_json(value);
            let text = value_string(vm, json)
                .ok_or_else(|| "value could not be serialized as JSON".to_string())?;
            makepad_strict_json::parse(text.as_bytes())
                .map_err(|error| format!("invalid JSON output: {error}"))?;
            Ok(if ty == PortType::List {
                Value::list(text)
            } else {
                Value::json(text)
            })
        }
        _ => {
            let object = value
                .as_object()
                .ok_or_else(|| "expected an opaque media handle".to_string())?;
            let digest = deep_value(vm, object, "digest")
                .and_then(|value| value_string(vm, value))
                .ok_or_else(|| "media handle has no digest".to_string())?;
            let existing = media
                .get(&digest)
                .ok_or_else(|| "media handle is not one of this call's inputs".to_string())?;
            if existing.ty != ty {
                return Err(format!(
                    "media handle has type {}, expected {}",
                    existing.ty.as_str(),
                    ty.as_str()
                ));
            }
            Ok(existing.clone())
        }
    }
}

/// Evaluate the shipped prelude and expose its palette metadata. Field and
/// object descriptions come through splash's `construction_chain` docs API,
/// so the HTTP catalog cannot drift from the `/** */` text authors edit.
pub fn prelude_catalog() -> Result<Vec<NodeTypeCatalog>, EvalError> {
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm {
        host: &mut host,
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    vm.new_module(id!(flow));
    vm.eval(make_mod(PRELUDE_FILE, crate::PRELUDE));
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(error_from_vm(&errors[0], PRELUDE_FILE));
    }
    vm.bx.captured_errors = Some(Vec::new());
    vm.eval(make_mod(RECIPE_PRELUDE_FILE, RECIPE_PRELUDE));
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(error_from_vm(&errors[0], RECIPE_PRELUDE_FILE));
    }
    let module = own_value(&vm, vm.bx.heap.modules, "flow")
        .and_then(|value| value.as_object())
        .ok_or_else(|| at(PRELUDE_FILE, 1, 1, "prelude module is missing"))?;
    let ui_module = own_value(&vm, module, "ui").and_then(|value| value.as_object());

    // Walk every key `mod.flow` itself sets rather than a hard-coded name
    // list, so every recipe type built purely in splash (Mesh, Video,
    // Music, Inpaint, ...) reaches the catalog alongside the language's
    // built-in kinds.
    let mut raw = Vec::new();
    vm.bx.heap.object_data(module).map_iter_ordered(|key, value| {
        if let Some(name) = key_name(&vm, key) {
            raw.push((name, value));
        }
    });
    for entry in &vm.bx.heap.object_data(module).vec {
        if let Some(name) = key_name(&vm, entry.key) {
            raw.push((name, entry.value));
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (_key, value) in raw {
        let Some(obj) = value.as_object() else { continue };
        let Some(type_name) = own_value(&vm, obj, "type_name").and_then(|v| value_string(&vm, v))
        else {
            continue;
        };
        let kind = deep_value(&vm, obj, "kind").and_then(value_id).unwrap_or_default();
        if !matches!(
            kind.as_str(),
            "input" | "output" | "publish" | "chat" | "fn" | "http" | "ask" | "gen"
        ) {
            continue;
        }
        if !seen.insert(type_name.clone()) {
            continue;
        }
        let domain = deep_value(&vm, obj, "domain")
            .and_then(|v| value_string(&vm, v))
            .filter(|value| !value.is_empty());

        let chain = vm.construction_chain(obj.into());
        let doc = chain.iter().find_map(|level| level.doc.clone()).unwrap_or_default();
        let field_doc = |name: &str| {
            let wanted = LiveId::from_str(name);
            chain
                .iter()
                .find_map(|level| {
                    level
                        .field_docs
                        .iter()
                        .find_map(|(id, text)| (*id == wanted).then(|| text.clone()))
                })
                .unwrap_or_default()
        };

        let spec = type_spec(&type_name);
        let mut discard = Vec::new();
        let loc = Loc { line: 1, col: 1 };
        let (ports_in, ports_out) = if kind == "gen" {
            let ports_in = ports_side_typed(
                &vm,
                obj,
                "in",
                "",
                None,
                RECIPE_PRELUDE_FILE,
                &loc,
                &type_name,
                &mut discard,
            )?
            .into_iter()
            .map(|(name, ty)| Port { name, ty })
            .collect();
            let ports_out = ports_side_typed(
                &vm,
                obj,
                "out",
                "",
                None,
                RECIPE_PRELUDE_FILE,
                &loc,
                &type_name,
                &mut discard,
            )?
            .into_iter()
            .map(|(name, ty)| Port { name, ty })
            .collect();
            (ports_in, ports_out)
        } else if let Some(spec) = &spec {
            let ports_in = spec
                .inputs
                .iter()
                .map(|port| Port {
                    name: port.name.to_string(),
                    ty: port.ty,
                })
                .collect();
            (ports_in, catalog_outputs(&type_name))
        } else {
            (Vec::new(), Vec::new())
        };
        let params = if let Some(spec) = &spec {
            spec
                .params
                .iter()
                .map(|param| {
                    let doc = field_doc(param.name);
                    let hint = parse_doc_hint(&doc);
                    let range = match (hint.min, hint.max) {
                        (Some(min), Some(max)) => Some(ParamRange {
                            min,
                            max,
                            step: hint.step,
                        }),
                        _ => None,
                    };
                    NodeParamCatalog {
                        name: param.name.to_string(),
                        default: default_json(param.default),
                        doc,
                        range,
                    }
                })
                .collect()
        } else {
            let input_names: HashSet<String> =
                ports_in.iter().map(|port| port.name.clone()).collect();
            let generic_params =
                generic_gen_params(&vm, obj, "", None, RECIPE_PRELUDE_FILE, &loc, &input_names)?;
            generic_params
                .into_iter()
                .map(|(name, literal)| {
                    let doc = field_doc(&name);
                    let hint = parse_doc_hint(&doc);
                    let range = match (hint.min, hint.max) {
                        (Some(min), Some(max)) => Some(ParamRange {
                            min,
                            max,
                            step: hint.step,
                        }),
                        _ => None,
                    };
                    NodeParamCatalog {
                        name,
                        default: literal_to_json_value(&literal),
                        doc,
                        range,
                    }
                })
                .collect()
        };

        let face = ui_module
            .and_then(|ui_module| face_name(&vm, ui_module, obj))
            .unwrap_or_default();

        out.push(NodeTypeCatalog {
            type_name,
            kind,
            domain,
            models: Vec::new(),
            ports: NodePortsCatalog { _in: ports_in, out: ports_out },
            params,
            face,
            doc,
        });
    }
    out.sort_by(|a, b| a.type_name.cmp(&b.type_name));
    Ok(out)
}

/// Resolve a node's `ui:` value back to its name in `mod.flow.ui` (for
/// example `"GenFace"`), for the catalog's descriptive `face` field.
fn face_name(vm: &ScriptVm<'_>, ui_module: ScriptObject, obj: ScriptObject) -> Option<String> {
    let ui_obj = deep_value(vm, obj, "ui")?.as_object()?;
    let mut found = None;
    vm.bx.heap.object_data(ui_module).map_iter_ordered(|key, value| {
        if found.is_some() {
            return;
        }
        if value.as_object() == Some(ui_obj) {
            found = key_name(vm, key);
        }
    });
    found
}

fn literal_to_json_value(value: &Literal) -> JsonValue {
    match value {
        Literal::Null => JsonValue::Null,
        Literal::Bool(value) => JsonValue::Bool(*value),
        Literal::Num(value) => JsonValue::F64(*value),
        Literal::Str(value) | Literal::Id(value) => JsonValue::String(value.clone()),
        Literal::Arr(values) => JsonValue::Array(values.iter().map(literal_to_json_value).collect()),
        Literal::Obj(values) => JsonValue::Object(
            values
                .iter()
                .map(|(name, value)| (name.clone(), literal_to_json_value(value)))
                .collect(),
        ),
    }
}

fn catalog_outputs(type_name: &str) -> Vec<Port> {
    match type_name {
        "Input" | "Text" | "Ask" | "Llm" => vec![Port {
            name: "text".to_string(),
            ty: PortType::Text,
        }],
        "Http" => vec![
            Port { name: "value".to_string(), ty: PortType::Text },
            Port { name: "meta".to_string(), ty: PortType::Json },
        ],
        "Publish" => vec![Port {
            name: "asset".to_string(),
            ty: PortType::Json,
        }],
        "Image" | "Upscale" => vec![Port {
            name: "image".to_string(),
            ty: PortType::Image,
        }],
        _ => Vec::new(),
    }
}

fn default_json(value: DefaultValue) -> JsonValue {
    match value {
        DefaultValue::Null => JsonValue::Null,
        DefaultValue::Num(value) => JsonValue::F64(value),
        DefaultValue::Str(value) | DefaultValue::Id(value) => JsonValue::String(value.to_string()),
        DefaultValue::Arr => JsonValue::Array(Vec::new()),
        DefaultValue::Obj => JsonValue::Object(HashMap::new()),
    }
}

fn error_from_vm(text: &str, fallback_file: &str) -> EvalError {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix(fallback_file).and_then(|s| s.strip_prefix(':')) {
        let mut parts = rest.splitn(3, ':');
        if let (Some(line), Some(col), Some(message)) = (parts.next(), parts.next(), parts.next()) {
            if let (Ok(line), Ok(col)) = (line.parse(), col.parse()) {
                return EvalError {
                    file: fallback_file.to_string(),
                    line,
                    col,
                    message: trim_origin(message),
                };
            }
        }
    }
    let mut parts = text.splitn(4, ':');
    if let (Some(file), Some(line), Some(col), Some(message)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    {
        if let (Ok(line), Ok(col)) = (line.parse(), col.parse()) {
            return EvalError {
                file: file.to_string(),
                line,
                col,
                message: trim_origin(message),
            };
        }
    }
    EvalError {
        file: fallback_file.to_string(),
        line: 1,
        col: 1,
        message: text.to_string(),
    }
}

fn trim_origin(message: &str) -> String {
    let message = message.trim();
    if let Some(pos) = message.rfind(" (") {
        if message.ends_with(')') {
            return message[..pos].trim().to_string();
        }
    }
    message.to_string()
}

fn extract(
    vm: &ScriptVm<'_>,
    flow_value: ScriptValue,
    source: &str,
    file_name: &str,
) -> Result<Graph, EvalError> {
    let flow_obj = flow_value
        .as_object()
        .ok_or_else(|| at(file_name, 1, 1, "last expression is not a Flow{}"))?;
    let prototypes = PreludePrototypes::new(vm)?;
    if prototypes.type_of(vm, flow_obj).as_deref() != Some("Flow") {
        let loc = object_loc(vm, flow_obj, file_name);
        return Err(at(
            file_name,
            loc.line,
            loc.col,
            "last expression is not a Flow{}",
        ));
    }
    let flow_span = object_span(vm, flow_obj, source);
    let flow_loc = object_loc(vm, flow_obj, file_name);
    let label = expect_string_field(vm, flow_obj, "label", source, flow_span, file_name)?;
    let brief = expect_string_field(vm, flow_obj, "brief", source, flow_span, file_name)?;
    let trigger = expect_id_field(vm, flow_obj, "trigger", source, flow_span, file_name)?;
    if trigger != "manual" && trigger != "input" {
        return Err(field_error(
            source,
            flow_span,
            "trigger",
            file_name,
            "trigger must be @manual or @input",
            &flow_loc,
        ));
    }
    let concurrency_value = deep_value(vm, flow_obj, "concurrency").unwrap_or(NIL);
    let concurrency_num = concurrency_value.as_number().ok_or_else(|| {
        field_error(
            source,
            flow_span,
            "concurrency",
            file_name,
            "concurrency must be a positive integer",
            &flow_loc,
        )
    })?;
    if concurrency_num < 1.0 || concurrency_num.fract() != 0.0 {
        return Err(field_error(
            source,
            flow_span,
            "concurrency",
            file_name,
            "concurrency must be a positive integer",
            &flow_loc,
        ));
    }
    let autostart = deep_value(vm, flow_obj, "autostart")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| {
            field_error(
                source,
                flow_span,
                "autostart",
                file_name,
                "autostart must be a bool",
                &flow_loc,
            )
        })?;

    let mut raw_nodes = Vec::new();
    vm.bx.heap.object_data(flow_obj).map_iter_ordered(|key, value| {
        if let Some(name) = key_name(vm, key) {
            if !matches!(
                name.as_str(),
                "kind"
                    | "type_name"
                    | "label"
                    | "brief"
                    | "trigger"
                    | "concurrency"
                    | "autostart"
                    | "tools"
                    | "ui"
            ) {
                raw_nodes.push((name, value));
            }
        }
    });
    for entry in &vm.bx.heap.object_data(flow_obj).vec {
        if let Some(name) = key_name(vm, entry.key) {
            raw_nodes.push((name, entry.value));
        }
    }
    if raw_nodes.is_empty() {
        // Empty flows are valid, but this also makes the intended distinction explicit.
    }
    let mut object_ids = HashMap::new();
    for (id, value) in &raw_nodes {
        let obj = value.as_object().ok_or_else(|| {
            at(
                file_name,
                flow_loc.line,
                flow_loc.col,
                format!("flow field `{id}` is not a node"),
            )
        })?;
        if object_ids.insert(obj, id.clone()).is_some() {
            let loc = object_loc(vm, obj, file_name);
            return Err(at(
                file_name,
                loc.line,
                loc.col,
                "the same node is listed more than once in Flow{}",
            ));
        }
    }

    let mut nodes = Vec::with_capacity(raw_nodes.len());
    let mut warnings = Vec::new();
    for (id, value) in &raw_nodes {
        let obj = value.as_object().unwrap();
        let type_name = prototypes.type_of(vm, obj).ok_or_else(|| {
            let loc = object_loc(vm, obj, file_name);
            at(
                file_name,
                loc.line,
                loc.col,
                format!("`{id}` is not a node type from mod.flow"),
            )
        })?;
        if type_name == "Flow" {
            let loc = object_loc(vm, obj, file_name);
            return Err(at(
                file_name,
                loc.line,
                loc.col,
                format!("flow field `{id}` is not a node"),
            ));
        }
        nodes.push(extract_node(
            vm,
            source,
            file_name,
            id,
            obj,
            &type_name,
            &object_ids,
            &mut warnings,
        )?);
    }

    resolve_flexible_input_types(&mut nodes);
    let mut edges = Vec::new();
    for node in &nodes {
        for input in &node.inputs {
            if let NodeInputValue::Edge(edge) = &input.value {
                edges.push(Edge {
                    from_node: edge.from_node.clone(),
                    from_port: edge.from_port.clone(),
                    to_node: node.id.clone(),
                    to_port: input.port.clone(),
                });
            }
        }
    }
    validate_acyclic(&nodes, &edges, file_name)?;
    validate_edges(&nodes, &edges, file_name)?;
    let tools = extract_tools(vm, flow_obj, &nodes, &edges, source, flow_span, file_name)?;
    let flow_ui_src = flow_span
        .and_then(|span| field_sources(source, span).get("ui").copied())
        .map(|span| source[span.0..span.1].trim().to_string())
        .filter(|text| text != "nil");
    Ok(Graph {
        revision: 0,
        label,
        brief,
        trigger,
        concurrency: concurrency_num as u64,
        autostart,
        nodes,
        edges,
        tools,
        flow_ui_src,
        warnings,
    })
}

struct PreludePrototypes {
    entries: Vec<(&'static str, ScriptObject)>,
}

impl PreludePrototypes {
    fn new(vm: &ScriptVm<'_>) -> Result<Self, EvalError> {
        let module = own_value(vm, vm.bx.heap.modules, "flow")
            .and_then(|value| value.as_object())
            .ok_or_else(|| at(PRELUDE_FILE, 1, 1, "prelude module is missing"))?;
        let mut entries = Vec::new();
        for name in [
            "Text", "Image", "Upscale", "Input", "Output", "Publish", "Llm", "Fn", "Http",
            "Ask", "Gen", "Flow",
        ] {
            let value = own_value(vm, module, name).ok_or_else(|| {
                at(
                    PRELUDE_FILE,
                    1,
                    1,
                    format!("prelude did not register `{name}`"),
                )
            })?;
            let obj = value.as_object().ok_or_else(|| {
                at(
                    PRELUDE_FILE,
                    1,
                    1,
                    format!("prelude `{name}` is not an object"),
                )
            })?;
            let static_name = match name {
                "Text" => "Text",
                "Image" => "Image",
                "Upscale" => "Upscale",
                "Input" => "Input",
                "Output" => "Output",
                "Publish" => "Publish",
                "Llm" => "Llm",
                "Fn" => "Fn",
                "Http" => "Http",
                "Ask" => "Ask",
                "Gen" => "Gen",
                "Flow" => "Flow",
                _ => unreachable!(),
            };
            entries.push((static_name, obj));
        }
        Ok(Self { entries })
    }

    fn type_of(&self, vm: &ScriptVm<'_>, object: ScriptObject) -> Option<String> {
        let mut cur = Some(object);
        let mut depth = 0;
        while let Some(obj) = cur {
            for (name, prototype) in &self.entries {
                if obj == *prototype {
                    return Some((*name).to_string());
                }
            }
            cur = vm.bx.heap.proto(obj).as_object();
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        None
    }
}

fn extract_node(
    vm: &ScriptVm<'_>,
    source: &str,
    file_name: &str,
    id: &str,
    obj: ScriptObject,
    type_name: &str,
    object_ids: &HashMap<ScriptObject, String>,
    warnings: &mut Vec<String>,
) -> Result<Node, EvalError> {
    let spec = type_spec(type_name).unwrap();
    let loc = object_loc(vm, obj, file_name);
    let span = object_span(vm, obj, source);
    let fields = span.map(|span| field_sources(source, span)).unwrap_or_default();
    let context = format!("node `{id}`");
    let mut params = Vec::new();
    for param in spec.params {
        let mut field_source = fields
            .get(param.name)
            .copied()
            .or_else(|| source_field_in_chain(vm, obj, source, file_name, param.name));
        let value = if spec.kind == "input" && param.name == "value" && field_source.is_none() {
            let legacy_source = fields
                .get("default")
                .copied()
                .or_else(|| source_field_in_chain(vm, obj, source, file_name, "default"));
            if legacy_source.is_some() {
                field_source = legacy_source;
                deep_value(vm, obj, "default").unwrap_or(NIL)
            } else {
                deep_value(vm, obj, param.name).unwrap_or(NIL)
            }
        } else {
            deep_value(vm, obj, param.name).unwrap_or(NIL)
        };
        let mut literal = literal_from_value(vm, value)
            .map_err(|message| source_range_error(source, field_source, file_name, message, &loc))?;
        validate_param(param, &literal)
            .map_err(|message| source_range_error(source, field_source, file_name, message, &loc))?;
        snap_documented_param(vm, obj, &context, param.name, &mut literal, warnings);
        params.push((param.name.to_string(), literal));
    }
    let gen_inputs = if spec.kind == "gen" {
        Some(ports_side_typed(
            vm, obj, "in", source, span, file_name, &loc, &context, warnings,
        )?)
    } else {
        None
    };
    if type_name == "Gen" {
        let input_names: HashSet<String> = gen_inputs
            .as_ref()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        params = generic_gen_params(vm, obj, source, span, file_name, &loc, &input_names)?;
        for (name, literal) in &mut params {
            snap_documented_param(vm, obj, &context, name, literal, warnings);
        }
    }

    let mut inputs = Vec::new();
    if type_name == "Fn" {
        let in_value = deep_value(vm, obj, "in").unwrap_or(NIL);
        let in_obj = in_value.as_object().ok_or_else(|| {
            field_error(
                source,
                span,
                "in",
                file_name,
                "Fn.in must be an object",
                &loc,
            )
        })?;
        let mut pairs = Vec::new();
        vm.bx.heap.object_data(in_obj).map_iter_ordered(|key, value| {
            if let Some(name) = key_name(vm, key) {
                pairs.push((name, value));
            }
        });
        for (name, value) in pairs {
            let inferred = if looks_like_port_ref(vm, value) {
                port_type(&name).unwrap_or(PortType::Json)
            } else {
                infer_literal_type(vm, value).unwrap_or(PortType::Json)
            };
            inputs.push(extract_input(
                vm,
                source,
                file_name,
                span,
                &loc,
                id,
                &name,
                inferred,
                true,
                value,
                object_ids,
            )?);
        }
        let declared: HashSet<&str> = inputs.iter().map(|input| input.port.as_str()).collect();
        for name in fields.keys() {
            if matches!(name.as_str(), "in" | "out" | "run") || declared.contains(name.as_str()) {
                continue;
            }
            if own_value(vm, obj, name).is_some_and(|value| looks_like_port_ref(vm, value)) {
                return Err(field_error(
                    source,
                    span,
                    name,
                    file_name,
                    format!(
                        "Fn field `{name}` receives an edge but is undeclared; declare it in `Fn.in`"
                    ),
                    &loc,
                ));
            }
        }
    } else if spec.kind == "gen" {
        for (name, ty) in gen_inputs.unwrap() {
            let default = spec
                .inputs
                .iter()
                .find_map(|input| (input.name == name).then_some(input.default))
                .unwrap_or(DefaultValue::Null);
            let value = input_value(vm, obj, &name, default);
            inputs.push(extract_input(
                vm,
                source,
                file_name,
                span,
                &loc,
                id,
                &name,
                ty,
                false,
                value,
                object_ids,
            )?);
        }
    } else {
        for input in spec.inputs {
            let mut declared_ty = input.ty;
            if type_name == "Output" {
                declared_ty = param_port_type(&params, "type").unwrap();
            }
            let value = input_value(vm, obj, input.name, input.default);
            inputs.push(extract_input(
                vm,
                source,
                file_name,
                span,
                &loc,
                id,
                input.name,
                declared_ty,
                input.flexible,
                value,
                object_ids,
            )?);
        }
    }

    let outputs = outputs_for(
        vm, obj, type_name, &params, source, span, file_name, &loc, &context, warnings,
    )?;
    let at_source = fields.get("at").copied().or_else(|| {
        source_field_in_chain(vm, obj, source, file_name, "at")
    });
    let at_value = at_source.and_then(|range| parse_vec2(source[range.0..range.1].trim()));
    if at_source.is_some() && at_value.is_none() {
        return Err(field_error(
            source,
            span,
            "at",
            file_name,
            "at must be vec2(number, number)",
            &loc,
        ));
    }
    let size_source = fields.get("size").copied().or_else(|| {
        source_field_in_chain(vm, obj, source, file_name, "size")
    });
    let size_value = size_source.and_then(|range| {
        let value = source[range.0..range.1].trim();
        (value != "nil").then(|| parse_vec2(value)).flatten()
    });
    if size_source.is_some()
        && size_value.is_none()
        && size_source.is_some_and(|range| source[range.0..range.1].trim() != "nil")
    {
        return Err(field_error(
            source,
            span,
            "size",
            file_name,
            "size must be nil or vec2(number, number)",
            &loc,
        ));
    }
    let flip = deep_value(vm, obj, "flip")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| {
            field_error(
                source,
                span,
                "flip",
                file_name,
                "flip must be a bool",
                &loc,
            )
        })?;
    let fn_src = if type_name == "Fn" {
        let run = deep_value(vm, obj, "run").unwrap_or(NIL);
        let is_fn = run
            .as_object()
            .map(|obj| vm.bx.heap.is_fn(obj))
            .unwrap_or(false);
        if !is_fn {
            return Err(field_error(
                source,
                span,
                "run",
                file_name,
                "Fn.run must be a closure",
                &loc,
            ));
        }
        fields
            .get("run")
            .copied()
            .or_else(|| source_field_in_chain(vm, obj, source, file_name, "run"))
            .map(|range| source[range.0..range.1].trim().to_string())
    } else {
        None
    };
    let face_src = fields
        .get("ui")
        .copied()
        .or_else(|| source_field_in_chain(vm, obj, source, file_name, "ui"))
        .map(|range| source[range.0..range.1].trim().to_string())
        .filter(|value| value != "nil");
    let on_fail = expect_id_field(vm, obj, "on_fail", source, span, file_name)?;
    if on_fail != "fail" && on_fail != "skip" {
        return Err(field_error(
            source,
            span,
            "on_fail",
            file_name,
            "on_fail must be @fail or @skip",
            &loc,
        ));
    }
    let label = match deep_value(vm, obj, "label").unwrap_or(NIL) {
        value if value.is_nil() => None,
        value => Some(value_string(vm, value).ok_or_else(|| {
            field_error(
                source,
                span,
                "label",
                file_name,
                "label must be a string",
                &loc,
            )
        })?),
    };
    let domain = if matches!(spec.kind, "gen" | "chat") {
        Some(expect_string_field(vm, obj, "domain", source, span, file_name)?)
    } else {
        None
    };
    let doc = vm
        .construction_chain(obj.into())
        .first()
        .and_then(|level| level.doc.clone());
    Ok(Node {
        id: id.to_string(),
        kind: spec.kind.to_string(),
        type_name: spec.type_name.to_string(),
        params,
        inputs,
        outputs,
        at: at_value,
        size: size_value,
        flip,
        loc,
        fn_src,
        face_src,
        on_fail,
        label,
        domain,
        doc,
    })
}

fn extract_input(
    vm: &ScriptVm<'_>,
    source: &str,
    file_name: &str,
    span: Option<(usize, usize)>,
    loc: &Loc,
    node_id: &str,
    name: &str,
    declared_ty: PortType,
    flexible: bool,
    value: ScriptValue,
    object_ids: &HashMap<ScriptObject, String>,
) -> Result<NodeInput, EvalError> {
    if let Some(edge) = port_ref(
        vm, value, object_ids, source, span, node_id, name, file_name, loc,
    )? {
        return Ok(NodeInput {
            port: name.to_string(),
            ty: if flexible { declared_ty } else { declared_ty },
            value: NodeInputValue::Edge(edge),
        });
    }
    if value
        .as_object()
        .map(|obj| vm.bx.heap.is_fn(obj))
        .unwrap_or(false)
    {
        return Err(field_error(
            source,
            span,
            name,
            file_name,
            "closure where a literal or port reference was expected",
            loc,
        ));
    }
    if let Some(referenced) = value.as_object().and_then(|object| object_ids.get(&object)) {
        return Err(field_error(
            source,
            span,
            name,
            file_name,
            format!("expected a port reference such as `{referenced}.text()`"),
            loc,
        ));
    }
    let literal = literal_from_value(vm, value)
        .map_err(|message| field_error(source, span, name, file_name, message, loc))?;
    let ty = if flexible {
        literal_type(&literal).unwrap_or(declared_ty)
    } else {
        declared_ty
    };
    Ok(NodeInput {
        port: name.to_string(),
        ty,
        value: NodeInputValue::Literal(literal),
    })
}

fn generic_gen_params(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
    loc: &Loc,
    input_names: &HashSet<String>,
) -> Result<Vec<(String, Literal)>, EvalError> {
    let reserved: HashSet<&str> = [
        "kind", "type_name", "domain", "ports", "at", "size", "flip", "ui", "on_fail", "label", "out",
    ]
    .into_iter()
    .collect();
    let mut params: Vec<(String, Literal)> = Vec::new();
    let chain = vm.construction_chain(obj.into());
    for level in chain.iter().rev() {
        if level.loc.as_ref().is_some_and(|location| location.file == PRELUDE_FILE) {
            continue;
        }
        let mut fields = Vec::new();
        vm.bx
            .heap
            .object_data(level.object)
            .map_iter_ordered(|key, value| {
                if let Some(name) = key_name(vm, key) {
                    fields.push((name, value));
                }
            });
        for (name, value) in fields {
            if reserved.contains(name.as_str()) || input_names.contains(&name) {
                continue;
            }
            if value
                .as_object()
                .is_some_and(|object| vm.bx.heap.is_fn(object))
            {
                continue;
            }
            let literal = literal_from_value(vm, value).map_err(|message| {
                field_error(source, span, &name, file_name, message, loc)
            })?;
            if let Some((_, old)) = params.iter_mut().find(|(param, _)| param == &name) {
                *old = literal;
            } else {
                params.push((name, literal));
            }
        }
    }
    Ok(params)
}

fn field_doc(vm: &ScriptVm<'_>, obj: ScriptObject, name: &str) -> Option<String> {
    let wanted = LiveId::from_str(name);
    vm.construction_chain(obj.into()).iter().find_map(|level| {
        level
            .field_docs
            .iter()
            .find_map(|(id, text)| (*id == wanted).then(|| text.clone()))
    })
}

fn display_number(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// Apply a documented numeric step before the evaluated graph reaches an
/// executor. Ranges remain editor bounds; the step is a backend contract.
fn snap_documented_param(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    context: &str,
    name: &str,
    literal: &mut Literal,
    warnings: &mut Vec<String>,
) {
    let Literal::Num(value) = literal else {
        return;
    };
    let Some(step) = field_doc(vm, obj, name)
        .map(|doc| parse_doc_hint(&doc))
        .and_then(|hint| hint.step)
        .filter(|step| step.is_finite() && *step > 0.0)
    else {
        return;
    };
    let snapped = (*value / step).round() * step;
    if (snapped - *value).abs() <= 1e-9 {
        return;
    }
    warnings.push(format!(
        "{context}: {name} {} snapped to {}",
        display_number(*value),
        display_number(snapped)
    ));
    *value = snapped;
}

fn port_ref(
    vm: &ScriptVm<'_>,
    value: ScriptValue,
    object_ids: &HashMap<ScriptObject, String>,
    source: &str,
    span: Option<(usize, usize)>,
    to_node: &str,
    field: &str,
    file_name: &str,
    loc: &Loc,
) -> Result<Option<EdgeRef>, EvalError> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if vm.bx.heap.is_fn(obj) {
        return Ok(None);
    }
    let Some(node_value) = own_value(vm, obj, "node") else {
        return Ok(None);
    };
    let Some(port_value) = own_value(vm, obj, "port") else {
        return Ok(None);
    };
    let node_obj = node_value.as_object().ok_or_else(|| {
        field_error(
            source,
            span,
            field,
            file_name,
            "invalid port reference node",
            loc,
        )
    })?;
    let from_node = object_ids.get(&node_obj).cloned().ok_or_else(|| {
        let referenced = source_port_ref_node(source, span, field).unwrap_or("unknown");
        field_error(
            source,
            span,
            field,
            file_name,
            format!(
                "node `{referenced}` is referenced by `{to_node}.{field}` but not listed in `Flow{{}}`"
            ),
            loc,
        )
    })?;
    let from_port = value_id(port_value).ok_or_else(|| {
        field_error(
            source,
            span,
            field,
            file_name,
            "invalid port reference port",
            loc,
        )
    })?;
    Ok(Some(EdgeRef {
        from_node,
        from_port,
    }))
}

fn looks_like_port_ref(vm: &ScriptVm<'_>, value: ScriptValue) -> bool {
    value.as_object().is_some_and(|obj| {
        !vm.bx.heap.is_fn(obj)
            && own_value(vm, obj, "node").is_some()
            && own_value(vm, obj, "port").is_some()
    })
}

fn source_port_ref_node<'a>(
    source: &'a str,
    span: Option<(usize, usize)>,
    field: &str,
) -> Option<&'a str> {
    let range = field_sources(source, span?).get(field).copied()?;
    let expression = source[range.0..range.1].trim();
    let name = expression.split_once('.')?.0.trim();
    (!name.is_empty()
        && name.as_bytes().first().is_some_and(|byte| is_ident_start(*byte))
        && name.as_bytes().iter().all(|byte| is_ident_continue(*byte)))
    .then_some(name)
}

fn outputs_for(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    type_name: &str,
    params: &[(String, Literal)],
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
    loc: &Loc,
    context: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<Port>, EvalError> {
    if type_spec(type_name).is_some_and(|spec| spec.kind == "gen") {
        return Ok(ports_side_typed(
            vm, obj, "out", source, span, file_name, loc, context, warnings,
        )?
        .into_iter()
        .map(|(name, ty)| Port { name, ty })
        .collect());
    }
    Ok(match type_name {
        "Input" | "Text" | "Ask" => vec![Port {
            name: port_type_name(param_port_type(params, "type").unwrap()).to_string(),
            ty: param_port_type(params, "type").unwrap(),
        }],
        "Output" => vec![],
        "Publish" => vec![Port {
            name: "asset".to_string(),
            ty: PortType::Json,
        }],
        "Llm" => vec![Port {
            name: "text".to_string(),
            ty: PortType::Text,
        }],
        "Fn" => {
            let names = param_id_array(params, "out").ok_or_else(|| {
                field_error(
                    source,
                    span,
                    "out",
                    file_name,
                    "Fn.out must be an array of port ids",
                    loc,
                )
            })?;
            names
                .into_iter()
                .map(|name| Port {
                    ty: port_type(&name).unwrap_or(PortType::Json),
                    name,
                })
                .collect()
        }
        "Http" => vec![
            Port {
                name: "value".to_string(),
                ty: param_port_type(params, "out").unwrap(),
            },
            Port {
                name: "meta".to_string(),
                ty: PortType::Json,
            },
        ],
        _ => unreachable!(),
    })
}

/// Read one side (`"in"` or `"out"`) of a `Gen`-kind node's `ports:`
/// declaration.
///
/// The current form is a typed map, `{name: @type, ...}`, and every named
/// port carries its own declared type. The old form, `[@name, ...]`, still
/// parses for one release: the type is inferred from the port's name (the
/// historical, ambiguous behavior a second image or text port could not
/// escape), and a deprecation note is pushed to `warnings`.
fn ports_side_typed(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    side: &str,
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
    loc: &Loc,
    context: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<(String, PortType)>, EvalError> {
    let ports = deep_value(vm, obj, "ports")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            field_error(
                source,
                span,
                "ports",
                file_name,
                "Gen.ports must be an object",
                loc,
            )
        })?;
    let value = deep_value(vm, ports, side).unwrap_or(NIL);
    if let Some(map_obj) = value.as_object() {
        if !vm.bx.heap.is_fn(map_obj) {
            let mut pairs = Vec::new();
            vm.bx.heap.object_data(map_obj).map_iter_ordered(|key, val| {
                if let Some(name) = key_name(vm, key) {
                    pairs.push((name, val));
                }
            });
            let mut out = Vec::with_capacity(pairs.len());
            for (name, val) in pairs {
                let ty = value_id(val).and_then(|id| port_type(&id)).ok_or_else(|| {
                    field_error(
                        source,
                        span,
                        "ports",
                        file_name,
                        format!("Gen.ports.{side}.{name} must be a port type"),
                        loc,
                    )
                })?;
                out.push((name, ty));
            }
            return Ok(out);
        }
    }
    if value.as_array().is_some() {
        let names = id_array(vm, value).ok_or_else(|| {
            field_error(
                source,
                span,
                "ports",
                file_name,
                format!("Gen.ports.{side} must be an array of port ids"),
                loc,
            )
        })?;
        warnings.push(format!(
            "{context}: ports.{side} uses the deprecated array form `[{}]`; declare `{side}: {{ name: @type, ... }}` with explicit types",
            names
                .iter()
                .map(|name| format!("@{name}"))
                .collect::<Vec<_>>()
                .join(", "),
        ));
        return Ok(names
            .into_iter()
            .map(|name| {
                let ty = if side == "in" {
                    input_port_type(&name)
                } else {
                    port_type(&name).unwrap_or(PortType::Json)
                };
                (name, ty)
            })
            .collect());
    }
    Err(field_error(
        source,
        span,
        "ports",
        file_name,
        format!("Gen.ports.{side} must be an object or an array of port ids"),
        loc,
    ))
}

fn validate_param(param: &ParamSpec, value: &Literal) -> Result<(), String> {
    let ok = match param.expected {
        ParamType::String => matches!(value, Literal::Str(_)),
        ParamType::Literal => true,
        ParamType::Number => matches!(value, Literal::Num(n) if n.is_finite()),
        ParamType::NonNegativeInteger => {
            matches!(value, Literal::Num(n) if n.is_finite() && *n >= 0.0 && n.fract() == 0.0)
        }
        ParamType::PortTypeNoBytes => match value {
            Literal::Id(value) => port_type(value).is_some_and(|ty| ty != PortType::Bytes),
            _ => false,
        },
        ParamType::PortTypeWithBytes => match value {
            Literal::Id(value) => port_type(value).is_some(),
            _ => false,
        },
        ParamType::HttpOut => match value {
            Literal::Id(value) => matches!(
                value.as_str(),
                "text" | "json" | "image" | "audio" | "video" | "bytes"
            ),
            _ => false,
        },
        ParamType::HttpMethod => {
            matches!(value, Literal::Id(value) if matches!(value.as_str(), "get" | "post" | "put" | "delete"))
        }
        ParamType::LiteralArray => matches!(value, Literal::Arr(_)),
    };
    if ok {
        Ok(())
    } else if matches!(param.expected, ParamType::HttpMethod) {
        Err(
            "wrong type for parameter `method`; allowed methods are @get, @post, @put, @delete"
                .to_string(),
        )
    } else {
        Err(format!("wrong type for parameter `{}`", param.name))
    }
}

fn resolve_flexible_input_types(nodes: &mut [Node]) {
    let outputs: HashMap<(String, String), PortType> = nodes
        .iter()
        .flat_map(|node| {
            node.outputs.iter().map(|port| {
                ((node.id.clone(), port.name.clone()), port.ty)
            })
        })
        .collect();
    for node in nodes {
        for input in &mut node.inputs {
            let flexible = node.kind == "fn"
                || node.kind == "publish"
                || (node.kind == "http" && input.port == "body");
            if !flexible {
                continue;
            }
            if let NodeInputValue::Edge(edge) = &input.value {
                if let Some(ty) = outputs.get(&(edge.from_node.clone(), edge.from_port.clone())) {
                    input.ty = *ty;
                }
            }
        }
    }
}

fn validate_edges(nodes: &[Node], edges: &[Edge], file_name: &str) -> Result<(), EvalError> {
    let node_map: HashMap<_, _> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    for edge in edges {
        let from = node_map[edge.from_node.as_str()];
        let to = node_map[edge.to_node.as_str()];
        let output = from
            .outputs
            .iter()
            .find(|port| port.name == edge.from_port)
            .ok_or_else(|| {
                at(
                    file_name,
                    to.loc.line,
                    to.loc.col,
                    format!(
                        "node `{}` has no output port `{}`",
                        edge.from_node, edge.from_port
                    ),
                )
            })?;
        let input = to
            .inputs
            .iter()
            .find(|port| port.port == edge.to_port)
            .unwrap();
        let flexible = to.kind == "fn"
            || to.kind == "publish"
            || (to.kind == "http" && input.port == "body");
        if !flexible && output.ty != input.ty {
            return Err(at(
                file_name,
                to.loc.line,
                to.loc.col,
                format!(
                    "type mismatch: {}.{} is {}, but {}.{} expects {}",
                    edge.from_node,
                    edge.from_port,
                    port_type_name(output.ty),
                    edge.to_node,
                    edge.to_port,
                    port_type_name(input.ty)
                ),
            ));
        }
    }
    Ok(())
}

fn validate_acyclic(nodes: &[Node], edges: &[Edge], file_name: &str) -> Result<(), EvalError> {
    let mut indegree: HashMap<&str, usize> = nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        *indegree.get_mut(edge.to_node.as_str()).unwrap() += 1;
        outgoing
            .entry(edge.from_node.as_str())
            .or_default()
            .push(edge.to_node.as_str());
    }
    let mut ready: Vec<&str> = nodes
        .iter()
        .filter(|node| indegree[node.id.as_str()] == 0)
        .map(|node| node.id.as_str())
        .collect();
    let mut seen = 0;
    while !ready.is_empty() {
        let id = ready.remove(0);
        seen += 1;
        for next in outgoing.get(id).into_iter().flatten() {
            let degree = indegree.get_mut(next).unwrap();
            *degree -= 1;
            if *degree == 0 {
                ready.push(next);
            }
        }
    }
    if seen != nodes.len() {
        let node = nodes
            .iter()
            .find(|node| indegree[node.id.as_str()] != 0)
            .unwrap();
        return Err(at(
            file_name,
            node.loc.line,
            node.loc.col,
            format!("cycle involving node `{}`", node.id),
        ));
    }
    Ok(())
}

fn extract_tools(
    vm: &ScriptVm<'_>,
    flow: ScriptObject,
    nodes: &[Node],
    edges: &[Edge],
    source: &str,
    flow_span: Option<(usize, usize)>,
    file_name: &str,
) -> Result<Vec<ToolEntry>, EvalError> {
    let input_ids: Vec<String> = nodes
        .iter()
        .filter(|node| node.kind == "input")
        .map(|node| node.id.clone())
        .collect();
    let output_ids: Vec<String> = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
        .map(|node| node.id.clone())
        .collect();
    let mut tools = vec![ToolEntry {
        name: "run".to_string(),
        inputs: input_ids,
        outputs: output_ids,
        nodes: nodes.iter().map(|node| node.id.clone()).collect(),
    }];
    let Some(tools_obj) = deep_value(vm, flow, "tools").and_then(|v| v.as_object()) else {
        return Err(field_error(
            source,
            flow_span,
            "tools",
            file_name,
            "Flow.tools must be an object",
            &object_loc(vm, flow, file_name),
        ));
    };
    let node_by_obj: HashMap<ScriptObject, &Node> = nodes
        .iter()
        .filter_map(|node| {
            // Resolve through the Flow object's own value so identity remains authoritative.
            own_value(vm, flow, &node.id)
                .and_then(|v| v.as_object())
                .map(|obj| (obj, node))
        })
        .collect();
    let mut custom = Vec::new();
    vm.bx.heap
        .object_data(tools_obj)
        .map_iter_ordered(|key, value| {
            if let Some(name) = key_name(vm, key) {
                custom.push((name, value));
            }
        });
    for (name, value) in custom {
        if name == "run" {
            return Err(field_error(
                source,
                flow_span,
                "tools",
                file_name,
                "`run` is reserved for the full-flow tool",
                &object_loc(vm, flow, file_name),
            ));
        }
        let entry = value.as_object().ok_or_else(|| {
            field_error(
                source,
                flow_span,
                "tools",
                file_name,
                "tool entry must be an object",
                &object_loc(vm, flow, file_name),
            )
        })?;
        let inputs = tool_node_array(vm, entry, "in", &node_by_obj, "input").map_err(|msg| {
            field_error(
                source,
                flow_span,
                "tools",
                file_name,
                msg,
                &object_loc(vm, flow, file_name),
            )
        })?;
        let outputs =
            tool_terminal_node_array(vm, entry, "out", &node_by_obj).map_err(|msg| {
                field_error(
                    source,
                    flow_span,
                    "tools",
                    file_name,
                    msg,
                    &object_loc(vm, flow, file_name),
                )
            })?;
        let keep = dependency_set(&outputs, edges);
        let selected_nodes = nodes
            .iter()
            .filter(|node| keep.contains(&node.id))
            .map(|node| node.id.clone())
            .collect();
        tools.push(ToolEntry {
            name,
            inputs,
            outputs,
            nodes: selected_nodes,
        });
    }
    Ok(tools)
}

fn tool_node_array(
    vm: &ScriptVm<'_>,
    entry: ScriptObject,
    field: &str,
    node_by_obj: &HashMap<ScriptObject, &Node>,
    expected_kind: &str,
) -> Result<Vec<String>, String> {
    let value = deep_value(vm, entry, field).ok_or_else(|| format!("tool.{field} is missing"))?;
    let array = value
        .as_array()
        .ok_or_else(|| format!("tool.{field} must be an array of nodes"))?;
    let mut ids = Vec::new();
    for index in 0..vm.bx.heap.array_len(array) {
        let value = vm.bx.heap.array_index_unchecked(array, index);
        let obj = value
            .as_object()
            .ok_or_else(|| format!("tool.{field} entry is not a node"))?;
        let node = node_by_obj
            .get(&obj)
            .ok_or_else(|| "node not in flow".to_string())?;
        if node.kind != expected_kind {
            return Err(format!(
                "tool.{field} node `{}` must be an {expected_kind} node",
                node.id
            ));
        }
        ids.push(node.id.clone());
    }
    Ok(ids)
}

fn tool_terminal_node_array(
    vm: &ScriptVm<'_>,
    entry: ScriptObject,
    field: &str,
    node_by_obj: &HashMap<ScriptObject, &Node>,
) -> Result<Vec<String>, String> {
    let value = deep_value(vm, entry, field).ok_or_else(|| format!("tool.{field} is missing"))?;
    let array = value
        .as_array()
        .ok_or_else(|| format!("tool.{field} must be an array of nodes"))?;
    let mut ids = Vec::new();
    for index in 0..vm.bx.heap.array_len(array) {
        let value = vm.bx.heap.array_index_unchecked(array, index);
        let obj = value
            .as_object()
            .ok_or_else(|| format!("tool.{field} entry is not a node"))?;
        let node = node_by_obj
            .get(&obj)
            .ok_or_else(|| "node not in flow".to_string())?;
        if !matches!(node.kind.as_str(), "output" | "publish") {
            return Err(format!(
                "tool.{field} node `{}` must be an output or Publish node",
                node.id
            ));
        }
        ids.push(node.id.clone());
    }
    Ok(ids)
}

fn dependency_set(outputs: &[String], edges: &[Edge]) -> HashSet<String> {
    let mut keep: HashSet<String> = outputs.iter().cloned().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for edge in edges {
            if keep.contains(&edge.to_node) && keep.insert(edge.from_node.clone()) {
                changed = true;
            }
        }
    }
    keep
}

fn param_port_type(params: &[(String, Literal)], name: &str) -> Option<PortType> {
    params.iter().find_map(|(param, value)| {
        if param == name {
            if let Literal::Id(value) = value {
                return port_type(value);
            }
        }
        None
    })
}

fn param_id_array(params: &[(String, Literal)], name: &str) -> Option<Vec<String>> {
    params.iter().find_map(|(param, value)| {
        if param != name {
            return None;
        }
        let Literal::Arr(values) = value else {
            return None;
        };
        values
            .iter()
            .map(|value| match value {
                Literal::Id(value) => Some(value.clone()),
                _ => None,
            })
            .collect()
    })
}

fn id_array(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<Vec<String>> {
    let array = value.as_array()?;
    let mut values = Vec::new();
    for index in 0..vm.bx.heap.array_len(array) {
        values.push(value_id(vm.bx.heap.array_index_unchecked(array, index))?);
    }
    Some(values)
}

fn infer_literal_type(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<PortType> {
    if value.as_object().is_some_and(|obj| vm.bx.heap.is_fn(obj)) {
        return None;
    }
    literal_from_value(vm, value)
        .ok()
        .and_then(|literal| literal_type(&literal))
}

fn literal_type(value: &Literal) -> Option<PortType> {
    Some(match value {
        Literal::Str(_) | Literal::Id(_) => PortType::Text,
        Literal::Arr(_) => PortType::List,
        Literal::Obj(_) | Literal::Bool(_) | Literal::Num(_) => PortType::Json,
        Literal::Null => return None,
    })
}

fn literal_from_value(vm: &ScriptVm<'_>, value: ScriptValue) -> Result<Literal, String> {
    literal_from_value_inner(vm, value, &mut HashSet::new(), &mut HashSet::new())
}

fn literal_from_value_inner(
    vm: &ScriptVm<'_>,
    value: ScriptValue,
    objects: &mut HashSet<ScriptObject>,
    arrays: &mut HashSet<ScriptArray>,
) -> Result<Literal, String> {
    if value.is_nil() {
        return Ok(Literal::Null);
    }
    if let Some(value) = value.as_bool() {
        return Ok(Literal::Bool(value));
    }
    if let Some(value) = value.as_number() {
        if !value.is_finite() {
            return Err("non-finite number is not a flow literal".to_string());
        }
        return Ok(Literal::Num(value));
    }
    if let Some(value) = value_string(vm, value) {
        return Ok(Literal::Str(value));
    }
    if let Some(value) = value_id(value) {
        return Ok(Literal::Id(value));
    }
    if let Some(array) = value.as_array() {
        if !arrays.insert(array) {
            return Err("cyclic array is not a flow literal".to_string());
        }
        let mut values = Vec::new();
        for index in 0..vm.bx.heap.array_len(array) {
            values.push(literal_from_value_inner(
                vm,
                vm.bx.heap.array_index_unchecked(array, index),
                objects,
                arrays,
            )?);
        }
        arrays.remove(&array);
        return Ok(Literal::Arr(values));
    }
    if let Some(obj) = value.as_object() {
        if vm.bx.heap.is_fn(obj) {
            return Err("closure is not a literal".to_string());
        }
        if !objects.insert(obj) {
            return Err("cyclic object is not a flow literal".to_string());
        }
        let mut values = Vec::new();
        vm.bx.heap.object_data(obj).map_iter_ordered(|key, value| {
            if let Some(key) = key_name(vm, key) {
                values.push((key, value));
            }
        });
        let mut literals = Vec::with_capacity(values.len());
        for (key, value) in values {
            literals.push((
                key,
                literal_from_value_inner(vm, value, objects, arrays)?,
            ));
        }
        objects.remove(&obj);
        return Ok(Literal::Obj(literals));
    }
    Err("value is not a flow literal".to_string())
}

fn own_value(vm: &ScriptVm<'_>, obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    let key: ScriptValue = LiveId::from_str(name).into();
    let data = vm.bx.heap.object_data(obj);
    data.map_get(&key).or_else(|| {
        data.vec
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value)
    })
}

fn deep_value(vm: &ScriptVm<'_>, mut obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    loop {
        if let Some(value) = own_value(vm, obj, name) {
            return Some(value);
        }
        obj = vm.bx.heap.proto(obj).as_object()?;
    }
}

fn key_name(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<String> {
    value_id(value).or_else(|| value_string(vm, value))
}

fn value_id(value: ScriptValue) -> Option<String> {
    value
        .as_id()
        .and_then(|id| id.as_string(|name| name.map(str::to_string)))
}

fn value_string(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<String> {
    vm.bx
        .heap
        .string_with(value, |_heap, value| value.to_string())
}

fn input_value(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    name: &str,
    default: DefaultValue,
) -> ScriptValue {
    let value = deep_value(vm, obj, name).unwrap_or(NIL);
    if matches!(default, DefaultValue::Null)
        && value
            .as_object()
            .is_some_and(|object| vm.bx.heap.is_fn(object))
    {
        NIL
    } else {
        value
    }
}

fn expect_string_field(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    name: &str,
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
) -> Result<String, EvalError> {
    let loc = object_loc(vm, obj, file_name);
    deep_value(vm, obj, name)
        .and_then(|value| value_string(vm, value))
        .ok_or_else(|| {
            field_error(
                source,
                span,
                name,
                file_name,
                format!("{name} must be a string"),
                &loc,
            )
        })
}

fn expect_id_field(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    name: &str,
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
) -> Result<String, EvalError> {
    let loc = object_loc(vm, obj, file_name);
    deep_value(vm, obj, name)
        .and_then(value_id)
        .ok_or_else(|| {
            field_error(
                source,
                span,
                name,
                file_name,
                format!("{name} must be an id"),
                &loc,
            )
        })
}

fn object_loc(vm: &ScriptVm<'_>, obj: ScriptObject, _file_name: &str) -> Loc {
    vm.bx
        .code
        .ip_to_loc(vm.bx.heap.made_at(obj))
        .map(|loc| Loc {
            line: loc.line,
            col: loc.col,
        })
        .unwrap_or(Loc { line: 1, col: 1 })
}

fn at(file: &str, line: u32, col: u32, message: impl Into<String>) -> EvalError {
    EvalError {
        file: file.to_string(),
        line,
        col,
        message: message.into(),
    }
}

pub(crate) fn source_size_error(file_name: &str) -> EvalError {
    at(
        file_name,
        1,
        1,
        "flow source exceeds the 192 KiB size budget",
    )
}

fn field_error(
    source: &str,
    span: Option<(usize, usize)>,
    field: &str,
    file_name: &str,
    message: impl Into<String>,
    fallback: &Loc,
) -> EvalError {
    let (line, col) = span
        .and_then(|span| field_sources(source, span).get(field).map(|range| range.0))
        .map(|offset| offset_line_col(source, offset))
        .unwrap_or((fallback.line, fallback.col));
    at(file_name, line, col, message)
}

fn source_range_error(
    source: &str,
    range: Option<(usize, usize)>,
    file_name: &str,
    message: impl Into<String>,
    fallback: &Loc,
) -> EvalError {
    let (line, col) = range
        .map(|range| offset_line_col(source, range.0))
        .unwrap_or((fallback.line, fallback.col));
    at(file_name, line, col, message)
}

fn object_span(vm: &ScriptVm<'_>, obj: ScriptObject, source: &str) -> Option<(usize, usize)> {
    let loc = vm.bx.code.ip_to_loc(vm.bx.heap.made_at(obj))?;
    let offset = line_col_offset(source, loc.line, loc.col)?;
    let open = find_next_code_char(source, offset, b'{')?;
    let close = matching_delimiter(source, open, b'{', b'}')?;
    Some((open, close + 1))
}

fn source_field_in_chain(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    source: &str,
    file_name: &str,
    field: &str,
) -> Option<(usize, usize)> {
    for level in vm.construction_chain(obj.into()) {
        if !level
            .loc
            .as_ref()
            .is_some_and(|location| location.file == file_name)
        {
            continue;
        }
        if own_value(vm, level.object, field).is_none() {
            continue;
        }
        let span = object_span(vm, level.object, source)?;
        if let Some(range) = field_sources(source, span).get(field).copied() {
            return Some(range);
        }
    }
    None
}

fn line_col_offset(source: &str, line: u32, col: u32) -> Option<usize> {
    let mut offset = 0usize;
    // Object construction lines are zero-based while columns are one-based;
    // source locations exposed by flow diagnostics are one-based.
    let target_line = line as usize + 1;
    for (index, text) in source.split_inclusive('\n').enumerate() {
        if index + 1 == target_line {
            let byte_col = text
                .char_indices()
                .nth(col.max(1) as usize - 1)
                .map(|(index, _)| index)
                .unwrap_or(text.len());
            return Some(offset + byte_col);
        }
        offset += text.len();
    }
    if target_line == source.lines().count() + 1 {
        Some(source.len())
    } else {
        None
    }
}

fn offset_line_col(source: &str, offset: usize) -> (u32, u32) {
    let prefix = &source[..offset.min(source.len())];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let col = prefix
        .rfind('\n')
        .map(|index| prefix[index + 1..].chars().count())
        .unwrap_or_else(|| prefix.chars().count()) as u32
        + 1;
    (line, col)
}

fn validate_source(source: &str, file_name: &str) -> Result<(), EvalError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(source_size_error(file_name));
    }
    let bytes = source.as_bytes();
    let mut braces = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = skip_string(bytes, index).ok_or_else(|| {
                    let (line, col) = offset_line_col(source, index);
                    at(file_name, line, col, "unterminated string literal")
                })?;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index);
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index).ok_or_else(|| {
                    let (line, col) = offset_line_col(source, index);
                    at(file_name, line, col, "unterminated block comment")
                })?;
            }
            b'{' => {
                braces.push(index);
                index += 1;
            }
            b'}' => {
                if braces.pop().is_none() {
                    let (line, col) = offset_line_col(source, index);
                    return Err(at(file_name, line, col, "unmatched closing brace"));
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    if let Some(open) = braces.pop() {
        let (line, col) = offset_line_col(source, open);
        return Err(at(file_name, line, col, "unterminated object literal"));
    }
    Ok(())
}

fn find_next_code_char(source: &str, start: usize, needle: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start.min(bytes.len());
    while index < bytes.len() {
        match bytes[index] {
            b'"' => index = skip_string(bytes, index)?,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => index = skip_block_comment(bytes, index)?,
            value if value == needle => return Some(index),
            _ => index += 1,
        }
    }
    None
}

fn matching_delimiter(source: &str, open: usize, left: u8, right: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => index = skip_string(bytes, index)?,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => index = skip_block_comment(bytes, index)?,
            value if value == left => {
                depth += 1;
                index += 1;
            }
            value if value == right => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

fn skip_string(bytes: &[u8], mut index: usize) -> Option<usize> {
    index += 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> Option<usize> {
    index += 2;
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return Some(index + 2);
        }
        index += 1;
    }
    None
}

fn field_sources(source: &str, span: (usize, usize)) -> HashMap<String, (usize, usize)> {
    let bytes = source.as_bytes();
    let mut keys = Vec::new();
    let mut index = span.0 + 1;
    let end = span.1.saturating_sub(1);
    let mut braces = 0usize;
    let mut brackets = 0usize;
    let mut parens = 0usize;
    while index < end {
        match bytes[index] {
            b'"' => {
                index = skip_string(bytes, index).unwrap_or(end);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index).unwrap_or(end);
                continue;
            }
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            byte if braces == 0 && brackets == 0 && parens == 0 && is_ident_start(byte) => {
                let key_start = index;
                index += 1;
                while index < end && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                let name = &source[key_start..index];
                let mut colon = index;
                while colon < end && bytes[colon].is_ascii_whitespace() {
                    colon += 1;
                }
                if bytes.get(colon) == Some(&b':') && bytes.get(colon + 1) != Some(&b'=') {
                    let mut value_start = colon + 1;
                    while value_start < end && bytes[value_start].is_ascii_whitespace() {
                        value_start += 1;
                    }
                    keys.push((name.to_string(), key_start, value_start));
                    index = value_start;
                    continue;
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    let mut out = HashMap::new();
    for key_index in 0..keys.len() {
        let (name, _key_start, value_start) = &keys[key_index];
        let mut value_end = keys
            .get(key_index + 1)
            .map(|(_, key_start, _)| *key_start)
            .unwrap_or(end);
        while value_end > *value_start
            && (bytes[value_end - 1].is_ascii_whitespace() || bytes[value_end - 1] == b',')
        {
            value_end -= 1;
        }
        out.insert(name.clone(), (*value_start, value_end));
    }
    out
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn parse_vec2(value: &str) -> Option<(f64, f64)> {
    let value = value.trim();
    let inner = value.strip_prefix("vec2(")?.strip_suffix(')')?;
    let mut values = inner.split(',').map(str::trim);
    let x = values.next()?.parse().ok()?;
    let y = values.next()?.parse().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some((x, y))
}

fn source_for_eval(source: &str) -> String {
    let mut out = source.as_bytes().to_vec();
    mask_ui_values(source, &mut out);
    String::from_utf8(out).unwrap()
}

fn mask_ui_values(source: &str, out: &mut [u8]) {
    let bytes = source.as_bytes();
    let mut stack = Vec::new();
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = skip_string(bytes, index).unwrap_or(bytes.len());
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index).unwrap_or(bytes.len());
                continue;
            }
            b'{' => stack.push(index),
            b'}' => {
                if let Some(open) = stack.pop() {
                    spans.push((open, index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    for span in spans {
        let Some(range) = field_sources(source, span).get("ui").copied() else {
            continue;
        };
        for byte in &mut out[range.0..range.1] {
            if !byte.is_ascii_whitespace() {
                *byte = b' ';
            }
        }
        if range.1.saturating_sub(range.0) >= 3 {
            out[range.0..range.0 + 3].copy_from_slice(b"nil");
        } else if range.0 < range.1 {
            out[range.0] = b'0';
        }
    }
}

fn port_type(value: &str) -> Option<PortType> {
    Some(match value {
        "text" => PortType::Text,
        "image" => PortType::Image,
        "audio" => PortType::Audio,
        "video" => PortType::Video,
        "mesh" => PortType::Mesh,
        "json" => PortType::Json,
        "list" => PortType::List,
        "bytes" => PortType::Bytes,
        _ => return None,
    })
}

fn input_port_type(name: &str) -> PortType {
    match name {
        "prompt" | "url" | "text" | "system" | "content_type" => PortType::Text,
        "image" => PortType::Image,
        "audio" => PortType::Audio,
        "video" => PortType::Video,
        "mesh" => PortType::Mesh,
        "headers" | "json" | "meta" => PortType::Json,
        "list" => PortType::List,
        "bytes" => PortType::Bytes,
        _ => PortType::Json,
    }
}

fn port_type_name(value: PortType) -> &'static str {
    match value {
        PortType::Text => "text",
        PortType::Image => "image",
        PortType::Audio => "audio",
        PortType::Video => "video",
        PortType::Mesh => "mesh",
        PortType::Json => "json",
        PortType::List => "list",
        PortType::Bytes => "bytes",
    }
}

mod schema;
mod writer;

pub use schema::tool_schema;
pub use writer::{is_canonical, write};
