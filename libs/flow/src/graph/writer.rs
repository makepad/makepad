use super::*;
use std::collections::HashMap;

/// Serialize a graph into the stable, flat splash representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError { Unregistered(String), Invalid(String), NonLayout }
impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unregistered(name) => write!(f, "node type `{name}` is not registered"),
            Self::Invalid(message) => f.write_str(message),
            Self::NonLayout => f.write_str("source-preserving graph edits permit only node layout changes; edit source to change behavior"),
        }
    }
}
impl std::error::Error for WriteError {}

/// Move, resize or flip uniquely constructed root nodes without rewriting
/// their custom source. All other evaluated fields, code and module pins
/// must remain unchanged. This evaluates twice; call it off the UI thread.
/// This checks the evaluated graph, not arbitrary future closure behavior:
/// code that reads a node's authored layout can observe the layout edit.
pub fn patch_layout_with_registry(source: &str, wanted: &Graph, closure: &Closure, registry: &Registry) -> Result<String, WriteError> {
    const FILE: &str = "<flow-layout-edit>";
    let (mut loaded, before) = FlowVm::load_with_registry(source, FILE, closure, registry)
        .map_err(layout_eval_error)?;
    if layout_projection(&before, true) != layout_projection(wanted, true) {
        return Err(WriteError::NonLayout);
    }
    validate_projection_with_registry(wanted, &before.components, registry).map_err(WriteError::Invalid)?;
    let flow = loaded.flow.as_object();
    let written = loaded.with_vm(|vm| patch_node_layout(vm, flow, source, FILE, &before, wanted))?;
    let after = evaluate_with_registry(&written, FILE, closure, registry).map_err(layout_eval_error)?;
    if layout_projection(&before, true) != layout_projection(&after, true) {
        return Err(WriteError::Invalid("layout is used by the flow's code; moving this node would change its evaluated fields".into()));
    }
    if layout_projection(wanted, false) != layout_projection(&after, false) {
        return Err(WriteError::Invalid("the requested layout does not survive evaluation; edit its computed source".into()));
    }
    Ok(written)
}

/// Patch root and pinned component layouts as one source/closure revision.
/// The caller supplies the authoritative closure; submitted component
/// identities and hashes must still match it. Only the edited layout
/// literals change, and only their modules receive new content hashes.
pub fn patch_layout_closure_with_registry(
    source: &str,
    wanted: &Graph,
    closure: &Closure,
    registry: &Registry,
) -> Result<(String, Closure), WriteError> {
    const FILE: &str = "<flow-layout-edit>";
    let (mut loaded, before) = FlowVm::load_with_registry(source, FILE, closure, registry)
        .map_err(layout_eval_error)?;
    if before.components.len() != wanted.components.len()
        || before.components.iter().zip(&wanted.components).any(|(old, new)| {
            old.alias != new.alias || old.path != new.path || old.hash != new.hash
                || old.inputs != new.inputs || old.outputs != new.outputs
        })
    {
        return Err(WriteError::Invalid("component identities, interfaces or pinned hashes changed; reload the current graph before editing its layout".into()));
    }
    if closure_layout_projection(&before, true, false) != closure_layout_projection(wanted, true, false) {
        return Err(WriteError::NonLayout);
    }
    validate_projection_with_registry(wanted, &wanted.components, registry).map_err(WriteError::Invalid)?;
    let flow = loaded.flow.as_object();
    let mut modules = closure.modules.clone();
    let written = loaded.with_vm(|vm| -> Result<String, WriteError> {
        let written = patch_node_layout(vm, flow, source, FILE, &before, wanted)?;
        let component_module = own_value(vm, vm.modules(), COMPONENTS_MODULE)
            .and_then(|value| value.as_object());
        for (old, new) in before.components.iter().zip(&wanted.components) {
            if !layout_changed(&old.graph, &new.graph) { continue; }
            let module = modules.iter_mut().find(|module| module.alias == old.alias)
                .ok_or_else(|| WriteError::Invalid(format!("component `{}` has no authoritative source", old.alias)))?;
            let object = component_module.and_then(|components| own_value(vm, components, &old.alias))
                .and_then(|value| value.as_object())
                .ok_or_else(|| WriteError::Invalid(format!("component `{}` has no evaluated declaration", old.alias)))?;
            module.source = patch_node_layout(vm, object, &module.source, &module.path, &old.graph, &new.graph)
                .map_err(|error| WriteError::Invalid(format!("{}: {error}", module.path)))?;
            module.hash = crate::document::closure::source_hash(&module.source);
        }
        Ok(written)
    })?;
    let closure = Closure::ordered(modules).map_err(layout_eval_error)?;
    let after = evaluate_with_registry(&written, FILE, &closure, registry).map_err(layout_eval_error)?;
    // New hashes are derived above, never accepted from the submitted graph.
    // Re-evaluate all dependent modules and the root together: a layout read
    // which changes an evaluated parameter or interface rejects the whole edit.
    if closure_layout_projection(&before, true, true) != closure_layout_projection(&after, true, true) {
        return Err(WriteError::Invalid("layout is used by the flow's code; moving this node would change its evaluated fields".into()));
    }
    if closure_layout_projection(wanted, false, true) != closure_layout_projection(&after, false, true) {
        return Err(WriteError::Invalid("the requested component layout does not survive evaluation; edit its computed source".into()));
    }
    Ok((written, closure))
}

/// Connect or disconnect wires of uniquely constructed nodes, root or pinned
/// component, without rewriting custom source. Only input values (wires and
/// their literal fallbacks) may differ from the evaluated graph; parameters,
/// layout, metadata and interfaces must stay, and are re-checked after the
/// root and every dependent module evaluate again. A field is edited only
/// where its source is the writer's own literal/wire form; control inputs,
/// component and `Fn` inputs are edited inside a literal `in: {..}` map.
/// New wires use named references resolved by Flow membership, so a source
/// may be declared after its consumer without reordering construction code.
/// Computed, shared or externally assigned sources are refused, never
/// flattened. Evaluates twice; call it off the UI thread.
pub fn patch_inputs_closure_with_registry(
    source: &str,
    wanted: &Graph,
    closure: &Closure,
    registry: &Registry,
) -> Result<(String, Closure), WriteError> {
    const FILE: &str = "<flow-input-edit>";
    let (mut loaded, before) = FlowVm::load_with_registry(source, FILE, closure, registry)
        .map_err(layout_eval_error)?;
    if before.components.len() != wanted.components.len()
        || before.components.iter().zip(&wanted.components).any(|(old, new)| {
            old.alias != new.alias || old.path != new.path || old.hash != new.hash
                || old.inputs != new.inputs || old.outputs != new.outputs
        })
    {
        return Err(WriteError::Invalid("component identities, interfaces or pinned hashes changed; reload the current graph before editing its wires".into()));
    }
    if inputs_projection(&before, true, false) != inputs_projection(wanted, true, false) {
        return Err(WriteError::Invalid("source-preserving wire edits change only input wires; edit source for anything else".into()));
    }
    validate_projection_with_registry(wanted, &wanted.components, registry).map_err(WriteError::Invalid)?;
    let flow = loaded.flow.as_object();
    let mut modules = closure.modules.clone();
    let written = loaded.with_vm(|vm| -> Result<String, WriteError> {
        let written = patch_node_inputs(vm, flow, source, FILE, &before, wanted, registry)?;
        let component_module = own_value(vm, vm.modules(), COMPONENTS_MODULE)
            .and_then(|value| value.as_object());
        for (old, new) in before.components.iter().zip(&wanted.components) {
            if !inputs_changed(&old.graph, &new.graph) { continue; }
            let module = modules.iter_mut().find(|module| module.alias == old.alias)
                .ok_or_else(|| WriteError::Invalid(format!("component `{}` has no authoritative source", old.alias)))?;
            let object = component_module.and_then(|components| own_value(vm, components, &old.alias))
                .and_then(|value| value.as_object())
                .ok_or_else(|| WriteError::Invalid(format!("component `{}` has no evaluated declaration", old.alias)))?;
            module.source = patch_node_inputs(vm, object, &module.source, &module.path, &old.graph, &new.graph, registry)
                .map_err(|error| WriteError::Invalid(format!("{}: {error}", module.path)))?;
            module.hash = crate::document::closure::source_hash(&module.source);
        }
        Ok(written)
    })?;
    let closure = Closure::ordered(modules).map_err(layout_eval_error)?;
    let after = evaluate_with_registry(&written, FILE, &closure, registry).map_err(layout_eval_error)?;
    if inputs_projection(&before, true, true) != inputs_projection(&after, true, true) {
        return Err(WriteError::Invalid("the flow's code reads this wire; changing it would change other evaluated fields".into()));
    }
    if inputs_projection(wanted, false, true) != inputs_projection(&after, false, true) {
        return Err(WriteError::Invalid("the requested wiring does not survive evaluation; edit its computed source".into()));
    }
    Ok((written, closure))
}

