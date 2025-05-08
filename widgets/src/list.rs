use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    scroll_bar::*,
};

const GAP_BETWEEN_ITEM_AND_BAR: f64 = 6.;

live_design! {
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use crate::scroll_bar::ScrollBar;

    pub List = {{List}} {
        scroll_bar: <ScrollBar> {}
    }
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
enum Orientation {
    Horizontal,
    #[pick] Vertical
}


#[derive(Live, Widget, LiveHook)]
struct List {
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[rust] area: Area,

    #[live] orientation: Orientation,
    #[live] scroll_bar: ScrollBar,

    #[rust] items: Vec<WidgetRef>,
    #[rust(0.)] scroll_pos: f64,
}

impl Widget for List {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.scroll_bar.handle_event_with(cx, event, &mut |cx, action| {
            if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                self.scroll_pos = scroll_pos;
                cx.redraw_all();
            }
        });

        if let Event::Scroll(e) = event {
            if self.area.rect(cx).contains(e.abs) {
                self.scroll_pos += e.scroll.y * 4.;
                // Clamping is handled in draw_walk, update scroll bar here
                self.scroll_bar.set_scroll_pos_no_action(cx, self.scroll_pos);
                cx.redraw_all();
            }
        }

        self.items.iter().for_each(|item| {
            item.handle_event(cx, event, scope);
        });
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let viewport = cx.turtle().rect();
        cx.begin_turtle(walk, self.layout);

        let (scroll_axis, scrollbar_view_rect, scrollbar_view_total) = match self.orientation {
            Orientation::Horizontal => {
                let (item_widths, cumulative_widths, max_item_height) = self.compute_the_values_for_horizontal(cx);
                let total_width = *cumulative_widths.last().unwrap();
                self.scroll_pos = self.scroll_pos.clamp(0., (total_width - viewport.size.x).max(0.));
                // Draw visible items
                let mut i = 0;
                while i < self.items.len() && cumulative_widths[i] < self.scroll_pos {
                    i += 1;
                }
                i = i.saturating_sub(1);

                let mut x = cumulative_widths[i];

                while i < self.items.len() && x < self.scroll_pos + viewport.size.x {
                    let item = &self.items[i];
                    let item_width = item_widths[i];
                    let item_x = x - self.scroll_pos;

                    if item_x + item_width > 0.0 && item_x < viewport.size.x {
                        let item_walk = item.walk(cx);
                        cx.begin_turtle(Walk {
                            abs_pos: Some(dvec2(viewport.pos.x + item_x, viewport.pos.y)),
                            margin: Default::default(),
                            width: Size::Fixed(item_width),
                            height: item_walk.height,
                        }, Layout::default());
                        item.draw_all(cx, scope);
                        cx.end_turtle();
                    }
                    x += item_width;
                    i += 1;
                }

                let bar_height = max_item_height + GAP_BETWEEN_ITEM_AND_BAR;
                (
                    ScrollAxis::Horizontal,
                    Rect {
                        pos: dvec2(viewport.pos.x, viewport.pos.y + viewport.size.y - bar_height),
                        size: dvec2(viewport.size.x, bar_height),
                    },
                    DVec2 {
                        x: total_width,
                        y: 0.,
                    },
                )
            }
            Orientation::Vertical => {
                let (item_heights, cumulative_heights, max_item_width) = self.compute_the_values_for_vertical(cx);
                let total_height = *cumulative_heights.last().unwrap();
                self.scroll_pos = self.scroll_pos.clamp(0., (total_height - viewport.size.y).max(0.));
                // Draw visible items
                let mut i = 0;
                while i < self.items.len() && cumulative_heights[i] < self.scroll_pos {
                    i += 1;
                }
                i = i.saturating_sub(1);

                let mut y = cumulative_heights[i];

                while i < self.items.len() && y < self.scroll_pos + viewport.size.y {
                    let item = &self.items[i];
                    let item_height = item_heights[i];
                    let item_y = y - self.scroll_pos;

                    if item_y + item_height > 0.0 && item_y < viewport.size.y {
                        let item_walk = item.walk(cx);
                        cx.begin_turtle(Walk {
                            abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y + item_y)),
                            margin: Default::default(),
                            width: item_walk.width,
                            height: Size::Fixed(item_height),
                        }, Layout::default());
                        item.draw_all(cx, scope);
                        cx.end_turtle();
                    }
                    y += item_height;
                    i += 1;
                }

                let bar_width = max_item_width + GAP_BETWEEN_ITEM_AND_BAR;
                (
                    ScrollAxis::Vertical,
                    Rect {
                        pos: dvec2(viewport.pos.x + viewport.size.x - bar_width, viewport.pos.y),
                        size: dvec2(bar_width, viewport.size.y),
                    },
                    DVec2 {
                        x: 0.,
                        y: total_height,
                    },
                )
            }
        };
        self.scroll_bar.set_scroll_pos_no_action(cx, self.scroll_pos);
        self.scroll_bar.draw_scroll_bar(cx, scroll_axis, scrollbar_view_rect, scrollbar_view_total);

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl List {
    pub fn compute_the_values_for_vertical(&self, cx: &mut Cx) -> (Vec<f64>, Vec<f64>, f64) {
        let mut item_heights = vec![0.];
        let mut cumulative_heights = vec![0.];
        let mut max_item_width = 0.;

        self.items.iter().for_each(|wr|{
            let walk = wr.walk(cx);
            if let Size::Fixed(height) = walk.height {
                item_heights.push(height);
                cumulative_heights.push(cumulative_heights.last().unwrap() + height);
            }
            if let Size::Fixed(width) = walk.width {
                max_item_width = max_item_width.max(width);
            }
        });
        (item_heights, cumulative_heights, max_item_width)
    }

    pub fn compute_the_values_for_horizontal(&self, cx: &mut Cx) -> (Vec<f64>, Vec<f64>, f64) {
        let mut item_widths = vec![0.];
        let mut cumulative_widths = vec![0.];
        let mut max_item_height = 0.;

        self.items.iter().for_each(|wr|{
            let walk = wr.walk(cx);
            if let Size::Fixed(width) = walk.width {
                item_widths.push(width);
                cumulative_widths.push(cumulative_widths.last().unwrap() + width);
            }
            if let Size::Fixed(height) = walk.height {
                max_item_height = max_item_height.max(height);
            }
        });
        (item_widths, cumulative_widths, max_item_height)
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.scroll_pos = 0.;
    }

    pub fn add(&mut self, widget: WidgetRef) {
        self.items.push(widget);
    }
}

impl ListRef {
    pub fn clear(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear();
        }
    }

    pub fn add(&self, widget: WidgetRef) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.add(widget);
        }
    }
}
