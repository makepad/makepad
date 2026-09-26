//! One module per component. Each writes its templates under `mod.stories`
//! and exports a table of records; [`modules`] lists the two halves side by
//! side, in navigator order.
//!
//! The records are read at startup and the templates are not. A file is
//! evaluated when the canvas is asked for a page it writes, and again after a
//! style reload has thrown the record of it away -- see [`script_mod`], which
//! is where a theme switch used to spend nearly all of its time.
//!
//! One page at a time, then, rather than a hundred and twelve on every
//! switch.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::collections::HashMap;
use std::sync::Mutex;

pub mod welcome;
pub mod slug;
pub mod foundations;
pub mod tween;
pub mod ease_editor;
pub mod tween_script;
pub mod motion_path;
pub mod sequencer;
pub mod layout;
pub mod grid;
pub mod masonry;
pub mod splitter;
pub mod dock;
pub mod align_scroll;
pub mod scroll_marks_and_fades;
pub mod divider;
pub mod view;
pub mod corner_cap_view;
pub mod card;
pub mod accordion;
pub mod carousel;
pub mod pageflip;
pub mod moving_panels;
pub mod glasspanel;
pub mod glass_surfaces;
pub mod glass_controls;
pub mod surface_material;
pub mod knob_presets;
pub mod splash;
pub mod label;
pub mod typography;
pub mod text_flow;
pub mod html;
pub mod markdown;
pub mod rich_text;
pub mod code_block;
pub mod marquee;
pub mod icon;
pub mod image;
pub mod media;
pub mod image_nine_slice;
pub mod svg;
pub mod vector;
pub mod playback_bar;
pub mod waveform;
pub mod button;
pub mod button_group;
pub mod floating_action;
pub mod textinput;
pub mod field_well;
pub mod number_field;
pub mod slider;
pub mod slider_fader;
pub mod range_slider;
pub mod rotary;
pub mod rating;
pub mod tag_field;
pub mod date_picker;
pub mod calendar;
pub mod time_picker;
pub mod color;
pub mod gradient_editor;
pub mod dropzone;
pub mod form;
pub mod property_inspector;
pub mod hotkeys;
pub mod gizmo;
pub mod checkbox;
pub mod radio_group;
pub mod select;
pub mod combobox;
pub mod chip;
pub mod wheel_picker;
pub mod sliding_ruler;
pub mod column_picker;
pub mod svg_select;
pub mod toolbar;
pub mod window_chrome;
pub mod drop_controls;
pub mod tabs;
pub mod pill_nav;
pub mod nav_list;
pub mod hamburger_menu;
pub mod breadcrumb;
pub mod line_menu;
pub mod pagination;
pub mod stacknavigation;
pub mod tip;
pub mod popover;
pub mod menu;
pub mod pie_menu;
pub mod radial_menu;
pub mod command_palette;
pub mod dialog;
pub mod modal;
pub mod drawer;
pub mod floating_panel;
pub mod tour;
pub mod alert;
pub mod toast;
pub mod progress;
pub mod level_meter;
pub mod spinner;
pub mod placeholder;
pub mod empty_state;
pub mod lists;
pub mod list_item;
pub mod table;
pub mod tree;
pub mod filetree;
pub mod data_grid;
pub mod tile_list;
pub mod item_grid;
pub mod kanban;
pub mod log_list;
pub mod badge;
pub mod avatar;
pub mod kbd;
pub mod chart;
pub mod chart_shapes;
pub mod timeline;
pub mod chat;
pub mod marked_spans;
pub mod spinner_states;
pub mod dropzone_states;

/// One story file: the templates it writes under `mod.stories`, and the
/// records of the pages it documents.
///
/// The catalogue reads the two halves at different times. The records are
/// plain Rust and are read the moment the app starts: the navigator's tree,
/// the search, the story count, the settings keys and the coverage page's
/// "is this documented" column are all built from them, and not one of them
/// needs a template. The templates are script, they cost real time to
/// evaluate, and only ever one page of them is on screen -- so they wait
/// until that page is asked for. See [`script_mod`] for why that matters.
pub struct StoryModule {
    /// Evaluating this writes the file's templates into `mod.stories`.
    pub script_mod: fn(&mut ScriptVm) -> ScriptValue,
    /// The pages the file documents, in navigator order.
    pub stories: &'static [Story],
}

