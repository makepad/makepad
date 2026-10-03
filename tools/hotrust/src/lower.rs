//! Lowering of one type-checked function instance (typeck::Body) to RIR.
//! Small aggregates live in registers as their scalar leaves; everything else
//! lives in stack slots. Locals whose address is taken live in slots.

use crate::ast::*;
use crate::jit::{FnKey, Unit};
use crate::layout::{Layout, Leaf};
use crate::program::{DefId, DefKind, Prim};
use crate::rir::*;
use crate::typeck::{Body, Coerce, Res};
use crate::types::{TyId, TyKind};
use std::collections::HashMap;

#[derive(Clone)]
pub enum Val {
    /// scalar leaves in registers
    L(Vec<VReg>),
    /// aggregate in memory at base + offset
    M(VReg, i32),
}

#[derive(Clone)]
pub enum Place {
    Regs(Vec<VReg>),
    Mem(VReg, i32),
}

#[derive(Clone)]
enum LocalSt {
    Regs(Vec<VReg>),
    Slot(VReg),
}

struct LoopCx {
    label: Option<u32>,
    brk: u32,
    cont: u32,
    result: Option<(Place, TyId)>,
}

pub struct Lcx<'a> {
    pub u: &'a mut Unit,
    pub b: &'a Body,
    pub f: Func,
    cur: u32,
    pos: u32,
    locals: Vec<Option<LocalSt>>,
    addr_taken: Vec<bool>,
    loops: Vec<LoopCx>,
    /// Some(ptr) when the function returns through a hidden pointer
    sret: Option<VReg>,
    ret_ty: TyId,
    /// closure being lowered (its expr id), for parameter binding
    pub closure_of: Option<u32>,
    pub new_closures: Vec<(u32, u32)>, // (closure expr, fn id)
    pub errors: Vec<String>,
}

pub fn fn_name(u: &Unit, key: &FnKey) -> String {
    match key {
        FnKey::Inst(d, _) => u.prog.def_path(*d),
        FnKey::Closure(_, e, p) => format!("{{closure#{}}}@{}", e, p),
    }
}

impl<'a> Lcx<'a> {
    pub fn new(u: &'a mut Unit, b: &'a Body, name: String) -> Lcx<'a> {
        let mut f = Func::new(name, b.file);
        let entry = f.block();
        let n = b.locals.len();
        let ret = b.ret;
        Lcx {
            u,
            b,
            f,
            cur: entry,
            pos: 0,
            locals: vec![None; n],
            addr_taken: vec![false; n],
            loops: Vec::new(),
            sret: None,
            ret_ty: ret,
            closure_of: None,
            new_closures: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn err(&mut self, pos: u32, msg: String) {
        let prog = &self.u.prog;
        let (l, c) = crate::lexer::line_col(&prog.files[self.b.file as usize].src, pos);
        self.errors.push(format!("{}:{}:{}: {}", prog.file_paths[self.b.file as usize], l, c, msg));
    }

    fn ast(&self) -> &'a Ast {
        // the program outlives the lowering context; the AST is never mutated during lowering
        let p: *const Ast = &self.u.prog.files[self.b.file as usize].ast;
        unsafe { &*p }
    }

    fn src_text(&self, lo: u32, hi: u32) -> String {
        String::from_utf8_lossy(&self.u.prog.files[self.b.file as usize].src[lo as usize..hi as usize]).into_owned()
    }

    // ------------------------------------------------------------ emission

