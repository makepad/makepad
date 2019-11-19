// Shared shader-compiler code for generating GLSL and Metal shading language

use std::hash::{Hash, Hasher};
use crate::shader::*;

#[derive(Default, Clone, PartialEq)]
pub struct ShaderGen {
    pub log: i32,
    pub name: String,
    pub geometry_vertices: Vec<f32>,
    pub geometry_indices: Vec<u32>,
    pub asts: Vec<ShAst>,
}

impl Eq for ShaderGen {}

impl Hash for ShaderGen {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.log.hash(state);
        self.name.hash(state);
        self.geometry_indices.hash(state);
        for vertex in &self.geometry_vertices {
            vertex.to_bits().hash(state);
        }
        self.asts.hash(state);
    }
}

impl ShaderGen {
    pub fn new() -> Self {
        let sg = ShaderGen::default();
        let sg = CxShader::def_builtins(sg);
        let sg = CxShader::def_df(sg);
        let sg = CxPass::def_uniforms(sg);
        let sg = CxView::def_uniforms(sg);
        sg
    }
    
    pub fn compose(mut self, ast: ShAst) -> Self {
        self.asts.push(ast);
        self
    }
    
    // flatten our
    pub fn flat_vars<F>(&self, cb:F) -> Vec<ShVar> 
    where F: Fn(&ShVarStore)->bool{
        let mut ret = Vec::new();
        for ast in self.asts.iter() {
            for shvar in &ast.vars {
                if cb(&shvar.store){
                    ret.push(shvar.clone());
                }
            }
        }
        ret
    }
    
    // flatten our
    pub fn flat_consts(&self) -> Vec<ShConst> {
        let mut ret = Vec::new();
        for ast in self.asts.iter().rev() {
            for shconst in &ast.consts {
                ret.push(shconst.clone());
            }
        };
        ret
    }
    
    // find a function
    pub fn find_fn(&self, name: &str) -> Option<&ShFn> {
        for ast in self.asts.iter().rev() {
            for shfn in &ast.fns {
                if shfn.name == name {
                    return Some(&shfn)
                }
            }
        }
        None
    }
    
    pub fn find_var(&self, name: &str) -> Option<&ShVar> {
        for ast in self.asts.iter().rev() {
            for shvar in &ast.vars {
                if shvar.name == name {
                    return Some(&shvar)
                }
            }
        }
        None
    }
    
    pub fn find_const(&self, name: &str) -> Option<&ShConst> {
        for ast in self.asts.iter().rev() {
            for shconst in &ast.consts {
                if shconst.name == name {
                    return Some(&shconst)
                }
            }
        }
        None
    }
    
    pub fn find_type(&self, name: &str) -> Option<&ShType> {
        for ast in self.asts.iter().rev() {
            for shtype in &ast.types {
                if shtype.name == name {
                    return Some(&shtype)
                }
            }
        }
        None
    }
    
    pub fn get_type_slots(&self, name: &str) -> usize {
        if let Some(ty) = self.find_type(name) {
            return ty.slots;
        }
        0
    }
    
    pub fn compute_slot_total(&self, vars: &Vec<ShVar>) -> usize {
        let mut slots: usize = 0;
        for var in vars {
            slots += self.get_type_slots(&var.ty);
        }
        slots
    }
}


