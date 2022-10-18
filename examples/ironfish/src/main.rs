pub use makepad_widgets;
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_math;
pub use makepad_media;

use makepad_widgets::*;
use makepad_draw_2d::*;
use makepad_media::*;
use makepad_media::audio_graph::*;

mod sequencer;
mod display_audio;
mod piano;
mod ironfish;
mod waveguide;

use crate::ironfish::*;
use crate::piano::*;
use crate::sequencer::*;
use crate::display_audio::*;

use std::fs::File;
use std::io::prelude::*;


live_design!{
    registry AudioComponent::*;
    registry Widget::*;
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_draw_2d::shader::std::*;
    
    const SPACING_PANELS = 10.0
    const SPACING_CONTROLS = 4.0
    const COLOR_OSC = #xFFFF99FF // yellow
    const COLOR_MIX = #xC // gray
    const COLOR_ENV = #xFFC499 // light red
    const COLOR_FILTER = #xA7BEF2 // indigo
    const COLOR_LFO = #xFF9999 // red
    const COLOR_TOUCH = #xBBFF99 // light green
    const COLOR_FX = #x99EEFF // light green
    const COLOR_TEXT_H1 = #x000000CC
    const COLOR_TEXT_H2 = #xFFFFFF66
    const COLOR_TEXT_H2_HOVER = #xD
    const COLOR_BEVEL_SHADOW = #x00000066
    const COLOR_BEVEL_HIGHLIGHT = #xFFFFFF33
    const COLOR_CONTROL_OUTSET = #xFFFFFF66
    const COLOR_HIDDEN_WHITE = #xFFFFFF00
    const COLOR_CONTROL_INSET = #x00000066
    const COLOR_CONTROL_INSET_HOVER = #x00000088
    const COLOR_TODO = #xFF1493FF
    const FONT_SIZE_H1 = 11.0
    const FONT_SIZE_H2 = 9.5
    
    ElementBox = <Frame> {
        bg: {color: #4}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: {left: (SPACING_CONTROLS), top: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), right: (SPACING_CONTROLS)}, spacing: (SPACING_CONTROLS)}
    }
    
    FishDropDown = <DropDown> {
        walk: {margin: {left: 5.0, right: 0.0, top: 0.0, bottom: 0.0}}
        layout: {padding: 6.0}
        label: {
            // DrawLabelText= {{DrawLabelText}} {
            // },
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"resources/IBMPlexSans-SemiBold.ttf"}},
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            (COLOR_TEXT_H2),
                            (COLOR_TEXT_H2),
                            self.focus
                        ),
                        (COLOR_TEXT_H2),
                        self.hover
                    ),
                    (COLOR_TEXT_H2),
                    self.pressed
                )
            }
        }
        
        
        popup_menu: {
            bg: {color: (COLOR_TODO)}
            menu_item: {
                indent_width: 10.0
                layout: {
                    padding: {left: 15, top: 5, bottom: 5},
                }
            }
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
                sdf.stroke_keep(mix(mix((COLOR_BEVEL_HIGHLIGHT), (COLOR_BEVEL_SHADOW), pow(self.pos.y, 1.0)), mix((COLOR_BEVEL_SHADOW), (COLOR_BEVEL_HIGHLIGHT), pow(self.pos.y, 5.0)), self.pressed), 1.);
                sdf.fill(
                    mix(
                        mix(
                            mix((COLOR_CONTROL_OUTSET), (COLOR_HIDDEN_WHITE), pow(self.pos.y, 0.075)),
                            mix(#xFFFFFF20, #xFFFFFF10, pow(self.pos.y, 0.2)),
                            self.hover
                        ),
                        mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 0.3)),
                        self.pressed
                    )
                );
            }
        }
    }
    
    FishButton = <Button>{
        label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"resources/IBMPlexSans-SemiBold.ttf"}}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_TEXT_H2),
                        (COLOR_TEXT_H2),
                        self.hover
                    ),
                    (COLOR_TEXT_H2),
                    self.pressed
                )
            }
        }
        bg: {
            instance hover: 0.0
            instance pressed: 0.0
            
            const BORDER_RADIUS = 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    2.0
                )
                
                sdf.stroke_keep(mix(mix((COLOR_BEVEL_HIGHLIGHT), (COLOR_BEVEL_SHADOW), pow(self.pos.y, 1.0)), mix((COLOR_BEVEL_SHADOW), (COLOR_BEVEL_HIGHLIGHT), pow(self.pos.y, 5.0)), self.pressed), 1.);
                sdf.fill(
                    mix(
                        mix(
                            mix((COLOR_CONTROL_OUTSET), (COLOR_HIDDEN_WHITE), pow(self.pos.y, 0.075)),
                            mix(#xFFFFFF20, #xFFFFFF10, pow(self.pos.y, 0.2)),
                            self.hover
                        ),
                        mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 0.3)),
                        self.pressed
                    )
                );
                
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
            padding: 6
        }
    }
    
    FishSlider = <Slider> {
        walk: {
            height: 36,
            margin: {left: 0.0, right: 1.0, top: 0.0, bottom: 0.0},
        }
        label: "CutOff1"
        label_text: {text_style: {font_size: (FONT_SIZE_H2), font: {path: d"resources/IBMPlexSans-SemiBold.ttf"}}, color: (COLOR_TEXT_H2)}
        text_input: {
            cursor_margin_bottom: 3.0,
            cursor_margin_top: 4.0,
            select_pad_edges: 3.0
            cursor_size: 2.0,
            empty_message: "0",
            numeric_only: true,
            bg: {
                shape: None
                color: #5
                radius: 2
            },
            layout: {
                padding: 0,
                align: {y: 0.}
            },
            walk: {
                margin: {top: 3, right: 5}
            }
        }
        slider: {
            instance line_color: #f00
            instance bipolar: 0.0
            fn pixel(self) -> vec4 {
                let nub_size = 3
                
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 20.0;
                
                sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, 1);
                sdf.fill_keep(
                    mix(
                        mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 1.0)),
                        mix((COLOR_CONTROL_INSET) * 1.75, (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                ) // Control backdrop gradient
                
                sdf.stroke(mix(mix(#x00000060, #x00000070, self.drag), #xFFFFFF10, pow(self.pos.y, 10.0)), 1.0) // Control outline
                let in_side = 5.0;
                let in_top = 5.0; // Ridge: vertical position
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(mix(#x00000066, #00000088, self.drag)); // Ridge color
                let in_top = 7.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(#FFFFFF18); // Ridge: Rim light catcher
                
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);
                
                sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
                sdf.stroke_keep(mix((COLOR_HIDDEN_WHITE), self.line_color, self.drag), 1.5)
                sdf.stroke(
                    mix(mix(self.line_color * 0.85, self.line_color, self.hover), #xFFFFFF80, self.drag),
                    1
                )
                
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 3) - 3;
                sdf.box(nub_x + in_side, top + 1.0, 12, 12, 1.)
                
                sdf.fill_keep(mix(mix(#x7, #x8, self.hover), #3, self.pos.y)); // Nub background gradient
                sdf.stroke(
                    mix(
                        mix(#xa, #xC, self.hover),
                        #0,
                        pow(self.pos.y, 1.5)
                    ),
                    1.
                ); // Nub outline gradient
                
                
                return sdf.result
            }
        }
    }
    
    InstrumentSlider = <ElementBox> {
        slider = <FishSlider> {
            slider: {bipolar: 0.0}
        }
    }
    
    InstrumentBipolarSlider = <ElementBox> {
        slider = <FishSlider> {
            slider: {bipolar: 1.0}
        }
    }
    
    InstrumentCheckbox = <ElementBox> {
        layout: {padding: 0.0}
        checkbox = <CheckBox> {
            layout: {padding: 2.5}
            label: "CutOff1"
            check_box: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let left = 3;
                    let sz = 7.0;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.box(left, c.y - sz, sz * 2.0, sz * 2.2, 1.5); // rounding = 3rd value
                    sdf.fill_keep(mix(
                        mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 1.)),
                        mix((COLOR_CONTROL_INSET_HOVER), (COLOR_CONTROL_INSET_HOVER) * 0.1, pow(self.pos.y, 1.0)),
                        self.hover
                    ))
                    sdf.stroke(mix((COLOR_BEVEL_SHADOW), (COLOR_BEVEL_HIGHLIGHT), pow(self.pos.y, 5.0)), 1.0) // outline
                    let szs = sz * 0.5;
                    let dx = 1.0;
                    sdf.move_to(left + 4.0, c.y);
                    sdf.line_to(c.x, c.y + szs);
                    sdf.line_to(c.x + szs, c.y - szs);
                    sdf.stroke(mix((COLOR_HIDDEN_WHITE), mix((COLOR_TEXT_H2), (COLOR_TEXT_H2_HOVER), self.hover), self.selected), 1.25);
                    return sdf.result
                }
            }
            label_text: {text_style: {font_size: (FONT_SIZE_H2), font: {path: d"resources/IBMPlexSans-SemiBold.ttf"}}, color: (COLOR_TEXT_H2)}
            walk: {margin: {top: 3, right: 0, bottom: 5, left: 0}}
            label_walk: {
                margin: {left: 23.0, top: 0, bottom: 0, right: 0}
            }
        }
    }
    
    InstrumentDropdown = <ElementBox> {
        layout: {align: {y: 0.5}, padding: 0, flow: Right}
        label = <Label> {walk: {width: Fit, margin: {right: 5}}, label: {
            color: (COLOR_TEXT_H2)
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"resources/IBMPlexSans-SemiBold.ttf"}},
            color: (COLOR_TEXT_H2)
        }}
        walk: {margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0}}
        dropdown = <FishDropDown> {}
    }
    
    GraphPaper = <Box> {
        walk: {width: Fill, height: 100, margin: {top: -4.0, right: -4.0, bottom: 0.0, left: -4.0,}}
        bg: {
            radius: 0,
            color: #x44,
            color2: #x0,
            
            instance attack: 0.05
            instance hold: 0.0
            instance decay: 0.2
            instance sustain: 0.5
            instance release: 0.2
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size); //mod (self.pos * self.rect_size, 15))
                let base_color = mix(self.color, self.color2, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 2.0));
                let darker = base_color * 0.85;
                let pos = self.pos * self.rect_size;
                sdf.clear(mix(base_color, darker, pow(abs(sin(pos.x * 0.5)), 24) + pow(abs(sin(pos.y * 0.5)), 32.0))); // Grid
                sdf.rect(1.0, 1.0, 16, 16)
                sdf.stroke(darker, 1)
                let pad_b = 8
                let pad_s = 8
                let width = self.rect_size.x - 2 * pad_s
                let height = self.rect_size.y - 2 * pad_b
                let total = self.attack + self.decay + self.release + 0.5 + self.hold
                let sustain = self.rect_size.y - pad_b - height * self.sustain;
                sdf.pos = self.pos * self.rect_size;
                sdf.move_to(pad_s, self.rect_size.y - pad_b)
                sdf.line_to(pad_s + width * (self.attack / total), pad_b)
                sdf.line_to(pad_s + width * ((self.attack + self.hold) / total), pad_b)
                sdf.line_to(pad_s + width * ((self.attack + self.decay + self.hold) / total), sustain)
                sdf.line_to(pad_s + width * (1.0 - self.release / total), sustain)
                sdf.line_to(pad_s + width, self.rect_size.y - pad_b)
                sdf.stroke_keep(#xFFC49910, 8.0);
                sdf.stroke_keep(#xFFC49910, 6.0);
                sdf.stroke_keep(#xFFC49920, 4.0);
                sdf.stroke_keep(#xFFC49980, 2.0);
                sdf.stroke(#xFFFFFFFF, 1.0);
                /*let fill = sdf.result;
                sdf.result = #0000;
                sdf.box(0.,0.,self.rect_size.x, self.rect_size.y, 3.); 
                sdf.fill(fill);
                */
                return sdf.result
            }
        }
    }
    EnvelopePanel = <Frame> {
        layout: {flow: Down}
        walk: {width: Fill, height: Fill}
        display = <GraphPaper> {
        }
        attack = <InstrumentSlider> {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                min: 0.0
                max: 1.0
                label: "A"
            }
        }
        hold = <InstrumentSlider> {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                walk: {margin: 0.0}
                min: 0.0
                max: 1.0
                label: "H"
            }
        }
        decay = <InstrumentSlider> {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                min: 0.0
                max: 1.0
                label: "D"
            }
        }
        sustain = <InstrumentSlider> {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                min: 0.0
                max: 1.0
                label: "S"
            }
        }
        release = <InstrumentSlider> {
            slider = {
                slider: {line_color: (COLOR_ENV)}
                min: 0.0
                max: 1.0
                label: "R"
            }
        }
    }
    
    FishHeader = <Solid> {
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
        label = <Label> {
            label: {text_style: {font_size: (FONT_SIZE_H1), font: {path: d"resources/IBMPlexSans-SemiBold.ttf"}}, color: (COLOR_TEXT_H1)}
            text: "replace me!"
        }
    }
    
    
    FishPanel = <Frame> {
        layout: {flow: Down, flow: Down, clip_y: true, clip_x: true}
        walk: {width: Fill, height: Fit}
        label = <FishHeader> {label = {text: "ReplaceMe", walk: {margin: {top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS)}}}}
        body = <Box> {
            layout: {flow: Down, padding: {top: (SPACING_CONTROLS), left: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
            walk: {width: Fill, height: Fit, margin: {top: -3, left: 0.25}}
            bg: {
                color: #FFFFFF00
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    let edge = 8.0;
                    sdf.move_to(1.0, 1.0);
                    sdf.line_to(self.rect_size.x - 2.0, 1.0);
                    sdf.line_to(self.rect_size.x - 2.0, self.rect_size.y - edge)
                    sdf.line_to(self.rect_size.x - edge, self.rect_size.y - 2.0)
                    sdf.line_to(1.0, self.rect_size.y - 2.0);
                    sdf.close_path();
                    sdf.fill_keep(mix(#xFFFFFF40, #xFFFFFF10, pow(self.pos.y, 0.20)));
                    sdf.stroke(self.color, 1.0)
                    return sdf.result
                }
            }
        }
    }
    
    TouchPanel = <FishPanel> {
        label = {bg: {color: (COLOR_TOUCH)}, label = {text: "Touch"}}
        body = {
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                scale = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_TOUCH)}
                        slider: {line_color: (COLOR_TOUCH)}
                        slider: {line_color: (COLOR_TOUCH)}
                        min: -1.0
                        max: 1.0
                        label: "Scale"
                    }
                }
                curve = <InstrumentSlider> {
                    slider = {
                        slider: {line_color: (COLOR_TOUCH)}
                        min: 0.0
                        max: 1.0
                        label: "Curvature"
                    }
                }
            }
            twocol = <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                offset = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_TOUCH)}
                        min: -1.0
                        max: 1.0
                        label: "Offset"
                    }
                }
                touchamount = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_TOUCH)}
                        min: -1.0
                        max: 1.0
                        label: "Touch -> Cutoff"
                    }
                }
            }
        }
    }

    SequencerPanel = <FishPanel> {
        label = {bg: {color: (COLOR_MIX)}, label = {text: "Sequencer"}}
        walk: {width: Fill, height: Fill}
        body = {
            walk: {width: Fill, height: Fill}
            <Frame> {
                walk: {height: Fit}
                layout: {flow: Right}
                
                playpause = <InstrumentCheckbox> {
                    walk: {width: Fit, height: Fit, margin: 5}
                    layout: {align: {x: 0.0, y: 0.5}}
                    checkbox = {
                        walk: {width: 20, height: 20, margin: 5}
                        label: ""
                        check_box: {
                            fn pixel(self) -> vec4 {
                                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                                let left = 3;
                                let sz = 20.0;
                                let c = vec2(left + sz, self.rect_size.y);
                                sdf.move_to(0.0, 0.0);
                                sdf.line_to(c.x * 0.75, c.y * 0.5);
                                sdf.line_to(0.0, c.y);
                                sdf.close_path();
                                sdf.fill_keep(
                                    mix(
                                        mix((COLOR_TEXT_H2) * 0.75, (COLOR_TEXT_H2), self.hover),
                                        mix(
                                            mix(#xFFFDDDFF, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                                            mix(#xFFFDDDFF, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                                            self.hover
                                        ),
                                        self.selected
                                    )
                                )
                                sdf.stroke_keep(
                                    mix(
                                        mix(#xFFFFFF66, #xFFFFFF10, pow(self.pos.y, 0.5)),
                                        #xFFFFFF80,
                                        self.selected
                                    ),
                                    1.
                                )
                                return sdf.result
                            }
                        }
                    }
                }
                
                speed = <InstrumentSlider> {
                    walk: {width: 200}
                    slider = {
                        slider: {line_color: (COLOR_MIX)}
                        min: 0.0
                        max: 240.0
                        label: "BPM"
                    }
                }
                
                rootnote = <InstrumentDropdown> {
                    walk: {height: Fill}
                    layout: {align: {x: 0.0, y: 0.5}}
                    dropdown = {
                        layout: {align: {x: 0.0, y: 0.0}}
                        walk: {width: Fill, height: Fit, margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0.0}}
                        display: ["A", "A#", "B", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#"]
                        bind_enum: "RootNote"
                        bind: "sequencer.rootnote"
                        items: ["A", "Asharp", "B", "C", "Csharp", "D", "Dsharp", "E", "F", "Fsharp", "G", "Gsharp"]
                    }
                }
                
                scaletype = <InstrumentDropdown> {
                    walk: {height: Fill}
                    layout: {align: {x: 0.0, y: 0.5}}
                    dropdown = {
                        layout: {align: {x: 0.0, y: 0.0}}
                        walk: {width: Fill, height: Fit, margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0.0}}
                        items: ["Minor", "Major", "Dorian", "Pentatonic"]
                        bind: "sequencer.scale"
                        bind_enum: "MusicalScale"
                        display: ["Minor", "Major", "Dorian", "Pentatonic"]
                    }
                }
                
                arp = <InstrumentCheckbox> {
                    walk: {width: Fit, height: Fill, margin: 5}
                    layout: {align: {x: 0.0, y: 0.5}}
                    checkbox = {
                        bind: "arp.enabled",
                        label: "Arp"
                    }
                }
                
                <Frame> {
                    walk: {width: Fit, height: Fill}
                    layout: {align: {x: 0.0, y: 0.5}}
                    clear_grid = <FishButton> {
                        text: "Clear Grid"
                        walk: {width: Fit, height: Fit, margin: 5}
                    }
                    grid_up = <FishButton> {
                        text: "Up"
                        walk: {width: Fit, height: Fit, margin: 5}
                    }
                    grid_down = <FishButton> {
                        text: "Down"
                        walk: {width: Fit, height: Fit, margin: 5}
                    }
                }
            }
            
            sequencer = <Sequencer> {
                walk: {width: Fill, height: Fill}
            }
        }
    }
    
    
    MixerPanel = <FishPanel> {
        label = {bg: {color: (COLOR_MIX)}, label = {text: "Mixer"}}
        body = {
            balance = <InstrumentBipolarSlider> {
                slider = {
                    slider: {line_color: (COLOR_MIX)}
                    bind: "osc_balance"
                    min: 0.0
                    max: 1.0
                    label: "Oscillator 1/2 Balance"
                }
            }
            twocol = <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                noise = <InstrumentSlider> {
                    slider = {
                        slider: {line_color: (COLOR_MIX)}
                        bind: "noise"
                        min: 0.0
                        max: 1.0
                        label: "Noise"
                    }
                }
                sub = <InstrumentSlider> {
                    slider = {
                        slider: {line_color: (COLOR_MIX)}
                        bind: "sub_osc"
                        min: 0.0
                        max: 1.0
                        label: "Sub"
                    }
                }
                porta = <InstrumentSlider> {
                    slider = {
                        slider: {line_color: (COLOR_MIX)}
                        bind: "portamento"
                        min: 0.0
                        max: 1.0
                        label: "Portamento"
                    }
                }
            }
        }
    }
    
    DelayFXPanel = <FishPanel> {
        label = {bg: {color: (COLOR_FX)}, label = {text: "Delay",}}
        body = {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit}
            delaysend = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "fx.delaysend"
                    min: 0.0
                    max: 1.0
                    label: "Delay Send"
                }
            }
            delayfeedback = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "fx.delayfeedback"
                    min: 0.0
                    max: 1.0
                    label: "Delay Feedback"
                    
                }
            }
            delaydifference = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "fx.difference"
                    min: 0.0
                    max: 1.0
                    label: "Delay Stereo"
                }
            }
            delaycross = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "fx.cross"
                    min: 0.0
                    max: 1.0
                    label: "Delay Cross"
                    
                }
            }
        }
    }
    
    ChorusFXPanel = <FishPanel> {
        label = {bg: {color: (COLOR_FX)}, label = {text: "Chorus",}}
        body = {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit}
            chorusmix = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "chorus.mix"
                    min: 0.0
                    max: 1.0
                    label: "Mix"
                }
            }
            chorusdelay = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "chorus.mindelay"
                    min: 0.0
                    max: 1.0
                    label: "Pre"
                    
                }
            }
            chorusmod = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "chorus.moddepth"
                    min: 0.0
                    max: 1.0
                    label: "Depth"
                }
            }
            chorusrate = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "chorus.rate"
                    min: 0.0
                    max: 1.0
                    label: "Rate"
                    
                }
            }
            chorusphase = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "chorus.phasediff"
                    min: 0.0
                    max: 1.0
                    label: "Phasing"
                    
                }
            }
            
            chorusfeedback = <InstrumentBipolarSlider> {
                slider = {
                    slider: {line_color: (COLOR_FX)}
                    bind: "chorus.feedback"
                    min: -1
                    max: 1
                    label: "Feedback"
                    
                }
            }
        }
    }
    
    LFOPanel = <FishPanel> {
        label = {bg: {color: (COLOR_LFO)}, label = {text: "LFO"}}
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            rate = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_LFO)}
                    bind: "lfo.rate"
                    min: 0.0
                    max: 1.0
                    label: "Rate"
                }
            }
            lfoamount = <InstrumentBipolarSlider> {
                slider = {
                    slider: {line_color: (COLOR_LFO)}
                    bind: "filter1.lfo_amount"
                    min: -1.0
                    max: 1.0
                    label: "LFO -> Cutoff"
                }
            }
            sync = <InstrumentCheckbox> {
                walk: {height: Fit, margin: 3.0}
                layout: {flow: Down, spacing: 0.0, align: {x: 0.0, y: 1.0}}
                checkbox = {
                    bind: "lfo.synconkey",
                    label: "Key sync"
                }
            }
        }
    }
    
    VolumeEnvelopePanel = <FishPanel> {
        label = {bg: {color: (COLOR_ENV)}, label = {text: "Volume Env"}}
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fill}
            vol_env = <EnvelopePanel> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fill}
                attack = {slider = {bind: "volume_envelope.a"}}
                hold = {slider = {bind: "volume_envelope.h"}}
                decay = {slider = {bind: "volume_envelope.d"}}
                sustain = {slider = {bind: "volume_envelope.s"}}
                release = {slider = {bind: "volume_envelope.r"}}
            }
        }
    }
    
    ModEnvelopePanel = <FishPanel> {
        label = {bg: {color: (COLOR_ENV)}, label = {text: "Modulation Env"}}
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fill}
            mod_env = <EnvelopePanel> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
                
                attack = {slider = {bind: "mod_envelope.a"}}
                hold = {slider = {bind: "mod_envelope.h"}}
                decay = {slider = {bind: "mod_envelope.d"}}
                sustain = {slider = {bind: "mod_envelope.s"}}
                release = {slider = {bind: "mod_envelope.r"}}
            }
            modamount = <InstrumentBipolarSlider> {
                slider = {
                    slider: {line_color: (COLOR_ENV)}
                    bind: "filter1.envelope_amount"
                    min: -1.0
                    max: 1.0
                    label: "Mod -> Cutoff"
                }
            }
        }
    }
    
    FilterPanel = <FishPanel> {
        label = {bg: {color: (COLOR_FILTER)}, label = {text: "Filter"}}
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fill}
            <InstrumentDropdown> {
                walk: {margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0.0}}
                dropdown = {
                    bind_enum: "FilterType"
                    bind: "filter1.filter_type"
                    items: ["LowPass", "HighPass", "BandPass", "BandReject"]
                }
            }
            
            cutoff = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.cutoff"
                    min: 0.0
                    max: 1.0
                    label: "Cutoff"
                }
            }
            
            resonance = <InstrumentSlider> {
                slider = {
                    slider: {line_color: (COLOR_FILTER)}
                    bind: "filter1.resonance"
                    min: 0.0
                    max: 1.0
                    label: "Resonance"
                }
            }
        }
    }
    
    OscPanel = <FishPanel> {
        label = {bg: {color: (COLOR_OSC)}, label = {text: "Oscillator ?"}}
        body = {
            type = <InstrumentDropdown> {
                layout: {flow: Down}
                walk: {margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0.0}}
                dropdown = {
                    bind_enum: "OscType"
                    bind: "osc1.osc_type"
                    items: ["DPWSawPulse", "BlampTri", "Pure", "SuperSaw", "HyperSaw", "HarmonicSeries"]
                    display: ["Saw", "Triangle", "Sine", "Super Saw", "Hyper Saw", "Harmonic"]
                }
                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit}
                    detune = <InstrumentSlider> {
                        slider = {
                            slider: {line_color: (COLOR_OSC)}
                            bind: "supersaw1.spread"
                            min: 0.0
                            max: 1.0
                            label: "Spread"
                        }
                    }
                    mix = <InstrumentSlider> {
                        slider = {
                            slider: {line_color: (COLOR_OSC)}
                            bind: "supersaw1.diffuse"
                            min: 0.0
                            max: 1.0
                            label: "Diffuse"
                        }
                    }
                }
            }
            
            twocol = <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                transpose = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_OSC)}
                        bind: "osc1.transpose"
                        min: -24.0
                        max: 24.0
                        label: "Transpose"
                    }
                }
                
                detune = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_OSC)}
                        bind: "osc1.detune"
                        min: -1.0
                        max: 1.0
                        label: "Detune"
                    }
                }
            }
            
            threecol = <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                harmonic = <InstrumentSlider> {
                    slider = {
                        slider: {line_color: (COLOR_OSC)}
                        bind: "osc1.harmonic"
                        min: 0
                        max: 1.0
                        label: "Harmonic"
                    }
                }
                harmonicenv = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_OSC)}
                        bind: "osc1.harmonicenv"
                        min: -1.0
                        max: 1.0
                        label: "Mod -> Harmonic"
                    }
                }
                harmoniclfo = <InstrumentBipolarSlider> {
                    slider = {
                        slider: {line_color: (COLOR_OSC)}
                        bind: "osc1.harmoniclfo"
                        min: -1.0
                        max: 1.0
                        label: "LFO -> Harmonic"
                    }
                }
            }
        }
    }
    
    App = {{App}} {
        window: {window: {inner_size: vec2(1280, 1000)}, pass: {clear_color: #3}}
        
        audio_graph: {
            root: <Mixer> {
                c1 = <Instrument> {
                    /*  AudioUnitInstrument {
                        plugin: "Kontakt"
                    }*/
                    <IronFish> {
                    }
                    //key_range: {start: 34, end: 47 shift: 30}
                    /*
                    AudioUnitInstrument {
                        plugin: "Kontakt"
                    }*/
                }
            }
        }
        
        ui: {
            design_mode: false,
            bg: {color: #f00},
            walk: {width: Fill, height: Fill}
            layout: {
                padding: 0
                align: {x: 0.0, y: 0.0}
                spacing: 0.,
                flow: Down
            },
            <Frame> {
                <Frame> {
                    walk: {width: Fill, height: Fit, margin: {bottom: 5, left: 80, top: 10}}
                    bg: {color: #f00}
                    layout: {flow: Down, spacing: 8.0, align: {x: 1.0, y: 1.0}}
                    <Frame> {
                        walk: {width: Fill, height: Fit}
                        layout: {flow: Right, spacing: 2.0}
                        save1 = <FishButton> {text: "1"}
                        save2 = <FishButton> {text: "2"}
                        save3 = <FishButton> {text: "3"}
                        save4 = <FishButton> {text: "4"}
                        save5 = <FishButton> {text: "5"}
                        save6 = <FishButton> {text: "6"}
                        save7 = <FishButton> {text: "7"}
                        save8 = <FishButton> {text: "8"}
                        panic = <FishButton> {text: "Panic", walk: {width: Fit}}
                    }
                }
                
                layout: {flow: Right, spacing: 0.0, padding: {bottom: -50}, align: {x: 1.0, y: 0.0}}
                walk: {margin: {left: 0, right: 5}, width: Fill, height: Fit}
                <Image> {
                    image: d"resources/tinrs.png",
                    walk: {width: (1000 * 0.25), height: (175 * 0.25)}
                }
            }
            <GradientY> {
                walk: {width: Fill, height: 150, margin: {top: 0, right: 0, bottom: 0, left: 0}}
                bg: {color: #100A, color2: #0034}
                display_audio = <DisplayAudio> {
                    walk: {height: Fill, width: Fill}
                }
            }
            
            piano = <Piano> {
                walk: {width: Fill, height: Fit, margin: 0.0}
            }
            
            <GradientY> {
                walk: {width: Fill, height: 15, margin: {top: 0, left: 0}}
                bg: {color: #131820FF, color2: #13182000}
            }
            
            <Frame> {
                walk: {margin: {top: (SPACING_PANELS), right: (SPACING_PANELS * 1.5), bottom: (SPACING_PANELS), left: (SPACING_PANELS * 1.5)}}
                layout: {flow: Right, spacing: (SPACING_PANELS)}
                <Frame> {
                    layout: {flow: Down, spacing: (SPACING_PANELS)}
                    <Frame> {
                        walk: {height: Fit, width: Fill}
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        osc1 = <OscPanel> {
                            label = {label = {text: "Oscillator 1"}}
                            body = {
                                type = {dropdown = {bind: "osc1.osc_type"}}
                                threecol = {harmonic = {slider = {bind: "osc1.harmonic"}}}
                                twocol = {transpose = {slider = {bind: "osc1.transpose"}}}
                                twocol = {detune = {slider = {bind: "osc1.detune"}}}
                            }
                        }
                        <OscPanel> {
                            label = {label = {text: "Oscillator 2"}}
                            body = {
                                type = {dropdown = {bind: "osc2.osc_type"}}
                                threecol = {harmonic = {slider = {bind: "osc2.harmonic"}}}
                                twocol = {transpose = {slider = {bind: "osc2.transpose"}}}
                                twocol = {detune = {slider = {bind: "osc2.detune"}}}
                            }
                        }
                    }
                    <Frame> {
                        walk: {height: Fit, width: Fill}
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        <MixerPanel> {walk: {width: Fill, height: Fit}}
                        touch = <TouchPanel> {}
                    }
                    <Frame> {
                        layout: {flow: Right, spacing: (SPACING_PANELS)}
                        walk: {height: Fill, width: Fill}
                        <ModEnvelopePanel> {
                            layout: {flow: Down, clip_y: true}
                            walk: {width: Fill, height: Fill}
                        }
                        <VolumeEnvelopePanel> {
                            layout: {flow: Down}
                            walk: {width: Fill, height: Fill}
                        }
                        <Frame> {
                            walk: {height: Fill, width: Fill}
                            layout: {flow: Down, spacing: (SPACING_PANELS)}
                            <LFOPanel> {
                                walk: {width: Fill, height: Fit}
                            }
                            <FilterPanel> {
                                layout: {flow: Down}
                                walk: {width: Fill, height: Fill}
                            }
                        }
                    }
                }
                <Frame> {
                    layout: {flow: Down, spacing: (SPACING_PANELS)}
                    <DelayFXPanel> {}
                    <ChorusFXPanel> {}
                    <SequencerPanel> {}
                }
            }
            
        }
    }
}
main_app!(App);

#[derive(Live)]
pub struct App {
    ui: FrameRef,
    audio_graph: AudioGraph,
    window: BareWindow,
}

impl LiveHook for App{
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode])->Option<usize>{
        //_nodes.debug_print(0,100);
        None
    }
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_media::live_design(cx);
        crate::display_audio::live_design(cx);
        crate::ironfish::live_design(cx);
        crate::piano::live_design(cx);
        crate::sequencer::live_design(cx);
    }
    
    pub fn data_bind(&mut self, cx: &mut Cx, db:&mut DataBinding, act:&WidgetActions){
        // this one should read AND write depending on what db is set to

        self.ui.get_slider(id!(touch.scale.slider)).bind_to(cx, db, id!(touch.scale), act);
        self.ui.get_slider(id!(touch.curve.slider)).bind_to(cx, db, id!(touch.curve), act);
        self.ui.get_slider(id!(touch.offset.slider)).bind_to(cx, db, id!(touch.offset), act);
        self.ui.get_slider(id!(touch.touchamount.slider)).bind_to(cx, db, id!(filter1.touch_amount), act);
        
        self.ui.get_check_box(id!(playpause.checkbox)).bind_to(cx, db, id!(sequencer.playing), act);

        self.ui.get_slider(id!(speed.slider)).bind_to(cx, db, id!(sequencer.bpm), act);

        self.ui.get_slider(id!(rootnote.dropdown)).bind_to(cx, db, id!(sequencer.rootnote), act);
        
        //let disp = self.ui.get_frame(id!(mod_env.display));
        
        /*if let Some(nodes) = db.from_widgets(){
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            ironfish.settings.apply_over(cx, &nodes);
        }*/
        // this updates the read value to the envelope 
        //disp.bind_apply(cx, db, id!(mod_envelope.a), id!(bg.attack));
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let ui = self.ui.clone();
        
        let mut db = DataBinding::new();
        
        cx.handle_midi_inputs(event);
        
        if let Event::Draw(event) = event {
            return Cx2d::draw(cx, event, self, | cx, s | s.draw(cx));
        }
        
        self.window.handle_event(cx, event);
        
        let act = ui.handle_event(cx, event);
        
        if let Event::Construct = event {
            cx.start_midi_input();
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            db.to_widgets(ironfish.settings.live_read());
            // ui.bind_read(&ironfish.settings.live_read());
            ui.get_piano(id!(piano)).set_key_focus(cx);
        }
        
        let display_audio = ui.get_display_audio(id!(display_audio));
        
        let mut buffers = 0;
        self.audio_graph.handle_event_fn(cx, event, &mut | cx, action | {
            match action {
                AudioGraphAction::DisplayAudio {buffer, voice, active} => {
                    display_audio.process_buffer(cx, active, voice, buffer);
                    buffers += 1;
                }
                AudioGraphAction::VoiceOff {voice} => {
                    display_audio.voice_off(cx, voice);
                }
            };
        });
        
        // fetch ui binding deltas
        /*
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
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            ironfish.settings.apply_over(ui.cx, &delta);
            ui.bind_read(&delta);
        }*/
        let piano = ui.get_piano(id!(piano));
        /*
        for inp in cx.handle_midi_received(event) {
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
                    
                    //ui.bind_read(&delta);
                    
                    cx.send_midi_data(MidiData {
                        data0: 0xb0,
                        data1: (3 + ring)as u8,
                        data2: (((bind.value - bind.min) / (bind.max - bind.min)) * 127.0) as u8
                    });
                    let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
                    ironfish.settings.apply_over(cx, &delta);
                }
            }
            self.audio_graph.send_midi_data(inp.data);
            if let Some(note) = inp.data.decode().on_note() {
                log!("{:?}", inp.data);
                piano.set_note(cx, note.is_on, note.note_number)
            }
        }
        */
        for note in piano.notes_played(&act) {
            self.audio_graph.send_midi_data(MidiNote {
                channel: 0,
                is_on: note.is_on,
                note_number: note.note_number,
                velocity: note.velocity
            }.into());
        }
        
        let sequencer = ui.get_sequencer(id!(sequencer));
        
        for (btn_x, btn_y, active) in sequencer.buttons_clicked(&act) {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            let _s = ironfish.settings.clone();
            let bit = 1 << btn_y;
            let act = if active {bit} else {0};
            let step = ironfish.settings.sequencer.get_step(btn_x);
            ironfish.settings.sequencer.set_step(btn_x, step ^ bit | act);
        }
        
        if ui.get_button(id!(panic)).clicked(&act) {
            self.audio_graph.all_notes_off();
        }
        
        let shift = if let Event::FingerUp(fu) = event {fu.modifiers.shift}else {false};
        if ui.get_button(id!(clear_grid)).clicked(&act) {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            for j in 0..16 {
                ironfish.settings.sequencer.set_step(j, 0);
            }
            sequencer.clear_buttons(cx);
        }
        
        if ui.get_button(id!(grid_down)).clicked(&act) {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            for j in 0..16 {
                //let bv = 1<<j;
                let step = ironfish.settings.sequencer.get_step(j);
                let mut modstep = step << 1;
                
                if (modstep & 1 << 16) == 1 << 16 {modstep += 1; modstep -= 1 << 16};
                
                ironfish.settings.sequencer.set_step(j, modstep);
            }
            
            for j in 0..16 {
                let val = ironfish.settings.sequencer.get_step(j);
                for i in 0..16 {
                    let bv = 1 << i;
                    sequencer.update_button(cx, j, i, if val & bv == bv {true} else {false});
                }
            }
        }
        
        let mut reload_sequencer = false;
        
        if ui.get_button(id!(grid_up)).clicked(&act) {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            for j in 0..16 {
                //let bv = 1<<j;
                let step = ironfish.settings.sequencer.get_step(j);
                let mut modstep = step >> 1;
                if (step & 1) == 1 {modstep += 1 << 15;}
                
                ironfish.settings.sequencer.set_step(j, modstep);
                
            }
            reload_sequencer = true;
        }
        
        if ui.get_button(id!(save1)).clicked(&act) {self.preset(cx, 1, shift); reload_sequencer = true;}
        if ui.get_button(id!(save2)).clicked(&act) {self.preset(cx, 2, shift); reload_sequencer = true;}
        if ui.get_button(id!(save3)).clicked(&act) {self.preset(cx, 3, shift); reload_sequencer = true;}
        if ui.get_button(id!(save4)).clicked(&act) {self.preset(cx, 4, shift); reload_sequencer = true;}
        if ui.get_button(id!(save5)).clicked(&act) {self.preset(cx, 5, shift); reload_sequencer = true;}
        if ui.get_button(id!(save6)).clicked(&act) {self.preset(cx, 6, shift); reload_sequencer = true;}
        if ui.get_button(id!(save7)).clicked(&act) {self.preset(cx, 7, shift); reload_sequencer = true;}
        if ui.get_button(id!(save8)).clicked(&act) {self.preset(cx, 8, shift); reload_sequencer = true;}
        
        if reload_sequencer {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            for j in 0..16 {
                let val = ironfish.settings.sequencer.get_step(j);
                for i in 0..16 {
                    let bv = 1 << i;
                    sequencer.update_button(cx, j, i, if val & bv == bv {true} else {false});
                }
            }
        }
        
        self.data_bind(cx, &mut db, &act);
    }
    
    pub fn preset(&mut self, cx: &mut Cx, index: usize, save: bool) {
        let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
        let file_name = format!("preset_{}.txt", index);
        if save {
            let nodes = ironfish.settings.live_read();
            let data = nodes.to_cbor(0).unwrap();
            let data = makepad_miniz::compress_to_vec(&data, 10);
            let data = makepad_base64::base64_encode(&data, &makepad_base64::BASE64_URL_SAFE);
            log!("Saving preset {}", file_name);
            let mut file = File::create(&file_name).unwrap();
            file.write_all(&data).unwrap();
        }
        else if let Ok(mut file) = File::open(&file_name) {
            log!("Loading preset {}", file_name);
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if let Ok(data) = makepad_base64::base64_decode(&data) {
                if let Ok(data) = makepad_miniz::decompress_to_vec(&data) {
                    let mut nodes = Vec::new();
                    nodes.from_cbor(&data).unwrap();
                    ironfish.settings.apply_over(cx, &nodes);
                    //self.imgui.root_frame().bind_read(cx, &nodes);
                }
                else {
                    log!("Error decompressing preset");
                }
            }
            else {
                log!("Error base64 decoding preset");
            }
        }
    }
    
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}