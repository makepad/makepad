//! Language-item identities and collection, independent of crate and item spelling.
//! rustc_hir/src/lang_items.rs:153 (complete table); rustc_passes/src/lang_items.rs:60,175.

use crate::ast::{Ast, Attr, ItemKind};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::fxhash::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Trait, Struct, Union, Enum, Fn, Static, Variant, AssocConst, AssocTy,
    Method(MethodKind), ForeignTy, Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind { Inherent, Trait { body: bool }, TraitImpl }

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Self::Trait => "trait", Self::Struct => "struct", Self::Union => "union",
            Self::Enum => "enum", Self::Fn => "function", Self::Static => "static",
            Self::Variant => "enum variant", Self::AssocConst => "associated constant",
            Self::AssocTy => "associated type", Self::Method(MethodKind::Inherent) => "inherent method",
            Self::Method(MethodKind::Trait { body: false }) => "required trait method",
            Self::Method(MethodKind::Trait { body: true }) => "provided trait method",
            Self::Method(MethodKind::TraitImpl) => "trait implementation method", Self::ForeignTy => "foreign type", Self::Other => "item",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenericRequirement { None, Exact(usize), Minimum(usize) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LangItem {
    Sized,
    MetaSized,
    PointeeSized,
    Unsize,
    AlignOf,
    SizeOf,
    OffsetOf,
    StructuralPeq,
    Copy,
    Clone,
    CloneFn,
    UseCloned,
    TrivialClone,
    Sync,
    DiscriminantKind,
    Discriminant,
    PointeeTrait,
    Metadata,
    DynMetadata,
    Freeze,
    UnsafeUnpin,
    FnPtrTrait,
    FnPtrAsPtr,
    FnPtrFromPtr,
    Code,
    NonNull,
    Drop,
    Destruct,
    AsyncDrop,
    AsyncDropInPlace,
    CoerceUnsized,
    DispatchFromDyn,
    TransmuteOpts,
    TransmuteTrait,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    BitXor,
    BitAnd,
    BitOr,
    Shl,
    Shr,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    RemAssign,
    BitXorAssign,
    BitAndAssign,
    BitOrAssign,
    ShlAssign,
    ShrAssign,
    Index,
    IndexMut,
    UnsafeCell,
    UnsafePinned,
    VaArgSafe,
    VaList,
    Deref,
    DerefMut,
    DerefPure,
    DerefTarget,
    Receiver,
    ReceiverTarget,
    LegacyReceiver,
    Fn,
    FnMut,
    FnOnce,
    AsyncFn,
    AsyncFnMut,
    AsyncFnOnce,
    AsyncFnOnceOutput,
    CallOnceFuture,
    CallRefFuture,
    AsyncFnKindHelper,
    AsyncFnKindUpvars,
    FnOnceOutput,
    Iterator,
    FusedIterator,
    Future,
    FutureOutput,
    AsyncIterator,
    CoroutineState,
    Coroutine,
    CoroutineReturn,
    CoroutineYield,
    CoroutineResume,
    Unpin,
    Pin,
    OrderingEnum,
    PartialEq,
    PartialOrd,
    CVoid,
    Type,
    TypeId,
    Panic,
    PanicNounwind,
    PanicFmt,
    PanicDisplay,
    ConstPanicFmt,
    PanicBoundsCheck,
    PanicMisalignedPointerDereference,
    PanicInfo,
    PanicLocation,
    PanicImpl,
    PanicCannotUnwind,
    PanicInCleanup,
    PanicAddOverflow,
    PanicSubOverflow,
    PanicMulOverflow,
    PanicDivOverflow,
    PanicRemOverflow,
    PanicNegOverflow,
    PanicShrOverflow,
    PanicShlOverflow,
    PanicDivZero,
    PanicRemZero,
    PanicCoroutineResumed,
    PanicAsyncFnResumed,
    PanicAsyncGenFnResumed,
    PanicGenFnNone,
    PanicCoroutineResumedPanic,
    PanicAsyncFnResumedPanic,
    PanicAsyncGenFnResumedPanic,
    PanicGenFnNonePanic,
    PanicNullPointerDereference,
    PanicInvalidEnumConstruction,
    PanicCoroutineResumedDrop,
    PanicAsyncFnResumedDrop,
    PanicAsyncGenFnResumedDrop,
    PanicGenFnNoneDrop,
    BeginPanic,
    FormatArgument,
    FormatArguments,
    DropGlue,
    AllocLayout,
    Start,
    EhPersonality,
    EhCatchTypeinfo,
    CompilerMove,
    CompilerCopy,
    OwnedBox,
    GlobalAlloc,
    PhantomData,
    ManuallyDrop,
    MaybeDangling,
    BikeshedGuaranteedNoDrop,
    MaybeUninit,
    Termination,
    Try,
    Tuple,
    SliceLen,
    TryTraitFromResidual,
    TryTraitFromOutput,
    TryTraitBranch,
    TryTraitFromYeet,
    ResidualIntoTryType,
    CoercePointeeValidated,
    ConstParamTy,
    Poll,
    PollReady,
    PollPending,
    AsyncGenReady,
    AsyncGenPending,
    AsyncGenFinished,
    ResumeTy,
    GetContext,
    Context,
    FuturePoll,
    AsyncIteratorPollNext,
    IntoAsyncIterIntoIter,
    Option,
    OptionSome,
    OptionNone,
    ResultOk,
    ResultErr,
    ControlFlowContinue,
    ControlFlowBreak,
    IntoFutureIntoFuture,
    IntoIterIntoIter,
    IteratorNext,
    PinNewUnchecked,
    RangeFrom,
    RangeFull,
    RangeInclusiveStruct,
    RangeInclusiveNew,
    Range,
    RangeToInclusive,
    RangeTo,
    RangeMax,
    RangeMin,
    RangeSub,
    RangeFromCopy,
    RangeCopy,
    RangeInclusiveCopy,
    RangeToInclusiveCopy,
    String,
    CStr,
    ContractBuildCheckEnsures,
    ContractCheckRequires,
    DefaultTrait4,
    DefaultTrait3,
    DefaultTrait2,
    DefaultTrait1,
    ContractCheckEnsures,
    Reborrow,
    CoerceShared,
    FieldRepresentingType,
    Field,
    FieldBase,
    FieldType,
    FieldOffset,
    From,
}


struct LangItemData {
    item: LangItem,
    name: &'static str,
    target: Target,
    generics: GenericRequirement,
}

static LANG_ITEM_DATA: &[LangItemData] = &[
    LangItemData { item: LangItem::Sized, name: "sized", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::MetaSized, name: "meta_sized", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PointeeSized, name: "pointee_sized", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::Unsize, name: "unsize", target: Target::Trait, generics: GenericRequirement::Minimum(1) },
    LangItemData { item: LangItem::AlignOf, name: "mem_align_const", target: Target::AssocConst, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::SizeOf, name: "mem_size_const", target: Target::AssocConst, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::OffsetOf, name: "offset_of", target: Target::Fn, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::StructuralPeq, name: "structural_peq", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Copy, name: "copy", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::Clone, name: "clone", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::CloneFn, name: "clone_fn", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::UseCloned, name: "use_cloned", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::TrivialClone, name: "trivial_clone", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Sync, name: "sync", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::DiscriminantKind, name: "discriminant_kind", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Discriminant, name: "discriminant_type", target: Target::AssocTy, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PointeeTrait, name: "pointee_trait", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Metadata, name: "metadata_type", target: Target::AssocTy, generics: GenericRequirement::None },
    LangItemData { item: LangItem::DynMetadata, name: "dyn_metadata", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Freeze, name: "freeze", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::UnsafeUnpin, name: "unsafe_unpin", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FnPtrTrait, name: "fn_ptr_trait", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FnPtrAsPtr, name: "fn_ptr_as_ptr", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::FnPtrFromPtr, name: "fn_ptr_from_ptr", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::Code, name: "code", target: Target::ForeignTy, generics: GenericRequirement::None },
    LangItemData { item: LangItem::NonNull, name: "non_null", target: Target::Struct, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Drop, name: "drop", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Destruct, name: "destruct", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::AsyncDrop, name: "async_drop", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::AsyncDropInPlace, name: "async_drop_in_place", target: Target::Fn, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CoerceUnsized, name: "coerce_unsized", target: Target::Trait, generics: GenericRequirement::Minimum(1) },
    LangItemData { item: LangItem::DispatchFromDyn, name: "dispatch_from_dyn", target: Target::Trait, generics: GenericRequirement::Minimum(1) },
    LangItemData { item: LangItem::TransmuteOpts, name: "transmute_opts", target: Target::Struct, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::TransmuteTrait, name: "transmute_trait", target: Target::Trait, generics: GenericRequirement::Exact(2) },
    LangItemData { item: LangItem::Add, name: "add", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Sub, name: "sub", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Mul, name: "mul", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Div, name: "div", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Rem, name: "rem", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Neg, name: "neg", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::Not, name: "not", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::BitXor, name: "bitxor", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::BitAnd, name: "bitand", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::BitOr, name: "bitor", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Shl, name: "shl", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Shr, name: "shr", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AddAssign, name: "add_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::SubAssign, name: "sub_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::MulAssign, name: "mul_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::DivAssign, name: "div_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::RemAssign, name: "rem_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::BitXorAssign, name: "bitxor_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::BitAndAssign, name: "bitand_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::BitOrAssign, name: "bitor_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::ShlAssign, name: "shl_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::ShrAssign, name: "shr_assign", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::Index, name: "index", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::IndexMut, name: "index_mut", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::UnsafeCell, name: "unsafe_cell", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::UnsafePinned, name: "unsafe_pinned", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::VaArgSafe, name: "va_arg_safe", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::VaList, name: "va_list", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Deref, name: "deref", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::DerefMut, name: "deref_mut", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::DerefPure, name: "deref_pure", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::DerefTarget, name: "deref_target", target: Target::AssocTy, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Receiver, name: "receiver", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ReceiverTarget, name: "receiver_target", target: Target::AssocTy, generics: GenericRequirement::None },
    LangItemData { item: LangItem::LegacyReceiver, name: "legacy_receiver", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Fn, name: "fn", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::FnMut, name: "fn_mut", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::FnOnce, name: "fn_once", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncFn, name: "async_fn", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncFnMut, name: "async_fn_mut", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncFnOnce, name: "async_fn_once", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncFnOnceOutput, name: "async_fn_once_output", target: Target::AssocTy, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CallOnceFuture, name: "call_once_future", target: Target::AssocTy, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CallRefFuture, name: "call_ref_future", target: Target::AssocTy, generics: GenericRequirement::Exact(2) },
    LangItemData { item: LangItem::AsyncFnKindHelper, name: "async_fn_kind_helper", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncFnKindUpvars, name: "async_fn_kind_upvars", target: Target::AssocTy, generics: GenericRequirement::Exact(5) },
    LangItemData { item: LangItem::FnOnceOutput, name: "fn_once_output", target: Target::AssocTy, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Iterator, name: "iterator", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FusedIterator, name: "fused_iterator", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::Future, name: "future_trait", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FutureOutput, name: "future_output", target: Target::AssocTy, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::AsyncIterator, name: "async_iterator", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::CoroutineState, name: "coroutine_state", target: Target::Enum, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Coroutine, name: "coroutine", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CoroutineReturn, name: "coroutine_return", target: Target::AssocTy, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CoroutineYield, name: "coroutine_yield", target: Target::AssocTy, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CoroutineResume, name: "coroutine_resume", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::Unpin, name: "unpin", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Pin, name: "pin", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::OrderingEnum, name: "Ordering", target: Target::Enum, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PartialEq, name: "eq", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::PartialOrd, name: "partial_ord", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::CVoid, name: "c_void", target: Target::Enum, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Type, name: "type_info", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::TypeId, name: "type_id", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Panic, name: "panic", target: Target::Fn, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PanicNounwind, name: "panic_nounwind", target: Target::Fn, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PanicFmt, name: "panic_fmt", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicDisplay, name: "panic_display", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ConstPanicFmt, name: "const_panic_fmt", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicBoundsCheck, name: "panic_bounds_check", target: Target::Fn, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PanicMisalignedPointerDereference, name: "panic_misaligned_pointer_dereference", target: Target::Fn, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PanicInfo, name: "panic_info", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicLocation, name: "panic_location", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicImpl, name: "panic_impl", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicCannotUnwind, name: "panic_cannot_unwind", target: Target::Fn, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PanicInCleanup, name: "panic_in_cleanup", target: Target::Fn, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::PanicAddOverflow, name: "panic_const_add_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicSubOverflow, name: "panic_const_sub_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicMulOverflow, name: "panic_const_mul_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicDivOverflow, name: "panic_const_div_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicRemOverflow, name: "panic_const_rem_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicNegOverflow, name: "panic_const_neg_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicShrOverflow, name: "panic_const_shr_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicShlOverflow, name: "panic_const_shl_overflow", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicDivZero, name: "panic_const_div_by_zero", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicRemZero, name: "panic_const_rem_by_zero", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicCoroutineResumed, name: "panic_const_coroutine_resumed", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicAsyncFnResumed, name: "panic_const_async_fn_resumed", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicAsyncGenFnResumed, name: "panic_const_async_gen_fn_resumed", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicGenFnNone, name: "panic_const_gen_fn_none", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicCoroutineResumedPanic, name: "panic_const_coroutine_resumed_panic", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicAsyncFnResumedPanic, name: "panic_const_async_fn_resumed_panic", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicAsyncGenFnResumedPanic, name: "panic_const_async_gen_fn_resumed_panic", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicGenFnNonePanic, name: "panic_const_gen_fn_none_panic", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicNullPointerDereference, name: "panic_null_pointer_dereference", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicInvalidEnumConstruction, name: "panic_invalid_enum_construction", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicCoroutineResumedDrop, name: "panic_const_coroutine_resumed_drop", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicAsyncFnResumedDrop, name: "panic_const_async_fn_resumed_drop", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicAsyncGenFnResumedDrop, name: "panic_const_async_gen_fn_resumed_drop", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PanicGenFnNoneDrop, name: "panic_const_gen_fn_none_drop", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::BeginPanic, name: "begin_panic", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::FormatArgument, name: "format_argument", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::FormatArguments, name: "format_arguments", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::DropGlue, name: "drop_glue", target: Target::Fn, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AllocLayout, name: "alloc_layout", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Start, name: "start", target: Target::Fn, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::EhPersonality, name: "eh_personality", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::EhCatchTypeinfo, name: "eh_catch_typeinfo", target: Target::Static, generics: GenericRequirement::None },
    LangItemData { item: LangItem::CompilerMove, name: "compiler_move", target: Target::Fn, generics: GenericRequirement::Exact(2) },
    LangItemData { item: LangItem::CompilerCopy, name: "compiler_copy", target: Target::Fn, generics: GenericRequirement::Exact(2) },
    LangItemData { item: LangItem::OwnedBox, name: "owned_box", target: Target::Struct, generics: GenericRequirement::Minimum(1) },
    LangItemData { item: LangItem::GlobalAlloc, name: "global_alloc_ty", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PhantomData, name: "phantom_data", target: Target::Struct, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::ManuallyDrop, name: "manually_drop", target: Target::Struct, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::MaybeDangling, name: "maybe_dangling", target: Target::Struct, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::BikeshedGuaranteedNoDrop, name: "bikeshed_guaranteed_no_drop", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::MaybeUninit, name: "maybe_uninit", target: Target::Union, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Termination, name: "termination", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Try, name: "Try", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Tuple, name: "tuple_trait", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::SliceLen, name: "slice_len_fn", target: Target::Method(MethodKind::Inherent), generics: GenericRequirement::None },
    LangItemData { item: LangItem::TryTraitFromResidual, name: "from_residual", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::TryTraitFromOutput, name: "from_output", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::TryTraitBranch, name: "branch", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::TryTraitFromYeet, name: "from_yeet", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ResidualIntoTryType, name: "into_try_type", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::CoercePointeeValidated, name: "coerce_pointee_validated", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::ConstParamTy, name: "const_param_ty", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::Poll, name: "Poll", target: Target::Enum, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PollReady, name: "Ready", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::PollPending, name: "Pending", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::AsyncGenReady, name: "AsyncGenReady", target: Target::Method(MethodKind::Inherent), generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncGenPending, name: "AsyncGenPending", target: Target::AssocConst, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::AsyncGenFinished, name: "AsyncGenFinished", target: Target::AssocConst, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::ResumeTy, name: "ResumeTy", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::GetContext, name: "get_context", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Context, name: "Context", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::FuturePoll, name: "poll", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::AsyncIteratorPollNext, name: "async_iterator_poll_next", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::IntoAsyncIterIntoIter, name: "into_async_iter_into_iter", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::Option, name: "Option", target: Target::Enum, generics: GenericRequirement::None },
    LangItemData { item: LangItem::OptionSome, name: "Some", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::OptionNone, name: "None", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ResultOk, name: "Ok", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ResultErr, name: "Err", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ControlFlowContinue, name: "Continue", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ControlFlowBreak, name: "Break", target: Target::Variant, generics: GenericRequirement::None },
    LangItemData { item: LangItem::IntoFutureIntoFuture, name: "into_future", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::IntoIterIntoIter, name: "into_iter", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::None },
    LangItemData { item: LangItem::IteratorNext, name: "next", target: Target::Method(MethodKind::Trait { body: false}), generics: GenericRequirement::None },
    LangItemData { item: LangItem::PinNewUnchecked, name: "new_unchecked", target: Target::Method(MethodKind::Inherent), generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeFrom, name: "RangeFrom", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeFull, name: "RangeFull", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeInclusiveStruct, name: "RangeInclusive", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeInclusiveNew, name: "range_inclusive_new", target: Target::Method(MethodKind::Inherent), generics: GenericRequirement::None },
    LangItemData { item: LangItem::Range, name: "Range", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeToInclusive, name: "RangeToInclusive", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeTo, name: "RangeTo", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeMax, name: "RangeMax", target: Target::AssocConst, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::RangeMin, name: "RangeMin", target: Target::AssocConst, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::RangeSub, name: "RangeSub", target: Target::Method(MethodKind::Trait { body: false }), generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::RangeFromCopy, name: "RangeFromCopy", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeCopy, name: "RangeCopy", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeInclusiveCopy, name: "RangeInclusiveCopy", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::RangeToInclusiveCopy, name: "RangeToInclusiveCopy", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::String, name: "String", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::CStr, name: "CStr", target: Target::Struct, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ContractBuildCheckEnsures, name: "contract_build_check_ensures", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ContractCheckRequires, name: "contract_check_requires", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::DefaultTrait4, name: "default_trait4", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::DefaultTrait3, name: "default_trait3", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::DefaultTrait2, name: "default_trait2", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::DefaultTrait1, name: "default_trait1", target: Target::Trait, generics: GenericRequirement::None },
    LangItemData { item: LangItem::ContractCheckEnsures, name: "contract_check_ensures", target: Target::Fn, generics: GenericRequirement::None },
    LangItemData { item: LangItem::Reborrow, name: "reborrow", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::CoerceShared, name: "coerce_shared", target: Target::Trait, generics: GenericRequirement::Exact(1) },
    LangItemData { item: LangItem::FieldRepresentingType, name: "field_representing_type", target: Target::Struct, generics: GenericRequirement::Exact(3) },
    LangItemData { item: LangItem::Field, name: "field", target: Target::Trait, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FieldBase, name: "field_base", target: Target::AssocTy, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FieldType, name: "field_type", target: Target::AssocTy, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::FieldOffset, name: "field_offset", target: Target::AssocConst, generics: GenericRequirement::Exact(0) },
    LangItemData { item: LangItem::From, name: "From", target: Target::Trait, generics: GenericRequirement::Exact(1) },
];

impl LangItem {
    pub fn name(self) -> &'static str { LANG_ITEM_DATA[self as usize].name }
    pub fn target(self) -> Target { LANG_ITEM_DATA[self as usize].target }
    pub fn required_generics(self) -> GenericRequirement { LANG_ITEM_DATA[self as usize].generics }
    pub fn from_name(name: &str) -> Option<Self> {
        for entry in LANG_ITEM_DATA {
            if entry.name == name { return Some(entry.item); }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingLangItem(pub LangItem);

impl std::fmt::Display for MissingLangItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "requires `{}` lang_item", self.0.name())
    }
}

/// rustc_hir/src/lang_items.rs:25. Populated before signatures, including dependency items.
pub struct LangItems {
    pub items: Vec<Option<DefId>>,
    pub by_def: HashMap<DefId, LangItem>,
}

impl Default for LangItems {
    fn default() -> Self {
        Self { items: vec![None; LANG_ITEM_DATA.len()], by_def: HashMap::default() }
    }
}

impl LangItems {
    pub fn get(&self, item: LangItem) -> Option<DefId> { self.items[item as usize] }
    pub fn require(&self, item: LangItem) -> Result<DefId, MissingLangItem> {
        self.get(item).ok_or(MissingLangItem(item))
    }

    /// rustc_passes/src/lang_items.rs:60,95,175: validate target/generics before insertion.
    /// Repeated collection of the same definition is idempotent; duplicates keep the first.
    pub fn collect(&mut self, prog: &Program, defs: &[DefId], errors: &mut Vec<String>) {
        for &did in defs {
            let def = prog.def(did);
            if def.parent == NO_DEF || matches!(def.kind, DefKind::Prim(_) | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Closure | DefKind::Field) { continue; }
            let ast = &prog.files[def.file as usize].ast;
            let item = ast.item(def.item);
            let attrs = if def.kind == DefKind::Variant {
                let parent = prog.def(def.parent);
                let Some(en) = ast.as_enum(&ast.item(parent.item).kind) else { continue };
                ast.list(ast.list(en.variants)[def.sub as usize].attrs)
            } else {
                ast.list(item.attrs)
            };
            let src = prog.src(def.file);
            for attr in attrs {
                let lo = attr.toks.lo as usize;
                if crate::cfg::tok_str(src, lo) != b"lang" { continue; }
                let (line, col) = prog.files[def.file as usize].line_col(src.tok(lo).lo);
                let location = format!("{}:{}:{}", prog.file_paths[def.file as usize], line, col);
                if attr.toks.hi as usize != lo + 3 || crate::cfg::tok_str(src, lo + 1) != b"="
                    || !matches!(src.tok(lo + 2).kind, crate::lexer::T::Str | crate::lexer::T::RawStr) {
                    errors.push(format!("{location}: malformed `lang` attribute"));
                    break;
                }
                let value = std::str::from_utf8(crate::cfg::tok_str(src, lo + 2)).unwrap_or("");
                let name = crate::lit::unescape_str(value);
                let Some(lang) = LangItem::from_name(&name) else {
                    errors.push(format!("{location}: E0522: definition of an unknown lang item: `{name}`"));
                    break;
                };
                let actual = target(prog, did);
                if actual != lang.target() {
                    errors.push(format!("{location}: E0718: `{name}` lang item must be applied to a {}, not a {}", lang.target().name(), actual.name()));
                    break;
                }
                if let Some(mut count) = generic_count(ast, &item.kind) {
                    if matches!(actual, Target::Method(_) | Target::AssocTy | Target::AssocConst) {
                        let parent = prog.def(def.parent);
                        let pa = &prog.files[parent.file as usize].ast;
                        count += generic_count(pa, &pa.item(parent.item).kind).unwrap_or(0);
                    }
                    let bad = match lang.required_generics() {
                        GenericRequirement::Exact(n) if count != n => Some((n, "")),
                        GenericRequirement::Minimum(n) if count < n => Some((n, "at least ")),
                        _ => None,
                    };
                    if let Some((n, at_least)) = bad {
                        errors.push(format!("{location}: E0718: `{name}` lang item must be applied to a {} with {at_least}{n} generic arguments; this item has {count}", actual.name()));
                        break;
                    }
                }
                if let Some(previous) = self.get(lang) {
                    if previous != did {
                        errors.push(format!("{location}: E0152: found duplicate lang item `{name}`; first defined at `{}`", prog.def_path(previous)));
                    }
                } else {
                    self.items[lang as usize] = Some(did);
                    self.by_def.insert(did, lang);
                }
                // rustc's extract_ast chooses the first lang attribute on a definition.
                break;
            }
        }
    }
}

pub fn has_attr(prog: &Program, file: u32, attrs: &[Attr], name: &str) -> bool {
    for attr in attrs {
        if crate::cfg::tok_str(prog.src(file), attr.toks.lo as usize) == name.as_bytes() { return true; }
    }
    false
}

fn generic_count(ast: &Ast, kind: &ItemKind) -> Option<usize> {
    let g = match kind {
        ItemKind::Const(..) => return Some(0),
        k => ast.kind_generics(k)?,
    };
    Some(ast.list(g.params).len())
}

fn target(prog: &Program, did: DefId) -> Target {
    let def = prog.def(did);
    match def.kind {
        DefKind::Trait => Target::Trait,
        DefKind::Struct => Target::Struct,
        DefKind::Union => Target::Union,
        DefKind::Enum => Target::Enum,
        DefKind::Variant => Target::Variant,
        DefKind::Fn | DefKind::ForeignFn => Target::Fn,
        DefKind::Static | DefKind::ForeignStatic => Target::Static,
        DefKind::ForeignTy => Target::ForeignTy,
        DefKind::AssocTy => Target::AssocTy,
        DefKind::AssocConst => Target::AssocConst,
        DefKind::AssocFn => {
            let parent = prog.def(def.parent);
            let ast = &prog.files[def.file as usize].ast;
            if parent.kind == DefKind::Trait {
                let body = match ast.item(def.item).kind {
                    ItemKind::Fn(sig, body, _) => body.is_some() || ast.fn_sig(sig).lazy_body.is_some(),
                    _ => false,
                };
                Target::Method(MethodKind::Trait { body })
            } else {
                let pa = &prog.files[parent.file as usize].ast;
                match pa.as_impl(&pa.item(parent.item).kind) {
                    Some(i) if i.trait_.is_some() => Target::Method(MethodKind::TraitImpl),
                    _ => Target::Method(MethodKind::Inherent),
                }
            }
        }
        _ => Target::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(sess: &std::sync::Arc<crate::Session>, source: &str) -> (Program, LangItems, Vec<String>) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("relax-lang-{}-{n}.rs", std::process::id()));
        std::fs::write(&root, source).unwrap();
        let mut prog = Program::new(sess);
        prog.load_crate("renamed", root.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        std::fs::remove_file(root).unwrap();
        let mut defs = Vec::new();
        for i in 0..prog.defs.len() { defs.push(DefId(i as u32)); }
        let mut items = LangItems::default();
        let mut errors = Vec::new();
        items.collect(&prog, &defs, &mut errors);
        (prog, items, errors)
    }

    #[test]
    fn lang_items_table() {
        // rustc_attr_ir/src/lang_items.rs @8d1a764: N01 replaced FnPtrAddr with FnPtrAsPtr,
        // FnPtrFromPtr and Code, and added NonNull (all nightly items).
        assert_eq!(LANG_ITEM_DATA.len(), 217);
        for (i, entry) in LANG_ITEM_DATA.iter().enumerate() {
            assert_eq!(entry.item as usize, i);
            assert_eq!(LangItem::from_name(entry.name), Some(entry.item));
        }
        assert_eq!(LangItems::default().require(LangItem::Drop), Err(MissingLangItem(LangItem::Drop)));
    }

    #[test]
    fn lang_items_collection() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (prog, mut items, errors) = collect(sess, r#"
            #[lang = "owned_box"] pub struct Heap<T>(*mut T);
            #[lang = "drop"] pub trait Destroy { fn drop(&mut self); }
            #[lang = "Option"] pub enum Choice<T> {
                #[lang = "Some"] Yes(T), #[lang = "None"] No,
            }
            pub trait Producer {
                #[lang = "next"] fn advance(&mut self);
            }
        "#);
        assert!(errors.is_empty(), "{errors:?}");
        for (li, name) in [(LangItem::OwnedBox, "Heap"), (LangItem::Drop, "Destroy"),
            (LangItem::OptionSome, "Yes"), (LangItem::OptionNone, "No"), (LangItem::IteratorNext, "advance")] {
            let did = items.require(li).unwrap();
            assert_eq!(prog.name(did), name);
            assert_eq!(items.by_def.get(&did), Some(&li));
        }
        let mut errors = Vec::new();
        let did = items.require(LangItem::Drop).unwrap();
        items.collect(&prog, &[did], &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn lang_items_duplicate_and_target_errors() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (_, items, errors) = collect(sess, r#"
            #[lang = "owned_box"] struct First<T>(*mut T);
            #[lang = "owned_box"] struct Second<T>(*mut T);
            #[lang = "drop"] struct Wrong;
            #[lang = "manually_drop"] union WrongUnion<T> { value: T }
            #[lang = "phantom_data"] struct NoGenerics;
            trait Trait<T> { #[lang = "next"] fn provided(&mut self) {} }
            #[lang = "unknown_language_item"] struct Unknown;
            #[lang = r] struct Malformed;
        "#);
        assert_eq!(errors.len(), 7, "{errors:?}");
        assert!(errors[0].contains("E0152"), "{errors:?}");
        assert_eq!(errors.iter().filter(|e| e.contains("E0718")).count(), 4);
        assert!(errors[5].contains("E0522"));
        assert!(errors[6].contains("malformed"));
        assert!(items.get(LangItem::OwnedBox).is_some());
        assert!(items.get(LangItem::Drop).is_none());
        assert!(items.get(LangItem::ManuallyDrop).is_none());
    }

    #[test]
    fn lang_items_generics_include_parent_and_lifetimes() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (_, items, errors) = collect(sess, r#"
            struct Parent<T>(T);
            impl<T> Parent<T> {
                #[lang = "AsyncGenReady"] fn ready() {}
                #[lang = "AsyncGenPending"] const PENDING: u8 = 0;
                #[lang = "mem_size_const"] const BAD: u8 = 0;
            }
            trait WithLifetime<'a> { #[lang = "async_iterator_poll_next"] fn poll(&mut self); }
            #[lang = "phantom_data"] struct TooMany<'a, T>(&'a T);
        "#);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(items.get(LangItem::AsyncGenPending).is_some());
        assert!(items.get(LangItem::SizeOf).is_none());
        assert!(items.get(LangItem::AsyncGenReady).is_some());
        assert!(items.get(LangItem::AsyncIteratorPollNext).is_none());
        assert!(items.get(LangItem::PhantomData).is_none());
    }
}
