//! The nav list story: a rail and a bar over the same model, and the one
//! thing every hand-rolled version of this gets wrong.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.NavListOverview = StoryPage{
        StoryNote{text: "A handful of destinations, exactly one of them where you are. Six strips in five apps in this repository are this control written six times; four paint the lit one four different ways and two never paint it at all, so in those two the strip cannot say where you are."}

        StoryHeading{text: "A rail and a bar are one widget"}
        StoryNote{text: "The same list, the same model, the same keyboard. Down a side or along an edge is the flow, not a different control."}
        StoryRow{
            View{
                width: 180. height: 190.
                draw_bg +: {color: theme.color_surface_container_low}
                rail := NavRail{
                    width: Fill
                    height: Fit
                    labels: ["Overview" "Detail" "History" "Settings"]
                }
            }
            View{
                width: 420. height: 190. flow: Down
                draw_bg +: {color: theme.color_surface_container_low}
                bar := NavBar{
                    width: Fill
                    height: Fit
                    labels: ["Overview" "Detail" "History" "Settings"]
                }
            }
        }

        StoryHeading{text: "What it reports"}
        StoryNote{text: "Choosing raises one typed action carrying the destination. None of the six hand-rolled strips reports anything: each host polls its own buttons by id."}
        StoryRow{
            picked := Label{text: "nothing chosen yet"}
        }

        StoryHeading{text: "The keyboard nobody had"}
        StoryNote{text: "Click a destination, then use the arrow keys: they move the choice and choose as they go. Home and End go to the ends. The whole list is ONE tab stop — five destinations should be one Tab away, not five."}
        StoryRow{
            keyed := NavBar{
                width: Fill
                height: Fit
                labels: ["One" "Two" "Three" "Four" "Five"]
            }
        }
    }
}

fn nav_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (name, id) in [("rail", ids!(rail)), ("bar", ids!(bar)), ("keyed", ids!(keyed))] {
        let list = root.nav_list(cx, id);
        if list.chosen(actions).is_some() {
            let where_ = list.text();
            root.label(cx, ids!(picked))
                .set_text(cx, &format!("{name}: {where_}"));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/nav-list/overview",
    category: "Navigation",
    component: "NavList",
    name: "Overview",
    dsl: "NavListOverview",
    added: "2026-09-08",
    tags: &["new"],
    doc: "# NavList

A set of destinations of which exactly one is where you are.

**It owns the model, not the look.** The rows come from a `destination` template the caller supplies, because the six strips this replaces have six different looks — a row drawn in Rust here would be re-skinned by every one of them, which is the same reason a shared splitter preset turned out to be worth nothing.

What cannot be written per-app cheaply, and so lives here: the rule that exactly one is lit, **one** keyboard stop for the whole list rather than one per row, arrows that move the choice and choose as they go, and a typed action saying which destination was picked.

**A row must be radio-shaped** — a `RadioButton` or a preset of one. That is the contract: the list lights exactly one and puts the others out, and a row that cannot be lit cannot be a destination.

`NavRail` and `NavBar` are not two widgets. They are `flow: Down` and `flow: Right` over the same list.",
    subject: "rail",
    feature: None,
    controls: &[],
    on_actions: Some(nav_actions),
}];
