use crate::{makepad_draw::*, widget::*};
use super::props::reflect_flat;
use crate::makepad_script::ScriptMod;
use std::collections::HashMap;

pub fn require_target(widget: &WidgetRef) -> Result<(), String> {
    if widget.try_widget_uid().is_none() {
        return Err("widget is unavailable or currently handling an event/draw; apply between events".to_string());
    }
    Ok(())
}

/// The script-defined shader fns of a widget's draw layer — `pixel` and
/// `vertex` when the layer (or a proto in its chain) sets one — as
/// (name, "file:line", source text). What the Shader tab shows under the
/// well and what a code-only prompt rewrites.
pub fn layer_fn_sources(cx: &mut Cx, widget: &WidgetRef, layer: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    let layer_id = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let value = vm.value(source, layer_id);
        if value.as_object().is_none() {
            return;
        }
        // Closest level first: the fn the widget actually runs is the
        // nearest definition up the layer's construction chain (a `+:`
        // merge in the widget's script, else the draw type's own).
        let chain = vm.construction_chain(value);
        for (name, key) in [("pixel", id!(pixel)), ("vertex", id!(vertex))] {
            for lvl in &chain {
                if !lvl.own_keys().contains(&key) {
                    continue;
                }
                let f = vm.value(lvl.object(), key);
                let Some(fobj) = f.as_object() else { break };
                let Some(ip) = vm.heap().fn_script_ip(fobj) else {
                    break;
                };
                if let Some((loc, text)) = vm.fn_source_text(ip) {
                    let base = loc.file().rsplit('/').next().unwrap_or(loc.file()).to_string();
                    out.push((name.to_string(), format!("{base}:{}", loc.line()), text));
                }
                break;
            }
        }
    });
    out
}

/// The bare eval-apply: evaluate a splash chunk with the widget's own
/// `__script_source__` scope and apply it through the ordinary machinery.
/// No diff, no log — [`apply_splash_chunk`] wraps this for user-visible
/// edits; the tweaker's own scaffolding uses it raw. Never for the room the
/// panel takes from the app: that is kept back by the window as it lays the
/// body out (`View::set_child_reserve`), because an apply to the body is a
/// rebuild of it and a reload takes whatever it wrote away again.
pub fn eval_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str) -> Result<(), String> {
    require_target(widget)?;
    let chunk = chunk.trim();
    let body = if chunk.starts_with('{') {
        chunk.to_string()
    } else {
        format!("{{{chunk}}}")
    };
    let code = format!("use mod.prelude.widgets.*\n__script_source__{body};");
    let callsite = chunk_callsite_line(&code);
    let errors = cx.with_vm(|vm| {
        // Install a captured-error sink so parse/apply problems come back to
        // the caller instead of only landing in the log.
        vm.capture_errors();
        let script_mod = ScriptMod::new("", "tweak", "tweak://apply", callsite as usize, 1, &(code), &[]);
        let mut target = widget.clone();
        use crate::makepad_script::traits::ScriptApply;
        target.script_apply_eval(vm, script_mod);
        vm.take_errors()
    });
    if !errors.is_empty() {
        // `tweak://apply:<callsite+n>:<col>` → `line <n>:<col>` of the chunk
        // (chunk line 1 sits on code line 2, right after the `use`).
        let errors: Vec<String> = errors
            .iter()
            .map(|e| relocate_chunk_error(e, callsite))
            .collect();
        return Err(format!("splash error: {}", errors.join("; ")));
    }
    Ok(())
}

fn revert_layer_fns(cx: &mut Cx, widget: &WidgetRef, layer: &str) {
    let sources = layer_fn_sources(cx, widget, layer);
    let mut body = String::new();
    for (name, _loc, src) in &sources {
        let text = cx.get_global_ref::<ShaderEdits>().and_then(|s| s.functions.get(&(widget.widget_uid().0, layer.to_string(), name.to_string())).cloned())
            .unwrap_or_else(|| src.clone());
        body.push_str(&text);
        body.push('\n');
    }
    if body.trim().is_empty() {
        return;
    }
    let _ = makepad_platform::shader_error::take();
    if let Err(error) = eval_chunk(cx, widget, &format!("{layer} +: {{\n{body}}}")) {
        log!("TWEAK revert of {layer} fns failed: {error}");
    }
    if let Some(error) = makepad_platform::shader_error::take() {
        log!("TWEAK revert of {layer} fns did not compile either: {}", terse_shader_error(&error));
    }
}

