//! The card stories: three settings of one surface, a picture that reaches
//! the edges, and a surface that answers a press.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CardOverview = StoryPage{
        StoryNote{text: "A surface that holds a picture, a header, a body and a footer. The three appearances are three settings of one shader — a fill, a line, and a rung of the theme's elevation ladder — so a theme change moves all three together."}
        StoryHeading{text: "One card, under the controls"}
        StoryNote{text: "One card under the controls: how the surface is dressed, whether the whole of it answers a press, its corner, and whether it is switched off."}
        StoryRow{
            subject := Card{
                width: 300.
                media: CardMedia{
                    height: 130.
                    Image{
                        width: Fill height: Fill
                        src: crate_resource("self:resources/photo_landscape.jpg")
                        fit: ImageFit.Horizontal
                    }
                }
                header: CardHeader{H4{text: "Ridge route"}}
                body: CardBody{P{text: "Nine kilometres, most of it above the treeline."}}
                footer: CardFooter{
                    subject_button := Button{text: "Details"}
                }
            }
        }
        StoryRow{
            state := Label{text: "nothing pressed yet"}
        }

        StoryHeading{text: "Three appearances"}
        StoryNote{text: "Elevated takes the low container tint and rests on the ladder's first rung. Filled takes the highest container tint and lies flat. Outlined takes the page's own colour with a line around it. Nothing here is hand-mixed: every fill, line and shadow is a theme token."}
        StoryRow{
            ElevatedCard{
                width: 240.
                header: CardHeader{H4{text: "Elevated"}}
                body: CardBody{P{text: "A lift off the page. One card on its own, or a few that have to be told apart from what is behind them."}}
                footer: CardFooter{Button{text: "Open"}}
            }
            FilledCard{
                width: 240.
                header: CardHeader{H4{text: "Filled"}}
                body: CardBody{P{text: "A tint of the page rather than a lift off it. A grid of these reads as a grid; a grid of shadows reads as noise."}}
                footer: CardFooter{Button{text: "Open"}}
            }
            OutlinedCard{
                width: 240.
                header: CardHeader{H4{text: "Outlined"}}
                body: CardBody{P{text: "A line drawn on the page, for a dense list where even a tint is more weight than the content can carry."}}
                footer: CardFooter{Button{text: "Open"}}
            }
        }

        StoryHeading{text: "A picture that reaches the edges"}
        StoryNote{text: "The card holds no padding on the outside: the media slot is laid out first, at the full width, against the bare edge, and the padding you write belongs to the header, body and footer together. Put the picture in a CardMedia and its top corners follow the card's rounding; put it in a plain View and they do not, which is the card on the right."}
        StoryRow{
            ElevatedCard{
                width: 240.
                media: CardMedia{
                    height: 130.
                    Image{
                        width: Fill height: Fill
                        src: crate_resource("self:resources/photo_landscape.jpg")
                        fit: ImageFit.Horizontal
                    }
                }
                header: CardHeader{H4{text: "In a CardMedia"}}
                body: CardBody{P{text: "The band is drawn into a texture and sampled inside the curve, so the picture stops where the corner does."}}
            }
            ElevatedCard{
                width: 240.
                media: View{
                    width: Fill height: 130.
                    Image{
                        width: Fill height: Fill
                        src: crate_resource("self:resources/photo_landscape.jpg")
                        fit: ImageFit.Horizontal
                    }
                }
                header: CardHeader{H4{text: "In a plain View"}}
                body: CardBody{P{text: "Same picture, same slot, no clipping. The two top corners are square and the card's rounding is gone under them."}}
            }
        }

        StoryHeading{text: "The rounding is one number"}
        StoryNote{text: "The card writes its own radius into the media band on every draw, so a card with a different corner does not need the picture told about it separately."}
        StoryRow{
            ElevatedCard{
                width: 240.
                radius: 2.
                media: CardMedia{
                    height: 110.
                    Image{
                        width: Fill height: Fill
                        src: crate_resource("self:resources/photo_landscape.jpg")
                        fit: ImageFit.Horizontal
                    }
                }
                header: CardHeader{H4{text: "radius: 2"}}
            }
            ElevatedCard{
                width: 240.
                radius: 22.
                media: CardMedia{
                    height: 110.
                    Image{
                        width: Fill height: Fill
                        src: crate_resource("self:resources/photo_landscape.jpg")
                        fit: ImageFit.Horizontal
                    }
                }
                header: CardHeader{H4{text: "radius: 22"}}
            }
        }

        StoryHeading{text: "A card you can press"}
        StoryNote{text: "Move the pointer over it: the whole surface rises one rung of the ladder and takes the hover layer. Press it and it comes back down under the finger, because a surface that moves away from the finger pressing it is the wrong way round. Press the button in the footer instead and the button answers, not the card — it is drawn on top of the surface, which is what anyone would expect and the reason a card's own action is worth having."}
        StoryRow{
            pressy := PressableCard{
                width: 260.
                header: CardHeader{H4{text: "Ridge route"}}
                body: CardBody{P{text: "Nine kilometres, most of it above the treeline. The whole card is the target."}}
                footer: CardFooter{
                    details := Button{text: "Details"}
                }
            }
            View{
                width: 300. height: Fit flow: Down spacing: theme.space_1
                said := Label{text: "nothing pressed yet"}
                StoryNote{text: "Only Elevated rests off the ground, so a pressable Filled or Outlined card rises from flat to the first rung instead."}
            }
        }
        StoryRow{
            pressy_filled := PressableCard{
                width: 260.
                appearance: Filled
                header: CardHeader{H4{text: "Filled, pressable"}}
                body: CardBody{P{text: "Flat at rest, one rung up under the pointer."}}
            }
            pressy_outlined := PressableCard{
                width: 260.
                appearance: Outlined
                header: CardHeader{H4{text: "Outlined, pressable"}}
                body: CardBody{P{text: "The line stays; the lift is the only thing that changes."}}
            }
        }

        StoryHeading{text: "Switched off"}
        StoryNote{text: "One flag reaches all four slots, so the button inside goes quiet with the card. The surface lies flat — nothing inert is raised — and the line and the state layer fade, but the fill is left alone: a surface that fades out stops being a surface, and it is the content standing on it that has to read as unavailable."}
        StoryRow{
            OutlinedCard{
                width: 260.
                disabled: true
                header: CardHeader{H4{text: "Ridge route"}}
                body: CardBody{P{text: "Closed until the thaw."}}
                footer: CardFooter{Button{text: "Details"}}
            }
            ElevatedCard{
                width: 260.
                disabled: true
                header: CardHeader{H4{text: "Elevated, switched off"}}
                body: CardBody{P{text: "The shadow is gone as well as the answer."}}
                footer: CardFooter{Button{text: "Open"}}
            }
        }
    }

}

