mod hooks;

use crate::ast::*;
use crate::ident::Ident;
use crate::lit::Lit;
use crate::swizzle::Swizzle;
use crate::ty::Ty;
use crate::ty_lit::TyLit;
use crate::value::Value;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::error;
use std::fmt::{self, Write};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct Emitter {
    scope: Scope,
}

impl Emitter {
    pub fn new() -> Emitter {
        let mut scope = Scope::default();
        // TODO
        scope.insert(
            Ident::new("all"),
            Info::Builtin(BuiltinInfo {
                return_tys_by_param_tys: [
                    (vec![Ty::Bvec2], Ty::Bool),
                    (vec![Ty::Bvec3], Ty::Bool),
                    (vec![Ty::Bvec4], Ty::Bool),
                ]
                .iter()
                .cloned()
                .collect(),
            }),
        );
        Emitter { scope }
    }

    fn find_info(&self, ident: Ident) -> Option<&Info> {
        self.scope.get(ident)
    }
}

#[derive(Debug)]
struct ShaderEmitter<'a> {
    parent: &'a mut Emitter,
    scope: Scope,
    fn_decls_by_ident: HashMap<Ident, &'a FnDecl>,
    struct_decls_by_ident: HashMap<Ident, &'a StructDecl>,
    active_fn_idents: Vec<Ident>,
    active_struct_idents: Vec<Ident>,
    fn_decls_attrs: Vec<FnDeclAttrs>,
    struct_decls_attrs: Vec<StructDeclAttrs>,
}

impl<'a> ShaderEmitter<'a> {
    fn new(
        parent: &'a mut Emitter,
        fn_decls_by_ident: HashMap<Ident, &'a FnDecl>,
        struct_decls_by_ident: HashMap<Ident, &'a StructDecl>,
    ) -> ShaderEmitter<'a> {
        ShaderEmitter {
            parent,
            scope: Scope::default(),
            fn_decls_by_ident,
            struct_decls_by_ident,
            active_fn_idents: Vec::new(),
            active_struct_idents: Vec::new(),
            fn_decls_attrs: Vec::new(),
            struct_decls_attrs: Vec::new(),
        }
    }

    fn find_info(&self, ident: Ident) -> Option<&Info> {
        self.scope
            .get(ident)
            .or_else(|| self.parent.find_info(ident))
    }
}

#[derive(Debug)]
struct DeclEmitter<'a, 'b> {
    parent: &'a mut ShaderEmitter<'b>,
    scopes: Vec<Scope>,
    is_entry_point: bool,
    return_ty: Ty,
    is_in_for_stmt_block: bool,
    is_in_lvalue_context: bool,
}

impl<'a, 'b> DeclEmitter<'a, 'b> {
    fn new(parent: &'a mut ShaderEmitter<'b>) -> DeclEmitter<'a, 'b> {
        DeclEmitter {
            parent,
            scopes: Vec::new(),
            is_entry_point: true,
            return_ty: Ty::Void,
            is_in_for_stmt_block: false,
            is_in_lvalue_context: false,
        }
    }

    fn find_info(&self, ident: Ident) -> Option<&Info> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(ident))
            .or_else(|| self.parent.find_info(ident))
    }

    fn scope_mut(&mut self) -> &mut Scope {
        self.scopes.last_mut().unwrap_or(&mut self.parent.scope)
    }
}

#[derive(Clone, Debug)]
pub struct ShaderAttrs {
    pub attribute_decls_attrs: Vec<AttributeDeclAttrs>,
    pub uniform_decls_attrs_by_block_ident: HashMap<Ident, Vec<UniformDeclAttrs>>,
    pub vertex_string: String,
    pub fragment_string: String,
}

#[derive(Clone, Debug)]
pub struct AttributeDeclAttrs {
    pub ident: Ident,
    pub ty: Ty,
}

#[derive(Clone, Debug)]
pub struct UniformDeclAttrs {
    pub ident: Ident,
    pub ty: Ty,
    pub block_ident: Ident,
}

#[derive(Clone, Debug)]
struct VaryingDeclAttrs {
    ident: Ident,
    ty: Ty,
}

#[derive(Clone, Debug)]
struct FnDeclAttrs {
    ident: Ident,
    deps: Deps,
    string: String,
}

#[derive(Clone, Debug)]
struct ParamAttrs {
    ty: Ty,
    string: String,
}

#[derive(Clone, Debug)]
struct StructDeclAttrs {
    string: String,
}

#[derive(Clone, Debug)]
struct MemberAttrs {
    ident: Ident,
    contains_arrays: bool,
    ty: Ty,
    string: String,
}

#[derive(Clone, Debug)]
struct BlockAttrs {
    is_empty: bool,
    string: String,
    deps: Deps,
}

#[derive(Clone, Debug)]
struct StmtAttrs {
    is_block: bool,
    string: String,
    deps: Deps,
}

#[derive(Clone, Debug)]
struct TyExprAttrs {
    contains_arrays: bool,
    ty: Ty,
}

#[derive(Clone, Debug)]
struct ExprAttrs {
    ty: Ty,
    deps: Deps,
    value_or_string: ValueOrString,
}

