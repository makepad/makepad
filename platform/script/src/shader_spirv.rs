//! The SPIR-V backend: compiles the shader module the draw-shader emitter
//! writes (`shader_wgsl`, the module WebGPU runs as is) to one SPIR-V binary
//! per entry point, for Vulkan.
//!
//! The module is parsed once (`shader_spirv_parse`), its structs laid out
//! with the host-shareable rules the CPU side packs uniforms by, and each
//! entry point lowered on its own: only the functions and globals it reaches
//! are emitted, as a 1.3 module with the GLSL.std.450 instruction set.
//!
//! Lowering rules taken from naga's SPIR-V backend (MIT/Apache-2.0, The
//! gfx-rs developers; see platform/script/CREDITS.md), each marked where it
//! is used: integer division and remainder that never divide by zero or
//! overflow (back/spv/writer.rs `write_wrapped_binary_op`); float to integer
//! conversion clamped to the target's range (back/spv/block.rs `write_as`);
//! integer `clamp` as max-then-min, `saturate` as FClamp, `mix` with a
//! splatted scalar selector, `abs` of unsigned as a copy (block.rs, math
//! functions); `Flat` on every integer fragment input, builtins included,
//! and never on vertex inputs (writer.rs `write_varying`); uniform and
//! storage globals wrapped in a `Block` struct, matrices `ColMajor` with a
//! `MatrixStride`, read-only storage `NonWritable` (writer.rs
//! `write_global_variable`, `decorate_struct_member`); private globals
//! zero-initialized.

use crate::shader_spirv_parse::*;
use crate::spirv::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    Bool,
    F32,
    F16,
    I32,
    U32,
}

