//! The app shell: a toolbar, the navigator, the canvas and the side panels.
use crate::actions::*;
use crate::canvas::*;
use crate::controls::*;
use crate::docs::*;
use crate::makepad_widgets::*;
use crate::navigator::*;
use crate::registry;
use crate::remote;
use crate::settings;
use crate::theme;
use crate::theme_panel::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    /** The row across the top. A template of its own, and not part of the
     * shell below, because the app measures it: what gives way when the row
     * runs out of width is decided from the row alone. */
    mod.storybook.CatalogueToolbar = ConcedingRow{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
        padding: theme.mspace_2

        // A stated width, not Fit. A label that can wrap shares the
        // row's leftover with the Filler at the end, half each, so
        // the title broke into lines as soon as the row filled up.
        catalogue_title := H4{
            text: "Widget catalogue"
            width: 200.
            give_up := 2
            tight: { visible: false }
        }
        story_search := TextInput{
            width: 160.
            empty_text: "Search stories"
        }
        // The two ways of reading the search text sit next to it:
        // step through the matches, or take everything else away.
        find_prev := ButtonFlat{text: "▲"}
        find_next := ButtonFlat{text: "▼"}
        match_count := Label{text: ""}
        search_filter := Toggle{text: "Filter"}
        // New means the widget arrived within this many days of
        // today. Once the person sets the number the marker moves
        // with the calendar; until then the field shows the days
        // since the catalogue was started, so the default marker
        // stays on that date. The count after the field says the
        // same thing in a sentence.
        new_only := Toggle{
            text: "New only"
            give_up := 4
            tight: { text: "New" }
        }
        new_days := NumberField{
            width: 84.
            min: 1.0
            max: 3650.0
            step: 1.0
            suffix: " days"
        }
        new_count := Label{
            text: ""
            give_up := 3
            tight: { visible: false }
        }
        theme_select := DropDown{
            labels: ["Dark" "Light" "Skeleton"]
            selected_item: 0
        }
        inspect := ButtonFlat{
            text: "Inspect"
            give_up := 1
            tight: { text: "<>" }
        }
        // One command with two faces: the word while the row has the width
        // for it, the mark when it has not. The mark is a button preset of
        // its own rather than this one emptied of its text, because a
        // button with a label keeps the gap in front of one whether or not
        // there is anything in it, and the mark sits off to one side.
        reset := ButtonFlat{
            text: "Reset"
            give_up := 1
            tight: { visible: false }
        }
        reset_icon := ButtonFlatIcon{
            visible: false
            give_up := 1
            tight: { visible: true }
            icon_walk: Walk{width: 14. height: Fit}
            draw_icon +: {
                svg: crate_resource("self:resources/reset.svg")
                color: theme.color_label_inner
            }
        }
        // What the row has left over, and the only number the fitting reads.
        // A Fill takes what the others have not, so the story title to its
        // right is already counted in it.
        slack := Filler{}
        story_title := Label{text: ""}
    }

    let Shell = View{
        width: Fill
        height: Fill
        flow: Down

        toolbar := mod.storybook.CatalogueToolbar{}

        split := Splitter{
            axis: SplitterAxis.Horizontal
            align: SplitterAlign.FromA(260.)
            a: View{
                width: Fill
                height: Fill
                flow: Down
                navigator := mod.storybook.StoryNavigator{}
                // What the switches keep out of the tree for the search
                // in the box, so a search that finds nothing says why.
                // Shown only while it has something to say.
                hidden_note := Label{
                    visible: false
                    width: Fill
                    padding: theme.mspace_2
                    text: ""
                }
            }
            b: Splitter{
                axis: SplitterAxis.Horizontal
                align: SplitterAlign.FromB(380.)
                a: View{
                    width: Fill
                    height: Fill
                    padding: theme.mspace_2
                    canvas := mod.storybook.StoryCanvas{}
                }
                b: View{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: theme.space_2
                    padding: theme.mspace_2
                    tabs := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: theme.space_1
                        tab_docs := RadioButtonTab{
                            text: "Docs"
                            animator +: {active: {default: @on}}
                        }
                        tab_controls := RadioButtonTab{text: "Controls"}
                        tab_actions := RadioButtonTab{text: "Actions"}
                        tab_tokens := RadioButtonTab{text: "Theme"}
                    }
                    // Every panel is built up front: the app writes into them
                    // before they are shown, and a page that does not exist
                    // yet swallows the write.
                    panels := PageFlip{
                        width: Fill
                        height: Fill
                        active_page: @docs
                        docs := mod.storybook.DocsPanel{}
                        controls := mod.storybook.ControlsPanel{}
                        actions := mod.storybook.ActionsPanel{}
                        // Not `theme`: that name is the token table every
                        // sibling here reads its spacing from.
                        tokens := mod.storybook.ThemePanel{}
                    }
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400 900)
                body +: {
                    flow: Down
                    spacing: 0.
                    margin: 0.
                    shell := Shell{}
                }
            }
        }
    }
}

const PANELS: &[LiveId] = &[live_id!(docs), live_id!(controls), live_id!(actions), live_id!(tokens)];

/// The order the toolbar gives way in, cheapest loss first: the two commands
/// at the end shrink to their marks, then the title goes, then the count
/// beside the new-only switch, then that switch's name shortens. A step is
/// taken only when the one before it was not enough, and given back in the
/// reverse order as the row is handed its width back.
const STEP_MARKS: usize = 0;
const STEP_TITLE: usize = 1;
const STEP_NEW_COUNT: usize = 2;
const STEP_NEW_ONLY: usize = 3;
const TOOLBAR_STEPS: usize = 4;

