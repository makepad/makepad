//! Lane D. The Z pie menu — a real radial, not a list pretending to be one.
//!
//! Wedges are `Sdf2d` (§3.4 allows procedural marks: gizmo balls, grid,
//! outline, pie, drag arrows). Slots follow Fab's compass order — the
//! first item sits West, then East, South, North, then the diagonals — so
//! muscle memory transfers. The nearest slot to the pointer is hot from the
//! moment the pie appears; releasing or clicking picks it, Escape cancels.
//!
//! Like `FabMenuLayer` this lives at the end of the shell's overlay stack
//! and is raised by an action, so any control can throw one without owning it.

use crate::ui::popover::{FabUiAction, PieItem};
use makepad_widgets::*;

/// Fab's slot order, degrees, 0° = +x (right), counter-clockwise.
const SLOT_DEG: [f32; 8] = [180.0, 0.0, 270.0, 90.0, 225.0, 315.0, 135.0, 45.0];

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    set_type_default() do #(DrawPieWedge::script_shader(vm)){
        ..mod.draw.DrawQuad

        a0: 0.0
        a1: 0.0
        hot: 0.0
        press: 0.0
        inner: 30.0
        outer: 96.0
        t: 1.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            let p = self.pos * self.rect_size - c
            let r = length(p)
            let inner = self.inner
            let outer = self.inner + (self.outer - self.inner) * self.t
            if r < inner || r > outer {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            // Screen y grows downward; negate so 90° reads as up.
            let mut a = atan2(-p.y, p.x)
            if a < 0.0 {
                a = a + 2.0 * PI
            }
            let a0 = self.a0
            let a1 = self.a1
            let mut inside = 0.0
            if a1 > a0 {
                if a >= a0 && a <= a1 { inside = 1.0 }
            } else {
                if a >= a0 || a <= a1 { inside = 1.0 }
            }
            if inside < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let edge = min(r - inner, outer - r)
            let aa = clamp(edge, 0.0, 1.5) / 1.5
            let col = fab.color_pie_wedge
                .mix(fab.color_pie_wedge_hot, self.hot)
                .mix(fab.color_accent_dim, self.press)
            return vec4(col.xyz, col.w * aa * self.t)
        }
    }

    mod.widgets.FabPieLayerBase = #(FabPieLayer::register_widget(vm))
    mod.widgets.FabPieLayer = set_type_default() do mod.widgets.FabPieLayerBase{
        width: Fill
        height: Fill
        draw_wedge +: { }
        draw_hub +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                sdf.circle(c.x, c.y, fab.pie_inner * 0.55)
                sdf.fill_keep(fab.color_pie_bg)
                sdf.stroke(fab.color_border_light, 1.0)
                return sdf.result
            }
        }
        draw_label +: {
            color: fab.color_text_active
            ink_centered: true
            text_style: theme.font_bold{
                font_size: fab.font_size_ui
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPieWedge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    a0: f32,
    #[live]
    a1: f32,
    #[live]
    hot: f32,
    #[live]
    press: f32,
    #[live]
    inner: f32,
    #[live]
    outer: f32,
    #[live]
    t: f32,
}

struct OpenPie {
    owner: LiveId,
    items: Vec<PieItem>,
    at: Vec2d,
    hot: usize,
    /// Wedge held under the pointer (press). `usize::MAX` = none.
    press: usize,
    /// Keyboard focus is tracking `hot`; this is true after an arrow key so
    /// the hub can show the focus ring.
    key_focus: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabPieLayer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_list: DrawList2d,
    #[redraw]
    #[live]
    draw_wedge: DrawPieWedge,
    #[live]
    draw_hub: DrawQuad,
    #[live]
    draw_label: DrawText,
    #[walk]
    walk: Walk,
    #[rust]
    area: Area,
    #[rust]
    open: Option<OpenPie>,
    #[rust]
    opened_at: f64,
    #[rust]
    next_frame: NextFrame,
}

impl FabPieLayer {
    /// Angular slot centre for item `i` of `n`, radians, 0 = +x, CCW.
    fn slot_angle(i: usize) -> f32 {
        SLOT_DEG[i.min(7)].to_radians()
    }

    fn pick(&self, at: Vec2d, p: Vec2d, n: usize) -> usize {
        let d = p - at;
        let r = (d.x * d.x + d.y * d.y).sqrt();
        if r < 18.0 {
            return usize::MAX;
        }
        let a = (-d.y).atan2(d.x) as f32;
        let mut best = 0usize;
        let mut best_d = f32::MAX;
        for i in 0..n.min(8) {
            let s = Self::slot_angle(i);
            let mut diff = (a - s).abs();
            while diff > std::f32::consts::PI {
                diff = (std::f32::consts::TAU - diff).abs();
            }
            if diff < best_d {
                best_d = diff;
                best = i;
            }
        }
        best
    }

    pub fn close(&mut self, cx: &mut Cx) -> Option<LiveId> {
        let owner = self.open.take().map(|m| m.owner);
        if owner.is_some() {
            self.draw_list.redraw(cx);
            self.area.redraw(cx);
        }
        owner
    }
}

impl Widget for FabPieLayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let window = cx.turtle().rect();
        cx.end_turtle_with_area(&mut self.area);

        let Some(pie) = self.open.as_ref() else {
            return DrawStep::done();
        };
        let items: Vec<PieItem> = pie.items.clone();
        let at = pie.at;
        let hot = pie.hot;
        let n = items.len().min(8);
        if n == 0 {
            return DrawStep::done();
        }
        let t = (((cx.seconds_since_app_start() - self.opened_at) / 0.15).min(1.0)) as f32;
        let outer = 96.0f32;
        let inner = 30.0f32;
        let press = pie.press;
        let key_focus = pie.key_focus;
        let box_size = (outer as f64) * 2.0 + 8.0;

        self.draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());
        let origin = dvec2(0.0, 0.0);
        let rect = Rect {
            pos: origin,
            size: dvec2(box_size, box_size),
        };
        // The wedges: each spans half-way to its neighbours.
        let mut angles: Vec<f32> = (0..n).map(Self::slot_angle).collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|a, b| angles[*a].partial_cmp(&angles[*b]).unwrap());
        for oi in 0..n {
            let i = order[oi];
            let prev = order[(oi + n - 1) % n];
            let next = order[(oi + 1) % n];
            let a = angles[i];
            let mut half_prev = (a - angles[prev]).rem_euclid(std::f32::consts::TAU) * 0.5;
            let mut half_next = (angles[next] - a).rem_euclid(std::f32::consts::TAU) * 0.5;
            if n == 1 {
                half_prev = std::f32::consts::PI;
                half_next = std::f32::consts::PI;
            }
            let a0 = (a - half_prev).rem_euclid(std::f32::consts::TAU);
            let a1 = (a + half_next).rem_euclid(std::f32::consts::TAU);
            self.draw_wedge.a0 = a0;
            self.draw_wedge.a1 = a1;
            self.draw_wedge.hot = if hot == i { 1.0 } else { 0.0 };
            self.draw_wedge.press = if press == i { 1.0 } else { 0.0 };
            self.draw_wedge.inner = inner;
            self.draw_wedge.outer = outer;
            self.draw_wedge.t = t;
            self.draw_wedge.draw_abs(cx, rect);
        }
        angles.clear();
        let _ = key_focus;
        self.draw_hub.draw_abs(cx, rect);
        // Labels sit on the wedge mid-radius, in a box whose centre is the
        // wedge centre — `ink_centered` does the vertical work, no nudge.
        let mid = ((inner + outer) * 0.5) as f64;
        for (i, item) in items.iter().enumerate().take(8) {
            let a = Self::slot_angle(i) as f64;
            let cx_pos = box_size * 0.5 + a.cos() * mid;
            let cy_pos = box_size * 0.5 - a.sin() * mid;
            let w = (item.label.chars().count() as f64 * 5.9).max(12.0);
            self.draw_label.color = if item.active {
                vec4(1.0, 1.0, 1.0, 1.0)
            } else if hot == i {
                vec4(1.0, 1.0, 1.0, 1.0)
            } else {
                vec4(0.90, 0.90, 0.90, 1.0)
            };
            self.draw_label.draw_walk(
                cx,
                Walk::abs_rect(Rect {
                    pos: dvec2(cx_pos - w * 0.5, cy_pos - 8.0),
                    size: dvec2(w, 16.0),
                }),
                Align { x: 0.5, y: 0.5 },
                &item.label,
            );
        }
        let shift = dvec2(at.x - box_size * 0.5, at.y - box_size * 0.5) - window.pos;
        cx.end_pass_sized_turtle_with_shift(self.area, shift);
        self.draw_list.end(cx);
        if t < 1.0 {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.draw_list.redraw(cx);
            self.area.redraw(cx);
        }
        if let Event::Actions(actions) = event {
            let mut request = None;
            for a in actions.iter() {
                if let Some(FabUiAction::OpenPie { owner, at, items }) =
                    a.downcast_ref::<FabUiAction>()
                {
                    request = Some((*owner, *at, items.clone()));
                }
            }
            if let Some((owner, at, items)) = request {
                let hot = items.iter().position(|i| i.active).unwrap_or(usize::MAX);
                self.open = Some(OpenPie {
                    owner,
                    items,
                    at,
                    hot,
                    press: usize::MAX,
                    key_focus: false,
                });
                self.opened_at = cx.seconds_since_app_start();
                self.draw_list.redraw(cx);
                self.area.redraw(cx);
            }
        }
        if self.open.is_none() {
            return;
        }
        match event {
            Event::MouseMove(e) => {
                let (at, n) = {
                    let p = self.open.as_ref().unwrap();
                    (p.at, p.items.len())
                };
                let hot = self.pick(at, e.abs, n);
                if let Some(p) = self.open.as_mut() {
                    if p.hot != hot {
                        p.hot = hot;
                        p.key_focus = false;
                        self.draw_list.redraw(cx);
                        self.area.redraw(cx);
                    }
                }
            }
            Event::MouseDown(e) => {
                let (at, n) = {
                    let p = self.open.as_ref().unwrap();
                    (p.at, p.items.len())
                };
                let hot = self.pick(at, e.abs, n);
                if let Some(p) = self.open.as_mut() {
                    p.hot = hot;
                    p.press = hot;
                    self.draw_list.redraw(cx);
                    self.area.redraw(cx);
                }
            }
            Event::MouseUp(_) => {
                let (press, picked, owner) = {
                    let p = self.open.as_ref().unwrap();
                    let press = p.press;
                    let picked = p.items.get(press).map(|i| i.id);
                    (press, picked, p.owner)
                };
                self.close(cx);
                if press != usize::MAX {
                    if let Some(id) = picked {
                        cx.action(FabUiAction::PiePicked { owner, id });
                    }
                }
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                self.close(cx);
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::ArrowLeft
                    || ke.key_code == KeyCode::ArrowRight
                    || ke.key_code == KeyCode::ArrowUp
                    || ke.key_code == KeyCode::ArrowDown =>
            {
                if let Some(p) = self.open.as_mut() {
                    let n = p.items.len().max(1);
                    let dir = if matches!(ke.key_code, KeyCode::ArrowRight | KeyCode::ArrowUp) {
                        1isize
                    } else {
                        -1
                    };
                    let cur = if p.hot >= n { 0 } else { p.hot };
                    p.hot = ((cur as isize + dir).rem_euclid(n as isize)) as usize;
                    p.key_focus = true;
                    self.draw_list.redraw(cx);
                    self.area.redraw(cx);
                }
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::ReturnKey || ke.key_code == KeyCode::NumpadEnter =>
            {
                let (hot, picked, owner) = {
                    let p = self.open.as_ref().unwrap();
                    (p.hot, p.items.get(p.hot).map(|i| i.id), p.owner)
                };
                self.close(cx);
                if hot != usize::MAX {
                    if let Some(id) = picked {
                        cx.action(FabUiAction::PiePicked { owner, id });
                    }
                }
            }
            _ => {}
        }
    }
}

impl FabPieLayerRef {
    pub fn is_open(&self) -> bool {
        self.borrow().map_or(false, |i| i.open.is_some())
    }
}
