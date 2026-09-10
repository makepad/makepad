//! The progress family: bars, rings, arcs, activity rings, gauges and the
//! navigation line, plus one controlled bar.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ProgressOverview = StoryPage{
        StoryNote{text: "Every shape of progress. The first bar advances a tenth per click; the rest are set values, an unknown one, the intents, labels, stacked segments, then rings, an arc, activity rings, gauges and the navigation line."}

        StoryHeading{text: "Determinate"}
        StoryRow{
            bar := ProgressBar{width: 260. value: 0.0 show_percent: true}
            advance := Button{text: "Advance"}
        }

        StoryHeading{text: "Style ladder"}
        StoryRow{
            Label{width: 150. text: "ProgressBarFlat"}
            ProgressBarFlat{width: 260. value: 0.65}
        }
        StoryRow{
            Label{width: 150. text: "ProgressBar"}
            ProgressBar{width: 260. value: 0.65}
        }
        StoryRow{
            Label{width: 150. text: "ProgressBarGradientX"}
            ProgressBarGradientX{width: 260. value: 0.65}
        }
        StoryRow{
            Label{width: 150. text: "ProgressBarGradientY"}
            ProgressBarGradientY{width: 260. value: 0.65}
        }

        StoryHeading{text: "Indeterminate"}
        StoryRow{
            ProgressBar{width: 260. value: -1.0}
            ProgressBar{width: 160. value: -1.0 intent: Info draw_bg.thickness: 4.0}
        }

        StoryHeading{text: "Intents"}
        StoryRow{
            ProgressBar{width: 100. value: 0.7 intent: Primary}
            ProgressBar{width: 100. value: 0.7 intent: Success}
            ProgressBar{width: 100. value: 0.7 intent: Warning}
            ProgressBar{width: 100. value: 0.7 intent: Error}
            ProgressBar{width: 100. value: 0.7 intent: Info}
            ProgressBar{width: 100. value: 0.7 disabled: true}
        }

        StoryHeading{text: "With a label"}
        StoryRow{
            ProgressBar{width: 200. value: 0.42 show_percent: true}
            ProgressBar{width: 200. value: 0.42 text: "3 of 7" label_width: 44.}
        }

        StoryHeading{text: "Stop indicator, gap and thickness"}
        StoryRow{
            ProgressBar{width: 200. value: 0.42 draw_bg.gap: 4.0 draw_bg.stop_indicator: 1.0}
            ProgressBar{width: 200. height: 16. value: 0.42 draw_bg.thickness: 16.0 draw_bg.gap: 4.0 draw_bg.stop_indicator: 1.0}
            ProgressBar{width: 200. value: 0.42 draw_bg.thickness: 2.0}
        }

        StoryHeading{text: "Segments"}
        StoryRow{
            ProgressBar{width: 300. height: 12. draw_bg.thickness: 12.0 segments: [0.35, 0.2, 0.15]}
            ProgressBar{width: 300. height: 12. draw_bg.thickness: 12.0 draw_bg.gap: 3.0 segments: [0.5, 0.25, 0.1, 0.05]}
        }

        StoryHeading{text: "Rings"}
        StoryRow{
            ProgressRingFlat{value: 0.65 show_percent: true}
            ProgressRing{value: 0.65 show_percent: true}
            ProgressRing{value: 0.4 sections: 5 draw_bg.thickness: 6.0}
            ProgressRing{value: -1.0}
            ProgressRing{value: 0.3 intent: Warning draw_bg.rounded_caps: 0.0}
            ProgressRing{width: 72. height: 72. value: 0.8 intent: Success draw_bg.thickness: 10.0 text: "8/10"}
        }

        StoryHeading{text: "Arc"}
        StoryRow{
            ProgressArc{value: 0.72 show_percent: true}
            ProgressArc{value: 0.72 text: "72 MB" intent: Info draw_bg.direction: -1.0}
            ProgressArc{value: -1.0}
        }

        StoryHeading{text: "Activity rings"}
        StoryRow{
            ActivityRings{values: [0.8, 0.55, 0.3]}
            ActivityRings{width: 100. height: 100. values: [1.0, 0.7, 0.45] draw_bg.thickness: 10.0}
        }

        StoryHeading{text: "Gauge"}
        StoryRow{
            Gauge{value: 42.0 unit: "%"}
            Gauge{value: 88.0 max: 120.0 unit: " C" draw_bg.zone_warn: 0.5 draw_bg.zone_critical: 0.75}
            Gauge{value: 30.0 draw_bg.needle: 0.0 draw_bg.band: 0.6 text: "idle"}
            GaugeLinear{width: 240. value: 68.0 unit: " kW"}
        }

        StoryHeading{text: "Navigation progress"}
        StoryNote{text: "Start puts the line on screen and it trickles toward the end; Complete finishes and fades it."}
        // Above the buttons, not below them: as the last child of the page the
        // hairline sat past the bottom edge, so the one thing this section is
        // about could not be seen at all.
        nav := NavigationProgress{}
        StoryRow{
            nav_start := Button{text: "Start"}
            nav_step := Button{text: "Step"}
            nav_done := Button{text: "Complete"}
            nav_reset := Button{text: "Reset"}
        }
    

        StoryHeading{text: "One bar, under the controls"}
        StoryNote{text: "One bar; its value, intent, thickness and label come from the controls."}
        StoryRow{
            subject := ProgressBar{width: 260. value: 0.35 show_percent: true}
        }
    }
}

