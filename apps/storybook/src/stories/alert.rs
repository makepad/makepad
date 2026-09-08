//! Alerts, banners, inline tips and callouts: the message that sits in the
//! page. One overview of every shape and a controlled instance.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Reflowing = Alert{
        title: "Update ready"
        description: "Version 2.4 has been downloaded."
        intent: AlertIntent.Success
        closable: true
        action: ButtonFlat{text: "Install now"}
    }

    mod.stories.AlertOverview = StoryPage{
        StoryHeading{text: "Four intents, three appearances"}
        StoryNote{text: "The intent picks the palette; the appearance decides how the face wears it. Light tints, Filled paints, Outline strokes."}
        Alert{title: "Information", description: "A neutral note about the state of things.", intent: AlertIntent.Info}
        Alert{title: "Saved", description: "Your changes are on the server.", intent: AlertIntent.Success}
        Alert{title: "Check the date", description: "The end date is before the start date.", intent: AlertIntent.Warning}
        Alert{title: "Upload failed", description: "The file could not be sent. Try again.", intent: AlertIntent.Error}
        StoryNote{text: "Filled"}
        AlertFlat{title: "Information", description: "A neutral note about the state of things.", intent: AlertIntent.Info, appearance: AlertAppearance.Filled}
        AlertFlat{title: "Saved", description: "Your changes are on the server.", intent: AlertIntent.Success, appearance: AlertAppearance.Filled}
        AlertFlat{title: "Check the date", description: "The end date is before the start date.", intent: AlertIntent.Warning, appearance: AlertAppearance.Filled}
        AlertFlat{title: "Upload failed", description: "The file could not be sent. Try again.", intent: AlertIntent.Error, appearance: AlertAppearance.Filled}
        StoryNote{text: "Outline"}
        AlertFlat{title: "Information", description: "A neutral note about the state of things.", intent: AlertIntent.Info, appearance: AlertAppearance.Outline}
        AlertFlat{title: "Saved", description: "Your changes are on the server.", intent: AlertIntent.Success, appearance: AlertAppearance.Outline}
        AlertFlat{title: "Check the date", description: "The end date is before the start date.", intent: AlertIntent.Warning, appearance: AlertAppearance.Outline}
        AlertFlat{title: "Upload failed", description: "The file could not be sent. Try again.", intent: AlertIntent.Error, appearance: AlertAppearance.Outline}

        StoryHeading{text: "Title only, closable, with an action"}
        title_only := Alert{title: "A title on its own"}
        closable := Alert{title: "Closable", description: "The cross folds this away; the button below brings it back.", intent: AlertIntent.Warning, closable: true}
        with_action := Alert{title: "Update ready", description: "Version 2.4 has been downloaded.", intent: AlertIntent.Info, closable: true, action: ButtonFlat{text: "Install now"}}
        StoryRow{
            reopen := ButtonFlat{text: "Show the closed ones again"}
            closed_count := Label{text: "nothing closed yet"}
        }

        StoryHeading{text: "Banner"}
        StoryNote{text: "Full width, square corners, one or two actions and no cross: it stays until one of them is taken."}
        banner := Banner{
            title: "You are offline"
            description: "Changes are kept on this device until the connection is back."
            intent: AlertIntent.Warning
            action: ButtonFlat{text: "Retry"}
            secondary: LinkLabel{text: "Keep working offline"}
        }

        StoryHeading{text: "Inline tip"}
        StoryNote{text: "A guide banner: folded guidance, a media slot and a dismiss key the host persists."}
        tip := InlineTip{
            title: "Drag rows to reorder them"
            description: "Every row in this list has a handle at its left edge."
            guidance: "Hold the handle, move the row to where it belongs and let go. Press Escape while dragging to put it back where it was. The order is saved as soon as you drop."
            dismiss_key: "tip.reorder"
            media: RoundedView{
                width: 56.
                height: 40.
                show_bg: true
                draw_bg +: {
                    color: theme.color_info_container
                    border_radius: 4.
                }
            }
            action: LinkLabel{text: "Learn more"}
        }

        StoryHeading{text: "Callout"}
        StoryNote{text: "The card nudge: an accent bar, a title, an action and a cross."}
        callout := Callout{
            title: "Try the new editor"
            description: "It folds sections, shows a minimap and keeps your place between sessions."
            intent: AlertIntent.Success
            action: ButtonFlat{text: "Open it"}
        }

        StoryHeading{text: "Reflow"}
        StoryNote{text: "The same alert at three widths. The action stays on the line while it fits and drops under the text when it does not."}
        View{width: 640. height: Fit  Reflowing{}}
        View{width: 440. height: Fit  Reflowing{}}
        View{width: 300. height: Fit  Reflowing{}}
    }

    mod.stories.AlertBasic = StoryPage{
        StoryNote{text: "One alert. The controls write its title, description, intent, appearance and close cross; the button brings it back after the cross folds it away."}
        subject := Alert{
            title: "Title"
            description: "A description of what happened and what to do about it."
            closable: true
        }
        StoryRow{
            reopen := ButtonFlat{text: "Show again"}
        }
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let closables = [
        ids!(closable),
        ids!(with_action),
        ids!(banner),
        ids!(tip),
        ids!(callout),
    ];
    if root.button(cx, ids!(reopen)).clicked(actions) {
        for path in closables {
            root.alert(cx, path).open(cx);
        }
    }
    for path in closables {
        let alert = root.alert(cx, path);
        if alert.closed(actions) {
            let n = crate::stories::bump(live_id!(alert_overview_closed));
            let key = alert.dismissed(actions).unwrap_or_default();
            let text = if key.is_empty() {
                format!("closed {n}")
            } else {
                format!("closed {n}, dismiss key {key}")
            };
            root.label(cx, ids!(closed_count)).set_text(cx, &text);
        }
    }
    if root.alert(cx, ids!(banner)).action(actions) {
        root.alert(cx, ids!(banner)).close(cx);
    }
}

