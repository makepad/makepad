//! A scene of a film in a panel: `PanelScene{node: "p_open"}`, its
//! picture, its name and its time in the film as a bar with the playhead;
//! a click opens its own panel. An edit's panel shows the film as a tree of
//! sections, each with its scenes.

use super::control::fit_text;
use super::host;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawSceneThumb::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        pixel: fn() {
            // The picture with the panel's rounded corners.
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 4.0)
            sdf.fill(self.image.sample(self.pos))
            return sdf.result
        }
    }

    mod.widgets.PanelSceneBase = #(PanelScene::register_widget(vm))
    /** A scene of the film (an edit's clip): its picture, name and time; a click opens its own panel. */
    mod.widgets.PanelScene = set_type_default() do mod.widgets.PanelSceneBase{
        width: Fill height: 46
        draw_bg +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(5.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneThumb {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelScene {
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_cell: DrawColor,
    #[live]
    draw_thumb: DrawSceneThumb,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    /// The scene: an edit's clip.
    #[live]
    pub node: String,
    #[live]
    pub label: String,
    #[rust]
    hover: bool,
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

impl Widget for PanelScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.hover = true;
                cx.set_cursor(MouseCursor::Hand);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = false;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && !fe.cancelled => host::open(&self.node),
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let card = host::scene(&self.node);
        // (Its paths: the clip's, so the panel lights it with its time.)
        host::placed(cx, &mut self.place, self.uid.0, rect, vec![format!("edit.{}.span", self.node)]);
        self.draw_bg.color = color(if self.hover { p.control_hover } else { p.well }, if self.hover { 1.0 } else { 0.6 });
        self.draw_bg.draw_abs(cx, rect);
        // The picture, 16:9 at the card's height.
        let th = rect.size.y - 6.0;
        let thumb = Rect { pos: dvec2(rect.pos.x + 3.0, rect.pos.y + 3.0), size: dvec2(th * 16.0 / 9.0, th) };
        match card.as_ref().and_then(|c| c.thumb.clone()) {
            Some(texture) => {
                self.draw_thumb.draw_vars.set_texture(0, &texture);
                self.draw_thumb.draw_abs(cx, thumb);
            }
            None => {
                self.draw_cell.color = color(p.panel, 1.0);
                self.draw_cell.draw_abs(cx, thumb);
            }
        }
        let x = thumb.pos.x + thumb.size.x + 8.0;
        let name = if !self.label.is_empty() { self.label.clone() } else { card.as_ref().map_or(self.node.clone(), |c| c.label.clone()) };
        self.draw_text.color = color(p.text_bright, 1.0);
        let room = (rect.pos.x + rect.size.x - 6.0 - x).max(10.0);
        let (name, _) = fit_text(&self.draw_text, cx, &name, room);
        self.draw_text.draw_abs(cx, dvec2(x, rect.pos.y + 5.0), &name);
        if let Some(c) = &card {
            self.draw_small.color = color(p.label, 1.0);
            let (when, _) = fit_text(&self.draw_small, cx, &format!("{:.1}–{:.1} s", c.start, c.end), room);
            self.draw_small.draw_abs(cx, dvec2(x, rect.pos.y + 19.0), &when);
            // Its time in the film, and the playhead.
            let bar = Rect { pos: dvec2(x, rect.pos.y + rect.size.y - 9.0), size: dvec2((rect.pos.x + rect.size.x - 8.0 - x).max(10.0), 3.0) };
            self.draw_cell.color = color(p.panel, 1.0);
            self.draw_cell.draw_abs(cx, bar);
            let at = |t: f64| bar.pos.x + bar.size.x * (t / c.film.max(1e-3)).clamp(0.0, 1.0);
            let head = host::playhead().map(|(t, ..)| t);
            let on = head.is_some_and(|t| t >= c.start && t < c.end);
            self.draw_cell.color = color(if on { p.accent } else { p.label }, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(at(c.start), bar.pos.y - 1.0), size: dvec2((at(c.end) - at(c.start)).max(2.0), 5.0) });
            if let Some(t) = head {
                self.draw_cell.color = color(p.text_bright, 0.9);
                self.draw_cell.draw_abs(cx, Rect { pos: dvec2(at(t), bar.pos.y - 3.0), size: dvec2(1.0, 9.0) });
            }
        }
        DrawStep::done()
    }
}
