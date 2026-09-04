//! ComboBox — an editable, *filtering* selector over a **closed set** of items.
//!
//! This is a selector over a closed set, not a free-text field: half-typed
//! filter text is never itself a value. The control only ever commits ITEMS
//! from `labels` — typing filters the list, Enter commits the highlighted
//! match, Esc or blur restores the previous selection and the filter text
//! evaporates. No action is ever emitted carrying free text.
//!
//! # Interaction contract
//!
//! Conventions followed: the W3C **WAI-ARIA APG combobox pattern**, variant
//! *"Editable Combobox with List Autocomplete"* (`aria-autocomplete="list"`)
//! for the keyboard map and the activedescendant highlight model; **macOS
//! AppKit `NSComboBox`** for the disclosure-arrow and click-outside rules;
//! **VS Code's suggest widget / quick pick** for top-match preselection,
//! match highlighting and the revert-on-blur rule.
//!
//! ## Keyboard (key focus lives in the embedded `TextInput` at all times —
//! the popup highlight is a purely visual "activedescendant")
//!
//! | Key | Popup closed | Popup open |
//! |---|---|---|
//! | printable | opens popup, filters, highlights top match | filters, highlights top match |
//! | Backspace to empty | (as above) shows all items | shows all items, highlights the committed item |
//! | Down | opens popup, highlights **first** item | moves highlight down (wraps) |
//! | Alt+Down | opens popup, highlight unchanged | — |
//! | Up | opens popup, highlights **last** item | moves highlight up (wraps) |
//! | Alt+Up | — | closes popup, keeps the committed value |
//! | PageDown / PageUp | (caret: text start/end, `TextInput`) | moves highlight by one page |
//! | Home / End | caret to start / end of text | caret to start / end of text (APG: never a list jump) |
//! | Enter | nothing | commits the highlighted item and closes; **no match ⇒ nothing happens**, filter stays editable |
//! | Esc | restores the committed label | closes popup **and** restores the committed label |
//! | Tab | moves focus on | commits the highlighted item, then moves focus on |
//!
//! ## Pointer
//!
//! | Gesture | Result |
//! |---|---|
//! | click the arrow / field padding | toggles the popup showing the **full** list, focuses and selects the text |
//! | click the text of an unfocused box | focuses, selects all, opens the full list (keeps plain-dropdown ergonomics) |
//! | click the text of a focused box | places the caret; closes the popup if it was open (macOS: the field counts as "outside") |
//! | hover a row | paints a weak hover highlight — it does **not** move the keyboard highlight |
//! | click a row | commits that item and closes |
//! | click outside | closes and restores the committed label |
//! | wheel / scrollbar drag | scrolls the list |
//!
//! ## Deliberate divergences (macOS-native / closed-set semantics win)
//!
//! * `NSComboBox` accepts typed text as a value; we never do — blur and
//!   click-outside **revert** to the last committed item (VS Code model).
//! * APG's second Escape *clears* the textbox; an empty box is not a member of
//!   a closed set, so a second Escape re-asserts the committed label instead.
//! * APG's list-autocomplete variant highlights nothing until an arrow key is
//!   pressed; Enter-commits-the-top-match needs a target, so the top match is
//!   preselected as you type (VS Code `editor.suggestSelection: "first"`).
//! * No inline ("ghost text") completion — APG's list variant omits it and
//!   completed-but-uncommitted text reads as a phantom value.

use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    drop_down::PopupAnchorTransform,
    makepad_derive_widget::*,
    makepad_draw::*,
    scroll_bar::{ScrollAxis, ScrollBar, ScrollBarAction},
    text_input::{TextInput, TextInputAction},
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawComboItemTextBase = #(DrawComboItemText::script_component(vm))
    set_type_default() do #(DrawComboItemText::script_shader(vm)){
        ..mod.draw.DrawText
    }
    mod.widgets.DrawComboItemBgBase = #(DrawComboItemBg::script_component(vm))
    set_type_default() do #(DrawComboItemBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.ComboBoxBase = #(ComboBox::register_widget(vm))

    mod.widgets.ComboBoxFlat = set_type_default() do mod.widgets.ComboBoxBase{
        width: Fit
        height: Fit
        flow: Right
        align: Align{y: 0.5}
        padding: theme.mspace_1{left: theme.space_2, right: 22.5}
        margin: theme.mspace_v_1{}

        item_height: 22.0
        max_visible_items: 12
        popup_margin: 8.0
        popup_gap: 2.0
        popup_padding: 3.0
        no_match_text: "No matches"

        input: TextInput{
            width: Fill
            height: Fit
            margin: 0.
            padding: 0.
            empty_text: ""
            label_align: Align{y: 0.5}

            draw_text +: {
                color: theme.color_label_inner
                color_hover: theme.color_label_inner_hover
                color_focus: theme.color_label_inner_focus
                color_down: theme.color_label_inner_down
                color_disabled: theme.color_label_inner_disabled
                color_empty: theme.color_text_placeholder
                color_empty_hover: theme.color_text_placeholder_hover
                color_empty_focus: theme.color_text_focus
                text_style: theme.font_regular{ font_size: theme.font_size_p }
            }

            draw_bg +: {
                border_radius: 0.
                border_size: 0.
                color: theme.color_u_hidden
                color_hover: theme.color_u_hidden
                color_focus: theme.color_u_hidden
                color_disabled: theme.color_u_hidden
                color_empty: theme.color_u_hidden
                border_color: theme.color_u_hidden
                border_color_hover: theme.color_u_hidden
                border_color_empty: theme.color_u_hidden
                border_color_disabled: theme.color_u_hidden
                border_color_focus: theme.color_u_hidden
            }
        }

        scroll_bar: mod.widgets.ScrollBar{}

        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            down: instance(0.0)
            disabled: instance(0.0)
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)
            color: uniform(theme.color_outset)
            color_hover: uniform(theme.color_outset_hover)
            color_focus: uniform(theme.color_outset_focus)
            color_down: uniform(theme.color_outset_down)
            color_disabled: uniform(theme.color_outset_disabled)
            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_down: uniform(theme.color_bevel_down)
            arrow_color: uniform(theme.color_label_inner)
            arrow_color_hover: uniform(theme.color_label_inner_hover)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down * self.hover)
                    .mix(self.color_disabled, self.disabled)
                sdf.fill_keep(fill)
                sdf.stroke(
                    self.border_color
                        .mix(self.border_color_focus, self.focus)
                        .mix(self.border_color_hover, self.hover)
                        .mix(self.border_color_down, self.down * self.hover)
                    self.border_size
                )
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5
                sdf.move_to(c.x - sz, c.y - sz * 0.5)
                sdf.line_to(c.x + sz, c.y - sz * 0.5)
                sdf.line_to(c.x, c.y + sz)
                sdf.close_path()
                sdf.fill(self.arrow_color.mix(self.arrow_color_hover, self.hover))
                sdf.result
            }
        }

        draw_popup_bg +: {
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)
            color: uniform(theme.color_fg_app)
            border_color: uniform(theme.color_bevel)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                sdf.result
            }
        }

        draw_item +: {
            hover: 0.0
            active: 0.0
            color: uniform(theme.color_u_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_active: uniform(theme.color_outset_active)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0., 0., self.rect_size.x, self.rect_size.y)
                sdf.fill(
                    self.color
                        .mix(self.color_hover, self.hover)
                        .mix(self.color_active, self.active)
                )
                sdf.result
            }
        }

        draw_item_text +: {
            hover: 0.0
            active: 0.0
            matched: 0.0
            dim: 0.0
            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_active: uniform(theme.color_label_inner_active)
            color_match: uniform(theme.color_label_inner_focus)
            color_dim: uniform(theme.color_label_inner_disabled)
            text_style: theme.font_regular{ font_size: theme.font_size_p }
            get_color: fn() {
                self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_active, self.active)
                    .mix(self.color_match, self.matched)
                    .mix(self.color_dim, self.dim)
            }
        }

        selected_item: 0

        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.}}
                    apply: { draw_bg: {disabled: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: { draw_bg: {disabled: 1.0} }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.1}}
                    apply: { draw_bg: {down: 0.0, hover: 0.0} }
                }
                on: AnimatorState{
                    from: { all: Forward{duration: 0.1} down: Forward{duration: 0.01} }
                    apply: { draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}]} }
                }
                down: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: { draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0} }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: { draw_bg: {focus: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Forward{duration: 0.0}}
                    apply: { draw_bg: {focus: 1.0} }
                }
            }
        }
    }

    mod.widgets.ComboBox = set_type_default() do mod.widgets.ComboBoxFlat{}
}