const fn file(
    script_mod: fn(&mut ScriptVm) -> ScriptValue,
    stories: &'static [Story],
) -> StoryModule {
    StoryModule { script_mod, stories }
}

/// Every story file, in navigator order: the categories run in the order they
/// are listed here, and so do the components inside each and the pages inside
/// each component. A host page's file comes before the file of its second
/// page, so Overview heads its folder.
///
/// No file's templates lean on another file's, which is what lets any one of
/// them be evaluated on its own.
static FILES: &[StoryModule] = &[
    // 0 Overview
    file(welcome::script_mod, welcome::STORIES),
    file(crate::coverage::script_mod, crate::coverage::STORIES),
    file(slug::script_mod, slug::STORIES),
    // 1 Foundations
    file(foundations::script_mod, foundations::STORIES),
    file(tween::script_mod, tween::STORIES),
    file(ease_editor::script_mod, ease_editor::STORIES),
    file(tween_script::script_mod, tween_script::STORIES),
    file(motion_path::script_mod, motion_path::STORIES),
    file(sequencer::script_mod, sequencer::STORIES),
    // 2 Layout
    file(layout::script_mod, layout::STORIES),
    file(grid::script_mod, grid::STORIES),
    file(masonry::script_mod, masonry::STORIES),
    file(splitter::script_mod, splitter::STORIES),
    file(dock::script_mod, dock::STORIES),
    file(align_scroll::script_mod, align_scroll::STORIES),
    file(scroll_marks_and_fades::script_mod, scroll_marks_and_fades::STORIES),
    file(divider::script_mod, divider::STORIES),
    // 3 Containers
    file(view::script_mod, view::STORIES),
    file(corner_cap_view::script_mod, corner_cap_view::STORIES),
    file(card::script_mod, card::STORIES),
    file(accordion::script_mod, accordion::STORIES),
    file(carousel::script_mod, carousel::STORIES),
    file(pageflip::script_mod, pageflip::STORIES),
    file(moving_panels::script_mod, moving_panels::STORIES),
    file(glasspanel::script_mod, glasspanel::STORIES),
    file(glass_surfaces::script_mod, glass_surfaces::STORIES),
    file(glass_controls::script_mod, glass_controls::STORIES),
    file(surface_material::script_mod, surface_material::STORIES),
    file(knob_presets::script_mod, knob_presets::STORIES),
    file(splash::script_mod, splash::STORIES),
    // 4 Text
    file(label::script_mod, label::STORIES),
    file(typography::script_mod, typography::STORIES),
    file(text_flow::script_mod, text_flow::STORIES),
    file(marked_spans::script_mod, marked_spans::STORIES),
    file(html::script_mod, html::STORIES),
    file(markdown::script_mod, markdown::STORIES),
    file(rich_text::script_mod, rich_text::STORIES),
    file(code_block::script_mod, code_block::STORIES),
    file(marquee::script_mod, marquee::STORIES),
    // 5 Media
    file(icon::script_mod, icon::STORIES),
    file(image::script_mod, image::STORIES),
    file(media::script_mod, media::STORIES),
    file(image_nine_slice::script_mod, image_nine_slice::STORIES),
    file(svg::script_mod, svg::STORIES),
    file(vector::script_mod, vector::STORIES),
    file(playback_bar::script_mod, playback_bar::STORIES),
    file(waveform::script_mod, waveform::STORIES),
    // 6 Actions
    file(button::script_mod, button::STORIES),
    file(button_group::script_mod, button_group::STORIES),
    file(floating_action::script_mod, floating_action::STORIES),
    // 7 Inputs
    file(textinput::script_mod, textinput::STORIES),
    file(field_well::script_mod, field_well::STORIES),
    file(number_field::script_mod, number_field::STORIES),
    file(slider::script_mod, slider::STORIES),
    file(slider_fader::script_mod, slider_fader::STORIES),
    file(range_slider::script_mod, range_slider::STORIES),
    file(rotary::script_mod, rotary::STORIES),
    file(rating::script_mod, rating::STORIES),
    file(tag_field::script_mod, tag_field::STORIES),
    file(date_picker::script_mod, date_picker::STORIES),
    file(calendar::script_mod, calendar::STORIES),
    file(time_picker::script_mod, time_picker::STORIES),
    file(color::script_mod, color::STORIES),
    file(gradient_editor::script_mod, gradient_editor::STORIES),
    file(dropzone::script_mod, dropzone::STORIES),
    file(dropzone_states::script_mod, dropzone_states::STORIES),
    file(form::script_mod, form::STORIES),
    file(property_inspector::script_mod, property_inspector::STORIES),
    file(hotkeys::script_mod, hotkeys::STORIES),
    file(gizmo::script_mod, gizmo::STORIES),
    // 8 Selection
    file(checkbox::script_mod, checkbox::STORIES),
    file(radio_group::script_mod, radio_group::STORIES),
    file(select::script_mod, select::STORIES),
    file(combobox::script_mod, combobox::STORIES),
    file(chip::script_mod, chip::STORIES),
    file(wheel_picker::script_mod, wheel_picker::STORIES),
    file(sliding_ruler::script_mod, sliding_ruler::STORIES),
    file(column_picker::script_mod, column_picker::STORIES),
    file(svg_select::script_mod, svg_select::STORIES),
    // 9 Navigation
    file(toolbar::script_mod, toolbar::STORIES),
    file(window_chrome::script_mod, window_chrome::STORIES),
    file(drop_controls::script_mod, drop_controls::STORIES),
    file(tabs::script_mod, tabs::STORIES),
    file(pill_nav::script_mod, pill_nav::STORIES),
    file(nav_list::script_mod, nav_list::STORIES),
    file(hamburger_menu::script_mod, hamburger_menu::STORIES),
    file(breadcrumb::script_mod, breadcrumb::STORIES),
    file(line_menu::script_mod, line_menu::STORIES),
    file(pagination::script_mod, pagination::STORIES),
    file(stacknavigation::script_mod, stacknavigation::STORIES),
    // 10 Overlay
    file(tip::script_mod, tip::STORIES),
    file(popover::script_mod, popover::STORIES),
    file(menu::script_mod, menu::STORIES),
    file(pie_menu::script_mod, pie_menu::STORIES),
    file(radial_menu::script_mod, radial_menu::STORIES),
    file(command_palette::script_mod, command_palette::STORIES),
    file(dialog::script_mod, dialog::STORIES),
    file(modal::script_mod, modal::STORIES),
    file(drawer::script_mod, drawer::STORIES),
    file(floating_panel::script_mod, floating_panel::STORIES),
    file(tour::script_mod, tour::STORIES),
    // 11 Feedback
    file(alert::script_mod, alert::STORIES),
    file(toast::script_mod, toast::STORIES),
    file(progress::script_mod, progress::STORIES),
    file(level_meter::script_mod, level_meter::STORIES),
    file(spinner::script_mod, spinner::STORIES),
    file(spinner_states::script_mod, spinner_states::STORIES),
    file(placeholder::script_mod, placeholder::STORIES),
    file(empty_state::script_mod, empty_state::STORIES),
    // 12 Collections
    file(lists::script_mod, lists::STORIES),
    file(list_item::script_mod, list_item::STORIES),
    file(table::script_mod, table::STORIES),
    file(tree::script_mod, tree::STORIES),
    file(filetree::script_mod, filetree::STORIES),
    file(data_grid::script_mod, data_grid::STORIES),
    file(tile_list::script_mod, tile_list::STORIES),
    file(item_grid::script_mod, item_grid::STORIES),
    file(kanban::script_mod, kanban::STORIES),
    file(log_list::script_mod, log_list::STORIES),
    // 13 Data display
    file(badge::script_mod, badge::STORIES),
    file(avatar::script_mod, avatar::STORIES),
    file(kbd::script_mod, kbd::STORIES),
    file(chart::script_mod, chart::STORIES),
    file(chart_shapes::script_mod, chart_shapes::STORIES),
    file(timeline::script_mod, timeline::STORIES),
    file(chat::script_mod, chat::STORIES),
];

