use crate::{makepad_draw::*, makepad_script::{trap::NoTrap, ScriptHeap}, widget::*, widget_tree::live_id_token};
use crate::makepad_micro_serde::{SerJson, SerJsonState};
use std::collections::{HashMap, HashSet};

pub fn fmt_f64(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else {
        format!("{}", (value * 10000.0).round() / 10000.0)
    }
}

fn fmt_scalar(heap: &ScriptHeap, value: ScriptValue) -> Option<String> {
    if value.is_nil() {
        return Some("null".to_string());
    }
    if let Some(color) = value.as_color() {
        return Some(format!("#{color:08x}"));
    }
    if let Some(b) = value.as_bool() {
        return Some(if b { "true" } else { "false" }.to_string());
    }
    if let Some(n) = value.as_number() {
        if n.fract() == 0.0 && n.abs() < 1.0e15 {
            return Some(format!("{}", n as i64));
        }
        return Some(format!("{}", (n * 10000.0).round() / 10000.0));
    }
    if let Some(id) = value.as_id() {
        return Some(format!("@{}", live_id_token(id)));
    }
    if value.is_string_like() {
        if let Some(text) = heap.string_with(value, |_, s| format!("{s:?}")) {
            return Some(text);
        }
        return Some("\"\"".to_string());
    }
    if let Some(pod) = value.as_pod() {
        let (pod_type, data) = heap.pod_data(pod);
        let name = pod_type
            .name()
            .map(live_id_token)
            .unwrap_or_else(|| "pod".to_string());
        // A unit-range vec4f in UI code is a color; render it the way the
        // splash source would spell it, so diffs paste straight back.
        if name == "vec4f" && data.len() == 4 {
            let mut components = [0.0f32; 4];
            let mut unit = true;
            for (index, raw) in data.iter().enumerate() {
                let v = f32::from_bits(*raw);
                if !(0.0..=1.0).contains(&v) {
                    unit = false;
                    break;
                }
                components[index] = v;
            }
            if unit {
                let to_byte = |v: f32| (v * 255.0).round() as u32;
                return Some(format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    to_byte(components[0]),
                    to_byte(components[1]),
                    to_byte(components[2]),
                    to_byte(components[3])
                ));
            }
        }
        let mut out = format!("{name}(");
        for (index, raw) in data.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            let v = f32::from_bits(*raw);
            if v.fract() == 0.0 {
                out.push_str(&format!("{}", v as i64));
            } else {
                out.push_str(&format!("{v}"));
            }
        }
        out.push(')');
        return Some(out);
    }
    None
}

/// Splash-ish one-line rendering of a value, `depth` levels of objects deep.
/// How an enum value is written out. `Full` names the enum and carries
/// every field that is set, so the text re-applies exactly under
/// `Apply::Eval`; `Display` is for a row's eye and drops the enum's name and
/// the fields equal to the variant's own defaults.
#[derive(Clone, Copy, PartialEq)]
pub enum EnumFmt {
    Full,
    Display,
}

/// (enum, variant) for an object that came from a derived enum, else None.
/// The derive stamps `__enum` on every variant object -- a bare variant, a
/// tuple, a named one -- and instances inherit it through the proto chain;
/// the variant itself is the first bare id in that chain.
pub fn enum_info(heap: &ScriptHeap, obj: ScriptObject) -> Option<(LiveId, LiveId)> {
    let en = heap.value(obj, live_id!(__enum).into(), NoTrap).as_id()?;
    let mut ptr = obj;
    for _ in 0..8 {
        let proto = heap.proto(ptr);
        if let Some(id) = proto.as_id() {
            return Some((en, id));
        }
        ptr = proto.as_object()?;
    }
    None
}

/// The nearest proto that is an object: for an instance of a named variant
/// that is the frozen variant itself, which holds the field defaults.
fn enum_variant_proto(heap: &ScriptHeap, obj: ScriptObject) -> Option<ScriptObject> {
    heap.proto(obj).as_object()
}

