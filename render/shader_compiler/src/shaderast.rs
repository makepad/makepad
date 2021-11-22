use makepad_live_compiler::Span;
use makepad_live_compiler::Token;
use makepad_live_compiler::Id;
use makepad_live_compiler::LivePtr;
use makepad_live_compiler::LiveType;
use makepad_live_compiler::LiveOrCalc;
use makepad_live_compiler::Vec4;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::BTreeSet;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;
use std::ops::Deref;
use std::ops::DerefMut;
use makepad_live_compiler::PrettyPrintedF32;
use makepad_live_compiler::id;
//use crate::shaderregistry::ShaderResourceId;

// all the Live node pointer newtypes

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct FnPtr(pub LivePtr);
impl Deref for FnPtr{type Target = LivePtr;fn deref(&self) -> &Self::Target {&self.0}}
impl DerefMut for FnPtr{fn deref_mut(&mut self) -> &mut Self::Target {&mut self.0}} 

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct StructPtr(pub LivePtr);
impl Deref for StructPtr{type Target = LivePtr;fn deref(&self) -> &Self::Target {&self.0}}
impl DerefMut for StructPtr{fn deref_mut(&mut self) -> &mut Self::Target {&mut self.0}} 

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct DrawShaderPtr(pub LivePtr);
impl Deref for DrawShaderPtr{type Target = LivePtr;fn deref(&self) -> &Self::Target {&self.0}}
impl DerefMut for DrawShaderPtr{fn deref_mut(&mut self) -> &mut Self::Target {&mut self.0}} 

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ConstPtr(pub LivePtr);
impl Deref for ConstPtr{type Target = LivePtr;fn deref(&self) -> &Self::Target {&self.0}}
impl DerefMut for ConstPtr{fn deref_mut(&mut self) -> &mut Self::Target {&mut self.0}} 

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ValuePtr(pub LivePtr);
impl Deref for ValuePtr{type Target = LivePtr;fn deref(&self) -> &Self::Target {&self.0}}
impl DerefMut for ValuePtr{fn deref_mut(&mut self) -> &mut Self::Target {&mut self.0}} 

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct VarDefPtr(pub LivePtr);
impl Deref for VarDefPtr{type Target = LivePtr;fn deref(&self) -> &Self::Target {&self.0}}
impl DerefMut for VarDefPtr{fn deref_mut(&mut self) -> &mut Self::Target {&mut self.0}} 


#[derive(Clone, Default)]
pub struct DrawShaderConstTable {
    pub table: Vec<f32>,
    pub offsets: BTreeMap<FnPtr, usize>
}


#[derive(Clone, Copy, Default)]
pub struct DrawShaderFlags{
    pub debug: bool,
    pub draw_call_compare: bool,
    pub draw_call_always: bool,
}

#[derive(Clone, Default)]
pub struct DrawShaderDef {
    pub flags: DrawShaderFlags,
    //pub default_geometry: Option<ShaderResourceId>,
    pub fields: Vec<DrawShaderFieldDef>,
    pub methods: Vec<FnPtr>,
    pub enums: Vec<LiveType>,
    // analysis results:
    pub all_const_refs: RefCell<BTreeSet<ConstPtr>>,
    pub all_live_refs: RefCell<BTreeMap<ValuePtr, Ty >>,
    pub all_fns: RefCell<Vec<FnPtr >>,
    pub vertex_fns: RefCell<Vec<FnPtr >>,
    pub pixel_fns: RefCell<Vec<FnPtr >>,
    pub all_structs: RefCell<Vec<StructPtr >>,
    pub vertex_structs: RefCell<Vec<StructPtr >>,
    pub pixel_structs: RefCell<Vec<StructPtr >>,
    // ok these 2 things dont belong here
    //pub const_table: DrawShaderConstTable,
    //pub var_inputs: RefCell<DrawShaderVarInputs>
}

#[derive(Clone)]
pub struct DrawShaderFieldDef {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub kind: DrawShaderFieldKind
}

/*
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DrawShaderInputType {
    VarDef(LivePtr),
    ShaderResourceId(ShaderResourceId)
}*/



#[derive(Clone)]
pub enum DrawShaderFieldKind {
    Geometry {
        is_used_in_pixel_shader: Cell<bool >,
        var_def_ptr: Option<VarDefPtr>,
    },
    Instance {
        is_used_in_pixel_shader: Cell<bool >,
        live_or_calc: LiveOrCalc,
        var_def_ptr: Option<VarDefPtr>,
        //input_type: DrawShaderInputType,
    },
    Texture {
        var_def_ptr: Option<VarDefPtr>,
        //input_type: DrawShaderInputType,
    },
    Uniform {
        var_def_ptr: Option<VarDefPtr>,
        //input_type: DrawShaderInputType,
        block_ident: Ident,
    },
    Varying {
        var_def_ptr: VarDefPtr,
    }
}

