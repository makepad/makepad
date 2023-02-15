
pub use makepad_audio_widgets::makepad_widgets;
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_math;
pub use makepad_synth_ironfish;
pub use makepad_synth_ironfish::makepad_audio_graph;

use makepad_widgets::*;
use makepad_draw::*;
use makepad_audio_graph::*;
use makepad_platform::midi::*;

mod sequencer;

use makepad_synth_ironfish::ironfish::*;
use makepad_audio_widgets::piano::*;
use crate::sequencer::*;
use makepad_audio_widgets::display_audio::*;

//use std::fs::File;
//use std::io::prelude::*;
live_design!{
    registry AudioComponent::*;
    registry Widget::*;
    
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    
    const FONT_SIZE_H2 = 9.5

    const HEIGHT_AUDIOVIZ = 150 

    const SSPACING_0 = 0.0
    const SSPACING_1 = 3.0
    const SSPACING_2 = 7.5
    const SSPACING_3 = 12.5

    SPACING_0 = {top: (SSPACING_0), right: (SSPACING_0), bottom: (SSPACING_0), left: (SSPACING_0)}
    SPACING_1 = {top: (SSPACING_1), right: (SSPACING_1), bottom: (SSPACING_1), left: (SSPACING_1)}
    SPACING_2 = {top: (SSPACING_2), right: (SSPACING_2), bottom: (SSPACING_2), left: (SSPACING_2)}
    SPACING_3 = {top: (SSPACING_3), right: (SSPACING_3), bottom: (SSPACING_3), left: (SSPACING_3)}

    const COLOR_HIDDEN_WHITE = #xFFFFFF00
    const COLOR_HIDDEN_BLACK = #x00000000
    
    const COLOR_OSC = #xFFFF99FF
    const COLOR_MUSIC = #xC
    const COLOR_ENV = #xF9A894
    const COLOR_SEQ = #xFFFFFFAA
    const COLOR_FILTER = #x88FF88
    const COLOR_FX = #x99EEFF

    const COLOR_TEXT_H1 = #x000000CC
    const COLOR_TEXT_H2 = #xFFFFFF66

    const COLOR_DIVIDER = #x000000AA
    const COLOR_TEXT_H2_HOVER = #xD
    const COLOR_BEVEL_SHADOW = #x00000066
    const COLOR_BEVEL_HIGHLIGHT = #xFFFFFF44
    const COLOR_CONTROL_INSET = #x00000066
    const COLOR_BG_GRADIENT_BRIGHT = #xFFFFFF20
    const COLOR_BG_GRADIENT_DARK = #xFFFFFF0A

    // HELPERS
    FillerH = <Frame> {
        walk: {width: Fill}
    }

    FillerV = <Frame> {
        walk: {height: Fill}
    }

    Divider = <Frame> {
        walk: {width: Fill, height: Fit, margin: {top: (SSPACING_2 * 2), right: 0, bottom: (SSPACING_2 * 2.5), left: 0}}
        layout: {flow: Down}
        <Box> {
            walk: {width: Fill, height: 1.0}
            draw_bg: {color: (COLOR_DIVIDER)}
        }
        <Box> {
            walk: {width: Fill, height: 1.0}
            draw_bg: {color: (COLOR_BEVEL_HIGHLIGHT)}
        }
    }

    // WIDGETS
    ElementBox = <Frame> {
        draw_bg: {color: (COLOR_HIDDEN_BLACK)}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: <SPACING_1> {}, spacing: (SSPACING_1)}
    }
    
    FishPanelContainer = <CachedFrame> {
        layout: {flow: Down},
        walk: {width: Fill, height: Fit}
    }
    
    SubheaderContainer = <Box> {
        draw_bg: { color: #xFFFFFF10 }
        walk: {width: Fill, height: Fit, margin: {bottom: (SSPACING_2), top: (SSPACING_2 * 2) }}
        layout: {padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_1) }}
    }

    FishPanel = <GradientY> {
        layout: {flow: Down, padding: <SPACING_2> {} }
        walk: {width: Fill, height: Fit}
        draw_bg: {
            instance border_width: 1.0
            instance border_color: #ffff
            instance inset: vec4(1.0, 1.0, 1.0, 1.0)
            instance radius: 2.5
            instance dither: 1.0
            color: (COLOR_BG_GRADIENT_BRIGHT),
            color2: (COLOR_BG_GRADIENT_DARK)
            instance border_color: #x6
            instance border_color2: #x4
            instance border_color3: #x3A

            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
                return mix(self.color, self.color2, self.pos.y + dither)
            }

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    self.inset.x + self.border_width,
                    self.inset.y + self.border_width,
                    self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                    self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(
                        mix(
                            mix(self.border_color, self.border_color2, clamp(self.pos.y * 10, 0, 1)),
                            mix(self.border_color2, self.border_color3, self.pos.y),
                            self.pos.y
                        ),
                        self.border_width
                    )
                }
                return sdf.result;
            }
        }
    }
    
    FishDropDown = <DropDown> {
        walk: {width: Fit}
        layout: {padding: {top: (SSPACING_2), right: 18.0, bottom: (SSPACING_2), left: (SSPACING_2)}}

        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-Text.ttf"}},
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            (#xFFF8),
                            (#xFFF8),
                            self.focus
                        ),
                        (#xFFFF),
                        self.hover
                    ),
                    (#x000A),
                    self.pressed
                )
            }
        }
        
        popup_menu: {
            menu_item: {
                indent_width: 10.0
                walk: {width: Fill, height: Fit}

                layout: {
                    padding: {left: 15, top: 5, bottom: 5, right: 15},
                }

                draw_bg: {
                    color: #x48,
                    color_selected: #x6
                }
            }
        }

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                self.get_bg(sdf);
                // triangle
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                
                sdf.move_to(c.x - sz, c.y - sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.line_to(c.x, c.y + sz * 0.75);
                sdf.close_path();
                
                sdf.fill(mix(#FFFA, #FFFF, self.hover));
                
                return sdf.result
            }

            fn get_bg(self, inout sdf: Sdf2d) {
                sdf.rect(
                    0,
                    0,
                    self.rect_size.x,
                    self.rect_size.y
                )
                sdf.fill((COLOR_HIDDEN_WHITE))
            }
        }
    }
    
    FishButton = <Button> {
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: <SPACING_2> {}
        }
        
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
                            mix(#xFFFFFF66, #x00000066, pow(self.pos.y, .2)),
                            mix((COLOR_BEVEL_HIGHLIGHT), #x00000044, pow(self.pos.y, 0.25)),
                            self.hover
                        ),
                        mix((COLOR_BEVEL_SHADOW), (COLOR_BEVEL_HIGHLIGHT), pow(self.pos.y, 0.75)),
                        self.pressed
                    ),
                    1.
                );
                sdf.fill(
                    mix(
                        mix(
                            #FFFFFF08,
                            #FFFFFF20,
                            self.hover
                        ),
                        mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 0.3)),
                        self.pressed
                    )
                );
                
                return sdf.result
            }
        }
        
    }
    
    FishSlider = <Slider> {
        walk: {
            height: 36,
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
                color: (COLOR_HIDDEN_BLACK)
            },
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
        walk: {margin: {left: -5}}
        layout: {padding: <SPACING_0> {} }
        checkbox = <CheckBox> {
            layout: { padding: { top: (SSPACING_0), right: (SSPACING_2), bottom: (SSPACING_0), left: 23 } }
            label: "CutOff1"
            draw_check: {
                instance border_width: 1.0
                instance border_color: #x06
                instance border_color2: #xFFFFFF0A
                size: 8.5;
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let sz = self.size;
                    let left = sz + 1.;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);

                    sdf.stroke_keep(
                        mix(self.border_color, self.border_color2, clamp(self.pos.y - 0.2, 0, 1)),
                        self.border_width
                    )
                    // sdf.fill(mix(#x0006, #x0008, self.hover));

                    sdf.fill(
                        mix(
                            mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 1.0)),
                            mix((COLOR_CONTROL_INSET) * 1.75, (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 1.0)),
                            self.hover
                        )
                    )
                    let isz = sz * 0.65;
                    sdf.circle(left + sz + self.selected * sz, c.y, isz);
                    sdf.circle(left + sz + self.selected * sz, c.y, 0.425 * isz);
                    sdf.subtract();
                    sdf.circle(left + sz + self.selected * sz, c.y, isz);
                    sdf.blend(self.selected)
                    sdf.fill(mix(#xFFF8, #xFFFC, self.hover));
                    return sdf.result
                }
            }
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_TEXT_H2)
            }
        }
    }
    
    InstrumentDropdown = <ElementBox> {
        layout: {align: {y: 0.5}, padding: <SPACING_0> {}, flow: Right}
        label = <Label> {
            walk: {width: Fit}
            draw_label: {
                color: (COLOR_TEXT_H2)
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
            }
        }
        dropdown = <FishDropDown> {
            walk: {margin: {left: (SSPACING_1), right: (SSPACING_1)}}
        }
    }
    
    GraphPaper = <Box> {
        walk: {width: Fill, height: 120}
        draw_bg: {
            color: #x44,
            instance color2: #x0,
            
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
                return sdf.result
            }
        }
    }
    
    FishTitle = <Box> {
        walk: {width: Fit, height: Fit, margin: {bottom: 5.0}}
        layout: {padding: <SPACING_2> {}}
        label = <Label> {
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_TEXT_H1)
            }
            text: "replace me!"
        }
    }
    
    FishHeader = <Box> {
        layout: {flow: Right }
        walk: {height: Fit, width: Fill, margin: { bottom: (SSPACING_2)} }
        title = <FishTitle> {
            walk: {height: Fit, width: Fill, margin: 0}
            layout: {padding: <SPACING_2> {} }
        }
        menu = <Frame> {
            layout: {flow: Right}
            walk: {height: Fit, width: Fit}
        }
    }
    
    FishSubTitle = <Frame> {
        walk: {width: Fit, height: Fit}
        layout: {padding: <SPACING_2> {}}
        
        label = <Label> {
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_TEXT_H2)
            }
            text: "replace me!"
        }
    }
    
    PlayPause = <InstrumentCheckbox> {
        walk: {width: Fit, height: Fit, margin: 10.0}
        layout: {align: {x: 0.0, y: 0.5}}
        checkbox = {
            walk: {width: 20, height: 20, margin: { right: -30 }}
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
    
    // PANELS
    EnvelopePanel = <Box> {
        layout: {flow: Down, padding: <SPACING_0> {} }
        walk: {width: Fill, height: Fit}
        
        display = <GraphPaper> {}
        
        <Frame> { // TODO: REPLACE WITH DEDICATED WIDGET?
            walk: {width: Fill, height: Fit}
            layout: {flow: Right, spacing: (SSPACING_1)}
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
                    min: 0.0
                    max: 1.0
                    label: "H"
                }
            }
            
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
            
            release = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    label: "R"
                }
            }
            
        }
        
    }
    
    VolumeEnvelopePanel = <Frame> {
        vol_env = <EnvelopePanel> {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
        }
    }
    
    ModEnvelopePanel = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down}
        
        <Frame> {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            <Frame> {
                layout: {flow: Right, align: {x: 0.0, y: 0.0}}
                walk: {width: Fill, height: Fit}
                
                <SubheaderContainer> {
                    <FishSubTitle> {
                        walk: {width: Fill}
                        label = {
                            text: "Modulation",
                            draw_label: {color: (COLOR_ENV)},
                        }
                    }
                }  
                
            }
        }
        
        mod_env = <EnvelopePanel> {
            layout: {flow: Down, padding: <SPACING_0> {}}
            walk: {width: Fill, height: Fit}
        }
        
        modamount = <InstrumentBipolarSlider> {
            walk: {width: Fill}
            slider = {
                draw_slider: {line_color: (COLOR_ENV)}
                min: -1.0
                max: 1.0
                label: "Influence on Cutoff"
            }
        }

    }
    
    SequencerControls = <Frame> {
        walk: {height: Fit, width: Fill, margin: { top: 0, right: 10, bottom: 5, left: 10 }}
        layout: {flow: Down, padding: <SPACING_2> {} }

        
        <Frame> {
            walk: {height: Fit, width: Fill}
            layout: {flow: Right, spacing: (SSPACING_1), padding: {bottom: 10, top: 5}}
            
            rootnote = <InstrumentDropdown> {
                walk: {height: Fit, width: Fit}
                dropdown = {
                    labels: ["A", "A#", "B", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#"]
                    values: [A, Asharp, B, C, Csharp, D, Dsharp, E, F, Fsharp, G, Gsharp]
                }
            }
            
            scaletype = <InstrumentDropdown> {
                walk: {height: Fit, width: Fit}
                dropdown = {
                    labels: ["Minor", "Major", "Dorian", "Pentatonic"]
                    values: [Minor, Major, Dorian, Pentatonic]
                }
            }
            
            <Frame> {
                walk: {width: Fill}
            }
            
            clear_grid = <FishButton> {
                text: "Clear"
                walk: {width: Fit, height: Fit}
            }
            grid_up = <FishButton> {
                text: "↑"
                walk: {width: Fit, height: Fit}
            }
            grid_down = <FishButton> {
                text: "↓"
                walk: {width: Fit, height: Fit}
            }
        }
    }
    
    PianoControls = <GradientY> {
        layout: {flow: Down, padding: <SPACING_0> {}, spacing: (SSPACING_2)}
        walk: {height: Fit, width: 120, margin: 0.0 }
        draw_bg: {color: (COLOR_HIDDEN_WHITE), color2: (COLOR_HIDDEN_WHITE)}

            <Frame> {
                layout: {flow: Right, align: {x: 0.0, y: 0.0}, padding: <SPACING_0> {} }
                walk: {width: Fill, height: Fit, margin: 0.0 }
                
                <SubheaderContainer> {
                        walk: {margin: 0}
                    <FishSubTitle> {
                        label = {
                            text: "Arp",
                            draw_label: {color: (COLOR_MUSIC)},
                        }
                    }

                    <FillerH> {} 

                    arp = <InstrumentCheckbox> {
                        walk: {margin: 0}
                        layout: {padding: <SPACING_0> {} }
                        checkbox = {
                            label: " "
                            layout: { padding: {top: (SSPACING_0), right: 5.0, bottom: (SSPACING_0), left: (SSPACING_0)}}
                            walk: { margin: 0.0 }
                        }
                        walk: {width: Fit, height: Fit, margin: 0}
                    }
                }


            }

            arpoctaves = <InstrumentBipolarSlider> {
                walk: {width: Fill, margin: 0}
                layout: {padding: <SPACING_0> {} }
                slider = {
                    draw_slider: {line_color: (COLOR_MUSIC)}
                    min: -4.0
                    max: 4.0
                    step: 1.0
                    precision:0,
                    label: "Octaves"
                }
            }
    }
    
    SequencerPanel = <Box> {
        layout: {flow: Down}
        walk: {margin: 0}
                    
        <FishPanel> {
            walk: {width: Fill, height: Fill}
            layout: {flow: Down, spacing: 0.0, padding: {top: (SSPACING_2)}}
            draw_bg: {color: (COLOR_BG_GRADIENT_BRIGHT), color2: (COLOR_BG_GRADIENT_DARK)}

            <FishHeader> {
                title = {
                    walk: {width: Fill}
                    label = {
                        text: "Sequencer",
                    },
                    draw_bg: {color: (COLOR_SEQ)}
                }
                menu = {
                    walk: {width: Fit}
                }
            }
            
            <GradientY> {
                walk: {height: Fit}
                layout: {flow: Down}
                draw_bg: {
                    instance border_width: 1.0
                    instance border_color: #ffff
                    instance inset: vec4(1.0, 1.0, 1.0, 1.0)
                    instance radius: 2.5
                    instance dither: 1.0
                    color: (#x00000008),
                    color2: (#x0004)
                    instance border_color: #x1A
                    instance border_color2: #x28
                    instance border_color3: #x50

                    fn get_color(self) -> vec4 {
                        let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
                        return mix(self.color, self.color2, pow(self.pos.y, 0.5) + dither)
                    }

                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        sdf.box(
                            self.inset.x + self.border_width,
                            self.inset.y + self.border_width,
                            self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                            self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                            max(1.0, self.radius)
                        )
                        sdf.fill_keep(self.get_color())
                        if self.border_width > 0.0 {
                            sdf.stroke(
                                mix(
                                    mix(self.border_color, self.border_color2, clamp(self.pos.y * 10, 0, 1)),
                                    mix(self.border_color2, self.border_color3, self.pos.y),
                                    self.pos.y
                                ),
                                self.border_width
                            )
                        }
                        return sdf.result;
                    }
                }

                <Frame> {
                    walk: {height: Fit, width: Fill}
                    layout: {flow: Right, align: {x: 0.0, y: 0.5}, spacing: 15, padding: {top: (SSPACING_2), right: (SSPACING_3), bottom: (SSPACING_0), left: (SSPACING_3) } }
                    
                    playpause = <PlayPause> {}
                    
                    speed = <InstrumentSlider> {
                        walk: {width: Fill}
                        slider = {
                            draw_slider: {line_color: (COLOR_MUSIC)}
                            min: 0.0
                            max: 240.0
                            label: "BPM"
                        }
                    }
                }

                <Divider> { walk: { margin: {top: (SSPACING_2), right: 0, bottom: 0.0 }} }

                sequencer = <Sequencer> {walk: {width: Fill, height: 300, margin: {top: 10}} }

                <Divider> { walk: { margin: {top: (SSPACING_2), right: 0, bottom: 0.0 }} }

                <SequencerControls> {}

            }
        }
    }
    
    CrushFXPanel = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down}
        
        <Frame> {
            layout: {flow: Right, align: {x: 0.0, y: 0.0}}
            walk: {width: Fill, height: Fit}
            
            <SubheaderContainer> {
                walk: {margin: { top: 0 }}
                <FishSubTitle> {
                    label = {
                        text: "Bitcrush",
                        draw_label: {color: (COLOR_FX)},
                    }
                }

                <FillerV> {}

                crushenable = <InstrumentCheckbox> {
                    walk: {margin: 0}
                    layout: {padding: <SPACING_0> {} }
                    checkbox = {
                        label: " "
                        layout: { padding: {top: (SSPACING_0), right: 5.0, bottom: (SSPACING_0), left: (SSPACING_0)}}
                        walk: { margin: 0.0 }
                    }
                    walk: {width: Fit, height: Fit, margin: { top: 0 } }
                }
            }
        }
        
        <Frame> {
            walk: {width: Fill, height: Fit}
            crushamount = <InstrumentSlider> {
                walk: {width: Fill, height: Fit}
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 1.0
                    label: "Amount"
                    
                }
            }
        }
    }
    
    DelayFXPanel = <FishPanelContainer> {
        <SubheaderContainer> {
            <FishSubTitle> {
                label = {
                    text: "Delay",
                    draw_label: {color: (COLOR_FX)},
                }
            }
        }
        <Frame> {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            
            <Frame> {
                layout: {flow: Right, spacing: (SSPACING_1)}
                walk: {width: Fill, height: Fit}
                
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
                layout: {flow: Right, spacing: (SSPACING_1)}
                walk: {width: Fill, height: Fit}
                
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
    
    ChorusFXPanel = <FishPanelContainer> {
        <SubheaderContainer> {
            <FishSubTitle> {
                label = {
                    text: "Chorus",
                    draw_label: {color: (COLOR_FX)},
                }
            }
        }
        <Frame> {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            
            <Frame> {
                layout: {flow: Right, spacing: (SSPACING_1)}
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
                layout: {flow: Right, spacing: (SSPACING_1)}
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
                layout: {flow: Right, spacing: (SSPACING_1)}
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
    
    DelayToyFXPanel = <FishPanelContainer> {
        <SubheaderContainer> {
            <FishSubTitle> {
                label = {
                    text: "Reverb",
                    draw_label: {color: (COLOR_FX)},
                }
            }
        }
        <Frame> {
            layout: {flow: Down}
            walk: {width: Fill, height: Fit}
            
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                
                reverbmix = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        label: "Mix"
                    }
                }
                reverbfeedback = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        label: "Feedback"
                    }
                }
            }
        }
    }
    
    FishPanelFilter = <FishPanelContainer> {
        
        <FishPanel> {
            walk: {height: Fit}

            <FishHeader> {
                draw_bg: { color: (COLOR_FILTER) }
                title = {
                    walk: {width: Fit}
                    label = {
                        text: "Filter",
                    },
                }
                
                menu = <Frame> {
                    walk: { margin: {top: -1, right: -1, bottom: 1} }
                    filter_type = <FishDropDown> {
                        walk: { width: Fill }
                        
                        labels: ["LowPass", "HighPass", "BandPass", "BandReject"]
                        values: [LowPass, HighPass, BandPass, BandReject]

                        draw_label: {
                            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-Text.ttf"}},
                            fn get_color(self) -> vec4 {
                                return mix(
                                    mix(
                                        mix(
                                            (#x0008),
                                            (#x0008),
                                            self.focus
                                        ),
                                        (#x000F),
                                        self.hover
                                    ),
                                    (#x000A),
                                    self.pressed
                                )
                            }
                        }
                        
                        draw_bg: {
                            fn pixel(self) -> vec4 {
                                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                                self.get_bg(sdf);
                                // triangle
                                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                                let sz = 2.5;
                                
                                sdf.move_to(c.x - sz, c.y - sz);
                                sdf.line_to(c.x + sz, c.y - sz);
                                sdf.line_to(c.x, c.y + sz * 0.75);
                                sdf.close_path();
                                
                                sdf.fill(mix(#000A, #000F, self.hover));
                                
                                return sdf.result
                            }

                            fn get_bg(self, inout sdf: Sdf2d) {
                                sdf.rect(
                                    0,
                                    0,
                                    self.rect_size.x,
                                    self.rect_size.y
                                )
                                sdf.fill((COLOR_HIDDEN_WHITE))
                            }
                        }

                        popup_menu: {
                            menu_item: {
                                indent_width: 10.0
                                walk: {width: Fill, height: Fit}
                                layout: {
                                    padding: {left: (SSPACING_2), top: (SSPACING_1), bottom: (SSPACING_1), right: (SSPACING_2)},
                                }
                            }
                        }

                    }
                }
            }

            <Frame> {
                layout: {flow: Right, spacing: (SSPACING_1)}
                walk: {width: Fill, height: Fit}
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
                layout: {flow: Right, spacing: (SSPACING_1)}
                walk: {width: Fill, height: Fit}
                
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
            
            sync = <InstrumentCheckbox> {checkbox = {label: "LFO Key sync"}}
        }
    }
    
    OscPanel = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down}
        
            <Frame> {
                layout: {flow: Right}
                walk: {width: Fill, height: Fit}
                
                <SubheaderContainer> {
                    <FishSubTitle> {label = {text: "Osc", draw_label: {color: (COLOR_OSC)}, walk: {width: Fit}}}
                    type = <InstrumentDropdown> {
                        layout: {flow: Down}
                        dropdown = {
                            walk: {width: Fill, height: Fit}
                            values: [DPWSawPulse, BlampTri, Pure, SuperSaw, HyperSaw, HarmonicSeries]
                            labels: ["Saw", "Triangle", "Sine", "Super Saw", "Hyper Saw", "Harmonic"]
                        }
                    }
                }   
            }
            
            twocol = <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
                transpose = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: -24.0
                        max: 24.0
                        step: 1.0
                        precision:0,
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
        
            <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
                supersaw = <Frame> {
                    layout: {flow: Down}
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
                    layout: {flow: Down}
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
                    harmoniclfo = <InstrumentBipolarSlider> {
                        slider = {
                            draw_slider: {line_color: (COLOR_OSC)}
                            min: -1.0
                            max: 1.0
                            label: "LFO mod"
                        }
                    }
                }
            }
    }
    
    MixerPanel = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down}
        <Frame> {
            layout: { flow: Right, spacing: (SSPACING_1)}
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
            balance = <InstrumentBipolarSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_OSC)}
                    min: 0.0
                    max: 1.0
                    label: "Oscillator Balance"
                }
            }
        }
    }
    
    FishPanelSoundSources = <FishPanelContainer> {
        walk: {width: Fill, height: Fill}
        
        <FishPanel> {
            walk: {width: Fill, height: Fill }
            layout: { flow: Down, spacing: (SSPACING_1) }

            <FishHeader> {
                title = {
                    walk: {width: Fill}
                    label = {
                        text: "Sound Sources",
                    },
                    draw_bg: {color: (COLOR_OSC)}
                }
            }
        
            <SubheaderContainer> {
                walk: {margin: {top: 0 }}
                <FishSubTitle> {
                    label = {
                        text: "Mixer",
                        draw_label: {color: (COLOR_OSC)},
                    }
                }
            }

            <MixerPanel> {walk: {width: Fill, height: Fit}}
            <Frame> {
                walk: {width: Fill, height: Fit}
                layout: { flow: Right, spacing: 7.5 }

                osc1 = <OscPanel> {}
                osc2 = <OscPanel> {}
            }
        }
    }
    
    // TABS
    FishPanelEnvelopes = <FishPanelContainer> {
        walk: {width: Fill, height: Fill}
        layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: 0., flow: Down}
        
        <FishPanel> {
            walk: {height: Fill}

            <FishHeader> {
                title = {
                    label = {
                        text: "Envelopes",
                    },
                    draw_bg: {color: (COLOR_ENV)}
                }
            }

            <SubheaderContainer> {
                walk: {margin: {top: 0 }}
                <FishSubTitle> {
                    label = {
                        text: "Volume",
                        draw_label: {color: (COLOR_ENV)},
                    }
                }
            }
            
            <VolumeEnvelopePanel> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
            }
            
            <ModEnvelopePanel> {
                layout: {flow: Down, clip_y: true}
                walk: {width: Fill, height: Fit}
            }
        }
    }
    
    FishPanelEffects = <FishPanelContainer> {
        walk: {width: Fill, height: Fill}
        layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: 0., flow: Down}
        
        <FishPanel> {
            
            <FishHeader> {
                title = {
                    label = {
                        text: "Effects",
                    },
                    draw_bg: {color: (COLOR_FX)}
                }
            }

            walk: {width: Fill, height: Fill}
            <CrushFXPanel> {}
            <ChorusFXPanel> {}
            <DelayToyFXPanel> {}
            <DelayFXPanel> {}
        }
    }
    
    Play = <FishPanel> {
        layout: {flow: Right, padding: {top: (SSPACING_3)}, spacing: 0.0 }
        walk: { height: Fit, width: Fill, margin: {top: 0, right: (SSPACING_2 * 2), bottom: (SSPACING_2 * 2), left: (SSPACING_2 * 2)} }
        draw_bg: {color: (COLOR_BG_GRADIENT_BRIGHT), color2: (COLOR_BG_GRADIENT_DARK)}

        <PianoControls> {}

        piano = <Piano> {
            walk: {height: Fit, width: Fill, margin: {top: 0, right: (SSPACING_2); bottom: (SSPACING_2 * 2), left: (SSPACING_2)} }
        }

        <Frame> {
            layout: {flow: Down, padding: <SPACING_0> {}, spacing: (SSPACING_2)}
            walk: {height: Fit, width: 120, margin: 0.0 }

            <SubheaderContainer> {
                walk: {margin: 0.0 }
                <FishSubTitle> {
                    label = {
                        text: "Settings",
                        draw_label: {color: (COLOR_MUSIC)},
                    }
                }
            }
            
            porta = <InstrumentSlider> {
                walk: { width: Fill, margin: 0.0 }
                layout: { padding: <SPACING_0> {} }
                slider = {
                    walk: { width: Fill } 
                    draw_slider: {line_color: (COLOR_MUSIC)}
                    min: 0.0
                    max: 1.0
                    label: "Portamento"
                }
            }
        }
    }
    
    // APP
    App = {{App}} {
        window: {window: {inner_size: vec2(1280, 1000)}, pass: {clear_color: #2A}}
        
        audio_graph: {
            root: <Mixer> {
                c1 = <Instrument> {
                    <IronFish> {}
                }
            }
        }
        
        ui: {
            design_mode: false
            walk: {width: Fill, height: Fill}
            layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}
            
            // APPLICATION HEADER
            <Frame> {
                walk: {width: Fill, height: Fit}
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
                walk: {width: Fill, height: (HEIGHT_AUDIOVIZ), margin: {top: -10}}
                draw_bg: { color: #0004, color2: #000C }
                display_audio = <DisplayAudio> {
                    walk: {height: Fill, width: Fill}
                }
            }
            
            
            // CONTROLS
            <Frame> {
                walk: {width: Fill, height: Fill}
                layout: {flow: Right, spacing: (SSPACING_2 * 0.5), padding: <SPACING_3> {} }
                
                <ScrollY> {
                    layout: { flow: Down, spacing: (SSPACING_1) }
                    walk: {height: Fill, width: Fill}
                    
                    oscillators = <FishPanelSoundSources> {}
                }
                
                <ScrollY> {
                    layout: {flow: Down, spacing: (SSPACING_2 * 0.5)}
                    walk: {height: Fill, width: Fill}
                    envelopes = <FishPanelEnvelopes> {}
                    <FishPanelFilter> { }
                }

                <ScrollY> {
                    layout: {flow: Down, spacing: (SSPACING_3)}
                    walk: {height: Fill, width: Fill}
                    
                    effects = <FishPanelEffects> {}
                }

                <ScrollY> {
                    layout: {flow: Down}
                    walk: {height: Fill, width: Fill}
                    
                    <SequencerPanel> {walk: {height: Fill, width: Fill}}
                }
            }

            <Play> {}        
            
        }
    }
}
main_app!(App);

#[derive(Live)]
pub struct App {
    ui: FrameRef,
    audio_graph: AudioGraph,
    window: DesktopWindow,
    #[rust] midi_input: MidiInput,
}

impl LiveHook for App {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) -> Option<usize> {
        //_nodes.debug_print(0,100);
        None
    }
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_widgets::live_design(cx);
        makepad_audio_graph::live_design(cx);
        makepad_synth_ironfish::live_design(cx);
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
        data_to_widget!(db, arp.octaves => arpoctaves.slider);
        
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
        
        // Reverb panel
        data_to_widget!(db, reverb.mix => reverbmix.slider);
        data_to_widget!(db, reverb.feedback => reverbfeedback.slider);
        
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
        
        let actions = ui.handle_event(cx, event);
        
        if let Event::Construct = event {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            db.to_widgets(ironfish.settings.live_read());
            ui.get_piano(id!(piano)).set_key_focus(cx);
            self.midi_input = cx.midi_input();
            //self.midi_data = cx.midi_output_create_sender();
        }
        
        if let Event::MidiPorts(ports) = event {
            //println!("{}", ports);
            cx.use_midi_inputs(&ports.all_inputs());
        }
        
        if let Event::AudioDevices(devices) = event {
            //println!("{}", devices);
            cx.use_audio_outputs(&devices.default_output());
        }
        
        // ui.get_radio_group(&[
        //     id!(envelopes.tab1),
        //     id!(envelopes.tab2),
        // ]).selected_to_visible(cx, &ui, &actions, &[
        //     id!(envelopes.tab1_frame),
        //     id!(envelopes.tab2_frame),
        // ]);
        
        ui.get_radio_group(&[
            id!(oscillators.tab1),
            id!(oscillators.tab2),
        ]).selected_to_visible(cx, &ui, &actions, &[
            id!(oscillators.osc1),
            id!(oscillators.osc2),
        ]);
        
        // ui.get_radio_group(&[
        //     id!(effects.tab1),
        //     id!(effects.tab2),
        //     id!(effects.tab3),
        // ]).selected_to_visible(cx, &ui, &actions, &[
        //     id!(effects.tab1_frame),
        //     id!(effects.tab2_frame),
        //     id!(effects.tab3_frame),
        // ]);
        
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
        
        while let Some((_, data)) = self.midi_input.receive() {
            self.audio_graph.send_midi_data(data);
            if let Some(note) = data.decode().on_note() {
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
            cx.midi_reset();
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
        if self.window.begin(cx).is_not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done() {};
        
        self.window.end(cx);
    }
}