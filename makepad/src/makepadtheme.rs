use render::*;

theme_layout!(LogListLayout_item);

theme_layout!(FileTreeLayout_drag_bg);
theme_layout!(FileTreeLayout_node);
theme_text_style!(FileTreeTextStyle);
theme_walk!(FileTreeWalk_filler);
theme_walk!(FileTreeWalk_folder);

pub fn set_makepad_theme_values(cx: &mut Cx){
    FileTreeTextStyle::set(cx, TextStyle {
        top_drop: 1.3,
        ..TextStyle::default()
    });
    
    LogListLayout_item::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        align: Align::left_center(),
        padding: Padding::zero(), // {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });
    
    FileTreeLayout_drag_bg::set(cx, Layout {
        padding: Padding {l: 5., t: 5., r: 5., b: 5.},
        walk: Walk::wh(Width::Compute, Height::Compute),
        ..Default::default()
    });
    
    FileTreeLayout_node::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        align: Align::left_center(),
        padding: Padding {l: 5., t: 0., r: 0., b: 1.},
        ..Default::default()
    });
    
    FileTreeWalk_filler::set(cx, Walk{
        width:Width::Fix(10.),
        height:Height::Fill,
        margin:Margin {l: 1., t: 0., r: 4., b: 0.}
    });

    FileTreeWalk_folder::set(cx, Walk{
        width:Width::Fix(14.), 
        height:Height::Fill, 
        margin: Margin {l: 0., t: 0., r: 2., b: 0.}
    });
}

pub fn set_dark_makepad_theme(cx: &mut Cx) {
    set_makepad_theme_values(cx);
}