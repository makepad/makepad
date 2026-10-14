//! The app's own widgets: the playlist list in the sidebar, the track table,
//! and the bar used for the song position and the volume. Each reads the
//! [`Model`] out of the `Scope` `App` passes down; none keeps data of its
//! own beyond what it last drew.
//!
//! Both lists are `DataGrid`s: they only lay out and draw the rows in view,
//! so a playlist of thousands of tracks scrolls as fast as one of ten.

use crate::columns::{self, Column};
use crate::model::{duration, Model, Page, Row};
use crate::{with_alpha, Theme};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A filled rounded box: the bar tracks, the handle, the art frame.
    mod.widgets.DrawMusicRound = set_type_default() do #(DrawMusicRound::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let r = min(self.radius, min(self.rect_size.x, self.rect_size.y) * 0.5)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, r)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.MusicBarBase = #(MusicBar::register_widget(vm))
    mod.widgets.MusicBar = set_type_default() do mod.widgets.MusicBarBase{
        width: Fill
        height: 16
        draw_round: mod.widgets.DrawMusicRound{}
    }

    // The grids are lists, not spreadsheets: a press picks the whole row,
    // no row numbers, no zebra, headings only where a table needs them.
    let MusicGrid = DataGrid{
        width: Fill
        height: Fill
        rows: 0
        cols: 1
        show_row_headers: false
        selection: GridSelectMode.Rows
        // Rows are choices: the hand says so.
        cell_cursor: MouseCursor.Hand
        zebra_stripes: false
        allow_col_resize: true
        allow_row_resize: false
        allow_col_reorder: false
        default_col_width: 120.0
        col_header_height: 30.0
        header_align: 0.0
        cell_pad_x: 10.0
        draw_cell +: {border_size: 0.0}
        draw_text +: {text_style: theme.font_regular{font_size: 11.0}}
        draw_text_bold +: {text_style: theme.font_regular{font_size: 10.5}}
        draw_text_header +: {text_style: theme.font_regular{font_size: 10.0}}
    }

    mod.widgets.LibraryListBase = #(LibraryList::register_widget(vm))
    mod.widgets.LibraryList = set_type_default() do mod.widgets.LibraryListBase{
        width: Fill
        height: Fill
        flow: Down
        draw_name +: {text_style: theme.font_regular{font_size: 11.0}}
        draw_count +: {text_style: theme.font_regular{font_size: 9.5}}
        draw_icon +: {text_style: theme.font_icons{font_size: 9.0}}
        grid := MusicGrid{
            show_col_headers: false
            default_row_height: 28.0
        }
    }

    mod.widgets.TrackTableBase = #(TrackTable::register_widget(vm))
    mod.widgets.TrackTable = set_type_default() do mod.widgets.TrackTableBase{
        width: Fill
        height: Fill
        flow: Down
        draw_title +: {text_style: theme.font_regular{font_size: 11.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 10.0}}
        draw_section +: {text_style: theme.font_bold{font_size: 12.5}}
        draw_icon +: {text_style: theme.font_icons{font_size: 10.0}}
        draw_round_rule: mod.widgets.DrawMusicRound{}
        grid := MusicGrid{
            default_row_height: 36.0
            cols: 6
            allow_col_reorder: true
        }
    }

    mod.widgets.UpNextBase = #(UpNext::register_widget(vm))
    mod.widgets.UpNext = set_type_default() do mod.widgets.UpNextBase{
        width: Fill
        height: Fill
        flow: Down
        draw_title +: {text_style: theme.font_regular{font_size: 11.0}}
        draw_small +: {text_style: theme.font_regular{font_size: 9.5}}
        draw_section +: {text_style: theme.font_bold{font_size: 10.5}}
        draw_icon +: {text_style: theme.font_icons{font_size: 9.5}}
        draw_round: mod.widgets.DrawMusicRound{}
        // A list: rows are carried to a new place, no headings.
        grid := MusicGrid{
            show_col_headers: false
            default_row_height: 46.0
            allow_col_resize: false
            row_drag: true
            row_drag_scroll: true
        }
    }
}

/// Instance fields after the draw base, as the instance buffer reads them.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMusicRound {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live(4.0)]
    pub radius: f32,
    /// What this quad is, for a style's `pixel` (the spectrum sets it):
    /// 0 other, 1 well, 2 rule, 3 slot, 4 bar, 5 peak, 6 waveform midline.
    #[live]
    pub part: f32,
    /// The band a slot, bar or peak belongs to, and how many there are.
    #[live]
    pub band: f32,
    #[live]
    pub bands: f32,
    /// The band's level (0..1) and the spectrum's height in points.
    #[live]
    pub level: f32,
    #[live]
    pub span: f32,
}

// ---- glyphs (Font Awesome 5 solid, carried by the theme) ----

pub const ICON_PLAY: &str = "\u{f04b}";
pub const ICON_PAUSE: &str = "\u{f04c}";
pub const ICON_NEXT: &str = "\u{f051}";
pub const ICON_PREVIOUS: &str = "\u{f048}";
pub const ICON_SHUFFLE: &str = "\u{f074}";
pub const ICON_REPEAT: &str = "\u{f01e}";
pub const ICON_VOLUME: &str = "\u{f028}";
/// A radio mast: a track's or the playing track's radio.
pub const ICON_RADIO: &str = "\u{f519}";
/// A numbered list: the Up Next panel.
pub const ICON_QUEUE: &str = "\u{f0cb}";
const ICON_FOLDER: &str = "\u{f07b}";
const ICON_LIST: &str = "\u{f03a}";
const ICON_SPEAKER: &str = "\u{f027}";
/// A track there is a local file of: a drive.
const ICON_LOCAL: &str = "\u{f0a0}";
const ICON_CACHED: &str = "\u{f058}";
const CACHED_INK: Vec4f = Vec4f { x: 0.24, y: 0.80, z: 0.43, w: 1.0 };

/// The row under the pointer in a list's grid (for its hover tint);
/// `true` when it changed (the grid is redrawn).
fn track_hover(cx: &mut Cx, grid: &DataGridRef, event: &Event, row: &mut Option<usize>) -> bool {
    let abs = match event {
        Event::MouseMove(e) => Some(e.abs),
        Event::MouseLeave(_) => None,
        _ => return false,
    };
    track_hover_at(cx, grid, abs, row)
}

fn track_hover_at(cx: &mut Cx, grid: &DataGridRef, abs: Option<DVec2>, row: &mut Option<usize>) -> bool {
    let area = grid.area();
    let next = abs.filter(|abs| area.is_valid(cx) && area.rect(cx).contains(*abs)).and_then(|abs| grid.borrow().and_then(|grid| grid.row_at(abs)));
    if next != *row {
        *row = next;
        grid.redraw(cx);
        return true;
    }
    false
}

/// A row's hover tint: faint, under the selection's stronger one.
fn hover_tint(theme: &Theme) -> Vec4f {
    with_alpha(theme.accent, crate::hw::row_tint_alphas(theme.background, theme.foreground, theme.accent).1)
}

/// A search heading's height: a slim divider row.
const SECTION_H: f64 = 24.0;

/// "Tracks in your playlists (12)" → ("Tracks in your playlists", "12").
fn section_parts(title: &str) -> (String, String) {
    match (title.rfind('('), title.ends_with(')')) {
        (Some(at), true) => (title[..at].trim_end().to_string(), title[at + 1..title.len() - 1].to_string()),
        _ => (title.to_string(), String::new()),
    }
}

