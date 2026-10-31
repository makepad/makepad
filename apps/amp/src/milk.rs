//! MilkDrop on the equalizer page: the page shows either the spectrum or a
//! MilkDrop preset (`makepad-milkdrop`), fed with the same sound the
//! spectrum reads — the analyzer tap after the equalizer, of the routed
//! player or of a local file.
//!
//! The preset browser is the library's own `MilkdropBrowser`: the pack, its
//! index and the person's favourites live in the MilkDrop data folder every
//! app shares, so a pack downloaded in the explorer is here too, and the
//! other way round.
//!
//! No repaint at rest: samples are handed over, and the view animates, only
//! while sound arrives on a visible view. Silence, a paused player, the
//! equalizer off or another page freeze the last frame. The next worker
//! message (a playback poll, a local file starting) looks again.

use crate::layout::{self, HwStageWidgetRefExt};
use crate::model::Page;
use crate::worker::Setting;
use crate::App;
use makepad_milkdrop::{MilkdropBrowserAction, MilkdropBrowserWidgetRefExt, MilkdropViewWidgetRefExt, PresetPick};
use makepad_widgets::*;

/// Samples arrive and the view animates for this long after the last
/// sound.
const STALE: f64 = 0.5;
/// Samples no louder than this are silence (a paused player's blocks).
const SILENCE: f32 = 1.0e-5;
/// The most samples handed over in one frame (a frame after a long pause
/// takes only the newest).
const MAX_FEED: usize = 8192;
/// The automatic preset change choices, in seconds (0 = off).
pub const AUTO_CHOICES: [u32; 5] = [0, 15, 30, 60, 120];
/// The resolution choices, as scales of the view's pixels.
/// The picture always renders at full resolution (a saved smaller scale
/// from an older run is ignored).
pub const SCALE_CHOICES: [f64; 1] = [1.0];

/// The presets Amp ships, its own: the picture works before (and without)
/// a preset pack. Plain MilkDrop 1 presets (no shaders, so no blur
/// passes): a warp and a composite pass at the render size, cheap enough
/// for a phone.
pub const BUILTIN: [(&str, &str); 6] = [
    ("Ember Tunnel", include_str!("../resources/visualizer/ember_tunnel.milk")),
    ("Aurora", include_str!("../resources/visualizer/aurora.milk")),
    ("Orbit Lines", include_str!("../resources/visualizer/orbit_lines.milk")),
    ("Ripple Pool", include_str!("../resources/visualizer/ripple_pool.milk")),
    ("Prism Bloom", include_str!("../resources/visualizer/prism_bloom.milk")),
    ("Starfield", include_str!("../resources/visualizer/starfield.milk")),
];

/// The most pixels the picture renders at on a phone (it is scaled up to
/// the view): a feedback picture is soft anyway, and the GPU work stays a
/// fraction of a frame. On a Pixel (PowerVR) Amp holds 60 fps with the
/// picture fullscreen at this size (and also at the screen's full
/// 1080x2404, which costs ~11x the fill for a slightly crisper line).
pub const PHONE_PIXELS: f64 = 480.0 * 480.0;

/// A finger that moves this far sideways on the picture is a swipe (next
/// or previous preset), less is a tap (fullscreen).
const SWIPE: f64 = 48.0;

/// What the page keeps between runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MilkSettings {
    /// MilkDrop instead of the spectrum.
    pub on: bool,
    /// Index into [`AUTO_CHOICES`].
    pub auto: usize,
    /// Keep the current preset: no automatic change.
    pub locked: bool,
    /// Index into [`SCALE_CHOICES`].
    pub scale: usize,
    /// The preset browser shown under the picture.
    pub browser: bool,
    /// The app last closed on the equalizer page.
    pub on_page: bool,
}

impl Default for MilkSettings {
    fn default() -> Self {
        Self { on: false, auto: 2, locked: false, scale: 0, browser: false, on_page: false }
    }
}

impl MilkSettings {
    pub fn serialize(&self) -> String {
        format!(
            "mode {}\nauto {}\nlock {}\nscale {}\nbrowser {}\npage {}\n",
            if self.on { "milkdrop" } else { "spectrum" },
            AUTO_CHOICES[self.auto],
            self.locked as u8,
            SCALE_CHOICES.get(self.scale).copied().unwrap_or(1.0),
            self.browser as u8,
            if self.on_page { "equalizer" } else { "other" },
        )
    }

