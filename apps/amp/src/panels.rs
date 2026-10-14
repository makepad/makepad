//! The panels host: the list area swaps between the track list, the
//! equalizer (the stage draws the page under tab keys; see
//! `layout.rs`). The tab keys, the device keys (EQ) and Cmd+E /
//! Cmd+K choose the page; the last page is kept per size class
//! (`<data>/panels.txt`), so a pocket window and a large one each come back
//! as they were left.

use crate::hw;
use crate::layout::{self, HwStageWidgetRefExt, SizeClass};
use crate::micro::HwMarqueeWidgetRefExt;
use crate::widgets::LibraryListWidgetRefExt;
use crate::App;
use makepad_widgets::*;
use std::collections::HashMap;

#[derive(Default)]
struct PanelsState {
    /// The class the page was last recorded for.
    class: Option<SizeClass>,
    /// The page last seen, to notice a change.
    seen: String,
    /// The list shown (a playlist, the search), to notice the person going
    /// to another: that brings the Tracks page up.
    list: Option<(crate::model::Page, String)>,
    /// The small window's docked sections: shown by the window's height
    /// until a section key is pressed (then by hand, until the class
    /// changes); the height level last used, for hysteresis.
    small_manual: bool,
    small_eq: bool,
    small_pl: bool,
    small_level: usize,
    /// The playlist band shows the playlists, not the tracks.
    small_lists: bool,
    /// The playlist band's height when it was last folded, and the window
    /// height a section key asked for (a resize to anything else is the
    /// person's, and gives the bands back to the window's height).
    pl_h: f64,
    expect_h: Option<f64>,
    expect_reached: bool,
    /// The EQ panel drawn flat (in a band), as last applied.
    flat: Option<(bool, bool, String)>,
    last: HashMap<String, String>,
    loaded: bool,
}

/// The equalizer band's height with its gap (mini.splash: 158 + 4).
const EQ_BAND_H: f64 = 162.0;

fn class_key(class: SizeClass) -> &'static str {
    match class {
        SizeClass::Pocket => "pocket",
        SizeClass::Tiny => "tiny",
        SizeClass::Small => "small",
        SizeClass::Medium => "medium",
        SizeClass::Large => "large",
    }
}

fn path() -> Option<std::path::PathBuf> {
    Some(crate::worker::data_dir()?.join("panels.txt"))
}

fn state(cx: &mut Cx) -> &mut PanelsState {
    if !cx.has_global::<PanelsState>() {
        cx.set_global(PanelsState::default());
    }
    let s = cx.global::<PanelsState>();
    if !s.loaded {
        s.loaded = true;
        if let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                if let Some((class, page)) = line.split_once(' ') {
                    s.last.insert(class.to_string(), page.trim().to_string());
                }
            }
        }
    }
    s
}

fn save(s: &PanelsState) {
    let mut lines: Vec<String> = s.last.iter().map(|(c, p)| format!("{c} {p}")).collect();
    lines.sort();
    if let Some(path) = path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(path, lines.join("\n") + "\n");
    }
}

