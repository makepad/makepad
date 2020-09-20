use makepad_live_compiler::analyse::ShaderCompileOptions;
use makepad_live_compiler::livetypes::{Geometry,live_str_to_id};
use makepad_live_compiler::livestyles::{LiveStyles, LiveBody,};

struct Cx {
    live_styles: LiveStyles,
}

impl Cx {
    pub fn add_live_body(&mut self, live_body: LiveBody) {
        let mut shader_alloc_start = 0;
        if let Err(err) = self.live_styles.add_live_body(live_body, &mut shader_alloc_start) {
            eprintln!("{:?}", err);
        }
    }
}

macro_rules!live {
    ( $ cx: ident, $ code: literal) => {
        $ cx.add_live_body(LiveBody {
            file: file!().to_string().replace("\\", ","),
            module_path: module_path!().to_string(),
            line: line!() as usize,
            column: column!() as usize,
            code: $ code.to_string()
        })
    }
}

#[macro_export]
macro_rules!live_id {
    ( $ path: path) => {
        live_str_to_id(module_path!(), stringify!( $ path))
    }
}

fn main() {
    let mut cx = Cx {live_styles: LiveStyles::default()};
    live!(cx, r#"
        self::anim_default: Anim {
            play: Cut {duration: 0.1},
            tracks:[
                Float{keys:{0.0: 0.0, 1.0: 1.0}}
            ]
        }
        self::my_walk: Walk {
            width: Fix(10.),
            height: Fix(10.),
            margin: {l: -4., t: 0., r: 4., b: 0.}
        }
        self::my_layout: Layout {
            align: all(0.5),
            walk: {
                width: Compute,
                height: Compute,
                margin: all(1.0),
            },
            padding: {l: 16.0, t: 12.0, r: 16.0, b: 12.0},
        }
        self::text_style_unscaled: TextStyle {
            font: "resources/Ubuntu-R.ttf",
            font_size: 8.0,
            brightness: 1.0,
            curve: 0.6,
            line_spacing: 1.4,
            top_drop: 1.2,
            height_factor: 1.3,
        }
        self::mycolor: #ff0f;
        self::mycolor2: self::mycolor;
        self::myslider: 1.0;
        
        render::quad::shader: ShaderLib {
            struct Mp {
                x: float
            }
            impl Mp {
                fn myfn(inout self) {
                }
            }
            
            fn vertex() -> vec4 {
                return vec4(0., 0., 0., 1.);
            }
            fn pixel() -> vec4 {
                return vec4(1.0, 0.0, 0.0, 1.0);
            }
        }
        
        self::shader_bg: Shader {
            default_geometry: self::mygeom;
            geometry mygeom: vec2;
            instance myinst: vec2;
            use render::quad::shader::*;
            
            fn pixel() -> vec4 {
                let v: Mp;
                v.myfn();
                
                let x = self::myslider;
                let y = self::mycolor;
                return vec4(0., 0., 0., 1.);
            }
        }
    "#);
    
    cx.live_styles.geometries.insert(
        live_id!(self::mygeom),
        Geometry{geometry_id:0}
    );

    let options = ShaderCompileOptions {
        gather_all: false,
        create_const_table: false,
        no_const_collapse: false
    };
    cx.live_styles.enumerate_all_shaders( | shader_ast | {
        match cx.live_styles.collect_and_analyse_shader_ast(&shader_ast, options) {
            Err(err) => {
                eprintln!("{}", err);
                panic!()
            },
            Ok(_) => {
                println!("OK!");
            }
        }
    })
    
}