/// `Flow.Down`, `Size.Fixed(200)`, `Size.Fill{weight: 100 basis: 0 shrink: 0}`.
/// A relative size prints as the CSS it was written as: `50%`, `25vw`.
pub fn fmt_enum(heap: &ScriptHeap, obj: ScriptObject, fmt: EnumFmt) -> Option<String> {
    let (en, var) = enum_info(heap, obj)?;
    let en_name = live_id_token(en);
    let var_name = live_id_token(var);

    // A relative size is a string in the person's head, not a struct.
    if var_name == "Rel" && (en_name == "Size" || en_name == "FitBound") {
        let factor = heap.value(obj, live_id!(factor).into(), NoTrap).as_number();
        let base = heap
            .value(obj, live_id!(base).into(), NoTrap)
            .as_object()
            .and_then(|b| enum_info(heap, b))
            .map(|(_, v)| live_id_token(v));
        if let (Some(factor), Some(base)) = (factor, base) {
            let unit = match base.as_str() {
                "Parent" => Some("%"),
                "Vw" => Some("vw"),
                "Vh" => Some("vh"),
                "Cqw" => Some("cqw"),
                "Cqh" => Some("cqh"),
                _ => None,
            };
            if let Some(unit) = unit {
                // Quoted: this text is what a reset or an undo re-applies,
                // and `50%` bare is an operator with nothing after it.
                return Some(format!("\"{}{unit}\"", fmt_f64(factor * 100.0)));
            }
        }
    }

    let head = match fmt {
        EnumFmt::Full => format!("{en_name}.{var_name}"),
        EnumFmt::Display => var_name.clone(),
    };

    // Tuple: the positional values.
    let len = heap.vec_len(obj);
    if len > 0 {
        let mut out = head;
        out.push('(');
        for index in 0..len {
            if index > 0 {
                out.push_str(", ");
            }
            let value = heap.vec_value_if_exist(obj, index)?;
            out.push_str(&fmt_value(heap, value, 2).unwrap_or_else(|| "?".to_string()));
        }
        out.push(')');
        return Some(out);
    }

    // Named: the instance's own fields first, in the order they were set,
    // then the variant's defaults it did not set. Hidden keys stay hidden.
    let mut keys: Vec<LiveId> = Vec::new();
    collect_prop_keys(heap, obj, &mut keys);
    keys.retain(|k| *k != live_id!(__enum) && !live_id_token(*k).starts_with('_'));
    if keys.is_empty() {
        return Some(head); // bare
    }
    let defaults = enum_variant_proto(heap, obj);
    let mut out = head.clone();
    out.push('{');
    let mut first = true;
    for key in keys {
        let value = heap.value(obj, key.into(), NoTrap);
        if value.is_nil() {
            continue; // an Option that is None
        }
        let Some(text) = fmt_value(heap, value, 2) else { continue };
        if fmt == EnumFmt::Display {
            if let Some(defaults) = defaults {
                let default = heap.value(defaults, key.into(), NoTrap);
                if fmt_value(heap, default, 2).as_deref() == Some(text.as_str()) {
                    continue;
                }
            }
        }
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(&live_id_token(key));
        out.push_str(": ");
        out.push_str(&text);
    }
    if first {
        // Nothing to say beyond the variant: every field is its default
        // (Display) or none is set. `Fill`, not `Fill{}`.
        return Some(head);
    }
    out.push('}');
    Some(out)
}

pub fn fmt_value(heap: &ScriptHeap, value: ScriptValue, depth: usize) -> Option<String> {
    if let Some(text) = fmt_scalar(heap, value) {
        return Some(text);
    }
    if let Some(array) = value.as_array() {
        let mut out = String::from("[");
        // A list of plain values -- tracks, area rows -- prints whole, since
        // an editor writes the printed list back; a list of objects is a
        // dump and stops at eight. Commas keep a spaced minus from becoming subtraction.
        let full = heap.array_len(array);
        let objects = full > 0 && heap.array_index(array, 0, NoTrap).as_object().is_some();
        let len = if objects { full.min(8) } else { full.min(4096) };
        for index in 0..len {
            if index > 0 {
                out.push_str(", ");
            }
            let item = heap.array_index(array, index, NoTrap);
            match fmt_value(heap, item, depth.saturating_sub(1)) {
                Some(text) => out.push_str(&text),
                None => out.push('?'),
            }
        }
        out.push(']');
        return Some(out);
    }
    if let Some(obj) = value.as_object() {
        if heap.as_fn(obj).is_some() {
            return None; // functions are not properties
        }
        // `instance(v)` / `uniform(v)` shader-io wrappers carry the value in
        // their proto slot — unwrap so shader inputs read as plain values.
        if heap.as_shader_io(obj).is_some() {
            let inner = heap.proto(obj);
            if inner.is_nil() {
                return None; // texture/buffer declarations, not values
            }
            return fmt_value(heap, inner, depth);
        }
        // A derived enum prints as one whole value, never as its fields.
        if let Some(text) = fmt_enum(heap, obj, EnumFmt::Full) {
            return Some(text);
        }
        if depth == 0 {
            return Some("{..}".to_string());
        }
        let mut keys: Vec<LiveId> = Vec::new();
        collect_prop_keys(heap, obj, &mut keys);
        let mut out = String::from("{");
        let mut first = true;
        for key in keys.into_iter().take(24) {
            let value = heap.value(obj, key.into(), NoTrap);
            let Some(text) = fmt_value(heap, value, depth - 1) else {
                continue;
            };
            if !first {
                out.push(' ');
            }
            first = false;
            out.push_str(&live_id_token(key));
            out.push_str(": ");
            out.push_str(&text);
        }
        out.push('}');
        return Some(out);
    }
    None
}

