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

use std::fs::File;
use std::io::prelude::*;

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
    
    ElementBox: Frame {
        bg: {color: #4}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: {left: 8, top: 5, bottom: 3, right: 8}, spacing: 5}
    }
    
    FishDropDown: DropDown {
        layout: {
            padding: {left: 6.0, top: 6.0, right: 4.0, bottom: 6.0}
        }
        bg: {
            fn get_bg(self, inout sdf: Sdf2d) {
                sdf.box(
                    1,
                    1,
                    self.rect_size.x - 2,
                    self.rect_size.y - 2,
                    3
                )
                sdf.stroke_keep(#0, 1);
                sdf.fill(mix(#5, #2, self.pos.y));
            }
        }
    }
    
    FishButton: Button {
        bg: {
            instance hover: 0.0
            instance pressed: 0.0
            
            const BORDER_RADIUS: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    2.0
                )
                sdf.fill_keep(mix(mix(#5, #2, self.pos.y), mix(#1, #3, self.pos.y), self.pressed))
                
                sdf.stroke(
                    #0,
                    1.0
                )
                
                return sdf.result
            }
        }
        
        walk: {
            width: Size::Fit,
            height: Size::Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: 8
        }
    }
    FishSlider: Slider {
        label: "CutOff1"
        walk: {height: 40}
        slider: {
            instance line_color: #f00
            instance bipolar: 0.0
            fn pixel(self) -> vec4 {
                let slider_height = 7;
                let nub_size = 3
                
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 20.0;
                
                sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, 2);
                sdf.fill_keep(mix(#2, #3c, self.pos.y))
                sdf.stroke(#5, 1.0)
                let in_side = 5.0;
                let in_top = 7.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(#1);
                let in_top = 9.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(#4);
                
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);
                
                sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
                sdf.stroke(self.line_color, 1)
                
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.box(nub_x + in_side, top + 3.0, 12, 12, 1.)
                
                sdf.fill_keep(mix(mix(#7, #a, self.hover), #3, self.pos.y));
                sdf.stroke(mix(mix(#7, #a, self.hover), #0, pow(self.pos.y, 3)), 1.);
                
                return sdf.result
            }
        }
    }
    
    InstrumentSlider: ElementBox {
        slider = FishSlider {
            slider: {bipolar: 0.0}
        }
    }
    
    InstrumentBipolarSlider: ElementBox {
        slider = FishSlider {
            slider: {bipolar: 1.0}
        }
    }
    
    InstrumentCheckbox: ElementBox {
        checkbox = CheckBox {
            label: "CutOff1"
        }
    }
    
    InstrumentDropdown: ElementBox {
        layout: {align: {y: 0.5}, padding: 5, flow: Right}
        label = Label {walk: {width: 30, margin: {left: 7}}}
        dropdown = FishDropDown {}
    }
    GraphPaper: Box {
        walk: {width: Fill, height: 100, margin: {left: 5, right: 5}}
        bg: {
            radius: 3,
            color: #452C20ff,
            color2: #0,
            
            instance attack: 0.05
            instance decay: 0.2
            instance sustain: 0.5
            instance release: 0.2
            
            fn get_fill(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size); //mod (self.pos * self.rect_size, 15))
                let base_color = mix(self.color, self.color2, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 2.0));
                let darker = base_color * 0.8;
                let pos = self.pos * self.rect_size; 
                sdf.clear(mix(base_color,darker,pow(abs(sin(pos.x*0.5)),24)+pow(abs(sin(pos.y*0.5)),32.0)));
                sdf.rect(1.0, 1.0, 16, 16)
                sdf.stroke(darker, 1)
                let pad_b = 8
                let pad_s = 8
                let width = self.rect_size.x - 2 * pad_s
                let height = self.rect_size.y - 2 * pad_b
                let total = self.attack + self.decay + self.release + 0.5
                let sustain = self.rect_size.y - pad_b - height * self.sustain;
                sdf.pos = self.pos * self.rect_size;
                sdf.move_to(pad_s, self.rect_size.y - pad_b)
                sdf.line_to(pad_s + width * (self.attack/total), pad_b)
                sdf.line_to(pad_s + width * ((self.attack + self.decay)/total), sustain)
                sdf.line_to(pad_s + width * (1.0 - self.release/total), sustain)
                sdf.line_to(pad_s + width, self.rect_size.y - pad_b)
                sdf.stroke_keep(#f9b08b, 1.);
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
                slider: {line_color: #f9b08b}
                bind: "adsr.a"
                min: 0.0
                max: 1.0
                label: "A"
            }
        }
        hold = InstrumentSlider {
            slider = {
                slider: {line_color: #f9b08b}
                bind: "adsr.h"
                min: 0.0
                max: 1.0
                label: "H"
            }
        }
        decay = InstrumentSlider {
            slider = {
                slider: {line_color: #f9b08b}
                bind: "adsr.d"
                min: 0.0
                max: 1.0
                label: "D"
            }
        }
        sustain = InstrumentSlider {
            slider = {
                slider: {line_color: #f9b08b}
                bind: "adsr.s"
                min: 0.0
                max: 1.0
                label: "S"
            }
        }
        release = InstrumentSlider {
            slider = {
                slider: {line_color: #f9b08b}
                bind: "adsr.r"
                min: 0.0
                max: 1.0
                label: "R"
            }
        }
    }
    
    FishHeader: Solid {
        bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let edge = 5.0;
                sdf.move_to(1.0 + edge, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, self.rect_size.y - 2.0)
                sdf.line_to(1.0, self.rect_size.y - 2.0);
                sdf.line_to(1.0, 1.0 + edge);
                sdf.close_path();
                sdf.fill(self.color);
                //sdf.stroke(self.color, 1.0)
                return sdf.result
            }
        }
        walk: {width: Fit, height: Fit}
        layout: {padding: {left: 10, top: 5, right: 10, bottom: 5}}
        label = Label {
            label: {text_style: {font_size: 12}, color: #0}
            text: "replace me!"
        }
    }
    
    
    FishPanel: Frame {
        layout: {flow: Down}
        walk: {width: Fill, height: Fit}
        label = FishHeader {label = {text: "ReplaceMe"}}
        body = Box {
            layout: {flow: Down, padding: {top: 10, left: 6, right: 6, bottom: 15}}
            walk: {width: Fill, height: Fit, margin: {top: -3, left: 0.25}}
            bg: {
                color: #5
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    let edge = 8.0;
                    sdf.move_to(1.0, 1.0);
                    sdf.line_to(self.rect_size.x - 2.0, 1.0);
                    sdf.line_to(self.rect_size.x - 2.0, self.rect_size.y - edge)
                    sdf.line_to(self.rect_size.x - edge, self.rect_size.y - 2.0)
                    sdf.line_to(1.0, self.rect_size.y - 2.0);
                    sdf.close_path();
                    sdf.fill_keep(mix(#47, #3c, pow(self.pos.x, 8.0)));
                    sdf.stroke(self.color, 1.0)
                    return sdf.result
                }
            }
        }
    }
    
    TouchPanel: FishPanel {
        label = {bg: {color: #ccfc9f}, label = {text: "Touch"}}
        body = {
            bg: {color: #ccfc9f}
            
            scale = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #ccfc9f}
                    bind: "touch.scale"
                    min: -1.0
                    max: 1.0
                    label: "Scale"
                }
            }
            offset = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #ccfc9f}
                    bind: "touch.offset"
                    min: -1.0
                    max: 1.0
                    label: "Offset"
                }
            }
            curve = InstrumentSlider {
                slider = {
                    slider: {line_color: #ccfc9f}
                    bind: "touch.curve"
                    min: 0.0
                    max: 1.0
                    label: "Curvature"
                }
            }
        }
    }
    
    MixerPanel: FishPanel {
        label = {bg: {color: #c8c8c8}, label = {text: "Mixer"}}
        body = {
            bg: {color: #c8c8c8}
            balance = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #c8c8c8}
                    bind: "osc_balance"
                    min: 0.0
                    max: 1.0
                    label: "Oscillator 1/2 Balance"
                }
            }
            noise = InstrumentSlider {
                slider = {
                    slider: {line_color: #c8c8c8}
                    bind: "noise"
                    min: 0.0
                    max: 1.0
                    label: "Noise"
                }
            }
            sub = InstrumentSlider {
                slider = {
                    slider: {line_color: #c8c8c8}
                    bind: "sub_osc"
                    min: 0.0
                    max: 1.0
                    label: "Sub Oscillator"
                }
            }
        }
        
    }
    FXPanel: FishPanel {
        label = {bg: {color: #9fe2fc}, label = {text: "Effects",}}
        body = {bg: {color: #9fe2fc} delaysend = InstrumentSlider {
            slider = {
                slider: {line_color: #9fe2fc}
                bind: "fx.delaysend"
                min: 0.0
                max: 1.0
                label: "Delay Send"
            }
        } delayfeedback = InstrumentSlider {
            slider = {
                slider: {line_color: #9fe2fc}
                bind: "fx.delayfeedback"
                min: 0.0
                max: 1.0
                label: "Delay Feedback"
            }
        }}
    }
    LFOPanel: FishPanel {
        label = {bg: {color: #f4756e}, label = {text: "LFO"}}
        body = {
            bg: {color: #f4756e}
            rate = InstrumentSlider {
                slider = {
                    slider: {line_color: #f4756e}
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
    }
    VolumeEnvelopePanel: FishPanel {
        label = {bg: {color: #f9b08b}, label = {text: "Volume Env"}}
        body = {
            bg: {color: #f9b08b}
            vol_env = EnvelopePanel {
                attack = {slider = {bind: "volume_envelope.a"}}
                hold = {slider = {bind: "volume_envelope.h"}}
                decay = {slider = {bind: "volume_envelope.d"}}
                sustain = {slider = {bind: "volume_envelope.s"}}
                release = {slider = {bind: "volume_envelope.r"}}
            }
        }
    }
    
    ModEnvelopePanel: FishPanel {
        label = {bg: {color: #f9b08b}, label = {text: "Modulation Env"}}
        body = {
            bg: {color: #f9b08b}
            mod_env = EnvelopePanel {
                attack = {slider = {bind: "mod_envelope.a"}}
                hold = {slider = {bind: "mod_envelope.h"}}
                decay = {slider = {bind: "mod_envelope.d"}}
                sustain = {slider = {bind: "mod_envelope.s"}}
                release = {slider = {bind: "mod_envelope.r"}}
            }
        }
    }
    
    FilterPanel: FishPanel {
        label = {bg: {color: #3F64A1}, label = {text: "Filter"}}
        body = {
            bg: {color: #3F64A1}
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
                    slider: {line_color: #3F64A1}
                    bind: "filter1.cutoff"
                    min: 0.0
                    max: 1.0
                    label: "Cutoff"
                }
            }
            
            resonance = InstrumentSlider {
                slider = {
                    slider: {line_color: #3F64A1}
                    bind: "filter1.resonance"
                    min: 0.0
                    max: 1.0
                    label: "Resonance"
                }
            }
            
            modamount = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #3F64A1}
                    bind: "filter1.envelope_amount"
                    min: -1.0
                    max: 1.0
                    label: "Mod Env Amount"
                }
            }
            lfoamount = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #3F64A1}
                    bind: "filter1.lfo_amount"
                    min: -1.0
                    max: 1.0
                    label: "LFO Amount"
                }
            }
            touchamount = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #3F64A1}
                    bind: "filter1.touch_amount"
                    min: -1.0
                    max: 1.0
                    label: "Touch Amount"
                }
            }
        }
    }
    
    OscPanel: FishPanel {
        label = {bg: {color: #fffb9f}, label = {text: "Oscillator ?"}}
        body = {
            bg: {color: #fffb9f} 
            type = InstrumentDropdown {
                label = {text: "Type"}
                dropdown = {
                    bind_enum: "OscType"
                    bind: "osc1.osc_type"
                    items: ["DPWSawPulse", "TrivialSaw", "BlampTri", "Naive", "Pure"]
                    display: ["SawPulse", "Saw", "Triangle", "Naive", "Pure"]
                }
            }
            
            transpose = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #fffb9f}
                    bind: "osc1.transpose"
                    min: -24.0
                    max: 24.0
                    label: "Transpose"
                }
            }
            
            detune = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: #fffb9f}
                    bind: "osc1.detune"
                    min: -1.0
                    max: 1.0
                    label: "Detune"
                }
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
                }
            }
        }
    }
    
    App: {{App}} {
        window: {window: {inner_size: vec2(1280, 1000)}, pass: {clear_color: (#3)}}
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
            bg: {color: #f00},
            walk: {width: Fill, height: Fill}
            layout: {
                padding: 8
                align: {x: 0.0, y: 0.0}
                spacing: 0.,
                flow: Flow::Down
            },
            Frame {
                layout: {flow: Right, spacing: 5.0, padding: {bottom: 0}, align: {x: 0.0, y: 1.0}}
                walk: {margin: {left: 00}, width: Fill, height: Fit}
                Image {
                    image: d"resources/tinrs.png",
                    walk: {width: 480, height: 100}
                }
                Frame {
                    walk: {width: Fill, height: Fit, margin: {bottom: 20}}
                    layout: {flow: Down, spacing: 8.0, align: {x: 1.0, y: 1.0}}
                    Frame {
                        walk: {width: Fit, height: Fit}
                        layout: {flow: Right, spacing: 8.0}
                        save1 = FishButton {text: "Preset 1"}
                        save2 = FishButton {text: "Preset 2"}
                        save3 = FishButton {text: "Preset 3"}
                        save4 = FishButton {text: "Preset 4"}
                        save5 = FishButton {text: "Preset 5"}
                        save6 = FishButton {text: "Preset 6"}
                        save7 = FishButton {text: "Preset 7"}
                        save8 = FishButton {text: "Preset 8"}
                    }
                }
                panic = FishButton {layout: {padding: 20}, walk: {height: Fill, margin: {left: 20, top: 20, right: 20, bottom: 22}}, text: "Panic"}
            }
            
            piano = Piano {}
            GradientY {
                walk: {width: Fill, height: 10}
                bg: {color: #000a, color2: #0008}
            }
            GradientY {
                walk: {width: Fill, height: 100}
                bg: {color: #0008, color2: #0000}
                display_audio = DisplayAudio {
                    walk: {height: Fill, width: Fill}
                }
            }
            
            Frame {
                layout: {flow: Right, spacing: 12.0}
                Frame {
                    layout: {flow: Down, spacing: 12.0}
                    OscPanel {
                        label = {label = {text: "Oscillator 1"}}
                        body = {
                            type = {dropdown = {bind: "osc1.osc_type"}}
                            transpose = {slider = {bind: "osc1.transpose"}}
                            detune = {slider = {bind: "osc1.detune"}}
                        }
                    }
                    OscPanel {
                        label = {label = {text: "Oscillator 2"}}
                        body = {
                            type = {dropdown = {bind: "osc2.osc_type"}}
                            transpose = {slider = {bind: "osc2.transpose"}}
                            detune = {slider = {bind: "osc2.detune"}}
                        }
                    }
                }
                Frame {
                    MixerPanel {}
                }
                Frame {
                    ModEnvelopePanel {}
                }
                Frame {
                    layout: {flow: Down, spacing: 5.0}
                    VolumeEnvelopePanel {}
                }
                Frame {
                    FilterPanel {}
                }
                Frame {
                    layout: {flow: Down, spacing: 5.0}
                    
                    LFOPanel {}
                    
                    TouchPanel {}
                    FXPanel {}
                }
            }
            
        }
    }
}
main_app!(App);

#[derive(Copy, Clone)]
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

#[derive(Copy, Clone)]
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
        KnobBind {name: "osc1.detune".into(), value: 0.0, rgb: KnobRGB::Yellow, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        KnobBind {name: "osc2.detune".into(), value: 0.0, rgb: KnobRGB::Yellow, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        KnobBind {name: "lfo.rate".into(), value: 0.0, rgb: KnobRGB::Red, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        
        KnobBind {name: "osc1.transpose".into(), value: 0.0, rgb: KnobRGB::Yellow, ty: KnobType::BiPolar, min: -36.0, max: 36.0},
        KnobBind {name: "osc2.transpose".into(), value: 0.0, rgb: KnobRGB::Yellow, ty: KnobType::BiPolar, min: -36.0, max: 36.0},
        
        KnobBind {name: "touch.offset".into(), value: 0.0, rgb: KnobRGB::Green, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        KnobBind {name: "touch.curve".into(), value: 0.0, rgb: KnobRGB::Green, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "touch.scale".into(), value: 0.0, rgb: KnobRGB::Green, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        
        KnobBind {name: "fx.delaysend".into(), value: 0.0, rgb: KnobRGB::LightBlue, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "fx.delayfeedback".into(), value: 0.0, rgb: KnobRGB::LightBlue, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        
        KnobBind {name: "filter1.cutoff".into(), value: 0.0, rgb: KnobRGB::Indigo, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "filter1.resonance".into(), value: 0.0, rgb: KnobRGB::Indigo, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "filter1.touch_amount".into(), value: 0.0, rgb: KnobRGB::Indigo, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        KnobBind {name: "filter1.lfo_amount".into(), value: 0.0, rgb: KnobRGB::Indigo, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        KnobBind {name: "filter1.envelope_amount".into(), value: 0.0, rgb: KnobRGB::Indigo, ty: KnobType::BiPolar, min: -1.0, max: 1.0},
        
        KnobBind {name: "osc_balance".into(), value: 0.0, rgb: KnobRGB::Grey, ty: KnobType::BiPolar, min: 0.0, max: 1.0},
        KnobBind {name: "noise".into(), value: 0.0, rgb: KnobRGB::Grey, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "sub_osc".into(), value: 0.0, rgb: KnobRGB::Grey, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        
        KnobBind {name: "mod_envelope.a".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "mod_envelope.h".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "mod_envelope.d".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "mod_envelope.s".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "mod_envelope.r".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        
        KnobBind {name: "volume_envelope.a".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "volume_envelope.h".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "volume_envelope.d".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "volume_envelope.s".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0},
        KnobBind {name: "volume_envelope.r".into(), value: 0.0, rgb: KnobRGB::Orange, ty: KnobType::UniPolar, min: 0.0, max: 1.0}
        
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
                if let Some(value) = delta.read_path(&bind.name) && let Some(v) = value.as_float() {
                    
                    let mod_env = ui.frame(ids!(mod_env.display));
                    let vol_env = ui.frame(ids!(vol_env.display));
                    match bind.name.as_ref() {
                        "mod_envelope.a" => mod_env.apply_over(ui.cx, live!{bg: {attack: (v)}}),
                        "mod_envelope.d" => mod_env.apply_over(ui.cx, live!{bg: {decay: (v)}}),
                        "mod_envelope.s" => mod_env.apply_over(ui.cx, live!{bg: {sustain: (v)}}),
                        "mod_envelope.r" => mod_env.apply_over(ui.cx, live!{bg: {release: (v)}}),
                        "volume_envelope.a" => vol_env.apply_over(ui.cx, live!{bg: {attack: (v)}}),
                        "volume_envelope.d" => vol_env.apply_over(ui.cx, live!{bg: {decay: (v)}}),
                        "volume_envelope.s" => vol_env.apply_over(ui.cx, live!{bg: {sustain: (v)}}),
                        "volume_envelope.r" => vol_env.apply_over(ui.cx, live!{bg: {release: (v)}}),
                        _ => ()
                    }
                    
                    let mut knob = 3;
                    if self.knob_bind[0] == index {
                        knob = 0
                    }
                    if self.knob_bind[1] == index {
                        knob = 1
                    }
                    if knob == 3
                    {
                        knob = self.knob_change;
                        self.knob_change = (self.knob_change + 1) % (self.knob_bind.len());
                        self.last_knob_index = index;
                        self.knob_bind[knob] = index;
                        
                        ui.cx.send_midi_1_data(Midi1Data {
                            data0: 0xb0,
                            data1: (1 + knob)as u8,
                            data2: bind.ty as u8
                        });
                        
                        ui.cx.send_midi_1_data(Midi1Data {
                            data0: 0xb0,
                            data1: (5 + knob) as u8,
                            data2: bind.rgb as u8
                        });
                        
                    }
                    
                    bind.value = v;
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
                let mut ring = 3;
                let mut keypressure = 40;
                match inp.data.data1 {
                    10 => {
                        ring = 0;
                    }
                    11 => {
                        ring = 1;
                    }
                    20 => {keypressure = 0;}
                    21 => {keypressure = 1;}
                    22 => {keypressure = 2;}
                    23 => {keypressure = 3;}
                    24 => {keypressure = 4;}
                    25 => {keypressure = 5;}
                    26 => {keypressure = 6;}
                    27 => {keypressure = 7;}
                    28 => {keypressure = 8;}
                    29 => {keypressure = 9;}
                    30 => {keypressure = 10;}
                    31 => {keypressure = 11;}
                    _ => ()
                }
                if keypressure < 40 {
                    
                }
                
                if ring<3 {
                    log!("{:?}", inp.data);
                    
                    let bind_id = self.knob_bind[ring];
                    let bind = &mut self.knob_table[bind_id];
                    bind.value = ((inp.data.data2 as f64 - 63.0) * ((bind.max - bind.min) * 0.001) + bind.value).min(bind.max).max(bind.min);
                    let mut delta = Vec::new();
                    delta.write_path(&bind.name, LiveValue::Float64(bind.value));
                    delta.debug_print(0, 100);
                    ui.bind_read(&delta);
                    
                    ui.cx.send_midi_1_data(Midi1Data {
                        data0: 0xb0,
                        data1: (3 + ring)as u8,
                        data2: (((bind.value - bind.min) / (bind.max - bind.min)) * 127.0) as u8
                    });
                    let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
                    iron_fish.settings.apply_over(ui.cx, &delta);
                }
            }
            self.audio_graph.send_midi_1_data(inp.data);
            if let Some(note) = inp.data.decode().on_note() {
                log!("{:?}", inp.data);
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
        if ui.button(ids!(panic)).was_clicked() {
            self.audio_graph.all_notes_off();
        }
        
        let shift = if let Event::FingerUp(fu) = event {fu.modifiers.shift}else {false};
        if ui.button(ids!(save1)).was_clicked() {self.preset(ui.cx, 1, shift);}
        if ui.button(ids!(save2)).was_clicked() {self.preset(ui.cx, 2, shift);}
        if ui.button(ids!(save3)).was_clicked() {self.preset(ui.cx, 3, shift);}
        if ui.button(ids!(save4)).was_clicked() {self.preset(ui.cx, 4, shift);}
        if ui.button(ids!(save5)).was_clicked() {self.preset(ui.cx, 5, shift);}
        if ui.button(ids!(save6)).was_clicked() {self.preset(ui.cx, 6, shift);}
        if ui.button(ids!(save7)).was_clicked() {self.preset(ui.cx, 7, shift);}
        if ui.button(ids!(save8)).was_clicked() {self.preset(ui.cx, 8, shift);}
    }
    
    pub fn preset(&mut self, cx: &mut Cx, index: usize, save: bool) {
        let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
        let file_name = format!("preset_{}.bin", index);
        if save {
            let nodes = iron_fish.settings.live_read();
            let data = nodes.to_msgpack(0).unwrap();
            let data = makepad_miniz::compress_to_vec(&data, 10);
            log!("Saving preset {}", file_name);
            let mut file = File::create(&file_name).unwrap();
            file.write_all(&data).unwrap();
        }
        else if let Ok(mut file) = File::open(&file_name) {
            log!("Loading preset {}", file_name);
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if let Ok(data) = makepad_miniz::decompress_to_vec(&data){
                let mut nodes = Vec::new();
                nodes.from_msgpack(&data).unwrap();
                iron_fish.settings.apply_over(cx, &nodes);
                self.imgui.root_frame().bind_read(cx, &nodes);
            }
            else{
                log!("Error decompressing preset");
            }
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