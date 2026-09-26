//! An image well that can show an add mark, a chosen ring, its own progress
//! and a refusal.
use crate::makepad_widgets::dropzone::*;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ImageWellStates = StoryPage{
        StoryNote{text: "The well already drew a picture, a hover, a press, a disabled face and a drag it would or would not take. These are the four states it did not have, and each of them is wanted by any host with more than one well on the screen."}

        StoryHeading{text: "The four, under the controls"}
        StoryNote{text: "Arm and Refuse set the look from outside, which a drag cannot do — a well filled by a press on a browse button somewhere else could neither arm nor refuse. Reject flashes the refusal and lets go by itself. Choose is the ring a list row has had all along. Load steps the bar."}
        StoryRow{
            subject := mod.widgets.ImageWell{
                width: 150
                height: 150
                text: "Drop a picture"
                add_mark: true
            }
        }
        StoryRow{
            arm := Button{text: "Arm"}
            reject := Button{text: "Reject"}
            choose := Button{text: "Choose"}
            load := Button{text: "Load"}
            empty := Button{text: "Empty it"}
            well_note := Label{text: "idle"}
        }

        StoryHeading{text: "The add mark"}
        StoryNote{text: "A plus in a square is the most recognisable \"put something here\" mark there is, and a face carrying nothing but a line of grey text does not read as a target at a glance. It is drawn from boxes rather than paths, the way the library's other empty-state figures are, and engraved — a lit copy a touch lower, then the ink over it — so it reads as cut into the face rather than laid on it like a button. Off by default: a well that has always been words alone keeps looking like one."}
        StoryRow{
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Drop a picture"
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Drop a picture"
                add_mark: true
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: ""
                add_mark: true
                add_mark_size: 40.
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Off"
                add_mark: true
                disabled: true
            }
        }

        StoryHeading{text: "Chosen"}
        StoryNote{text: "A ring, and a tinted face while the well is empty. Over a picture the ring is drawn again on top, because the ground is painted before the picture and a picture cropped to fill covers it whole — border and all. The well never chooses itself: which one is chosen is the host's to know."}
        StoryRow{
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Not chosen"
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Chosen"
                selected: true
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                selected: true
                picture: mod.widgets.Image{
                    src: crate_resource("self:resources/photo_landscape.jpg")
                    fit: mod.widgets.ImageFit.CropToFill
                }
            }
        }

        StoryHeading{text: "Progress, on the well"}
        StoryNote{text: "The bar belongs on the well and not on a row beside it: the well is where the picture went, and a bar somewhere else leaves the reader matching the two up by eye. Below zero there is no bar at all, which is where a well with nothing going on sits. A one percent load draws a dot rather than a smear."}
        StoryRow{
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Nothing going on"
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Just begun"
                progress: 0.01
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Half way"
                progress: 0.5
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                progress: 0.75
                picture: mod.widgets.Image{
                    src: crate_resource("self:resources/photo_landscape.jpg")
                    fit: mod.widgets.ImageFit.CropToFill
                }
            }
        }

        StoryHeading{text: "Armed and refusing, without a drag"}
        StoryNote{text: "Both looks are exactly the ones a drag over the well produces; the only new thing is that a host can set them. Use Arm and Reject above to see them come and go."}
        StoryRow{
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Armed"
                accepting: true
            }
            mod.widgets.ImageWell{
                width: 120
                height: 120
                text: "Refusing"
                refusing: true
            }
        }
    }
}