/// "1 track", "2 tracks".
pub(crate) fn tracks_word(n: u64) -> String {
    format!("{n} {}", if n == 1 { "track" } else { "tracks" })
}

pub(crate) fn text_width(draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.width as f64
}

fn text_height(draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.height as f64
}

/// Text centred vertically on `mid`, cut to `max_width` with an ellipsis.
pub(crate) fn text_fit(draw: &mut DrawText, cx: &mut Cx2d, x: f64, mid: f64, max_width: f64, text: &str) {
    if max_width <= 4.0 || text.is_empty() {
        return;
    }
    let y = mid - text_height(draw, cx, text) * 0.5;
    if text_width(draw, cx, text) <= max_width {
        draw.draw_abs(cx, dvec2(x, y), text);
        return;
    }
    // Binary search on the character count: measured O(log n) times.
    let bounds: Vec<usize> = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).collect();
    let (mut low, mut high) = (0usize, bounds.len() - 1);
    while low < high {
        let middle = (low + high).div_ceil(2);
        if text_width(draw, cx, &format!("{}…", &text[..bounds[middle]])) <= max_width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    if low > 0 {
        draw.draw_abs(cx, dvec2(x, y), &format!("{}…", &text[..bounds[low]]));
    }
}

/// Text ending at `right`, centred vertically on `mid`.
fn text_right(draw: &mut DrawText, cx: &mut Cx2d, right: f64, mid: f64, text: &str) {
    let width = text_width(draw, cx, text);
    let y = mid - text_height(draw, cx, text) * 0.5;
    draw.draw_abs(cx, dvec2(right - width, y), text);
}

fn round(draw: &mut DrawMusicRound, cx: &mut Cx2d, rect: Rect, color: Vec4f, radius: f32) {
    draw.color = color;
    draw.radius = radius;
    draw.draw_abs(cx, rect);
}

