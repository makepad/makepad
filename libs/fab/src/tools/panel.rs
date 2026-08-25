//! Lane E: `FabToolPanel` — the N sidebar's **Tool** tab.
//!
//! Everything the tools need that is not a click in the viewport: the measure
//! kind and its snap settings, the display unit, the sticky measurement list
//! (with delete), the section axes / box / flip / clear, the visibility
//! commands, the explode mode + slider, and the sun study's date, time and
//! latitude with a "play day" scrub.
//!
//! It reads `AppState` and emits `ShellAction`s, with one exception the
//! contract grants lane E: deleting a row writes `AppState::measurements`
//! directly (`api.rs`, "hot-path direct writes allowed … tools (E):
//! `measurements`").
//!
//! `FabToolOverlay` hosts it today; lane D can place it in the real N
//! sidebar and set `hosts_panel: false` on the overlay.

use crate::api::*;
use crate::tools::{explode, isolate, measure, section, session, sun_study};
use crate::model::units::LengthUnit;
use crate::ui::dragnum::*;
use makepad_widgets::*;

/// How many measurements the list shows before it says "+N more".
const LIST_ROWS: usize = 6;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // A small text toggle. Hover and active are real states (polish law).
    let Chip = View{
        width: Fit
        height: 18
        padding: Inset{left: 7 right: 7 top: 0 bottom: 0}
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            active: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                let c = fab.color_button.mix(fab.color_button_hover, self.hover).mix(fab.color_button_active, self.active)
                sdf.fill_keep(c)
                sdf.stroke(fab.color_border, 1.0)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
        label := mod.widgets.FabLabelSmall{
            text: "?"
            draw_text +: {
                color: fab.color_text
            }
        }
    }

    // The icon twin of `Chip`. Built here from `FabIcon` rather than from
    // `FabIconButton` so the panel does not move every time lane D's kit
    // does; every colour and size is still a `fab.*` token.
    let IconChip = View{
        width: 22
        height: 18
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            active: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                let c = fab.color_button.mix(fab.color_button_hover, self.hover).mix(fab.color_button_active, self.active)
                sdf.fill_keep(c)
                sdf.stroke(fab.color_border, 1.0)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
        icon := mod.widgets.FabIcon{
            width: 13
            height: 13
            icon_walk: Walk{ width: 13 height: 13 }
        }
    }

    let Row = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: 3
        align: Align{x: 0.0 y: 0.5}
        margin: Inset{top: 2 bottom: 2}
    }

    let Head = mod.widgets.FabPanelHeader{
        margin: Inset{top: 4}
    }

    // Every numeric row is the shared drag-number field: drag to change
    // (live), click to type, arrows and Ctrl+Wheel to step. `show_fill`
    // marks the rows whose range means something.
    let PanelNum = mod.widgets.FabDragNumber{
        width: Fill
        height: 18
        margin: Inset{top: 1 bottom: 1}
        padding: Inset{left: 6 right: 6 top: 0 bottom: 0}
        draw_text +: {
            color: fab.color_text_dim
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
        }
        text_input +: {
            draw_text +: {
                text_style: theme.font_regular{
                    font_size: fab.font_size_small
                }
            }
        }
    }

    let MeasureRow = View{
        width: Fill
        height: 17
        flow: Right
        spacing: 4
        align: Align{x: 0.0 y: 0.5}
        kind := mod.widgets.FabLabelSmall{
            width: 34
            text: "—"
        }
        value := mod.widgets.FabLabelSmall{
            width: Fill
            text: ""
            draw_text +: {
                color: fab.color_vp_measure
            }
        }
        del := IconChip{
            width: 16
            height: 15
            icon +: {
                width: 11
                height: 11
                icon_walk: Walk{ width: 11 height: 11 }
                draw_icon +: {
                    color: fab.color_text_dim
                    svg: crate_resource("self://resources/icons/close.svg")
                }
            }
        }
    }

    mod.widgets.FabToolPanelBase = #(FabToolPanel::register_widget(vm))
    mod.widgets.FabToolPanel = set_type_default() do mod.widgets.FabToolPanelBase{
        width: Fill
        height: Fill
        flow: Right
        // The column tracks its host: the N sidebar decides the width (and
        // the user can drag that), the panel just fills it.
        col := ScrollYView{
            width: Fill
            height: Fill
            flow: Down
            spacing: 1
            margin: Inset{top: 112 right: 8 bottom: 10}
            padding: Inset{left: 8 right: 8 top: 6 bottom: 10}
            show_bg: true
            draw_bg +: {
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                    sdf.fill_keep(vec4(fab.color_panel_sub.xyz, 0.93))
                    sdf.stroke(fab.color_border, 1.0)
                    return sdf.result
                }
            }

            head_tool := Head{ title +: { text: "Tool" } }
            tool_name := mod.widgets.FabLabelSmall{ text: "Select" }

            head_measure := Head{ title +: { text: "Measure" } }
            mrow := Row{
                m_dist := IconChip{
                    icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/measure.svg") } }
                }
                m_area := IconChip{
                    icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/measure_area.svg") } }
                }
                m_angle := IconChip{
                    icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/measure_angle.svg") } }
                }
                Filler{}
                m_hint := mod.widgets.FabLabelSmall{ text: "M" }
            }
            srow := Row{
                snap_icon := mod.widgets.FabIconDim{
                    width: 13
                    height: 13
                    icon_walk: Walk{ width: 13 height: 13 }
                    draw_icon +: { svg: crate_resource("self://resources/icons/snap_magnet.svg") }
                }
                s_vertex := Chip{ label +: { text: "V" } }
                s_mid := Chip{ label +: { text: "M" } }
                s_edge := Chip{ label +: { text: "E" } }
                s_face := Chip{ label +: { text: "F" } }
            }
            urow := Row{
                u_label := mod.widgets.FabLabelSmall{ width: 34 text: "Units" }
                u_mm := Chip{ label +: { text: "mm" } }
                u_cm := Chip{ label +: { text: "cm" } }
                u_m := Chip{ label +: { text: "m" } }
                u_ft := Chip{ label +: { text: "ft" } }
            }
            mlist := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: 1
                margin: Inset{top: 2 bottom: 2}
                r0 := MeasureRow{}
                r1 := MeasureRow{}
                r2 := MeasureRow{}
                r3 := MeasureRow{}
                r4 := MeasureRow{}
                r5 := MeasureRow{}
                more := mod.widgets.FabLabelSmall{ text: "" }
            }
            mfoot := Row{
                m_clear := Chip{ label +: { text: "Clear all" } }
                Filler{}
                m_total := mod.widgets.FabLabelSmall{ text: "" }
            }

            head_section := Head{ title +: { text: "Section" } }
            xrow := Row{
                sec_x := Chip{ label +: { text: "X" } }
                sec_y := Chip{ label +: { text: "Y" } }
                sec_z := Chip{ label +: { text: "Z" } }
                sec_flip := Chip{ label +: { text: "Flip" } }
            }
            brow := Row{
                sec_box := Chip{ label +: { text: "Box" } }
                sec_sel := Chip{ label +: { text: "Selected" } }
                sec_clear := Chip{ label +: { text: "Clear" } }
            }
            sec_state := mod.widgets.FabLabelSmall{ text: "none" }

            head_vis := Head{ title +: { text: "Visibility" } }
            vrow := Row{
                v_hide := Chip{ label +: { text: "Hide" } }
                v_iso := Chip{ label +: { text: "Isolate" } }
                v_show := Chip{ label +: { text: "Show all" } }
                v_solo := Chip{ label +: { text: "Solo" } }
            }
            vrow2 := Row{
                v_storey := Chip{ label +: { text: "Storey" } }
                v_layer := Chip{ label +: { text: "Layer" } }
                v_type := Chip{ label +: { text: "Type" } }
                v_hide_type := Chip{ label +: { text: "Hide type" } }
            }
            vis_state := mod.widgets.FabLabelSmall{ text: "Everything visible" }

            head_explode := Head{ title +: { text: "Explode" } }
            erow := Row{
                e_story := Chip{ label +: { text: "Storey" } }
                e_elem := Chip{ label +: { text: "Element" } }
                e_value_tip := mod.widgets.FabTipFill{
                    e_value := mod.widgets.FabLabelSmall{
                        width: Fill
                        align: Align{x: 1.0 y: 0.5}
                        text: "0 %"
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                }
            }
            FabTipFill{ text: "Amount"
                e_amount := PanelNum{ label: "Amount" min: 0.0 max: 1.0 step: 0.05 precision: 2 show_fill: true }
            }

            head_sun := Head{ title +: { text: "Sun Study" } }
            FabTipFill{ text: "Time of day"
                sun_hour := PanelNum{ label: "Time" min: 0.0 max: 24.0 step: 0.25 wrap: true show_fill: true time_of_day: true text_input +: {is_numeric_only: false} }
            }
            FabTipFill{ text: "Year"
                sun_year := PanelNum{ label: "Year" min: 2000.0 max: 2100.0 step: 1.0 precision: 0 quantize: true }
            }
            FabTipFill{ text: "Month"
                sun_month := PanelNum{ label: "Month" min: 1.0 max: 13.0 step: 1.0 precision: 0 wrap: true show_fill: true quantize: true }
            }
            FabTipFill{ text: "Day"
                sun_day := PanelNum{ label: "Day" min: 1.0 max: 31.0 step: 1.0 precision: 0 show_fill: true quantize: true }
            }
            FabTipFill{ text: "Latitude"
                sun_lat := PanelNum{ label: "Latitude" min: -90.0 max: 90.0 step: 1.0 precision: 3 show_fill: true }
            }
            FabTipFill{ text: "Longitude"
                sun_lon := PanelNum{ label: "Longitude" min: -180.0 max: 180.0 step: 1.0 precision: 3 show_fill: true }
            }
            FabTipFill{ text: "UTC offset"
                sun_tz := PanelNum{ label: "UTC offset" min: -12.0 max: 14.0 step: 0.25 precision: 2 show_fill: true }
            }
            FabTipFill{ text: "North offset"
                sun_north := PanelNum{ label: "North" min: -180.0 max: 180.0 step: 1.0 precision: 1 wrap: true show_fill: true }
            }
            FabTipFill{ text: "Sky turbidity"
                sun_turbidity := PanelNum{ label: "Turbidity" min: 1.2 max: 10.0 step: 0.1 precision: 1 show_fill: true }
            }
            FabTipFill{ text: "Distance haze"
                sun_haze := PanelNum{ label: "Haze" min: 0.0 max: 1.0 step: 0.05 precision: 2 show_fill: true }
            }
            FabTipFill{ text: "Exposure EV"
                sun_exposure := PanelNum{ label: "Exposure EV" min: -6.0 max: 6.0 step: 0.25 precision: 2 show_fill: true }
            }
            sunrow := Row{
                sun_play := IconChip{
                    icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/play.svg") } }
                }
                sun_shadows := Chip{ label +: { text: "Shadows" } }
                Filler{}
                sun_info := mod.widgets.FabLabelSmall{ text: "" }
            }
            sun_read := mod.widgets.FabLabelSmall{ text: "" }
        }
    }
}

