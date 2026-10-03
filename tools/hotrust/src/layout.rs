//! Machine layout of types: size, alignment, field offsets and the scalar
//! "leaves" a small aggregate is split into when it lives in registers.

use crate::program::Prim;
use crate::rir::{Cls, Mem};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Leaf {
    pub off: u32,
    pub mem: Mem,
    pub cls: Cls,
}

#[derive(Clone)]
pub struct Layout {
    pub size: u32,
    pub align: u32,
    /// struct/tuple field offsets (variant 0 for structs)
    pub fields: Vec<u32>,
    /// enums: tag offset/width and per-variant field offsets
    pub tag: Option<(u32, Mem)>,
    pub variant_fields: Vec<Vec<u32>>,
    /// Some when the value can live in registers (<= MAX_LEAVES scalars, no arrays)
    pub leaves: Option<Vec<Leaf>>,
}

pub const MAX_LEAVES: usize = 8;

pub struct Layouts {
    cache: HashMap<TyId, Layout>,
}

fn scalar(size: u32, mem: Mem, cls: Cls) -> Layout {
    Layout { size, align: size, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: Some(vec![Leaf { off: 0, mem, cls }]) }
}

impl Layouts {
    pub fn new() -> Layouts {
        Layouts { cache: HashMap::new() }
    }

    pub fn of(&mut self, tcx: &mut Tcx, t: TyId) -> Layout {
        if let Some(l) = self.cache.get(&t) {
            return l.clone();
        }
        let l = self.compute(tcx, t);
        self.cache.insert(t, l.clone());
        l
    }

    fn compute(&mut self, tcx: &mut Tcx, t: TyId) -> Layout {
        let k = tcx.tys.kind(t).clone();
        let ptr = Mem::Int(8, false);
        match k {
            TyKind::Bool => scalar(1, Mem::Int(1, false), Cls::I),
            TyKind::Char => scalar(4, Mem::Int(4, false), Cls::I),
            TyKind::Int(p) => {
                let (b, s) = crate::types::Types::int_info(p);
                if b == 16 {
                    // i128/u128: two words, memory only for now
                    return Layout { size: 16, align: 16, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: None };
                }
                scalar(b, Mem::Int(b as u8, s), Cls::I)
            }
            TyKind::Float(Prim::F32) => scalar(4, Mem::F32, Cls::F32),
            TyKind::Float(_) => scalar(8, Mem::F64, Cls::F64),
            TyKind::Ref(_, inner) | TyKind::Ptr(_, inner) => {
                if self.is_unsized(tcx, inner) {
                    Layout {
                        size: 16,
                        align: 8,
                        fields: vec![0, 8],
                        tag: None,
                        variant_fields: Vec::new(),
                        leaves: Some(vec![Leaf { off: 0, mem: ptr, cls: Cls::I }, Leaf { off: 8, mem: ptr, cls: Cls::I }]),
                    }
                } else {
                    scalar(8, ptr, Cls::I)
                }
            }
            TyKind::FnPtr(..) => scalar(8, ptr, Cls::I),
            TyKind::Closure(_, _, _, up, _) => self.of(tcx, up),
            TyKind::FnDef(..) | TyKind::Never => {
                Layout { size: 0, align: 1, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: Some(Vec::new()) }
            }
            TyKind::Tuple(v) => self.record(tcx, &v),
            TyKind::Array(e, n) => {
                let el = self.of(tcx, e);
                let stride = round_up(el.size, el.align);
                Layout { size: stride * n as u32, align: el.align, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: None }
            }
            TyKind::Adt(d, args) => {
                let adt = match tcx.adts.get(&d) {
                    Some(a) => a.clone(),
                    None => return Layout { size: 0, align: 1, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: Some(Vec::new()) },
                };
                if !adt.is_enum {
                    let mut fts = Vec::new();
                    for f in &adt.variants[0].fields {
                        fts.push(tcx.tys.subst(f.ty, &args));
                    }
                    if adt.is_union {
                        let mut size = 0;
                        let mut align = 1;
                        for f in &fts {
                            let l = self.of(tcx, *f);
                            size = size.max(l.size);
                            align = align.max(l.align);
                        }
                        let n = fts.len();
                        return Layout { size: round_up(size, align), align, fields: vec![0; n], tag: None, variant_fields: Vec::new(), leaves: None };
                    }
                    return self.record(tcx, &fts);
                }
                // enum: tag then payload
                let mut max_disc: i128 = 0;
                let mut min_disc: i128 = 0;
                let mut fieldless = true;
                for v in &adt.variants {
                    max_disc = max_disc.max(v.disc);
                    min_disc = min_disc.min(v.disc);
                    if !v.fields.is_empty() {
                        fieldless = false;
                    }
                }
                let (tag_bytes, tag_signed) = match adt.repr_int {
                    Some(p) => {
                        let (b, s) = crate::types::Types::int_info(p);
                        (b, s)
                    }
                    None => {
                        if min_disc >= 0 && max_disc < 256 {
                            (1, false)
                        } else if min_disc >= i32::MIN as i128 && max_disc <= i32::MAX as i128 {
                            (4, true)
                        } else {
                            (8, true)
                        }
                    }
                };
                let tag_mem = Mem::Int(tag_bytes as u8, tag_signed);
                if fieldless {
                    let mut l = scalar(tag_bytes, tag_mem, Cls::I);
                    l.tag = Some((0, tag_mem));
                    for _ in 0..adt.variants.len() {
                        l.variant_fields.push(Vec::new());
                    }
                    return l;
                }
                let mut align = tag_bytes;
                let mut size = tag_bytes;
                let mut vfs = Vec::new();
                for v in &adt.variants {
                    let mut off = tag_bytes;
                    let mut offs = Vec::new();
                    for f in &v.fields {
                        let ft = tcx.tys.subst(f.ty, &args);
                        let fl = self.of(tcx, ft);
                        off = round_up(off, fl.align);
                        offs.push(off);
                        off += fl.size;
                        align = align.max(fl.align);
                    }
                    size = size.max(off);
                    vfs.push(offs);
                }
                Layout { size: round_up(size, align), align, fields: Vec::new(), tag: Some((0, tag_mem)), variant_fields: vfs, leaves: None }
            }
            TyKind::Str => Layout { size: 0, align: 1, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: None },
            TyKind::Slice(e) => {
                let el = self.of(tcx, e);
                Layout { size: 0, align: el.align, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: None }
            }
            // a trait object's alignment is only known from its vtable; struct tails of dyn type
            // are placed at an 8-aligned offset (the vtable's align beyond 8 is not supported)
            TyKind::Dyn(..) => Layout { size: 0, align: 8, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: None },
            _ => Layout { size: 0, align: 1, fields: Vec::new(), tag: None, variant_fields: Vec::new(), leaves: Some(Vec::new()) },
        }
    }