/// A graph without volatile fields; `without_inputs` also blanks every
/// input value. Edges are derived from inputs, so they never compare.
fn inputs_projection(graph: &Graph, without_inputs: bool, without_hashes: bool) -> Graph {
    let mut graph = layout_projection(graph, false);
    graph.edges.clear();
    if without_inputs {
        for input in graph.nodes.iter_mut().flat_map(|node| node.inputs.iter_mut()) {
            input.value = NodeInputValue::Literal(Literal::Null);
        }
    }
    for component in &mut graph.components {
        if without_hashes { component.hash.clear(); }
        component.graph = inputs_projection(&component.graph, without_inputs, without_hashes);
    }
    graph
}

fn inputs_changed(before: &Graph, wanted: &Graph) -> bool {
    before.nodes.iter().zip(&wanted.nodes).any(|(old, new)| old.inputs != new.inputs)
}

/// Whether `text` is exactly what the writer emits for `value`, modulo
/// whitespace: only such a source is replaced.
fn writer_form(text: &str, value: &NodeInputValue, by_id: &HashMap<&str, &Node>) -> bool {
    let mut expected = String::new();
    write_input_value(&mut expected, value, by_id);
    if compact(text) == compact(&expected) { return true; }
    expected.clear();
    write_named_input_value(&mut expected, value, true);
    if compact(text) == compact(&expected) { return true; }
    expected.clear();
    write_named_input_value(&mut expected, value, false);
    compact(text) == compact(&expected)
}

/// Source-preserving edits cannot move constructors to satisfy lexical
/// variable order. Fully qualified named references avoid both that ordering
/// constraint and any local binding which shadows the prelude's `wire`.
fn write_named_input_value(out: &mut String, value: &NodeInputValue, qualified: bool) {
    let edge = |out: &mut String, edge: &EdgeRef| {
        out.push_str(if qualified { "mod.flow.wire(" } else { "wire(" });
        write_port_name(out, &edge.from_node);
        out.push_str(", ");
        write_port_name(out, &edge.from_port);
        out.push(')');
    };
    match value {
        NodeInputValue::Literal(value) => write_literal(out, value),
        NodeInputValue::Edge(value) => edge(out, value),
        NodeInputValue::Edges(entries) => {
            out.push('[');
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 { out.push_str(", "); }
                out.push_str(if qualified { "mod.flow.conn(" } else { "conn(" });
                write_number(out, entry.id as f64);
                out.push_str(", ");
                edge(out, &entry.from);
                out.push(')');
            }
            out.push(']');
        }
    }
}

fn patch_node_inputs(
    vm: &mut ScriptVm<'_>,
    flow: ScriptObject,
    source: &str,
    file: &str,
    before: &Graph,
    wanted: &Graph,
    registry: &Registry,
) -> Result<String, WriteError> {
    let changed: Vec<_> = before.nodes.iter().zip(&wanted.nodes)
        .filter(|(old, new)| old.inputs != new.inputs)
        .collect();
    if changed.is_empty() {
        return Ok(source.to_string());
    }
    let old_ids: HashMap<&str, &Node> = before.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut objects = HashMap::new();
    let mut sites: HashMap<(usize, usize), usize> = HashMap::new();
    for node in &before.nodes {
        let object = own_value(vm, flow, &node.id).and_then(|value| value.as_object())
            .ok_or_else(|| layout_error(&node.id, "node has no direct object"))?;
        for level in vm.construction_chain(object.into()) {
            if level.loc().is_some_and(|loc| loc.file() == file) {
                if let Some(span) = object_span(vm, level.object(), source, file) {
                    *sites.entry(span).or_default() += 1;
                }
            }
        }
        objects.insert(node.id.as_str(), object);
    }
    let mut patches: Vec<(usize, usize, String)> = Vec::new();
    for (old, new) in &changed {
        let object = objects[old.id.as_str()];
        vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(object))
            .filter(|loc| loc.file() == file)
            .ok_or_else(|| layout_error(&old.id, "its constructor is not in this graph's source"))?;
        let span = object_span(vm, object, source, file)
            .ok_or_else(|| layout_error(&old.id, "cannot locate its constructor"))?;
        if sites.get(&span) != Some(&1) || !direct_layout_constructor(source, span.0) {
            return Err(layout_error(&old.id, "wiring needs a unique top-level `let name = Type{...}` constructor; computed or shared constructors remain source-owned"));
        }
        let descriptor = registry.descriptor(&old.type_name);
        let fields = field_sources(source, span);
        let edits_map = old.inputs.iter().zip(&new.inputs).any(|(old_input, new_input)| {
            old_input.value != new_input.value && (old.kind == "component"
                || descriptor.is_some_and(|descriptor| descriptor.codec == NodeCodec::Fn || descriptor.control_input(&old_input.port).is_some()))
        });
        let in_map = edits_map.then(|| fields.get("in").copied().map(|range| layout_value_range(source, range))).flatten();
        if let Some((start, end)) = in_map {
            if source.as_bytes().get(start) != Some(&b'{') || matching_delimiter(source, start, b'{', b'}') != Some(end - 1) {
                return Err(layout_error(&old.id, "its `in` is an expression, not an editable literal map"));
            }
        }
        let in_fields = in_map.map(|range| field_sources(source, range)).unwrap_or_default();
        let mut field_inserts = Vec::new();
        let mut map_inserts = Vec::new();
        for (old_input, new_input) in old.inputs.iter().zip(&new.inputs) {
            if old_input.value == new_input.value { continue; }
            let port = &old_input.port;
            let mut text = String::new();
            write_named_input_value(&mut text, &new_input.value, true);
            let mapped = old.kind == "component"
                || descriptor.is_some_and(|descriptor| descriptor.codec == NodeCodec::Fn || descriptor.control_input(port).is_some());
            let existing = if mapped { in_fields.get(port) } else { fields.get(port) };
            if let Some(range) = existing.copied() {
                let range = layout_value_range(source, range);
                if !writer_form(&source[range.0..range.1], &old_input.value, &old_ids) {
                    return Err(layout_error(&old.id, &format!("`{port}` is an expression, not an editable wire or literal")));
                }
                patches.push((range.0, range.1, text));
            } else if mapped {
                if in_map.is_none() && deep_value(vm, object, "in").is_some_and(|value| !value.is_nil()) {
                    return Err(layout_error(&old.id, "its `in` is not written in its constructor"));
                }
                map_inserts.push(format!("{port}: {text}"));
            } else {
                if own_value(vm, object, port).is_some() {
                    return Err(layout_error(&old.id, &format!("`{port}` was assigned outside its constructor")));
                }
                field_inserts.push(format!("{port}: {text}"));
            }
        }
        if !map_inserts.is_empty() {
            match in_map {
                Some((start, end)) => {
                    let inner = &source[start + 1..end - 1];
                    let entries = map_inserts.join("  ");
                    if inner.trim().is_empty() {
                        patches.push((start + 1, end - 1, entries));
                    } else {
                        // Begin on a fresh line so a trailing line comment
                        // remains a comment rather than swallowing the edit.
                        patches.push((end - 1, end - 1, format!("\n    {entries}\n")));
                    }
                }
                None => field_inserts.push(format!("in: {{{}}}", map_inserts.join("  "))),
            }
        }
        if !field_inserts.is_empty() {
            let close = span.1 - 1;
            let line = source[..close].rfind('\n').map_or(0, |index| index + 1);
            let prefix = &source[line..close];
            let (position, indentation, mut text) = if prefix.bytes().all(|byte| matches!(byte, b' ' | b'\t' | b'\r')) {
                (line, prefix, String::new())
            } else {
                (close, "", "\n".to_string())
            };
            for field in field_inserts {
                text.push_str(indentation);
                text.push_str("    ");
                text.push_str(&field);
                text.push('\n');
            }
            patches.push((position, position, text));
        }
    }
    patches.sort_by_key(|patch| (patch.0, patch.1));
    if patches.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(WriteError::Invalid("wire edits overlap; constructors are ambiguous".into()));
    }
    let mut written = source.to_string();
    for (start, end, text) in patches.into_iter().rev() {
        written.replace_range(start..end, &text);
    }
    Ok(written)
}

