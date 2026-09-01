//! Lane D. The 3D viewport *area*: Fab's header row, the body as an
//! overlay stack — the viewport (lane B) at the bottom, then the tool overlay
//! (E), the T toolbar (left), the N sidebar (right), the nav gizmo (C,
//! top-right) and the overlay text (top-left).
//!
//! Function first. Everything in this header changes the picture:
//! * the mode dropdown switches Object / Walk / Fly (`SetNavMode`, `SetTool`),
//! * View / Select / Object are real menus over the shared menu layer,
//! * the shading strip covers every mode including Realtime and hidden-line ink,
//! * the Overlays and Gizmo dropdowns are checkbox / radio menus writing
//!   `SetOverlays` and `PresetView`,
//! * X-ray, lock-views and every T-tool toggle their state,
//! * right-click in the viewport raises the context menu, `Z` the shading pie,
//! * the N sidebar's Item / View / Tool tabs show the active element, the
//!   camera (fov and clip planes are live drag-numerics) and the snapping
//!   options (`SetSnap`).
//!
//! Toggles are `RadioButton`-derived (`FabIconToggle`) rather than buttons,
//! because `RadioButton` already owns the `active` animator state — a plain
//! button has nowhere to keep "this mode is the current one".

use crate::api::*;
use crate::ui::dragnum::*;
use crate::ui::dropdown::*;
use crate::ui::popover::{
    dropdown_clicked, menu_picked, open_menu, pie_picked, ui_actions, FabUiAction, MenuItem,
    MenuPlace, PieItem,
};
use crate::ui::widgets::{clamp_header_pan, fold_panel_clicked, set_panel_chevron};
use makepad_widgets::*;

const HEADER_OVERFLOW_OWNER: LiveId = live_id!(fab_vp_header_overflow);
const HEADER_CONTEXT_OWNER: LiveId = LiveId(0x6269_6d78_0012_0001);
const COMPACT_MENU: LiveId = live_id!(fab_vp_compact);
const COMPACT_PARENT_BASE: u64 = 0x6269_6d78_0012_0200;
const HEADER_CONTROL_COUNT: usize = 21;

fn header_control_path(index: usize) -> &'static [LiveId] {
    match index {
        0 => ids!(header.scroller.content.left_cluster.editor_type),
        1 => ids!(header.scroller.content.left_cluster.mode),
        2 => ids!(header.scroller.content.left_cluster.menus.menu_view),
        3 => ids!(header.scroller.content.left_cluster.menus.menu_select),
        4 => ids!(header.scroller.content.left_cluster.menus.menu_object),
        5 => ids!(header.scroller.content.left_cluster.menu_compact),
        6 => ids!(header.scroller.content.right_cluster.sun_controls.time_control),
        7 => ids!(header.scroller.content.right_cluster.sun_controls.haze_control),
        8 => ids!(header.scroller.content.right_cluster.render_controls.badge_box),
        9 => ids!(header.scroller.content.right_cluster.render_controls.spp_limit),
        10 => ids!(header.scroller.content.right_cluster.render_controls.stop_resume),
        11 => ids!(header.scroller.content.right_cluster.shading.shade_wire),
        12 => ids!(header.scroller.content.right_cluster.shading.shade_solid),
        13 => ids!(header.scroller.content.right_cluster.shading.shade_material),
        14 => ids!(header.scroller.content.right_cluster.shading.shade_realtime),
        15 => ids!(header.scroller.content.right_cluster.shading.shade_rendered),
        16 => ids!(header.scroller.content.right_cluster.shading.shade_ink),
        17 => ids!(header.scroller.content.right_cluster.xray),
        18 => ids!(header.scroller.content.right_cluster.overlays_btn),
        19 => ids!(header.scroller.content.right_cluster.gizmo_btn),
        _ => ids!(header.scroller.content.right_cluster.lock_btn),
    }
}

fn overflow_control_path(index: usize) -> &'static [LiveId] {
    match index {
        0 => ids!(body.overflow_popup_dock.overflow_popup.editor_type),
        1 => ids!(body.overflow_popup_dock.overflow_popup.mode),
        2 => ids!(body.overflow_popup_dock.overflow_popup.menu_view),
        3 => ids!(body.overflow_popup_dock.overflow_popup.menu_select),
        4 => ids!(body.overflow_popup_dock.overflow_popup.menu_object),
        5 => ids!(body.overflow_popup_dock.overflow_popup.menu_compact),
        6 => ids!(body.overflow_popup_dock.overflow_popup.time_row),
        7 => ids!(body.overflow_popup_dock.overflow_popup.haze_row),
        8 => ids!(body.overflow_popup_dock.overflow_popup.render_badge),
        9 => ids!(body.overflow_popup_dock.overflow_popup.spp_row),
        10 => ids!(body.overflow_popup_dock.overflow_popup.stop_resume),
        11 => ids!(body.overflow_popup_dock.overflow_popup.shade_wire),
        12 => ids!(body.overflow_popup_dock.overflow_popup.shade_solid),
        13 => ids!(body.overflow_popup_dock.overflow_popup.shade_material),
        14 => ids!(body.overflow_popup_dock.overflow_popup.shade_realtime),
        15 => ids!(body.overflow_popup_dock.overflow_popup.shade_rendered),
        16 => ids!(body.overflow_popup_dock.overflow_popup.shade_ink),
        17 => ids!(body.overflow_popup_dock.overflow_popup.xray),
        18 => ids!(body.overflow_popup_dock.overflow_popup.overlays),
        19 => ids!(body.overflow_popup_dock.overflow_popup.gizmo),
        _ => ids!(body.overflow_popup_dock.overflow_popup.lock),
    }
}

fn overflow_button_path(index: usize) -> &'static [LiveId] {
    overflow_control_path(index)
}

fn overflow_number_path(index: usize) -> &'static [LiveId] {
    match index {
        6 => ids!(body.overflow_popup_dock.overflow_popup.time_row.value),
        7 => ids!(body.overflow_popup_dock.overflow_popup.haze_row.value),
        _ => ids!(body.overflow_popup_dock.overflow_popup.spp_row.value),
    }
}

/// Drop order when the header runs out of room: least-reached-for first.
/// The menus go before the mode and editor dropdowns, those before Haze,
/// and the user's constant companions — Time and the shading strip — go
/// last. The right cluster rides a filler, so the survivors keep their
/// distance from the pane's right edge: a shading-mode switch changes
/// which *variable* controls sit in the middle, never where the icons are.
const HEADER_DROP_ORDER: [usize; HEADER_CONTROL_COUNT] = [
    2, 3, 4, // View / Select / Object menus
    5,  // the collapsed ☰ when it stands in for them
    1,  // interaction mode
    0,  // editor type
    7,  // haze
    8,  // render badge (information, not a control)
    9,  // spp limit
    10, // stop / resume
    20, // lock views
    19, 18, 17, // gizmo, overlays, x-ray
    6,  // time — outlasts everything above
    16, 15, 14, 13, 12, 11, // the shading strip goes last
];

/// The flow spacing a control costs on top of its own width: the clusters
/// space controls 4 apart, except inside the shading strip where the icons
/// sit 1 apart (the strip's first icon still pays the cluster gap).
fn header_control_gap(index: usize) -> f64 {
    if (12..=16).contains(&index) {
        1.0
    } else {
        4.0
    }
}

