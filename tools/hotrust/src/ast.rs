//! Syntax tree. One `Ast` per file; nodes live in per-kind arenas and refer to
//! each other by index. Names are source spans (`Ident`), resolved later.

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Ident {
    pub lo: u32,
    pub hi: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ExprId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PatId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TyId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ItemId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BlockId(pub u32);

/// A token range `[lo, hi)` into the file's token list (macro bodies, attribute args).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct TokRange {
    pub lo: u32,
    pub hi: u32,
}

#[derive(Clone)]
pub struct Attr {
    pub inner: bool,
    /// tokens inside `#[ ... ]`
    pub toks: TokRange,
}

#[derive(Clone)]
pub enum Vis {
    Private,
    Pub,
    Crate,
    Super,
    SelfMod,
    In(Path),
}

#[derive(Clone)]
pub struct Path {
    pub global: bool,
    pub qself: Option<Box<QSelf>>,
    pub segs: Vec<PathSeg>,
    pub lo: u32,
}

#[derive(Clone)]
pub struct QSelf {
    pub ty: TyId,
    /// `<T as Trait>`: the trait path; number of segments of `segs` belonging to it
    pub trait_path: Option<Path>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    Ident,
    SelfValue,
    SelfType,
    Super,
    Crate,
}

#[derive(Clone)]
pub struct PathSeg {
    pub kind: SegKind,
    pub name: Ident,
    pub args: Option<Box<GenericArgs>>,
}

#[derive(Clone)]
pub enum GenericArgs {
    Angle(Vec<GenericArg>),
    /// `Fn(A, B) -> C`
    Paren(Vec<TyId>, Option<TyId>),
}

#[derive(Clone)]
pub enum GenericArg {
    Lifetime(Ident),
    Type(TyId),
    Const(ExprId),
    /// `Item = T`
    Binding(Ident, Option<Box<GenericArgs>>, TyId),
    /// `Item: Bound`
    Constraint(Ident, Vec<Bound>),
}

#[derive(Clone)]
pub enum Bound {
    Lifetime(Ident),
    Trait {
        hrtb: Vec<GenericParam>,
        maybe: bool,
        konst: bool,
        path: Path,
    },
    /// `use<'a, T>` precise capture
    Use,
}

#[derive(Clone)]
pub struct GenericParam {
    pub attrs: Vec<Attr>,
    pub name: Ident,
    pub kind: GenericParamKind,
}

#[derive(Clone)]
pub enum GenericParamKind {
    Lifetime(Vec<Ident>),
    Type(Vec<Bound>, Option<TyId>),
    Const(TyId, Option<ExprId>),
}

#[derive(Clone, Default)]
pub struct Generics {
    pub params: Vec<GenericParam>,
    pub where_: Vec<WherePred>,
}

#[derive(Clone)]
pub enum WherePred {
    Bound {
        hrtb: Vec<GenericParam>,
        ty: TyId,
        bounds: Vec<Bound>,
    },
    Lifetime(Ident, Vec<Ident>),
}

// ---------------------------------------------------------------- types

#[derive(Clone)]
pub enum Ty {
    Path(Path),
    Ref(Option<Ident>, bool, TyId),
    Ptr(bool, TyId),
    Slice(TyId),
    Array(TyId, ExprId),
    Tuple(Vec<TyId>),
    Fn(Box<FnPtr>),
    Never,
    Infer,
    ImplTrait(Vec<Bound>),
    DynTrait(Vec<Bound>, bool),
    Paren(TyId),
    Mac(MacCall),
    /// `Self` inside impls is a path; this is the implicit `self` type of `&self`
    ImplicitSelf,
    CVarArgs,
}

#[derive(Clone)]
pub struct FnPtr {
    pub hrtb: Vec<GenericParam>,
    pub unsafe_: bool,
    pub abi: Option<Ident>,
    pub params: Vec<TyId>,
    pub ret: Option<TyId>,
    pub variadic: bool,
}

// ---------------------------------------------------------------- patterns

