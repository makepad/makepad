use render::*;

theme_layout!(LayoutCodeEditor);
theme_text_style!()

pub fn set_editor_dark_theme(cx: &mut Cx) {
    LayoutCodeEditor::set(cx, Layout {
        width: Width::Fill,
        height: Height::Fill,
        margin: Margin::all(0.),
        padding: Padding {l: 4.0, t: 4.0, r: 4.0, b: 4.0},
        ..Default::default()
    });
}