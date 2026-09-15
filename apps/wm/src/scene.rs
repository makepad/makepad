//! WM-owned framebuffer transition. Detaching a cached render target freezes
//! pixels without readback or a second app/widget tree.
use makepad_widgets::*;
script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.WmSceneBase = #(WmScene::register_widget(vm))
    mod.widgets.WmScene = set_type_default() do mod.widgets.WmSceneBase {
        width: Fill height: Fill flow: Overlay
        texture_caching: true
        draw_bg +: {
            image: texture_2d(float)
            pixel: fn() { return self.image.sample(self.pos) }
        }
        draw_old +: {
            image: texture_2d(float)
            opacity: uniform(0.0)
            pixel: fn() { return self.image.sample(self.pos) * self.opacity }
        }
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct WmScene {
    #[deref]
    view: View,
    #[live]
    draw_old: DrawQuad,
    #[rust]
    frozen: Vec<(ViewTextureSnapshot, f32)>,
    #[rust]
    progress: f64,
    #[rust]
    next: NextFrame,
    #[rust]
    last: f64,
}
impl WmScene {
    pub fn cut(&mut self, cx: &mut Cx) {
        self.frozen.clear();
        self.progress = 1.0;
        self.last = 0.0;
        self.view.redraw(cx);
    }
    pub fn transition(&mut self, cx: &mut Cx) {
        if let Some(frame) = self.view.take_texture_snapshot(cx) {
            let t = smooth(self.progress);
            for (_, weight) in &mut self.frozen {
                *weight *= 1.0 - t;
            }
            let weight = if self.frozen.is_empty() { 1.0 } else { t };
            self.frozen.push((frame, weight));
            self.frozen.retain(|(_, weight)| *weight > 0.001);
            // Keep GPU memory bounded during repeated rapid switches. The least
            // visible contribution is discarded, then the weights normalized.
            if self.frozen.len() > 6 {
                let i = self
                    .frozen
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
                    .unwrap()
                    .0;
                self.frozen.remove(i);
            }
            let sum: f32 = self.frozen.iter().map(|(_, w)| w).sum();
            for (_, weight) in &mut self.frozen {
                *weight /= sum;
            }
            self.progress = 0.0;
            self.last = 0.0;
            self.next = cx.new_next_frame();
        }
        self.view.redraw(cx);
    }
}
impl Widget for WmScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(ne) = self.next.is_event(event) {
            let dt = if self.last == 0.0 {
                0.0
            } else {
                (ne.time - self.last).min(0.05)
            };
            self.last = ne.time;
            self.progress = (self.progress + dt / 0.65).min(1.0);
            if self.progress < 1.0 {
                self.next = cx.new_next_frame();
            } else {
                self.frozen.clear();
            }
            self.view.redraw(cx);
        }
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)?;
        let t = smooth(self.progress);
        let mut accumulated = t;
        for (frame, weight) in &self.frozen {
            let contribution = weight * (1.0 - t);
            accumulated += contribution;
            self.draw_old.draw_vars.set_texture(0, frame.texture());
            self.draw_old.draw_vars.set_uniform(
                cx,
                live_id!(opacity),
                &[contribution / accumulated.max(0.00001)],
            );
            self.draw_old.draw_abs(cx, self.view.area().rect(cx));
        }
        DrawStep::done()
    }
}

fn smooth(t: f64) -> f32 {
    let t = t as f32;
    t * t * (3.0 - 2.0 * t)
}
