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

    let Shell = View{
        width: Fill
        height: Fill
        flow: Down

        toolbar := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_2

            H4{text: "Widget catalogue"}
            story_search := TextInput{
                width: 240.
                empty_text: "Search stories"
            }
            // The two ways of reading the search text sit next to it:
            // step through the matches, or take everything else away.
            find_prev := ButtonFlat{text: "▲"}
            find_next := ButtonFlat{text: "▼"}
            match_count := Label{text: ""}
            search_filter := Toggle{text: "Filter"}
            new_only := Toggle{text: "New only"}
            new_count := Label{text: ""}
            theme_select := DropDown{
                labels: ["Dark" "Light" "Skeleton"]
                selected_item: 0
            }
            inspect := ButtonFlat{text: "Inspect"}
            reset := ButtonFlat{text: "Reset"}
            Filler{}
            story_title := Label{text: ""}
        }

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
        self.current = Some(story.key.to_string());
        self.ui.story_canvas(cx, ids!(canvas)).open(cx, story.dsl);
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

    fn drain_requests(&mut self, cx: &mut Cx) {
        for request in remote::take_requests() {
            match request {
                remote::Request::Open(key) => self.open_story(cx, &key),
                remote::Request::Theme(index) => theme::select(cx, index),
                remote::Request::Reset => {
                    self.ui.story_canvas(cx, ids!(canvas)).reset(cx);
                    self.ui.controls_panel(cx, ids!(controls)).reset(cx);
                    self.ui.actions_panel(cx, ids!(actions)).clear(cx);
                }
            }
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
            .set_text(cx, &format!("{} new since {}", n, settings::baseline()));
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
        // Before the first draw, which is the one that sets the folders.
        navigator.set_folded(cx, &settings::get(settings::FOLDED).unwrap_or_default());
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
        self.ui
            .drop_down(cx, ids!(theme_select))
            .set_selected_item(cx, theme::choice());
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
        if self.ui.button(cx, ids!(reset)).clicked(actions) {
            self.ui.story_canvas(cx, ids!(canvas)).reset(cx);
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
        if let Some(name) = settings::get(settings::THEME) {
            if let Some(index) = theme::index_of(&name) {
                theme::set_choice(index);
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
        crate::stories::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::LiveEdit = event {
            let choice = theme::choice();
            self.ui.drop_down(cx, ids!(theme_select)).set_selected_item(cx, choice);
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
            self.refresh_new_count(cx);
            // The match count and the hidden-matches line under the
            // tree were rebuilt blank too; without this a filtered tree
            // says nothing about why after a theme switch.
            self.refresh_match_count(cx);
        }
    }
}
