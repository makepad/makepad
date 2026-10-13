//! Rust source from the shader IR, retained for compiler validation.
//! Every function takes the
//! invocation's `rcx: &mut RenderCx` (the host-defined record holding the
//! IO the IR keeps in globals: `rcx.vb_*`, `rcx.dyninst_*`, `rcx.var_*`,
//! `rcx.frag_fb*`, `rcx.tex_*` ...).
//!
//! The dialect is the fast-compile one: concrete types, no generics, no
//! macros, no traits. Vector and matrix arithmetic, component-wise
//! builtins and swizzles go through small helper functions generated for
//! the types a shader uses (`_mp_h<n>`), so an operand is evaluated once and
//! nothing depends on operator overloading. The module's preamble supplies
//! `Vec2f` / `Vec3f` / `Vec4f`, `Mat4f` (column-major `v: [f32; 16]`) and
//! `Texture2D`.

use crate::shader_ir::*;
use crate::shader_output::{ShaderOutput, TextureType};
use crate::shader_types::ShaderTypes;
use makepad_live_id::{id, LiveId};
use std::collections::BTreeMap;
use std::fmt::Write;

/// The shader functions of a draw shader compiled to IR, as Rust.
pub struct RustShaderSource {
    /// Helpers, struct definitions and the functions reachable from
    /// `io_vertex` / `io_fragment`.
    pub source: String,
    /// `io_vertex` returns the clip position (else it writes `rcx.vtx_pos`).
    pub vertex_returns_position: bool,
}

/// Types the preamble defines (struct definitions of the same name are
/// skipped).
const PREAMBLE_TYPES: &[&str] = &["Vec2f", "Vec3f", "Vec4f", "Mat4f", "Texture2D"];

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl",
    "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof",
    "unsized", "virtual", "yield", "try", "gen",
];

pub fn rust_shader_source(output: &ShaderOutput, types: &impl ShaderTypes) -> RustShaderSource {
    // Every io's type, used or not: the host's `RenderCx` holds them all.
    let mut ir = output.ir.clone();
    for io in &output.io {
        if !matches!(io.kind, crate::shader_output::ShaderIoKind::Texture(_) | crate::shader_output::ShaderIoKind::Sampler(_)) {
            ir.pod_ty(types, io.ty);
        }
    }
    ir.refresh_struct_names(types);
    let m = &ir;
    let mut roots = Vec::new();
    for name in ["io_vertex", "io_fragment"] {
        if let Some(f) = m.function_by_name(name) {
            roots.push(f);
        }
    }
    let vertex_returns_position = match m.function_by_name("io_vertex") {
        Some(f) => matches!(m.get(m.functions[f as usize].ret), IrTy::Vec(4, IrScalar::F32)),
        None => false,
    };
    let funcs = crate::shader_ir_text::callees_first(m, &roots);
    let mut p = RustPrinter { m, helpers: BTreeMap::new(), helper_text: String::new(), types: BTreeMap::new() };
    let mut bodies = String::new();
    for f in &funcs {
        p.function(*f, &mut bodies);
    }
    let mut source = String::new();
    // Struct definitions, members before their users.
    let mut done = vec![false; m.structs.len()];
    for s in 0..m.structs.len() {
        p.struct_def(s as u32, &mut done, &mut source);
    }
    // Vector types the preamble does not define.
    let types = std::mem::take(&mut p.types);
    for (_, def) in &types {
        source.push_str(def);
    }
    source.push_str(&p.helper_text);
    source.push_str(&bodies);
    RustShaderSource { source, vertex_returns_position }
}

struct RustPrinter<'a> {
    m: &'a IrModule,
    /// helper key -> name
    helpers: BTreeMap<String, String>,
    helper_text: String,
    /// extra vector types: name -> definition
    types: BTreeMap<String, String>,
}

fn lid(id: LiveId) -> String {
    format!("{}", id)
}

fn ident(n: String) -> String {
    if RUST_KEYWORDS.contains(&n.as_str()) {
        return format!("r#{}", n);
    }
    n
}

const LANE: [&str; 4] = ["x", "y", "z", "w"];

