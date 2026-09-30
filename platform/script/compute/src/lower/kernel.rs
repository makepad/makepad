//! The kernel front end: stateless per-element programs over host buffers
//! (vertices, instances, emitted primitives, map and reduce), lowered into
//! one AIR program whose outer loop runs the elements of one call.
//!
//! A kernel declares, at its top level:
//!
//! ```text
//! let hf  = input(f32)                  // host buffer "hf", read-only
//! let pos = output(vec3, 16, 0, verts)  // view of host buffer "verts":
//!                                       //   element i at words 16*i + 0..3
//! let seg = emit_buffer(18, 8)          // up to 8 records of 18 words per
//!                                       //   element, plus "seg_count"
//! let amp = param(1.0, 0, 10)           // uniform, set by name
//! fn vertex(i) { pos[i] = vec3(...) }   // or element / instance /
//!                                       //   primitive / reduce_sum|min|max
//! ```
//!
//! Inputs `time` (f32), `seed` and `count` (i32) are always available.
//! Buffer accesses clamp to the host buffer's real length, so a kernel
//! cannot read or write outside what the host bound.

use super::*;

/// Ctx words of a kernel call.
pub const K_BASE: u32 = 0;
pub const K_COUNT: u32 = 1;
pub const K_TIME: u32 = 2;
pub const K_SEED: u32 = 3;
/// Host buffer 0: the control word (non-zero = stop at the next element),
/// bound by the runtime to the call's cancel token.
pub const CONTROL_BUFFER: &str = "#control";
/// Set to 1 when an emit found its element's slots full.
pub const K_OVERFLOW: u32 = 5;
/// A reduce kernel's running value (up to 16 lanes).
pub const K_ACC: u32 = 6;
/// Set to 1 when a host function call failed (its results were zero).
pub const K_HOST_ERR: u32 = 22;
pub const K_PARAMS: u32 = 23;
/// Most elements one call may run.
pub const ELEMENT_CAP: u32 = 1 << 30;
pub const MAX_BUFFERS: usize = 64;
/// Worst-case AIR ops per element.
pub const MAX_COST_PER_ELEMENT: u64 = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    /// Emitted records: `width` words each, `capacity` slots per element.
    Emit { width: u32, capacity: u32 },
    /// The per-element record counts of an emit buffer.
    EmitCount,
}

