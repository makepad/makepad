use makepad_component;
use makepad_component::*;
use makepad_draw_2d::*;
use makepad_component::imgui::*;

live_register!{
    import makepad_component::frame::*;
    registry FrameComponent::*;
    App: {{App}} {imgui: {Frame {
        walk: {width: Fill, height: Fill},
        bg: {
            shape:Solid
            fn pixel(self) -> vec4 {
                let pixel = self.rect_size * self.pos;
                if mod(pixel.y * 2.0, 2)>=1.0{
                    return #f
                }
                else{
                    return #1
                }
            }
        }
    }}}
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    window: BareWindow,
    imgui: ImGUI,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return Cx2d::draw(cx, event, self, | cx, s | s.draw(cx));
        }
        
        self.window.handle_event(cx, event);
        
        let ui = self.imgui.run(cx, event);
        if ui.on_construct() {
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        // ok so. we should d
        // here we actually draw the imgui UI tree.
        while let Some(_) = self.imgui.draw(cx).into_not_done() {
            // we have to draw our own Uid. which in this case is simply
        };
        self.imgui.root_frame().redraw(cx);
        self.window.end(cx);
    }
}