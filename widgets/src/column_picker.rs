//! Three ways to choose out of a set that will not fit in one list.
//!
//! A drop-down works while the answers fit on one screen and the reader
//! knows which one they want. Past that it stops working, and the three
//! controls in this file are the three shapes that replace it.
//!
//! * **`ColumnPicker`** — columns side by side, one per level. Choosing in a
//!   column fills the one to its right, and choosing again at any level
//!   throws away everything to the right of it. For a deep, narrow hierarchy
//!   where the reader is following a path: a category, then a subcategory,
//!   then the thing.
//! * **`TreeSelect`** — a tree in a popover with boxes that cascade, the
//!   chosen leaves standing as chips on the face. For picking a SET out of a
//!   hierarchy, where the shape of the hierarchy is the reason the set makes
//!   sense.
//! * **`Transfer`** — two lists with move-across controls, a search over
//!   each side and a count on each side. For a long flat set where what
//!   matters is what is in and what is out, both readable at once.
//!
//! They share one flattened forest and one text elider, and nothing else.
//!
//! # What they deliberately do NOT do
//!
//! * **No scrolling of their own.** Every row is drawn every pass and rows
//!   past the stated budget are not drawn at all. That is honest for a
//!   picker of a few dozen rows, which is what all three are for; past that
//!   the answer is a list widget with a viewport, not a scrollbar bolted on
//!   here.
//! * **No data source, no lazy children.** A host hands over the whole set
//!   up front, as an indented outline or a flat list of words, and gets back
//!   typed actions. A column picker that fetched the next column would need
//!   a loading state, a failure state and a cancel, and none of those are
//!   decisions this file gets to make.
//! * **No sorting and no grouping.** The order given is the order shown.
//!
//! The look decisions worth writing down: every mark here is a rectangle, a
//! rounded box, a circle or a TEXT GLYPH. Small marks drawn as shader paths
//! do not paint reliably in this renderer, so there is not one in the file.
//! And the glyphs used are all ones the default face carries — the angle
//! quotes and the guillemets — because a missing glyph draws as an empty
//! box and nothing says so.

use crate::{
    badge::measure,
    event::TouchState,
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{place, PlaceRequest, Placement, Side},
    text_input::TextInputAction,
    widget::*,
    widget_tree::CxWidgetExt,
};
use std::collections::HashSet;

/// Where a glyph's ink begins below the y handed to `draw_abs`, as a share
/// of the font size: the call takes the top of the LINE box, not the ink.
const INK_TOP: f64 = 0.30;

/// The mark on a row that has a column behind it, and the mark on a face
/// that opens a panel. Every glyph named in this file was checked against
/// the default face first: most of the pictorial ranges render as an empty
/// box here, and nothing says when one has.
const BRANCH_MARK: &str = "\u{203A}";
const CARET_MARK: &str = "\u{25BC}";
/// The four move-across marks: one across, all across, and back.
const MOVE_ONE_RIGHT: &str = "\u{203A}";
const MOVE_ALL_RIGHT: &str = "\u{00BB}";
const MOVE_ONE_LEFT: &str = "\u{2039}";
const MOVE_ALL_LEFT: &str = "\u{00AB}";

/// The y to hand `draw_abs` so one line of `font_size` sits centred in a box
/// of `height` starting at `top`. `draw_abs` takes the top of the line box
/// and the ink starts about [`INK_TOP`] of the font size below it, so a run
/// centred on the line box alone rides low by that much.
fn text_y(font_size: f64, top: f64, height: f64) -> f64 {
    top + (height - font_size) * 0.5 - font_size * INK_TOP
}

/// `text` cut to fit `max_w`, with an ellipsis when anything came off.
///
/// The first cut is guessed from the ratio of the measured width to the room
/// available, so a label twice too long costs two or three measurements
/// rather than one per character taken off.
fn elide(draw_text: &DrawText, cx: &mut Cx2d, text: &str, max_w: f64) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let full = measure(draw_text, cx, text);
    if full <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = ((chars.len() as f64) * (max_w / full.max(1.0))).floor() as usize;
    keep = keep.min(chars.len());
    loop {
        if keep == 0 {
            return String::new();
        }
        let mut cut: String = chars[..keep].iter().collect();
        cut.push('\u{2026}');
        if measure(draw_text, cx, &cut) <= max_w {
            return cut;
        }
        keep -= 1;
    }
}

/// How deep an outline line is indented: a tab, or every two spaces, is one
/// level. An odd trailing space is ignored rather than rounded up.
fn indent_of(line: &str) -> usize {
    let mut level = 0;
    let mut spaces = 0;
    for c in line.chars() {
        match c {
            '\t' => {
                level += 1;
                spaces = 0;
            }
            ' ' => {
                spaces += 1;
                if spaces == 2 {
                    level += 1;
                    spaces = 0;
                }
            }
            _ => break,
        }
    }
    level
}

/// One node of the forest the two outline-driven controls here share.
#[derive(Clone, Debug, PartialEq)]
struct Node {
    id: LiveId,
    label: String,
    depth: usize,
    children: Vec<usize>,
}

/// A nested set of names, flattened once into document order.
///
/// It is deliberately smaller than a general tree: no fold state, no icons,
/// no selection, no host-supplied ids. None of the three controls here needs
/// any of those, and a picker carrying a tree's whole state would give two
/// places to look for the answer to "what is chosen".
#[derive(Clone, Debug, Default, PartialEq)]
struct Forest {
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

impl Forest {
    /// A forest written as indented lines. Ids are the line numbers counting
    /// from one, so a host reading an action back can find its own line.
    ///
    /// An indent that jumps more than one level past the line above it is
    /// clamped to one level rather than refused: a hand-written outline with
    /// a stray space should still draw the shape its author meant.
    fn from_outline(lines: &[String]) -> Self {
        let mut forest = Self::default();
        // The node standing at each depth, so a line's parent is whatever is
        // on top once the stack is cut back to that line's own depth.
        let mut stack: Vec<usize> = Vec::new();
        for (line_no, line) in lines.iter().enumerate() {
            let label = line.trim();
            if label.is_empty() {
                continue;
            }
            let depth = indent_of(line).min(stack.len());
            stack.truncate(depth);
            let index = forest.nodes.len();
            forest.nodes.push(Node {
                id: LiveId(line_no as u64 + 1),
                label: label.to_string(),
                depth,
                children: Vec::new(),
            });
            match stack.last() {
                Some(&parent) => forest.nodes[parent].children.push(index),
                None => forest.roots.push(index),
            }
            stack.push(index);
        }
        forest
    }

    fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }

    fn children(&self, index: usize) -> &[usize] {
        self.nodes.get(index).map(|node| node.children.as_slice()).unwrap_or(&[])
    }

    fn is_leaf(&self, index: usize) -> bool {
        self.children(index).is_empty()
    }

    fn label(&self, index: usize) -> &str {
        self.nodes.get(index).map(|node| node.label.as_str()).unwrap_or("")
    }

    fn id(&self, index: usize) -> Option<LiveId> {
        self.nodes.get(index).map(|node| node.id)
    }