/// The union of map keys over the object and its proto chain, leaf first —
/// exactly the widget's live surface: what was applied plus every default
/// the type registered.
pub(super) fn collect_prop_keys(heap: &ScriptHeap, obj: ScriptObject, keys: &mut Vec<LiveId>) {
    let mut ptr = obj;
    let mut hops = 0;
    loop {
        heap.map_entries(ptr).into_iter().for_each(|e| {
            let key = e.key;
            if let Some(id) = key.as_id() {
                if !keys.contains(&id) {
                    let name = live_id_token(id);
                    if !name.starts_with("__") && !skip_key(&name) {
                        keys.push(id);
                    }
                }
            }
        });
        match heap.proto(ptr).as_object() {
            Some(next) => {
                ptr = next;
                hops += 1;
                if hops > 24 {
                    break;
                }
            }
            None => break,
        }
    }
}

/// Keys that are structure, not style: reflected objects skip them.
fn skip_key(name: &str) -> bool {
    matches!(name, "animator")
}

/// Draw-shader plumbing the GPU pipeline owns — never user style. Skipped in
/// the dotted expansion so the sidebar shows design inputs, not internals.
pub(super) fn is_shader_plumbing(name: &str) -> bool {
    matches!(
        name,
        "rect_pos"
            | "rect_size"
            | "draw_clip"
            | "delta_clip"
            | "draw_depth"
            | "draw_zbias"
            | "char_index"
            | "texture_index"
            | "atlas_plane"
            | "t"
            | "t_min"
            | "t_max"
            | "temp_y_shift"
            | "aa_2x2"
            | "aa_4x4"
            | "pad1"
            | "pad2"
            | "font_scale"
            | "depth_clip"
            | "debug"
            | "stem_darken_max"
            | "draw_scroll"
            | "view_shift"
            | "view_clip"
            | "camera_projection"
            | "camera_view"
            | "camera_inv"
            | "dpi_factor"
            | "dpi_dilate"
            | "seed"
            | "pos"
            | "vertex_pos"
            | "total_chars"
            | "char_depth"
            | "draw_call"
            | "draw_list"
            | "draw_pass"
            | "geom"
            | "world"
            | "extend_area"
            | "ink_centered"
            | "sdf_sharpness"
            | "sdf_luma_bias"
            | "draw_flags"
            | "clip_x"
            | "clip_y"
    )
}

/// A value rendering that carries no information for the sidebar or diff.
fn is_noise(text: &str) -> bool {
    matches!(text, "{}" | "{..}" | "null")
}

