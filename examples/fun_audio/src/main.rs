pub use makepad_component;
pub use makepad_component::makepad_platform;
pub use makepad_platform::makepad_math;
pub use makepad_media;

use makepad_component::*;
use makepad_component::imgui::*;
use makepad_draw_2d::*;
use makepad_media::*;
use makepad_media::audio_graph::*;
use makepad_platform::live_atomic::*;

mod sequencer;
mod display_audio;
mod piano;
mod iron_fish;

use crate::iron_fish::*;
use crate::piano::*;
use crate::sequencer::*;
use crate::display_audio::*;

use std::fs::File;
use std::io::prelude::*;


live_register!{
    registry AudioComponent::*;
    registry FrameComponent::*;
    import makepad_component::theme::*;
    import makepad_component::frame::*;
    import makepad_draw_2d::shader::std::*;

    const SPACING_PANELS : 10.0
    const SPACING_CONTROLS : 4.0
    const COLOR_OSC : #xFFFF99 // yellow
    const COLOR_MIX : #xC // gray
    const COLOR_ENV : #xFFC499 // light red
    const COLOR_FILTER : #xA7ADF2 // indigo
    const COLOR_LFO : #xFF9999 // red
    const COLOR_TOUCH : #xBBFF99 // light green
    const COLOR_FX : #x99EEFF // light green
    const COLOR_TEXT_H1 : #x181818
    const COLOR_TEXT_H2 : #x9
    const FONT_SIZE_H1: 11.0
    const FONT_SIZE_H2: 9.5
    

    // MainHeader: FoldHeader {
    //     state: {
    //         open = {
    //             off = {apply: {header: {bg: {radius: vec2(3.0, 3.0)}}}}
    //             on = {apply: {header: {bg: {radius: vec2(3.0, 1.0)}}}}
    //         }
    //     }
    //     header: BoxY {
    //         cursor: Default,
    //         bg: {color: #6},
    //         walk: {width: Fill, height: Fit},
    //         layout: {flow: Right, padding: 8, spacing: 5}
    //     }
    // }
    
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
        layout: {flow: Down, padding: {left: (SPACING_CONTROLS), top: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), right: (SPACING_CONTROLS)}, spacing: (SPACING_CONTROLS)}
    }
    
    FishDropDown: DropDown {
        walk: { margin: {left: 0.0, right: 0.0, top: 0.0, bottom: 0.0}}
        layout: {padding: 6.0}
        label: {text_style: {font_size: (FONT_SIZE_H2), font: { path: d"resources/IBMPlexSans-SemiBold.ttf" }}, color: (COLOR_TEXT_H1)}
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
        label:{text_style: {font_size: (FONT_SIZE_H2), font: { path: d"resources/IBMPlexSans-SemiBold.ttf" }}, color: (COLOR_TEXT_H1)}
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
            width: 30,
            height: 30,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: 8
        }
    }

    FishSlider: Slider {
        label: "CutOff1"
        walk: {
                height: 40,
                margin: {left: 0.0, right: 1.0, top: 0.0, bottom: 0.0},
        }

        label_text:{text_style: {font_size: (FONT_SIZE_H2), font: { path: d"resources/IBMPlexSans-SemiBold.ttf" }}, color: (COLOR_TEXT_H2)}
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
        bg: {color: #f00}
        layout: { padding: 0.0 }
        checkbox = CheckBox {
            label: "CutOff1"
            label_text: {text_style: {font_size: (FONT_SIZE_H2), font: { path: d"resources/IBMPlexSans-SemiBold.ttf" }}, color: (COLOR_TEXT_H2)}
            walk: { margin: {top: 5, right: 0, bottom: 5, left: 0} }
            label_walk: {
                margin: {left: 23.0, top: 0, bottom: 0, right: 0}
            }
        }
    }
    
    InstrumentDropdown: ElementBox {
        layout: {align: {y: 0.5}, padding: 0, flow: Right}
        label = Label { walk: {width: Fit, margin: {right: 5} }, 
            label: {
                color: (COLOR_TEXT_H2)
                text_style: {font_size: (FONT_SIZE_H2), font: { path: d"resources/IBMPlexSans-SemiBold.ttf" }}, color: (COLOR_TEXT_H2)
            }
        }
        walk: {margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: (SPACING_CONTROLS)}}
        dropdown = FishDropDown {}
    }

    GraphPaper: Box {
        walk: {width: Fill, height: 100, margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: (SPACING_CONTROLS)}}
        bg: {
            radius: 3,
            color: #x3F,
            color2: #0,
            
            instance attack: 0.05
            instance hold: 0.0
            instance decay: 0.2
            instance sustain: 0.5
            instance release: 0.2
            
            fn get_fill(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size); //mod (self.pos * self.rect_size, 15))
                let base_color = mix(self.color, self.color2, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 2.0));
                let darker = base_color * 0.8;
                let pos = self.pos * self.rect_size;
                sdf.clear(mix(base_color, darker, pow(abs(sin(pos.x * 0.5)), 24) + pow(abs(sin(pos.y * 0.5)), 32.0)));
                sdf.rect(1.0, 1.0, 16, 16)
                sdf.stroke(darker, 1)
                let pad_b = 8
                let pad_s = 8
                let width = self.rect_size.x - 2 * pad_s
                let height = self.rect_size.y - 2 * pad_b
                let total = self.attack + self.decay + self.release + 0.5  + self.hold
                let sustain = self.rect_size.y - pad_b - height * self.sustain;
                sdf.pos = self.pos * self.rect_size;
                sdf.move_to(pad_s, self.rect_size.y - pad_b)
                sdf.line_to(pad_s + width * (self.attack / total), pad_b)
                sdf.line_to(pad_s + width * ((self.attack +self.hold)/ total), pad_b)
                sdf.line_to(pad_s + width * ((self.attack + self.decay + self.hold) / total), sustain)
                sdf.line_to(pad_s + width * (1.0 - self.release / total), sustain)
                sdf.line_to(pad_s + width, self.rect_size.y - pad_b)
                sdf.stroke_keep(#xFFC49910, 8.0);
                sdf.stroke_keep(#xFFC49910, 6.0);
                sdf.stroke_keep(#xFFC49920, 4.0);
                sdf.stroke_keep(#xFFC49980, 2.0);
                sdf.stroke_keep(#xFFFFFFFF, 1.0);
                // sdf.stroke_keep(#f9b08b, 1.);
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
                slider: {line_color: (COLOR_ENV)}
                bind: "adsr.a"
                min: 0.0
                max: 1.0
                label: "A"
            }
        }
        hold = InstrumentSlider {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                bind: "adsr.h"
            walk: { margin: 0.0 }
                min: 0.0
                max: 1.0
                label: "H"
            }
        }
        decay = InstrumentSlider {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                bind: "adsr.d"
                min: 0.0
                max: 1.0
                label: "D"
            }
        }
        sustain = InstrumentSlider {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                bind: "adsr.s"
                min: 0.0
                max: 1.0
                label: "S"
            }
        }
        release = InstrumentSlider {
            slider = {
                slider: {line_color: (COLOR_ENV)}
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
                return sdf.result
            }
        }
        walk: {width: Fill, height: Fit}
        layout: {padding: {left: (SPACING_CONTROLS), top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
        label = Label {
            label: {text_style: {font_size: (FONT_SIZE_H1), font: { path: d"resources/IBMPlexSans-SemiBold.ttf" }}, color: (COLOR_TEXT_H1)}
            text: "replace me!"
        }
    }
    
    
    FishPanel: Frame {
        layout: {flow: Down}
        walk: {width: Fill, height: Fit}
        label = FishHeader {label = {text: "ReplaceMe", walk: { margin: { top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS) }}}}
        body = Box {
            layout: {flow: Down, padding: {top: (SPACING_CONTROLS), left: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
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
                    sdf.fill_keep(mix(#xFFFFFF60, #xFFFFFF10, pow(self.pos.x, 0.05)));
                    // sdf.fill_keep(
                    //  mix(
                    //     mix(#xf00f, #x00ff, self.rect_size.x / 0.5),
                    //     mix(#x00ff, #ffff, (self.rect_size.x - 0.5)/(1.0 - 0.5)),
                    //     step(0.5, self.rect_size.x))
                    // )
                    sdf.stroke(self.color, 1.0)
                    return sdf.result
                }
            }
        }
    }
    
    TouchPanel: FishPanel {
        label = {bg: {color: (COLOR_TOUCH)}, label = {text: "Touch"}}
        body = {
            scale = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: (COLOR_TOUCH)}
                    slider: {line_color: (COLOR_TOUCH)}
                    slider: {line_color: (COLOR_TOUCH)}
                    bind: "touch.scale"
                    min: -1.0
                    max: 1.0
                    label: "Scale"
                }
            }
            twocol = Frame {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                offset = InstrumentBipolarSlider {
                    slider = {
                        slider: {line_color: (COLOR_TOUCH)}
                        bind: "touch.offset"
                        min: -1.0
                        max: 1.0
                        label: "Offset"
                    }
                }
                curve = InstrumentSlider {
                    slider = {
                        slider: {line_color: (COLOR_TOUCH)}
                        bind: "touch.curve"
                        min: 0.0
                        max: 1.0
                        label: "Curvature"
                    }
                }
            }
        }
    }
    SequencerPanel: FishPanel {
        label = {bg: {color: (COLOR_MIX)}, label = {text: "Sequencer"}}
        walk: {width: Fill, height: Fill}
        body = {
            walk: {width: Fill, height: Fill}
            Frame {
                walk: {height: Fit}
                playpause = InstrumentCheckbox {
                    checkbox = {
                        bind: "sequencer.playing",
                        label: "Play"
                    }
                }

                arp = InstrumentCheckbox {
                    checkbox = {
                        bind: "arp.enabled",
                        label: "Arpeggiator"
                    }
                }
            }
            speed = InstrumentSlider {
                slider = {
                    slider: {line_color: (COLOR_MIX)}
                    bind: "sequencer.bpm"
                    min: 0.0
                    max: 240.0
                    label: "BPM"
                }
                sequencer = Sequencer {
                }
            }
        }
    }  
    MixerPanel: FishPanel {
        label = {bg: {color: (COLOR_MIX)}, label = {text: "Mixer"}}
        body = {
            balance = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: (COLOR_MIX)}
                    bind: "osc_balance"
                    min: 0.0
                    max: 1.0
                    label: "Oscillator 1/2 Balance"
                }
            }
            twocol = Frame {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                noise = InstrumentSlider {
                    slider = {
                        slider: {line_color: (COLOR_MIX)}
                        bind: "noise"
                        min: 0.0
                        max: 1.0
                        label: "Noise"
                    }
                }
                sub = InstrumentSlider {
                    slider = {
                        slider: {line_color: (COLOR_MIX)}
                        bind: "sub_osc"
                        min: 0.0
                        max: 1.0
                        label: "Sub Oscillator"
                    }
                }
            }
        }        
    }
    FXPanel: FishPanel {
        label = {bg: {color: (COLOR_FX)}, label = {text: "Effects",}}
        body = {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit}
            delaysend = InstrumentSlider {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "fx.delaysend"
                    min: 0.0
                    max: 1.0
                    label: "Delay Send"
                }
            }
            delayfeedback = InstrumentSlider {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "fx.delayfeedback"
                    min: 0.0
                    max: 1.0
                    label: "Delay Feedback"

                }
            }
        }
    }
    LFOPanel: FishPanel {
        label = {bg: {color: (COLOR_LFO)}, label = {text: "LFO"}}
        body = {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit}
            rate = InstrumentSlider {
                slider = {
                    slider: {line_color: (COLOR_LFO)}
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
        label = {bg: {color: (COLOR_ENV)}, label = {text: "Volume Env"}}
        body = {
            vol_env = EnvelopePanel {
                attack = { slider = {bind: "volume_envelope.a"}}
                hold = { slider = {bind: "volume_envelope.h"}}
                decay = {slider = {bind: "volume_envelope.d"}}
                sustain = {slider = {bind: "volume_envelope.s"}}
                release = {slider = {bind: "volume_envelope.r"}}
            }
        }
    }
    
    ModEnvelopePanel: FishPanel {
        label = {bg: {color: (COLOR_ENV)}, label = {text: "Modulation Env"}}
        body = {
            mod_env = EnvelopePanel {
                attack = { slider = {bind: "mod_envelope.a"}}
                hold = { slider = {bind: "mod_envelope.h"}}
                decay = {slider = {bind: "mod_envelope.d"}}
                sustain = {slider = {bind: "mod_envelope.s"}}
                release = {slider = {bind: "mod_envelope.r"}}
            }
        }
    }
    
    FilterPanel: FishPanel {
        label = {bg: {color: (COLOR_FILTER)}, label = {text: "Filter"}}
        walk: {width: Fill, height: Fill}
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fill}
            InstrumentDropdown {
                label = {text: "Filter"}
                dropdown = {
                    bind_enum: "FilterType"
                    bind: "filter1.filter_type"
                    items: ["LowPass", "HighPass", "BandPass", "BandReject"]
                }
            }
           
            cutoff = InstrumentSlider {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.cutoff"
                    min: 0.0
                    max: 1.0
                    label: "Cutoff"
                }
            }
            
            resonance = InstrumentSlider {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.resonance"
                    min: 0.0
                    max: 1.0
                    label: "Resonance"
                }
            }
            modamount = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.envelope_amount"
                    min: -1.0
                    max: 1.0
                    label: "Mod Env Amount"
                }
            }
            lfoamount = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.lfo_amount"
                    min: -1.0
                    max: 1.0
                    label: "LFO Amount"
                }
            }
            touchamount = InstrumentBipolarSlider {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.touch_amount"
                    min: -1.0
                    max: 1.0
                    label: "Touch Amount"
                }
            }
        }
    }
    
    OscPanel: FishPanel {
        label = {bg: {color: (COLOR_OSC)}, label = {text: "Oscillator ?"}}
        body = {
            type = InstrumentDropdown {
                label = {text: "Type"}
                dropdown = {
                    bind_enum: "OscType"
                    bind: "osc1.osc_type"
                    items: ["DPWSawPulse","BlampTri",  "Pure"]
                    display: ["Saw", "Triangle",  "Sine"]
                }
            }
            
            twocol = Frame {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                transpose = InstrumentBipolarSlider {
                    slider = {
                        slider: {line_color:(COLOR_OSC)}
                        bind: "osc1.transpose"
                        min: -24.0
                        max: 24.0
                        label: "Transpose"
                    }
                }
                
                detune = InstrumentBipolarSlider {
                    slider = {
                        slider: {line_color: (COLOR_OSC)}
                        bind: "osc1.detune"
                        min: -1.0
                        max: 1.0
                        label: "Detune"
                    }
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
                padding: 0
                align: {x: 0.0, y: 0.0}
                spacing: 0.,
                flow: Flow::Down
            },
            Frame {
                Frame {
                    walk: {width: Fill, height: Fit, margin: {bottom: 5, left: 80, top: 10}}
                    bg: {color: #f00}
                    layout: {flow: Down, spacing: 8.0, align: {x: 1.0, y: 1.0}}
                    Frame {
                        walk: {width: Fill, height: Fit}
                        layout: {flow: Right, spacing: 2.0}
                        save1 = FishButton {text: "1"}
                        save2 = FishButton {text: "2"}
                        save3 = FishButton {text: "3"}
                        save4 = FishButton {text: "4"}
                        save5 = FishButton {text: "5"}
                        save6 = FishButton {text: "6"}
                        save7 = FishButton {text: "7"}
                        save8 = FishButton {text: "8"}
                        panic = FishButton {text: "Panic", walk: {width: Fit}}
                    }
                }
            
                layout: {flow: Right, spacing: 0.0, padding: {bottom: -50}, align: {x: 1.0, y: 0.0}}
                walk: {margin: {left: 0, right: 5}, width: Fill, height: Fit}
                Image {
                    image: d"resources/tinrs.png",
                    walk: {width: (1000 * 0.25), height: (175 * 0.25)}
                }
            }
            GradientY {
                walk: {width: Fill, height: 150, margin: {top: 0, right: 0, bottom: 0, left: 0}}
                bg: {color: #100A, color2: #0034}
                display_audio = DisplayAudio {
                    walk: {height: Fill, width: Fill}
                }
            }
            
            piano = Piano {
                walk: {width: Fill, height: Fit, margin: 0.0}
            }
            GradientY {
                walk: {width: Fill, height: 25, margin: {top: 0, left: 0}}
                bg: {color: #131820FF, color2: #13182000}
            }

            Frame {
                walk: { margin: {top: (SPACING_PANELS), right: (SPACING_PANELS * 1.5), bottom: (SPACING_PANELS), left: (SPACING_PANELS * 1.5)}}
                layout: {flow: Right, spacing: (SPACING_PANELS)}
                Frame {
                    layout: {flow: Down, spacing: (SPACING_PANELS)}
                    Frame {
                        walk: {height: Fit, width: Fill}
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        OscPanel {
                            label = {label = {text: "Oscillator 1"}}
                            body = {
                                type = {dropdown = {bind: "osc1.osc_type"}}
                                twocol = {transpose = {slider = {bind: "osc1.transpose"}}}
                                twocol = {detune = {slider = {bind: "osc1.detune"}}}
                            }
                        }
                        OscPanel {
                            label = {label = {text: "Oscillator 2"}}
                            body = {
                                type = {dropdown = {bind: "osc2.osc_type"}}
                                twocol = {transpose = {slider = {bind: "osc2.transpose"}}}
                                twocol = {detune = {slider = {bind: "osc2.detune"}}}
                            }
                        }
                    }
                    Frame {
                        walk: {height: Fit, width: Fill}
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        MixerPanel {}
                        TouchPanel {}
                    }
                    Frame {
                        walk: {height: Fit, width: Fill}
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        ModEnvelopePanel {}
                        VolumeEnvelopePanel {}
                        FilterPanel {}
                    }
                    Frame {
                        walk: {height: Fit, width: Fill}
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        LFOPanel {}
                        FXPanel {}
                    }
                }
                Frame {
                    layout: {flow: Down, spacing: (SPACING_PANELS)}
                    SequencerPanel {}
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
        crate::sequencer::live_register(cx);
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
                if let Some(value) = delta.read_path(&bind.name) {
                    if let Some(v) = value.as_float() {
                        
                        let mod_env = ui.frame(ids!(mod_env.display));
                        let vol_env = ui.frame(ids!(vol_env.display));
                        match bind.name.as_ref() {
                            "mod_envelope.a" => mod_env.apply_over(ui.cx, live!{bg: {attack: (v)}}),
                            "mod_envelope.h" => mod_env.apply_over(ui.cx, live!{bg: {hold: (v)}}),
                            "mod_envelope.d" => mod_env.apply_over(ui.cx, live!{bg: {decay: (v)}}),
                            "mod_envelope.s" => mod_env.apply_over(ui.cx, live!{bg: {sustain: (v)}}),
                            "mod_envelope.r" => mod_env.apply_over(ui.cx, live!{bg: {release: (v)}}),
                            "volume_envelope.a" => vol_env.apply_over(ui.cx, live!{bg: {attack: (v)}}),
                            "volume_envelope.h" => vol_env.apply_over(ui.cx, live!{bg: {hold: (v)}}),
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
        
        let sequencer = ui.sequencer(ids!(sequencer));
        
        for (btn_x,btn_y,active) in sequencer.on_buttons(){
            let iron_fish = self.audio_graph.by_type::<IronFish>().unwrap();
            let s = iron_fish.settings.clone();
            let bit = 1<<btn_y;
            let act = if active{bit} else{0};
            match btn_x{
                0=>s.sequencer.step0.set(s.sequencer.step0.get()^bit|act),
                1=>s.sequencer.step1.set(s.sequencer.step1.get()^bit|act),
                2=>s.sequencer.step2.set(s.sequencer.step2.get()^bit|act),
                3=>s.sequencer.step3.set(s.sequencer.step3.get()^bit|act),
                4=>s.sequencer.step4.set(s.sequencer.step4.get()^bit|act),
                5=>s.sequencer.step5.set(s.sequencer.step5.get()^bit|act),
                6=>s.sequencer.step6.set(s.sequencer.step6.get()^bit|act),
                7=>s.sequencer.step7.set(s.sequencer.step7.get()^bit|act),
                8=>s.sequencer.step8.set(s.sequencer.step8.get()^bit|act),
                9=>s.sequencer.step9.set(s.sequencer.step9.get()^bit|act),
                10=>s.sequencer.step10.set(s.sequencer.step10.get()^bit|act),
                11=>s.sequencer.step11.set(s.sequencer.step11.get()^bit|act),
                12=>s.sequencer.step12.set(s.sequencer.step12.get()^bit|act),
                13=>s.sequencer.step13.set(s.sequencer.step13.get()^bit|act),
                14=>s.sequencer.step14.set(s.sequencer.step14.get()^bit|act),
                15=>s.sequencer.step15.set(s.sequencer.step15.get()^bit|act),
                _=>()
            }
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
            let data = nodes.to_cbor(0).unwrap();
            let data = makepad_miniz::compress_to_vec(&data, 10);
            log!("Saving preset {}", file_name);
            let mut file = File::create(&file_name).unwrap();
            file.write_all(&data).unwrap();
        }
        else if let Ok(mut file) = File::open(&file_name) {
            log!("Loading preset {}", file_name);
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            //if let Ok(data) = makepad_miniz::decompress_to_vec(&data) {
                let mut nodes = Vec::new();
                nodes.from_cbor(&data).unwrap();
                
                iron_fish.settings.apply_over(cx, &nodes);
                self.imgui.root_frame().bind_read(cx, &nodes);
            //}
            //else {
           //     log!("Error decompressing preset");
            //}
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