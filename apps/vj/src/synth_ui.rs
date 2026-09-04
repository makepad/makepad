//! Compact step-grid widget shared by the Piano, Ironfish and drum editors.

use crate::synth::{StepPattern, STEPS};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawStepCell::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.VjStepGridBase = #(VjStepGrid::register_widget(vm))
    mod.widgets.VjStepGrid = set_type_default() do mod.widgets.VjStepGridBase{
        width: Fill
        height: Fill
        rows: 12
        draw_bg +: {color: #00000000}
        draw_cell +: {
            active: 0.0
            playhead: 0.0
            beat: 0.0
            dim: 0.0
            color_off: #x202731
            color_on: #xff5c39
            color_play: #xffe0a3
            color_beat: #x2b3541
            color_rim: #xffffff22
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let base = self.color_off.mix(self.color_beat, self.beat)
                let lit = self.color_on.mix(self.color_play, self.playhead)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, 2.5)
                sdf.fill(base.mix(lit, self.active))
                sdf.stroke(self.color_rim.mix(self.color_play, self.playhead * 0.75), 1.0)
                return sdf.result * (1.0 - self.dim * 0.6)
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStepCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    active: f32,
    #[live]
    playhead: f32,
    #[live]
    beat: f32,
    #[live]
    dim: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum VjStepGridAction {
    Changed,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjStepGrid {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_cell: DrawStepCell,
    #[live(12)]
    rows: usize,
    #[live]
    read_only: bool,
    #[live]
    dim: f32,
    #[rust]
    pattern: StepPattern,
    #[rust(255)]
    playhead: u8,
    #[rust]
    area: Area,
}

impl VjStepGrid {
    pub fn set_pattern(&mut self, cx: &mut Cx, pattern: StepPattern) {
        if self.pattern != pattern {
            self.pattern = pattern;
            self.area.redraw(cx);
        }
    }

    pub fn pattern(&self) -> StepPattern {
        self.pattern
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        if self.pattern != [0; STEPS] {
            self.pattern = [0; STEPS];
            self.area.redraw(cx);
        }
    }

    pub fn set_playhead(&mut self, cx: &mut Cx, playhead: Option<u8>) {
        let playhead = playhead.unwrap_or(255);
        if self.playhead != playhead {
            self.playhead = playhead;
            self.area.redraw(cx);
        }
    }

    pub fn set_dim(&mut self, cx: &mut Cx, dim: f32) {
        let dim = dim.clamp(0.0, 1.0);
        if self.dim != dim {
            self.dim = dim;
            self.area.redraw(cx);
        }
    }

    fn cell_at(&self, cx: &Cx, abs: DVec2) -> Option<(usize, usize)> {
        let rect = self.area.rect(cx);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(abs) {
            return None;
        }
        let rows = self.rows.clamp(1, 16);
        let column = (((abs.x - rect.pos.x) / rect.size.x) * STEPS as f64)
            .floor()
            .clamp(0.0, (STEPS - 1) as f64) as usize;
        let visual_row = (((abs.y - rect.pos.y) / rect.size.y) * rows as f64)
            .floor()
            .clamp(0.0, (rows - 1) as f64) as usize;
        Some((column, rows - 1 - visual_row))
    }
}

impl Widget for VjStepGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if !self.read_only {
                    if let Some((column, row)) = self.cell_at(cx, fe.abs) {
                        self.pattern[column] ^= 1u16 << row;
                        self.area.redraw(cx);
                        cx.widget_action(self.widget_uid(), VjStepGridAction::Changed);
                    }
                }
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                if !self.read_only {
                    cx.set_cursor(MouseCursor::Hand);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        // A sequencer grid is the editor surface, never a content-sized
        // adornment. Enforce that contract here as well as in script: a
        // PageFlip can pass a fitted fallback walk on its first activation,
        // which otherwise collapses the grid into the lower-right corner.
        if !walk.width.is_fixed() {
            walk.width = Size::fill();
        }
        if !walk.height.is_fixed() {
            walk.height = Size::fill();
        }
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        let rows = self.rows.clamp(1, 16);
        let cell_w = rect.size.x / STEPS as f64;
        let cell_h = rect.size.y / rows as f64;
        for visual_row in 0..rows {
            let row = rows - 1 - visual_row;
            for column in 0..STEPS {
                self.draw_cell.active = if self.pattern[column] & (1u16 << row) != 0 {
                    1.0
                } else {
                    0.0
                };
                self.draw_cell.playhead = if self.playhead as usize == column { 1.0 } else { 0.0 };
                self.draw_cell.beat = if column % 4 == 0 { 1.0 } else { 0.0 };
                self.draw_cell.dim = self.dim;
                self.draw_cell.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(rect.pos.x + column as f64 * cell_w, rect.pos.y + visual_row as f64 * cell_h),
                        size: dvec2(cell_w, cell_h),
                    },
                );
            }
        }
        DrawStep::done()
    }
}
