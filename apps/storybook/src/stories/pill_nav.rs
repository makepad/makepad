//! The pill nav story: a bar of destinations whose panels grow out of the
//! pill under the word, and the same items folded into one pill.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PillNavOverview = StoryPage{
        StoryNote{text: "A row of destinations in one rounded bar. An item can own a panel of links that grows out of the bar when the pointer rests on it, and a soft pill slides under whichever item the pointer, the keyboard or the open panel is on."}

        StoryHeading{text: "A bar with panels"}
        StoryNote{text: "Rest the pointer on Build or Learn. The panel grows out of the pill under the word rather than dropping in from nowhere, so it reads as part of the bar. Move along the bar and the open panel follows; leave it and the panel waits a moment before it goes, which is what lets the pointer cross the gap."}
        StoryRow{
            height: 300.
            align: Align{x: 0.5 y: 0.0}
            subject := PillNav{
                current: @overview
                items: [
                    {id: @overview label: "Overview"}
                    {id: @build label: "Build" links: [
                        {id: @editor label: "Editor" hint: "Write and run code in one place" group: "Make"}
                        {id: @debugger label: "Debugger" hint: "Step through a running program" group: "Make"}
                        {id: @profiler label: "Profiler" hint: "See where the time goes" group: "Make"}
                        {id: @hosting label: "Hosting" hint: "Put a build on a server" group: "Ship"}
                        {id: @updates label: "Updates" hint: "Send a new version to every copy" group: "Ship"}
                    ]}
                    {id: @learn label: "Learn" links: [
                        {id: @guides label: "Guides" hint: "Longer walks through one task"}
                        {id: @reference label: "Reference" hint: "Every property and every type"}
                        {id: @examples label: "Examples" hint: "Small programs to start from"}
                        {id: @changes label: "Changes" hint: "Not written yet" enabled: false}
                    ]}
                    {id: @pricing label: "Pricing"}
                    {id: @contact label: "Contact"}
                ]
            }
        }
        StoryRow{
            readout := Label{text: "nothing chosen yet"}
        }

        StoryHeading{text: "Where you are"}
        StoryNote{text: "The pill rests on the current item when nothing else has the pointer. Choosing a plain item or a link moves the current item there, so the bar always says which section you are in."}

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "The bar is one tab stop. Left and Right walk the items, Down opens a panel and steps into it, Up from the top row goes back to the bar, Left and Right cross columns, Enter chooses and Escape closes. The focus ring only appears when you are using the keys."}

        StoryHeading{text: "Motion"}
        StoryNote{text: "The panel grows out of the pill on the theme's emphasized decelerate curve and shrinks back on standard accelerate. The pill glides between items on the standard curve, comes in where nothing had it on standard decelerate and goes once nothing has it on standard accelerate. The ease controls hand each of them any of the theme's eight easings, the same curves Foundations > Motion plays side by side; lengthen the times to see them. On spring or bounce the panel swings past its place and settles back, and what the pointer and the keys reach is still the panel where it comes to rest."}

        StoryHeading{text: "Folded into one pill"}
        StoryNote{text: "The same items as one pill. A press opens them as a list; an item with links opens in place and closes the one that was open. A bar whose parent is too narrow for it does this by itself."}
        StoryRow{
            height: 360.
            align: Align{x: 0.0 y: 0.0}
            folded := PillNavCompact{
                current: @overview
                items: [
                    {id: @overview label: "Overview"}
                    {id: @build label: "Build" links: [
                        {id: @editor label: "Editor" hint: "Write and run code in one place"}
                        {id: @debugger label: "Debugger" hint: "Step through a running program"}
                        {id: @hosting label: "Hosting" hint: "Put a build on a server"}
                    ]}
                    {id: @learn label: "Learn" links: [
                        {id: @guides label: "Guides"}
                        {id: @reference label: "Reference"}
                    ]}
                    {id: @pricing label: "Pricing"}
                ]
            }
        }
    }
}

/// The theme's easings by token, in the order Foundations > Motion plays
/// them. Written as the DSL names them, so the control applies the token
/// itself and follows the theme rather than a copy of its curves.
const EASES: &[&str] = &[
    "theme.motion_ease_standard",
    "theme.motion_ease_standard_decelerate",
    "theme.motion_ease_standard_accelerate",
    "theme.motion_ease_emphasized_decelerate",
    "theme.motion_ease_emphasized_accelerate",
    "theme.motion_ease_linear",
    "theme.motion_ease_spring",
    "theme.motion_ease_bounce",
];