/// Flat `name -> value` reflection of a widget's CURRENT state, one level of
/// nesting expanded with dotted names (`draw_bg.color`). Values come from
/// `script_to_value` — the Rust fields serialized back to script — so runtime
/// applies are visible; the `#[source]` object's own map (what the DSL
/// explicitly applied) supplies the `set` flag. This is both the sidebar's
/// data and the before/after capture the diff log works from. Part of the
/// public reflection API: the same read a catalogue app's Docs and
/// Controls panels build on.
pub fn reflect_flat(cx: &mut Cx, widget: &WidgetRef) -> Vec<(String, String, bool)> {
    cx.with_vm(|vm| {
        // Serializing a widget back to script trips harmless type-check
        // complaints on fn-ref fields (`on_click` serializes to a value its
        // own type check rejects). Reflection is read-only — capture and
        // drop them instead of spamming the log on every state read.
        vm.capture_errors();
        let current = widget.current_to_value(vm);
        let _ = vm.take_errors();
        let Some(current_obj) = current.as_object() else {
            return Vec::new();
        };
        let source = widget.script_source();
        let heap = vm.heap();
        let own: Vec<LiveId> = if source == ScriptObject::ZERO {
            Vec::new()
        } else {
            heap.map_entries(source)
                .into_iter()
                .filter_map(|e| e.key.as_id())
                .collect()
        };
        let mut out = Vec::new();
        // Only the object's OWN map: script_to_value wrote every live field
        // explicitly, the proto behind it is just the type object.
        let keys: Vec<LiveId> = heap
            .map_entries(current_obj)
            .into_iter()
            .filter_map(|e| e.key.as_id())
            .filter(|id| {
                let name = live_id_token(*id);
                !name.starts_with("__") && !skip_key(&name)
            })
            .collect();
        for key in keys {
            let value = heap.value(current_obj, key.into(), NoTrap);
            let is_set = own.contains(&key);
            let name = live_id_token(key);
            if let Some(obj) = value.as_object() {
                if heap.as_fn(obj).is_some() {
                    continue;
                }
                // An enum is one row, whole -- `width: Size.Fill{weight: 100}`
                // -- and is never dotted into `width.weight`: a field of a
                // variant is not a property a person can set on its own.
                if enum_info(heap, obj).is_some() {
                    if let Some(text) = fmt_enum(heap, obj, EnumFmt::Full) {
                        out.push((name, text, is_set));
                    }
                    continue;
                }
                // One level of dotted expansion for typed sub-structs
                // (draw_bg.color, padding.left ...), own map only.
                let sub_keys: Vec<LiveId> = heap
                    .map_entries(obj)
                    .into_iter()
                    .filter_map(|e| e.key.as_id())
                    .filter(|id| {
                        let name = live_id_token(*id);
                        !name.starts_with("__") && !is_shader_plumbing(&name)
                    })
                    .collect();
                let had_subs = !sub_keys.is_empty();
                let mut wrote_sub = false;
                for sub_key in sub_keys.into_iter().take(200) {
                    let sub_value = heap.value(obj, sub_key.into(), NoTrap);
                    if sub_value.as_object().is_some_and(|o| heap.as_fn(o).is_some()) {
                        continue;
                    }
                    if let Some(text) = fmt_value(heap, sub_value, 1) {
                        if is_noise(&text) {
                            continue;
                        }
                        out.push((format!("{name}.{}", live_id_token(sub_key)), text, is_set));
                        wrote_sub = true;
                    }
                }
                if !wrote_sub && !had_subs {
                    if let Some(text) = fmt_value(heap, value, 1) {
                        if !is_noise(&text) {
                            out.push((name, text, is_set));
                        }
                    }
                }
            } else if let Some(text) = fmt_value(heap, value, 1) {
                if !is_noise(&text) || is_set {
                    out.push((name, text, is_set));
                }
            }
            if out.len() > 400 {
                break;
            }
        }
        // Second pass: the type's shader-input registry. Instance/uniform
        // inputs declared in the DSL (`border_radius: instance(2.0)`) are
        // not Rust fields, so `script_to_value` never sees them — but they
        // live in the source object's proto chain. Union them in (source
        // values are the pre-tweak truth; tweaks to them land in the diff).
        if source != ScriptObject::ZERO {
            let mut keys = Vec::new();
            collect_prop_keys(heap, source, &mut keys);
            for key in keys {
                let name = live_id_token(key);
                if is_shader_plumbing(&name) {
                    continue;
                }
                let value = heap.value(source, key.into(), NoTrap);
                if let Some(obj) = value.as_object() {
                    if heap.as_fn(obj).is_some() {
                        continue;
                    }
                    if enum_info(heap, obj).is_some() {
                        if !out.iter().any(|(existing, _, _)| *existing == name) {
                            if let Some(text) = fmt_enum(heap, obj, EnumFmt::Full) {
                                out.push((name, text, false));
                            }
                        }
                        continue;
                    }
                    // The live value answered for this key with nothing --
                    // `cell: null` after it was cleared -- so the source's
                    // fields under it are what WAS, not what is. A live
                    // object that dumped whole still gets its declared
                    // inputs read out of the source (a draw layer's
                    // `color`, `border_size`), which only live there.
                    if out.iter().any(|(existing, text, _)| *existing == name && text == "null") {
                        continue;
                    }
                    let mut sub_keys = Vec::new();
                    collect_prop_keys(heap, obj, &mut sub_keys);
                    sub_keys.retain(|id| {
                        let name = live_id_token(*id);
                        !name.starts_with("__") && !is_shader_plumbing(&name)
                    });
                    for sub_key in sub_keys.into_iter().take(200) {
                        let sub_name = live_id_token(sub_key);
                        let dotted = format!("{name}.{sub_name}");
                        if out.iter().any(|(existing, _, _)| *existing == dotted) {
                            continue;
                        }
                        let sub_value = heap.value(obj, sub_key.into(), NoTrap);
                        if sub_value
                            .as_object()
                            .is_some_and(|o| heap.as_fn(o).is_some())
                        {
                            continue;
                        }
                        if let Some(text) = fmt_value(heap, sub_value, 1) {
                            if !is_noise(&text) {
                                out.push((dotted, text, false));
                            }
                        }
                    }
                } else {
                    if out.iter().any(|(existing, _, _)| *existing == name) {
                        continue;
                    }
                    if let Some(text) = fmt_value(heap, value, 1) {
                        if !is_noise(&text) {
                            out.push((name, text, false));
                        }
                    }
                }
                if out.len() > 400 {
                    break;
                }
            }
        }
        out
    })
}

