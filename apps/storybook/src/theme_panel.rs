//! The theme panel: every token the running theme defines, editable in place.
//!
//! The other panels describe one story. This one describes what all of them
//! are drawn from. It lists the running theme's colours and numbers by group
//! — colour, shape, elevation, motion, spacing, size, type, state — and gives
//! each token a row: a preview of what the value means, the token's name, and
//! the value itself in a field that can be typed over. Committing an edit
//! goes through the library's own theme write, the same one the design
//! overlay's sidebar uses, so a colour is retargeted in every draw buffer
//! holding it and the whole catalogue repaints under it.
//!
//! The rows come from the reflection surface, the same read the foundations
//! pages make, so the two cannot disagree about what the theme contains.
//!
//! # What it deliberately does not do
//!
//! * It does not write an edit back to the theme source. The edit is
//!   ledgered in the overlay's session, which is where a diff of the theme
//!   already lives; this panel is for trying a value, not for landing it.
//! * It does not list the tokens that are objects rather than values — text
//!   styles, easings and insets. There is no single value to type into a
//!   field, and the foundations pages show those as what they are.
//! * It does not say where a token is defined. The foundations tables have
//!   the width for that column and a side panel does not.
//! * It does not commit an edit when the field loses focus. Return commits;
//!   clicking away is how a half-typed value is abandoned.
use crate::makepad_widgets::reflect::{theme_set_value, theme_values, ThemeVal};
use crate::makepad_widgets::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ThemeRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_1
        align: Align{x: 0. y: 0.5}
        padding: Inset{top: 1. bottom: 1. left: 0. right: 0.}
    }

    mod.storybook.ThemePanelBase = #(ThemePanel::register_widget(vm))
    /** The running theme's tokens, grouped, previewed and editable; Return commits a value. */
    mod.storybook.ThemePanel = set_type_default() do mod.storybook.ThemePanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2

        head := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_1
            align: Align{x: 0. y: 0.5}
            // The one label is a placeholder: the panel sets the real list
            // from its own group table, so there is one place to add a
            // group. A drop down with no labels at all has nothing to draw.
            group := DropDown{
                width: 92.
                labels: ["All"]
                selected_item: 0
            }
            search := TextInput{
                width: Fill
                empty_text: "Filter tokens"
            }
        }

        status_row := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_1
            align: Align{x: 0. y: 0.5}
            status := Label{width: Fill text: ""}
            revert := ButtonFlat{text: "Revert"}
        }

        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            RowColor := ThemeRow{
                // The outline is what makes a surface colour visible: with
                // no border, half the ladder is a swatch the same shade as
                // the panel it sits on.
                swatch := RoundedView{
                    width: 56.
                    height: 20.
                    show_bg: true
                    draw_bg +: {
                        color: #x00000000
                        border_size: 1.
                        border_color: theme.color_outline
                        border_radius: theme.radius_xs
                    }
                }
                name := Label{width: Fill text: ""}
                value := TextInput{width: 96.}
            }
            RowNum := ThemeRow{
                // A fixed track so the name column starts at the same x
                // whatever the bar inside it measures.
                track := View{
                    width: 56.
                    height: 20.
                    flow: Right
                    align: Align{x: 0. y: 0.5}
                    bar := RoundedView{
                        width: 2.
                        height: 8.
                        show_bg: true
                        draw_bg +: {
                            color: theme.color_text
                            border_radius: theme.radius_xs
                        }
                    }
                }
                name := Label{width: Fill text: ""}
                value := TextInput{width: 96.}
            }
        }
    }
}

/// The groups, in the order they are offered. `All` is first because the
/// filter is most useful across the whole theme; the rest follow the order
/// the foundations pages are in, with the tokens no prefix claims at the end
/// so nothing in the theme is unreachable.
const GROUPS: &[&str] = &[
    "All",
    "Colour",
    "Type",
    "Spacing",
    "Size",
    "Shape",
    "Elevation",
    "State",
    "Motion",
    "Other",
];

