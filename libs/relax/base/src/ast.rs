//! Syntax tree. One `Ast` per file; nodes live in per-kind arenas and refer to
//! each other by index. Names are source spans (`Ident`), resolved later.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ident {
    pub lo: u32,
    pub hi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TyId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockId(pub u32);

/// ExprKind::Async capture modes: by use, all by value, and an async closure's body (the
/// closure's parameters by value, its upvars by use -- rustc's async closure coroutine)
pub const ASYNC_PLAIN: u8 = 0;
pub const ASYNC_MOVE: u8 = 1;
pub const ASYNC_CLOSURE_BODY: u8 = 2;

/// A token range `[lo, hi)` into the file's token list (macro bodies, attribute args).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokRange {
    pub lo: u32,
    pub hi: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Attr {
    pub inner: bool,
    /// tokens inside `#[ ... ]`
    pub toks: TokRange,
}

#[derive(Debug, Clone, Copy)]
pub enum Vis {
    Private,
    Pub,
    Crate,
    Super,
    SelfMod,
    In(Row<Path>),
}

/// `len` consecutive entries of one of its tree's flat arrays from `lo` (Ast::list): the
/// position-independent form of a node's list of children.
pub struct List<T> {
    pub lo: u32,
    pub len: u32,
    _t: std::marker::PhantomData<fn() -> T>,
}

impl<T> List<T> {
    pub const EMPTY: List<T> = List { lo: 0, len: 0, _t: std::marker::PhantomData };
    #[inline]
    pub fn new(lo: u32, len: u32) -> List<T> {
        List { lo, len, _t: std::marker::PhantomData }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T> Clone for List<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for List<T> {}
impl<T> Default for List<T> {
    fn default() -> Self {
        List::EMPTY
    }
}
impl<T> PartialEq for List<T> {
    fn eq(&self, o: &Self) -> bool {
        self.lo == o.lo && self.len == o.len
    }
}
impl<T> std::fmt::Debug for List<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "List({}+{})", self.lo, self.len)
    }
}

/// One entry of one of its tree's row arrays (Ast::row): the big, rare payload of a node kept
/// out of the node so the hot nodes stay small.
pub struct Row<T> {
    pub id: u32,
    _t: std::marker::PhantomData<fn() -> T>,
}

impl<T> Row<T> {
    #[inline]
    pub fn new(id: u32) -> Row<T> {
        Row { id, _t: std::marker::PhantomData }
    }
}
impl<T> Clone for Row<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Row<T> {}
impl<T> PartialEq for Row<T> {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}
impl<T> Eq for Row<T> {}
impl<T> std::fmt::Debug for Row<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Row({})", self.id)
    }
}

/// The flat arrays a tree's lists live in (index into Ast::base).
pub trait FlatArray<T> {
    const BASE: usize;
    fn array(a: &Ast) -> &[T];
    fn array_mut(a: &mut Ast) -> &mut Vec<T>;
}