/// The width the row keeps beyond what it needs. A Fill is never narrower
/// than nothing, so with no margin at all an exactly full row and a badly
/// overfull one read the same and the row has no threshold it can observe;
/// a few points is a gap both the eye and the measurement can tell from none.
const TOOLBAR_MARGIN: f64 = 6.0;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// The key of the story on the canvas.
    #[rust]
    current: Option<String>,
}

impl App {
    fn current(&self) -> Option<&'static registry::Story> {
        self.current.as_deref().and_then(registry::find)
    }

    fn open_story(&mut self, cx: &mut Cx, key: &str) {
        let Some(story) = registry::find(key) else {
            log!("storybook: no story {}", key);
            return;
        };
        // An old key opens the page its content went to; the settings, the
        // navigator and the remote state all take the live key from here on.
        if story.key != key {
            log!("storybook: {} has moved to {}", key, story.key);
        }
        self.current = Some(story.key.to_string());
        self.ui.story_canvas(cx, ids!(canvas)).open(cx, story.dsl);
        self.settle_story(cx);
        // Selecting the row opens the folders over it; one the person
        // had closed is closed no longer, and the settings say so.
        if self.ui.story_navigator(cx, ids!(navigator)).select(cx, story.key) {
            self.persist_folds(cx);
        }
        self.ui.docs_panel(cx, ids!(docs)).set_story(cx, story);
        self.ui.controls_panel(cx, ids!(controls)).set_story(cx, story);
        self.ui.actions_panel(cx, ids!(actions)).clear(cx);
        self.ui
            .label(cx, ids!(story_title))
            .set_text(cx, &format!("{} / {}", story.component, story.name));
        settings::set(settings::LAST_STORY, story.key);
        remote::set_current(story.key);
        log!("storybook: story {}", story.key);
    }

    /// Whether the reset was pressed, whichever of its two faces the row
    /// has the width for.
    fn reset_pressed(&self, cx: &mut Cx, actions: &Actions) -> bool {
        self.ui.button_set(cx, ids_array!(reset, reset_icon)).clicked(actions)
    }

    fn drain_requests(&mut self, cx: &mut Cx) {
        for request in remote::take_requests() {
            match request {
                remote::Request::Open(key) => self.open_story(cx, &key),
                remote::Request::Theme(index) => theme::select(cx, index),
                remote::Request::Reset => {
                    self.ui.story_canvas(cx, ids!(canvas)).reset(cx);
                    self.settle_story(cx);
                    self.ui.controls_panel(cx, ids!(controls)).reset(cx);
                    self.ui.actions_panel(cx, ids!(actions)).clear(cx);
                }
            }
        }
    }

    /// Run the story's handler once with no actions, straight after its page
    /// is built. What only a host can put on a page, such as the rows an
    /// inspector lists or the value a toolbar number starts at, is put there
    /// on this pass, so the page opens with it rather than waiting for the
    /// first action anything on it happens to raise. A handler reads
    /// actions, so with none it only does that setting up.
    fn settle_story(&self, cx: &mut Cx) {
        let Some(on_actions) = self.current().and_then(|story| story.on_actions) else {
            return;
        };
        if let Some(root) = self.ui.story_canvas(cx, ids!(canvas)).shown_root() {
            on_actions(cx, &root, &[]);
        }
    }

    /// Hand the docs panel the story's subject once the canvas has built
    /// the story; the panel reads it once per build.
    fn refresh_subject(&self, cx: &mut Cx) {
        let Some(story) = self.current() else {
            return;
        };
        let canvas = self.ui.story_canvas(cx, ids!(canvas));
        let subject = canvas.shown_root().map(|root| {
            if story.subject.is_empty() {
                root
            } else {
                root.widget(cx, &id_path(story.subject))
            }
        });
        let subject = subject.filter(|w| !w.is_empty());
        self.ui.docs_panel(cx, ids!(docs)).set_subject(cx, subject.as_ref());
    }

    /// Take the walk one step and show what it landed on. Opening the
    /// story selects its row, which is what scrolls the tree to it, so
    /// finding and opening are the same movement.
    fn step_match(&mut self, cx: &mut Cx, delta: isize) {
        let key = self
            .ui
            .story_navigator(cx, ids!(navigator))
            .step_match(cx, delta);
        if let Some(key) = key {
            self.open_story(cx, key);
        }
        self.refresh_match_count(cx);
    }

    /// How many stories the search text picks out, and which one the
    /// arrows are on. Empty while the box is empty: a count of nothing
    /// beside an empty box is noise. The line under the tree moves with
    /// it: what the switches keep out of the same search.
    fn refresh_match_count(&self, cx: &mut Cx) {
        let navigator = self.ui.story_navigator(cx, ids!(navigator));
        let report = navigator.match_report();
        self.ui.label(cx, ids!(match_count)).set_text(cx, &report);
        let hidden = navigator.hidden_line();
        let note = self.ui.label(cx, ids!(hidden_note));
        note.set_text(cx, &hidden);
        note.set_visible(cx, !hidden.is_empty());
    }

    fn refresh_new_count(&self, cx: &mut Cx) {
        let navigator = self.ui.story_navigator(cx, ids!(navigator));
        let n = navigator.new_count();
        self.ui
            .label(cx, ids!(new_count))
            .set_text(cx, &new_count_line(n, settings::new_days()));
    }

    /// Show the reach in the field. Told on startup and again after a
    /// live edit rebuilt the toolbar from its template.
    fn refresh_new_days(&self, cx: &mut Cx) {
        self.ui
            .number_field(cx, ids!(new_days))
            .set_value(cx, settings::new_days() as f64);
    }

    /// Move the reach: the baseline the navigator marks against is
    /// today less this many days, and every count reads from it.
    fn set_new_days(&mut self, cx: &mut Cx, days: u32) {
        settings::set(settings::NEW_DAYS, &days.to_string());
        self.ui
            .story_navigator(cx, ids!(navigator))
            .set_baseline(cx, &settings::baseline());
        self.refresh_new_count(cx);
        self.refresh_match_count(cx);
    }

    /// Write the closed folders down when they differ from what is
    /// written; every open or close of a folder comes through here.
    fn persist_folds(&self, cx: &mut Cx) {
        let folded = self.ui.story_navigator(cx, ids!(navigator)).folded();
        if settings::get(settings::FOLDED).unwrap_or_default() != folded {
            settings::set(settings::FOLDED, &folded);
        }
    }

    fn apply_control(&self, cx: &mut Cx, control: &registry::Control, value: &ControlValue) {
        let canvas = self.ui.story_canvas(cx, ids!(canvas));
        if let registry::ControlKind::Disabled { .. } = control.kind {
            if let (Some(root), ControlValue::Bool(on)) = (canvas.shown_root(), value) {
                let target = if control.target.is_empty() {
                    root
                } else {
                    root.widget(cx, &id_path(control.target))
                };
                target.set_disabled(cx, *on);
                target.redraw(cx);
            }
            return;
        }
        let Some(chunk) = chunk_for(control, value) else {
            return;
        };
        if let Err(e) = canvas.apply(cx, control.target, prop_of(control), &chunk) {
            log!("storybook: control {} on {}: {}", control.label, control.target, e);
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let baseline = settings::baseline();
        let navigator = self.ui.story_navigator(cx, ids!(navigator));
        navigator.set_baseline(cx, &baseline);
        self.refresh_new_days(cx);
        // Before the first draw, which is the one that sets the folders.
        // Folders a moved page left behind are closed where it went, and
        // the line is written again when that changed it.
        navigator.set_folded(cx, &settings::get(settings::FOLDED).unwrap_or_default());
        self.persist_folds(cx);
        // Filtering unless the person turned it off last time. Told to
        // the switch as well as the navigator, for the reason the
        // new-only switch below says at length.
        let filtering = settings::get(settings::SEARCH_FILTER).as_deref() != Some("0");
        navigator.set_filtering(cx, filtering);
        self.ui
            .check_box(cx, ids!(search_filter))
            .set_active(cx, filtering, Animate::No);
        if settings::get(settings::NEW_ONLY).as_deref() == Some("1") {
            navigator.set_new_only(cx, true);
            // The switch has to be told as well as the navigator. Setting
            // only the navigator leaves the filter running behind a control
            // that reads OFF: two thirds of the catalogue is missing, the
            // search finds nothing for any widget older than the baseline,
            // and there is nothing on screen to say why. The first press
            // then sets it to the value it already had, so it takes two to
            // get out.
            self.ui
                .check_box(cx, ids!(new_only))
                .set_active(cx, true, Animate::No);
        }
        // The list is the library's, read at startup: the base themes and
        // every sheet it ships. The markup above names only the three it
        // could not do without if this never ran.
        let theme_select = self.ui.drop_down(cx, ids!(theme_select));
        theme_select.set_labels(cx, theme::labels());
        theme_select.set_selected_item(cx, theme::choice());
        self.refresh_new_count(cx);
        let key = match settings::get(settings::LAST_STORY) {
            Some(k) if registry::find(&k).is_some() => k,
            _ => "overview/welcome/welcome".to_string(),
        };
        self.open_story(cx, &key);
        remote::install(cx);
        log!("storybook: {} stories registered", registry::all().count());
        log!(
            "storybook: {} search terms over {} components",
            crate::synonyms::term_count(),
            crate::synonyms::component_count()
        );
    }

    /// Everything that can crowd the top row ends in a draw of it -- the
    /// window resized, the splitter moved, a longer story title beside the
    /// Filler -- so the row is read after a draw and nowhere else.
    fn handle_draw(&mut self, cx: &mut Cx, _e: &DrawEvent) {
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(key) = self.ui.story_navigator(cx, ids!(navigator)).opened(actions) {
            self.open_story(cx, &key);
        }
        if self.ui.story_navigator(cx, ids!(navigator)).folded_changed(actions) {
            self.persist_folds(cx);
        }
        if let Some(text) = self.ui.text_input(cx, ids!(story_search)).changed(actions) {
            self.ui.story_navigator(cx, ids!(navigator)).set_filter(cx, &text);
            self.refresh_match_count(cx);
        }
        // Return walks the matches, the way a find bar does, so holding
        // it down goes through them one at a time without reaching for
        // the arrows. The arrows do the same thing in both directions.
        if self.ui.text_input(cx, ids!(story_search)).returned(actions).is_some() {
            self.step_match(cx, 1);
        }
        if self.ui.button(cx, ids!(find_next)).clicked(actions) {
            self.step_match(cx, 1);
        }
        if self.ui.button(cx, ids!(find_prev)).clicked(actions) {
            self.step_match(cx, -1);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(search_filter)).changed(actions) {
            self.ui.story_navigator(cx, ids!(navigator)).set_filtering(cx, on);
            settings::set(settings::SEARCH_FILTER, if on { "1" } else { "0" });
            self.refresh_match_count(cx);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(new_only)).changed(actions) {
            self.ui.story_navigator(cx, ids!(navigator)).set_new_only(cx, on);
            settings::set(settings::NEW_ONLY, if on { "1" } else { "0" });
            // It narrows what matches, so the count moves with it.
            self.refresh_match_count(cx);
        }
        if let Some(days) = self.ui.number_field(cx, ids!(new_days)).changed(actions) {
            // The field bounds it at one already; the rounding is for a
            // value that arrived by a route the field did not quantise.
            self.set_new_days(cx, days.round().max(1.0) as u32);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(theme_select)).selected(actions) {
            theme::select(cx, index);
            // A new theme is a new set of values; the panel reads it again
            // rather than showing the old one's numbers under the new one's
            // colours.
            self.ui.theme_panel(cx, ids!(tokens)).reread(cx);
        }
        if let Some(index) = self
            .ui
            .radio_button_set(cx, ids_array!(tab_docs, tab_controls, tab_actions, tab_tokens))
            .selected(cx, actions)
        {
            if let Some(page) = PANELS.get(index) {
                self.ui.page_flip(cx, ids!(panels)).set_active_page(cx, *page);
            }
        }
        if self.ui.button(cx, ids!(inspect)).clicked(actions) {
            crate::makepad_widgets::tweaker::set_tweak_on(cx, true);
        }
        if self.reset_pressed(cx, actions) {
            self.ui.story_canvas(cx, ids!(canvas)).reset(cx);
            self.settle_story(cx);
            self.ui.controls_panel(cx, ids!(controls)).reset(cx);
            self.ui.actions_panel(cx, ids!(actions)).clear(cx);
        }
        for (control, value) in self.ui.controls_panel(cx, ids!(controls)).changed(actions) {
            self.apply_control(cx, control, &value);
        }
        if let Some(story) = self.current() {
            if let Some(on_actions) = story.on_actions {
                if let Some(root) = self.ui.story_canvas(cx, ids!(canvas)).shown_root() {
                    on_actions(cx, &root, actions);
                }
            }
        }
        let raised = self.ui.story_canvas(cx, ids!(canvas)).take_log();
        if !raised.is_empty() {
            self.ui.actions_panel(cx, ids!(actions)).push(cx, raised);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // Restore the saved theme ONCE, at startup. Done on every run, as it
        // was, this made the catalogue's own list the last writer on every
        // reload: the developer panel's theme picker was undone a tick after
        // it was used, and so would anything else that set a theme.
        static RESTORED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !RESTORED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            if let Some(name) = settings::get(settings::THEME) {
                if let Some(index) = theme::index_of(&name) {
                    theme::set_choice(index);
                    let picked = theme::choices()[index];
                    theme::apply_choice(vm, picked);
                }
            }
        }
        theme::widgets_script_mod(vm);
        makepad_code_editor::script_mod(vm);
        crate::shell::script_mod(vm);
        crate::canvas::script_mod(vm);
        crate::navigator::script_mod(vm);
        crate::docs::script_mod(vm);
        crate::controls::script_mod(vm);
        crate::actions::script_mod(vm);
        crate::theme_panel::script_mod(vm);
        // Evaluates no story file. It throws away the record of which files
        // this context has already evaluated -- `shell` above has just
        // emptied `mod.stories`, and every page that was in it belonged to
        // the theme being left. The canvas evaluates the one page it is
        // showing on the rebuild that follows.
        crate::stories::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::LiveEdit = event {
            let choice = theme::choice();
            // A switch re-applies the whole tree from its markup, and the
            // markup knows three names: the list has to go back on before
            // the fourteenth of them can be the one that is selected.
            let theme_select = self.ui.drop_down(cx, ids!(theme_select));
            theme_select.set_labels(cx, theme::labels());
            theme_select.set_selected_item(cx, choice);
            remote::install(cx);
        }
        self.drain_requests(cx);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // The tree first, then the app and the story's own handler: a
        // handler that asks a widget what state it is in has to be asking
        // after that widget has dealt with the same actions, or every
        // readout in a story is a step behind what the screen shows.
        self.match_event(cx, event);
        // After the tree has handled the event its index is current, so the
        // subject lookup lands on the story just built rather than on what
        // the index still held from before.
        self.refresh_subject(cx);
        if let Event::LiveEdit = event {
            // Everything was rebuilt from its templates; the panels are plain
            // state the rebuild reset, so they get their story again.
            if let Some(story) = self.current() {
                self.ui
                    .label(cx, ids!(story_title))
                    .set_text(cx, &format!("{} / {}", story.component, story.name));
                self.ui.docs_panel(cx, ids!(docs)).set_story(cx, story);
            }
            // The canvas rebuilt the page from its template in the tree's
            // pass above, blank of what its handler had put on it.
            self.settle_story(cx);
            self.refresh_new_days(cx);
            self.refresh_new_count(cx);
            // The match count and the hidden-matches line under the
            // tree were rebuilt blank too; without this a filtered tree
            // says nothing about why after a theme switch.
            self.refresh_match_count(cx);
            // The toolbar came back from its markup having given up
            // nothing, whatever the ladder had settled on, and a theme
            // carries its own type: what the steps were worth under the
            // last one is worth nothing here. It starts again from the
            // state the markup is in, and the next draw walks it down.
        }
    }
}

