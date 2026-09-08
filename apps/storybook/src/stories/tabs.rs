//! The tab stories: a strip that behaves like a browser's, and the shapes a
//! row of choices can take when it is not one.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::sync::Mutex;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TabsOverview = StoryPage{
        StoryNote{text: "Tabs are not a row of buttons. They SHARE the width, they shrink together as more arrive, they can be shut, and the one in use is drawn in the panel's own colour so the two read as one surface. Open a few, shut a few, and watch the widths move."}

        StoryHeading{text: "A strip and its panel"}
        // No gap and no seam: the tab in use and the panel are one surface,
        // so the strip has to sit flush on a panel of the same colour. A
        // rounded panel with space above it turns the strip back into a row
        // of buttons floating over something else.
        joined := View{
            width: Fill
            height: Fit
            flow: Down
            spacing: 0.
            strip := Tabs{labels: ["Overview" "Detail" "History"]}
            panel := View{
                width: Fill
                height: 110.
                flow: Down
                padding: theme.mspace_3
                spacing: theme.space_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                panel_title := H4{text: "Overview"}
                panel_note := P{text: "The panel follows the strip. The tab in use carries this panel's colour, which is what says the strip chooses what is underneath."}
            }
        }
        StoryRow{
            add_one := Button{text: "Add a tab"}
            tab_count := Label{text: "3 open, each 200 wide"}
        }

        StoryHeading{text: "When there are many"}
        StoryNote{text: "Drag the width. Tabs share what there is, down to a floor: below it they stop shrinking and the strip overruns, which is the honest failure until a strip learns to scroll."}
        StoryRow{
            strip_width := Slider{
                width: 300.
                text: "Strip width"
                min: 220.
                max: 720.
                default: 700.
                step: 10.
            }
            width_note := Label{text: "700 points"}
        }
        many_frame := View{
            width: 700.
            height: Fit
            flow: Down
            many := Tabs{
                can_add: false
                labels: ["Mixer" "Effects" "Routing" "Automation" "Metering" "Settings" "Output" "Notes"]
            }
        }

        StoryHeading{text: "Rows of choices that are not tabs"}
        StoryNote{text: "Three neighbours worth telling apart. A segmented control glides a pill between peers and never grows or shrinks. A chip group is the same choice, lighter, and can hold more than one. A vertical segmented control is a rail. None of them shares its width, and that is exactly why none of them is a tab strip."}
        StoryRow{
            period := SegmentedControl{options: ["Day" "Week" "Month"]}
        }
        StoryRow{
            filters := ChipGroup{
                selection: Single
                ChipFlat{text: "All" selectable: true appearance: Outline}
                ChipFlat{text: "Open" selectable: true appearance: Outline}
                ChipFlat{text: "Done" selectable: true appearance: Outline}
            }
        }
        StoryRow{
            SegmentedControlVertical{options: ["North" "East" "South"]}
        }
    }
}

/// The strip's own list. A tab strip owns an ORDER that changes, so the story
/// has to own one too; a fixed list in the DSL could never be shut or added to.
static OPEN: Mutex<Option<Vec<(u64, String)>>> = Mutex::new(None);
static NEXT_ID: Mutex<u64> = Mutex::new(4);

fn entries() -> Vec<TabEntry> {
    let mut guard = OPEN.lock().unwrap();
    let list = guard.get_or_insert_with(|| {
        vec![
            (1, "Overview".to_string()),
            (2, "Detail".to_string()),
            (3, "History".to_string()),
        ]
    });
    list.iter().map(|(id, name)| TabEntry::new(LiveId(*id), name)).collect()
}

fn close(id: LiveId) {
    let mut guard = OPEN.lock().unwrap();
    if let Some(list) = guard.as_mut() {
        // Never the last one: a strip with nothing in it has no panel to
        // show and no way back.
        if list.len() > 1 {
            list.retain(|(other, _)| LiveId(*other) != id);
        }
    }
}

fn add() {
    let mut next = NEXT_ID.lock().unwrap();
    let id = *next;
    *next += 1;
    let mut guard = OPEN.lock().unwrap();
    if let Some(list) = guard.as_mut() {
        list.push((id, format!("Tab {id}")));
    }
}