#[derive(Clone)]
pub struct ConstDef {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub expr: Expr,
}

// the unique identification of a fn call
//#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
//pub enum Callee {
//    PlainFn {fn_node_ptr: FnNodePtr},
//    DrawShaderMethod {shader_node_ptr: DrawShaderNodePtr, ident: Ident},
//    StructMethod {struct_node_ptr: StructNodePtr, ident: Ident},
//}

#[derive(Clone, Copy,  Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum FnSelfKind {
    Struct(StructPtr),
    DrawShader(DrawShaderPtr)
}

impl FnSelfKind {
    pub fn to_ty_expr_kind(self) -> TyExprKind {
        match self {
            FnSelfKind::DrawShader(shader_ptr) => {
                TyExprKind::DrawShader(shader_ptr)
            },
            FnSelfKind::Struct(struct_ptr) => {
                TyExprKind::Struct(struct_ptr)
            },
        }
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum HiddenArgKind {
    Geometries,
    Instances,
    Varyings,
    Textures,
    Uniform(Ident),
    LiveUniforms,
}

#[derive(Clone)]
pub struct FnDef {
    pub fn_ptr: FnPtr,
    
    pub ident: Ident,
    
    pub self_kind: Option<FnSelfKind>,
    pub has_return: Cell<bool>,
    
    pub callees: RefCell<Option<BTreeSet<FnPtr >> >,
    pub builtin_deps: RefCell<Option<BTreeSet<Ident >> >,
    // pub closure_deps: RefCell<Option<BTreeSet<Ident >> >,
    
    // the const table (per function)
    pub const_table: RefCell<Option<Vec<f32 >> >,
    pub const_table_spans: RefCell<Option<Vec<(usize, Span) >> >,
    
    pub hidden_args: RefCell<Option<BTreeSet<HiddenArgKind >> >,
    pub draw_shader_refs: RefCell<Option<BTreeSet<Ident >> >,
    pub const_refs: RefCell<Option<BTreeSet<ConstPtr >> >,
    pub live_refs: RefCell<Option<BTreeMap<ValuePtr, Ty >> >,
    
    pub struct_refs: RefCell<Option<BTreeSet<StructPtr >> >,
    pub constructor_fn_deps: RefCell<Option<BTreeSet<(TyLit, Vec<Ty>) >> >,
    
    pub closure_defs: Vec<ClosureDef>,
    pub closure_sites: RefCell<Option<Vec<ClosureSite >> >,
    
    // base
    pub span: Span,
    pub return_ty: RefCell<Option<Ty >>,
    pub params: Vec<Param>,
    pub return_ty_expr: Option<TyExpr>,
    pub block: Block,
}


#[derive(Clone, Debug, Copy, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ClosureDefIndex(pub usize);

#[derive(Clone)]
pub struct ClosureParam {
    pub span: Span,
    pub ident: Ident,
    pub shadow: Cell<Option<ScopeSymShadow >>
}

#[derive(Clone)]
pub struct ClosureDef {
    pub span: Span,
    pub closed_over_syms: RefCell<Option<Vec<Sym >> >,
    pub params: Vec<ClosureParam>,
    pub kind: ClosureDefKind
}

#[derive(Clone)]
pub enum ClosureDefKind {
    Expr(Expr),
    Block(Block)
}

#[derive(Clone)]
pub struct ClosureSite { //
    pub call_to: FnPtr,
    pub all_closed_over: BTreeSet<Sym>,
    pub closure_args: Vec<ClosureSiteArg>
}

#[derive(Clone, Copy)]
pub struct ClosureSiteArg {
    pub param_index: usize,
    pub closure_def_index: ClosureDefIndex
}

#[derive(Clone)]
pub struct StructDef {
    pub span: Span,
    //pub ident: Ident,
    pub struct_refs: RefCell<Option<BTreeSet<StructPtr >> >,
    pub fields: Vec<StructFieldDef>,
    pub methods: Vec<FnPtr>,
}

#[derive(Clone)]
pub struct StructFieldDef {
    pub var_def_ptr: VarDefPtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

impl StructDef {
    pub fn find_field(&self, ident: Ident) -> Option<&StructFieldDef> {
        self.fields.iter().find( | field | field.ident == ident)
    }
    
}


#[derive(Clone)]
pub struct Param {
    pub span: Span,
    pub is_inout: bool,
    pub ident: Ident,
    pub shadow: Cell<Option<ScopeSymShadow >>,
    pub ty_expr: TyExpr,
}

#[derive(Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone)]
pub enum Stmt {
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    For {
        span: Span,
        ident: Ident,
        from_expr: Expr,
        to_expr: Expr,
        step_expr: Option<Expr>,
        block: Box<Block>,
    },
    If {
        span: Span,
        expr: Expr,
        block_if_true: Box<Block>,
        block_if_false: Option<Box<Block >>,
    },
    Match {
        span: Span,
        expr: Expr,
        matches: Vec<Match>,
    },

    Let {
        span: Span,
        ty: RefCell<Option<Ty >>,
        shadow: Cell<Option<ScopeSymShadow >>,
        ident: Ident,
        ty_expr: Option<TyExpr>,
        expr: Option<Expr>,
    },
    Return {
        span: Span,
        expr: Option<Expr>,
    },
    Block {
        span: Span,
        block: Box<Block>,
    },
    Expr {
        span: Span,
        expr: Expr,
    },
}

#[derive(Clone)]
pub struct Match {
    pub span: Span,
    pub enum_name: Ident,
    pub enum_variant: Ident,
    pub enum_value: Cell<Option<usize>>,
    pub block: Block
}

#[derive(Clone)]
pub struct Expr {
    pub span: Span,
    pub ty: RefCell<Option<Ty >>,
    pub const_val: RefCell<Option<Option<Val >> >,
    pub const_index: Cell<Option<usize >>,
    pub kind: ExprKind,
}

#[derive(Clone)]
pub enum ExprKind {
    Cond {
        span: Span,
        expr: Box<Expr>,
        expr_if_true: Box<Expr>,
        expr_if_false: Box<Expr>,
    },
    Bin {
        span: Span,
        op: BinOp,
        left_expr: Box<Expr>,
        right_expr: Box<Expr>,
    },
    Un {
        span: Span,
        op: UnOp,
        expr: Box<Expr>,
    },
    Field {
        span: Span,
        expr: Box<Expr>,
        field_ident: Ident,
    },
    Index {
        span: Span,
        expr: Box<Expr>,
        index_expr: Box<Expr>,
    },
    MethodCall {
        span: Span,
        ident: Ident,
        closure_site_index: Cell<Option<usize >>,
        arg_exprs: Vec<Expr>,
    },
    PlainCall { // not very pretty but otherwise closures cannot override a normal fn
        // possible solution is to capture it in a refcell sub-enum.
        span: Span,
        fn_ptr: Option<FnPtr>,
        ident: Option<Ident>,
        param_index: Cell<Option<usize >>, // used by the closure case
        closure_site_index: Cell<Option<usize >>, // used by the plain fn case
        arg_exprs: Vec<Expr>,
    },
    BuiltinCall {
        span: Span,
        ident: Ident,
        arg_exprs: Vec<Expr>,
    },
    ClosureDef(ClosureDefIndex),
    ConsCall {
        span: Span,
        ty_lit: TyLit,
        arg_exprs: Vec<Expr>,
    },
    StructCons {
        struct_ptr: StructPtr,
        span: Span,
        args: Vec<(Ident, Expr)>
    },
    Var {
        span: Span,
        ident: Option<Ident>,
        kind: Cell<Option<VarKind >>,
        var_resolve: VarResolve,
        //ident_path: IdentPath,
    },
    Lit {
        span: Span,
        lit: Lit,
    },
}

pub enum PlainCallType {
    Plain {
        
    }
}

#[derive(Clone, Copy)]
pub enum VarResolve {
    NotFound,
    Function(FnPtr),
    Const(ConstPtr),
    LiveValue(ValuePtr, TyLit)
}

#[derive(Clone, Copy)]
pub enum VarKind {
    Local {ident: Ident, shadow: ScopeSymShadow},
    MutLocal {ident: Ident, shadow: ScopeSymShadow},
    Const(ConstPtr),
    LiveValue(ValuePtr)
}

#[derive(Clone)]
pub struct TyExpr {
    pub span: Span,
    pub ty: RefCell<Option<Ty >>,
    pub kind: TyExprKind,
}

#[derive(Clone)]
pub enum TyExprKind {
    Array {
        elem_ty_expr: Box<TyExpr>,
        len: u32,
    },
    Struct(StructPtr),
    Enum(LiveType),
    DrawShader(DrawShaderPtr),
    Lit {
        ty_lit: TyLit,
    },
    ClosureDecl {
        return_ty: RefCell<Option<Ty >>,
        return_ty_expr: Box<Option<TyExpr >>,
        params: Vec<Param>
    },
}


#[derive(Clone, Copy)]
pub enum MacroCallAnalysis {
}

#[derive(Clone, Copy)]
pub enum BinOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}



#[derive(Clone, Copy)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum Ty {
    Void,
    Bool,
    Int,
    Float,
    Bvec2,
    Bvec3,
    Bvec4,
    Ivec2,
    Ivec3,
    Ivec4,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
    Texture2D,
    Array {elem_ty: Rc<Ty>, len: usize},
    Struct(StructPtr),
    Enum(LiveType),
    DrawShader(DrawShaderPtr),
    ClosureDef(ClosureDefIndex),
    ClosureDecl
}


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum TyLit {
    Bool,
    Int,
    Float,
    Bvec2,
    Bvec3,
    Bvec4,
    Ivec2,
    Ivec3,
    Ivec4,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
    Texture2D,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Lit {
    Bool(bool),
    Int(i32),
    Float(f32),
    Color(u32),
}

#[derive(Clone, PartialEq)]
pub enum Val {
    Bool(bool),
    Int(i32),
    Float(f32),
    Vec4(Vec4),
}


pub type Scope = HashMap<Ident, ScopeSym>;

#[derive(Clone)]
pub struct Scopes {
    pub scopes: Vec<Scope>,
    pub closure_scopes: RefCell<HashMap<ClosureDefIndex, Vec<Scope >> >,
    pub closure_sites: RefCell<Vec<ClosureSite >>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ScopeSymShadow(pub usize);

#[derive(Clone, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct Sym {
    pub ident: Ident,
    pub ty: Ty,
    pub shadow: ScopeSymShadow, // how many times this symbol has been shadowed
}

#[derive(Clone)]
pub struct ScopeSym {
    pub span: Span,
    pub sym: Sym,
    pub referenced: Cell<bool>,
    pub kind: ScopeSymKind
}

#[derive(Clone)]
pub enum ScopeSymKind {
    Local,
    MutLocal,
    Closure {
        return_ty: Ty,
        param_index: usize,
        params: Vec<Param>
    }
}

impl Scopes {
    pub fn new() -> Scopes {
        Scopes {
            closure_scopes: RefCell::new(HashMap::new()),
            closure_sites: RefCell::new(Vec::new()),
            scopes: Vec::new(),
        }
    }
    
    pub fn find_sym_on_scopes(&self, ident: Ident, _span: Span,) -> Option<&ScopeSym> {
        let ret = self.scopes.iter().rev().find_map( | scope | scope.get(&ident));
        if ret.is_some() {
            return Some(ret.unwrap())
        }
        return None
    }
    
    pub fn capture_closure_scope(&self, index: ClosureDefIndex) {
        self.closure_scopes.borrow_mut().insert(index, self.scopes.clone());
    }
    
    pub fn push_scope(&mut self) {
        self.scopes.push(Scope::new())
    }
    
    pub fn pop_scope(&mut self) {
        self.scopes.pop().unwrap();
    }
    
    pub fn clear_referenced_syms(&self) {
        for scope in self.scopes.iter().rev() {
            for (_, sym) in scope {
                sym.referenced.set(false);
            }
        }
    }
    
    pub fn all_referenced_syms(&self) -> Vec<Sym> {
        let mut ret = Vec::new();
        for scope in self.scopes.iter().rev() {
            for (_, sym) in scope {
                if sym.referenced.get() {
                    ret.push(sym.sym.clone());
                }
            }
        }
        ret
    }
    
    pub fn insert_sym(&mut self, span: Span, ident: Ident, ty: Ty, sym_kind: ScopeSymKind) -> ScopeSymShadow {
        
        if let Some(item) = self.scopes.last_mut().unwrap().get_mut(&ident) {
            item.sym.shadow = ScopeSymShadow(item.sym.shadow.0 + 1);
            item.kind = sym_kind;
            item.sym.shadow
        }
        else {
            // find it anyway
            let shadow = if let Some(ret) = self.scopes.iter().rev().find_map( | scope | scope.get(&ident)) {
                ScopeSymShadow(ret.sym.shadow.0 + 1)
            }
            else {
                ScopeSymShadow(0)
            };
            self.scopes.last_mut().unwrap().insert(ident, ScopeSym {
                sym: Sym {
                    ty,
                    ident,
                    shadow,
                },
                span,
                referenced: Cell::new(false),
                kind: sym_kind
            });
            shadow
        }
    }
}

#[derive(Clone, Copy, Ord, PartialOrd, Default, Eq, Hash, PartialEq)]
pub struct Ident(pub Id);


impl StructDef {
    pub fn init_analysis(&self) {
        *self.struct_refs.borrow_mut() = Some(BTreeSet::new());
    }
}


impl FnDef {

    pub fn new(
        fn_ptr: FnPtr,
        span: Span,
        ident: Ident,
        self_kind: Option<FnSelfKind>,
        params: Vec<Param>,
        return_ty_expr: Option<TyExpr>,
        block:Block,
        closure_defs:Vec<ClosureDef>
    )->Self{
        FnDef {
            fn_ptr,
            span,
            ident,
            self_kind,
            params,
            return_ty_expr,
            block,
            closure_defs,
            has_return: Cell::new(false),
            hidden_args:RefCell::new(None),
            closure_sites: RefCell::new(None),
            const_refs: RefCell::new(None),
            live_refs: RefCell::new(None),
            struct_refs: RefCell::new(None),
            draw_shader_refs: RefCell::new(None),
            return_ty: RefCell::new(None),
            callees: RefCell::new(None),
            builtin_deps: RefCell::new(None),
            constructor_fn_deps: RefCell::new(None),
            const_table: RefCell::new(None),
            const_table_spans: RefCell::new(None),
        }
    }

    pub fn init_analysis(&self) {
        *self.struct_refs.borrow_mut() = Some(BTreeSet::new());
        *self.callees.borrow_mut() = Some(BTreeSet::new());
        *self.builtin_deps.borrow_mut() = Some(BTreeSet::new());
        //*self.closure_deps.borrow_mut() = Some(BTreeSet::new());
        *self.constructor_fn_deps.borrow_mut() = Some(BTreeSet::new());
        *self.draw_shader_refs.borrow_mut() = Some(BTreeSet::new());
        *self.const_refs.borrow_mut() = Some(BTreeSet::new());
        *self.live_refs.borrow_mut() = Some(BTreeMap::new());
        *self.const_table.borrow_mut() = Some(Vec::new());
        *self.const_table_spans.borrow_mut() = Some(Vec::new());
    }
    
    pub fn has_closure_args(&self) -> bool {
        for param in &self.params {
            if let TyExprKind::ClosureDecl {..} = &param.ty_expr.kind {
                return true
            }
        }
        return false
    }
}

impl DrawShaderDef {
    
    pub fn find_field(&self, ident: Ident) -> Option<&DrawShaderFieldDef> {
        self.fields.iter().find( | decl | {
            decl.ident == ident
        })
    }
    
    pub fn fields_as_uniform_blocks(&self)->BTreeMap<Ident,Vec<(usize,Ident)>>{
        let mut uniform_blocks = BTreeMap::new();
        for (field_index, field) in self.fields.iter().enumerate() {
            match &field.kind {
                DrawShaderFieldKind::Uniform{
                    block_ident,
                    ..
                } => {
                    let uniform_block = uniform_blocks
                        .entry(*block_ident)
                        .or_insert(Vec::new());
                    uniform_block.push((field_index, field.ident));
                }
                _ => {}
            }
        }
        uniform_blocks
    }
    
    pub fn add_uniform(&mut self, id:Id, block:Id, ty:Ty, span:Span){
        self.fields.push(
            DrawShaderFieldDef {
                kind: DrawShaderFieldKind::Uniform {
                    block_ident: Ident(block),
                    var_def_ptr: None
                },
                span,
                ident: Ident(id),
                ty_expr: ty.to_ty_expr(),
            }
        )
    }
    
    pub fn add_instance(&mut self, id:Id, ty:Ty, span:Span, live_or_calc:LiveOrCalc){
        self.fields.push(
            DrawShaderFieldDef { 
                kind: DrawShaderFieldKind::Instance { 
                    live_or_calc,
                    is_used_in_pixel_shader: Cell::new(false),
                    var_def_ptr: None
                },
                span,
                ident: Ident(id),
                ty_expr: ty.to_ty_expr(),
            }
        )
    }   

    pub fn add_geometry(&mut self, id:Id, ty:Ty, span:Span){
        self.fields.push(
            DrawShaderFieldDef {
                kind: DrawShaderFieldKind::Geometry {
                    is_used_in_pixel_shader: Cell::new(false),
                    var_def_ptr: None
                },
                span,
                ident: Ident(id),
                ty_expr: ty.to_ty_expr(),
            }
        )
    }  
    
     
    pub fn add_texture(&mut self, id:Id, ty:Ty, span:Span){
        self.fields.push(
            DrawShaderFieldDef {
                kind: DrawShaderFieldKind::Texture {
                    var_def_ptr: None
                },
                span,
                ident: Ident(id),
                ty_expr: ty.to_ty_expr(),
            }
        )
    }    
}

impl BinOp {
    pub fn from_assign_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!( =)) => Some(BinOp::Assign),
            Token::Punct(id!( +=)) => Some(BinOp::AddAssign),
            Token::Punct(id!( -=)) => Some(BinOp::SubAssign),
            Token::Punct(id!( *=)) => Some(BinOp::MulAssign),
            Token::Punct(id!( /=)) => Some(BinOp::DivAssign),
            _ => None,
        }
    }
    
    pub fn from_or_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!( ||)) => Some(BinOp::Or),
            _ => None,
        }
    }
    
    pub fn from_and_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!( &&)) => Some(BinOp::And),
            _ => None,
        }
    }
    
    pub fn from_eq_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!( ==)) => Some(BinOp::Eq),
            Token::Punct(id!( !=)) => Some(BinOp::Ne),
            _ => None,
        }
    }
    
    pub fn from_rel_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!(<)) => Some(BinOp::Lt),
            Token::Punct(id!( <=)) => Some(BinOp::Le),
            Token::Punct(id!(>)) => Some(BinOp::Gt),
            Token::Punct(id!( >=)) => Some(BinOp::Ge),
            _ => None,
        }
    }
    
    pub fn from_add_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!( +)) => Some(BinOp::Add),
            Token::Punct(id!(-)) => Some(BinOp::Sub),
            _ => None,
        }
    }
    
    pub fn from_mul_op(token: Token) -> Option<BinOp> {
        match token {
            Token::Punct(id!(*)) => Some(BinOp::Mul),
            Token::Punct(id!( /)) => Some(BinOp::Div),
            _ => None,
        }
    }
    
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                BinOp::Assign => "=",
                BinOp::AddAssign => "+=",
                BinOp::SubAssign => "-=",
                BinOp::MulAssign => "*=",
                BinOp::DivAssign => "/=",
                BinOp::Or => "||",
                BinOp::And => "&&",
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
            }
        )
    }
}

