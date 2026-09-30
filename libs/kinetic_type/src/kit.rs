//! Reading a kit: one `Kinetic{...}` Splash document.
//!
//! ```text
//! fn lift(g, h) { ... }                 // top-level fns: kernel helpers (CPU)
//! Kinetic{
//!     name: "Wave Line"  text: "KINETIC TEXT"  case: @upper  font: @bold
//!     size: 1.0  depth: 0.35  bevel: 0.03  bevel_type: @round  material: @metal
//!     layout: @line  wrap: 12  align: @center  line_gap: 1.2  tracking: 0.0
//!     copies: 1  alphabet: "#%&"  cells: {res: 12 layers: 2 fill: @block}
//!     colors: {bg: #05060d a: #ffc84a b: #2a1450 c: #49e6ff}
//!     dials: {swing: 0.0 drive: 0.0 split: 0.5}  // p1.. in order, 0..1
//!     camera: {fov: 50 dist: 9 height: 0}   // dist, height in cap heights
//!     floor: {y: -0.7 size: 14}  backdrop: true
//!     post: [Glow{threshold: 0.62 strength: 0.9}]
//!     glyph: fn(g, o) { ... }            // the animator: a kernel (CPU)
//!     camera_fn: fn(c) { ... }           // optional camera kernel (CPU)
//!     look: fn() -> vec4 { ... }         // every other fn: the glyph shader
//! }
//! ```
//!
//! `glyph` and `camera_fn` (and the top-level fns) are kernel source: they
//! are cut out of the text before the document VM sees it (their lines kept
//! blank, so every diagnostic keeps its line) and compiled by
//! makepad-script-compute. Every other `name: fn` field is a member of the
//! kit's subclass of `DrawKineticGlyph` (`look`, `deform`, `floor`,
//! helpers) or, for `backdrop`, of `DrawKineticBackdrop`. The rest is read
//! as values.

/// A function cut out of the kit: its parameter names, its body text and
/// the line its body starts on (1-based).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FnSrc {
    pub params: Vec<String>,
    /// The whole `fn(...) -> T { ... }` text.
    pub text: String,
    /// The text between the outer braces.
    pub body: String,
    pub line: usize,
}

/// The kit text taken apart.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Split {
    /// Kernel items before `Kinetic{` (fns, consts, `use` lines).
    pub kernel_items: String,
    pub glyph: Option<FnSrc>,
    pub camera: Option<FnSrc>,
    /// Shader members of the glyph draw (`name: fn ...`), in order.
    pub shader: Vec<(String, FnSrc)>,
    /// `backdrop: fn() -> vec4 {...}`.
    pub backdrop: Option<FnSrc>,
    /// The document with every fn cut out, for the VM.
    pub values: String,
}

fn skip_trivia(b: &[u8], i: usize) -> usize {
    if b[i] == b'"' {
        let mut j = i + 1;
        while j < b.len() && b[j] != b'"' {
            j += if b[j] == b'\\' { 2 } else { 1 };
        }
        return (j + 1).min(b.len());
    }
    if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
        let mut j = i;
        while j < b.len() && b[j] != b'\n' {
            j += 1;
        }
        return j;
    }
    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
        let mut j = i + 2;
        while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
            j += 1;
        }
        return (j + 2).min(b.len());
    }
    i
}

