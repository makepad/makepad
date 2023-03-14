use crate::makepad_widgets::*;

live_design!{
    registry Widget::*;
    
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    
    const SPACING_OS = 40.0;
    const SPACING_CONTROLS = 7.5;
    const SPACING_BASE_PADDING = 6.0;
    
    const COLOR_CLEAR = #x3;
    
    const COLOR_PLAYARROW_INNER = #xFFFDDDFF;
    
    const COLOR_DOWN_OFF = #x00000000;
    const COLOR_DOWN_1 = #x00000022;
    const COLOR_DOWN_2 = #x00000044;
    const COLOR_DOWN_3 = #x00000066;
    const COLOR_DOWN_4 = #x00000088;
    const COLOR_DOWN_5 = #x000000AA;
    const COLOR_DOWN_FULL = #x000000FF;
    
    const COLOR_UP_OFF = #xFFFFFF00;
    const COLOR_UP_2 = #xFFFFFF11;
    const COLOR_UP_3 = #xFFFFFF22;
    const COLOR_UP_4 = #xFFFFFF33;
    const COLOR_UP_5 = #xFFFFFF44;
    const COLOR_UP_6 = #xFFFFFF66;
    const COLOR_UP_7 = #xFFFFFF88;
    const COLOR_UP_8 = #xFFFFFFCC;
    const COLOR_UP_FULL = #xFFFFFFFF;
    
    const GRADIENT_A = #x08221D;
    const GRADIENT_B = #x3F3769;
    
    const FONT_SIZE_H1 = 17.5;
    const FONT_SIZE_H2 = 12.0;
    
    // WIDGETS
    
    Divider = <Frame> {
        walk: {width: Fill, height: Fit, margin: {top: (SPACING_BASE_PADDING), right: (SPACING_BASE_PADDING), bottom: (SPACING_BASE_PADDING), left: (SPACING_BASE_PADDING)}}
        layout: {flow: Down}
        <Box> {
            walk: {width: Fill, height: 1.0}
            draw_bg: {color: (COLOR_DOWN_5)}
        }
        <Box> {
            walk: {width: Fill, height: 1.0}
            draw_bg: {color: (COLOR_UP_5)}
        }
    }
    
    FillerX = <Frame> {
        walk: {width: Fill, height: Fit}
    }
    
    FillerY = <Frame> {
        walk: {width: Fit, height: Fill}
    }
    
    ElementBox = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: {left: (SPACING_CONTROLS), top: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), right: (SPACING_CONTROLS)}, spacing: (SPACING_CONTROLS)}
    }
    
    FishPanelContainer = <Frame> {
        layout: {flow: Down},
        walk: {width: Fill, height: Fit}
    }
    
    FishTab = <RadioButton> {
        walk: {height: Fill, width: Fit}
        layout: {align: {x: 0.0, y: 0.5}}
        draw_radio: {
            radio_type: Tab,
            color_inactive: (COLOR_UP_OFF),
            COLOR_UP_3: (COLOR_UP_OFF)
        }
        draw_label: {
            color_selected: (COLOR_UP_8),
            color_unselected: (COLOR_UP_6),
            color_unselected_hover: (COLOR_UP_8),
            text_style:
            {
                font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"},
                font_size: (FONT_SIZE_H2)
            }
        }
    }
    
    FishDropDown = <DropDown> {
        walk: {width: Fit}
        layout: {padding: {top: (SPACING_BASE_PADDING), right: 18.0, bottom: (SPACING_BASE_PADDING), left: (SPACING_BASE_PADDING)}}
        
        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_UP_6),
                        (COLOR_UP_6),
                        self.focus
                    ),
                    (COLOR_UP_6),
                    self.pressed
                )
            }
        }
        
        popup_menu: {
            menu_item: {
                indent_width: 10.0
                walk: {width: Fit, height: Fit}
                layout: {
                    padding: {left: 15, top: 5, bottom: 5, right: 15},
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
                    mix((COLOR_UP_OFF), (COLOR_UP_OFF), pow(self.pos.y, .25)),
                    1.
                );
                sdf.fill((COLOR_UP_OFF));
            }
        }
    }
    
    FishButton = <Button> {
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: 10
        }
        
        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_UP_6),
                        (COLOR_UP_6),
                        self.hover
                    ),
                    (COLOR_UP_6),
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
                
                sdf.fill(
                    mix(
                        (COLOR_UP_OFF),
                        (COLOR_UP_3),
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
        label: "Change Me"
        label_text: {text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}}, color: (COLOR_UP_6)}
        text_input: {
            cursor_margin_bottom: 3.0,
            cursor_margin_top: 4.0,
            select_pad_edges: 3.0
            cursor_size: 2.0,
            empty_message: "0",
            numeric_only: true,
            draw_bg: {
                shape: None
                color: (COLOR_UP_OFF);
                radius: 2.0
            },
        }
        draw_slider: {
            instance line_color: (COLOR_UP_4)
            instance bipolar: 0.0
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 25.0;
                
                let in_side = 5.0;
                let x_offset = 5.0
                
                sdf.move_to(mix(in_side + x_offset, self.rect_size.x, self.bipolar), top);
                
                sdf.box(in_side, top - 5, self.rect_size.x - 2 - 2 * in_side, 10, 2) // ridge
                sdf.fill(mix((COLOR_DOWN_1), (COLOR_DOWN_2), self.drag)); // ridge color
                
                let fill_x = self.slide_pos * (self.rect_size.x - in_side * 2 - 9) + x_offset;
                sdf.line_to(fill_x + in_side, top);
                
                sdf.stroke_keep(mix((COLOR_UP_OFF), self.line_color, self.drag), 3.0);
                sdf.stroke(mix(self.line_color, (COLOR_UP_4), self.drag), 2.0)
                
                return sdf.result
            }
        }
    }
    
    TextSlider = <ElementBox> {
        slider = <FishSlider> {
            draw_slider: {
                instance line_color: (COLOR_UP_4)
                instance bipolar: 0.0
                fn pixel(self) -> vec4 {
                    let nub_size = 3
                    
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let top = 25.0;
                    
                    let in_side = 5.0;
                    let x_offset = 5.0
                    
                    sdf.move_to(mix(in_side + x_offset, self.rect_size.x, self.bipolar), top);
                    
                    return sdf.result
                }
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
        checkbox = <CheckBox> {
            layout: {padding: {top: (SPACING_CONTROLS), right: (SPACING_CONTROLS), bottom: (SPACING_CONTROLS), left: (SPACING_CONTROLS)}}
            label: "CutOff1"
            draw_check: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let left = 3;
                    let sz = 7.0;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.box(left, c.y - sz, sz * 2.0, sz * 2.2, 1.5); // rounding = 3rd value
                    sdf.fill_keep(mix((COLOR_CONTROL_INSET), (COLOR_CONTROL_INSET) * 0.1, pow(self.pos.y, 1.)))
                    sdf.stroke(mix((COLOR_DOWN_3), (COLOR_UP_FULL), pow(self.pos.y, 3.0)), 1.0) // outline
                    
                    let szs = sz * 0.5;
                    let dx = 1.0;
                    sdf.move_to(left + 4.0, c.y);
                    sdf.line_to(c.x, c.y + szs);
                    sdf.line_to(c.x + szs, c.y - szs);
                    sdf.stroke(mix((COLOR_UP_OFF), (COLOR_UP_6), self.selected), 1.25); // CHECKMARK
                    return sdf.result
                }
            }
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_UP_6)
            }
        }
    }
    
    InstrumentDropdown = <ElementBox> {
        layout: {align: {y: 0.5}, padding: 0, flow: Right}
        label = <Label> {
            walk: {width: Fit}
            draw_label: {
                color: (COLOR_UP_6)
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
            }
        }
        dropdown = <FishDropDown> {
            walk: {margin: {left: (SPACING_CONTROLS), right: (SPACING_CONTROLS)}}
        }
    }
    
    PlayPause = <InstrumentCheckbox> {
        walk: {width: Fit, height: Fit, margin: {top: 10, right: 0, bottom: 0, left: 0}}
        layout: {align: {x: 0.0, y: 0.5}}
        draw_bg: {color: (COLOR_UP_OFF)}
        checkbox = {
            walk: {width: 20, height: 20}
            label: ""
            draw_check: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let left = 3;
                    let sz = 15.0;
                    let c = vec2(left + sz, sz);
                    // let c = vec2(left + sz, self.rect_size.y);
                    
                    sdf.move_to(0.0, 0.0);
                    sdf.line_to(c.x * 0.75, c.y * 0.5);
                    sdf.line_to(0.0, c.y);
                    sdf.close_path();
                    sdf.fill_keep(mix((COLOR_UP_5), (COLOR_UP_FULL), self.selected))
                    sdf.stroke_keep(
                        mix(
                            mix((COLOR_UP_6), (COLOR_UP_2), pow(self.pos.y, 0.5)),
                            (COLOR_UP_7),
                            self.selected
                        ),
                        1.
                    )
                    return sdf.result
                }
            }
        }
    }
    
    SequencerControls = <Box> {
        layout: {flow: Right, padding: 0, spacing: (SPACING_CONTROLS), align: {x: 0.0, y: 0.5}}
        walk: {width: Fill, height: Fit, margin: {top: (SPACING_OS / 2), right: (SPACING_OS), bottom: (SPACING_OS / 2), left: (SPACING_OS)}}
        playpause = <PlayPause> {}
        
        speed = <TextSlider> {
            walk: {width: 110, height: 35}
            draw_bg: {color: (COLOR_UP_OFF)}
            slider = {
                min: 0.0
                max: 240.0
                label: "BPM"
            }
        }
        
        <FillerX> {}
        
        <Box> {
            walk: {width: Fit, height: Fill}
            layout: {flow: Right, clip_x: true, spacing: 0}
            
            rootnote = <InstrumentDropdown> {
                walk: {height: Fill, width: Fit, margin: 0}
                layout: {align: {x: 0.0, y: 0.5}, padding: 0}
                draw_bg: {color: (COLOR_UP_OFF)}
                dropdown = {
                    walk: {margin: 5}
                    labels: ["A", "A#", "B", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#"]
                    values: [A, Asharp, B, C, Csharp, D, Dsharp, E, F, Fsharp, G, Gsharp]
                }
            }
            
            scaletype = <InstrumentDropdown> {
                walk: {height: Fill, width: Fit}
                layout: {align: {x: 0.0, y: 0.5}}
                draw_bg: {color: (COLOR_UP_OFF)}
                dropdown = {
                    walk: {margin: 5}
                    labels: ["Minor", "Major", "Dorian", "Pentatonic"]
                    values: [Minor, Major, Dorian, Pentatonic]
                }
            }
            
        }
        
        <Frame> {
            walk: {width: Fit, height: Fill}
            layout: {align: {x: 0.0, y: 0.5}, spacing: (SPACING_CONTROLS)}
            
            clear_grid = <FishButton> {
                text: "Clear"
                walk: {width: Fit, height: Fit, margin: {top: 2}}
            }
            share = <FishButton> {
                text: "Share"
                walk: {width: Fit, height: Fit, margin: {top: 2}}
            }
            // grid_up = <FishButton> {
            //     text: "↑"
            //     walk: {width: Fit, height: Fit}
            // }
            // grid_down = <FishButton> {
            //     text: "↓"
            //     walk: {width: Fit, height: Fit}
            // }
        }
        
    }
    
    SequencerPanel = <Frame> {
        walk: {width: Fill, height: Fill}
        layout: {flow: Down, spacing: 0.0}
        
        sequencer = <Sequencer> {
            walk: {width: Fill, height: Fill, margin: {top: 10, right: 0, bottom: 10, left: 0}}
            
            button: {
                draw_button: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        sdf.box(1, 1, self.rect_size.x - 5, self.rect_size.y - 5, 2);
                        sdf.glow_keep(
                            mix(
                                (COLOR_UP_OFF),
                                (COLOR_UP_4),
                                self.active
                            ),
                            1.0
                        );
                        sdf.glow_keep(
                            mix(
                                (COLOR_UP_OFF),
                                (COLOR_UP_3),
                                self.active
                            ),
                            1.5
                        );
                        sdf.glow_keep(
                            mix(
                                (COLOR_UP_OFF),
                                (COLOR_UP_2),
                                self.active
                            ),
                            2.0
                        );
                        sdf.fill(mix((COLOR_UP_2), (COLOR_UP_FULL), self.active))
                        return sdf.result
                    }
                    
                }
            }
        }
    }
    
    ModeSequencer = <Frame> {
        visible: true
        walk: {width: Fill, height: Fill, margin: {top: (SPACING_OS / 2), right: (SPACING_OS), bottom: (SPACING_OS / 2), left: (SPACING_OS)}}
        layout: {flow: Down}
        
        <SequencerPanel> {walk: {height: Fill, width: Fill}}
        
        PresetNavigation = <Frame> {
            walk: {width: Fill, height: Fit}
            layout: {flow: Right, align: {x: 0.0, y: 0.5}}
            
            <FishButton> {
                text: "<"
                walk: {width: 40, height: 40}
                draw_label: {text_style: {font_size: (FONT_SIZE_H1)}}
            }
            
            <Frame> {}
            
            <Frame> {
                walk: {width: Fit, height: Fit}
                layout: {flow: Right, align: {x: 0.0, y: 0.5}}
                
                label = <Label> {
                    draw_label: {
                        text_style: {font_size: (FONT_SIZE_H1)},
                        color: (COLOR_UP_6)
                    }
                    text: "Preset Name"
                }
                
                <FishButton> {
                    text: "Edit"
                    walk: {width: Fit, height: Fit}
                    draw_label: {text_style: {font_size: (FONT_SIZE_H2)}}
                }
            }
            
            <Frame> {}
            
            <FishButton> {
                text: ">"
                walk: {width: 40, height: 40}
                draw_label: {text_style: {font_size: (FONT_SIZE_H1)}}
            }
        }
    }
    
    ChordButton = <Button> {
        walk: {width: Fill, height: Fill}
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: 0,
        }
        
        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_UP_6),
                        (COLOR_UP_6),
                        self.hover
                    ),
                    (COLOR_UP_6),
                    self.pressed
                )
            }
        }
        
        draw_bg: {
            instance pressed: 0.0
            instance color_default: 0.0
            instance COLOR_UP_3: 0.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    2.0
                )
                
                sdf.fill(
                    mix(
                        self.color_default,
                        self.COLOR_UP_3,
                        self.pressed
                    )
                );
                
                return sdf.result
            }
            
            color_default: (COLOR_UP_2),
            COLOR_UP_3: (COLOR_UP_3);
        }
        
    }
    
    ChordStrip = <Box> {
        walk: {width: Fill, height: Fill}
        layout: {flow: Right, spacing: (SPACING_CONTROLS / 2), padding: 5}
        draw_bg: {
            color: (COLOR_UP_2),
            radius: 5.0
        }
        
        
        label = <Label> {
            walk: {width: Fill, height: Fill, margin: 10}
            draw_label: {
                text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                color: (COLOR_UP_6)
            }
            text: "change me"
        }
        
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        <ChordButton> {text: ""}
        
    }
    
    ChordPiano = <Frame> {
        layout: {flow: Down, spacing: (SPACING_CONTROLS), padding: {left: (SPACING_OS), right: (SPACING_OS)}}
        walk: {width: Fill, height: Fill}
        
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
        <ChordStrip> {}
    }
    
    ModePlay = <Frame> {
        visible: true,
        layout: {flow: Down, spacing: 20}
        walk: {width: Fill, height: Fill}
        
        <Frame> {
            walk: {width: Fill, height: Fill}
            layout: {flow: Down, align: {x: 0.0, y: 0.0}}
            
            arp = <InstrumentCheckbox> {
                walk: {margin: 0, width: Fit, height: Fit}
                layout: {padding: 0, spacing: 0}
                checkbox = {
                    walk: {margin: 0, height: Fill}
                    layout: {padding: 0, spacing: 0, align: {x: 0.0, y: 0.5}}
                    label: "Arp"
                }
                draw_bg: {color: (COLOR_UP_OFF)}
            }
            
            <ChordPiano> {}
            
        }
        
        <Frame> {
            layout: {flow: Right, padding: {top: (SPACING_OS), right: (SPACING_OS), bottom: (SPACING_OS), left: (SPACING_OS)}, spacing: 10}
            walk: {width: Fill, height: Fit}
            
            <Box> {
                layout: {flow: Down}
                walk: {width: Fill, height: 200}
                draw_bg: {
                    color: COLOR_UP_2,
                    radius: 5.0
                }
                
            }
            
            <Frame> {
                layout: {flow: Down}
                walk: {width: Fill, height: Fit}
                
                crushamount = <FishSlider> {
                    label: "Crush"
                    slider = {
                        min: 0.0
                        max: 1.0
                        label: "Amount"
                        
                    }
                }
                
                chorusmix = <FishSlider> {
                    label: "Chorus"
                    slider = {
                        min: 0.0
                        max: 1.0
                        label: "Mix"
                    }
                }
                
                delaysend = <FishSlider> {
                    label: "Delay",
                    slider = {
                        min: 0.0
                        max: 1.0
                        label: "Delay Send"
                    }
                }
                
                porta = <FishSlider> {
                    label: "Portamento",
                    slider = {
                        min: 0.0
                        max: 1.0
                        label: "Portamento"
                    }
                }
                
            }
        }
        
    }
    
    FishCheckbox = <CheckBox> {
        draw_check: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                
                let left = 1;
                let sz = 8.0;
                
                let c = vec2(left + sz, self.rect_size.y * 0.5 - 2.0);
                
                sdf.box(left, c.y - sz, sz * 2.5, sz * 2.5, 2.0); // 3rd parameter == corner radius
                sdf.fill_keep(mix((COLOR_UP_2), (COLOR_UP_7), self.selected))
                sdf.stroke(#x888, 0.0) // outline
                
                let szs = sz * 0.5;
                let dx = 1.0;
                
                let offset = 1.5;
                
                sdf.move_to(left + 4.0 + offset, c.y + offset);
                sdf.line_to(c.x + offset, c.y + szs + offset);
                sdf.line_to(c.x + szs + offset, c.y - szs + offset);
                
                sdf.stroke_keep(mix(#fff0, (COLOR_DOWN_FULL), self.selected), 1.75);
                
                return sdf.result
            }
        }
        draw_label: {
            text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
            color: (COLOR_UP_6)
        }
    }
    
    PresetListEntry = <Frame> {
        layout: {flow: Down, padding: {top: 5, right: 20, bottom: 5, left: 20}}
        walk: {width: Fill, height: Fit}
        
        <Frame> {
            layout: {flow: Right, align: {x: 0.0, y: 0.5}}
            walk: {width: Fill, height: Fit}
            
            presetselector = <FishCheckbox> {
                label: " "
            }
            
            label = <Button> {
                walk: {width: Fill}
                layout: {align: {x: 0.0, y: 0.5}}
                draw_label: {
                    fn get_color(self) -> vec4 {
                        return mix(
                            (COLOR_UP_6),
                            (COLOR_UP_8),
                            self.pressed
                        )
                    }
                    text_style: {font_size: (FONT_SIZE_H2), font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"}},
                    color: (COLOR_UP_6)
                }
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        return sdf.result
                    }
                }
                text: "Preset Name"
            }
            
            presetfavorite = <CheckBox> {
                label: "Favorite"
            }
            
            share = <FishButton> {
                text: "Share"
                walk: {width: Fit, height: Fit}
                // color: (COLOR_UP_6)
                draw_label: {text_style: {font_size: (FONT_SIZE_H2)}}
            }
        }
        
    }
    
    PaginationButton = <FishButton> {
        text: "1"
        walk: {width: 40, height: 40, margin: {top: 20}}
        draw_label: {text_style: {font_size: (FONT_SIZE_H2)}}
        
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
                
                sdf.fill(mix((COLOR_UP_2), (COLOR_UP_3), self.pressed));
                
                return sdf.result;
            }
        }
    }
    
    Pagination = <Frame> {
        walk: {width: Fill, height: Fit, margin: {bottom: (SPACING_OS)}}
        layout: {flow: Right, align: {x: 0.5, y: 0.0}, spacing: 10}
        
        <PaginationButton> {text: "…"}
        <PaginationButton> {text: "3"}
        <PaginationButton> {text: "4"}
        <PaginationButton> {text: "5"}
        <PaginationButton> {text: "6"}
        <PaginationButton> {text: "7"}
        <PaginationButton> {text: "…"}
    }
    
    PresetList = <Box> {
        
        walk: {width: Fill, height: Fill, margin: {top: (SPACING_OS / 2), right: (SPACING_OS), bottom: (SPACING_OS / 2), left: (SPACING_OS)}}
        layout: {flow: Down, align: {x: 0.5, y: 0.5},}
        
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        <Divider> {}
        <PresetListEntry> {}
        
    }
    
    ModePresetmanager = <Box> {
        visible: true,
        walk: {width: Fill, height: Fill}
        layout: {flow: Down}
        
        <GradientY> {
            draw_bg: {color: (COLOR_DOWN_OFF), color2: (COLOR_DOWN_2)}
            walk: {width: Fill, height: 80}
            layout: {flow: Right}
            
            <Frame> {
                walk: {width: Fill, margin: {top: 0, right: (SPACING_OS), bottom: 0, left: (SPACING_OS)}}
                layout: {align: {x: 0.0, y: 1.0}}
                
                <TextInput> {
                    walk: {width: Fill, height: 50}
                    layout: {align: {x: 0.0, y: 0.5}}
                    text: "Search"
                    draw_label: {
                        text_style:
                        {
                            font: {path: d"crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf"},
                            font_size: (FONT_SIZE_H1)
                        }
                    }
                }
                
                modes = <Frame> {
                    walk: {width: Fit}
                    layout: {spacing: 10, align: {x: 0.0, y: 1.0}}
                    
                    tab1 = <FishTab> {
                        label: "All",
                        walk: {height: Fit, margin: 10}
                        state: {selected = {default: on}},
                        draw_label: {color_selected: (COLOR_UP_8)}
                    }
                    tab2 = <FishTab> {
                        label: "Favorites",
                        walk: {height: Fit, margin: 10}
                        draw_label: {color_selected: (COLOR_UP_8)}
                    }
                }
            }
            
        }
        
        <PresetList> {}
        <Pagination> {}
    }
    
    AppMobile = <Frame> {
        design_mode: false,
        walk: {width: Fill, height: Fill}
        layout: {padding: 0, align: {x: 0.0, y: 0.0}, spacing: 0., flow: Down}
        
        <GradientY> {
            draw_bg: {color: (GRADIENT_A), color2: (GRADIENT_B)}
            layout: {flow: Down, spacing: (SPACING_BASE_PADDING)}
            
            os_header_placeholder = <Box> {
                walk: {width: Fill, height: 50}
                layout: {flow: Right, spacing: (SPACING_BASE_PADDING), padding: 20}
                draw_bg: {color: (COLOR_DOWN_1)}
            }
            
            <SequencerControls> {}
            
            application_pages = <Box> {
                // tab1_frame = <ModeSequencer> {}
                // tab2_frame = <ModePlay> {} // TODO: enable again
                tab3_frame = <ModePresetmanager> {} // TODO: enable again
            }
            
            menu = <Box> {
                walk: {width: Fill, height: 150}
                layout: {flow: Right, spacing: (SPACING_BASE_PADDING), padding: 20}
                draw_bg: {color: (COLOR_DOWN_2)}
                
                modes = <Frame> {
                    tab1 = <FishTab> {
                        walk: {width: Fill}
                        layout: {align: {x: 0.5, y: 0.5}}
                        label: "Sequence",
                        state: {selected = {default: on}},
                        draw_label: {color_selected: (COLOR_UP_8)}
                    }
                    tab2 = <FishTab> {
                        walk: {width: Fill}
                        layout: {align: {x: 0.5, y: 0.5}}
                        label: "Play",
                        draw_label: {color_selected: (COLOR_UP_8)}
                    }
                    tab3 = <FishTab> {
                        walk: {width: Fill}
                        layout: {align: {x: 0.5, y: 0.5}}
                        label: "Presets",
                        draw_label: {color_selected: (COLOR_UP_8)}
                    }
                }
            }
        }
    }
}
/*
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
        // data_to_widget!(db, touch.scale => touch.scale.slider);
        // data_to_widget!(db, touch.scale => touch.scale.slider);
        // data_to_widget!(db, touch.curve => touch.curve.slider);
        // data_to_widget!(db, touch.offset => touch.offset.slider);
        // data_to_widget!(db, filter1.touch_amount => touch.touchamount.slider);
        
        // sequencer
        data_to_widget!(db, sequencer.playing => playpause.checkbox);
        data_to_widget!(db, sequencer.bpm => speed.slider);
        data_to_widget!(db, sequencer.rootnote => rootnote.dropdown);
        data_to_widget!(db, sequencer.scale => scaletype.dropdown);
        data_to_widget!(db, arp.enabled => arp.checkbox);
        
        // Mixer panel
        // data_to_widget!(db, osc_balance => balance.slider);
        // data_to_widget!(db, noise => noise.slider);
        // data_to_widget!(db, sub_osc => sub.slider);
        // data_to_widget!(db, portamento => porta.slider);
        
        // DelayFX Panel
        // data_to_widget!(db, delay.delaysend => delaysend.slider);
        // data_to_widget!(db, delay.delayfeedback => delayfeedback.slider);
        
        // data_to_widget!(db, bitcrush.enable => crushenable.checkbox);
        // data_to_widget!(db, bitcrush.amount => crushamount.slider);
        
        // data_to_widget!(db, delay.difference => delaydifference.slider);
        // data_to_widget!(db, delay.cross => delaycross.slider);
        
        // Chorus panel
        // data_to_widget!(db, chorus.mix => chorusmix.slider);
        // data_to_widget!(db, chorus.mindelay => chorusdelay.slider);
        // data_to_widget!(db, chorus.moddepth => chorusmod.slider);
        // data_to_widget!(db, chorus.rate => chorusrate.slider);
        // data_to_widget!(db, chorus.phasediff => chorusphase.slider);
        // data_to_widget!(db, chorus.feedback => chorusfeedback.slider);
        
        //LFO Panel
        // data_to_widget!(db, lfo.rate => rate.slider);
        // data_to_widget!(db, filter1.lfo_amount => lfoamount.slider);
        // data_to_widget!(db, lfo.synconkey => sync.checkbox);
        
        //Volume Envelope
        // data_to_widget!(db, volume_envelope.a => vol_env.attack.slider);
        // data_to_widget!(db, volume_envelope.h => vol_env.hold.slider);
        // data_to_widget!(db, volume_envelope.d => vol_env.decay.slider);
        // data_to_widget!(db, volume_envelope.s => vol_env.sustain.slider);
        // data_to_widget!(db, volume_envelope.r => vol_env.release.slider);
        
        // //Mod Envelope
        // data_to_widget!(db, mod_envelope.a => mod_env.attack.slider);
        // data_to_widget!(db, mod_envelope.h => mod_env.hold.slider);
        // data_to_widget!(db, mod_envelope.d => mod_env.decay.slider);
        // data_to_widget!(db, mod_envelope.s => mod_env.sustain.slider);
        // data_to_widget!(db, mod_envelope.r => mod_env.release.slider);
        // data_to_widget!(db, filter1.envelope_amount => modamount.slider);
        
        // Filter panel
        // data_to_widget!(db, filter1.filter_type => filter_type.dropdown);
        // data_to_widget!(db, filter1.cutoff => cutoff.slider);
        // data_to_widget!(db, filter1.resonance => resonance.slider);
        
        // Osc1 panel
        // data_to_widget!(db, supersaw1.spread => osc1.supersaw.spread.slider);
        // data_to_widget!(db, supersaw1.diffuse => osc1.supersaw.diffuse.slider);
        // data_to_widget!(db, supersaw1.spread => osc1.supersaw.spread.slider);
        // data_to_widget!(db, supersaw1.diffuse => osc1.supersaw.diffuse.slider);
        // data_to_widget!(db, supersaw1.spread => osc1.hypersaw.spread.slider);
        // data_to_widget!(db, supersaw1.diffuse => osc1.hypersaw.diffuse.slider);
        
        // data_to_widget!(db, osc1.osc_type => osc1.type.dropdown);
        // data_to_widget!(db, osc1.transpose => osc1.transpose.slider);
        // data_to_widget!(db, osc1.detune => osc1.detune.slider);
        // data_to_widget!(db, osc1.harmonic => osc1.harmonicshift.slider);
        // data_to_widget!(db, osc1.harmonicenv => osc1.harmonicenv.slider);
        // data_to_widget!(db, osc1.harmoniclfo => osc1.harmoniclfo.slider);
        
        // // Osc2 panel
        // data_to_widget!(db, supersaw1.spread => osc2.supersaw.spread.slider);
        // data_to_widget!(db, supersaw1.diffuse => osc2.supersaw.diffuse.slider);
        // data_to_widget!(db, supersaw2.spread => osc2.supersaw.spread.slider);
        // data_to_widget!(db, supersaw2.diffuse => osc2.supersaw.diffuse.slider);
        // data_to_widget!(db, supersaw2.spread => osc2.hypersaw.spread.slider);
        // data_to_widget!(db, supersaw2.diffuse => osc2.hypersaw.diffuse.slider);
        
        // data_to_widget!(db, osc2.osc_type => osc2.type.dropdown);
        // data_to_widget!(db, osc2.transpose => osc2.transpose.slider);
        // data_to_widget!(db, osc2.detune => osc2.detune.slider);
        // data_to_widget!(db, osc2.harmonic => osc2.harmonicshift.slider);
        // data_to_widget!(db, osc2.harmonicenv => osc2.harmonicenv.slider);
        // data_to_widget!(db, osc2.harmoniclfo => osc2.harmoniclfo.slider);
        
        // sequencer
        data_to_widget!(db, sequencer.steps => sequencer);
        
        // data_to_apply!(db, osc1.osc_type => osc1.supersaw, visible => | v | v == id!(SuperSaw).to_enum());
        // data_to_apply!(db, osc2.osc_type => osc2.supersaw, visible => | v | v == id!(SuperSaw).to_enum());
        // data_to_apply!(db, osc1.osc_type => osc1.hypersaw, visible => | v | v == id!(HyperSaw).to_enum());
        // data_to_apply!(db, osc2.osc_type => osc2.hypersaw, visible => | v | v == id!(HyperSaw).to_enum());
        // data_to_apply!(db, osc1.osc_type => osc1.harmonic, visible => | v | v == id!(HarmonicSeries).to_enum());
        // data_to_apply!(db, osc2.osc_type => osc2.harmonic, visible => | v | v == id!(HarmonicSeries).to_enum());
        
        // data_to_apply!(db, mod_envelope.a => mod_env.display, draw_bg.attack => | v | v); data_to_apply!(db, mod_envelope.h => mod_env.display, draw_bg.hold => | v | v);
        // data_to_apply!(db, mod_envelope.d => mod_env.display, draw_bg.decay => | v | v);
        // data_to_apply!(db, mod_envelope.s => mod_env.display, draw_bg.sustain => | v | v);
        // data_to_apply!(db, mod_envelope.r => mod_env.display, draw_bg.release => | v | v);
        // data_to_apply!(db, volume_envelope.a => vol_env.display, draw_bg.attack => | v | v);
        // data_to_apply!(db, volume_envelope.h => vol_env.display, draw_bg.hold => | v | v);
        // data_to_apply!(db, volume_envelope.d => vol_env.display, draw_bg.decay => | v | v);
        // data_to_apply!(db, volume_envelope.s => vol_env.display, draw_bg.sustain => | v | v);
        // data_to_apply!(db, volume_envelope.r => vol_env.display, draw_bg.release => | v | v);
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
            println!("MidiPorts:\n{:?}", ports);
            cx.use_midi_inputs(&ports.all_inputs());
        }
        
        if let Event::AudioDevices(devices) = event {
            cx.use_audio_outputs(&devices.default_output());
        }
        
        ui.get_radio_group(&[
            id!(modes.tab1),
            // id!(modes.tab2),
            // id!(modes.tab3),
        ]).selected_to_visible(cx, &ui, &actions, &[
            id!(application_pages.tab1_frame),
            // id!(application_pages.tab2_frame),
            // id!(application_pages.tab3_frame),
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
}*/