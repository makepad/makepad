use crate::makepad_platform::*;

live_design!{
    import crate::button::*;
    
    FONT_LABEL = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
        }
    }
    
    FONT_DATA = {
        font_size: 9.4,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
        }
    }
    
    FONT_META = {
        font_size: 9.4,
        top_drop: 1.2,
        font: {
            path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
        }
    }
    
    FONT_CODE = {
        font: {
            path: dep("crate://self/resources/LiberationMono-Regular.ttf")
        }
        brightness: 1.1
        font_size: 9.0
        line_spacing: 2.0
        top_drop: 1.3
    }
    
    const DIM_DATA_ITEM_HEIGHT = 23.0
    const DIM_DATA_ICON_WIDTH = 16.0
    const DIM_DATA_ICON_HEIGHT = 24.0
    // ABSOLUTE DEFS
    
    const BRIGHTNESS = #x40
    const COLOR_HIGHLIGHT = #42
    const COLOR_HIGH = #C00
    const COLOR_MID = #FA0
    const COLOR_LOW = #8A0
    
    // RELATIVE =DEFS
    //    42, =78, 117
    const COLOR_WHITE = #FFF
    const COLOR_UP_80 = #FFFFFFCC
    const COLOR_UP_50 = #FFFFFF80
    const COLOR_UP_25 = #FFFFFF40
    const COLOR_UP_15 = #FFFFFF26
    const COLOR_UP_10 = #FFFFFF1A
    const COLOR_UP_4 = #FFFFFF0A
    const COLOR_DOWN_7 = #00000013
    const COLOR_DOWN_10 = #00000030
    const COLOR_DOWN_20 = #00000040
    const COLOR_DOWN_50 = #00000080
    const COLOR_BLACK = #000
    
    // CORE BACKGROUND COLORS
    
    const COLOR_BG_APP = (BRIGHTNESS)
    
    const COLOR_BG_HEADER = (blend(
        COLOR_BG_APP,
        COLOR_DOWN_10
    ))
    
    const COLOR_CLEAR = (COLOR_BG_APP)
    
    const COLOR_BG_EDITOR = (blend(
        COLOR_BG_HEADER,
        COLOR_DOWN_10
    ))
    
    const COLOR_BG_ODD = (blend(
        COLOR_BG_EDITOR,
        COLOR_DOWN_7
    ))
    
    const COLOR_BG_SELECTED = (COLOR_HIGHLIGHT)
    
    const COLOR_BG_UNFOCUSSED = (blend(
        COLOR_BG_EDITOR,
        COLOR_UP_10
    ))
    
    const COLOR_EDITOR_SELECTED = (COLOR_BG_SELECTED)
    const COLOR_EDITOR_SELECTED_UNFOCUSSED = (COLOR_BG_SELECTED_UNFOCUSSED)
    
    const COLOR_BG_CURSOR = (blend(
        COLOR_BG_EDITOR,
        COLOR_UP_4
    ))
    
    const COLOR_FG_CURSOR = (blend(
        COLOR_BG_EDITOR,
        COLOR_UP_50
    ))
    
    // TEXT / ICON COLORS
    
    const COLOR_TEXT_DEFAULT = (COLOR_UP_50)
    const COLOR_TEXT_HOVER = (COLOR_UP_80)
    const COLOR_TEXT_META = (COLOR_UP_25)
    const COLOR_TEXT_SELECTED = (COLOR_UP_80)
    
    // SPLITTER AND SCROLLBAR
    
    const COLOR_SCROLL_BAR_DEFAULT = (COLOR_UP_10)
    
    const COLOR_CONTROL_HOVER = (blend(
        COLOR_BG_HEADER,
        COLOR_UP_50
    ))
    
    const COLOR_CONTROL_PRESSED = (blend(
        COLOR_BG_HEADER,
        COLOR_UP_25
    ))
    
    // ICON COLORS
    
    const COLOR_ICON_WAIT = (COLOR_LOW),
    const COLOR_ERROR = (COLOR_HIGH),
    const COLOR_WARNING = (COLOR_MID),
    const COLOR_ICON_PANIC = (COLOR_HIGH)
    const COLOR_DRAG_QUAD = (COLOR_UP_50)
    const COLOR_PANIC = #f0f
    
    const DIM_TAB_HEIGHT = 26.0,
    const DIM_SPLITTER_HORIZONTAL = 16.0,
    const DIM_SPLITTER_MIN_HORIZONTAL = (DIM_TAB_HEIGHT),
    const DIM_SPLITTER_MAX_HORIZONTAL = (DIM_TAB_HEIGHT + DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_MIN_VERTICAL = (DIM_SPLITTER_HORIZONTAL),
    const DIM_SPLITTER_MAX_VERTICAL = (DIM_SPLITTER_HORIZONTAL + DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_SIZE = 5.0
    
    Button = <Button>{
         draw_text: {
            instance hover: 0.0
            instance pressed: 0.0
            text_style: {
                font: {
                    //path: d"resources/IBMPlexSans-SemiBold.ttf"
                }
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
        
        draw_icon:{
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
        
        width: Fit,
        height: Fit,
        margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0}
        align: {x: 0.5, y: 0.5}
        padding: {left: 14.0, top: 10.0, right: 14.0, bottom: 10.0}
        
        label_walk:{
            width: Fit,
            height: Fit
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
    
}

