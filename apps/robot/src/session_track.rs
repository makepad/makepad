use crate::recording::ClearanceEvent;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.SessionTrack = set_type_default() do #(SessionTrack::register_widget(vm)){
        width: Fill height: 44
        background +: {color: #x162025}
        ink +: {color: #x83c7bd}
    }
}

#[derive(Clone, Debug, Default)]
pub enum TrackAction {
    #[default]
    None,
    Seek(u64),
    Event(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct SessionTrack {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    background: DrawColor,
    #[live]
    ink: DrawColor,
    #[rust]
    duration: u64,
    #[rust]
    cursor: u64,
    #[rust]
    events: Arc<Vec<ClearanceEvent>>,
}

const INSET: f64 = 9.;
impl SessionTrack {
    pub fn update(
        &mut self,
        cx: &mut Cx,
        duration: u64,
        cursor: u64,
        events: &Arc<Vec<ClearanceEvent>>,
    ) {
        if self.duration != duration || self.cursor != cursor || !Arc::ptr_eq(&self.events, events)
        {
            self.duration = duration;
            self.cursor = cursor;
            self.events = events.clone();
            self.background.redraw(cx);
        }
    }
    fn bar(&mut self, cx: &mut Cx2d, x: f64, y: f64, w: f64, h: f64, color: Vec4f) {
        self.ink.color = color;
        self.ink.draw_abs(
            cx,
            Rect {
                pos: dvec2(x, y),
                size: dvec2(w, h),
            },
        );
    }
    fn pointer(&self, rect: Rect, pos: Vec2d, select_event: bool) -> TrackAction {
        let width = (rect.size.x - 2. * INSET).max(1.);
        let time = (((pos.x - rect.pos.x - INSET) / width).clamp(0., 1.) * self.duration as f64)
            .round() as u64;
        if select_event && pos.y - rect.pos.y <= 27. {
            // A short excursion must still be selectable at a wide time scale.
            let tolerance = (3. * self.duration as f64 / width).ceil() as u64;
            if let Some((index, _)) = self
                .events
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    (
                        i,
                        e.time_us
                            .saturating_sub(time)
                            .max(time.saturating_sub(e.end_us)),
                    )
                })
                .filter(|(_, d)| *d <= tolerance)
                .min_by_key(|&(_, d)| d)
            {
                return TrackAction::Event(index);
            }
        }
        TrackAction::Seek(time)
    }
}
impl Widget for SessionTrack {
    fn draw_walk(&mut self, cx: &mut Cx2d, _: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.walk_turtle(walk);
        self.background.draw_abs(cx, r);
        let x = r.pos.x + INSET;
        let y = r.pos.y;
        let w = (r.size.x - 2. * INSET).max(1.);
        let scale = w / self.duration.max(1) as f64;
        self.bar(cx, x, y + 13., w, 10., vec4(0.19, 0.32, 0.34, 1.));
        let steps = (w / 48.).floor().clamp(2., 24.) as usize;
        for i in 0..=steps {
            self.bar(
                cx,
                x + w * (i as f64 / steps as f64),
                y + 26.,
                1.,
                if i % 2 == 0 { 7. } else { 4. },
                vec4(0.36, 0.47, 0.48, 1.),
            );
        }
        // Coalesce overlapping on-screen bands; the picker keeps each original interval.
        let events = self.events.clone();
        let color = vec4(0.99, 0.57, 0.32, 1.);
        let mut band: Option<(f64, f64)> = None;
        for e in events.iter() {
            let a = (e.time_us as f64 * scale).min((w - 3.).max(0.));
            let b = (e.end_us as f64 * scale).max(a + 3.).min(w);
            if let Some((first, last)) = band {
                if a <= last + 0.5 {
                    band = Some((first, last.max(b)));
                    continue;
                }
                self.bar(cx, x + first, y + 10., last - first, 16., color);
            }
            band = Some((a, b));
        }
        if let Some((a, b)) = band {
            self.bar(cx, x + a, y + 10., b - a, 16., color);
        }
        let head = x + self.cursor.min(self.duration) as f64 * scale;
        self.bar(cx, head - 1., y + 5., 2., 30., vec4(0.94, 1., 0.96, 1.));
        self.bar(cx, head - 3., y + 4., 6., 3., vec4(0.94, 1., 0.96, 1.));
        DrawStep::done()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _: &mut Scope) {
        let area = self.background.area();
        let rect = area.rect(cx);
        match event.hits(cx, area) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(e) => cx.widget_action(self.uid, self.pointer(rect, e.abs, true)),
            Hit::FingerMove(e) => cx.widget_action(self.uid, self.pointer(rect, e.abs, false)),
            _ => {}
        }
    }
}