/// Horizontal space kept free for the scrollbar when it is visible.
const SCROLL_BAR_RESERVE: f64 = 12.0;

// ---------------------------------------------------------------------------
// Pure filter model — the closed-set rules live here so they can be tested
// without a `Cx`. `labels` and the committed index stay on the widget; this
// only owns the *view* state derived from them.
// ---------------------------------------------------------------------------

/// Case-insensitive substring filter over `labels`. An empty filter matches
/// everything; a filter that matches nothing yields an empty set (the
/// "no match" state, in which nothing can be committed).
pub fn filter_indices(labels: &[String], filter: &str) -> Vec<usize> {
    if filter.trim().is_empty() {
        return (0..labels.len()).collect();
    }
    let needle = filter.to_lowercase();
    labels
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// Byte range of the first case-insensitive occurrence of `filter` in `label`,
/// used to paint the matched run in the match colour. `None` when the filter is
/// empty or does not occur.
pub fn match_range(label: &str, filter: &str) -> Option<(usize, usize)> {
    if filter.trim().is_empty() {
        return None;
    }
    let hay = label.to_lowercase();
    let needle = filter.to_lowercase();
    let start = hay.find(&needle)?;
    // Lowercasing can change byte lengths (e.g. 'İ'), so only trust the byte
    // offsets when they still land on char boundaries of the original.
    let end = start + needle.len();
    if label.is_char_boundary(start) && label.is_char_boundary(end) {
        Some((start, end))
    } else {
        None
    }
}

/// Scrolls just enough to bring `row` fully inside a `view_h` tall viewport.
pub fn scroll_row_into_view(scroll: f64, view_h: f64, item_h: f64, row: usize) -> f64 {
    let top = row as f64 * item_h;
    let bot = top + item_h;
    let max = (0.0f64).max(top);
    if top < scroll {
        top
    } else if bot > scroll + view_h {
        (bot - view_h).min(max)
    } else {
        scroll
    }
}

/// The filter/highlight view state of a [`ComboBox`].
#[derive(Clone, Debug, Default)]
pub struct ComboFilter {
    /// Live filter text. Empty means "no filter" — every item is listed.
    pub filter: String,
    /// Indices into the widget's `labels`, in list order.
    pub filtered: Vec<usize>,
    /// Position **within `filtered`** of the visually highlighted row, i.e. the
    /// item Enter would commit. `None` in the no-match state.
    pub highlight: Option<usize>,
    /// True while the text field holds filter text rather than the committed
    /// label. Cleared on commit, revert and blur.
    pub editing: bool,
}

impl ComboFilter {
    /// Drops any filter and lists everything, highlighting the committed item.
    pub fn reset(&mut self, label_count: usize, selected: usize) {
        self.filter.clear();
        self.editing = false;
        self.filtered = (0..label_count).collect();
        self.highlight = if self.filtered.is_empty() {
            None
        } else {
            Some(selected.min(self.filtered.len() - 1))
        };
    }

    /// Applies a new filter string, preselecting the **top match** (VS Code
    /// model) unless the committed item is itself still in the filtered set,
    /// in which case the highlight stays on it.
    pub fn set_filter(&mut self, labels: &[String], filter: &str, selected: usize) {
        self.filter = filter.to_string();
        self.editing = true;
        self.filtered = filter_indices(labels, filter);
        self.highlight = if self.filtered.is_empty() {
            None
        } else if let Some(pos) = self.filtered.iter().position(|i| *i == selected) {
            Some(pos)
        } else {
            Some(0)
        };
    }

    /// The index into `labels` that Enter would commit, or `None` when nothing
    /// matches — in which case Enter must do nothing at all.
    pub fn highlighted_label_index(&self) -> Option<usize> {
        self.filtered.get(self.highlight?).copied()
    }

    pub fn has_matches(&self) -> bool {
        !self.filtered.is_empty()
    }

    pub fn len(&self) -> usize {
        self.filtered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// Moves the highlight by `delta` rows, wrapping at both ends (APG).
    pub fn move_highlight(&mut self, delta: isize) {
        let n = self.filtered.len();
        if n == 0 {
            self.highlight = None;
            return;
        }
        let n_i = n as isize;
        let cur = self.highlight.map(|h| h as isize);
        let next = match cur {
            Some(c) => (c + delta).rem_euclid(n_i),
            None if delta >= 0 => 0,
            None => n_i - 1,
        };
        self.highlight = Some(next as usize);
    }

    /// Moves the highlight by `rows` without wrapping (PageUp/PageDown).
    pub fn page_highlight(&mut self, rows: isize) {
        let n = self.filtered.len();
        if n == 0 {
            self.highlight = None;
            return;
        }
        let cur = self.highlight.unwrap_or(0) as isize;
        let next = (cur + rows).clamp(0, n as isize - 1);
        self.highlight = Some(next as usize);
    }

    pub fn highlight_first(&mut self) {
        self.highlight = if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    pub fn highlight_last(&mut self) {
        self.highlight = if self.filtered.is_empty() {
            None
        } else {
            Some(self.filtered.len() - 1)
        };
    }

    /// Position of the committed item inside the filtered set, if visible.
    pub fn position_of(&self, label_index: usize) -> Option<usize> {
        self.filtered.iter().position(|i| *i == label_index)
    }
}

// ---------------------------------------------------------------------------
// Popup geometry
// ---------------------------------------------------------------------------

/// Where the popup lands, and how tall its scrolling viewport is. The list
/// drops **below** the field (macOS combo box) and flips above when there is
/// not enough room; the height caps at `max_visible_items` rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComboPopupGeom {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Top of the scrolling viewport (below the popup's top padding).
    pub list_y: f64,
    /// Height of the scrolling viewport.
    pub list_h: f64,
    pub item_h: f64,
    pub pad: f64,
    /// Total height of all rows.
    pub content_h: f64,
    /// True when the popup was flipped above the field.
    pub above: bool,
}

impl ComboPopupGeom {
    pub fn max_scroll(&self) -> f64 {
        (self.content_h - self.list_h).max(0.0)
    }

    pub fn needs_scroll_bar(&self) -> bool {
        self.max_scroll() > 0.5
    }

    pub fn popup_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.x, self.y),
            size: dvec2(self.width, self.height),
        }
    }

    pub fn list_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.x + self.pad, self.list_y),
            size: dvec2((self.width - self.pad * 2.0).max(0.0), self.list_h),
        }
    }

    /// Row index (into the *filtered* set) under an absolute point.
    pub fn row_at(&self, abs: Vec2d, scroll: f64, count: usize) -> Option<usize> {
        let list = self.list_rect();
        if !list.contains(abs) {
            return None;
        }
        let local = abs.y - list.pos.y + scroll;
        if local < 0.0 {
            return None;
        }
        let i = (local / self.item_h.max(1.0)).floor() as usize;
        if i < count {
            Some(i)
        } else {
            None
        }
    }
}

