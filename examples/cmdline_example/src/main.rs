use makepad_live_macros::*;
use makepad_live_compiler::livetypes::*;
use makepad_live_compiler::ast::ShaderAst;
use makepad_live_compiler::analyse::ShaderAnalyser;
use makepad_live_compiler::builtin::{self};
use makepad_live_compiler::env::{Sym, Env};
use makepad_live_compiler::span::{Span};
use makepad_live_compiler::generate_hlsl;

struct Cx {
    live_styles: LiveStyles,
}

impl Cx {

}

fn main() { 
    let mut cx = Cx {live_styles:LiveStyles::default()}; 
    
    let x = live!(cx, r#"
        self::anim_default: anim{
            play: Cut{duration:0.1},
            self::shader_bg::myinst: float_track{
                ease: Lin,
                0.0: 0.0,
                1.0: 1.0
            }
        },
        self::my_walk: walk{
            width: Fix(10.),
            height: Fix(10.),
            margin: {l: -4., t: 0., r: 4., b: 0.}
        },
        self::my_layout: layout {
            align: all(0.5),
            walk: {
                width: Compute,
                height: Compute,
                margin: all(1.0),
            },
            padding: {l: 16.0, t: 12.0, r: 16.0, b: 12.0},
        },
        self::text_style_unscaled:text_style{
            font: "resources/Ubuntu-R.ttf",
            font_size: 8.0,
            brightness: 1.0,
            curve: 0.6,
            line_spacing: 1.4,
            top_drop: 1.2,
            height_factor: 1.3,
        }
        self::mycolor: #ff0f,
        self::myslider: 1.0,
        
        render::quad::shader: shader_lib{
            struct Mp{
                x: float
            }
            impl Mp{
                fn myfn(inout self){
                }
            }
            
            fn vertex()->vec4{
                return vec4(0.,0.,0.,1.);
            }
            fn pixel()->vec4{
                return vec4(1.0,0.0,0.0,1.0);
            }
        }
        
        self::shader_bg: shader{
            instance myinst: vec2;
            use render::quad::shader::*; 
            
            fn pixel()->vec4{
                let v: Mp;
                v.myfn();
                
                let x = self::myslider;
                let y = self::mycolor;
                return vec4(0.,0.,0.,1.);
            }
        }
    "#);
    
    // alright now lets generate out our shader.
    // lets iterate shaders
    for (_live_id, shader_ast) in &cx.live_styles.base.shaders{
        let mut out_ast = ShaderAst::new();
        let mut visited = Vec::new();
        fn recurse(visited: &mut Vec<LiveId>, in_ast:&ShaderAst, out_ast:&mut ShaderAst, live_styles:&LiveStyles){
            for use_ident_path in &in_ast.uses{
                let use_live_id = use_ident_path.to_live_id(&in_ast.module_path);
                
                if !visited.contains(&use_live_id){
                    
                    visited.push(use_live_id);
                    if let Some(shader_lib) = live_styles.shader_libs.get(&use_live_id){
                        
                        recurse(visited, shader_lib, out_ast, live_styles);
                    }
                    else if let Some(shader) = live_styles.base.shaders.get(&use_live_id){
                        
                        recurse(visited, shader, out_ast, live_styles);
                    }
                    else{ // error somehow
                        eprintln!("Cannot find library or shader {}", use_ident_path);
                    }
                }
            }
            if in_ast.debug{out_ast.debug = true};
            for decl in &in_ast.decls{
                out_ast.decls.push(decl.clone())
            }
        }
        recurse(&mut visited, shader_ast, &mut out_ast, &cx.live_styles);

        let mut env = Env::new(&cx.live_styles);
        env.push_scope();
        let builtins = builtin::generate_builtins();

        for &ident in builtins.keys() {
            let _ = env.insert_sym(Span::default(), ident.to_ident_path(), Sym::Builtin);
        }
        
        // lets run analyse and glsl gen on it.
        let gather_all = false;
        if let Err(err) = (ShaderAnalyser {
            builtins: &builtins,
            shader: &out_ast,
            env: &mut env,
            gather_all,
            no_const_collapse: false,
        }.analyse_shader()) {
            // error
            let err = cx.live_styles.live_body_error(err.span.live_body_id, err);
            println!("{:?}", err);
        }
        
        // lets generate a glsl shader
        let shader = generate_hlsl::generate_shader(&out_ast, &env, false);
        //let fragment = generate_glsl::generate_fragment_shader(&out_ast, &env, false);
        println!("{}",shader);
    }
    
    if let Err(x) = x{
        println!("{:?}", x);
    }
}