/// Append new nodes to the root graph and pinned component graphs without
/// rewriting existing source. `wanted` must be the evaluated graph with new
/// nodes only at the end of each `nodes` list: every existing node, input,
/// parameter, layout, tool, flow field and component interface stays exactly
/// as evaluated. Each new node is written as a fully qualified
/// `let id = mod.flow.Type{...}` just before the file's final top-level
/// `Flow{...}` / `Component{...}` literal, and its id is appended to that
/// literal's membership on a fresh line. Computed, returned or non-final
/// flow literals are refused, never canonicalized. The root and every
/// module evaluate again; the result must equal `wanted` apart from
/// locations, revision, warnings and derived hashes, which are recomputed
/// here, never taken from `wanted`. Evaluates twice; call it off the UI thread.
pub fn patch_add_nodes_closure_with_registry(
    source: &str,
    wanted: &Graph,
    closure: &Closure,
    registry: &Registry,
) -> Result<(String, Closure), WriteError> {
    const FILE: &str = "<flow-add-edit>";
    let (mut loaded, before) = FlowVm::load_with_registry(source, FILE, closure, registry)
        .map_err(layout_eval_error)?;
    if before.components.len() != wanted.components.len()
        || before.components.iter().zip(&wanted.components).any(|(old, new)| {
            old.alias != new.alias || old.path != new.path || old.hash != new.hash
                || old.inputs != new.inputs || old.outputs != new.outputs
        })
    {
        return Err(WriteError::Invalid("component identities, interfaces or pinned hashes changed; reload the current graph before adding nodes".into()));
    }
    append_only(&before, wanted, false)?;
    for (old, new) in before.components.iter().zip(&wanted.components) {
        append_only(&old.graph, &new.graph, true)
            .map_err(|error| WriteError::Invalid(format!("component `{}`: {error}", old.alias)))?;
    }
    if wanted.nodes.len() == before.nodes.len()
        && before.components.iter().zip(&wanted.components).all(|(old, new)| old.graph.nodes.len() == new.graph.nodes.len())
    {
        return Err(WriteError::Invalid("no node to add".into()));
    }
    validate_projection_with_registry(wanted, &wanted.components, registry).map_err(WriteError::Invalid)?;
    let flow = loaded.flow.as_object();
    let mut modules = closure.modules.clone();
    let written = loaded.with_vm(|vm| -> Result<String, WriteError> {
        let written = append_nodes(vm, flow, source, FILE, &before, wanted, registry, &["Flow", "mod.flow.Flow"])?;
        let component_module = own_value(vm, vm.modules(), COMPONENTS_MODULE)
            .and_then(|value| value.as_object());
        for (old, new) in before.components.iter().zip(&wanted.components) {
            if old.graph.nodes.len() == new.graph.nodes.len() { continue; }
            let module = modules.iter_mut().find(|module| module.alias == old.alias)
                .ok_or_else(|| WriteError::Invalid(format!("component `{}` has no authoritative source", old.alias)))?;
            let object = component_module.and_then(|components| own_value(vm, components, &old.alias))
                .and_then(|value| value.as_object())
                .ok_or_else(|| WriteError::Invalid(format!("component `{}` has no evaluated declaration", old.alias)))?;
            module.source = append_nodes(vm, object, &module.source, &module.path, &old.graph, &new.graph, registry,
                &["Component", "mod.flow.Component"])
                .map_err(|error| WriteError::Invalid(format!("{}: {error}", module.path)))?;
            module.hash = crate::document::closure::source_hash(&module.source);
        }
        Ok(written)
    })?;
    let closure = Closure::ordered(modules).map_err(layout_eval_error)?;
    let after = evaluate_with_registry(&written, FILE, &closure, registry).map_err(layout_eval_error)?;
    if before.components.len() != after.components.len()
        || before.components.iter().zip(&after.components)
            .any(|(old, new)| old.alias != new.alias || old.inputs != new.inputs || old.outputs != new.outputs)
    {
        return Err(WriteError::Invalid("adding these nodes would change a component's interface; edit its source".into()));
    }
    if addition_projection(wanted) != addition_projection(&after) {
        return Err(WriteError::Invalid("the added nodes do not evaluate as requested; the flow's code or membership is computed".into()));
    }
    Ok((written, closure))
}

