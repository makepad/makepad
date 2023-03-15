use crate::makepad_widgets::*;

//#[cfg(feature = "nightly")]

live_design!{
    import makepad_widgets::frame::*;
    registry Widget::*;
    App = {{App}} {
        ui: {
            walk: {width: Fill, height: Fill},
            
            <Mandelbrot> {
                walk: {width: Fill, height: Fill}
            }
        }
    }
}
app_main!(App);

#[derive(Live, LiveHook)]
#[live_design_with{
    crate::makepad_widgets::live_design(cx);
    crate::mandelbrot::live_design(cx);
}]
pub struct App {
    ui: FrameRef,
    window: DesktopWindow,
}

impl App{
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_not_redrawing() {
            return;
        }
        while self.ui.draw(cx).is_not_done() {};
        self.window.end(cx);
    }
}

impl AppMain for App {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        
        self.window.handle_event(cx, event);
        
        self.ui.handle_event(cx, event);
    }
}