/// Which wanted controls move into the ⋯ overflow so the rest fit
/// `available`. Pure: cached widths in, dropped indexes (ascending) out.
/// [`header_control_gap`] covers the flow spacing per control and
/// `OVERHEAD` the separators and cluster gaps, so the fit test tracks the
/// real layout closely enough without consulting it.
fn header_priority_drops(
    widths: &[f64; HEADER_CONTROL_COUNT],
    wanted: &[bool; HEADER_CONTROL_COUNT],
    available: f64,
) -> Vec<usize> {
    const OVERHEAD: f64 = 36.0;
    let mut need: f64 = OVERHEAD
        + (0..HEADER_CONTROL_COUNT)
            .filter(|index| wanted[*index])
            .map(|index| widths[index] + header_control_gap(index))
            .sum::<f64>();
    let mut dropped: Vec<usize> = Vec::new();
    for &index in HEADER_DROP_ORDER.iter() {
        if need <= available {
            break;
        }
        if !wanted[index] || dropped.contains(&index) {
            continue;
        }
        // The three menus leave together — a lone surviving Object menu
        // reads as an accident, not a decision.
        let group: &[usize] = if (2..=4).contains(&index) {
            &[2, 3, 4]
        } else {
            std::slice::from_ref(&index)
        };
        for &g in group {
            if wanted[g] && !dropped.contains(&g) {
                need -= widths[g] + header_control_gap(g);
                dropped.push(g);
            }
        }
    }
    dropped.sort_unstable();
    dropped
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    // T toolbar: a 34 px column = 28 px buttons on a
    // 3 px pad, carrying the 20 px toolbar icon grid.
    let ToolButton = mod.widgets.FabToolToggle{
        margin: Inset{bottom: 2 top: 0 left: 0 right: 0}
    }

    // Header controls sit on the 16 px row icon grid in a 24x20 well.
    let ShadeButton = mod.widgets.FabIconToggle{
        width: 24
        height: fab.row_height
    }

    // Same look, but its own on/off — see FabIconCheck.
    let CheckButton = mod.widgets.FabIconCheck{
        width: 24
        height: fab.row_height
    }

    // Header numbers share the drag-number contract: drag to change, click
    // to type, arrows and Ctrl+Wheel to step.
    let HeaderNumber = mod.widgets.FabDragNumber{
        height: fab.row_height
        quantize: true
    }

    let SideRow = mod.widgets.FabPropRow{
        name +: { width: 78 }
    }
    let SideNum = mod.widgets.FabDragNumber{}

    // Popover rows are plain flat buttons: they live inside the overflow
    // popover, not behind the menu layer's grab, so they need none of the
    // popup-button open-state machinery `FabMenuButton` now carries.
    let OverflowButton = mod.widgets.FabFlatButton{
        width: Fill
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        label_walk: Walk{width: Fill height: Fit}
    }

    let OverflowNumberRow = View{
        width: Fill
        height: fab.row_height
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        spacing: 6
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        name := mod.widgets.FabLabelDim{width: 72 text: ""}
    }

    mod.widgets.FabViewportAreaBase = #(FabViewportArea::register_widget(vm))
    mod.widgets.FabViewportArea = set_type_default() do mod.widgets.FabViewportAreaBase{
        width: Fill
        height: Fill
        flow: Down
        view_index: 0
        header := mod.widgets.FabAreaHeader{
            flow: Overlay
            spacing: 0
            scroller := View{
                width: Fill
                height: Fill
                flow: Right
                clip_x: true
                clip_y: true
                margin: Inset{right: 24}
                content := View{
                    width: Fill
                    height: Fill
                    flow: Right
                    clip_x: false
                    align: Align{x: 0.0 y: 0.5}
                    spacing: 4
                    left_cluster := View{
                        width: Fit
                        height: Fill
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        spacing: 4
                        FabTip{ text: "Choose editor"
                            editor_type := mod.widgets.FabDropdownButton{ label +: { text: "3D Viewport" } }
                        }
                        FabTip{ text: "Change interaction mode"
                            mode := mod.widgets.FabDropdownButton{ tag: @vp_mode owner: @fab_vp_mode label +: { text: "Object Mode" } }
                        }
                        mod.widgets.FabVr{ height: Fill margin: Inset{left: 3 right: 3} }
                        menus := View{
                            width: Fit
                            height: Fill
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            FabTip{ text: "Open View menu"
                                menu_view := mod.widgets.FabMenuButton{ tag: @vp_menu_view owner: @fab_vp_view label +: { text: "View" } }
                            }
                            FabTip{ text: "Open Select menu"
                                menu_select := mod.widgets.FabMenuButton{ tag: @vp_menu_select owner: @fab_vp_select label +: { text: "Select" } }
                            }
                            FabTip{ text: "Open Object menu"
                                menu_object := mod.widgets.FabMenuButton{ tag: @vp_menu_object owner: @fab_vp_object label +: { text: "Object" } }
                            }
                        }
                        FabTip{ text: "Show header menus"
                            menu_compact := mod.widgets.FabMenuButton{
                                visible: false
                                width: 22
                                tag: @vp_menu_compact
                                owner: @fab_vp_compact
                                label +: { text: "☰" }
                                padding: Inset{left: 4 right: 4 top: 0 bottom: 0}
                            }
                        }
                    }
                    Filler{}
                    right_cluster := View{
                        width: Fit
                        height: Fill
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        spacing: 4
                        render_controls := View{
                            visible: false
                            width: Fit
                            height: Fill
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            spacing: 4
                            // A fixed-width box around the badge: the text
                            // varies per frame ("converging · 12345 spp"),
                            // the fit pass needs one stable measure, and a
                            // bare Label's area under-reports its rect.
                            badge_box := View{
                                width: 112
                                height: Fill
                                align: Align{x: 0.0 y: 0.5}
                                render_badge := mod.widgets.FabLabelDim{ width: Fill height: Fit text: "converging · 0 spp" }
                            }
                            FabTip{ text: "SPP limit (64–8192)"
                                spp_limit := mod.widgets.FabDragNumber{
                                    width: 94
                                    height: fab.row_height
                                    label: "spp limit"
                                    min: 64.0
                                    max: 8192.0
                                    step: 16.0
                                    snap: 64.0
                                    precision: 0
                                    value: 1024.0
                                    padding: Inset{left: 5 right: 5 top: 0 bottom: 0}
                                }
                            }
                            FabTip{ text: "Stop / Resume path tracer (P)"
                                stop_resume := mod.widgets.FabIconButton{
                                    width: 20
                                    height: fab.row_height
                                    padding: Inset{left: 2 right: 2 top: 2 bottom: 2}
                                    draw_icon +: { svg: crate_resource("self://resources/icons/pause.svg") }
                                }
                            }
                        }
                        shading := View{
                            width: Fit
                            height: Fill
                            flow: Right
                            spacing: 1
                            FabTip{ text: "Use wireframe shading (Z)"
                                shade_wire := ShadeButton{ draw_icon +: { svg: crate_resource("self://resources/icons/wireframe.svg") } }
                            }
                            FabTip{ text: "Use solid shading (Z)"
                                shade_solid := ShadeButton{ draw_icon +: { svg: crate_resource("self://resources/icons/solid.svg") } }
                            }
                            FabTip{ text: "Material (Z)"
                                shade_material := ShadeButton{ draw_icon +: { svg: crate_resource("self://resources/icons/material.svg") } }
                            }
                            FabTip{ text: "Realtime — PBR, textures, sun, sky and shadows (Z)"
                                shade_realtime := ShadeButton{ draw_icon +: { svg: crate_resource("self://resources/icons/realtime.svg") } }
                            }
                            FabTip{ text: "Raytraced (Z)"
                                shade_rendered := ShadeButton{ draw_icon +: { svg: crate_resource("self://resources/icons/rendered.svg") } }
                            }
                            FabTip{ text: "Show hidden lines (Z)"
                                shade_ink := ShadeButton{ draw_icon +: { svg: crate_resource("self://resources/icons/hidden_line.svg") } }
                            }
                        }
                        FabTip{ text: "Toggle X-ray (Alt+Z)"
                            xray := CheckButton{
                                margin: Inset{left: 4 right: 0 top: 0 bottom: 0}
                                draw_icon +: { svg: crate_resource("self://resources/icons/xray.svg") }
                            }
                        }
                        FabTip{ text: "Configure overlays"
                            overlays_btn := mod.widgets.FabDropdownButton{
                                tag: @vp_overlays
                                owner: @fab_vp_overlays
                                padding: Inset{left: 3 right: 2 top: 0 bottom: 0}
                                label +: { text: "" }
                                ico_slot := View{
                                    width: fab.icon_size
                                    height: Fill
                                    padding: Inset{top: 2 bottom: 2 left: 0 right: 0}
                                    ico := mod.widgets.FabIconDim{
                                        width: fab.icon_size
                                        height: fab.icon_size
                                        icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
                                        draw_icon +: { svg: crate_resource("self://resources/icons/overlays.svg") }
                                    }
                                }
                            }
                        }
                        FabTip{ text: "Choose view preset (Numpad 1/3/7/9)"
                            gizmo_btn := mod.widgets.FabDropdownButton{
                                tag: @vp_gizmo
                                owner: @fab_vp_gizmo
                                padding: Inset{left: 3 right: 2 top: 0 bottom: 0}
                                label +: { text: "" }
                                ico_slot := View{
                                    width: fab.icon_size
                                    height: Fill
                                    padding: Inset{top: 2 bottom: 2 left: 0 right: 0}
                                    ico := mod.widgets.FabIconDim{
                                        width: fab.icon_size
                                        height: fab.icon_size
                                        icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
                                        draw_icon +: { svg: crate_resource("self://resources/icons/persp.svg") }
                                    }
                                }
                            }
                        }
                        mod.widgets.FabVr{ height: Fill margin: Inset{left: 3 right: 3} }
                        FabTip{ text: "Lock Views — orbit and pan move both panes together"
                            lock_btn := CheckButton{ draw_icon +: { svg: crate_resource("self://resources/icons/lock.svg") } }
                        }
                    }
                }
            }
            overflow_pin := View{
                width: Fill
                height: Fill
                flow: Right
                align: Align{x: 0.0 y: 0.5}
                Filler{}
                FabTip{ text: "Show controls outside the visible header"
                    overflow := mod.widgets.FabMenuButton{
                        visible: false
                        width: 22
                        tag: @vp_header_overflow
                        owner: @fab_vp_header_overflow
                        label +: { text: "⋯" }
                        padding: Inset{left: 4 right: 4 top: 0 bottom: 0}
                    }
                }
            }
        }
        body := View{
            width: Fill
            height: Fill
            flow: Overlay
            viewport := FabViewport{ view: 0 }
            tool_overlay := FabToolOverlay{ view: 0 }
            overlay_text := View{
                width: Fit
                height: Fit
                flow: Down
                margin: Inset{left: 46 top: 8}
                spacing: 1
                view_label := Label{
                    draw_text +: {
                        ink_centered: true
                        color: fab.color_vp_text
                        text_style: theme.font_regular{ font_size: fab.font_size_vp }
                    }
                    text: "User Perspective"
                }
                context_label := Label{
                    draw_text +: {
                        ink_centered: true
                        color: fab.color_vp_text
                        text_style: theme.font_regular{ font_size: fab.font_size_vp }
                    }
                    text: ""
                }
            }
            tool_stack := View{
                width: Fit
                height: Fit
                flow: Right
                align: Align{x: 0.0 y: 0.0}
                margin: Inset{left: 6 top: 6}
                toolbar := View{
                    width: Fit
                    height: Fit
                    flow: Down
                    padding: 3
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                            sdf.fill_keep(fab.color_float)
                            sdf.stroke(fab.color_float_border, 1.0)
                            return sdf.result
                        }
                    }
                    FabTip{ text: "Select objects"
                        tool_select := ToolButton{ draw_icon +: { svg: crate_resource("self://resources/icons/select.svg") } }
                    }
                    FabTip{ text: "Measure geometry (M)"
                        tool_measure := ToolButton{ draw_icon +: { svg: crate_resource("self://resources/icons/measure.svg") } }
                    }
                    FabTip{ text: "Cut section (C)"
                        tool_section := ToolButton{ draw_icon +: { svg: crate_resource("self://resources/icons/section.svg") } }
                    }
                    FabTip{ text: "Walk mode (W)"
                        tool_walk := ToolButton{
                            margin: Inset{bottom: 0 top: 0 left: 0 right: 0}
                            draw_icon +: { svg: crate_resource("self://resources/icons/walk.svg") }
                        }
                    }
                }
                measure_fly := View{
                    visible: false
                    width: Fit
                    height: Fit
                    flow: Down
                    margin: Inset{left: 4 top: 33}
                    padding: 3
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                            sdf.fill_keep(fab.color_float)
                            sdf.stroke(fab.color_float_border, 1.0)
                            return sdf.result
                        }
                    }
                    FabTip{ text: "Measure distance"
                        fly_dist := ToolButton{ draw_icon +: { svg: crate_resource("self://resources/icons/measure.svg") } }
                    }
                    FabTip{ text: "Measure area"
                        fly_area := ToolButton{ draw_icon +: { svg: crate_resource("self://resources/icons/measure_area.svg") } }
                    }
                    FabTip{ text: "Measure angle"
                        fly_angle := ToolButton{
                            margin: Inset{bottom: 0 top: 0 left: 0 right: 0}
                            draw_icon +: { svg: crate_resource("self://resources/icons/measure_angle.svg") }
                        }
                    }
                }
            }
            gizmo_dock := View{
                width: Fill
                height: Fit
                flow: Right
                margin: Inset{top: 6 right: 8}
                Filler{}
                gizmo := FabNavGizmo{ view: 0 }
            }
            sidebar_dock := View{
                width: Fill
                height: Fill
                flow: Right
                Filler{}
                // The sidebar's inner edge is a drag handle: pull it to
                // resize the panel; the width holds for the session.
                sidebar_grip := View{
                    visible: false
                    width: 6
                    height: Fill
                    cursor: MouseCursor.EwResize
                    show_bg: true
                    draw_bg +: {
                        hover: instance(0.0)
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(self.rect_size.x - 2.0, 0.0, 2.0, self.rect_size.y)
                            sdf.fill(vec4(fab.color_accent.xyz, self.hover * 0.9))
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
                }
                sidebar := View{
                    visible: false
                    width: fab.sidebar_width
                    height: Fill
                    flow: Down
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            sdf.fill(fab.color_float)
                            sdf.rect(0.0, 0.0, 1.0, self.rect_size.y)
                            sdf.fill(fab.color_border)
                            return sdf.result
                        }
                    }
                    tabs := View{
                        width: Fill
                        height: 24
                        flow: Right
                        align: Align{x: 0.0 y: 0.0}
                        padding: Inset{left: 4 right: 4 top: 2 bottom: 2}
                        spacing: 2
                        FabTip{ text: "Show item settings (N)"
                            tab_item := mod.widgets.FabSegmentTab{ text: "Item" }
                        }
                        FabTip{ text: "Show view settings (N)"
                            tab_view := mod.widgets.FabSegmentTab{ text: "View" }
                        }
                        FabTip{ text: "Show tool settings (N)"
                            tab_tool := mod.widgets.FabSegmentTab{ text: "Tool" }
                        }
                    }
                    mod.widgets.FabHr{}
                    page_item := mod.widgets.FabScroll{
                        padding: Inset{left: 3 right: 3 top: 3 bottom: 6}
                        item_panel := mod.widgets.FabPanel{
                            header +: { hdr +: { title +: { text: "Active Element" } } }
                            body +: {
                                width: Fill height: Fit flow: Down
                                padding: Inset{left: 0 right: 0 top: 2 bottom: 4}
                                si_name := SideRow{ name +: { text: "Name" } }
                                si_type := SideRow{ name +: { text: "Type" } }
                                si_story := SideRow{ name +: { text: "Story" } }
                                si_size := SideRow{ name +: { text: "Size" } }
                                si_count := SideRow{ name +: { text: "Selected" } }
                            }
                        }
                    }
                    page_view := mod.widgets.FabScroll{
                        visible: false
                        padding: Inset{left: 3 right: 3 top: 3 bottom: 6}
                        view_panel := mod.widgets.FabPanel{
                            header +: { hdr +: { title +: { text: "View" } } }
                            body +: {
                                width: Fill height: Fit flow: Down
                                padding: Inset{left: 6 right: 4 top: 2 bottom: 4}
                                spacing: 2
                                num_fov := SideNum{ label: "Focal" min: 8.0 max: 120.0 step: 0.15 snap: 5.0 precision: 1 suffix: "°" }
                                num_near := SideNum{ label: "Clip start" min: 0.001 max: 10.0 step: 0.004 snap: 0.1 precision: 3 }
                                num_far := SideNum{ label: "Clip end" min: 1.0 max: 20000.0 step: 4.0 snap: 100.0 precision: 0 }
                                cb_ortho := mod.widgets.FabCheckBox{ text: "Orthographic" }
                                cb_lock := mod.widgets.FabCheckBox{ text: "Lock views" }
                            }
                        }
                        overlay_panel := mod.widgets.FabPanel{
                            header +: { hdr +: { title +: { text: "Overlays" } } }
                            body +: {
                                width: Fill height: Fit flow: Down
                                padding: Inset{left: 6 right: 4 top: 2 bottom: 4}
                                spacing: 2
                                cb_grid := mod.widgets.FabCheckBox{ text: "Grid" }
                                cb_outlines := mod.widgets.FabCheckBox{ text: "Outlines" }
                                cb_wire := mod.widgets.FabCheckBox{ text: "Wire on shaded" }
                                cb_text := mod.widgets.FabCheckBox{ text: "Text info" }
                            }
                        }
                    }
                    page_tool := mod.widgets.FabScroll{
                        visible: false
                        padding: Inset{left: 3 right: 3 top: 3 bottom: 6}
                        // Lane E owns the Tool tab contents; every control
                        // there already emits a `ShellAction`.
                        tools := FabToolPanel{}
                    }
                }
            }
            overflow_popup_dock := View{
                width: Fill
                height: Fill
                flow: Right
                align: Align{x: 1.0 y: 0.0}
                padding: Inset{right: 2 top: 2}
                overflow_popup := View{
                    visible: false
                    width: 224
                    height: Fit
                    flow: Down
                    padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                            sdf.fill_keep(fab.color_popover)
                            sdf.stroke(fab.color_popover_border, 1.0)
                            return sdf.result
                        }
                    }
                    editor_type := OverflowButton{text: "Editor Type…"}
                    mode := OverflowButton{text: "Interaction Mode…"}
                    menu_view := OverflowButton{text: "View"}
                    menu_select := OverflowButton{text: "Select"}
                    menu_object := OverflowButton{text: "Object"}
                    menu_compact := OverflowButton{text: "Menus"}
                    time_row := OverflowNumberRow{
                        name +: {text: "Time"}
                        value := HeaderNumber{
                            width: Fill
                            label: ""
                            min: 0.0
                            max: 24.0
                            step: 0.25
                            wrap: true
                            show_fill: true
                            time_of_day: true
                            text_input +: {is_numeric_only: false}
                        }
                    }
                    haze_row := OverflowNumberRow{
                        name +: {text: "Haze"}
                        value := HeaderNumber{
                            width: Fill
                            label: ""
                            min: 0.0
                            max: 1.0
                            step: 0.05
                            precision: 2
                            show_fill: true
                        }
                    }
                    render_badge := mod.widgets.FabLabelDim{
                        width: Fill
                        height: fab.row_height
                        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
                        text: "converging · 0 spp"
                    }
                    spp_row := OverflowNumberRow{
                        name +: {text: "SPP limit"}
                        value := HeaderNumber{
                            width: Fill
                            label: ""
                            min: 64.0
                            max: 8192.0
                            step: 16.0
                            snap: 64.0
                            precision: 0
                            value: 1024.0
                        }
                    }
                    stop_resume := OverflowButton{text: "Stop rendering"}
                    shade_wire := OverflowButton{text: "Wireframe"}
                    shade_solid := OverflowButton{text: "Solid"}
                    shade_material := OverflowButton{text: "Material"}
                    shade_realtime := OverflowButton{text: "Realtime"}
                    shade_rendered := OverflowButton{text: "Raytraced"}
                    shade_ink := OverflowButton{text: "Hidden Line"}
                    xray := OverflowButton{text: "X-Ray"}
                    overlays := OverflowButton{text: "Overlays…"}
                    gizmo := OverflowButton{text: "View Preset…"}
                    lock := OverflowButton{text: "Lock Views"}
                }
            }
        }
    }
}