/// Grid colours from the app palette, applied once per palette.
/// `background` is the surface the list sits on.
pub(crate) fn style_grid(cx: &mut Cx, grid: &mut WidgetRef, theme: &Theme, background: Vec4f) {
    let raised = theme.raised();
    let foreground = theme.foreground;
    let secondary = theme.muted;
    // An inverted selection paints its own row (see `row_invert`).
    // Its strength gives way where the rows' text would stop reading.
    let (strength, _) = crate::hw::row_tint_alphas(background, foreground, theme.accent);
    let selection = with_alpha(theme.accent, if theme.row_invert { 0.0 } else { strength });
    let clear = with_alpha(theme.accent, 0.0);
    let guide = with_alpha(theme.accent, 0.4);
    script_apply_eval!(cx, grid, {
        color_bg: #(background)
        color_cell: #(background)
        color_cell_alt: #(background)
        color_text: #(foreground)
        color_header: #(raised)
        color_header_active: #(raised)
        color_header_text: #(secondary)
        color_selection: #(selection)
        color_selection_border: #(clear)
        color_resize_guide: #(guide)
        draw_text +: {color: #(foreground)}
        draw_text_bold +: {color: #(secondary)}
        draw_text_header +: {color: #(secondary)}
    });
    // The scroll bars: a thumb of the text colour, lit in the accent under
    // the hand.
    let thumb = with_alpha(foreground, 0.22);
    let thumb_hover = with_alpha(foreground, 0.40);
    let thumb_drag = with_alpha(theme.accent, 0.8);
    script_apply_eval!(cx, grid, {
        scroll_bar_v +: {draw_bg +: {color: #(thumb) color_hover: #(thumb_hover) color_drag: #(thumb_drag)}}
        scroll_bar_h +: {draw_bg +: {color: #(thumb) color_hover: #(thumb_hover) color_drag: #(thumb_drag)}}
    });
}

fn row_selection(row: usize) -> GridSelection {
    GridSelection { kind: GridSelectKind::Rows, anchor: (row, 0), head: (row, 0) }
}

// ---- MusicBar ----

/// How far (pt) a finger moves along a drag-only bar before it counts as a
/// drag: a tap that wobbles a little changes nothing.
const DRAG_START: f64 = 4.0;

/// A slider without numbers: the song position, the volume and the
/// equalizer. Dragging shows the new value at once and reports it when let
/// go (the music system is slow to answer, so it is told once, not on every
/// move); a host that can follow every move (an equalizer knob) reads
/// [`MusicBarRef::moved`] too.
#[derive(Script, ScriptHook, Widget)]
pub struct MusicBar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_round: DrawMusicRound,
    /// The fill runs from the middle: a value either side of a centre
    /// (an equalizer band's boost or cut).
    #[live(false)]
    centered: bool,
    /// Only a drag changes the value, by how far it moves along the bar; a
    /// press or a tap leaves it (a volume must never jump under a touch).
    #[live(false)]
    relative: bool,
    #[rust]
    value: f64,
    /// While dragging, the value under the pointer.
    #[rust]
    dragging: Option<f64>,
    /// A relative drag: where it started and the value then.
    #[rust]
    drag_from: Option<(f64, f64)>,
    /// Where presses are taken when bigger than the bar (a host's room
    /// around a thin bar, for a finger-sized target); the bar's own rect
    /// when empty.
    #[rust]
    touch: Area,
    #[rust]
    hover: bool,
    #[rust]
    enabled: bool,
    /// The whole bar, for pointer hits: the drawn shapes are thin and a
    /// quad's area names only the last one drawn.
    #[rust]
    area: Area,
    #[rust]
    theme: Theme,
}

#[derive(Clone, Debug, Default)]
pub enum MusicBarAction {
    #[default]
    None,
    /// Let go at this value, 0..1.
    Set(f64),
    /// Pressed or dragged to this value, 0..1.
    Moved(f64),
}

impl MusicBar {
    pub fn set(&mut self, cx: &mut Cx, value: Option<f64>, theme: Theme) {
        let enabled = value.is_some();
        let value = value.unwrap_or(0.0).clamp(0.0, 1.0);
        if (value - self.value).abs() > 1e-4 || enabled != self.enabled || theme.accent != self.theme.accent {
            self.value = value;
            self.enabled = enabled;
            self.theme = theme;
            self.draw_round.redraw(cx);
        }
    }

    /// A drag-only bar: the value follows how far the finger moves along the
    /// bar from where it went down; a press, a tap or a click sets nothing.
    /// A wheel or trackpad scroll steps it.
    fn handle_relative(&mut self, cx: &mut Cx, event: &Event, touch: Area) {
        let width = self.area.rect(cx).size.x.max(1.0);
        match event.hits(cx, touch) {
            Hit::FingerHoverIn(_) => {
                self.hover = true;
                cx.set_cursor(MouseCursor::Hand);
                self.draw_round.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = false;
                self.draw_round.redraw(cx);
            }
            Hit::FingerDown(e) => {
                self.drag_from = Some((e.abs.x, self.value));
            }
            Hit::FingerMove(e) => {
                if let Some((x, from)) = self.drag_from {
                    let value = (from + (e.abs.x - x) / width).clamp(0.0, 1.0);
                    if self.dragging.is_some() || (e.abs.x - x).abs() >= DRAG_START {
                        self.dragging = Some(value);
                        self.draw_round.redraw(cx);
                        cx.widget_action(self.uid, MusicBarAction::Moved(value));
                    }
                }
            }
            Hit::FingerUp(e) => {
                self.hover = e.is_over;
                self.drag_from = None;
                if let Some(value) = self.dragging.take() {
                    // A drag something else took away keeps the old value.
                    if !e.cancelled {
                        self.value = value;
                        cx.widget_action(self.uid, MusicBarAction::Set(value));
                    } else {
                        cx.widget_action(self.uid, MusicBarAction::Moved(self.value));
                    }
                }
                self.draw_round.redraw(cx);
            }
            Hit::FingerScroll(e) => {
                let step = if e.scroll.x.abs() > e.scroll.y.abs() { e.scroll.x } else { -e.scroll.y };
                let value = (self.value + step / 600.0).clamp(0.0, 1.0);
                if (value - self.value).abs() > 1e-6 {
                    self.value = value;
                    self.draw_round.redraw(cx);
                    cx.widget_action(self.uid, MusicBarAction::Set(value));
                }
            }
            _ => {}
        }
    }

    /// The value at `abs` along the bar, from the hit's `rect`: that rect is
    /// in window coordinates like `abs`, where the rect the bar was drawn
    /// in can be local to a view that draws on its own (the now-playing bar).
    fn at(abs: Vec2d, rect: Rect) -> f64 {
        ((abs.x - rect.pos.x) / rect.size.x.max(1.0)).clamp(0.0, 1.0)
    }
}

impl MusicBarRef {
    /// Take presses in `area` (drawn around the bar by the host) instead of
    /// the bar's own rect.
    pub fn set_touch_area(&self, area: Area) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.touch = area;
        }
    }

    pub fn set(&self, cx: &mut Cx, value: Option<f64>, theme: Theme) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set(cx, value, theme);
        }
    }

    pub fn released(&self, actions: &Actions) -> Option<f64> {
        actions.filter_widget_actions(self.widget_uid()).filter_map(|action| match action.cast() {
            MusicBarAction::Set(value) => Some(value),
            _ => None,
        }).last()
    }

    /// Pressed or dragged to, 0..1 (for a host that follows every move).
    pub fn moved(&self, actions: &Actions) -> Option<f64> {
        actions.filter_widget_actions(self.widget_uid()).filter_map(|action| match action.cast() {
            MusicBarAction::Moved(value) => Some(value),
            _ => None,
        }).last()
    }
}

impl Widget for MusicBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.enabled {
            return;
        }
        let touch = if self.touch.is_valid(cx) { self.touch } else { self.area };
        if self.relative {
            return self.handle_relative(cx, event, touch);
        }
        match event.hits(cx, touch) {
            Hit::FingerHoverIn(_) => {
                self.hover = true;
                cx.set_cursor(MouseCursor::Hand);
                self.draw_round.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = false;
                self.draw_round.redraw(cx);
            }
            Hit::FingerDown(e) => {
                let value = Self::at(e.abs, e.rect);
                self.dragging = Some(value);
                self.draw_round.redraw(cx);
                cx.widget_action(self.uid, MusicBarAction::Moved(value));
            }
            Hit::FingerMove(e) => {
                let value = Self::at(e.abs, e.rect);
                self.dragging = Some(value);
                self.draw_round.redraw(cx);
                cx.widget_action(self.uid, MusicBarAction::Moved(value));
            }
            Hit::FingerUp(e) => {
                // A drag let go outside the bar gets no hover-out of its own.
                self.hover = e.is_over;
                self.dragging = None;
                self.draw_round.redraw(cx);
                // A press something else took away (a cancelled release)
                // sets nothing.
                if !e.cancelled {
                    let value = Self::at(e.abs, e.rect);
                    self.value = value;
                    cx.widget_action(self.uid, MusicBarAction::Set(value));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let theme = self.theme;
        let mid = rect.pos.y + rect.size.y * 0.5;
        let thick = if self.hover || self.dragging.is_some() { 5.0 } else { 4.0 };
        let track = Rect { pos: dvec2(rect.pos.x, mid - thick * 0.5), size: dvec2(rect.size.x, thick) };
        round(&mut self.draw_round, cx, track, with_alpha(theme.foreground, 0.16), 2.5);
        if self.enabled {
            let value = self.dragging.unwrap_or(self.value);
            let color = if self.hover || self.dragging.is_some() { theme.accent } else { theme.foreground };
            let from = if self.centered { 0.5 } else { 0.0 };
            let (left, right) = (value.min(from), value.max(from));
            round(&mut self.draw_round, cx, Rect { pos: dvec2(track.pos.x + track.size.x * left, track.pos.y), size: dvec2(track.size.x * (right - left), thick) }, color, 2.5);
            if self.centered {
                round(&mut self.draw_round, cx, Rect { pos: dvec2(track.pos.x + track.size.x * 0.5 - 1.0, mid - 5.0), size: dvec2(2.0, 10.0) }, with_alpha(theme.foreground, 0.35), 1.0);
            }
            // A drag-only bar always shows its handle: the thing to drag.
            if self.relative || self.hover || self.dragging.is_some() {
                let x = rect.pos.x + rect.size.x * value;
                round(&mut self.draw_round, cx, Rect { pos: dvec2(x - 6.0, mid - 6.0), size: dvec2(12.0, 12.0) }, theme.foreground, 6.0);
            }
        }
        DrawStep::done()
    }
}

// ---- LibraryList ----

/// The playlist tree in the sidebar: folders as headings, playlists under
/// them with their track counts. A press opens the playlist.
#[derive(Script, ScriptHook, Widget)]
pub struct LibraryList {
    #[deref]
    view: View,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_count: DrawText,
    #[live]
    draw_icon: DrawText,
    #[rust]
    built_version: Option<u64>,
    #[rust]
    width: f64,
    #[rust]
    theme_applied: Option<(Vec4f, u64)>,
    /// A player's playlist chooser: only the playlists, numbered, the open
    /// one lit, filtered by `filter` (set from Rust).
    #[live(false)]
    flat: bool,
    #[rust]
    pub filter: String,
    /// In flat mode, the entry each row shows.
    #[rust]
    flat_rows: Vec<usize>,
    #[rust]
    hover_any: Option<usize>,
}

impl LibraryList {
    fn draw_flat(&mut self, cx: &mut Cx2d, model: &Model, walk: Walk) -> DrawStep {
        let theme = model.theme;
        if self.theme_applied != Some((theme.background, model.style_gen)) {
            self.theme_applied = Some((theme.background, model.style_gen));
            let mut grid = self.view.widget(cx, ids!(grid));
            style_grid(cx, &mut grid, &theme, theme.background);
        }
        let width = cx.peek_walk_turtle(walk).size.x;
        let resized = (width - self.width).abs() > 0.5;
        self.width = width;
        let index = model.index.clone();
        let needle = self.filter.to_lowercase();
        self.flat_rows = index
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_folder && (needle.is_empty() || e.name.to_lowercase().contains(&needle)))
            .map(|(at, _)| at)
            .collect();
        let open = match model.page {
            Page::Playlist(at) => Some(at),
            _ => None,
        };
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            grid.set_grid_size(self.flat_rows.len(), 1);
            if resized {
                grid.set_col_widths(cx, &[(width - 2.0).max(40.0)]);
            }
            while let Some(cell) = grid.next_cell(cx) {
                let Some(&at) = self.flat_rows.get(cell.row) else { continue };
                let Some(entry) = index.entries.get(at) else { continue };
                let mid = cell.rect.pos.y + cell.rect.size.y * 0.5;
                let x = cell.rect.pos.x + 8.0;
                let right = cell.rect.pos.x + cell.rect.size.x - 10.0;
                let current = open == Some(at);
                if self.hover_any == Some(cell.row) {
                    grid.cell_bg(cx, &cell, hover_tint(&theme));
                }
                let count = entry.track_count.max(entry.tracks.len() as u32).to_string();
                let count_w = text_width(&self.draw_count, cx, &count) + 10.0;
                self.draw_name.color = if current { theme.accent } else { theme.foreground };
                text_fit(&mut self.draw_name, cx, x, mid, right - x - count_w, &format!("{}. {}", cell.row + 1, entry.name));
                self.draw_count.color = if current { theme.accent } else { theme.muted };
                text_right(&mut self.draw_count, cx, right, mid, &count);
            }
        }
        DrawStep::done()
    }
}

impl Widget for LibraryList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(model) = scope.data_mut().get::<Model>() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        if self.flat {
            return self.draw_flat(cx, model, walk);
        }
        let theme = model.theme;
        // A restyle reapplies the grid's defaults: style it again then too.
        if self.theme_applied != Some((theme.background, model.style_gen)) {
            self.theme_applied = Some((theme.background, model.style_gen));
            let mut grid = self.view.widget(cx, ids!(grid));
            style_grid(cx, &mut grid, &theme, theme.raised());
        }
        let width = cx.peek_walk_turtle(walk).size.x;
        let resized = (width - self.width).abs() > 0.5;
        self.width = width;
        let rebuilt = self.built_version != Some(model.version);
        self.built_version = Some(model.version);
        let index = model.index.clone();
        let open = match model.page {
            Page::Playlist(at) => Some(at),
            _ => None,
        };
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            grid.set_grid_size(index.entries.len(), 1);
            if resized {
                grid.set_col_widths(cx, &[(width - 2.0).max(40.0)]);
            }
            if rebuilt {
                grid.set_selection(cx, open.map(row_selection));
            }
            while let Some(cell) = grid.next_cell(cx) {
                let Some(entry) = index.entries.get(cell.row) else { continue };
                let theme = if model.theme.row_invert && Some(cell.row) == open {
                    grid.cell_bg(cx, &cell, model.theme.foreground);
                    model.theme.inverted()
                } else {
                    model.theme
                };
                let mid = cell.rect.pos.y + cell.rect.size.y * 0.5;
                let right = cell.rect.pos.x + cell.rect.size.x - 10.0;
                let mut x = cell.rect.pos.x + 12.0 + (entry.depth.saturating_sub(1) as f64) * 14.0;
                self.draw_icon.color = with_alpha(theme.muted, 0.8);
                let icon = if entry.is_folder { ICON_FOLDER } else { ICON_LIST };
                text_fit(&mut self.draw_icon, cx, x, mid, 16.0, icon);
                x += 22.0;
                let count = if entry.is_folder {
                    String::new()
                } else if entry.tracks.is_empty() && entry.read_secs == 0 {
                    "…".to_string()
                } else {
                    entry.track_count.max(entry.tracks.len() as u32).to_string()
                };
                let count_width = if count.is_empty() { 0.0 } else { text_width(&self.draw_count, cx, &count) + 8.0 };
                self.draw_name.color = if Some(cell.row) == open { theme.foreground } else { crate::mix(theme.muted, theme.foreground, 0.6) };
                text_fit(&mut self.draw_name, cx, x, mid, right - x - count_width, &entry.name);
                if !count.is_empty() {
                    self.draw_count.color = theme.muted;
                    text_right(&mut self.draw_count, cx, right, mid, &count);
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, &mut Scope::empty());
        let uid = self.widget_uid();
        let grid = self.view.data_grid(cx, ids!(grid));
        track_hover(cx, &grid, event, &mut self.hover_any);
        let Event::Actions(actions) = event else { return };
        let Some(model) = scope.data_mut().get_mut::<Model>() else { return };
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            match action {
                DataGridAction::SelectionChanged { selection: Some(selection) } if self.flat => {
                    if let Some(&at) = self.flat_rows.get(selection.head.0) {
                        model.open(Page::Playlist(at));
                        cx.widget_action(uid, LibraryListAction::Picked(at));
                    }
                }
                DataGridAction::SelectionChanged { selection: Some(selection) } => {
                    let row = selection.head.0;
                    if model.index.entries.get(row).is_some_and(|entry| !entry.is_folder) && model.page != Page::Playlist(row) {
                        model.open(Page::Playlist(row));
                    }
                    // The Local Files heading opens the folders page (add or
                    // remove folders).
                    if model.index.entries.get(row).is_some_and(|entry| entry.uri == crate::local::LOCAL_ROOT) && model.page != Page::Folders {
                        model.open(Page::Folders);
                    }
                }
                DataGridAction::CellContextMenu { row, abs, .. } => {
                    if model.index.entries.get(row).is_some_and(|entry| !entry.is_folder) {
                        cx.widget_action(uid, LibraryListAction::Menu(row, abs));
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum LibraryListAction {
    #[default]
    None,
    /// A right press on the playlist at this entry, at `abs`.
    Menu(usize, DVec2),
    /// A playlist chosen in the flat chooser.
    Picked(usize),
}

impl LibraryListRef {
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            LibraryListAction::Picked(at) => Some(at),
            _ => None,
        }
    }

    pub fn set_filter(&self, cx: &mut Cx, filter: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.filter != filter {
                inner.filter = filter.to_string();
                inner.view.redraw(cx);
            }
        }
    }

    pub fn menu(&self, actions: &Actions) -> Option<(usize, DVec2)> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            LibraryListAction::Menu(at, abs) => Some((at, abs)),
            _ => None,
        }
    }
}

