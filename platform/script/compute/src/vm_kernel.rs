//! Kernels made from Splash values: a host hands the compiler the
//! declarations it read from a document (typed, [`Decl`]) and the entry as
//! the VM's own fn object, and the compiler finds everything the entry
//! reaches through the VM: each name the fn uses is looked up in the scope
//! the fn was made in, the way the VM would at runtime. A script fn found
//! there is compiled in (with what it reaches in turn), a number, colour or
//! float vector becomes a constant. No host reads or cuts document text.
//!
//! ```ignore
//! // the document:  let R = 3.5
//! //                let spin = fn(p, t) { return rot2(p, t * R) }
//! //                let sparks = Kernel{ count: 600 out: output(Sprite)
//! //                                     element: fn(i) { out[i].pos = spin(..) } }
//! let k = vm_kernel::compile(vm, &VmKernel {
//!     decls: vec![Decl::Output { name: "out".into(), ty: "Sprite".into(), stride: None, offset: None, buffer: None }],
//!     entry: Entry::Element,
//!     entry_fn: element_fn_object,
//!     math: MathMode::Fast,
//! }, &layouts, Backend::Native, &[])?;
//! ```
//!
//! Errors carry the document location of the code they point into
//! ([`KernelSource::locate`]).

use crate::kernel::{self, Kernel, Layout, MathMode};
use crate::{Backend, ShaderError};
use makepad_script::*;
use std::collections::HashSet;
use std::fmt::Write;
use std::sync::Arc;

/// One top-level declaration of a kernel (the kernel language's
/// `let name = input(..)`, `output(..)`, `emit_buffer(..)`, `param(..)`).
/// `ty` is an element type (f32, i32, vec2, vec3, vec4, mat4) or the name of
/// a host layout.
#[derive(Clone, Debug, PartialEq)]
pub enum Decl {
    Input { name: String, ty: String, stride: Option<u32>, offset: Option<u32>, buffer: Option<String> },
    Output { name: String, ty: String, stride: Option<u32>, offset: Option<u32>, buffer: Option<String> },
    /// Records of `record` (a type, or a width in words when `width` is set)
    /// emitted per element, at most `capacity` each.
    EmitBuffer { name: String, record: Record, capacity: u32, buffer: Option<String> },
    Param { name: String, default: f32, range: Option<(f32, f32)> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Type(String),
    Words(u32),
}

impl Decl {
    pub fn name(&self) -> &str {
        match self {
            Decl::Input { name, .. } | Decl::Output { name, .. } | Decl::EmitBuffer { name, .. } | Decl::Param { name, .. } => name,
        }
    }
}

/// The entry kinds (`fn vertex(i)` …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Element,
    Vertex,
    Instance,
    Primitive,
    ReduceSum,
    ReduceMin,
    ReduceMax,
}

impl Entry {
    pub fn name(self) -> &'static str {
        match self {
            Entry::Element => "element",
            Entry::Vertex => "vertex",
            Entry::Instance => "instance",
            Entry::Primitive => "primitive",
            Entry::ReduceSum => "reduce_sum",
            Entry::ReduceMin => "reduce_min",
            Entry::ReduceMax => "reduce_max",
        }
    }

    pub fn from_name(name: &str) -> Option<Entry> {
        Some(match name {
            "element" => Entry::Element,
            "vertex" => Entry::Vertex,
            "instance" => Entry::Instance,
            "primitive" => Entry::Primitive,
            "reduce_sum" => Entry::ReduceSum,
            "reduce_min" => Entry::ReduceMin,
            "reduce_max" => Entry::ReduceMax,
            _ => return None,
        })
    }
}

pub struct VmKernel {
    pub decls: Vec<Decl>,
    pub entry: Entry,
    /// The entry: a script fn of one parameter (the element index).
    pub entry_fn: ScriptObject,
    pub math: MathMode,
}