#[derive(Clone)]
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
    TupleStruct(Path, Vec<PatId>),
    Struct(Path, Vec<FieldPat>, bool),
    Tuple(Vec<PatId>),
    Slice(Vec<PatId>),
    Ref(bool, PatId),
    Or(Vec<PatId>),
    Paren(PatId),
    Box(PatId),
    Mac(MacCall),
}

#[derive(Clone)]
pub struct FieldPat {
    pub attrs: Vec<Attr>,
    pub name: Ident,
    /// None: shorthand `x` / `ref mut x`
    pub pat: PatId,
    pub shorthand: bool,
}

// ---------------------------------------------------------------- expressions

#[derive(Clone, Copy, PartialEq, Eq)]
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Deref,
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

#[derive(Clone)]
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
pub struct MacArgs {
    pub exprs: Vec<ExprId>,
    /// `vec![x; n]`
    pub repeat: bool,
    /// `matches!(e, pat if guard)`
    pub pat: Option<PatId>,
    pub guard: Option<ExprId>,
}

#[derive(Clone)]
pub struct Arm {
    pub attrs: Vec<Attr>,
    pub pat: PatId,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}

#[derive(Clone)]
pub struct FieldExpr {
    pub attrs: Vec<Attr>,
    pub name: Ident,
    pub expr: ExprId,
    pub shorthand: bool,
}

#[derive(Clone)]
pub struct ClosureParam {
    pub attrs: Vec<Attr>,
    pub pat: PatId,
    pub ty: Option<TyId>,
}

#[derive(Clone)]
pub enum ExprKind {
    Lit(LitKind),
    Path(Path),
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Assign(ExprId, ExprId),
    AssignOp(BinOp, ExprId, ExprId),
    Call(ExprId, Vec<ExprId>),
    MethodCall {
        recv: ExprId,
        name: Ident,
        turbofish: Option<Box<GenericArgs>>,
        args: Vec<ExprId>,
    },
    Field(ExprId, Ident),
    TupleField(ExprId, u32),
    Index(ExprId, ExprId),
    AddrOf(bool, bool, ExprId), // (raw, mut, expr)
    Cast(ExprId, TyId),
    Block(BlockId, Option<Ident>),
    Unsafe(BlockId),
    Async(bool, BlockId),
    ConstBlock(BlockId),
    If(ExprId, BlockId, Option<ExprId>),
    Let(PatId, ExprId),
    Match(ExprId, Vec<Arm>),
    While(ExprId, BlockId, Option<Ident>),
    Loop(BlockId, Option<Ident>),
    For(PatId, ExprId, BlockId, Option<Ident>),
    Break(Option<Ident>, Option<ExprId>),
    Continue(Option<Ident>),
    Return(Option<ExprId>),
    Closure {
        is_move: bool,
        is_async: bool,
        params: Vec<ClosureParam>,
        ret: Option<TyId>,
        body: ExprId,
    },
    Tuple(Vec<ExprId>),
    Array(Vec<ExprId>),
    Repeat(ExprId, ExprId),
    Struct(Path, Vec<FieldExpr>, Option<ExprId>),
    Range(Option<ExprId>, Option<ExprId>, bool),
    Try(ExprId),
    Await(ExprId),
    Mac(MacCall),
    Paren(ExprId),
    Underscore,
    Yield(Option<ExprId>),
    Box(ExprId),
}

#[derive(Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub lo: u32,
    pub hi: u32,
}

#[derive(Clone)]
pub enum Stmt {
    Let {
        attrs: Vec<Attr>,
        pat: PatId,
        ty: Option<TyId>,
        init: Option<ExprId>,
        else_: Option<BlockId>,
    },
    Item(ItemId),
    Expr(ExprId, bool),
    Empty,
}

#[derive(Clone)]
pub struct Block {
    pub attrs: Vec<Attr>,
    pub stmts: Vec<Stmt>,
    pub lo: u32,
    pub hi: u32,
}

// ---------------------------------------------------------------- items

