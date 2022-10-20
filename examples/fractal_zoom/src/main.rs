#![cfg_attr(feature = "nightly", feature(portable_simd))]

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_draw_2d::*;
mod mandelbrot;

#[cfg(feature = "nightly")]
mod mandelbrot_simd;

live_design!{
    import makepad_widgets::frame::*;
    registry Widget::*;
    App = {{App}} {
        ui: {
            walk:{width: Fill, height: Fill},
            
            <Mandelbrot> {
                walk:{width: Fill, height: Fill}
            }
        }
    }
}
main_app!(App);
 
#[derive(Live, LiveHook)]
pub struct App {
    ui: FrameRef,
    window: BareWindow,
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        mandelbrot::live_design(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        
        self.window.handle_event(cx, event);
        
        self.ui.handle_event(cx, event);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        while self.ui.draw(cx).is_not_done(){};
        self.window.end(cx);
    }
}