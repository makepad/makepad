use crate::env::VarKind;
use crate::ident::{Ident, IdentPath, QualifiedIdentPath, IdentPathWithSpan};
use crate::lit::{Lit};
use crate::span::{Span, LiveBodyId};
use crate::ty::{Ty,TyLit,TyExpr};
use crate::val::Val;
use crate::livestyles::LiveStyles;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Debug, Default)]
pub struct ShaderAst {
    //pub qualified_ident_path: QualifiedIdentPath,
    pub live_body_id: LiveBodyId,
    pub debug: bool,
    pub default_geometry: Option<IdentPathWithSpan>,
    pub draw_input: Option<(Span, QualifiedIdentPath)>,
    pub decls: Vec<Decl>,
    pub uses: Vec<IdentPathWithSpan>,
    // generated
    pub const_table: RefCell<Option<Vec<f32 >> >,
    pub const_table_spans: RefCell<Option<Vec<(usize, Span) >> >,
    pub livestyle_uniform_deps: RefCell<Option<BTreeSet<(Ty, QualifiedIdentPath) >> >,
}

impl ShaderAst {
    
    pub fn convert_draw_input_to_decls(&mut self, live_styles: &LiveStyles, span: Span) -> Result<(), (Span, String)> {
        // we convert the draw inputs to decls
        if self.draw_input.is_none() {
            // lets check if we have instance decls, ifso its ok
            for decl in &self.decls{
                if let Decl::Instance(_) = decl{
                    return Ok(())
                }
            }
            return Err((span, format!("please define draw_input for shader")))
        }
    
        for decl in &self.decls{
            if let Decl::Instance(_) = decl{
                return Err((span, format!("Both draw_input and instance vars are used, please use one or the other")))
            }
        }

        let (inp_span, qualified_ident_path) = self.draw_input.as_ref().unwrap();
        
        // lets find draw_input
        let draw_input = live_styles.draw_inputs.get(&qualified_ident_path.to_live_item_id());
        if draw_input.is_none() {
            return Err((inp_span.clone(), format!("draw_input {} not registered", qualified_ident_path)));
        }
        let draw_input = draw_input.unwrap();
        
        if draw_input.instances.len() == 0 {
            return Err((inp_span.clone(), format!("please define atleast 1 float in the instance data")))
        }
        
        for instance in &draw_input.instances {
            self.decls.push(
                Decl::Instance(InstanceDecl {
                    is_used_in_fragment_shader: Cell::new(None),
                    span,
                    ident: instance.ident,
                    ty_expr: instance.ty_expr.clone(),
                    qualified_ident_path: instance.qualified_ident_path,
                })
            )
        }
        
        for uniform in &draw_input.uniforms {
            self.decls.push(
                Decl::Uniform(UniformDecl {
                    block_ident: None,
                    span,
                    ident: uniform.ident,
                    ty_expr: uniform.ty_expr.clone(),
                    qualified_ident_path: uniform.qualified_ident_path,
                })
            )
        }
        
        for texture in &draw_input.textures {
            self.decls.push(
                Decl::Texture(TextureDecl {
                    span,
                    ident: texture.ident,
                    ty_expr: texture.ty_expr.clone(),
                    qualified_ident_path: texture.qualified_ident_path,
                })
            )
        }
        return Ok(())
    }
    
    pub fn find_geometry_decl(&self, ident: Ident) -> Option<&GeometryDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Geometry(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    pub fn find_const_decl(&self, ident: Ident) -> Option<&ConstDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Const(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    pub fn find_fn_decl(&self, ident_path: IdentPath) -> Option<&FnDecl> {
        self.decls.iter().rev().find_map( | decl | {
            match decl {
                Decl::Fn(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident_path == ident_path)
        })
    }
    
    pub fn find_instance_decl(&self, ident: Ident) -> Option<&InstanceDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Instance(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    pub fn find_struct_decl(&self, ident: Ident) -> Option<&StructDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Struct(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    pub fn find_uniform_decl(&self, ident: Ident) -> Option<&UniformDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Uniform(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    pub fn find_varying_decl(&self, ident: Ident) -> Option<&VaryingDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Varying(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
}

#[derive(Clone, Debug)]
pub enum Decl {
    Geometry(GeometryDecl),
    Const(ConstDecl),
    Fn(FnDecl),
    Instance(InstanceDecl),
    Struct(StructDecl),
    Texture(TextureDecl),
    Uniform(UniformDecl),
    Varying(VaryingDecl),
}

#[derive(Clone, Debug)]
pub struct GeometryDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub span: Span,
    pub ident: Ident,
    pub qualified_ident_path: QualifiedIdentPath,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub span: Span,
    pub return_ty: RefCell<Option<Ty >>,
    pub is_used_in_vertex_shader: Cell<Option<bool >>,
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub callees: RefCell<Option<BTreeSet<IdentPath >> >,
    pub uniform_block_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub has_texture_deps: Cell<Option<bool >>,
    pub geometry_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub instance_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub has_varying_deps: Cell<Option<bool >>,
    pub builtin_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub cons_fn_deps: RefCell<Option<BTreeSet<(TyLit, Vec<Ty>) >> >,
    pub ident_path: IdentPath,
    pub params: Vec<Param>,
    pub return_ty_expr: Option<TyExpr>,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct InstanceDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub span: Span,
    pub ident: Ident,
    pub qualified_ident_path: QualifiedIdentPath,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub span: Span,
    pub ident: Ident,
    pub fields: Vec<Field>,
}

impl StructDecl {
    pub fn find_field(&self, ident: Ident) -> Option<&Field> {
        self.fields.iter().find( | field | field.ident == ident)
    }
}

#[derive(Clone, Debug)]
pub struct TextureDecl {
    pub span: Span,
    pub ident: Ident,
    pub qualified_ident_path: QualifiedIdentPath,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct UniformDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub qualified_ident_path: QualifiedIdentPath,
    pub block_ident: Option<Ident>,
}

#[derive(Clone, Debug)]
pub struct VaryingDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub span: Span,
    pub is_inout: bool,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
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
    Let {
        span: Span,
        ty: RefCell<Option<Ty >>,
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

#[derive(Clone, Debug)]
pub struct Expr {
    pub span: Span,
    pub ty: RefCell<Option<Ty >>,
    pub const_val: RefCell<Option<Option<Val >> >,
    pub const_index: Cell<Option<usize >>,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
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
    MethodCall {
        span: Span,
        ident: Ident,
        arg_exprs: Vec<Expr>,
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
    Call {
        span: Span,
        ident_path: IdentPath,
        arg_exprs: Vec<Expr>,
    },
    MacroCall {
        span: Span,
        analysis: Cell<Option<MacroCallAnalysis >>,
        ident: Ident,
        arg_exprs: Vec<Expr>,
    },
    ConsCall {
        span: Span,
        ty_lit: TyLit,
        arg_exprs: Vec<Expr>,
    },
    Var {
        span: Span,
        kind: Cell<Option<VarKind >>,
        ident_path: IdentPath,
    },
    Lit {
        span: Span,
        lit: Lit,
    },
}


#[derive(Clone, Copy, Debug)]
pub enum MacroCallAnalysis {
}

#[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Copy, Debug)]
pub enum UnOp {
    Not,
    Neg,
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
