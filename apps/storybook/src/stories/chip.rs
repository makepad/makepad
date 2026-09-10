//! The chip stories: the ladder, the roles, the two behaviours, and one
//! controlled chip.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ChipOverview = StoryPage{
        StoryNote{text: "A pill standing for one thing the user chose: a filter that is on, a recipient in a field, a label on a record. Selectable chips toggle and carry a tick; removable chips carry a cross with its own hit area; a Tag answers nothing at all."}
        StoryHeading{text: "One chip, under the controls"}
        StoryNote{text: "One chip. Every control on the right writes into it."}
        StoryRow{
            subject := Chip{text: "Chip" selectable: true}
        }
        StoryRow{
            state := Label{text: "not pressed yet"}
        }

        StoryHeading{text: "Style ladder"}
        StoryRow{
            ChipFlat{text: "ChipFlat"}
            Chip{text: "Chip"}
            ChipGradientX{text: "GradientX" appearance: Filled intent: Primary}
            ChipGradientY{text: "GradientY" appearance: Filled intent: Primary}
        }

        StoryHeading{text: "Appearance"}
        StoryRow{
            Chip{text: "Filled" appearance: Filled intent: Primary}
            Chip{text: "Tonal" appearance: Tonal intent: Primary}
            Chip{text: "Outline" appearance: Outline intent: Primary}
            Chip{text: "Ghost" appearance: Ghost intent: Primary}
        }

        StoryHeading{text: "Roles"}
        StoryRow{
            Chip{text: "Neutral" intent: Neutral}
            Chip{text: "Primary" intent: Primary}
            Chip{text: "Secondary" intent: Secondary}
            Chip{text: "Tertiary" intent: Tertiary}
        }
        StoryRow{
            Chip{text: "Error" intent: Error}
            Chip{text: "Warning" intent: Warning}
            Chip{text: "Success" intent: Success}
            Chip{text: "Info" intent: Info}
        }

        StoryHeading{text: "Sizes"}
        StoryRow{
            Chip{text: "Small" size: Small}
            Chip{text: "Medium" size: Medium}
            Chip{text: "Large" size: Large}
        }

        StoryHeading{text: "Choosing"}
        StoryNote{text: "A selectable chip toggles on the press and shows a tick; the caption follows the ones that are on."}
        StoryRow{
            filter_new := Chip{text: "New" selectable: true intent: Primary}
            filter_open := Chip{text: "Open" selectable: true selected: true intent: Primary}
            filter_done := Chip{text: "Done" selectable: true intent: Primary}
        }
        StoryRow{
            chosen := Label{text: "on: Open"}
        }

        StoryHeading{text: "Removing"}
        StoryNote{text: "The cross is its own target: pressing it asks the host to take the chip away, pressing the body does not."}
        StoryRow{
            recipient := Chip{text: "ada@example.com" removable: true appearance: Outline}
            recipient_2 := Chip{text: "grace@example.com" removable: true appearance: Outline}
        }
        StoryRow{
            removed := Label{text: "nothing removed yet"}
        }

        StoryHeading{text: "Groups"}
        StoryNote{text: "A group is one tab stop: Tab reaches it, the arrows walk it and show where they are standing, Return chooses, Delete asks a removable chip to go. The arrows step past a chip that is switched off, because the ring would otherwise promise a Return that does nothing. A Single group puts the others back when one is chosen."}
        StoryRow{
            single := ChipGroup{
                width: Fit
                selection: Single
                ChipFlat{text: "Day" selectable: true selected: true appearance: Outline}
                ChipFlat{text: "Week" selectable: true appearance: Outline}
                ChipFlat{text: "Month" selectable: true appearance: Outline}
                ChipFlat{text: "Year" selectable: true appearance: Outline disabled: true}
            }
        }
        StoryRow{
            single_note := Label{text: "period: Day"}
        }
        StoryNote{text: "A filter summary holds what a filter is currently made of, and drops the lot."}
        StoryRow{
            summary := FilterSummary{
                width: Fit
                f_status := ChipFlat{text: "status: open" removable: true}
                f_owner := ChipFlat{text: "owner: me" removable: true}
                f_label := ChipFlat{text: "label: bug" removable: true}
                clear := LinkLabel{text: "Clear all"}
            }
        }
        StoryRow{
            summary_note := Label{text: "three filters"}
        }

        StoryHeading{text: "Tags"}
        StoryNote{text: "A tag states a fact and answers nothing: no hover, no press, no focus."}
        StoryRow{
            Tag{text: "rust"}
            Tag{text: "shader" intent: Info}
            Tag{text: "deprecated" intent: Warning}
            Tag{text: "broken" intent: Error}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            Chip{text: "Filled" appearance: Filled intent: Primary disabled: true}
            Chip{text: "Tonal" appearance: Tonal intent: Primary disabled: true}
            Chip{text: "Outline" appearance: Outline disabled: true}
            Chip{text: "Chosen" selectable: true selected: true disabled: true}
        }
    }
}

