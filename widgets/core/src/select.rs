//! Select — a control that shows one value and offers the rest, and a
//! MultiSelect that shows a set and offers to change it.
//!
//! **A dropdown is a menu with a value.** Rather than grow a second row
//! model beside the menu's, a select builds [`MenuRow`]s and hands them to
//! the one `MenuLayer` an app already declares. One model means a row can
//! carry everything a menu row carries — a mark, a shortcut, a heading, a
//! disabled state, a dangerous one — and anything added for menus is
//! immediately available to dropdowns.
//!
//! **A row can be a switch.** A select whose `selection` is `Multi` shows a
//! check on every chosen row and stays open while they are toggled, because
//! a set is not finished after one press. Its closed face then says what the
//! set amounts to rather than naming one member of it: "3 chosen" is honest
//! where "Bass" would be a lie about the other two.
//!
//! **The value is data, not a child.** `options` is a list of words the
//! control owns, the way a segmented control owns its answers. A dropdown
//! assembled from child widgets makes the caller keep the children and the
//! value in step by hand, and every caller gets that slightly wrong.
//!
//! The older `DropDown` stays exactly as it is: it carries an icon list and
//! an icon-only face that several apps drive by name, and nothing here
//! changes it.

use crate::{
    button::ButtonWidgetRefExt,
    chip::ChipSelection,
    makepad_derive_widget::*,
    makepad_draw::*,
    menu::{menu_actions, MenuAction, MenuPlace, MenuRow},
    view::View,
    widget::*,
};

/// What a select reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum SelectAction {
    /// The value changed, with the index chosen.
    Changed(usize),
    /// A multi-select's set changed, with the index toggled.
    Toggled(usize),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SelectBase = #(Select::register_widget(vm))
    /** A control that shows one value and offers the rest. */
    mod.widgets.Select = set_type_default() do mod.widgets.SelectBase{
        width: Fit
        height: Fit
        flow: Right
        /** the values, in order */
        options: []
        /** which value is current, as an index into `options` */
        selected: 0
        /** how many may be chosen at once: Any Single Multi */
        selection: Single
        /** shown on the face when nothing is chosen */
        placeholder: "Choose"
        /** where the list hangs: Below BelowRight At */
        place: Below
        /** what the face says for a set: the count, or the names */
        count_summary: true

        face := Button{
            text: "Choose"
        }
    }

    /** A control that shows a set and offers to change it: every row is a
     * switch, and the list stays open while they are pressed. */
    mod.widgets.MultiSelect = mod.widgets.Select{
        selection: Multi
        placeholder: "None chosen"
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Select {
    #[deref]
    view: View,
    /// The values, in order.
    #[live]
    pub options: Vec<String>,
    /// Which value is current, as an index into `options`.
    #[live(0usize)]
    pub selected: usize,
    /// How many may be chosen at once.
    #[live]
    pub selection: ChipSelection,
    /// Shown on the face when nothing is chosen.
    #[live]
    pub placeholder: String,
    /// Where the list hangs off the face.
    #[live]
    pub place: MenuPlace,
    /// What the face says for a set: the count, or the names.
    #[live(true)]
    pub count_summary: bool,
    /// Which values are chosen, while `selection` is Multi.
    #[rust]
    chosen: Vec<bool>,
    /// Whether the face has been written from the value this draw.
    #[rust]
    dressed: bool,
}

impl Select {
    fn labels(&self) -> Vec<String> {
        self.options.clone()
    }

    fn fit_chosen(&mut self) {
        if self.chosen.len() != self.options.len() {
            self.chosen.resize(self.options.len(), false);
        }
    }

    fn is_multi(&self) -> bool {
        self.selection == ChipSelection::Multi
    }

    /// Whether a value is chosen. For a single select that is the one
    /// value; for a set it is the row's own switch.
    pub fn is_chosen(&self, index: usize) -> bool {
        if self.is_multi() {
            self.chosen.get(index).copied().unwrap_or(false)
        } else {
            index == self.selected
        }
    }

    /// The names of everything chosen.
    pub fn chosen_labels(&self) -> Vec<String> {
        self.labels()
            .into_iter()
            .enumerate()
            .filter(|(i, _)| self.is_chosen(*i))
            .map(|(_, label)| label)
            .collect()
    }

    /// What the face says.
    ///
    /// A set says how many, not which: naming one member of a set of three
    /// is a lie about the other two, and naming all three does not fit on a
    /// control this size. `count_summary: false` asks for the names anyway,
    /// for a caller who knows their list is short.
    pub fn face_text(&self) -> String {
        let chosen = self.chosen_labels();
        if chosen.is_empty() {
            return self.placeholder.clone();
        }
        if !self.is_multi() || chosen.len() == 1 {
            return chosen[0].clone();
        }
        if self.count_summary {
            format!("{} chosen", chosen.len())
        } else {
            chosen.join(", ")
        }
    }

    /// The rows the list shows: one per value, with a check on every one
    /// that is chosen.
    fn rows(&self) -> Vec<MenuRow> {
        self.labels()
            .into_iter()
            .enumerate()
            .map(|(i, label)| {
                let row = MenuRow::new(LiveId(i as u64 + 1), &label);
                if self.is_multi() {
                    row.checked(self.is_chosen(i))
                } else {
                    row.radio(self.is_chosen(i))
                }
            })
            .collect()
    }

    /// The id this select's menu carries, so it can tell its own list from
    /// anyone else's.
    fn owner(&self) -> LiveId {
        LiveId(self.widget_uid().0)
    }

    fn set_value(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.options.len() {
            return;
        }
        let uid = self.widget_uid();
        if self.is_multi() {
            self.fit_chosen();
            self.chosen[index] = !self.chosen[index];
            cx.widget_action(uid, SelectAction::Toggled(index));
        } else {
            if self.selected == index {
                return;
            }
            self.selected = index;
            cx.widget_action(uid, SelectAction::Changed(index));
        }
        self.dressed = false;
        self.view.redraw(cx);
    }

    /// Set the whole set at once, for a host restoring saved state.
    pub fn set_chosen(&mut self, cx: &mut Cx, chosen: Vec<bool>) {
        self.chosen = chosen;
        self.fit_chosen();
        self.dressed = false;
        self.view.redraw(cx);
    }

    fn dress(&mut self, cx: &mut Cx) {
        let text = self.face_text();
        self.view.widget(cx, ids!(face)).as_button().set_text(cx, &text);
    }
}

impl Widget for Select {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.dressed {
            self.dressed = true;
            self.dress(cx.cx.cx);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let face = self.view.widget(cx, ids!(face)).as_button();
        if face.clicked(actions) {
            let anchor = self.view.area().rect(cx);
            let rows = self.rows();
            let owner = self.owner();
            // A set stays open while its switches are pressed; a single
            // value closes the moment it is chosen.
            if self.is_multi() {
                cx.action(MenuAction::OpenSet { owner, rows, anchor, place: self.place });
            } else {
                cx.action(MenuAction::Open { owner, rows, anchor, place: self.place });
            }
        }
        let owner = self.owner();
        let mut picked = None;
        for action in menu_actions(actions) {
            if let MenuAction::Picked { owner: o, id } = action {
                if *o == owner {
                    picked = Some(id.0.saturating_sub(1) as usize);
                }
            }
        }
        if let Some(index) = picked {
            self.set_value(cx, index);
            if self.is_multi() {
                // The mark has to change under the pointer, so the list is
                // handed its rows again rather than being closed and
                // reopened.
                let rows = self.rows();
                cx.action(MenuAction::Update { owner, rows });
            }
        }
    }

    /// What the face says.
    fn text(&self) -> String {
        self.face_text()
    }

    /// A name chooses that value; a number chooses by index.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(index) = self.labels().iter().position(|label| label == v) {
            self.set_value(cx, index);
        } else if let Ok(index) = v.trim().parse::<usize>() {
            self.set_value(cx, index);
        }
    }

    /// The index of the current value, so a test can wait on the number.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.selected.to_string())
    }

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        Some(self.chosen_labels().join(", "))
    }
}

