//! Type context: item signatures, ADT fields, the inherent/trait impl index and
//! lowering of syntactic types (`ast::Ty`) to interned types.

use crate::ast::{self, GenericArg, GenericArgs, GenericParamKind, ItemKind, SegKind, TyId as AstTy, VariantData};
use crate::program::{DefId, DefKind, Prim, Program, Sym, NO_DEF};
use crate::types::{TyId, TyKind, Types};
use std::collections::HashMap;

/// Key for impl lookup: the outer type constructor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TyKey {
    Prim(Prim),
    Adt(DefId),
    Slice,
    Array,
    Str,
    Ref,
    Ptr,
    Tuple,
    FnPtr,
    Other,
}

#[derive(Clone)]
pub struct FnSigT {
    pub params: Vec<TyId>,
    pub ret: TyId,
    /// 0 none, 1 self by value, 2 &self, 3 &mut self
    pub self_kind: u8,
    /// generic params of the owner impl/trait followed by the fn's own
    pub n_generics: u32,
    pub n_parent_generics: u32,
    pub is_unsafe: bool,
    pub c_abi: bool,
    pub variadic: bool,
}

#[derive(Clone)]
pub struct FieldT {
    pub name: Sym,
    pub ty: TyId,
    pub is_pub: bool,
}

#[derive(Clone)]
pub struct VariantT {
    pub def: DefId,
    pub name: Sym,
    /// 0 unit, 1 tuple, 2 struct
    pub shape: u8,
    pub fields: Vec<FieldT>,
    pub disc: i128,
}

#[derive(Clone)]
pub struct AdtT {
    pub is_enum: bool,
    pub is_union: bool,
    pub n_generics: u32,
    pub variants: Vec<VariantT>,
    pub repr_c: bool,
    pub repr_int: Option<Prim>,
}

/// Generic parameter environment while lowering types.
#[derive(Clone, Default)]
pub struct GenEnv {
    pub names: Vec<Sym>,
    pub self_ty: Option<TyId>,
    /// inside a trait (its items see `Self::Assoc` as projections)
    pub trait_def: Option<DefId>,
    /// inside an impl (its items see `Self::Assoc` as the impl's own types)
    pub impl_def: Option<DefId>,
    /// `impl Trait` in argument position: (ast type id, the anonymous generic param it is)
    pub impl_params: Vec<(u32, u32)>,
}

pub struct Tcx {
    pub tys: Types,
    pub sigs: HashMap<DefId, FnSigT>,
    pub adts: HashMap<DefId, AdtT>,
    pub const_tys: HashMap<DefId, TyId>,
    pub impl_self: HashMap<DefId, TyId>,
    pub impl_trait: HashMap<DefId, DefId>,
    pub impl_generics: HashMap<DefId, GenEnv>,
    pub inherent: HashMap<TyKey, Vec<DefId>>,
    pub trait_impls: HashMap<(TyKey, DefId), Vec<DefId>>,
    /// trait impls by self type constructor
    pub impls_by_key: HashMap<TyKey, Vec<DefId>>,
    /// trait arguments of each trait impl (in the impl's generic environment, defaults filled)
    pub impl_trait_args: HashMap<DefId, Vec<TyId>>,
    pub alias_tys: HashMap<DefId, TyId>,
    /// per fn: generic params bounded by Fn/FnMut/FnOnce, with the bound's signature as a
    /// FnPtr over the fn's generics (guides closure inference, checks callee signatures)
    pub fn_bounds: HashMap<DefId, Vec<(u32, TyId)>>,
    pub errors: Vec<String>,
}

pub fn ty_key(tys: &Types, t: TyId) -> TyKey {
    match tys.kind(t) {
        TyKind::Bool => TyKey::Prim(Prim::Bool),
        TyKind::Char => TyKey::Prim(Prim::Char),
        TyKind::Int(p) | TyKind::Float(p) => TyKey::Prim(*p),
        TyKind::Str => TyKey::Str,
        TyKind::Adt(d, _) => TyKey::Adt(*d),
        TyKind::Slice(_) => TyKey::Slice,
        TyKind::Array(..) => TyKey::Array,
        TyKind::Ref(..) => TyKey::Ref,
        TyKind::Ptr(..) => TyKey::Ptr,
        TyKind::Tuple(_) => TyKey::Tuple,
        TyKind::FnPtr(..) => TyKey::FnPtr,
        _ => TyKey::Other,
    }
}