// ---- TrackTable ----

/// The column chooser's menu.
fn columns_menu_owner() -> LiveId {
    live_id!(music_columns)
}

/// Room kept at a hovered track title's right end for its radio button.
const RADIO_ROOM: f64 = 26.0;
/// Room kept at a title's right end for its local-file mark.
const LOCAL_ROOM: f64 = 20.0;

/// The main pane's table: a playlist's tracks, search results grouped by
/// kind. Double-click or Return plays or opens the row. The columns are
/// the ones chosen (a right press on a heading), in the order they were
/// dragged to; a heading press sorts by it. A hovered track shows a radio
/// button at the end of its title.
#[derive(Script, ScriptHook, Widget)]
pub struct TrackTable {
    #[deref]
    view: View,
    /// A heading's rule.
    #[live]
    draw_round_rule: DrawMusicRound,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_section: DrawText,
    #[live]
    draw_icon: DrawText,
    #[rust]
    built_version: Option<u64>,
    /// The table's right edge, where a group heading may run to.
    #[rust]
    right: f64,
    #[rust]
    theme_applied: Option<(Vec4f, u64)>,
    /// The row the keyboard or a press picked.
    #[rust]
    picked: Option<usize>,
    /// The page and query drawn: a new one starts at the top.
    #[rust]
    shown: Option<(Page, String)>,
    /// The columns drawn, fitted from the model's arrangement.
    #[rust]
    fit: GridColumnFit<Column>,
    /// The track row under the pointer, and where the pointer is.
    #[rust]
    hover_row: Option<usize>,
    #[rust]
    pointer: DVec2,
    /// The radio button drawn on the hovered row: (row, rect).
    #[rust]
    radio_button: Option<(usize, Rect)>,
    /// A player's playlist: one full-width column, "N. Artist – Title" and
    /// the time at the right, the playing row lit; no headings.
    #[live(false)]
    numbered: bool,
    #[rust]
    numbered_width: f64,
    /// Any row under the pointer (its hover tint).
    #[rust]
    hover_any: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub enum TrackTableAction {
    #[default]
    None,
    /// A row was double-clicked or Return was pressed on it.
    Activated(usize),
    /// A right press on a row, at `abs`: where its menu opens.
    Menu(usize, DVec2),
    /// A row's radio button was pressed.
    Radio(usize),
}

impl TrackTable {
    /// Text in a cell that is empty when `text` is: a figure nobody knows
    /// reads as a quiet dash.
    fn figure(draw: &mut DrawText, cx: &mut Cx2d, color: Vec4f, cell: &Rect, mid: f64, text: Option<String>) {
        let right = cell.pos.x + cell.size.x - 14.0;
        match text {
            Some(text) => {
                draw.color = color;
                text_right(draw, cx, right, mid, &text);
            }
            None => {
                draw.color = with_alpha(color, color.w * 0.4);
                text_right(draw, cx, right, mid, "–");
            }
        }
    }

