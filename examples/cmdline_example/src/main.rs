use makepad_live_macros::*;
use makepad_live_compiler::livetypes::*;
use makepad_live_compiler::lex;
use makepad_live_compiler::error::LiveError;
use makepad_live_compiler::parse;

struct Cx {
    live_styles: LiveStyles
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
    
    pub fn live_body_error(err: LiveError, live_body: &LiveBody) -> LiveBodyError {
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
        
        if let Err(err) = parse::parse(&tokens, &live_body.module_path, &mut self.live_styles) {
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
        self::shader_bg: shader{
            instance myinst: vec2;
            
            use render::quad::shader::*; 
            
            fn pixel(){
                let x = self::myslider;
                let y = self::mycolor;
            }
        }
    "#);
    if let Err(x) = x{
        println!("{:?}", x);
    }
}