impl Tcx {
    pub fn new() -> Tcx {
        Tcx {
            tys: Types::new(),
            sigs: HashMap::new(),
            adts: HashMap::new(),
            const_tys: HashMap::new(),
            impl_self: HashMap::new(),
            impl_trait: HashMap::new(),
            impl_generics: HashMap::new(),
            inherent: HashMap::new(),
            trait_impls: HashMap::new(),
            impls_by_key: HashMap::new(),
            impl_trait_args: HashMap::new(),
            alias_tys: HashMap::new(),
            fn_bounds: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn err(&mut self, prog: &Program, file: u32, pos: u32, msg: String) {
        let (l, c) = crate::lexer::line_col(&prog.files[file as usize].src, pos);
        self.errors.push(format!("{}:{}:{}: {}", prog.file_paths[file as usize], l, c, msg));
    }

    /// Collects impls, ADTs and signatures of every item (bodies come later).
    pub fn collect(&mut self, prog: &Program) {
        // impls first: their self types key the index
        for ii in 0..prog.impls.len() {
            let im = &prog.impls[ii];
            let ast = &prog.files[im.file as usize].ast;
            let (generics, trait_, self_ty) = match &ast.item(im.item).kind {
                ItemKind::Impl { generics, trait_, self_ty, .. } => (generics, trait_, *self_ty),
                _ => continue,
            };
            let mut env = GenEnv::default();
            push_generics(prog, im.file, generics, &mut env);
            env.impl_def = Some(im.def);
            let st = self.lower_ty(prog, im.file, im.module, &env, self_ty);
            env.self_ty = Some(st);
            self.impl_self.insert(im.def, st);
            let key = ty_key(&self.tys, st);
            match trait_ {
                None => {
                    self.inherent.entry(key).or_default().push(im.def);
                }
                Some(tp) => {
                    if let Some(td) = self.resolve_type_path_def(prog, im.file, im.module, tp) {
                        self.impl_trait.insert(im.def, td);
                        self.trait_impls.entry((key, td)).or_default().push(im.def);
                        self.impls_by_key.entry(key).or_default().push(im.def);
                        let explicit = self.lower_generic_args(prog, im.file, im.module, &env, tp);
                        let targs = self.fill_trait_defaults(prog, td, st, explicit);
                        self.impl_trait_args.insert(im.def, targs);
                    }
                }
            }
            self.impl_generics.insert(im.def, env);
        }
        for di in 0..prog.defs.len() {
            let d = DefId(di as u32);
            let def = prog.def(d);
            match def.kind {
                DefKind::Struct | DefKind::Enum | DefKind::Union => self.collect_adt(prog, d),
                _ => {}
            }
        }
        for di in 0..prog.defs.len() {
            let d = DefId(di as u32);
            let def = prog.def(d);
            match def.kind {
                DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => self.collect_sig(prog, d),
                DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic => self.collect_const(prog, d),
                _ => {}
            }
        }
    }

    /// Module scope used to resolve names inside an item.
    pub fn item_module(&self, prog: &Program, d: DefId) -> u32 {
        prog.def(d).scope
    }

    /// Generic environment of an item (impl/trait generics first, then the item's own).
    pub fn item_env(&mut self, prog: &Program, d: DefId) -> GenEnv {
        let def = prog.def(d);
        let mut env = GenEnv::default();
        if def.parent != NO_DEF {
            let pd = prog.def(def.parent);
            if pd.kind == DefKind::Impl {
                if let Some(e) = self.impl_generics.get(&def.parent) {
                    env = e.clone();
                }
            } else if pd.kind == DefKind::Trait {
                let ast = &prog.files[pd.file as usize].ast;
                if let ItemKind::Trait { generics, .. } = &ast.item(pd.item).kind {
                    // `Self` is param 0 inside traits
                    env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    push_generics(prog, pd.file, generics, &mut env);
                    env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
                    env.trait_def = Some(def.parent);
                }
            }
        }
        let ast = &prog.files[def.file as usize].ast;
        let it = ast.item(def.item);
        match &it.kind {
            ItemKind::Fn(sig, _, _) => {
                push_generics(prog, def.file, &sig.generics, &mut env);
                // each `impl Trait` parameter type is an anonymous generic param
                for p in &sig.params {
                    let mut t = p.ty;
                    loop {
                        match ast.ty(t) {
                            ast::Ty::Ref(_, _, x) | ast::Ty::Paren(x) | ast::Ty::Ptr(_, x) => t = *x,
                            ast::Ty::ImplTrait(_) => {
                                env.impl_params.push((t.0, env.names.len() as u32));
                                env.names.push(u32::MAX);
                                break;
                            }
                            _ => break,
                        }
                    }
                }
            }
            ItemKind::Struct(g, _) | ItemKind::Enum(g, _) | ItemKind::Union(g, _) => push_generics(prog, def.file, g, &mut env),
            ItemKind::TypeAlias { generics, .. } => push_generics(prog, def.file, generics, &mut env),
            _ => {}
        }
        env
    }

    fn collect_adt(&mut self, prog: &Program, d: DefId) {
        let def = prog.def(d);
        let file = def.file;
        let module = self.item_module(prog, d);
        let mut env = self.item_env(prog, d);
        let n_generics = env.names.len() as u32;
        let self_args: Vec<TyId> = {
            let mut v = Vec::new();
            for i in 0..n_generics {
                v.push(self.tys.intern(TyKind::Param(i)));
            }
            v
        };
        env.self_ty = Some(self.tys.intern(TyKind::Adt(d, self_args)));
        let ast = &prog.files[file as usize].ast;
        let it = ast.item(def.item);
        let mut repr_c = false;
        let mut repr_int = None;
        read_repr(prog, file, &it.attrs, &mut repr_c, &mut repr_int);
        let mut adt = AdtT { is_enum: false, is_union: false, n_generics, variants: Vec::new(), repr_c, repr_int };
        match &it.kind {
            ItemKind::Struct(_, data) => {
                let (shape, fields) = self.lower_fields(prog, file, module, &env, data);
                adt.variants.push(VariantT { def: d, name: def.name, shape, fields, disc: 0 });
            }
            ItemKind::Union(_, fields) => {
                adt.is_union = true;
                let (_, f) = self.lower_fields(prog, file, module, &env, &VariantData::Struct(fields.clone()));
                adt.variants.push(VariantT { def: d, name: def.name, shape: 2, fields: f, disc: 0 });
            }
            ItemKind::Enum(_, vars) => {
                adt.is_enum = true;
                let mut next_disc: i128 = 0;
                for (vi, v) in vars.iter().enumerate() {
                    let (shape, fields) = self.lower_fields(prog, file, module, &env, &v.data);
                    let disc = match v.disc {
                        Some(e) => match crate::consteval::eval_int_literal(prog, file, e) {
                            Some(x) => x,
                            None => next_disc,
                        },
                        None => next_disc,
                    };
                    next_disc = disc + 1;
                    let vdef = prog.variants[def.sub as usize][vi];
                    adt.variants.push(VariantT { def: vdef, name: prog.def(vdef).name, shape, fields, disc });
                }
            }
            _ => {}
        }
        self.adts.insert(d, adt);
    }

    fn lower_fields(&mut self, prog: &Program, file: u32, module: u32, env: &GenEnv, data: &VariantData) -> (u8, Vec<FieldT>) {
        let mut out = Vec::new();
        let (shape, fields) = match data {
            VariantData::Unit => (0, None),
            VariantData::Tuple(f) => (1, Some(f)),
            VariantData::Struct(f) => (2, Some(f)),
        };
        if let Some(fs) = fields {
            for (i, f) in fs.iter().enumerate() {
                let ty = self.lower_ty(prog, file, module, env, f.ty);
                let name = match f.name {
                    Some(n) => {
                        let s = prog.files[file as usize].text(n);
                        prog.syms.get(s).unwrap_or(u32::MAX)
                    }
                    None => i as u32 | 0x8000_0000,
                };
                out.push(FieldT { name, ty, is_pub: !matches!(f.vis, ast::Vis::Private) });
            }
        }
        (shape, out)
    }

    fn collect_sig(&mut self, prog: &Program, d: DefId) {
        let def = prog.def(d);
        let file = def.file;
        let module = self.item_module(prog, d);
        let env = self.item_env(prog, d);
        let n_parent = match prog.def(def.parent).kind {
            DefKind::Impl => self.impl_generics.get(&def.parent).map_or(0, |e| e.names.len() as u32),
            DefKind::Trait => 1 + self.trait_generic_count(prog, def.parent) as u32,
            _ => 0,
        };
        let ast = &prog.files[file as usize].ast;
        let sig = match &ast.item(def.item).kind {
            ItemKind::Fn(s, _, _) => s,
            _ => return,
        };
        let mut params = Vec::new();
        let mut self_kind = 0;
        if let Some(sp) = &sig.self_param {
            let st = env.self_ty.unwrap_or(self.tys.error);
            let t = match sp {
                ast::SelfParam::Value(_) => {
                    self_kind = 1;
                    st
                }
                ast::SelfParam::Ref(_, m) => {
                    self_kind = if *m { 3 } else { 2 };
                    self.tys.intern(TyKind::Ref(*m, st))
                }
                ast::SelfParam::Typed(_, t) => {
                    self_kind = 1;
                    let lt = self.lower_ty(prog, file, module, &env, *t);
                    if let TyKind::Ref(m, _) = self.tys.kind(lt) {
                        self_kind = if *m { 3 } else { 2 };
                    }
                    lt
                }
            };
            params.push(t);
        }
        for p in &sig.params {
            params.push(self.lower_ty(prog, file, module, &env, p.ty));
        }
        let ret = match sig.ret {
            Some(r) => self.lower_ty(prog, file, module, &env, r),
            None => self.tys.unit,
        };
        let bounds = self.collect_fn_bounds(prog, file, module, &env, sig, n_parent);
        if !bounds.is_empty() {
            self.fn_bounds.insert(d, bounds);
        }
        let c_abi = def.kind == DefKind::ForeignFn || matches!(sig.abi, Some(Some(_)));
        self.sigs.insert(
            d,
            FnSigT {
                params,
                ret,
                self_kind,
                n_generics: env.names.len() as u32,
                n_parent_generics: n_parent,
                is_unsafe: sig.unsafe_,
                c_abi,
                variadic: sig.variadic,
            },
        );
    }

    /// Fn-family bounds on a fn's own generic params (inline, `where`, and `impl Fn(..)`).
    fn collect_fn_bounds(&mut self, prog: &Program, file: u32, module: u32, env: &GenEnv, sig: &ast::FnSig, n_parent: u32) -> Vec<(u32, TyId)> {
        let ast = &prog.files[file as usize].ast;
        let f = &prog.files[file as usize];
        let mut pending: Vec<(u32, &ast::Bound)> = Vec::new();
        let mut k = n_parent;
        for gp in &sig.generics.params {
            match &gp.kind {
                GenericParamKind::Type(bs, _) => {
                    for b in bs {
                        pending.push((k, b));
                    }
                    k += 1;
                }
                GenericParamKind::Const(..) => k += 1,
                GenericParamKind::Lifetime(_) => {}
            }
        }
        for w in &sig.generics.where_ {
            if let ast::WherePred::Bound { ty, bounds, .. } = w {
                if let ast::Ty::Path(p) = ast.ty(*ty) {
                    if p.segs.len() == 1 && p.segs[0].args.is_none() {
                        let name = prog.syms.get(f.text(p.segs[0].name)).unwrap_or(u32::MAX - 1);
                        if let Some(i) = env.names.iter().rposition(|n| *n == name) {
                            if i as u32 >= n_parent {
                                for b in bounds {
                                    pending.push((i as u32, b));
                                }
                            }
                        }
                    }
                }
            }
        }
        for &(t, i) in &env.impl_params {
            if let ast::Ty::ImplTrait(bs) = ast.ty(ast::TyId(t)) {
                for b in bs {
                    pending.push((i, b));
                }
            }
        }
        let mut out = Vec::new();
        for (i, b) in pending {
            if let ast::Bound::Trait { path, .. } = b {
                if let Some(last) = path.segs.last() {
                    let n = f.text(last.name);
                    if n == "Fn" || n == "FnMut" || n == "FnOnce" {
                        if let Some(a) = &last.args {
                            if let GenericArgs::Paren(ps, r) = &**a {
                                let mut pts = Vec::new();
                                for x in ps {
                                    pts.push(self.lower_ty(prog, file, module, env, *x));
                                }
                                let rt = match r {
                                    Some(r) => self.lower_ty(prog, file, module, env, *r),
                                    None => self.tys.unit,
                                };
                                out.push((i, self.tys.intern(TyKind::FnPtr(pts, rt))));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn collect_const(&mut self, prog: &Program, d: DefId) {
        let def = prog.def(d);
        let file = def.file;
        let module = self.item_module(prog, d);
        let env = self.item_env(prog, d);
        let ast = &prog.files[file as usize].ast;
        let t = match &ast.item(def.item).kind {
            ItemKind::Const(Some(t), _) => *t,
            ItemKind::Static(_, t, _) => *t,
            _ => return,
        };
        let ty = self.lower_ty(prog, file, module, &env, t);
        self.const_tys.insert(d, ty);
    }

    // ------------------------------------------------------------ type lowering

    pub fn lower_ty(&mut self, prog: &Program, file: u32, module: u32, env: &GenEnv, t: AstTy) -> TyId {
        let ast = &prog.files[file as usize].ast;
        match ast.ty(t) {
            ast::Ty::Tuple(v) => {
                let mut out = Vec::new();
                for x in v {
                    out.push(self.lower_ty(prog, file, module, env, *x));
                }
                self.tys.intern(TyKind::Tuple(out))
            }
            ast::Ty::Paren(x) => self.lower_ty(prog, file, module, env, *x),
            ast::Ty::Never => self.tys.never,
            ast::Ty::Ref(_, m, x) => {
                let inner = self.lower_ty(prog, file, module, env, *x);
                self.tys.intern(TyKind::Ref(*m, inner))
            }
            ast::Ty::Ptr(m, x) => {
                let inner = self.lower_ty(prog, file, module, env, *x);
                self.tys.intern(TyKind::Ptr(*m, inner))
            }
            ast::Ty::Slice(x) => {
                let inner = self.lower_ty(prog, file, module, env, *x);
                self.tys.intern(TyKind::Slice(inner))
            }
            ast::Ty::Array(x, n) => {
                let inner = self.lower_ty(prog, file, module, env, *x);
                let len = match crate::consteval::eval_usize_expr(prog, self, file, module, *n) {
                    Some(v) => v,
                    None => {
                        let lo = ast.expr(*n).lo;
                        self.err(prog, file, lo, "array length must be a constant".to_string());
                        0
                    }
                };
                self.tys.intern(TyKind::Array(inner, len))
            }
            ast::Ty::Fn(f) => {
                let mut ps = Vec::new();
                for x in &f.params {
                    ps.push(self.lower_ty(prog, file, module, env, *x));
                }
                let r = match f.ret {
                    Some(r) => self.lower_ty(prog, file, module, env, r),
                    None => self.tys.unit,
                };
                self.tys.intern(TyKind::FnPtr(ps, r))
            }
            ast::Ty::Infer => self.tys.error,
            ast::Ty::Path(p) => self.lower_path_ty(prog, file, module, env, p),
            ast::Ty::DynTrait(bounds, _) => self.lower_dyn(prog, file, module, env, bounds, 0),
            ast::Ty::ImplTrait(_) if env.impl_params.iter().any(|x| x.0 == t.0) => {
                let i = env.impl_params.iter().find(|x| x.0 == t.0).unwrap().1;
                self.tys.intern(TyKind::Param(i))
            }
            _ => {
                let lo = match ast.ty(t) {
                    _ => 0,
                };
                self.err(prog, file, lo, "unsupported type (dyn/impl/macro)".to_string());
                self.tys.error
            }
        }
    }

    /// `dyn Principal<..> + AutoTraits + 'a`
    fn lower_dyn(&mut self, prog: &Program, file: u32, module: u32, env: &GenEnv, bounds: &[ast::Bound], lo: u32) -> TyId {
        let f = &prog.files[file as usize];
        for b in bounds {
            let path = match b {
                ast::Bound::Trait { path, .. } => path,
                _ => continue,
            };
            let td = match self.resolve_type_path_def(prog, file, module, path) {
                Some(d) if prog.def(d).kind == DefKind::Trait => d,
                _ => continue,
            };
            let name = prog.name(td);
            if matches!(name, "Send" | "Sync" | "Sized" | "Unpin" | "UnwindSafe" | "RefUnwindSafe") && prog.def(td).krate == self.core_crate(prog) {
                continue;
            }
            let last = path.segs.last().unwrap();
            let mut args = Vec::new();
            let mut binds = Vec::new();
            match last.args.as_deref() {
                Some(GenericArgs::Paren(ps, r)) => {
                    let mut pts = Vec::new();
                    for x in ps {
                        pts.push(self.lower_ty(prog, file, module, env, *x));
                    }
                    let rt = match r {
                        Some(r) => self.lower_ty(prog, file, module, env, *r),
                        None => self.tys.unit,
                    };
                    args.push(self.tys.intern(TyKind::FnPtr(pts, rt)));
                }
                Some(GenericArgs::Angle(v)) => {
                    for g in v {
                        match g {
                            GenericArg::Type(t) => args.push(self.lower_ty(prog, file, module, env, *t)),
                            GenericArg::Binding(n, _, t) => {
                                let sym = prog.syms.get(f.text(*n)).unwrap_or(u32::MAX);
                                if let Some(ad) = self.trait_assoc(prog, td, sym) {
                                    let bt = self.lower_ty(prog, file, module, env, *t);
                                    binds.push((ad, bt));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                None => {}
            }
            binds.sort_by_key(|b| b.0 .0);
            return self.tys.intern(TyKind::Dyn(td, args, binds));
        }
        self.err(prog, file, lo, "trait object without a principal trait".to_string());
        self.tys.error
    }

    fn core_crate(&self, prog: &Program) -> u32 {
        prog.prelude_crate.unwrap_or(u32::MAX)
    }

    pub fn lower_path_ty(&mut self, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path) -> TyId {
        let f = &prog.files[file as usize];
        // `<T as Trait>::Name`
        if let Some(q) = &p.qself {
            let self_t = self.lower_ty(prog, file, module, env, q.ty);
            if let Some(tp) = &q.trait_path {
                if let Some(td) = self.resolve_type_path_def(prog, file, module, tp) {
                    let name = prog.syms.get(f.text(p.segs[p.segs.len() - 1].name)).unwrap_or(u32::MAX);
                    if let Some(ad) = self.trait_assoc(prog, td, name) {
                        let explicit = self.lower_generic_args(prog, file, module, env, tp);
                        let mut args = vec![self_t];
                        args.extend(self.fill_trait_defaults(prog, td, self_t, explicit));
                        return self.tys.intern(TyKind::Assoc(ad, args));
                    }
                }
            }
            self.err(prog, file, p.lo, "unsupported qualified type path".to_string());
            return self.tys.error;
        }
        // `Self::Name`: inside an impl the impl's own associated type, inside a trait a projection
        if p.segs.len() == 2 && p.segs[0].kind == SegKind::SelfType {
            let name = prog.syms.get(f.text(p.segs[1].name)).unwrap_or(u32::MAX);
            if let Some(imp) = env.impl_def {
                if let Some(t) = self.impl_assoc_ty(prog, imp, name) {
                    return t;
                }
            }
            if let Some(td) = env.trait_def {
                if let Some(ad) = self.trait_assoc(prog, td, name) {
                    let mut args = Vec::new();
                    for i in 0..1 + self.trait_generic_count(prog, td) {
                        args.push(self.tys.intern(TyKind::Param(i as u32)));
                    }
                    return self.tys.intern(TyKind::Assoc(ad, args));
                }
            }
        }
        if p.qself.is_none() && p.segs.len() == 1 {
            let seg = &p.segs[0];
            if seg.kind == SegKind::SelfType {
                return env.self_ty.unwrap_or(self.tys.error);
            }
            let name = f.text(seg.name);
            if let Some(s) = prog.syms.get(name) {
                for (i, n) in env.names.iter().enumerate() {
                    if *n == s {
                        return self.tys.intern(TyKind::Param(i as u32));
                    }
                }
            }
        }
        let d = match self.resolve_type_path_def(prog, file, module, p) {
            Some(d) => d,
            None => {
                self.err(prog, file, p.lo, format!("unresolved type `{}`", path_str(prog, file, p)));
                return self.tys.error;
            }
        };
        let args = self.lower_generic_args(prog, file, module, env, p);
        let def = prog.def(d);
        match def.kind {
            DefKind::Prim(pr) => self.tys.prim(pr),
            DefKind::Struct | DefKind::Enum | DefKind::Union => self.tys.intern(TyKind::Adt(d, args)),
            DefKind::TypeAlias => {
                let a = self.alias_ty(prog, d);
                self.tys.subst(a, &args)
            }
            _ => {
                self.err(prog, file, p.lo, format!("`{}` is not a type", path_str(prog, file, p)));
                self.tys.error
            }
        }
    }

    pub fn alias_ty(&mut self, prog: &Program, d: DefId) -> TyId {
        if let Some(&t) = self.alias_tys.get(&d) {
            return t;
        }
        self.alias_tys.insert(d, self.tys.error);
        let def = prog.def(d);
        let module = self.item_module(prog, d);
        let env = self.item_env(prog, d);
        let ast = &prog.files[def.file as usize].ast;
        let t = match &ast.item(def.item).kind {
            ItemKind::TypeAlias { ty: Some(t), .. } => self.lower_ty(prog, def.file, module, &env, *t),
            _ => self.tys.error,
        };
        self.alias_tys.insert(d, t);
        t
    }

    pub fn lower_generic_args(&mut self, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path) -> Vec<TyId> {
        let mut out = Vec::new();
        if let Some(last) = p.segs.last() {
            if let Some(a) = &last.args {
                if let GenericArgs::Angle(v) = &**a {
                    for g in v {
                        if let GenericArg::Type(t) = g {
                            out.push(self.lower_ty(prog, file, module, env, *t));
                        }
                    }
                }
            }
        }
        out
    }

    /// Resolves a path in the type namespace to a def.
    pub fn resolve_type_path_def(&self, prog: &Program, file: u32, module: u32, p: &ast::Path) -> Option<DefId> {
        let segs = path_segs(prog, file, p);
        if segs.is_empty() {
            return None;
        }
        if segs.len() == 1 && !p.global {
            if segs[0].0 != SegKind::Ident {
                return prog.resolve_mod_path(module, false, &segs, false, false);
            }
            return prog.lookup_in_scope(module, segs[0].1, true);
        }
        let parent = prog.resolve_mod_path(module, p.global, &segs[..segs.len() - 1], false, false)?;
        prog.lookup_in_container(parent, segs[segs.len() - 1].1, true)
    }

    /// Inherent associated item `name` on type `t`: (item def, impl def).
    pub fn inherent_item(&self, prog: &Program, t: TyId, name: Sym) -> Option<(DefId, DefId)> {
        let key = ty_key(&self.tys, t);
        if let Some(list) = self.inherent.get(&key) {
            for &imp in list {
                let ii = prog.def(imp).sub as usize;
                for &it in &prog.impls[ii].items {
                    if prog.def(it).name == name {
                        // non-generic impls must match exactly (e.g. impl Foo<u8> vs Foo<u16>)
                        if let Some(&st) = self.impl_self.get(&imp) {
                            if !self.impl_matches(st, t) {
                                continue;
                            }
                        }
                        return Some((it, imp));
                    }
                }
            }
        }
        None
    }

    /// Could impl self type `pat` (with params) match `t`? Shallow structural check.
    pub fn impl_matches(&self, pat: TyId, t: TyId) -> bool {
        if pat == t {
            return true;
        }
        match (self.tys.kind(pat), self.tys.kind(t)) {
            (TyKind::Param(_), _) => true,
            (_, TyKind::Infer(_)) => true,
            (TyKind::Adt(a, pa), TyKind::Adt(b, ta)) => {
                if a != b || pa.len() != ta.len() {
                    return false;
                }
                for i in 0..pa.len() {
                    if !self.impl_matches(pa[i], ta[i]) {
                        return false;
                    }
                }
                true
            }
            (TyKind::Ref(m1, a), TyKind::Ref(m2, b)) => m1 == m2 && self.impl_matches(*a, *b),
            (TyKind::Slice(a), TyKind::Slice(b)) => self.impl_matches(*a, *b),
            (TyKind::Array(a, n1), TyKind::Array(b, n2)) => n1 == n2 && self.impl_matches(*a, *b),
            (TyKind::Ptr(m1, a), TyKind::Ptr(m2, b)) => m1 == m2 && self.impl_matches(*a, *b),
            (TyKind::Tuple(a), TyKind::Tuple(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                for i in 0..a.len() {
                    if !self.impl_matches(a[i], b[i]) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }
}

fn read_repr(prog: &Program, file: u32, attrs: &[ast::Attr], repr_c: &mut bool, repr_int: &mut Option<Prim>) {
    let f = &prog.files[file as usize];
    for a in attrs {
        if a.toks.hi <= a.toks.lo || f.tok_text(a.toks.lo) != "repr" {
            continue;
        }
        for i in a.toks.lo + 1..a.toks.hi {
            let t = f.tok_text(i);
            if t == "C" {
                *repr_c = true;
            }
            for (n, p) in crate::program::PRIMS {
                if n == t {
                    *repr_int = Some(p);
                }
            }
        }
    }
}

pub fn push_generics(prog: &Program, file: u32, g: &ast::Generics, env: &mut GenEnv) {
    let f = &prog.files[file as usize];
    for p in &g.params {
        match p.kind {
            GenericParamKind::Type(..) | GenericParamKind::Const(..) => {
                env.names.push(prog.syms.get(f.text(p.name)).unwrap_or(u32::MAX));
            }
            GenericParamKind::Lifetime(_) => {}
        }
    }
}

pub fn path_segs(prog: &Program, file: u32, p: &ast::Path) -> Vec<(SegKind, Sym)> {
    let f = &prog.files[file as usize];
    let mut v = Vec::new();
    for s in &p.segs {
        let t = f.text(s.name);
        let t = match t.strip_prefix("r#") {
            Some(x) => x,
            None => t,
        };
        v.push((s.kind, prog.syms.get(t).unwrap_or(u32::MAX)));
    }
    v
}

pub fn path_str(prog: &Program, file: u32, p: &ast::Path) -> String {
    let f = &prog.files[file as usize];
    let mut s = String::new();
    for (i, seg) in p.segs.iter().enumerate() {
        if i > 0 {
            s.push_str("::");
        }
        s.push_str(f.text(seg.name));
    }
    s
}

// ---------------------------------------------------------------- traits

impl Tcx {
    pub fn trait_generic_count(&self, prog: &Program, td: DefId) -> usize {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        if let ItemKind::Trait { generics, .. } = &ast.item(pd.item).kind {
            let mut n = 0;
            for g in &generics.params {
                if !matches!(g.kind, GenericParamKind::Lifetime(_)) {
                    n += 1;
                }
            }
            return n;
        }
        0
    }

    /// Trait args with defaults filled (`Rhs = Self` etc.).
    pub fn fill_trait_defaults(&mut self, prog: &Program, td: DefId, self_t: TyId, explicit: Vec<TyId>) -> Vec<TyId> {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        let mut out = explicit;
        let generics = match &ast.item(pd.item).kind {
            ItemKind::Trait { generics, .. } => generics.clone(),
            _ => return out,
        };
        let mut k = 0;
        for g in &generics.params {
            if let GenericParamKind::Type(_, def) = &g.kind {
                if k >= out.len() {
                    let t = match def {
                        Some(dt) => {
                            let mut env = GenEnv::default();
                            env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                            push_generics(prog, pd.file, &generics, &mut env);
                            env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
                            let lt = self.lower_ty(prog, pd.file, pd.scope, &env, *dt);
                            let mut sub = vec![self_t];
                            sub.extend(out.iter().copied());
                            self.tys.subst(lt, &sub)
                        }
                        None => self.tys.error,
                    };
                    out.push(t);
                }
                k += 1;
            }
        }
        out
    }

    /// Associated type `name` of trait `td` or (transitively) its supertraits.
    pub fn trait_assoc(&self, prog: &Program, td: DefId, name: Sym) -> Option<DefId> {
        for &it in &prog.traits[prog.def(td).sub as usize] {
            let d = prog.def(it);
            if d.name == name && d.kind == DefKind::AssocTy {
                return Some(it);
            }
        }
        for sup in self.trait_supers(prog, td) {
            if sup != td {
                if let Some(x) = self.trait_assoc(prog, sup, name) {
                    return Some(x);
                }
            }
        }
        None
    }

    /// Direct supertraits of a trait (resolved from its `: A + B` bounds).
    pub fn trait_supers(&self, prog: &Program, td: DefId) -> Vec<DefId> {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        let mut out = Vec::new();
        if let ItemKind::Trait { supers, .. } = &ast.item(pd.item).kind {
            for b in supers {
                if let ast::Bound::Trait { path, .. } = b {
                    if let Some(d) = self.resolve_type_path_def(prog, pd.file, pd.scope, path) {
                        if prog.def(d).kind == DefKind::Trait {
                            out.push(d);
                        }
                    }
                }
            }
        }
        out
    }

    /// An impl's `type Name = T;` lowered in the impl's environment.
    pub fn impl_assoc_ty(&mut self, prog: &Program, imp: DefId, name: Sym) -> Option<TyId> {
        let ii = prog.def(imp).sub as usize;
        for &it in &prog.impls[ii].items {
            let d = prog.def(it);
            if d.name == name && d.kind == DefKind::AssocTy {
                let env = match self.impl_generics.get(&imp) {
                    Some(e) => e.clone(),
                    None => GenEnv::default(),
                };
                let ast = &prog.files[d.file as usize].ast;
                if let ItemKind::TypeAlias { ty: Some(t), .. } = &ast.item(d.item).kind {
                    return Some(self.lower_ty(prog, d.file, d.scope, &env, *t));
                }
            }
        }
        None
    }

    /// Structural match of an impl pattern (with impl params) against a concrete type.
    pub fn match_ty(&self, pat: TyId, t: TyId, binds: &mut Vec<Option<TyId>>) -> bool {
        if pat == t {
            return true;
        }
        match (self.tys.kind(pat), self.tys.kind(t)) {
            (TyKind::Param(i), _) => {
                let i = *i as usize;
                if i >= binds.len() {
                    binds.resize(i + 1, None);
                }
                match binds[i] {
                    Some(b) => b == t,
                    None => {
                        binds[i] = Some(t);
                        true
                    }
                }
            }
            (TyKind::Adt(a, pa), TyKind::Adt(b, ta)) if a == b && pa.len() == ta.len() => {
                for k in 0..pa.len() {
                    if !self.match_ty(pa[k], ta[k], binds) {
                        return false;
                    }
                }
                true
            }
            (TyKind::Tuple(pa), TyKind::Tuple(ta)) if pa.len() == ta.len() => {
                for k in 0..pa.len() {
                    if !self.match_ty(pa[k], ta[k], binds) {
                        return false;
                    }
                }
                true
            }
            (TyKind::Ref(m1, a), TyKind::Ref(m2, b)) if m1 == m2 => self.match_ty(*a, *b, binds),
            (TyKind::Ptr(m1, a), TyKind::Ptr(m2, b)) if m1 == m2 => self.match_ty(*a, *b, binds),
            (TyKind::Slice(a), TyKind::Slice(b)) => self.match_ty(*a, *b, binds),
            (TyKind::Array(a, n), TyKind::Array(b, m)) if n == m => self.match_ty(*a, *b, binds),
            _ => false,
        }
    }

    /// The impl of trait `td` for `self_t` with trait args `targs`, and the impl's args.
    pub fn find_impl(&mut self, td: DefId, self_t: TyId, targs: &[TyId]) -> Option<(DefId, Vec<TyId>)> {
        let key = ty_key(&self.tys, self_t);
        let list = self.trait_impls.get(&(key, td))?.clone();
        for imp in list {
            let pat = *self.impl_self.get(&imp)?;
            let n = self.impl_generics.get(&imp).map_or(0, |e| e.names.len());
            let mut binds: Vec<Option<TyId>> = vec![None; n];
            if !self.match_ty(pat, self_t, &mut binds) {
                continue;
            }
            let itargs = self.impl_trait_args.get(&imp).cloned().unwrap_or_default();
            let mut ok = true;
            for k in 0..itargs.len().min(targs.len()) {
                if !self.tys.is_concrete(targs[k]) {
                    continue;
                }
                if !self.match_ty(itargs[k], targs[k], &mut binds) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            let mut args = Vec::new();
            for b in binds {
                args.push(b.unwrap_or(self.tys.error));
            }
            return Some((imp, args));
        }
        None
    }

    /// Replaces resolvable associated type projections by the impl's types.
    pub fn normalize(&mut self, prog: &Program, t: TyId) -> TyId {
        let k = self.tys.kind(t).clone();
        match k {
            TyKind::Assoc(ad, args) => {
                let mut n = Vec::new();
                for x in &args {
                    n.push(self.normalize(prog, *x));
                }
                if let Some(TyKind::Dyn(_, _, bs)) = n.first().map(|x| self.tys.kind(*x).clone()) {
                    if let Some(b) = bs.iter().find(|b| b.0 == ad) {
                        return b.1;
                    }
                }
                if !n.is_empty() && self.tys.is_concrete(n[0]) {
                    let td = prog.def(ad).parent;
                    if let Some((imp, iargs)) = self.find_impl(td, n[0], &n[1..]) {
                        let name = prog.def(ad).name;
                        if let Some(at) = self.impl_assoc_ty(prog, imp, name) {
                            let r = self.tys.subst(at, &iargs);
                            return self.normalize(prog, r);
                        }
                    }
                }
                self.tys.intern(TyKind::Assoc(ad, n))
            }
            TyKind::Tuple(v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.normalize(prog, x));
                }
                self.tys.intern(TyKind::Tuple(n))
            }
            TyKind::Adt(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.normalize(prog, x));
                }
                self.tys.intern(TyKind::Adt(d, n))
            }
            TyKind::Ref(m, e) => {
                let e = self.normalize(prog, e);
                self.tys.intern(TyKind::Ref(m, e))
            }
            TyKind::Ptr(m, e) => {
                let e = self.normalize(prog, e);
                self.tys.intern(TyKind::Ptr(m, e))
            }
            TyKind::Slice(e) => {
                let e = self.normalize(prog, e);
                self.tys.intern(TyKind::Slice(e))
            }
            TyKind::Array(e, l) => {
                let e = self.normalize(prog, e);
                self.tys.intern(TyKind::Array(e, l))
            }
            TyKind::Dyn(d, v, bs) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.normalize(prog, x));
                }
                let mut nb = Vec::new();
                for (a, x) in bs {
                    nb.push((a, self.normalize(prog, x)));
                }
                self.tys.intern(TyKind::Dyn(d, n, nb))
            }
            _ => t,
        }
    }

    /// A call to trait method `d` with args [Self, trait args.., own..] -> the concrete
    /// function to run: the impl's method, or the trait's default body.
    pub fn resolve_trait_method(&mut self, prog: &Program, d: DefId, args: &[TyId]) -> (DefId, Vec<TyId>) {
        let td = prog.def(d).parent;
        if prog.def(td).kind != DefKind::Trait || args.is_empty() {
            return (d, args.to_vec());
        }
        let nt = self.trait_generic_count(prog, td);
        let self_t = args[0];
        let targs: Vec<TyId> = args[1..(1 + nt).min(args.len())].to_vec();
        let own: Vec<TyId> = if args.len() > 1 + nt { args[1 + nt..].to_vec() } else { Vec::new() };
        if let Some((imp, iargs)) = self.find_impl(td, self_t, &targs) {
            let name = prog.def(d).name;
            let ii = prog.def(imp).sub as usize;
            for &it in &prog.impls[ii].items {
                if prog.def(it).name == name && prog.def(it).kind == DefKind::AssocFn {
                    let mut a = iargs.clone();
                    a.extend(own.iter().copied());
                    return (it, a);
                }
            }
        }
        (d, args.to_vec())
    }

    /// Trait methods named `name` callable on `t` (via trait impls for its type constructor):
    /// (trait method def, impl def).
    pub fn trait_methods_for(&self, prog: &Program, t: TyId, name: Sym) -> Vec<(DefId, DefId)> {
        let key = ty_key(&self.tys, t);
        let mut out = Vec::new();
        if let Some(list) = self.impls_by_key.get(&key) {
            for &imp in list {
                if let Some(&pat) = self.impl_self.get(&imp) {
                    if !self.impl_matches(pat, t) {
                        continue;
                    }
                }
                let td = match self.impl_trait.get(&imp) {
                    Some(t) => *t,
                    None => continue,
                };
                for &it in &prog.traits[prog.def(td).sub as usize] {
                    if prog.def(it).name == name && prog.def(it).kind == DefKind::AssocFn {
                        out.push((it, imp));
                    }
                }
            }
        }
        out
    }
}
