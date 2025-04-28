use makepad_widgets::*;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
        
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
    
    H2_TEXT_BOLD = <THEME_FONT_BOLD>{
        font_size: (FONT_SIZE_H2),
    }
    
    H2_TEXT_REGULAR = <THEME_FONT_REGULAR>{
        font_size: (FONT_SIZE_H2),
    }
    
    TEXT_BOLD = <THEME_FONT_BOLD>{
        font_size: 10.0,
    }
    
    TEXT_MONO = <THEME_FONT_REGULAR>{
        font_size: 10.0,
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
    
    SettingsInput = <View> {
        width: 150,
        height: Fit,
        margin: {top: 10},
        label = <Label> {
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
                    let pan = mix(vec2(0.0), (vec2(1.0) - max_scale) * 0.5, self.hover)* self.image_scale;
                    let color = self.get_color_scale_pan(scale * self.image_scale, pan + self.image_pan) + mix(vec4(0.0), vec4(0.1), self.down);
                    if color.a<0.0001{
                        color = #3
                    }
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
    
    AIChat = <View>{
        flow: Down
        <RoundedView>{
            draw_bg:{
                color: (COLOR_DOWN_2)
                border_size: 1.0
                border_color: #x00000044
            }
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
        <View>{
            height: Fit,
            padding: 5
            trim_button = <Button> {
                text: "Ask"
            }
            clear_button = <Button> {
                text: "Clear"
            }
        }
        <View>{
            height: Fit,
            chat = <TextInput> {
                height: Fit,
                width: Fill,
                margin: {top: 0.0, left: 0.0, bottom: 0.0, right: 0.0},
                margin: {bottom: 0}
                empty_text: "-"
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
    }
    
    PromptInput = <View>{
        flow:Down
        <View>{             
            height: Fit
            padding:3    
            voice_check_box = <CheckBox> {
                text: "Voice"
            }
            mute_check_box = <CheckBox> {
                text: "Mute"
            }
            random_check_box = <CheckBox> {
                text: "Seed"
                animator:{active={default:on}}
            }
            seed_input = <TextInput> {
                draw_text: {text_style: <TEXT_BOLD> {}}
                height: Fit,
                width: Fit,
                margin: {bottom: 0, left: 0}
            }
        }
        prompt_input = <TextInput> {
            width: Fill,
            height: Fill,
            margin: {top: 0.0, left: 5.0, bottom: 0.0, right: 0.0},
            text: "Beautiful woman"
            draw_text: {
                text_style:{font_size: (TEXT_BIG)}
            }
            draw_bg: {
                color: (COLOR_TEXT_INPUT)
            }
        }
                                                
    }
    
    LoraDropdown = <DropDownFlat> {
        width: Fit, 
        margin:{left:0},
        popup_menu_position: BelowInput ,
        labels:  [
            "aesthetic2-cdo-0.5",
            "flux_realism_lora",
            "Flux.1_Turbo_Detailer",
            "FantasyWizardWitchesFluxV2-000001",
            "aidmaMJ6.1-FLUX-v0.4",
            "aidmaHyperrealism-FLUX-v0.3",
        ]
    }
    
    ResDropdown = <DropDownFlat> {
        width: Fit, 
        margin:{left:0},
        popup_menu_position: BelowInput 
        labels: [
            "1920x1088 - 16:9",
            "1440x816 - 16:9",
            "1280x720 - 16:9",
            "960x544 - 16:9",
            "1088x1920 - 9:16",
            "2048x1448 - A3",
            "768x1024 - 3:4"
        ]
    }
    
    LoraSlider = <Slider> {
        width:100,text: "Influence", default: 0.0, min:0.0, max: 1.0, step:0.01, precision:2
    }
    
    MachineSettings = <RoundedView> {
        height: Fit,
        width: Fill
        draw_bg:{color:#3a}
        align: {x: 0.0, y: 0.5}
        padding: 5
        /*
        <BarLabel> {
            text: "Workflow"
        }*/
        flow: Down
        inner = <View>{
            height: Fit,
            width: Fill
            cancel_button = <Button> {
                text: "X"
                margin:{right:4}
            }
            render_check_box = <Toggle> {
                text: ">"
            }
            model = <DropDownFlat> {
                margin:{left:0},
                popup_menu_position: BelowInput 
                labels: [
                    "FluxFusion",
                    "FluxDev",
                    "Hunyuan",
                ]
            }
            page_flip = <PageFlip>{
                width: Fit,
                height: Fit,
                flow: Down
                active_page: fluxfusion
                fluxfusion = <View>{
                    width: Fit,
                    
                    resolution = <ResDropdown>{}
                    steps_slider = <Slider> {width:100,text: "Steps", default: 4.0, min:1, max: 30, step:1, precision:0}
                }
                hunyuan = <View>{
                    width: Fit,
                    height: Fit
                    resolution = <DropDownFlat> {
                        width: Fit, 
                        margin:{left:0},
                        popup_menu_position: BelowInput 
                        labels: [
                            "1280x720 - 16:9",
                            "960x544 - 16:9",
                            "720x1280 - 9:16",
                        ]
                    }
                    frames_slider = <Slider> {width:100, text: "Frames", default: 29.0, min:1, max: 127, step:4, precision:0}
                }
                fluxdev = <View>{
                    width: Fit,
                    flow: Down
                    <View>{
                        width: Fit,
                        height: Fit
                        resolution = <ResDropdown>{
                            labels: [
                                "1024x1024",
                                "768x1152",
                                "1152x768",
                                "960x544",
                            ]
                        }
                        steps_slider = <Slider> {width:100,text: "Steps", default: 35, min:20, max: 70, step:1, precision:0}
                        guidance_slider = <Slider> {width:100,text: "Guidance", default: 4, min:3.5, max: 7, step:0.01, precision:2}
                    }
                    <View>{
                        width: Fit,
                        height: Fit
                        lora1_slider = <LoraSlider>{}
                        lora1 = <LoraDropdown>{}
                    }
                    <View>{
                        width: Fit,
                        height: Fit
                        lora2_slider = <LoraSlider>{}
                        lora2 = <LoraDropdown>{}
                    }
                    <View>{
                        width: Fit,
                        height: Fit
                        lora3_slider = <LoraSlider>{}
                        lora3 = <LoraDropdown>{}
                    }
                }
                
            }
            reconnect_button = <Button> {
                text: "<>"
                margin:{right:4}
            }
                 
            
        }
        <RoundedView>{
            width:Fill
            height:Fit
            draw_bg:{color:#3}
            padding:3
            
            progress = <Label> {
                text: "Ready:"
                draw_text: {
                    text_style: {font_size: 9}
                }
            }
            last_set = <Label> {
                margin:{left:3}
                text: ""
                draw_text: {
                    text_style: {font_size: 9}
                }
            }
        }
    }
    pub AppUI = <View> {
                            
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
                    <Splitter>{
                        align:FromA(19)
                        a:<AIChat>{}
                        b:<Splitter>{
                            a:<PromptInput>{}
                            b:<View>{
                                height: Fill,
                                width: Fill
                                flow:Down
                                padding:10
                                spacing:3
                                m1 = <MachineSettings>{inner = {render_check_box={text:"1"}}}
                                m2 = <MachineSettings>{inner = {render_check_box={text:"2"}}}
                                m3 = <MachineSettings>{inner = {render_check_box={text:"3"}}}
                                m4 = <MachineSettings>{inner = {render_check_box={text:"4"}}}
                                m7 = <MachineSettings>{inner = {render_check_box={text:"7"}}}
                                m8 = <MachineSettings>{inner = {render_check_box={text:"8"}}}
                            }
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
                        empty_text: "Search"
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
                /*<RoundedView>{
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
                }*/
                
            }
        }
                            
        big_image = <RectView> {
            visible: false,
            draw_bg: {draw_depth: 10.0}
            draw_bg: {color: #0}
            height: All,
            width: All,
            abs_pos: vec2(0.0, 0.0)
            image1 = <ImageBlend> {
            }
        }
    }
    
    pub AppWindow = <View>{
        
        second_image = <RectView> {
            draw_bg: {color: #0}
            height: Fill,
            width: Fill
            flow: Overlay,
            align: {x: 0.5, y: 0.5}
            image1 = <ImageBlend> {
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
                        text_style: <TEXT_BOLD> {font_size: 15}
                                                                                                    
                        color: #c
                    },
                    text: "HELLO WORLD"
                }
            }
        }
    }
}
 