use makepad_widgets;
use makepad_widgets::*;
use makepad_draw_2d::*;
mod number_grid;
use makepad_widgets::imgui::*;

live_register!{
    import makepad_widgets::frame::*;
    registry Widget::*;
    App: {{App}} {
        imgui:{
            ScrollY{
                bg:{color:#5, shape:Solid}
                NumberGrid{
                }
            }
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    window: BareWindow,
    imgui: ImGUI,
}

impl App {  
    pub fn live_register(cx: &mut Cx) {
        makepad_widgets::live_register(cx);
        number_grid::live_register(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {

        if let Event::Draw(event) = event {
            return Cx2d::draw(cx, event, self, | cx, s | s.draw(cx));
        }

        self.window.handle_event(cx, event);

        let ui = self.imgui.run(cx, event);
        if ui.on_construct(){ 
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