/// Every `/** */` annotation behind a widget's properties, gathered up its
/// construction chain: property name (dotted for a sub-object's field, as
/// [`reflect_flat`] names it) to the doc's text. Part of the
/// public reflection API: a Docs panel's third column.
pub fn collect_row_docs(cx: &mut Cx, widget: &WidgetRef) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    cx.with_vm(|vm| {
        let chain = vm.construction_chain(source.into());
        for lvl in &chain {
            for (f, d) in &lvl.field_docs() {
                out.entry(live_id_token(*f)).or_insert_with(|| d.clone());
            }
        }
        for lvl in &chain {
            for key in lvl.own_keys() {
                let value = vm.value(lvl.object(), *key);
                let Some(obj) = value.as_object() else { continue };
                if vm.heap().as_fn(obj).is_some() {
                    continue;
                }
                let name = live_id_token(*key);
                for sub in vm.construction_chain(value) {
                    if let Some(doc) = sub.doc() {
                        out.entry(name.clone()).or_insert_with(|| doc.to_owned());
                    }
                    for (f, d) in &sub.field_docs() {
                        out.entry(format!("{name}.{}", live_id_token(*f)))
                            .or_insert_with(|| d.clone());
                    }
                }
            }
        }
    });
    out
}

/// One level of the selection's construction chain, display-ready.
/// Built from `vm.construction_chain` over the widget's `#[source]` object:
/// the proto chain of `made_at` ips, each resolved to a source location and
/// its `///` docs (see platform/script/src/docs.rs).
#[derive(Clone)]
pub struct CascadeLevel {
    /// "button.rs:52" (basename:line), or "native" for Rust-built levels.
    pub loc: String,
    /// Full file path for click-through / the AI ("" for native levels).
    pub file: String,
    pub line: u32,
    /// `///` doc attached to the level's object literal.
    pub doc: Option<String>,
    /// `///` docs attached to fields of that literal.
    pub field_docs: Vec<(String, String)>,
    /// Keys the level sets itself; true = overridden by a closer level.
    pub sets: Vec<(String, bool)>,
}

pub fn cascade_levels(cx: &mut Cx, widget: &WidgetRef) -> Vec<CascadeLevel> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Vec::new();
    }
    cx.with_vm(|vm| {
        let chain = vm.construction_chain(source.into());
        let mut seen: std::collections::HashSet<String> = Default::default();
        let mut out = Vec::new();
        for lvl in chain {
            let (loc, file, line) = match lvl.loc() {
                Some(l) => {
                    let base = l.file().rsplit('/').next().unwrap_or(l.file());
                    (format!("{base}:{}", l.line()), l.file().to_owned(), l.line())
                }
                None => ("native".to_string(), String::new(), 0),
            };
            let mut sets: Vec<(String, bool)> = Vec::new();
            for key in lvl.own_keys() {
                let name = live_id_token(*key);
                if name.starts_with("__") || skip_key(&name) {
                    continue;
                }
                let overridden = seen.contains(&name);
                sets.push((name, overridden));
            }
            for (name, _) in &sets {
                seen.insert(name.clone());
            }
            out.push(CascadeLevel {
                loc,
                file,
                line,
                doc: lvl.doc().map(str::to_owned),
                field_docs: lvl
                    .field_docs()
                    .iter()
                    .map(|(f, d)| (live_id_token(*f), d.clone()))
                    .collect(),
                sets,
            });
        }
        out
    })
}

