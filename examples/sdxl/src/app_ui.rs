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
    
    BarLabel = <Label> {
        margin: {left: 10},
        text: "Workflow",
        draw_text: {
            text_style: <TEXT_BOLD> {},
            fn get_color(self) -> vec4 {
                return (COLOR_LABEL)
            }
        }
    }
    
    BarButton = <Button> {
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
    
    SettingsSlider = <View> {
        width: 250,
        height: Fit,
        margin: {top: 10},
        label = <BarLabel> {
            width: Fit,
            margin: {left: 5},
            align: {x: 1.0}
        }
        input = <Slider> {
            padding: 0
            height: Fit,
            width: 125,
            margin: {top: 1, left: 2}
            text: "1344"
        }
    }
    
    FillerH = <View> {
        width: Fill,
        height: Fit
    }
    
    FillerV = <View> {
        width: Fit,
        height: Fill
    }
    
    
    DividerV = <View> {
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
    
    ProgressCircle = <View> {
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
    
    VideoFrame = <Image> {
        height: 160,
        width: 90,
        width_scale: 2.0,
        fit: Biggest,
        draw_bg: {
            uniform image_size: vec2
            uniform is_rgb: 0.0
            uniform dial1: 0.0,
            uniform dial2: 0.0,
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
                let d1 = (self.dial1-0.5)*2.0;
                let d2 = pow(self.dial2*5.0,2.0);
                let shift = vec4(d1,d1,d1,0.0)
                let scale = vec4(d2,d2,d2,1.0)
                let c = (self.get_video_pixel(self.pos)-shift)*scale + shift;
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
                align: FromB(200.0),
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
                image = <Image> {
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
                                                    
                    <BarLabel> {
                        text: "Workflow"
                    }
                                                    
                    workflow_dropdown = <SdxlDropDown> {}
                   
                    <BarLabel> {
                        text: "Seed"
                    }
                    seed_input = <TextInput> {
                        draw_text: {text_style: <TEXT_BOLD> {}}
                        height: Fit,
                        width: Fit,
                        margin: {bottom: 0, left: 0}
                    }
                                                    
                    take_photo = <BarButton> {
                        text: "Photo"
                    }
                    render_single = <BarButton> {
                        text: "Render"
                    }

                    random_check_box = <SdxlCheckBox> {
                        text: "Random"
                    }                      
                    <DividerH> {}
                    auto_check_box = <SdxlCheckBox> {
                        text: "Auto"
                    }
                          
                    <DividerH> {}
                    clear_toodo = <BarButton> {
                        text: "Clear Todo"
                    }
                    <FillerH> {}
                    progress1 = <ProgressCircle> {}
                    todo_label = <BarLabel> {
                        margin: {right: 5.0}
                        text: "Todo 0"
                    }
                }
                <View> {
                    positive = <TextInput> {
                        ascii_only: true,
                        width: Fill,
                        height: Fill,
                        margin: {top: 0.0, left: 10.0, bottom: 10.0, right: 5.0},
                        text: "Positive"
                        draw_text: {
                            text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}
                        }
                        draw_bg: {
                            color: (COLOR_TEXT_INPUT)
                            border_width: 1.0
                            border_color: #x00000044
                        }
                    }
                    negative = <TextInput> {
                        ascii_only: true,
                        width: 200,
                        height: Fill,
                        margin: {top: 0.0, left: 5.0, bottom: 10.0, right: 10.0},
                        draw_text: {text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}}
                        text: "text, watermark, cartoon"
                        draw_bg: {
                            color: (COLOR_TEXT_INPUT)
                            border_width: 1.0
                            border_color: #x00000044
                        }
                    }
                    <View> {
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
                            settings_steps = <SettingsSlider> {label = {text: "steps:"}, input = {text: "10", min:1, max: 10, step:1}}
                            settings_cfg = <SettingsSlider> {label = {text: "cfg:"}, input = {text: "2.0", min:1.0, max:8.0, step:0.01}}
                            settings_denoise = <SettingsSlider> {label = {text: "denoise:"}, input = {text: "0.85", min:0.2, max:1.0, step:0.01}}
                        }
                        <View>{ 
                            width: Fit,
                            height: Fit,
                            margin: {top: 10},
                            video_input0 = <VideoFrame>{}
                            video_input1 = <VideoFrame>{}
                        }
                        /* <View> {
                            width: Fit,
                            height: Fit,
                            margin: {top: 10},
                            flow: Down
                            settings_base_cfg = <SettingsInput> {label = {text: "base_cfg:"}, input = {text: "8.5"}}
                            settings_refiner_cfg = <SettingsInput> {label = {text: "refiner_cfg:"}, input = {text: "9.5"}}
                            settings_pos_score = <SettingsInput> {label = {text: "pos_score:"}, input = {text: "6"}}
                            settings_neg_score = <SettingsInput> {label = {text: "neg_score:"}, input = {text: "2"}}
                        }
                        <View> {
                            width: Fit,
                            height: Fit,
                            margin: {top: 10},
                            flow: Down
                            settings_base_start_step = <SettingsInput> {label = {text: "base_start_step:"}, input = {text: "0"}}
                            settings_base_end_step = <SettingsInput> {label = {text: "base_end_step:"}, input = {text: "20"}}
                            settings_refiner_start_step = <SettingsInput> {label = {text: "refiner_start_step:"}, input = {text: "20"}}
                            settings_refiner_end_step = <SettingsInput> {label = {text: "refiner_end_step:"}, input = {text: "1000"}}
                        }
                        <View> {
                            width: Fit,
                            height: Fit,
                            margin: {top: 10},
                            flow: Down
                            settings_upscale_steps = <SettingsInput> {label = {text: "upscale_steps:"}, input = {text: "31"}}
                            settings_upscale_start_step = <SettingsInput> {label = {text: "upscale_start_step:"}, input = {text: "29"}}
                            settings_upscale_end_step = <SettingsInput> {label = {text: "upscale_end_step:"}, input = {text: "1000"}}
                        }*/
                        /*
                        <View> {
                            width: Fill, height: Fit, margin: {top: 10},
                            <BarLabel> {text: "base_cfg:"}
                            base_cfg_input = <SettingsInput> {text: "1344"}
                            <BarLabel> {text: "refiner_cfg:"}
                            refiner_cfg_input = <SettingsInput> {text: "768"}
                            <BarLabel> {text: "pos_score:"}
                            positive_score_input = <SettingsInput> {text: "6.0"}
                            <BarLabel> {text: "neg_score:"}
                            negative_score_input = <SettingsInput> {text: "2.0"}
                        }
                        <View> {
                            width: Fill, height: Fit, margin: {top: 10},
                            <BarLabel> {text: "base_start_step:"}
                            base_cfg_input = <SettingsInput> {text: "1344"}
                            <BarLabel> {text: "base_end_step:"}
                            refiner_cfg_input = <SettingsInput> {text: "768"}
                            <BarLabel> {text: "refiner_start_step:"}
                            positive_score_input = <SettingsInput> {text: "6.0"}
                            <BarLabel> {text: "refiner_end_step:"}
                            negative_score_input = <SettingsInput> {text: "2.0"}
                        }*/
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
                            border_width: 1.0
                            border_color: #x00000044
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
                    ImageRow2 = <View> {
                        height: Fit,
                        width: Fill,
                        margin: {bottom: 10}
                        spacing: 20,
                        flow: Right
                        row1 = <ImageTile> {}
                        row2 = <ImageTile> {}
                    }
                    ImageRow3 = <View> {
                        height: Fit,
                        width: Fill,
                        margin: {bottom: 10}
                        spacing: 20,
                        flow: Right
                        row1 = <ImageTile> {}
                        row2 = <ImageTile> {}
                        row3 = <ImageTile> {}
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
            image1 = <Image> {
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
            image1 = <Image> {
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
