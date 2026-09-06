use crate::disk::Snapshot;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioDiskGraph = #(DiskGraph::register_widget(vm)){
        width: 120 height: 26
        draw_bar +: {color: theme.color_text}
        background: theme.color_bg_app
    }
}

/// Volume gauge above a real sampled usage sparkline. No invented history:
/// the lower plot stays empty until the second filesystem sample arrives.
#[derive(Script, ScriptHook, Widget)]
pub struct DiskGraph {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_bar: DrawColor,
    #[live] background: Vec4f,
    #[rust] snapshot: Option<Arc<Snapshot>>,
}

impl DiskGraph {
    pub fn update(&mut self, cx: &mut Cx, snapshot: Arc<Snapshot>) {
        self.snapshot = Some(snapshot);
        self.draw_bar.redraw(cx);
    }
}
impl Widget for DiskGraph {
    fn handle_event(&mut self, _: &mut Cx, _: &Event, _: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let ink = self.draw_bar.color;
        self.draw_bar.color = self.background;
        self.draw_bar.draw_abs(cx, rect);
        self.draw_bar.color = ink;
        let Some(s) = &self.snapshot else { return DrawStep::done(); };
        if let Some(v) = &s.volume {
            let ratio = (v.used as f64 / v.total.max(1) as f64).clamp(0.0, 1.0);
            self.draw_bar.draw_abs(cx, Rect { pos: rect.pos, size: dvec2(rect.size.x*ratio, 5.0) });
        }
        if s.history.len() >= 2 {
            let min = s.history.iter().map(|(_, n)| *n).min().unwrap() as f64;
            let max = s.history.iter().map(|(_, n)| *n).max().unwrap() as f64;
            let span = (max-min).max(1.0);
            let width = rect.size.x / s.history.len() as f64;
            for (i, (_, n)) in s.history.iter().enumerate() {
                let h = 2.0 + (*n as f64-min)/span * (rect.size.y-10.0).max(0.0);
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(rect.pos.x + i as f64*width, rect.pos.y+rect.size.y-h), size: dvec2((width-1.0).max(1.0), h) });
            }
        }
        DrawStep::done()
    }
}
