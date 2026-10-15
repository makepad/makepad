//! Typed AST + typeck::Body -> BIR, following rustc_mir_build: places (as_place), operands
//! (as_operand), destination-passing (into), scopes with StorageDead/Drop at exits, temporary
//! scopes (statement temps, `let` extension, edition-dependent tail temporaries), match / if-let /
//! while-let / for / `?` desugarings, two-phase autoref'd receivers, implicit reborrows at
//! coercion sites, closures with 2021 precise captures (capture.rs).
//!
//! lower.rs is the reference for how each Body fact is read (autoderef counts, overloaded
//! deref/index with the mutable variants in mutable place contexts, binding modes).

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::bir::*;
use super::tyx::Tx;
use crate::ast::*;
use crate::fxhash::HashMap;
use crate::program::{DefId, DefKind};
use crate::typeck::{Body, Coerce, Res};
use crate::types::{TyId, TyKind};

/// Places rooted at an enclosing body's local (closure bodies): local = VBASE + typeck local.
pub const VBASE: u32 = 0x8000_0000;

pub(super) struct Scope {
    /// locals to StorageDead (and Drop when the bool is set), in scheduling order
    drops: Vec<(Local, bool)>,
    /// a temporary scope (statement, condition, guard, body): temps go to the innermost one
    temp: bool,
}

struct LoopCx {
    label: Option<Vec<u8>>,
    brk: Bb,
    cont: Option<Bb>,
    dest: Option<Place>,
    depth: usize,
}

pub struct ClosureOut {
    pub expr: u32,
    pub bir: Bir,
    pub kind: ClosureKind,
}

pub struct Build<'a> {
    pub tx: &'a Tx<'a>,
    pub b: &'a Body,
    pub ast: &'a Ast,
    pub pf: &'a crate::parser::ParsedFile,
    pub out: Bir,
    pub(super) cur: Bb,
    pub(super) span: Span,
    pub(super) scopes: Vec<Scope>,
    loops: Vec<LoopCx>,
    /// typeck local -> its place in this body
    pub(super) locals: HashMap<u32, Place>,
    /// guard bindings: (typeck local, its mapping before the guard)
    guard_saved: Vec<(u32, Option<Place>)>,
    /// spans of the next call's arguments
    arg_spans: Vec<Span>,
    /// temps whose value drop is scheduled once they are filled: (local, scope)
    pending_drop: Vec<(Local, usize)>,
    /// evaluating call arguments: coercion reborrows are two-phase
    args_two_phase: bool,
    /// building a closure body: its expr id
    closure: Option<u32>,
    edition: u16,
    ret_ty: TyId,
    /// expression ids whose temporaries are lifetime-extended to this scope index
    extend: Vec<(u32, Option<usize>)>,
    /// closure bodies built inside this body (nested ones are in their parent's list)
    pub closures_out: Vec<ClosureOut>,
    /// kinds of closures built so far (file, expr) -> kind, shared with nested builders
    pub closure_kinds: HashMap<u32, ClosureKind>,
    /// the def of the fn whose body this is (for generic bounds of callee params)
    def: DefId,
    /// the next call's user type (written self type, explicit type arguments)
    call_user_ty: PathUserTy,
    /// the next call's callee, when it is a fn item value held in a place
    call_value: Option<Place>,
    /// closure expressions passed where an `Fn(&T, ..)` bound gives their signature: per
    /// parameter, whether its outer reference is higher-ranked
    hr_closures: HashMap<u32, Vec<bool>>,
    /// closure expressions passed where an `Fn*` bound gives their kind
    pub(super) expected_kinds: HashMap<u32, ClosureKind>,
    /// the next block tail has an expected type (false: a statement / unannotated `let`)
    tail_expect: bool,
    /// while binding a pattern: the matched expression's span
    pub(super) scrut_span: Option<Span>,
    /// builtin index expressions that are the base chain of an outer index (rustc
    /// lower_index_expression: only the outermost one reads the fake borrows)
    inner_index: crate::fxhash::HashSet<u32>,
    /// the fake borrows of the inner indexes of the index being built
    index_fake_temps: Vec<Local>,
    /// closure expr -> the parameter whose higher-ranked region the bound's elided output
    /// takes (`impl FnOnce(&u32) -> &u32`)
    hr_ret_ties: HashMap<u32, usize>,
}

macro_rules! unsup {
    ($s:expr, $msg:expr) => {{
        if $s.out.unsupported.is_none() {
            $s.out.unsupported = Some($msg.to_string());
        }
    }};
}

pub fn strip_paren(ast: &Ast, mut e: ExprId) -> ExprId {
    while let ExprKind::Paren(x) = &ast.expr(e).kind {
        e = *x;
    }
    e
}

