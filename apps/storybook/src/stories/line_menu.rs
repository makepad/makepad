//! The line menu story: a table of contents drawn as a stack of short lines
//! beside a long page, lit where the page is being read.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    /** A short field guide long enough to scroll through several times. */
    let Article = ScrollYView{
        width: Fill
        height: 420.
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_3
        show_bg: true
        // A plain View's pixel is transparent and never reads `color`: paint it here.
        draw_bg +: {color: uniform(theme.color_surface_container_low) pixel: fn() {return Pal.premul(self.color)}}

        intro := H2{text: "Starting a small plot"}
        P{text: "A vegetable plot does not have to be large to be worth the work. A bed the size of a door, looked after well, gives more than a field left to itself, and it teaches you what your ground and your weather will allow before you spend on anything bigger."}
        P{text: "This guide walks through one season, from choosing where the bed goes to the last picking. Each part is short, and each one can be read on the day it is needed rather than all at once."}

        spot := H2{text: "Choosing a spot"}
        P{text: "Where the bed goes decides more than anything you do later. Walk the garden at different times of day before you dig, and look for the place that stays dry underfoot, gets the most light and is close enough to the house that you will visit it often."}

        sun := H3{text: "Sunlight"}
        P{text: "Most vegetables want six hours of direct sun or more. Leafy crops forgive a little shade, but fruiting ones such as beans and squash grow thin and give little without it."}
        P{text: "Note where the shadows of fences, sheds and trees fall in the morning and in the late afternoon. A spot that is bright at noon can still spend half the day in shade."}

        soil := H3{text: "Soil"}
        P{text: "Good soil crumbles in the hand when it is moist and does not set hard when it dries. Heavy ground can be improved with compost worked in over a few seasons; very sandy ground needs the same, for the opposite reason."}

        soil_test := H4{text: "Testing the soil"}
        P{text: "A simple test tells you whether the ground is acid or chalky. Take small samples from several places across the bed, mix them in a clean bucket and use a kit from a garden shop."}
        P{text: "Squeeze a damp handful as well. If it holds a shape and feels sticky it is mostly clay; if it falls apart at once it is mostly sand. Somewhere between the two is what you are aiming for."}

        planting := H2{text: "Planting"}
        P{text: "Sow and plant only once the ground has warmed and the last frost has passed. Seeds put into cold soil sit and rot, and plants set out too early stop growing for weeks even when they survive."}

        spacing := H3{text: "Spacing"}
        P{text: "The distances on a seed packet look generous when the plants are small. Keep to them anyway: crowded plants compete for water and light, and air that cannot move between the leaves brings mould."}
        P{text: "Paths matter as much as rows. Leave room to kneel beside every part of the bed without stepping on it, because trodden soil packs down and roots struggle to get through it."}

        watering := H3{text: "Watering"}
        P{text: "Water deeply and less often rather than a little every day. A good soak sends roots down where the ground stays moist, while a daily sprinkle keeps them near the surface where it dries first."}
        P{text: "Morning is the best time. The leaves dry before evening, the plants have water for the heat of the day, and less is lost to the air than at noon."}

        season := H2{text: "Through the season"}
        P{text: "Once things are growing, the work becomes little and often. A few minutes every other day keeps the weeds small and lets you notice trouble while it is still easy to deal with."}

        pests := H3{text: "Pests"}
        P{text: "Walk the bed and turn a few leaves over each visit. Most damage starts small, and picking off a handful of caterpillars early saves a crop that would be lost a fortnight later."}
        P{text: "Birds, frogs and ground beetles eat far more pests than any spray. A shallow dish of water and a pile of stones at the edge of the bed give them a reason to stay."}

        harvest := H3{text: "Harvest"}
        P{text: "Pick often and pick young. Beans, courgettes and salad leaves taste best small, and most plants keep producing only while their fruit is being taken. A bean left to swell on the plant tells it that its work is done, and within a week the flowers stop coming."}
        P{text: "Harvest in the cool of the morning when the plants are full of water, and eat or store what you pick the same day. Anything left too long on the plant turns tough and tells the plant to stop. Carry a shallow basket rather than a bag, so that soft fruit and leaves are not crushed on the way back to the house."}
        P{text: "Not everything has to be eaten at once. Onions, garlic and squash keep for months if they are left to dry in the sun for a week or two after lifting, then stored somewhere cool, dry and dark with air around them. Potatoes want darkness above all, because light turns their skins green and bitter, and they keep best in paper sacks rather than plastic, which holds the damp. Beans and peas freeze well if they are blanched in boiling water for a minute first, and a glut of tomatoes can be cooked down into sauce and kept in jars for the winter. Whatever you store, look it over every few weeks and take out anything that has begun to soften before it spoils the rest."}
        P{text: "Saving seed costs nothing and gives you plants that suit your own ground. Choose the healthiest plants rather than the first to crop, and let a few of their pods or fruits ripen fully on the plant. Beans and peas are the easiest to start with: leave the pods until they rattle, then shell them and spread the seeds on a plate indoors for a couple of weeks. Tomato seeds need washing out of their pulp and drying on paper. Keep each kind in its own envelope, write the name and the year on it, and store the envelopes in a tin in a cool room, where most seed stays good for three years or more."}
        P{text: "When a crop is finished, clear it rather than leaving the stems to stand. Old plants shelter slugs and carry disease into the next year, while a bare bed can be put straight back to work. Cut the spent plants off at the soil and leave the roots of beans and peas in the ground, where they feed the soil as they rot. Spread a layer of compost over the surface, or sow a quick cover of clover or winter grain to hold the soil together through the wet months. By spring the worms will have taken most of it down, and the bed will be ready to dig again with far less effort than the first time."}

        notes := H2{text: "Closing notes"}
        P{text: "Keep a notebook of what went well."}
    }

    mod.stories.LineMenuOverview = StoryPage{
        StoryNote{text: "A table of contents that takes almost no room: one short line per section, the deeper the section the shorter the line. Rest the pointer on the lines and the names appear beside them. The line of the section you are reading is lit and follows the page as it scrolls, and pressing a name scrolls the page there."}

        StoryHeading{text: "Beside a long page"}
        StoryRow{
            width: Fill
            align: Align{x: 0. y: 0.}
            toc := LineMenu{
                scroll_view: "article"
                sections: [
                    {target: "intro" label: "Starting a small plot" level: 1}
                    {target: "spot" label: "Choosing a spot" level: 1}
                    {target: "sun" label: "Sunlight" level: 2}
                    {target: "soil" label: "Soil" level: 2}
                    {target: "soil_test" label: "Testing the soil" level: 3}
                    {target: "planting" label: "Planting" level: 1}
                    {target: "spacing" label: "Spacing" level: 2}
                    {target: "watering" label: "Watering" level: 2}
                    {target: "season" label: "Through the season" level: 1}
                    {target: "pests" label: "Pests" level: 2}
                    {target: "harvest" label: "Harvest" level: 2}
                    {target: "notes" label: "Closing notes" level: 1}
                ]
            }
            article := Article{}
        }
        StoryRow{
            reading := Label{text: "reading: Starting a small plot"}
        }

        StoryHeading{text: "What is lit"}
        StoryNote{text: "A section counts as reached once its heading has come a quarter of the way down the view. At the very bottom of the page the last section whose heading is on screen is lit, because a short final section never gets that far and would otherwise never light."}

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "The stack is one tab stop. Focusing it shows the names; Up and Down pick a section and Enter goes there. The page does not move until you choose."}

        StoryHeading{text: "Motion"}
        StoryNote{text: "The lines lengthen and the names come in on the theme's emphasized decelerate curve, and go on standard accelerate. A press scrolls the page on the standard curve, which starts and stops on screen. The ease controls hand each any of the theme's eight easings, the same curves Foundations > Motion plays side by side; lengthen the times to see them. On the spring the lines swing past their length and back, and the stack is a little wider to leave them room, while the names and the rows a press lands on stay where they rest; a sprung jump runs past the heading and settles back, but never past either end of the page."}
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