fn progress_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(advance)).clicked(actions) {
        root.progress_bar(cx, ids!(bar)).advance(cx, 0.1);
    }
    let nav = root.navigation_progress(cx, ids!(nav));
    if root.button(cx, ids!(nav_start)).clicked(actions) {
        nav.start(cx);
    }
    if root.button(cx, ids!(nav_step)).clicked(actions) {
        nav.increment(cx, 0.2);
    }
    if root.button(cx, ids!(nav_done)).clicked(actions) {
        nav.complete(cx);
    }
    if root.button(cx, ids!(nav_reset)).clicked(actions) {
        nav.reset(cx);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/progress/overview",
        category: "Feedback",
        component: "Progress",
        also: &["ActivityRings", "Gauge", "GaugeLinear", "NavigationProgress", "ProgressArc", "ProgressBar", "ProgressRing"],
        name: "Overview",
        dsl: "ProgressOverview",
        added: "2026-09-05",
        tags: &["controls", "new"],
        doc: "# Progress\n\nEvery widget that answers \"how far along is it\". They share one contract: `value` is a fraction 0..1 that eases into place over `theme.motion_medium_1`, only ever forward (a lower value snaps, a negative one is indeterminate), and `Completed` is raised when the value reaches the end.\n\n- `ProgressBarFlat` is the default; `ProgressBar` adds the inset bevel, `ProgressBarGradientX/Y` shade the fill. `show_percent` or `text` puts a label beside the bar; `segments` stacks sections; `draw_bg.gap` and `draw_bg.stop_indicator` are the track details.\n- `ProgressRingFlat`/`ProgressRing` fill clockwise from the top with the label in the middle; `sections` cuts the ring. `ProgressArc` is half a ring with the label inside. `ActivityRings` nests one ring per entry of `values`.\n- `Gauge` is a read-only dial with safe, warning and critical zones and a needle; `GaugeLinear` lays the same zones along a bar.\n- `NavigationProgress` is the hairline at the top of a page: `start`, `increment`, `complete`, `reset`.",
        subject: "bar",
        feature: None,
        controls: &[
            Control { label: "Value", target: "subject", kind: ControlKind::Number { prop: "value", min: 0., max: 1., step: 0.01, default: 0.35 } },
            Control { label: "Intent", target: "subject", kind: ControlKind::Choice { prop: "intent", options: &["Primary", "Success", "Warning", "Error", "Info"], default: 0 } },
            Control { label: "Thickness", target: "subject", kind: ControlKind::Number { prop: "draw_bg.thickness", min: 1., max: 24., step: 0.5, default: 8. } },
            Control { label: "Percent label", target: "subject", kind: ControlKind::Bool { prop: "show_percent", default: true } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(progress_actions),
    },
];
