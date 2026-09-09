//! The splash story: a widget whose contents are a script it runs in a VM of
//! its own.
use crate::makepad_widgets::splash::Splash;
use crate::makepad_widgets::*;
use crate::registry::Story;

/// The isolate's whole world. It is a `script_mod!` body as a string, and it
/// is compiled and run by a VM that is not this application's.
const BODY: &str = r#"
width: Fill
height: Fit
flow: Down
spacing: 8
padding: 12
show_bg: true
draw_bg +: {
    color: #2a2a34
}

title := Label {
    text: "This label lives in another VM"
    draw_text.color: #e6e6e6
}

status := Label {
    text: "not clicked"
    draw_text.color: #9aa4b2
}

clicker := Button {
    text: "click me"
    on_click: || {
        ui.status.set_text("clicked, and the host never heard about it")
    }
}
"#;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StorySplashBase = #(StorySplash::register_widget(vm))

    mod.storybook.StorySplash = set_type_default() do mod.storybook.StorySplashBase{
        width: Fill
        height: Fit
    }

    mod.stories.SplashOverview = StoryPage{
        StoryNote{text: "A widget whose contents are not declared here. It holds a script, runs it in a VM of its own, and draws whatever that script builds — so the thing below was written as a string and compiled at runtime."}

        StoryHeading{text: "A script running inside a page"}
        StoryNote{text: "The label and the button below belong to the isolate, not to this page. Press the button: its handler runs in the other VM and changes a label there. Nothing about it reaches the story's own code, which is the point."}
        StoryRow{
            View{
                width: 380. height: Fit
                subject := mod.storybook.StorySplash{}
            }
        }
        StoryRow{
            note := Label{text: "the button above is handled entirely inside the isolate" draw_text +: {color: theme.color_text_meta}}
        }
    }
}

#[derive(Script, Widget)]
pub struct StorySplash {
    #[deref]
    splash: Splash,
}

impl Widget for StorySplash {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.splash.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.splash.handle_event(cx, event, scope);
    }
}

impl ScriptHook for StorySplash {
    /// Handing over the body is what starts the isolate, and it has to happen
    /// somewhere that runs without being prompted. A story's action handler
    /// does not: it is called when there ARE actions, so a page that seeds
    /// itself there seeds nothing until someone touches it.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.splash.set_text(cx, BODY);
        });
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/splash/overview",
    category: "Containers",
    component: "Splash",
    also: &[],
    name: "Overview",
    dsl: "SplashOverview",
    added: "2025-05-06",
    tags: &[],
    doc: "# Splash

A widget whose contents are a script it runs in a VM of its own.

**`set_text` is not a caption.** The string you hand a `Splash` *is* a `script_mod!` body, and giving it one is what compiles and starts the isolate. The label and the button on this page were written as a Rust string constant, not declared in the story's DSL, and they are built by a virtual machine that is not this application's.

That separation is the whole point. The button's `on_click` runs inside the isolate and changes a label inside the isolate; the story around it never sees the press and could not intercept it. A host embeds untrusted or hot-swappable UI this way — a plugin, a downloaded panel, a document that draws itself.

Because the isolate is a guest, the host decides what it may do: `set_allow_net`, `set_sandbox_dir`, `set_storage_quota`, `set_host_caps` and `set_host_prompts` are all on the widget, and `validate_splash_body` will tell you what a body would need before you run it. `call_script_fn` is the way in from the other direction — the host calling into the guest by name.

None of that is set here, which is the default and the least privileged version.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: None,
}];