/// Remove root nodes without rewriting the rest of the source: each node's
/// unique top-level `let id = Type{...}` statement and its entry in the
/// final literal `Flow{...}` membership are deleted, and nothing else.
/// Wires between removed nodes go with their constructors. Refused while
/// anything outside the removed set still reads a removed node (a wire, a
/// custom tool, an interface node or any other source mention), and when a
/// constructor or the membership is computed, shared or not the writer's
/// plain form. The root and its modules evaluate again; the result must
/// equal the accepted graph without those nodes. Component graphs and their
/// modules are untouched. Evaluates twice; call it off the UI thread.
pub fn patch_remove_nodes_closure_with_registry(
    source: &str,
    remove: &[String],
    closure: &Closure,
    registry: &Registry,
) -> Result<String, WriteError> {
    const FILE: &str = "<flow-remove-edit>";
    let (mut loaded, before) = FlowVm::load_with_registry(source, FILE, closure, registry)
        .map_err(layout_eval_error)?;
    if remove.is_empty() {
        return Err(WriteError::Invalid("no node to remove".into()));
    }
    let removed: HashSet<&str> = remove.iter().map(String::as_str).collect();
    if removed.len() != remove.len() {
        return Err(WriteError::Invalid("a node is named twice".into()));
    }
    for id in remove {
        let node = before.nodes.iter().find(|node| &node.id == id)
            .ok_or_else(|| layout_error(id, "no such root node"))?;
        if matches!(node.kind.as_str(), "input" | "output") {
            return Err(layout_error(id, "removing it would change the flow's interface; edit its source"));
        }
    }
    if let Some(edge) = before.edges.iter().find(|edge| removed.contains(edge.from_node.as_str()) && !removed.contains(edge.to_node.as_str())) {
        return Err(layout_error(&edge.to_node, &format!("reads `{}.{}`; disconnect it first", edge.from_node, edge.from_port)));
    }
    if let Some(tool) = before.tools.iter().find(|tool| tool.name != "run" && tool.nodes.iter().any(|node| removed.contains(node.as_str()))) {
        return Err(WriteError::Invalid(format!("tool `{}` selects a removed node; edit its source", tool.name)));
    }
    let mut expected = before.clone();
    expected.nodes.retain(|node| !removed.contains(node.id.as_str()));
    expected.edges.retain(|edge| !removed.contains(edge.from_node.as_str()) && !removed.contains(edge.to_node.as_str()));
    let flow = loaded.flow.as_object();
    let written = loaded.with_vm(|vm| remove_nodes(vm, flow, source, FILE, &before, &removed))?;
    let after = evaluate_with_registry(&written, FILE, closure, registry).map_err(layout_eval_error)?;
    if addition_projection(&expected) != addition_projection(&after) {
        return Err(WriteError::Invalid("the flow does not evaluate without these nodes as requested; its code or membership is computed".into()));
    }
    if let Some(id) = remove.iter().find(|id| source_mentions(&written, id)) {
        return Err(layout_error(id, "the source still mentions it; edit its source"));
    }
    Ok(written)
}

fn remove_nodes(
    vm: &mut ScriptVm<'_>,
    flow: ScriptObject,
    source: &str,
    file: &str,
    before: &Graph,
    removed: &HashSet<&str>,
) -> Result<String, WriteError> {
    vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(flow))
        .filter(|loc| loc.file() == file)
        .ok_or_else(|| WriteError::Invalid("the flow literal is not constructed in this source".into()))?;
    let span = object_span(vm, flow, source, file)
        .ok_or_else(|| WriteError::Invalid("cannot locate the flow literal".into()))?;
    terminal_literal(source, span, &["Flow", "mod.flow.Flow"])?;
    let members = literal_membership(source, span, before)?;
    // Constructor sites of every node: a shared or looped constructor is
    // not one node's source.
    let mut sites: HashMap<(usize, usize), usize> = HashMap::new();
    let mut objects = HashMap::new();
    for node in &before.nodes {
        let object = own_value(vm, flow, &node.id).and_then(|value| value.as_object())
            .ok_or_else(|| layout_error(&node.id, "node has no direct object"))?;
        for level in vm.construction_chain(object.into()) {
            if level.loc().is_some_and(|loc| loc.file() == file) {
                if let Some(span) = object_span(vm, level.object(), source, file) {
                    *sites.entry(span).or_default() += 1;
                }
            }
        }
        objects.insert(node.id.as_str(), object);
    }
    let bytes = source.as_bytes();
    let mut cuts: Vec<(usize, usize, String)> = Vec::new();
    for id in before.nodes.iter().map(|node| node.id.as_str()).filter(|id| removed.contains(id)) {
        let object = objects[id];
        vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(object))
            .filter(|loc| loc.file() == file)
            .ok_or_else(|| layout_error(id, "its constructor is not in this graph's source"))?;
        let constructor = object_span(vm, object, source, file)
            .ok_or_else(|| layout_error(id, "cannot locate its constructor"))?;
        if sites.get(&constructor) != Some(&1) || !direct_layout_constructor(source, constructor.0) {
            return Err(layout_error(id, "removal needs a unique top-level `let name = Type{...}` constructor; computed or shared constructors remain source-owned"));
        }
        // The statement is whole lines: `let id = Type{...}` from its own
        // line's start, then only blanks or a line comment to the line's end.
        let line = source[..constructor.0].rfind('\n').map_or(0, |index| index + 1);
        let mut head = source[line..constructor.0].split_whitespace();
        if head.next() != Some("let") || head.next() != Some(id) || head.next() != Some("=") {
            return Err(layout_error(id, "its constructor does not start its own line as `let name = Type{...}`"));
        }
        let mut end = constructor.1;
        while end < bytes.len() && matches!(bytes[end], b' ' | b'\t' | b'\r') { end += 1; }
        if bytes.get(end) == Some(&b'/') && bytes.get(end + 1) == Some(&b'/') { end = skip_line_comment(bytes, end); }
        match bytes.get(end) {
            None => {}
            Some(b'\n') => end += 1,
            Some(_) => return Err(layout_error(id, "code follows its constructor on the same line")),
        }
        cuts.push((line, end, String::new()));
    }
    // Membership: each line holding a removed member keeps its other
    // members, separator style and trailing comment; a line left empty goes.
    let members_start = members.first().map(|member| member.0)
        .ok_or_else(|| WriteError::Invalid("the flow has no membership to edit".into()))?;
    // (segment start, its line's start, segment end): a segment never
    // reaches before the membership nor past the literal's closing brace.
    let mut lines: Vec<(usize, usize, usize)> = Vec::new();
    for &(start, end) in &members {
        if !removed.contains(&source[start..end]) { continue; }
        let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
        let stop = source[end..].find('\n').map_or(bytes.len(), |index| end + index).min(span.1 - 1);
        let segment = (line_start.max(members_start), line_start, stop);
        if lines.last() != Some(&segment) { lines.push(segment); }
    }
    for (line, line_start, stop) in lines {
        let segment = &source[line..stop];
        let body_end = segment.find("//").unwrap_or(segment.len());
        let (body, comment) = segment.split_at(body_end);
        if body.contains("/*") {
            return Err(WriteError::Invalid("a block comment sits among the removed members; edit the membership in source".into()));
        }
        let kept: Vec<&str> = body.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|word| !word.is_empty() && !removed.contains(word))
            .collect();
        let whole = line == line_start && bytes.get(stop) == Some(&b'\n');
        let indent: String = body.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let comma = body.trim_end().ends_with(',');
        let mut text = String::new();
        if !kept.is_empty() {
            text.push_str(&indent);
            text.push_str(&kept.join(", "));
            if comma { text.push(','); }
            if !comment.is_empty() { text.push(' '); }
        } else if !comment.is_empty() {
            text.push_str(&indent);
        }
        text.push_str(comment);
        if text.trim().is_empty() && whole {
            cuts.push((line, stop + 1, String::new()));
        } else {
            cuts.push((line, stop, text));
        }
    }
    cuts.sort_by_key(|cut| (cut.0, cut.1));
    if cuts.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(WriteError::Invalid("removed constructors and membership overlap; edit the source".into()));
    }
    let mut written = source.to_string();
    for (start, end, text) in cuts.into_iter().rev() {
        written.replace_range(start..end, &text);
    }
    Ok(written)
}

