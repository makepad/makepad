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
use crate::borrowck::rty::Rty;

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
/// Selection may erase an ADT's entire lifetime argument list. Restore its
/// declared arity before enumerating free regions, so signature projection and
/// the BIR layout visit the same slots. Each erased occurrence is subsequently
/// instantiated independently, as in nightly's closure argument renumbering.
pub fn with_erased_adt_regions(tcx: &Tcx, ty: TyId) -> TyId {
    struct Restore<'a>(&'a Tcx);
    impl TypeFolder for Restore<'_> {
        fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
            let ty = crate::types::fold::super_fold_ty(tys, ty, self, depth);
            if let TyKind::Adt(d, args, rs) = tys.kind(ty) {
                let n = self.0.adts.get(d).map_or(0, |adt| adt.generic_indices.lifetimes.len());
                if *rs == crate::types::RegionArgsId::EMPTY && n != 0 {
                    return tys.intern(TyKind::Adt(*d, args.clone(), tys.intern_region_args(&vec![RegionId::ERASED; n])));
                }
            }
            ty
        }
    }
    Restore(tcx).fold_ty(&tcx.tys, ty, 0)
}
enum DefiningTy { FnDef(DefId, bool), Const(DefId), Closure { owner: DefId, expr: u32, signature: TyId, bound: TyId, annotation_eq: bool } }
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
                    let va_list = tcx.lang_items.get(crate::lang_items::LangItem::VaList)
                        .expect("C-variadic signature requires the va_list language item");
                    inputs.push(tcx.tys.intern(TyKind::Adt(va_list, tcx.tys.mk_list(&[]), tcx.tys.intern_region_args(&[region]))));
                }
                (inputs, sig.ret, bound_regions)
            }
            Self::Const(d) => (vec![],tcx.const_ty(prog, d).unwrap(),0),
            Self::Closure { signature, bound, .. } => {
                let TyKind::FnPtr(ins,out,_,_) = tcx.tys.kind(signature) else { unreachable!() };
                let TyKind::FnPtr(_,_,_,n) = tcx.tys.kind(bound) else { unreachable!() };
                (tcx.tys.list(*ins).to_vec(),*out,*n)
            }
        }
    }
    fn scope(&self) -> RegionScope {
        match *self { Self::FnDef(d, _) | Self::Const(d) => RegionScope::Item(d), Self::Closure { owner,expr,.. } => RegionScope::Closure { owner,expr } }
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
            // a distinct identity. The BIR adapter instantiates these as free vars.
            // Relax's liberated closure signature can still contain regions
            // bound by an enclosing callable. Instantiate that outer binder as
            // external query identities before the ordinary erased-region pass.
            let raw: Vec<_> = inputs.iter().copied().chain([output]).map(|t| with_erased_adt_regions(tcx,t)).collect();
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
    pub fn closure(rty: &Rty<'_>, owner: DefId, expr: u32) -> Option<Self> {
        let sig = *rty.closure_signatures.borrow().get(&expr)?;
        // compute_inputs_and_output: the closure's one (bound) signature, not
        // the body's supplied types; the annotation is ascribed onto it.
        let signature = sig.liberated_bound();
        if !matches!(rty.tx.tcx.tys.kind(signature),TyKind::FnPtr(..)) { return None; }
        let parent = match rty.closure_parents.borrow().get(&expr).copied().flatten() {
            Some(p) => Self::closure(rty,owner,p)?, None => Self::function(rty.tx.tcx,rty.tx.prog,owner)?,
        };
        Some(UniversalRegionsBuilder { tcx:rty.tx.tcx,prog:rty.tx.prog,
            defining_ty:DefiningTy::Closure { owner,expr,signature,bound:sig.bound,annotation_eq:sig.annotation_eq.is_some() },
            contract:parent.contract.clone(),parent:Some(parent) }.build())
    }
}
