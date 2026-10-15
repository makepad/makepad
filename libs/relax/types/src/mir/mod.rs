//! Shared body IR, following rustc_middle/src/mir/mod.rs:206 (Body).
//! M0 defines the representation only; producers and consumers migrate in later steps.

pub mod dataflow;
pub mod drop_flag_effects;
pub mod move_paths;
pub mod transform;
pub mod shim;
mod place_ty;
pub mod pretty;
pub mod syntax;
pub mod validate;
pub mod visit;
pub use syntax::*;
pub use place_ty::PlaceTy;

use crate::program::DefId;
use crate::types::TyId;

macro_rules! index {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);
    };
}
index!(Local);
index!(BasicBlock);
index!(SourceScope);
index!(PromotedId);
index!(UserTypeAnnotationIndex);
index!(UserTypeId);
index!(VariantIdx);
index!(FieldIdx);
index!(AllocId);

/// Pointer permissions, not allocation permissions. Immutable provenance is always shared.
/// rustc_middle/src/mir/interpret/pointer.rs:82,118.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CtfeProvenance {
    pub alloc: AllocId,
    immutable: bool,
    shared_ref: bool,
}
impl CtfeProvenance {
    pub fn new(alloc: AllocId) -> Self {
        Self {
            alloc,
            immutable: false,
            shared_ref: false,
        }
    }
    pub fn immutable(self) -> bool {
        self.immutable
    }
    pub fn shared_ref(self) -> bool {
        self.shared_ref
    }
    pub fn as_shared_ref(self) -> Self {
        Self {
            shared_ref: true,
            ..self
        }
    }
    pub fn as_immutable(self) -> Self {
        Self {
            immutable: true,
            shared_ref: true,
            ..self
        }
    }
}

/// Temporary region slot for A1; unlike TyId it is not an interned type.
/// rustc_middle/src/mir/syntax.rs:1382 carries ty::Region on Ref.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region(pub u32);
impl Region {
    pub const ERASED: Region = Region(u32::MAX);
}

/// File-qualified byte offsets, preserving BIR's lo/hi and body file identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

/// rustc_middle/src/mir/mod.rs:842.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceInfo {
    pub span: Span,
    pub scope: SourceScope,
}

/// rustc_middle/src/mir/mod.rs:1478; lexical scope identity survives lowering.
#[derive(Clone, Debug)]
pub struct SourceScopeData {
    pub span: Span,
    pub parent_scope: Option<SourceScope>,
}

/// rustc_middle/src/mir/mod.rs:118. Relax closures currently have expression identities.
#[derive(Clone, Debug)]
pub struct MirSource {
    pub owner: DefId,
    pub kind: BodyKind,
}
#[derive(Clone, Debug)]
pub enum BodyKind {
    Fn,
    Closure(u32),
    Const,
    Static,
    InlineConst,
    AnonConst,
    Promoted(DefId, PromotedId),
    Shim(ShimKind),
}
/// rustc_middle/src/ty/instance.rs:62; instance arguments remain on the owning query.
#[derive(Clone, Debug)]
pub enum ShimKind {
    FnPtr,
    ClosureOnce,
    DropGlue(TyId),
    Clone(TyId),
    Virtual(u32),
    Reify,
}

/// rustc_middle/src/mir/mod.rs:146. This slot is present before coroutine lowering.
#[derive(Clone, Debug)]
pub struct CoroutineInfo {
    pub yield_ty: TyId,
    pub resume_ty: TyId,
    pub kind: CoroutineKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoroutineKind {
    Coroutine,
    Async,
    Gen,
    AsyncGen,
}

/// rustc_middle/src/mir/mod.rs:967. Region skeletons belong to borrowck, not local declarations.
#[derive(Clone, Debug)]
pub struct LocalDecl {
    pub ty: TyId,
    pub mutability: Mutability,
    pub user_ty: Vec<UserTypeProjection>,
    pub source_info: SourceInfo,
    pub kind: LocalKind,
    pub local_info: LocalInfo,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    Return,
    Arg,
    UserVar,
    Temp,
}
/// Diagnostic facts from rustc_middle/src/mir/mod.rs:899,1074, not borrowck constraints.
#[derive(Clone, Debug, Default)]
pub enum LocalInfo {
    User {
        is_ref_binding: bool,
        can_be_made_mutable: bool,
        match_place: Option<(Option<Place>, Span)>,
    },
    StaticRef {
        def_id: DefId,
        is_thread_local: bool,
    },
    DerefTemp,
    FakeBorrow,
    GuardBinding(Local),
    BlockTail,
    /// A temporary created for an aggregate operand (mod.rs:1101 LocalInfo::AggregateTemp).
    AggregateTemp,
    #[default]
    Boring,
}

/// DESIGN-user-types supplies the canonical payload behind this opaque identity.
/// rustc_middle/src/mir/mod.rs:1563.
#[derive(Clone, Debug)]
pub struct UserTypeProjection {
    pub base: UserTypeAnnotationIndex,
    pub projection: Vec<PlaceElem>,
}
#[derive(Clone, Debug)]
pub struct VarDebugInfo {
    pub name: String,
    pub source_info: SourceInfo,
    pub place: Place,
}

/// rustc_middle/src/mir/mod.rs:206. Arguments occupy _1..=arg_count, return is _0.
#[derive(Clone, Debug)]
pub struct Body {
    pub phase: MirPhase,
    pub source: MirSource,
    pub basic_blocks: Vec<BasicBlockData>,
    pub local_decls: Vec<LocalDecl>,
    pub source_scopes: Vec<SourceScopeData>,
    pub user_type_annotations: Vec<UserTypeId>,
    pub var_debug_info: Vec<VarDebugInfo>,
    pub arg_count: u32,
    pub spread_arg: Option<Local>,
    pub promoted: Vec<Body>,
    pub coroutine: Option<CoroutineInfo>,
}

/// rustc_middle/src/mir/mod.rs:1319. An unfinished builder block may lack a terminator;
/// validation requires all blocks to be finished.
#[derive(Clone, Debug)]
pub struct BasicBlockData {
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
    pub is_cleanup: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub block: BasicBlock,
    pub statement_index: usize,
}

/// rustc_middle/src/mir/interpret/allocation.rs:95. AllocId is local to the interpreter's
/// allocation table; provenance offsets mark pointer starts, not host addresses.
#[derive(Clone, Debug)]
pub struct Allocation {
    pub bytes: Vec<u8>,
    pub align: u64,
    pub init_mask: Vec<bool>,
    pub deallocated: bool,
    pub provenance: Vec<(u64, CtfeProvenance)>,
    /// Byte fragments retain provenance across partial pointer copies (allocation.rs:95).
    pub provenance_fragments: Vec<(u64, CtfeProvenance, u8)>,
    pub mutability: Mutability,
}
#[derive(Clone, Debug, Default)]
pub struct AllocMap {
    pub allocations: Vec<Allocation>,
    /// Function provenance is stable across interpreter instances sharing this table.
    /// rustc_middle/src/mir/interpret/mod.rs (GlobalAlloc::Function).
    pub functions: Vec<(AllocId, DefId, Vec<TyId>)>,
}
impl AllocMap {
    pub fn insert(&mut self, allocation: Allocation) -> AllocId {
        let id = AllocId(u32::try_from(self.allocations.len()).expect("too many MIR allocations"));
        self.allocations.push(allocation);
        id
    }
    pub fn get(&self, id: AllocId) -> Option<&Allocation> {
        self.allocations.get(id.0 as usize)
    }
}

#[cfg(test)]
mod tests;
