/// This is Makepad, a work-in-progress livecoding IDE for 2D Design.
// This application is nearly 100% Wasm running on webGL. NO HTML AT ALL.
// The vision is to build a livecoding / design hybrid program,
// here procedural design and code are fused in one environment.
// If you have missed 'learnable programming' please check this out:
// http://worrydream.com/LearnableProgramming/
// Makepad aims to fulfill (some) of these ideas using a completely
// from-scratch renderstack built on the GPU and Rust/wasm.
// It will be like an IDE meets a vector designtool, and had offspring.
// Direct manipulation of the vectors modifies the code, the code modifies the vectors.
// And the result can be lasercut, embroidered or drawn using a plotter.
// This means our first goal is 2D path CNC with booleans (hence the CAD),
// and not dropshadowy-gradients.

// Find the repo and more explanation at github.com/makepad/makepad.
// We are developing the UI kit and code-editor as MIT, but the full stack
// will be a cloud/native app product in a few months.

// However to get to this amazing mixed-mode code editing-designtool,
// we first have to build an actually nice code editor (what you are looking at)
// And a vector stack with booleans (in progress)
// Up next will be full multiplatform support and more visual UI.
// All of the code is written in Rust, and it compiles to native and Wasm
// Its built on a novel immediate-mode UI architecture
// The styling is done with shaders written in Rust, transpiled to metal/glsl

// for now enjoy how smooth a full GPU editor can scroll (try select scrolling the edge)
// Also the tree fold-animates and the docking panel system works.
// Multi cursor/grid cursor also works with ctrl+click / ctrl+shift+click
// press alt or escape for animated codefolding outline view!

use render::*;
use widget::*;

struct App {
    window: DesktopWindow,
    quad: Quad
}

main_app!(App);

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            window: DesktopWindow::style(cx),
            quad: Quad::style(cx)
        }
    }
}

impl App {
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        match event {
            Event::Construct => {
            },
            _ => ()
        }
        
        self.window.handle_desktop_window(cx, event);
    }
   
    fn draw_app(&mut self, cx: &mut Cx) {
        if let Err(_) = self.window.begin_desktop_window(cx) {
            return
        }
        
        self.quad.draw_quad_abs(cx, Rect{x:0.,y:0.,w:100.,h:100.});
        
        self.window.end_desktop_window(cx);
    }
}