/// The program the compiler reads, and where each part of it came from.
#[derive(Clone, Debug)]
pub struct KernelSource {
    pub text: String,
    /// (start in `text`, the fn's document location) per compiled-in fn.
    chunks: Vec<(usize, String, u32, u32)>,
}

impl KernelSource {
    /// The document location (file, 1-based line and column) of a byte
    /// offset into [`Self::text`], when it falls inside a fn taken from the
    /// document.
    pub fn locate(&self, at: usize) -> Option<(String, u32, u32)> {
        let (start, file, line, col) = self.chunks.iter().rev().find(|c| c.0 <= at)?;
        let end = start + self.text[*start..].find("\n}\n").map_or(self.text.len() - start, |e| e + 2);
        if at > end {
            return None;
        }
        let before = &self.text[*start..at];
        let lines = before.matches('\n').count() as u32;
        let col = if lines == 0 { col + before.len() as u32 } else { before.len() as u32 - before.rfind('\n').map_or(0, |i| i as u32 + 1) + 1 };
        Some((file.clone(), line + lines, col))
    }

    /// `message (file:line:col)` for an error, when its place is known.
    pub fn describe(&self, e: &ShaderError) -> String {
        match self.locate(e.start) {
            Some((file, line, col)) => format!("{} ({file}:{line}:{col})", e.message),
            None => e.message.clone(),
        }
    }
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "var", "if", "else", "for", "in", "while", "loop", "break", "continue", "return", "true", "false", "use", "as", "match", "self",
];

/// The source of a script fn: its document location and text, with the
/// text starting at its parameter list (`(i) { … }`).
fn fn_source(vm: &ScriptVm, f: ScriptObject) -> Option<(ScriptLoc, String)> {
    let Some(ScriptFnPtr::Script(ip)) = vm.bx.heap.as_fn(f) else { return None };
    let (mut loc, text) = vm.bx.code.fn_text(ip)?;
    let rest = text.trim_start().strip_prefix("fn")?.trim_start();
    // `fn name(..)` as well as `fn(..)`.
    let rest = rest.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_').trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    // The location of the parameter list (where the text taken starts),
    // with a 1-based line: the `fn` token's line is 0-based.
    let skipped = &text[..text.len() - rest.len()];
    loc.line += 1 + skipped.matches('\n').count() as u32;
    if let Some(nl) = skipped.rfind('\n') {
        loc.col = (skipped.len() - nl) as u32;
    } else {
        loc.col += skipped.len() as u32;
    }
    Some((loc, rest.to_string()))
}

/// A constant's Splash spelling, for the values a kernel can hold.
fn constant(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    fn num(x: f64) -> String {
        let s = format!("{x:?}");
        if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") }
    }
    if let Some(c) = v.as_color() {
        return Some(format!("#{c:08x}"));
    }
    if let Some(x) = v.as_number() {
        return x.is_finite().then(|| num(x));
    }
    let pod = v.as_pod()?;
    let (ty, data) = vm.bx.heap.pod_data(pod);
    let n = match ty.ty {
        pod::ScriptPodTy::Vec(pod::ScriptPodVec::Vec2f) => 2,
        pod::ScriptPodTy::Vec(pod::ScriptPodVec::Vec3f) => 3,
        pod::ScriptPodTy::Vec(pod::ScriptPodVec::Vec4f) => 4,
        _ => return None,
    };
    let lanes: Vec<String> = data.iter().take(n).map(|w| num(f32::from_bits(*w) as f64)).collect();
    (lanes.len() == n).then(|| format!("vec{n}({})", lanes.join(", ")))
}