    /// Whether `row` can start a radio now: a track of the system (or a
    /// local copy of one), while the system is there.
    fn has_radio(model: &Model, row: usize) -> bool {
        model.system_ready() && matches!(model.rows.get(row), Some(Row::Track { .. }) | Some(Row::Catalog { kind: "Track", .. })) && model.radio_for(Some(row)).is_some()
    }

    /// Follow the pointer over the rows: the hovered track shows its radio
    /// button.
    fn hover(&mut self, cx: &mut Cx, model: &Model, abs: DVec2) {
        self.pointer = abs;
        let grid = self.view.data_grid(cx, ids!(grid));
        let _ = track_hover_at(cx, &grid, Some(abs), &mut self.hover_any);
        let inside = grid.area().rect(cx).contains(abs);
        let row = if inside { grid.borrow().and_then(|grid| grid.row_at(abs)) } else { None };
        let row = row.filter(|row| Self::has_radio(model, *row));
        if row != self.hover_row {
            self.hover_row = row;
            grid.redraw(cx);
        }
    }

    /// The row's selection, the one a Return or a radio shortcut acts on.
    pub fn picked(&self) -> Option<usize> {
        self.picked
    }
}

impl TrackTableRef {
    pub fn activated(&self, actions: &Actions) -> Option<usize> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TrackTableAction::Activated(row) => Some(row),
            _ => None,
        }
    }

    pub fn menu(&self, actions: &Actions) -> Option<(usize, DVec2)> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TrackTableAction::Menu(row, abs) => Some((row, abs)),
            _ => None,
        }
    }

    pub fn radio(&self, actions: &Actions) -> Option<usize> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TrackTableAction::Radio(row) => Some(row),
            _ => None,
        }
    }

    pub fn picked(&self) -> Option<usize> {
        self.borrow().and_then(|table| table.picked())
    }
}

impl TrackTable {
    fn draw_numbered(&mut self, cx: &mut Cx2d, model: &Model, walk: Walk) -> DrawStep {
        let theme = model.theme;
        if self.theme_applied != Some((theme.background, model.style_gen)) {
            self.theme_applied = Some((theme.background, model.style_gen));
            let mut grid = self.view.widget(cx, ids!(grid));
            style_grid(cx, &mut grid, &theme, theme.background);
        }
        let width = cx.peek_walk_turtle(walk).size.x;
        let resized = (width - self.numbered_width).abs() > 0.5;
        self.numbered_width = width;
        let index = model.index.clone();
        let playing_uri = model.playback.now.as_ref().map(|now| now.uri.clone()).unwrap_or_default();
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            grid.set_grid_size(model.rows.len(), 1);
            if self.built_version != Some(model.version) {
                self.built_version = Some(model.version);
                grid.clear_row_heights();
                for (at, row) in model.rows.iter().enumerate() {
                    if matches!(row, Row::Section(_)) {
                        grid.set_row_height(at, SECTION_H - 4.0);
                    }
                }
            }
            if resized {
                grid.set_col_widths(cx, &[(width - 2.0).max(40.0)]);
            }
            let key = (model.page.clone(), model.query.clone());
            if self.shown.as_ref() != Some(&key) {
                self.shown = Some(key);
                self.picked = None;
                grid.set_selection(cx, None);
                grid.set_scroll(cx, dvec2(0.0, 0.0));
            }
            while let Some(cell) = grid.next_cell(cx) {
                let Some(row) = model.rows.get(cell.row) else { continue };
                let mid = cell.rect.pos.y + cell.rect.size.y * 0.5;
                let x = cell.rect.pos.x + 8.0;
                let right = cell.rect.pos.x + cell.rect.size.x - 10.0;
                let (text, time, current) = match row {
                    Row::Track { id, number } => {
                        let Some(track) = index.tracks.get(*id as usize) else { continue };
                        let current = track.uri == playing_uri || track.local_copy.is_some_and(|copy| index.tracks[copy as usize].uri == playing_uri);
                        let name = if track.artists.is_empty() { track.name.clone() } else { format!("{} – {}", track.artists, track.name) };
                        let time = (track.duration_ms > 0).then(|| duration(track.duration_ms as u64)).unwrap_or_default();
                        (format!("{number}. {name}"), time, current)
                    }
                    Row::Catalog { name, subtitle, uri, .. } => {
                        let name = if subtitle.is_empty() { name.clone() } else { format!("{subtitle} – {name}") };
                        (format!("{}. {name}", cell.row + 1), String::new(), *uri == playing_uri)
                    }
                    Row::Section(title) => {
                        self.draw_small.color = with_alpha(theme.muted, 0.8);
                        text_fit(&mut self.draw_small, cx, x, mid, right - x, &title.to_uppercase());
                        continue;
                    }
                    Row::Note(text) => {
                        self.draw_small.color = theme.muted;
                        text_fit(&mut self.draw_small, cx, x, mid, right - x, text);
                        continue;
                    }
                    Row::Folder { path, .. } => (path.clone(), String::new(), false),
                    Row::Playlist(at) => (index.entries.get(*at).map(|e| e.name.clone()).unwrap_or_default(), String::new(), false),
                    Row::Album { name, .. } | Row::Artist { name, .. } => (name.clone(), String::new(), false),
                };
                if self.hover_any == Some(cell.row) {
                    grid.cell_bg(cx, &cell, hover_tint(&theme));
                }
                let color = if current { theme.accent } else { theme.foreground };
                let time_w = if time.is_empty() { 0.0 } else { text_width(&self.draw_small, cx, &time) + 10.0 };
                self.draw_title.color = color;
                text_fit(&mut self.draw_title, cx, x, mid, right - x - time_w, &text);
                if !time.is_empty() {
                    self.draw_small.color = if current { theme.accent } else { theme.muted };
                    text_right(&mut self.draw_small, cx, right, mid, &time);
                }
            }
        }
        DrawStep::done()
    }
}