impl UnOp {
    pub fn from_un_op(token: Token) -> Option<UnOp> {
        match token {
            Token::Punct(id!(!)) => Some(UnOp::Not),
            Token::Punct(id!(-)) => Some(UnOp::Neg),
            _ => None,
        }
    }
}

impl fmt::Display for UnOp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                UnOp::Not => "!",
                UnOp::Neg => "-",
            }
        )
    }
}


impl Ty {
    
    pub fn maybe_ty_lit(&self) -> Option<TyLit> {
        match self {
            Ty::Void => None,
            Ty::Bool => Some(TyLit::Bool),
            Ty::Int => Some(TyLit::Int),
            Ty::Float => Some(TyLit::Float),
            Ty::Bvec2 => Some(TyLit::Bvec2),
            Ty::Bvec3 => Some(TyLit::Bvec3),
            Ty::Bvec4 => Some(TyLit::Bvec4),
            Ty::Ivec2 => Some(TyLit::Ivec2),
            Ty::Ivec3 => Some(TyLit::Ivec3),
            Ty::Ivec4 => Some(TyLit::Ivec4),
            Ty::Vec2 => Some(TyLit::Vec2),
            Ty::Vec3 => Some(TyLit::Vec3),
            Ty::Vec4 => Some(TyLit::Vec4),
            Ty::Mat2 => Some(TyLit::Mat2),
            Ty::Mat3 => Some(TyLit::Mat3),
            Ty::Mat4 => Some(TyLit::Mat4),
            Ty::Texture2D => Some(TyLit::Bool),
            Ty::Array {..} => None,
            Ty::Struct(_) => None,
            Ty::Enum(_) => None,
            Ty::DrawShader(_) => None,
            Ty::ClosureDecl => None,
            Ty::ClosureDef {..} => None
        }
    }
    
