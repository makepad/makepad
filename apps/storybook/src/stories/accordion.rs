//! The accordion stories: which sections have room, and who decides.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Section = FoldHeader{
        width: Fill
        height: Fit
        header := View{
            width: Fill
            height: Fit
            flow: Right
            align: Align{y: 0.5}
            spacing: theme.space_2
            padding: theme.mspace_2
            show_bg: true
            draw_bg +: {color: theme.color_surface_container}
            fold_button := FoldButton{}
            heading := Label{text: "Section"}
        }
        body := View{
            width: Fill
            height: Fit
            flow: Down
            padding: theme.mspace_3
            spacing: theme.space_1
        }
    }

    mod.stories.AccordionOverview = StoryPage{
        StoryNote{text: "A panel decides how many sections have room; the person decides which. Folding one leaves its room empty rather than opening something nobody asked for, and a section that was only crowded out comes back when the room grows."}

        StoryHeading{text: "Room for two of three"}
        StoryRow{
            room_two := Button{text: "Room for 2"}
            room_one := Button{text: "Room for 1"}
            room_all := Button{text: "No limit"}
        }
        StoryRow{
            state := Label{text: "showing: 0, 1"}
        }

        StoryRow{
            panel := Accordion{
                width: 420.
                room: 2
                closed_first: [2]
                Section{
                    header +: {heading +: {text: "Equalizer"}}
                    body +: {P{text: "High, mid, low and the filter."}}
                }
                Section{
                    header +: {heading +: {text: "Stems"}}
                    body +: {P{text: "Drums, bass, vocals and the rest."}}
                }
                Section{
                    header +: {heading +: {text: "Transcript"}}
                    body +: {P{text: "The words, as they are sung. This is the section a short panel can most afford to lose, so it starts closed."}}
                }
            }
        }

        StoryNote{text: "Press a section's mark to fold or open it. With room for one, opening a section closes whichever was open longest ago; the last open one cannot be folded."}

        StoryHeading{text: "Opening on hover"}
        StoryNote{text: "Optional, and off by default: a section opens when the pointer RESTS on its header. The dwell is real, so crossing the panel on the way somewhere else opens nothing, and resting on an open section never folds it."}
        StoryRow{
            hover_panel := AccordionHover{
                width: 420.
                Section{
                    header +: {heading +: {text: "First"}}
                    body +: {P{text: "Rest the pointer on another heading and this one gives way."}}
                }
                Section{
                    header +: {heading +: {text: "Second"}}
                    body +: {P{text: "Opened by the pointer resting on the heading above it."}}
                }
                Section{
                    header +: {heading +: {text: "Third"}}
                    body +: {P{text: "The same, without a press anywhere."}}
                }
            }
        }
        StoryRow{
            hover_state := Label{text: "showing: 0"}
        }
    }
}

fn accordion_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let panel = root.accordion(cx, ids!(panel));
    for (id, room) in [
        (ids!(room_two), 2usize),
        (ids!(room_one), 1),
        (ids!(room_all), 0),
    ] {
        if root.button(cx, id).clicked(actions) {
            panel.set_room(cx, room);
        }
    }
    let showing = panel.showing();
    let text = if showing.is_empty() {
        "showing: nothing".to_string()
    } else {
        format!(
            "showing: {}",
            showing.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
        )
    };
    let label = root.label(cx, ids!(state));
    if label.text() != text {
        label.set_text(cx, &text);
    }

    let hover_panel = root.accordion(cx, ids!(hover_panel));
    let hover_text = format!(
        "showing: {}",
        hover_panel
            .showing()
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let hover_label = root.label(cx, ids!(hover_state));
    if hover_label.text() != hover_text {
        hover_label.set_text(cx, &hover_text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "layout/accordion/overview",
    category: "Layout",
    component: "Accordion",
    name: "Overview",
    dsl: "AccordionOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Accordion\n\nA panel that decides which of its sections have room. The host sets how many may be open, because a panel narrows when the window does rather than because anyone asked; the person chooses which ones fill that room.\n\nThree rules, each learned by getting it wrong first. Folding a section leaves its room **empty**: handing it to a section nobody asked for means the panel answers a fold by opening something else, which then sits there while the sections actually in use trade places underneath it. A section that is wanted but crowded out **comes back on its own** when the room grows, because wanting is remembered apart from showing. And the **last open section cannot be folded**, since a column of headings over dead space is not a state worth reaching.\n\nThe widget only says which sections are open; it never draws one. Any fold header will do, including an app's own.",
    subject: "panel",
    feature: None,
    controls: &[],
    on_actions: Some(accordion_actions),
}];
