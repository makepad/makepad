use crate::ast::ShaderAst;
use crate::error::Error;
use crate::lex;
use crate::token::TokenWithSpan;
use crate::parse;
use crate::ty::*;
use crate::geometry::*;
use crate::token::Token;
use crate::analyse::ShaderAnalyser;
use crate::span::Span;
use crate::ident::Ident;
use crate::env::{Env, Sym};
use crate::lit::Lit;
use crate::builtin::{self, Builtin};

use std::any::TypeId;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::collections::HashMap;

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct LiveLoc {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct PropDef {
    pub name: String,
    pub ident: String,
    pub block: Option<String>,
    pub prop_id: PropId,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct ShaderSub {
    pub loc: LiveLoc,
    pub code: String,
    pub geometries: Vec<PropDef>,
    pub instances: Vec<PropDef>,
    pub uniforms: Vec<PropDef>,
    pub textures: Vec<PropDef>,
}

#[derive(Default, Clone, PartialEq)]
pub struct ShaderGen {
    pub geometry: Geometry,
    pub subs: Vec<ShaderSub>,
}

impl Eq for ShaderGen {}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderGenError {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub msg: String,
}

impl fmt::Display for ShaderGenError {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {} {} - {}",
            self.path,
            self.line,
            self.col,
            self.msg
        )
    }
}

#[derive(Clone, Debug)]
pub struct ShaderInheritConst {
    const_table: Vec<f32>,
    const_table_spans: Vec<(usize, Span)>
}

#[derive(Clone, Debug)]
pub struct ShaderInheritCacheNode {
    pub code: String,
    pub ast: ShaderAst,
    pub env: Env,
    pub tokens: Vec<TokenWithSpan>,
    pub prev_consts: Option<ShaderInheritConst>,
}

#[derive(Clone, Debug)]
pub struct ShaderInheritCache {
    pub builtins: HashMap<Ident, Builtin>,
    pub map: HashMap<LiveLoc, ShaderInheritCacheNode>
}

impl ShaderInheritCache {
    pub fn new() -> Self {
        Self {
            builtins: builtin::generate_builtins(),
            map: HashMap::new()
        }
    }
}

pub enum ShaderGenResult {
    ShaderAst(ShaderAst),
    PatchedConstTable(Vec<f32>),
    Error(ShaderGenError)
}

impl ShaderGen {
    pub fn new() -> Self {
        ShaderGen::default()
    }
    
    pub fn byte_to_row_col(byte: usize, source: &str) -> (usize, usize) {
        let lines = source.split("\n");
        let mut o = 0;
        for (index, line) in lines.enumerate() {
            if byte >= o && byte < o + line.len() {
                return (index, byte - o);
            }
            o += line.len() + 1;
        }
        return (0, 0);
    }
    
    pub fn shader_gen_error(err: &Error, sub: &ShaderSub) -> ShaderGenError {
        // lets find the span info
        let start = ShaderGen::byte_to_row_col(err.span.start, &sub.code);
        ShaderGenError {
            path: sub.loc.path.to_string(),
            line: start.0 + sub.loc.line,
            col: start.1 + 1,
            len: err.span.end - err.span.start,
            msg: err.to_string(),
        }
    }
    
    pub fn compose(mut self, sub: ShaderSub) -> Self {
        self.subs.push(sub);
        self
    }
    
