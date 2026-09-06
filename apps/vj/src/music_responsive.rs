//! Compact layout of the DJ page: Perform, Library and Decks on a phone or
//! a short landscape window.
//!
//! Navigation only moves the VISIBILITY, FLOW and WIDTHS of the resident
//! deck widgets. Nothing is re-created, duplicated or reparented, so every
//! cached ref in `MusicRefs`, every action id and every running engine stays
//! exactly where it is — and a style reload, which re-applies the DSL to the
//! same children, cannot leave a stale copy behind. Root calls [`apply`]
//! after that reload, so everything set here is re-asserted from scratch.
//!
//! Desktop choices (tab stage, follow mode, shown deck, fold) are captured on
//! the way into compact and put back on the way out; the geometry-derived
//! parts are recomputed with the same rules the desktop syncs use, because
//! the event that leaves compact may reach those syncs before or after us.
use crate::*;
use crate::console_scale::{self, TabStage};
use crate::deck_sections::{DeckSections, Fold};
use crate::deck_tabs::{DeckTabs, TabFollow};

/// What the desktop looked like when the console went compact.
#[derive(Clone, Copy)]
struct Desktop {
    tab_stage: TabStage,
    deck_tabs: DeckTabs,
    deck_sections: DeckSections,
}

/// Module state, kept in `Cx` so it outlives style reloads. No widget refs
/// live here: every lookup goes through the tree by id, so nothing can go
/// stale when the DSL is re-applied.
#[derive(Default)]
struct State {
    /// Present exactly while compact.
    desktop: Option<Desktop>,
    page: usize,
}

/// A phone in landscape: the transport keys stand as a two-column block
/// either side of the waves. Two 48pt keys and their gap.
const LANDSCAPE_TRANSPORT_WIDTH: f64 = 110.0;

macro_rules! style {
    ($app:expr, $cx:ident, $id:ident, $($body:tt)*) => {{
        let cx = &mut *$cx;
        let mut widget = $app.ui.widget(cx, ids!($id));
        script_apply_eval!(cx, widget, {$($body)*});
    }};
}

fn set_visible(app: &mut App, cx: &mut Cx, path: &[LiveId], visible: bool) {
    let widget = app.ui.widget(cx, path);
    if widget.visible() != visible {
        widget.set_visible(cx, visible);
    }
}