/// Lays the popup out under (or over) `trigger`, capped to `visible_rows` rows
/// and to the pass, never overflowing the screen margins.
#[allow(clippy::too_many_arguments)]
pub fn layout_combo_popup(
    pass: Vec2d,
    trigger: Rect,
    row_count: usize,
    visible_rows: usize,
    item_h: f64,
    content_w: f64,
    pad: f64,
    margin: f64,
    gap: f64,
) -> ComboPopupGeom {
    let item_h = item_h.max(1.0);
    let pad = pad.max(0.0);
    let margin = margin.max(0.0);
    let gap = gap.max(0.0);
    let rows = row_count.max(1);
    let visible_rows = visible_rows.max(1);

    let content_h = rows as f64 * item_h;
    let capped_h = pad * 2.0 + content_h.min(visible_rows as f64 * item_h);

    let width = content_w
        .max(trigger.size.x)
        .min((pass.x - margin * 2.0).max(40.0))
        .max(40.0);
    let x = trigger
        .pos
        .x
        .clamp(margin, (pass.x - margin - width).max(margin));

    let below_top = trigger.pos.y + trigger.size.y + gap;
    let below_space = (pass.y - margin - below_top).max(0.0);
    let above_bottom = trigger.pos.y - gap;
    let above_space = (above_bottom - margin).max(0.0);

    let (height, y, above) = if capped_h <= below_space {
        (capped_h, below_top, false)
    } else if capped_h <= above_space {
        (capped_h, above_bottom - capped_h, true)
    } else if below_space >= above_space {
        (below_space.max(item_h + pad * 2.0), below_top, false)
    } else {
        let h = above_space.max(item_h + pad * 2.0);
        (h, (above_bottom - h).max(margin), true)
    };
    let y = y.clamp(margin, (pass.y - margin - height).max(margin));

    let list_h = (height - pad * 2.0).max(item_h);
    ComboPopupGeom {
        x,
        y,
        width,
        height,
        list_y: y + pad,
        list_h,
        item_h,
        pad,
        content_h,
        above,
    }
}

/// Rough width needed to show the longest label without eliding.
pub fn estimate_popup_width(labels: &[String], rows: &[usize], font_px: f64) -> f64 {
    let n = rows
        .iter()
        .filter_map(|i| labels.get(*i))
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(1) as f64;
    n * font_px.max(6.0) * 0.62 + 28.0
}

