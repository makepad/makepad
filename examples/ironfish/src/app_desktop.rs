use crate::makepad_widgets::*;

live_design!{
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    
    import makepad_widgets::label::Label;
    import makepad_widgets::drop_down::DropDown;
    import makepad_widgets::button::Button;
    import makepad_widgets::slider::Slider;
    import makepad_widgets::check_box::CheckBox;
    import makepad_widgets::text_input::TextInput;
    import makepad_widgets::radio_button::RadioButton;
    import makepad_widgets::swipe_list::SwipeList;
    import makepad_widgets::swipe_list::SwipeListEntry;

    import makepad_example_ironfish::sequencer::Sequencer;
    import makepad_audio_widgets::display_audio::DisplayAudio;
    import makepad_audio_widgets::piano::Piano;

    FONT_SIZE_H2 = 9.5
    
    HEIGHT_AUDIOVIZ = 150
    
    SSPACING_0 = 0.0
    SSPACING_1 = 4.0
    SSPACING_2 = (SSPACING_1 * 2)
    SSPACING_3 = (SSPACING_1 * 3)
    SSPACING_4 = (SSPACING_1 * 4)
    
    SPACING_0 = {top: (SSPACING_0), right: (SSPACING_0), bottom: (SSPACING_0), left: (SSPACING_0)}
    SPACING_1 = {top: (SSPACING_1), right: (SSPACING_1), bottom: (SSPACING_1), left: (SSPACING_1)}
    SPACING_2 = {top: (SSPACING_2), right: (SSPACING_2), bottom: (SSPACING_2), left: (SSPACING_2)}
    SPACING_3 = {top: (SSPACING_3), right: (SSPACING_3), bottom: (SSPACING_3), left: (SSPACING_3)}
    SPACING_4 = {top: (SSPACING_4), right: (SSPACING_4), bottom: (SSPACING_4), left: (SSPACING_4)}
    H2_TEXT_BOLD = {
        font_size: (FONT_SIZE_H2),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }
    H2_TEXT_NORMAL = {
        font_size: (FONT_SIZE_H2),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
    }
    
    COLOR_DOWN_FULL = #000
    
    COLOR_DOWN_0 = #x00000000
    COLOR_DOWN_1 = #x00000011
    COLOR_DOWN_2 = #x00000022
    COLOR_DOWN_3 = #x00000044
    COLOR_DOWN_4 = #x00000066
    COLOR_DOWN_5 = #x000000AA
    COLOR_DOWN_6 = #x000000CC
    
    COLOR_UP_0 = #xFFFFFF00
    COLOR_UP_1 = #xFFFFFF0A
    COLOR_UP_2 = #xFFFFFF10
    COLOR_UP_3 = #xFFFFFF20
    COLOR_UP_4 = #xFFFFFF40
    COLOR_UP_5 = #xFFFFFF66
    COLOR_UP_6 = #xFFFFFFCC
    COLOR_UP_FULL = #xFFFFFFFF
    
    COLOR_ALERT = #xFF0000FF
    COLOR_OSC = #xFFFF99FF
    COLOR_ENV = #xF9A894
    COLOR_FILTER = #x88FF88
    COLOR_FX = #x99EEFF
    COLOR_DEFAULT = (COLOR_UP_6)
    
    COLOR_VIZ_1 = (COLOR_DOWN_2)
    COLOR_VIZ_2 = (COLOR_DOWN_6)
    COLOR_DIVIDER = (COLOR_DOWN_5)
    
    // HELPERS
    FillerH = <Frame> {
        walk: {width: Fill}
    }
    
    FillerV = <Frame> {
        walk: {height: Fill}
    }
    
    Divider = <Frame> {
        walk: {width: Fill, height: Fit, margin: {top: (SSPACING_3), right: 0, bottom: (SSPACING_3), left: (SSPACING_0)}}
        layout: {flow: Down}
        <Box> {
            walk: {width: Fill, height: 1.0}
            draw_bg: {color: (COLOR_DIVIDER)}
        }
        <Box> {
            walk: {width: Fill, height: 1.0}
            draw_bg: {color: (COLOR_UP_4)}
        }
    }
    
    
    // WIDGETS
    ElementBox = <Frame> {
        draw_bg: {color: (COLOR_DOWN_0)}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: <SPACING_1> {}, spacing: (SSPACING_1)}
    }
    
    FishPanelContainer = <CachedFrame> {
        layout: {flow: Down},
        walk: {width: Fill, height: Fit}
    }
    
    SubheaderContainer = <Box> {
        draw_bg: {color: (COLOR_UP_2)}
        walk: {width: Fill, height: Fit, margin: {bottom: (SSPACING_2), top: (SSPACING_2)}}
        layout: {padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_1)}}
    }
    
    FishSubTitle = <Frame> {
        walk: {width: Fit, height: Fit, margin: {top: 1}}
        layout: {padding: {top: (SSPACING_2), right: (SSPACING_1), bottom: (SSPACING_2), left: (SSPACING_1)}}
        
        label = <Label> {
            draw_label: {
                text_style: <H2_TEXT_BOLD> {},
                color: (COLOR_UP_5)
            }
            label: "replace me!"
        }
    }
    
    FishPanel = <GradientY> {
        layout: {flow: Down, padding: <SPACING_2> {}}
        walk: {width: Fill, height: Fit}
        draw_bg: {
            instance border_width: 1.0
            instance border_color: (COLOR_UP_FULL)
            instance inset: vec4(1.0, 1.0, 1.0, 1.0)
            instance radius: 2.5
            instance dither: 1.0
            color: (COLOR_UP_3),
            color2: (COLOR_UP_1)
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
        layout: {padding: {top: (SSPACING_2), right: (SSPACING_4), bottom: (SSPACING_2), left: (SSPACING_2)}}
        
        draw_label: {
            text_style: <H2_TEXT_NORMAL> {},
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
                    padding: {left: (SSPACING_4), top: (SSPACING_2), bottom: (SSPACING_2), right: (SSPACING_4)},
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
                sdf.fill((COLOR_UP_0))
            }
        }
    }
    
    TextButton = <Button> {
        layout: {align: {x: 0.5, y: 0.5}, padding: <SPACING_0> {}}
        walk: {margin: {left: 2.5, right: 2.5}}
        
        draw_label: {
            text_style: <H2_TEXT_BOLD>{}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_ALERT),
                        (COLOR_UP_FULL),
                        self.hover
                    ),
                    (COLOR_DOWN_FULL),
                    self.pressed
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        
    }
    
    FishButton = <Button> {
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: <SPACING_2> {}
        }
        
        draw_label: {
            text_style: <H2_TEXT_BOLD>{}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_UP_5),
                        (COLOR_UP_6),
                        self.hover
                    ),
                    (COLOR_UP_5),
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
                            mix((COLOR_UP_3), (COLOR_DOWN_4), pow(self.pos.y, .2)),
                            mix((COLOR_UP_5), (COLOR_DOWN_4), pow(self.pos.y, 0.25)),
                            self.hover
                        ),
                        mix((COLOR_DOWN_5), (COLOR_UP_3), pow(self.pos.y, 3)),
                        self.pressed
                    ),
                    1.
                );
                sdf.fill(
                    mix(
                        mix(
                            #FFFFFF06,
                            #FFFFFF10,
                            self.hover
                        ),
                        mix((COLOR_DOWN_4), (COLOR_DOWN_4) * 0.1, pow(self.pos.y, 0.3)),
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
        label_text: {text_style:<H2_TEXT_BOLD>{}, color: (COLOR_UP_5)}
        text_input: {
            cursor_margin_bottom: (SSPACING_1),
            cursor_margin_top: (SSPACING_1),
            select_pad_edges: (SSPACING_1),
            cursor_size: (SSPACING_1),
            empty_message: "0",
            numeric_only: true,
            draw_bg: {
                color: (COLOR_DOWN_0)
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
                        mix((COLOR_DOWN_4), (COLOR_DOWN_4) * 0.1, pow(self.pos.y, 1.0)),
                        mix((COLOR_DOWN_4) * 1.75, (COLOR_DOWN_4) * 0.1, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                ) // Control backdrop gradient
                
                sdf.stroke(mix(mix(#x00000060, #x00000070, self.drag), #xFFFFFF10, pow(self.pos.y, 10.0)), 1.0) // Control outline
                let in_side = 5.0;
                let in_top = 5.0; // Ridge: vertical position
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(mix((COLOR_DOWN_4), #00000088, self.drag)); // Ridge color
                let in_top = 7.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(#FFFFFF18); // Ridge: Rim light catcher
                
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);
                
                sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
                sdf.stroke_keep(mix((COLOR_UP_0), self.line_color, self.drag), 1.5)
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
    
    FishToggle = <ElementBox> {
        layout: {padding: <SPACING_0> {}}
        checkbox = <CheckBox> {
            layout: {padding: {top: (SSPACING_0), right: (SSPACING_2), bottom: (SSPACING_0), left: 23}}
            label: "CutOff1"
            state: {
                selected = {
                    default: off
                    off = {
                        from: {all: Forward {duration: 0.1}}
                        apply: {draw_check: {selected: 0.0}}
                    }
                    on = {
                        cursor: Arrow,
                        from: {all: Forward {duration: 0.1}}
                        apply: {draw_check: {selected: 1.0}}
                    }
                }
            }
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
                    
                    sdf.fill(
                        mix(
                            mix((COLOR_DOWN_4), (COLOR_DOWN_4) * 0.1, pow(self.pos.y, 1.0)),
                            mix((COLOR_DOWN_4) * 1.75, (COLOR_DOWN_4) * 0.1, pow(self.pos.y, 1.0)),
                            self.hover
                        )
                    )
                    let isz = sz * 0.65;
                    sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                    sdf.circle(left + sz + self.selected * sz, c.y - 0.5, 0.425 * isz);
                    sdf.subtract();
                    sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                    sdf.blend(self.selected)
                    sdf.fill(mix(#xFFF8, #xFFFC, self.hover));
                    return sdf.result
                }
            }
            draw_label: {
                text_style: <H2_TEXT_BOLD>{},
                color: (COLOR_UP_5)
            }
        }
    }
    
    InstrumentDropdown = <ElementBox> {
        layout: {align: {y: 0.5}, padding: <SPACING_0> {}, flow: Right}
        label = <Label> {
            walk: {width: Fit}
            draw_label: {
                color: (COLOR_UP_5)
                text_style: <H2_TEXT_BOLD>{},
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
        walk: {width: Fit, height: Fit, margin: {bottom: (SSPACING_1)}}
        layout: {padding: <SPACING_2> {}}
        label = <Label> {
            walk: {margin: {top: 1}}
            draw_label: {
                text_style: <H2_TEXT_BOLD>{},
                color: (COLOR_DOWN_6)
            }
            label: "replace me!"
        }
    }
    
    FishHeader = <Box> {
        layout: {flow: Right}
        walk: {height: Fit, width: Fill, margin: {bottom: (SSPACING_2)}}
        title = <FishTitle> {
            walk: {height: Fit, width: Fill, margin: <SPACING_0> {}}
            layout: {padding: <SPACING_2> {}}
        }
        menu = <Frame> {
            layout: {flow: Right}
            walk: {height: Fit, width: Fit}
        }
    }
    
    CheckboxTextual = <CheckBox> {
        draw_check: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                
                return sdf.result
            }
        }
        
        draw_label: {
            text_style: <H2_TEXT_NORMAL>{},
            fn get_color(self) -> vec4 {
                return mix(
                    (COLOR_UP_4),
                    (COLOR_UP_6),
                    self.selected
                )
            }
        }
        
        
        label_walk: {margin: {left: 0.0}}
        
    }
    
    PlayPause = <FishToggle> {
        walk: {width: Fit, height: Fit, margin: <SPACING_3> {}}
        layout: {align: {x: 0.5, y: 0.5}}
        checkbox = {
            walk: {width: 30, height: 30, margin: {right: -20}}
            label: ""
            state: {
                hover = {
                    default: off
                    off = {
                        from: {all: Forward {duration: 0.1}}
                        apply: {
                            draw_check: {hover: 0.0}
                        }
                    }
                    on = {
                        from: {all: Snap}
                        apply: {
                            draw_check: {hover: 1.0}
                        }
                    }
                }
                focus = {
                    default: off
                    off = {
                        from: {all: Forward {duration: 0.0}}
                        apply: {
                            draw_check: {focus: 0.0}
                        }
                    }
                    on = {
                        from: {all: Snap}
                        apply: {
                            draw_check: {focus: 1.0}
                        }
                    }
                }
                selected = {
                    default: off
                    off = {
                        from: {all: Forward {duration: 0.0}}
                        apply: {draw_check: {selected: 0.0}}
                    }
                    on = {
                        cursor: Arrow,
                        from: {all: Forward {duration: 0.0}}
                        apply: {draw_check: {selected: 1.0}}
                    }
                }
            }
            draw_check: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let sz = self.rect_size.x;
                    let c = vec2(self.rect_size.x, self.rect_size.y);
                    let pad = 0.35;
                    sdf.box(
                        0.,
                        0.,
                        self.rect_size.x,
                        self.rect_size.y,
                        4.0
                    )
                    
                    sdf.fill_keep(
                        mix(
                            mix(
                                mix(#xFFFFFF20, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.75), 1.25)),
                                mix(#xFFFFFF40, #xFFFFFF10, pow(length((self.pos - vec2(0.5, 0.5)) * 1.75), 1.25)),
                                self.hover
                            ),
                            mix(
                                mix(#x9C9C64FF, #x00000088, pow(length((self.pos - vec2(0.5, 0.5)) * 1.4), 1.25)),
                                mix(#x9C9C64FF, #x00000088, pow(length((self.pos - vec2(0.5, 0.5)) * 1.75), 1.25)),
                                self.hover
                            ),
                            self.selected
                        )
                    )
                    
                    sdf.stroke_keep(
                        mix(
                            mix((COLOR_UP_5), (COLOR_DOWN_4), pow(self.pos.y, .2)),
                            mix((COLOR_DOWN_5), (COLOR_UP_3), pow(self.pos.y, 3)),
                            self.selected
                        ),
                        1.5
                    );
                                        
                    sdf.subtract()
                    
                    let padx = c.x * pad;
                    let pady = c.y * pad;
                    
                    sdf.move_to(c.x - sz + padx, c.y - sz + pady);
                    sdf.line_to(c.x - padx, c.y * 0.5);
                    sdf.line_to(0.0 + padx, c.y - pady);
                    sdf.close_path();
                    
                    sdf.fill_keep(
                        mix(
                            mix(
                                #fff6,
                                #ffff,
                                self.hover
                            ),
                            #fff,
                            self.selected
                        )
                    )
                    
                    return sdf.result
                }
            }
        }
    }
    
    FishCheckbox = <CheckBox> {
        draw_check: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                
                let left = 1;
                let sz = 6.5;
                
                let c = vec2(left + sz, self.rect_size.y * 0.5 - 2.0);
                
                sdf.box(left, c.y - sz, sz * 2.5, sz * 2.5, 2.0); // 3rd parameter == corner radius
                sdf.fill_keep(mix(
                    mix((COLOR_DOWN_4), (COLOR_DOWN_2), self.pos.y),
                    mix((COLOR_DOWN_5), (COLOR_DOWN_3), self.pos.y),
                    self.selected
                ))
                
                sdf.stroke(
                    mix(
                        mix((COLOR_DOWN_5), (COLOR_UP_3), pow(self.pos.y, 2)),
                        mix((COLOR_DOWN_6), (COLOR_UP_4), pow(self.pos.y, 1.5)),
                        self.hover
                    ),
                    1.0
                ) // outline
                
                let szs = sz * 0.5;
                let dx = 1.0;
                
                let offset = 1.5;
                
                sdf.move_to(left + 4.0 + offset, c.y + offset);
                sdf.line_to(c.x + offset, c.y + szs + offset);
                sdf.line_to(c.x + szs + offset, c.y - szs + offset);
                
                sdf.stroke_keep(mix(
                    mix(
                        #fff0,
                        #fff2,
                        self.hover
                    ),
                    mix(
                        (COLOR_UP_5),
                        (COLOR_UP_6),
                        self.hover
                    ),
                    self.selected
                ), 1.75);
                
                return sdf.result
            }
        }
        
        label_walk: {margin: {left: 23.0}}
        
        draw_label: {
            text_style: <H2_TEXT_BOLD>{},
            fn get_color(self) -> vec4 {
                return (COLOR_UP_5)
            }
        }
    }
    
    FishInput = <TextInput> {
        walk: {width: Fill, height: Fit, margin: 0}
        layout: {
            clip_x: true,
            clip_y: true,
            align: {y: 0.5}
        },
        text: "Search"
        label_walk: {
            margin: 0.0
        }
        draw_bg: {
            instance radius: 3.0
            instance border_width: 0.0
            instance border_color: #3
            instance inset: vec4(0.0, 0.0, 0.0, 0.0)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    self.inset.x + self.border_width,
                    self.inset.y + self.border_width,
                    self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                    self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                
                sdf.fill_keep(mix((COLOR_DOWN_3), (COLOR_DOWN_1), pow(self.pos.y, 0.5)))
                sdf.stroke(mix((COLOR_UP_0), (COLOR_UP_3), pow(self.pos.y, 4.0)), 1.0)
                
                return sdf.result;
            }
        },
        draw_label: {
            text_style:<H2_TEXT_NORMAL>{},
        }
    }
    
    PaginationButton = <Button> {
        label: "1"
        walk: {width: 30, height: 30, margin: {top: 5}}
        layout: {align: {x: 0.5, y: 0.5}, padding: <SPACING_2> {}}
        
        draw_label: {
            text_style: <H2_TEXT_BOLD>{},
            fn get_color(self) -> vec4 {
                return mix(
                    mix((COLOR_UP_4), (COLOR_UP_6), self.hover),
                    (COLOR_UP_4),
                    self.pressed
                )
            }
        }
        
        draw_bg: {
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
                        #x00000000,
                        mix((COLOR_DOWN_5), (COLOR_UP_3), pow(self.pos.y, 4)),
                        self.pressed
                    ),
                    1.0
                )
                
                sdf.fill(mix(
                    (COLOR_UP_0),
                    (COLOR_DOWN_3),
                    self.pressed
                ));
                
                return sdf.result;
            }
        }
    }
    
    PresetFavorite = <CheckBox> {
        draw_check: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                
                let left = 1;
                let sz = self.rect_size.x / 2;
                let c = vec2(sz, sz);
                
                let csz = 4.0;
                sdf.circle(csz, csz, csz);
                sdf.circle(csz * 3, csz, csz);
                sdf.union();
                
                let squeeze = sz * 0.025;
                let top_offset = 0.6;
                // let top_offset = 0.6;
                sdf.move_to(c.x - sz + squeeze + 1.0, c.y - (sz * top_offset));
                sdf.line_to(c.x - sz * 0.5, c.y - (sz * 0.8));
                sdf.line_to(c.x - squeeze, c.y - (sz * top_offset));
                sdf.line_to(c.x * 0.5, c.y);
                sdf.close_path();
                
                sdf.fill_keep(
                    mix(
                        mix(#141414, #444, self.hover),
                        mix(#888, #CCC, self.hover),
                        self.selected
                    )
                )
                
                
                return sdf.result
            }
        }
        draw_label: {
            text_style: <H2_TEXT_BOLD>{},
            color: (COLOR_UP_6)
        }
    }
    
    
    // PANELS
    EnvelopePanel = <Box> {
        layout: {flow: Down, padding: <SPACING_0> {}}
        walk: {width: Fill, height: Fit}
        
        display = <GraphPaper> {}
        
        <Frame> {
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
                            label: "Modulation",
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
        walk: {height: Fit, width: Fill, margin: <SPACING_1> {}}
        layout: {flow: Down, padding: <SPACING_2> {}}
        
        
        <Frame> {
            walk: {height: Fit, width: Fill}
            layout: {flow: Right, spacing: (SSPACING_1), padding: {bottom: (SSPACING_3), top: (SSPACING_2)}}
            
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
                label: "Clear"
                walk: {width: Fit, height: Fit}
            }
            grid_up = <FishButton> {
                label: "↑"
                walk: {width: Fit, height: Fit}
            }
            grid_down = <FishButton> {
                label: "↓"
                walk: {width: Fit, height: Fit}
            }
        }
    }
    
    Arp = <GradientY> {
        layout: {flow: Down, padding: <SPACING_0> {}, spacing: (SSPACING_2)}
        walk: {height: Fit, width: 120, margin: <SPACING_0> {}}
        draw_bg: {color: (COLOR_UP_0), color2: (COLOR_UP_0)}
        
        <Frame> {
            layout: {flow: Right, align: {x: 0.0, y: 0.0}, padding: <SPACING_0> {}}
            walk: {width: Fill, height: Fit, margin: <SPACING_0> {}}
            
            <SubheaderContainer> {
                walk: {margin: <SPACING_0> {}}
                <FishSubTitle> {
                    label = {
                        label: "Arp",
                        draw_label: {color: (COLOR_DEFAULT)},
                    }
                }
                
                <FillerH> {}
                
                arp = <FishToggle> {
                    walk: {margin: <SPACING_0> {}}
                    layout: {padding: <SPACING_0> {}}
                    checkbox = {
                        label: " "
                        layout: {padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_0)}}
                        walk: {margin: <SPACING_0> {}}
                    }
                    walk: {width: Fit, height: Fit, margin: <SPACING_0> {}}
                }
            }
            
            
        }
        
        arpoctaves = <InstrumentBipolarSlider> {
            walk: {width: Fill, margin: <SPACING_0> {}}
            layout: {padding: <SPACING_0> {}}
            slider = {
                draw_slider: {line_color: (COLOR_DEFAULT)}
                min: -4.0
                max: 4.0
                step: 1.0
                precision: 0,
                label: "Octaves"
            }
        }
    }
    
    PianoSettings = <Frame> {
        layout: {flow: Down, padding: <SPACING_0> {}, spacing: (SSPACING_2)}
        walk: {height: Fit, width: 120, margin: <SPACING_0> {}}
        
        <SubheaderContainer> {
            walk: {margin: <SPACING_0> {}}
            <FishSubTitle> {
                label = {
                    label: "Settings",
                    draw_label: {color: (COLOR_DEFAULT)},
                }
            }
        }
        
        porta = <InstrumentSlider> {
            walk: {width: Fill, margin: <SPACING_0> {}}
            layout: {padding: <SPACING_0> {}}
            slider = {
                walk: {width: Fill}
                draw_slider: {line_color: (COLOR_DEFAULT)}
                min: 0.0
                max: 1.0
                label: "Portamento"
            }
        }
    }
    
    SequencerPanel = <Box> {
        layout: {flow: Down}
        walk: {margin: <SPACING_0> {}}
        
        <FishPanel> {
            walk: {width: Fill, height: Fill}
            layout: {flow: Down, spacing: (SSPACING_0), padding: {top: (SSPACING_2)}}
            draw_bg: {color: (COLOR_UP_3), color2: (COLOR_UP_1)}
            
            <FishHeader> {
                title = {
                    walk: {width: Fill}
                    label = {
                        label: "Sequencer",
                    },
                    draw_bg: {color: (COLOR_DEFAULT)}
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
                    layout: {flow: Right, align: {x: 0.0, y: 0.5}, spacing: (SSPACING_4), padding: {top: (SSPACING_1), right: (SSPACING_3), bottom: (SSPACING_0), left: (SSPACING_0)}}
                    
                    playpause = <PlayPause> {}
                    
                    speed = <InstrumentSlider> {
                        walk: {width: Fill}
                        slider = {
                            draw_slider: {line_color: (COLOR_DEFAULT)}
                            min: 0.0
                            max: 240.0
                            label: "BPM"
                        }
                    }
                }
                
                <Divider> {walk: {margin: {top: (SSPACING_2), right: (SSPACING_0), bottom: (SSPACING_0)}}}
                
                sequencer = <Sequencer> {walk: {width: Fill, height: 300, margin: {top: (SSPACING_3)}}}
                
                <Divider> {walk: {margin: {top: (SSPACING_2), right: (SSPACING_0), bottom: (SSPACING_0)}}}
                
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
                walk: {margin: {top: (SSPACING_0)}}
                <FishSubTitle> {
                    label = {
                        label: "Bitcrush",
                        draw_label: {color: (COLOR_FX)},
                    }
                }
                
                <FillerV> {}
                
                crushenable = <FishToggle> {
                    walk: {margin: <SPACING_0> {}}
                    layout: {padding: <SPACING_0> {}}
                    checkbox = {
                        label: " "
                        layout: {padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_0)}}
                        walk: {margin: <SPACING_0> {}}
                    }
                    walk: {width: Fit, height: Fit, margin: {top: (SSPACING_0)}}
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
                    label: "Delay",
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
                    label: "Chorus",
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
                    label: "Reverb",
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
                draw_bg: {color: (COLOR_FILTER)}
                title = {
                    walk: {width: Fit}
                    label = {
                        label: "Filter",
                    },
                }
                
                menu = <Frame> {
                    filter_type = <FishDropDown> {
                        walk: {width: Fill}
                        
                        labels: ["LowPass", "HighPass", "BandPass", "BandReject"]
                        values: [LowPass, HighPass, BandPass, BandReject]
                        
                        draw_label: {
                            text_style: <H2_TEXT_NORMAL>{},
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
                                sdf.fill((COLOR_UP_0))
                            }
                        }
                        
                        popup_menu: {
                            menu_item: {
                                indent_width: 10.0
                                walk: {width: Fill, height: Fit}
                                layout: {
                                    padding: {left: (SSPACING_4), top: (SSPACING_2), bottom: (SSPACING_2), right: (SSPACING_2)},
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
            
            sync = <FishToggle> {checkbox = {label: "LFO Key sync"}}
        }
    }
    
    OscPanel = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down}
        
        <Frame> {
            layout: {flow: Right}
            walk: {width: Fill, height: Fit}
            
            <SubheaderContainer> {
                <FishSubTitle> {label = {label: "Osc", draw_label: {color: (COLOR_OSC)}, walk: {width: Fit}}}
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
                    precision: 0,
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
            layout: {flow: Right, spacing: (SSPACING_1)}
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
        layout: {padding: <SPACING_0> {}, spacing: (SSPACING_0), flow: Down}
        
        <FishPanel> {
            walk: {height: Fill}
            
            <FishHeader> {
                title = {
                    label = {
                        label: "Sound Sources",
                    },
                    draw_bg: {color: (COLOR_OSC)}
                }
            }
            
            <SubheaderContainer> {
                walk: {margin: {top: (SSPACING_0)}}
                <FishSubTitle> {
                    label = {
                        label: "Mixer",
                        draw_label: {color: (COLOR_OSC)},
                    }
                }
            }
            
            <MixerPanel> {walk: {width: Fill, height: Fit}}
            
            <Frame> {
                walk: {width: Fill, height: Fit}
                layout: {flow: Right, spacing: (SSPACING_2)}
                
                osc1 = <OscPanel> {}
                osc2 = <OscPanel> {}
            }
            
            <FillerV> {}
        }
    }
    
    HeaderMenu = <Frame> {
        walk: {width: Fill, height: Fit, margin: {top: -150}}
        layout: {flow: Right, spacing: (SSPACING_0), align: {x: 0.0, y: 0.0}}
        
        <Frame> { // TODO: Remove excessive nesting?
        layout: {flow: Down, align: {x: 0.0, y: 0.0}, spacing: 0, padding: <SPACING_2> {}}
        walk: {height: 135, width: Fill, margin: <SPACING_2> {}}
            
            <Frame> {
                walk: {width: Fill}
                layout: {flow: Right, align: {x: 0.0, y: 0.0}}
                
                <Frame> {
                    layout: {flow: Down, align: {x: 0.0, y: 0.0}}
                    
                    <Label> {
                        walk: {margin: {bottom: (SSPACING_1)}}
                        draw_label: {
                            text_style: <H2_TEXT_BOLD>{},
                            color: (COLOR_UP_5)
                        }
                        label: "Preset"
                    }
                    
                    <Label> {
                        draw_label: {
                            text_style: <H2_TEXT_NORMAL>{font_size: 18},
                            color: (COLOR_UP_6)
                        }
                        label: "Wide Strings"
                    }
                }
                <Frame> {
                    walk: {width: Fill, height: Fit, margin: <SPACING_4> {}}
                    layout: {spacing: (SSPACING_1)}
                }
                
                <Image> {
                    image: dep("crate://self/resources/tinrs.png"),
                    walk: {width: (1000 * 0.175), height: (175 * 0.175), margin: 0}
                    layout: {padding: 0}
                }
            }
            
            <FillerV> {}
            
            <Frame> {
                walk: {width: Fill, height: Fit}
                layout: {spacing: (SSPACING_1)}
                
                panic = <FishButton> {label: "Panic"}
                platformtoggle = <FishButton> {label: "Mobile"}
                presets = <FishButton> {label: "Browse", walk: {width: Fit}}
                <FishButton> {label: "←"}
                <FishButton> {label: "→"}
                <FillerH> {}
                undo = <FishButton> {label: "Undo"}
                redo = <FishButton> {label: "Redo"}
            }
            
        }
        
    }
    
    Play = <FishPanel> {
        layout: {flow: Right, padding: {top: (SSPACING_3)}, spacing: (SSPACING_0)}
        walk: {height: Fit, width: Fill, margin: {top: (SSPACING_0), right: (SSPACING_3), bottom: (SSPACING_3), left: (SSPACING_3)}}
        draw_bg: {color: (COLOR_UP_3), color2: (COLOR_UP_1)}
        
        <Arp> {}
        piano = <Piano> {walk: {height: Fit, width: Fill, margin: {top: (SSPACING_0), right: (SSPACING_2); bottom: (SSPACING_3), left: (SSPACING_2)}}}
        <PianoSettings> {}
    }
    
    Pagination = <Frame> {
        walk: {width: Fill, height: Fit, margin: <SPACING_3> {}}
        layout: {flow: Right, align: {x: 0.5, y: 0.0}, spacing: 0}
        
        <PaginationButton> {label: "<"}
        <PaginationButton> {label: "4"}
        <PaginationButton> {label: "5"}
        <PaginationButton> {label: "6"}
        <PaginationButton> {label: "7"}
        <PaginationButton> {label: "8"}
        <PaginationButton> {label: ">"}
    }
    
    
    // TABS
    FishPanelEnvelopes = <FishPanelContainer> {
        walk: {width: Fill, height: Fill}
        layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}
        
        <FishPanel> {
            walk: {height: Fill}
            
            <FishHeader> {
                title = {
                    label = {
                        label: "Envelopes",
                    },
                    draw_bg: {color: (COLOR_ENV)}
                }
            }
            
            <SubheaderContainer> {
                walk: {margin: {top: (SSPACING_0)}}
                <FishSubTitle> {
                    label = {
                        label: "Volume",
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
        layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}
        
        <FishPanel> {
            
            <FishHeader> {
                title = {
                    label = {
                        label: "Effects",
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

    
    PresetHeader = <Frame> {
        walk: {width: Fill, height: Fit, margin: {top: 0, right: (SSPACING_4), bottom: 0, left: (SSPACING_4)}}
        layout: {flow: Down, spacing: (SSPACING_2), padding: 0}
        
        <SubheaderContainer> {
            <FishSubTitle> {
                walk: {width: Fill}
                label = {
                    label: "Browse",
                    draw_label: {color: (COLOR_UP_6)}
                }
            }
            
            <FillerH> {}
            <CheckboxTextual> {label: "Synth", walk: {width: Fit}}
            <CheckboxTextual> {label: "Seq", walk: {width: Fit}}
            <CheckboxTextual> {label: "Fav", walk: {width: Fit}}
        }
        
        <FishInput> {}
        
    }
        
    PresetListEntry = <SwipeListEntry> {
        layout: {flow: Down, padding: {top: 0, right: 5, bottom: 5, left: 5}}
        walk: {width: Fill, height: Fit}
        
        center: <Frame> {
            layout: {flow: Right, align: {x: 0.0, y: 0.5}}
            walk: {width: Fill, height: Fit}
            
            label = <Button> {
                walk: {width: Fill, height: Fill}
                layout: {align: {x: 0.0, y: 0.5}, padding: {left: 5}}
                draw_label: {
                    fn get_color(self) -> vec4 {
                        return mix(
                            mix((COLOR_UP_5), (COLOR_UP_6), self.hover),
                            (COLOR_UP_4),
                            self.pressed
                        )
                    }
                    text_style: <H2_TEXT_NORMAL>{},
                    color: (COLOR_UP_6)
                }
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        return sdf.result
                    }
                }
                label: "Preset Name"
            }
            
            <Box> {
                walk: {width: Fit, height: Fit}
                
                presetfavorite = <PresetFavorite> {
                    walk: {width: 30, height: 30, margin: {top: 7.5}}
                    label: " "
                }
                
                share = <FishButton> {
                    draw_label: {
                        text_style: <H2_TEXT_BOLD>{},
                        fn get_color(self) -> vec4 {
                            return mix(
                                mix((COLOR_UP_4), (COLOR_UP_6), self.hover),
                                (COLOR_UP_4),
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
                                    #0000,
                                    mix((COLOR_DOWN_5), (COLOR_UP_3), pow(self.pos.y, 3)),
                                    self.pressed
                                ),
                                1.
                            );
                            
                            sdf.fill(
                                mix(
                                    #FFFFFF00,
                                    mix((COLOR_DOWN_4), (COLOR_DOWN_4) * 0.1, pow(self.pos.y, 0.3)),
                                    self.pressed
                                )
                            );
                            
                            return sdf.result
                        }
                    }
                    label: "→"
                    walk: {width: Fit, height: Fit}
                    draw_label: {text_style: {font_size: (FONT_SIZE_H2)}}
                }
                
            }
        }
        
        /*<Divider> {
            walk: {margin: {top: (SSPACING_1), right: (SSPACING_0), bottom: (SSPACING_0)}}
        }*/
    }
    
    PresetList = <SwipeList> {
        Entry = <PresetListEntry>{
        }
    }
    
    PresetSaver = <Frame> {
        walk: {width: Fill, height: Fit, margin: {top: (SSPACING_4), right: (SSPACING_4), bottom: (SSPACING_0), left: (SSPACING_4)}}
        // layout: { flow: Down, spacing: (SSPACING_2) }
        layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}
        
        <FishHeader> {
            title = {
                label = {
                    label: "Presets",
                    draw_label: {
                        color: (COLOR_UP_6)
                    }
                },
                draw_bg: {color: (COLOR_UP_4)}
            }
        }
        
        <SubheaderContainer> {
            walk: {margin: {top: (SSPACING_0)}}
            <FishSubTitle> {
                walk: {width: Fill}
                label = {
                    label: "Save",
                    draw_label: {color: (COLOR_UP_6)}
                }
            }
            
            <FillerV> {}
            
            <CheckboxTextual> {label: "Synth"}
            <CheckboxTextual> {label: "Seq"}
        }
        
        <Frame> {
            walk: {width: Fill, height: Fit}
            layout: {flow: Down, spacing: (SSPACING_2), align: {x: 0.0, y: 0.5}}
            
            presetname = <FishInput> {
                text: "Preset Name"
            }
            
            <Frame> {
                walk: {width: Fill, height: Fit}
                layout: {padding: {top: (SSPACING_0), right: (SSPACING_2), bottom: (SSPACING_0), left: (SSPACING_2)}}
                <Label> {
                    walk: {margin: {right: 2.5}}
                    label: "Overwrite preset?"
                    draw_label: {
                        color: (COLOR_UP_5)
                    }
                }
                <FillerH> {}
                confirm = <TextButton> {label: "Yes"}
                <Label> {
                    label: " · "
                    draw_label: {
                        color: (COLOR_UP_5)
                    }
                }
                cancel = <TextButton> {label: "No"}
            }
            
            save = <FishButton> {label: "Save", walk: {width: Fill}}
        }
    }
    
    Presets = <GradientX> {
        walk: {width: 250, height: Fill}
        layout: {flow: Down, padding: {right: 5}}
        
        draw_bg: {
            instance dither: 1.0
            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
                return mix((COLOR_DOWN_0), (COLOR_DOWN_5), pow(self.pos.x, 17.5) + dither)
            }
            
            fn pixel(self) -> vec4 {
                return Pal::premul(self.get_color())
            }
        }
        
        <PresetSaver> {}
        <PresetHeader> {}
        preset_list = <PresetList> {}
        <Pagination> {}
    }
    
    AppDesktop = <Frame> {
        design_mode: false
        layout:{flow:Right}
        walk: {width: Fill, height: Fill}
        // layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}
        
        <Presets> {}
        
        <Frame> {
            walk: {width: Fill, height: Fill}
            layout: {padding: <SPACING_0> {}, align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}
            // APPLICATION HEADER
            <GradientY> {
                walk: {width: Fill, height: (HEIGHT_AUDIOVIZ)}
                draw_bg: {color: (COLOR_VIZ_1), color2: (COLOR_VIZ_2)}
                display_audio = <DisplayAudio> {
                    walk: {height: Fill, width: Fill}
                }
            }
            
            <HeaderMenu> {}
            
            // CONTROLS
            <Frame> {
                walk: {width: Fill, height: Fill}
                layout: {flow: Right, spacing: (SSPACING_1), padding: <SPACING_3> {}}
                
                <ScrollY> {
                    layout: {flow: Down, spacing: (SSPACING_1)}
                    walk: {height: Fill, width: Fill}
                    oscillators = <FishPanelSoundSources> {}
                }
                
                <ScrollY> {
                    layout: {flow: Down, spacing: (SSPACING_1)}
                    walk: {height: Fill, width: Fill}
                    envelopes = <FishPanelEnvelopes> {}
                    <FishPanelFilter> {}
                }
                
                <ScrollY> {
                    layout: {flow: Down, spacing: (SSPACING_1)}
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