pub(super) fn apply(app: &mut App, cx: &mut Cx, size: Vec2d, compact: bool, page: usize) {
    let landscape = size.x > size.y;
    let (entering, leaving) = {
        let state = cx.global::<State>();
        let entering = compact && state.desktop.is_none();
        let leaving = !compact && state.desktop.is_some();
        if entering {
            state.desktop = Some(Desktop {
                tab_stage: app.tab_stage,
                deck_tabs: app.deck_tabs,
                deck_sections: app.deck_sections,
            });
        }
        state.page = page;
        (entering, leaving)
    };
    if !compact {
        if leaving {
            let saved = cx.global::<State>().desktop.take().unwrap();
            restore(app, cx, size, saved);
        }
        return;
    }
    if entering {
        // Three tabs, the operator's own: on a phone every deck and the
        // mixer take the width in turn, exactly the desktop's last stage,
        // which is also what makes `App::handle_deck_tabs` listen.
        app.tab_stage = TabStage::All;
        app.deck_tabs.set_decks(3);
        let (target, audible) = (app.tab_target(), app.tab_audible());
        app.deck_tabs.set_follow(TabFollow::Manual, target, audible);
    }
    let perform = page == 0;
    let library = page == 1;

    // ---- headers and overviews ----
    // Portrait: each deck's identity on a line of its own, QUANT under
    // them. Landscape: one line, without the art and artist, so the height
    // goes to the waves.
    let heads = !library;
    style!(app, cx, deck_heads, visible: #(heads));
    if landscape {
        style!(app, cx, deck_heads, flow: mod.turtle.Right);
        style!(app, cx, deck_a_head, width: mod.turtle.Fill);
        style!(app, cx, deck_b_head, width: mod.turtle.Fill);
    } else {
        style!(app, cx, deck_heads, flow: mod.turtle.Right{wrap: true});
        style!(app, cx, deck_a_head, width: mod.turtle.Fill{basis: 600.0 min: 280.0});
        style!(app, cx, deck_b_head, width: mod.turtle.Fill{basis: 600.0 min: 280.0});
    }
    for id in [ids!(deck_a_art), ids!(deck_b_art), ids!(deck_a_artist), ids!(deck_b_artist)] {
        set_visible(app, cx, id, false);
    }
    for id in [ids!(deck_a_key),ids!(deck_b_key),ids!(deck_a_time),ids!(deck_b_time),ids!(deck_a_pitch_text),ids!(deck_b_pitch_text)] {
        set_visible(app,cx,id,false);
    }
    set_visible(app, cx, ids!(quant_cluster), page == 2);
    style!(app, cx, deck_overviews, visible: #(page == 2 && !landscape));

    // ---- the body: decks OR lists, never both ----
    style!(app, cx, page_body, flow: mod.turtle.Down);
    style!(app, cx, deck_region, visible: #(!library) width: mod.turtle.Fill height: mod.turtle.Fill spacing: 8);
    if landscape {
        style!(app, cx, deck_region, flow: mod.turtle.Right);
    } else {
        style!(app, cx, deck_region, flow: mod.turtle.Down);
    }
    // Perform: a transport row above the waves and one below (portrait),
    // or a transport block either side of them (landscape). Decks: the
    // shown panel takes the whole region.
    for panel in [ids!(deck_a_panel), ids!(deck_b_panel)] {
        let mut widget = app.ui.widget(cx, panel);
        if perform && landscape {
            script_apply_eval!(cx, widget, {
                width: #(LANDSCAPE_TRANSPORT_WIDTH) height: mod.turtle.Fill align: mod.turtle.Align{x: 0.0 y: 0.5}
            });
        } else if perform {
            script_apply_eval!(cx, widget, { width: mod.turtle.Fill height: mod.turtle.Fit align: mod.turtle.Align{x: 0.0 y: 0.0} });
        } else {
            script_apply_eval!(cx, widget, { width: mod.turtle.Fill height: mod.turtle.Fill align: mod.turtle.Align{x: 0.0 y: 0.0} });
        }
    }
    // A short window has width to spare: the EQ and stem blocks stand side
    // by side, and the karaoke block folds to its heading (the accordion
    // the desktop already has, so a chevron still opens it).
    if landscape {
        style!(app, cx, deck_a_knobs, flow: mod.turtle.Right{wrap: true});
        style!(app, cx, deck_b_knobs, flow: mod.turtle.Right{wrap: true});
        for body in [ids!(deck_a_eq_body), ids!(deck_a_stems_body), ids!(deck_b_eq_body), ids!(deck_b_stems_body)] {
            let mut widget = app.ui.widget(cx, body);
            script_apply_eval!(cx, widget, { width: mod.turtle.Fit });
        }
    } else {
        style!(app, cx, deck_a_knobs, flow: mod.turtle.Down);
        style!(app, cx, deck_b_knobs, flow: mod.turtle.Down);
        for body in [ids!(deck_a_eq_body), ids!(deck_a_stems_body), ids!(deck_b_eq_body), ids!(deck_b_stems_body)] {
            let mut widget = app.ui.widget(cx, body);
            script_apply_eval!(cx, widget, { width: mod.turtle.Fill });
        }
    }
    let fold = if landscape { Fold::Pairs } else { Fold::None };
    if app.deck_sections.set_fold(fold) {
        app.paint_deck_sections(cx);
    }

    // ---- the library ----
    // The lists take the whole page, and the explorer's tool row wraps:
    // the search keeps a usable width and the chips fall to a second line.
    style!(app, cx, lists_column, visible: #(library) width: mod.turtle.Fill height: mod.turtle.Fill);
    style!(app, cx, music_library_tools, flow: mod.turtle.Right{wrap: true});
    style!(app, cx, music_catalog, width: mod.turtle.Fill{basis: 420.0 min: 240.0} flow: mod.turtle.Right{wrap: true});
    style!(app, cx, music_search, width: mod.turtle.Fill{basis: 200.0 min: 120.0});
    style!(app, cx, queue_drop, width: mod.turtle.Fill);
    // A finger scrolls the lists. The desktop keeps drags for its controls.
    set_drag_scrolling(app, cx, true);

    paint_deck_tabs(app, cx);
    app.ui.redraw(cx);
}

fn set_drag_scrolling(app: &mut App, cx: &mut Cx, on: bool) {
    for list in [ids!(music_tracks.list), ids!(music_queue.list)] {
        let mut widget = app.ui.widget(cx, list);
        if widget.is_empty() {
            continue;
        }
        script_apply_eval!(cx, widget, { drag_scrolling: #(on) });
    }
}

/// The compact form of `App::paint_deck_tabs`: called after every
/// `paint_deck_sections` on the music pump and on every tab press, so it is
/// where page-dependent visibility is re-asserted. Every set is guarded, so
/// a settled console does no work.
pub(super) fn paint_deck_tabs(app: &mut App, cx: &mut Cx) {
    let page = cx.global::<State>().page;
    let perform = page == 0;
    let decks = page == 2;
    let shown = app.deck_tabs.shown();

    // `paint_deck_sections` just put the desktop's 330-point floor back on
    // the region. A phone has no such floor: the region is whatever the
    // headers leave.
    let region = app.ui.view(cx, ids!(deck_region));
    if let Some(mut view) = region.borrow_mut() {
        if let Size::Fill { weight, basis, shrink, min, max } = view.walk.height {
            if min.is_some() {
                view.walk.height = Size::Fill { weight, basis, shrink, min: None, max };
            }
        }
        // Perform stands A above the waves and B below them, whatever the
        // desktop's tabs last swapped.
        let rank = |id: LiveId| match id {
            live_id!(deck_a_panel) => 0,
            live_id!(deck_lanes) => 1,
            live_id!(deck_b_panel) => 2,
            _ => 3,
        };
        let sorted = view.children.windows(2).all(|pair| rank(pair[0].0) <= rank(pair[1].0));
        if !sorted {
            view.children.sort_by_key(|(id, _)| rank(*id));
        }
    }

    set_visible(app, cx, ids!(deck_a_panel), perform || (decks && shown == 0));
    set_visible(app, cx, ids!(deck_b_panel), perform || (decks && shown == 1));
    set_visible(app, cx, ids!(deck_lanes), perform || (decks && shown == 2));
    // Perform keeps only the primary transport of each deck; Decks shows
    // the whole panel — tools, knobs, both transport rows — behind tabs.
    for id in [
        ids!(deck_a_tools), ids!(deck_a_body), ids!(deck_a_transport_loop),
        ids!(deck_b_tools), ids!(deck_b_body), ids!(deck_b_transport_loop),
        ids!(strip_shaping), ids!(strip_automation),
    ] {
        set_visible(app, cx, id, decks);
    }
    set_visible(app, cx, ids!(deck_a_transport_main), true);
    set_visible(app, cx, ids!(deck_b_transport_main), true);
    for (strip, tabs, modes) in App::tab_strips() {
        set_visible(app, cx, strip, decks);
        for (index, tab) in tabs.into_iter().enumerate() {
            set_visible(app, cx, tab, true);
            if decks {
                app.paint_lit(cx, tab, index == shown);
            }
        }
        // Three tabs and nothing to follow: the mode marks go, as on the
        // desktop's last stage.
        for (_, chip) in modes {
            set_visible(app, cx, chip, false);
        }
    }

    // The explorer's column heads follow the rows: `VjTrackList` drops the
    // detail columns when `App::sync_library_density` measures it narrow,
    // and the heads over them go with them.
    let narrow = app.library_narrow.unwrap_or(true);
    for head in [
        ids!(th_key), ids!(th_time), ids!(music_th_stem_cell),
        ids!(music_th_krk_cell), ids!(th_license), ids!(th_tags),
    ] {
        set_visible(app, cx, head, !narrow);
    }
}

/// Back to the desktop: the DSL's own geometry, the operator's saved
/// choices, and the geometry-derived stages recomputed the way the desktop
/// syncs compute them.
fn restore(app: &mut App, cx: &mut Cx, size: Vec2d, saved: Desktop) {
    app.tab_stage = saved.tab_stage;
    app.deck_tabs = saved.deck_tabs;
    app.deck_sections = saved.deck_sections;

    if let Some(window_id) = app.ui.window(cx, ids!(main_window)).window_id() {
        if cx.windows.is_valid(window_id) {
            let native = cx.windows[window_id].native_dpi_factor();
            // `size` is the window in native points; the syncs key on
            // physical pixels.
            let physical = size * native;
            let dpi = console_scale::console_dpi(physical.x, physical.y, native);
            let stage = console_scale::console_tabs_for(
                console_scale::deck_span(physical.x, physical.y, native) / dpi,
            );
            if stage != app.tab_stage {
                app.tab_stage = stage;
                app.deck_tabs.set_decks(if stage == TabStage::All { 3 } else { 2 });
                if stage == TabStage::All {
                    let (target, audible) = (app.tab_target(), app.tab_audible());
                    app.deck_tabs.set_follow(TabFollow::Manual, target, audible);
                }
            } else {
                app.deck_tabs.set_decks(if stage == TabStage::All { 3 } else { 2 });
            }
            let fold = match console_scale::console_fold(physical.x, physical.y, native) {
                console_scale::ConsoleFold::None => Fold::None,
                console_scale::ConsoleFold::Pairs => Fold::Pairs,
                console_scale::ConsoleFold::Singles => Fold::Singles,
            };
            app.deck_sections.set_fold(fold);
            let beside = console_scale::console_lists_beside(physical.x, physical.y, native);
            app.lists_beside = beside;
            if beside {
                let width = console_scale::lists_width_points(physical.x / dpi - 6.0);
                style!(app, cx, page_body, flow: mod.turtle.Right);
                style!(app, cx, lists_column, width: #(width));
            } else {
                style!(app, cx, page_body, flow: mod.turtle.Down);
                style!(app, cx, lists_column, width: mod.turtle.Fill);
            }
        }
    }

    style!(app, cx, deck_heads, visible: true flow: mod.turtle.Right);
    style!(app, cx, deck_a_head, width: mod.turtle.Fill);
    style!(app, cx, deck_b_head, width: mod.turtle.Fill);
    for id in [
        ids!(deck_a_art), ids!(deck_b_art), ids!(deck_a_artist), ids!(deck_b_artist),
        ids!(deck_a_key),ids!(deck_b_key),ids!(deck_a_time),ids!(deck_b_time),ids!(deck_a_pitch_text),ids!(deck_b_pitch_text),
        ids!(quant_cluster), ids!(deck_overviews), ids!(deck_region), ids!(deck_lanes),
        ids!(lists_column), ids!(deck_a_tools), ids!(deck_a_body), ids!(deck_a_transport_main),
        ids!(deck_a_transport_loop), ids!(deck_b_tools), ids!(deck_b_body),
        ids!(deck_b_transport_main), ids!(deck_b_transport_loop), ids!(strip_shaping),
        ids!(strip_automation), ids!(th_key), ids!(th_time), ids!(music_th_stem_cell),
        ids!(music_th_krk_cell), ids!(th_license), ids!(th_tags),
    ] {
        set_visible(app, cx, id, true);
    }
    style!(app, cx, deck_region, width: mod.turtle.Fill height: mod.turtle.Fill{min: 330.0} flow: mod.turtle.Right spacing: 8);
    for panel in [ids!(deck_a_panel), ids!(deck_b_panel)] {
        let mut widget = app.ui.widget(cx, panel);
        script_apply_eval!(cx, widget, { width: 316 height: mod.turtle.Fill align: mod.turtle.Align{x: 0.0 y: 0.0} });
    }
    style!(app, cx, deck_a_knobs, flow: mod.turtle.Down);
    style!(app, cx, deck_b_knobs, flow: mod.turtle.Down);
    for body in [ids!(deck_a_eq_body), ids!(deck_a_stems_body), ids!(deck_b_eq_body), ids!(deck_b_stems_body)] {
        let mut widget = app.ui.widget(cx, body);
        script_apply_eval!(cx, widget, { width: mod.turtle.Fill });
    }
    style!(app, cx, lists_column, height: mod.turtle.Fill);
    style!(app, cx, music_library_tools, flow: mod.turtle.Right);
    style!(app, cx, music_catalog, width: mod.turtle.Fill flow: mod.turtle.Right);
    style!(app, cx, music_search, width: mod.turtle.Fill{min: 96.0 max: 488.0});
    style!(app, cx, queue_drop, width: 320);
    set_drag_scrolling(app, cx, false);
    // The desktop painters own the rest: panel turns, strips, mode marks,
    // the fold's chevrons and the region floor.
    app.paint_deck_sections(cx);
    app.paint_deck_tabs(cx);
    app.paint_lists_tabs(cx);
    app.ui.redraw(cx);
}
