//! Windows snap geometry and the compositor's layout preview.
use crate::shell::{
    alpha, rgb,
    ui::{rect, HAlign, ShellDraw},
};
use crate::{
    desk::WmState,
    desktop::DrawDesktopChrome,
    layout::{ClientId, LRect},
};
use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Maximize,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftWide,
    RightNarrow,
    LeftThird,
    MiddleThird,
    RightThird,
}
impl Zone {
    fn fractions(self) -> (f64, f64, f64, f64) {
        match self {
            Self::Maximize => (0., 0., 1., 1.),
            Self::Left => (0., 0., 0.5, 1.),
            Self::Right => (0.5, 0., 0.5, 1.),
            Self::Top => (0., 0., 1., 0.5),
            Self::Bottom => (0., 0.5, 1., 0.5),
            Self::TopLeft => (0., 0., 0.5, 0.5),
            Self::TopRight => (0.5, 0., 0.5, 0.5),
            Self::BottomLeft => (0., 0.5, 0.5, 0.5),
            Self::BottomRight => (0.5, 0.5, 0.5, 0.5),
            Self::LeftWide => (0., 0., 2. / 3., 1.),
            Self::RightNarrow => (2. / 3., 0., 1. / 3., 1.),
            Self::LeftThird => (0., 0., 1. / 3., 1.),
            Self::MiddleThird => (1. / 3., 0., 1. / 3., 1.),
            Self::RightThird => (2. / 3., 0., 1. / 3., 1.),
        }
    }
    pub fn bounds(self, area: LRect) -> LRect {
        let (x, y, w, h) = self.fractions();
        LRect::new(
            area.x + x * area.w,
            area.y + y * area.h,
            w * area.w,
            h * area.h,
        )
    }
}

