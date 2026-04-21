use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let StackNavDemoButton = Button{
        width: Fit height: Fit
        padding: Inset{top: 10. bottom: 10. left: 20. right: 20.}
        margin: Inset{top: 5. bottom: 5.}
    }

    mod.widgets.DemoStackNavigation = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# StackNavigation\n\nStackNavigation provides a stack-based navigation pattern with slide-in/slide-out transitions.\n\n## Features\n- Push views onto a navigation stack\n- Pop views with back button or swipe\n- Slide animation transitions (full screen mode)\n- Nested navigation support\n- Built-in header with back button\n\n## Usage\nClick the buttons in the root view to push different views onto the stack. Use the back button (top-left) or mouse back button to pop.\n\n## Note\nThis demo uses `full_screen: false` to stay within the dock tab. In a real app, full-screen mode slides views across the entire window.\n\n## API\n- `push(cx, view_id)` - Navigate to view\n- `pop(cx)` - Go back one level\n- `pop_to_root(cx)` - Return to root\n- `depth()` - Stack depth\n- `can_pop()` - Check if back is possible"}
        }
        demos +: {
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
                    full_screen: false
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
                    full_screen: false
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
                    full_screen: false
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
}
