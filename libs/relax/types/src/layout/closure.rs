use crate::expr_use::{BorrowKind, Projection};
use crate::tcx::Tcx;
use crate::typeck::Body;
use crate::types::{TyId, TyKind};

// EXCEPTION(N4): root-slot closure storage until per-place capture codegen
#[derive(Clone, Default)]
pub struct ClosureStorage {
    pub fields: Vec<TyId>,
    pub roots: Vec<u32>,
    pub by_value: Vec<bool>,
    pub owned_fields: Vec<Option<Vec<u32>>>,
    pub drop_flags: bool,
}

impl ClosureStorage {
    pub fn new(tcx: &Tcx, body: &Body, ce: u32) -> Self {
        Self::with_types(tcx, body, ce, |ty| ty)
    }

    /// The parent may be the shared typeck result. Normalize capture reads without
    /// copying its tables (rustc_codegen_ssa::mir::FunctionCx::monomorphize).
    pub fn for_codegen(prog: &crate::program::Program, tcx: &Tcx, body: &Body, ce: u32) -> Self {
        Self::with_types(tcx, body, ce, |ty| tcx.normalize_erasing_regions(
            prog, crate::tcx::TypingEnv::fully_monomorphized(prog), ty))
    }

    fn with_types(tcx: &Tcx, body: &Body, ce: u32, ty: impl Fn(TyId) -> TyId) -> Self {
        let captures = &body.min_captures[&ce].captures;
        let roots: Vec<_> = body
            .closure_locals
            .get(&ce)
            .into_iter()
            .flatten()
            .copied()
            .filter(|root| captures.iter().any(|c| c.place.root == *root))
            .collect();
        let mut by_value = Vec::new();
        let mut fields = Vec::new();
        let mut upvars = Vec::new();
        for &root in &roots {
            let cs: Vec<_> = captures.iter().filter(|c| c.place.root == root).collect();
            let ty = ty(body.locals[root as usize].ty);
            // A captured *shared_ref is already a pointer to its referent. The old
            // ABI accesses the root, so store that reference value, not &shared_ref.
            let deref_shared = matches!(tcx.tys.kind(ty), TyKind::Ref(false, _, _))
                && cs.iter().all(|c| {
                    c.place
                        .projections
                        .first()
                        .is_some_and(|p| p.0 == Projection::Deref)
                });
            let value = deref_shared || cs.iter().any(|c| c.info.kind == crate::upvar::CaptureKind::ByValue);
            let mutable = cs.iter().any(|c| {
                matches!(
                    c.info.kind,
                    crate::upvar::CaptureKind::ByRef(BorrowKind::Mut | BorrowKind::UniqueImm)
                )
            });
            by_value.push(value);
            upvars.push(if value {
                ty
            } else {
                tcx.tys.intern(TyKind::Ref(mutable, ty, crate::types::RegionId::ERASED))
            });
            let mut fs = Vec::new();
            let precise = value
                && !deref_shared
                && cs.iter().filter(|c| c.info.kind == crate::upvar::CaptureKind::ByValue).all(|c| {
                    if let Some((Projection::Field(0, field), _)) = c.place.projections.first() {
                        fs.push(*field);
                        true
                    } else {
                        false
                    }
                });
            fs.sort_unstable();
            fs.dedup();
            fields.push(precise.then_some(fs));
        }
        let result = &body.min_captures[&ce];
        let drop_flags = if result.suspension_points.is_some() {
            by_value.iter().any(|v| *v)
        } else { result.kind == crate::upvar::ClosureKind::FnOnce };
        if drop_flags { for _ in &roots { upvars.push(tcx.tys.u8_); } }
        if let Some(points) = result.suspension_points {
            let states = points as u64 + 3;
            upvars.push(tcx.tys.prim(if states <= 1 << 8 { crate::program::Prim::U8 }
                else if states <= 1 << 16 { crate::program::Prim::U16 } else { crate::program::Prim::U32 }));
        }
        Self { fields: upvars, roots, by_value, owned_fields: fields, drop_flags }
    }
}
