use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ViewA = RoundedView{
        width: 200 height: Fill
        show_bg: true
        draw_bg.color: #176951
        padding: 10.
        align: Align{x: 0.5 y: 0.5}
        Label{text: "View A"}
    }

    let ViewB = RoundedView{
        width: Fill height: Fill
        show_bg: true
        draw_bg.color: #1f3a67
        padding: 10.
        flow: Down
        spacing: 5.
        align: Align{x: 0.5 y: 0.5}
        Label{text: "View B"}
    }

    mod.widgets.DemoAdaptiveView = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# AdaptiveView\n\nAdaptiveView adapts its content based on the current context, automatically switching between layout variants.\n\n## Features\n- Define named variants (default: `Desktop` / `Mobile`)\n- Automatic switching based on screen width (>= 860px = Desktop)\n- Custom variant selectors with access to `Cx` and parent size\n- Parent-size-relative breakpoints (not just window size)\n- Any number of custom-named variants\n- Optional state retention for unused variants\n- Responds to window resize and parent size changes\n\n## Default Selector\nSwitches at 860px window width. Override with `set_variant_selector`.\n\n## Custom Variants (Rust)\n```rust\n// Parent-size-based with custom names:\nself.adaptive_view(ids!(my_view))\n  .set_variant_selector(\n    |_cx, parent_size| {\n      match parent_size.x {\n        w if w <= 70.0  => id!(OnlyIcon),\n        w if w <= 200.0 => id!(Compact),\n        _ => id!(Full),\n      }\n    }\n  );\n```\nThe selector receives `&mut Cx` and the parent container size (`&Vec2d`), so variants can adapt to the available space, not just the window.\n\n## API\n- `set_variant_selector(closure)` - Custom selector\n- `set_default_variant_selector()` - Reset to default\n- `retain_unused_variants` - Preserve inactive variant state"}
        }
        demos +: {
            H4{text: "Default Desktop/Mobile"}
            Label{
                width: Fill height: Fit
                text: "Resize the window below 860px width to see it switch to Mobile."
            }

            RoundedView{
                width: Fill height: 200
                show_bg: true
                draw_bg.color: theme.color_bg_app
                padding: 10.

                AdaptiveView{
                    Desktop := View{
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        spacing: 20.

                        ViewA{}
                        ViewB{}
                    }

                    Mobile := View{
                        flow: Down
                        align: Align{x: 0.5 y: 0.0}
                        spacing: 10.

                        ViewA{}
                        ViewB{}
                    }
                }
            }
        }
    }
}
