//! The chat story: one message, a run of them, and the column they live in.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ChatOverview = StoryPage{
        StoryNote{text: "A message is not a label in a box. It has a side, which decides where it sits and what colour it is; a face and a name; the time it was said; and, for the ones you sent, a mark for how far it got. The part worth writing down once is the grouping: what belongs with what, and what breaks a run."}

        StoryHeading{text: "Two sides"}
        StoryNote{text: "Which side a message is on decides both where it sits and what it is coloured, so the conversation can be read at a glance without reading a word of it. The face and the name go with the side."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down spacing: 2.
                ChatBubble{
                    name: "Ada Lovelace" initials: "AL" time: "09:38"
                    text: "Did the numbers come out?"
                }
                ChatBubble{
                    own: true time: "09:41" delivery: ChatDelivery.Read
                    text: "They did. Every one."
                }
            }
        }

        StoryHeading{text: "A run reads as one block"}
        StoryNote{text: "Consecutive messages from the same sender are one run. Only the first carries the face and the name — repeating them down five lines says the name five times and says who is speaking no better than once would — and the corners on the edge the run stands on tighten, so the run reads as a block rather than as five separate slabs. The other edge keeps its shape."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down spacing: 2.
                ChatBubble{
                    group: ChatGroup.First name: "Ada Lovelace" initials: "AL"
                    text: "I have been staring at column four all morning."
                }
                ChatBubble{
                    group: ChatGroup.Middle
                    text: "It is the rounding, I think."
                }
                ChatBubble{
                    group: ChatGroup.Last time: "09:39"
                    text: "Or the order the rounding happens in, which is worse."
                }
            }
        }
        StoryNote{text: "The same run on your own side, where the joining edge is the right one instead."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down spacing: 2.
                ChatBubble{
                    own: true group: ChatGroup.First
                    text: "The last column needed a second pass."
                }
                ChatBubble{
                    own: true group: ChatGroup.Middle
                    text: "I will send the table over tonight."
                }
                ChatBubble{
                    own: true group: ChatGroup.Last time: "09:43" delivery: ChatDelivery.Delivered
                    text: "Do not read it on a phone."
                }
            }
        }

        StoryHeading{text: "Still arriving"}
        StoryNote{text: "Three dots stand in while nothing has come yet; once there is text, a caret follows the last glyph and moves with it. Both read the pass clock, which repaints the window at display rate, so the mark is drawn only while a message really is streaming — a host that forgets to turn it off leaves the window awake."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down spacing: 2.
                ChatBubble{
                    streaming: true name: "Ada Lovelace" initials: "AL"
                }
                ChatBubble{
                    streaming: true group: ChatGroup.Last
                    text: "The first three columns agree. The fourth is still"
                }
            }
        }

        StoryHeading{text: "How far it got"}
        StoryNote{text: "Five marks, each a different shape as well as a different weight: a ring for handed over, one tick for the server has it, two for the other device has it, two in another colour for a person has seen it, and a warned ring for it did not go. Shape first, colour second, so the marks are still told apart by a reader who cannot separate the colours."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down spacing: 2.
                ChatBubble{own: true time: "09:44" delivery: ChatDelivery.Sending text: "Sending"}
                ChatBubble{own: true time: "09:44" delivery: ChatDelivery.Sent text: "Sent"}
                ChatBubble{own: true time: "09:45" delivery: ChatDelivery.Delivered text: "Delivered"}
                ChatBubble{own: true time: "09:45" delivery: ChatDelivery.Read text: "Read"}
                ChatBubble{own: true time: "09:46" delivery: ChatDelivery.Failed text: "Failed"}
            }
        }

        StoryHeading{text: "A conversation"}
        StoryNote{text: "The list works out the runs itself and draws a separator wherever the day changes. Watch the third and fourth messages: same sender, same day, four hours apart, and the run breaks — two messages four hours apart are two thoughts, not one."}
        StoryNote{text: "Watch the day change as well. Today's clock reads EARLIER than yesterday's, so a list that grouped by the clock alone would have joined them backwards. The day is compared as a label, which is also why the label is the host's string and not something this widget works out from a date."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down
                conversation := ChatList{
                    lines: [
                        "other|Ada Lovelace|09:38|Yesterday|Did the numbers come out?"
                        "other|Ada Lovelace|09:39|Yesterday|I have been staring at column four all morning."
                        "own:read|You|09:41|Yesterday|They did. Every one."
                        "own:read|You|09:42|Yesterday|The last column needed a second pass."
                        "own:read|You|09:43|Yesterday|I will send the table over tonight."
                        "other|Ada Lovelace|14:05|Yesterday|No rush. Tomorrow is fine."
                        "other|Ada Lovelace|08:15|Today|Morning. Any luck with the table?"
                        "own:sent|You|08:31|Today|Sending it now."
                        "other:streaming|Ada Lovelace|08:32|Today|"
                    ]
                }
            }
        }

        StoryHeading{text: "Adding to it"}
        StoryNote{text: "Press the same button twice and the two messages join a run; press the other one between them and they do not. Nothing is told where to draw a corner: the list is handed a message and works the whole plan out again."}
        StoryRow{
            say := Button{text: "Say something"}
            answer := Button{text: "Hear something back"}
            counted := Label{text: "9 messages"}
        }

        StoryHeading{text: "One message, driven"}
        StoryNote{text: "The controls drive this one. A wide message shows the cap and the gutter: a bubble never spans the row, because a message that reaches both edges has stopped saying which side it came from."}
        StoryRow{
            View{
                width: 560. height: Fit flow: Down
                subject := ChatBubble{
                    name: "Ada Lovelace" initials: "AL" time: "09:41"
                    text: "The engine weaves algebraic patterns the way the loom weaves flowers and leaves, and the table below is the proof of it."
                }
            }
        }
    }
}

