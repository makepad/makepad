//! Reading passes from Splash documents: the fields every host reads the
//! same way. A host (motion3d, Motion, Sandbox) finds a `Pass{}` or a kit
//! in its document, calls a kit's template ([`crate::kits`]) when it is
//! one, and hands each pass object to [`read_pass`]. The uniforms come back
//! as script values: the host evaluates them with its own keyables (every
//! post parameter is read once per frame, at the frame point) and sets
//! each uniform's width from what it evaluates to.

use crate::pass::{PassCode, PassDecl, PassFn, PassHelper};
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
pub const PASS_FIELDS: &[&str] = &["outputs", "at", "name", "reads", "slots", "scale", "scale_height", "preview_scale", "size", "format", "uniforms", "pixel", "helpers", "history", "map"];

fn get(vm: &ScriptVm, o: ScriptObject, name: &str) -> ScriptValue {
    let v = vm.value(o, LiveId::from_str(name));
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
        return vm.cast_to_owned_string(v, "graph pass");
    }
    None
}

/// A Splash function's text (`fn(..) -> .. { .. }`) and where it is written,
/// or `None` when `v` is no script function.
pub fn fn_text(vm: &ScriptVm, v: ScriptValue) -> Option<(crate::pass::CodeAt, String)> {
    let o = v.as_object()?;
    let ip = vm.heap().fn_script_ip(o)?;
    let (loc, text) = vm.fn_text(ip)?;
    Some((crate::pass::CodeAt { file: loc.file().to_owned(), line: loc.line(), col: loc.col() }, text))
}

/// A Splash function as a pass's code (moved to the GPU's VM by its tokens,
/// [`PassFn`]), or `None` when `v` is no script function.
pub fn fn_code(vm: &ScriptVm, v: ScriptValue) -> Option<PassFn> {
    let o = v.as_object()?;
    vm.fn_image(o).map(PassFn::new)
}

/// A pass's `helpers` (see [`helper_fns`]) as its code.
pub fn helper_codes(vm: &ScriptVm, v: ScriptValue) -> Result<Vec<PassHelper>, String> {
    if v.is_nil() {
        return Ok(Vec::new());
    }
    let Some(o) = v.as_object().filter(|o| !vm.is_fn(*o)) else {
        return Err("`helpers` is an object of shader functions: `helpers: {name: fn(x: float) -> float { ... }}`".into());
    };
    let mut chain = vec![o];
    while let Some(p) = vm.heap().proto(*chain.last().unwrap()).as_object() {
        if chain.len() > 16 || vm.is_fn(p) {
            break;
        }
        chain.push(p);
    }
    let mut out: Vec<PassHelper> = Vec::new();
    for level in chain.iter().rev() {
        // (A pass's `pixel` is never a helper: an object of hooks may hold
        // the step it runs as its `pixel` too.)
        for n in own_fields(vm, *level).into_iter().filter(|n| n != "pixel") {
            let f = get(vm, *level, &n);
            let Some(code) = fn_code(vm, f) else {
                return Err(format!("helper `{n}` is a shader function: `helpers: {{{n}: fn(x: float) -> float {{ ... }}}}`"));
            };
            out.retain(|h| h.name != n);
            out.push(PassHelper { name: n, code: PassCode::Fn(code), namespace: None });
        }
    }
    Ok(out)
}

/// A pass's `helpers`: an object of shader functions, its own and those it
/// derives from (`helpers: engrave_helpers{mine: fn(..) {..}}`; a name
/// given again replaces the one it derives), as (name, where, text).
pub fn helper_fns(vm: &ScriptVm, v: ScriptValue) -> Result<Vec<(String, crate::pass::CodeAt, String)>, String> {
    if v.is_nil() {
        return Ok(Vec::new());
    }
    let Some(o) = v.as_object().filter(|o| !vm.is_fn(*o)) else {
        return Err("`helpers` is an object of shader functions: `helpers: {name: fn(x: float) -> float { ... }}`".into());
    };
    let mut chain = vec![o];
    while let Some(p) = vm.heap().proto(*chain.last().unwrap()).as_object() {
        if chain.len() > 16 || vm.is_fn(p) {
            break;
        }
        chain.push(p);
    }
    let mut out: Vec<(String, crate::pass::CodeAt, String)> = Vec::new();
    for level in chain.iter().rev() {
        // (A pass's `pixel` is never a helper: an object of hooks may hold
        // the step it runs as its `pixel` too.)
        for n in own_fields(vm, *level).into_iter().filter(|n| n != "pixel") {
            let f = get(vm, *level, &n);
            let Some((at, text)) = fn_text(vm, f) else {
                return Err(format!("helper `{n}` is a shader function: `helpers: {{{n}: fn(x: float) -> float {{ ... }}}}`"));
            };
            out.retain(|(m, _, _)| *m != n);
            out.push((n, at, text));
        }
    }
    Ok(out)
}

