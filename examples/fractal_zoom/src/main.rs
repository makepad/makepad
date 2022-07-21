#![feature(portable_simd)]
use makepad_component::*;
use makepad_platform::*;
mod mandelbrot;

#[cfg(any(not(target_arch = "wasm32"), target_feature = "simd128"))]
mod mandelbrot_simd;

live_register!{
    use makepad_component::frame::*;
    use FrameComponent::*;
    App: {{App}} {
        frame: {
            width: Fill
            height: Fill
            Mandelbrot{
                walk:{width: Fill, height: Fill}
            }
        }
    }
}
main_app!(App);
 
#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    window: DesktopWindow,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
        mandelbrot::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_event(cx, event);
        
        for _ in self.frame.handle_event(cx, event) {
        }
        
        match event {
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx, None).is_err() {
            return;
        }
        while let Err(_child) = self.frame.draw(cx){
        };
        self.window.end(cx);
    }
}