impl Scalar {
    fn is_float(self) -> bool {
        matches!(self, Scalar::F32 | Scalar::F16)
    }
    fn is_int(self) -> bool {
        matches!(self, Scalar::I32 | Scalar::U32)
    }
    fn width(self) -> u32 {
        if self == Scalar::F16 {
            2
        } else {
            4
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TexDim {
    D2,
    D2Array,
    D3,
    Cube,
    CubeArray,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Space {
    Function,
    Private,
    Uniform,
    Storage,
    Handle,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Ty {
    Void,
    Scalar(Scalar),
    Vector(u32, Scalar),
    /// Columns, rows; f32 elements.
    Matrix(u32, u32),
    /// Element and length; 0 is a runtime-sized array.
    Array(Box<Ty>, u32),
    Struct(usize),
    Ptr(Box<Ty>, Space),
    /// Dimension, depth, sampled scalar.
    Texture(TexDim, bool, Scalar),
    Sampler(bool),
}

impl Ty {
    fn leaf(&self) -> Option<Scalar> {
        match self {
            Ty::Scalar(s) | Ty::Vector(_, s) => Some(*s),
            Ty::Matrix(..) => Some(Scalar::F32),
            _ => None,
        }
    }
    fn with_leaf(&self, s: Scalar) -> Ty {
        match self {
            Ty::Vector(n, _) => Ty::Vector(*n, s),
            _ => Ty::Scalar(s),
        }
    }
    fn components(&self) -> u32 {
        match self {
            Ty::Vector(n, _) => *n,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug)]
struct StructInfo {
    name: String,
    members: Vec<(String, Ty, Attrs)>,
    offsets: Vec<u32>,
    size: u32,
    align: u32,
}

/// What an expression evaluated to.
#[derive(Clone, Debug)]
enum Val {
    /// An SSA value (a pointer value when `ty` is `Ty::Ptr`).
    V(u32, Ty),
    /// A reference: a pointer to memory holding a `Ty`.
    R(u32, Ty, Space),
    /// Abstract numbers: literals and what folds from them.
    AInt(i64),
    AFloat(f64),
    /// An entry point's input struct: its members, loaded.
    Fields(Vec<(String, Val)>),
}

struct LoopCx {
    merge: u32,
    cont: u32,
    broke: bool,
}

#[derive(Default)]
struct FnState {
    body: Vec<u32>,
    vars: Vec<u32>,
    scopes: Vec<Vec<(String, Val)>>,
    open: bool,
    loops: Vec<LoopCx>,
    ret: Option<Ty>,
    /// For an entry point: the output variables `return` stores to, with
    /// their member names (empty name for a bare return value).
    outputs: Vec<(String, u32, Ty)>,
    is_entry: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Vertex,
    Fragment,
}

struct Gen<'a> {
    m: &'a ModuleAst,
    b: SpvBuilder,
    structs: &'a [StructInfo],
    struct_ids: Vec<u32>,
    struct_names: HashMap<&'a str, usize>,
    stage: Stage,
    globals: HashMap<&'a str, usize>,
    global_ids: Vec<u32>,
    fns: HashMap<&'a str, usize>,
    fn_ids: Vec<u32>,
    fn_queue: Vec<usize>,
    interface: Vec<u32>,
    entry_fn: u32,
    depth_replacing: bool,
    f: FnState,
}

/// The vertex and fragment binaries of a module's `vertex_main` /
/// `fragment_main` (or whichever functions carry `@vertex` / `@fragment`).
pub fn compile_wgsl_to_spirv(src: &str) -> Result<(Option<Vec<u32>>, Option<Vec<u32>>), String> {
    let m = Parser::parse(src)?;
    let structs = resolve_structs(&m)?;
    let mut vertex = None;
    let mut fragment = None;
    for i in 0..m.fns.len() {
        let stage = match m.fns[i].attrs.stage.as_deref() {
            Some("vertex") => Stage::Vertex,
            Some("fragment") => Stage::Fragment,
            _ => continue,
        };
        let words = compile_entry(&m, &structs, i, stage)
            .map_err(|e| format!("{} (in entry point {})", e, m.fns[i].name))?;
        match stage {
            Stage::Vertex => vertex = Some(words),
            Stage::Fragment => fragment = Some(words),
        }
    }
    if vertex.is_none() && fragment.is_none() {
        return Err("shader module has no entry points".to_string());
    }
    Ok((vertex, fragment))
}

fn compile_entry(m: &ModuleAst, structs: &[StructInfo], entry: usize, stage: Stage) -> Result<Vec<u32>, String> {
    let mut g = Gen {
        m,
        b: SpvBuilder::new(),
        structs,
        struct_ids: vec![0; structs.len()],
        struct_names: HashMap::new(),
        stage,
        globals: HashMap::new(),
        global_ids: vec![0; m.globals.len()],
        fns: HashMap::new(),
        fn_ids: vec![0; m.fns.len()],
        fn_queue: Vec::new(),
        interface: Vec::new(),
        entry_fn: 0,
        depth_replacing: false,
        f: FnState::default(),
    };
    for (i, gl) in m.globals.iter().enumerate() {
        g.globals.insert(gl.name.as_str(), i);
    }
    for (i, st) in structs.iter().enumerate() {
        g.struct_names.insert(st.name.as_str(), i);
    }
    for (i, f) in m.fns.iter().enumerate() {
        g.fns.insert(f.name.as_str(), i);
    }
    g.entry_fn = g.b.id();
    g.fn_ids[entry] = g.entry_fn;
    g.gen_function(entry, true)?;
    while let Some(fi) = g.fn_queue.pop() {
        g.gen_function(fi, false)?;
    }
    let model = match stage {
        Stage::Vertex => EXEC_MODEL_VERTEX,
        Stage::Fragment => EXEC_MODEL_FRAGMENT,
    };
    let name = m.fns[entry].name.clone();
    let interface = std::mem::take(&mut g.interface);
    let entry_fn = g.entry_fn;
    g.b.entry_point(model, entry_fn, &name, &interface);
    if stage == Stage::Fragment {
        g.b.execution_mode(entry_fn, EXEC_MODE_ORIGIN_UPPER_LEFT);
        if g.depth_replacing {
            g.b.execution_mode(entry_fn, EXEC_MODE_DEPTH_REPLACING);
        }
    }
    Ok(g.b.finish())
}

fn round_up(v: u32, align: u32) -> u32 {
    (v + align - 1) / align * align
}

/// Size and alignment by the host-shareable layout rules (WGSL's, which
/// are std140/std430's for everything the draw path uploads).
fn layout(structs: &[StructInfo], ty: &Ty) -> (u32, u32) {
    match ty {
        Ty::Scalar(s) => (s.width(), s.width()),
        Ty::Vector(n, s) => {
            let w = s.width();
            match n {
                2 => (2 * w, 2 * w),
                3 => (3 * w, 4 * w),
                _ => (4 * w, 4 * w),
            }
        }
        Ty::Matrix(c, r) => {
            let (vs, va) = layout(structs, &Ty::Vector(*r, Scalar::F32));
            (c * round_up(vs, va), va)
        }
        Ty::Array(e, n) => {
            let (es, ea) = layout(structs, e);
            let stride = round_up(es, ea);
            (stride * (*n).max(1), ea)
        }
        Ty::Struct(i) => (structs[*i].size, structs[*i].align),
        _ => (4, 4),
    }
}

fn array_stride(structs: &[StructInfo], elem: &Ty) -> u32 {
    let (es, ea) = layout(structs, elem);
    round_up(es, ea)
}

fn resolve_structs(m: &ModuleAst) -> Result<Vec<StructInfo>, String> {
    let mut names: HashMap<&str, usize> = HashMap::new();
    for (i, s) in m.structs.iter().enumerate() {
        names.insert(s.name.as_str(), i);
    }
    // A struct's layout needs those of the structs it holds: resolve
    // recursively, in declaration order otherwise.
    let mut done = vec![false; m.structs.len()];
    let mut infos = Vec::with_capacity(m.structs.len());
    for _ in 0..m.structs.len() {
        infos.push(StructInfo { name: String::new(), members: Vec::new(), offsets: Vec::new(), size: 0, align: 1 });
    }
    for i in 0..m.structs.len() {
        resolve_struct(m, &names, i, &mut done, &mut infos, 0)?;
    }
    Ok(infos)
}

fn resolve_struct(
    m: &ModuleAst,
    names: &HashMap<&str, usize>,
    i: usize,
    done: &mut Vec<bool>,
    infos: &mut Vec<StructInfo>,
    depth: u32,
) -> Result<(), String> {
    if done[i] {
        return Ok(());
    }
    if depth > 64 {
        return Err(format!("struct {} is recursive", m.structs[i].name));
    }
    let s = &m.structs[i];
    let mut members = Vec::new();
    for mem in &s.members {
        let mut deps = Vec::new();
        collect_struct_names(&mem.ty, &mut deps);
        for d in deps {
            if let Some(j) = names.get(d.as_str()) {
                resolve_struct(m, names, *j, done, infos, depth + 1)?;
            }
        }
        let ty = resolve_type(names, infos, &mem.ty)?;
        members.push((mem.name.clone(), ty, mem.attrs.clone()));
    }
    let mut offsets = Vec::new();
    let mut offset = 0u32;
    let mut align = 1u32;
    for (_, ty, _) in &members {
        let (sz, al) = layout(infos, ty);
        offset = round_up(offset, al);
        offsets.push(offset);
        offset += sz;
        align = align.max(al);
    }
    let size = round_up(offset.max(1), align);
    infos[i] = StructInfo { name: s.name.clone(), members, offsets, size, align };
    done[i] = true;
    Ok(())
}

fn collect_struct_names(t: &TypeAst, out: &mut Vec<String>) {
    out.push(t.name.clone());
    for a in &t.args {
        collect_struct_names(a, out);
    }
}

fn scalar_by_name(name: &str) -> Option<Scalar> {
    match name {
        "f32" => Some(Scalar::F32),
        "f16" => Some(Scalar::F16),
        "i32" => Some(Scalar::I32),
        "u32" => Some(Scalar::U32),
        "bool" => Some(Scalar::Bool),
        _ => None,
    }
}

/// `vec3f`, `vec2<u32>`, `mat4x4f` ... as a type, None for any other name.
fn builtin_type_by_name(name: &str, args: &[Ty]) -> Option<Ty> {
    if let Some(s) = scalar_by_name(name) {
        return Some(Ty::Scalar(s));
    }
    let b = name.as_bytes();
    if name.starts_with("vec") && b.len() >= 4 {
        let n = (b[3] as char).to_digit(10)?;
        if !(2..=4).contains(&n) {
            return None;
        }
        let s = match &name[4..] {
            "" => match args.first() {
                Some(Ty::Scalar(s)) => *s,
                _ => return None,
            },
            "f" => Scalar::F32,
            "h" => Scalar::F16,
            "i" => Scalar::I32,
            "u" => Scalar::U32,
            _ => return None,
        };
        return Some(Ty::Vector(n, s));
    }
    if name.starts_with("mat") && b.len() >= 6 && b[4] == b'x' {
        let c = (b[3] as char).to_digit(10)?;
        let r = (b[5] as char).to_digit(10)?;
        if !(2..=4).contains(&c) || !(2..=4).contains(&r) {
            return None;
        }
        match &name[6..] {
            "" | "f" => {}
            _ => return None,
        }
        return Some(Ty::Matrix(c, r));
    }
    None
}

fn resolve_type(names: &HashMap<&str, usize>, structs: &[StructInfo], t: &TypeAst) -> Result<Ty, String> {
    let name = t.name.as_str();
    if let Some(i) = names.get(name) {
        return Ok(Ty::Struct(*i));
    }
    match name {
        "array" => {
            let elem = match t.args.first() {
                Some(a) => resolve_type(names, structs, a)?,
                None => return Err("array without an element type".to_string()),
            };
            let len = match t.args.get(1) {
                Some(n) => n
                    .name
                    .trim_end_matches(['u', 'i'])
                    .parse::<u32>()
                    .map_err(|_| format!("array length '{}' is not a number", n.name))?,
                None => 0,
            };
            return Ok(Ty::Array(Box::new(elem), len));
        }
        "ptr" => {
            let space = match t.args.first().map(|a| a.name.as_str()) {
                Some("function") => Space::Function,
                Some("private") => Space::Private,
                Some("uniform") => Space::Uniform,
                Some("storage") => Space::Storage,
                _ => return Err("unsupported pointer address space".to_string()),
            };
            let base = match t.args.get(1) {
                Some(a) => resolve_type(names, structs, a)?,
                None => return Err("pointer without a type".to_string()),
            };
            return Ok(Ty::Ptr(Box::new(base), space));
        }
        "sampler" => return Ok(Ty::Sampler(false)),
        "sampler_comparison" => return Ok(Ty::Sampler(true)),
        _ => {}
    }
    if let Some(rest) = name.strip_prefix("texture_") {
        let sampled = match t.args.first() {
            Some(a) => scalar_by_name(&a.name).unwrap_or(Scalar::F32),
            None => Scalar::F32,
        };
        let (dim, depth) = match rest {
            "2d" => (TexDim::D2, false),
            "2d_array" => (TexDim::D2Array, false),
            "3d" => (TexDim::D3, false),
            "cube" => (TexDim::Cube, false),
            "cube_array" => (TexDim::CubeArray, false),
            "depth_2d" => (TexDim::D2, true),
            "depth_2d_array" => (TexDim::D2Array, true),
            "depth_cube" => (TexDim::Cube, true),
            "depth_cube_array" => (TexDim::CubeArray, true),
            _ => return Err(format!("unsupported texture type {}", name)),
        };
        return Ok(Ty::Texture(dim, depth, if depth { Scalar::F32 } else { sampled }));
    }
    let mut args = Vec::new();
    for a in &t.args {
        args.push(resolve_type(names, structs, a)?);
    }
    match builtin_type_by_name(name, &args) {
        Some(ty) => Ok(ty),
        None => Err(format!("unknown type {}", name)),
    }
}

/// An f32 as IEEE half bits (round to nearest even).
fn f32_to_f16_bits(v: f32) -> u32 {
    let x = v.to_bits();
    let sign = (x >> 16) & 0x8000;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - e);
        let round = if (m & 0x1fff) > 0x1000 || ((m & 0x1fff) == 0x1000 && (m & 0x2000) != 0) { 1 } else { 0 };
        return sign | ((m >> 13) + round);
    }
    let round = if (mant & 0x1fff) > 0x1000 || ((mant & 0x1fff) == 0x1000 && (mant & 0x2000) != 0) { 1 } else { 0 };
    sign | (((e as u32) << 10) | (mant >> 13)) + round
}

fn swizzle_index(c: u8) -> Option<u32> {
    match c {
        b'x' | b'r' => Some(0),
        b'y' | b'g' => Some(1),
        b'z' | b'b' => Some(2),
        b'w' | b'a' => Some(3),
        _ => None,
    }
}

impl<'a> Gen<'a> {
    fn err<T>(&self, e: ExprId, msg: &str) -> Result<T, String> {
        let line = self.m.expr_lines.get(e as usize).copied().unwrap_or(0);
        Err(format!("line {}: {}", line, msg))
    }

    // ---- types ----

    fn ty_id(&mut self, ty: &Ty) -> u32 {
        match ty {
            Ty::Void => self.b.type_void(),
            Ty::Scalar(s) => self.scalar_id(*s),
            Ty::Vector(n, s) => {
                let c = self.scalar_id(*s);
                self.b.type_vector(c, *n)
            }
            Ty::Matrix(c, r) => {
                let col = self.ty_id(&Ty::Vector(*r, Scalar::F32));
                self.b.type_matrix(col, *c)
            }
            Ty::Array(e, n) => {
                let eid = self.ty_id(e);
                let stride = array_stride(self.structs, e);
                if *n == 0 {
                    self.b.type_runtime_array(eid, stride)
                } else {
                    self.b.type_array(eid, *n, stride)
                }
            }
            Ty::Struct(i) => self.struct_id(*i),
            Ty::Ptr(base, space) => {
                let bid = self.ty_id(base);
                self.b.type_pointer(storage_class(*space), bid)
            }
            Ty::Texture(dim, depth, s) => {
                let sid = self.scalar_id(*s);
                let (d, arrayed) = match dim {
                    TexDim::D2 => (DIM_2D, false),
                    TexDim::D2Array => (DIM_2D, true),
                    TexDim::D3 => (DIM_3D, false),
                    TexDim::Cube => (DIM_CUBE, false),
                    TexDim::CubeArray => {
                        self.b.capability(CAP_SAMPLED_CUBE_ARRAY);
                        (DIM_CUBE, true)
                    }
                };
                self.b.type_image(sid, d, *depth, arrayed)
            }
            Ty::Sampler(_) => self.b.type_sampler(),
        }
    }

    fn scalar_id(&mut self, s: Scalar) -> u32 {
        match s {
            Scalar::Bool => self.b.type_bool(),
            Scalar::F32 => self.b.type_f32(),
            Scalar::F16 => self.b.type_f16(),
            Scalar::I32 => self.b.type_i32(),
            Scalar::U32 => self.b.type_u32(),
        }
    }

    /// Matrices (also inside arrays) in a laid-out struct are column-major
    /// with an explicit column stride (naga back/spv/writer.rs
    /// `decorate_struct_member`).
    fn decorate_member_layout(&mut self, id: u32, index: u32, ty: &Ty, offset: u32) {
        self.b.member_decorate(id, index, DECO_OFFSET, &[offset]);
        let mut inner = ty;
        while let Ty::Array(e, _) = inner {
            inner = e;
        }
        if let Ty::Matrix(_, r) = inner {
            let (vs, va) = layout(self.structs, &Ty::Vector(*r, Scalar::F32));
            self.b.member_decorate(id, index, DECO_COL_MAJOR, &[]);
            self.b.member_decorate(id, index, DECO_MATRIX_STRIDE, &[round_up(vs, va)]);
        }
    }

    fn struct_id(&mut self, i: usize) -> u32 {
        if self.struct_ids[i] != 0 {
            return self.struct_ids[i];
        }
        let info = &self.structs[i];
        let mut member_ids = Vec::new();
        let n = info.members.len();
        for k in 0..n {
            let ty = self.structs[i].members[k].1.clone();
            member_ids.push(self.ty_id(&ty));
        }
        let id = self.b.id();
        let mut words = vec![id];
        words.extend_from_slice(&member_ids);
        spv_inst(&mut self.b.globals, OP_TYPE_STRUCT, &words);
        for k in 0..n {
            let ty = self.structs[i].members[k].1.clone();
            let off = self.structs[i].offsets[k];
            self.decorate_member_layout(id, k as u32, &ty, off);
        }
        self.struct_ids[i] = id;
        id
    }

    fn resolve(&self, t: &TypeAst) -> Result<Ty, String> {
        resolve_type(&self.struct_names, self.structs, t)
    }

    // ---- instruction helpers ----

    fn emit(&mut self, op: u32, operands: &[u32]) {
        spv_inst(&mut self.f.body, op, operands);
    }

    /// `op type result args...`, returning the result id.
    fn ins(&mut self, op: u32, ty: &Ty, args: &[u32]) -> u32 {
        let tid = self.ty_id(ty);
        let id = self.b.id();
        let mut words = Vec::with_capacity(args.len() + 2);
        words.push(tid);
        words.push(id);
        words.extend_from_slice(args);
        spv_inst(&mut self.f.body, op, &words);
        id
    }

    fn ext(&mut self, gl: u32, ty: &Ty, args: &[u32]) -> u32 {
        let mut words = Vec::with_capacity(args.len() + 2);
        words.push(self.b.glsl_ext);
        words.push(gl);
        words.extend_from_slice(args);
        self.ins(OP_EXT_INST, ty, &words)
    }

    fn label(&mut self, id: u32) {
        self.emit(OP_LABEL, &[id]);
        self.f.open = true;
    }

    fn branch(&mut self, target: u32) {
        self.emit(OP_BRANCH, &[target]);
        self.f.open = false;
    }

    /// A function-scope variable (declared in the entry block).
    fn local_var(&mut self, ty: &Ty) -> u32 {
        let ptr = self.ty_id(&Ty::Ptr(Box::new(ty.clone()), Space::Function));
        let id = self.b.id();
        spv_inst(&mut self.f.vars, OP_VARIABLE, &[ptr, id, SC_FUNCTION]);
        id
    }

    // ---- constants ----

    fn const_scalar_f(&mut self, s: Scalar, v: f64) -> u32 {
        match s {
            Scalar::F32 => self.b.const_f32(v as f32),
            Scalar::F16 => {
                let ty = self.b.type_f16();
                self.b.intern_const(OP_CONSTANT, ty, &[f32_to_f16_bits(v as f32)])
            }
            Scalar::I32 => self.b.const_i32(v as i32),
            Scalar::U32 => self.b.const_u32(v as u32),
            Scalar::Bool => self.b.const_bool(v != 0.0),
        }
    }

    fn const_scalar_i(&mut self, s: Scalar, v: i64) -> u32 {
        match s {
            Scalar::I32 => self.b.const_i32(v as i32),
            Scalar::U32 => self.b.const_u32(v as u32),
            _ => self.const_scalar_f(s, v as f64),
        }
    }

    /// A constant of `ty` (scalar or vector) with every component `v`.
    fn const_splat(&mut self, ty: &Ty, v: f64) -> u32 {
        match ty {
            Ty::Vector(n, s) => {
                let c = self.const_scalar_f(*s, v);
                let parts = vec![c; *n as usize];
                let tid = self.ty_id(ty);
                self.b.const_composite(tid, &parts)
            }
            Ty::Scalar(s) => self.const_scalar_f(*s, v),
            _ => {
                let tid = self.ty_id(ty);
                self.b.const_null(tid)
            }
        }
    }

    fn splat_value(&mut self, ty: &Ty, scalar: u32) -> u32 {
        match ty {
            Ty::Vector(n, _) => {
                let parts = vec![scalar; *n as usize];
                self.ins(OP_COMPOSITE_CONSTRUCT, ty, &parts)
            }
            _ => scalar,
        }
    }

    // ---- values ----

    fn load(&mut self, v: Val) -> Val {
        match v {
            Val::R(ptr, ty, _) => {
                let id = self.ins(OP_LOAD, &ty, &[ptr]);
                Val::V(id, ty)
            }
            other => other,
        }
    }

    /// A concrete value; abstract numbers become `prefer` (f32 / i32 when
    /// None).
    fn concrete(&mut self, v: Val, prefer: Option<Scalar>, e: ExprId) -> Result<(u32, Ty), String> {
        match self.load(v) {
            Val::V(id, ty) => Ok((id, ty)),
            Val::AInt(i) => {
                let s = prefer.unwrap_or(Scalar::I32);
                Ok((self.const_scalar_i(s, i), Ty::Scalar(s)))
            }
            Val::AFloat(f) => {
                let s = match prefer {
                    Some(s) if s.is_float() => s,
                    _ => Scalar::F32,
                };
                Ok((self.const_scalar_f(s, f), Ty::Scalar(s)))
            }
            Val::Fields(_) => self.err(e, "an entry point input struct is not a value"),
            Val::R(..) => unreachable!(),
        }
    }

    /// The value converted to `to`: abstract numbers as constants of it
    /// (splatted for a vector), concrete values through `convert`.
    fn coerce(&mut self, v: Val, to: &Ty, e: ExprId) -> Result<u32, String> {
        match v {
            Val::AInt(i) => {
                let s = match to.leaf() {
                    Some(s) => s,
                    None => return self.err(e, "a number where a non-numeric value is expected"),
                };
                let c = self.const_scalar_i(s, i);
                Ok(self.splat_const(to, c))
            }
            Val::AFloat(f) => {
                let s = match to.leaf() {
                    Some(s) if s.is_float() => s,
                    _ => return self.err(e, "a float where an integer is expected"),
                };
                let c = self.const_scalar_f(s, f);
                Ok(self.splat_const(to, c))
            }
            v => {
                let (id, ty) = self.concrete(v, to.leaf(), e)?;
                self.convert(id, &ty, to, e)
            }
        }
    }

    fn splat_const(&mut self, ty: &Ty, c: u32) -> u32 {
        match ty {
            Ty::Vector(n, _) => {
                let parts = vec![c; *n as usize];
                let tid = self.ty_id(ty);
                self.b.const_composite(tid, &parts)
            }
            _ => c,
        }
    }

    /// Value conversion between scalar / vector types of the same shape
    /// (naga back/spv/block.rs `write_as`: float to integer clamps to the
    /// target's representable range first).
    fn convert(&mut self, id: u32, from: &Ty, to: &Ty, e: ExprId) -> Result<u32, String> {
        if from == to {
            return Ok(id);
        }
        let (fs, ts) = match (from, to) {
            (Ty::Scalar(a), Ty::Scalar(b)) => (*a, *b),
            (Ty::Vector(n, a), Ty::Vector(m, b)) if n == m => (*a, *b),
            _ => return self.err(e, &format!("cannot convert {:?} to {:?}", from, to)),
        };
        Ok(match (fs, ts) {
            (Scalar::Bool, _) => {
                let one = self.const_splat(to, 1.0);
                let zero = self.const_splat(to, 0.0);
                self.ins(OP_SELECT, to, &[id, one, zero])
            }
            (_, Scalar::Bool) => {
                let zero = self.const_splat(from, 0.0);
                let op = if fs.is_float() { OP_F_ORD_NOT_EQUAL } else { OP_I_NOT_EQUAL };
                self.ins(op, to, &[id, zero])
            }
            (Scalar::F32 | Scalar::F16, Scalar::I32 | Scalar::U32) => {
                let (lo, hi) = if ts == Scalar::I32 {
                    (-2147483648.0, 2147483520.0)
                } else {
                    (0.0, 4294967040.0)
                };
                let lo = self.const_splat(from, lo);
                let hi = self.const_splat(from, hi);
                let clamped = self.ext(GL_F_CLAMP, from, &[id, lo, hi]);
                let op = if ts == Scalar::I32 { OP_CONVERT_F_TO_S } else { OP_CONVERT_F_TO_U };
                self.ins(op, to, &[clamped])
            }
            (Scalar::I32, Scalar::F32 | Scalar::F16) => self.ins(OP_CONVERT_S_TO_F, to, &[id]),
            (Scalar::U32, Scalar::F32 | Scalar::F16) => self.ins(OP_CONVERT_U_TO_F, to, &[id]),
            (Scalar::F32, Scalar::F16) | (Scalar::F16, Scalar::F32) => self.ins(OP_F_CONVERT, to, &[id]),
            _ => self.ins(OP_BITCAST, to, &[id]),
        })
    }

    // ---- globals and functions ----

    fn fn_id(&mut self, fi: usize) -> u32 {
        if self.fn_ids[fi] == 0 {
            self.fn_ids[fi] = self.b.id();
            self.fn_queue.push(fi);
        }
        self.fn_ids[fi]
    }

    fn global(&mut self, gi: usize, e: ExprId) -> Result<Val, String> {
        let gl = &self.m.globals[gi];
        if gl.space == "const" {
            let init = match gl.init {
                Some(i) => i,
                None => return self.err(e, "const without a value"),
            };
            let v = self.expr(init)?;
            return match &gl.ty {
                Some(t) => {
                    let ty = self.resolve(t)?;
                    let id = self.coerce(v, &ty, init)?;
                    Ok(Val::V(id, ty))
                }
                None => Ok(v),
            };
        }
        let ty = match &gl.ty {
            Some(t) => self.resolve(t)?,
            None => return self.err(e, "global without a type"),
        };
        let space = match gl.space.as_str() {
            "private" => Space::Private,
            "uniform" => Space::Uniform,
            "storage" => Space::Storage,
            "" | "handle" => Space::Handle,
            other => return self.err(e, &format!("unsupported address space {}", other)),
        };
        let wrapped = matches!(space, Space::Uniform | Space::Storage);
        if self.global_ids[gi] == 0 {
            let id = match space {
                Space::Private => {
                    let ptr = self.ty_id(&Ty::Ptr(Box::new(ty.clone()), Space::Private));
                    let init = match gl.init {
                        Some(ie) => self.const_expr(ie, &ty)?,
                        None => {
                            let tid = self.ty_id(&ty);
                            self.b.const_null(tid)
                        }
                    };
                    self.b.global_variable(ptr, SC_PRIVATE, init)
                }
                Space::Uniform | Space::Storage => {
                    // naga back/spv/writer.rs `write_global_variable`: the
                    // buffer's type wrapped in a `Block` struct.
                    let inner = self.ty_id(&ty);
                    let wrapper = self.b.id();
                    spv_inst(&mut self.b.globals, OP_TYPE_STRUCT, &[wrapper, inner]);
                    self.b.decorate(wrapper, DECO_BLOCK, &[]);
                    self.decorate_member_layout(wrapper, 0, &ty, 0);
                    let sc = storage_class(space);
                    let ptr = self.b.type_pointer(sc, wrapper);
                    let var = self.b.global_variable(ptr, sc, 0);
                    if space == Space::Storage && !gl.write {
                        self.b.decorate(var, DECO_NON_WRITABLE, &[]);
                    }
                    var
                }
                _ => {
                    let ptr = self.ty_id(&Ty::Ptr(Box::new(ty.clone()), Space::Handle));
                    self.b.global_variable(ptr, SC_UNIFORM_CONSTANT, 0)
                }
            };
            if space != Space::Private {
                self.b.decorate(id, DECO_DESCRIPTOR_SET, &[gl.attrs.group.unwrap_or(0)]);
                self.b.decorate(id, DECO_BINDING, &[gl.attrs.binding.unwrap_or(0)]);
            }
            self.global_ids[gi] = id;
        }
        let var = self.global_ids[gi];
        if wrapped {
            let ptr_ty = Ty::Ptr(Box::new(ty.clone()), space);
            let zero = self.b.const_u32(0);
            let p = self.ins(OP_ACCESS_CHAIN, &ptr_ty, &[var, zero]);
            Ok(Val::R(p, ty, space))
        } else {
            Ok(Val::R(var, ty, space))
        }
    }

    /// A module-scope initializer: a literal, a negated literal, or a
    /// constructor of those.
    fn const_expr(&mut self, e: ExprId, ty: &Ty) -> Result<u32, String> {
        let m = self.m;
        match &m.exprs[e as usize] {
            Expr::Int(v, _) => match ty.leaf() {
                Some(s) => {
                    let c = self.const_scalar_i(s, *v);
                    Ok(self.splat_const(ty, c))
                }
                None => self.err(e, "a number initializing a non-numeric global"),
            },
            Expr::Float(v, _) => match ty.leaf() {
                Some(s) => {
                    let c = self.const_scalar_f(s, *v);
                    Ok(self.splat_const(ty, c))
                }
                None => self.err(e, "a number initializing a non-numeric global"),
            },
            Expr::Bool(b) => Ok(self.b.const_bool(*b)),
            Expr::Unary(UnOp::Neg, inner) => match &m.exprs[*inner as usize] {
                Expr::Int(v, _) => {
                    let s = ty.leaf().unwrap_or(Scalar::I32);
                    let c = self.const_scalar_i(s, -*v);
                    Ok(self.splat_const(ty, c))
                }
                Expr::Float(v, _) => {
                    let s = ty.leaf().unwrap_or(Scalar::F32);
                    let c = self.const_scalar_f(s, -*v);
                    Ok(self.splat_const(ty, c))
                }
                _ => self.err(e, "a global initializer must be a constant"),
            },
            Expr::Call { args, .. } => {
                let tid = self.ty_id(ty);
                if args.is_empty() {
                    return Ok(self.b.const_null(tid));
                }
                let parts_ty = match ty {
                    Ty::Vector(_, s) => Ty::Scalar(*s),
                    Ty::Matrix(_, r) => Ty::Vector(*r, Scalar::F32),
                    _ => return self.err(e, "a global initializer must be a constant"),
                };
                if args.len() == 1 {
                    let c = self.const_expr(args[0], &parts_ty)?;
                    return Ok(self.splat_const(ty, c));
                }
                let mut parts = Vec::new();
                for a in args {
                    parts.push(self.const_expr(*a, &parts_ty)?);
                }
                Ok(self.b.const_composite(tid, &parts))
            }
            _ => self.err(e, "a global initializer must be a constant"),
        }
    }

    /// An Input or Output interface variable for an entry point.
    fn io_var(&mut self, ty: &Ty, input: bool, attrs: &Attrs, e: ExprId) -> Result<u32, String> {
        let sc = if input { SC_INPUT } else { SC_OUTPUT };
        let tid = self.ty_id(ty);
        let ptr = self.b.type_pointer(sc, tid);
        let var = self.b.global_variable(ptr, sc, 0);
        self.interface.push(var);
        let is_int = matches!(ty.leaf(), Some(s) if s.is_int() || s == Scalar::Bool);
        if let Some(loc) = attrs.location {
            self.b.decorate(var, DECO_LOCATION, &[loc]);
            // naga back/spv/writer.rs `write_varying`: no interpolation
            // decorations on vertex inputs or fragment outputs.
            let allowed = !(input && self.stage == Stage::Vertex) && !(!input && self.stage == Stage::Fragment);
            if allowed && attrs.flat {
                self.b.decorate(var, DECO_FLAT, &[]);
            } else if allowed && input && is_int {
                self.b.decorate(var, DECO_FLAT, &[]);
            }
        } else if let Some(builtin) = &attrs.builtin {
            let bi = match builtin.as_str() {
                "position" => {
                    if input {
                        BUILTIN_FRAG_COORD
                    } else {
                        BUILTIN_POSITION
                    }
                }
                "vertex_index" => BUILTIN_VERTEX_INDEX,
                "instance_index" => BUILTIN_INSTANCE_INDEX,
                "view_index" => {
                    self.b.capability(CAP_MULTI_VIEW);
                    BUILTIN_VIEW_INDEX
                }
                "front_facing" => BUILTIN_FRONT_FACING,
                "frag_depth" => {
                    self.depth_replacing = true;
                    BUILTIN_FRAG_DEPTH
                }
                other => return self.err(e, &format!("unsupported builtin {}", other)),
            };
            if attrs.invariant {
                self.b.decorate(var, DECO_INVARIANT, &[]);
            }
            self.b.decorate(var, DECO_BUILT_IN, &[bi]);
            // naga back/spv/writer.rs `write_varying`: every integer
            // fragment input is Flat (VUID-StandaloneSpirv-Flat-04744),
            // builtins included.
            if input && self.stage == Stage::Fragment && is_int {
                self.b.decorate(var, DECO_FLAT, &[]);
            }
        } else {
            return self.err(e, "entry point input/output without @location or @builtin");
        }
        Ok(var)
    }

    fn gen_function(&mut self, fi: usize, is_entry: bool) -> Result<(), String> {
        let m = self.m;
        let f = &m.fns[fi];
        let fid = self.fn_ids[fi];
        self.f = FnState { open: true, is_entry, ..Default::default() };
        self.f.scopes.push(Vec::new());
        let mut header = Vec::new();
        let ret_ty = match &f.ret {
            Some(t) => self.resolve(t)?,
            None => Ty::Void,
        };
        let line_e = 0;
        let mut prologue: Vec<(String, Val)> = Vec::new();
        if is_entry {
            let void = self.b.type_void();
            let fnty = self.b.type_function(void, &[]);
            spv_inst(&mut header, OP_FUNCTION, &[void, fid, 0, fnty]);
            // Inputs: loaded at the top of the entry block.
            let mut loads: Vec<(String, u32, Ty, Option<String>)> = Vec::new();
            for p in &f.params {
                let ty = self.resolve(&p.ty)?;
                if let Ty::Struct(si) = ty {
                    let n = self.structs[si].members.len();
                    for k in 0..n {
                        let (mname, mty, mattrs) = self.structs[si].members[k].clone();
                        let var = self.io_var(&mty, true, &mattrs, line_e)?;
                        loads.push((mname, var, mty, Some(p.name.clone())));
                    }
                } else {
                    let var = self.io_var(&ty, true, &p.attrs, line_e)?;
                    loads.push((p.name.clone(), var, ty, None));
                }
            }
            // Outputs.
            if let Ty::Struct(si) = ret_ty {
                let n = self.structs[si].members.len();
                for k in 0..n {
                    let (mname, mty, mattrs) = self.structs[si].members[k].clone();
                    let var = self.io_var(&mty, false, &mattrs, line_e)?;
                    self.f.outputs.push((mname, var, mty));
                }
            } else if ret_ty != Ty::Void {
                let var = self.io_var(&ret_ty, false, &f.ret_attrs, line_e)?;
                self.f.outputs.push((String::new(), var, ret_ty.clone()));
            }
            self.f.ret = Some(ret_ty.clone());
            for (name, var, ty, parent) in loads {
                let id = self.ins(OP_LOAD, &ty, &[var]);
                match parent {
                    Some(pname) => {
                        let mut found = false;
                        for entry in prologue.iter_mut() {
                            if entry.0 == pname {
                                if let Val::Fields(fields) = &mut entry.1 {
                                    fields.push((name.clone(), Val::V(id, ty.clone())));
                                }
                                found = true;
                            }
                        }
                        if !found {
                            prologue.push((pname, Val::Fields(vec![(name, Val::V(id, ty))])));
                        }
                    }
                    None => prologue.push((name, Val::V(id, ty))),
                }
            }
        } else {
            let ret_id = self.ty_id(&ret_ty);
            let mut param_tys = Vec::new();
            let mut param_ids = Vec::new();
            for p in &f.params {
                let ty = self.resolve(&p.ty)?;
                param_ids.push(self.ty_id(&ty));
                param_tys.push(ty);
            }
            let fnty = self.b.type_function(ret_id, &param_ids);
            spv_inst(&mut header, OP_FUNCTION, &[ret_id, fid, 0, fnty]);
            for (k, p) in f.params.iter().enumerate() {
                let id = self.b.id();
                spv_inst(&mut header, OP_FUNCTION_PARAMETER, &[param_ids[k], id]);
                prologue.push((p.name.clone(), Val::V(id, param_tys[k].clone())));
            }
            self.f.ret = Some(ret_ty.clone());
        }
        for p in prologue {
            self.f.scopes[0].push(p);
        }
        self.stmts(&f.body)?;
        if self.f.open {
            if ret_ty == Ty::Void || is_entry {
                self.emit(OP_RETURN, &[]);
            } else {
                self.emit(OP_UNREACHABLE, &[]);
            }
            self.f.open = false;
        }
        let entry_label = self.b.id();
        let out = &mut self.b.functions;
        out.extend_from_slice(&header);
        spv_inst(out, OP_LABEL, &[entry_label]);
        out.extend_from_slice(&self.f.vars);
        out.extend_from_slice(&self.f.body);
        spv_inst(out, OP_FUNCTION_END, &[]);
        Ok(())
    }

    // ---- statements ----

    fn lookup(&self, name: &str) -> Option<Val> {
        for scope in self.f.scopes.iter().rev() {
            for (n, v) in scope.iter().rev() {
                if n == name {
                    return Some(v.clone());
                }
            }
        }
        None
    }

    fn bind(&mut self, name: &str, v: Val) {
        let scope = self.f.scopes.last_mut().unwrap();
        scope.push((name.to_string(), v));
    }

    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        for s in stmts {
            if !self.f.open {
                // Dead code after a return / break / continue / discard.
                break;
            }
            self.stmt(s)?;
        }
        Ok(())
    }

    fn scoped(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        self.f.scopes.push(Vec::new());
        let r = self.stmts(stmts);
        self.f.scopes.pop();
        r
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), String> {
        match s {
            Stmt::Let { name, ty, init } => {
                let v = self.expr(*init)?;
                let v = match ty {
                    Some(t) => {
                        let ty = self.resolve(t)?;
                        let id = self.coerce(v, &ty, *init)?;
                        Val::V(id, ty)
                    }
                    None => match self.load(v) {
                        Val::AInt(i) => Val::V(self.b.const_i32(i as i32), Ty::Scalar(Scalar::I32)),
                        Val::AFloat(f) => Val::V(self.b.const_f32(f as f32), Ty::Scalar(Scalar::F32)),
                        other => other,
                    },
                };
                self.bind(name, v);
            }
            Stmt::Var { name, ty, init } => {
                let declared = match ty {
                    Some(t) => Some(self.resolve(t)?),
                    None => None,
                };
                let (value, vty) = match (init, declared) {
                    (Some(ie), Some(dt)) => {
                        let v = self.expr(*ie)?;
                        (self.coerce(v, &dt, *ie)?, dt)
                    }
                    (Some(ie), None) => {
                        let v = self.expr(*ie)?;
                        self.concrete(v, None, *ie)?
                    }
                    (None, Some(dt)) => {
                        let tid = self.ty_id(&dt);
                        (self.b.const_null(tid), dt)
                    }
                    (None, None) => return Err(format!("var {} has neither a type nor a value", name)),
                };
                let var = self.local_var(&vty);
                self.emit(OP_STORE, &[var, value]);
                self.bind(name, Val::R(var, vty, Space::Function));
            }
            Stmt::Assign { lhs, op, rhs } => {
                let target = self.expr(*lhs)?;
                let (ptr, ty) = match target {
                    Val::R(p, t, _) => (p, t),
                    _ => return self.err(*lhs, "assignment to something that is not a reference"),
                };
                let value = match op {
                    None => {
                        let v = self.expr(*rhs)?;
                        self.coerce(v, &ty, *rhs)?
                    }
                    Some(op) => {
                        let cur = self.ins(OP_LOAD, &ty, &[ptr]);
                        let r = self.expr(*rhs)?;
                        let res = self.binary(*op, Val::V(cur, ty.clone()), r, *rhs)?;
                        self.coerce(res, &ty, *rhs)?
                    }
                };
                self.emit(OP_STORE, &[ptr, value]);
            }
            Stmt::Incr(lhs, up) => {
                let target = self.expr(*lhs)?;
                let (ptr, ty) = match target {
                    Val::R(p, t, _) => (p, t),
                    _ => return self.err(*lhs, "increment of something that is not a reference"),
                };
                let cur = self.ins(OP_LOAD, &ty, &[ptr]);
                let op = if *up { BinOp::Add } else { BinOp::Sub };
                let res = self.binary(op, Val::V(cur, ty.clone()), Val::AInt(1), *lhs)?;
                let value = self.coerce(res, &ty, *lhs)?;
                self.emit(OP_STORE, &[ptr, value]);
            }
            Stmt::Phony(e) | Stmt::Call(e) => {
                let v = self.expr(*e)?;
                // A reference read for its effects only needs no load.
                let _ = v;
            }
            Stmt::Block(b) => self.scoped(b)?,
            Stmt::If { cond, then, els } => {
                let c = self.expr(*cond)?;
                let c = self.coerce(c, &Ty::Scalar(Scalar::Bool), *cond)?;
                let then_l = self.b.id();
                let merge_l = self.b.id();
                let else_l = if els.is_empty() { merge_l } else { self.b.id() };
                self.emit(OP_SELECTION_MERGE, &[merge_l, 0]);
                self.emit(OP_BRANCH_CONDITIONAL, &[c, then_l, else_l]);
                self.label(then_l);
                self.scoped(then)?;
                let mut reaches_merge = els.is_empty();
                if self.f.open {
                    self.branch(merge_l);
                    reaches_merge = true;
                }
                if !els.is_empty() {
                    self.label(else_l);
                    self.scoped(els)?;
                    if self.f.open {
                        self.branch(merge_l);
                        reaches_merge = true;
                    }
                }
                self.label(merge_l);
                if !reaches_merge {
                    self.emit(OP_UNREACHABLE, &[]);
                    self.f.open = false;
                }
            }
            Stmt::Loop { body, continuing, break_if } => {
                self.gen_loop(None, body, continuing, *break_if)?;
            }
            Stmt::For { init, cond, update, body } => {
                self.f.scopes.push(Vec::new());
                if let Some(i) = init {
                    self.stmt(i)?;
                }
                let mut cont = Vec::new();
                if let Some(u) = update {
                    cont.push((**u).clone());
                }
                let r = self.gen_loop(*cond, body, &cont, None);
                self.f.scopes.pop();
                r?;
            }
            Stmt::While { cond, body } => {
                self.gen_loop(Some(*cond), body, &[], None)?;
            }
            Stmt::Break => {
                let merge = match self.f.loops.last_mut() {
                    Some(l) => {
                        l.broke = true;
                        l.merge
                    }
                    None => return Err("break outside a loop".to_string()),
                };
                self.branch(merge);
            }
            Stmt::Continue => {
                let cont = match self.f.loops.last() {
                    Some(l) => l.cont,
                    None => return Err("continue outside a loop".to_string()),
                };
                self.branch(cont);
            }
            Stmt::Discard => {
                self.emit(OP_KILL, &[]);
                self.f.open = false;
            }
            Stmt::Return(value) => {
                if self.f.is_entry {
                    if let Some(ve) = value {
                        let v = self.expr(*ve)?;
                        let outputs = self.f.outputs.clone();
                        if outputs.len() == 1 && outputs[0].0.is_empty() {
                            let id = self.coerce(v, &outputs[0].2, *ve)?;
                            self.emit(OP_STORE, &[outputs[0].1, id]);
                        } else {
                            let (id, _) = self.concrete(v, None, *ve)?;
                            for (k, (_, var, ty)) in outputs.iter().enumerate() {
                                let part = self.ins(OP_COMPOSITE_EXTRACT, ty, &[id, k as u32]);
                                self.emit(OP_STORE, &[*var, part]);
                            }
                        }
                    }
                    self.emit(OP_RETURN, &[]);
                } else {
                    match value {
                        Some(ve) => {
                            let v = self.expr(*ve)?;
                            let ret = self.f.ret.clone().unwrap_or(Ty::Void);
                            let id = self.coerce(v, &ret, *ve)?;
                            self.emit(OP_RETURN_VALUE, &[id]);
                        }
                        None => self.emit(OP_RETURN, &[]),
                    }
                }
                self.f.open = false;
            }
        }
        Ok(())
    }

    /// `loop`, `for` and `while`: a header block with the loop merge, the
    /// body (opening with the exit test when there is a condition), and the
    /// continue block holding the continuing statements and the back edge.
    fn gen_loop(
        &mut self,
        cond: Option<ExprId>,
        body: &[Stmt],
        continuing: &[Stmt],
        break_if: Option<ExprId>,
    ) -> Result<(), String> {
        let header = self.b.id();
        let body_l = self.b.id();
        let cont_l = self.b.id();
        let merge_l = self.b.id();
        self.branch(header);
        self.label(header);
        self.emit(OP_LOOP_MERGE, &[merge_l, cont_l, 0]);
        self.branch(body_l);
        self.label(body_l);
        self.f.loops.push(LoopCx { merge: merge_l, cont: cont_l, broke: false });
        self.f.scopes.push(Vec::new());
        let mut result = Ok(());
        if let Some(c) = cond {
            match self.expr(c).and_then(|v| self.coerce(v, &Ty::Scalar(Scalar::Bool), c)) {
                Ok(cv) => {
                    let go = self.b.id();
                    let stop = self.b.id();
                    self.emit(OP_SELECTION_MERGE, &[go, 0]);
                    self.emit(OP_BRANCH_CONDITIONAL, &[cv, go, stop]);
                    self.label(stop);
                    self.f.loops.last_mut().unwrap().broke = true;
                    self.branch(merge_l);
                    self.label(go);
                }
                Err(e) => result = Err(e),
            }
        }
        if result.is_ok() {
            result = self.stmts(body);
        }
        if result.is_ok() {
            if self.f.open {
                self.branch(cont_l);
            }
            self.label(cont_l);
            result = self.stmts(continuing);
        }
        if result.is_ok() && self.f.open {
            match break_if {
                Some(bi) => match self.expr(bi).and_then(|v| self.coerce(v, &Ty::Scalar(Scalar::Bool), bi)) {
                    Ok(c) => {
                        self.f.loops.last_mut().unwrap().broke = true;
                        self.emit(OP_BRANCH_CONDITIONAL, &[c, merge_l, header]);
                        self.f.open = false;
                    }
                    Err(e) => result = Err(e),
                },
                None => self.branch(header),
            }
        }
        self.f.scopes.pop();
        let lp = self.f.loops.pop().unwrap();
        result?;
        self.label(merge_l);
        if !lp.broke {
            self.emit(OP_UNREACHABLE, &[]);
            self.f.open = false;
        }
        Ok(())
    }

    // ---- expressions ----

    fn expr(&mut self, e: ExprId) -> Result<Val, String> {
        let m = self.m;
        match &m.exprs[e as usize] {
            Expr::Int(v, suffix) => Ok(match suffix {
                b'u' => Val::V(self.b.const_u32(*v as u32), Ty::Scalar(Scalar::U32)),
                b'i' => Val::V(self.b.const_i32(*v as i32), Ty::Scalar(Scalar::I32)),
                _ => Val::AInt(*v),
            }),
            Expr::Float(v, suffix) => Ok(match suffix {
                b'f' => Val::V(self.b.const_f32(*v as f32), Ty::Scalar(Scalar::F32)),
                b'h' => Val::V(self.const_scalar_f(Scalar::F16, *v), Ty::Scalar(Scalar::F16)),
                _ => Val::AFloat(*v),
            }),
            Expr::Bool(b) => Ok(Val::V(self.b.const_bool(*b), Ty::Scalar(Scalar::Bool))),
            Expr::Ident(name) => {
                if let Some(v) = self.lookup(name) {
                    return Ok(v);
                }
                if let Some(gi) = self.globals.get(name.as_str()).copied() {
                    return self.global(gi, e);
                }
                self.err(e, &format!("unknown identifier {}", name))
            }
            Expr::Unary(op, a) => {
                let v = self.expr(*a)?;
                self.unary(*op, v, *a)
            }
            Expr::Binary(op, a, b) => {
                let l = self.expr(*a)?;
                let r = self.expr(*b)?;
                self.binary(*op, l, r, e)
            }
            Expr::Member(base, name) => {
                let v = self.expr(*base)?;
                self.member(v, name, e)
            }
            Expr::Index(base, index) => {
                let v = self.expr(*base)?;
                self.index(v, *index, e)
            }
            Expr::AddrOf(a) => match self.expr(*a)? {
                Val::R(p, ty, space) => Ok(Val::V(p, Ty::Ptr(Box::new(ty), space))),
                _ => self.err(e, "& of something that is not a reference"),
            },
            Expr::Deref(a) => match self.expr(*a)? {
                Val::V(p, Ty::Ptr(ty, space)) => Ok(Val::R(p, *ty, space)),
                Val::R(p, ty, space) => Ok(Val::R(p, ty, space)),
                _ => self.err(e, "* of something that is not a pointer"),
            },
            Expr::Call { name, templ, args } => self.call(name, templ, args, e),
        }
    }

    fn unary(&mut self, op: UnOp, v: Val, e: ExprId) -> Result<Val, String> {
        match (op, &v) {
            (UnOp::Neg, Val::AInt(i)) => return Ok(Val::AInt(-*i)),
            (UnOp::Neg, Val::AFloat(f)) => return Ok(Val::AFloat(-*f)),
            (UnOp::BitNot, Val::AInt(i)) => return Ok(Val::AInt(!*i)),
            _ => {}
        }
        let (id, ty) = self.concrete(v, None, e)?;
        let s = match ty.leaf() {
            Some(s) => s,
            None => return self.err(e, "unary operator on a non-numeric value"),
        };
        let r = match op {
            UnOp::Neg => {
                if s.is_float() {
                    self.ins(OP_F_NEGATE, &ty, &[id])
                } else {
                    self.ins(OP_S_NEGATE, &ty, &[id])
                }
            }
            UnOp::Not => self.ins(OP_LOGICAL_NOT, &ty, &[id]),
            UnOp::BitNot => self.ins(OP_NOT, &ty, &[id]),
        };
        Ok(Val::V(r, ty))
    }

    fn fold(&mut self, op: BinOp, l: &Val, r: &Val, e: ExprId) -> Result<Option<Val>, String> {
        let as_f = |v: &Val| match v {
            Val::AInt(i) => *i as f64,
            Val::AFloat(f) => *f,
            _ => 0.0,
        };
        match (l, r) {
            (Val::AInt(a), Val::AInt(b)) => {
                let (a, b) = (*a, *b);
                let v = match op {
                    BinOp::Add => Val::AInt(a.wrapping_add(b)),
                    BinOp::Sub => Val::AInt(a.wrapping_sub(b)),
                    BinOp::Mul => Val::AInt(a.wrapping_mul(b)),
                    BinOp::Div | BinOp::Rem => {
                        if b == 0 {
                            return self.err(e, "integer division by zero in a constant");
                        }
                        Val::AInt(if op == BinOp::Div { a / b } else { a % b })
                    }
                    BinOp::BitAnd => Val::AInt(a & b),
                    BinOp::BitOr => Val::AInt(a | b),
                    BinOp::BitXor => Val::AInt(a ^ b),
                    BinOp::Shl => Val::AInt(a << (b & 63)),
                    BinOp::Shr => Val::AInt(a >> (b & 63)),
                    _ => {
                        let c = match op {
                            BinOp::Eq => a == b,
                            BinOp::Ne => a != b,
                            BinOp::Lt => a < b,
                            BinOp::Le => a <= b,
                            BinOp::Gt => a > b,
                            _ => a >= b,
                        };
                        Val::V(self.b.const_bool(c), Ty::Scalar(Scalar::Bool))
                    }
                };
                Ok(Some(v))
            }
            (Val::AInt(_) | Val::AFloat(_), Val::AInt(_) | Val::AFloat(_)) => {
                let (a, b) = (as_f(l), as_f(r));
                let v = match op {
                    BinOp::Add => Val::AFloat(a + b),
                    BinOp::Sub => Val::AFloat(a - b),
                    BinOp::Mul => Val::AFloat(a * b),
                    BinOp::Div => Val::AFloat(a / b),
                    BinOp::Rem => Val::AFloat(a % b),
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        let c = match op {
                            BinOp::Eq => a == b,
                            BinOp::Ne => a != b,
                            BinOp::Lt => a < b,
                            BinOp::Le => a <= b,
                            BinOp::Gt => a > b,
                            _ => a >= b,
                        };
                        Val::V(self.b.const_bool(c), Ty::Scalar(Scalar::Bool))
                    }
                    _ => return self.err(e, "bit operation on a float"),
                };
                Ok(Some(v))
            }
            _ => Ok(None),
        }
    }

    fn binary(&mut self, op: BinOp, l: Val, r: Val, e: ExprId) -> Result<Val, String> {
        if let Some(v) = self.fold(op, &l, &r, e)? {
            return Ok(v);
        }
        let l = self.load(l);
        let r = self.load(r);
        // An abstract operand takes the other's element type.
        let other_leaf = |v: &Val| match v {
            Val::V(_, t) => t.leaf(),
            _ => None,
        };
        let (lid, lty) = match &l {
            Val::AInt(_) | Val::AFloat(_) => {
                let pref = if matches!(op, BinOp::Shl | BinOp::Shr) { None } else { other_leaf(&r) };
                self.concrete(l.clone(), pref, e)?
            }
            _ => self.concrete(l.clone(), None, e)?,
        };
        let (rid, rty) = match &r {
            Val::AInt(_) | Val::AFloat(_) => {
                let pref = if matches!(op, BinOp::Shl | BinOp::Shr) { Some(Scalar::U32) } else { lty.leaf() };
                self.concrete(r.clone(), pref, e)?
            }
            _ => self.concrete(r.clone(), None, e)?,
        };
        let ls = match lty.leaf() {
            Some(s) => s,
            None => return self.err(e, "binary operator on a non-numeric value"),
        };
        let rs = rty.leaf().unwrap_or(ls);
        let bool_of = |t: &Ty| t.with_leaf(Scalar::Bool);
        match op {
            BinOp::LogicAnd | BinOp::LogicOr => {
                let o = if op == BinOp::LogicAnd { OP_LOGICAL_AND } else { OP_LOGICAL_OR };
                Ok(Val::V(self.ins(o, &lty, &[lid, rid]), lty))
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (lid, rid, ty) = self.match_shapes(lid, &lty, rid, &rty);
                let o = match (op, ls) {
                    (BinOp::Eq, Scalar::Bool) => OP_LOGICAL_EQUAL,
                    (BinOp::Ne, Scalar::Bool) => OP_LOGICAL_NOT_EQUAL,
                    (BinOp::Eq, s) if s.is_float() => OP_F_ORD_EQUAL,
                    (BinOp::Ne, s) if s.is_float() => OP_F_ORD_NOT_EQUAL,
                    (BinOp::Lt, s) if s.is_float() => OP_F_ORD_LESS_THAN,
                    (BinOp::Le, s) if s.is_float() => OP_F_ORD_LESS_THAN_EQUAL,
                    (BinOp::Gt, s) if s.is_float() => OP_F_ORD_GREATER_THAN,
                    (BinOp::Ge, s) if s.is_float() => OP_F_ORD_GREATER_THAN_EQUAL,
                    (BinOp::Eq, _) => OP_I_EQUAL,
                    (BinOp::Ne, _) => OP_I_NOT_EQUAL,
                    (BinOp::Lt, Scalar::I32) => OP_S_LESS_THAN,
                    (BinOp::Le, Scalar::I32) => OP_S_LESS_THAN_EQUAL,
                    (BinOp::Gt, Scalar::I32) => OP_S_GREATER_THAN,
                    (BinOp::Ge, Scalar::I32) => OP_S_GREATER_THAN_EQUAL,
                    (BinOp::Lt, _) => OP_U_LESS_THAN,
                    (BinOp::Le, _) => OP_U_LESS_THAN_EQUAL,
                    (BinOp::Gt, _) => OP_U_GREATER_THAN,
                    _ => OP_U_GREATER_THAN_EQUAL,
                };
                let bty = bool_of(&ty);
                Ok(Val::V(self.ins(o, &bty, &[lid, rid]), bty))
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (lid, rid, ty) = self.match_shapes(lid, &lty, rid, &rty);
                let o = match (op, ls) {
                    (BinOp::BitAnd, Scalar::Bool) => OP_LOGICAL_AND,
                    (BinOp::BitOr, Scalar::Bool) => OP_LOGICAL_OR,
                    (BinOp::BitXor, Scalar::Bool) => OP_LOGICAL_NOT_EQUAL,
                    (BinOp::BitAnd, _) => OP_BITWISE_AND,
                    (BinOp::BitOr, _) => OP_BITWISE_OR,
                    _ => OP_BITWISE_XOR,
                };
                Ok(Val::V(self.ins(o, &ty, &[lid, rid]), ty))
            }
            BinOp::Shl | BinOp::Shr => {
                let rid = if rty.components() != lty.components() {
                    self.splat_value(&lty.with_leaf(rs), rid)
                } else {
                    rid
                };
                let o = match (op, ls) {
                    (BinOp::Shl, _) => OP_SHIFT_LEFT_LOGICAL,
                    (_, Scalar::I32) => OP_SHIFT_RIGHT_ARITHMETIC,
                    _ => OP_SHIFT_RIGHT_LOGICAL,
                };
                Ok(Val::V(self.ins(o, &lty, &[lid, rid]), lty))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if let (BinOp::Mul, Some(v)) = (op, self.mul_special(lid, &lty, rid, &rty)) {
                    return Ok(v);
                }
                if matches!(lty, Ty::Matrix(..)) || matches!(rty, Ty::Matrix(..)) {
                    return self.matrix_componentwise(op, lid, &lty, rid, &rty, e);
                }
                let (lid, rid, ty) = self.match_shapes(lid, &lty, rid, &rty);
                let s = ty.leaf().unwrap();
                let id = match (op, s.is_float()) {
                    (BinOp::Add, true) => self.ins(OP_F_ADD, &ty, &[lid, rid]),
                    (BinOp::Add, false) => self.ins(OP_I_ADD, &ty, &[lid, rid]),
                    (BinOp::Sub, true) => self.ins(OP_F_SUB, &ty, &[lid, rid]),
                    (BinOp::Sub, false) => self.ins(OP_I_SUB, &ty, &[lid, rid]),
                    (BinOp::Mul, true) => self.ins(OP_F_MUL, &ty, &[lid, rid]),
                    (BinOp::Mul, false) => self.ins(OP_I_MUL, &ty, &[lid, rid]),
                    (BinOp::Div, true) => self.ins(OP_F_DIV, &ty, &[lid, rid]),
                    (BinOp::Rem, true) => self.ins(OP_F_REM, &ty, &[lid, rid]),
                    (_, false) => self.safe_int_div(op, lid, rid, &ty),
                    _ => unreachable!(),
                };
                Ok(Val::V(id, ty))
            }
        }
    }

    /// Integer `/` and `%` that never divide by zero or overflow: the
    /// divisor becomes 1 when it is 0, or when it is -1 and the dividend
    /// the type's minimum (naga back/spv/writer.rs `write_wrapped_binary_op`,
    /// inlined).
    fn safe_int_div(&mut self, op: BinOp, l: u32, r: u32, ty: &Ty) -> u32 {
        let s = ty.leaf().unwrap();
        let bty = ty.with_leaf(Scalar::Bool);
        let zero = self.const_splat(ty, 0.0);
        let one = self.const_splat(ty, 1.0);
        let r_zero = self.ins(OP_I_EQUAL, &bty, &[r, zero]);
        let bad = if s == Scalar::I32 {
            let min_c = self.b.const_i32(i32::MIN);
            let min = self.splat_const(ty, min_c);
            let neg_c = self.b.const_i32(-1);
            let neg = self.splat_const(ty, neg_c);
            let l_min = self.ins(OP_I_EQUAL, &bty, &[l, min]);
            let r_neg = self.ins(OP_I_EQUAL, &bty, &[r, neg]);
            let both = self.ins(OP_LOGICAL_AND, &bty, &[l_min, r_neg]);
            self.ins(OP_LOGICAL_OR, &bty, &[r_zero, both])
        } else {
            r_zero
        };
        let divisor = self.ins(OP_SELECT, ty, &[bad, one, r]);
        let o = match (op, s) {
            (BinOp::Div, Scalar::I32) => OP_S_DIV,
            (BinOp::Div, _) => OP_U_DIV,
            (_, Scalar::I32) => OP_S_REM,
            _ => OP_U_MOD,
        };
        self.ins(o, ty, &[l, divisor])
    }

    /// Matrix and vector-times-scalar products.
    fn mul_special(&mut self, lid: u32, lty: &Ty, rid: u32, rty: &Ty) -> Option<Val> {
        match (lty, rty) {
            (Ty::Matrix(c, r), Ty::Vector(n, _)) if n == c => {
                let ty = Ty::Vector(*r, Scalar::F32);
                Some(Val::V(self.ins(OP_MATRIX_TIMES_VECTOR, &ty, &[lid, rid]), ty))
            }
            (Ty::Vector(n, _), Ty::Matrix(c, r)) if n == r => {
                let ty = Ty::Vector(*c, Scalar::F32);
                Some(Val::V(self.ins(OP_VECTOR_TIMES_MATRIX, &ty, &[lid, rid]), ty))
            }
            (Ty::Matrix(c, r), Ty::Matrix(c2, r2)) if c == r2 => {
                let ty = Ty::Matrix(*c2, *r);
                Some(Val::V(self.ins(OP_MATRIX_TIMES_MATRIX, &ty, &[lid, rid]), ty))
            }
            (Ty::Matrix(..), Ty::Scalar(Scalar::F32)) => {
                Some(Val::V(self.ins(OP_MATRIX_TIMES_SCALAR, lty, &[lid, rid]), lty.clone()))
            }
            (Ty::Scalar(Scalar::F32), Ty::Matrix(..)) => {
                Some(Val::V(self.ins(OP_MATRIX_TIMES_SCALAR, rty, &[rid, lid]), rty.clone()))
            }
            (Ty::Vector(_, s), Ty::Scalar(s2)) if s.is_float() && s == s2 => {
                Some(Val::V(self.ins(OP_VECTOR_TIMES_SCALAR, lty, &[lid, rid]), lty.clone()))
            }
            (Ty::Scalar(s2), Ty::Vector(_, s)) if s.is_float() && s == s2 => {
                Some(Val::V(self.ins(OP_VECTOR_TIMES_SCALAR, rty, &[rid, lid]), rty.clone()))
            }
            _ => None,
        }
    }

    /// `+` / `-` of two matrices, column by column.
    fn matrix_componentwise(&mut self, op: BinOp, lid: u32, lty: &Ty, rid: u32, rty: &Ty, e: ExprId) -> Result<Val, String> {
        let (c, r) = match (lty, rty) {
            (Ty::Matrix(c, r), Ty::Matrix(c2, r2)) if c == c2 && r == r2 && matches!(op, BinOp::Add | BinOp::Sub) => (*c, *r),
            _ => return self.err(e, "unsupported matrix operation"),
        };
        let col_ty = Ty::Vector(r, Scalar::F32);
        let mut cols = Vec::new();
        for k in 0..c {
            let a = self.ins(OP_COMPOSITE_EXTRACT, &col_ty, &[lid, k]);
            let b = self.ins(OP_COMPOSITE_EXTRACT, &col_ty, &[rid, k]);
            let o = if op == BinOp::Add { OP_F_ADD } else { OP_F_SUB };
            cols.push(self.ins(o, &col_ty, &[a, b]));
        }
        Ok(Val::V(self.ins(OP_COMPOSITE_CONSTRUCT, lty, &cols), lty.clone()))
    }

    /// Splats a scalar operand against a vector one.
    fn match_shapes(&mut self, lid: u32, lty: &Ty, rid: u32, rty: &Ty) -> (u32, u32, Ty) {
        match (lty, rty) {
            (Ty::Vector(..), Ty::Scalar(_)) => {
                let r = self.splat_value(lty, rid);
                (lid, r, lty.clone())
            }
            (Ty::Scalar(_), Ty::Vector(..)) => {
                let l = self.splat_value(rty, lid);
                (l, rid, rty.clone())
            }
            _ => (lid, rid, lty.clone()),
        }
    }

    fn member(&mut self, v: Val, name: &str, e: ExprId) -> Result<Val, String> {
        match v {
            Val::Fields(fields) => {
                for (n, fv) in fields {
                    if n == name {
                        return Ok(fv);
                    }
                }
                self.err(e, &format!("no input named {}", name))
            }
            Val::R(ptr, Ty::Struct(si), space) => {
                let k = self.member_index(si, name, e)?;
                let mty = self.structs[si].members[k].1.clone();
                let idx = self.b.const_u32(k as u32);
                let pty = Ty::Ptr(Box::new(mty.clone()), space);
                let p = self.ins(OP_ACCESS_CHAIN, &pty, &[ptr, idx]);
                Ok(Val::R(p, mty, space))
            }
            Val::V(id, Ty::Struct(si)) => {
                let k = self.member_index(si, name, e)?;
                let mty = self.structs[si].members[k].1.clone();
                Ok(Val::V(self.ins(OP_COMPOSITE_EXTRACT, &mty, &[id, k as u32]), mty))
            }
            Val::R(ptr, Ty::Vector(n, s), space) => {
                let comps = self.swizzle(name, n, e)?;
                if comps.len() == 1 {
                    let idx = self.b.const_u32(comps[0]);
                    let pty = Ty::Ptr(Box::new(Ty::Scalar(s)), space);
                    let p = self.ins(OP_ACCESS_CHAIN, &pty, &[ptr, idx]);
                    Ok(Val::R(p, Ty::Scalar(s), space))
                } else {
                    let vty = Ty::Vector(n, s);
                    let id = self.ins(OP_LOAD, &vty, &[ptr]);
                    self.shuffle(id, &comps, s)
                }
            }
            Val::V(id, Ty::Vector(n, s)) => {
                let comps = self.swizzle(name, n, e)?;
                if comps.len() == 1 {
                    Ok(Val::V(self.ins(OP_COMPOSITE_EXTRACT, &Ty::Scalar(s), &[id, comps[0]]), Ty::Scalar(s)))
                } else {
                    self.shuffle(id, &comps, s)
                }
            }
            Val::AInt(_) | Val::AFloat(_) => {
                let (id, ty) = self.concrete(v, None, e)?;
                self.member(Val::V(id, ty), name, e)
            }
            _ => self.err(e, &format!("no member {} here", name)),
        }
    }

    fn member_index(&self, si: usize, name: &str, e: ExprId) -> Result<usize, String> {
        for (k, m) in self.structs[si].members.iter().enumerate() {
            if m.0 == name {
                return Ok(k);
            }
        }
        self.err(e, &format!("struct {} has no member {}", self.structs[si].name, name))
    }

    fn swizzle(&self, name: &str, n: u32, e: ExprId) -> Result<Vec<u32>, String> {
        let mut out = Vec::new();
        for c in name.bytes() {
            match swizzle_index(c) {
                Some(i) if i < n => out.push(i),
                _ => return self.err(e, &format!("bad swizzle .{}", name)),
            }
        }
        if out.is_empty() || out.len() > 4 {
            return self.err(e, &format!("bad swizzle .{}", name));
        }
        Ok(out)
    }

    fn shuffle(&mut self, id: u32, comps: &[u32], s: Scalar) -> Result<Val, String> {
        let ty = Ty::Vector(comps.len() as u32, s);
        let mut args = vec![id, id];
        args.extend_from_slice(comps);
        Ok(Val::V(self.ins(OP_VECTOR_SHUFFLE, &ty, &args), ty))
    }

    fn elem_ty(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Array(e, _) => Some((**e).clone()),
            Ty::Vector(_, s) => Some(Ty::Scalar(*s)),
            Ty::Matrix(_, r) => Some(Ty::Vector(*r, Scalar::F32)),
            _ => None,
        }
    }

