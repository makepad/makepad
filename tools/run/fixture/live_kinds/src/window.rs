//! The partial-landing probe also presents a real app frame, so a run overlay
//! would cover it and `session view` can assert that the frame stays available. It hands its
//! tick over a restart (restart_state): a restarted run prints "out restored <tick>" and goes on
//! from it (restart_exec.py).
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(420, 220)
                body +: {
                    align: Center
                    Label{text: "Live patch probe"}
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
    #[rust]
    tick: u64,
    #[rust]
    state: Option<crate::kinds::State>,
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn restart_state(&mut self, _cx: &mut Cx) -> Option<String> {
        Some(self.tick.to_string())
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &str) {
        self.tick = state.trim().parse().unwrap_or(0);
        println!("out restored {}", self.tick);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::Startup) {
            self.state = Some(crate::kinds::State::new());
            self.timer = cx.start_interval(0.02);
        }
        if self.timer.is_event(event).is_some() {
            if let Some(state) = &mut self.state {
                let line = crate::kinds::report(state);
                println!("out {} main_loop={} inlined={} {} {}", self.tick, 1, crate::kinds::tiny(), line, crate::kinds::report2());
                self.tick += 1;
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