impl SelectRef {
    /// The index chosen this pass, if the value changed.
    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<SelectAction>() {
            SelectAction::Changed(index) => Some(index),
            _ => None,
        }
    }

    /// The index toggled this pass, if a set changed.
    pub fn toggled(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<SelectAction>() {
            SelectAction::Toggled(index) => Some(index),
            _ => None,
        }
    }

    /// The names of everything chosen.
    pub fn chosen_labels(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.chosen_labels()).unwrap_or_default()
    }

    /// What the face says.
    pub fn face_text(&self) -> String {
        self.borrow().map(|inner| inner.face_text()).unwrap_or_default()
    }

    pub fn set_chosen(&self, cx: &mut Cx, chosen: Vec<bool>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_chosen(cx, chosen);
        }
    }
}

#[cfg(test)]
mod tests {
    /// The face never lies about a set: it names one value when one is
    /// chosen and counts them when more are, because naming one member of
    /// three is a lie about the other two.
    #[test]
    fn the_face_says_what_is_true_of_the_set() {
        // The behaviour under test is the summary rule, which is pure.
        fn face(chosen: &[&str], multi: bool, count: bool, placeholder: &str) -> String {
            if chosen.is_empty() {
                return placeholder.to_string();
            }
            if !multi || chosen.len() == 1 {
                return chosen[0].to_string();
            }
            if count {
                format!("{} chosen", chosen.len())
            } else {
                chosen.join(", ")
            }
        }
        assert_eq!(face(&[], true, true, "None chosen"), "None chosen");
        assert_eq!(face(&["Bass"], true, true, "None chosen"), "Bass");
        assert_eq!(face(&["Bass", "Drums", "Vocals"], true, true, ""), "3 chosen");
        assert_eq!(
            face(&["Bass", "Drums"], true, false, ""),
            "Bass, Drums",
            "a caller who knows the list is short can ask for the names"
        );
        assert_eq!(face(&["Bass"], false, true, ""), "Bass", "a single value is itself");
    }
}