    pub fn is_scalar(&self) -> bool {
        match self {
            Ty::Bool | Ty::Int | Ty::Float => true,
            _ => false,
        }
    }
    
    pub fn is_vector(&self) -> bool {
        match self {
            Ty::Bvec2
                | Ty::Bvec3
                | Ty::Bvec4
                | Ty::Ivec2
                | Ty::Ivec3
                | Ty::Ivec4
                | Ty::Vec2
                | Ty::Vec3
                | Ty::Vec4 => true,
            _ => false,
        }
    }
    
    pub fn is_matrix(&self) -> bool {
        match self {
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => true,
            _ => false,
        }
    }
    
    pub fn slots(&self) -> usize {
        match self {
            Ty::Void => 0,
            Ty::Bool | Ty::Int | Ty::Float => 1,
            Ty::Bvec2 | Ty::Ivec2 | Ty::Vec2 => 2,
            Ty::Bvec3 | Ty::Ivec3 | Ty::Vec3 => 3,
            Ty::Bvec4 | Ty::Ivec4 | Ty::Vec4 | Ty::Mat2 => 4,
            Ty::Mat3 => 9,
            Ty::Mat4 => 16,
            Ty::Texture2D {..} => panic!(),
            Ty::Array {elem_ty, len} => elem_ty.slots() * len,
            Ty::Enum(_) => 1,
            Ty::Struct(_) => panic!(),
            Ty::DrawShader(_) => panic!(),
            Ty::ClosureDecl => panic!(),
            Ty::ClosureDef {..} => panic!(),
        }
    }
    