    fn record(&mut self, tcx: &mut Tcx, fts: &[TyId]) -> Layout {
        let mut off = 0;
        let mut align = 1;
        let mut fields = Vec::new();
        let mut leaves = Some(Vec::new());
        for f in fts {
            let l = self.of(tcx, *f);
            off = round_up(off, l.align);
            fields.push(off);
            match (&mut leaves, &l.leaves) {
                (Some(acc), Some(fl)) => {
                    for x in fl {
                        acc.push(Leaf { off: off + x.off, mem: x.mem, cls: x.cls });
                    }
                }
                _ => leaves = None,
            }
            off += l.size;
            align = align.max(l.align);
        }
        if let Some(v) = &leaves {
            if v.len() > MAX_LEAVES {
                leaves = None;
            }
        }
        Layout { size: round_up(off, align), align, fields, tag: None, variant_fields: Vec::new(), leaves }
    }

    /// Dynamically sized: str, slices, trait objects, and structs/tuples whose last field is.
    pub fn is_unsized(&mut self, tcx: &mut Tcx, t: TyId) -> bool {
        // by type structure only (no layouts: pointee layouts may be recursive)
        match tcx.tys.kind(t).clone() {
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => true,
            TyKind::Tuple(v) => match v.last() {
                Some(l) => self.is_unsized(tcx, *l),
                None => false,
            },
            TyKind::Adt(d, args) => {
                let adt = match tcx.adts.get(&d) {
                    Some(a) => a.clone(),
                    None => return false,
                };
                if adt.is_enum || adt.is_union {
                    return false;
                }
                match adt.variants[0].fields.last() {
                    Some(f) => {
                        let ft = tcx.tys.subst(f.ty, &args);
                        self.is_unsized(tcx, ft)
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// The innermost dynamically sized tail of `t` and its offset from the start of `t`
    /// (`(0, t)` for str/slice/dyn themselves); None for sized types.
    pub fn unsized_tail(&mut self, tcx: &mut Tcx, t: TyId) -> Option<(u32, TyId)> {
        if !self.is_unsized(tcx, t) {
            return None;
        }
        match tcx.tys.kind(t).clone() {
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => Some((0, t)),
            TyKind::Tuple(v) => {
                let last = *v.last()?;
                let (o, tail) = self.unsized_tail(tcx, last)?;
                let l = self.of(tcx, t);
                Some((l.fields[v.len() - 1] + o, tail))
            }
            TyKind::Adt(d, args) => {
                let adt = tcx.adts.get(&d)?.clone();
                if adt.is_enum || adt.is_union {
                    return None;
                }
                let f = adt.variants[0].fields.last()?;
                let ft = tcx.tys.subst(f.ty, &args);
                let (o, tail) = self.unsized_tail(tcx, ft)?;
                let l = self.of(tcx, t);
                Some((l.fields[adt.variants[0].fields.len() - 1] + o, tail))
            }
            _ => None,
        }
    }
}

pub fn round_up(x: u32, a: u32) -> u32 {
    if a <= 1 {
        x
    } else {
        (x + a - 1) / a * a
    }
}
