//! A window app that panics on cue, for the run's paused view over the app
//! (tools/run/fixture/park_view.sh): `PARK_VIEW=main` indexes past a Vec's end on the UI thread
//! 0.6 s after startup, `PARK_VIEW=thread` does it on a thread named `park-worker`; anything else
//! runs quietly. Under `makepad run` the panic parks and the run's view covers the window.
pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 400)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        align: Center
                        Label{
                            text: "Park view fixture"
                            draw_text.text_style.font_size: 24
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    timer: Timer,
}

/// The panic: an index past the end (its message and location are what the view shows).
#[inline(never)]
fn slot_of(items: &[u32], at: usize) -> u32 {
    items[at]
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.timer = cx.start_timeout(0.6);
        }
        if self.timer.is_event(event).is_some() {
            let items: Vec<u32> = std::hint::black_box(vec![1, 2, 3]);
            match std::env::var("PARK_VIEW").as_deref() {
                Ok("main") => log!("slot {}", slot_of(&items, 7)),
                Ok("thread") => {
                    let _ = std::thread::Builder::new()
                        .name("park-worker".into())
                        .spawn(move || slot_of(&items, 9));
                }
                _ => {}
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
