pub use makepad_component::{self, *};
pub use makepad_platform::{self, *, audio::*, midi::*};

mod piano;
mod audio;
use crate::piano::*;
use crate::audio::*;

live_register!{
    use AudioComponent::*;
    use FrameComponent::*;
    use makepad_component::theme::*;
    App: {{App}} {
        window: {pass: {clear_color: (COLOR_BG_APP)}}
        audio_graph: {
            root: Mixer {
                /*c0: BasicSynth {
                    plugin: "AUMIDISynth"
                    preset_data: "21adslkfjalkwqwe"
                }*/
                c1:Instrument {
                    key_range: {start: 34, end: 47 shift: 30}
                    PluginEffect {
                        plugin: "AUReverb2"
                    }
                    PluginMusicDevice {
                        plugin: "AUMIDISynth"
                    }
                }
            }
        }
        
        frame: {
            color: #3
            padding: 30
            width: Size::Fill
            height: Size::Fill
            align: {x: 0.0, y: 0.5}
            spacing: 30.,
            flow: Flow::Down,
            piano:= Piano{}
            Frame{
                flow: Flow::Right,
                spacing: 30.
                Frame {color: #0f0, width: Size::Fill, height: 40}
                Frame {
                    color: #0ff
                    padding: 10
                    flow: Flow::Down
                    width: Size::Fit
                    height: 300
                    spacing: 10
                    Frame {color: #00f, width: 40, height: Size::Fill}
                    Frame {color: #f00, width: 40, height: 40}
                    Frame {color: #00f, width: 40, height: 40}
                }
                Frame {color: #f00, width: 40, height: 40}
                Frame {color: #f0f, width: Size::Fill, height: 60}
                Frame {color: #f00, width: 40, height: 40}
            }
        }
        
        scroll_view: {
            h_show: true,
            v_show: true,
            view: {}
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    audio_graph: AudioGraph,
    window: BareWindow,
    scroll_view: ScrollView,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
        crate::audio::live_register(cx);
        crate::piano::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        
        //self.desktop_window.handle_event(cx, event);
        self.scroll_view.handle_event(cx, event);
        
        for item in self.frame.handle_event(cx, event){
            match item.id{
                id!(piano)=>if let PianoAction::Note{is_on, note_number, velocity} = item.action.cast(){
                    self.audio_graph.send_midi_1_data(Midi1Note {
                        is_on,
                        note_number,
                        channel: 0,
                        velocity
                    }.into());
                }
                _=>()
            }
        }
        
        for action in self.audio_graph.handle_event(cx, event) {
            match action {
                AudioGraphAction::Midi1Data(data) => if let Midi1Event::Note(note) = data.decode() {
                    let piano = self.frame.child_mut::<Piano>(id!(piano)).unwrap();
                    piano.set_note(cx, note.is_on, note.note_number)
                }
            }
        };
        
        //let instrument = self.instrument.clone();
        /*
        for action in self.piano.handle_event(cx, event) {
            match action {
                PianoAction::Note {is_on, note_number, velocity} => {
                    self.audio_graph.send_midi_1_data(Midi1Note {
                        is_on,
                        note_number,
                        channel: 0,
                        velocity
                    }.into());
                }
            }
        };*/
        
        match event {
            Event::KeyDown(ke) => {
                if let KeyCode::F1 = ke.key_code {
                }
                if let KeyCode::Escape = ke.key_code {
                }
                
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
                //self.piano.set_key_focus(cx);
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_err() {
            return;
        }
        
        //self.piano.draw(cx);
        while self.frame.draw(cx).is_ok(){};
        /*
        if self.scroll_view.begin(cx).is_ok() {
            self.scroll_view.end(cx);
        }*/
        
        
        self.window.end(cx);
    }
}