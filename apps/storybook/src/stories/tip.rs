//! The tooltip story: one tip around a control, its pointer, what it may
//! carry, how it is reached without a pointer, and the two tips a caller
//! places by hand.
use crate::makepad_widgets::callout_tooltip::CalloutTooltipOptions;
use crate::makepad_widgets::tooltip::TooltipPosition;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TipOverview = StoryPage{
        StoryNote{text: "A tooltip is a wrapper, not a call site: put a Tip around a control and the window's one TipLayer does the dwell, the placement and the chrome. Hover a button below, or Tab to one."}

        StoryHeading{text: "A tip around a control"}
        StoryRow{
            subject := Tip{text: "Saves the file where it was opened from" Button{text: "Save"}}
        }
        StoryNote{text: "place takes the twelve placements a popover takes, through the same helper: the wanted side, flipped when there is no room, shifted to stay in the window. Overlay > Popover draws all twelve; a tip's Bottom, Top, Left and Right are the centred ones a popover calls BottomCenter and so on."}

        StoryHeading{text: "With a pointer"}
        StoryNote{text: "The arrow sits on the bubble's anchor-facing edge, at the point the placement worked out, so it keeps aiming at the control after a flip."}
        StoryRow{
            Tip{text: "Aimed at this one" arrow: true Button{text: "Arrow"}}
            Tip{text: "Aimed from above" arrow: true place: Top Button{text: "Arrow, above"}}
        }
        StoryNote{text: "When the control is wider than the tip, the point would sit on a rounded corner, so it stops at the flat part of the edge instead of straddling it."}
        StoryRow{
            Tip{text: "Short" arrow: true place: BottomStart Button{width: 260. text: "Wider than its own tip"}}
        }
        StoryNote{text: "A tip beside a control turns its pointer a quarter turn, so it still aims at the thing it belongs to."}
        StoryRow{
            Tip{text: "Aimed from the left" arrow: true place: Left Button{text: "Arrow, left"}}
            Tip{text: "Aimed from the right" arrow: true place: Right Button{text: "Arrow, right"}}
        }

        StoryHeading{text: "Room for a sentence"}
        StoryNote{text: "A wrap width turns a phrase into a paragraph, for the tip that has to explain rather than name."}
        StoryRow{
            TipRich{
                text: "Rendering writes every frame to disk as a PNG. It is slower than playback and needs room for the whole sequence."
                Button{text: "Render"}
            }
        }

        StoryHeading{text: "Roles"}
        StoryNote{text: "A tip can carry a role when it is saying something is wrong rather than naming a control."}
        StoryRow{
            TipError{text: "This file no longer exists" Button{text: "Broken link"}}
            TipWarning{text: "Unsaved changes will be lost" Button{text: "Discard"}}
        }

        StoryHeading{text: "Timing"}
        StoryNote{text: "The default dwell is half a second, and moving along a row of tipped controls follows instantly. A tip can ask for its own."}
        StoryRow{
            Tip{text: "No dwell at all" delay_secs: 0. Button{text: "Instant"}}
            Tip{text: "A long wait" delay_secs: 1.5 Button{text: "Slow"}}
        }

        StoryHeading{text: "Without a pointer"}
        StoryNote{text: "Tab into the field below: its tip appears on the focus, and Escape takes it down."}
        StoryRow{
            Tip{text: "The name people will see" TextInput{width: 200. empty_text: "Display name"}}
        }

        StoryHeading{text: "A tooltip you place"}
        StoryNote{text: "Tooltip takes a position in the window and a string, and appears there pointing at nothing. It suits a label that follows the pointer, where the caller already knows the position. Here the caller puts it just under the button that asked. Any press, scroll or back gesture hides it again."}
        StoryRow{
            show_placed := Button{text: "Show it"}
            hide_placed := Button{text: "Hide it"}
        }

        StoryHeading{text: "A callout that points"}
        StoryNote{text: "CalloutTooltip is handed the rect of the control it is about. It sits on the side it is asked for, draws a triangle back at that control, and moves to the other side or inward rather than leave the window. The caller shows and hides it, and a menu opening over the page takes it down with every other hover."}
        StoryRow{
            explain := Button{text: "Explain this button"}
            hide_callout := Button{text: "Hide it"}
        }

        // Declared last on purpose. The tooltip claims no room, but the
        // callout is a view that takes a slot in this column wherever it is
        // written, so written here it takes that slot below everything else.
        placed := Tooltip{}
        callout := CalloutTooltip{}
        tips := TipLayer{}
    }
}