fn line_menu_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let toc = root.line_menu(cx, ids!(toc));
    // A jump lights its row in the same pass, so the jump is the news; the
    // next change after it goes back to saying what is being read.
    let text = if let Some(id) = toc.jumped(actions) {
        Some(format!("went to {}", toc.label_of(id)))
    } else {
        toc.changed(actions).map(|id| format!("reading: {}", toc.label_of(id)))
    };
    if let Some(text) = text {
        root.label(cx, ids!(reading)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/line-menu/overview",
    category: "Navigation",
    component: "LineMenu",
    also: &[],
    name: "Overview",
    dsl: "LineMenuOverview",
    added: "2026-09-13",
    tags: &["new"],
    doc: "# LineMenu

A table of contents as a stack of short lines, one per section. A line's length says how deep its section is. Resting on the stack, or focusing it, lengthens the lines and shows the names on a small card; the card lies over the page, so the menu never takes more room than its lines.

**It follows a scroll view.** `scroll_view` names the view by id, searched outward from the menu. `sections` names a widget inside that view for each section, by id, with the words to show and a `level`. The lit section is the deepest one whose heading has reached `spy_line` of the way down the view; at the very end of the page it is the last one on screen.

**Pressing a name scrolls there**, leaving `scroll_margin` above the heading, and lights that row at once. It stays lit until the page is scrolled again, so a short last section you chose does not give its light away just because the page cannot scroll it to the top.

**Nesting is by level.** A section's parent is the nearest earlier section with a smaller level; the lit section's parents take a softer tint. Sections deeper than `max_level` get no line, and when one of them is being read its nearest shown parent is lit.

**Motion comes from the theme.** The lines lengthen and the names come in over `reveal_secs` along `reveal_ease` (defaults `theme.motion_short_4` and `theme.motion_ease_emphasized_decelerate`), and go over `conceal_secs` along `conceal_ease` (`theme.motion_short_3` and `theme.motion_ease_standard_accelerate`). The jump scrolls over `jump_secs` along `jump_ease` (`theme.motion_medium_4` and `theme.motion_ease_standard`). Any of the theme's eight easings fits; Foundations > Motion plays them side by side. On a curve that overshoots, such as the spring, the lines swing past their revealed length and back, so the stack is widened by the swing to leave them room; the card and the rows a press lands on stay where they rest, and a jump runs past its heading and back but never past either end of the page. `reduced_motion` shows and hides the names, and makes the jump, at once.

**Put the menu beside the view, not inside it.** A view is busy handling its children when they run, so a menu inside the view it follows can never ask where the view is.

`LineMenuLabeled` always shows the names. `LineMenuMirrored` hugs the right edge and opens its names to the left; near a window edge the card opens on whichever side has room. A host with a view of its own kind sets `follow_scroll: false`, calls `set_current` and scrolls on `Jumped`.",
    subject: "toc",
    feature: None,
    controls: &[
        Control { label: "Always show names", target: "toc", kind: ControlKind::Bool { prop: "always_show_labels", default: false } },
        Control { label: "Mirror", target: "toc", kind: ControlKind::Bool { prop: "mirror", default: false } },
        Control { label: "Deepest level shown", target: "toc", kind: ControlKind::Number { prop: "max_level", min: 1., max: 6., step: 1., default: 3. } },
        Control { label: "Reached at", target: "toc", kind: ControlKind::Number { prop: "spy_line", min: 0., max: 1., step: 0.05, default: 0.25 } },
        Control { label: "Room above a heading", target: "toc", kind: ControlKind::Number { prop: "scroll_margin", min: 0., max: 96., step: 1., default: 12. } },
        Control { label: "Jump time", target: "toc", kind: ControlKind::Number { prop: "jump_secs", min: 0., max: 1.5, step: 0.05, default: 0.4 } },
        Control { label: "Jump ease", target: "toc", kind: ControlKind::Choice { prop: "jump_ease", options: EASES, default: 0 } },
        Control { label: "Row height", target: "toc", kind: ControlKind::Number { prop: "row_height", min: 10., max: 40., step: 1., default: 18. } },
        Control { label: "Line length", target: "toc", kind: ControlKind::Number { prop: "line_length", min: 4., max: 80., step: 1., default: 20. } },
        Control { label: "Reveal time", target: "toc", kind: ControlKind::Number { prop: "reveal_secs", min: 0., max: 1., step: 0.01, default: 0.2 } },
        Control { label: "Reveal ease", target: "toc", kind: ControlKind::Choice { prop: "reveal_ease", options: EASES, default: 3 } },
        Control { label: "Conceal time", target: "toc", kind: ControlKind::Number { prop: "conceal_secs", min: 0., max: 1., step: 0.01, default: 0.15 } },
        Control { label: "Conceal ease", target: "toc", kind: ControlKind::Choice { prop: "conceal_ease", options: EASES, default: 2 } },
        Control { label: "Reduced motion", target: "toc", kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
    ],
    on_actions: Some(line_menu_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{apply_chunk, id_path};
    use crate::controls::{chunk_for, ControlValue};

    /// The page is markup, which the compiler never reads: the menu finds its
    /// article by name and each section's heading by id, and nothing checks
    /// either until the page is drawn. Building the page turns a heading id
    /// written wrong into a failed test rather than a row that never lights.
    #[test]
    fn the_page_builds_and_every_section_names_a_heading_in_the_article() {
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
        for target in [story.subject, "article", "reading"]
            .into_iter()
            .chain(story.controls.iter().map(|control| control.target))
            .filter(|target| !target.is_empty())
        {
            assert!(!page.widget(&cx, &id_path(target)).is_empty(), "no widget at {target}");
        }

        let article = page.widget(&cx, &id_path("article"));
        assert!(
            article.borrow::<View>().map_or(false, |view| view.scroll_extent().is_some()),
            "the article is a view the menu can follow"
        );
        let toc = page.widget(&cx, &id_path("toc"));
        let menu = toc.borrow::<LineMenu>().expect("toc is a LineMenu");
        assert_eq!(menu.scroll_view, "article");
        let levels: Vec<u8> = menu.sections().iter().map(|s| s.level).collect();
        assert_eq!(levels, vec![1, 1, 2, 2, 3, 1, 2, 2, 1, 2, 2, 1]);
        for section in menu.sections() {
            assert!(
                !article.widget(&cx, &id_path(&section.target)).is_empty(),
                "no heading {} in the article",
                section.target
            );
            assert_eq!(section.id, LiveId::from_str(&section.target), "the id comes from the target");
        }
        drop(menu);

        // Each ease control offers the theme's eight easings by token, starts
        // on the curve the menu already has, and every option, applied the
        // way the controls panel applies it, puts that curve on the menu.
        let curve = |prop: &str| {
            let menu = toc.borrow::<LineMenu>().expect("toc is a LineMenu");
            match prop {
                "reveal_ease" => menu.reveal_ease,
                "conceal_ease" => menu.conceal_ease,
                "jump_ease" => menu.jump_ease,
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
            assert_eq!(curve(*prop), named, "{} starts on the menu's own curve", control.label);
            for (i, option) in options.iter().enumerate() {
                let chunk = chunk_for(control, &ControlValue::Choice(i)).expect("a choice writes a chunk");
                apply_chunk(&mut cx, &toc, &chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"));
                let named = crate::stories::foundations::theme_ease(&mut cx, option.trim_start_matches("theme."));
                assert_eq!(curve(*prop), named, "{chunk}");
            }
        }
        assert_eq!(eases, 3, "the reveal, the conceal and the jump each have one");
    }

    /// The article is about four views tall. Every section but the short
    /// closing one can be brought up to the margin a jump leaves, so a jump
    /// to Harvest lands on Harvest rather than on the end of the page, a jump
    /// to Pests leaves the page room to be scrolled on by a few points, and
    /// at the very end the closing notes' heading is still in the lower half
    /// of the view, where only the end rule lights it. Measured on the page
    /// as drawn at the canvas width of the catalogue's default window, and at
    /// a wider one.
    #[test]
    fn the_article_lets_every_jump_but_the_last_reach_the_top() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            WidgetRef::script_from_value(vm, value)
        });
        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
        for width in [740.0, 1000.0] {
            let size = dvec2(width, 900.0);
            // Twice: the scroll bars learn the content's size on the first.
            for _ in 0..2 {
                pass.set_size(&mut cx, size);
                let event = DrawEvent::default();
                let mut draw = crate::makepad_widgets::cx_draw::CxDraw::new(&mut cx, &event);
                let mut cx2d = Cx2d::new(&mut draw);
                cx2d.begin_pass(&pass, None);
                list.begin_always(&mut cx2d);
                overlay.begin(&mut cx2d);
                cx2d.begin_root_turtle(size, Layout::flow_down());
                page.draw_all(&mut cx2d, &mut Scope::empty());
                cx2d.end_pass_sized_turtle();
                overlay.end(&mut cx2d);
                list.end(&mut cx2d);
                cx2d.end_pass(&pass);
            }
            let article = page.widget(&cx, &id_path("article"));
            let extent = article.borrow::<View>().and_then(|view| view.scroll_extent()).expect("the article scrolls");
            assert_eq!(extent.pos.y, 0.0);
            let top = article.area().rect(&cx).pos.y;
            let heading = |id: &str| page.widget(&cx, &id_path(id)).area().rect(&cx).pos.y - top;
            let furthest = extent.total.y - extent.visible.y;
            let views = extent.total.y / extent.visible.y;
            assert!((3.6..=5.0).contains(&views), "at {width}: the article is {views:.2} views tall");
            let margin = 12.0;
            for id in ["intro", "spot", "sun", "soil", "soil_test", "planting", "spacing", "watering", "season", "pests", "harvest"] {
                assert!(
                    heading(id) - margin <= furthest,
                    "at {width}: {id} at {} cannot reach the top, the page scrolls only {furthest}",
                    heading(id)
                );
            }
            assert!(heading("pests") - margin + 4.0 <= furthest, "at {width}: after a jump to Pests the page still scrolls on");
            assert!(
                heading("notes") - furthest > extent.visible.y * 0.5,
                "at {width}: at the end the closing notes' heading is at {}, not in the lower half of the view",
                heading("notes") - furthest
            );
        }
    }
}
