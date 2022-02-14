use crate::makepad_platform::*;

live_register!{
    
    FONT_LABEL: {
        font_size: 9.4,
        font: {
            path: "resources/IBMPlexSans-Text.ttf"
        }
    }
    
    
    FONT_DATA: {
        font_size: 9.4,
        font: {
            path: "resources/IBMPlexSans-Text.ttf"
        }
    }
    
    
    FONT_META: {
        font_size: 9.4,
        top_drop: 1.2,
        font: {
            path: "resources/IBMPlexSans-Text.ttf"
        }
    }
    
    FONT_CODE: {
        font: {
            path: "resources/LiberationMono-Regular.ttf"
        }
        brightness: 1.1
        font_size: 9.0
        line_spacing: 2.0
        top_drop: 1.3
    }
    
    DIM_DATA_ITEM_HEIGHT: 23.0
    DIM_DATA_ICON_WIDTH: 16.0
    DIM_DATA_ICON_HEIGHT: 24.0
    // ABSOLUTE DEFS
    
    const BRIGHTNESS: #x40
    const COLOR_HIGHLIGHT: #42
    const COLOR_HIGH: #C00
    const COLOR_MID: #FA0
    const COLOR_LOW: #8A0
    
    // RELATIVE DEFS
    //    42, 78, 117
    const COLOR_WHITE: #FFF
    const COLOR_UP_80: #FFFFFFCC
    const COLOR_UP_50: #FFFFFF80
    const COLOR_UP_25: #FFFFFF40
    const COLOR_UP_15: #FFFFFF26
    const COLOR_UP_10: #FFFFFF1A
    const COLOR_UP_4: #FFFFFF0A
    const COLOR_DOWN_7: #00000013
    const COLOR_DOWN_10: #00000030
    const COLOR_DOWN_20: #00000040
    const COLOR_DOWN_50: #00000080
    const COLOR_BLACK: #000
    
    // CORE BACKGROUND COLORS
    
    const COLOR_BG_APP: (BRIGHTNESS)
    
    const COLOR_BG_HEADER: (blend(
        COLOR_BG_APP,
        COLOR_DOWN_10
    ))
    
    //const COLOR_CLEAR: (COLOR_BG_APP)
    
    const COLOR_BG_EDITOR: (blend(
        COLOR_BG_HEADER,
        COLOR_DOWN_10
    ))
    
    const COLOR_BG_ODD: (blend(
        COLOR_BG_EDITOR,
        COLOR_DOWN_7
    ))
    
    const COLOR_BG_SELECTED: (COLOR_HIGHLIGHT)
    
    const COLOR_BG_UNFOCUSSED: (blend(
        COLOR_BG_EDITOR,
        COLOR_UP_10
    ))
    
    const COLOR_EDITOR_SELECTED: (COLOR_BG_SELECTED)
    const COLOR_EDITOR_SELECTED_UNFOCUSSED: (COLOR_BG_SELECTED_UNFOCUSSED)
    
    const COLOR_BG_CURSOR: (blend(
        COLOR_BG_EDITOR,
        COLOR_UP_4
    ))
    
    const COLOR_FG_CURSOR: (blend(
        COLOR_BG_EDITOR,
        COLOR_UP_50
    ))
    
    // TEXT / ICON COLORS
    
    const COLOR_TEXT_DEFAULT: (COLOR_UP_50)
    const COLOR_TEXT_HOVER: (COLOR_UP_80)
    const COLOR_TEXT_META: (COLOR_UP_25)
    const COLOR_TEXT_SELECTED: (COLOR_UP_80)
    
    // SPLITTER AND SCROLLBAR
    
    const COLOR_SCROLL_BAR_DEFAULT: (COLOR_UP_10)
    
    const COLOR_CONTROL_HOVER: (blend(
        COLOR_BG_HEADER,
        COLOR_UP_50
    ))
    
    const COLOR_CONTROL_PRESSED: (blend(
        COLOR_BG_HEADER,
        COLOR_DOWN_10
    ))
    
    // ICON COLORS
    
    const COLOR_ICON_WAIT: (COLOR_LOW),
    const COLOR_ERROR: (COLOR_HIGH),
    const COLOR_WARNING: (COLOR_MID),
    const COLOR_ICON_PANIC: (COLOR_HIGH)
    const COLOR_DRAG_QUAD: (COLOR_UP_50)
    
    const DIM_TAB_HEIGHT: 26.0,
    const DIM_SPLITTER_HORIZONTAL: 16.0,
    const DIM_SPLITTER_MIN_HORIZONTAL: (DIM_TAB_HEIGHT),
    const DIM_SPLITTER_MAX_HORIZONTAL: (DIM_TAB_HEIGHT + DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_MIN_VERTICAL: (DIM_SPLITTER_HORIZONTAL),
    const DIM_SPLITTER_MAX_VERTICAL: (DIM_SPLITTER_HORIZONTAL + DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_SIZE: 5.0
}

