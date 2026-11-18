//! Event programs: one state layout (the program's `var`s and tables) and
//! several named entries over it, each a bounded program that runs to
//! completion when its event fires. A front end describes its surface with an
//! [`EventSpec`]:
//!
//! ```text
//! let out  = channel(3)               // a declaration form: its handle is a constant
//! var on   = false                    // state, shared by every entry
//! fn init() { send(out, 0) }          // an entry function by name
//! let tick = periodic(100, fn() {     // a declaration with a function argument:
//!     on = !on                        //   the function is an entry of its own
//!     send(out, if on { 1 } else { 0 })
//! })
//! ```
//!
//! Calls of the spec's ports lower to [`Stmt::Port`](crate::ir::Stmt::Port),
//! each naming its site: the innermost call in the user's code that led to it
//! (a port called inside a prelude helper names the user's call of the
//! helper). Every call is inlined, so each site is one place in the source,
//! unless the spec keeps big helpers as functions ([`EventSpec::call_ops`]):
//! a port inside such a function names its own site.
//! A declaration's arguments are constants or functions; the program cannot
//! create a declaration at run time, so its host knows every declaration and
//! entry before anything runs.

use super::*;
use crate::ir::{Elem, PortSig};

/// How a declaration form's handle (the value its `let` name stands for) is
/// made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclHandle {
    /// The declaration's argument `k` (an integer constant).
    Arg(usize),
    /// Its ordinal among the declarations of its form's class (0, 1, ...).
    Ordinal,
}

/// A top-level declaration form: `let name = <form>(args..)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclForm {
    pub name: &'static str,
    pub handle: DeclHandle,
    /// Forms of one class share one numbering of their ordinals.
    pub class: u8,
    pub buffer: Option<BufForm>,
}

/// A buffer declaration; arguments at `len`'s indices multiply to its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufForm {
    pub elem: Elem,
    pub len: &'static [usize],
    pub writable: bool,
}

#[derive(Clone, Debug)]
pub struct EventBuf {
    /// Index into `EventLowered::decls`.
    pub decl: u16,
    pub elem: Elem,
    pub len: u32,
    pub writable: bool,
}

/// The surface of an event-program front end.
#[derive(Clone, Copy, Debug)]
pub struct EventSpec<'a> {
    /// The ports, by number: a call of `ports[k].name` is port `k`.
    pub ports: &'a [PortSig],
    pub forms: &'a [DeclForm],
    /// Functions that are entries by their name (no parameters; never called
    /// by the program).
    pub entry_fns: &'a [&'a str],
    /// Whether the program has a float type. Without one, an untyped number
    /// literal defaults to i32 wherever it would otherwise default to f32.
    /// Literals written with a fraction or exponent, and PI, TAU and E, are
    /// refused at their span. Constant division of two literals truncates
    /// toward zero, as i32 division does.
    pub floats: bool,
    /// Worst-case ops one entry may run ([`Program::total_cost`]).
    pub max_entry_cost: u64,
    /// The port (no arguments, no results) a loop calls, with its own site,
    /// when it reaches its run-time cap: reported, never silently cut.
    pub cap_port: Option<u16>,
    /// Include source statement and condition marks in the IR.
    // cost: off creates no marks; on adds one Stmt per user statement and branching condition.
    pub marks: bool,
    /// Keep a helper as a function of the program ([`Stmt::Call`](crate::ir::Stmt::Call)) when
    /// two or more places call it and its body costs more than this many ops; None inlines every
    /// call. A function is shared by every entry ([`EventLowered::funcs`]); the state scalars it
    /// touches go in and out by value, and its locals take frame words above every entry's.
    // cost: one Call per call site instead of the helper's body; a few argument copies per call.
    pub call_ops: Option<u64>,
}

/// A declaration's argument as lowered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeclArg {
    Int(i32),
    Float(f32),
    /// A function: entry `k` of [`EventLowered::entries`].
    Entry(u16),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    /// Its form, an index into [`EventSpec::forms`].
    pub form: u16,
    /// The `let` name.
    pub name: String,
    pub args: Vec<DeclArg>,
    pub handle: i32,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EventEntry {
    /// An entry function's name, or the `let` name of the declaration whose
    /// function it is.
    pub name: String,
    pub program: Program,
    /// Worst-case ops of one run.
    pub cost: u64,
    pub span: Span,
    /// IDs in `EventLowered::marks` allocated while lowering this entry, including
    /// statements whose code optimization removes and conditions
    /// (`EventLowered::conds`). Empty when marks are disabled.
    pub marks: std::ops::Range<u32>,
}

