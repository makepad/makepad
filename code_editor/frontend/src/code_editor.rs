use {
    makepad_code_editor_core::{state::ViewId, Pos, State, sel},
    makepad_widgets::*,
    std::iter::Peekable,
};

live_design! {
    import makepad_widgets::theme::*;

    CodeEditor = {{CodeEditor}} {
        draw_grapheme: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_caret: {
            draw_depth: 2.0,
            color: #08F8
        }   
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    draw_grapheme: DrawText,
    #[live]
    draw_caret: DrawColor,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, state: &State, view: ViewId) {
        let cell_size =
            self.draw_grapheme.text_style.font_size * self.draw_grapheme.get_monospace_base(cx);
        state.draw(view, |text, sel| {
            let mut drawer = Drawer {
                draw_grapheme: &mut self.draw_grapheme,
                draw_caret: &mut self.draw_caret,
                cell_size,
                active_sel_region: None,
                pending_sel_regions: sel.iter().peekable(),
                text_pos: Pos::default(),
                screen_pos: DVec2::default(),
            };
            for line in text.as_lines() {
                drawer.draw_line(cx, line);
            }
        });
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // TODO
    }
}

struct Drawer<'a> {
    draw_grapheme: &'a mut DrawText,
    draw_caret: &'a mut DrawColor,
    cell_size: DVec2,
    active_sel_region: Option<ActiveSelRegion>,
    pending_sel_regions: Peekable<sel::Iter<'a>>,
    text_pos: Pos,
    screen_pos: DVec2,
}

impl<'a> Drawer<'a> {
    fn draw_line(&mut self, cx: &mut Cx2d, line: &str) {
        use makepad_code_editor_core::str::StrExt;

        for grapheme in line.graphemes() {
            self.check_sel_region_start();
            self.draw_grapheme(cx, grapheme);
            self.check_sel_region_end(cx);
            self.text_pos.byte += grapheme.len(); 
            self.screen_pos.x += self.cell_size.x;
        }
        if let Some(&region) = self.active_sel_region.as_ref() {
            self.draw_sel_region(cx, region);
        }
        self.text_pos.byte = 0;
        self.text_pos.line += 1;
        self.screen_pos.x = 0.0;
        self.screen_pos.y += self.cell_size.y;
    }

    fn check_sel_region_start(&mut self) {
        if self.pending_sel_regions.peek().map_or(false, |region| {
            region.start() == self.text_pos
        }) {
            self.active_sel_region = Some(ActiveSelRegion {
                inner: self.pending_sel_regions.next().unwrap(),
                start_x: self.screen_pos.x,
            });   
        }
    }

    fn check_sel_region_end(&mut self, cx: &mut Cx2d) {
        if self.active_sel_region.as_ref().map_or(false, |region| {
            region.inner.end() == self.text_pos
        }) {
            let region = self.active_sel_region.take().unwrap();
            self.draw_sel_region(cx, region);
        }
    }

    fn draw_grapheme(&mut self, cx: &mut Cx2d, grapheme: &str) {
        self.draw_grapheme.draw_abs(cx, self.screen_pos, grapheme);
    }

    fn draw_sel_region(&mut self, cx: &mut Cx2d, region: ActiveSelRegion) {
        if region.inner.active.line == self.text_pos.line {
            self.draw_caret(cx);
        }
    }

    fn draw_caret(&mut self, cx: &mut Cx2d) {
        println!("PENELEIN {:?}", self.screen_pos);
        self.draw_caret.draw_abs(
            cx,
            Rect {
                pos: self.screen_pos,
                size: DVec2 {
                    x: 1.0,
                    y: self.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy)]
struct ActiveSelRegion {
    inner: sel::Region,
    start_x: f64,
}