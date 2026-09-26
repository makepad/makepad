//! The avatar stories: the picture-initials-mark ladder, the colour a name
//! always gets, shapes, sizes, presence and an overlapping group.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    mod.stories.AvatarOverview = StoryPage{
        StoryNote{text: "One person or thing on a small plate. It shows the best it has: a picture when there is one, the initials of a name when there is not, and a person mark when there is neither."}

        StoryHeading{text: "The three rungs"}
        StoryRow{
            Avatar{
                plate: 56.
                name: "Mira Okafor"
                picture +: {src: crate_resource("self:resources/photo_landscape.jpg")}
            }
            Avatar{plate: 56. name: "Mira Okafor"}
            Avatar{plate: 56.}
            Caption{text: "a picture, a name, and neither"}
        }
        StoryNote{text: "The picture is cropped to the plate rather than squashed into it, and the initials stand in while it loads, so a directory never shows a row of empty holes waiting for the network."}

        StoryHeading{text: "The colour comes from the name"}
        StoryNote{text: "A fixed function of the name, so the same person is the same colour on every screen and in every session, and nothing has to be stored to keep it that way. Case and spacing are folded first: a record that writes \u{201c}ADA  SORENSEN\u{201d} and one that writes \u{201c}Ada Sorensen\u{201d} are one person."}
        StoryRow{
            Avatar{name: "Mira Okafor"}
            Avatar{name: "Jun Park"}
            Avatar{name: "Ada Sorensen"}
            Avatar{name: "Tomas Ruiz"}
            Avatar{name: "Leah Bright"}
            Avatar{name: "Noor Haddad"}
            Avatar{name: "Kai"}
            Avatar{name: "Jean-Luc Vermeer"}
        }
        StoryRow{
            subject := Avatar{
                plate: 56.
                name: "Mira Okafor"
                presence: Online
            }
            cycle := Button{text: "Another name"}
            subject_note := Caption{text: "Mira Okafor \u{2014} MO"}
        }
        StoryNote{text: "Press it a few times: the letters and the colour both follow the name, and a name with nothing in it falls back to the mark and the quiet grey."}

        StoryHeading{text: "What the letters are"}
        StoryNote{text: "The first word's letter and the last word's. Middle names are skipped, because nobody is called by one and three letters in a small circle are three smudges. Punctuation is not a letter, so a stray hyphen never becomes an initial."}
        StoryRow{
            Avatar{name: "Kai"}
            Avatar{name: "Mira Okafor"}
            Avatar{name: "Mira Adaeze Okafor"}
            Avatar{name: "Jean-Luc Vermeer"}
            Avatar{name: "4th Floor"}
            Avatar{name: "Mira Okafor" initials_max: 1}
            Avatar{initials: "OK"}
        }
        StoryRow{
            Caption{text: "Kai \u{2014} Mira Okafor \u{2014} Mira Adaeze Okafor \u{2014} Jean-Luc Vermeer \u{2014} 4th Floor \u{2014} one letter only \u{2014} letters given outright"}
        }

        StoryHeading{text: "Shapes"}
        StoryNote{text: "A circle for a person, a rounded square for a thing: a team, a project, a repository. The picture is cut to whatever shape the plate is."}
        StoryRow{
            Avatar{plate: 48. name: "Mira Okafor" shape: AvatarShape.Circle}
            Avatar{plate: 48. name: "Mira Okafor" shape: AvatarShape.Rounded}
            Avatar{plate: 48. name: "Mira Okafor" shape: AvatarShape.Square}
            Avatar{
                plate: 48.
                shape: AvatarShape.Rounded
                picture +: {src: crate_resource("self:resources/ducky.png")}
            }
            Avatar{
                plate: 48.
                shape: AvatarShape.Circle
                picture +: {src: crate_resource("self:resources/ducky.png")}
            }
        }

        StoryHeading{text: "Sizes"}
        StoryNote{text: "The letters and the presence dot are shares of the plate, so an avatar at any size has ink of the right weight for it. `AvatarSmall`, `AvatarLarge` and `AvatarXl` are the rungs most rows want."}
        StoryRow{
            Avatar{plate: 20. name: "Mira Okafor"}
            AvatarSmall{name: "Mira Okafor"}
            Avatar{name: "Mira Okafor"}
            AvatarLarge{name: "Mira Okafor"}
            AvatarXl{name: "Mira Okafor"}
        }

        StoryHeading{text: "Presence"}
        StoryNote{text: "The dot is the badge's status mark, so every state has a SHAPE as well as a colour and somebody who cannot tell the amber from the green still tells away from online. On a circle it rides the rim rather than the corner of the box, where there is nothing to sit on."}
        StoryRow{
            Avatar{plate: 48. name: "Mira Okafor" presence: Online}
            Avatar{plate: 48. name: "Jun Park" presence: Away}
            Avatar{plate: 48. name: "Ada Sorensen" presence: Busy}
            Avatar{plate: 48. name: "Tomas Ruiz" presence: Offline}
            Avatar{plate: 48. name: "Leah Bright" presence: Unknown}
        }
        StoryRow{
            Caption{text: "online \u{2014} away \u{2014} busy \u{2014} offline \u{2014} unknown"}
        }
        StoryRow{
            Avatar{plate: 48. name: "Mira Okafor" presence: Online presence_corner: BadgeCorner.TopRight}
            Avatar{plate: 48. name: "Mira Okafor" presence: Online presence_corner: BadgeCorner.TopLeft}
            Avatar{plate: 48. name: "Mira Okafor" presence: Online presence_corner: BadgeCorner.BottomLeft}
            Avatar{plate: 48. shape: AvatarShape.Rounded name: "Mira Okafor" presence: Busy}
            Caption{text: "any corner, and the rectangle hangs its dot off one"}
        }

        StoryHeading{text: "A group"}
        StoryNote{text: "Several overlapping, with a count of the ones there was no room for. `max` counts PLATES, the cap included, so the row is never wider than it was told it could be."}
        StoryRow{
            AvatarGroup{
                Avatar{name: "Mira Okafor"}
                Avatar{name: "Jun Park"}
                Avatar{name: "Ada Sorensen"}
            }
            Caption{text: "three of three"}
        }
        StoryRow{
            crowd := AvatarGroup{
                max: 5
                Avatar{name: "Mira Okafor"}
                Avatar{name: "Jun Park"}
                Avatar{name: "Ada Sorensen"}
                Avatar{name: "Tomas Ruiz"}
                Avatar{name: "Leah Bright"}
                Avatar{name: "Noor Haddad"}
                Avatar{name: "Kai"}
                Avatar{name: "Jean-Luc Vermeer"}
            }
            fewer := Button{text: "Fewer plates"}
            crowd_note := Caption{text: "five plates: four faces and a cap for four"}
        }
        StoryNote{text: "The ring round each plate is the colour of whatever the row sits on, which is what makes the overlap read as one face in front of another rather than as a smear. On a coloured panel, set `ring_color` on the avatars to that panel's colour."}
        StoryRow{
            AvatarGroup{
                plate: 28.
                overlap: 0.5
                max: 4
                Avatar{name: "Mira Okafor"}
                Avatar{name: "Jun Park"}
                Avatar{name: "Ada Sorensen"}
                Avatar{name: "Tomas Ruiz"}
                Avatar{name: "Leah Bright"}
            }
            AvatarGroup{
                plate: 44.
                overlap: 0.2
                ring: 3.
                Avatar{name: "Mira Okafor" shape: AvatarShape.Rounded}
                Avatar{name: "Jun Park" shape: AvatarShape.Rounded}
                Avatar{name: "Ada Sorensen" shape: AvatarShape.Rounded}
            }
            Caption{text: "tighter and smaller, or looser and larger"}
        }

        StoryHeading{text: "A plate for a thing"}
        StoryNote{text: "`tint_from_name: false` hands the colour back to `intent`, for a plate that stands for a state or a team rather than a person."}
        StoryRow{
            Avatar{plate: 44. shape: AvatarShape.Rounded name: "Release Train" tint_from_name: false intent: Primary}
            Avatar{plate: 44. shape: AvatarShape.Rounded name: "Build Failed" tint_from_name: false intent: Error}
            Avatar{plate: 44. shape: AvatarShape.Rounded name: "Shipped" tint_from_name: false intent: Success}
            Avatar{plate: 44. shape: AvatarShape.Rounded tint_from_name: false intent: Neutral}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            Avatar{plate: 48. name: "Mira Okafor" presence: Online disabled: true}
            Avatar{
                plate: 48.
                picture +: {src: crate_resource("self:resources/ducky.png")}
                disabled: true
            }
        }
    }
}

