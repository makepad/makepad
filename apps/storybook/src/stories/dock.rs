//! The dock story: tabs in panels a person can rearrange, and the three
//! handlers without which none of the rearranging happens.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let DockBody = SolidView{
        width: Fill
        height: Fill
        align: Align{x: 0.5, y: 0.5}
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.DockOverview = StoryPage{
        StoryNote{text: "Panels of tabs, split any way round, that a person can take apart and put back together. Eight places in this repository build one; the editor you may know is the biggest of them."}

        StoryHeading{text: "Drag a tab"}
        StoryNote{text: "Drag a tab onto the other bar to move it. Drag it against an edge of a body — the outer tenth — to split THAT panel and take half of it. Drag it right out to the edge of the dock instead and it lies across everything: with two panels sharing the square, a drop down the far left gives the newcomer the whole left half and leaves the other two stacked in the right half. Drop it along the outside of the way they are already stacked and all three end up the same size, and so does dropping it straight onto the bar between them. Drag the bar between panels to resize them; nothing a drop decides is final. Close a tab with its cross. Everything here is reported below as it happens."}
        StoryRow{
            View{
                width: Fill height: 340.
                specimen := Dock{
                    width: Fill
                    height: Fill

                    // `root` is not a name I chose. The dock seeds its draw
                    // from id!(root) and nothing else, and a dock without one
                    // draws nothing at all and says nothing about it.
                    root := DockSplitter{
                        axis: Horizontal
                        align: Weighted(0.5)
                        a: @left_panel
                        b: @right_panel
                    }

                    // `closable` is the panel's own word, and it does not
                    // mean what the tabs' crosses mean: it is whether this
                    // panel goes away when its last tab leaves, handing its
                    // room back to whatever it was sharing with. Declared
                    // false, the room stays empty after the last tab is
                    // dragged out, which reads as a bug and is a setting.
                    left_panel := DockTabs{tabs: [@tab_one @tab_two] selected: 0 closable: true}
                    right_panel := DockTabs{tabs: [@tab_three] selected: 0 closable: true}

                    // `template` picks the tab's chrome, and it is the ONLY
                    // thing that decides whether a tab has a cross.
                    tab_one := DockTab{name: "One" template: @CloseableTab kind: @BodyOne}
                    tab_two := DockTab{name: "Two" template: @CloseableTab kind: @BodyTwo}
                    tab_three := DockTab{name: "Three" template: @PermanentTab kind: @BodyThree}

                    BodyOne := DockBody{Label{text: "one"}}
                    BodyTwo := DockBody{Label{text: "two"}}
                    BodyThree := DockBody{Label{text: "three, and no cross"}}
                }
            }
        }
        StoryRow{
            reported := Label{text: "nothing yet"}
        }
    }
}

