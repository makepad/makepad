//! The modal story: an overlay that takes the whole window and none of the
//! layout.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Card = View{
        width: 320. height: Fit
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_3
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_high}
    }

    mod.stories.ModalOverview = StoryPage{
        StoryNote{text: "A dimming layer over the whole window with something on top of it. Twelve places in this repository use one, and it is the container under the dialog and the command palette."}

        StoryHeading{text: "Open one"}
        StoryNote{text: "It reports when it was dismissed, so a host can put its own state back. Escape closes it, and so does a press on the dark ground outside the card."}
        StoryRow{
            open_plain := Button{text: "open"}
            plain_state := Label{text: "closed"}
        }

        StoryHeading{text: "One that has to be answered"}
        StoryNote{text: "can_dismiss: false takes away Escape and the press outside. The only way out is a control inside the card, which is what you want when the question has consequences and what you must NOT use when it does not."}
        StoryRow{
            open_firm := Button{text: "open the firm one"}
            firm_state := Label{text: "closed"}
        }

        StoryHeading{text: "A modal takes no room"}
        StoryNote{text: "Both modals are declared inside the row below, between the two chips. They claim no width and no height in their parent, so the row is laid out as though they were not there — the chips sit side by side. A modal that asked for Fill would quietly take a share of every spare point in the row whether or not it was ever opened."}
        StoryRow{
            Chip{text: "before"}
            plain := Modal{
                content: Card{
                    Label{text: "A plain modal"}
                    Label{text: "Escape, or press the dark ground." draw_text +: {color: theme.color_text_meta}}
                }
            }
            firm := Modal{
                can_dismiss: false
                content: Card{
                    Label{text: "This one has to be answered"}
                    Label{text: "Escape does nothing here." draw_text +: {color: theme.color_text_meta}}
                    close_firm := Button{text: "close"}
                }
            }
            Chip{text: "after"}
        }
    }
}

fn modal_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let plain = root.modal(cx, ids!(plain));
    let firm = root.modal(cx, ids!(firm));

    if root.button(cx, ids!(open_plain)).clicked(actions) {
        plain.open(cx);
    }
    if root.button(cx, ids!(open_firm)).clicked(actions) {
        firm.open(cx);
    }
    if root.button(cx, ids!(close_firm)).clicked(actions) {
        firm.close(cx);
    }

    // Asked every pass rather than tracked: a modal closes for reasons the
    // page never hears about — Escape, a press on the ground, the back
    // gesture — and a label that only listened would go on saying open.
    let plain_text = if plain.is_open() { "open" } else { "closed" };
    let label = root.label(cx, ids!(plain_state));
    if label.text() != plain_text {
        label.set_text(cx, plain_text);
    }
    let firm_text = if firm.is_open() { "open" } else { "closed" };
    let label = root.label(cx, ids!(firm_state));
    if label.text() != firm_text {
        label.set_text(cx, firm_text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/modal/overview",
    category: "Overlay",
    component: "Modal",
    also: &[],
    name: "Overview",
    dsl: "ModalOverview",
    added: "2026-09-08",
    tags: &["new", "overlay"],
    doc: "# Modal

A dimming layer over the whole window with `content` on top of it. It is the container the dialog and the command palette are built on, and twelve places in this repository use one.

`open` and `close` drive it, `is_open` asks, and it raises `Dismissed` when it was closed by the person rather than by the host. Ask rather than track: it closes for reasons a page never hears about, and a host that only listened would go on believing it was open.

`can_dismiss` is `true` by default, which gives you Escape, a press on the ground outside the content, and the platform's back gesture. Setting it `false` removes all three at once, so the only way out is a control you put inside — right for a question with consequences, wrong for anything else, because a person who cannot leave will try the window's close box instead.

**A modal claims no room.** It is an overlay: the walk it reports to its parent is empty, and the overlay itself is sized by the pass rather than by the layout around it. That is why it can be declared in the middle of a row without moving anything in that row. It is also load-bearing — giving a modal `width: Fill` makes it a *deferred fill* of its parent, taking a share of the parent's spare length whether or not it is ever opened.",
    subject: "plain",
    feature: None,
    controls: &[],
    on_actions: Some(modal_actions),
}];