/// Every story file, in navigator order.
pub fn modules() -> &'static [StoryModule] {
    FILES
}

/// Which story files have been evaluated into this context's script heap.
///
/// Kept on the `Cx` rather than in a static. The record is only ever true of
/// one heap, and a process that builds a second context -- any test that
/// makes its own `Cx` -- would otherwise be told the first one's work was
/// already done and go reading templates that were never written.
#[derive(Default)]
struct Evaluated {
    /// One flag per entry in [`modules`].
    flags: Vec<bool>,
}

/// Throw away the record of which files have been evaluated. Evaluates
/// nothing itself.
///
/// The platform re-runs the whole `script_mod` chain on every
/// `cx.request_style_reload()`, which is what a theme switch is. Evaluating
/// the 112 story files in that chain is what made a switch slow: measured
/// against the whole widget library they were about 99% of a reload (see
/// `cost_of_a_switch`), and 111 of them were pages nobody was looking at.
///
/// Skipping them is not enough on its own, and the record is the other half.
/// The same chain runs `shell`, whose first act is `mod.stories = {}`, so a
/// reload leaves the module EMPTY: a file that still counted as evaluated
/// would never be run again and its page would come up blank. Even were the
/// module left standing, a template bakes in what `theme.color_*` resolved to
/// when its file ran, so it would come up in the colours of the theme just
/// switched away from. Either way the record has to go, and with it every
/// file's claim to be current.
///
/// What follows the reload is the canvas asking for the page it is showing,
/// which evaluates that one file again under the new theme. Every other file
/// waits until somebody navigates to it, and costs one file's evaluation when
/// they do.
pub fn script_mod(vm: &mut ScriptVm) {
    vm.cx_mut().global::<Evaluated>().flags.clear();
}