/// A host buffer a kernel uses (region `Buf(index)`).
#[derive(Clone, Debug)]
pub struct BufferDecl {
    pub name: String,
    pub access: Access,
    /// Largest per-element stride among its views (words).
    pub stride: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReduceOp {
    Sum,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelKind {
    /// Writes outputs per element (vertex, instance, element, primitive).
    Map,
    /// Combines one value per element (lanes 1..=16) in element order.
    Reduce(ReduceOp, u8),
}

#[derive(Default)]
pub struct KernelCtx {
    pub buffers: Vec<BufferDecl>,
    pub(super) element: Option<Val>,
    /// (emit data region, counter var) while lowering the element body.
    counters: Vec<(u8, Var)>,
    /// Buffer offsets that address the current element's own record.
    pub(super) local_offsets: std::collections::HashSet<Val>,
    /// Some access to a writable buffer is not to the element's own record:
    /// the kernel cannot be split across threads.
    pub nonlocal: bool,
    /// Buffers viewed with different strides (records of different
    /// elements may overlap).
    mixed: std::collections::HashSet<u8>,
}

impl KernelCtx {
    /// Notes an access to buffer k at `off`.
    pub(super) fn access(&mut self, k: u8, off: Option<Val>) {
        if self.writable(k) && (self.mixed.contains(&k) || !off.is_some_and(|o| self.local_offsets.contains(&o))) {
            self.nonlocal = true;
        }
    }

    pub fn writable(&self, k: u8) -> bool {
        self.buffers.get(k as usize).is_some_and(|b| b.access != Access::Read)
    }

    fn buffer(&mut self, name: &str, access: Access, stride: u32, span: Span) -> LResult<u8> {
        if let Some(k) = self.buffers.iter().position(|b| b.name == name) {
            let b = &mut self.buffers[k];
            match (b.access, access) {
                (Access::Read, Access::Write) | (Access::Write, Access::Read) => b.access = Access::Write,
                (a, b2) if a == b2 => {}
                _ => return err(span, format!("buffer `{}` is declared two incompatible ways", name)),
            }
            if b.stride != stride {
                self.mixed.insert(k as u8);
                b.stride = b.stride.max(stride);
            }
            return Ok(k as u8);
        }
        if self.buffers.is_empty() {
            self.buffers.push(BufferDecl { name: CONTROL_BUFFER.into(), access: Access::Read, stride: 1 });
        }
        if self.buffers.len() >= MAX_BUFFERS {
            return err(span, format!("at most {} buffers", MAX_BUFFERS));
        }
        self.buffers.push(BufferDecl { name: name.to_string(), access, stride });
        Ok(self.buffers.len() as u8 - 1)
    }
}

/// A kernel's math functions (`let math = portable | fast`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathMode {
    /// f32 Cephes-style polynomials: fast, deterministic on every machine.
    Fast,
    /// fdlibm kernels in f64, rounded once: the same bits as
    /// `makepad_csg_math::portable`; NaN results are stored as one
    /// canonical quiet NaN.
    Portable,
}

/// The one NaN a portable kernel stores.
pub const CANONICAL_NAN: u32 = 0x7FC0_0000;

pub struct KernelLowered {
    pub kind: KernelKind,
    pub math: MathMode,
    /// The entry's name (vertex, instance, element, primitive, reduce_*).
    pub entry: String,
    pub program: Program,
    pub init: Option<Program>,
    pub params: Vec<ParamInfo>,
    pub buffers: Vec<BufferDecl>,
    pub shared_init: Vec<u32>,
    /// Worst-case AIR ops per element.
    pub cost: u64,
    /// Every write (and every read of a written buffer) touches only the
    /// element's own records: element ranges can run on different threads.
    pub parallel_safe: bool,
}

/// A host-defined record layout (a GPU vertex or instance struct from the
/// shader compiler's reflection): the kernel writes it in place by field.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub name: String,
    /// Words per record (including padding).
    pub stride: u32,
    pub fields: Vec<LayoutField>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutField {
    pub name: String,
    pub ty: FieldTy,
    /// Word offset inside the record.
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldTy {
    F32,
    I32,
    Vec2,
    Vec3,
    Vec4,
    Mat4,
}

impl FieldTy {
    pub fn words(self) -> u32 {
        match self {
            FieldTy::F32 | FieldTy::I32 => 1,
            FieldTy::Vec2 => 2,
            FieldTy::Vec3 => 3,
            FieldTy::Vec4 => 4,
            FieldTy::Mat4 => 16,
        }
    }
}

pub const ENTRIES: &[&str] = &["element", "vertex", "instance", "primitive", "reduce_sum", "reduce_min", "reduce_max"];

fn type_words(name: &str) -> Option<(T, u32)> {
    Some(match name {
        "f32" | "float" => (T::F, 1),
        "i32" | "int" | "u32" => (T::I, 1),
        // Two words: low, then high.
        "f64" => (T::D, 2),
        "vec2" => (T::Vec(2), 2),
        "vec3" => (T::Vec(3), 3),
        "vec4" => (T::Vec(4), 4),
        "mat4" => (T::Vec(16), 16),
        _ => return None,
    })
}

impl Lowerer {
    /// Registers host layouts as structs (fields at their given offsets).
    fn add_layouts(&mut self, layouts: &[Layout]) -> LResult<()> {
        for lay in layouts {
            let mut fields = Vec::new();
            for f in &lay.fields {
                let t = match f.ty {
                    FieldTy::F32 => T::F,
                    FieldTy::I32 => T::I,
                    FieldTy::Vec2 => T::Vec(2),
                    FieldTy::Vec3 => T::Vec(3),
                    FieldTy::Vec4 => T::Vec(4),
                    FieldTy::Mat4 => T::Vec(16),
                };
                if f.offset + f.ty.words() > lay.stride {
                    return Err(ShaderError::new(0, 1, format!("layout {}: field {} does not fit its stride", lay.name, f.name)));
                }
                let init = match &t {
                    T::Vec(n) => CInit::Vec(*n, [0; 16]),
                    t => CInit::W(0, t.clone()),
                };
                fields.push((f.name.clone(), t, f.offset, init));
            }
            fields.sort_by_key(|f| f.2);
            self.structs.push(StructDef { name: lay.name.clone(), fields, words: lay.stride.max(1) });
        }
        Ok(())
    }

    /// A small non-negative integer constant argument.
    fn const_arg(&mut self, e: &Expr, what: &str) -> LResult<u32> {
        let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
        let r = self.expr(e);
        self.b = saved;
        match r? {
            V::Lit(x) if x.fract() == 0.0 && x >= 0.0 && x < (1u64 << 24) as f64 => Ok(x as u32),
            _ => err(e.span, format!("{} must be a whole-number constant", what)),
        }
    }

    /// A top-level `input(...)`, `output(...)`, `emit_buffer(...)` or
    /// `param(...)` of a kernel.
    pub(super) fn kernel_decl(&mut self, name: &str, f: &str, args: &[Expr], span: Span) -> LResult<Bind> {
        let ident = |e: &Expr| match &e.kind {
            ExprKind::Ident(s) => Some(s.clone()),
            _ => None,
        };
        match f {
            "param" => {
                let mut nums = Vec::new();
                for a in args {
                    let scratch = self.scratch();
                    let saved = std::mem::replace(&mut self.b, scratch);
                    let r = self.expr(a);
                    self.b = saved;
                    match r? {
                        V::Lit(x) => nums.push(x as f32),
                        _ => return err(a.span, "param(default, min, max) takes constant numbers"),
                    }
                }
                let (default, min, max) = match nums[..] {
                    [d] => (d, f32::MIN, f32::MAX),
                    [d, lo, hi] => (d, lo, hi),
                    _ => return err(span, "param(default) or param(default, min, max)"),
                };
                if self.params.len() >= 256 {
                    return err(span, "at most 256 params");
                }
                self.params.push(ParamInfo { name: name.to_string(), default, min, max });
                Ok(Bind::Uniform(self.params.len() - 1))
            }
            "input" | "output" => {
                let Some(a0) = args.first() else {
                    return err(span, format!("{}(type [, stride, offset, buffer])", f));
                };
                let tn = ident(a0);
                let layout = tn.as_ref().and_then(|n| self.structs.iter().position(|s| s.name == *n));
                let (elem, words) = match (tn.as_deref().and_then(type_words), layout) {
                    (Some(t), _) => t,
                    (None, Some(sid)) => (T::Struct(sid), self.structs[sid].words),
                    _ => return err(a0.span, "the element type: f32, i32, vec2, vec3, vec4, mat4, or a layout or struct name"),
                };
                let stride = match args.get(1) {
                    Some(e) => self.const_arg(e, "the stride")?,
                    None => words,
                };
                let offset = match args.get(2) {
                    Some(e) => self.const_arg(e, "the offset")?,
                    None => 0,
                };
                // A record must fit inside its stride, or neighbouring
                // elements' records would overlap.
                if stride < words {
                    return err(span, format!("a stride of {} is smaller than the {} words of one record", stride, words));
                }
                if offset.checked_add(words).is_none_or(|end| end > stride) {
                    return err(span, format!("offset {} + {} words does not fit a stride of {}", offset, words, stride));
                }
                let host = match args.get(3) {
                    Some(e) => match ident(e) {
                        Some(n) => n,
                        None => return err(e.span, "the buffer name (an identifier)"),
                    },
                    None => name.to_string(),
                };
                let writable = f == "output";
                let access = if writable { Access::Write } else { Access::Read };
                let k = self.kernel.buffer(&host, access, stride, span)?;
                Ok(Bind::Place(Place {
                    region: Region::Buf(k),
                    root: 0,
                    extent: u32::MAX,
                    off: None,
                    stat: 0,
                    ty: T::Buf(Box::new(elem), stride, offset, writable),
                }))
            }
            _ => {
                // emit_buffer(width or type, capacity [, buffer])
                if args.len() < 2 {
                    return err(span, "emit_buffer(width, slots_per_element [, buffer])");
                }
                let tn = ident(&args[0]);
                let layout = tn.as_ref().and_then(|n| self.structs.iter().position(|s| s.name == *n));
                let width = match (tn.as_deref().and_then(type_words), layout) {
                    (Some((_, w)), _) => w,
                    (None, Some(sid)) => self.structs[sid].words,
                    _ => self.const_arg(&args[0], "the record width")?,
                };
                let capacity = self.const_arg(&args[1], "the slots per element")?;
                if width == 0 || capacity == 0 {
                    return err(span, "an emit buffer needs a width and at least one slot");
                }
                let host = match args.get(2) {
                    Some(e) => ident(e).ok_or_else(|| ShaderError::new(e.span.start, e.span.end, "the buffer name (an identifier)".into()))?,
                    None => name.to_string(),
                };
                let Some(slots) = width.checked_mul(capacity).filter(|w| *w <= 1 << 24) else {
                    return err(span, "records x slots per element is too large");
                };
                let kd = self.kernel.buffer(&host, Access::Emit { width, capacity }, slots, span)?;
                let kc = self.kernel.buffer(&format!("{}_count", host), Access::EmitCount, 1, span)?;
                Ok(Bind::Emit(kd, kc, width, capacity))
            }
        }
    }

    pub(super) fn kernel_ident(&mut self, name: &str) -> Option<V> {
        Some(match name {
            "time" => V::F(self.b.load(Ty::F32, Region::Ctx, K_TIME, 1, None)),
            "seed" => V::I(self.b.load(Ty::I32, Region::Ctx, K_SEED, 1, None)),
            "count" => V::I(self.b.load(Ty::I32, Region::Ctx, K_COUNT, 1, None)),
            _ => return None,
        })
    }

    /// A call of a registered host function: per slice parameter three
    /// arguments (a buffer, a word offset, a word length), then the scalar
    /// parameters.
    pub(super) fn kernel_host_call(&mut self, name: &str, vals: &[V], args: &[Expr], span: Span) -> LResult<V> {
        let f = crate::host::find(name).unwrap();
        let h = crate::host::get(f).unwrap();
        let want = 3 * h.slices.len() + h.params.len();
        if vals.len() != want {
            return err(span, format!("`{}` takes {} arguments: {}", name, want, h.doc));
        }
        if self.portable && h.tier != crate::host::Tier::X {
            return err(span, format!("`{}` gives the same bits only on one device: not in a `math: portable` kernel", name));
        }
        let mut slices = Vec::new();
        for (k, sig) in h.slices.iter().enumerate() {
            let (b, o, l) = (&vals[3 * k], &vals[3 * k + 1], &vals[3 * k + 2]);
            let buf = match b {
                V::Place(Place { region: Region::Buf(kb), ty: T::Buf(..), .. }) => *kb,
                _ => return err(args[3 * k].span, format!("`{}`: argument {} is a buffer (then its word offset and word length)", name, 3 * k + 1)),
            };
            if sig.writable {
                if !self.kernel.writable(buf) {
                    return err(args[3 * k].span, format!("`{}` writes this buffer: bind an output(...)", name));
                }
                // The call writes a range the compiler cannot prove is the
                // element's own: the kernel runs on one thread.
                self.kernel.nonlocal = true;
            } else {
                self.kernel.access(buf, None);
            }
            let off = self.to_i(o, args[3 * k + 1].span)?;
            let len = self.to_i(l, args[3 * k + 2].span)?;
            slices.push(ir::SliceArg { buf, off, len });
        }
        let mut sargs = Vec::new();
        for (k, t) in h.params.iter().enumerate() {
            let at = 3 * h.slices.len() + k;
            let (v, sp) = (&vals[at], args[at].span);
            sargs.push(match t {
                Ty::F32 => self.to_f(v, sp)?,
                Ty::I32 => self.to_i(v, sp)?,
                Ty::Bool => self.truth(v, sp)?,
                Ty::F64 => return err(sp, "host functions take word arguments"),
            });
        }
        let mut rets = Vec::new();
        for t in h.rets {
            self.b.prog.vals.push(*t);
            self.b.consts.push(None);
            rets.push(Val(self.b.prog.vals.len() as u32 - 1));
        }
        self.b.push(IS::CallHost { f, args: sargs, slices, rets: rets.clone() });
        Ok(match (h.rets.first(), rets.first()) {
            (Some(Ty::F32), Some(v)) => V::F(*v),
            (Some(Ty::I32), Some(v)) => V::I(*v),
            (Some(Ty::Bool), Some(v)) => V::B(*v),
            _ => V::Unit,
        })
    }

    /// `emit(buf, values...)`: appends one record to the element's slots.
    pub(super) fn kernel_emit(&mut self, args: &[Expr], span: Span) -> LResult<V> {
        let Some(first) = args.first() else {
            return err(span, "emit(buffer, values...)");
        };
        let Some(Bind::Emit(kd, _kc, width, capacity)) = (match &first.kind {
            ExprKind::Ident(n) => self.lookup(n),
            _ => None,
        }) else {
            return err(first.span, "emit's first argument is an emit_buffer(...)");
        };
        let Some(elem) = self.kernel.element else {
            return err(span, "emit() runs inside the kernel");
        };
        let Some(&(_, counter)) = self.kernel.counters.iter().find(|(k, _)| *k == kd) else {
            return err(span, "emit() runs inside the kernel");
        };
        // Flatten the values into words.
        let mut words: Vec<Val> = Vec::new();
        for a in &args[1..] {
            let v = self.expr(a)?;
            match v {
                V::Vec(n, l) => words.extend_from_slice(&l[..n as usize]),
                V::I(x) => words.push(x),
                // A struct or layout record: all of its words, as stored.
                V::Place(p) if matches!(p.ty, T::Struct(_)) => {
                    let n = self.words(&p.ty);
                    for k in 0..n {
                        let leaf = self.leaf_ty(&p.ty, k);
                        let (base, extent, off) = self.offset_plus(&p, k);
                        words.push(self.b.load(leaf, p.region, base, extent, off));
                    }
                }
                V::B(x) => {
                    let one = self.b.ci(1);
                    let zero = self.b.ci(0);
                    words.push(self.b.sel(x, one, zero));
                }
                v => words.push(self.to_f(&v, a.span)?),
            }
        }
        if words.len() as u32 != width {
            return err(span, format!("this emit writes {} words; the buffer's records are {}", words.len(), width));
        }
        let c = self.b.get(counter);
        let cap = self.b.ci(capacity as i32);
        let room = self.b.cmpi(Cmp::Lt, c, cap);
        self.b.open();
        let slot = self.b.ib(Bin::MulI, elem, cap);
        let slot = self.b.ib(Bin::AddI, slot, c);
        let w = self.b.ci(width as i32);
        let at = self.b.ib(Bin::MulI, slot, w);
        for (k, x) in words.iter().enumerate() {
            self.b.push(IS::Store { region: Region::Buf(kd), base: k as u32, extent: u32::MAX, off: Some(at), val: *x });
        }
        let one = self.b.ci(1);
        let c1 = self.b.ib(Bin::AddI, c, one);
        self.b.set(counter, c1);
        let then_ = self.b.close();
        self.b.open();
        let one = self.b.ci(1);
        self.b.store(Region::Ctx, K_OVERFLOW, 1, None, one);
        let else_ = self.b.close();
        self.b.push(IS::If(room, then_, else_));
        Ok(V::Unit)
    }
}

/// Lowers a kernel.
pub fn lower_kernel(items: &[Item], prelude_base: usize, layouts: &[Layout]) -> Result<KernelLowered, ShaderError> {
    let mut l = new_lowerer(prelude_base, Domain::Kernel);
    l.add_layouts(layouts)?;
    // `let math = portable | fast`: the kernel's math mode.
    let mut math = MathMode::Fast;
    let mut rest = Vec::with_capacity(items.len());
    for item in items {
        if let Item::Let { name, value, span, .. } = item {
            if name == "math" {
                math = match &value.kind {
                    ExprKind::Ident(m) if m == "portable" => MathMode::Portable,
                    ExprKind::Ident(m) if m == "fast" => MathMode::Fast,
                    _ => return err(*span, "`let math = portable` (f32 math through f64, bit-exact with the modelling libraries) or `let math = fast`"),
                };
                continue;
            }
        }
        rest.push(item.clone());
    }
    let items = &rest[..];
    l.portable = math == MathMode::Portable;
    l.top(items)?;
    let entries: Vec<&str> = ENTRIES.iter().copied().filter(|e| l.fns.contains_key(*e)).collect();
    let entry_name = match entries[..] {
        [e] => e.to_string(),
        [] => {
            return Err(ShaderError::new(
                0,
                1,
                "missing kernel: add `fn vertex(i)`, `fn instance(i)`, `fn element(i)`, `fn primitive(i)` or `fn reduce_sum(i)` / `reduce_min` / `reduce_max`".into(),
            ))
        }
        _ => {
            let f = &l.fns[entries[1]];
            return err(f.span, format!("a kernel has one entry; found {}", entries.join(", ")));
        }
    };
    let entry = l.fns[&entry_name].clone();
    if entry.params.len() != 1 {
        return err(entry.span, format!("`fn {}(i)` takes the element index", entry_name));
    }
    let init = match l.fns.get("init").cloned() {
        Some(f) => Some(l.init_program(&f)?),
        None => None,
    };
    // The element loop.
    let n = l.b.raw(Ty::I32, Op::FrameCount, None);
    let base = l.b.load(Ty::I32, Region::Ctx, K_BASE, 1, None);
    let i = l.b.var(Ty::I32);
    let zero = l.b.ci(0);
    l.b.set(i, zero);
    l.b.open_loop();
    l.loops.push(LoopKind::Frame);
    let iv = l.b.get(i);
    let done = l.b.cmpi(Cmp::Ge, iv, n);
    l.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
    // The control word: the cancel token, polled every element.
    if l.kernel.buffers.is_empty() {
        l.kernel.buffers.push(BufferDecl { name: CONTROL_BUFFER.into(), access: Access::Read, stride: 1 });
    }
    let cancel = l.b.load(Ty::I32, Region::Buf(0), 0, 1, None);
    let zero = l.b.ci(0);
    let stop = l.b.cmpi(Cmp::Ne, cancel, zero);
    l.b.push(IS::If(stop, vec![IS::Break(0)], vec![]));
    let e = l.b.ib(Bin::AddI, base, iv);
    l.kernel.element = Some(e);
    let emits: Vec<(u8, u8)> = l
        .globals
        .values()
        .filter_map(|b| match b {
            Bind::Emit(kd, kc, ..) => Some((*kd, *kc)),
            _ => None,
        })
        .collect();
    let mut emits = emits;
    emits.sort();
    for (kd, _) in &emits {
        let c = l.b.var(Ty::I32);
        let zero = l.b.ci(0);
        l.b.set(c, zero);
        l.kernel.counters.push((*kd, c));
    }
    let v = l.call(&entry, vec![V::I(e)], entry.span)?;
    let kind = match entry_name.as_str() {
        "reduce_sum" | "reduce_min" | "reduce_max" => {
            let op = match entry_name.as_str() {
                "reduce_sum" => ReduceOp::Sum,
                "reduce_min" => ReduceOp::Min,
                _ => ReduceOp::Max,
            };
            let (n, lanes) = match v {
                V::Vec(n, lanes) => (n, lanes),
                V::Unit => return err(entry.span, "a reduce kernel returns the element's value (f32 or a vector)"),
                v => (1, [l.to_f(&v, entry.span)?; 16]),
            };
            for k in 0..n as usize {
                let a = l.b.load(Ty::F32, Region::Ctx, K_ACC + k as u32, 1, None);
                let r = match op {
                    ReduceOp::Sum => l.b.add(a, lanes[k]),
                    ReduceOp::Min => l.b.fb(Bin::MinF, a, lanes[k]),
                    ReduceOp::Max => l.b.fb(Bin::MaxF, a, lanes[k]),
                };
                l.b.store(Region::Ctx, K_ACC + k as u32, 1, None, r);
            }
            KernelKind::Reduce(op, n)
        }
        _ => {
            if !matches!(v, V::Unit) {
                return err(entry.span, format!("`fn {}(i)` returns nothing: write its results into output buffers", entry_name));
            }
            KernelKind::Map
        }
    };
    for (kd, kc) in &emits {
        let c = l.kernel.counters.iter().find(|(k, _)| k == kd).unwrap().1;
        let cv = l.b.get(c);
        l.b.push(IS::Store { region: Region::Buf(*kc), base: 0, extent: u32::MAX, off: Some(e), val: cv });
    }
    let one = l.b.ci(1);
    let next = l.b.ib(Bin::AddI, iv, one);
    l.b.set(i, next);
    l.loops.pop();
    let body = l.b.close();
    l.b.push(IS::Loop { cap: ELEMENT_CAP, body });
    let mut program = l.b.prog;
    program.body = l.b.blocks.into_iter().next().unwrap();
    program.frame_words = l.frame_words;
    if math == MathMode::Portable {
        canonical_nan_stores(&mut program.body, &mut program.vals);
    }
    dce(&mut program);
    let cost = program.cost();
    if cost > MAX_COST_PER_ELEMENT {
        return Err(ShaderError::new(0, 1, format!("too much work per element (worst case {} ops); reduce loop sizes", cost)));
    }
    let parallel_safe = !l.kernel.nonlocal;
    Ok(KernelLowered { kind, math, entry: entry_name, program, init, params: l.params, buffers: l.kernel.buffers, shared_init: l.shared_init, cost, parallel_safe })
}

/// Every f32 stored to a host buffer goes through `x != x ? NaN : x`, so
/// any NaN leaves the kernel as [`CANONICAL_NAN`] whatever produced it.
fn canonical_nan_stores(b: &mut Block, vals: &mut Vec<Ty>) {
    let mut out = Vec::with_capacity(b.len());
    for mut s in std::mem::take(b) {
        match &mut s {
            IS::Store { region: Region::Buf(_), val, .. } if vals[val.0 as usize] == Ty::F32 => {
                let x = *val;
                let mut new = |ty: Ty| {
                    vals.push(ty);
                    Val(vals.len() as u32 - 1)
                };
                let (is_nan, nan, sel) = (new(Ty::Bool), new(Ty::F32), new(Ty::F32));
                out.push(IS::Def(is_nan, Op::CmpF(Cmp::Ne, x, x)));
                out.push(IS::Def(nan, Op::ConstF(f32::from_bits(CANONICAL_NAN))));
                out.push(IS::Def(sel, Op::Sel(is_nan, nan, x)));
                *val = sel;
            }
            IS::If(_, t, e) => {
                canonical_nan_stores(t, vals);
                canonical_nan_stores(e, vals);
            }
            IS::Loop { body, .. } => canonical_nan_stores(body, vals),
            _ => {}
        }
        out.push(s);
    }
    *b = out;
}