    pub fn to_ty_expr(&self) -> TyExpr {
        TyExpr {
            ty: RefCell::new(None),
            span: Span::default(),
            kind: match self{
                Ty::Void=> panic!(),
                Ty::Bool=> TyExprKind::Lit{ty_lit:TyLit::Bool},
                Ty::Int=> TyExprKind::Lit{ty_lit:TyLit::Int},
                Ty::Float=> TyExprKind::Lit{ty_lit:TyLit::Float},
                Ty::Bvec2=> TyExprKind::Lit{ty_lit:TyLit::Bvec2},
                Ty::Bvec3=> TyExprKind::Lit{ty_lit:TyLit::Bvec3},
                Ty::Bvec4=> TyExprKind::Lit{ty_lit:TyLit::Bvec4},
                Ty::Ivec2=> TyExprKind::Lit{ty_lit:TyLit::Ivec2},
                Ty::Ivec3=> TyExprKind::Lit{ty_lit:TyLit::Ivec3},
                Ty::Ivec4=> TyExprKind::Lit{ty_lit:TyLit::Ivec4},
                Ty::Vec2=> TyExprKind::Lit{ty_lit:TyLit::Vec2},
                Ty::Vec3=> TyExprKind::Lit{ty_lit:TyLit::Vec3},
                Ty::Vec4=> TyExprKind::Lit{ty_lit:TyLit::Vec4},
                Ty::Mat2=> TyExprKind::Lit{ty_lit:TyLit::Mat2},
                Ty::Mat3=> TyExprKind::Lit{ty_lit:TyLit::Mat3},
                Ty::Mat4=> TyExprKind::Lit{ty_lit:TyLit::Mat4},
                Ty::Texture2D=> TyExprKind::Lit{ty_lit:TyLit::Texture2D},
                Ty::Array{elem_ty, len}=>{
                    TyExprKind::Array{
                        elem_ty_expr: Box::new(elem_ty.to_ty_expr()),
                        len:*len as u32
                    }
                }
                Ty::Struct(struct_node_ptr)=>{
                    TyExprKind::Struct(*struct_node_ptr)
                }
                Ty::DrawShader(draw_shader_node_ptr)=>{
                    TyExprKind::DrawShader(*draw_shader_node_ptr)
                },
                Ty::Enum(live_type) => {
                    TyExprKind::Enum(*live_type)
                },
                Ty::ClosureDef(_)=>panic!(),
                Ty::ClosureDecl=>panic!()
            }
        }
    }    
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Ty::Void => write!(f, "void"),
            Ty::Bool => write!(f, "bool"),
            Ty::Int => write!(f, "int"),
            Ty::Float => write!(f, "float"),
            Ty::Bvec2 => write!(f, "bvec2"),
            Ty::Bvec3 => write!(f, "bvec3"),
            Ty::Bvec4 => write!(f, "bvec4"),
            Ty::Ivec2 => write!(f, "ivec2"),
            Ty::Ivec3 => write!(f, "ivec3"),
            Ty::Ivec4 => write!(f, "ivec4"),
            Ty::Vec2 => write!(f, "vec2"),
            Ty::Vec3 => write!(f, "vec3"),
            Ty::Vec4 => write!(f, "vec4"),
            Ty::Mat2 => write!(f, "mat2"),
            Ty::Mat3 => write!(f, "mat3"),
            Ty::Mat4 => write!(f, "mat4"),
            Ty::Texture2D => write!(f, "texture2D"),
            Ty::Array {elem_ty, len} => write!(f, "{}[{}]", elem_ty, len),
            Ty::Struct(struct_ptr) => write!(f, "Struct:{:?}", struct_ptr),
            Ty::DrawShader(shader_ptr) => write!(f, "DrawShader:{:?}", shader_ptr),
            Ty::Enum(_) => write!(f, "Enum"),
            Ty::ClosureDecl => write!(f, "ClosureDecl"),
            Ty::ClosureDef {..} => write!(f, "ClosureDef"),
        }
    }
}

