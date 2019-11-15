use render::*;

theme_layout!(LayoutCodeEditor);
theme_text_style!(TextStyleCodeEditorText);
theme_walk!(WalkCodeIcon);

pub fn set_editor_theme_values(cx: &mut Cx) {
    
    TextStyleCodeEditorText::set(cx, TextStyle{
        font_path: "resources/LiberationMono-Regular.ttf".to_string(),
        brightness: 1.1,
        line_spacing: 1.8,
        top_drop: 1.3,
        ..TextStyle::default()
    });
    
    LayoutCodeEditor::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fill),
        padding: Padding {l: 4.0, t: 4.0, r: 4.0, b: 4.0},
        ..Layout::default()
    });
    
    WalkCodeIcon::set(cx, Walk{
        width: Width::Fix(14.0),
        height: Height::Fix(14.0),
        margin: Margin {l: 0., t: 0.5, r: 4., b: 0.},
    });
}

pub fn set_dark_editor_theme(cx: &mut Cx) {
    set_editor_theme_values(cx);
}

