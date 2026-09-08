//! The badge stories: dots, counts and the overflow cap, intents,
//! appearances, sizes and shapes, badges anchored on a button and an
//! avatar, status dots, label:value pills and markers over content.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Avatar = CircleView{
        width: 36.
        height: 36.
        show_bg: true
        draw_bg +: {color: theme.color_tertiary_container}
    }

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    mod.stories.BadgeOverview = StoryPage{
        StoryHeading{text: "Dot, count and the overflow cap"}
        StoryNote{text: "A badge with nothing to say is a dot. A count shows its number up to max, then max+; a word shows the word."}
        StoryRow{
            Badge{}
            Badge{count: 1}
            Badge{count: 12}
            Badge{count: 99}
            Badge{count: 100}
            Badge{count: 1204 max: 999}
            Badge{text: "NEW"}
            Badge{count: 0 show_zero: true}
        }

        StoryHeading{text: "Intents"}
        StoryRow{
            Badge{count: 3 intent: Neutral}
            Badge{count: 3 intent: Primary}
            Badge{count: 3 intent: Secondary}
            Badge{count: 3 intent: Tertiary}
            Badge{count: 3 intent: Error}
            Badge{count: 3 intent: Warning}
            Badge{count: 3 intent: Success}
            Badge{count: 3 intent: Info}
        }
        StoryRow{
            Badge{text: "neutral" intent: Neutral}
            Badge{text: "primary" intent: Primary}
            Badge{text: "secondary" intent: Secondary}
            Badge{text: "tertiary" intent: Tertiary}
            Badge{text: "error" intent: Error}
            Badge{text: "warning" intent: Warning}
            Badge{text: "success" intent: Success}
            Badge{text: "info" intent: Info}
        }

        StoryHeading{text: "Appearances"}
        StoryRow{
            Badge{text: "filled" intent: Error appearance: Filled}
            Badge{text: "ghost" intent: Error appearance: Ghost}
            Badge{text: "outline" intent: Error appearance: Outline}
            Badge{text: "tint" intent: Error appearance: Tint}
            Badge{count: 7 intent: Success appearance: Filled}
            Badge{count: 7 intent: Success appearance: Ghost}
            Badge{count: 7 intent: Success appearance: Outline}
            Badge{count: 7 intent: Success appearance: Tint}
        }

        StoryHeading{text: "Sizes"}
        StoryRow{
            Badge{count: 5 intent: Primary size: Tiny}
            Badge{count: 5 intent: Primary size: Small}
            Badge{count: 5 intent: Primary size: Medium}
            Badge{count: 5 intent: Primary size: Large}
            Badge{count: 5 intent: Primary size: Xl}
            Badge{intent: Primary size: Tiny}
            Badge{intent: Primary size: Small}
            Badge{intent: Primary size: Medium}
            Badge{intent: Primary size: Large}
            Badge{intent: Primary size: Xl}
        }

        StoryHeading{text: "Shapes"}
        StoryRow{
            Badge{count: 42 intent: Info shape: Round}
            Badge{count: 42 intent: Info shape: Rounded}
            Badge{count: 42 intent: Info shape: Square}
            BadgeFlat{count: 42 intent: Info shape: Rounded}
            Caption{text: "the last one is BadgeFlat, without the bevel"}
        }

        StoryHeading{text: "Anchored"}
        StoryNote{text: "The badge draws on the overlay, so the button and the avatars keep their size. Increment pushes the count past the cap; the anchor hides itself at 0. The avatars pull their badges in with offset_x and offset_y so they sit on the circle rather than on its bounding box."}
        StoryRow{
            spacing: theme.space_3
            count_badge := BadgeAnchor{
                badge +: {intent: Error}
                Button{text: "Inbox"}
            }
            BadgeAnchor{
                offset_x: -4.
                offset_y: 4.
                badge +: {dot: true intent: Success}
                Avatar{}
            }
            BadgeAnchor{
                corner: BadgeCorner.BottomRight
                offset_x: -4.
                offset_y: -4.
                badge +: {count: 8 intent: Info size: Small}
                Avatar{}
            }
            BadgeAnchor{
                corner: BadgeCorner.TopLeft
                offset_x: 4.
                offset_y: 4.
                badge +: {text: "beta" intent: Tertiary appearance: Tint size: Small}
                ButtonFlat{text: "Settings"}
            }
            inc := ButtonFlat{text: "Increment"}
            reset := ButtonFlat{text: "Reset"}
            count_label := Caption{text: "count 0"}
        }

        StoryHeading{text: "Status dots"}
        StoryNote{text: "Each status has a shape as well as a colour, so none of them is colour-only."}
        StoryRow{
            spacing: theme.space_3
            StoryRow{width: Fit StatusDot{status: StatusKind.Success} Caption{text: "success"}}
            StoryRow{width: Fit StatusDot{status: StatusKind.Warning} Caption{text: "warning"}}
            StoryRow{width: Fit StatusDot{status: StatusKind.Error} Caption{text: "error"}}
            StoryRow{width: Fit StatusDot{status: StatusKind.Info} Caption{text: "info"}}
            StoryRow{width: Fit StatusDot{status: StatusKind.InProgress} Caption{text: "in progress"}}
            StoryRow{width: Fit StatusDot{status: StatusKind.Unknown} Caption{text: "unknown"}}
            StoryRow{width: Fit StatusDot{status: StatusKind.Pending} Caption{text: "pending"}}
        }
        StoryRow{
            spacing: theme.space_3
            StatusDot{width: 20. height: 20. status: StatusKind.Success}
            StatusDot{width: 20. height: 20. status: StatusKind.Warning}
            StatusDot{width: 20. height: 20. status: StatusKind.Error}
            StatusDot{width: 20. height: 20. status: StatusKind.Info}
            StatusDot{width: 20. height: 20. status: StatusKind.InProgress}
            StatusDot{width: 20. height: 20. status: StatusKind.Unknown}
            StatusDot{width: 20. height: 20. status: StatusKind.Pending}
            Caption{text: "the same seven at 20 points"}
        }

        StoryHeading{text: "Label and value"}
        StoryNote{text: "The value half is coloured by where the value sits between min and max."}
        StoryRow{
            LabelValue{label: "cpu" value: 12. unit: "%" max: 100.}
            LabelValue{label: "cpu" value: 55. unit: "%" max: 100.}
            LabelValue{label: "cpu" value: 91. unit: "%" max: 100.}
            LabelValue{label: "latency" value: 42.5 precision: 1 unit: " ms" min: 20. max: 200.}
            LabelValue{label: "build" value: 3. precision: 0 min: 0. max: 0.}
        }

        StoryHeading{text: "Markers"}
        StoryNote{text: "Numbered pins at relative positions over any content. Click one; the caption says which."}
        StoryRow{
            spacing: theme.space_3
            View{
                width: 320.
                height: 160.
                flow: Overlay
                RoundedView{
                    width: Fill
                    height: Fill
                    show_bg: true
                    draw_bg +: {
                        color: theme.color_surface_container_high
                        border_radius: 6.
                    }
                }
                marker_1 := Marker{width: Fill height: Fill x: 0.2 y: 0.3 number: 1}
                marker_2 := Marker{width: Fill height: Fill x: 0.55 y: 0.62 number: 2}
                marker_3 := Marker{width: Fill height: Fill x: 0.8 y: 0.22 number: 3 badge +: {intent: Error}}
                marker_4 := Marker{width: Fill height: Fill x: 0.4 y: 0.85 badge +: {intent: Success}}
            }
            picked := Caption{text: "no marker clicked yet"}
        }
    }

    mod.stories.BadgeBasic = StoryPage{
        StoryNote{text: "One badge; its count, cap, intent and appearance come from the controls."}
        StoryRow{
            subject := Badge{count: 5}
        }
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let anchor = root.badge_anchor(cx, ids!(count_badge));
    if root.button(cx, ids!(inc)).clicked(actions) {
        let n = anchor.count() + 25;
        anchor.set_count(cx, n);
        root.label(cx, ids!(count_label)).set_text(cx, &format!("count {n}"));
    }
    if root.button(cx, ids!(reset)).clicked(actions) {
        anchor.set_count(cx, 0);
        root.label(cx, ids!(count_label)).set_text(cx, "count 0");
    }
    for id in [live_id!(marker_1), live_id!(marker_2), live_id!(marker_3), live_id!(marker_4)] {
        let marker = root.marker(cx, &[id]);
        if marker.clicked(actions) {
            let what = match marker.number() {
                0 => "the dot marker".to_string(),
                n => format!("marker {n}"),
            };
            root.label(cx, ids!(picked)).set_text(cx, &format!("{what} clicked"));
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "data-display/badge/overview",
        category: "Data display",
        component: "Badge",
        also: &["BadgeAnchor", "LabelValue", "Marker", "StatusDot"],
        name: "Overview",
        dsl: "BadgeOverview",
        added: "2026-09-05",
        tags: &["new"],
        doc: "# Badge\n\nA badge says one fact about the thing it sits next to. Empty, it is a dot; with a `count` it shows the number, capped at `max` as `99+`; with a `text` it shows the word. `intent` picks the theme role it is coloured by, `appearance` how loudly (filled, ghost, outline, tint), `size` its rung on the ladder and `shape` its corners. `Badge` carries the theme's bevel stroke, `BadgeFlat` does not.\n\n`BadgeAnchor` wraps any widget and draws its badge on the overlay at a corner, so the wrapped widget keeps its size; it hides at a count of 0 unless the badge is a `dot` or a word. `StatusDot` gives every status a shape as well as a colour. `LabelValue` is a two-tone pill whose value half is coloured by the value's place between `min` and `max`. `Marker` is an anchor positioned by relative coordinates over content, numbered, raising `Clicked`.",
        subject: "count_badge",
        feature: None,
        controls: &[],
        on_actions: Some(overview_actions),
    },
    Story {
        key: "data-display/badge/basic",
        category: "Data display",
        component: "Badge",
        also: &[],
        name: "Basic",
        dsl: "BadgeBasic",
        added: "2026-09-05",
        tags: &["new", "controls"],
        doc: "# Badge\n\nOne badge under the controls: drive the count past the cap, move the cap, and change the intent and the appearance.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Count", target: "subject", kind: ControlKind::Number { prop: "count", min: 0., max: 150., step: 1., default: 5. } },
            Control { label: "Max", target: "subject", kind: ControlKind::Number { prop: "max", min: 1., max: 150., step: 1., default: 99. } },
            Control {
                label: "Intent",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "intent",
                    options: &["Neutral", "Primary", "Secondary", "Tertiary", "Error", "Warning", "Success", "Info"],
                    default: 0,
                },
            },
            Control {
                label: "Appearance",
                target: "subject",
                kind: ControlKind::Choice { prop: "appearance", options: &["Filled", "Ghost", "Outline", "Tint"], default: 0 },
            },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: None,
    },
];
