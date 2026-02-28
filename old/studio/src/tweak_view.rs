use crate::makepad_platform::studio::TweakHitsResponse;
use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.TweakView = #(TweakView::script_component(vm))
}

#[derive(Script, ScriptHook)]
pub struct TweakView {
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_vector: DrawVector,
    #[rust]
    hit_rect: Option<Rect>,
    #[rust]
    widget_uids: Vec<u64>,
}

impl TweakView {
    pub fn clear(&mut self, cx: &mut Cx) {
        if self.hit_rect.is_some() {
            self.hit_rect = None;
            self.widget_uids.clear();
            cx.redraw_all();
        }
    }

    pub fn set_hits(&mut self, cx: &mut Cx, hits: &TweakHitsResponse) {
        let dpi = hits.dpi_factor.max(1.0);
        self.widget_uids = hits.widget_uids.clone();
        if hits.width <= 0.0 || hits.height <= 0.0 {
            self.hit_rect = None;
            log!("TweakRay: no hit");
        } else {
            let rect = Rect {
                pos: dvec2(hits.left / dpi, hits.top / dpi),
                size: dvec2(hits.width / dpi, hits.height / dpi),
            };
            self.hit_rect = Some(rect);
            log!(
                "TweakRay hit=({:.1},{:.1} {:.1}x{:.1}) uids={:?}",
                rect.pos.x,
                rect.pos.y,
                rect.size.x,
                rect.size.y,
                self.widget_uids
            );
        }
        cx.redraw_all();
    }

    pub fn draw_overlay(&mut self, cx: &mut Cx2d, run_view_rect: Rect) {
        let Some(hit_rect) = self.hit_rect else {
            return;
        };
        let draw_rect = Rect {
            pos: run_view_rect.pos + hit_rect.pos,
            size: hit_rect.size,
        };

        self.draw_vector.begin();
        self.draw_vector.clear();
        self.draw_vector.set_color(0.17, 1.0, 0.52, 0.95);
        self.draw_vector.rect(
            draw_rect.pos.x as f32,
            draw_rect.pos.y as f32,
            draw_rect.size.x as f32,
            draw_rect.size.y as f32,
        );
        self.draw_vector.fill();
        self.draw_vector.set_color(0.05, 0.30, 0.12, 1.0);
        self.draw_vector.stroke(2.0);
        self.draw_vector.end(cx);
    }
}
