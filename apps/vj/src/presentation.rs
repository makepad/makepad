//! State-preserving layout and appearance coordination for the console. The
//! individual modes own their layout; this module owns navigation and reloads.
use crate::*;

#[derive(Default)]
pub(super) struct Presentation {
    size: Vec2d,
    pub compact: bool,
    pub page: usize,
    mode: LiveId,
    generation: u64,
    dirty: bool,
    pub video_split: Option<f64>,
    synth_track: Option<SynthTrack>,
}

impl App {
    pub(super) fn presentation_startup_size(&mut self, cx: &mut Cx) {
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            let value = match arg.strip_prefix("--size=") {
                Some(value) => value.to_owned(),
                None if arg == "--size" => args.next().unwrap_or_default(),
                _ => continue,
            };
            let Some((w, h)) = value.split_once(['x', 'X']) else { continue; };
            let (Ok(w), Ok(h)) = (w.parse::<f64>(), h.parse::<f64>()) else { continue; };
            if w.is_finite() && h.is_finite() && w >= 280.0 && h >= 240.0 {
                self.ui.window(cx, ids!(main_window)).resize(cx, dvec2(w, h));
            }
        }
    }

    pub(super) fn presentation_before_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::WindowGeomChange(e) = event {
            if self.ui.window(cx, ids!(main_window)).window_id() == Some(e.window_id) {
                let native = cx.windows[e.window_id].native_dpi_factor();
                let size = e.new_geom.inner_size * (e.new_geom.dpi_factor / native);
                if size.x > 1.0 && size.y > 1.0 && size != self.presentation.size {
                    if !self.presentation.compact {
                        if let Some(SplitterAlign::FromA(value)) = self.ui.splitter(cx, ids!(deck_split)).align() {
                            self.presentation.video_split = Some(value);
                        }
                    }
                    self.presentation.size = size;
                    self.presentation.compact = size.x < 980.0 || size.y < 500.0;
                    self.presentation.dirty = true;
                    crate::theme::set_compact(cx, self.presentation.compact);
                }
            }
        }
        let generation = crate::theme::generation(cx);
        if generation != self.presentation.generation {
            self.presentation.generation = generation;
            self.presentation.dirty = true;
            self.lit_state.clear();
            self.icon_color.clear();
            self.stem_state_painted = [None; 2];
            self.kar_title_painted = [None; 2];
            self.strip_shape = [None, None];
            let text = if crate::theme::is_custom(cx) { "Black / orange" } else { "System theme" };
            self.ui.button(cx, ids!(appearance_toggle)).set_text(cx, text);
            self.paint_tabs(cx, self.console_page);
        }
        if self.console_page != self.presentation.mode {
            self.presentation.mode = self.console_page;
            self.presentation.page = 0;
            self.presentation.dirty = true;
        }
    }

    pub(super) fn presentation_after_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.presentation.synth_track != Some(self.synth_mix.selected) {
            self.presentation.synth_track = Some(self.synth_mix.selected);
            self.presentation.dirty = true;
        }
        if self.console_page != self.presentation.mode {
            self.presentation.mode = self.console_page;
            self.presentation.page = 0;
            self.presentation.dirty = true;
        }
        // PageFlip constructs a page on its first draw. Apply the layout after
        // that draw so the same resident widgets are available on every path.
        if matches!(event, Event::Draw(_)) && self.presentation.dirty {
            self.presentation.dirty = false;
            self.apply_presentation(cx);
            self.ui.redraw(cx);
        }
    }

    pub(super) fn presentation_invalidate(&mut self, cx: &mut Cx) {
        self.presentation.dirty = true;
        self.ui.redraw(cx);
    }

    pub(super) fn presentation_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.ui.button(cx, ids!(appearance_toggle)).clicked(actions) {
            crate::theme::toggle(cx);
            return true;
        }
        if !self.presentation.compact { return false; }
        for (page, id) in [ids!(compact_tab_0), ids!(compact_tab_1), ids!(compact_tab_2), ids!(compact_tab_3)].into_iter().enumerate() {
            if self.ui.button(cx, id).clicked(actions) {
                self.presentation.page = page;
                self.presentation.dirty = true;
                self.ui.redraw(cx);
                return true;
            }
        }
        if self.ui.button(cx, ids!(gen_fold)).clicked(actions) {
            self.presentation.page = if self.presentation.page == 3 { 0 } else { 3 };
            self.presentation.dirty = true;
            self.ui.redraw(cx);
            return true;
        }
        false
    }

    fn apply_presentation(&mut self, cx: &mut Cx) {
        let size = self.presentation.size;
        if size.x <= 1.0 || size.y <= 1.0 { return; }
        let compact = self.presentation.compact;
        let page = self.presentation.page;
        crate::console_layout::apply(self, cx, size, compact, page);
        crate::music_responsive::apply(self, cx, size, compact, page);
        self.ui.widget(cx, ids!(compact_nav)).set_visible(cx, compact);
        let labels = match self.console_page {
            live_id!(music_page) => ["Perform", "Library", "Decks", "Create"],
            live_id!(synth_page) => ["Sequence", "Sound", "Rack", "Create"],
            live_id!(mix_page) => ["Channels", "Master", "Meters", "Create"],
            _ => ["Perform", "Library", "Effects", "Create"],
        };
        for (index, id) in [ids!(compact_tab_0), ids!(compact_tab_1), ids!(compact_tab_2), ids!(compact_tab_3)].into_iter().enumerate() {
            self.ui.widget(cx, id).set_visible(cx, index != 2 || self.console_page != live_id!(mix_page));
            self.ui.button(cx, id).set_text(cx, labels[index]);
            self.paint_lit(cx, id, page == index);
        }
        let mut split = self.ui.widget(cx, ids!(gen_split));
        if compact {
            let width = if page == 3 { (size.x - 16.0).max(1.0) } else { 0.0 };
            script_apply_eval!(cx, split, {
                align: mod.widgets.SplitterAlign.FromA(#(width))
                min_vertical: 0.0 max_vertical: 100000.0 size: 0.0
            });
        } else {
            let width = if self.gen_panel_open { self.gen_panel_width.max(240.0) } else { 0.0 };
            script_apply_eval!(cx, split, {
                align: mod.widgets.SplitterAlign.FromA(#(width))
                min_vertical: 0.0 max_vertical: 360.0 size: 6.0
            });
        }
    }
}
