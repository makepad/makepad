use makepad_render::*;
live_register!{
    const COLOR_APP_BG: #26
    const COLOR_DOCK_BG: #38
    const COLOR_DOCK_TAB_ACTIVE: #30
    
    const COLOR_WHITE: #FFF
    const COLOR_80_UP: #FFFFFFCC
    const COLOR_40_UP: #FFFFFF66
    const COLOR_25_UP: #FFFFFF40
    const COLOR_10_UP: #FFFFFF1A
    const COLOR_2_UP: #FFFFFF05
    const COLOR_15_DOWN: #00000026
    const COLOR_25_DOWN: #00000040
    const COLOR_50_DOWN: #00000080
    const COLOR_BLACK: #000
    
    const COLOR_EVEN: (blend(
        COLOR_DOCK_BG,
        COLOR_15_DOWN
    ))
    const COLOR_ODD: (blend(
        COLOR_EVEN,
        COLOR_15_DOWN
    ))
    const COLOR_WINDOW_BG: (blend(
        COLOR_DOCK_BG,
        COLOR_15_DOWN
    ))
    
    const COLOR_SELECTED: #x11466E
    
    const COLOR_FILE: #9d
    const COLOR_FOLDER: #ff
    
    const COLOR_ICON: #80
    
    const COLOR_DRAG_QUAD: #ffffff80
    
}

