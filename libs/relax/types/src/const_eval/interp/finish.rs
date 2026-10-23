// ported from N:rustc_const_eval/src/interpret/{intern.rs,validity.rs} @8d1a764
//! The final value: interning (no dangling or partial pointer may remain) and validity.
use super::*;

impl Interp<'_> {
    pub(super) fn intern_and_validate(&mut self, root: u32, is_static: bool) -> R<()> {
        let what = if is_static { "static" } else { "constant" };
        // intern.rs intern_const_alloc_recursive: every allocation the value reaches
        let mut todo = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(a) = todo.pop() {
            if !seen.insert(a) { continue; }
            let al = &self.allocs[a as usize];
            // (memory already in tcx -- another constant's or a static's -- is interned already)
            if al.global { continue; }
            if !al.frags.is_empty() { return Err(Stop::Item(format!("encountered partial pointer in final value of {what}"))); }
            for &(_, p) in &al.ptrs {
                let t = &self.allocs[p.alloc as usize];
                if !t.live { return Err(Stop::Item(format!("encountered dangling pointer in final value of {what}"))); }
                todo.push(p.alloc);
            }
        }
        let ty = self.frames_root_ty.unwrap_or_else(|| self.tcx.tys.unit);
        let place = MPlace { ptr: Ptr { prov: Some(Prov { alloc: root, shared: false }), off: 0 }, meta: None, ty, variant: None, packed: None };
        let mut visited = std::collections::HashSet::new();
        self.validating = true;
        let r = self.validate(place, String::new(), 0, &mut visited);
        self.validating = false;
        match r {
            Err(Stop::Item(m)) => Err(Stop::Item(format!("E0080: constructing invalid value of type {}: {m}", self.ty_str(ty)))),
            Err(Stop::Ub(_)) => unsupported("validity read"),
            r => r,
        }
    }

    fn ty_str(&self, t: TyId) -> String { crate::typeck::ty_to_string(false, self.prog, self.tcx, t) }

    fn invalid<T>(&self, path: &str, what: String) -> R<T> {
        Err(Stop::Item(if path.is_empty() { format!("encountered {what}") } else { format!("at {path}, encountered {what}") }))
    }

    /// validity.rs ValidityVisitor (const mode: references are followed).
    fn validate(&mut self, p: MPlace, path: String, depth: u32, visited: &mut std::collections::HashSet<(u32, u64, TyId)>) -> R<()> {
        if depth > 32 { return unsupported("deep validation"); }
        let tys = &self.tcx.tys;
        let kind = tys.kind(p.ty).clone();
        let expected = |k: &str| k.to_string();
        match kind {
            TyKind::Bool | TyKind::Int(_) | TyKind::Float(_) | TyKind::Char => {
                let v = self.read_place(p)?;
                let k = match kind { TyKind::Bool => "a boolean", TyKind::Char => "a unicode scalar value", TyKind::Float(_) => "a floating point number", _ => "an integer" };
                if v.init.iter().any(|b| !b) { return self.invalid(&path, format!("uninitialized memory, but expected {k}")); }
                if !v.ptrs.is_empty() || !v.frags.is_empty() { return self.invalid(&path, format!("a pointer, but expected {k}")); }
                let x = self.int_of(&v)?;
                match kind {
                    TyKind::Bool if x > 1 => return self.invalid(&path, format!("{x:#04x}, but expected a boolean")),
                    TyKind::Char if char::from_u32(x as u32).is_none() => return self.invalid(&path, format!("{x:#010x}, but expected a valid unicode scalar value (in `0..=0x10FFFF` but not in `0xD800..=0xDFFF`)")),
                    _ => {}
                }
                Ok(())
            }
            TyKind::Never => self.invalid(&path, "a value of the never type `!`".into()),
            TyKind::Ptr(..) => {
                let v = self.read_place(p)?;
                if v.init.iter().any(|b| !b) { return self.invalid(&path, expected("uninitialized memory, but expected a raw pointer")); }
                Ok(())
            }
            TyKind::FnPtr(..) => {
                let v = self.read_place(p)?;
                if v.init.iter().any(|b| !b) { return self.invalid(&path, expected("uninitialized memory, but expected a function pointer")); }
                let q = match self.ptr_of(&v, 0) { Ok(q) => q, Err(_) => return self.invalid(&path, "a partial pointer, but expected a function pointer".into()) };
                match q.prov { Some(pr) if matches!(self.allocs[pr.alloc as usize].kind, Kind::Function(_)) && q.off == 0 => Ok(()), _ => self.invalid(&path, format!("{}, but expected a function pointer", self.ptr_str(q))) }
            }
            TyKind::Ref(_, pointee, _) => {
                let v = self.read_place(p)?;
                if v.init.iter().any(|b| !b) { return self.invalid(&path, expected("uninitialized memory, but expected a reference")); }
                let q = match self.ptr_of(&v, 0) { Ok(q) => q, Err(_) => return self.invalid(&path, "a partial pointer, but expected a reference".into()) };
                let fat = self.is_fat(pointee)?;
                let meta = if fat { Some(self.int_of(&v.slice(self.ps, self.ps, self.ps))? as u64) } else { None };
                let size = match (self.tcx.tys.kind(pointee), meta) {
                    (TyKind::Slice(e), Some(n)) => { let (s, _) = self.size_align(*e)?; s * n }
                    (TyKind::Str, Some(n)) => n,
                    (_, Some(_)) => return unsupported("unsized struct reference"),
                    _ => self.size_align(pointee)?.0,
                };
                let align = match self.tcx.tys.kind(pointee) { TyKind::Slice(e) => self.size_align(*e)?.1, TyKind::Str => 1, _ => self.size_align(pointee)?.1 };
                match q.prov {
                    None if q.off == 0 => return self.invalid(&path, "a null reference".into()),
                    None if size > 0 => return self.invalid(&path, format!("a dangling reference ({:#x}[noalloc] has no provenance)", q.off)),
                    None => { if q.off % align != 0 { return self.invalid(&path, format!("an unaligned reference (required {align} byte alignment but found {})", 1u64 << q.off.trailing_zeros().min(63))); } return Ok(()); }
                    Some(pr) => {
                        let a = &self.allocs[pr.alloc as usize];
                        if !a.live { return self.invalid(&path, "a dangling reference (use-after-free)".into()); }
                        if q.off + size > a.size() { return self.invalid(&path, "a dangling reference (going beyond the bounds of its allocation)".into()); }
                        if align > 1 && (a.align < align || q.off % align != 0) { return self.invalid(&path, format!("an unaligned reference (required {align} byte alignment but found {})", a.align.min(1u64 << q.off.trailing_zeros().min(63)))); }
                        if matches!(a.kind, Kind::Function(_)) { return self.invalid(&path, "a reference pointing to a function".into()); }
                        // validity.rs check_safe_pointer: a static is checked on its own (static mode);
                        // a constant does not look into mutable memory of one
                        if let Kind::Static(_) = a.kind { if self.static_root.is_some() || a.mutable { return Ok(()); } }
                    }
                }
                if !visited.insert((q.prov.unwrap().alloc, q.off, pointee)) { return Ok(()); }
                let inner = MPlace { ptr: q, meta, ty: pointee, variant: None, packed: None };
                self.validate_pointee(inner, format!("{path}.<deref>"), depth + 1, visited)
            }
            TyKind::Tuple(l) => {
                let l2: Vec<TyId> = tys.list(l).to_vec();
                let lay = self.layout(p.ty)?;
                for (i, t) in l2.iter().enumerate() {
                    let f = MPlace { ptr: Ptr { off: p.ptr.off + lay.fields[i] as u64, ..p.ptr }, meta: None, ty: *t, variant: None, packed: p.packed };
                    self.validate(f, format!("{path}.{i}"), depth, visited)?;
                }
                Ok(())
            }
            TyKind::Array(e, n) => {
                let Some(n) = n.try_to_target_usize(tys) else { return unsupported("array length") };
                let (size, _) = self.size_align(e)?;
                // (the elements of a zero-sized array are one value)
                let n = if size == 0 { n.min(1) } else { n };
                for i in 0..n {
                    let f = MPlace { ptr: Ptr { off: p.ptr.off + i * size, ..p.ptr }, meta: None, ty: e, variant: None, packed: p.packed };
                    self.validate(f, format!("{path}[{i}]"), depth, visited)?;
                }
                Ok(())
            }
            TyKind::Adt(d, args, _) => {
                let Some(adt) = self.tcx.adts.get(&d) else { return unsupported("adt") };
                if adt.is_union { return Ok(()); }
                if adt.is_box() { return unsupported("box validity"); }
                if adt.valid_start.is_some() || adt.valid_end.is_some() { return unsupported("scalar valid range"); }
                let lay = self.layout(p.ty)?;
                if lay.uninhabited { return self.invalid(&path, format!("a value of uninhabited type `{}`", self.ty_str(p.ty))); }
                let args: Vec<TyId> = tys.list(args).to_vec();
                let (vi, offs) = if adt.is_enum {
                    let vi = match self.read_variant(p) {
                        Ok(v) => v,
                        Err(Stop::Ub(_)) => return self.invalid(&format!("{path}.<enum-tag>"), "an invalid enum tag".into()),
                        Err(e) => return Err(e),
                    };
                    (vi as usize, lay.variant_fields.get(vi as usize).cloned().unwrap_or_default())
                } else { (0, lay.fields.clone()) };
                let variant = &adt.variants[vi];
                for (i, f) in variant.fields.iter().enumerate() {
                    let Some(&o) = offs.get(i) else { return unsupported("field offset") };
                    if o == u32::MAX { return unsupported("extension field"); }
                    let t = tys.subst_erasing_regions(f.ty, &args);
                    let name = self.prog.syms.str(f.name).to_string();
                    let seg = if adt.is_enum { format!("{path}.<enum-variant({})>.{name}", self.prog.syms.str(variant.name)) } else { format!("{path}.{name}") };
                    let packed = if adt.repr_packed > 0 { Some(p.packed.map_or(adt.repr_packed as u64, |q| q.min(adt.repr_packed as u64))) } else { p.packed };
                    self.validate(MPlace { ptr: Ptr { off: p.ptr.off + o as u64, ..p.ptr }, meta: None, ty: t, variant: None, packed }, seg, depth, visited)?;
                }
                Ok(())
            }
            _ => unsupported("validity of this type"),
        }
    }

    fn validate_pointee(&mut self, p: MPlace, path: String, depth: u32, visited: &mut std::collections::HashSet<(u32, u64, TyId)>) -> R<()> {
        match (self.tcx.tys.kind(p.ty).clone(), p.meta) {
            (TyKind::Str, _) => Ok(()),
            (TyKind::Slice(e), Some(n)) => {
                let (size, _) = self.size_align(e)?;
                for i in 0..n { self.validate(MPlace { ptr: Ptr { off: p.ptr.off + i * size, ..p.ptr }, meta: None, ty: e, variant: None, packed: None }, format!("{path}[{i}]"), depth, visited)?; }
                Ok(())
            }
            _ => self.validate(p, path, depth, visited),
        }
    }
}
