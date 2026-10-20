use crate::program::Program;
// ported from N:rustc_borrowck/src/universal_regions.rs @8d1a764
use super::constraints::RegionVid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionClassification { Global, External, Local }
/// The input adapter numbers global, external and local universals before
/// existential variables, preserving nightly's representative ordering.
#[derive(Clone, Debug)]
pub struct UniversalRegions {
    pub fr_static: RegionVid,
    pub fr_fn_body: Option<RegionVid>,
    regions: Vec<(RegionVid, RegionClassification)>,
    /// UniversalRegionIndices::indices: the universal regions by their original region.
    pub indices: Vec<(crate::types::RegionId, RegionVid)>,
}
impl UniversalRegions {
    pub fn new(fr_static: RegionVid, fr_fn_body: Option<RegionVid>, regions: Vec<(RegionVid, RegionClassification)>) -> Self {
        assert!(regions.contains(&(fr_static, RegionClassification::Global)));
        Self { fr_static, fr_fn_body, regions, indices: Vec::new() }
    }
    /// universal_regions.rs to_region_vid: region variables are themselves, universal
    /// regions map through their indices.
    pub fn to_region_vid(&self, tys: &crate::types::Types, r: crate::types::RegionId) -> RegionVid {
        match tys.region_kind(r) {
            crate::types::RegionKind::ReVar(v) => v as RegionVid,
            crate::types::RegionKind::ReStatic | crate::types::RegionKind::ReError => self.fr_static,
            _ => self.indices.iter().find(|(o, _)| *o == r).map(|(_, v)| *v).unwrap_or_else(|| panic!("cannot convert `{:?}` to a region vid", tys.region_kind(r))),
        }
    }
    pub fn implicit_region_bound(&self) -> Option<RegionVid> { self.fr_fn_body }
    pub fn universal_regions_iter(&self) -> impl Iterator<Item=RegionVid> + '_ { self.regions.iter().map(|r| r.0) }
    pub fn region_classification(&self, r: RegionVid) -> Option<RegionClassification> {
        self.regions.iter().find(|v| v.0 == r).map(|v| v.1)
    }
    pub fn is_universal_region(&self, r: RegionVid) -> bool { self.region_classification(r).is_some() }
    pub fn is_local_free_region(&self, r: RegionVid) -> bool { self.region_classification(r) == Some(RegionClassification::Local) }
    pub fn is_external_free_region(&self, r: RegionVid) -> bool { self.region_classification(r) == Some(RegionClassification::External) }
    pub fn num_global_and_external_regions(&self) -> usize { self.regions.iter().filter(|r| r.1 != RegionClassification::Local).count() }
}