/// Everything the panel mirrors from state; re-applied only when it changes.
#[derive(Clone, PartialEq)]
struct Synced {
    tool: Tool,
    snap: SnapOptions,
    unit: Option<LengthUnit>,
    measurements: usize,
    section: String,
    hidden: usize,
    isolated: bool,
    explode: ExplodeState,
    sun: SunSettings,
    sun_shadows: bool,
    playing: bool,
    labels: Vec<(String, String)>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabToolPanel {
    #[deref]
    view: View,
    #[rust]
    synced: Option<Synced>,
}

fn row_id(i: usize) -> &'static [LiveId] {
    match i {
        0 => ids!(col.mlist.r0),
        1 => ids!(col.mlist.r1),
        2 => ids!(col.mlist.r2),
        3 => ids!(col.mlist.r3),
        4 => ids!(col.mlist.r4),
        _ => ids!(col.mlist.r5),
    }
}

fn row_kind_id(i: usize) -> &'static [LiveId] {
    match i {
        0 => ids!(col.mlist.r0.kind),
        1 => ids!(col.mlist.r1.kind),
        2 => ids!(col.mlist.r2.kind),
        3 => ids!(col.mlist.r3.kind),
        4 => ids!(col.mlist.r4.kind),
        _ => ids!(col.mlist.r5.kind),
    }
}