// ---------------------------------------------------------------------------
// Draw shaders
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawComboItemText {
    #[deref]
    draw_super: DrawText,
    #[live]
    hover: f32,
    #[live]
    active: f32,
    #[live]
    matched: f32,
    #[live]
    dim: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawComboItemBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct ComboBox {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_popup_bg: DrawQuad,
    #[live]
    draw_item: DrawComboItemBg,
    #[live]
    draw_item_text: DrawComboItemText,
    #[live]
    draw_list: DrawList2d,

    /// The closed control: a real text field, so selection, clipboard and IME
    /// all work. Its content is either the committed label or filter text.
    #[live]
    input: TextInput,
    #[live]
    scroll_bar: ScrollBar,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    labels: Vec<String>,
    #[live]
    selected_item: usize,
    #[live(22.0)]
    item_height: f64,
    /// Rows shown before the list starts scrolling.
    #[live(12)]
    max_visible_items: usize,
    #[live(8.0)]
    popup_margin: f64,
    #[live(2.0)]
    popup_gap: f64,
    #[live(3.0)]
    popup_padding: f64,
    #[live(9.0)]
    popup_font_px: f64,
    #[live]
    no_match_text: String,

    #[rust]
    state: ComboFilter,
    #[rust]
    is_open: bool,
    #[rust]
    hover_row: Option<usize>,
    #[rust]
    scroll: f64,
    /// Set when the popup opens: the highlighted row must be scrolled into
    /// view on the next draw, when the viewport height is finally known.
    #[rust]
    scroll_to_highlight: bool,
    #[rust]
    geom: Option<ComboPopupGeom>,
    /// The field's rect as last seen BETWEEN draws (final, aligned). During a
    /// draw the field's own area still sits at its pre-alignment position when
    /// it follows a Fill sibling, so the popup cannot be placed from it.
    #[rust]
    aligned_rect: Option<Rect>,
    /// Optional mapping from a transformed parent draw list into the
    /// window-space overlay where the popup is drawn and hit-tested.
    #[rust]
    popup_anchor_transform: Option<PopupAnchorTransform>,
    #[action_data]
    #[rust]
    action_data: WidgetActionData,
}

#[derive(Clone, Debug, Default)]
pub enum ComboBoxAction {
    /// An ITEM was committed. Carries the index into `labels` — never text.
    Select(usize),
    #[default]
    None,
}

impl ComboBox {
    fn clamp_selected(&mut self) {
        if self.labels.is_empty() {
            self.selected_item = 0;
        } else {
            self.selected_item = self.selected_item.min(self.labels.len() - 1);
        }
    }

    fn selected_label(&self) -> &str {
        self.labels
            .get(self.selected_item)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Pushes the committed label into the text field. Only ever called when
    /// the field is not holding filter text.
    fn sync_text(&mut self, cx: &mut Cx) {
        let label = self.selected_label().to_string();
        if self.input.text() != label {
            self.input.set_text(cx, &label);
            // A closed control shows the START of a long label, not the tail
            // the text field would scroll to after a set_text.
            self.input.set_cursor(
                cx,
                crate::makepad_draw::text::selection::Cursor {
                    index: 0,
                    prefer_next_row: false,
                },
                false,
            );
        }
    }

    /// Drops the full list open without touching key focus. Mirrors
    /// `DropDown2::set_active` so headless captures can pose the popup.
    pub fn set_active(&mut self, cx: &mut Cx) {
        self.open_popup(cx, false);
    }

    /// Closes the list, leaving the committed value alone.
    pub fn set_closed(&mut self, cx: &mut Cx) {
        self.close_popup(cx);
    }

    fn open_popup(&mut self, cx: &mut Cx, keep_filter: bool) {
        self.clamp_selected();
        if !keep_filter {
            self.state.reset(self.labels.len(), self.selected_item);
            self.sync_text(cx);
        }
        if !self.is_open {
            self.is_open = true;
            cx.sweep_lock(self.draw_bg.area());
        }
        self.hover_row = None;
        self.scroll = 0.0;
        self.scroll_to_highlight = true;
        self.geom = None;
        self.draw_bg.redraw(cx);
        self.draw_list.redraw(cx);
    }

    fn close_popup(&mut self, cx: &mut Cx) {
        if self.is_open {
            self.is_open = false;
            cx.sweep_unlock(self.draw_bg.area());
        }
        self.hover_row = None;
        self.geom = None;
        self.draw_bg.redraw(cx);
        self.draw_list.redraw(cx);
    }

    /// Esc / blur / click-outside: the half-typed filter evaporates and the
    /// previously committed item comes back. Never emits an action.
    fn revert(&mut self, cx: &mut Cx) {
        self.state.reset(self.labels.len(), self.selected_item);
        self.sync_text(cx);
        self.close_popup(cx);
    }

    /// Commits the highlighted ITEM. Returns false in the no-match state, in
    /// which case nothing happens at all and the filter stays editable.
    fn commit_highlighted(&mut self, cx: &mut Cx) -> bool {
        let Some(index) = self.state.highlighted_label_index() else {
            return false;
        };
        self.commit_index(cx, index);
        true
    }

    fn commit_index(&mut self, cx: &mut Cx, index: usize) {
        if self.labels.is_empty() {
            return;
        }
        let index = index.min(self.labels.len() - 1);
        let changed = index != self.selected_item;
        self.selected_item = index;
        self.state.reset(self.labels.len(), self.selected_item);
        self.sync_text(cx);
        self.close_popup(cx);
        if changed {
            cx.widget_action_with_data(
                &self.action_data,
                self.uid,
                ComboBoxAction::Select(self.selected_item),
            );
        }
        self.draw_bg.redraw(cx);
    }

    fn visible_rows(&self) -> usize {
        self.max_visible_items.max(1)
    }

    fn set_scroll(&mut self, cx: &mut Cx, scroll: f64) {
        let max = self.geom.map(|g| g.max_scroll()).unwrap_or(0.0);
        let next = scroll.clamp(0.0, max);
        if (next - self.scroll).abs() > f64::EPSILON {
            self.scroll = next;
            self.scroll_bar.set_scroll_pos_no_action(cx, next);
            self.draw_list.redraw(cx);
        }
    }

    fn scroll_highlight_into_view(&mut self, cx: &mut Cx) {
        let Some(g) = self.geom else { return };
        let Some(row) = self.state.highlight else {
            return;
        };
        let next = scroll_row_into_view(self.scroll, g.list_h, g.item_h, row);
        self.set_scroll(cx, next);
    }

    /// Re-runs the filter after the text field changed and keeps the popup in
    /// sync. Typing always opens the popup (APG: printable characters filter
    /// the listbox).
    fn on_text_changed(&mut self, cx: &mut Cx, text: &str) {
        self.state.set_filter(&self.labels, text, self.selected_item);
        if !self.is_open {
            self.is_open = true;
            cx.sweep_lock(self.draw_bg.area());
        }
        self.hover_row = None;
        self.scroll = 0.0;
        self.scroll_to_highlight = true;
        self.draw_bg.redraw(cx);
        self.draw_list.redraw(cx);
    }

    /// Keys the popup owns. Returns true when the key was consumed and must not
    /// reach the text field.
    fn handle_nav_key(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        match ke.key_code {
            KeyCode::ArrowDown => {
                if !self.is_open {
                    self.open_popup(cx, self.state.editing);
                    if !ke.modifiers.alt {
                        self.state.highlight_first();
                    }
                } else {
                    self.state.move_highlight(1);
                }
                self.scroll_highlight_into_view(cx);
                self.draw_list.redraw(cx);
                true
            }
            KeyCode::ArrowUp => {
                if ke.modifiers.alt {
                    if self.is_open {
                        self.close_popup(cx);
                    }
                    return true;
                }
                if !self.is_open {
                    self.open_popup(cx, self.state.editing);
                    self.state.highlight_last();
                } else {
                    self.state.move_highlight(-1);
                }
                self.scroll_highlight_into_view(cx);
                self.draw_list.redraw(cx);
                true
            }
            KeyCode::PageDown if self.is_open => {
                self.state.page_highlight(self.visible_rows() as isize);
                self.scroll_highlight_into_view(cx);
                self.draw_list.redraw(cx);
                true
            }
            KeyCode::PageUp if self.is_open => {
                self.state.page_highlight(-(self.visible_rows() as isize));
                self.scroll_highlight_into_view(cx);
                self.draw_list.redraw(cx);
                true
            }
            KeyCode::ReturnKey => {
                if self.is_open {
                    // No match: Enter does nothing, the filter stays editable.
                    self.commit_highlighted(cx);
                }
                true
            }
            KeyCode::Escape => {
                self.revert(cx);
                true
            }
            KeyCode::Tab => {
                // Commit what is highlighted, then let focus move on.
                if self.is_open && !self.commit_highlighted(cx) {
                    self.revert(cx);
                } else if self.state.editing {
                    self.revert(cx);
                }
                false
            }
            _ => false,
        }
    }

    fn handle_popup_pointer(&mut self, cx: &mut Cx, event: &Event) {
        let Some(g) = self.geom else { return };
        match event {
            Event::Scroll(e) => {
                if g.popup_rect().contains(e.abs) && !e.handled_y.get() {
                    let next = self.scroll + e.scroll.y;
                    self.set_scroll(cx, next);
                    e.handled_y.set(true);
                }
            }
            Event::MouseMove(e) => {
                let row = g.row_at(e.abs, self.scroll, self.state.len());
                if self.hover_row != row {
                    self.hover_row = row;
                    self.draw_list.redraw(cx);
                }
            }
            Event::MouseUp(e) => {
                if let Some(row) = g.row_at(e.abs, self.scroll, self.state.len()) {
                    if let Some(index) = self.state.filtered.get(row).copied() {
                        self.commit_index(cx, index);
                    }
                }
            }
            _ => (),
        }
    }

    fn draw_field(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        let input_walk = self.input.walk(cx);
        let _ = self.input.draw_walk(cx, &mut Scope::empty(), input_walk);
        self.draw_bg.end(cx);
    }

    fn draw_row_text(&mut self, cx: &mut Cx2d, label: &str) {
        let range = if self.state.editing {
            match_range(label, &self.state.filter)
        } else {
            None
        };
        match range {
            Some((s, e)) => {
                if s > 0 {
                    self.draw_item_text.matched = 0.0;
                    self.draw_item_text
                        .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &label[..s]);
                }
                self.draw_item_text.matched = 1.0;
                self.draw_item_text
                    .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &label[s..e]);
                self.draw_item_text.matched = 0.0;
                if e < label.len() {
                    self.draw_item_text
                        .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &label[e..]);
                }
            }
            None => {
                self.draw_item_text.matched = 0.0;
                self.draw_item_text
                    .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, label);
            }
        }
    }

    fn draw_popup(&mut self, cx: &mut Cx2d, trigger: Rect) {
        let pass = cx.current_pass_size();
        let rows = self.state.len();
        let content_w = estimate_popup_width(&self.labels, &self.state.filtered, self.popup_font_px);
        let geom = layout_combo_popup(
            pass,
            trigger,
            rows,
            self.visible_rows(),
            self.item_height,
            content_w,
            self.popup_padding,
            self.popup_margin,
            self.popup_gap,
        );
        self.geom = Some(geom);
        if self.scroll_to_highlight {
            self.scroll_to_highlight = false;
            if let Some(row) = self.state.highlight {
                self.scroll = scroll_row_into_view(self.scroll, geom.list_h, geom.item_h, row);
            }
        }
        self.scroll = self.scroll.clamp(0.0, geom.max_scroll());

        self.draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle(pass, Layout::flow_overlay());

        let popup_walk = Walk::fixed(geom.width, geom.height).with_abs_pos(dvec2(geom.x, geom.y));
        self.draw_popup_bg.begin(
            cx,
            popup_walk,
            Layout::flow_down().with_padding(Inset {
                left: geom.pad,
                right: geom.pad,
                top: geom.pad,
                bottom: geom.pad,
            }),
        );

        cx.begin_turtle(
            Walk::new(Size::fill(), Size::Fixed(geom.list_h)),
            Layout::flow_down(),
        );
        let list_rect = cx.turtle().rect();
        let bar = if geom.needs_scroll_bar() {
            SCROLL_BAR_RESERVE
        } else {
            0.0
        };
        let row_w = (list_rect.size.x - bar).max(1.0);

        if rows == 0 {
            // No-match state: one dim, non-selectable row.
            let text = if self.no_match_text.is_empty() {
                "No matches".to_string()
            } else {
                self.no_match_text.clone()
            };
            self.draw_item.hover = 0.0;
            self.draw_item.active = 0.0;
            self.draw_item.begin(
                cx,
                Walk::fixed(row_w, geom.item_h).with_abs_pos(list_rect.pos),
                row_layout(),
            );
            self.draw_item_text.hover = 0.0;
            self.draw_item_text.active = 0.0;
            self.draw_item_text.dim = 1.0;
            self.draw_item_text.matched = 0.0;
            self.draw_item_text
                .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &text);
            self.draw_item_text.dim = 0.0;
            self.draw_item.end(cx);
        } else {
            let first = (self.scroll / geom.item_h).floor().max(0.0) as usize;
            let last = (((self.scroll + geom.list_h) / geom.item_h).ceil() as usize).min(rows);
            let highlight = self.state.highlight;
            let hover = self.hover_row;
            for row in first..last {
                let Some(label) = self
                    .state
                    .filtered
                    .get(row)
                    .and_then(|i| self.labels.get(*i))
                    .cloned()
                else {
                    continue;
                };
                let y = list_rect.pos.y + row as f64 * geom.item_h - self.scroll;
                let is_active = highlight == Some(row);
                let is_hover = hover == Some(row) && !is_active;
                self.draw_item.hover = if is_hover { 1.0 } else { 0.0 };
                self.draw_item.active = if is_active { 1.0 } else { 0.0 };
                self.draw_item.begin(
                    cx,
                    Walk::fixed(row_w, geom.item_h).with_abs_pos(dvec2(list_rect.pos.x, y)),
                    row_layout(),
                );
                self.draw_item_text.hover = self.draw_item.hover;
                self.draw_item_text.active = self.draw_item.active;
                self.draw_item_text.dim = 0.0;
                self.draw_row_text(cx, &label);
                self.draw_item.end(cx);
            }
        }

        if geom.needs_scroll_bar() {
            self.scroll_bar
                .set_scroll_pos_no_action(cx, self.scroll);
            self.scroll_bar.draw_scroll_bar(
                cx,
                ScrollAxis::Vertical,
                list_rect,
                dvec2(list_rect.size.x, geom.content_h),
            );
        }
        cx.end_turtle();

        self.draw_popup_bg.end(cx);
        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);
    }
}

