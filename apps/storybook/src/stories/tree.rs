//! The tree component's overview: the general tree, with rows that fold,
//! an indent you can trace by eye, boxes that cascade, and the keyboard
//! walk that makes a deep tree usable without the mouse. The component's
//! other page, the file browser, is `filetree.rs`.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TreeOverview = StoryPage{
        StoryNote{text: "A tree is three things a list is not: rows that hide other rows, rows that stand at a depth, and a tick on a branch that has to say something true about the leaves under it. All three are arithmetic, and all three are what every hand-rolled tree writes again for itself."}

        StoryHeading{text: "Depth you can see"}
        StoryNote{text: "Each row carries a hairline standing exactly where its ancestor's fold mark stands, so a row four levels down can be traced back to its parent by eye. Turn the guides off and the same tree is a stack of indented words."}
        StoryRow{
            subject := TreeView{
                width: 240. height: Fit
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Documents"
                    "  Reports"
                    "    Spring"
                    "    Autumn"
                    "  Letters"
                    "    To the council"
                    "Pictures"
                    "  Harbour"
                ]
            }
            TreeView{
                width: 240. height: Fit
                show_guides: false
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Documents"
                    "  Reports"
                    "    Spring"
                    "    Autumn"
                    "  Letters"
                    "    To the council"
                    "Pictures"
                    "  Harbour"
                ]
            }
        }

        StoryHeading{text: "The fold mark"}
        StoryNote{text: "A plus opens a branch and a minus shuts it, and only a branch has one. It is two rectangles rather than a chevron on purpose: small marks drawn as shader paths do not paint reliably here, and a fold mark that sometimes is not there is worse than a plain one that always is."}
        StoryNote{text: "The tree on the right arrives shut. Folding is a state of the tree, not of the host: fold a branch, unfold it again, and everything inside comes back exactly as it was."}
        StoryRow{
            folds := TreeView{
                width: 240. height: Fit
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Site"
                    "  Ground floor"
                    "    Hall"
                    "    Kitchen"
                    "  First floor"
                    "    Study"
                ]
            }
            TreeView{
                width: 240. height: Fit
                start_open: false
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Site"
                    "  Ground floor"
                    "    Hall"
                    "    Kitchen"
                    "  First floor"
                    "    Study"
                ]
            }
        }
        StoryRow{
            fold_state := Label{text: "nothing folded yet"}
        }

        StoryHeading{text: "One row, or many"}
        StoryNote{text: "A plain tree selects one row. Turn multi_select on and shift takes the range from the last row you touched, while control or command adds and removes one at a time. A row folded out of sight stays selected — a fold hides a row, it does not throw it away."}
        StoryRow{
            many := TreeView{
                width: 300. height: Fit
                multi_select: true
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Team"
                    "  Design"
                    "    Ana"
                    "    Bo"
                    "  Build"
                    "    Cai"
                    "    Dee"
                ]
            }
            picked := Label{text: "nothing selected yet"}
        }

        StoryHeading{text: "Boxes that cascade"}
        StoryNote{text: "Ticking a branch reaches every leaf under it, and a branch whose leaves disagree shows a dash rather than a tick. The branch state is derived from the leaves every time it is drawn, so a parent can never sit there claiming something its children contradict."}
        StoryNote{text: "A dash for mixed and a dot for on, for the same reason the fold mark is a plus: these are rectangles and circles, which paint every time."}
        StoryRow{
            boxes := TreeView{
                width: 300. height: Fit
                checkable: true
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Export"
                    "  Images"
                    "    Full size"
                    "    Thumbnails"
                    "  Text"
                    "    Captions"
                    "    Notes"
                ]
            }
            counted := Label{text: "nothing ticked yet"}
        }

        StoryHeading{text: "A glyph before the label"}
        StoryNote{text: "The icon column is a list of glyphs beside the outline, one per line. It is text, not a resource, so the column costs nothing when no row uses it — and a caller has to pick a glyph the default fonts carry, because a missing one draws an empty box."}
        StoryRow{
            TreeView{
                width: 240. height: Fit
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Library"
                    "  Sets"
                    "    Evening"
                    "  Loops"
                ]
                icons: ["\u{25CF}" "\u{25CF}" "\u{25CB}" "\u{25CF}"]
            }
        }

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "Tab into the tree, then walk it: up and down move a row at a time through what is visible, right opens a shut branch and then steps into it, left shuts an open one and otherwise climbs to the parent, and Home and End reach the ends. The whole tree is ONE tab stop, so Tab again leaves it rather than walking every row on the way past."}
        StoryNote{text: "Space sets the box on the row you are standing on. Without it the boxes would be reachable by pointer only, which makes a checkable tree unusable from the keyboard."}
        StoryRow{
            keyed := TreeView{
                width: 300. height: Fit
                checkable: true
                multi_select: true
                draw_bg +: {color: theme.color_surface_container_low}
                outline: [
                    "Chapters"
                    "  One"
                    "    Opening"
                    "    Middle"
                    "  Two"
                    "    Closing"
                    "Appendix"
                ]
            }
        }
    }
}

