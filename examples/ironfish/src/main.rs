pub use makepad_audio_graph::makepad_widgets;
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_math;
pub use makepad_audio_graph;

use makepad_widgets::*;
use makepad_draw::*;
use makepad_audio_graph::*;
use makepad_platform::midi::*;

mod sequencer;
mod ironfish;
mod waveguide;

use crate::ironfish::*;
use crate::piano::*;
use crate::sequencer::*;
use crate::display_audio::*;

//use std::fs::File;
//use std::io::prelude::*;
live_design!{
    registry AudioComponent::*;
    registry Widget::*;
    
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    
    const SPACING_PANELS = 10.0
    const SPACING_CONTROLS = 3.0
    const COLOR_OSC = #xFFFF99FF // yellow
    const COLOR_MIX = #xC // gray
    const COLOR_ENV = #xFF8888 // light red
    // const COLOR_ENV = #xFFC499 // light red
    const COLOR_FILTER = #x88FF88 // green
    // const COLOR_FILTER = #xA7BEF2 // indigo
    const COLOR_LFO = #xFF9999 // red
    const COLOR_TOUCH = #xBBFF99 // light green
    const COLOR_FX = #x99EEFF // light green
    const COLOR_TEXT_H1 = #x000000CC
    const COLOR_TEXT_H2 = #xFFFFFF66
    const COLOR_DIVIDER = #x000000AA
    const COLOR_TEXT_H2_HOVER = #xD
    const COLOR_BEVEL_SHADOW = #x00000066
    const COLOR_BEVEL_HIGHLIGHT = #xFFFFFF22
    const COLOR_CONTROL_OUTSET = #xFFFFFF66
    const COLOR_HIDDEN_WHITE = #xFFFFFF00
    const COLOR_CONTROL_INSET = #x00000066
    const COLOR_CONTROL_INSET_HOVER = #x00000088
    const COLOR_TODO = #xFF1493FF
    const FONT_SIZE_H1 = 11.0
    const FONT_SIZE_H2 = 9.5
    
    
    // WIDGETS
    ElementBox = <Frame> {
        draw_bg: {color: #4}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: {left: (SPACING_CONTROLS), top: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), right: (SPACING_CONTROLS)}, spacing: (SPACING_CONTROLS)}
    }
    
    FishTab = <RadioButton> {
        draw_radio: {
            radio_type: Tab,
            color_inactive: #x00000000,
            color_active: #x00000000
        }
        draw_label: {
            color_selected: (COLOR_TEXT_H2_HOVER),
            color_unselected: (COLOR_TEXT_H2),
            color_unselected_hover: (COLOR_TEXT_H2_HOVER),
            text_style:
            {
                font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"},
                font_size: (FONT_SIZE_H2)
            }
        }
    }
    
    FishTabPanel = <Frame> {
        layout: {flow: Down, clip_y: true, clip_x: true}
        walk: {width: Fill, height: Fit}
        body = <Box> {
            layout: {flow: Down, padding: {top: (SPACING_CONTROLS), left: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
            walk: {width: Fill, height: Fit, margin: {top: -3, left: 0.25}}
            draw_bg: {color: #FFFFFFFF fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let edge = 8.0;
                sdf.move_to(1.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, self.rect_size.y - edge)
                sdf.line_to(self.rect_size.x - edge, self.rect_size.y - 2.0)
                sdf.line_to(1.0, self.rect_size.y - 2.0);
                sdf.close_path();
                sdf.fill_keep(mix(#xFFFFFF40, #xFFFFFF10, pow(self.pos.y, 0.20)));
                return sdf.result
            }}
        }
    }
    
    FishDropDown = <DropDown> {
        walk: {margin: {left: 5.0, right: 0.0, top: 0.0, bottom: 0.0}, width: Fit}
        layout: {padding: {top: 5.0, right: 18.0, bottom: 5.0, left: 5.0}}
        
        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
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
            //draw_bg: {color: (COLOR_TODO)}
            menu_item: {
                indent_width: 10.0
                layout: {
                    padding: {left: 15, top: 5, bottom: 5},
                }
            }
        }
        draw_bg: {
            fn get_bg(self, inout sdf: Sdf2d) {
                sdf.box(
                    1,
                    1,
                    self.rect_size.x - 2,
                    self.rect_size.y - 2,
                    3
                )
                sdf.stroke_keep(
                    mix(
                        mix((COLOR_HIDDEN_WHITE), (COLOR_HIDDEN_WHITE), pow(self.pos.y, .25)),
                        mix((COLOR_BEVEL_HIGHLIGHT), #x00000044, pow(self.pos.y, .25)),
                        self.hover
                    ),
                1.);
                sdf.fill(
                    mix(
                        #FFFFFF00,
                        #FFFFFF10,
                        self.hover
                    )
                );
            }
        }
    }
    
    FishButton = <Button> {
        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}}
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

        draw_bg: {
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
                
                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(#x00000044, #x00000066, pow(self.pos.y, 4.0)),
                            mix((COLOR_BEVEL_HIGHLIGHT), #x00000044, pow(self.pos.y, 0.25)),
                            self.hover
                        ),
                        mix((COLOR_BEVEL_SHADOW), (COLOR_BEVEL_HIGHLIGHT), pow(self.pos.y, 0.75)),
                    self.pressed), 1.
                );
                sdf.fill(
                    mix(
                        mix(
                            #FFFFFF00, 
                            #FFFFFF10, 
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
        label_text: {text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}}, color: (COLOR_TEXT_H2)}
        text_input: {
            cursor_margin_bottom: 3.0,
            cursor_margin_top: 4.0,
            select_pad_edges: 3.0
            cursor_size: 2.0,
            empty_message: "0",
            numeric_only: true,
            draw_bg: {
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
        draw_slider: {
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
            draw_slider: {bipolar: 0.0}
        }
    }
    
    InstrumentBipolarSlider = <ElementBox> {
        slider = <FishSlider> {
            draw_slider: {bipolar: 1.0}
        }
    }
    
    InstrumentCheckbox = <ElementBox> {
        layout: {padding: 0.0}
        checkbox = <CheckBox> {
            layout: {padding: 2.5}
            label: "CutOff1"
            draw_check: {
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
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_TEXT_H2)
            }
            walk: {margin: {top: 3, right: 0, bottom: 5, left: 0}}
            label_walk: {
                margin: {left: 23.0, top: 0, bottom: 0, right: 0}
            }
        }
    }
    
    InstrumentDropdown = <ElementBox> {
        layout: {align: {y: 0.5}, padding: 0, flow: Right}
        walk: {margin: 0.0}
        label = <Label> {
            walk: {width: Fit, margin: {right: 5}},
            draw_label: {
                color: (COLOR_TEXT_H2)
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
            }
        }
        dropdown = <FishDropDown> {}
    }
    
    GraphPaper = <Box> {
        walk: {width: Fill, height: 150, margin: {top: -4.0, right: -4.0, bottom: 0.0, left: -4.0,}}

        draw_bg: {
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

    FishHeader = <Solid> {
        draw_bg: {
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
        walk: {width: Fit, height: Fit}
        layout: {padding: {left: (SPACING_CONTROLS), top: (SPACING_CONTROLS * 2), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS * 2)}}
        label = <Label> {
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_TEXT_H1)
            }
            text: "replace me!"
        }
    }
    
    FishSubHeader = <Solid> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        walk: {width: Fit, height: Fit, margin: 0.0}
        layout: {padding: {top: 5.0, right: 0.0, bottom: 5.0, left: 5.0}}


        label = <Label> {
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_TEXT_H2)
            }
            text: "replace me!"
        }
    }
    
    FishPanel = <Frame> {
        layout: {flow: Down, flow: Down, clip_y: true, clip_x: true}
        walk: {width: Fill, height: Fit}
        label = <FishHeader> {label = {text: "ReplaceMe", walk: {margin: {top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS)}}}}
        body = <Box> {
            layout: {flow: Down, padding: {top: (SPACING_CONTROLS * 3), left: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
            walk: {width: Fill, height: Fit, margin: {top: -3, left: 0.25}}
            draw_bg: {
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
    
    
    
    
    // PANELS   
    EnvelopePanel = <Frame> {
        layout: {flow: Down}
        walk: {width: Fill, height: Fit}
        
        display = <GraphPaper> {}
        
        <Frame> {
            layout: {flow: Right, padding: 0.0}
            walk: {width: Fill, height: Fit, margin: {top: 0.0, right: 5.0, bottom: 0.0, left: 5.0}}
            attack = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    label: "A"
                }
            }
            
            hold = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    walk: {margin: 0.0}
                    min: 0.0
                    max: 1.0
                    label: "H"
                }
            }
        }
        
        <Frame> {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit, margin: {top: 0.0, right: 5.0, bottom: 0.0, left: 5.0}}
            decay = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    label: "D"
                }
            }
            
            sustain = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    label: "S"
                }
            }
        }
        
        <Frame> {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit, margin: {top: 0.0, right: 5.0, bottom: 0.0, left: 5.0}}
            
            release = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    label: "R"
                }
            }
            
            <Frame> {}
        }
    }
    
    VolumeEnvelopePanel = <FishTabPanel> {
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            draw_bg: {color: #xffffff00}

            
            vol_env = <EnvelopePanel> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
            }
        }
    }
    
    ModEnvelopePanel = <FishTabPanel> {
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            draw_bg: {color: #xffffff00}
            
            mod_env = <EnvelopePanel> {
                layout: {flow: Down, padding: 0.0}
                walk: {width: Fill, height: Fit, margin: 0.0}
            }
            
            modamount = <InstrumentBipolarSlider> {
                walk: {margin: 5.0}
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: -1.0
                    max: 1.0
                    label: "Modulation Cutoff Amount"
                }
            }
        }
    }
    
    SequencerPanel = <FishPanel> {
        label = {draw_bg: {color: (COLOR_MIX)}, label = {text: "Sequencer"}}
        walk: {width: Fill, height: Fill}
        body = {
            walk: {width: Fill, height: Fill}
            <Frame> {
                walk: {height: Fit, margin: 5.0}
                layout: {flow: Right}
                
                playpause = <InstrumentCheckbox> {
                    walk: {width: Fit, height: Fit, margin: 5}
                    layout: {align: {x: 0.0, y: 0.5}}
                    checkbox = {
                        walk: {width: 20, height: 20, margin: 5}
                        label: ""
                        draw_check: {
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
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 240.0
                        label: "BPM"
                    }
                }
                
                rootnote = <InstrumentDropdown> {
                    walk: {height: Fill, width: Fit}
                    layout: {align: {x: 0.0, y: 0.5}}
                    dropdown = {
                        layout: {align: {x: 0.0, y: 0.0}}
                        walk: {width: Fit, height: Fit, margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0.0}}
                        labels: ["A", "A#", "B", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#"]
                        values: [A, Asharp, B, C, Csharp, D, Dsharp, E, F, Fsharp, G, Gsharp]
                    }
                }
                
                scaletype = <InstrumentDropdown> {
                    walk: {height: Fill, width: Fit}
                    layout: {align: {x: 0.0, y: 0.5}}
                    dropdown = {
                        layout: {align: {x: 0.0, y: 0.0}}
                        walk: {width: Fit, height: Fit, margin: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: 0.0}}
                        labels: ["Minor", "Major", "Dorian", "Pentatonic"]
                        values: [Minor, Major, Dorian, Pentatonic]
                    }
                }
                
                arp = <InstrumentCheckbox> {
                    walk: {width: Fit, height: Fill, margin: 5}
                    layout: {align: {x: 0.0, y: 0.5}}
                    checkbox = {
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
    
    CrushFXPanel = <FishTabPanel> {
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            draw_bg: {color: #xffffff00}
            
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit, margin: 5.0}
                
                crushamount = <InstrumentSlider> {
                    walk: {width: Fill, height: Fit}
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        label: "Amount"
                        
                    }
                }
                
                crushenable = <InstrumentCheckbox> {
                    walk: {width: Fit, height: Fit, margin: {top: 15.0, right: 5.0, bottom: 0.0, left: 5.0}}
                    checkbox = {label: "On"}
                }
                
            }
        }
    }
    
    DelayFXPanel = <FishTabPanel> {
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit, margin: 5.0}

                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit, margin: 0.0}
                    delaysend = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Delay Send"
                        }
                    }
                    delayfeedback = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Delay Feedback"
                            
                        }
                    }
                }
                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit, margin: 0.0}
                    delaydifference = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Delay Stereo"
                        }
                    }
                    delaycross = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Delay Cross"
                        }
                    }
                }
            }
        }
    }
    
    ChorusFXPanel = <FishTabPanel> {
        body = {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            draw_bg: {color: #xf00}
            <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit, margin: 5.0}

                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit}
                    
                    chorusmix = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Mix"
                        }
                    }
                    chorusdelay = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Pre"
                        }
                    }
                }
                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit}
                    chorusmod = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Depth"
                        }
                    }
                    chorusrate = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Rate"
                            
                        }
                    }
                }
                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit}
                    chorusphase = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: 0.0
                            max: 1.0
                            label: "Phasing"
                        }
                    }
                    
                    chorusfeedback = <InstrumentBipolarSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FX)}
                            min: -1
                            max: 1
                            label: "Feedback"
                        }
                    }
                }

            }
        }
    }
    
    
    FilterPanel = <Box> {
        draw_bg: { color: #x00000000 } 
        <Frame> {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit}
            <FishHeader> { draw_bg: {color: (COLOR_FILTER)}, label = {text: "Filter"}, layout: {padding: 5.0} }
            filter_type = <InstrumentDropdown> {
                dropdown = {
                    labels: ["LowPass", "HighPass", "BandPass", "BandReject"]
                    values: [LowPass, HighPass, BandPass, BandReject]
                }
            }
        }

        <Box> {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            draw_bg: {color: #FFFFFFFF fn pixel(self) -> vec4 { let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let edge = 8.0;
                sdf.move_to(1.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, self.rect_size.y - edge)
                sdf.line_to(self.rect_size.x - edge, self.rect_size.y - 2.0)
                sdf.line_to(1.0, self.rect_size.y - 2.0);
                sdf.close_path();
                sdf.fill_keep(mix(#xFFFFFF40, #xFFFFFF10, pow(self.pos.y, 0.20)));
                return sdf.result
            }}
            
            <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit, margin: 10.0}

                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit, margin: 0.0}
                    cutoff = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FILTER)}
                            min: 0.0
                            max: 1.0
                            label: "Cutoff"
                        }
                    }
                    
                    resonance = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FILTER)}
                            min: 0.0
                            max: 1.0
                            label: "Resonance"
                        }
                    }
                }
                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit, margin: 0.0}
                    lfoamount = <InstrumentBipolarSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FILTER)}
                            min: -1.0
                            max: 1.0
                            label: "Cutoff LFO Amount"
                        }
                    }
                    rate = <InstrumentSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_FILTER)}
                            max: 1.0
                            label: "Cutoff LFO Rate"
                        }
                    }
                }
                sync = <InstrumentCheckbox> {
                    walk: {height: Fit, margin: 0.0}
                    layout: {flow: Down, spacing: 0.0, align: {x: 0.0, y: 1.0}}
                    checkbox = {
                        label: "LFO Key sync"
                    }
                }
                
            }
        }
    }
        
    Divider = <Frame> {
        walk: {width: Fill, height: 2.0, margin: {top: 5.0, right: 5.0, bottom: 5.0, left: 5.0}}
        layout: {flow: Down}
        <Box> {
            walk: {width: Fill, height: 1.0, margin: 0.0}
            draw_bg: {color: (COLOR_DIVIDER)}
        }
        <Box> {
            walk: {width: Fill, height: 1.0, margin: 0.0}
            draw_bg: {color: (COLOR_BEVEL_HIGHLIGHT)}
        }
    }

    OscPanel = <FishTabPanel> {
        body = {
            draw_bg: {color: #FFFFFF00
                fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }}
            walk: {width: Fill, height: Fit, margin: 0.0}

            <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
                <Frame> {
                    layout: {flow: Right}
                    walk: {width: Fill, height: Fit}


                    <FishSubHeader> {label = {text: "Oscillator", draw_label: {color: (COLOR_OSC)}, walk: {width: Fit}}}
                    type = <InstrumentDropdown> {
                        layout: {flow: Down}
                        dropdown = {
                            walk: {width: Fit, height: Fit}
                            values: [DPWSawPulse, BlampTri, Pure, SuperSaw, HyperSaw, HarmonicSeries]
                            labels: ["Saw", "Triangle", "Sine", "Super Saw", "Hyper Saw", "Harmonic"]
                        }
                    }
                }

                <Frame> {
                    layout: {flow: Down}
                    walk: {width: Fill, height: Fit}
                    supersaw = <Frame> {
                        layout: {flow: Right}
                        walk: {width: Fill, height: Fit}
                        spread = <InstrumentSlider> {
                            slider = {
                                draw_slider: {line_color: (COLOR_OSC)}
                                min: 0.0
                                max: 1.0
                                label: "Spread"
                            }
                        }
                        diffuse = <InstrumentSlider> {
                            slider = {
                                draw_slider: {line_color: (COLOR_OSC)}
                                min: 0.0
                                max: 1.0
                                label: "Diffuse"
                            }
                        }
                    }
                    
                    hypersaw = <Frame> {
                        layout: {flow: Right}
                        walk: {width: Fill, height: Fit}
                        spread = <InstrumentSlider> {
                            slider = {
                                draw_slider: {line_color: (COLOR_OSC)}
                                min: 0.0
                                max: 1.0
                                label: "Spread"
                            }
                        }
                        diffuse = <InstrumentSlider> {
                            slider = {
                                draw_slider: {line_color: (COLOR_OSC)}
                                min: 0.0
                                max: 1.0
                                label: "Diffuse"
                            }
                        }
                    }
                    
                    harmonic = <Frame> {
                        layout: {flow: Down}
                        walk: {width: Fill, height: Fit}
                        <Frame> {
                            layout: {flow: Right}
                            walk: {width: Fill, height: Fit}
                            harmonicshift = <InstrumentSlider> {
                                slider = {
                                    draw_slider: {line_color: (COLOR_OSC)}
                                    min: 0
                                    max: 1.0
                                    label: "Shift"
                                }
                            }
                            harmonicenv = <InstrumentBipolarSlider> {
                                slider = {
                                    draw_slider: {line_color: (COLOR_OSC)}
                                    min: -1.0
                                    max: 1.0
                                    label: "Env mod"
                                }
                            }
                        }
                        <Frame> {
                            layout: {flow: Right}
                            walk: {width: Fill, height: Fit}
                            harmoniclfo = <InstrumentBipolarSlider> {
                                slider = {
                                    draw_slider: {line_color: (COLOR_OSC)}
                                    min: -1.0
                                    max: 1.0
                                    label: "LFO mod"
                                }
                            }
                            <Frame> {}
                        }
                    }
                }
            }

            
            twocol = <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                transpose = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: -24.0
                        max: 24.0
                        label: "Transpose"
                    }
                }
                
                detune = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: -1.0
                        max: 1.0
                        label: "Detune"
                    }
                }
            }
            
        }
    }

    MixerPanel = <FishTabPanel> {
        // label = {draw_bg: {color: (COLOR_MIX)}, label = {text: "Mixer"}}
        body = {
            draw_bg: {color: #FFFFFF00
                fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }}
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                balance = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        label: "Oscillator Balance"
                    }
                }
            }
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                noise = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        label: "Noise"
                    }
                }
                sub = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        label: "Sub"
                    }
                }
            }
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                porta = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        label: "Portamento"
                    }
                }
            }
        }
    }
    
    OscPanels = <Frame> {
        <FishHeader> {label = {text: "Sound Sources", walk: {margin: {top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS)}}}, draw_bg: {color: (COLOR_OSC)}}

        <Box> {
            layout: {flow: Down, padding: {top: (SPACING_CONTROLS), left: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
            walk: {width: Fill, height: Fit, margin: {top: -3, left: 0.25}}
            draw_bg: {color: #FFFFFFFF fn pixel(self) -> vec4 { let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let edge = 8.0;
                sdf.move_to(1.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, 1.0);
                sdf.line_to(self.rect_size.x - 2.0, self.rect_size.y - edge)
                sdf.line_to(self.rect_size.x - edge, self.rect_size.y - 2.0)
                sdf.line_to(1.0, self.rect_size.y - 2.0);
                sdf.close_path();
                sdf.fill_keep(mix(#xFFFFFF40, #xFFFFFF10, pow(self.pos.y, 0.20)));
                return sdf.result
            }}


            osc1 = <OscPanel> {
                body = {
                    layout: {flow: Down, padding: {top: (SPACING_CONTROLS * 3), left: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS)}}
                }
            }
            <Divider> {}
            osc2 = <OscPanel> {}
            <Divider> {}
            <FishSubHeader> {label = {text: "Mixer", draw_label: {color: (COLOR_OSC)}, walk: {margin: {top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS)}}}}
            <MixerPanel> {walk: {width: Fill, height: Fit}}
        }
    }

    
    // TABS
    EnvelopeTabs = <Box> {
        layout: {flow: Down}
        walk: {height: Fit, width: Fill}
        draw_bg: {color: #00000000}
        <Frame> {
            layout: {flow: Right, spacing: 0}
            walk: {height: Fit, width: Fill}
            <FishHeader> {label = {text: "Envelopes", walk: {height: Fit, margin: {top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS)}}}, draw_bg: {color: (COLOR_ENV)}}
            <Box> {
                layout: {flow: Right, spacing: 0}
                walk: {height: Fit, width: Fill, margin: {left: -2, top: -0}}
                draw_bg: {color: #x00000000}
                tab1 = <FishTab> {
                    label: "Modulation",
                    state: {selected = {default: on}},
                    draw_label: {color_selected: (COLOR_ENV)}
                }
                tab2 = <FishTab> {
                    label: "Volume",
                    draw_label: {color_selected: (COLOR_ENV)}
                }
            }
        }
        tab1_frame = <ModEnvelopePanel> {
            visible: true,
            layout: {flow: Down, clip_y: true}
            walk: {width: Fill, height: Fit}
        }
        tab2_frame = <VolumeEnvelopePanel> {
            visible: false
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
        }
    }
    


    
    EffectsTabs = <Box> {
        layout: {flow: Down, spacing: 0}
        walk: {height: Fit, width: Fill}
        draw_bg: {color: #00000000}
        <Frame> {
            layout: {flow: Right, spacing: 0}
            walk: {height: Fit, width: Fill}
            <FishHeader> {label = {text: "Effects", walk: {margin: {top: 0, right: (SPACING_CONTROLS), bottom: 0, left: (SPACING_CONTROLS)}}}, draw_bg: {color: (COLOR_FX)}}
            <Box> {
                layout: {flow: Right, spacing: 0}
                walk: {height: Fit, width: Fit, margin: {left: -2, top: -0}}
                draw_bg: {color: #x00000000}
                tab1 = <FishTab> {label: "Bitcrush", state: {selected = {default: on}}, draw_label: {color_selected: (COLOR_FX)}}
                tab2 = <FishTab> {label: "Chorus", draw_label: {color_selected: (COLOR_FX)}}
                tab3 = <FishTab> {label: "Delay", draw_label: {color_selected: (COLOR_FX)}}
            }
        }
        tab1_frame = <CrushFXPanel> {visible: true}
        tab2_frame = <ChorusFXPanel> {visible: false}
        tab3_frame = <DelayFXPanel> {visible: false}
    }
    
    
    
    // APP
    App = {{App}} {
        window: {window: {inner_size: vec2(1280, 1000)}, pass: {clear_color: #3}}
        
        audio_graph: {
            root: <Mixer> {
                c1 = <Instrument> {
                    <IronFish> {}
                }
            }
        }
        
        ui: {
            design_mode: false,
            draw_bg: {color: #f00},
            walk: {width: Fill, height: Fill}
            layout: {padding: 0, align: {x: 0.0, y: 0.0}, spacing: 0., flow: Down}
            
            // APPLICATION HEADER
            <Frame> {
                walk: {margin: {left: 0, right: 5}, width: Fill, height: Fit}
                layout: {flow: Right, spacing: 0.0, padding: {bottom: -50}, align: {x: 1.0, y: 0.0}}
                
                <Frame> {
                    walk: {width: Fill, height: Fit, margin: {left: 70, top: 10}}
                    layout: {flow: Right, spacing: 2.0}
                    
                    panic = <FishButton> {text: "Panic", walk: {width: Fit}}
                    // save1 = <FishButton> {text: "1"}
                }
                <Image> {image: d"crate://self/resources/tinrs.png", walk: {width: (1000 * 0.25), height: (175 * 0.25)}}
            }
            <GradientY> {
                walk: {width: Fill, height: 150, margin: {top: 0, right: 0, bottom: 0, left: 0}}
                draw_bg: {color: #100A, color2: #0034}
                display_audio = <DisplayAudio> {
                    walk: {height: Fill, width: Fill}
                }
            }
            
            
            piano = <Piano> {
                walk: {width: Fill, height: Fit, margin: {top: 0.0, right: 0.0, bottom: 10.0, left: 0.0}}
            }
            
            // CONTROLS
            <Frame> {
                walk: {width: Fill, height: Fill, margin: {top: (SPACING_PANELS), right: (SPACING_PANELS * 1.5), bottom: (SPACING_PANELS), left: (SPACING_PANELS * 1.5)}}
                layout: {flow: Right, spacing: 0.0}
                
                // COLUMN 1
                <ScrollY> {
                    
                    layout: {flow: Down, clip_y: true, clip_x: true, spacing: (SPACING_PANELS), padding: {top: 0, left: 0, right: 10, bottom: 0}}
                    walk: {height: Fill, width: 350}
                    
                    oscillators = <OscPanels> {layout: {flow: Down}, walk: {width: Fill, height: Fit}}
                    envelopes = <EnvelopeTabs> {layout: {flow: Down}, walk: {width: Fill, height: Fit}}
                    <FilterPanel> {layout: {flow: Down}, walk: {width: Fill, height: Fit}}
                    effects = <EffectsTabs> {}
                }
                
                // COLUMN 2
                <Frame> {
                    layout: {flow: Down, spacing: (SPACING_PANELS)}
                    walk: {height: Fill, width: Fill}
                    <SequencerPanel> {
                        walk: {height: Fill, width: Fill}
                    }
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

impl LiveHook for App {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) -> Option<usize> {
        //_nodes.debug_print(0,100);
        None
    }
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_graph::live_design(cx);
        crate::ironfish::live_design(cx);
        crate::sequencer::live_design(cx);
    }
    
    pub fn data_bind(&mut self, cx: &mut Cx, db: &mut DataBinding, actions: &WidgetActions) {
        let mut db = db.borrow_cx(cx, &self.ui, actions);
        // touch
        data_to_widget!(db, touch.scale => touch.scale.slider);
        data_to_widget!(db, touch.scale => touch.scale.slider);
        data_to_widget!(db, touch.curve => touch.curve.slider);
        data_to_widget!(db, touch.offset => touch.offset.slider);
        data_to_widget!(db, filter1.touch_amount => touch.touchamount.slider);
        
        // sequencer
        data_to_widget!(db, sequencer.playing => playpause.checkbox);
        data_to_widget!(db, sequencer.bpm => speed.slider);
        data_to_widget!(db, sequencer.rootnote => rootnote.dropdown);
        data_to_widget!(db, sequencer.scale => scaletype.dropdown);
        data_to_widget!(db, arp.enabled => arp.checkbox);
        
        // Mixer panel
        data_to_widget!(db, osc_balance => balance.slider);
        data_to_widget!(db, noise => noise.slider);
        data_to_widget!(db, sub_osc => sub.slider);
        data_to_widget!(db, portamento => porta.slider);
        
        // DelayFX Panel
        data_to_widget!(db, delay.delaysend => delaysend.slider);
        data_to_widget!(db, delay.delayfeedback => delayfeedback.slider);
        
        data_to_widget!(db, bitcrush.enable => crushenable.checkbox);
        data_to_widget!(db, bitcrush.amount => crushamount.slider);
        
        data_to_widget!(db, delay.difference => delaydifference.slider);
        data_to_widget!(db, delay.cross => delaycross.slider);
        
        // Chorus panel
        data_to_widget!(db, chorus.mix => chorusmix.slider);
        data_to_widget!(db, chorus.mindelay => chorusdelay.slider);
        data_to_widget!(db, chorus.moddepth => chorusmod.slider);
        data_to_widget!(db, chorus.rate => chorusrate.slider);
        data_to_widget!(db, chorus.phasediff => chorusphase.slider);
        data_to_widget!(db, chorus.feedback => chorusfeedback.slider);
        
        //LFO Panel
        data_to_widget!(db, lfo.rate => rate.slider);
        data_to_widget!(db, filter1.lfo_amount => lfoamount.slider);
        data_to_widget!(db, lfo.synconkey => sync.checkbox);
        
        //Volume Envelope
        data_to_widget!(db, volume_envelope.a => vol_env.attack.slider);
        data_to_widget!(db, volume_envelope.h => vol_env.hold.slider);
        data_to_widget!(db, volume_envelope.d => vol_env.decay.slider);
        data_to_widget!(db, volume_envelope.s => vol_env.sustain.slider);
        data_to_widget!(db, volume_envelope.r => vol_env.release.slider);
        
        //Mod Envelope
        data_to_widget!(db, mod_envelope.a => mod_env.attack.slider);
        data_to_widget!(db, mod_envelope.h => mod_env.hold.slider);
        data_to_widget!(db, mod_envelope.d => mod_env.decay.slider);
        data_to_widget!(db, mod_envelope.s => mod_env.sustain.slider);
        data_to_widget!(db, mod_envelope.r => mod_env.release.slider);
        data_to_widget!(db, filter1.envelope_amount => modamount.slider);
        
        // Filter panel
        data_to_widget!(db, filter1.filter_type => filter_type.dropdown);
        data_to_widget!(db, filter1.cutoff => cutoff.slider);
        data_to_widget!(db, filter1.resonance => resonance.slider);
        
        // Osc1 panel
        data_to_widget!(db, supersaw1.spread => osc1.supersaw.spread.slider);
        data_to_widget!(db, supersaw1.diffuse => osc1.supersaw.diffuse.slider);
        data_to_widget!(db, supersaw1.spread => osc1.supersaw.spread.slider);
        data_to_widget!(db, supersaw1.diffuse => osc1.supersaw.diffuse.slider);
        data_to_widget!(db, supersaw1.spread => osc1.hypersaw.spread.slider);
        data_to_widget!(db, supersaw1.diffuse => osc1.hypersaw.diffuse.slider);
        
        data_to_widget!(db, osc1.osc_type => osc1.type.dropdown);
        data_to_widget!(db, osc1.transpose => osc1.transpose.slider);
        data_to_widget!(db, osc1.detune => osc1.detune.slider);
        data_to_widget!(db, osc1.harmonic => osc1.harmonicshift.slider);
        data_to_widget!(db, osc1.harmonicenv => osc1.harmonicenv.slider);
        data_to_widget!(db, osc1.harmoniclfo => osc1.harmoniclfo.slider);
        
        // Osc2 panel
        data_to_widget!(db, supersaw1.spread => osc2.supersaw.spread.slider);
        data_to_widget!(db, supersaw1.diffuse => osc2.supersaw.diffuse.slider);
        data_to_widget!(db, supersaw2.spread => osc2.supersaw.spread.slider);
        data_to_widget!(db, supersaw2.diffuse => osc2.supersaw.diffuse.slider);
        data_to_widget!(db, supersaw2.spread => osc2.hypersaw.spread.slider);
        data_to_widget!(db, supersaw2.diffuse => osc2.hypersaw.diffuse.slider);
        
        data_to_widget!(db, osc2.osc_type => osc2.type.dropdown);
        data_to_widget!(db, osc2.transpose => osc2.transpose.slider);
        data_to_widget!(db, osc2.detune => osc2.detune.slider);
        data_to_widget!(db, osc2.harmonic => osc2.harmonicshift.slider);
        data_to_widget!(db, osc2.harmonicenv => osc2.harmonicenv.slider);
        data_to_widget!(db, osc2.harmoniclfo => osc2.harmoniclfo.slider);
        
        // sequencer
        data_to_widget!(db, sequencer.steps => sequencer);
        
        data_to_apply!(db, osc1.osc_type => osc1.supersaw, visible => | v | v == id!(SuperSaw).to_enum());
        data_to_apply!(db, osc2.osc_type => osc2.supersaw, visible => | v | v == id!(SuperSaw).to_enum());
        data_to_apply!(db, osc1.osc_type => osc1.hypersaw, visible => | v | v == id!(HyperSaw).to_enum());
        data_to_apply!(db, osc2.osc_type => osc2.hypersaw, visible => | v | v == id!(HyperSaw).to_enum());
        data_to_apply!(db, osc1.osc_type => osc1.harmonic, visible => | v | v == id!(HarmonicSeries).to_enum());
        data_to_apply!(db, osc2.osc_type => osc2.harmonic, visible => | v | v == id!(HarmonicSeries).to_enum());
        
        data_to_apply!(db, mod_envelope.a => mod_env.display, draw_bg.attack => | v | v);
        data_to_apply!(db, mod_envelope.h => mod_env.display, draw_bg.hold => | v | v);
        data_to_apply!(db, mod_envelope.d => mod_env.display, draw_bg.decay => | v | v);
        data_to_apply!(db, mod_envelope.s => mod_env.display, draw_bg.sustain => | v | v);
        data_to_apply!(db, mod_envelope.r => mod_env.display, draw_bg.release => | v | v);
        data_to_apply!(db, volume_envelope.a => vol_env.display, draw_bg.attack => | v | v);
        data_to_apply!(db, volume_envelope.h => vol_env.display, draw_bg.hold => | v | v);
        data_to_apply!(db, volume_envelope.d => vol_env.display, draw_bg.decay => | v | v);
        data_to_apply!(db, volume_envelope.s => vol_env.display, draw_bg.sustain => | v | v);
        data_to_apply!(db, volume_envelope.r => vol_env.display, draw_bg.release => | v | v);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        
        self.window.handle_event(cx, event);
        
        let ui = self.ui.clone();
        let mut db = DataBinding::new();
        
        cx.handle_midi_inputs(event);
        
        let actions = ui.handle_event(cx, event);
        
        if let Event::Construct = event {
            cx.start_midi_input();
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            db.to_widgets(ironfish.settings.live_read());
            ui.get_piano(id!(piano)).set_key_focus(cx);
        }
        
        ui.get_radio_group(&[
            id!(envelopes.tab1),
            id!(envelopes.tab2),
        ]).selected_to_visible(cx, &ui, &actions, &[
            id!(envelopes.tab1_frame),
            id!(envelopes.tab2_frame),
        ]);
        
        ui.get_radio_group(&[
            id!(oscillators.tab1),
            id!(oscillators.tab2),
        ]).selected_to_visible(cx, &ui, &actions, &[
            id!(oscillators.osc1),
            id!(oscillators.osc2),
        ]);
        
        ui.get_radio_group(&[
            id!(effects.tab1),
            id!(effects.tab2),
            id!(effects.tab3),
        ]).selected_to_visible(cx, &ui, &actions, &[
            id!(effects.tab1_frame),
            id!(effects.tab2_frame),
            id!(effects.tab3_frame),
        ]);
        
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
        
        let piano = ui.get_piano(id!(piano));
        
        for inp in cx.handle_midi_received(event) {
            self.audio_graph.send_midi_data(inp.data);
            if let Some(note) = inp.data.decode().on_note() {
                piano.set_note(cx, note.is_on, note.note_number)
            }
        }
        
        for note in piano.notes_played(&actions) {
            self.audio_graph.send_midi_data(MidiNote {
                channel: 0,
                is_on: note.is_on,
                note_number: note.note_number,
                velocity: note.velocity
            }.into());
        }
        
        if ui.get_button(id!(panic)).clicked(&actions) {
            self.audio_graph.all_notes_off();
        }
        
        let sequencer = ui.get_sequencer(id!(sequencer));
        // lets fetch and update the tick.
        
        if ui.get_button(id!(clear_grid)).clicked(&actions) {
            sequencer.clear_grid(cx, &mut db);
        }
        
        if ui.get_button(id!(grid_down)).clicked(&actions) {
            sequencer.grid_down(cx, &mut db);
        }
        
        if ui.get_button(id!(grid_up)).clicked(&actions) {
            sequencer.grid_up(cx, &mut db);
        }
        
        self.data_bind(cx, &mut db, &actions);
        if let Some(nodes) = db.from_widgets() {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            ironfish.settings.apply_over(cx, &nodes);
        }
        
    }
    /*
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
    }*/
    
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}