fn card_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // The button is asked first. A press that lands on it is the button's
    // and never the card's, so reporting the card as well would say two
    // things happened when one did.
    let text = if root.button(cx, ids!(details)).clicked(actions) {
        Some("the footer button answered, not the card")
    } else if root.card(cx, ids!(pressy)).clicked(actions) {
        Some("the card answered")
    } else if root.card(cx, ids!(pressy_filled)).clicked(actions) {
        Some("the filled card answered")
    } else if root.card(cx, ids!(pressy_outlined)).clicked(actions) {
        Some("the outlined card answered")
    } else {
        None
    };
    if let Some(text) = text {
        root.label(cx, ids!(said)).set_text(cx, text);
    }
}

fn card_basic_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let text = if root.button(cx, ids!(subject_button)).clicked(actions) {
        Some("the footer button answered")
    } else if root.card(cx, ids!(subject)).clicked(actions) {
        Some("the card answered")
    } else {
        None
    };
    if let Some(text) = text {
        root.label(cx, ids!(state)).set_text(cx, text);
    }
}

fn card_actions_all(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // One page now, so both halves' handlers run for it.
    card_actions(cx, root, actions);
    card_basic_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/card/overview",
        category: "Containers",
        component: "Card",
        also: &[
            "ElevatedCard",
            "FilledCard",
            "OutlinedCard",
            "PressableCard",
            "CardMedia",
            "CardHeader",
            "CardBody",
            "CardFooter",
        ],
        name: "Overview",
        dsl: "CardOverview",
        added: "2026-09-10",
        tags: &["new", "layout"],
        doc: "# Card

A surface that holds a picture, a header, a body and a footer, and knows which rung of the theme's elevation ladder it stands on.

## Three appearances, one surface

`Elevated`, `Filled` and `Outlined` are three settings of one shader rather than three drawings of a card: a fill, an optional line, and a rung of the ladder. Every one of those values comes out of `CardPalette`, which reads the theme's surface, outline and elevation tokens. A theme change therefore moves all three appearances together, and a card cannot end up a shade nothing else in the window is.

Only the elevated card is off the ground. A filled card is a tint of the page and an outlined one is a line drawn on it; giving either a shadow would make all three the same card with different fills, which is the collapse that having three appearances is meant to prevent.

## The press

`pressable` makes the whole surface a target. Hovering it moves it one rung up the ladder; pressing it returns it to the rung it rests on. **A card comes back down under the finger.** Rising away from the finger that is pressing it is the one thing about a card's shadow that people notice, and it is always wrong. The lift is read back out of the shader's own animated values, so the shadow climbs at exactly the rate the hover layer fades in rather than on a second clock of its own.

It is off by default, because a surface that lights up under the pointer and then answers nothing is a promise the widget cannot keep.

A pressable card whose footer holds buttons only hears the presses that reach the surface. The buttons are drawn after the card and sit on top of it, so a press on a button belongs to the button. That is what anyone expects, and it does mean a card is not a way to make its footer bigger.

## The media band

A picture that stops short of the rounding is not a card's picture, it is a picture in a card. So the card holds no padding on the outside: the media slot is laid out first, at the full width, against the bare edge, and the `padding` a caller writes belongs to the header, body and footer together, which are laid out inside it.

`CardMedia` rounds the band's own top corners by drawing its children into a texture and sampling that inside the curve — nothing else in the library clips a child to anything but a rectangle. The bottom corners stay square because the body of the card is directly under them. The card pushes its own radius into the band on every draw, so there is one radius and one place to change it. A media slot that is something else — a plain `View`, an `Image` on its own — carries no such corner, which is the second card in that row.

## What it is not

It does not scroll: a card whose body scrolls is a panel. It carries no title or subtitle typography — the header slot takes whatever you put in it, and the library's headings are already a ladder of their own. It has no expanded state, no swipe, and no fourth appearance.",
        subject: "pressy",
        feature: None,
        controls: &[
            Control {
                label: "Appearance",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "appearance",
                    options: &["Elevated", "Filled", "Outlined"],
                    default: 0,
                },
            },
            Control {
                label: "Pressable",
                target: "subject",
                kind: ControlKind::Bool { prop: "pressable", default: false },
            },
            Control {
                label: "Radius",
                target: "subject",
                kind: ControlKind::Number { prop: "radius", min: 0., max: 40., step: 0.5, default: 8. },
            },
            Control {
                label: "Disabled",
                target: "subject",
                kind: ControlKind::Disabled { default: false },
            },
        ],
        on_actions: Some(card_actions),
    },];