const G_ALL: usize = 0;
const G_COLOUR: usize = 1;
const G_TYPE: usize = 2;
const G_SPACING: usize = 3;
const G_SIZE: usize = 4;
const G_SHAPE: usize = 5;
const G_ELEVATION: usize = 6;
const G_STATE: usize = 7;
const G_MOTION: usize = 8;
const G_OTHER: usize = 9;

/// How long the preview column is, and so how long a full bar is.
const TRACK: f64 = 56.0;

/// Which group a token belongs to, by the family its name starts with. The
/// first match wins, so the longer names come before the prefixes that would
/// swallow them.
fn group_of(name: &str) -> usize {
    const PREFIXES: &[(&str, usize)] = &[
        ("color", G_COLOUR),
        ("radius_", G_SHAPE),
        ("container_corner_radius", G_SHAPE),
        ("textselection_corner_radius", G_SHAPE),
        ("corner_radius", G_SHAPE),
        ("beveling", G_SHAPE),
        ("elevation_", G_ELEVATION),
        ("motion_", G_MOTION),
        ("space_", G_SPACING),
        ("mspace_", G_SPACING),
        ("state_", G_STATE),
        ("size_", G_SIZE),
        ("splitter_", G_SIZE),
        ("dock_", G_SIZE),
        ("tab_", G_SIZE),
        ("data_", G_SIZE),
        ("type_", G_TYPE),
        ("font_", G_TYPE),
    ];
    PREFIXES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, group)| *group)
        .unwrap_or(G_OTHER)
}

/// What the left column shows for a token.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Preview {
    /// A filled swatch of the colour itself.
    Color(u32),
    /// A bar as long as the token, in points.
    Length(f64),
    /// A bar as long as the duration, stretched so a tenth of a second is
    /// something the eye can compare.
    Time(f64),
    /// A bar that is that fraction of the track.
    Opacity(f64),
}

fn preview_of(name: &str, value: ThemeVal) -> Preview {
    match value {
        ThemeVal::Color(c) => Preview::Color(c),
        // A state token is already a fraction, so its bar is a share of the
        // track; a duration in seconds would be a hairline drawn at its own
        // number. Everything else is a length in points and is drawn at that
        // length, which is the only reading that lets two of them be
        // compared by eye rather than by reading the numbers.
        ThemeVal::Num(v) if name.starts_with("state_") => Preview::Opacity(v),
        ThemeVal::Num(v) if name.starts_with("motion_") => Preview::Time(v),
        ThemeVal::Num(v) => Preview::Length(v),
    }
}

/// How wide to draw the bar. Never zero: a token set to nothing still has a
/// row, and a row with no mark in it reads as a row that failed to load.
fn bar_px(preview: Preview) -> f64 {
    let len = match preview {
        Preview::Color(_) => return 0.0,
        Preview::Opacity(v) => v * TRACK,
        Preview::Time(v) => v * 60.0,
        Preview::Length(v) => v,
    };
    if !len.is_finite() {
        return 1.0;
    }
    len.clamp(1.0, TRACK)
}

/// The value as the theme write reads it back: a colour in the eight-digit
/// form the ledger uses, a number as plainly as it prints.
fn value_text(value: ThemeVal) -> String {
    match value {
        ThemeVal::Color(c) => format!("#{c:08x}"),
        ThemeVal::Num(v) => format!("{v}"),
    }
}

/// `needle` is already trimmed and lower case; an empty one matches all.
fn name_matches(name: &str, needle: &str) -> bool {
    needle.is_empty() || name.to_lowercase().contains(needle)
}