/// Evaluate the file that writes this template, unless this context has it
/// already. False when no page in the catalogue names the template, which is
/// the canvas's "missing template" case.
///
/// The canvas calls this as it resolves the page it is about to build, which
/// is the only place a story template is ever read.
pub fn evaluate_for(vm: &mut ScriptVm, dsl: &str) -> bool {
    let Some(index) = modules()
        .iter()
        .position(|file| file.stories.iter().any(|story| story.dsl == dsl))
    else {
        return false;
    };
    evaluate(vm, index);
    true
}

/// Evaluate one file, once per context per reload.
fn evaluate(vm: &mut ScriptVm, index: usize) {
    {
        // The borrow of the context ends before the file runs: evaluating one
        // registers widgets and reaches into the same `Cx` itself.
        let evaluated = vm.cx_mut().global::<Evaluated>();
        if evaluated.flags.len() != modules().len() {
            evaluated.flags.resize(modules().len(), false);
        }
        if evaluated.flags[index] {
            return;
        }
        // Marked before the call and not after: a file that reached back for
        // a page -- its own, or one another file writes -- would otherwise go
        // round for ever, and a file pulled in that way is on the heap like
        // any other, so its flag belongs up whichever way it was reached.
        evaluated.flags[index] = true;
    }
    (modules()[index].script_mod)(vm);
}

/// Forget which files were evaluated, without touching the module: the next
/// page a canvas asks for evaluates its file again, on top of what is there.
/// For a designer preview of one story file: its override is installed,
/// the record dropped, and the canvas rebuilt, and no other file runs.
pub fn forget_evaluated(cx: &mut Cx) {
    cx.global::<Evaluated>().flags.clear();
}

/// How many story files this context has evaluated.
pub fn evaluated_count(cx: &mut Cx) -> usize {
    cx.global::<Evaluated>().flags.iter().filter(|on| **on).count()
}

/// Evaluate every story file, which is what the catalogue used to do on every
/// reload. Only the cost test wants this.
#[cfg(test)]
fn evaluate_all(vm: &mut ScriptVm) {
    for index in 0..modules().len() {
        evaluate(vm, index);
    }
}

static COUNTERS: Mutex<Option<HashMap<LiveId, usize>>> = Mutex::new(None);

/// A per-story counter for demos that count their own clicks; returns the
/// count after the bump.
pub fn bump(key: LiveId) -> usize {
    let mut guard = COUNTERS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let n = map.entry(key).or_insert(0);
    *n += 1;
    *n
}

#[cfg(test)]
mod wiring {
    //! A story file that nothing lists is dead code wearing live code's
    //! clothes: it compiles, it reads well, and the app never sees a line of
    //! it, because [`FILES`] is the one road from a file to the navigator. A
    //! file sat here written and unreachable for a day, and what gave it away
    //! was the story count not moving.

    /// The one entry in [`super::FILES`] that is not a file in this
    /// directory: the coverage page is written beside the count it draws.
    const LISTED_FROM_ELSEWHERE: usize = 1;

