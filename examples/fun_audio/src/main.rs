pub use makepad_component::{self, *};
pub use makepad_platform::{
    *,
    audio::*,
    midi::*,
    live_atomic::*,
};

mod piano;
mod audio;
use crate::piano::*;
use crate::audio::*;
use crate::audio::iron_fish::*;

live_register!{
    registry AudioComponent::*;
    registry FrameComponent::*;
    import makepad_component::theme::*;
    import makepad_component::frame::*;
    import makepad_platform::shader::std::*;
    
    MainHeader: FoldHeader {
        walk: {
        }
        state: {
            open = {
                off = {apply: {header: {bg: {radius: vec2(3.0, 3.0)}}}}
                on = {apply: {header: {bg: {radius: vec2(3.0, 1.0)}}}}
            }
        }
        header: BoxY {
            cursor: Default,
            bg: {color: #6},
            width: Fill
            layout: {flow: Right, padding: 8, spacing: 5}
        }
    }
    
    InstrumentHeader: FoldHeader {
        header: Rect {
            cursor: Default,
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
            open = {
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
    
    InstrumentSlider: Rect {
        bg: {color: #4}
        width: Fill
        height: Fit
        layout: {flow: Right, padding: 8, spacing: 5, align: {y: 0.5}}
        slider = Slider {
            label: "CutOff1"
            height: 22
        }
    }
    
    TextInputTest: Rect {
        bg: {color: #4}
        width: Fill
        height: Fit
        layout: {flow: Right, padding: 0}
        textbox = TextInput {
            text: "Hello WOrld"
        }
    }
    
    IronFishUI: InstrumentHeader {
        header: {
            layout: {align: {y: 0.5}}
            fold_button = FoldButton {}
            swatch = Circle {
                width: 10
                height: 10
                bg: {color: #f00}
            }
            label = Label {text: "IronFish"}
        }
        body: Frame {
            layout: {flow: Down}
            stack = LayerHeader {
                header: {
                    fold_button = FoldButton {}
                    label = Label {text: "Stack item", walk: {width: Fill}}
                }
                body: Frame {
                    layout: {flow: Down}
                    bg: {color: #f00},
                    width: Fill
                    height: Fit
                    InstrumentSlider {
                        slider = {
                            bind: "filter1.cutoff"
                            min: 0.0
                            max: 1.0
                            label: "Filter Cutoff"
                        }
                    }
                    InstrumentSlider {
                        slider = {
                            bind: "filter1.resonance"
                            min: 0.02
                            max: 1.0
                            label: "Filter Resonance"
                        }
                    }
                    InstrumentSlider {
                        slider = {
                            bind: "filter1.envelope_amount"
                            min: 0.0
                            max: 1.0
                            label: "Envelope Amount"
                        }
                    }
                    InstrumentSlider {
                        slider = {
                            bind: "osc1.detune"
                            min: 0.0
                            max: 10.0
                            label: "Osc1 detune"
                        }
                    }
                    InstrumentSlider {
                        slider = {
                            bind: "osc2.detune"
                            min: 0.0
                            max: 10.0
                            label: "Osc2 detune"
                        }
                    }
                    TextInputTest {}
                    
                }
            }
        }
    }
    
    App: {{App}} {
        window: {pass: {clear_color: (COLOR_BG_APP)}}
        audio_graph: {
            root: Mixer {
                c1 = Instrument {
                    IronFish {
                    }
                    //key_range: {start: 34, end: 47 shift: 30}
                    /*
                    AudioUnitEffect {
                        plugin: "AUReverb2"
                    }
                    AudioUnitInstrument {
                        plugin: "Kontakt"
                    }*/
                }
            }
        }
        imgui: {
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
                Button {text: "+  Band"}
                Button {text: "<"}
                Button {text: ">"}
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
                    /*Image{
                        bg:{shape:Box, radius:30,color:#ff0, image_scale:vec2(1.0,0.2)},
                        height:Fill,
                        width:Fill
                    }*/
                }
                b: Box {
                    clip: true,
                    cursor: Default,
                    bg: {color: #4, radius: 3.0, border_width: 0.5, border_color: #3}
                    height: Fill
                    layout: {padding: 0.5}
                    MainHeader {
                        header: {
                            cursor: Hand,
                            label = Label {text: "Instruments"}
                        }
                        body: Frame {
                            layout: {flow: Down}
                            instrument = IronFishUI {}
                        }
                    }
                }
            }
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    imgui: ImGUI,
    audio_graph: AudioGraph,
    window: BareWindow,
    data: f32,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
        crate::audio::live_register(cx);
        crate::piano::live_register(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(de) = event {
            return self.draw(&mut Cx2d::new(cx, de));
        }
        
        self.window.handle_event(cx, event);
        self.audio_graph.handle_event_iter(cx, event);
        
        let mut ui = self.imgui.run(cx, event); 

        if ui.on_construct(){
            let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
            ui.bind_read(&iron_fish.settings.live_read());
        }
        
        // fetch ui binding deltas
        for delta in ui.on_bind_deltas(){
            let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
            iron_fish.settings.apply_over(ui.cx, &delta);
        }
        
        let piano = ui.piano(ids!(piano));
        
        for note in piano.on_notes(){
            self.audio_graph.send_midi_1_data(Midi1Note {
                channel: 0,
                is_on: note.is_on,
                note_number: note.note_number,
                velocity: note.velocity
            }.into());
        }
        
        for note in ui.on_midi_1_notes(){
            piano.set_note(ui.cx, note.is_on, note.note_number)
        }
        
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.imgui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}