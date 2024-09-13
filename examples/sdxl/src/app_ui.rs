use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;
    
    TEXT_BIG = 12.0
    
    COLOR_UP_0 = #xFFFFFF00
    COLOR_DOWN_2 = #x00000022
    FONT_SIZE_H2 = 10.0
    
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
    
    H2_TEXT_REGULAR = {
        font_size: (FONT_SIZE_H2),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
    }
    
    TEXT_BOLD = {
        font_size: 10.0,
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }
    
    TEXT_MONO = {
        font_size: 10.0,
        font: {path: dep("crate://makepad-widgets/resources/LiberationMono-Regular.ttf")}
    }
    
    COLOR_PANEL_BG = (COLOR_DOWN_2)
    COLOR_TEXT_INPUT = (COLOR_DOWN_2)
    COLOR_LABEL = #xFFF9
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
    
    SdxlDropDown = <DropDown> {
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
    
    BarLabel = <Label dx:-103.6 dy:393.8 dw:400.0 dh:300.0> {
        margin: {left: 10},
        text: "Workflow",
        draw_text: {
            text_style: <TEXT_BOLD> {},
            fn get_color(self) -> vec4 {
                return (COLOR_LABEL)
            }
        }
    }
    
    BarButton = <Button dx:279.6 dy:-136.8 dw:400.0 dh:300.0> {
        padding: {top: 5.0, right: 7.5, bottom: 5.0, left: 7.5}
        margin: {top: 5.0, right: 5.0, bottom: 5.0, left: 5.0}
        text: "Cancel"
        draw_text: {
            text_style: <TEXT_BOLD> {},
        }
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(#53, #5c, self.hover), #33, self.pressed);
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(body_transp, mix(#6d, #1f, self.pressed), max(0.0, grad_top - sdf.pos.y) / grad_top);
                let bot_gradient = mix(
                    mix(body_transp, #5c, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                // the little drop shadow at the bottom
                let shift_inward = self.border_radius + 4.0;
                sdf.move_to(shift_inward, self.rect_size.y - self.border_radius);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y - self.border_radius);
                sdf.stroke(
                    mix(mix(#0006, #1f, self.hover), #0000, self.pressed),
                    self.border_radius
                )
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    1.0
                )
                
                return sdf.result
            }
        }
    }
    
    SettingsInput = <View> {
        width: 150,
        height: Fit,
        margin: {top: 10},
        label = <BarLabel> {
            width: Fit,
            margin: {left: 5},
            align: {x: 1.0}
        }
        input = <TextInput> {
            padding: 0
            height: Fit,
            width: 50,
            margin: {top: 1, left: 2}
            text: "1344"
        }
    }
    
    FishSlider = <Slider> {
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
            instance line_color: #8
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
    
    SettingsSlider = <View dx:210.7 dy:304.6 dw:400.0 dh:300.0> {
        width: 130,
        height: Fit,
        margin: {top: 0},
       /* label = <BarLabel> {
            width: Fit,
            margin: {left: 5},
            align: {x: 1.0}
        }*/
        input = <FishSlider> {
            padding: 0
            height: 37,
            width: 125,
            margin: {top: 1, left: 2}
            text: "HELLO WOLD"
        }
    }
    
    FillerH = <View dx:303.1 dy:271.4 dw:400.0 dh:300.0> {
        width: Fill,
        height: Fit
    }
    
    FillerV = <View> {
        width: Fit,
        height: Fill
    }
    
    
    DividerV = <View dx:-313.3 dy:119.9 dw:400.0 dh:300.0> {
        flow: Down,
        spacing: 0.0
        margin: {top: 0.0, right: 0.0, bottom: 10.0, left: 0.0}
        width: Fill,
        height: Fit
            <RectView> {
            height: 2,
            width: Fill,
            margin: 0.0
            flow: Down,
            padding: 0.0
            draw_bg: {color: #x00000033}
        }
        <RectView> {
            height: 2,
            width: Fill,
            margin: 0.0
            flow: Down,
            padding: 0.0
            draw_bg: {color: #xFFFFFF18}
        }
    }
    
    DividerH = <View> {
        flow: Right,
        spacing: 0.0
        margin: {top: 0.0, right: 5.0, bottom: 0.0, left: 5.0}
        width: Fit,
        height: Fill
            <RectView> {
            height: Fill,
            width: 2,
            margin: 0.0
            flow: Down,
            padding: 0.0
            draw_bg: {color: #x00000033}
        }
        <RectView> {
            height: Fill,
            width: 2,
            margin: 0.0
            flow: Down,
            padding: 0.0
            draw_bg: {color: #xFFFFFF18}
        }
    }
    
    SdxlCheckBox = <CheckBox> {
        padding: {top: (SSPACING_0), right: 0, bottom: (SSPACING_0), left: 23}
        label_walk: {margin: {left: 20.0, top: 8, bottom: 8, right: 10}}
        animator: {
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
                
                sdf.stroke_keep(#xFFF2, 1.25)
                
                sdf.fill(#xFFF0)
                let isz = sz * 0.65;
                sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                sdf.circle(left + sz + self.selected * sz, c.y - 0.5, 0.425 * isz);
                sdf.subtract();
                sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                sdf.blend(self.selected)
                sdf.fill(#xFFF8);
                return sdf.result
            }
        }
        draw_text: {
            text_style: <TEXT_BOLD> {},
            fn get_color(self) -> vec4 {
                return (COLOR_LABEL)
            }
        }
        text: "Slideshow"
    }
    
    ProgressCircle = <View dx:-194.2 dy:-240.4 dw:400.0 dh:300.0> {
        show_bg: true,
        width: 24,
        height: 24
        draw_bg: {
            instance progress: 0.0
            instance active: 0.0
            
            fn circle_pie(inout sdf: Sdf2d, x: float, y: float, r: float, s: float) {
                let c = sdf.pos - vec2(x, y);
                let len = sqrt(c.x * c.x + c.y * c.y) - r;
                let pi = 3.141592653589793;
                let ang = (pi - atan(c.x, c.y)) / (2.0 * pi);
                let ces = s * 0.5;
                let ang2 = clamp((abs(ang - ces) - ces) * -r * r * sdf.scale_factor, 0.0, 1.0);
                sdf.dist = len * ang2 / sdf.scale_factor;
                sdf.old_shape = sdf.shape;
                sdf.shape = min(sdf.shape, sdf.dist);
            }
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.circle(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.rect_size.x * 0.4
                );
                sdf.fill(mix(#4, #575, self.active));
                circle_pie(
                    sdf,
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.rect_size.x * 0.4,
                    self.progress
                );
                sdf.fill(mix(#4, #8f8, self.active));
                return sdf.result;
            }
        }
    }
    
    PromptGroup = <RectView> {
        <DividerV> {}
        height: Fit,
        width: Fill,
        margin: {bottom: 10, top: 0}
        flow: Down,
        spacing: 0,
        padding: 0
        draw_bg: {
            instance hover: 0.0
            instance down: 0.0
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let body = mix(mix(#53, #5c, self.hover), #33, self.down);
                sdf.fill_keep(body)
                return sdf.result
            }
        }
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        draw_bg: {hover: 0.0}
                        prompt = {draw_text: {hover: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.2}
                    }
                    apply: {
                        draw_bg: {hover: 1.0}
                        prompt = {draw_text: {hover: 1.0}}
                    }
                }
            }
            down = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        draw_bg: {down: 0.0}
                        prompt = {draw_text: {down: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.2}
                    }
                    apply: {
                        draw_bg: {down: 1.0}
                        prompt = {draw_text: {down: 1.0}}
                    }
                }
            }
        }
        prompt = <Label> {
            width: Fill
            draw_text: {
                text_style: <TEXT_BOLD> {},
                instance hover: 0.0
                instance down: 0.0
                fn get_color(self) -> vec4 {
                    return mix(mix(#xFFFA, #xFFFF, self.hover), #xFFF8, self.down);
                }
                wrap: Word,
            }
            text: ""
        }
    }
    
    ImageTile = <View> {
        width: Fill,
        height: Fit
        cursor: Hand
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        img = {draw_bg: {hover: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.3}
                    }
                    apply: {
                        img = {draw_bg: {hover: 1.0}}
                    }
                }
            }
            down = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        img = {draw_bg: {down: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.3}
                    }
                    apply: {
                        img = {draw_bg: {down: 1.0}}
                    }
                }
            }
        }
        
        img = <Image> {
            width: Fill,
            height: Fill
            min_width: 1920,
            min_height: 1080,
            fit: Horizontal,
            draw_bg: {
                instance hover: 0.0
                instance down: 0.0
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(1, 1, self.rect_size.x - 2, self.rect_size.y - 2, 4.0)
                    let max_scale = vec2(0.92);
                    let scale = mix(vec2(1.0), max_scale, self.hover);
                    let pan = mix(vec2(0.0), (vec2(1.0) - max_scale) * 0.5, self.hover);
                    let color = self.get_color_scale_pan(scale, pan) + mix(vec4(0.0), vec4(0.1), self.down);
                    sdf.fill_keep(color);
                    sdf.stroke(
                        mix(mix(#x0000, #x0006, self.hover), #xfff2, self.down),
                        1.0
                    )
                    
                    return sdf.result
                }
            }
        }
    }
    
    VideoFrame = <Image dx:-22.8 dy:313.4 dw:400.0 dh:300.0> {
        height: 160,
        width: 90,
        width_scale: 2.0,
        fit: Biggest,
        draw_bg: {
            uniform image_size: vec2
            uniform is_rgb: 0.0
            
            fn yuv_to_rgb(y: float, u: float, v: float) -> vec4 {
                return vec4(
                    y + 1.14075 * (v - 0.5),
                    y - 0.3455 * (u - 0.5) - 0.7169 * (v - 0.5),
                    y + 1.7790 * (u - 0.5),
                    1.0
                )
            }
                        
            fn get_video_pixel(self, pos:vec2) -> vec4 {
                let pix = self.pos * self.image_size;
                                
                // fetch pixel
                let data = sample2d(self.image, pos).xyzw;
                if self.is_rgb > 0.5 {
                    return vec4(data.xyz, 1.0);
                }
                if mod (pix.x, 2.0)>1.0 {
                    return yuv_to_rgb(data.x, data.y, data.w)
                }
                return yuv_to_rgb(data.z, data.y, data.w)
            }
                        
            fn pixel(self) -> vec4 {
                let c = self.get_video_pixel(self.pos);
                return c;// mix(vec4(c.x, c.x,c.x, 1.0), c, 1.0-self.dial2)
            }
        }
    }
    AppUI = <View> {
                            
        flow: Overlay,
                            
                            
        width: Fill,
        height: Fill
                            
                            
        dock = <Dock> {
            height: Fill,
            width: Fill
                                    
            root = Splitter {
                axis: Horizontal,
                align: FromA(300.0),
                a: image_library,
                b: split1
            }
                                    
            split1 = Splitter {
                axis: Vertical,
                align: FromB(400.0),
                a: image_view,
                b: input_panel
            }
                                    
            image_library = Tab {
                name: ""
                kind: ImageLibrary
            }
                                    
            input_panel = Tab {
                name: ""
                kind: InputPanel
            }
                                    
            image_view = Tab {
                name: ""
                kind: ImageView
            }
                                    
            ImageView = <RectView> {
                draw_bg: {color: #2}
                height: Fill,
                width: Fill
                flow: Down,
                align: {x: 0.5, y: 0.5}
                cursor: Hand,
                image = <ImageBlend> {
                    fit: Smallest,
                    width: Fill,
                    height: Fill
                }
            }
                                    
            InputPanel = <RectView> {
                height: Fill,
                width: Fill
                flow: Down,
                padding: 0.0
                draw_bg: {color: (COLOR_PANEL_BG)}
                <View> {
                    height: Fit,
                    width: Fill
                    align: {x: 0.0, y: 0.5}
                    padding: 5
                    /*                                
                    <BarLabel> {
                        text: "Workflow"
                    }*/
                    
                    settings_cfg = <SettingsSlider> {input = {text: "Config", default: 4.0, min:1.0, max:8.0, step:0.01}}
                    
                    settings_steps = <SettingsSlider> {input = {text: "Steps", default: 10.0, min:1, max: 10, step:1}}
                                                
                    settings_denoise = <SettingsSlider> {input = {text: "Denoise", default: 0.5, min:0.2, max:1.0, step:0.01}}
                    
                    settings_delay = <SettingsSlider> {input = {text: "Delay", default: 0., min:0., max:5.0, step:0.01}}                    
                                          
                    //workflow_dropdown = <SdxlDropDown> {}
                    random_check_box = <SdxlCheckBox> {
                        text: "Random"
                    }
                                    
                    render_check_box = <SdxlCheckBox> {
                        text: "Render"
                    }
                    single_check_box = <SdxlCheckBox> {
                        text: "Single"
                    }
                    trim_button = <BarButton> {
                        text: "Trim"
                    }
                    clear_button = <BarButton> {
                        text: "Clear"
                    }
                    <BarLabel> {
                        text: "Seed:"
                    }
                    seed_input = <TextInput> {
                        draw_text: {text_style: <TEXT_BOLD> {}}
                        height: Fit,
                        width: Fit,
                        margin: {bottom: 0, left: 0}
                    }
                    <FillerH> {}
                }
                <View> {
                    <View>{
                        flow: Down
                        <RoundedView>{
                            draw_bg:{
                                color: (COLOR_DOWN_2)
                                border_width: 1.0
                                border_color: #x00000044
                            }
                            margin:{top:0, left:0, right: 0, bottom:5}
                            align: {x:0.5},
                            padding: 2
                            width: Fill,
                            height: Fill
                            llm_chat = <PortalList>{  
                                auto_tail:true,
                                width: Fill,
                                height: Fill,
                                margin: {top: 0},
                                AI = <TextInput> {
                                    ascii_only: true,
                                    width: Fill,
                                    height: Fill,
                                    margin: {top: 0.0, left: 20.0, bottom: 5.0, right: 0.0},
                                    text: "LLM Output"
                                    draw_text: {
                                        text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}
                                    }
                                    draw_bg: {
                                        color: (#335)
                                    }
                                }
                                Human = <TextInput> {
                                    ascii_only: true,
                                    width: Fill,
                                    height: Fill,
                                    margin: {top: 0.0, left: 0.0, bottom: 5.0, right: 0.0},
                                    text: "LLM Output"
                                    draw_text: {
                                        text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}
                                    }
                                    draw_bg: {
                                        color: (#353)
                                    }
                                }
                            }
                        }
                       
                        chat = <TextInput> {
                            height: Fit,
                            width: Fill,
                            margin: {top: 0.0, left: 0.0, bottom: 0.0, right: 0.0},
                            margin: {bottom: 0}
                            empty_message: "Talk here"
                            draw_bg: {
                                color: (COLOR_TEXT_INPUT)
                            }
                            draw_text: {
                                text_style: {font_size: (TEXT_BIG)}
                                fn get_color(self) -> vec4 {
                                    return
                                    mix(
                                        mix(
                                            mix(
                                                #xFFFFFF55,
                                                #xFFFFFF88,
                                                self.hover
                                            ),
                                            #xFFFFFFCC,
                                            self.focus
                                        ),
                                        #xFFFFFF66,
                                        self.is_empty
                                    )
                                }
                            }
                        }
                    }
                    positive = <TextInput> {
                        ascii_only: true,
                        width: Fill,
                        height: Fill,
                        margin: {top: 0.0, left: 5.0, bottom: 0.0, right: 0.0},
                        text: "Positive"
                        draw_text: {
                            text_style: <THEME_FONT_LABEL> {font_size: (TEXT_BIG)}
                        }
                        draw_bg: {
                            color: (COLOR_TEXT_INPUT)
                        }
                    }
                    <View>{
                        visible: false,
                        negative = <TextInput> {
                            ascii_only: true,
                            width: 200,
                            height: Fill,
                            margin: {top: 0.0, left: 5.0, bottom: 10.0, right: 10.0},
                            draw_text: {text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}}
                            text: "text, watermark, cartoon"
                            draw_bg: {
                                color: (COLOR_TEXT_INPUT)
                            }
                        }
                    }
                    <View> {
                        visible: false,
                        width: Fill,
                        height: Fill
                        flow: Right
                        <View> {
                            width: 200,
                            height: Fit,
                            
                            margin: {top: 10, right: 20},
                            flow: Down
                            settings_width = <SettingsInput> {label = {text: "width:"}, input = {text: "1344"}}
                            settings_height = <SettingsInput> {label = {text: "height:"}, input = {text: "768"}}

                        }
                        
                    }
                }
            }
                                    
            ImageLibrary = <RectView> {
                draw_bg: {color: (COLOR_PANEL_BG)}
                height: Fill,
                width: Fill
                flow: Down
                <View> {
                    height: Fit,
                    width: Fill
                    flow: Right,
                    padding: {left: 10, right: 10, top: 10, bottom: 10},
                    search = <TextInput> {
                        height: Fit,
                        width: Fill,
                        margin: {bottom: 0}
                        empty_message: "Search"
                        draw_bg: {
                            color: (COLOR_TEXT_INPUT)
                        }
                        draw_text: {
                            text_style: {font_size: (TEXT_BIG)}
                            fn get_color(self) -> vec4 {
                                return
                                mix(
                                    mix(
                                        mix(
                                            #xFFFFFF55,
                                            #xFFFFFF88,
                                            self.hover
                                        ),
                                        #xFFFFFFCC,
                                        self.focus
                                    ),
                                    #xFFFFFF66,
                                    self.is_empty
                                )
                            }
                        }
                    }
                }
                image_list = <PortalList> {
                    height: Fill,
                    width: Fill,
                    margin: {top: 0}
                    flow: Down,
                    padding: {top: 0, right: 10.0, bottom: 10.0, left: 10.0}
                                                    
                    PromptGroup = <PromptGroup> {}
                                                    
                    Empty = <View> {}
                                                    
                    ImageRow1 = <View> {
                        height: Fit,
                        width: Fill,
                        margin: {bottom: 10}
                        spacing: 20,
                        flow: Right
                        row1 = <ImageTile> {}
                    }
                }
                <RoundedView>{
                    draw_bg:{
                        color:#2
                    }
                    margin:{top:0, left:10, right: 10, bottom:10}
                    align: {x:0.5},
                    padding: 2
                    width: Fill,
                    height: 164
                    <View>{  
                        width: Fit,
                        height: Fit,
                        margin: {top: 0},
                        video_input0 = <VideoFrame>{}
                        video_input1 = <VideoFrame>{}
                    }
                }
                
            }
        }
                            
        big_image = <RectView> {
            visible: false,
            draw_bg: {draw_depth: 10.0}
            draw_bg: {color: #0}
            height: All,
            width: All,
            abs_pos: vec2(0.0, 0.0)
            flow: Overlay,
            align: {x: 0.5, y: 0.5}
            image1 = <ImageBlend> {
                draw_bg: {draw_depth: 11.0}
                fit: Smallest,
                width: Fill,
                height: Fill
            }
        }
    }
    
    AppWindow = <View>{
        
        second_image = <RectView> {
            draw_bg: {color: #0}
            height: Fill,
            width: Fill
            flow: Overlay,
            align: {x: 0.5, y: 0.5}
            image1 = <ImageBlend> {
                breathe: true,
                fit: Smallest,
                width: Fill,
                height: Fill
            }
            prompt_frame = <View> {
                width: Fill,
                height: Fill
                align: {y: 1.0}
                padding: {left: 120, bottom: 40, right: 120}
                prompt = <Label> {
                    width: Fill,
                    height: Fit
                    draw_text: {
                        wrap: Word
                        text_style: <TEXT_BOLD> {font_size: 20}
                                                                                                    
                        color: #c
                    },
                    text: "HELLO WORLD"
                }
            }
        }
    }
}