fn pill_nav_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for id in [ids!(subject), ids!(folded)] {
        let nav = root.pill_nav(cx, id);
        // A choice closes the panel it was made in, so the opening that
        // came before it in the same pass is not news.
        let text = if let Some((item, link)) = nav.picked(actions) {
            Some(format!("went to {} / {}", nav.label_of(item), nav.label_of(link)))
        } else if let Some(item) = nav.selected(actions) {
            Some(format!("went to {}", nav.label_of(item)))
        } else if let Some(item) = nav.opened(actions) {
            let label = nav.label_of(item);
            Some(if label.is_empty() { "showing every item".to_string() } else { format!("showing {label}") })
        } else {
            None
        };
        if let Some(text) = text {
            root.label(cx, ids!(readout)).set_text(cx, &text);
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/pill-nav/overview",
    category: "Navigation",
    component: "PillNav",
    also: &["PillNavCompact"],
    name: "Overview",
    dsl: "PillNavOverview",
    added: "2026-09-13",
    tags: &["new"],
    doc: "# PillNav

A row of destinations in one rounded bar. Some items are places; some own a panel of links. The panel grows out of the pill under its item, so it reads as part of the bar rather than a menu that happened to open nearby.

**Hover has intent.** A panel opens after the pointer has rested for `open_delay`, moves to another item after `switch_delay`, and closes `close_delay` after the pointer has left the bar, the gap and the panel. Sweeping across the bar flashes nothing, and crossing from an item down into its panel never lands on the neighbour.

**Panels are data.** `links` gives each row a label, an optional one-line `hint` and an optional `group`; rows with the same group share a column under that heading. Owning the rows is what lets the panel fade and clip them while it grows, walk them by keyboard, and report each one to tests.

**What it reports.** `Selected` for a plain item, `Picked` with the item and the link for a link, `Opened` and `Closed` for the panel.

**Where it fits.** A panel keeps inside the window: it slides back along the bar near an edge and opens on the other side when there is no room where it was asked for. When the bar is wider than the room its parent gives it, it folds into one pill that opens every item as an accordion (`compact_when_crowded`); `compact: true`, or `PillNavCompact`, folds it always.

**Motion comes from the theme.** The panel grows over `open_secs` along `open_ease` (defaults `theme.motion_medium_1` and `theme.motion_ease_emphasized_decelerate`) and shrinks over `close_secs` along `close_ease` (`theme.motion_short_3` and `theme.motion_ease_standard_accelerate`). The pill glides between items over `glide_secs` along `glide_ease`, and an open panel moves to another item over `switch_secs` along `switch_ease` (`theme.motion_short_4` and `theme.motion_ease_standard` for both). The pill comes in where nothing had it over `highlight_enter_secs` along `highlight_enter_ease` (`theme.motion_short_4` and `theme.motion_ease_standard_decelerate`), and goes once nothing has it over `highlight_exit_secs` along `highlight_exit_ease` (`theme.motion_short_3` and `theme.motion_ease_standard_accelerate`). Any of the theme's eight easings fits; Foundations > Motion plays them side by side. A spring or a bounce swings the surface past its place and back, while hovering, pressing and the keys keep to where it settles. `reduced_motion` lands every change at once.

`open_on: Click`, or `PillNavClick`, opens panels on a press only, and moving along the bar still moves an open one; `Manual` leaves opening to the host.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Opens on", target: "subject", kind: ControlKind::Choice { prop: "open_on", options: &["Hover", "Click", "Manual"], default: 0 } },
        Control { label: "Open delay", target: "subject", kind: ControlKind::Number { prop: "open_delay", min: 0., max: 1., step: 0.01, default: 0.1 } },
        Control { label: "Close grace", target: "subject", kind: ControlKind::Number { prop: "close_delay", min: 0., max: 1., step: 0.01, default: 0.2 } },
        Control { label: "Glide", target: "subject", kind: ControlKind::Number { prop: "glide_secs", min: 0., max: 1., step: 0.01, default: 0.2 } },
        Control { label: "Glide ease", target: "subject", kind: ControlKind::Choice { prop: "glide_ease", options: EASES, default: 0 } },
        Control { label: "Pill in", target: "subject", kind: ControlKind::Number { prop: "highlight_enter_secs", min: 0., max: 1., step: 0.01, default: 0.2 } },
        Control { label: "Pill in ease", target: "subject", kind: ControlKind::Choice { prop: "highlight_enter_ease", options: EASES, default: 1 } },
        Control { label: "Pill out", target: "subject", kind: ControlKind::Number { prop: "highlight_exit_secs", min: 0., max: 1., step: 0.01, default: 0.15 } },
        Control { label: "Pill out ease", target: "subject", kind: ControlKind::Choice { prop: "highlight_exit_ease", options: EASES, default: 2 } },
        Control { label: "Grow", target: "subject", kind: ControlKind::Number { prop: "open_secs", min: 0., max: 1., step: 0.01, default: 0.25 } },
        Control { label: "Grow ease", target: "subject", kind: ControlKind::Choice { prop: "open_ease", options: EASES, default: 3 } },
        Control { label: "Shrink", target: "subject", kind: ControlKind::Number { prop: "close_secs", min: 0., max: 1., step: 0.01, default: 0.15 } },
        Control { label: "Shrink ease", target: "subject", kind: ControlKind::Choice { prop: "close_ease", options: EASES, default: 2 } },
        Control { label: "Item height", target: "subject", kind: ControlKind::Number { prop: "item_height", min: 24., max: 56., step: 1., default: 32. } },
        Control { label: "Panel side", target: "subject", kind: ControlKind::Choice { prop: "panel_placement", options: &["BottomStart", "BottomCenter", "BottomEnd", "TopCenter"], default: 1 } },
        Control { label: "Rest on current", target: "subject", kind: ControlKind::Bool { prop: "highlight_current", default: true } },
        Control { label: "Reduced motion", target: "subject", kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(pill_nav_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{apply_chunk, id_path};
    use crate::controls::{chunk_for, ControlValue};

    /// The page is markup, which the compiler never reads: the bars are
    /// reached by name, their items are a script tree, and every control
    /// writes a property the page only claims to have. Building the page
    /// turns a mistake in any of that into a failed test rather than an
    /// empty bar in the catalogue.
    #[test]
    fn the_page_builds_and_every_bar_is_the_one_the_page_describes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        // The subject, every control's target, and the label the page's own
        // handler writes to.
        for target in [story.subject, "folded", "readout"]
            .into_iter()
            .chain(story.controls.iter().map(|control| control.target))
            .filter(|target| !target.is_empty())
        {
            assert!(!page.widget(&cx, &id_path(target)).is_empty(), "no widget at {target}");
        }

        let subject = page.widget(&cx, &id_path("subject"));
        let bar = subject.borrow::<PillNav>().expect("the subject is a PillNav");
        let ids: Vec<LiveId> = bar.items().iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![live_id!(overview), live_id!(build), live_id!(learn), live_id!(pricing), live_id!(contact)]);
        assert_eq!(bar.current(), Some(live_id!(overview)));
        let build = &bar.items()[1];
        assert_eq!(build.links.len(), 5);
        assert_eq!(build.links.iter().filter(|link| link.group == "Ship").count(), 2, "Build has two columns");
        let learn = &bar.items()[2];
        assert!(learn.links.iter().all(|link| link.group.is_empty()), "Learn is one column without a heading");
        assert!(!learn.links[3].enabled, "Changes is dimmed");
        assert!(!bar.compact);
        drop(bar);

        let folded = page.widget(&cx, &id_path("folded"));
        let folded = folded.borrow::<PillNav>().expect("folded is a PillNav");
        assert!(folded.compact, "the folded bar is folded");
        assert_eq!(folded.items().len(), 4);
        assert_eq!(folded.current(), Some(live_id!(overview)), "the folded pill says where you are");
        assert_eq!(Widget::text(&*folded), "Overview", "and its word is the current item, not Menu");
        drop(folded);

        // Each ease control offers the theme's eight easings by token, starts
        // on the curve the bar already has, and every option, applied the way
        // the controls panel applies it, puts that curve on the bar.
        let curve = |prop: &str| {
            let bar = subject.borrow::<PillNav>().expect("the subject is a PillNav");
            match prop {
                "glide_ease" => bar.glide_ease,
                "highlight_enter_ease" => bar.highlight_enter_ease,
                "highlight_exit_ease" => bar.highlight_exit_ease,
                "open_ease" => bar.open_ease,
                "close_ease" => bar.close_ease,
                other => panic!("no ease control writes {other}"),
            }
        };
        let mut eases = 0;
        for control in story.controls {
            let ControlKind::Choice { prop, options, default } = &control.kind else {
                continue;
            };
            if !prop.ends_with("_ease") {
                continue;
            }
            eases += 1;
            assert_eq!(*options, EASES, "{} offers every theme easing", control.label);
            let token = options[*default].trim_start_matches("theme.");
            let named = crate::stories::foundations::theme_ease(&mut cx, token);
            assert_eq!(curve(*prop), named, "{} starts on the bar's own curve", control.label);
            for (i, option) in options.iter().enumerate() {
                let chunk = chunk_for(control, &ControlValue::Choice(i)).expect("a choice writes a chunk");
                apply_chunk(&mut cx, &subject, &chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"));
                let named = crate::stories::foundations::theme_ease(&mut cx, option.trim_start_matches("theme."));
                assert_eq!(curve(*prop), named, "{chunk}");
            }
        }
        assert_eq!(eases, 5, "the glide, the pill coming and going, the grow and the shrink each have one");
    }
}
