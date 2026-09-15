//! App-local theme roles and pure presentation adapters. No document ownership.
use crate::model::*;
use makepad_civil_time::{self as civil, Day};
use makepad_widgets::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CalendarColors {
    pub paper: Vec4f,
    pub chrome: Vec4f,
    pub sidebar: Vec4f,
    pub ink: Vec4f,
    pub secondary: Vec4f,
    pub rule: Vec4f,
    pub selection: Vec4f,
    pub focus: Vec4f,
    pub action: Vec4f,
    pub categories: [Vec4f; 4],
    pub dark: bool,
    raw_secondary: Vec4f,
    raw_action: Vec4f,
    raw_categories: [Vec4f; 4],
}

pub fn mix(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    a * (1.0 - t) + b * t
}
fn luminance(c: Vec4f) -> f32 {
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.x) + 0.7152 * linear(c.y) + 0.0722 * linear(c.z)
}
pub fn composite(color: Vec4f, surface: Vec4f) -> Vec4f {
    Vec4f { w: 1.0, ..mix(surface, color, color.w.clamp(0.0, 1.0)) }
}
pub fn contrast(a: Vec4f, b: Vec4f) -> f32 {
    let (a, b) = (luminance(composite(a, b)), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
pub fn readable(color: Vec4f, surface: Vec4f, ink: Vec4f, minimum: f32) -> Vec4f {
    let color = composite(color, surface);
    let ink = composite(ink, surface);
    let ink = if contrast(ink, surface) >= minimum { ink } else {
        let black = vec4(0.0, 0.0, 0.0, 1.0);
        let white = vec4(1.0, 1.0, 1.0, 1.0);
        if contrast(black, surface) > contrast(white, surface) { black } else { white }
    };
    if contrast(color, surface) >= minimum {
        return color;
    }
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let mid = (low + high) * 0.5;
        if contrast(mix(color, ink, mid), surface) >= minimum {
            high = mid;
        } else {
            low = mid;
        }
    }
    mix(color, ink, high)
}
fn unpack(c: u32) -> Vec4f {
    vec4(
        (c >> 24) as f32 / 255.0,
        ((c >> 16) & 255) as f32 / 255.0,
        ((c >> 8) & 255) as f32 / 255.0,
        (c & 255) as f32 / 255.0,
    )
}
impl CalendarColors {
    pub fn resolve(vm: &mut ScriptVm) -> Self {
        // Resolve in the current isolate, including theme refreshes made by the host.
        let _palette = makepad_wm_theme::current_for_vm(vm);
        let theme = vm.module(id!(theme));
        let role = |name: &str| {
            unpack(
                vm.bx
                    .heap
                    .value(theme, LiveId::from_str(name).into(), NoTrap)
                    .as_color()
                    .unwrap_or(0),
            )
        };
        let paper = role("color_inset");
        let ink = role("color_text");
        let chrome = role("color_bg_app");
        let sidebar = role("color_bg_container");
        let raw_secondary = role("color_text_meta");
        let raw_action = role("color_error");
        let raw_categories = [role("color_map_1"), role("color_map_4"), role("color_map_6"), role("color_map_3")];
        let secondary = readable(raw_secondary, paper, ink, 4.5);
        let action = readable(role("color_error"), paper, ink, 4.5);
        let dark = luminance(paper) < luminance(ink);
        Self {
            raw_secondary,
            raw_action,
            raw_categories,
            paper,
            chrome,
            sidebar,
            ink,
            secondary,
            action,
            dark,
            rule: mix(paper, ink, if dark { 0.22 } else { 0.16 }),
            selection: role("color_bg_highlight"),
            focus: role("color_focus"),
            categories: raw_categories.map(|c| readable(c, paper, ink, 3.0)),
        }
    }
    pub fn on_surface(self, surface: Vec4f) -> Self {
        let paper = composite(surface, self.paper);
        Self {
            paper,
            secondary: readable(self.raw_secondary, paper, self.ink, 4.5),
            action: readable(self.raw_action, paper, self.ink, 4.5),
            categories: self.raw_categories.map(|c| readable(c, paper, self.ink, 3.0)),
            rule: mix(paper, self.ink, if self.dark {0.22} else {0.16}),
            ..self
        }
    }
    pub fn category(self, c: CalendarColour) -> Vec4f {
        self.categories[match c {
            CalendarColour::Home => 0,
            CalendarColour::Work => 1,
            CalendarColour::Birthdays => 2,
            CalendarColour::Holidays => 3,
        }]
    }
    pub fn tint(self, c: CalendarColour) -> Vec4f {
        mix(
            self.paper,
            self.category(c),
            if self.dark { 0.22 } else { 0.12 },
        )
    }
    pub fn on_action(self) -> Vec4f {
        if contrast(self.paper, self.action) > contrast(self.ink, self.action) {
            self.paper
        } else {
            self.ink
        }
    }
}

pub fn period_heading(mode: CalendarMode, displayed: Day, selected: Day) -> String {
    match mode {
        CalendarMode::Month => format_month_year(displayed),
        CalendarMode::Day => format!(
            "{}, {}",
            format_heading_compact_day(selected),
            civil::to_ymd(selected).0
        ),
        CalendarMode::Week => {
            let start = monday_of(selected);
            let end = start + 6;
            let (sy, sm, sd) = civil::to_ymd(start);
            let (ey, em, ed) = civil::to_ymd(end);
            if sy != ey {
                format!(
                    "{} {sd}, {sy} – {} {ed}, {ey}",
                    civil::month_name(sm),
                    civil::month_name(em)
                )
            } else if sm != em {
                format!(
                    "{} {sd} – {} {ed}, {ey}",
                    civil::month_name(sm),
                    civil::month_name(em)
                )
            } else {
                format!("{} {sd}–{ed}, {ey}", civil::month_name(sm))
            }
        }
    }
}
pub fn occurrence_for_key(doc: &CalendarDocument, key: OccurrenceKey) -> Option<Occurrence> {
    let event = event_by_id(doc, key.event_id)?;
    crate::engine::expand_range(
        std::slice::from_ref(event),
        key.start_day,
        key.start_day + 1,
        UI_EXPAND_CAP,
    )
    .ok()?
    .items
    .into_iter()
    .find(|o| o.key == key)
}
pub fn occurrence_dates(timing: Timing) -> String {
    match timing {
        Timing::Timed { start, end } if start.day == end.day => format!(
            "{}\n{} – {}",
            period_heading(CalendarMode::Day, start.day, start.day),
            start.format_hm(),
            end.format_hm()
        ),
        Timing::Timed { start, end } => format!(
            "{} at {}\n– {} at {}",
            format_heading_compact_day(start.day),
            start.format_hm(),
            format_heading_compact_day(end.day),
            end.format_hm()
        ),
        Timing::AllDay {
            start,
            end_exclusive,
        } if end_exclusive == start + 1 => format!(
            "{}\nAll-day",
            period_heading(CalendarMode::Day, start, start)
        ),
        Timing::AllDay {
            start,
            end_exclusive,
        } => format!(
            "{} – {}\nAll-day",
            format_heading_compact_day(start),
            format_heading_compact_day(end_exclusive - 1)
        ),
    }
}
pub fn occurrence_id(prefix: &str, key: OccurrenceKey) -> LiveId {
    LiveId::from_str(&format!("{prefix}_{}", key.as_tool_id()))
}
pub fn fixed(rect: Rect) -> Walk {
    Walk {
        abs_pos: Some(rect.pos),
        width: Size::Fixed(rect.size.x.max(0.0)),
        height: Size::Fixed(rect.size.y.max(0.0)),
        ..Walk::default()
    }
}
pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        pos: dvec2(x, y),
        size: dvec2(w.max(0.0), h.max(0.0)),
    }
}
pub fn hairline(cx: &Cx2d) -> f64 {
    0.5_f64.max(1.0 / cx.current_dpi_factor())
}
pub fn text_at(cx: &mut Cx2d, text: &mut DrawText, r: Rect, value: &str, align: Align) {
    if r.size.x < 2.0 || r.size.y < 2.0 {
        return;
    }
    cx.begin_turtle(
        fixed(r),
        Layout {
            flow: Flow::right(),
            ..Layout::default()
        },
    );
    text.draw_walk(
        cx,
        Walk {
            width: Size::fill(),
            height: Size::fill(),
            ..Walk::default()
        },
        align,
        value,
    );
    cx.end_turtle();
}

