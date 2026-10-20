//! rustc_mir_build/src/builder/expr/as_place.rs PlaceBuilder: a place whose base may still be an upvar,
//! resolved to `_1.field` (+ deref) through the closure's captures when it becomes a `Place`.
use super::builder::Builder;
use super::thir::LocalVarId;
use crate::mir::{FieldIdx, Local, Place, PlaceElem, UpvarBorrowKind, VariantIdx};
use crate::program::Sym;
use crate::types::{RegionId, TyId, TyKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceBase {
    Local(Local),
    /// A captured variable of the enclosing closure body (as_place.rs:`PlaceBase::Upvar`).
    Upvar { var: LocalVarId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceBuilder {
    base: PlaceBase,
    projection: Vec<PlaceElem>,
}

impl From<Local> for PlaceBuilder {
    fn from(local: Local) -> Self {
        Self { base: PlaceBase::Local(local), projection: Vec::new() }
    }
}
impl From<PlaceBase> for PlaceBuilder {
    fn from(base: PlaceBase) -> Self {
        Self { base, projection: Vec::new() }
    }
}
impl From<Place> for PlaceBuilder {
    fn from(place: Place) -> Self {
        Self { base: PlaceBase::Local(place.local), projection: place.projection }
    }
}

impl PlaceBuilder {
    pub fn base(&self) -> PlaceBase {
        self.base
    }
    pub fn projection(&self) -> &[PlaceElem] {
        &self.projection
    }
    pub fn field(self, f: FieldIdx, ty: TyId) -> Self {
        self.project(PlaceElem::Field(f, ty))
    }
    pub fn deref(self) -> Self {
        self.project(PlaceElem::Deref)
    }
    /// as_place.rs downcast: the variant's name is kept for diagnostics.
    pub fn downcast(self, name: Option<Sym>, variant: VariantIdx) -> Self {
        self.project(PlaceElem::Downcast(name, variant))
    }
    pub fn index(self, index: Local) -> Self {
        self.project(PlaceElem::Index(index))
    }
    /// as_place.rs:`clone_project`.
    pub fn clone_project(&self, elem: PlaceElem) -> Self {
        self.clone().project(elem)
    }
    pub fn project(mut self, elem: PlaceElem) -> Self {
        self.projection.push(elem);
        self
    }

    /// as_place.rs:`resolve_upvar`: the place as seen from inside the closure body, or None if this builder's
    /// base is not an upvar (or no capture covers it).
    pub fn resolve_upvar(&self, cx: &Builder<'_>) -> Option<PlaceBuilder> {
        let PlaceBase::Upvar { var } = self.base else { return None };
        // OpaqueCast projections are not part of a capture path (as_place.rs:strip_prefix).
        let projection: Vec<PlaceElem> = self.projection.iter().filter(|elem| !matches!(elem, PlaceElem::OpaqueCast(_))).cloned().collect();
        let (index, capture, covered) = find_capture_matching_projections(&cx.upvars, var, &projection)?;
        let env = cx.upvar_env.as_ref()?;
        let mut builder = PlaceBuilder::from(env.local);
        if env.by_ref {
            builder = builder.deref();
        }
        let tys = &cx.root.cx.tcx.tys;
        let field_ty = match capture.capture {
            crate::mir::UpvarCapture::ByValue => capture.ty,
            crate::mir::UpvarCapture::ByRef(kind) => {
                tys.intern(TyKind::Ref(kind != UpvarBorrowKind::Immutable, capture.ty, RegionId::ERASED))
            }
        };
        builder = builder.field(FieldIdx(index as u32), field_ty);
        if capture.is_by_ref() {
            builder = builder.deref();
        }
        builder.projection.extend_from_slice(&projection[covered..]);
        Some(builder)
    }

    /// as_place.rs:`try_to_place`: None while the base is an upvar no capture resolves.
    pub fn try_to_place(&self, cx: &Builder<'_>) -> Option<Place> {
        match self.base {
            PlaceBase::Local(local) => Some(Place { local, projection: self.projection.clone() }),
            PlaceBase::Upvar { .. } => {
                let resolved = self.resolve_upvar(cx)?;
                resolved.try_to_place(cx)
            }
        }
    }
    /// as_place.rs:`to_place`.
    pub fn to_place(&self, cx: &Builder<'_>) -> Place {
        self.try_to_place(cx).unwrap_or_else(|| panic!("upvar place not covered by a capture: {:?} + {:?}; captures: {:?}", self.base, self.projection, cx.upvars.iter().map(|c| (c.var, &c.projection)).collect::<Vec<_>>()))
    }
}

/// Two projection elements name the same path step (types are ignored: they are place facts).
fn same_step(a: &PlaceElem, b: &PlaceElem) -> bool {
    match (a, b) {
        (PlaceElem::Deref, PlaceElem::Deref) => true,
        (PlaceElem::Field(f, _), PlaceElem::Field(g, _)) => f == g,
        (PlaceElem::Downcast(_, v), PlaceElem::Downcast(_, w)) => v == w,
        _ => false,
    }
}
/// as_place.rs:`find_capture_matching_projections`: the first capture of `var` whose place is a prefix of
/// `projection`; also the number of projection steps the capture covers.
fn find_capture_matching_projections<'c>(
    upvars: &'c [crate::mir::CapturedPlace],
    var: LocalVarId,
    projection: &[PlaceElem],
) -> Option<(usize, &'c crate::mir::CapturedPlace, usize)> {
    upvars.iter().enumerate().find_map(|(i, capture)| {
        let covered = capture.projection.len();
        (capture.var == var.0 && covered <= projection.len() && capture.projection.iter().zip(projection).all(|(a, b)| same_step(a, b)))
            .then_some((i, capture, covered))
    })
}
