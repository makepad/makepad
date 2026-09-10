//! The empty-state stories: the five wordings, the slots they hang on, and
//! one under the controls.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let Column = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5}
    }

    // Every empty state on this page sits on a face, because that is where
    // one lives: in the box the list would have filled.
    let ListArea = RoundedView{
        width: Fill
        height: Fit
        draw_bg +: {
            color: theme.color_surface_container_low
            border_radius: theme.radius_m
        }
    }

    mod.stories.EmptyStateOverview = StoryPage{
        StoryHeading{text: "Five wordings, one shape"}
        StoryNote{text: "A list with no rows still owes the reader an answer, and these five are not the same answer: one of them means start, one means ask for less, one means ask someone, one means try again, and one means wait. The layout never changes, so they are presets of one widget."}
        StoryHeading{text: "One empty state, under the controls"}
        StoryNote{text: "One empty state under the controls: the two lines it says, how wide the words get before they wrap, and the gap between them."}
        ListArea{
            subject := EmptyState{
                heading: "Nothing here yet"
                body: "Whatever you add shows up in this list."
                action: ButtonPrimary{text: "Add the first one"}
            }
        }
        ListArea{
            EmptyStateNothingYet{action: ButtonPrimary{text: "Add the first one"}}
        }
        ListArea{
            EmptyStateNoMatches{action: ButtonOutline{text: "Clear the filters"}}
        }
        ListArea{
            EmptyStateNotAllowed{action: ButtonOutline{text: "Ask for access"}}
        }
        ListArea{
            EmptyStateFailed{action: ButtonPrimary{text: "Try again"}}
        }
        ListArea{
            EmptyStateOffline{}
        }

        StoryHeading{text: "The marks"}
        StoryNote{text: "Circles, boxes and rects, never paths: a small mark drawn as a path does not paint reliably here, and a mark that sometimes fails to appear is worse than a plainer one that always does."}
        StoryRow{
            spacing: theme.space_3
            Column{
                EmptyGlyph{mark: EmptyMark.Nothing}
                Caption{text: "nothing"}
            }
            Column{
                EmptyGlyph{mark: EmptyMark.NoMatches}
                Caption{text: "no matches"}
            }
            Column{
                EmptyGlyph{mark: EmptyMark.NotAllowed}
                Caption{text: "not allowed"}
            }
            Column{
                EmptyGlyph{mark: EmptyMark.Failed}
                Caption{text: "failed"}
            }
            Column{
                EmptyGlyph{mark: EmptyMark.Offline}
                Caption{text: "offline"}
            }
        }

        StoryHeading{text: "The mark is a slot"}
        StoryNote{text: "Anything can stand there. An app with real artwork puts the artwork in; an app with none hides the slot and keeps the words."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            ListArea{
                width: 300.
                EmptyStateNothingYet{
                    heading: "No boards yet"
                    body: "A board holds the cards for one piece of work."
                    icon: RoundedView{
                        width: 72.
                        height: 48.
                        draw_bg +: {
                            color: theme.color_primary_container
                            border_radius: theme.radius_m
                        }
                    }
                }
            }
            ListArea{
                width: 300.
                EmptyStateNoMatches{
                    icon: EmptyGlyph{visible: false}
                }
            }
        }

        StoryHeading{text: "Up to two actions"}
        StoryNote{text: "They share a line while both fit the column and stand one over the other when they do not. The host reads them by id — an empty state that raised one 'the action was taken' event could not say which of the two was taken, which is the whole reason there are two."}
        ListArea{
            retry := EmptyStateFailed{
                action: ButtonPrimary{text: "Try again"}
                secondary: ButtonOutline{text: "Work offline"}
            }
        }
        StoryRow{
            answer := Label{text: "nothing pressed yet"}
        }
        StoryNote{text: "The same pair in a narrow column, where they do not both fit."}
        ListArea{
            width: 220.
            EmptyStateFailed{
                max_width: 180.
                action: ButtonPrimary{text: "Try again"}
                secondary: ButtonOutline{text: "Work offline"}
            }
        }

        StoryHeading{text: "The words wrap to a column, not to the window"}
        StoryNote{text: "max_width caps how wide the body gets before it wraps, and the column is centred in whatever room there is. Without the cap a wide empty area would produce one very long line, which is the hardest kind of line to read."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            ListArea{
                width: 260.
                EmptyStateNotAllowed{}
            }
            ListArea{
                width: 520.
                EmptyStateNotAllowed{max_width: 460.}
            }
        }

        StoryHeading{text: "The wording is a default, not a rule"}
        StoryNote{text: "A preset is one line at the call site because it already says something sensible. An app that knows what the list actually holds should say that instead."}
        ListArea{
            EmptyStateNoMatches{
                heading: "No takes match that"
                body: "Nothing recorded in this session is longer than four minutes. Widen the length filter to see the rest."
                action: ButtonOutline{text: "Widen the filter"}
            }
        }
    }

}

