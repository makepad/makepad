//! A segmented control: a well, the active segment in the one picked look
//! (`Palette::pick`), hover lifting the others. The side panel's tabs (AI,
//! Library, Panel) and the library's lanes.

use makepad_widgets::badge::advance;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    set_type_default() do #(DrawSeg::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1., self.rect_size.y - 1., self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, 1.)
            return sdf.result
        }
    }

    mod.widgets.PaneTabsBase = #(PaneTabs::register_widget(vm))
    mod.widgets.PaneTabs = set_type_default() do mod.widgets.PaneTabsBase{
        width: Fit height: 24
        draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 8.5}}
        draw_on +: {color: stage.text_hi text_style: theme.font_bold{font_size: 8.5}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSeg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub radius: f32,
}

#[derive(Clone, Debug, Default)]
pub enum TabsAction {
    #[default]
    None,
    Selected(usize),
}

/// A segment's padding either side of its label.
const SEG_PAD: f64 = 11.0;

#[derive(Script, ScriptHook, Widget)]
pub struct PaneTabs {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawSeg,
    #[live]
    draw_box: DrawSeg,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_on: DrawText,
    #[live]
    labels: Vec<String>,
    #[rust]
    selected: usize,
    #[rust]
    hover: Option<usize>,
    /// The segments, relative to the control's left edge.
    #[rust]
    segs: Vec<Rect>,
    #[rust]
    area: Area,
}

impl PaneTabs {
    pub fn set_selected(&mut self, cx: &mut Cx, index: usize) {
        if self.selected != index {
            self.selected = index;
            self.redraw(cx);
        }
    }

    fn at(&self, dx: f64) -> Option<usize> {
        self.segs.iter().position(|r| dx >= r.pos.x && dx < r.pos.x + r.size.x)
    }
}

impl Widget for PaneTabs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Where it is now: a Fill beside it moves it after it was walked.
        let left = self.area.rect(cx).pos.x;
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(MouseCursor::Hand);
                let hover = self.at(fe.abs.x - left);
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && !fe.cancelled => {
                if let Some(index) = self.at(fe.abs.x - left) {
                    self.selected = index;
                    self.redraw(cx);
                    cx.widget_action(self.uid, TabsAction::Selected(index));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let labels = self.labels.clone();
        let widths: Vec<f64> = labels.iter().enumerate().map(|(i, label)| {
            let text = if i == self.selected { &self.draw_on } else { &self.draw_text };
            SEG_PAD * 2.0 + advance(text, cx, label)
        }).collect();
        let total: f64 = widths.iter().sum::<f64>() + 4.0;
        let rect = cx.walk_turtle(Walk { width: Size::Fixed(total), ..walk });
        self.draw_bg.radius = 4.0;
        self.draw_bg.fill = Vec4f::from_u32(p.well);
        self.draw_bg.edge = Vec4f::from_u32(p.line_faint);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        self.segs.clear();
        let mut x = rect.pos.x + 2.0;
        for (i, label) in labels.iter().enumerate() {
            let seg = Rect { pos: dvec2(x, rect.pos.y + 2.0), size: dvec2(widths[i], rect.size.y - 4.0) };
            x += widths[i];
            self.segs.push(Rect { pos: dvec2(seg.pos.x - rect.pos.x, 0.0), size: seg.size });
            let active = i == self.selected;
            if active || self.hover == Some(i) {
                self.draw_box.radius = 3.0;
                let look = p.pick(active, true, p.well);
                self.draw_box.fill = Vec4f::from_u32(look.fill);
                self.draw_box.edge = Vec4f::from_u32(if active { look.edge } else { look.fill });
                self.draw_box.draw_abs(cx, seg);
            }
            let text = if active { &mut self.draw_on } else { &mut self.draw_text };
            text.color = Vec4f::from_u32(if active { p.text_hi } else if self.hover == Some(i) { p.text } else { p.secondary });
            text.draw_abs(cx, dvec2(seg.pos.x + SEG_PAD, seg.pos.y + (seg.size.y - 11.0) * 0.5), label);
        }
        DrawStep::done()
    }
}
