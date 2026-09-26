//! The page flip story: three pages behind three buttons, one showing at a
//! time, and the slide deck, the same idea stepped through with the arrow
//! keys.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PageFlipOverview = StoryPage{
        StoryNote{text: "A PageFlip holds several pages and shows one of them. It draws no tabs and no buttons of its own: whatever chooses the page is the navigation, and the flip only shows the page it is told to."}

        StoryHeading{text: "Three pages, one showing"}
        StoryNote{text: "Each button sets the active page by its id. The change is a cut: nothing moves between one page and the next, and the page that goes out of sight is kept rather than rebuilt."}
        StoryRow{
            pageflipbutton_a := Button{text: "Page A"}
            pageflipbutton_b := Button{text: "Page B"}
            pageflipbutton_c := Button{text: "Page C"}
        }
        StoryRow{
            width: Fill
            // A stated height. The page scrolls, so `height: Fill` resolves
            // to nothing once anything is written after the flip.
            page_flip := PageFlip{
                width: Fill height: 220.
                active_page: @page_a

                // SolidView, not a View with show_bg: a bare View's draw_bg
                // has no colour in its shader and paints nothing.
                page_a := SolidView{
                    width: Fill height: Fill
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {color: theme.color_primary_container}
                    H3{width: Fit text: "Page A" draw_text +: {color: theme.color_on_primary_container}}
                }

                page_b := SolidView{
                    width: Fill height: Fill
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {color: theme.color_secondary_container}
                    H3{width: Fit text: "Page B" draw_text +: {color: theme.color_on_secondary_container}}
                }

                page_c := SolidView{
                    width: Fill height: Fill
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {color: theme.color_tertiary_container}
                    H3{width: Fit text: "Page C" draw_text +: {color: theme.color_on_tertiary_container}}
                }
            }
        }

        StoryHeading{text: "A deck"}
        StoryNote{text: "SlidesView is the same idea for a sequence: one slide fills the space and the arrow keys step through the slides in the order they are written. Press the deck first. It takes the keyboard on a press, so the arrows go to the deck rather than scrolling the page behind it."}
        StoryRow{
            width: Fill
            // A stated height, for the same reason as the flip above: a
            // deck given `height: Fill` here is laid out and never painted.
            SlidesView{
                width: Fill
                height: 320.

                SlideChapter{
                    title := H1{text: "Hey!"}
                    SlideBody{text: "This is the 1st slide. Use your right\ncursor key to show the next slide."}
                }

                Slide{
                    title := H1{text: "Second slide"}
                    SlideBody{text: "This is the 2nd slide. Use your left\ncursor key to show the previous slide."}
                }
            }
        }
        StoryNote{text: "Unlike the flip, a deck moves: it slides sideways from one slide to the next rather than cutting. The first slide is a SlideChapter, the same slide on a heavier ground for the title of a section; the second is a plain Slide."}
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(pageflipbutton_a)).clicked(actions) {
        root.page_flip(cx, ids!(page_flip)).set_active_page(cx, live_id!(page_a));
    }
    if root.button(cx, ids!(pageflipbutton_b)).clicked(actions) {
        root.page_flip(cx, ids!(page_flip)).set_active_page(cx, live_id!(page_b));
    }
    if root.button(cx, ids!(pageflipbutton_c)).clicked(actions) {
        root.page_flip(cx, ids!(page_flip)).set_active_page(cx, live_id!(page_c));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/pageflip/overview",
    category: "Containers",
    component: "PageFlip",
    also: &["SlidesView", "Slide", "SlideBody", "SlideChapter"],
    name: "Overview",
    dsl: "PageFlipOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# PageFlip\n\nA PageFlip holds several pages and shows one of them. It draws no tabs, no buttons and no motion of its own: whatever chooses the page is the navigation, and the flip only shows the page it is told to.\n\n## Which one to use\n\n| You want | Use |\n|---|---|\n| pages chosen by something else: tabs, a list, a menu | `PageFlip` |\n| a sequence a person steps through with the arrow keys, one slide filling the space | `SlidesView` |\n\nA PageFlip cuts from one page to the next; a SlidesView slides between them.\n\n## Choosing the page\n\nThe pages are the named children written under it, and `active_page` names the one showing. `set_active_page` changes it from code and hands back the page it switched to. A page that goes out of sight is kept, not rebuilt, so it comes back holding whatever state it was left in.\n\nEvery page is built when the flip is. `lazy_init: true` builds each page the first time it is shown instead, which suits a flip whose pages are expensive and mostly never visited.\n\nPresses, pointer moves and the wheel reach only the page that is showing. Every other event reaches every page that has been built, so a page out of sight still hears its timers and network replies while it waits.\n\nInside a scrolling column, give the flip a real height: `height: Fill` there resolves to nothing once anything follows it.\n\n## A deck\n\n`SlidesView` is a deck. One slide fills the space it is given and the arrow keys move between them; nothing else is on screen, which is the point of a slide. It takes the keyboard on a press, so the arrows reach the deck rather than scrolling whatever is behind it. Each step reports `flipped` with the slide it is moving to.\n\nA deck does not size itself to its slides, and a deck given `height: Fill` inside a scrolling page is laid out without ever being painted: give it a real height. `anim_speed` is how much of the remaining distance each frame keeps, so a value nearer 1 slides more slowly.\n\n`Slide` is one slide with a title. `SlideChapter` is the same slide on a heavier ground, for the title of a section. `SlideBody` is the text under a slide title.\n\n## A page that is not there\n\n`set_active_page` with an id that names none of the pages logs an error, returns nothing and leaves the showing page where it is.",
    subject: "page_flip",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];
