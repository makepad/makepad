//! Reading passes from Splash documents: the fields every host reads the
//! same way. A host (motion3d, Motion, Sandbox) finds a `Pass{}` or a kit
//! in its document, calls a kit's template ([`crate::kits`]) when it is
//! one, and hands each pass object to [`read_pass`]. The uniforms come back
//! as script values: the host evaluates them with its own keyables (every
//! post parameter is read once per frame, at the frame point) and sets
//! each uniform's width from what it evaluates to.

use crate::pass::PassDecl;
use crate::plan::{Format, Stage};
use makepad_script::*;

/// A pass read from a document, before its uniforms are typed.
pub struct PassRead {
    /// The pass, with `uniforms` still empty.
    pub decl: PassDecl,
    /// The author's uniforms: name and value (a number, a colour, a vector,
    /// or the host's keyable).
    pub uniforms: Vec<(String, ScriptValue)>,
}

/// The fields a pass object may have (for a host's unknown-field check).
pub const PASS_FIELDS: &[&str] = &["outputs", "at", "name", "reads", "slots", "scale", "size", "format", "uniforms", "pixel", "helpers", "history"];

fn get(vm: &ScriptVm, o: ScriptObject, name: &str) -> ScriptValue {
    let v = vm.bx.heap.value(o, LiveId::from_str(name).into(), NoTrap);
    if v.is_err() { NIL } else { v }
}

fn text(vm: &mut ScriptVm, v: ScriptValue) -> Option<String> {
    if v.is_nil() {
        return None;
    }
    if let Some(id) = v.as_id() {
        return Some(id.to_string());
    }
    if v.is_string_like() {
        return vm.bx.heap.cast_to_owned_string(v, "graph pass");
    }
    None
}

fn list(vm: &ScriptVm, v: ScriptValue) -> Vec<ScriptValue> {
    let h = &vm.bx.heap;
    if let Some(a) = v.as_array() {
        return (0..h.array_len(a)).map(|i| h.array_index(a, i, NoTrap)).collect();
    }
    if let Some(o) = v.as_object() {
        return (0..h.vec_len(o)).map(|i| h.vec_value(o, i, NoTrap)).collect();
    }
    Vec::new()
}

/// The own field names of an object, in order.
pub fn own_fields(vm: &ScriptVm, obj: ScriptObject) -> Vec<String> {
    let h = &vm.bx.heap;
    let mut out = Vec::new();
    h.map_ref(obj).iter().for_each(|(k, _)| {
        if let Some(id) = k.as_id() {
            out.push(id.to_string());
        }
    });
    for i in 0..h.vec_len(obj) {
        let e = h.vec_key_value(obj, i, NoTrap);
        if let Some(id) = e.key.as_id() {
            out.push(id.to_string());
        }
    }
    out
}

