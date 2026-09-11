//! The list item story: the three shapes, the two slots, the rule between
//! rows, and the one thing a row deliberately refuses to decide.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ListItemOverview = StoryPage{
        StoryNote{text: "One row of a list: something at the leading edge, one to three lines of text, and something at the trailing edge. The parts never differ between two hand-written rows; the height, the two type sizes and where the rule starts always do. Those are what the row owns."}
        StoryHeading{text: "One row, under the controls"}
        StoryNote{text: "One row. The controls write its lines, its shape, the rule under it and its states."}
        subject := ListItem{
            lines: ListItemLines.Three
            text: "A row of a list"
            secondary: "and a line about it"
            tertiary: "and one more line after that"
            trailing: Label{text: "›"}
        }

        StoryHeading{text: "The three shapes"}
        StoryNote{text: "One line, two or three. The shape brings the height and the line count with it, so two lists in one application cannot disagree by four points about how tall a two-line row is."}
        SolidView{
            width: Fill height: Fit flow: Down
            draw_bg +: {color: theme.color_surface_container_low}
            ListItemOne{
                text: "One line"
                trailing: Label{text: "4"}
                divider: ListItemDivider.Inset
            }
            ListItemTwo{
                text: "Two lines"
                secondary: "and a line about it underneath"
                trailing: Label{text: "12"}
                divider: ListItemDivider.Inset
            }
            ListItemThree{
                text: "Three lines"
                secondary: "a line about it"
                tertiary: "and one more line after that"
                trailing: Label{text: "40"}
            }
        }

        StoryHeading{text: "The slots"}
        StoryNote{text: "A slot takes any widget: an avatar, a checkbox, an icon at the leading edge; a value, a chevron, a switch at the trailing one. A press the slot's own control answers never reaches the row, so the switch below flips without opening the row; a press on the avatar, which answers nothing, does open it. The last row has no leading widget at all and holds the column open with leading_width, so its text still lines up with the two above it."}
        SolidView{
            width: Fill height: Fit flow: Down
            draw_bg +: {color: theme.color_surface_container_low}
            ListItemTwo{
                leading: CircleView{
                    width: 28. height: 28.
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {color: theme.color_secondary_container}
                    Label{
                        text: "AB"
                        draw_text +: {color: theme.color_on_secondary_container}
                    }
                }
                text: "A name and a face"
                secondary: "the avatar is a widget in the leading slot"
                trailing: Label{text: "›"}
                divider: ListItemDivider.Text
            }
            ListItemOne{
                leading: CheckBox{}
                text: "A row you tick"
                trailing: Label{text: "›"}
                divider: ListItemDivider.Text
            }
            ListItemTwo{
                leading_width: 28.
                text: "Notifications"
                secondary: "the switch answers the press by itself"
                trailing: Toggle{}
            }
        }

        StoryHeading{text: "The rule between rows"}
        StoryNote{text: "The rule belongs to the row, because only the row knows where its own text starts. Full runs edge to edge, Text starts at the text so it does not cut under the avatars, Inset stands off both edges by the row's padding."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            SolidView{
                width: 300. height: Fit flow: Down
                draw_bg +: {color: theme.color_surface_container_low}
                ListItemOne{
                    leading: CircleView{
                        width: 24. height: 24.
                        draw_bg +: {color: theme.color_secondary_container}
                    }
                    text: "Full"
                    divider: ListItemDivider.Full
                }
                ListItemOne{
                    leading: CircleView{
                        width: 24. height: 24.
                        draw_bg +: {color: theme.color_secondary_container}
                    }
                    text: "cuts under the avatar"
                    divider: ListItemDivider.Full
                }
            }
            SolidView{
                width: 300. height: Fit flow: Down
                draw_bg +: {color: theme.color_surface_container_low}
                ListItemOne{
                    leading: CircleView{
                        width: 24. height: 24.
                        draw_bg +: {color: theme.color_secondary_container}
                    }
                    text: "Text"
                    divider: ListItemDivider.Text
                }
                ListItemOne{
                    leading: CircleView{
                        width: 24. height: 24.
                        draw_bg +: {color: theme.color_secondary_container}
                    }
                    text: "lines up with the words"
                    divider: ListItemDivider.Text
                }
            }
        }

        StoryHeading{text: "States"}
        StoryNote{text: "Chosen, pressed, disabled, and a row that only reads out. A disabled row dims its text and takes no press, and hands the flag to the widgets in its slots as well."}
        SolidView{
            width: Fill height: Fit flow: Down
            draw_bg +: {color: theme.color_surface_container_low}
            ListItemOne{
                text: "Chosen"
                selected: true
                trailing: Label{text: "›"}
            }
            ListItemOne{
                text: "Press and hold me"
                trailing: Label{text: "›"}
            }
            ListItemOne{
                text: "Disabled"
                disabled: true
                trailing: Toggle{}
            }
            ListItemTwo{
                text: "Only reads out"
                secondary: "interactive: false, so it never lights up under the pointer"
                interactive: false
                trailing: Label{text: "3.2 GB"}
            }
        }

        StoryHeading{text: "Who decides which row is chosen"}
        StoryNote{text: "The row reports that it was pressed and changes nothing else. Which row ends up chosen is the list's answer, because only the list knows how many may be chosen at once — here, one."}
        SolidView{
            width: Fill height: Fit flow: Down
            draw_bg +: {color: theme.color_surface_container_low}
            pick_all := ListItemOne{
                text: "Everything"
                selected: true
                divider: ListItemDivider.Inset
            }
            pick_mine := ListItemOne{
                text: "Only mine"
                divider: ListItemDivider.Inset
            }
            pick_none := ListItemOne{
                text: "Nothing"
            }
        }
        StoryRow{
            picked := Label{text: "Everything is chosen"}
        }
    }

}