    fn index(&mut self, v: Val, index: ExprId, e: ExprId) -> Result<Val, String> {
        let iv = self.expr(index)?;
        let literal = match iv {
            Val::AInt(i) => Some(i),
            _ => None,
        };
        match v {
            Val::R(ptr, ty, space) => {
                let elem = match self.elem_ty(&ty) {
                    Some(t) => t,
                    None => return self.err(e, "indexing a value that is not an array, vector or matrix"),
                };
                let idx = match literal {
                    Some(i) => self.b.const_u32(i as u32),
                    None => self.concrete(iv, Some(Scalar::U32), index)?.0,
                };
                let pty = Ty::Ptr(Box::new(elem.clone()), space);
                let p = self.ins(OP_ACCESS_CHAIN, &pty, &[ptr, idx]);
                Ok(Val::R(p, elem, space))
            }
            Val::V(id, ty) => {
                let elem = match self.elem_ty(&ty) {
                    Some(t) => t,
                    None => return self.err(e, "indexing a value that is not an array, vector or matrix"),
                };
                if let Some(i) = literal {
                    return Ok(Val::V(self.ins(OP_COMPOSITE_EXTRACT, &elem, &[id, i as u32]), elem));
                }
                let (idx, _) = self.concrete(iv, Some(Scalar::U32), index)?;
                if let Ty::Vector(..) = ty {
                    return Ok(Val::V(self.ins(OP_VECTOR_EXTRACT_DYNAMIC, &elem, &[id, idx]), elem));
                }
                // A dynamically indexed array or matrix value goes through
                // a local copy.
                let var = self.local_var(&ty);
                self.emit(OP_STORE, &[var, id]);
                let pty = Ty::Ptr(Box::new(elem.clone()), Space::Function);
                let p = self.ins(OP_ACCESS_CHAIN, &pty, &[var, idx]);
                Ok(Val::V(self.ins(OP_LOAD, &elem, &[p]), elem))
            }
            _ => self.err(e, "indexing a value that is not an array, vector or matrix"),
        }
    }

