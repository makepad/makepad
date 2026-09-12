//! Geometry shared by the performance dropdowns. Tall menus become multiple
//! columns on a short phone viewport; every option keeps its touch target.
use makepad_widgets::*;
use makepad_widgets::makepad_platform::event::TouchState;

pub enum ChoiceEvent {
    None,
    Hover(Option<usize>),
    Pick(usize),
    Dismiss,
}

pub fn handle_event(event: &Event, area: Area, chip: Rect, layout: ChoiceLayout) -> ChoiceEvent {
    let choose = |point| layout.pick(point).map(ChoiceEvent::Pick).unwrap_or_else(|| {
        if chip.contains(point) { ChoiceEvent::None } else { ChoiceEvent::Dismiss }
    });
    match event {
        Event::MouseMove(e) => ChoiceEvent::Hover(layout.pick(e.abs)),
        Event::MouseDown(e) => {
            if layout.panel.contains(e.abs) { e.handled.set(area); }
            choose(e.abs)
        }
        Event::MouseUp(e) => choose(e.abs),
        Event::TouchUpdate(e) => {
            for touch in &e.touches {
                match touch.state {
                    TouchState::Start => {
                        if layout.panel.contains(touch.abs) { touch.handled.set(area); }
                        return choose(touch.abs);
                    }
                    TouchState::Stop => return choose(touch.abs),
                    TouchState::Move => return ChoiceEvent::Hover(layout.pick(touch.abs)),
                    TouchState::Stable => {}
                }
            }
            ChoiceEvent::None
        }
        Event::KeyDown(e) if e.key_code == KeyCode::Escape => ChoiceEvent::Dismiss,
        Event::BackPressed { handled } => { handled.set(true); ChoiceEvent::Dismiss }
        _ => ChoiceEvent::None,
    }
}

#[derive(Clone, Copy, Default)]
pub struct ChoiceLayout {
    pub panel: Rect,
    pub row_height: f64,
    pub cell_width: f64,
    pub rows: usize,
    pub count: usize,
}

impl ChoiceLayout {
    pub fn new(chip: Rect, viewport: Vec2d, count: usize, row_height: f64, cell_width: f64) -> Self {
        let row_height = row_height.max(20.0);
        let rows = (((viewport.y - 16.0) / row_height).floor() as usize).max(1).min(count.max(1));
        let columns = count.div_ceil(rows).max(1);
        let cell_width = cell_width.min((viewport.x - 16.0).max(1.0) / columns as f64);
        let size = dvec2(cell_width * columns as f64 + 8.0, rows as f64 * row_height + 8.0);
        let below = chip.pos.y + chip.size.y + 4.0;
        let y = if below + size.y <= viewport.y - 4.0 { below } else { chip.pos.y - size.y - 4.0 };
        let pos = dvec2(
            (chip.pos.x + (chip.size.x - size.x) * 0.5).clamp(4.0, (viewport.x - size.x - 4.0).max(4.0)),
            y.clamp(4.0, (viewport.y - size.y - 4.0).max(4.0)),
        );
        Self { panel: Rect { pos, size }, row_height, cell_width, rows, count }
    }

    pub fn cell(&self, index: usize) -> Rect {
        Rect {
            pos: self.panel.pos + dvec2(4.0 + (index / self.rows.max(1)) as f64 * self.cell_width,
                4.0 + (index % self.rows.max(1)) as f64 * self.row_height),
            size: dvec2(self.cell_width, self.row_height),
        }
    }

    pub fn pick(&self, point: Vec2d) -> Option<usize> {
        (0..self.count).find(|&index| self.cell(index).contains(point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nine_options_remain_reachable_in_phone_landscape() {
        let viewport = dvec2(850.0, 300.0);
        let layout = ChoiceLayout::new(Rect { pos: dvec2(810.0, 264.0), size: dvec2(40.0, 36.0) },
            viewport, 9, 44.0, 60.0);
        assert!(layout.rows < 9);
        for index in 0..9 {
            let cell = layout.cell(index);
            assert!(cell.pos.x >= 0.0 && cell.pos.y >= 0.0);
            assert!(cell.pos.x + cell.size.x <= viewport.x);
            assert!(cell.pos.y + cell.size.y <= viewport.y);
            assert_eq!(layout.pick(cell.pos + cell.size * 0.5), Some(index));
        }
    }
}