/// The reserved full-flow `run` tool is derived by extraction from membership,
/// including newly inserted nodes. Custom tool selections remain exact.
fn addition_projection(graph: &Graph) -> Graph {
    let mut graph = inputs_projection(graph, false, true);
    fn refresh_run(graph: &mut Graph) {
        for tool in graph.tools.iter_mut().filter(|tool| tool.name == "run") {
            tool.nodes = graph.nodes.iter().map(|node| node.id.clone()).collect();
            tool.inputs = graph.nodes.iter().filter(|node| node.kind == "input")
                .map(|node| node.id.clone()).collect();
            tool.outputs = graph.nodes.iter().filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
                .map(|node| node.id.clone()).collect();
        }
        for component in &mut graph.components { refresh_run(&mut component.graph); }
    }
    refresh_run(&mut graph);
    graph
}

/// `wanted` is `before` followed by new nodes, and nothing else changed.
fn append_only(before: &Graph, wanted: &Graph, component: bool) -> Result<(), WriteError> {
    if wanted.nodes.len() < before.nodes.len() {
        return Err(WriteError::Invalid("nodes can only be appended here".into()));
    }
    let mut prefix = wanted.clone();
    prefix.nodes.truncate(before.nodes.len());
    // Pinned component graphs are compared through their own entries.
    let strip = |graph: &Graph| {
        let mut graph = addition_projection(graph);
        graph.components.clear();
        graph
    };
    if strip(before) != strip(&prefix) {
        return Err(WriteError::Invalid("adding nodes must leave every existing node, wire and flow field unchanged".into()));
    }
    for node in &wanted.nodes[before.nodes.len()..] {
        if component && matches!(node.kind.as_str(), "input" | "output") {
            return Err(WriteError::Invalid(format!("`{}` would change the component's interface", node.id)));
        }
    }
    Ok(())
}

fn append_nodes(
    vm: &mut ScriptVm<'_>,
    flow: ScriptObject,
    source: &str,
    file: &str,
    before: &Graph,
    wanted: &Graph,
    registry: &Registry,
    heads: &[&str],
) -> Result<String, WriteError> {
    let added = &wanted.nodes[before.nodes.len()..];
    if added.is_empty() {
        return Ok(source.to_string());
    }
    vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(flow))
        .filter(|loc| loc.file() == file)
        .ok_or_else(|| WriteError::Invalid("the flow literal is not constructed in this source".into()))?;
    let span = object_span(vm, flow, source, file)
        .ok_or_else(|| WriteError::Invalid("cannot locate the flow literal".into()))?;
    let line = terminal_literal(source, span, heads)?;
    literal_membership(source, span, before)?;
    let mut ids = HashSet::new();
    for node in added {
        if !plain_identifier(&node.id) || !ids.insert(node.id.as_str()) || source_mentions(source, &node.id)
            || before.nodes.iter().any(|old| old.id == node.id)
        {
            return Err(layout_error(&node.id, "a new node needs a fresh identifier that no source binding or word uses"));
        }
        if node.kind != "component" && registry.descriptor(&node.type_name).is_none() {
            return Err(WriteError::Unregistered(node.type_name.clone()));
        }
    }
    let by_id: HashMap<&str, &Node> = wanted.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut constructors = String::new();
    for (offset, node) in added.iter().enumerate() {
        write_node_mode(&mut constructors, node, before.nodes.len() + offset, &by_id, registry, true);
    }
    let members = added.iter().map(|node| node.id.as_str()).collect::<Vec<_>>().join(", ");
    let close = span.1 - 1;
    let close_line = source[..close].rfind('\n').map_or(0, |index| index + 1);
    let (at, text) = if close_line > span.0 && source[close_line..close].bytes().all(|byte| matches!(byte, b' ' | b'\t' | b'\r')) {
        (close_line, format!("    {members}\n"))
    } else {
        (close, format!("\n    {members}\n"))
    };
    let mut written = source.to_string();
    written.insert_str(at, &text);
    written.insert_str(line, &constructors);
    Ok(written)
}

/// The start of the line of a final, top-level `Flow{...}` (or `heads`)
/// literal: nothing but whitespace and comments may follow it, and its head
/// must open its own line outside any bracket.
fn terminal_literal(source: &str, span: (usize, usize), heads: &[&str]) -> Result<usize, WriteError> {
    let refuse = |message: &str| WriteError::Invalid(format!("{message}; add the node in source"));
    let bytes = source.as_bytes();
    let mut index = span.1;
    while index < bytes.len() {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() => index += 1,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index).ok_or_else(|| refuse("an unterminated comment follows the flow"))?;
            }
            _ => return Err(refuse("the flow literal is not the source's final expression")),
        }
    }
    let line = source[..span.0].rfind('\n').map_or(0, |index| index + 1);
    if !heads.contains(&source[line..span.0].trim()) {
        return Err(refuse("the flow membership is not a direct top-level literal"));
    }
    let mut index = 0;
    let mut depth = 0usize;
    while index < line {
        match bytes[index] {
            b'"' => { index = skip_string(bytes, index).ok_or_else(|| refuse("unterminated string"))?; continue; }
            b'/' if bytes.get(index + 1) == Some(&b'/') => { index = skip_line_comment(bytes, index); continue; }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index).ok_or_else(|| refuse("unterminated comment"))?;
                continue;
            }
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        index += 1;
    }
    if depth != 0 {
        return Err(refuse("the flow literal is nested in code"));
    }
    Ok(line)
}

/// The editable membership is a suffix of direct node identifiers. Earlier
/// metadata and surrounding construction code keep their original bytes;
/// computed membership, aliases and statements inside the literal stay
/// source-owned. This also prevents an insertion from silently appending to
/// a loop-built collection whose current expansion happens to be empty.
fn literal_membership(source: &str, span: (usize, usize), graph: &Graph) -> Result<Vec<(usize, usize)>, WriteError> {
    let refuse = || WriteError::Invalid("the flow membership is computed or aliased; add the node in source".into());
    let bytes = source.as_bytes();
    let mut index = span.0 + 1;
    let end = span.1 - 1;
    let mut members = None;
    while index < end {
        match bytes[index] {
            b'"' => { index = skip_string(bytes, index).ok_or_else(refuse)?; continue; }
            b'/' if bytes.get(index + 1) == Some(&b'/') => { index = skip_line_comment(bytes, index); continue; }
            b'/' if bytes.get(index + 1) == Some(&b'*') => { index = skip_block_comment(bytes, index).ok_or_else(refuse)?; continue; }
            b'{' | b'[' | b'(' => {
                let right = match bytes[index] { b'{' => b'}', b'[' => b']', _ => b')' };
                index = matching_delimiter(source, index, bytes[index], right).ok_or_else(refuse)? + 1;
                continue;
            }
            byte if is_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < end && is_ident_continue(bytes[index]) { index += 1; }
                let word = &source[start..index];
                if matches!(word, "for" | "if" | "while" | "loop" | "let" | "return" | "do" | "match") { return Err(refuse()); }
                if graph.nodes.first().is_some_and(|node| node.id == word) {
                    members = Some(start);
                    break;
                }
                continue;
            }
            _ => index += 1,
        }
    }
    if graph.nodes.is_empty() { return Ok(Vec::new()); }
    let mut index = members.ok_or_else(refuse)?;
    let mut found = Vec::new();
    while index < end {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() || byte == b',' => index += 1,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => index = skip_block_comment(bytes, index).ok_or_else(refuse)?,
            byte if is_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < end && is_ident_continue(bytes[index]) { index += 1; }
                found.push((start, index));
            }
            _ => return Err(refuse()),
        }
    }
    if found.iter().map(|&(start, end)| &source[start..end]).ne(graph.nodes.iter().map(|node| node.id.as_str())) { return Err(refuse()); }
    Ok(found)
}

