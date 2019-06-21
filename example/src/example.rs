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
pub mod cap_winmf;
pub use crate::cap_winmf::*;
use std::sync::Mutex;

#[derive(Default)]
struct Mandelbrot {
    texture: Texture,
    num_threads: usize,
    width: usize,
    zoom: f64,
    height: usize,
    image_signal: Signal,
    image_front: Mutex<Vec<u32>>
}

impl Mandelbrot {
    fn init(&mut self, cx: &mut Cx) {
        // lets start a mandelbrot thread that produces frames
        self.image_signal = cx.new_signal();
        self.width = 3840;
        self.height = 2160;
        self.zoom = 1.0;
        self.num_threads = 32;
        self.texture.set_desc(cx, Some(TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(self.width),
            height: Some(self.height),
            multisample: None
        }));
        self.image_thread();
    }
    
    fn image_thread(&mut self) {
        fn draw_mandel(x: f64, y: f64) -> u32 {
            let mut rr = x;
            let mut ri = y;
            let mut n = 0;
            while n < 80 {
                let mut tr = rr * rr - ri * ri + x;
                let mut ti = 2.0 * rr * ri + y;
                rr = tr;
                ri = ti;
                n = n + 1;
                if rr * ri > 5.0 {
                    return n;
                }
            }
            return n;
        }
        
        let mut zoom = self.zoom;
        self.zoom /= 1.01;
        // lets spawn fractal.height over 32 threads
        let num_threads = self.num_threads;
        let width = self.width;
        let height = self.height;
        let fwidth = self.width as f64;
        let fheight = self.height as f64;
        let center_x = 0;
        let center_y = 0;
        let chunk_height = height / num_threads;
        let mut thread_pool = scoped_threadpool::Pool::new(self.num_threads as u32);
        let mut image_back:Vec<u32> = Vec::new();
        let mut image_front = &mut self.image_front;
        let mut image_signal = self.image_signal.clone();
        std::thread::spawn(move || {
            loop {
                thread_pool.scoped( | scope | {
                    image_back.resize(width * height, 0);
                    let mut iter = image_back.chunks_mut((chunk_height * width) as usize);
                    for i in 0..num_threads {
                        let thread_num = i;
                        let slice = iter.next().unwrap();
                        
                        scope.execute(move || {
                            let start = thread_num * chunk_height as usize;
                            for y in start..(start + chunk_height) {
                                for x in 0..width {
                                    let c_re = (x as f64 - fwidth * 0.5) * 4.0 / fwidth;
                                    let c_im = (y as f64 - fheight * 0.5) * 4.0 / fheight;
                                    let mandel = draw_mandel(c_re, c_im) * 3;
                                    slice[(x + (y - start) * width) as usize] = mandel | (mandel << 8) | (mandel << 16);
                                }
                            }
                        })
                    }
                    
                });
                // alright we have a texture ready. now what.
                let mut front = image_front.lock().unwrap();
                std::mem::swap(&mut (*front), &mut image_back);
                Cx::send_signal(image_signal, 0);
            }
        });
    }
    
    fn handle_signal(&mut self, cx:&mut Cx, event: &Event)->bool{
        if let Event::Signal(se) = event {
            if self.image_signal.is_signal(se) { // we haz new texture
                let mut front = self.image_front.lock().unwrap();
                let cxtex = &mut cx.textures[self.texture.texture_id.unwrap()];
                std::mem::swap(&mut (*front), &mut cxtex.image_u32);
                cxtex.update_image = true;
                return true
            }
        }
        false
    }
}

struct App {
    view: View<ScrollBar>,
    window: DesktopWindow,
    buttons: Elements<u64, Button, Button>,
    text: Text,
    quad: Quad,
    blit: Blit,
    cap: CapWinMF,
    mandel: Mandelbrot,
    clickety: u64
}

main_app!(App, "Example");

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            mandel: Mandelbrot::default(),
            cap: CapWinMF::default(),
            window: DesktopWindow::style(cx),
            view: View {
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar::style(cx)),
                ..Style::style(cx)
            },
            quad: Quad {
                shader: cx.add_shader(App::def_quad_shader(), "App.quad"),
                ..Style::style(cx)
            },
            text: Text ::style(cx),
            blit: Blit::style(cx),
            buttons: Elements::new(Button {
                ..Style::style(cx)
            }),
            clickety: 0
        }
    }
}

impl App {
    fn def_quad_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_rect(0., 0., w, h);
                df_fill(vec4(pos.x, pos.y, sin(pos.x), 1.));
                df_move_to(0., 0.);
                df_line_to(w, h);
                return df_stroke(color, 4.);
            }
        }))
    }
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        if let Event::Construct = event {
            self.cap.init(cx, 0, 3840, 2160, 30.0);
            self.mandel.init(cx);
        }
        
        self.window.handle_desktop_window(cx, event);
        
        self.view.handle_scroll_bars(cx, event);
        
        if let CapEvent::NewImage = self.cap.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
        
        if self.mandel.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
        /*
        for btn in self.buttons.iter() {
            match btn.handle_button(cx, event) {
                ButtonEvent::Clicked => {
                    // boop
                    self.clickety += 1;
                    cx.redraw_child_area(Area::All);
                },
                _ => ()
            }
        }*/
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        
        let _ = self.window.begin_desktop_window(cx);
        
        let _ = self.view.begin_view(cx, Layout {
            padding: Padding {l: 0., t: 0., r: 0., b: 0.},
            ..Default::default()
        });
        
        self.view.redraw_view_area(cx);
        self.blit.draw_blit_walk(cx, &self.mandel.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
        
        
        
        /*
        for i in 0..100 {
            
            self.buttons.get_draw(cx, i, | _cx, templ | {templ.clone()})
                .draw_button_with_label(cx, &format!("Btn {}", i));
            
            if i % 10 == 9 {
                cx.turtle_new_line()
            }
        }*/
        
        //cx.turtle_new_line();
        //self.text.draw_text(cx, &format!("It works {}", self.clickety));
        //self.quad.draw_quad_walk(cx, Bounds::Fix(100.), Bounds::Fix(100.), Margin {l: 15., t: 0., r: 0., b: 0.});
        
        self.view.end_view(cx);
        
        self.window.end_desktop_window(cx);
    }
}