fn empty_state_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // The slots are children under their own names, so the host reaches the
    // buttons through the state rather than through a shared event.
    if root.button(cx, ids!(retry.action)).clicked(actions) {
        let n = crate::stories::bump(live_id!(empty_state_tries));
        root.label(cx, ids!(answer))
            .set_text(cx, &format!("tried again {n} times"));
    }
    if root.button(cx, ids!(retry.secondary)).clicked(actions) {
        root.label(cx, ids!(answer)).set_text(cx, "working offline");
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/empty-state/overview",
        category: "Feedback",
        component: "EmptyState",
        also: &[
            "EmptyGlyph",
            "EmptyStateFailed",
            "EmptyStateNoMatches",
            "EmptyStateNotAllowed",
            "EmptyStateNothingYet",
            "EmptyStateOffline",
        ],
        name: "Overview",
        dsl: "EmptyStateOverview",
        added: "2026-09-10",
        tags: &["new"],
        doc: "# EmptyState\n\nWhat a list shows when it has nothing to show: a mark, a heading, a line of body text and up to two actions, in a centred column.\n\nThe five reasons a list is empty are not one reason, and a reader acts differently on each, so there are five presets that differ only in what they say: `EmptyStateNothingYet` (start), `EmptyStateNoMatches` (ask for less), `EmptyStateNotAllowed` (ask someone), `EmptyStateFailed` (try again) and `EmptyStateOffline` (wait). Each carries its own `heading` and `body`, so the common case is one line at the call site, and either can be overridden by writing the property.\n\nThe mark is a **slot**, not a property: an `EmptyGlyph` by default, or an icon, a picture, an illustration the app owns. `EmptyGlyph` draws one of five figures — `EmptyMark.Nothing`, `NoMatches`, `NotAllowed`, `Failed`, `Offline` — out of circles, boxes and rects. `visible: false` on the slot keeps the words and drops the picture.\n\n`action` and `secondary` are slots too, and the widget raises nothing of its own: a host reads `ids!(state.action)` and `ids!(state.secondary)`, because one event could not say which of two actions was taken.\n\nIt has no face — no card, no border, no fill — so it takes the colour of whatever it is dropped into. Every example on this page is sitting on a `RoundedView` that the widget knows nothing about.",
        subject: "retry",
        feature: None,
        controls: &[
            Control { label: "Heading", target: "subject", kind: ControlKind::Text { prop: "heading", default: "Nothing here yet" } },
            Control {
                label: "Body",
                target: "subject",
                kind: ControlKind::Text { prop: "body", default: "Whatever you add shows up in this list." },
            },
            Control { label: "Max width", target: "subject", kind: ControlKind::Number { prop: "max_width", min: 160., max: 520., step: 10., default: 320. } },
            Control { label: "Text spacing", target: "subject", kind: ControlKind::Number { prop: "text_spacing", min: 0., max: 24., step: 1., default: 3. } },
        ],
        on_actions: Some(empty_state_actions),
    },];