    pub fn lex_parse_analyse(&self, gather_all: bool, use_const_table: bool, inherit_cache: &mut ShaderInheritCache) -> ShaderGenResult {
        fn add_inputs_to_env(env: &mut Env, sub: &ShaderSub) {
            for prop in &sub.geometries {
                let _ = env.insert_sym(
                    Span::default(),
                    Ident::new(&prop.ident),
                    Sym::TyVar {
                        ty: prop.prop_id.shader_ty(),
                    },
                );
            }
            for prop in &sub.instances {
                let _ = env.insert_sym(
                    Span::default(),
                    Ident::new(&prop.ident),
                    Sym::TyVar {
                        ty: prop.prop_id.shader_ty(),
                    },
                );
            }
            for prop in &sub.textures {
                let _ = env.insert_sym(
                    Span::default(),
                    Ident::new(&prop.ident),
                    Sym::TyVar {
                        ty: prop.prop_id.shader_ty(),
                    },
                );
            }
            for prop in &sub.uniforms {
                let _ = env.insert_sym(
                    Span::default(),
                    Ident::new(&prop.ident),
                    Sym::TyVar {
                        ty: prop.prop_id.shader_ty(),
                    },
                );
            }
        }
        
        fn check_const_table_patch(cache: &mut ShaderInheritCacheNode, tokens: &Vec<TokenWithSpan>) -> Option<Vec<f32>> {
            if tokens.len() != cache.tokens.len() {
                return None
            }
            if let Some(prev_consts) = &mut cache.prev_consts {
                // lets do a token diff
                let mut delta = 0;
                let mut last = 0;
                for i in 0..tokens.len() {
                    if tokens[i].token != cache.tokens[i].token {
                        delta += 1;
                        last = i;
                    }
                }
                if delta != 1 {
                    return None
                }
                
                let new_lit = if let Token::Lit(lit) = &tokens[last].token {
                    lit.clone()
                }
                else {
                    return None
                };
                let old_lit = if let Token::Lit(lit) = &cache.tokens[last].token {
                    lit.clone()
                }
                else {
                    return None
                };
                   
                if new_lit.to_ty() != old_lit.to_ty(){
                    return None
                } 
                
                for i in 0..prev_consts.const_table_spans.len(){
                    let (index, span) = prev_consts.const_table_spans[i];
                    if span == cache.tokens[last].span {
                        // ok now. we have a new_lit, now we simply have to stuff it the const table
                        match new_lit{
                            Lit::Float(v)=>{
                                prev_consts.const_table[index] = v;
                            },
                            Lit::Vec4(v)=>{
                                prev_consts.const_table[index+0] = v.x;
                                prev_consts.const_table[index+1] = v.y;
                                prev_consts.const_table[index+2] = v.z;
                                prev_consts.const_table[index+3] = v.w;
                            },
                            _=>()
                        }
                        // now we have to patch up the const_table_spans that come after to the new token spans
                        let mut last_span = 0;
                        for i in 0..tokens.len() {
                            match cache.tokens[i].token{
                                Token::Lit(Lit::Float(_)) | Token::Lit(Lit::Vec4(_))=>{ // might be a const table
                                    for i in last_span..prev_consts.const_table_spans.len(){
                                        let (_, span) = &mut prev_consts.const_table_spans[i];
                                        if *span == cache.tokens[i].span {
                                            *span = tokens[i].span;
                                            last_span = i + 1;
                                            break; 
                                        }
                                    }
                                },
                                _=>()
                            }
                        }
                        cache.tokens = tokens.clone();
                        return Some(prev_consts.const_table.clone()) 
                    }
                }
            } 
            None
        }
        
        if use_const_table { // we are a dynamic recompile. see if we can use inheritance cache
            let sub = self.subs.last().unwrap();
            if let Some(cache) = inherit_cache.map.get_mut(&sub.loc) {
                if cache.code != sub.code { // we can reuse the cache
                    // lets see if we can diff the tokens.
                    let tokens = lex::lex(sub.code.chars(), self.subs.len() - 1).collect::<Result<Vec<_>, _>>();
                    if let Err(err) = &tokens {
                        return ShaderGenResult::Error(ShaderGen::shader_gen_error(err, sub));
                    }
                    let tokens = tokens.unwrap();
                    
                    if let Some(patched_const_table) = check_const_table_patch(cache, &tokens){
                        cache.code = sub.code.clone();
                        return ShaderGenResult::PatchedConstTable(patched_const_table);
                    }

                    let mut shader_ast = cache.ast.clone();
                    let mut env = cache.env.clone();

                    if let Err(err) = parse::parse(&tokens, &mut shader_ast) {
                        return ShaderGenResult::Error(ShaderGen::shader_gen_error(&err, sub));
                    }
                    
                    //parse_sub(sub, self.subs.len() - 1, &mut shader_ast) ?;
                    add_inputs_to_env(&mut env, &sub);
                    env.push_scope();
                    
                    if let Err(err) = (ShaderAnalyser {
                        builtins: &inherit_cache.builtins,
                        shader: &shader_ast,
                        env,
                        gather_all,
                        no_const_collapse: true
                    }.analyse_shader()) {
                        let sub = &self.subs[err.span.loc_id];
                        return ShaderGenResult::Error(Self::shader_gen_error(&err, sub));
                    }
                    
                    cache.prev_consts = Some(ShaderInheritConst {
                        const_table: shader_ast.const_table.borrow().clone().unwrap(),
                        const_table_spans: shader_ast.const_table_spans.borrow().clone().unwrap()
                    });
                    
                    return ShaderGenResult::ShaderAst(shader_ast)
                }
            }
        } 
        
        let mut shader_ast = ShaderAst::new();
        let mut env = Env::new();
        env.push_scope();
        
        for &ident in inherit_cache.builtins.keys() {
            let _ = env.insert_sym(Span::default(), ident, Sym::Builtin);
        }
        
        let last_index = self.subs.len() - 1;
        for (index, sub) in self.subs.iter().enumerate() {
            
            let tokens = lex::lex(sub.code.chars(), index).collect::<Result<Vec<_>, _>>();
            if let Err(err) = &tokens {
                return ShaderGenResult::Error(ShaderGen::shader_gen_error(err, sub));
            }
            let tokens = tokens.unwrap();
            
            // lets store the tokens.
            if index == last_index { // lets store the inheritance chain in our cache
                inherit_cache.map.insert(sub.loc.clone(), ShaderInheritCacheNode {
                    code: sub.code.clone(),
                    ast: shader_ast.clone(),
                    env: env.clone(),
                    tokens: tokens.clone(),
                    prev_consts: None
                });
            }
            
            if let Err(err) = parse::parse(&tokens, &mut shader_ast) {
                return ShaderGenResult::Error(ShaderGen::shader_gen_error(&err, sub));
            }
            
            add_inputs_to_env(&mut env, &sub);
        }
        
        if let Err(err) = (ShaderAnalyser {
            builtins: &inherit_cache.builtins,
            shader: &shader_ast,
            env,
            gather_all,
            no_const_collapse: false,
        }.analyse_shader()) {
            let sub = &self.subs[err.span.loc_id];
            return ShaderGenResult::Error(Self::shader_gen_error(&err, sub));
        }
        
        ShaderGenResult::ShaderAst(shader_ast)
    }
}