/// A helper kept as a function ([`EventSpec::call_ops`]).
#[derive(Clone, Debug)]
pub struct EventFunc {
    pub name: String,
    pub span: Span,
    /// IDs in `EventLowered::marks` allocated while lowering it, as [`EventEntry::marks`]. The
    /// ranges nest: a function first called inside another (or inside an entry) has its range
    /// inside the caller's, so a mark belongs to the innermost range that holds it.
    pub marks: std::ops::Range<u32>,
}

pub struct EventLowered {
    /// The entry functions present (in [`EventSpec::entry_fns`] order), then
    /// the declarations' functions in source order.
    pub entries: Vec<EventEntry>,
    /// The helpers kept as functions, in [`Program::funcs`] order: every entry's program holds
    /// all of them, identical in each (empty when none are).
    pub funcs: Vec<EventFunc>,
    pub state_init: Vec<u32>,
    pub state_vars: Vec<StateVar>,
    pub shared_init: Vec<u32>,
    pub decls: Vec<Decl>,
    pub buffers: Vec<EventBuf>,
    /// The port sites: spans in the user's source (a port's `site` indexes it).
    pub sites: Vec<Span>,
    /// User statement and condition spans indexed by the IR's marks; empty when disabled.
    pub marks: Vec<Span>,
    /// The marks that name a source condition rather than a statement, ascending: (mark, whether
    /// the `If` right after the mark tests the condition itself; false: its negation, as a
    /// `while`'s exit does). The conditions of `if`s, `while`s and of a `&&` or `||` whose right
    /// side runs only on its left's value. A condition that takes no branch (a select between two
    /// values, a constant) has no `If`: its mark is in no program. Its statements, after the `If`,
    /// carry their own marks.
    pub conds: Vec<(u32, bool)>,
    /// Per helper kept as a function, by name: the calls that kept a copy of it instead (inlined,
    /// or a function of their own), because an untyped literal argument could not go in as a
    /// value without computing differently ([`EventSpec::call_ops`]; empty without it).
    pub copies: Vec<(String, u32)>,
}

impl EventLowered {
    /// What every entry may touch.
    pub fn regions(&self, entry: &EventEntry) -> crate::ir::Regions {
        crate::ir::Regions {
            ctx: 0,
            state: self.state_init.len() as u32,
            shared: self.shared_init.len() as u32,
            frame: entry.program.frame_words,
            shared_writable: false,
            bufs: self.buffers.iter().map(|b| b.writable).collect(),
            io: false,
        }
    }
}

/// The event front end's part of the lowerer.
#[derive(Default)]
pub struct EventCtx {
    pub(super) integer_literals: bool,
    pub(super) ports: Vec<PortSig>,
    pub(super) forms: Vec<DeclForm>,
    pub(super) entry_fns: Vec<String>,
    pub(super) cap_port: Option<u16>,
    pub(super) call_ops: Option<u64>,
    pub(super) decls: Vec<Decl>,
    pub(super) buffers: Vec<EventBuf>,
    /// Declarations' functions waiting to be lowered as entries.
    pub(super) handlers: Vec<FnDecl>,
    pub(super) sites: Vec<Span>,
    /// Per helper: the calls that kept a copy because an untyped literal argument could not be
    /// passed as a value ([`EventLowered::copies`]).
    pub(super) copies: std::collections::BTreeMap<String, u32>,
}