fn plain_identifier(name: &str) -> bool {
    name.len() <= 128
        && name.bytes().next().is_some_and(is_ident_start)
        && name.bytes().all(is_ident_continue)
        && !matches!(name, "let" | "fn" | "if" | "else" | "for" | "in" | "while" | "loop" | "return" | "break"
            | "continue" | "true" | "false" | "nil" | "use" | "mod" | "me" | "self" | "Flow" | "Component" | "Node")
}

/// Whether `name` occurs anywhere in `source` as a whole word: a
/// conservative stand-in for "no binding of that name exists".
fn source_mentions(source: &str, name: &str) -> bool {
    let bytes = source.as_bytes();
    source.match_indices(name).any(|(at, _)| {
        let end = at + name.len();
        !(at > 0 && is_ident_continue(bytes[at - 1])) && !(end < bytes.len() && is_ident_continue(bytes[end]))
    })
}

fn closure_layout_projection(graph: &Graph, without_layout: bool, without_hashes: bool) -> Graph {
    let mut graph = layout_projection(graph, without_layout);
    for component in &mut graph.components {
        if without_hashes { component.hash.clear(); }
        component.graph = closure_layout_projection(&component.graph, without_layout, without_hashes);
    }
    graph
}

fn layout_changed(before: &Graph, wanted: &Graph) -> bool {
    before.nodes.iter().zip(&wanted.nodes)
        .any(|(old, new)| old.at != new.at || old.size != new.size || old.flip != new.flip)
}

fn patch_node_layout(
    vm: &mut ScriptVm<'_>,
    flow: ScriptObject,
    source: &str,
    file: &str,
    before: &Graph,
    wanted: &Graph,
) -> Result<String, WriteError> {
    let changed: Vec<_> = before.nodes.iter().zip(&wanted.nodes)
        .filter(|(old, new)| old.at != new.at || old.size != new.size || old.flip != new.flip)
        .collect();
    if changed.is_empty() {
        return Ok(source.to_string());
    }
    let mut patches = {
        let mut objects = HashMap::new();
        let mut sites: HashMap<(usize, usize), usize> = HashMap::new();
        // A constructor used by another node, including as its prototype,
        // is shared source. Never alter it on behalf of just one node.
        for node in &before.nodes {
            let object = own_value(vm, flow, &node.id).and_then(|value| value.as_object())
                .ok_or_else(|| layout_error(&node.id, "node has no direct root object"))?;
            for level in vm.construction_chain(object.into()) {
                if level.loc().is_some_and(|loc| loc.file() == file) {
                    if let Some(span) = object_span(vm, level.object(), source, file) {
                        *sites.entry(span).or_default() += 1;
                    }
                }
            }
            objects.insert(node.id.as_str(), object);
        }
        let mut patches = Vec::new();
        for (old, new) in &changed {
            let object = objects[old.id.as_str()];
            vm.bx.code_ref().ip_to_loc(vm.bx.heap_ref().made_at(object))
                .filter(|loc| loc.file() == file)
                .ok_or_else(|| layout_error(&old.id, "its constructor is not in this graph's source"))?;
            let span = object_span(vm, object, source, file)
                .ok_or_else(|| layout_error(&old.id, "cannot locate its constructor"))?;
            if sites.get(&span) != Some(&1) || !direct_layout_constructor(source, span.0) {
                return Err(layout_error(&old.id, "layout needs a unique top-level `let name = Type{...}` constructor; computed or shared constructors remain source-owned"));
            }
            let fields = field_sources(source, span);
            let mut insert = Vec::new();
            for (name, differs) in [("at", old.at != new.at), ("size", old.size != new.size), ("flip", old.flip != new.flip)] {
                if !differs { continue; }
                let replacement = match name {
                    "flip" => if new.flip { "true".to_string() } else { "false".to_string() },
                    _ => match if name == "at" { new.at } else { new.size } {
                        Some((x, y)) if x.is_finite() && y.is_finite() => {
                            let mut text = "mod.flow.vec2(".to_string();
                            write_number(&mut text, x);
                            text.push_str(", ");
                            write_number(&mut text, y);
                            text.push(')');
                            text
                        }
                        Some(_) => return Err(layout_error(&old.id, &format!("{name} must contain finite numbers"))),
                        None if name == "size" => "nil".to_string(),
                        None => return Err(layout_error(&old.id, "remove an authored position in source; dragging supplies a new position")),
                    },
                };
                if let Some(range) = fields.get(name).copied() {
                    let range = layout_value_range(source, range);
                    let value = &source[range.0..range.1];
                    let literal = match name {
                        "flip" => matches!(value, "true" | "false"),
                        "size" if value == "nil" => true,
                        _ => parse_vec2(value).is_some_and(|(x, y)| x.is_finite() && y.is_finite()),
                    };
                    if !literal {
                        return Err(layout_error(&old.id, &format!("{name} is an expression, not an editable literal")));
                    }
                    patches.push((range.0, range.1, replacement));
                } else {
                    if own_value(vm, object, name).is_some() {
                        return Err(layout_error(&old.id, &format!("{name} was assigned outside its constructor")));
                    }
                    // An own override is safe even when a prototype supplies
                    // the inherited value: that prototype is left untouched.
                    insert.push(format!("{name}: {replacement}"));
                }
            }
            if !insert.is_empty() {
                let close = span.1 - 1;
                let line = source[..close].rfind('\n').map_or(0, |index| index + 1);
                let prefix = &source[line..close];
                let (position, indentation, mut text) = if prefix.bytes().all(|byte| matches!(byte, b' ' | b'\t' | b'\r')) {
                    (line, prefix, String::new())
                } else {
                    (close, "", "\n".to_string())
                };
                for field in insert {
                    text.push_str(indentation);
                    text.push_str("    ");
                    text.push_str(&field);
                    text.push('\n');
                }
                patches.push((position, position, text));
            }
        }
        patches
    };
    patches.sort_by_key(|patch| (patch.0, patch.1));
    if patches.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(WriteError::Invalid("layout edits overlap; constructors are ambiguous".into()));
    }
    let mut written = source.to_string();
    for (start, end, text) in patches.into_iter().rev() {
        written.replace_range(start..end, &text);
    }
    Ok(written)
}

fn layout_error(node: &str, message: &str) -> WriteError {
    WriteError::Invalid(format!("`{node}`: {message}"))
}

fn layout_eval_error(error: EvalError) -> WriteError {
    WriteError::Invalid(format!("{}:{}:{}: {}", error.file, error.line, error.col, error.message))
}

fn layout_projection(graph: &Graph, without_layout: bool) -> Graph {
    let mut graph = graph.clone();
    graph.revision = 0;
    graph.warnings.clear();
    for node in &mut graph.nodes {
        node.loc = Loc::default();
        if without_layout {
            node.at = None;
            node.size = None;
            node.flip = false;
        }
    }
    graph
}