// The AST block
#[derive(Clone, Hash, PartialEq)]
pub struct ShAst {
    pub types: Vec<ShType>,
    pub vars: Vec<ShVar>,
    pub consts: Vec<ShConst>,
    pub fns: Vec<ShFn>
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShFnArg {
    pub name: String,
    pub ty: String
}

impl ShFnArg {
    pub fn new(name: &str, ty: &str) -> Self {
        Self {
            name: name.to_string(),
            ty: ty.to_string()
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShFn {
    pub name: String,
    pub args: Vec<ShFnArg>,
    pub ret: String,
    pub block: Option<ShBlock>,
}

#[derive(Clone, Hash, PartialEq)]
pub enum UniformType {
    Color(UniformColor),
    Vec4(UniformVec4),
    Vec3(UniformVec3),
    Vec2(UniformVec2),
    Float(UniformFloat)
}

impl UniformType{
    fn type_name(&self)->String{
        match self{
            UniformType::Color(_)=>"vec4".to_string(),
            UniformType::Vec4(_)=>"vec4".to_string(),
            UniformType::Vec3(_)=>"vec3".to_string(),
            UniformType::Vec2(_)=>"vec2".to_string(),
            UniformType::Float(_)=>"float".to_string(),
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub enum InstanceType {
    Color(InstanceColor),
    Vec4(InstanceVec4),
    Vec3(InstanceVec3),
    Vec2(InstanceVec2),
    Float(InstanceFloat)
}

impl InstanceType{
    fn type_name(&self)->String{
        match self{
            InstanceType::Color(_)=>"vec4".to_string(),
            InstanceType::Vec4(_)=>"vec4".to_string(),
            InstanceType::Vec3(_)=>"vec3".to_string(),
            InstanceType::Vec2(_)=>"vec2".to_string(),
            InstanceType::Float(_)=>"float".to_string(),
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub enum ShVarStore {
    Uniform(UniformType),
    UniformColor(ColorId),
    UniformVw,
    UniformCx,
    Instance(InstanceType),
    Geometry,
    Texture,
    Local,
    Varying,
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShVar {
    pub name: String,
    pub ty: String,
    pub store: ShVarStore
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShConst {
    pub name: String,
    pub ty: String,
    pub value: ShExpr
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShTypeField {
    pub name: String,
    pub ty: String,
}

impl ShTypeField {
    pub fn new(name: &str, ty: &str) -> Self {
        Self {
            name: name.to_string(),
            ty: ty.to_string()
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShType {
    pub name: String,
    pub slots: usize,
    pub prim: bool,
    pub fields: Vec<ShTypeField>
}

// AST tree nodes

#[derive(Clone, Hash, PartialEq)]
pub enum ShExpr {
    ShId(ShId),
    ShLit(ShLit),
    ShField(ShField),
    ShIndex(ShIndex),
    ShAssign(ShAssign),
    ShAssignOp(ShAssignOp),
    ShBinary(ShBinary),
    ShUnary(ShUnary),
    ShParen(ShParen),
    ShBlock(ShBlock),
    ShCall(ShCall),
    ShIf(ShIf),
    ShWhile(ShWhile),
    ShForLoop(ShForLoop),
    ShReturn(ShReturn),
    ShBreak(ShBreak),
    ShContinue(ShContinue)
}

#[derive(Clone, Hash, PartialEq)]
pub enum ShBinOp {
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
    AddEq,
    SubEq,
    MulEq,
    DivEq,
    RemEq,
    BitXorEq,
    BitAndEq,
    BitOrEq,
    ShlEq,
    ShrEq
}

impl ShBinOp {
    pub fn to_string(&self) -> &str {
        match self {
            ShBinOp::Add => "+",
            ShBinOp::Sub => "-",
            ShBinOp::Mul => "*",
            ShBinOp::Div => "/",
            ShBinOp::Rem => "%",
            ShBinOp::And => "&&",
            ShBinOp::Or => "||",
            ShBinOp::BitXor => "^",
            ShBinOp::BitAnd => "&",
            ShBinOp::BitOr => "|",
            ShBinOp::Shl => "<<",
            ShBinOp::Shr => ">>",
            ShBinOp::Eq => "==",
            ShBinOp::Lt => "<",
            ShBinOp::Le => "<=",
            ShBinOp::Ne => "!=",
            ShBinOp::Ge => ">=",
            ShBinOp::Gt => ">",
            ShBinOp::AddEq => "+=",
            ShBinOp::SubEq => "-=",
            ShBinOp::MulEq => "*=",
            ShBinOp::DivEq => "/=",
            ShBinOp::RemEq => "%=",
            ShBinOp::BitXorEq => "^=",
            ShBinOp::BitAndEq => "&=",
            ShBinOp::BitOrEq => "|=",
            ShBinOp::ShlEq => "<<=",
            ShBinOp::ShrEq => ">>=",
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShId {
    pub name: String
}

#[derive(Clone)]
pub enum ShLit {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool)
}

impl Hash for ShLit {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ShLit::Int(iv) => iv.hash(state),
            ShLit::Float(fv) => fv.to_bits().hash(state),
            ShLit::Str(sv) => sv.hash(state),
            ShLit::Bool(bv) => bv.hash(state)
        }
    }
}

impl PartialEq for ShLit {
    fn eq(&self, other: &ShLit) -> bool {
        match self {
            ShLit::Int(iv) => match other {
                ShLit::Int(ov) => iv == ov,
                _ => false
            },
            ShLit::Float(iv) => match other {
                ShLit::Float(ov) => iv.to_bits() == ov.to_bits(),
                _ => false
            },
            ShLit::Str(iv) => match other {
                ShLit::Str(ov) => iv == ov,
                _ => false
            },
            ShLit::Bool(iv) => match other {
                ShLit::Bool(ov) => iv == ov,
                _ => false
            },
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShField {
    pub base: Box<ShExpr>,
    pub member: String
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShIndex {
    pub base: Box<ShExpr>,
    pub index: Box<ShExpr>
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShAssign {
    pub left: Box<ShExpr>,
    pub right: Box<ShExpr>
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShAssignOp {
    pub left: Box<ShExpr>,
    pub right: Box<ShExpr>,
    pub op: ShBinOp
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShBinary {
    pub left: Box<ShExpr>,
    pub right: Box<ShExpr>,
    pub op: ShBinOp
}

#[derive(Clone, Hash, PartialEq)]
pub enum ShUnaryOp {
    Not,
    Neg
}

impl ShUnaryOp {
    pub fn to_string(&self) -> &str {
        match self {
            ShUnaryOp::Not => "!",
            ShUnaryOp::Neg => "-"
        }
    }
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShUnary {
    pub expr: Box<ShExpr>,
    pub op: ShUnaryOp
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShParen {
    pub expr: Box<ShExpr>,
}

#[derive(Clone, Hash, PartialEq)]
pub enum ShStmt {
    ShLet(ShLet),
    ShExpr(ShExpr),
    ShSemi(ShExpr)
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShBlock {
    pub stmts: Vec<Box<ShStmt>>
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShCall {
    pub call: String,
    pub args: Vec<Box<ShExpr>>
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShIf {
    pub cond: Box<ShExpr>,
    pub then_branch: ShBlock,
    pub else_branch: Option<Box<ShExpr>>,
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShWhile {
    pub cond: Box<ShExpr>,
    pub body: ShBlock,
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShForLoop {
    pub iter: String,
    pub from: Box<ShExpr>,
    pub to: Box<ShExpr>,
    pub body: ShBlock
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShReturn {
    pub expr: Option<Box<ShExpr>>
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShBreak {
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShContinue {
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShLet {
    pub name: String,
    pub ty: String,
    pub init: Box<ShExpr>
}

#[derive(Clone)]
pub struct Sl {
    pub sl: String,
    pub ty: String
}

#[derive(Clone)]
pub struct SlErr {
    pub msg: String
}

pub struct SlDecl {
    pub name: String,
    pub ty: String
}

#[derive(Clone)]
pub enum SlTarget {
    Pixel,
    Vertex,
    Constant
}

pub struct SlCx<'a> {
    pub depth: usize,
    pub target: SlTarget,
    pub defargs_fn: String,
    pub defargs_call: String,
    pub call_prefix: String,
    pub shader_gen: &'a ShaderGen,
    pub scope: Vec<SlDecl>,
    pub fn_deps: Vec<String>,
    pub fn_done: Vec<Sl>,
    pub auto_vary: Vec<ShVar>
}

pub enum MapCallResult {
    Rename(String),
    Rewrite(String, String),
    None
}

impl<'a> SlCx<'a> {
    pub fn scan_scope(&self, name: &str) -> Option<&str> {
        if let Some(decl) = self.scope.iter().find( | i | i.name == name) {
            return Some(&decl.ty);
        }
        None
    }
    pub fn get_type(&self, name: &str) -> Result<&ShType,
    SlErr> {
        if let Some(ty) = self.shader_gen.find_type(name) {
            return Ok(ty);
        }
        Err(SlErr {msg: format!("Cannot find type {}", name)})
    }
}

impl ShExpr {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        match self {
            ShExpr::ShId(x) => x.sl(slcx),
            ShExpr::ShLit(x) => x.sl(slcx),
            ShExpr::ShAssign(x) => x.sl(slcx),
            ShExpr::ShCall(x) => x.sl(slcx),
            ShExpr::ShBinary(x) => x.sl(slcx),
            ShExpr::ShUnary(x) => x.sl(slcx),
            ShExpr::ShAssignOp(x) => x.sl(slcx),
            ShExpr::ShIf(x) => x.sl(slcx),
            ShExpr::ShWhile(x) => x.sl(slcx),
            ShExpr::ShForLoop(x) => x.sl(slcx),
            ShExpr::ShBlock(x) => x.sl(slcx),
            ShExpr::ShField(x) => x.sl(slcx),
            ShExpr::ShIndex(x) => x.sl(slcx),
            ShExpr::ShParen(x) => x.sl(slcx),
            ShExpr::ShReturn(x) => x.sl(slcx),
            ShExpr::ShBreak(x) => x.sl(slcx),
            ShExpr::ShContinue(x) => x.sl(slcx),
        }
    }
}


impl ShId {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        // ok so. we have to find our id on
        if let Some(ty) = slcx.scan_scope(&self.name) {
            Ok(Sl {sl: self.name.to_string(), ty: ty.to_string()})
        }
        else if let Some(cnst) = slcx.shader_gen.find_const(&self.name) {
            Ok(Sl {sl: self.name.to_string(), ty: cnst.ty.to_string()})
        }
        else if let Some(var) = slcx.shader_gen.find_var(&self.name) {
            Ok(Sl {sl: slcx.map_var(var), ty: var.ty.to_string()})
        }
        else { // id not found.. lets give an error
            Err(SlErr {
                msg: format!("Id {} not resolved, is it declared?", self.name)
            })
        }
    }
}

impl ShLit {
    pub fn sl(&self, _slcx: &mut SlCx) -> Result<Sl, SlErr> {
        // we do a literal
        match self {
            ShLit::Int(val) => {
                Ok(Sl {sl: format!("{}", val), ty: "int".to_string()})
            }
            ShLit::Str(val) => {
                Ok(Sl {sl: format!("{}", val), ty: "string".to_string()})
            }
            ShLit::Float(val) => {
                if val.ceil() == *val {
                    Ok(Sl {sl: format!("{}.0", val), ty: "float".to_string()})
                }
                else {
                    Ok(Sl {sl: format!("{}", val), ty: "float".to_string()})
                }
            }
            ShLit::Bool(val) => {
                Ok(Sl {sl: format!("{}", val), ty: "bool".to_string()})
            }
        }
    }
}

impl ShField {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let base = self.base.sl(slcx) ?;
        // we now have to figure out the type of member
        let shty = slcx.get_type(&base.ty) ?;
        // lets get our member
        if let Some(field) = shty.fields.iter().find( | i | i.name == self.member) {
            Ok(Sl {
                sl: format!("{}.{}", base.sl, self.member),
                ty: field.ty.to_string()
            })
        }
        else {
            let mut mode = 0;
            let slots = shty.slots;
            
            if shty.name != "float" && shty.name != "vec2" && shty.name != "vec3" && shty.name != "vec4" {
                return Err(SlErr {
                    msg: format!("member {} not found {}", self.member, base.ty)
                })
            }
            if self.member.len() >4 {
                return Err(SlErr {
                    msg: format!("member {} not found or a valid swizzle of {} {}", self.member, base.ty, base.sl)
                })
            }
            for chr in self.member.chars() {
                if chr == 'x' || chr == 'y' || chr == 'z' || chr == 'w' {
                    if chr == 'y' && slots<2 {mode = 3;}
                    else if chr == 'z' && slots<3 {mode = 3;}
                    else if chr == 'w' && slots<4 {mode = 3;};
                    if mode == 0 {mode = 1;}
                    else if mode != 1 {
                        return Err(SlErr {
                            msg: format!("member {} not a valid swizzle of {} {}", self.member, base.ty, base.sl)
                        })
                    }
                }
                else if chr == 'r' || chr == 'g' || chr == 'b' || chr == 'a' {
                    if chr == 'r' && slots<2 {mode = 3;}
                    else if chr == 'g' && slots<3 {mode = 3;}
                    else if chr == 'b' && slots<4 {mode = 3;};
                    if mode == 0 {mode = 2;}
                    else if mode != 2 {
                        return Err(SlErr {
                            msg: format!("member {} not a valid swizzle of {} {}", self.member, base.ty, base.sl)
                        })
                    }
                }
            }
            
            match self.member.len() {
                1 => return Ok(Sl {
                    sl: format!("{}.{}", base.sl, self.member),
                    ty: "float".to_string()
                }),
                2 => return Ok(Sl {
                    sl: format!("{}.{}", base.sl, self.member),
                    ty: "vec2".to_string()
                }),
                3 => return Ok(Sl {
                    sl: format!("{}.{}", base.sl, self.member),
                    ty: "vec3".to_string()
                }),
                4 => return Ok(Sl {
                    sl: format!("{}.{}", base.sl, self.member),
                    ty: "vec4".to_string()
                }),
                _ => Err(SlErr {
                    msg: format!("member {} not cannot be found on type {} {}", self.member, base.ty, base.sl)
                })
            }
        }
    }
}

impl ShIndex {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let base = self.base.sl(slcx) ?;
        let index = self.index.sl(slcx) ?;
        // limit base type to vec2/3/4
        if base.ty != "vec2" && base.ty != "vec3" && base.ty != "vec4" {
            Err(SlErr {
                msg: format!("index on unsupported type {}", base.ty)
            })
        }
        else {
            Ok(Sl {
                sl: format!("{}[{}]", base.sl, index.sl),
                ty: "float".to_string()
            })
        }
    }
}

impl ShAssign {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        // if we are assigning to a geom
        
        let left = self.left.sl(slcx) ?;
        let right = self.right.sl(slcx) ?;
        if left.ty != right.ty {
            Err(SlErr {
                msg: format!("Left type {} not the same as right {} in assign {}={}", left.ty, right.ty, left.sl, right.sl)
            })
        }
        else {
            Ok(Sl {
                sl: format!("{} = {}", left.sl, right.sl),
                ty: left.ty
            })
        }
    }
}

impl ShAssignOp {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let left = self.left.sl(slcx) ?;
        let right = self.right.sl(slcx) ?;
        
        if left.ty != right.ty {
            Err(SlErr {
                msg: format!("Left type {} not the same as right {} in assign op {}{}{}", left.ty, self.op.to_string(), right.ty, left.sl, right.sl)
            })
        }
        else {
            Ok(Sl {
                sl: format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                ty: left.ty
            })
        }
    }
}

impl ShBinary {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let left = self.left.sl(slcx) ?;
        let right = self.right.sl(slcx) ?;
        if left.ty != right.ty {
            if left.ty == "float" && (right.ty == "vec2" || right.ty == "vec3" || right.ty == "vec4") {
                Ok(Sl {
                    sl: format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                    ty: right.ty
                })
            }
            else if right.ty == "float" && (left.ty == "vec2" || left.ty == "vec3" || left.ty == "vec4") {
                Ok(Sl {
                    sl: format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                    ty: left.ty
                })
            }
            else if right.ty == "mat4" && left.ty == "vec4" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: left.ty
                })
            }
            else if right.ty == "mat3" && left.ty == "vec3" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: left.ty
                })
            }
            else if right.ty == "mat2" && left.ty == "vec2" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: left.ty
                })
            }
            else if left.ty == "mat4" && right.ty == "vec4" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: right.ty
                })
            }
            else if left.ty == "mat3" && right.ty == "vec3" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: right.ty
                })
            }
            else if left.ty == "mat2" && right.ty == "vec2" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: right.ty
                })
            }
            else {
                Err(SlErr {
                    msg: format!("Left type {} not the same as right {} in binary op {}{}{}", left.ty, right.ty, left.sl, self.op.to_string(), right.sl)
                })
            }
        }
        else {
            if left.ty == "mat4" || left.ty == "mat3" || left.ty == "mat2" {
                Ok(Sl {
                    sl: slcx.mat_mul(&left.sl, &right.sl),
                    ty: left.ty
                })
                
            }
            else {
                Ok(Sl {
                    sl: format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                    ty: left.ty
                })
            }
        }
    }
}

impl ShUnary {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let expr = self.expr.sl(slcx) ?;
        Ok(Sl {
            sl: format!("{}{}", self.op.to_string(), expr.sl),
            ty: expr.ty
        })
    }
}

impl ShParen {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let expr = self.expr.sl(slcx) ?;
        Ok(Sl {
            sl: format!("({})", expr.sl),
            ty: expr.ty
        })
    }
}

impl ShBlock {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut sl = String::new();
        sl.push_str("{\n");
        slcx.depth += 1;
        for stmt in &self.stmts {
            for _i in 0..slcx.depth {
                sl.push_str("  ");
            }
            match &**stmt {
                ShStmt::ShLet(stmt) => {
                    let out = stmt.sl(slcx) ?;
                    sl.push_str(&out.sl);
                },
                ShStmt::ShExpr(stmt) => {
                    let out = stmt.sl(slcx) ?;
                    sl.push_str(&out.sl);
                }
                ShStmt::ShSemi(stmt) => {
                    let out = stmt.sl(slcx) ?;
                    sl.push_str(&out.sl);
                }
            }
            sl.push_str(";\n");
        }
        slcx.depth -= 1;
        sl.push_str("}");
        Ok(Sl {
            sl: sl,
            ty: "void".to_string()
        })
    }
}

impl ShCall {
    pub fn sl(&self, slcx: &mut SlCx) -> Result<Sl, SlErr> {
        // we have a call, look up the call type on cx
        let mut out = String::new();
        if let Some(shfn) = slcx.shader_gen.find_fn(&self.call) {
            let mut defargs_call = "".to_string();
            
            if let Some(_block) = &shfn.block { // not internal, so its a dep
                let index = slcx.fn_deps.iter().position( | i | **i == self.call);
                if let Some(index) = index {
                    slcx.fn_deps.remove(index);
                }
                slcx.fn_deps.push(self.call.clone());
                defargs_call = slcx.defargs_call.to_string();
                out.push_str(&slcx.call_prefix);
            };
            
            
            // lets check our args and compose return type
            let mut gen_t = "".to_string();
            
            let mut args_gl = Vec::new();
            // loop over args and typecheck / fill in generics
            for arg in &self.args {
                let arg_gl = arg.sl(slcx) ?;
                args_gl.push(arg_gl);
            };
            
            let map_call = slcx.map_call(&self.call, &args_gl);
            let ret_ty;
            
            if let MapCallResult::Rewrite(rewrite, rty) = map_call {
                out.push_str(&rewrite);
                ret_ty = rty;
            }
            else {
                if let MapCallResult::Rename(name) = map_call {
                    out.push_str(&name);
                }
                else {
                    out.push_str(&self.call);
                }
                out.push_str("(");
                
                // loop over args and typecheck / fill in generics
                for (i, arg_gl) in args_gl.iter().enumerate() {
                    //let arg_gl = args_gl[i];//.sl(cx)?;
                    let in_ty = arg_gl.ty.clone();
                    if i != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&arg_gl.sl);
                    // lets check the type against our shfn
                    if i >= shfn.args.len() {
                        return Err(SlErr {
                            msg: format!("Too many function arguments for call {} got:{} can use:{}", self.call, i + 1, shfn.args.len())
                        })
                    }
                    // lets check our arg type
                    let fnarg = &shfn.args[i];
                    // lets see if ty is "T" or "O" or "F" or "B"
                    if fnarg.ty == "T" {
                        // we already have a gen_t but its not the same
                        if gen_t != "" && gen_t != in_ty {
                            return Err(SlErr {
                                msg: format!("Function type T incorrectly redefined for call {} type was {} given {} for arg {}", self.call, gen_t, in_ty, i)
                            })
                        }
                        gen_t = in_ty;
                    }
                    else if fnarg.ty == "F" { // we have to be a float type
                        if in_ty != "float" && in_ty != "vec2" && in_ty != "vec3" && in_ty != "vec4" {
                            return Err(SlErr {
                                msg: format!("Function type F is not a float-ty type for call {} for arg {} type {}", self.call, i, in_ty)
                            })
                        }
                    }
                    else if fnarg.ty == "B" { // have to be a boolvec
                        if in_ty != "bool" && in_ty != "bvec2" && in_ty != "bvec3" && in_ty != "bvec4" {
                            return Err(SlErr {
                                msg: format!("Function arg is not a bool-ty type for call {} for arg {} type {}", self.call, i, in_ty)
                            })
                        }
                        gen_t = in_ty;
                    }
                    else if fnarg.ty != in_ty {
                        return Err(SlErr {
                            msg: format!("Arg wrong type for call {} for arg {} expected type {} got type {}", self.call, i, fnarg.ty, in_ty)
                        })
                    }
                }
                // we have less args provided than the fn signature
                // check if they were optional
                if self.args.len() < shfn.args.len() {
                    for i in self.args.len()..shfn.args.len() {
                        let fnarg = &shfn.args[i];
                        if fnarg.ty != "O" {
                            return Err(SlErr {
                                msg: format!("Not enough args for call {} not enough args provided at {}, possible {}", self.call, i, shfn.args.len())
                            })
                        }
                    }
                };
                ret_ty = if shfn.ret == "T" || shfn.ret == "B" {
                    gen_t
                }
                else {
                    shfn.ret.clone()
                };
                if defargs_call.len() != 0 {
                    if self.args.len() != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&defargs_call);
                }
                out.push_str(")");
            }
            // check our arg types
            // if our return type is T,
            // use one of the args marked T as its type
            // make sure all args are the same type T
            Ok(Sl {
                sl: out,
                ty: ret_ty
            })
        }
        else {
            // its a constructor call
            if let Some(slty) = slcx.shader_gen.find_type(&self.call) {
                
                // HLSL doesn't allow vec3(0.) so we have to polyfill it
                // vec3(x), vec3(xy,z) vec3(x,yz), 
                let mut args = Vec::new();
                
                // TODO check args
                let mut arg_slots = 0;
                for arg in &self.args {
                    let arg_sl = arg.sl(slcx) ?;
                    if let Some(arg_ty) = slcx.shader_gen.find_type(&arg_sl.ty) {
                        arg_slots += arg_ty.slots;
                    }
                    else{
                        return Err(SlErr {
                            msg: format!("Cannot find constructor arg type {}", arg_sl.ty)
                        })
                    }
                    args.push(arg_sl);
                }
                if arg_slots > slty.slots{
                    let mut out = String::new();
                    out.push_str(&self.call);
                    out.push_str("(");
                    for (i,arg) in args.iter().enumerate(){
                        if i != 0{
                            out.push_str(", ")
                        }
                        out.push_str(&arg.sl);
                    }
                    out.push_str(")");
                    return Err(SlErr {
                        msg: format!("Constructor slots don't match given {} need {} - {}", arg_slots, slty.slots, out)
                    })
                }
                
                // lets sum up the arg type slots and see if it fits in our constructor
                // if it does lets pass it to map_constructor so each platform can polyfill it
                
                out.push_str(&slcx.map_constructor(&self.call, &args));
                Ok(Sl {
                    sl: out,
                    ty: slty.name.clone()
                })
            }
            else {
                Err(SlErr {
                    msg: format!("Cannot find function {}", self.call)
                })
            }
        }
        
    }
}

impl ShIf {
    pub fn sl(&self, cx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut out = "".to_string();
        out.push_str("if(");
        let cond = self.cond.sl(cx) ?;
        out.push_str(&cond.sl);
        out.push_str(")");
        
        let then = self.then_branch.sl(cx) ?;
        
        out.push_str(&then.sl);
        if let Some(else_branch) = &self.else_branch {
            let else_gl = else_branch.sl(cx) ?;
            out.push_str("else ");
            out.push_str(&else_gl.sl);
        }
        
        Ok(Sl {
            sl: out,
            ty: "void".to_string()
        })
    }
}

impl ShWhile {
    pub fn sl(&self, cx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut out = "".to_string();
        out.push_str("while(");
        let cond = self.cond.sl(cx) ?;
        out.push_str(&cond.sl);
        out.push_str(")");
        
        let body = self.body.sl(cx) ?;
        
        out.push_str(&body.sl);
        
        Ok(Sl {
            sl: out,
            ty: "void".to_string()
        })
    }
}

impl ShForLoop {
    pub fn sl(&self, cx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut out = "".to_string();
        
        out.push_str("for(int ");
        out.push_str(&self.iter);
        out.push_str("=");
        
        let from = self.from.sl(cx) ?;
        out.push_str(&from.sl);
        
        out.push_str(";");
        out.push_str(&self.iter);
        out.push_str(" < ");
        
        let to = self.to.sl(cx) ?;
        out.push_str(&to.sl);
        
        out.push_str(";");
        out.push_str(&self.iter);
        out.push_str("++)");
        
        let body = self.body.sl(cx) ?;
        
        out.push_str(&body.sl);
        
        Ok(Sl {
            sl: out,
            ty: "void".to_string()
        })
    }
}

impl ShReturn {
    pub fn sl(&self, cx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut out = "".to_string();
        if let Some(expr) = &self.expr {
            let expr_gl = expr.sl(cx) ?;
            out.push_str("return ");
            out.push_str(&expr_gl.sl);
        }
        else {
            out.push_str("return;");
        }
        Ok(Sl {
            sl: out,
            ty: "void".to_string()
        })
    }
}

impl ShBreak {
    pub fn sl(&self, _cx: &mut SlCx) -> Result<Sl, SlErr> {
        Ok(Sl {
            sl: "break".to_string(),
            ty: "void".to_string()
        })
    }
}

impl ShContinue {
    pub fn sl(&self, _cx: &mut SlCx) -> Result<Sl, SlErr> {
        Ok(Sl {
            sl: "continue".to_string(),
            ty: "void".to_string()
        })
    }
}

impl ShLet {
    pub fn sl(&self, cx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut out = "".to_string();
        let init = self.init.sl(cx) ?;
        
        let ty = init.ty.clone();
        if self.ty != "" && self.ty != init.ty {
            return Err(SlErr {
                msg: format!("Let definition {} type {} is different from initializer {}", self.name, self.ty, init.ty)
            })
        }
        
        out.push_str(&cx.map_type(&ty));
        out.push_str(" ");
        out.push_str(&self.name);
        out.push_str(" = ");
        
        // lets define our identifier on scope
        cx.scope.push(SlDecl {
            name: self.name.clone(),
            ty: init.ty.clone()
        });
        
        out.push_str(&init.sl);
        Ok(Sl {
            sl: out,
            ty: "void".to_string()
        })
    }
}

impl ShFn {
    pub fn sl(&self, cx: &mut SlCx) -> Result<Sl, SlErr> {
        let mut out = "".to_string();
        out.push_str(&cx.map_type(&self.ret));
        out.push_str(" ");
        out.push_str(&cx.call_prefix);
        out.push_str(&self.name);
        out.push_str("(");
        for (i, arg) in self.args.iter().enumerate() {
            if i != 0 {
                out.push_str(", ");
            }
            out.push_str(&cx.map_type(&arg.ty));
            out.push_str(" ");
            out.push_str(&arg.name);
            cx.scope.push(SlDecl {
                name: arg.name.clone(),
                ty: arg.ty.clone()
            });
        };
        if cx.defargs_fn.len() != 0 {
            if self.args.len() != 0 {
                out.push_str(", ");
            }
            out.push_str(&cx.defargs_fn);
        }
        out.push_str(")");
        if let Some(block) = &self.block {
            let block = block.sl(cx) ?;
            out.push_str(&block.sl);
        };
        Ok(Sl {
            sl: out,
            ty: self.name.clone()
        })
    }
}

pub fn assemble_fn_and_deps(sg: &ShaderGen, cx: &mut SlCx) -> Result<String, SlErr> {
    
    let mut fn_local = Vec::new();
    loop {
        
        // find what deps we haven't done yet
        let fn_not_done = cx.fn_deps.iter().find( | cxfn | {
            if let Some(_done) = cx.fn_done.iter().find( | i | i.ty == **cxfn) {
                false
            }
            else {
                true
            }
        });
        // do that dep.
        if let Some(fn_not_done) = fn_not_done {
            let fn_name = fn_not_done.clone();
            let fn_to_do = sg.find_fn(&fn_name);
            if let Some(fn_to_do) = fn_to_do {
                cx.scope.clear();
                let result = fn_to_do.sl(cx) ?;
                cx.fn_done.push(result.clone());
                fn_local.push((fn_name, result.clone()));
            }
            else {
                return Err(SlErr {msg: format!("Cannot find entry function {}", fn_not_done)})
            }
        }
        else {
            break;
        }
    }
    // ok lets reverse concatinate it
    let mut out = String::new();
    for dep in cx.fn_deps.iter().rev() {
        if let Some((_, fnd)) = fn_local.iter().find( | (name, _) | name == dep) {
            out.push_str(&fnd.sl);
            out.push_str("\n");
        }
    };
    Ok(out)
}

pub fn assemble_const_init(cnst: &ShConst, cx: &mut SlCx) -> Result<Sl, SlErr> {
    // lets process the expr of a constant
    let result = cnst.value.sl(cx) ?;
    if cx.fn_deps.len() > 0 {
        return Err(SlErr {msg: "Const cant have function call deps".to_string()});
    }
    return Ok(result)
}

