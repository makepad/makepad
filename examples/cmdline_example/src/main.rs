use makepad_live_macros::*;
use makepad_live_compiler::liveparse::*;

struct Cx {
}

impl Cx {
    pub fn add_live_body(&mut self, body: LiveBody) {
        
        let tokens = lex::lex(sub.code.chars(), index).collect::<Result<Vec<_>, _>>();
        if let Err(err) = &tokens {
            return ShaderGenResult::Error(ShaderGen::shader_gen_error(err, sub));
        }
        let tokens = tokens.unwrap();
        
        if let Err(err) = parse::parse(&tokens, &mut shader_ast) {
            return ShaderGenResult::Error(ShaderGen::shader_gen_error(&err, sub));
        }
        // lets parse this body
        // and add all the different bits into our live system
        // this will only happen at startup like the shadercompiler
        // to make sure all deps are there.
        println!("ADD BODY");
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