    fn emit(&mut self, i: Inst) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(i);
        b.pos.push(((self.b.file as u64) << 32) | self.pos as u64);
    }
    fn term(&mut self, t: Term) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.term = t;
        b.term_pos = ((self.b.file as u64) << 32) | self.pos as u64;
    }
    fn goto(&mut self, target: u32) {
        self.term(Term::Jump(target));
    }
    fn switch(&mut self, b: u32) {
        self.cur = b;
    }
    /// After a diverging expression: continue emitting into an unreachable block.
    fn dead(&mut self) {
        let b = self.f.block();
        self.switch(b);
    }
    fn iconst(&mut self, v: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Iconst(d, v));
        d
    }
    fn addr_add(&mut self, base: VReg, off: i32) -> VReg {
        if off == 0 {
            return base;
        }
        let k = self.iconst(off as i64);
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Add, U64, d, base, k));
        d
    }

    // ------------------------------------------------------------ types

    fn layout(&mut self, t: TyId) -> Layout {
        let u = &mut *self.u;
        u.lay.of(&mut u.tcx, t)
    }
    fn kind(&self, t: TyId) -> TyKind {
        self.u.tcx.tys.kind(t).clone()
    }
    fn int_ty(&self, t: TyId) -> IntTy {
        match self.u.tcx.tys.kind(t) {
            TyKind::Int(p) => {
                let (b, s) = crate::types::Types::int_info(*p);
                IntTy { bits: (b * 8) as u8, signed: s }
            }
            TyKind::Bool => IntTy { bits: 8, signed: false },
            TyKind::Char => IntTy { bits: 32, signed: false },
            _ => U64,
        }
    }
    fn is_f64(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Float(Prim::F64))
    }

    fn fresh_leaves(&mut self, leaves: &[Leaf]) -> Vec<VReg> {
        let mut v = Vec::new();
        for l in leaves {
            v.push(self.f.vreg(l.cls));
        }
        v
    }

    fn new_slot_for(&mut self, t: TyId) -> VReg {
        let l = self.layout(t);
        let s = self.f.slot(l.size, l.align);
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::SlotAddr(d, s));
        d
    }

    // ------------------------------------------------------------ values and places

    /// Loads a value of type `t` from memory into leaves if it fits in registers.
    fn load_val(&mut self, base: VReg, off: i32, t: TyId) -> Val {
        let l = self.layout(t);
        match l.leaves {
            Some(leaves) => {
                let mut v = Vec::new();
                for lf in &leaves {
                    let d = self.f.vreg(lf.cls);
                    self.emit(Inst::Load(lf.mem, d, base, off + lf.off as i32));
                    v.push(d);
                }
                Val::L(v)
            }
            None => Val::M(base, off),
        }
    }

    fn store_val(&mut self, base: VReg, off: i32, t: TyId, v: Val) {
        let l = self.layout(t);
        match v {
            Val::L(regs) => {
                if let Some(leaves) = l.leaves {
                    for (i, lf) in leaves.iter().enumerate() {
                        if i < regs.len() {
                            self.emit(Inst::Store(lf.mem, base, off + lf.off as i32, regs[i]));
                        }
                    }
                }
            }
            Val::M(src, soff) => {
                let s = self.addr_add(src, soff);
                let d = self.addr_add(base, off);
                if l.size > 0 {
                    self.emit(Inst::Copy(d, s, l.size));
                }
            }
        }
    }

    /// Value as register leaves (loads from memory when needed).
    fn to_leaves(&mut self, v: Val, t: TyId) -> Vec<VReg> {
        match v {
            Val::L(r) => r,
            Val::M(b, o) => match self.load_val(b, o, t) {
                Val::L(r) => r,
                Val::M(..) => Vec::new(),
            },
        }
    }

    fn read_place(&mut self, p: &Place, t: TyId) -> Val {
        match p {
            Place::Regs(r) => {
                let mut v = Vec::new();
                for x in r {
                    let c = self.f.vregs[x.0 as usize];
                    let d = self.f.vreg(c);
                    self.emit(Inst::Mov(d, *x));
                    v.push(d);
                }
                Val::L(v)
            }
            Place::Mem(b, o) => {
                let v = self.load_val(*b, *o, t);
                if let Val::M(..) = v {
                    // aggregate: copy so later writes to the place do not alias the value
                    let l = self.layout(t);
                    let tmp = self.new_slot_for(t);
                    let src = self.addr_add(*b, *o);
                    if l.size > 0 {
                        self.emit(Inst::Copy(tmp, src, l.size));
                    }
                    return Val::M(tmp, 0);
                }
                v
            }
        }
    }

    fn write_place(&mut self, p: &Place, t: TyId, v: Val) {
        match p {
            Place::Regs(r) => {
                let src = self.to_leaves(v, t);
                for i in 0..r.len().min(src.len()) {
                    self.emit(Inst::Mov(r[i], src[i]));
                }
            }
            Place::Mem(b, o) => self.store_val(*b, *o, t, v),
        }
    }

    /// Address of a place (spilling register places to a fresh slot).
    fn place_addr(&mut self, p: &Place, t: TyId) -> VReg {
        match p {
            Place::Mem(b, o) => self.addr_add(*b, *o),
            Place::Regs(r) => {
                let a = self.new_slot_for(t);
                self.store_val(a, 0, t, Val::L(r.clone()));
                a
            }
        }
    }

    fn val_to_place(&mut self, v: Val) -> Place {
        match v {
            Val::L(r) => Place::Regs(r),
            Val::M(b, o) => Place::Mem(b, o),
        }
    }

    /// Projection to field `fi` of variant `vi` of a place of type `t`.
    fn project(&mut self, p: &Place, t: TyId, vi: u32, fi: u32) -> (Place, TyId) {
        let k = self.kind(t);
        let l = self.layout(t);
        let (fty, off, field_tys): (TyId, u32, Vec<TyId>) = match &k {
            TyKind::Tuple(v) => (v[fi as usize], l.fields.get(fi as usize).copied().unwrap_or(0), v.clone()),
            TyKind::Adt(d, args) => {
                let adt = self.u.tcx.adts.get(d).cloned().unwrap();
                let var = &adt.variants[vi as usize];
                let mut fts = Vec::new();
                for f in &var.fields {
                    fts.push(self.u.tcx.tys.subst(f.ty, args));
                }
                let off = if adt.is_enum {
                    l.variant_fields[vi as usize][fi as usize]
                } else if adt.is_union {
                    0
                } else {
                    l.fields[fi as usize]
                };
                (fts[fi as usize], off, fts)
            }
            _ => (self.u.tcx.tys.error, 0, Vec::new()),
        };
        match p {
            Place::Mem(b, o) => (Place::Mem(*b, *o + off as i32), fty),
            Place::Regs(r) => {
                // leaves of preceding fields
                let mut start = 0usize;
                for i in 0..fi as usize {
                    let fl = self.layout(field_tys[i]);
                    start += fl.leaves.map_or(0, |x| x.len());
                }
                let fl = self.layout(fty);
                let n = fl.leaves.map_or(0, |x| x.len());
                let end = (start + n).min(r.len());
                (Place::Regs(r[start.min(end)..end].to_vec()), fty)
            }
        }
    }

    // ------------------------------------------------------------ locals

    fn alloc_local(&mut self, li: u32) -> LocalSt {
        let t = self.b.locals[li as usize].ty;
        let l = self.layout(t);
        let st = match (&l.leaves, self.addr_taken[li as usize]) {
            (Some(leaves), false) => LocalSt::Regs(self.fresh_leaves(&leaves.clone())),
            _ => {
                let a = self.new_slot_for(t);
                LocalSt::Slot(a)
            }
        };
        self.locals[li as usize] = Some(st.clone());
        st
    }

    fn local_place(&mut self, li: u32) -> Place {
        let st = match &self.locals[li as usize] {
            Some(s) => s.clone(),
            None => self.alloc_local(li),
        };
        match st {
            LocalSt::Regs(r) => Place::Regs(r),
            LocalSt::Slot(a) => Place::Mem(a, 0),
        }
    }

    /// Marks locals whose address is taken (they live in memory).
    fn scan_addr_taken(&mut self) {
        let ast = self.ast();
        let b = self.b;
        for (k, m) in &b.methods {
            if m.autoref != 0 {
                if let ExprKind::MethodCall { recv, .. } = &ast.expr(ExprId(*k)).kind {
                    self.mark_root(*recv);
                }
            }
        }
        for i in 0..b.expr_ty.len() {
            let e = ExprId(b.expr_lo + i as u32);
            if let ExprKind::AddrOf(_, _, x) = &ast.expr(e).kind {
                self.mark_root(*x);
            }
        }
        // pattern bindings by reference
        for (p, li) in &b.pat_local {
            if let Pat::Ident { by_ref: true, .. } = ast.pat(PatId(*p)) {
                let _ = li;
            }
        }
    }

    fn mark_root(&mut self, e: ExprId) {
        let ast = self.ast();
        match &ast.expr(e).kind {
            ExprKind::Path(_) => {
                if let Some(Res::Local(li)) = self.b.res.get(&e.0) {
                    self.addr_taken[*li as usize] = true;
                }
            }
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Paren(x) => self.mark_root(*x),
            ExprKind::Index(x, _) => self.mark_root(*x),
            _ => {}
        }
    }

    // ------------------------------------------------------------ function entry

    /// Lowers a fn item body. `params`: ABI parameter vregs are created here.
    pub fn lower_fn_body(&mut self, def: DefId) {
        self.scan_addr_taken();
        let ast = self.ast();
        let prog_def_item = self.u.prog.def(def).item;
        let (fsig, body) = match &ast.item(prog_def_item).kind {
            ItemKind::Fn(s, b, _) => (s.clone(), *b),
            ItemKind::Const(_, Some(e)) | ItemKind::Static(_, _, Some(e)) => {
                // const/static initializer thunk: fn(out: *mut T)
                let e = *e;
                let out = self.f.vreg(Cls::I);
                self.f.params.push(out);
                self.pos = ast.expr(e).lo;
                let v = self.lower_expr(e);
                let t = self.b.ty(e);
                self.store_val(out, 0, t, v);
                self.term(Term::Ret(Vec::new()));
                return;
            }
            _ => return,
        };
        let ret = self.b.ret;
        let rl = self.layout(ret);
        let ret_regs = match &rl.leaves {
            Some(l) if ret_fits(l) => {
                for lf in l {
                    self.f.rets.push(lf.cls);
                }
                true
            }
            _ => false,
        };
        if !ret_regs {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.sret = Some(p);
        }
        // self param: local 0 when present
        let mut li_self = None;
        if fsig.self_param.is_some() {
            li_self = Some(0u32);
        }
        let mut param_vals: Vec<(Option<u32>, Option<PatId>, TyId, Vec<VReg>, bool)> = Vec::new();
        if let Some(li) = li_self {
            let t = self.b.locals[li as usize].ty;
            let (regs, by_ptr) = self.abi_param(t);
            param_vals.push((Some(li), None, t, regs, by_ptr));
        }
        for p in &self.b.param_pats {
            let t = self.b.pty(*p);
            let (regs, by_ptr) = self.abi_param(t);
            param_vals.push((None, Some(*p), t, regs, by_ptr));
        }
        for (li, pat, t, regs, by_ptr) in param_vals {
            let v = if by_ptr { Val::M(regs[0], 0) } else { Val::L(regs) };
            match (li, pat) {
                (Some(li), _) => {
                    let pl = self.local_place(li);
                    self.write_place(&pl, t, v);
                }
                (None, Some(p)) => self.bind_pat(p, v, t),
                _ => {}
            }
        }
        if let Some(bid) = body {
            self.pos = ast.block(bid).lo;
            let v = self.lower_block(bid);
            self.pos = ast.block(bid).hi;
            self.ret_val(v, ret);
        }
    }

    /// Lowers a non-capturing closure as a function of its own.
    pub fn lower_closure_body(&mut self, ce: u32) {
        self.scan_addr_taken();
        let ast = self.ast();
        let (params, body) = match &ast.expr(ExprId(ce)).kind {
            ExprKind::Closure { params, body, .. } => (params.clone(), *body),
            _ => return,
        };
        let ret = self.b.ty(body);
        self.ret_ty = ret;
        let rl = self.layout(ret);
        let ret_regs = match &rl.leaves {
            Some(l) if ret_fits(l) => {
                for lf in l {
                    self.f.rets.push(lf.cls);
                }
                true
            }
            _ => false,
        };
        if !ret_regs {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.sret = Some(p);
        }
        let mut pv = Vec::new();
        for p in &params {
            let t = self.b.pty(p.pat);
            let (regs, by_ptr) = self.abi_param(t);
            pv.push((p.pat, t, regs, by_ptr));
        }
        for (p, t, regs, by_ptr) in pv {
            let v = if by_ptr { Val::M(regs[0], 0) } else { Val::L(regs) };
            self.bind_pat(p, v, t);
        }
        self.pos = ast.expr(body).lo;
        let v = self.lower_expr(body);
        self.ret_val(v, ret);
    }

    fn abi_param(&mut self, t: TyId) -> (Vec<VReg>, bool) {
        let l = self.layout(t);
        match l.leaves {
            Some(leaves) => {
                let regs = self.fresh_leaves(&leaves);
                for r in &regs {
                    self.f.params.push(*r);
                }
                if let TyKind::Ref(true, inner) = self.kind(t) {
                    if !matches!(self.kind(inner), TyKind::Str | TyKind::Slice(_)) && regs.len() == 1 {
                        self.f.noalias.push(regs[0]);
                    }
                }
                (regs, false)
            }
            None => {
                let p = self.f.vreg(Cls::I);
                self.f.params.push(p);
                // by-pointer aggregate params point to the caller's private copy
                self.f.noalias.push(p);
                (vec![p], true)
            }
        }
    }

    fn ret_val(&mut self, v: Val, t: TyId) {
        match self.sret {
            Some(p) => {
                self.store_val(p, 0, t, v);
                self.term(Term::Ret(Vec::new()));
            }
            None => {
                let r = self.to_leaves(v, t);
                self.term(Term::Ret(r));
            }
        }
    }

    // ------------------------------------------------------------ blocks / statements

    fn lower_block(&mut self, b: BlockId) -> Val {
        let ast = self.ast();
        let blk = ast.block(b);
        let n = blk.stmts.len();
        let mut result = Val::L(Vec::new());
        for (i, s) in blk.stmts.iter().enumerate() {
            match s {
                Stmt::Let { attrs, pat, init, else_, .. } => {
                    if !self.active(attrs) {
                        continue;
                    }
                    let t = self.b.pty(*pat);
                    match init {
                        Some(e) => {
                            self.pos = ast.expr(*e).lo;
                            let v = self.lower_expr_coerced(*e, t);
                            match else_ {
                                None => self.bind_pat(*pat, v, t),
                                Some(eb) => {
                                    let fail = self.f.block();
                                    let place = self.val_to_place(v);
                                    self.test_pat(*pat, &place, t, fail);
                                    let ok = self.cur;
                                    self.switch(fail);
                                    self.lower_block(*eb);
                                    self.term(Term::Unreachable);
                                    self.switch(ok);
                                    self.bind_pat_place(*pat, &place, t);
                                }
                            }
                        }
                        None => {
                            // declared, assigned later
                            if let Pat::Ident { .. } = ast.pat(*pat) {
                                if let Some(&li) = self.b.pat_local.get(&pat.0) {
                                    self.alloc_local(li);
                                }
                            }
                        }
                    }
                }
                Stmt::Item(_) | Stmt::Empty => {}
                Stmt::Expr(e, semi) => {
                    self.pos = ast.expr(*e).lo;
                    let v = self.lower_expr(*e);
                    if i == n - 1 && !*semi {
                        result = v;
                    }
                }
            }
        }
        result
    }

    fn active(&self, attrs: &[Attr]) -> bool {
        if attrs.is_empty() {
            return true;
        }
        let prog = &self.u.prog;
        let krate = prog.file_crate[self.b.file as usize];
        crate::cfg::active(prog.src(self.b.file), attrs, &prog.crates[krate as usize].cfg)
    }

    // ------------------------------------------------------------ patterns

    fn bind_pat(&mut self, p: PatId, v: Val, t: TyId) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Ident { sub: None, by_ref: false, .. } if !self.b.pat_res.contains_key(&p.0) => {
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    let pl = self.local_place(li);
                    self.write_place(&pl, t, v);
                }
            }
            _ => {
                let place = self.val_to_place(v);
                self.bind_pat_place(p, &place, t);
            }
        }
    }

    /// Binds the variables of an (already tested) pattern from a place.
    fn bind_pat_place(&mut self, p: PatId, place: &Place, t: TyId) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => {}
            Pat::Ident { by_ref, sub, .. } => {
                if self.b.pat_res.contains_key(&p.0) {
                    return;
                }
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    let lt = self.b.locals[li as usize].ty;
                    let v = if *by_ref {
                        let a = self.place_addr(place, t);
                        Val::L(vec![a])
                    } else {
                        self.read_place(place, t)
                    };
                    let pl = self.local_place(li);
                    self.write_place(&pl, lt, v);
                }
                if let Some(s) = sub {
                    self.bind_pat_place(*s, place, t);
                }
            }
            Pat::Paren(x) | Pat::Box(x) => self.bind_pat_place(*x, place, t),
            Pat::Ref(_, x) => {
                let inner = match self.kind(t) {
                    TyKind::Ref(_, i) => i,
                    _ => t,
                };
                let ptr = self.read_place(place, t);
                let ptr = self.to_leaves(ptr, t);
                if !ptr.is_empty() {
                    self.bind_pat_place(*x, &Place::Mem(ptr[0], 0), inner);
                }
            }
            Pat::Tuple(v) => {
                let n = match self.kind(t) {
                    TyKind::Tuple(x) => x.len(),
                    _ => 0,
                };
                let idx = seq_indices(ast, v, n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, 0, fi);
                    self.bind_pat_place(v[pi], &fp, ft);
                }
            }
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                let n = self.variant_len(t, vi);
                let idx = seq_indices(ast, v, n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, vi, fi);
                    self.bind_pat_place(v[pi], &fp, ft);
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                for fp in fields {
                    let fname = self.u.prog.syms.get(self.ident_text(fp.name)).unwrap_or(u32::MAX);
                    if let Some(fi) = self.field_index(t, vi, fname) {
                        let (pl, ft) = self.project(place, t, vi, fi);
                        self.bind_pat_place(fp.pat, &pl, ft);
                    }
                }
            }
            Pat::Slice(_) | Pat::Or(_) | Pat::Mac(_) => {}
        }
    }

    fn ident_text(&self, id: Ident) -> &'a str {
        let f = &self.u.prog.files[self.b.file as usize];
        let p: *const crate::parser::ParsedFile = f;
        let f = unsafe { &*p };
        f.text(id)
    }

    fn variant_of_pat(&self, p: PatId) -> u32 {
        match self.b.pat_res.get(&p.0) {
            Some(Res::Def(d)) => {
                let def = self.u.prog.def(*d);
                if def.kind == DefKind::Variant {
                    def.sub
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn variant_len(&self, t: TyId, vi: u32) -> usize {
        if let TyKind::Adt(d, _) = self.u.tcx.tys.kind(t) {
            if let Some(a) = self.u.tcx.adts.get(d) {
                return a.variants.get(vi as usize).map_or(0, |v| v.fields.len());
            }
        }
        0
    }

    fn field_index(&self, t: TyId, vi: u32, name: u32) -> Option<u32> {
        if let TyKind::Adt(d, _) = self.u.tcx.tys.kind(t) {
            if let Some(a) = self.u.tcx.adts.get(d) {
                let v = a.variants.get(vi as usize)?;
                for (i, f) in v.fields.iter().enumerate() {
                    if f.name == name {
                        return Some(i as u32);
                    }
                }
            }
        }
        None
    }

    /// Emits the test of a refutable pattern: falls through on match, jumps to `fail` otherwise.
    fn test_pat(&mut self, p: PatId, place: &Place, t: TyId, fail: u32) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident { sub, .. } => {
                if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
                    self.test_def_pat(d, place, t, fail);
                }
                if let Some(s) = sub {
                    self.test_pat(*s, place, t, fail);
                }
            }
            Pat::Path(_) => {
                if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
                    self.test_def_pat(d, place, t, fail);
                }
            }
            Pat::Paren(x) | Pat::Box(x) => self.test_pat(*x, place, t, fail),
            Pat::Lit(e) => {
                let v = self.read_place(place, t);
                let lv = self.lower_expr(*e);
                let c = self.eq_vals(v, lv, t);
                let ok = self.f.block();
                self.term(Term::Branch(c, ok, fail));
                self.switch(ok);
            }
            Pat::Range(a, b, incl) => {
                let v = self.read_place(place, t);
                let v = self.to_leaves(v, t);
                let fl = matches!(self.kind(t), TyKind::Float(_));
                let signed = self.int_ty(t).signed;
                let f64_ = self.is_f64(t);
                if let Some(a) = a {
                    let av = self.lower_expr(*a);
                    let av = self.to_leaves(av, t);
                    let c = self.f.vreg(Cls::I);
                    if fl {
                        self.emit(Inst::FCmp(Cond::Ge, f64_, c, v[0], av[0]));
                    } else {
                        self.emit(Inst::ICmp(Cond::Ge, signed, c, v[0], av[0]));
                    }
                    let ok = self.f.block();
                    self.term(Term::Branch(c, ok, fail));
                    self.switch(ok);
                }
                if let Some(b) = b {
                    let bv = self.lower_expr(*b);
                    let bv = self.to_leaves(bv, t);
                    let c = self.f.vreg(Cls::I);
                    let cond = if *incl { Cond::Le } else { Cond::Lt };
                    if fl {
                        self.emit(Inst::FCmp(cond, f64_, c, v[0], bv[0]));
                    } else {
                        self.emit(Inst::ICmp(cond, signed, c, v[0], bv[0]));
                    }
                    let ok = self.f.block();
                    self.term(Term::Branch(c, ok, fail));
                    self.switch(ok);
                }
            }
            Pat::Tuple(v) => {
                let n = match self.kind(t) {
                    TyKind::Tuple(x) => x.len(),
                    _ => 0,
                };
                let idx = seq_indices(ast, v, n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, 0, fi);
                    self.test_pat(v[pi], &fp, ft, fail);
                }
            }
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                self.test_variant(p, place, t, fail);
                let n = self.variant_len(t, vi);
                let idx = seq_indices(ast, v, n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, vi, fi);
                    self.test_pat(v[pi], &fp, ft, fail);
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                self.test_variant(p, place, t, fail);
                for fp in fields {
                    let fname = self.u.prog.syms.get(self.ident_text(fp.name)).unwrap_or(u32::MAX);
                    if let Some(fi) = self.field_index(t, vi, fname) {
                        let (pl, ft) = self.project(place, t, vi, fi);
                        self.test_pat(fp.pat, &pl, ft, fail);
                    }
                }
            }
            Pat::Ref(_, x) => {
                let inner = match self.kind(t) {
                    TyKind::Ref(_, i) => i,
                    _ => t,
                };
                let ptr = self.read_place(place, t);
                let ptr = self.to_leaves(ptr, t);
                if !ptr.is_empty() {
                    self.test_pat(*x, &Place::Mem(ptr[0], 0), inner, fail);
                }
            }
            Pat::Or(alts) => {
                let ok = self.f.block();
                for (i, a) in alts.iter().enumerate() {
                    let next = if i + 1 == alts.len() { fail } else { self.f.block() };
                    self.test_pat(*a, place, t, next);
                    self.goto(ok);
                    if i + 1 < alts.len() {
                        self.switch(next);
                    }
                }
                self.switch(ok);
            }
            Pat::Slice(_) | Pat::Mac(_) => {
                self.err(0, "slice/macro patterns are not supported yet".to_string());
            }
        }
    }

    fn test_def_pat(&mut self, d: DefId, place: &Place, t: TyId, fail: u32) {
        let k = self.u.prog.def(d).kind;
        match k {
            DefKind::Variant => {
                let vi = self.u.prog.def(d).sub;
                self.test_tag(place, t, vi, fail);
            }
            DefKind::Const | DefKind::AssocConst => {
                let v = self.read_place(place, t);
                let cv = self.const_val(d, Vec::new(), t);
                let c = self.eq_vals(v, cv, t);
                let ok = self.f.block();
                self.term(Term::Branch(c, ok, fail));
                self.switch(ok);
            }
            _ => {}
        }
    }

    fn test_variant(&mut self, p: PatId, place: &Place, t: TyId, fail: u32) {
        if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
            if self.u.prog.def(d).kind == DefKind::Variant {
                let vi = self.u.prog.def(d).sub;
                self.test_tag(place, t, vi, fail);
            }
        }
    }

    fn test_tag(&mut self, place: &Place, t: TyId, vi: u32, fail: u32) {
        let l = self.layout(t);
        let adt = match self.kind(t) {
            TyKind::Adt(d, _) => self.u.tcx.adts.get(&d).cloned(),
            _ => None,
        };
        let adt = match adt {
            Some(a) => a,
            None => return,
        };
        if adt.variants.len() <= 1 {
            return;
        }
        let disc = adt.variants[vi as usize].disc as i64;
        let tag = match (place, l.tag) {
            (Place::Mem(b, o), Some((toff, tmem))) => {
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Load(tmem, d, *b, *o + toff as i32));
                d
            }
            (Place::Regs(r), _) if !r.is_empty() => r[0],
            _ => return,
        };
        let k = self.iconst(disc);
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Eq, true, c, tag, k));
        let ok = self.f.block();
        self.term(Term::Branch(c, ok, fail));
        self.switch(ok);
    }

    fn eq_vals(&mut self, a: Val, b: Val, t: TyId) -> VReg {
        let k = self.kind(t);
        match k {
            TyKind::Float(_) => {
                let f64_ = self.is_f64(t);
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::FCmp(Cond::Eq, f64_, c, a[0], b[0]));
                c
            }
            TyKind::Ref(_, inner) if matches!(self.kind(inner), TyKind::Str) => {
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                let rt = self.u.rt.str_eq;
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::Call(Callee::Host(rt), vec![a[0], a[1], b[0], b[1]], vec![c]));
                c
            }
            _ => {
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                // all leaves equal
                let mut acc: Option<VReg> = None;
                for i in 0..a.len().min(b.len()) {
                    let c = self.f.vreg(Cls::I);
                    if self.f.vregs[a[i].0 as usize] == Cls::I {
                        self.emit(Inst::ICmp(Cond::Eq, false, c, a[i], b[i]));
                    } else {
                        let f64_ = self.f.vregs[a[i].0 as usize] == Cls::F64;
                        self.emit(Inst::FCmp(Cond::Eq, f64_, c, a[i], b[i]));
                    }
                    acc = Some(match acc {
                        None => c,
                        Some(p) => {
                            let d = self.f.vreg(Cls::I);
                            self.emit(Inst::IBin(IOp::And, U64, d, p, c));
                            d
                        }
                    });
                }
                match acc {
                    Some(c) => c,
                    None => self.iconst(1),
                }
            }
        }
    }

    // ------------------------------------------------------------ expressions

    fn lower_expr_coerced(&mut self, e: ExprId, target: TyId) -> Val {
        let v = self.lower_expr(e);
        self.apply_coercion(e, v, target)
    }

    fn apply_coercion(&mut self, e: ExprId, v: Val, target: TyId) -> Val {
        match self.b.coerce.get(&e.0).copied() {
            None | Some(Coerce::MutToConst) | Some(Coerce::Never) => v,
            Some(Coerce::Unsize) => {
                let src = self.b.ty(e);
                let len = match self.kind(src) {
                    TyKind::Ref(_, a) => match self.kind(a) {
                        TyKind::Array(_, n) => n,
                        _ => 0,
                    },
                    _ => 0,
                };
                let p = self.to_leaves(v, src);
                let l = self.iconst(len as i64);
                let _ = target;
                Val::L(vec![p[0], l])
            }
            Some(Coerce::ReifyFn) => {
                let t = self.b.ty(e);
                if let TyKind::FnDef(d, args) = self.kind(t) {
                    let id = self.u.fn_id(FnKey::Inst(d, args));
                    let r = self.f.vreg(Cls::I);
                    self.emit(Inst::FnAddr(r, id));
                    return Val::L(vec![r]);
                }
                v
            }
            Some(Coerce::ClosureFn) => {
                let id = self.closure_fn(e);
                let r = self.f.vreg(Cls::I);
                self.emit(Inst::FnAddr(r, id));
                Val::L(vec![r])
            }
        }
    }

    fn closure_fn(&mut self, e: ExprId) -> u32 {
        // the closure expression may be wrapped in parens
        let mut ce = e;
        loop {
            match &self.ast().expr(ce).kind {
                ExprKind::Paren(x) => ce = *x,
                _ => break,
            }
        }
        if let Some(caps) = self.b.closure_locals.get(&ce.0) {
            if !caps.is_empty() {
                self.err(self.ast().expr(ce).lo, "capturing closures are not supported yet".to_string());
            }
        }
        let parent = self.u.cur_fn;
        let id = self.u.fn_id(FnKey::Closure(self.b.file, ce.0, parent));
        self.new_closures.push((ce.0, id));
        id
    }

    pub fn lower_expr(&mut self, e: ExprId) -> Val {
        let ast = self.ast();
        let ex = ast.expr(e);
        let save_pos = self.pos;
        self.pos = ex.lo;
        let t = self.b.ty(e);
        let v = self.lower_expr_inner(e, t);
        self.pos = save_pos;
        v
    }

    fn unit(&self) -> Val {
        Val::L(Vec::new())
    }

    fn lower_expr_inner(&mut self, e: ExprId, t: TyId) -> Val {
        let ast = self.ast();
        let ex = ast.expr(e);
        match &ex.kind {
            ExprKind::Lit(k) => self.lower_lit(e, *k, t),
            ExprKind::Paren(x) => self.lower_expr(*x),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Local(li)) => {
                    let pl = self.local_place(li);
                    let lt = self.b.locals[li as usize].ty;
                    self.read_place(&pl, lt)
                }
                Some(Res::Def(d)) => self.lower_def_value(e, d, t),
                _ => self.unit(),
            },
            ExprKind::Unary(op, x) => {
                let xt = self.b.ty(*x);
                match op {
                    UnOp::Deref => {
                        let p = self.lower_expr(*x);
                        let p = self.to_leaves(p, xt);
                        self.read_place(&Place::Mem(p[0], 0), t)
                    }
                    UnOp::Neg => {
                        let v = self.lower_expr(*x);
                        let v = self.to_leaves(v, xt);
                        match self.kind(t) {
                            TyKind::Float(_) => {
                                let f64_ = self.is_f64(t);
                                let d = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                                self.emit(Inst::FUnary(FUn::Neg, f64_, d, v[0]));
                                Val::L(vec![d])
                            }
                            _ => {
                                let it = self.int_ty(t);
                                let d = self.f.vreg(Cls::I);
                                self.emit(Inst::INeg(it, d, v[0]));
                                Val::L(vec![d])
                            }
                        }
                    }
                    UnOp::Not => {
                        let v = self.lower_expr(*x);
                        let v = self.to_leaves(v, xt);
                        let d = self.f.vreg(Cls::I);
                        if matches!(self.kind(t), TyKind::Bool) {
                            let one = self.iconst(1);
                            self.emit(Inst::IBin(IOp::Xor, U64, d, v[0], one));
                        } else {
                            let it = self.int_ty(t);
                            self.emit(Inst::INot(it, d, v[0]));
                        }
                        Val::L(vec![d])
                    }
                }
            }
            ExprKind::Binary(op, a, b) => {
                if let Some(m) = self.b.binops.get(&e.0).cloned() {
                    // operator trait call: Op::op(a, b)
                    let at = self.b.ty(*a);
                    let bt = self.b.ty(*b);
                    let av = self.lower_expr(*a);
                    let bv = self.lower_expr(*b);
                    let (d, args) = self.u.tcx.resolve_trait_method(&self.u.prog, m.def, &m.args);
                    let id = self.u.fn_id(FnKey::Inst(d, args));
                    return self.emit_call(Callee::Fn(id), vec![(av, at), (bv, bt)], t);
                }
                self.lower_binary(*op, *a, *b, t)
            }
            ExprKind::Assign(a, b) => {
                let at = self.b.ty(*a);
                let v = self.lower_expr_coerced(*b, at);
                let (pl, pt) = self.lower_place(*a);
                self.write_place(&pl, pt, v);
                self.unit()
            }
            ExprKind::AssignOp(op, a, b) => {
                let (pl, pt) = self.lower_place(*a);
                let cur = self.read_place(&pl, pt);
                let cur = self.to_leaves(cur, pt);
                let bt = self.b.ty(*b);
                let rv = self.lower_expr(*b);
                let rv = self.to_leaves(rv, bt);
                let r = self.arith(*op, pt, cur[0], rv[0], bt);
                self.write_place(&pl, pt, Val::L(vec![r]));
                self.unit()
            }
            ExprKind::Cast(x, _) => {
                let xt = self.b.ty(*x);
                let v = self.lower_expr(*x);
                self.cast(v, xt, t)
            }
            ExprKind::Block(b, label) => {
                if label.is_some() {
                    let brk = self.f.block();
                    let res = self.result_place(t);
                    let l = label.map(|x| self.lifetime_sym(x));
                    self.loops.push(LoopCx { label: l, brk, cont: brk, result: Some((res.clone(), t)) });
                    let v = self.lower_block(*b);
                    self.loops.pop();
                    if !self.is_never(t) {
                        self.write_place(&res, t, v);
                    }
                    self.goto(brk);
                    self.switch(brk);
                    return self.read_place(&res, t);
                }
                self.lower_block(*b)
            }
            ExprKind::Unsafe(b) | ExprKind::ConstBlock(b) => self.lower_block(*b),
            ExprKind::If(c, then, els) => self.lower_if(*c, *then, *els, t),
            ExprKind::Let(p, x) => {
                // only reached as a bare condition (`if let` handles its own); evaluate as a test
                let xt = self.b.ty(*x);
                let v = self.lower_expr(*x);
                let place = self.val_to_place(v);
                let res = self.f.vreg(Cls::I);
                let fail = self.f.block();
                let join = self.f.block();
                self.test_pat(*p, &place, xt, fail);
                self.bind_pat_place(*p, &place, xt);
                self.emit(Inst::Iconst(res, 1));
                self.goto(join);
                self.switch(fail);
                self.emit(Inst::Iconst(res, 0));
                self.goto(join);
                self.switch(join);
                Val::L(vec![res])
            }
            ExprKind::Match(x, arms) => self.lower_match(*x, arms, t),
            ExprKind::While(c, b, label) => {
                let head = self.f.block();
                let body = self.f.block();
                let exit = self.f.block();
                self.goto(head);
                self.switch(head);
                
                self.lower_cond(*c, body, exit);
                self.switch(body);
                let l = label.map(|x| self.lifetime_sym(x));
                self.loops.push(LoopCx { label: l, brk: exit, cont: head, result: None });
                self.lower_block(*b);
                self.loops.pop();
                self.goto(head);
                self.switch(exit);
                self.unit()
            }
            ExprKind::Loop(b, label) => {
                let head = self.f.block();
                let exit = self.f.block();
                let res = self.result_place(t);
                self.goto(head);
                self.switch(head);
                
                let l = label.map(|x| self.lifetime_sym(x));
                self.loops.push(LoopCx { label: l, brk: exit, cont: head, result: Some((res.clone(), t)) });
                self.lower_block(*b);
                self.loops.pop();
                self.goto(head);
                self.switch(exit);
                if self.is_never(t) {
                    self.term(Term::Unreachable);
                    self.dead();
                    return self.unit();
                }
                self.read_place(&res, t)
            }
            ExprKind::For(p, it, b, label) => {
                self.lower_for(*p, *it, *b, *label);
                self.unit()
            }
            ExprKind::Break(label, x) => {
                let l = label.map(|y| self.lifetime_sym(y));
                let idx = self.loop_index(l);
                if let Some(i) = idx {
                    if let Some(v) = x {
                        let val = self.lower_expr(*v);
                        if let Some((pl, pt)) = self.loops[i].result.clone() {
                            self.write_place(&pl, pt, val);
                        }
                    }
                    let brk = self.loops[i].brk;
                    self.goto(brk);
                }
                self.dead();
                self.unit()
            }
            ExprKind::Continue(label) => {
                let l = label.map(|y| self.lifetime_sym(y));
                if let Some(i) = self.loop_index(l) {
                    let c = self.loops[i].cont;
                    self.goto(c);
                }
                self.dead();
                self.unit()
            }
            ExprKind::Return(x) => {
                let rt = self.ret_ty;
                let v = match x {
                    Some(v) => self.lower_expr_coerced(*v, rt),
                    None => self.unit(),
                };
                self.ret_val(v, rt);
                self.dead();
                self.unit()
            }
            ExprKind::Tuple(v) => {
                let tys = match self.kind(t) {
                    TyKind::Tuple(x) => x,
                    _ => Vec::new(),
                };
                let mut vals = Vec::new();
                for (i, x) in v.iter().enumerate() {
                    let ft = tys.get(i).copied().unwrap_or(self.u.tcx.tys.error);
                    vals.push((self.lower_expr_coerced(*x, ft), ft));
                }
                self.build_record(t, 0, vals)
            }
            ExprKind::Array(v) => {
                let et = match self.kind(t) {
                    TyKind::Array(e, _) => e,
                    _ => self.u.tcx.tys.error,
                };
                let el = self.layout(et);
                let stride = crate::layout::round_up(el.size, el.align);
                let a = self.new_slot_for(t);
                for (i, x) in v.iter().enumerate() {
                    let val = self.lower_expr_coerced(*x, et);
                    self.store_val(a, (i as u32 * stride) as i32, et, val);
                }
                Val::M(a, 0)
            }
            ExprKind::Repeat(x, _) => {
                let (et, n) = match self.kind(t) {
                    TyKind::Array(e, n) => (e, n),
                    _ => (self.u.tcx.tys.error, 0),
                };
                let el = self.layout(et);
                let stride = crate::layout::round_up(el.size, el.align);
                let a = self.new_slot_for(t);
                let val = self.lower_expr(*x);
                if n <= 16 {
                    for i in 0..n {
                        self.store_val(a, (i as u32 * stride) as i32, et, val.clone());
                    }
                } else {
                    // loop: i in 0..n
                    let i = self.f.vreg(Cls::I);
                    self.emit(Inst::Iconst(i, 0));
                    let nn = self.iconst(n as i64);
                    let st = self.iconst(stride as i64);
                    let head = self.f.block();
                    let body = self.f.block();
                    let exit = self.f.block();
                    self.goto(head);
                    self.switch(head);
                    let c = self.f.vreg(Cls::I);
                    self.emit(Inst::ICmp(Cond::Lt, false, c, i, nn));
                    self.term(Term::Branch(c, body, exit));
                    self.switch(body);
                    let off = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Mul, U64, off, i, st));
                    let addr = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Add, U64, addr, a, off));
                    self.store_val(addr, 0, et, val);
                    let one = self.iconst(1);
                    self.emit(Inst::IBin(IOp::Add, U64, i, i, one));
                    self.goto(head);
                    self.switch(exit);
                }
                Val::M(a, 0)
            }
            ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) => {
                if let ExprKind::Index(a, i) = &ex.kind {
                    if matches!(self.kind(self.b.ty(*i)), TyKind::Adt(..)) {
                        return self.lower_slice_range(*a, *i, t);
                    }
                }
                let (pl, pt) = self.lower_place(e);
                self.read_place(&pl, pt)
            }
            ExprKind::AddrOf(_, _, x) => {
                // `&[T; N]` from an array literal, `&local`, `&expr`
                let (pl, pt) = self.lower_place(*x);
                if matches!(self.kind(pt), TyKind::Str | TyKind::Slice(_)) {
                    // reborrow of an unsized place: the fat pointer itself
                    if let Place::Regs(r) = pl {
                        return Val::L(r);
                    }
                }
                let a = self.place_addr(&pl, pt);
                Val::L(vec![a])
            }
            ExprKind::Call(f, args) => self.lower_call(e, *f, args, t),
            ExprKind::MethodCall { recv, args, .. } => self.lower_method(e, *recv, args, t),
            ExprKind::Struct(_, fields, base) => self.lower_struct_lit(e, fields, *base, t),
            ExprKind::Range(a, b, _) => {
                // Range { start, end } as a plain struct value
                let mut vals = Vec::new();
                let tys = self.range_field_tys(t);
                if let Some(a) = a {
                    vals.push((self.lower_expr(*a), tys[0]));
                }
                if let Some(b) = b {
                    let ft = if tys.len() > 1 { tys[1] } else { tys[0] };
                    vals.push((self.lower_expr(*b), ft));
                }
                // RangeInclusive has an extra `exhausted: bool` field
                if let TyKind::Adt(d, _) = self.kind(t) {
                    let nf = self.u.tcx.adts.get(&d).map_or(0, |a| a.variants[0].fields.len());
                    while vals.len() < nf {
                        let z = self.iconst(0);
                        vals.push((Val::L(vec![z]), self.u.tcx.tys.bool_));
                    }
                }
                self.build_record(t, 0, vals)
            }
            ExprKind::Closure { .. } => {
                // a closure value of closure type is zero-sized; calls go to its function
                self.unit()
            }
            ExprKind::Mac(m) => self.lower_mac(e, m, t),
            _ => {
                self.err(ex.lo, "unsupported expression in lowering".to_string());
                self.unit()
            }
        }
    }

    fn range_field_tys(&mut self, t: TyId) -> Vec<TyId> {
        if let TyKind::Adt(d, args) = self.kind(t) {
            if let Some(a) = self.u.tcx.adts.get(&d).cloned() {
                let mut v = Vec::new();
                for f in &a.variants[0].fields {
                    v.push(self.u.tcx.tys.subst(f.ty, &args));
                }
                return v;
            }
        }
        vec![self.u.tcx.tys.error]
    }

    fn is_never(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Never)
    }

    fn lifetime_sym(&self, id: Ident) -> u32 {
        let t = self.ident_text(id);
        let t = t.trim_start_matches('\'');
        self.u.prog.syms.get(t).unwrap_or(u32::MAX - 1)
    }

    fn loop_index(&self, label: Option<u32>) -> Option<usize> {
        let mut i = self.loops.len();
        while i > 0 {
            i -= 1;
            match label {
                None => {
                    // unlabeled break/continue skip labeled blocks (they have cont == brk)
                    if self.loops[i].cont != self.loops[i].brk || self.loops[i].label.is_none() {
                        return Some(i);
                    }
                }
                Some(l) => {
                    if self.loops[i].label == Some(l) {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    fn result_place(&mut self, t: TyId) -> Place {
        let l = self.layout(t);
        match l.leaves {
            Some(leaves) => Place::Regs(self.fresh_leaves(&leaves)),
            None => {
                let a = self.new_slot_for(t);
                Place::Mem(a, 0)
            }
        }
    }

    /// Builds a struct/tuple value from field values (registers when it fits).
    fn build_record(&mut self, t: TyId, vi: u32, vals: Vec<(Val, TyId)>) -> Val {
        let l = self.layout(t);
        match l.leaves {
            Some(_) => {
                let mut out = Vec::new();
                for (v, ft) in vals {
                    let r = self.to_leaves(v, ft);
                    out.extend(r);
                }
                Val::L(out)
            }
            None => {
                let a = self.new_slot_for(t);
                let place = Place::Mem(a, 0);
                for (i, (v, _)) in vals.into_iter().enumerate() {
                    let (fp, ft) = self.project(&place, t, vi, i as u32);
                    self.write_place(&fp, ft, v);
                }
                Val::M(a, 0)
            }
        }
    }

    fn lower_lit(&mut self, e: ExprId, k: LitKind, t: TyId) -> Val {
        let ex = self.ast().expr(e);
        let text = self.src_text(ex.lo, ex.hi);
        match k {
            LitKind::Int | LitKind::Float => match self.kind(t) {
                TyKind::Float(p) => {
                    let num = strip_num(&text);
                    if p == Prim::F32 {
                        let v: f32 = num.parse().unwrap_or(0.0);
                        let d = self.f.vreg(Cls::F32);
                        self.emit(Inst::Fconst(d, v.to_bits() as u64, false));
                        Val::L(vec![d])
                    } else {
                        let v: f64 = num.parse().unwrap_or(0.0);
                        let d = self.f.vreg(Cls::F64);
                        self.emit(Inst::Fconst(d, v.to_bits(), true));
                        Val::L(vec![d])
                    }
                }
                _ => {
                    let v = crate::consteval::parse_int_lit(&text).unwrap_or(0) as i64;
                    let it = self.int_ty(t);
                    let v = extend_const(v, it);
                    Val::L(vec![self.iconst(v)])
                }
            },
            LitKind::Bool(b) => Val::L(vec![self.iconst(b as i64)]),
            LitKind::Char => {
                let s = unescape_str(&text);
                let c = s.chars().next().unwrap_or('\0');
                Val::L(vec![self.iconst(c as i64)])
            }
            LitKind::Byte => {
                let b = unescape_bytes(&text);
                Val::L(vec![self.iconst(*b.first().unwrap_or(&0) as i64)])
            }
            LitKind::Str | LitKind::RawStr => {
                let s = unescape_str(&text);
                let addr = self.u.data(s.as_bytes(), 1);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                let l = self.iconst(s.len() as i64);
                Val::L(vec![p, l])
            }
            LitKind::ByteStr => {
                let b = unescape_bytes(&text);
                let addr = self.u.data(&b, 1);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                Val::L(vec![p])
            }
            LitKind::CStr => self.unit(),
        }
    }

    fn lower_def_value(&mut self, e: ExprId, d: DefId, t: TyId) -> Val {
        let k = self.u.prog.def(d).kind;
        match k {
            DefKind::Const | DefKind::AssocConst => {
                let args = self.b.res_args.get(&e.0).cloned().unwrap_or_default();
                self.const_val(d, args, t)
            }
            DefKind::Static => {
                let addr = self.u.static_addr(d);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                self.read_place(&Place::Mem(p, 0), t)
            }
            DefKind::Variant | DefKind::Struct => {
                // unit variant / unit struct value
                let vi = if k == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
                self.build_enum(t, vi, Vec::new())
            }
            // fn items are zero-sized values; calls resolve them directly
            _ => self.unit(),
        }
    }

    fn const_val(&mut self, d: DefId, args: Vec<TyId>, t: TyId) -> Val {
        match self.u.const_addr(d, args) {
            Some(addr) => {
                let l = self.layout(t);
                // scalars become immediates
                if let Some(leaves) = &l.leaves {
                    if leaves.len() == 1 {
                        let lf = leaves[0];
                        let raw = unsafe { read_raw(addr + lf.off as u64, lf.mem) };
                        return Val::L(vec![self.const_leaf(raw, lf)]);
                    }
                }
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                self.load_val(p, 0, t)
            }
            None => {
                self.err(self.pos, format!("could not evaluate constant {}", self.u.prog.def_path(d)));
                self.unit()
            }
        }
    }

    fn const_leaf(&mut self, raw: u64, lf: Leaf) -> VReg {
        match lf.cls {
            Cls::I => {
                let v = match lf.mem {
                    Mem::Int(1, true) => raw as u8 as i8 as i64,
                    Mem::Int(2, true) => raw as u16 as i16 as i64,
                    Mem::Int(4, true) => raw as u32 as i32 as i64,
                    _ => raw as i64,
                };
                self.iconst(v)
            }
            Cls::F32 => {
                let d = self.f.vreg(Cls::F32);
                self.emit(Inst::Fconst(d, raw & 0xffff_ffff, false));
                d
            }
            Cls::F64 => {
                let d = self.f.vreg(Cls::F64);
                self.emit(Inst::Fconst(d, raw, true));
                d
            }
        }
    }

    /// Enum (or struct) value of variant `vi` with field values.
    fn build_enum(&mut self, t: TyId, vi: u32, vals: Vec<(Val, TyId)>) -> Val {
        let adt = match self.kind(t) {
            TyKind::Adt(d, _) => self.u.tcx.adts.get(&d).cloned(),
            _ => None,
        };
        let adt = match adt {
            Some(a) => a,
            None => return self.unit(),
        };
        if !adt.is_enum {
            return self.build_record(t, 0, vals);
        }
        let l = self.layout(t);
        let disc = adt.variants[vi as usize].disc as i64;
        if l.leaves.is_some() {
            // fieldless enum: the tag itself
            return Val::L(vec![self.iconst(disc)]);
        }
        let a = self.new_slot_for(t);
        if let Some((toff, tmem)) = l.tag {
            let k = self.iconst(disc);
            self.emit(Inst::Store(tmem, a, toff as i32, k));
        }
        let place = Place::Mem(a, 0);
        for (i, (v, _)) in vals.into_iter().enumerate() {
            let (fp, ft) = self.project(&place, t, vi, i as u32);
            self.write_place(&fp, ft, v);
        }
        Val::M(a, 0)
    }

    fn lower_struct_lit(&mut self, e: ExprId, fields: &[FieldExpr], base: Option<ExprId>, t: TyId) -> Val {
        let d = match self.b.res.get(&e.0) {
            Some(Res::Def(d)) => *d,
            _ => return self.unit(),
        };
        let vi = if self.u.prog.def(d).kind == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
        let a = self.new_slot_for(t);
        let place = Place::Mem(a, 0);
        if let Some(bx) = base {
            let bv = self.lower_expr(bx);
            self.write_place(&place, t, bv);
        }
        let adt = match self.kind(t) {
            TyKind::Adt(ad, _) => self.u.tcx.adts.get(&ad).cloned(),
            _ => None,
        };
        if let Some(adt) = &adt {
            if adt.is_enum {
                let l = self.layout(t);
                if let Some((toff, tmem)) = l.tag {
                    let k = self.iconst(adt.variants[vi as usize].disc as i64);
                    self.emit(Inst::Store(tmem, a, toff as i32, k));
                }
            }
        }
        for fe in fields {
            let txt = self.ident_text(fe.name);
            let name = if txt.as_bytes()[0].is_ascii_digit() {
                txt.parse::<u32>().unwrap_or(0) | 0x8000_0000
            } else {
                self.u.prog.syms.get(txt).unwrap_or(u32::MAX)
            };
            if let Some(fi) = self.field_index(t, vi, name) {
                let (fp, ft) = self.project(&place, t, vi, fi);
                let v = self.lower_expr_coerced(fe.expr, ft);
                self.write_place(&fp, ft, v);
            }
        }
        self.load_val(a, 0, t)
    }

    // ------------------------------------------------------------ places

    pub fn lower_place(&mut self, e: ExprId) -> (Place, TyId) {
        let ast = self.ast();
        let ex = ast.expr(e);
        let t = self.b.ty(e);
        match &ex.kind {
            ExprKind::Paren(x) => self.lower_place(*x),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Local(li)) => {
                    let lt = self.b.locals[li as usize].ty;
                    (self.local_place(li), lt)
                }
                Some(Res::Def(d)) if self.u.prog.def(d).kind == DefKind::Static => {
                    let addr = self.u.static_addr(d);
                    let p = self.f.vreg(Cls::I);
                    self.emit(Inst::Addr(p, addr));
                    (Place::Mem(p, 0), t)
                }
                _ => {
                    let v = self.lower_expr(e);
                    (self.val_to_place(v), t)
                }
            },
            ExprKind::Unary(UnOp::Deref, x) => {
                let xt = self.b.ty(*x);
                let p = self.lower_expr(*x);
                let p = self.to_leaves(p, xt);
                if matches!(self.kind(t), TyKind::Str | TyKind::Slice(_)) {
                    return (Place::Regs(p), t);
                }
                (Place::Mem(p[0], 0), t)
            }
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (derefs, fi) = match self.b.field_idx.get(&e.0) {
                    Some(v) => *v,
                    None => return (Place::Regs(Vec::new()), t),
                };
                let (mut pl, mut pt) = self.lower_place(*x);
                for _ in 0..derefs {
                    let inner = match self.kind(pt) {
                        TyKind::Ref(_, i) | TyKind::Ptr(_, i) => i,
                        _ => pt,
                    };
                    let p = self.read_place(&pl, pt);
                    let p = self.to_leaves(p, pt);
                    pl = Place::Mem(p[0], 0);
                    pt = inner;
                }
                self.project(&pl, pt, 0, fi)
            }
            ExprKind::Index(a, i) => {
                let derefs = self.b.index_derefs.get(&e.0).copied().unwrap_or(0);
                let (mut pl, mut pt) = self.lower_place(*a);
                for _ in 0..derefs {
                    let inner = match self.kind(pt) {
                        TyKind::Ref(_, i) | TyKind::Ptr(_, i) => i,
                        _ => pt,
                    };
                    let p = self.read_place(&pl, pt);
                    let p = self.to_leaves(p, pt);
                    if matches!(self.kind(inner), TyKind::Slice(_) | TyKind::Str) {
                        pl = Place::Regs(p);
                    } else {
                        pl = Place::Mem(p[0], 0);
                    }
                    pt = inner;
                }
                let it = self.b.ty(*i);
                let iv = self.lower_expr(*i);
                let iv = self.to_leaves(iv, it);
                let (elem, base, len) = match self.kind(pt) {
                    TyKind::Array(el, n) => {
                        let a = self.place_addr(&pl, pt);
                        let l = self.iconst(n as i64);
                        (el, a, l)
                    }
                    TyKind::Slice(el) => match &pl {
                        Place::Regs(r) if r.len() == 2 => (el, r[0], r[1]),
                        _ => {
                            self.err(ex.lo, "slice place without length".to_string());
                            return (Place::Regs(Vec::new()), t);
                        }
                    },
                    _ => {
                        self.err(ex.lo, "cannot index".to_string());
                        return (Place::Regs(Vec::new()), t);
                    }
                };
                self.bounds_check(iv[0], len);
                let el = self.layout(elem);
                let stride = crate::layout::round_up(el.size, el.align);
                let s = self.iconst(stride as i64);
                let off = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Mul, U64, off, iv[0], s));
                let addr = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, addr, base, off));
                (Place::Mem(addr, 0), elem)
            }
            _ => {
                let v = self.lower_expr(e);
                (self.val_to_place(v), t)
            }
        }
    }

    fn bounds_check(&mut self, idx: VReg, len: VReg) {
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Lt, false, c, idx, len));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(c, ok, bad));
        self.switch(bad);
        let site = self.u.site(self.b.file, self.pos, "index out of bounds");
        let s = self.iconst(site as i64);
        let rt = self.u.rt.panic_bounds;
        self.emit(Inst::Call(Callee::Host(rt), vec![idx, len, s], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
    }

    fn lower_slice_range(&mut self, a: ExprId, i: ExprId, t: TyId) -> Val {
        // base[lo..hi] -> fat pointer
        let at = self.b.ty(a);
        let (mut pl, mut pt) = self.lower_place(a);
        while let TyKind::Ref(_, inner) = self.kind(pt) {
            let p = self.read_place(&pl, pt);
            let p = self.to_leaves(p, pt);
            pl = if matches!(self.kind(inner), TyKind::Slice(_) | TyKind::Str) { Place::Regs(p) } else { Place::Mem(p[0], 0) };
            pt = inner;
        }
        let _ = at;
        let (elem_size, base, len) = match self.kind(pt) {
            TyKind::Array(el, n) => {
                let l = self.layout(el);
                let a = self.place_addr(&pl, pt);
                (crate::layout::round_up(l.size, l.align), a, self.iconst(n as i64))
            }
            TyKind::Slice(el) => {
                let l = self.layout(el);
                match &pl {
                    Place::Regs(r) => (crate::layout::round_up(l.size, l.align), r[0], r[1]),
                    _ => return self.unit(),
                }
            }
            TyKind::Str => match &pl {
                Place::Regs(r) => (1, r[0], r[1]),
                _ => return self.unit(),
            },
            _ => return self.unit(),
        };
        let it = self.b.ty(i);
        let rv = self.lower_expr(i);
        let rv = self.to_leaves(rv, it);
        let name = match self.kind(it) {
            TyKind::Adt(d, _) => self.u.prog.name(d).to_string(),
            _ => String::new(),
        };
        let (lo, hi) = match name.as_str() {
            "Range" => (rv[0], rv[1]),
            "RangeFrom" => (rv[0], len),
            "RangeTo" => (self.iconst(0), rv[0]),
            "RangeInclusive" => {
                let one = self.iconst(1);
                let h = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, h, rv[1], one));
                (rv[0], h)
            }
            "RangeToInclusive" => {
                let one = self.iconst(1);
                let h = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, h, rv[0], one));
                (self.iconst(0), h)
            }
            _ => (self.iconst(0), len),
        };
        // lo <= hi <= len
        let c1 = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Le, false, c1, hi, len));
        let c2 = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Le, false, c2, lo, hi));
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::And, U64, c, c1, c2));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(c, ok, bad));
        self.switch(bad);
        let site = self.u.site(self.b.file, self.pos, "slice index out of range");
        let s = self.iconst(site as i64);
        let rt = self.u.rt.panic_bounds;
        self.emit(Inst::Call(Callee::Host(rt), vec![hi, len, s], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
        let es = self.iconst(elem_size as i64);
        let off = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Mul, U64, off, lo, es));
        let p = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Add, U64, p, base, off));
        let n = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Sub, U64, n, hi, lo));
        let _ = t;
        Val::L(vec![p, n])
    }

    // ------------------------------------------------------------ operators

    fn lower_binary(&mut self, op: BinOp, a: ExprId, b: ExprId, t: TyId) -> Val {
        match op {
            BinOp::And | BinOp::Or => {
                let res = self.f.vreg(Cls::I);
                let rhs = self.f.block();
                let short = self.f.block();
                let join = self.f.block();
                let at = self.b.ty(a);
                let av = self.lower_expr(a);
                let av = self.to_leaves(av, at);
                if op == BinOp::And {
                    self.term(Term::Branch(av[0], rhs, short));
                } else {
                    self.term(Term::Branch(av[0], short, rhs));
                }
                self.switch(short);
                self.emit(Inst::Iconst(res, if op == BinOp::And { 0 } else { 1 }));
                self.goto(join);
                self.switch(rhs);
                let bt = self.b.ty(b);
                let bv = self.lower_expr(b);
                let bv = self.to_leaves(bv, bt);
                self.emit(Inst::Mov(res, bv[0]));
                self.goto(join);
                self.switch(join);
                Val::L(vec![res])
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let at = self.b.ty(a);
                let av = self.lower_expr(a);
                let bv = self.lower_expr(b);
                let cond = match op {
                    BinOp::Eq => Cond::Eq,
                    BinOp::Ne => Cond::Ne,
                    BinOp::Lt => Cond::Lt,
                    BinOp::Le => Cond::Le,
                    BinOp::Gt => Cond::Gt,
                    _ => Cond::Ge,
                };
                match self.kind(at) {
                    TyKind::Float(_) => {
                        let f64_ = self.is_f64(at);
                        let av = self.to_leaves(av, at);
                        let bv = self.to_leaves(bv, at);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::FCmp(cond, f64_, d, av[0], bv[0]));
                        Val::L(vec![d])
                    }
                    TyKind::Int(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..) => {
                        let signed = self.int_ty(at).signed && matches!(self.kind(at), TyKind::Int(_));
                        let av = self.to_leaves(av, at);
                        let bv = self.to_leaves(bv, at);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::ICmp(cond, signed, d, av[0], bv[0]));
                        Val::L(vec![d])
                    }
                    _ => {
                        if cond == Cond::Eq || cond == Cond::Ne {
                            let c = self.eq_vals(av, bv, at);
                            if cond == Cond::Ne {
                                let one = self.iconst(1);
                                let d = self.f.vreg(Cls::I);
                                self.emit(Inst::IBin(IOp::Xor, U64, d, c, one));
                                return Val::L(vec![d]);
                            }
                            return Val::L(vec![c]);
                        }
                        self.err(self.pos, "ordering comparison of non-primitive types is not supported yet".to_string());
                        Val::L(vec![self.iconst(0)])
                    }
                }
            }
            _ => {
                let at = self.b.ty(a);
                let bt = self.b.ty(b);
                let av = self.lower_expr(a);
                let av = self.to_leaves(av, at);
                let bv = self.lower_expr(b);
                let bv = self.to_leaves(bv, bt);
                let r = self.arith(op, t, av[0], bv[0], bt);
                Val::L(vec![r])
            }
        }
    }

    /// Arithmetic on primitive operands of type `t` (rhs type `bt` matters for shifts).
    fn arith(&mut self, op: BinOp, t: TyId, a: VReg, b: VReg, bt: TyId) -> VReg {
        match self.kind(t) {
            TyKind::Float(_) => {
                let f64_ = self.is_f64(t);
                let d = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                let fop = match op {
                    BinOp::Add => FOp::Add,
                    BinOp::Sub => FOp::Sub,
                    BinOp::Mul => FOp::Mul,
                    BinOp::Div => FOp::Div,
                    BinOp::Rem => {
                        // a - trunc(a / b) * b is not exact; call the host fmod
                        let rt = if f64_ { self.u.rt.fmod } else { self.u.rt.fmodf };
                        self.emit(Inst::Call(Callee::Host(rt), vec![a, b], vec![d]));
                        return d;
                    }
                    _ => FOp::Add,
                };
                self.emit(Inst::FBin(fop, f64_, d, a, b));
                d
            }
            _ => {
                let it = self.int_ty(t);
                let d = self.f.vreg(Cls::I);
                let iop = match op {
                    BinOp::Add => IOp::Add,
                    BinOp::Sub => IOp::Sub,
                    BinOp::Mul => IOp::Mul,
                    BinOp::Div => IOp::Div,
                    BinOp::Rem => IOp::Rem,
                    BinOp::BitAnd => IOp::And,
                    BinOp::BitOr => IOp::Or,
                    BinOp::BitXor => IOp::Xor,
                    BinOp::Shl => IOp::Shl,
                    BinOp::Shr => IOp::Shr,
                    _ => IOp::Add,
                };
                if iop == IOp::Div || iop == IOp::Rem {
                    self.div_check(b, it, a);
                }
                let b = if iop == IOp::Shl || iop == IOp::Shr {
                    // release semantics: shift amount masked to the bit width
                    let _ = bt;
                    let m = self.iconst(it.bits as i64 - 1);
                    let mb = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::And, U64, mb, b, m));
                    mb
                } else {
                    b
                };
                self.emit(Inst::IBin(iop, it, d, a, b));
                d
            }
        }
    }

    fn div_check(&mut self, b: VReg, it: IntTy, a: VReg) {
        let z = self.iconst(0);
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Ne, false, c, b, z));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(c, ok, bad));
        self.switch(bad);
        let site = self.u.site(self.b.file, self.pos, "attempt to divide by zero");
        let s = self.iconst(site as i64);
        let rt = self.u.rt.panic_site;
        self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
        if it.signed {
            // MIN / -1 overflows
            let m1 = self.iconst(-1);
            let min = self.iconst(extend_const(1i64 << (it.bits as u32 - 1).min(63), it));
            let c1 = self.f.vreg(Cls::I);
            self.emit(Inst::ICmp(Cond::Eq, true, c1, b, m1));
            let c2 = self.f.vreg(Cls::I);
            self.emit(Inst::ICmp(Cond::Eq, true, c2, a, min));
            let both = self.f.vreg(Cls::I);
            self.emit(Inst::IBin(IOp::And, U64, both, c1, c2));
            let ok2 = self.f.block();
            let bad2 = self.f.block();
            self.term(Term::Branch(both, bad2, ok2));
            self.switch(bad2);
            let site = self.u.site(self.b.file, self.pos, "attempt to divide with overflow");
            let s = self.iconst(site as i64);
            let rt = self.u.rt.panic_site;
            self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
            self.term(Term::Unreachable);
            self.switch(ok2);
        }
    }

    fn cast(&mut self, v: Val, from: TyId, to: TyId) -> Val {
        let fk = self.kind(from);
        let tk = self.kind(to);
        let v = self.to_leaves(v, from);
        if v.is_empty() {
            return Val::L(v);
        }
        let x = v[0];
        match (&fk, &tk) {
            (TyKind::Float(_), TyKind::Float(_)) => {
                let ff = self.is_f64(from);
                let tf = self.is_f64(to);
                if ff == tf {
                    return Val::L(vec![x]);
                }
                let d = self.f.vreg(if tf { Cls::F64 } else { Cls::F32 });
                self.emit(Inst::Conv(if tf { Conv::F32ToF64 } else { Conv::F64ToF32 }, d, x));
                Val::L(vec![d])
            }
            (TyKind::Float(_), TyKind::Int(_)) => {
                let ff = self.is_f64(from);
                let it = self.int_ty(to);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::FToInt(ff, it), d, x));
                Val::L(vec![d])
            }
            (TyKind::Int(_), TyKind::Float(_)) | (TyKind::Bool, TyKind::Float(_)) => {
                let it = self.int_ty(from);
                let tf = self.is_f64(to);
                let d = self.f.vreg(if tf { Cls::F64 } else { Cls::F32 });
                self.emit(Inst::Conv(Conv::IntToF(it, tf), d, x));
                Val::L(vec![d])
            }
            (_, TyKind::Int(_)) | (_, TyKind::Char) => {
                // int/bool/char/pointer -> int: re-extend to the target type
                let it = self.int_ty(to);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::IntToInt(it), d, x));
                Val::L(vec![d])
            }
            (TyKind::Ref(..), TyKind::Ptr(..)) | (TyKind::Ptr(..), TyKind::Ptr(..)) | (TyKind::FnPtr(..), _) | (TyKind::Int(_), TyKind::Ptr(..)) => Val::L(v),
            _ => Val::L(v),
        }
    }

    // ------------------------------------------------------------ control flow

    /// Branches on a boolean condition expression (handles `let` chains with `&&`).
    fn lower_cond(&mut self, c: ExprId, t: u32, f: u32) {
        let ast = self.ast();
        match &ast.expr(c).kind {
            ExprKind::Let(p, x) => {
                let xt = self.b.ty(*x);
                let (place, pt) = if self.is_place_expr(*x) {
                    self.lower_place(*x)
                } else {
                    let v = self.lower_expr(*x);
                    (self.val_to_place(v), xt)
                };
                self.test_pat(*p, &place, pt, f);
                self.bind_pat_place(*p, &place, pt);
                self.goto(t);
            }
            ExprKind::Binary(BinOp::And, a, b) => {
                let mid = self.f.block();
                self.lower_cond(*a, mid, f);
                self.switch(mid);
                self.lower_cond(*b, t, f);
            }
            ExprKind::Paren(x) if !matches!(ast.expr(*x).kind, ExprKind::Let(..)) => self.lower_cond(*x, t, f),
            _ => {
                let ct = self.b.ty(c);
                let v = self.lower_expr(c);
                let v = self.to_leaves(v, ct);
                if v.is_empty() {
                    self.goto(f);
                } else {
                    self.term(Term::Branch(v[0], t, f));
                }
            }
        }
    }

    fn is_place_expr(&self, e: ExprId) -> bool {
        match &self.ast().expr(e).kind {
            ExprKind::Path(_) => matches!(self.b.res.get(&e.0), Some(Res::Local(_))),
            ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) | ExprKind::Unary(UnOp::Deref, _) => true,
            ExprKind::Paren(x) => self.is_place_expr(*x),
            _ => false,
        }
    }

    fn lower_if(&mut self, c: ExprId, then: BlockId, els: Option<ExprId>, t: TyId) -> Val {
        let tb = self.f.block();
        let eb = self.f.block();
        let join = self.f.block();
        let res = self.result_place(t);
        self.lower_cond(c, tb, eb);
        self.switch(tb);
        let v = self.lower_block(then);
        if !self.is_never(t) {
            self.write_place(&res, t, v);
        }
        self.goto(join);
        self.switch(eb);
        if let Some(x) = els {
            let v = self.lower_expr_coerced(x, t);
            if !self.is_never(t) {
                self.write_place(&res, t, v);
            }
        }
        self.goto(join);
        self.switch(join);
        if self.is_never(t) {
            self.term(Term::Unreachable);
            self.dead();
            return self.unit();
        }
        self.read_place(&res, t)
    }

    fn lower_match(&mut self, x: ExprId, arms: &[Arm], t: TyId) -> Val {
        let xt = self.b.ty(x);
        let (place, pt) = if self.is_place_expr(x) {
            self.lower_place(x)
        } else {
            let v = self.lower_expr(x);
            (self.val_to_place(v), xt)
        };
        let join = self.f.block();
        let res = self.result_place(t);
        for arm in arms {
            if !self.active(&arm.attrs) {
                continue;
            }
            let next = self.f.block();
            self.test_pat(arm.pat, &place, pt, next);
            self.bind_pat_place(arm.pat, &place, pt);
            if let Some(g) = arm.guard {
                let body = self.f.block();
                self.lower_cond(g, body, next);
                self.switch(body);
            }
            let v = self.lower_expr_coerced(arm.body, t);
            if !self.is_never(t) {
                self.write_place(&res, t, v);
            }
            self.goto(join);
            self.switch(next);
        }
        // no arm matched: unreachable for exhaustive matches
        self.term(Term::Unreachable);
        self.switch(join);
        if self.is_never(t) {
            self.term(Term::Unreachable);
            self.dead();
            return self.unit();
        }
        self.read_place(&res, t)
    }

    fn lower_for(&mut self, p: PatId, it: ExprId, b: BlockId, label: Option<Ident>) {
        let itt = self.b.ty(it);
        let l = label.map(|x| self.lifetime_sym(x));
        let ast = self.ast();
        match self.kind(itt) {
            TyKind::Adt(d, args) => {
                // integer ranges
                let name = self.u.prog.name(d).to_string();
                let et = args[0];
                let ity = self.int_ty(et);
                let (start, end) = match &ast.expr(strip_paren(ast, it)).kind {
                    ExprKind::Range(a, bb, _) => {
                        let s = match a {
                            Some(a) => {
                                let v = self.lower_expr(*a);
                                self.to_leaves(v, et)[0]
                            }
                            None => self.iconst(0),
                        };
                        let e = match bb {
                            Some(bb) => {
                                let v = self.lower_expr(*bb);
                                Some(self.to_leaves(v, et)[0])
                            }
                            None => None,
                        };
                        (s, e)
                    }
                    _ => {
                        let v = self.lower_expr(it);
                        let r = self.to_leaves(v, itt);
                        (r[0], r.get(1).copied())
                    }
                };
                let i = self.f.vreg(Cls::I);
                self.emit(Inst::Mov(i, start));
                let head = self.f.block();
                let body = self.f.block();
                let step = self.f.block();
                let exit = self.f.block();
                let incl = name == "RangeInclusive";
                if incl {
                    // empty if start > end
                    let c = self.f.vreg(Cls::I);
                    self.emit(Inst::ICmp(Cond::Le, ity.signed, c, i, end.unwrap()));
                    self.term(Term::Branch(c, body, exit));
                    self.switch(head);
                    self.goto(body);
                } else {
                    self.goto(head);
                    self.switch(head);
                    match end {
                        Some(e) => {
                            let c = self.f.vreg(Cls::I);
                            self.emit(Inst::ICmp(Cond::Lt, ity.signed, c, i, e));
                            self.term(Term::Branch(c, body, exit));
                        }
                        None => self.goto(body),
                    }
                }
                self.switch(body);
                
                let iv = self.f.vreg(Cls::I);
                self.emit(Inst::Mov(iv, i));
                self.bind_pat(p, Val::L(vec![iv]), et);
                self.loops.push(LoopCx { label: l, brk: exit, cont: step, result: None });
                self.lower_block(b);
                self.loops.pop();
                self.goto(step);
                self.switch(step);
                if incl {
                    let c = self.f.vreg(Cls::I);
                    self.emit(Inst::ICmp(Cond::Eq, false, c, i, end.unwrap()));
                    let inc = self.f.block();
                    self.term(Term::Branch(c, exit, inc));
                    self.switch(inc);
                }
                let one = self.iconst(1);
                self.emit(Inst::IBin(IOp::Add, ity, i, i, one));
                self.goto(head);
                self.switch(exit);
            }
            TyKind::Array(_, _) | TyKind::Ref(..) => {
                // arrays by value, &[T; N], &[T]: index loop
                let (base, len, elem, by_ref) = match self.kind(itt) {
                    TyKind::Array(el, n) => {
                        let v = self.lower_expr(it);
                        let pl = self.val_to_place(v);
                        let a = self.place_addr(&pl, itt);
                        (a, self.iconst(n as i64), el, false)
                    }
                    TyKind::Ref(_, inner) => {
                        let v = self.lower_expr(it);
                        let r = self.to_leaves(v, itt);
                        match self.kind(inner) {
                            TyKind::Array(el, n) => (r[0], self.iconst(n as i64), el, true),
                            TyKind::Slice(el) => (r[0], r[1], el, true),
                            _ => return,
                        }
                    }
                    _ => return,
                };
                let el = self.layout(elem);
                let stride = crate::layout::round_up(el.size, el.align);
                let i = self.f.vreg(Cls::I);
                self.emit(Inst::Iconst(i, 0));
                let head = self.f.block();
                let body = self.f.block();
                let step = self.f.block();
                let exit = self.f.block();
                self.goto(head);
                self.switch(head);
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::ICmp(Cond::Lt, false, c, i, len));
                self.term(Term::Branch(c, body, exit));
                self.switch(body);
                
                let s = self.iconst(stride as i64);
                let off = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Mul, U64, off, i, s));
                let addr = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, addr, base, off));
                let pt = self.b.pty(p);
                if by_ref {
                    self.bind_pat(p, Val::L(vec![addr]), pt);
                } else {
                    let v = self.read_place(&Place::Mem(addr, 0), elem);
                    self.bind_pat(p, v, elem);
                }
                self.loops.push(LoopCx { label: l, brk: exit, cont: step, result: None });
                self.lower_block(b);
                self.loops.pop();
                self.goto(step);
                self.switch(step);
                let one = self.iconst(1);
                self.emit(Inst::IBin(IOp::Add, U64, i, i, one));
                self.goto(head);
                self.switch(exit);
            }
            _ => {
                self.err(self.pos, "unsupported `for` iterator".to_string());
            }
        }
    }

    // ------------------------------------------------------------ calls

    /// Emits a call with HotRust's ABI. Returns the result value.
    fn emit_call(&mut self, callee: Callee, args: Vec<(Val, TyId)>, ret: TyId) -> Val {
        let rl = self.layout(ret);
        let mut abi_args = Vec::new();
        let mut rets = Vec::new();
        let mut sret = None;
        match &rl.leaves {
            Some(l) if ret_fits(l) => {
                for lf in l {
                    rets.push(self.f.vreg(lf.cls));
                }
            }
            _ => {
                let a = self.new_slot_for(ret);
                abi_args.push(a);
                sret = Some(a);
            }
        }
        let n_fixed_src = match &callee {
            Callee::HostVariadic(_, n) => *n as usize,
            _ => usize::MAX,
        };
        let mut n_fixed_abi = u32::MAX;
        for (k, (v, t)) in args.into_iter().enumerate() {
            if k == n_fixed_src {
                n_fixed_abi = abi_args.len() as u32;
            }
            let l = self.layout(t);
            match l.leaves {
                Some(_) => {
                    let r = self.to_leaves(v, t);
                    abi_args.extend(r);
                }
                None => {
                    // by pointer to a private copy
                    let tmp = self.new_slot_for(t);
                    self.store_val(tmp, 0, t, v);
                    abi_args.push(tmp);
                }
            }
        }
        let callee = match callee {
            Callee::HostVariadic(a, _) => {
                // no variadic arguments passed: all are fixed
                let n = if n_fixed_abi == u32::MAX { abi_args.len() as u32 } else { n_fixed_abi };
                Callee::HostVariadic(a, n)
            }
            c => c,
        };
        self.emit(Inst::Call(callee, abi_args, rets.clone()));
        if self.is_never(ret) {
            self.term(Term::Unreachable);
            self.dead();
            return self.unit();
        }
        match sret {
            Some(a) => Val::M(a, 0),
            None => Val::L(rets),
        }
    }

    fn lower_call(&mut self, e: ExprId, f: ExprId, args: &[ExprId], t: TyId) -> Val {
        let ft = self.b.ty(f);
        let _ = e;
        match self.kind(ft) {
            TyKind::FnDef(d0, gargs0) => {
                let (d, gargs) = self.u.tcx.resolve_trait_method(&self.u.prog, d0, &gargs0);
                let sig = self.u.tcx.sigs.get(&d).cloned();
                let sig = match sig {
                    Some(s) => s,
                    None => return self.unit(),
                };
                let mut vals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let pt = if i < sig.params.len() {
                        { let x = self.u.tcx.tys.subst(sig.params[i], &gargs); let u = &mut *self.u; u.tcx.normalize(&u.prog, x) }
                    } else {
                        self.b.ty(*a)
                    };
                    let v = self.lower_expr_coerced(*a, pt);
                    vals.push((v, pt));
                }
                if let Some(v) = self.intrinsic(d, &vals, t) {
                    return v;
                }
                let foreign = self.u.prog.def(d).kind == DefKind::ForeignFn;
                let callee = if foreign {
                    match self.u.foreign_addr(d) {
                        Some(a) if sig.variadic => Callee::HostVariadic(a, sig.params.len() as u32),
                        Some(a) => Callee::Host(a),
                        None => {
                            let n = self.u.prog.name(d).to_string();
                            self.err(self.pos, format!("unknown foreign function `{}`", n));
                            return self.unit();
                        }
                    }
                } else {
                    Callee::Fn(self.u.fn_id(FnKey::Inst(d, gargs)))
                };
                let r = self.emit_call(callee, vals, t);
                // C returns narrow integers with undefined upper bits: re-extend by type
                if foreign {
                    if let TyKind::Int(_) | TyKind::Bool | TyKind::Char = self.kind(t) {
                        let it = self.int_ty(t);
                        if it.bits < 64 {
                            if let Val::L(v) = &r {
                                if v.len() == 1 {
                                    let d2 = self.f.vreg(Cls::I);
                                    self.emit(Inst::Conv(Conv::IntToInt(it), d2, v[0]));
                                    return Val::L(vec![d2]);
                                }
                            }
                        }
                    }
                }
                r
            }
            TyKind::FnPtr(ps, _) => {
                // tuple struct / variant constructors resolve to FnPtr types too
                if let ExprKind::Path(_) = &self.ast().expr(strip_paren(self.ast(), f)).kind {
                    if let Some(Res::Def(d)) = self.b.res.get(&strip_paren(self.ast(), f).0).copied() {
                        let k = self.u.prog.def(d).kind;
                        if k == DefKind::Variant || k == DefKind::Struct {
                            let mut vals = Vec::new();
                            for (i, a) in args.iter().enumerate() {
                                let pt = ps.get(i).copied().unwrap_or(self.u.tcx.tys.error);
                                vals.push((self.lower_expr_coerced(*a, pt), pt));
                            }
                            let vi = if k == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
                            return self.build_enum(t, vi, vals);
                        }
                    }
                }
                let fv = self.lower_expr(f);
                let fv = self.to_leaves(fv, ft);
                let mut vals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let pt = ps.get(i).copied().unwrap_or(self.u.tcx.tys.error);
                    vals.push((self.lower_expr_coerced(*a, pt), pt));
                }
                self.emit_call(Callee::Indirect(fv[0]), vals, t)
            }
            TyKind::Closure(_, ce) => {
                let id = self.closure_fn(ExprId(ce));
                let mut vals = Vec::new();
                for a in args {
                    let at = self.b.ty(*a);
                    vals.push((self.lower_expr(*a), at));
                }
                self.emit_call(Callee::Fn(id), vals, t)
            }
            _ => {
                self.err(self.pos, "unsupported call".to_string());
                self.unit()
            }
        }
    }

    fn lower_method(&mut self, e: ExprId, recv: ExprId, args: &[ExprId], t: TyId) -> Val {
        let mut m = match self.b.methods.get(&e.0) {
            Some(m) => m.clone(),
            None => return self.unit(),
        };
        let (rd, ra) = self.u.tcx.resolve_trait_method(&self.u.prog, m.def, &m.args);
        m.def = rd;
        m.args = ra;
        let sig = match self.u.tcx.sigs.get(&m.def).cloned() {
            Some(s) => s,
            None => return self.unit(),
        };
        let self_pt = { let x = self.u.tcx.tys.subst(sig.params[0], &m.args); let u = &mut *self.u; u.tcx.normalize(&u.prog, x) };
        // receiver with derefs, then autoref
        let rt = self.b.ty(recv);
        let recv_val = if m.autoref != 0 {
            let (mut pl, mut pt) = self.lower_place(recv);
            for _ in 0..m.derefs {
                let inner = match self.kind(pt) {
                    TyKind::Ref(_, i) | TyKind::Ptr(_, i) => i,
                    _ => pt,
                };
                let p = self.read_place(&pl, pt);
                let p = self.to_leaves(p, pt);
                pl = if matches!(self.kind(inner), TyKind::Slice(_) | TyKind::Str) { Place::Regs(p) } else { Place::Mem(p[0], 0) };
                pt = inner;
            }
            if matches!(self.kind(pt), TyKind::Slice(_) | TyKind::Str) {
                match pl {
                    Place::Regs(r) => Val::L(r),
                    _ => self.unit(),
                }
            } else if let TyKind::Array(_, n) = self.kind(pt) {
                // array receiver for a slice method: unsize
                let a = self.place_addr(&pl, pt);
                let is_slice_self = matches!(self.kind(self_pt), TyKind::Ref(_, x) if matches!(self.kind(x), TyKind::Slice(_)));
                if is_slice_self {
                    let l = self.iconst(n as i64);
                    Val::L(vec![a, l])
                } else {
                    Val::L(vec![a])
                }
            } else {
                let a = self.place_addr(&pl, pt);
                Val::L(vec![a])
            }
        } else {
            let mut v = self.lower_expr(recv);
            let mut cur = rt;
            for _ in 0..m.derefs {
                let inner = match self.kind(cur) {
                    TyKind::Ref(_, i) | TyKind::Ptr(_, i) => i,
                    _ => cur,
                };
                let p = self.to_leaves(v, cur);
                v = if matches!(self.kind(inner), TyKind::Slice(_) | TyKind::Str) {
                    Val::L(p)
                } else {
                    self.read_place(&Place::Mem(p[0], 0), inner)
                };
                cur = inner;
            }
            v
        };
        let mut vals = vec![(recv_val, self_pt)];
        for (i, a) in args.iter().enumerate() {
            let pt = if i + 1 < sig.params.len() {
                { let x = self.u.tcx.tys.subst(sig.params[i + 1], &m.args); let u = &mut *self.u; u.tcx.normalize(&u.prog, x) }
            } else {
                self.b.ty(*a)
            };
            vals.push((self.lower_expr_coerced(*a, pt), pt));
        }
        if let Some(v) = self.intrinsic(m.def, &vals, t) {
            return v;
        }
        let id = self.u.fn_id(FnKey::Inst(m.def, m.args.clone()));
        self.emit_call(Callee::Fn(id), vals, t)
    }

    /// Calls to `extern "hotrust-intrinsic"` functions in HotRust's core become instructions.
    fn intrinsic(&mut self, d: DefId, vals: &[(Val, TyId)], t: TyId) -> Option<Val> {
        if self.u.prog.def(d).kind != DefKind::ForeignFn || !self.u.is_intrinsic(d) {
            return None;
        }
        let name = self.u.prog.name(d).to_string();
        let mut a = Vec::new();
        for (v, ty) in vals {
            let r = self.to_leaves(v.clone(), *ty);
            a.extend(r);
        }
        let f64_ = name.ends_with("f64");
        let cls = if f64_ { Cls::F64 } else { Cls::F32 };
        let un = |n: &str| -> Option<FUn> {
            Some(match n {
                "fabs" => FUn::Abs,
                "sqrt" => FUn::Sqrt,
                "floor" => FUn::Floor,
                "ceil" => FUn::Ceil,
                "trunc" => FUn::Trunc,
                "rint" => FUn::RoundEven,
                _ => return None,
            })
        };
        let base = name.trim_end_matches("f64").trim_end_matches("f32").trim_end_matches('_');
        if let Some(op) = un(base) {
            let d = self.f.vreg(cls);
            self.emit(Inst::FUnary(op, f64_, d, a[0]));
            return Some(Val::L(vec![d]));
        }
        match base {
            "fmin" | "fmax" => {
                let d = self.f.vreg(cls);
                let op = if base == "fmin" { FOp::Min } else { FOp::Max };
                self.emit(Inst::FBin(op, f64_, d, a[0], a[1]));
                Some(Val::L(vec![d]))
            }
            "to_bits" => {
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::FToBits(f64_), d, a[0]));
                Some(Val::L(vec![d]))
            }
            "from_bits" => {
                let d = self.f.vreg(cls);
                self.emit(Inst::Conv(Conv::BitsToF(f64_), d, a[0]));
                Some(Val::L(vec![d]))
            }
            "str_as_bytes" => Some(Val::L(a)),
            "slice_len" => Some(Val::L(vec![a[1]])),
            "unreachable" => {
                self.term(Term::Unreachable);
                self.dead();
                Some(self.unit())
            }
            _ => {
                let _ = t;
                self.err(self.pos, format!("unknown intrinsic `{}`", name));
                Some(self.unit())
            }
        }
    }

    // ------------------------------------------------------------ macros

    fn lower_mac(&mut self, e: ExprId, m: &MacCall, t: TyId) -> Val {
        let ast = self.ast();
        let name = match m.path.segs.last() {
            Some(s) => self.ident_text(s.name),
            None => "",
        };
        if m.args == NO_MAC_ARGS {
            return self.unit();
        }
        let margs = &ast.mac_args[m.args as usize];
        match name {
            "assert" => {
                let c = margs.exprs[0];
                let ok = self.f.block();
                let bad = self.f.block();
                self.lower_cond(c, ok, bad);
                self.switch(bad);
                if margs.exprs.len() > 1 {
                    self.lower_fmt(e, &margs.exprs[1..]);
                } else {
                    let ce = ast.expr(c);
                    let msg = format!("assertion failed: {}", self.src_text(ce.lo, ce.hi));
                    self.fmt_lit(&msg);
                }
                self.panic_here("");
                self.switch(ok);
                self.unit()
            }
            "debug_assert" | "debug_assert_eq" | "debug_assert_ne" => self.unit(),
            "assert_eq" | "assert_ne" => {
                let a = margs.exprs[0];
                let b2 = margs.exprs[1];
                let at = self.b.ty(a);
                let av = self.lower_expr(a);
                let bv = self.lower_expr(b2);
                // compare through references
                let (av2, bv2, ct) = self.deref_for_cmp(av.clone(), bv.clone(), at);
                let c = self.eq_vals(av2.clone(), bv2.clone(), ct);
                let ok = self.f.block();
                let bad = self.f.block();
                if name == "assert_eq" {
                    self.term(Term::Branch(c, ok, bad));
                } else {
                    self.term(Term::Branch(c, bad, ok));
                }
                self.switch(bad);
                let op = if name == "assert_eq" { "==" } else { "!=" };
                self.fmt_lit(&format!("assertion `left {} right` failed", op));
                if margs.exprs.len() > 2 {
                    self.fmt_lit(": ");
                    self.lower_fmt(e, &margs.exprs[2..]);
                }
                self.fmt_lit("\n  left: ");
                self.fmt_value(av2, ct, b'?');
                self.fmt_lit("\n right: ");
                self.fmt_value(bv2, ct, b'?');
                self.panic_here("");
                self.switch(ok);
                self.unit()
            }
            "print" | "println" | "eprint" | "eprintln" => {
                self.lower_fmt(e, &margs.exprs);
                if name.ends_with("ln") {
                    self.fmt_lit("\n");
                }
                let stream = if name.starts_with('e') { 2 } else { 1 };
                let s = self.iconst(stream);
                let rt = self.u.rt.fmt_print;
                self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
                self.unit()
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                let prefix = match name {
                    "unreachable" => "internal error: entered unreachable code",
                    "todo" => "not yet implemented",
                    "unimplemented" => "not implemented",
                    _ => "",
                };
                if !prefix.is_empty() {
                    self.fmt_lit(prefix);
                    if !margs.exprs.is_empty() {
                        self.fmt_lit(": ");
                    }
                }
                self.lower_fmt(e, &margs.exprs);
                if name == "panic" && margs.exprs.is_empty() {
                    self.fmt_lit("explicit panic");
                }
                self.panic_here("");
                self.dead();
                self.unit()
            }
            "matches" => {
                let x = margs.exprs[0];
                let xt = self.b.ty(x);
                let v = self.lower_expr(x);
                let place = self.val_to_place(v);
                let res = self.f.vreg(Cls::I);
                let fail = self.f.block();
                let join = self.f.block();
                if let Some(p) = margs.pat {
                    self.test_pat(p, &place, xt, fail);
                    self.bind_pat_place(p, &place, xt);
                    if let Some(g) = margs.guard {
                        let ok = self.f.block();
                        self.lower_cond(g, ok, fail);
                        self.switch(ok);
                    }
                }
                self.emit(Inst::Iconst(res, 1));
                self.goto(join);
                self.switch(fail);
                self.emit(Inst::Iconst(res, 0));
                self.goto(join);
                self.switch(join);
                let _ = t;
                Val::L(vec![res])
            }
            _ => self.unit(),
        }
    }

    fn deref_for_cmp(&mut self, a: Val, b: Val, t: TyId) -> (Val, Val, TyId) {
        let mut a = a;
        let mut b = b;
        let mut t = t;
        while let TyKind::Ref(_, inner) = self.kind(t) {
            if matches!(self.kind(inner), TyKind::Str | TyKind::Slice(_)) {
                break;
            }
            let pa = self.to_leaves(a, t);
            let pb = self.to_leaves(b, t);
            a = self.read_place(&Place::Mem(pa[0], 0), inner);
            b = self.read_place(&Place::Mem(pb[0], 0), inner);
            t = inner;
        }
        (a, b, t)
    }

    fn panic_here(&mut self, msg: &str) {
        let site = self.u.site(self.b.file, self.pos, msg);
        let s = self.iconst(site as i64);
        let rt = self.u.rt.panic_site;
        self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
        self.term(Term::Unreachable);
    }

    fn fmt_lit(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let addr = self.u.data(s.as_bytes(), 1);
        let p = self.f.vreg(Cls::I);
        self.emit(Inst::Addr(p, addr));
        let l = self.iconst(s.len() as i64);
        let z = self.iconst(0);
        let rt = self.u.rt.fmt_str;
        self.emit(Inst::Call(Callee::Host(rt), vec![p, l, z], Vec::new()));
    }

    /// Appends a formatted value to the runtime's format buffer.
    fn fmt_value_spec(&mut self, v: Val, t: TyId, spec: u64) {
        let mut v = v;
        let mut t = t;
        // format through references
        while let TyKind::Ref(_, inner) = self.kind(t) {
            if matches!(self.kind(inner), TyKind::Str) {
                break;
            }
            let p = self.to_leaves(v, t);
            v = self.read_place(&Place::Mem(p[0], 0), inner);
            t = inner;
        }
        let sp = self.iconst(spec as i64);
        match self.kind(t) {
            TyKind::Int(_) | TyKind::Bool | TyKind::Char => {
                let it = self.int_ty(t);
                let r = self.to_leaves(v, t);
                let kind = match self.kind(t) {
                    TyKind::Bool => 3,
                    TyKind::Char => 4,
                    _ => {
                        if it.signed {
                            1
                        } else {
                            2
                        }
                    }
                };
                let k = self.iconst(kind | ((it.bits as i64) << 8));
                let rt = self.u.rt.fmt_int;
                self.emit(Inst::Call(Callee::Host(rt), vec![r[0], k, sp], Vec::new()));
            }
            TyKind::Float(_) => {
                let f64_ = self.is_f64(t);
                let r = self.to_leaves(v, t);
                let x = if f64_ {
                    r[0]
                } else {
                    let d = self.f.vreg(Cls::F64);
                    self.emit(Inst::Conv(Conv::F32ToF64, d, r[0]));
                    d
                };
                let k = self.iconst(if f64_ { 1 } else { 0 });
                let rt = self.u.rt.fmt_float;
                self.emit(Inst::Call(Callee::Host(rt), vec![x, k, sp], Vec::new()));
            }
            TyKind::Ref(_, inner) if matches!(self.kind(inner), TyKind::Str) => {
                let r = self.to_leaves(v, t);
                let rt = self.u.rt.fmt_str;
                self.emit(Inst::Call(Callee::Host(rt), vec![r[0], r[1], sp], Vec::new()));
            }
            _ => {
                let s = crate::typeck::ty_to_string(&self.u.prog, &self.u.tcx, t);
                self.err(self.pos, format!("cannot format a value of type `{}` yet", s));
            }
        }
    }

    fn fmt_value(&mut self, v: Val, t: TyId, ty_char: u8) {
        let spec = FmtSpec { ty: ty_char, ..FmtSpec::default() }.pack();
        self.fmt_value_spec(v, t, spec);
    }

    fn lower_fmt(&mut self, e: ExprId, exprs: &[ExprId]) {
        if exprs.is_empty() {
            return;
        }
        let ast = self.ast();
        let fe = ast.expr(exprs[0]);
        let text = self.src_text(fe.lo, fe.hi);
        if !matches!(fe.kind, ExprKind::Lit(LitKind::Str) | ExprKind::Lit(LitKind::RawStr)) {
            // panic!(value) with a non-literal (2018 style): format it with Display
            let t = self.b.ty(exprs[0]);
            let v = self.lower_expr(exprs[0]);
            self.fmt_value(v, t, 0);
            return;
        }
        let s = unescape_str(&text);
        let pieces = parse_fmt(&s);
        // evaluate positional args once, in order
        let mut argv = Vec::new();
        for x in &exprs[1..] {
            let t = self.b.ty(*x);
            argv.push((self.lower_expr(*x), t));
        }
        let mut next = 0usize;
        for p in pieces {
            match p {
                FmtPiece::Lit(l) => self.fmt_lit(&l),
                FmtPiece::Arg { index, named, spec } => {
                    let (v, t) = match (index, named) {
                        (_, Some(n)) => {
                            let sy = self.u.prog.syms.get(&n).unwrap_or(u32::MAX);
                            match self.b.fmt_named.get(&(e.0, sy)).copied() {
                                Some(Res::Local(li)) => {
                                    let pl = self.local_place(li);
                                    let lt = self.b.locals[li as usize].ty;
                                    (self.read_place(&pl, lt), lt)
                                }
                                Some(Res::Def(d)) => {
                                    let ct = self.u.tcx.const_tys.get(&d).copied().unwrap_or(self.u.tcx.tys.error);
                                    (self.const_val(d, Vec::new(), ct), ct)
                                }
                                _ => continue,
                            }
                        }
                        (Some(i), None) => match argv.get(i) {
                            Some(x) => x.clone(),
                            None => continue,
                        },
                        (None, None) => {
                            let x = argv.get(next).cloned();
                            next += 1;
                            match x {
                                Some(x) => x,
                                None => continue,
                            }
                        }
                    };
                    self.fmt_value_spec(v, t, spec.pack());
                }
            }
        }
    }
}