use crate::program::DefId;
use crate::tcx::{Tcx, outlives::RegionContract};
use crate::types::{TyId, TyKind, RegionId, RegionKind, RegionScope, LateParamKind, BoundVar};
use crate::types::fold::{BoundVarReplacer, TypeFolder};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Identity { Static, Region(RegionScope, RegionId), FnBody(RegionScope) }
#[derive(Clone, Debug)]
pub struct UniversalRegion { pub identity: Identity, pub class: RegionClassification, pub name: String }
/// Typed declaration input for the BIR projection. The actual region-variable
/// numbering is provided by the producer until C0b renumbering is available.
#[derive(Clone, Debug)]
pub struct SignatureRegions {
    pub regions: Vec<UniversalRegion>,
    pub inputs: Vec<TyId>, pub output: TyId, pub implicit_input: Option<TyId>,
    pub contract: RegionContract, pub fr_static: usize, pub fr_fn_body: usize,
    pub query_placeholders: Vec<RegionId>,
}
enum DefiningTy { FnDef(DefId, bool), Const(DefId), AnonConst(DefId), Closure { owner: DefId, expr: u32, signature: TyId, bound: TyId, annotation_eq: bool } }
impl DefiningTy {
    fn inputs_and_output(&self, tcx: &Tcx, prog: &Program) -> (Vec<TyId>, TyId, u32) {
        match *self {
            Self::FnDef(d, body) => {
                let sig = tcx.fn_item_sig(prog, d).unwrap();
                let mut inputs = sig.params.clone();
                let mut bound_regions = sig.bound_regions;
                if body && sig.header.c_variadic {
                    let region = tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(bound_regions)));
                    bound_regions += 1;
                    // (a core without the `va_list` lang item has no value for the `...` argument)
                    if let Some(va_list) = tcx.lang_items.get(crate::lang_items::LangItem::VaList) {
                        inputs.push(tcx.tys.intern(TyKind::Adt(va_list, tcx.tys.mk_list(&[]), tcx.tys.intern_region_args(&[region]))));
                    }
                }
                (inputs, sig.ret, bound_regions)
            }
            Self::Const(d) => (vec![],tcx.const_ty(prog, d).unwrap(),0),
            Self::AnonConst(_) => (vec![],tcx.tys.usize_,0),
            Self::Closure { signature, bound, .. } => {
                let TyKind::FnPtr(ins,out,_,_) = tcx.tys.kind(signature) else { unreachable!() };
                let TyKind::FnPtr(_,_,_,n) = tcx.tys.kind(bound) else { unreachable!() };
                (tcx.tys.list(*ins).to_vec(),*out,*n)
            }
        }
    }
    fn scope(&self) -> RegionScope {
        match *self { Self::FnDef(d, _) | Self::Const(d) | Self::AnonConst(d) => RegionScope::Item(d), Self::Closure { owner,expr,.. } => RegionScope::Closure { owner,expr } }
    }
}
struct UniversalRegionIndices { regions: Vec<UniversalRegion> }
impl UniversalRegionIndices {
    fn insert_late_bound_region(&mut self, scope: RegionScope, r: RegionId, name: &str, class: RegionClassification) {
        if !self.regions.iter().any(|u| matches!(u.identity,Identity::Region(_,a) if a == r)) {
            self.regions.push(UniversalRegion { identity: Identity::Region(scope,r),name:name.into(),class });
        }
    }
}
struct UniversalRegionsBuilder<'a> { tcx: &'a Tcx, prog: &'a Program, defining_ty: DefiningTy, parent: Option<SignatureRegions>, contract: RegionContract }
impl UniversalRegionsBuilder<'_> {
    fn build(mut self) -> SignatureRegions {
        let tcx = self.tcx;
        let scope = self.defining_ty.scope();
        let (mut inputs,mut output,bound_regions) = self.defining_ty.inputs_and_output(tcx,self.prog);
        let mut query_placeholders = vec![];
        if matches!(self.defining_ty,DefiningTy::Closure { .. }) {
            // Like the implied-bounds query, give each erased external occurrence
            // a distinct identity. The MIR driver instantiates these as free vars.
            // Relax's liberated closure signature can still contain regions
            // bound by an enclosing callable. Instantiate that outer binder as
            // external query identities before the ordinary erased-region pass.
            let raw: Vec<_> = inputs.iter().copied().chain([output]).collect();
            let mut replaced = crate::borrowck::implied_bounds::replace_erased_regions_with_placeholders(tcx,&raw);
            let next = std::cell::Cell::new(crate::borrowck::implied_bounds::implied_bounds_query_var_values(tcx,&replaced,
                |r| matches!(tcx.tys.region_kind(r),RegionKind::RePlaceholder { .. })).len() as u32);
            let outer = std::cell::RefCell::new(crate::fxhash::HashMap::default());
            replaced = replaced.into_iter().map(|ty| tcx.tys.map_regions(ty,&|r,depth| {
                if let RegionKind::ReBound(d,v) = tcx.tys.region_kind(r) {
                    if d >= depth {
                        return *outer.borrow_mut().entry((d-depth,v)).or_insert_with(|| {
                            let var = BoundVar(next.get()); next.set(var.0+1);
                            tcx.tys.intern_region(RegionKind::RePlaceholder { universe:crate::types::UniverseIndex::ROOT,var })
                        });
                    }
                }
                r
            },0)).collect();
            query_placeholders = crate::borrowck::implied_bounds::implied_bounds_query_var_values(tcx,&replaced,|r|
                matches!(tcx.tys.region_kind(r),RegionKind::RePlaceholder { universe: crate::types::UniverseIndex::ROOT,.. }));
            output = replaced.pop().unwrap(); inputs = replaced;
        }
        let mut indices = self.compute_indices();
        let late = |v: BoundVar| tcx.tys.intern_region(RegionKind::ReLateParam(scope,LateParamKind::Anon(v.0)));
        // Declaration parameters are enumerated even when absent from I/O.
        for (name,r) in &mut self.contract.regions {
            if let RegionKind::ReBound(0,v) = tcx.tys.region_kind(*r) { *r = late(v); }
            if !matches!(self.defining_ty,DefiningTy::Closure { .. }) {
                indices.insert_late_bound_region(scope,*r,name,RegionClassification::Local);
            }
        }
        for v in 0..bound_regions { indices.insert_late_bound_region(scope,late(BoundVar(v)),"'_",RegionClassification::Local); }
        if let DefiningTy::Closure { bound, annotation_eq, owner, .. } = self.defining_ty {
            // Free regions of a nested signature belong to its creator. Bound
            // regions have already been liberated by Relax's typeck metadata.
            // The closure type's free regions are `bound`'s (universal_regions.rs
            // defining_ty: replace_free_regions_with_nll_infer_vars over the
            // closure type); after a merge, region variables that only the
            // liberated signature's normalization introduced stay existential.
            for (t, expected) in inputs.iter().copied().chain([output]).map(|t| (t,false)).chain([(bound,true)]) {
                let found = std::cell::RefCell::new(vec![]);
                tcx.tys.map_regions(t,&|r,_| {
                    if !matches!(tcx.tys.region_kind(r),RegionKind::ReBound(..)|RegionKind::ReErased|RegionKind::ReError|RegionKind::ReStatic)
                        && (expected || !annotation_eq || !matches!(tcx.tys.region_kind(r),RegionKind::ReVar(_))) { found.borrow_mut().push(r); }
                    r
                },0);
                for r in found.into_inner() {
                    let scope = match tcx.tys.region_kind(r) { RegionKind::ReLateParam(s,_) => s, _ => RegionScope::Item(owner) };
                    indices.insert_late_bound_region(scope,r,"'_",RegionClassification::External);
                }
            }
        }
        // Globals, externals, locals, then the body region, as in nightly.
        indices.regions.sort_by_key(|r| match r.class { RegionClassification::Global => 0, RegionClassification::External => 1, RegionClassification::Local => 2 });
        let (inputs,output) = if matches!(self.defining_ty,DefiningTy::Closure { .. }) { (inputs,output) }
            else { let mut f = BoundVarReplacer::new(late, |_| panic!("signature bound type"));
                (inputs.iter().map(|&t| f.fold_ty(&tcx.tys,t,0)).collect(),f.fold_ty(&tcx.tys,output,0)) };
        let fr_fn_body = indices.regions.len();
        indices.regions.push(UniversalRegion { identity:Identity::FnBody(scope),class:RegionClassification::Local,name:"'fn_body".into() });
        SignatureRegions { regions:indices.regions,inputs,output,implicit_input:None,contract:self.contract,fr_static:0,fr_fn_body,query_placeholders }
    }
    fn compute_indices(&mut self) -> UniversalRegionIndices {
        let mut regions = match self.parent.take() {
            Some(parent) => parent.regions.into_iter().filter(|r| !matches!(r.identity,Identity::FnBody(_))).map(|mut r| {
                if r.class != RegionClassification::Global { r.class = RegionClassification::External; } r
            }).collect(),
            None => vec![],
        };
        if regions.is_empty() { regions.push(UniversalRegion { identity:Identity::Static,class:RegionClassification::Global,name:"'static".into() }); }
        UniversalRegionIndices { regions }
    }
}
/// `LateParamKind::Anon` numbers from here name the erased regions of a closure's environment type.
pub const ENV_ERASED_BASE: u32 = 0x4000_0000;