impl TyLit {
    pub fn from_id(id: Id) -> Option<TyLit> {
        match id {
            id!(vec4) => Some(TyLit::Vec4),
            id!(vec3) => Some(TyLit::Vec3),
            id!(vec2) => Some(TyLit::Vec2),
            id!(mat4) => Some(TyLit::Mat4),
            id!(mat3) => Some(TyLit::Mat3),
            id!(mat2) => Some(TyLit::Mat2),
            id!(float) => Some(TyLit::Float),
            id!(bool) => Some(TyLit::Bool),
            id!(int) => Some(TyLit::Int),
            id!(bvec2) => Some(TyLit::Bvec2),
            id!(bvec3) => Some(TyLit::Bvec3),
            id!(bvec4) => Some(TyLit::Bvec4),
            id!(ivec2) => Some(TyLit::Ivec4),
            id!(ivec3) => Some(TyLit::Ivec4),
            id!(ivec4) => Some(TyLit::Ivec4),
            id!(texture2D) => Some(TyLit::Texture2D),
            _ => None
        }
    }
    
    pub fn to_ty(self) -> Ty {
        match self {
            TyLit::Bool => Ty::Bool,
            TyLit::Int => Ty::Int,
            TyLit::Float => Ty::Float,
            TyLit::Bvec2 => Ty::Bvec2,
            TyLit::Bvec3 => Ty::Bvec3,
            TyLit::Bvec4 => Ty::Bvec4,
            TyLit::Ivec2 => Ty::Ivec2,
            TyLit::Ivec3 => Ty::Ivec3,
            TyLit::Ivec4 => Ty::Ivec4,
            TyLit::Vec2 => Ty::Vec2,
            TyLit::Vec3 => Ty::Vec3,
            TyLit::Vec4 => Ty::Vec4,
            TyLit::Mat2 => Ty::Mat2,
            TyLit::Mat3 => Ty::Mat3,
            TyLit::Mat4 => Ty::Mat4,
            TyLit::Texture2D => Ty::Texture2D,
        }
    }
    
}

