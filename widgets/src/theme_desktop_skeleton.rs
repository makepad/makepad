use crate::makepad_platform::*;

live_design! {
    link theme_desktop_skeleton;
    use link::shaders::*;
    
    // GLOBAL PARAMETERS

    pub THEME_SPACE_FACTOR = 10. // Increase for a less dense layout
    pub THEME_CORNER_RADIUS = 2.5
    pub THEME_BEVELING = 0.75
    pub THEME_FONT_SIZE_BASE = 10. // TODO: can this be removed? this is used somewhere

    // DIMENSIONS
    pub THEME_SPACE_1 = 3.
    pub THEME_SPACE_2 = 6.
    pub THEME_SPACE_3 = 9.

    pub THEME_MSPACE_1 = {top: 3., right: 3., bottom: 3., left: 3.} 
    pub THEME_MSPACE_H_1 = {top: 0., right: 3., bottom: 0., left: 3.}
    pub THEME_MSPACE_V_1 = {top: 3., right: 0., bottom: 3., left: 0.}
    pub THEME_MSPACE_2 = {top: 6., right: 6., bottom: 6., left: 6.}
    pub THEME_MSPACE_H_2 = {top: 0., right: 6., bottom: 0., left: 6.}
    pub THEME_MSPACE_V_2 = {top: 6., right: 0., bottom: 6., left: 0.}
    pub THEME_MSPACE_3 = {top: 9., right: 9., bottom: 9., left: 9.}
    pub THEME_MSPACE_H_3 = {top: 0., right: 9., bottom: 0., left: 9.}
    pub THEME_MSPACE_V_3 = {top: 9., right: 0., bottom: 9., left: 0.}

    pub THEME_DATA_ITEM_HEIGHT = 23.25;
    pub THEME_DATA_ICON_WIDTH = 15.6;
    pub THEME_DATA_ICON_HEIGHT = 21.6;

    pub THEME_CONTAINER_CORNER_RADIUS = 5. 
    pub THEME_TEXTSELECTION_CORNER_RADIUS = 12.5
    pub THEME_TAB_HEIGHT = 36.
    pub THEME_TAB_FLAT_HEIGHT = 33.
    pub THEME_SPLITTER_HORIZONTAL = 16.0
    pub THEME_SPLITTER_SIZE = 10.0
    pub THEME_SPLITTER_MIN_HORIZONTAL = 36.
    pub THEME_SPLITTER_MAX_HORIZONTAL = 46.
    pub THEME_SPLITTER_MIN_VERTICAL = 16.0
    pub THEME_SPLITTER_MAX_VERTICAL = 26.
    pub THEME_SPLITTER_SIZE = 5.0
    pub THEME_DOCK_BORDER_SIZE: 0.0

    // COLOR PALETTE
    pub THEME_COLOR_W = #FFFFFFFF

    pub THEME_COLOR_U_5 = #FFFFFFDD
    pub THEME_COLOR_U_4 = #FFFFFFAA
    pub THEME_COLOR_U_3 = #FFFFFF88
    pub THEME_COLOR_U_2 = #FFFFFF66
    pub THEME_COLOR_U_15 = #FFFFFF44
    pub THEME_COLOR_U_1 = #FFFFFF22
    pub THEME_COLOR_U_HIDDEN = #FFFFFF00

    pub THEME_COLOR_D_HIDDEN = #00000000
    pub THEME_COLOR_D_05 = #00000008
    pub THEME_COLOR_D_075 = #0000000A
    pub THEME_COLOR_D_1 = #00000011
    pub THEME_COLOR_D_2 = #00000022
    pub THEME_COLOR_D_3 = #00000044
    pub THEME_COLOR_D_4 = #00000066
    pub THEME_COLOR_D_5 = #00000088

    pub THEME_COLOR_BG_APP = #D
    pub THEME_COLOR_FG_APP = #E

    // BASICS
    pub THEME_COLOR_MAKEPAD = #FF5C39FF

    pub THEME_COLOR_SHADOW = #00000011
    pub THEME_COLOR_SHADOW_FOCUS = #00000022
    pub THEME_COLOR_SHADOW_DISABLED = #00000008
    pub THEME_COLOR_SHADOW_FLAT = #00000011
    pub THEME_COLOR_SHADOW_FLAT_DISABLED = #3
    pub THEME_COLOR_LIGHT = #FFFFFFAA
    pub THEME_COLOR_LIGHT_HOVER = #FFFFFFDD
    pub THEME_COLOR_LIGHT_FOCUS = #FFFFFFDD
    pub THEME_COLOR_LIGHT_DISABLED = #00000008

    pub THEME_COLOR_FLAT_FOCUS = #00000022

    pub THEME_COLOR_BG_HIGHLIGHT = #FFFFFF22
    pub THEME_COLOR_BG_UNFOCUSSED = #ffffff18
    pub THEME_COLOR_APP_CAPTION_BAR = #00000000
    pub THEME_COLOR_DRAG_QUAD = #FFFFFFDD
    pub THEME_COLOR_DRAG_TARGET_PREVIEW = #FFFFFF66

    pub THEME_COLOR_CURSOR = #FFFFFF
    pub THEME_COLOR_CURSOR_FOCUS = #FFFFFF
    pub THEME_COLOR_CURSOR_EMPTY = #FFFFFF
    pub THEME_COLOR_CURSOR_DISABLED = #FFFFFF00
    pub THEME_COLOR_CURSOR_BORDER = #FFFFFF

    pub THEME_COLOR_HIGHLIGHT = #f00
    pub THEME_COLOR_TEXT_CURSOR = #FFFFFF
    pub THEME_COLOR_BG_HIGHLIGHT_INLINE = #00000011

    pub THEME_COLOR_TEXT = #000000AA
    pub THEME_COLOR_TEXT_VAL = #00000044
    pub THEME_COLOR_TEXT_HL = #000000AA
    pub THEME_COLOR_TEXT_HOVER = #000000AA
    pub THEME_COLOR_TEXT_FOCUS = #000000AA
    pub THEME_COLOR_TEXT_DOWN = #000000AA
    pub THEME_COLOR_TEXT_DISABLED = #00000022
    pub THEME_COLOR_TEXT_PLACEHOLDER = #00000088
    pub THEME_COLOR_TEXT_PLACEHOLDER_HOVER = #000000AA
    pub THEME_COLOR_TEXT_META = #00000088

    pub THEME_COLOR_LABEL_INNER = #000000AA
    pub THEME_COLOR_LABEL_INNER_DOWN = #000000CC
    pub THEME_COLOR_LABEL_INNER_DRAG = #000000CC
    pub THEME_COLOR_LABEL_INNER_HOVER = #000000AA
    pub THEME_COLOR_LABEL_INNER_FOCUS = #000000AA
    pub THEME_COLOR_LABEL_INNER_ACTIVE = #000000AA
    pub THEME_COLOR_LABEL_INNER_INACTIVE = #00000088
    pub THEME_COLOR_LABEL_INNER_DISABLED = #00000022

    pub THEME_COLOR_LABEL_OUTER = #000000CC
    pub THEME_COLOR_LABEL_OUTER_OFF = #00000088
    pub THEME_COLOR_LABEL_OUTER_DOWN = #000000AA

    pub THEME_COLOR_LABEL_OUTER_DRAG = #000000AA
    pub THEME_COLOR_LABEL_OUTER_HOVER = #000000AA
    pub THEME_COLOR_LABEL_OUTER_FOCUS = #000000AA
    pub THEME_COLOR_LABEL_OUTER_ACTIVE = #000000AA
    pub THEME_COLOR_LABEL_OUTER_ACTIVE_FOCUS = #000000AA
    pub THEME_COLOR_LABEL_OUTER_DISABLED = #00000044


    pub THEME_COLOR_ICON = #FFFFFF66
    pub THEME_COLOR_ICON_ACTIVE = #00000066
    pub THEME_COLOR_ICON_DISABLED = #FFFFFFAA

    pub THEME_COLOR_BG_CONTAINER = #00000011
    pub THEME_COLOR_BG_EVEN = #ffffff44
    pub THEME_COLOR_BG_ODD = #ffffff00

    pub THEME_COLOR_BEVEL = #00000011
    pub THEME_COLOR_BEVEL_HOVER = #00000022
    pub THEME_COLOR_BEVEL_FOCUS = #00000022
    pub THEME_COLOR_BEVEL_ACTIVE = #00000011
    pub THEME_COLOR_BEVEL_EMPTY = #00000011
    pub THEME_COLOR_BEVEL_DOWN = #00000022
    pub THEME_COLOR_BEVEL_DRAG = #00000022
    pub THEME_COLOR_BEVEL_DISABLED = #3

    pub THEME_COLOR_BEVEL_INSET_1 = #FFFFFFAA
    pub THEME_COLOR_BEVEL_INSET_1_HOVER = #FFFFFFDD
    pub THEME_COLOR_BEVEL_INSET_1_FOCUS = #FFFFFFDD
    pub THEME_COLOR_BEVEL_INSET_1_ACTIVE = #FFFFFFAA
    pub THEME_COLOR_BEVEL_INSET_1_EMPTY = #FFFFFFAA
    pub THEME_COLOR_BEVEL_INSET_1_DOWN = #FFFFFFDD
    pub THEME_COLOR_BEVEL_INSET_1_DRAG = #FFFFFFDD
    pub THEME_COLOR_BEVEL_INSET_1_DISABLED = #00000008

    pub THEME_COLOR_BEVEL_INSET_2 = #00000011
    pub THEME_COLOR_BEVEL_INSET_2_HOVER = #00000011
    pub THEME_COLOR_BEVEL_INSET_2_FOCUS = #00000022
    pub THEME_COLOR_BEVEL_INSET_2_ACTIVE = #00000011
    pub THEME_COLOR_BEVEL_INSET_2_EMPTY = #00000011
    pub THEME_COLOR_BEVEL_INSET_2_DOWN = #00000011
    pub THEME_COLOR_BEVEL_INSET_2_DRAG = #00000011
    pub THEME_COLOR_BEVEL_INSET_2_DISABLED = #00000008

    pub THEME_COLOR_BEVEL_OUTSET_1 = #FFFFFFAA
    pub THEME_COLOR_BEVEL_OUTSET_1_HOVER = #FFFFFFDD
    pub THEME_COLOR_BEVEL_OUTSET_1_FOCUS = #FFFFFFDD
    pub THEME_COLOR_BEVEL_OUTSET_1_ACTIVE = #FFFFFFAA
    pub THEME_COLOR_BEVEL_OUTSET_1_DOWN = #00000011
    pub THEME_COLOR_BEVEL_OUTSET_1_DRAG = #00000011
    pub THEME_COLOR_BEVEL_OUTSET_1_DISABLED = #00000008

    pub THEME_COLOR_BEVEL_OUTSET_2 = #00000011
    pub THEME_COLOR_BEVEL_OUTSET_2_HOVER = #00000011
    pub THEME_COLOR_BEVEL_OUTSET_2_FOCUS = #00000011
    pub THEME_COLOR_BEVEL_OUTSET_2_ACTIVE = #00000011
    pub THEME_COLOR_BEVEL_OUTSET_2_DOWN = #FFFFFFAA
    pub THEME_COLOR_BEVEL_OUTSET_2_DRAG = #FFFFFFAA
    pub THEME_COLOR_BEVEL_OUTSET_2_FOCUS = #00000022
    pub THEME_COLOR_BEVEL_OUTSET_2_DISABLED = #00000008

    // Background of textinputs, radios, checkboxes etc.
    pub THEME_COLOR_INSET = #0000000A
    pub THEME_COLOR_INSET_HOVER = #00000008
    pub THEME_COLOR_INSET_DOWN = #00000008
    pub THEME_COLOR_INSET_ACTIVE = #00000008
    pub THEME_COLOR_INSET_FOCUS = #00000008
    pub THEME_COLOR_INSET_DRAG = #00000008
    pub THEME_COLOR_INSET_DISABLED = #FFFFFF22
    pub THEME_COLOR_INSET_EMPTY = #00000008

    pub THEME_COLOR_INSET_1 = #00000008
    pub THEME_COLOR_INSET_1_HOVER = #00000011
    pub THEME_COLOR_INSET_1_DOWN = #00000011
    pub THEME_COLOR_INSET_1_ACTIVE = #00000011
    pub THEME_COLOR_INSET_1_FOCUS = #00000011
    pub THEME_COLOR_INSET_1_DRAG = #00000008
    pub THEME_COLOR_INSET_1_DISABLED = #FFFFFF22
    pub THEME_COLOR_INSET_1_EMPTY = #00000008

    pub THEME_COLOR_INSET_2 = #00000011
    pub THEME_COLOR_INSET_2_HOVER = #00000022
    pub THEME_COLOR_INSET_2_DOWN = #00000022
    pub THEME_COLOR_INSET_2_ACTIVE = #00000022
    pub THEME_COLOR_INSET_2_FOCUS = #00000022
    pub THEME_COLOR_INSET_2_DRAG = #00000011
    pub THEME_COLOR_INSET_2_EMPTY = #FFFFFF00
    pub THEME_COLOR_INSET_2_DISABLED = #00000011

    // WIDGET COLORS
    pub THEME_COLOR_OUTSET = #FFFFFF88
    pub THEME_COLOR_OUTSET_DOWN = #FFFFFF22
    pub THEME_COLOR_OUTSET_HOVER = #FFFFFFAA
    pub THEME_COLOR_OUTSET_ACTIVE = #FFFFFFDD
    pub THEME_COLOR_OUTSET_FOCUS = #FFFFFF88
    pub THEME_COLOR_OUTSET_DRAG = #FFFFFFAA
    pub THEME_COLOR_OUTSET_DISABLED = #FFFFFF88
    pub THEME_COLOR_OUTSET_INACTIVE = #00000000

    pub THEME_COLOR_OUTSET_1 = #FFFFFFAA
    pub THEME_COLOR_OUTSET_1_DOWN = #00000022
    pub THEME_COLOR_OUTSET_1_DRAG = #00000022
    pub THEME_COLOR_OUTSET_1_HOVER = #FFFFFFAA
    pub THEME_COLOR_OUTSET_1_ACTIVE = #FFFFFFEE
    pub THEME_COLOR_OUTSET_1_FOCUS = #FFFFFFAA
    pub THEME_COLOR_OUTSET_1_DISABLED = #FFFFFF22

    pub THEME_COLOR_OUTSET_2 = #FFFFFF00
    pub THEME_COLOR_OUTSET_2_DOWN = #00000000
    pub THEME_COLOR_OUTSET_2_HOVER = #FFFFFF66
    pub THEME_COLOR_OUTSET_2_DRAG = #00000000
    pub THEME_COLOR_OUTSET_2_ACTIVE = #FFFFFFAA
    pub THEME_COLOR_OUTSET_2_FOCUS = #FFFFFF66
    pub THEME_COLOR_OUTSET_2_DISABLED = #FFFFFF00

    pub THEME_COLOR_MARK = #00000066
    pub THEME_COLOR_MARK_EMPTY = #00000008
    pub THEME_COLOR_MARK_OFF = #00000000
    pub THEME_COLOR_MARK_HOVER = #00000066
    pub THEME_COLOR_MARK_ACTIVE = #00000066
    pub THEME_COLOR_MARK_ACTIVE_HOVER = #00000066
    pub THEME_COLOR_MARK_FOCUS = #00000066
    pub THEME_COLOR_MARK_DOWN = #00000044
    pub THEME_COLOR_MARK_DISABLED = #00000022

    pub THEME_COLOR_SELECTION = #FFFFFF00
    pub THEME_COLOR_SELECTION_HOVER = #00000044
    pub THEME_COLOR_SELECTION_DOWN = #00000044
    pub THEME_COLOR_SELECTION_FOCUS = #00000044
    pub THEME_COLOR_SELECTION_EMPTY = #FFFFFF00
    pub THEME_COLOR_SELECTION_DISABLED = #FFFFFF00

    // Progress bars, slider amounts etc.
    pub THEME_COLOR_VAL = #9
    pub THEME_COLOR_VAL_HOVER = #A
    pub THEME_COLOR_VAL_FOCUS = #A
    pub THEME_COLOR_VAL_DRAG = #A
    pub THEME_COLOR_VAL_DISABLED = #00000000

    pub THEME_COLOR_VAL_1 = #4
    pub THEME_COLOR_VAL_1_HOVER = #6
    pub THEME_COLOR_VAL_1_FOCUS = #6
    pub THEME_COLOR_VAL_1_DRAG = #6
    pub THEME_COLOR_VAL_1_DISABLED = #00000000
    
    pub THEME_COLOR_VAL_2 = #3
    pub THEME_COLOR_VAL_2_HOVER = #4
    pub THEME_COLOR_VAL_2_FOCUS = #4
    pub THEME_COLOR_VAL_2_DRAG = #4
    pub THEME_COLOR_VAL_2_DISABLED = #00000000


    // WIDGET SPECIFIC COLORS
    pub THEME_COLOR_HANDLE: #6;
    pub THEME_COLOR_HANDLE_HOVER: #6;
    pub THEME_COLOR_HANDLE_FOCUS: #6;
    pub THEME_COLOR_HANDLE_DISABLED: #2;
    pub THEME_COLOR_HANDLE_DRAG: #6;

    pub THEME_COLOR_HANDLE_1: #FFFFFF;
    pub THEME_COLOR_HANDLE_1_HOVER: #FFFFFF;
    pub THEME_COLOR_HANDLE_1_FOCUS: #FFFFFF;
    pub THEME_COLOR_HANDLE_1_DISABLED: #1;
    pub THEME_COLOR_HANDLE_1_DRAG: #FFFFFF;

    pub THEME_COLOR_HANDLE_2: #8;
    pub THEME_COLOR_HANDLE_2_HOVER: #8;
    pub THEME_COLOR_HANDLE_2_FOCUS: #8;
    pub THEME_COLOR_HANDLE_2_DISABLED: #A;
    pub THEME_COLOR_HANDLE_2_DRAG: #8;

    pub THEME_COLOR_DOCK_TAB_ACTIVE = #E 

    // TODO: THESE ARE APPLICATION SPECIFIC COLORS THAT SHOULD BE MOVED FROM THE GENERAL THEME TO THE GIVEN PROJECT
    pub THEME_COLOR_HIGH = #C00
    pub THEME_COLOR_MID = #FA0
    pub THEME_COLOR_LOW = #8A0
    pub THEME_COLOR_PANIC = #f0f
    pub THEME_COLOR_ICON_WAIT = #8A0
    pub THEME_COLOR_ERROR = #C00
    pub THEME_COLOR_WARNING = #FA0
    pub THEME_COLOR_ICON_PANIC = #C00

    // TYPOGRAPHY
    pub THEME_FONT_SIZE_CODE = 9.0
    pub THEME_FONT_WDGT_LINE_SPACING = 1.2
    pub THEME_FONT_HL_LINE_SPACING = 1.05
    pub THEME_FONT_LONGFORM_LINE_SPACING = 1.2

    pub THEME_FONT_SIZE_1 = 26.
    pub THEME_FONT_SIZE_2 = 18.
    pub THEME_FONT_SIZE_3 = 14.
    pub THEME_FONT_SIZE_4 = 12.
    pub THEME_FONT_SIZE_P = 10.

    pub THEME_FONT_LABEL = {
        font_family:{
            latin = font("crate://self/resources/IBMPlexSans-Text.ttf", -0.1, 0.0),
            chinese = font(
                "crate://makepad_fonts_chinese_regular/resources/LXGWWenKaiRegular.ttf",
                "crate://makepad_fonts_chinese_regular_2/resources/LXGWWenKaiRegular.ttf.2",
                0.0, 
                0.0)
            emoji = font("crate://makepad_fonts_emoji/resources/NotoColorEmoji.ttf", 0.0, 0.0)
        },
        line_spacing: 1.2
    } // TODO: LEGACY, REMOVE. REQUIRED BY RUN LIST IN STUDIO ATM
    pub THEME_FONT_REGULAR = {
        font_family: {
            latin = font("crate://self/resources/IBMPlexSans-Text.ttf", -0.1, 0.0),
            chinese = font(
                "crate://makepad_fonts_chinese_regular/resources/LXGWWenKaiRegular.ttf",
                "crate://makepad_fonts_chinese_regular_2/resources/LXGWWenKaiRegular.ttf.2",
                0.0, 
                0.0)
            emoji = font("crate://makepad_fonts_emoji/resources/NotoColorEmoji.ttf", 0.0, 0.0)
        },
        line_spacing: 1.2
    }
    pub THEME_FONT_BOLD = {
        font_family:{
            latin = font("crate://self/resources/IBMPlexSans-SemiBold.ttf", -0.1, 0.0),
            chinese = font(
                "crate://makepad_fonts_chinese_bold/resources/LXGWWenKaiBold.ttf",
                "crate://makepad_fonts_chinese_bold_2/resources/LXGWWenKaiBold.ttf.2",
                0.0, 
                0.0)
            emoji = font("crate://makepad_fonts_emoji/resources/NotoColorEmoji.ttf", 0.0, 0.0)
        },
        line_spacing: 1.2
    }
    pub THEME_FONT_ITALIC = {
        font_family:{
            latin = font("crate://self/resources/IBMPlexSans-Italic.ttf", -0.1, 0.0),
            chinese = font(
                "crate://makepad_fonts_chinese_regular/resources/LXGWWenKaiRegular.ttf",
                "crate://makepad_fonts_chinese_regular_2/resources/LXGWWenKaiRegular.ttf.2",
                0.0, 
                0.0)
        },
        line_spacing: 1.2
    }
    pub THEME_FONT_BOLD_ITALIC = {
        font_family:{
            latin = font("crate://self/resources/IBMPlexSans-BoldItalic.ttf", -0.1, 0.0),
            chinese = font(
                "crate://makepad_fonts_chinese_bold/resources/LXGWWenKaiBold.ttf",
                "crate://makepad_fonts_chinese_bold_2/resources/LXGWWenKaiBold.ttf.2",
                0.0, 
                0.0)
        },
        line_spacing: 1.2
    }
    pub THEME_FONT_CODE = {
        font_size: (THEME_FONT_SIZE_CODE),
        font_family:{
            latin = font("crate://self/resources/LiberationMono-Regular.ttf", 0.0, 0.0)
        },
        line_spacing: 1.35
    }
    pub THEME_FONT_ICONS = {
        font_family:{
            latin = font("crate://self/resources/fa-solid-900.ttf", 0.0, 0.0)
        },
        line_spacing: 1.2,
    }
}