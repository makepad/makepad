//! Alerts, banners, guides and callouts: the message that sits in the
//! page. One overview of every shape, a controlled instance, and the host
//! that shows one banner at a time.
use crate::makepad_widgets::alert::AlertIntent;
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
        StoryHeading{text: "One alert, under the controls"}
        StoryNote{text: "One alert. The controls write its title, description, intent, appearance and close cross; the button brings it back after the cross folds it away."}
        subject := Alert{
            title: "Title"
            description: "A description of what happened and what to do about it."
            closable: true
        }

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
        offline := Banner{
            title: "You are offline"
            description: "Changes are kept on this device until the connection is back."
            intent: AlertIntent.Warning
            action: ButtonFlat{text: "Retry"}
            secondary: LinkLabel{text: "Keep working offline"}
        }

        StoryHeading{text: "A banner that waits for its message"}
        StoryNote{text: "BannerHost is a place under a toolbar for the one banner that matters now. It holds a single Banner, folded away and taking no room until show fills it with an intent, a title and a line of detail. A second show replaces the text in place, so only the latest message is ever on screen, and dismiss folds it away again."}
        StoryRow{
            banner_info := Button{text: "Info"}
            banner_warn := Button{text: "Warning"}
            banner_off := Button{text: "Dismiss"}
        }
        banner_host := BannerHost{}

        StoryHeading{text: "Guide"}
        StoryNote{text: "AlertGuide is the alert that teaches rather than reports: folded guidance, a media slot and a dismiss key the host persists."}
        guide := AlertGuide{
            title: "Drag rows to reorder them"
            description: "Every row in this list has a handle at its left edge."
            guidance: "Hold the handle, move the row to where it belongs and let go. Press Escape while dragging to put it back where it was. The order is saved as soon as you drop."
            dismiss_key: "guide.reorder"
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
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let closables = [
        ids!(closable),
        ids!(with_action),
        ids!(offline),
        ids!(guide),
        ids!(callout),
    ];
    if root.button(cx, ids!(reopen)).clicked(actions) {
        for path in closables {
            root.alert(cx, path).open(cx);
        }
        root.alert(cx, ids!(subject)).open(cx);
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
    if root.alert(cx, ids!(offline)).action(actions) {
        root.alert(cx, ids!(offline)).close(cx);
    }
}

fn banner_host_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let host = root.banner_host(cx, ids!(banner_host));
    if root.button(cx, ids!(banner_info)).clicked(actions) {
        host.show(cx, AlertIntent::Info, "Rendering", "Three of nine frames are done.");
    }
    if root.button(cx, ids!(banner_warn)).clicked(actions) {
        host.show(cx, AlertIntent::Warning, "Disconnected", "The device stopped answering.");
    }
    if root.button(cx, ids!(banner_off)).clicked(actions) {
        host.dismiss(cx);
    }
}

/// The page's one handler: the alerts, then the banner host.
fn alert_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    overview_actions(cx, root, actions);
    banner_host_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/alert/overview",
        category: "Feedback",
        component: "Alert",
        also: &["Banner", "Callout", "AlertGuide", "BannerHost"],
        name: "Overview",
        dsl: "AlertOverview",
        added: "2026-09-05",
        tags: &["controls", "new"],
        doc: "# Alert\n\nA message that sits in the page: an intent icon, a bold title, a description, an action slot and a close cross on a face coloured by intent. `Alert` is the bevelled standard, `AlertFlat` the plain face, `Banner` the full-width strip, `AlertGuide` the guide with folded guidance and a media slot, `Callout` the card nudge with an accent bar.\n\nClosing folds the alert away over `motion_medium_1` and raises `Closed`; a guide with a `dismiss_key` also raises `Dismissed(key)`. A click on the widget in an action slot raises `Action`. `open` brings a closed alert back.\n\nWhile the title, the description and the actions fit on one line they share it; when the width shrinks the text stacks and the actions drop under it.\n\n## BannerHost\n\n`BannerHost` is the place under a toolbar for the one banner that matters now. It holds a single `Banner`, folded away and taking no room until `show(cx, intent, title, description)` fills and unfolds it. A second `show` replaces the text in place, so only the latest message is ever on screen; `dismiss` folds it away and `is_showing` asks. `banner()` hands over the inner banner for a host that reads its actions, and the host's `banner` property is where the banner is styled.",
        subject: "closable",
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
        on_actions: Some(alert_page_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page reads without a script error and holds every alert its
    /// handler addresses. Nothing the compiler checks reads the DSL: a
    /// preset the page names and the library no longer registers is one log
    /// line and a gap where the guide was meant.
    #[test]
    fn the_page_builds_and_holds_the_alerts_its_handler_addresses() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let errors = cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
            vm.take_errors()
        });
        assert!(errors.is_empty(), "{errors:?}");
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        for path in [ids!(subject), ids!(closable), ids!(with_action), ids!(offline), ids!(guide), ids!(callout)] {
            assert!(page.alert(&cx, path).borrow().is_some(), "{}: no alert {path:?}", story.key);
        }
        assert!(page.banner_host(&cx, ids!(banner_host)).borrow().is_some(), "{}: no banner host", story.key);
    }
}