impl fmt::Display for TyLit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TyLit::Bool => "bool",
                TyLit::Int => "int",
                TyLit::Float => "float",
                TyLit::Bvec2 => "bvec2",
                TyLit::Bvec3 => "bvec3",
                TyLit::Bvec4 => "bvec4",
                TyLit::Ivec2 => "ivec2",
                TyLit::Ivec3 => "ivec3",
                TyLit::Ivec4 => "ivec4",
                TyLit::Vec2 => "vec2",
                TyLit::Vec3 => "vec3",
                TyLit::Vec4 => "vec4",
                TyLit::Mat2 => "mat2",
                TyLit::Mat3 => "mat3",
                TyLit::Mat4 => "mat4",
                TyLit::Texture2D => "texture2D",
            }
        )
    }
}


impl Lit {
    pub fn to_ty(self) -> Ty {
        match self {
            Lit::Bool(_) => Ty::Bool,
            Lit::Int(_) => Ty::Int,
            Lit::Float(_) => Ty::Float,
            Lit::Color(_) => Ty::Vec4
        }
    }
    
    pub fn to_val(self) -> Val {
        match self {
            Lit::Bool(v) => Val::Bool(v),
            Lit::Int(v) => Val::Int(v),
            Lit::Float(v) => Val::Float(v),
            Lit::Color(v) => Val::Vec4(Vec4::from_u32(v))
        }
    }
    
