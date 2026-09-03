use crate::wire::*;
use makepad_script::*;
use std::collections::{HashMap, HashSet};

const INSTRUCTION_LIMIT: usize = 5_000_000;
const HEAP_LIMIT: usize = 64 * 1024 * 1024;
const PRELUDE_FILE: &str = "<makepad-flow-prelude>";
const RECIPE_PRELUDE_FILE: &str = "<makepad-flow-recipe-prelude>";
const RECIPE_PRELUDE: &str = include_str!("../../recipes/prelude_recipes.splash");

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
    ParamSpec::new("default", ParamType::Literal, DefaultValue::Str("")),
];
const OUTPUT_PARAMS: &[ParamSpec] = &[ParamSpec::new(
    "type",
    ParamType::PortTypeWithBytes,
    DefaultValue::Id("text"),
)];
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
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm {
        host: &mut host,
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
                return Err(error_from_vm(
                    &recipe_errors[0],
                    RECIPE_PRELUDE_FILE,
                ));
            }
            vm.bx.captured_errors = Some(Vec::new());
            let eval_source = source_for_eval(source);
            let value = vm.eval(make_mod(file_name, &eval_source));
            let errors = vm.take_errors();
            if !errors.is_empty() {
                return Err(error_from_vm(&errors[0], file_name));
            }
            extract(&vm, value, source, file_name)
        })
    });
    if allocation.exceeded {
        result.and_then(|_| {
            Err(at(
                file_name,
                1,
                1,
                "flow exceeded the 64 MiB heap allocation limit",
            ))
        })
    } else {
        result
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
    validate_edges(&nodes, &edges, file_name)?;
    validate_acyclic(&nodes, &edges, file_name)?;
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
            "Text", "Image", "Upscale", "Input", "Output", "Llm", "Fn", "Http", "Ask",
            "Gen", "Flow",
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
) -> Result<Node, EvalError> {
    let spec = type_spec(type_name).unwrap();
    let loc = object_loc(vm, obj, file_name);
    let span = object_span(vm, obj, source);
    let fields = span.map(|span| field_sources(source, span)).unwrap_or_default();
    let mut params = Vec::new();
    for param in spec.params {
        let value = deep_value(vm, obj, param.name).unwrap_or(NIL);
        let literal = literal_from_value(vm, value).map_err(|message| {
            field_error(source, span, param.name, file_name, message, &loc)
        })?;
        validate_param(param, &literal).map_err(|message| {
            field_error(source, span, param.name, file_name, message, &loc)
        })?;
        params.push((param.name.to_string(), literal));
    }
    if type_name == "Gen" {
        params = generic_gen_params(vm, obj, source, span, file_name, &loc)?;
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
                &name,
                inferred,
                true,
                value,
                object_ids,
            )?);
        }
    } else if type_name == "Gen" {
        let input_names = ports_side(vm, obj, "in", source, span, file_name, &loc)?;
        for name in input_names {
            let value = input_value(vm, obj, &name, DefaultValue::Null);
            inputs.push(extract_input(
                vm,
                source,
                file_name,
                span,
                &loc,
                &name,
                input_port_type(&name),
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
                input.name,
                declared_ty,
                input.flexible,
                value,
                object_ids,
            )?);
        }
    }

    let outputs = outputs_for(vm, obj, type_name, &params, source, span, file_name, &loc)?;
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
    let domain = if spec.kind == "gen" {
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
    name: &str,
    declared_ty: PortType,
    flexible: bool,
    value: ScriptValue,
    object_ids: &HashMap<ScriptObject, String>,
) -> Result<NodeInput, EvalError> {
    if let Some(edge) = port_ref(vm, value, object_ids, source, span, name, file_name, loc)? {
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
) -> Result<Vec<(String, Literal)>, EvalError> {
    let input_names: HashSet<String> = ports_side(
        vm, obj, "in", source, span, file_name, loc,
    )?
    .into_iter()
    .collect();
    let reserved: HashSet<&str> = [
        "kind", "type_name", "domain", "ports", "at", "ui", "on_fail", "label", "out",
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

fn port_ref(
    vm: &ScriptVm<'_>,
    value: ScriptValue,
    object_ids: &HashMap<ScriptObject, String>,
    source: &str,
    span: Option<(usize, usize)>,
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
        field_error(
            source,
            span,
            field,
            file_name,
            "node not in flow",
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

fn outputs_for(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    type_name: &str,
    params: &[(String, Literal)],
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
    loc: &Loc,
) -> Result<Vec<Port>, EvalError> {
    Ok(match type_name {
        "Input" | "Text" | "Ask" => vec![Port {
            name: port_type_name(param_port_type(params, "type").unwrap()).to_string(),
            ty: param_port_type(params, "type").unwrap(),
        }],
        "Output" => vec![],
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
        "Image" => vec![Port {
            name: "image".to_string(),
            ty: PortType::Image,
        }],
        "Upscale" => vec![Port {
            name: "image".to_string(),
            ty: PortType::Image,
        }],
        "Gen" => ports_side(vm, obj, "out", source, span, file_name, loc)?
            .into_iter()
            .map(|name| Port {
                ty: port_type(&name).unwrap_or(PortType::Json),
                name,
            })
            .collect(),
        _ => unreachable!(),
    })
}

fn ports_side(
    vm: &ScriptVm<'_>,
    obj: ScriptObject,
    side: &str,
    source: &str,
    span: Option<(usize, usize)>,
    file_name: &str,
    loc: &Loc,
) -> Result<Vec<String>, EvalError> {
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
    id_array(vm, value).ok_or_else(|| {
        field_error(
            source,
            span,
            "ports",
            file_name,
            format!("Gen.ports.{side} must be an array of port ids"),
            loc,
        )
    })
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
            let flexible = node.kind == "fn" || (node.kind == "http" && input.port == "body");
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
        let flexible = to.kind == "fn" || (to.kind == "http" && input.port == "body");
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
        .filter(|node| node.kind == "output")
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
            tool_node_array(vm, entry, "out", &node_by_obj, "output").map_err(|msg| {
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
    // Object construction ips currently resolve one-based while diagnostic
    // ips resolve zero-based; this is the coordinate convention exposed by
    // `made_at` + `ip_to_loc` in makepad-script.
    let target_line = line.max(1) as usize;
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
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let col = prefix
        .rfind('\n')
        .map(|index| prefix[index + 1..].chars().count())
        .unwrap_or_else(|| prefix.chars().count()) as u32;
    (line, col)
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

// `in` is a language keyword in the current parser, although the flow syntax uses
// it as the conventional Fn closure argument. Rewrite only that closure binding
// and its body for VM execution. The replacement is the same byte width, so all
// source locations and slices continue to address the original file.
fn source_for_eval(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    mask_ui_values(source, &mut out);
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
            b'|' => {
                let mut name_start = index + 1;
                while bytes.get(name_start).is_some_and(u8::is_ascii_whitespace) {
                    name_start += 1;
                }
                if bytes.get(name_start..name_start + 2) == Some(b"in")
                    && !bytes
                        .get(name_start + 2)
                        .is_some_and(|byte| is_ident_continue(*byte))
                {
                    let mut pipe = name_start + 2;
                    while bytes.get(pipe).is_some_and(u8::is_ascii_whitespace) {
                        pipe += 1;
                    }
                    if bytes.get(pipe) == Some(&b'|') {
                        let Some(open) = find_next_code_char(source, pipe + 1, b'{') else {
                            break;
                        };
                        let Some(close) = matching_delimiter(source, open, b'{', b'}') else {
                            break;
                        };
                        out[name_start + 1] = b't';
                        rewrite_identifier(bytes, &mut out, open + 1, close, b"in", b"it");
                        index = close + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
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

fn rewrite_identifier(
    source: &[u8],
    out: &mut [u8],
    mut index: usize,
    end: usize,
    from: &[u8; 2],
    to: &[u8; 2],
) {
    while index < end {
        match source[index] {
            b'"' => {
                index = skip_string(source, index).unwrap_or(end);
                continue;
            }
            b'/' if source.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(source, index);
                continue;
            }
            b'/' if source.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(source, index).unwrap_or(end);
                continue;
            }
            byte if is_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < end && is_ident_continue(source[index]) {
                    index += 1;
                }
                let previous = source[..start]
                    .iter()
                    .rev()
                    .copied()
                    .find(|byte| !byte.is_ascii_whitespace());
                let next = source[index..end]
                    .iter()
                    .copied()
                    .find(|byte| !byte.is_ascii_whitespace());
                let is_variable_use = next == Some(b'.')
                    || (previous != Some(b'.')
                        && previous != Some(b'@')
                        && next != Some(b':')
                        && !next.is_some_and(is_ident_start));
                if &source[start..index] == from && is_variable_use {
                    out[start..index].copy_from_slice(to);
                }
                continue;
            }
            _ => index += 1,
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
