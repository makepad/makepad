use std::collections::{HashMap, HashSet, BTreeMap, BTreeSet};
use crate::shaderast::ShaderAst;
use crate::span::LiveBodyId;
use crate::lex;
use crate::analyse::{ShaderCompileOptions, ShaderAnalyser};
use crate::env::Env;
use crate::span::Span;
use crate::token::{TokenWithSpan};
use crate::builtin::{self, Builtin};
use crate::ident::{Ident, IdentPath, QualifiedIdentPath};
use crate::livetypes::*;
use crate::detok::{DeTokParserImpl};
use crate::ty::{TyLit, TyExpr, TyExprKind};
use crate::math::*;
use std::fmt;
use crate::error::LiveError;
use std::cell::RefCell;

#[derive(Clone, Debug)]
pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveBodyError {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub len: usize,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveStyleId(usize);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveChangeType {
    Recompile,
    UpdateValue,
}

#[derive(Clone, Debug, Default)]
pub struct LiveDrawInput {
    pub cls: String,
    pub uniforms: Vec<LiveDrawInputDef>,
    pub instances: Vec<LiveDrawInputDef>,
    pub textures: Vec<LiveDrawInputDef>,
}

impl LiveDrawInput {
    pub fn add_uniform(&mut self, modpath: &str, cls: &str, name: &str, ty_expr: TyExpr) {
        if let TyExprKind::Lit {ty_lit, ..} = ty_expr.kind {
            if ty_lit == TyLit::Texture2D {
                self.textures.push(LiveDrawInputDef::new(modpath, cls, name, ty_expr));
                return
            }
        }
        self.uniforms.push(LiveDrawInputDef::new(modpath, cls, name, ty_expr));
    }
    
    pub fn add_instance(&mut self, modpath: &str, cls: &str, name: &str, ty_expr: TyExpr) {
        self.instances.push(LiveDrawInputDef::new(modpath, cls, name, ty_expr));
    }
}

#[derive(Clone, Debug)]
pub struct LiveDrawInputDef {
    pub qualified_ident_path: QualifiedIdentPath,
    pub ident: Ident,
    pub ty_expr: TyExpr
}

impl LiveDrawInputDef {
    pub fn new(modpath: &str, cls: &str, name: &str, ty_expr: TyExpr) -> LiveDrawInputDef {
        let ident = IdentPath::from_three(Ident::new("self"), Ident::new(cls), Ident::new(name));
        Self {
            qualified_ident_path: ident.qualify(modpath),
            ident: Ident::new(name),
            ty_expr
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LiveStyles {
    pub file_to_live_bodies: HashMap<String, Vec<LiveBodyId >>,
    pub live_body_to_file: HashMap<LiveBodyId, String>,
    
    pub live_access_errors: RefCell<Vec<String >>,
    
    // change sets
    pub changed_live_bodies: BTreeSet<LiveBodyId>,
    
    pub changed_deps: BTreeMap<LiveItemId, LiveChangeType>,
    pub changed_tokens: HashSet<LiveItemId>,
    
    pub changed_shaders: HashMap<LiveItemId, LiveChangeType>,
    
    pub draw_inputs: HashMap<LiveItemId, LiveDrawInput>,
    
    pub builtins: HashMap<Ident, Builtin>,
    pub live_bodies: Vec<LiveBody>,
    pub live_bodies_contains: HashMap<LiveBodyId, HashSet<LiveItemId >>,
    pub item_in_live_body: HashMap<LiveItemId, LiveBodyId>,
    pub live_bodies_items: HashMap<LiveBodyId, Vec<LiveItemId >>,
    pub live_depends_on: HashMap<LiveItemId, HashSet<LiveItemId >>,
    pub depends_on_live: HashMap<LiveItemId, HashSet<LiveItemId >>,
    
    pub tokens: HashMap<LiveItemId, LiveTokens>,
    
    // cached values from tokens
    pub floats: HashMap<LiveItemId, Float>,
    pub vec2s: HashMap<LiveItemId, Vec2>,
    pub vec3s: HashMap<LiveItemId, Vec3>,
    pub vec4s: HashMap<LiveItemId, Vec4>,
    pub text_styles: HashMap<LiveItemId, TextStyle>,
    pub layouts: HashMap<LiveItemId, Layout>,
    pub walks: HashMap<LiveItemId, Walk>,
    pub anims: HashMap<LiveItemId, Anim>,
    pub styles: HashMap<LiveItemId, LiveStyleId>,
    pub shaders: HashMap<LiveItemId, Shader>,
    
    // things that stay around
    pub shader_alloc: HashMap<LiveItemId, Shader>,
    
    pub style_alloc: HashMap<LiveItemId, LiveStyleId>,
    pub style_list: Vec<LiveStyle>,
    pub style_stack: Vec<LiveStyleId>,
    pub shader_asts: HashMap<LiveItemId, ShaderAst>,
    
    pub geometries: HashMap<LiveItemId, Geometry>,
    
    pub font_index: HashMap<Ident, Font>
}

#[derive(Clone, Debug)]
pub struct LiveTokens {
    pub ident_path: IdentPath,
    pub qualified_ident_path: QualifiedIdentPath,
    pub tokens: Vec<TokenWithSpan>,
    pub live_tokens_type: LiveTokensType
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveTokensType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    TextStyle,
    Layout,
    Walk,
    Anim,
    Style,
    ShaderLib,
    Shader,
}

#[derive(Clone, Debug, Default)]
pub struct LiveStyle {
    pub remap: HashMap<LiveItemId, LiveItemId>,
}

impl fmt::Display for LiveBodyError {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {} {} - {}",
            self.file,
            self.line,
            self.column,
            self.message
        )
    }
}

impl LiveStyles {
    pub fn new() -> Self {
        Self {
            builtins: builtin::generate_builtins(),
            ..Self::default()
        }
    }
    
    pub fn live_item_id_to_string(&self, live_item_id: LiveItemId) -> Option<String> {
        if let Some(tokens) = self.tokens.get(&live_item_id) {
            return Some(tokens.qualified_ident_path.to_string())
        }
        return None
    }
    
    // we have to check if live_id exists in the on_live_id dependency tree
    pub fn check_depends_on(&self, live_item_id: LiveItemId, on_live_item_id: LiveItemId) -> bool {
        if let Some(deps) = self.live_depends_on.get(&on_live_item_id) {
            if deps.contains(&live_item_id) {
                return true
            }
            for dep_live_id in deps {
                if self.check_depends_on(live_item_id, *dep_live_id) {
                    return true
                }
            }
        }
        return false
    }
    
    
    pub fn update_deps(&mut self, live_item_id: LiveItemId, new_deps: HashSet<LiveItemId>) {
        if let Some(old_deps) = self.live_depends_on.get_mut(&live_item_id) {
            for on_live_item_id in old_deps.clone() {
                if !new_deps.contains(&on_live_item_id) {
                    old_deps.remove(&on_live_item_id);
                    if let Some(set) = self.depends_on_live.get_mut(&on_live_item_id) {
                        set.remove(&live_item_id);
                    }
                }
            }
        }
        // add new deps
        let live_depends_on = self.live_depends_on.entry(live_item_id).or_insert_with( || HashSet::new());
        for on_live_id in new_deps {
            
            
            live_depends_on.insert(on_live_id);
            
            let v = self.depends_on_live.entry(on_live_id).or_insert_with( || HashSet::new());
            v.insert(live_item_id);
        }
    }
    
    pub fn add_changed_deps(&mut self, live_item_id: LiveItemId, new_tokens: &Vec<TokenWithSpan>, live_tokens_type: LiveTokensType) {
        
        if let Some(live_tokens) = self.tokens.get(&live_item_id) {
            // if the type is
            let live_dep_change = if live_tokens_type == LiveTokensType::Shader
                || live_tokens_type == LiveTokensType::ShaderLib
                || live_tokens.live_tokens_type != live_tokens_type {
                LiveChangeType::Recompile
            }
            else {
                LiveChangeType::UpdateValue
            };
            if live_tokens.tokens.len() != new_tokens.len() {
                self.changed_tokens.insert(live_item_id);
                self._add_changed_deps_recursive(live_item_id, live_dep_change);
            }
            else {
                for i in 0..new_tokens.len() {
                    if new_tokens[i].token != live_tokens.tokens[i].token {
                        self.changed_tokens.insert(live_item_id);
                        self._add_changed_deps_recursive(live_item_id, live_dep_change);
                        break;
                    }
                }
            }
        }
        else {
            // its always a recompile
            self.changed_tokens.insert(live_item_id);
            self._add_changed_deps_recursive(live_item_id, LiveChangeType::Recompile);
        }
    }
    
    // return all the live items for a certain file, in order
    pub fn get_live_items_for_file(&self, file: &str) -> Vec<LiveItemId> {
        //println!("get_live_items_for_file {}", file);
        let mut ret = Vec::new();
        if let Some(live_bodies_id) = self.file_to_live_bodies.get(file) {
            //println!("HERE {}", live_bodies_id.len());
            for live_body_id in live_bodies_id {
                if let Some(live_items) = self.live_bodies_items.get(live_body_id) {
                    for live_item_id in live_items {
                        ret.push(*live_item_id);
                    }
                }
            }
        }
        ret
    }
    
    pub fn remove_live_id(&mut self, live_item_id: LiveItemId) {
        self._add_changed_deps_recursive(live_item_id, LiveChangeType::Recompile);
        self.clear_computed_live_id(live_item_id);
        self.tokens.remove(&live_item_id);
        self.style_alloc.remove(&live_item_id);
        self.shader_asts.remove(&live_item_id);
    }
    
    pub fn register_draw_input(&mut self, live_item_id: LiveItemId, live_draw_input: LiveDrawInput) {
        self.draw_inputs.insert(live_item_id, live_draw_input);
    }
    
    pub fn add_direct_value_change(&mut self, live_item_id: LiveItemId) {
        if let Some(set) = self.depends_on_live.get(&live_item_id).cloned() {
            for dep_live in set {
                self._add_changed_deps_recursive(dep_live, LiveChangeType::UpdateValue);
            }
        }
    }
    
    pub fn _add_changed_deps_recursive(&mut self, live_item_id: LiveItemId, live_change_type: LiveChangeType) {
        if let Some(set) = self.depends_on_live.get(&live_item_id).cloned() {
            for dep_live in set {
                self._add_changed_deps_recursive(dep_live, live_change_type);
            }
        }
        if let Some(prev_change) = self.changed_deps.get(&live_item_id) {
            if *prev_change == LiveChangeType::Recompile {
                return
            }
        }
        self.changed_deps.insert(live_item_id, live_change_type);
    }
    
    pub fn clear_computed_live_id(&mut self, live_item_id: LiveItemId) {
        if let Some(tokens) = self.tokens.get(&live_item_id) {
            match tokens.live_tokens_type {
                LiveTokensType::Float => {
                    self.floats.remove(&live_item_id);
                },
                LiveTokensType::Vec2 => {
                    self.vec2s.remove(&live_item_id);
                },
                LiveTokensType::Vec3 => {
                    self.vec3s.remove(&live_item_id);
                },
                LiveTokensType::Vec4 => {
                    self.vec4s.remove(&live_item_id);
                },
                LiveTokensType::TextStyle => {
                    self.text_styles.remove(&live_item_id);
                },
                LiveTokensType::Layout => {
                    self.layouts.remove(&live_item_id);
                },
                LiveTokensType::Walk => {
                    self.walks.remove(&live_item_id);
                },
                LiveTokensType::Anim => {
                    self.anims.remove(&live_item_id);
                },
                LiveTokensType::Shader => {
                    self.shaders.remove(&live_item_id);
                },
                LiveTokensType::Style => { // what to do here
                    self.styles.remove(&live_item_id);
                },
                LiveTokensType::ShaderLib => { // dont do anything..
                },
            }
        }
    }
    
    pub fn find_remap(&self, live_item_id: LiveItemId) -> LiveItemId {
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].remap.get(&live_item_id) {
                return *fwd;
            }
        }
        live_item_id
    }
    
    pub fn get_float(&self, live_item_id: LiveItemId, name: &str) -> f32 {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.floats.get(&live_item_id) {
            return v.value;
        }
        self.live_access_errors.borrow_mut().push(format!("Float not found {}", name));
        return 0.0;
    }
    
    pub fn get_vec2(&self, live_item_id: LiveItemId, name: &str) -> Vec2 {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.vec2s.get(&live_item_id) {
            return *v;
        }
        self.live_access_errors.borrow_mut().push(format!("Vec2 not found {}", name));
        return Vec2::all(0.);
    }
    
    pub fn get_vec3(&self, live_item_id: LiveItemId, name: &str) -> Vec3 {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.vec3s.get(&live_item_id) {
            return *v
        }
        self.live_access_errors.borrow_mut().push(format!("Vec3 not found {}", name));
        return Vec3::all(0.);
    }
    
    pub fn get_vec4(&self, live_item_id: LiveItemId, name: &str) -> Vec4 {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.vec4s.get(&live_item_id) {
            return *v
        }
        self.live_access_errors.borrow_mut().push(format!("Vec4 not found {}", name));
        return Vec4 {x: 0.0, y: 1.0, z: 0.0, w: 1.0};
    }
    
    pub fn get_text_style(&self, live_item_id: LiveItemId, name: &str) -> TextStyle {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.text_styles.get(&live_item_id) {
            return *v
        }
        self.live_access_errors.borrow_mut().push(format!("TextStyle not found {}", name));
        return TextStyle {
            font: Font {font_id: 0},
            font_size: 8.0,
            brightness: 1.0,
            curve: 0.6,
            line_spacing: 1.4,
            top_drop: 1.2,
            height_factor: 1.3,
        }
    }
    
    pub fn get_layout(&self, live_item_id: LiveItemId, name: &str) -> Layout {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.layouts.get(&live_item_id) {
            return *v
        }
        self.live_access_errors.borrow_mut().push(format!("Layout not found {}", name));
        return Layout::default()
    }
    
    pub fn get_walk(&self, live_item_id: LiveItemId, name: &str) -> Walk {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.walks.get(&live_item_id) {
            return *v
        }
        self.live_access_errors.borrow_mut().push(format!("Walk not found {}", name));
        return Walk::default();
    }
    
    pub fn get_anim(&self, live_item_id: LiveItemId, name: &str) -> Anim {
        let live_item_id = self.find_remap(live_item_id);
        if let Some(v) = self.anims.get(&live_item_id) {
            return v.clone()
        }
        self.live_access_errors.borrow_mut().push(format!("Anim not found {}", name));
        return Anim::default()
    }
    
    pub fn get_shader(&self, live_item_id: LiveItemId, location_hash: u64, _module_path: &str, name: &str) -> Shader {
        let live_item_id = self.find_remap(live_item_id);
        let shader = self.shaders.get(&live_item_id);
        if let Some(shader) = shader {
            Shader {
                shader_id: shader.shader_id,
                location_hash
            }
        }
        else {
            //panic!();
            eprintln!("Shader not found {}", name);
            Shader {
                shader_id: 0,
                location_hash
            }
        }
    }
    
    pub fn get_default_shader_overload(&self, base: Shader, live_item_id: LiveItemId, _module_path: &str, name: &str) -> Shader {
        if base.shader_id == 0 {
            self.get_shader(live_item_id, base.location_hash, _module_path, name)
        }
        else {
            base
        }
    }
    
    pub fn style_begin(&mut self, style_id: LiveItemId, name: &str) {
        // lets fetch the style, if it doesnt exist allocate it
        if let Some(index) = self.styles.get(&style_id) {
            self.style_stack.push(*index);
        }
        else {
            panic!("Style {} not found", name)
        }
    }
    
    pub fn style_end(&mut self, style_id: LiveItemId, name: &str) {
        // lets fetch the style, if it doesnt exist allocate it
        if let Some(index) = self.styles.get(&style_id) {
            if self.style_stack.len() == 0 {
                panic!("Style stack empty at style_end of {}", name)
            }
            if *index != self.style_stack.pop().unwrap() {
                panic!("Style stack disaligned at style_end of {}", name)
            }
        }
        else {
            panic!("Style {} not found at style_end", name)
        }
    }
    
    pub fn get_geometry(&self, live_id: LiveItemId, name: &str) -> Geometry {
        if let Some(geometry) = self.geometries.get(&live_id) {
            return *geometry
        }
        panic!("Geometry not found {}", name);
    }
    
    pub fn get_or_insert_font_by_ident(&mut self, ident: Ident) -> Font {
        if let Some(font) = self.font_index.get(&ident) {
            return *font
        }
        else {
            let id = self.font_index.len();
            let font = Font {font_id: id};
            self.font_index.insert(ident, font);
            return font
        }
    }
    
    pub fn live_error_to_live_body_error(&self, err: LiveError) -> LiveBodyError {
        let live_body = &self.live_bodies[err.span.live_body_id.0];
        fn byte_to_row_col(byte: usize, source: &str) -> (usize, usize) {
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
        // lets find the span info
        let start = byte_to_row_col(err.span.start, &live_body.code);
        LiveBodyError {
            file: live_body.file.clone(),
            line: start.0 + live_body.line,
            column: start.1 + 1,
            len: err.span.end - err.span.start,
            message: err.to_string(),
        }
    }
    
    pub fn process_changed_live_bodies(&mut self, errors: &mut Vec<LiveBodyError>) {
        let mut changed_live_bodies = BTreeSet::new();
        std::mem::swap(&mut changed_live_bodies, &mut self.changed_live_bodies);
        
        for live_body_id in changed_live_bodies {
            // tokenize
            let tokens = lex::lex(self.live_bodies[live_body_id.0].code.chars(), live_body_id).collect::<Result<Vec<_>, _ >> ();
            if let Err(err) = tokens {
                errors.push(self.live_error_to_live_body_error(err));
                continue;
            }
            
            // parse it into depstructures
            let tokens = tokens.unwrap();
            if let Err(err) = DeTokParserImpl::new(&tokens, self).parse_live() {
                errors.push(self.live_error_to_live_body_error(err));
                continue;
            }
        }
    }
    
    pub fn process_changed_deps(&mut self, errors: &mut Vec<LiveBodyError>) {
        // flatten the recompute list by dependency
        let mut recompute_list = Vec::new();
        let mut recompute_map = HashMap::new();
        
        // ok we have a bunch of changed deps,
        // now what we need is to flatten it according to what depends on what.
        for (dep_live_id, _) in &self.changed_deps {
            fn recur(
                recompute_list: &mut Vec<Option<LiveItemId >>,
                recompute_map: &mut HashMap<LiveItemId, usize>,
                live_id: LiveItemId,
                depens_on_live: &HashMap<LiveItemId, HashSet<LiveItemId >>
            ) {
                if let Some(pos) = recompute_map.get(&live_id) {
                    recompute_list[*pos] = None;
                }
                recompute_map.insert(live_id, recompute_list.len());
                recompute_list.push(Some(live_id));
                if let Some(on_set) = depens_on_live.get(&live_id).cloned() {
                    for on_live_id in on_set {
                        recur(recompute_list, recompute_map, on_live_id, depens_on_live);
                    }
                }
            }
            recur(&mut recompute_list, &mut recompute_map, *dep_live_id, &self.depends_on_live);
        }
        
        for live_id in recompute_list {
            // we have a list to recompile
            if live_id.is_none() {
                continue;
            }
            let live_id = live_id.unwrap();
            
            self.clear_computed_live_id(live_id);
            // this is needed for the borrowchecker.
            let mut swap_live_tokens = if let Some(tokens) = self.tokens.get_mut(&live_id) {
                let mut t = Vec::new();
                std::mem::swap(&mut t, &mut tokens.tokens);
                LiveTokens {
                    ident_path: tokens.ident_path,
                    qualified_ident_path: tokens.qualified_ident_path,
                    tokens: t,
                    live_tokens_type: tokens.live_tokens_type
                }
            }
            else {
                continue;
            };
            
            let live_change_type = *self.changed_deps.get(&live_id).unwrap();
            
            match swap_live_tokens.live_tokens_type {
                LiveTokensType::Float => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_float() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.floats.insert(live_id, v);}
                    }
                },
                LiveTokensType::Vec2 => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_vec2() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.vec2s.insert(live_id, v);}
                    }
                },
                LiveTokensType::Vec3 => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_vec3() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.vec3s.insert(live_id, v);}
                    }
                },
                LiveTokensType::Vec4 => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_vec4() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.vec4s.insert(live_id, v);}
                    }
                },
                LiveTokensType::TextStyle => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_text_style() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.text_styles.insert(live_id, v);}
                    }
                },
                LiveTokensType::Layout => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_layout() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.layouts.insert(live_id, v);}
                    }
                },
                LiveTokensType::Walk => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_walk() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.walks.insert(live_id, v);}
                    }
                },
                LiveTokensType::Anim => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_anim() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {self.anims.insert(live_id, v);}
                    }
                },
                LiveTokensType::Shader => {
                    if self.changed_tokens.contains(&live_id) {
                        match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_shader(swap_live_tokens.qualified_ident_path) {
                            Err(err) => {
                                errors.push(self.live_error_to_live_body_error(err));
                            },
                            Ok(v) => {
                                let id = self.shader_alloc.len();
                                self.shader_alloc.entry(live_id).or_insert_with( || Shader {shader_id: id, location_hash: 0});
                                self.shader_asts.insert(live_id, v);
                            }
                        }
                    }
                    if let Some(shader) = self.shader_alloc.get(&live_id) {
                        self.shaders.insert(live_id, *shader);
                    }
                    self.changed_shaders.insert(live_id, live_change_type);
                }
                LiveTokensType::ShaderLib => {
                    if self.changed_tokens.contains(&live_id) {
                        match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_shader(swap_live_tokens.qualified_ident_path) {
                            Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                            Ok(v) => {self.shader_asts.insert(live_id, v);}
                        }
                    }
                },
                LiveTokensType::Style => {
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_style() {
                        Err(err) => {errors.push(self.live_error_to_live_body_error(err));},
                        Ok(v) => {
                            // allocate style, write it
                            if let Some(existing) = self.style_alloc.get(&live_id) {
                                self.style_list[existing.0] = v;
                                self.styles.insert(live_id, *existing);
                            }
                            else {
                                let id = LiveStyleId(self.style_list.len());
                                self.style_alloc.insert(live_id, id);
                                self.style_list.push(v);
                                self.styles.insert(live_id, id);
                            }
                        }
                    }
                },
            }
            if let Some(tokens) = self.tokens.get_mut(&live_id) {
                std::mem::swap(&mut swap_live_tokens.tokens, &mut tokens.tokens);
            }
        }
        // clear the sets
        self.changed_tokens.clear();
        self.changed_deps.clear();
    }
    
    
    pub fn add_live_body(&mut self, live_body: LiveBody) {
        let live_body_id = LiveBodyId(self.live_bodies.len());
        let file_live_bodies = self.file_to_live_bodies.entry(live_body.file.clone()).or_insert_with( || Vec::new());
        
        let seek_line = live_body.line;
        let live_bodies = &self.live_bodies;
        match file_live_bodies.binary_search_by( | probe | {
            live_bodies[probe.as_index()].line.cmp(&seek_line)
        }) {
            Err(pos) => { // not found
                file_live_bodies.insert(pos, live_body_id)
            }
            Ok(_pos) => {
                panic!("Called add_live_body twice for the same body! {} {}", live_body.file, live_body.line)
            }
        }
        
        self.live_body_to_file.insert(live_body_id, live_body.file.clone());
        
        self.live_bodies.push(live_body);
        self.changed_live_bodies.insert(live_body_id);
    }
    
    // alright we got a new live body
    pub fn update_live_body(&mut self, file: &str, line: usize, column: usize, code: String) -> Result<LiveBodyId, ()> {
        
        if let Some(list) = self.file_to_live_bodies.get(file) {
            // find the nearest block
            let mut nearest = std::usize::MAX;
            let mut nearest_id = None;
            for live_body_id in list {
                let other_line = self.live_bodies[live_body_id.0].line;
                let dist = if other_line > line {other_line - line}else {line - other_line};
                if dist < nearest {
                    nearest_id = Some(live_body_id);
                    nearest = dist;
                }
            }
            let nearest_id = nearest_id.unwrap();
            let live_body = &mut self.live_bodies[nearest_id.0];
            if live_body.code != code {
                self.changed_live_bodies.insert(*nearest_id);
                live_body.code = code;
            }
            live_body.line = line;
            live_body.column = column;
            return Ok(*nearest_id)
        }
        return Err(())
    }
    
    
    
    pub fn collect_and_analyse_shader(&self, live_item_id: LiveItemId, options: ShaderCompileOptions) -> Result<(ShaderAst, Option<Geometry>), LiveBodyError> {
        let mut out_ast = ShaderAst::default();
        let mut visited = Vec::new();
        
        let in_ast = match self.shader_asts.get(&live_item_id) {
            Some(ast) => ast,
            None => {
                return Err(self.live_error_to_live_body_error(LiveError {
                    span: Span::default(),
                    message: format!("Cannot find library or shader")
                }))
            }
        };
        
        fn recur(visited: &mut Vec<LiveItemId>, in_ast: &ShaderAst, out_ast: &mut ShaderAst, live_styles: &LiveStyles) -> Result<(), LiveBodyError> {
            for use_ipws in &in_ast.uses {
                let module_path = &live_styles.live_bodies[in_ast.live_body_id.0].module_path;
                let use_live_id = use_ipws.ident_path.qualify(module_path).to_live_item_id();
                
                if !visited.contains(&use_live_id) {
                    
                    visited.push(use_live_id);
                    if let Some(shader_lib) = live_styles.shader_asts.get(&use_live_id) {
                        recur(visited, shader_lib, out_ast, live_styles) ?;
                    }
                    else { // error
                        return Err(live_styles.live_error_to_live_body_error(LiveError {
                            span: use_ipws.span,
                            message: format!("Cannot find library or shader: {}", use_ipws.ident_path)
                        }))
                    }
                }
            }
            if let Some(dg) = in_ast.default_geometry {
                out_ast.default_geometry = Some(dg);
            }
            if let Some(di) = in_ast.draw_input {
                out_ast.draw_input = Some(di);
            }
            if in_ast.debug {out_ast.debug = true};
            for decl in &in_ast.decls {
                out_ast.decls.push(decl.clone())
            }
            out_ast.live_body_id = in_ast.live_body_id;
            Ok(())
        }
        recur(&mut visited, in_ast, &mut out_ast, self) ?;
        // lets expand
        
        let mut env = Env::new(self);
        let span = Span {live_body_id: out_ast.live_body_id, start: 0, end: 0};
        if let Err((span, msg)) = out_ast.convert_draw_input_to_decls(self, span) {
            return Err(self.live_error_to_live_body_error(LiveError {
                span: span,
                message: msg
            }));
        }
        
        let default_geometry = if let Some(geom_ipws) = out_ast.default_geometry {
            let live_id = geom_ipws.to_live_item_id(self);
            match self.geometries.get(&live_id) {
                None => {
                    return Err(self.live_error_to_live_body_error(LiveError {
                        span: geom_ipws.span,
                        message: format!("Cannot find default geometry {}", geom_ipws.ident_path)
                    }))
                }
                Some(geometry) => Some(*geometry)
            }
        }
        else {
            None
        };
        
        if let Err(err) = (ShaderAnalyser {
            builtins: &self.builtins,
            shader: &out_ast,
            env: &mut env,
            options,
        }.analyse_shader()) {
            return Err(self.live_error_to_live_body_error(err))
        }
        
        Ok((out_ast, default_geometry))
    }
}


