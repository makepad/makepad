//! The checkbox page: one box and one switch under the controls, a counted
//! change, the mixed state a select-all box needs, the faces, the disabled
//! and error faces, and a mark of your own.
//!
//! And the icon-only face, which is the same widget with a mark for each
//! state and no word at all: the padlock at the head of a row.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::sync::atomic::{AtomicUsize, Ordering};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CheckBoxOverview = StoryPage{
        StoryHeading{text: "One checkbox and one switch, under the controls"}
        StoryNote{text: "The controls write the box's label, the switch's label at rest, its knob growth and outline, and can disable either. Drag the switch's knob along the track: a release past the middle commits. The knob grows while pressed, the pill takes an outline while off, and the label changes with the state."}
        StoryRow{
            subject := CheckBox{text: "Option"}
            switch := Toggle{
                text: "Switch"
                text_on: "Switch is on"
                text_off: "Switch is off"
                draw_bg +: {
                    knob_grow: 0.25
                    outline_size: 2.0
                }
            }
        }

        StoryHeading{text: "A change, counted"}
        StoryNote{text: "Every flip raises a change carrying the new value; the label beside the box counts them."}
        StoryRow{
            simplecheckbox := CheckBox{text: "CheckBox"}
            simplecheckbox_output := Label{text: ""}
        }

        StoryHeading{text: "Mixed"}
        StoryNote{text: "A select-all box over a group. The button walks it through Off, On and Mixed; a click on Mixed resolves to On."}
        StoryRow{
            mixed := CheckBox{text: "All items" state: CheckState.Mixed}
            cycle := Button{text: "Cycle state"}
            state_label := Label{text: "state: Mixed"}
        }

        StoryHeading{text: "Faces"}
        StoryRow{
            CheckBox{text: "CheckBox"}
            CheckBoxFlat{text: "CheckBoxFlat"}
            CheckBoxCircle{text: "CheckBoxCircle"}
            CheckBoxCircle{text: "CheckBoxCircle, on" active: true}
        }
        StoryRow{
            Toggle{text: "Toggle"}
            ToggleFlat{text: "ToggleFlat"}
        }

        StoryHeading{text: "Label first"}
        StoryRow{
            CheckBox{text: "Label before the box" label_before: true}
            Toggle{text: "Label before the pill" label_before: true}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            CheckBox{
                text: "CheckBox"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }

        StoryHeading{text: "Error intent"}
        StoryRow{
            CheckBox{text: "Accept the terms" error: true}
            CheckBox{text: "Checked, still wrong" error: true active: true}
            Toggle{text: "Pill in error" error: true}
        }

        StoryHeading{text: "A mark of your own"}
        StoryNote{text: "CheckBoxCustom draws no box, so the icon it is given is the whole mark. A toggle carries one icon per state on its knob."}
        StoryRow{
            CheckBoxCustom{
                text: "CheckBoxCustom"
                align: Align{x: 0. y: 0.5}
                padding: Inset{top: 0. left: 0. bottom: 0. right: 0.}
                margin: Inset{top: 0. left: 0. bottom: 0. right: 0.}

                label_walk: Walk{
                    width: Fit height: Fit
                    margin: theme.mspace_h_1{left: 5.5}
                }

                draw_icon +: {
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }

                icon_walk: Walk{
                    width: 13.0
                    height: Fit
                }
            }
            Toggle{
                text: "A mark on the knob"
                draw_bg +: {size: 24.0 knob_grow: 0.2}
                label_walk +: {margin: Inset{left: 44.}}
                draw_icon_on +: {
                    svg: crate_resource("self:resources/mark_check.svg")
                    color: theme.color_bg_app
                }
                draw_icon_off +: {
                    svg: crate_resource("self:resources/mark_cross.svg")
                    color: theme.color_label_outer
                }
            }
        }
    }

    mod.stories.CheckBoxIconPage = StoryPage{
        StoryHeading{text: "One lock, and what the press left it in"}
        StoryNote{text: "CheckBoxIcon is the checkbox with a mark for each state and no word: the face for the state it is in, in the middle of the control, and nothing else. Press it. The line beside it is written from the change the control reports, not from anything the page kept of its own."}
        StoryRow{
            subject := CheckBoxIcon{
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
            Label{text: "Roundness"}
            lock_state := Label{text: "open"}
        }

        StoryHeading{text: "The colour is what is read, not the shape"}
        StoryNote{text: "At the size one of these is drawn at, a shackle moves three pixels between open and shut. Down a column of nine rows nobody sees three pixels, so the two states are handed different inks and the ink is what carries the state. The pair on the right proves how far that goes: one mark for both states, two inks, and it still says which one it is in."}
        StoryRow{
            CheckBoxIcon{
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
            CheckBoxIcon{
                active: true
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
            CheckBoxIcon{
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/note_pin.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/note_pin.svg")
                }
            }
            CheckBoxIcon{
                active: true
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/note_pin.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/note_pin.svg")
                }
            }
        }

        StoryHeading{text: "Sizes"}
        StoryNote{text: "The mark takes the icon walk and the face is that plus its padding, square, whatever size it is given. There is no label to lean the mark off centre, which is the whole of why this face exists rather than a checkbox with its text left empty."}
        StoryRow{
            CheckBoxIcon{
                icon_walk: Walk{width: 12. height: 12.}
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
            CheckBoxIcon{
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
            CheckBoxIcon{
                icon_walk: Walk{width: 20. height: 20.}
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
            CheckBoxIcon{
                icon_walk: Walk{width: 28. height: 28.}
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
            }
        }

        StoryHeading{text: "At the head of a row"}
        StoryNote{text: "Where one of these earns its keep: a column of them in front of a column of settings, where the shut ones are picked out at a glance. What a shut one MEANS is the caller's -- here, that a die and a palette leave that row where it is. The control itself only turns over and says so."}
        StoryRow{
            View{
                width: Fit height: Fit flow: Down spacing: 4.
                View{
                    width: Fit height: Fit flow: Right spacing: 6. align: Align{x: 0. y: 0.5}
                    CheckBoxIcon{
                        active: true
                        draw_icon_off +: {
                            svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                        }
                        draw_icon_on +: {
                            svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                        }
                    }
                    Label{width: 90. text: "Saturation"}
                }
                View{
                    width: Fit height: Fit flow: Right spacing: 6. align: Align{x: 0. y: 0.5}
                    CheckBoxIcon{
                        draw_icon_off +: {
                            svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                        }
                        draw_icon_on +: {
                            svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                        }
                    }
                    Label{width: 90. text: "Lightness"}
                }
                View{
                    width: Fit height: Fit flow: Right spacing: 6. align: Align{x: 0. y: 0.5}
                    CheckBoxIcon{
                        active: true
                        draw_icon_off +: {
                            svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                        }
                        draw_icon_on +: {
                            svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                        }
                    }
                    Label{width: 90. text: "Roundness"}
                }
            }
        }

        StoryHeading{text: "Disabled"}
        StoryNote{text: "The other faces go dim through the mark box and the label. This one has neither, so being disabled has to reach the mark itself: what falls is the mark's opacity, which dims a mark of any colour and hands your own two inks back untouched when the control comes alive again."}
        StoryRow{
            CheckBoxIcon{
                draw_icon_off +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                }
                draw_icon_on +: {
                    svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                }
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }
    }
}

/// How many times the counted checkbox changed.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn state_name(state: CheckState) -> &'static str {
    match state {
        CheckState::Off => "Off",
        CheckState::On => "On",
        CheckState::Mixed => "Mixed",
    }
}

/// The counted checkbox writes each change to the label beside it.
fn counter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(check) = root.check_box(cx, ids!(simplecheckbox)).changed(actions) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        log!("CHECK BUTTON CLICKED {} {}", n, check);
        root.label(cx, ids!(simplecheckbox_output))
            .set_text(cx, &format!("{} {}", n + 1, check));
    }
}

