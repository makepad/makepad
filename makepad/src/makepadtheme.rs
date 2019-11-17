use render::*;

theme_layout!(LogList_layout_item);

theme_layout!(FileTree_layout_drag_bg);
theme_layout!(FileTree_layout_node);
theme_text_style!(FileTree_text_style);
theme_walk!(FileTree_walk_filler);
theme_walk!(FileTree_walk_folder);

pub fn set_makepad_theme_values(cx: &mut Cx){
    FileTree_text_style::set_base(cx, TextStyle {
        top_drop: 1.3,
        ..TextStyle::default()
    });
    
    LogList_layout_item::set_base(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        align: Align::left_center(),
        padding: Padding::zero(), // {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });
    
    FileTree_layout_drag_bg::set_base(cx, Layout {
        padding: Padding {l: 5., t: 5., r: 5., b: 5.},
        walk: Walk::wh(Width::Compute, Height::Compute),
        ..Default::default()
    });
    
    FileTree_layout_node::set_base(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        align: Align::left_center(),
        padding: Padding {l: 5., t: 0., r: 0., b: 1.},
        ..Default::default()
    });
    
    FileTree_walk_filler::set_base(cx, Walk{
        width:Width::Fix(10.),
        height:Height::Fill,
        margin:Margin {l: 1., t: 0., r: 4., b: 0.}
    });

    FileTree_walk_folder::set_base(cx, Walk{
        width:Width::Fix(14.), 
        height:Height::Fill, 
        margin: Margin {l: 0., t: 0., r: 2., b: 0.}
    });
}

pub fn set_dark_makepad_theme(cx: &mut Cx) {
    set_makepad_theme_values(cx);
}