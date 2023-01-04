use makepad_audio_graph;
use makepad_audio_graph::makepad_widgets;
use makepad_widgets::*;
use makepad_draw::*;

live_design!{
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    registry Widget::*;
    App = {{App}} {
        ui: {
            walk: {width: Fill, height: Fill},
            draw_bg: {
                shape:Rect
                fn pixel(self) -> vec4 {
                    //return #f00
                    return Pal::premul(#3)
                    //return vec4(1.0,0.0,0.0,1.0);
                }
            }
            piano = <Piano>{walk:{abs_pos:vec2(10,40)}}
        }
    } 
} 
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    window: DesktopWindow,
    ui: FrameRef,
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_graph::live_design(cx);
    }
    
    pub fn start_audio_output(&mut self, cx:&mut Cx){
        cx.start_audio_input(move | _time, _input_buffer | {
            println!("GOT BUFFER!")
        });   
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        if let Event::Construct = event{
            self.start_audio_output(cx);
        }
        self.ui.handle_event(cx, event);
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