    // ---- calls ----

    fn call(&mut self, name: &str, templ: &[TypeAst], args: &[ExprId], e: ExprId) -> Result<Val, String> {
        if let Some(fi) = self.fns.get(name).copied() {
            return self.user_call(fi, args, e);
        }
        if name == "bitcast" {
            let to = match templ.first() {
                Some(t) => self.resolve(t)?,
                None => return self.err(e, "bitcast without a target type"),
            };
            let v = self.arg(args, 0, e)?;
            let (id, ty) = self.concrete(v, to.leaf(), e)?;
            if ty == to {
                return Ok(Val::V(id, to));
            }
            return Ok(Val::V(self.ins(OP_BITCAST, &to, &[id]), to));
        }
        // Constructors.
        if name == "array" && !templ.is_empty() {
            let ty = self.resolve(&TypeAst { name: "array".to_string(), args: templ.to_vec() })?;
            return self.construct(ty, false, false, args, e);
        }
        let mut targs = Vec::new();
        for t in templ {
            targs.push(self.resolve(t)?);
        }
        let ctor = if name == "array" {
            Some(Ty::Array(Box::new(Ty::Void), 0))
        } else if let Some(si) = self.struct_by_name(name) {
            Some(Ty::Struct(si))
        } else {
            builtin_type_by_name(name, &targs).or_else(|| match name {
                "vec2" => Some(Ty::Vector(2, Scalar::Bool)),
                "vec3" => Some(Ty::Vector(3, Scalar::Bool)),
                "vec4" => Some(Ty::Vector(4, Scalar::Bool)),
                _ => None,
            })
        };
        if let Some(ty) = ctor {
            // `vecN(..)` without a template: Bool stands for "infer".
            let infer = templ.is_empty() && matches!(name, "vec2" | "vec3" | "vec4");
            return self.construct(ty, infer, name == "array" && templ.is_empty(), args, e);
        }
        self.builtin(name, args, e)
    }

