use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    scroll_bar::*,
};

live_design! {
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use crate::scroll_bar::ScrollBar;

    pub List = {{List}} {
        scroll_bar: <ScrollBar> {
            bar_size: 15.0

                draw_bg: {
                    instance drag: 0.0
                    instance hover: 0.0

                    uniform size: 6.0
                    uniform border_size: 1.0
                    uniform border_radius: 1.5

                    uniform color: #1F1F1F80
                    uniform color_hover: #09090980
                    uniform color_drag: #00000080

                    uniform border_color: #00000080
                    uniform border_color_hover: #00000080
                    uniform border_color_drag: #00000080

                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        if self.is_vertical > 0.5 {
                            sdf.box(
                                1.,
                                self.rect_size.y * self.norm_scroll,
                                self.size,
                                self.rect_size.y * self.norm_handle,
                                self.border_radius
                            );
                        }
                        else {
                            sdf.box(
                                self.rect_size.x * self.norm_scroll,
                                1.,
                                self.rect_size.x * self.norm_handle,
                                self.size,
                                self.border_radius
                            );
                        }

                        sdf.fill_keep(mix(
                            self.color,
                            mix(
                                self.color_hover,
                                self.color_drag,
                                self.drag
                            ),
                            self.hover
                        ));

                        sdf.stroke(mix(
                            self.border_color,
                            mix(
                                self.border_color_hover,
                                self.border_color_drag,
                                self.drag
                            ),
                            self.hover
                        ), self.border_size);

                        return sdf.result
                    }
                }
        }
    }
}

#[derive(Live, Widget, LiveHook)]
struct List {
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[rust] area: Area,
    #[rust] items: Vec<WidgetRef>,
    #[rust(0.0)] scroll_pos: f64,
    #[rust] item_heights: Vec<f64>,
    #[live] scroll_bar: ScrollBar,
}

impl Widget for List {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Forward events to child items
        self.items.iter().for_each(|item| {
            item.handle_event(cx, event, scope);
        });

        // Handle scroll bar events
        self.scroll_bar.handle_event_with(cx, event, &mut |cx, action| {
            if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                self.scroll_pos = scroll_pos;
                cx.redraw_all();
            }
        });

        // Handle mouse wheel scrolling
        if let Event::Scroll(e) = event {
            if self.area.rect(cx).contains(e.abs) {
                self.scroll_pos += e.scroll.y * 10.0;
                // Clamping is handled in draw_walk, update scroll bar here
                self.scroll_bar.set_scroll_pos_no_action(cx, self.scroll_pos);
                cx.redraw_all();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let viewport = cx.turtle().padded_rect();
        let viewport_height = viewport.size.y;

        // Compute item heights if necessary
        if self.item_heights.len() != self.items.len() {
            self.compute_item_heights(cx);
        }

        // Calculate cumulative heights and total height
        let mut cumulative_heights = vec![0.0];
        for &height in &self.item_heights {
            cumulative_heights.push(cumulative_heights.last().unwrap() + height);
        }
        let total_height = *cumulative_heights.last().unwrap();

        // Clamp scroll position
        self.scroll_pos = self.scroll_pos.clamp(0.0, (total_height - viewport_height).max(0.0));

        // Draw visible items
        let mut i = 0;
        while i < self.items.len() && cumulative_heights[i] < self.scroll_pos {
            i += 1;
        }
        i = i.saturating_sub(1);

        let mut y = cumulative_heights[i];
        while i < self.items.len() && y < self.scroll_pos + viewport_height {
            let item = &self.items[i];
            let item_height = self.item_heights[i];
            let item_y = y - self.scroll_pos;

            if item_y + item_height > 0.0 && item_y < viewport_height {
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

        // Draw scroll bar
        let scroll_bar_width = 15.0;
        let scroll_bar_rect = Rect {
            pos: dvec2(viewport.pos.x + viewport.size.x - scroll_bar_width, viewport.pos.y),
            size: dvec2(scroll_bar_width, viewport.size.y),
        };
        self.scroll_bar.set_scroll_pos_no_action(cx, self.scroll_pos);
        self.scroll_bar.draw_scroll_bar(cx, ScrollAxis::Vertical, scroll_bar_rect, dvec2(0.0, total_height));

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl List {
    pub fn compute_item_heights(&mut self, cx: &mut Cx) {
        self.item_heights.clear();
        for item in &self.items {
            let item_walk = item.walk(cx);
            if let Size::Fixed(height) = item_walk.height {
                self.item_heights.push(height);
            } else {
                self.item_heights.push(50.0); // Default height
            }
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.item_heights.clear();
        self.scroll_pos = 0.0;
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