impl App {
    /// The tab keys follow the host: shown for the pages it offers (none
    /// when it offers one), lit for the page it shows. The page is kept for
    /// the size class; a new class brings back its own.
    pub(crate) fn apply_panels(&mut self, cx: &mut Cx) {
        let (pages, page) = self.ui.hw_stage(cx, ids!(stage)).panels();
        // The theatre deck: the picture's name (scrolling while it plays),
        // and its EQ keys lit for the page shown.
        let name = self.ui.label(cx, ids!(milk_now)).text();
        let playing = self.model.playback.is_playing();
        self.ui.hw_marquee(cx, ids!(deck_name)).set(cx, &name, playing, None);
        self.ui.relief_view(cx, ids!(deck_eq_cap)).set_lit(cx, if page == "eq" { 1.0 } else { 0.0 });
        self.ui.relief_view(cx, ids!(deck_eq_icon_cap)).set_lit(cx, if page == "eq" { 1.0 } else { 0.0 });
        // A page chosen while its host is folded away (a theatre's closed
        // drawer) opens it.
        let host_shown = self.ui.hw_stage(cx, ids!(stage)).shown("tracks_host").is_some() || self.ui.hw_stage(cx, ids!(stage)).shown("tracks").is_some();
        let seen = std::mem::replace(&mut state(cx).seen, page.clone());
        if seen != page && page != "tracks" && !host_shown && !self.up_next_open {
            self.up_next_open = true;
            self.apply_up_next(cx);
            self.save_up_next(cx);
        }
        // Narrow: the tabs are icons alone.
        let tabs_w = self.ui.hw_stage(cx, ids!(stage)).shown("tracks_host").map(|r| r.size.x).unwrap_or(1000.0);
        let words = tabs_w >= 380.0;
        for id in [ids!(tab_tracks), ids!(tab_eq)] {
            self.set_visible(cx, id, words);
        }
        for (name, cap) in [("tracks", ids!(tab_tracks_cap)), ("eq", ids!(tab_eq_cap))] {
            let offered = pages.len() > 1 && pages.iter().any(|p| p == name);
            self.set_visible(cx, cap, offered);
            self.ui.relief_view(cx, cap).set_lit(cx, if page == name { 1.0 } else { 0.0 });
        }
        // Going to a list (a playlist picked, a search typed) is going to the
        // tracks: the list area shows them, and that is the page kept.
        let list_now = (self.model.page.clone(), self.model.query.clone());
        let moved = state(cx).list.as_ref().is_some_and(|l| *l != list_now);
        state(cx).list = Some(list_now);
        if moved && page != "tracks" && pages.len() > 1 {
            self.show_page(cx, "tracks");
            let s = state(cx);
            s.last.insert("page".to_string(), "tracks".to_string());
            save(s);
        }
        let class = layout::active_class(cx);
        // The AI stylist opened in the tiny widget: there is no room for it,
        // so the window grows to the mini player first.
        if class == SizeClass::Tiny && cx.has_global::<crate::ai::ThemeAi>() && cx.global::<crate::ai::ThemeAi>().chat_open {
            self.go_to_layout_size(cx, "mini");
        }
        // One page for the whole app, whatever the size or layout.
        let key = "page";
        let _ = class_key;
        if class == SizeClass::Small {
            self.apply_small_sections(cx);
        }
        // Flat (no plate of their own) in a small window's band and as a tab
        // page; the tab or the band is then their close.
        let band = class == SizeClass::Small;
        let tabbed = pages.len() > 1;
        self.apply_flat_panels(cx, band || tabbed, band);
        // The list area's tabs open the equalizer: the display
        // wing's own keys for them would only repeat them.
        self.set_visible(cx, ids!(eq_page_cap), !tabbed);
        // The EQ's own close key goes (the EQ's module shows it again on its
        // updates, so this is set every pass).
        if band || tabbed {
            self.set_visible(cx, ids!(eq_close_cap), false);
        }
        // In a band the EQ's header is a player's: ON, AUTO, PRESETS.
        for id in [ids!(eq_presets_row), ids!(eq_flat_cap), ids!(eq_save_cap), ids!(eq_led), ids!(eq_title)] {
            if band {
                self.set_visible(cx, id, false);
            }
        }
        if !band {
            self.set_visible(cx, ids!(eq_presets_row), true);
        }
        self.set_visible(cx, ids!(eq_presets_cap), band);
        self.set_visible(cx, ids!(eq_band_fill), band);
        if band {
            self.set_visible(cx, ids!(eq_auto_cap), true);
            let on = self.model.eq.on;
            self.set_label(cx, live_id!(eq_power), ids!(eq_power), if on { "ON" } else { "OFF" });
        }
        let s = state(cx);
        if s.class != Some(class) {
            let first = s.class.is_none();
            s.class = Some(class);
            s.small_manual = false;
            // Coming into a class: the page the person chose (a small
            // window's bands may have changed what shows). The first class of
            // a run keeps what the panels restored.
            if let Some(want) = s.last.get(key).cloned() {
                if !first && want != page && (pages.contains(&want) || want == "tracks") {
                    self.show_page(cx, &want);
                }
            }
            return;
        }
        if !page.is_empty() && s.last.get(key) != Some(&page) && pages.len() > 1 {
            s.last.insert(key.to_string(), page.clone());
            save(s);
        }
    }