    #[test]
    fn every_file_in_this_directory_is_listed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/stories");
        let written = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".rs") && name != "mod.rs"
            })
            .count();
        assert_eq!(
            super::modules().len() - LISTED_FROM_ELSEWHERE,
            written,
            concat!(
                "the table lists one number of story files and the directory holds ",
                "another; a file nothing lists is evaluated by nothing, and its pages ",
                "are in no tree, no search and no count"
            )
        );
    }
}

#[cfg(test)]
mod lazy_evaluation {
    //! A story file is evaluated when the canvas is asked for a page it
    //! writes, and a style reload makes every file evaluated before it stale.
    //! The second half is the one laziness breaks if it is left out. Taking
    //! the invalidation away and running these shows what that costs: the
    //! page after a switch is not merely the old theme's, it is gone
    //! altogether, because the reload emptied `mod.stories` and nothing put
    //! the page back.
    use crate::canvas::id_path;
    use crate::makepad_widgets::makepad_script::trap::NoTrap;
    use crate::makepad_widgets::*;
    use crate::theme::Choice;

    /// The page these tests open, and a label on it.
    ///
    /// The label is painted in a colour the page itself names --
    /// `written := Label{draw_text +: {color: theme.color_text_meta}}` -- and
    /// not one it inherits from the widget library. That matters: the library
    /// is re-evaluated on every reload whatever this file does, so a colour
    /// inherited from it would come back fresh even on a page left stale, and
    /// the switch test would be watching nothing.
    const PAGE: &str = "PropertyInspectorOverview";
    const LABEL: &str = "written";

    /// Bring a context up under a theme the way the app's `script_mod` does,
    /// and no further: this evaluates no story file.
    fn run_script_mod(vm: &mut ScriptVm, theme: Choice) {
        crate::theme::apply_choice(vm, theme);
        crate::theme::widgets_script_mod(vm);
        crate::shell::script_mod(vm);
        super::script_mod(vm);
    }

    /// Whether the page's template is on the heap for the canvas to find.
    fn page_is_written(cx: &mut Cx) -> bool {
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            vm.bx
                .heap
                .value(stories, LiveId::from_str(PAGE).into(), NoTrap)
                .as_object()
                .is_some()
        })
    }

    /// Build the page from its template the way the canvas does, and read
    /// back the colour the page asked for on the label it painted.
    fn label_color(cx: &mut Cx) -> Vec4f {
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm
                .bx
                .heap
                .value(stories, LiveId::from_str(PAGE).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {PAGE}");
            WidgetRef::script_from_value(vm, value)
        });
        let _ = makepad_platform::shader_error::take();
        assert!(!page.is_empty(), "{PAGE} built no widget");
        let widget = page.widget(&*cx, &id_path(LABEL));
        assert!(!widget.is_empty(), "no widget at {LABEL} on {PAGE}");
        let label = widget
            .borrow::<Label>()
            .unwrap_or_else(|| panic!("{LABEL} is not a Label"));
        label.draw_text.color
    }

    #[test]
    fn a_page_is_written_when_it_is_asked_for_and_not_before() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| run_script_mod(vm, Choice::Base(0)));
        assert_eq!(
            super::evaluated_count(&mut cx),
            0,
            "starting up evaluates no story file"
        );
        assert!(
            !page_is_written(&mut cx),
            "so no page template is on the heap yet"
        );

        cx.with_vm(|vm| assert!(super::evaluate_for(vm, PAGE), "{PAGE} is a page"));
        assert_eq!(
            super::evaluated_count(&mut cx),
            1,
            "and opening one page costs one file, not a hundred and twelve"
        );
        assert!(page_is_written(&mut cx), "which is what wrote the page");
    }

    #[test]
    fn a_name_no_page_owns_is_not_a_story() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| run_script_mod(vm, Choice::Base(0)));
        cx.with_vm(|vm| {
            assert!(
                !super::evaluate_for(vm, "NotAPageAnybodyWrote"),
                "the canvas needs this back to say the template is missing"
            );
        });
        assert_eq!(super::evaluated_count(&mut cx), 0);
    }

    #[test]
    fn a_style_reload_leaves_no_page_claiming_to_be_current() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            run_script_mod(vm, Choice::Base(0));
            super::evaluate_for(vm, PAGE);
        });
        assert_eq!(super::evaluated_count(&mut cx), 1);

        cx.with_vm(|vm| vm.with_reload(|vm| run_script_mod(vm, Choice::Base(1))));
        assert_eq!(
            super::evaluated_count(&mut cx),
            0,
            "a page written under the theme just switched away from still \
             counted as current, so the canvas would never write it again"
        );
    }

    #[test]
    fn a_page_shows_the_new_theme_after_a_switch() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            run_script_mod(vm, Choice::Base(0));
            // The canvas, opening the page the catalogue starts on.
            super::evaluate_for(vm, PAGE);
        });
        let dark = label_color(&mut cx);

        // The switch, in the order the platform runs it: the whole
        // `script_mod` chain again under the new theme, and then the canvas
        // rebuilding its page on the `LiveEdit` that follows.
        cx.with_vm(|vm| vm.with_reload(|vm| run_script_mod(vm, Choice::Base(1))));
        cx.with_vm(|vm| {
            super::evaluate_for(vm, PAGE);
        });
        let light = label_color(&mut cx);

        assert_ne!(
            dark, light,
            "the page came back painted in the theme it was switched away from"
        );
        assert_eq!(
            super::evaluated_count(&mut cx),
            1,
            "and the switch still paid for only the one page on screen"
        );
    }
}