impl<'a> RustPrinter<'a> {
    fn scalar(&self, s: IrScalar) -> &'static str {
        match s {
            IrScalar::F32 | IrScalar::F16 => "f32",
            IrScalar::I32 => "i32",
            IrScalar::U32 => "u32",
            IrScalar::Bool => "bool",
        }
    }

    fn ty(&mut self, t: TyId) -> String {
        match self.m.get(t).clone() {
            IrTy::Void => "()".to_string(),
            IrTy::Scalar(s) => self.scalar(s).to_string(),
            IrTy::AbstractInt => "i32".to_string(),
            IrTy::AbstractFloat => "f32".to_string(),
            IrTy::Vec(n, s) => {
                let name = match s {
                    IrScalar::F32 | IrScalar::F16 => format!("Vec{}f", n),
                    IrScalar::I32 => format!("Vec{}i", n),
                    IrScalar::U32 => format!("Vec{}u", n),
                    IrScalar::Bool => format!("Vec{}b", n),
                };
                if !matches!(s, IrScalar::F32 | IrScalar::F16) && !self.types.contains_key(&name) {
                    let st = self.scalar(s);
                    let mut def = format!("#[derive(Clone, Copy, Default, PartialEq)]\n#[repr(C)]\npub struct {} {{", name);
                    for k in 0..n as usize {
                        write!(def, " pub {}: {},", LANE[k], st).ok();
                    }
                    def.push_str(" }\n");
                    self.types.insert(name.clone(), def);
                }
                name
            }
            IrTy::Mat(c, r) => {
                if c == 4 && r == 4 {
                    return "Mat4f".to_string();
                }
                let name = format!("Mat{}x{}f", c, r);
                if !self.types.contains_key(&name) {
                    let def = format!("#[derive(Clone, Copy, PartialEq)]\n#[repr(C)]\npub struct {} {{ pub v: [f32; {}] }}\nimpl Default for {} {{ fn default() -> Self {{ {} {{ v: [0.0; {}] }} }} }}\n", name, c * r, name, name, c * r);
                    self.types.insert(name.clone(), def);
                }
                name
            }
            IrTy::Array(e, n) => format!("[{}; {}]", self.ty(e), n),
            IrTy::Struct(s) => self.struct_name(s),
            IrTy::Texture(_) => "Texture2D".to_string(),
            IrTy::Sampler(_) => "()".to_string(),
            IrTy::CooperativeMatrix(_) => unreachable!("cooperative matrices are compute SPIR-V types"),
        }
    }

    fn struct_name(&self, s: u32) -> String {
        let st = &self.m.structs[s as usize];
        if let Some(n) = st.synth {
            return n.to_string();
        }
        if st.name == LiveId(0) {
            return format!("MpStruct{}", s);
        }
        ident(lid(st.name))
    }

    fn struct_def(&mut self, s: u32, done: &mut Vec<bool>, out: &mut String) {
        if done[s as usize] {
            return;
        }
        done[s as usize] = true;
        let st = self.m.structs[s as usize].clone();
        if st.synth.is_some() {
            return;
        }
        let name = self.struct_name(s);
        if PREAMBLE_TYPES.contains(&name.as_str()) {
            return;
        }
        for f in &st.fields {
            let mut t = f.ty;
            while let IrTy::Array(e, _) = self.m.get(t) {
                t = *e;
            }
            if let IrTy::Struct(inner) = self.m.get(t) {
                let inner = *inner;
                self.struct_def(inner, done, out);
            }
        }
        writeln!(out, "#[derive(Clone, Copy, Default)]\n#[repr(C)]\npub struct {} {{", name).ok();
        for f in &st.fields {
            let t = self.ty(f.ty);
            writeln!(out, "    pub {}: {},", ident(lid(f.name)), t).ok();
        }
        out.push_str("}\n");
    }

    /// A value of `t` that is all zeros (`Mat4f`'s `Default` is the identity).
    fn zero(&mut self, t: TyId) -> String {
        match self.m.get(t).clone() {
            IrTy::Scalar(IrScalar::Bool) => "false".to_string(),
            IrTy::Scalar(IrScalar::F32 | IrScalar::F16) | IrTy::AbstractFloat => "0.0f32".to_string(),
            IrTy::Scalar(IrScalar::I32) | IrTy::AbstractInt => "0i32".to_string(),
            IrTy::Scalar(IrScalar::U32) => "0u32".to_string(),
            IrTy::Mat(4, 4) => "Mat4f { v: [0.0; 16] }".to_string(),
            IrTy::Array(e, n) => format!("[{}; {}]", self.zero(e), n),
            IrTy::Void => "()".to_string(),
            _ => format!("{}::default()", self.ty(t)),
        }
    }

    // ---- helpers ----

    /// The name of the helper `key`, made by `make(name)` the first time.
    fn helper(&mut self, key: String, make: impl FnOnce(&mut Self, &str) -> String) -> String {
        if let Some(n) = self.helpers.get(&key) {
            return n.clone();
        }
        let name = format!("_mp_h{}", self.helpers.len());
        self.helpers.insert(key, name.clone());
        let text = make(self, &name);
        self.helper_text.push_str(&text);
        name
    }

    /// Lane `k` of a value named `v` of type `t` (a scalar is its own
    /// every lane).
    fn lane_of(&self, v: &str, t: TyId, k: usize) -> String {
        match self.m.get(t) {
            IrTy::Vec(..) => format!("{}.{}", v, LANE[k]),
            _ => v.to_string(),
        }
    }

    fn vec_parts(&mut self, t: TyId, parts: &[String]) -> String {
        let name = self.ty(t);
        let mut s = format!("{} {{", name);
        for (k, p) in parts.iter().enumerate() {
            write!(s, " {}: {},", LANE[k], p).ok();
        }
        s.push_str(" }");
        s
    }

    /// A component-wise helper over operands of types `args` giving `ret`:
    /// lane k is `lane(k, [lane k of each operand])`.
    fn componentwise(&mut self, key: &str, args: &[TyId], ret: TyId, lane: &dyn Fn(&mut Self, &[String]) -> String) -> String {
        let mut k = key.to_string();
        for a in args {
            write!(k, ":{}", a).ok();
        }
        write!(k, "->{}", ret).ok();
        let args = args.to_vec();
        self.helper(k, |p, name| {
            let mut s = format!("#[inline(always)]\nfn {}(", name);
            for (i, a) in args.iter().enumerate() {
                let t = p.ty(*a);
                write!(s, "a{}: {}, ", i, t).ok();
            }
            let rt = p.ty(ret);
            write!(s, ") -> {} {{ ", rt).ok();
            let n = p.m.lanes(ret).max(1) as usize;
            if matches!(p.m.get(ret), IrTy::Vec(..)) {
                let mut parts = Vec::new();
                for l in 0..n {
                    let mut ops = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        ops.push(p.lane_of(&format!("a{}", i), *a, l));
                    }
                    parts.push(lane(p, &ops));
                }
                let v = p.vec_parts(ret, &parts);
                s.push_str(&v);
            } else {
                let mut ops = Vec::new();
                for i in 0..args.len() {
                    ops.push(format!("a{}", i));
                }
                s.push_str(&lane(p, &ops));
            }
            s.push_str(" }\n");
            s
        })
    }

    // ---- expressions ----

    fn expr(&mut self, f: &IrFunction, e: ExprId) -> String {
        if e == NO_EXPR {
            return "()".to_string();
        }
        let ex = &f.exprs[e as usize];
        let ty = ex.ty;
        match ex.kind.clone() {
            ExprKind::Lit(l) => self.lit(l, ty),
            ExprKind::Local(l) => self.local_name(f, l),
            ExprKind::Param(i) => self.param_name(f, i),
            ExprKind::Global(g) => self.global(g),
            ExprKind::Field(b, i) => {
                let bt = f.exprs[b as usize].ty;
                let base = self.expr(f, b);
                match self.m.get(bt) {
                    IrTy::Struct(s) => {
                        let name = ident(lid(self.m.structs[*s as usize].fields[i as usize].name));
                        format!("{}.{}", base, name)
                    }
                    _ => format!("{}.{}", base, LANE[i as usize & 3]),
                }
            }
            ExprKind::Swizzle(b, lanes, n) => {
                let base = self.expr(f, b);
                if n == 1 {
                    return format!("{}.{}", base, LANE[lanes[0] as usize & 3]);
                }
                let mut parts = Vec::new();
                for k in 0..n as usize {
                    parts.push(format!("_s.{}", LANE[lanes[k] as usize & 3]));
                }
                let v = self.vec_parts(ty, &parts);
                format!("{{ let _s = {}; {} }}", base, v)
            }
            ExprKind::Index(b, i) => {
                let bt = f.exprs[b as usize].ty;
                let base = self.expr(f, b);
                let idx = self.expr(f, i);
                match self.m.get(bt).clone() {
                    IrTy::Array(..) => format!("{}[({}) as usize]", base, idx),
                    IrTy::Vec(n, _) => {
                        let it = f.exprs[i as usize].ty;
                        let vt = self.ty(bt);
                        let et = self.ty(ty);
                        let ixt = self.ty(it);
                        let h = self.helper(format!("index:{}", bt), |_, name| {
                            let mut s = format!("#[inline(always)]\nfn {}(v: {}, i: usize) -> {} {{ match i {{", name, vt, et);
                            for k in 0..n as usize {
                                write!(s, " {} => v.{},", k, LANE[k]).ok();
                            }
                            write!(s, " _ => v.{} }} }}\n", LANE[n as usize - 1]).ok();
                            s
                        });
                        let _ = ixt;
                        format!("{}({}, ({}) as usize)", h, base, idx)
                    }
                    IrTy::Mat(c, r) => {
                        let mt = self.ty(bt);
                        let ct = self.ty(ty);
                        let h = self.helper(format!("column:{}", bt), |p, name| {
                            let mut parts = Vec::new();
                            for k in 0..r as usize {
                                parts.push(format!("m.v[c * {} + {}]", r, k));
                            }
                            let v = p.vec_parts(ty, &parts);
                            format!("#[inline(always)]\nfn {}(m: {}, i: usize) -> {} {{ let c = i.min({}); {} }}\n", name, mt, ct, c - 1, v)
                        });
                        format!("{}({}, ({}) as usize)", h, base, idx)
                    }
                    _ => format!("{}[({}) as usize]", base, idx),
                }
            }
            ExprKind::Deref(p) => format!("(*{})", self.expr(f, p)),
            ExprKind::AddrOf(p) => format!("&mut {}", self.expr(f, p)),
            ExprKind::Unary(op, a) => {
                let at = f.exprs[a as usize].ty;
                let s = self.expr(f, a);
                if matches!(self.m.get(at), IrTy::Vec(..)) {
                    let o = match op {
                        UnOp::Neg => "-",
                        UnOp::Not | UnOp::BitNot => "!",
                    };
                    let h = self.componentwise(&format!("unary{}", o), &[at], ty, &|_, a| format!("{}{}", o, a[0]));
                    return format!("{}({})", h, s);
                }
                match op {
                    UnOp::Neg => format!("(-{})", s),
                    UnOp::Not | UnOp::BitNot => format!("(!{})", s),
                }
            }
            ExprKind::Binary(op, a, b) => self.binary(f, op, a, b, ty),
            ExprKind::Convert(a) => {
                let at = f.exprs[a as usize].ty;
                let s = self.expr(f, a);
                let from = self.m.scalar_of(at).unwrap_or(IrScalar::F32);
                let to = self.m.scalar_of(ty).unwrap_or(IrScalar::F32);
                if matches!(self.m.get(ty), IrTy::Vec(..)) {
                    let h = self.componentwise("convert", &[at], ty, &|p, a| p.convert_scalar(&a[0], from, to));
                    return format!("{}({})", h, s);
                }
                self.convert_scalar(&s, from, to)
            }
            ExprKind::Bitcast(a) => {
                let at = f.exprs[a as usize].ty;
                let s = self.expr(f, a);
                let from = self.m.scalar_of(at).unwrap_or(IrScalar::F32);
                let to = self.m.scalar_of(ty).unwrap_or(IrScalar::F32);
                if matches!(self.m.get(ty), IrTy::Vec(..)) {
                    let h = self.componentwise("bitcast", &[at], ty, &|p, a| p.bitcast_scalar(&a[0], from, to));
                    return format!("{}({})", h, s);
                }
                self.bitcast_scalar(&s, from, to)
            }
            ExprKind::Construct(args) => self.construct(f, &args, ty),
            ExprKind::Call(fi, args) => {
                let name = self.m.functions[fi as usize].name.clone();
                if args.is_empty() {
                    return format!("{}(rcx)", name);
                }
                // The arguments first: `rcx` is borrowed for the call.
                let mut s = String::from("{ ");
                let mut names = Vec::new();
                for (k, a) in args.iter().enumerate() {
                    let v = self.expr(f, *a);
                    let inout = self.m.functions[fi as usize].params.get(k).map(|p| p.inout).unwrap_or(false);
                    if matches!(f.exprs[*a as usize].kind, ExprKind::AddrOf(_)) {
                        names.push(v);
                    } else if inout {
                        // A reference passed on: reborrowed, not moved.
                        names.push(format!("&mut *{}", v));
                    } else {
                        write!(s, "let _a{} = {}; ", k, v).ok();
                        names.push(format!("_a{}", k));
                    }
                }
                write!(s, "{}(rcx, {}) }}", name, names.join(", ")).ok();
                s
            }
            ExprKind::Builtin(op, args) => self.builtin(f, op, &args, ty),
            ExprKind::Select(c, t, fl) => {
                let ct = f.exprs[c as usize].ty;
                let cs = self.expr(f, c);
                let ts = self.expr(f, t);
                let fs = self.expr(f, fl);
                if matches!(self.m.get(ct), IrTy::Vec(..)) {
                    let tt = f.exprs[t as usize].ty;
                    let h = self.componentwise("select", &[ct, tt, tt], ty, &|_, a| format!("if {} {{ {} }} else {{ {} }}", a[0], a[1], a[2]));
                    return format!("{}({}, {}, {})", h, cs, ts, fs);
                }
                format!("(if {} {{ {} }} else {{ {} }})", cs, ts, fs)
            }
            ExprKind::Sample(smp) => {
                // The coordinates first: they may call functions that take
                // `rcx` mutably (Texture2D is Copy).
                let t = self.expr(f, smp.tex);
                let c = self.expr(f, smp.coord);
                let r = match smp.kind {
                    SampleKind::CompareLevel0 => {
                        let r = self.expr(f, smp.reference);
                        return format!("{{ let _c = {}; let _r = {}; let _t = {}; _t.sample_compare(_c, _r) }}", c, r, t);
                    }
                    SampleKind::Lod => {
                        let l = self.expr(f, smp.lod);
                        format!("{{ let _c = {}; let _l = {}; let _t = {}; _t.sample_lod(_c, _l) }}", c, l, t)
                    }
                    _ => {
                        let method = if smp.method == id!(sample_nearest) {
                            "sample_nearest"
                        } else if smp.method == id!(sample_repeat) {
                            "sample_repeat"
                        } else {
                            "sample"
                        };
                        format!("{{ let _c = {}; let _t = {}; _t.{}(_c) }}", c, t, method)
                    }
                };
                if ty == TY_F32 {
                    return format!("{}.x", r);
                }
                r
            }
            ExprKind::TexSize(t) => {
                let ts = self.expr(f, t);
                if self.m.scalar_of(ty) == Some(IrScalar::U32) {
                    let v = self.vec_parts(ty, &["_t.width as u32".to_string(), "_t.height as u32".to_string()]);
                    format!("{{ let _t = {}; {} }}", ts, v)
                } else {
                    let v = self.vec_parts(ty, &["_t.width as f32".to_string(), "_t.height as f32".to_string()]);
                    format!("{{ let _t = {}; {} }}", ts, v)
                }
            }
            ExprKind::TexLoad(t, c, _layer, _lod) => {
                let ts = self.expr(f, t);
                let cs = self.expr(f, c);
                let r = format!(
                    "{{ let _c = {1}; let _t = {0}; _t.sample_nearest(Vec2f {{ x: (_c.x as f32 + 0.5) / (_t.width.max(1) as f32), y: (_c.y as f32 + 0.5) / (_t.height.max(1) as f32) }}) }}",
                    ts, cs
                );
                if ty == TY_F32 {
                    format!("{}.x", r)
                } else {
                    r
                }
            }
            ExprKind::PassMatrix(which) => format!("rcx.unibuf_draw_pass.{}", crate::shader_ir_text::pass_matrix_name(which)),
            ExprKind::InstanceIndex => "rcx.instance_index".to_string(),
            ExprKind::DepthClip(_, color, _) => self.expr(f, color),
            ExprKind::InstanceTexWord(_) | ExprKind::EntryIn(_) | ExprKind::EntryOut(_) | ExprKind::Nop | ExprKind::Error | ExprKind::Range(..) => self.zero(ty),
        }
    }

    fn lit(&mut self, l: IrLit, ty: TyId) -> String {
        match l {
            IrLit::Bool(b) => format!("{}", b),
            IrLit::F32(v) | IrLit::F16(v) => f32_lit(v),
            IrLit::I32(v) => format!("({}i32)", v),
            IrLit::U32(v) => format!("{}u32", v),
            IrLit::AbstractInt(v) => match self.m.scalar_of(ty) {
                Some(IrScalar::U32) => format!("{}u32", v),
                Some(IrScalar::F32 | IrScalar::F16) => f32_lit(v as f32),
                _ => format!("({}i32)", v),
            },
            IrLit::AbstractFloat(v) => f32_lit(v as f32),
        }
    }

    fn local_name(&self, f: &IrFunction, l: LocalId) -> String {
        let local = &f.locals[l as usize];
        match local.kind {
            LocalKind::Phi => format!("_phi_{}", l),
            LocalKind::Guard => format!("_mp_loop_guard_{}", l),
            LocalKind::LoopEnd => format!("_mp_end_{}", l),
            LocalKind::Temp => format!("_t{}", l),
            LocalKind::User => format!("l{}_{}", l, lid(local.name)),
        }
    }

    fn param_name(&self, f: &IrFunction, i: u32) -> String {
        let p = &f.params[i as usize];
        if p.inout || p.name == id!(self) {
            return "_self".to_string();
        }
        format!("p_{}", lid(p.name))
    }

    fn global(&self, g: GlobalId) -> String {
        let glob = &self.m.globals[g as usize];
        let n = lid(glob.name);
        match glob.kind {
            IrGlobalKind::Io(io) => match io {
                IrIo::Geometry => format!("rcx.vb_{}", n),
                IrIo::DynInstance => format!("rcx.dyninst_{}", n),
                IrIo::RustInstance => format!("rcx.rustinst_{}", n),
                IrIo::Uniform => format!("rcx.uni_{}", n),
                IrIo::ScopeUniform => format!("rcx.su_{}", n),
                IrIo::UniformBuffer => format!("rcx.unibuf_{}", n),
                IrIo::Varying => format!("rcx.var_{}", n),
                IrIo::FragmentOutput(i) => format!("rcx.frag_fb{}", i),
                IrIo::Texture(_) => format!("rcx.tex_{}", n),
                // Compute kernels print only to SPIR-V.
                IrIo::Storage(_) | IrIo::Workgroup => format!("rcx.kernel_{}", n),
            },
            IrGlobalKind::VertexPosition => "rcx.vtx_pos".to_string(),
            IrGlobalKind::IterCounter => "rcx._mp_iter".to_string(),
            IrGlobalKind::ViewId => "0i32".to_string(),
            IrGlobalKind::InstanceIndex => "rcx.instance_index".to_string(),
            _ => format!("rcx.{}", n),
        }
    }

    fn convert_scalar(&self, s: &str, from: IrScalar, to: IrScalar) -> String {
        if from == to || (from.is_float() && to.is_float()) {
            return s.to_string();
        }
        match (from, to) {
            (IrScalar::Bool, _) => {
                let (one, zero) = match to {
                    IrScalar::F32 | IrScalar::F16 => ("1.0f32", "0.0f32"),
                    IrScalar::I32 => ("1i32", "0i32"),
                    _ => ("1u32", "0u32"),
                };
                format!("(if {} {{ {} }} else {{ {} }})", s, one, zero)
            }
            (_, IrScalar::Bool) => {
                let zero = match from {
                    IrScalar::F32 | IrScalar::F16 => "0.0f32",
                    IrScalar::I32 => "0i32",
                    _ => "0u32",
                };
                format!("({} != {})", s, zero)
            }
            _ => format!("(({}) as {})", s, self.scalar(to)),
        }
    }

    fn bitcast_scalar(&self, s: &str, from: IrScalar, to: IrScalar) -> String {
        match (from.is_float(), to) {
            _ if from == to => s.to_string(),
            (true, IrScalar::U32) => format!("({}).to_bits()", s),
            (true, IrScalar::I32) => format!("(({}).to_bits() as i32)", s),
            (false, IrScalar::F32 | IrScalar::F16) => format!("f32::from_bits(({}) as u32)", s),
            (false, IrScalar::I32) => format!("(({}) as i32)", s),
            (false, IrScalar::U32) => format!("(({}) as u32)", s),
            _ => s.to_string(),
        }
    }

    fn construct(&mut self, f: &IrFunction, args: &[ExprId], ty: TyId) -> String {
        match self.m.get(ty).clone() {
            IrTy::Struct(s) => {
                if args.is_empty() {
                    return self.zero(ty);
                }
                let name = self.struct_name(s);
                let fields = self.m.structs[s as usize].fields.clone();
                let mut out = format!("{} {{", name);
                for (k, a) in args.iter().enumerate() {
                    let v = self.expr(f, *a);
                    if let Some(fd) = fields.get(k) {
                        write!(out, " {}: {},", ident(lid(fd.name)), v).ok();
                    }
                }
                out.push_str(" }");
                out
            }
            IrTy::Array(..) => {
                if args.is_empty() {
                    return self.zero(ty);
                }
                let mut parts = Vec::new();
                for a in args {
                    parts.push(self.expr(f, *a));
                }
                format!("[{}]", parts.join(", "))
            }
            IrTy::Vec(n, s) => {
                if args.is_empty() {
                    return self.zero(ty);
                }
                // Bind the parts, then pick their lanes.
                let mut out = String::from("{ ");
                let mut lanes = Vec::new();
                for (k, a) in args.iter().enumerate() {
                    let at = f.exprs[*a as usize].ty;
                    let v = self.expr(f, *a);
                    write!(out, "let _p{} = {}; ", k, v).ok();
                    let from = self.m.scalar_of(at).unwrap_or(s);
                    match self.m.get(at) {
                        IrTy::Vec(an, _) => {
                            for l in 0..*an as usize {
                                lanes.push(self.convert_scalar(&format!("_p{}.{}", k, LANE[l]), from, s));
                            }
                        }
                        IrTy::Mat(..) => {}
                        _ => lanes.push(self.convert_scalar(&format!("_p{}", k), from, s)),
                    }
                }
                if lanes.len() == 1 {
                    let l = lanes[0].clone();
                    lanes = vec![l; n as usize];
                }
                lanes.truncate(n as usize);
                while lanes.len() < (n as usize) {
                    lanes.push(match s {
                        IrScalar::Bool => "false".to_string(),
                        IrScalar::F32 | IrScalar::F16 => "0.0f32".to_string(),
                        IrScalar::I32 => "0i32".to_string(),
                        IrScalar::U32 => "0u32".to_string(),
                    });
                }
                let v = self.vec_parts(ty, &lanes);
                out.push_str(&v);
                out.push_str(" }");
                out
            }
            IrTy::Mat(c, r) => {
                let name = self.ty(ty);
                let cells = (c * r) as usize;
                if args.is_empty() {
                    return self.zero(ty);
                }
                let mut out = String::from("{ ");
                let mut lanes = Vec::new();
                let mut from_mat = None;
                for (k, a) in args.iter().enumerate() {
                    let at = f.exprs[*a as usize].ty;
                    let v = self.expr(f, *a);
                    write!(out, "let _p{} = {}; ", k, v).ok();
                    match self.m.get(at).clone() {
                        IrTy::Vec(an, _) => {
                            for l in 0..an as usize {
                                lanes.push(format!("_p{}.{}", k, LANE[l]));
                            }
                        }
                        IrTy::Mat(ac, ar) => from_mat = Some((k, ac, ar)),
                        _ => lanes.push(format!("_p{}", k)),
                    }
                }
                let mut v = Vec::new();
                if let Some((k, ac, ar)) = from_mat {
                    // The upper-left part of another matrix (identity outside).
                    for ci in 0..c {
                        for ri in 0..r {
                            if ci < ac && ri < ar {
                                v.push(format!("_p{}.v[{}]", k, ci * ar + ri));
                            } else {
                                v.push(if ci == ri { "1.0f32".to_string() } else { "0.0f32".to_string() });
                            }
                        }
                    }
                } else if lanes.len() == 1 {
                    for ci in 0..c {
                        for ri in 0..r {
                            v.push(if ci == ri { lanes[0].clone() } else { "0.0f32".to_string() });
                        }
                    }
                } else {
                    v = lanes;
                    v.resize(cells, "0.0f32".to_string());
                }
                write!(out, "{} {{ v: [{}] }} }}", name, v.join(", ")).ok();
                out
            }
            _ => {
                if let Some(a) = args.first() {
                    let at = f.exprs[*a as usize].ty;
                    let v = self.expr(f, *a);
                    let from = self.m.scalar_of(at).unwrap_or(IrScalar::F32);
                    let to = self.m.scalar_of(ty).unwrap_or(IrScalar::F32);
                    return self.convert_scalar(&v, from, to);
                }
                self.zero(ty)
            }
        }
    }

    fn scalar_binop(&self, op: BinOp, a: &str, b: &str, s: IrScalar) -> String {
        match op {
            BinOp::Div | BinOp::Rem if matches!(s, IrScalar::I32 | IrScalar::U32) => {
                // GPU integer division by zero is undefined, Rust's panics.
                let m = if op == BinOp::Div { "checked_div" } else { "checked_rem" };
                format!("({}).{}({}).unwrap_or(0)", a, m, b)
            }
            BinOp::Shl | BinOp::Shr => {
                let m = if op == BinOp::Shl { "wrapping_shl" } else { "wrapping_shr" };
                format!("({}).{}(({}) as u32)", a, m, b)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul if matches!(s, IrScalar::I32 | IrScalar::U32) => {
                let m = match op {
                    BinOp::Add => "wrapping_add",
                    BinOp::Sub => "wrapping_sub",
                    _ => "wrapping_mul",
                };
                format!("({}).{}({})", a, m, b)
            }
            BinOp::LogicAnd => format!("({} && {})", a, b),
            BinOp::LogicOr => format!("({} || {})", a, b),
            _ => format!("({} {} {})", a, op.text(), b),
        }
    }

    fn binary(&mut self, f: &IrFunction, op: BinOp, a: ExprId, b: ExprId, ty: TyId) -> String {
        let at = f.exprs[a as usize].ty;
        let bt = f.exprs[b as usize].ty;
        let sa = self.expr(f, a);
        let sb = self.expr(f, b);
        let a_mat = matches!(self.m.get(at), IrTy::Mat(..));
        let b_mat = matches!(self.m.get(bt), IrTy::Mat(..));
        if op == BinOp::Mul && (a_mat || b_mat) && !(self.m.lanes(at) == 1 || self.m.lanes(bt) == 1) {
            return self.matrix_mul(at, bt, ty, &sa, &sb);
        }
        let vector = matches!(self.m.get(at), IrTy::Vec(..) | IrTy::Mat(..)) || matches!(self.m.get(bt), IrTy::Vec(..) | IrTy::Mat(..));
        if !vector {
            let s = self.m.scalar_of(at).unwrap_or(IrScalar::F32);
            if op == BinOp::LogicAnd || op == BinOp::LogicOr {
                return self.scalar_binop(op, &sa, &sb, s);
            }
            // `&&`/`||` short-circuit; the IR has evaluated both sides.
            return self.scalar_binop(op, &sa, &sb, s);
        }
        if a_mat || b_mat {
            // Matrix (+ - * /) matrix or scalar: cell by cell.
            let mt = if a_mat { at } else { bt };
            let (c, r) = match self.m.get(mt) {
                IrTy::Mat(c, r) => (*c, *r),
                _ => (4, 4),
            };
            let name = self.ty(mt);
            let at_s = self.ty(at);
            let bt_s = self.ty(bt);
            let h = self.helper(format!("matop:{}:{}:{}", op.text(), at, bt), |_, hn| {
                let mut s = format!("#[inline(always)]\nfn {}(a: {}, b: {}) -> {} {{ let mut v = [0.0f32; {}]; for k in 0..{} {{ v[k] = ", hn, at_s, bt_s, name, c * r, c * r);
                let l = if a_mat { "a.v[k]" } else { "a" };
                let rr = if b_mat { "b.v[k]" } else { "b" };
                write!(s, "{} {} {}; }} {} {{ v }} }}\n", l, op.text(), rr, name).ok();
                s
            });
            return format!("{}({}, {})", h, sa, sb);
        }
        let s = self.m.scalar_of(at).or(self.m.scalar_of(bt)).unwrap_or(IrScalar::F32);
        let h = self.componentwise(&format!("bin{}", op.text()), &[at, bt], ty, &|p, x| p.scalar_binop(op, &x[0], &x[1], s));
        format!("{}({}, {})", h, sa, sb)
    }

    fn matrix_mul(&mut self, at: TyId, bt: TyId, ty: TyId, sa: &str, sb: &str) -> String {
        let a = self.m.get(at).clone();
        let b = self.m.get(bt).clone();
        let at_s = self.ty(at);
        let bt_s = self.ty(bt);
        let rt_s = self.ty(ty);
        let h = self.helper(format!("matmul:{}:{}", at, bt), |p, name| {
            let mut s = format!("#[inline(always)]\nfn {}(a: {}, b: {}) -> {} {{ ", name, at_s, bt_s, rt_s);
            match (a, b) {
                (IrTy::Mat(c, r), IrTy::Vec(..)) => {
                    // m * v: sum over columns.
                    let mut parts = Vec::new();
                    for ri in 0..r as usize {
                        let mut terms = Vec::new();
                        for ci in 0..c as usize {
                            terms.push(format!("a.v[{}] * b.{}", ci * r as usize + ri, LANE[ci]));
                        }
                        parts.push(terms.join(" + "));
                    }
                    let v = p.vec_parts(ty, &parts);
                    s.push_str(&v);
                }
                (IrTy::Vec(..), IrTy::Mat(c, r)) => {
                    // v * m: dot with each column.
                    let mut parts = Vec::new();
                    for ci in 0..c as usize {
                        let mut terms = Vec::new();
                        for ri in 0..r as usize {
                            terms.push(format!("a.{} * b.v[{}]", LANE[ri], ci * r as usize + ri));
                        }
                        parts.push(terms.join(" + "));
                    }
                    let v = p.vec_parts(ty, &parts);
                    s.push_str(&v);
                }
                (IrTy::Mat(ac, ar), IrTy::Mat(bc, _br)) => {
                    let mut cells = Vec::new();
                    for ci in 0..bc as usize {
                        for ri in 0..ar as usize {
                            let mut terms = Vec::new();
                            for k in 0..ac as usize {
                                terms.push(format!("a.v[{}] * b.v[{}]", k * ar as usize + ri, ci * ac as usize + k));
                            }
                            cells.push(terms.join(" + "));
                        }
                    }
                    write!(s, "{} {{ v: [{}] }}", rt_s, cells.join(", ")).ok();
                }
                _ => s.push_str("Default::default()"),
            }
            s.push_str(" }\n");
            s
        });
        format!("{}({}, {})", h, sa, sb)
    }

    /// A builtin over scalars of kind `s`.
    fn scalar_builtin(&self, op: Builtin, a: &[String], s: IrScalar) -> String {
        let float = s.is_float();
        let x = a.first().cloned().unwrap_or_default();
        match op {
            Builtin::Abs => {
                if s == IrScalar::U32 {
                    x
                } else {
                    format!("({}).abs()", x)
                }
            }
            Builtin::Min => format!("({}).min({})", x, a[1]),
            Builtin::Max => format!("({}).max({})", x, a[1]),
            Builtin::Clamp => format!("({}).max({}).min({})", x, a[1], a[2]),
            Builtin::Sign => {
                if float {
                    format!("{{ let _x = {}; if _x > 0.0 {{ 1.0f32 }} else if _x < 0.0 {{ -1.0f32 }} else {{ 0.0f32 }} }}", x)
                } else {
                    format!("({}).signum()", x)
                }
            }
            Builtin::Acos => format!("({}).acos()", x),
            Builtin::Acosh => format!("({}).acosh()", x),
            Builtin::Asin => format!("({}).asin()", x),
            Builtin::Asinh => format!("({}).asinh()", x),
            Builtin::Atan => format!("({}).atan()", x),
            Builtin::Atanh => format!("({}).atanh()", x),
            Builtin::Atan2 => format!("({}).atan2({})", x, a[1]),
            Builtin::Ceil => format!("({}).ceil()", x),
            Builtin::Cos => format!("({}).cos()", x),
            Builtin::Cosh => format!("({}).cosh()", x),
            Builtin::Degrees => format!("({}).to_degrees()", x),
            Builtin::Radians => format!("({}).to_radians()", x),
            Builtin::Exp => format!("({}).exp()", x),
            Builtin::Exp2 => format!("({}).exp2()", x),
            Builtin::Floor => format!("({}).floor()", x),
            Builtin::Fma => format!("({} * {} + {})", x, a[1], a[2]),
            Builtin::Fract => format!("{{ let _x = {}; _x - _x.floor() }}", x),
            Builtin::InverseSqrt => format!("(1.0f32 / ({}).sqrt())", x),
            Builtin::Log => format!("({}).ln()", x),
            Builtin::Log2 => format!("({}).log2()", x),
            Builtin::Mix => format!("{{ let _a = {}; _a + ({} - _a) * {} }}", x, a[1], a[2]),
            Builtin::FMod => format!("{{ let _x = {}; let _y = {}; _x - _y * (_x / _y).floor() }}", x, a[1]),
            Builtin::Pow => format!("({}).powf({})", x, a[1]),
            Builtin::Round => format!("({}).round()", x),
            Builtin::Sin => format!("({}).sin()", x),
            Builtin::Sinh => format!("({}).sinh()", x),
            Builtin::Sqrt => format!("({}).sqrt()", x),
            Builtin::Step => format!("(if {} < {} {{ 0.0f32 }} else {{ 1.0f32 }})", a[1], x),
            Builtin::Smoothstep => format!("{{ let _e0 = {}; let _t = (({} - _e0) / ({} - _e0)).max(0.0).min(1.0); _t * _t * (3.0 - 2.0 * _t) }}", x, a[2], a[1]),
            Builtin::Tan => format!("({}).tan()", x),
            Builtin::Tanh => format!("({}).tanh()", x),
            Builtin::Trunc => format!("({}).trunc()", x),
            _ => x,
        }
    }

    fn builtin(&mut self, f: &IrFunction, op: Builtin, args: &[ExprId], ty: TyId) -> String {
        let mut parts = Vec::new();
        let mut tys = Vec::new();
        for a in args {
            parts.push(self.expr(f, *a));
            tys.push(f.exprs[*a as usize].ty);
        }
        let first = tys.first().copied().unwrap_or(ty);
        let call = |h: String, parts: &[String]| format!("{}({})", h, parts.join(", "));
        match op {
            Builtin::Dot | Builtin::Length | Builtin::Distance | Builtin::Normalize | Builtin::Cross => {
                let n = self.m.lanes(first) as usize;
                let vt = self.ty(first);
                let rt = self.ty(ty);
                let h = self.helper(format!("geo:{:?}:{}", op, first), |p, name| {
                    let dot = |x: &str, y: &str| -> String {
                        let mut terms = Vec::new();
                        for k in 0..n {
                            terms.push(format!("{}.{} * {}.{}", x, LANE[k], y, LANE[k]));
                        }
                        terms.join(" + ")
                    };
                    match op {
                        Builtin::Dot => format!("#[inline(always)]\nfn {}(a: {}, b: {}) -> f32 {{ {} }}\n", name, vt, vt, dot("a", "b")),
                        Builtin::Length => format!("#[inline(always)]\nfn {}(a: {}) -> f32 {{ ({}).sqrt() }}\n", name, vt, dot("a", "a")),
                        Builtin::Distance => {
                            let mut lanes = Vec::new();
                            for k in 0..n {
                                lanes.push(format!("a.{} - b.{}", LANE[k], LANE[k]));
                            }
                            let d = p.vec_parts(first, &lanes);
                            format!("#[inline(always)]\nfn {}(a: {}, b: {}) -> f32 {{ let d = {}; ({}).sqrt() }}\n", name, vt, vt, d, dot("d", "d"))
                        }
                        Builtin::Normalize => {
                            let mut lanes = Vec::new();
                            for k in 0..n {
                                lanes.push(format!("a.{} * il", LANE[k]));
                            }
                            let v = p.vec_parts(first, &lanes);
                            format!("#[inline(always)]\nfn {}(a: {}) -> {} {{ let l = ({}).sqrt(); let il = if l > 0.0 {{ 1.0 / l }} else {{ 0.0 }}; {} }}\n", name, vt, rt, dot("a", "a"), v)
                        }
                        _ => {
                            let v = p.vec_parts(
                                first,
                                &["a.y * b.z - a.z * b.y".to_string(), "a.z * b.x - a.x * b.z".to_string(), "a.x * b.y - a.y * b.x".to_string()],
                            );
                            format!("#[inline(always)]\nfn {}(a: {}, b: {}) -> {} {{ {} }}\n", name, vt, vt, rt, v)
                        }
                    }
                });
                if !matches!(self.m.get(first), IrTy::Vec(..)) {
                    // On scalars.
                    return match op {
                        Builtin::Dot => format!("({} * {})", parts[0], parts[1]),
                        Builtin::Length => format!("({}).abs()", parts[0]),
                        Builtin::Distance => format!("({} - {}).abs()", parts[0], parts[1]),
                        Builtin::Normalize => format!("({}).signum()", parts[0]),
                        _ => parts[0].clone(),
                    };
                }
                call(h, &parts)
            }
            Builtin::Inverse => {
                let mt = self.ty(first);
                let h = self.helper(format!("inverse:{}", first), |_, name| format!("#[inline(always)]\nfn {}(m: {}) -> {} {{ m.invert() }}\n", name, mt, mt));
                call(h, &parts)
            }
            Builtin::Dpdx | Builtin::Dpdy => {
                let n = self.m.lanes(first).max(1) as usize;
                let vt = self.ty(first);
                let dx = op == Builtin::Dpdx;
                let h = self.helper(format!("deriv:{}:{}", dx, first), |p, name| {
                    let (buf, lane) = if dx { ("quad_dx_buf", "quad_lane_x") } else { ("quad_dy_buf", "quad_lane_y") };
                    // The 3-pass quad derivative: two recording passes store the
                    // neighbours' values, the third differences against them.
                    let get = |k: usize| -> String {
                        if n == 1 {
                            "v".to_string()
                        } else {
                            format!("v.{}", LANE[k])
                        }
                    };
                    let mut store_dx = String::new();
                    let mut store_dy = String::new();
                    let mut diff = Vec::new();
                    let mut neg = Vec::new();
                    let mut zero = Vec::new();
                    for k in 0..n {
                        write!(store_dx, "rcx.quad_dx_buf[s + {}] = {}; ", k, get(k)).ok();
                        write!(store_dy, "rcx.quad_dy_buf[s + {}] = {}; ", k, get(k)).ok();
                        diff.push(format!("rcx.{}[s + {}] - {}", buf, k, get(k)));
                        neg.push(format!("-d{}", if n == 1 { String::new() } else { format!(".{}", LANE[k]) }));
                        zero.push("0.0f32".to_string());
                    }
                    let (d, nd, z) = if n == 1 {
                        (diff[0].clone(), neg[0].clone(), "0.0f32".to_string())
                    } else {
                        (p.vec_parts(first, &diff), p.vec_parts(first, &neg), p.vec_parts(first, &zero))
                    };
                    format!(
                        "fn {name}(rcx: &mut RenderCx, v: {vt}) -> {vt} {{ let s = rcx.quad_slot as usize; rcx.quad_slot = rcx.quad_slot.saturating_add({n}); if s + {n} > rcx.quad_dx_buf.len() {{ return {z}; }} if rcx.quad_mode == 2 {{ let d = {d}; if rcx.{lane} == 0 {{ d }} else {{ {nd} }} }} else {{ if rcx.quad_mode == 0 {{ {store_dx}}} else {{ {store_dy}}} {z} }} }}\n"
                    )
                });
                format!("{{ let _v = {}; {}(rcx, _v) }}", parts[0], h)
            }
            Builtin::Unpack2f16 | Builtin::Unpack2x16Float => {
                let bits = if self.m.scalar_of(first) == Some(IrScalar::U32) { parts[0].clone() } else { format!("({}).to_bits()", parts[0]) };
                let h = self.helper("unpack2x16float".to_string(), |_, name| {
                    format!(
                        "fn {name}(u: u32) -> Vec2f {{ Vec2f {{ x: {name}_h((u & 0xffff) as u16), y: {name}_h((u >> 16) as u16) }} }}\nfn {name}_h(h: u16) -> f32 {{ let s = ((h >> 15) & 1) as u32; let e = ((h >> 10) & 0x1f) as i32; let m = (h & 0x3ff) as u32; let v = if e == 0 {{ (m as f32) * (1.0 / 16777216.0) }} else if e == 31 {{ if m == 0 {{ f32::INFINITY }} else {{ f32::NAN }} }} else {{ f32::from_bits(((e - 15 + 127) as u32) << 23 | (m << 13)) }}; if s == 1 {{ -v }} else {{ v }} }}\n"
                    )
                });
                format!("{}({})", h, bits)
            }
            Builtin::Unpack4u8 | Builtin::Unpack4x8Unorm => {
                let bits = if self.m.scalar_of(first) == Some(IrScalar::U32) { parts[0].clone() } else { format!("({}).to_bits()", parts[0]) };
                let h = self.helper("unpack4x8unorm".to_string(), |_, name| {
                    format!("fn {name}(u: u32) -> Vec4f {{ Vec4f {{ x: (u & 0xff) as f32 / 255.0, y: ((u >> 8) & 0xff) as f32 / 255.0, z: ((u >> 16) & 0xff) as f32 / 255.0, w: ((u >> 24) & 0xff) as f32 / 255.0 }} }}\n")
                });
                format!("{}({})", h, bits)
            }
            Builtin::Unpack4x8Snorm => {
                let h = self.helper("unpack4x8snorm".to_string(), |_, name| {
                    format!("fn {name}(u: u32) -> Vec4f {{ let c = |k: u32| (((u >> k) & 0xff) as u8 as i8 as f32 / 127.0).max(-1.0); Vec4f {{ x: c(0), y: c(8), z: c(16), w: c(24) }} }}\n")
                });
                format!("{}({})", h, parts[0])
            }
            Builtin::Unpack2x16Unorm => {
                let h = self.helper("unpack2x16unorm".to_string(), |_, name| {
                    format!("fn {name}(u: u32) -> Vec2f {{ Vec2f {{ x: (u & 0xffff) as f32 / 65535.0, y: (u >> 16) as f32 / 65535.0 }} }}\n")
                });
                format!("{}({})", h, parts[0])
            }
            Builtin::Unpack2x16Snorm => {
                let h = self.helper("unpack2x16snorm".to_string(), |_, name| {
                    format!("fn {name}(u: u32) -> Vec2f {{ Vec2f {{ x: ((u & 0xffff) as u16 as i16 as f32 / 32767.0).max(-1.0), y: ((u >> 16) as u16 as i16 as f32 / 32767.0).max(-1.0) }} }}\n")
                });
                format!("{}({})", h, parts[0])
            }
            _ => {
                let s = self.m.scalar_of(ty).unwrap_or(IrScalar::F32);
                if matches!(self.m.get(ty), IrTy::Vec(..)) {
                    let h = self.componentwise(&format!("builtin{:?}", op), &tys, ty, &|p, x| p.scalar_builtin(op, x, s));
                    return call(h, &parts);
                }
                self.scalar_builtin(op, &parts, s)
            }
        }
    }

    // ---- statements ----

    fn pad(out: &mut String, indent: usize) {
        for _ in 0..indent {
            out.push_str("    ");
        }
    }

    fn stmts(&mut self, f: &IrFunction, stmts: &[Stmt], indent: usize, out: &mut String) {
        for s in stmts {
            self.stmt(f, s, indent, out);
        }
    }

    /// `place = value` for a place Rust cannot assign to directly (a vector
    /// lane by index, several swizzled lanes, a matrix column), else None.
    fn assign_special(&mut self, f: &IrFunction, p: ExprId, value: &str) -> Option<String> {
        match f.exprs[p as usize].kind.clone() {
            ExprKind::Swizzle(b, lanes, n) if n > 1 => {
                let base = self.expr(f, b);
                let mut s = format!("{{ let _w = {};", value);
                for k in 0..n as usize {
                    write!(s, " {}.{} = _w.{};", base, LANE[lanes[k] as usize & 3], LANE[k]).ok();
                }
                s.push_str(" }");
                Some(s)
            }
            ExprKind::Index(b, i) => {
                let bt = f.exprs[b as usize].ty;
                match self.m.get(bt).clone() {
                    IrTy::Vec(n, _) => {
                        let base = self.expr(f, b);
                        let idx = self.expr(f, i);
                        let mut s = format!("{{ let _w = {}; match ({}) as usize {{", value, idx);
                        for k in 0..n as usize {
                            write!(s, " {} => {}.{} = _w,", k, base, LANE[k]).ok();
                        }
                        s.push_str(" _ => {} } }");
                        Some(s)
                    }
                    IrTy::Mat(_, r) => {
                        let base = self.expr(f, b);
                        let idx = self.expr(f, i);
                        let mut s = format!("{{ let _w = {}; let _c = ({}) as usize;", value, idx);
                        for k in 0..r as usize {
                            write!(s, " {}.v[_c * {} + {}] = _w.{};", base, r, k, LANE[k]).ok();
                        }
                        s.push_str(" }");
                        Some(s)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn stmt(&mut self, f: &IrFunction, s: &Stmt, indent: usize, out: &mut String) {
        match s {
            Stmt::Local(l, init) => {
                let local = &f.locals[*l as usize];
                if local.ty == TY_VOID {
                    return;
                }
                let name = self.local_name(f, *l);
                let t = self.ty(local.ty);
                let missing = *init == NO_EXPR || matches!(f.exprs[*init as usize].kind, ExprKind::Error);
                let v = if missing { self.zero(local.ty) } else { self.expr(f, *init) };
                Self::pad(out, indent);
                writeln!(out, "let mut {}: {} = {};", name, t, v).ok();
            }
            Stmt::Assign(p, v) => {
                let val = self.expr(f, *v);
                Self::pad(out, indent);
                if let Some(s) = self.assign_special(f, *p, &val) {
                    writeln!(out, "{}", s).ok();
                    return;
                }
                let place = self.expr(f, *p);
                writeln!(out, "{} = {};", place, val).ok();
            }
            Stmt::CompoundAssign(p, op, v) => {
                // `place = place op value` through the expression printer.
                let pt = f.exprs[*p as usize].ty;
                let vt = f.exprs[*v as usize].ty;
                let cur = self.expr(f, *p);
                let val = self.expr(f, *v);
                let res = match (self.m.get(pt).clone(), self.m.get(vt).clone()) {
                    (IrTy::Scalar(sc), IrTy::Scalar(_)) => self.scalar_binop(*op, &cur, &val, sc),
                    _ => {
                        if *op == BinOp::Mul && (matches!(self.m.get(pt), IrTy::Mat(..)) || matches!(self.m.get(vt), IrTy::Mat(..))) && self.m.lanes(vt) != 1 {
                            self.matrix_mul(pt, vt, pt, &cur, &val)
                        } else {
                            let sc = self.m.scalar_of(pt).unwrap_or(IrScalar::F32);
                            let op = *op;
                            if matches!(self.m.get(pt), IrTy::Mat(..)) {
                                let (c, r) = match self.m.get(pt) {
                                    IrTy::Mat(c, r) => (*c, *r),
                                    _ => (4, 4),
                                };
                                let name = self.ty(pt);
                                let vts = self.ty(vt);
                                let bm = matches!(self.m.get(vt), IrTy::Mat(..));
                                let h = self.helper(format!("matop:{}:{}:{}", op.text(), pt, vt), |_, hn| {
                                    let rr = if bm { "b.v[k]" } else { "b" };
                                    format!("#[inline(always)]\nfn {}(a: {}, b: {}) -> {} {{ let mut v = [0.0f32; {}]; for k in 0..{} {{ v[k] = a.v[k] {} {}; }} {} {{ v }} }}\n", hn, name, vts, name, c * r, c * r, op.text(), rr, name)
                                });
                                format!("{}({}, {})", h, cur, val)
                            } else {
                                let h = self.componentwise(&format!("bin{}", op.text()), &[pt, vt], pt, &|p, x| p.scalar_binop(op, &x[0], &x[1], sc));
                                format!("{}({}, {})", h, cur, val)
                            }
                        }
                    }
                };
                Self::pad(out, indent);
                if let Some(s) = self.assign_special(f, *p, &res) {
                    writeln!(out, "{}", s).ok();
                    return;
                }
                writeln!(out, "{} = {};", cur, res).ok();
            }
            Stmt::Expr(e) => {
                if matches!(f.exprs[*e as usize].kind, ExprKind::Nop | ExprKind::Error) {
                    return;
                }
                let v = self.expr(f, *e);
                Self::pad(out, indent);
                writeln!(out, "let _ = {};", v).ok();
            }
            Stmt::If(c, a, b) => {
                let c = self.expr(f, *c);
                Self::pad(out, indent);
                writeln!(out, "if {} {{", c).ok();
                self.stmts(f, a, indent + 1, out);
                if !b.is_empty() {
                    Self::pad(out, indent);
                    out.push_str("} else {\n");
                    self.stmts(f, b, indent + 1, out);
                }
                Self::pad(out, indent);
                out.push_str("}\n");
            }
            Stmt::For(var, start, end, body) => {
                let v = self.local_name(f, *var);
                let s = self.expr(f, *start);
                let e = self.expr(f, *end);
                Self::pad(out, indent);
                writeln!(out, "for {} in ({})..({}) {{", v, s, e).ok();
                self.stmts(f, body, indent + 1, out);
                Self::pad(out, indent);
                out.push_str("}\n");
            }
            Stmt::Loop(body) => {
                Self::pad(out, indent);
                out.push_str("loop {\n");
                self.stmts(f, body, indent + 1, out);
                Self::pad(out, indent);
                out.push_str("}\n");
            }
            Stmt::Break => {
                Self::pad(out, indent);
                out.push_str("break;\n");
            }
            Stmt::Continue => {
                Self::pad(out, indent);
                out.push_str("continue;\n");
            }
            Stmt::Discard => {
                // The host reads the flag after the fragment function.
                let z = self.zero(f.ret);
                Self::pad(out, indent);
                if f.ret == TY_VOID {
                    out.push_str("rcx.discard = 1.0; return;\n");
                } else {
                    writeln!(out, "rcx.discard = 1.0; return {};", z).ok();
                }
            }
            Stmt::Return(e) => {
                Self::pad(out, indent);
                if *e == NO_EXPR || f.ret == TY_VOID {
                    out.push_str("return;\n");
                } else {
                    let v = self.expr(f, *e);
                    writeln!(out, "return {};", v).ok();
                }
            }
        }
    }

    fn function(&mut self, fi: FuncId, out: &mut String) {
        let m = self.m;
        let f = &m.functions[fi as usize];
        let mut params = vec!["rcx: &mut RenderCx".to_string()];
        for (k, p) in f.params.iter().enumerate() {
            let n = self.param_name(f, k as u32);
            let t = self.ty(p.ty);
            if p.inout {
                params.push(format!("{}: &mut {}", n, t));
            } else {
                params.push(format!("mut {}: {}", n, t));
            }
        }
        let ret = if f.ret == TY_VOID { String::new() } else { format!(" -> {}", self.ty(f.ret)) };
        writeln!(out, "fn {}({}){} {{", f.name, params.join(", "), ret).ok();
        self.stmts(f, &f.body, 1, out);
        if f.ret != TY_VOID {
            // Every path returns; this satisfies the type checker.
            let z = self.zero(f.ret);
            writeln!(out, "    #[allow(unreachable_code)]\n    {{ {} }}", z).ok();
        }
        out.push_str("}\n\n");
    }
}

fn f32_lit(v: f32) -> String {
    if v.is_nan() {
        return "f32::NAN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "f32::INFINITY".to_string() } else { "f32::NEG_INFINITY".to_string() };
    }
    let s = format!("{:?}", v);
    let s = if s.contains('.') || s.contains('e') { s } else { format!("{}.0", s) };
    if v < 0.0 || s.starts_with('-') {
        format!("({}f32)", s)
    } else {
        format!("{}f32", s)
    }
}

/// The texture kinds Rust shaders sample (every kind reads `Texture2D`).
pub fn rust_texture_kind(_tt: TextureType) -> &'static str {
    "Texture2D"
}

pub fn rust_shader_source_into(output: &ShaderOutput, types: &impl ShaderTypes, out: &mut String) -> bool {
    let source = rust_shader_source(output, types);
    out.push_str(&source.source);
    source.vertex_returns_position
}

impl RustShaderSource {
    pub fn source(&self) -> &str { &self.source }
    pub fn vertex_returns_position(&self) -> bool { self.vertex_returns_position }
}