/// One row: a token of the running theme.
struct Token {
    name: String,
    /// The value as it reads now, which is also what the field shows.
    value: String,
    group: usize,
    preview: Preview,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ThemePanel {
    #[deref]
    view: View,
    #[rust]
    tokens: Vec<Token>,
    /// Indices into `tokens` that the group and the filter let through.
    #[rust]
    visible: Vec<usize>,
    /// What each token read as when the panel first saw it, so an edit can
    /// be taken back. Only ever filled in for a name it does not hold yet:
    /// a read after an edit must not adopt the edit as the value to return
    /// to.
    #[rust]
    originals: HashMap<String, String>,
    #[rust]
    group: usize,
    #[rust]
    filter: String,
    /// What the last commit or revert said, shown instead of the counts
    /// until the reader moves the group or the filter.
    #[rust]
    note: String,
    #[rust(true)]
    needs_read: bool,
    /// The header carries state the panel owns, not the DSL: after a reload
    /// rebuilt it from the template it has to be told again.
    #[rust(true)]
    needs_restore: bool,
}

impl ThemePanel {
    /// Read every colour and number in the running theme.
    fn read(&mut self, cx: &mut Cx) {
        let mut tokens: Vec<Token> = Vec::new();
        for (name, _key, value, _at) in theme_values(cx) {
            let text = value_text(value);
            self.originals.entry(name.clone()).or_insert_with(|| text.clone());
            tokens.push(Token {
                group: group_of(&name),
                preview: preview_of(&name, value),
                name,
                value: text,
            });
        }
        // Grouped, then by name, so a family reads base, container, and the
        // ladders count up — the order the theme files declare them in is
        // not the order they are read in.
        tokens.sort_by(|a, b| a.group.cmp(&b.group).then_with(|| a.name.cmp(&b.name)));
        self.tokens = tokens;
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Cx) {
        let needle = self.filter.trim().to_lowercase();
        let group = self.group;
        self.visible = self
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| (group == G_ALL || t.group == group) && name_matches(&t.name, &needle))
            .map(|(i, _)| i)
            .collect();
        self.view.redraw(cx);
    }

    fn is_edited(&self, token: &Token) -> bool {
        self.originals.get(&token.name).is_some_and(|o| *o != token.value)
    }

    fn status_text(&self) -> String {
        if !self.note.is_empty() {
            return self.note.clone();
        }
        let edited = self.tokens.iter().filter(|t| self.is_edited(t)).count();
        format!("{} of {} · {} edited", self.visible.len(), self.tokens.len(), edited)
    }

    /// Put the group and the filter back into the header, and give the drop
    /// down the real list of groups. Answers whether it landed: on the very
    /// first draw the panel's own children are not in the widget index yet,
    /// and a write into an empty reference is silently thrown away.
    fn restore(&mut self, cx: &mut Cx) -> bool {
        let chooser = self.view.drop_down(cx, ids!(group));
        if chooser.borrow().is_none() {
            return false;
        }
        chooser.set_labels(cx, GROUPS.iter().map(|g| g.to_string()).collect());
        chooser.set_selected_item(cx, self.group);
        let search = self.view.widget(cx, ids!(search));
        if search.text() != self.filter {
            search.set_text(cx, &self.filter);
        }
        true
    }

    /// The status line is written only when it would change, so a panel
    /// sitting still does not ask for a frame it has nothing new to put in.
    fn sync_status(&mut self, cx: &mut Cx) {
        let text = self.status_text();
        let status = self.view.label(cx, ids!(status));
        if status.text() != text {
            status.set_text(cx, &text);
        }
    }

    /// Write one token, from the field in row `index` of what is on screen.
    fn commit(&mut self, cx: &mut Cx, index: usize, text: &str) {
        let Some(name) = self
            .visible
            .get(index)
            .and_then(|i| self.tokens.get(*i))
            .map(|t| t.name.clone())
        else {
            return;
        };
        let text = text.trim().to_string();
        match theme_set_value(cx, &name, &text) {
            Ok(()) => {
                self.note = format!("{name} = {text}");
                // Read again rather than storing what was typed: the write
                // normalises a colour, and the row should show the value the
                // theme now holds, not the shorthand that reached it.
                self.needs_read = true;
            }
            Err(e) => self.note = e,
        }
        self.view.redraw(cx);
    }

    /// Put every edited token back to what it read as when the panel opened.
    fn revert_all(&mut self, cx: &mut Cx) {
        let edits: Vec<(String, String)> = self
            .tokens
            .iter()
            .filter(|t| self.is_edited(t))
            .filter_map(|t| self.originals.get(&t.name).map(|o| (t.name.clone(), o.clone())))
            .collect();
        if edits.is_empty() {
            self.note = "Nothing to revert.".to_string();
            self.view.redraw(cx);
            return;
        }
        let asked = edits.len();
        let mut done = 0;
        for (name, text) in edits {
            if theme_set_value(cx, &name, &text).is_ok() {
                done += 1;
            }
        }
        self.note = if done == asked {
            format!("Reverted {asked}.")
        } else {
            format!("Reverted {done} of {asked}.")
        };
        self.needs_read = true;
        self.view.redraw(cx);
    }

