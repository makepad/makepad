use super::*;
use std::collections::HashMap;

/// Serialize a graph into the stable, flat splash representation.
pub fn write(graph: &Graph) -> String {
    let order = topological_order(graph);
    let by_id: HashMap<_, _> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut out = String::from("use mod.flow.*\n\n");
    for (position, id) in order.iter().enumerate() {
        write_node(&mut out, by_id[id.as_str()], position, &by_id);
    }
    out.push_str("Flow{\n");
    out.push_str("    label: ");
    write_string(&mut out, &graph.label);
    out.push_str("\n    brief: ");
    write_string(&mut out, &graph.brief);
    out.push('\n');
    if graph.trigger != "manual" {
        out.push_str("    trigger: @");
        out.push_str(&graph.trigger);
        out.push('\n');
    }
    if graph.concurrency != 1 {
        out.push_str("    concurrency: ");
        out.push_str(&graph.concurrency.to_string());
        out.push('\n');
    }
    if graph.autostart {
        out.push_str("    autostart: true\n");
    }
    let custom_tools: Vec<_> = graph.tools.iter().filter(|tool| tool.name != "run").collect();
    if !custom_tools.is_empty() {
        out.push_str("    tools: {\n");
        for tool in custom_tools {
            out.push_str("        ");
            out.push_str(&tool.name);
            out.push_str(": { in: [");
            out.push_str(&tool.inputs.join(", "));
            out.push_str("]  out: [");
            out.push_str(&tool.outputs.join(", "));
            out.push_str("] }\n");
        }
        out.push_str("    }\n");
    }
    if let Some(face) = &graph.flow_ui_src {
        out.push_str("    ui: ");
        out.push_str(face.trim());
        out.push('\n');
    }
    out.push_str("    ");
    out.push_str(&order.join(", "));
    out.push_str("\n}\n");
    out
}

fn write_node(
    out: &mut String,
    node: &Node,
    position: usize,
    by_id: &HashMap<&str, &Node>,
) {
    if let Some(doc) = &node.doc {
        out.push_str("/** ");
        out.push_str(&doc.replace("*/", "* /"));
        out.push_str(" */\n");
    }
    out.push_str("let ");
    out.push_str(&node.id);
    out.push_str(" = ");
    out.push_str(&node.type_name);
    out.push_str("{\n");

    match node.type_name.as_str() {
        "Input" | "Text" => {
            write_param(out, node, "type", false);
            write_param(out, node, "default", false);
        }
        "Output" => {
            write_param(out, node, "type", false);
            write_input(out, node, "value", true, by_id);
        }
        "Llm" => {
            write_param(out, node, "system", false);
            write_input(out, node, "prompt", false, by_id);
            write_param(out, node, "model", false);
            write_param(out, node, "temperature", false);
            write_param(out, node, "max_tokens", false);
        }
        "Fn" => {
            out.push_str("    in: {");
            for (index, input) in node.inputs.iter().enumerate() {
                if index > 0 {
                    out.push_str("  ");
                }
                out.push_str(&input.port);
                out.push_str(": ");
                write_input_value(out, &input.value, by_id);
            }
            out.push_str("}\n");
            out.push_str("    out: [");
            for (index, port) in node.outputs.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push('@');
                out.push_str(&port.name);
            }
            out.push_str("]\n");
            out.push_str("    run: ");
            out.push_str(node.fn_src.as_deref().unwrap_or("|i| { i }").trim());
            out.push('\n');
        }
        "Http" => {
            write_param(out, node, "method", false);
            write_input(out, node, "url", false, by_id);
            write_input(out, node, "headers", false, by_id);
            write_input(out, node, "body", false, by_id);
            write_param(out, node, "content_type", false);
            write_param(out, node, "out", false);
            write_param(out, node, "accept", false);
        }
        "Ask" => {
            for name in ["question", "type", "options", "default", "timeout"] {
                write_param(out, node, name, false);
            }
        }
        "Image" => {
            write_ports(out, node);
            for input in &node.inputs {
                write_input(out, node, &input.port, false, by_id);
            }
            write_param(out, node, "width", true);
            write_param(out, node, "height", true);
            for name in ["steps", "seed", "negative", "model"] {
                write_param(out, node, name, false);
            }
        }
        "Upscale" => {
            write_ports(out, node);
            for input in &node.inputs {
                write_input(out, node, &input.port, false, by_id);
            }
            write_param(out, node, "factor", false);
        }
        "Gen" => {
            out.push_str("    domain: ");
            write_string(out, node.domain.as_deref().unwrap_or(""));
            out.push('\n');
            write_ports(out, node);
            for (name, value) in &node.params {
                out.push_str("    ");
                out.push_str(name);
                out.push_str(": ");
                write_literal(out, value);
                out.push('\n');
            }
            for input in &node.inputs {
                write_input(out, node, &input.port, false, by_id);
            }
        }
        _ => {}
    }

    if node.on_fail != "fail" {
        out.push_str("    on_fail: @");
        out.push_str(&node.on_fail);
        out.push('\n');
    }
    if let Some(label) = &node.label {
        out.push_str("    label: ");
        write_string(out, label);
        out.push('\n');
    }
    let at = node
        .at
        .unwrap_or((40.0 + position as f64 * 320.0, 120.0));
    out.push_str("    at: vec2(");
    write_number(out, at.0);
    out.push_str(", ");
    write_number(out, at.1);
    out.push_str(")\n");
    if let Some(face) = &node.face_src {
        out.push_str("    ui: ");
        out.push_str(face.trim());
        out.push('\n');
    }
    out.push_str("}\n\n");
}