impl Lowerer {
    /// `let name = form(args..)` at the top level.
    pub(super) fn event_decl(&mut self, name: &str, form: usize, args: &[Expr], span: Span) -> LResult<Bind> {
        let mut out = Vec::new();
        for a in args {
            let lambda = match &a.kind {
                ExprKind::Lambda(f) => Some(FnDecl { name: name.to_string(), ..(**f).clone() }),
                ExprKind::Ident(n) if self.fns.contains_key(n) && self.globals.get(n).is_none() => self.fns.get(n).cloned(),
                _ => None,
            };
            if let Some(f) = lambda {
                if !f.params.is_empty() {
                    return err(f.span, "an event's function takes no parameters");
                }
                out.push(DeclArg::Entry(self.event.handlers.len() as u16));
                self.event.handlers.push(f);
                continue;
            }
            let scratch = self.scratch();
            let saved = std::mem::replace(&mut self.b, scratch);
            let r = self.expr(a);
            let arg = match r {
                Ok(V::Lit(x)) if self.event.integer_literals => self.literal_i32(x, a.span).map(DeclArg::Int),
                Ok(V::Lit(x)) if x.fract() == 0.0 && x >= i32::MIN as f64 && x <= i32::MAX as f64 => Ok(DeclArg::Int(x as i32)),
                Ok(V::Lit(x)) => Ok(DeclArg::Float(x as f32)),
                Ok(V::I(v)) | Ok(V::B(v)) if self.b.cst(v).is_some() => Ok(DeclArg::Int(self.b.cst(v).unwrap_or(0) as i32)),
                Ok(V::F(v)) if self.b.cst(v).is_some() => Ok(DeclArg::Float(f32::from_bits(self.b.cst(v).unwrap_or(0)))),
                Ok(_) => err(a.span, format!("`{}(..)` takes constants and functions", self.event.forms[form].name)),
                Err(e) => Err(e),
            };
            self.b = saved;
            out.push(arg?);
        }
        let f = self.event.forms[form];
        let handle = match f.handle {
            DeclHandle::Ordinal => self.event.decls.iter().filter(|d| self.event.forms[d.form as usize].class == f.class).count() as i32,
            DeclHandle::Arg(k) => match out.get(k) {
                Some(DeclArg::Int(h)) => *h,
                _ => return err(span, format!("`{}(..)`: argument {} is an integer constant", f.name, k + 1)),
            },
        };
        let bind = if let Some(b) = f.buffer {
            let mut len = 1i64;
            for &k in b.len {
                let Some(DeclArg::Int(n)) = out.get(k) else {
                    return err(span, format!("`{}(..)`: buffer length arguments must be integer constants whose product is in 1..=i32::MAX", f.name));
                };
                let Some(product) = len.checked_mul(*n as i64) else {
                    return err(span, format!("`{}(..)`: buffer length product overflows", f.name));
                };
                len = product;
            }
            if !(1..=i32::MAX as i64).contains(&len) {
                return err(span, format!("`{}(..)`: buffer length must be in 1..=i32::MAX", f.name));
            }
            let k = self.event.buffers.len();
            if k > u8::MAX as usize {
                return err(span, format!("`{}(..)`: at most 256 buffers (Region::Buf's u8 format limit)", f.name));
            }
            let Ok(decl) = u16::try_from(self.event.decls.len()) else {
                return err(span, format!("`{}(..)`: declaration index exceeds the u16 format limit", f.name));
            };
            self.event.buffers.push(EventBuf { decl, elem: b.elem, len: len as u32, writable: b.writable });
            Bind::Place(Place { region: Region::Buf(k as u8), root: 0, extent: len as u32, off: None, stat: 0, ty: T::Buf(Box::new(T::I), 1, 0, b.writable) })
        } else {
            Bind::Const(V::Lit(handle as f64))
        };
        self.event.decls.push(Decl { form: form as u16, name: name.to_string(), args: out, handle, span });
        Ok(bind)
    }

    /// A call of port `k`: its arguments converted to the port's types.
    pub(super) fn event_port(&mut self, k: usize, vals: &[V], args: &[Expr], span: Span) -> LResult<V> {
        let sig = self.event.ports[k];
        if vals.len() != sig.params.len() {
            return err(span, format!("`{}` takes {} argument{}", sig.name, sig.params.len(), if sig.params.len() == 1 { "" } else { "s" }));
        }
        let mut pargs = Vec::new();
        for (param, ((v, t), a)) in vals.iter().zip(sig.params).zip(args).enumerate() {
            let buffer = match v {
                V::Place(Place { region: Region::Buf(k), ty: T::Buf(..), .. }) => Some(*k),
                _ => None,
            };
            if let Some(b) = sig.bufs.iter().find(|b| b.param as usize == param) {
                let Some(k) = buffer.filter(|_| matches!(a.kind, ExprKind::Ident(_))) else {
                    return err(a.span, format!("`{}`: argument {} must be a buffer name", sig.name, param + 1));
                };
                let buf = &self.event.buffers[k as usize];
                let name = &self.event.decls[buf.decl as usize].name;
                if b.elem.is_some_and(|e| e != buf.elem) {
                    return err(a.span, format!("`{}`: buffer `{}` has the wrong element kind (expected {:?}, got {:?})", sig.name, name, b.elem.unwrap(), buf.elem));
                }
                if b.writes && !buf.writable {
                    return err(a.span, format!("`{}` writes read-only buffer `{}`", sig.name, name));
                }
                pargs.push(self.b.ci(k as i32));
                continue;
            }
            if buffer.is_some() {
                return err(a.span, format!("`{}`: argument {} takes a number, not a buffer", sig.name, param + 1));
            }
            pargs.push(match t {
                Ty::F32 => self.to_f(v, a.span)?,
                Ty::I32 => self.to_i(v, a.span)?,
                Ty::Bool => self.truth(v, a.span)?,
                Ty::F64 => return err(a.span, "ports take 32-bit arguments"),
            });
        }
        let mut rets = Vec::new();
        for t in sig.rets {
            self.b.prog.vals.push(*t);
            self.b.consts.push(None);
            rets.push(Val(self.b.prog.vals.len() as u32 - 1));
        }
        let site = self.event_site(span);
        self.b.push(IS::Port { port: k as u16, args: pargs, rets: rets.clone(), site });
        Ok(match (sig.rets.first(), rets.first()) {
            (Some(Ty::F32), Some(v)) => V::F(*v),
            (Some(Ty::I32), Some(v)) => V::I(*v),
            (Some(Ty::Bool), Some(v)) => V::B(*v),
            _ => V::Unit,
        })
    }

