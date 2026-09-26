//! Columns side by side, one per level.
//!
//! Choosing in a column fills the one to its right, and choosing again at
//! any level throws away everything to the right of it. For a deep, narrow
//! hierarchy where the reader is following a path: a category, then a
//! subcategory, then the thing.
//!
//! One of three ways to choose out of a set that will not fit in one list;
//! `tree_select` and `transfer` are the other two. `picker_parts` holds what
//! they share, and says what all three deliberately leave out.

use crate::{
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    picker_parts::{elide, text_y, DrawPickerPanel, DrawPickerRow, Forest},
    widget::*,
};

/// The mark on a row that has a column behind it. Checked against the
/// default face first, as every glyph the pickers name was: most of the
/// pictorial ranges render as an empty box here, and nothing says when one
/// has.
const BRANCH_MARK: &str = "\u{203A}";

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ColumnPickerBase = #(ColumnPicker::register_widget(vm))

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
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker_parts::fixtures::places;

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
}