/// The kernel program for `k`: its declarations, its entry and every
/// document fn and constant the entry reaches.
pub fn kernel_source(vm: &ScriptVm, k: &VmKernel) -> Result<KernelSource, ShaderError> {
    let mut text = String::new();
    let mut chunks = Vec::new();
    if k.math == MathMode::Portable {
        text.push_str("let math = portable\n");
    }
    let mut known: HashSet<String> = k.decls.iter().map(|d| d.name().to_string()).collect();
    known.insert(k.entry.name().to_string());
    for d in &k.decls {
        let opt = |s: &mut String, v: &Option<u32>| {
            if let Some(v) = v {
                let _ = write!(s, ", {v}");
            }
        };
        match d {
            Decl::Input { name, ty, stride, offset, buffer } | Decl::Output { name, ty, stride, offset, buffer } => {
                let f = if matches!(d, Decl::Input { .. }) { "input" } else { "output" };
                let mut args = ty.clone();
                if stride.is_some() || offset.is_some() || buffer.is_some() {
                    let _ = write!(args, ", {}", stride.unwrap_or(0));
                }
                if offset.is_some() || buffer.is_some() {
                    opt(&mut args, &Some(offset.unwrap_or(0)));
                }
                if let Some(b) = buffer {
                    let _ = write!(args, ", {b}");
                }
                let _ = writeln!(text, "let {name} = {f}({args})");
            }
            Decl::EmitBuffer { name, record, capacity, buffer } => {
                let rec = match record {
                    Record::Type(t) => t.clone(),
                    Record::Words(w) => w.to_string(),
                };
                let buf = buffer.as_ref().map(|b| format!(", {b}")).unwrap_or_default();
                let _ = writeln!(text, "let {name} = emit_buffer({rec}, {capacity}{buf})");
            }
            Decl::Param { name, default, range } => {
                let r = range.map(|(lo, hi)| format!(", {lo:?}, {hi:?}")).unwrap_or_default();
                let _ = writeln!(text, "let {name} = param({default:?}{r})");
            }
        }
    }
    // The entry, then what it reaches (breadth first; each name once).
    let Some((loc, src)) = fn_source(vm, k.entry_fn) else {
        return Err(ShaderError::new(0, 1, format!("the {} entry is not a script fn", k.entry.name())));
    };
    let mut todo = vec![(k.entry_fn, k.entry.name().to_string(), loc, src)];
    let mut emitted: HashSet<String> = HashSet::new();
    let mut consts = String::new();
    while let Some((f, name, loc, src)) = todo.pop() {
        emitted.insert(name.clone());
        let _ = write!(text, "fn {name}");
        chunks.push((text.len(), loc.file.clone(), loc.line, loc.col));
        text.push_str(&src);
        text.push('\n');
        let toks = crate::parse::lex(&src).map_err(|e| ShaderError::new(e.start, e.end, format!("{name}: {}", e.message)))?;
        for (n, t) in toks.iter().enumerate() {
            let crate::parse::Tk::Ident(id) = &t.tk else { continue };
            let after_dot = n > 0 && toks[n - 1].tk == crate::parse::Tk::Punct(".");
            if after_dot || KEYWORDS.contains(&id.as_str()) || known.contains(id) || emitted.contains(id) {
                continue;
            }
            let value = vm.bx.heap.scope_value(f, LiveId::from_str(id), NoTrap);
            if value.is_nil() || value.is_err() {
                continue;
            }
            if let Some(obj) = value.as_object() {
                if let Some((loc, src)) = fn_source(vm, obj) {
                    known.insert(id.clone());
                    todo.push((obj, id.clone(), loc, src));
                }
                continue;
            }
            if let Some(c) = constant(vm, value) {
                known.insert(id.clone());
                let _ = writeln!(consts, "let {id} = {c}");
            }
        }
    }
    // Constants go first (a kernel's lets are in order); the fns move down.
    for c in chunks.iter_mut() {
        c.0 += consts.len();
    }
    consts.push_str(&text);
    Ok(KernelSource { text: consts, chunks })
}

/// Compiles a kernel made from Splash values (see the module docs). The
/// errors are the compiler's, their messages naming the document place.
pub fn compile(vm: &ScriptVm, k: &VmKernel, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module]) -> Result<(Arc<Kernel>, KernelSource), Vec<ShaderError>> {
    let source = kernel_source(vm, k).map_err(|e| vec![e])?;
    match kernel::compile_with_modules(&source.text, layouts, backend, modules) {
        Ok(kernel) => Ok((kernel, source)),
        Err(errors) => Err(errors.into_iter().map(|e| ShaderError { message: source.describe(&e), ..e }).collect()),
    }
}

