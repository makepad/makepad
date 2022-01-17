use crate::makepad_render::*;

live_register!{
    
    // RELATIVE DEFS
        
    const COLOR_WHITE: #FFF
    const COLOR_80_UP: #FFFFFFCC
    const COLOR_40_UP: #FFFFFF66
    const COLOR_25_UP: #FFFFFF40
    const COLOR_15_UP: #FFFFFF2a
    const COLOR_10_UP: #FFFFFF1A
    const COLOR_2_UP: #FFFFFF0a
    const COLOR_7_DOWN: #00000013
    const COLOR_20_DOWN: #00000035
    const COLOR_30_DOWN: #00000050
    const COLOR_50_DOWN: #00000080
    const COLOR_BLACK: #000
    
    // CORE BACKGROUND COLORS

    const COLOR_BG_APP: #26
    
    const COLOR_BG_HEADER: (blend(
        COLOR_BG_APP,
        COLOR_10_UP
    ))
    
    const COLOR_BG_EDITOR: (blend(
        COLOR_BG_HEADER,
        COLOR_20_DOWN
    ))
    
     const COLOR_BG_ODD: (blend(
        COLOR_BG_EDITOR,
        COLOR_7_DOWN
    ))
    
    const COLOR_BG_SELECTED:(blend(
        COLOR_BG_EDITOR,
        COLOR_15_UP
    ))
    
    const COLOR_BG_MARKED: (blend(
        COLOR_BG_EDITOR,
        COLOR_2_UP
    ))
    
    // Text / Icon colors
    
    const COLOR_TEXT_SELECTED: (blend(
        COLOR_BG_HEADER,
        COLOR_80_UP
    ))
    
    const COLOR_TEXT_HOVER: (COLOR_WHITE)
    
    const COLOR_TEXT_DEFAULT: (blend(
        COLOR_TEXT_SELECTED,
        COLOR_30_DOWN
    ))
    
     const COLOR_TEXT_META: (blend(
        COLOR_TEXT_SELECTED,
        COLOR_50_DOWN
    ))
    
    // Splitter and scrollbar
    
    const COLOR_SCROLL_BAR_DEFAULT: #fff3
    const COLOR_CONTROL_HOVER: (blend(
        COLOR_BG_HEADER,
        COLOR_40_UP
    ))
    const COLOR_CONTROL_PRESSED: (blend(
        COLOR_BG_HEADER,
        COLOR_10_UP
    ))
    
    const COLOR_DRAG_QUAD: #ffffff80
    
    const DIM_TAB_HEIGHT: 26.0,
    const DIM_SPLITTER_HORIZONTAL: 16.0,
    const DIM_SPLITTER_MIN_HORIZONTAL: (DIM_TAB_HEIGHT),
    const DIM_SPLITTER_MAX_HORIZONTAL: (DIM_TAB_HEIGHT+DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_MIN_VERTICAL: (DIM_SPLITTER_HORIZONTAL),
    const DIM_SPLITTER_MAX_VERTICAL: (DIM_SPLITTER_HORIZONTAL+DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_SIZE: 5.0    
}