    pub fn from_token(token: Token) -> Option<Lit> {
        match token {
            Token::Bool(v) => Some(Lit::Bool(v)),
            Token::Int(v) => Some(Lit::Int(v as i32)),
            Token::Float(v) => Some(Lit::Float(v as f32)),
            Token::Color(v) => Some(Lit::Color(v)),
            _ => None
        }
    }
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Lit::Bool(lit) => write!(f, "{}", lit),
            Lit::Int(lit) => write!(f, "{}", lit),
            Lit::Float(lit) => write!(f, "{}", PrettyPrintedF32(*lit)),
            Lit::Color(lit) => {
                let v = Vec4::from_u32(*lit);
                write!(
                    f,
                    "vec4({},{},{},{})",
                    PrettyPrintedF32(v.x),
                    PrettyPrintedF32(v.y),
                    PrettyPrintedF32(v.z),
                    PrettyPrintedF32(v.w)
                )
            }
        }
    }
}


impl Ident {
    pub fn to_id(self) -> Id {self.0}

}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Default, Copy, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct IdentPath {
    pub segs: [Id; 6],
    pub len: usize
}

impl IdentPath {

    pub fn len(&self) -> usize {
        self.len
    }
    
    pub fn push(&mut self, ident: Ident) -> bool {
        if self.len >= self.segs.len() {
            return false
        }
        self.segs[self.len] = ident.0;
        self.len += 1;
        return true
    }
}

impl fmt::Debug for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(f, "::").unwrap();
            }
            let _ = write!(f, "{}", self.segs[i]);
        }
        Ok(())
    }
}

impl fmt::Display for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(f, "::").unwrap();
            }
            let _ = write!(f, "{}", self.segs[i]);
        }
        Ok(())
    }
}

impl Val {
    pub fn to_bool(&self) -> Option<bool> {
        match *self {
            Val::Bool(val) => Some(val),
            _ => None,
        }
    }
    
    pub fn to_int(&self) -> Option<i32> {
        match *self {
            Val::Int(val) => Some(val),
            _ => None,
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Val::Bool(val) => write!(f, "{}", val),
            Val::Int(val) => write!(f, "{}", val),
            Val::Float(v) => write!(f, "{}", PrettyPrintedF32(v)),
            Val::Vec4(val) => write!(f, "{}", val),
        }
    }
}

impl fmt::Display for StructPtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "struct_{}", self.0)
    }
}

impl fmt::Display for FnPtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "fn_{}", self.0)
    }
}

impl fmt::Display for ConstPtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "const_{}", self.0)
    }
}

impl fmt::Display for ValuePtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "live_{}", self.0)
    }
}
