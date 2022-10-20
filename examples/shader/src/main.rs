use makepad_widgets;
use makepad_widgets::*;
use makepad_draw_2d::*;

live_design!{
    import makepad_widgets::frame::*;
    registry Widget::*;
    App = {{App}} {
        ui: {
            walk: {width: Fill, height: Fill},
            bg: {
                shape: Solid
                fn pixel(self) -> vec4 {
                    let pixel = self.rect_size * self.pos;
                    if mod (pixel.y * 2.0, 2) >= 1.0 {
                        return #f
                    }
                    else {
                        return #1
                    }
                }
            }
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    window: BareWindow,
    ui: FrameRef,
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        
        self.window.handle_event(cx, event);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done(){};
        
        self.ui.redraw(cx);
        
        self.window.end(cx);
    }
}