#[derive(Clone, Debug)]
pub enum Error {
    CannotApplyBinOp {
        op: BinOp,
        x_ty: Ty,
        y_ty: Ty,
    },
    CannotApplyIndexOp {
        x_ty: Ty,
        i_ty: Ty,
    },
    CannotApplyUnOp {
        op: UnOp,
        x_ty: Ty,
    },
    CannotCallCons {
        ty_lit: TyLit,
        xs_ty: Vec<Ty>,
    },
    CannotCallFn {
        ident: Ident,
        xs_ty: Vec<Ty>,
    },
    CannotAssignToAttributeVar {
        ident: Ident,
    },
    CannotAssignToImmutableVar {
        ident: Ident,
    },
    CannotAssignToUniformVar {
        ident: Ident,
    },
    CannotInferTyForVar {
        ident: Ident,
    },
    ExprIsNotConst,
    FnCannotReadFromVaryings {
        ident: Ident,
    },
    FnCannotReadFromAndWriteToVaryings {
        ident: Ident,
    },
    FnCannotWriteToVaryings {
        ident: Ident,
    },
    FnHasCyclicDepChain {
        ident: Ident,
        dep_idents: Vec<Ident>,
    },
    IdentCannotBeRedefined(Ident),
    IdentIsNotAFn(Ident),
    IdentIsNotAStruct(Ident),
    IdentIsNotAVar(Ident),
    IdentIsNotDefined(Ident),
    InvalidBreakStmt,
    InvalidContinueStmt,
    InvalidLeftHandSide,
    InvalidReturnTyForFn {
        ident: Ident,
        ty: Ty,
    },
    InvalidTyForAttributeVar {
        ident: Ident,
        ty: Ty,
    },
    InvalidTyForVaryingVar {
        ident: Ident,
        ty: Ty,
    },
    MemberIsNotDefinedOnTy {
        ident: Ident,
        ty: Ty,
    },
    MismatchedReturnTyForFn {
        ident: Ident,
        expected_return_ty: Ty,
        actual_return_ty: Ty,
    },
    MismatchedTyForArg {
        index: usize,
        fn_ident: Ident,
        expected_ty: Ty,
        actual_ty: Ty,
    },
    MismatchedTyForExpr {
        expected_ty: Ty,
        actual_ty: Ty,
    },
    MissingFn {
        ident: Ident,
    },
    MissingReturnExpr,
    StepMustBeNonZero,
    StepMustBePositive,
    StepMustBeNegative,
    StructHasCyclicDepChain {
        ident: Ident,
        dep_idents: Vec<Ident>,
    },
    TooFewArgsForCall {
        fn_ident: Ident,
        expected_count: usize,
        actual_count: usize,
    },
    TooManyArgsForCall {
        fn_ident: Ident,
        expected_count: usize,
        actual_count: usize,
    },
    TooFewCompsForConsCall {
        ty_lit: TyLit,
        expected_count: usize,
        actual_count: usize,
    },
    TooManyCompsForConsCall {
        ty_lit: TyLit,
        expected_count: usize,
        actual_count: usize,
    },
    TooManyParamsForFn {
        ident: Ident,
        expected_count: usize,
        actual_count: usize,
    },
    TyLitIsNotACons(TyLit),
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::CannotApplyBinOp {
                op,
                ref x_ty,
                ref y_ty,
            } => write!(
                f,
                "operator {} cannot be applied to operands of type {} and {}",
                op, x_ty, y_ty
            ),
            Error::CannotApplyIndexOp {
                ref x_ty,
                ref i_ty,
            } => write!(
                f,
                "operator [] cannot be applied to operands of type {} and {}",
                x_ty, i_ty,
            ),
            Error::CannotApplyUnOp { op, ref x_ty } => write!(
                f,
                "operator {} cannot be applied to operand of type {}",
                op, x_ty
            ),
            Error::CannotCallCons { ty_lit, ref xs_ty } => {
                write!(
                    f,
                    "{} constructor cannot be called with arguments of types ",
                    ty_lit
                )?;
                let mut sep = "";
                for x_ty in xs_ty {
                    write!(f, "{}{}", sep, x_ty)?;
                    sep = ", ";
                }
                Ok(())
            },
            Error::CannotCallFn { ident, ref xs_ty } => {
                write!(
                    f,
                    "function {} cannot be called with arguments of types ",
                    ident
                )?;
                let mut sep = "";
                for x_ty in xs_ty {
                    write!(f, "{}{}", sep, x_ty)?;
                    sep = ", ";
                }
                Ok(())
            }
            Error::CannotAssignToAttributeVar { ident } => write!(
                f,
                "cannot assign to attribute variable {}: attributes are immutable",
                ident,
            ),
            Error::CannotAssignToImmutableVar { ident } => write!(
                f,
                "cannot assign to immutable variable {}",
                ident,
            ),
            Error::CannotAssignToUniformVar { ident } => write!(
                f,
                "cannot assign to uniform variable {}: uniforms are immutable",
                ident,
            ),
            Error::CannotInferTyForVar { ident } => write!(
                f,
                "cannot infer type for variable {}",
                ident
            ),
            Error::ExprIsNotConst => write!(f, "expression is not const"),
            Error::FnCannotReadFromVaryings { ident } => {
                write!(f, "function {} cannot read from varyings", ident)
            }
            Error::FnCannotReadFromAndWriteToVaryings { ident } => {
                write!(f, "function {} cannot both read from and write to varyings", ident)
            },
            Error::FnCannotWriteToVaryings { ident } => {
                write!(f, "function {} cannot write to varyings", ident)
            }
            Error::FnHasCyclicDepChain { ident, ref dep_idents} => {
                write!(f, "function {} has cyclic dependency chain ", ident)?;
                let mut sep = "";
                for dep_ident in dep_idents {
                    write!(f, "{}{}", sep, dep_ident)?;
                    sep = ", ";
                }
                Ok(())
            },
            Error::IdentCannotBeRedefined(ident) => write!(
                f,
                "{} cannot be redefined",
                ident
            ),
            Error::IdentIsNotAFn(ident) => write!(f, "{} is not a function", ident),
            Error::IdentIsNotAStruct(ident) => write!(f, "{} is not a struct", ident),
            Error::IdentIsNotAVar(ident) => write!(f, "{} is not a variable", ident),
            Error::IdentIsNotDefined(ident) => write!(f, "{} is not defined", ident),
            Error::InvalidBreakStmt => write!(
                f,
                "invalid break statement"
            ),
            Error::InvalidContinueStmt => write!(
                f,
                "invalid continue statement"
            ),
            Error::InvalidLeftHandSide => write!(f, "invalid left hand side"),
            Error::InvalidReturnTyForFn {
                ident,
                ref ty,
            } => write!(
                f,
                "invalid return type for function {}: expected non-array type, got {}",
                ident, ty
            ),
            Error::InvalidTyForAttributeVar { ident, ref ty } => write!(
                f,
                "invalid type for attribute variable {}: expected either float, vec2, vec3, vec4, mat2, mat3, or mat4, got {}",
                ident,
                ty
            ),
            Error::InvalidTyForVaryingVar { ident, ref ty } => write!(
                f,
                "invalid type for varying variable {}: expected either float, vec2, vec3, vec4, mat2, mat3, mat4, or array of these, got {}",
                ident,
                ty
            ),
            Error::MemberIsNotDefinedOnTy { ident, ref ty } => write!(f, "{} is not defined on type {}", ident, ty),
            Error::MismatchedReturnTyForFn {
                ident,
                ref expected_return_ty,
                ref actual_return_ty,
            } => write!(
                f,
                "mismatched return type for function {}: expected {}, got {}",
                ident, expected_return_ty, actual_return_ty
            ),
            Error::MismatchedTyForArg {
                index,
                fn_ident,
                ref expected_ty,
                ref actual_ty,
            } => write!(
                f,
                "mismatched type for argument {} in call to function {}: expected {}, got {}",
                index, fn_ident, expected_ty, actual_ty
            ),
            Error::MismatchedTyForExpr {
                ref expected_ty,
                ref actual_ty,
            } => write!(
                f,
                "mismatched type for expression: expected {}, got {}",
                expected_ty, actual_ty
            ),
            Error::MissingFn { ident } => write!(f, "missing function {}", ident),
            Error::MissingReturnExpr => write!(f, "missing return expression"),
            Error::StepMustBeNonZero => write!(f, "step must be non-zero"),
            Error::StepMustBePositive => write!(f, "step must be positive"),
            Error::StepMustBeNegative => write!(f, "step must be negative"),
            Error::StructHasCyclicDepChain { ident, ref dep_idents} => {
                write!(f, "struct {} has cyclic dependency chain ", ident)?;
                let mut sep = "";
                for dep_ident in dep_idents {
                    write!(f, "{}{}", sep, dep_ident)?;
                    sep = ", ";
                }
                Ok(())
            },
            Error::TooFewArgsForCall {
                fn_ident,
                expected_count,
                actual_count,
            } => write!(
                f,
                "too few arguments for call to function {}: expected {}, got {}",
                fn_ident, expected_count, actual_count
            ),
            Error::TooManyArgsForCall {
                fn_ident,
                expected_count,
                actual_count,
            } => write!(
                f,
                "too many arguments for call to function {}: expected {}, got {}",
                fn_ident, expected_count, actual_count
            ),
            Error::TooFewCompsForConsCall {
                ty_lit,
                expected_count,
                actual_count,
            } => write!(
                f,
                "too few components for constructor call to {}: expected {}, got {}",
                ty_lit, expected_count, actual_count
            ),
            Error::TooManyCompsForConsCall {
                ty_lit,
                expected_count,
                actual_count,
            } => write!(
                f,
                "too many components for constructor call to {}: expected {}, got {}",
                ty_lit, expected_count, actual_count
            ),
            Error::TooManyParamsForFn {
                ident,
                expected_count,
                actual_count,
            } => write!(
                f,
                "too many parameters for function {}: expected {}, got {}",
                ident, expected_count, actual_count
            ),
            Error::TyLitIsNotACons(ty_lit) => write!(
                f,
                "{} is not a constructor",
                ty_lit,
            ),
        }
    }
}

impl ParsedShader {
    pub fn emit(&self, emitter: &mut Emitter) -> Result<ShaderAttrs, Error> {
        let mut attribute_decls = Vec::new();
        let mut fn_decls_by_ident = HashMap::new();
        let mut struct_decls_by_ident = HashMap::new();
        let mut uniform_decls = Vec::new();
        let mut varying_decls = Vec::new();
        for decl in &self.decls {
            match decl {
                Decl::Attribute(decl) => {
                    attribute_decls.push(decl);
                }
                Decl::Fn(decl) => {
                    fn_decls_by_ident.insert(decl.ident, decl);
                }
                Decl::Struct(decl) => {
                    struct_decls_by_ident.insert(decl.ident, decl);
                }
                Decl::Uniform(decl) => {
                    uniform_decls.push(decl);
                }
                Decl::Varying(decl) => {
                    varying_decls.push(decl);
                }
            }
        }

        let mut emitter = ShaderEmitter::new(emitter, fn_decls_by_ident, struct_decls_by_ident);

        let attribute_decls_attrs = attribute_decls
            .iter()
            .map(|attribute_decl| attribute_decl.emit(&mut emitter))
            .collect::<Result<Vec<_>, _>>()?;

        let mut uniform_decls_attrs_by_block_ident = HashMap::new();
        for uniform_decl in uniform_decls {
            let uniform_decl_attrs = uniform_decl.emit(&mut emitter)?;
            uniform_decls_attrs_by_block_ident
                .entry(uniform_decl_attrs.block_ident)
                .or_insert(Vec::new())
                .push(uniform_decl_attrs);
        }

        let varying_decls_attrs = varying_decls
            .iter()
            .map(|varying_decl| varying_decl.emit(&mut emitter))
            .collect::<Result<Vec<_>, _>>()?;

        let vertex = Ident::new("vertex");
        emitter.active_fn_idents.push(vertex);
        let vertex_fn_decl_attrs = emitter
            .fn_decls_by_ident
            .get(&vertex)
            .ok_or(Error::MissingFn { ident: vertex })?
            .emit(&mut emitter)?;
        emitter.active_fn_idents.pop();
        if vertex_fn_decl_attrs.deps.has_input_varyings {
            return Err(Error::FnCannotReadFromVaryings { ident: vertex });
        }
        emitter.fn_decls_attrs.push(vertex_fn_decl_attrs);

        let vertex_fn_info = match emitter.find_info(vertex).unwrap() {
            Info::Fn(info) => info,
            _ => panic!(),
        };
        if vertex_fn_info.param_tys.len() != 0 {
            return Err(Error::TooManyParamsForFn {
                ident: vertex,
                expected_count: 0,
                actual_count: vertex_fn_info.param_tys.len(),
            });
        }
        if vertex_fn_info.return_ty != Ty::Vec4 {
            return Err(Error::MismatchedReturnTyForFn {
                ident: vertex,
                expected_return_ty: Ty::Vec4,
                actual_return_ty: vertex_fn_info.return_ty.clone(),
            });
        }

        let mut vertex_string = String::new();
        hooks::write_uniform_decls(&mut vertex_string, &uniform_decls_attrs_by_block_ident);
        hooks::write_attribute_decls(&mut vertex_string, &attribute_decls_attrs);
        hooks::write_varying_decls(&mut vertex_string, &varying_decls_attrs);
        for struct_decl_attrs in &emitter.struct_decls_attrs {
            writeln!(vertex_string, "{}", struct_decl_attrs.string).unwrap();
        }
        for fn_decl_attrs in &emitter.fn_decls_attrs {
            if fn_decl_attrs.ident == vertex
                || vertex_fn_info.deps.fn_idents.contains(&fn_decl_attrs.ident)
            {
                writeln!(vertex_string, "{}", fn_decl_attrs.string).unwrap();
            }
        }

        if hooks::should_share_decls() {
            emitter.fn_decls_attrs.clear();
        }

        let fragment = Ident::new("fragment");
        emitter.active_fn_idents.push(fragment);
        let fragment_fn_decl_attrs = emitter
            .fn_decls_by_ident
            .get(&fragment)
            .ok_or(Error::MissingFn { ident: fragment })?
            .emit(&mut emitter)?;
        emitter.active_fn_idents.pop();
        if fragment_fn_decl_attrs.deps.has_output_varyings {
            return Err(Error::FnCannotWriteToVaryings { ident: fragment });
        }
        emitter.fn_decls_attrs.push(fragment_fn_decl_attrs);

        let fragment_fn_info = match emitter.find_info(fragment).unwrap() {
            Info::Fn(info) => info,
            _ => panic!(),
        };
        if fragment_fn_info.param_tys.len() != 0 {
            return Err(Error::TooManyParamsForFn {
                ident: fragment,
                expected_count: 0,
                actual_count: fragment_fn_info.param_tys.len(),
            });
        }
        if fragment_fn_info.return_ty != Ty::Vec4 {
            return Err(Error::MismatchedReturnTyForFn {
                ident: fragment,
                expected_return_ty: Ty::Vec4,
                actual_return_ty: fragment_fn_info.return_ty.clone(),
            });
        }

        let mut fragment_string = String::new();
        if !hooks::should_share_decls() {
            hooks::write_uniform_decls(&mut fragment_string, &uniform_decls_attrs_by_block_ident);
            hooks::write_varying_decls(&mut fragment_string, &varying_decls_attrs);
        }
        for struct_decl_attrs in &emitter.struct_decls_attrs {
            writeln!(fragment_string, "{}", struct_decl_attrs.string).unwrap();
        }
        for fn_decl_attrs in &emitter.fn_decls_attrs {
            if fn_decl_attrs.ident == fragment
                || fragment_fn_info
                    .deps
                    .fn_idents
                    .contains(&fn_decl_attrs.ident)
            {
                writeln!(fragment_string, "{}", fn_decl_attrs.string).unwrap();
            }
        }

        Ok(ShaderAttrs {
            attribute_decls_attrs,
            uniform_decls_attrs_by_block_ident,
            vertex_string,
            fragment_string,
        })
    }
}