fn tree_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let many = root.tree_view(cx, ids!(many));
    if many.chosen(actions).is_some() {
        let names: Vec<String> = many
            .selected()
            .into_iter()
            .filter_map(|id| many.label_of(id))
            .collect();
        let text = if names.is_empty() {
            "nothing selected".to_string()
        } else {
            format!("{} selected: {}", names.len(), names.join(", "))
        };
        root.label(cx, ids!(picked)).set_text(cx, &text);
    }

    let boxes = root.tree_view(cx, ids!(boxes));
    if boxes.tick_changed(actions).is_some() {
        let names: Vec<String> = boxes
            .ticked()
            .into_iter()
            .filter_map(|id| boxes.label_of(id))
            .collect();
        let text = if names.is_empty() {
            "nothing ticked".to_string()
        } else {
            format!("{} ticked: {}", names.len(), names.join(", "))
        };
        root.label(cx, ids!(counted)).set_text(cx, &text);
    }

    let folds = root.tree_view(cx, ids!(folds));
    if let Some((id, folded)) = folds.folded(actions) {
        let name = folds.label_of(id).unwrap_or_default();
        let state = if folded { "shut" } else { "open" };
        root.label(cx, ids!(fold_state))
            .set_text(cx, &format!("{name} is now {state}"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/tree/overview",
    category: "Data display",
    component: "Tree",
    also: &["TreeView"],
    name: "Overview",
    dsl: "TreeOverview",
    added: "2026-09-10",
    tags: &["new"],
    doc: "# TreeView

The general tree: nested rows that fold, with an indent guide, a fold mark, an optional glyph and a label. The Files page beside this one is the file browser, FileTree, which draws folders and files from a model the host walks; this one takes an outline as text and is for a hierarchy of anything.

**The arithmetic is separate from the drawing.** `visible_rows`, `tick_of`, `cascade_tick` and `selection_for_click` are free functions with their own tests, and the widget is only their painting. Folding re-flattens an index; it never creates or destroys anything.

**Fold state is kept by id, not by index**, so a node that leaves the model and comes back keeps its fold. That is what a host refreshing a tree on a timer needs and what an index-keyed fold cannot give it.

**A branch's box is derived, never stored.** Ticking a branch writes to the leaves under it and nothing else; the branch reads its own state back out of them every time it draws. That is the whole of why a parent can never disagree with its children, and why a mixed box exists at all. A press on a mixed box resolves to on, the way a select-all box does.

**Selection is of rows.** A branch selects itself, not its subtree — this is the general tree, and a host that wants \"select everything under this\" can walk the model it handed over. A row folded out of sight stays selected.

**Marks are rectangles, boxes and circles.** The fold mark is a plus and a minus, the tick is a dot and a dash. Small marks drawn as shader paths do not paint reliably here, and a mark that is sometimes missing is worse than a plain one that is always there.

What it deliberately does NOT do: no virtualisation and no scrolling of its own (every visible row draws every pass, which is honest up to a few hundred rows — past that, put your own list around it), no drag, no rename, no context menu, and no data source. The Files page beside this one is the browser; this is the general one.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Row height", target: "subject", kind: ControlKind::Number { prop: "row_height", min: 16., max: 48., step: 1., default: 24. } },
        Control { label: "Indent", target: "subject", kind: ControlKind::Number { prop: "indent", min: 8., max: 40., step: 1., default: 16. } },
        Control { label: "Guide width", target: "subject", kind: ControlKind::Number { prop: "guide_size", min: 0., max: 3., step: 0.5, default: 1. } },
        Control { label: "Indent guides", target: "subject", kind: ControlKind::Bool { prop: "show_guides", default: true } },
        Control { label: "Checkboxes", target: "subject", kind: ControlKind::Bool { prop: "checkable", default: false } },
        Control { label: "Select many", target: "subject", kind: ControlKind::Bool { prop: "multi_select", default: false } },
    ],
    on_actions: Some(tree_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Cx` with this crate's theme, the shared page templates and this
    /// file's own story registered, and nothing else: a failure here is
    /// this page's, not some other story's.
    fn shell() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        cx
    }

    fn build(cx: &mut Cx, dsl: &str) -> WidgetRef {
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {dsl}");
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// The page is built from the DSL, which the Rust compiler never reads:
    /// the record's template name and the ids the controls panel and the
    /// action handler address are strings, and a wrong one is a page that
    /// opens empty or a label that never changes, with no error anywhere.
    /// The page moved under a new component and its template was renamed
    /// with it, so every one of those strings is asked for here.
    #[test]
    fn the_page_builds_and_every_addressed_id_is_on_it() {
        let mut cx = shell();
        for story in STORIES {
            let page = build(&mut cx, story.dsl);
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert_eq!(
                makepad_platform::shader_error::take(),
                None,
                "{}: a draw shader failed to compile",
                story.key
            );
            let addressed = std::iter::once(story.subject)
                .chain(story.controls.iter().map(|c| c.target))
                .chain(["folds", "fold_state", "many", "picked", "boxes", "counted", "keyed"])
                .filter(|t| !t.is_empty());
            for target in addressed {
                assert!(
                    !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                    "{}: no widget at {}",
                    story.key,
                    target
                );
            }
            for tree in ["subject", "folds", "many", "boxes", "keyed"] {
                assert!(
                    page.widget(&cx, &[LiveId::from_str(tree)]).borrow::<TreeView>().is_some(),
                    "{}: {} is not a TreeView",
                    story.key,
                    tree
                );
            }
        }
    }
}