impl SignatureRegions {
    pub fn function(tcx: &Tcx, prog: &Program, owner: DefId) -> Option<Self> { Self::function_signature(tcx,prog,owner,true) }
    /// Calling a C-variadic function does not pass the body's implicit VaList.
    pub fn function_call(tcx: &Tcx, prog: &Program, owner: DefId) -> Option<Self> { Self::function_signature(tcx,prog,owner,false) }
    fn function_signature(tcx: &Tcx, prog: &Program, owner: DefId, body: bool) -> Option<Self> {
        let (defining_ty,contract) = if let Some(sig) = tcx.fn_sig(prog, owner).or_else(|| tcx.ctor_sigs.get(&owner)) {
            (DefiningTy::FnDef(owner,body),sig.region_contract.clone())
        } else { tcx.const_ty(prog, owner)?;
            (DefiningTy::Const(owner),tcx.const_region_contract(prog, owner).cloned().unwrap_or_default()) };
        Some(UniversalRegionsBuilder { tcx,prog,defining_ty,parent:None,contract }.build())
    }
    /// An array-length anonymous constant: its own root, no inputs, output `usize`, no lifetime parameters in scope.
    pub fn anon_const(tcx: &Tcx, prog: &Program, owner: DefId) -> Self {
        UniversalRegionsBuilder { tcx,prog,defining_ty:DefiningTy::AnonConst(owner),parent:None,contract:Default::default() }.build()
    }
    /// A closure's universal regions (universal_regions.rs `DefiningTy::Closure`): its parent's
    /// are external, its own signature's are local. The signature is the typeck result's, with
    /// the lifetime arguments of its types as written back.
    pub fn closure_in(tcx: &Tcx, prog: &Program, owner: DefId, expr: u32, sig: &crate::typeck::ClosureSignatures, parent: Self) -> Option<Self> {
        // compute_inputs_and_output: the closure's one (bound) signature, not
        // the body's supplied types; the annotation is ascribed onto it.
        let signature = sig.liberated_bound();
        if !matches!(tcx.tys.kind(signature),TyKind::FnPtr(..)) { return None; }
        Some(UniversalRegionsBuilder { tcx,prog,
            defining_ty:DefiningTy::Closure { owner,expr,signature,bound:sig.bound,annotation_eq:sig.annotation_eq.is_some() },
            contract:parent.contract.clone(),parent:Some(parent) }.build())
    }
    /// The closure body's implicit first argument (the environment, `&'env C`, `&'env mut C`
    /// or `C` for FnOnce), with the `'env` universal region inserted as the last local region,
    /// ahead of the body region. The free regions of the closure type `C` (its signature and
    /// captured places) are external regions, as for rustc's `DefiningTy::Closure` whose
    /// arguments they are; a nested use maps them to its own instantiation by position.
    pub fn with_closure_env(mut self, tcx: &Tcx, owner: DefId, expr: u32, env: TyId) -> Self {
        let closure_ty = match tcx.tys.kind(env) { TyKind::Ref(_, ct, _) => *ct, _ => env };
        // rustc's `replace_free_regions_with_nll_infer_vars` makes every region of the defining type,
        // erased ones included, a universal region of its own: each erased occurrence takes a distinct
        // identity, scoped to this closure (a nested closure inherits its creator's universal regions,
        // which must stay different) and numbered in order (the creator finds the slot by that number).
        let erased = std::cell::Cell::new(0u32);
        let closure_scope = RegionScope::Closure { owner, expr };
        let closure_ty = tcx.tys.map_regions(closure_ty, &|r, depth| match tcx.tys.region_kind(r) {
            RegionKind::ReErased => {
                let n = erased.get(); erased.set(n + 1);
                tcx.tys.intern_region(RegionKind::ReLateParam(closure_scope, LateParamKind::Anon(ENV_ERASED_BASE + n)))
            }
            RegionKind::ReBound(d, _) if d < depth => r,
            _ => r,
        }, 0);
        let env = match tcx.tys.kind(env) { TyKind::Ref(m, _, r) => tcx.tys.intern(TyKind::Ref(*m, closure_ty, *r)), _ => closure_ty };
        let mut external = vec![];
        for r in crate::borrowck::mir_nll::free_regions_in_order(tcx, closure_ty) {
            if matches!(tcx.tys.region_kind(r), RegionKind::ReErased | RegionKind::ReError | RegionKind::ReStatic) { continue; }
            let known = |u: &UniversalRegion| matches!(u.identity, Identity::Region(_, x) if x == r);
            if self.regions.iter().any(known) || external.iter().any(known) { continue; }
            let scope = match tcx.tys.region_kind(r) { RegionKind::ReLateParam(s, _) => s, _ => RegionScope::Item(owner) };
            external.push(UniversalRegion { identity: Identity::Region(scope, r), class: RegionClassification::External, name: "'_".into() });
        }
        // Global and external regions precede the local ones.
        let at = self.regions.iter().take_while(|u| matches!(u.class, RegionClassification::Global | RegionClassification::External)).count();
        self.fr_fn_body += external.len();
        self.regions.splice(at..at, external);
        if let TyKind::Ref(m, ct, _) = tcx.tys.kind(env) {
            let scope = RegionScope::Closure { owner, expr };
            let r = tcx.tys.intern_region(RegionKind::ReLateParam(scope, LateParamKind::ClosureEnv));
            self.regions.insert(self.fr_fn_body, UniversalRegion { identity: Identity::Region(scope, r), class: RegionClassification::Local, name: "'env".into() });
            self.fr_fn_body += 1;
            self.implicit_input = Some(tcx.tys.intern(TyKind::Ref(*m, *ct, r)));
        } else {
            self.implicit_input = Some(env);
        }
        self
    }
}

impl UniversalRegions {
    /// `UniversalRegions::new`'s region variables: one fresh variable per declared universal
    /// region, in the signature's order (global, external, local, then the body region), so
    /// that vids `0..n` are the universals and the type checker's variables follow.
    pub fn from_signature(tcx: &Tcx, sig: &SignatureRegions, mut next_free_region: impl FnMut() -> RegionVid) -> Self {
        let (mut regions, mut indices) = (vec![], vec![]);
        let (mut fr_static, mut fr_fn_body) = (None, None);
        for universal in &sig.regions {
            let vid = next_free_region();
            regions.push((vid, universal.class));
            match universal.identity {
                Identity::Static => { fr_static = Some(vid); indices.push((tcx.tys.intern_region(RegionKind::ReStatic), vid)); }
                Identity::Region(_, r) => indices.push((r, vid)),
                Identity::FnBody(_) => fr_fn_body = Some(vid),
            }
        }
        let mut universals = Self::new(fr_static.expect("'static is the first universal region"), fr_fn_body, regions);
        universals.indices = indices;
        universals
    }
}