impl AttributeDecl {
    fn emit(&self, emitter: &mut ShaderEmitter) -> Result<AttributeDeclAttrs, Error> {
        let mut emitter = DeclEmitter::new(emitter);
        let ty_expr_attrs = self.ty_expr.emit(&mut emitter)?;
        match ty_expr_attrs.ty {
            Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 | Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => {}
            _ => {
                return Err(Error::InvalidTyForAttributeVar {
                    ident: self.ident,
                    ty: ty_expr_attrs.ty.clone(),
                })
            }
        };
        emitter.scope_mut().insert(
            self.ident,
            Info::Var(VarInfo {
                ty: ty_expr_attrs.ty.clone(),
                kind: VarInfoKind::Attribute,
            }),
        );
        Ok(AttributeDeclAttrs {
            ident: self.ident,
            ty: ty_expr_attrs.ty,
        })
    }
}

impl FnDecl {
    fn emit(&self, emitter: &mut ShaderEmitter) -> Result<FnDeclAttrs, Error> {
        let mut emitter = DeclEmitter::new(emitter);
        let return_ty_expr_attrs = self
            .return_ty_expr
            .as_ref()
            .map(|return_ty_expr| return_ty_expr.emit(&mut emitter))
            .transpose()?;
        let return_ty = return_ty_expr_attrs
            .as_ref()
            .map(|return_ty_expr_attrs| &return_ty_expr_attrs.ty)
            .unwrap_or(&Ty::Void);
        emitter.scopes.push(Scope::default());
        emitter.return_ty = return_ty.clone();
        let params_attrs = self
            .params
            .iter()
            .map(|param| param.emit(&mut emitter))
            .collect::<Result<Vec<_>, _>>()?;
        let block_attrs = self.block.emit(&mut emitter)?;
        if block_attrs.deps.has_input_varyings && block_attrs.deps.has_output_varyings {
            return Err(Error::FnCannotReadFromAndWriteToVaryings { ident: self.ident });
        }
        emitter.return_ty = Ty::Void;
        emitter.scopes.pop();
        if !emitter.scope_mut().insert(
            self.ident,
            Info::Fn(FnInfo {
                param_tys: params_attrs
                    .iter()
                    .map(|param_attrs| param_attrs.ty.clone())
                    .collect(),
                return_ty: return_ty.clone(),
                deps: block_attrs.deps.clone(),
            }),
        ) {
            return Err(Error::IdentCannotBeRedefined(self.ident));
        }
        Ok(FnDeclAttrs {
            ident: self.ident,
            deps: block_attrs.deps.clone(),
            string: {
                let mut string = String::new();
                write_ident_and_ty(
                    &mut string,
                    self.ident,
                    return_ty_expr_attrs
                        .as_ref()
                        .map(|return_ty_expr_attrs| &return_ty_expr_attrs.ty)
                        .unwrap_or(&Ty::Void),
                );
                write!(string, "(").unwrap();
                hooks::write_params(
                    &mut string,
                    &params_attrs,
                    &block_attrs.deps.uniform_block_idents,
                    block_attrs.deps.has_attributes,
                    block_attrs.deps.has_input_varyings,
                    block_attrs.deps.has_output_varyings,
                );
                write!(string, ") {}", block_attrs.string).unwrap();
                string
            },
        })
    }
}

impl Param {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ParamAttrs, Error> {
        let ty_expr_attrs = self.ty_expr.emit(emitter)?;
        if !emitter.scope_mut().insert(
            self.ident,
            Info::Var(VarInfo {
                ty: ty_expr_attrs.ty.clone(),
                kind: VarInfoKind::Local { is_mut: false },
            }),
        ) {
            return Err(Error::IdentCannotBeRedefined(self.ident));
        }
        Ok(ParamAttrs {
            ty: ty_expr_attrs.ty.clone(),
            string: {
                let mut string = String::new();
                write_ident_and_ty(&mut string, self.ident, &ty_expr_attrs.ty);
                string
            },
        })
    }
}

impl StructDecl {
    fn emit(&self, emitter: &mut ShaderEmitter) -> Result<StructDeclAttrs, Error> {
        let mut emitter = DeclEmitter::new(emitter);
        let members = self
            .members
            .iter()
            .map(|member| member.emit(&mut emitter))
            .collect::<Result<Vec<_>, _>>()?;
        if !emitter.scope_mut().insert(
            self.ident,
            Info::Struct(StructInfo {
                contains_arrays: members.iter().any(|member| member.contains_arrays),
                member_tys_by_ident: members
                    .iter()
                    .map(|member| (member.ident, member.ty.clone()))
                    .collect(),
            }),
        ) {
            return Err(Error::IdentCannotBeRedefined(self.ident));
        }
        Ok(StructDeclAttrs {
            string: {
                let mut string = format!("struct {} {{", self.ident);
                if !members.is_empty() {
                    writeln!(string).unwrap();
                    for member in &members {
                        for line in member.string.lines() {
                            writeln!(string, "    {}", line).unwrap();
                        }
                    }
                }
                write!(string, "}};").unwrap();
                string
            },
        })
    }
}

impl Member {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<MemberAttrs, Error> {
        let ty_expr = self.ty_expr.emit(emitter)?;
        Ok(MemberAttrs {
            ident: self.ident,
            contains_arrays: ty_expr.contains_arrays,
            ty: ty_expr.ty.clone(),
            string: {
                let mut string = String::new();
                write_ident_and_ty(&mut string, self.ident, &ty_expr.ty);
                string
            },
        })
    }
}

impl UniformDecl {
    fn emit(&self, emitter: &mut ShaderEmitter) -> Result<UniformDeclAttrs, Error> {
        let mut emitter = DeclEmitter::new(emitter);
        let ty_expr_attrs = self.ty_expr.emit(&mut emitter)?;
        let block_ident = self.block_ident.unwrap_or(Ident::new("default"));
        emitter.scope_mut().insert(
            self.ident,
            Info::Var(VarInfo {
                ty: ty_expr_attrs.ty.clone(),
                kind: VarInfoKind::Uniform { block_ident },
            }),
        );
        Ok(UniformDeclAttrs {
            ident: self.ident,
            ty: ty_expr_attrs.ty,
            block_ident,
        })
    }
}

impl VaryingDecl {
    fn emit(&self, emitter: &mut ShaderEmitter) -> Result<VaryingDeclAttrs, Error> {
        let mut emitter = DeclEmitter::new(emitter);
        let ty_expr_attrs = self.ty_expr.emit(&mut emitter)?;
        if !match ty_expr_attrs.ty {
            Ty::Bool
            | Ty::Int
            | Ty::Float
            | Ty::Bvec2
            | Ty::Bvec3
            | Ty::Bvec4
            | Ty::Ivec2
            | Ty::Ivec3
            | Ty::Ivec4
            | Ty::Vec2
            | Ty::Vec3
            | Ty::Vec4 => true,
            Ty::Array { ref elem_ty, .. } => match **elem_ty {
                Ty::Bool
                | Ty::Int
                | Ty::Float
                | Ty::Bvec2
                | Ty::Bvec3
                | Ty::Bvec4
                | Ty::Ivec2
                | Ty::Ivec3
                | Ty::Ivec4
                | Ty::Vec2
                | Ty::Vec3
                | Ty::Vec4 => true,
                _ => false,
            },
            _ => false,
        } {
            return Err(Error::InvalidTyForVaryingVar {
                ident: self.ident,
                ty: ty_expr_attrs.ty.clone(),
            });
        };
        emitter.scope_mut().insert(
            self.ident,
            Info::Var(VarInfo {
                ty: ty_expr_attrs.ty.clone(),
                kind: VarInfoKind::Varying,
            }),
        );
        Ok(VaryingDeclAttrs {
            ident: self.ident,
            ty: ty_expr_attrs.ty,
        })
    }
}

