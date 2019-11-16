use render::*;

theme_layout!(LayoutLogListItem);
theme_layout!(LayoutFileTreeDragBg);
theme_layout!(LayoutFileTreeNode);

theme_text_style!(TextStyleFileTree);

theme_walk!(WalkFileTreeFiller);
theme_walk!(WalkFileTreeFolder);

pub fn set_makepad_theme_values(cx: &mut Cx){
    TextStyleFileTree::set(cx, TextStyle {
        top_drop: 1.3,
        ..TextStyle::default()
    });
    
    LayoutLogListItem::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        align: Align::left_center(),
        padding: Padding::zero(), // {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });
    
    LayoutFileTreeDragBg::set(cx, Layout {
        padding: Padding {l: 5., t: 5., r: 5., b: 5.},
        walk: Walk::wh(Width::Compute, Height::Compute),
        ..Default::default()
    });
    
    LayoutFileTreeNode::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        align: Align::left_center(),
        padding: Padding {l: 5., t: 0., r: 0., b: 1.},
        ..Default::default()
    });
    
    WalkFileTreeFiller::set(cx, Walk{
        width:Width::Fix(10.),
        height:Height::Fill,
        margin:Margin {l: 1., t: 0., r: 4., b: 0.}
    });

    WalkFileTreeFolder::set(cx, Walk{
        width:Width::Fix(14.), 
        height:Height::Fill, 
        margin: Margin {l: 0., t: 0., r: 2., b: 0.}
    });
}

pub fn set_dark_makepad_theme(cx: &mut Cx) {
    set_makepad_theme_values(cx);
}