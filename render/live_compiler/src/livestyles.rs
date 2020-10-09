use std::collections::{HashMap, HashSet, BTreeSet};
use crate::shaderast::{ShaderAst};
use crate::span::LiveBodyId;
use crate::lex;
use crate::token::{TokenWithSpan};
use crate::builtin::{self, Builtin};
use crate::ident::{Ident, QualifiedIdentPath};
use crate::livetypes::*;
use crate::detok::{DeTokParserImpl};
use crate::colors::Color;
use std::fmt;
use crate::error::LiveError;

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


#[derive(Clone, Debug, Default)]
pub struct LiveStyles {
    pub live_body_errors: Vec<LiveBodyError>,
    
    pub file_to_live_bodies: HashMap<String, Vec<LiveBodyId >>,
    
    pub recompute_dep: BTreeSet<LiveId>,
    pub recompute_tok: HashSet<LiveId>,
    
    pub builtins: HashMap<Ident, Builtin>,
    pub live_bodies: Vec<LiveBody>,
    pub live_bodies_contains: HashMap<LiveBodyId, HashSet<LiveId >>,
    
    pub live_depends_on: HashMap<LiveId, HashSet<LiveId >>,
    pub depends_on_live: HashMap<LiveId, HashSet<LiveId >>,
    
    pub tokens: HashMap<LiveId, LiveTokens>,
    
    // cached values from tokens
    pub floats: HashMap<LiveId, Float>,
    pub colors: HashMap<LiveId, Color>,
    pub text_styles: HashMap<LiveId, TextStyle>,
    pub layouts: HashMap<LiveId, Layout>,
    pub walks: HashMap<LiveId, Walk>,
    pub anims: HashMap<LiveId, Anim>,
    // these just 'exist' or not.
    pub styles: HashMap<LiveId, LiveStyleId>,
    pub shaders: HashMap<LiveId, Shader>,

    // things that stay around
    pub shader_alloc: HashMap<LiveId, Shader>,
    pub style_alloc: HashMap<LiveId, LiveStyleId>,
    pub style_list: Vec<LiveStyle>,
    pub shader_asts: HashMap<LiveId, ShaderAst>,
    // not accessible directly
    
    pub geometries: HashMap<LiveId, Geometry>,
    pub style_stack: Vec<LiveStyleId>,
    
    pub font_index: HashMap<Ident, Font>
}

#[derive(Clone, Debug)]
pub struct LiveTokens {
    pub qualified_ident_path: QualifiedIdentPath,
    pub tokens: Vec<TokenWithSpan>,
    pub live_tokens_type: LiveTokensType
}


