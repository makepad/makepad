use crate::document::registry::{Registry, TypeDescriptor, ParamDescriptor, InputDescriptor, NodeCodec, WriteField, ParamType, BUILTIN_OWNER};
use crate::document::types::TypeRegistry;
use crate::document::closure::{Closure, COMPONENTS_MODULE, MAX_COMPONENT_DEPTH, MAX_INSTANCES};
use crate::wire::*;
use crate::{PortValue, Value};
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
///
/// With a module closure the VM also owns one evaluated object per
/// component instance path (`a`, `a/shout`, `b`, …): the module's source
/// evaluated once more for that instance, so every `let`, node object and
/// closure inside it is that instance's own. A qualified node id resolves
/// through that object exactly as a root id resolves through the `Flow{}`.
pub struct FlowVm {
    host: Box<ScriptVmHost<i32, ()>>,
    bx: Option<Box<ScriptVmBase>>,
    flow: ScriptObjectRef,
    graph: Graph,
    /// The lowered graph a run executes; `graph` stays the authored one.
    lowered: Option<Graph>,
    instances: HashMap<String, ScriptObjectRef>,
    registry: Registry,
}

/// A pinned component as evaluated once for its interface.
pub(crate) struct Declared {
    pub alias: String,
    pub object: ScriptObject,
    pub def: ComponentDef,
}

impl FlowVm {
    pub fn load(source: &str, file_name: &str) -> Result<(Self, Graph), EvalError> {
        Self::load_with(source, file_name, &Closure::default())
    }

    /// Load `source` with its module closure: every pinned module is
    /// installed under `mod.components.<alias>` in dependency order, the
    /// root is evaluated against them, and every container the root
    /// reaches gets its own evaluated instance object.
    pub fn load_with(source: &str, file_name: &str, closure: &Closure) -> Result<(Self, Graph), EvalError> {
        Self::load_with_registry(source, file_name, closure, &Registry::with_builtins())
    }