fn row_layout() -> Layout {
    Layout {
        padding: Inset {
            left: 10.0,
            right: 8.0,
            top: 0.0,
            bottom: 0.0,
        },
        align: Align { x: 0.0, y: 0.5 },
        ..Layout::flow_right()
    }
}

impl Widget for ComboBox {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
        self.input.set_is_read_only(cx, disabled);
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn text(&self) -> String {
        self.selected_label().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(i) = self.labels.iter().position(|l| l == v) {
            self.selected_item = i;
            self.state.reset(self.labels.len(), self.selected_item);
            self.sync_text(cx);
        }
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        // Between draws every deferred alignment has been applied, so this is
        // the field's true on-screen rect (see `aligned_rect`).
        let rect = self.draw_bg.area().rect(cx);
        if rect.size.x > 0.0 && rect.size.y > 0.0 {
            self.aligned_rect = Some(rect);
        }

        // 1. Keys the combobox owns, before the text field can eat them.
        if let Event::KeyDown(ke) = event {
            if self.input.key_focus(cx) && self.handle_nav_key(cx, ke) {
                return;
            }
        }

        // 2. The open popup owns the pointer inside its own rect. The sweep
        //    lock keeps every other widget (including our own text field) from
        //    seeing these, so the geometry hit test below is authoritative.
        let mut dismissed = false;
        if self.is_open {
            let popup_event = self
                .popup_anchor_transform
                .and_then(|transform| transform_combo_popup_event(event, transform));
            let popup_event = popup_event.as_ref().unwrap_or(event);
            let inside = self
                .geom
                .is_some_and(|g| pointer_pos(popup_event).is_some_and(|p| g.popup_rect().contains(p)));
            if inside {
                // The scrollbar hit-tests with its own area and would be
                // refused by our sweep lock; lend it the lock for one dispatch.
                if self.geom.is_some_and(|g| g.needs_scroll_bar()) {
                    cx.sweep_unlock(self.draw_bg.area());
                    let mut scrolled = None;
                    self.scroll_bar
                        .handle_event_with(cx, popup_event, &mut |_cx, action| {
                            if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                                scrolled = Some(scroll_pos);
                            }
                        });
                    cx.sweep_lock(self.draw_bg.area());
                    if let Some(pos) = scrolled {
                        if (pos - self.scroll).abs() > f64::EPSILON {
                            self.scroll = pos;
                            self.draw_list.redraw(cx);
                        }
                        return;
                    }
                    if self.scroll_bar.is_area_captured(cx) {
                        return;
                    }
                }
                self.handle_popup_pointer(cx, popup_event);
                return;
            }
            // Anything pressed outside the popup dismisses it and restores the
            // committed label (macOS: the text field itself counts as
            // "outside"). `dismissed` keeps the press from re-opening the list
            // further down — a click on the arrow must toggle, not cycle.
            if matches!(popup_event, Event::MouseDown(_) | Event::TouchUpdate(_)) {
                self.revert(cx);
                dismissed = true;
            } else {
                self.handle_popup_pointer(cx, popup_event);
            }
        }

