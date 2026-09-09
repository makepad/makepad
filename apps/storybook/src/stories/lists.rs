//! The lists story: one that draws every row it is given, and one that lets
//! a person drag the rows into a different order.
use crate::makepad_widgets::flat_list::FlatList;
use crate::makepad_widgets::portal_list::PortalList;
use crate::makepad_widgets::reorder_list::ReorderListAction;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryListsBase = #(StoryLists::register_widget(vm))

    mod.storybook.StoryLists = set_type_default() do mod.storybook.StoryListsBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        View{
            width: Fill height: 180.
            flat := FlatList{
                width: Fill height: Fill
                Row := View{
                    width: Fill height: Fit
                    padding: theme.mspace_1
                    label := Label{text: ""}
                }
            }
        }

        View{
            width: Fill height: 220.
            // The gripper is named here and pointed at: the list watches that
            // one child of every row and starts a reorder from a press on it,
            // so a drag anywhere else still scrolls.
            reorder := ReorderList{
                width: Fill height: Fill
                drag_handle: @gripper
                Row := View{
                    width: Fill height: Fit
                    flow: Right
                    spacing: theme.space_2
                    align: Align{y: 0.5}
                    padding: theme.mspace_1
                    gripper := View{
                        width: Fit height: Fit
                        padding: theme.mspace_1
                        show_bg: true
                        draw_bg +: {color: theme.color_surface_container_high}
                        Label{text: "\u{2261}" draw_text +: {color: theme.color_text_meta}}
                    }
                    label := Label{text: ""}
                }
            }
        }
    }

    mod.stories.ListsOverview = StoryPage{
        StoryNote{text: "Two lists that are not the portal list, and one of them is only a specialisation of it. What separates all three is how much they hold on your behalf."}

        StoryHeading{text: "A list of rows, and a list you can reorder"}
        StoryNote{text: "The first draws whatever rows it is handed. The second is a portal list that also watches one named child of each row — the gripper — and turns a press on that into a drag. Take hold of a gripper and move a row; the order below follows, because the list moved nothing itself."}
        demo := mod.storybook.StoryLists{}
        StoryRow{
            order := Label{text: "order: A B C D E F G H"}
        }
    }
}

const NAMES: &[&str] = &["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel"];

#[derive(Script, ScriptHook, Widget)]
pub struct StoryLists {
    #[deref]
    view: View,
    /// The model. The reorder list deliberately owns none of this: it reports
    /// where a row was dropped and the host is what actually moves it.
    #[rust(NAMES.iter().map(|s| s.to_string()).collect::<Vec<String>>())]
    order: Vec<String>,
}

impl Widget for StoryLists {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<FlatList>() {
                // A flat list is asked for each row by id; it draws them all
                // rather than working out which are on screen.
                for (i, name) in self.order.iter().enumerate() {
                    let id = LiveId(i as u64 + 1);
                    if let Some(row) = list.item(cx, id, live_id!(Row)) {
                        row.label(cx, ids!(label)).set_text(cx, name);
                        row.draw_all(cx, &mut Scope::empty());
                    }
                }
                continue;
            }
            // A reorder list IS a portal list, so it is driven like one.
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.order.len());
                while let Some(id) = list.next_visible_item(cx) {
                    let Some(name) = self.order.get(id) else {
                        continue;
                    };
                    let name = name.clone();
                    let row = list.item(cx, id, live_id!(Row));
                    row.label(cx, ids!(label)).set_text(cx, &name);
                    row.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl StoryLists {
    /// Apply a reorder the list has only reported. Nothing moves until this
    /// runs, which is the whole contract.
    fn move_row(&mut self, from: usize, to: usize) -> bool {
        if from >= self.order.len() || to > self.order.len() || from == to {
            return false;
        }
        let row = self.order.remove(from);
        let to = if to > from { to - 1 } else { to };
        self.order.insert(to.min(self.order.len()), row);
        true
    }

    fn order_text(&self) -> String {
        format!("order: {}", self.order.join(" "))
    }
}

fn lists_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let host = root.widget(cx, ids!(demo));
    let mut moved = None;
    for action in actions {
        if let ReorderListAction::Reordered { from, to } = action.as_widget_action().cast() {
            moved = Some((from, to));
        }
    }
    let Some((from, to)) = moved else { return };
    let mut text = None;
    if let Some(mut inner) = host.borrow_mut::<StoryLists>() {
        if inner.move_row(from, to) {
            text = Some(inner.order_text());
        }
    }
    if let Some(text) = text {
        root.label(cx, ids!(order)).set_text(cx, &text);
        host.redraw(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data display/lists/overview",
    category: "Data display",
    component: "Lists",
    also: &["FlatList", "ReorderList"],
    name: "Overview",
    dsl: "ListsOverview",
    added: "2025-05-06",
    tags: &[],
    doc: "# FlatList and ReorderList

Two lists that are not `PortalList`, though one of them is made of it. What separates all three is **how much each holds on your behalf**.

## FlatList

Asked for each row by id, and it draws them all. There is no visible range to compute and no recycling: it is the list you want when there are twenty rows and virtualisation would be ceremony. Give it more than a screenful of expensive rows and it will draw every one of them.

## ReorderList

A `PortalList` that can also reorder itself — templates, scrolling, virtualisation and item actions all pass straight through, because the portal list is its deref base.

What it adds is one rule: `drag_handle` names **a child of the row template**, and a press on that child becomes a reorder instead of a scroll. The press is captured by the reorder list *before* the inner portal list sees it, so drag-to-scroll never fights the gesture — and a drag anywhere else on the row still scrolls, which is what you want.

**It moves nothing.** On release it raises `Reordered { from, to }` with indices into the host's own range, and stops. The model is yours, the move is yours, the redraw is yours. This page keeps a `Vec` of names and applies the move itself; take the list's report away and the rows spring back, because the list never held the order in the first place.

The widget's own source records why no `Area` is cached anywhere inside it: a finger capture is keyed on the captured widget's area, every redraw remaps that capture to a fresh one, and any area a widget snapshots for itself goes stale on the first redraw — `hits` then fails `is_valid` and returns nothing for ever. The gripper's current area is re-resolved from the live row on every event instead.",
    subject: "demo",
    feature: None,
    controls: &[],
    on_actions: Some(lists_actions),
}];