impl Block {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<BlockAttrs, Error> {
        let stmts_attrs = self
            .stmts
            .iter()
            .map(|stmt| stmt.emit(emitter))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(if stmts_attrs.len() == 1 {
            if let Some(stmt_attrs) = &stmts_attrs[0] {
                if stmt_attrs.is_block {
                    Some(BlockAttrs {
                        is_empty: false,
                        deps: stmt_attrs.deps.clone(),
                        string: stmt_attrs.string.clone(),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
        .unwrap_or_else(|| BlockAttrs {
            is_empty: stmts_attrs.iter().all(|stmt_attrs| stmt_attrs.is_none()),
            deps: {
                let mut deps = Deps::default();
                for stmt_attrs in &stmts_attrs {
                    if let Some(stmt_attrs) = stmt_attrs {
                        deps = deps.union(&stmt_attrs.deps);
                    }
                }
                deps
            },
            string: {
                let mut string = "{".to_string();
                if !stmts_attrs.is_empty() {
                    writeln!(string).unwrap();
                    for stmt_attrs in &stmts_attrs {
                        if let Some(stmt_attrs) = stmt_attrs {
                            for line in stmt_attrs.string.lines() {
                                writeln!(string, "    {}", line).unwrap();
                            }
                        }
                    }
                }
                write!(string, "}}").unwrap();
                string
            },
        }))
    }
}

impl Stmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        match self {
            Stmt::Break(stmt) => stmt.emit(emitter),
            Stmt::Continue(stmt) => stmt.emit(emitter),
            Stmt::For(stmt) => stmt.emit(emitter),
            Stmt::If(stmt) => stmt.emit(emitter),
            Stmt::Let(stmt) => stmt.emit(emitter),
            Stmt::Return(stmt) => stmt.emit(emitter),
            Stmt::Block(stmt) => stmt.emit(emitter),
            Stmt::Expr(stmt) => stmt.emit(emitter),
        }
    }
}

impl BreakStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        if !emitter.is_in_for_stmt_block {
            return Err(Error::InvalidBreakStmt);
        }
        Ok(Some(StmtAttrs {
            is_block: false,
            deps: Deps::default(),
            string: "break;".to_string(),
        }))
    }
}

impl ContinueStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        if !emitter.is_in_for_stmt_block {
            return Err(Error::InvalidContinueStmt);
        }
        Ok(Some(StmtAttrs {
            is_block: false,
            deps: Deps::default(),
            string: "continue;".to_string(),
        }))
    }
}

impl ForStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        let from_expr_attrs = self.from_expr.emit(emitter)?;
        let to_expr_attrs = self.to_expr.emit(emitter)?;
        let step_expr_attrs = self
            .step_expr
            .as_ref()
            .map(|step_expr| step_expr.emit(emitter))
            .transpose()?;
        emitter.scopes.push(Scope::default());
        if !emitter.scope_mut().insert(
            self.ident,
            Info::Var(VarInfo {
                ty: Ty::Int,
                kind: VarInfoKind::Local { is_mut: false },
            }),
        ) {
            return Err(Error::IdentCannotBeRedefined(self.ident));
        }
        let is_in_for_stmt_block = emitter.is_in_for_stmt_block;
        emitter.is_in_for_stmt_block = true;
        let block_attrs = self.block.emit(emitter)?;
        emitter.is_in_for_stmt_block = is_in_for_stmt_block;
        emitter.scopes.pop();
        if from_expr_attrs.ty != Ty::Int {
            return Err(Error::MismatchedTyForExpr {
                expected_ty: Ty::Int,
                actual_ty: from_expr_attrs.ty,
            });
        }
        if to_expr_attrs.ty != Ty::Int {
            return Err(Error::MismatchedTyForExpr {
                expected_ty: Ty::Int,
                actual_ty: to_expr_attrs.ty,
            });
        }
        if let Some(step_expr_attrs) = &step_expr_attrs {
            if step_expr_attrs.ty != Ty::Int {
                return Err(Error::MismatchedTyForExpr {
                    expected_ty: Ty::Int,
                    actual_ty: step_expr_attrs.ty.clone(),
                });
            }
        }
        let from = from_expr_attrs
            .value_or_string
            .to_value()
            .ok_or(Error::ExprIsNotConst)?
            .to_int()
            .unwrap();
        let to = to_expr_attrs
            .value_or_string
            .to_value()
            .ok_or(Error::ExprIsNotConst)?
            .to_int()
            .unwrap();
        let step = if let Some(step_expr_attrs) = &step_expr_attrs {
            let step = step_expr_attrs
                .value_or_string
                .to_value()
                .ok_or(Error::ExprIsNotConst)?
                .to_int()
                .unwrap();
            if step == 0 {
                return Err(Error::StepMustBeNonZero);
            }
            if from < to {
                if step < 0 {
                    return Err(Error::StepMustBePositive);
                }
            } else if from > to {
                if step > 0 {
                    return Err(Error::StepMustBeNegative);
                }
            }
            step
        } else {
            if from < to {
                1
            } else {
                -1
            }
        };
        Ok(if from == to {
            None
        } else {
            Some(StmtAttrs {
                is_block: false,
                deps: block_attrs.deps,
                string: {
                    format!(
                        "for (int {0} = {1}; {0} {2} {3}; {0} {4} {5}) {6}",
                        self.ident,
                        from,
                        if from < to { "<" } else { ">" },
                        to,
                        if step > 0 { "+=" } else { "-=" },
                        step,
                        block_attrs.string
                    )
                },
            })
        })
    }
}

impl IfStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        let expr_attrs = self.expr.emit(emitter)?;
        let block_if_true_attrs = self.block_if_true.emit(emitter)?;
        let block_if_false_attrs = self
            .block_if_false
            .as_ref()
            .map(|block_if_false| block_if_false.emit(emitter))
            .transpose()?;
        if expr_attrs.ty != Ty::Bool {
            return Err(Error::MismatchedTyForExpr {
                expected_ty: Ty::Bool,
                actual_ty: expr_attrs.ty.clone(),
            });
        }
        Ok(match expr_attrs.value_or_string {
            ValueOrString::Value(value) => if value.to_bool().unwrap() {
                Some(block_if_true_attrs)
            } else {
                block_if_false_attrs
            }
            .and_then(|block_attrs| {
                if block_attrs.is_empty {
                    None
                } else {
                    Some(StmtAttrs {
                        is_block: true,
                        deps: block_attrs.deps,
                        string: block_attrs.string,
                    })
                }
            }),
            ValueOrString::String(string) => Some(StmtAttrs {
                is_block: false,
                deps: {
                    let mut deps = expr_attrs.deps.union(&block_if_true_attrs.deps);
                    if let Some(block_if_false_attrs) = &block_if_false_attrs {
                        deps = deps.union(&block_if_false_attrs.deps);
                    }
                    deps
                },
                string: {
                    let mut string = format!("if ({}) {}", string, block_if_true_attrs.string);
                    if let Some(block_if_false_attrs) = &block_if_false_attrs {
                        write!(string, " else {}", block_if_false_attrs.string).unwrap()
                    }
                    string
                },
            }),
        })
    }
}

impl LetStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        let ty_expr_attrs = self
            .ty_expr
            .as_ref()
            .map(|ty_expr| ty_expr.emit(emitter))
            .transpose()?;
        let expr_attrs = self
            .expr
            .as_ref()
            .map(|expr| expr.emit(emitter))
            .transpose()?;
        let ty = ty_expr_attrs
            .as_ref()
            .map(|ty_expr_attrs| &ty_expr_attrs.ty)
            .or_else(|| expr_attrs.as_ref().map(|expr_attrs| &expr_attrs.ty))
            .ok_or(Error::CannotInferTyForVar { ident: self.ident })?
            .clone();
        if let Some(expr_attrs) = &expr_attrs {
            if expr_attrs.ty != ty {
                return Err(Error::MismatchedTyForExpr {
                    expected_ty: ty.clone(),
                    actual_ty: expr_attrs.ty.clone(),
                });
            }
        }
        if !emitter.scope_mut().insert(
            self.ident,
            Info::Var(VarInfo {
                ty: ty.clone(),
                kind: VarInfoKind::Local { is_mut: true },
            }),
        ) {
            return Err(Error::IdentCannotBeRedefined(self.ident));
        }
        Ok(Some(StmtAttrs {
            is_block: false,
            deps: expr_attrs
                .as_ref()
                .map(|expr_attrs| expr_attrs.deps.clone())
                .unwrap_or_default(),
            string: {
                let mut string = String::new();
                write_ident_and_ty(&mut string, self.ident, &ty);
                if let Some(expr_attrs) = &expr_attrs {
                    write!(string, " = {}", expr_attrs.value_or_string).unwrap();
                }
                write!(string, ";").unwrap();
                string
            },
        }))
    }
}