impl Widget for TrackTable {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(model) = scope.data_mut().get::<Model>() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        if self.numbered {
            return self.draw_numbered(cx, model, walk);
        }
        let theme = model.theme;
        // A restyle reapplies the grid's defaults: style it again then too.
        if self.theme_applied != Some((theme.background, model.style_gen)) {
            self.theme_applied = Some((theme.background, model.style_gen));
            let mut grid = self.view.widget(cx, ids!(grid));
            style_grid(cx, &mut grid, &theme, theme.background);
        }
        let area = cx.peek_walk_turtle(walk);
        let width = area.size.x;
        self.right = area.pos.x + width;
        // Fitted again when the width or the chosen columns change, never
        // on new rows, so a column being resized is left alone.
        let reconfigure = self.fit.fit(width, &model.columns);
        let rebuilt = self.built_version != Some(model.version);
        self.built_version = Some(model.version);
        let index = model.index.clone();
        let playing_uri = model.playback.now.as_ref().map(|now| now.uri.clone()).unwrap_or_default();
        let playing = model.playback.is_playing();
        let sort = self.fit.sort_indicator(&model.columns);
        let columns = self.fit.columns().to_vec();
        let hover_row = self.hover_row.filter(|row| Self::has_radio(model, *row));
        self.radio_button = None;
        // Group headings and notes start in the second column and run to
        // the table's edge.
        let span_col = if columns.len() > 1 { 1 } else { 0 };
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            if reconfigure {
                self.fit.configure(cx, &mut grid, model.rows.len(), width, &model.columns);
            }
            grid.set_grid_size(model.rows.len(), columns.len());
            // Headings are slim divider rows, not rows of their own height.
            if rebuilt || reconfigure {
                grid.clear_row_heights();
                for (at, row) in model.rows.iter().enumerate() {
                    if matches!(row, Row::Section(_)) {
                        grid.set_row_height(at, SECTION_H);
                    }
                }
            }
            grid.set_sort_indicator(sort);
            let key = (model.page.clone(), model.query.clone());
            if self.shown.as_ref() != Some(&key) {
                self.shown = Some(key);
                self.picked = None;
                grid.set_selection(cx, None);
                grid.set_scroll(cx, dvec2(0.0, 0.0));
            }
            if (rebuilt || reconfigure) && self.picked.is_some_and(|row| row >= model.rows.len()) {
                self.picked = None;
                grid.set_selection(cx, None);
            }
            while let Some(cell) = grid.next_cell(cx) {
                let Some(row) = model.rows.get(cell.row) else { continue };
                let Some(&column) = columns.get(cell.col) else { continue };
                let theme = if model.theme.row_invert && self.picked == Some(cell.row) {
                    grid.cell_bg(cx, &cell, model.theme.foreground);
                    model.theme.inverted()
                } else {
                    model.theme
                };
                if self.hover_any == Some(cell.row) && !matches!(row, Row::Section(_) | Row::Note(_)) {
                    grid.cell_bg(cx, &cell, hover_tint(&theme));
                }
                let mid = cell.rect.pos.y + cell.rect.size.y * 0.5;
                let x = cell.rect.pos.x + 10.0;
                let room = cell.rect.size.x - 16.0;
                let to_edge = self.right - x - 16.0;
                let number_right = cell.rect.pos.x + cell.rect.size.x - 12.0;
                // The hovered track's title makes room for its radio button.
                let radio_here = hover_row == Some(cell.row) && column == Column::Title && room > RADIO_ROOM * 2.0;
                let title_room = if radio_here { room - RADIO_ROOM } else { room };
                if radio_here {
                    let button = Rect { pos: dvec2(cell.rect.pos.x + cell.rect.size.x - RADIO_ROOM - 4.0, mid - 11.0), size: dvec2(RADIO_ROOM, 22.0) };
                    let over = button.contains(self.pointer);
                    self.draw_icon.color = if over { theme.accent } else { theme.muted };
                    text_right(&mut self.draw_icon, cx, button.pos.x + button.size.x - 6.0, mid, ICON_RADIO);
                    self.radio_button = Some((cell.row, button));
                }
                match row {
                    Row::Section(title) => {
                        // A divider: the heading in small capitals, muted, and
                        // its count (the part in brackets) at the table's edge.
                        if cell.col == 0 {
                            let (name, count) = section_parts(title);
                            self.draw_small.color = with_alpha(theme.muted, 0.9);
                            let right = self.right - 14.0;
                            let count_w = if count.is_empty() { 0.0 } else { text_width(&self.draw_small, cx, &count) + 12.0 };
                            text_fit(&mut self.draw_small, cx, cell.rect.pos.x + 10.0, mid, right - cell.rect.pos.x - 10.0 - count_w, &name.to_uppercase());
                            if !count.is_empty() {
                                text_right(&mut self.draw_small, cx, right, mid, &count);
                            }
                            let rule = Rect { pos: dvec2(cell.rect.pos.x + 6.0, cell.rect.pos.y + cell.rect.size.y - 1.0), size: dvec2((self.right - cell.rect.pos.x - 12.0).max(0.0), 1.0) };
                            round(&mut self.draw_round_rule, cx, rule, with_alpha(theme.muted, 0.25), 0.0);
                        }
                    }
                    Row::Note(text) => {
                        if cell.col == span_col {
                            self.draw_small.color = theme.muted;
                            text_fit(&mut self.draw_small, cx, x, mid, to_edge, text);
                        }
                    }
                    Row::Track { id, number } => {
                        let Some(track) = index.tracks.get(*id as usize) else { continue };
                        // The playing track is the one itself, or its local copy.
                        let current = track.uri == playing_uri || track.local_copy.is_some_and(|copy| index.tracks[copy as usize].uri == playing_uri);
                        // What cannot play now (the system away, no local
                        // file) reads faint.
                        let faint = |color: Vec4f| if model.playable(*id) { color } else { with_alpha(color, 0.38) };
                        match column {
                            Column::Number => {
                                if current {
                                    self.draw_icon.color = theme.accent;
                                    text_right(&mut self.draw_icon, cx, number_right, mid, if playing { ICON_SPEAKER } else { ICON_PAUSE });
                                } else {
                                    self.draw_small.color = faint(theme.muted);
                                    text_right(&mut self.draw_small, cx, number_right, mid, &number.to_string());
                                }
                            }
                            Column::Title => {
                                // Completed offline songs keep their check
                                // mark while hovering the radio action too.
                                let cached = track.cached_path.is_some();
                                let marked = index.has_local(*id) && title_room > LOCAL_ROOM * 2.0;
                                let text_room = if marked { title_room - LOCAL_ROOM } else { title_room };
                                self.draw_title.color = if current { theme.accent } else { faint(theme.foreground) };
                                text_fit(&mut self.draw_title, cx, x, mid, text_room, &track.name);
                                if marked {
                                    self.draw_icon.color = if cached { CACHED_INK } else { with_alpha(theme.muted, 0.8) };
                                    let right = cell.rect.pos.x + cell.rect.size.x - 10.0 - if radio_here { RADIO_ROOM } else { 0.0 };
                                    text_right(&mut self.draw_icon, cx, right, mid, if cached { ICON_CACHED } else { ICON_LOCAL });
                                }
                            }
                            Column::Artist => {
                                self.draw_small.color = faint(theme.muted);
                                text_fit(&mut self.draw_small, cx, x, mid, room, &track.artists);
                            }
                            Column::Album => {
                                self.draw_small.color = faint(theme.muted);
                                text_fit(&mut self.draw_small, cx, x, mid, room, &track.album);
                            }
                            Column::Released => Self::figure(&mut self.draw_small, cx, faint(theme.muted), &cell.rect, mid, (track.release != 0).then(|| columns::release(track.release))),
                            Column::Time => {
                                if track.duration_ms > 0 {
                                    self.draw_small.color = faint(theme.muted);
                                    text_right(&mut self.draw_small, cx, cell.rect.pos.x + cell.rect.size.x - 14.0, mid, &duration(track.duration_ms as u64));
                                }
                            }
                        }
                    }
                    Row::Folder { path, files } => match column {
                        Column::Number => {
                            self.draw_icon.color = theme.muted;
                            text_right(&mut self.draw_icon, cx, number_right, mid, ICON_FOLDER);
                        }
                        Column::Title => {
                            let name = std::path::Path::new(path).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
                            self.draw_title.color = theme.foreground;
                            text_fit(&mut self.draw_title, cx, x, mid, room, &name);
                        }
                        // The count, then the whole path to the table's edge.
                        Column::Artist => {
                            self.draw_small.color = theme.muted;
                            text_fit(&mut self.draw_small, cx, x, mid, to_edge, &format!("{files} files · {path}"));
                        }
                        _ => {}
                    },
                    Row::Playlist(at) => {
                        let Some(entry) = index.entries.get(*at) else { continue };
                        match column {
                            Column::Number => {
                                self.draw_icon.color = theme.muted;
                                text_right(&mut self.draw_icon, cx, number_right, mid, ICON_LIST);
                            }
                            Column::Title => {
                                self.draw_title.color = theme.foreground;
                                text_fit(&mut self.draw_title, cx, x, mid, room, &entry.name);
                            }
                            Column::Artist => {
                                self.draw_small.color = theme.muted;
                                text_fit(&mut self.draw_small, cx, x, mid, room, &format!("Playlist · {}", tracks_word(entry.track_count.max(entry.tracks.len() as u32) as u64)));
                            }
                            _ => {}
                        }
                    }
                    Row::Album { name, artists, .. } => match column {
                        Column::Title => {
                            self.draw_title.color = theme.foreground;
                            text_fit(&mut self.draw_title, cx, x, mid, room, name);
                        }
                        Column::Artist => {
                            self.draw_small.color = theme.muted;
                            text_fit(&mut self.draw_small, cx, x, mid, room, &format!("Album · {artists}"));
                        }
                        _ => {}
                    },
                    Row::Artist { name, tracks } => match column {
                        Column::Title => {
                            self.draw_title.color = theme.foreground;
                            text_fit(&mut self.draw_title, cx, x, mid, room, name);
                        }
                        Column::Artist => {
                            self.draw_small.color = theme.muted;
                            text_fit(&mut self.draw_small, cx, x, mid, room, &format!("Artist · {} in your playlists", tracks_word(*tracks as u64)));
                        }
                        _ => {}
                    },
                    Row::Catalog { kind, name, subtitle, uri } => match column {
                        Column::Title => {
                            self.draw_title.color = if *uri == playing_uri { theme.accent } else { theme.foreground };
                            text_fit(&mut self.draw_title, cx, x, mid, title_room, name);
                        }
                        Column::Artist => {
                            self.draw_small.color = theme.muted;
                            let text = if subtitle.is_empty() { kind.to_string() } else { format!("{kind} · {subtitle}") };
                            text_fit(&mut self.draw_small, cx, x, mid, room, &text);
                        }
                        _ => {}
                    },
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, &mut Scope::empty());
        let uid = self.widget_uid();
        let Some(model) = scope.data_mut().get_mut::<Model>() else { return };
        if let Event::MouseMove(e) = event {
            self.hover(cx, model, e.abs);
            if let Some((_, button)) = self.radio_button {
                // The button lights under the pointer.
                if button.contains(e.abs) != button.contains(self.pointer) || self.hover_row.is_some() {
                    self.view.data_grid(cx, ids!(grid)).redraw(cx);
                }
            }
        }
        let Event::Actions(actions) = event else { return };
        if let Some(id) = menu_picked(actions, columns_menu_owner()) {
            if model.columns.apply_pick(id) {
                model.columns_changed(true);
            }
            cx.action(MenuAction::Update { owner: columns_menu_owner(), rows: model.columns.menu_rows() });
        }
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            match action {
                DataGridAction::SelectionChanged { selection } => {
                    self.picked = selection.map(|selection| selection.head.0);
                }
                // Return and a double-click both ask to "edit" a cell; in a
                // read-only list that is the row being chosen.
                DataGridAction::EditCell { row, replace: None, .. } => {
                    cx.widget_action(uid, TrackTableAction::Activated(row));
                }
                DataGridAction::CellReleased { row, .. } => {
                    if self.radio_button.is_some_and(|(at, button)| at == row && button.contains(self.pointer)) {
                        cx.widget_action(uid, TrackTableAction::Radio(row));
                    }
                }
                DataGridAction::CellContextMenu { row, abs, .. } => {
                    cx.widget_action(uid, TrackTableAction::Menu(row, abs));
                }
                DataGridAction::HeaderClicked { col, .. } => {
                    if let Some(column) = self.fit.column(col) {
                        model.press_heading(column);
                    }
                }
                DataGridAction::HeaderContextMenu { abs, .. } => {
                    let anchor = Rect { pos: abs, size: dvec2(0.0, 0.0) };
                    cx.action(MenuAction::OpenSet { owner: columns_menu_owner(), rows: model.columns.menu_rows(), anchor, place: MenuPlace::At });
                }
                DataGridAction::ColumnOrderChanged { order } => {
                    model.columns.fold_order(&self.fit, &order);
                    model.columns_changed(true);
                }
                DataGridAction::ColumnResized { col, width, .. } => {
                    if model.columns.set_width(&self.fit, col, width) {
                        model.requests.columns = true;
                    }
                }
                _ => {}
            }
        }
    }
}

