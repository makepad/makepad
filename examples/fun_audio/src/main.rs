pub use makepad_component;
pub use makepad_component::makepad_platform;
pub use makepad_platform::makepad_math;
pub use makepad_media;

use makepad_component::*;
use makepad_component::imgui::*;
use makepad_draw_2d::*;
use makepad_media::*;
use makepad_media::audio_graph::*;

mod display_audio;
mod piano;
mod iron_fish;

use crate::iron_fish::*;
use crate::piano::*;
use crate::display_audio::*;

live_register!{
    registry AudioComponent::*;
    registry FrameComponent::*;
    import makepad_component::theme::*;
    import makepad_component::frame::*;
    import makepad_draw_2d::shader::std::*;
    
    MainHeader: FoldHeader {
        state: {
            open = {
                off = {apply: {header: {bg: {radius: vec2(3.0, 3.0)}}}}
                on = {apply: {header: {bg: {radius: vec2(3.0, 1.0)}}}}
            }
        }
        header: BoxY {
            cursor: Default,
            bg: {color: #6},
            walk: {width: Fill, height: Fit},
            layout: {flow: Right, padding: 8, spacing: 5}
        }
    }
    
    InstrumentHeader: FoldHeader {
        header: Rect {
            cursor: Default,
            bg: {color: #5},
            walk: {width: Fill, height: Fit}
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
            walk: {width: Fit, height: Fit}
            Frame {
                layout: {flow: Down}
                walk: {width: Fit, height: Fit},
                piano = Piano {}
                GradientY {
                    walk: {width: Fill, height: 10}
                    bg: {color: #000a, color2: #0000}
                }
            }
            g1 = GradientY {
                walk: {width: Fill, height: 2}
                bg: {color: #000a, color2: #0000}
            }
        }
    }
    
    ElementBox: Frame {
        bg: {color: #4}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: 8, spacing: 5}
    }
    
    InstrumentSlider: ElementBox {
        slider = Slider {
            label: "CutOff1"
            walk: {height: 22}
        }
    }
    InstrumentBipolarSlider: ElementBox {
        slider = Slider {
            label: "CutOff1"
            walk: {height: 22}
            slider: {
                fn pixel(self) -> vec4 {
                    let slider_height = 7;
                    let nub_size = mix(3, 4, self.hover);
                    let nubbg_size = 18
                    
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    
                    let slider_bg_color = mix(#38, #30, self.focus);
                    
                    let slider_color = mix(mix(#5, #68, self.hover), #68, self.focus);
                    let nub_color = mix(mix(#8, #f, self.hover), mix(#c, #f, self.drag), self.focus);
                    let nubbg_color = mix(#eee0, #8, self.drag);
                    
                    sdf.rect(0, self.rect_size.y - slider_height, self.rect_size.x, slider_height)
                    sdf.fill(slider_bg_color);
                    
                    sdf.rect(self.rect_size.x / 2, self.rect_size.y - slider_height, self.slide_pos * (self.rect_size.x - nub_size) + nub_size, slider_height)
                    sdf.fill(slider_color);
                    
                    let nubbg_x = self.slide_pos * (self.rect_size.x - nub_size) - nubbg_size * 0.5 + 0.5 * nub_size;
                    sdf.rect(nubbg_x, self.rect_size.y - slider_height, nubbg_size, slider_height)
                    sdf.fill(nubbg_color);
                    
                    // the nub
                    let nub_x = self.slide_pos * (self.rect_size.x - nub_size);
                    sdf.rect(nub_x, self.rect_size.y - slider_height, nub_size, slider_height)
                    sdf.fill(nub_color);
                    
                    return sdf.result
                }
            }
        }
    }
    InstrumentCheckbox: ElementBox {
        checkbox = CheckBox {
            label: "CutOff1"
        }
    }
    
    TextInputTest: ElementBox {
        layout: {padding: {left: 8}}
        textbox = TextInput {
            text: "Hello WOrld"
        }
    }
    
    ListBoxTest: ElementBox {
        listbox = ListBox {
            items: ["One", "Two", "Three", "Four", "Five", "Six"]
        }
    }
    
    InstrumentDropdown: ElementBox {
        layout: {align: {y: 0.5}, padding: 5, flow: Right}
        label = Label {walk: {width: 70, margin: {left: 4}}}
        dropdown = DropDown {}
    }
    GraphPaper: Box {
        walk: {width: Fill, height: 100}
        bg: {
            color: #202020ff,
            color: #202020ff,
            color2: #707070ff,
            fn get_fill(self) -> vec4 {
                let sdf = Sdf2d::viewport(mod (self.pos * self.rect_size, 15))
                sdf.clear(self.color)
                sdf.rect(0.0, 0.0, 16, 16)
                sdf.stroke(self.color2, 2)
                return sdf.result
            }
        }
    }
    EnvelopePanel: Frame {
        layout: {flow: Down}
        walk: {width: Fill, height: Fit}
        display = GraphPaper {
            
        }
        attack = InstrumentSlider {
            slider = {
                bind: "adsr.a"
                min: 0.0
                max: 1.0
                label: "A"
            }
        }
        hold = InstrumentSlider {
            slider = {
                bind: "adsr.h"
                min: 0.0
                max: 1.0
                label: "H"
            }
        }
        decay = InstrumentSlider {
            slider = {
                bind: "adsr.d"
                min: 0.0
                max: 1.0
                label: "D"
            }
        }
        sustain = InstrumentSlider {
            slider = {
                bind: "adsr.s"
                min: 0.0
                max: 1.0
                label: "S"
            }
        }
        release = InstrumentSlider {
            slider = {
                bind: "adsr.r"
                min: 0.0
                max: 1.0
                label: "R"
            }
        }
    }
    
    FishHeader: Frame {
        walk: {width: Fit, height: Fit}
        layout: {padding: 5}
        label = Label {
            label: {text_style: {font_size: 12}, color: #0}
            text: "replace me!"
        }
    }
    
    FishPanel: Box {
        layout: {flow: Down}
        walk: {width: Fill, height: Fit}
        label = FishHeader {label = {text: "ReplaceMe"}}
        bg: {
            color: #804030,
            color2: #404040,
            fn get_fill(self) -> vec4 {
                return mix(self.color * 0.9, self.color2, clamp((self.pos.y * self.rect_size.y) / 300, 0, 1))
            }
        }
    }
    
    TouchPanel: FishPanel {
        bg: {color: #f08000}
        
        label = {label = {text: "Touch"}}
    }
    
    MixerPanel: FishPanel {
        bg: {color: #d0d0d0}
        label = {label = {text: "Mixer"}}
        balance = InstrumentBipolarSlider {
            slider = {
                bind: "osc_balance"
                min: 0.0
                max: 1.0
                label: "Oscillator 1/2 Balance"
            }
        }
        noise = InstrumentSlider {
            slider = {
                bind: "noise"
                min: 0.0
                max: 1.0
                label: "Noise"
            }
        }
        sub = InstrumentSlider {
            slider = {
                bind: "sub_osc"
                min: 0.0
                max: 1.0
                label: "Sub Oscillator"
            }
        }
        
    }
    FXPanel: FishPanel {
        bg: {color: #8080f0}
        label = {label = {text: "FX", label: {color: #fff}}}
    }
    LFOPanel: FishPanel {
        bg: {color: #ff0000}
        label = {label = {text: "LFO"}}
        
        rate = InstrumentSlider {
            slider = {
                bind: "lfo.rate"
                min: 0.0
                max: 1.0
                label: "Rate"
            }
        }
        sync = InstrumentCheckbox {
            checkbox = {
                bind: "lfo.synconkey",
                label: "Key sync"
            }
        }
    }
    VolumeEnvelopePanel: FishPanel {
        bg: {color: #f08000}
        label = {label = {text: "Volume Envelope"}}
        env = EnvelopePanel {
            attack = {slider = {bind: "volume_envelope.a"}}
            hold = {slider = {bind: "volume_envelope.h"}}
            decay = {slider = {bind: "volume_envelope.d"}}
            sustain = {slider = {bind: "volume_envelope.s"}}
            release = {slider = {bind: "volume_envelope.r"}}
        }
    }
    
    ModEnvelopePanel: FishPanel {
        bg: {color: #f08000}
        label = {label = {text: "Modulation Envelope"}}
        env = EnvelopePanel {
            attack = {slider = {bind: "mod_envelope.a"}}
            hold = {slider = {bind: "mod_envelope.h"}}
            decay = {slider = {bind: "mod_envelope.d"}}
            sustain = {slider = {bind: "mod_envelope.s"}}
            release = {slider = {bind: "mod_envelope.r"}}
        }
    }
    
    FilterPanel: FishPanel {
        
        bg: {color: #0000f0}
        
        label = {
            label = {text: "Filter", label: {color: #fff}}
        }
        InstrumentDropdown {
            label = {text: "Filter"}
            dropdown = {
                bind_enum: "FilterType"
                bind: "filter1.filter_type"
                items: ["Lowpass", "Highpass", "Bandpass"]
            }
        }
        
        cutoff = InstrumentSlider {
            slider = {
                bind: "filter1.cutoff"
                min: 0.0
                max: 1.0
                label: "Cutoff"
            }
        }
        
        resonance = InstrumentSlider {
            slider = {
                bind: "filter1.resonance"
                min: 0.0
                max: 1.0
                label: "Resonance"
            }
        }
        
        modamount = InstrumentBipolarSlider {
            slider = {
                bind: "filter1.envelope_amount"
                min: -1.0
                max: 1.0
                label: "Mod Env Amount"
            }
        }
        lfoamount = InstrumentBipolarSlider {
            slider = {
                bind: "filter1.lfo_amount"
                min: -1.0
                max: 1.0
                label: "LFO Amount"
            }
        }
        touchamount = InstrumentBipolarSlider {
            slider = {
                bind: "filter1.touch_amount"
                min: -1.0
                max: 1.0
                label: "Touch Amount"
            }
        }
    }
    
    OscPanel: FishPanel {
        label = {label = {text: "Oscillator ?"}}
        bg: {color: #f0f000}
        type = InstrumentDropdown {
            label = {text: "Osc1 type"}
            dropdown = {
                bind_enum: "OscType"
                bind: "osc1.osc_type"
                items: ["DPWSawPulse", "TrivialSaw", "BlampTri", "Naive", "Pure"]
            }
        }
        
        transpose = InstrumentBipolarSlider {
            slider = {
                bind: "osc1.transpose"
                min: -24.0
                max: 24.0
                label: "Transpose"
            }
        }
        
        detune = InstrumentBipolarSlider {
            slider = {
                bind: "osc1.detune"
                min: -1.0
                max: 1.0
                label: "Detune"
            }
        }
    }
    
    IronFishUI: InstrumentHeader {
        header: {
            layout: {align: {y: 0.5}}
            fold_button = FoldButton {}
            swatch = Circle {
                walk: {width: 10, height: 10}
                bg: {color: #f00}
            }
            label = Label {text: "IronFish"}
        }
        body: Frame {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            stack = LayerHeader {
                walk: {width: Fill, height: Fit}
                header: {
                    fold_button = FoldButton {}
                    label = Label {text: "Stack item", walk: {width: Fill}}
                }
                body: Frame {
                    layout: {flow: Down}
                    walk: {width: Fill, height: Fit}
                    
                    
                    
                    
                    /*
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
                    InstrumentDropdown {
                        label = {text: "Osc1 type"}
                        dropdown = {
                            bind_enum: "OscType"
                            bind: "osc1.osc_type"
                            items: ["DPWSawPulse", "TrivialSaw", "BlampTri", "Naive", "Pure"]
                        }
                    }
                    InstrumentDropdown {
                        label = {text: "Osc2 type"}
                        dropdown = {
                            bind_enum: "OscType"
                            bind: "osc2.osc_type"
                            items: ["DPWSawPulse", "TrivialSaw", "BlampTri", "Naive", "Pure"]
                        }
                    }
                    InstrumentDropdown {
                        label = {text: "Filter"}
                        dropdown = {
                            bind_enum: "FilterType"
                            bind: "filter1.filter_type"
                            items: ["Lowpass", "Highpass", "Bandpass"]
                        }
                    }
                    InstrumentCheckbox{
                        checkbox = {
                            label:"Hello world"
                        }
                    }*/
                }
            }
        }
    }
    
    App: {{App}} {
        window: {window: {inner_size: vec2(1280, 1000)}, pass: {clear_color: (#1)}}
        audio_graph: {
            root: Mixer {
                c1 = Instrument {
                    
                    
                    /*  AudioUnitInstrument {
                        plugin: "Kontakt"
                    }*/
                    
                    IronFish {
                    }
                    
                    //key_range: {start: 34, end: 47 shift: 30}
                    /*
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
                save1 = Button {text: "S1"}
                save2 = Button {text: "S2"}
                save3 = Button {text: "S3"}
                save4 = Button {text: "S4"}
                load1 = Button {text: "L1"}
                load2 = Button {text: "L2"}
                load3 = Button {text: "L3"}
                load4 = Button {text: "L4"}
            }
            
            FoldablePiano {}
            Frame {
                layout: {flow: Right, spacing: 5.0}
                Frame {
                    layout: {flow: Down, spacing: 5.0}
                    OscPanel {
                        label = {label = {text: "Oscillator 1"}}
                        type = {dropdown = {bind: "osc1.osc_type"}}
                        transpose = {slider = {bind: "osc1.transpose"}}
                        detune = {slider = {bind: "osc1.detune"}}
                    }
                    MixerPanel {}
                    OscPanel {
                        label = {label = {text: "Oscillator 2"}}
                        type = {dropdown = {bind: "osc2.osc_type"}}
                        transpose = {slider = {bind: "osc2.transpose"}}
                        detune = {slider = {bind: "osc2.detune"}}
                    }
                }
                Frame {
                    layout: {flow: Down, spacing: 5.0}
                    VolumeEnvelopePanel {}
                    ModEnvelopePanel {}
                }
                Frame {
                    layout: {flow: Down, spacing: 5.0}
                    
                    LFOPanel {}
                    FilterPanel {}
                    TouchPanel {}
                    
                    FXPanel {}
                    FishPanel {
                        bg: {color: #3}
                        label = {label = {text: "Scope"}}
                        display_audio = DisplayAudio {
                            walk: {height: 300, width: Fill}
                        }
                    }
                }
            }
            
        }
    }
}
main_app!(App);

#[repr(u8)]
#[allow(unused)]
enum KnobType {
    UniPolar = 0,
    BiPolar,
    ToggleHoriz,
    ToggleVert,
    OneOutOf4,
    OneOutOf4Smooth,
    Circular1,
    Circular2,
    Circular3,
    Circular4
}

#[repr(u8)]
#[allow(unused)]
enum KnobRGB {
    Yellow = 0,
    Orange,
    Red,
    Indigo,
    LightBlue,
    Green,
    Grey
}

struct KnobBind {
    name: String,
    min: f64,
    max: f64,
    value: f64,
    rgb: KnobRGB,
    ty: KnobType
}

#[derive(Live, LiveHook)]
pub struct App {
    imgui: ImGUI,
    audio_graph: AudioGraph,
    window: BareWindow,
    data: f32,
    #[rust(usize::MAX)] last_knob_index: usize,
    #[rust] knob_bind: [usize; 2],
    #[rust] knob_change: usize,
    #[rust(vec![
        KnobBind {name: "osc1.detune".into(), value:0.0, rgb: KnobRGB::Yellow, ty: KnobType::BiPolar, min: -1.0, max: 1.0}
    ])] knob_table: Vec<KnobBind>
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
        makepad_media::live_register(cx);
        crate::display_audio::live_register(cx);
        crate::iron_fish::live_register(cx);
        crate::piano::live_register(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return Cx2d::draw(cx, event, self, | cx, s | s.draw(cx));
        }
        //let dt = profile_start();
        
        self.window.handle_event(cx, event);
        
        let mut ui = self.imgui.run(cx, event);
        
        if ui.on_construct() {
            ui.cx.start_midi_input();
            let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
            ui.bind_read(&iron_fish.settings.live_read());
            ui.piano(ids!(piano)).set_key_focus(ui.cx);
        }
        
        let display_audio = ui.display_audio(ids!(display_audio));
        
        let mut buffers = 0;
        self.audio_graph.handle_event(ui.cx, ui.event, &mut | cx, action | {
            match action {
                AudioGraphAction::DisplayAudio {buffer, voice, active} => {
                    display_audio.process_buffer(cx, active, voice, buffer);
                    buffers += 1;
                }
                AudioGraphAction::VoiceOff {voice} => {
                    display_audio.voice_off(cx, voice);
                }
            }
        });
        
        // fetch ui binding deltas
        
        for delta in ui.on_bind_deltas() {
            for (index, bind) in self.knob_table.iter_mut().enumerate() {
                if let Some(LiveValue::Float(v)) = delta.read_path(&bind.name) {
                    let knob = self.knob_change;
                    if index != self.last_knob_index {
                        self.last_knob_index = index;
                        self.knob_bind[knob] = index;
                        
                        ui.cx.send_midi_1_data(Midi1Data {
                            data0: 0xb0,
                            data1: (1 + knob)as u8,
                            data2: bind.ty as u8
                        });
                        
                        ui.cx.send_midi_1_data(Midi1Data {
                            data0: 0xb0,
                            data1: (5 + knob)as u8,
                            data2: bind.rgb as u8
                        });
                        //log!("SET SHIT");
                        self.knob_change = (self.knob_change + 1) % (self.knob_bind.len());
                        bind.value = *v;
                    }
                    
                    //log!("SEND SHIT {} {}", v, (((v - bind.min) / (bind.max - bind.min)) * 127.0)  as u8);
                    ui.cx.send_midi_1_data(Midi1Data {
                        data0: 0xb0,
                        data1: (3 + knob)as u8,
                        data2: (((v - bind.min) / (bind.max - bind.min)) * 127.0) as u8
                    });
                }
            }
            let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
            iron_fish.settings.apply_over(ui.cx, &delta);
            ui.bind_read(&delta);
        }
        
        let piano = ui.piano(ids!(piano));
        
        ui.cx.on_midi_input_list(ui.event);
        for inp in ui.cx.on_midi_1_input_data(ui.event) {
            if inp.data.data0 == 0xb0 {
                log!("{:?}", inp.data);
                match inp.data.data1 {
                    10 => {
                        let last_knob = (self.knob_change + 1) % (self.knob_bind.len());
                        let bind_id = self.knob_bind[last_knob];
                        let bind = &mut self.knob_table[bind_id];
                        bind.value = ((inp.data.data2 as f64 - 63.0) * ((bind.max-bind.min)*0.001) + bind.value).min(bind.max).max(bind.min);
                        let mut delta = Vec::new();
                        delta.write_path(&bind.name, LiveValue::Float(bind.value));
                        delta.debug_print(0,100);
                        ui.bind_read(&delta);
                    }
                    11 => {
                        
                    }
                    _ => ()
                }
            }
            self.audio_graph.send_midi_1_data(inp.data);
            if let Some(note) = inp.data.decode().on_note() {
                piano.set_note(ui.cx, note.is_on, note.note_number)
            }
        }
        
        for note in piano.on_notes() {
            self.audio_graph.send_midi_1_data(Midi1Note {
                channel: 0,
                is_on: note.is_on,
                note_number: note.note_number,
                velocity: note.velocity
            }.into());
        }
        //profile_end(dt);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.imgui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}