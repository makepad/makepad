use crate::makepad_platform::*;

live_design! {
    import makepad_draw::shader::std::*;
    import crate::base::*;

    // DIMENSIONS
    THEME_SPACE_FACTOR = 10.0 // Increase for a less dense layout
    THEME_SPACE_1 = (0.5 * (THEME_SPACE_FACTOR))
    THEME_SPACE_2 = (1 * (THEME_SPACE_FACTOR))
    THEME_SPACE_3 = (1.5 * (THEME_SPACE_FACTOR))

    THEME_MSPACE_1 = {top: (THEME_SPACE_1), right: (THEME_SPACE_1), bottom: (THEME_SPACE_1), left: (THEME_SPACE_1)} THEME_MSPACE_H_1 = {top: 0., right: (THEME_SPACE_1), bottom: 0., left: (THEME_SPACE_1)}
    THEME_MSPACE_V_1 = {top: (THEME_SPACE_1), right: 0., bottom: (THEME_SPACE_1), left: 0.}
    THEME_MSPACE_2 = {top: (THEME_SPACE_2), right: (THEME_SPACE_2), bottom: (THEME_SPACE_2), left: (THEME_SPACE_2)}
    THEME_MSPACE_H_2 = {top: 0., right: (THEME_SPACE_2), bottom: 0., left: (THEME_SPACE_2)}
    THEME_MSPACE_V_2 = {top: (THEME_SPACE_2), right: 0., bottom: (THEME_SPACE_2), left: 0.}
    THEME_MSPACE_3 = {top: (THEME_SPACE_3), right: (THEME_SPACE_3), bottom: (THEME_SPACE_3), left: (THEME_SPACE_3)}
    THEME_MSPACE_H_3 = {top: 0., right: (THEME_SPACE_3), bottom: 0., left: (THEME_SPACE_3)}
    THEME_MSPACE_V_3 = {top: (THEME_SPACE_3), right: 0., bottom: (THEME_SPACE_3), left: 0.}

    THEME_DATA_ITEM_HEIGHT = 23.0
    THEME_DATA_ICON_WIDTH = 16.0
    THEME_DATA_ICON_HEIGHT = 24.0

    THEME_CTRL_CORNER_RADIUS = 2.5
    THEME_CONTAINER_CORNER_RADIUS = 5.0
    THEME_TEXTSELECTION_CORNER_RADIUS = 1.25
    THEME_BEVEL_BORDER = .75
    THEME_TAB_HEIGHT = 32.0,
    THEME_SPLITTER_HORIZONTAL = 16.0,
    THEME_SPLITTER_MIN_HORIZONTAL = (THEME_TAB_HEIGHT),
    THEME_SPLITTER_MAX_HORIZONTAL = (THEME_TAB_HEIGHT + THEME_SPLITTER_SIZE),
    THEME_SPLITTER_MIN_VERTICAL = (THEME_SPLITTER_HORIZONTAL),
    THEME_SPLITTER_MAX_VERTICAL = (THEME_SPLITTER_HORIZONTAL + THEME_SPLITTER_SIZE),
    THEME_SPLITTER_SIZE = 5.0
    THEME_DOCK_BORDER_SIZE: 0.0


    // COLOR PALETTE
    THEME_COLOR_CONTRAST = 1.0 // HIGHER VALUE = HIGHER CONTRAST, RECOMMENDED VALUES: 0.5 - 2.5

    THEME_COLOR_WHITE = #FFFFFFFF
    THEME_COLOR_U_8 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.2, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_6 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.35, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_5 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.5, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_4 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.6, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_3 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.75, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_2 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.85, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_1 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.9, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_04 = (mix(#FFFFFFFF, #FFFFFF00, pow(0.95, THEME_COLOR_CONTRAST)))

    THEME_COLOR_D_04 = (mix(#000000FF, #00000000, pow(0.9, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_075 = (mix(#000000FF, #00000000, pow(0.85, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_1 = (mix(#000000FF, #00000000, pow(0.8, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_2 = (mix(#000000FF, #00000000, pow(0.75, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_3 = (mix(#000000FF, #00000000, pow(0.6, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_4 = (mix(#000000FF, #00000000, pow(0.4, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_5 = (mix(#000000FF, #00000000, pow(0.35, THEME_COLOR_CONTRAST)))
    THEME_COLOR_BLACK = #000000FF

    // BASICS
    THEME_COLOR_MAKEPAD = #FF5C39FF

    THEME_COLOR_BG_APP = (mix(#000000FF, #7, pow(0.5,THEME_COLOR_CONTRAST)))
    THEME_COLOR_APP_CAPTION_BAR = (THEME_COLOR_D_HIDDEN)
    THEME_COLOR_DRAG_QUAD = (THEME_COLOR_U_5)

    THEME_COLOR_CURSOR_BG = (THEME_COLOR_BLACK)
    THEME_COLOR_CURSOR_BORDER = (THEME_COLOR_WHITE)

    THEME_COLOR_U_HIDDEN = #FFFFFF00
    THEME_COLOR_D_HIDDEN = #00000000

    THEME_COLOR_TEXT_DEFAULT = (THEME_COLOR_U_6)
    THEME_COLOR_TEXT_HL = (THEME_COLOR_TEXT_DEFAULT)
    THEME_COLOR_TEXT_META = (THEME_COLOR_U_4)

    THEME_COLOR_TEXT_PRESSED = (THEME_COLOR_U_3)
    THEME_COLOR_TEXT_HOVER = (THEME_COLOR_WHITE)
    THEME_COLOR_TEXT_ACTIVE = (THEME_COLOR_U_6)
    THEME_COLOR_TEXT_INACTIVE = (THEME_COLOR_U_4)
    THEME_COLOR_TEXT_SELECTED = (THEME_COLOR_U_8)
    THEME_COLOR_TEXT_FOCUSED = (THEME_COLOR_U_6)
    THEME_COLOR_TEXT_PLACEHOLDER = (THEME_COLOR_U_4)

    THEME_COLOR_TEXT_CURSOR = (THEME_COLOR_WHITE)

    THEME_COLOR_BG_CONTAINER = (THEME_COLOR_D_075)
    THEME_COLOR_BG_EVEN = (THEME_COLOR_BG_CONTAINER * 0.75)
    THEME_COLOR_BG_ODD = (THEME_COLOR_BG_CONTAINER * 1.25)
    THEME_COLOR_BG_HIGHLIGHT = (THEME_COLOR_U_04) // Code-blocks and quotes.
    THEME_COLOR_BG_HIGHLIGHT_INLINE = (THEME_COLOR_U_2) // i.e. inline code

    THEME_COLOR_BEVEL_RIMLIGHT = (THEME_COLOR_U_3)
    THEME_COLOR_BEVEL_SHADOW = (THEME_COLOR_D_5)
 
    // WIDGET COLORS
    THEME_COLOR_CTRL_DEFAULT = (THEME_COLOR_U_04)
    THEME_COLOR_CTRL_PRESSED = (THEME_COLOR_D_075)
    THEME_COLOR_CTRL_HOVER = (THEME_COLOR_U_2)
    THEME_COLOR_CTRL_ACTIVE = (THEME_COLOR_D_04)
    THEME_COLOR_CTRL_SELECTED = (THEME_COLOR_U_8)
    THEME_COLOR_CTRL_INACTIVE = (THEME_COLOR_D_HIDDEN)

    THEME_COLOR_FLOATING_BG = #505050FF // Elements that live on top of the UI like dialogs, popovers, and context menus.

    // Background of textinputs, radios, checkboxes etc.
    THEME_COLOR_INSET_DEFAULT = (THEME_COLOR_U_04)
    THEME_COLOR_INSET_HOVER = (THEME_COLOR_U_1)
    THEME_COLOR_INSET_ACTIVE = (THEME_COLOR_U_2)
    THEME_COLOR_INSET_PIT_TOP = (THEME_COLOR_D_4)
    THEME_COLOR_INSET_PIT_TOP_HOVER = (THEME_COLOR_D_4)
    THEME_COLOR_INSET_PIT_BOTTOM = (THEME_COLOR_D_HIDDEN)

    // Progress bars, slider amounts etc.
    THEME_COLOR_AMOUNT_DEFAULT = (THEME_COLOR_U_3)
    THEME_COLOR_AMOUNT_HOVER = (THEME_COLOR_U_4)
    THEME_COLOR_AMOUNT_ACTIVE = (THEME_COLOR_U_5)
    THEME_COLOR_AMOUNT_TRACK_DEFAULT = (THEME_COLOR_D_3)
    THEME_COLOR_AMOUNT_TRACK_HOVER = (THEME_COLOR_D_4)
    THEME_COLOR_AMOUNT_TRACK_ACTIVE = (THEME_COLOR_D_5)

    THEME_COLOR_MENU_BG_DEFAULT = (THEME_COLOR_D_HIDDEN)
    THEME_COLOR_MENU_BG_HOVER = (THEME_COLOR_D_HIDDEN)
    THEME_COLOR_MENU_TEXT_DEFAULT = (THEME_COLOR_D_HIDDEN)
    THEME_COLOR_MENU_TEXT_HOVER = (THEME_COLOR_D_HIDDEN)

    // WIDGET SPECIFIC COLORS
    THEME_COLOR_DIVIDER = (THEME_COLOR_D_3)

    THEME_COLOR_SLIDER_NUB_DEFAULT = (THEME_COLOR_WHITE)
    THEME_COLOR_SLIDER_NUB_HOVER = (THEME_COLOR_WHITE)
    THEME_COLOR_SLIDER_NUB_ACTIVE = (THEME_COLOR_WHITE)

    THEME_COLOR_SLIDES_CHAPTER = (THEME_COLOR_MAKEPAD)
    THEME_COLOR_SLIDES_BG = (THEME_COLOR_D_4)


    // TODO: THESE ARE APPLICATION SPECIFIC COLORS THAT SHOULD BE MOVED FROM THE GENERAL THEME TO THE GIVEN PROJECT
    THEME_COLOR_HIGH = #C00
    THEME_COLOR_MID = #FA0
    THEME_COLOR_LOW = #8A0
    THEME_COLOR_PANIC = #f0f
    THEME_COLOR_ICON_WAIT = (THEME_COLOR_LOW),
    THEME_COLOR_ERROR = (THEME_COLOR_HIGH),
    THEME_COLOR_WARNING = (THEME_COLOR_MID),
    THEME_COLOR_ICON_PANIC = (THEME_COLOR_HIGH)


    // TYPOGRAPHY
    THEME_FONT_SIZE_BASE = 7.5
    THEME_FONT_SIZE_CONTRAST = 2.5// Greater values = greater font-size steps between font-formats (i.e. from H3 to H2)

    THEME_FONT_SIZE_CODE = 9.0
    THEME_FONT_LINE_SPACING = 1.43

    THEME_FONT_SIZE_1 = (THEME_FONT_SIZE_BASE + 16 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_2 = (THEME_FONT_SIZE_BASE + 8 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_3 = (THEME_FONT_SIZE_BASE + 4 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_4 = (THEME_FONT_SIZE_BASE + 2 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_P = (THEME_FONT_SIZE_BASE + 1 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_META = (THEME_FONT_SIZE_BASE + 0.5 * THEME_FONT_SIZE_CONTRAST)

    THEME_FONT_LABEL = { font: { path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf") } } // TODO: LEGACY, REMOVE. REQUIRED BY RUN LIST IN STUDIO ATM
    THEME_FONT_REGULAR = { font: { path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf") } }
    THEME_FONT_BOLD = { font: { path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf") } }
    THEME_FONT_ITALIC = { font: { path: dep("crate://self/resources/NotoSans-Italic.ttf") } }
    THEME_FONT_BOLD_ITALIC = { font: { path: dep("crate://self/resources/NotoSans-BoldItalic.ttf") } }
    THEME_FONT_DATA = { font: { path: dep("crate://self/resources/LiberationMono-Regular.ttf") } }
    THEME_FONT_CODE = {
        font: {
            path: dep("crate://self/resources/LiberationMono-Regular.ttf")
        }
        font_size: (THEME_FONT_SIZE_P)
        brightness: 1.1
        top_drop: 1.3
    }

    Label = <LabelBase> {
        width: Fit, height: Fit,
        draw_text: {
            color: (THEME_COLOR_TEXT_DEFAULT),
            text_style: <THEME_FONT_REGULAR> {},
            wrap: Word
        }
    }

    H1 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_1)}
        draw_text: {
            wrap: Word
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_1)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H1"
    }

    H1italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_1)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_1)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H1"
    }

    H2 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_2)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_2)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H2"
    }

    H2italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_2)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_2)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H2"
    }

    H3 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_3)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_3)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H3"
    }

    H3italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_3)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_3)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H3"
    }

    H4 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_4)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_4)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H4"
    }

    H4italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_4)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_4)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Headline H4"
    }

    P = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_P), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Paragraph"
    }

    Pbold = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_P), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Paragraph"
    }

    Pitalic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_P), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Paragraph"
    }

    Pbolditalic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_P), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Paragraph"
    }

    Meta = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_P), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_META)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Meta data"
    }

    Hr = <View> {
        width: Fill, height: Fit,
        flow: Down,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}
        <View> {
            width: Fill, height: (THEME_BEVEL_BORDER * 2.0),
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_SHADOW) }
        }
        <View> {
            width: Fill, height: (THEME_BEVEL_BORDER * 0.5),
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_RIMLIGHT) }
        }
    }