    fn template_for(preview: Preview) -> LiveId {
        match preview {
            Preview::Color(_) => live_id!(RowColor),
            _ => live_id!(RowNum),
        }
    }

    fn fill_row(&self, cx: &mut Cx, item: &WidgetRef, token: &Token) {
        let mark = if self.is_edited(token) { " \u{25CF}" } else { "" };
        item.label(cx, ids!(name)).set_text(cx, &format!("{}{}", token.name, mark));
        match token.preview {
            Preview::Color(c) => {
                let mut swatch = item.widget(cx, ids!(swatch));
                let color = color_vec(c);
                script_apply_eval!(cx, swatch, {
                    draw_bg +: {color: #(color)}
                });
            }
            other => {
                let mut bar = item.widget(cx, ids!(track.bar));
                let width = bar_px(other);
                script_apply_eval!(cx, bar, { width: #(width) });
            }
        }
        // A field being typed into is left alone: it holds a value that is
        // not the theme's yet, and that is the whole point of it.
        let field = item.widget(cx, ids!(value));
        if !field.key_focus(cx) && field.text() != token.value {
            field.set_text(cx, &token.value);
        }
    }
}

fn color_vec(c: u32) -> Vec4f {
    Vec4f {
        x: ((c >> 24) & 0xFF) as f32 / 255.0,
        y: ((c >> 16) & 0xFF) as f32 / 255.0,
        z: ((c >> 8) & 0xFF) as f32 / 255.0,
        w: (c & 0xFF) as f32 / 255.0,
    }
}

impl Widget for ThemePanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.needs_restore && self.restore(cx) {
            self.needs_restore = false;
        }
        if self.needs_read {
            self.needs_read = false;
            self.read(cx);
        }
        self.sync_status(cx);
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.visible.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    let Some(token) = self.visible.get(item_id).and_then(|i| self.tokens.get(*i))
                    else {
                        continue;
                    };
                    let item = list.item(cx, item_id, Self::template_for(token.preview));
                    self.fill_row(cx, &item, token);
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            // A theme switch arrives as a reload, and so does an edit to any
            // source file. Either way the tokens have to be read again, and
            // the baseline goes with them: after a reload the theme reads as
            // whatever the new files say, and holding the old numbers as
            // "the original" would make Revert write a foreign theme's
            // values into this one.
            self.tokens.clear();
            self.visible.clear();
            self.originals.clear();
            self.note = String::new();
            self.needs_read = true;
            self.needs_restore = true;
        }
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        if let Some(index) = self.view.drop_down(cx, ids!(group)).selected(actions) {
            self.group = index;
            self.note = String::new();
            self.refilter(cx);
        }
        if let Some(text) = self.view.text_input(cx, ids!(search)).changed(actions) {
            self.filter = text;
            self.note = String::new();
            self.refilter(cx);
        }
        if self.view.button(cx, ids!(revert)).clicked(actions) {
            self.revert_all(cx);
        }
        let list = self.view.portal_list(cx, ids!(list));
        let mut commit: Option<(usize, String)> = None;
        for (item_id, item) in list.items_with_actions(actions) {
            let field = item.text_input(cx, ids!(value));
            if let Some((text, _)) = field.returned(actions) {
                commit = Some((item_id, text));
            }
        }
        if let Some((index, text)) = commit {
            self.commit(cx, index, &text);
        }
    }
}