    /// Read back what [`Self::serialize`] wrote; unknown lines and values
    /// keep the defaults.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let (Some(key), Some(value)) = (parts.next(), parts.next()) else { continue };
            match key {
                "mode" => settings.on = value == "milkdrop",
                "auto" => {
                    if let Some(at) = value.parse::<u32>().ok().and_then(|secs| AUTO_CHOICES.iter().position(|c| *c == secs)) {
                        settings.auto = at;
                    }
                }
                "lock" => settings.locked = value == "1",
                "scale" => {
                    if let Some(at) = value.parse::<f64>().ok().and_then(|s| SCALE_CHOICES.iter().position(|c| (*c - s).abs() < 1e-6)) {
                        settings.scale = at.min(SCALE_CHOICES.len() - 1);
                    }
                }
                "browser" => settings.browser = value == "1",
                "page" => settings.on_page = value == "equalizer",
                _ => {}
            }
        }
        settings
    }
}

/// The feed between the analyzer tap and the view.
#[derive(Default)]
pub struct Milk {
    pub settings: MilkSettings,
    /// The window shows the picture alone.
    pub fullscreen: bool,
    /// The analyzer's sample count as last read.
    written: u64,
    /// When sound last arrived, in app seconds.
    fresh_at: Option<f64>,
    /// When the current preset was loaded, for the automatic change.
    loaded_at: f64,
    /// Samples read this frame.
    buffer: Vec<f32>,
    frame: NextFrame,
    /// The view's clock runs.
    playing: bool,
    /// A preset has been handed to the view.
    loaded: bool,
    /// The built-in preset showing, while no pack is installed.
    builtin: Option<usize>,
    /// Where a finger went down on the picture.
    press: Option<DVec2>,
}

impl App {
    /// The screen always shows its picture: MilkDrop runs whenever it is
    /// the picture chosen (and sound flows).
    fn milk_visible(&self) -> bool {
        self.milk.settings.on
    }

    /// Read what arrived from the tap since the last frame, hand it to the
    /// view, and run or stop the view's clock. Asks for the next frame only
    /// while sound flows into a visible view.
    pub(crate) fn drive_milk(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        let visible = self.milk_visible();
        let view = self.ui.milkdrop_view(cx, ids!(milk));
        if visible {
            if let Some(analyzer) = self.model.sound() {
                let written = analyzer.written();
                if written != self.milk.written {
                    let fresh = if self.milk.written == 0 || written < self.milk.written { 2048 } else { (written - self.milk.written) as usize };
                    self.milk.written = written;
                    let count = fresh.min(MAX_FEED);
                    self.milk.buffer.resize(count, 0.0);
                    analyzer.latest(&mut self.milk.buffer);
                    if self.milk.buffer.iter().any(|s| s.abs() > SILENCE) {
                        self.milk.fresh_at = Some(now);
                        view.feed_audio(&self.milk.buffer, analyzer.sample_rate().max(8000.0));
                    }
                }
            }
        }
        let sounding = visible && self.milk.fresh_at.is_some_and(|at| now - at < STALE);
        if sounding != self.milk.playing {
            self.milk.playing = sounding;
            view.set_playing(cx, sounding);
        }
        if sounding {
            self.milk_auto_change(cx, now);
            self.milk.frame = cx.new_next_frame();
        }
    }

    /// Change preset when the automatic change is due.
    fn milk_auto_change(&mut self, cx: &mut Cx, now: f64) {
        let secs = AUTO_CHOICES[self.milk.settings.auto];
        if secs == 0 || self.milk.settings.locked || !self.milk.loaded || now - self.milk.loaded_at < secs as f64 {
            return;
        }
        self.visual_random(cx);
    }

    /// No pack on disk: the built-in presets are the list.
    fn milk_builtin(&self, cx: &mut Cx) -> bool {
        self.ui.milkdrop_browser(cx, ids!(milk_browser)).needs_pack()
    }

    /// The next (`1`) or previous (`-1`) preset. The wing's and the deck's
    /// keys, a swipe on the picture, and the arrows in a fullscreen view.
    pub(crate) fn visual_step(&mut self, cx: &mut Cx, delta: isize) {
        if self.milk_builtin(cx) {
            let n = BUILTIN.len() as isize;
            let at = match self.milk.builtin {
                Some(at) => (at as isize + delta).rem_euclid(n) as usize,
                None => 0,
            };
            self.milk_load_builtin(cx, at);
            return;
        }
        let pick = self.ui.milkdrop_browser(cx, ids!(milk_browser)).step(cx, delta);
        self.milk_load(cx, pick);
    }