/// The cycle button walks the select-all box through its three states, and
/// the readout follows a click on the box itself as well.
fn mixed_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let mixed = root.check_box(cx, ids!(mixed));
    if root.button(cx, ids!(cycle)).clicked(actions) {
        let next = match mixed.state(cx) {
            CheckState::Off => CheckState::On,
            CheckState::On => CheckState::Mixed,
            CheckState::Mixed => CheckState::Off,
        };
        mixed.set_state(cx, next, Animate::Yes);
        root.label(cx, ids!(state_label))
            .set_text(cx, &format!("state: {}", state_name(next)));
    }
    if mixed.changed(actions).is_some() {
        let now = mixed.state(cx);
        root.label(cx, ids!(state_label))
            .set_text(cx, &format!("state: {}", state_name(now)));
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    counter_actions(cx, root, actions);
    mixed_actions(cx, root, actions);
}

/// The lock writes the state its press left it in beside itself. Read off
/// the change the control reported rather than off anything the page kept,
/// so the line is only ever right if the widget said so.
fn lock_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(shut) = root.check_box(cx, ids!(subject)).changed(actions) {
        root.label(cx, ids!(lock_state))
            .set_text(cx, if shut { "shut" } else { "open" });
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/checkbox/overview",
    category: "Selection",
    component: "CheckBox",
    also: &["CheckBoxCircle", "CheckBoxCustom", "Toggle", "ToggleFlat"],
    name: "Overview",
    dsl: "CheckBoxOverview",
    added: "2026-02-23",
    tags: &["controls", "ported"],
    doc: "# CheckBox\n\nA checkbox turns one option on or off. `Toggle` is the same widget drawn as a pill with a knob that slides, for a setting that takes effect the moment it flips; a checkbox suits an option that is read later, when a form is sent.\n\n## States\n\nA checkbox carries a `CheckState`: `Off`, `On` or `Mixed`. Mixed is the select-all box's \"some of them\" state: it draws a dash, reports as not checked, and a click on it resolves to `On`. `state()` and `set_state()` sit beside `active()`, `set_active()` and `changed()`.\n\n## The toggle\n\nThe knob drags along the track and commits on release past the middle, grows while pressed by `knob_grow`, takes an `outline_size` stroke while off, and swaps its label through `text_on` and `text_off`.\n\n## Faces\n\n`CheckBox` is the bevelled standard and `CheckBoxFlat` the plain face; `Toggle` and `ToggleFlat` are the same pair as pills. `CheckBoxCircle` rounds the box all the way, and `label_before` draws the label first and the box at the end of the row.\n\n## Disabled and error\n\nDisabled draws the box, its mark and its label in the theme's disabled colours. `set_disabled` plays it, and a box that starts disabled sets its animator's `disabled` state to `@on`, as the one on this page does.\n\n`error` recolours the box, mark and label with the error ink, whether the box is on or off.\n\n## Styling\n\n`CheckBoxCustom` draws no box of its own, so the icon in `draw_icon` is the whole mark. A toggle carries an icon per state on its knob, in `draw_icon_on` and `draw_icon_off`. `CheckBoxIcon` is the icon-only face: those same two icons in the row instead of on a knob, no word at all, and a page of its own.\n\nThe controls drive the checkbox and the switch beside it.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Option" } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        Control { label: "Switch label", target: "switch", kind: ControlKind::Text { prop: "text_off", default: "Switch is off" } },
        Control { label: "Knob growth", target: "switch", kind: ControlKind::Number { prop: "draw_bg.knob_grow", min: 0., max: 1., step: 0.05, default: 0.25 } },
        Control { label: "Outline", target: "switch", kind: ControlKind::Number { prop: "draw_bg.outline_size", min: 0., max: 4., step: 0.5, default: 2. } },
        Control { label: "Switch disabled", target: "switch", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(overview_actions),
}, Story {
    key: "selection/checkbox/icon-toggle",
    category: "Selection",
    component: "CheckBox",
    also: &["CheckBoxIcon"],
    name: "Icon toggle",
    dsl: "CheckBoxIconPage",
    added: "2026-09-23",
    tags: &["new", "controls", "icon"],
    doc: "# CheckBoxIcon

A checkbox whose whole face is a mark: one icon for off, another for on, drawn in the middle of the control with no word beside it. It turns over on a press, says which state it is in, and reports the change — a checkbox in every respect except what it looks like.

## When to reach for it

Reach for it when the option belongs to something else on the row rather than to a sentence of its own. A padlock in front of a setting, an eye over a layer, a pin on a panel: the row already says what the option is about, and a second word for it would only be in the way. Where there is room for a word, use a `CheckBox` and write one — a mark alone is always a thing somebody has to learn.

It is also the face to reach for when there are many of them in a column. Nine of these read as a column; nine checkboxes with empty labels read as a column of holes.

## Two inks, and why

The two states are drawn in different colours and a caller who retunes them should keep them apart. This is not decoration. At the size one of these is drawn at, the difference between the two padlocks above is a shackle moving about three pixels; down a column nobody sees three pixels, and what a person actually reads is the colour. The declaration takes its pair off the theme — the quieter label ink for off, the full one for on — so the gap survives a theme switch. The controls on this page will let you push them together; do it and watch the column stop saying anything.

The right-hand pair on the second row is the same argument from the other end: one mark for both states, two inks, and it still reads.

## Icon only, and centred

An empty label is not the same as no label. Walked all the same it still takes its own line height and the margin that clears a mark box, and on a face that is nothing but an icon that phantom is exactly what pushes the mark off the middle. The widget skips the label when the text is empty, and `CheckBoxIcon` clears the margin as well, so the face is the icon plus its padding, square, at every size.

## What a lock means is yours

The widget knows nothing about locking. It toggles, it answers `active()` and `state()`, and it reports `CheckBoxAction::Change`. That a shut one holds a row against a die and a palette, or that a shut eye hides a layer, is the caller's to implement and the caller's to say — in a tip, a heading, or the shape of the row it sits in.

## Styling

`draw_icon_off` and `draw_icon_on` carry a mark and an ink each; `icon_walk` sets the side the mark takes, and the padding around it comes from the widget's own `padding`. Everything else is the checkbox: `active`, `set_active`, `state`, `changed`, and the disabled animator track — which on this face drops the marks' opacity, there being no box and no label to go dim instead, and so leaves whatever colours you gave them alone.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Open ink", target: "subject", kind: ControlKind::Color { prop: "draw_icon_off.color", default: 0xFFFFFF40 } },
        Control { label: "Shut ink", target: "subject", kind: ControlKind::Color { prop: "draw_icon_on.color", default: 0xFFFFFFA6 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(lock_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{apply_chunk, id_path};
    use crate::controls::{chunk_for, default_of};

    /// The icon page is markup, which the compiler never reads: the locks
    /// are reached by name and every control writes a property the page
    /// only claims to have. Building it turns a mistake in any of that into
    /// a failed test rather than an empty row in the catalogue.
    ///
    /// The ink pair is asserted here as well as in the library, because the
    /// page's own argument is that the colour is what a person reads: a
    /// page showing two marks in one colour would be illustrating the
    /// opposite of what it says.
    #[test]
    fn the_icon_page_builds_with_two_marks_in_two_inks_and_live_controls() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = STORIES.iter().find(|s| s.dsl == "CheckBoxIconPage").expect("the icon page");
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        // The subject, the line the page's own handler writes, and every
        // control's target.
        for target in [story.subject, "lock_state"]
            .into_iter()
            .chain(story.controls.iter().map(|control| control.target))
            .filter(|target| !target.is_empty())
        {
            assert!(!page.widget(&cx, &id_path(target)).is_empty(), "no widget at {target}");
        }

        let subject = page.widget(&cx, &id_path("subject"));
        {
            let lock = subject.borrow::<CheckBox>().expect("the subject is a CheckBox");
            assert!(lock.draw_icon_off.svg.is_some(), "the open state has no mark");
            assert!(lock.draw_icon_on.svg.is_some(), "the shut state has no mark");
            assert_ne!(
                lock.draw_icon_off.color, lock.draw_icon_on.color,
                "the page draws its two states in one colour"
            );
            assert_eq!(Widget::text(&*lock), "", "the icon-only face carries a word");
        }

        // Every control writes a property the subject really has, applied
        // the way the controls panel applies it and with the value the
        // panel opens on. A control that silently did nothing would look
        // exactly like one that worked.
        for control in story.controls {
            let Some(chunk) = chunk_for(control, &default_of(control)) else {
                continue;
            };
            apply_chunk(&mut cx, &subject, &chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"));
        }
        // And the ink controls land where the page's argument needs them
        // to: a colour written from the panel is the colour the mark takes.
        let ink = "{draw_icon_on.color: #xFF5C39FF}";
        apply_chunk(&mut cx, &subject, ink).unwrap_or_else(|e| panic!("{ink}: {e}"));
        let lock = subject.borrow::<CheckBox>().expect("the subject is a CheckBox");
        assert_eq!(
            lock.draw_icon_on.color,
            Vec4f { x: 1.0, y: 92.0 / 255.0, z: 57.0 / 255.0, w: 1.0 },
            "the shut ink control wrote nothing"
        );
    }
}