fn basic_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(reopen)).clicked(actions) {
        root.alert(cx, ids!(subject)).open(cx);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/alert/overview",
        category: "Feedback",
        component: "Alert",
        also: &[],
        name: "Overview",
        dsl: "AlertOverview",
        added: "2026-09-05",
        tags: &["new"],
        doc: "# Alert\n\nA message that sits in the page: an intent icon, a bold title, a description, an action slot and a close cross on a face coloured by intent. `Alert` is the bevelled standard, `AlertFlat` the plain face, `Banner` the full-width strip, `InlineTip` the guide banner with folded guidance and a media slot, `Callout` the card nudge with an accent bar.\n\nClosing folds the alert away over `motion_medium_1` and raises `Closed`; a tip with a `dismiss_key` also raises `Dismissed(key)`. A click on the widget in an action slot raises `Action`. `open` brings a closed alert back.\n\nWhile the title, the description and the actions fit on one line they share it; when the width shrinks the text stacks and the actions drop under it.",
        subject: "closable",
        feature: None,
        controls: &[],
        on_actions: Some(overview_actions),
    },
    Story {
        key: "feedback/alert/basic",
        category: "Feedback",
        component: "Alert",
        also: &[],
        name: "Basic",
        dsl: "AlertBasic",
        added: "2026-09-05",
        tags: &["new", "controls"],
        doc: "# Alert\n\nOne alert under the controls: title, description, intent, appearance, the close cross and the disabled dimming.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Title", target: "subject", kind: ControlKind::Text { prop: "title", default: "Title" } },
            Control {
                label: "Description",
                target: "subject",
                kind: ControlKind::Text { prop: "description", default: "A description of what happened and what to do about it." },
            },
            Control {
                label: "Intent",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "intent",
                    options: &["AlertIntent.Info", "AlertIntent.Success", "AlertIntent.Warning", "AlertIntent.Error"],
                    default: 0,
                },
            },
            Control {
                label: "Appearance",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "appearance",
                    options: &["AlertAppearance.Light", "AlertAppearance.Filled", "AlertAppearance.Outline"],
                    default: 0,
                },
            },
            Control { label: "Closable", target: "subject", kind: ControlKind::Bool { prop: "closable", default: true } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(basic_actions),
    },
];
