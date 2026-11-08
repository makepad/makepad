//! The partial-landing probe also presents a real app frame, so a run overlay
//! would cover it and `session view` can assert that the frame stays available. It hands its
//! tick over a restart (restart_state): a restarted run prints "out restored <tick>" and goes on
//! from it (restart_in_place.py). On cue it panics, calls an unloaded module, starts a thread that
//! loops in code a layout edit rebuilds, starts GPU work the run's end comes in the middle of, or takes
//! seconds over its first event. Its swatch is what the shader kinds of live_kinds.py edit.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(420, 220)
                body +: {
                    flow: Down
                    align: Center
                    Label{text: "Live patch probe"}
                    // the shader kinds (live_kinds.py): the window's left half is this file's
                    // colour and the tint of swatch.rs, the right half swatch.rs's own type, in
                    // the tree and held outside it (its own colour, then the tint, each)
                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        swatch := View{
                            width: Fill
                            height: Fill
                            show_bg: true
                            draw_bg +: {
                                ..mod.widgets.LiveKindsTint,
                                pixel: fn() {
                                    return mix(vec4(1.0, 0.0, 0.0, 1.0), self.swatch_tint(), step(0.5, self.pos.x))
                                }
                            }
                        }
                        typed := mod.widgets.LiveKindsSwatch{}
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
    #[rust]
    tick: u64,
    #[rust]
    state: Option<crate::kinds::State>,
    #[rust]
    gpu: Option<(ComputeKernel, GpuBuffer)>,
}

/// One invocation's loop: a long one is GPU work a run's end comes in the middle of.
const GPU_KERNEL: &str = "use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{workgroup_size: 64\nwords: shader.buffer_rw(u32)\niters: shader.uniform(u32)\nmain: fn() {\nlet i = global_id().x\nvar h = i\nfor k in 0..self.iters { h = h * 1664525u + k + 1013904223u }\nself.words[i] = h }}";

impl App {
    /// A compute job of `iters` steps per invocation and the read of its first word (restart_in_place.py).
    fn gpu_job(&mut self, cx: &mut Cx, iters: u32) -> Result<(), String> {
        if self.gpu.is_none() {
            let buffer = GpuBuffer::new(cx, 256, GpuBufferFlags::default())?;
            let kernel = cx.with_vm(|vm| {
                vm.capture_errors();
                let value = vm.eval(makepad_script::ScriptMod::new("", "", "restart_gpu", 0, 0, GPU_KERNEL, &[]));
                let errors = vm.take_errors();
                if !errors.is_empty() {
                    return Err(format!("{errors:?}"));
                }
                ComputeKernel::new(vm, value.as_object().ok_or("the kernel is no object")?, "restart_gpu")
            })?;
            self.gpu = Some((kernel, buffer));
        }
        let (kernel, buffer) = self.gpu.as_ref().unwrap();
        let mut params = kernel.params();
        params.set_u32(live_id!(iters), iters)?;
        let mut pass = ComputePass::new("restart gpu", None);
        pass.dispatch(kernel, [1, 1, 1], &params, &[buffer.clone()], &[])?;
        pass.submit(cx);
        // (the word a short job writes for invocation 0)
        let want = (iters <= 1 << 20).then(|| (0..iters).fold(0u32, |h, k| h.wrapping_mul(1664525).wrapping_add(k).wrapping_add(1013904223)));
        let start = std::time::Instant::now();
        cx.read(&[(buffer, 0..4)], move |_cx, result| match result {
            Ok(ranges) => {
                let bytes: &[u8] = &ranges[0];
                let got = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let ok = match want { Some(w) if w == got => "ok", Some(_) => "WRONG", None => "unchecked" };
                println!("gpu: done {iters} {ok} in {} ms", start.elapsed().as_millis());
            }
            Err(e) => println!("gpu: read failed {iters} {e:?}"),
        })
        .map_err(|e| format!("{e:?}"))
    }
}

/// restart_in_place.py's held stage adds a variant: code built before that never runs again, so
/// the patch does not install while a thread runs it.
pub enum Held {
    A,
}

#[inline(never)]
fn held_mode() -> Held {
    Held::A
}

/// A thread that never leaves this loop. A layout edit of `Held` rebuilds it and finds the
/// thread's frame in the loop's first version: the edit offers the restart and the app runs on.
#[inline(never)]
fn hold_loop() {
    println!("held: the thread runs");
    loop {
        match held_mode() {
            Held::A => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        crate::swatch::script_mod(vm);
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
            // (restart_in_place.py: a first event that takes seconds, before the app says it
            // restarts with its state)
            if std::env::var_os("RESTART_SLOW_START_FILE").is_some_and(|f| std::fs::remove_file(f).is_ok()) {
                println!("slow start: the first event waits");
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
        if self.timer.is_event(event).is_some() {
            // (restart_in_place.py: a real panic of the app's own)
            if std::env::var_os("RESTART_PANIC_FILE").is_some_and(|f| std::fs::remove_file(f).is_ok()) {
                panic!("the fixture's panic");
            }
            // (and a stale access: a call into a module gone)
            if std::env::var_os("RESTART_STALE_FILE").is_some_and(|f| std::fs::remove_file(f).is_ok()) {
                let module = std::relax::Module::load_source("stale_probe", "stale_probe.rs", "pub fn f(x: u64) -> u64 { x + 1 }", &[]).unwrap();
                let f: fn(u64) -> u64 = module.get("f").unwrap();
                module.unload();
                println!("out called an unloaded module's code: {}", f(3));
            }
            // (and a thread that loops in code a layout edit rebuilds)
            if std::env::var_os("RESTART_HOLD_FILE").is_some_and(|f| std::fs::remove_file(f).is_ok()) {
                std::thread::spawn(hold_loop);
            }
            // (and GPU work in flight: the file holds the job's steps per invocation)
            let job = std::env::var_os("RESTART_GPU_FILE").and_then(|f| {
                let iters = std::fs::read_to_string(&f).ok()?.trim().parse::<u32>().ok()?;
                std::fs::remove_file(&f).ok().map(|_| iters)
            });
            if let Some(iters) = job {
                match self.gpu_job(cx, iters) {
                    Ok(()) => println!("gpu: in flight {iters}"),
                    Err(e) => println!("gpu: failed {iters} {e}"),
                }
            }
            if let Some(state) = &mut self.state {
                let line = crate::kinds::report(state);
                println!("out {} main_loop={} inlined={} {} {}", self.tick, 1, crate::kinds::tiny(), line, crate::kinds::report2());
                self.tick += 1;
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
