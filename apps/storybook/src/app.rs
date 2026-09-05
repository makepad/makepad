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
                navigator := StoryNavigator{}
            }
            b: Splitter{
                axis: SplitterAxis.Horizontal
                align: SplitterAlign.FromB(380.)
                a: View{
                    width: Fill
                    height: Fill
                    padding: theme.mspace_2
                    canvas := StoryCanvas{}
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
                    }
                    // Every panel is built up front: the app writes into them
                    // before they are shown, and a page that does not exist
                    // yet swallows the write.
                    panels := PageFlip{
                        width: Fill
                        height: Fill
                        active_page: @docs
                        docs := DocsPanel{}
                        controls := ControlsPanel{}
                        actions := ActionsPanel{}
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

const PANELS: &[LiveId] = &[live_id!(docs), live_id!(controls), live_id!(actions)];

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
        self.ui.story_navigator(cx, ids!(navigator)).select(cx, story.key);
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

    fn refresh_new_count(&self, cx: &mut Cx) {
        let navigator = self.ui.story_navigator(cx, ids!(navigator));
        let n = navigator.new_count();
        self.ui
            .label(cx, ids!(new_count))
            .set_text(cx, &format!("{} new since {}", n, settings::baseline()));
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
        if settings::get(settings::NEW_ONLY).as_deref() == Some("1") {
            navigator.set_new_only(cx, true);
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
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(key) = self.ui.story_navigator(cx, ids!(navigator)).opened(actions) {
            self.open_story(cx, &key);
        }
        if let Some(text) = self.ui.text_input(cx, ids!(story_search)).changed(actions) {
            self.ui.story_navigator(cx, ids!(navigator)).set_filter(cx, &text);
        }
        if self.ui.text_input(cx, ids!(story_search)).returned(actions).is_some() {
            if let Some(story) = self.ui.story_navigator(cx, ids!(navigator)).first_match() {
                self.open_story(cx, story.key);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(new_only)).changed(actions) {
            self.ui.story_navigator(cx, ids!(navigator)).set_new_only(cx, on);
            settings::set(settings::NEW_ONLY, if on { "1" } else { "0" });
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(theme_select)).selected(actions) {
            theme::select(cx, index);
        }
        if let Some(index) = self
            .ui
            .radio_button_set(cx, ids_array!(tab_docs, tab_controls, tab_actions))
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
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
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
        }
    }
}