fn ret_fits(l: &[Leaf]) -> bool {
    let mut ni = 0;
    let mut nf = 0;
    for x in l {
        if x.cls == Cls::I {
            ni += 1;
        } else {
            nf += 1;
        }
    }
    ni <= 2 && nf <= 2
}

fn strip_paren(ast: &Ast, mut e: ExprId) -> ExprId {
    while let ExprKind::Paren(x) = &ast.expr(e).kind {
        e = *x;
    }
    e
}

/// Indices (pattern index, field index) for tuple-like patterns with `..`.
fn seq_indices(ast: &Ast, v: &[PatId], n: usize) -> Vec<(usize, u32)> {
    let mut rest = None;
    for (i, p) in v.iter().enumerate() {
        if let Pat::Rest = ast.pat(*p) {
            rest = Some(i);
        }
    }
    let mut out = Vec::new();
    match rest {
        None => {
            for i in 0..v.len() {
                out.push((i, i as u32));
            }
        }
        Some(r) => {
            for i in 0..r {
                out.push((i, i as u32));
            }
            let after = v.len() - r - 1;
            for j in 0..after {
                out.push((r + 1 + j, (n - after + j) as u32));
            }
        }
    }
    out
}

pub fn extend_const(v: i64, it: IntTy) -> i64 {
    match (it.bits, it.signed) {
        (8, true) => v as i8 as i64,
        (8, false) => v as u8 as i64,
        (16, true) => v as i16 as i64,
        (16, false) => v as u16 as i64,
        (32, true) => v as i32 as i64,
        (32, false) => v as u32 as i64,
        _ => v,
    }
}