fn label_of(id: LiveId) -> String {
    OPEN.lock()
        .unwrap()
        .as_ref()
        .and_then(|l| l.iter().find(|(other, _)| LiveId(*other) == id).map(|(_, n)| n.clone()))
        .unwrap_or_default()
}

fn many_entries() -> Vec<TabEntry> {
    ["Mixer", "Effects", "Routing", "Automation", "Metering", "Settings", "Output", "Notes"]
        .iter()
        .enumerate()
        .map(|(i, name)| TabEntry::new(LiveId(i as u64 + 100), name))
        .collect()
}

fn sync(cx: &mut Cx, root: &WidgetRef) {
    let list = entries();
    let count = list.len();
    root.tabs(cx, ids!(strip)).set_tabs(cx, list);
    let width = ((700.0 - 12.0 - 26.0) / count as f64).min(200.0).max(60.0);
    root.label(cx, ids!(tab_count))
        .set_text(cx, &format!("{count} open, each {width:.0} wide"));
}

fn tabs_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let strip = root.tabs(cx, ids!(strip));
    // The strip shows its own `labels` from the markup, so it is never
    // blank. This hands it the list the PAGE owns instead, which is the one
    // that can be added to and shut from, whenever the two have drifted
    // apart -- on the first pass, and again after a reload rebuilds the
    // strip from the markup underneath us.
    if strip.count() != entries().len() {
        sync(cx, root);
    }
    if root.tabs(cx, ids!(many)).count() != many_entries().len() {
        root.tabs(cx, ids!(many)).set_tabs(cx, many_entries());
    }

    if let Some(id) = strip.chosen(actions) {
        let name = label_of(id);
        root.label(cx, ids!(panel_title)).set_text(cx, &name);
    }
    if let Some(id) = strip.closed(actions) {
        close(id);
        sync(cx, root);
        if let Some(now) = root.tabs(cx, ids!(strip)).selected() {
            root.label(cx, ids!(panel_title)).set_text(cx, &label_of(now));
        }
    }
    if strip.added(actions) || root.button(cx, ids!(add_one)).clicked(actions) {
        add();
        sync(cx, root);
    }

    if let Some(w) = root.slider(cx, ids!(strip_width)).slided(actions) {
        let mut frame = root.widget(cx, ids!(many_frame));
        script_apply_eval!(cx, frame, { width: #(w) });
        let each = ((w - 12.0) / 8.0).min(200.0).max(60.0);
        root.label(cx, ids!(width_note))
            .set_text(cx, &format!("{w:.0} points, each tab {each:.0}"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/tabs/overview",
    category: "Navigation",
    component: "Tabs",
    also: &[],
    name: "Overview",
    dsl: "TabsOverview",
    added: "2026-09-07",
    tags: &["new"],
    doc: "# Tabs\n\nTabs are not a row of buttons, and the difference is one rule: **they share the width**. They are as wide as they can be up to a maximum, and they shrink together as more arrive rather than running off the edge. A segmented control never does that, which is why a segmented control never reads as tabs however it is painted.\n\nThree more things follow from taking that seriously.\n\n**A tab can be shut, and the mark for it appears when it earns its room.** On every tab at all times it is noise; on none of them the strip is a dead end. It shows on the tab under the pointer, on the tab in use, and on any tab wide enough that it costs nothing. The middle button shuts one without having to aim at the mark, which is the whole reason people use it.\n\n**The tab in use belongs to the panel below it.** It is drawn in the panel's own colour, so the two read as one surface. That is what says the strip chooses what is underneath, rather than that these are buttons which happen to sit above something.\n\n**The strip owns an order that changes.** Tabs are added and shut, so the list cannot live in the markup; the host holds it and hands it over. That is why this page keeps its own list rather than declaring three children.\n\nWhat it does not do yet: a strip narrower than its floor overruns rather than scrolling, and there is no overflow menu and no scroll-the-chosen-tab-into-view. The arithmetic for the overflow split is already in the library and still has no caller.\n\nThe behaviour was lifted from a working browser chrome in this repository rather than invented. Everything particular to that browser stayed behind.",
    subject: "strip",
    feature: None,
    controls: &[],
    on_actions: Some(tabs_actions),
}];
