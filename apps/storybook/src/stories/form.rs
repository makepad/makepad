//! The form story: fields that know when to speak, and a submit that puts
//! the keyboard in the first one that is wrong.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.FormOverview = StoryPage{
        StoryNote{text: "A form knows the fields under it by the path each one is bound to. It keeps the state each field needs to decide when to speak, and answers with the AND of them. The reason it is worth having over five fields that each check themselves: a refused submit puts the keyboard in the first field that is wrong, and no single field knows where it sits among the others."}

        StoryHeading{text: "A form that will not be sent until it is right"}
        StoryNote{text: "Press Submit with the form empty. It refuses, every field starts speaking, and the keyboard lands in the first one down the page — not the worst failure and not the last one found, because the first is where a person reading down the page would get stuck."}
        StoryRow{
            width: Fill
            View{
                width: 420. height: Fit
                subject := Form{
                    name_field := FormField{
                        bind: "name"
                        title: "Name"
                        rules: ["blur: required" "blur: min_len 2" "change: max_len 40"]
                        input: FieldWell{input: WellInput{empty_text: "Who is filling this in"}}
                    }
                    age_field := FormField{
                        bind: "age"
                        title: "Age"
                        rules: ["blur: range 18 120"]
                        input: FieldWell{input: WellInput{empty_text: "18 or over"}}
                    }
                    code_field := FormField{
                        bind: "code"
                        title: "Post code"
                        rules: ["blur: pattern ##-### | Two digits, a dash, three digits."]
                        input: FieldWell{input: WellInput{empty_text: "12-345"}}
                    }
                    pass_field := FormField{
                        bind: "pass"
                        title: "Pass phrase"
                        rules: ["submit: required" "blur: min_len 8"]
                        input: FieldWell{input: WellInput{empty_text: "eight characters at least"}}
                    }
                    again_field := FormField{
                        bind: "again"
                        title: "Pass phrase again"
                        rules: ["change: same_as pass | The two do not match yet."]
                        input: FieldWell{input: WellInput{}}
                    }
                }
            }
        }
        StoryRow{
            send := Button{text: "Submit"}
            clear := Button{text: "Reset"}
        }
        StoryRow{
            outcome := Label{text: "nothing sent yet"}
        }

        StoryHeading{text: "When a rule speaks"}
        StoryNote{text: "Each rule carries its own moment. Name is required on blur, so it waits until you leave it; its length cap is a change rule, so it stops you at the fortieth character rather than telling you afterwards. The pass phrase is required only on submit, because a form that calls a field missing before you have reached it is telling you off for reading in order."}
        StoryNote{text: "The three moments are a ladder rather than three separate lists: a rule due on change is still due at blur and at submit. A rule written with no moment on it is a blur rule."}

        StoryHeading{text: "A rule that reads another field"}
        StoryNote{text: "The second pass phrase carries `change: same_as pass`, and it is checked whenever ANY field changes — including the one it has to match. Typing in the first box re-judges the second, which is a field nobody touched, and that is exactly what a field checking only itself cannot do."}

        StoryHeading{text: "Reading and writing by path"}
        StoryNote{text: "`value` and `set_value` take the same path the field is bound to, so a host that fills a form from somewhere else never has to know which widget is where."}
        StoryRow{
            fill := Button{text: "Fill the post code"}
            status := Label{text: "reading the form"}
        }

        StoryHeading{text: "Reset"}
        StoryNote{text: "Every field back to what its input held when the form was built, and quiet again: no messages, nothing touched, nothing dirty. It is not the same as clearing the fields — a form built with values in it resets to those values."}

        StoryHeading{text: "Switched off"}
        StoryNote{text: "One flag reaches every field under the form, rather than each caller remembering to switch off five inputs and forgetting the fifth. A submit while it is on is refused without judging anything, which is the state a form is in while the last one is still in flight."}
        StoryRow{
            width: Fill
            View{
                width: 420. height: Fit
                Form{
                    disabled: true
                    FormField{
                        bind: "quiet"
                        title: "Not taking answers"
                        rules: ["blur: required"]
                        input: FieldWell{input: WellInput{text: "in flight"}}
                    }
                }
            }
        }

        StoryHeading{text: "A field on its own"}
        StoryNote{text: "A FormField outside a form is a labelled input and nothing more. Its rules are parsed and never run, because the thing that runs them is the controller, and the controller is what the form is."}
        StoryRow{
            width: Fill
            View{
                width: 420. height: Fit
                lonely := FormField{
                    bind: "lonely"
                    title: "Nobody is checking this"
                    rules: ["change: required"]
                    input: FieldWell{input: WellInput{empty_text: "type anything, or nothing"}}
                }
            }
        }
    }
}