        // 3. The text field. It owns clicks on the text, so it must be
        //    dispatched before the field-wide hit test below, or the arrow's
        //    hit area would swallow every press.
        let input_area = self.input.area();
        let input_rect = input_area.rect(cx);
        let was_focused = self.input.key_focus(cx);
        let clicked_text = matches!(event, Event::MouseDown(e) if input_rect.contains(e.abs));
        // A mouse-up on our own chrome (the arrow, the padding) makes a focused
        // TextInput blur itself — "the press landed outside me" — which would
        // throw the filter away every time the arrow is clicked. Keep those
        // ups away from it unless it is mid-drag and owns the finger.
        let chrome_up = matches!(event, Event::MouseUp(e)
            if self.aligned_rect.is_some_and(|r| r.contains(e.abs))
                && !input_rect.contains(e.abs)
                && !cx.fingers.is_area_captured(input_area));
        if !chrome_up {
            for action in cx.capture_actions(|cx| self.input.handle_event(cx, event, scope)) {
                match action.as_widget_action().cast() {
                    TextInputAction::KeyFocus => {
                        self.animator_play(cx, ids!(focus.on));
                    }
                    TextInputAction::KeyFocusLost => {
                        // The field also reports a blur when its area is
                        // rebuilt between frames; only a real focus move may
                        // drop the filter.
                        if !self.input.key_focus(cx) {
                            self.animator_play(cx, ids!(focus.off));
                            self.revert(cx);
                        }
                    }
                    TextInputAction::Changed(text) => {
                        self.on_text_changed(cx, &text);
                    }
                    TextInputAction::Returned(_, _) => {
                        if self.is_open {
                            self.commit_highlighted(cx);
                        }
                    }
                    TextInputAction::Escaped => {
                        self.revert(cx);
                    }
                    _ => (),
                }
            }
        }
        if clicked_text && !was_focused && !dismissed {
            // First click into an unfocused box behaves like the plain
            // dropdown it replaces: select all and drop the full list.
            self.input.select_all(cx);
            self.open_popup(cx, false);
        }

        // 4. The rest of the field — the arrow and the padding around the text.
        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(disabled.off)) {
                    self.animator_play(cx, ids!(hover.down));
                    if self.is_open {
                        self.close_popup(cx);
                    } else if !dismissed {
                        self.input.take_key_focus(cx);
                        self.input.select_all(cx);
                        self.open_popup(cx, false);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.clamp_selected();
        if !self.state.editing {
            // The field shows the committed label whenever it is not holding
            // filter text — this also picks up `labels` arriving from the DSL.
            self.sync_text(cx);
        }
        self.draw_field(cx, walk);
        if self.is_open {
            // Turtle alignment is deferred: a field laid out after a Fill
            // sibling is recorded at its pre-shift position and only moved when
            // the ancestor turtle ends, so the rect visible during THIS draw
            // would put the popup at the row's pre-alignment origin. Place it
            // from the rect captured between draws instead.
            let trigger = self
                .aligned_rect
                .unwrap_or_else(|| self.draw_bg.area().rect(cx));
            let trigger = self
                .popup_anchor_transform
                .map(|transform| transform.rect(trigger))
                .unwrap_or(trigger);
            self.draw_popup(cx, trigger);
        }
        DrawStep::done()
    }
}

impl ComboBoxRef {
    /// Replaces the closed set. The committed index is clamped and the text
    /// field re-shows the (possibly new) committed label.
    pub fn set_labels(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.labels = labels;
            inner.clamp_selected();
            let n = inner.labels.len();
            let selected = inner.selected_item;
            inner.state.reset(n, selected);
            inner.sync_text(cx);
            inner.draw_bg.redraw(cx);
            if inner.is_open {
                inner.draw_list.redraw(cx);
            }
        }
    }

    pub fn labels(&self) -> Vec<String> {
        self.borrow().map(|i| i.labels.clone()).unwrap_or_default()
    }

    /// The index committed by this action batch, if any. Never carries text.
    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ComboBoxAction::Select(id) = item.cast() {
                return Some(id);
            }
        }
        None
    }

    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        self.selected(actions)
    }

    /// The label committed by this action batch, if any.
    pub fn changed_label(&self, actions: &Actions) -> Option<String> {
        let index = self.selected(actions)?;
        self.borrow()?.labels.get(index).cloned()
    }

    pub fn set_selected_item(&self, cx: &mut Cx, item: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            let new_selected = if inner.labels.is_empty() {
                0
            } else {
                item.min(inner.labels.len() - 1)
            };
            if new_selected != inner.selected_item {
                inner.selected_item = new_selected;
                let n = inner.labels.len();
                let selected = inner.selected_item;
                inner.state.reset(n, selected);
                inner.sync_text(cx);
                inner.draw_bg.redraw(cx);
            }
        }
    }

    pub fn selected_item(&self) -> usize {
        self.borrow().map(|inner| inner.selected_item).unwrap_or(0)
    }

    pub fn selected_label(&self) -> String {
        self.borrow()
            .map(|inner| inner.selected_label().to_string())
            .unwrap_or_default()
    }

    /// Selects the first item whose complete label equals `label`.
    pub fn set_selected_by_label(&self, label: &str, cx: &mut Cx) {
        let index = self
            .borrow()
            .and_then(|inner| inner.labels.iter().position(|item| item == label));
        if let Some(index) = index {
            self.set_selected_item(cx, index);
        }
    }

    pub fn set_popup_anchor_transform(
        &self,
        cx: &mut Cx,
        transform: Option<PopupAnchorTransform>,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.popup_anchor_transform != transform {
                inner.popup_anchor_transform = transform;
                inner.draw_bg.redraw(cx);
                inner.draw_list.redraw(cx);
            }
        }
    }

    /// Drops the full list open (headless captures, programmatic reveal).
    pub fn open_list(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx);
        }
    }

    pub fn close_list(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_closed(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open).unwrap_or(false)
    }

    /// Rows shown before the popup starts scrolling.
    pub fn set_max_visible_items(&self, cx: &mut Cx, rows: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.max_visible_items = rows.max(1);
            inner.draw_list.redraw(cx);
        }
    }
}