fn row_value_id(i: usize) -> &'static [LiveId] {
    match i {
        0 => ids!(col.mlist.r0.value),
        1 => ids!(col.mlist.r1.value),
        2 => ids!(col.mlist.r2.value),
        3 => ids!(col.mlist.r3.value),
        4 => ids!(col.mlist.r4.value),
        _ => ids!(col.mlist.r5.value),
    }
}

fn row_del_id(i: usize) -> &'static [LiveId] {
    match i {
        0 => ids!(col.mlist.r0.del),
        1 => ids!(col.mlist.r1.del),
        2 => ids!(col.mlist.r2.del),
        3 => ids!(col.mlist.r3.del),
        4 => ids!(col.mlist.r4.del),
        _ => ids!(col.mlist.r5.del),
    }
}

impl FabToolPanel {
    fn set_active(&self, cx: &mut Cx, id: &[LiveId], on: bool) {
        let mut w = self.view.view(cx, id);
        let v = if on { 1.0 } else { 0.0 };
        script_apply_eval!(cx, w, {
            draw_bg +: { active: #(v) }
        });
    }

    fn clicked(&self, cx: &mut Cx, actions: &Actions, id: &[LiveId]) -> bool {
        self.view.view(cx, id).finger_up(actions).is_some()
    }

    fn label(&self, cx: &mut Cx, id: &[LiveId], text: &str) {
        self.view.label(cx, id).set_text(cx, text);
    }

    /// Build + animate a section, so every route into a section looks the same.
    fn set_section(&self, cx: &mut Cx, state: &AppState, target: SectionState) {
        let bounds = state.scene.bounds;
        let (anim, first) = section::animate_to(&state.scene_state.section, target, &bounds);
        session::with(|s| s.section_anim = Some(anim));
        cx.action(ShellAction::SetSection(first));
    }

    fn handle_measure(&self, cx: &mut Cx, actions: &Actions, state: &mut AppState) {
        for (id, kind) in [
            (ids!(col.mrow.m_dist), MeasureKind::Distance),
            (ids!(col.mrow.m_area), MeasureKind::Area),
            (ids!(col.mrow.m_angle), MeasureKind::Angle),
        ] {
            if self.clicked(cx, actions, id) {
                cx.action(ShellAction::SetTool(Tool::Measure(kind)));
            }
        }
        let mut snap = state.snap;
        let mut snap_changed = false;
        for (id, field) in [
            (ids!(col.srow.s_vertex), 0),
            (ids!(col.srow.s_mid), 1),
            (ids!(col.srow.s_edge), 2),
            (ids!(col.srow.s_face), 3),
        ] {
            if self.clicked(cx, actions, id) {
                match field {
                    0 => snap.vertex = !snap.vertex,
                    1 => snap.edge_midpoint = !snap.edge_midpoint,
                    2 => snap.edge = !snap.edge,
                    _ => snap.face = !snap.face,
                }
                snap_changed = true;
            }
        }
        if snap_changed {
            cx.action(ShellAction::SetSnap(snap));
        }
        for (id, unit) in [
            (ids!(col.urow.u_mm), LengthUnit::Millimeter),
            (ids!(col.urow.u_cm), LengthUnit::Centimeter),
            (ids!(col.urow.u_m), LengthUnit::Meter),
            (ids!(col.urow.u_ft), LengthUnit::Foot),
        ] {
            if self.clicked(cx, actions, id) {
                session::with(|s| s.display_unit = Some(unit));
                // Re-label what is already measured so the list agrees with
                // the overlay.
                let units = session::with(|s| s.units(&state.scene.units));
                for m in &mut state.measurements {
                    m.label = measure::format(m.kind, m.value, &units);
                }
                cx.redraw_all();
            }
        }
        if self.clicked(cx, actions, ids!(col.mfoot.m_clear)) {
            cx.action(ShellAction::ClearMeasurements);
        }
        for i in 0..LIST_ROWS.min(state.measurements.len()) {
            if self.clicked(cx, actions, row_del_id(i)) {
                state.measurements.remove(i);
                cx.redraw_all();
                break;
            }
        }
    }

    fn handle_section(&self, cx: &mut Cx, actions: &Actions, state: &mut AppState) {
        let bounds = state.scene.bounds;
        if aabb_is_empty(&bounds) {
            return;
        }
        for (id, axis) in [
            (ids!(col.xrow.sec_x), 0usize),
            (ids!(col.xrow.sec_y), 1),
            (ids!(col.xrow.sec_z), 2),
        ] {
            if self.clicked(cx, actions, id) {
                // Clicking the same axis again flips it.
                let existing = state.scene_state.section.planes.first().map(|p| {
                    let n = section::normal(&p.plane);
                    let a = [n.x, n.y, n.z][axis];
                    (a.abs() > 0.9, a > 0.0)
                });
                let positive = match existing {
                    Some((true, pos)) => !pos,
                    _ => true,
                };
                let target = section::single(section::plane_from_axis(&bounds, axis, positive));
                self.set_section(cx, state, target);
                cx.action(ShellAction::SetTool(Tool::Section));
            }
        }
        if self.clicked(cx, actions, ids!(col.xrow.sec_flip)) {
            let mut s = state.scene_state.section.clone();
            for p in &mut s.planes {
                *p = section::flip(p);
            }
            if let Some(b) = s.boxed {
                let _ = b;
            }
            cx.action(ShellAction::SetSection(s));
        }
        if self.clicked(cx, actions, ids!(col.brow.sec_box)) {
            if state.scene_state.section.boxed.is_some() {
                cx.action(ShellAction::SetSection(SectionState::default()));
            } else {
                let target = section::boxed(section::box_from_bounds(&bounds, 0.12));
                self.set_section(cx, state, target);
                cx.action(ShellAction::SetTool(Tool::Section));
            }
        }
        if self.clicked(cx, actions, ids!(col.brow.sec_sel)) {
            if let Some(b) = state.selection_bounds() {
                let pad = aabb_radius(&b).max(0.2) * 0.25;
                let grown = Aabb {
                    min: b.min - vec3(pad, pad, pad),
                    max: b.max + vec3(pad, pad, pad),
                };
                let target = section::boxed(grown);
                self.set_section(cx, state, target);
                cx.action(ShellAction::SetTool(Tool::Section));
            } else {
                session::with(|s| s.hint = "Select something to box first".into());
            }
        }
        if self.clicked(cx, actions, ids!(col.brow.sec_clear)) {
            session::with(|s| s.section_anim = None);
            cx.action(ShellAction::SetSection(SectionState::default()));
        }
    }

    fn handle_visibility(&self, cx: &mut Cx, actions: &Actions, state: &AppState) {
        if self.clicked(cx, actions, ids!(col.vrow.v_hide)) {
            isolate::hide_selected(cx);
        }
        if self.clicked(cx, actions, ids!(col.vrow.v_iso)) {
            isolate::isolate_selected(cx);
        }
        if self.clicked(cx, actions, ids!(col.vrow.v_show)) {
            isolate::unhide_all(cx);
        }
        if self.clicked(cx, actions, ids!(col.vrow.v_solo)) {
            let hint = isolate::toggle_solo(cx, state);
            session::with(|s| s.hint = hint.into());
        }
        // "Everything on this storey / layer / of this type", from whatever is
        // active. Lane D's outliner context menu calls the same functions.
        let active = state.scene_state.selection.active;
        if self.clicked(cx, actions, ids!(col.vrow2.v_storey)) {
            match active.and_then(|id| state.scene.element(id)).and_then(|e| e.story) {
                Some(s) => {
                    isolate::isolate_story(cx, &state.scene, s);
                }
                None => session::with(|s| s.hint = "Select an element on a storey first".into()),
            }
        }
        if self.clicked(cx, actions, ids!(col.vrow2.v_layer)) {
            match active.and_then(|id| state.scene.element(id)).and_then(|e| e.layer) {
                Some(l) => {
                    isolate::isolate_layer(cx, &state.scene, l);
                }
                None => session::with(|s| s.hint = "Select an element on a layer first".into()),
            }
        }
        if self.clicked(cx, actions, ids!(col.vrow2.v_type)) {
            match active.and_then(|id| state.scene.element(id)).map(|e| e.class.clone()) {
                Some(c) => {
                    isolate::isolate_class(cx, &state.scene, &c);
                }
                None => session::with(|s| s.hint = "Select an element first".into()),
            }
        }
        if self.clicked(cx, actions, ids!(col.vrow2.v_hide_type)) {
            match active.and_then(|id| state.scene.element(id)).map(|e| e.class.clone()) {
                Some(c) => {
                    let n = isolate::hide_class(cx, &state.scene, &c);
                    session::with(|s| s.hint = format!("Hid {n} × {}", c.label()));
                }
                None => session::with(|s| s.hint = "Select an element first".into()),
            }
        }
    }

    fn handle_explode(&self, cx: &mut Cx, actions: &Actions, state: &AppState) {
        let mut ex = state.scene_state.explode;
        let mut changed = false;
        if self.clicked(cx, actions, ids!(col.erow.e_story)) {
            ex.mode = ExplodeMode::ByStory;
            changed = true;
        }
        if self.clicked(cx, actions, ids!(col.erow.e_elem)) {
            ex.mode = ExplodeMode::ByElement;
            changed = true;
        }
        if let Some(v) = self.view.fab_drag_number(cx, ids!(col.e_amount)).changed(actions) {
            ex.amount = v.clamp(0.0, 1.0) as f32;
            changed = true;
        }
        if changed {
            cx.action(ShellAction::SetExplode(ex));
        }
    }

    fn handle_sun(&self, cx: &mut Cx, actions: &Actions, state: &AppState, view: usize) {
        let mut sun = state.sun;
        let mut changed = false;
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_hour)).changed(actions) {
            sun.time_local = value.clamp(0.0, 24.0) as f32;
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_year)).changed(actions) {
            sun.date.year = value.round().clamp(2000.0, 2100.0) as i32;
            sun.date.day = sun
                .date
                .day
                .min(days_in_month(sun.date.year, sun.date.month));
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_month)).changed(actions) {
            sun.date.month = value.clamp(1.0, 12.0) as u8;
            sun.date.day = sun
                .date
                .day
                .min(days_in_month(sun.date.year, sun.date.month));
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_day)).changed(actions) {
            sun.date.day = (value.clamp(1.0, 31.0) as u8)
                .min(days_in_month(sun.date.year, sun.date.month));
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_lat)).changed(actions) {
            sun.latitude = value.clamp(-90.0, 90.0) as f32;
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_lon)).changed(actions) {
            sun.longitude = value.clamp(-180.0, 180.0) as f32;
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_tz)).changed(actions) {
            sun.tz_offset = value.clamp(-12.0, 14.0) as f32;
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_north)).changed(actions) {
            sun.north_deg = value.clamp(-180.0, 180.0) as f32;
            changed = true;
        }
        if let Some(value) = self
            .view
            .fab_drag_number(cx, ids!(col.sun_turbidity))
            .changed(actions)
        {
            sun.turbidity = value.clamp(1.2, 10.0) as f32;
            changed = true;
        }
        if let Some(value) = self.view.fab_drag_number(cx, ids!(col.sun_haze)).changed(actions) {
            sun.haze = value.clamp(0.0, 1.0) as f32;
            changed = true;
        }
        if let Some(value) = self
            .view
            .fab_drag_number(cx, ids!(col.sun_exposure))
            .changed(actions)
        {
            sun.exposure_ev = value.clamp(-6.0, 6.0) as f32;
            changed = true;
        }
        if self.clicked(cx, actions, ids!(col.sunrow.sun_shadows)) {
            cx.action(ShellAction::SetSunShadows(!state.sun_shadows));
        }
        if self.clicked(cx, actions, ids!(col.sunrow.sun_play)) {
            let playing = session::with(|session| {
                session.sun_play.playing = !session.sun_play.playing;
                session.sun_play.owner = view;
                session.sun_play.speed = sun_study::PLAY_HOURS_PER_SECOND;
                session.sun_play.playing
            });
            if playing && sun.time_local < sun_study::PLAY_FROM {
                sun.time_local = sun_study::PLAY_FROM;
                changed = true;
            }
            cx.redraw_all();
        }
        if changed {
            cx.action(ShellAction::SetSun(sun));
        }
    }

    fn sync(&mut self, cx: &mut Cx, state: &AppState) {
        let units = session::with(|s| s.units(&state.scene.units));
        let labels: Vec<(String, String)> = state
            .measurements
            .iter()
            .take(LIST_ROWS)
            .map(|m| {
                let k = match m.kind {
                    MeasureKind::Distance => "Dist",
                    MeasureKind::Area => "Area",
                    MeasureKind::Angle => "Ang",
                };
                (k.to_string(), measure::format(m.kind, m.value, &units))
            })
            .collect();
        let key = Synced {
            tool: state.tool,
            snap: state.snap,
            unit: session::with(|s| s.display_unit),
            measurements: state.measurements.len(),
            section: section::describe(&state.scene_state.section),
            hidden: state.scene_state.hidden.len(),
            isolated: state.scene_state.isolated.is_some(),
            explode: state.scene_state.explode,
            sun: state.sun,
            sun_shadows: state.sun_shadows,
            playing: session::with(|s| s.sun_play.playing),
            labels,
        };
        if self.synced.as_ref() == Some(&key) {
            return;
        }
        let first = self.synced.is_none();
        self.synced = Some(key.clone());

        self.label(cx, ids!(col.tool_name), state.tool.label());
        self.set_active(cx, ids!(col.mrow.m_dist), key.tool == Tool::Measure(MeasureKind::Distance));
        self.set_active(cx, ids!(col.mrow.m_area), key.tool == Tool::Measure(MeasureKind::Area));
        self.set_active(cx, ids!(col.mrow.m_angle), key.tool == Tool::Measure(MeasureKind::Angle));
        self.set_active(cx, ids!(col.srow.s_vertex), key.snap.vertex);
        self.set_active(cx, ids!(col.srow.s_mid), key.snap.edge_midpoint);
        self.set_active(cx, ids!(col.srow.s_edge), key.snap.edge);
        self.set_active(cx, ids!(col.srow.s_face), key.snap.face);

        let display = key.unit.unwrap_or(state.scene.units.display);
        self.set_active(cx, ids!(col.urow.u_mm), display == LengthUnit::Millimeter);
        self.set_active(cx, ids!(col.urow.u_cm), display == LengthUnit::Centimeter);
        self.set_active(cx, ids!(col.urow.u_m), display == LengthUnit::Meter);
        self.set_active(cx, ids!(col.urow.u_ft), display == LengthUnit::Foot);

        for i in 0..LIST_ROWS {
            let has = i < key.labels.len();
            self.view.view(cx, row_id(i)).set_visible(cx, has);
            if has {
                self.label(cx, row_kind_id(i), &key.labels[i].0);
                self.label(cx, row_value_id(i), &key.labels[i].1);
            }
        }
        let more = key.measurements.saturating_sub(LIST_ROWS);
        self.label(
            cx,
            ids!(col.mlist.more),
            &if more > 0 {
                format!("+{more} more")
            } else if key.measurements == 0 {
                "Nothing measured yet".to_string()
            } else {
                String::new()
            },
        );
        let total: f64 = state
            .measurements
            .iter()
            .filter(|m| m.kind == MeasureKind::Distance)
            .map(|m| m.value)
            .sum();
        self.label(
            cx,
            ids!(col.mfoot.m_total),
            &if total > 0.0 {
                format!("Σ {}", units.format_length(total))
            } else {
                String::new()
            },
        );

        let (kept, total) = section::kept_elements(&state.scene_state.section, &state.scene);
        self.label(
            cx,
            ids!(col.sec_state),
            &if kept == total {
                key.section.clone()
            } else {
                format!("{} · {kept} of {total} elements", key.section)
            },
        );
        let planes = &state.scene_state.section.planes;
        for (i, id) in [ids!(col.xrow.sec_x), ids!(col.xrow.sec_y), ids!(col.xrow.sec_z)]
            .into_iter()
            .enumerate()
        {
            let on = planes.first().map_or(false, |p| {
                let n = section::normal(&p.plane);
                [n.x, n.y, n.z][i].abs() > 0.9
            });
            self.set_active(cx, id, on);
        }
        self.set_active(cx, ids!(col.brow.sec_box), state.scene_state.section.boxed.is_some());

        self.label(cx, ids!(col.vis_state), &isolate::describe(state));
        self.set_active(cx, ids!(col.vrow.v_solo), key.isolated);

        self.set_active(cx, ids!(col.erow.e_story), key.explode.mode == ExplodeMode::ByStory);
        self.set_active(cx, ids!(col.erow.e_elem), key.explode.mode == ExplodeMode::ByElement);
        let explode_readout = format!(
            "{} · {:.0} %",
            explode::mode_label(key.explode.mode),
            key.explode.amount * 100.0
        );
        self.label(cx, ids!(col.erow.e_value), &explode_readout);
        // The narrow-panel degrade elides the readout; the tooltip keeps the
        // whole of it reachable.
        let mut tip = self.view.widget(cx, ids!(col.erow.e_value_tip));
        script_apply_eval!(cx, tip, { text: #(explode_readout) });
        self.label(
            cx,
            ids!(col.mrow.m_hint),
            match key.tool {
                Tool::Measure(k) => measure::kind_label(k),
                _ => "M",
            },
        );

        self.set_active(cx, ids!(col.sunrow.sun_shadows), key.sun_shadows);
        self.set_active(cx, ids!(col.sunrow.sun_play), key.playing);
        self.label(
            cx,
            ids!(col.sunrow.sun_info),
            &sun_study::clock(key.sun.time_local),
        );
        self.label(cx, ids!(col.sun_read), &sun_study::describe(&key.sun));

        // The number rows are two-way: only push when the value really
        // moved, so a drag never fights its own feedback.
        for (id, value) in [
            (ids!(col.e_amount), key.explode.amount as f64),
            (ids!(col.sun_hour), key.sun.time_local as f64),
            (ids!(col.sun_year), key.sun.date.year as f64),
            (ids!(col.sun_month), key.sun.date.month as f64),
            (ids!(col.sun_day), key.sun.date.day as f64),
            (ids!(col.sun_lat), key.sun.latitude as f64),
            (ids!(col.sun_lon), key.sun.longitude as f64),
            (ids!(col.sun_tz), key.sun.tz_offset as f64),
            (ids!(col.sun_north), key.sun.north_deg as f64),
            (ids!(col.sun_turbidity), key.sun.turbidity as f64),
            (ids!(col.sun_haze), key.sun.haze as f64),
            (ids!(col.sun_exposure), key.sun.exposure_ev as f64),
        ] {
            let num = self.view.fab_drag_number(cx, id);
            if first || (num.value() - value).abs() > 1e-3 {
                num.set_value(cx, value);
            }
        }
    }
}

impl Widget for FabToolPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let view = scope.data.get::<AppState>().map(|s| s.active_view).unwrap_or(0);
            if let Some(state) = scope.data.get_mut::<AppState>() {
                // Split so each helper borrows what it needs.
                self.handle_measure(cx, actions, state);
                self.handle_section(cx, actions, state);
                self.handle_visibility(cx, actions, state);
                self.handle_explode(cx, actions, state);
                self.handle_sun(cx, actions, state, view);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The scope borrow ends with this block, so the view can draw after.
        {
            if let Some(state) = scope.data.get::<AppState>() {
                self.sync(cx, state);
            }
        }
        self.view.draw_walk(cx, scope, walk)
    }
}
