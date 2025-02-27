use crate::{
    makepad_derive_widget::*,
    makepad_draw::{text::layout::Cursor, *}, *,
    widget::*,
};


live_design! {
    link widgets;

    use link::theme::*;

    pub TextInput2Base = {{TextInput2}} {}
    
    pub TextInput2 = <TextInput2Base> {
        width: 200,
        height: Fit,

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            wrap: Word,
            text_style: {
                font_family: (THEME_FONT_FAMILY_REGULAR),
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: 16.0
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput2 {
    #[redraw] #[live] draw_bg: DrawColor,
    #[live] draw_text: DrawText2,
    #[live] draw_cursor: DrawQuad,

    #[layout] layout: Layout,
    #[walk] text_walk: Walk,
    #[live] text_align: Align,

    #[live] pub text: String,
    #[rust] text_area: Area,
    #[rust] cursor: Cursor,
}

impl Widget for TextInput2 {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let text_rect = self.draw_text.draw_walk(
            cx,
            self.text_walk,
            self.text_align,
            &self.text,
        );
        self.text_area = Area::Empty;
        cx.add_aligned_rect_area(&mut self.text_area, text_rect);
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // TODO
    }
}