impl Hash for ShaderGen {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.geometry.indices.hash(state);
        for vertex in &self.geometry.vertices {
            vertex.to_bits().hash(state);
        }
        self.subs.hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum PropId {
    Texture2d(Texture2dId),
    Color(ColorId),
    Vec4(Vec4Id),
    Vec3(Vec3Id),
    Vec2(Vec2Id),
    Float(FloatId),
    Mat4(Mat4Id),
}

impl PropId {
    pub fn shader_ty(&self) -> Ty {
        match self.clone() {
            PropId::Texture2d(t) => t.into(),
            PropId::Color(t) => t.into(),
            PropId::Vec4(t) => t.into(),
            PropId::Vec3(t) => t.into(),
            PropId::Vec2(t) => t.into(),
            PropId::Float(t) => t.into(),
            PropId::Mat4(t) => t.into(),
        }
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Texture2dId(pub TypeId);

impl Into<PropId> for Texture2dId {
    fn into(self) -> PropId {
        PropId::Texture2d(self)
    }
}

impl Into<Ty> for Texture2dId {
    fn into(self) -> Ty {
        Ty::Texture2D
    }
}

impl Into<Texture2dId> for TypeId {
    fn into(self) -> Texture2dId {
        Texture2dId(self)
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct ColorId(pub TypeId);

impl Into<PropId> for ColorId {
    fn into(self) -> PropId {
        PropId::Color(self)
    }
}

impl Into<Ty> for ColorId {
    fn into(self) -> Ty {
        Ty::Vec4
    }
}

impl Into<ColorId> for TypeId {
    fn into(self) -> ColorId {
        ColorId(self)
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec4Id(pub TypeId);

impl Into<PropId> for Vec4Id {
    fn into(self) -> PropId {
        PropId::Vec4(self)
    }
}

impl Into<Vec4Id> for TypeId {
    fn into(self) -> Vec4Id {
        Vec4Id(self)
    }
}

impl Into<Ty> for Vec4Id {
    fn into(self) -> Ty {
        Ty::Vec4
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec3Id(pub TypeId);

impl Into<PropId> for Vec3Id {
    fn into(self) -> PropId {
        PropId::Vec3(self)
    }
}

impl Into<Vec3Id> for TypeId {
    fn into(self) -> Vec3Id {
        Vec3Id(self)
    }
}

impl Into<Ty> for Vec3Id {
    fn into(self) -> Ty {
        Ty::Vec3
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec2Id(pub TypeId);

impl Into<PropId> for Vec2Id {
    fn into(self) -> PropId {
        PropId::Vec2(self)
    }
}

impl Into<Vec2Id> for TypeId {
    fn into(self) -> Vec2Id {
        Vec2Id(self)
    }
}

impl Into<Ty> for Vec2Id {
    fn into(self) -> Ty {
        Ty::Vec2
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct FloatId(pub TypeId);

impl Into<PropId> for FloatId {
    fn into(self) -> PropId {
        PropId::Float(self)
    }
}

impl Into<FloatId> for TypeId {
    fn into(self) -> FloatId {
        FloatId(self)
    }
}

impl Into<Ty> for FloatId {
    fn into(self) -> Ty {
        Ty::Float
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Mat4Id(pub TypeId);

impl Into<PropId> for Mat4Id {
    fn into(self) -> PropId {
        PropId::Mat4(self)
    }
}

impl Into<Mat4Id> for TypeId {
    fn into(self) -> Mat4Id {
        Mat4Id(self)
    }
}

impl Into<Ty> for Mat4Id {
    fn into(self) -> Ty {
        Ty::Mat4
    }
}

#[macro_export]
macro_rules!uid {
    () => {{
        struct Unique {};
        std::any::TypeId::of::<Unique>().into()
    }};
}