fn dock_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let dock = root.dock(cx, ids!(specimen));
    let mut said: Option<String> = None;

    for action in actions {
        match action.as_widget_action().cast() {
            // A request, not an event. The dock will not start the drag for
            // you, and without this the tab bar does nothing on a drag: no
            // ghost, no drop preview, no reorder. Every dock in this
            // repository except the editor's is inert for exactly this
            // reason.
            DockAction::ShouldTabStartDrag(tab_id) => {
                dock.tab_start_drag(
                    cx,
                    tab_id,
                    DragItem::FilePath {
                        path: String::new(),
                        // The tab's identity has to travel inside the drag
                        // payload; there is nowhere else it survives.
                        internal_id: Some(tab_id),
                    },
                );
                said = Some("dragging a tab".to_string());
            }
            DockAction::Drag(drag) => {
                dock.accept_drag(cx, drag, DragResponse::Move);
            }
            DockAction::Drop(drop) => {
                if let Some(DragItem::FilePath {
                    internal_id: Some(tab_id),
                    ..
                }) = drop.items.first()
                {
                    dock.drop_move(cx, drop.abs, *tab_id);
                    said = Some("dropped, and the layout changed".to_string());
                }
            }
            // Also a request. The cross animates and closes nothing until
            // the host says so.
            DockAction::TabCloseWasPressed(tab_id) => {
                dock.close_tab(cx, tab_id);
                said = Some("closed a tab".to_string());
            }
            DockAction::TabWasPressed(_) => {
                said = Some("selected a tab (the dock did that itself)".to_string());
            }
            DockAction::SplitPanelChanged { .. } => {
                said = Some("resized the panels".to_string());
            }
            _ => {}
        }
    }

    if let Some(text) = said {
        let label = root.label(cx, ids!(reported));
        if label.text() != text {
            label.set_text(cx, &text);
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/dock/overview",
    category: "Containers",
    component: "Dock",
    also: &["Tab", "TabBar"],
    name: "Overview",
    dsl: "DockOverview",
    added: "2025-05-06",
    tags: &["layout"],
    doc: "# Dock

Panels of tabs, split any way round, that a person can take apart and put back together. Eight places in this repository build one.

Unlike the portal list or the data grid, it needs no draw loop: the bodies are templates declared inside the `Dock{}` block and the dock instantiates them itself. A tab's `kind` names its body template; its `template` names the tab's own chrome.

## `root` is not a name you choose

The dock seeds its draw from `id!(root)` and nothing else. **A dock with no entry called `root` draws nothing and says nothing about why** — no warning, no error, just an empty rectangle. It may be a `DockTabs` or a `DockSplitter`.

## Dragging is opt-in, and almost nothing opts in

`ShouldTabStartDrag` is a *request*. The dock will not call `start_dragging` for you, so a dock with no handler for it has a tab bar that does nothing when dragged: no ghost, no drop preview, no reorder. It takes three handlers — `ShouldTabStartDrag` to begin, `Drag` to accept, `Drop` to move — and the tab's identity has to be smuggled through the drag payload's `internal_id`, because there is nowhere else it survives the trip.

Of the eight docks in this repository, exactly one wires them. Every other one, including every example, is inert. This page wires them, which is why the tabs above actually move.

## Two names that read backwards

**The close button does not close.** `TabCloseWasPressed` is a request too; the cross animates and the tab stays until the host calls `close_tab`.

**`closable` is not what makes the cross.** On a `DockTabs` panel, `closable` means *when the last tab leaves, collapse this panel and hand its room back to whatever it was sharing with*. Declare it false and an emptied panel stays on screen as a blank half, which reads as a bug and is a setting. The cross comes from the tab's `template` — `@CloseableTab` has one, `@PermanentTab` does not. Note the spelling: `closable` on the panel, `closeable` on the tab. The third tab above is permanent, and has no cross.

## Where a drop lands

A drop is read in five passes, and the order between them is the behaviour.

**A tab.** Dropping on one puts the newcomer beside it, wherever that tab happens to be.

**The outside of the dock**, within 24 points of it. This splits the *root*, so the panel arriving lies across whatever is already in there rather than beside one member of it. Two panels sharing a square top and bottom, and a drop down the far left: the newcomer takes the whole left half and the pair stay stacked in the right one. The preview says which it will be — half the whole dock, not half a panel.

**The bar between two panels**, within a few points of it — the same slop the bar's own drag uses. This puts the newcomer BETWEEN them rather than inside either, and moves both bars so everything sharing that direction ends up the same size: two panels and a newcomer become three thirds. A bar somebody pinned a fixed distance from one side is left where it was pinned and the newcomer goes on the other side of the seam, because a pinned pane is one whose size was decided on purpose.

**The empty part of a tab bar**, which joins that panel.

**The edges and middle of the panel under the pointer.** The outer tenth of a body splits that panel alone and nests a splitter inside it; the middle joins its tabs.

The outside of the dock has to beat the tab bars, and that is not a preference. The topmost panels' bars run along the top edge, so a tab-bar pass in front of the outer one meant a drop could take the bottom half of the dock and never the top. Nothing is lost by the order: dropping in the middle of a panel joins that panel exactly as dropping on its bar does.

The bar of a new split starts where everything sharing that direction ends up the same size: beside one panel that is the middle, and under a pair already stacked it is a third. Every bar can be dragged afterwards, so those fractions are only where it opens.

## Smaller sharp edges

`hide_tab_bar: true` also stops that panel being a drop target, which the name does not say. `create_tab`'s parent must be a `DockTabs` id, not a splitter's, or it returns nothing silently. A `kind` that names no template is a warning and an empty body, not a failure. And the dock fills by default, so a `Fit` parent collapses it to nothing — give it a height.",
    subject: "specimen",
    feature: None,
    controls: &[],
    on_actions: Some(dock_actions),
}];
