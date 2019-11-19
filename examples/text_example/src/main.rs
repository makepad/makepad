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
    desktop_window: DesktopWindow,
    menu: Menu,
    menu_signal: Signal
}

main_app!(App);

impl App {
    pub fn proto(cx: &mut Cx) -> Self {
        let menu_signal = cx.new_signal();
        Self {
            desktop_window: DesktopWindow::proto(cx),
            menu: Menu::main(vec![
                Menu::sub("Makepad", "M", vec![
                    Menu::item("Quitter!", "q", false, menu_signal, 0),
                    Menu::line(),
                    Menu::item("Thingie", "q", false, menu_signal, 1),
                ]),
                Menu::sub("Edit", "e", vec![
                    Menu::item("Copy", "c", false, menu_signal, 2),
                    Menu::line(),
                    Menu::item("Paste", "v", false, menu_signal, 3),
                ])
            ]),
            menu_signal,
        }
    }
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);
        match event {
            Event::Construct => {
            },
            Event::Signal(se) => if self.menu_signal.is_signal(se){
                println!("CLICKED {}", se.value);
            },
            _ => ()
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, Some(&self.menu)).is_err() {
            return
        };
        
        self.desktop_window.end_desktop_window(cx);
    }
}