#[derive(Clone)]
pub struct Item {
    /// arena ranges [expr_lo, expr_hi, pat_lo, pat_hi) covering everything parsed for this item
    pub ranges: [u32; 4],
    pub attrs: Vec<Attr>,
    pub vis: Vis,
    pub name: Ident,
    pub kind: ItemKind,
    pub lo: u32,
    pub hi: u32,
}

#[derive(Clone)]
pub struct Param {
    pub attrs: Vec<Attr>,
    pub pat: PatId,
    pub ty: TyId,
}

#[derive(Clone)]
pub enum SelfParam {
    Value(bool),                // self / mut self
    Ref(Option<Ident>, bool),   // &'a mut self
    Typed(bool, TyId),          // mut self: Box<Self>
}

#[derive(Clone)]
pub struct FnSig {
    pub konst: bool,
    pub asyncness: bool,
    pub unsafe_: bool,
    pub abi: Option<Option<Ident>>, // extern / extern "C"
    pub generics: Generics,
    pub self_param: Option<SelfParam>,
    pub params: Vec<Param>,
    pub variadic: bool,
    pub ret: Option<TyId>,
    /// lazy-body parse mode: the unparsed body's token range (inside the braces)
    pub lazy_body: Option<TokRange>,
}

#[derive(Clone)]
pub struct FieldDef {
    pub attrs: Vec<Attr>,
    pub vis: Vis,
    pub name: Option<Ident>,
    pub ty: TyId,
}

#[derive(Clone)]
pub enum VariantData {
    Unit,
    Tuple(Vec<FieldDef>),
    Struct(Vec<FieldDef>),
}

#[derive(Clone)]
pub struct Variant {
    pub attrs: Vec<Attr>,
    pub name: Ident,
    pub data: VariantData,
    pub disc: Option<ExprId>,
}

#[derive(Clone)]
pub enum UseTree {
    Path(PathSeg, Box<UseTree>),
    Name(PathSeg, Option<Ident>), // rename; `as _` -> Some(ident "_")
    Glob,
    Group(Vec<UseTree>),
}

#[derive(Clone)]
pub enum ItemKind {
    Use { global: bool, tree: UseTree },
    ExternCrate(Option<Ident>),
    Fn(Box<FnSig>, Option<BlockId>, bool), // bool: `default fn`
    Struct(Generics, VariantData),
    Union(Generics, Vec<FieldDef>),
    Enum(Generics, Vec<Variant>),
    TypeAlias {
        generics: Generics,
        bounds: Vec<Bound>,
        ty: Option<TyId>,
    },
    Const(Option<TyId>, Option<ExprId>),
    Static(bool, TyId, Option<ExprId>),
    Trait {
        unsafe_: bool,
        auto: bool,
        generics: Generics,
        supers: Vec<Bound>,
        items: Vec<ItemId>,
    },
    TraitAlias(Generics, Vec<Bound>),
    Impl {
        unsafe_: bool,
        default_: bool,
        generics: Generics,
        negative: bool,
        trait_: Option<Path>,
        self_ty: TyId,
        items: Vec<ItemId>,
    },
    Mod(Option<Vec<ItemId>>),
    ForeignMod(Option<Ident>, Vec<ItemId>),
    MacroRules(TokRange),
    Mac(MacCall),
}

#[derive(Default)]
pub struct Ast {
    pub exprs: Vec<Expr>,
    pub pats: Vec<Pat>,
    pub tys: Vec<Ty>,
    pub items: Vec<Item>,
    pub blocks: Vec<Block>,
    pub root_attrs: Vec<Attr>,
    pub root_items: Vec<ItemId>,
    pub mac_args: Vec<MacArgs>,
}

impl Ast {
    #[inline]
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }
    #[inline]
    pub fn pat(&self, id: PatId) -> &Pat {
        &self.pats[id.0 as usize]
    }
    #[inline]
    pub fn ty(&self, id: TyId) -> &Ty {
        &self.tys[id.0 as usize]
    }
    #[inline]
    pub fn item(&self, id: ItemId) -> &Item {
        &self.items[id.0 as usize]
    }
    #[inline]
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }
}
