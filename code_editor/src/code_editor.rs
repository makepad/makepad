use {
    crate::{
        state::{Block, SessionId, Wrapped},
        token, State, Token,
    },
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::theme::*;

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        },
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, state: &mut State, session_id: SessionId) {
        let mut view = state.view_mut(session_id);
        let viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        let cell_size = self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        view.set_max_column((viewport_rect.size.x / cell_size.x) as usize);
        let start = view.find_first_line_ending_after_y(viewport_rect.pos.y / cell_size.y);
        let end = view.find_first_line_starting_after_y(
            (viewport_rect.pos.y + viewport_rect.size.y) / cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
        let mut y = 0.0;
        for block in view.blocks(0..view.line_count()) {
            match block {
                Block::Line { line, .. } => {
                    let mut token_iter = line.tokens().iter().copied();
                    let mut token_slot = token_iter.next();
                    let mut column = 0;
                    for wrapped in line.wrappeds() {
                        match wrapped {
                            Wrapped::Text {
                                is_inlay: false,
                                mut text,
                            } => {
                                while !text.is_empty() {
                                    let token = match token_slot {
                                        Some(token) => {
                                            if text.len() < token.len {
                                                token_slot = Some(Token {
                                                    kind: token.kind,
                                                    len: token.len - text.len(),
                                                });
                                                Token {
                                                    kind: token.kind,
                                                    len: text.len(),
                                                }
                                            } else {
                                                token_slot = token_iter.next();
                                                token
                                            }
                                        }
                                        None => Token {
                                            kind: token::Kind::Unknown,
                                            len: text.len(),
                                        },
                                    };
                                    let (text_0, text_1) = text.split_at(token.len);
                                    text = text_1;
                                    self.draw_text.draw_abs(
                                        cx,
                                        DVec2 {
                                            x: line.column_to_x(column),
                                            y,
                                        } * cell_size
                                            - viewport_rect.pos,
                                        text_0,
                                    );
                                }
                            }
                            Wrapped::Text {
                                is_inlay: true,
                                text,
                            } => {
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * cell_size
                                        - viewport_rect.pos,
                                    text,
                                );
                            }
                            Wrapped::Widget(widget) => {
                                column += widget.column_count;
                            }
                            Wrapped::Wrap => {
                                column = line.indent();
                                y += line.scale();
                            }
                        }
                    }
                    y += line.scale();
                }
                Block::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        cx.turtle_mut()
            .set_used(view.width() * cell_size.x, view.height() * cell_size.y);
        self.scroll_bars.end(cx);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        _state: &mut State,
        _session_id: SessionId,
        event: &Event,
    ) {
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
    }
}