fn chip_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let filters = [
        (ids!(filter_new), "New"),
        (ids!(filter_open), "Open"),
        (ids!(filter_done), "Done"),
    ];
    let mut changed = false;
    for (id, _) in filters {
        if root.chip(cx, id).toggled(actions).is_some() {
            changed = true;
        }
    }
    if changed {
        let on: Vec<&str> = filters
            .iter()
            .filter(|(id, _)| root.chip(cx, *id).selected())
            .map(|(_, name)| *name)
            .collect();
        let text = if on.is_empty() {
            "on: nothing".to_string()
        } else {
            format!("on: {}", on.join(", "))
        };
        root.label(cx, ids!(chosen)).set_text(cx, &text);
    }
    let single = root.chip_group(cx, ids!(single));
    if single.changed(actions) {
        let chosen = single.chosen();
        let text = chosen.first().map(|s| format!("period: {s}")).unwrap_or_else(|| "period: none".to_string());
        root.label(cx, ids!(single_note)).set_text(cx, &text);
    }
    // Each filter reports its own removal, whether it was the cross, the
    // Delete key or the clear link that asked, so one loop covers all three.
    let filters_in_summary = [ids!(f_status), ids!(f_owner), ids!(f_label)];
    let mut any_removed = false;
    for id in filters_in_summary {
        let chip = root.chip(cx, id);
        if chip.removed(actions) {
            chip.set_visible(cx, false);
            any_removed = true;
        }
    }
    if any_removed {
        let left = filters_in_summary
            .iter()
            .filter(|id| root.chip(cx, **id).visible())
            .count();
        let text = match left {
            0 => "no filters".to_string(),
            1 => "one filter".to_string(),
            n => format!("{n} filters"),
        };
        root.label(cx, ids!(summary_note)).set_text(cx, &text);
    }
    for (id, name) in [(ids!(recipient), "ada@example.com"), (ids!(recipient_2), "grace@example.com")] {
        let chip = root.chip(cx, id);
        if chip.removed(actions) {
            chip.set_visible(cx, false);
            root.label(cx, ids!(removed)).set_text(cx, &format!("removed {name}"));
        }
    }
    chip_subject_actions(cx, root, actions);
}

fn chip_subject_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.chip(cx, ids!(subject));
    if let Some(on) = subject.toggled(actions) {
        let text = if on { "chosen" } else { "not chosen" };
        root.label(cx, ids!(state)).set_text(cx, text);
    }
    if subject.removed(actions) {
        root.label(cx, ids!(state)).set_text(cx, "the cross was pressed");
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "inputs/chip/overview",
        category: "Inputs",
        component: "Chip",
        also: &["ChipGroup", "FilterSummary", "Tag"],
        name: "Overview",
        dsl: "ChipOverview",
        added: "2026-09-05",
        tags: &["controls", "new"],
        doc: "# Chip\n\nA chip stands for one thing the user chose, typed or was given. It is a noun, not a verb: the press toggles whether the thing is there rather than making something happen.\n\n`selectable` makes the press a toggle and gives the chosen chip a tick, which is what a filter is. `removable` puts a cross at the trailing edge with its own hit area, which is what a recipient in a field is. `Tag` is neither: `interactive: false`, so it takes no hover, no press and no focus.\n\nThe roles and their colours are the badge's, so a chip and a badge that both mean \"error\" are the same red. `appearance` decides how loudly the role is spoken: `Filled`, `Tonal`, `Outline` or `Ghost`. There is no elevated appearance on purpose — a shadow is a fact about the surface a chip sits on, so an elevated row of chips is a chip row inside an `ElevatedView1`.\n\nHover and press are drawn as state layers at `theme.state_hover_opacity` and `theme.state_press_opacity`, the same arithmetic every other control uses.",
        subject: "filter_open",
        feature: None,
        controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Chip" } },
            Control { label: "Intent", target: "subject", kind: ControlKind::Choice { prop: "intent", options: &["Neutral", "Primary", "Secondary", "Tertiary", "Error", "Warning", "Success", "Info"], default: 0 } },
            Control { label: "Appearance", target: "subject", kind: ControlKind::Choice { prop: "appearance", options: &["Filled", "Tonal", "Outline", "Ghost"], default: 1 } },
            Control { label: "Size", target: "subject", kind: ControlKind::Choice { prop: "size", options: &["Small", "Medium", "Large"], default: 1 } },
            Control { label: "Selectable", target: "subject", kind: ControlKind::Bool { prop: "selectable", default: true } },
            Control { label: "Removable", target: "subject", kind: ControlKind::Bool { prop: "removable", default: false } },
            Control { label: "Radius", target: "subject", kind: ControlKind::Number { prop: "radius", min: 0., max: 999., step: 1., default: 999. } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(chip_actions),
    },
];