fn form_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let form = root.form(cx, ids!(subject));

    if root.button(cx, ids!(send)).clicked(actions) {
        form.submit(cx);
    }
    if root.button(cx, ids!(clear)).clicked(actions) {
        form.reset(cx);
    }
    if root.button(cx, ids!(fill)).clicked(actions) {
        form.set_value(cx, "code", "12-345");
    }

    if form.submitted(actions) {
        root.label(cx, ids!(outcome)).set_text(cx, "Sent.");
    }
    if let Some(path) = form.rejected(actions) {
        root.label(cx, ids!(outcome))
            .set_text(cx, &format!("Refused. The keyboard went to \"{path}\"."));
    }
    if form.was_reset(actions) {
        root.label(cx, ids!(outcome))
            .set_text(cx, "Every field is back where it started.");
    }

    let ready = if form.is_valid() { "ready to send" } else { "not ready" };
    let dirty = if form.is_dirty() { "edited" } else { "untouched" };
    let text = format!("{ready}, {dirty} \u{2014} post code is \"{}\"", form.value("code"));
    let status = root.label(cx, ids!(status));
    if status.text() != text {
        status.set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/form/overview",
    category: "Inputs",
    component: "Form",
    also: &["FormField", "FormFieldLabel", "FormFieldMessage"],
    name: "Overview",
    dsl: "FormOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "form", "validation", "required", "submit", "fields", "rules"],
    doc: "# Form\n\nA controller that knows the fields under it by the path each one is bound to.\n\nA field on its own can say whether what it holds is acceptable. What it cannot say is whether the *form* is, and that is the question the send button is actually asking.\n\n## Why the controller earns its place\n\nOne behaviour: **a refused submit puts the keyboard in the first field that is wrong.** Everything else here — the state per field, the moments, the line under the input — a caller can hand-roll per form, and plenty do. What cannot be hand-rolled field by field is the *order*, because no single field knows where it sits among the others or whether anything above it already failed. Without it, a refused submit leaves a person hunting down the page for the red line, and on a long form they scroll straight past it.\n\n## When a rule speaks\n\nEach rule carries its own moment: `change` while the value is being typed, `blur` when the keyboard leaves, `submit` only when the form is sent. The three are a ladder, not three lists — a rule due on change is still due at blur and at submit. A rule with no moment written on it is a blur rule, because the moment somebody is done typing is the moment they are ready to hear about it, and complaining on every keystroke while they are halfway through an address is the commonest way a form becomes unpleasant.\n\nA field says nothing at all until it has been touched — until the keyboard has been in it and left, or a submit has been refused. An untouched field is still evaluated, since the form's validity is the AND of every field whether or not it has been visited; it just does not shout about a value nobody has typed yet.\n\n## Writing a rule\n\n`[moment:] check [arguments] [| message]`\n\n| Check | Passes when |\n|---|---|\n| `required` | there is something left after the spaces |\n| `min_len N` | at least N characters |\n| `max_len N` | at most N characters |\n| `range LO HI` | it parses as a number between the two, inclusive |\n| `pattern MASK` | it has the mask's shape |\n| `same_as PATH` | it equals another field's value |\n\nEvery check but `required` passes an empty field. A length, a range or a shape that also complained about emptiness would quietly make every optional field on the form compulsory, which is a form nobody can submit.\n\n`pattern` is a **mask, not a regular expression**: `#` is one digit, `@` one letter, `*` any one character, `\\\\` makes the next character literal, and everything else stands for itself; the value must be exactly as long as the mask. There is no regular-expression engine here, and the patterns fields actually need — a post code, a phone number, a date, a reference — are shapes rather than languages. A mask says the shape in a form a reader can check at a glance.\n\n`same_as` is the rule that makes the controller necessary rather than convenient: it reads a value this field does not own, so it has to be re-checked when a *different* field changes.\n\n## The surface\n\n`value` and `set_value` take the bound path. `submit` returns whether the form went, and moves the keyboard when it did not. `reset` puts every field back to what its input held when the form was built — which is not the same as clearing it. `is_valid` asks whether a submit would pass, without touching anything. `disabled` reaches every field under the form and refuses a submit outright, which is the state a form is in while the last one is still in flight.\n\n## What it is not\n\nIt is not a data binder: nothing here writes to a model or watches one, and `bind` is a name a field answers to inside this form and nothing more. There are no async rules — a rule is a pure function of the values on the page, and a check that has to ask a server whether a name is taken belongs in the host, which can set the message itself. It does not repaint the input when the value is wrong: the well has hover, focus and disabled states and no invalid one, and the line under the field is the signal. And it only knows `FormField`s — a bare input dropped into a form is drawn like anything else and is invisible to the rules.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "First field's label",
            target: "name_field",
            kind: ControlKind::Text { prop: "title", default: "Name" },
        },
        Control {
            label: "Age field's label",
            target: "age_field",
            kind: ControlKind::Text { prop: "title", default: "Age" },
        },
        Control {
            label: "Disabled",
            target: "subject",
            kind: ControlKind::Disabled { default: false },
        },
    ],
    on_actions: Some(form_actions),
}];