#[cfg(test)]
mod cost_of_a_switch {
    //! A theme switch re-runs every module that read a theme token. This says
    //! where that time goes.
    //!
    //! Each measurement gets a FRESH `Cx`. Measured one after another in one
    //! context they are meaningless: every reload inherits what the last one
    //! left behind, so whichever ran first looks cheap and the rest look
    //! ruinous. The first attempt at this put the library at 0.9 s and the
    //! stories at 59 s that way, against a switch that really takes about 0.8 s.
    use crate::makepad_widgets::*;
    use std::time::Instant;

    /// How much of the catalogue a reload evaluates.
    #[derive(Clone, Copy)]
    enum Stories {
        /// The widget library alone.
        None,
        /// The one page the canvas is showing, which is what a switch costs
        /// now.
        Shown,
        /// Every story file, which is what a switch cost before they were
        /// evaluated lazily.
        All,
    }

    fn one_reload(stories: Stories) -> f64 {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut ms = 0.0;
        cx.with_vm(|vm| {
            crate::makepad_widgets::script_mod(vm);
            warm(vm, stories);
            let t = Instant::now();
            vm.with_reload(|vm| {
                crate::makepad_widgets::script_mod(vm);
                // Throws the record away; evaluates nothing itself.
                super::script_mod(vm);
                warm(vm, stories);
            });
            ms = t.elapsed().as_secs_f64() * 1000.0;
        });
        ms
    }

    fn warm(vm: &mut ScriptVm, stories: Stories) {
        match stories {
            Stories::None => {}
            Stories::Shown => {
                super::evaluate_for(vm, "DropControlsOverview");
            }
            Stories::All => super::evaluate_all(vm),
        }
    }

    /// Slow (minutes) and its ABSOLUTE numbers do not reconcile with a live
    /// switch -- in the app the same reload reports about 0.8 s, here about
    /// 50 s -- so read it for the RATIO only, which holds across every shape
    /// this was measured in: every story file is ~99% of a reload, the whole
    /// widget library is the other 1%, and the one page on the canvas is a
    /// fraction of one file.
    /// `cargo test -p makepad-storybook where_a_theme_switch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn where_a_theme_switch_spends_its_time() {
        let lib = one_reload(Stories::None);
        let all = one_reload(Stories::All);
        let shown = one_reload(Stories::Shown);
        let share = |total: f64| if total > 0.0 { (total - lib) / total * 100.0 } else { 0.0 };
        println!("COST library alone            {lib:>9.1} ms");
        println!(
            "COST library + all {} files {all:>9.1} ms | the stories' share {:>6.1} %",
            super::modules().len(),
            share(all)
        );
        println!(
            "COST library + the page shown {shown:>9.1} ms | the story's share  {:>6.1} %",
            share(shown)
        );
        println!("COST a switch is now {:>6.1}x cheaper", if shown > 0.0 { all / shown } else { 0.0 });
    }
}