/// Recognize direct root constructors without treating a function body's
/// or loop's repeatedly executed literal as one editable source instance.
fn direct_layout_constructor(source: &str, open: usize) -> bool {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut depth = 0usize;
    let mut tokens = Vec::new();
    while index < open {
        match bytes[index] {
            b'"' => {
                let Some(end) = skip_string(bytes, index) else { return false };
                if depth == 0 { tokens.push("<string>"); }
                index = end;
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => { index = skip_line_comment(bytes, index); continue; }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let Some(end) = skip_block_comment(bytes, index) else { return false };
                index = end;
                continue;
            }
            b'{' | b'(' | b'[' => { depth += 1; }
            b'}' | b')' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 { tokens.clear(); }
            }
            byte if depth == 0 && is_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < open && is_ident_continue(bytes[index]) { index += 1; }
                let token = &source[start..index];
                if token == "let" { tokens.clear(); }
                tokens.push(token);
                continue;
            }
            byte if depth == 0 && !byte.is_ascii() => {
                tokens.push("<non-ascii>");
                index += source[index..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            byte if depth == 0 && !byte.is_ascii_whitespace() => {
                if byte == b';' { tokens.clear(); } else { tokens.push(&source[index..index + 1]); }
            }
            _ => {}
        }
        index += 1;
    }
    depth == 0 && tokens.len() >= 4 && tokens[0] == "let" && tokens[2] == "="
        && tokens[1].bytes().next().is_some_and(is_ident_start)
        && tokens[3..].iter().enumerate().all(|(index, token)| {
            if index % 2 == 1 { *token == "." } else { token.bytes().next().is_some_and(is_ident_start) }
        }) && tokens.len() % 2 == 0
}

pub fn write(graph: &Graph) -> Result<String, WriteError> {
    write_with_registry(graph, &Registry::with_builtins())
}

pub fn write_with_registry(graph: &Graph, registry: &Registry) -> Result<String, WriteError> {
    for node in &graph.nodes {
        if node.kind != "component" && registry.descriptor(&node.type_name).is_none() {
            return Err(WriteError::Unregistered(node.type_name.clone()));
        }
    }
    validate_projection_with_registry(graph, &graph.components, registry).map_err(WriteError::Invalid)?;
    let order = topological_order(graph);
    let by_id: HashMap<_, _> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut out = String::from("use mod.flow.*\n");
    if graph.nodes.iter().any(|node| node.kind == "component") {
        out.push_str("use mod.components.*\n");
    }
    out.push('\n');
    for (position, id) in order.iter().enumerate() {
        write_node(&mut out, by_id[id.as_str()], position, &by_id, registry);
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
    Ok(out)
}

fn write_node(
    out: &mut String,
    node: &Node,
    position: usize,
    by_id: &HashMap<&str, &Node>,
    registry: &Registry,
) {
    write_node_mode(out, node, position, by_id, registry, false);
}

/// `named`: a source-preserving insertion. The type is fully qualified and
/// wires are `mod.flow.wire`/`mod.flow.conn`, safe before or after their
/// source's declaration and whatever the file imports.
fn write_node_mode(
    out: &mut String,
    node: &Node,
    position: usize,
    by_id: &HashMap<&str, &Node>,
    registry: &Registry,
    named: bool,
) {
    if let Some(doc) = &node.doc {
        out.push_str("/** ");
        out.push_str(&doc.replace("*/", "* /"));
        out.push_str(" */\n");
    }
    out.push_str("let ");
    out.push_str(&node.id);
    out.push_str(" = ");
    if named {
        out.push_str(if node.kind == "component" { "mod.components." } else { "mod.flow." });
    }
    out.push_str(&node.type_name);
    out.push_str("{\n");

    if node.kind == "component" {
        if !node.inputs.is_empty() {
            out.push_str("    in: {");
            for (index, input) in node.inputs.iter().enumerate() {
                if index > 0 { out.push_str("  "); }
                out.push_str(&input.port);
                out.push_str(": ");
                write_value(out, &input.value, by_id, named);
            }
            out.push_str("}\n");
        }
    } else {
        let descriptor = registry.descriptor(&node.type_name).expect("writer validated the descriptor");
        for field in &descriptor.write {
            match field {
                WriteField::Param { name, force } => write_param(out, node, name, *force, descriptor),
                WriteField::Input { name, force } => write_input(out, node, name, *force, by_id, descriptor, named),
                WriteField::Ports => write_ports(out, node),
                WriteField::Domain => {
                    out.push_str("    domain: ");
                    write_string(out, node.domain.as_deref().unwrap_or(""));
                    out.push('\n');
                }
                WriteField::Params => for (name, _) in &node.params {
                    write_param(out, node, name, true, descriptor);
                },
                WriteField::Inputs => {
                    for input in node.inputs.iter().filter(|input| descriptor.control_input(&input.port).is_none()) {
                        write_input(out, node, &input.port, false, by_id, descriptor, named);
                    }
                    // Wired control inputs; the fallback stays in its param.
                    let wired: Vec<_> = node.inputs.iter()
                        .filter(|input| descriptor.control_input(&input.port).is_some()
                            && !matches!(input.value, NodeInputValue::Literal(Literal::Null)))
                        .collect();
                    if !wired.is_empty() {
                        out.push_str("    in: {");
                        for (index, input) in wired.iter().enumerate() {
                            if index > 0 { out.push_str("  "); }
                            out.push_str(&input.port);
                            out.push_str(": ");
                            write_value(out, &input.value, by_id, named);
                        }
                        out.push_str("}\n");
                    }
                }
                WriteField::FnBody => {
            out.push_str("    in: {");
            for (index, input) in node.inputs.iter().enumerate() {
                if index > 0 {
                    out.push_str("  ");
                }
                out.push_str(&input.port);
                out.push_str(": ");
                match input.many {
                    // An Fn declares its ordered inputs itself: the element
                    // type and the bound travel with the entries, so the
                    // declaration survives an empty entry list.
                    Some(bound) => {
                        out.push_str(if named { "mod.flow.ordered(" } else { "ordered(" });
                        write_type(out, &input.ty);
                        out.push_str(", ");
                        write_number(out, bound as f64);
                        out.push_str(", ");
                        write_value(out, &input.value, by_id, named);
                        out.push(')');
                    }
                    None => write_value(out, &input.value, by_id, named),
                }
            }
            out.push_str("}\n");
            out.push_str("    out: [");
            for (index, port) in node.outputs.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_port_name(out, &port.name);
            }
            out.push_str("]\n");
            out.push_str("    run: ");
            out.push_str(node.fn_src.as_deref().unwrap_or("|i| { i }").trim());
            out.push('\n');

                }
            }
        }
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
    out.push_str(if named { "    at: mod.flow.vec2(" } else { "    at: vec2(" });
    write_number(out, at.0);
    out.push_str(", ");
    write_number(out, at.1);
    out.push_str(")\n");
    if let Some(size) = node.size {
        out.push_str(if named { "    size: mod.flow.vec2(" } else { "    size: vec2(" });
        write_number(out, size.0);
        out.push_str(", ");
        write_number(out, size.1);
        out.push_str(")\n");
    }
    if node.flip {
        out.push_str("    flip: true\n");
    }
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
        out.push_str(": ");
        write_type(out, &input.ty);
    }
    out.push_str("}  out: {");
    for (index, port) in node.outputs.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&port.name);
        out.push_str(": ");
        write_type(out, &port.ty);
    }
    out.push_str("} }\n");
}

