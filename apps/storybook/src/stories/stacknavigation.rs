//! The stack navigation story: a root view pushing three pages, a push from
//! inside a pushed page, and the way back to the root.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let StackNavDemoButton = Button{
        width: Fit height: Fit
        padding: Inset{top: 10. bottom: 10. left: 20. right: 20.}
        margin: Inset{top: 5. bottom: 5.}
    }

    mod.stories.StackNavigationOverview = StoryPage{
        StoryNote{text: "A root view with pages pushed over it. A pushed page slides in from the side under a header with a back button, and going back slides it away again."}

        StoryHeading{text: "Push, and go back"}
        StoryNote{text: "Push a page from the root, then push another from inside it. The back button at the top left, the mouse's back button and the platform's back gesture all return to the root rather than to the page before: the widget keeps the page in view and the one leaving, and no history."}
        // The navigation draws each page it shows with its own walk and
        // reserves no room of its own, so a pushed page would collapse the
        // space and let the section below ride up under it. The frame holds
        // the height, and its overlay flow lays the pages over each other.
        View{
            width: Fill height: 440.
            flow: Overlay
            stack_nav_demo := StackNavigation{
                width: Fill height: Fill

                root_view +: {
                    flow: Down
                    align: Align{x: 0.5 y: 0.3}
                    spacing: 10.
                    padding: 20.

                    H3{text: "Root View"}
                    Label{text: "This is the root of the StackNavigation."}

                    push_view_a := StackNavDemoButton{
                        text: "Push View A"
                    }
                    push_view_b := StackNavDemoButton{
                        text: "Push View B"
                    }
                    push_view_c := StackNavDemoButton{
                        text: "Push View C"
                    }
                }

                stack_view_a := StackNavigationView{
                    header +: {
                        content +: {
                            title_container +: {
                                title +: {text: "View A"}
                            }
                        }
                    }
                    body +: {
                        flow: Down
                        align: Align{x: 0.5 y: 0.3}
                        spacing: 10.
                        padding: 20.

                        H3{text: "View A"}
                        Label{text: "This view was pushed onto the stack.\nUse the back button (top-left) or mouse back to pop."}
                        push_nested_from_a := StackNavDemoButton{
                            text: "Push View B from here"
                        }
                    }
                }

                stack_view_b := StackNavigationView{
                    header +: {
                        content +: {
                            title_container +: {
                                title +: {text: "View B"}
                            }
                        }
                    }
                    body +: {
                        flow: Down
                        align: Align{x: 0.5 y: 0.3}
                        spacing: 10.
                        padding: 20.

                        H3{text: "View B"}
                        Label{text: "Another stack view.\nYou can push more views from here."}
                        push_nested_from_b := StackNavDemoButton{
                            text: "Push View C from here"
                        }
                    }
                }

                stack_view_c := StackNavigationView{
                    header +: {
                        content +: {
                            title_container +: {
                                title +: {text: "View C"}
                            }
                        }
                    }
                    body +: {
                        flow: Down
                        align: Align{x: 0.5 y: 0.3}
                        spacing: 10.
                        padding: 20.

                        H3{text: "View C"}
                        Label{text: "Deepest view in this demo."}
                        pop_to_root_btn := StackNavDemoButton{
                            text: "Pop to Root"
                        }
                    }
                }
            }
        }

        StoryHeading{text: "The header"}
        StoryNote{text: "Every pushed page carries a header: its title in the middle and the back button at the leading edge. The title is header.content.title_container.title, written in the markup as these pages do, or set at runtime with set_title."}
        StoryNote{text: "StackViewHeader is that header on its own. It is 80 points tall with 50 of them above the title, and lays the back button over the leading edge of the title row, so a long title stays centred on the page rather than on the space the button leaves."}
        StoryRow{
            View{
                width: 320. height: Fit
                StackViewHeader{
                    content +: {
                        title_container +: {
                            title +: {text: "Settings"}
                        }
                    }
                }
            }
        }
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let stack_nav = root.stack_navigation(cx, ids!(stack_nav_demo));

    if root.button(cx, ids!(push_view_a)).clicked(actions) {
        stack_nav.push(cx, live_id!(stack_view_a));
    }
    if root.button(cx, ids!(push_view_b)).clicked(actions) {
        stack_nav.push(cx, live_id!(stack_view_b));
    }
    if root.button(cx, ids!(push_view_c)).clicked(actions) {
        stack_nav.push(cx, live_id!(stack_view_c));
    }
    if root.button(cx, ids!(push_nested_from_a)).clicked(actions) {
        stack_nav.push(cx, live_id!(stack_view_b));
    }
    if root.button(cx, ids!(push_nested_from_b)).clicked(actions) {
        stack_nav.push(cx, live_id!(stack_view_c));
    }
    if root.button(cx, ids!(pop_to_root_btn)).clicked(actions) {
        stack_nav.pop_to_root(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/stacknavigation/overview",
    category: "Navigation",
    component: "StackNavigation",
    also: &["StackNavigationView", "StackViewHeader"],
    name: "Overview",
    dsl: "StackNavigationOverview",
    added: "2026-05-23",
    tags: &["ported"],
    doc: "# StackNavigation\n\nA root view with pages pushed over it, one at a time. A pushed page slides in from the side under a header with a back button, and going back slides it away. The slide stays inside the navigation's own rect, so it can sit in one pane of a window.\n\nIt is bounded on purpose: it keeps the page in view and the one in transition, and no history. Back returns to the root, not to the page before, so a host that wants a trail of screens keeps that trail itself and reveals the right page with `pop_to_view`. A push that arrives while a slide is still running is ignored.\n\nThe pages are `StackNavigationView` children declared beside `root_view`, each with a `header` and a `body`. The header is a `StackViewHeader`, which holds the title and the back button: 80 points tall, the title centred in the lower part and the back button laid over its leading edge. The back button, the mouse's back button and the platform's back gesture all raise the same pop.\n\n## Driving it\n\n| Call | Result |\n|---|---|\n| `push(cx, id)` | slides the page with that id in |\n| `pop(cx)`, `pop_to_root(cx)` | slide the page away and show the root |\n| `pop_to_view(cx, id)` | slides the page away and reveals the page with that id behind it |\n| `set_title(cx, id, title)` | sets a page's title, kept when the page is rebuilt |\n| `can_pop()`, `depth()` | whether a page is showing over the root, as a flag or as 0 or 1 |",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];