// ---------------------------------------------------------------------------
// Kernels as Splash objects
// ---------------------------------------------------------------------------

/// `mod.kernel`: what a document writes a kernel with, as values the VM
/// evaluates (no host reads the text):
///
/// ```splash
/// use mod.kernel.*
/// let sparks = {
///     out: output(Sprite)            // input(ty [, stride, offset, "buffer"])
///     path: emit_buffer(vec3, 420)   // emit_buffer(ty or words, slots [, "buffer"])
///     amp: param(1.0, 0.0, 10.0)     // param(default [, min, max])
///     element: fn(i) { out[i].pos = vec3(float(i) * amp, 0.0, 0.0) }
/// }
/// ```
///
/// Element types are the pod types (`f32`, `vec3`, …) or a host layout,
/// which this registers by name (`Sprite` above). [`from_object`] reads such
/// an object back.
pub fn script_mod(vm: &mut ScriptVm, layouts: &[Layout]) {
    let m = match vm.bx.heap.value(vm.bx.heap.modules, id!(kernel).into(), NoTrap).as_object() {
        Some(m) => m,
        None => vm.new_module(id!(kernel)),
    };
    for lay in layouts {
        let o = vm.bx.heap.new_object();
        vm.bx.heap.set_value_def(o, id!(kernel_layout).into(), LiveId::from_str(&lay.name).into());
        vm.bx.heap.set_value_def(m, LiveId::from_str(&lay.name).into(), o.into());
    }
    fn decl(vm: &mut ScriptVm, kind: LiveId, args: ScriptObject, keys: &[LiveId]) -> ScriptValue {
        let o = vm.bx.heap.new_object();
        vm.bx.heap.set_value_def(o, id!(kernel_decl).into(), kind.into());
        for k in keys {
            let v = vm.bx.heap.value(args, (*k).into(), NoTrap);
            vm.bx.heap.set_value_def(o, (*k).into(), v);
        }
        o.into()
    }
    vm.add_method(m, id_lut!(input), script_args!(ty = NIL, stride = NIL, offset = NIL, buffer = NIL), |vm, args| {
        decl(vm, id!(input), args, &[id!(ty), id!(stride), id!(offset), id!(buffer)])
    });
    vm.add_method(m, id_lut!(output), script_args!(ty = NIL, stride = NIL, offset = NIL, buffer = NIL), |vm, args| {
        decl(vm, id!(output), args, &[id!(ty), id!(stride), id!(offset), id!(buffer)])
    });
    vm.add_method(m, id_lut!(emit_buffer), script_args!(ty = NIL, slots = NIL, buffer = NIL), |vm, args| {
        decl(vm, id!(emit_buffer), args, &[id!(ty), id!(slots), id!(buffer)])
    });
    vm.add_method(m, id_lut!(param), script_args!(default = NIL, min = NIL, max = NIL), |vm, args| {
        decl(vm, id!(param), args, &[id!(default), id!(min), id!(max)])
    });
}

/// An element type value's name: a pod type or a registered layout.
fn type_name(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    if let Some(o) = v.as_object() {
        if let Some(id) = vm.bx.heap.value(o, id!(kernel_layout).into(), NoTrap).as_id() {
            return Some(id.to_string());
        }
    }
    let ty = vm.bx.heap.pod_type(v)?;
    use makepad_script::pod::{ScriptPodTy as P, ScriptPodVec as V};
    Some(
        match vm.bx.heap.pod_type_ref(ty).ty {
            P::F32 => "f32",
            P::I32 | P::U32 => "i32",
            P::Vec(V::Vec2f) => "vec2",
            P::Vec(V::Vec3f) => "vec3",
            P::Vec(V::Vec4f) => "vec4",
            P::Mat(makepad_script::pod::ScriptPodMat::Mat4x4f) => "mat4",
            _ => return None,
        }
        .to_string(),
    )
}

