use crate::{
    makepad_derive_widget::*,
    makepad_draw::{text::layout::Cursor, *}, *,
    widget::*,
};


live_design! {
    link widgets;

    use link::theme::*;
    use makepad_draw::shader::std::*;

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

        draw_cursor: {
            instance focus: 1.0 // TODO: Animate this
            uniform border_radius: 0.5
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_TEXT_CURSOR, self.focus));
                return sdf.result
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
        let text = self.draw_text.layout(cx, self.text_walk, self.text_align, &self.text);
        let text_rect = self.draw_text.draw_walk_laidout(
            cx,
            self.text_walk,
            self.text_align,
            &text,
        );
        cx.add_aligned_rect_area(&mut self.text_area, text_rect);
        let cursor = text.cursor_to_position(self.cursor);
        let cursor_row = &text.rows[cursor.row_index];
        let cursor_ascender_in_lpxs = cursor_row.ascender_in_lpxs;
        let cursor_descender_in_lpxs = cursor_row.descender_in_lpxs;
        self.draw_cursor.draw_abs(
            cx,
            rect(
                text_rect.pos.x + cursor.origin_in_lpxs.x as f64 - 2.0 / 2.0,
                text_rect.pos.y + (cursor.origin_in_lpxs.y - cursor_ascender_in_lpxs) as f64,
                2.0,
                (cursor_ascender_in_lpxs - cursor_descender_in_lpxs) as f64,
            )
        );
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // TODO
    }
}