/// The next message in a conversation: it follows the last one's clock and
/// its day, so whether it joins the run above is decided by who is speaking
/// rather than by an accident of the numbers.
fn next_message(list: &ChatListRef, own: bool, name: &str, text: &str) -> ChatMessage {
    let last = list.last();
    let at = last.as_ref().map(|message| message.at + 40.0).unwrap_or(0.0);
    let day = last.map(|message| message.day).unwrap_or_default();
    let minutes = (at as u64 / 60) % (24 * 60);
    ChatMessage {
        // Past the seeded ids, which are numbered from one.
        id: LiveId(list.count() as u64 + 1000),
        sender: sender_of(name),
        name: name.to_string(),
        initials: bubble_initials(name),
        time: format!("{:02}:{:02}", minutes / 60, minutes % 60),
        day,
        at,
        text: text.to_string(),
        own,
        delivery: if own { ChatDelivery::Sent } else { ChatDelivery::Unmarked },
        streaming: false,
    }
}

fn chat_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let list = root.chat_list(cx, ids!(conversation));
    if root.button(cx, ids!(say)).clicked(actions) {
        let message = next_message(&list, true, "You", "One more thing about the table.");
        list.push(cx, message);
    }
    if root.button(cx, ids!(answer)).clicked(actions) {
        let message = next_message(&list, false, "Ada Lovelace", "Go on, then.");
        list.push(cx, message);
    }
    let text = format!("{} messages", list.count());
    let label = root.label(cx, ids!(counted));
    if label.text() != text {
        label.set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/chat-bubble/overview",
    category: "Data display",
    component: "ChatBubble",
    also: &["ChatList"],
    name: "Overview",
    dsl: "ChatOverview",
    added: "2026-09-10",
    tags: &["new"],
    doc: "# ChatBubble and ChatList

One message, and a column of them.

A message has a side — yours or the other party's — which decides both where it sits and what colour it is; a face and a name; the time it was said; and, for the ones you sent, a mark for how far it got. While an answer is still arriving it has to say so, and the moment it stops saying so is the moment the answer is complete.

## The grouping is arithmetic

Consecutive messages from the same sender are one run. Only the first of a run carries the face and the name, and the corners on the edge the run stands on tighten so the run reads as a single block. Three things break a run, and all three matter:

- **A different sender.** The obvious one.
- **A long enough silence.** Two messages an hour apart are two thoughts, not one. `run_gap` is how long, in seconds.
- **A change of day.** A run that reads as one block cannot have a separator drawn through the middle of it.

A backwards clock breaks it too: messages handed over out of order are not a run in any order the reader can see, and tightening a corner against a neighbour that is not there is worse than not tightening it.

That rule is `plan(entries, gap)`, a free function over `ChatEntry` with its own tests, and the widget is only its drawing. `ChatGroup::radii` is the other half: it answers which corners tighten, and only the corners on the joining edge do — tightening all four would make every bubble in a run squarer and say nothing about what belongs with what.

## What it deliberately does NOT do

**It owns no calendar.** The timestamp and the day label are strings the host hands over already formatted. A UI library that decides what \"yesterday\" means in the reader's timezone is a library that gets it wrong somewhere. The grouping reads seconds and only ever differences between them, and the day is compared as a label.

**It holds no history.** `ChatList` draws every message it is given, so it suits a conversation that fits in a screenful or three. A transcript of ten thousand lines belongs in a `PortalList` drawing `ChatBubble`s, which is why the bubble is usable entirely on its own.

**It carries no composer, no attachments, no reactions and no rich text.** The body is one run of plain text that wraps. A message that needs headings and links is a `Markdown` inside a bubble-shaped container, not a bigger bubble.

## The still-arriving mark

Three dots stand in while nothing has come yet; once there is text a caret follows the last glyph. Its shader reads the pass clock, which pins the window at display rate, so it is drawn **only** while `streaming` is true. Turning `streaming` off is what says the answer is finished — a host that forgets leaves the window awake for as long as the page is open.

## The avatar

The face is a disc with the sender's initials on it, drawn by the bubble. The `avatar` slot is drawn over that disc, so `avatar: Image{...}` replaces it and an empty slot lets it through. The column is reserved down the whole run, whether or not a given message draws a face: a continuation that started at the row's edge would sit further out than the message above it, and a run whose edge moves does not read as a block.

## Markup and Rust

`lines` is a convenience for markup and for this page: one message per line, `side|name|time|day|text`, where `side` may carry flags after a colon (`own:read`, `other:streaming`). It groups by the clock already in the `time` column. `ChatListRef::set_messages` is the real way in, and a message that keeps its id keeps its widget.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Message", target: "subject", kind: ControlKind::Text { prop: "text", default: "The engine weaves algebraic patterns the way the loom weaves flowers and leaves, and the table below is the proof of it." } },
        Control { label: "Own side", target: "subject", kind: ControlKind::Bool { prop: "own", default: false } },
        Control { label: "Streaming", target: "subject", kind: ControlKind::Bool { prop: "streaming", default: false } },
        Control {
            label: "Run position",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "group",
                options: &["ChatGroup.Single", "ChatGroup.First", "ChatGroup.Middle", "ChatGroup.Last"],
                default: 0,
            },
        },
        Control {
            label: "Delivery",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "delivery",
                options: &[
                    "ChatDelivery.Unmarked",
                    "ChatDelivery.Sending",
                    "ChatDelivery.Sent",
                    "ChatDelivery.Delivered",
                    "ChatDelivery.Read",
                    "ChatDelivery.Failed",
                ],
                default: 0,
            },
        },
        Control { label: "Max width", target: "subject", kind: ControlKind::Number { prop: "max_width", min: 80., max: 560., step: 10., default: 420. } },
        Control { label: "Gutter", target: "subject", kind: ControlKind::Number { prop: "min_gutter", min: 0., max: 200., step: 4., default: 56. } },
        Control { label: "Face size", target: "subject", kind: ControlKind::Number { prop: "avatar_size", min: 0., max: 64., step: 2., default: 28. } },
    ],
    on_actions: Some(chat_actions),
}];
