// ported from N:rustc_hir_analysis/src/coherence/inherent_impls_overlap.rs @8d1a764
use super::overlap::overlapping_impls;
use super::fast_reject::{simplify_type,SimplifiedType};
use crate::fxhash::HashMap;
use crate::program::{DefId,DefKind,Program};
use crate::tcx::Tcx;
use crate::program::Sym;

struct InherentOverlapChecker<'a> { tcx: &'a Tcx, prog: &'a Program, krate: u32, ok: bool }
impl InherentOverlapChecker<'_> {
    fn items(&self, def: DefId) -> &[DefId] { &self.prog.impls[self.prog.def(def).sub as usize].items }
    fn compare_hygienically(&self,a:DefId,b:DefId)->bool {
        let a=self.prog.def(a); let b=self.prog.def(b);
        a.name==b.name && (a.kind==DefKind::AssocTy)==(b.kind==DefKind::AssocTy)
    }
    fn impls_have_common_items(&self,a:DefId,b:DefId)->bool {
        let mut a=self.items(a); let mut b=self.items(b);
        if a.len()>b.len() { std::mem::swap(&mut a,&mut b); }
        for &x in a { for &y in b { if self.compare_hygienically(x,y) { return true; } } }
        false
    }
    fn report(&mut self,item:DefId) {
        let d=self.prog.def(item);
        self.tcx.dcx.report(super::fulfill::ObligationCause { body_id:item,span:self.prog.files[d.file as usize].ast.item(d.item).lo },
            format!("E0592: duplicate definitions with name `{}`",self.prog.syms.str(d.name)));
        self.ok=false;
    }
    fn check_for_duplicate_items_in_impl(&mut self,def:DefId) {
        let mut seen=Vec::new();
        for item in self.items(def).to_vec() {
            let name=self.prog.def(item).name;
            if seen.contains(&name) { self.report(item); } else { seen.push(name); }
        }
    }
    fn check_for_overlapping_inherent_impls(&mut self,a:DefId,b:DefId) {
        if overlapping_impls(self.tcx,self.prog,a,b,self.krate,true,false).is_none() { return; }
        for item in self.items(a).to_vec() {
            for &other in self.items(b) { if self.compare_hygienically(item,other) { self.report(item); break; } }
        }
    }
    fn check_pairs(&mut self,impls:&[DefId]) {
        for (i,&a) in impls.iter().enumerate() {
            self.check_for_duplicate_items_in_impl(a);
            for &b in &impls[i+1..] { if self.impls_have_common_items(a,b) { self.check_for_overlapping_inherent_impls(a,b); } }
        }
    }
    fn check_item(&mut self,impls:&[DefId]) {
        if impls.len()<500 { self.check_pairs(impls); return; }
        // inherent_impls_overlap.rs:217-341: connected components by item name,
        // including the zero, one, and multiple existing-component cases.
        struct ConnectedRegion { idents:Vec<Sym>, impl_blocks:Vec<usize> }
        let mut regions:Vec<Option<ConnectedRegion>>=Vec::new();
        let mut ids_by_name=HashMap::default();
        for (i,&def) in impls.iter().enumerate() {
            let items=self.items(def);
            if items.is_empty() { continue; }
            let mut names=Vec::new(); let mut ids=Vec::new();
            for &item in items {
                let name=self.prog.def(item).name;
                if let Some(&id)=ids_by_name.get(&name) { ids.push(id); } else { names.push(name); }
            }
            ids.sort_unstable(); ids.dedup();
            let id=if ids.is_empty() { let id=regions.len(); regions.push(Some(ConnectedRegion { idents:Vec::new(),impl_blocks:Vec::new() })); id } else { ids[0] };
            let mut region=regions[id].take().unwrap();
            region.impl_blocks.push(i);
            for name in names { ids_by_name.insert(name,id); region.idents.push(name); }
            for &other in &ids {
                if other==id { continue; }
                let other=regions[other].take().unwrap();
                for name in other.idents { ids_by_name.insert(name,id); region.idents.push(name); }
                for index in other.impl_blocks { if !region.impl_blocks.contains(&index) { region.impl_blocks.push(index); } }
            }
            regions[id]=Some(region);
        }
        for region in regions.into_iter().flatten() {
            let mut defs=Vec::new(); for i in region.impl_blocks { defs.push(impls[i]); }
            self.check_pairs(&defs);
        }
    }
}

pub fn crate_inherent_impls_overlap_check(tcx:&Tcx,prog:&Program,krate:u32)->bool {
    let mut buckets:HashMap<SimplifiedType,Vec<DefId>>=HashMap::default(); let mut order=Vec::new();
    for im in &prog.impls {
        if prog.def(im.def).krate!=krate || tcx.impl_trait.contains_key(&im.def) { continue; }
        let Some(&ty)=tcx.impl_self.get(&im.def) else { continue; };
        let Some(key)=simplify_type(&tcx.tys,ty) else { continue; };
        // check_item visits nominal type declarations; invalid inherent headers
        // are diagnosed by orphan checking. Primitive impls live in skipped std.
        if !matches!(key,SimplifiedType::Adt(_)|SimplifiedType::Trait(_)) { continue; }
        if !buckets.contains_key(&key) { order.push(key); }
        buckets.entry(key).or_default().push(im.def);
    }
    let mut checker=InherentOverlapChecker { tcx,prog,krate,ok:true };
    for key in order { checker.check_item(&buckets[&key]); }
    checker.ok
}
