/// This is Makepad, a work-in-progress livecoding IDE for 2D and VR.
// press alt or escape for animated codefolding outline view!
// Find the repo and more explanation at github.com/makepad/makepad.
// All makepad does today is we use it ourselves to develop itself.
// If you have webVR on a Quest or otherwise you can walk around the IDE in 3D

// This application is nearly 100% Wasm running on webGL. NO HTML AT ALL.
// The vision is to build a livecoding / design hybrid program,
// here procedural design and code are fused in one environment.
// If you have missed 'learnable programming' please check this out:
// http://worrydream.com/LearnableProgramming/
// Makepad aims to fulfill (some) of these ideas using a completely
// from-scratch renderstack built on the GPU and Rust/wasm.

// However to get to this amazing mixed-mode code editing-designtool,
// we first have to build an actually nice code editor (what you are looking at)
// And a vector stack with booleans (in progress)
// All of the code is written in Rust, and it compiles to native and Wasm
// Its built on a novel immediate-mode UI architecture
// The styling is done with shaders written in Rust, transpiled to metal/glsl

// for now enjoy how smooth a full GPU editor can scroll (try select scrolling the edge)
// Also the tree fold-animates and the docking panel system works.
// Multi cursor/grid cursor also works with ctrl+click / ctrl+shift+click

use render::*;

struct App {
    window: Window,
    pass: Pass,
    color_texture: Texture,
    main_view: View<NoScrollBar>,
    quad: Quad
}

main_app!(App);

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        Self {
            window: Window::style(cx),
            pass: Pass::default(),
            color_texture: Texture::default(),
            main_view: View::style(cx),
            quad: Quad {
                shader: cx.add_shader(Self::def_quad_shader(), "quad"),
                ..Style::style(cx)
            },
        }
    }
}

impl App {
    pub fn def_quad_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast !({
            
            fn pixel() -> vec4 { 
                return vec4(1.0,0.0,0.0,1.0);
            }
        }))
    }
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        match event {
            Event::Construct => {
            },
            _ => ()
        }
    }
   
    fn draw_app(&mut self, cx: &mut Cx) {
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, &mut self.color_texture, Some(color256(30,30,30)));

        let _ = self.main_view.begin_view(cx, Layout::default());
        
        self.quad.draw_quad_abs(cx, Rect{x:30.,y:30.,w:100.,h:100.});
        
        self.main_view.end_view(cx);
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