fn pointer_pos(event: &Event) -> Option<Vec2d> {
    match event {
        Event::MouseDown(e) => Some(e.abs),
        Event::MouseUp(e) => Some(e.abs),
        Event::MouseMove(e) => Some(e.abs),
        Event::Scroll(e) => Some(e.abs),
        Event::TouchUpdate(e) => e.touches.first().map(|t| t.abs),
        _ => None,
    }
}

fn transform_combo_popup_event(event: &Event, transform: PopupAnchorTransform) -> Option<Event> {
    let point = |point: DVec2| transform.rect(Rect { pos: point, size: dvec2(0.0, 0.0) }).pos;
    Some(match event {
        Event::MouseDown(e) => {
            let mut e = e.clone();
            e.abs = point(e.abs);
            Event::MouseDown(e)
        }
        Event::MouseMove(e) => {
            let mut e = e.clone();
            e.abs = point(e.abs);
            e.lock_delta *= transform.scale;
            Event::MouseMove(e)
        }
        Event::MouseUp(e) => {
            let mut e = e.clone();
            e.abs = point(e.abs);
            Event::MouseUp(e)
        }
        Event::MouseLeave(e) => {
            let mut e = e.clone();
            e.abs = point(e.abs);
            Event::MouseLeave(e)
        }
        Event::Scroll(e) => {
            let mut e = e.clone();
            e.abs = point(e.abs);
            e.scroll *= transform.scale;
            Event::Scroll(e)
        }
        Event::LongPress(e) => {
            let mut e = e.clone();
            e.abs = point(e.abs);
            Event::LongPress(e)
        }
        Event::TouchUpdate(e) => {
            let mut e = e.clone();
            for touch in &mut e.touches {
                touch.abs = point(touch.abs);
                touch.radius *= transform.scale;
            }
            Event::TouchUpdate(e)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn combo_ref(cx: &mut Cx) -> ComboBoxRef {
        let combo = cx.with_vm(ComboBox::script_new_with_default);
        WidgetRef::new_with_inner(Box::new(combo)).as_combo_box()
    }

    fn labels() -> Vec<String> {
        [
            "image", "music", "music-hd", "mesh", "matte", "video", "world",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn ref_selects_an_item_by_its_complete_label() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let combo = combo_ref(&mut cx);
        combo.set_labels(&mut cx, labels());

        combo.set_selected_by_label("music-hd", &mut cx);
        assert_eq!(combo.selected_item(), 2);
        assert_eq!(combo.selected_label(), "music-hd");

        combo.set_selected_by_label("missing", &mut cx);
        assert_eq!(combo.selected_item(), 2);
    }

    #[test]
    fn ref_reports_the_label_selected_by_an_action() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let combo = combo_ref(&mut cx);
        combo.set_labels(&mut cx, labels());
        let actions: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(ComboBoxAction::Select(4)),
            widget_uid: combo.widget_uid(),
            group: None,
        })];

        assert_eq!(combo.changed_label(&actions).as_deref(), Some("matte"));
    }

    fn trigger(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    #[test]
    fn popup_anchor_transform_maps_canvas_geometry_and_pointer_input() {
        let transform = PopupAnchorTransform {
            scale: 0.5,
            translation: dvec2(20.0, -5.0),
        };
        assert_eq!(
            transform.rect(trigger(100.0, 80.0, 200.0, 26.0)),
            trigger(70.0, 35.0, 100.0, 13.0)
        );

        let event = Event::MouseDown(MouseDownEvent {
            abs: dvec2(120.0, 100.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        });
        assert!(matches!(
            transform_combo_popup_event(&event, transform),
            Some(Event::MouseDown(event)) if event.abs == dvec2(80.0, 45.0)
        ));
    }

    // -- filtering -----------------------------------------------------------

    #[test]
    fn empty_filter_lists_everything() {
        let l = labels();
        assert_eq!(filter_indices(&l, ""), (0..l.len()).collect::<Vec<_>>());
        assert_eq!(filter_indices(&l, "   "), (0..l.len()).collect::<Vec<_>>());
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        let l = labels();
        assert_eq!(filter_indices(&l, "mus"), vec![1, 2]);
        assert_eq!(filter_indices(&l, "MUS"), vec![1, 2]);
        // substring, not prefix
        assert_eq!(filter_indices(&l, "hd"), vec![2]);
        assert_eq!(filter_indices(&l, "e"), vec![0, 3, 4, 5]);
    }

    #[test]
    fn no_match_yields_empty_set() {
        let l = labels();
        assert!(filter_indices(&l, "zzz").is_empty());
    }

    #[test]
    fn match_range_marks_the_typed_run() {
        assert_eq!(match_range("music-hd", "mus"), Some((0, 3)));
        assert_eq!(match_range("music-hd", "IC"), Some((3, 5)));
        assert_eq!(match_range("music-hd", ""), None);
        assert_eq!(match_range("music-hd", "zz"), None);
    }

    // -- commit / restore semantics -----------------------------------------

    #[test]
    fn typing_preselects_the_top_match_and_enter_commits_it() {
        let l = labels();
        let mut selected = 0usize;
        let mut f = ComboFilter::default();
        f.reset(l.len(), selected);

        f.set_filter(&l, "mus", selected);
        assert_eq!(f.filtered, vec![1, 2]);
        assert_eq!(f.highlight, Some(0));
        // Enter commits the ITEM under the highlight, never the typed text.
        let committed = f.highlighted_label_index().expect("top match");
        assert_eq!(l[committed], "music");
        selected = committed;
        f.reset(l.len(), selected);
        assert!(!f.editing);
        assert_eq!(f.highlight, Some(1));
    }

    #[test]
    fn no_match_commits_nothing_and_esc_restores() {
        let l = labels();
        let selected = 5usize; // "video"
        let mut f = ComboFilter::default();
        f.reset(l.len(), selected);

        f.set_filter(&l, "zzz", selected);
        assert!(!f.has_matches());
        assert_eq!(f.highlight, None);
        // Enter: nothing to commit -> the widget leaves `selected` alone.
        assert_eq!(f.highlighted_label_index(), None);
        assert!(f.editing, "the filter stays editable in the no-match state");

        // Esc / blur: the filter evaporates, the committed item comes back.
        f.reset(l.len(), selected);
        assert_eq!(f.filter, "");
        assert!(!f.editing);
        assert_eq!(f.highlighted_label_index(), Some(selected));
        assert_eq!(l[selected], "video");
    }

    #[test]
    fn filter_keeps_the_committed_item_highlighted_when_it_still_matches() {
        let l = labels();
        let selected = 2usize; // "music-hd"
        let mut f = ComboFilter::default();
        f.set_filter(&l, "mus", selected);
        assert_eq!(f.filtered, vec![1, 2]);
        assert_eq!(f.highlight, Some(1));
        assert_eq!(f.highlighted_label_index(), Some(2));
    }

    #[test]
    fn clearing_the_filter_shows_all_items_again() {
        let l = labels();
        let selected = 3usize;
        let mut f = ComboFilter::default();
        f.set_filter(&l, "mus", selected);
        f.set_filter(&l, "", selected);
        assert_eq!(f.len(), l.len());
        assert_eq!(f.highlighted_label_index(), Some(selected));
    }

    #[test]
    fn arrows_wrap_and_pages_clamp() {
        let l = labels();
        let mut f = ComboFilter::default();
        f.reset(l.len(), 0);
        f.move_highlight(-1);
        assert_eq!(f.highlight, Some(l.len() - 1));
        f.move_highlight(1);
        assert_eq!(f.highlight, Some(0));
        f.page_highlight(12);
        assert_eq!(f.highlight, Some(l.len() - 1));
        f.page_highlight(-12);
        assert_eq!(f.highlight, Some(0));

        f.set_filter(&l, "zzz", 0);
        f.move_highlight(1);
        assert_eq!(f.highlight, None, "no match: nothing to move onto");
    }

    #[test]
    fn empty_label_set_has_no_highlight() {
        let empty: Vec<String> = vec![];
        let mut f = ComboFilter::default();
        f.reset(empty.len(), 0);
        assert_eq!(f.highlight, None);
        assert_eq!(f.highlighted_label_index(), None);
    }

    // -- scroll-into-view ----------------------------------------------------

    #[test]
    fn scroll_into_view_pulls_rows_above_and_below() {
        let item_h = 22.0;
        let view_h = 22.0 * 5.0;
        // already visible -> unchanged
        assert_eq!(scroll_row_into_view(0.0, view_h, item_h, 2), 0.0);
        // below the fold -> scroll just enough
        assert_eq!(
            scroll_row_into_view(0.0, view_h, item_h, 5),
            22.0 * 6.0 - view_h
        );
        // above the fold -> top-align it
        assert_eq!(scroll_row_into_view(220.0, view_h, item_h, 3), 66.0);
        // last row of a long list
        let s = scroll_row_into_view(0.0, view_h, item_h, 40);
        assert_eq!(s, 41.0 * item_h - view_h);
    }

    #[test]
    fn keyboard_walk_keeps_the_highlight_on_screen() {
        let item_h = 22.0;
        let view_h = item_h * 12.0;
        let mut scroll = 0.0;
        for row in 0..40 {
            scroll = scroll_row_into_view(scroll, view_h, item_h, row);
            let top = row as f64 * item_h;
            assert!(top >= scroll - 0.001, "row {row} above viewport");
            assert!(top + item_h <= scroll + view_h + 0.001, "row {row} below");
        }
    }

    // -- popup geometry ------------------------------------------------------

    #[test]
    fn short_list_drops_below_the_field() {
        let g = layout_combo_popup(
            dvec2(800.0, 600.0),
            trigger(40.0, 40.0, 220.0, 28.0),
            5,
            12,
            22.0,
            220.0,
            3.0,
            8.0,
            2.0,
        );
        assert!(!g.above);
        assert!(g.y >= 40.0 + 28.0);
        assert!(g.y + g.height <= 592.0);
        assert!(!g.needs_scroll_bar());
        assert_eq!(g.max_scroll(), 0.0);
    }

    #[test]
    fn long_list_caps_at_max_visible_rows_and_scrolls() {
        let g = layout_combo_popup(
            dvec2(800.0, 900.0),
            trigger(40.0, 30.0, 220.0, 28.0),
            60,
            12,
            22.0,
            220.0,
            3.0,
            8.0,
            2.0,
        );
        assert_eq!(g.height, 3.0 * 2.0 + 12.0 * 22.0);
        assert!(g.needs_scroll_bar());
        assert_eq!(g.max_scroll(), 60.0 * 22.0 - g.list_h);
    }

    #[test]
    fn popup_flips_above_when_there_is_no_room_below() {
        let g = layout_combo_popup(
            dvec2(800.0, 600.0),
            trigger(40.0, 540.0, 220.0, 28.0),
            12,
            12,
            22.0,
            220.0,
            3.0,
            8.0,
            2.0,
        );
        assert!(g.above, "{g:?}");
        assert!(g.y + g.height <= 540.0);
        assert!(g.y >= 8.0);
    }

    #[test]
    fn never_taller_than_the_pass() {
        let g = layout_combo_popup(
            dvec2(400.0, 300.0),
            trigger(10.0, 150.0, 180.0, 24.0),
            80,
            12,
            22.0,
            180.0,
            3.0,
            8.0,
            2.0,
        );
        assert!(g.height <= 300.0 - 16.0);
        assert!(g.y >= 8.0);
        assert!(g.y + g.height <= 292.0);
        assert!(g.needs_scroll_bar());
    }

    #[test]
    fn popup_stays_inside_the_horizontal_margins() {
        let g = layout_combo_popup(
            dvec2(400.0, 600.0),
            trigger(360.0, 100.0, 200.0, 24.0),
            4,
            12,
            22.0,
            300.0,
            3.0,
            8.0,
            2.0,
        );
        assert!(g.x >= 8.0);
        assert!(g.x + g.width <= 392.0);
    }

    #[test]
    fn row_at_maps_points_through_the_scroll_offset() {
        let g = layout_combo_popup(
            dvec2(800.0, 900.0),
            trigger(40.0, 30.0, 220.0, 28.0),
            60,
            12,
            22.0,
            220.0,
            3.0,
            8.0,
            2.0,
        );
        let list = g.list_rect();
        let p = |dy: f64| dvec2(list.pos.x + 4.0, list.pos.y + dy);
        assert_eq!(g.row_at(p(1.0), 0.0, 60), Some(0));
        assert_eq!(g.row_at(p(23.0), 0.0, 60), Some(1));
        // scrolled down by 5 rows
        assert_eq!(g.row_at(p(1.0), 110.0, 60), Some(5));
        // outside the list
        assert_eq!(g.row_at(dvec2(0.0, 0.0), 0.0, 60), None);
        // past the end of a short set
        assert_eq!(g.row_at(p(23.0), 0.0, 1), None);
    }
}