unsafe fn read_raw(addr: u64, m: Mem) -> u64 {
    let p = addr as *const u8;
    match m {
        Mem::Int(1, _) => *p as u64,
        Mem::Int(2, _) => (p as *const u16).read_unaligned() as u64,
        Mem::Int(4, _) | Mem::F32 => (p as *const u32).read_unaligned() as u64,
        _ => (p as *const u64).read_unaligned(),
    }
}

fn strip_num(text: &str) -> String {
    let mut s = String::new();
    for c in text.chars() {
        if c != '_' {
            s.push(c);
        }
    }
    for suf in ["f32", "f64"] {
        if let Some(x) = s.strip_suffix(suf) {
            return x.to_string();
        }
    }
    // integer-typed suffixes never reach floats; strip anyway
    s
}

// ---------------------------------------------------------------- literals

/// Value of a (possibly raw) string literal token.
pub fn unescape_str(text: &str) -> String {
    let b = unescape_bytes(text);
    String::from_utf8_lossy(&b).into_owned()
}

pub fn unescape_bytes(text: &str) -> Vec<u8> {
    let t = text.as_bytes();
    let mut i = 0;
    // prefix b / c / r
    while i < t.len() && (t[i] == b'b' || t[i] == b'c') {
        i += 1;
    }
    let raw = i < t.len() && t[i] == b'r';
    if raw {
        i += 1;
        let mut hashes = 0;
        while i < t.len() && t[i] == b'#' {
            hashes += 1;
            i += 1;
        }
        let start = i + 1;
        let end = t.len() - 1 - hashes;
        let end = end.max(start);
        // strip suffix-free closing quote
        let mut e = end;
        while e > start && t[e] != b'"' {
            e -= 1;
        }
        return t[start..e].to_vec();
    }
    let q = t[i];
    i += 1;
    let mut out = Vec::new();
    while i < t.len() && t[i] != q {
        if t[i] == b'\\' && i + 1 < t.len() {
            i += 1;
            match t[i] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'0' => out.push(0),
                b'\\' => out.push(b'\\'),
                b'\'' => out.push(b'\''),
                b'"' => out.push(b'"'),
                b'x' => {
                    let h = std::str::from_utf8(&t[i + 1..i + 3]).unwrap_or("0");
                    out.push(u8::from_str_radix(h, 16).unwrap_or(0));
                    i += 2;
                }
                b'u' => {
                    // \u{XXXX}
                    let mut j = i + 2;
                    let mut v = 0u32;
                    while j < t.len() && t[j] != b'}' {
                        if t[j] != b'_' {
                            v = v * 16 + (t[j] as char).to_digit(16).unwrap_or(0);
                        }
                        j += 1;
                    }
                    let c = char::from_u32(v).unwrap_or('\u{fffd}');
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    i = j;
                }
                b'\n' => {
                    // line continuation: skip whitespace
                    while i + 1 < t.len() && (t[i + 1] == b' ' || t[i + 1] == b'\n' || t[i + 1] == b'\t' || t[i + 1] == b'\r') {
                        i += 1;
                    }
                }
                c => out.push(c),
            }
            i += 1;
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------- format strings

#[derive(Clone, Copy, Default)]
pub struct FmtSpec {
    pub fill: u32,
    /// 0 none, 1 left, 2 center, 3 right
    pub align: u8,
    pub plus: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: u16,
    pub precision: Option<u16>,
    /// 0 Display, b'?' Debug, b'x', b'X', b'b', b'o', b'e'
    pub ty: u8,
}

impl FmtSpec {
    pub fn pack(&self) -> u64 {
        let mut v = 0u64;
        v |= self.width as u64;
        if let Some(p) = self.precision {
            v |= (p as u64) << 16;
            v |= 1 << 32;
        }
        v |= (self.align as u64) << 33;
        v |= (self.plus as u64) << 35;
        v |= (self.alt as u64) << 36;
        v |= (self.zero as u64) << 37;
        v |= (self.ty as u64) << 40;
        let fill = if self.fill == 0 { ' ' as u32 } else { self.fill };
        v |= (fill as u64 & 0x1f_ffff) << 48 >> 5 << 5;
        v
    }
}

pub enum FmtPiece {
    Lit(String),
    Arg { index: Option<usize>, named: Option<String>, spec: FmtSpec },
}

pub fn parse_fmt(s: &str) -> Vec<FmtPiece> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '{' {
            if i + 1 < c.len() && c[i + 1] == '{' {
                lit.push('{');
                i += 2;
                continue;
            }
            if !lit.is_empty() {
                out.push(FmtPiece::Lit(std::mem::take(&mut lit)));
            }
            let mut j = i + 1;
            let mut arg = String::new();
            while j < c.len() && c[j] != '}' && c[j] != ':' {
                arg.push(c[j]);
                j += 1;
            }
            let mut spec = FmtSpec::default();
            if j < c.len() && c[j] == ':' {
                j += 1;
                let mut sp = String::new();
                while j < c.len() && c[j] != '}' {
                    sp.push(c[j]);
                    j += 1;
                }
                spec = parse_spec(&sp);
            }
            let arg = arg.trim().to_string();
            let (index, named) = if arg.is_empty() {
                (None, None)
            } else if let Ok(n) = arg.parse::<usize>() {
                (Some(n), None)
            } else {
                (None, Some(arg))
            };
            out.push(FmtPiece::Arg { index, named, spec });
            i = j + 1;
        } else if c[i] == '}' {
            lit.push('}');
            i += if i + 1 < c.len() && c[i + 1] == '}' { 2 } else { 1 };
        } else {
            lit.push(c[i]);
            i += 1;
        }
    }
    if !lit.is_empty() {
        out.push(FmtPiece::Lit(lit));
    }
    out
}