impl ThemePanelRef {
    /// Read the theme again on the next draw. For a host that changed a
    /// token by some other route and wants the panel to agree with it.
    pub fn reread(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.needs_read = true;
            inner.view.redraw(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_lands_in_its_own_group() {
        assert_eq!(group_of("color_primary"), G_COLOUR);
        assert_eq!(group_of("color_elevation_3"), G_COLOUR, "a colour is a colour first");
        assert_eq!(group_of("radius_full"), G_SHAPE);
        assert_eq!(group_of("corner_radius"), G_SHAPE);
        assert_eq!(group_of("container_corner_radius"), G_SHAPE);
        assert_eq!(group_of("beveling"), G_SHAPE);
        assert_eq!(group_of("elevation_2_radius"), G_ELEVATION);
        assert_eq!(group_of("motion_long_4"), G_MOTION);
        assert_eq!(group_of("space_factor"), G_SPACING);
        assert_eq!(group_of("size_touch_target"), G_SIZE);
        assert_eq!(group_of("splitter_size"), G_SIZE);
        assert_eq!(group_of("tab_flat_height"), G_SIZE);
        assert_eq!(group_of("type_body_m_size"), G_TYPE);
        assert_eq!(group_of("font_size_base"), G_TYPE, "the type scale is built on it");
        assert_eq!(group_of("state_hover_opacity"), G_STATE);
    }

    #[test]
    fn a_token_no_prefix_claims_is_still_reachable() {
        // Otherwise a token added to the theme would be listed nowhere and
        // the panel would quietly stop being the whole theme.
        assert_eq!(group_of("whatever_comes_next"), G_OTHER);
        assert!(G_OTHER < GROUPS.len());
    }

    #[test]
    fn a_group_index_always_names_a_group() {
        for name in ["color_a", "radius_s", "beveling", "nothing_like_it"] {
            assert!(group_of(name) < GROUPS.len());
        }
    }

    #[test]
    fn the_groups_between_all_and_other_are_the_foundations_pages_in_order() {
        let mut pages: Vec<&str> = Vec::new();
        for story in crate::registry::all().filter(|story| story.category == "Foundations") {
            if !pages.contains(&story.component) {
                pages.push(story.component);
            }
        }
        assert_eq!(&GROUPS[1..GROUPS.len() - 1], pages.as_slice());
    }

    #[test]
    fn the_preview_says_what_kind_of_number_it_is() {
        assert_eq!(preview_of("color_primary", ThemeVal::Color(0xff5c39ff)), Preview::Color(0xff5c39ff));
        assert_eq!(preview_of("state_hover_opacity", ThemeVal::Num(0.08)), Preview::Opacity(0.08));
        assert_eq!(preview_of("motion_short_1", ThemeVal::Num(0.05)), Preview::Time(0.05));
        assert_eq!(preview_of("space_3", ThemeVal::Num(12.0)), Preview::Length(12.0));
    }

    #[test]
    fn a_bar_is_never_nothing_and_never_longer_than_the_track() {
        assert_eq!(bar_px(Preview::Length(0.0)), 1.0, "a token set to nothing still has a row");
        assert_eq!(bar_px(Preview::Length(12.0)), 12.0);
        assert_eq!(bar_px(Preview::Length(999.0)), TRACK, "a pill radius is not a mile of bar");
        assert_eq!(bar_px(Preview::Opacity(1.0)), TRACK);
        assert_eq!(bar_px(Preview::Opacity(0.5)), TRACK * 0.5);
        assert_eq!(bar_px(Preview::Time(0.5)), 30.0);
        assert_eq!(bar_px(Preview::Time(f64::NAN)), 1.0, "a width of NaN would lay out nothing");
    }

    #[test]
    fn a_value_reads_back_the_way_the_write_takes_it() {
        // The field's text goes straight back to the theme write, so what is
        // shown has to be something it accepts.
        assert_eq!(value_text(ThemeVal::Color(0xff5c39ff)), "#ff5c39ff");
        assert_eq!(value_text(ThemeVal::Color(0x00000080)), "#00000080");
        assert_eq!(value_text(ThemeVal::Num(2.0)), "2");
        assert_eq!(value_text(ThemeVal::Num(0.125)), "0.125");
    }

    #[test]
    fn the_filter_ignores_case_and_matches_anywhere() {
        assert!(name_matches("color_primary_container", "primary"), "and in the middle of a name");
        assert!(name_matches("Color_PRIMARY", "primary"), "the name is lowered, not just the needle");
        assert!(name_matches("color_primary_container", ""), "an empty filter hides nothing");
        assert!(!name_matches("color_primary_container", "secondary"));
    }
}