impl<'a> Build<'a> {
    pub fn new(tx: &'a Tx<'a>, b: &'a Body, name: String) -> Build<'a> {
        let pf = &tx.prog.files[b.file as usize];
        let out = Bir {
            checked_body: None,
            blocks: vec![BasicBlock { stmts: Vec::new(), term: None }],
            locals: Vec::new(),
            n_args: 0,
            regions: vec![RegionOrigin::Static],
            known: Vec::new(),
            outlives: Vec::new(),
            captures: Vec::new(),
            closure_kind: None,
            closures: Vec::new(),
            unsupported: None,
            file: b.file,
            name,
            def: None,
            args: Vec::new(),
            closure_expr: None,
            module: 0,
            user_tys: Vec::new(),
            rvalue_user_tys: Vec::new(),
            local_user_tys: Vec::new(),
            ascriptions: Vec::new(),
            agg_callee: Vec::new(),
            hr_args: Vec::new(),
            ov_results: Vec::new(),
            owner: None,
            capture_spans: Vec::new(),
            guard_locals: Vec::new(),
            index_fakes: Vec::new(),
            expected_sig: false,
            static_temps: Vec::new(),
            static_callees: Vec::new(),
            infer_params: Vec::new(),
            union_inits: Vec::new(),
            const_uses: Vec::new(),
            hr_ret: None,
            move_sites: Vec::new(),
            call_user_tys: Vec::new(),
            fn_values: Vec::new(),
            fn_anns: Vec::new(),
            wf_annots: Vec::new(),
        };
        let mut bd = Build {
            tx,
            b,
            ast: &pf.ast,
            pf,
            out,
            cur: 0,
            span: Span::default(),
            scopes: Vec::new(),
            loops: Vec::new(),
            locals: HashMap::default(),
            guard_saved: Vec::new(),
            arg_spans: Vec::new(),
            pending_drop: Vec::new(),
            args_two_phase: false,
            closure: None,
            edition: pf.edition,
            ret_ty: b.ret,
            extend: Vec::new(),
            closures_out: Vec::new(),
            closure_kinds: HashMap::default(),
            def: b.def,
            call_user_ty: PathUserTy::default(),
            call_value: None,
            hr_closures: HashMap::default(),
            expected_kinds: HashMap::default(),
            tail_expect: true,
            scrut_span: None,
            inner_index: Default::default(),
            index_fake_temps: Vec::new(),
            hr_ret_ties: HashMap::default(),
        };
        if b.def != crate::program::NO_DEF {
            bd.out.module = tx.prog.def(b.def).scope;
            bd.out.owner = Some(b.def);
        }
        bd
    }

    // ------------------------------------------------------------------ emission

    pub fn new_block(&mut self) -> Bb {
        self.out.blocks.push(BasicBlock { stmts: Vec::new(), term: None });
        self.out.blocks.len() as u32 - 1
    }

    pub(super) fn push(&mut self, k: StmtKind) {
        let sp = self.span;
        let c = self.cur as usize;
        if self.out.blocks[c].term.is_some() {
            // after a terminator: unreachable code goes to a fresh (unreachable) block
            self.cur = self.new_block();
        }
        let c = self.cur as usize;
        self.out.blocks[c].stmts.push(Statement { kind: k, span: sp });
    }

    pub(super) fn term(&mut self, k: TermKind) {
        let sp = self.span;
        let c = self.cur as usize;
        if self.out.blocks[c].term.is_some() {
            self.cur = self.new_block();
        }
        let c = self.cur as usize;
        self.out.blocks[c].term = Some(Term { kind: k, span: sp });
    }

    pub(super) fn goto(&mut self, t: Bb) {
        self.term(TermKind::Goto(t));
    }

    pub(super) fn switch_to(&mut self, b: Bb) {
        self.cur = b;
    }

    /// A fresh unreachable block for code after a diverging expression.
    pub(super) fn dead(&mut self) {
        self.cur = self.new_block();
    }

    pub(super) fn assign(&mut self, p: Place, rv: Rvalue) {
        let l = if p.proj.is_empty() { Some(p.local) } else { None };
        self.push(StmtKind::Assign(p, rv));
        if let Some(l) = l {
            self.filled(l);
        }
    }

    pub(super) fn rv(&mut self) -> Rv {
        self.out.regions.push(RegionOrigin::Var);
        self.out.regions.len() as u32 - 1
    }

    pub fn new_local(&mut self, ty: TyId, kind: LocalKind, mutable: bool, name: Option<String>) -> Local {
        self.out.locals.push(LocalDecl { ty, kind, mutable, name, span: self.span, regions: Vec::new(), is_ref_binding: false, can_be_made_mutable: false });
        self.out.locals.len() as u32 - 1
    }

    /// A temporary in the innermost temporary scope (or the extended scope for `e`).
    pub(super) fn temp_for(&mut self, ty: TyId, e: Option<ExprId>) -> Local {
        let l = self.new_local(ty, LocalKind::Temp, true, None);
        self.push(StmtKind::StorageLive(l));
        let nd = self.tx.needs_drop(ty);
        let mut target = None;
        if let Some(e) = e {
            if let Some(&(_, d)) = self.extend.iter().find(|x| x.0 == e.0) {
                target = Some(d);
            }
        }
        // None is the outer lifetime of a constant initializer: rustc's
        // as_temp schedules neither storage nor value drops for these temps.
        let si = match target {
            Some(Some(d)) => Some(d),
            Some(None) => return l,
            None => self.temp_scope_index(),
        };
        let Some(si) = si else { return l };
        if si < self.scopes.len() {
            // storage now; the value's drop once the temp is filled (rustc as_temp schedules
            // the value drop after building the expression, so it runs before the inner temps'
            // storage ends)
            self.scopes[si].drops.push((l, false));
            if nd {
                self.pending_drop.push((l, si));
            }
        }
        l
    }

    /// The temp `l` is filled: schedule its value drop.
    fn filled(&mut self, l: Local) {
        if let Some(i) = self.pending_drop.iter().position(|x| x.0 == l) {
            let (_, si) = self.pending_drop.remove(i);
            if si < self.scopes.len() {
                self.scopes[si].drops.push((l, true));
            }
        }
    }

    pub(super) fn temp(&mut self, ty: TyId) -> Local {
        self.temp_for(ty, None)
    }

    pub(super) fn temp_scope_index(&self) -> Option<usize> {
        for i in (0..self.scopes.len()).rev() {
            if self.scopes[i].temp {
                return Some(i);
            }
        }
        None
    }

    pub(super) fn push_scope(&mut self, temp: bool) -> usize {
        self.scopes.push(Scope { drops: Vec::new(), temp });
        self.scopes.len() - 1
    }

    /// Emits the drops of scope `i` (innermost first within it) without popping it.
    pub(super) fn emit_scope_drops(&mut self, i: usize) {
        let drops: Vec<(Local, bool)> = self.scopes[i].drops.clone();
        for (l, value) in drops.into_iter().rev() {
            if value {
                let next = self.new_block();
                self.term(TermKind::Drop(Place::local(l), next));
                self.switch_to(next);
            } else {
                self.push(StmtKind::StorageDead(l));
            }
        }
    }

    pub(super) fn pop_scope(&mut self) {
        let i = self.scopes.len() - 1;
        self.emit_scope_drops(i);
        self.scopes.pop();
    }

    /// Leave scopes down to `depth` (break / continue / return): their drops, scopes kept.
    pub(super) fn exit_to(&mut self, depth: usize) {
        let n = self.scopes.len();
        for i in (depth..n).rev() {
            self.emit_scope_drops(i);
        }
    }

    pub(super) fn schedule(&mut self, l: Local) {
        if !self.scopes.is_empty() {
            let si = self.scopes.len() - 1;
            self.schedule_in(l, si);
        }
    }

    pub(super) fn schedule_in(&mut self, l: Local, si: usize) {
        let nd = self.tx.needs_drop(self.out.locals[l as usize].ty);
        if let Some(s) = self.scopes.get_mut(si) {
            s.drops.push((l, false));
            if nd {
                s.drops.push((l, true));
            }
        }
    }

    pub(super) fn set_span(&mut self, e: ExprId) -> Span {
        let old = self.span;
        let ex = self.ast.expr(e);
        self.span = Span { lo: ex.lo, hi: ex.hi };
        old
    }

    // ------------------------------------------------------------------ types

    pub fn place_ty(&self, p: &Place) -> TyId {
        let mut t = if p.local >= VBASE {
            self.b.locals[(p.local - VBASE) as usize].ty
        } else {
            self.out.locals[p.local as usize].ty
        };
        for pr in &p.proj {
            t = match pr {
                Proj::Deref => self.tx.builtin_deref(t).unwrap_or(t),
                Proj::Field(_, ft) => *ft,
                Proj::Index(_) | Proj::ConstIndex(..) => match self.tx.kind(t) {
                    TyKind::Array(e, _) | TyKind::Slice(e) => e,
                    _ => t,
                },
                Proj::Subslice(..) | Proj::Downcast(_) => t,
            };
        }
        t
    }

    pub(super) fn ty(&self, e: ExprId) -> TyId {
        self.b.ty(e)
    }

    pub(super) fn operand_of(&self, p: Place) -> Operand {
        let t = self.place_ty(&p);
        if self.tx.is_copy(t) {
            Operand::Copy(p)
        } else {
            Operand::Move(p)
        }
    }

    pub(super) fn is_place_expr(&self, e: ExprId) -> bool {
        match &self.ast.expr(e).kind {
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                Some(Res::Local(_)) => true,
                Some(Res::Def(d)) => matches!(self.tx.prog.def(*d).kind, DefKind::Static | DefKind::ForeignStatic),
                _ => false,
            },
            ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) | ExprKind::Unary(UnOp::Deref, _) => true,
            ExprKind::Paren(x) => self.is_place_expr(*x),
            _ => false,
        }
    }

    // ------------------------------------------------------------------ fn bodies

    pub fn build_body(&mut self) {
        use super::body::{BodyKind, CheckedBody};
        let Some(owner) = CheckedBody::item(self.tx.prog, self.b.def) else {
            unsup!(self, "no checked body owner");
            return;
        };
        self.out.checked_body = Some(owner);
        if owner.kind == BodyKind::Function {
            self.build_fn();
        } else {
            let d = self.tx.prog.def(self.b.def);
            let init = match self.ast.item(d.item).kind {
                ItemKind::Const(_, init) | ItemKind::Static(_, _, init) if init.is_some() => self.tx.prog.const_body(d.file, d.item).expect("checked initializer").unwrap(),
                _ => unreachable!(),
            };
            self.out.def = Some(self.b.def);
            self.out.args = self.b.args.to_vec();
            self.build_const_expr(init, self.b.ret);
        }
    }

    pub fn build_const_child(&mut self, owner: super::body::CheckedBody) {
        use super::body::BodyKind;
        self.out.checked_body = Some(owner);
        self.out.args = self.b.args.to_vec();
        match owner.kind {
            BodyKind::InlineConst(e) => {
                let ExprKind::ConstBlock(block) = self.ast.expr(e).kind else { unreachable!() };
                let ret = self.b.ty(e);
                self.ret_ty = ret;
                self.new_local(ret, LocalKind::Ret, true, None);
                if let Some(Stmt::Expr(tail, false)) = self.ast.stmts(block).last() {
                    self.extend_const_initializer(*tail);
                }
                self.push_scope(true);
                self.block_into(Place::local(0), block, Some(ret));
                self.pop_scope();
                self.term(TermKind::Return);
            }
            BodyKind::AnonConst(e) => self.build_const_expr(e, self.tx.tcx.tys.usize_),
            _ => unreachable!(),
        }
    }

    /// rustc_mir_build::builder::construct_const: zero arguments, typed return
    /// place, independent CFG and the ordinary lexical drop scopes.
    fn build_const_expr(&mut self, init: ExprId, ret: TyId) {
        self.ret_ty = ret;
        self.span = Span { lo: self.ast.expr(init).lo, hi: self.ast.expr(init).hi };
        self.new_local(ret, LocalKind::Ret, true, None);
        self.extend_const_initializer(init);
        // Unlike a function body, the constant root is not a terminating
        // temporary scope. Nested statement/condition/tail scopes still are.
        // A temporary reaching this root has no StorageDead or value drop.
        self.push_scope(false);
        self.into_coerced(Place::local(0), init, ret);
        self.pop_scope();
        self.term(TermKind::Return);
    }

    /// rustc_hir_analysis::check::region::visit_body: const initializers use
    /// the same extending-expression rules as lets, with no enclosing drop scope.
    fn extend_const_initializer(&mut self, init: ExprId) {
        let mut ext = Vec::new();
        self.collect_extending(init, &mut ext);
        self.extend.extend(ext.into_iter().map(|e| (e, None)));
    }

    /// `_0`, the args, the param patterns and the body block.
    pub fn build_fn(&mut self) {
        self.out.def = Some(self.b.def);
        self.out.args = self.b.args.to_vec();
        let ret = self.b.ret;
        self.ret_ty = ret;
        let r0 = self.new_local(ret, LocalKind::Ret, true, None);
        debug_assert_eq!(r0, 0);
        let ast = self.ast;
        let d = self.tx.prog.def(self.b.def);
        let it = ast.item(d.item);
        let fsig = match &it.kind {
            ItemKind::Fn(s, _, _) => ast.fn_sig(*s),
            _ => {
                unsup!(self, "not a fn");
                return;
            }
        };
        if fsig.asyncness {
            unsup!(self, "async fn");
            return;
        }
        let body = match self.b.body {
            Some(b) => b,
            None => {
                unsup!(self, "no body");
                return;
            }
        };
        // args scope (dropped last), then the body's temporary scope
        self.push_scope(false);
        let mut pi = 0usize;
        if fsig.self_param.is_some() {
            let t = self.b.locals.get(0).map_or(self.tx.unit(), |l| l.ty);
            let mutable = self.b.locals.get(0).map_or(false, |l| l.mutable);
            let a = self.new_local(t, LocalKind::Arg, mutable, Some("self".to_string()));
            self.out.locals[a as usize].can_be_made_mutable = matches!(fsig.self_param, Some(crate::ast::SelfParam::Value(false) | crate::ast::SelfParam::Typed(false, _)));
            self.out.n_args += 1;
            self.locals.insert(0, Place::local(a));
            self.schedule(a);
            pi = 1;
        }
        let _ = pi;
        let mut binds = Vec::new();
        for p in ast.list(fsig.params) {
            let t = self.b.pty(p.pat);
            let a = self.new_local(t, LocalKind::Arg, false, None);
            self.out.n_args += 1;
            self.schedule(a);
            binds.push((p.pat, a));
        }
        for (p, a) in binds {
            self.bind_param(p, a);
        }
        self.push_scope(true);
        let bl = ast.block(body);
        self.span = Span { lo: bl.lo, hi: bl.hi };
        self.block_into(Place::local(0), body, Some(ret));
        self.pop_scope();
        self.span = Span { lo: bl.hi.saturating_sub(1), hi: bl.hi };
        self.pop_scope();
        self.term(TermKind::Return);
    }

    /// A parameter pattern bound from its arg local (a plain binding is the arg itself).
    pub(super) fn bind_param(&mut self, p: PatId, a: Local) {
        let ast = self.ast;
        match ast.pat(p) {
            Pat::Ident { by_ref: false, sub: None, mutbl, name } if !self.b.pat_res.contains_key(&p.0) && !self.b.pat_bind_ref.contains_key(&p.0) => {
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    self.locals.insert(li, Place::local(a));
                    self.out.locals[a as usize].mutable = *mutbl;
                    self.out.locals[a as usize].name = Some(self.pf.text(*name).to_string());
                    self.out.locals[a as usize].span = Span { lo: name.lo, hi: name.hi };
                    self.out.locals[a as usize].can_be_made_mutable = true;
                }
            }
            _ => {
                let t = self.out.locals[a as usize].ty;
                self.declare_bindings(p);
                self.bind_pat(p, Place::local(a), t, false);
            }
        }
    }

    pub fn build_closure_body(&mut self, ce: u32, kind: ClosureKind, closure_ty: TyId) {
        self.closure = Some(ce);
        self.out.checked_body = Some(super::body::CheckedBody {
            owner: self.b.def, kind: super::body::BodyKind::Closure(ExprId(ce)),
        });
        self.out.closure_expr = Some(ce);
        let (params, body) = match &self.ast.expr(ExprId(ce)).kind {
            ExprKind::Closure(row) => {
                let crate::ast::ClosureRow { params, body, is_async, .. } = self.ast.row(*row);
                if *is_async {
                    unsup!(self, "async closure");
                    return;
                }
                (params.clone(), *body)
            }
            _ => {
                unsup!(self, "async block");
                return;
            }
        };
        let ret = match self.tx.kind(closure_ty) {
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => match self.tx.kind(sig) {
                TyKind::FnPtr(_, r, _, _) => r,
                _ => self.ty(body),
            },
            _ => self.ty(body),
        };
        self.ret_ty = ret;
        self.span = { let x = self.ast.expr(ExprId(ce)); Span { lo: x.lo, hi: x.hi } };
        let r0 = self.new_local(ret, LocalKind::Ret, true, None);
        debug_assert_eq!(r0, 0);
        if let ExprKind::Closure(row) = &self.ast.expr(ExprId(ce)).kind {
            if let Some(rt) = &self.ast.row(*row).ret {
                if ast_ty_has_ref(self.ast, *rt, 0) {
                    self.ascribe_local(0, *rt);
                }
            }
        }
        let env_ty = match kind {
            ClosureKind::Fn => self.tx.ref_ty(false, closure_ty),
            ClosureKind::FnMut => self.tx.ref_ty(true, closure_ty),
            ClosureKind::FnOnce => closure_ty,
        };
        self.push_scope(false);
        let env = self.new_local(env_ty, LocalKind::Arg, kind == ClosureKind::FnOnce, Some("<env>".to_string()));
        self.out.n_args += 1;
        if kind == ClosureKind::FnOnce {
            self.schedule(env);
        }
        let mut binds = Vec::new();
        let ret_has_ref = match &self.ast.expr(ExprId(ce)).kind {
            // an elided reference in the return type ties it to a parameter's (elision)
            ExprKind::Closure(row) => self.ast.row(*row).ret.is_some_and(|r| ast_ty_has_elided_ref(self.ast, r, 0)),
            _ => false,
        };
        let hr_bound = if ret_has_ref { Vec::new() } else { self.hr_closures.get(&ce).cloned().unwrap_or_default() };
        // passed where an `Fn*` bound / object type gives its signature: rustc takes the
        // signature from it (elided regions higher-ranked), not from inference
        self.out.expected_sig = self.expected_closure_kind(ce).is_some() || self.hr_closures.contains_key(&ce);
        for (k, p) in self.ast.list(params).iter().enumerate() {
            let t = self.b.pty(p.pat);
            let a = self.new_local(t, LocalKind::Arg, false, None);
            self.out.n_args += 1;
            // an elided `&` in the parameter's annotation, or from the bound, is higher-ranked
            let annotated_hr = match p.ty.map(|at| self.ast.ty(at)) {
                Some(crate::ast::Ty::Ref(None, _, _)) => true,
                Some(crate::ast::Ty::Ref(Some(l), _, _)) => self.pf.text(*l) == "'_",
                _ => false,
            };
            let unannotated = p.ty.is_none() || matches!(p.ty.map(|at| self.ast.ty(at)), Some(crate::ast::Ty::Infer));
            // a written parameter type annotates the parameter (named lifetimes; elided ones
            // stay free)
            if let Some(at) = p.ty {
                if ast_ty_has_ref(self.ast, at, 0) {
                    self.ascribe_local(a, at);
                }
            }
            if unannotated {
                self.out.infer_params.push(a);
            }
            if !ret_has_ref && (annotated_hr || (unannotated && hr_bound.get(k).copied().unwrap_or(false))) && matches!(self.tx.kind(t), TyKind::Ref(..)) {
                self.out.hr_args.push(a);
                let unwritten_ret = matches!(&self.ast.expr(ExprId(ce)).kind, ExprKind::Closure(row) if self.ast.row(*row).ret.is_none());
                if unwritten_ret && self.hr_ret_ties.get(&ce) == Some(&k) {
                    self.out.hr_ret = Some(a);
                }
            }
            self.schedule(a);
            binds.push((p.pat, a));
        }
        for (p, a) in binds {
            self.bind_param(p, a);
        }
        self.push_scope(true);
        let sp = self.set_span(body);
        self.into_coerced(Place::local(0), body, ret);
        self.span = sp;
        self.pop_scope();
        self.pop_scope();
        self.term(TermKind::Return);
    }

    // ------------------------------------------------------------------ blocks / statements

    pub(super) fn block_into(&mut self, dest: Place, b: BlockId, target: Option<TyId>) {
        crate::preempt::point();
        let ast = self.ast;
        let blk = ast.block(b);
        // no expected type for the tail (a statement `{src};`, `let x = {src};`): the value
        // moves as written, no coercion reborrow (rustc coerces only to an expectation)
        let expect = std::mem::replace(&mut self.tail_expect, true);
        let si = self.push_scope(false);
        let n = blk.stmts.len();
        let mut tail = None;
        for (i, s) in ast.list(blk.stmts).iter().enumerate() {
            let s = match *s {
                Stmt::Attrs(r) => { let a = ast.row(r); Stmt::Expr(a.expr, a.semi) }
                s => s,
            };
            if i + 1 == n {
                if let Stmt::Expr(e, false) = s {
                    tail = Some(e);
                    break;
                }
            }
            self.stmt(&s, si);
        }
        match tail {
            Some(e) => {
                let sp = self.set_span(e);
                if self.edition >= 2024 {
                    // the tail is a temporary scope of its own, inside the block's locals
                    self.push_scope(true);
                    self.tail_into(dest, e, target, expect);
                    self.pop_scope();
                } else {
                    // its temporaries live in the enclosing temporary scope (outlive the locals)
                    self.tail_into(dest, e, target, expect);
                }
                self.span = sp;
            }
            None => {
                // a block without tail is `()` (or diverges)
                let t = self.place_ty(&dest);
                if matches!(self.tx.kind(t), TyKind::Tuple(ref v) if v.is_empty()) {
                    self.assign(dest, Rvalue::Aggregate(AggKind::Tuple, Vec::new()));
                }
            }
        }
        let bl = ast.block(b);
        let old = self.span;
        self.span = Span { lo: bl.hi.saturating_sub(1), hi: bl.hi };
        self.pop_scope();
        self.span = old;
    }

    fn tail_into(&mut self, dest: Place, e: ExprId, target: Option<TyId>, expect: bool) {
        let is_block = matches!(self.ast.expr(strip_paren(self.ast, e)).kind, ExprKind::Block(..) | ExprKind::Unsafe(_));
        if expect || target.is_some() || (!is_block && !self.is_place_expr(e)) {
            self.into_maybe_coerced(dest, e, target);
        } else {
            self.tail_expect = false;
            self.into(dest, e);
            self.tail_expect = true;
        }
    }

    pub(super) fn into_maybe_coerced(&mut self, dest: Place, e: ExprId, target: Option<TyId>) {
        // a block's tail is a coercion site of the block's expected type (a `&mut` place is
        // reborrowed, not moved, when that type is known)
        let t = match target {
            Some(t) => t,
            None => self.place_ty(&dest),
        };
        self.into_coerced(dest, e, t)
    }

    pub(super) fn stmt(&mut self, s: &Stmt, block_scope: usize) {
        crate::preempt::point();
        match s {
            Stmt::Let(r) => {
                let crate::ast::LetStmt { pat, init, else_, ty, .. } = self.ast.row(*r);
                let sp = match init {
                    Some(e) => self.set_span(*e),
                    None => self.span,
                };
                self.let_stmt(*pat, *init, *else_, *ty, block_scope);
                self.span = sp;
            }
            Stmt::Expr(e, _) => {
                let sp = self.set_span(*e);
                self.push_scope(true);
                let t = self.ty(*e);
                if self.is_place_expr(*e) {
                    // `x;` / `s.f;`: a place in value context is read (moved) and dropped
                    let p = self.as_place(*e, false);
                    let op = self.operand_of(p);
                    if let Operand::Move(_) = op {
                        let tmp = self.temp(t);
                        self.assign(Place::local(tmp), Rvalue::Use(op));
                    } else {
                        let tmp = self.temp(t);
                        self.assign(Place::local(tmp), Rvalue::Use(op));
                    }
                } else {
                    let tmp = self.temp(t);
                    // a statement's value has no expected type (a block's tail moves as written)
                    self.tail_expect = !matches!(self.ast.expr(strip_paren(self.ast, *e)).kind, ExprKind::Block(..) | ExprKind::Unsafe(_));
                    self.into(Place::local(tmp), *e);
                    self.tail_expect = true;
                }
                self.pop_scope();
                self.span = sp;
            }
            Stmt::Item(_) | Stmt::Empty => {}
            Stmt::Attrs(r) => { let a = *self.ast.row(*r); self.stmt(&Stmt::Expr(a.expr, a.semi), block_scope) }
        }
    }

    /// Typeck locals bound by a pattern get BIR locals (StorageLive, scheduled in the current
    /// scope).
    pub(super) fn declare_bindings(&mut self, p: PatId) {
        let si = self.scopes.len().saturating_sub(1);
        self.declare_bindings_in(p, si);
    }

    pub(super) fn declare_bindings_in(&mut self, p: PatId, si: usize) {
        let mut lis = Vec::new();
        self.pat_bindings(p, &mut lis);
        for (pid, li) in lis {
            if self.locals.contains_key(&li) {
                continue;
            }
            let bl = &self.b.locals[li as usize];
            let (mutable, by_ref) = match self.ast.pat(pid) {
                Pat::Ident { mutbl, by_ref, .. } => (*mutbl, *by_ref || self.b.pat_bind_ref.contains_key(&pid.0)),
                _ => (bl.mutable, false),
            };
            let name = self.tx.prog.syms.str(bl.name).to_string();
            let old = self.span;
            if let Pat::Ident { name: nm, .. } = self.ast.pat(pid) {
                self.span = Span { lo: nm.lo, hi: nm.hi };
            }
            let l = self.new_local(bl.ty, LocalKind::User, mutable, Some(name));
            self.out.locals[l as usize].is_ref_binding = by_ref;
            self.out.locals[l as usize].can_be_made_mutable = !by_ref;
            self.push(StmtKind::StorageLive(l));
            self.schedule_in(l, si);
            self.span = old;
            self.locals.insert(li, Place::local(l));
        }
    }

    /// (binding pattern, typeck local) of every binding in `p` (an or-pattern's first
    /// alternative names them all).
    pub(super) fn pat_bindings(&self, p: PatId, out: &mut Vec<(PatId, u32)>) {
        match self.ast.pat(p) {
            Pat::Ident { sub, .. } => {
                if !self.b.pat_res.contains_key(&p.0) {
                    if let Some(&li) = self.b.pat_local.get(&p.0) {
                        out.push((p, li));
                    }
                }
                if let Some(s) = sub {
                    self.pat_bindings(*s, out);
                }
            }
            Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) => {
                for x in self.ast.list(*v) {
                    self.pat_bindings(*x, out);
                }
            }
            Pat::Struct(_, fs, _) => {
                for f in self.ast.list(*fs) {
                    self.pat_bindings(f.pat, out);
                }
            }
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_bindings(*x, out),
            Pat::Or(alts) => {
                if let Some(a) = self.ast.list(*alts).first() {
                    self.pat_bindings(*a, out);
                }
                // the other alternatives bind the same typeck locals under other pattern ids
                for a in self.ast.list(*alts).iter().skip(1) {
                    let mut more = Vec::new();
                    self.pat_bindings(*a, &mut more);
                    for m in more {
                        if !out.iter().any(|x| x.1 == m.1) {
                            out.push(m);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn collect_extending(&self, e: ExprId, out: &mut Vec<u32>) {
        let ast = self.ast;
        match &ast.expr(e).kind {
            ExprKind::AddrOf(_, _, x) => {
                let x = strip_paren(ast, *x);
                out.push(x.0);
                self.extend_place_operands(x, out);
                self.collect_extending(x, out);
            }
            ExprKind::Paren(x) | ExprKind::Cast(x, _) => self.collect_extending(*x, out),
            ExprKind::Tuple(v) | ExprKind::Array(v) => {
                for x in self.ast.list(*v) {
                    self.collect_extending(*x, out);
                }
            }
            ExprKind::Struct(row) => {
                for f in self.ast.list(self.ast.row(*row).fields) {
                    self.collect_extending(f.expr, out);
                }
            }
            ExprKind::Call(f, args) => {
                // tuple struct / variant constructors extend their operands like literals
                let f = strip_paren(ast, *f);
                if matches!(self.b.res.get(&f.0), Some(Res::Def(d)) if matches!(self.tx.prog.def(*d).kind, DefKind::Variant | DefKind::Struct)) {
                    for x in self.ast.list(*args) {
                        self.collect_extending(*x, out);
                    }
                }
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => {
                if let Some(Stmt::Expr(x, false)) = ast.stmts(*b).last() {
                    self.collect_extending(*x, out);
                }
            }
            // rustc record_rvalue_scope_if_borrow_expr: through if / else and match arms too
            ExprKind::If(_, then, els) => {
                if let Some(Stmt::Expr(x, false)) = ast.stmts(*then).last() {
                    self.collect_extending(*x, out);
                }
                if let Some(e2) = els.get() {
                    self.collect_extending(e2, out);
                }
            }
            ExprKind::Match(_, arms) => {
                for a in self.ast.list(*arms) {
                    self.collect_extending(a.body, out);
                }
            }
            _ => {}
        }
    }

    pub(super) fn extend_place_operands(&self, e: ExprId, out: &mut Vec<u32>) {
        let ast = self.ast;
        let base = match &ast.expr(e).kind {
            ExprKind::Index(b, _) | ExprKind::Field(b, _) | ExprKind::TupleField(b, _) | ExprKind::Unary(UnOp::Deref, b) | ExprKind::AddrOf(_, _, b) => *b,
            _ => return,
        };
        let b = strip_paren(ast, base);
        out.push(b.0);
        self.extend_place_operands(b, out);
    }

    pub(super) fn let_stmt(&mut self, pat: PatId, init: Option<ExprId>, else_: Option<BlockId>, ann: Option<crate::ast::TyId>, block_scope: usize) {
        let ast = self.ast;
        let t = self.b.pty(pat);
        let annotated = ann.is_some();
        if let Some(at) = ann {
            let sp = ty_first_span(ast, at, 0).or_else(|| init.map(|e| { let x = ast.expr(e); Span { lo: x.lo, hi: x.hi } })).unwrap_or(self.span);
            self.out.wf_annots.push((at, t, sp));
        }
        let init = match init {
            None => {
                self.declare_bindings(pat);
                if let Some(at) = ann {
                    self.ascribe_pat_bindings(pat, at, t);
                }
                return;
            }
            Some(e) => e,
        };
        // the statement's temporary scope
        let stmt_scope = self.push_scope(true);
        let _ = stmt_scope;
        let mut ext = Vec::new();
        self.collect_extending(init, &mut ext);
        // `let ref x = rvalue;` extends the initializer itself
        let binds_ref = self.pat_has_ref_binding(pat);
        if binds_ref && !self.is_place_expr(init) {
            ext.push(strip_paren(ast, init).0);
        }
        let mark = self.extend.len();
        for x in &ext {
            self.extend.push((*x, Some(block_scope)));
        }
        match (ast.pat(pat), else_) {
            (Pat::Ident { by_ref: false, sub: None, .. }, None) if !self.b.pat_res.contains_key(&pat.0) && !self.b.pat_bind_ref.contains_key(&pat.0) => {
                // declared in the block's scope before the initializer runs (rustc: storage-live
                // first, the value built in place)
                self.declare_bindings_in(pat, block_scope);
                let li = self.b.pat_local.get(&pat.0).copied();
                match li.and_then(|li| self.locals.get(&li).cloned()) {
                    Some(dest) => {
                        let reborrow = annotated;
                        self.into_coerced_rb(dest.clone(), init, t, reborrow);
                        if let (Some(at), true) = (ann, dest.proj.is_empty()) {
                            self.ascribe_local(dest.local, at);
                        }
                        self.push(StmtKind::FakeRead(FakeReadCause::ForLet, Place::local(self.locals[&li.unwrap()].local)));
                    }
                    None => {
                        let tmp = self.temp(t);
                        self.into(Place::local(tmp), init);
                    }
                }
            }
            (_, None) => {
                // declare the bindings in the block's scope
                self.declare_bindings_in(pat, block_scope);
                if matches!(ast.pat(pat), Pat::Wild) && self.is_place_expr(init) {
                    // `let _ = place;`: evaluates the place, neither reads nor moves it
                    let p = self.as_place(init, false);
                    self.push(StmtKind::PlaceMention(p));
                } else if self.is_place_expr(init) && !self.b.coerce.contains_key(&init.0) {
                    let mutbl = self.pat_binds_mut_ref(pat);
                    let p = self.as_place(init, mutbl);
                    self.push(StmtKind::PlaceMention(p.clone()));
                    if let Some(at) = ann {
                        self.ascribe_place(&p, at);
                    }
                    self.scrut_span = Some({ let xe = ast.expr(init); Span { lo: xe.lo, hi: xe.hi } });
                    self.bind_pat(pat, p, t, false);
                    self.scrut_span = None;
                    if let Some(at) = ann {
                        self.ascribe_pat_bindings(pat, at, t);
                    }
                } else {
                    let tmp = self.temp_for(t, Some(strip_paren(ast, init)));
                    self.into_coerced_rb(Place::local(tmp), init, t, annotated);
                    if let Some(at) = ann {
                        self.ascribe_place(&Place::local(tmp), at);
                    }
                    if matches!(ast.pat(pat), Pat::Wild) {
                        // rustc place_into_pattern: `let _ = value;` mentions the value's temp
                        self.push(StmtKind::PlaceMention(Place::local(tmp)));
                    }
                    self.scrut_span = Some({ let xe = ast.expr(init); Span { lo: xe.lo, hi: xe.hi } });
                    self.bind_pat(pat, Place::local(tmp), t, false);
                    self.scrut_span = None;
                    if let Some(at) = ann {
                        self.ascribe_pat_bindings(pat, at, t);
                    }
                }
            }
            (_, Some(eb)) => {
                self.declare_bindings_in(pat, block_scope);
                let p = if self.is_place_expr(init) && !self.b.coerce.contains_key(&init.0) {
                    let mutbl = self.pat_binds_mut_ref(pat);
                    self.as_place(init, mutbl)
                } else {
                    let tmp = self.temp_for(t, Some(strip_paren(ast, init)));
                    self.into_coerced_rb(Place::local(tmp), init, t, annotated);
                    if let Some(at) = ann {
                        self.ascribe_place(&Place::local(tmp), at);
                    }
                    Place::local(tmp)
                };
                self.push(StmtKind::PlaceMention(p.clone()));
                let fail = self.new_block();
                self.test_pat(pat, p.clone(), t, fail);
                let ok = self.cur;
                self.switch_to(fail);
                // the else block runs after the statement's temporaries end
                let depth = self.scopes.len();
                self.exit_to(depth - 1);
                let unit = self.tx.unit();
                let tmp = self.new_local(unit, LocalKind::Temp, true, None);
                self.block_into(Place::local(tmp), eb, None);
                self.term(TermKind::Unreachable);
                self.switch_to(ok);
                self.scrut_span = Some({ let xe = ast.expr(init); Span { lo: xe.lo, hi: xe.hi } });
                    self.bind_pat(pat, p, t, false);
                    self.scrut_span = None;
            }
        }
        self.extend.truncate(mark);
        self.pop_scope();
    }

    /// rustc user_type_annotations: the index of the written type `at` (erased type `ty`).
    pub(super) fn user_ty_index(&mut self, at: UserAnn, ty: TyId) -> u32 {
        if let Some(i) = self.out.user_tys.iter().position(|x| x.0 == at && x.1 == ty) {
            return i as u32;
        }
        self.out.user_tys.push((at, ty));
        self.out.user_tys.len() as u32 - 1
    }

    /// A whole local whose type is exactly the annotation (rustc LocalDecl.user_ty).
    pub(super) fn ascribe_local(&mut self, l: Local, at: crate::ast::TyId) {
        let ty = self.out.locals[l as usize].ty;
        let ann = self.user_ty_index(UserAnn::Ty(at), ty);
        self.out.local_user_tys.push(LocalUserTy { local: l, ann, projs: Vec::new(), by_ref: false });
    }

    /// `let PAT: T = place`: the place's type is a subtype of the annotation (rustc
    /// AscribeUserType, Covariant).
    pub(super) fn ascribe_place(&mut self, p: &Place, at: crate::ast::TyId) {
        let ty = self.place_ty(p);
        let ann = self.user_ty_index(UserAnn::Ty(at), ty);
        self.out.ascriptions.push(Ascription { place: p.clone(), ann, projs: Vec::new() });
    }

    pub(super) fn pat_has_ref_binding(&self, p: PatId) -> bool {
        let mut v = Vec::new();
        self.pat_bindings(p, &mut v);
        v.iter().any(|(pid, _)| matches!(self.ast.pat(*pid), Pat::Ident { by_ref: true, .. }) || self.b.pat_bind_ref.contains_key(&pid.0))
    }

    pub(super) fn pat_binds_mut_ref(&self, p: PatId) -> bool {
        let mut v = Vec::new();
        self.pat_bindings(p, &mut v);
        v.iter().any(|(pid, _)| matches!(self.ast.pat(*pid), Pat::Ident { by_ref: true, mutbl: true, .. }) || self.b.pat_bind_ref.get(&pid.0) == Some(&2))
    }

    // ------------------------------------------------------------------ places

    pub(super) fn local_place(&mut self, li: u32) -> Place {
        match self.locals.get(&li) {
            Some(p) => p.clone(),
            None => {
                if self.closure.is_some() {
                    Place { local: VBASE + li, proj: Vec::new() }
                } else {
                    unsup!(self, "local used before its binding");
                    let t = self.b.locals[li as usize].ty;
                    let l = self.new_local(t, LocalKind::User, true, None);
                    self.locals.insert(li, Place::local(l));
                    Place::local(l)
                }
            }
        }
    }

    /// The deref of `base` (type `bt`) at autoderef step (key, k): overloaded through
    /// Deref::deref / DerefMut::deref_mut (in a mutable context), else builtin.
    pub(super) fn deref_step(&mut self, key: u32, k: usize, base: Place, bt: TyId, mutbl: bool) -> Place {
        if self.tx.box_inner(bt).is_some() {
            // Box derefs are builtin (a place of its own, movable)
            return base.deref();
        }
        if let Some((m, args)) = self.b.ov_derefs.get(&(key, k)).cloned() {
            return self.overloaded_deref(m, self.tx.list(args).to_vec(), base, bt, mutbl);
        }
        base.deref()
    }

    /// The type after autoderef step (key, k) of a value of type `t` (no code).
    pub(super) fn deref_target_ty(&self, key: u32, k: usize, t: TyId) -> TyId {
        if self.tx.box_inner(t).is_none() {
            if let Some((m, _)) = self.b.ov_derefs.get(&(key, k)) {
                let tx = self.tx;
                let td = tx.prog.def(*m).parent;
                let tsym = tx.prog.syms.get("Target").unwrap_or(u32::MAX);
                return match tx.tcx.trait_assoc(tx.prog, td, tsym) {
                    Some(ad) => {
                        let proj = tx.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: tx.tcx.tys.mk_list(&[t]), regions: crate::types::RegionArgsId::EMPTY }));
                        tx.normalize(proj)
                    }
                    None => t,
                };
            }
        }
        self.tx.builtin_deref(t).unwrap_or(t)
    }

    pub(super) fn overloaded_deref(&mut self, m: DefId, args: Vec<TyId>, base: Place, bt: TyId, mutbl: bool) -> Place {
        let mut def = m;
        let mut is_mut = false;
        if mutbl {
            if let Some(td) = self.tx.lang("ops::DerefMut") {
                if self.tx.evaluate_trait(td, bt, &[]).must_apply_modulo_regions() {
                    for &it in &self.tx.prog.traits[self.tx.prog.def(td).sub as usize] {
                        if self.tx.prog.name(it) == "deref_mut" {
                            def = it;
                            is_mut = true;
                        }
                    }
                }
            }
        }
        let target = {
            let tx = self.tx;
            let td = tx.prog.def(m).parent;
            let tsym = tx.prog.syms.get("Target").unwrap_or(u32::MAX);
            match tx.tcx.trait_assoc(tx.prog, td, tsym) {
                Some(ad) => {
                    let proj = tx.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: tx.tcx.tys.mk_list(&[bt]), regions: crate::types::RegionArgsId::EMPTY }));
                    tx.normalize(proj)
                }
                None => tx.tcx.tys.error,
            }
        };
        let (d, a) = self.tx.resolve_trait_method(def, &args);
        let self_t = self.tx.ref_ty(is_mut, bt);
        let r = self.rv();
        let arg = self.temp(self_t);
        let bk = if is_mut { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
        self.assign(Place::local(arg), Rvalue::Ref(r, bk, base));
        let ret_t = self.tx.ref_ty(is_mut, target);
        let res = self.temp(ret_t);
        self.call(Callee::Overloaded(d, a), vec![Operand::Move(Place::local(arg))], Place::local(res));
        self.out.ov_results.push((res, bt, false));
        Place::local(res).deref()
    }

    /// An ADT value built at the last statement from a path with explicit generic arguments
    /// (or through `Self`): its user type (rustc AggregateKind::Adt's user_ty).
    fn note_agg_targs(&mut self, e: ExprId) {
        let p = match &self.ast.expr(strip_paren(self.ast, e)).kind {
            ExprKind::Path(p) => p,
            ExprKind::Struct(row) => &self.ast.row(*row).path,
            _ => return,
        };
        let v = Self::path_args(self.ast, p);
        let si = self.out.blocks[self.cur as usize].stmts.iter().rposition(|s| matches!(&s.kind, StmtKind::Assign(_, Rvalue::Aggregate(AggKind::Adt(..), _))));
        let Some(si) = si else { return };
        let t = match &self.out.blocks[self.cur as usize].stmts[si].kind {
            StmtKind::Assign(_, Rvalue::Aggregate(AggKind::Adt(t, _), _)) => *t,
            _ => return,
        };
        let src = if p.no_qself() && p.segs.len() == 1 && self.ast.segs(p.segs)[0].kind() == crate::ast::SegKind::SelfType {
            UserAnn::SelfTy
        } else if !v.is_empty() {
            UserAnn::Path(v)
        } else {
            return;
        };
        let ann = self.user_ty_index(src, t);
        self.out.rvalue_user_tys.push(((self.cur, si as u32), ann));
    }

    fn note_targs(&mut self) {
        let u = std::mem::take(&mut self.call_user_ty);
        if !u.is_empty() {
            self.out.call_user_tys.push((self.cur, u));
        }
        if let Some(v) = self.call_value.take() {
            self.out.fn_values.push((self.cur, v));
        }
    }

    /// The generic arguments of an ADT path (`S::<..>`, `E::V::<..>` or `E::<..>::V`):
    /// lifetimes and types in order; empty when there are none or a const argument.
    pub(super) fn path_args(ast: &Ast, p: &crate::ast::Path) -> Vec<PathArg> {
        let conv = |ga: Option<&crate::ast::GenericArgs>| -> Vec<PathArg> {
            match ga {
                Some(crate::ast::GenericArgs::Angle(v)) if !ast.list(*v).iter().any(|a| matches!(a, crate::ast::GenericArg::Const(_))) => ast.list(*v).iter().filter_map(|a| match a {
                    crate::ast::GenericArg::Type(t) => Some(PathArg::Type(*t)),
                    crate::ast::GenericArg::Lifetime(l) => Some(PathArg::Lifetime(*l)),
                    _ => None,
                }).collect(),
                _ => Vec::new(),
            }
        };
        let n = p.segs.len();
        let segs = ast.segs(p.segs);
        let mut v = conv(segs.last().and_then(|s| ast.gargs(s.args())));
        if v.is_empty() && n >= 2 {
            v = conv(ast.gargs(segs[n - 2].args()));
        }
        v
    }

    /// the type arguments written on a path's last segment / a method's turbofish
    /// The user type written on a path to a fn (rustc UserType::TypeOf): its self type and
    /// explicit type arguments.
    fn path_user_ty(ast: &Ast, p: &crate::ast::Path) -> PathUserTy {
        let n = p.segs.len();
        let self_ann = if let Some(q) = ast.qself(p) {
            Some(SelfAnn::Ty(q.ty))
        } else if n >= 2 {
            let v = Self::path_args(ast, &crate::ast::Path { segs: p.segs.prefix(n - 1).without_qself(), lo: p.lo });
            if v.is_empty() { None } else { Some(SelfAnn::Args(v)) }
        } else {
            None
        };
        let trait_targs = match ast.qself(p).and_then(|q| q.trait_path.as_ref()) {
            Some(tp) => Self::turbofish(ast, ast.segs(tp.segs).last().and_then(|s| ast.gargs(s.args()))),
            None => Vec::new(),
        };
        let trait_lts: Vec<crate::ast::Ident> = match ast.qself(p).and_then(|q| q.trait_path.as_ref()).and_then(|tp| ast.segs(tp.segs).last().and_then(|s| ast.gargs(s.args()))) {
            Some(crate::ast::GenericArgs::Angle(v)) => ast.list(*v).iter().filter_map(|a| if let crate::ast::GenericArg::Lifetime(l) = a { Some(*l) } else { None }).collect(),
            _ => Vec::new(),
        };
        PathUserTy { self_ann, trait_lts, trait_targs, targs: Self::turbofish(ast, ast.segs(p.segs).last().and_then(|s| ast.gargs(s.args()))) }
    }

    fn turbofish(ast: &Ast, ga: Option<&crate::ast::GenericArgs>) -> Vec<crate::ast::TyId> {
        match ga {
            Some(crate::ast::GenericArgs::Angle(v)) if !ast.list(*v).iter().any(|a| matches!(a, crate::ast::GenericArg::Const(_))) => ast.list(*v).iter().filter_map(|a| match a {
                crate::ast::GenericArg::Type(t) => Some(*t),
                _ => None,
            }).collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn call(&mut self, callee: Callee, args: Vec<Operand>, dest: Place) {
        let next = self.new_block();
        let arg_spans = std::mem::take(&mut self.arg_spans);
        self.note_targs();
        let dl = if dest.proj.is_empty() { Some(dest.local) } else { None };
        self.term(TermKind::Call { callee, args, dest, target: Some(next), sig: Box::new(CallSig::default()), arg_spans });
        self.switch_to(next);
        if let Some(l) = dl {
            self.filled(l);
        }
    }

    pub(super) fn call_diverging(&mut self, callee: Callee, args: Vec<Operand>, dest: Place, diverges: bool) {
        if diverges {
            let arg_spans = std::mem::take(&mut self.arg_spans);
            self.note_targs();
            self.term(TermKind::Call { callee, args, dest, target: None, sig: Box::new(CallSig::default()), arg_spans });
            self.dead();
        } else {
            self.call(callee, args, dest);
        }
    }

    /// A call through a static (`FOO(&n)` with `static FOO: &(dyn Fn(&'static u8) + Sync)`):
    /// its declared type gives the parameters' named regions.
    fn note_static_callee(&mut self, f: ExprId) {
        let mut e = strip_paren(self.ast, f);
        while let ExprKind::Unary(UnOp::Deref, x) = &self.ast.expr(e).kind {
            e = strip_paren(self.ast, *x);
        }
        if let ExprKind::Path(_) = &self.ast.expr(e).kind {
            if let Some(Res::Def(d)) = self.b.res.get(&e.0).copied() {
                if self.tx.prog.def(d).kind == DefKind::Static {
                    self.out.static_callees.push((self.cur, d));
                }
            }
        }
    }

    /// The builtin index expressions in `e`'s place chain (through parens, fields, derefs and
    /// index bases) are inner indexes of the index whose base `e` is.
    fn mark_inner_indexes(&mut self, mut e: ExprId) {
        loop {
            match &self.ast.expr(e).kind {
                ExprKind::Paren(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Unary(UnOp::Deref, x) => e = *x,
                ExprKind::Index(a, _) => {
                    self.inner_index.insert(e.0);
                    e = *a;
                }
                _ => return,
            }
        }
    }

    /// rustc add_fake_borrows_of_base, for an inner (not outermost) index of a slice: shallow
    /// borrows of the places the base's pointer derefs go through (up to the previous slice
    /// index), read by the outermost index after its index expression, so that it cannot change
    /// what was bounds-checked (E0510 "in indexing expression").
    fn index_fake_borrows(&mut self, base: &Place) {
        if !matches!(self.tx.kind(self.place_ty(base)), TyKind::Slice(_)) {
            return;
        }
        let mut qs: Vec<Place> = Vec::new();
        for k in (0..base.proj.len()).rev() {
            let q = Place { local: base.local, proj: base.proj[..k].to_vec() };
            match base.proj[k] {
                Proj::Deref => qs.push(q),
                Proj::Index(_) if matches!(self.tx.kind(self.place_ty(&q)), TyKind::Slice(_)) => break,
                _ => {}
            }
        }
        for q in qs {
            let qt = self.place_ty(&q);
            let fr = self.rv();
            let rt = self.tx.ref_ty(false, qt);
            let l = self.new_local(rt, LocalKind::Temp, true, None);
            self.push(StmtKind::StorageLive(l));
            self.assign(Place::local(l), Rvalue::Ref(fr, BorrowKind::Shallow, q));
            self.out.index_fakes.push(l);
            self.index_fake_temps.push(l);
        }
    }

    pub fn as_place(&mut self, e: ExprId, mutbl: bool) -> Place {
        let ast = self.ast;
        let ex = ast.expr(e);
        let t = self.ty(e);
        let sp = self.set_span(e);
        let r = match &ex.kind {
            ExprKind::Paren(x) => self.as_place(*x, mutbl),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Local(li)) => self.local_place(li),
                // Named constants are values, even when a projection needs a place.
                // rustc_mir_build expr/as_place.rs lowers NamedConst through as_temp:
                // each use owns its temporary and may move fields out of it. Promotion
                // is a separate decision at a borrow, not an implicit shared reference.
                Some(Res::Def(d)) if matches!(self.tx.prog.def(d).kind, DefKind::Const | DefKind::AssocConst) => self.rvalue_place(e, t),
                Some(Res::Def(d)) if matches!(self.tx.prog.def(d).kind, DefKind::Static | DefKind::ForeignStatic) => {
                    // `*&'static STATIC` (`*mut` for `static mut` / foreign statics)
                    let is_mut = match &self.tx.prog.files[self.tx.prog.def(d).file as usize].ast.item(self.tx.prog.def(d).item).kind {
                        ItemKind::Static(m, _, _) => *m,
                        _ => true,
                    };
                    let pt = if is_mut || self.tx.prog.def(d).kind == DefKind::ForeignStatic {
                        self.tx.tcx.tys.intern(TyKind::Ptr(true, t))
                    } else {
                        self.tx.ref_ty(false, t)
                    };
                    let tmp = self.temp(pt);
                    self.assign(Place::local(tmp), Rvalue::Use(self.static_pointer(pt, d)));
                    if self.tx.prog.def(d).kind == DefKind::Static {
                        self.out.static_temps.push((tmp, d));
                    }
                    Place::local(tmp).deref()
                }
                _ => self.rvalue_place(e, t),
            },
            ExprKind::Unary(UnOp::Deref, x) => {
                if self.b.ov_derefs.contains_key(&(e.0, 0)) && self.tx.box_inner(self.ty(*x)).is_none() {
                    let xt = self.ty(*x);
                    let base = self.as_place(*x, mutbl);
                    self.deref_step(e.0, 0, base, xt, mutbl)
                } else {
                    let xt = self.ty(*x);
                    let is_box = self.tx.box_inner(xt).is_some();
                    let base = if is_box || self.is_place_expr(*x) {
                        // writing / mutably borrowing through a `&mut` needs the path to the
                        // reference itself mutable (rustc convert_place_derefs_to_mutable: an
                        // overloaded deref on the way is DerefMut -- `*pin.r = 1`,
                        // `&mut *self.stream` with self: Pin<&mut Self>)
                        let mut_ref = matches!(self.tx.kind(xt), TyKind::Ref(true, _, _));
                        self.as_place(*x, if is_box || mut_ref { mutbl } else { false })
                    } else {
                        // `*f()`: the pointer in a temporary
                        let tmp = self.temp(xt);
                        self.into(Place::local(tmp), *x);
                        Place::local(tmp)
                    };
                    base.deref()
                }
            }
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (derefs, fi) = match self.b.field_idx.get(&e.0) {
                    Some(&(d, f)) => (d as usize, f),
                    None => {
                        unsup!(self, "field without resolution");
                        (0, 0)
                    }
                };
                let mut pl = self.as_place(*x, mutbl);
                let mut pt = self.ty(*x);
                for k in 0..derefs {
                    pl = self.deref_step(e.0, k, pl, pt, mutbl);
                    pt = self.place_ty(&pl);
                }
                // a struct field (variant 0); unions too
                let ft = self.tx.field_ty(pt, 0, fi as u32).unwrap_or(t);
                pl.project(Proj::Field(fi as u32, ft))
            }
            ExprKind::Index(a, i) => {
                let derefs = self.b.index_derefs.get(&e.0).copied().unwrap_or(0);
                // the index is builtin unless typeck chose an Index impl or it is a range
                let builtin = !self.b.ov_index.contains_key(&e.0) && !self.is_range_ty(self.ty(*i));
                let outermost = !self.inner_index.contains(&e.0);
                let mut saved_fakes = if outermost && builtin { Some(std::mem::take(&mut self.index_fake_temps)) } else { None };
                if builtin {
                    self.mark_inner_indexes(*a);
                }
                let mut pl = self.as_place(*a, mutbl);
                let mut pt = self.ty(*a);
                for k in 0..derefs {
                    pl = self.deref_step(e.0, k, pl, pt, mutbl);
                    pt = self.place_ty(&pl);
                }
                if let Some((m, args)) = self.b.ov_index.get(&e.0).cloned() {
                    self.overloaded_index(m, self.tx.list(args).to_vec(), pl, pt, *i, t, mutbl)
                } else if matches!(self.tx.kind(pt), TyKind::Array(..) | TyKind::Slice(_)) && !self.is_range_ty(self.ty(*i)) {
                    // builtin: the index value in a temp
                    let it = self.ty(*i);
                    let tmp = self.temp(it);
                    self.into(Place::local(tmp), *i);
                    // rustc len_of_slice_or_array: an array's bounds check is a fake read of the
                    // array (a use for the move checker: `a[2] = v` after moving out of `a`)
                    if matches!(self.tx.kind(pt), TyKind::Array(..)) {
                        self.push(StmtKind::FakeRead(FakeReadCause::ForIndex, pl.clone()));
                    }
                    if let Some(saved) = saved_fakes.take() {
                        // rustc read_fake_borrows: the outermost index reads its inner indexes' fake borrows
                        for l in std::mem::replace(&mut self.index_fake_temps, saved) {
                            self.push(StmtKind::FakeRead(FakeReadCause::ForIndex, Place::local(l)));
                        }
                    } else if builtin {
                        self.index_fake_borrows(&pl);
                    }
                    pl.project(Proj::Index(tmp))
                } else {
                    if let Some(saved) = saved_fakes.take() {
                        self.index_fake_temps = saved;
                    }
                    // a range index of an array / slice / str without an overloaded impl entry:
                    // a borrow of the base, as Index::index would take
                    let r = self.rv();
                    let bt = self.tx.ref_ty(mutbl, pt);
                    let a0 = self.temp(bt);
                    let bk = if mutbl { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                    self.assign(Place::local(a0), Rvalue::Ref(r, bk, pl));
                    let it = self.ty(*i);
                    let a1 = self.temp(it);
                    self.into(Place::local(a1), *i);
                    let rt = self.tx.ref_ty(mutbl, t);
                    let res = self.temp(rt);
                    self.call(Callee::Indirect, vec![Operand::Move(Place::local(a0)), Operand::Move(Place::local(a1))], Place::local(res));
                    Place::local(res).deref()
                }
            }
            _ => self.rvalue_place(e, t),
        };
        self.span = sp;
        r
    }

    pub(super) fn is_range_ty(&self, t: TyId) -> bool {
        match self.tx.kind(t) {
            TyKind::Adt(d, _, _) => {
                let n = self.tx.prog.name(d);
                n.starts_with("Range") && self.tx.is_std_def(d)
            }
            _ => false,
        }
    }

    pub(super) fn rvalue_place(&mut self, e: ExprId, t: TyId) -> Place {
        let tmp = self.temp_for(t, Some(e));
        self.into(Place::local(tmp), e);
        Place::local(tmp)
    }

    pub(super) fn overloaded_index(&mut self, m: DefId, args: Vec<TyId>, base: Place, bt: TyId, i: ExprId, t: TyId, mutbl: bool) -> Place {
        let mut def = m;
        let mut is_mut = false;
        if mutbl {
            if let Some(td) = self.tx.lang("ops::IndexMut") {
                if self.tx.evaluate_trait(td, args[0], &args[1..]).must_apply_modulo_regions() {
                    for &it in &self.tx.prog.traits[self.tx.prog.def(td).sub as usize] {
                        if self.tx.prog.name(it) == "index_mut" {
                            def = it;
                            is_mut = true;
                        }
                    }
                }
            }
        }
        let (d, a) = self.tx.resolve_trait_method(def, &args);
        let self_t = self.tx.ref_ty(is_mut, bt);
        let r = self.rv();
        // rustc: `*Index{,Mut}::index{,_mut}(&[mut] base, index)`, arguments in order: the
        // receiver is borrowed (not two-phase) before the index is evaluated
        let arg = self.temp(self_t);
        let bk = if is_mut { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
        self.assign(Place::local(arg), Rvalue::Ref(r, bk, base));
        let it = self.ty(i);
        let idx = self.temp(it);
        self.into(Place::local(idx), i);
        let ret_t = self.tx.ref_ty(is_mut, t);
        let res = self.temp(ret_t);
        let iop = self.operand_of(Place::local(idx));
        self.call(Callee::Overloaded(d, a), vec![Operand::Move(Place::local(arg)), iop], Place::local(res));
        self.out.ov_results.push((res, bt, true));
        Place::local(res).deref()
    }

    // ------------------------------------------------------------------ operands

    fn constant(&self, ty: TyId, kind: ConstantKind) -> Operand {
        Operand::Const(Box::new(Constant { ty, kind }))
    }

    fn unknown_constant(&self, ty: TyId) -> Operand {
        self.constant(ty, ConstantKind::Unknown)
    }

    fn bool_constant(&self, value: bool) -> Operand {
        self.constant(self.tx.tcx.tys.bool_, ConstantKind::Scalar(value as u128))
    }

    fn item_constant(&self, ty: TyId, def: DefId, args: Vec<TyId>) -> Operand {
        self.constant(ty, ConstantKind::Item { def, args })
    }

    /// Lower a constant's value/identity at the point where it is known, rather
    /// than asking a later pass to recover it from a diagnostic span.
    fn expr_constant(&self, e: ExprId) -> Operand {
        let e = strip_paren(self.ast, e);
        let ex = self.ast.expr(e);
        let ty = self.ty(e);
        let kind = match &ex.kind {
            ExprKind::Lit(lit) => {
                let text = String::from_utf8_lossy(self.pf.bytes(ex.lo, ex.hi));
                match lit {
                    LitKind::Bool(b) => ConstantKind::Scalar(*b as u128),
                    LitKind::Int | LitKind::Float => match self.tx.kind(ty) {
                        TyKind::Float(p) => {
                            let num = text.replace('_', "");
                            let num = num.strip_suffix("f32").or_else(|| num.strip_suffix("f64")).unwrap_or(&num);
                            let bits = if p == crate::program::Prim::F32 {
                                num.parse::<f32>().ok().map(|v| v.to_bits() as u64)
                            } else {
                                num.parse::<f64>().ok().map(f64::to_bits)
                            };
                            bits.map(ConstantKind::Float).unwrap_or(ConstantKind::Unknown)
                        }
                        _ => crate::const_fold::parse_int_lit(&text).map(ConstantKind::Scalar).unwrap_or(ConstantKind::Unknown),
                    },
                    LitKind::Char => crate::lit::unescape_str(&text).chars().next()
                        .map(|c| ConstantKind::Scalar(c as u128)).unwrap_or(ConstantKind::Unknown),
                    LitKind::Byte => crate::lit::unescape_bytes(&text).first()
                        .map(|b| ConstantKind::Scalar(*b as u128)).unwrap_or(ConstantKind::Unknown),
                    LitKind::Str | LitKind::RawStr | LitKind::ByteStr | LitKind::CStr => {
                        let mut bytes = crate::lit::unescape_bytes(&text);
                        if *lit == LitKind::CStr { bytes.push(0); }
                        ConstantKind::Bytes(bytes.into())
                    }
                }
            }
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Def(def)) => ConstantKind::Item {
                    def,
                    args: self.b.res_args.get(&e.0).map_or_else(Vec::new, |&l| self.tx.list(l).to_vec()),
                },
                Some(Res::ConstParam(index)) => ConstantKind::Param { owner: self.b.def, index, args: self.b.args.to_vec() },
                _ => ConstantKind::Unknown,
            },
            ExprKind::ConstBlock(_) => ConstantKind::Inline { owner: self.b.def, expr: e, args: self.b.args.to_vec() },
            _ => ConstantKind::Unknown,
        };
        self.constant(ty, kind)
    }

    fn static_pointer(&self, ty: TyId, def: DefId) -> Operand {
        let d = self.tx.prog.def(def);
        let f = &self.tx.prog.files[d.file as usize];
        let thread_local = f.ast.list(f.ast.item(d.item).attrs).iter()
            .any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "thread_local");
        self.constant(ty, ConstantKind::StaticPointer { def, thread_local })
    }

    /// An associated const whose type has regions and whose path writes the self type or the
    /// trait's arguments: the value in a temp that carries the user type (rustc relates a
    /// constant's type to its path's user type invariantly).
    fn assoc_const_operand(&mut self, e: ExprId, d: DefId) -> Option<Operand> {
        let t = self.ty(e);
        let ExprKind::Path(p) = &self.ast.expr(e).kind else { return None };
        let l = self.const_use(p, e.0, t, d, None)?;
        Some(Operand::Move(Place::local(l)))
    }

    /// An associated const read through a path with a user type: its value in a temp whose type
    /// is the const's declared type over the path's generic arguments (rustc Constant.user_ty,
    /// related invariantly by relate_type_and_user_type); `matched`: a const pattern's place.
    pub(super) fn const_use(&mut self, p: &crate::ast::Path, key: u32, t: TyId, d: DefId, matched: Option<Place>) -> Option<Local> {
        let user = Self::path_user_ty(self.ast, p);
        if user.is_empty() {
            return None;
        }
        // a trait's const is the selected impl's (rustc proves the trait ref, which relates the
        // written regions to the impl header's)
        let args0 = self.b.res_args.get(&key).map_or_else(Vec::new, |&l| self.tx.list(l).to_vec());
        let (d, args) = self.tx.resolve_trait_method(d, &args0);
        let tmp = self.temp(t);
        self.assign(Place::local(tmp), Rvalue::Use(self.item_constant(t, d, args.clone())));
        let si = self.out.blocks[self.cur as usize].stmts.len().saturating_sub(1) as u32;
        self.out.const_uses.push(ConstUse { at: (self.cur, si), local: tmp, def: d, args, user, matched });
        Some(tmp)
    }

    pub fn as_operand(&mut self, e: ExprId) -> Operand {
        let ast = self.ast;
        let ex = ast.expr(e);
        match &ex.kind {
            ExprKind::Paren(x) => return self.as_operand(*x),
            ExprKind::Lit(_) => return self.expr_constant(e),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Def(d)) if !matches!(self.tx.prog.def(d).kind, DefKind::Static | DefKind::ForeignStatic) => {
                    let k = self.tx.prog.def(d).kind;
                    if k == DefKind::AssocConst {
                        if let Some(o) = self.assoc_const_operand(e, d) {
                            return o;
                        }
                    }
                    if matches!(k, DefKind::Const | DefKind::AssocConst | DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn) {
                        return self.expr_constant(e);
                    }
                }
                Some(Res::ConstParam(_)) => return self.expr_constant(e),
                _ => {}
            },
            _ => {}
        }
        // rustc as_operand: places and rvalues go through a temporary, read at this point
        let t = self.ty(e);
        let sp = self.set_span(e);
        let tmp = self.temp_for(t, Some(e));
        self.into(Place::local(tmp), e);
        self.span = sp;
        self.operand_of(Place::local(tmp))
    }

    /// `e` as an operand at a coercion site with target type `target` (Body.coerce applied;
    /// `reborrow`: a `&mut` place passed where a reference is expected is reborrowed).
    pub(super) fn as_operand_coerced(&mut self, e: ExprId, target: TyId, reborrow: bool) -> Operand {
        let src = self.ty(e);
        match self.b.coerce.get(&e.0).copied() {
            None => {
                if reborrow && self.is_mut_ref(src) && matches!(self.tx.kind(target), TyKind::Ref(..)) && self.is_place_expr(e) {
                    let tm = matches!(self.tx.kind(target), TyKind::Ref(true, _, _));
                    let p = self.as_place(e, false);
                    let r = self.rv();
                    let tt = if tm { src } else { target };
                    let tmp = self.temp(tt);
                    // call arguments coerce with two-phase borrows allowed (rustc
                    // check_argument_types: AllowTwoPhase::Yes)
                    let bk = if tm { BorrowKind::Mut { two_phase: self.args_two_phase } } else { BorrowKind::Shared };
                    self.assign(Place::local(tmp), Rvalue::Ref(r, bk, p.deref()));
                    return Operand::Move(Place::local(tmp));
                }
                self.as_operand(e)
            }
            Some(Coerce::Never) => self.as_operand(e),
            Some(Coerce::RefToRaw(m)) => {
                let base = self.ptr_base(e);
                let inner = self.tx.builtin_deref(src).expect("reference-to-raw adjustment");
                let ptr = self.tx.tcx.tys.intern(TyKind::Ptr(m, inner));
                let tmp = self.temp(ptr);
                self.assign(Place::local(tmp), Rvalue::RawPtr(m, base.deref()));
                // Array-to-element casts retain the element's region slots too.
                let out = self.temp(target);
                self.assign(Place::local(out), Rvalue::Cast(CastKind::Same, Operand::Move(Place::local(tmp)), target));
                Operand::Move(Place::local(out))
            }
            Some(Coerce::MutToConst) => {
                // `&mut T -> &T`: a shared reborrow `&*e`
                let base = self.ptr_base(e);
                if matches!(self.tx.kind(src), TyKind::Ptr(..)) {
                    let tmp = self.temp(target);
                    self.assign(Place::local(tmp), Rvalue::Cast(CastKind::Same, self.operand_of(base), target));
                    return Operand::Move(Place::local(tmp));
                }
                let r = self.rv();
                let tmp = self.temp(target);
                self.assign(Place::local(tmp), Rvalue::Ref(r, BorrowKind::Shared, base.deref()));
                Operand::Move(Place::local(tmp))
            }
            Some(Coerce::DerefRef(n)) => {
                let (m, _inner) = match self.tx.kind(src) {
                    TyKind::Ref(m, i, _) => (m, i),
                    _ => return self.as_operand(e),
                };
                let tm = matches!(self.tx.kind(target), TyKind::Ref(true, _, _)) && m;
                let base = self.ptr_base(e);
                let mut pl = base.deref();
                for k in 0..n {
                    let pt = self.place_ty(&pl);
                    pl = self.deref_step(e.0 | crate::typeck::DEREF_COERCE_KEY, k, pl, pt, tm);
                }
                let r = self.rv();
                let tmp = self.temp(target);
                let bk = if tm { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                self.assign(Place::local(tmp), Rvalue::Ref(r, bk, pl));
                Operand::Move(Place::local(tmp))
            }
            Some(c @ (Coerce::Unsize | Coerce::ToDyn(..) | Coerce::Upcast(..))) => {
                let ck = if c == Coerce::Unsize { CastKind::Unsize } else { CastKind::ToDyn };
                // a reference is reborrowed first (`&mut *x` / `&*x`), then unsized: the source
                // reference stays usable (rustc coerce_unsized after a reborrow adjustment)
                let op = match self.tx.kind(src) {
                    TyKind::Ref(sm, inner, _) => {
                        let base = self.ptr_base(e);
                        // Ref-to-raw coercions first take a raw borrow, then unsize that
                        // pointer. The reference's region is not part of CoerceUnsized.
                        // rustc_hir_typeck/coercion.rs:652; rustc_borrowck/type_check/mod.rs:1233.
                        if let TyKind::Ptr(tm, _) = self.tx.kind(target) {
                            let rt = self.tx.tcx.tys.intern(TyKind::Ptr(tm, inner));
                            let tmp = self.temp(rt);
                            self.assign(Place::local(tmp), Rvalue::RawPtr(tm, base.deref()));
                            let out = self.temp(target);
                            self.assign(Place::local(out), Rvalue::Cast(ck, Operand::Move(Place::local(tmp)), target));
                            return Operand::Move(Place::local(out));
                        }
                        let tm = matches!(self.tx.kind(target), TyKind::Ref(true, _, _)) && sm;
                        let r = self.rv();
                        let rt = self.tx.ref_ty(tm, inner);
                        let t1 = self.temp(rt);
                        let bk = if tm { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                        self.assign(Place::local(t1), Rvalue::Ref(r, bk, base.deref()));
                        Operand::Move(Place::local(t1))
                    }
                    _ => self.as_operand(e),
                };
                let tmp = self.temp(target);
                self.assign(Place::local(tmp), Rvalue::Cast(ck, op, target));
                Operand::Move(Place::local(tmp))
            }
            Some(Coerce::ReifyFn) => {
                let _ = self.as_operand(e);
                // Preserve the fn item identity at the coerced pointer type.
                let mut op = self.expr_constant(e);
                if let Operand::Const(c) = &mut op { c.ty = target; }
                op
            }
            Some(Coerce::ClosureFn | Coerce::UnsafeFn) => {
                let op = self.as_operand(e);
                let tmp = self.temp(target);
                self.assign(Place::local(tmp), Rvalue::Cast(CastKind::Plain, op, target));
                Operand::Move(Place::local(tmp))
            }
        }
    }

    pub(super) fn is_prim(&self, t: TyId) -> bool {
        match self.tx.kind(t) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::Never | TyKind::Error | TyKind::Infer(_) => true,
            _ => false,
        }
    }

    pub(super) fn is_mut_ref(&self, t: TyId) -> bool {
        matches!(self.tx.kind(t), TyKind::Ref(true, _, _))
    }

    /// The place holding the pointer value of `e` (the place itself, or a temp).
    pub(super) fn ptr_base(&mut self, e: ExprId) -> Place {
        if self.is_place_expr(e) {
            self.as_place(e, false)
        } else {
            let t = self.ty(e);
            let tmp = self.temp(t);
            self.into(Place::local(tmp), e);
            Place::local(tmp)
        }
    }

    pub(super) fn into_coerced(&mut self, dest: Place, e: ExprId, target: TyId) {
        self.into_coerced_rb(dest, e, target, true)
    }

    pub(super) fn into_coerced_rb(&mut self, dest: Place, e: ExprId, target: TyId, reborrow: bool) {
        if self.b.coerce.contains_key(&e.0) && !matches!(self.b.coerce.get(&e.0), Some(Coerce::Never)) || (reborrow && self.is_mut_ref(self.ty(e)) && self.is_place_expr(e)) {
            let sp = self.set_span(e);
            let op = self.as_operand_coerced(e, target, reborrow);
            self.assign(dest, Rvalue::Use(op));
            self.span = sp;
        } else {
            // an unannotated `let x = { src };`: the block's tail has no expected type
            if !reborrow && matches!(self.ast.expr(strip_paren(self.ast, e)).kind, ExprKind::Block(..) | ExprKind::Unsafe(_)) {
                self.tail_expect = false;
            }
            self.into(dest, e);
            self.tail_expect = true;
        }
    }

    // ------------------------------------------------------------------ into

    pub fn into(&mut self, dest: Place, e: ExprId) {
        crate::preempt::point();
        let ast = self.ast;
        let ex = ast.expr(e);
        let t = self.ty(e);
        let sp = self.set_span(e);
        let dest_local_flush = if dest.proj.is_empty() { Some(dest.local) } else { None };
        match &ex.kind {
            ExprKind::Paren(x) => self.into(dest, *x),
            ExprKind::Lit(_) => self.assign(dest, Rvalue::Use(self.expr_constant(e))),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Local(_)) => {
                    let p = self.as_place(e, false);
                    let op = self.operand_of(p);
                    self.assign(dest, Rvalue::Use(op));
                }
                Some(Res::Def(d)) => {
                    let k = self.tx.prog.def(d).kind;
                    match k {
                        DefKind::Static | DefKind::ForeignStatic => {
                            let p = self.as_place(e, false);
                            let op = self.operand_of(p);
                            self.assign(dest, Rvalue::Use(op));
                        }
                        DefKind::Struct | DefKind::Variant if !matches!(self.tx.kind(t), TyKind::FnDef(..) | TyKind::FnPtr(..)) => {
                            let vi = if k == DefKind::Variant { self.tx.prog.def(d).sub } else { 0 };
                            self.assign(dest, Rvalue::Aggregate(AggKind::Adt(t, vi), Vec::new()));
                            self.note_agg_targs(e);
                        }
                        // a tuple constructor as a value: its fn item type has the slots of the
                        // ADT it builds (rty skel), set from the path's annotation like an
                        // aggregate of that ADT with no fields yet
                        DefKind::Struct | DefKind::Variant => {
                            let vi = if k == DefKind::Variant { self.tx.prog.def(d).sub } else { 0 };
                            match self.tx.kind(t) {
                                TyKind::FnDef(_, args, _) => {
                                    let ad = if k == DefKind::Variant { self.tx.prog.def(d).parent } else { d };
                                    let at = self.tx.tcx.tys.intern(TyKind::Adt(ad, args, crate::types::RegionArgsId::EMPTY));
                                    self.assign(dest, Rvalue::Aggregate(AggKind::Adt(at, vi), Vec::new()));
                                    self.note_agg_targs(e);
                                }
                                _ => self.assign(dest, Rvalue::Use(self.expr_constant(e))),
                            }
                        }
                        // a fn item as a value: its slots are the callee's generic arguments'
                        // regions; the user type written on the path relates to them (rustc
                        // Constant.user_ty)
                        DefKind::Fn | DefKind::AssocFn if matches!(self.tx.kind(t), TyKind::FnDef(..)) => {
                            let u = match &self.ast.expr(strip_paren(self.ast, e)).kind {
                                ExprKind::Path(p) => Self::path_user_ty(self.ast, p),
                                _ => PathUserTy::default(),
                            };
                            let plain = dest.proj.is_empty();
                            self.assign(dest, Rvalue::Use(self.expr_constant(e)));
                            if plain && !u.is_empty() {
                                let si = self.out.blocks[self.cur as usize].stmts.len() - 1;
                                self.out.fn_anns.push(((self.cur, si as u32), u));
                            }
                        }
                        DefKind::AssocConst => {
                            let op = self.assoc_const_operand(e, d).unwrap_or_else(|| self.expr_constant(e));
                            self.assign(dest, Rvalue::Use(op));
                        }
                        _ => self.assign(dest, Rvalue::Use(self.expr_constant(e))),
                    }
                }
                _ => self.assign(dest, Rvalue::Use(self.expr_constant(e))),
            },
            ExprKind::Unary(UnOp::Deref, _) | ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) => {
                let p = self.as_place(e, false);
                let op = self.operand_of(p);
                self.assign(dest, Rvalue::Use(op));
            }
            ExprKind::Unary(unary, x) => {
                if let Some(m) = self.b.binops.get(&e.0).cloned() {
                    let op = self.as_operand(*x);
                    let (d, a) = self.tx.resolve_trait_method(m.def, self.tx.list(m.args));
                    self.call(Callee::Fn(d, a), vec![op], dest);
                } else {
                    let op = self.as_operand(*x);
                    self.assign(dest, Rvalue::Op(Operation::Unary(*unary), vec![op]));
                }
            }
            ExprKind::Binary(op, a, b2) => self.binary(dest, e, *op, *a, *b2),
            ExprKind::Assign(a, b2) => {
                if crate::lit::is_assignee_pattern(ast, *a) {
                    let bt = self.ty(*b2);
                    let tmp = self.temp(bt);
                    self.into(Place::local(tmp), *b2);
                    self.destructure_assign(*a, Place::local(tmp), bt);
                } else {
                    // rhs first, then the place (rustc: assignment evaluates the RHS first)
                    let at = self.ty(*a);
                    let op = self.as_operand_coerced(*b2, at, true);
                    let p = self.as_place(*a, true);
                    let sp2 = self.set_span(*a);
                    if self.tx.needs_drop(at) {
                        // the old value is dropped (replaced)
                        let next = self.new_block();
                        self.term(TermKind::Drop(p.clone(), next));
                        self.switch_to(next);
                    }
                    self.span = sp2;
                    self.assign(p, Rvalue::Use(op));
                }
                self.assign_unit(dest);
            }
            ExprKind::AssignOp(binary, a, b2) => {
                if let Some(m) = self.b.binops.get(&e.0).cloned() {
                    // OpAssign::op_assign(&mut a, b): lhs place first, two-phase borrow
                    let p = self.as_place(*a, true);
                    let pt = self.place_ty(&p);
                    let r = self.rv();
                    let rt = self.tx.ref_ty(true, pt);
                    let tmp = self.temp(rt);
                    self.assign(Place::local(tmp), Rvalue::Ref(r, BorrowKind::Mut { two_phase: true }, p));
                    let bt = self.tx.list(m.args).get(1).copied().unwrap_or(self.ty(*b2));
                    let op = self.as_operand_coerced(*b2, bt, true);
                    let (d, aa) = self.tx.resolve_trait_method(m.def, self.tx.list(m.args));
                    let unit = self.tx.unit();
                    let res = self.temp(unit);
                    self.call(Callee::Fn(d, aa), vec![Operand::Move(Place::local(tmp)), op], Place::local(res));
                } else {
                    // builtin: rhs first, then the place, read-modify-write
                    let op = self.as_operand(*b2);
                    let p = self.as_place(*a, true);
                    self.assign(p.clone(), Rvalue::Op(Operation::Binary(*binary), vec![Operand::Copy(p), op]));
                }
                self.assign_unit(dest);
            }
            ExprKind::Cast(x, cast_at) => {
                let xt = self.ty(*x);
                // `e as &'static T`: the written type annotates the value (rustc user type of
                // the cast, covariant)
                if dest.proj.is_empty() && ast_ty_has_ref(self.ast, *cast_at, 0) {
                    self.ascribe_place(&dest.clone(), *cast_at);
                }
                match self.b.coerce.get(&x.0) {
                    Some(Coerce::ToDyn(..) | Coerce::Upcast(..) | Coerce::Unsize | Coerce::DerefRef(_) | Coerce::ReifyFn | Coerce::ClosureFn | Coerce::UnsafeFn | Coerce::MutToConst | Coerce::RefToRaw(_)) => {
                        let op = self.as_operand_coerced(*x, t, false);
                        self.assign(dest, Rvalue::Use(op));
                    }
                    _ => {
                        // `&x as *const T`: the borrow becomes a raw pointer
                        let op = self.as_operand(*x);
                        let k = match (self.tx.kind(xt), self.tx.kind(t)) {
                            (TyKind::Ptr(..) | TyKind::FnPtr(..), TyKind::Int(_)) => CastKind::PointerExposeProvenance,
                            (TyKind::Int(_), TyKind::Ptr(..)) => CastKind::PointerWithExposedProvenance,
                            (TyKind::Ref(..), TyKind::Ptr(..)) | (TyKind::Ptr(..), TyKind::Ptr(..)) => CastKind::PtrToPtr,
                            _ if xt == t => CastKind::Same,
                            (TyKind::Adt(..), _) | (TyKind::Tuple(_), _) | (TyKind::Array(..), _) => CastKind::Same,
                            _ => CastKind::Plain,
                        };
                        self.assign(dest, Rvalue::Cast(k, op, t));
                    }
                }
            }
            ExprKind::Block(b, label) => {
                if label.is_some() {
                    let brk = self.new_block();
                    let lbl = label.map(|l| self.pf.text(l).as_bytes().to_vec());
                    self.loops.push(LoopCx { label: lbl, brk, cont: None, dest: Some(dest.clone()), depth: self.scopes.len() });
                    self.block_into(dest, *b, None);
                    self.loops.pop();
                    self.goto(brk);
                    self.switch_to(brk);
                } else {
                    self.block_into(dest, *b, None);
                }
            }
            ExprKind::Unsafe(b) => self.block_into(dest, *b, None),
            ExprKind::ConstBlock(_) => self.assign(dest, Rvalue::Use(self.expr_constant(e))),
            ExprKind::If(c, then, els) => self.if_expr(dest, *c, *then, els.get()),
            ExprKind::Let(p, x) => {
                // a bare `let` condition outside if/while (rare): test into a bool
                let yes = self.new_block();
                let no = self.new_block();
                let join = self.new_block();
                self.cond_let(*p, *x, yes, no);
                self.switch_to(yes);
                self.assign(dest.clone(), Rvalue::Use(self.bool_constant(true)));
                self.goto(join);
                self.switch_to(no);
                self.assign(dest, Rvalue::Use(self.bool_constant(false)));
                self.goto(join);
                self.switch_to(join);
            }
            ExprKind::Match(x, arms) => self.match_expr(dest, *x, ast.list(*arms)),
            ExprKind::While(row) => {
                let crate::ast::WhileRow { cond: c, body, label } = ast.row(*row);
                let head = self.new_block();
                let bodyb = self.new_block();
                let exit = self.new_block();
                self.goto(head);
                self.switch_to(head);
                let fu = self.new_block();
                self.term(TermKind::FalseUnwind(fu));
                self.switch_to(fu);
                let depth = self.scopes.len();
                // the condition's scope covers the body (while-let bindings, scrutinee temps)
                self.push_scope(true);
                let lbl = label.map(|l| self.pf.text(l).as_bytes().to_vec());
                self.loops.push(LoopCx { label: lbl, brk: exit, cont: Some(head), dest: None, depth });
                self.cond(*c, bodyb, exit, true);
                self.switch_to(bodyb);
                let unit = self.tx.unit();
                let tmp = self.new_local(unit, LocalKind::Temp, true, None);
                self.push_scope(true);
                self.block_into(Place::local(tmp), *body, None);
                self.pop_scope();
                self.loops.pop();
                self.pop_scope();
                self.goto(head);
                // the exit edge leaves the condition scope (its drops on the failing edge)
                self.switch_to(exit);
                let _ = depth;
                self.assign_unit(dest);
            }
            ExprKind::Loop(body, label) => {
                let head = self.new_block();
                let exit = self.new_block();
                self.goto(head);
                self.switch_to(head);
                let fu = self.new_block();
                self.term(TermKind::FalseUnwind(fu));
                self.switch_to(fu);
                let lbl = label.map(|l| self.pf.text(l).as_bytes().to_vec());
                self.loops.push(LoopCx { label: lbl, brk: exit, cont: Some(head), dest: Some(dest), depth: self.scopes.len() });
                let unit = self.tx.unit();
                let tmp = self.new_local(unit, LocalKind::Temp, true, None);
                self.push_scope(true);
                self.block_into(Place::local(tmp), *body, None);
                self.pop_scope();
                self.loops.pop();
                self.goto(head);
                self.switch_to(exit);
            }
            ExprKind::For(row) => {
                let crate::ast::ForRow { pat: p, iter: it, body, label } = ast.row(*row);
                self.for_loop(*p, *it, *body, label.map(|l| self.pf.text(l).as_bytes().to_vec()));
                self.assign_unit(dest);
            }
            ExprKind::Break(label, x) => {
                let idx = self.loop_index(label.map(|l| self.pf.text(l).as_bytes().to_vec()));
                match idx {
                    Some(i) => {
                        if let Some(v) = x.get() {
                            match self.loops[i].dest.clone() {
                                Some(d) => {
                                    let dt = self.place_ty(&d);
                                    self.into_coerced(d, v, dt)
                                }
                                None => {
                                    let vt = self.ty(v);
                                    let tmp = self.temp(vt);
                                    self.into(Place::local(tmp), v);
                                }
                            }
                        } else if let Some(d) = self.loops[i].dest.clone() {
                            if self.loops[i].cont.is_some() {
                                self.assign_unit(d);
                            }
                        }
                        let depth = self.loops[i].depth;
                        self.exit_to(depth);
                        let brk = self.loops[i].brk;
                        self.goto(brk);
                    }
                    None => unsup!(self, "break outside a loop"),
                }
                self.dead();
            }
            ExprKind::Continue(label) => {
                let idx = self.loop_index(label.map(|l| self.pf.text(l).as_bytes().to_vec()));
                match idx.and_then(|i| self.loops[i].cont.map(|c| (i, c))) {
                    Some((i, c)) => {
                        let depth = self.loops[i].depth;
                        self.exit_to(depth);
                        self.goto(c);
                    }
                    None => unsup!(self, "continue outside a loop"),
                }
                self.dead();
            }
            ExprKind::Return(x) => {
                let rt = self.ret_ty;
                match &x.get() {
                    Some(v) => self.into_coerced(Place::local(0), *v, rt),
                    None => self.assign_unit(Place::local(0)),
                }
                self.exit_to(0);
                self.term(TermKind::Return);
                self.dead();
            }
            ExprKind::Try(x) => self.try_expr(dest, e, *x),
            ExprKind::Tuple(v) => {
                let tys = match self.tx.kind(t) {
                    TyKind::Tuple(x) => self.tx.tcx.tys.list(x).to_vec(),
                    _ => Vec::new(),
                };
                let mut ops = Vec::new();
                for (i, x) in ast.list(*v).iter().enumerate() {
                    let ft = tys.get(i).copied().unwrap_or(self.ty(*x));
                    ops.push(self.as_operand_coerced(*x, ft, true));
                }
                self.assign(dest, Rvalue::Aggregate(AggKind::Tuple, ops));
            }
            ExprKind::Array(v) => {
                let et = match self.tx.kind(t) {
                    TyKind::Array(e2, _) => e2,
                    _ => t,
                };
                let mut ops = Vec::new();
                for x in ast.list(*v) {
                    // array elements are coercion sites (LUB to the element type)
                    ops.push(self.as_operand_coerced(*x, et, true));
                }
                self.assign(dest, Rvalue::Aggregate(AggKind::Array(et), ops));
            }
            ExprKind::Repeat(x, _) => {
                let et = match self.tx.kind(t) {
                    TyKind::Array(e2, _) => e2,
                    _ => t,
                };
                let op = self.as_operand_coerced(*x, et, false);
                self.assign(dest, Rvalue::Repeat(op));
            }
            ExprKind::Struct(row) => { let crate::ast::StructRow { fields, base, .. } = ast.row(*row); self.struct_lit(dest, e, ast.list(*fields), base.get(), t) }
            ExprKind::Range(a, b2, _) => {
                let mut ops = Vec::new();
                if let Some(a) = a.get() {
                    ops.push(self.as_operand(a));
                }
                if let Some(b2) = b2.get() {
                    ops.push(self.as_operand(b2));
                }
                self.assign(dest, Rvalue::Aggregate(AggKind::Adt(t, 0), ops));
            }
            ExprKind::AddrOf(raw, m, x) => {
                if *raw {
                    let p = self.as_place(*x, *m);
                    self.assign(dest, Rvalue::RawPtr(*m, p));
                } else {
                    let p = self.as_place(*x, *m);
                    let r = self.rv();
                    let bk = if *m { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                    self.assign(dest, Rvalue::Ref(r, bk, p));
                }
            }
            ExprKind::Call(f, args) => self.call_expr(dest, e, *f, ast.list(*args)),
            ExprKind::MethodCall(recv, row) => self.method_call(dest, e, *recv, ast.list(ast.row(*row).args)),
            ExprKind::Closure(_) => self.closure_expr(dest, e),
            ExprKind::Async(..) | ExprKind::Await(_) | ExprKind::Yield(_) => {
                unsup!(self, "async / await");
                self.assign(dest, Rvalue::Use(self.expr_constant(e)));
            }
            ExprKind::Mac(m) => self.mac(dest, e, ast.row(*m)),
            ExprKind::Box(x) => {
                let op = self.as_operand(*x);
                self.assign(dest, Rvalue::Aggregate(AggKind::Tuple, vec![op]));
            }
            ExprKind::Underscore => {}
        }
        if dest_local_flush.is_some() {
            if let Some(l) = dest_local_flush {
                self.filled(l);
            }
        }
        self.span = sp;
    }

    pub(super) fn assign_unit(&mut self, dest: Place) {
        let t = self.place_ty(&dest);
        if matches!(self.tx.kind(t), TyKind::Tuple(ref v) if v.is_empty()) {
            self.assign(dest, Rvalue::Aggregate(AggKind::Tuple, Vec::new()));
        }
    }

    pub(super) fn loop_index(&self, label: Option<Vec<u8>>) -> Option<usize> {
        for i in (0..self.loops.len()).rev() {
            match &label {
                None => {
                    if self.loops[i].cont.is_some() {
                        return Some(i);
                    }
                }
                Some(l) => {
                    if self.loops[i].label.as_ref() == Some(l) {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    pub(super) fn binary(&mut self, dest: Place, e: ExprId, op: BinOp, a: ExprId, b2: ExprId) {
        if let Some(m) = self.b.binops.get(&e.0).cloned() {
            let (d, aa) = self.tx.resolve_trait_method(m.def, self.tx.list(m.args));
            if m.autoref == 1 {
                // comparisons: PartialEq::eq(&a, &b)
                let pa = self.as_place(a, false);
                let ta = self.place_ty(&pa);
                let ra = self.rv();
                let t1 = self.temp(self.tx.ref_ty(false, ta));
                self.assign(Place::local(t1), Rvalue::Ref(ra, BorrowKind::Shared, pa));
                let pb = self.as_place(b2, false);
                let tb = self.place_ty(&pb);
                let rb = self.rv();
                let t2 = self.temp(self.tx.ref_ty(false, tb));
                self.assign(Place::local(t2), Rvalue::Ref(rb, BorrowKind::Shared, pb));
                self.call(Callee::Fn(d, aa), vec![Operand::Move(Place::local(t1)), Operand::Move(Place::local(t2))], dest);
                return;
            }
            let o1 = self.as_operand(a);
            let bt = self.tx.list(m.args).get(1).copied().unwrap_or(self.ty(b2));
            let o2 = self.as_operand_coerced(b2, bt, false);
            self.call(Callee::Fn(d, aa), vec![o1, o2], dest);
            return;
        }
        match op {
            BinOp::And | BinOp::Or => {
                // short circuit: the left operand into the destination, the right one (its own
                // temporary scope) only on one edge
                let rhs = self.new_block();
                let join = self.new_block();
                let short = self.new_block();
                let o1 = self.as_operand(a);
                if op == BinOp::And {
                    self.term(TermKind::Switch(o1, vec![rhs, short]));
                } else {
                    self.term(TermKind::Switch(o1, vec![short, rhs]));
                }
                self.switch_to(short);
                self.assign(dest.clone(), Rvalue::Use(self.bool_constant(op == BinOp::Or)));
                self.goto(join);
                self.switch_to(rhs);
                self.push_scope(true);
                self.into(dest, b2);
                self.pop_scope();
                self.goto(join);
                self.switch_to(join);
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if !self.is_prim(self.ty(a)) => {
                // a comparison typeck left without an operator method (a polymorphic body):
                // PartialEq / PartialOrd take both operands by reference
                let pa = self.as_place(a, false);
                let ta = self.place_ty(&pa);
                let ra = self.rv();
                let t1 = self.temp(self.tx.ref_ty(false, ta));
                self.assign(Place::local(t1), Rvalue::Ref(ra, BorrowKind::Shared, pa));
                let pb = self.as_place(b2, false);
                let tb = self.place_ty(&pb);
                let rb = self.rv();
                let t2 = self.temp(self.tx.ref_ty(false, tb));
                self.assign(Place::local(t2), Rvalue::Ref(rb, BorrowKind::Shared, pb));
                self.assign(dest, Rvalue::Op(Operation::TraitComparison(op), vec![Operand::Move(Place::local(t1)), Operand::Move(Place::local(t2))]));
            }
            _ => {
                let o1 = self.as_operand(a);
                let o2 = self.as_operand(b2);
                self.assign(dest, Rvalue::Op(Operation::Binary(op), vec![o1, o2]));
            }
        }
    }

    // ------------------------------------------------------------------ conditions

    pub(super) fn cond_has_let(&self, c: ExprId) -> bool {
        match &self.ast.expr(c).kind {
            ExprKind::Let(..) => true,
            ExprKind::Binary(BinOp::And, a, b) => self.cond_has_let(*a) || self.cond_has_let(*b),
            ExprKind::Paren(x) => self.cond_has_let(*x),
            _ => false,
        }
    }

    /// Branch on condition `c` (let-chains included): bindings go to the current scope (the
    /// caller's scope for the taken branch).
    pub(super) fn cond(&mut self, c: ExprId, yes: Bb, no: Bb, _in_loop: bool) {
        let ast = self.ast;
        match &ast.expr(c).kind {
            ExprKind::Paren(x) => self.cond(*x, yes, no, _in_loop),
            ExprKind::Let(p, x) => self.cond_let(*p, *x, yes, no),
            ExprKind::Binary(BinOp::And, a, b) if self.cond_has_let(c) && !self.b.binops.contains_key(&c.0) => {
                let mid = self.new_block();
                self.cond(*a, mid, no, _in_loop);
                self.switch_to(mid);
                self.cond(*b, yes, no, _in_loop);
            }
            _ => {
                let sp = self.set_span(c);
                self.push_scope(true);
                let op = self.as_operand(c);
                let t1 = self.new_block();
                let f1 = self.new_block();
                self.term(TermKind::Switch(op, vec![t1, f1]));
                // the condition's temporaries end before either branch
                self.switch_to(t1);
                let i = self.scopes.len() - 1;
                self.emit_scope_drops(i);
                self.goto(yes);
                self.switch_to(f1);
                self.emit_scope_drops(i);
                self.goto(no);
                self.scopes.pop();
                self.span = sp;
            }
        }
    }

    pub(super) fn cond_let(&mut self, p: PatId, x: ExprId, yes: Bb, no: Bb) {
        let sp = self.set_span(x);
        let t = self.ty(x);
        let place = if self.is_place_expr(x) && !self.b.coerce.contains_key(&x.0) {
            let mutbl = self.pat_binds_mut_ref(p);
            self.as_place(x, mutbl)
        } else {
            let tmp = self.temp(t);
            self.into(Place::local(tmp), x);
            Place::local(tmp)
        };
        self.push(StmtKind::PlaceMention(place.clone()));
        self.test_pat(p, place.clone(), t, no);
        // rustc's false edge to the otherwise branch: reachable also after an irrefutable test
        let real = self.new_block();
        self.term(TermKind::FalseEdge(real, no));
        self.switch_to(real);
        self.declare_bindings(p);
        self.bind_pat(p, place, t, false);
        self.goto(yes);
        self.span = sp;
    }

    pub(super) fn if_expr(&mut self, dest: Place, c: ExprId, then: BlockId, els: Option<ExprId>) {
        let yes = self.new_block();
        let no = self.new_block();
        let join = self.new_block();
        let has_let = self.cond_has_let(c);
        // let-conditions: bindings and (edition < 2024) scrutinee temporaries live through the
        // then block; a scope around the whole `if`
        let depth = self.scopes.len();
        if has_let {
            self.push_scope(self.edition >= 2024);
        }
        self.cond(c, yes, no, false);
        self.switch_to(yes);
        // the then / else branches are terminating scopes (rustc resolve_expr(then, true))
        self.push_scope(true);
        self.block_into(dest.clone(), then, None);
        self.pop_scope();
        if has_let {
            self.emit_scope_drops(depth);
        }
        self.goto(join);
        self.switch_to(no);
        if has_let && self.edition >= 2024 {
            // 2024: the scrutinee temporaries end before the else branch
            self.emit_scope_drops(depth);
        }
        match els {
            Some(x) => {
                self.push_scope(true);
                self.into(dest, x);
                self.pop_scope();
            }
            None => self.assign_unit(dest),
        }
        if has_let && self.edition < 2024 {
            self.emit_scope_drops(depth);
        }
        self.goto(join);
        if has_let {
            self.scopes.truncate(depth);
        }
        self.switch_to(join);
    }

    // ------------------------------------------------------------------ calls

    /// Whether an argument at formal position `i` of `d` is a reborrow site: the formal type is
    /// a reference in the signature, or a parameter of the parent impl / trait (known from the
    /// receiver when the argument is checked); the callee's own type parameter is still an
    /// inference variable then, and the `&mut` moves (rustc coerce: target is a ty var).
    fn trait_param_arg(&self, d0: DefId, i: usize) -> bool {
        let prog = self.tx.prog;
        let parent = prog.def(d0).parent;
        if parent == crate::program::NO_DEF || prog.def(parent).kind != DefKind::Trait {
            return false;
        }
        match self.tx.tcx.fn_sig(self.tx.prog, d0).and_then(|s| s.params.get(i).map(|p| (*p, s.n_parent_generics))) {
            Some((p, np)) => matches!(self.tx.kind(p), TyKind::Param(k) if k >= 1 && k < np),
            None => false,
        }
    }

    pub(super) fn reborrow_site(&self, d: DefId, i: usize) -> bool {
        let sig = match self.tx.tcx.fn_sig(self.tx.prog, d) {
            Some(s) => s,
            None => return true,
        };
        let p = match sig.params.get(i) {
            Some(p) => *p,
            None => return true,
        };
        match self.tx.kind(p) {
            TyKind::Param(k) => k < sig.n_parent_generics,
            _ => true,
        }
    }

    /// A call of a fn item (`d0` with the type's generic arguments), written as a path or
    /// held in a value (`callee`).
    fn fn_def_call(&mut self, dest: Place, f: ExprId, args: &[ExprId], d0: DefId, gargs0: Vec<TyId>, callee: Option<Place>, diverges: bool) {
        let ast = self.ast;
        let (d, gargs) = self.tx.resolve_trait_method(d0, &gargs0);
        let sig = self.tx.tcx.fn_item_sig(self.tx.prog, d).cloned();
        let mut ops = Vec::new();
        self.note_hr_closures(d, 0, args);
        for (i, a) in args.iter().enumerate() {
            let pt = match &sig {
                Some(s) if i < s.params.len() => {
                    let x = self.tx.tcx.tys.subst(s.params[i], &gargs);
                    self.tx.normalize(x)
                }
                _ => self.ty(*a),
            };
            // a path call of a trait method: the parameter's type is the trait's own
            // generic (not Self), an inference variable when the argument coerces --
            // rustc does not reborrow there (`Other::handle(value)` moves `value`)
            let rb = self.reborrow_site(d, i) && !self.trait_param_arg(d0, i);
            let saved = std::mem::replace(&mut self.args_two_phase, true);
            ops.push(self.as_operand_coerced(*a, pt, rb));
            self.args_two_phase = saved;
        }
        self.arg_spans = args.iter().map(|a| self.espan(*a)).collect();
        if let (None, ExprKind::Path(p)) = (&callee, &ast.expr(strip_paren(ast, f)).kind) {
            self.call_user_ty = Self::path_user_ty(ast, p);
        }
        self.call_value = callee;
        self.call_diverging(Callee::Fn(d, gargs), ops, dest, diverges);
        }

    pub(super) fn call_expr(&mut self, dest: Place, e: ExprId, f: ExprId, args: &[ExprId]) {
        let ast = self.ast;
        let ft = self.ty(f);
        let legacy_kept: Vec<ExprId>;
        let args: &[ExprId] = match self.b.legacy_args.get(&e.0) {
            Some(lc) => {
                legacy_kept = args.iter().enumerate().filter(|(i, _)| !lc.contains(&(*i as u32))).map(|(_, a)| *a).collect();
                &legacy_kept
            }
            None => args,
        };
        let t = self.ty(e);
        let diverges = matches!(self.tx.kind(t), TyKind::Never);
        let selection = self.b.call_traits.get(&e.0).copied();
        let call_derefs = selection.map(|s| s.derefs).or_else(|| self.b.call_derefs.get(&e.0).copied()).or_else(|| if matches!(self.tx.kind(ft), TyKind::Dyn(..)) { Some(0) } else { None });
        if let Some(n) = call_derefs {
            // the callable's type after the derefs decides whether the derefs are mutable
            // (calling a `RefMut<dyn FnMut>` derefs it mutably)
            let mut ft2 = ft;
            for k in 0..n {
                ft2 = self.deref_target_ty(e.0, k, ft2);
            }
            let Some(kind) = self.callable_kind(e, ft2) else { return };
            let mutbl = kind == ClosureKind::FnMut;
            let base = self.as_place(f, mutbl);
            let mut pl = base.clone();
            let mut pt = ft;
            for k in 0..n {
                pl = self.deref_step(e.0, k, pl, pt, mutbl);
                pt = self.place_ty(&pl);
            }
            if let TyKind::FnDef(d0, g0, _) = self.tx.kind(pt) {
                if selection.is_none() && !matches!(self.tx.prog.def(d0).kind, DefKind::Variant | DefKind::Struct) {
                    // `(&path)(x)`, a promoted fn item reference: the callee is the value behind
                    self.fn_def_call(dest, f, args, d0, self.tx.tcx.tys.list(g0).to_vec(), Some(pl), diverges);
                    return;
                }
            }
            let recv = match kind {
                ClosureKind::FnOnce => {
                    if selection.is_some_and(|s| s.from_param) {
                        if selection.unwrap().by_move { Operand::Move(pl) } else { Operand::Copy(pl) }
                    } else if n == 0 {
                        self.operand_of(pl)
                    } else {
                        // `(*boxed)()` of a Box<dyn FnOnce>: the box moves
                        self.operand_of(base)
                    }
                }
                _ => {
                    let r = self.rv();
                    let m = kind == ClosureKind::FnMut;
                    let rt = self.tx.ref_ty(m, pt);
                    let tmp = self.temp(rt);
                    let bk = if m { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                    self.assign(Place::local(tmp), Rvalue::Ref(r, bk, pl));
                    Operand::Move(Place::local(tmp))
                }
            };
            let mut ops = vec![recv];
            // The formal inputs are the selected `Fn*` obligation's argument tuple: a callee whose
            // type implements the trait itself (`Box<dyn Fn(&Ui)>`: Box<F>: Fn<A>) has no
            // signature of its own to read, and a `&mut` argument for a `&` input needs its
            // coercion (rustc_hir_typeck/src/callee.rs confirm_builtin_call / check_call inputs
            // from the method signature of `call`/`call_mut`/`call_once`)
            let ps = match selection.map(|s| self.tx.kind(s.tupled_args)) {
                Some(TyKind::Tuple(ps)) => self.tx.tcx.tys.list(ps).to_vec(),
                _ => self.callable_params(pt),
            };
            let saved = std::mem::replace(&mut self.args_two_phase, true);
            for (i, a) in args.iter().enumerate() {
                let pt2 = ps.get(i).copied().unwrap_or(self.ty(*a));
                ops.push(self.as_operand_coerced(*a, pt2, true));
            }
            self.args_two_phase = saved;
            self.arg_spans = std::iter::once(self.espan(f)).chain(args.iter().map(|a| self.espan(*a))).collect();
            self.note_static_callee(f);
            self.call_diverging(Callee::Indirect, ops, dest, diverges);
            return;
        }
        match self.tx.kind(ft) {
            // a tuple constructor (a path, or a fn item value of it): an aggregate
            TyKind::FnDef(d, gargs, _) if matches!(self.tx.prog.def(d).kind, DefKind::Variant | DefKind::Struct) => {
                let k = self.tx.prog.def(d).kind;
                let ps: Vec<TyId> = match self.tx.tcx.ctor_sigs.get(&d) {
                    Some(sg) => sg.params.iter().map(|p| self.tx.tcx.tys.subst(*p, self.tx.tcx.tys.list(gargs))).collect(),
                    None => Vec::new(),
                };
                let mut ops = Vec::new();
                let vi = if k == DefKind::Variant { self.tx.prog.def(d).sub } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    let pt = ps.get(i).copied().unwrap_or(self.ty(*a));
                    // a field declared with a reference type is a reborrow site; a
                    // generic param field takes the value (its type is still open) unless
                    // typeck coerced the argument to the expected reference (rustc
                    // check_argument_types coerces to the expected input: `x == Some(path)`
                    // with `path: &mut T` reborrows `&*path` as the `&T` the comparison expects)
                    let rb = !self.declared_field_is_param(t, vi, Some(i as u32)) || self.b.reborrows.contains_key(&a.0);
                    ops.push(self.as_operand_coerced(*a, pt, rb));
                }
                // a constructor value (`f(x)` with `let f = S::<&'a u32>`): the aggregate's
                // slots equal the value's
                // (copied into a temp of this body: a use of the value -- a closure captures
                // it -- whose slots the copy equates, the skeleton being invariant)
                let callee = if matches!(ast.expr(strip_paren(ast, f)).kind, ExprKind::Path(_)) && matches!(self.b.res.get(&strip_paren(ast, f).0), Some(Res::Def(_))) {
                    None
                } else {
                    let cp = self.as_place(f, false);
                    let tmp = self.temp(ft);
                    self.assign(Place::local(tmp), Rvalue::Use(Operand::Copy(cp)));
                    Some(Place::local(tmp))
                };
                self.assign(dest, Rvalue::Aggregate(AggKind::Adt(t, vi), ops));
                self.note_agg_targs(f);
                if let Some(cp) = callee {
                    if let Some(si) = self.out.blocks[self.cur as usize].stmts.iter().rposition(|s| matches!(&s.kind, StmtKind::Assign(_, Rvalue::Aggregate(AggKind::Adt(..), _)))) {
                        self.out.agg_callee.push(((self.cur, si as u32), cp));
                    }
                }
            }
            TyKind::FnDef(d0, gargs0, _) => {
                // a fn item value (a local, a promoted reference...): its slots are the
                // callee's generic arguments' regions
                let direct = matches!(ast.expr(strip_paren(ast, f)).kind, ExprKind::Path(_)) && matches!(self.b.res.get(&strip_paren(ast, f).0), Some(Res::Def(_)));
                let cp = if direct { None } else { Some(self.as_place(f, false)) };
                self.fn_def_call(dest, f, args, d0, self.tx.tcx.tys.list(gargs0).to_vec(), cp, diverges);
            }
            TyKind::FnPtr(ps, _, _, _) => {
                let fop = self.as_operand(f);
                let mut ops = vec![fop];
                let saved = std::mem::replace(&mut self.args_two_phase, true);
                for (i, a) in args.iter().enumerate() {
                    let pt = self.tx.tcx.tys.list(ps).get(i).copied().unwrap_or(self.ty(*a));
                    ops.push(self.as_operand_coerced(*a, pt, true));
                }
                self.args_two_phase = saved;
                self.arg_spans = std::iter::once(self.espan(f)).chain(args.iter().map(|a| self.espan(*a))).collect();
                self.note_static_callee(f);
                self.call_diverging(Callee::Indirect, ops, dest, diverges);
            }
            // a value of a generic param type called through its Fn* bound (polymorphic bodies)
            TyKind::Closure(..) | TyKind::Param(_) => {
                let Some(kind) = self.callable_kind(e, ft) else { return };
                let pl = self.as_place(f, kind == ClosureKind::FnMut);
                let recv = match kind {
                    ClosureKind::FnOnce => self.operand_of(pl),
                    _ => {
                        let r = self.rv();
                        let m = kind == ClosureKind::FnMut;
                        let rt = self.tx.ref_ty(m, ft);
                        let tmp = self.temp(rt);
                        let bk = if m { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                        self.assign(Place::local(tmp), Rvalue::Ref(r, bk, pl));
                        Operand::Move(Place::local(tmp))
                    }
                };
                let mut ops = vec![recv];
                let ps = self.callable_params(ft);
                let saved = std::mem::replace(&mut self.args_two_phase, true);
                for (i, a) in args.iter().enumerate() {
                    let pt = ps.get(i).copied().unwrap_or(self.ty(*a));
                    ops.push(self.as_operand_coerced(*a, pt, true));
                }
                self.args_two_phase = saved;
                self.arg_spans = std::iter::once(self.espan(f)).chain(args.iter().map(|a| self.espan(*a))).collect();
                self.note_static_callee(f);
                self.call_diverging(Callee::Indirect, ops, dest, diverges);
            }
            _ => {
                unsup!(self, "call of an unsupported callee type");
                for a in args {
                    let _ = self.as_operand(*a);
                }
            }
        }
    }

    /// Capture analysis and BIR use the same typeck selection, never an assumed Fn.
    fn callable_kind(&mut self, e: ExprId, t: TyId) -> Option<ClosureKind> {
        let kind = self.b.call_kind(&self.tx, e, t).map(|k| match k {
            crate::upvar::ClosureKind::Fn => ClosureKind::Fn,
            crate::upvar::ClosureKind::FnMut => ClosureKind::FnMut,
            crate::upvar::ClosureKind::FnOnce => ClosureKind::FnOnce,
        });
        if kind.is_none() { self.unsupported("call-trait selection unavailable"); }
        kind
    }

    pub(super) fn callable_params(&self, t: TyId) -> Vec<TyId> {
        match self.tx.kind(t) {
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => match self.tx.kind(sig) {
                TyKind::FnPtr(ps, _, _, _) => self.tx.tcx.tys.list(ps).to_vec(),
                _ => Vec::new(),
            },
            TyKind::Dyn(_, targs, _, _, _, _, _) => match self.tx.tcx.tys.list(targs).first().map(|x| self.tx.kind(*x)) {
                Some(TyKind::Tuple(ps)) => self.tx.tcx.tys.list(ps).to_vec(),
                _ => Vec::new(),
            },
            TyKind::FnPtr(ps, _, _, _) => self.tx.tcx.tys.list(ps).to_vec(),
            // a generic param: its Fn* bound's signature
            TyKind::Param(k) => {
                let sig = self.tx.tcx.callable_signatures(self.tx.prog, self.def).iter().find(|b| b.param == k).map(|b| b.signature);
                match sig.map(|sg| self.tx.kind(sg)) {
                    Some(TyKind::FnPtr(ps, _, _, _)) => self.tx.tcx.tys.list(ps).to_vec(),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn method_call(&mut self, dest: Place, e: ExprId, recv: ExprId, args: &[ExprId]) {
        /// typeck's resolution, adjusted below (impl selection, receiver reborrow)
        struct MethodCall { def: DefId, derefs: usize, autoref: u8, args: Vec<TyId> }
        let mut m = match self.b.methods.get(&e.0) {
            Some(m) => MethodCall { def: m.def, derefs: m.derefs as usize, autoref: m.autoref, args: self.tx.list(m.args).to_vec() },
            None => {
                unsup!(self, "method without resolution");
                return;
            }
        };
        let t = self.ty(e);
        let diverges = matches!(self.tx.kind(t), TyKind::Never);
        let is_trait_fn = self.tx.prog.def(self.tx.prog.def(m.def).parent).kind == DefKind::Trait;
        let virt = match m.args.first().map(|t| self.tx.kind(*t)) {
            Some(TyKind::Dyn(..)) if is_trait_fn && self.tx.tcx.dyn_implements(self.tx.prog.def(m.def).parent, m.args[0]) => true,
            _ => false,
        };
        // the method's Self as typeck chose it (before impl resolution)
        let self_t0 = if is_trait_fn { m.args.first().copied() } else { None };
        if !virt {
            let (rd, ra) = self.tx.resolve_trait_method(m.def, &m.args);
            m.def = rd;
            m.args = ra;
        }
        let sig = self.tx.tcx.fn_sig(self.tx.prog, m.def).cloned();
        let rt = self.ty(recv);
        // polymorphic bodies: typeck may autoref a `&mut` receiver where rustc's probe takes it
        // by value at the first step (a bound's method on `&mut Self`): reborrow instead
        // (not when Self is the receiver's type itself: `(&x).clone()` with `impl Clone for &T`
        // takes `&&T`, an autoref rustc makes too)
        if self.tx.polymorphic.get() && m.autoref != 0 && m.derefs == 0 && matches!(self.tx.kind(rt), TyKind::Ref(..)) && self_t0 != Some(rt) {
            // (also `&S` receivers autoref'd through `impl AsRef for &T`: rustc takes the
            // reference by value at step 0)
            m.autoref = 0;
            m.derefs = 0;
        }
        let recv_op = if m.autoref != 0 {
            let mutbl = m.autoref == 2;
            let mut pl = self.as_place(recv, mutbl);
            let mut pt = rt;
            for k in 0..m.derefs {
                pl = self.deref_step(e.0, k, pl, pt, mutbl);
                pt = self.place_ty(&pl);
            }
            let r = self.rv();
            // an array receiver of a slice method: the borrow unsizes
            let reft = self.tx.ref_ty(mutbl, pt);
            let tmp = self.temp(reft);
            let bk = if mutbl { BorrowKind::Mut { two_phase: true } } else { BorrowKind::Shared };
            self.assign(Place::local(tmp), Rvalue::Ref(r, bk, pl));
            Operand::Move(Place::local(tmp))
        } else if m.derefs == 0 {
            let self_pt = sig.as_ref().and_then(|s| s.params.first().copied()).map(|p| {
                let x = self.tx.tcx.tys.subst(p, &m.args);
                self.tx.normalize(x)
            });
            // a `&mut self` receiver passed by value: reborrowed (rustc: the receiver adjusts
            // with a reborrow)
            match self_pt {
                Some(pt) if self.is_mut_ref(rt) && self.is_place_expr(recv) => self.as_operand_coerced(recv, pt, true),
                // an unsized receiver "by value" (Relax passes its fat pointer): a shared borrow
                _ if self.tx.is_unsized(rt) && self.is_place_expr(recv) => {
                    let p = self.as_place(recv, false);
                    let r = self.rv();
                    let tmp = self.temp(self.tx.ref_ty(false, rt));
                    self.assign(Place::local(tmp), Rvalue::Ref(r, BorrowKind::Shared, p));
                    Operand::Move(Place::local(tmp))
                }
                _ => self.as_operand(recv),
            }
        } else {
            // by value through derefs (Box / Copy through references)
            let mut pl = self.as_place(recv, false);
            let mut pt = rt;
            for k in 0..m.derefs {
                pl = self.deref_step(e.0, k, pl, pt, false);
                pt = self.place_ty(&pl);
            }
            // `&mut T` reached through derefs, passed by value: reborrow
            if self.is_mut_ref(pt) {
                let r = self.rv();
                let tmp = self.temp(pt);
                self.assign(Place::local(tmp), Rvalue::Ref(r, BorrowKind::Mut { two_phase: true }, pl.deref()));
                Operand::Move(Place::local(tmp))
            } else {
                self.operand_of(pl)
            }
        };
        let mut ops = vec![recv_op];
        self.note_hr_closures(m.def, 1, args);
        for (i, a) in args.iter().enumerate() {
            let pt = match &sig {
                Some(s) if i + 1 < s.params.len() => {
                    let x = self.tx.tcx.tys.subst(s.params[i + 1], &m.args);
                    self.tx.normalize(x)
                }
                _ => self.ty(*a),
            };
            let rb = self.reborrow_site(m.def, i + 1);
            let saved = std::mem::replace(&mut self.args_two_phase, true);
            ops.push(self.as_operand_coerced(*a, pt, rb));
            self.args_two_phase = saved;
        }
        let callee = if virt { Callee::Virtual(m.def, m.args) } else { Callee::Fn(m.def, m.args) };
        // the call's span: the method name onwards (rustc: the method call span)
        self.arg_spans = std::iter::once(self.espan(recv)).chain(args.iter().map(|a| self.espan(*a))).collect();
        if !virt {
            if let ExprKind::MethodCall(_, row) = &self.ast.expr(e).kind {
                let turbofish = &self.ast.row(*row).turbofish;
                self.call_user_ty = PathUserTy { self_ann: None, trait_lts: Vec::new(), trait_targs: Vec::new(), targs: Self::turbofish(self.ast, self.ast.gargs(*turbofish)) };
            }
        }
        self.call_diverging(callee, ops, dest, diverges);
    }

    pub(super) fn struct_lit(&mut self, dest: Place, e: ExprId, fields: &[FieldExpr], base: Option<ExprId>, t: TyId) {
        let (adt_d, vi) = match self.b.res.get(&e.0).copied() {
            Some(Res::Def(d)) if self.tx.prog.def(d).kind == DefKind::Variant => (Some(d), self.tx.prog.def(d).sub),
            _ => (None, 0),
        };
        let _ = adt_d;
        let n = self.tx.n_fields(t, vi);
        let mut ops: Vec<Option<Operand>> = vec![None; n];
        let mut order = Vec::new();
        for f in fields {
            let name = { let tx = self.pf.text(f.name); crate::ast::tuple_index(tx).ok_or(()).map(|n| n | 0x8000_0000).unwrap_or_else(|_| self.tx.prog.syms.get(tx).unwrap_or(u32::MAX)) };
            let fi = self.field_index(t, vi, name);
            let ft = match fi {
                Some(fi) => self.tx.field_ty(t, vi, fi).unwrap_or(self.ty(f.expr)),
                None => self.ty(f.expr),
            };
            // a generic param field takes the value unless typeck coerced it (as for tuple
            // constructors above)
            let rb = !self.declared_field_is_param(t, vi, fi) || self.b.reborrows.contains_key(&f.expr.0);
            let op = self.as_operand_coerced(f.expr, ft, rb);
            match fi {
                Some(fi) if (fi as usize) < n => {
                    ops[fi as usize] = Some(op);
                    order.push(fi);
                }
                _ => {}
            }
        }
        if let Some(bx) = base {
            // `..base`: the remaining fields are moved / copied out of the base place
            let bp = if self.is_place_expr(bx) {
                self.as_place(bx, false)
            } else {
                let bt = self.ty(bx);
                let tmp = self.temp(bt);
                self.into(Place::local(tmp), bx);
                Place::local(tmp)
            };
            for fi in 0..n {
                if ops[fi].is_none() {
                    let ft = self.tx.field_ty(t, vi, fi as u32).unwrap_or(t);
                    let fp = bp.project(Proj::Field(fi as u32, ft));
                    ops[fi] = Some(self.operand_of(fp));
                }
            }
        }
        let defaults = self.b.struct_defaults.get(&e.0);
        let ops: Vec<Operand> = ops.into_iter().enumerate().map(|(fi, o)| o.unwrap_or_else(|| {
            let ft = self.tx.field_ty(t, vi, fi as u32).unwrap_or(t);
            match defaults.and_then(|ds| ds.iter().find(|(i, _)| *i == fi as u32)) {
                Some((_, d)) => self.item_constant(ft, *d, Vec::new()),
                None => self.unknown_constant(ft),
            }
        })).collect();
        if self.tx.is_union(t) {
            // a union literal initializes one field
            let fi = order.first().copied().unwrap_or(0);
            let ft = self.tx.field_ty(t, 0, fi).unwrap_or(t);
            let op = ops.into_iter().nth(fi as usize).unwrap_or_else(|| self.unknown_constant(ft));
            self.assign(dest.project(Proj::Field(fi, ft)), Rvalue::Use(op));
            let si = self.out.blocks[self.cur as usize].stmts.len().saturating_sub(1) as u32;
            self.out.union_inits.push((self.cur, si));
            return;
        }
        self.assign(dest, Rvalue::Aggregate(AggKind::Adt(t, vi), ops));
        self.note_agg_targs(e);
    }

    pub(super) fn declared_field_is_param(&self, t: TyId, vi: u32, fi: Option<u32>) -> bool {
        let fi = match fi {
            Some(f) => f,
            None => return false,
        };
        if let TyKind::Adt(d, _, _) = self.tx.kind(t) {
            if let Some(f) = self.tx.tcx.adts.get(&d).and_then(|a| a.variants.get(vi as usize)).and_then(|v| v.fields.get(fi as usize)) {
                return matches!(self.tx.kind(f.ty), TyKind::Param(_));
            }
        }
        false
    }

    pub fn field_index(&self, t: TyId, vi: u32, name: u32) -> Option<u32> {
        if let TyKind::Adt(d, _, _) = self.tx.kind(t) {
            if let Some(a) = self.tx.tcx.adts.get(&d) {
                let v = a.variants.get(vi as usize)?;
                for (i, f) in v.fields.iter().enumerate() {
                    if f.name == name {
                        return Some(i as u32);
                    }
                }
            }
        }
        // tuple struct fields named by index (`S { 0: x }`)
        std::str::from_utf8(self.tx.prog.syms.str(name).as_bytes()).ok().and_then(|s| s.parse::<u32>().ok())
    }

    // rustc_ast_lowering/src/expr.rs:1945-2033 @8d1a764: consume the typed
    // desugaring, including both calls, so borrowck sees the same moves as codegen.
    pub(super) fn try_expr(&mut self, dest: Place, e: ExprId, x: ExprId) {
        use crate::lang_items::LangItem;
        let xt = self.ty(x);
        let input = self.temp(xt);
        self.into(Place::local(input), x);
        let (branch, args, flow) = self.b.try_branch[&e.0].clone();
        let tmp = self.temp(flow);
        let op = self.operand_of(Place::local(input));
        let (dd, aa) = self.tx.resolve_trait_method(branch, self.tx.list(args));
        self.call(Callee::Fn(dd, aa), vec![op], Place::local(tmp));
        let TyKind::Adt(def, _, _) = self.tx.kind(flow) else { return; };
        let variants = &self.tx.tcx.adts[&def].variants;
        let vi_ok = variants.iter().position(|v| Some(v.def) == self.tx.tcx.lang_items.get(LangItem::ControlFlowContinue)).unwrap() as u32;
        let vi_err = variants.iter().position(|v| Some(v.def) == self.tx.tcx.lang_items.get(LangItem::ControlFlowBreak)).unwrap() as u32;
        let okb = self.new_block();
        let errb = self.new_block();
        let d = self.temp(self.tx.tcx.tys.prim(crate::program::Prim::Isize));
        self.assign(Place::local(d), Rvalue::Discriminant(Place::local(tmp)));
        let targets = if vi_ok == 0 { vec![okb, errb] } else { vec![errb, okb] };
        self.term(TermKind::Switch(Operand::Copy(Place::local(d)), targets));
        self.switch_to(errb);
        let et = self.tx.field_ty(flow, vi_err, 0).unwrap();
        let ep = Place::local(tmp).project(Proj::Downcast(vi_err)).project(Proj::Field(0, et));
        let eop = self.operand_of(ep);
        let (fd, args) = self.b.try_from_residual[&e.0].clone();
        let (dd, aa) = self.tx.resolve_trait_method(fd, self.tx.list(args));
        self.call(Callee::Fn(dd, aa), vec![eop], Place::local(0));
        self.exit_to(0);
        self.term(TermKind::Return);
        self.switch_to(okb);
        let ot = self.tx.field_ty(flow, vi_ok, 0).unwrap();
        let op = Place::local(tmp).project(Proj::Downcast(vi_ok)).project(Proj::Field(0, ot));
        let o = self.operand_of(op);
        self.assign(dest, Rvalue::Use(o));
    }

    pub fn variant_index(&self, t: TyId, name: &str) -> u32 {
        if let TyKind::Adt(d, _, _) = self.tx.kind(t) {
            if let Some(a) = self.tx.tcx.adts.get(&d) {
                for (i, v) in a.variants.iter().enumerate() {
                    if self.tx.prog.syms.str(v.name) == name {
                        return i as u32;
                    }
                }
            }
        }
        0
    }

    pub(super) fn for_loop(&mut self, p: PatId, it: ExprId, body: BlockId, label: Option<Vec<u8>>) {
        // rustc_ast_lowering/src/expr.rs:1774-1784,1826-1838: both calls are
        // mandatory typeck results, including range and slice heads.
        let sp = self.set_span(it);
        let outer = self.push_scope(false);
        let (d, args, itt) = self.b.for_into[&it.0].clone();
        let op = self.as_operand(it);
        let (dd, aa) = self.tx.resolve_trait_method(d, self.tx.list(args));
        let iter = self.new_local(itt, LocalKind::Temp, true, Some("iter".to_string()));
        self.push(StmtKind::StorageLive(iter));
        self.schedule(iter);
        self.call(Callee::Fn(dd, aa), vec![op], Place::local(iter));
        let head = self.new_block();
        let exit = self.new_block();
        self.goto(head);
        self.switch_to(head);
        let fu = self.new_block();
        self.term(TermKind::FalseUnwind(fu));
        self.switch_to(fu);
        let depth = self.scopes.len();
        self.push_scope(true);
        let (nd, nargs) = self.b.for_next[&it.0].clone();
        let (dd, aa) = self.tx.resolve_trait_method(nd, self.tx.list(nargs));
        let sig = self.tx.tcx.fn_sig(self.tx.prog, dd).unwrap().clone();
        let ot = self.tx.tcx.tys.subst(sig.ret, &aa);
        let ot = self.tx.normalize(ot);
        let r = self.rv();
        let rt = self.tx.ref_ty(true, itt);
        let tmp = self.temp(rt);
        self.assign(Place::local(tmp), Rvalue::Ref(r, BorrowKind::Mut { two_phase: false }, Place::local(iter)));
        let res = self.temp(ot);
        self.call(Callee::Fn(dd, aa), vec![Operand::Move(Place::local(tmp))], Place::local(res));
        let some_vi = self.variant_index(ot, "Some");
        let d = self.temp(self.tx.tcx.tys.prim(crate::program::Prim::Isize));
        self.assign(Place::local(d), Rvalue::Discriminant(Place::local(res)));
        let some_b = self.new_block();
        let none_b = self.new_block();
        self.term(TermKind::Switch(Operand::Copy(Place::local(d)), vec![some_b, none_b]));
        self.switch_to(none_b);
        self.exit_to(depth);
        self.goto(exit);
        self.switch_to(some_b);
        let pt = self.b.pty(p);
        let vp = Place::local(res).project(Proj::Downcast(some_vi)).project(Proj::Field(0, pt));
        self.declare_bindings(p);
        self.bind_pat(p, vp, pt, false);
        self.loops.push(LoopCx { label, brk: exit, cont: Some(head), dest: None, depth });
        let unit = self.tx.unit();
        let tmp = self.new_local(unit, LocalKind::Temp, true, None);
        self.block_into(Place::local(tmp), body, None);
        self.loops.pop();
        self.pop_scope();
        self.goto(head);
        self.switch_to(exit);
        let _ = outer;
        self.pop_scope();
        self.span = sp;
    }

    pub(super) fn mac(&mut self, dest: Place, e: ExprId, m: &MacCall) {
        let name = match self.ast.segs(m.path.segs).last() {
            Some(s) => self.pf.text(s.name).to_string(),
            None => String::new(),
        };
        if name == "cfg" || m.args == NO_MAC_ARGS {
            self.assign(dest, Rvalue::Use(self.expr_constant(e)));
            return;
        }
        let ast = self.ast;
        let margs = ast.mac_arg(m.args);
        let exprs = ast.list(margs.exprs);
        match name.as_str() {
            "asm" => {
                unsup!(self, "inline asm");
            }
            "matches" => {
                let x = exprs[0];
                let t = self.ty(x);
                let place = if self.is_place_expr(x) {
                    self.as_place(x, false)
                } else {
                    let tmp = self.temp(t);
                    self.into(Place::local(tmp), x);
                    Place::local(tmp)
                };
                let yes = self.new_block();
                let no = self.new_block();
                let join = self.new_block();
                if let Some(p) = margs.pat {
                    self.push_scope(false);
                    self.test_pat(p, place.clone(), t, no);
                    self.declare_bindings(p);
                    if let Some(g) = margs.guard {
                        // bindings by reference in the guard
                        let gm = self.guard_mark();
                        self.bind_pat(p, place, t, true);
                        let ok = self.new_block();
                        self.cond(g, ok, no, false);
                        self.switch_to(ok);
                        self.clear_guard_aliases_to(gm);
                    }
                    self.goto(yes);
                    self.scopes.pop();
                } else {
                    self.goto(yes);
                }
                self.switch_to(yes);
                self.assign(dest.clone(), Rvalue::Use(self.bool_constant(true)));
                self.goto(join);
                self.switch_to(no);
                self.assign(dest, Rvalue::Use(self.bool_constant(false)));
                self.goto(join);
                self.switch_to(join);
            }
            "assert" => {
                if let Some(&c) = exprs.first() {
                    let ok = self.new_block();
                    let bad = self.new_block();
                    self.cond(c, ok, bad, false);
                    self.switch_to(bad);
                    for x in exprs.iter().skip(1) {
                        self.fmt_arg(*x);
                    }
                    self.term(TermKind::Unreachable);
                    self.switch_to(ok);
                }
                self.assign_unit(dest);
            }
            "debug_assert" | "debug_assert_eq" | "debug_assert_ne" => self.assign_unit(dest),
            _ => {
                // formatting family: the arguments are borrowed (format_args takes `&arg`)
                for x in exprs {
                    self.fmt_arg(*x);
                }
                if matches!(name.as_str(), "panic" | "unreachable" | "todo" | "unimplemented") {
                    self.term(TermKind::Unreachable);
                    self.dead();
                } else {
                    self.assign_unit(dest);
                }
                let _ = e;
            }
        }
    }

    pub(super) fn fmt_arg(&mut self, x: ExprId) {
        if matches!(self.ast.expr(x).kind, ExprKind::Lit(_)) {
            return;
        }
        let p = self.as_place(x, false);
        let pt = self.place_ty(&p);
        let r = self.rv();
        let tmp = self.temp(self.tx.ref_ty(false, pt));
        self.assign(Place::local(tmp), Rvalue::Ref(r, BorrowKind::Shared, p));
    }

    pub(super) fn destructure_assign(&mut self, a: ExprId, src: Place, t: TyId) {
        let ast = self.ast;
        match &ast.expr(a).kind {
            ExprKind::Paren(x) => self.destructure_assign(*x, src, t),
            ExprKind::Underscore => {}
            ExprKind::Tuple(v) => {
                let tys = match self.tx.kind(t) {
                    TyKind::Tuple(x) => self.tx.tcx.tys.list(x).to_vec(),
                    _ => Vec::new(),
                };
                let rest = ast.list(*v).iter().position(|x| matches!(ast.expr(*x).kind, ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false)));
                let n = tys.len();
                for (i, x) in ast.list(*v).iter().enumerate() {
                    if Some(i) == rest {
                        continue;
                    }
                    let fi = match rest {
                        Some(r) if i > r => n - (v.len() - i),
                        _ => i,
                    };
                    let ft = tys.get(fi).copied().unwrap_or(t);
                    self.destructure_assign(*x, src.project(Proj::Field(fi as u32, ft)), ft);
                }
            }
            ExprKind::Array(v) => {
                let et = match self.tx.kind(t) {
                    TyKind::Array(e2, _) | TyKind::Slice(e2) => e2,
                    _ => t,
                };
                for (i, x) in ast.list(*v).iter().enumerate() {
                    self.destructure_assign(*x, src.project(Proj::ConstIndex(i as u32, false, v.len() as u32)), et);
                }
            }
            ExprKind::Call(_, args) => {
                let vi = match self.b.res.get(&strip_paren(ast, match &ast.expr(a).kind { ExprKind::Call(f, _) => *f, _ => a }).0) {
                    Some(Res::Def(d)) if self.tx.prog.def(*d).kind == DefKind::Variant => self.tx.prog.def(*d).sub,
                    _ => 0,
                };
                let base = if self.tx.is_enum(t) { src.project(Proj::Downcast(vi)) } else { src };
                for (i, x) in ast.list(*args).iter().enumerate() {
                    let ft = self.tx.field_ty(t, vi, i as u32).unwrap_or(t);
                    self.destructure_assign(*x, base.project(Proj::Field(i as u32, ft)), ft);
                }
            }
            ExprKind::Struct(row) => {
                let fields = &ast.row(*row).fields;
                let vi = match self.b.res.get(&a.0) {
                    Some(Res::Def(d)) if self.tx.prog.def(*d).kind == DefKind::Variant => self.tx.prog.def(*d).sub,
                    _ => 0,
                };
                let base = if self.tx.is_enum(t) { src.project(Proj::Downcast(vi)) } else { src };
                for f in ast.list(*fields) {
                    let name = { let tx = self.pf.text(f.name); crate::ast::tuple_index(tx).ok_or(()).map(|n| n | 0x8000_0000).unwrap_or_else(|_| self.tx.prog.syms.get(tx).unwrap_or(u32::MAX)) };
                    if let Some(fi) = self.field_index(t, vi, name) {
                        let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                        self.destructure_assign(f.expr, base.project(Proj::Field(fi, ft)), ft);
                    }
                }
            }
            _ => {
                let at = self.ty(a);
                let op = self.operand_of(src);
                let p = self.as_place(a, true);
                if self.tx.needs_drop(at) {
                    let next = self.new_block();
                    self.term(TermKind::Drop(p.clone(), next));
                    self.switch_to(next);
                }
                self.assign(p, Rvalue::Use(op));
            }
        }
    }

    // ------------------------------------------------------------------ closures

    pub(super) fn closure_expr(&mut self, dest: Place, e: ExprId) {
        let ct = self.ty(e);
        let caps = super::capture::build_closure(self, e.0, ct);
        let idx = self.out.closures.len() as u32;
        self.out.closures.push(e.0);
        let mut ops = Vec::new();
        for c in &caps {
            // the captured place in this body's terms (its root local may be ours or an
            // upvar of ours)
            let p = self.local_place_with(c.root, &c.proj);
            match c.kind {
                0 => ops.push(self.operand_of(p)),
                k => {
                    let r = self.rv();
                    let pt = self.place_ty(&p);
                    let rt = self.tx.ref_ty(k >= 2, pt);
                    let tmp = self.temp(rt);
                    let bk = match k {
                        1 => BorrowKind::Shared,
                        2 => BorrowKind::Mut { two_phase: false },
                        _ => BorrowKind::Unique,
                    };
                    self.assign(Place::local(tmp), Rvalue::Ref(r, bk, p));
                    if c.span != Span::default() {
                        self.out.capture_spans.push((tmp, c.span));
                    }
                    ops.push(Operand::Move(Place::local(tmp)));
                }
            }
        }
        self.assign(dest, Rvalue::Aggregate(AggKind::Closure(idx), ops));
    }

    pub(super) fn unsupported(&mut self, msg: &str) {
        if self.out.unsupported.is_none() {
            self.out.unsupported = Some(msg.to_string());
        }
    }

    pub(super) fn guard_alias(&mut self, li: u32, p: Place) {
        let old = self.locals.insert(li, p);
        self.guard_saved.push((li, old));
    }

    /// The guard copy of `li` made in the current guard (or-pattern alternatives reuse it).
    pub(super) fn guard_local(&self, li: u32) -> Option<Local> {
        if self.guard_saved.iter().any(|x| x.0 == li) {
            self.locals.get(&li).map(|p| p.local)
        } else {
            None
        }
    }

    /// The guard binding locals made since `mark` (rustc ref_for_guard locals).
    pub(super) fn guard_locals_since(&self, mark: usize) -> Vec<Local> {
        let mut v: Vec<Local> = self.guard_saved[mark..].iter().filter_map(|(li, _)| self.locals.get(li).map(|p| p.local)).collect();
        v.dedup();
        v
    }

    pub(super) fn guard_mark(&self) -> usize {
        self.guard_saved.len()
    }

    pub(super) fn clear_guard_aliases_to(&mut self, mark: usize) {
        while self.guard_saved.len() > mark {
            let (li, old) = self.guard_saved.pop().unwrap();
            match old {
                Some(p) => {
                    self.locals.insert(li, p);
                }
                None => {
                    self.locals.remove(&li);
                }
            }
        }
    }

    pub(super) fn espan(&self, e: ExprId) -> Span {
        let x = self.ast.expr(e);
        Span { lo: x.lo, hi: x.hi }
    }

    pub fn local_place_with(&mut self, li: u32, proj: &[Proj]) -> Place {
        let mut p = self.local_place(li);
        p.proj.extend_from_slice(proj);
        p
    }

    pub(super) fn expected_closure_kind(&self, ce: u32) -> Option<ClosureKind> {
        use crate::typeck::ExpectedClosureKind;
        self.expected_kinds.get(&ce).copied().or_else(|| {
            self.b.closure_expected_kinds.get(&ce).map(|k| match k {
                ExpectedClosureKind::Fn => ClosureKind::Fn,
                ExpectedClosureKind::FnMut => ClosureKind::FnMut,
                ExpectedClosureKind::FnOnce => ClosureKind::FnOnce,
            })
        })
    }

    /// A nested builder for closure `ce` sharing this body's facts.
    pub fn sub_builder(&self, ce: u32) -> Build<'a> {
        let mut nb = Build::new(self.tx, self.b, format!("{}::{{closure}}", self.out.name));
        nb.closure = Some(ce);
        nb.edition = self.edition;
        nb.closure_kinds = self.closure_kinds.clone();
        nb.def = self.def;
        nb.hr_closures = self.hr_closures.clone();
        nb.hr_ret_ties = self.hr_ret_ties.clone();
        nb.expected_kinds = self.expected_kinds.clone();
        nb
    }

    /// Closure arguments given at a call: where the callee's parameter is a type param (or
    /// `impl Trait`) bounded by `Fn*(..)`, which of the closure's params get an elided `&`
    /// (higher-ranked) from the bound (rustc deduces the closure signature from it).
    fn note_hr_closures(&mut self, d: DefId, first_sig_param: usize, args: &[ExprId]) {
        let ast = self.ast;
        // the closure expression of an argument: `|..| ..` or `&mut |..| ..`
        let closure_of = |a: ExprId| -> Option<ExprId> {
            let mut e = strip_paren(ast, a);
            if let ExprKind::AddrOf(false, _, x) = &ast.expr(e).kind {
                e = strip_paren(ast, *x);
            }
            if matches!(ast.expr(e).kind, ExprKind::Closure { .. }) { Some(e) } else { None }
        };
        if !args.iter().any(|a| closure_of(*a).is_some()) {
            return;
        }
        let prog = self.tx.prog;
        let def = prog.def(d);
        let dfile = &prog.files[def.file as usize];
        let dast = &dfile.ast;
        let fsig = match &dast.item(def.item).kind {
            crate::ast::ItemKind::Fn(s, _, _) => dast.fn_sig(*s),
            _ => return,
        };
        let skip = if fsig.self_param.is_some() { 1 } else { 0 };
        for (i, a) in args.iter().enumerate() {
            let ce = match closure_of(*a) {
                Some(c) => c,
                None => continue,
            };
            let si = first_sig_param + i;
            if si < skip || si - skip >= fsig.params.len() {
                continue;
            }
            let mut pt = dast.list(fsig.params)[si - skip].ty;
            // `&mut dyn FnMut(&T)` / `&impl Fn(&T)`: the bound behind the reference
            while let crate::ast::Ty::Ref(_, _, x) | crate::ast::Ty::Paren(x) = dast.ty(pt) {
                pt = *x;
            }
            let mut bounds: Vec<&crate::ast::Bound> = Vec::new();
            // lifetimes bound by `for<'r>` (on the where-predicate or the bound itself)
            let mut hr_names: Vec<&str> = Vec::new();
            match dast.ty(pt) {
                crate::ast::Ty::ImplTrait(bs) | crate::ast::Ty::DynTrait(bs, _) => bounds.extend(dast.list(*bs).iter()),
                crate::ast::Ty::Path(p) if p.segs.len() == 1 && dast.segs(p.segs)[0].args().is_none() => {
                    let n = dfile.text(dast.segs(p.segs)[0].name);
                    for g in dast.list(fsig.generics.params) {
                        if let crate::ast::GenericParamKind::Type(bs, _) = &g.kind {
                            if dfile.text(g.name) == n {
                                bounds.extend(dast.list(*bs).iter());
                            }
                        }
                    }
                    for wp in dast.list(fsig.generics.where_) {
                        if let crate::ast::WherePred::Bound { ty, bounds: bs, hrtb } = wp {
                            if let crate::ast::Ty::Path(q) = dast.ty(*ty) {
                                if q.segs.len() == 1 && dfile.text(dast.segs(q.segs)[0].name) == n {
                                    bounds.extend(dast.list(*bs).iter());
                                    hr_names.extend(dast.list(*hrtb).iter().map(|g| dfile.text(g.name)));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            for b in bounds {
                if let crate::ast::Bound::Trait { path, hrtb, .. } = b {
                    let mut names = hr_names.clone();
                    names.extend(dast.list(*hrtb).iter().map(|g| dfile.text(g.name)));
                    let last = match dast.segs(path.segs).last() {
                        Some(l) => l,
                        None => continue,
                    };
                    let ek = match dfile.text(last.name) {
                        "Fn" => ClosureKind::Fn,
                        "FnMut" => ClosureKind::FnMut,
                        "FnOnce" => ClosureKind::FnOnce,
                        _ => continue,
                    };
                    // rustc deduces the closure's kind from the bound it is passed to
                    let rank = |k: ClosureKind| match k { ClosureKind::Fn => 0, ClosureKind::FnMut => 1, ClosureKind::FnOnce => 2 };
                    let cur = self.expected_kinds.get(&ce.0).copied();
                    if cur.map_or(true, |c| rank(ek) < rank(c)) {
                        self.expected_kinds.insert(ce.0, ek);
                    }
                    if let Some(crate::ast::GenericArgs::Paren(tys, ret)) = dast.gargs(last.args()) {
                        let tys = dast.list(*tys);
                        // `Fn(&T) -> &U`: elision ties the output to the input's region when
                        // the inputs have exactly one lifetime, a plain `&T`, and the output is a
                        // plain `&U`; other shapes stay lenient
                        if let Some(r) = ret.filter(|r| ast_ty_has_ref(dast, *r, 0)) {
                            let plain = |t: crate::ast::TyId| match dast.ty(t) {
                                crate::ast::Ty::Ref(l, _, x) if !ast_ty_has_ref(dast, *x, 0) && !ast_ty_has_lifetime_args(dast, *x) => Some(l.map(|l| dfile.text(l))),
                                _ => None,
                            };
                            let with_refs: Vec<usize> = (0..tys.len()).filter(|k| ast_ty_has_ref(dast, tys[*k], 0) || ast_ty_has_lifetime_args(dast, tys[*k])).collect();
                            let tie = match (plain(r), with_refs.as_slice()) {
                                (Some(rl), [k]) => match plain(tys[*k]) {
                                    Some(pl) => {
                                        let elided = |l: Option<&str>| l.map_or(true, |l| l == "'_");
                                        let ok = (elided(rl) && elided(pl)) || (rl.is_some() && rl == pl && names.contains(&rl.unwrap()));
                                        if ok { Some(*k) } else { None }
                                    }
                                    None => None,
                                },
                                _ => None,
                            };
                            if let Some(k) = tie {
                                let mut v = vec![false; tys.len()];
                                v[k] = true;
                                self.hr_closures.insert(ce.0, v);
                                self.hr_ret_ties.insert(ce.0, k);
                            }
                            continue;
                        }
                        let v: Vec<bool> = tys.iter().map(|t| match dast.ty(*t) {
                            crate::ast::Ty::Ref(None, _, _) => true,
                            crate::ast::Ty::Ref(Some(l), _, _) => dfile.text(*l) == "'_" || names.contains(&dfile.text(*l)),
                            _ => false,
                        }).collect();
                        if v.iter().any(|x| *x) {
                            self.hr_closures.insert(ce.0, v);
                        }
                    }
                }
            }
        }
    }

    pub fn closure_locals_known(&self) -> &HashMap<u32, Place> {
        &self.locals
    }
}

/// A source position inside an AST type (its first lifetime / path name), for diagnostics.
fn ty_first_span(ast: &Ast, t: crate::ast::TyId, depth: u32) -> Option<Span> {
    use crate::ast::Ty as A;
    if depth > 12 {
        return None;
    }
    match ast.ty(t) {
        A::Ref(Some(l), _, _) => Some(Span { lo: l.lo, hi: l.hi }),
        A::Ref(None, _, x) | A::Ptr(_, x) | A::Slice(x) | A::Array(x, _) | A::Paren(x) => ty_first_span(ast, *x, depth + 1),
        A::Tuple(v) => ast.list(*v).iter().find_map(|x| ty_first_span(ast, *x, depth + 1)),
        A::Path(p) => ast.segs(p.segs).first().map(|s| Span { lo: s.name.lo, hi: s.name.hi }),
        A::DynTrait(bs, _) | A::ImplTrait(bs) => ast.list(*bs).iter().find_map(|b| match b {
            crate::ast::Bound::Trait { path, .. } => ast.segs(path.segs).first().map(|s| Span { lo: s.name.lo, hi: s.name.hi }),
            crate::ast::Bound::Lifetime(l) => Some(Span { lo: l.lo, hi: l.hi }),
            _ => None,
        }),
        _ => None,
    }
}

/// Whether an AST path type has lifetime arguments (`Foo<'a>`); elided ones are not seen.
fn ast_ty_has_lifetime_args(ast: &Ast, t: crate::ast::TyId) -> bool {
    match ast.ty(t) {
        crate::ast::Ty::Path(p) => ast.segs(p.segs).iter().any(|s| match ast.gargs(s.args()) {
            Some(crate::ast::GenericArgs::Angle(v)) => ast.list(*v).iter().any(|a| matches!(a, crate::ast::GenericArg::Lifetime(_))),
            _ => false,
        }),
        crate::ast::Ty::DynTrait(..) | crate::ast::Ty::ImplTrait(_) => true,
        _ => false,
    }
}

/// Whether an AST type has a reference with an elided (or `'_`) lifetime.
fn ast_ty_has_elided_ref(ast: &Ast, t: crate::ast::TyId, depth: u32) -> bool {
    use crate::ast::Ty as A;
    if depth > 20 {
        return true;
    }
    match ast.ty(t) {
        A::Ref(None, _, _) => true,
        A::Ref(Some(_), _, x) | A::Ptr(_, x) | A::Slice(x) | A::Array(x, _) | A::Paren(x) => ast_ty_has_elided_ref(ast, *x, depth + 1),
        A::Tuple(v) => ast.list(*v).iter().any(|x| ast_ty_has_elided_ref(ast, *x, depth + 1)),
        A::Path(p) => ast.segs(p.segs).iter().any(|s| match ast.gargs(s.args()) {
            Some(crate::ast::GenericArgs::Angle(v)) => ast.list(*v).iter().any(|a| match a {
                crate::ast::GenericArg::Type(x) | crate::ast::GenericArg::Binding(_, _, x) => ast_ty_has_elided_ref(ast, *x, depth + 1),
                _ => false,
            }),
            Some(crate::ast::GenericArgs::Paren(..)) => true,
            None => false,
        }),
        A::Never | A::Infer | A::ImplicitSelf | A::CVarArgs => false,
        _ => true,
    }
}

/// Whether an AST type mentions a reference or a lifetime anywhere.
fn ast_ty_has_ref(ast: &Ast, t: crate::ast::TyId, depth: u32) -> bool {
    use crate::ast::Ty as A;
    if depth > 20 {
        return true;
    }
    match ast.ty(t) {
        A::Ref(..) => true,
        A::Ptr(_, x) | A::Slice(x) | A::Array(x, _) | A::Paren(x) => ast_ty_has_ref(ast, *x, depth + 1),
        A::Tuple(v) => ast.list(*v).iter().any(|x| ast_ty_has_ref(ast, *x, depth + 1)),
        A::Path(p) => ast.segs(p.segs).iter().any(|s| match ast.gargs(s.args()) {
            Some(crate::ast::GenericArgs::Angle(v)) => ast.list(*v).iter().any(|a| match a {
                crate::ast::GenericArg::Lifetime(_) => true,
                crate::ast::GenericArg::Type(x) | crate::ast::GenericArg::Binding(_, _, x) => ast_ty_has_ref(ast, *x, depth + 1),
                _ => false,
            }),
            Some(crate::ast::GenericArgs::Paren(v, r)) => ast.list(*v).iter().chain(r.iter()).any(|x| ast_ty_has_ref(ast, *x, depth + 1)),
            None => false,
        }) || ast_ty_is_local_alias_use(ast, p),
        A::Never | A::Infer | A::ImplicitSelf | A::CVarArgs => false,
        _ => true,
    }
}

/// A single-segment path with type arguments (`PairCoupled<u32>` may be an alias
/// `type PairCoupled<'a, T> = (&'a T, &'a T)` whose expansion holds references).
fn ast_ty_is_local_alias_use(_ast: &Ast, p: &crate::ast::Path) -> bool {
    p.no_qself() && p.segs.len() == 1 && matches!(_ast.gargs(_ast.segs(p.segs)[0].args()), Some(crate::ast::GenericArgs::Angle(v)) if _ast.list(*v).iter().any(|a| matches!(a, crate::ast::GenericArg::Type(_))))
}

pub fn unit_ty(tx: &Tx) -> TyId {
    tx.unit()
}