    /// A random preset (another one than showing).
    fn visual_random(&mut self, cx: &mut Cx) {
        if self.milk_builtin(cx) {
            let n = BUILTIN.len();
            let skip = 1 + (cx.seconds_since_app_start() * 1000.0) as usize % (n - 1);
            self.milk_load_builtin(cx, self.milk.builtin.map_or(0, |at| (at + skip) % n));
            return;
        }
        let pick = self.ui.milkdrop_browser(cx, ids!(milk_browser)).random(cx);
        self.milk_load(cx, pick);
    }

    /// Show the built-in preset `at`.
    fn milk_load_builtin(&mut self, cx: &mut Cx, at: usize) {
        let Some((name, source)) = BUILTIN.get(at) else { return };
        self.milk.builtin = Some(at);
        self.milk.loaded = true;
        self.milk.loaded_at = cx.seconds_since_app_start();
        self.ui.milkdrop_view(cx, ids!(milk)).load_preset_source(cx, name, source);
        self.set_label(cx, live_id!(milk_now), ids!(milk_now), name);
        self.update_milk(cx);
    }

    /// Load `pick` into the view (read and parsed off the UI thread, then
    /// faded to).
    fn milk_load(&mut self, cx: &mut Cx, pick: Option<PresetPick>) {
        let Some(pick) = pick else { return };
        self.milk.builtin = None;
        self.milk.loaded = true;
        self.milk.loaded_at = cx.seconds_since_app_start();
        self.ui.milkdrop_view(cx, ids!(milk)).load_preset(cx, &pick.path);
        self.set_label(cx, live_id!(milk_now), ids!(milk_now), &pick.line());
        self.update_milk(cx);
    }