/// The count beside the switch: how many are new, and over how long.
fn new_count_line(new: usize, days: u32) -> String {
    match days {
        1 => format!("{new} new in the last day"),
        days => format!("{new} new in the last {days} days"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_widgets::makepad_draw::cx_draw::CxDraw;

    #[test]
    fn the_new_count_says_how_far_back_it_looked() {
        assert_eq!(new_count_line(12, 30), "12 new in the last 30 days");
        assert_eq!(new_count_line(0, 7), "0 new in the last 7 days");
        assert_eq!(new_count_line(3, 1), "3 new in the last day");
    }

    /// The row on its own, in an app that has nothing else. It is built
    /// from the same template the shell builds it from, so a control
    /// renamed or dropped there is renamed or dropped here.
    fn fixture() -> (Cx, App, DrawPass, DrawList2d) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let ui = cx.with_vm(|vm| {
            <App as AppMain>::script_mod(vm);
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    toolbar := mod.storybook.CatalogueToolbar{}
                }
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });
        let pass = DrawPass::new(&mut cx);
        let list = DrawList2d::new(&mut cx);
        let app = App {
            ui,
            current: None,
        };
        (cx, app, pass, list)
    }

    /// One draw of the row at `width`.
    fn draw(cx: &mut Cx, app: &App, pass: &DrawPass, list: &mut DrawList2d, width: f64) {
        let size = dvec2(width, 120.0);
        pass.set_size(cx, size);
        cx.redraw_all();
        let event = std::mem::take(&mut cx.new_draw_event);
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, Some(1.0));
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_down());
        app.ui.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }

    /// Draw until the row stops changing what it says. Answers how many
    /// draws it took, which for a row that can price its own rungs is one.
    ///
    /// The bound and the panic stay: they are what stands between a row that
    /// flaps and a test that hangs, and a row deciding from its own
    /// measurements is not by itself a proof that it cannot.
    fn settle(cx: &mut Cx, app: &mut App, pass: &DrawPass, list: &mut DrawList2d, width: f64) -> usize {
        draw(cx, app, pass, list, width);
        let mut was = shown(cx, app);
        for draws in 1..=16 {
            draw(cx, app, pass, list, width);
            let now = shown(cx, app);
            if now == was {
                return draws;
            }
            was = now;
        }
        panic!("the toolbar never settled at {width}");
    }

    /// Which rung the row has settled on, read off what it says rather than
    /// off a number it keeps. The number is the row's own business now.
    fn rung(cx: &Cx, app: &App) -> usize {
        let now = shown(cx, app);
        (0..=TOOLBAR_STEPS)
            .find(|l| expected(*l) == now)
            .unwrap_or_else(|| panic!("the row is in no state the ladder describes: {now:?}"))
    }

    /// What the row says, read off the controls rather than off the ladder.
    #[derive(Debug, PartialEq)]
    struct Shown {
        title: bool,
        inspect: String,
        reset: String,
        mark: bool,
        count: bool,
        new_only: String,
    }

    fn shown(cx: &Cx, app: &App) -> Shown {
        let at = |path: &[LiveId]| app.ui.widget(cx, path);
        Shown {
            title: at(ids!(catalogue_title)).visible(),
            inspect: at(ids!(inspect)).text(),
            reset: if at(ids!(reset)).visible() { at(ids!(reset)).text() } else { String::new() },
            mark: at(ids!(reset_icon)).visible(),
            count: at(ids!(new_count)).visible(),
            new_only: at(ids!(new_only)).text(),
        }
    }

    /// What the row is meant to say with `level` of its steps taken: the
    /// resting state, then one thing given up at a time, in the order they
    /// were asked for.
    ///
    /// The rungs are written out here rather than read from the constants
    /// the row is wired up from, so that a ladder reordered stays a ladder
    /// this disagrees with.
    fn expected(level: usize) -> Shown {
        Shown {
            title: level < 2,
            inspect: if level >= 1 { "<>" } else { "Inspect" }.to_string(),
            reset: if level >= 1 { "" } else { "Reset" }.to_string(),
            mark: level >= 1,
            count: level < 3,
            new_only: if level >= 4 { "New" } else { "New only" }.to_string(),
        }
    }

    /// The widest the row is asked to be: the window's own starting width.
    const WIDE: f64 = 1400.0;

    /// The resting state, and that reaching it costs one draw. Catches a
    /// row that concedes something it did not have to, and a row that
    /// keeps re-deciding at a width where nothing is wrong.
    #[test]
    fn a_row_with_room_to_spare_gives_up_nothing() {
        let (mut cx, mut app, pass, mut list) = fixture();
        let draws = settle(&mut cx, &mut app, &pass, &mut list, WIDE);
        assert_eq!(draws, 1, "a row with room to spare was laid out twice");
        assert_eq!(rung(&cx, &app), 0);
        assert_eq!(shown(&cx, &app), expected(0));
    }

    /// Each step in turn as the row is narrowed, and nothing skipped or
    /// taken out of order. Catches a ladder wired up in the wrong order,
    /// a step that fires while the one before it would have done, and a
    /// step whose controls do not actually change when it is taken.
    #[test]
    fn the_row_gives_way_one_step_at_a_time_in_the_order_it_was_asked_to() {
        let (mut cx, mut app, pass, mut list) = fixture();
        let mut level = 0;
        let mut taken = Vec::new();
        let mut width = WIDE;
        while width >= 300.0 {
            settle(&mut cx, &mut app, &pass, &mut list, width);
            let now = rung(&cx, &app);
            assert!(now >= level, "the row gave something back as it was narrowed, at {width}");
            assert_eq!(shown(&cx, &app), expected(now), "at {width}");
            if now != level {
                taken.push(now);
                level = now;
            }
            width -= 5.0;
        }
        assert_eq!(taken, vec![1, 2, 3, 4], "the steps, in the order they were taken");
    }

    /// And back up the ladder as the room comes back, to the same states at
    /// the same widths. Catches a row that concedes and never restores, and
    /// one that settles somewhere else depending on which way the window was
    /// dragged -- which on a slow drag reads as the toolbar lagging a step
    /// behind the mouse.
    #[test]
    fn the_row_takes_its_concessions_back_as_it_is_given_room() {
        let (mut cx, mut app, pass, mut list) = fixture();
        let widths: Vec<f64> = (0..111).map(|step| WIDE - 10.0 * step as f64).collect();
        let mut down = Vec::new();
        for width in &widths {
            settle(&mut cx, &mut app, &pass, &mut list, *width);
            down.push(rung(&cx, &app));
        }
        assert_eq!(down.last(), Some(&TOOLBAR_STEPS), "the narrow end gave up everything");
        let mut up = Vec::new();
        for width in widths.iter().rev() {
            settle(&mut cx, &mut app, &pass, &mut list, *width);
            up.push(rung(&cx, &app));
        }
        up.reverse();
        assert_eq!(up, down, "the row settled elsewhere coming back up");
        assert_eq!(shown(&cx, &app), expected(0), "the widest row is the resting one again");
    }

    /// A row that has settled must cost nothing per draw: the measuring is
    /// there to stop the row jumping about, and a fit that keeps finding
    /// work is the jitter itself.
    #[test]
    fn a_settled_row_changes_nothing_on_the_draws_that_follow() {
        let (mut cx, mut app, pass, mut list) = fixture();
        for width in [WIDE, 900.0, 800.0, 700.0, 600.0, 500.0, 400.0] {
            settle(&mut cx, &mut app, &pass, &mut list, width);
            let settled = shown(&cx, &app);
            for _ in 0..3 {
                draw(&mut cx, &app, &pass, &mut list, width);
                assert_eq!(shown(&cx, &app), settled, "the row went on deciding at {width}");
            }
        }
    }
    /// A row decides from what it measures, not from what it drew last time,
    /// so the state that used to be dangerous is now merely uninteresting.
    ///
    /// The old loop read the row AFTER a draw. A tree not yet laid out answered
    /// nothing to every width asked of it, and a row with no slack looks
    /// exactly like an overfull one -- so it walked the whole ladder down and
    /// priced every step it took at nothing, which is a step it is never
    /// allowed to give back. A style reload handed the app a tree in that
    /// state, so it was not a hypothetical opening frame.
    ///
    /// Pricing before the draw removes that rather than guarding it: a row that
    /// has never drawn has nothing to read back and nothing to be misled by.
    /// What is left worth checking is that it still gives way, and still gives
    /// back.
    #[test]
    fn a_row_prices_itself_from_scratch_rather_than_from_its_last_layout() {
        let (mut cx, mut app, pass, mut list) = fixture();
        // Never drawn: the authored face, nothing given up.
        assert_eq!(shown(&cx, &app), expected(0), "an undrawn row gave something up");

        settle(&mut cx, &mut app, &pass, &mut list, 700.0);
        assert!(rung(&cx, &app) > 0, "the narrow row gave nothing up");

        settle(&mut cx, &mut app, &pass, &mut list, WIDE);
        assert_eq!(rung(&cx, &app), 0, "a step priced at nothing is a step kept for good");
        assert_eq!(shown(&cx, &app), expected(0));
    }

    /// One draw settles it, however many rungs it owes.
    ///
    /// This is what pricing before the draw buys, and it is the whole
    /// difference from the loop this replaced. That one decided AFTER the draw
    /// which had already shown the old state, and could only learn what a rung
    /// saved by taking it and measuring the result -- so falling four rungs
    /// took four draws, and each one had to ask for the next with
    /// `cx.redraw_all()`, because inside a draw event every redraw is refused
    /// but the whole-window one. Deciding first means the draw that follows is
    /// already the right one, and there is nothing to ask for.
    #[test]
    fn a_row_that_gives_way_needs_no_second_draw() {
        let (mut cx, mut app, pass, mut list) = fixture();
        settle(&mut cx, &mut app, &pass, &mut list, WIDE);

        // Each of these owes more than the last, and the deepest owes several
        // rungs at once. Every one of them is one draw.
        for width in [900.0, 700.0, 500.0, 400.0, 300.0] {
            assert_eq!(
                settle(&mut cx, &mut app, &pass, &mut list, width),
                1,
                "the row needed more than one draw to settle at {width}"
            );
        }
        assert_ne!(shown(&cx, &app), expected(0), "nothing moved, so nothing was proved");

        // And the same coming back up, in one draw each.
        for width in [400.0, 600.0, 800.0, WIDE] {
            assert_eq!(
                settle(&mut cx, &mut app, &pass, &mut list, width),
                1,
                "the row needed more than one draw to give its faces back at {width}"
            );
        }
        assert_eq!(shown(&cx, &app), expected(0), "the faces did not all come back");
    }

    /// The reset is one command with two faces, and a press on either is
    /// the same press. Catches the mark being left out of what the app
    /// listens to, which is a button that looks live and does nothing.
    #[test]
    fn the_reset_answers_from_either_of_its_faces() {
        let (mut cx, mut app, pass, mut list) = fixture();
        settle(&mut cx, &mut app, &pass, &mut list, WIDE);
        for face in [ids!(reset), ids!(reset_icon)] {
            let widget = app.ui.widget(&cx, face);
            assert!(!widget.is_empty(), "the row has no such control");
            let actions: ActionsBuf = vec![Box::new(WidgetAction {
                data: None,
                action: Box::new(ButtonAction::Clicked(KeyModifiers::default())),
                widget_uid: widget.widget_uid(),
                group: None,
            })];
            assert!(app.reset_pressed(&mut cx, &actions), "a press went unanswered");
        }
    }

    /// What a named override block can actually change, and under which apply.
    ///
    /// A block is authored `tight: {...}` at the use site and declared
    /// `tight := {}` on the base. The colon form is only legal because the
    /// checked path falls back to the vec before erroring: a widget proto is
    /// frozen VALIDATED, so without the declaration this is a hard error at
    /// construction, not a warning and not a value quietly ignored.
    ///
    /// The variant matters more than anything else here. Under
    /// `Apply::ScriptReapply` -- what `request_style_reload` produces --
    /// `String`, `ArcStringMut` and every `#[visible]` field return early, so a
    /// block driven that way changes nothing on screen while passing any test
    /// written against `Apply::New`. That is how a feature ships as a no-op.
    #[test]
    fn a_compact_block_is_readable_and_only_some_applies_wear_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            <App as AppMain>::script_mod(vm);
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    probe := ButtonFlat{
                        text: "Inspect"
                        give_up := 1
                        tight: { text: "<>"  visible: false }
                    }
                }
            });
            assert!(
                vm.take_errors().is_empty(),
                "`tight:` must be legal on an instance once the base declares it"
            );
            let root = WidgetRef::script_from_value(vm, value);
            let probe = root.widget(vm.cx_mut(), ids!(probe));
            let src = probe.script_source();

            // The block is readable back by name off the instance.
            let block = vm.bx.heap.value(src, id!(tight).into(), NoTrap);
            assert!(block.is_object(), "tight must read back as an object");

            // The rank is an integer. `as_f64` answers None for an integer
            // literal, so a row reading ranks that way finds none of them and
            // builds no ladder, with no error anywhere.
            // A small integer literal is stored as U40, not I32 and not F64.
            // `as_i32`, `as_u32` and `as_f64` all answer None for it, so a row
            // reading ranks with any single-type accessor finds none of them and
            // builds no ladder, with no error anywhere. `as_number` is the only
            // accessor that covers every numeric representation.
            let rank = vm.bx.heap.value(src, id!(give_up).into(), NoTrap);
            assert_eq!(rank.as_number(), Some(1.0), "give_up reads through as_number");
            assert_eq!(rank.as_i32(), None, "and is NOT an i32");
            assert_eq!(rank.as_f64(), None, "and is NOT an f64");
            assert_eq!(rank.as_u40(), Some(1), "it is a u40");

            // The block sits in the vec, which the derived apply never reads,
            // so the button still says what it was authored to say.
            assert_eq!(probe.text(), "Inspect", "the block must not apply itself");

            // The table. Each variant against a button in its authored state.
            for (apply, want_text, want_visible) in [
                (Apply::New, "<>", false),
                (Apply::Animate, "<>", false),
                (Apply::Eval, "<>", false),
                (Apply::ScriptReapply, "Inspect", true),
                (Apply::Rebake, "Inspect", true),
            ] {
                let fresh = script_eval!(vm, {
                    use mod.prelude.widgets.*
                    use mod.widgets.*
                    ButtonFlat{ text: "Inspect" }
                });
                let mut fresh = WidgetRef::script_from_value(vm, fresh);
                fresh.script_apply(vm, &apply, &mut Scope::empty(), block);
                assert_eq!(fresh.text(), want_text, "text under {apply:?}");
                assert_eq!(
                    fresh.visible(),
                    want_visible,
                    "visible under {apply:?}"
                );
            }
        });
    }

    /// And that the row actually draws narrower for it.
    ///
    /// A field changing is not a pixel changing. Two features this month passed
    /// every test they had and did nothing on screen, so the block is measured
    /// here off the drawn rect rather than off the button it was applied to.
    #[test]
    fn a_compact_block_draws_narrower_than_the_face_it_replaces() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let (root, probe, block) = cx.with_vm(|vm| {
            <App as AppMain>::script_mod(vm);
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    probe := ButtonFlat{
                        text: "Inspect the widget"
                        tight: { text: "<>" }
                    }
                }
            });
            assert!(vm.take_errors().is_empty());
            let root = WidgetRef::script_from_value(vm, value);
            let probe = root.widget(vm.cx_mut(), ids!(probe));
            let block = vm.bx.heap.value(probe.script_source(), id!(tight).into(), NoTrap);
            (root, probe, block)
        });

        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let draw_once = |cx: &mut Cx, list: &mut DrawList2d| {
            let size = dvec2(600.0, 120.0);
            pass.set_size(cx, size);
            cx.redraw_all();
            let event = std::mem::take(&mut cx.new_draw_event);
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, Some(1.0));
            list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        };

        draw_once(&mut cx, &mut list);
        let long = probe.area().rect(&cx).size.x;
        assert!(long > 0.0, "the long face must draw with a width of its own");

        cx.with_vm(|vm| {
            let mut probe = probe.clone();
            probe.script_apply(vm, &Apply::Animate, &mut Scope::empty(), block);
        });
        draw_once(&mut cx, &mut list);
        let short = probe.area().rect(&cx).size.x;

        assert!(
            short < long,
            "the compact face must DRAW narrower, not merely say it is: {long} -> {short}"
        );
    }


    /// A measured width is the width that actually gets drawn.
    ///
    /// This is the claim the whole ladder rests on: a row prices every rung
    /// before it draws anything, so if `measure_width` and the turtle disagree
    /// the row settles on a number that is not the one on screen -- and prices a
    /// rung wrong, which is a rung it never gives back.
    #[test]
    fn a_measured_button_is_the_width_it_actually_draws() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            <App as AppMain>::script_mod(vm);
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    a := ButtonFlat{ text: "Inspect" }
                    b := ButtonFlat{ text: "<>" }
                    c := ButtonFlat{ text: "New only" }
                    d := ButtonFlat{ text: "Reset" }
                    e := Label{ width: Fit  text: "12 new in the last day" }
                }
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });

        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let size = dvec2(600.0, 400.0);
        pass.set_size(&mut cx, size);
        cx.redraw_all();

        let names = [id!(a), id!(b), id!(c), id!(d), id!(e)];
        let mut measured = Vec::new();
        {
            let event = std::mem::take(&mut cx.new_draw_event);
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, Some(1.0));
            list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            // Measured mid-pass, from the same Cx2d that is about to draw them.
            for n in names {
                let w = root.widget(cx2d.cx, &[n]);
                measured.push(w.measure_width(&mut cx2d, None));
            }
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }

        for (n, m) in names.into_iter().zip(measured) {
            let m = m.unwrap_or_else(|| panic!("{n:?} must price itself"));
            if n == id!(e) {
                // A Label's area is not its drawn width -- it reports a few
                // points wide whatever it renders -- so there is nothing here to
                // check a measurement against. That the label answers a width at
                // all, and one in the right order of magnitude for its string,
                // is all this can say.
                assert!(m > 80.0, "the label priced its own string at {m}");
                continue;
            }
            let drawn = root.widget(&cx, &[n]).area().rect(&cx).size.x;
            assert!(
                (m - drawn).abs() < 2.0,
                "{n:?}: measured {m} but drew {drawn}"
            );
        }
    }

    /// A row of its own, giving way as it is narrowed.
    ///
    /// The row prices every rung before it draws, so narrowing it past three
    /// rungs at once settles in ONE draw rather than one draw per rung. That is
    /// the whole point of measuring rather than taking a rung and looking.
    #[test]
    fn a_conceding_row_settles_in_one_draw_however_far_it_has_to_fall() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            <App as AppMain>::script_mod(vm);
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    row := ConcedingRow{
                        a := ButtonFlat{ text: "Inspect the widget"  give_up := 1  tight: { text: "<>" } }
                        b := ButtonFlat{ text: "Reset everything"    give_up := 2  tight: { visible: false } }
                        c := ButtonFlat{ text: "New only"            give_up := 3  tight: { text: "New" } }
                        d := ButtonFlat{ text: "Always here" }
                    }
                }
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });

        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let one = |cx: &mut Cx, list: &mut DrawList2d, w: f64| {
            let size = dvec2(w, 120.0);
            pass.set_size(cx, size);
            cx.redraw_all();
            let event = std::mem::take(&mut cx.new_draw_event);
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, Some(1.0));
            list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        };
        let face = |cx: &Cx| {
            (
                root.widget(cx, ids!(a)).text(),
                root.widget(cx, ids!(b)).visible(),
                root.widget(cx, ids!(c)).text(),
            )
        };

        // Wide: everything as authored.
        one(&mut cx, &mut list, 900.0);
        assert_eq!(
            face(&cx),
            ("Inspect the widget".to_string(), true, "New only".to_string()),
            "a row with room to spare must give up nothing"
        );

        // Narrow enough to need all three rungs, in ONE step.
        one(&mut cx, &mut list, 120.0);
        assert_eq!(
            face(&cx),
            ("<>".to_string(), false, "New".to_string()),
            "three rungs, one draw"
        );

        // A second draw at the same width must change nothing.
        let settled = face(&cx);
        one(&mut cx, &mut list, 120.0);
        assert_eq!(face(&cx), settled, "a settled row must stay settled");

        // And back up, to the same faces at the same widths.
        one(&mut cx, &mut list, 900.0);
        assert_eq!(
            face(&cx),
            ("Inspect the widget".to_string(), true, "New only".to_string()),
            "the row must give its faces back when the room returns"
        );
    }


    /// The same widths on the way down and on the way back up.
    ///
    /// Two things this catches that a single narrowing does not: a row that
    /// settles somewhere else depending on which way the window was dragged,
    /// which on a slow drag reads as the row lagging behind the mouse; and a row
    /// that flaps, where the face it puts on changes the width it measures and
    /// it changes its mind forever.
    #[test]
    fn a_conceding_row_reaches_the_same_face_going_down_and_coming_back() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            <App as AppMain>::script_mod(vm);
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    row := ConcedingRow{
                        a := ButtonFlat{ text: "Inspect the widget"  give_up := 1  tight: { text: "<>" } }
                        b := ButtonFlat{ text: "Reset everything"    give_up := 2  tight: { visible: false } }
                        c := ButtonFlat{ text: "New only"            give_up := 3  tight: { text: "New" } }
                        d := ButtonFlat{ text: "Always here" }
                    }
                }
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });

        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let mut one = |cx: &mut Cx, w: f64| {
            let size = dvec2(w, 120.0);
            pass.set_size(cx, size);
            cx.redraw_all();
            let event = std::mem::take(&mut cx.new_draw_event);
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, Some(1.0));
            list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        };
        let face = |cx: &Cx| {
            (
                root.widget(cx, ids!(a)).text(),
                root.widget(cx, ids!(b)).visible(),
                root.widget(cx, ids!(c)).text(),
            )
        };

        let widths: Vec<f64> = (0..80).map(|s| 700.0 - 8.0 * s as f64).collect();

        let mut down = Vec::new();
        for &w in &widths {
            one(&mut cx, w);
            down.push(face(&cx));
        }

        let mut up = Vec::new();
        for &w in widths.iter().rev() {
            one(&mut cx, w);
            up.push(face(&cx));
        }
        up.reverse();

        for (i, &w) in widths.iter().enumerate() {
            assert_eq!(
                down[i], up[i],
                "at {w} the row settled differently going down and coming back"
            );
        }

        // A drawn row must never change its mind on a draw that changed nothing.
        for &w in &widths {
            one(&mut cx, w);
            let settled = face(&cx);
            one(&mut cx, w);
            assert_eq!(face(&cx), settled, "the row flapped at {w}");
        }

        // And it must actually have given way somewhere across that range.
        assert!(
            down.first() != down.last(),
            "the row never gave anything up across 700 points of narrowing"
        );
    }

}
