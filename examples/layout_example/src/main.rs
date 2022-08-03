use makepad_component::*;
use makepad_component::imgui::*;
use makepad_platform::*;

live_register!{
    import makepad_component::frame::*;
    registry FrameComponent::*;
    App: {{App}} {
        imgui: {
            // the imgui object contains the DSL templates it spawns
            button =? Button{ 
            }
            my_red_button =? Button{
               color: #f000 
            }
            Image{
                image:d"resources/makepad_logo_WIP.png"
            }
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    imgui: ImGUI,
    window: DesktopWindow,
}

impl App {  
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.window.handle_event(cx, event, &mut |_,_|{});
        
        if let Event::Draw(draw_event) = event{
            return self.draw(&mut Cx2d::new(cx, draw_event))
        }
        
        // the ImGUI component exposes an immediate mode component API
        // this runs in the event handling flow
        // this does have (eventual) scalability issues, so its more of a convenience api
        // do note this is not the actual 'drawing' flow. its simply a code based way
        // to spawn the UI in the retained UI Frame system.
        let mut ui = self.imgui.run(cx, event); 
        
        for i in 0..10{
            if ui.button("Button").was_clicked(){
                log!("CLicked {}",i);
            }
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx, None).not_redrawing() {
            return;
        }
        // here we actually draw the imgui UI tree.
        while self.imgui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}