fn well_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let well = root.image_well(cx, ids!(subject));
    let note = root.label(cx, ids!(well_note));
    if root.button(cx, ids!(arm)).clicked(actions) {
        well.set_accepting(cx, true);
        note.set_text(cx, "armed: it would take one");
    }
    if root.button(cx, ids!(reject)).clicked(actions) {
        well.reject(cx);
        note.set_text(cx, "refused, and it lets go by itself");
    }
    if root.button(cx, ids!(choose)).clicked(actions) {
        let on = !well.selected();
        well.set_selected(cx, on);
        note.set_text(cx, if on { "chosen" } else { "not chosen" });
    }
    if root.button(cx, ids!(load)).clicked(actions) {
        // Round the loop: nothing, then quarters, then nothing again.
        let next = match well.progress() {
            None => Some(0.0),
            Some(p) if p >= 1.0 => None,
            Some(p) => Some((p + 0.25).min(1.0)),
        };
        well.set_progress(cx, next);
        note.set_text(
            cx,
            &match next {
                Some(p) => format!("loading: {:.0}%", p * 100.0),
                None => "no bar".to_string(),
            },
        );
    }
    if root.button(cx, ids!(empty)).clicked(actions) {
        well.clear(cx);
        well.set_progress(cx, None);
        well.set_accepting(cx, false);
        note.set_text(cx, "empty");
    }
    if let Some(file) = well.picked(actions) {
        note.set_text(cx, &format!("took {}", file.name));
    }
    if well.refused(actions).is_some() {
        note.set_text(cx, "would not take that");
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "inputs/dropzone/well-states",
        category: "Inputs",
        component: "Dropzone",
        also: &["ImageWell"],
        name: "Well states",
        dsl: "ImageWellStates",
        added: "2026-09-18",
        tags: &["new", "upload", "picture", "progress", "selected", "reject"],
        doc: "# The four states an image well was missing\n\nThe well already drew six of the seven a host needs: a picture, a hover, a press, a disabled face, a drag it would or would not take, and a ring that thickens for the loud ones. These are the other four.\n\n## An add mark\n\n`add_mark` draws an engraved plus over the prompt while the well is empty. A plus in a square is the most recognisable \"put something here\" mark there is, and a face carrying nothing but a line of grey text does not read as a target at a glance. `add_mark_size` is its box and `add_mark_gap` the clear space to the prompt under it; the two are centred together as one stack, so turning the mark on lifts the words rather than landing on top of them.\n\nIt is drawn from **boxes, not paths**, the way the library's other empty-state figures are: two mirrored path segments in one shader here once painted only the second, and a mark that sometimes fails to appear is worse than a plainer one that always does. The engraving is a lit copy a touch lower with the ink over it, which is what stops the plus reading like a button.\n\nOff by default, because a well that has always been words alone must keep looking like one.\n\n## Chosen\n\n`selected` draws a ring, thickened the way the loud states are, and tints the face while the well is empty. Over a picture the ring is drawn a second time on top: the ground is painted before the picture and a picture cropped to fill covers it whole, border and all. The well never chooses itself, exactly as a list row does not — which one is chosen is the host's to know.\n\n## Progress\n\n`set_progress(cx, Some(0.4))` draws a bar across the foot of the well; `None` takes it away. It belongs here rather than on a file row beside it, because the well is where the picture went and a bar somewhere else leaves the reader matching the two up by eye. In the DSL the same thing is one number, `progress`, with anything below zero meaning no bar — which is the default. A one percent load draws a dot, not a smear.\n\n## Arming and refusing from outside\n\n`set_accepting`, `set_refusing` and `reject` set the same two looks a drag produces. Until now they could only be reached from inside `drag_hits`, so a well filled by a press on a browse button somewhere else could neither arm nor refuse, and a picture the host turned away for a reason the well cannot know — too large, the wrong shape, the server said no — had nowhere to show it. `reject` flashes the refusal for `reject_secs` and then lets go by itself.\n\nThe host's look and the drag's are kept apart. A drag is the platform talking and lasts exactly as long as the pointer is over the target; a host-set look outlives it, and one field would lose it, since a drag anywhere else in the window reports no hit here and would wipe it back to idle.\n\n## What was deliberately left out\n\nNo caption band. A scrim with a title, a tag and a status word would turn a file target into a thumbnail widget; let a second application ask for it.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Add mark", target: "subject", kind: ControlKind::Bool { prop: "add_mark", default: true } },
            Control { label: "Mark size", target: "subject", kind: ControlKind::Number { prop: "add_mark_size", min: 8., max: 80., step: 1., default: 28. } },
            Control { label: "Chosen", target: "subject", kind: ControlKind::Bool { prop: "selected", default: false } },
            Control { label: "Armed", target: "subject", kind: ControlKind::Bool { prop: "accepting", default: false } },
            Control { label: "Refusing", target: "subject", kind: ControlKind::Bool { prop: "refusing", default: false } },
            Control { label: "Progress", target: "subject", kind: ControlKind::Number { prop: "progress", min: -1., max: 1., step: 0.01, default: -1. } },
            Control { label: "Flash (s)", target: "subject", kind: ControlKind::Number { prop: "reject_secs", min: 0., max: 3., step: 0.05, default: 0.7 } },
            Control { label: "Prompt", target: "subject", kind: ControlKind::Text { prop: "text", default: "Drop a picture" } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(well_actions),
    },
];