const VIEW_MENU: LiveId = live_id!(fab_vp_view);
const SELECT_MENU: LiveId = live_id!(fab_vp_select);
const OBJECT_MENU: LiveId = live_id!(fab_vp_object);
const MODE_MENU: LiveId = live_id!(fab_vp_mode);
const OVERLAY_MENU: LiveId = live_id!(fab_vp_overlays);
const GIZMO_MENU: LiveId = live_id!(fab_vp_gizmo);
const CONTEXT_MENU: LiveId = live_id!(fab_vp_context);
/// Owner of the Z shading pie.
pub const PIE_OWNER: LiveId = live_id!(fab_vp_pie);

const SIDE_PAGES: [(&[LiveId], &[LiveId]); 3] = [
    (
        ids!(body.sidebar_dock.sidebar.tabs.tab_item),
        ids!(body.sidebar_dock.sidebar.page_item),
    ),
    (
        ids!(body.sidebar_dock.sidebar.tabs.tab_view),
        ids!(body.sidebar_dock.sidebar.page_view),
    ),
    (
        ids!(body.sidebar_dock.sidebar.tabs.tab_tool),
        ids!(body.sidebar_dock.sidebar.page_tool),
    ),
];

const SIDE_PANELS: [&[LiveId]; 3] = [
    ids!(body.sidebar_dock.sidebar.page_item.item_panel),
    ids!(body.sidebar_dock.sidebar.page_view.view_panel),
    ids!(body.sidebar_dock.sidebar.page_view.overlay_panel),
];

fn shading_id(s: Shading) -> LiveId {
    match s {
        Shading::Wireframe => live_id!(sh_wire),
        Shading::Solid => live_id!(sh_solid),
        Shading::Material => live_id!(sh_material),
        Shading::Realtime => live_id!(sh_realtime),
        Shading::Rendered => live_id!(sh_rendered),
        Shading::HiddenLine => live_id!(sh_ink),
    }
}

fn shading_from_id(id: LiveId) -> Option<Shading> {
    Shading::ALL.iter().copied().find(|s| shading_id(*s) == id)
}

fn preset_id(p: PresetView) -> LiveId {
    match p {
        PresetView::Front => live_id!(pv_front),
        PresetView::Back => live_id!(pv_back),
        PresetView::Right => live_id!(pv_right),
        PresetView::Left => live_id!(pv_left),
        PresetView::Top => live_id!(pv_top),
        PresetView::Bottom => live_id!(pv_bottom),
        PresetView::Isometric => live_id!(pv_iso),
    }
}

const PRESETS: [PresetView; 7] = [
    PresetView::Front,
    PresetView::Back,
    PresetView::Right,
    PresetView::Left,
    PresetView::Top,
    PresetView::Bottom,
    PresetView::Isometric,
];

