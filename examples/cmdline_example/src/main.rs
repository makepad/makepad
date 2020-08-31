use makepad_live_macros::*;
use makepad_live_compiler::livetypes::*;
use makepad_live_compiler::lex;
use makepad_live_compiler::error::Error;
use makepad_live_compiler::ast::ShaderAst;
use makepad_live_compiler::parse;

struct Cx {
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveBodyError {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub msg: String,
}

impl Cx {
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
    
    pub fn live_body_error(err: Error, live_body: &LiveBody) -> LiveBodyError {
        // lets find the span info
        let start = Self::byte_to_row_col(err.span.start, &live_body.code);
        LiveBodyError {
            file: live_body.file.clone(),
            line: start.0 + live_body.line,
            col: start.1 + 1,
            len: err.span.end - err.span.start,
            msg: err.to_string(),
        }
    }
    
    pub fn add_live_body(&mut self, live_body: LiveBody)->Result<(),LiveBodyError> {
        
        let tokens = lex::lex(live_body.code.chars(), 0).collect::<Result<Vec<_>, _>>();
        if let Err(err) = tokens {
            return Err(Self::live_body_error(err, &live_body));
        }
        let tokens = tokens.unwrap();
        
        let mut live_body_ast = ShaderAst::new();
        
        if let Err(err) = parse::parse(&tokens, &mut live_body_ast) {
            return Err(Self::live_body_error(err, &live_body));
        }
        // lets parse this body
        // and add all the different bits into our live system
        // this will only happen at startup like the shadercompiler
        // to make sure all deps are there.
        println!("ADD BODY");
        return Ok(());
    }
}

fn main() {
    let mut cx = Cx {};
    
    live!(cx, {"
        self::anim_default: Anim {
            mode: Cut,
            self::shader_bg::myinst: Track {
                ease: Linear,
                0.0: 0.0,
                1.0: 1.0
            }
        },
        self::mycolor: #ff0f,
        self::myslider: 1.0,
        self::shader_bg: Shader {
            instance myinst: vec2;
            use render::quad;
            let x = self::myslider;
            let y = self::mycolor;
        }
    "});
    
    
    // lets run a const fn
    
    //
}
