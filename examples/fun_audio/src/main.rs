pub use makepad_component::{self, *};
pub use makepad_platform::{self, *};

mod piano;
use crate::piano::*;

mod plugin_music_device;
mod audio_graph;
use crate::audio_graph::*;

#[macro_use]
mod audio_registry;

live_register!{
    use makepad_component::frame::Frame;
    use makepad_component::button::Button;
    App: {{App}} {
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
    
    desktop_window: DesktopWindow,
    scroll_view: ScrollView,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
        crate::plugin_music_device::live_register(cx);
        crate::audio_graph::live_register(cx);
        crate::audio_registry::live_register(cx);
        crate::piano::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        
        self.desktop_window.handle_event(cx, event);
        self.scroll_view.handle_event(cx, event);
        self.audio_graph.handle_event_with_fn(cx, event, &mut |_cx, _action|{});
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