    /// The page's layout and controls, from the settings.
    pub(crate) fn update_milk(&mut self, cx: &mut Cx) {
        let settings = self.milk.settings;
        self.set_visible(cx, ids!(visual), !settings.on);
        self.set_visible(cx, ids!(milk_stage), settings.on);
        // Without a pack the built-in presets play; the presets drawer
        // (where the pack download is offered) opens only when asked for.
        if settings.on && !self.milk.loaded && self.milk_builtin(cx) {
            self.milk_load_builtin(cx, 0);
            return;
        }
        // The presets open in a drawer beside the screen, over the right of
        // the device: the screen neither moves nor changes size.
        let drawer = settings.on && settings.browser;
        self.set_visible(cx, ids!(milk_drawer), drawer && !self.milk.fullscreen);
        // Fullscreen: the screen alone, the rest of the device folded away.
        let full = self.milk.fullscreen;
        if self.milk_full_applied != Some(full) {
            self.milk_full_applied = Some(full);
            // Only on the switch: the components' own fits decide the rest
            // (a deck's volume is a slider, not the wing).
            for path in [ids!(left_wing), ids!(right_wing), ids!(keys), ids!(now_bar), ids!(lower)] {
                self.set_visible(cx, path, !full);
            }
            self.fits_applied = None;
            // The device takes the whole front: the lists fold away.
            let split = self.ui.splitter(cx, ids!(device_split));
            split.set_collapse(cx, if full { SplitterCollapse::B } else { SplitterCollapse::None });
        }
        if !full {
            self.apply_up_next(cx);
        }
        let theme = self.model.theme;
        let (favourite, _) = self.ui.milkdrop_browser(cx, ids!(milk_browser)).current_marks();
        let fav_color = if favourite { theme.legend_on } else { theme.legend_off };
        let lock_color = if settings.locked { theme.legend_on } else { theme.legend_off };
        let list_color = if settings.browser { theme.legend_on } else { theme.legend_off };
        for (path, on) in [(ids!(milk_fav_cap), favourite), (ids!(milk_lock_cap), settings.locked), (ids!(milk_list_cap), settings.browser)] {
            self.ui.relief_view(cx, path).set_lit(cx, if on { 1.0 } else { 0.0 });
        }
        let colors = [fav_color, lock_color, list_color];
        if self.milk_colors != Some(colors) {
            self.milk_colors = Some(colors);
            for (path, color) in [(ids!(milk_fav), fav_color), (ids!(milk_lock), lock_color), (ids!(milk_list), list_color)] {
                let mut button = self.ui.widget(cx, path);
                script_apply_eval!(cx, button, {draw_text +: {color: #(color)}});
            }
        }
        self.set_label(cx, live_id!(milk_full), ids!(milk_full), if full { "\u{f066}" } else { "\u{f065}" });
        self.ui.segmented_control(cx, ids!(eq_view)).set_selected(cx, settings.on as usize);
        self.ui.segmented_control(cx, ids!(deck_mode)).set_selected(cx, settings.on as usize);
        self.ui.relief_view(cx, ids!(deck_presets_cap)).set_lit(cx, if drawer { 1.0 } else { 0.0 });
        self.ui.relief_view(cx, ids!(deck_presets_icon_cap)).set_lit(cx, if drawer { 1.0 } else { 0.0 });
        self.ui.drop_down(cx, ids!(milk_auto)).set_selected_item(cx, settings.auto);
        let every = AUTO_CHOICES[settings.auto];
        self.set_label(cx, live_id!(deck_auto), ids!(deck_auto), &format!("CHANGE: {}", auto_words(every).to_uppercase()));
        let view = self.ui.milkdrop_view(cx, ids!(milk));
        view.set_resolution_scale(cx, SCALE_CHOICES.get(settings.scale).copied().unwrap_or(1.0));
        view.set_max_pixels(cx, Self::milk_max_pixels(cx));
        if layout::active_class(cx) == layout::SizeClass::Small {
            self.apply_small_sections(cx);
        }
        self.drive_milk(cx);
    }

    /// On a phone the picture renders at no more than [`PHONE_PIXELS`]
    /// (the view keeps that budget at any size: rotation, fullscreen).
    fn milk_max_pixels(cx: &Cx) -> f64 {
        if matches!(cx.os_type(), OsType::Android(_)) {
            PHONE_PIXELS
        } else {
            0.0
        }
    }

    fn save_milk(&mut self) {
        let mut settings = self.milk.settings;
        settings.on_page = self.model.page == Page::Equalizer;
        self.save(Setting::Visualizer, settings.serialize());
    }

    /// Settings read back at start: the mode, and the page if the app last
    /// closed on it.
    pub(crate) fn restore_milk(&mut self, cx: &mut Cx, text: &str) {
        let settings = MilkSettings::parse(text);
        self.milk.settings = settings;
        if settings.on_page && self.model.page != Page::Equalizer {
            self.model.open(Page::Equalizer);
        }
        self.model.requests.redraw = true;
        self.update_milk(cx);
    }

    /// The page's buttons, the browser's picks and the view switch.
    pub(crate) fn milk_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let browser = self.ui.milkdrop_browser(cx, ids!(milk_browser));
        if let Some(MilkdropBrowserAction::NeedsPack) = actions.find_widget_action(browser.widget_uid()).map(|a| a.cast::<MilkdropBrowserAction>()) {
            self.update_milk(cx);
        }
        if let Some(pick) = browser.played(actions) {
            // The browser's first pick arrives when its library loads; only
            // the first one or a person's pick changes the picture.
            self.milk_load(cx, Some(pick));
        }
        let mut changed = false;
        // The theatre deck's controls do what the display wing's do.
        if let Some(at) = self.ui.segmented_control(cx, ids!(deck_mode)).selected(actions) {
            self.milk.settings.on = at == 1;
            changed = true;
        }
        if self.ui.button(cx, ids!(deck_presets)).clicked(actions) || self.ui.button(cx, ids!(deck_presets_icon)).clicked(actions) {
            self.milk.settings.browser = !self.milk.settings.browser;
            changed = true;
        }
        if self.ui.button(cx, ids!(deck_eq)).clicked(actions) || self.ui.button(cx, ids!(deck_eq_icon)).clicked(actions) {
            let (_, page) = self.ui.hw_stage(cx, ids!(stage)).panels();
            self.show_page(cx, if page == "eq" { "tracks" } else { "eq" });
        }
        if let Some(at) = self.ui.segmented_control(cx, ids!(eq_view)).selected(actions) {
            self.milk.settings.on = at == 1;
            changed = true;
        }
        // The wing's keys, and the theatre deck's.
        let next = self.ui.button(cx, ids!(milk_next)).clicked(actions) || self.ui.button(cx, ids!(deck_next)).clicked(actions);
        let prev = self.ui.button(cx, ids!(milk_prev)).clicked(actions) || self.ui.button(cx, ids!(deck_prev)).clicked(actions);
        let random = self.ui.button(cx, ids!(milk_random)).clicked(actions) || self.ui.button(cx, ids!(deck_random)).clicked(actions);
        if next {
            self.visual_step(cx, 1);
        }
        if prev {
            self.visual_step(cx, -1);
        }
        if random {
            self.visual_random(cx);
        }
        if self.ui.button(cx, ids!(milk_fav)).clicked(actions) {
            browser.toggle_favourite(cx);
            self.update_milk(cx);
        }
        if self.ui.button(cx, ids!(milk_lock)).clicked(actions) {
            self.milk.settings.locked = !self.milk.settings.locked;
            changed = true;
        }
        if self.ui.button(cx, ids!(milk_drawer_close)).clicked(actions) {
            self.milk.settings.browser = false;
            changed = true;
        }
        if self.ui.button(cx, ids!(milk_list)).clicked(actions) {
            self.milk.settings.browser = !self.milk.settings.browser;
            changed = true;
        }
        if self.ui.button(cx, ids!(milk_full)).clicked(actions) {
            self.set_milk_fullscreen(cx, !self.milk.fullscreen);
        }
        // The theatre deck's CHANGE key: the same choices, as a menu.
        if self.ui.button(cx, ids!(deck_auto)).clicked(actions) || self.ui.button(cx, ids!(deck_auto_icon)).clicked(actions) {
            let rows = AUTO_CHOICES
                .iter()
                .enumerate()
                .map(|(i, secs)| MenuRow::new(LiveId(i as u64 + 1), &format!("Change: {}", auto_words(*secs))).radio(i == self.milk.settings.auto))
                .collect();
            let key = if self.ui.widget(cx, ids!(deck_auto_cap)).visible() { ids!(deck_auto_cap) } else { ids!(deck_auto_icon_cap) };
            let anchor = self.ui.widget(cx, key).area().rect(cx);
            self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(deck_auto_menu), rows, anchor, MenuPlace::Below);
        }
        // A narrow deck's menu: the keys it has no room for.
        if self.ui.button(cx, ids!(deck_more)).clicked(actions) {
            let tabbed = self.ui.hw_stage(cx, ids!(stage)).panels().0.len() > 1;
            let mut rows = vec![
                MenuRow::new(live_id!(deck_m_random), "Random preset"),
                MenuRow::new(live_id!(deck_m_presets), "Presets…").checked(self.milk.settings.browser),
                MenuRow::new(live_id!(deck_m_map), "Theme colours").checked(crate::hw::color_map_on(cx)),
            ];
            if !tabbed {
                rows.push(MenuRow::new(live_id!(deck_m_eq), "Equalizer"));
            }
            rows.push(MenuRow::separator());
            for (i, secs) in AUTO_CHOICES.iter().enumerate() {
                rows.push(MenuRow::new(LiveId(i as u64 + 1), &format!("Change: {}", auto_words(*secs))).radio(i == self.milk.settings.auto));
            }
            let anchor = self.ui.widget(cx, ids!(deck_more_cap)).area().rect(cx);
            self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(deck_more_menu), rows, anchor, MenuPlace::Below);
        }
        if let Some(id) = menu_picked(actions, live_id!(deck_more_menu)) {
            if id == live_id!(deck_m_random) {
                self.visual_random(cx);
            } else if id == live_id!(deck_m_presets) {
                self.milk.settings.browser = !self.milk.settings.browser;
                changed = true;
            } else if id == live_id!(deck_m_map) {
                let on = !crate::hw::color_map_on(cx);
                crate::hw::set_color_map(cx, on);
                self.apply_color_map(cx);
            } else if id == live_id!(deck_m_eq) {
                let (_, page) = self.ui.hw_stage(cx, ids!(stage)).panels();
                self.show_page(cx, if page == "eq" { "tracks" } else { "eq" });
            } else {
                let at = (id.0 as usize).saturating_sub(1).min(AUTO_CHOICES.len() - 1);
                let auto = self.ui.drop_down(cx, ids!(milk_auto));
                auto.set_selected_item(cx, at);
                cx.widget_action(auto.widget_uid(), DropDownAction::Select(at));
            }
        }
        if let Some(id) = menu_picked(actions, live_id!(deck_auto_menu)) {
            let at = (id.0 as usize).saturating_sub(1).min(AUTO_CHOICES.len() - 1);
            let auto = self.ui.drop_down(cx, ids!(milk_auto));
            auto.set_selected_item(cx, at);
            cx.widget_action(auto.widget_uid(), DropDownAction::Select(at));
        }
        if let Some(at) = self.ui.drop_down(cx, ids!(milk_auto)).selected(actions) {
            self.milk.settings.auto = at.min(AUTO_CHOICES.len() - 1);
            self.milk.loaded_at = cx.seconds_since_app_start();
            changed = true;
        }
        if changed {
            self.save_milk();
            self.update_milk(cx);
        }
    }