// ---- UpNext ----

/// A line of the Up Next panel.
#[derive(Clone, Debug, PartialEq)]
enum UpRow {
    Heading(String),
    /// What plays now.
    Current,
    /// The upcoming track at this place in the queue.
    Item(usize),
    Note(String),
}

/// The Up Next panel: what plays now, what was queued by hand, then the
/// rest of the list playback started from. Rows drag to a new place;
/// double-click or Return plays one; Delete takes one out.
#[derive(Script, ScriptHook, Widget)]
pub struct UpNext {
    #[deref]
    view: View,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_section: DrawText,
    #[live]
    draw_icon: DrawText,
    #[live]
    draw_round: DrawMusicRound,
    /// The row under the pointer (its hover tint).
    #[rust]
    hover_any: Option<usize>,
    #[rust]
    rows: Vec<UpRow>,
    #[rust]
    built_version: Option<u64>,
    #[rust]
    width: f64,
    #[rust]
    theme_applied: Option<(Vec4f, u64)>,
    #[rust]
    picked: Option<usize>,
    /// The upcoming track being carried, and the gap (in rows) it would
    /// drop into.
    #[rust]
    carrying: Option<usize>,
    #[rust]
    gap: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub enum UpNextAction {
    #[default]
    None,
    /// Play the upcoming track at this place.
    Jump(usize),
    /// A right press on the upcoming track at this place, at `abs`.
    Menu(usize, DVec2),
    Remove(usize),
    /// Move the upcoming track `from` into the gap `to`.
    Move { from: usize, to: usize },
}

impl UpNext {
    fn build(queue: &crate::queue::PlayQueue) -> Vec<UpRow> {
        let mut rows = Vec::new();
        if queue.parked {
            rows.push(UpRow::Note("Kept from last time; press play to go on".into()));
        } else if !queue.driving && queue.current.is_some() {
            rows.push(UpRow::Note("The player plays something of its own; play or queue from here to steer again".into()));
        }
        // What plays is on the now-playing strip; only a track kept from
        // last time is shown here, to go on from.
        if queue.current.is_some() && queue.parked {
            rows.push(UpRow::Heading("Last played".into()));
            rows.push(UpRow::Current);
        }
        if !queue.user.is_empty() {
            rows.push(UpRow::Heading("Next in queue".into()));
            rows.extend((0..queue.user.len()).map(UpRow::Item));
        }
        if !queue.list.is_empty() {
            let from = if queue.source.is_empty() { "Next up".to_string() } else { format!("Next from: {}", queue.source) };
            rows.push(UpRow::Heading(from));
            rows.extend((queue.user.len()..queue.upcoming_len()).map(UpRow::Item));
        }
        if rows.is_empty() {
            rows.push(UpRow::Note("Nothing queued. Play a list, or Add to Queue from a track's menu.".into()));
        }
        rows
    }

