use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design! {
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    pub List = {{List}} {
        flow: Down
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
}

impl Widget for List {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.items.iter().for_each(|item| {
            item.handle_event(cx, event, scope);
        });

        if let Event::Scroll(e) = event {
            if self.area.rect(cx).contains(e.abs) {
                self.scroll_pos += e.scroll.y * 10.0;
                cx.redraw_all();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let viewport = cx.turtle().padded_rect();
        let viewport_height = viewport.size.y;

        if self.item_heights.len() != self.items.len() {
            self.compute_item_heights(cx);
        }

        let mut cumulative_heights = vec![0.0];
        for &height in &self.item_heights {
            cumulative_heights.push(cumulative_heights.last().unwrap() + height);
        }
        let total_height = *cumulative_heights.last().unwrap();

        self.scroll_pos = self.scroll_pos.clamp(0.0, (total_height - viewport_height).max(0.0));

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
                self.item_heights.push(50.0);
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
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.clear();
    }

    pub fn add(&self, widget: WidgetRef) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.add(widget);
    }
}
