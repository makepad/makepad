use crate::makepad_platform::*;

live_design! {
    import makepad_draw::shader::std::*;
    import crate::base::*;

    // GLOBAL PARAMETERS
    THEME_COLOR_CONTRAST = 1.0
    THEME_COLOR_TINT = #f00
    THEME_COLOR_TINT_AMOUNT = 0.0
    THEME_SPACE_FACTOR = 6. // Increase for a less dense layout
    THEME_CORNER_RADIUS = 2.5
    THEME_BEVELING = 0.75
    THEME_FONT_SIZE_BASE = 7.5
    THEME_FONT_SIZE_CONTRAST = 2.5// Greater values = greater font-size steps between font-formats (i.e. from H3 to H2)

    // DIMENSIONS
    THEME_SPACE_1 = (0.5 * (THEME_SPACE_FACTOR))
    THEME_SPACE_2 = (1.0 * (THEME_SPACE_FACTOR))
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

    THEME_CONTAINER_CORNER_RADIUS = (THEME_CORNER_RADIUS * 2.)
    THEME_TEXTSELECTION_CORNER_RADIUS = (THEME_CORNER_RADIUS * .5)
    THEME_TAB_HEIGHT = 32.0,
    THEME_SPLITTER_HORIZONTAL = 16.0,
    THEME_SPLITTER_SIZE = 10.0,
    THEME_SPLITTER_MIN_HORIZONTAL = (THEME_TAB_HEIGHT),
    THEME_SPLITTER_MAX_HORIZONTAL = (THEME_TAB_HEIGHT + THEME_SPLITTER_SIZE),
    THEME_SPLITTER_MIN_VERTICAL = (THEME_SPLITTER_HORIZONTAL),
    THEME_SPLITTER_MAX_VERTICAL = (THEME_SPLITTER_HORIZONTAL + THEME_SPLITTER_SIZE),
    THEME_SPLITTER_SIZE = 5.0
    THEME_DOCK_BORDER_SIZE: 0.0

    // COLOR PALETTE
    // HIGHER VALUE = HIGHER CONTRAST, RECOMMENDED VALUES: 0.5 - 2.5

    THEME_COLOR_W = #FFFFFFFF
    THEME_COLOR_W_H = #FFFFFF00
    THEME_COLOR_B = #000000FF
    THEME_COLOR_B_H = #00000000

    THEME_COLOR_WHITE = (mix(THEME_COLOR_W, #FFFFFF00, pow(0.1, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_5 = (mix(THEME_COLOR_W, THEME_COLOR_W_H, pow(0.35, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_4 = (mix(THEME_COLOR_W, THEME_COLOR_W_H, pow(0.6, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_3 = (mix(THEME_COLOR_W, THEME_COLOR_W_H, pow(0.75, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_2 = (mix(THEME_COLOR_W, THEME_COLOR_W_H, pow(0.9, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_1 = (mix(THEME_COLOR_W, THEME_COLOR_W_H, pow(0.95, THEME_COLOR_CONTRAST)))
    THEME_COLOR_U_HIDDEN = (THEME_COLOR_W_H)

    THEME_COLOR_D_HIDDEN = (THEME_COLOR_B_H)
    THEME_COLOR_D_1 = (mix(THEME_COLOR_B, THEME_COLOR_B_H, pow(0.85, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_2 = (mix(THEME_COLOR_B, THEME_COLOR_B_H, pow(0.75, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_3 = (mix(THEME_COLOR_B, THEME_COLOR_B_H, pow(0.6, THEME_COLOR_CONTRAST)))
    THEME_COLOR_D_4 = (mix(THEME_COLOR_B, THEME_COLOR_B_H, pow(0.4, THEME_COLOR_CONTRAST)))
    THEME_COLOR_BLACK = (mix(THEME_COLOR_B, THEME_COLOR_B_H, pow(0.1, THEME_COLOR_CONTRAST)))

    // BASICS
    THEME_COLOR_MAKEPAD = #FF5C39FF

    THEME_COLOR_BG_APP = (mix(
        mix(THEME_COLOR_B, THEME_COLOR_TINT, THEME_COLOR_TINT_AMOUNT),
        mix(THEME_COLOR_W, THEME_COLOR_TINT, THEME_COLOR_TINT_AMOUNT),
        pow(0.3, THEME_COLOR_CONTRAST)))
    THEME_COLOR_FG_APP = (mix(
        mix(THEME_COLOR_B, THEME_COLOR_TINT, THEME_COLOR_TINT_AMOUNT),
        mix(THEME_COLOR_W, THEME_COLOR_TINT, THEME_COLOR_TINT_AMOUNT),
        pow(0.36, THEME_COLOR_CONTRAST))
    )
    THEME_COLOR_BG_HIGHLIGHT = (THEME_COLOR_FG_APP)
    THEME_COLOR_BG_UNFOCUSSED = (THEME_COLOR_BG_HIGHLIGHT * 0.85)
    THEME_COLOR_APP_CAPTION_BAR = (THEME_COLOR_D_HIDDEN)
    THEME_COLOR_DRAG_QUAD = (THEME_COLOR_U_5)

    THEME_COLOR_CURSOR_BG = (THEME_COLOR_BLACK)
    THEME_COLOR_CURSOR_BORDER = (THEME_COLOR_WHITE)

    THEME_COLOR_TEXT_DEFAULT = (THEME_COLOR_U_5)
    THEME_COLOR_TEXT_DEFAULT_DARK = (THEME_COLOR_D_4)
    THEME_COLOR_TEXT_HL = (THEME_COLOR_TEXT_DEFAULT)

    THEME_COLOR_TEXT_PRESSED = (THEME_COLOR_U_4)
    THEME_COLOR_TEXT_HOVER = (THEME_COLOR_WHITE)
    THEME_COLOR_TEXT_ACTIVE = (THEME_COLOR_U_5)
    THEME_COLOR_TEXT_INACTIVE = (THEME_COLOR_U_5)
    THEME_COLOR_TEXT_SELECTED = (THEME_COLOR_WHITE)
    THEME_COLOR_TEXT_FOCUSED = (THEME_COLOR_U_5)
    THEME_COLOR_TEXT_PLACEHOLDER = (THEME_COLOR_U_4)
    THEME_COLOR_TEXT_META = (THEME_COLOR_U_4)

    THEME_COLOR_TEXT_CURSOR = (THEME_COLOR_WHITE)

    THEME_COLOR_BG_CONTAINER = (THEME_COLOR_D_3 * 0.8)
    THEME_COLOR_BG_EVEN = (THEME_COLOR_BG_CONTAINER * 0.875)
    THEME_COLOR_BG_ODD = (THEME_COLOR_BG_CONTAINER * 1.125)
    THEME_COLOR_BG_HIGHLIGHT = (THEME_COLOR_U_1) // Code-blocks and quotes.
    THEME_COLOR_BG_HIGHLIGHT_INLINE = (THEME_COLOR_U_3) // i.e. inline code

    THEME_COLOR_BEVEL_LIGHT = (THEME_COLOR_U_3)
    THEME_COLOR_BEVEL_SHADOW = (THEME_COLOR_D_3)

    // WIDGET COLORS
    THEME_COLOR_CTRL_DEFAULT = (THEME_COLOR_U_1)
    THEME_COLOR_CTRL_PRESSED = (THEME_COLOR_D_1)
    THEME_COLOR_CTRL_HOVER = (THEME_COLOR_U_2)
    THEME_COLOR_CTRL_ACTIVE = (THEME_COLOR_D_2)
    THEME_COLOR_CTRL_SELECTED = (THEME_COLOR_U_2)
    THEME_COLOR_CTRL_INACTIVE = (THEME_COLOR_D_HIDDEN)

    THEME_COLOR_FLOATING_BG = #505050FF // Elements that live on top of the UI like dialogs, popovers, and context menus.

    // Background of textinputs, radios, checkboxes etc.
    THEME_COLOR_INSET_DEFAULT = (THEME_COLOR_D_1)
    THEME_COLOR_INSET_PIT_TOP = (THEME_COLOR_D_4)
    THEME_COLOR_INSET_PIT_TOP_HOVER = (THEME_COLOR_D_4)
    THEME_COLOR_INSET_PIT_BOTTOM = (THEME_COLOR_D_HIDDEN)

    // Progress bars, slider amounts etc.
    THEME_COLOR_AMOUNT_DEFAULT = (THEME_COLOR_U_3)
    THEME_COLOR_AMOUNT_DEFAULT_BIG = #A
    THEME_COLOR_AMOUNT_HOVER = (THEME_COLOR_U_4)
    THEME_COLOR_AMOUNT_ACTIVE = (THEME_COLOR_U_5)
    THEME_COLOR_AMOUNT_TRACK_DEFAULT = (THEME_COLOR_D_3)
    THEME_COLOR_AMOUNT_TRACK_HOVER = (THEME_COLOR_D_3)
    THEME_COLOR_AMOUNT_TRACK_ACTIVE = (THEME_COLOR_D_4)

    // WIDGET SPECIFIC COLORS
    THEME_COLOR_DIVIDER = (THEME_COLOR_D_4)

    THEME_COLOR_SLIDER_NUB_DEFAULT = (THEME_COLOR_WHITE)
    THEME_COLOR_SLIDER_NUB_HOVER = (THEME_COLOR_WHITE)
    THEME_COLOR_SLIDER_NUB_ACTIVE = (THEME_COLOR_WHITE)

    THEME_COLOR_SLIDES_CHAPTER = (THEME_COLOR_MAKEPAD)
    THEME_COLOR_SLIDES_BG = (THEME_COLOR_D_4)

    THEME_COLOR_SLIDER_BIG_NUB_TOP = #8
    THEME_COLOR_SLIDER_BIG_NUB_TOP_HOVER = #A
    THEME_COLOR_SLIDER_BIG_NUB_BOTTOM = #282828
    THEME_COLOR_SLIDER_BIG_NUB_BOTTOM_HOVER = #3

    THEME_COLOR_CTRL_SCROLLBAR_HOVER = (THEME_COLOR_U_3)

    THEME_COLOR_DOCK_CONTAINER = (THEME_COLOR_BG_CONTAINER)
    THEME_COLOR_DOCK_TAB_SELECTED = (THEME_COLOR_FG_APP)
    THEME_COLOR_DOCK_TAB_SELECTED_MINIMAL = (THEME_COLOR_U_4)


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
    THEME_FONT_SIZE_CODE = 9.0
    THEME_FONT_LINE_SPACING = 1.43

    THEME_FONT_SIZE_1 = (THEME_FONT_SIZE_BASE + 16 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_2 = (THEME_FONT_SIZE_BASE + 8 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_3 = (THEME_FONT_SIZE_BASE + 4 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_4 = (THEME_FONT_SIZE_BASE + 2 * THEME_FONT_SIZE_CONTRAST)
    THEME_FONT_SIZE_P = (THEME_FONT_SIZE_BASE + 1 * THEME_FONT_SIZE_CONTRAST)

    THEME_FONT_LABEL = {
        font: { path: dep("crate://self/resources/IBMPlexSans-Text.ttf") },
        font2: { path: dep("crate://self/resources/LXGWWenKaiRegular.ttf") },
    } // TODO: LEGACY, REMOVE. REQUIRED BY RUN LIST IN STUDIO ATM
    THEME_FONT_REGULAR = {
        font: { path: dep("crate://self/resources/IBMPlexSans-Text.ttf") }
        font2: { path: dep("crate://self/resources/LXGWWenKaiRegular.ttf") },
    }
    THEME_FONT_BOLD = {
        font: { path: dep("crate://self/resources/IBMPlexSans-SemiBold.ttf") }
        font2: { path: dep("crate://self/resources/LXGWWenKaiBold.ttf") },
    }
    THEME_FONT_ITALIC = {
        font: { path: dep("crate://self/resources/IBMPlexSans-Italic.ttf") }
        font2: { path: dep("crate://self/resources/LXGWWenKaiRegular.ttf") },
    }
    THEME_FONT_BOLD_ITALIC = {
        font: { path: dep("crate://self/resources/IBMPlexSans-BoldItalic.ttf") },
        font2: { path: dep("crate://self/resources/LXGWWenKaiBold.ttf") },
    }
    THEME_FONT_CODE = {
        font: { path: dep("crate://self/resources/LiberationMono-Regular.ttf") }
        font_size: (THEME_FONT_SIZE_CODE)
        //brightness: 1.1
        line_scale: 1.2,
        line_spacing: 1.16
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
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
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H4"
    }

    P = <Label> {
        width: Fill,
        margin: 0.,
        padding: 0.,
        // margin: {top: (THEME_SPACE_2), bottom: (THEME_FONT_SIZE_P * 0.5)}
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
        margin: {top: (THEME_SPACE_2), bottom: (THEME_FONT_SIZE_P * 0.5)}
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
        margin: {top: (THEME_SPACE_2), bottom: (THEME_FONT_SIZE_P * 0.5)}
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
        margin: {top: (THEME_SPACE_2), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT_DEFAULT)
        }
        text: "Paragraph"
    }

    Hr = <View> {
        width: Fill, height: Fit,
        flow: Down,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}
        <View> {
            width: Fill, height: (THEME_BEVELING * 2.0),
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_SHADOW) }
        }
        <View> {
            width: Fill, height: (THEME_BEVELING * 0.5),
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_LIGHT) }
        }
    }

//    TODO: enable once Makepad's layout supports Fill that knows how high adjacent elements are. For now this is not possible.
//    Vr = <View> {
//         width: Fit, height: Fill,
//         flow: Right,
//         spacing: 0.,
//         margin: <THEME_MSPACE_V_2> {}
//         <View> {
//             width: (THEME_BEVELING * 2.0), height: Fill
//             show_bg: true,
//             draw_bg: { color: #f00 }
//         }
//         <View> {
//             width: (THEME_BEVELING * 0.5), height: Fill,
//             show_bg: true,
//             draw_bg: { color: #f0f }
//         }
//     }

    // Spacer = <View> { width: Fill, height: Fill }
    Filler = <View> { width: Fill, height: Fill }


    LinkLabel = <LinkLabelBase> {
        // TODO: add a focus states
        instance hover: 0.0
        instance pressed: 0.0

        width: Fit, height: Fit,
        margin: <THEME_MSPACE_2> {}
        padding: 0.,

        label_walk: { width: Fit, height: Fit, },

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
            color: (THEME_COLOR_TEXT_DEFAULT),
            instance color_hover: (THEME_COLOR_TEXT_HOVER),
            instance color_pressed: (THEME_COLOR_TEXT_PRESSED),
            instance pressed: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }
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
            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        mix(self.color, #f, 0.5),
                        self.hover
                    ),
                    self.color * 0.75,
                    self.pressed
                )
            }
        }
    }

    HtmlLink = <HtmlLinkBase> {
        width: Fit, height: Fit,
        align: {x: 0., y: 0.}

        color: #x0000EE,
        hover_color: #x00EE00,
        pressed_color: #xEE0000,
        
        // instance hovered: 0.0
        // instance pressed: 0.0

        animator: {
            hover = {
                default: off,
                off = {
                    redraw: true,
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: 0.0,
                        pressed: 0.0,
                    }
                }

                on = {
                    redraw: true,
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        hovered: [{time: 0.0, value: 1.0}],
                        pressed: [{time: 0.0, value: 1.0}],
                    }
                }

                pressed = {
                    redraw: true,
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: [{time: 0.0, value: 1.0}],
                        pressed: [{time: 0.0, value: 1.0}],
                    }
                }
            }
        }
    }

    Html = <HtmlBase> {
        width: Fill, height: Fit,
        flow: RightWrap,
        width:Fill,
        height:Fit,
        padding: <THEME_MSPACE_1> {}

        font_size: (THEME_FONT_SIZE_P),
        font_color: (THEME_COLOR_TEXT_DEFAULT),

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

        quote_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        quote_walk: { width: Fill, height: Fit, }

        list_item_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_1> {}
        }
        list_item_walk: {
            height: Fit, width: Fill,
        }

        inline_code_padding: <THEME_MSPACE_1> {},
        inline_code_margin: <THEME_MSPACE_1> {},

        sep_walk: {
            width: Fill, height: 4.
            margin: <THEME_MSPACE_V_1> {}
        }

        a = <HtmlLink> {}

        draw_block:{
            line_color: (THEME_COLOR_TEXT_DEFAULT)
            sep_color: (THEME_COLOR_DIVIDER)
            quote_bg_color: (THEME_COLOR_BG_HIGHLIGHT)
            quote_fg_color: (THEME_COLOR_TEXT_DEFAULT)
            code_color: (THEME_COLOR_BG_HIGHLIGHT)
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
                        sdf.fill(self.quote_bg_color)
                        sdf.box(
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            self.rect_size.y - THEME_SPACE_2,
                            1.5
                        );
                        sdf.fill(self.quote_fg_color);
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
                        sdf.fill(self.sep_color);
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
                        sdf.fill(self.code_color);
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
                        sdf.fill(self.code_color);
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
                        sdf.fill(self.line_color);
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
                        sdf.fill(self.line_color);
                        return sdf.result;
                    }
                }
                return #f00
            }
        }
    }
    
    TextFlowLink = <TextFlowLinkBase> {
        color: #xa,
        hover_color: #xf,
        pressed_color: #x3,
        
        margin:{right:5}
        
        animator: {
            hover = {
                default: off,
                off = {
                    redraw: true,
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: 0.0,
                        pressed: 0.0,
                    }
                }
                
                on = {
                    redraw: true,
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        hovered: [{time: 0.0, value: 1.0}],
                        pressed: [{time: 0.0, value: 1.0}],
                    }
                }
                
                pressed = {
                    redraw: true,
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: [{time: 0.0, value: 1.0}],
                        pressed: [{time: 0.0, value: 1.0}],
                    }
                }
            }
        }
    }
    
    TextFlow = <TextFlowBase> {
        width: Fill, height: Fit,
        flow: RightWrap,
        width:Fill,
        height:Fit,
        padding: 0
        
        font_size: (THEME_FONT_SIZE_P),
        font_color: (THEME_COLOR_TEXT_DEFAULT),
        
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
        
        quote_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        quote_walk: { width: Fill, height: Fit, }
        
        list_item_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_1> {}
        }
        list_item_walk: {
            height: Fit, width: Fill,
        }
        
        inline_code_padding: <THEME_MSPACE_1> {},
        inline_code_margin: <THEME_MSPACE_1> {},
        
        sep_walk: {
            width: Fill, height: 4.
            margin: <THEME_MSPACE_V_1> {}
        }
        
        link = <TextFlowLink> {}
        
        draw_block:{
            line_color: (THEME_COLOR_TEXT_DEFAULT)
            sep_color: (THEME_COLOR_DIVIDER)
            quote_bg_color: (THEME_COLOR_BG_HIGHLIGHT)
            quote_fg_color: (THEME_COLOR_TEXT_DEFAULT)
            code_color: (THEME_COLOR_BG_HIGHLIGHT)
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
                        sdf.fill(self.quote_bg_color)
                        sdf.box(
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            self.rect_size.y - THEME_SPACE_2,
                            1.5
                        );
                        sdf.fill(self.quote_fg_color);
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
                        sdf.fill(self.sep_color);
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
                        sdf.fill(self.code_color);
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
                        sdf.fill(self.code_color);
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
                        sdf.fill(self.line_color);
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
                        sdf.fill(self.line_color);
                        return sdf.result;
                    }
                }
                return #f00
            }
        }
    }

    MarkdownLink = <MarkdownLinkBase> {
        width: Fit, height: Fit,
        align: {x: 0., y: 0.}

        label_walk: { width: Fit, height: Fit }

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
                    THEME_COLOR_TEXT_PRESSED,
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
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }
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

    Markdown = <MarkdownBase> {
        width:Fill, height:Fit,
        flow: RightWrap,
        padding: <THEME_MSPACE_1> {}
        
        font_size: (THEME_FONT_SIZE_P),
        font_color: (THEME_COLOR_TEXT_DEFAULT),

        paragraph_spacing: 16,
        pre_code_spacing: 8,
        inline_code_padding: <THEME_MSPACE_1> {},
        inline_code_margin: <THEME_MSPACE_1> {},
        
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

        quote_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        quote_walk: { width: Fill, height: Fit, }

        list_item_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_1> {}
        }
        list_item_walk: {
            height: Fit, width: Fill,
        }

        sep_walk: {
            width: Fill, height: 4.
            margin: <THEME_MSPACE_V_1> {}
        }

        draw_block: {
            line_color: (THEME_COLOR_TEXT_DEFAULT)
            sep_color: (THEME_COLOR_DIVIDER)
            quote_bg_color: (THEME_COLOR_BG_HIGHLIGHT)
            quote_fg_color: (THEME_COLOR_TEXT_DEFAULT)
            code_color: (THEME_COLOR_BG_HIGHLIGHT)

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
                        sdf.fill(self.quote_bg_color)
                        sdf.box(
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            self.rect_size.y - THEME_SPACE_2,
                            1.5
                        );
                        sdf.fill(self.quote_fg_color)
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
                        sdf.fill(self.sep_color);
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
                        sdf.fill(self.code_color);
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
                        sdf.fill(self.code_color);
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
                        sdf.fill(self.line_color);
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
                        sdf.fill(self.line_color);
                        return sdf.result;
                    }
                }
                return #f00
            }
        }

        link = <MarkdownLink> {}
    }

    ScrollBarTabs = <ScrollBarBase> {
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
                return sdf.fill(THEME_COLOR_U_HIDDEN)
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

    ScrollBarsTabs = <ScrollBarsBase> {
        show_scroll_x: true,
        show_scroll_y: true,
        scroll_bar_x: <ScrollBarTabs> {}
        scroll_bar_y: <ScrollBarTabs> {}
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
                return sdf.fill( mix(
                    THEME_COLOR_CTRL_DEFAULT,
                    mix(
                        THEME_COLOR_CTRL_SCROLLBAR_HOVER,
                        THEME_COLOR_CTRL_SCROLLBAR_HOVER * 1.2,
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
        // TODO: NEEDS FOCUS STATE

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
            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        mix(self.color, #f, 0.5),
                        self.hover
                    ),
                    self.color * 0.75,
                    self.pressed
                )
            }
        }

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_CTRL_DEFAULT)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_CTRL_PRESSED, self.pressed);

                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_SHADOW, self.pressed),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

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
                    THEME_BEVELING
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
        padding: <THEME_MSPACE_2> {}
        margin: 0.
        align: { x: 0.5, y: 0.5 }
        icon_walk: { width: 12. }
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.fill(#f00)
                return sdf.result
            }
        }

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

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_U_HIDDEN)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_CTRL_PRESSED, self.pressed);

                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_BEVEL_SHADOW, self.pressed),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_BEVEL_LIGHT, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

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
                    THEME_BEVELING
                )

                return sdf.result
            }
        }

    }

    ButtonFlatter = <ButtonIcon> {
        height: Fit, width: Fit,
        padding: <THEME_MSPACE_2> {},
        margin: <THEME_MSPACE_2> {},
        align: { x: 0.5, y: 0.5 },
        icon_walk: { width: 12. },
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.fill(#f00)
                return sdf.result
            }
        }

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

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_U_HIDDEN)
            instance bodybottom: (THEME_COLOR_U_HIDDEN)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_D_HIDDEN, self.pressed);

                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_D_HIDDEN, self.pressed),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_D_HIDDEN, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

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
                    THEME_BEVELING
                )

                return sdf.result
            }
        }

    }

    CheckBox = <CheckBoxBase> {
        width: Fit, height: Fit,
        padding: <THEME_MSPACE_2> {}
        align: { x: 0., y: 0. }

        margin: { right: 0.5}

        label_walk: {
            width: Fit, height: Fit,
            // margin: { left: 20., right: (THEME_SPACE_2) }
            margin: <THEME_MSPACE_H_1> { left: 20.}
    }

        draw_check: {
            uniform size: 7.5;
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.check_type {
                    CheckType::Check => {
                        let left = 1;
                        let sz = self.size - 1.0;
                        let offset_x = 5.0;
                        let offset_y = -1.0;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        sdf.box(left + offset_x, c.y - sz, sz * 2.0, sz * 2.0, 1.5 + offset_y);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pos.y), THEME_BEVELING)

                        let szs = sz * 0.5;
                        let dx = 1.0;
                        sdf.move_to(left + 4.0 + offset_x, c.y);
                        sdf.line_to(c.x + offset_x, c.y + szs);
                        sdf.line_to(c.x + szs + offset_x, c.y - szs);
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
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pos.y), THEME_BEVELING)
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
                        let left = sz + 5.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);

                        sdf.stroke_keep(
                            mix(
                                THEME_COLOR_BEVEL_SHADOW,
                                THEME_COLOR_BEVEL_LIGHT,
                                clamp(self.pos.y - 0.2, 0, 1)),
                            THEME_BEVELING
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
                        sdf.fill(THEME_COLOR_D_HIDDEN);
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
                    THEME_COLOR_TEXT_DEFAULT,
                    self.selected
                )
            }
        }

        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance selected: 0.0
            uniform color: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_active: (THEME_COLOR_TEXT_ACTIVE)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color * 1.4,
                        self.hover
                    ),
                    mix(
                        self.color_active,
                        self.color_active * 1.4,
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
        align: { x: 0., y: 0. }
        draw_check: { check_type: Toggle }
        margin: { right: -17.5}
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
                        sdf.clear(mix(THEME_COLOR_APP_CAPTION_BAR, mix(#6, #9, self.pressed), self.hover));
                        sdf.move_to(c.x - sz, c.y);
                        sdf.line_to(c.x + sz, c.y);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsMax => {
                        sdf.clear(mix(THEME_COLOR_APP_CAPTION_BAR, mix(#6, #9, self.pressed), self.hover));
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsMaxToggled => {
                        let clear = mix(THEME_COLOR_APP_CAPTION_BAR, mix(#6, #9, self.pressed), self.hover);
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
                        sdf.clear(mix(THEME_COLOR_APP_CAPTION_BAR, mix(#e00, #c00, self.pressed), self.hover));
                        sdf.move_to(c.x - sz, c.y - sz);
                        sdf.line_to(c.x + sz, c.y + sz);
                        sdf.move_to(c.x - sz, c.y + sz);
                        sdf.line_to(c.x + sz, c.y - sz);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        return sdf.result;
                    }
                    DesktopButtonType::XRMode => {
                        sdf.clear(mix(THEME_COLOR_APP_CAPTION_BAR, mix(#0aa, #077, self.pressed), self.hover));
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
        pass: { clear_color: (THEME_COLOR_BG_APP) }
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
                sdf.clear(THEME_COLOR_BG_APP); // TODO: This should be a transparent color instead.

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
                        THEME_COLOR_CTRL_SCROLLBAR_HOVER,
                        THEME_COLOR_CTRL_SCROLLBAR_HOVER * 1.2,
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
        // TODO: NEEDS FOCUS STATE
        height: 10.0, width: 10.0,
        margin: { right: (THEME_SPACE_2), left: -3.5 },
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
                    THEME_COLOR_TEXT_INACTIVE,
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                ), 1.0);
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
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
        padding: <THEME_MSPACE_3> { }

        close_button: <TabCloseButton> {}
        draw_name: {
            text_style: <THEME_FONT_REGULAR> {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_INACTIVE,
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
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    -1.,
                    -1.,
                    self.rect_size.x + 2,
                    self.rect_size.y + 2,
                    1.
                )
                sdf.fill_keep(
                    mix(
                        THEME_COLOR_D_2 * 0.75,
                        THEME_COLOR_DOCK_TAB_SELECTED,
                        self.selected
                    )
                )
                return sdf.result
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
        CloseableTab = <Tab> {closeable:true}
        PermanentTab = <Tab> {closeable:false}

        draw_drag: {
            draw_depth: 10
            color: (THEME_COLOR_BG_CONTAINER)
        }
        draw_fill: {
            color: (THEME_COLOR_D_1)
        }

        width: Fill, height: (THEME_TAB_HEIGHT)

        scroll_bars: <ScrollBarsTabs> {
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
        flow: Down,

        round_corner: {
            draw_depth: 20.0
            border_radius: 20.
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
                sdf.fill(THEME_COLOR_BG_APP)
                return sdf.result
            }
        }
        border_size: (THEME_DOCK_BORDER_SIZE)

        padding: {left: (THEME_DOCK_BORDER_SIZE), top: 0, right: (THEME_DOCK_BORDER_SIZE), bottom: (THEME_DOCK_BORDER_SIZE)}
        padding_fill: {color: (THEME_COLOR_BG_APP)} // TODO: unclear what this does
        drag_quad: {
            draw_depth: 10.0
            color: (THEME_COLOR_DRAG_QUAD)
        }
        tab_bar: <TabBar> {}
        splitter: <Splitter> {}
    }

    TabMinimal = <TabBase> {
        width: Fit, height: Fill, //Fixed((THEME_TAB_HEIGHT)),
        align: {x: 0.0, y: 0.5}
        padding: <THEME_MSPACE_3> { }

        close_button: <TabCloseButton> {}
        draw_name: {
            text_style: <THEME_FONT_REGULAR> {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_INACTIVE,
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
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let marker_height = 2.5

                sdf.rect(0, self.rect_size.y - marker_height, self.rect_size.x, marker_height)
                sdf.fill(mix((THEME_COLOR_U_HIDDEN), (THEME_COLOR_DOCK_TAB_SELECTED_MINIMAL), self.selected));
                return sdf.result
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

    TabBarMinimal = <TabBarBase> {
        tab: <TabMinimal> {}
        draw_drag: {
            draw_depth: 10
            color: (THEME_COLOR_BG_CONTAINER)
        }
        draw_fill: {
            color: (THEME_COLOR_U_HIDDEN)
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

    DockMinimal = <DockBase> {
        flow: Down,

        round_corner: {
            draw_depth: 20.0
            border_radius: 20.
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
                return sdf.fill(THEME_COLOR_BG_APP)
            }
        }
        border_size: (THEME_DOCK_BORDER_SIZE)

        padding: {left: (THEME_DOCK_BORDER_SIZE), top: 0, right: (THEME_DOCK_BORDER_SIZE), bottom: (THEME_DOCK_BORDER_SIZE)}
        padding_fill: {color: (THEME_COLOR_BG_APP)} // TODO: unclear what this does
        drag_quad: {
            draw_depth: 10.0
            color: (THEME_COLOR_DRAG_QUAD)
        }
        tab_bar: <TabBarMinimal> {}
        splitter: <Splitter> {}
    }

    // TODO: remove?
    RectView = <View> {
        show_bg: true,
        draw_bg: { color: (THEME_COLOR_DOCK_CONTAINER) }
    }

    DockToolbar = <RectShadowView> {
        width: Fill, height: 38.,
        flow: Down,
        align: { x: 0., y: 0. }
        margin: { top: -1. }
        padding: <THEME_MSPACE_2> {}
        spacing: 0.,

        draw_bg: {
            border_width: 0.0
            border_color: (THEME_COLOR_BEVEL_LIGHT)
            shadow_color: (THEME_COLOR_D_4)
            shadow_radius: 7.5
            shadow_offset: vec2(0.0, 0.0)
            color: (THEME_COLOR_FG_APP),
        }

        content = <View> {
            flow: Right,
            width: Fill, height: Fill,
            margin: 0.
            padding: 0.
            align: { x: 0., y: 0. }
            spacing: (THEME_SPACE_3)
        }
    }
                

    PopupMenuItem = <PopupMenuItemBase> {
        width: Fill, height: Fit,
        align: { y: 0.5 }
        padding: <THEME_MSPACE_1> { left: 15. }

        draw_name: {
            instance selected: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P),
            }
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
                return mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_SHADOW, pow(self.pos.y, 0.35))
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
                    sdf.stroke(self.get_border_color(), THEME_BEVELING)
                }
                return sdf.result;
            }
        }
    }

    DropDown = <DropDownBase> {
        // TODO: utilize the existing focus state
        width: Fit, height: Fit,
        margin: 0.,
        padding: <THEME_MSPACE_2> { right: 22.5 }
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
            instance open: 0.0
            
            uniform border_radius: (THEME_CORNER_RADIUS)
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
                            THEME_COLOR_BEVEL_LIGHT,
                            self.hover
                        ),
                        THEME_COLOR_BEVEL_LIGHT,
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
                    ), THEME_BEVELING
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
                    THEME_BEVELING * 1.5
                )

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                let offset = 1.;
                let offset_x = 2.;

                sdf.move_to(c.x - sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x + sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x - offset_x, c.y + sz * 0.25 + offset);
                sdf.close_path();

                sdf.fill(mix(THEME_COLOR_TEXT_DEFAULT, THEME_COLOR_TEXT_HOVER, self.hover));

                return sdf.result
            }
        }

        popup_menu: <PopupMenu> {}

        selected_item: 0,

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

    DropDownFlat = <DropDown> {
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0
            instance pressed: 0.0
            instance open: 0.0
            
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_U_HIDDEN)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), self.bodybottom, self.focus);

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)

                sdf.stroke(
                    THEME_COLOR_U_HIDDEN,
                    THEME_BEVELING * 1.5
                )

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                let offset = 1.;
                let offset_x = 2.;

                sdf.move_to(c.x - sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x + sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x - offset_x, c.y + sz * 0.25 + offset);
                sdf.close_path();

                sdf.fill(mix(THEME_COLOR_TEXT_DEFAULT, THEME_COLOR_TEXT_HOVER, self.hover));

                return sdf.result
            }
        }
    }

    FileTreeNode = <FileTreeNodeBase> {
        align: { y: 0.5 }
        padding: { left: (THEME_SPACE_1) },
        is_folder: false,
        indent_width: 10.0
        min_drag_distance: 10.0

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    -2.,
                    self.rect_size.x,
                    self.rect_size.y + 3.0,
                    1.
                )
                sdf.fill_keep(
                    mix(
                        mix(
                            THEME_COLOR_BG_EVEN,
                            THEME_COLOR_BG_ODD,
                            self.is_even
                        ),
                        THEME_COLOR_CTRL_SELECTED,
                        self.selected
                    )
                )
                return sdf.result
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
                    THEME_COLOR_TEXT_SELECTED,
                    self.selected
                ));
            }
        }

        draw_name: {
            fn get_color(self) -> vec4 {
                return mix(
                    THEME_COLOR_TEXT_DEFAULT * self.scale,
                    THEME_COLOR_TEXT_SELECTED,
                    self.selected
                )
            }

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
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
        // TODO: adda  focus states
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
                default: on
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        open: 0.0
                        draw_bg: {open: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        open: 1.0
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

    RadioButton = <RadioButtonBase> {
        // TODO: adda  focus states
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
            // uniform color_active: (THEME_COLOR_U_2)
            // uniform color_inactive: (THEME_COLOR_D_4)

            // instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
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
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pos.y), (THEME_BEVELING))
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
                        let top_gradient = mix(body_transp, mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_SHADOW, self.selected), max(0.0, grad_top - sdf.pos.y) / grad_top);
                        let bot_gradient = mix(
                            mix(body_transp, THEME_COLOR_BEVEL_LIGHT, self.selected),
                            top_gradient,
                            clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                        );

                        // the little drop shadow at the bottom
                        let shift_inward = 0. * 1.75;
                        sdf.move_to(shift_inward, self.rect_size.y);
                        sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
                        sdf.stroke(
                            mix(
                                THEME_COLOR_BEVEL_SHADOW,
                                THEME_COLOR_U_HIDDEN,
                                self.selected
                            ), THEME_BEVELING * 2.)

                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x - 2.0,
                            self.rect_size.y - 2.0,
                            1.
                        )
                        sdf.fill_keep(body)

                        sdf.stroke(bot_gradient, THEME_BEVELING * 1.5)
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
            uniform color: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_active: (THEME_COLOR_TEXT_ACTIVE)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        mix(self.color, #f, 0.4),
                        self.hover
                    ),
                    mix(
                        self.color_active,
                        mix(self.color_active, #f, 0.75),
                        self.hover
                    ),
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

            uniform color_unselected: (THEME_COLOR_U_3)
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

    RadioButtonImage = <RadioButton> { }

    RadioButtonTab = <RadioButton> {
        height: Fit,
        draw_radio: { radio_type: Tab }
        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25)}

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
        clip_x: false,
        clip_y: false,

        cursor_width: 2.0,

        is_read_only: false,
        is_numeric_only: false,
        empty_message: "0",

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_text: {hover: 0.0},
                        draw_selection: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_text: {hover: 1.0},
                        draw_selection: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: .25}}
                    apply: {
                        draw_bg: {focus: 0.0},
                        draw_text: {focus: 0.0},
                        draw_cursor: {focus: 0.0},
                        draw_selection: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0},
                        draw_text: {focus: 1.0}
                        draw_cursor: {focus: 1.0},
                        draw_selection: {focus: 1.0}
                    }
                }
            }
        }

        draw_bg: {
            instance radius: (THEME_CORNER_RADIUS)
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
                    (THEME_COLOR_BEVEL_LIGHT),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );

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
                    THEME_BEVELING * 0.9
                )

                return sdf.result
            }
        }

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

        draw_selection: {
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
    }

    Slider = <SliderBase> {
        min: 0.0, max: 1.0,
        step: 0.0,
        label_align: { y: 0.0 }
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
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

                let slider_bg_color = mix(mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_HOVER, self.hover), THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.focus);
                let slider_color = mix(
                    mix(THEME_COLOR_AMOUNT_DEFAULT, THEME_COLOR_AMOUNT_HOVER, self.hover),
                    THEME_COLOR_AMOUNT_ACTIVE,
                    self.focus);

                let nub_color = (THEME_COLOR_SLIDER_NUB_DEFAULT);
                let nubbg_color = mix(THEME_COLOR_SLIDER_NUB_HOVER, THEME_COLOR_SLIDER_NUB_ACTIVE, self.drag);

                match self.slider_type {
                    SliderType::Horizontal => {
                        sdf.rect(0, self.rect_size.y - slider_height * 1.25, self.rect_size.x, slider_height)
                        sdf.fill(slider_bg_color);

                        sdf.rect(0, self.rect_size.y - slider_height * 0.5, self.rect_size.x, slider_height)
                        sdf.fill(THEME_COLOR_BEVEL_LIGHT);

                        sdf.rect(
                            0,
                            self.rect_size.y - slider_height * 1.25,
                            self.slide_pos * (self.rect_size.x - nub_size) + nub_size,
                            slider_height
                        )
                        sdf.fill(slider_color);

                        let nubbg_x = self.slide_pos * (self.rect_size.x - nub_size) - nubbg_size * 0.5 + 0.5 * nub_size;
                        sdf.rect(
                            nubbg_x,
                            self.rect_size.y - slider_height * 1.25,
                            nubbg_size,
                            slider_height
                        )
                        sdf.fill(nubbg_color);

                        // the nub
                        let nub_x = self.slide_pos * (self.rect_size.x - nub_size);
                        sdf.rect(
                            nub_x,
                            self.rect_size.y - slider_height * 1.25,
                            nub_size,
                            slider_height
                        )
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

        label_walk: { width: Fill, height: Fit }

        text_input: <TextInput> {
            width: Fit, padding: 0.,
            // cursor_margin_bottom: (THEME_SPACE_1),
            // cursor_margin_top: (THEME_SPACE_1),
            // select_pad_edges: 3.0
            // cursor_size: 2.0,
            empty_message: "0",
            is_numeric_only: true,

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

    SliderBig = <Slider> {
        height: 36
        text: "CutOff1"
        // draw_text: {text_style: <H2_TEXT_BOLD> {}, color: (COLOR_UP_5)}
        text_input: {
            // cursor_margin_bottom: (THEME_SPACE_1),
            // cursor_margin_top: (THEME_SPACE_1),
            // select_pad_edges: (THEME_SPACE_1),
            // cursor_size: (THEME_SPACE_1),
            empty_message: "0",
            is_numeric_only: true,
            draw_bg: {
                color: (THEME_COLOR_D_HIDDEN)
            },
        }
        draw_slider: {
            instance line_color: (THEME_COLOR_AMOUNT_DEFAULT_BIG)
            instance bipolar: 0.0
            fn pixel(self) -> vec4 {
                let nub_size = 3

                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 20.0;

                sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, 1);
                sdf.fill_keep(
                    mix(
                        mix((THEME_COLOR_INSET_PIT_TOP), (THEME_COLOR_INSET_PIT_BOTTOM) * 0.1, pow(self.pos.y, 1.0)),
                        mix((THEME_COLOR_INSET_PIT_TOP_HOVER) * 1.75, (THEME_COLOR_BEVEL_LIGHT) * 0.1, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                ) // Control backdrop gradient

                sdf.stroke(mix(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_SHADOW * 1.25, self.drag), THEME_COLOR_BEVEL_LIGHT, pow(self.pos.y, 10.0)), 1.0) // Control outline
                let in_side = 5.0;
                let in_top = 5.0; // Ridge: vertical position
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.drag)); // Ridge color
                let in_top = 7.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 1.5);
                sdf.fill(THEME_COLOR_BEVEL_LIGHT); // Ridge: Rim light catcher

                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);

                sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
                sdf.stroke_keep(mix((THEME_COLOR_U_HIDDEN), self.line_color, self.drag), 1.5)
                sdf.stroke(
                    mix(mix(self.line_color * 0.85, self.line_color, self.hover), THEME_COLOR_AMOUNT_ACTIVE, self.drag),
                    1.5
                )

                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 3) - 3;
                sdf.box(nub_x + in_side, top + 1.0, 11, 11, 1.)

                sdf.fill_keep(mix(
                    mix(
                        mix(THEME_COLOR_SLIDER_BIG_NUB_TOP, THEME_COLOR_SLIDER_BIG_NUB_TOP_HOVER, self.hover),
                        mix(THEME_COLOR_SLIDER_BIG_NUB_BOTTOM, THEME_COLOR_SLIDER_BIG_NUB_BOTTOM_HOVER, self.hover),
                        self.pos.y
                    ),
                    mix(THEME_COLOR_SLIDER_BIG_NUB_BOTTOM, THEME_COLOR_SLIDER_BIG_NUB_TOP, pow(self.pos.y, 1.5)),
                    self.drag
                    ) // Nub background gradient
                )
                sdf.stroke(
                    mix(
                        mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_LIGHT * 1.2, self.hover),
                        THEME_COLOR_BLACK,
                        pow(self.pos.y, 1.)
                    ),
                    1.
                ); // Nub outline gradient


                return sdf.result
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

                title = <H4> {
                    width: Fit, height: Fit,
                    margin: 0,
                    text: "Stack View Title"
                }
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

    TOGGLE_PANEL_CLOSE_ICON = dep("crate://self/resources/icons/close_left_panel.svg")
    TOGGLE_PANEL_OPEN_ICON = dep("crate://self/resources/icons/open_left_panel.svg")
    TogglePanel = <TogglePanelBase> {
        flow: Overlay,
        width: 300,
        height: Fill,

        open_content = <CachedView> {
            width: Fill
            height: Fill

            draw_bg: {
                instance opacity: 1.0
    
                fn pixel(self) -> vec4 {
                    let color = sample2d_rt(self.image, self.pos * self.scale + self.shift) + vec4(self.marked, 0.0, 0.0, 0.0);
                    return Pal::premul(vec4(color.xyz, color.w * self.opacity))
                }
            }
        }

        persistent_content = <View> {
            height: Fit
            width: Fill
            default = <View> {
                height: Fit,
                width: Fill,
                padding: {top: 58, left: 15, right: 15}
                spacing: 10,

                before = <View> {
                    height: Fit,
                    width: Fit,
                    spacing: 10,
                }

                close = <Button> {
                    draw_icon: {
                        svg_file: (TOGGLE_PANEL_CLOSE_ICON),
                    }
                }

                open = <Button> {
                    visible: false,
                    draw_icon: {
                        svg_file: (TOGGLE_PANEL_OPEN_ICON),
                    }
                }

                after = <View> {
                    height: Fit,
                    width: Fit,
                    spacing: 10,
                }
            }
        }

        animator: {
            panel = {
                default: open,
                open = {
                    redraw: true,
                    from: {all: Forward {duration: 0.3}}
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    apply: {animator_panel_progress: 1.0, open_content = { draw_bg: {opacity: 1.0} }}
                }
                close = {
                    redraw: true,
                    from: {all: Forward {duration: 0.3}}
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    apply: {animator_panel_progress: 0.0, open_content = { draw_bg: {opacity: 0.0} }}
                }
            }
        }
    }

    DesignerOutlineTreeNode = <DesignerOutlineTreeNodeBase> {
        align: { y: 0.5 }
        padding: { left: (THEME_SPACE_1) },

        indent_width: 10.0
        min_drag_distance: 10.0
        button_open_width: 24.0,
        draw_eye: false,

        draw_bg: {
            instance selected: 0.0
            instance hover: 0.0
            instance focussed: 0.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    -2.,
                    self.rect_size.x,
                    self.rect_size.y + 3.0,
                    1.
                )
                sdf.fill_keep(
                    mix(
                        mix(
                            THEME_COLOR_BG_EVEN,
                            THEME_COLOR_BG_ODD,
                            self.is_even
                        ),
                        THEME_COLOR_CTRL_SELECTED,
                        self.selected
                    )
                )
                return sdf.result
            }
        }
        icon_walk:{
            margin:{top:3,left:3,right:5}
            width:12,
            height:12,
        }
        draw_icon: {
            instance selected: 0.0
            instance hover: 0.0
            instance focussed: 0.0
            fn get_color(self) -> vec4 {
                return self.color * self.scale;
            }
        }

        draw_name: {
            instance selected: 0.0
            instance hover: 0.0
            instance focussed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    THEME_COLOR_TEXT_DEFAULT * self.scale,
                    THEME_COLOR_TEXT_SELECTED,
                    self.selected
                )
            }

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
                //top_drop: 1.2,
            }
        }

        button_open: <FoldButton> {
            height: 25, width: 15,
            margin: { left: (THEME_SPACE_2) }
            animator: { open = { default: off } },
            draw_bg: {
                uniform size: 3.75;
                instance open: 0.0

                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let left = 2;
                    let sz = self.size;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);

                    // PLUS
                    sdf.box(0.5, sz * 3.0, sz * 2.5, sz * 0.7, 1.0); // rounding = 3rd value
                    // vertical
                    sdf.fill_keep(mix(#8F, #FF, self.hover));
                    sdf.box(sz * 1.0, sz * 2.125, sz * 0.7, sz * 2.5, 1.0); // rounding = 3rd value

                    sdf.fill_keep(mix(mix(#8F, #FF, self.hover), #FFF0, self.open))

                    return sdf.result
                }
            }
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
        }
    }

    STUDIO_PALETTE_1 = #B2FF64
    STUDIO_PALETTE_2 = #80FFBF
    STUDIO_PALETTE_3 = #80BFFF
    STUDIO_PALETTE_4 = #BF80FF
    STUDIO_PALETTE_5 = #FF80BF
    STUDIO_PALETTE_6 = #FFB368
    STUDIO_PALETTE_7 = #FFD864

    STUDIO_COLOR_FILE = (THEME_COLOR_TEXT_DEFAULT)
    STUDIO_COLOR_FOLDER = (THEME_COLOR_TEXT_DEFAULT)
    STUDIO_COLOR_LAYOUT = (STUDIO_PALETTE_6)
    STUDIO_COLOR_WIDGET = (STUDIO_PALETTE_2)
    STUDIO_COLOR_ASSET = (STUDIO_PALETTE_5)
    STUDIO_COLOR_TEXT = (STUDIO_PALETTE_1)

    DesignerOutlineTree = <DesignerOutlineTreeBase> {
        flow: Down,

        scroll_bars: <ScrollBars> {}
        scroll_bars: {}
        node_height: (THEME_DATA_ITEM_HEIGHT),
        clip_x: true,
        clip_y: true

        File = <DesignerOutlineTreeNode> {
            draw_eye: true,
            draw_icon: {
                color: (STUDIO_COLOR_FILE)
                svg_file: dep("crate://self/resources/icons/icon_file.svg"),
            }
        }

        Folder = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_FOLDER)
                svg_file: dep("crate://self/resources/icons/icon_folder.svg"),
            }
        }

        Layout = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_LAYOUT)
                svg_file: dep("crate://self/resources/icons/icon_layout.svg"),
            }
        }

        Widget = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_WIDGET)
                svg_file: dep("crate://self/resources/icons/icon_widget.svg"),
            }
        }

        Asset = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_ASSET)
                svg_file: dep("crate://self/resources/icons/icon_image.svg"),
            }
        }

        Text = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_TEXT)
                svg_file: dep("crate://self/resources/icons/icon_text.svg"),
            }
        }

        filler: {
            fn pixel(self) -> vec4 {
                return mix(
                    THEME_COLOR_BG_EVEN,
                    THEME_COLOR_BG_ODD,
                    self.is_even
                );
            }
        }
    }

    DesignerOutline = <DesignerOutlineBase>{ }

    Vr = <View> {
        width: Fit, height: 27.,
        flow: Right,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}
        <View> {
            width: (THEME_BEVELING * 2.0), height: Fill
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_SHADOW) }
        }
        <View> {
            width: (THEME_BEVELING), height: Fill,
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_LIGHT) }
        }
    }

    DesignerToolbox = <DesignerToolboxBase>{
        width: Fill,
        height: Fill
        show_bg: false

        <DockToolbar> {
            content = {
                align: { x: 0., y: 0.5 }
                spacing: (THEME_SPACE_3 * 1.5)
                <ButtonFlat> {
                    width: 32.
                    text: ""
                    margin: { right: -10. }
                    icon_walk: { width: 11. }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_typography.svg"),
                    }
                }
                <Vr> {}
                <View> {
                    width: Fit,
                    flow: Right,
                    spacing: (THEME_SPACE_1)
                    <Pbold> {
                        width: Fit,
                        text: "Font",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "Noto Sans",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                }
                <View> {
                    width: Fit,
                    spacing: (THEME_SPACE_1)
                    flow: Right,
                    <Pbold> {
                        width: Fit,
                        text: "Weight",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "bold",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                }
                <View> {
                    width: Fit,
                    spacing: (THEME_SPACE_1)
                    flow: Right,
                    <Pbold> {
                        width: Fit,
                        text: "Size",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "11 pt",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                } 
                <View> {
                    width: Fit,
                    spacing: (THEME_SPACE_1)
                    flow: Right,
                    <Pbold> {
                        width: Fit,
                        text: "Line height",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "1.2",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                } 
                <Vr> {}
                <View> {
                    width: Fit,
                    flow: Right,
                    spacing: 0,
                    <ButtonFlat> {
                        width: 25.
                        text: ""
                        icon_walk: { width: 11. }
                        draw_icon: {
                            svg_file: dep("crate://self/resources/icons/icon_text_align_left.svg"),
                        }
                    }
                    <ButtonFlat> {
                        width: 25.
                        text: ""
                        icon_walk: { width: 11. }
                        draw_icon: {
                            color: (THEME_COLOR_D_3),
                            svg_file: dep("crate://self/resources/icons/icon_text_align_justify.svg"),
                        }
                    }
                    <ButtonFlat> {
                        width: 25.
                        text: ""
                        icon_walk: { width: 11. }
                        draw_icon: {
                            color: (THEME_COLOR_D_3),
                            svg_file: dep("crate://self/resources/icons/icon_text_align_right.svg"),
                        }
                    }
                }
                <Vr> {}
                <P> { width: Fit, text: "Stroke" }
                <RoundedView> {
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_5),
                        radius: 5.0
                    }
                }
                <P> { width: Fit, text: "Fill" }
                <RoundedView> {
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_2),
                        radius: 5.0
                    }
                }
                <Filler> {}
                <Vr> {}
                <P> { width: Fit, text: "Canvas" }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (THEME_COLOR_D_3),
                        radius: 5.0
                    }
                }
            }
        }

        <RoundedShadowView>{
            abs_pos: vec2(25., 65.)
            padding: 0.
            width: 36., height: Fit,
            spacing: 0.,
            align: { x: 0.5, y: 0.0 }
            flow: Down,
            clip_x: false, clip_y: false,

            draw_bg: {
                border_width: 1.0
                border_color: (THEME_COLOR_BEVEL_LIGHT)
                shadow_color: (THEME_COLOR_D_4)
                shadow_radius: 10.0,
                shadow_offset: vec2(0.0, 5.0)
                radius: 2.5
                color: (THEME_COLOR_FG_APP),
            }

            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 9. }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_select.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 14.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_draw.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,

                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 12. }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_text.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 13.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_layout.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    flow: Down,
                    icon_walk: { width: 15.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_widget.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 15.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_image.svg"),
                    }
                    text: ""
                }
            }
        }
        /*
        <RoundedShadowView>{
            width: 250., height: 350.,
            abs_pos: vec2(25., 325.)
            padding: <THEME_MSPACE_2> {}
            spacing: (THEME_SPACE_1)
            align: { x: 0.5, y: 0.0 }
            flow: Down,
            clip_x: false, clip_y: false,

            draw_bg: {
                border_width: 1.0
                border_color: (THEME_COLOR_BEVEL_LIGHT)
                shadow_color: (THEME_COLOR_D_4)
                shadow_radius: 10.0,
                shadow_offset: vec2(0.0, 5.0)
                radius: 2.5
                color: (THEME_COLOR_FG_APP),
            }
            
            <View> {
                flow: Right,
                width: Fill, height: Fit, 
                align: { x: 0.0, y: 0.5 }
                <RoundedView> {
                    margin: { left: (THEME_SPACE_2), right: (THEME_SPACE_1), top: 5. }
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (#f00),
                        radius: 5.0
                    }
                }
                <Pbold> { width: Fit, margin: {left: 3.}, text: "Canvas" }
            }
            <Hr> { margin: <THEME_MSPACE_1> {} }
            <ColorPicker>{}
            <View> {
                width: Fill, height: Fit, 
                spacing: (THEME_SPACE_2)
                align: { x: 0.5, y: 0.5 }
                flow: Right,
                <Pbold> { width: Fit, text: "RGBA" }
                <P> { width: Fit, text: "0 / 255 / 0 / 255" }
                <P> { width: Fit, text: "#83741AFF" }
            }
            <View> {
                align: { x: 0.5, y: 0.5 }
                width: Fill, height: Fit, 
                flow: Right,
                spacing: (THEME_SPACE_1),
                margin: { bottom: 10. }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_1),
                        radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_2),
                        radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_3),
                        radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_4),
                        radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_5),
                        radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_6),
                        radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_7),
                        radius: 5.0
                    }
                }
            }
        }*/
    }

    DesignerContainer = <DesignerContainerBase>{
        width: 1200,
        height: 1200,
        flow: Overlay,
        clip_x:false,
        clip_y:false,
        align:{x:1.0},
        animator: {
            select = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        view = {draw_bg:{border_color:#5}}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        view = {draw_bg:{border_color:#c}}
                    }
                }

            }
        }
        view = <RoundedView>{
            draw_bg:{
                color:#3,
                border_width:2
                border_color:#5
            }
            padding: 10
            inner = <BareStep>{}
        }

        widget_label = <RoundedShadowView>{
            margin: { top: -35., right: 0. }
            padding: 0.
            width: Fit, height: Fit,
            spacing: 0.,
            align: { x: 1.0, y: 0.0 }
            flow: Down,
            clip_x: false, clip_y: false,

            draw_bg: {
                border_width: 1.0
                border_color: (THEME_COLOR_BEVEL_LIGHT)
                shadow_color: (THEME_COLOR_D_3)
                shadow_radius: 5.0,
                shadow_offset: vec2(0.0, 0.0)
                radius: 2.5
                color: (THEME_COLOR_FG_APP),
            }

            label = <Button> {
                padding: <THEME_MSPACE_2> {}
                text:"Hello world"

                draw_bg: {
                    instance hover: 0.0
                    instance pressed: 0.0
                    uniform border_radius: (THEME_CORNER_RADIUS)
                    instance bodytop: (THEME_COLOR_FG_APP)
                    instance bodybottom: #f00
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        let grad_top = 5.0;
                        let grad_bot = 2.0;
                        let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_CTRL_PRESSED, self.pressed);

                        let body_transp = vec4(body.xyz, 0.0);
                        let top_gradient = mix(
                            body_transp,
                            mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_SHADOW, self.pressed),
                            max(0.0, grad_top - sdf.pos.y) / grad_top
                        );
                        let bot_gradient = mix(
                            mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pressed),
                            top_gradient,
                            clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                        );

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
                            THEME_BEVELING
                        )

                        return sdf.result
                    }
                }
            }
        }
    }

    DesignerView = <DesignerViewBase>{
        clear_color: #333333
        draw_outline:{
            fn pixel(self) -> vec4 {
                let p = self.pos * self.rect_size;
                let sdf = Sdf2d::viewport(p)
                sdf.rect(0., 0., self.rect_size.x, self.rect_size.y);
                
                let line_width = 0.58;
                let dash_length = 10;
                let pos = p.x + p.y;//+self.time*10.0 ;
                let dash_pattern = fract(pos / dash_length);
                let alpha = step(dash_pattern, line_width);
                
                let c = vec4(mix(#c, #0000, alpha))
                
                sdf.stroke(c, 1.5);
                return sdf.result;
                //return vec4(self.color.xyz * self.color.w, self.color.w)
            }
        }
        
        draw_bg: {
            texture image: texture2d
            varying scale: vec2
            varying shift: vec2
            fn vertex(self) -> vec4 {

                let dpi = self.dpi_factor;
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size;
                self.shift = (self.rect_pos - floor_pos) / ceil_size;
                return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            }
            fn pixel(self) -> vec4 {
                return sample2d_rt(self.image, self.pos * self.scale + self.shift);
            }
        }
        container: <DesignerContainer>{
        }
    }

    Designer = <DesignerBase>{
        <Window> {
            window: { kind_id: 2 }
            body = <View> {
                designer_outline = <DesignerOutline> {
                    flow: Down,
                    <DockToolbar> {
                        content = {
                            margin: {left: (THEME_SPACE_1), right: (THEME_SPACE_1) },
                            align: { x: 0., y: 0.0 }
                            spacing: (THEME_SPACE_3)
                            <Pbold> {
                                width: Fit,
                                text: "Filter",
                                margin: 0.,
                                padding: <THEME_MSPACE_V_1> {}
                            }

                            <View> {
                                width: Fit
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                <CheckBoxCustom> {
                                    margin: {left: (THEME_SPACE_1)}
                                    text: ""
                                    draw_check: { check_type: None }
                                    icon_walk: {width: 13.5 }
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_2),
                                        svg_file: dep("crate://self/resources/icons/icon_widget.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text: ""
                                    draw_check: { check_type: None }
                                    icon_walk: {width: 12.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_6),
                                        svg_file: dep("crate://self/resources/icons/icon_layout.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text: ""
                                    draw_check: { check_type: None }
                                    icon_walk: {width: 10.5}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_1),
                                        svg_file: dep("crate://self/resources/icons/icon_text.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text:""
                                    draw_check: { check_type: None }
                                    icon_walk: {width: 13.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_5),
                                        svg_file: dep("crate://self/resources/icons/icon_image.svg"),
                                    }
                                }
                            }
                            <TextInput> {
                                width: Fill,
                                empty_message: "Filter",
                            }
                        }
                    }
                    outline_tree = <DesignerOutlineTree>{

                    }
                }
            }
        }
        <Window>{
            window:{ kind_id: 1 }
            body = <View>{
                flow: Overlay
                designer_view = <DesignerView> {
                    width: Fill, height: Fill
                }
                toolbox = <DesignerToolbox>{
                }
            }
        }
    }

    Modal = <ModalBase> {
        width: Fill
        height: Fill
        flow: Overlay
        align: {x: 0.5, y: 0.5}

        draw_bg: {
            fn pixel(self) -> vec4 {
                return vec4(0., 0., 0., 0.0)
            }
        }

        bg_view: <View> {
            width: Fill
            height: Fill
            show_bg: true
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return vec4(0., 0., 0., 0.7)
                }
            }
        }

        content: <View> {
            flow: Overlay
            width: Fit
            height: Fit
        }
    }

    Tooltip = <TooltipBase> {
        width: Fill,
        height: Fill,

        flow: Overlay
        align: {x: 0.0, y: 0.0}

        draw_bg: {
            fn pixel(self) -> vec4 {
                return vec4(0., 0., 0., 0.0)
            }
        }

        content: <View> {
            flow: Overlay
            width: Fit
            height: Fit

            <RoundedView> {
                width: Fit,
                height: Fit,

                padding: 16,

                draw_bg: {
                    color: #fff,
                    border_width: 1.0,
                    border_color: #D0D5DD,
                    radius: 2.
                }

                tooltip_label = <Label> {
                    width: 270,
                    draw_text: {
                        text_style: <THEME_FONT_REGULAR>{font_size: 9},
                        text_wrap: Word,
                        color: #000
                    }
                }
            }
        }
    }

    PopupNotification = <PopupNotificationBase> {
        width: Fill
        height: Fill
        flow: Overlay
        align: {x: 1.0, y: 0.0}

        draw_bg: {
            fn pixel(self) -> vec4 {
                return vec4(0., 0., 0., 0.0)
            }
        }

        content: <View> {
            flow: Overlay
            width: Fit
            height: Fit

            cursor: Default
            capture_overload: true
        }
    }

    Root = <RootBase> { design_window = <Designer> {} }
}
