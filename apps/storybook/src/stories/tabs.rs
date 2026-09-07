//! The tab stories: the strip bound to a pager, and the looks a strip can take.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let TabPage = View{
        width: Fill
        height: 90.
        flow: Down
        padding: theme.mspace_3
        spacing: theme.space_2
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.TabsOverview = StoryPage{
        StoryNote{text: "A tab strip is a radio group; a tab BODY is a pager. The library has both and nothing binds them, so every app writes the same four lines. This page is that recipe, and the looks a strip can take."}

        StoryHeading{text: "A strip bound to a pager"}
        StoryRow{
            tab_one := RadioButtonTab{text: "Overview"}
            tab_two := RadioButtonTab{text: "Detail"}
            tab_three := RadioButtonTab{text: "History"}
        }
        pages := PageFlip{
            width: Fill
            height: Fit
            lazy_init: true
            active_page: @one
            one := TabPage{P{text: "The first page. Only the page on screen is built, because the pager is lazy."}}
            two := TabPage{P{text: "The second. It was not built until you asked for it."}}
            three := TabPage{P{text: "The third, likewise."}}
        }
        StoryRow{
            built_note := Label{text: "built so far: overview"}
        }

        StoryHeading{text: "The looks a strip can take"}
        StoryNote{text: "Four shapes for one job. A segmented control glides a pill between its answers; a chip group is the same choice in outline; a tab strip is the boxier one; and the segmented control turns on its side without changing anything else."}
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

        StoryHeading{text: "An icon in a tab"}
        StoryNote{text: "A tab is a radio button without the circle, so anything that reserved room for that circle has to give it back."}
        StoryRow{
            icon_one := RadioButtonTab{
                text: "Starred"
                draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
            }
            icon_two := RadioButtonTab{
                text: "Plain"
            }
        }

        StoryHeading{text: "When the strip runs out of room"}
        StoryNote{text: "Drag the width. A strip narrower than its tabs is the case a fixed row cannot answer, and it is the one nothing in the library handles yet: the tabs simply run past the edge."}
        StoryRow{
            strip_width := Slider{
                width: 300.
                text: "Strip width"
                min: 200.
                max: 720.
                default: 700.
                step: 10.
            }
            strip_note := Label{text: "700 points"}
        }
        strip_frame := View{
            width: 700.
            height: Fit
            flow: Right
            spacing: theme.space_1
            show_bg: true
            draw_bg +: {color: theme.color_surface_container_low}
            padding: theme.mspace_1
            RadioButtonTab{text: "Mixer"}
            RadioButtonTab{text: "Effects"}
            RadioButtonTab{text: "Routing"}
            RadioButtonTab{text: "Automation"}
            RadioButtonTab{text: "Metering"}
            RadioButtonTab{text: "Settings"}
        }
    }
}

const PAGES: [LiveId; 3] = [live_id!(one), live_id!(two), live_id!(three)];
const NAMES: [&str; 3] = ["overview", "detail", "history"];

fn tabs_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(index) = root
        .radio_button_set(cx, ids_array!(tab_one, tab_two, tab_three))
        .selected(cx, actions)
    {
        if let Some(page) = PAGES.get(index) {
            root.page_flip(cx, ids!(pages)).set_active_page(cx, *page);
        }
        // Which pages have been BUILT, not which is showing: a lazy pager
        // only ever builds what has been asked for, and that is the whole
        // reason to reach for one.
        let seen = crate::stories::bump(live_id!(tabs_seen_marker));
        let _ = seen;
        if let Some(name) = NAMES.get(index) {
            let label = root.label(cx, ids!(built_note));
            let text = label.text();
            if !text.contains(name) {
                label.set_text(cx, &format!("{text}, {name}"));
            }
        }
    }
    if let Some(w) = root.slider(cx, ids!(strip_width)).slided(actions) {
        let mut frame = root.widget(cx, ids!(strip_frame));
        script_apply_eval!(cx, frame, { width: #(w) });
        root.label(cx, ids!(strip_note)).set_text(cx, &format!("{w:.0} points"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/tabs/overview",
    category: "Navigation",
    component: "Tabs",
    name: "Overview",
    dsl: "TabsOverview",
    added: "2026-02-16",
    tags: &[],
    doc: "# Tabs\n\nThere is no `Tabs` widget, and this page is the argument for one plus the recipe until it exists.\n\n**A tab strip is a radio group and a tab body is a pager.** The library ships both and nothing joins them, so four applications in this repository each wrote the same four lines: read which radio was chosen, look up the page id, set the pager's active page. That binding is the first section here.\n\nThe pager is `lazy_init`, so a page is built the first time it is asked for and not before. The line under it names the pages that have actually been built, which is the reason to reach for a lazy pager rather than a stack of hidden views.\n\n**Four shapes for one job.** A segmented control glides a pill between its answers and is right when the choices are peers. A chip group in outline is the same choice, lighter. A tab strip is the boxier one, and it is what an editor with pages wants. The segmented control also turns on its side, which is the vertical rail case, without changing anything else about it.\n\n**What is missing, and this page shows it rather than hiding it.** Drag the strip narrow and the tabs simply run past the edge: nothing in the library scrolls a strip, collapses it into an overflow menu, or scrolls a chosen tab back into view, even though the arithmetic for the overflow split is already written and unused. A tab cannot be closed or added from the DSL either. Those are the widget this page argues for.",
    subject: "pages",
    feature: None,
    controls: &[],
    on_actions: Some(tabs_actions),
}];