    /// The site of a port call at `span`: the innermost call in user code.
    pub(super) fn event_site(&mut self, span: Span) -> u32 {
        let at = if span.start < self.prelude_base { span } else { self.call_spans.iter().rev().find(|s| s.start < self.prelude_base).copied().unwrap_or(span) };
        match self.event.sites.iter().position(|s| *s == at) {
            Some(k) => k as u32,
            None => {
                self.event.sites.push(at);
                self.event.sites.len() as u32 - 1
            }
        }
    }

    /// One entry: the state scalars loaded, `f` inlined, the ones it may
    /// have set stored back.
    fn event_entry(&mut self, template: &Builder, f: &FnDecl, max_cost: u64) -> LResult<EventEntry> {
        self.b = template.clone();
        self.frame_words = 0;
        self.load_promoted();
        let mark = self.b.blocks[0].len();
        let first_mark = self.marks_spans.len() as u32;
        self.call(f, vec![], f.span)?;
        let marks = first_mark..self.marks_spans.len() as u32;
        let mut set = std::collections::HashSet::new();
        fn sets(b: &[IS], out: &mut std::collections::HashSet<u32>) {
            for s in b {
                match s {
                    IS::Set(v, _) => {
                        out.insert(v.0);
                    }
                    IS::If(_, t, e) => {
                        sets(t, out);
                        sets(e, out);
                    }
                    IS::Loop { body, .. } => sets(body, out),
                    _ => {}
                }
            }
        }
        sets(&self.b.blocks[0][mark..], &mut set);
        for (word, var) in self.promoted.clone() {
            if set.contains(&var.0) {
                let x = self.b.get(var);
                self.b.store(Region::State, word, 1, None, x);
            }
        }
        let b = std::mem::replace(&mut self.b, Builder::new());
        let mut program = b.prog;
        program.body = b.blocks.into_iter().next().unwrap_or_default();
        program.frame_words = self.frame_words;
        program.bufs = self.event.buffers.iter().map(|b| b.elem).collect();
        let mut entry = EventEntry { name: f.name.clone(), program, cost: 0, span: f.span, marks };
        // With functions, the entries are finished once every function is known.
        if self.event.call_ops.is_none() {
            dce(&mut entry.program);
            crate::opt::optimize(&mut entry.program);
            entry_cost(&mut entry, max_cost)?;
        }
        Ok(entry)
    }
}

fn entry_cost(e: &mut EventEntry, max_cost: u64) -> LResult<()> {
    e.cost = e.program.total_cost();
    if e.cost > max_cost {
        return err(e.span, format!("`{}` may run {} operations; an event's limit is {}", e.name, e.cost, max_cost));
    }
    Ok(())
}

/// Moves a function's frame words up by `by` (its locals' words are its own from 0).
fn move_frame(b: &mut crate::ir::Block, by: u32) {
    use crate::ir::{Op, Region, Stmt};
    for s in b {
        match s {
            Stmt::Def(_, Op::Load { region: Region::Frame, base, .. }) | Stmt::Store { region: Region::Frame, base, .. } => *base += by,
            Stmt::If(_, t, e) => {
                move_frame(t, by);
                move_frame(e, by);
            }
            Stmt::Loop { body, .. } => move_frame(body, by),
            _ => {}
        }
    }
}

