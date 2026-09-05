//! The app shell: a toolbar, the navigator, the canvas and the side panels.
use crate::canvas::*;
use crate::makepad_widgets::*;
use crate::navigator::*;
use crate::registry;
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
                align: SplitterAlign.FromB(360.)
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
                    padding: theme.mspace_2
                    side_note := Label{text: "Docs, controls and actions arrive with the next commits."}
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
        self.ui
            .label(cx, ids!(story_title))
            .set_text(cx, &format!("{} / {}", story.component, story.name));
        settings::set(settings::LAST_STORY, story.key);
        log!("storybook: story {}", story.key);
    }

    fn refresh_new_count(&self, cx: &mut Cx) {
        let navigator = self.ui.story_navigator(cx, ids!(navigator));
        let n = navigator.new_count();
        self.ui
            .label(cx, ids!(new_count))
            .set_text(cx, &format!("{} new since {}", n, settings::baseline()));
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
        if self.ui.button(cx, ids!(inspect)).clicked(actions) {
            crate::makepad_widgets::tweaker::set_tweak_on(cx, true);
        }
        if self.ui.button(cx, ids!(reset)).clicked(actions) {
            self.ui.story_canvas(cx, ids!(canvas)).reset(cx);
        }
        if let Some(story) = self.current() {
            if let Some(on_actions) = story.on_actions {
                if let Some(root) = self.ui.story_canvas(cx, ids!(canvas)).shown_root() {
                    on_actions(cx, &root, actions);
                }
            }
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
        crate::stories::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::LiveEdit = event {
            // Everything was rebuilt from its templates; the theme dropdown
            // and the title are plain state the rebuild reset.
            let choice = theme::choice();
            self.ui.drop_down(cx, ids!(theme_select)).set_selected_item(cx, choice);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Event::LiveEdit = event {
            if let Some(story) = self.current() {
                self.ui
                    .label(cx, ids!(story_title))
                    .set_text(cx, &format!("{} / {}", story.component, story.name));
            }
            self.refresh_new_count(cx);
        }
    }
}
