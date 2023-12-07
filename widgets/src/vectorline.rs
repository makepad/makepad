use crate::{makepad_draw::*, *};

live_design! {
    VectorLine = {{VectorLine}} {
        width: Fill,
        height: Fill,
        line_align: Top,
        color: #f,
        line_width: 10
    }
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum LineAlign {
    Free,
    Left,
    #[pick]
    Top,
    DiagonalBottomLeftTopRight,
    DiagonalTopLeftBottomRight,
    Right,
    Bottom,
    VerticalCenter,
    HorizontalCenter,
}

#[derive(Live)]
pub struct VectorLine {
    #[animator]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[live]
    draw_ls: DrawLine,
    #[rust]
    area: Area,
    #[live(15.0)]
    line_width: f64,
    #[live]
    color: Vec4,
    #[live(true)]
    contained: bool,
    #[live(LineAlign::Top)]
    line_align: LineAlign,
    #[rust(dvec2(350., 10.))]
    line_start: DVec2,
    #[rust(dvec2(1000., 1440.))]
    line_end: DVec2,
}

#[derive(Clone, WidgetAction)]
pub enum LineAction {
    None,
    Pressed,
    Clicked,
    Released,
}

impl Widget for VectorLine {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut WidgetScope,
    ) -> WidgetActions {
        let actions = WidgetActions::new();
        self.animator_handle_event(cx, event);
        actions
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut WidgetScope, walk: Walk) -> WidgetDraw {
        // lets draw a bunch of quads
        let fullrect = cx.walk_turtle_with_area(&mut self.area, walk);

        let mut rect = fullrect;
        let hw = self.line_width / 2.;
        if self.contained == false {
            rect.size.x += self.line_width;
            rect.size.y += self.line_width;
            rect.pos.x -= hw;
            rect.pos.y -= hw;
        }
        // self.line_width = 10.5;

        let mut line_start = self.line_start;
        let mut line_end = self.line_end;

        match self.line_align {
            LineAlign::Top => {
                line_start = dvec2(rect.pos.x + hw, rect.pos.y + hw);
                line_end = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + hw);
            }
            LineAlign::Bottom => {
                line_start = dvec2(rect.pos.x + hw, rect.pos.y + rect.size.y - hw);
                line_end = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + rect.size.y - hw);
            }
            LineAlign::Right => {
                line_start = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + hw);
                line_end = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + rect.size.y - hw);
            }
            LineAlign::Left => {
                line_start = dvec2(rect.pos.x + hw, rect.pos.y + hw);
                line_end = dvec2(rect.pos.x + hw, rect.pos.y + rect.size.y - hw);
            }
            LineAlign::HorizontalCenter => {
                line_start = dvec2(rect.pos.x + hw, rect.pos.y + rect.size.y / 2.);
                line_end = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + rect.size.y / 2.);
            }
            LineAlign::VerticalCenter => {
                line_start = dvec2(rect.pos.x + rect.size.x / 2., rect.pos.y + hw);
                line_end = dvec2(rect.pos.x + rect.size.x / 2., rect.pos.y + rect.size.y - hw);
            }
            LineAlign::DiagonalTopLeftBottomRight => {
                line_start = dvec2(rect.pos.x + hw, rect.pos.y + hw);
                line_end = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + rect.size.y - hw);
            }
            LineAlign::DiagonalBottomLeftTopRight => {
                line_start = dvec2(rect.pos.x + hw, rect.pos.y + rect.size.y - hw);
                line_end = dvec2(rect.pos.x + rect.size.x - hw, rect.pos.y + hw);
            }
            _ => {}
        }

        self.draw_ls
            .draw_line_abs(cx, line_start, line_end, self.color, self.line_width);

        WidgetDraw::done()
    }
}

impl LiveHook for VectorLine {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, VectorLine)
    }

    fn after_new_from_doc(&mut self, _cx: &mut Cx) {}
}
