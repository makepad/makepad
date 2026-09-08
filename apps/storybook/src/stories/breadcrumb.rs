//! The breadcrumb story: which end a trail gives up when it runs out of
//! room, shown side by side with the same trail that has room to spare.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.BreadcrumbOverview = StoryPage{
        StoryNote{text: "Where you are, and every step back to the top. A trail has one rule that outranks the rest: the last crumb is where you are, and it must never be the one that disappears."}

        StoryHeading{text: "Room to spare"}
        View{
            width: Fill height: Fit
            subject := Breadcrumb{
                trail: ["Home" "Projects" "makepad" "widgets" "src"]
            }
        }

        StoryHeading{text: "The same trail, squeezed"}
        StoryNote{text: "The middle folds into an ellipsis and the two ends stay, because the top of the tree and where you are now are the two places anyone navigates to. Squeeze it further and the root goes too — never the leaf."}
        View{
            width: 260. height: Fit
            draw_bg +: {color: theme.color_surface_container_low}
            narrow := Breadcrumb{
                width: Fill
                trail: ["Home" "Projects" "makepad" "widgets" "src" "themes" "desktop"]
            }
        }
        View{
            width: 130. height: Fit
            draw_bg +: {color: theme.color_surface_container_low}
            tiny := Breadcrumb{
                width: Fill
                trail: ["Home" "Projects" "makepad" "widgets" "src" "themes" "desktop"]
            }
        }

        StoryHeading{text: "The crumb you are on"}
        StoryNote{text: "By default the last crumb is not clickable: navigating to where you already are is not a thing to offer. A host that wants it back says so."}
        StoryRow{
            View{
                width: 300. height: Fit
                plain := Breadcrumb{trail: ["Home" "Projects" "here"]}
            }
            View{
                width: 300. height: Fit
                clickable := Breadcrumb{trail: ["Home" "Projects" "here"] current_interactive: true}
            }
        }

        StoryHeading{text: "What was picked"}
        StoryRow{
            picked := Label{text: "nothing picked yet"}
        }
    }
}

fn breadcrumb_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for id in [ids!(subject), ids!(narrow), ids!(tiny), ids!(plain), ids!(clickable)] {
        if let Some(index) = root.breadcrumb(cx, id).picked(actions) {
            let trail = root.breadcrumb(cx, id).text();
            root.label(cx, ids!(picked))
                .set_text(cx, &format!("picked crumb {index} of \"{trail}\""));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/breadcrumb/overview",
    category: "Navigation",
    component: "Breadcrumb",
    name: "Overview",
    dsl: "BreadcrumbOverview",
    added: "2026-09-08",
    tags: &["new"],
    doc: "# Breadcrumb

Where you are, and every step back to the top.

**One rule outranks the rest: the last crumb must never be the one that disappears.** A trail too long for its room has to drop something, and dropping the end tells the reader where they came from while hiding where they got to — which is the one thing the control exists to say. Both trails already in this repository broke that rule in different ways before this widget existed, so it is enforced by `breadcrumb_window`, a free function with its own tests, rather than left to each drawing.

What folds is the middle. The root stays while it can be afforded alongside the mark and the leaf, because the top of the tree is the other end people navigate to; squeezed harder than that the root goes as well, and only where you are is left.

`current_interactive` is off by default: the crumb you are already on does not answer a click, since navigating to where you already are is not a thing to offer.

`OverflowRow` is deliberately not used here — it fills from the front and hides the tail, which is exactly backwards for a trail.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Separator", target: "subject", kind: ControlKind::Text { prop: "separator", default: "\u{203a}" } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 24., step: 1., default: 6. } },
        Control { label: "Current clickable", target: "subject", kind: ControlKind::Bool { prop: "current_interactive", default: false } },
    ],
    on_actions: Some(breadcrumb_actions),
}];