    fn index_of(&self, id: LiveId) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The shared surfaces. Every value with a Rust field behind it is
    // written PLAIN and every value the shader alone owns is a uniform:
    // a plain value with no field behind it would take an instance slot and
    // push the fields that do have one off their own, and the row would then
    // read a colour channel as its hover.
    set_type_default() do #(DrawPickerPanel::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: uniform(#00000000)
        border_color: uniform(#00000000)
        /** the outline's thickness in pixels 0..4 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..16 step 0.5 */
        radius: uniform(theme.corner_radius)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            // fill_KEEP: a plain fill wipes the shape and the stroke that
            // follows lands on nothing.
            sdf.fill_keep(self.color)
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerFace::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        open: 0.0
        color: uniform(theme.color_inset)
        color_hover: uniform(theme.color_inset_hover)
        color_open: uniform(theme.color_inset_focus)
        border_color: uniform(theme.color_bevel)
        border_color_open: uniform(theme.color_primary)
        /** the outline's thickness in pixels 0..4 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..16 step 0.5 */
        radius: uniform(theme.corner_radius)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_hover, self.hover).mix(self.color_open, self.open))
            sdf.stroke(self.border_color.mix(self.border_color_open, self.open), self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerRow::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        chosen: 0.0
        keyed: 0.0
        color: uniform(#00000000)
        color_hover: uniform(theme.color_surface_container_high)
        color_chosen: uniform(theme.color_primary_container)
        /** the ring marking where the arrow keys are standing */
        color_ring: uniform(theme.color_primary)
        /** how thick the keyboard ring is 0..3 step 0.5 */
        ring_size: uniform(1.0)
        /** corner rounding radius 0..12 step 0.5 */
        radius: uniform(3.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            // Being chosen is a FILL and being where the keys are is a RING.
            // They are different facts and a reader has to be able to see
            // both at once: a keyed row that borrowed the chosen fill would
            // be claiming a choice nobody has made yet.
            sdf.fill_keep(self.color.mix(self.color_hover, self.hover).mix(self.color_chosen, self.chosen))
            sdf.stroke(vec4(0.0, 0.0, 0.0, 0.0).mix(self.color_ring, self.keyed), self.ring_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerTick::script_shader(vm)){
        ..mod.draw.DrawQuad
        on: 0.0
        mixed: 0.0
        color: uniform(#00000000)
        color_set: uniform(theme.color_primary)
        color_mark: uniform(theme.color_on_primary)
        border_color: uniform(theme.color_outline)
        /** the empty box's stroke 0..3 step 0.5 */
        border_size: uniform(1.0)
        /** box corner radius 0..8 step 0.5 */
        radius: uniform(3.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let s = min(self.rect_size.x, self.rect_size.y)
            let x0 = (self.rect_size.x - s) * 0.5
            let y0 = (self.rect_size.y - s) * 0.5
            let lit = max(self.on, self.mixed)
            sdf.box(
                x0 + self.border_size
                y0 + self.border_size
                s - self.border_size * 2.0
                s - self.border_size * 2.0
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_set, lit))
            sdf.stroke(self.border_color.mix(self.color_set, lit), self.border_size)
            // On is a dot and mixed is a dash: both are shapes this renderer
            // paints every time, where a tick drawn as a path is the mark
            // that goes missing.
            if self.on > 0.5 {
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, s * 0.17)
                sdf.fill(self.color_mark)
            }
            if self.mixed > 0.5 {
                sdf.rect(x0 + s * 0.27, self.rect_size.y * 0.5 - s * 0.07, s * 0.46, s * 0.14)
                sdf.fill(self.color_mark)
            }
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerChip::script_shader(vm)){
        ..mod.draw.DrawQuad
        hot: 0.0
        color: uniform(theme.color_surface_container_high)
        color_hot: uniform(theme.color_error_container)
        border_color: uniform(theme.color_outline_variant)
        /** the outline's thickness in pixels 0..3 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..16 step 0.5 */
        radius: uniform(6.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_hot, self.hot))
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerButton::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        down: 0.0
        color: uniform(theme.color_surface_container)
        color_hover: uniform(theme.color_surface_container_high)
        color_down: uniform(theme.color_surface_container_highest)
        border_color: uniform(theme.color_bevel)
        /** the outline's thickness in pixels 0..3 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..12 step 0.5 */
        radius: uniform(theme.corner_radius)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_hover, self.hover).mix(self.color_down, self.down))
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    mod.widgets.ColumnPickerBase = #(ColumnPicker::register_widget(vm))
    mod.widgets.TreeSelectBase = #(TreeSelect::register_widget(vm))
    mod.widgets.TransferBase = #(Transfer::register_widget(vm))

    /** Columns side by side, one per level: a choice in a column fills the
     * column to its right, and throws away everything beyond it. */
    mod.widgets.ColumnPicker = set_type_default() do mod.widgets.ColumnPickerBase{
        width: Fit
        height: Fit
        padding: Inset{left: 1. right: 1. top: 1. bottom: 1.}

        /** the levels as indented lines: a tab, or every two spaces, is one level */
        outline: []
        /** how wide one column is, in pixels 80..400 step 1 */
        column_width: 150.
        /** how tall one row is 16..40 step 1 */
        row_height: 24.
        /** how many rows tall the columns are; the rest are not drawn 3..40 step 1 */
        row_budget: 8
        /** the hairline between two columns 0..3 step 0.5 */
        rule_size: 1.
        /** room before a label and after a branch mark 0..20 step 1 */
        pad_x: 8.

        draw_bg +: {
            color: theme.color_surface_container_lowest
            border_color: theme.color_outline_variant
        }
        draw_rule +: {
            color: theme.color_outline_variant
            border_size: 0.0
            radius: 0.0
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_text_chosen +: {
            color: theme.color_on_primary_container
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_mark +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }

    /** A tree in a popover with boxes that cascade to the leaves, the chosen
     * leaves standing as chips on the face. */
    mod.widgets.TreeSelect = set_type_default() do mod.widgets.TreeSelectBase{
        width: 260.
        // A stated height, not Fit: the face lays its chips out from Rust
        // against the box it was given, and a Fit face has no box yet.
        height: 30.
        padding: Inset{left: 6. right: 6. top: 4. bottom: 4.}

        /** the tree as indented lines: a tab, or every two spaces, is one level */
        outline: []
        /** what the face says while nothing is ticked */
        placeholder: "Nothing chosen"
        /** how many chips the face shows before it counts the rest 1..8 step 1 */
        chip_limit: 3
        /** how wide the panel is, in pixels 140..420 step 1 */
        panel_width: 240.
        /** how many rows the panel shows; the rest are not drawn 3..40 step 1 */
        row_budget: 12
        /** how tall one panel row is 16..40 step 1 */
        row_height: 22.
        /** how far one level is set in from its parent 8..32 step 1 */
        indent: 14.
        /** the tick box 10..24 step 1 */
        tick_size: 14.
        /** room between two things in a row, and inside a chip 2..16 step 1 */
        gap: 6.

        draw_panel +: {
            color: theme.color_surface_container_low
            border_color: theme.color_outline_variant
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_summary +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_chip_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_caret +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: 8., line_spacing: 1.0}
        }
    }

    /** Two lists with move-across controls, a search over each side and a
     * count on each side. */
    mod.widgets.Transfer = set_type_default() do mod.widgets.TransferBase{
        width: Fill
        height: Fit
        padding: Inset{left: 0. right: 0. top: 0. bottom: 0.}

        /** every item, in the order the lists show them */
        items: []
        /** the heading over the list of what is out */
        left_title: "Available"
        /** the heading over the list of what is in */
        right_title: "Chosen"
        /** how tall one row is 16..40 step 1 */
        row_height: 22.
        /** how tall each list is, in pixels 60..600 step 10 */
        list_height: 170.
        /** the heading strip over each list 0..40 step 1 */
        header_height: 18.
        /** the search box over each list 0..40 step 1 */
        search_height: 24.
        /** how wide the column of move controls is 24..80 step 1 */
        controls_width: 34.
        /** the room between the three columns 0..24 step 1 */
        gap: 8.
        /** room before a label inside a list 0..20 step 1 */
        pad_x: 8.
        /** a search box over each list */
        searchable: true

        left_search: mod.widgets.TextInput{
            empty_text: "Search"
        }
        right_search: mod.widgets.TextInput{
            empty_text: "Search"
        }

        draw_panel +: {
            color: theme.color_surface_container_lowest
            border_color: theme.color_outline_variant
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_title +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_count +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_move +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_move_off +: {
            color: theme.color_text_disabled
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerPanel {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerFace {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    open: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    chosen: f32,
    #[live]
    keyed: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerTick {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    on: f32,
    #[live]
    mixed: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerChip {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hot: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerButton {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
}

// ---------------------------------------------------------------------------
// ColumnPicker
// ---------------------------------------------------------------------------

/// The node chosen at each level, left to right — the whole state of a
/// column picker, apart from where the pointer is.
#[derive(Clone, Debug, Default, PartialEq)]
struct Trail {
    levels: Vec<usize>,
}

impl Trail {
    /// Choose `node` in the column standing at `level`.
    ///
    /// Everything to the RIGHT of that column goes. It has to: those columns
    /// listed the children of a node that is no longer chosen, and a picker
    /// that kept them would be showing a path nobody is standing on, with a
    /// highlighted row in it that a host reading `path_ids` would take as an
    /// answer. Truncating is the whole rule of this control.
    fn choose(&mut self, level: usize, node: usize) {
        self.levels.truncate(level.min(self.levels.len()));
        self.levels.push(node);
    }

    fn at(&self, level: usize) -> Option<usize> {
        self.levels.get(level).copied()
    }

    fn depth(&self) -> usize {
        self.levels.len()
    }

    fn last(&self) -> Option<usize> {
        self.levels.last().copied()
    }

    fn clear(&mut self) {
        self.levels.clear();
    }
}

/// The columns a trail opens: the roots, then the children of each chosen
/// node in turn. A chosen leaf opens nothing, so there is never an empty
/// column hanging off the end.
fn columns(forest: &Forest, trail: &Trail) -> Vec<Vec<usize>> {
    let mut out = vec![forest.roots.clone()];
    for level in 0..trail.depth() {
        let Some(node) = trail.at(level) else {
            break;
        };
        let children = forest.children(node);
        if children.is_empty() {
            break;
        }
        out.push(children.to_vec());
    }
    out
}

/// What a column picker reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ColumnPickerAction {
    /// A row was chosen at this level; everything to the right of it is
    /// already gone by the time this arrives.
    Chosen(usize, LiveId),
    /// The chosen row was a leaf, so the path is as deep as it goes.
    Picked(LiveId),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ColumnPicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The picker's own rect: every row is placed absolutely and so leaves
    /// the widget nothing to be hovered by or found by.
    #[redraw]
    #[live]
    draw_bg: DrawPickerPanel,
    #[live]
    draw_rule: DrawPickerPanel,
    #[live]
    draw_row: DrawPickerRow,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_text_chosen: DrawText,
    #[live]
    pub draw_mark: DrawText,

    /// The levels declared in markup, as indented lines.
    #[live]
    pub outline: Vec<String>,
    #[live(150.0)]
    pub column_width: f64,
    #[live(24.0)]
    pub row_height: f64,
    #[live(8usize)]
    pub row_budget: usize,
    #[live(1.0)]
    pub rule_size: f64,
    #[live(8.0)]
    pub pad_x: f64,

    #[rust]
    forest: Forest,
    #[rust]
    trail: Trail,
    /// The outline this forest was built from, so a live edit rebuilds it
    /// and an unchanged one costs nothing.
    #[rust]
    seeded_from: Vec<String>,
    /// The row under the pointer, as (level, node) rather than as a row
    /// number: choosing changes what is in the columns to the right, and a
    /// row number would then light whatever moved into that slot.
    #[rust]
    hover: Option<(usize, usize)>,
    /// The column the arrow keys are in. Not derived from the trail: the
    /// keys can step back to a shallower column without changing anything
    /// that has been chosen.
    #[rust]
    key_level: usize,
    #[rust]
    area: Area,
}

impl ColumnPicker {
    /// A picker declared in markup shows itself without the host saying
    /// anything, and shows itself again after a live edit changes the lines.
    fn seed(&mut self) {
        if self.seeded_from == self.outline {
            return;
        }
        self.seeded_from = self.outline.clone();
        self.forest = Forest::from_outline(&self.outline);
        // A trail into a forest that has been replaced means nothing.
        self.trail.clear();
        self.key_level = 0;
        self.hover = None;
    }

    /// Replace the levels. The trail is dropped: an index into the old
    /// forest is not an index into this one, and quietly keeping one is how
    /// a picker ends up reporting a row that is not on the screen.
    pub fn set_outline(&mut self, cx: &mut Cx, lines: &[String]) {
        if self.outline == lines {
            return;
        }
        self.outline = lines.to_vec();
        self.seeded_from.clear();
        self.seed();
        self.draw_bg.redraw(cx);
    }

    /// The chosen path, root first.
    pub fn path_ids(&self) -> Vec<LiveId> {
        self.trail.levels.iter().filter_map(|node| self.forest.id(*node)).collect()
    }

    /// The chosen path as its labels, root first.
    pub fn path_labels(&self) -> Vec<String> {
        self.trail.levels.iter().map(|node| self.forest.label(*node).to_string()).collect()
    }

    /// The deepest thing chosen, whether or not it is a leaf.
    pub fn chosen_id(&self) -> Option<LiveId> {
        self.trail.last().and_then(|node| self.forest.id(node))
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.trail.clear();
        self.key_level = 0;
        self.draw_bg.redraw(cx);
    }

    /// Walk a path of ids in from the outside. Ids that do not name a child
    /// of what came before them stop the walk rather than being skipped: a
    /// path with a hole in it is not a shorter path, it is a wrong one.
    pub fn set_path(&mut self, cx: &mut Cx, ids: &[LiveId]) {
        self.seed();
        self.trail.clear();
        for id in ids {
            let Some(node) = self.forest.index_of(*id) else {
                break;
            };
            let level = self.trail.depth();
            let column = columns(&self.forest, &self.trail);
            let Some(rows) = column.get(level) else {
                break;
            };
            if !rows.contains(&node) {
                break;
            }
            self.trail.choose(level, node);
        }
        self.key_level = self.trail.depth().saturating_sub(1);
        self.draw_bg.redraw(cx);
    }

    /// How many rows tall the columns are drawn: the tallest column, capped
    /// by the row budget, because this control does not scroll.
    fn rows_drawn(&self, cols: &[Vec<usize>]) -> usize {
        cols.iter().map(|column| column.len()).max().unwrap_or(0).min(self.row_budget).max(1)
    }

    fn choose(&mut self, cx: &mut Cx, level: usize, node: usize) {
        self.trail.choose(level, node);
        self.key_level = level;
        let uid = self.uid;
        self.draw_bg.redraw(cx);
        if let Some(id) = self.forest.id(node) {
            cx.widget_action(uid, ColumnPickerAction::Chosen(level, id));
            if self.forest.is_leaf(node) {
                cx.widget_action(uid, ColumnPickerAction::Picked(id));
            }
        }
    }

    /// Which row a point lands on, as (level, node).
    ///
    /// Worked out from the rect the pointer landed in and the same
    /// arithmetic the draw uses, rather than from rects captured mid-draw:
    /// the draw pass only knows the widget's PRE-alignment position, so a
    /// picker sitting in a centred row would answer for a place the reader
    /// never pressed.
    fn spot_at(&self, rect: Rect, pos: DVec2) -> Option<(usize, usize)> {
        let inner = Rect {
            pos: dvec2(
                rect.pos.x + self.layout.padding.left,
                rect.pos.y + self.layout.padding.top,
            ),
            size: rect.size,
        };
        let cols = columns(&self.forest, &self.trail);
        let budget = self.rows_drawn(&cols);
        let mut x = inner.pos.x;
        for (level, column) in cols.iter().enumerate() {
            if level > 0 {
                x += self.rule_size;
            }
            if pos.x >= x && pos.x < x + self.column_width {
                let row = ((pos.y - inner.pos.y) / self.row_height.max(1.0)).floor();
                if row < 0.0 {
                    return None;
                }
                let row = row as usize;
                if row < column.len().min(budget) {
                    return Some((level, column[row]));
                }
                return None;
            }
            x += self.column_width;
        }
        None
    }

    /// Step the choice in the keyed column by `by` rows. With nothing chosen
    /// there yet, the first press lands on the first or the last row, so a
    /// keyboard reader never has to click once to get started.
    fn step(&mut self, cx: &mut Cx, by: isize) {
        let cols = columns(&self.forest, &self.trail);
        let level = self.key_level.min(cols.len().saturating_sub(1));
        let Some(column) = cols.get(level) else {
            return;
        };
        if column.is_empty() {
            return;
        }
        let last = column.len() - 1;
        let at = self.trail.at(level).and_then(|node| column.iter().position(|c| *c == node));
        let to = match at {
            Some(at) => (at as isize + by).clamp(0, last as isize) as usize,
            None => {
                if by > 0 {
                    0
                } else {
                    last
                }
            }
        };
        let node = column[to];
        self.choose(cx, level, node);
    }
}

impl Widget for ColumnPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.seed();
        let cols = columns(&self.forest, &self.trail);
        let rows = self.rows_drawn(&cols);

        // A Fill inside a Fit resolves to nothing, so a Fit picker works out
        // its own size before a single row is laid out.
        let natural_w = cols.len() as f64 * self.column_width
            + cols.len().saturating_sub(1) as f64 * self.rule_size
            + self.layout.padding.left
            + self.layout.padding.right;
        let natural_h =
            rows as f64 * self.row_height + self.layout.padding.top + self.layout.padding.bottom;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural_w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        // Read before anything paints: the keyed tint means "the arrow keys
        // are in this column", and setting it afterwards lands on rows that
        // have already drawn.
        let keyed = cx.cx.cx.has_key_focus(self.area);

        self.draw_bg.begin(cx, walk, self.layout);
        // The INNER rect: the turtle's own rect is the outer one, and rows
        // laid out against that ignore the padding the preset asks for.
        let inner = cx.turtle().inner_rect();

        let mark_w = measure(&self.draw_mark, cx, BRANCH_MARK);
        let (row_h, pad_x, col_w) = (self.row_height, self.pad_x, self.column_width);
        let mut x = inner.pos.x;
        for (level, column) in cols.iter().enumerate() {
            if level > 0 && self.rule_size > 0.0 {
                self.draw_rule.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x, inner.pos.y),
                        size: dvec2(self.rule_size, inner.size.y),
                    },
                );
                x += self.rule_size;
            }
            for (row, node) in column.iter().enumerate().take(rows) {
                let node = *node;
                let y = inner.pos.y + row as f64 * row_h;
                let rect = Rect { pos: dvec2(x, y), size: dvec2(col_w, row_h) };
                let chosen = self.trail.at(level) == Some(node);
                let hovered = self.hover == Some((level, node));

                self.draw_row.hover = if hovered { 1.0 } else { 0.0 };
                self.draw_row.chosen = if chosen { 1.0 } else { 0.0 };
                // The ring goes on the chosen row of the keyed column only:
                // ringing every row in that column would say the keyboard is
                // in all of them.
                self.draw_row.keyed =
                    if keyed && chosen && level == self.key_level { 1.0 } else { 0.0 };
                self.draw_row.draw_abs(cx, rect);

                let branch = !self.forest.is_leaf(node);
                let room = col_w - pad_x * 2.0 - if branch { mark_w + pad_x } else { 0.0 };
                let label = self.forest.label(node).to_string();
                // The two pens are asked for their own font size: a host may
                // give the chosen row a heavier face, and centring both on
                // one of the two sizes would leave the other sitting low.
                let pen = if chosen { &mut self.draw_text_chosen } else { &mut self.draw_text };
                let ty = text_y(pen.text_style.font_size as f64, y, row_h);
                let cut = elide(&*pen, cx, &label, room);
                pen.draw_abs(cx, dvec2(x + pad_x, ty), &cut);
                if branch {
                    let mark_x = x + col_w - pad_x - mark_w;
                    let mark_y =
                        text_y(self.draw_mark.text_style.font_size as f64, y, row_h);
                    self.draw_mark.draw_abs(cx, dvec2(mark_x, mark_y), BRANCH_MARK);
                }
            }
            x += col_w;
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        // ONE stop for the whole picker: Tab reaches it, the arrows move
        // inside it, and Tab again leaves it rather than walking every row.
        cx.add_nav_stop(self.area, NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self.spot_at(fe.rect, fe.abs);
                if at != self.hover {
                    self.hover = at;
                    cx.set_cursor(if at.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                let Some((level, node)) = self.spot_at(fe.rect, fe.abs) else {
                    self.draw_bg.redraw(cx);
                    return;
                };
                self.choose(cx, level, node);
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => self.draw_bg.redraw(cx),
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::ArrowUp => self.step(cx, -1),
                KeyCode::ArrowDown => self.step(cx, 1),
                // Right steps into the column a chosen branch opened, and
                // lands on its first row: a press that moved the keys into
                // an empty column would look like the key had done nothing.
                KeyCode::ArrowRight => {
                    let cols = columns(&self.forest, &self.trail);
                    if self.key_level + 1 < cols.len() {
                        self.key_level += 1;
                        if self.trail.at(self.key_level).is_none() {
                            if let Some(&node) = cols[self.key_level].first() {
                                self.choose(cx, self.key_level, node);
                                return;
                            }
                        }
                        self.draw_bg.redraw(cx);
                    }
                }
                KeyCode::ArrowLeft => {
                    if self.key_level > 0 {
                        self.key_level -= 1;
                        self.draw_bg.redraw(cx);
                    }
                }
                KeyCode::Home => self.step(cx, -(i32::MAX as isize)),
                KeyCode::End => self.step(cx, i32::MAX as isize),
                KeyCode::ReturnKey => {
                    if let (Some(node), Some(id)) = (self.trail.last(), self.chosen_id()) {
                        if self.forest.is_leaf(node) {
                            let uid = self.uid;
                            cx.widget_action(uid, ColumnPickerAction::Picked(id));
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// The path, so a test can read the picker in one line.
    fn text(&self) -> String {
        self.path_labels().join(" / ")
    }
}

impl ColumnPickerRef {
    pub fn set_outline(&self, cx: &mut Cx, lines: &[String]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_outline(cx, lines);
        }
    }

    /// The chosen path, root first.
    pub fn path_ids(&self) -> Vec<LiveId> {
        self.borrow().map(|inner| inner.path_ids()).unwrap_or_default()
    }

    pub fn path_labels(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.path_labels()).unwrap_or_default()
    }

    pub fn set_path(&self, cx: &mut Cx, ids: &[LiveId]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_path(cx, ids);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    /// The level and the row chosen this pass.
    pub fn chosen(&self, actions: &Actions) -> Option<(usize, LiveId)> {
        match actions.find_widget_action(self.widget_uid())?.cast::<ColumnPickerAction>() {
            ColumnPickerAction::Chosen(level, id) => Some((level, id)),
            _ => None,
        }
    }

    /// The leaf the path ended on, when this pass ended it.
    pub fn picked(&self, actions: &Actions) -> Option<LiveId> {
        match actions.find_widget_action(self.widget_uid())?.cast::<ColumnPickerAction>() {
            ColumnPickerAction::Picked(id) => Some(id),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TreeSelect
// ---------------------------------------------------------------------------

/// What one box in the tree says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tick {
    Off,
    /// Some of the leaves under this branch are ticked and some are not.
    Mixed,
    On,
}

/// What a node's box shows.
///
/// A branch is **derived, never stored**: its state is read back out of its
/// leaves every time it is asked for. That is the whole of why a parent can
/// never sit there disagreeing with its children, and why [`cascade`] writes
/// to the leaves rather than to the branch it was handed.
fn tick_of(forest: &Forest, ticked: &HashSet<LiveId>, node: usize) -> Tick {
    let Some(entry) = forest.node(node) else {
        return Tick::Off;
    };
    if entry.children.is_empty() {
        return if ticked.contains(&entry.id) { Tick::On } else { Tick::Off };
    }
    let mut any_on = false;
    let mut any_off = false;
    for child in &entry.children {
        match tick_of(forest, ticked, *child) {
            Tick::On => any_on = true,
            Tick::Off => any_off = true,
            Tick::Mixed => {
                any_on = true;
                any_off = true;
            }
        }
    }
    match (any_on, any_off) {
        (true, false) => Tick::On,
        (false, true) => Tick::Off,
        _ => Tick::Mixed,
    }
}

/// Tick or untick a node and everything under it.
///
/// Only leaves are written. Storing the branch as well would give the set
/// two answers to the same question the moment one child changed, and the
/// stale one is the one a naive reader finds first.
fn cascade(forest: &Forest, ticked: &mut HashSet<LiveId>, node: usize, on: bool) {
    let Some(entry) = forest.node(node) else {
        return;
    };
    if entry.children.is_empty() {
        if on {
            ticked.insert(entry.id);
        } else {
            ticked.remove(&entry.id);
        }
        return;
    }
    let children = entry.children.clone();
    for child in children {
        cascade(forest, ticked, child, on);
    }
}

/// The ticked leaves, in document order. Branches are derived and so are
/// never in this list.
fn ticked_leaves(forest: &Forest, ticked: &HashSet<LiveId>) -> Vec<usize> {
    (0..forest.nodes.len())
        .filter(|index| {
            forest.is_leaf(*index)
                && forest.id(*index).is_some_and(|id| ticked.contains(&id))
        })
        .collect()
}

/// What a tree select reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TreeSelectAction {
    /// A box was set; the cascade has already run, so read `chosen` for the
    /// whole answer.
    Toggled(LiveId, bool),
    /// A chip was pressed off the face.
    Removed(LiveId),
    #[default]
    None,
}

/// Popover geometry, in layout points.
const PANEL_PAD: f64 = 6.0;
const PANEL_GAP: f64 = 4.0;
/// Window edge kept free when the panel has to be pushed back on.
const PANEL_EDGE: f64 = 6.0;

#[derive(Script, Widget)]
pub struct TreeSelect {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawPickerFace,
    #[live]
    draw_chip: DrawPickerChip,
    #[live]
    draw_panel: DrawPickerPanel,
    #[live]
    draw_row: DrawPickerRow,
    #[live]
    draw_tick: DrawPickerTick,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_summary: DrawText,
    #[live]
    pub draw_chip_text: DrawText,
    #[live]
    pub draw_caret: DrawText,

    #[live]
    pub outline: Vec<String>,
    #[live]
    pub placeholder: String,
    #[live(3usize)]
    pub chip_limit: usize,
    #[live(240.0)]
    pub panel_width: f64,
    #[live(12usize)]
    pub row_budget: usize,
    #[live(22.0)]
    pub row_height: f64,
    #[live(14.0)]
    pub indent: f64,
    #[live(14.0)]
    pub tick_size: f64,
    #[live(6.0)]
    pub gap: f64,

    #[rust]
    forest: Forest,
    #[rust]
    seeded_from: Vec<String>,
    /// The ticked LEAVES. Branches are worked out from these every draw.
    #[rust]
    ticked: HashSet<LiveId>,
    #[rust]
    open: bool,
    #[rust]
    hover_row: Option<usize>,
    /// The chip under the pointer, so it can say it is a remove target
    /// before it is pressed.
    #[rust]
    hover_chip: Option<usize>,
    /// The row the arrow keys are standing on while the panel is open, and
    /// whether a key has actually put it anywhere. Without the second half
    /// a panel opened with the mouse would draw a ring on its first row,
    /// claiming a keyboard position nobody asked for.
    #[rust]
    key_row: usize,
    #[rust]
    key_used: bool,
    /// Where each chip was drawn and which leaf it stands for.
    #[rust]
    chips: Vec<(Rect, LiveId)>,
    /// The face's FINAL rect and the panel's, both captured on the event
    /// side. Mid-draw the face only knows its pre-alignment position, and
    /// the flip decision needs the place the reader actually pressed.
    #[rust]
    face_rect: Rect,
    #[rust]
    panel_rect: Rect,
    /// The window this drew into last, for the edge flip. The event side has
    /// no `Cx2d` to ask, so the draw side leaves it here.
    #[rust]
    pass_size: DVec2,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for TreeSelect {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl TreeSelect {
    fn seed(&mut self) {
        if self.seeded_from == self.outline {
            return;
        }
        self.seeded_from = self.outline.clone();
        let forest = Forest::from_outline(&self.outline);
        // Ticks survive a live edit only where the same line is still there:
        // the ids are line numbers, so a rewritten outline would otherwise
        // hand its ticks to whatever moved into those lines.
        self.ticked.retain(|id| forest.index_of(*id).is_some());
        self.forest = forest;
        self.hover_row = None;
        self.key_row = 0;
        self.key_used = false;
    }

    /// How many rows the panel draws: the whole tree, capped by the budget,
    /// because the panel does not scroll.
    fn rows_drawn(&self) -> usize {
        self.forest.nodes.len().min(self.row_budget)
    }

    fn panel_size(&self) -> DVec2 {
        let rows = self.rows_drawn().max(1) as f64;
        dvec2(self.panel_width.max(60.0), rows * self.row_height + PANEL_PAD * 2.0)
    }

    /// Offset from the face's top-left to the panel's: below with the left
    /// edges aligned, flipped above when there is room up there and none
    /// down, and pulled back inboard of the window edge.
    fn panel_offset(&self, face: Rect) -> DVec2 {
        let size = self.panel_size();
        let placed = place(&PlaceRequest {
            anchor: face,
            size,
            bounds: Rect {
                pos: dvec2(PANEL_EDGE, PANEL_EDGE),
                size: self.pass_size - dvec2(PANEL_EDGE * 2.0, PANEL_EDGE * 2.0),
            },
            gap: PANEL_GAP,
            placement: Placement::BOTTOM_START,
            match_anchor_width: true,
        });
        // Only the SIDE is read off the placement, not its rect: the helper
        // shortens a popup to the room it has, and a panel shortened from
        // the bottom would drop the last rows with nothing to say so. A
        // panel that overruns the window edge is the lesser fault.
        let y = match placed.side {
            Side::Top => -(size.y + PANEL_GAP),
            _ => face.size.y + PANEL_GAP,
        };
        dvec2(placed.rect.pos.x - face.pos.x, y)
    }

    /// Which panel row a window-absolute point lands on. `None` for the
    /// panel's own padding above the first row and below the last.
    fn row_at(&self, abs: DVec2) -> Option<usize> {
        if !self.panel_rect.contains(abs) {
            return None;
        }
        let row = ((abs.y - self.panel_rect.pos.y - PANEL_PAD) / self.row_height).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        if row < self.rows_drawn() {
            Some(row)
        } else {
            None
        }
    }

    fn chip_at(&self, abs: DVec2) -> Option<usize> {
        self.chips.iter().position(|(rect, _)| rect.contains(abs))
    }

    /// The ticked leaves, in document order.
    pub fn chosen(&self) -> Vec<LiveId> {
        ticked_leaves(&self.forest, &self.ticked)
            .into_iter()
            .filter_map(|node| self.forest.id(node))
            .collect()
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        ticked_leaves(&self.forest, &self.ticked)
            .into_iter()
            .map(|node| self.forest.label(node).to_string())
            .collect()
    }

    pub fn set_chosen(&mut self, cx: &mut Cx, ids: &[LiveId]) {
        self.seed();
        // Only leaves: a branch id in the incoming set would be stored
        // alongside its own leaves and then contradict them.
        let ticked: HashSet<LiveId> = ids
            .iter()
            .copied()
            .filter(|id| self.forest.index_of(*id).is_some_and(|at| self.forest.is_leaf(at)))
            .collect();
        self.ticked = ticked;
        self.redraw_all(cx);
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
    }

    /// Opening takes the pointer for the whole widget tree and closing hands
    /// it back. Without it the panel is a picture: the widgets it floats
    /// over were walked first and have acted on the press already.
    ///
    /// An empty tree refuses to open. A panel that answers nothing is a hole
    /// in the screen, and one holding the pointer lock is a hole that
    /// swallows the rest of the window with it.
    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        let open = open && !self.forest.is_empty();
        if self.open == open {
            return;
        }
        self.open = open;
        self.hover_row = None;
        self.key_used = false;
        if open {
            cx.sweep_lock(self.draw_bg.area());
        } else {
            cx.sweep_unlock(self.draw_bg.area());
        }
        self.draw_bg.open = if open { 1.0 } else { 0.0 };
        self.redraw_all(cx);
    }

    fn toggle_row(&mut self, cx: &mut Cx, row: usize) {
        let Some(id) = self.forest.id(row) else {
            return;
        };
        // Mixed resolves to on, the way a select-all box does: the useful
        // next step from "some of them" is "all of them", not "none".
        let on = tick_of(&self.forest, &self.ticked, row) != Tick::On;
        cascade(&self.forest, &mut self.ticked, row, on);
        self.key_row = row;
        let uid = self.uid;
        self.redraw_all(cx);
        cx.widget_action(uid, TreeSelectAction::Toggled(id, on));
    }

    /// A press while the panel is open. True when it landed inside the
    /// panel, meaning the caller must mark the event handled: the sweep lock
    /// turned away everything walked BEFORE this widget, and this stops
    /// anything walked after it.
    fn press_at(&mut self, cx: &mut Cx, abs: DVec2, primary: bool) -> bool {
        if self.panel_rect.contains(abs) {
            if primary {
                if let Some(row) = self.row_at(abs) {
                    self.toggle_row(cx, row);
                }
            }
            return true;
        }
        if !self.face_rect.contains(abs) {
            self.set_open(cx, false);
        }
        false
    }

    fn hover_at(&mut self, cx: &mut Cx, abs: DVec2) {
        let row = self.row_at(abs);
        if row != self.hover_row {
            self.hover_row = row;
            self.redraw_all(cx);
        }
    }
}

impl Widget for TreeSelect {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.seed();

        self.draw_bg.begin(cx, walk, self.layout);
        let inner = cx.turtle().inner_rect();
        self.chips.clear();

        let chip_font = self.draw_chip_text.text_style.font_size as f64;
        let caret_font = self.draw_caret.text_style.font_size as f64;
        let caret_w = measure(&self.draw_caret, cx, CARET_MARK);
        let chosen = ticked_leaves(&self.forest, &self.ticked);
        let room_x = inner.pos.x + inner.size.x - caret_w - self.gap;

        if chosen.is_empty() {
            let placeholder = self.placeholder.clone();
            let text = elide(&self.draw_summary, cx, &placeholder, room_x - inner.pos.x);
            let font = self.draw_summary.text_style.font_size as f64;
            let y = text_y(font, inner.pos.y, inner.size.y);
            self.draw_summary.draw_abs(cx, dvec2(inner.pos.x, y), &text);
        } else {
            let chip_h = (inner.size.y - 4.0).max(12.0);
            let chip_y = inner.pos.y + (inner.size.y - chip_h) * 0.5;
            let mut x = inner.pos.x;
            let mut shown = 0;
            for node in chosen.iter().take(self.chip_limit) {
                let label = self.forest.label(*node).to_string();
                let label_w = measure(&self.draw_chip_text, cx, &label);
                let width = label_w + self.gap * 2.0;
                // A chip that would run under the caret is not drawn at all:
                // half a chip reads as a whole one with a short name.
                if x + width > room_x {
                    break;
                }
                let rect = Rect { pos: dvec2(x, chip_y), size: dvec2(width, chip_h) };
                let at = self.chips.len();
                self.draw_chip.hot = if self.hover_chip == Some(at) { 1.0 } else { 0.0 };
                self.draw_chip.draw_abs(cx, rect);
                self.draw_chip_text.draw_abs(
                    cx,
                    dvec2(x + self.gap, text_y(chip_font, chip_y, chip_h)),
                    &label,
                );
                if let Some(id) = self.forest.id(*node) {
                    self.chips.push((rect, id));
                }
                x += width + 4.0;
                shown += 1;
            }
            let rest = chosen.len() - shown;
            if rest > 0 {
                let text = format!("+{rest}");
                let font = self.draw_summary.text_style.font_size as f64;
                // Pulled back off the caret rather than allowed to run under
                // it: the count is the only thing saying the face is not the
                // whole answer, so it is the one run that must stay legible.
                let w = measure(&self.draw_summary, cx, &text);
                let at = x.min(room_x - w).max(inner.pos.x);
                self.draw_summary.draw_abs(
                    cx,
                    dvec2(at, text_y(font, inner.pos.y, inner.size.y)),
                    &text,
                );
            }
        }

        self.draw_caret.draw_abs(
            cx,
            dvec2(room_x + self.gap, text_y(caret_font, inner.pos.y, inner.size.y)),
            CARET_MARK,
        );
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());

        if self.open {
            // Everything the overlay pass needs is worked out here, before
            // the draw list is borrowed out of `self`.
            self.pass_size = cx.current_pass_size();
            let drawn = self.draw_bg.area().rect(cx);
            let anchor = Rect {
                pos: if self.face_rect.size.y > 0.0 { self.face_rect.pos } else { drawn.pos },
                size: drawn.size,
            };
            let panel_size = self.panel_size();
            let offset = self.panel_offset(anchor);
            let rows = self.rows_drawn();
            let font = self.draw_text.text_style.font_size as f64;
            let mut ticks = Vec::with_capacity(rows);
            let mut labels = Vec::with_capacity(rows);
            for row in 0..rows {
                ticks.push(tick_of(&self.forest, &self.ticked, row));
                let depth = self.forest.node(row).map(|node| node.depth).unwrap_or(0);
                labels.push((depth, self.forest.label(row).to_string()));
            }
            let hover_row = self.hover_row;
            let key_row = if self.key_used { Some(self.key_row) } else { None };
            let row_h = self.row_height;
            let tick_size = self.tick_size;
            let indent = self.indent;
            let gap = self.gap;
            if let Some(draw_list) = self.draw_list.as_mut() {
                // The proven popup idiom: draw the panel as turtle content
                // at the overlay root, then SHIFT the whole list to hang off
                // the face. A `draw_abs` into a bare overlay list renders
                // nothing at all.
                draw_list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                self.draw_panel.begin(
                    cx,
                    Walk::fixed(panel_size.x, panel_size.y),
                    Layout::default(),
                );
                let panel = cx.turtle().rect();
                for row in 0..rows {
                    let y = panel.pos.y + PANEL_PAD + row as f64 * row_h;
                    let rect = Rect {
                        pos: dvec2(panel.pos.x + PANEL_PAD, y),
                        size: dvec2((panel.size.x - PANEL_PAD * 2.0).max(1.0), row_h),
                    };
                    // A panel row is never "chosen": what is chosen here is
                    // said by the box, not by the row's fill. The ring is
                    // only where the arrow keys are standing.
                    self.draw_row.hover = if hover_row == Some(row) { 1.0 } else { 0.0 };
                    self.draw_row.chosen = 0.0;
                    self.draw_row.keyed = if key_row == Some(row) { 1.0 } else { 0.0 };
                    self.draw_row.draw_abs(cx, rect);

                    let (depth, label) = &labels[row];
                    let x = rect.pos.x + gap + *depth as f64 * indent;
                    self.draw_tick.on = if ticks[row] == Tick::On { 1.0 } else { 0.0 };
                    self.draw_tick.mixed = if ticks[row] == Tick::Mixed { 1.0 } else { 0.0 };
                    self.draw_tick.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y + (row_h - tick_size) * 0.5),
                            size: dvec2(tick_size, tick_size),
                        },
                    );
                    let text_x = x + tick_size + gap;
                    let room = rect.pos.x + rect.size.x - text_x - gap;
                    let cut = elide(&self.draw_text, cx, label, room);
                    self.draw_text.draw_abs(cx, dvec2(text_x, text_y(font, y, row_h)), &cut);
                }
                self.draw_panel.end(cx);
                cx.end_pass_sized_turtle_with_shift(self.draw_bg.area(), offset);
                draw_list.end(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.open {
            // Raw events rather than `hits`: the panel has no area of its
            // own, and the sweep lock taken in `set_open` is what keeps
            // every other widget out of these same presses.
            let face = self.draw_bg.area().rect(cx);
            self.face_rect = face;
            self.panel_rect =
                Rect { pos: face.pos + self.panel_offset(face), size: self.panel_size() };
            match event {
                Event::MouseDown(me) => {
                    if self.press_at(cx, me.abs, me.button.is_primary()) {
                        me.handled.set(self.draw_bg.area());
                    }
                }
                // Touch never becomes a MouseDown: `hits` synthesises a
                // FingerDown for the face, but the panel is not an area, so
                // without this the panel opens on a phone and then answers
                // nothing at all.
                Event::TouchUpdate(te) => {
                    if let Some(touch) = te.touches.first() {
                        match touch.state {
                            TouchState::Start => {
                                if self.press_at(cx, touch.abs, true) {
                                    touch.handled.set(self.draw_bg.area());
                                }
                            }
                            TouchState::Move => self.hover_at(cx, touch.abs),
                            _ => {}
                        }
                    }
                }
                Event::MouseMove(me) => self.hover_at(cx, me.abs),
                Event::KeyDown(ke) => {
                    let rows = self.rows_drawn();
                    match ke.key_code {
                        KeyCode::Escape | KeyCode::ReturnKey => self.set_open(cx, false),
                        // Without these the boxes are reachable by pointer
                        // only, which makes the whole control unusable from
                        // the keyboard.
                        KeyCode::ArrowUp => {
                            self.key_row = if self.key_used {
                                self.key_row.saturating_sub(1)
                            } else {
                                rows.saturating_sub(1)
                            };
                            self.key_used = true;
                            self.redraw_all(cx);
                        }
                        KeyCode::ArrowDown => {
                            if rows > 0 {
                                self.key_row = if self.key_used {
                                    (self.key_row + 1).min(rows - 1)
                                } else {
                                    0
                                };
                                self.key_used = true;
                                self.redraw_all(cx);
                            }
                        }
                        KeyCode::Space => {
                            if self.key_used && self.key_row < rows {
                                let row = self.key_row;
                                self.toggle_row(cx, row);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // Named as its own sweep area, or the lock this widget took would
        // turn the face's own hits away along with everyone else's.
        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.draw_bg.hover = 1.0;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.hover = 0.0;
                if self.hover_chip.take().is_some() {
                    self.redraw_all(cx);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOver(fe) => {
                let chip = self.chip_at(fe.abs);
                if chip != self.hover_chip {
                    self.hover_chip = chip;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.face_rect = self.draw_bg.area().rect(cx);
                // A chip is a remove target, so a press on one takes that
                // leaf off rather than opening the panel. Without this the
                // chips would be decoration and every removal would be a
                // round trip through the tree.
                if let Some(at) = self.chip_at(fe.abs) {
                    let id = self.chips[at].1;
                    if let Some(node) = self.forest.index_of(id) {
                        cascade(&self.forest, &mut self.ticked, node, false);
                        let uid = self.uid;
                        self.hover_chip = None;
                        self.redraw_all(cx);
                        cx.widget_action(uid, TreeSelectAction::Removed(id));
                    }
                    return;
                }
                cx.set_key_focus(self.draw_bg.area());
                let open = self.open;
                self.set_open(cx, !open);
            }
            _ => {}
        }
    }

    /// What the face says, so a test can read the control in one line.
    fn text(&self) -> String {
        let labels = self.chosen_labels();
        if labels.is_empty() {
            self.placeholder.clone()
        } else {
            labels.join(", ")
        }
    }
}

impl TreeSelectRef {
    /// The ticked leaves, in document order.
    pub fn chosen(&self) -> Vec<LiveId> {
        self.borrow().map(|inner| inner.chosen()).unwrap_or_default()
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.chosen_labels()).unwrap_or_default()
    }

    pub fn set_chosen(&self, cx: &mut Cx, ids: &[LiveId]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_chosen(cx, ids);
        }
    }

    /// The box set this pass, and what it was set to.
    pub fn toggled(&self, actions: &Actions) -> Option<(LiveId, bool)> {
        match actions.find_widget_action(self.widget_uid())?.cast::<TreeSelectAction>() {
            TreeSelectAction::Toggled(id, on) => Some((id, on)),
            _ => None,
        }
    }

    /// The chip taken off the face this pass.
    pub fn removed(&self, actions: &Actions) -> Option<LiveId> {
        match actions.find_widget_action(self.widget_uid())?.cast::<TreeSelectAction>() {
            TreeSelectAction::Removed(id) => Some(id),
            _ => None,
        }
    }

    /// True when the set changed this pass, however it changed.
    pub fn changed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast::<TreeSelectAction>()),
            Some(TreeSelectAction::Toggled(..)) | Some(TreeSelectAction::Removed(_))
        )
    }
}

// ---------------------------------------------------------------------------
// Transfer
// ---------------------------------------------------------------------------

/// Which side each item stands on, and the moves between the two.
///
/// One bool per item rather than two lists of indices: with two lists there
/// are two places an item can be and therefore a state where it is in both,
/// or in neither, and every move has to be written twice to keep them
/// honest. With one array the two sides are views of the same fact, and an
/// item is on exactly one side by construction.
#[derive(Clone, Debug, Default, PartialEq)]
struct Sides {
    across: Vec<bool>,
}

impl Sides {
    fn fit(&mut self, len: usize) {
        self.across.resize(len, false);
    }

    /// The items on one side, in ITEM order — never the order they were
    /// moved in. Two readers who moved the same set in a different sequence
    /// should be looking at the same list.
    fn side(&self, across: bool) -> Vec<usize> {
        (0..self.across.len()).filter(|index| self.across[*index] == across).collect()
    }

    fn counts(&self) -> (usize, usize) {
        let across = self.across.iter().filter(|on| **on).count();
        (self.across.len() - across, across)
    }

    /// Move `picked` to one side; how many actually moved comes back.
    ///
    /// Anything already on that side is left alone rather than counted
    /// again, so a move of a set that overlaps what is already across is a
    /// no-op for the overlap instead of a duplicate.
    fn move_across(&mut self, picked: &[usize], across: bool) -> usize {
        let mut moved = 0;
        for index in picked {
            let Some(slot) = self.across.get_mut(*index) else {
                continue;
            };
            if *slot != across {
                *slot = across;
                moved += 1;
            }
        }
        moved
    }
}

/// Case-insensitive substring. An empty query matches everything, so a side
/// with nothing typed into it shows the whole side.
fn search_matches(label: &str, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    label.to_lowercase().contains(&query.trim().to_lowercase())
}

/// The rows one side shows: its items, in item order, that pass the search.
///
/// This is the list the move-all controls act on, NOT the whole side. A
/// move-all that reached past the search would take rows the reader cannot
/// see and did not ask for, which is the one way a transfer control can
/// silently lose somebody's work.
fn showing(items: &[String], side: &[usize], query: &str) -> Vec<usize> {
    side.iter()
        .copied()
        .filter(|index| {
            items.get(*index).is_some_and(|label| search_matches(label, query))
        })
        .collect()
}

/// What a transfer reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TransferAction {
    /// Some items moved; true means they moved to the right-hand list. Read
    /// `chosen` for the whole answer.
    Moved(usize, bool),
    #[default]
    None,
}

/// Which control was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    OneRight,
    AllRight,
    OneLeft,
    AllLeft,
}

impl Move {
    const ORDER: [Move; 4] = [Move::OneRight, Move::AllRight, Move::OneLeft, Move::AllLeft];

    fn mark(self) -> &'static str {
        match self {
            Move::OneRight => MOVE_ONE_RIGHT,
            Move::AllRight => MOVE_ALL_RIGHT,
            Move::OneLeft => MOVE_ONE_LEFT,
            Move::AllLeft => MOVE_ALL_LEFT,
        }
    }

    fn to_right(self) -> bool {
        matches!(self, Move::OneRight | Move::AllRight)
    }

    fn is_all(self) -> bool {
        matches!(self, Move::AllRight | Move::AllLeft)
    }
}

/// Where the three columns of a transfer were laid out. Worked out from the
/// widget's own rect by both the draw and the hit test, so what is drawn is
/// what is pressable even after the parent's alignment has moved the whole
/// control.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    left_search: Rect,
    right_search: Rect,
    left_list: Rect,
    right_list: Rect,
    buttons: [Rect; 4],
    header_y: f64,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Transfer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawPickerPanel,
    #[live]
    draw_panel: DrawPickerPanel,
    #[live]
    draw_row: DrawPickerRow,
    #[live]
    draw_button: DrawPickerButton,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_title: DrawText,
    #[live]
    pub draw_count: DrawText,
    #[live]
    pub draw_move: DrawText,
    #[live]
    pub draw_move_off: DrawText,

    /// The search box over each list. They are real inputs, so selection,
    /// the clipboard and undo all work in them.
    #[find]
    #[live]
    pub left_search: WidgetRef,
    #[find]
    #[live]
    pub right_search: WidgetRef,

    #[live]
    pub items: Vec<String>,
    #[live]
    pub left_title: String,
    #[live]
    pub right_title: String,
    #[live(22.0)]
    pub row_height: f64,
    #[live(170.0)]
    pub list_height: f64,
    #[live(18.0)]
    pub header_height: f64,
    #[live(24.0)]
    pub search_height: f64,
    #[live(34.0)]
    pub controls_width: f64,
    #[live(8.0)]
    pub gap: f64,
    #[live(8.0)]
    pub pad_x: f64,
    #[live(true)]
    pub searchable: bool,

    #[rust]
    sides: Sides,
    /// The highlighted items. ONE set, not one per side: an item is on
    /// exactly one side, so a second set would only ever be able to
    /// disagree with the first.
    #[rust]
    picked: HashSet<usize>,
    #[rust]
    hover_row: Option<(bool, usize)>,
    #[rust]
    hover_button: Option<usize>,
    #[rust]
    down_button: Option<usize>,
    #[rust]
    frame: Frame,
    #[rust]
    area: Area,
}

impl Transfer {
    fn fit(&mut self) {
        if self.sides.across.len() != self.items.len() {
            self.sides.fit(self.items.len());
            self.picked.retain(|index| *index < self.items.len());
        }
    }

    fn query(&self, across: bool) -> String {
        if !self.searchable {
            return String::new();
        }
        if across {
            self.right_search.text()
        } else {
            self.left_search.text()
        }
    }

    /// The rows one side shows, after its own search.
    fn rows(&self, across: bool) -> Vec<usize> {
        let side = self.sides.side(across);
        showing(&self.items, &side, &self.query(across))
    }

    /// Everything the two lists and the four buttons need, from the widget's
    /// own rect. Both the draw and the hit test go through this, so a
    /// pressable row is always a drawn row.
    fn frame_of(&self, rect: Rect) -> Frame {
        let inner = Rect {
            pos: dvec2(
                rect.pos.x + self.layout.padding.left,
                rect.pos.y + self.layout.padding.top,
            ),
            size: dvec2(
                (rect.size.x - self.layout.padding.left - self.layout.padding.right).max(0.0),
                (rect.size.y - self.layout.padding.top - self.layout.padding.bottom).max(0.0),
            ),
        };
        let panel_w =
            ((inner.size.x - self.controls_width - self.gap * 2.0) * 0.5).max(40.0);
        let left_x = inner.pos.x;
        let controls_x = left_x + panel_w + self.gap;
        let right_x = controls_x + self.controls_width + self.gap;
        let search_h = if self.searchable { self.search_height } else { 0.0 };
        let list_y = inner.pos.y + self.header_height + search_h;

        let search = |x: f64| Rect {
            pos: dvec2(x, inner.pos.y + self.header_height),
            size: dvec2(panel_w, search_h),
        };
        let list = |x: f64| Rect {
            pos: dvec2(x, list_y),
            size: dvec2(panel_w, self.list_height),
        };

        // The four controls are centred against the LISTS, not the whole
        // widget: a column of buttons lined up with the headings sits high
        // and reads as belonging to the left panel.
        let button = self.controls_width.min(28.0);
        let spread = button * 4.0 + 6.0 * 3.0;
        let top = list_y + (self.list_height - spread) * 0.5;
        let bx = controls_x + (self.controls_width - button) * 0.5;
        let mut buttons = [Rect::default(); 4];
        for (index, slot) in buttons.iter_mut().enumerate() {
            *slot = Rect {
                pos: dvec2(bx, top + index as f64 * (button + 6.0)),
                size: dvec2(button, button),
            };
        }

        Frame {
            left_search: search(left_x),
            right_search: search(right_x),
            left_list: list(left_x),
            right_list: list(right_x),
            buttons,
            header_y: inner.pos.y,
        }
    }

    /// Which row a window-absolute point lands on, as (side, item index).
    fn row_at(&self, pos: DVec2) -> Option<(bool, usize)> {
        for across in [false, true] {
            let list = if across { self.frame.right_list } else { self.frame.left_list };
            if !list.contains(pos) {
                continue;
            }
            let row = ((pos.y - list.pos.y) / self.row_height).floor();
            if row < 0.0 {
                return None;
            }
            let rows = self.rows(across);
            let row = row as usize;
            if row < rows.len() && row < self.row_budget() {
                return Some((across, rows[row]));
            }
            return None;
        }
        None
    }

    /// How many rows fit in a list. This control does not scroll, so a row
    /// past this is not drawn and not pressable — the two have to agree.
    fn row_budget(&self) -> usize {
        (self.list_height / self.row_height.max(1.0)).floor().max(0.0) as usize
    }

    /// What a control would move, given the searches as they stand.
    fn payload(&self, action: Move) -> Vec<usize> {
        let from = !action.to_right();
        if action.is_all() {
            self.rows(from)
        } else {
            // Only the picked items on the side being emptied, in item
            // order. A picked item on the OTHER side is not in this move.
            self.rows(from).into_iter().filter(|index| self.picked.contains(index)).collect()
        }
    }

    fn run(&mut self, cx: &mut Cx, action: Move) {
        let payload = self.payload(action);
        if payload.is_empty() {
            return;
        }
        let moved = self.sides.move_across(&payload, action.to_right());
        // The highlight goes with the move: an item that has crossed and is
        // still lit would be moved straight back by the next press of the
        // control facing the other way.
        for index in &payload {
            self.picked.remove(index);
        }
        let uid = self.uid;
        self.draw_bg.redraw(cx);
        cx.widget_action(uid, TransferAction::Moved(moved, action.to_right()));
    }

    /// The items on the right, in item order.
    pub fn chosen(&self) -> Vec<usize> {
        self.sides.side(true)
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        self.chosen().iter().filter_map(|index| self.items.get(*index).cloned()).collect()
    }

    pub fn set_chosen(&mut self, cx: &mut Cx, indices: &[usize]) {
        self.fit();
        for (index, slot) in self.sides.across.iter_mut().enumerate() {
            *slot = indices.contains(&index);
        }
        self.picked.clear();
        self.draw_bg.redraw(cx);
    }

    pub fn set_items(&mut self, cx: &mut Cx, items: &[String]) {
        if self.items == items {
            return;
        }
        self.items = items.to_vec();
        // Everything that indexed the old list is dropped: an index into a
        // replaced list names a different item, not a missing one.
        self.sides = Sides::default();
        self.sides.fit(self.items.len());
        self.picked.clear();
        self.draw_bg.redraw(cx);
    }
}

impl Widget for Transfer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.fit();
        let search_h = if self.searchable { self.search_height } else { 0.0 };
        // A Fill inside a Fit resolves to nothing, so the height is worked
        // out here rather than left to the turtle.
        let natural_h = self.header_height
            + search_h
            + self.list_height
            + self.layout.padding.top
            + self.layout.padding.bottom;
        let walk = Walk {
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let frame = self.frame_of(rect);
        self.frame = frame;

        let title_font = self.draw_title.text_style.font_size as f64;
        let count_font = self.draw_count.text_style.font_size as f64;
        let row_font = self.draw_text.text_style.font_size as f64;
        let budget = self.row_budget();

        let (left_total, right_total) = self.sides.counts();
        for across in [false, true] {
            let list = if across { frame.right_list } else { frame.left_list };
            let title =
                if across { self.right_title.clone() } else { self.left_title.clone() };
            let total = if across { right_total } else { left_total };
            let rows = self.rows(across);

            let ty = text_y(title_font, frame.header_y, self.header_height);
            let cut = elide(&self.draw_title, cx, &title, list.size.x * 0.6);
            self.draw_title.draw_abs(cx, dvec2(list.pos.x, ty), &cut);

            // The count says both numbers while a search is narrowing the
            // side, and one when it is not: "4 of 30" and "30" answer
            // different questions and only one of them is being asked.
            let count = if rows.len() == total {
                format!("{total}")
            } else {
                format!("{} of {}", rows.len(), total)
            };
            let count_w = measure(&self.draw_count, cx, &count);
            self.draw_count.draw_abs(
                cx,
                dvec2(
                    list.pos.x + list.size.x - count_w,
                    text_y(count_font, frame.header_y, self.header_height),
                ),
                &count,
            );

            self.draw_panel.draw_abs(cx, list);

            for (row, index) in rows.iter().enumerate().take(budget) {
                let y = list.pos.y + row as f64 * self.row_height;
                let rect = Rect {
                    pos: dvec2(list.pos.x + 2.0, y),
                    size: dvec2((list.size.x - 4.0).max(1.0), self.row_height),
                };
                self.draw_row.hover =
                    if self.hover_row == Some((across, *index)) { 1.0 } else { 0.0 };
                self.draw_row.chosen = if self.picked.contains(index) { 1.0 } else { 0.0 };
                self.draw_row.keyed = 0.0;
                self.draw_row.draw_abs(cx, rect);

                let label = self.items.get(*index).cloned().unwrap_or_default();
                let cut =
                    elide(&self.draw_text, cx, &label, rect.size.x - self.pad_x * 2.0);
                self.draw_text.draw_abs(
                    cx,
                    dvec2(rect.pos.x + self.pad_x, text_y(row_font, y, self.row_height)),
                    &cut,
                );
            }
        }

        for (slot, action) in Move::ORDER.iter().enumerate() {
            let rect = frame.buttons[slot];
            let live = !self.payload(*action).is_empty();
            self.draw_button.hover =
                if live && self.hover_button == Some(slot) { 1.0 } else { 0.0 };
            self.draw_button.down =
                if live && self.down_button == Some(slot) { 1.0 } else { 0.0 };
            self.draw_button.draw_abs(cx, rect);

            let mark = action.mark();
            // Two text layers rather than one whose colour is written every
            // frame: a layer whose ink is overwritten has nowhere left to
            // keep the ink the call site handed it.
            let font = if live {
                self.draw_move.text_style.font_size as f64
            } else {
                self.draw_move_off.text_style.font_size as f64
            };
            let pen = if live { &mut self.draw_move } else { &mut self.draw_move_off };
            let w = measure(&*pen, cx, mark);
            pen.draw_abs(
                cx,
                dvec2(
                    rect.pos.x + (rect.size.x - w) * 0.5,
                    text_y(font, rect.pos.y, rect.size.y),
                ),
                mark,
            );
        }

        if self.searchable {
            // Drawn here rather than by a container, so nothing else puts
            // them in the tree and a host can reach ids!(transfer.left_search).
            cx.widget_tree_insert_child(self.uid, live_id!(left_search), self.left_search.clone());
            cx.widget_tree_insert_child(
                self.uid,
                live_id!(right_search),
                self.right_search.clone(),
            );
            let _ = self.left_search.draw_walk(
                cx,
                scope,
                Walk::fixed(frame.left_search.size.x, frame.left_search.size.y)
                    .with_abs_pos(frame.left_search.pos),
            );
            let _ = self.right_search.draw_walk(
                cx,
                scope,
                Walk::fixed(frame.right_search.size.x, frame.right_search.size.y)
                    .with_abs_pos(frame.right_search.pos),
            );
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.left_search.set_disabled(cx, disabled);
        self.right_search.set_disabled(cx, disabled);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.fit();
        if self.searchable {
            let actions = cx.capture_actions(|cx| {
                self.left_search.handle_event(cx, event, scope);
                self.right_search.handle_event(cx, event, scope);
            });
            for action in actions {
                // The lists are filtered from the boxes on the next draw, so
                // a change only has to ask for one.
                if let TextInputAction::Changed(_) = action.as_widget_action().cast() {
                    self.draw_bg.redraw(cx);
                }
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                // The three columns are worked out again from the rect the
                // pointer actually landed in. The rect the draw pass saw is
                // the widget's PRE-alignment one — a control in a centred
                // row has not been moved yet when it draws — so a hit test
                // against it answers for a place the reader never pressed.
                self.frame = self.frame_of(fe.rect);
                let row = self.row_at(fe.abs);
                let button =
                    self.frame.buttons.iter().position(|rect| rect.contains(fe.abs));
                if row != self.hover_row || button != self.hover_button {
                    self.hover_row = row;
                    self.hover_button = button;
                    cx.set_cursor(if row.is_some() || button.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover_row.take().is_some() || self.hover_button.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.frame = self.frame_of(fe.rect);
                if let Some(slot) = self.frame.buttons.iter().position(|r| r.contains(fe.abs)) {
                    self.down_button = Some(slot);
                    let action = Move::ORDER[slot];
                    self.run(cx, action);
                    return;
                }
                let Some((across, index)) = self.row_at(fe.abs) else {
                    return;
                };
                // A double press moves it: the hand is already on the row,
                // and the round trip out to the control column is the whole
                // reason a transfer feels slow.
                if fe.tap_count == 2 {
                    self.picked.insert(index);
                    let action = if across { Move::OneLeft } else { Move::OneRight };
                    self.run(cx, action);
                    return;
                }
                // Plain click toggles: this is a set being built, and a
                // click that cleared the rest would make choosing five
                // things five separate journeys.
                if !self.picked.remove(&index) {
                    self.picked.insert(index);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(_) => {
                if self.down_button.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            _ => {}
        }
    }

    /// What is on the right, so a test can read the control in one line.
    fn text(&self) -> String {
        self.chosen_labels().join(", ")
    }
}

impl TransferRef {
    /// The items on the right, as indices into `items`.
    pub fn chosen(&self) -> Vec<usize> {
        self.borrow().map(|inner| inner.chosen()).unwrap_or_default()
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.chosen_labels()).unwrap_or_default()
    }

    pub fn set_chosen(&self, cx: &mut Cx, indices: &[usize]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_chosen(cx, indices);
        }
    }

    pub fn set_items(&self, cx: &mut Cx, items: &[String]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_items(cx, items);
        }
    }

    /// How many moved this pass, and which way.
    pub fn moved(&self, actions: &Actions) -> Option<(usize, bool)> {
        match actions.find_widget_action(self.widget_uid())?.cast::<TransferAction>() {
            TransferAction::Moved(count, to_right) => Some((count, to_right)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forest(lines: &[&str]) -> Forest {
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        Forest::from_outline(&lines)
    }

    /// Four levels: continent, country, region, city.
    fn places() -> Forest {
        forest(&[
            "Europe",
            "  France",
            "    Brittany",
            "      Rennes",
            "      Brest",
            "    Alsace",
            "      Colmar",
            "  Spain",
            "    Galicia",
            "      Vigo",
            "Asia",
            "  Japan",
            "    Kansai",
            "      Kobe",
        ])
    }

    fn walk(forest: &Forest, labels: &[&str]) -> Trail {
        let mut trail = Trail::default();
        for label in labels {
            let node = forest
                .nodes
                .iter()
                .position(|node| node.label == *label)
                .expect("no such node");
            let level = trail.depth();
            trail.choose(level, node);
        }
        trail
    }

    #[test]
    fn a_choice_at_a_level_throws_away_everything_to_its_right() {
        let forest = places();
        let mut trail = walk(&forest, &["Europe", "France", "Brittany", "Rennes"]);
        assert_eq!(trail.depth(), 4);

        // A fresh choice in the SECOND column: the two columns beyond it
        // described a Brittany nobody is standing in any more.
        let spain = forest.nodes.iter().position(|node| node.label == "Spain").unwrap();
        trail.choose(1, spain);
        assert_eq!(trail.depth(), 2, "one level chosen behind it, and this one");
        assert_eq!(forest.label(trail.at(1).unwrap()), "Spain");
        assert_eq!(trail.at(2), None);

        // And in the FIRST column of four, one level is all that is left.
        let mut trail = walk(&forest, &["Europe", "France", "Brittany", "Rennes"]);
        let asia = forest.nodes.iter().position(|node| node.label == "Asia").unwrap();
        trail.choose(0, asia);
        assert_eq!(trail.depth(), 1);
        assert_eq!(forest.label(trail.at(0).unwrap()), "Asia");
    }

    #[test]
    fn choosing_the_same_row_again_leaves_that_level_alone_and_clears_the_rest() {
        let forest = places();
        let mut trail = walk(&forest, &["Europe", "France", "Brittany"]);
        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        trail.choose(1, france);
        assert_eq!(trail.depth(), 2);
        assert_eq!(forest.label(trail.at(1).unwrap()), "France");
    }

    #[test]
    fn a_column_opens_for_every_chosen_branch_and_none_for_a_leaf() {
        let forest = places();
        let trail = Trail::default();
        assert_eq!(columns(&forest, &trail).len(), 1, "the roots, and nothing beyond them");

        let trail = walk(&forest, &["Europe"]);
        assert_eq!(columns(&forest, &trail).len(), 2);

        let trail = walk(&forest, &["Europe", "France", "Brittany", "Rennes"]);
        // Rennes is a leaf: it opens nothing, so there is no empty fifth
        // column hanging off the end for the reader to press at.
        assert_eq!(columns(&forest, &trail).len(), 4);
    }

    #[test]
    fn the_columns_hold_the_children_of_what_is_chosen_beside_them() {
        let forest = places();
        let trail = walk(&forest, &["Europe", "France"]);
        let cols = columns(&forest, &trail);
        let names: Vec<&str> = cols[2].iter().map(|node| forest.label(*node)).collect();
        assert_eq!(names, vec!["Brittany", "Alsace"]);
    }

    #[test]
    fn an_outline_indent_that_jumps_is_clamped_rather_than_refused() {
        // A hand-written outline with a stray extra indent should still draw
        // the shape its author meant, not lose the line.
        let forest = forest(&["Top", "      Deep"]);
        assert_eq!(forest.nodes.len(), 2);
        assert_eq!(forest.nodes[1].depth, 1);
        assert_eq!(forest.children(0), [1usize].as_slice());
    }

    #[test]
    fn ticking_a_branch_reaches_every_leaf_under_it() {
        let forest = places();
        let mut ticked = HashSet::new();
        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        cascade(&forest, &mut ticked, france, true);

        let names: Vec<&str> =
            ticked_leaves(&forest, &ticked).iter().map(|node| forest.label(*node)).collect();
        assert_eq!(names, vec!["Rennes", "Brest", "Colmar"]);
        // Only leaves are stored, so the branch cannot end up disagreeing
        // with its own children.
        assert_eq!(ticked.len(), 3);
        assert_eq!(tick_of(&forest, &ticked, france), Tick::On);
    }

    #[test]
    fn a_branch_whose_leaves_disagree_is_mixed_all_the_way_up() {
        let forest = places();
        let mut ticked = HashSet::new();
        let rennes = forest.nodes.iter().position(|node| node.label == "Rennes").unwrap();
        cascade(&forest, &mut ticked, rennes, true);

        let brittany = forest.nodes.iter().position(|node| node.label == "Brittany").unwrap();
        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        let europe = forest.nodes.iter().position(|node| node.label == "Europe").unwrap();
        let asia = forest.nodes.iter().position(|node| node.label == "Asia").unwrap();
        assert_eq!(tick_of(&forest, &ticked, brittany), Tick::Mixed);
        assert_eq!(tick_of(&forest, &ticked, france), Tick::Mixed);
        assert_eq!(tick_of(&forest, &ticked, europe), Tick::Mixed);
        assert_eq!(tick_of(&forest, &ticked, asia), Tick::Off, "a branch nobody touched");
    }

    #[test]
    fn unticking_a_branch_reaches_the_same_leaves_the_tick_did() {
        let forest = places();
        let mut ticked = HashSet::new();
        let europe = forest.nodes.iter().position(|node| node.label == "Europe").unwrap();
        cascade(&forest, &mut ticked, europe, true);
        assert_eq!(ticked.len(), 4);

        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        cascade(&forest, &mut ticked, france, false);
        let names: Vec<&str> =
            ticked_leaves(&forest, &ticked).iter().map(|node| forest.label(*node)).collect();
        assert_eq!(names, vec!["Vigo"], "Spain's leaf was not in the branch that was cleared");
    }

    fn stock() -> Vec<String> {
        ["Kick", "Snare", "Hat", "Clap", "Rim", "Crash"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn moving_across_keeps_the_two_counts_adding_up() {
        let items = stock();
        let mut sides = Sides::default();
        sides.fit(items.len());
        assert_eq!(sides.counts(), (6, 0));

        sides.move_across(&[1, 3], true);
        assert_eq!(sides.counts(), (4, 2));
        assert_eq!(sides.side(true), vec![1, 3]);
        assert_eq!(sides.side(false), vec![0, 2, 4, 5]);

        sides.move_across(&[3], false);
        assert_eq!(sides.counts(), (5, 1));
    }

    #[test]
    fn moving_something_that_is_already_across_moves_nothing() {
        let mut sides = Sides::default();
        sides.fit(stock().len());
        assert_eq!(sides.move_across(&[0, 1], true), 2);
        // The overlap is not counted a second time; only the one that had
        // not crossed yet reports as moved.
        assert_eq!(sides.move_across(&[1, 2], true), 1);
        assert_eq!(sides.counts(), (3, 3));
    }

    #[test]
    fn a_row_that_came_back_stands_where_it_did_before_it_left() {
        // Item order, never move order: two readers who moved the same set
        // in a different sequence must be looking at the same list.
        let mut sides = Sides::default();
        sides.fit(stock().len());
        sides.move_across(&[4, 0, 2], true);
        assert_eq!(sides.side(true), vec![0, 2, 4]);
        sides.move_across(&[0], false);
        assert_eq!(sides.side(false), vec![0, 1, 3, 5]);
    }

    #[test]
    fn a_move_all_takes_what_the_search_is_showing_and_nothing_else() {
        // The one way this control can silently lose somebody's work: a
        // move-all that reaches past the search takes rows the reader cannot
        // see and did not ask for.
        let items = stock();
        let mut sides = Sides::default();
        sides.fit(items.len());

        let left = sides.side(false);
        let shown = showing(&items, &left, "a");
        let names: Vec<&str> = shown.iter().map(|i| items[*i].as_str()).collect();
        assert_eq!(names, vec!["Snare", "Hat", "Clap", "Crash"]);

        sides.move_across(&shown, true);
        assert_eq!(sides.counts(), (2, 4));
        let stayed: Vec<&str> =
            sides.side(false).iter().map(|i| items[*i].as_str()).collect();
        assert_eq!(stayed, vec!["Kick", "Rim"], "the rows the search had hidden stayed put");
    }

    #[test]
    fn an_empty_search_shows_the_whole_side() {
        let items = stock();
        let left: Vec<usize> = (0..items.len()).collect();
        assert_eq!(showing(&items, &left, "").len(), 6);
        assert_eq!(showing(&items, &left, "   ").len(), 6);
    }

    #[test]
    fn the_search_does_not_care_about_case() {
        let items = stock();
        let left: Vec<usize> = (0..items.len()).collect();
        assert_eq!(showing(&items, &left, "KICK"), vec![0]);
        assert_eq!(showing(&items, &left, "sNaRe"), vec![1]);
    }

    #[test]
    fn a_search_that_matches_nothing_leaves_a_move_all_with_nothing_to_do() {
        let items = stock();
        let mut sides = Sides::default();
        sides.fit(items.len());
        let shown = showing(&items, &sides.side(false), "tuba");
        assert!(shown.is_empty());
        assert_eq!(sides.move_across(&shown, true), 0);
        assert_eq!(sides.counts(), (6, 0));
    }

    #[test]
    fn an_outline_line_carries_its_own_id_so_a_host_can_find_it_again() {
        // Ids are line numbers counting from one, blank lines included, so
        // an action read back names the line the host actually wrote.
        let forest = forest(&["Europe", "", "  France"]);
        assert_eq!(forest.nodes.len(), 2);
        assert_eq!(forest.id(0), Some(LiveId(1)));
        assert_eq!(forest.id(1), Some(LiveId(3)));
        assert_eq!(forest.index_of(LiveId(3)), Some(1));
    }
}