pub fn edge_zone(p: Vec2d, area: LRect) -> Option<Zone> {
    let left = p.x <= area.x + 16.;
    let right = p.x >= area.x + area.w - 16.;
    let top = p.y <= area.y + 54.;
    let bottom = p.y >= area.y + area.h - 54.;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(Zone::TopLeft),
        (_, true, true, _) => Some(Zone::TopRight),
        (true, _, _, true) => Some(Zone::BottomLeft),
        (_, true, _, true) => Some(Zone::BottomRight),
        (true, _, _, _) => Some(Zone::Left),
        (_, true, _, _) => Some(Zone::Right),
        _ if p.y <= area.y + 10. => Some(Zone::Maximize),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct Picker {
    pub client: ClientId,
    pub bounds: Rect,
    pub drag: bool,
    pub cells: Vec<(Rect, Zone)>,
}
impl Picker {
    pub fn new(client: ClientId, area: LRect, anchor: Option<Rect>) -> Self {
        use Zone::*;
        let mut layouts = vec![
            vec![Left, Right],
            vec![LeftWide, RightNarrow],
            vec![Left, TopRight, BottomRight],
            vec![TopLeft, TopRight, BottomLeft, BottomRight],
        ];
        if area.w >= 1000. {
            layouts.push(vec![LeftThird, MiddleThird, RightThird]);
        }
        if area.h > area.w {
            layouts.push(vec![Top, Bottom]);
        }
        let columns = if area.w < 450. { 2 } else { 3 };
        let width = (columns as f64 * 108. + 32.).min(area.w - 12.);
        let rows = layouts.len().div_ceil(columns);
        let height = 48. + rows as f64 * 72.;
        let x = anchor
            .map(|r| r.pos.x + r.size.x - width)
            .unwrap_or(area.x + (area.w - width) * 0.5)
            .clamp(area.x + 6., (area.x + area.w - width - 6.).max(area.x + 6.));
        let y = anchor
            .map(|r| r.pos.y + r.size.y + 6.)
            .unwrap_or(area.y + 12.)
            .min((area.y + area.h - height - 6.).max(area.y + 6.));
        let bounds = rect(x, y, width, height);
        let mut cells = Vec::new();
        for (i, layout) in layouts.iter().enumerate() {
            let sample = LRect::new(
                x + 16. + (i % columns) as f64 * 108.,
                y + 40. + (i / columns) as f64 * 72.,
                92.,
                56.,
            );
            for zone in layout {
                let r = zone.bounds(sample);
                cells.push((rect(r.x + 1.5, r.y + 1.5, r.w - 3., r.h - 3.), *zone));
            }
        }
        Self {
            client,
            bounds,
            drag: anchor.is_none(),
            cells,
        }
    }
    pub fn hit(&self, p: Vec2d) -> Option<Zone> {
        self.cells
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, z)| *z)
    }
    pub fn near(&self, p: Vec2d) -> bool {
        rect(
            self.bounds.pos.x - 12.,
            self.bounds.pos.y - 14.,
            self.bounds.size.x + 24.,
            self.bounds.size.y + 28.,
        )
        .contains(p)
    }
}
#[derive(Clone, Debug, Default)]
pub struct SnapState {
    pub picker: Option<Picker>,
    pub preview: Option<Zone>,
    pub area: Option<LRect>,
    pub hovered: Option<(ClientId, Rect)>,
}
impl SnapState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn drag(&mut self, client: ClientId, p: Vec2d, area: LRect) {
        self.area = Some(area);
        if p.y <= area.y + 12. && p.x > area.x + 70. && p.x < area.x + area.w - 70. {
            if !self
                .picker
                .as_ref()
                .is_some_and(|p| p.drag && p.client == client)
            {
                self.picker = Some(Picker::new(client, area, None));
            }
        } else if self.picker.as_ref().is_some_and(|picker| !picker.near(p)) {
            self.picker = None;
        }
        self.preview = self
            .picker
            .as_ref()
            .and_then(|picker| picker.hit(p))
            .or_else(|| edge_zone(p, area));
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.SnapOverlayBase = #(SnapOverlay::register_widget(vm))
    mod.widgets.SnapOverlay = set_type_default() do mod.widgets.SnapOverlayBase {
        width: Fill height: Fill
        chrome +: {}
        d +: {text.text_style: theme.font_regular text_bold.text_style: theme.font_bold}
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct SnapOverlay {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    #[redraw]
    chrome: DrawDesktopChrome,
    #[live]
    d: ShellDraw,
}
impl Widget for SnapOverlay {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.walk_turtle(walk);
        let Some(state) = scope.data.get_mut::<WmState>() else {
            return DrawStep::done();
        };
        if state.style.target != desktop_style::DesktopStyle::Windows {
            return DrawStep::done();
        }
        let snap = &state.snap;
        self.chrome.radius = 8.;
        self.chrome.bevel = 0.;
        self.chrome.top_only = 0.;
        self.chrome.frame_width = 0.;
        if let (Some(zone), Some(area)) = (snap.preview, snap.area) {
            let r = zone.bounds(area);
            self.chrome.color = alpha(rgb(90, 166, 242), 0.25);
            self.chrome.draw_abs(
                cx,
                rect(r.x + 6., r.y + 6., (r.w - 12.).max(1.), (r.h - 12.).max(1.)),
            );
            self.chrome.frame_width = 1.;
            self.chrome.color = alpha(rgb(116, 190, 255), 0.9);
            self.chrome.draw_abs(
                cx,
                rect(r.x + 6., r.y + 6., (r.w - 12.).max(1.), (r.h - 12.).max(1.)),
            );
            self.chrome.frame_width = 0.;
        }
        if let Some(picker) = &snap.picker {
            let dark = state.style.dark;
            self.chrome.color = if dark {
                rgb(38, 38, 42)
            } else {
                rgb(247, 247, 250)
            };
            self.chrome.draw_abs(cx, picker.bounds);
            self.d.label(
                cx,
                rect(
                    picker.bounds.pos.x + 16.,
                    picker.bounds.pos.y + 8.,
                    picker.bounds.size.x - 32.,
                    24.,
                ),
                false,
                11.,
                if dark {
                    rgb(239, 239, 243)
                } else {
                    rgb(40, 40, 44)
                },
                HAlign::Left,
                if picker.drag {
                    "Drop into a layout"
                } else {
                    "Choose a layout"
                },
            );
            self.chrome.radius = 3.;
            for (cell, zone) in &picker.cells {
                self.chrome.color = if snap.preview == Some(*zone) {
                    rgb(0, 120, 215)
                } else if dark {
                    rgb(77, 77, 85)
                } else {
                    rgb(220, 225, 235)
                };
                self.chrome.draw_abs(cx, *cell);
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edges_and_corners_use_the_workarea() {
        let a = LRect::new(150., 40., 1200., 800.);
        assert_eq!(edge_zone(dvec2(151., 500.), a), Some(Zone::Left));
        assert_eq!(edge_zone(dvec2(1348., 837.), a), Some(Zone::BottomRight));
        assert_eq!(edge_zone(dvec2(152., 42.), a), Some(Zone::TopLeft));
        assert_eq!(edge_zone(dvec2(700., 41.), a), Some(Zone::Maximize));
        assert_eq!(edge_zone(dvec2(700., 837.), a), None);
        assert_eq!(
            Zone::BottomRight.bounds(a),
            LRect::new(750., 440., 600., 400.)
        );
    }
    #[test]
    fn top_picker_keeps_its_target_until_pointer_leaves() {
        let a = LRect::new(0., 30., 1200., 800.);
        let mut s = SnapState::default();
        s.drag(1, dvec2(600., 32.), a);
        let cell = s.picker.as_ref().unwrap().cells[1];
        s.drag(1, cell.0.pos + cell.0.size * 0.5, a);
        assert_eq!(s.preview, Some(cell.1));
        s.drag(1, dvec2(600., 600.), a);
        assert!(s.picker.is_none());
        assert!(s.preview.is_none());
    }
}