fn list(vm: &ScriptVm, v: ScriptValue) -> Vec<ScriptValue> {
    let h = vm.heap();
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
    let h = vm.heap();
    let mut out = Vec::new();
    h.map_entries(obj).into_iter().for_each(|e| {
        let k = e.key;
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
    let Some(o) = v.as_object() else { return Err(format!("{label}: a pass is an object: Pass{{reads: [@color] pixel: fn() -> vec4 {{ ... }}}}")) };
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
    let preview_v = get(vm, o, "preview_scale");
    let preview_scale = if preview_v.is_nil() { 1.0 } else { preview_v.as_number().filter(|s| *s > 0.0 && *s <= 1.0).ok_or_else(|| format!("{label}: `preview_scale` is a number in (0, 1]: the pass's resolution in a realtime preview (0.5 = half)"))? as f32 };
    let scale_height_v = get(vm, o, "scale_height");
    let scale_height = if scale_height_v.is_nil() { None } else { Some(scale_height_v.as_number().filter(|h| *h >= 1.0).ok_or_else(|| format!("{label}: `scale_height` is a frame height in pixels (1080: `scale` is of a 1080p frame)"))? as f32) };
    let size_v = get(vm, o, "size");
    let size = if size_v.is_nil() {
        None
    } else {
        match makepad_script::numeric::NumericValue::from_script_value_heap(vm.heap(), size_v, Default::default()).as_vec2() {
            Some(v) if v.x >= 1.0 && v.y >= 1.0 => Some((v.x as u32, v.y as u32)),
            _ => return Err(format!("{label}: `size` is the output in pixels: vec2(512, 256)")),
        }
    };
    let format_v = get(vm, o, "format");
    let format = match text(vm, format_v) {
        None => None,
        Some(f) => Some(Format::by_name(&f).ok_or_else(|| format!("{label}: `format: @{f}` is not a format; one of @rgba16f @rgba32f @rgba8"))?),
    };
    // `pixel: fn() -> vec4 { ... }` and `helpers: {name: fn(..) -> .. {..}}`:
    // Splash functions in the document, compiled as the pass's shader (moved
    // by their tokens, so their errors point at the document).
    let pixel_v = get(vm, o, "pixel");
    let helpers_v = get(vm, o, "helpers");
    let Some(pixel) = fn_code(vm, pixel_v) else {
        return Err(format!("{label}: a pass needs `pixel: fn() -> vec4 {{ ... }}`, a shader function"));
    };
    let pixel = PassCode::Fn(pixel);
    let helpers = helper_codes(vm, helpers_v).map_err(|e| format!("{label}: {e}"))?;
    let mut uniforms = Vec::new();
    let uv = get(vm, o, "uniforms");
    if let Some(uo) = uv.as_object() {
        for n in own_fields(vm, uo) {
            let val = get(vm, uo, &n);
            uniforms.push((n, val));
        }
    }
    let history = get(vm, o, "history").as_bool().unwrap_or(false);
    let map = get(vm, o, "map").as_bool().unwrap_or(false);
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
    let decl = PassDecl { name, stage, reads, slots, scale, scale_height, preview_scale, size, format, uniforms: Vec::new(), pixel, helpers, history, outputs, label: label.to_string(), map, live_literals: false };
    Ok(PassRead { decl, uniforms })
}

/// The tag a post type carries (`__post: glow` on the `Glow` a document
/// derives from): what [`post_kind`] reads.
pub const POST_TAG: &str = "__post";

/// Put the post types `types` (Splash name, kind) in `module`, each an
/// object tagged with its kind; [`install_kits`] then gives the graph's
/// kits among them their defaults and templates.
pub fn register_post_types(vm: &mut ScriptVm, module: ScriptObject, types: &[(&str, &str)]) {
    for (name, kind) in types {
        let proto = vm.new_object();
        let tag = LiveId::from_str_with_lut(kind).unwrap_or(LiveId::from_str(kind));
        vm.set_value_def(proto, LiveId::from_str(POST_TAG), tag);
        vm.set_value_def(module, LiveId::from_str(name), proto);
    }
}

/// `mod.post`: the render graph's post types (`Pass`, `MotionBlur` and every
/// kit of [`crate::kits::KITS`]: `Glow`, `Halation`, `Bloom`, ...), tagged
/// with their kind, with the kits' defaults and templates. A VM has one,
/// whoever installs it first (a Motion document, a standalone scene); a
/// document uses it by name (`use mod.post`, then `post.Glow{..}`). The
/// evaluation errors.
pub fn install_post(vm: &mut ScriptVm) -> Vec<String> {
    if vm.value(vm.modules(), id!(post)).as_object().is_some() {
        return Vec::new();
    }
    let post = vm.new_module(id!(post));
    let mut types = vec![("Pass", "pass"), ("MotionBlur", "motion_blur")];
    types.extend(crate::kits::KITS.iter().map(|kit| (kit.name, kit.kind)));
    register_post_types(vm, post, &types);
    let mut errors = install_kits(vm, "mod.post", None);
    vm.capture_errors();
    vm.eval(ScriptMod::new("", "", "mod.post", 0, 0, "mod.post.MotionBlur = mod.post.MotionBlur{shutter: 180 samples: @auto tol: 3}\ntrue\n", &[]));
    errors.extend(vm.take_errors());
    errors
}

/// The post kind of a document's entry (its type's [`POST_TAG`]).
pub fn post_kind(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    let o = v.as_object()?;
    get(vm, o, POST_TAG).as_id().map(|k| k.to_string())
}

/// The entries of a `post:` list (an array or an object's vector part).
pub fn post_items(vm: &ScriptVm, v: ScriptValue) -> Vec<ScriptValue> {
    list(vm, v)
}

/// One entry of a document's `post:`, read the same way by every host.
pub enum PostEntry {
    /// A `Pass{}` (as written), or the passes a kit's template built (their
    /// names prefixed, so two kits never collide).
    Passes(Vec<PassRead>),
    /// One of the host's own post types, which the host reads.
    Host(ScriptObject),
}

/// Read the post entry `item` of kind `kind` (`"pass"`, a kit whose template
/// `kit_<kind>` is in `module`, or else the host's own). `label` names it in
/// errors; a kit's pass names get `prefix`.
pub fn read_post_entry(vm: &mut ScriptVm, module: ScriptObject, kind: &str, item: ScriptValue, label: &str, prefix: &str) -> Result<PostEntry, String> {
    let Some(o) = item.as_object() else { return Err(format!("{label} is not a post effect (Glow{{..}}, Pass{{..}}, ...)")) };
    if kind == "pass" {
        return Ok(PostEntry::Passes(vec![read_pass(vm, item, &format!("{label} Pass"))?]));
    }
    let template = get(vm, module, &format!("kit_{kind}"));
    if template.is_nil() {
        return Ok(PostEntry::Host(o));
    }
    vm.capture_errors();
    let built = vm.call(template, &[item]);
    let errors = vm.take_errors();
    if !errors.is_empty() || built.is_err() {
        return Err(format!("{label}: the {kind} kit did not build: {}", if errors.is_empty() { "it did not evaluate".to_string() } else { errors.join("; ") }));
    }
    let objs = list(vm, built);
    if objs.is_empty() {
        return Err(format!("{label}: the {kind} kit built no passes"));
    }
    let mut passes = Vec::new();
    for (j, v) in objs.into_iter().enumerate() {
        passes.push(read_pass(vm, v, &format!("{label} {kind}[{j}]"))?);
    }
    let mut decls: Vec<PassDecl> = passes.iter().map(|p| p.decl.clone()).collect();
    crate::pass::namespace(&mut decls, prefix);
    for (p, d) in passes.iter_mut().zip(decls) {
        p.decl = d;
    }
    Ok(PostEntry::Passes(passes))
}

/// Install the kits into module `module` (whose kit types the host has
/// registered, see [`crate::kits::KITS`]): all, or the kinds in `only`.
/// Returns the evaluation errors.
pub fn install_kits(vm: &mut ScriptVm, module: &str, only: Option<&[&str]>) -> Vec<String> {
    vm.capture_errors();
    // A file of its own per module and selection: the kits' functions keep
    // pointing into their own code when another module installs them too.
    let file = format!("graph://kits/{module}/{}", only.map_or("all".to_string(), |o| o.join(",")));
    let v = vm.eval(ScriptMod::new("", "", &file, 0, 0, &(crate::kits::kit_source(module, only)), &[]));
    let mut errors = vm.take_errors();
    if v.is_err() && errors.is_empty() {
        errors.push("the graph kits did not evaluate".into());
    }
    errors
}
