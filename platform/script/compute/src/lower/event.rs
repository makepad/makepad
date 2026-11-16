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
//! helper). Every call is inlined, so each site is one place in the source.
//! A declaration's arguments are constants or functions; the program cannot
//! create a declaration at run time, so its host knows every declaration and
//! entry before anything runs.

use super::*;
use crate::ir::PortSig;

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
    /// Worst-case ops one entry may run ([`Program::total_cost`]).
    pub max_entry_cost: u64,
    /// The port (no arguments, no results) a loop calls, with its own site,
    /// when it reaches its run-time cap: reported, never silently cut.
    pub cap_port: Option<u16>,
    /// Include source statement marks in the IR.
    // cost: off creates no marks; on adds one Stmt per user statement.
    pub marks: bool,
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
    /// statements whose code optimization removes. Empty when marks are disabled.
    pub marks: std::ops::Range<u32>,
}

pub struct EventLowered {
    /// The entry functions present (in [`EventSpec::entry_fns`] order), then
    /// the declarations' functions in source order.
    pub entries: Vec<EventEntry>,
    pub state_init: Vec<u32>,
    pub state_vars: Vec<StateVar>,
    pub shared_init: Vec<u32>,
    pub decls: Vec<Decl>,
    /// The port sites: spans in the user's source (a port's `site` indexes it).
    pub sites: Vec<Span>,
    /// User statement spans indexed by the IR's marks; empty when disabled.
    pub marks: Vec<Span>,
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
            bufs: Vec::new(),
            io: false,
        }
    }
}

/// The event front end's part of the lowerer.
#[derive(Default)]
pub struct EventCtx {
    pub(super) ports: Vec<PortSig>,
    pub(super) forms: Vec<DeclForm>,
    pub(super) entry_fns: Vec<String>,
    pub(super) cap_port: Option<u16>,
    pub(super) decls: Vec<Decl>,
    /// Declarations' functions waiting to be lowered as entries.
    pub(super) handlers: Vec<FnDecl>,
    pub(super) sites: Vec<Span>,
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
        self.event.decls.push(Decl { form: form as u16, name: name.to_string(), args: out, handle, span });
        Ok(Bind::Const(V::Lit(handle as f64)))
    }

    /// A call of port `k`: its arguments converted to the port's types.
    pub(super) fn event_port(&mut self, k: usize, vals: &[V], args: &[Expr], span: Span) -> LResult<V> {
        let sig = self.event.ports[k];
        if vals.len() != sig.params.len() {
            return err(span, format!("`{}` takes {} argument{}", sig.name, sig.params.len(), if sig.params.len() == 1 { "" } else { "s" }));
        }
        let mut pargs = Vec::new();
        for ((v, t), a) in vals.iter().zip(sig.params).zip(args) {
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
        dce(&mut program);
        crate::opt::optimize(&mut program);
        let cost = program.total_cost();
        if cost > max_cost {
            return err(f.span, format!("`{}` may run {} operations; an event's limit is {}", f.name, cost, max_cost));
        }
        Ok(EventEntry { name: f.name.clone(), program, cost, span: f.span, marks })
    }
}

/// Lowers an event program (its prelude's items first, as [`crate::with_prelude`]
/// orders them).
pub fn lower_events(items: &[Item], prelude_base: usize, spec: &EventSpec) -> Result<EventLowered, ShaderError> {
    let mut l = new_lowerer(prelude_base, Domain::Event);
    l.marks = spec.marks;
    l.event.ports = spec.ports.to_vec();
    l.event.forms = spec.forms.to_vec();
    l.event.entry_fns = spec.entry_fns.iter().map(|s| s.to_string()).collect();
    l.event.cap_port = spec.cap_port;
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
    fns.extend(std::mem::take(&mut l.event.handlers));
    if fns.is_empty() {
        let names: Vec<String> = spec.entry_fns.iter().map(|n| format!("`fn {}()`", n)).chain(spec.forms.iter().map(|f| format!("`{}(..)`", f.name))).collect();
        return Err(ShaderError::new(0, 1, format!("nothing runs: add {}", names.join(" or "))));
    }
    let template = std::mem::replace(&mut l.b, Builder::new());
    let mut entries = Vec::new();
    for f in &fns {
        entries.push(l.event_entry(&template, f, spec.max_entry_cost)?);
    }
    let out = EventLowered { entries, state_init: l.state_init, state_vars: l.state_vars, shared_init: l.shared_init, decls: l.event.decls, sites: l.event.sites, marks: l.marks_spans };
    for e in &out.entries {
        if let Err(m) = crate::ir::validate_ports(&e.program, &out.regions(e), spec.ports) {
            return err(e.span, format!("internal compiler error in `{}`: {}", e.name, m));
        }
    }
    Ok(out)
}