impl FlatArray<Stmt> for Ast {
    const BASE: usize = 6;
    fn array(a: &Ast) -> &[Stmt] {
        &a.stmts
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Stmt> {
        a.stmts.vec_mut()
    }
}

impl FlatArray<ExprId> for Ast {
    const BASE: usize = 7;
    fn array(a: &Ast) -> &[ExprId] {
        &a.expr_lists
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ExprId> {
        a.expr_lists.vec_mut()
    }
}

impl FlatArray<PatId> for Ast {
    const BASE: usize = 8;
    fn array(a: &Ast) -> &[PatId] {
        &a.pat_lists
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<PatId> {
        a.pat_lists.vec_mut()
    }
}

impl FlatArray<TyId> for Ast {
    const BASE: usize = 9;
    fn array(a: &Ast) -> &[TyId] {
        &a.ty_lists
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<TyId> {
        a.ty_lists.vec_mut()
    }
}

impl FlatArray<FieldExpr> for Ast {
    const BASE: usize = 10;
    fn array(a: &Ast) -> &[FieldExpr] {
        &a.field_exprs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<FieldExpr> {
        a.field_exprs.vec_mut()
    }
}

impl FlatArray<Arm> for Ast {
    const BASE: usize = 11;
    fn array(a: &Ast) -> &[Arm] {
        &a.arms
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Arm> {
        a.arms.vec_mut()
    }
}

impl FlatArray<ClosureParam> for Ast {
    const BASE: usize = 12;
    fn array(a: &Ast) -> &[ClosureParam] {
        &a.closure_params
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ClosureParam> {
        a.closure_params.vec_mut()
    }
}

impl FlatArray<FieldPat> for Ast {
    const BASE: usize = 13;
    fn array(a: &Ast) -> &[FieldPat] {
        &a.field_pats
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<FieldPat> {
        a.field_pats.vec_mut()
    }
}

impl FlatArray<GenericArg> for Ast {
    const BASE: usize = 17;
    fn array(a: &Ast) -> &[GenericArg] {
        &a.generic_arg_lists
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<GenericArg> {
        a.generic_arg_lists.vec_mut()
    }
}

impl FlatArray<Bound> for Ast {
    const BASE: usize = 18;
    fn array(a: &Ast) -> &[Bound] {
        &a.bounds
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Bound> {
        a.bounds.vec_mut()
    }
}

impl FlatArray<GenericParam> for Ast {
    const BASE: usize = 19;
    fn array(a: &Ast) -> &[GenericParam] {
        &a.generic_params
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<GenericParam> {
        a.generic_params.vec_mut()
    }
}

impl FlatArray<Ident> for Ast {
    const BASE: usize = 20;
    fn array(a: &Ast) -> &[Ident] {
        &a.ident_lists
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Ident> {
        a.ident_lists.vec_mut()
    }
}

impl FlatArray<Attr> for Ast {
    const BASE: usize = 21;
    fn array(a: &Ast) -> &[Attr] {
        &a.attrs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Attr> {
        a.attrs.vec_mut()
    }
}

impl FlatArray<AsmOp> for Ast {
    const BASE: usize = 22;
    fn array(a: &Ast) -> &[AsmOp] {
        &a.asm_ops
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<AsmOp> {
        a.asm_ops.vec_mut()
    }
}

impl FlatArray<Param> for Ast {
    const BASE: usize = 25;
    fn array(a: &Ast) -> &[Param] {
        &a.params
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Param> {
        a.params.vec_mut()
    }
}

impl FlatArray<FieldDef> for Ast {
    const BASE: usize = 26;
    fn array(a: &Ast) -> &[FieldDef] {
        &a.field_defs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<FieldDef> {
        a.field_defs.vec_mut()
    }
}

impl FlatArray<Variant> for Ast {
    const BASE: usize = 27;
    fn array(a: &Ast) -> &[Variant] {
        &a.variants
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Variant> {
        a.variants.vec_mut()
    }
}

impl FlatArray<UseTree> for Ast {
    const BASE: usize = 28;
    fn array(a: &Ast) -> &[UseTree] {
        &a.use_trees
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<UseTree> {
        a.use_trees.vec_mut()
    }
}

impl FlatArray<ItemId> for Ast {
    const BASE: usize = 29;
    fn array(a: &Ast) -> &[ItemId] {
        &a.item_lists
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ItemId> {
        a.item_lists.vec_mut()
    }
}

impl FlatArray<MethodCallRow> for Ast {
    const BASE: usize = 31;
    fn array(a: &Ast) -> &[MethodCallRow] {
        &a.method_calls
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<MethodCallRow> {
        a.method_calls.vec_mut()
    }
}

impl FlatArray<ForRow> for Ast {
    const BASE: usize = 32;
    fn array(a: &Ast) -> &[ForRow] {
        &a.for_exprs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ForRow> {
        a.for_exprs.vec_mut()
    }
}

impl FlatArray<ClosureRow> for Ast {
    const BASE: usize = 33;
    fn array(a: &Ast) -> &[ClosureRow] {
        &a.closure_exprs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ClosureRow> {
        a.closure_exprs.vec_mut()
    }
}

impl FlatArray<StructRow> for Ast {
    const BASE: usize = 34;
    fn array(a: &Ast) -> &[StructRow] {
        &a.struct_exprs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<StructRow> {
        a.struct_exprs.vec_mut()
    }
}

impl FlatArray<MacCall> for Ast {
    const BASE: usize = 35;
    fn array(a: &Ast) -> &[MacCall] {
        &a.mac_calls
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<MacCall> {
        a.mac_calls.vec_mut()
    }
}

impl FlatArray<WhileRow> for Ast {
    const BASE: usize = 36;
    fn array(a: &Ast) -> &[WhileRow] {
        &a.while_exprs
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<WhileRow> {
        a.while_exprs.vec_mut()
    }
}

impl FlatArray<LetStmt> for Ast {
    const BASE: usize = 37;
    fn array(a: &Ast) -> &[LetStmt] {
        &a.let_stmts
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<LetStmt> {
        a.let_stmts.vec_mut()
    }
}

impl FlatArray<AttrsStmt> for Ast {
    const BASE: usize = 38;
    fn array(a: &Ast) -> &[AttrsStmt] {
        &a.attrs_stmts
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<AttrsStmt> {
        a.attrs_stmts.vec_mut()
    }
}

impl FlatArray<UseItem> for Ast {
    const BASE: usize = 39;
    fn array(a: &Ast) -> &[UseItem] {
        &a.use_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<UseItem> {
        a.use_items.vec_mut()
    }
}

impl FlatArray<StructItem> for Ast {
    const BASE: usize = 40;
    fn array(a: &Ast) -> &[StructItem] {
        &a.struct_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<StructItem> {
        a.struct_items.vec_mut()
    }
}

impl FlatArray<UnionItem> for Ast {
    const BASE: usize = 41;
    fn array(a: &Ast) -> &[UnionItem] {
        &a.union_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<UnionItem> {
        a.union_items.vec_mut()
    }
}

impl FlatArray<EnumItem> for Ast {
    const BASE: usize = 42;
    fn array(a: &Ast) -> &[EnumItem] {
        &a.enum_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<EnumItem> {
        a.enum_items.vec_mut()
    }
}

impl FlatArray<TypeAliasItem> for Ast {
    const BASE: usize = 43;
    fn array(a: &Ast) -> &[TypeAliasItem] {
        &a.alias_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<TypeAliasItem> {
        a.alias_items.vec_mut()
    }
}

impl FlatArray<TraitItem> for Ast {
    const BASE: usize = 44;
    fn array(a: &Ast) -> &[TraitItem] {
        &a.trait_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<TraitItem> {
        a.trait_items.vec_mut()
    }
}

impl FlatArray<TraitAliasItem> for Ast {
    const BASE: usize = 45;
    fn array(a: &Ast) -> &[TraitAliasItem] {
        &a.trait_alias_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<TraitAliasItem> {
        a.trait_alias_items.vec_mut()
    }
}

impl FlatArray<ImplItem> for Ast {
    const BASE: usize = 46;
    fn array(a: &Ast) -> &[ImplItem] {
        &a.impl_items
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ImplItem> {
        a.impl_items.vec_mut()
    }
}

impl FlatArray<ForeignModItem> for Ast {
    const BASE: usize = 47;
    fn array(a: &Ast) -> &[ForeignModItem] {
        &a.foreign_mods
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<ForeignModItem> {
        a.foreign_mods.vec_mut()
    }
}

impl FlatArray<Path> for Ast {
    const BASE: usize = 48;
    fn array(a: &Ast) -> &[Path] {
        &a.vis_paths
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<Path> {
        a.vis_paths.vec_mut()
    }
}

impl FlatArray<WherePred> for Ast {
    const BASE: usize = 30;
    fn array(a: &Ast) -> &[WherePred] {
        &a.where_preds
    }
    fn array_mut(a: &mut Ast) -> &mut Vec<WherePred> {
        a.where_preds.vec_mut()
    }
}

/// A path's segments: `len` consecutive entries of its tree's segment array from `lo`
/// (Ast::segs). A prefix of a path is a path (`SegList { lo, len: n }`).
/// The top bit of `n` is the path's leading `::` (Path::global), the next one marks a qualified
/// path `<T as Trait>::..`: its QSelf id is the `args` of the placeholder segment stored just
/// before the first segment (Ast::qself).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SegList {
    pub lo: u32,
    n: u32,
}

const GLOBAL_BIT: u32 = 1 << 31;
const QSELF_BIT: u32 = 1 << 30;
const LEN_MASK: u32 = !(GLOBAL_BIT | QSELF_BIT);

impl SegList {
    #[inline]
    pub fn new(lo: u32, len: u32, global: bool) -> SegList {
        SegList { lo, n: len | if global { GLOBAL_BIT } else { 0 } }
    }
    #[inline]
    pub fn len(&self) -> usize {
        (self.n & LEN_MASK) as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub fn global(&self) -> bool {
        self.n & GLOBAL_BIT != 0
    }
    /// The path is `<T as Trait>::..` (its segment list is preceded by the QSelf placeholder).
    #[inline]
    pub fn has_qself(&self) -> bool {
        self.n & QSELF_BIT != 0
    }
    /// This list marked as qualified (a tree being built: the placeholder precedes `lo`).
    #[inline]
    pub fn with_qself(self) -> SegList {
        SegList { lo: self.lo, n: self.n | QSELF_BIT }
    }
    /// The same segments as a plain path (no `<T as Trait>` part).
    #[inline]
    pub fn without_qself(self) -> SegList {
        SegList { lo: self.lo, n: self.n & !QSELF_BIT }
    }
    /// The first `n` segments (a qualified path's prefix is qualified).
    #[inline]
    pub fn prefix(&self, n: usize) -> SegList {
        let len = (n as u32).min(self.len() as u32);
        SegList { lo: self.lo, n: len | (self.n & !LEN_MASK) }
    }
}

/// An optional expression in 4 bytes (u32::MAX: none) -- keeps ExprKind at 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptExprId(pub u32);

impl OptExprId {
    pub const NONE: OptExprId = OptExprId(u32::MAX);
    #[inline]
    pub fn get(self) -> Option<ExprId> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(ExprId(self.0))
        }
    }
    #[inline]
    pub fn from(e: Option<ExprId>) -> OptExprId {
        e.map_or(OptExprId::NONE, |e| OptExprId(e.0))
    }
    #[inline]
    pub fn is_some(self) -> bool {
        self.0 != u32::MAX
    }
    #[inline]
    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(ExprId) -> U) -> Option<U> {
        self.get().map(f)
    }
    #[inline]
    pub fn map_or<U>(self, d: U, f: impl FnOnce(ExprId) -> U) -> U {
        self.get().map_or(d, f)
    }
}

/// An optional type in 4 bytes (u32::MAX: none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptTyId(pub u32);

impl OptTyId {
    pub const NONE: OptTyId = OptTyId(u32::MAX);
    #[inline]
    pub fn get(self) -> Option<TyId> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(TyId(self.0))
        }
    }
    #[inline]
    pub fn from(t: Option<TyId>) -> OptTyId {
        t.map_or(OptTyId::NONE, |t| OptTyId(t.0))
    }
    #[inline]
    pub fn is_some(self) -> bool {
        self.0 != u32::MAX
    }
    #[inline]
    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(TyId) -> U) -> Option<U> {
        self.get().map(f)
    }
}

/// An optional name (a label) in 8 bytes (lo == u32::MAX: none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptIdent(Ident);

impl OptIdent {
    pub const NONE: OptIdent = OptIdent(Ident { lo: u32::MAX, hi: u32::MAX });
    #[inline]
    pub fn get(self) -> Option<Ident> {
        if self.0.lo == u32::MAX {
            None
        } else {
            Some(self.0)
        }
    }
    #[inline]
    pub fn from(i: Option<Ident>) -> OptIdent {
        i.map_or(OptIdent::NONE, OptIdent)
    }
    #[inline]
    pub fn is_none(self) -> bool {
        self.0.lo == u32::MAX
    }
    #[inline]
    pub fn is_some(self) -> bool {
        self.0.lo != u32::MAX
    }
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(Ident) -> U) -> Option<U> {
        self.get().map(f)
    }
}

/// A path's `<T as Trait>` part: an index into its tree's Ast::qselfs (u32::MAX: none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QSelfId(pub u32);

impl QSelfId {
    pub const NONE: QSelfId = QSelfId(u32::MAX);
    #[inline]
    pub fn is_some(&self) -> bool {
        self.0 != u32::MAX
    }
    #[inline]
    pub fn is_none(&self) -> bool {
        self.0 == u32::MAX
    }
}

/// 12 bytes: segments (with the leading-`::` and qualified-self bits), the position.
#[derive(Debug, Clone, Copy)]
pub struct Path {
    pub segs: SegList,
    pub lo: u32,
}

impl Path {
    /// no `<T as Trait>` part
    #[inline]
    pub fn no_qself(&self) -> bool {
        !self.segs.has_qself()
    }
    /// `<T as Trait>::..` (Ast::qself has the part)
    #[inline]
    pub fn has_qself(&self) -> bool {
        self.segs.has_qself()
    }
    /// `::a::b`
    #[inline]
    pub fn global(&self) -> bool {
        self.segs.global()
    }
}

const _: () = assert!(std::mem::size_of::<Path>() == 12);

#[derive(Debug, Clone, Copy)]
pub struct QSelf {
    pub ty: TyId,
    /// `<T as Trait>`: the trait path; number of segments of `segs` belonging to it
    pub trait_path: Option<Path>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SegKind {
    Ident,
    SelfValue,
    SelfType,
    Super,
    Crate,
}

/// A segment's or method call's generic arguments: an index into its tree's
/// Ast::generic_args (u32::MAX: none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericArgsId(pub u32);

impl GenericArgsId {
    pub const NONE: GenericArgsId = GenericArgsId(u32::MAX);
    #[inline]
    pub fn is_some(&self) -> bool {
        self.0 != u32::MAX
    }
    #[inline]
    pub fn is_none(&self) -> bool {
        self.0 == u32::MAX
    }
}

/// A fn pointer type's signature: an index into its tree's Ast::fn_ptrs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnPtrId(pub u32);

/// An item function signature: an index into its tree's Ast::fn_sigs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnSigId(pub u32);

/// A use-tree path's child: an index into its tree's Ast::use_trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UseTreeId(pub u32);

/// One path segment in 12 bytes: the name and, in one word, the segment kind (top 3 bits) and
/// the generic arguments id (low 29 bits, all ones: none). A qualified path's placeholder
/// segment (SegList::has_qself) keeps its QSelf id in the id bits.
#[derive(Clone, Copy)]
pub struct PathSeg {
    pub name: Ident,
    kind_args: u32,
}

const SEG_ID_BITS: u32 = 29;
const SEG_ID_NONE: u32 = (1 << SEG_ID_BITS) - 1;

impl PathSeg {
    #[inline]
    pub fn new(kind: SegKind, name: Ident, args: GenericArgsId) -> PathSeg {
        let id = if args.0 == u32::MAX { SEG_ID_NONE } else { args.0 };
        assert!(id < SEG_ID_NONE || args.0 == u32::MAX, "path segment: generic args id out of range");
        PathSeg { name, kind_args: (kind as u32) << SEG_ID_BITS | id }
    }
    #[inline]
    pub fn kind(&self) -> SegKind {
        match self.kind_args >> SEG_ID_BITS {
            0 => SegKind::Ident,
            1 => SegKind::SelfValue,
            2 => SegKind::SelfType,
            3 => SegKind::Super,
            _ => SegKind::Crate,
        }
    }
    #[inline]
    pub fn args(&self) -> GenericArgsId {
        let id = self.kind_args & SEG_ID_NONE;
        GenericArgsId(if id == SEG_ID_NONE { u32::MAX } else { id })
    }
    #[inline]
    pub fn set_args(&mut self, args: GenericArgsId) {
        *self = PathSeg::new(self.kind(), self.name, args);
    }
}

impl std::fmt::Debug for PathSeg {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("PathSeg").field("kind", &self.kind()).field("name", &self.name).field("args", &self.args()).finish()
    }
}

const _: () = assert!(std::mem::size_of::<PathSeg>() == 12);

#[derive(Debug, Clone, Copy)]
pub enum GenericArgs {
    Angle(List<GenericArg>),
    /// `Fn(A, B) -> C`
    Paren(List<TyId>, Option<TyId>),
}

#[derive(Debug, Clone, Copy)]
pub enum GenericArg {
    Lifetime(Ident),
    Type(TyId),
    Const(ExprId),
    /// `Item = T`
    Binding(Ident, GenericArgsId, TyId),
    /// `Item: Bound`
    Constraint(Ident, GenericArgsId, List<Bound>),
}

#[derive(Debug, Clone, Copy)]
pub enum Bound {
    Lifetime(Ident),
    Trait {
        hrtb: List<GenericParam>,
        maybe: bool,
        konst: bool,
        /// `async Fn()` (async_trait_bounds)
        async_: bool,
        path: Path,
    },
    /// `use<'a, T>` precise capture
    Use(GenericArgsId),
}

#[derive(Debug, Clone, Copy)]
pub struct GenericParam {
    pub attrs: List<Attr>,
    pub name: Ident,
    pub kind: GenericParamKind,
}

#[derive(Debug, Clone, Copy)]
pub enum GenericParamKind {
    Lifetime(List<Ident>),
    Type(List<Bound>, Option<TyId>),
    Const(TyId, Option<ExprId>),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Generics {
    pub params: List<GenericParam>,
    pub where_: List<WherePred>,
}

#[derive(Debug, Clone, Copy)]
pub enum WherePred {
    Bound {
        hrtb: List<GenericParam>,
        ty: TyId,
        bounds: List<Bound>,
    },
    Lifetime(Ident, List<Ident>),
}

// ---------------------------------------------------------------- types

#[derive(Debug, Clone, Copy)]
pub enum Ty {
    Path(Path),
    Ref(Option<Ident>, bool, TyId),
    Ptr(bool, TyId),
    Slice(TyId),
    Array(TyId, ExprId),
    Tuple(List<TyId>),
    Fn(FnPtrId),
    Never,
    Infer,
    ImplTrait(List<Bound>),
    DynTrait(List<Bound>, bool),
    Paren(TyId),
    Mac(Row<MacCall>),
    /// `Self` inside impls is a path; this is the implicit `self` type of `&self`
    ImplicitSelf,
    CVarArgs,
}

// Ty was 36 bytes, Pat 36: their macro calls (32 bytes inline) are rows.
const _: () = assert!(std::mem::size_of::<Ty>() == 20);
const _: () = assert!(std::mem::size_of::<Pat>() == 24);

#[derive(Debug, Clone, Copy)]
pub struct FnPtr {
    pub hrtb: List<GenericParam>,
    pub unsafe_: bool,
    /// `extern` / `extern "abi"`: Some(abi string, if written)
    pub abi: Option<Option<Ident>>,
    pub params: List<TyId>,
    pub ret: Option<TyId>,
    pub variadic: bool,
}

// ---------------------------------------------------------------- patterns

#[derive(Debug, Clone, Copy)]
pub enum Pat {
    Wild,
    Rest,
    Ident {
        by_ref: bool,
        mutbl: bool,
        name: Ident,
        sub: Option<PatId>,
    },
    Lit(ExprId),
    Range(Option<ExprId>, Option<ExprId>, bool),
    Path(Path),
    TupleStruct(Path, List<PatId>),
    Struct(Path, List<FieldPat>, bool),
    Tuple(List<PatId>),
    Slice(List<PatId>),
    Ref(bool, PatId),
    Or(List<PatId>),
    Paren(PatId),
    Box(PatId),
    Mac(Row<MacCall>),
}

#[derive(Debug, Clone, Copy)]
pub struct FieldPat {
    pub attrs: List<Attr>,
    pub name: Ident,
    /// None: shorthand `x` / `ref mut x`
    pub pat: PatId,
    pub shorthand: bool,
}

// ---------------------------------------------------------------- expressions

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    BitXor,
    BitAnd,
    BitOr,
    Shl,
    Shr,
    Eq,
    Lt,
    Le,
    Ne,
    Ge,
    Gt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LitKind {
    Int,
    Float,
    Str,
    ByteStr,
    CStr,
    RawStr,
    Char,
    Byte,
    Bool(bool),
}

#[derive(Debug, Clone, Copy)]
pub struct MacCall {
    pub path: Path,
    pub delim: u8,
    /// tokens between the delimiters
    pub toks: TokRange,
    /// index into `Ast::mac_args` for built-in macros whose arguments were parsed
    pub args: u32,
}

pub const NO_MAC_ARGS: u32 = u32::MAX;

/// Parsed arguments of a built-in macro (format family, assert*, vec!, matches!).
#[derive(Debug, Clone, Copy)]
pub struct MacArgs {
    pub exprs: List<ExprId>,
    /// `vec![x; n]`
    pub repeat: bool,
    /// `matches!(e, pat if guard)`
    pub pat: Option<PatId>,
    pub guard: Option<ExprId>,
    /// `asm!`: the operands and options; the template strings are `exprs[..asm.templates]`
    pub asm: Option<AsmArgs>,
}

/// Operands of an `asm!` (Reference inline-assembly.md [asm.syntax]).
#[derive(Debug, Clone, Copy)]
pub struct AsmArgs {
    /// how many leading `exprs` are template string literals
    pub templates: u32,
    pub ops: List<AsmOp>,
    /// ASM_* option bits
    pub options: u32,
}

pub const ASM_NORETURN: u32 = 1;
pub const ASM_RAW: u32 = 2;
pub const ASM_ATT_SYNTAX: u32 = 4;
pub const ASM_NOMEM: u32 = 8;
pub const ASM_READONLY: u32 = 16;
pub const ASM_PURE: u32 = 32;
pub const ASM_PRESERVES_FLAGS: u32 = 64;
pub const ASM_NOSTACK: u32 = 128;
pub const ASM_MAY_UNWIND: u32 = 256;
/// `clobber_abi("C")` / `"system"` / `"efiapi"` (the target's C convention)
pub const ASM_CLOBBER_C: u32 = 1 << 16;
/// `clobber_abi("sysv64")`
pub const ASM_CLOBBER_SYSV64: u32 = 1 << 17;
/// `clobber_abi("win64")`
pub const ASM_CLOBBER_WIN64: u32 = 1 << 18;
/// an ABI `clobber_abi` names that is none of these (an error when the block is checked)
pub const ASM_CLOBBER_UNKNOWN: u32 = 1 << 19;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmDir {
    In,
    Out,
    LateOut,
    InOut,
    InLateOut,
    Const,
    Sym,
}

#[derive(Debug, Clone, Copy)]
pub struct AsmOp {
    /// `name = ...`
    pub name: Option<Ident>,
    pub dir: AsmDir,
    /// the register class (`reg`) or, when `explicit`, the register's string literal token
    pub reg: Ident,
    pub explicit: bool,
    /// in / inout input / const value / sym path expression
    pub input: Option<ExprId>,
    /// out / inout output place; None for `_`
    pub output: Option<ExprId>,
}

#[derive(Debug, Clone, Copy)]
pub struct Arm {
    pub attrs: List<Attr>,
    pub pat: PatId,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldExpr {
    pub attrs: List<Attr>,
    pub name: Ident,
    pub expr: ExprId,
    pub shorthand: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ClosureParam {
    pub attrs: List<Attr>,
    pub pat: PatId,
    pub ty: Option<TyId>,
}

/// `recv.name::<turbofish>(args)` without the receiver.
#[derive(Debug, Clone, Copy)]
pub struct MethodCallRow {
    pub name: Ident,
    pub turbofish: GenericArgsId,
    pub args: List<ExprId>,
}

/// `'label: while cond body`
#[derive(Debug, Clone, Copy)]
pub struct WhileRow {
    pub cond: ExprId,
    pub body: BlockId,
    pub label: Option<Ident>,
}

/// `'label: for pat in iter body`
#[derive(Debug, Clone, Copy)]
pub struct ForRow {
    pub pat: PatId,
    pub iter: ExprId,
    pub body: BlockId,
    pub label: Option<Ident>,
}

#[derive(Debug, Clone, Copy)]
pub struct ClosureRow {
    pub is_move: bool,
    pub is_async: bool,
    /// `static || ..` (coroutine syntax)
    pub is_static: bool,
    pub params: List<ClosureParam>,
    pub ret: Option<TyId>,
    pub body: ExprId,
}

/// `S { fields, ..base }`; `rest`: a bare `..` (rustc default_field_values: the omitted fields
/// take their declared defaults)
#[derive(Debug, Clone, Copy)]
pub struct StructRow {
    pub path: Path,
    pub fields: List<FieldExpr>,
    pub base: OptExprId,
    pub rest: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum ExprKind {
    Lit(LitKind),
    Path(Path),
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Assign(ExprId, ExprId),
    AssignOp(BinOp, ExprId, ExprId),
    Call(ExprId, List<ExprId>),
    /// the receiver and the call's row (name, turbofish, arguments)
    MethodCall(ExprId, Row<MethodCallRow>),
    Field(ExprId, Ident),
    TupleField(ExprId, u32),
    Index(ExprId, ExprId),
    AddrOf(bool, bool, ExprId), // (raw, mut, expr)
    Cast(ExprId, TyId),
    Block(BlockId, OptIdent),
    Unsafe(BlockId),
    /// `async { }` / `async move { }` / an async closure's body (ASYNC_*)
    Async(u8, BlockId),
    ConstBlock(BlockId),
    If(ExprId, BlockId, OptExprId),
    Let(PatId, ExprId),
    Match(ExprId, List<Arm>),
    While(Row<WhileRow>),
    Loop(BlockId, OptIdent),
    For(Row<ForRow>),
    Break(OptIdent, OptExprId),
    Continue(OptIdent),
    Return(OptExprId),
    Closure(Row<ClosureRow>),
    Tuple(List<ExprId>),
    Array(List<ExprId>),
    Repeat(ExprId, ExprId),
    Struct(Row<StructRow>),
    Range(OptExprId, OptExprId, bool),
    Try(ExprId),
    Await(ExprId),
    Mac(Row<MacCall>),
    Paren(ExprId),
    Underscore,
    Yield(OptExprId),
    Box(ExprId),
}

#[derive(Debug, Clone, Copy)]
pub struct Expr {
    pub kind: ExprKind,
    pub lo: u32,
    pub hi: u32,
}

// Expr used to be 44 bytes (ExprKind 36); the big, rare payloads live in rows, labels and optional ids are 8 / 4 bytes.
const _: () = assert!(std::mem::size_of::<ExprKind>() == 16);
const _: () = assert!(std::mem::size_of::<Expr>() == 24);

/// `let pat: ty = init else { else_ };`
#[derive(Debug, Clone, Copy)]
pub struct LetStmt {
    pub attrs: List<Attr>,
    pub pat: PatId,
    pub ty: Option<TyId>,
    pub init: Option<ExprId>,
    pub else_: Option<BlockId>,
}

/// An expression statement with outer attributes (`Stmt::Attrs`).
#[derive(Debug, Clone, Copy)]
pub struct AttrsStmt {
    pub attrs: List<Attr>,
    pub expr: ExprId,
    pub semi: bool,
}

/// 8 bytes: a `let` and an attributed expression statement are rows (Stmt used to be 36).
#[derive(Debug, Clone, Copy)]
pub enum Stmt {
    Let(Row<LetStmt>),
    Item(ItemId),
    Expr(ExprId, bool),
    /// an expression statement (`Stmt::Expr(expr, semi)`) with outer attributes; program
    /// loading strips it by its `#[cfg]` (keeping the statement or dropping it), so later
    /// passes never see it
    Attrs(Row<AttrsStmt>),
    Empty,
}

const _: () = assert!(std::mem::size_of::<Stmt>() == 8);

#[derive(Debug, Clone, Copy)]
pub struct Block {
    pub attrs: List<Attr>,
    pub stmts: List<Stmt>,
    pub lo: u32,
    pub hi: u32,
}

// ---------------------------------------------------------------- items

#[derive(Debug, Clone, Copy)]
pub struct Item {
    /// The specialization keyword, retained until collection rejects E0658.
    pub default_: bool,
    /// arena ranges [expr_lo, expr_hi, pat_lo, pat_hi) covering everything parsed for this item
    pub ranges: [u32; 4],
    pub attrs: List<Attr>,
    pub vis: Vis,
    pub name: Ident,
    pub kind: ItemKind,
    pub lo: u32,
    pub hi: u32,
}

// Item used to be 168 bytes, before its owned children moved into flat arrays.
const _: () = assert!(std::mem::size_of::<Item>() <= 68);

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub attrs: List<Attr>,
    pub pat: PatId,
    pub ty: TyId,
}

#[derive(Debug, Clone, Copy)]
pub enum SelfParam {
    Value(bool),                // self / mut self
    Ref(Option<Ident>, bool),   // &'a mut self
    Typed(bool, TyId),          // mut self: Box<Self>
}

#[derive(Debug, Clone, Copy)]
pub struct FnSig {
    pub konst: bool,
    pub asyncness: bool,
    pub unsafe_: bool,
    pub abi: Option<Option<Ident>>, // extern / extern "C"
    pub generics: Generics,
    pub self_param: Option<SelfParam>,
    pub params: List<Param>,
    pub variadic: bool,
    pub ret: Option<TyId>,
    /// lazy-body parse mode: the unparsed body's token range (inside the braces)
    pub lazy_body: Option<TokRange>,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldDef {
    pub attrs: List<Attr>,
    pub vis: Vis,
    pub name: Option<Ident>,
    pub ty: TyId,
    pub default: Option<ExprId>,
    /// source range of the field's type (the default's anonymous const is written with it)
    pub ty_span: (u32, u32),
}

#[derive(Debug, Clone, Copy)]
pub enum VariantData {
    Unit,
    Tuple(List<FieldDef>),
    Struct(List<FieldDef>),
}

#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub attrs: List<Attr>,
    pub name: Ident,
    pub data: VariantData,
    pub disc: Option<ExprId>,
}

#[derive(Debug, Clone, Copy)]
pub enum UseTree {
    Path(PathSeg, UseTreeId),
    Name(PathSeg, Option<Ident>), // rename; `as _` -> Some(ident "_")
    Glob,
    Group(List<UseTree>),
}

/// `use tree;` (global: a leading `::`)
#[derive(Debug, Clone, Copy)]
pub struct UseItem {
    pub global: bool,
    pub tree: UseTree,
}

#[derive(Debug, Clone, Copy)]
pub struct StructItem {
    pub generics: Generics,
    pub data: VariantData,
}

#[derive(Debug, Clone, Copy)]
pub struct UnionItem {
    pub generics: Generics,
    pub fields: List<FieldDef>,
}

#[derive(Debug, Clone, Copy)]
pub struct EnumItem {
    pub generics: Generics,
    pub variants: List<Variant>,
}

#[derive(Debug, Clone, Copy)]
pub struct TypeAliasItem {
    pub generics: Generics,
    pub bounds: List<Bound>,
    pub ty: Option<TyId>,
}

#[derive(Debug, Clone, Copy)]
pub struct TraitItem {
    pub unsafe_: bool,
    pub auto: bool,
    pub generics: Generics,
    pub supers: List<Bound>,
    pub items: List<ItemId>,
}

#[derive(Debug, Clone, Copy)]
pub struct TraitAliasItem {
    pub generics: Generics,
    pub bounds: List<Bound>,
}

#[derive(Debug, Clone, Copy)]
pub struct ImplItem {
    pub unsafe_: bool,
    pub default_: bool,
    pub generics: Generics,
    pub negative: bool,
    pub trait_: Option<Path>,
    pub self_ty: TyId,
    pub items: List<ItemId>,
}

#[derive(Debug, Clone, Copy)]
pub struct ForeignModItem {
    pub abi: Option<Ident>,
    pub items: List<ItemId>,
    /// `unsafe extern { .. }` (a u32 keeps the row free of padding: it is a Pod image row)
    pub unsafe_: u32,
}

/// An item's kind in 16 bytes: the kinds with generics or other big payloads keep them in a row
/// of their own array (an Item used to be 116 bytes, its ItemKind 56).
#[derive(Debug, Clone, Copy)]
pub enum ItemKind {
    Use(Row<UseItem>),
    ExternCrate(Option<Ident>),
    Fn(FnSigId, Option<BlockId>, bool), // bool: `default fn`
    Struct(Row<StructItem>),
    Union(Row<UnionItem>),
    Enum(Row<EnumItem>),
    TypeAlias(Row<TypeAliasItem>),
    Const(OptTyId, OptExprId),
    Static(bool, TyId, OptExprId),
    Trait(Row<TraitItem>),
    TraitAlias(Row<TraitAliasItem>),
    Impl(Row<ImplItem>),
    Mod(Option<List<ItemId>>),
    ForeignMod(Row<ForeignModItem>),
    MacroRules(TokRange),
    Mac(Row<MacCall>),
}

const _: () = assert!(std::mem::size_of::<ItemKind>() == 16);

/// Number of id ranges a tree has (Ast::base / Ast::ends).
pub const ARENAS: usize = 49;

/// Ast::base (an array of more than 32 entries has no Default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bases(pub [u32; ARENAS]);

impl Default for Bases {
    fn default() -> Self {
        Bases([0; ARENAS])
    }
}
impl std::ops::Deref for Bases {
    type Target = [u32; ARENAS];
    #[inline]
    fn deref(&self) -> &[u32; ARENAS] {
        &self.0
    }
}
impl std::ops::DerefMut for Bases {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u32; ARENAS] {
        &mut self.0
    }
}
impl From<[u32; ARENAS]> for Bases {
    fn from(b: [u32; ARENAS]) -> Bases {
        Bases(b)
    }
}

#[derive(Debug, Default)]
pub struct Ast {
    pub exprs: crate::arena::Arena<Expr>,
    pub pats: crate::arena::Arena<Pat>,
    pub tys: crate::arena::Arena<Ty>,
    pub items: crate::arena::Arena<Item>,
    pub blocks: crate::arena::Arena<Block>,
    pub root_attrs: crate::arena::Arena<Attr>,
    pub root_items: crate::arena::Arena<ItemId>,
    pub mac_args: crate::arena::Arena<MacArgs>,
    /// every path's segments (Path::segs indexes it)
    pub path_segs: crate::arena::Arena<PathSeg>,
    /// every block's statements (Block::stmts)
    pub stmts: crate::arena::Arena<Stmt>,
    /// child lists of nodes (call / tuple / array elements, patterns, types, struct literal
    /// fields, match arms, closure params, struct pattern fields)
    pub expr_lists: crate::arena::Arena<ExprId>,
    pub pat_lists: crate::arena::Arena<PatId>,
    pub ty_lists: crate::arena::Arena<TyId>,
    pub field_exprs: crate::arena::Arena<FieldExpr>,
    pub arms: crate::arena::Arena<Arm>,
    pub closure_params: crate::arena::Arena<ClosureParam>,
    pub field_pats: crate::arena::Arena<FieldPat>,
    /// the `<T as Trait>` parts of paths (Ast::qself: the id sits in the placeholder segment before the path's segments)
    pub qselfs: crate::arena::Arena<QSelf>,
    /// generic arguments of path segments and method calls (PathSeg::args, GenericArgsId)
    pub generic_args: crate::arena::Arena<GenericArgs>,
    /// fn pointer types' signatures (Ty::Fn)
    pub fn_ptrs: crate::arena::Arena<FnPtr>,
    /// lists: generic arguments, bounds, generic params (hrtb and item generics' bounds),
    /// lifetimes (lifetime params' bounds), attributes of body-level nodes and generic params,
    /// asm! operands
    pub generic_arg_lists: crate::arena::Arena<GenericArg>,
    pub bounds: crate::arena::Arena<Bound>,
    pub generic_params: crate::arena::Arena<GenericParam>,
    pub ident_lists: crate::arena::Arena<Ident>,
    pub attrs: crate::arena::Arena<Attr>,
    pub asm_ops: crate::arena::Arena<AsmOp>,
    /// Item signatures and children (all position-independent, including use-tree edges).
    pub fn_sigs: crate::arena::Arena<FnSig>,
    pub params: crate::arena::Arena<Param>,
    pub field_defs: crate::arena::Arena<FieldDef>,
    pub variants: crate::arena::Arena<Variant>,
    pub use_trees: crate::arena::Arena<UseTree>,
    pub item_lists: crate::arena::Arena<ItemId>,
    pub where_preds: crate::arena::Arena<WherePred>,
    /// the big, rare payloads of expression nodes (ExprKind::MethodCall / For / Closure / Struct
    /// / Mac): Row<T> indexes them
    pub method_calls: crate::arena::Arena<MethodCallRow>,
    pub for_exprs: crate::arena::Arena<ForRow>,
    pub closure_exprs: crate::arena::Arena<ClosureRow>,
    pub struct_exprs: crate::arena::Arena<StructRow>,
    pub mac_calls: crate::arena::Arena<MacCall>,
    pub while_exprs: crate::arena::Arena<WhileRow>,
    pub let_stmts: crate::arena::Arena<LetStmt>,
    pub attrs_stmts: crate::arena::Arena<AttrsStmt>,
    /// the big payloads of item kinds (ItemKind::Struct / Impl / ..) and `pub(in path)` paths
    pub use_items: crate::arena::Arena<UseItem>,
    pub struct_items: crate::arena::Arena<StructItem>,
    pub union_items: crate::arena::Arena<UnionItem>,
    pub enum_items: crate::arena::Arena<EnumItem>,
    pub alias_items: crate::arena::Arena<TypeAliasItem>,
    pub trait_items: crate::arena::Arena<TraitItem>,
    pub trait_alias_items: crate::arena::Arena<TraitAliasItem>,
    pub impl_items: crate::arena::Arena<ImplItem>,
    pub foreign_mods: crate::arena::Arena<ForeignModItem>,
    pub vis_paths: crate::arena::Arena<Path>,
    /// lint-level attributes (`#[allow(..)]` ..) of expression statements: cfg stripping drops the
    /// statement wrapper, this keeps the attributes for the lint passes (expression -> attributes)
    pub stmt_attrs: Vec<(ExprId, List<Attr>)>,
    /// syntax the parser met that a feature gates, wherever it is written (cfg'd-out code and
    /// macro arguments included; rustc ParseSess::gated_spans): (feature, position)
    pub gated_syntax: Vec<(&'static str, u32)>,
    /// first id of each arena (in `ends` order): 0 for a file's tree; a file's lazily parsed
    /// fn bodies (`part`) continue its ids
    pub base: Bases,
    /// the fn bodies the file's parse skipped (FnSig::lazy_body), parsed on the first need of
    /// any of them (parser::ParsedFile::part): their nodes, ids after this tree's
    pub part: std::sync::OnceLock<Box<Ast>>,
}

impl Ast {
    #[inline]
    pub fn expr(&self, id: ExprId) -> &Expr {
        match self.exprs.get(id.0.wrapping_sub(self.base[0]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("expr", id.0).expr(id),
        }
    }
    #[inline]
    pub fn pat(&self, id: PatId) -> &Pat {
        match self.pats.get(id.0.wrapping_sub(self.base[1]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("pat", id.0).pat(id),
        }
    }
    #[inline]
    pub fn ty(&self, id: TyId) -> &Ty {
        match self.tys.get(id.0.wrapping_sub(self.base[2]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("ty", id.0).ty(id),
        }
    }
    #[inline]
    pub fn item(&self, id: ItemId) -> &Item {
        match self.items.get(id.0.wrapping_sub(self.base[23]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("item", id.0).item(id),
        }
    }
    #[inline]
    pub fn block(&self, id: BlockId) -> &Block {
        match self.blocks.get(id.0.wrapping_sub(self.base[3]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("block", id.0).block(id),
        }
    }
    /// A built-in macro's parsed arguments (MacCall::args).
    #[inline]
    pub fn mac_arg(&self, i: u32) -> &MacArgs {
        match self.mac_args.get(i.wrapping_sub(self.base[4]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("mac_args", i).mac_arg(i),
        }
    }
    /// A path's segments (in this tree or its lazily parsed part).
    #[inline]
    pub fn segs(&self, l: SegList) -> &[PathSeg] {
        if l.is_empty() {
            return &[];
        }
        let i = l.lo.wrapping_sub(self.base[5]) as usize;
        match self.path_segs.get(i..i + l.len()) {
            Some(x) => x,
            None => self.part_or_panic("path segments", l.lo).segs(l),
        }
    }
    /// A path's `<T as Trait>` part (in this tree or its lazily parsed part).
    #[inline]
    pub fn qself(&self, p: &Path) -> Option<&QSelf> {
        if p.no_qself() {
            return None;
        }
        let q = self.segs(SegList::new(p.segs.lo - 1, 1, false))[0].args().0;
        match self.qselfs.get(q.wrapping_sub(self.base[14]) as usize) {
            Some(q) => Some(q),
            None => self.part_or_panic("qself", q).qself(p),
        }
    }
    /// A segment's or method call's generic arguments (in this tree or its lazily parsed part).
    #[inline]
    pub fn gargs(&self, id: GenericArgsId) -> Option<&GenericArgs> {
        if id.is_none() {
            return None;
        }
        match self.generic_args.get(id.0.wrapping_sub(self.base[15]) as usize) {
            Some(g) => Some(g),
            None => self.part_or_panic("generic args", id.0).gargs(id),
        }
    }
    /// A fn pointer type's signature (in this tree or its lazily parsed part).
    #[inline]
    pub fn fn_ptr(&self, id: FnPtrId) -> &FnPtr {
        match self.fn_ptrs.get(id.0.wrapping_sub(self.base[16]) as usize) {
            Some(f) => f,
            None => self.part_or_panic("fn ptr", id.0).fn_ptr(id),
        }
    }
    /// An item function signature (in this tree or its lazily parsed part).
    #[inline]
    pub fn fn_sig(&self, id: FnSigId) -> &FnSig {
        match self.fn_sigs.get(id.0.wrapping_sub(self.base[24]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("fn_sigs", id.0).fn_sig(id),
        }
    }
    /// Stores an item function signature (a tree being built).
    pub fn push_fn_sig(&mut self, x: FnSig) -> FnSigId {
        let id = FnSigId(self.base[24] + self.fn_sigs.len() as u32);
        self.fn_sigs.push(x);
        id
    }
    /// A use-tree path child (in this tree or its lazily parsed part).
    #[inline]
    pub fn use_tree(&self, id: UseTreeId) -> &UseTree {
        match self.use_trees.get(id.0.wrapping_sub(self.base[28]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("use_trees", id.0).use_tree(id),
        }
    }
    /// Stores a use-tree path child (a tree being built).
    pub fn push_use_tree(&mut self, x: UseTree) -> UseTreeId {
        let id = UseTreeId(self.base[28] + self.use_trees.len() as u32);
        self.use_trees.push(x);
        id
    }
    /// Stores generic arguments (a tree being built).
    pub fn push_gargs(&mut self, g: GenericArgs) -> GenericArgsId {
        self.generic_args.push(g);
        GenericArgsId(self.base[15] + self.generic_args.len() as u32 - 1)
    }
    /// Stores a fn pointer type's signature (a tree being built).
    pub fn push_fn_ptr(&mut self, f: FnPtr) -> FnPtrId {
        self.fn_ptrs.push(f);
        FnPtrId(self.base[16] + self.fn_ptrs.len() as u32 - 1)
    }
    /// A list's entries (in this tree or its lazily parsed part).
    #[inline]
    pub fn list<T>(&self, l: List<T>) -> &[T]
    where
        Ast: FlatArray<T>,
    {
        if l.len == 0 {
            return &[];
        }
        let i = l.lo.wrapping_sub(self.base[<Ast as FlatArray<T>>::BASE]) as usize;
        match <Ast as FlatArray<T>>::array(self).get(i..i + l.len as usize) {
            Some(x) => x,
            None => self.part_or_panic("list", l.lo).list(l),
        }
    }
    /// The `Use` payload of an item kind.
    #[inline]
    pub fn as_use(&self, k: &ItemKind) -> Option<&UseItem> {
        match k {
            ItemKind::Use(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `Struct` payload of an item kind.
    #[inline]
    pub fn as_struct(&self, k: &ItemKind) -> Option<&StructItem> {
        match k {
            ItemKind::Struct(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `Union` payload of an item kind.
    #[inline]
    pub fn as_union(&self, k: &ItemKind) -> Option<&UnionItem> {
        match k {
            ItemKind::Union(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `Enum` payload of an item kind.
    #[inline]
    pub fn as_enum(&self, k: &ItemKind) -> Option<&EnumItem> {
        match k {
            ItemKind::Enum(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `TypeAlias` payload of an item kind.
    #[inline]
    pub fn as_alias(&self, k: &ItemKind) -> Option<&TypeAliasItem> {
        match k {
            ItemKind::TypeAlias(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `Trait` payload of an item kind.
    #[inline]
    pub fn as_trait(&self, k: &ItemKind) -> Option<&TraitItem> {
        match k {
            ItemKind::Trait(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `TraitAlias` payload of an item kind.
    #[inline]
    pub fn as_trait_alias(&self, k: &ItemKind) -> Option<&TraitAliasItem> {
        match k {
            ItemKind::TraitAlias(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `Impl` payload of an item kind.
    #[inline]
    pub fn as_impl(&self, k: &ItemKind) -> Option<&ImplItem> {
        match k {
            ItemKind::Impl(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The `ForeignMod` payload of an item kind.
    #[inline]
    pub fn as_foreign_mod(&self, k: &ItemKind) -> Option<&ForeignModItem> {
        match k {
            ItemKind::ForeignMod(r) => Some(self.row(*r)),
            _ => None,
        }
    }
    /// The generics of an item kind that has them (a fn's included).
    pub fn kind_generics(&self, k: &ItemKind) -> Option<&Generics> {
        match k {
            ItemKind::Struct(r) => Some(&self.row(*r).generics),
            ItemKind::Union(r) => Some(&self.row(*r).generics),
            ItemKind::Enum(r) => Some(&self.row(*r).generics),
            ItemKind::TypeAlias(r) => Some(&self.row(*r).generics),
            ItemKind::Trait(r) => Some(&self.row(*r).generics),
            ItemKind::TraitAlias(r) => Some(&self.row(*r).generics),
            ItemKind::Impl(r) => Some(&self.row(*r).generics),
            ItemKind::Fn(sig, _, _) => Some(&self.fn_sig(*sig).generics),
            _ => None,
        }
    }
    /// A row (in this tree or its lazily parsed part).
    #[inline]
    pub fn row<T>(&self, r: Row<T>) -> &T
    where
        Ast: FlatArray<T>,
    {
        match <Ast as FlatArray<T>>::array(self).get(r.id.wrapping_sub(self.base[<Ast as FlatArray<T>>::BASE]) as usize) {
            Some(x) => x,
            None => self.part_or_panic("row", r.id).row(r),
        }
    }
    /// A row of a tree being built, to change.
    pub fn row_mut<T>(&mut self, r: Row<T>) -> &mut T
    where
        Ast: FlatArray<T>,
        T: Clone,
    {
        let b = self.base[<Ast as FlatArray<T>>::BASE];
        let a = <Ast as FlatArray<T>>::array_mut(self);
        &mut a[r.id.wrapping_sub(b) as usize]
    }
    /// Stores a row (a tree being built).
    pub fn push_row<T>(&mut self, x: T) -> Row<T>
    where
        Ast: FlatArray<T>,
    {
        let b = self.base[<Ast as FlatArray<T>>::BASE];
        let a = <Ast as FlatArray<T>>::array_mut(self);
        let id = b + a.len() as u32;
        a.push(x);
        Row::new(id)
    }
    /// Stores a list's entries (a tree being built).
    pub fn push_list<T>(&mut self, v: Vec<T>) -> List<T>
    where
        Ast: FlatArray<T>,
    {
        let b = self.base[<Ast as FlatArray<T>>::BASE];
        let a = <Ast as FlatArray<T>>::array_mut(self);
        let lo = b + a.len() as u32;
        let len = v.len() as u32;
        a.extend(v);
        List::new(lo, len)
    }
    /// The expression of an expression statement, with or without outer attributes, and its
    /// trailing `;`.
    #[inline]
    pub fn stmt_expr(&self, s: &Stmt) -> Option<(ExprId, bool)> {
        match s {
            Stmt::Expr(e, semi) => Some((*e, *semi)),
            Stmt::Attrs(r) => {
                let a = self.row(*r);
                Some((a.expr, a.semi))
            }
            _ => None,
        }
    }
    /// The lint-level attributes an expression statement was written with.
    pub fn stmt_attrs(&self, e: ExprId) -> List<Attr> {
        if let Some((_, l)) = self.stmt_attrs.iter().find(|(x, _)| *x == e) { return *l; }
        match self.part.get() {
            Some(p) => p.stmt_attrs(e),
            None => List::EMPTY,
        }
    }
    /// A block's statements.
    #[inline]
    pub fn stmts(&self, b: BlockId) -> &[Stmt] {
        self.list(self.block(b).stmts)
    }
    /// The expression exists (in this tree or its lazily parsed part).
    pub fn has_expr(&self, id: ExprId) -> bool {
        (id.0.wrapping_sub(self.base[0]) as usize) < self.exprs.len() || self.part.get().map_or(false, |p| p.has_expr(id))
    }
    pub fn has_block(&self, id: BlockId) -> bool {
        (id.0.wrapping_sub(self.base[3]) as usize) < self.blocks.len() || self.part.get().map_or(false, |p| p.has_block(id))
    }
    /// Next ids of each arena after this tree (Ast::base order).
    pub fn ends(&self) -> [u32; ARENAS] {
        [
            self.base[0] + self.exprs.len() as u32,
            self.base[1] + self.pats.len() as u32,
            self.base[2] + self.tys.len() as u32,
            self.base[3] + self.blocks.len() as u32,
            self.base[4] + self.mac_args.len() as u32,
            self.base[5] + self.path_segs.len() as u32,
            self.base[6] + self.stmts.len() as u32,
            self.base[7] + self.expr_lists.len() as u32,
            self.base[8] + self.pat_lists.len() as u32,
            self.base[9] + self.ty_lists.len() as u32,
            self.base[10] + self.field_exprs.len() as u32,
            self.base[11] + self.arms.len() as u32,
            self.base[12] + self.closure_params.len() as u32,
            self.base[13] + self.field_pats.len() as u32,
            self.base[14] + self.qselfs.len() as u32,
            self.base[15] + self.generic_args.len() as u32,
            self.base[16] + self.fn_ptrs.len() as u32,
            self.base[17] + self.generic_arg_lists.len() as u32,
            self.base[18] + self.bounds.len() as u32,
            self.base[19] + self.generic_params.len() as u32,
            self.base[20] + self.ident_lists.len() as u32,
            self.base[21] + self.attrs.len() as u32,
            self.base[22] + self.asm_ops.len() as u32,
            self.base[23] + self.items.len() as u32,
            self.base[24] + self.fn_sigs.len() as u32,
            self.base[25] + self.params.len() as u32,
            self.base[26] + self.field_defs.len() as u32,
            self.base[27] + self.variants.len() as u32,
            self.base[28] + self.use_trees.len() as u32,
            self.base[29] + self.item_lists.len() as u32,
            self.base[30] + self.where_preds.len() as u32,
            self.base[31] + self.method_calls.len() as u32,
            self.base[32] + self.for_exprs.len() as u32,
            self.base[33] + self.closure_exprs.len() as u32,
            self.base[34] + self.struct_exprs.len() as u32,
            self.base[35] + self.mac_calls.len() as u32,
            self.base[36] + self.while_exprs.len() as u32,
            self.base[37] + self.let_stmts.len() as u32,
            self.base[38] + self.attrs_stmts.len() as u32,
            self.base[39] + self.use_items.len() as u32,
            self.base[40] + self.struct_items.len() as u32,
            self.base[41] + self.union_items.len() as u32,
            self.base[42] + self.enum_items.len() as u32,
            self.base[43] + self.alias_items.len() as u32,
            self.base[44] + self.trait_items.len() as u32,
            self.base[45] + self.trait_alias_items.len() as u32,
            self.base[46] + self.impl_items.len() as u32,
            self.base[47] + self.foreign_mods.len() as u32,
            self.base[48] + self.vis_paths.len() as u32,
        ]
    }
    #[cold]
    #[inline(never)]
    fn part_or_panic(&self, what: &str, id: u32) -> &Ast {
        match self.part.get() {
            Some(p) => p,
            None => panic!("makepad: {} {} is outside the parsed tree (its lazily parsed body part is missing)", what, id),
        }
    }
    /// Drops the arrays' growth slack (a parsed tree is not appended to again).
    pub fn shrink(&mut self) {
        self.exprs.shrink_to_fit();
        self.pats.shrink_to_fit();
        self.tys.shrink_to_fit();
        self.items.shrink_to_fit();
        self.blocks.shrink_to_fit();
        self.root_attrs.shrink_to_fit();
        self.root_items.shrink_to_fit();
        self.mac_args.shrink_to_fit();
        self.path_segs.shrink_to_fit();
        self.stmts.shrink_to_fit();
        self.expr_lists.shrink_to_fit();
        self.pat_lists.shrink_to_fit();
        self.ty_lists.shrink_to_fit();
        self.field_exprs.shrink_to_fit();
        self.arms.shrink_to_fit();
        self.closure_params.shrink_to_fit();
        self.field_pats.shrink_to_fit();
        self.qselfs.shrink_to_fit();
        self.generic_args.shrink_to_fit();
        self.fn_ptrs.shrink_to_fit();
        self.generic_arg_lists.shrink_to_fit();
        self.bounds.shrink_to_fit();
        self.generic_params.shrink_to_fit();
        self.ident_lists.shrink_to_fit();
        self.attrs.shrink_to_fit();
        self.asm_ops.shrink_to_fit();
        self.fn_sigs.shrink_to_fit();
        self.params.shrink_to_fit();
        self.field_defs.shrink_to_fit();
        self.variants.shrink_to_fit();
        self.use_trees.shrink_to_fit();
        self.item_lists.shrink_to_fit();
        self.where_preds.shrink_to_fit();
        self.method_calls.shrink_to_fit();
        self.for_exprs.shrink_to_fit();
        self.closure_exprs.shrink_to_fit();
        self.struct_exprs.shrink_to_fit();
        self.mac_calls.shrink_to_fit();
        self.while_exprs.shrink_to_fit();
        self.let_stmts.shrink_to_fit();
        self.attrs_stmts.shrink_to_fit();
        self.use_items.shrink_to_fit();
        self.struct_items.shrink_to_fit();
        self.union_items.shrink_to_fit();
        self.enum_items.shrink_to_fit();
        self.alias_items.shrink_to_fit();
        self.trait_items.shrink_to_fit();
        self.trait_alias_items.shrink_to_fit();
        self.impl_items.shrink_to_fit();
        self.foreign_mods.shrink_to_fit();
        self.vis_paths.shrink_to_fit();
    }
    /// Mutable block (cfg stripping of a tree being built).
    pub fn block_mut(&mut self, id: BlockId) -> &mut Block {
        let b = self.base[3];
        &mut self.blocks[(id.0 - b) as usize]
    }
}

/// A field name written as a tuple index (`S { 0: a }`, `x.1`): plain decimal digits without a
/// leading zero (rustc: `0x1`, `0b1`, `01` and suffixed `0u32` name no field).
pub fn tuple_index(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) || (b.len() > 1 && b[0] == b'0') {
        return None;
    }
    text.parse::<u32>().ok()
}
