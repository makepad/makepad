use crate::makepad_render::*;

live_register!{
    
    const COLOR_APP_BG: #26
    
    const COLOR_WHITE: #FFF
    const COLOR_80_UP: #FFFFFFCC
    const COLOR_40_UP: #FFFFFF66
    const COLOR_25_UP: #FFFFFF40
    const COLOR_17_UP: #FFFFFF30
    const COLOR_10_UP: #FFFFFF1A
    const COLOR_2_UP: #FFFFFF05
    const COLOR_7_DOWN: #00000013
    const COLOR_15_DOWN: #00000026
    const COLOR_25_DOWN: #00000040
    const COLOR_37_DOWN: #00000060
    const COLOR_50_DOWN: #00000080
    const COLOR_BLACK: #000
    
    const COLOR_WINDOW_BG: (COLOR_EDITOR_BG)
    const COLOR_WINDOW_CAPTION: (COLOR_APP_BG)
    
    const COLOR_DOCK_BG: (blend(
        COLOR_APP_BG,
        COLOR_10_UP
    ))
    
    const COLOR_EDITOR_BG: (blend(
        COLOR_DOCK_BG,
        COLOR_25_DOWN
    ))
     
    // TABS
    
    const COLOR_TAB_BG_SELECTED: (blend(
        COLOR_DOCK_BG,
        COLOR_25_DOWN
    ))
    
    const COLOR_TAB_BG_UNSELECTED: (COLOR_DOCK_BG)
    
    const COLOR_TAB_TEXT_SELECTED: (blend(
        COLOR_DOCK_BG,
        COLOR_80_UP
    ))
    
    const COLOR_TAB_TEXT_UNSELECTED: (blend(
        COLOR_DOCK_BG,
        COLOR_40_UP
    ))
    
    const COLOR_TAB_CLOSE_DEFAULT: (blend(
        COLOR_DOCK_BG,
        COLOR_40_UP
    ))
    
    const COLOR_TAB_CLOSE_HOVER: (blend(
        COLOR_DOCK_BG,
        COLOR_80_UP
    ))
    
    const COLOR_DRAG_QUAD: #ffffff80
    
    const DIM_TAB_HEIGHT: 26.0,

    // even/odd BG items
    
    const COLOR_BG_ODD: (blend(
        COLOR_EDITOR_BG,
        COLOR_7_DOWN
    ))
    const COLOR_BG_EVEN: (COLOR_EDITOR_BG)
    
    const COLOR_BG_SELECTED: (blend(
        COLOR_EDITOR_BG,
        COLOR_37_DOWN
    ))
    
    const COLOR_BG_HOVER: (blend(
        COLOR_EDITOR_BG,
        COLOR_10_UP
    ))
    
    
    // CODE EDITOR
    
    const COLOR_TEXT_SELECTION:(blend(
        COLOR_EDITOR_BG,
        COLOR_17_UP
    ))
    
    const COLOR_TEXT_SELECTION_BORDER:#fff
    
    
    // FILETREE
    
    
    const COLOR_TREE_TEXT_DEFAULT: (blend(
        COLOR_DOCK_BG,
        COLOR_40_UP
    ))
    
    const COLOR_TREE_TEXT_HOVER: (blend(
        COLOR_DOCK_BG,
        COLOR_80_UP
    ))

    const COLOR_TREE_TEXT_SELECTED: (blend(
        COLOR_DOCK_BG,
        COLOR_80_UP
    ))
    
    const COLOR_ICON: (blend(
        COLOR_DOCK_BG,
        COLOR_40_UP
    ))
    
    
    // SCROLL BAR
    
    
    const COLOR_SCROLL_BAR_DEFAULT: #fff3
    
    const COLOR_SCROLL_BAR_HOVER: (blend(
        COLOR_DOCK_BG,
        COLOR_40_UP
    ))
    
    const COLOR_SCROLL_BAR_PRESSED: (blend(
        COLOR_DOCK_BG,
        COLOR_10_UP
    ))
    
    
    // SPLITTER
    
    
    const COLOR_SPLITTER_BG: (COLOR_APP_BG)
 
    
    const COLOR_SPLITTER_HOVER: (blend(
        COLOR_DOCK_BG,
        COLOR_40_UP
    ))
    
    const COLOR_SPLITTER_PRESSED: (blend(
        COLOR_DOCK_BG,
        COLOR_10_UP
    ))

    const DIM_SPLITTER_HORIZONTAL: 16.0,
    const DIM_SPLITTER_MIN_HORIZONTAL: (DIM_TAB_HEIGHT),
    const DIM_SPLITTER_MAX_HORIZONTAL: (DIM_TAB_HEIGHT+DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_MIN_VERTICAL: (DIM_SPLITTER_HORIZONTAL),
    const DIM_SPLITTER_MAX_VERTICAL: (DIM_SPLITTER_HORIZONTAL+DIM_SPLITTER_SIZE),
    const DIM_SPLITTER_SIZE: 5.0    
}

