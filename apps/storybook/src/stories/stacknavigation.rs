//! The stack navigation stories: a root view pushing three views with nested pushes and a pop to root, ported from the widget zoo.
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
    also: &["StackNavigationView"],
    name: "Overview",
    dsl: "StackNavigationOverview",
    added: "2026-05-23",
    tags: &["ported"],
    doc: "# StackNavigation\n\nStackNavigation provides bounded visual push/pop transitions. It keeps only the current visual view and one transition view; callers that need screen history should store it separately.\n\n## Features\n- Push a view with a slide-in transition\n- Pop the current view to root or to a caller-provided previous view\n- Parent-contained slide animation transitions\n- Built-in header with back button\n\n## Usage\nClick the buttons in the root view to push different views. Use the back button (top-left) or mouse back button to pop to the root view.\n\n## API\n- `push(cx, view_id)` - Show a view\n- `pop(cx)` - Return to root\n- `pop_to_view(cx, view_id)` - Reveal a caller-managed previous view\n- `pop_to_root(cx)` - Return to root\n- `depth()` - Visual depth\n- `can_pop()` - Check if a visual view is active",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];