impl ReturnStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        let expr_attrs = self
            .expr
            .as_ref()
            .map(|expr_attrs| expr_attrs.emit(emitter))
            .transpose()?;
        match &expr_attrs {
            Some(expr_attrs) => {
                if expr_attrs.ty != emitter.return_ty {
                    return Err(Error::MismatchedTyForExpr {
                        expected_ty: emitter.return_ty.clone(),
                        actual_ty: expr_attrs.ty.clone(),
                    });
                }
            }
            None => {
                if emitter.return_ty != Ty::Void {
                    return Err(Error::MissingReturnExpr);
                }
            }
        }
        Ok(Some(StmtAttrs {
            is_block: false,
            deps: expr_attrs
                .as_ref()
                .map(|expr_attrs| expr_attrs.deps.clone())
                .unwrap_or_default(),
            string: {
                let mut string = "return".to_string();
                if let Some(expr_attrs) = &expr_attrs {
                    write!(string, " {}", expr_attrs.value_or_string).unwrap();
                }
                write!(string, ";").unwrap();
                string
            },
        }))
    }
}

impl BlockStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        emitter.scopes.push(Scope::default());
        let block_attrs = self.block.emit(emitter)?;
        emitter.scopes.pop();
        Ok(if block_attrs.is_empty {
            None
        } else {
            Some(StmtAttrs {
                is_block: true,
                deps: block_attrs.deps,
                string: block_attrs.string,
            })
        })
    }
}

impl ExprStmt {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<Option<StmtAttrs>, Error> {
        let expr_attrs = self.expr.emit(emitter)?;
        Ok(Some(StmtAttrs {
            is_block: false,
            deps: expr_attrs.deps,
            string: format!("{};", expr_attrs.value_or_string),
        }))
    }
}

impl TyExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<TyExprAttrs, Error> {
        match self {
            TyExpr::Array(ty_expr) => ty_expr.emit(emitter),
            TyExpr::Struct(ty_expr) => ty_expr.emit(emitter),
            TyExpr::TyLit(ty_lit) => ty_lit.emit(emitter),
        }
    }
}

impl ArrayTyExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<TyExprAttrs, Error> {
        let elem_ty_expr_attrs = self.elem_ty_expr.emit(emitter)?;
        Ok(TyExprAttrs {
            contains_arrays: true,
            ty: Ty::Array {
                elem_ty: Rc::new(elem_ty_expr_attrs.ty),
                len: self.len as usize,
            },
        })
    }
}

impl StructTyExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<TyExprAttrs, Error> {
        let info = match emitter.find_info(self.ident) {
            Some(info) => Some(info),
            None => {
                if let Some(struct_decl) = emitter.parent.struct_decls_by_ident.get(&self.ident) {
                    if let Some(index) = emitter
                        .parent
                        .active_struct_idents
                        .iter()
                        .position(|&ident| ident == self.ident)
                    {
                        return Err(Error::StructHasCyclicDepChain {
                            ident: self.ident,
                            dep_idents: {
                                let mut dep_idents =
                                    emitter.parent.active_struct_idents[index..].to_owned();
                                dep_idents.push(self.ident);
                                dep_idents
                            },
                        });
                    }
                    emitter.parent.active_struct_idents.push(self.ident);
                    let struct_decl_attrs = struct_decl.emit(&mut emitter.parent)?;
                    emitter.parent.active_struct_idents.pop();
                    emitter.parent.struct_decls_attrs.push(struct_decl_attrs);
                }
                emitter.find_info(self.ident)
            }
        }
        .ok_or(Error::IdentIsNotDefined(self.ident))?;
        match *info {
            Info::Struct(StructInfo {
                contains_arrays, ..
            }) => Ok(TyExprAttrs {
                contains_arrays,
                ty: Ty::Struct { ident: self.ident },
            }),
            _ => Err(Error::IdentIsNotAStruct(self.ident)),
        }
    }
}

impl TyLit {
    fn emit(self, _emitter: &mut DeclEmitter) -> Result<TyExprAttrs, Error> {
        Ok(TyExprAttrs {
            contains_arrays: false,
            ty: self.to_ty(),
        })
    }
}

impl Expr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        match self {
            Expr::Cond(expr) => expr.emit(emitter),
            Expr::Bin(expr) => expr.emit(emitter),
            Expr::Un(expr) => expr.emit(emitter),
            Expr::Index(expr) => expr.emit(emitter),
            Expr::Member(expr) => expr.emit(emitter),
            Expr::Call(expr) => expr.emit(emitter),
            Expr::ConsCall(expr) => expr.emit(emitter),
            Expr::Var(expr) => expr.emit(emitter),
            Expr::Lit(lit) => lit.emit(emitter),
        }
    }
}

impl CondExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        if emitter.is_in_lvalue_context {
            return Err(Error::InvalidLeftHandSide);
        }
        let x_attrs = self.x.emit(emitter)?;
        let y_attrs = self.y.emit(emitter)?;
        let z_attrs = self.z.emit(emitter)?;
        Ok(ExprAttrs {
            ty: {
                if x_attrs.ty != Ty::Bool {
                    return Err(Error::MismatchedTyForExpr {
                        expected_ty: Ty::Bool,
                        actual_ty: x_attrs.ty.clone(),
                    });
                }
                if z_attrs.ty != y_attrs.ty {
                    return Err(Error::MismatchedTyForExpr {
                        expected_ty: y_attrs.ty.clone(),
                        actual_ty: z_attrs.ty.clone(),
                    });
                }
                y_attrs.ty
            },
            deps: match &x_attrs.value_or_string {
                ValueOrString::Value(x) => {
                    if x.to_bool().unwrap() {
                        x_attrs.deps.union(&y_attrs.deps)
                    } else {
                        x_attrs.deps.union(&z_attrs.deps)
                    }
                }
                _ => x_attrs.deps.union(&y_attrs.deps).union(&z_attrs.deps),
            },
            value_or_string: match &x_attrs.value_or_string {
                ValueOrString::Value(x) => {
                    if x.to_bool().unwrap() {
                        y_attrs.value_or_string
                    } else {
                        z_attrs.value_or_string
                    }
                }
                _ => format!(
                    "{} ? {} : {}",
                    x_attrs.value_or_string, y_attrs.value_or_string, z_attrs.value_or_string
                )
                .into(),
            },
        })
    }
}