pub fn script_mod(vm: &mut ScriptVm) {
    let c = CalendarColors::resolve(vm);
    script_eval!(vm, {
        use mod.prelude.widgets.*
        use mod.widgets.*
        mod.calendar = {}
        mod.calendar.paper = #(c.paper)
        mod.calendar.chrome = #(c.chrome)
        mod.calendar.sidebar = #(c.sidebar)
        mod.calendar.ink = #(c.ink)
        mod.calendar.secondary = #(c.secondary)
        mod.calendar.rule = #(c.rule)
        mod.calendar.action = #(c.action)
        mod.calendar.selection = #(c.selection)
        mod.calendar.focus = #(c.focus)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn period_titles_and_occurrence_dates_follow_presentation() {
        let d = civil::from_ymd(2026, 9, 9);
        assert_eq!(period_heading(CalendarMode::Month, d, d), "September 2026");
        assert_eq!(
            period_heading(CalendarMode::Week, d, d),
            "September 7–13, 2026"
        );
        let doc = crate::seed::seed(d);
        let series = doc.events.iter().find(|e| e.title == "Standup").unwrap();
        let key = OccurrenceKey {
            event_id: series.id,
            start_day: monday_of(d) + 7,
        };
        let occ = occurrence_for_key(&doc, key).unwrap();
        assert_eq!(occ.timing.start_day(), key.start_day);
        assert_ne!(occ.timing.start_day(), series.timing.start_day());
        assert!(occurrence_dates(occ.timing).contains("14 September"));
    }
    #[test]
    fn contrast_adjustment_meets_text_and_mark_thresholds() {
        for (paper, ink) in [
            (vec4(1.0, 1.0, 1.0, 1.0), vec4(0.0, 0.0, 0.0, 1.0)),
            (vec4(0.06, 0.06, 0.06, 1.0), vec4(0.95, 0.95, 0.95, 1.0)),
        ] {
            for ratio in [3.0, 4.5] {
                assert!(
                    contrast(readable(mix(paper, ink, 0.15), paper, ink, ratio), paper)
                        >= ratio - 0.001
                );
            }
        }
    }
    #[test]
    fn translucent_metadata_is_composited_against_each_actual_surface() {
        let white=vec4(1.0,1.0,1.0,1.0);
        let black=vec4(0.0,0.0,0.0,1.0);
        let meta=vec4(0.0,0.0,0.0,0.4);
        assert!((contrast(meta,white)-2.849).abs()<0.01);
        for (paper, ink) in [(white,black),(black,white)] {
            let raw = if paper==white {meta} else {vec4(1.0,1.0,1.0,0.4)};
            let c=CalendarColors{paper,ink,raw_secondary:raw,raw_action:raw,raw_categories:[raw;4],..CalendarColors::default()};
            for surface in [paper,mix(paper,ink,0.08),mix(paper,ink,0.2)] {
                let adjusted=c.on_surface(surface);
                assert!(contrast(adjusted.secondary,surface)>=4.5-0.001);
                assert!(contrast(adjusted.action,surface)>=4.5-0.001);
                assert!(adjusted.categories.iter().all(|mark|contrast(*mark,surface)>=3.0-0.001));
                assert_eq!(adjusted.secondary.w,1.0);
            }
        }
    }

}

#[derive(Clone, Debug)]
pub struct PresentedOccurrence {
    pub occurrence: Occurrence,
    pub title: String,
    pub calendar_name: String,
    pub colour: CalendarColour,
}
pub fn present(doc: &CalendarDocument, occurrences: Vec<Occurrence>) -> Vec<PresentedOccurrence> {
    occurrences
        .into_iter()
        .filter_map(|occurrence| {
            let event = event_by_id(doc, occurrence.key.event_id)?;
            let cal = calendar_by_id(doc, event.calendar_id)?;
            Some(PresentedOccurrence {
                occurrence,
                title: event.title.clone(),
                calendar_name: cal.name.clone(),
                colour: cal.colour,
            })
        })
        .collect()
}

/// Refresh static recipe colours without rehydrating any text, selection, or undo state.
pub fn retint_widget(
    cx: &mut Cx,
    widget: &WidgetRef,
    recipe: CalendarColors,
    current: CalendarColors,
    surface: Vec4f,
    apply: bool,
) -> Vec4f {
    let source = widget.script_source();
    let colors = cx.with_vm(|vm| {
        [live_id!(draw_bg), live_id!(draw_text), live_id!(draw_icon)].map(|field| {
            let draw = vm.bx.heap.value(source, field.into(), NoTrap);
            let mut value = vm
                .bx
                .heap
                .value(draw.as_object()?, live_id!(color).into(), NoTrap);
            if let Some(object) = value.as_object() {
                value = vm.bx.heap.value(object, live_id!(value).into(), NoTrap);
            }
            if value.as_color().is_some() || value.as_pod().is_some() {
                Some(Vec4f::script_from_value(vm, value))
            } else {
                None
            }
        })
    });
    let mapped = |value: Vec4f| {
        [
            (recipe.paper, current.paper),
            (recipe.chrome, current.chrome),
            (recipe.sidebar, current.sidebar),
            (recipe.ink, current.ink),
            (recipe.secondary, current.secondary),
            (recipe.rule, current.rule),
            (recipe.selection, current.selection),
            (recipe.action, current.action),
            (recipe.focus, current.focus),
        ]
        .into_iter()
        .find_map(|(old, new)| if old == value { Some(new) } else { None })
    };
    let paints = cx.with_vm(|vm| vm.bx.heap.value(source, live_id!(show_bg).into(), NoTrap).as_bool().unwrap_or(false));
    let surface = if paints {
        colors[0].and_then(mapped).map(|c| composite(c, surface)).unwrap_or(surface)
    } else { surface };
    if !apply { return surface; }
    let mut widget = widget.clone();
    if let Some(color) = colors[0].and_then(mapped) {
        script_apply_eval!(cx,widget,{draw_bg.color:#(color)});
    }
    if let Some(color) = colors[2].and_then(mapped) {
        let color = readable(color, surface, current.ink, 3.0);
        script_apply_eval!(cx,widget,{draw_icon +: {color:#(color) color_hover:#(color) color_down:#(color)}});
    }
    if let Some(color) = colors[1].and_then(mapped) {
        let color = readable(color, surface, current.ink, 4.5);
        if widget.borrow::<Button>().is_some() || widget.borrow::<DropDown>().is_some() {
            script_apply_eval!(cx,widget,{draw_text +: {color:#(color) color_hover:#(color) color_down:#(color)}});
        } else {
            script_apply_eval!(cx,widget,{draw_text.color:#(color)});
        }
    }
    surface
}