//    TODO: enable once Makepad's layout supports Fill that knows how high adjacent elements are. For now this is not possible.
//    Vr = <View> {
//         width: Fit., height: Fill,
//         flow: Right,
//         spacing: 0.,
//         margin: <THEME_MSPACE_V_2> {}
//         <View> {
//             width: (THEME_BEVEL_BORDER * 2.0), height: Fill
//             show_bg: true,
//             draw_bg: { color: #f00 }
//         }
//         <View> {
//             width: (THEME_BEVEL_BORDER * 0.5), height: Fill,
//             show_bg: true,
//             draw_bg: { color: #f0f }
//         }
//     }


    Filler = <View> {
        width: Fill, height: Fill
    }

    HtmlLink = <HtmlLinkBase> {
        width: Fit,
        height: Fit,
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
            color: (THEME_COLOR_TEXT_DEFAULT),
            instance color_hover: (THEME_COLOR_TEXT_HOVER),
            instance color_pressed: (THEME_COLOR_TEXT_PRESSED),
            instance pressed: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_LABEL>{}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color_hover,
                        self.hover
                    ),
                    self.color_pressed,
                    self.pressed
                )
            }
        }
    }

    Html = <HtmlBase> {
        width: Fill, height: Fit,
        flow: RightWrap,

        font_size: (THEME_FONT_SIZE_P),
        line_spacing: (THEME_FONT_LINE_SPACING),

        draw_normal: {
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_italic: {
            text_style: <THEME_FONT_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_bold: {
            text_style: <THEME_FONT_BOLD> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_bold_italic: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_fixed: {
            text_style: <THEME_FONT_CODE> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        code_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> {}
        }
        code_walk: { width: Fill, height: Fit }

        quote_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_3> {}
        }
        quote_walk: { width: Fill, height: Fit }

        list_item_layout: {
            flow: RightWrap,
            padding: { right: 10. }
        }
        list_item_walk: { width: Fill, height: Fit }

        inline_code_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_1> {}
        }
        inline_code_walk:{ height:Fit, width:Fit, margin: { top: -2 } }

        sep_walk: {
            width: Fill, height: 4.
            margin: <THEME_MSPACE_V_3> {}
        }

        a = <HtmlLink> {
            draw_text: {
                text_style: {
                    font_size: 12,
                }
            }
        }

        draw_block: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                match self.block_type {
                    FlowBlockType::Quote => {
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            2.
                        );
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT)
                        sdf.box(
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            self.rect_size.y - THEME_SPACE_2,
                            1.5
                        );
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT);
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
                        sdf.fill(THEME_COLOR_DIVIDER);
                        return sdf.result;
                    }
                    FlowBlockType::Code => {
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            2.
                        );
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT);
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
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT_INLINE);
                        return sdf.result;
                    }
                    FlowBlockType::Underline => {
                        sdf.box(
                            0.,
                            self.rect_size.y-2,
                            self.rect_size.x,
                            2.0,
                            0.5
                        );
                        sdf.fill(THEME_COLOR_TEXT_DEFAULT);
                        return sdf.result;
                    }
                    FlowBlockType::Strikethrough => {
                        sdf.box(
                            0.,
                            self.rect_size.y * 0.45,
                            self.rect_size.x,
                            2.0,
                            0.5
                        );
                        sdf.fill(THEME_COLOR_TEXT_DEFAULT);
                        return sdf.result;
                    }
                }
                return #f00
            }
        }
    }

    Markdown = <MarkdownBase> {
        width:Fill, height:Fit,
        flow: RightWrap,
        padding: <THEME_MSPACE_1> {}

        line_spacing: (THEME_FONT_LINE_SPACING),
        font_size: (THEME_FONT_SIZE_P),

        draw_normal: {
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_italic: {
            text_style: <THEME_FONT_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_bold: {
            text_style: <THEME_FONT_BOLD> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_bold_italic: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        draw_fixed: {
            text_style: <THEME_FONT_CODE> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }

        code_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        code_walk: { width: Fill, height: Fit }

        inline_code_layout: {
            flow: RightWrap,
            padding: { left: (THEME_SPACE_1), right: (THEME_SPACE_1) }
        }
        inline_code_walk: {
            width: Fit, height: Fit,
            margin: { top: -2. }
        }

        quote_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        quote_walk: { width: Fill, height: Fit, }

        list_item_walk: {
            height: Fit, width: Fill,
        }
        list_item_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_1> {}
        }

        sep_walk: {
            width: Fill, height: 4.
            margin: <THEME_MSPACE_V_3> {}
        }

        draw_block: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                match self.block_type {
                    FlowBlockType::Quote => {
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            2.
                        );
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT)
                        sdf.box(
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            self.rect_size.y - THEME_SPACE_2,
                            1.5
                        );
                        sdf.fill(THEME_COLOR_TEXT_DEFAULT);
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
                        sdf.fill(THEME_COLOR_DIVIDER);
                        return sdf.result;
                    }
                    FlowBlockType::Code => {
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            2.
                        );
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT);
                        return sdf.result;
                    }
                    FlowBlockType::InlineCode => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x,
                            self.rect_size.y - 2.,
                            2.
                        );
                        sdf.fill(THEME_COLOR_BG_HIGHLIGHT_INLINE);
                        return sdf.result;
                    }
                    FlowBlockType::Underline => {
                        sdf.box(
                            0.,
                            self.rect_size.y-2,
                            self.rect_size.x,
                            2.0,
                            0.5
                        );
                        sdf.fill(THEME_COLOR_TEXT_DEFAULT);
                        return sdf.result;
                    }
                    FlowBlockType::Strikethrough => {
                        sdf.box(
                            0.,
                            self.rect_size.y * 0.45,
                            self.rect_size.x,
                            2.0,
                            0.5
                        );
                        sdf.fill(THEME_COLOR_TEXT_DEFAULT);
                        return sdf.result;
                    }
                }
                return #f00
            }
        }
    }

    Spacer = <View> { width: Fill, height: Fill }

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
                return sdf.fill( mix(
                    THEME_COLOR_CTRL_DEFAULT,
                    mix(
                        THEME_COLOR_CTRL_HOVER,
                        THEME_COLOR_CTRL_PRESSED,
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

    Button = <ButtonBase> {
        width: Fit, height: Fit,
        spacing: 7.5,
        align: {x: 0.5, y: 0.5},
        padding: <THEME_MSPACE_2> {}
        label_walk: { width: Fit, height: Fit },

        draw_text: {
            instance hover: 0.0,
            instance pressed: 0.0,
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return THEME_COLOR_TEXT_DEFAULT
            }
        }

        icon_walk: {
            width: (THEME_DATA_ICON_WIDTH), height: Fit,
        }

        draw_icon: {
            instance hover: 0.0
            instance pressed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                )
            }
        }

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CTRL_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_CTRL_DEFAULT)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_CTRL_PRESSED, self.pressed);

                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(body_transp, mix(THEME_COLOR_BEVEL_RIMLIGHT, THEME_COLOR_BEVEL_SHADOW, self.pressed), max(0.0, grad_top - sdf.pos.y) / grad_top);
                let bot_gradient = mix(
                    mix(body_transp, THEME_COLOR_BEVEL_RIMLIGHT, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

                // the little drop shadow at the bottom
                let shift_inward = self.border_radius * 1.75;
                sdf.move_to(shift_inward, self.rect_size.y);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
                sdf.stroke(
                    mix(
                        THEME_COLOR_BEVEL_SHADOW,
                        THEME_COLOR_BEVEL_RIMLIGHT,
                        self.pressed
                    ), THEME_BEVEL_BORDER
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
                    THEME_BEVEL_BORDER * 1.5
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

    ButtonIcon = <Button> {
        icon_walk: {
            width: 12.
            margin: { left: 0. }
        }
    }

    ButtonFlat = <ButtonIcon> {
        height: Fit, width: Fit,
        margin: <THEME_MSPACE_H_1> {}
        align: { x: 0.5, y: 0.5 }
        icon_walk: { width: 12. }
        draw_bg: { fn pixel(self) -> vec4 { return (THEME_COLOR_D_HIDDEN) } }

        draw_text: {
            instance hover: 0.0,
            instance pressed: 0.0,
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_PRESSED,
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
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                )
            }
        }
    }

    CheckBox = <CheckBoxBase> {
        width: Fit, height: 20,
        margin: { top: (THEME_SPACE_1), bottom: (THEME_SPACE_1) }
        align: { x: 0.0, y: 0.5 }
        label_walk: {
            width: Fit, height: Fit,
            margin: { left: 20., right: (THEME_SPACE_2) }
        }

        draw_check: {
            uniform size: 7.5;
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.check_type {
                    CheckType::Check => {
                        let left = 1;
                        let sz = self.size - 0.5;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.box(left, c.y - sz, sz * 2.0, sz * 2.0, 1.5);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_RIMLIGHT, self.pos.y), THEME_BEVEL_BORDER)
                        let szs = sz * 0.5;
                        let dx = 1.0;
                        sdf.move_to(left + 4.0, c.y);
                        sdf.line_to(c.x, c.y + szs);
                        sdf.line_to(c.x + szs, c.y - szs);
                        sdf.stroke(mix(
                            mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_CTRL_HOVER, self.hover),
                            THEME_COLOR_TEXT_ACTIVE,
                            self.selected), 1.25
                        );
                    }
                    CheckType::Radio => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_RIMLIGHT, self.pos.y), THEME_BEVEL_BORDER)
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(
                            mix(
                                mix(
                                    THEME_COLOR_U_HIDDEN,
                                    THEME_COLOR_CTRL_HOVER,
                                    self.hover
                                ),
                                THEME_COLOR_TEXT_ACTIVE,
                                self.selected
                            )
                        );
                    }
                    CheckType::Toggle => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);

                        sdf.stroke_keep(
                            mix(
                                THEME_COLOR_BEVEL_SHADOW,
                                THEME_COLOR_BEVEL_RIMLIGHT,
                                clamp(self.pos.y - 0.2, 0, 1)),
                            THEME_BEVEL_BORDER
                        )

                        sdf.fill(
                            mix(
                                mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM * 0.1, pow(self.pos.y, 1.0)),
                                mix(THEME_COLOR_INSET_PIT_TOP_HOVER * 1.75, THEME_COLOR_INSET_PIT_BOTTOM * 0.1, pow(self.pos.y, 1.0)),
                                self.hover
                            )
                        )
                        let isz = sz * 0.65;
                        sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                        sdf.circle(left + sz + self.selected * sz, c.y - 0.5, 0.425 * isz);
                        sdf.subtract();
                        sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                        sdf.blend(self.selected)
                        sdf.fill(mix(THEME_COLOR_TEXT_DEFAULT, THEME_COLOR_TEXT_HOVER, self.hover));
                    }
                    CheckType::None => {
                        return THEME_COLOR_D_HIDDEN
                    }
                }
                return sdf.result
            }
        }

        draw_text: {
            instance focus: 0.0
            instance selected: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_DEFAULT,
                        self.hover
                    ),
                    THEME_COLOR_CTRL_SELECTED,
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
                        THEME_COLOR_INSET_PIT_TOP,
                        THEME_COLOR_CTRL_HOVER,
                        self.hover
                    ),
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    self.selected
                )
            }
        }

        icon_walk: { width: 13.0, height: Fit }

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

    CheckBoxToggle = <CheckBox> {
        margin: { left: -8. }
        draw_check: { check_type: Toggle }
        label_walk: { margin: <THEME_MSPACE_H_1> { left: 35.} }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.25}}
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
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 0.0},
                        draw_text: {selected: 0.0},
                        draw_icon: {selected: 0.0},
                    }
                }
                on = {
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0},
                    }
                }
            }
        }
    }

    CheckBoxCustom = <CheckBox> {
        draw_check: { check_type: None }
        label_walk: { margin: <THEME_MSPACE_H_1> {} }
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
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
    }

    WindowMenu = <WindowMenuBase> { height: 0, width: 0, }

    Window = <WindowBase> {
        pass: {clear_color: (THEME_COLOR_BG_APP)}
        flow: Down
        nav_control: <NavControl> {}
        caption_bar = <SolidView> {
            visible: false,

            flow: Right

            draw_bg: {color: (THEME_COLOR_APP_CAPTION_BAR)}
            height: 27,
            caption_label = <View> {
                width: Fill, height: Fill,
                align: {x: 0.5, y: 0.5},
                label = <Label> {text: "Makepad", margin: {left: 100}}
            }
            windows_buttons = <View> {
                visible: false,
                width: Fit, height: Fit,
                min = <DesktopButton> {draw_bg: {button_type: WindowsMin}}
                max = <DesktopButton> {draw_bg: {button_type: WindowsMax}}
                close = <DesktopButton> {draw_bg: {button_type: WindowsClose}}
            }
            web_fullscreen = <View> {
                visible: false,
                width: Fit, height: Fit,
                fullscreen = <DesktopButton> {draw_bg: {button_type: Fullscreen}}
            }
            web_xr = <View> {
                visible: false,
                width: Fit, height: Fit,
                xr_on = <DesktopButton> {draw_bg: {button_type: XRMode}}
            }
        }

        window_menu = <WindowMenu> {
            main = Main{items:[app]}
            app = Sub { name:"Makepad", items:[quit] }
            quit = Item {
                name:"Quit",
                shift: false,
                key: KeyQ,
                enabled: true
            }
        }
        body = <KeyboardView> {
            width: Fill, height: Fill,
            keyboard_min_shift: 30,
        }

        cursor: Default
        mouse_cursor_size: vec2(20, 20),
        draw_cursor: {
            instance border_width: 1.5
            instance color: (THEME_COLOR_CURSOR_BG)
            instance border_color: (THEME_COLOR_CURSOR_BORDER)

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

    Splitter = <SplitterBase> {
        draw_splitter: {
            uniform border_radius: 1.0
            uniform splitter_pad: 1.0
            uniform splitter_grabber: 110.0

            instance pressed: 0.0
            instance hover: 0.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(THEME_COLOR_D_HIDDEN);

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
                    THEME_COLOR_D_HIDDEN,
                    mix(
                        THEME_COLOR_CTRL_HOVER,
                        THEME_COLOR_CTRL_PRESSED,
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
                    from: { all: Forward { duration: 0.1 }}
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
        height: 10.0, width: 10.0,
        margin: { right: 5 },
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
        width: Fit, height: Fill, //Fixed((THEME_TAB_HEIGHT)),

        align: {x: 0.0, y: 0.5}
        padding: <THEME_MSPACE_3> {}

        close_button: <TabCloseButton> {}
        draw_name: {
            text_style: <THEME_FONT_REGULAR> {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_CTRL_SELECTED,
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
                    THEME_COLOR_BG_CONTAINER,
                    THEME_COLOR_CTRL_SELECTED,
                    self.selected
                )
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
            color: (THEME_COLOR_BG_CONTAINER)
        }
        draw_fill: {
            color: (THEME_COLOR_BG_CONTAINER)
        }

        width: Fill, height: (THEME_TAB_HEIGHT)

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

    Dock = <DockBase> {
        round_corner: {
            draw_depth: 6.0
            border_radius: (THEME_CONTAINER_CORNER_RADIUS)
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
                return sdf.fill(THEME_COLOR_BG_APP); // TODO: This should be a transparent color instead
            }
        }
        border_size: (THEME_DOCK_BORDER_SIZE)

        flow: Down
        padding: {left: (THEME_DOCK_BORDER_SIZE), top: 0, right: (THEME_DOCK_BORDER_SIZE), bottom: (THEME_DOCK_BORDER_SIZE)}
        padding_fill: {color: (THEME_COLOR_BG_APP)} // TODO: unclear what this does
        drag_quad: {
            draw_depth: 10.0
            color: (THEME_COLOR_DRAG_QUAD)
        }
        tab_bar: <TabBar> {}
        splitter: <Splitter> {}
    }

    PopupMenuItem = <PopupMenuItemBase> {
        width: Fill, height: Fit,
        align: { y: 0.5 }
        padding: <THEME_MSPACE_1> { left: 15. }

        draw_name: {
            instance selected: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_CTRL_SELECTED,
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
            instance color: (THEME_COLOR_FLOATING_BG)
            instance color_selected: (THEME_COLOR_CTRL_HOVER)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                sdf.clear(mix(
                    self.color,
                    self.color_selected,
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
                sdf.stroke(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_TEXT_DEFAULT, self.selected), 1.0);

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
        width: 150., height: Fit,
        flow: Down,
        padding: <THEME_MSPACE_1> {}

        menu_item: <PopupMenuItem> {}

        draw_bg: {
            instance color: (THEME_COLOR_FLOATING_BG)
            instance border_width: 1.0,
            instance inset: vec4(0.0, 0.0, 0.0, 0.0),
            instance radius: 2.0
            instance blur: 0.0

            fn get_color(self) -> vec4 {
                return self.color
            }

            fn get_border_color(self) -> vec4 {
                return mix(THEME_COLOR_BEVEL_RIMLIGHT, THEME_COLOR_BEVEL_SHADOW, pow(self.pos.y, 0.35))
            }

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.blur = self.blur
                sdf.box(
                    self.inset.x + self.border_width,
                    self.inset.y + self.border_width,
                    self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                    self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), THEME_BEVEL_BORDER)
                }
                return sdf.result;
            }
        }
    }

    DropDown = <DropDownBase> {
        width: Fit, height: Fit,
        padding: <THEME_MSPACE_1> { left: (THEME_SPACE_2), right: 20. }
        align: {x: 0., y: 0.}

        draw_text: {
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }

            fn get_color(self) -> vec4 {
                return mix(
                    THEME_COLOR_TEXT_DEFAULT,
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                )
            }
        }

        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CTRL_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_U_HIDDEN)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), self.bodybottom, self.focus);
                let body_transp = vec4(body.xyz, 0.0);

                let top_gradient = mix(
                    body_transp,
                    mix(
                        mix(
                            THEME_COLOR_U_HIDDEN,
                            THEME_COLOR_BEVEL_RIMLIGHT,
                            self.hover
                        ),
                        THEME_COLOR_BEVEL_RIMLIGHT,
                        self.focus
                    ),
                    max(0.0, grad_top - sdf.pos.y) / grad_top);

                let bot_gradient = mix(
                    mix(body_transp, THEME_COLOR_BEVEL_SHADOW, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

                // the little drop shadow at the bottom
                let shift_inward = self.border_radius * 1.75;
                sdf.move_to(shift_inward, self.rect_size.y);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
                sdf.stroke(mix(
                    mix(
                        THEME_COLOR_D_HIDDEN,
                        THEME_COLOR_BEVEL_SHADOW,
                        self.hover
                    ),
                    THEME_COLOR_BEVEL_SHADOW,
                    self.focus
                    ), THEME_BEVEL_BORDER
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
                    THEME_BEVEL_BORDER * 1.5
                )

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 3.;
                let offset = 1.;

                sdf.move_to(c.x - sz, c.y - sz + offset);
                sdf.line_to(c.x + sz, c.y - sz + offset);
                sdf.line_to(c.x, c.y + sz * 0.25 + offset);
                sdf.close_path();

                sdf.fill(mix(THEME_COLOR_TEXT_DEFAULT, THEME_COLOR_TEXT_HOVER, self.hover));

                return sdf.result
            }
        }

        popup_menu: <PopupMenu> {}

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
                    from: {all: Forward {duration: 0.2}}
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
        align: {y: 0.5}
        padding: { left: (THEME_SPACE_1) },
        is_folder: false,
        indent_width: 10.0
        min_drag_distance: 10.0

        draw_bg: {
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                    THEME_COLOR_BG_EVEN,
                    THEME_COLOR_BG_ODD,
                    self.is_even
                ),
                THEME_COLOR_CTRL_SELECTED,
                self.selected)
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
                    THEME_COLOR_TEXT_DEFAULT * self.scale,
                    THEME_COLOR_CTRL_SELECTED,
                    self.selected
                ));
            }
        }

        draw_name: {
            fn get_color(self) -> vec4 {
                return mix(
                    THEME_COLOR_TEXT_DEFAULT * self.scale,
                    THEME_COLOR_CTRL_SELECTED,
                    self.selected
                )
            }

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
                top_drop: 1.2,
            }
        }

        icon_walk: {
            width: (THEME_DATA_ICON_WIDTH - 2), height: (THEME_DATA_ICON_HEIGHT),
            margin: { right: 3.0 }
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
    }

    FileTree = <FileTreeBase> {
        flow: Down,

        scroll_bars: <ScrollBars> {}
        scroll_bars: {}
        node_height: (THEME_DATA_ITEM_HEIGHT),
        clip_x: true,
        clip_y: true

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

        filler: { // TODO: Clarify what this is for. Appears not to do anything.
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EVEN,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    mix(
                        THEME_COLOR_CTRL_INACTIVE,
                        THEME_COLOR_CTRL_SELECTED,
                        self.focussed
                    ),
                    self.selected
                );
            }
        }
    }

    FoldButton = <FoldButtonBase> {
        width: 12., height: 12.,

        draw_bg: {
            instance open: 0.0
            instance hover: 0.0
            uniform fade: 1.0

            fn pixel(self) -> vec4 {
                let sz = 2.5;
                let c = vec2(5.0, 0.6 * self.rect_size.y);
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(vec4(0.));

                // we have 3 points, and need to rotate around its center
                sdf.rotate(self.open * 0.5 * PI + 0.5 * PI, c.x, c.y);
                sdf.move_to(c.x - sz, c.y + sz);
                sdf.line_to(c.x, c.y - sz);
                sdf.line_to(c.x + sz, c.y + sz);
                sdf.close_path();
                sdf.fill(mix(
                    THEME_COLOR_TEXT_DEFAULT,
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                )
                );
                return sdf.result * self.fade;
            }
        }

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
        width: Fill, height: Fit,
        body_walk: { width: Fill, height: Fit}

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
        instance hover: 0.0
        instance pressed: 0.0

        width: Fit, height: Fit,
        padding: { bottom: 2. }
        spacing: 7.5,
        align: {x: 0., y: 0.}

        label_walk: {
            width: Fit, height: Fit,
        },

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
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                ), mix(.7, 1., self.hover));
            }
        }

        draw_text: {
            wrap: Word
            instance pressed: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_PRESSED,
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

    }

    LinkLabelIcon = <LinkLabel> {
        padding: { bottom: 2. }
        label_walk: { margin: { left: -5. }},
        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance pressed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_META,
                    self.pressed
                )
            }
        }
    }

    RadioButton = <RadioButtonBase> {
        width: Fit, height: 16.,
        align: { x: 0.0, y: 0.5 }

        icon_walk: { margin: { left: 20. } }

        label_walk: {
            width: Fit, height: Fit,
            margin: { left: 20. }
        }
        label_align: { y: 0.0 }

        draw_radio: {
            uniform size: 7.0;
            // uniform color_active: (THEME_COLOR_U_1)
            // uniform color_inactive: (THEME_COLOR_D_1)

            // instance pressed: 0.0
            uniform border_radius: (THEME_CTRL_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_CTRL_DEFAULT)
            instance bodybottom: (THEME_COLOR_CTRL_ACTIVE)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.radio_type {
                    RadioType::Round => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_RIMLIGHT, self.pos.y), (THEME_BEVEL_BORDER))
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(
                            mix(
                                mix(
                                    THEME_COLOR_U_HIDDEN,
                                    THEME_COLOR_CTRL_HOVER,
                                    self.hover
                                ),
                                THEME_COLOR_TEXT_ACTIVE,
                                self.selected
                            )
                        );
                    }
                    RadioType::Tab => {
                        let grad_top = 5.0;
                        let grad_bot = 1.0;
                        let body = mix(
                            mix(self.bodytop, (THEME_COLOR_CTRL_HOVER), self.hover),
                            self.bodybottom,
                            self.selected
                        );
                        let body_transp = vec4(body.xyz, 0.0);
                        let top_gradient = mix(body_transp, mix(THEME_COLOR_BEVEL_RIMLIGHT, THEME_COLOR_BEVEL_SHADOW, self.selected), max(0.0, grad_top - sdf.pos.y) / grad_top);
                        let bot_gradient = mix(
                            mix(body_transp, THEME_COLOR_BEVEL_RIMLIGHT, self.selected),
                            top_gradient,
                            clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                        );

                        // the little drop shadow at the bottom
                        let shift_inward = 0. * 1.75;
                        sdf.move_to(shift_inward, self.rect_size.y);
                        sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
                        sdf.stroke(
                            mix(
                                mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_RIMLIGHT, self.hover),
                                THEME_COLOR_BEVEL_RIMLIGHT,
                                self.selected
                            ), THEME_DOCK_BORDER_SIZE)

                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x - 2.0,
                            self.rect_size.y - 2.0,
                            1.
                        )
                        sdf.fill_keep(body)

                        sdf.stroke(bot_gradient, THEME_BEVEL_BORDER * 1.5)
                    }
                }
                return sdf.result
            }
        }

        draw_text: {
            instance hover: 0.0
            instance selected: 0.0

            uniform color_unselected: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_unselected_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_selected: (THEME_COLOR_TEXT_SELECTED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected,
                        // self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_unselected,
                    // self.color_selected,
                    self.selected
                )
            }
        }

        draw_icon: {
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_INSET_PIT_TOP,
                        THEME_COLOR_CTRL_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_ACTIVE,
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
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
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

    RadioButtonCustom = <RadioButton> {
        height: Fit,
        draw_radio: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
        margin: { left: -17.5 }
        label_walk: {
            width: Fit, height: Fit,
            margin: { left: (THEME_SPACE_1) }
        }
    }

    RadioButtonTextual = <RadioButton> {
        height: Fit,
        draw_radio: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
        label_walk: {
            margin: 0.,
            width: Fit, height: Fit,
        }
        draw_text: {
            instance hover: 0.0
            instance selected: 0.0

            uniform color_unselected: (THEME_COLOR_TEXT_INACTIVE)
            uniform color_unselected_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_selected: (THEME_COLOR_TEXT_SELECTED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
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
    }

    RadioButtonTab = <RadioButton> {
        height: Fit,
        draw_radio: { radio_type: Tab }
        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1)}

        draw_text: {
            instance hover: 0.0
            instance selected: 0.0

            uniform color_unselected: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_unselected_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_selected: (THEME_COLOR_TEXT_HOVER)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected,
                        // self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_selected,
                    self.selected
                )
            }
        }
    }

    ButtonGroup = <CachedRoundedView> {
        height: Fit, width: Fit,
        spacing: 0.0,
        flow: Right
        align: { x: 0.0, y: 0.5 }
        draw_bg: {
            radius: 4.
        }
    }

    PortalList = <PortalListBase> {
        width: Fill, height: Fill,
        capture_overload: true
        scroll_bar: <ScrollBar> {}
        flow: Down
    }

    FlatList = <FlatListBase> {
        width: Fill, height: Fill,
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
        width: 200, height: Fit,
        padding: <THEME_MSPACE_2> {}

        label_align: {y: 0.}
        cursor_margin_bottom: (THEME_SPACE_1),
        cursor_margin_top: (THEME_SPACE_1),
        select_pad_edges: 3.0
        cursor_size: 2.0,
        numeric_only: false,
        on_focus_select_all: false,
        empty_message: "0",
        clip_x: false, clip_y: false,

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            wrap: Word,
            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return
                mix(
                    mix(
                        mix(THEME_COLOR_TEXT_DEFAULT, THEME_COLOR_TEXT_HOVER, self.hover),
                        THEME_COLOR_TEXT_FOCUSED,
                        self.focus
                    ),
                    mix(THEME_COLOR_TEXT_PLACEHOLDER, THEME_COLOR_TEXT_DEFAULT, self.hover),
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
                sdf.fill(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_TEXT_CURSOR, self.focus));
                return sdf.result
            }
        }

        draw_select: {
            instance hover: 0.0
            instance focus: 0.0
            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
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
                sdf.fill(
                    mix(THEME_COLOR_U_HIDDEN,
                    THEME_COLOR_BG_HIGHLIGHT_INLINE,
                    self.focus)
                ); // Pad color
                return sdf.result
            }
        }

        draw_bg: {
            instance radius: (THEME_CTRL_CORNER_RADIUS)
            instance hover: 0.0
            instance focus: 0.0
            instance bodytop: (THEME_COLOR_INSET_DEFAULT)
            instance bodybottom: (THEME_COLOR_CTRL_ACTIVE)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.5;

                let body = mix(
                    self.bodytop,
                    self.bodybottom,
                    self.focus
                );

                let body_transp = (THEME_COLOR_D_HIDDEN)

                let top_gradient = mix(
                    body_transp,
                    THEME_COLOR_BEVEL_SHADOW,
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );

                let bot_gradient = mix(
                    (THEME_COLOR_BEVEL_RIMLIGHT),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

                // some rim-light at the bottom
                let shift_inward = self.radius * 1.75;
                sdf.move_to(shift_inward, self.rect_size.y);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.radius
                )

                sdf.fill_keep(body)

                sdf.stroke(
                    bot_gradient,
                    THEME_BEVEL_BORDER * 0.9
                )

                return sdf.result
            }
        }

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
                    from: {all: Forward {duration: .25}}
                    apply: {
                        draw_cursor: {focus: 0.0},
                        draw_bg: {focus: 0.0},
                        draw_select: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_cursor: {focus: 1.0},
                        draw_bg: {focus: 1.0},
                        draw_select: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    Slider = <SliderBase> {
        min: 0.0, max: 1.0,
        step: 0.0,
        label_align: { y: 0.0 }
        margin: <THEME_MSPACE_1> {}
        precision: 2,
        height: Fit

        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float

            fn pixel(self) -> vec4 {
                let slider_height = 3;
                let nub_size = mix(3, 5, self.hover);
                let nubbg_size = mix(0, 13, self.hover)

                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                let slider_bg_color = mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.focus);
                let slider_color = mix(
                    mix(THEME_COLOR_AMOUNT_DEFAULT, THEME_COLOR_AMOUNT_HOVER, self.hover),
                    THEME_COLOR_AMOUNT_ACTIVE,
                    self.focus);

                let nub_color = (THEME_COLOR_SLIDER_NUB_DEFAULT);
                let nubbg_color = mix(THEME_COLOR_SLIDER_NUB_HOVER, THEME_COLOR_SLIDER_NUB_ACTIVE, self.drag);

                match self.slider_type {
                    SliderType::Horizontal => {

                        sdf.rect(0, self.rect_size.y - slider_height, self.rect_size.x, slider_height)
                        sdf.fill(slider_bg_color);

                        sdf.rect(0, self.rect_size.y - slider_height * 0.3, self.rect_size.x, slider_height)
                        sdf.fill(THEME_COLOR_BEVEL_RIMLIGHT);

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
            color: (THEME_COLOR_TEXT_DEFAULT),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }

        label_walk: { width: Fill, height: Fill }

        text_input: <TextInput> {
            width: Fit, padding: 0.,
            cursor_margin_bottom: (THEME_SPACE_1),
            cursor_margin_top: (THEME_SPACE_1),
            select_pad_edges: 3.0
            cursor_size: 2.0,
            empty_message: "0",
            numeric_only: true,

            label_align: {y: 0.},
            margin: { bottom: (THEME_SPACE_2), left: (THEME_SPACE_2) }
            draw_bg: {
                instance radius: 1.0
                instance border_width: 0.0
                instance border_color: (#f00) // TODO: This appears not to do anything.
                instance inset: vec4(0.0, 0.0, 0.0, 0.0)
                instance focus: 0.0,
                color: (THEME_COLOR_D_HIDDEN)
                instance color_selected: (THEME_COLOR_D_HIDDEN)

                fn get_color(self) -> vec4 {
                    return mix(self.color, self.color_selected, self.focus)
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
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: OutQuad
                    apply: {
                        draw_slider: {hover: 0.0}
                        // text_input: { draw_bg: { hover: 0.0}}
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: {hover: 1.0}
                        // text_input: { draw_bg: { hover: 1.0}}
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

    SlidesView = <SlidesViewBase> {
        anim_speed: 0.9
    }

    Slide = <RoundedView> {
        width: Fill, height: Fill,
        flow: Down, spacing: 10,
        align: { x: 0.0, y: 0.5 }
        padding: 50.
        draw_bg: { color: (THEME_COLOR_SLIDES_BG), radius: (THEME_CONTAINER_CORNER_RADIUS) }
        title = <H1> {
            text: "SlideTitle",
            draw_text: {color: (THEME_COLOR_TEXT_DEFAULT) }
        }
    }

    SlideChapter = <Slide> {
        width: Fill, height: Fill,
        flow: Down,
        align: {x: 0.0, y: 0.5}
        spacing: 10,
        padding: 50,
        draw_bg: {color: (THEME_COLOR_SLIDES_CHAPTER), radius: (THEME_CONTAINER_CORNER_RADIUS)}
        title = <H1> {
            text: "SlideTitle",
            draw_text: {color: (THEME_COLOR_TEXT_DEFAULT) }
        }
    }

    SlideBody = <H2> {
        text: "Body of the slide"
        draw_text: {color: (THEME_COLOR_TEXT_DEFAULT) }
    }

    DrawScrollShadow = <DrawScrollShadowBase> {

        shadow_size: 4.0,

        fn pixel(self) -> vec4 { // TODO: make the corner overlap properly with a distance field eq.
            let is_viz = clamp(self.scroll * 0.1, 0., 1.);
            let pos = self.pos;
            let base = THEME_COLOR_BG_CONTAINER.xyz;
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
            color: (THEME_COLOR_APP_CAPTION_BAR)
        }

        content = <View> {
            width: Fill, height: Fit,
            flow: Overlay,

            title_container = <View> {
                width: Fill, height: Fit,
                align: {x: 0.5, y: 0.5}

                title = <H4> { text: "Stack View Title" }
            }

            button_container = <View> {
                left_button = <Button> {
                    width: Fit, height: 68,
                    icon_walk: {width: 10, height: 68}
                    draw_bg: {
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            return sdf.result
                        }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/back.svg"),
                        color: (THEME_COLOR_TEXT_DEFAULT);
                        brightness: 0.8;
                    }
                }
            }
        }
    }

    StackNavigationView = <StackNavigationViewBase> {
        visible: false
        width: Fill, height: Fill,
        flow: Overlay

        show_bg: true
        draw_bg: {
            color: (THEME_COLOR_WHITE)
        }

        // Empty slot to place a generic full-screen background
        background = <View> {
            width: Fill, height: Fill,
            visible: false
        }

        body = <View> {
            width: Fill, height: Fill,
            flow: Down,

            // THEME_SPACE between body and header can be adjusted overriding this margin
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