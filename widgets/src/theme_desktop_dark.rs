use crate::makepad_platform::*;

live_design! {
    import makepad_draw::shader::std::*;
    import crate::base::*;

    THEME_FONT_LABEL = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
        }
    }
    
    THEME_FONT_BOLD = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf")
        }
    }
    
    THEME_FONT_ITALIC = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/IBMPlexSans-Italic.ttf")
        }
    }
    
    THEME_FONT_BOLD_ITALIC = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/IBMPlexSans-BoldItalic.ttf")
        }
    }
    
    THEME_FONT_DATA = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
        }
    }

    THEME_FONT_META = {
        font_size: 9.4,
        top_drop: 1.2,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
        }
    }

    THEME_FONT_CODE = {
        font: {
            path: dep("crate://self/resources/LiberationMono-Regular.ttf")
        }
        brightness: 1.1
        font_size: 9.0
        line_spacing: 2.0
        top_drop: 1.3
    }

    const THEME_DATA_ITEM_HEIGHT = 23.0
    const THEME_DATA_ICON_WIDTH = 16.0
    const THEME_DATA_ICON_HEIGHT = 24.0
    // ABSOLUTE DEFS

    const THEME_BRIGHTNESS = #x40
    const THEME_COLOR_HIGHLIGHT = #42
    const THEME_COLOR_HIGH = #C00
    const THEME_COLOR_MID = #FA0
    const THEME_COLOR_LOW = #8A0

    // RELATIVE =DEFS
    //    42, =78, 117
    const THEME_COLOR_WHITE = #FFF
    const THEME_COLOR_UP_80 = #FFFFFFCC
    const THEME_COLOR_UP_50 = #FFFFFF80
    const THEME_COLOR_UP_25 = #FFFFFF40
    const THEME_COLOR_UP_15 = #FFFFFF26
    const THEME_COLOR_UP_10 = #FFFFFF1A
    const THEME_COLOR_UP_4 = #FFFFFF0A
    const THEME_COLOR_DOWN_7 = #00000013
    const THEME_COLOR_DOWN_10 = #00000030
    const THEME_COLOR_DOWN_20 = #00000040
    const THEME_COLOR_DOWN_50 = #00000080
    const THEME_COLOR_BLACK = #000

    // CORE BACKGROUND COLORS

    const THEME_COLOR_BG_APP = (THEME_BRIGHTNESS)

    const THEME_COLOR_BG_HEADER = (blend(
        THEME_COLOR_BG_APP,
        THEME_COLOR_DOWN_10
    ))

    const THEME_COLOR_CLEAR = (THEME_COLOR_BG_APP)

    const THEME_COLOR_BG_EDITOR = (blend(
        THEME_COLOR_BG_HEADER,
        THEME_COLOR_DOWN_10
    ))

    const THEME_COLOR_BG_ODD = (blend(
        THEME_COLOR_BG_EDITOR,
        THEME_COLOR_DOWN_7
    ))

    const THEME_COLOR_BG_SELECTED = (THEME_COLOR_HIGHLIGHT)

    const THEME_COLOR_BG_UNFOCUSSED = (blend(
        THEME_COLOR_BG_EDITOR,
        THEME_COLOR_UP_10
    ))

    const THEME_COLOR_EDITOR_SELECTED = (THEME_COLOR_BG_SELECTED)
    const THEME_COLOR_EDITOR_SELECTED_UNFOCUSSED = (THEME_COLOR_BG_SELECTED_UNFOCUSSED)

    const THEME_COLOR_BG_CURSOR = (blend(
        THEME_COLOR_BG_EDITOR,
        THEME_COLOR_UP_4
    ))

    const THEME_COLOR_FG_CURSOR = (blend(
        THEME_COLOR_BG_EDITOR,
        THEME_COLOR_UP_50
    ))

    // TEXT / ICON COLORS

    const THEME_COLOR_TEXT_DEFAULT = (THEME_COLOR_UP_50)
    const THEME_COLOR_TEXT_HOVER = (THEME_COLOR_UP_80)
    const THEME_COLOR_TEXT_META = (THEME_COLOR_UP_25)
    const THEME_COLOR_TEXT_SELECTED = (THEME_COLOR_UP_80)

    // SPLITTER AND SCROLLBAR

    const THEME_COLOR_SCROLL_BAR_DEFAULT = (THEME_COLOR_UP_10)

    const THEME_COLOR_CONTROL_HOVER = (blend(
        THEME_COLOR_BG_HEADER,
        THEME_COLOR_UP_50
    ))

    const THEME_COLOR_CONTROL_PRESSED = (blend(
        THEME_COLOR_BG_HEADER,
        THEME_COLOR_UP_25
    ))

    // ICON COLORS

    const THEME_COLOR_ICON_WAIT = (THEME_COLOR_LOW),
    const THEME_COLOR_ERROR = (THEME_COLOR_HIGH),
    const THEME_COLOR_WARNING = (THEME_COLOR_MID),
    const THEME_COLOR_ICON_PANIC = (THEME_COLOR_HIGH)
    const THEME_COLOR_DRAG_QUAD = (THEME_COLOR_UP_50)
    const THEME_COLOR_PANIC = #f0f

    const THEME_TAB_HEIGHT = 26.0,
    const THEME_SPLITTER_HORIZONTAL = 16.0,
    const THEME_SPLITTER_MIN_HORIZONTAL = (THEME_TAB_HEIGHT),
    const THEME_SPLITTER_MAX_HORIZONTAL = (THEME_TAB_HEIGHT + THEME_SPLITTER_SIZE),
    const THEME_SPLITTER_MIN_VERTICAL = (THEME_SPLITTER_HORIZONTAL),
    const THEME_SPLITTER_MAX_VERTICAL = (THEME_SPLITTER_HORIZONTAL + THEME_SPLITTER_SIZE),
    const THEME_SPLITTER_SIZE = 5.0
    
    Html = <HtmlBase>{
        font_size: 12,
        flow: RightWrap,
        width:Fill,
        height:Fit,
        padding: 5,
        line_spacing: 10,
        
        draw_normal: {text_style:<THEME_FONT_LABEL>{}}
        draw_italic: {text_style:<THEME_FONT_ITALIC>{}}
        draw_bold: {text_style:<THEME_FONT_BOLD>{}}
        draw_bold_italic: {text_style:<THEME_FONT_BOLD_ITALIC>{}}
        draw_fixed: {text_style:<THEME_FONT_CODE>{}}
        
        code_layout:{flow: RightWrap, padding:{left:10,top:10,right:10,bottom:10}},
        code_walk:{height:Fit,width:Fill}
        
        quote_layout:{flow: RightWrap, padding:{left:15,top:10,right:10,bottom:10}},
        quote_walk:{height:Fit,width:Fill}
        
        list_item_layout:{flow: RightWrap, padding:{left:0,top:0,right:10,bottom:0}},
        list_item_walk:{height:Fit,width:Fill}
          
        sep_walk:{height:4, width: Fill},
        
        draw_block:{
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                match self.block_type {
                    FlowBlockType::Quote => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x-2.,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#6)
                        sdf.box(
                            4.,
                            3.,
                            4.,
                            self.rect_size.y-6, 
                            1.
                        );
                        sdf.fill(#8);
                        return sdf.result;
                    }
                    FlowBlockType::Sep => {
                        sdf.box(
                            0.,
                            1.,
                            self.rect_size.x-1,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#8);
                        return sdf.result;
                    }
                    FlowBlockType::Code => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x-2.,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#7);
                        return sdf.result;
                    }
                    FlowBlockType::Underline => {
                        sdf.box(
                            0.,
                            self.rect_size.y-2,
                            self.rect_size.x,
                            1.5,
                            0.5
                        );
                        sdf.fill(#f);
                        return sdf.result;
                    }
                    FlowBlockType::Strikethrough => {
                        sdf.box(
                            0.,
                            self.rect_size.y*0.5,
                            self.rect_size.x,
                            1.5,
                            0.5
                        );
                        sdf.fill(#f);
                        return sdf.result;
                    }
                }
                return #f00
            } 
        }
    }
    
    Markdown = <MarkdownBase>{
        font_size: 12,
        flow: RightWrap,
        width:Fill,
        height:Fit,
        padding: 5,
        line_spacing: 10,
        
        draw_normal: {text_style:<THEME_FONT_LABEL>{}}
        draw_italic: {text_style:<THEME_FONT_ITALIC>{}}
        draw_bold: {text_style:<THEME_FONT_BOLD>{}}
        draw_bold_italic: {text_style:<THEME_FONT_BOLD_ITALIC>{}}
        draw_fixed: {text_style:<THEME_FONT_CODE>{}}
                
        code_layout:{flow: RightWrap,align:{x:0.0,y:0.0}, padding:{left:10,top:10,right:10,bottom:10}},
        code_walk:{height:Fit,width:Fill}
        
        inline_code_layout:{flow: RightWrap,  padding:{left:3,top:2,right:3,bottom:2}},
        inline_code_walk:{height:Fit,width:Fit,margin:{top:-4}} 
                        
        quote_layout:{flow: RightWrap, padding:{left:15,top:10,right:10,bottom:10}},
        quote_walk:{height:Fit,width:Fill}
                
        list_item_layout:{flow: RightWrap, line_spacing: 10 padding:{left:15,top:0,right:10,bottom:0}},
        list_item_walk:{margin:{top:0},height:Fit,width:Fill}
                
        sep_walk:{height:4, width: Fill},
                
        draw_block:{
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                match self.block_type {
                    FlowBlockType::Quote => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x-2.,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#6)
                        sdf.box(
                            4.,
                            3.,
                            4.,
                            self.rect_size.y-6, 
                            1.
                        );
                        sdf.fill(#8);
                        return sdf.result;
                    }
                    FlowBlockType::Sep => {
                        sdf.box(
                            0.,
                            1.,
                            self.rect_size.x-1,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#6);
                        return sdf.result;
                    }
                    FlowBlockType::Code => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x-2.,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#7);
                        return sdf.result;
                    }
                    FlowBlockType::InlineCode => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x-2.,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(#7);
                        return sdf.result;
                    }
                }
                return #f00
            } 
        }
    }
    
    ScrollBar = <ScrollBarBase> {
        bar_size: 10.0,
        bar_side_margin: 3.0
        min_handle_size: 30.0
        draw_bar: {
            //draw_depth: 5.0
            uniform border_radius: 1.5
            instance bar_width: 6.0
            instance pressed: 0.0
            instance hover: 0.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                if self.is_vertical > 0.5 {
                    sdf.box(
                        1.,
                        self.rect_size.y * self.norm_scroll,
                        self.bar_width,
                        self.rect_size.y * self.norm_handle,
                        self.border_radius
                    );
                }
                else {
                    sdf.box(
                        self.rect_size.x * self.norm_scroll,
                        1.,
                        self.rect_size.x * self.norm_handle,
                        self.bar_width,
                        self.border_radius
                    );
                }
                return sdf.fill(mix(
                    THEME_COLOR_SCROLL_BAR_DEFAULT,
                    mix(
                        THEME_COLOR_CONTROL_HOVER,
                        THEME_COLOR_CONTROL_PRESSED,
                        self.pressed
                    ),
                    self.hover
                ));
            }
        }
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bar: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    cursor: Default,
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bar: {
                            pressed: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }

                pressed = {
                    cursor: Default,
                    from: {all: Snap}
                    apply: {
                        draw_bar: {
                            pressed: 1.0,
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }

    ScrollBars = <ScrollBarsBase> {
        show_scroll_x: true,
        show_scroll_y: true,
        scroll_bar_x: <ScrollBar> {}
        scroll_bar_y: <ScrollBar> {}
    }


    Label = <LabelBase> {
        width: Fit
        height: Fit
        draw_text: {
            color: #8,
            text_style: <THEME_FONT_LABEL>{}
            wrap: Word
        }
    }

    // Button



    Button = <ButtonBase> {
        width: Fit,
        height: Fit,
        margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0}
        align: {x: 0.5, y: 0.5}
        padding: {left: 14.0, top: 10.0, right: 14.0, bottom: 10.0}

        label_walk: {
            width: Fit,
            height: Fit
        }

        draw_text: {
            instance hover: 0.0
            instance pressed: 0.0
            text_style: <THEME_FONT_LABEL>{
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }

        draw_icon: {
            instance hover: 0.0
            instance pressed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: 3.0
            instance bodytop: #53
            instance bodybottom: #5c
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), #33, self.pressed);
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
                    mix(mix(#2f, #1f, self.hover), #0000, self.pressed),
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

        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
    }


    // Checkbox



     CheckBox = <CheckBoxBase> {

        width: Fit,
        height: Fit

        label_walk: {
            margin: {left: 20.0, top: 8, bottom: 8, right: 10}
            width: Fit,
            height: Fit,
        }

        label_align: {
            y: 0.0
        }

        draw_check: {
            uniform size: 7.0;
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.check_type {
                    CheckType::Check => {
                        let left = 3;
                        let sz = self.size;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.box(left, c.y - sz, sz * 2.0, sz * 2.0, 3.0); // rounding = 3rd value
                        sdf.fill_keep(mix(mix(#x00000077, #x00000044, pow(self.pos.y, 1.)), mix(#x000000AA, #x00000066, pow(self.pos.y, 1.0)), self.hover))
                        sdf.stroke(#x888, 1.0) // outline
                        let szs = sz * 0.5;
                        let dx = 1.0;
                        sdf.move_to(left + 4.0, c.y);
                        sdf.line_to(c.x, c.y + szs);
                        sdf.line_to(c.x + szs, c.y - szs);
                        sdf.stroke(mix(#fff0, #f, self.selected), 1.25);
                    }
                    CheckType::Radio => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill(#2);
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(mix(#fff0, #f, self.selected));
                    }
                    CheckType::Toggle => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);
                        sdf.fill(#2);
                        let isz = sz * 0.5;
                        sdf.circle(left + sz + self.selected * sz, c.y, isz);
                        sdf.circle(left + sz + self.selected * sz, c.y, 0.5 * isz);
                        sdf.subtract();
                        sdf.circle(left + sz + self.selected * sz, c.y, isz);
                        sdf.blend(self.selected)
                        sdf.fill(#f);
                    }
                    CheckType::None => {
                        return #0000
                    }
                }
                return sdf.result
            }
        }
        draw_text: {
            color: #9,
            instance focus: 0.0
            instance selected: 0.0
            instance hover: 0.0
            text_style: {
                font: {
                    //path: d"resources/IBMPlexSans-SemiBold.ttf"
                }
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #fff6,
                        #fff6,
                        self.hover
                    ),
                    #fff6,
                    self.selected
                )
            }
        }

        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #f,
                    self.selected
                )
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_check: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 0.0},
                        draw_text: {selected: 0.0},
                        draw_icon: {selected: 0.0},
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_check: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0},
                    }
                }
            }
        }
    }


    DesktopButton = <DesktopButtonBase> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.aa *= 3.0;
                let sz = 4.5;
                let c = self.rect_size * vec2(0.5, 0.5);

                // WindowsMin
                match self.button_type {
                    DesktopButtonType::WindowsMin => {
                        sdf.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                        sdf.move_to(c.x - sz, c.y);
                        sdf.line_to(c.x + sz, c.y);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsMax => {
                        sdf.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsMaxToggled => {
                        let clear = mix(#3, mix(#6, #9, self.pressed), self.hover);
                        sdf.clear(clear);
                        let sz = 3.5;
                        sdf.rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        sdf.rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz);
                        sdf.fill_keep(clear);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsClose => {
                        sdf.clear(mix(#3, mix(#e00, #c00, self.pressed), self.hover));
                        sdf.move_to(c.x - sz, c.y - sz);
                        sdf.line_to(c.x + sz, c.y + sz);
                        sdf.move_to(c.x - sz, c.y + sz);
                        sdf.line_to(c.x + sz, c.y - sz);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::XRMode => {
                        sdf.clear(mix(#3, mix(#0aa, #077, self.pressed), self.hover));
                        let w = 12.;
                        let h = 8.;
                        sdf.box(c.x - w, c.y - h, 2. * w, 2. * h, 2.);
                        // subtract 2 eyes
                        sdf.circle(c.x - 5.5, c.y, 3.5);
                        sdf.subtract();
                        sdf.circle(c.x + 5.5, c.y, 3.5);
                        sdf.subtract();
                        sdf.circle(c.x, c.y + h - 0.75, 2.5);
                        sdf.subtract();
                        sdf.fill(#8);

                        return sdf.result;
                    }
                    DesktopButtonType::Fullscreen => {
                        sz = 8.;
                        sdf.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        sdf.rect(c.x - sz + 1.5, c.y - sz + 1.5, 2. * (sz - 1.5), 2. * (sz - 1.5));
                        sdf.subtract();
                        sdf.rect(c.x - sz + 4., c.y - sz - 2., 2. * (sz - 4.), 2. * (sz + 2.));
                        sdf.subtract();
                        sdf.rect(c.x - sz - 2., c.y - sz + 4., 2. * (sz + 2.), 2. * (sz - 4.));
                        sdf.subtract();
                        sdf.fill(#f); //, 0.5 + 0.5 * dpi_dilate);

                        return sdf.result;
                    }
                }
                return #f00;
            }
        }
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_down: Snap
                    }
                    apply: {
                        draw_bg: {
                            pressed: 0.0,
                            hover: 1.0,
                        }
                    }
                }

                pressed = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {
                            pressed: 1.0,
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }

    NavControl = <NavControlBase> {
        draw_focus: {
            fn pixel(self) -> vec4 {
                return #000f
            }
        }
        draw_text: {
            text_style: {
                font_size: 6
            },
            color: #a
        }
    }

    WindowMenu = <WindowMenuBase>{
        height: 0,
        width: 0
    }

    Window = <WindowBase> {
        pass: {clear_color: (THEME_COLOR_CLEAR)}
        flow: Down
        nav_control: <NavControl> {}
        caption_bar = <SolidView> {
            visible: false,

            flow: Right

            draw_bg: {color: (THEME_COLOR_BG_APP)}
            height: 27
            caption_label = <View> {
                width: Fill,
                height: Fill
                align: {x: 0.5, y: 0.5},
                label = <Label> {text: "Makepad", margin: {left: 100}}
            }
            windows_buttons = <View> {
                visible: false,
                width: Fit,
                height: Fit
                min = <DesktopButton> {draw_bg: {button_type: WindowsMin}}
                max = <DesktopButton> {draw_bg: {button_type: WindowsMax}}
                close = <DesktopButton> {draw_bg: {button_type: WindowsClose}}
            }
            web_fullscreen = <View> {
                visible: false,
                width: Fit,
                height: Fit
                fullscreen = <DesktopButton> {draw_bg: {button_type: Fullscreen}}
            }
            web_xr = <View> {
                visible: false,
                width: Fit,
                height: Fit
                xr_on = <DesktopButton> {draw_bg: {button_type: XRMode}}
            }
        }
        
        window_menu = <WindowMenu>{
            main = Main{items:[app]}
            app = Sub{name:"Makepad",items:[quit]}
            quit = Item{
                name:"Quit",
                shift: false,
                key: KeyQ,
                enabled: true
            }
        }
        body = <KeyboardView>{
            keyboard_min_shift: 30,
            width: Fill,
            height: Fill
        }

        cursor: Default
        mouse_cursor_size: vec2(20, 20),
        draw_cursor: {
            instance border_width: 1.5
            instance color: #000
            instance border_color: #fff

            fn get_color(self) -> vec4 {
                return self.color
            }

            fn get_border_color(self) -> vec4 {
                return self.border_color
            }

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.move_to(1.0, 1.0);
                sdf.line_to(self.rect_size.x - 1.0, self.rect_size.y * 0.5)
                sdf.line_to(self.rect_size.x * 0.5, self.rect_size.y - 1.0)
                sdf.close_path();
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result
            }
        }
        window: {
            inner_size: vec2(1024, 768)
        }
    }


    // Dock


    Splitter = <SplitterBase> {
        draw_splitter: {
            uniform border_radius: 1.0
            uniform splitter_pad: 1.0
            uniform splitter_grabber: 110.0

            instance pressed: 0.0
            instance hover: 0.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(THEME_COLOR_BG_APP);

                if self.is_vertical > 0.5 {
                    sdf.box(
                        self.splitter_pad,
                        self.rect_size.y * 0.5 - self.splitter_grabber * 0.5,
                        self.rect_size.x - 2.0 * self.splitter_pad,
                        self.splitter_grabber,
                        self.border_radius
                    );
                }
                else {
                    sdf.box(
                        self.rect_size.x * 0.5 - self.splitter_grabber * 0.5,
                        self.splitter_pad,
                        self.splitter_grabber,
                        self.rect_size.y - 2.0 * self.splitter_pad,
                        self.border_radius
                    );
                }
                return sdf.fill_keep(mix(
                    THEME_COLOR_BG_APP,
                    mix(
                        THEME_COLOR_CONTROL_HOVER,
                        THEME_COLOR_CONTROL_PRESSED,
                        self.pressed
                    ),
                    self.hover
                ));
            }
        }
        split_bar_size: (THEME_SPLITTER_SIZE)
        min_horizontal: (THEME_SPLITTER_MIN_HORIZONTAL)
        max_horizontal: (THEME_SPLITTER_MAX_HORIZONTAL)
        min_vertical: (THEME_SPLITTER_MIN_VERTICAL)
        max_vertical: (THEME_SPLITTER_MAX_VERTICAL)

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_splitter: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_down: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_splitter: {
                            pressed: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }

                pressed = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_splitter: {
                            pressed: [{time: 0.0, value: 1.0}],
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }


    TabCloseButton = <TabCloseButtonBase> {
        height: 10.0,
        width: 10.0,
        margin: {right: 5},
        draw_button: {

            instance hover: float;
            instance selected: float;

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let mid = self.rect_size / 2.0;
                let size = (self.hover * 0.25 + 0.5) * 0.25 * length(self.rect_size);
                let min = mid - vec2(size);
                let max = mid + vec2(size);
                sdf.move_to(min.x, min.y);
                sdf.line_to(max.x, max.y);
                sdf.move_to(min.x, max.y);
                sdf.line_to(max.x, min.y);
                return sdf.stroke(mix(
                    THEME_COLOR_TEXT_DEFAULT,
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                ), 1.0);
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_button: {hover: 0.0}
                    }
                }

                on = {
                    cursor: Hand,
                    from: {all: Snap}
                    apply: {
                        draw_button: {hover: 1.0}
                    }
                }
            }
        }
    }

    Tab = <TabBase> {
        width: Fit,
        height: Fill, //Fixed((THEME_TAB_HEIGHT)),

        align: {x: 0.0, y: 0.5}
        padding: {
            left: 10.0,
            top: 2.0,
            right: 15.0,
            bottom: 0.0,
        },

        close_button: <TabCloseButton> {}
        draw_name: {
            text_style: <THEME_FONT_LABEL> {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                )
            }
        }

        draw_bg: {
            instance hover: float
            instance selected: float

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return mix(
                    mix(
                        THEME_COLOR_BG_HEADER,
                        THEME_COLOR_BG_EDITOR,
                        self.selected
                    ),
                    #f,
                    0.0 //mix(self.hover * 0.05, self.hover * -0.025, self.selected)
                );
                /*sdf.clear(color)
                sdf.move_to(0.0, 0.0)
                sdf.line_to(0.0, self.rect_size.y)
                sdf.move_to(self.rect_size.x, 0.0)
                sdf.line_to(self.rect_size.x, self.rect_size.y)
                return sdf.stroke(BORDER_COLOR, BORDER_WIDTH)*/
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_name: {hover: 0.0}
                    }
                }

                on = {
                    cursor: Hand,
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: [{time: 0.0, value: 1.0}]}
                        draw_name: {hover: [{time: 0.0, value: 1.0}]}
                    }
                }
            }

            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.3}}
                    apply: {
                        close_button: {draw_button: {selected: 0.0}}
                        draw_bg: {selected: 0.0}
                        draw_name: {selected: 0.0}
                    }
                }

                on = {
                    from: {all: Snap}
                    apply: {
                        close_button: {draw_button: {selected: 1.0}}
                        draw_bg: {selected: 1.0}
                        draw_name: {selected: 1.0}
                    }
                }
            }
        }
    }

    TabBar = <TabBarBase> {
        tab: <Tab> {}
        draw_drag: {
            draw_depth: 10
            color: #c
        }
        draw_fill: {
            color: (THEME_COLOR_BG_HEADER)
        }

        width: Fill
        height: Fixed((THEME_TAB_HEIGHT))

        scroll_bars: <ScrollBars> {
            show_scroll_x: true
            show_scroll_y: false
            scroll_bar_x: {
                draw_bar: {bar_width: 3.0}
                bar_size: 4
                use_vertical_finger_scroll: true
            }
        }
    }


    const BORDER_SIZE: 6.0
    Dock = <DockBase> {
        round_corner: {
            draw_depth: 6.0
            border_radius: 10.0
            fn pixel(self) -> vec4 {

                let pos = vec2(
                    mix(self.pos.x, 1.0 - self.pos.x, self.flip.x),
                    mix(self.pos.y, 1.0 - self.pos.y, self.flip.y)
                )

                let sdf = Sdf2d::viewport(pos * self.rect_size);
                sdf.rect(-10., -10., self.rect_size.x * 2.0, self.rect_size.y * 2.0);
                sdf.box(
                    0.25,
                    0.25,
                    self.rect_size.x * 2.0,
                    self.rect_size.y * 2.0,
                    4.0
                );

                sdf.subtract()
                return sdf.fill(THEME_COLOR_BG_APP);
            }
        }
        border_size: (BORDER_SIZE)

        flow: Down
        padding: {left: (BORDER_SIZE), top: (0), right: (BORDER_SIZE), bottom: (BORDER_SIZE)}
        padding_fill: {color: (THEME_COLOR_BG_APP)}
        drag_quad: {
            draw_depth: 10.0
            color: (THEME_COLOR_DRAG_QUAD)
        }
        tab_bar: <TabBar> {}
        splitter: <Splitter> {}
    }




    PopupMenuItem = <PopupMenuItemBase> {

        align: {y: 0.5}
        padding: {left: 15, top: 5, bottom: 5},
        width: Fill,
        height: Fit

        draw_name: {
            text_style: <THEME_FONT_LABEL> {}
            instance selected: 0.0
            instance hover: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                )
            }
        }

        draw_bg: {
            instance selected: 0.0
            instance hover: 0.0
            instance color: #0
            instance color_selected: #4

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                sdf.clear(mix(
                    self.color,
                    self.color_selected,
                    // THEME_COLOR_BG_EDITOR,
                    // THEME_COLOR_BG_SELECTED,
                    self.hover
                ))

                //
                // we have 3 points, and need to rotate around its center
                let sz = 3.;
                let dx = 2.0;
                let c = vec2(8.0, 0.5 * self.rect_size.y);
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx);
                sdf.line_to(c.x, c.y + sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.stroke(mix(#fff0, #f, self.selected), 1.0);

                return sdf.result;
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_name: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_name: {hover: 1.0}
                    }
                }
            }

            select = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0,}
                        draw_name: {selected: 0.0,}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0,}
                        draw_name: {selected: 1.0,}
                    }
                }
            }
        }
        indent_width: 10.0
    }

    PopupMenu = <PopupMenuBase> {
        menu_item: <PopupMenuItem> {}

        flow: Down,
        padding: 5


        width: 100,
        height: Fit

        draw_bg: {
            instance color: #0
            instance border_width: 0.0,
            instance border_color: #0000,
            instance inset: vec4(0.0, 0.0, 0.0, 0.0),
            instance radius: 4.0

            fn get_color(self) -> vec4 {
                return self.color
            }

            fn get_border_color(self) -> vec4 {
                return self.border_color
            }

            fn pixel(self) -> vec4 {

                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.blur = 20.0;
                sdf.box(
                    self.inset.x + self.border_width,
                    self.inset.y + self.border_width,
                    self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                    self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                sdf.fill_keep(self.get_color())
                return sdf.result;
            }
        }
    }



    DropDown = <DropDownBase> {

       

        draw_text: {
            text_style: <THEME_FONT_DATA> {}

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            #9,
                            #b,
                            self.focus
                        ),
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            instance focus: 0.0,
            uniform border_radius: 0.5

            fn get_bg(self, inout sdf: Sdf2d) {
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(mix(#2, #3, self.hover));
            }

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                self.get_bg(sdf);
                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;

                sdf.move_to(c.x - sz, c.y - sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.line_to(c.x, c.y + sz * 0.75);
                sdf.close_path();

                sdf.fill(mix(#8, #c, self.hover));

                return sdf.result
            }
        }

        width: Fill,
        height: Fit,
        margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0}
        align: {x: 0., y: 0.}
        padding: {left: 5.0, top: 5.0, right: 4.0, bottom: 5.0}

        popup_menu: <PopupMenu> {}

        popup_shift: vec2(-6.0, 4.0)

        selected_item: 0
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 0.0},
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0},
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    FileTreeNode = <FileTreeNodeBase> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    mix(
                        THEME_COLOR_BG_UNFOCUSSED,
                        THEME_COLOR_BG_SELECTED,
                        self.focussed
                    ),
                    self.selected
                );
            }
        }

        draw_icon: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let w = self.rect_size.x;
                let h = self.rect_size.y;
                sdf.box(0. * w, 0.35 * h, 0.87 * w, 0.39 * h, 0.75);
                sdf.box(0. * w, 0.28 * h, 0.5 * w, 0.3 * h, 1.);
                sdf.union();
                return sdf.fill(mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT * self.scale,
                        THEME_COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                ));
            }
        }

        draw_name: {
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT * self.scale,
                        THEME_COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                )
            }

            text_style: <THEME_FONT_DATA> {
                top_drop: 1.2,
            }
        }

        align: {y: 0.5}
        padding: {left: 5.0, bottom: 0,},

        icon_walk: {
            width: Fixed((THEME_DATA_ICON_WIDTH - 2)),
            height: Fixed((THEME_DATA_ICON_HEIGHT)),
            margin: {
                left: 0
                top: 0
                right: 2
                bottom: 0
            },
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        hover: 0.0
                        draw_bg: {hover: 0.0}
                        draw_name: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }

                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        hover: 1.0
                        draw_bg: {hover: 1.0}
                        draw_name: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    },
                }
            }

            focus = {
                default: on
                on = {
                    from: {all: Snap}
                    apply: {focussed: 1.0}
                }

                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {focussed: 0.0}
                }
            }

            select = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        selected: 0.0
                        draw_bg: {selected: 0.0}
                        draw_name: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        selected: 1.0
                        draw_bg: {selected: 1.0}
                        draw_name: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                    }
                }

            }

            open = {
                default: off
                off = {
                    //from: {all: Exp {speed1: 0.80, speed2: 0.97}}
                    //duration: 0.2
                    redraw: true

                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.80, d2: 0.97}

                    //ease: Ease::OutExp
                    apply: {
                        opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                        draw_bg: {opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                        draw_name: {opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                        draw_icon: {opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                    }
                }

                on = {
                    //from: {all: Exp {speed1: 0.82, speed2: 0.95}}

                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.82, d2: 0.95}

                    //from: {all: Exp {speed1: 0.82, speed2: 0.95}}
                    redraw: true
                    apply: {
                        opened: 1.0
                        draw_bg: {opened: 1.0}
                        draw_name: {opened: 1.0}
                        draw_icon: {opened: 1.0}
                    }
                }
            }
        }
        is_folder: false,
        indent_width: 10.0
        min_drag_distance: 10.0
    }

    FileTree = <FileTreeBase> {
        scroll_bars: <ScrollBars>{}
        node_height: (THEME_DATA_ITEM_HEIGHT),
        file_node: <FileTreeNode> {
            is_folder: false,
            draw_bg: {is_folder: 0.0}
            draw_name: {is_folder: 0.0}
        }
        folder_node: <FileTreeNode> {
            is_folder: true,
            draw_bg: {is_folder: 1.0}
            draw_name: {is_folder: 1.0}
        }
        filler: {
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    mix(
                        THEME_COLOR_BG_UNFOCUSSED,
                        THEME_COLOR_BG_SELECTED,
                        self.focussed
                    ),
                    self.selected
                );
            }
        }
        flow: Down,
        clip_x: true,
        clip_y: true
        scroll_bars: {}
    }

    FoldButton = <FoldButtonBase> {
        draw_bg: {
            instance open: 0.0
            instance hover: 0.0

            uniform fade: 1.0

            fn pixel(self) -> vec4 {

                let sz = 3.;
                let c = vec2(5.0, 0.5 * self.rect_size.y);
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(vec4(0.));
                // we have 3 points, and need to rotate around its center
                sdf.rotate(self.open * 0.5 * PI + 0.5 * PI, c.x, c.y);
                sdf.move_to(c.x - sz, c.y + sz);
                sdf.line_to(c.x, c.y - sz);
                sdf.line_to(c.x + sz, c.y + sz);
                sdf.close_path();
                sdf.fill(mix(#a, #f, self.hover));
                return sdf.result * self.fade;
            }
        }

        abs_size: vec2(32, 12)
        abs_offset: vec2(4., 0.)
        width: 12,
        height: 12,

        animator: {

            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {hover: 0.0}}
                }

                on = {
                    from: {all: Snap}
                    apply: {draw_bg: {hover: 1.0}}
                }
            }

            open = {
                default: yes
                no = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        draw_bg: {open: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                    }
                }
                yes = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        draw_bg: {open: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
                    }
                }
            }
        }
    }

    FoldHeader = <FoldHeaderBase> {
        width: Fill,
        height: Fit
        body_walk: {
            width: Fill,
            height: Fit
        }

        flow: Down,

        animator: {
            open = {
                default: on
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]
                    }
                }
            }
        }
    }


    LinkLabel = <LinkLabelBase> {
        width: Fit,
        height: Fit,
        margin: 0
        padding: 0
        align: {x: 0., y: 0.}

        label_walk: {
            width: Fit,
            height: Fit
        }

        draw_icon: {
            instance hover: 0.0
            instance pressed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }

        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }


        draw_bg: {
            instance pressed: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                return sdf.stroke(mix(
                    THEME_COLOR_TEXT_DEFAULT,
                    THEME_COLOR_TEXT_META,
                    self.pressed
                ), mix(0.0, 0.8, self.hover));
            }
        }

        draw_text: {
            wrap: Word
            instance pressed: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_LABEL>{}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_META,
                        THEME_COLOR_TEXT_DEFAULT,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_META,
                    self.pressed
                )
            }
        }

    }


    RadioButton = <RadioButtonBase> {

        draw_radio: {

            uniform size: 7.0;
            uniform color_active: #00000000
            uniform color_inactive: #x99EEFF

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.radio_type {
                    RadioType::Round => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill(#2);
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(mix(#fff0, #f, self.selected));
                    }
                    RadioType::Tab => {
                        let sz = self.size;
                        let left = 0.;
                        let c = vec2(left, self.rect_size.y);
                        sdf.rect(
                            -1.,
                            0.,
                            self.rect_size.x + 2.0,
                            self.rect_size.y
                        );
                        sdf.fill(mix(self.color_inactive, self.color_active, self.selected));
                    }
                }
                return sdf.result
            }
        }
        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            instance selected: 0.0

            uniform color_unselected: #x00000088
            uniform color_unselected_hover: #x000000CC
            uniform color_selected: #xFFFFFF66

            color: #9
            text_style: {
                font: {
                    //path: d"resources/ibmplexsans-semibold.ttf"
                }
                font_size: 9.5
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_selected,
                    self.selected
                )
            }
        }

        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.selected
                )
            }
        }

        width: Fit,
        height: Fit

        label_walk: {
            margin: {top: 4.5, bottom: 4.5, left: 8, right: 8}
            width: Fit,
            height: Fit,
        }

        label_align: {
            y: 0.0
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_radio: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                        draw_text: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                    }
                }
            }
        }
    }
    
    

    PortalList = <PortalListBase> {
        width: Fill
        height: Fill
        capture_overload: true
        scroll_bar: <ScrollBar> {}
        flow: Down
    }

    FlatList = <FlatListBase> {
        width: Fill
        height: Fill
        capture_overload: true
        scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
        flow: Down
    }

    CachedScrollXY = <CachedView> {
        scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}
    }

    CachedScrollX = <CachedView> {
        scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}
    }

    CachedScrollY = <CachedView> {
        scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
    }

    ScrollXYView = <ViewBase> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}}
    ScrollXView = <ViewBase> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}}
    ScrollYView = <ViewBase> {scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}}

    TextInput = <TextInputBase> {
        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            wrap: Word,
            text_style: <THEME_FONT_LABEL> {}
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
                    #3,
                    self.is_empty
                )
            }
        }

        draw_cursor: {
            instance focus: 0.0
            uniform border_radius: 0.5
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(mix(#ccc0, #f, self.focus));
                return sdf.result
            }
        }

        draw_select: {
            instance hover: 0.0
            instance focus: 0.0
            uniform border_radius: 2.0
            fn pixel(self) -> vec4 {
                //return mix(#f00,#0f0,self.pos.y)
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(mix(#5550, #xFFFFFF40, self.focus)); // Pad color
                return sdf.result
            }
        }

        cursor_margin_bottom: 3.0,
        cursor_margin_top: 4.0,
        select_pad_edges: 3.0
        cursor_size: 2.0,
        numeric_only: false,
        on_focus_select_all: false,
        empty_message: "0",
        draw_bg: {
            instance radius: 2.0
            instance border_width: 0.0
            instance border_color: #3
            instance inset: vec4(0.0, 0.0, 0.0, 0.0)

            fn get_color(self) -> vec4 {
                return self.color
            }

            fn get_border_color(self) -> vec4 {
                return self.border_color
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
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result;
            }
        },
        clip_x: false,
        clip_y: false,
        padding: {left: 10, top: 11, right: 10, bottom: 10}
        label_align: {y: 0.}
        //margin: {top: 5, right: 5}
        width: Fit,
        height: Fit,

        /*label_walk: {
            width: Fit,
            height: Fit,
            //margin: 0//{left: 5.0, right: 5.0, top: 0.0, bottom: 2.0},
        }*/

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_select: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_select: {hover: 1.0}
                        draw_text: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_cursor: {focus: 0.0},
                        draw_select: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_cursor: {focus: 1.0},
                        draw_select: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    Slider = <SliderBase> {
        min: 0.0,
        max: 1.0,
        step: 0.0,

        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float

            fn pixel(self) -> vec4 {
                let slider_height = 3;
                let nub_size = mix(3, 4, self.hover);
                let nubbg_size = 18

                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                let slider_bg_color = mix(#38, #30, self.focus);
                let slider_color = mix(mix(#5, #68, self.hover), #68, self.focus);
                let nub_color = mix(mix(#8, #f, self.hover), mix(#c, #f, self.drag), self.focus);
                let nubbg_color = mix(#eee0, #8, self.drag);

                match self.slider_type {
                    SliderType::Horizontal => {
                        sdf.rect(0, self.rect_size.y - slider_height, self.rect_size.x, slider_height)
                        sdf.fill(slider_bg_color);

                        sdf.rect(0, self.rect_size.y - slider_height, self.slide_pos * (self.rect_size.x - nub_size) + nub_size, slider_height)
                        sdf.fill(slider_color);

                        let nubbg_x = self.slide_pos * (self.rect_size.x - nub_size) - nubbg_size * 0.5 + 0.5 * nub_size;
                        sdf.rect(nubbg_x, self.rect_size.y - slider_height, nubbg_size, slider_height)
                        sdf.fill(nubbg_color);

                        // the nub
                        let nub_x = self.slide_pos * (self.rect_size.x - nub_size);
                        sdf.rect(nub_x, self.rect_size.y - slider_height, nub_size, slider_height)
                        sdf.fill(nub_color);
                    }
                    SliderType::Vertical => {

                    }
                    SliderType::Rotary => {

                    }
                }
                return sdf.result
            }
        }

        draw_text: {
            color: #9
        }

        label_walk: {
            margin: {left: 4.0, top: 3.0}
            width: Fill,
            height: Fill
        }

        label_align: {
            y: 0.0
        }

        precision: 2,

        text_input: <TextInput> {
            cursor_margin_bottom: 3.0,
            cursor_margin_top: 4.0,
            select_pad_edges: 3.0
            cursor_size: 2.0,
            empty_message: "0",
            numeric_only: true,
            draw_bg: {
                shape: None
                color: #5
                radius: 2.0
            },

            padding: 0,
            label_align: {y: 0.},
            margin: {top: 3, right: 3}
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_slider: {hover: 0.0}
                        //text_input: {animator: {hover = off}}
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: {hover: 1.0}
                        //text_input: {animator: {hover = on}}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_slider: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {draw_slider: {drag: 1.0}}
                }
            }
        }
    }


    SlideBody = <Label> {
        margin:{top:20}
        draw_text: {
            color: #D
            text_style: {
                line_spacing:1.5
                font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
                font_size: 35
            }
        }
        text: ""
    }

    Slide = <RoundedView> {
        draw_bg: {color: #x1A, radius: 5.0}
        width: Fill,
        height: Fill
        align: {x: 0.0, y: 0.5} flow: Down, spacing: 10, padding: 50
        title = <Label> {
            draw_text: {
                color: #f
                text_style: {
                    line_spacing:1.0
                    font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
                    font_size: 84
                }
            }
            text: "SlideTitle"
        }
    }

    SlideChapter = <Slide> {
        draw_bg: {color: #xFF5C39, radius: 5.0}
        width: Fill,
        height: Fill
        align: {x: 0.0, y: 0.5} flow: Down, spacing: 10, padding: 50
        title = <Label> {
            draw_text: {
                color: #x181818
                text_style: {
                    line_spacing:1.0
                    font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
                    font_size: 90
                }
            }
            text: "SlideTitle"
        }
    }

    SlidesView = <SlidesViewBase> {
        anim_speed: 0.9
    }

    DrawScrollShadow = <DrawScrollShadowBase> {

        shadow_size: 4.0,

        fn pixel(self) -> vec4 { // TODO make the corner overlap properly with a distance field eq.
            let is_viz = clamp(self.scroll * 0.1, 0., 1.);
            let pos = self.pos;
            let base = THEME_COLOR_BG_EDITOR.xyz;
            let alpha = 0.0;
            if self.shadow_is_top > 0.5 {
                alpha = pow(pos.y, 0.5);
            }
            else {
                alpha = pow(pos.x, 0.5);
            }
            //turn vec4(base,is_viz);
            return Pal::premul(mix(vec4(#000.xyz, is_viz), vec4(base, 0.), alpha));
        }
    }

    // StackView DSL begin

    HEADER_HEIGHT = 80.0

    StackViewHeader = <View> {
        width: Fill, height: (HEADER_HEIGHT),
        padding: {bottom: 10., top: 50.}
        show_bg: true
        draw_bg: {
            color: #EDEDED
        }

        content = <View> {
            width: Fill, height: Fit
            flow: Overlay,
        
            title_container = <View> {
                width: Fill, height: Fit
                align: {x: 0.5, y: 0.5}
    
                title = <Label> {
                    width: Fit, height: Fit
                    draw_text: {
                        text_style: { font_size: 12. },
                        color: #000,
                    },
                    text: "Stack View Title"
                }
            }

            button_container = <View> {
                left_button = <Button> {
                    width: Fit, height: 68
                    icon_walk: {width: 10, height: 68}
                    draw_bg: {
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            return sdf.result
                        }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/back.svg"),
                        color: #000;
                        brightness: 0.8;
                    }
                }
            }
        }
    }

    StackNavigationView = <StackNavigationViewBase> {
        visible: false
        width: Fill, height: Fill
        flow: Overlay

        show_bg: true
        draw_bg: {
            color: #fff
        }

        // Empty slot to place a generic full-screen background
        background = <View> {
            width: Fill, height: Fill
            visible: false
        }

        body = <View> {
            width: Fill,
            height: Fill,
            flow: Down,

            // Space between body and header can be adjusted overriding this margin
            margin: {top: (HEADER_HEIGHT)},
        }

        header = <StackViewHeader> {}

        offset: 4000.0

        animator: {
            slide = {
                default: hide,
                hide = {
                    redraw: true
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    from: {all: Forward {duration: 5.0}}
                    // Large enough number to cover several screens,
                    // but we need a way to parametrize it
                    apply: {offset: 4000.0}
                }

                show = {
                    redraw: true
                    ease: ExpDecay {d1: 0.82, d2: 0.95}
                    from: {all: Forward {duration: 0.5}}
                    apply: {offset: 0.0}
                }
            }
        }
    }

    StackNavigation = <StackNavigationBase> {
        width: Fill, height: Fill
        flow: Overlay

        root_view = <View> {}
    }

    // StackView DSL end
}
