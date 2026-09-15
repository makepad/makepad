//! Layout of the resident video, sequencer and mixer controls. Compact
//! navigation changes visibility, never the engines or their running state.
use crate::*;

pub(super) fn apply(app: &mut App, cx: &mut Cx, size: Vec2d, compact: bool, page: usize) {
    macro_rules! style {
        ($id:ident, $($body:tt)*) => {{
            let mut widget = app.ui.widget(cx, ids!($id));
            script_apply_eval!(cx, widget, {$($body)*});
        }};
    }
    let landscape = size.x > size.y;
    let video = app.console_page == live_id!(video_page);
    let create = compact && page == 3;
    style!(pages, visible: #( !create ));
    style!(status_bar, height: 48 flow: mod.turtle.Right padding: mod.turtle.Inset{left: 8 right: 8});
    style!(brand_grip, visible: #(!compact));
    let appearance = if compact {"Style"} else if crate::theme::is_custom(cx) {"Black / orange"} else {"System theme"};
    app.ui.button(cx, ids!(appearance_toggle)).set_text(cx, appearance);
    style!(vj_source_a_preview, visible: #(!compact));
    style!(vj_source_b_preview, visible: #(!compact));
    let control_height = if compact {44.0} else {26.0};
    style!(deck_a_controls, height: #(control_height));
    style!(deck_b_controls, height: #(control_height));
    // The phone's main monitors already show both sources. Keep their
    // transport controls here; detailed timing remains on the desktop.
    for path in [ids!(deck_a_rate), ids!(deck_b_rate), ids!(deck_a_tween), ids!(deck_b_tween),
        ids!(deck_a_ai3_status), ids!(deck_b_ai3_status), ids!(deck_a_wheel_learn), ids!(deck_b_wheel_learn),
        ids!(slot_a_spin), ids!(slot_b_spin)] {
        app.ui.widget(cx, path).set_visible(cx, !compact);
    }
    if compact {
        style!(status_label, visible: false);
        style!(vj_controls_row, flow: mod.turtle.Right{wrap: true} align: mod.turtle.Align{x: 0.0 y: 0.0});
        let controls = page != 1;
        style!(vj_controls_row, visible: #(controls));
        let sources = page == 0;
        style!(vj_source_a, visible: #(sources) width: mod.turtle.Fill{basis: 360.0 min: 300.0} height: mod.turtle.Fit min_height: 56);
        style!(vj_source_b, visible: #(sources) width: mod.turtle.Fill{basis: 360.0 min: 300.0} height: mod.turtle.Fit min_height: 56);
        style!(vj_mix_column, width: mod.turtle.Fill{basis: 900.0 min: 260.0} height: mod.turtle.Fit);
        let effects = page == 2;
        style!(vj_effect_slots, visible: #(effects));
        style!(vj_library, visible: #(page != 0) flow: mod.turtle.Down min_height: 260);
        style!(vj_library_rail, width: mod.turtle.Fill height: 56 flow: mod.turtle.Right spacing: 8);
        let monitor_height = if page == 0 { if landscape { 100.0 } else { 170.0 } } else { 0.0 };
        if video {
            style!(deck_split, align: mod.widgets.SplitterAlign.FromA(#(monitor_height)) min_horizontal: 0.0 max_horizontal: 100000.0 size: 0.0);
        }
    } else {
        style!(status_label, visible: true);
        style!(vj_controls_row, visible: true flow: mod.turtle.Right align: mod.turtle.Align{x: 0.5 y: 0.0});
        style!(vj_source_a, visible: true width: mod.turtle.Fit height: 270 min_height: 0);
        style!(vj_source_b, visible: true width: mod.turtle.Fit height: 270 min_height: 0);
        style!(vj_mix_column, width: mod.turtle.Fill{basis: 478.0 min: 260.0 max: 620.0} height: mod.turtle.Fit);
        style!(vj_effect_slots, visible: true);
        style!(vj_library, visible: true flow: mod.turtle.Right min_height: 260);
        style!(vj_library_rail, width: 104 height: mod.turtle.Fill flow: mod.turtle.Down spacing: 6);
        if video {
            let height = app.presentation.video_split.unwrap_or(260.0);
            style!(deck_split, align: mod.widgets.SplitterAlign.FromA(#(height)) min_horizontal: 100.0 max_horizontal: 100000.0 size: 6.0);
        }
    }
    // Put the crossfader first on a phone. Reorder the existing views, so
    // playback, video textures and automation retain their identities.
    if let Some(mut view) = app.ui.view(cx, ids!(vj_controls_row)).borrow_mut() {
        let rank = |id: LiveId| match id {
            live_id!(vj_mix_column) => if compact { 0 } else { 2 },
            live_id!(vj_source_a) => 1,
            live_id!(vj_source_b) => 3,
            _ => if compact { 4 } else { 0 },
        };
        view.children.sort_by_key(|(id, _)| rank(*id));
    }

    let sequence = !compact || page == 0;
    let rack = !compact || page == 2 || (page == 1 && app.synth_mix.selected != SynthTrack::Ironfish);
    let sound = app.synth_mix.selected == SynthTrack::Ironfish && (!compact || page == 1);
    style!(synth_sequence_column, visible: #(sequence || rack));
    style!(synth_editors, visible: #(sequence));
    style!(synth_rack, visible: #(rack));
    style!(synth_engine_column, visible: #(sound));
    if compact {
        style!(synth_engine_column, width: mod.turtle.Fill);
        style!(synth_rack, height: mod.turtle.Fill);
    } else {
        style!(synth_engine_column, width: mod.turtle.Fill{basis: 520.0 min: 280.0 max: 700.0});
        style!(synth_rack, height: mod.turtle.Fit);
    }
    style!(mix_channels, visible: #(!compact || page == 0));
    style!(mix_master_panel, visible: #(!compact || page == 1 || page == 2));
    if compact { style!(mix_channels, width: mod.turtle.Fill); }
    else { style!(mix_channels, width: mod.turtle.Fill{basis: 460.0 min: 200.0 max: 560.0}); }
}
