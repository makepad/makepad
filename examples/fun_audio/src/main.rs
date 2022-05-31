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
    use makepad_component::frame::*;
    use makepad_platform::shader::std::*;
    
    MainHeader: FoldHeader {
        walk:{
        }
        state: {
            open = {
                off = {apply: {header: {bg: {radius: vec2(3.0, 3.0)}}}}
                on = {apply: {header: {bg: {radius: vec2(3.0, 1.0)}}}}
            }
        }
        header: BoxY {
            mouse_cursor: Default,
            bg: {color: #6},
            width: Fill
            layout: {flow: Right, padding: 8, spacing: 5}
        }
    }
    
    InstrumentHeader: FoldHeader {
        header: Rect {
            mouse_cursor: Default,
            bg: {color: #5},
            width: Fill
            height: Fit
            layout: {flow: Right, padding: 8, spacing: 5}
        }
    }
    
    LayerHeader: InstrumentHeader {
        header: {
            bg: {color: #4},
        }
    }
    
    FoldablePiano: MainHeader {
        header: {
            fold_button = FoldButton {}
            label = Label {text: "Keys"}
        }
        state: {
            open ={
                off = {apply: {body: {g1 = {bg: {color: #0000}}}}}
                on = {apply: {body: {g1 = {bg: {color: #000a}}}}}
            }
        }
        body: Frame {
            layout: {flow: Overlay}
            width: Fit
            height: Fit
            Frame {
                layout: {flow: Down}
                width: Fit
                height: Fit
                piano = Piano {}
                GradientY {
                    width: Fill
                    height: 10
                    bg: {color: #000a, color2: #0000}
                }
            }
            g1 = GradientY {
                width: Fill
                height: 2
                bg: {color: #000a, color2: #0000, no_v_scroll: true}
            }
        }
    }
    
    App: {{App}} {
        window: {pass: {clear_color: (COLOR_BG_APP)}}
        audio_graph: {
            root: Mixer {
                /*c0: BasicSynth {
                    plugin: "AUMIDISynth"
                    preset_data: "21adslkfjalkwqwe"
                }*/
                c1 = Instrument {
                    //key_range: {start: 34, end: 47 shift: 30}
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
            design_mode: false,
            bg: {color: (COLOR_BG_APP)},
            walk: {width: Fill, height: Fill}
            layout: {
                padding: 8
                align: {x: 0.0, y: 0.0}
                spacing: 5.,
                flow: Flow::Down
            },
            Frame {
                layout: {flow: Right, spacing: 5.0}
                walk: {margin: {left: 60}, height: Fit}
                Button {label: "+  Band"}
                Button {label: "<"}
                Button {label: ">"}
                Solid {
                    width: Fill
                    height: 36
                    bg: {
                        const WAVE_HEIGHT: 0.15
                        const WAVE_FREQ: 0.2
                        fn pixel(self) -> vec4 {
                            let offset_y = 1.5
                            let pos2 = vec2(self.pos.x, self.pos.y + WAVE_HEIGHT * sin(WAVE_FREQ * self.pos.x * self.rect_size.x))
                            let sdf = Sdf2d::viewport(pos2 * self.rect_size)
                            sdf.clear(#2f)
                            sdf.move_to(0., self.rect_size.y * 0.5)
                            sdf.line_to(self.rect_size.x, self.rect_size.y * 0.5)
                            return sdf.stroke(#f, 1.0)
                        }
                    }
                }
            }
            Splitter {
                align: SplitterAlign::FromEnd(300)
                walk: {width: Fill, height: Fill}
                a: Frame {
                    layout: {flow: Down}
                    FoldablePiano {}
                }
                b: Box {
                    clip: true,
                    mouse_cursor: Default,
                    bg: {color: #4, radius: 3.0, border_width: 0.5, border_color: #3}
                    height: Fill
                    layout: {padding: 0.5}
                    MainHeader {
                        header: {
                            mouse_cursor: Hand,
                            label = Label {text: "Instruments"}
                        }
                        body: Frame {
                            layout: {flow: Down}
                            instrument = ? InstrumentHeader {
                                header: {
                                    layout:{align:{y:0.5}}
                                    fold_button = FoldButton {}
                                    swatch = Circle {
                                        width: 10,
                                        height: 10
                                        bg: {color: #f00}
                                    }
                                    label = Label {text: "Instrument"}
                                    //Rect {bg: {color: #f00}, width: Fill, height: 2}
                                }
                                body: Frame {
                                    layout: {flow: Down}
                                    stack = ? LayerHeader {
                                        header: {
                                            fold_button = FoldButton {}
                                            label = Label {text: "Stack item"}
                                            range = Frame {
                                                user_draw: false,
                                                layout: {flow: Right, align: {x: 1.0}, spacing: 4}
                                                Label {text: "Start"}
                                                Label {text: "D#3"}
                                                Label {text: "-"}
                                                Label {text: "End"}
                                                mylabel = Label {text: "E-4"}
                                            }
                                        }
                                        body: Frame {
                                            layout: {flow: Down}
                                            bg: {color: #f00},
                                            width: Fill
                                            height: Fit
                                            Rect {
                                                mouse_cursor: Default
                                                bg: {color: #4}
                                                width: Fill
                                                height: Fit
                                                layout: {flow: Right, padding: 8, spacing: 5, align: {y: 0.5}}
                                                label = Label {text: "Cutoff"}
                                                Slider{height:22}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
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
        
        for item in self.frame.handle_event(cx, event) {
            match item.id {
                id!(piano) => if let PianoAction::Note {is_on, note_number, velocity} = item.action.cast() {
                    self.audio_graph.send_midi_1_data(Midi1Note {
                        is_on,
                        note_number,
                        channel: 0,
                        velocity
                    }.into());
                }
                _ => ()
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
        
        match event {
            Event::Construct => {
                
                if let Some(instrument) = self.frame.add_child(cx, id!(my_instrument), ids!(instrument), live!{
                    header: {label = {text: "HELLO WORLD"}}
                }) {
                    
                    instrument.add_child(cx, id!(my_stack1), ids!(stack), live!{
                        header: {label = {text: "MyStackItem"}}
                    });
                    
                    instrument.add_child(cx, id!(my_stack2), ids!(stack), live!{
                        header: {label = {text: "MyStackItem2"}, range = {mylabel = {text: "WHEE"}}}
                    });

                    instrument.add_child(cx, id!(my_stack3), ids!(stack), live!{
                        header: {label = {text: "MyStackItem3"}, range = {mylabel = {text: "WHEE"}}}
                    });
                }
                if let Some(instrument) = self.frame.add_child(cx, id!(my_id2), ids!(instrument), live!{
                    header: {label = {text: "MyInstrument"}}
                }) {
                    instrument.add_child(cx, id!(my_stack1), ids!(stack), live!{
                        header: {label = {text: "MyStackItem"}}
                    });
                    instrument.add_child(cx, id!(my_stack2), ids!(stack), live!{
                        header: {label = {text: "MyStackItem2"}, range = {mylabel = {text: "WHEE"}}}
                    });
                }
            }
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
            _=>()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_err() {
            return;
        }
        /*
        if let Some(instrument) = self.frame.create_child(id!(instrument), id!(my_id), live!{
            header: {label = {text: "MyInstrument"}}
        }) {
            // lets render/append all instruments
            instrument.template(id!(stack), id!(my_id), live!{
                header {label = {text: "MyStack"}}
            });
        }*/
        // ok so.. what do we do.
        // we should reference a node
        // then override properties
        // as in, what if we don't do any expansions. justz
        while let Err(_child) = self.frame.draw(cx) {
            /*
            match child.id {
                id!(myframe) => if let Some(frame) = child.as_frame() {
                    while let Err(child) = self.frame.draw(cx) {
                        
                    }
                }
                id!(mylabel) => draw_apply!(frame, cx, {
                    text: "hello world"
                })
            }*/
        };
        
        self.window.end(cx);
    }
}