    fn struct_by_name(&self, name: &str) -> Option<usize> {
        self.struct_names.get(name).copied()
    }

    fn arg(&mut self, args: &[ExprId], k: usize, e: ExprId) -> Result<Val, String> {
        match args.get(k) {
            Some(a) => self.expr(*a),
            None => self.err(e, "missing argument"),
        }
    }

    fn user_call(&mut self, fi: usize, args: &[ExprId], e: ExprId) -> Result<Val, String> {
        let m = self.m;
        let f = &m.fns[fi];
        if f.params.len() != args.len() {
            return self.err(e, &format!("{} takes {} arguments", f.name, f.params.len()));
        }
        let mut ids = Vec::new();
        for (k, p) in f.params.iter().enumerate() {
            let pty = self.resolve(&p.ty)?;
            let v = self.expr(args[k])?;
            let id = match (&pty, v) {
                (Ty::Ptr(..), Val::V(id, Ty::Ptr(..))) => id,
                (Ty::Ptr(..), _) => return self.err(args[k], "a pointer argument must be &variable"),
                (_, v) => self.coerce(v, &pty, args[k])?,
            };
            ids.push(id);
        }
        let ret = match &f.ret {
            Some(t) => self.resolve(t)?,
            None => Ty::Void,
        };
        let fid = self.fn_id(fi);
        let mut words = vec![fid];
        words.extend_from_slice(&ids);
        let id = self.ins(OP_FUNCTION_CALL, &ret, &words);
        Ok(Val::V(id, ret))
    }