/// The index after the bracket that closes the opener at `i`.
fn skip_balanced(b: &[u8], i: usize) -> usize {
    let mut depth = 0i32;
    let mut j = i;
    while j < b.len() {
        let k = skip_trivia(b, j);
        if k != j {
            j = k;
            continue;
        }
        match b[j] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    b.len()
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn line_of(b: &[u8], at: usize) -> usize {
    1 + b[..at.min(b.len())].iter().filter(|c| **c == b'\n').count()
}

/// Blank `range` keeping its newlines.
fn blank(out: &mut [u8], from: usize, to: usize) {
    for c in &mut out[from..to] {
        if *c != b'\n' {
            *c = b' ';
        }
    }
}

/// Reads `fn(a, b) -> T { body }` starting at `i` (at the `fn`); returns
/// the function and the index after it.
fn read_fn(src: &str, i: usize) -> Result<(FnSrc, usize), String> {
    let b = src.as_bytes();
    let mut j = i + 2;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if b.get(j) != Some(&b'(') {
        return Err(format!("line {}: `fn` needs its parameters: `fn(g, o) {{ ... }}`", line_of(b, i)));
    }
    let close = skip_balanced(b, j);
    let params = src[j + 1..close - 1].split(',').map(|p| p.split(':').next().unwrap_or("").trim().to_string()).filter(|p| !p.is_empty()).collect();
    let mut k = close;
    while k < b.len() && b[k] != b'{' {
        if b[k] == b'}' || b[k] == b']' {
            return Err(format!("line {}: a `fn` without a body", line_of(b, i)));
        }
        k += 1;
    }
    let end = skip_balanced(b, k);
    let body = src[k + 1..end.saturating_sub(1)].to_string();
    Ok((FnSrc { params, text: src[i..end].to_string(), body, line: line_of(b, k) }, end))
}

/// Takes the kit text apart (§ module docs).
pub fn split(src: &str) -> Result<Split, String> {
    let b = src.as_bytes();
    let mut out = src.as_bytes().to_vec();
    let mut split = Split::default();
    // Find `Kinetic{` at the top level.
    let mut i = 0;
    let mut open = None;
    while i < b.len() {
        let k = skip_trivia(b, i);
        if k != i {
            i = k;
            continue;
        }
        if b[i..].starts_with(b"Kinetic") && (i == 0 || !is_ident(b[i - 1])) {
            let mut j = i + 7;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if b.get(j) == Some(&b'{') {
                open = Some((i, j));
                break;
            }
        }
        if b[i] == b'{' || b[i] == b'(' || b[i] == b'[' {
            i = skip_balanced(b, i);
            continue;
        }
        i += 1;
    }
    let Some((start, brace)) = open else { return Err("a kit is one `Kinetic{ ... }` object".into()) };
    split.kernel_items = src[..start].to_string();
    blank(&mut out, 0, start);
    let end = skip_balanced(b, brace);
    // The object's own fields: `name: fn`.
    let mut i = brace + 1;
    while i < end - 1 {
        let k = skip_trivia(b, i);
        if k != i {
            i = k;
            continue;
        }
        let c = b[i];
        if c == b'{' || c == b'[' || c == b'(' {
            i = skip_balanced(b, i);
            continue;
        }
        if (c.is_ascii_alphabetic() || c == b'_') && !is_ident(b[i - 1]) {
            let s = i;
            while i < b.len() && is_ident(b[i]) {
                i += 1;
            }
            let name = &src[s..i];
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if b.get(j) == Some(&b':') {
                let mut v = j + 1;
                while v < b.len() && b[v].is_ascii_whitespace() {
                    v += 1;
                }
                if b[v..].starts_with(b"fn") && !b.get(v + 2).is_some_and(|c| is_ident(*c)) {
                    let (f, after) = read_fn(src, v)?;
                    blank(&mut out, s, after);
                    match name {
                        "glyph" => split.glyph = Some(f),
                        "camera_fn" => split.camera = Some(f),
                        "backdrop" => split.backdrop = Some(f),
                        _ => split.shader.push((name.to_string(), f)),
                    }
                    i = after;
                }
            }
            continue;
        }
        i += 1;
    }
    split.values = String::from_utf8(out).map_err(|_| "the kit is not UTF-8".to_string())?;
    Ok(split)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fns_are_cut_out_and_lines_kept() {
        let src = "// a kit\nfn lift(g) { g.t * 2.0 }\nKinetic{\n    name: \"x // not a comment\"\n    dials: {a: 0.5}\n    glyph: fn(g, o) {\n        o.pos.y = lift(g)\n    }\n    look: fn() -> vec4 { return vec4(1.0, 0.0, 0.0, 1.0) }\n    backdrop: fn() -> vec4 { return self.col_bg }\n    size: 2.0\n}\n";
        let s = split(src).unwrap();
        assert!(s.kernel_items.contains("fn lift"));
        let g = s.glyph.unwrap();
        assert_eq!(g.params, vec!["g", "o"]);
        assert!(g.body.contains("o.pos.y = lift(g)"));
        assert_eq!(g.line, 6);
        assert_eq!(s.shader.len(), 1);
        assert_eq!(s.shader[0].0, "look");
        assert!(s.shader[0].1.text.starts_with("fn() -> vec4 {"));
        assert!(s.backdrop.is_some());
        assert_eq!(s.values.lines().count(), src.lines().count());
        assert!(s.values.contains("size: 2.0") && !s.values.contains("glyph") && !s.values.contains("fn lift"));
        assert!(split("Nope{}").is_err());
    }
}

// ---------------------------------------------------------------------------
// The values (read through the document VM)
// ---------------------------------------------------------------------------

use crate::shapes::{CellSpec, ShapeSpec};
use makepad_draw::*;
use makepad_text_mesh::letters::{BevelProfile, FontSource, TextAlign};

/// The kit's type settings, palette, camera, floor and passes.
#[derive(Clone, Debug)]
pub struct KitValues {
    pub name: String,
    pub text: String,
    pub upper: bool,
    pub lower: bool,
    /// The shape settings (its `text` is set per text).
    pub shape: ShapeSpec,
    pub copies: usize,
    /// bg, a, b, c.
    pub colors: [Vec4f; 4],
    /// 0 matte, 1 metal, 2 neon, 3 plastic, 4 glass, 5 holo.
    pub material: f32,
    pub dials: Vec<(String, f32)>,
    pub fov: f32,
    /// Camera distance in cap heights; None = frame the text.
    pub dist: Option<f32>,
    /// Camera height in cap heights; None = level (raised over a floor).
    pub height: Option<f32>,
    pub floor: Option<(Option<f32>, Option<f32>)>,
    /// `grow` runs 0..1 over this many beats (0: stays 1).
    pub cycle_beats: f32,
    pub pingpong: bool,
    pub passes: Vec<makepad_render_graph::PassDecl>,
    pub pass_values: Vec<Vec<[f32; 4]>>,
}

fn field(vm: &ScriptVm, o: ScriptObject, name: &str) -> ScriptValue {
    let v = vm.bx.heap.value(o, LiveId::from_str(name).into(), NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn text_of(vm: &mut ScriptVm, v: ScriptValue) -> Option<String> {
    if v.is_nil() {
        return None;
    }
    if let Some(id) = v.as_id() {
        return Some(id.to_string());
    }
    if v.is_string_like() {
        return vm.bx.heap.cast_to_owned_string(v, "kinetic kit");
    }
    None
}

fn num(v: ScriptValue) -> Option<f32> {
    v.as_number().map(|n| n as f32).or_else(|| v.as_bool().map(|b| if b { 1.0 } else { 0.0 }))
}

fn color(v: ScriptValue) -> Option<Vec4f> {
    v.as_color().map(Vec4f::from_u32)
}

/// The names of `{a: .. b: ..}` under `key:` in the (fn-free) kit text, in
/// the order written.
fn ordered_keys(values: &str, key: &str) -> Vec<String> {
    let b = values.as_bytes();
    let pat = format!("{key}:");
    let mut at = 0;
    while let Some(k) = values[at..].find(&pat) {
        let s = at + k;
        at = s + pat.len();
        if s > 0 && is_ident(b[s - 1]) {
            continue;
        }
        let mut j = at;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'{') {
            continue;
        }
        let end = skip_balanced(b, j);
        let mut out = Vec::new();
        let mut i = j + 1;
        while i < end - 1 {
            let k = skip_trivia(b, i);
            if k != i {
                i = k;
                continue;
            }
            if b[i] == b'{' || b[i] == b'[' || b[i] == b'(' {
                i = skip_balanced(b, i);
                continue;
            }
            if (b[i].is_ascii_alphabetic() || b[i] == b'_') && !is_ident(b[i - 1]) {
                let s = i;
                while is_ident(b[i]) {
                    i += 1;
                }
                let mut j = i;
                while b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if b[j] == b':' {
                    out.push(values[s..i].to_string());
                }
                continue;
            }
            i += 1;
        }
        return out;
    }
    Vec::new()
}

/// The module kits are read in: `Kinetic` plus the graph's post kits.
pub const KIT_MODULE: &str = "kin";

/// Registers the kit module (once per VM).
pub fn script_mod(vm: &mut ScriptVm) {
    let have = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str(KIT_MODULE).into(), NoTrap).as_object().is_some();
    if have {
        return;
    }
    let m = vm.new_module(LiveId::from_str(KIT_MODULE));
    let proto = vm.bx.heap.new_object();
    vm.bx.heap.set_value_def(m, LiveId::from_str("Kinetic").into(), proto.into());
    for k in makepad_render_graph::kits::KITS {
        let proto = vm.bx.heap.new_object();
        vm.bx.heap.set_value_def(proto, LiveId::from_str("__kind").into(), LiveId::from_str(k.kind).into());
        vm.bx.heap.set_value_def(m, LiveId::from_str(k.name).into(), proto.into());
    }
    let pass = vm.bx.heap.new_object();
    vm.bx.heap.set_value_def(m, LiveId::from_str("Pass").into(), pass.into());
    for e in makepad_render_graph::script::install_kits(vm, &format!("mod.{KIT_MODULE}"), None) {
        log!("kinetic: graph kits: {e}");
    }
}

/// Reads the kit's values (`split.values`) through the VM.
pub fn read_values(vm: &mut ScriptVm, split: &Split, file: &str) -> Result<KitValues, String> {
    script_mod(vm);
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!("use mod.{KIT_MODULE}.*\nuse mod.math.*\nuse mod.pod.*\n{}", split.values);
    let v = vm.eval(ScriptMod { file: file.to_string(), code, ..Default::default() });
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let o = v.as_object().ok_or_else(|| "the kit did not evaluate to a Kinetic{} object".to_string())?;
    let mut shape = ShapeSpec::default();
    let s = |vm: &mut ScriptVm, n: &str| {
        let v = field(vm, o, n);
        text_of(vm, v)
    };
    let f = |vm: &ScriptVm, n: &str| num(field(vm, o, n));
    if let Some(font) = s(vm, "font") {
        shape.font = if font.contains('/') || font.contains('.') { FontSource::Path(font.into()) } else { FontSource::Bundled(font) };
    }
    shape.weight = f(vm, "weight");
    // Variable-font axes by four-letter tag: `axes: {wdth: 125 slnt: -8}`
    // (`wght` is `weight`).
    let axes = field(vm, o, "axes");
    if let Some(a) = axes.as_object() {
        for tag in ordered_keys(&split.values, "axes") {
            let Some(v) = num(field(vm, a, &tag)) else { continue };
            let b = tag.as_bytes();
            if b.len() != 4 {
                return Err(format!("axes: `{tag}` is not a four-letter axis tag (wdth, wght, slnt, opsz, GRAD, ...)"));
            }
            if tag == "wght" {
                shape.weight = Some(v);
            } else {
                shape.axes.push((u32::from_be_bytes([b[0], b[1], b[2], b[3]]), v));
            }
        }
    }
    if let Some(v) = f(vm, "size") {
        shape.size = v.max(0.001);
    }
    if let Some(v) = f(vm, "depth") {
        shape.depth = v.max(0.0);
    }
    if let Some(v) = f(vm, "bevel") {
        shape.bevel = v.max(0.0);
    }
    if let Some(b) = s(vm, "bevel_type") {
        shape.bevel_profile = BevelProfile::by_name(&b).ok_or_else(|| format!("bevel_type: @{b} is not one of {}", BevelProfile::NAMES.join(", ")))?;
        if shape.bevel == 0.0 && b != "flat" {
            shape.bevel = (shape.depth * 0.25).min(shape.size * 0.06);
        }
        if matches!(shape.bevel_profile, BevelProfile::Round | BevelProfile::Cove | BevelProfile::Ogee) {
            shape.bevel_segments = 4;
        }
    }
    if let Some(v) = f(vm, "bevel_rings") {
        shape.bevel_segments = v.clamp(1.0, 8.0) as u32;
    }
    if let Some(v) = f(vm, "detail") {
        shape.detail = v;
    }
    if let Some(v) = f(vm, "tracking") {
        shape.tracking = v;
    }
    if let Some(v) = f(vm, "line_gap") {
        shape.line_height = v;
    }
    if let Some(v) = f(vm, "wrap") {
        shape.wrap = Some(v);
    }
    if let Some(a) = s(vm, "align") {
        shape.align = match a.as_str() {
            "left" => TextAlign::Left,
            "right" => TextAlign::Right,
            _ => TextAlign::Center,
        };
    }
    if let Some(a) = s(vm, "alphabet") {
        shape.alphabet = a;
    }
    let cells = field(vm, o, "cells");
    if let Some(c) = cells.as_object() {
        let fill = {
            let v = field(vm, c, "fill");
            text_of(vm, v)
        };
        shape.cells = Some(CellSpec {
            res: num(field(vm, c, "res")).unwrap_or(12.0) as u32,
            layers: num(field(vm, c, "layers")).unwrap_or(2.0) as u32,
            block: fill.as_deref() == Some("block"),
        });
    }
    let case = s(vm, "case");
    let mut colors = [vec4(0.02, 0.02, 0.04, 1.0), vec4(1.0, 1.0, 1.0, 1.0), vec4(0.2, 0.2, 0.3, 1.0), vec4(1.0, 0.45, 0.2, 1.0)];
    let cv = field(vm, o, "colors");
    if let Some(c) = cv.as_object() {
        for (k, n) in ["bg", "a", "b", "c"].iter().enumerate() {
            if let Some(v) = color(field(vm, c, n)) {
                colors[k] = v;
            }
        }
    }
    let material = match s(vm, "material").as_deref() {
        None | Some("matte") => 0.0,
        Some("metal") | Some("chrome") | Some("gold") => 1.0,
        Some("neon") => 2.0,
        Some("plastic") => 3.0,
        Some("glass") => 4.0,
        Some("holo") => 5.0,
        Some(other) => return Err(format!("material: @{other} is not one of matte, metal, neon, plastic, glass, holo")),
    };
    let mut dials = Vec::new();
    let dv = field(vm, o, "dials");
    if let Some(d) = dv.as_object() {
        for name in ordered_keys(&split.values, "dials") {
            dials.push((name.clone(), num(field(vm, d, &name)).unwrap_or(0.5)));
        }
    }
    let (mut fov, mut dist, mut height) = (50.0, None, None);
    let cam = field(vm, o, "camera");
    if let Some(c) = cam.as_object() {
        fov = num(field(vm, c, "fov")).unwrap_or(fov);
        dist = num(field(vm, c, "dist"));
        height = num(field(vm, c, "height"));
    }
    let fl = field(vm, o, "floor");
    let floor = fl.as_object().map(|c| (num(field(vm, c, "y")), num(field(vm, c, "size"))));
    let (mut cycle_beats, mut pingpong) = (0.0, false);
    let cy = field(vm, o, "cycle");
    if let Some(c) = cy.as_object() {
        cycle_beats = num(field(vm, c, "beats")).unwrap_or(4.0);
        pingpong = num(field(vm, c, "pingpong")).unwrap_or(0.0) > 0.5;
    }
    // Passes: kits call their template, `Pass{}`s are read as they are.
    let mut passes = Vec::new();
    let mut pass_values = Vec::new();
    let post = field(vm, o, "post");
    let items: Vec<ScriptValue> = match post.as_array() {
        Some(a) => (0..vm.bx.heap.array_len(a)).map(|i| vm.bx.heap.array_index(a, i, NoTrap)).collect(),
        None => Vec::new(),
    };
    let module = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str(KIT_MODULE).into(), NoTrap).as_object();
    for (k, item) in items.into_iter().enumerate() {
        let Some(io) = item.as_object() else { return Err(format!("post[{k}] is a kit (Glow{{..}}) or a Pass{{..}}")) };
        let kind = {
            let v = field(vm, io, "__kind");
            text_of(vm, v)
        };
        let objs: Vec<ScriptValue> = match (kind, module) {
            (Some(kind), Some(m)) => {
                let template = vm.bx.heap.value(m, LiveId::from_str(&format!("kit_{kind}")).into(), NoTrap);
                vm.bx.captured_errors = Some(Vec::new());
                let built = vm.call(template, &[item]);
                let errors = vm.take_errors();
                if !errors.is_empty() {
                    return Err(format!("post[{k}] ({kind}): {}", errors.join("; ")));
                }
                match built.as_array() {
                    Some(a) => (0..vm.bx.heap.array_len(a)).map(|i| vm.bx.heap.array_index(a, i, NoTrap)).collect(),
                    None => return Err(format!("post[{k}] ({kind}) built no passes")),
                }
            }
            _ => vec![item],
        };
        let mut decls = Vec::new();
        let mut vals = Vec::new();
        for (j, po) in objs.into_iter().enumerate() {
            let read = makepad_render_graph::script::read_pass(vm, po, &format!("post[{k}][{j}]"))?;
            let mut decl = read.decl;
            let mut v = Vec::new();
            for (name, val) in read.uniforms {
                let (width, value) = if let Some(n) = num(val) {
                    (1, [n, 0.0, 0.0, 0.0])
                } else if let Some(c) = color(val) {
                    (4, [c.x, c.y, c.z, c.w])
                } else {
                    return Err(format!("post[{k}][{j}]: uniform `{name}` is a number or a colour"));
                };
                decl.uniforms.push(makepad_render_graph::UniformDecl { name, width });
                v.push(value);
            }
            decl.validate().map_err(|e| format!("post[{k}][{j}]: {e}"))?;
            decls.push(decl);
            vals.push(v);
        }
        makepad_render_graph::pass::namespace(&mut decls, &format!("p{k}_"));
        passes.extend(decls);
        pass_values.extend(vals);
    }
    Ok(KitValues {
        name: s(vm, "name").unwrap_or_default(),
        text: s(vm, "text").unwrap_or_default(),
        upper: case.as_deref() == Some("upper"),
        lower: case.as_deref() == Some("lower"),
        shape,
        copies: f(vm, "copies").unwrap_or(1.0).clamp(1.0, 64.0) as usize,
        colors,
        material,
        dials,
        fov,
        dist,
        height,
        floor,
        cycle_beats,
        pingpong,
        passes,
        pass_values,
    })
}