    /// Show `page` in the host: the others close.
    pub(crate) fn show_page(&mut self, _cx: &mut Cx, page: &str) {
        let eq_open = self.model.eq.open;
        match page {
            "eq" => {
                if !self.model.eq.open {
                    self.toggle_eq();
                }
            }
            _ => {
                if eq_open {
                    self.toggle_eq();
                }
            }
        }
    }

    /// The small window's bands under the main window, shown by the
    /// window's height (20 pt of hysteresis each way) until a key folds or
    /// opens one by hand: the playlist from 300 pt, the equalizer between
    /// them from 560. In a band the EQ sits flat: no plate, frame or keys of
    /// its own (the band's title strip does that).
    pub(crate) fn apply_small_sections(&mut self, cx: &mut Cx) {
        // The playlist band from where it holds a few rows (not a sliver).
        const LEVELS: [f64; 2] = [380.0, 600.0];
        const SLACK: f64 = 20.0;
        let h = self.window_size(cx).y;
        let s = state(cx);
        // A resize by hand (not one a section key asked for): the bands
        // follow the window's height again.
        if let Some(want) = s.expect_h {
            if (h - want).abs() <= 2.0 {
                s.expect_reached = true;
            } else if s.expect_reached && s.small_manual {
                s.small_manual = false;
                s.expect_h = None;
                s.expect_reached = false;
                let mut level = 0;
                while level < LEVELS.len() && h >= LEVELS[level] {
                    level += 1;
                }
                s.small_level = level;
            }
        }
        let mut level = s.small_level;
        while level < LEVELS.len() && h >= LEVELS[level] + SLACK {
            level += 1;
        }
        while level > 0 && h < LEVELS[level - 1] - SLACK {
            level -= 1;
        }
        s.small_level = level;
        if !s.small_manual {
            s.small_pl = level >= 1;
            s.small_eq = level >= 2;
        }
        let (eq, pl) = (s.small_eq, s.small_pl);
        // The visualizer band (VIS) shares the room under the main window
        // with the playlist; a tap on its picture shows it alone.
        // A wide, low window (a phone on its side) has room for one of the
        // picture and the playlist under the main window: the picture, while
        // VIS is on.
        let vis = self.milk.settings.on;
        let full = vis && self.milk.fullscreen;
        let size = self.window_size(cx);
        let pl = pl && !(vis && size.x > size.y);
        let eq = eq && !full;
        self.set_visible(cx, ids!(main_band), !full);
        self.set_visible(cx, ids!(vis_band), vis);
        self.set_visible(cx, ids!(vis_strip), !full);
        self.set_visible(cx, ids!(eq_band), eq);
        self.set_visible(cx, ids!(pl_band), pl && !full);
        self.ui.relief_view(cx, ids!(tog_vis_cap)).set_lit(cx, if vis { 1.0 } else { 0.0 });
        // The equalizer band is the EQ panel, open (closed under the
        // fullscreen picture: its panel draws wherever it is open).
        if eq != self.model.eq.open {
            self.toggle_eq();
        }
        self.ui.relief_view(cx, ids!(tog_eq_cap)).set_lit(cx, if eq { 1.0 } else { 0.0 });
        self.ui.relief_view(cx, ids!(tog_pl_cap)).set_lit(cx, if pl { 1.0 } else { 0.0 });
        // The file readouts: known for a local file (its decoder), not for
        // the music system's stream (left empty, and the row folds).
        let info = if self.model.engine == crate::model::Engine::Local && self.model.playback.is_playing() {
            self.workers.as_ref().map(|w| w.shared.file_info()).filter(|i| i.1 > 0)
        } else {
            None
        };
        let (kbps, khz, ch) = match info {
            Some((rate, channels, bytes)) => {
                let secs = self.model.playback.duration_ms as f64 / 1000.0;
                let kbps = if secs > 1.0 { format!("{} kbps", (bytes as f64 * 8.0 / secs / 1000.0).round() as u64) } else { String::new() };
                (kbps, format!("{} kHz", (rate as f64 / 1000.0).round() as u64), if channels >= 2 { "stereo".to_string() } else { "mono".to_string() })
            }
            None => (String::new(), String::new(), String::new()),
        };
        self.set_visible(cx, ids!(info_kbps_well), !kbps.is_empty());
        self.set_visible(cx, ids!(info_khz_well), !khz.is_empty());
        self.set_visible(cx, ids!(info_ch_well), !ch.is_empty());
        self.set_label(cx, live_id!(info_kbps), ids!(info_kbps), &kbps);
        self.set_label(cx, live_id!(info_khz), ids!(info_khz), &khz);
        self.set_label(cx, live_id!(info_ch), ids!(info_ch), &ch);
        // The list's name on its chooser, and its total time.
        let (title, _) = self.model.heading();
        let total_ms: u64 = self
            .model
            .rows
            .iter()
            .filter_map(|row| match row {
                crate::model::Row::Track { id, .. } => self.model.index.tracks.get(*id as usize).map(|t| t.duration_ms as u64),
                _ => None,
            })
            .sum();
        let _ = title;
        let total = if total_ms > 0 { format!("{} · {}", crate::widgets::tracks_word(self.model.rows.len() as u64), crate::model::duration(total_ms)) } else { String::new() };
        self.set_label(cx, live_id!(pl_total), ids!(pl_total), &total);
    }

