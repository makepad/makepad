use makepad_component::*;
use makepad_platform::*;

live_register!{
    import makepad_component::frame::*;
    registry FrameComponent::*;
    App: {{App}} {
        shape: {shape: Solid}
        imgui: {
            button =? Button{ // default template
            }
            my_red_button =? Button{
               color: #f000 
            }
            // here is our root frame
        }
    }
}
main_app!(App);

#[derive(Clone, Debug)]
pub enum ToUI {
    TestMessage(Vec<u32>),
}

#[derive(Clone, Debug)]
pub enum FromUI {
    TestMessage(Vec<u32>),
}


#[derive(Live, LiveHook)]
pub struct App {
    imgui: ImGUI,
    shape: DrawShape,
    window: DesktopWindow,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.window.handle_event(cx, event);
        
        // give frame an immediate mode gui
        let mut ui = self.imgui.run(cx, event); // run our frame_ui as immediate mode wrapper

        for i in 0..10{
            if ui.button(&format!("Hello world {}", i)).was_clicked(){
                log!("CLicked {}",i);
            }
        }

        ui.end();
/*
        if let PianoAction::Note {is_on, note_number, velocity} = ui.piano_id(ids!(piano)).action(){
            self.audio_graph.send_midi_1_data(Midi1Note {
                is_on,
                note_number,
                channel: 0,
                velocity
            }.into());
        }*/
        
        match event {
            Event::Construct => {
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        
        if self.window.begin(cx, None).not_redrawing() {
            return;
        }
        while self.imgui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}