    fn construct(&mut self, ty: Ty, infer: bool, infer_array: bool, args: &[ExprId], e: ExprId) -> Result<Val, String> {
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.expr(*a)?);
        }
        // Inferred element type: the first concrete argument's, else the
        // abstract literals' default.
        let mut ty = ty;
        if infer || infer_array {
            let mut leaf = None;
            let mut first_ty = None;
            let mut any_float = false;
            for v in &vals {
                match v {
                    Val::V(_, t) | Val::R(_, t, _) => {
                        if leaf.is_none() {
                            leaf = t.leaf();
                            first_ty = Some(t.clone());
                        }
                    }
                    Val::AFloat(_) => any_float = true,
                    _ => {}
                }
            }
            if infer_array {
                let elem = match first_ty {
                    Some(t) => t,
                    None => Ty::Scalar(if any_float { Scalar::F32 } else { Scalar::I32 }),
                };
                ty = Ty::Array(Box::new(elem), vals.len() as u32);
            } else {
                let s = leaf.unwrap_or(if any_float { Scalar::F32 } else { Scalar::I32 });
                ty = ty.with_leaf(s);
            }
        }
        if let Ty::Array(elem, 0) = &ty {
            ty = Ty::Array(elem.clone(), vals.len() as u32);
        }
        if vals.is_empty() {
            let tid = self.ty_id(&ty);
            return Ok(Val::V(self.b.const_null(tid), ty));
        }
        match ty.clone() {
            Ty::Scalar(_) => {
                let v = vals.pop().unwrap();
                let id = self.coerce(v, &ty, e)?;
                Ok(Val::V(id, ty))
            }
            Ty::Vector(n, s) => {
                if vals.len() == 1 {
                    let v = vals.pop().unwrap();
                    return match v {
                        Val::AInt(_) | Val::AFloat(_) => Ok(Val::V(self.coerce(v, &ty, e)?, ty)),
                        v => {
                            let (id, vty) = self.concrete(v, Some(s), e)?;
                            match vty {
                                Ty::Vector(..) => Ok(Val::V(self.convert(id, &vty, &ty, e)?, ty)),
                                _ => {
                                    let c = self.convert(id, &vty, &Ty::Scalar(s), e)?;
                                    Ok(Val::V(self.splat_value(&ty, c), ty))
                                }
                            }
                        }
                    };
                }
                let mut parts = Vec::new();
                let mut count = 0;
                for v in vals {
                    match v {
                        Val::AInt(_) | Val::AFloat(_) => {
                            parts.push(self.coerce(v, &Ty::Scalar(s), e)?);
                            count += 1;
                        }
                        v => {
                            let (id, vty) = self.concrete(v, Some(s), e)?;
                            let target = vty.with_leaf(s);
                            count += vty.components();
                            parts.push(self.convert(id, &vty, &target, e)?);
                        }
                    }
                }
                if count != n {
                    return self.err(e, &format!("vec{} constructor with {} components", n, count));
                }
                Ok(Val::V(self.ins(OP_COMPOSITE_CONSTRUCT, &ty, &parts), ty))
            }
            Ty::Matrix(c, r) => {
                let col_ty = Ty::Vector(r, Scalar::F32);
                if vals.len() == 1 {
                    let v = vals.pop().unwrap();
                    let (id, vty) = self.concrete(v, Some(Scalar::F32), e)?;
                    if vty == ty {
                        return Ok(Val::V(id, ty));
                    }
                    return self.err(e, "unsupported matrix conversion");
                }
                let mut cols = Vec::new();
                if vals.len() as u32 == c {
                    for v in vals {
                        cols.push(self.coerce(v, &col_ty, e)?);
                    }
                } else if vals.len() as u32 == c * r {
                    let mut scalars = Vec::new();
                    for v in vals {
                        scalars.push(self.coerce(v, &Ty::Scalar(Scalar::F32), e)?);
                    }
                    for k in 0..c {
                        let part = &scalars[(k * r) as usize..((k + 1) * r) as usize];
                        cols.push(self.ins(OP_COMPOSITE_CONSTRUCT, &col_ty, part));
                    }
                } else {
                    return self.err(e, "matrix constructor with the wrong number of arguments");
                }
                Ok(Val::V(self.ins(OP_COMPOSITE_CONSTRUCT, &ty, &cols), ty))
            }
            Ty::Struct(si) => {
                let n = self.structs[si].members.len();
                if vals.len() != n {
                    return self.err(e, &format!("{} takes {} members", self.structs[si].name, n));
                }
                let mut parts = Vec::new();
                for (k, v) in vals.into_iter().enumerate() {
                    let mty = self.structs[si].members[k].1.clone();
                    parts.push(self.coerce(v, &mty, e)?);
                }
                Ok(Val::V(self.ins(OP_COMPOSITE_CONSTRUCT, &ty, &parts), ty))
            }
            Ty::Array(elem, _) => {
                let mut parts = Vec::new();
                for v in vals {
                    parts.push(self.coerce(v, &elem, e)?);
                }
                Ok(Val::V(self.ins(OP_COMPOSITE_CONSTRUCT, &ty, &parts), ty))
            }
            _ => self.err(e, "cannot construct this type"),
        }
    }

    /// Concrete arguments, abstract ones taking the first concrete
    /// argument's element type.
    fn concrete_args(&mut self, args: &[ExprId], e: ExprId) -> Result<Vec<(u32, Ty)>, String> {
        let mut vals = Vec::new();
        for a in args {
            let v = self.expr(*a)?;
            vals.push(self.load(v));
        }
        let mut leaf = None;
        let mut any_float = false;
        for v in &vals {
            match v {
                Val::V(_, t) => {
                    if leaf.is_none() {
                        leaf = t.leaf();
                    }
                }
                Val::AFloat(_) => any_float = true,
                _ => {}
            }
        }
        let leaf = leaf.unwrap_or(if any_float { Scalar::F32 } else { Scalar::I32 });
        let mut out = Vec::new();
        for v in vals {
            out.push(self.concrete(v, Some(leaf), e)?);
        }
        Ok(out)
    }

    /// Arguments of a component-wise builtin: abstract and scalar arguments
    /// splatted to the shape of the widest one.
    fn uniform_args(&mut self, args: &[ExprId], e: ExprId) -> Result<(Vec<u32>, Ty), String> {
        let cs = self.concrete_args(args, e)?;
        let mut ty = Ty::Void;
        for (_, t) in &cs {
            if ty == Ty::Void || t.components() > ty.components() {
                ty = t.clone();
            }
        }
        let mut ids = Vec::new();
        for (id, t) in cs {
            if t.components() != ty.components() {
                ids.push(self.splat_value(&ty, id));
            } else {
                ids.push(id);
            }
        }
        Ok((ids, ty))
    }

    fn builtin(&mut self, name: &str, args: &[ExprId], e: ExprId) -> Result<Val, String> {
        let f32_ty = Ty::Scalar(Scalar::F32);
        // GLSL.std.450 functions whose overload is the argument type.
        let float_ext = match name {
            "sin" => Some(GL_SIN),
            "cos" => Some(GL_COS),
            "tan" => Some(GL_TAN),
            "asin" => Some(GL_ASIN),
            "acos" => Some(GL_ACOS),
            "atan" => Some(GL_ATAN),
            "sinh" => Some(GL_SINH),
            "cosh" => Some(GL_COSH),
            "tanh" => Some(GL_TANH),
            "asinh" => Some(GL_ASINH),
            "acosh" => Some(GL_ACOSH),
            "atanh" => Some(GL_ATANH),
            "atan2" => Some(GL_ATAN2),
            "radians" => Some(GL_RADIANS),
            "degrees" => Some(GL_DEGREES),
            "exp" => Some(GL_EXP),
            "exp2" => Some(GL_EXP2),
            "log" => Some(GL_LOG),
            "log2" => Some(GL_LOG2),
            "pow" => Some(GL_POW),
            "sqrt" => Some(GL_SQRT),
            "inverseSqrt" => Some(GL_INVERSE_SQRT),
            "floor" => Some(GL_FLOOR),
            "ceil" => Some(GL_CEIL),
            "fract" => Some(GL_FRACT),
            "trunc" => Some(GL_TRUNC),
            "round" => Some(GL_ROUND_EVEN),
            "fma" => Some(GL_FMA),
            "step" => Some(GL_STEP),
            "smoothstep" => Some(GL_SMOOTH_STEP),
            "normalize" => Some(GL_NORMALIZE),
            "cross" => Some(GL_CROSS),
            "reflect" => Some(GL_REFLECT),
            "faceForward" => Some(GL_FACE_FORWARD),
            _ => None,
        };
        if let Some(gl) = float_ext {
            let (ids, ty) = self.float_args(args, e)?;
            return Ok(Val::V(self.ext(gl, &ty, &ids), ty));
        }
        match name {
            "abs" => {
                let (ids, ty) = self.uniform_args(args, e)?;
                let id = match ty.leaf() {
                    Some(Scalar::I32) => self.ext(GL_S_ABS, &ty, &ids),
                    // naga back/spv/block.rs: abs of an unsigned is a copy.
                    Some(Scalar::U32) => self.ins(OP_COPY_OBJECT, &ty, &ids),
                    _ => self.ext(GL_F_ABS, &ty, &ids),
                };
                Ok(Val::V(id, ty))
            }
            "min" | "max" => {
                let (ids, ty) = self.uniform_args(args, e)?;
                let gl = match (name, ty.leaf()) {
                    ("min", Some(Scalar::I32)) => GL_S_MIN,
                    ("min", Some(Scalar::U32)) => GL_U_MIN,
                    ("min", _) => GL_F_MIN,
                    (_, Some(Scalar::I32)) => GL_S_MAX,
                    (_, Some(Scalar::U32)) => GL_U_MAX,
                    _ => GL_F_MAX,
                };
                Ok(Val::V(self.ext(gl, &ty, &ids), ty))
            }
            "clamp" => {
                let (ids, ty) = self.uniform_args(args, e)?;
                if ids.len() != 3 {
                    return self.err(e, "clamp takes 3 arguments");
                }
                // naga back/spv/block.rs: integer clamp as max then min,
                // which WGSL defines for low > high too.
                let id = match ty.leaf() {
                    Some(Scalar::I32) => {
                        let t = self.ext(GL_S_MAX, &ty, &[ids[0], ids[1]]);
                        self.ext(GL_S_MIN, &ty, &[t, ids[2]])
                    }
                    Some(Scalar::U32) => {
                        let t = self.ext(GL_U_MAX, &ty, &[ids[0], ids[1]]);
                        self.ext(GL_U_MIN, &ty, &[t, ids[2]])
                    }
                    _ => self.ext(GL_F_CLAMP, &ty, &ids),
                };
                Ok(Val::V(id, ty))
            }
            "saturate" => {
                let (ids, ty) = self.float_args(args, e)?;
                let zero = self.const_splat(&ty, 0.0);
                let one = self.const_splat(&ty, 1.0);
                Ok(Val::V(self.ext(GL_F_CLAMP, &ty, &[ids[0], zero, one]), ty))
            }
            "sign" => {
                let (ids, ty) = self.uniform_args(args, e)?;
                let gl = if ty.leaf() == Some(Scalar::I32) { GL_S_SIGN } else { GL_F_SIGN };
                Ok(Val::V(self.ext(gl, &ty, &ids), ty))
            }
            "mix" => {
                // naga back/spv/block.rs: a scalar selector is splatted.
                let (ids, ty) = self.uniform_args(args, e)?;
                Ok(Val::V(self.ext(GL_F_MIX, &ty, &ids), ty))
            }
            "length" | "distance" => {
                let (ids, _) = self.float_args(args, e)?;
                let gl = if name == "length" { GL_LENGTH } else { GL_DISTANCE };
                Ok(Val::V(self.ext(gl, &f32_ty, &ids), f32_ty))
            }
            "refract" => {
                let cs = self.concrete_args(args, e)?;
                let ty = cs[0].1.clone();
                let ids = [cs[0].0, cs[1].0, cs[2].0];
                Ok(Val::V(self.ext(GL_REFRACT, &ty, &ids), ty))
            }
            "dot" => {
                let (ids, ty) = self.uniform_args(args, e)?;
                let s = ty.leaf().unwrap_or(Scalar::F32);
                if s.is_float() {
                    let rty = Ty::Scalar(s);
                    return Ok(Val::V(self.ins(OP_DOT, &rty, &ids), rty));
                }
                // Integer dot product: multiply and sum the components.
                let prod = self.ins(OP_I_MUL, &ty, &ids);
                let rty = Ty::Scalar(s);
                let mut sum = self.ins(OP_COMPOSITE_EXTRACT, &rty, &[prod, 0]);
                for k in 1..ty.components() {
                    let c = self.ins(OP_COMPOSITE_EXTRACT, &rty, &[prod, k]);
                    sum = self.ins(OP_I_ADD, &rty, &[sum, c]);
                }
                Ok(Val::V(sum, rty))
            }
            "ldexp" => {
                let cs = self.concrete_args(args, e)?;
                let ty = cs[0].1.clone();
                let ids = [cs[0].0, cs[1].0];
                Ok(Val::V(self.ext(GL_LDEXP, &ty, &ids), ty))
            }
            "determinant" => {
                let cs = self.concrete_args(args, e)?;
                Ok(Val::V(self.ext(GL_DETERMINANT, &f32_ty, &[cs[0].0]), f32_ty))
            }
            "inverse" => {
                let cs = self.concrete_args(args, e)?;
                let ty = cs[0].1.clone();
                Ok(Val::V(self.ext(GL_MATRIX_INVERSE, &ty, &[cs[0].0]), ty))
            }
            "transpose" => {
                let cs = self.concrete_args(args, e)?;
                let ty = match cs[0].1 {
                    Ty::Matrix(c, r) => Ty::Matrix(r, c),
                    _ => return self.err(e, "transpose of a non-matrix"),
                };
                Ok(Val::V(self.ins(OP_TRANSPOSE, &ty, &[cs[0].0]), ty))
            }
            "select" => {
                if args.len() != 3 {
                    return self.err(e, "select takes 3 arguments");
                }
                let cs = self.concrete_args(&args[..2], e)?;
                let ty = cs[0].1.clone();
                let f = self.convert(cs[0].0, &cs[0].1, &ty, e)?;
                let t = self.convert(cs[1].0, &cs[1].1, &ty, e)?;
                let cv = self.expr(args[2])?;
                let (c, cty) = self.concrete(cv, Some(Scalar::Bool), args[2])?;
                let c = if cty.components() != ty.components() {
                    self.splat_value(&ty.with_leaf(Scalar::Bool), c)
                } else {
                    c
                };
                Ok(Val::V(self.ins(OP_SELECT, &ty, &[c, t, f]), ty))
            }
            "all" | "any" => {
                let cs = self.concrete_args(args, e)?;
                let bty = Ty::Scalar(Scalar::Bool);
                if let Ty::Vector(..) = cs[0].1 {
                    let o = if name == "all" { OP_ALL } else { OP_ANY };
                    return Ok(Val::V(self.ins(o, &bty, &[cs[0].0]), bty));
                }
                Ok(Val::V(cs[0].0, bty))
            }
            "dpdx" | "dpdy" | "fwidth" | "dpdxFine" | "dpdyFine" | "fwidthFine" | "dpdxCoarse" | "dpdyCoarse"
            | "fwidthCoarse" => {
                let (ids, ty) = self.float_args(args, e)?;
                let o = match name {
                    "dpdx" => OP_DPDX,
                    "dpdy" => OP_DPDY,
                    "fwidth" => OP_FWIDTH,
                    "dpdxFine" => OP_DPDX_FINE,
                    "dpdyFine" => OP_DPDY_FINE,
                    "fwidthFine" => OP_FWIDTH_FINE,
                    "dpdxCoarse" => OP_DPDX_COARSE,
                    "dpdyCoarse" => OP_DPDY_COARSE,
                    _ => OP_FWIDTH_COARSE,
                };
                if name.ends_with("Fine") || name.ends_with("Coarse") {
                    self.b.capability(CAP_DERIVATIVE_CONTROL);
                }
                Ok(Val::V(self.ins(o, &ty, &ids), ty))
            }
            "countOneBits" | "reverseBits" => {
                let cs = self.concrete_args(args, e)?;
                let ty = cs[0].1.clone();
                let o = if name == "countOneBits" { OP_BIT_COUNT } else { OP_BIT_REVERSE };
                Ok(Val::V(self.ins(o, &ty, &[cs[0].0]), ty))
            }
            "firstLeadingBit" | "firstTrailingBit" => {
                let cs = self.concrete_args(args, e)?;
                let ty = cs[0].1.clone();
                let gl = match (name, ty.leaf()) {
                    ("firstTrailingBit", _) => GL_FIND_I_LSB,
                    (_, Some(Scalar::I32)) => GL_FIND_S_MSB,
                    _ => GL_FIND_U_MSB,
                };
                Ok(Val::V(self.ext(gl, &ty, &[cs[0].0]), ty))
            }
            "pack4x8unorm" | "pack4x8snorm" | "pack2x16float" | "pack2x16unorm" | "pack2x16snorm" => {
                let cs = self.concrete_args(args, e)?;
                let gl = match name {
                    "pack4x8unorm" => GL_PACK_UNORM_4X8,
                    "pack4x8snorm" => GL_PACK_SNORM_4X8,
                    "pack2x16float" => GL_PACK_HALF_2X16,
                    "pack2x16unorm" => GL_PACK_UNORM_2X16,
                    _ => GL_PACK_SNORM_2X16,
                };
                let u = Ty::Scalar(Scalar::U32);
                Ok(Val::V(self.ext(gl, &u, &[cs[0].0]), u))
            }
            "unpack4x8unorm" | "unpack4x8snorm" | "unpack2x16float" | "unpack2x16unorm" | "unpack2x16snorm" => {
                let v = self.arg(args, 0, e)?;
                let id = self.coerce(v, &Ty::Scalar(Scalar::U32), e)?;
                let (gl, n) = match name {
                    "unpack4x8unorm" => (GL_UNPACK_UNORM_4X8, 4),
                    "unpack4x8snorm" => (GL_UNPACK_SNORM_4X8, 4),
                    "unpack2x16float" => (GL_UNPACK_HALF_2X16, 2),
                    "unpack2x16unorm" => (GL_UNPACK_UNORM_2X16, 2),
                    _ => (GL_UNPACK_SNORM_2X16, 2),
                };
                let ty = Ty::Vector(n, Scalar::F32);
                Ok(Val::V(self.ext(gl, &ty, &[id]), ty))
            }
            "arrayLength" => {
                // `arrayLength(&buffer)` of a storage global: member 0 of
                // its Block wrapper.
                let m = self.m;
                let inner = match args.first().map(|a| &m.exprs[*a as usize]) {
                    Some(Expr::AddrOf(i)) => *i,
                    _ => return self.err(e, "arrayLength takes &storage_buffer"),
                };
                let gi = match &m.exprs[inner as usize] {
                    Expr::Ident(n) => match self.globals.get(n.as_str()) {
                        Some(gi) => *gi,
                        None => return self.err(e, "arrayLength takes &storage_buffer"),
                    },
                    _ => return self.err(e, "arrayLength takes &storage_buffer"),
                };
                self.global(gi, e)?;
                let var = self.global_ids[gi];
                let u = Ty::Scalar(Scalar::U32);
                Ok(Val::V(self.ins(OP_ARRAY_LENGTH, &u, &[var, 0]), u))
            }
            "textureSample" | "textureSampleLevel" | "textureSampleBias" | "textureSampleGrad"
            | "textureSampleCompare" | "textureSampleCompareLevel" | "textureGather" => self.texture_sample(name, args, e),
            "textureLoad" => self.texture_load(args, e),
            "textureDimensions" | "textureNumLevels" | "textureNumLayers" => self.texture_query(name, args, e),
            _ => self.err(e, &format!("unsupported function {}", name)),
        }
    }

    /// Float arguments of one shape (vector arguments set it, scalars and
    /// abstract numbers are splatted).
    fn float_args(&mut self, args: &[ExprId], e: ExprId) -> Result<(Vec<u32>, Ty), String> {
        let mut vals = Vec::new();
        for a in args {
            let v = self.expr(*a)?;
            vals.push(self.load(v));
        }
        let mut ty = Ty::Scalar(Scalar::F32);
        for v in &vals {
            if let Val::V(_, t) = v {
                if t.components() > ty.components() || (ty == Ty::Scalar(Scalar::F32) && matches!(t, Ty::Scalar(Scalar::F16))) {
                    ty = t.clone();
                }
            }
        }
        let mut ids = Vec::new();
        for v in vals {
            let id = match v {
                Val::AInt(_) | Val::AFloat(_) => self.coerce(v, &ty, e)?,
                v => {
                    let (id, vt) = self.concrete(v, ty.leaf(), e)?;
                    if vt.components() != ty.components() {
                        let target = Ty::Scalar(ty.leaf().unwrap());
                        let c = self.convert(id, &vt, &target, e)?;
                        self.splat_value(&ty, c)
                    } else {
                        self.convert(id, &vt, &ty, e)?
                    }
                }
            };
            ids.push(id);
        }
        Ok((ids, ty))
    }

    /// The texture (loaded image) and its type from a texture argument.
    fn texture_arg(&mut self, args: &[ExprId], k: usize, e: ExprId) -> Result<(u32, TexDim, bool, Scalar, Ty), String> {
        let v = self.arg(args, k, e)?;
        match self.load(v) {
            Val::V(id, ty @ Ty::Texture(dim, depth, s)) => Ok((id, dim, depth, s, ty)),
            _ => self.err(e, "expected a texture"),
        }
    }

    /// Coordinates with the array layer appended as the last component.
    fn coords_with_layer(&mut self, coords: (u32, Ty), layer: Option<Val>, e: ExprId) -> Result<u32, String> {
        let Some(layer) = layer else { return Ok(coords.0) };
        let s = coords.1.leaf().unwrap_or(Scalar::F32);
        let (lid, lty) = self.concrete(layer, Some(Scalar::I32), e)?;
        let lid = self.convert(lid, &lty, &Ty::Scalar(s), e)?;
        let ty = Ty::Vector(coords.1.components() + 1, s);
        Ok(self.ins(OP_COMPOSITE_CONSTRUCT, &ty, &[coords.0, lid]))
    }

    fn texture_sample(&mut self, name: &str, args: &[ExprId], e: ExprId) -> Result<Val, String> {
        let gather = name == "textureGather";
        let mut k = 0;
        let mut component = 0u32;
        if gather {
            // `textureGather(component, t, s, coords)` for color textures.
            let m = self.m;
            if let Some(Expr::Int(c, _)) = args.first().map(|a| &m.exprs[*a as usize]) {
                let next_is_tex = matches!(args.get(1).map(|a| &m.exprs[*a as usize]), Some(Expr::Ident(_)));
                if next_is_tex {
                    component = *c as u32;
                    k = 1;
                }
            }
        }
        let (img, dim, depth, s, img_ty) = self.texture_arg(args, k, e)?;
        let sv = self.arg(args, k + 1, e)?;
        let smp = match self.load(sv) {
            Val::V(id, Ty::Sampler(_)) => id,
            _ => return self.err(e, "expected a sampler"),
        };
        let img_tid = self.ty_id(&img_ty);
        let si_ty = self.b.type_sampled_image(img_tid);
        let sampled = self.b.id();
        self.emit(OP_SAMPLED_IMAGE, &[si_ty, sampled, img, smp]);
        let ncoord = match dim {
            TexDim::D2 | TexDim::D2Array => 2,
            _ => 3,
        };
        let cv = self.arg(args, k + 2, e)?;
        let cty = Ty::Vector(ncoord, Scalar::F32);
        let cid = self.coerce(cv, &cty, args[k + 2])?;
        let mut next = k + 3;
        let layer = if matches!(dim, TexDim::D2Array | TexDim::CubeArray) {
            next += 1;
            Some(self.arg(args, k + 3, e)?)
        } else {
            None
        };
        let coords = self.coords_with_layer((cid, cty), layer, e)?;
        let out4 = Ty::Vector(4, s);
        let f32_ty = Ty::Scalar(Scalar::F32);
        let result = match name {
            "textureSample" => self.ins(OP_IMAGE_SAMPLE_IMPLICIT_LOD, &out4, &[sampled, coords]),
            "textureSampleLevel" => {
                let lv = self.arg(args, next, e)?;
                let (lid, lty) = self.concrete(lv, Some(Scalar::F32), e)?;
                let lod = self.convert(lid, &lty, &f32_ty, e)?;
                self.ins(OP_IMAGE_SAMPLE_EXPLICIT_LOD, &out4, &[sampled, coords, IMAGE_OPERAND_LOD, lod])
            }
            "textureSampleBias" => {
                let bv = self.arg(args, next, e)?;
                let bias = self.coerce(bv, &f32_ty, e)?;
                self.ins(OP_IMAGE_SAMPLE_IMPLICIT_LOD, &out4, &[sampled, coords, IMAGE_OPERAND_BIAS, bias])
            }
            "textureSampleGrad" => {
                let gty = Ty::Vector(ncoord, Scalar::F32);
                let dx = self.arg(args, next, e)?;
                let dx = self.coerce(dx, &gty, e)?;
                let dy = self.arg(args, next + 1, e)?;
                let dy = self.coerce(dy, &gty, e)?;
                self.ins(OP_IMAGE_SAMPLE_EXPLICIT_LOD, &out4, &[sampled, coords, IMAGE_OPERAND_GRAD, dx, dy])
            }
            "textureSampleCompare" | "textureSampleCompareLevel" => {
                let dv = self.arg(args, next, e)?;
                let dref = self.coerce(dv, &f32_ty, e)?;
                let r = if name == "textureSampleCompare" {
                    self.ins(OP_IMAGE_SAMPLE_DREF_IMPLICIT_LOD, &f32_ty, &[sampled, coords, dref])
                } else {
                    let zero = self.b.const_f32(0.0);
                    self.ins(OP_IMAGE_SAMPLE_DREF_EXPLICIT_LOD, &f32_ty, &[sampled, coords, dref, IMAGE_OPERAND_LOD, zero])
                };
                return Ok(Val::V(r, f32_ty));
            }
            _ => {
                if depth {
                    // `textureGather(t, s, coords)` of a depth texture.
                    let zero = self.b.const_f32(0.0);
                    self.ins(OP_IMAGE_DREF_GATHER, &out4, &[sampled, coords, zero])
                } else {
                    let c = self.b.const_u32(component);
                    self.ins(OP_IMAGE_GATHER, &out4, &[sampled, coords, c])
                }
            }
        };
        if depth && !gather {
            return Ok(Val::V(self.ins(OP_COMPOSITE_EXTRACT, &f32_ty, &[result, 0]), f32_ty));
        }
        Ok(Val::V(result, out4))
    }

    fn texture_load(&mut self, args: &[ExprId], e: ExprId) -> Result<Val, String> {
        let (img, dim, depth, s, _) = self.texture_arg(args, 0, e)?;
        let cv = self.arg(args, 1, e)?;
        let (cid, cty) = self.concrete(cv, Some(Scalar::I32), e)?;
        let mut next = 2;
        let layer = if matches!(dim, TexDim::D2Array | TexDim::CubeArray) {
            next += 1;
            Some(self.arg(args, 2, e)?)
        } else {
            None
        };
        let coords = self.coords_with_layer((cid, cty), layer, e)?;
        let lod = match args.get(next) {
            Some(a) => {
                let lv = self.expr(*a)?;
                self.concrete(lv, Some(Scalar::I32), *a)?.0
            }
            None => self.b.const_i32(0),
        };
        let out4 = Ty::Vector(4, s);
        let r = self.ins(OP_IMAGE_FETCH, &out4, &[img, coords, IMAGE_OPERAND_LOD, lod]);
        if depth {
            let f32_ty = Ty::Scalar(Scalar::F32);
            return Ok(Val::V(self.ins(OP_COMPOSITE_EXTRACT, &f32_ty, &[r, 0]), f32_ty));
        }
        Ok(Val::V(r, out4))
    }

    fn texture_query(&mut self, name: &str, args: &[ExprId], e: ExprId) -> Result<Val, String> {
        self.b.capability(CAP_IMAGE_QUERY);
        let (img, dim, _, _, _) = self.texture_arg(args, 0, e)?;
        let u = Ty::Scalar(Scalar::U32);
        if name == "textureNumLevels" {
            return Ok(Val::V(self.ins(OP_IMAGE_QUERY_LEVELS, &u, &[img]), u));
        }
        let lod = match args.get(1) {
            Some(a) => {
                let lv = self.expr(*a)?;
                self.concrete(lv, Some(Scalar::U32), *a)?.0
            }
            None => self.b.const_u32(0),
        };
        let (n, arrayed) = match dim {
            TexDim::D2 | TexDim::Cube => (2, false),
            TexDim::D2Array | TexDim::CubeArray => (3, true),
            TexDim::D3 => (3, false),
        };
        let qty = Ty::Vector(n, Scalar::U32);
        let size = self.ins(OP_IMAGE_QUERY_SIZE_LOD, &qty, &[img, lod]);
        if name == "textureNumLayers" {
            return Ok(Val::V(self.ins(OP_COMPOSITE_EXTRACT, &u, &[size, 2]), u));
        }
        if arrayed {
            return self.shuffle(size, &[0, 1], Scalar::U32);
        }
        Ok(Val::V(size, qty))
    }
}