/// Undo a chunk whose shader failed to compile: an fn chunk puts the
/// layer's fns back (the last text applied from the view or the AI, else
/// the fn as written); a single-property chunk puts the property back.
pub fn revert_failed_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str, before: &[(String, String, bool)]) {
    if chunk.contains("fn") {
        if let Some(open) = chunk.find("+:") {
            let layer = chunk[..open].trim().to_string();
            revert_layer_fns(cx, widget, &layer);
            return;
        }
    }
    if let Some((prop, _)) = single_prop_chunk(chunk) {
        if let Some((_, old, quoted)) = before.iter().find(|(name, _, _)| *name == prop) {
            let text = if *quoted { format!("{old:?}") } else { old.clone() };
            let _ = makepad_platform::shader_error::take();
            if let Err(error) = eval_chunk(cx, widget, &format!("{prop}: {text}")) {
                log!("TWEAK revert of {prop} failed: {error}");
            }
            let _ = makepad_platform::shader_error::take();
        }
    }
}

/// `"a.b: value"` (no braces, one property) — the shape every sidebar edit
/// and `prop=`/`value=` shorthand takes.
pub fn single_prop_chunk(chunk: &str) -> Option<(String, String)> {
    let chunk = chunk.trim();
    let inner = chunk.strip_prefix('{').map_or(chunk, |rest| rest.strip_suffix('}').unwrap_or(rest));
    let inner = inner.trim();
    if inner.contains('\n') || inner.contains('{') || inner.contains(',') {
        return None;
    }
    let (name, value) = inner.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

/// The first problem of a draw-shader compile report, without the
/// compiler's own source pointers; says how many more there were.
pub fn terse_shader_error(report: &str) -> String {
    let lines: Vec<&str> = report.lines().filter(|l| !l.trim().is_empty()).collect();
    let first = lines.first().copied().unwrap_or(report);
    // `DrawQuad: shader field …` — the type name is the layer's, keep it.
    let first = match first.find(" (platform/") {
        Some(at) => &first[..at],
        None => first,
    };
    let first: String = first.chars().take(160).collect();
    if lines.len() > 1 {
        format!("{first} (+{} more)", lines.len() - 1)
    } else {
        first
    }
}

/// Rewrite every `tweak://apply:LINE:COL` in an error into the chunk's own
/// line numbers.
fn relocate_chunk_error(error: &str, callsite: u32) -> String {
    let mut out = String::new();
    let mut rest = error;
    while let Some(at) = rest.find("tweak://apply:") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + "tweak://apply:".len()..];
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.parse::<u32>() {
            Ok(line) if line > callsite => {
                // The parser counts the `use` line and one more for the
                // wrapper: measured, `@@` on chunk line 4 reports callsite+5.
                out.push_str(&format!("line {}", line - callsite - 1));
                rest = &tail[digits.len()..];
            }
            _ => {
                out.push_str("tweak://apply:");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A stable pseudo-line for a chunk: FNV-1a of its text, so each distinct
/// chunk is its own script body (and its own fn ScriptIps).
fn chunk_callsite_line(code: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in code.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 1_000_000) + 2
}


/// Live shader definitions a later apply to the same instance must preserve.
#[derive(Default)]
pub struct ShaderEdits {
    pub functions: HashMap<(u64, String, String), String>,
    pub generation: u64,
}

/// Values survive an apply (they live in the Rust draw vars) but the
/// SHADER is recomputed from the chain of the object just applied — and
/// every chunk derives a fresh object from the widget's untouched source,
/// so `draw_bg +: {color: #00f}` after a live `pixel: fn` edit compiled the
/// file's pixel back in (the fn edit vanished on the next colour tweak).
/// After a chunk that touches a layer with live fn edits, put those fns
/// back on top; their text is unchanged, so the compiled shader is a cache
/// hit.
pub fn reinject_fn_overrides(cx: &mut Cx, widget: &WidgetRef, chunk: &str) {
    let uid = widget.widget_uid().0;
    let Some(edits) = cx.get_global_ref::<ShaderEdits>() else { return };
    let overrides: Vec<((u64, String, String), String)> = edits.functions
        .iter()
        .filter(|((owner, _, _), _)| *owner == uid)
        .map(|(key, text)| (key.clone(), text.clone()))
        .collect();
    if overrides.is_empty() {
        return;
    }
    let mut layers: Vec<String> = Vec::new();
    for ((_, layer, name), _) in &overrides {
        let touches_layer = chunk.contains(&format!("{layer} +:"))
            || chunk.contains(&format!("{layer}:"))
            || chunk.contains(&format!("{layer}."));
        let defines_fn = chunk.contains(&format!("{name}:"));
        if touches_layer && !defines_fn && !layers.contains(layer) {
            layers.push(layer.clone());
        }
    }
    for layer in layers {
        let mut body = String::new();
        for ((_, l, _), text) in &overrides {
            if *l == layer {
                body.push_str(text);
                body.push('\n');
            }
        }
        let _ = makepad_platform::shader_error::take();
        if let Err(error) = eval_chunk(cx, widget, &format!("{layer} +: {{\n{body}}}")) {
            log!("TWEAK re-applying the live {layer} fns failed: {error}");
        }
        if let Some(error) = makepad_platform::shader_error::take() {
            log!("TWEAK re-applied live {layer} fns did not compile: {}", terse_shader_error(&error));
        }
    }
}

/// Remember `name: fn() {…}` entries of an applied `<layer> +: { … }` chunk
/// so the source view shows what runs, not what the file says.
pub fn record_fn_overrides(cx: &mut Cx, uid: u64, chunk: &str) {
    let chunk = chunk.trim();
    let Some(open) = chunk.find("+:") else { return };
    let layer = chunk[..open].trim().to_string();
    let Some(body) = chunk[open..].find('{').map(|i| &chunk[open + i + 1..]) else { return };
    let body = body.trim_end().trim_end_matches('}');
    let s = cx.global::<ShaderEdits>();
    s.generation += 1;
    for name in ["pixel", "vertex"] {
        if let Some(seg) = body.split(&format!("{name}:")).nth(1) {
            let seg = format!("{name}:{seg}");
            s.functions.insert((uid, layer.clone(), name.to_string()), reindent_two(seg.trim_end()));
        }
    }
}

/// Re-indent a fn's source for the view: drop the common leading indent and
/// turn every remaining 4-space (or tab) step into 2 spaces — "much less
/// aggressive indenting like 2 spaces per tab".
pub fn reindent_two(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            let indent: usize = l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum();
            let rest = l.trim_start_matches([' ', '\t']);
            let level = indent.saturating_sub(common) / 4;
            format!("{}{}", "  ".repeat(level), rest)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply to one instance and retain its pre-edit reflection for the caller.
pub fn apply_splash_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str) -> Result<Vec<(String, String, bool)>, String> {
    require_target(widget)?;
    let chunk = chunk.trim();
    let before = reflect_flat(cx, widget);
    // The draw shader compiles inside the apply itself, so an fn that names
    // a field this layer does not have fails RIGHT HERE. Never answer ok
    // with a widget that stopped drawing: put the layer back and say why.
    let _ = makepad_platform::shader_error::take();
    eval_chunk(cx, widget, chunk)?;
    if let Some(error) = makepad_platform::shader_error::take() {
        revert_failed_chunk(cx, widget, chunk, &before);
        let terse = terse_shader_error(&error);
        return Err(format!("shader compile error: {terse}"));
    }
    Ok(before)
}

pub(super) fn remote(cx: &mut Cx, widget: &WidgetRef, args: &super::Args) -> Result<String, String> {
    let chunk = match (super::arg(args, "splash"), super::arg(args, "prop"), super::arg(args, "value")) {
        (Some(chunk), None, None) => chunk.to_string(),
        (None, Some(prop), Some(value)) => format!("{prop}: {value}"),
        _ => return Err("need splash= or prop= and value=".into()),
    };
    let defined_layer = chunk.trim().trim_start_matches('{').split_once("+:")
        .map(|(layer, _)| layer.trim()).filter(|_| chunk.contains("fn"));
    let uid = widget.widget_uid().0;
    let functions = |cx: &mut Cx| {
        let mut out = Vec::new();
        if let Some(layer) = defined_layer {
            for (name, _, source) in layer_fn_sources(cx, widget, layer) {
                let key = (uid, layer.to_owned(), name.clone());
                let text = cx.get_global_ref::<ShaderEdits>().and_then(|edits| edits.functions.get(&key))
                    .cloned().unwrap_or(source);
                out.push((format!("{layer}.{name}"), text, true));
            }
        }
        out
    };
    let drawn = super::props::drawn_values(cx, widget);
    let mut before_fns = functions(cx);
    let mut before = apply_splash_chunk(cx, widget, &chunk)?;
    super::props::use_drawn_values(&mut before, &drawn);
    before.append(&mut before_fns);
    reinject_fn_overrides(cx, widget, &chunk);
    // The historical override reader expects a layer chunk without the outer
    // object braces. Both forms are accepted on the design wire.
    let body = chunk.trim();
    let body = body.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')).unwrap_or(body).trim();
    record_fn_overrides(cx, uid, body);
    let mut after = reflect_flat(cx, widget);
    super::props::use_drawn_values(&mut after, &super::props::drawn_values(cx, widget));
    after.extend(functions(cx));
    let single = single_prop_chunk(body);
    let target = super::arg(args, "prop").or_else(|| single.as_ref().map(|(name, _)| name.as_str()));
    Ok(answer(cx, widget, &before, &after, target))
}

/// The `{"old":..,"new":..}` answer of an edit: every row that changed, and `target` (the
/// edited property and the rows under it) even when it did not.
pub(super) fn answer(cx: &mut Cx, widget: &WidgetRef, before: &[(String, String, bool)], after: &[(String, String, bool)], target: Option<&str>) -> String {
    use super::json_str;
    let previously: HashMap<_, _> = before.iter().map(|(name, value, _)| (name.as_str(), value.as_str())).collect();
    let mut old = Vec::new();
    let mut new = Vec::new();
    for (name, value, _) in after {
        let previous = previously.get(name.as_str()).copied();
        if previous == Some(value) && !target.is_some_and(|prop| name == prop || name.strip_prefix(prop).is_some_and(|tail| tail.starts_with('.'))) {
            continue;
        }
        let key = json_str(name);
        old.push(format!("{key}:{}", previous.map_or_else(|| "null".into(), json_str)));
        new.push(format!("{key}:{}", json_str(value)));
    }
    for (name, value, _) in before {
        if !after.iter().any(|(new_name, _, _)| new_name == name) {
            let key = json_str(name);
            old.push(format!("{key}:{}", json_str(value)));
            new.push(format!("{key}:null"));
        }
    }
    widget.redraw(cx);
    cx.redraw_all();
    format!(r#"{{"old":{{{}}},"new":{{{}}}}}"#, old.join(","), new.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_prop_chunks_parse() {
        assert_eq!(
            single_prop_chunk("padding.left: 12"),
            Some(("padding.left".to_string(), "12".to_string()))
        );
        assert_eq!(
            single_prop_chunk("{draw_bg.border_radius: 8}"),
            Some(("draw_bg.border_radius".to_string(), "8".to_string()))
        );
        // multi-property or nested chunks are not single props
        assert_eq!(single_prop_chunk("{a: 1, b: 2}"), None);
        assert_eq!(single_prop_chunk("padding: Inset{left: 4}"), None);
        assert_eq!(single_prop_chunk("draw_bg +: {color: #f00}"), None);
    }
}