/// Names the page cycles the subject through, chosen to walk the initials
/// rule as well as the colours: one word, two, a middle name, a hyphen, a
/// number, and nothing at all.
const NAMES: &[&str] = &[
    "Mira Okafor",
    "Jun Park",
    "Ada Sorensen",
    "Mira Adaeze Okafor",
    "Jean-Luc Vermeer",
    "Kai",
    "4th Floor",
    "",
];

fn next_name(current: &str) -> &'static str {
    let next = NAMES
        .iter()
        .position(|name| *name == current)
        .map(|at| at + 1)
        .unwrap_or(0);
    NAMES[next % NAMES.len()]
}

fn avatar_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(cycle)).clicked(actions) {
        let subject = root.avatar(cx, ids!(subject));
        let name = next_name(&subject.name());
        subject.set_name(cx, name);
        let letters = subject.initials();
        let said = if name.is_empty() { "no name at all" } else { name };
        let shows = if letters.is_empty() { "the mark".to_string() } else { letters };
        root.label(cx, ids!(subject_note))
            .set_text(cx, &format!("{said} \u{2014} {shows}"));
    }
    if root.button(cx, ids!(fewer)).clicked(actions) {
        let crowd = root.avatar_group(cx, ids!(crowd));
        // Round the ladder rather than stopping at the bottom, so the
        // button goes on answering.
        let max = crowd.max();
        let next = if max <= 2 { 8 } else { max - 1 };
        crowd.set_max(cx, next);
        root.label(cx, ids!(crowd_note))
            .set_text(cx, &format!("at most {next} plates, the cap counted"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/avatar/overview",
    category: "Data display",
    component: "Avatar",
    also: &["AvatarGroup", "AvatarSmall", "AvatarLarge", "AvatarXl", "AvatarSquare"],
    name: "Overview",
    dsl: "AvatarOverview",
    added: "2026-09-10",
    tags: &["new", "people", "profile", "initials", "picture", "presence", "group", "identity"],
    doc: "# Avatar\n\nOne person or thing on a small plate. A plate this size can carry exactly one thing, so it carries the best one it has:\n\n1. a **picture**, when the host has pointed `picture` at a source;\n2. the **initials** of `name`, when it has not;\n3. a **person mark**, when there is no name either.\n\nThey are a ladder rather than three widgets because the host almost never knows in advance which rung it is on \u{2014} a directory has photographs for a third of its people and names for the rest, and every call site would otherwise have to write that branch itself.\n\n## The colour a name gets\n\nA fixed function of the name. The same person is the same colour on every screen, in every session and on every machine, and **nothing has to be stored** to make that true \u{2014} which is the point, because a colour that is allocated instead of computed has to be kept somewhere, and then two services disagree about it and the same person is two people.\n\nThe name is folded before it is hashed: case dropped, runs of spaces collapsed. Two records of one person rarely agree on either, and a colour that changed with the spelling would defeat the whole idea.\n\nThe seven colours are the theme's role containers \u{2014} the same ones a badge or a chip speaks \u{2014} so an avatar never introduces a palette of its own. Neutral is deliberately not among them: grey is what a plate with no name gets, and the point of the rest is that people differ. `tint_from_name: false` hands the colour back to `intent`, for a plate standing for a state or a team.\n\n## The letters\n\nThe first word's letter and the last word's. Middle names are skipped rather than crowded in: nobody is called by a middle name, and three letters in a 36 point circle are three grey smudges. A word with no letter or digit in it is not a word, so a lone hyphen between two names never becomes an initial. `initials_max: 1` takes the first only, which is what very small plates want. `initials` overrides the lot and is drawn verbatim \u{2014} it is how a group's cap says \u{201c}+3\u{201d}.\n\n## Presence\n\nThe dot is the badge's status mark, so every state carries a SHAPE as well as a colour: online a filled circle, away a triangle, busy a filled square, offline a hollow ring, unknown a broken one. An operator who cannot tell the amber from the green still tells away from online, and so does a screenshot in grey.\n\nOn a circle the dot rides the rim at forty-five degrees rather than sitting in the corner of the bounding box, where there is nothing under it and it reads as floating away. On a rectangle it hangs off the corner, which is where the eye already looks for a mark.\n\n## AvatarGroup\n\nSeveral avatars overlapping, ending in a count of the ones there was no room for. `max` counts **plates, the cap included**: the whole job of a cap is to bound the row's width, and a cap that made the row one plate wider every time it appeared would not be doing it. So eight people in a row of five are four faces and a \u{201c}+4\u{201d}.\n\nThe group owns its children's size, their ring and where they sit, because a row of five different sizes is not a row. The ring is the colour of whatever the row sits on \u{2014} that is what makes the overlap read as one face in front of another rather than as a smear \u{2014} so on a coloured panel, set `ring_color` on the avatars to that panel's colour.\n\n## What it deliberately does not do\n\nIt does not fetch anything: `picture` is an ordinary image the host points at a source, and the avatar only decides what shape it is cut to and what stands in while it loads.\n\nIt does not answer a press. An avatar is a picture of someone, not a control; a clickable one is an avatar inside a button, and that way the press keeps a button's hover, focus ring and keyboard behaviour instead of a second-rate copy of them.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Name", target: "subject", kind: ControlKind::Text { prop: "name", default: "Mira Okafor" } },
        Control { label: "Shape", target: "subject", kind: ControlKind::Choice { prop: "shape", options: &["Circle", "Rounded", "Square"], default: 0 } },
        Control { label: "Plate", target: "subject", kind: ControlKind::Number { prop: "plate", min: 16., max: 128., step: 2., default: 56. } },
        Control { label: "Corner", target: "subject", kind: ControlKind::Number { prop: "radius", min: 0., max: 40., step: 0.5, default: 6. } },
        Control { label: "Letters", target: "subject", kind: ControlKind::Number { prop: "initials_max", min: 1., max: 2., step: 1., default: 2. } },
        Control { label: "Letter size", target: "subject", kind: ControlKind::Number { prop: "text_scale", min: 0.2, max: 0.7, step: 0.01, default: 0.4 } },
        Control { label: "Presence", target: "subject", kind: ControlKind::Choice { prop: "presence", options: &["Hidden", "Online", "Away", "Busy", "Offline", "Unknown"], default: 1 } },
        Control { label: "Dot size", target: "subject", kind: ControlKind::Number { prop: "presence_scale", min: 0.15, max: 0.5, step: 0.01, default: 0.3 } },
        Control { label: "Ring", target: "subject", kind: ControlKind::Number { prop: "ring", min: 0., max: 8., step: 0.5, default: 0. } },
        Control { label: "Colour from name", target: "subject", kind: ControlKind::Bool { prop: "tint_from_name", default: true } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(avatar_actions),
}];