/// The kernel a Splash object declares (see [`script_mod`]): its
/// declaration fields, its one entry fn and an optional `math: "portable"`.
/// Other fields are the host's (a count, inputs it wires itself).
pub fn from_object(vm: &ScriptVm, obj: ScriptObject) -> Result<VmKernel, String> {
    let mut fields = Vec::new();
    vm.bx.heap.object_data(obj).map_iter_ordered(|k, v| {
        if let Some(k) = k.as_id() {
            fields.push((k, v));
        }
    });
    let mut decls = Vec::new();
    let mut entry = None;
    let mut math = MathMode::Fast;
    for (key, v) in fields {
        let name = key.to_string();
        if let Some(e) = Entry::from_name(&name) {
            let Some(f) = v.as_object().filter(|f| vm.bx.heap.as_fn(*f).is_some()) else {
                return Err(format!("`{name}` is the kernel's entry: a fn(i)"));
            };
            if let Some((other, _)) = entry {
                return Err(format!("a kernel has one entry; found {} and {name}", Entry::name(other)));
            }
            entry = Some((e, f));
            continue;
        }
        if name == "math" {
            math = match vm.bx.heap.string_with(v, |_, s| s.to_string()).as_deref() {
                Some("portable") => MathMode::Portable,
                Some("fast") => MathMode::Fast,
                _ => return Err("math: \"portable\" or \"fast\"".into()),
            };
            continue;
        }
        let Some(o) = v.as_object() else { continue };
        let Some(kind) = vm.bx.heap.value(o, id!(kernel_decl).into(), NoTrap).as_id() else { continue };
        let get = |k: LiveId| vm.bx.heap.value(o, k.into(), NoTrap);
        let count = |k: LiveId| -> Result<Option<u32>, String> {
            let v = get(k);
            if v.is_nil() {
                return Ok(None);
            }
            match v.as_number() {
                Some(x) if x >= 0.0 && x.fract() == 0.0 && x < 16_777_216.0 => Ok(Some(x as u32)),
                _ => Err(format!("{name}: {k} is a whole number")),
            }
        };
        let buffer = || vm.bx.heap.string_with(get(id!(buffer)), |_, s| s.to_string());
        let number = |k: LiveId| get(k).as_number().map(|x| x as f32);
        decls.push(if kind == id!(input) || kind == id!(output) {
            let ty = type_name(vm, get(id!(ty))).ok_or_else(|| format!("{name}: the element type (f32, i32, vec2, vec3, vec4, mat4 or a layout)"))?;
            let (stride, offset, buffer) = (count(id!(stride))?, count(id!(offset))?, buffer());
            if kind == id!(input) {
                Decl::Input { name, ty, stride, offset, buffer }
            } else {
                Decl::Output { name, ty, stride, offset, buffer }
            }
        } else if kind == id!(emit_buffer) {
            let record = match type_name(vm, get(id!(ty))) {
                Some(t) => Record::Type(t),
                None => Record::Words(count(id!(ty))?.ok_or_else(|| format!("{name}: the record type or width"))?),
            };
            let capacity = count(id!(slots))?.ok_or_else(|| format!("{name}: the slots per element"))?;
            Decl::EmitBuffer { name, record, capacity, buffer: buffer() }
        } else {
            let default = number(id!(default)).ok_or_else(|| format!("{name}: param(default [, min, max]) takes numbers"))?;
            let range = match (number(id!(min)), number(id!(max))) {
                (Some(lo), Some(hi)) => Some((lo, hi)),
                _ => None,
            };
            Decl::Param { name, default, range }
        });
    }
    let (entry, entry_fn) = entry.ok_or("missing kernel entry: element, vertex, instance, primitive or reduce_sum / reduce_min / reduce_max")?;
    Ok(VmKernel { decls, entry, entry_fn, math })
}