fn parse_spec(s: &str) -> FmtSpec {
    let c: Vec<char> = s.chars().collect();
    let mut sp = FmtSpec::default();
    let mut i = 0;
    let is_align = |x: char| x == '<' || x == '^' || x == '>';
    if c.len() >= 2 && is_align(c[1]) {
        sp.fill = c[0] as u32;
        sp.align = match c[1] {
            '<' => 1,
            '^' => 2,
            _ => 3,
        };
        i = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        sp.align = match c[0] {
            '<' => 1,
            '^' => 2,
            _ => 3,
        };
        i = 1;
    }
    if i < c.len() && c[i] == '+' {
        sp.plus = true;
        i += 1;
    }
    if i < c.len() && c[i] == '#' {
        sp.alt = true;
        i += 1;
    }
    if i < c.len() && c[i] == '0' {
        sp.zero = true;
        i += 1;
    }
    let mut w = 0u16;
    while i < c.len() && c[i].is_ascii_digit() {
        w = w * 10 + c[i].to_digit(10).unwrap() as u16;
        i += 1;
    }
    sp.width = w;
    if i < c.len() && c[i] == '.' {
        i += 1;
        let mut p = 0u16;
        while i < c.len() && c[i].is_ascii_digit() {
            p = p * 10 + c[i].to_digit(10).unwrap() as u16;
            i += 1;
        }
        sp.precision = Some(p);
    }
    if i < c.len() {
        sp.ty = c[i] as u8;
    }
    sp
}

pub fn closure_map(_b: &Body) -> HashMap<u32, u32> {
    HashMap::new()
}