/// Raise the Z shading pie centred on `at`.
pub fn open_shading_pie(cx: &mut Cx, at: Vec2d, current: Shading) {
    let items = Shading::ALL
        .iter()
        .map(|s| PieItem {
            id: shading_id(*s),
            label: s.label().to_string(),
            active: *s == current,
        })
        .collect();
    cx.action(FabUiAction::OpenPie {
        owner: PIE_OWNER,
        at,
        items,
    });
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabViewportArea {
    #[deref]
    view: View,
    /// Index into `AppState::views`; pushed into the child viewport, tool
    /// overlay and gizmo on first draw.
    #[live(0)]
    view_index: usize,
    #[rust]
    synced: Option<(Shading, bool, Tool, bool, NavMode, bool)>,
    #[rust(false)]
    children_indexed: bool,
    #[rust]
    side_tab: usize,
    #[rust]
    synced_side: Option<(usize, bool)>,
    #[rust]
    synced_nums: Option<u64>,
    #[rust]
    render_icon_paused: Option<bool>,
    #[rust]
    fly_open: bool,
    #[rust]
    sidebar_t: f32,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_t: f64,
    #[rust(true)]
    show_menus: bool,
    #[rust]
    header_pan: f64,
    #[rust]
    header_content_width: f64,
    #[rust]
    header_visible_width: f64,
    #[rust]
    header_pan_drag: Option<(f64, f64)>,
    #[rust]
    clipped_header_controls: Vec<usize>,
    #[rust]
    overflow_open: bool,
    /// Last measured width of each header control, kept while a control is
    /// dropped so the fit pass can decide when it earns its place back.
    #[rust([0.0; HEADER_CONTROL_COUNT])]
    header_control_width: [f64; HEADER_CONTROL_COUNT],
    /// The N sidebar's width for this pane — the default matches the theme's
    /// `fab.sidebar_width`; dragging the sidebar grip changes it and the
    /// choice holds for the session.
    #[rust(260.0)]
    sidebar_user_width: f64,
    /// An in-flight sidebar-grip drag: (pointer x at press, width then).
    #[rust]
    sidebar_drag: Option<(f64, f64)>,
}

impl FabViewportArea {
    /// One menu identity per pane: the open look and the pick belong to the
    /// pane that raised the menu, so the same control in another pane stays
    /// idle — and a pick is applied once, by that pane, not by every pane
    /// listening on a shared owner. Scoped by the widget instance (not the
    /// view index: two areas can host viewports of the same view).
    fn pane(&self, base: LiveId) -> LiveId {
        base.bytes_append(&self.view.widget_uid().0.to_le_bytes())
    }

    fn view_menu(state: &AppState, v: usize) -> Vec<MenuItem> {
        let vs = state.view_at(v);
        vec![
            MenuItem::new(live_id!(frame_all), "Frame All").key("Home"),
            MenuItem::new(live_id!(frame_sel), "Frame Selected")
                .key(".")
                .enabled_if(!state.scene_state.selection.is_empty()),
            MenuItem::sep(),
            MenuItem::new(live_id!(toggle_ortho), "Orthographic")
                .key("Numpad 5")
                .checked(vs.camera.ortho),
            MenuItem::new(live_id!(sidebar), "Sidebar")
                .key("N")
                .checked(state.ui.sidebar_open),
            MenuItem::sep(),
            MenuItem::new(live_id!(lock), "Lock Views").checked(state.ui.lock_views),
        ]
    }

    fn select_menu(state: &AppState) -> Vec<MenuItem> {
        let any = !state.scene_state.selection.is_empty();
        vec![
            MenuItem::new(live_id!(select_all), "Select All").key("A"),
            MenuItem::new(live_id!(select_none), "Select None")
                .key("Alt+A")
                .enabled_if(any),
            MenuItem::new(live_id!(invert), "Invert Selection").key("Ctrl+I"),
        ]
    }

    fn object_menu(state: &AppState) -> Vec<MenuItem> {
        let any = !state.scene_state.selection.is_empty();
        vec![
            MenuItem::new(live_id!(hide), "Hide Selected")
                .key("H")
                .enabled_if(any),
            MenuItem::new(live_id!(isolate), "Isolate Selected")
                .key("Shift+H")
                .enabled_if(any),
            MenuItem::new(live_id!(unhide), "Unhide All").key("Alt+H"),
            MenuItem::sep(),
            MenuItem::new(live_id!(reveal), "Reveal in Outliner").enabled_if(any),
            MenuItem::new(live_id!(clear_measure), "Clear Measurements")
                .enabled_if(!state.measurements.is_empty()),
        ]
    }

    fn overlay_menu(o: &Overlays) -> Vec<MenuItem> {
        vec![
            MenuItem::new(live_id!(ov_grid), "Grid").checked(o.grid),
            MenuItem::new(live_id!(ov_axes), "Axes").checked(o.axes),
            MenuItem::new(live_id!(ov_outlines), "Selection Outline").checked(o.outlines),
            MenuItem::new(live_id!(ov_wire), "Wireframe on Shaded").checked(o.wire_on_shaded),
            MenuItem::sep(),
            MenuItem::new(live_id!(ov_cavity), "Cavity").checked(o.cavity),
            MenuItem::new(live_id!(ov_ssao), "Ambient Occlusion").checked(o.ssao),
            // Shadows / Depth of Field / Section Caps / Pivot Marker rows were
            // removed: no renderer path answers them yet (no dead controls).
            MenuItem::sep(),
            MenuItem::new(live_id!(ov_sections), "Section Planes").checked(o.section_planes),
            MenuItem::new(live_id!(ov_measure), "Measurements").checked(o.measurements),
            MenuItem::new(live_id!(ov_text), "Text Info").checked(o.text_info),
            MenuItem::new(live_id!(ov_gizmo), "Navigation Gizmo").checked(o.nav_gizmo),
        ]
    }

    fn gizmo_menu(vs: &ViewportState) -> Vec<MenuItem> {
        let mut items: Vec<MenuItem> = PRESETS
            .iter()
            .map(|p| {
                MenuItem::new(preset_id(*p), p.label())
                    .radio(vs.preset == Some(*p))
                    .key(match p {
                        PresetView::Front => "Numpad 1",
                        PresetView::Right => "Numpad 3",
                        PresetView::Top => "Numpad 7",
                        PresetView::Isometric => "Numpad 9",
                        _ => "",
                    })
            })
            .collect();
        items.push(MenuItem::sep());
        items.push(
            MenuItem::new(live_id!(toggle_ortho), "Orthographic")
                .key("Numpad 5")
                .checked(vs.camera.ortho),
        );
        items
    }

    fn mode_menu(vs: &ViewportState) -> Vec<MenuItem> {
        vec![
            MenuItem::new(live_id!(m_object), "Object Mode").radio(vs.nav_mode == NavMode::Orbit),
            MenuItem::new(live_id!(m_walk), "Walk Mode")
                .radio(vs.nav_mode == NavMode::Walk)
                .key("Shift+`"),
            MenuItem::new(live_id!(m_fly), "Fly Mode").radio(vs.nav_mode == NavMode::Fly),
        ]
    }

    fn context_menu(state: &AppState) -> Vec<MenuItem> {
        let any = !state.scene_state.selection.is_empty();
        vec![
            MenuItem::new(live_id!(frame_sel), "Frame Selected")
                .key(".")
                .enabled_if(any),
            MenuItem::new(live_id!(frame_all), "Frame All").key("Home"),
            MenuItem::sep(),
            MenuItem::new(live_id!(hide), "Hide").key("H").enabled_if(any),
            MenuItem::new(live_id!(isolate), "Isolate")
                .key("Shift+H")
                .enabled_if(any),
            MenuItem::new(live_id!(unhide), "Unhide All").key("Alt+H"),
            MenuItem::sep(),
            MenuItem::new(live_id!(reveal), "Reveal in Outliner").enabled_if(any),
            MenuItem::new(live_id!(select_none), "Deselect")
                .key("Alt+A")
                .enabled_if(any),
        ]
    }

    fn header_context_menu(&self) -> Vec<MenuItem> {
        vec![MenuItem::new(live_id!(show_menus), "Show Menus").checked(self.show_menus)]
    }

    fn compact_menu(state: &AppState, v: usize) -> Vec<MenuItem> {
        vec![
            MenuItem::new(LiveId(COMPACT_PARENT_BASE), "View")
                .flyout(Self::view_menu(state, v)),
            MenuItem::new(LiveId(COMPACT_PARENT_BASE + 1), "Select")
                .flyout(Self::select_menu(state)),
            MenuItem::new(LiveId(COMPACT_PARENT_BASE + 2), "Object")
                .flyout(Self::object_menu(state)),
        ]
    }

    fn header_control_is_visible(&self, cx: &mut Cx, index: usize) -> bool {
        match index {
            2..=4 => self.show_menus,
            5 => !self.show_menus,
            6 | 7 => self
                .view
                .widget(
                    cx,
                    ids!(header.scroller.content.right_cluster.sun_controls),
                )
                .visible(),
            8..=10 => self
                .view
                .widget(
                    cx,
                    ids!(header.scroller.content.right_cluster.render_controls),
                )
                .visible(),
            _ => true,
        }
    }

    fn sync_header_menu_visibility(&mut self, cx: &mut Cx) {
        self.view
            .widget(cx, ids!(header.scroller.content.left_cluster.menus))
            .set_visible(cx, self.show_menus);
        self.view
            .widget(cx, header_control_path(5))
            .set_visible(cx, !self.show_menus);
        self.view.redraw(cx);
    }

    fn set_header_pan(&mut self, cx: &mut Cx, pan: f64) -> bool {
        let pan = clamp_header_pan(
            pan,
            self.header_content_width,
            self.header_visible_width,
        );
        if (pan - self.header_pan).abs() <= f64::EPSILON {
            return false;
        }
        self.header_pan = pan;
        self.view
            .view(cx, ids!(header.scroller))
            .set_scroll_pos(cx, dvec2(pan, 0.0));
        self.view.redraw(cx);
        true
    }

    fn update_header_metrics(&mut self, cx: &mut Cx) {
        let viewport = self
            .view
            .view(cx, ids!(header.scroller))
            .area()
            .rect(cx);
        if viewport.size.x <= 0.0 {
            return;
        }

        // What the pane's state wants in the header right now. Applicability
        // (menus shown, sun and render controls per shading mode) is separate
        // from fitting: a wanted control that does not fit is dropped and
        // gets an overflow row; an inapplicable one has neither.
        let mut wanted = [false; HEADER_CONTROL_COUNT];
        for index in 0..HEADER_CONTROL_COUNT {
            wanted[index] = self.header_control_is_visible(cx, index);
        }

        // Refresh the width cache from whatever is actually drawn. A control
        // hidden by the drop pass keeps its last measured width — that is
        // what the next fit decision uses to consider bringing it back. A
        // control that just became wanted and was never measured is shown
        // for one frame so the cache can learn its width.
        let mut unmeasured = false;
        for index in 0..HEADER_CONTROL_COUNT {
            if !wanted[index] {
                continue;
            }
            let control = self.view.widget(cx, header_control_path(index));
            let width = control.area().rect(cx).size.x;
            if width > 0.1 {
                self.header_control_width[index] = width;
            } else if self.header_control_width[index] <= 0.0 && !control.visible() {
                control.set_visible(cx, true);
                unmeasured = true;
            }
        }
        if unmeasured {
            self.view.redraw(cx);
            return;
        }

        let dropped =
            header_priority_drops(&self.header_control_width, &wanted, viewport.size.x);
        let mut changed = false;
        for index in 0..HEADER_CONTROL_COUNT {
            if !wanted[index] {
                continue;
            }
            let control = self.view.widget(cx, header_control_path(index));
            let keep = !dropped.contains(&index);
            if control.visible() != keep {
                control.set_visible(cx, keep);
                changed = true;
            }
        }
        if changed {
            self.view.redraw(cx);
        }

        // The drop pass guarantees the survivors fit, so the pan idles at
        // zero; the clamp still guards the one frame where they do not yet.
        self.header_visible_width = viewport.size.x;
        self.header_content_width = viewport.size.x;
        let clamped = clamp_header_pan(
            self.header_pan,
            self.header_content_width,
            self.header_visible_width,
        );
        if (clamped - self.header_pan).abs() > f64::EPSILON {
            self.header_pan = clamped;
            self.view
                .view(cx, ids!(header.scroller))
                .set_scroll_pos(cx, dvec2(clamped, 0.0));
            self.view.redraw(cx);
        }

        if dropped != self.clipped_header_controls {
            self.clipped_header_controls = dropped;
            if self.clipped_header_controls.is_empty() {
                self.set_overflow_open(cx, false);
            }
            self.sync_overflow_visibility(cx);
        }
    }

    fn sync_overflow_visibility(&mut self, cx: &mut Cx) {
        let has_overflow = !self.clipped_header_controls.is_empty();
        self.view
            .widget(cx, ids!(header.overflow_pin.overflow))
            .set_visible(cx, has_overflow);
        for index in 0..HEADER_CONTROL_COUNT {
            self.view
                .widget(cx, overflow_control_path(index))
                .set_visible(cx, self.clipped_header_controls.contains(&index));
        }
        self.view
            .widget(cx, ids!(body.overflow_popup_dock.overflow_popup))
            .set_visible(cx, self.overflow_open && has_overflow);
        self.view.redraw(cx);
    }

    /// Every open/close of the overflow popover funnels through here — the
    /// same single-writer discipline the menu layer's `set_open` gives the
    /// menus. The ⋯ button's "open" look mirrors this state directly (its
    /// popover is not menu-layer-owned, so no `MenuOpened` broadcast carries
    /// its owner), and every close path — a pick, Escape, a press outside,
    /// a menu opening over it, focus loss, the clipped set emptying — lands
    /// here, so the look cannot linger.
    fn set_overflow_open(&mut self, cx: &mut Cx, open: bool) {
        if self.overflow_open != open {
            self.overflow_open = open;
            self.view
                .fab_dropdown_button(cx, ids!(header.overflow_pin.overflow))
                .set_open(cx, open);
            self.sync_overflow_visibility(cx);
        }
    }

    fn apply_common(&self, cx: &mut Cx, state: &AppState, v: usize, pick: LiveId) {
        match pick {
            x if x == live_id!(frame_all) => cx.action(ShellAction::FrameAll(v)),
            x if x == live_id!(frame_sel) => cx.action(ShellAction::FrameSelected(v)),
            x if x == live_id!(toggle_ortho) => cx.action(ShellAction::ToggleOrtho(v)),
            x if x == live_id!(sidebar) => cx.action(ShellAction::ToggleSidebar),
            x if x == live_id!(lock) => cx.action(ShellAction::ToggleLockViews),
            x if x == live_id!(hide) => cx.action(ShellAction::HideSelected),
            x if x == live_id!(isolate) => cx.action(ShellAction::IsolateSelected),
            x if x == live_id!(unhide) => cx.action(ShellAction::UnhideAll),
            x if x == live_id!(clear_measure) => cx.action(ShellAction::ClearMeasurements),
            x if x == live_id!(select_none) => cx.action(ShellAction::ClearSelection),
            x if x == live_id!(select_all) => {
                let ids: Vec<ElementId> = state
                    .scene
                    .elements
                    .iter()
                    .filter(|e| e.has_geometry())
                    .map(|e| e.id)
                    .collect();
                cx.action(ShellAction::SelectSet(ids));
            }
            x if x == live_id!(invert) => {
                let ids: Vec<ElementId> = state
                    .scene
                    .elements
                    .iter()
                    .filter(|e| e.has_geometry() && !state.scene_state.selection.contains(e.id))
                    .map(|e| e.id)
                    .collect();
                cx.action(ShellAction::SelectSet(ids));
            }
            x if x == live_id!(reveal) => {
                if let Some(id) = state.scene_state.selection.active {
                    cx.action(ShellAction::RevealInOutliner(id));
                }
            }
            _ => {}
        }
    }

    fn measure_fly_hot(&self, cx: &mut Cx, p: Vec2d) -> bool {
        let measure = self
            .view
            .widget(cx, ids!(body.tool_stack.toolbar.tool_measure))
            .area()
            .rect(cx);
        if measure.size.x < 1.0 {
            return false;
        }
        if measure.contains(p) {
            return true;
        }
        if !self.fly_open {
            return false;
        }
        self.view
            .widget(cx, ids!(body.tool_stack.measure_fly))
            .area()
            .rect(cx)
            .contains(p)
    }
}

impl Widget for FabViewportArea {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let header_viewport = self
            .view
            .view(cx, ids!(header.scroller))
            .area()
            .rect(cx);
        match event {
            Event::Scroll(e) if header_viewport.contains(e.abs) => {
                let delta = e.scroll.x + e.scroll.y;
                if !e.handled_x.get()
                    && !e.handled_y.get()
                    && delta.abs() > f64::EPSILON
                    && self.set_header_pan(cx, self.header_pan + delta)
                {
                    e.handled_x.set(true);
                    e.handled_y.set(true);
                }
            }
            Event::MouseDown(e) if e.button.is_middle() && header_viewport.contains(e.abs) => {
                self.header_pan_drag = Some((e.abs.x, self.header_pan));
            }
            Event::MouseDown(e) if e.button.is_secondary() && header_viewport.contains(e.abs) => {
                open_menu(
                    cx,
                    self.pane(HEADER_CONTEXT_OWNER),
                    self.header_context_menu(),
                    Rect {
                        pos: e.abs,
                        size: dvec2(0.0, 0.0),
                    },
                    MenuPlace::At,
                );
            }
            Event::MouseMove(e) => {
                if let Some((start_x, start_pan)) = self.header_pan_drag {
                    self.set_header_pan(cx, start_pan - (e.abs.x - start_x));
                }
            }
            Event::MouseUp(e) if e.button.is_middle() => self.header_pan_drag = None,
            _ => {}
        }
        if self.overflow_open {
            match event {
                Event::MouseDown(e) => {
                    let popup = self
                        .view
                        .view(cx, ids!(body.overflow_popup_dock.overflow_popup))
                        .area()
                        .rect(cx);
                    let button = self
                        .view
                        .widget(cx, ids!(header.overflow_pin.overflow))
                        .area()
                        .rect(cx);
                    if !popup.contains(e.abs) && !button.contains(e.abs) {
                        self.set_overflow_open(cx, false);
                    }
                }
                Event::KeyDown(e) if e.key_code == KeyCode::Escape => {
                    self.set_overflow_open(cx, false);
                }
                // The popover cannot outlive its window's focus, any more
                // than a menu can.
                Event::WindowLostFocus(_) | Event::Pause | Event::Background => {
                    self.set_overflow_open(cx, false);
                }
                _ => {}
            }
        }
        self.view.handle_event(cx, event, scope);
        if self.next_frame.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        if let Event::MouseMove(e) = event {
            let want = self.measure_fly_hot(cx, e.abs);
            if want != self.fly_open {
                self.fly_open = want;
                self.view
                    .widget(cx, ids!(body.tool_stack.measure_fly))
                    .set_visible(cx, want);
                self.view.redraw(cx);
            }
        }
        let Event::Actions(actions) = event else {
            return;
        };
        let v = self.view_index;

        // Only one popup is ever up: a menu opening anywhere — another
        // pane's, the menubar's, even one raised from this popover's own
        // rows — takes the overflow popover down with it.
        if self.overflow_open
            && ui_actions(actions).any(|a| matches!(a, FabUiAction::MenuOpened { .. }))
        {
            self.set_overflow_open(cx, false);
        }

        // ---- the N sidebar's resize grip
        let grip = self.view.view(cx, ids!(body.sidebar_dock.sidebar_grip));
        if let Some(fd) = grip.finger_down(actions) {
            self.sidebar_drag = Some((fd.abs.x, self.sidebar_user_width));
        }
        if let Some(fm) = grip.finger_move(actions) {
            if let Some((from_x, from_w)) = self.sidebar_drag {
                let w = (from_w + (from_x - fm.abs.x)).clamp(200.0, 480.0);
                if (w - self.sidebar_user_width).abs() > 0.5 {
                    self.sidebar_user_width = w;
                    let mut side = self.view.widget(cx, ids!(body.sidebar_dock.sidebar));
                    script_apply_eval!(cx, side, { width: #(w) });
                    self.view.redraw(cx);
                }
            }
        }
        if grip.finger_up(actions).is_some() {
            self.sidebar_drag = None;
        }

        // ---- shading strip / x-ray / lock
        for (id, s) in [
            (header_control_path(11), Shading::Wireframe),
            (header_control_path(12), Shading::Solid),
            (header_control_path(13), Shading::Material),
            (header_control_path(14), Shading::Realtime),
            (header_control_path(15), Shading::Rendered),
            (header_control_path(16), Shading::HiddenLine),
        ] {
            if self.view.radio_button(cx, id).clicked(actions) {
                cx.action(ShellAction::SetShading(v, s));
            }
        }
        if self.view.radio_button(cx, header_control_path(17)).clicked(actions) {
            cx.action(ShellAction::ToggleXray(v));
        }
        if self
            .view
            .radio_button(cx, header_control_path(20))
            .clicked(actions)
        {
            cx.action(ShellAction::ToggleLockViews);
        }
        if self
            .view
            .button(cx, header_control_path(10))
            .clicked(actions)
        {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let paused = state.view_at(v).rendered_paused;
                cx.action(ShellAction::SetRenderedPaused(v, !paused));
            }
        }
        for path in [header_control_path(9), overflow_number_path(9)] {
            if let Some(value) = self.view.fab_drag_number(cx, path).changed(actions) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    let mut settings = state.render;
                    settings.max_samples =
                        RenderSettings::clamp_max_samples(value.round() as u32);
                    cx.action(ShellAction::SetRenderSettings(settings));
                }
            }
        }

        // ---- T toolbar: one radio action path, including the walk figure.
        let tools = self.view.radio_button_set(
            cx,
            ids_array!(
                body.tool_stack.toolbar.tool_select,
                body.tool_stack.toolbar.tool_measure,
                body.tool_stack.toolbar.tool_section,
                body.tool_stack.toolbar.tool_walk,
            ),
        );
        if let Some(i) = tools.selected(cx, actions) {
            cx.action(ShellAction::SetTool(match i {
                1 => Tool::Measure(MeasureKind::Distance),
                2 => Tool::Section,
                3 => Tool::Walk,
                _ => Tool::Select,
            }));
        }
        for (id, tool) in [
            (ids!(body.tool_stack.measure_fly.fly_dist), MeasureKind::Distance),
            (ids!(body.tool_stack.measure_fly.fly_area), MeasureKind::Area),
            (ids!(body.tool_stack.measure_fly.fly_angle), MeasureKind::Angle),
        ] {
            if self.view.radio_button(cx, id).clicked(actions) {
                cx.action(ShellAction::SetTool(Tool::Measure(tool)));
            }
        }

        // ---- header menus (the buttons report presses through the shared
        // dropdown machinery, which is also what drives their open look; all
        // ids are pane-scoped, see `Self::pane`)
        for base in [VIEW_MENU, SELECT_MENU, OBJECT_MENU] {
            if let Some(anchor) = dropdown_clicked(actions, self.pane(base)) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    let items = if base == VIEW_MENU {
                        Self::view_menu(state, v)
                    } else if base == SELECT_MENU {
                        Self::select_menu(state)
                    } else {
                        Self::object_menu(state)
                    };
                    open_menu(cx, self.pane(base), items, anchor, MenuPlace::Below);
                }
            }
        }
        if let Some(anchor) = dropdown_clicked(actions, self.pane(COMPACT_MENU)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                open_menu(
                    cx,
                    self.pane(COMPACT_MENU),
                    Self::compact_menu(state, v),
                    anchor,
                    MenuPlace::Below,
                );
            }
        }
        // The ⋯ button reports its press through the shared dropdown
        // machinery (pane-scoped tag, see `Self::pane`), so a click-away
        // from an open menu re-presses it and one click moves the popup —
        // but what it raises is the overflow *popover* with real rows, not
        // a menu-layer menu.
        if dropdown_clicked(actions, self.pane(HEADER_OVERFLOW_OWNER)).is_some() {
            self.set_overflow_open(cx, !self.overflow_open);
        }
        let overflow_pick = (0..HEADER_CONTROL_COUNT)
            .filter(|index| !matches!(index, 6 | 7 | 8 | 9))
            .find(|index| {
                self.view
                    .button(cx, overflow_button_path(*index))
                    .clicked(actions)
            });
        if let Some(index) = overflow_pick {
            let anchor = self
                .view
                .widget(cx, overflow_control_path(index))
                .area()
                .rect(cx);
            self.set_overflow_open(cx, false);
            match index {
                0 => cx.action(FabUiAction::DropdownClicked {
                    tag: live_id!(editor_type),
                    anchor,
                }),
                1 => {
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        open_menu(
                            cx,
                            self.pane(MODE_MENU),
                            Self::mode_menu(state.view_at(v)),
                            anchor,
                            MenuPlace::BelowRight,
                        );
                    }
                }
                2..=4 => {
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        let (owner, items) = match index {
                            2 => (VIEW_MENU, Self::view_menu(state, v)),
                            3 => (SELECT_MENU, Self::select_menu(state)),
                            _ => (OBJECT_MENU, Self::object_menu(state)),
                        };
                        open_menu(cx, self.pane(owner), items, anchor, MenuPlace::BelowRight);
                    }
                }
                5 => {
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        open_menu(
                            cx,
                            self.pane(COMPACT_MENU),
                            Self::compact_menu(state, v),
                            anchor,
                            MenuPlace::BelowRight,
                        );
                    }
                }
                10 => {
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        cx.action(ShellAction::SetRenderedPaused(
                            v,
                            !state.view_at(v).rendered_paused,
                        ));
                    }
                }
                11..=16 => cx.action(ShellAction::SetShading(v, Shading::ALL[index - 11])),
                17 => cx.action(ShellAction::ToggleXray(v)),
                18 => {
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        open_menu(
                            cx,
                            self.pane(OVERLAY_MENU),
                            Self::overlay_menu(&state.view_at(v).overlays),
                            anchor,
                            MenuPlace::BelowRight,
                        );
                    }
                }
                19 => {
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        open_menu(
                            cx,
                            self.pane(GIZMO_MENU),
                            Self::gizmo_menu(state.view_at(v)),
                            anchor,
                            MenuPlace::BelowRight,
                        );
                    }
                }
                20 => cx.action(ShellAction::ToggleLockViews),
                _ => {}
            }
        }
        if let Some(anchor) = dropdown_clicked(actions, self.pane(MODE_MENU)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let items = Self::mode_menu(state.view_at(v));
                open_menu(cx, self.pane(MODE_MENU), items, anchor, MenuPlace::Below);
            }
        }
        if let Some(anchor) = dropdown_clicked(actions, self.pane(OVERLAY_MENU)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let items = Self::overlay_menu(&state.view_at(v).overlays);
                open_menu(cx, self.pane(OVERLAY_MENU), items, anchor, MenuPlace::BelowRight);
            }
        }
        if let Some(anchor) = dropdown_clicked(actions, self.pane(GIZMO_MENU)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let items = Self::gizmo_menu(state.view_at(v));
                open_menu(cx, self.pane(GIZMO_MENU), items, anchor, MenuPlace::BelowRight);
            }
        }

        // ---- right-click in the viewport body
        if let Some(fd) = self.view.view(cx, ids!(body.viewport)).finger_down(actions) {
            let secondary = fd
                .device
                .mouse_button()
                .map(|b| !b.is_primary())
                .unwrap_or(false);
            if secondary {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    let items = Self::context_menu(state);
                    open_menu(
                        cx,
                        self.pane(CONTEXT_MENU),
                        items,
                        Rect {
                            pos: fd.abs,
                            size: dvec2(0.0, 0.0),
                        },
                        MenuPlace::At,
                    );
                }
            }
        }

        // ---- picks (pane-scoped: only the pane that raised a menu applies
        // its pick)
        for base in [VIEW_MENU, SELECT_MENU, OBJECT_MENU, CONTEXT_MENU] {
            if let Some(pick) = menu_picked(actions, self.pane(base)) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    let state: &AppState = state;
                    self.apply_common(cx, state, v, pick);
                }
            }
        }
        if let Some(pick) = menu_picked(actions, self.pane(COMPACT_MENU)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let state: &AppState = state;
                self.apply_common(cx, state, v, pick);
            }
        }
        if menu_picked(actions, self.pane(HEADER_CONTEXT_OWNER)) == Some(live_id!(show_menus)) {
            self.show_menus = !self.show_menus;
            self.sync_header_menu_visibility(cx);
        }
        if let Some(pick) = menu_picked(actions, self.pane(MODE_MENU)) {
            let mode = if pick == live_id!(m_walk) {
                NavMode::Walk
            } else if pick == live_id!(m_fly) {
                NavMode::Fly
            } else {
                NavMode::Orbit
            };
            cx.action(ShellAction::SetNavMode(v, mode));
            cx.action(ShellAction::SetTool(if mode == NavMode::Walk {
                Tool::Walk
            } else {
                Tool::Select
            }));
        }
        if let Some(pick) = menu_picked(actions, self.pane(GIZMO_MENU)) {
            if let Some(p) = PRESETS.iter().copied().find(|p| preset_id(*p) == pick) {
                cx.action(ShellAction::PresetView(v, p));
            } else if pick == live_id!(toggle_ortho) {
                cx.action(ShellAction::ToggleOrtho(v));
            }
        }
        if let Some(pick) = menu_picked(actions, self.pane(OVERLAY_MENU)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let mut o = state.view_at(v).overlays;
                match pick {
                    x if x == live_id!(ov_grid) => o.grid = !o.grid,
                    x if x == live_id!(ov_axes) => o.axes = !o.axes,
                    x if x == live_id!(ov_outlines) => o.outlines = !o.outlines,
                    x if x == live_id!(ov_wire) => o.wire_on_shaded = !o.wire_on_shaded,
                    x if x == live_id!(ov_cavity) => o.cavity = !o.cavity,
                    x if x == live_id!(ov_ssao) => o.ssao = !o.ssao,
                    x if x == live_id!(ov_sections) => o.section_planes = !o.section_planes,
                    x if x == live_id!(ov_measure) => o.measurements = !o.measurements,
                    x if x == live_id!(ov_text) => o.text_info = !o.text_info,
                    x if x == live_id!(ov_gizmo) => o.nav_gizmo = !o.nav_gizmo,
                    _ => {}
                }
                cx.action(ShellAction::SetOverlays(v, o));
            }
        }
        if let Some(pick) = pie_picked(actions, PIE_OWNER) {
            if let Some(s) = shading_from_id(pick) {
                cx.action(ShellAction::SetShading(v, s));
            }
        }

        // ---- N sidebar
        let set = self.view.radio_button_set(
            cx,
            ids_array!(
                body.sidebar_dock.sidebar.tabs.tab_item,
                body.sidebar_dock.sidebar.tabs.tab_view,
                body.sidebar_dock.sidebar.tabs.tab_tool,
            ),
        );
        if let Some(i) = set.selected(cx, actions) {
            self.side_tab = i.min(2);
            self.synced_side = None;
            cx.action(ShellAction::SetSidebarTab(match self.side_tab {
                1 => SidebarTab::View,
                2 => SidebarTab::Tool,
                _ => SidebarTab::Item,
            }));
        }
        for panel in SIDE_PANELS {
            fold_panel_clicked(&self.view, cx, actions, panel);
        }

        let now = scope.data.get_mut::<AppState>().map(|s| {
            (
                s.view_at(v).camera,
                s.view_at(v).overlays,
                s.ui.lock_views,
            )
        });
        if let Some((mut camera, mut overlays, lock)) = now {
            let mut cam_changed = false;
            for (id, which) in [
                (
                    ids!(body.sidebar_dock.sidebar.page_view.view_panel.num_fov),
                    0,
                ),
                (
                    ids!(body.sidebar_dock.sidebar.page_view.view_panel.num_near),
                    1,
                ),
                (
                    ids!(body.sidebar_dock.sidebar.page_view.view_panel.num_far),
                    2,
                ),
            ] {
                if let Some(val) = self.view.fab_drag_number(cx, id).changed(actions) {
                    match which {
                        0 => camera.fov_y_deg = val as f32,
                        1 => camera.near = val as f32,
                        _ => camera.far = val as f32,
                    }
                    cam_changed = true;
                }
            }
            if cam_changed {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    let vsm = state.view_at_mut(v);
                    vsm.camera = camera;
                    vsm.render_dirty = true;
                    vsm.camera_revision = vsm.camera_revision.wrapping_add(1);
                }
            }
            if self
                .view
                .check_box(
                    cx,
                    ids!(body.sidebar_dock.sidebar.page_view.view_panel.cb_ortho),
                )
                .changed(actions)
                .is_some()
            {
                cx.action(ShellAction::ToggleOrtho(v));
            }
            if let Some(on) = self
                .view
                .check_box(
                    cx,
                    ids!(body.sidebar_dock.sidebar.page_view.view_panel.cb_lock),
                )
                .changed(actions)
            {
                if on != lock {
                    cx.action(ShellAction::ToggleLockViews);
                }
            }
            let mut ov_changed = false;
            for (id, f) in [
                (
                    ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_grid),
                    0,
                ),
                (
                    ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_outlines),
                    1,
                ),
                (
                    ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_wire),
                    2,
                ),
                (
                    ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_text),
                    3,
                ),
            ] {
                if let Some(on) = self.view.check_box(cx, id).changed(actions) {
                    match f {
                        0 => overlays.grid = on,
                        1 => overlays.outlines = on,
                        2 => overlays.wire_on_shaded = on,
                        _ => overlays.text_info = on,
                    }
                    ov_changed = true;
                }
            }
            if ov_changed {
                cx.action(ShellAction::SetOverlays(v, overlays));
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.children_indexed {
            self.children_indexed = true;
            let idx = self.view_index;
            let mut w = self.view.widget(cx, ids!(body.viewport));
            script_apply_eval!(cx, w, { view: #(idx) });
            let mut w = self.view.widget(cx, ids!(body.tool_overlay));
            script_apply_eval!(cx, w, { view: #(idx) hosts_panel: false });
            let mut w = self.view.widget(cx, ids!(body.gizmo_dock.gizmo));
            script_apply_eval!(cx, w, { view: #(idx) });
            // One popup identity per pane: stamp the header's popup buttons
            // with pane-scoped tag/owner ids, so "open" lights exactly the
            // button whose menu is up and a pick lands only in this pane.
            for (index, base) in [
                (1, MODE_MENU),
                (2, VIEW_MENU),
                (3, SELECT_MENU),
                (4, OBJECT_MENU),
                (5, COMPACT_MENU),
                (18, OVERLAY_MENU),
                (19, GIZMO_MENU),
            ] {
                let button = self.view.fab_dropdown_button(cx, header_control_path(index));
                button.set_tag(self.pane(base));
                button.set_owner(self.pane(base));
            }
            let overflow = self
                .view
                .fab_dropdown_button(cx, ids!(header.overflow_pin.overflow));
            overflow.set_tag(self.pane(HEADER_OVERFLOW_OWNER));
            overflow.set_owner(self.pane(HEADER_OVERFLOW_OWNER));
            for panel in SIDE_PANELS {
                set_panel_chevron(&self.view, cx, panel, true);
            }
        }
        if let Some(state) = scope.data.get_mut::<AppState>() {
            let v = self.view_index;
            let vs = state.view_at(v).clone();
            let label = match vs.nav_mode {
                NavMode::Walk => format!("Walk — {}", vs.view_label()),
                NavMode::Fly => format!("Fly — {}", vs.view_label()),
                NavMode::Orbit => format!("{} — {}", vs.name, vs.view_label()),
            };
            self.view
                .label(cx, ids!(body.overlay_text.view_label))
                .set_text(cx, &label);
            let context = match state
                .scene_state
                .selection
                .active
                .and_then(|id| state.scene.element(id))
            {
                Some(e) => {
                    let story = state
                        .scene
                        .story_of(e.id)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "Site".into());
                    format!(
                        "({}) {} › {}",
                        state.scene_state.selection.len(),
                        story,
                        e.name
                    )
                }
                None => {
                    if state.scene.name.is_empty() {
                        "Drop a .fab here or press Cmd+O".into()
                    } else {
                        state.scene.name.clone()
                    }
                }
            };
            self.view
                .label(cx, ids!(body.overlay_text.context_label))
                .set_text(cx, &context);
            self.view
                .widget(cx, ids!(body.overlay_text))
                .set_visible(cx, vs.overlays.text_info);
            self.view
                .widget(cx, ids!(body.gizmo_dock))
                .set_visible(cx, vs.overlays.nav_gizmo);
            self.view
                .widget(cx, ids!(body.tool_stack))
                .set_visible(cx, state.ui.toolbar_open);

            let rendered = vs.shading == Shading::Rendered;
            self.view
                .widget(cx, ids!(header.scroller.content.right_cluster.render_controls))
                .set_visible(cx, rendered);
            if rendered {
                self.view
                    .widget(cx, header_control_path(8))
                    .label(cx, ids!(render_badge))
                    .set_text(cx, &vs.rendered_badge());
                self.view
                    .label(cx, overflow_control_path(8))
                    .set_text(cx, &vs.rendered_badge());
                for path in [header_control_path(9), overflow_number_path(9)] {
                    self.view
                        .fab_drag_number(cx, path)
                        .set_value(cx, state.render.max_samples as f64);
                }
                if self.render_icon_paused != Some(vs.rendered_paused) {
                    self.render_icon_paused = Some(vs.rendered_paused);
                    let mut button = self
                        .view
                        .widget(cx, header_control_path(10));
                    if vs.rendered_paused {
                        script_apply_eval!(cx, button, {
                            draw_icon +: { svg: crate_resource("self://resources/icons/play.svg") }
                        });
                    } else {
                        script_apply_eval!(cx, button, {
                            draw_icon +: { svg: crate_resource("self://resources/icons/pause.svg") }
                        });
                    }
                }
            }

            self.view
                .widget(cx, overflow_button_path(10))
                .set_text(
                    cx,
                    if vs.rendered_paused {
                        "Resume rendering"
                    } else {
                        "Stop rendering"
                    },
                );
            for (index, shading) in Shading::ALL.iter().copied().enumerate() {
                let label = if vs.shading == shading {
                    format!("✓ {}", shading.label())
                } else {
                    shading.label().to_string()
                };
                self.view
                    .widget(cx, overflow_button_path(index + 11))
                    .set_text(cx, &label);
            }
            self.view
                .widget(cx, overflow_button_path(17))
                .set_text(cx, if vs.xray { "✓ X-Ray" } else { "X-Ray" });
            self.view
                .widget(cx, overflow_button_path(20))
                .set_text(
                    cx,
                    if state.ui.lock_views {
                        "✓ Lock Views"
                    } else {
                        "Lock Views"
                    },
                );

            let key = (
                vs.shading,
                vs.xray,
                state.tool,
                state.ui.lock_views,
                vs.nav_mode,
                state.ui.toolbar_open,
            );
            if self.synced != Some(key) {
                self.synced = Some(key);
                let toggles: [(&[LiveId], bool); 15] = [
                    (header_control_path(20), state.ui.lock_views),
                    (
                        header_control_path(11),
                        vs.shading == Shading::Wireframe,
                    ),
                    (header_control_path(12), vs.shading == Shading::Solid),
                    (
                        header_control_path(13),
                        vs.shading == Shading::Material,
                    ),
                    (
                        header_control_path(14),
                        vs.shading == Shading::Realtime,
                    ),
                    (
                        header_control_path(15),
                        vs.shading == Shading::Rendered,
                    ),
                    (
                        header_control_path(16),
                        vs.shading == Shading::HiddenLine,
                    ),
                    (header_control_path(17), vs.xray),
                    (
                        ids!(body.tool_stack.toolbar.tool_select),
                        state.tool == Tool::Select,
                    ),
                    (
                        ids!(body.tool_stack.toolbar.tool_measure),
                        matches!(state.tool, Tool::Measure(_)),
                    ),
                    (
                        ids!(body.tool_stack.toolbar.tool_section),
                        state.tool == Tool::Section,
                    ),
                    (
                        ids!(body.tool_stack.toolbar.tool_walk),
                        state.tool == Tool::Walk,
                    ),
                    (
                        ids!(body.tool_stack.measure_fly.fly_dist),
                        state.tool == Tool::Measure(MeasureKind::Distance),
                    ),
                    (
                        ids!(body.tool_stack.measure_fly.fly_area),
                        state.tool == Tool::Measure(MeasureKind::Area),
                    ),
                    (
                        ids!(body.tool_stack.measure_fly.fly_angle),
                        state.tool == Tool::Measure(MeasureKind::Angle),
                    ),
                ];
                for (id, on) in toggles {
                    self.view.radio_button(cx, id).set_active(cx, on, Animate::No);
                }
                self.view.label(cx, ids!(header.scroller.content.left_cluster.mode.label)).set_text(
                    cx,
                    match vs.nav_mode {
                        NavMode::Walk => "Walk Mode",
                        NavMode::Fly => "Fly Mode",
                        NavMode::Orbit => "Object Mode",
                    },
                );
            }

            // ---- N sidebar (150 ms slide)
            let open = state.ui.sidebar_open;
            let requested_side_tab = match state.ui.sidebar_tab {
                SidebarTab::Item => 0,
                SidebarTab::View => 1,
                SidebarTab::Tool => 2,
            };
            if self.side_tab != requested_side_tab {
                self.side_tab = requested_side_tab;
                self.synced_side = None;
            }
            let now = cx.seconds_since_app_start();
            if self.last_t == 0.0 {
                self.last_t = now;
                self.sidebar_t = if open { 1.0 } else { 0.0 };
            }
            let dt = (now - self.last_t).clamp(0.0, 0.05) as f32;
            self.last_t = now;
            let target = if open { 1.0f32 } else { 0.0 };
            if (self.sidebar_t - target).abs() > 0.01 {
                let speed = dt / 0.15;
                self.sidebar_t = if target > self.sidebar_t {
                    (self.sidebar_t + speed).min(target)
                } else {
                    (self.sidebar_t - speed).max(target)
                };
                self.next_frame = cx.new_next_frame();
            } else {
                self.sidebar_t = target;
            }
            let shown = self.sidebar_t > 0.01;
            let mut side = self.view.widget(cx, ids!(body.sidebar_dock.sidebar));
            side.set_visible(cx, shown);
            self.view
                .widget(cx, ids!(body.sidebar_dock.sidebar_grip))
                .set_visible(cx, shown);
            if shown {
                let w = self.sidebar_user_width * self.sidebar_t as f64;
                script_apply_eval!(cx, side, { width: #(w) });
            }
            if self.synced_side != Some((self.side_tab, open)) {
                self.synced_side = Some((self.side_tab, open));
                for (i, (tab, page)) in SIDE_PAGES.iter().enumerate() {
                    let on = i == self.side_tab;
                    self.view.radio_button(cx, tab).set_active(cx, on, Animate::No);
                    self.view.widget(cx, page).set_visible(cx, on);
                }
            }
            if shown {
                let units = state.scene.units;
                let sel = state.scene_state.selection.len();
                match state
                    .scene_state
                    .selection
                    .active
                    .and_then(|id| state.scene.element(id))
                {
                    Some(e) => {
                        let story = state
                            .scene
                            .story_of(e.id)
                            .map(|s| s.name.clone())
                            .unwrap_or_else(|| "—".into());
                        let size = if e.has_geometry() {
                            let ext = aabb_extent(&e.bounds);
                            format!(
                                "{} × {} × {}",
                                units.format_length(ext.x as f64),
                                units.format_length(ext.y as f64),
                                units.format_length(ext.z as f64)
                            )
                        } else {
                            "—".into()
                        };
                        let rows: [(&[LiveId], String); 5] = [
                            (
                                ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_name),
                                e.name.clone(),
                            ),
                            (
                                ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_type),
                                e.class.label().to_string(),
                            ),
                            (
                                ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_story),
                                story,
                            ),
                            (
                                ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_size),
                                size,
                            ),
                            (
                                ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_count),
                                sel.to_string(),
                            ),
                        ];
                        for (id, val) in rows {
                            self.view.view(cx, id).label(cx, ids!(value)).set_text(cx, &val);
                        }
                    }
                    None => {
                        for id in [
                            ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_name),
                            ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_type),
                            ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_story),
                            ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_size),
                        ] {
                            self.view.view(cx, id).label(cx, ids!(value)).set_text(cx, "—");
                        }
                        self.view
                            .view(
                                cx,
                                ids!(body.sidebar_dock.sidebar.page_item.item_panel.si_count),
                            )
                            .label(cx, ids!(value))
                            .set_text(cx, &sel.to_string());
                    }
                }
                let stamp = vs.camera_revision ^ ((state.ui.lock_views as u64) << 60);
                if self.synced_nums != Some(stamp) {
                    self.synced_nums = Some(stamp);
                    for (id, val) in [
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.view_panel.num_fov),
                            vs.camera.fov_y_deg as f64,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.view_panel.num_near),
                            vs.camera.near as f64,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.view_panel.num_far),
                            vs.camera.far as f64,
                        ),
                    ] {
                        self.view.fab_drag_number(cx, id).set_value(cx, val);
                    }
                    let o = vs.overlays;
                    let boxes: [(&[LiveId], bool); 6] = [
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.view_panel.cb_ortho),
                            vs.camera.ortho,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.view_panel.cb_lock),
                            state.ui.lock_views,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_grid),
                            o.grid,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_outlines),
                            o.outlines,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_wire),
                            o.wire_on_shaded,
                        ),
                        (
                            ids!(body.sidebar_dock.sidebar.page_view.overlay_panel.cb_text),
                            o.text_info,
                        ),
                    ];
                    for (id, on) in boxes {
                        self.view.check_box(cx, id).set_active(cx, on, Animate::No);
                    }
                }
            }
        }
        self.view
            .view(cx, ids!(header.scroller))
            .set_scroll_pos(cx, dvec2(self.header_pan, 0.0));
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_ok() {
            self.update_header_metrics(cx);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widths the running app measures for the header controls.
    fn widths() -> [f64; HEADER_CONTROL_COUNT] {
        let mut w = [0.0; HEADER_CONTROL_COUNT];
        w[0] = 86.0; // editor type
        w[1] = 87.0; // mode
        w[2] = 38.0; // View
        w[3] = 46.0; // Select
        w[4] = 47.0; // Object
        w[5] = 22.0; // ☰
        w[6] = 70.0; // time
        w[7] = 64.0; // haze
        w[8] = 112.0; // render badge (fixed width in the DSL)
        w[9] = 94.0; // spp limit
        w[10] = 20.0; // stop / resume
        for i in 11..=16 {
            w[i] = 24.0; // shading strip
        }
        w[17] = 24.0; // x-ray
        w[18] = 41.0; // overlays
        w[19] = 41.0; // gizmo
        w[20] = 24.0; // lock
        w
    }

    fn wanted(shading: Shading) -> [bool; HEADER_CONTROL_COUNT] {
        let mut want = [true; HEADER_CONTROL_COUNT];
        want[5] = false; // menus expanded, no ☰
        let sun = matches!(shading, Shading::Realtime | Shading::Rendered);
        want[6] = sun;
        want[7] = sun;
        let rendered = shading == Shading::Rendered;
        want[8] = rendered;
        want[9] = rendered;
        want[10] = rendered;
        want
    }

    fn kept(dropped: &[usize], want: &[bool; HEADER_CONTROL_COUNT]) -> Vec<usize> {
        (0..HEADER_CONTROL_COUNT)
            .filter(|i| want[*i] && !dropped.contains(i))
            .collect()
    }

    #[test]
    fn a_wide_header_drops_nothing() {
        for shading in [Shading::Solid, Shading::Realtime, Shading::Rendered] {
            let dropped = header_priority_drops(&widths(), &wanted(shading), 1200.0);
            assert!(dropped.is_empty(), "{shading:?}: {dropped:?}");
        }
    }

    #[test]
    fn menus_leave_first_and_time_outlasts_the_dropdowns() {
        let want = wanted(Shading::Realtime);
        // Just too narrow for everything: exactly the menus go.
        let dropped = header_priority_drops(&widths(), &want, 700.0);
        assert_eq!(dropped, vec![2, 3, 4]);
        // Narrower: the mode and editor dropdowns and Haze follow, in that
        // order, while Time and the whole icon suffix stay.
        let dropped = header_priority_drops(&widths(), &want, 460.0);
        assert_eq!(dropped, vec![0, 1, 2, 3, 4, 7]);
        let survivors = kept(&dropped, &want);
        assert!(survivors.contains(&6), "time must survive: {survivors:?}");
        for icon in 11..=20 {
            assert!(survivors.contains(&icon), "icon {icon} must survive");
        }
    }

    #[test]
    fn a_control_the_mode_does_not_want_is_never_dropped() {
        let want = wanted(Shading::Solid);
        let dropped = header_priority_drops(&widths(), &want, 300.0);
        for index in [5, 6, 7, 8, 9, 10] {
            assert!(!dropped.contains(&index), "{index} was not wanted");
        }
        let dropped = header_priority_drops(&widths(), &want, 460.0);
        assert!(dropped.iter().all(|i| want[*i]));
        assert!(dropped.windows(2).all(|w| w[0] < w[1]), "ascending order");
    }

    #[test]
    fn a_shading_switch_keeps_the_icon_suffix_identical_at_working_widths() {
        // The user's proof: Solid and Rendered at the same pane width keep
        // the same shading / x-ray / overlays / gizmo / lock set, so with
        // the right cluster right-anchored no icon moves under the pointer.
        for available in [582.0, 700.0, 900.0, 1200.0] {
            let solid = wanted(Shading::Solid);
            let rendered = wanted(Shading::Rendered);
            let kept_solid: Vec<usize> =
                kept(&header_priority_drops(&widths(), &solid, available), &solid)
                    .into_iter()
                    .filter(|i| (11..=20).contains(i))
                    .collect();
            let kept_rendered: Vec<usize> = kept(
                &header_priority_drops(&widths(), &rendered, available),
                &rendered,
            )
            .into_iter()
            .filter(|i| (11..=20).contains(i))
            .collect();
            assert_eq!(kept_solid, kept_rendered, "at {available}");
        }
    }


    #[test]
    fn the_menus_leave_together_never_leaving_one_behind() {
        let want = wanted(Shading::Solid);
        // A width where dropping only View and Select would already fit:
        // the whole trio must still go together.
        let dropped = header_priority_drops(&widths(), &want, 585.0);
        assert_eq!(dropped, vec![2, 3, 4]);
    }

    #[test]
    fn time_and_haze_stay_in_the_header_at_ordinary_pane_widths() {
        // A quad-layout pane on a 1600-wide window is ~582 points of header.
        let want = wanted(Shading::Realtime);
        let dropped = header_priority_drops(&widths(), &want, 582.0);
        assert!(!dropped.contains(&6), "time dropped: {dropped:?}");
        assert!(!dropped.contains(&7), "haze dropped: {dropped:?}");
        // And the editor-type dropdown survives too: only the menus and the
        // interaction mode leave at that width.
        assert_eq!(dropped, vec![1, 2, 3, 4]);
    }
}