/// Read shader inputs from the submitted draw of this instance. The flat
/// source registry provides their types and defaults; an eval or animation
/// can change their live slots without writing a Rust field.
pub(super) fn drawn_values(cx: &Cx, widget: &WidgetRef) -> HashMap<String, Vec<f32>> {
    let mut values = HashMap::new();
    for (layer, area) in widget.layer_areas() {
        let Some(inst) = area.valid_instance(cx) else { continue };
        let item = &cx.draw_lists[inst.draw_list_id].draw_items[inst.draw_item_id];
        let Some(call) = item.draw_call() else { continue };
        let mapping = &cx.draw_shaders[call.draw_shader_id.index].mapping;
        for input in &mapping.dyn_uniforms.inputs {
            if let Some(slots) = call.dyn_uniforms.get(input.offset..input.offset + input.slots) {
                values.insert(format!("{layer}.{}", live_id_token(input.id)), slots.to_vec());
            }
        }
        if let Some(instances) = item.instances.as_ref() {
            for input in &mapping.dyn_instances.inputs {
                let Some(field) = mapping.instances.find(input.id) else { continue };
                let start = inst.instance_offset + field.offset;
                if let Some(slots) = instances.get(start..start + field.slots) {
                    values.insert(format!("{layer}.{}", live_id_token(input.id)), slots.to_vec());
                }
            }
        }
    }
    values
}

