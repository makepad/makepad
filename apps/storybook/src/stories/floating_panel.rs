//! The floating panel story: a panel you move and size, over a page that
//! keeps working while you do.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.FloatingPanelOverview = StoryPage{
        StoryNote{text: "A panel the person moves and sizes, that does NOT stop the work. That is the whole difference from a dialog, and it decides everything: no scrim, no stolen keyboard, no dismissal when you press elsewhere."}

        StoryHeading{text: "Open one"}
        StoryRow{
            open_panel := Button{text: "Show the inspector"}
            panel_state := Label{text: "closed"}
        }

        StoryHeading{text: "The page keeps working"}
        StoryNote{text: "Press this while the panel is open. A dialog would not let you: that is the point of the difference."}
        StoryRow{
            underneath := Button{text: "Behind the panel"}
            under_note := Label{text: "pressed 0 times"}
        }

        StoryHeading{text: "Where it is"}
        StoryNote{text: "Drag the title bar to move it, the bottom-right corner to size it. It reports where a drag left it, and it cannot be dragged off the screen — a panel whose title bar has gone past an edge could never be dragged back."}
        StoryRow{
            placement := Label{text: "not moved yet"}
        }

        inspector := FloatingPanel{
            title: "Inspector"
            pos: vec2(420., 260.)
            size: vec2(300., 200.)
            content +: {
                body +: {
                    P{text: "Move me by the title bar. Size me by the corner."}
                    P{text: "The page behind stays live the whole time."}
                }
            }
        }
    }
}

fn floating_panel_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let panel = root.floating_panel(cx, ids!(inspector));
    if root.button(cx, ids!(open_panel)).clicked(actions) {
        panel.open(cx);
    }
    if root.button(cx, ids!(underneath)).clicked(actions) {
        let n = crate::stories::bump(live_id!(floating_under));
        root.label(cx, ids!(under_note))
            .set_text(cx, &format!("pressed {n} times"));
    }
    if let Some((pos, size)) = panel.placed(actions) {
        root.label(cx, ids!(placement)).set_text(
            cx,
            &format!("at {:.0},{:.0} sized {:.0}x{:.0}", pos.x, pos.y, size.x, size.y),
        );
    }
    let state = if panel.is_open() { "open" } else { "closed" };
    let label = root.label(cx, ids!(panel_state));
    if label.text() != state {
        label.set_text(cx, state);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/floating-panel/overview",
    category: "Overlay",
    component: "FloatingPanel",
    also: &[],
    name: "Overview",
    dsl: "FloatingPanelOverview",
    added: "2026-09-08",
    tags: &["new"],
    doc: "# FloatingPanel

A panel the person moves and sizes, that does not stop the work.

That last clause is the whole difference from a `Dialog`, and it decides every structural choice. A dialog asks a question and is owed an answer, so it paints a scrim, takes the keyboard, blocks scrolling and goes away when you press outside it. An inspector, a palette, a set of readings you watch while you work is none of those — so this derefs `View` and **not** `Modal`, because every one of Modal's behaviours would be wrong here.

**The title bar is the handle and the bottom-right corner is the grip.** Both are read from what was actually drawn rather than guessed from padding, and both claim a press before the panel's own contents see it — a drag that begins by dropping a caret into a text field is a drag you then have to undo.

**It cannot be dragged off the screen.** Every draw pulls it back inside the pass with `span_inboard`, the same arithmetic every anchored popup uses. A panel whose title bar has gone past an edge can never be dragged back, which would make losing it permanent.

Position is reported, never stored: `placed` says where a drag left it, and a caller that wants it back next run keeps the value itself. A widget that writes files has learned something it has no business knowing.

Promoted from the design overlay's note card, which has carried this mechanism for months; its notes file, session singleton and leader line stayed behind.",
    subject: "inspector",
    feature: None,
    controls: &[],
    on_actions: Some(floating_panel_actions),
}];