    /// The queue gap in front of the row gap `gap`: the first upcoming
    /// track at or after it, or the end.
    fn queue_gap(&self, gap: usize, upcoming: usize) -> usize {
        self.rows.iter().skip(gap).find_map(|row| if let UpRow::Item(at) = row { Some(*at) } else { None }).unwrap_or(upcoming)
    }

    fn item_at(&self, row: usize) -> Option<usize> {
        match self.rows.get(row) {
            Some(UpRow::Item(at)) => Some(*at),
            _ => None,
        }
    }
}

impl UpNextRef {
    pub fn action(&self, actions: &Actions) -> Option<UpNextAction> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            UpNextAction::None => None,
            action => Some(action),
        }
    }
}

impl Widget for UpNext {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(model) = scope.data_mut().get::<Model>() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        let theme = model.theme;
        // A restyle reapplies the grid's defaults: style it again then too.
        if self.theme_applied != Some((theme.background, model.style_gen)) {
            self.theme_applied = Some((theme.background, model.style_gen));
            let mut grid = self.view.widget(cx, ids!(grid));
            style_grid(cx, &mut grid, &theme, theme.raised());
        }
        let width = cx.peek_walk_turtle(walk).size.x;
        let resized = (width - self.width).abs() > 0.5;
        self.width = width;
        let rebuilt = self.built_version != Some(model.version);
        if rebuilt {
            self.built_version = Some(model.version);
            self.rows = Self::build(&model.queue);
        }
        let queue = &model.queue;
        let playing = model.playback.is_playing();
        let rows = self.rows.len();
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            grid.set_grid_size(rows, 1);
            if resized {
                grid.set_col_widths(cx, &[(width - 2.0).max(40.0)]);
            }
            if rebuilt && self.picked.is_some_and(|row| row >= rows) {
                self.picked = None;
                grid.set_selection(cx, None);
            }
            while let Some(cell) = grid.next_cell(cx) {
                if self.hover_any == Some(cell.row) {
                    grid.cell_bg(cx, &cell, hover_tint(&model.theme));
                }
                let Some(row) = self.rows.get(cell.row) else { continue };
                let rect = cell.rect;
                let x = rect.pos.x + 14.0;
                let room = rect.size.x - 28.0;
                let top = rect.pos.y + rect.size.y * 0.34;
                let bottom = rect.pos.y + rect.size.y * 0.70;
                let mid = rect.pos.y + rect.size.y * 0.5;
                // What cannot play now (the system away, no local file)
                // reads faint: it will be skipped.
                let two_lines = |this: &mut Self, cx: &mut Cx2d, item: &crate::queue::QueueItem, accent: bool| {
                    let alpha = if model.can_play(&item.uri) { 1.0 } else { 0.38 };
                    let cached = model.index.is_cached(&item.uri);
                    let right = x + room - if accent { LOCAL_ROOM } else { 0.0 };
                    let text_room = right - x - if cached { LOCAL_ROOM } else { 0.0 };
                    this.draw_title.color = with_alpha(if accent { theme.accent } else { theme.foreground }, alpha);
                    text_fit(&mut this.draw_title, cx, x, top, text_room, &item.name);
                    this.draw_small.color = with_alpha(theme.muted, alpha);
                    text_fit(&mut this.draw_small, cx, x, bottom, text_room, &item.artist);
                    if cached {
                        this.draw_icon.color = CACHED_INK;
                        text_right(&mut this.draw_icon, cx, right, mid, ICON_CACHED);
                    }
                };
                match row {
                    UpRow::Heading(title) => {
                        self.draw_section.color = theme.foreground;
                        text_fit(&mut self.draw_section, cx, x, mid + 6.0, room, title);
                    }
                    UpRow::Note(text) => {
                        self.draw_small.color = theme.muted;
                        text_fit(&mut self.draw_small, cx, x, mid, room, text);
                    }
                    UpRow::Current => {
                        if let Some(current) = &queue.current {
                            two_lines(self, cx, current, true);
                            self.draw_icon.color = theme.accent;
                            text_right(&mut self.draw_icon, cx, rect.pos.x + rect.size.x - 12.0, mid, if playing { ICON_SPEAKER } else { ICON_PAUSE });
                        }
                    }
                    UpRow::Item(at) => {
                        if let Some((_, item)) = queue.upcoming().nth(*at) {
                            let carried = self.carrying == Some(*at);
                            two_lines(self, cx, item, false);
                            if carried {
                                round(&mut self.draw_round, cx, rect, with_alpha(theme.accent, 0.10), 0.0);
                            }
                        }
                    }
                }
                // Where a carried track would land: a line along the gap.
                if self.carrying.is_some() {
                    let line_y = match self.gap {
                        Some(gap) if gap == cell.row => Some(rect.pos.y),
                        Some(gap) if gap == rows && cell.row + 1 == rows => Some(rect.pos.y + rect.size.y - 2.0),
                        _ => None,
                    };
                    if let Some(y) = line_y {
                        round(&mut self.draw_round, cx, Rect { pos: dvec2(rect.pos.x + 8.0, y - 1.0), size: dvec2(rect.size.x - 16.0, 2.0) }, theme.accent, 1.0);
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, &mut Scope::empty());
        let uid = self.widget_uid();
        let upcoming = scope.data_mut().get::<Model>().map(|model| model.queue.upcoming_len()).unwrap_or(0);
        let grid = self.view.data_grid(cx, ids!(grid));
        track_hover(cx, &grid, event, &mut self.hover_any);
        if let Event::KeyDown(key) = event {
            if matches!(key.key_code, KeyCode::Delete | KeyCode::Backspace) && grid.key_focus(cx) {
                if let Some(at) = self.picked.and_then(|row| self.item_at(row)) {
                    cx.widget_action(uid, UpNextAction::Remove(at));
                }
            }
        }
        let Event::Actions(actions) = event else { return };
        for action in grid.actions(actions) {
            match action {
                DataGridAction::SelectionChanged { selection } => {
                    self.picked = selection.map(|selection| selection.head.0);
                }
                DataGridAction::EditCell { row, replace: None, .. } => {
                    if let Some(at) = self.item_at(row) {
                        cx.widget_action(uid, UpNextAction::Jump(at));
                    }
                }
                DataGridAction::CellContextMenu { row, abs, .. } => {
                    if let Some(at) = self.item_at(row) {
                        cx.widget_action(uid, UpNextAction::Menu(at, abs));
                    }
                }
                DataGridAction::RowDragStarted { row, .. } => {
                    self.carrying = self.item_at(row);
                    self.gap = None;
                }
                DataGridAction::RowDragMoved { abs } => {
                    if self.carrying.is_some() {
                        let gap = grid.borrow().and_then(|grid| grid.row_gap_at(abs));
                        if gap != self.gap {
                            self.gap = gap;
                            grid.redraw(cx);
                        }
                    }
                }
                DataGridAction::RowDragEnded { abs, .. } => {
                    let gap = grid.borrow().and_then(|grid| grid.row_gap_at(abs));
                    if let (Some(from), Some(gap)) = (self.carrying.take(), gap) {
                        let to = self.queue_gap(gap, upcoming);
                        cx.widget_action(uid, UpNextAction::Move { from, to });
                    }
                    self.gap = None;
                    grid.redraw(cx);
                }
                _ => {}
            }
        }
    }
}