    fn show_pl_lists(&mut self, cx: &mut Cx, on: bool) {
        state(cx).small_lists = on;
        self.set_visible(cx, ids!(pl_lib), on);
        self.set_visible(cx, ids!(plist), !on);
        self.ui.relief_view(cx, ids!(pl_lists_cap)).set_lit(cx, if on { 1.0 } else { 0.0 });
        let search = self.ui.text_input(cx, ids!(pl_search));
        search.set_text(cx, "");
        self.ui.library_list(cx, ids!(pl_lib)).set_filter(cx, "");
        if !on {
            self.model.set_query("");
        }
        self.ui.redraw(cx);
    }

    /// The EQ panel flat in a band (the small window), or on
    /// their own plates with their own keys (elsewhere).
    pub(crate) fn apply_flat_panels(&mut self, cx: &mut Cx, flat: bool, band: bool) {
        // A restyle puts the templates back: the style is part of the key.
        let style = hw::current(cx);
        let s = state(cx);
        let key = Some((flat, band, style));
        if s.flat == key {
            return;
        }
        s.flat = key;
        let show = !flat;
        for id in [ids!(eq_plate), ids!(eq_well)] {
            let mut w = self.ui.widget(cx, id);
            script_apply_eval!(cx, w, {show_bg: #(show)});
        }
        let pad = if flat { 2.0 } else { 16.0 };
        for id in [ids!(eq_plate)] {
            let mut w = self.ui.widget(cx, id);
            script_apply_eval!(cx, w, {padding: mod.turtle.Inset{left: #(pad) right: #(pad) top: #(pad) bottom: #(pad)}});
        }
    }

    pub(crate) fn panels_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // The section keys set the bands and, like a desktop player's,
        // resize the window to them: the top edge stays, the bottom moves by
        // the band's height.
        let small = layout::active_class(cx) == SizeClass::Small;
        let eq_key = self.ui.button(cx, ids!(tog_eq)).clicked(actions);
        let eq_fold = self.ui.button(cx, ids!(eq_fold)).clicked(actions);
        let pl_key = self.ui.button(cx, ids!(tog_pl)).clicked(actions);
        let pl_fold = self.ui.button(cx, ids!(pl_fold)).clicked(actions);
        // VIS: the visualizer band on or off (the picture's mode, kept).
        if self.ui.button(cx, ids!(tog_vis)).clicked(actions) || self.ui.button(cx, ids!(vis_fold)).clicked(actions) {
            let on = !self.milk.settings.on;
            self.set_visualizer(cx, on);
        }
        let (eq_was, pl_was) = {
            let s = state(cx);
            (s.small_eq, s.small_pl)
        };
        let mut eq_now = eq_was;
        let mut pl_now = pl_was;
        if eq_key {
            eq_now = !eq_was;
        }
        if eq_fold {
            eq_now = false;
        }
        if pl_key {
            pl_now = !pl_was;
            // On its side the playlist and the picture take turns.
            let size = self.window_size(cx);
            if size.x > size.y && self.milk.settings.on {
                self.set_visualizer(cx, false);
                pl_now = true;
            }
        }
        if pl_fold {
            pl_now = false;
        }
        if (eq_now, pl_now) != (eq_was, pl_was) {
            let pl_band_h = self.ui.hw_stage(cx, ids!(stage)).shown("plist").map(|r| r.size.y + 64.0);
            let size = self.window_size(cx);
            let s = state(cx);
            s.small_manual = true;
            s.small_eq = eq_now;
            s.small_pl = pl_now;
            if small {
                if let (true, Some(h)) = (pl_was && !pl_now, pl_band_h) {
                    s.pl_h = h.max(160.0);
                }
                let pl_h = if s.pl_h > 0.0 { s.pl_h } else { 300.0 };
                let mut h = size.y;
                if eq_now != eq_was {
                    h += if eq_now { EQ_BAND_H } else { -EQ_BAND_H };
                }
                if pl_now != pl_was {
                    h += if pl_now { pl_h } else { -pl_h };
                }
                let h = h.max(180.0);
                s.expect_h = Some(h);
                s.expect_reached = false;
                self.ui.window(cx, ids!(main_window)).resize(cx, dvec2(size.x, h));
            }
            self.model.requests.redraw = true;
        }
        // The playlist band: PLAYLISTS swaps its list for the playlists
        // (numbered, filtered by the search); a pick loads it and goes back
        // to the tracks, as do the key again and Escape.
        if self.ui.button(cx, ids!(pl_lists)).clicked(actions) {
            let on = !state(cx).small_lists;
            self.show_pl_lists(cx, on);
        }
        if self.ui.library_list(cx, ids!(pl_lib)).picked(actions).is_some() {
            self.show_pl_lists(cx, false);
        }
        let search = self.ui.text_input(cx, ids!(pl_search));
        if let Some(query) = search.changed(actions) {
            if state(cx).small_lists {
                self.ui.library_list(cx, ids!(pl_lib)).set_filter(cx, &query);
            } else {
                self.model.set_query(&query);
            }
        }
        if search.escaped(actions) {
            if state(cx).small_lists {
                self.show_pl_lists(cx, false);
            } else {
                self.model.set_query("");
            }
            search.set_text(cx, "");
        }
        for (page, key, icon) in [("tracks", ids!(tab_tracks), ids!(tab_tracks_icon)), ("eq", ids!(tab_eq), ids!(tab_eq_icon))] {
            if self.ui.button(cx, key).clicked(actions) || self.ui.button(cx, icon).clicked(actions) {
                self.show_page(cx, page);
                let s = state(cx);
                s.last.insert("page".to_string(), page.to_string());
                save(s);
            }
        }
    }
}