/// Every entry gets the shared functions, their frame words above every entry's own (entries run
/// one at a time; a function's locals stay live across the calls it makes, so each has its own).
fn finish_with_funcs(l: &mut Lowerer, entries: &mut [EventEntry], max_cost: u64) -> LResult<Vec<EventFunc>> {
    let mut words = entries.iter().map(|e| e.program.frame_words).max().unwrap_or(0);
    let mut funcs = Vec::new();
    let mut named = Vec::new();
    for (mut g, meta) in std::mem::take(&mut l.funcs_out) {
        move_frame(&mut g.body, words);
        words += meta.frame;
        // (`dce` does not see a function's results; `optimize` below does.)
        funcs.push(g);
        named.push(EventFunc { name: meta.name, span: meta.span, marks: meta.marks });
    }
    if let (true, Some(f)) = (words > MAX_FRAME_WORDS, named.last()) {
        return err(f.span, format!("local arrays and structs exceed {} words; use a `var` (state) for big buffers", MAX_FRAME_WORDS));
    }
    // Optimized once, so that every entry holds the same functions.
    let mut shared = Program { funcs, ..Default::default() };
    crate::opt::optimize(&mut shared);
    let funcs = shared.funcs;
    for e in entries {
        if !funcs.is_empty() {
            e.program.funcs = funcs.clone();
            e.program.frame_words = words;
        }
        dce(&mut e.program);
        crate::opt::optimize(&mut e.program);
        if !funcs.is_empty() {
            e.program.funcs = funcs.clone();
        }
        entry_cost(e, max_cost)?;
    }
    Ok(named)
}

/// Lowers an event program (its prelude's items first, as [`crate::with_prelude`]
/// orders them).
pub fn lower_events(items: &[Item], prelude_base: usize, spec: &EventSpec) -> Result<EventLowered, ShaderError> {
    let mut l = new_lowerer(prelude_base, Domain::Event);
    l.marks = spec.marks;
    l.event.integer_literals = !spec.floats;
    l.event.ports = spec.ports.to_vec();
    l.event.forms = spec.forms.to_vec();
    l.event.entry_fns = spec.entry_fns.iter().map(|s| s.to_string()).collect();
    l.event.cap_port = spec.cap_port;
    l.event.call_ops = spec.call_ops;
    l.top(items)?;
    let mut fns: Vec<FnDecl> = Vec::new();
    for name in spec.entry_fns {
        if let Some(f) = l.fns.get(*name) {
            if !f.params.is_empty() {
                return err(f.span, format!("`fn {}()` takes no parameters", name));
            }
            fns.push(f.clone());
        }
    }
    // Entries: the entry functions, then the handlers (a declaration names
    // its handler by entry index).
    let first_handler = fns.len() as u16;
    for d in &mut l.event.decls {
        for a in &mut d.args {
            if let DeclArg::Entry(e) = a {
                *e += first_handler;
            }
        }
    }
    // The handlers stay in the context: their calls count as call sites (`Lowerer::sites`).
    fns.extend(l.event.handlers.iter().cloned());
    if fns.is_empty() {
        let names: Vec<String> = spec.entry_fns.iter().map(|n| format!("`fn {}()`", n)).chain(spec.forms.iter().map(|f| format!("`{}(..)`", f.name))).collect();
        return Err(ShaderError::new(0, 1, format!("nothing runs: add {}", names.join(" or "))));
    }
    let template = std::mem::replace(&mut l.b, Builder::new());
    let mut entries = Vec::new();
    for f in &fns {
        entries.push(l.event_entry(&template, f, spec.max_entry_cost)?);
    }
    let funcs = if spec.call_ops.is_some() { finish_with_funcs(&mut l, &mut entries, spec.max_entry_cost)? } else { Vec::new() };
    let out = EventLowered { entries, funcs, state_init: l.state_init, state_vars: l.state_vars, shared_init: l.shared_init, decls: l.event.decls, buffers: l.event.buffers, sites: l.event.sites, marks: l.marks_spans, conds: l.cond_marks, copies: l.event.copies.into_iter().collect() };
    for e in &out.entries {
        if let Err(m) = crate::ir::validate_ports(&e.program, &out.regions(e), spec.ports) {
            return err(e.span, format!("internal compiler error in `{}`: {}", e.name, m));
        }
    }
    Ok(out)
}