/// Read one pass object (a document's `Pass{}` or a kit's pass). `label`
/// names it in diagnostics.
pub fn read_pass(vm: &mut ScriptVm, v: ScriptValue, label: &str) -> Result<PassRead, String> {
    let Some(o) = v.as_object() else { return Err(format!("{label}: a pass is an object: Pass{{reads: [@color] pixel: \"fn() -> vec4 {{ ... }}\"}}")) };
    let at = get(vm, o, "at");
    let stage = match text(vm, at) {
        None => Stage::Hdr,
        Some(s) => Stage::by_name(&s).ok_or_else(|| format!("{label}: `at: @{s}` is not a stage; one of @pre @hdr @display @final"))?,
    };
    let name_v = get(vm, o, "name");
    let name = text(vm, name_v);
    let mut reads = Vec::new();
    let reads_v = get(vm, o, "reads");
    let read_items = if reads_v.is_nil() { Vec::new() } else { list(vm, reads_v) };
    for r in read_items {
        match text(vm, r) {
            Some(s) => reads.push(s),
            None => return Err(format!("{label}: `reads` lists @color, @depth, @glow, ... or pass names")),
        }
    }
    if reads_v.is_nil() {
        reads.push("color".to_string());
    }
    let mut slots = Vec::new();
    let slots_v = get(vm, o, "slots");
    for s in list(vm, slots_v) {
        slots.push(text(vm, s).ok_or_else(|| format!("{label}: `slots` are names"))?);
    }
    let scale_v = get(vm, o, "scale");
    let scale = if scale_v.is_nil() { 1.0 } else { scale_v.as_number().ok_or_else(|| format!("{label}: `scale` is a number (0.5 = half resolution)"))? as f32 };
    let size_v = get(vm, o, "size");
    let size = if size_v.is_nil() {
        None
    } else {
        match makepad_script::numeric::NumericValue::from_script_value_heap(&vm.bx.heap, size_v, Default::default()) {
            makepad_script::numeric::NumericValue::Vec2(v) if v.x >= 1.0 && v.y >= 1.0 => Some((v.x as u32, v.y as u32)),
            _ => return Err(format!("{label}: `size` is the output in pixels: vec2(512, 256)")),
        }
    };
    let format_v = get(vm, o, "format");
    let format = match text(vm, format_v) {
        None => None,
        Some(f) => Some(Format::by_name(&f).ok_or_else(|| format!("{label}: `format: @{f}` is not a format; one of @rgba16f @rgba32f @rgba8"))?),
    };
    let pixel_v = get(vm, o, "pixel");
    let pixel = text(vm, pixel_v).ok_or_else(|| format!("{label}: a pass needs `pixel: \"fn() -> vec4 {{ ... }}\"`"))?;
    let helpers_v = get(vm, o, "helpers");
    let helpers = text(vm, helpers_v).unwrap_or_default();
    let mut uniforms = Vec::new();
    let uv = get(vm, o, "uniforms");
    if let Some(uo) = uv.as_object() {
        for n in own_fields(vm, uo) {
            let val = get(vm, uo, &n);
            uniforms.push((n, val));
        }
    }
    let history = get(vm, o, "history").as_bool().unwrap_or(false);
    // `outputs: [@gbuf1, {name: "depth" format: @r32f}]`: further outputs
    // (linear half float unless given), written as `self.<name>`.
    let mut outputs = Vec::new();
    let outputs_v = get(vm, o, "outputs");
    for item in list(vm, outputs_v) {
        let (name, format) = match item.as_object().filter(|_| item.as_id().is_none() && !item.is_string_like()) {
            Some(oo) => {
                let n = get(vm, oo, "name");
                let f = get(vm, oo, "format");
                (text(vm, n), text(vm, f))
            }
            None => (text(vm, item), None),
        };
        let Some(name) = name else { return Err(format!("{label}: `outputs` are names or {{name format}}")) };
        let format = match format {
            None => Format::Rgba16f,
            Some(f) => Format::by_name(&f).ok_or_else(|| format!("{label}: output `{name}`: `@{f}` is not a format; one of @rgba16f @rgba32f @r32f"))?,
        };
        outputs.push(crate::pass::OutputDecl { slot: name.clone(), name, format });
    }
    let decl = PassDecl { name, stage, reads, slots, scale, size, format, uniforms: Vec::new(), pixel, helpers, history, outputs, label: label.to_string() };
    Ok(PassRead { decl, uniforms })
}

/// Install the kits into module `module` (whose kit types the host has
/// registered, see [`crate::kits::KITS`]): all, or the kinds in `only`.
/// Returns the evaluation errors.
pub fn install_kits(vm: &mut ScriptVm, module: &str, only: Option<&[&str]>) -> Vec<String> {
    vm.bx.captured_errors = Some(Vec::new());
    let v = vm.eval(ScriptMod { file: "graph://kits".into(), code: crate::kits::kit_source(module, only), ..Default::default() });
    let mut errors = vm.take_errors();
    if v.is_err() && errors.is_empty() {
        errors.push("the graph kits did not evaluate".into());
    }
    errors
}
