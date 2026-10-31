//! translator — the standalone window.

pub use makepad_widgets;

mod activity;
mod camera;
mod detect;
mod engine;
mod lang;
mod models;
mod ondevice;
mod pair;
mod palette;
mod prompt;
mod qr;
mod reader;
mod store;
mod view;
mod voice;

use view::TranslatorView;
use makepad_widgets::*;

app_main!(App, font_assets: [INTER_FONT_ASSET]);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Translator"
                window.inner_size: vec2(1180, 720)
                pass +: {clear_color: mod.translator.bg}
                body +: {
                    // Content reflows into the space above the on-screen keyboard (the
                    // bottom bar and the panes stay visible), not under it.
                    keyboard_resize: true
                    padding: 0 margin: 0 spacing: 0
                    translator := TranslatorView{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        match ondevice::activate() {
            Ok(home) => log!("translator: on-device models in {}", home.display()),
            Err(why) => log!("translator: {why}"),
        }
        // `--phone` sizes the window like a phone, to see the compact layout.
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--phone") {
            self.ui.window(cx, ids!(main_window)).resize(cx, dvec2(402.0, 780.0));
        }
        if let Some(mut view) = self.ui.widget(cx, ids!(translator)).borrow_mut::<TranslatorView>() {
            view.set_storage(cx.storage("translator"));
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        palette::set_base(vm);
        makepad_widgets::script_mod(vm);
        palette::apply(vm);
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