fn tip_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let show = root.button(cx, ids!(show_placed));
    if show.clicked(actions) {
        // The caller decides the position. A label that follows the pointer
        // would pass the pointer's; this one passes a point under the button.
        let rect = show.area().rect(cx);
        root.tooltip(cx, ids!(placed)).show_with_options(
            cx,
            Vec2d { x: rect.pos.x, y: rect.pos.y + rect.size.y + 8.0 },
            "placed here by the caller",
        );
    }
    if root.button(cx, ids!(hide_placed)).clicked(actions) {
        root.tooltip(cx, ids!(placed)).hide(cx);
    }

    let explain = root.button(cx, ids!(explain));
    if explain.clicked(actions) {
        // The rect of the control the message is about is all the callout
        // needs to choose a side and aim its triangle.
        let rect = explain.area().rect(cx);
        root.callout_tooltip(cx, ids!(callout)).show_with_options(
            cx,
            "this is the button the callout is about",
            rect,
            CalloutTooltipOptions {
                position: TooltipPosition::Bottom,
                ..Default::default()
            },
        );
    }
    if root.button(cx, ids!(hide_callout)).clicked(actions) {
        root.callout_tooltip(cx, ids!(callout)).hide(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/tip/overview",
    category: "Overlay",
    component: "Tip",
    also: &["TipError", "TipRich", "TipWarning", "Tooltip", "CalloutTooltip"],
    name: "Overview",
    dsl: "TipOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Tooltip\n\nA tooltip is declared by wrapping, not by writing code at the call site: put a `Tip` around a control, give it text, and the window's one `TipLayer` owns the rest.\n\n## Which one to use\n\n| You want | Use |\n|---|---|\n| a tip that belongs to one control, on hover and on focus | `Tip` |\n| a string at a position you work out, such as beside the pointer | `Tooltip` |\n| a message aimed at a control, shown and hidden by your own code | `CalloutTooltip` |\n\n`Tip` is the one to reach for: the layer does the dwell, the placement, the arrow and the keyboard. The other two do none of that, and are there for a caller that has to decide when and where.\n\n## Tip\n\nThe wrapper reports the control's final drawn rect and how the tip should look; the layer owns the dwell, the grace window that makes a row of tipped controls follow instantly, the placement and the chrome. `place` picks one of the twelve placements a popover takes, and the shared placement helper flips and shifts it to stay in the window; the Popover page draws all twelve, and a tip's `Bottom`, `Top`, `Left` and `Right` are the centred ones a popover calls `BottomCenter` and so on. `arrow` puts a pointer on the bubble's anchor-facing edge, aimed at the control even after a flip. `wrap_width` turns a phrase into a paragraph. `delay_secs` overrides the dwell for one tip, and `intent` gives it a role colour.\n\nA tip also appears when its control takes the keyboard focus, and Escape takes it down: a tip only the pointer can reach is a tip half the people using the app never see.\n\n## Tooltip\n\n`Tooltip` is placed by hand. `show_with_options(cx, pos, text)` puts the text at `pos` in window coordinates, and it points at nothing; `set_pos`, `set_text`, `show` and `hide` do the same in parts. Any press, scroll, touch or back gesture hides it. It draws on a layer of its own and claims no room in its parent, so it can be declared anywhere.\n\n## CalloutTooltip\n\n`CalloutTooltip` is aimed. `show_with_options(cx, text, rect, options)` takes the rect of the control the message is about, and `options.position` (`Top`, `Bottom`, `Left` or `Right`) is a preference: it flips to the other side, moves inward, or narrows and wraps rather than leave the window, and its triangle still aims at the middle of that rect. `text_color`, `bg_color` and `triangle_height` style it. It stays until the caller hides it, or until a menu opening over the page clears every hover. Unlike `Tooltip` it takes a slot in the column it is declared in, so declare it where that room does no harm, such as the end of a page.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: Some(tip_actions),
}];
