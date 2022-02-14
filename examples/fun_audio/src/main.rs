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
        const FS_ROOT: ""
        desktop_window: {pass: {clear_color: (COLOR_BG_APP)}}

        audio_graph: {
            root: Mixer {
                /*c0: BasicSynth {
                    plugin: "AUMIDISynth"
                    preset_data: "21adslkfjalkwqwe"
                }*/
                c1:Instrument {
                    key_range: {start: 34, end: 47 shift: 30}
                    s0 : PluginEffect {
                        plugin: "AUReverb2"
                    }
                    s1 : PluginMusicDevice {
                        plugin: "AUMIDISynth"
                    }
                }
            }
        }
        
        frame:{
            HBox{
                Button{}
                Button{}
            }
        }
        
        scroll_view: {
            h_show: true,
            v_show: true,
            view: {
                layout: {
                    line_wrap: LineWrap::NewLine
                }
            }
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    piano: Piano,
    audio_graph: AudioGraph,
    frame: Frame,
    desktop_window: DesktopWindow,
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
        
        self.desktop_window.handle_event(cx, event);
        self.scroll_view.handle_event(cx, event);
        
        for action in self.audio_graph.handle_event(cx, event) {
            match action {
                AudioGraphAction::Midi1Data(data) => if let Midi1Event::Note(note) = data.decode() {
                    self.piano.set_note(cx, note.is_on, note.note_number)
                }
            }
        };
        
        //let instrument = self.instrument.clone();
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
        };
        
        match event {
            Event::KeyDown(ke) => {
                if let KeyCode::F1 = ke.key_code {
                }
                if let KeyCode::Escape = ke.key_code {
                }
                
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
                self.piano.set_key_focus(cx);
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.desktop_window.begin(cx, None).is_err() {
            return;
        }
        if self.scroll_view.begin(cx).is_ok() {
            self.piano.draw(cx);
            self.scroll_view.end(cx);
        }
        
        self.desktop_window.end(cx);
    }
}