pub(super) fn use_drawn_values(rows: &mut [(String, String, bool)], drawn: &HashMap<String, Vec<f32>>) {
    for (name, text, _) in rows {
        let Some(slots) = drawn.get(name) else { continue };
        match slots.as_slice() {
            [v] if text == "true" || text == "false" => *text = (v != &0.0).to_string(),
            [v] => *text = fmt_f64(*v as f64),
            [r, g, b, a] if text.starts_with('#') => {
                let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                *text = format!("#{:02x}{:02x}{:02x}{:02x}", channel(*r), channel(*g), channel(*b), channel(*a));
            }
            _ if text.starts_with("vec") => {
                if let Some((ty, _)) = text.split_once('(') {
                    let components = slots.iter().map(|v| fmt_f64(*v as f64)).collect::<Vec<_>>();
                    *text = format!("{ty}({})", components.join(", "));
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderInputInfo {
    pub name: String,
    pub io: &'static str,
    pub doc: String,
}

impl SerJson for ShaderInputInfo {
    fn ser_json(&self, depth: usize, state: &mut SerJsonState) {
        state.st_pre();
        state.field(depth + 1, "name");
        self.name.ser_json(depth + 1, state);
        state.conl();
        state.field(depth + 1, "io");
        self.io.ser_json(depth + 1, state);
        if !self.doc.is_empty() {
            state.conl();
            state.field(depth + 1, "doc");
            self.doc.ser_json(depth + 1, state);
        }
        state.st_post(depth);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct ShaderConstInfo {
    pub name: String,
    pub value: String,
    pub initial: String,
    pub loc: String,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct ShaderFnInfo {
    pub name: String,
    pub loc: String,
    pub src: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct ShaderLayerInfo {
    pub layer: String,
    pub drawn: bool,
    pub doc: String,
    pub inputs: Vec<ShaderInputInfo>,
    pub consts: Vec<ShaderConstInfo>,
    pub fns: Vec<ShaderFnInfo>,
}

/// The shader submitted for this draw layer. Widgets with no named primary
/// area expose their primary material through the widget's own area.
pub fn layer_shader_id(cx: &Cx, widget: &WidgetRef, layer: &str, primary: bool) -> Option<DrawShaderId> {
    let area = widget.layer_areas().into_iter()
        .find(|(name, _)| *name == layer)
        .map(|(_, area)| area)
        .or_else(|| primary.then(|| widget.area()))?;
    let inst = area.valid_instance(cx)?;
    let item = &cx.draw_lists[inst.draw_list_id].draw_items[inst.draw_item_id];
    Some(item.draw_call()?.draw_shader_id)
}

pub fn layer_consts(cx: &mut Cx, widget: &WidgetRef, layer: &str, primary: bool) -> Vec<ShaderConstInfo> {
    let Some(shader) = layer_shader_id(cx, widget, layer, primary) else { return Vec::new() };
    shader_consts(cx, shader)
}

fn shader_consts(cx: &mut Cx, shader: DrawShaderId) -> Vec<ShaderConstInfo> {
    let mut seen = HashSet::new();
    let raw: Vec<_> = cx.shader_const_table(shader).iter()
        .filter(|tc| seen.insert(tc.name.as_str()))
        .map(|tc| (tc.name.clone(), tc.value, tc.initial, tc.ip, tc.doc.clone()))
        .collect();
    if raw.is_empty() {
        return Vec::new();
    }
    cx.with_vm(|vm| raw.into_iter().map(|(name, value, initial, ip, doc)| {
        let loc = vm.ip_to_loc(ip).map(|loc| {
            let base = loc.file().rsplit('/').next().unwrap_or(loc.file());
            format!("{base}:{}", loc.line())
        }).unwrap_or_else(|| "?".to_owned());
        ShaderConstInfo { name, value: fmt_f64(value as f64), initial: fmt_f64(initial as f64), loc, doc }
    }).collect())
}

fn shader_input_io(vm: &mut ScriptVm, shader: Option<DrawShaderId>, layer: Option<ScriptObject>, leaf: LiveId) -> &'static str {
    use crate::makepad_script::mod_shader::{SHADER_IO_DYN_INSTANCE, SHADER_IO_DYN_UNIFORM, SHADER_IO_RUST_INSTANCE};
    if let Some(shader) = shader {
        let mapping = &vm.cx().draw_shaders[shader.index].mapping;
        if mapping.dyn_instances.find(leaf).is_some() || mapping.instances.find(leaf).is_some() {
            return "instance";
        }
        if mapping.dyn_uniforms.find(leaf).is_some() {
            return "uniform";
        }
    }
    // A plain value can override an input. Its highest declaration still
    // determines the IO kind, as it does in the shader compiler.
    let heap = vm.heap();
    let mut current = layer;
    let mut highest = None;
    for _ in 0..24 {
        let Some(obj) = current else { break };
        if let Some(io) = heap.map_own_value(obj, leaf.into())
            .and_then(|own| own.as_object()).and_then(|own| heap.as_shader_io(own))
        {
            highest = Some(io);
        }
        current = heap.proto(obj).as_object();
    }
    match highest {
        Some(SHADER_IO_RUST_INSTANCE | SHADER_IO_DYN_INSTANCE) => "instance",
        Some(SHADER_IO_DYN_UNIFORM) => "uniform",
        _ => "field",
    }
}

pub fn collect_shaders(cx: &mut Cx, widget: &WidgetRef, rows: &[(String, String, bool)]) -> Vec<ShaderLayerInfo> {
    let mut layers = Vec::new();
    let reflected = rows.iter().filter_map(|(prop, _, _)| prop.split('.').next());
    let areas = widget.layer_areas();
    for layer in reflected.chain(areas.iter().map(|(layer, _)| *layer)) {
        if layer.starts_with("draw_") && !layers.contains(&layer) {
            layers.push(layer);
        }
    }
    if layers.is_empty() {
        return Vec::new();
    }
    let docs = collect_row_docs(cx, widget);
    let source = widget.script_source();
    let uid = widget.widget_uid().0;
    let mut shaders = Vec::with_capacity(layers.len());
    for (index, layer) in layers.into_iter().enumerate() {
        let shader = layer_shader_id(cx, widget, layer, index == 0);
        let prefix = format!("{layer}.");
        let inputs = cx.with_vm(|vm| {
            let layer_obj = (source != ScriptObject::ZERO)
                .then(|| vm.value(source, LiveId::from_str(layer)).as_object()).flatten();
            rows.iter().filter_map(|(prop, _, _)| {
                let leaf = prop.strip_prefix(&prefix)?;
                Some(ShaderInputInfo {
                    name: leaf.to_owned(),
                    io: shader_input_io(vm, shader, layer_obj, LiveId::from_str(leaf)),
                    doc: docs.get(prop).cloned().unwrap_or_default(),
                })
            }).collect()
        });
        let consts = shader.map(|id| shader_consts(cx, id)).unwrap_or_default();
        let sources = super::apply::layer_fn_sources(cx, widget, layer);
        let edits = cx.get_global_ref::<super::apply::ShaderEdits>();
        let fns = sources.into_iter().map(|(name, loc, raw)| {
            let src = edits.and_then(|edits| edits.functions.get(&(uid, layer.to_owned(), name.clone())))
                .cloned().unwrap_or_else(|| super::apply::reindent_two(&raw));
            ShaderFnInfo { name, loc, src }
        }).collect();
        shaders.push(ShaderLayerInfo {
            layer: layer.to_owned(), drawn: shader.is_some(),
            doc: docs.get(layer).cloned().unwrap_or_default(), inputs, consts, fns,
        });
    }
    shaders
}

pub(super) fn remote(cx: &mut Cx, widget: &WidgetRef) -> String {
    use super::json_str;
    let mut rows = reflect_flat(cx, widget);
    use_drawn_values(&mut rows, &drawn_values(cx, widget));
    let cascade = cascade_levels(cx, widget);
    let source = widget.script_source();
    let props = cx.with_vm(|vm| {
        let mut definitions = HashMap::new();
        if source != ScriptObject::ZERO {
            for (depth, level) in vm.construction_chain(source.into()).iter().enumerate() {
                for key in level.own_keys() {
                    let name = live_id_token(*key);
                    let value = vm.value(level.object(), *key);
                    definitions.entry(name.clone()).or_insert((depth, value));
                    if let Some(object) = value.as_object() {
                        for field in vm.heap().map_entries(object) {
                            if let Some(field_id) = field.key.as_id() {
                                definitions.entry(format!("{name}.{}", live_id_token(field_id)))
                                    .or_insert_with(|| (depth, vm.value(object, field_id)));
                            }
                        }
                    }
                }
            }
        }
        rows.iter().map(|(name, value, is_set)| {
            let definition = definitions.get(name).copied().or_else(|| {
                let (parent, field) = name.split_once('.')?;
                let &(depth, parent) = definitions.get(parent)?;
                let value = vm.value(parent.as_object()?, LiveId::from_str(field));
                (!value.is_nil()).then_some((depth, value))
            });
            let level = definition.map_or_else(|| "null".into(), |(d, _)| d.to_string());
            let origin = match definition {
                None => "runtime",
                Some((_, v)) if fmt_value(vm.heap(), v, 1).is_some_and(|v| v != *value) => "runtime",
                Some((0, _)) => "authored",
                Some(_) => "inherited",
            };
            format!(r#"{{"name":{},"value":{},"set":{is_set},"origin":"{origin}","level":{level}}}"#,
                json_str(name), json_str(value))
        }).collect::<Vec<_>>().join(",")
    });
    let levels = cascade.iter().map(|l| format!(r#"{{"file":{},"line":{},"loc":{}}}"#,
        json_str(&l.file), l.line, json_str(&l.loc))).collect::<Vec<_>>().join(",");
    let shaders = collect_shaders(cx, widget, &rows).serialize_json();
    format!(r#"{{"props":[{props}],"cascade":[{levels}],"shaders":{shaders}}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_enums_print_as_whole_values() {
        use crate::makepad_draw::turtle::RowAlign;
        use crate::makepad_draw::{Base, FitBound, Flow, Size};
        use crate::makepad_script::{ScriptApply, ScriptNew};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            // In dependency order, the way the draw crate's module does it:
            // building a named variant's proto converts its DEFAULT fields
            // to script values, and a bare default such as `Base::Full` or
            // `RowAlign::Top` looks its own type up on the way.
            <Base as ScriptNew>::script_api(vm);
            <RowAlign as ScriptNew>::script_api(vm);
            <FitBound as ScriptNew>::script_api(vm);
            <Size as ScriptNew>::script_api(vm);
            <Flow as ScriptNew>::script_api(vm);
            let fill = Size::Fill {
                weight: 100.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: None,
                max: None,
            }
            .script_to_value(vm);
            let fixed = Size::Fixed(200.0).script_to_value(vm);
            let rel = Size::Rel { base: Base::Parent, factor: 0.5 }.script_to_value(vm);
            let down = Flow::Down.script_to_value(vm);
            let heap = vm.heap();
            let fill_obj = fill.as_object().expect("a named variant is an object");
            assert_eq!(
                enum_info(heap, fill_obj).map(|(e, v)| (live_id_token(e), live_id_token(v))),
                Some(("Size".to_string(), "Fill".to_string()))
            );
            assert_eq!(
                fmt_value(heap, fill, 2).as_deref(),
                Some("Size.Fill{weight: 100 basis: FitBound.Abs(0) shrink: 0}")
            );
            assert_eq!(fmt_value(heap, fixed, 2).as_deref(), Some("Size.Fixed(200)"));
            assert_eq!(fmt_value(heap, down, 2).as_deref(), Some("Flow.Down"));
            // A relative size prints quoted: the text is what a reset
            // re-applies, and the panel reads it back the same.
            assert_eq!(fmt_value(heap, rel, 2).as_deref(), Some("\"50%\""));
            // Display drops the enum and the fields equal to the defaults.
            assert_eq!(fmt_enum(heap, fill_obj, EnumFmt::Display).as_deref(), Some("Fill"));
        });
    }
    #[test]
    fn fmt_f64_stays_compact() {
        assert_eq!(fmt_f64(12.0), "12");
        assert_eq!(fmt_f64(2.5), "2.5");
        assert_eq!(fmt_f64(0.33333333), "0.3333");
    }
}
