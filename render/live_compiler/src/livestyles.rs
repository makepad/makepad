use std::collections::{HashMap};
use crate::shaderast::{ShaderAst};
use crate::span::LiveBodyId;
use crate::lex;
use crate::parse;
use crate::env::Env;
use crate::builtin::{self, Builtin};
use crate::analyse::{ShaderAnalyser, ShaderCompileOptions};
use crate::ident::{Ident, QualifiedIdentPath};
use crate::livetypes::*;
use crate::colors::Color;
use std::fmt;
use crate::error::LiveError;

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveBodyError {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub len: usize,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct LiveStyles {
    pub collision_check: HashMap<LiveId, QualifiedIdentPath>,
    pub builtins: HashMap<Ident, Builtin>,
    pub live_bodies: Vec<LiveBody>,
    pub shader_libs: HashMap<LiveId, ShaderAst>,
    pub geometries: HashMap<LiveId, Geometry>,
    pub base: LiveStyle,
    pub style_list: Vec<LiveStyle>,
    pub style_map: HashMap<LiveId, usize>,
    pub style_stack: Vec<usize>,
    pub font_index: HashMap<Ident, Font>
}

#[derive(Clone, Debug, Default)]
pub struct LiveStyle {
    pub floats: HashMap<LiveId, f32>,
    pub colors: HashMap<LiveId, Color>,
    pub text_styles: HashMap<LiveId, TextStyle>,
    pub layouts: HashMap<LiveId, Layout>,
    pub walks: HashMap<LiveId, Walk>,
    pub anims: HashMap<LiveId, Anim>,
    pub shaders: HashMap<LiveId, ShaderAst>,
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
  
    
    pub fn get_style_mut(&mut self, live_id: &Option<LiveId>) -> &mut LiveStyle {
        if let Some(live_id) = live_id {
            let style_list = &mut self.style_list;
            let id = self.style_map.entry(*live_id).or_insert_with( || {
                let id = style_list.len();
                style_list.push(LiveStyle::default());
                id 
            });
            &mut self.style_list[*id]
        }
        else {
            return &mut self.base
        }
    }
    
    pub fn get_shader(&self, live_id: LiveId, location_hash: u64, module_path:&str,  name: &str) -> Shader {
        for style_index in &self.style_stack {
            if let Some(shader_ast) = self.style_list[*style_index].shaders.get(&live_id) {
                if let Some(shader) = &shader_ast.shader {
                    return Shader {
                        shader_id: shader.shader_id,
                        location_hash
                    }
                }
            }
        }

        Shader {
            shader_id: self.base.shaders.get(&live_id)
                .expect(&format!("Shader not found {}", name)).shader
                .expect(&format!("Shader not compiled {}", name)).shader_id,
            location_hash
        }
    }
    
    pub fn style_begin(&mut self, style_id: LiveId, name: &str) {
        // lets fetch the style, if it doesnt exist allocate it
        if let Some(index) = self.style_map.get(&style_id) {
            self.style_stack.push(*index);
        }
        else {
            panic!("Style {} not found", name)
        }
    }
    
    pub fn style_end(&mut self, style_id: LiveId, name: &str) {
        // lets fetch the style, if it doesnt exist allocate it
        if let Some(index) = self.style_map.get(&style_id) {
            if self.style_stack.len() == 0{
                panic!("Style stack empty at style_end of {}", name)
            }
            if *index != self.style_stack.pop().unwrap(){
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
    
    pub fn get_color(&self, live_id: LiveId, name: &str) -> Color {
        for style_index in &self.style_stack {
            if let Some(color) = self.style_list[*style_index].colors.get(&live_id) {
                return *color
            }
        }
        *self.base.colors.get(&live_id).expect(&format!("Color not found {}", name))
    }
    
    
    pub fn get_float(&self, live_id: LiveId, name: &str) -> f32 {
        for style_index in &self.style_stack {
            if let Some(color) = self.style_list[*style_index].floats.get(&live_id) {
                return *color
            }
        }
        *self.base.floats.get(&live_id).expect(&format!("Float not found {}", name))
    }

    pub fn get_text_style(&self, live_id: LiveId, name: &str) -> TextStyle {
        for style_index in &self.style_stack {
            if let Some(text_style) = self.style_list[*style_index].text_styles.get(&live_id) {
                return *text_style
            }
        }
        *self.base.text_styles.get(&live_id).expect(&format!("TextStyle not found {}", name))
    }

    pub fn get_walk(&self, live_id: LiveId, name: &str) -> Walk {
        for style_index in &self.style_stack {
            if let Some(walk) = self.style_list[*style_index].walks.get(&live_id) {
                return *walk
            }
        }
        *self.base.walks.get(&live_id).expect(&format!("Walk not found {}", name))
    }

    pub fn get_anim(&self, live_id: LiveId, name: &str) -> Anim {
        for style_index in &self.style_stack {
            if let Some(anim) = self.style_list[*style_index].anims.get(&live_id) {
                return anim.clone()
            }
        }
        self.base.anims.get(&live_id).expect(&format!("Anim not found {}", name)).clone()
    }
    
    pub fn get_layout(&self, live_id: LiveId, name: &str) -> Layout {
        for style_index in &self.style_stack {
            if let Some(layout) = self.style_list[*style_index].layouts.get(&live_id) {
                return *layout
            }
        }
        *self.base.layouts.get(&live_id).expect(&format!("Anim not found {}", name))
    }
    
    pub fn get_or_insert_font_by_ident(&mut self, ident:Ident)->Font{
         if let Some(font) = self.font_index.get(&ident){
            return *font
        }
        else{
            let id = self.font_index.len();
            let font =  Font{font_id:id};
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
    
    
    pub fn add_live_body(&mut self, live_body: LiveBody, shader_alloc_start: &mut usize) -> Result<(), LiveBodyError> {
        let live_body_id = LiveBodyId(self.live_bodies.len());
        self.live_bodies.push(live_body.clone());
        
        let tokens = lex::lex(live_body.code.chars(), live_body_id).collect::<Result<Vec<_>, _ >> ();
        if let Err(err) = tokens {
            return Err(self.live_body_error(err));
        }
        let tokens = tokens.unwrap();
        
        if let Err(err) = parse::parse(&tokens, &live_body.module_path, self, shader_alloc_start) {
            return Err(self.live_body_error(err));
        }
        
        return Ok(());
    }
    
    pub fn enumerate_all_shaders<F>(&self, mut cb: F)
    where F: FnMut(&ShaderAst)
    {
        for (_live_id, shader_ast) in &self.base.shaders {
            cb(shader_ast)
        }
        for style in &self.style_list {
            for (_live_id, shader_ast) in &style.shaders {
                cb(shader_ast)
            }
        }
    }
    
    pub fn collect_and_analyse_shader_ast(&self, in_ast: &ShaderAst, options: ShaderCompileOptions) -> Result<(ShaderAst, Option<Geometry>), LiveBodyError> {
        let mut out_ast = ShaderAst::new();
        out_ast.shader = in_ast.shader;
        let mut visited = Vec::new();
        fn recur(visited: &mut Vec<LiveId>, in_ast: &ShaderAst, out_ast: &mut ShaderAst, live_styles: &LiveStyles) -> Result<(), LiveBodyError> {
            for use_ipws in &in_ast.uses {
                let use_live_id = use_ipws.ident_path.qualify(&in_ast.module_path).to_live_id();
                
                if !visited.contains(&use_live_id) {
                    
                    visited.push(use_live_id);
                    if let Some(shader_lib) = live_styles.shader_libs.get(&use_live_id) {
                        recur(visited, shader_lib, out_ast, live_styles) ?;
                    }
                    else if let Some(shader) = live_styles.base.shaders.get(&use_live_id) {
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
    }
}