impl BinExpr {
    #[allow(clippy::float_cmp)]
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        if emitter.is_in_lvalue_context {
            return Err(Error::InvalidLeftHandSide);
        }
        let x_attrs = match self.op {
            BinOp::Assign
            | BinOp::AddAssign
            | BinOp::SubAssign
            | BinOp::MulAssign
            | BinOp::DivAssign => {
                let is_in_lvalue_context = emitter.is_in_lvalue_context;
                emitter.is_in_lvalue_context = true;
                let x_attrs = self.x.emit(emitter)?;
                emitter.is_in_lvalue_context = is_in_lvalue_context;
                x_attrs
            }
            _ => self.x.emit(emitter)?,
        };
        let y_attrs = self.y.emit(emitter)?;
        Ok(ExprAttrs {
            ty: match self.op {
                BinOp::Assign => {
                    if x_attrs.ty == y_attrs.ty {
                        Some(x_attrs.ty.clone())
                    } else {
                        None
                    }
                }
                BinOp::AddAssign | BinOp::SubAssign | BinOp::DivAssign => {
                    match (&x_attrs.ty, &y_attrs.ty) {
                        (Ty::Int, Ty::Int) => Some(Ty::Int),
                        (Ty::Float, Ty::Float) => Some(Ty::Float),
                        (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                        (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                        (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                        (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                        (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                        (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                        (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                        (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                        (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                        (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                        (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                        (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                        (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                        (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                        (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                        (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                        (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                        (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                        _ => None,
                    }
                }
                BinOp::MulAssign => match (&x_attrs.ty, &y_attrs.ty) {
                    (Ty::Int, Ty::Int) => Some(Ty::Int),
                    (Ty::Float, Ty::Float) => Some(Ty::Float),
                    (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                    (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                    (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                    (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                    (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                    (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                    (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                    (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                    (Ty::Vec2, Ty::Mat2) => Some(Ty::Vec2),
                    (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                    (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                    (Ty::Vec3, Ty::Mat3) => Some(Ty::Vec3),
                    (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                    (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                    (Ty::Vec4, Ty::Mat4) => Some(Ty::Vec4),
                    (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                    (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                    (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                    (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                    (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                    (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                    _ => None,
                },
                BinOp::Or | BinOp::And => match (&x_attrs.ty, &y_attrs.ty) {
                    (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
                    _ => None,
                },
                BinOp::Eq | BinOp::Ne => match (&x_attrs.ty, &y_attrs.ty) {
                    (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
                    (Ty::Int, Ty::Int) => Some(Ty::Bool),
                    (Ty::Float, Ty::Float) => Some(Ty::Bool),
                    (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Bvec2),
                    (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Bvec3),
                    (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Bvec4),
                    (Ty::Vec2, Ty::Vec2) => Some(Ty::Bvec2),
                    (Ty::Vec3, Ty::Vec3) => Some(Ty::Bvec3),
                    (Ty::Vec4, Ty::Vec4) => Some(Ty::Bvec4),
                    _ => None,
                },
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match (&x_attrs.ty, &y_attrs.ty) {
                    (Ty::Int, Ty::Int) => Some(Ty::Bool),
                    (Ty::Float, Ty::Float) => Some(Ty::Bool),
                    (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Bvec2),
                    (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Bvec3),
                    (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Bvec4),
                    (Ty::Vec2, Ty::Vec2) => Some(Ty::Bvec2),
                    (Ty::Vec3, Ty::Vec3) => Some(Ty::Bvec3),
                    (Ty::Vec4, Ty::Vec4) => Some(Ty::Bvec4),
                    _ => None,
                },
                BinOp::Add | BinOp::Sub | BinOp::Div => match (&x_attrs.ty, &y_attrs.ty) {
                    (Ty::Int, Ty::Int) => Some(Ty::Int),
                    (Ty::Float, Ty::Float) => Some(Ty::Float),
                    (Ty::Float, Ty::Vec2) => Some(Ty::Vec2),
                    (Ty::Float, Ty::Vec3) => Some(Ty::Vec3),
                    (Ty::Float, Ty::Vec4) => Some(Ty::Vec4),
                    (Ty::Float, Ty::Mat2) => Some(Ty::Mat2),
                    (Ty::Float, Ty::Mat3) => Some(Ty::Mat3),
                    (Ty::Float, Ty::Mat4) => Some(Ty::Mat4),
                    (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                    (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                    (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                    (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                    (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                    (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                    (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                    (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                    (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                    (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                    (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                    (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                    (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                    (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                    (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                    (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                    (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                    (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                    _ => None,
                },
                BinOp::Mul => match (&x_attrs.ty, &y_attrs.ty) {
                    (Ty::Int, Ty::Int) => Some(Ty::Int),
                    (Ty::Float, Ty::Float) => Some(Ty::Float),
                    (Ty::Float, Ty::Vec2) => Some(Ty::Vec2),
                    (Ty::Float, Ty::Vec3) => Some(Ty::Vec3),
                    (Ty::Float, Ty::Vec4) => Some(Ty::Vec4),
                    (Ty::Float, Ty::Mat2) => Some(Ty::Mat2),
                    (Ty::Float, Ty::Mat3) => Some(Ty::Mat3),
                    (Ty::Float, Ty::Mat4) => Some(Ty::Mat4),
                    (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                    (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                    (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                    (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                    (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                    (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                    (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                    (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                    (Ty::Vec2, Ty::Mat2) => Some(Ty::Vec2),
                    (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                    (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                    (Ty::Vec3, Ty::Mat3) => Some(Ty::Vec3),
                    (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                    (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                    (Ty::Vec4, Ty::Mat4) => Some(Ty::Vec4),
                    (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                    (Ty::Mat2, Ty::Vec2) => Some(Ty::Vec2),
                    (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                    (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                    (Ty::Mat3, Ty::Vec3) => Some(Ty::Vec3),
                    (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                    (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                    (Ty::Mat4, Ty::Vec4) => Some(Ty::Vec4),
                    (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                    _ => None,
                },
            }
            .ok_or(Error::CannotApplyBinOp {
                op: self.op,
                x_ty: x_attrs.ty,
                y_ty: y_attrs.ty,
            })?,
            deps: x_attrs.deps.union(&y_attrs.deps),
            value_or_string: {
                match self.op {
                    BinOp::Or => match &x_attrs.value_or_string {
                        ValueOrString::Value(x) => Some(if x.to_bool().unwrap() {
                            Value::Bool(true).into()
                        } else {
                            y_attrs.value_or_string.clone()
                        }),
                        _ => None,
                    },
                    BinOp::And => match &x_attrs.value_or_string {
                        ValueOrString::Value(x) => Some(if x.to_bool().unwrap() {
                            y_attrs.value_or_string.clone()
                        } else {
                            Value::Bool(false).into()
                        }),
                        _ => None,
                    },
                    BinOp::Eq => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Bool(x), Value::Bool(y)) => Some(Value::Bool(x == y)),
                            (Value::Int(x), Value::Int(y)) => Some(Value::Bool(x == y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Bool(x == y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Ne => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Bool(x), Value::Bool(y)) => Some(Value::Bool(x == y)),
                            (Value::Int(x), Value::Int(y)) => Some(Value::Bool(x != y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Bool(x != y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Lt => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Bool(x < y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Bool(x < y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Le => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Bool(x <= y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Bool(x <= y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Gt => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Bool(x > y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Bool(x > y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Ge => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Bool(x >= y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Bool(x >= y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Add => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Int(x + y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Float(x + y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Sub => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Int(x - y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Float(x - y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Mul => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Int(x * y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Float(x * y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    BinOp::Div => match (&x_attrs.value_or_string, &y_attrs.value_or_string) {
                        (ValueOrString::Value(x), ValueOrString::Value(y)) => match (x, y) {
                            (Value::Int(x), Value::Int(y)) => Some(Value::Int(x / y)),
                            (Value::Float(x), Value::Float(y)) => Some(Value::Float(x / y)),
                            _ => None,
                        },
                        _ => None,
                    }
                    .map(ValueOrString::Value),
                    _ => None,
                }
                .unwrap_or_else({
                    let x = x_attrs.value_or_string;
                    let y = y_attrs.value_or_string;
                    move || format!("({} {} {})", x, self.op, y).into()
                })
            },
        })
    }
}

impl UnExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        if emitter.is_in_lvalue_context {
            return Err(Error::InvalidLeftHandSide);
        }
        let x_attrs = self.x.emit(emitter)?;
        Ok(ExprAttrs {
            ty: match self.op {
                UnOp::Not => match &x_attrs.ty {
                    Ty::Bool => Some(Ty::Bool),
                    _ => None,
                },
                UnOp::Neg => match &x_attrs.ty {
                    Ty::Int => Some(Ty::Int),
                    Ty::Float => Some(Ty::Float),
                    Ty::Ivec2 => Some(Ty::Ivec2),
                    Ty::Ivec3 => Some(Ty::Ivec3),
                    Ty::Ivec4 => Some(Ty::Ivec4),
                    Ty::Vec2 => Some(Ty::Vec2),
                    Ty::Vec3 => Some(Ty::Vec3),
                    Ty::Vec4 => Some(Ty::Vec4),
                    Ty::Mat2 => Some(Ty::Mat2),
                    Ty::Mat3 => Some(Ty::Mat3),
                    Ty::Mat4 => Some(Ty::Mat4),
                    _ => None,
                },
            }
            .ok_or(Error::CannotApplyUnOp {
                op: self.op,
                x_ty: x_attrs.ty,
            })?,
            deps: x_attrs.deps,
            value_or_string: {
                match self.op {
                    UnOp::Not => match &x_attrs.value_or_string {
                        ValueOrString::Value(x) => match x {
                            Value::Bool(x) => Some(Value::Bool(!x)),
                            _ => None,
                        },
                        _ => None,
                    },
                    UnOp::Neg => match &x_attrs.value_or_string {
                        ValueOrString::Value(x) => match x {
                            Value::Int(x) => Some(Value::Int(-x)),
                            Value::Float(x) => Some(Value::Float(-x)),
                            _ => None,
                        },
                        _ => None,
                    },
                }
                .map(|value| value.into())
                .unwrap_or_else({
                    let x = x_attrs.value_or_string;
                    move || format!("{}{}", self.op, x).into()
                })
            },
        })
    }
}

impl IndexExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        let x_attrs = self.x.emit(emitter)?;
        let is_in_lvalue_context = emitter.is_in_lvalue_context;
        emitter.is_in_lvalue_context = false;
        let i_attrs = self.i.emit(emitter)?;
        emitter.is_in_lvalue_context = is_in_lvalue_context;
        Ok(ExprAttrs {
            ty: {
                if !(match &x_attrs.ty {
                    Ty::Bvec2
                    | Ty::Bvec3
                    | Ty::Bvec4
                    | Ty::Ivec2
                    | Ty::Ivec3
                    | Ty::Ivec4
                    | Ty::Vec2
                    | Ty::Vec3
                    | Ty::Vec4
                    | Ty::Mat2
                    | Ty::Mat3
                    | Ty::Mat4
                    | Ty::Array { .. } => true,
                    _ => false,
                }) || i_attrs.ty != Ty::Int
                {
                    return Err(Error::CannotApplyIndexOp {
                        x_ty: x_attrs.ty.clone(),
                        i_ty: i_attrs.ty.clone(),
                    });
                }
                x_attrs.ty.elem_ty().unwrap()
            },
            deps: x_attrs.deps.union(&i_attrs.deps),
            value_or_string: format!("{}[{}]", x_attrs.value_or_string, i_attrs.value_or_string)
                .into(),
        })
    }
}

impl MemberExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        let x_attrs = self.x.emit(emitter)?;
        Ok(ExprAttrs {
            ty: match x_attrs.ty {
                Ty::Struct { ident } => match emitter.find_info(ident).unwrap() {
                    Info::Struct(info) => info,
                    _ => panic!(),
                }
                .member_tys_by_ident
                .get(&self.ident)
                .cloned(),
                _ => None,
            }
            .map(Ok)
            .or_else(|| {
                match &x_attrs.ty {
                    Ty::Bvec2
                    | Ty::Bvec3
                    | Ty::Bvec4
                    | Ty::Ivec2
                    | Ty::Ivec3
                    | Ty::Ivec4
                    | Ty::Vec2
                    | Ty::Vec3
                    | Ty::Vec4 => {}
                    _ => return None,
                };
                let swizzle = Swizzle::parse(self.ident)?;
                let len = x_attrs.ty.len().unwrap();
                for index in &swizzle {
                    if index >= len {
                        return None;
                    }
                }
                if swizzle.has_dups() && emitter.is_in_lvalue_context {
                    return Some(Err(Error::InvalidLeftHandSide));
                }
                Some(Ok(match (x_attrs.ty.elem_ty().unwrap(), swizzle.len()) {
                    (Ty::Bool, 1) => Ty::Bool,
                    (Ty::Bool, 2) => Ty::Bvec2,
                    (Ty::Bool, 3) => Ty::Bvec3,
                    (Ty::Bool, 4) => Ty::Bvec4,
                    (Ty::Int, 1) => Ty::Int,
                    (Ty::Int, 2) => Ty::Ivec2,
                    (Ty::Int, 3) => Ty::Ivec3,
                    (Ty::Int, 4) => Ty::Ivec4,
                    (Ty::Float, 1) => Ty::Float,
                    (Ty::Float, 2) => Ty::Vec2,
                    (Ty::Float, 3) => Ty::Vec3,
                    (Ty::Float, 4) => Ty::Vec4,
                    _ => panic!(),
                }))
            })
            .transpose()?
            .ok_or(Error::MemberIsNotDefinedOnTy {
                ident: self.ident,
                ty: x_attrs.ty.clone(),
            })?,
            deps: x_attrs.deps,
            value_or_string: format!("{}.{}", x_attrs.value_or_string, self.ident).into(),
        })
    }
}

impl CallExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        if emitter.is_in_lvalue_context {
            return Err(Error::InvalidLeftHandSide);
        }
        let xs_attrs = self
            .xs
            .iter()
            .map(|x| x.emit(emitter))
            .collect::<Result<Vec<_>, _>>()?;
        let info = match emitter.find_info(self.ident) {
            Some(info) => info,
            None => {
                if let Some(fn_decl) = emitter.parent.fn_decls_by_ident.get(&self.ident) {
                    if let Some(index) = emitter
                        .parent
                        .active_fn_idents
                        .iter()
                        .position(|&ident| ident == self.ident)
                    {
                        return Err(Error::FnHasCyclicDepChain {
                            ident: self.ident,
                            dep_idents: {
                                let mut dep_idents =
                                    emitter.parent.active_fn_idents[index..].to_owned();
                                dep_idents.push(self.ident);
                                dep_idents
                            },
                        });
                    }
                    emitter.parent.active_fn_idents.push(self.ident);
                    let fn_decl_attrs = fn_decl.emit(&mut emitter.parent)?;
                    emitter.parent.active_fn_idents.pop();
                    emitter.parent.fn_decls_attrs.push(fn_decl_attrs);
                }
                emitter
                    .find_info(self.ident)
                    .ok_or(Error::IdentIsNotDefined(self.ident))?
            }
        };
        Ok(match info {
            Info::Builtin(info) => ExprAttrs {
                ty: info
                    .return_tys_by_param_tys
                    .get(
                        &xs_attrs
                            .iter()
                            .map(|x_attrs| x_attrs.ty.clone())
                            .collect::<Vec<_>>(),
                    )
                    .ok_or(Error::CannotCallFn {
                        ident: self.ident,
                        xs_ty: xs_attrs.iter().map(|x_attrs| x_attrs.ty.clone()).collect(),
                    })?
                    .clone(),
                deps: {
                    let mut deps = Deps::default();
                    for x_attrs in &xs_attrs {
                        deps = deps.union(&x_attrs.deps);
                    }
                    deps
                },
                value_or_string: {
                    let mut string = String::new();
                    hooks::write_builtin_ident(&mut string, self.ident);
                    write!(string, "(").unwrap();
                    let mut sep = "";
                    for x_attrs in &xs_attrs {
                        write!(string, "{}{}", sep, &x_attrs.value_or_string).unwrap();
                        sep = ", ";
                    }
                    write!(string, ")").unwrap();
                    string.into()
                },
            },
            Info::Fn(info) => ExprAttrs {
                ty: {
                    if xs_attrs.len() < info.param_tys.len() {
                        return Err(Error::TooFewArgsForCall {
                            fn_ident: self.ident,
                            expected_count: info.param_tys.len(),
                            actual_count: xs_attrs.len(),
                        });
                    }
                    if xs_attrs.len() > info.param_tys.len() {
                        return Err(Error::TooManyArgsForCall {
                            fn_ident: self.ident,
                            expected_count: info.param_tys.len(),
                            actual_count: xs_attrs.len(),
                        });
                    }
                    for (index, (x_attrs, param_ty)) in
                        xs_attrs.iter().zip(&info.param_tys).enumerate()
                    {
                        if &x_attrs.ty != param_ty {
                            return Err(Error::MismatchedTyForArg {
                                index,
                                fn_ident: self.ident,
                                expected_ty: param_ty.clone(),
                                actual_ty: x_attrs.ty.clone(),
                            });
                        }
                    }
                    info.return_ty.clone()
                },
                deps: {
                    let mut deps = info.deps.clone();
                    deps.fn_idents.insert(self.ident);
                    for x_attrs in &xs_attrs {
                        deps = deps.union(&x_attrs.deps);
                    }
                    deps
                },
                value_or_string: {
                    let mut string = String::new();
                    hooks::write_fn_ident(&mut string, self.ident);
                    write!(string, "(").unwrap();
                    hooks::write_args(
                        &mut string,
                        &xs_attrs,
                        &info.deps.uniform_block_idents,
                        info.deps.has_attributes,
                        info.deps.has_input_varyings,
                        info.deps.has_output_varyings,
                    );
                    write!(string, ")").unwrap();
                    string.into()
                },
            },
            _ => return Err(Error::IdentIsNotAFn(self.ident)),
        })
    }
}

impl ConsCallExpr {
    fn emit(&self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        if emitter.is_in_lvalue_context {
            return Err(Error::InvalidLeftHandSide);
        }
        let xs_attrs = self
            .xs
            .iter()
            .map(|x| x.emit(emitter))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExprAttrs {
            ty: if xs_attrs.len() == 1 {
                let x_attrs = &xs_attrs[0];
                match self.ty_lit {
                    TyLit::Bvec2 => match &x_attrs.ty {
                        Ty::Bool => Some(Ty::Bvec2),
                        Ty::Bvec2 => Some(Ty::Bvec2),
                        _ => None,
                    },
                    TyLit::Bvec3 => match &x_attrs.ty {
                        Ty::Bool => Some(Ty::Bvec3),
                        Ty::Bvec3 => Some(Ty::Bvec3),
                        _ => None,
                    },
                    TyLit::Bvec4 => match &x_attrs.ty {
                        Ty::Bool => Some(Ty::Bvec4),
                        Ty::Bvec4 => Some(Ty::Bvec4),
                        _ => None,
                    },
                    TyLit::Ivec2 => match &x_attrs.ty {
                        Ty::Int => Some(Ty::Vec2),
                        Ty::Ivec2 => Some(Ty::Vec2),
                        _ => None,
                    },
                    TyLit::Ivec3 => match &x_attrs.ty {
                        Ty::Int => Some(Ty::Vec3),
                        Ty::Ivec3 => Some(Ty::Vec3),
                        _ => None,
                    },
                    TyLit::Ivec4 => match &x_attrs.ty {
                        Ty::Int => Some(Ty::Vec4),
                        Ty::Ivec4 => Some(Ty::Vec4),
                        _ => None,
                    },
                    TyLit::Vec2 => match &x_attrs.ty {
                        Ty::Float => Some(Ty::Vec2),
                        Ty::Vec2 => Some(Ty::Vec2),
                        _ => None,
                    },
                    TyLit::Vec3 => match &x_attrs.ty {
                        Ty::Float => Some(Ty::Vec3),
                        Ty::Vec3 => Some(Ty::Vec3),
                        _ => None,
                    },
                    TyLit::Vec4 => match &x_attrs.ty {
                        Ty::Float => Some(Ty::Vec4),
                        Ty::Vec4 => Some(Ty::Vec4),
                        _ => None,
                    },
                    TyLit::Mat2 => match &x_attrs.ty {
                        Ty::Float => Some(Ty::Mat2),
                        Ty::Mat2 => Some(Ty::Mat2),
                        _ => None,
                    },
                    TyLit::Mat3 => match &x_attrs.ty {
                        Ty::Float => Some(Ty::Mat3),
                        Ty::Mat2 => Some(Ty::Mat3),
                        Ty::Mat3 => Some(Ty::Mat3),
                        _ => None,
                    },
                    TyLit::Mat4 => match &x_attrs.ty {
                        Ty::Float => Some(Ty::Mat4),
                        Ty::Mat2 => Some(Ty::Mat4),
                        Ty::Mat3 => Some(Ty::Mat4),
                        Ty::Mat4 => Some(Ty::Mat4),
                        _ => None,
                    },
                    _ => None,
                }
            } else {
                None
            }
            .map(Ok)
            .unwrap_or_else(|| {
                let ty = self.ty_lit.to_ty();
                match ty {
                    Ty::Bool
                    | Ty::Int
                    | Ty::Float
                    | Ty::Bvec2
                    | Ty::Ivec2
                    | Ty::Vec2
                    | Ty::Bvec3
                    | Ty::Ivec3
                    | Ty::Vec3
                    | Ty::Bvec4
                    | Ty::Ivec4
                    | Ty::Vec4
                    | Ty::Mat2
                    | Ty::Mat3
                    | Ty::Mat4 => {}
                    _ => return Err(Error::TyLitIsNotACons(self.ty_lit)),
                };
                let mut actual_count = 0;
                for x_attrs in &xs_attrs {
                    match x_attrs.ty {
                        Ty::Bool
                        | Ty::Int
                        | Ty::Float
                        | Ty::Bvec2
                        | Ty::Bvec3
                        | Ty::Bvec4
                        | Ty::Ivec2
                        | Ty::Ivec3
                        | Ty::Ivec4
                        | Ty::Vec2
                        | Ty::Vec3
                        | Ty::Vec4 => {}
                        _ => {
                            return Err(Error::CannotCallCons {
                                ty_lit: self.ty_lit,
                                xs_ty: xs_attrs.iter().map(|x_attrs| x_attrs.ty.clone()).collect(),
                            });
                        }
                    };
                    actual_count += x_attrs.ty.size().unwrap();
                }
                let expected_count = ty.size().unwrap();
                if actual_count < expected_count {
                    return Err(Error::TooFewCompsForConsCall {
                        ty_lit: self.ty_lit,
                        actual_count,
                        expected_count,
                    });
                }
                if actual_count > expected_count {
                    return Err(Error::TooManyCompsForConsCall {
                        ty_lit: self.ty_lit,
                        actual_count,
                        expected_count,
                    });
                }
                Ok(ty)
            })?,
            deps: {
                let mut deps = Deps::default();
                for x_attrs in &xs_attrs {
                    deps = deps.union(&x_attrs.deps);
                }
                deps
            },
            value_or_string: {
                let mut string = format!("{}(", self.ty_lit);
                let mut sep = "";
                for x_attrs in &xs_attrs {
                    write!(string, "{}{}", sep, x_attrs.value_or_string).unwrap();
                    sep = ", ";
                }
                write!(string, ")").unwrap();
                string.into()
            },
        })
    }
}

impl VarExpr {
    fn emit(self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        let info = match emitter
            .find_info(self.ident)
            .ok_or(Error::IdentIsNotDefined(self.ident))?
        {
            Info::Var(info) => info,
            _ => return Err(Error::IdentIsNotAVar(self.ident)),
        };
        let ty = info.ty.clone();
        if match ty {
            Ty::Array { .. } => true,
            Ty::Struct { ident } => {
                match emitter.find_info(ident).unwrap() {
                    Info::Struct(info) => info,
                    _ => panic!(),
                }
                .contains_arrays
            }
            _ => false,
        } {
            if emitter.is_in_lvalue_context {
                return Err(Error::InvalidLeftHandSide);
            }
        }
        Ok(match info.kind {
            VarInfoKind::Attribute => {
                if emitter.is_in_lvalue_context {
                    return Err(Error::CannotAssignToAttributeVar { ident: self.ident });
                }
                ExprAttrs {
                    ty,
                    deps: Deps {
                        has_attributes: true,
                        ..Deps::default()
                    },
                    value_or_string: {
                        let mut string = String::new();
                        hooks::write_attribute_var(&mut string, self.ident);
                        string
                    }
                    .into(),
                }
            }
            VarInfoKind::Local { is_mut } => {
                if emitter.is_in_lvalue_context && !is_mut {
                    return Err(Error::CannotAssignToImmutableVar { ident: self.ident });
                }
                ExprAttrs {
                    ty,
                    deps: Deps::default(),
                    value_or_string: self.ident.to_string().into(),
                }
            }
            VarInfoKind::Uniform { block_ident } => {
                if emitter.is_in_lvalue_context {
                    return Err(Error::CannotAssignToUniformVar { ident: self.ident });
                }
                ExprAttrs {
                    ty,
                    deps: Deps {
                        uniform_block_idents: [block_ident].iter().cloned().collect(),
                        ..Deps::default()
                    },
                    value_or_string: {
                        let mut string = String::new();
                        hooks::write_uniform_var(&mut string, block_ident, self.ident);
                        string
                    }
                    .into(),
                }
            }
            VarInfoKind::Varying => ExprAttrs {
                ty,
                deps: Deps {
                    has_input_varyings: !emitter.is_in_lvalue_context,
                    has_output_varyings: emitter.is_in_lvalue_context,
                    ..Deps::default()
                },
                value_or_string: {
                    let mut string = String::new();
                    hooks::write_varying_var(&mut string, self.ident);
                    string
                }
                .into(),
            },
        })
    }
}

impl Lit {
    fn emit(self, emitter: &mut DeclEmitter) -> Result<ExprAttrs, Error> {
        if emitter.is_in_lvalue_context {
            return Err(Error::InvalidLeftHandSide);
        }
        Ok(ExprAttrs {
            ty: self.to_ty(),
            deps: Deps::default(),
            value_or_string: self.to_value().into(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scope {
    infos_by_ident: HashMap<Ident, Info>,
}

impl Scope {
    fn get(&self, ident: Ident) -> Option<&Info> {
        self.infos_by_ident.get(&ident)
    }

    fn insert(&mut self, ident: Ident, info: Info) -> bool {
        match self.infos_by_ident.entry(ident) {
            Entry::Vacant(entry) => {
                entry.insert(info);
                true
            }
            Entry::Occupied(_) => false,
        }
    }
}

#[derive(Clone, Debug)]
enum Info {
    Builtin(BuiltinInfo),
    Fn(FnInfo),
    Struct(StructInfo),
    Var(VarInfo),
}

#[derive(Clone, Debug)]
struct BuiltinInfo {
    return_tys_by_param_tys: HashMap<Vec<Ty>, Ty>,
}

#[derive(Clone, Debug)]
struct FnInfo {
    param_tys: Vec<Ty>,
    return_ty: Ty,
    deps: Deps,
}

#[derive(Clone, Debug)]
struct StructInfo {
    contains_arrays: bool,
    member_tys_by_ident: HashMap<Ident, Ty>,
}

#[derive(Clone, Debug)]
struct VarInfo {
    ty: Ty,
    kind: VarInfoKind,
}

#[derive(Clone, Debug)]
enum VarInfoKind {
    Attribute,
    Local { is_mut: bool },
    Uniform { block_ident: Ident },
    Varying,
}

#[derive(Clone, Debug, Default)]
struct Deps {
    fn_idents: HashSet<Ident>,
    has_attributes: bool,
    has_input_varyings: bool,
    has_output_varyings: bool,
    uniform_block_idents: HashSet<Ident>,
}

impl Deps {
    fn union(self, other: &Deps) -> Deps {
        Deps {
            fn_idents: self.fn_idents.union(&other.fn_idents).cloned().collect(),
            has_attributes: self.has_attributes || other.has_attributes,
            has_input_varyings: self.has_input_varyings || other.has_input_varyings,
            has_output_varyings: self.has_output_varyings || other.has_output_varyings,
            uniform_block_idents: self
                .uniform_block_idents
                .union(&other.uniform_block_idents)
                .cloned()
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ValueOrString {
    Value(Value),
    String(String),
}

impl ValueOrString {
    pub fn to_value(&self) -> Option<Value> {
        if let ValueOrString::Value(value) = *self {
            Some(value)
        } else {
            None
        }
    }
}

impl From<Value> for ValueOrString {
    fn from(value: Value) -> ValueOrString {
        ValueOrString::Value(value)
    }
}

impl From<String> for ValueOrString {
    fn from(string: String) -> ValueOrString {
        ValueOrString::String(string)
    }
}

impl<'a> fmt::Display for ValueOrString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ValueOrString::Value(value) => write!(f, "{}", value),
            ValueOrString::String(string) => write!(f, "{}", string),
        }
    }
}

fn write_ident_and_ty(string: &mut String, ident: Ident, ty: &Ty) {
    match ty {
        Ty::Void => {
            write!(string, "void {}", ident).unwrap();
        }
        Ty::Bool => {
            hooks::write_ty_lit(string, TyLit::Bool);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Int => {
            hooks::write_ty_lit(string, TyLit::Int);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Float => {
            hooks::write_ty_lit(string, TyLit::Float);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Bvec2 => {
            hooks::write_ty_lit(string, TyLit::Bvec2);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Bvec3 => {
            hooks::write_ty_lit(string, TyLit::Bvec3);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Bvec4 => {
            hooks::write_ty_lit(string, TyLit::Bvec4);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Ivec2 => {
            hooks::write_ty_lit(string, TyLit::Ivec2);
            write!(string, "{}", ident).unwrap();
        }
        Ty::Ivec3 => {
            hooks::write_ty_lit(string, TyLit::Ivec3);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Ivec4 => {
            hooks::write_ty_lit(string, TyLit::Ivec4);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Vec2 => {
            hooks::write_ty_lit(string, TyLit::Vec2);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Vec3 => {
            hooks::write_ty_lit(string, TyLit::Vec3);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Vec4 => {
            hooks::write_ty_lit(string, TyLit::Vec4);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Mat2 => {
            hooks::write_ty_lit(string, TyLit::Mat2);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Mat3 => {
            hooks::write_ty_lit(string, TyLit::Mat3);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Mat4 => {
            hooks::write_ty_lit(string, TyLit::Mat4);
            write!(string, " {}", ident).unwrap();
        }
        Ty::Array { elem_ty, len } => {
            write_ident_and_ty(string, ident, elem_ty);
            write!(string, "[{}]", len).unwrap();
        }
        Ty::Struct {
            ident: struct_ident,
        } => write!(string, "{} {}", struct_ident, ident).unwrap(),
    }
}