    pub fn load_with_registry(source: &str, file_name: &str, closure: &Closure, registry: &Registry) -> Result<(Self, Graph), EvalError> {
        validate_source(source, file_name)?;
        if let Some(module) = closure.modules.iter().find(|module| module.path == file_name) {
            // One file identity per source in the VM's body cache.
            return Err(at(&module.path, 1, 1, "a pinned module has the root flow's own file name"));
        }
        let mut host = Box::new(ScriptVmHost::new(0i32, ()));
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.set_script_domain("flow");
        vm.capture_errors();
        let (result, allocation) = vm.with_heap_allocation_limit(HEAP_LIMIT, |vm| {
            vm.with_instruction_limit(INSTRUCTION_LIMIT, |vm| {
                install_prelude(vm, registry)?;
                let declared = install_declared(vm, closure, "", registry)?;
                let value = evaluate_in(vm, source, file_name)?;
                let mut graph = extract(vm, value, source, file_name, &declared, "Flow", registry)?;
                graph.components = declared.iter().map(|entry| entry.def.clone()).collect();
                let flow = value
                    .as_object()
                    .ok_or_else(|| at(file_name, 1, 1, "last expression is not a Flow{}"))?;
                let mut instances = HashMap::new();
                let mut count = 0usize;
                instantiate_containers(vm, closure, &declared, &graph, "", 0, &mut count, &mut instances, "")?;
                Ok((graph, vm.bx.heap_mut().new_object_ref(flow), instances))
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
        let (graph, flow, instances) = result?;
        let bx = vm.bx;
        let loaded = Self {
            host,
            bx: Some(bx),
            flow,
            graph: graph.clone(),
            lowered: None,
            instances,
            registry: registry.clone(),
        };
        Ok((loaded, graph))
    }

    /// The graph a run executes (the lowering of the authored one); node
    /// ids it names are qualified and resolve through the instance objects.
    pub fn set_run_graph(&mut self, graph: Graph) {
        self.lowered = Some(graph);
    }

    /// Every container instance path this VM evaluated.
    pub fn instance_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.instances.keys().cloned().collect();
        paths.sort();
        paths
    }


    pub fn call_fn(
        &mut self,
        node_id: &str,
        inputs: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, String> {
        let shaped: Vec<(String, PortValue)> =
            inputs.iter().map(|(port, value)| (port.clone(), PortValue::One(value.clone()))).collect();
        self.call_fn_shaped(node_id, &shaped)
    }

    /// Call an Fn with inputs in their declared shapes: a one input is its
    /// value, an ordered input a script array of its elements in order
    /// (media as the usual handles).
    pub fn call_fn_shaped(
        &mut self,
        node_id: &str,
        inputs: &[(String, PortValue)],
    ) -> Result<Vec<(String, Value)>, String> {
        let node = self
            .lowered
            .as_ref()
            .unwrap_or(&self.graph)
            .nodes
            .iter()
            .find(|node| node.id == node_id && node.kind == "fn")
            .cloned()
            .ok_or_else(|| format!("Fn node `{node_id}` is not declared"))?;
        // Shapes and counts first: nothing is cloned for an input that is
        // refused.
        for (port, value) in inputs {
            let declared = node
                .inputs
                .iter()
                .find(|input| input.port == *port)
                .ok_or_else(|| format!("Fn node `{node_id}` has no input port `{port}`"))?;
            value
                .matches(declared.shape())
                .map_err(|error| format!("Fn node `{node_id}` input `{port}`: {error}"))?;
        }
        for value in inputs.iter().flat_map(|(_, slot)| slot.values()) {
            self.registry.types().require_artifact(&value.ty).map_err(|error| error.to_string())?;
        }
        for output in &node.outputs {
            self.registry.types().require_artifact(&output.ty).map_err(|error| error.to_string())?;
        }
        let media: HashMap<_, _> = inputs
            .iter()
            .flat_map(|(_, value)| value.values().iter())
            .filter(|value| !matches!(value.ty, PortType::Text | PortType::Json | PortType::List))
            .map(|value| ((value.digest_hex(), value.ty.as_str().to_string()), value.clone()))
            .collect();
        let flow = self.flow.as_object();
        let node_obj = {
            let instances = &self.instances;
            let vm = ScriptVm {
                host: self.host.host_context(),
                bx: self.bx.take().expect("vm base"),
            };
            let found = node_object(&vm, flow, instances, node_id);
            self.bx = Some(vm.bx);
            found.ok_or_else(|| format!("Fn node `{node_id}` is missing from run VM"))?
        };
        self.with_vm(|vm| {
            let run = deep_value(vm, node_obj, "run")
                .ok_or_else(|| format!("Fn node `{node_id}` has no run closure"))?;
            let args = vm.bx.heap_mut().new_object();
            for (port, value) in inputs {
                let script_value = match value {
                    PortValue::One(value) => value_to_script(vm, value)?,
                    PortValue::Seq(sequence) => {
                        let array = vm.bx.heap_mut().new_array();
                        for element in &sequence.values {
                            let element = value_to_script(vm, element)?;
                            vm.bx.heap_mut().array_push_unchecked(array, element);
                        }
                        array.into()
                    }
                };
                vm.bx.heap_mut().set_value_def(
                    args,
                    LiveId::from_str(port).into(),
                    script_value,
                );
            }
            vm.capture_errors();
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
                    script_to_value(vm, value, output.ty.clone(), &media).map_err(|error| {
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
            host: self.host.host_context(),
            bx,
        };
        let result = f(&mut vm);
        self.bx = Some(vm.bx);
        result
    }
}

#[derive(Clone)]
struct TypeSpec {
    params: &'static [ParamSpec],
    inputs: &'static [InputSpec],
}

#[derive(Clone, Copy)]
struct ParamSpec {
    name: &'static str,
    expected: ParamType,
    default: DefaultValue,
}

#[derive(Clone)]
struct InputSpec {
    name: &'static str,
    ty: PortType,
    default: DefaultValue,
    flexible: bool,
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
    ParamSpec::new("many", ParamType::FanIn, DefaultValue::Null),
    ParamSpec::new("value", ParamType::Literal, DefaultValue::Str("")),
];
const OUTPUT_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("type", ParamType::PortTypeWithBytes, DefaultValue::Id("text")),
    ParamSpec::new("many", ParamType::FanIn, DefaultValue::Null),
];
const PUBLISH_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("title", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("namespace", ParamType::String, DefaultValue::Str("flows")),
    ParamSpec::new("tags", ParamType::LiteralArray, DefaultValue::Arr),
    ParamSpec::new("description", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("alias", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("artist", ParamType::String, DefaultValue::Str("")),
    ParamSpec::new("bpm", ParamType::Number, DefaultValue::Num(0.0)),
    ParamSpec::new("recipe_digest", ParamType::String, DefaultValue::Str("")),
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
const MAP_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("type", ParamType::PortTypeWithBytes, DefaultValue::Id("image")),
    ParamSpec::new("many", ParamType::FanIn, DefaultValue::Num(8.0)),
    ParamSpec::new("bind", ParamType::Literal, DefaultValue::Id("seed")),
    ParamSpec::new("concurrency", ParamType::NonNegativeInteger, DefaultValue::Num(2.0)),
];
const CHOOSE_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("type", ParamType::PortTypeWithBytes, DefaultValue::Id("image")),
    ParamSpec::new("many", ParamType::FanIn, DefaultValue::Num(8.0)),
    ParamSpec::new("question", ParamType::String, DefaultValue::Str("")),
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
    ParamSpec::new("seed", ParamType::Literal, DefaultValue::Id("random")),
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
// `body`'s type and `from`'s type and bound are the node's `type` and
// `many`; these are the catalog's defaults.
const MAP_INPUTS: &[InputSpec] = &[
    InputSpec::new("body", PortType::Image, DefaultValue::Null),
    InputSpec::new("over", PortType::List, DefaultValue::Arr),
];
const CHOOSE_INPUTS: &[InputSpec] = &[
    InputSpec::new("from", PortType::Image, DefaultValue::Null),
    InputSpec::new("pick", PortType::Json, DefaultValue::Null),
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

fn builtin_spec(type_name: &str) -> Option<TypeSpec> {
    Some(match type_name {
        "Input" | "Text" => TypeSpec {
            params: INPUT_PARAMS,
            inputs: &[],
        },
        "Output" => TypeSpec {
            params: OUTPUT_PARAMS,
            inputs: OUTPUT_INPUTS,
        },
        "Publish" => TypeSpec {
            params: PUBLISH_PARAMS,
            inputs: PUBLISH_INPUTS,
        },
        "Llm" => TypeSpec {
            params: LLM_PARAMS,
            inputs: LLM_INPUTS,
        },
        "Fn" => TypeSpec {
            params: FN_PARAMS,
            inputs: &[],
        },
        "Http" => TypeSpec {
            params: HTTP_PARAMS,
            inputs: HTTP_INPUTS,
        },
        "Ask" => TypeSpec {
            params: ASK_PARAMS,
            inputs: &[],
        },
        "Map" => TypeSpec {
            params: MAP_PARAMS,
            inputs: MAP_INPUTS,
        },
        "Choose" => TypeSpec {
            params: CHOOSE_PARAMS,
            inputs: CHOOSE_INPUTS,
        },
        "Image" => TypeSpec {
            params: IMAGE_PARAMS,
            inputs: IMAGE_INPUTS,
        },
        "Upscale" => TypeSpec {
            params: UPSCALE_PARAMS,
            inputs: UPSCALE_INPUTS,
        },
        "Gen" => TypeSpec {
            params: &[],
            inputs: &[],
        },
        _ => return None,
    })
}

pub(crate) fn builtin_descriptors(types: &TypeRegistry) -> Result<Vec<TypeDescriptor>, EvalError> {
    let mut out = Vec::new();
    for catalog in builtin_catalog(types)? {
        let spec = builtin_spec(&catalog.type_name);
        let codec = match catalog.type_name.as_str() {
            "Input" | "Text" => NodeCodec::Input,
            "Output" => NodeCodec::Output,
            "Fn" => NodeCodec::Fn,
            "Http" => NodeCodec::Http,
            "Ask" => NodeCodec::Ask,
            "Map" => NodeCodec::Map,
            "Choose" => NodeCodec::Choose,
            _ if catalog.kind == "gen" => NodeCodec::Gen,
            _ => NodeCodec::Fields,
        };
        let params = catalog.params.iter().map(|param| ParamDescriptor {
            name: param.name.clone(),
            expected: spec.as_ref().and_then(|spec| spec.params.iter().find(|p| p.name == param.name))
                .map_or(ParamType::Literal, |p| p.expected),
            default: spec.as_ref().and_then(|spec| spec.params.iter().find(|p| p.name == param.name))
                .map_or_else(|| json_literal(&param.default), |p| default_literal(p.default)),
            // A builtin's description and range stay in the prelude's doc
            // comments, which the catalog reads.
            doc: None, range: None, endpoint: None, signal: None,
        }).collect();
        let inputs = catalog.ports._in.iter().map(|port| {
            let input = spec.as_ref().and_then(|spec| spec.inputs.iter().find(|input| input.name == port.name));
            InputDescriptor {
                name: port.name.clone(), ty: port.ty.clone(), many: port.many,
                default: input.map_or(Literal::Null, |input| default_literal(input.default)),
                flexible: input.is_some_and(|input| input.flexible),
            }
        }).collect();
        let p = |name: &str, force| WriteField::Param { name: name.into(), force };
        let i = |name: &str, force| WriteField::Input { name: name.into(), force };
        let write = match catalog.type_name.as_str() {
            "Input" | "Text" => vec![p("type", false), p("many", false), p("value", false)],
            "Output" => vec![p("type", false), p("many", false), i("value", true)],
            "Publish" => vec![p("title", false), p("namespace", false), p("tags", false), p("description", false), p("alias", false), p("artist", false), p("bpm", false), p("recipe_digest", false), i("value", true)],
            "Llm" => vec![p("system", false), i("prompt", false), p("model", false), p("temperature", false), p("max_tokens", false)],
            "Fn" => vec![WriteField::FnBody],
            "Http" => vec![p("method", false), i("url", false), i("headers", false), i("body", false), p("content_type", false), p("out", false), p("accept", false)],
            "Ask" => vec![p("question", false), p("type", false), p("options", false), p("default", false), p("timeout", false)],
            "Map" => vec![p("type", true), p("many", true), i("body", true), i("over", true), p("bind", true), p("concurrency", false)],
            "Choose" => vec![p("type", true), p("many", true), i("from", true), i("pick", false), p("question", false), p("timeout", false)],
            "Image" => vec![WriteField::Ports, WriteField::Inputs, p("width", true), p("height", true), p("steps", false), p("seed", false), p("negative", false), p("model", false)],
            "Upscale" => vec![WriteField::Ports, WriteField::Inputs, p("factor", false)],
            _ => vec![WriteField::Domain, WriteField::Ports, WriteField::Params, WriteField::Inputs],
        };
        out.push(TypeDescriptor {
            type_name: catalog.type_name, kind: catalog.kind, owner: BUILTIN_OWNER.into(),
            params, inputs, outputs: catalog.ports.out, codec, write,
            domain: catalog.domain, face: catalog.face, doc: catalog.doc,
        });
    }
    Ok(out)
}

fn json_literal(value: &JsonValue) -> Literal {
    match value {
        JsonValue::Null => Literal::Null,
        JsonValue::Bool(value) => Literal::Bool(*value),
        JsonValue::I64(value) => Literal::Num(*value as f64),
        JsonValue::U64(value) => Literal::Num(*value as f64),
        JsonValue::I128(value) => Literal::Num(*value as f64),
        JsonValue::U128(value) => Literal::Num(*value as f64),
        JsonValue::F64(value) => Literal::Num(*value),
        JsonValue::Char(value) => Literal::Str(value.to_string()),
        JsonValue::BareIdent(value) => Literal::Id(value.clone()),
        JsonValue::Undefined => Literal::Null,
        JsonValue::String(value) => Literal::Str(value.clone()),
        JsonValue::Array(values) => Literal::Arr(values.iter().map(json_literal).collect()),
        JsonValue::Object(values) => {
            let mut fields: Vec<_> = values.iter().map(|(name, value)| (name.clone(), json_literal(value))).collect();
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            Literal::Obj(fields)
        }
    }
}

fn default_literal(value: DefaultValue) -> Literal {
    match value {
        DefaultValue::Null => Literal::Null,
        DefaultValue::Num(value) => Literal::Num(value),
        DefaultValue::Str(value) => Literal::Str(value.into()),
        DefaultValue::Id(value) => Literal::Id(value.into()),
        DefaultValue::Arr => Literal::Arr(Vec::new()),
        DefaultValue::Obj => Literal::Obj(Vec::new()),
    }
}

/// The VM a flow (or an edit) is evaluated in: the `mod.flow` module, the
/// flow prelude and recipe prelude, and `registry`'s node prototypes.
/// Errors are left captured-clean for the caller's own evaluation.
pub fn install_prelude(vm: &mut ScriptVm<'_>, registry: &Registry) -> Result<(), EvalError> {
    vm.capture_errors();
    vm.new_module(id!(flow));
    vm.eval(make_mod(PRELUDE_FILE, crate::PRELUDE));
    let prelude_errors = vm.take_errors();
    if !prelude_errors.is_empty() {
        return Err(error_from_vm(&prelude_errors[0], PRELUDE_FILE));
    }
    vm.capture_errors();
    vm.eval(make_mod(RECIPE_PRELUDE_FILE, RECIPE_PRELUDE));
    let recipe_errors = vm.take_errors();
    if !recipe_errors.is_empty() {
        return Err(error_from_vm(&recipe_errors[0], RECIPE_PRELUDE_FILE));
    }
    vm.capture_errors();
    install_registry(vm, registry)
}

/// Run `f` on `vm` under the flow evaluator's instruction and heap budgets.
pub fn within_limits<R>(vm: &mut ScriptVm<'_>, file_name: &str, f: impl FnOnce(&mut ScriptVm<'_>) -> R) -> Result<R, EvalError> {
    let (result, allocation) = vm.with_heap_allocation_limit(HEAP_LIMIT, |vm| vm.with_instruction_limit(INSTRUCTION_LIMIT, f));
    if allocation.exceeded {
        return Err(at(file_name, 1, 1, "flow exceeded the 64 MiB heap allocation limit"));
    }
    Ok(result)
}

/// Evaluate `source` (named `file_name`) in a VM prepared by
/// [`install_prelude`]; its value, or its first error.
pub fn evaluate_in(vm: &mut ScriptVm<'_>, source: &str, file_name: &str) -> Result<ScriptValue, EvalError> {
    vm.capture_errors();
    let eval_source = source_for_eval(source);
    let value = vm.eval(make_mod(file_name, &eval_source));
    let errors = vm.take_errors();
    vm.capture_errors();
    if !errors.is_empty() {
        return Err(error_from_vm(&errors[0], file_name));
    }
    Ok(value)
}

/// The graph of an evaluated `Flow{}` value in a VM prepared by
/// [`install_prelude`], whose `source` (named `file_name`) it was evaluated
/// from: the extraction `FlowVm::load_with_registry` ends with.
pub fn graph_of(vm: &ScriptVm<'_>, flow: ScriptValue, source: &str, file_name: &str, registry: &Registry) -> Result<Graph, EvalError> {
    extract(vm, flow, source, file_name, &[], "Flow", registry)
}

/// Install fixed provider prototypes after the builtin and recipe preludes,
/// before any pinned module or authored source is evaluated.
pub fn install_registry(vm: &mut ScriptVm<'_>, registry: &Registry) -> Result<(), EvalError> {
    let mut source = String::new();
    for descriptor in registry.descriptors().filter(|descriptor| descriptor.owner != BUILTIN_OWNER) {
        source.push_str("mod.flow.");
        source.push_str(&descriptor.type_name);
        source.push_str(" = mod.flow.Node{ type_name: ");
        writer::write_literal(&mut source, &Literal::Str(descriptor.type_name.clone()));
        source.push_str(" kind: @provider\n");
        for (name, value) in descriptor.params.iter().map(|param| (&param.name, &param.default))
            .chain(descriptor.inputs.iter().map(|input| (&input.name, &input.default)))
        {
            source.push_str(name);
            source.push_str(": ");
            writer::write_literal(&mut source, value);
            source.push('\n');
        }
        source.push_str("}\n");
    }
    if source.is_empty() { return Ok(()) }
    // A module's last statement is its result expression, so an assignment
    // in that position never lands: the last registered prototype would be
    // missing (the only one, for a single provider). The preludes end the
    // same way.
    source.push_str("nil\n");
    vm.capture_errors();
    vm.eval(make_mod("<flow-registry>", &source));
    if let Some(error) = vm.take_errors().first() {
        return Err(error_from_vm(error, "<flow-registry>"));
    }
    vm.capture_errors();
    Ok(())
}

fn make_mod(file: &str, code: &str) -> ScriptMod {
    ScriptMod::new("", "", &(file), 0, 0, &(code), &[])
}

/// Evaluate one flow source in an isolated, budgeted splash VM.
pub fn evaluate(source: &str, file_name: &str) -> Result<Graph, EvalError> {
    FlowVm::load(source, file_name).map(|(_, graph)| graph)
}

/// Evaluate one flow source with its module closure.
pub fn evaluate_with(source: &str, file_name: &str, closure: &Closure) -> Result<Graph, EvalError> {
    evaluate_with_registry(source, file_name, closure, &Registry::with_builtins())
}

pub fn evaluate_with_registry(source: &str, file_name: &str, closure: &Closure, registry: &Registry) -> Result<Graph, EvalError> {
    FlowVm::load_with_registry(source, file_name, closure, registry).map(|(_, graph)| graph)
}

/// Evaluate every module of `closure` into `mod.components.<alias>` in a
/// VM that already holds the flow prelude, prefixing each source with
/// `prefix` (a host's own `use` line, or nothing). Returns the declaration
/// objects. Shared by the run VM and a host's face isolate.
pub fn install_closure(vm: &mut ScriptVm<'_>, closure: &Closure, prefix: &str) -> Result<Vec<(String, ScriptObject)>, EvalError> {
    let components = vm.new_module(LiveId::from_str(COMPONENTS_MODULE));
    let mut out = Vec::with_capacity(closure.modules.len());
    for module in &closure.modules {
        let object = eval_component(vm, module, prefix)?;
        vm.bx.heap_mut().set_value_def(components, LiveId::from_str(&module.alias).into(), object.into());
        out.push((module.alias.clone(), object));
    }
    Ok(out)
}

/// Evaluate `alias`'s module once more: a fresh `Component{}` whose nodes,
/// bindings and closures belong to one instance. Every module it names
/// must already be installed (see [`install_closure`]).
pub fn instantiate_module(vm: &mut ScriptVm<'_>, closure: &Closure, alias: &str, prefix: &str) -> Result<ScriptObject, EvalError> {
    let module = closure
        .module(alias)
        .ok_or_else(|| at("<closure>", 1, 1, format!("component `{alias}` is not pinned")))?;
    eval_component(vm, module, prefix)
}

fn eval_component(vm: &mut ScriptVm<'_>, module: &ModuleSource, prefix: &str) -> Result<ScriptObject, EvalError> {
    validate_source(&module.source, &module.path)?;
    vm.capture_errors();
    let code = if prefix.is_empty() { source_for_eval(&module.source) } else { format!("{prefix}{}", source_for_eval(&module.source)) };
    let value = vm.eval(make_mod(&module.path, &code));
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(error_from_vm(&errors[0], &module.path));
    }
    vm.capture_errors();
    value
        .as_object()
        .ok_or_else(|| at(&module.path, 1, 1, "last expression is not a Component{}"))
}

/// Install the closure and extract each declaration's interface and graph,
/// in order, so a module may contain containers of the modules before it.
pub(crate) fn install_declared(vm: &mut ScriptVm<'_>, closure: &Closure, prefix: &str, registry: &Registry) -> Result<Vec<Declared>, EvalError> {
    let components = vm.new_module(LiveId::from_str(COMPONENTS_MODULE));
    let mut declared: Vec<Declared> = Vec::with_capacity(closure.modules.len());
    for module in &closure.modules {
        let object = eval_component(vm, module, prefix)?;
        vm.bx.heap_mut().set_value_def(components, LiveId::from_str(&module.alias).into(), object.into());
        let graph = extract(vm, object.into(), &module.source, &module.path, &declared, "Component", registry)?;
        let inputs = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "input")
            .filter_map(|node| node.outputs.first().map(|port| Port { name: node.id.clone(), ty: port.ty.clone(), many: port.many }))
            .collect();
        let outputs = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "output")
            .filter_map(|node| {
                node.inputs
                    .iter()
                    .find(|input| input.port == "value")
                    .map(|input| Port { name: node.id.clone(), ty: input.ty.clone(), many: input.many })
            })
            .collect();
        declared.push(Declared {
            alias: module.alias.clone(),
            object,
            def: ComponentDef {
                alias: module.alias.clone(),
                path: module.path.clone(),
                hash: module.hash.clone(),
                inputs,
                outputs,
                graph,
            },
        });
    }
    Ok(declared)
}

/// One fresh evaluation per container instance path, recursively, within
/// the depth and instance budgets.
#[allow(clippy::too_many_arguments)]
fn instantiate_containers(
    vm: &mut ScriptVm<'_>,
    closure: &Closure,
    declared: &[Declared],
    graph: &Graph,
    prefix: &str,
    depth: usize,
    count: &mut usize,
    instances: &mut HashMap<String, ScriptObjectRef>,
    file_name: &str,
) -> Result<(), EvalError> {
    for node in &graph.nodes {
        if node.kind != "component" {
            continue;
        }
        let path = format!("{prefix}{}", node.id);
        if depth >= MAX_COMPONENT_DEPTH {
            return Err(at_loc(
                file_name,
                &node.loc,
                format!("component `{path}` nests deeper than the {MAX_COMPONENT_DEPTH} level limit"),
            ));
        }
        *count += 1;
        if *count > MAX_INSTANCES {
            return Err(at_loc(
                file_name,
                &node.loc,
                format!("component `{path}` is the {MAX_INSTANCES}th instance and over the limit"),
            ));
        }
        let entry = declared
            .iter()
            .find(|entry| entry.alias == node.type_name)
            .ok_or_else(|| at_loc(file_name, &node.loc, format!("component `{}` is not pinned", node.type_name)))?;
        let object = instantiate_module(vm, closure, &entry.alias, "").map_err(|error| EvalError {
            message: format!("instance `{path}`: {}", error.message),
            ..error
        })?;
        // The instance runs the graph the declaration was planned from: every
        // declared node must exist on the fresh object under its own name.
        // Module code that shapes a different object graph per evaluation is
        // refused here, not silently run.
        for declared_node in &entry.def.graph.nodes {
            if own_value(vm, object, &declared_node.id).and_then(|value| value.as_object()).is_none() {
                return Err(at_loc(
                    &entry.def.path,
                    &declared_node.loc,
                    format!(
                        "instance `{path}` of `{}` does not match its declaration: node `{}` is missing",
                        entry.alias, declared_node.id
                    ),
                ));
            }
        }
        instances.insert(path.clone(), vm.bx.heap_mut().new_object_ref(object));
        let inner_prefix = format!("{path}/");
        instantiate_containers(vm, closure, declared, &entry.def.graph, &inner_prefix, depth + 1, count, instances, &entry.def.path)?;
    }
    Ok(())
}

/// The Splash object a node id names: a root id is an own field of the
/// `Flow{}`; `path/name` is an own field of the instance object evaluated
/// for `path`.
fn node_object(
    vm: &ScriptVm<'_>,
    flow: ScriptObject,
    instances: &HashMap<String, ScriptObjectRef>,
    node_id: &str,
) -> Option<ScriptObject> {
    let (container, name) = match node_id.rsplit_once('/') {
        Some((path, name)) => (instances.get(path)?.as_object(), name),
        None => (flow, node_id),
    };
    own_value(vm, container, name).and_then(|value| value.as_object())
}

/// The pinned component an instance object derives from, if any: the first
/// declaration object on its prototype chain (the object itself excluded,
/// so a declaration listed as a node is not an instance).
fn component_of<'a>(vm: &ScriptVm<'_>, object: ScriptObject, declared: &'a [Declared]) -> Option<&'a Declared> {
    let mut cur = vm.bx.heap_ref().proto(object).as_object();
    let mut depth = 0;
    while let Some(obj) = cur {
        if let Some(entry) = declared.iter().find(|entry| entry.object == obj) {
            return Some(entry);
        }
        cur = vm.bx.heap_ref().proto(obj).as_object();
        depth += 1;
        if depth > 32 {
            break;
        }
    }
    None
}

/// The prototype a package manifest instantiates. It carries no defaults the
/// reader relies on: the manifest reader sees only the fields the file sets.
/// A module's last statement is its result expression, so the prelude ends in
/// `nil` for the assignment before it to take effect (as `prelude.splash`).
const MANIFEST_PRELUDE_FILE: &str = "<makepad-flow-manifest-prelude>";
const MANIFEST_PRELUDE: &str = "mod.app.App = {\n    type_name: \"App\"\n}\n\nnil\n";
const MANIFEST_INSTRUCTION_LIMIT: usize = 200_000;
const MANIFEST_HEAP_LIMIT: usize = 8 * 1024 * 1024;

/// Evaluate a package manifest (`use mod.app.*` ... `App{...}`) in its own
/// small budgeted VM and return the fields the file set, in source order.
/// The flow prelude is not loaded: a manifest cannot declare nodes.
pub(crate) fn evaluate_manifest(source: &str, file_name: &str) -> Result<Vec<(String, Literal)>, EvalError> {
    validate_source(source, file_name)?;
    let mut host = Box::new(ScriptVmHost::new(0i32, ()));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.capture_errors();
    let (result, allocation) = vm.with_heap_allocation_limit(MANIFEST_HEAP_LIMIT, |vm| {
        vm.with_instruction_limit(MANIFEST_INSTRUCTION_LIMIT, |vm| {
            vm.new_module(id!(app));
            vm.eval(make_mod(MANIFEST_PRELUDE_FILE, MANIFEST_PRELUDE));
            let prelude_errors = vm.take_errors();
            if !prelude_errors.is_empty() {
                return Err(error_from_vm(&prelude_errors[0], MANIFEST_PRELUDE_FILE));
            }
            vm.capture_errors();
            let value = vm.eval(make_mod(file_name, source));
            let errors = vm.take_errors();
            if !errors.is_empty() {
                return Err(error_from_vm(&errors[0], file_name));
            }
            let object = value
                .as_object()
                .ok_or_else(|| at(file_name, 1, 1, "last expression is not an App{}"))?;
            let is_app = deep_value(vm, object, "type_name")
                .and_then(|value| value_string(vm, value))
                .is_some_and(|name| name == "App");
            if !is_app {
                return Err(at(file_name, 1, 1, "last expression is not an App{}"));
            }
            match literal_from_value(vm, value) {
                Ok(Literal::Obj(fields)) => Ok(fields),
                Ok(_) => Err(at(file_name, 1, 1, "last expression is not an App{}")),
                Err(error) => Err(at(file_name, 1, 1, format!("manifest value: {error}"))),
            }
        })
    });
    if allocation.exceeded {
        return Err(at(file_name, 1, 1, "manifest exceeded the 8 MiB heap allocation limit"));
    }
    result
}

fn value_to_script(vm: &mut ScriptVm<'_>, value: &Value) -> Result<ScriptValue, String> {
    if value.ty == PortType::Text {
        return Ok(vm.bx.heap_mut().new_string_from_str(value.as_text()?).into());
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
    let object = vm.bx.heap_mut().new_object();
    let digest = vm.bx.heap_mut().new_string_from_str(&value.digest_hex());
    let ty = vm.bx.heap_mut().new_string_from_str(value.ty.as_str());
    let content_type = vm.bx.heap_mut().new_string_from_str(&value.content_type);
    vm.bx.heap_mut().set_value_def(object, id!(type).into(), ty);
    vm.bx.heap_mut().set_value_def(object, id!(digest).into(), digest);
    vm.bx.heap_mut().set_value_def(object, id!(content_type).into(), content_type);
    vm.bx.heap_mut().set_value_def(
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
        Json::Str(value) => vm.bx.heap_mut().new_string_from_str(value),
        Json::Arr(values) => {
            let array = vm.bx.heap_mut().new_array();
            for value in values {
                let value = json_to_script(vm, value);
                vm.bx.heap_mut().array_push_unchecked(array, value);
            }
            array.into()
        }
        Json::Obj(values) => {
            let object = vm.bx.heap_mut().new_object();
            vm.bx.heap_mut().set_string_keys(object);
            for (name, value) in values {
                let key = vm.bx.heap_mut().new_string_from_str(name);
                let value = json_to_script(vm, value);
                vm.bx.heap_mut().set_value_def(object, key, value);
            }
            object.into()
        }
    }
}

fn script_to_value(
    vm: &mut ScriptVm<'_>,
    value: ScriptValue,
    ty: PortType,
    media: &HashMap<(String, String), Value>,
) -> Result<Value, String> {
    match ty {
        PortType::Text => value_string(vm, value)
            .map(Value::text)
            .ok_or_else(|| "expected a string".to_string()),
        PortType::Json | PortType::List => {
            if ty == PortType::List && value.as_array().is_none() {
                return Err("expected an array".to_string());
            }
            let json = vm.bx.heap_mut().to_json(value);
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
            let handle_ty = deep_value(vm, object, "type")
                .and_then(|value| value_string(vm, value))
                .ok_or_else(|| "opaque handle has no type".to_string())?;
            if handle_ty != ty.as_str() {
                return Err(format!("opaque handle has type {handle_ty}, expected {}", ty.as_str()));
            }
            let existing = media
                .get(&(digest, ty.as_str().to_string()))
                .ok_or_else(|| format!("opaque handle is not an input of type {}", ty.as_str()))?;
            Ok(existing.clone())
        }
    }
}

/// Evaluate the shipped prelude and expose its palette metadata. Field and
/// object descriptions come through splash's `construction_chain` docs API,
/// so the HTTP catalog cannot drift from the `/** */` text authors edit.
pub fn prelude_catalog() -> Result<Vec<NodeTypeCatalog>, EvalError> {
    catalog_with_registry(&Registry::with_builtins())
}

pub fn catalog_with_registry(registry: &Registry) -> Result<Vec<NodeTypeCatalog>, EvalError> {
    let builtin = builtin_catalog(registry.types())?;
    Ok(registry.descriptors().map(|descriptor| {
        let old = builtin.iter().find(|catalog| catalog.type_name == descriptor.type_name);
        NodeTypeCatalog {
            type_name: descriptor.type_name.clone(), kind: descriptor.kind.clone(),
            domain: descriptor.domain.clone(), models: Vec::new(),
            ports: NodePortsCatalog {
                _in: descriptor.all_inputs().into_iter().map(|input| Port { name: input.name, ty: input.ty, many: input.many }).collect(),
                out: descriptor.outputs.clone(),
            },
            params: descriptor.params.iter().map(|param| {
                let metadata = old.and_then(|catalog| catalog.params.iter().find(|entry| entry.name == param.name));
                // A provider's parameter carries its own description and
                // range; a builtin's come from the prelude.
                NodeParamCatalog { name: param.name.clone(), default: literal_to_json_value(&param.default),
                    doc: metadata.map(|entry| entry.doc.clone()).or_else(|| param.doc.clone()).unwrap_or_default(),
                    range: metadata.and_then(|entry| entry.range.clone())
                        .or_else(|| param.range.map(|(min, max, step)| ParamRange { min, max, step })) }
            }).collect(),
            face: descriptor.face.clone(), doc: descriptor.doc.clone(),
        }
    }).collect())
}

fn builtin_catalog(_types: &TypeRegistry) -> Result<Vec<NodeTypeCatalog>, EvalError> {
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.capture_errors();
    vm.new_module(id!(flow));
    vm.eval(make_mod(PRELUDE_FILE, crate::PRELUDE));
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(error_from_vm(&errors[0], PRELUDE_FILE));
    }
    vm.capture_errors();
    vm.eval(make_mod(RECIPE_PRELUDE_FILE, RECIPE_PRELUDE));
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(error_from_vm(&errors[0], RECIPE_PRELUDE_FILE));
    }
    let module = own_value(&vm, vm.modules(), "flow")
        .and_then(|value| value.as_object())
        .ok_or_else(|| at(PRELUDE_FILE, 1, 1, "prelude module is missing"))?;
    let ui_module = own_value(&vm, module, "ui").and_then(|value| value.as_object());

    // Walk every key `mod.flow` itself sets rather than a hard-coded name
    // list, so every recipe type built purely in splash (Mesh, Video,
    // Music, Inpaint, ...) reaches the catalog alongside the language's
    // built-in kinds.
    let mut raw = Vec::new();
    vm.bx.heap_ref().object_map_iter_ordered(module, |key, value| {
        if let Some(name) = key_name(&vm, key) {
            raw.push((name, value));
        }
    });
    for entry in vm.bx.heap_ref().vec_ref(module) {
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
            "input" | "output" | "publish" | "chat" | "fn" | "http" | "ask" | "gen" | "map" | "choose"
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
        let doc = chain.iter().find_map(|level| level.doc().map(str::to_owned)).unwrap_or_default();
        let field_doc = |name: &str| {
            let wanted = LiveId::from_str(name);
            chain
                .iter()
                .find_map(|level| {
                    level.field_doc(wanted).map(str::to_owned)
                })
                .unwrap_or_default()
        };

        let spec = builtin_spec(&type_name);
        let mut discard = Vec::new();
        let loc = Loc::default();
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
            .map(|(name, ty)| Port { name, ty, many: None })
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
            .map(|(name, ty)| Port { name, ty, many: None })
            .collect();
            (ports_in, ports_out)
        } else if let Some(spec) = &spec {
            let ports_in = spec
                .inputs
                .iter()
                .map(|port| Port {
                    name: port.name.to_string(),
                    ty: port.ty.clone(),
                    many: None,
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
                    let range = match (hint.min(), hint.max()) {
                        (Some(min), Some(max)) => Some(ParamRange {
                            min,
                            max,
                            step: hint.step(),
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
                    let range = match (hint.min(), hint.max()) {
                        (Some(min), Some(max)) => Some(ParamRange {
                            min,
                            max,
                            step: hint.step(),
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
    vm.bx.heap_ref().object_map_iter_ordered(ui_module, |key, value| {
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
            many: None,
        }],
        "Http" => vec![
            Port { name: "value".to_string(), ty: PortType::Text, many: None },
            Port { name: "meta".to_string(), ty: PortType::Json, many: None },
        ],
        "Publish" => vec![Port {
            name: "asset".to_string(),
            ty: PortType::Json,
            many: None,
        }],
        "Image" | "Upscale" | "Choose" => vec![Port {
            name: "image".to_string(),
            ty: PortType::Image,
            many: None,
        }],
        "Map" => vec![Port {
            name: "items".to_string(),
            ty: PortType::Image,
            many: Some(8),
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

/// Extract a `Flow{}` (or, for a pinned module, a `Component{}`) into a
/// graph. `declared` are the pinned components a node may instantiate.
fn extract(
    vm: &ScriptVm<'_>,
    flow_value: ScriptValue,
    source: &str,
    file_name: &str,
    declared: &[Declared],
    expect: &str,
    registry: &Registry,
) -> Result<Graph, EvalError> {
    let flow_obj = flow_value
        .as_object()
        .ok_or_else(|| at(file_name, 1, 1, format!("last expression is not a {expect}{{}}")))?;
    let prototypes = PreludePrototypes { registry };
    if prototypes.type_of(vm, flow_obj).as_deref() != Some(expect) {
        let loc = object_loc(vm, flow_obj, file_name);
        return Err(at_loc(
            file_name,
            &loc,
            format!("last expression is not a {expect}{{}}"),
        ));
    }
    let flow_span = object_span(vm, flow_obj, source, file_name);
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
    vm.bx.heap_ref().object_map_iter_ordered(flow_obj, |key, value| {
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
    for entry in vm.bx.heap_ref().vec_ref(flow_obj) {
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
            at_loc(
                file_name,
                &flow_loc,
                format!("flow field `{id}` is not a node"),
            )
        })?;
        if object_ids.insert(obj, id.clone()).is_some() {
            let loc = object_loc(vm, obj, file_name);
            return Err(at_loc(
                file_name,
                &loc,
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
            at_loc(
                file_name,
                &loc,
                format!("`{id}` has unregistered node type `{}`", deep_value(vm, obj, "type_name").and_then(|value| value_string(vm, value)).unwrap_or_default()),
            )
        })?;
        if type_name == "Flow" {
            let loc = object_loc(vm, obj, file_name);
            return Err(at_loc(
                file_name,
                &loc,
                format!("flow field `{id}` is not a node"),
            ));
        }
        if type_name == "Component" {
            // A container: an instance of a pinned component.
            let Some(entry) = component_of(vm, obj, declared) else {
                let loc = object_loc(vm, obj, file_name);
                return Err(at_loc(
                    file_name,
                    &loc,
                    format!("`{id}` is a Component{{}} declaration, not an instance of a pinned component"),
                ));
            };
            let mut node = extract_node(
                vm,
                source,
                file_name,
                id,
                obj,
                "Component",
                &object_ids,
                &mut warnings,
                Some(&entry.def),
                registry,
            )?;
            node.type_name = entry.alias.clone();
            nodes.push(node);
            continue;
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
            None,
            registry,
        )?);
    }

    for port in nodes.iter().flat_map(|node| node.inputs.iter().map(|input| &input.ty).chain(node.outputs.iter().map(|output| &output.ty))) {
        registry.types().resolve_port(port).map_err(|error| at(file_name, 1, 1, error.to_string()))?;
    }
    resolve_flexible_input_types(&mut nodes);
    let edges = edges_of(&nodes);
    validate_acyclic(&nodes, &edges, file_name)?;
    validate_edges(&nodes, &edges, file_name)?;
    validate_maps(&nodes).map_err(|(node, message)| {
        let loc = nodes.iter().find(|candidate| candidate.id == node).map(|node| node.loc.clone()).unwrap_or_default();
        at_loc(file_name, &loc, message)
    })?;
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
        components: Vec::new(),
    })
}

struct PreludePrototypes<'a> { registry: &'a Registry }
impl PreludePrototypes<'_> {
    fn type_of(&self, vm: &ScriptVm<'_>, object: ScriptObject) -> Option<String> {
        let name = deep_value(vm, object, "type_name").and_then(|value| value_string(vm, value))?;
        (matches!(name.as_str(), "Flow" | "Component") || self.registry.descriptor(&name).is_some()).then_some(name)
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_node(
    vm: &ScriptVm<'_>,
    source: &str,
    file_name: &str,
    id: &str,
    obj: ScriptObject,
    type_name: &str,
    object_ids: &HashMap<ScriptObject, String>,
    warnings: &mut Vec<String>,
    component: Option<&ComponentDef>,
    registry: &Registry,
) -> Result<Node, EvalError> {
    let mut component_spec = TypeDescriptor::provider("Component", "", Vec::new(), Vec::new());
    component_spec.kind = "component".into();
    let spec = if component.is_some() { &component_spec } else {
        registry.descriptor(type_name).ok_or_else(|| at(file_name, 1, 1, format!("node type `{type_name}` is not registered")))?
    };
    let loc = object_loc(vm, obj, file_name);
    let span = object_span(vm, obj, source, file_name);
    let fields = span.map(|span| field_sources(source, span)).unwrap_or_default();
    let context = format!("node `{id}`");
    // One docs resolution per node: the chain resolves the docs of every
    // body along it, the slow part of a load, and everything below reads
    // from this copy.
    let docs = vm.construction_chain(obj.into());
    if spec.owner != BUILTIN_OWNER && component.is_none() {
        for name in fields.keys() {
            if !matches!(name.as_str(), "at" | "size" | "flip" | "ui" | "on_fail" | "label")
                && !(name == "in" && spec.has_control_inputs())
                && !spec.params.iter().any(|param| param.name == *name)
                && !spec.inputs.iter().any(|input| input.name == *name)
            {
                return Err(field_error(source, span, name, file_name, format!("{context} has undeclared field `{name}`"), &loc));
            }
        }
        // A provider instance can inherit authored fields or receive them
        // through mutation. Inspect the actual chain, including the installed
        // descriptor and Node bases, whose fields obey this same contract.
        // Module bindings and construction locations are not identities.
        let mut current = Some(obj);
        let mut seen = HashSet::new();
        while let Some(object) = current {
            if !seen.insert(object) {
                return Err(at_loc(file_name, &loc, format!("{context} has a cyclic prototype chain")));
            }
            let heap = vm.bx.heap_ref();
            let mut names = Vec::new();
            heap.object_map_iter_ordered(object, |key, _| names.push(key_name(vm, key)));
            names.extend(heap.vec_ref(object).iter().map(|entry| key_name(vm, entry.key)));
            for name in names {
                let Some(name) = name else {
                    return Err(at_loc(file_name, &loc, format!("{context} has an unnamed field")));
                };
                if !matches!(name.as_str(), "kind" | "type_name" | "at" | "size" | "flip" | "ui" | "on_fail" | "label" | "out" | "port")
                    && !(name == "in" && spec.has_control_inputs())
                    && !spec.params.iter().any(|param| param.name == name)
                    && !spec.inputs.iter().any(|input| input.name == name)
                {
                    return Err(field_error(source, span, &name, file_name, format!("{context} has undeclared field `{name}`"), &loc));
                }
            }
            current = vm.bx.heap_ref().proto(object).as_object();
        }
    }
    let mut params = Vec::new();
    for param in &spec.params {
        let mut field_source = fields
            .get(&param.name)
            .copied()
            .or_else(|| source_field_in_chain(vm, obj, source, file_name, &param.name));
        let value = if spec.kind == "input" && param.name == "value" && field_source.is_none() {
            let legacy_source = fields
                .get("default")
                .copied()
                .or_else(|| source_field_in_chain(vm, obj, source, file_name, "default"));
            if legacy_source.is_some() {
                field_source = legacy_source;
                deep_value(vm, obj, "default").unwrap_or(NIL)
            } else {
                deep_value(vm, obj, &param.name).unwrap_or(NIL)
            }
        } else {
            deep_value(vm, obj, &param.name).unwrap_or(NIL)
        };
        let mut literal = literal_from_value(vm, value)
            .map_err(|message| source_range_error(source, field_source, file_name, message, &loc))?;
        validate_descriptor_param(param, &literal, registry.types())
            .map_err(|message| source_range_error(source, field_source, file_name, message, &loc))?;
        snap_documented_param(&docs, &context, &param.name, &mut literal, warnings);
        params.push((param.name.to_string(), literal));
    }
    if spec.kind == "input" {
        if let Some(bound) = param_fan_in(&params, "many") {
            // An ordered Input's design value is a list of its element
            // literals. Nothing authored is the typed empty list, never the
            // inherited one-value default; something authored that is not a
            // list is refused where it is written.
            let authored = fields
                .get("value")
                .copied()
                .or_else(|| source_field_in_chain(vm, obj, source, file_name, "value"))
                .or_else(|| fields.get("default").copied())
                .or_else(|| source_field_in_chain(vm, obj, source, file_name, "default"));
            let ty = param_port_type(&params, "type").unwrap_or(PortType::Text);
            let slot = params
                .iter_mut()
                .find(|(name, _)| name == "value")
                .expect("an Input has a value param");
            match (&slot.1, authored) {
                (Literal::Arr(items), _) => {
                    if items.len() > bound as usize {
                        return Err(source_range_error(
                            source,
                            authored,
                            file_name,
                            format!("an ordered Input's value lists {} literals, over its bound of {bound}", items.len()),
                            &loc,
                        ));
                    }
                    for item in items {
                        Value::from_literal(ty.clone(), item).map_err(|message| {
                            source_range_error(
                                source,
                                authored,
                                file_name,
                                format!("an ordered Input's value lists {} literals: {message}", port_type_name(&ty)),
                                &loc,
                            )
                        })?;
                    }
                }
                (_, None) => slot.1 = Literal::Arr(Vec::new()),
                (_, Some(range)) => {
                    return Err(source_range_error(
                        source,
                        Some(range),
                        file_name,
                        format!("an ordered Input's value is a list of {} literals (`[]` for none)", port_type_name(&ty)),
                        &loc,
                    ))
                }
            }
        }
    }
    let gen_inputs = if spec.kind == "gen" {
        Some(ports_side_typed(
            vm, obj, "in", source, span, file_name, &loc, &context, warnings,
        )?)
    } else {
        None
    };
    if spec.codec == NodeCodec::Gen && spec.write.contains(&WriteField::Params) {
        let input_names: HashSet<String> = gen_inputs
            .as_ref()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        params = generic_gen_params(vm, obj, source, span, file_name, &loc, &input_names)?;
        for (name, literal) in &mut params {
            snap_documented_param(&docs, &context, name, literal, warnings);
        }
    }

    let mut inputs = Vec::new();
    if let Some(def) = component {
        // A container's inputs are its component's input ports, wired or
        // given literally in `in:`, exactly as `Fn.in`; a port the
        // component does not declare is refused where it is written.
        let in_value = deep_value(vm, obj, "in").unwrap_or(NIL);
        let mut pairs = Vec::new();
        if let Some(in_obj) = in_value.as_object() {
            vm.bx.heap_ref().object_map_iter_ordered(in_obj, |key, value| {
                if let Some(name) = key_name(vm, key) {
                    pairs.push((name, value));
                }
            });
        } else if !in_value.is_nil() {
            return Err(field_error(source, span, "in", file_name, "a component's `in` must be an object", &loc));
        }
        for (name, value) in pairs {
            let port = def.inputs.iter().find(|port| port.name == name).ok_or_else(|| {
                field_error(
                    source,
                    span,
                    "in",
                    file_name,
                    format!("component `{}` has no input port `{name}`", def.alias),
                    &loc,
                )
            })?;
            inputs.push(extract_input(
                vm,
                source,
                file_name,
                span,
                &loc,
                id,
                &name,
                port.ty.clone(),
                false,
                false,
                port.many,
                value,
                object_ids,
            )?);
        }
    } else if spec.codec == NodeCodec::Fn {
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
        vm.bx.heap_ref().object_map_iter_ordered(in_obj, |key, value| {
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
                true,
                None,
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
                .find_map(|input| (input.name == name).then_some(input.default.clone()))
                .unwrap_or(Literal::Null);
            let value = input_value(vm, obj, &name, &default);
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
                false,
                None,
                value,
                object_ids,
            )?);
        }
    } else {
        for input in &spec.inputs {
            let mut declared_ty = input.ty.clone();
            let mut declared_many = input.many;
            if spec.codec == NodeCodec::Output {
                declared_ty = param_port_type(&params, "type").unwrap();
                declared_many = param_fan_in(&params, "many");
            }
            // A Map's body and a Choose's candidates carry the node's
            // `type`; the candidates are an ordered port of `many` values.
            if (spec.codec == NodeCodec::Map && input.name == "body")
                || (spec.codec == NodeCodec::Choose && input.name == "from")
            {
                declared_ty = param_port_type(&params, "type").unwrap();
            }
            if spec.codec == NodeCodec::Choose && input.name == "from" {
                declared_many = Some(param_fan_in(&params, "many").ok_or_else(|| {
                    field_error(source, span, "many", file_name, format!("{context} needs `many`, the most candidates it takes"), &loc)
                })?);
            }
            let value = input_value(vm, obj, &input.name, &input.default);
            inputs.push(extract_input(
                vm,
                source,
                file_name,
                span,
                &loc,
                id,
                &input.name,
                declared_ty,
                input.flexible,
                false,
                declared_many,
                value,
                object_ids,
            )?);
        }
        if spec.has_control_inputs() {
            // Control inputs are wired in `in: { gain: lfo.level }`; the
            // same-named field (`gain: 0.3`) stays the authored fallback.
            let in_value = deep_value(vm, obj, "in").unwrap_or(NIL);
            let mut pairs = HashMap::new();
            if in_value.as_object().is_none() && !in_value.is_nil() {
                return Err(field_error(source, span, "in", file_name, "`in` must be an object of control wires", &loc));
            }
            let mut current = in_value.as_object();
            let mut seen = HashSet::new();
            while let Some(in_obj) = current {
                // Match the VM's bounded prototype lookup. Nearest fields
                // win, including an explicit nil that disconnects a wire.
                if !seen.insert(in_obj) || seen.len() > 24 {
                    return Err(field_error(source, span, "in", file_name, "control input map has a cyclic or overlong prototype chain", &loc));
                }
                let heap = vm.bx.heap_ref();
                let mut fields = Vec::new();
                heap.object_map_iter_ordered(in_obj, |key, value| fields.push((key_name(vm, key), value)));
                fields.extend(heap.vec_ref(in_obj).iter().map(|entry| (key_name(vm, entry.key), entry.value)));
                for (name, value) in fields {
                    let Some(name) = name.filter(|name| spec.control_input(name).is_some()) else {
                        return Err(field_error(source, span, "in", file_name, format!("{context} has an undeclared or unnamed control input"), &loc));
                    };
                    pairs.entry(name).or_insert(value);
                }
                current = vm.bx.heap_ref().proto(in_obj).as_object();
            }
            for control in spec.control_inputs() {
                let value = pairs.get(&control.name).copied().unwrap_or(NIL);
                let input = extract_input(
                    vm, source, file_name, span, &loc, id, &control.name, control.ty.clone(),
                    false, false, None, value, object_ids,
                )?;
                if matches!(&input.value, NodeInputValue::Literal(literal) if *literal != Literal::Null) {
                    return Err(field_error(source, span, "in", file_name, format!(
                        "control input `{}` takes a wire; set `{}: value` for its fallback", control.name, control.name
                    ), &loc));
                }
                inputs.push(input);
            }
        }
    }

    let outputs = match component {
        Some(def) => def.outputs.clone(),
        None => outputs_for(
            vm, obj, spec, &params, source, span, file_name, &loc, &context, warnings,
        )?,
    };
    let at_source = fields.get("at").copied().or_else(|| {
        source_field_in_chain(vm, obj, source, file_name, "at")
    });
    let at_value = at_source.and_then(|range| {
        let range = layout_value_range(source, range);
        parse_vec2(&source[range.0..range.1])
    });
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
        let range = layout_value_range(source, range);
        let value = &source[range.0..range.1];
        (value != "nil").then(|| parse_vec2(value)).flatten()
    });
    if size_source.is_some()
        && size_value.is_none()
        && size_source.is_some_and(|range| {
            let range = layout_value_range(source, range);
            &source[range.0..range.1] != "nil"
        })
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
    let fn_src = if spec.codec == NodeCodec::Fn {
        let run = deep_value(vm, obj, "run").unwrap_or(NIL);
        let is_fn = run
            .as_object()
            .map(|obj| vm.bx.heap_ref().is_fn(obj))
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
    // A container's label is the instance's own; the component's display
    // label stays the component's and is not written back as the node's.
    let label_value = if component.is_some() {
        own_value(vm, obj, "label")
    } else {
        deep_value(vm, obj, "label")
    };
    let label = match label_value.unwrap_or(NIL) {
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
    let domain = if matches!(spec.kind.as_str(), "gen" | "chat") {
        Some(expect_string_field(vm, obj, "domain", source, span, file_name)?)
    } else {
        None
    };
    let doc = docs.first().and_then(|level| level.doc().map(str::to_owned));
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

#[allow(clippy::too_many_arguments)]
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
    self_declared: bool,
    many: Option<u32>,
    value: ScriptValue,
    object_ids: &HashMap<ScriptObject, String>,
) -> Result<NodeInput, EvalError> {
    let error = |message: String| field_error(source, span, name, file_name, message, loc);
    // An Fn declares an ordered input in place: the element type and the
    // bound travel with the entries, so the declaration outlives them.
    // Only an Fn declares its own inputs; a flexible built-in port (a
    // Publish value, an Http body) takes one value of any type, which is
    // not a cardinality.
    if let Some(decl) = ordered_decl(vm, value).map_err(error)? {
        if !self_declared {
            return Err(error(
                "`ordered(...)` declares an Fn input; this port's shape is declared by its node".to_string(),
            ));
        }
        let entries = entry_list(vm, decl.entries, object_ids, source, span, node_id, name, file_name, loc)?;
        return Ok(NodeInput {
            port: name.to_string(),
            ty: decl.ty,
            many: Some(decl.many),
            value: NodeInputValue::Edges(entries),
        });
    }
    if let Some(array) = value.as_array() {
        let len = vm.bx.heap_ref().array_len(array);
        let refs = (0..len)
            .filter(|&index| looks_like_port_ref(vm, vm.bx.heap_ref().array_index_unchecked(array, index)))
            .count();
        if refs > 0 && refs < len {
            return Err(error(
                "mixes port references and literal values; an ordered input lists entries `[conn(id, source.port()), ...]` only"
                    .to_string(),
            ));
        }
        if refs > 0 {
            let Some(bound) = many else {
                return Err(error(if self_declared {
                    "an ordered Fn input is declared with `ordered(@type, bound, [conn(id, source.port()), ...])`"
                        .to_string()
                } else {
                    "takes one value, not a list of connections".to_string()
                }));
            };
            let entries = entry_list(vm, value, object_ids, source, span, node_id, name, file_name, loc)?;
            return Ok(NodeInput {
                port: name.to_string(),
                ty: declared_ty,
                many: Some(bound),
                value: NodeInputValue::Edges(entries),
            });
        }
        if let Some(bound) = many {
            if len == 0 {
                return Ok(NodeInput {
                    port: name.to_string(),
                    ty: declared_ty,
                    many: Some(bound),
                    value: NodeInputValue::Edges(Vec::new()),
                });
            }
            return Err(error(
                "an ordered port takes entries `[conn(id, source.port()), ...]`, not literal values".to_string(),
            ));
        }
        // A list of literals stays a `list` literal, whatever the port.
    }
    if let Some(edge) = port_ref(
        vm, value, object_ids, source, span, node_id, name, file_name, loc,
    )? {
        let conn = port_ref_conn(vm, value).map_err(error)?;
        return Ok(match many {
            // One reference into an ordered port: its one entry (id 1
            // unless named); the writer materialises the array form.
            Some(bound) => NodeInput {
                port: name.to_string(),
                ty: declared_ty,
                many: Some(bound),
                value: NodeInputValue::Edges(vec![ConnectionEntry { id: conn.unwrap_or(1), from: edge }]),
            },
            None => {
                if conn.is_some() {
                    return Err(error(
                        "takes one value; `conn(id, ...)` names an entry of an ordered input".to_string(),
                    ));
                }
                NodeInput {
                    port: name.to_string(),
                    ty: declared_ty,
                    many: None,
                    value: NodeInputValue::Edge(edge),
                }
            }
        });
    }
    if value
        .as_object()
        .map(|obj| vm.bx.heap_ref().is_fn(obj))
        .unwrap_or(false)
    {
        return Err(error("closure where a literal or port reference was expected".to_string()));
    }
    if let Some(referenced) = value.as_object().and_then(|object| object_ids.get(&object)) {
        return Err(error(format!("expected a port reference such as `{referenced}.text()`")));
    }
    let literal = literal_from_value(vm, value).map_err(error)?;
    if let Some(bound) = many {
        if matches!(literal, Literal::Null) {
            // Unset: an ordered port with no entries.
            return Ok(NodeInput {
                port: name.to_string(),
                ty: declared_ty,
                many: Some(bound),
                value: NodeInputValue::Edges(Vec::new()),
            });
        }
        return Err(error(
            "an ordered port takes entries `[conn(id, source.port()), ...]`; a literal cannot be one".to_string(),
        ));
    }
    let ty = if flexible {
        literal_type(&literal).unwrap_or(declared_ty)
    } else {
        declared_ty
    };
    Ok(NodeInput {
        port: name.to_string(),
        ty,
        many: None,
        value: NodeInputValue::Literal(literal),
    })
}

/// What `ordered(@type, bound, [entries])` evaluated to.
struct OrderedDecl {
    ty: PortType,
    many: u32,
    entries: ScriptValue,
}

/// The declaration object the prelude's `ordered(...)` returns, if `value`
/// is one; an error when it is one with a bad type or bound.
fn ordered_decl(vm: &ScriptVm<'_>, value: ScriptValue) -> Result<Option<OrderedDecl>, String> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if vm.bx.heap_ref().is_fn(obj) {
        return Ok(None);
    }
    let is_decl = own_value(vm, obj, "ordered").and_then(|flag| flag.as_bool()) == Some(true);
    if !is_decl {
        return Ok(None);
    }
    let ty = own_value(vm, obj, "type")
        .and_then(|value| key_name(vm, value))
        .and_then(|id| port_type(&id))
        .ok_or_else(|| "`ordered(@type, bound, entries)`: the type must be a port type such as @text".to_string())?;
    let many = own_value(vm, obj, "many")
        .and_then(|value| value.as_number())
        .filter(|bound| bound.is_finite() && bound.fract() == 0.0 && *bound >= 1.0 && *bound <= MAX_SEQUENCE as f64)
        .ok_or_else(|| format!("`ordered(@type, bound, entries)`: the bound must be an integer from 1 to {MAX_SEQUENCE}"))?;
    let entries = own_value(vm, obj, "entries")
        .filter(|entries| entries.as_array().is_some())
        .ok_or_else(|| "`ordered(@type, bound, entries)`: the entries must be an array".to_string())?;
    Ok(Some(OrderedDecl { ty, many: many as u32, entries }))
}

/// The `conn` of a port reference made with `conn(id, source.port())`,
/// checked to be a whole number in `1..=MAX_CONNECTION_ID`.
fn port_ref_conn(vm: &ScriptVm<'_>, value: ScriptValue) -> Result<Option<u64>, String> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    let Some(conn) = own_value(vm, obj, "conn") else {
        return Ok(None);
    };
    if conn.is_nil() {
        return Ok(None);
    }
    let number = conn.as_number().ok_or_else(|| "conn id must be a number".to_string())?;
    if !number.is_finite() || number.fract() != 0.0 || number < 1.0 || number > MAX_CONNECTION_ID as f64 {
        return Err(format!("conn id must be an integer from 1 to {MAX_CONNECTION_ID}"));
    }
    Ok(Some(number as u64))
}

/// The entries of an ordered input from an array of `conn(...)` references:
/// every element a named reference, ids unique, at most `MAX_FAN_IN` of
/// them. Errors point at the entry that is wrong.
#[allow(clippy::too_many_arguments)]
fn entry_list(
    vm: &ScriptVm<'_>,
    value: ScriptValue,
    object_ids: &HashMap<ScriptObject, String>,
    source: &str,
    span: Option<(usize, usize)>,
    node_id: &str,
    name: &str,
    file_name: &str,
    loc: &Loc,
) -> Result<Vec<ConnectionEntry>, EvalError> {
    let array = value
        .as_array()
        .ok_or_else(|| field_error(source, span, name, file_name, "entries must be an array", loc))?;
    let len = vm.bx.heap_ref().array_len(array);
    if len > MAX_FAN_IN {
        return Err(field_error(
            source,
            span,
            name,
            file_name,
            format!("lists {len} entries, over the {MAX_FAN_IN} entry limit"),
            loc,
        ));
    }
    let mut entries: Vec<ConnectionEntry> = Vec::with_capacity(len);
    for index in 0..len {
        let element = vm.bx.heap_ref().array_index_unchecked(array, index);
        let at = |message: String| entry_error(source, span, name, index, file_name, message, loc);
        let edge = port_ref(vm, element, object_ids, source, span, node_id, name, file_name, loc)?
            .ok_or_else(|| at("is not a port reference; an entry is `conn(id, source.port())`".to_string()))?;
        let id = port_ref_conn(vm, element)
            .map_err(at)?
            .ok_or_else(|| at("needs an id: `conn(id, source.port())`".to_string()))?;
        if let Some(position) = entries.iter().position(|entry| entry.id == id) {
            return Err(at(format!("id {id} is already entry {}'s", position + 1)));
        }
        entries.push(ConnectionEntry { id, from: edge });
    }
    Ok(entries)
}

/// An error at entry `index` of the array written for `field` (inside the
/// node's `in:` object, or the field itself), falling back to the field.
#[allow(clippy::too_many_arguments)]
fn entry_error(
    source: &str,
    span: Option<(usize, usize)>,
    field: &str,
    index: usize,
    file_name: &str,
    message: String,
    loc: &Loc,
) -> EvalError {
    let message = format!("`{field}` entry {}: {message}", index + 1);
    match entry_range(source, span, field, index) {
        Some(range) => source_range_error(source, Some(range), file_name, message, loc),
        None => field_error(source, span, field, file_name, message, loc),
    }
}

/// Where entry `index` of `field`'s array is written.
fn entry_range(source: &str, span: Option<(usize, usize)>, field: &str, index: usize) -> Option<(usize, usize)> {
    let fields = field_sources(source, span?);
    let nested = fields
        .get("in")
        .copied()
        .filter(|range| source.as_bytes().get(range.0) == Some(&b'{'))
        .and_then(|range| field_sources(source, range).get(field).copied());
    let range = nested.or_else(|| fields.get(field).copied())?;
    let open = first_open_bracket(source, range)?;
    let close = matching_close(source, open)?;
    array_element_sources(source, (open, close + 1)).get(index).copied()
}

/// The first `[` inside `range` that is code, not a string or a comment.
fn first_open_bracket(source: &str, range: (usize, usize)) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = range.0;
    while index < range.1 {
        match bytes[index] {
            b'"' => index = skip_string(bytes, index)?,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => index = skip_block_comment(bytes, index)?,
            b'[' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

/// The index of the bracket closing the one opened at `open`, skipping
/// strings and comments.
fn matching_close(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = skip_string(bytes, index)?;
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index)?;
                continue;
            }
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// The byte range of every top-level element of the array literal at
/// `range` (`[` to past `]`), elements separated by commas or whitespace.
fn array_element_sources(source: &str, range: (usize, usize)) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let end = range.1.saturating_sub(1);
    let mut index = range.0 + 1;
    while index < end {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || byte == b',' {
            index += 1;
            continue;
        }
        if byte == b'/' && matches!(bytes.get(index + 1), Some(b'/') | Some(b'*')) {
            index = if bytes[index + 1] == b'/' {
                skip_line_comment(bytes, index)
            } else {
                skip_block_comment(bytes, index).unwrap_or(end)
            };
            continue;
        }
        let start = index;
        let mut depth = 0usize;
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
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => depth = depth.saturating_sub(1),
                byte if depth == 0 && (byte.is_ascii_whitespace() || byte == b',') => break,
                _ => {}
            }
            index += 1;
        }
        out.push((start, index));
    }
    out
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
    for (object, made_in) in prototype_chain(vm, obj).iter().rev() {
        if made_in.as_ref().is_some_and(|location| location.file() == PRELUDE_FILE) {
            continue;
        }
        let mut fields = Vec::new();
        vm.bx.heap_ref().object_map_iter_ordered(*object, |key, value| {
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
                .is_some_and(|object| vm.bx.heap_ref().is_fn(object))
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

fn field_doc(chain: &[ScriptChainLevel], name: &str) -> Option<String> {
    let wanted = LiveId::from_str(name);
    chain.iter().find_map(|level| {
        level.field_doc(wanted).map(str::to_owned)
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
    chain: &[ScriptChainLevel],
    context: &str,
    name: &str,
    literal: &mut Literal,
    warnings: &mut Vec<String>,
) {
    let Literal::Num(value) = literal else {
        return;
    };
    let Some(step) = field_doc(chain, name)
        .map(|doc| parse_doc_hint(&doc))
        .and_then(|hint| hint.step())
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
    if vm.bx.heap_ref().is_fn(obj) {
        return Ok(None);
    }
    let Some(node_value) = own_value(vm, obj, "node") else {
        return Ok(None);
    };
    let Some(port_value) = own_value(vm, obj, "port") else {
        return Ok(None);
    };
    // Named wires resolve against this Flow's completed membership, never
    // the lexical scope or a prototype. This permits later-declared sources
    // without reordering arbitrary construction code. Object references keep
    // their original identity check; both forms share port/type/cycle checks.
    let (from_node, referenced) = if let Some(node_obj) = node_value.as_object() {
        (object_ids.get(&node_obj).cloned(), source_port_ref_node(source, span, field).unwrap_or("unknown").to_string())
    } else if let Some(name) = key_name(vm, node_value) {
        (object_ids.values().find(|id| **id == name).cloned(), name)
    } else {
        return Err(field_error(source, span, field, file_name, "invalid port reference node", loc));
    };
    let from_node = from_node.ok_or_else(|| {
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
    let from_port = key_name(vm, port_value).ok_or_else(|| {
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
        !vm.bx.heap_ref().is_fn(obj)
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
    vm: &ScriptVm<'_>, obj: ScriptObject, spec: &TypeDescriptor,
    params: &[(String, Literal)], source: &str, span: Option<(usize, usize)>,
    file_name: &str, loc: &Loc, context: &str, warnings: &mut Vec<String>,
) -> Result<Vec<Port>, EvalError> {
    Ok(match spec.codec {
        NodeCodec::Gen => ports_side_typed(vm, obj, "out", source, span, file_name, loc, context, warnings)?
            .into_iter().map(|(name, ty)| Port { name, ty, many: None }).collect(),
        NodeCodec::Input | NodeCodec::Ask => {
            let ty = param_port_type(params, "type").ok_or_else(|| field_error(source, span, "type", file_name, "invalid port type", loc))?;
            vec![Port { name: ty.as_str().to_string(), ty,
                many: if spec.codec == NodeCodec::Input { param_fan_in(params, "many") } else { None } }]
        }
        NodeCodec::Output => Vec::new(),
        NodeCodec::Map => {
            let ty = param_port_type(params, "type").ok_or_else(|| field_error(source, span, "type", file_name, "invalid port type", loc))?;
            let many = param_fan_in(params, "many").ok_or_else(|| {
                field_error(source, span, "many", file_name, format!("{context} needs `many`, the most elements it maps"), loc)
            })?;
            vec![Port { name: "items".into(), ty, many: Some(many) }]
        }
        NodeCodec::Choose => {
            let ty = param_port_type(params, "type").ok_or_else(|| field_error(source, span, "type", file_name, "invalid port type", loc))?;
            vec![Port { name: ty.as_str().to_string(), ty, many: None }]
        }
        NodeCodec::Fn => param_id_array(params, "out")
            .ok_or_else(|| field_error(source, span, "out", file_name, "Fn.out must be an array of port ids", loc))?
            .into_iter().map(|name| Port { ty: port_type(&name).unwrap_or(PortType::Json), name, many: None }).collect(),
        NodeCodec::Http => vec![
            Port { name: "value".into(), ty: param_port_type(params, "out").unwrap(), many: None },
            Port { name: "meta".into(), ty: PortType::Json, many: None },
        ],
        NodeCodec::Fields => spec.outputs.clone(),
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
        if !vm.bx.heap_ref().is_fn(map_obj) {
            let mut pairs = Vec::new();
            vm.bx.heap_ref().object_map_iter_ordered(map_obj, |key, val| {
                if let Some(name) = key_name(vm, key) {
                    pairs.push((name, val));
                }
            });
            let mut out = Vec::with_capacity(pairs.len());
            for (name, val) in pairs {
                let ty = key_name(vm, val).and_then(|id| port_type(&id)).ok_or_else(|| {
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

pub(crate) fn validate_descriptor_param(param: &ParamDescriptor, value: &Literal, types: &TypeRegistry) -> Result<(), String> {
    let ok = match param.expected {
        ParamType::String => matches!(value, Literal::Str(_)),
        ParamType::Literal => true,
        ParamType::Number => matches!(value, Literal::Num(n) if n.is_finite()),
        ParamType::NonNegativeInteger => {
            matches!(value, Literal::Num(n) if n.is_finite() && *n >= 0.0 && n.fract() == 0.0)
        }
        ParamType::PortTypeNoBytes => match value {
            Literal::Id(value) | Literal::Str(value) => port_type(value).is_some_and(|ty| ty != PortType::Bytes && types.resolve_port(&ty).is_ok()),
            _ => false,
        },
        ParamType::PortTypeWithBytes => match value {
            Literal::Id(value) | Literal::Str(value) => port_type(value).is_some_and(|ty| types.resolve_port(&ty).is_ok()),
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
        ParamType::FanIn => match value {
            Literal::Null => true,
            Literal::Num(n) => n.is_finite() && n.fract() == 0.0 && *n >= 1.0 && *n <= MAX_SEQUENCE as f64,
            _ => false,
        },
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
                ((node.id.clone(), port.name.clone()), port.ty.clone())
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
                    input.ty = ty.clone();
                }
            }
        }
    }
}

/// Every edge of `nodes`, from their inputs: one per scalar edge, one per
/// entry of an ordered input (carrying the entry's id). The only producer
/// of `Graph.edges`, so the projection can never disagree with the inputs.
pub fn edges_of(nodes: &[Node]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for node in nodes {
        for input in &node.inputs {
            match &input.value {
                NodeInputValue::Edge(edge) => edges.push(Edge {
                    from_node: edge.from_node.clone(),
                    from_port: edge.from_port.clone(),
                    to_node: node.id.clone(),
                    to_port: input.port.clone(),
                    conn: None,
                }),
                NodeInputValue::Edges(entries) => {
                    for entry in entries {
                        edges.push(Edge {
                            from_node: entry.from.from_node.clone(),
                            from_port: entry.from.from_port.clone(),
                            to_node: node.id.clone(),
                            to_port: input.port.clone(),
                            conn: Some(entry.id),
                        });
                    }
                }
                NodeInputValue::Literal(_) => {}
            }
        }
    }
    edges
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
                at_loc(
                    file_name,
                    &to.loc,
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
        // A one input reads one value: an ordered source needs an ordered
        // input, whose entries take the whole sequence.
        if let (Some(bound), None) = (output.many, input.many) {
            return Err(at_loc(
                file_name,
                &to.loc,
                format!(
                    "`{}.{}` takes one value but `{}.{}` carries up to {bound}; read it through an ordered input",
                    edge.to_node, edge.to_port, edge.from_node, edge.from_port
                ),
            ));
        }
        let flexible = input.many.is_none()
            && (to.kind == "fn" || to.kind == "publish" || (to.kind == "http" && input.port == "body"));
        if !flexible && output.ty != input.ty {
            return Err(at_loc(
                file_name,
                &to.loc,
                format!(
                    "type mismatch: {}.{} is {}, but {}.{} expects {}",
                    edge.from_node,
                    edge.from_port,
                    port_type_name(&output.ty),
                    edge.to_node,
                    edge.to_port,
                    port_type_name(&input.ty)
                ),
            ));
        }
    }
    // An ordered input's bound is on the values it resolves to; entries
    // from one ports each contribute exactly one, so more of those than
    // the bound can never fit.
    for node in nodes {
        for input in &node.inputs {
            let (NodeInputValue::Edges(entries), Some(bound)) = (&input.value, input.many) else {
                continue;
            };
            let certain = entries
                .iter()
                .filter(|entry| {
                    node_map
                        .get(entry.from.from_node.as_str())
                        .and_then(|from| from.outputs.iter().find(|port| port.name == entry.from.from_port))
                        .is_some_and(|port| port.many.is_none())
                })
                .count();
            if certain > bound as usize {
                return Err(at_loc(
                    file_name,
                    &node.loc,
                    format!(
                        "`{}.{}` lists {certain} single-value entries, over its bound of {bound}",
                        node.id, input.port
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Whether the graph the writer's source evaluates to (`evaluated`) wires
/// what the client's graph (`sent`) wires: the same nodes, every input's
/// cardinality and wire or entries (ids and order included), the same
/// edges. Literals, types of unwired flexible inputs and layout are the
/// writer's and the evaluator's to normalise; wiring is not. A difference
/// means the writer could not say what the client sent, so it is refused
/// rather than saved as something else.
pub fn same_wiring(sent: &Graph, evaluated: &Graph) -> Result<(), String> {
    if sent.nodes.len() != evaluated.nodes.len() {
        return Err(format!("{} nodes were sent, {} survive a write and reload", sent.nodes.len(), evaluated.nodes.len()));
    }
    for node in &sent.nodes {
        let back = evaluated
            .nodes
            .iter()
            .find(|candidate| candidate.id == node.id)
            .ok_or_else(|| format!("`{}` does not survive a write and reload", node.id))?;
        if back.kind != node.kind || back.type_name != node.type_name {
            return Err(format!("`{}` reloads as a {}, not a {}", node.id, back.type_name, node.type_name));
        }
        for input in &node.inputs {
            let wired = !matches!(input.value, NodeInputValue::Literal(_));
            // A one input's type may be re-inferred from its wire; an
            // ordered input's element type is declared and must survive.
            let same = back.inputs.iter().find(|candidate| candidate.port == input.port).is_some_and(|candidate| {
                candidate.many == input.many
                    && match (&candidate.value, &input.value) {
                        (NodeInputValue::Literal(_), NodeInputValue::Literal(_)) => true,
                        (left, right) => left == right && (input.many.is_none() || candidate.ty == input.ty),
                    }
            });
            if !same && (wired || input.many.is_some()) {
                return Err(format!("`{}.{}` does not survive a write and reload as sent", node.id, input.port));
            }
        }
        // A one output's name and type follow params the evaluator owns
        // and may be normalised; an ordered output is declared, so its
        // name, element type and bound must all survive.
        let ordered = |ports: &[Port]| {
            ports
                .iter()
                .filter(|port| port.many.is_some())
                .map(|port| (port.name.clone(), port.ty.clone(), port.many))
                .collect::<Vec<_>>()
        };
        if ordered(&node.outputs) != ordered(&back.outputs) {
            return Err(format!("`{}`: its ordered output ports do not survive a write and reload as sent", node.id));
        }
    }
    let key = |edge: &Edge| (edge.to_node.clone(), edge.to_port.clone(), edge.conn, edge.from_node.clone(), edge.from_port.clone());
    let mut left = sent.edges.clone();
    let mut right = evaluated.edges.clone();
    left.sort_by_key(key);
    right.sort_by_key(key);
    if left != right {
        return Err("the edges do not survive a write and reload as sent".to_string());
    }
    Ok(())
}

/// Whether a graph a client sent agrees with itself: node and port names
/// unique and every reference resolvable, its `edges` the projection of
/// its inputs, every ordered input carrying entries with valid unique ids
/// within the limits, every port's shape the one its node or the pinned
/// component (`components`, the server's own, never the client's) declares,
/// and the edges type-checked. A refusal names what is wrong rather than
/// letting the writer drop it; nothing here indexes a name it has not
/// checked.
pub fn validate_projection(graph: &Graph, components: &[ComponentDef]) -> Result<(), String> {
    validate_projection_with_registry(graph, components, &Registry::with_builtins())
}

pub fn validate_projection_with_registry(graph: &Graph, components: &[ComponentDef], registry: &Registry) -> Result<(), String> {
    let mut ids: HashSet<&str> = HashSet::new();
    for node in &graph.nodes {
        if node.kind != "component" {
            let descriptor = registry.descriptor(&node.type_name)
                .ok_or_else(|| format!("node type `{}` is not registered", node.type_name))?;
            if descriptor.kind != node.kind {
                return Err(format!("`{}`: kind `{}` does not match descriptor `{}`", node.id, node.kind, node.type_name));
            }
            if descriptor.codec == NodeCodec::Fields {
                // Control inputs may be absent (a graph built before its
                // descriptor gained signals): absent is unwired.
                let declared_inputs = descriptor.all_inputs();
                if node.outputs != descriptor.outputs
                    || descriptor.inputs.iter().any(|port| !node.inputs.iter().any(|input| input.port == port.name))
                {
                    return Err(format!("`{}`: ports do not match descriptor `{}`", node.id, node.type_name));
                }
                for input in &node.inputs {
                    let declared = declared_inputs.iter().find(|port| port.name == input.port)
                        .ok_or_else(|| format!("`{}.{}` is not a declared input", node.id, input.port))?;
                    if descriptor.control_input(&input.port).is_some()
                        && matches!(&input.value, NodeInputValue::Literal(literal) if *literal != Literal::Null)
                    {
                        return Err(format!("`{}.{}` is a control input: a wire or nothing; its value is the parameter", node.id, input.port));
                    }
                    if input.many != declared.many || (!declared.flexible && input.ty != declared.ty) {
                        return Err(format!("`{}.{}` does not match its descriptor's shape", node.id, input.port));
                    }
                }
            }
            if descriptor.codec != NodeCodec::Gen {
                let mut names = HashSet::new();
                for (name, value) in &node.params {
                    let param = descriptor.params.iter().find(|param| param.name == *name)
                        .ok_or_else(|| format!("`{}.{name}` is not a declared parameter", node.id))?;
                    if !names.insert(name) { return Err(format!("`{}.{name}` occurs twice", node.id)); }
                    validate_descriptor_param(param, value, registry.types())?;
                }
            }
        }
        for (ty, bound) in node.inputs.iter().map(|input| (&input.ty, input.many))
            .chain(node.outputs.iter().map(|output| (&output.ty, output.many)))
        {
            registry.types().resolve_port(ty).map_err(|error| error.to_string())?;
            if bound.is_some_and(|bound| !(1..=MAX_SEQUENCE).contains(&bound)) {
                return Err(format!("`{}` has an invalid ordered port bound", node.id));
            }
        }
        if node.id.is_empty() || !ids.insert(node.id.as_str()) {
            return Err(format!("node id `{}` is empty or used twice", node.id));
        }
        let mut ports: HashSet<&str> = HashSet::new();
        for input in &node.inputs {
            if input.port.is_empty() || !ports.insert(input.port.as_str()) {
                return Err(format!("`{}` declares input `{}` twice", node.id, input.port));
            }
        }
        let mut outputs: HashSet<&str> = HashSet::new();
        for output in &node.outputs {
            if output.name.is_empty() || !outputs.insert(output.name.as_str()) {
                return Err(format!("`{}` declares output `{}` twice", node.id, output.name));
            }
        }
    }
    let has_output = |from: &EdgeRef| {
        graph
            .nodes
            .iter()
            .find(|node| node.id == from.from_node)
            .is_some_and(|node| node.outputs.iter().any(|port| port.name == from.from_port))
    };
    for node in &graph.nodes {
        for input in &node.inputs {
            let sources: Vec<&EdgeRef> = match &input.value {
                NodeInputValue::Edge(edge) => vec![edge],
                NodeInputValue::Edges(entries) => entries.iter().map(|entry| &entry.from).collect(),
                NodeInputValue::Literal(_) => Vec::new(),
            };
            for from in sources {
                if !has_output(from) {
                    return Err(format!(
                        "`{}.{}` reads `{}.{}`, which does not exist",
                        node.id, input.port, from.from_node, from.from_port
                    ));
                }
            }
        }
    }
    let mut expected = edges_of(&graph.nodes);
    let mut given = graph.edges.clone();
    let key = |edge: &Edge| (edge.to_node.clone(), edge.to_port.clone(), edge.conn, edge.from_node.clone(), edge.from_port.clone());
    expected.sort_by_key(key);
    given.sort_by_key(key);
    if expected != given {
        return Err("graph.edges does not match the nodes' inputs".to_string());
    }
    for node in &graph.nodes {
        if node.kind == "input" {
            let declared = param_fan_in(&node.params, "many");
            if node.outputs.first().and_then(|port| port.many) != declared {
                return Err(format!("`{}`: the output port's bound does not match its `many`", node.id));
            }
            // An ordered Input's element type is its `type`, and its port is
            // named after it: declared, never inferred, so it is checked
            // here rather than normalised by the writer.
            if declared.is_some() {
                let ty = param_port_type(&node.params, "type")
                    .ok_or_else(|| format!("`{}`: an ordered Input needs a `type`", node.id))?;
                let matches = node.outputs.len() == 1
                    && node.outputs.first().is_some_and(|port| port.ty == ty && port.name == port_type_name(&ty));
                if !matches {
                    return Err(format!(
                        "`{}`: its ordered output is not the {} port its `type` declares",
                        node.id,
                        port_type_name(&ty)
                    ));
                }
            }
        }
        let component = if node.kind == "component" {
            Some(
                components
                    .iter()
                    .find(|def| def.alias == node.type_name)
                    .ok_or_else(|| format!("`{}` uses component `{}`, which is not pinned", node.id, node.type_name))?,
            )
        } else {
            None
        };
        match component {
            // A container's outputs are its pinned component's, whatever
            // the client says they are.
            Some(def) => {
                if node.outputs != def.outputs {
                    return Err(format!(
                        "`{}`: its output ports are not those component `{}` declares",
                        node.id, def.alias
                    ));
                }
            }
            // Only an Input and a Map declare an ordered output (checked
            // against their `many`); no other built-in has one.
            None => {
                if node.kind != "input" && node.kind != "map" && registry.descriptor(&node.type_name).is_none_or(|descriptor| descriptor.codec != NodeCodec::Fields) {
                    if let Some(port) = node.outputs.iter().find(|port| port.many.is_some()) {
                        return Err(format!("`{}.{}` cannot be an ordered port", node.id, port.name));
                    }
                }
            }
        }
        for input in &node.inputs {
            match (&input.value, input.many) {
                (NodeInputValue::Edges(entries), Some(bound)) => {
                    if !(1..=MAX_SEQUENCE).contains(&bound) {
                        return Err(format!("`{}.{}`: bound {bound} is not within 1..={MAX_SEQUENCE}", node.id, input.port));
                    }
                    if entries.len() > MAX_FAN_IN {
                        return Err(format!("`{}.{}` lists {} entries, over the {MAX_FAN_IN} entry limit", node.id, input.port, entries.len()));
                    }
                    for (index, entry) in entries.iter().enumerate() {
                        if entry.id == 0 || entry.id > MAX_CONNECTION_ID {
                            return Err(format!("`{}.{}` entry {}: id {} is not within 1..={MAX_CONNECTION_ID}", node.id, input.port, index + 1, entry.id));
                        }
                        if entries[..index].iter().any(|other| other.id == entry.id) {
                            return Err(format!("`{}.{}` entry {}: id {} is used twice", node.id, input.port, index + 1, entry.id));
                        }
                    }
                }
                (NodeInputValue::Edges(_), None) => {
                    return Err(format!("`{}.{}` has entries but is a one port", node.id, input.port));
                }
                (_, Some(_)) => {
                    return Err(format!("`{}.{}` is an ordered port but carries no entry list", node.id, input.port));
                }
                _ => {}
            }
            if node.type_name == "Output" && input.port == "value" && input.many != param_fan_in(&node.params, "many") {
                return Err(format!("`{}`: the value port's bound does not match its `many`", node.id));
            }
            if node.kind == "choose" && input.port == "from"
                && (input.many.is_none() || input.many != param_fan_in(&node.params, "many")
                    || param_port_type(&node.params, "type") != Some(input.ty.clone()))
            {
                return Err(format!("`{}`: its candidates are not the ordered `type` and `many` it declares", node.id));
            }
            if node.kind == "map" && input.port == "body" && param_port_type(&node.params, "type") != Some(input.ty.clone()) {
                return Err(format!("`{}`: its body is not of the `type` it declares", node.id));
            }
            if node.type_name == "Output" && input.port == "value" && input.many.is_some() {
                // An ordered result's element type is its `type`.
                if param_port_type(&node.params, "type") != Some(input.ty.clone()) {
                    return Err(format!("`{}`: its ordered value is not of the `type` it declares", node.id));
                }
            }
            // An ordered input exists only where the language can write
            // one: an Fn's own declaration, an Output's `many`, a pinned
            // component's port. Anywhere else the writer could not say it.
            let may_be_ordered =
                registry.descriptor(&node.type_name).is_some_and(|descriptor| matches!(descriptor.codec, NodeCodec::Fn | NodeCodec::Fields))
                    || (node.type_name == "Output" && input.port == "value")
                    || (node.kind == "choose" && input.port == "from")
                    || component.is_some();
            if input.many.is_some() && !may_be_ordered {
                return Err(format!("`{}.{}` cannot be an ordered port", node.id, input.port));
            }
            if let Some(def) = component {
                let port = def
                    .inputs
                    .iter()
                    .find(|port| port.name == input.port)
                    .ok_or_else(|| format!("component `{}` has no input port `{}`", def.alias, input.port))?;
                if port.ty != input.ty || port.many != input.many {
                    return Err(format!(
                        "`{}.{}` is {} but component `{}` declares {}",
                        node.id,
                        input.port,
                        input.shape().describe(),
                        def.alias,
                        port.shape().describe()
                    ));
                }
            }
        }
    }
    for node in graph.nodes.iter().filter(|node| matches!(node.kind.as_str(), "map" | "choose")) {
        let ty = param_port_type(&node.params, "type")
            .ok_or_else(|| format!("`{}`: a {} needs a `type`", node.id, node.type_name))?;
        let expected = if node.kind == "map" {
            let many = param_fan_in(&node.params, "many").ok_or_else(|| format!("`{}`: a Map needs `many`", node.id))?;
            vec![Port { name: "items".into(), ty, many: Some(many) }]
        } else {
            vec![Port { name: ty.as_str().to_string(), ty, many: None }]
        };
        if node.outputs != expected {
            return Err(format!("`{}`: its output is not the one its `type` and `many` declare", node.id));
        }
    }
    validate_acyclic(&graph.nodes, &graph.edges, "graph").map_err(|error| error.message)?;
    validate_edges(&graph.nodes, &graph.edges, "graph").map_err(|error| error.message)?;
    validate_maps(&graph.nodes).map_err(|(_, message)| message)
}

/// The node kinds a Map may run once per element: the job kinds that
/// start from their inputs and complete without a caller.
pub const MAP_BODY_KINDS: &[&str] = &["gen", "chat", "http", "fn", "provider"];

/// A Map's `body` is a wire from the job node it runs once per element;
/// that node is the Map's alone (nothing else reads it), and `bind` names
/// one of its parameters or input ports. Returns the offending node and why.
pub fn validate_maps(nodes: &[Node]) -> Result<(), (String, String)> {
    for map in nodes.iter().filter(|node| node.kind == "map") {
        let fail = |message: String| Err((map.id.clone(), message));
        let Some(NodeInputValue::Edge(edge)) = map.inputs.iter().find(|input| input.port == "body").map(|input| &input.value) else {
            return fail(format!("Map `{}` needs `body: node.port()`, the job it runs once per element", map.id));
        };
        let Some(body) = nodes.iter().find(|node| node.id == edge.from_node) else {
            return fail(format!("Map `{}` reads `{}`, which is not declared", map.id, edge.from_node));
        };
        if !MAP_BODY_KINDS.contains(&body.kind.as_str()) {
            return fail(format!(
                "Map `{}` cannot run `{}` per element: a body is a generator, Llm, Http, Fn or provider job, not a {}",
                map.id, body.id, body.type_name
            ));
        }
        let bind = match map.params.iter().find(|(name, _)| name == "bind").map(|(_, value)| value) {
            Some(Literal::Id(name) | Literal::Str(name)) => name.as_str(),
            _ => return fail(format!("Map `{}` needs `bind`, the parameter or input each element sets", map.id)),
        };
        if !body.params.iter().any(|(name, _)| name == bind) && !body.inputs.iter().any(|input| input.port == bind) {
            return fail(format!("Map `{}` binds `{bind}`, which `{}` has as neither a parameter nor an input", map.id, body.id));
        }
        for other in nodes {
            let reads_body = other.inputs.iter().any(|input| match &input.value {
                NodeInputValue::Edge(from) => from.from_node == body.id,
                NodeInputValue::Edges(entries) => entries.iter().any(|entry| entry.from.from_node == body.id),
                NodeInputValue::Literal(_) => false,
            });
            let is_map_body = other.kind == "map"
                && other.inputs.iter().any(|input| input.port == "body" && matches!(&input.value, NodeInputValue::Edge(from) if from.from_node == body.id));
            if reads_body && (other.id != map.id || !is_map_body || other.inputs.iter().filter(|input| input.port != "body").any(|input| {
                matches!(&input.value, NodeInputValue::Edge(from) if from.from_node == body.id)
            })) {
                return fail(format!(
                    "`{}` is the body of Map `{}` and runs only there; `{}` reads it directly, read `{}.items()` instead",
                    body.id, map.id, other.id, map.id
                ));
            }
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
        return Err(at_loc(
            file_name,
            &node.loc,
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
    vm.bx.heap_ref().object_map_iter_ordered(tools_obj, |key, value| {
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
    for index in 0..vm.bx.heap_ref().array_len(array) {
        let value = vm.bx.heap_ref().array_index_unchecked(array, index);
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
    for index in 0..vm.bx.heap_ref().array_len(array) {
        let value = vm.bx.heap_ref().array_index_unchecked(array, index);
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

/// An Input's or Output's `many` bound, when it declares one.
fn param_fan_in(params: &[(String, Literal)], name: &str) -> Option<u32> {
    params.iter().find_map(|(param, value)| match value {
        Literal::Num(bound) if param == name => Some(*bound as u32),
        _ => None,
    })
}

fn param_port_type(params: &[(String, Literal)], name: &str) -> Option<PortType> {
    params.iter().find_map(|(param, value)| {
        if param == name {
            if let Literal::Id(value) | Literal::Str(value) = value {
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
                Literal::Id(value) | Literal::Str(value) => Some(value.clone()),
                _ => None,
            })
            .collect()
    })
}

fn id_array(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<Vec<String>> {
    let array = value.as_array()?;
    let mut values = Vec::new();
    for index in 0..vm.bx.heap_ref().array_len(array) {
        values.push(key_name(vm, vm.bx.heap_ref().array_index_unchecked(array, index))?);
    }
    Some(values)
}

fn infer_literal_type(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<PortType> {
    if value.as_object().is_some_and(|obj| vm.bx.heap_ref().is_fn(obj)) {
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
        for index in 0..vm.bx.heap_ref().array_len(array) {
            values.push(literal_from_value_inner(
                vm,
                vm.bx.heap_ref().array_index_unchecked(array, index),
                objects,
                arrays,
            )?);
        }
        arrays.remove(&array);
        return Ok(Literal::Arr(values));
    }
    if let Some(obj) = value.as_object() {
        if vm.bx.heap_ref().is_fn(obj) {
            return Err("closure is not a literal".to_string());
        }
        if !objects.insert(obj) {
            return Err("cyclic object is not a flow literal".to_string());
        }
        let mut values = Vec::new();
        vm.bx.heap_ref().object_map_iter_ordered(obj, |key, value| {
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
    let heap = vm.bx.heap_ref();
    heap.object_own_value(obj, &key).or_else(|| {
        heap.vec_ref(obj)
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value)
    }).or_else(|| vm.bx.heap_ref().check_intern_string(name).and_then(|key| heap.object_own_value(obj, &key)))
}

fn deep_value(vm: &ScriptVm<'_>, mut obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    loop {
        if let Some(value) = own_value(vm, obj, name) {
            return Some(value);
        }
        obj = vm.bx.heap_ref().proto(obj).as_object()?;
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
    vm.bx.heap_ref().string_with(value, |_heap, value| value.to_string())
}

fn input_value(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    name: &str,
    default: &Literal,
) -> ScriptValue {
    let value = deep_value(vm, obj, name).unwrap_or(NIL);
    if matches!(default, Literal::Null)
        && value
            .as_object()
            .is_some_and(|object| vm.bx.heap_ref().is_fn(object))
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
    vm.bx.code_ref()
        .ip_to_loc(vm.bx.heap_ref().made_at(obj))
        .map(|loc| Loc {
            line: loc.line(),
            col: loc.col(),
        })
        .unwrap_or_default()
}

/// An error at an object's construction site. A construction [`Loc`] is
/// the script's 0-based row and column of the object's `{`; diagnostics
/// are 1-based.
fn at_loc(file: &str, loc: &Loc, message: impl Into<String>) -> EvalError {
    at(file, loc.line + 1, loc.col + 1, message)
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
        .unwrap_or((fallback.line + 1, fallback.col + 1));
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
        .unwrap_or((fallback.line + 1, fallback.col + 1));
    at(file_name, line, col, message)
}

fn object_span(vm: &ScriptVm<'_>, obj: ScriptObject, source: &str, file: &str) -> Option<(usize, usize)> {
    let loc = vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(obj))?;
    // An object a prelude function made has its construction site in the
    // prelude's text: no place in `source`, whatever its row and column.
    if loc.file() != file {
        return None;
    }
    let offset = line_col_offset(source, loc.line(), loc.col())?;
    let open = find_next_code_char(source, offset, b'{')?;
    let close = matching_delimiter(source, open, b'{', b'}')?;
    Some((open, close + 1))
}

/// The prototype chain of an object with each level's construction site and
/// nothing else. The extractor asks this several times per node; the VM's
/// `construction_chain` also re-resolves the docs of every body along the
/// chain, which made a 512-node flow load in seconds.
fn prototype_chain(vm: &ScriptVm<'_>, obj: ScriptObject) -> Vec<(ScriptObject, Option<ScriptLoc>)> {
    // The VM's own chain bound.
    const MAX_CHAIN: usize = 24;
    let mut out = Vec::new();
    let mut cur: ScriptValue = obj.into();
    while let Some(object) = cur.as_object() {
        if out.len() >= MAX_CHAIN {
            break;
        }
        out.push((object, vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(object))));
        cur = vm.bx.heap_ref().proto(object);
    }
    out
}

fn source_field_in_chain(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    source: &str,
    file_name: &str,
    field: &str,
) -> Option<(usize, usize)> {
    for (object, loc) in prototype_chain(vm, obj) {
        if !loc.as_ref().is_some_and(|location| location.file() == file_name) {
            continue;
        }
        if own_value(vm, object, field).is_none() {
            continue;
        }
        let span = object_span(vm, object, source, file_name)?;
        if let Some(range) = field_sources(source, span).get(field).copied() {
            return Some(range);
        }
    }
    None
}

fn line_col_offset(source: &str, line: u32, col: u32) -> Option<usize> {
    let mut offset = 0usize;
    // Object construction locations are the script's zero-based row and
    // column; source locations exposed by flow diagnostics are one-based.
    let target_line = line as usize + 1;
    for (index, text) in source.split_inclusive('\n').enumerate() {
        if index + 1 == target_line {
            let byte_col = text
                .char_indices()
                .nth(col as usize)
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

/// Field spans extend through trailing comments. Exclude only surrounding
/// trivia when reading or replacing layout literals; preserve those bytes.
fn layout_value_range(source: &str, range: (usize, usize)) -> (usize, usize) {
    let bytes = source.as_bytes();
    let mut index = range.0;
    let mut start = None;
    let mut end = range.0;
    while index < range.1 {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() || byte == b',' => index += 1,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index).min(range.1),
            b'/' if bytes.get(index + 1) == Some(&b'*') => index = skip_block_comment(bytes, index).unwrap_or(range.1).min(range.1),
            b'"' => {
                start.get_or_insert(index);
                index = skip_string(bytes, index).unwrap_or(range.1).min(range.1);
                end = index;
            }
            _ => {
                start.get_or_insert(index);
                index += 1;
                end = index;
            }
        }
    }
    (start.unwrap_or(range.0), end)
}

fn parse_vec2(value: &str) -> Option<(f64, f64)> {
    let value = value.trim();
    let inner = value.strip_prefix("vec2(").or_else(|| value.strip_prefix("mod.flow.vec2("))?.strip_suffix(')')?;
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

fn port_type(value: &str) -> Option<PortType> { PortType::from_str(value) }

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

fn port_type_name(value: &PortType) -> &str { value.as_str() }

mod schema;
mod writer;

pub use schema::{tool_schema, tool_schema_with_registry};
pub use writer::{is_canonical, write, is_canonical_with, write_with_registry, is_canonical_with_registry, patch_layout_with_registry, patch_layout_closure_with_registry, patch_inputs_closure_with_registry, patch_add_nodes_closure_with_registry, patch_remove_nodes_closure_with_registry, WriteError};
