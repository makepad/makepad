use crate::makepad_widgets::*;
 
live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;

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
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Bold.ttf")}
    }
    H2_TEXT_REGULAR = {
        font_size: (FONT_SIZE_H2),
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
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

    ICO_ARP = dep("crate://self/resources/icons/Icon_Arp.svg")
    ICO_BROWSE = dep("crate://self/resources/icons/Icon_Browse.svg")
    ICO_DOWN = dep("crate://self/resources/icons/Icon_Down.svg")
    ICO_FAV = dep("crate://self/resources/icons/Icon_Favorite.svg")
    ICO_FILTER_BP = dep("crate://self/resources/icons/Icon_Filters_BP.svg")
    ICO_FILTER_BR = dep("crate://self/resources/icons/Icon_Filters_BR.svg")
    ICO_FILTER_HP = dep("crate://self/resources/icons/Icon_Filters_HP.svg")
    ICO_LIVEPLAY = dep("crate://self/resources/icons/Icon_LivePlaying.svg")
    ICO_NEXT = dep("crate://self/resources/icons/Icon_Next.svg")
    ICO_OSC_HARMONIC = dep("crate://self/resources/icons/Icon_OSC_Harmonic.svg")
    ICO_OSC_SAW = dep("crate://self/resources/icons/Icon_OSC_Saw.svg")
    ICO_OSC_SINE = dep("crate://self/resources/icons/Icon_OSC_Sine.svg")
    ICO_OSC_SUPERSAW = dep("crate://self/resources/icons/Icon_OSC_Supersaw.svg")
    ICO_OSC_TRI = dep("crate://self/resources/icons/Icon_OSC_Tri.svg")
    ICO_PANIC = dep("crate://self/resources/icons/Icon_Panic.svg")
    ICO_PLAT_MOBILE = dep("crate://self/resources/icons/Icon_Platform_Mobile.svg")
    ICO_PLAT_DESKTOP = dep("crate://self/resources/icons/Icon_Platform_Desktop.svg")
    ICO_PLAY = dep("crate://self/resources/icons/Icon_Play.svg")
    ICO_PRESET = dep("crate://self/resources/icons/Icon_Presets.svg")
    ICO_PREV = dep("crate://self/resources/icons/Icon_Prev.svg")
    ICO_REDO = dep("crate://self/resources/icons/Icon_Redo.svg")
    ICO_SAVE = dep("crate://self/resources/icons/Icon_Save.svg")
    ICO_SEARCH = dep("crate://self/resources/icons/Icon_Search.svg")
    ICO_SEQ_SWEEP = dep("crate://self/resources/icons/Icon_Seq_Sweep.svg")
    ICO_SEQ = dep("crate://self/resources/icons/Icon_Seq.svg")
    ICO_SETTINGS = dep("crate://self/resources/icons/Icon_Settings.svg")
    ICO_SHARE = dep("crate://self/resources/icons/Icon_Share.svg")
    ICO_UNDO = dep("crate://self/resources/icons/Icon_Undo.svg")
    ICO_UP = dep("crate://self/resources/icons/Icon_Up.svg")


    // HELPERS
    FillerH = <View dx:-768.5 dy:1424.7 dw:98.3 dh:28.3> {
        width: Fill
    }

    FillerV = <View dx:-583.2 dy:1424.9 dw:37.9 dh:73.9> {
        height: Fill
    }

    Divider = <View dx:-927.8 dy:2460.2 dw:375.2 dh:40.4> {
        width: Fill,
        height: Fit,
        margin: {top: (SSPACING_3), right: 0, bottom: (SSPACING_3), left: (SSPACING_0)}
        flow: Down
            <RoundedView> {
            width: Fill,
            height: 1.0
            draw_bg: {color: (COLOR_DIVIDER)}
        }
        <RoundedView> {
            width: Fill,
            height: 1.0
            draw_bg: {color: (COLOR_UP_4)}
        }
    }


    // WIDGETS
    ElementBox = <View dx:-941.0 dy:1557.3 dw:395.8 dh:72.4> {
        draw_bg: {color: (COLOR_DOWN_0)}
        width: Fill,
        height: Fit
        flow: Down,
        padding: <SPACING_1> {}
        spacing: (SSPACING_1)
    }

    FishPanelContainer = <View dx:-933.0 dy:1937.5 dw:389.0 dh:50.0> {
        flow: Down
        width: Fill,
        height: Fit
    }

    SubheaderContainer = <RoundedView dx:-935.6 dy:1842.3 dw:383.0 dh:43.6> {
        draw_bg: {color: (COLOR_UP_2)}
        width: Fill,
        height: Fit,
        margin: {bottom: (SSPACING_2), top: (SSPACING_2)}
        padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_1)}
    }

    FishSubTitle = <View dx:-929.5 dy:2362.2 dw:377.2 dh:45.6> {
        width: Fit,
        height: Fit,
        margin: {top: 1}
        padding: {top: (SSPACING_2), right: (SSPACING_1), bottom: (SSPACING_2), left: (SSPACING_1)}

        label = <Label> {
            draw_text: {
                text_style: <H2_TEXT_BOLD> {},
                color: (COLOR_UP_5)
            }
            text: "replace me!"
        }
    }

    FishPanel = <GradientYView dx:-938.4 dy:1734.8 dw:388.0 dh:41.1> {
        flow: Down,
        padding: <SPACING_2> {}
        width: Fill,
        height: Fit
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

    FishPanelScrollY = <FishPanel dx:-932.0 dy:2052.7 dw:387.7 dh:56.5> {
        height: Fill
        scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
    }

    FishDropDown = <DropDown dx:-924.5 dy:2947.3 dw:378.1 dh:54.0> {
        width: Fit
        padding: {top: (SSPACING_2), right: (SSPACING_4), bottom: (SSPACING_2), left: (SSPACING_2)}

        draw_text: {
            text_style: <H2_TEXT_REGULAR> {},
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
                width: Fill,
                height: Fit
                padding: {left: (SSPACING_4), top: (SSPACING_2), bottom: (SSPACING_2), right: (SSPACING_4)}

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

    IconButton = <Button dx:-923.1 dy:2743.6 dw:372.4 dh:47.3> {
        draw_icon: {
            svg_file: (ICO_SAVE),
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_UP_5),
                        (COLOR_UP_6),
                        self.hover
                    ),
                    (COLOR_UP_4),
                    self.pressed
                )
            }
        }
        icon_walk: {width: 7.5, height: Fit}
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        padding: 9.0
        text: ""
    }

    TextButton = <Button dx:-925.3 dy:2648.3 dw:375.4 dh:44.8> {
        align: {x: 0.5, y: 0.5}
        padding: <SPACING_0> {}
        margin: {left: 2.5, right: 2.5}

        draw_text: {
            text_style: <H2_TEXT_BOLD> {}
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

    FishButton = <Button dx:-925.7 dy:2556.0 dw:375.4 dh:39.3> {

        align: {x: 0.5, y: 0.5}
        padding: <SPACING_2> {}

        draw_text: {
            text_style: <H2_TEXT_BOLD> {}
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

    FishSlider = <Slider dx:387.0 dy:3462.0 dw:398.1 dh:92.6> {
        margin: 0
        height: 36
        text: "CutOff1"
        draw_text: {text_style: <H2_TEXT_BOLD> {}, color: (COLOR_UP_5)}
        text_input: {
            // cursor_margin_bottom: (SSPACING_1),
            // cursor_margin_top: (SSPACING_1),
            // select_pad_edges: (SSPACING_1),
            // cursor_size: (SSPACING_1),
            empty_message: "0",
            is_numeric_only: true,
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

    InstrumentSlider = <ElementBox dx:393.6 dy:3333.7 dw:390.4 dh:79.1> {
        slider = <FishSlider> {
            draw_slider: {bipolar: 0.0}
        }
    }

    InstrumentBipolarSlider = <ElementBox dx:-921.8 dy:3147.7 dw:377.6 dh:79.9> {
        slider = <FishSlider> {
            draw_slider: {bipolar: 1.0}
        }
    }

    FishToggle = <ElementBox dx:-922.8 dy:2842.8 dw:373.3 dh:50.9> {
        padding: <SPACING_0> {}
        checkbox = <CheckBox> {
            width: 40,
            height: 30
            padding: {top: (SSPACING_0), right: (SSPACING_2), bottom: (SSPACING_0), left: 23}
            text: "CutOff1"
            animator: {
                selected = {
                    default: off
                    off = {
                        from: {all: Forward {duration: 0.1}}
                        apply: {draw_check: {selected: 0.0}}
                    }
                    on = {
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
                    //return
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
            
            draw_text: {
                text_style: <H2_TEXT_BOLD> {},
                color: (COLOR_UP_5)
            }
        }
    }

    InstrumentDropdown = <ElementBox dx:-922.7 dy:3053.6 dw:378.6 dh:44.3> {
        align: {y: 0.5}
        padding: <SPACING_0> {},
        flow: Right
        label = <Label> {
            width: Fit
            draw_text: {
                color: (COLOR_UP_5)
                text_style: <H2_TEXT_BOLD> {},
            }
        }
        dropdown = <FishDropDown> {
            margin: {left: (SSPACING_1), right: (SSPACING_1)}
        }
    }

    GraphPaper = <RoundedView dx:395.4 dy:2855.5 dw:390.0 dh:137.4> {
        width: Fill,
        height: 120
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

    FishTitle = <RoundedView dx:-931.7 dy:2267.3 dw:386.0 dh:42.9> {
        width: Fit,
        height: Fit,
        margin: {bottom: (SSPACING_1)}
        padding: <SPACING_2> {}
        label = <Label> {
            margin: {top: 1}
            draw_text: {
                text_style: <H2_TEXT_BOLD> {},
                color: (COLOR_DOWN_6)
            }
            text: "replace me!"
        }
    }

    FishHeader = <RoundedView dx:-931.9 dy:2159.8 dw:384.2 dh:54.2> {
        flow: Right
        height: Fit,
        width: Fill,
        margin: {bottom: (SSPACING_2)}
        title = <FishTitle> {
            height: Fit,
            width: Fill,
            margin: <SPACING_0> {}
            padding: <SPACING_2> {}
        }
        menu = <View> {
            flow: Right
            height: Fit,
            width: Fit
        }
    }

    CheckboxTextual = <CheckBox dx:-922.0 dy:3388.9 dw:377.0 dh:66.2> {
        draw_check: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                return sdf.result
            }
        }

        draw_text: {
            text_style: <H2_TEXT_REGULAR> {},
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

    PlayPause = <FishToggle dx:1496.0 dy:2150.4 dw:77.3 dh:82.8> {
        width: Fit,
        height: Fit,
        margin: <SPACING_3> {}
        align: {x: 0.5, y: 0.5}
        checkbox = {
            width: 30,
            height: 30,
            margin: {right: -20}
            text: ""
            animator: {
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

    FishCheckbox = <CheckBox dx:-920.5 dy:3280.4 dw:376.2 dh:57.8> {
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

        draw_text: {
            text_style: <H2_TEXT_BOLD> {},
            fn get_color(self) -> vec4 {
                return (COLOR_UP_5)
            }
        }
    }

    FishInput = <TextInput dx:-498.3 dy:1605.1 dw:395.4 dh:51.3> {
        width: Fill,
        height: Fit,
        margin: 0

        clip_x: true,
        clip_y: true,
        align: {y: 0.5},
        text: "Search"
        
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
        draw_text: {
            text_style: <H2_TEXT_REGULAR> {},
        }
    }

    PresetFavorite = <CheckBox dx:-499.1 dy:1718.6 dw:397.5 dh:84.5> {
        height: Fit,
        width: Fit,
        margin: 0.0
        padding: 0.0

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
        draw_text: {
            text_style: <H2_TEXT_BOLD> {},
            color: (COLOR_UP_6)
        }
    }


    // PANELS
    EnvelopePanel = <RoundedView dx:384.6 dy:2602.6 dw:392.2 dh:197.0> {
        flow: Down,
        padding: <SPACING_0> {}
        width: Fill,
        height: Fit

        display = <GraphPaper> {}

        <View> {
            width: Fill,
            height: Fit
            flow: Right,
            spacing: (SSPACING_1)
            attack = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    text: "A"
                }
            }

            hold = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    text: "H"
                }
            }

            decay = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    text: "D"
                }
            }

            sustain = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    text: "S"
                }
            }

            release = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_ENV)}
                    min: 0.0
                    max: 1.0
                    text: "R"
                }
            }

        }

    }

    VolumeEnvelopePanel = <View dx:379.4 dy:2323.2 dw:395.0 dh:196.4> {
        vol_env = <EnvelopePanel> {
            flow: Down
            width: Fill,
            height: Fit
        }
    }

    ModEnvelopePanel = <View dx:373.2 dy:1984.5 dw:405.1 dh:299.0> {
        width: Fill,
        height: Fit
        flow: Down

            <View> {
            flow: Down
            width: Fill,
            height: Fit
                <View> {
                flow: Right,
                align: {x: 0.5, y: 0.0}
                width: Fill,
                height: Fit

                    <SubheaderContainer> {
                    <FishSubTitle> {
                        width: Fill
                        label = {
                            text: "Modulation",
                            draw_text: {color: (COLOR_ENV)},
                        }
                    }
                }

            }
        }

        mod_env = <EnvelopePanel> {
            flow: Down,
            padding: <SPACING_0> {}
            width: Fill,
            height: Fit
        }

        modamount = <InstrumentBipolarSlider> {
            width: Fill
            slider = {
                draw_slider: {line_color: (COLOR_ENV)}
                min: -1.0
                max: 1.0
                text: "Influence on Cutoff"
            }
        }

    }

    SequencerControls = <View dx:1201.2 dy:2017.1 dw:375.3 dh:76.3> {
        height: Fit,
        width: Fill,
        margin: <SPACING_1> {}
        flow: Down,
        padding: <SPACING_2> {}

        <View> {
            height: Fit,
            width: Fill
            flow: Right,
            spacing: (SSPACING_1),
            padding: {bottom: (SSPACING_0), top: (SSPACING_0)}
            align: {x: 0.0, y: 0.5}

            rootnote = <InstrumentDropdown> {
                height: Fit,
                width: Fit
                dropdown = {
                    labels: ["A", "A#", "B", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#"]
                    values: [A, Asharp, B, C, Csharp, D, Dsharp, E, F, Fsharp, G, Gsharp]
                }
            }

            scaletype = <InstrumentDropdown> {
                height: Fit,
                width: Fit
                dropdown = {
                    labels: ["Minor", "Major", "Dorian", "Pentatonic"]
                    values: [Minor, Major, Dorian, Pentatonic]
                }
            }

            <View> {
                width: Fill
            }

            clear_grid = <IconButton> {draw_icon: {svg_file: (ICO_SEQ_SWEEP)} icon_walk: {width: 15.0, height: Fit}}
            grid_up = <IconButton> {draw_icon: {svg_file: (ICO_UP)} icon_walk: {width: 10.0, height: Fit}}
            grid_down = <IconButton> {draw_icon: {svg_file: (ICO_DOWN)} icon_walk: {width: 10.0, height: Fit}}
        }
    }

    Arp = <GradientYView dx:-37.0 dy:3928.2 dw:157.2 dh:107.1> {
        flow: Down,
        padding: <SPACING_0> {}
        spacing: (SSPACING_2)
        height: Fit,
        width: 120,
        margin: <SPACING_0> {}
        draw_bg: {color: (COLOR_UP_0), color2: (COLOR_UP_0)}

        <View> {
            flow: Right,
            align: {x: 0.0, y: 0.0} padding: <SPACING_0> {}
            width: Fill,
            height: Fit,
            margin: <SPACING_0> {}

            <SubheaderContainer> {
                margin: <SPACING_0> {}
                <FishSubTitle> {
                    label = {
                        text: "Arp",
                        draw_text: {color: (COLOR_DEFAULT)},
                    }
                }

                <FillerH> {}

                arp = <FishToggle> {
                    margin: <SPACING_0> {}
                    padding: <SPACING_0> {}
                    checkbox = {
                        text: " "
                        padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_0)}
                        margin: <SPACING_0> {}
                    }
                    width: Fit,
                    height: Fit,
                    margin: <SPACING_0> {}
                }
            }


        }

        arpoctaves = <InstrumentBipolarSlider> {
            width: Fill,
            margin: <SPACING_0> {}
            padding: <SPACING_0> {}
            slider = {
                draw_slider: {line_color: (COLOR_DEFAULT)}
                min: -4.0
                max: 4.0
                step: 1.0
                precision: 0,
                text: "Octaves"
            }
        }
    }

    PianoSettings = <View dx:1017.3 dy:3930.0 dw:142.2 dh:109.1> {
        flow: Down,
        padding: <SPACING_0> {} spacing: (SSPACING_2)
        height: Fit,
        width: 120,
        margin: <SPACING_0> {}

        <SubheaderContainer> {
            margin: <SPACING_0> {}
            <FishSubTitle> {
                label = {
                    text: "Settings",
                    draw_text: {color: (COLOR_DEFAULT)},
                }
            }
        }

        porta = <InstrumentSlider> {
            width: Fill,
            margin: <SPACING_0> {}
            padding: <SPACING_0> {}
            slider = {
                width: Fill
                draw_slider: {line_color: (COLOR_DEFAULT)}
                min: 0.0
                max: 1.0
                text: "Portamento"
            }
        }
    }

    SequencerPanel = <RoundedView dx:1182.0 dy:1376.4 dw:400.0 dh:580.1> {
        flow: Down
        margin: <SPACING_0> {}

        <FishPanelScrollY> {
            width: Fill,
            height: Fill
            flow: Down,
            spacing: (SSPACING_0),
            padding: {top: (SSPACING_2)}
            draw_bg: {color: (COLOR_UP_3), color2: (COLOR_UP_1)}

            <FishHeader> {
                title = {
                    width: Fill
                    label = {
                        text: "Sequencer",
                    },
                    draw_bg: {color: (COLOR_DEFAULT)}
                }
                menu = {
                    width: Fit
                }
            }

            <GradientYView> {
                height: Fit
                flow: Down
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

                <View> {
                    height: Fit,
                    width: Fill
                    flow: Right,
                    align: {x: 0.0, y: 0.5}
                    spacing: (SSPACING_4),
                    padding: {top: (SSPACING_1), right: (SSPACING_3), bottom: (SSPACING_0), left: (SSPACING_0)}

                    // playpause = <PlayPause> {}

                    playpause = <CheckBox> {
                        draw_check: {check_type: None}
                        draw_icon: {svg_file: (ICO_PLAY)}
                        icon_walk: {
                            width: 25.0,
                            height: Fit,
                            margin: 0,
                        }

                        margin: {
                            top: 0,
                            right: -10,
                            bottom: 0,
                            left: 0,
                        }

                        padding: {
                            top: 2,
                            right: 0,
                            bottom: 0,
                            left: 15,
                        }
                    }

                    speed = <InstrumentSlider> {
                        width: Fill
                        slider = {
                            draw_slider: {line_color: (COLOR_DEFAULT)}
                            min: 0.0
                            max: 240.0
                            text: "BPM"
                        }
                    }
                }

                <Divider> {margin: {top: (SSPACING_2), right: (SSPACING_0), bottom: (SSPACING_0)}}

                sequencer = <Sequencer> {width: Fill, height: 300, margin: {top: (SSPACING_3)}}

                <Divider> {margin: {top: (SSPACING_2), right: (SSPACING_0), bottom: (SSPACING_0)}}

                <SequencerControls> {}

            }
        }
    }


    BlurFXPanel = <View dx:-51.6 dy:3225.7 dw:400.0 dh:176.7> {
        width: Fill,
        height: Fit
        flow: Down

            <View> {
            flow: Right,
            align: {x: 0.0, y: 0.0}
            width: Fill,
            height: Fit

                <SubheaderContainer> {
                margin: {top: (SSPACING_0)}
                <FishSubTitle> {
                    label = {
                        text: "Blur",
                        draw_text: {color: (COLOR_FX)},
                    }
                }

                <FillerV> {}


            }
        }

        <View> {
            width: Fill,
            height: Fit,
            flow: Down,
            blursize = <InstrumentSlider> {
                width: Fill,
                height: Fit
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 1.0
                    text: "Size"

                }
            }
            blurstd = <InstrumentSlider> {
                width: Fill,
                height: Fit
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 1.0
                    text: "Stddev"

                }
            }
        }
    }


    ShadowFXPanel = <View dx:-35.3 dy:3462.1 dw:400.5 dh:231.9> {
        width: Fill,
        height: Fit
        flow: Down

            <View> {
            flow: Right,
            align: {x: 0.0, y: 0.0}
            width: Fill,
            height: Fit

                <SubheaderContainer> {
                margin: {top: (SSPACING_0)}
                <FishSubTitle> {
                    label = {
                        text: "Shadow",
                        draw_text: {color: (COLOR_FX)},
                    }
                }

                <FillerV> {}


            }
        }

        <View> {
            width: Fill,
            height: Fit,
            flow: Down,
            shadowopacity = <InstrumentSlider> {
                width: Fill,
                height: Fit
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 1.0
                    text: "Opacity"

                }
            }
            shadowx = <InstrumentSlider> {
                width: Fill,
                height: Fit
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 100.0
                    text: "X"

                }
            }
            shadowy = <InstrumentSlider> {
                width: Fill,
                height: Fit
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 100.0
                    text: "Y"

                }
            }
        }
    }

    CrushFXPanel = <View dx:818.7 dy:2070.0 dw:338.6 dh:110.0> {
        width: Fill,
        height: Fit
        flow: Down

            <View> {
            flow: Right,
            align: {x: 0.0, y: 0.0}
            width: Fill,
            height: Fit

                <SubheaderContainer> {
                margin: {top: (SSPACING_0)}
                <FishSubTitle> {
                    label = {
                        text: "Bitcrush",
                        draw_text: {color: (COLOR_FX)},
                    }
                }

                <FillerV> {}

                crushenable = <FishToggle> {
                    margin: <SPACING_0> {}
                    padding: <SPACING_0> {}
                    checkbox = {
                        text: " "
                        padding: {top: (SSPACING_0), right: (SSPACING_1), bottom: (SSPACING_0), left: (SSPACING_0)}
                        margin: <SPACING_0> {}
                    }
                    width: Fit,
                    height: Fit,
                    margin: {top: (SSPACING_0)}
                }
            }
        }

        <View> {
            width: Fill,
            height: Fit
            crushamount = <InstrumentSlider> {
                width: Fill,
                height: Fit
                slider = {
                    draw_slider: {line_color: (COLOR_FX)}
                    min: 0.0
                    max: 1.0
                    text: "Amount"

                }
            }
        }
    }

    DelayFXPanel = <FishPanelContainer dx:834.7 dy:2708.5 dw:323.8 dh:187.0> {
        <SubheaderContainer> {
            <FishSubTitle> {
                label = {
                    text: "Delay",
                    draw_text: {color: (COLOR_FX)},
                }
            }
        }
        <View> {
            flow: Down
            width: Fill,
            height: Fit

                <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit

                delaysend = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Send"
                    }
                }

                delayfeedback = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Feedback"

                    }
                }

            }

            <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit

                delaydifference = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Stereo difference"
                    }
                }

                delaycross = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Crossover"
                    }
                }
            }
            <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit

                delaylength = <InstrumentSlider> {
                    

                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Length"
                    }
                }
                <View>{width: Fill}
            }
        }
    }

    ChorusFXPanel = <FishPanelContainer dx:824.1 dy:2219.5 dw:329.5 dh:238.6> {
        <SubheaderContainer> {
            <FishSubTitle> {
                label = {
                    text: "Chorus",
                    draw_text: {color: (COLOR_FX)},
                }
            }
        }
        <View> {
            flow: Down
            width: Fill,
            height: Fit

                <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit

                chorusmix = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Mix"
                    }
                }
                chorusdelay = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Pre"
                    }
                }
            }
            <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit
                chorusmod = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Depth"
                    }
                }
                chorusrate = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Rate"
                    }
                }
            }
            <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit
                chorusphase = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Phasing"
                    }
                }

                chorusfeedback = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: -1
                        max: 1
                        text: "Feedback"
                    }
                }
            }

        }
    }

    DelayToyFXPanel = <FishPanelContainer dx:831.2 dy:2530.3 dw:320.5 dh:123.9> {
        <SubheaderContainer> {
            <FishSubTitle> {
                label = {
                    text: "Reverb",
                    draw_text: {color: (COLOR_FX)},
                }
            }
        }
        <View> {
            flow: Down
            width: Fill,
            height: Fit

                <View> {
                flow: Right
                width: Fill,
                height: Fit

                reverbmix = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Mix"
                    }
                }
                reverbfeedback = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FX)}
                        min: 0.0
                        max: 1.0
                        text: "Feedback"
                    }
                }
            }
        }
    }

    FishPanelFilter = <FishPanelContainer dx:404.9 dy:3055.9 dw:377.3 dh:230.0> {

        <FishPanel> {
            height: Fit

                <FishHeader> {
                draw_bg: {color: (COLOR_FILTER)}
                title = {
                    width: Fit
                    label = {
                        text: "Hello",
                    },
                }

                menu = <View> {
                    filter_type = <FishDropDown> {
                        width: Fill

                        labels: ["LowPass", "HighPass", "BandPass", "BandReject"]
                        values: [LowPass, HighPass, BandPass, BandReject]

                        draw_text: {
                            text_style: <H2_TEXT_REGULAR> {},
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
                                width: Fill,
                                height: Fit

                                padding: {left: (SSPACING_4), top: (SSPACING_2), bottom: (SSPACING_2), right: (SSPACING_2)}
                            }
                        }

                    }
                }
            }

            <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit
                cutoff = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FILTER)}
                        min: 0.0
                        max: 1.0
                        text: "Cutoff"
                    }
                }

                resonance = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FILTER)}
                        min: 0.0
                        max: 1.0
                        text: "Resonance"
                    }
                }
            }
            <View> {
                flow: Right,
                spacing: (SSPACING_1)
                width: Fill,
                height: Fit

                lfoamount = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FILTER)}
                        min: -1.0
                        max: 1.0
                        text: "Cutoff LFO Amount"
                    }
                }
                rate = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_FILTER)}
                        max: 1.0
                        text: "Cutoff LFO Rate"
                    }
                }
            }

            sync = <FishToggle> { checkbox = {width: 200, text: "LFO Key sync"}}
        }
    }

    OscPanel = <View dx:-55.1 dy:2583.2 dw:400.0 dh:581.2> {
        width: Fill,
        height: Fit
        flow: Down
            <View> {
            flow: Right
            width: Fill,
            height: Fit

                <SubheaderContainer> {
                <FishSubTitle> {label = {text: "Osc", draw_text: {color: (COLOR_OSC)}, width: Fit}}
                type = <InstrumentDropdown> {
                    flow: Down
                    dropdown = {
                        width: Fill,
                        height: Fit
                        values: [DPWSawPulse, BlampTri, Pure, SuperSaw, HyperSaw, HarmonicSeries]
                        labels: ["Saw", "Triangle", "Sine", "Super Saw", "Hyper Saw", "Harmonic"]
                    }
                }
            }
        }

        twocol = <View> {
            flow: Down
            width: Fill,
            height: Fit
            transpose = <InstrumentBipolarSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_OSC)}
                    min: -24.0
                    max: 24.0
                    step: 1.0
                    precision: 0,
                    text: "Transpose"
                }
            }

            detune = <InstrumentBipolarSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_OSC)}
                    min: -1.0
                    max: 1.0
                    text: "Detune"
                }
            }
        }

        <View> {
            flow: Down
            width: Fill,
            height: Fit
            supersaw = <View> {
                flow: Down
                width: Fill,
                height: Fit
                spread = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        text: "Spread"
                    }
                }
                diffuse = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        text: "Diffuse"
                    }
                }
            }


            hypersaw = <View> {
                flow: Down
                width: Fill,
                height: Fit
                spread = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        text: "Spread"
                    }
                }
                diffuse = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0.0
                        max: 1.0
                        text: "Diffuse"
                    }
                }
            }

            harmonic = <View> {
                flow: Down
                width: Fill,
                height: Fit
                harmonicshift = <InstrumentSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: 0
                        max: 1.0
                        text: "Shift"
                    }
                }
                harmonicenv = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: -1.0
                        max: 1.0
                        text: "Env mod"
                    }
                }
                harmoniclfo = <InstrumentBipolarSlider> {
                    slider = {
                        draw_slider: {line_color: (COLOR_OSC)}
                        min: -1.0
                        max: 1.0
                        text: "LFO mod"
                    }
                }
            }
        }
    }

    MixerPanel = <View dx:836.1 dy:2954.0 dw:320.3 dh:137.5> {
        width: Fill,
        height: Fit
        flow: Down
            <View> {
            flow: Right,
            spacing: (SSPACING_1)
            width: Fill,
            height: Fit
            noise = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_OSC)}
                    min: 0.0
                    max: 1.0
                    text: "Noise"
                }
            }
            sub = <InstrumentSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_OSC)}
                    min: 0.0
                    max: 1.0
                    text: "Sub"
                }
            }
        }
        <View> {
            flow: Right
            width: Fill,
            height: Fit
            balance = <InstrumentBipolarSlider> {
                slider = {
                    draw_slider: {line_color: (COLOR_OSC)}
                    min: 0.0
                    max: 1.0
                    text: "Oscillator Balance"
                }
            }
        }
    }

    FishPanelSoundSources = <FishPanelContainer dx:-59.4 dy:1375.0 dw:400.0 dh:1144.1> {
        width: Fill,
        height: Fill
        padding: <SPACING_0> {}
        spacing: (SSPACING_0)
        flow: Down

            <FishPanelScrollY> {
            height: Fill

                <FishHeader> {
                title = {
                    label = {
                        text: "Sound Sources",
                    },
                    draw_bg: {color: (COLOR_OSC)}
                }
            }

            <SubheaderContainer> {
                margin: {top: (SSPACING_0)}
                <FishSubTitle> {
                    label = {
                        text: "Mixer",
                        draw_text: {color: (COLOR_OSC)},
                    }
                }
            }

            <MixerPanel> {width: Fill, height: Fit}

            <View> {
                width: Fill,
                height: Fit
                flow: Right,
                spacing: (SSPACING_2)

                osc1 = <OscPanel> {}
                osc2 = <OscPanel> {}
            }
            //<BlurFXPanel> {}
            //<ShadowFXPanel> {}
            <FillerV> {}
        }
    }

    HeaderMenu = <View dx:-919.9 dy:3508.2 dw:374.3 dh:65.2> {
        width: Fill,
        height: Fit,
        margin: {top: -150}
        flow: Right,
        spacing: (SSPACING_0),
        align: {x: 0.0, y: 0.0}

        <View> { // TODO: Remove excessive nesting?
            flow: Down,
            align: {x: 0.0, y: 0.0}
            spacing: 0,
            padding: <SPACING_2> {}
            height: 135,
            width: Fill,
            margin: <SPACING_2> {}

            <View> {
                width: Fill
                flow: Right,
                align: {x: 0.0, y: 0.0}

                <View> {
                    flow: Down,
                    align: {x: 0.0, y: 0.0}
                    <Label> {
                        margin: {bottom: (SSPACING_1), right:10}
                        draw_text: {
                            text_style: <H2_TEXT_BOLD> {},
                            color: (COLOR_UP_5)
                        }
                        text: "Preset"
                    }
                    <View>{
                        preset_1 = <Button>{text:"1"}
                        preset_2 = <Button>{text:"2"}
                        preset_3 = <Button>{text:"3"}
                        preset_4 = <Button>{text:"4"}
                        preset_5 = <Button>{text:"5"}
                        preset_6 = <Button>{text:"6"}
                        preset_7 = <Button>{text:"7"}
                        preset_8 = <Button>{text:"8"}
                    }
                        
                    
                    /*
                    <Label> {
                        draw_text: {
                            text_style: <H2_TEXT_REGULAR> {font_size: 18},
                            color: (COLOR_UP_6)
                        }
                        text: "Ironfish "
                    }*/
                }
                <View> {
                    width: Fill,
                    height: Fit,
                    margin: <SPACING_4> {}
                    spacing: (SSPACING_1)
                }

                <Image> {
                    source: dep("crate://self/resources/tinrs.png"),
                    width: (1000 * 0.175),
                    height: (175 * 0.175),
                    margin: 0
                }

            }

            <FillerV> {}

            <View> {
                width: Fill,
                height: 35
                spacing: (SSPACING_1)


                prev = <IconButton> {draw_icon: {svg_file: (ICO_PREV)} icon_walk: {width: Fit, height: 11.0}, margin: {top: 3.25, right: -10.0, bottom: 0.0, left: 0.0}}
                presets = <IconButton> {draw_icon: {svg_file: (ICO_PRESET)} icon_walk: {width: Fit, height: 17.5}, margin: 0.0}
                next = <IconButton> {draw_icon: {svg_file: (ICO_NEXT)}, icon_walk: {width: Fit, height: 11.0}, margin: {top: 3.25, right: 0.0, bottom: 0.0, left: -10.0}}

                panic = <IconButton> {draw_icon: {svg_file: (ICO_PANIC)} icon_walk: {width: Fit, height: 17.0}, margin: {left: 5.0, right: -10.0}}
                platformtoggle = <IconButton> {draw_icon: {svg_file: (ICO_PLAT_MOBILE)} icon_walk: {width: Fit, height: 18.5}}

                gitlink = <Label> {
                    draw_text: {text_style: <H2_TEXT_REGULAR> {}, color: (COLOR_UP_5)}
                    text: "Made with Makepad\ngithub.com/makepad/makepad"
                    margin: {top: 7.5, left: 5.0}
                }

                <FillerH> {}

                undo = <IconButton> {draw_icon: {svg_file: (ICO_UNDO)} icon_walk: {width: Fit, height: 15.0}, margin: {top: 3.25, right: -5.0, bottom: 0.0, left: 0.0}}
                redo = <IconButton> {draw_icon: {svg_file: (ICO_REDO)} icon_walk: {width: Fit, height: 15.0}, margin: {top: 3.25, right: 0.0, bottom: 0.0, left: -5.0}}
            }

        }

    }

    Play = <FishPanel dx:-45.1 dy:3723.3 dw:1216.0 dh:143.2> {
        flow: Right,
        padding: {top: (SSPACING_3)}
        spacing: (SSPACING_0)
        height: Fit,
        width: Fill,
        margin: {top: (SSPACING_0), right: (SSPACING_3), bottom: (SSPACING_3), left: (SSPACING_3)}
        draw_bg: {color: (COLOR_UP_3), color2: (COLOR_UP_1)}

        <Arp> {}
        <ScrollXView> {
            piano = <Piano> {height: Fit, width: Fill, margin: {top: (SSPACING_0), right: (SSPACING_2); bottom: (SSPACING_3), left: (SSPACING_2)}}
        }
        <PianoSettings> {}
    }


    // TABS
    FishPanelEnvelopes = <FishPanelContainer dx:379.3 dy:1377.3 dw:400.0 dh:555.7> {
        width: Fill,
        height: Fill
        padding: <SPACING_0> {}
        align: {x: 0.0, y: 0.0},
        spacing: (SSPACING_0),
        flow: Down

            <FishPanelScrollY> {

            <FishHeader> {
                title = {
                    label = {
                        text: "Envelopes",
                    },
                    draw_bg: {color: (COLOR_ENV)}
                }
            }

            <SubheaderContainer> {
                margin: {top: (SSPACING_0)}
                <FishSubTitle> {
                    label = {
                        text: "Volume",
                        draw_text: {color: (COLOR_ENV)},
                    }
                }
            }

            <VolumeEnvelopePanel> {
                flow: Down
                width: Fill,
                height: Fit
            }

            <ModEnvelopePanel> {
                flow: Down,
                clip_y: true
                width: Fill,
                height: Fit
            }
        }
    }

    FishPanelEffects = <FishPanelContainer dx:821.3 dy:1353.5 dw:338.4 dh:644.1> {
        width: Fill,
        height: Fill
        padding: <SPACING_0> {}
        align: {x: 0.0, y: 0.0},
        spacing: (SSPACING_0),
        flow: Down

            <FishPanelScrollY> {

            <FishHeader> {
                title = {
                    label = {
                        text: "Effects",
                    },
                    draw_bg: {color: (COLOR_FX)}
                }
            }

            width: Fill,
            height: Fill
                <CrushFXPanel> {}
            <ChorusFXPanel> {}
            <DelayToyFXPanel> {}
            <DelayFXPanel> {}
        }
    }
 
    PresetHeader = <View dx:-502.2 dy:1426.1 dw:400.0 dh:121.8> {
        width: Fill,
        height: Fit,
        margin: {top: 0, right: (SSPACING_4), bottom: 0, left: (SSPACING_4)}
        flow: Down,
        spacing: (SSPACING_2),
        padding: 0

            <SubheaderContainer> {
            <FishSubTitle> {
                width: Fill
                label = {
                    text: "Browse",
                    draw_text: {color: (COLOR_UP_6)}
                }
            }

            <FillerH> {}
            <CheckboxTextual> {text: "Synth", width: Fit}
            <CheckboxTextual> {text: "Seq", width: Fit}
            <CheckboxTextual> {text: "Fav", width: Fit}
        }

        <FishInput> {}

    }
    /*
    PresetListEntry = <SwipeListEntry> {
        flow: Down,
        padding: 0.0
        width: Fill,
        height: Fit

        center: <View> {
            flow: Down,
            padding: 0.0
            width: Fill,
            height: Fit

                <View> {
                flow: Right,
                align: {x: 0.0, y: 0.5 padding: 0.0}
                width: Fill,
                height: Fit,
                margin: {left: 5.0, top: 2.5}

                label = <Button> {
                    width: Fill,
                    height: Fill
                    align: {x: 0.0, y: 0.5 padding: 0.0}
                    draw_text: {
                        fn get_color(self) -> vec4 {
                            return mix(
                                mix((COLOR_UP_5), (COLOR_UP_6), self.hover),
                                (COLOR_UP_4),
                                self.pressed
                            )
                        }
                        text_style: <H2_TEXT_REGULAR> {},
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

                <View> {
                    width: Fit,
                    height: Fit,
                    margin: 0.0
                    padding: 0.0

                    presetfavorite = <PresetFavorite> {
                        width: 30.0,
                        height: 30.0,
                        margin: {top: 10.0, right: -10.0, bottom: 0.0, left: 0.0}
                        padding: 0.0
                        text: ""
                    }

                    share = <IconButton> {
                        margin: {top: 3.0, right: 0.0, bottom: 0.0, left: 0.0}
                        draw_icon: {
                            svg_file: (ICO_SHARE),
                            fn get_color(self) -> vec4 {
                                return mix(
                                    mix((COLOR_UP_3), (COLOR_UP_6), self.hover),
                                    (COLOR_UP_3),
                                    self.pressed
                                )
                            }
                        }
                        icon_walk: {width: 12.5, height: Fit}
                    }
                }
            }

            <Divider> {margin: {top: (SSPACING_1), right: (SSPACING_0), bottom: (SSPACING_0)}}
        }
    }

    PresetList = <SwipeList> {
        height: Fill,
        margin: 2.5
        Entry = <PresetListEntry> {
        }
    }

    PresetSaver = <View> {
        width: Fill,
        height: Fit,
        margin: {top: (SSPACING_4), right: (SSPACING_4), bottom: (SSPACING_0), left: (SSPACING_4)}
        //  flow: Down, spacing: (SSPACING_2)
        padding: <SPACING_0> {align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}

        <FishHeader> {
            title = {
                label = {
                    text: "Presets",
                    draw_text: {
                        color: (COLOR_UP_6)
                    }
                },
                draw_bg: {color: (COLOR_UP_4)}
            }
        }

        <SubheaderContainer> {
            margin: {top: (SSPACING_0)}
            <FishSubTitle> {
                width: Fill
                label = {
                    text: "Save",
                    draw_text: {color: (COLOR_UP_6)}
                }
            }

            <FillerV> {}

            <CheckboxTextual> {text: "Synth"}
            <CheckboxTextual> {text: "Seq"}
        }

        <View> {
            width: Fill,
            height: Fit
            flow: Down,
            spacing: (SSPACING_2),
            align: {x: 0.0, y: 0.5}

            <View> {
                width: Fill,
                height: Fit
                flow: Right,
                spacing: (SSPACING_2),
                align: {x: 0.0, y: 0.0}

                presetname = <FishInput> {
                    text: "Preset Name"
                }

                save = <IconButton> {
                    draw_icon: {svg_file: (ICO_SAVE)}
                    icon_walk: {width: 16, height: Fit}
                    padding: {top: 6.0, right: 3.0, bottom: 6.0, left: 0.0}
                }
            }

            <View> {
                width: Fill,
                height: Fit
                padding: {top: (SSPACING_0), right: (SSPACING_2), bottom: (SSPACING_0), left: (SSPACING_2)}
                <Label> {
                    margin: {right: 2.5}
                    text: "Overwrite preset?"
                    draw_text: {
                        color: (COLOR_UP_5)
                    }
                }
                <FillerH> {}
                confirm = <TextButton> {text: "Yes"}
                <Label> {
                    text: " · "
                    draw_text: {
                        color: (COLOR_UP_5)
                    }
                }
                cancel = <TextButton> {text: "No"}
            }

        }
    }

    Presets = <GradientXView> {
        width: 250,
        height: Fill
        flow: Down,
        padding: {right: 5, top: 15.0, left: 0.0}

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
    }*/
    
    AppDesktop = <View dx:-36.0 dy:-224.4 dw:1613.1 dh:1522.3>{
        flow: Right
        width: Fill,
        height: Fill
        // padding: <SPACING_0> { align: {x: 0.0, y: 0.0}, spacing: (SSPACING_0), flow: Down}

        <View> {
            width: Fill,
            height: Fill
            padding: <SPACING_0> {}
            align: {x: 0.0, y: 0.0},
            spacing: (SSPACING_0),
            flow: Down
            // APPLICATION HEADER
            <GradientYView> {
                width: Fill,
                height: (HEIGHT_AUDIOVIZ)
                draw_bg: {color: (COLOR_VIZ_1), color2: (COLOR_VIZ_2)}
                display_audio = <DisplayAudio> {
                    height: Fill,
                    width: Fill
                    draw_wave: {
                        fn vu_fill(self) -> vec4 {return #0000}
                    }
                }
            }

            <HeaderMenu> {
            }

            // CONTROLS
            <View> {
                width: Fill,
                height: Fill
                flow: Right,
                spacing: (SSPACING_1),
                padding: <SPACING_3> {}
                oscillators = <FishPanelSoundSources> {}
                <View> {
                    flow: Down,
                    spacing: (SSPACING_1)
                    height: Fill,
                    width: Fill
                    envelopes = <FishPanelEnvelopes> {}
                    <FishPanelFilter> {}
                }
                effects = <FishPanelEffects> {}
                <SequencerPanel> {height: Fill, width: Fill}
            }

            <Play> {}
        }
    }
}