fn write_param(out: &mut String, node: &Node, name: &str, force: bool, descriptor: &TypeDescriptor) {
    let Some(value) = node
        .params
        .iter()
        .find_map(|(param, value)| (param == name).then_some(value))
    else {
        return;
    };
    let param = descriptor.params.iter().find(|param| param.name == name);
    let default = param.map(|param| &param.default);
    if !force && default == Some(value) {
        return;
    }
    out.push_str("    ");
    out.push_str(name);
    out.push_str(": ");
    let quoted_type = param
        .filter(|param| matches!(param.expected, ParamType::PortTypeNoBytes | ParamType::PortTypeWithBytes | ParamType::HttpOut))
        .and_then(|_| match value {
            Literal::Id(name) | Literal::Str(name) => port_type(name),
            _ => None,
        })
        .filter(|ty| matches!(ty, PortType::Registered(_))
            || matches!(value, Literal::Id(name) if name.contains('.')));
    if let Some(ty) = quoted_type {
        write_type(out, &ty);
    } else {
        write_literal(out, value);
    }
    out.push('\n');
}

fn write_input(
    out: &mut String,
    node: &Node,
    name: &str,
    required: bool,
    by_id: &HashMap<&str, &Node>,
    descriptor: &TypeDescriptor,
    named: bool,
) {
    let Some(input) = node.inputs.iter().find(|input| input.port == name) else {
        return;
    };
    let default = descriptor.inputs.iter().find(|input| input.name == name).map(|input| &input.default);
    if !required {
        if let NodeInputValue::Literal(value) = &input.value {
            if default == Some(value) {
                return;
            }
        }
    }
    out.push_str("    ");
    out.push_str(name);
    out.push_str(": ");
    write_value(out, &input.value, by_id, named);
    out.push('\n');
}

fn write_value(out: &mut String, value: &NodeInputValue, by_id: &HashMap<&str, &Node>, named: bool) {
    if named { write_named_input_value(out, value, true) } else { write_input_value(out, value, by_id) }
}

fn write_input_value(out: &mut String, value: &NodeInputValue, by_id: &HashMap<&str, &Node>) {
    match value {
        NodeInputValue::Literal(value) => write_literal(out, value),
        NodeInputValue::Edge(edge) => write_edge_ref(out, edge, by_id),
        NodeInputValue::Edges(entries) => {
            out.push('[');
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str("conn(");
                write_number(out, entry.id as f64);
                out.push_str(", ");
                write_edge_ref(out, &entry.from, by_id);
                out.push(')');
            }
            out.push(']');
        }
    }
}

fn write_type(out: &mut String, ty: &PortType) {
    match ty {
        PortType::Registered(_) => write_string(out, ty.as_str()),
        _ => { out.push('@'); out.push_str(ty.as_str()); }
    }
}

fn write_port_name(out: &mut String, name: &str) {
    if name.as_bytes().first().is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        out.push('@'); out.push_str(name);
    } else { write_string(out, name); }
}

fn write_edge_ref(out: &mut String, edge: &EdgeRef, by_id: &HashMap<&str, &Node>) {
    out.push_str(&edge.from_node);
    let generic = by_id.get(edge.from_node.as_str())
        .is_some_and(|source| matches!(source.kind.as_str(), "gen" | "component" | "provider"));
    let named = edge.from_port.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if generic || !named {
        let is_fn = by_id.get(edge.from_node.as_str()).is_some_and(|source| source.kind == "fn");
        out.push_str(if is_fn { ".port(" } else { ".out(" });
        write_port_name(out, &edge.from_port);
        out.push(')');
    } else {
        out.push('.'); out.push_str(&edge.from_port); out.push_str("()");
    }
}

pub(super) fn write_literal(out: &mut String, value: &Literal) {
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
    // The upper bound is exclusive: i64::MAX rounds to 2^63 as f64.
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value < -(i64::MIN as f64) {
        out.push_str(&(value as i64).to_string());
    } else {
        out.push_str(&value.to_string());
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
    is_canonical_with(source, &crate::document::closure::Closure::default())
}

/// As [`is_canonical`], evaluating with the definition's module closure.
pub fn is_canonical_with(source: &str, closure: &crate::document::closure::Closure) -> bool {
    is_canonical_with_registry(source, closure, &Registry::with_builtins())
}

pub fn is_canonical_with_registry(source: &str, closure: &crate::document::closure::Closure, registry: &Registry) -> bool {
    let Ok(graph) = evaluate_with_registry(source, "<canonical>", closure, registry) else { return false };
    write_with_registry(&graph, registry).is_ok_and(|written| compact(source) == compact(&written))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_size_round_trips_after_position() {
        let source = r#"use mod.flow.*
let prompt = Input{
    type: @text
    default: "hello"
    at: vec2(12.5, 30)
    size: vec2(420, 180.25)
}
Flow{prompt}
"#;
        let graph = evaluate(source, "<size-round-trip>").unwrap();
        assert_eq!(graph.nodes[0].size, Some((420.0, 180.25)));
        let written = write(&graph).unwrap();
        let at = written.find("    at: vec2(12.5, 30)\n").unwrap();
        let size = written.find("    size: vec2(420, 180.25)\n").unwrap();
        assert!(at < size);
        let reparsed = evaluate(&written, "<size-round-trip-written>").unwrap();
        assert_eq!(reparsed.nodes[0].size, graph.nodes[0].size);
    }

    #[test]
    fn input_value_round_trips_multiline_text() {
        let source = "use mod.flow.*\nlet prompt = Input{value: \"first\\nsecond \\\"quoted\\\"\"}\nFlow{prompt}\n";
        let graph = evaluate(source, "<input-value>").unwrap();
        let written = write(&graph).unwrap();
        assert!(written.contains("    value: \"first\\nsecond \\\"quoted\\\"\"\n"));
        let reparsed = evaluate(&written, "<input-value-written>").unwrap();
        assert_eq!(reparsed.nodes[0].params, graph.nodes[0].params);
    }

    #[test]
    fn legacy_input_default_is_written_as_value() {
        let graph = evaluate(
            "use mod.flow.*\nlet prompt = Text{default: \"legacy\"}\nFlow{prompt}\n",
            "<legacy-input-default>",
        )
        .unwrap();
        let written = write(&graph).unwrap();
        assert!(written.contains("    value: \"legacy\"\n"));
        assert!(!written.contains("    default:"));
    }

    #[test]
    fn flipped_node_round_trips_after_size_and_false_stays_implicit() {
        let source = r#"use mod.flow.*
let prompt = Input{at: vec2(12, 30) size: vec2(420, 180) flip: true}
let plain = Output{at: vec2(500, 30)}
Flow{prompt, plain}
"#;
        let graph = evaluate(source, "<flip-round-trip>").unwrap();
        assert!(graph.nodes[0].flip);
        assert!(!graph.nodes[1].flip);
        let written = write(&graph).unwrap();
        let size = written.find("    size: vec2(420, 180)\n").unwrap();
        let flip = written.find("    flip: true\n").unwrap();
        assert!(size < flip);
        assert_eq!(written.matches("    flip: true\n").count(), 1);
        let reparsed = evaluate(&written, "<flip-round-trip-written>").unwrap();
        assert_eq!(reparsed.nodes[0].flip, graph.nodes[0].flip);
        assert_eq!(reparsed.nodes[1].flip, graph.nodes[1].flip);
    }
}