    /// The picture on (the visualizer) or off (the spectrum), kept for
    /// the next run.
    pub(crate) fn set_visualizer(&mut self, cx: &mut Cx, on: bool) {
        self.milk.settings.on = on;
        if !on {
            self.milk.fullscreen = false;
        }
        self.save_milk();
        self.update_milk(cx);
        self.ui.redraw(cx);
    }

    pub(crate) fn set_milk_fullscreen(&mut self, cx: &mut Cx, full: bool) {
        if self.milk.fullscreen == full {
            return;
        }
        self.milk.fullscreen = full;
        self.update_milk(cx);
        self.ui.redraw(cx);
    }

    /// Frames of the feed, and the page changing under it. On the picture
    /// a tap toggles fullscreen and a sideways swipe changes the preset.
    pub(crate) fn milk_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.milk.frame.is_event(event).is_some() {
            self.drive_milk(cx);
        }
        if !self.milk.settings.on {
            return;
        }
        let area = self.ui.widget(cx, ids!(milk_frame)).area();
        match event.hits(cx, area) {
            Hit::FingerDown(e) => {
                self.milk.press = Some(e.abs);
            }
            Hit::FingerUp(e) => {
                let Some(start) = self.milk.press.take() else { return };
                let moved = e.abs - start;
                if moved.x.abs() >= SWIPE && moved.x.abs() > moved.y.abs() {
                    self.visual_step(cx, if moved.x < 0.0 { 1 } else { -1 });
                } else if e.is_over && moved.length() < SWIPE {
                    let full = !self.milk.fullscreen;
                    self.set_milk_fullscreen(cx, full);
                }
            }
            _ => {}
        }
    }

    /// The page opened or closed: remember it for the next run.
    pub(crate) fn milk_page_changed(&mut self, cx: &mut Cx) {
        if !self.milk_visible() && self.milk.fullscreen {
            self.milk.fullscreen = false;
        }
        self.save_milk();
        self.update_milk(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_milkdrop::expr::{Context, Program};
    use makepad_milkdrop::Preset;

    #[test]
    fn built_in_presets_parse_and_every_equation_compiles() {
        for (name, source) in BUILTIN {
            let preset = Preset::parse(source.as_bytes());
            assert!(preset.unknown.is_empty(), "{name}: unknown lines {:?}", preset.unknown);
            assert!(!preset.per_frame.trim().is_empty(), "{name}: no per-frame code");
            let mut blocks = vec![&preset.per_frame_init, &preset.per_frame, &preset.per_pixel];
            for wave in &preset.waves {
                blocks.extend([&wave.init, &wave.per_frame, &wave.per_point]);
            }
            for shape in &preset.shapes {
                blocks.extend([&shape.init, &shape.per_frame]);
            }
            for block in blocks {
                if block.trim().is_empty() {
                    continue;
                }
                let mut context = Context::new(1);
                if let Err(e) = Program::compile(block, &mut context) {
                    panic!("{name}: {} in {block}", e.message);
                }
            }
        }
    }

    #[test]
    fn settings_read_back_and_junk_keeps_defaults() {
        let settings = MilkSettings { on: true, auto: 3, locked: true, scale: 0, browser: true, on_page: true };
        assert_eq!(MilkSettings::parse(&settings.serialize()), settings);
        let read = MilkSettings::parse("mode milkdrop\nauto 17\nscale 9\nwhat ever\n");
        assert!(read.on);
        assert_eq!(read.auto, MilkSettings::default().auto);
        assert_eq!(read.scale, MilkSettings::default().scale);
        assert_eq!(MilkSettings::parse(""), MilkSettings::default());
    }
}

/// An automatic change's interval in words: "never", "15 s", "1 min".
fn auto_words(secs: u32) -> String {
    match secs {
        0 => "never".to_string(),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}