/// The list the rows belong to. Choosing is done here rather than in the
/// row, which is the widget's whole contract about selection.
fn list_item_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let rows = [
        ("Everything", ids!(pick_all)),
        ("Only mine", ids!(pick_mine)),
        ("Nothing", ids!(pick_none)),
    ];
    let mut chosen = None;
    for (name, id) in rows {
        if root.list_item(cx, id).clicked(actions) {
            chosen = Some(name);
        }
    }
    if let Some(name) = chosen {
        for (row, id) in rows {
            root.list_item(cx, id).set_selected(cx, row == name);
        }
        root.label(cx, ids!(picked))
            .set_text(cx, &format!("{name} is chosen"));
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "data-display/list-item/overview",
        category: "Data display",
        component: "ListItem",
        also: &["ListItemOne", "ListItemTwo", "ListItemThree", "ListItemRuled"],
        name: "Overview",
        dsl: "ListItemOverview",
        added: "2026-09-10",
        tags: &["new", "list"],
        doc: "# ListItem

One row of a list: something at the leading edge, one to three lines of text, and something at the trailing edge.

**The shape decides the height and the line count.** `ListItemOne`, `ListItemTwo` and `ListItemThree` are the three shapes. A row takes its height from its shape, so two lists in one application cannot disagree about how tall a two-line row is, and no caller has to derive the number again.

**The rule belongs to the row.** A rule between rows has to start where the TEXT starts or it cuts under the avatars — and where the text starts depends on the leading slot, which is the caller's widget. A rule written as a sibling has to be told that number, and that number goes stale the day the leading slot changes size. Here it is measured every draw: `divider` is `None`, `Full`, `Text` or `Inset`.

**The gaps go between things that are there.** An empty leading slot does not indent the text, which is what the layout's own spacing would have done to every row in a list that has no leading widgets.

**A press a slot answers is not a press on the row.** The slots see the event first, so a trailing switch or a leading checkbox takes the press; a decorative icon answers nothing and the press opens the row.

**It is not a list.** It does not scroll, does not recycle, and does not decide which row is chosen: `selected` is a flag the list sets and the row draws. It has no keyboard either — one tab stop per row means Tab walks every row of a long list, so the keyboard belongs to whatever owns the rows.",
        subject: "pick_all",
        feature: None,
        controls: &[
            Control { label: "First line", target: "subject", kind: ControlKind::Text { prop: "text", default: "A row of a list" } },
            Control { label: "Second line", target: "subject", kind: ControlKind::Text { prop: "secondary", default: "and a line about it" } },
            Control { label: "Third line", target: "subject", kind: ControlKind::Text { prop: "tertiary", default: "and one more line after that" } },
            Control {
                label: "Lines",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "lines",
                    options: &["ListItemLines.One", "ListItemLines.Two", "ListItemLines.Three"],
                    default: 2,
                },
            },
            Control {
                label: "Divider",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "divider",
                    options: &[
                        "ListItemDivider.None",
                        "ListItemDivider.Full",
                        "ListItemDivider.Text",
                        "ListItemDivider.Inset",
                    ],
                    default: 0,
                },
            },
            Control { label: "Leading width", target: "subject", kind: ControlKind::Number { prop: "leading_width", min: 0., max: 96., step: 1., default: 0. } },
            Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 48., step: 1., default: 12. } },
            Control { label: "Chosen", target: "subject", kind: ControlKind::Bool { prop: "selected", default: false } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(list_item_actions),
    },];