fn write_ports(out: &mut String, node: &Node) {
    out.push_str("    ports: { in: {");
    for (index, input) in node.inputs.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&input.port);
        out.push_str(": @");
        out.push_str(port_type_name(input.ty));
    }
    out.push_str("}  out: {");
    for (index, port) in node.outputs.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&port.name);
        out.push_str(": @");
        out.push_str(port_type_name(port.ty));
    }
    out.push_str("} }\n");
}

fn write_param(out: &mut String, node: &Node, name: &str, force: bool) {
    let Some(value) = node
        .params
        .iter()
        .find_map(|(param, value)| (param == name).then_some(value))
    else {
        return;
    };
    let default = type_spec(&node.type_name)
        .and_then(|spec| spec.params.iter().find(|param| param.name == name).copied())
        .map(|param| default_literal(param.default));
    if !force && default.as_ref() == Some(value) {
        return;
    }
    out.push_str("    ");
    out.push_str(name);
    out.push_str(": ");
    write_literal(out, value);
    out.push('\n');
}

fn write_input(
    out: &mut String,
    node: &Node,
    name: &str,
    required: bool,
    by_id: &HashMap<&str, &Node>,
) {
    let Some(input) = node.inputs.iter().find(|input| input.port == name) else {
        return;
    };
    let default = type_spec(&node.type_name)
        .and_then(|spec| spec.inputs.iter().find(|input| input.name == name).copied())
        .map(|input| default_literal(input.default));
    if !required {
        if let NodeInputValue::Literal(value) = &input.value {
            if default.as_ref() == Some(value) || matches!(value, Literal::Null) {
                return;
            }
        }
    }
    out.push_str("    ");
    out.push_str(name);
    out.push_str(": ");
    write_input_value(out, &input.value, by_id);
    out.push('\n');
}

fn write_input_value(out: &mut String, value: &NodeInputValue, by_id: &HashMap<&str, &Node>) {
    match value {
        NodeInputValue::Literal(value) => write_literal(out, value),
        NodeInputValue::Edge(edge) => {
            out.push_str(&edge.from_node);
            let use_generic_out = by_id
                .get(edge.from_node.as_str())
                .is_some_and(|source| source.kind == "gen");
            if use_generic_out {
                out.push_str(".out(@");
                out.push_str(&edge.from_port);
                out.push(')');
            } else {
                out.push('.');
                out.push_str(&edge.from_port);
                out.push_str("()");
            }
        }
    }
}

fn write_literal(out: &mut String, value: &Literal) {
    match value {
        Literal::Null => out.push_str("nil"),
        Literal::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Literal::Num(value) => write_number(out, *value),
        Literal::Str(value) => write_string(out, value),
        Literal::Id(value) => {
            out.push('@');
            out.push_str(value);
        }
        Literal::Arr(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_literal(out, value);
            }
            out.push(']');
        }
        Literal::Obj(values) => {
            out.push('{');
            for (index, (name, value)) in values.iter().enumerate() {
                if index > 0 {
                    out.push_str("  ");
                }
                out.push_str(name);
                out.push_str(": ");
                write_literal(out, value);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, value: &str) {
    makepad_strict_json::Value::Str(value.to_string()).write_into(out);
}

fn write_number(out: &mut String, value: f64) {
    if value.fract() == 0.0 {
        out.push_str(&(value as i64).to_string());
    } else {
        out.push_str(&value.to_string());
    }
}

fn default_literal(value: DefaultValue) -> Literal {
    match value {
        DefaultValue::Null => Literal::Null,
        DefaultValue::Num(value) => Literal::Num(value),
        DefaultValue::Str(value) => Literal::Str(value.to_string()),
        DefaultValue::Id(value) => Literal::Id(value.to_string()),
        DefaultValue::Arr => Literal::Arr(Vec::new()),
        DefaultValue::Obj => Literal::Obj(Vec::new()),
    }
}

fn topological_order(graph: &Graph) -> Vec<String> {
    let original: HashMap<_, _> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let mut indegree: HashMap<_, usize> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0))
        .collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if let Some(degree) = indegree.get_mut(edge.to_node.as_str()) {
            *degree += 1;
        }
        outgoing
            .entry(edge.from_node.as_str())
            .or_default()
            .push(edge.to_node.as_str());
    }
    let mut ready: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| indegree[node.id.as_str()] == 0)
        .map(|node| node.id.as_str())
        .collect();
    let mut order = Vec::new();
    while !ready.is_empty() {
        ready.sort_by_key(|id| original[id]);
        let id = ready.remove(0);
        order.push(id.to_string());
        for next in outgoing.get(id).into_iter().flatten() {
            if let Some(degree) = indegree.get_mut(next) {
                *degree -= 1;
                if *degree == 0 {
                    ready.push(next);
                }
            }
        }
    }
    if order.len() != graph.nodes.len() {
        graph.nodes.iter().map(|node| node.id.clone()).collect()
    } else {
        order
    }
}

/// Whether `source` already is the writer's flat form, ignoring whitespace.
pub fn is_canonical(source: &str) -> bool {
    let Ok(graph) = evaluate(source, "<canonical>") else {
        return false;
    };
    compact(source) == compact(&write(&graph))
}

fn compact(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index] == b'"' {
            let end = skip_string(bytes, index).unwrap_or(bytes.len());
            out.push_str(&source[index..end]);
            index = end;
            continue;
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}