fn storage_class(space: Space) -> u32 {
    match space {
        Space::Function => SC_FUNCTION,
        Space::Private => SC_PRIVATE,
        Space::Uniform => SC_UNIFORM,
        Space::Storage => SC_STORAGE_BUFFER,
        Space::Handle => SC_UNIFORM_CONSTANT,
    }
}

/// A full-screen triangle sampling one texture: the vertex stage places
/// vertex 0..2 by `vertex_index` (uv (0,0), (2,0), (0,2)), the fragment
/// stage samples texture binding 0 with sampler binding 1 (set 0) at the
/// interpolated uv. Entry points `vertex_main` / `fragment_main`.
pub fn fullscreen_blit_spirv() -> (Vec<u32>, Vec<u32>) {
    // Vertex stage.
    let mut b = SpvBuilder::new();
    let void = b.type_void();
    let fn_ty = b.type_function(void, &[]);
    let u32_ty = b.type_u32();
    let f32_ty = b.type_f32();
    let v2 = b.type_vector(f32_ty, 2);
    let v4 = b.type_vector(f32_ty, 4);
    let in_u32 = b.type_pointer(SC_INPUT, u32_ty);
    let out_v4 = b.type_pointer(SC_OUTPUT, v4);
    let out_v2 = b.type_pointer(SC_OUTPUT, v2);
    let vertex_index = b.global_variable(in_u32, SC_INPUT, 0);
    b.decorate(vertex_index, DECO_BUILT_IN, &[BUILTIN_VERTEX_INDEX]);
    let position = b.global_variable(out_v4, SC_OUTPUT, 0);
    b.decorate(position, DECO_BUILT_IN, &[BUILTIN_POSITION]);
    let uv_out = b.global_variable(out_v2, SC_OUTPUT, 0);
    b.decorate(uv_out, DECO_LOCATION, &[0]);
    let one_u = b.const_u32(1);
    let two_u = b.const_u32(2);
    let zero_f = b.const_f32(0.0);
    let one_f = b.const_f32(1.0);
    let two_f = b.const_f32(2.0);
    let ones = b.const_composite(v2, &[one_f, one_f]);
    let main = b.id();
    let mut ids = Vec::new();
    for _ in 0..10 {
        ids.push(b.id());
    }
    let label = b.id();
    let f = &mut b.functions;
    spv_inst(f, OP_FUNCTION, &[void, main, 0, fn_ty]);
    spv_inst(f, OP_LABEL, &[label]);
    spv_inst(f, OP_LOAD, &[u32_ty, ids[0], vertex_index]);
    spv_inst(f, OP_SHIFT_LEFT_LOGICAL, &[u32_ty, ids[1], ids[0], one_u]);
    spv_inst(f, OP_BITWISE_AND, &[u32_ty, ids[2], ids[1], two_u]);
    spv_inst(f, OP_CONVERT_U_TO_F, &[f32_ty, ids[3], ids[2]]);
    spv_inst(f, OP_BITWISE_AND, &[u32_ty, ids[4], ids[0], two_u]);
    spv_inst(f, OP_CONVERT_U_TO_F, &[f32_ty, ids[5], ids[4]]);
    spv_inst(f, OP_COMPOSITE_CONSTRUCT, &[v2, ids[6], ids[3], ids[5]]);
    spv_inst(f, OP_STORE, &[uv_out, ids[6]]);
    spv_inst(f, OP_VECTOR_TIMES_SCALAR, &[v2, ids[7], ids[6], two_f]);
    spv_inst(f, OP_F_SUB, &[v2, ids[8], ids[7], ones]);
    spv_inst(f, OP_COMPOSITE_CONSTRUCT, &[v4, ids[9], ids[8], zero_f, one_f]);
    spv_inst(f, OP_STORE, &[position, ids[9]]);
    spv_inst(f, OP_RETURN, &[]);
    spv_inst(f, OP_FUNCTION_END, &[]);
    b.entry_point(EXEC_MODEL_VERTEX, main, "vertex_main", &[vertex_index, position, uv_out]);
    let vertex = b.finish();

    // Fragment stage.
    let mut b = SpvBuilder::new();
    let void = b.type_void();
    let fn_ty = b.type_function(void, &[]);
    let f32_ty = b.type_f32();
    let v2 = b.type_vector(f32_ty, 2);
    let v4 = b.type_vector(f32_ty, 4);
    let image = b.type_image(f32_ty, DIM_2D, false, false);
    let sampler = b.type_sampler();
    let sampled = b.type_sampled_image(image);
    let image_ptr = b.type_pointer(SC_UNIFORM_CONSTANT, image);
    let sampler_ptr = b.type_pointer(SC_UNIFORM_CONSTANT, sampler);
    let in_v2 = b.type_pointer(SC_INPUT, v2);
    let out_v4 = b.type_pointer(SC_OUTPUT, v4);
    let source = b.global_variable(image_ptr, SC_UNIFORM_CONSTANT, 0);
    b.decorate(source, DECO_DESCRIPTOR_SET, &[0]);
    b.decorate(source, DECO_BINDING, &[0]);
    let source_sampler = b.global_variable(sampler_ptr, SC_UNIFORM_CONSTANT, 0);
    b.decorate(source_sampler, DECO_DESCRIPTOR_SET, &[0]);
    b.decorate(source_sampler, DECO_BINDING, &[1]);
    let uv_in = b.global_variable(in_v2, SC_INPUT, 0);
    b.decorate(uv_in, DECO_LOCATION, &[0]);
    let color = b.global_variable(out_v4, SC_OUTPUT, 0);
    b.decorate(color, DECO_LOCATION, &[0]);
    let main = b.id();
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(b.id());
    }
    let label = b.id();
    let f = &mut b.functions;
    spv_inst(f, OP_FUNCTION, &[void, main, 0, fn_ty]);
    spv_inst(f, OP_LABEL, &[label]);
    spv_inst(f, OP_LOAD, &[image, ids[0], source]);
    spv_inst(f, OP_LOAD, &[sampler, ids[1], source_sampler]);
    spv_inst(f, OP_SAMPLED_IMAGE, &[sampled, ids[2], ids[0], ids[1]]);
    spv_inst(f, OP_LOAD, &[v2, ids[3], uv_in]);
    spv_inst(f, OP_IMAGE_SAMPLE_IMPLICIT_LOD, &[v4, ids[4], ids[2], ids[3]]);
    spv_inst(f, OP_STORE, &[color, ids[4]]);
    spv_inst(f, OP_RETURN, &[]);
    spv_inst(f, OP_FUNCTION_END, &[]);
    b.entry_point(EXEC_MODEL_FRAGMENT, main, "fragment_main", &[uv_in, color]);
    b.execution_mode(main, EXEC_MODE_ORIGIN_UPPER_LEFT);
    (vertex, b.finish())
}

/// Test support: runs `spirv-val` (Vulkan 1.1 rules) on a module when the
/// tool is installed. None when it is not.
pub fn spirv_val(words: &[u32]) -> Option<Result<(), String>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "makepad-spirv-val-{}-{}.spv",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    if std::fs::write(&path, &bytes).is_err() {
        return None;
    }
    let out = std::process::Command::new("spirv-val")
        .arg("--target-env")
        .arg("vulkan1.1")
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);
    match out {
        Err(_) => None,
        Ok(o) if o.status.success() => Some(Ok(())),
        Ok(o) => Some(Err(format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ))),
    }
}