#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveTokensType {
    Float,
    Color,
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
    pub floats: HashMap<LiveId, LiveId>,
    pub colors: HashMap<LiveId, LiveId>,
    pub text_styles: HashMap<LiveId, LiveId>,
    pub layouts: HashMap<LiveId, LiveId>,
    pub walks: HashMap<LiveId, LiveId>,
    pub anims: HashMap<LiveId, LiveId>,
    pub shaders: HashMap<LiveId, LiveId>,
    pub geometry: HashMap<LiveId, LiveId>,
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
    
    // we have to check if live_id exists in the on_live_id dependency tree
    pub fn check_depends_on(&self, live_id: LiveId, on_live_id: LiveId)->bool{
        if let Some(deps) = self.live_depends_on.get(&on_live_id) {
            if deps.contains(&live_id){
                return true
            }
            for dep_live_id in deps{
                if self.check_depends_on(live_id, *dep_live_id){
                    return true
                }
            }
        }
        return false
    }
    
    pub fn update_deps(&mut self, live_id: LiveId, new_deps: HashSet<LiveId>) {
        if let Some(old_deps) = self.live_depends_on.get_mut(&live_id) {
            for on_live_id in old_deps.clone() {
                if !new_deps.contains(&on_live_id) {
                    old_deps.remove(&on_live_id);
                    if let Some(set) = self.depends_on_live.get_mut(&on_live_id) {
                        set.remove(&live_id);
                    }
                }
            }
        }
        // add new deps
        let live_depends_on = self.live_depends_on.entry(live_id).or_insert_with( || HashSet::new());
        for on_live_id in new_deps {
            live_depends_on.insert(on_live_id);
            let v = self.depends_on_live.entry(on_live_id).or_insert_with( || HashSet::new());
            v.insert(live_id);
        }
    }
    
    pub fn add_recompute_when_tokens_different(&mut self, live_id: LiveId, tokens: &Vec<TokenWithSpan>) {
        if let Some(live_tokens) = self.tokens.get(&live_id) {
            if live_tokens.tokens.len() != tokens.len(){
                self.recompute_tok.insert(live_id);
                self.add_recompute_dep(live_id);
            }
            else{
                for i in 0..tokens.len(){
                    if tokens[i].token != live_tokens.tokens[i].token{
                        self.recompute_tok.insert(live_id);
                        self.add_recompute_dep(live_id);
                        break;
                    }
                }
            }
        }
        else {
            self.recompute_tok.insert(live_id);
            self.add_recompute_dep(live_id);
        }
    }
    
    pub fn add_recompute_dep(&mut self, live_id: LiveId) {
        if let Some(set) = self.depends_on_live.get(&live_id).cloned() {
            for dep_live in set {
                self.add_recompute_dep(dep_live);
            }
        }
        self.recompute_dep.insert(live_id);
        
        if let Some(tokens) = self.tokens.get(&live_id) {
            match tokens.live_tokens_type {
                LiveTokensType::Float => { 
                    self.floats.remove(&live_id);
                },
                LiveTokensType::Color => {
                    self.colors.remove(&live_id);
                },
                LiveTokensType::TextStyle => {
                    self.text_styles.remove(&live_id);
                },
                LiveTokensType::Layout => {
                    self.layouts.remove(&live_id);
                },
                LiveTokensType::Walk => {
                    self.walks.remove(&live_id);
                },
                LiveTokensType::Anim => {
                    self.anims.remove(&live_id);
                },
                LiveTokensType::Shader => {
                    self.shaders.remove(&live_id);
                },
                // special
                LiveTokensType::Style => { // what to do here
                    self.styles.remove(&live_id);
                },
                LiveTokensType::ShaderLib => { // dont do anything..
                    //self.styles.remove(&live_id);
                },
            }
        }
    }
    
    pub fn get_float(&self, live_id: LiveId, name: &str) -> f32 {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].floats.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        self.floats.get(&get_live_id).expect(&format!("Float not found {}", name)).value
    }
    
    pub fn get_color(&self, live_id: LiveId, name: &str) -> Color {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].colors.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        *self.colors.get(&get_live_id).expect(&format!("Color not found {}", name))
    }
    
    
    pub fn get_text_style(&self, live_id: LiveId, name: &str) -> TextStyle {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].text_styles.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        *self.text_styles.get(&get_live_id).expect(&format!("TextStyle not found {}", name))
    }
    
    pub fn get_layout(&self, live_id: LiveId, name: &str) -> Layout {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].layouts.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        *self.layouts.get(&get_live_id).expect(&format!("Anim not found {}", name))
    }
    
    pub fn get_walk(&self, live_id: LiveId, name: &str) -> Walk {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].walks.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        *self.walks.get(&get_live_id).expect(&format!("Walk not found {}", name))
    }
    
    pub fn get_anim(&self, live_id: LiveId, name: &str) -> Anim {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].anims.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        self.anims.get(&get_live_id).expect(&format!("Anim not found {}", name)).clone()
    }
    
    pub fn get_shader(&self, live_id: LiveId, location_hash: u64, _module_path: &str, name: &str) -> Shader {
        let mut get_live_id = live_id;
        for style_index in &self.style_stack {
            if let Some(fwd) = self.style_list[style_index.0].shaders.get(&live_id) {
                get_live_id = *fwd;
                break;
            }
        }
        
        Shader {
            shader_id: self.shaders.get(&get_live_id)
                .expect(&format!("Shader not found {}", name)).shader_id,
            location_hash
        }
    }
    
    pub fn style_begin(&mut self, style_id: LiveId, name: &str) {
        // lets fetch the style, if it doesnt exist allocate it
        if let Some(index) = self.styles.get(&style_id) {
            self.style_stack.push(*index);
        }
        else {
            panic!("Style {} not found", name)
        }
    }
    
    pub fn style_end(&mut self, style_id: LiveId, name: &str) {
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
    
    pub fn get_geometry(&self, live_id: LiveId, name: &str) -> Geometry {
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
    
    pub fn live_body_error(&self, err: LiveError) -> LiveBodyError {
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
    
    pub fn recompute_all(&mut self) {
        // flatten the recompute list by dependency
        let mut recompute_list = Vec::new();
        let mut recompute_map = HashMap::new();
        for dep_live_id in &self.recompute_dep {
            fn recur(
                recompute_list: &mut Vec<Option<LiveId >>,
                recompute_map: &mut HashMap<LiveId, usize>,
                live_id: LiveId,
                live_depends_on: &HashMap<LiveId, HashSet<LiveId >>
            ){
                if let Some(pos) = recompute_map.get(&live_id){
                    recompute_list[*pos] = None;
                }
                recompute_map.insert(live_id, recompute_list.len());
                recompute_list.push(Some(live_id));
                if let Some(on_set) = live_depends_on.get(&live_id).cloned(){
                    for on_live_id in on_set{
                        recur(recompute_list, recompute_map, on_live_id, live_depends_on);
                    } 
                }
            }
            recur(&mut recompute_list, &mut recompute_map, *dep_live_id, &self.live_depends_on);
        }

        for live_id in recompute_list.iter().rev(){
            // we have a list to recompile
            if live_id.is_none(){ 
                continue;
            }
            let live_id = live_id.unwrap();
            
            // this is needed for the borrowchecker.
            let mut swap_live_tokens = if let Some(tokens) = self.tokens.get_mut(&live_id) {
                let mut t = Vec::new();
                std::mem::swap(&mut t, &mut tokens.tokens);
                LiveTokens{
                    qualified_ident_path: tokens.qualified_ident_path,
                    tokens:t,
                    live_tokens_type:tokens.live_tokens_type
                }
            }
            else{
                continue;
            };

            match swap_live_tokens.live_tokens_type {
                LiveTokensType::Float => {
                    // we have to parse a float..
                    match DeTokParserImpl::new(&swap_live_tokens.tokens, self).parse_float() {
                        Err(err)=>{
                            
                        },
                        Ok(f)=>{
                        }
                    }

                },
                LiveTokensType::Color => {
                },
                LiveTokensType::TextStyle => {
                },
                LiveTokensType::Layout => {
                },
                LiveTokensType::Walk => {
                },
                LiveTokensType::Anim => {
                },
                LiveTokensType::Shader => {
                },
                LiveTokensType::Style => {
                },
                LiveTokensType::ShaderLib => {
                },
            }
            if let Some(tokens) = self.tokens.get_mut(&live_id) {
                std::mem::swap(&mut swap_live_tokens.tokens, &mut tokens.tokens);
            }
        }
    }
    
    pub fn add_live_body(&mut self, live_body: LiveBody) {
        let live_body_id = LiveBodyId(self.live_bodies.len());
        let v = self.file_to_live_bodies.entry(live_body.file.clone()).or_insert_with( || Vec::new());
        v.push(live_body_id);
        self.live_bodies.push(live_body);
        
        // tokenize
        let tokens = lex::lex(self.live_bodies[live_body_id.0].code.chars(), live_body_id).collect::<Result<Vec<_>, _ >> ();
        if let Err(err) = tokens {
            eprintln!("{}", self.live_body_error(err));
            panic!();
        }
        
        // parse it into depstructures
        let tokens = tokens.unwrap();
        
        if let Err(err) = DeTokParserImpl::new(&tokens, self).parse_live() {
            eprintln!("{}", self.live_body_error(err));
            panic!();
        }
    }
    
    // alright we got a new live body
    pub fn update_live_body(&mut self, file: &str, line: usize, column: usize, _code: String) -> Result<(), LiveBodyError> {
        // find the body
        
        if let Some(list) = self.file_to_live_bodies.get(file) {
            if list.len() == 0 {
                panic!()
            }
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
            let _nearest = nearest_id.unwrap();
            
            // lets tokenize, then overwrite our body
            // we then need to separately process which shaders need to recompile
            // also which shaders' liveblocks to update
            // we also know which liveblocks are dirty
        }
        return Err(LiveBodyError {
            file: file.to_string(),
            line,
            column,
            len: 0,
            message: "Cannot update live block, file not found".to_string()
        });
    }
    /*
    pub fn collect_and_analyse_shader_ast(&self, in_ast: &ShaderAst, options: ShaderCompileOptions) -> Result<(ShaderAst, Option<Geometry>), LiveBodyError> {
        let mut out_ast = ShaderAst::new();
        out_ast.shader = in_ast.shader;
        let mut visited = Vec::new();
        fn recur(visited: &mut Vec<LiveId>, in_ast: &ShaderAst, out_ast: &mut ShaderAst, live_styles: &LiveStyles) -> Result<(), LiveBodyError> {
            for use_ipws in &in_ast.uses {
                let use_live_id = use_ipws.ident_path.qualify(&in_ast.module_path).to_live_id();
                
                if !visited.contains(&use_live_id) {
                    
                    visited.push(use_live_id);
                    // it has to be in tokens. 
                    // ifso 
                    
                    if let Some(shader_lib) = live_styles.shader_libs.get(&use_live_id) {
                        recur(visited, shader_lib, out_ast, live_styles) ?;
                    }
                    else if let Some(shader) = live_styles.shaders.get(&use_live_id) {
                        recur(visited, shader, out_ast, live_styles) ?;
                    }
                    else { // error
                        return Err(live_styles.live_body_error(LiveError {
                            span: use_ipws.span,
                            message: format!("Cannot find library or shader: {}", use_ipws.ident_path)
                        }))
                    }
                }
            }
            if let Some(dg) = in_ast.default_geometry {
                out_ast.default_geometry = Some(dg);
            }
            if in_ast.debug {out_ast.debug = true};
            for decl in &in_ast.decls {
                out_ast.decls.push(decl.clone())
            }
            Ok(())
        }
        recur(&mut visited, in_ast, &mut out_ast, self) ?;
        let mut env = Env::new(self);
        
        let default_geometry = if let Some(geom_ipws) = out_ast.default_geometry {
            let live_id = geom_ipws.to_live_id(self);
            match self.geometries.get(&live_id) {
                None => {
                    return Err(self.live_body_error(LiveError {
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
            return Err(self.live_body_error(err))
        }
        
        Ok((out_ast, default_geometry))
    }*/
}


