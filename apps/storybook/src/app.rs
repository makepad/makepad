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
    mod.storybook.CatalogueToolbar = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
        padding: theme.mspace_2

        // A stated width, not Fit. A label that can wrap shares the
        // row's leftover with the Filler at the end, half each, so
        // the title broke into lines as soon as the row filled up.
        catalogue_title := H4{text: "Widget catalogue" width: 200.}
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
        new_only := Toggle{text: "New only"}
        new_days := NumberField{
            width: 84.
            min: 1.0
            max: 3650.0
            step: 1.0
            suffix: " days"
        }
        new_count := Label{text: ""}
        theme_select := DropDown{
            labels: ["Dark" "Light" "Skeleton"]
            selected_item: 0
        }
        inspect := ButtonFlat{text: "Inspect"}
        // One command with two faces: the word while the row has the width
        // for it, the mark when it has not. The mark is a button preset of
        // its own rather than this one emptied of its text, because a
        // button with a label keeps the gap in front of one whether or not
        // there is anything in it, and the mark sits off to one side.
        reset := ButtonFlat{text: "Reset"}
        reset_icon := ButtonFlatIcon{
            visible: false
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
    /// How far down its ladder the toolbar has had to go, and what each
    /// step turned out to save.
    #[rust(ConcessionLadder::new(TOOLBAR_STEPS, TOOLBAR_MARGIN))]
    toolbar: ConcessionLadder,
    /// A step taken and not yet priced: which one, and what the row was
    /// giving to the controls it changes before it was taken. The draw
    /// that follows says what it saved.
    #[rust]
    toolbar_pricing: Option<(usize, f64)>,
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

    /// What the row is giving to the controls step `step` changes, as it
    /// stands in the draw just made. A step's cost is this before it was
    /// taken less this after.
    ///
    /// A control that only changes what it says is measured by its own
    /// width: it keeps its place in the row either way. A control that goes
    /// away is measured as the room between the two controls it sits
    /// between, because it takes the gap in front of the next one with it,
    /// and a saving read short of what it really was is what makes a row
    /// give something back and immediately want it again. Measuring the
    /// room rather than the control also keeps the reading off the control
    /// itself, which is not on the row to be measured once the step is
    /// taken -- and, for a label, is not there to be measured while it is
    /// still waiting to be told what to say.
    ///
    /// Only true while the ladder and the pixels agree, which is where it
    /// is called from: before the level is moved, and on the pass after the
    /// draw that moved it.
    fn step_span(&self, cx: &Cx, step: usize) -> f64 {
        let rect = |path: &[LiveId]| self.ui.widget(cx, path).area().rect(cx);
        let x = |path: &[LiveId]| rect(path).pos.x;
        let w = |path: &[LiveId]| rect(path).size.x;
        match step {
            STEP_MARKS => {
                let reset = if self.toolbar.is_conceded(step) { ids!(reset_icon) } else { ids!(reset) };
                w(ids!(inspect)) + w(reset)
            }
            // The title is the first thing in the row, so the room in front
            // of the search box is measured from the row's own edge.
            STEP_TITLE => x(ids!(story_search)) - x(ids!(toolbar)),
            STEP_NEW_COUNT => x(ids!(theme_select)) - (x(ids!(new_days)) + w(ids!(new_days))),
            STEP_NEW_ONLY => w(ids!(new_only)),
            _ => 0.0,
        }
    }

    /// Read the row after a draw and settle how much of it has to give way.
    ///
    /// The Filler between the last command and the story title is what the
    /// row has left over, so that one width says whether the row fits, and
    /// says it with the title counted. Answers whether anything had to
    /// move: a row that has settled changes nothing and asks for no redraw,
    /// which is the whole of what keeps the measuring from becoming the
    /// jitter it is there to prevent.
    fn fit_toolbar(&mut self, cx: &mut Cx) -> bool {
        // A row that has not been laid out yet answers nothing to every
        // width asked of it, and a row with no slack is exactly what an
        // overfull one looks like. A style reload hands back a tree in
        // that state for a draw or two -- long enough for the row to walk
        // the whole ladder down on a measurement of zeroes, and to price
        // every step it took at nothing, which is a step it may never give
        // back. Nothing is read until the row has a width of its own.
        if self.ui.widget(cx, ids!(toolbar)).area().rect(cx).size.x <= 0.0 {
            return false;
        }
        // The step taken last time has been drawn since. The two readings
        // say what it saved, and a step whose saving is known is one the
        // row can safely give back later.
        if let Some((step, before)) = self.toolbar_pricing.take() {
            let saved = before - self.step_span(cx, step);
            self.toolbar.set_cost(step, saved);
        }
        let mut spans = [0.0; TOOLBAR_STEPS];
        for (step, span) in spans.iter_mut().enumerate() {
            *span = self.step_span(cx, step);
        }
        let slack = self.ui.widget(cx, ids!(slack)).area().rect(cx).size.x;
        let level = self.toolbar.level();
        if !self.toolbar.measured(slack) {
            return false;
        }
        let taken = self.toolbar.level();
        self.toolbar_pricing = (taken > level).then(|| (taken - 1, spans[taken - 1]));
        self.show_toolbar(cx);
        // The row is read inside the draw event, and a redraw asked for
        // from in there is refused unless it is the whole-window one --
        // so the redraws the sets above ask for on their own account are
        // dropped, and without this the row changes its mind once and
        // then never lays itself out to prove it. Asked for only on a
        // level that moved: a settled row goes on costing nothing.
        cx.redraw_all();
        true
    }

    /// Put the row in the state the ladder says it is in.
    fn show_toolbar(&self, cx: &mut Cx) {
        let marks = self.toolbar.is_conceded(STEP_MARKS);
        self.ui
            .button(cx, ids!(inspect))
            .set_text(cx, if marks { "<>" } else { "Inspect" });
        self.ui.button(cx, ids!(reset)).set_visible(cx, !marks);
        self.ui.button(cx, ids!(reset_icon)).set_visible(cx, marks);
        self.ui
            .widget(cx, ids!(catalogue_title))
            .set_visible(cx, !self.toolbar.is_conceded(STEP_TITLE));
        self.ui
            .label(cx, ids!(new_count))
            .set_visible(cx, !self.toolbar.is_conceded(STEP_NEW_COUNT));
        let new_only = if self.toolbar.is_conceded(STEP_NEW_ONLY) { "New" } else { "New only" };
        self.ui.widget(cx, ids!(new_only)).set_text(cx, new_only);
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
        self.fit_toolbar(cx);
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
            self.toolbar = ConcessionLadder::new(TOOLBAR_STEPS, TOOLBAR_MARGIN);
            self.toolbar_pricing = None;
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
            toolbar: ConcessionLadder::new(TOOLBAR_STEPS, TOOLBAR_MARGIN),
            toolbar_pricing: None,
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

    /// Draw and fit until the row stops moving, which is what the running
    /// app does: a fit that changes anything asks for another draw. Panics
    /// rather than spinning, so a row that flaps between two states fails
    /// the test instead of hanging it. Answers how many draws it took.
    fn settle(cx: &mut Cx, app: &mut App, pass: &DrawPass, list: &mut DrawList2d, width: f64) -> usize {
        for draws in 1..=16 {
            draw(cx, app, pass, list, width);
            if !app.fit_toolbar(cx) {
                return draws;
            }
        }
        panic!("the toolbar never settled at {width}");
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
        assert_eq!(app.toolbar.level(), 0);
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
            let now = app.toolbar.level();
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
            down.push(app.toolbar.level());
        }
        assert_eq!(down.last(), Some(&TOOLBAR_STEPS), "the narrow end gave up everything");
        let mut up = Vec::new();
        for width in widths.iter().rev() {
            settle(&mut cx, &mut app, &pass, &mut list, *width);
            up.push(app.toolbar.level());
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
            let level = app.toolbar.level();
            for _ in 0..3 {
                draw(&mut cx, &app, &pass, &mut list, width);
                assert!(!app.fit_toolbar(&mut cx), "the row went on deciding at {width}");
            }
            assert_eq!(app.toolbar.level(), level);
            assert_eq!(shown(&cx, &app), settled, "at {width}");
        }
    }

    /// Nothing is decided off a row that has never been laid out. Every
    /// width it is asked for comes back as nothing, and a row with no
    /// room left is precisely what that looks like -- so a row read in
    /// that state walks the whole ladder down for no reason and, worse,
    /// prices each step it takes at nothing, which is a step it is never
    /// allowed to give back. A style reload hands the app a tree in that
    /// state, so this is not a hypothetical opening frame.
    #[test]
    fn a_row_that_has_not_been_laid_out_is_not_read_at_all() {
        let (mut cx, mut app, pass, mut list) = fixture();
        for _ in 0..8 {
            assert!(!app.fit_toolbar(&mut cx), "the row decided off a layout it does not have");
        }
        assert_eq!(app.toolbar.level(), 0, "and gave nothing up");
        // The half that makes the above worth asserting: a ladder that
        // was left able to price its steps still comes back up.
        settle(&mut cx, &mut app, &pass, &mut list, 700.0);
        assert!(app.toolbar.level() > 0, "the narrow row gave nothing up");
        settle(&mut cx, &mut app, &pass, &mut list, WIDE);
        assert_eq!(app.toolbar.level(), 0, "a step priced at nothing is a step kept for good");
        assert_eq!(shown(&cx, &app), expected(0));
    }

    /// A row that moved has to ask for the draw that proves it, and the
    /// ask has to be one that survives where it is made. The fit runs
    /// inside the draw event, and in there every redraw is refused but
    /// the whole-window one -- so the redraws `set_text` and
    /// `set_visible` ask for on their own account go nowhere, and a row
    /// that leant on them gave something up, never laid itself out
    /// again, and sat there with the old widths on screen and a ladder
    /// that thought it had moved. And the other half: a row that has
    /// settled asks for no draw at all, or the asking is the jitter.
    #[test]
    fn a_level_that_moved_asks_for_the_one_redraw_a_draw_event_allows() {
        let (mut cx, mut app, pass, mut list) = fixture();
        settle(&mut cx, &mut app, &pass, &mut list, WIDE);
        // Narrow enough that the first step is owed. The draw clears
        // what is pending, so what is pending after the fit is the
        // fit's own asking and nothing else.
        let mut moves = 0;
        for _ in 0..16 {
            draw(&mut cx, &app, &pass, &mut list, 700.0);
            cx.new_draw_event = DrawEvent::default();
            if !app.fit_toolbar(&mut cx) {
                break;
            }
            moves += 1;
            assert!(
                cx.new_draw_event.redraw_all,
                "the row moved and asked for a redraw the draw event throws away"
            );
        }
        assert!(moves > 0, "nothing moved, so nothing was proved");
        assert!(!cx.new_draw_event.will_redraw(), "a settled row asked to be drawn again");
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

}
