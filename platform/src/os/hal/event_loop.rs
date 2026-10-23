//! The HAL client's event loop: open a session, then per turn pump events, feed them to Cx,
//! turn the platform ops into commands, run the frame beat when there is work, and submit.
//!
//! One crossing each way per turn (HAL.md 2): `hal_pump` brings events, `hal_submit` takes the
//! batch. A backend with a display clock (`info_flags::DISPLAY_CLOCK`: windows on a display)
//! announces a frame once per vsync after a `FrameRequest`, and the beat runs on that
//! announcement: an idle app asks for nothing and wakes for nothing. The headless backend and
//! offscreen windows have no display: the beat runs whenever Cx has work, 60 times a second,
//! or, for a batch renderer (`MAKEPAD_PAINT_INTERVAL`), as fast as the GPU completes frames.

use crate::{cx::{Cx, OsType}, event::hal_intake::hal_window_id, event::Event};
use makepad_hal::{hal_open, hal_pump, hal_submit, info_flags, open_flags, EventStore, OpenReq};
use crate::thread::SignalToUI;
use std::{cell::RefCell, rc::Rc};

/// Time between beats while the app animates or waits for a frame: 60 Hz. The headless backend
/// has no display to pace it; a window system's `FrameReady` takes this place.
const BEAT: std::time::Duration = std::time::Duration::from_nanos(16_666_667);
/// Window frames a batch renderer (`batch_interval`) keeps on the GPU at once.
const BATCH_FRAMES_IN_FLIGHT: usize = 2;

/// The longest a batch renderer's beat waits for the GPU to complete a frame.
const BATCH_STALL: std::time::Duration = BEAT;

/// `MAKEPAD_PAINT_INTERVAL` (seconds): a process whose windows are hidden
/// (`MAKEPAD_HIDE_WINDOWS`) and draw offscreen work only (a batch renderer:
/// Stage's export parts) beats at this interval instead of 60 times a second,
/// as fast as the GPU completes frames ([`BATCH_FRAMES_IN_FLIGHT`] bound the
/// beats ahead of it). Ignored for windows on a display, which pace on it.
fn batch_interval() -> Option<std::time::Duration> {
    static INTERVAL: std::sync::OnceLock<Option<std::time::Duration>> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| {
        std::env::var_os("MAKEPAD_HIDE_WINDOWS")?;
        let value: f64 = std::env::var("MAKEPAD_PAINT_INTERVAL").ok()?.trim().parse().ok()?;
        (value.is_finite() && value > 0.0).then(|| std::time::Duration::from_secs_f64(value.max(0.0002)))
    })
}

/// A frame request the HAL has not answered for this long is made again.
const REREQUEST: std::time::Duration = std::time::Duration::from_millis(500);
/// One budget for all glyph waits and rebuilds of a frame, including newly revealed text.
const GLYPH_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

fn log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_HAL_LOG").is_some())
}

fn app_key() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::env::current_exe().ok().hash(&mut h);
    h.finish() | 1
}

impl Cx {
    /// Attach a native context to an already opened HAL session. The embedder is
    /// its sole event consumer and retains responsibility for the session's lifetime.
    /// Call before pumping Startup; window creation and drawing follow the normal path.
    pub fn hal_attach_session(&mut self, info: makepad_hal::HalInfo) {
        assert!(self.hal.info.is_none(), "Cx already attached to a HAL session");
        assert_ne!(info.backend, makepad_hal::backend_id::NONE);
        self.hal.externally_driven = true;
        self.hal_init_session(info);
    }

    fn hal_init_session(&mut self, info: makepad_hal::HalInfo) {
        #[cfg(target_os = "linux")]
        if info.backend == makepad_hal::backend_id::VULKAN { crate::screen_capture::set_surfaces_supported(); }
        #[cfg(target_os = "linux")]
        { self.os_type = if info.flags & info_flags::DIRECT_DISPLAY != 0 { crate::cx::OsType::LinuxDirect } else { crate::cx::OsType::LinuxWindow(crate::cx::LinuxWindowParams {
            custom_window_chrome: info.flags & info_flags::CLIENT_CHROME != 0,
        }) }; }
        #[cfg(target_os = "linux")]
        crate::screen::set_hal_direct_screens((info.flags & info_flags::DIRECT_DISPLAY != 0).then(Vec::new));
        let supports = |format| info.formats & (1u64 << format) != 0;
        self.gpu_info.float_color_targets = supports(makepad_hal::format::RGBA32F);
        self.gpu_info.float16_blend_targets = supports(makepad_hal::format::RGBA16F);
        self.gpu_info.texture_bc7 = supports(makepad_hal::format::BC7);
        self.gpu_info.texture_astc4x4 = supports(makepad_hal::format::ASTC4X4);
        crate::gpu_info::set_max_gpu_buffer_bytes(info.max_buffer);
        crate::gpu_info::set_gpu_working_set_bytes(info.working_set);
        if info.working_set != 0 { self.gpu_admission.set_device_bytes(info.working_set); }
        self.hal.info = Some(info);
        // `MAKEPAD_PICK_VARIANTS=1`: every shader also compiles its pick variant from the start
        // (backend coverage of the pick emitters without an editor driving picks)
        if std::env::var_os("MAKEPAD_PICK_VARIANTS").is_some() {
            self.enable_pick_variants();
        }
        self.hal.frames.open(info.session, info);
        self.hal.events = Some(EventStore::new(info.session, 512, 1 << 20));
        super::register_wake_session(info.session);
    }

    /// One externally scheduled turn. Hidden contexts still consume events and
    /// service actions, but leave their draw tree pending. No session is closed here.
    pub fn hal_session_turn(&mut self, timeout_ns: u64, present: bool) -> bool {
        assert!(self.hal.externally_driven);
        if !self.hal_pump_events(timeout_ns) { return false; }
        self.hal_service_signals();
        self.call_event_handler(&Event::Signal);
        if !self.hal_handle_platform_ops() { self.hal_submit_batch(); return false; }
        if present && !self.hal_beat() { self.hal_submit_batch(); return false; }
        let alive = self.hal_handle_platform_ops();
        self.hal_run_tasks();
        self.hal_submit_batch();
        alive
    }

    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().hal_run();
        // A hosted app's domain is torn down when its threads end: the Cx, which owns the senders
        // its workers (the font rasterizer's) wait on, must go. `self_ref` is a cycle that
        // keeps it alive otherwise. The process's own app exits with the process instead.
        if crate::restart::hosted() {
            cx.borrow_mut().self_ref = None;
        }
    }

    /// Runs one turn's work so that a panic inside it is contained, as the deleted macOS backend
    /// contained one per event: the panic hook has reported it, then `Cx` is put back in order
    /// (the draw state, the script VM check, the exploded view that is the usual suspect) and a
    /// full redraw starts from a clean slate. `or` is what the turn reports after a panic.
    pub(super) fn hal_guarded(&mut self, or: bool, f: impl FnOnce(&mut Cx) -> bool) -> bool {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(alive) => alive,
            Err(_) => {
                // an intake that unwound took the event store with it
                if self.hal.events.is_none() {
                    if let Some(info) = self.hal.info {
                        self.hal.events = Some(EventStore::new(info.session, 512, self.hal.event_arena_min.max(1 << 20)));
                    }
                }
                self.recover_after_caught_panic();
                self.sploded_recover_after_panic();
                crate::error!("hal: a panic was contained; the app goes on with a full redraw");
                or
            }
        }
    }

    /// Runs the futures spawned through `cx.spawner()` until they wait on something (the old
    /// macOS backend did so after every event). A panicking task is contained and reported.
    pub(super) fn hal_run_tasks(&mut self) {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let Some(executor) = self.executor.take() else { return };
        let stalled = catch_unwind(AssertUnwindSafe(|| executor.run_until_stalled()));
        self.executor = Some(executor);
        if stalled.is_err() {
            crate::error!("hal: a spawned task panicked; it is dropped");
        }
    }

    /// The paint clock stands still when nothing needs a frame.
    pub(super) fn hal_has_work(&self) -> bool {
        if self.hosted_frames_held() {
            return false;
        }
        let next_frames_due = !self.new_next_frames.is_empty() && !crate::remote::app_clock::enabled();
        self.any_passes_dirty()
            || self.need_redrawing()
            || next_frames_due
            || self.demo_time_repaint
            || !self.screenshot_requests.is_empty()
            || !self.draw_shaders.compile_set.is_empty()
            || crate::remote::needs_ticks()
            || self.hal_software_video_active()
            || !self.pick.requests.is_empty()
    }

    fn hal_run(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        fn termination_requested() -> bool {
            crate::os::termination_signal::seen()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        fn termination_requested() -> bool {
            false
        }
        // before anything reads a relative path or loads a resource
        let dev_launch = dev_launch_begin();
        #[cfg(any(target_os = "ios", target_os = "tvos"))]
        install_panic_log();
        self.os_type = host_os_type();
        let hosted = crate::app_main::should_run_stdin_loop_from_env();
        // a desktop keyboard is physical (hosted children hear it from their host at startup)
        #[cfg(any(target_os = "macos", target_os = "windows", all(target_os = "linux", not(target_os = "android"))))]
        if !hosted {
            self.set_physical_keyboard_state(true);
        }
        if hosted {
            if !Self::has_studio_web_socket() {
                crate::error!("--stdin-loop mode requires a studio websocket");
                return;
            }
            self.in_makepad_studio = true;
            self.hal.hosted = Some(Default::default());
            #[cfg(target_os = "linux")]
            if let Err(error) = self.hal_hosted_start_transport() {
                crate::error!("hal hosted transport: {error}");
                return;
            }
            self.studio_backlog = Some(Default::default());
            self.stdin_install_wake();
        }
        let dpi_hint = std::env::var("MAKEPAD_HAL_DPI").ok().and_then(|s| s.parse::<f32>().ok()).filter(|d| *d > 0.0).unwrap_or(1.0);
        // `MAKEPAD_HAL_BACKEND=headless|offscreen|metal|vulkan`: Metal is the macOS default;
        // Linux defaults to desktop Vulkan; hidden/offscreen requests do not start an OS shell.
        let mut windows = 0;
        #[cfg(target_os = "linux")]
        if hosted { windows |= open_flags::OFFSCREEN_WINDOWS; }
        // a script (or the remote bridge) reads windows back: they keep their newest frame
        if std::env::var_os("MAKEPAD_HAL_SCRIPT").is_some() || crate::remote::requested() {
            windows |= open_flags::RETAIN_WINDOWS;
        }
        if crate::remote::requested() {
            windows |= open_flags::NO_FOCUS;
        }
        if crate::launched_hidden_until_first_present() {
            windows |= open_flags::HIDDEN_UNTIL_PRESENT;
        }
        if crate::app_main::is_window_manager() {
            windows |= open_flags::WINDOW_MANAGER;
        }
        let info = hal_open(&OpenReq::from_env(app_key(), windows, dpi_hint));
        if info.session == 0 {
            crate::error!("hal: the HAL refused to open a session");
            self.hal.hosted.take();
            return;
        }
        self.hal_init_session(info);
        // controllers connect and report values from here on (they ring the pump, no polling)
        #[cfg(any(target_vendor = "apple", target_os = "windows"))]
        self.hal.frames.w.game_out(0, makepad_hal::media::game_op::ENABLE, 0, 0, 0, 0, &[]);
        let display_clock = info.flags & info_flags::DISPLAY_CLOCK != 0;
        // a batch renderer beats as the GPU completes frames, not 60 times a second
        let batch = batch_interval().filter(|_| !display_clock && !hosted);
        let period = batch.unwrap_or(BEAT);
        // the submission serial after each of the last beats: a beat waits for the GPU to
        // have completed the oldest of them
        let mut in_flight: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
        // when the last beat ran: a frame whose completion never comes holds the beat back
        // for at most BATCH_STALL (it then paces as an unbatched one would)
        let mut last_beat = std::time::Instant::now();

        let mut next_beat = std::time::Instant::now();
        // a frame asked for and not yet announced: when, so a lost request is asked again
        let mut requested: Option<std::time::Instant> = None;
        let mut host_poll_due = hosted;
        'turns: loop {
            // A signal raised with no wake to carry it (before the session opened: its wake
            // reached no pump) is serviced now; `set_*_signal` wakes only on the flag's rising
            // edge, so left pending it silenced every later wake (a hosted child then slept
            // through its host's messages: the cold `makepad run` Browser showed no page).
            let signal_pending = SignalToUI::any_pending();
            // Terminated while hosted: the host's Ticks are not what the shutdown waits for
            // (the host may be gone, or hung), so the child runs its own timers until it quits.
            let quitting = hosted && termination_requested();
            let timeout = if host_poll_due || signal_pending {
                0
            } else if quitting {
                self.hal.hosted.as_ref().and_then(|hosted| hosted.timers.next_due_in()).map_or(u64::MAX, |secs| (secs * 1e9) as u64)
            } else if hosted || self.hal.device_lost {
                // Host input, GPU completion and worker signals all wake the HAL pump.
                // The host schedules logical timers, so an idle child needs no polling.
                // A lost GPU device draws nothing: the pump sleeps until the HAL's DeviceReset
                // (or input, a signal) wakes it.
                u64::MAX
            } else if display_clock || !self.hal_has_work() {
                // idle: sleep until an event, a timer the app set or a wake; a frame asked for
                // and not announced is asked again when its wait runs out
                match requested.filter(|_| display_clock && self.hal_has_work()) {
                    Some(at) => (at + REREQUEST).saturating_duration_since(std::time::Instant::now()).as_nanos() as u64,
                    None => u64::MAX,
                }
            } else if batch.is_some() && in_flight.len() >= BATCH_FRAMES_IN_FLIGHT && self.frame_completed_serial() < in_flight[0] {
                // the GPU is behind: its FrameDone wakes the pump
                BATCH_STALL.saturating_sub(last_beat.elapsed()).as_nanos() as u64
            } else {
                next_beat.saturating_duration_since(std::time::Instant::now()).as_nanos() as u64
            };
            if !self.hal_guarded(true, |cx| cx.hal_pump_events(timeout)) {
                crate::log!("hal: the last window closed");
                break 'turns;
            }

            // what other threads asked for: signals, actions, the control channel
            if std::mem::take(&mut self.hal.wake) || signal_pending {
                self.hal_service_signals();
            }
            if !self.hal_handle_platform_ops() {
                crate::log!("hal: the app quit");
                self.call_event_handler(&Event::Shutdown);
                self.hal_submit_batch();
                break 'turns;
            }
            if hosted {
                // Startup and its CreateWindow ops precede any host input. A consumed batch
                // gets another nonblocking pump, including wakes consumed by the glyph wait.
                let Some(received) = self.hal_hosted_poll() else { break 'turns };
                host_poll_due = received;
                if quitting {
                    // Nothing is drawn or announced to the host: timers and signals run.
                    self.hosted_hidden = true;
                    if !self.hal_guarded(true, |cx| cx.hal_beat()) {
                        self.call_event_handler(&Event::Shutdown);
                        self.hal_submit_batch();
                        break 'turns;
                    }
                }
            }
            let announced = std::mem::take(&mut self.hal.beat);
            let gpu_ready = batch.is_none() || in_flight.len() < BATCH_FRAMES_IN_FLIGHT || self.frame_completed_serial() >= in_flight[0]
                || last_beat.elapsed() >= BATCH_STALL;
            let due = std::time::Instant::now() >= next_beat && gpu_ready;
            if !hosted && !self.hal.device_lost && (announced || (!display_clock && due && self.hal_has_work())) {
                requested = None;
                if log_enabled() && announced {
                    eprintln!("[hal-client] session={} FrameReady starts draw beat", info.session);
                }
                if !self.hal_guarded(true, |cx| cx.hal_beat()) {
                    self.call_event_handler(&Event::Shutdown);
                    self.hal_submit_batch();
                    break 'turns;
                }
                // a fixed cadence: the next beat is one period after this one was DUE, not after
                // its work ended (that made the period 16.7 ms + the beat's CPU time: ~48 fps
                // offscreen); a beat more than a period late (idle, a stall) starts a new cadence
                let now = std::time::Instant::now();
                next_beat = if next_beat + period > now { next_beat + period } else { now + period };
                last_beat = now;
                if batch.is_some() {
                    while in_flight.len() >= BATCH_FRAMES_IN_FLIGHT {
                        in_flight.pop_front();
                    }
                    in_flight.push_back(self.frame_submission_serial());
                }
            }
            if !self.hal_handle_platform_ops() {
                self.call_event_handler(&Event::Shutdown);
                self.hal_submit_batch();
                break 'turns;
            }
            // work for the next vsync: ask each window for a frame
            if hosted {
                self.hal_hosted_announce_demand();
            } else if display_clock && !self.hal.device_lost && self.hal_has_work() && requested.is_none_or(|t| t.elapsed() > REREQUEST) {
                let ids: Vec<_> = self.windows.id_iter().filter(|w| self.hal.window_ready(*w)).collect();
                for id in ids {
                    self.hal.frames.w.frame_request(hal_window_id(id));
                }
                requested = Some(std::time::Instant::now());
            }
            self.hal_run_tasks();
            self.hal_submit_batch();
        }
        // the session ends: Quit in the last batch makes the HAL free what it owns (textures,
        // buffers, pipelines, windows)
        self.hal.frames.w.quit();
        self.hal_submit_batch();
        super::unregister_wake_session(info.session);
        self.hal.hosted.take();
        if crate::restart::hosted() {
            crate::restart::stop_workers();
        }
        if let Some(dir) = dev_launch {
            // the app closed its own windows rather than being killed: the only outcome the
            // development runner reads as a clean exit
            let _ = std::fs::write(dir.join("status"), "0\n");
        }
    }

    /// The normal session pump, also used while a built draw waits for its glyphs. Input,
    /// media, close and wake events keep their usual order and handling during that wait.
    fn hal_pump_events(&mut self, timeout_ns: u64) -> bool {
        let mut store = self.hal.events.take().expect("event store");
        let mut buf = store.buf();
        if self.hal.event_arena_min > buf.arena_cap as usize {
            store = EventStore::new(buf.session, buf.cap as usize, self.hal.event_arena_min);
            buf = store.buf();
        }
        let n = hal_pump(timeout_ns, &mut buf);
        let intake_start = makepad_hal::profile::on().then(makepad_hal::clock_ns);
        let mut alive = true;
        // Pointer moves are coalesced per batch: a move directly followed by another move of
        // the same window and modifiers is dropped and its delta carried into the next, so a
        // fast pointer costs the app one move per pump, not one per hardware sample.
        let mut events = store.reader(n, buf.arena_len).peekable();
        let mut carry = (0.0, 0.0);
        while let Some((time_ns, mut event)) = events.next() {
            use makepad_hal::HalEvent;
            if let HalEvent::MouseMove { window, dx, dy, mods, .. } = event {
                if let Some((_, HalEvent::MouseMove { window: next_window, mods: next_mods, .. })) = events.peek() {
                    if *next_window == window && *next_mods == mods {
                        carry = (carry.0 + dx, carry.1 + dy);
                        continue;
                    }
                }
                if let HalEvent::MouseMove { dx, dy, .. } = &mut event {
                    *dx += carry.0;
                    *dy += carry.1;
                }
                carry = (0.0, 0.0);
            }
            if log_enabled() {
                use makepad_hal::HalEvent;
                match event {
                    HalEvent::Geom { .. }
                    | HalEvent::SafeArea { .. }
                    | HalEvent::Display { .. }
                    | HalEvent::Lifecycle { .. }
                    | HalEvent::Focus { .. }
                    | HalEvent::ImeAnimate { .. }
                    | HalEvent::ImeResize { .. }
                    | HalEvent::ImeState { .. }
                    | HalEvent::PhysicalKeyboard { .. }
                    | HalEvent::FrameReady { .. }
                    | HalEvent::PipelineReady { .. }
                    | HalEvent::FrameDone { .. } => {
                        eprintln!("[hal-client] session={} received {event:?}", buf.session);
                    }
                    HalEvent::FontOpened { font, status, .. } => {
                        eprintln!("[hal-client] session={} FontOpened font={font} status={status}", buf.session);
                    }
                    HalEvent::GlyphsReady { font, count, .. } => {
                        eprintln!("[hal-client] session={} GlyphsReady font={font} count={count}", buf.session);
                    }
                    _ => {}
                }
            }
            if !self.hal_intake(time_ns, event) {
                alive = false;
                break;
            }
        }
        self.hal.events = Some(store);
        if let Some(start) = intake_start { self.hal.profile_input_ns += makepad_hal::clock_ns() - start; }
        alive
    }

    /// One batch to the HAL, then the memory its slices pointed at is ours again.
    pub(super) fn hal_submit_batch(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.hal.direct_display.flush(&mut self.hal.frames.w);
            if self.hal.direct_display.window.is_some() { crate::linux_input::flush(&mut self.hal.frames.w); }
        }
        if self.hal.frames.w.is_empty() {
            return;
        }
        let has_frames = self.hal.frames.w.buf().frames.len != 0;
        let epoch = self.gpu_admission.submitted(has_frames);
        self.hal.frames.w.gpu_budget(epoch);
        let buf = self.hal.frames.w.buf();
        if log_enabled() && !self.hal.frames.window_frames.is_empty() {
            eprintln!("[hal-client] session={} submitting window FramePackets targets={:?}", buf.session, self.hal.frames.window_frames);
        }
        if !self.hal.frames.window_frames.is_empty() { makepad_hal::profile::submitted(); }
        hal_submit(&buf);
        self.hal.glyph_wait.presented.extend(self.hal.frames.window_frames.drain(..));
        self.hal.frames.submitted();
        // the HAL no longer calls the audio entries this batch replaced
        self.hal.retired_ctx.clear();
        if let Some(hosted) = self.hal.hosted.as_mut() {
            hosted.batch_submitted();
        }
    }

    /// The frame beat (what macOS ran on its paint clock): services, next-frame callbacks, the
    /// draw event, shader compiles, then the passes into commands.
    pub(super) fn hal_beat(&mut self) -> bool {
        self.hal.beat_phases = [[0; 2]; 6];
        self.hal.beat_phases[0][0] = makepad_hal::clock_ns();
        frame_log::beat();
        crate::frame_sources::beat();
        if makepad_hal::profile::on() {
            use makepad_hal::profile::{record, Metric};
            let now = makepad_hal::clock_ns();
            let last = std::mem::replace(&mut self.hal.profile_beat, now);
            if last != 0 { record(Metric::Beat, now - last); }
            record(Metric::CpuInput, std::mem::take(&mut self.hal.profile_input_ns));
            // what this beat had to do (a beat with nothing to draw names its reason)
            let count = makepad_hal::profile::count;
            if self.any_passes_dirty() { count("beat: dirty pass"); }
            if self.need_redrawing() { count("beat: redraw"); }
            if !self.new_next_frames.is_empty() { count("beat: next frame"); }
            if self.demo_time_repaint { count("beat: demo repaint"); }
            if !self.screenshot_requests.is_empty() { count("beat: screenshot"); }
            if !self.draw_shaders.compile_set.is_empty() { count("beat: shaders"); }
            if crate::remote::needs_ticks() { count("beat: remote ticks"); }
            if self.hal_software_video_active() { count("beat: video"); }
            if !self.pick.requests.is_empty() { count("beat: pick"); }
        }
        self.hal_service_signals();
        if let Some(hosted) = self.hal.hosted.as_mut() {
            let timers = hosted.timers.get_dispatch();
            for event in timers {
                self.handle_script_timer(&event);
                self.call_event_handler(&Event::Timer(event));
            }
        }
        if !self.hosted_frames_held() {
            self.hal_poll_software_videos();
        }
        self.run_live_edit_if_needed("hal");
        if !self.hosted_frames_held() {
            if !self.hal_draw_frame() {
                return false;
            }
        }
        // Garbage collection also runs on hidden Ticks that serviced signals and timers.
        let gc_start = self.hal.hosted.is_some().then(|| self.hal_now());
        let mut gc_heap_live = None;
        self.with_vm(|vm| {
            if vm.heap().needs_gc() {
                vm.gc();
                if gc_start.is_some() { gc_heap_live = Some(vm.heap().gc_live_len() as u64); }
            }
        });
        if let (Some(start), Some(heap_live)) = (gc_start, gc_heap_live) {
            Self::send_studio_message(crate::studio::AppToStudio::GCSample(
                crate::studio::GCSample { start, end: self.hal_now(), heap_live },
            ));
        }
        true
    }

    /// Visible frame work: hidden hosted children keep callbacks and dirty passes pending.
    fn hal_draw_frame(&mut self) -> bool {
        // the GPU device is lost: nothing is drawn until the HAL's DeviceReset (dirty passes
        // stay dirty and paint then)
        if self.hal.device_lost {
            return true;
        }
        // A hosted app whose main window has no free shared image runs no frame beat at all:
        // like a native swapchain acquire, it waits (next-frame callbacks and dirty passes stay
        // pending, the Tick's demand asks the host again). `fb_pass` already holds the passes
        // of such a window; this also skips the app's NextFrame and draw walk, which a game ran
        // on every 240 Hz host Tick for frames nobody could see.
        if self.hal.hosted.as_ref().is_some_and(|h| h.first.is_some_and(|w| h.window_blocked(w))) {
            makepad_hal::profile::count("beat: hosted images busy");
            return true;
        }
        let time_now = self.hal_now();
        let draw_start = makepad_hal::profile::on().then(makepad_hal::clock_ns);
        let next_start = makepad_hal::clock_ns();
        self.hal.beat_phases[0][1] = next_start;
        if !self.new_next_frames.is_empty() {
            self.call_next_frame_event(time_now);
            self.hal.beat_phases[1] = [next_start, makepad_hal::clock_ns()];
        }
        let walk_start = makepad_hal::clock_ns();
        let mut deadline = None;
        loop {
            // A control-channel visibility message may arrive during the glyph wait.
            if self.hosted_frames_held() {
                return true;
            }
            self.hal.glyph_wait.pending.clear();
            self.hal.glyph_wait.drawn_blank = 0;
            if self.need_redrawing() {
                // (a frame drawn again below still paints what an earlier drawing of it recorded
                // and this one did not: `call_draw_event` keeps it)
                self.call_draw_event(time_now);
            }
            if !self.draw_shaders.compile_set.is_empty() {
                let mut fb = std::mem::take(&mut self.hal.frames);
                self.fb_compile_shaders(&mut fb);
                self.hal.frames = fb;
            }
            // Embedders must remain responsive while resources arrive. Keep the last
            // complete frame until every glyph and pipeline of this draw is ready;
            // submit just requests/resources, then rebuild on a later turn. In
            // particular, never display a partial first frame or a timeout fallback.
            let first_frame = self.windows.id_iter().any(|id| self.hal.window_ready(id)
                && !self.hal.glyph_wait.presented.contains(&hal_window_id(id)));
            if (self.hal.externally_driven || first_frame)
                && (!self.hal.glyph_wait.pending.is_empty() || self.hal.frames.pipelines_pending() != 0) {
                if log_enabled() && first_frame {
                    eprintln!("[hal-client] first window FramePacket deferred: pending_glyphs={} pending_pipelines={}; waiting for resources before submission",
                        self.hal.glyph_wait.pending.len(), self.hal.frames.pipelines_pending());
                }
                self.frame_drawn_again();
                self.redraw_all();
                return true;
            }
            if self.hal.glyph_wait.pending.is_empty() {
                break;
            }
            let end = *deadline.get_or_insert_with(|| std::time::Instant::now() + GLYPH_WAIT);
            if std::time::Instant::now() >= end {
                if !std::mem::replace(&mut self.hal.glyph_wait.timed_out, true) {
                    crate::warning!("hal: presenting after 250 ms with {} glyph requests still pending", self.hal.glyph_wait.pending.len());
                }
                break;
            }
            // No FramePacket exists yet: submitting here sends font requests and resources,
            // never the incomplete text. The next draw consumes the font events we pump.
            while !self.hal.glyph_wait.pending.is_empty() {
                self.hal_submit_batch();
                let remaining = end.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                if !self.hal_pump_events(remaining.as_nanos() as u64) {
                    return false;
                }
                if std::mem::take(&mut self.hal.wake) {
                    self.hal_service_signals();
                }
                if !self.hal_handle_platform_ops() {
                    return false;
                }
            }
            // Rebuild even on timeout, so every glyph that arrived within the budget is used.
            self.frame_drawn_again();
            self.redraw_all();
        }
        self.hal.beat_phases[2] = [walk_start, makepad_hal::clock_ns()];
        let build_start = draw_start.map(|start| {
            let now = makepad_hal::clock_ns();
            makepad_hal::profile::record(makepad_hal::profile::Metric::CpuDraw, now - start);
            now
        });
        let mut fb = std::mem::take(&mut self.hal.frames);
        let mut passes = Vec::new();
        self.compute_pass_repaint_order(&mut passes);
        if !passes.is_empty() {
            self.repaint_id += 1;
            fb.stats.repaints += 1;
            if self.hal.glyph_wait.drawn_blank > 0 {
                self.hal.glyph_wait.blank_frames += 1;
            }
        }
        let input_ns = std::mem::take(&mut self.hal.input_ns);
        // Also drain freed pool entries on a tick that produces no passes.
        let pass_time = self.pass_time(time_now);
        self.fb_build(&mut fb, &passes, pass_time, input_ns);
        self.fb_picks(&mut fb);
        self.hal.frames = fb;
        self.release_painted_recordings();
        if let Some(start) = build_start {
            makepad_hal::profile::since(makepad_hal::profile::Metric::CpuBuild, start);
            if passes.is_empty() { makepad_hal::profile::count("beat: no pass painted"); }
        }
        true
    }

    pub(crate) fn handle_networking_events(&mut self) {
        self.dispatch_network_runtime_events();
    }
}

/// The OS this client runs on, as widgets read it (`Cx::os_type`): the caption bar, window buttons
/// and keyboard conventions follow it.
fn host_os_type() -> OsType {
    #[cfg(target_os = "macos")]
    return OsType::Macos;
    #[cfg(target_os = "windows")]
    return OsType::Windows;
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    return OsType::Ios(ios_params());
    #[cfg(not(any(target_vendor = "apple", target_os = "windows")))]
    return OsType::Unknown;
}

/// The app's data directory (Application Support), the device model and the system version.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
fn ios_params() -> crate::cx::IosParams {
    use makepad_apple_sys::*;
    /// NSApplicationSupportDirectory, NSUserDomainMask
    const APPLICATION_SUPPORT: u64 = 14;
    const USER_DOMAIN: u64 = 1;
    let text = |s: ObjcId| if s == nil { String::new() } else { nsstring_to_string(s) };
    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let manager: ObjcId = msg_send![class!(NSFileManager), defaultManager];
        let urls: ObjcId = msg_send![manager, URLsForDirectory: APPLICATION_SUPPORT inDomains: USER_DOMAIN];
        let url: ObjcId = if urls == nil { nil } else { msg_send![urls, firstObject] };
        let data_path = if url == nil { String::new() } else { text(msg_send![url, path]) };
        let device: ObjcId = msg_send![class!(UIDevice), currentDevice];
        let params = crate::cx::IosParams { data_path, device_model: text(msg_send![device, model]), system_version: text(msg_send![device, systemVersion]) };
        let () = msg_send![pool, release];
        params
    }
}

/// The default panic hook writes to stderr, which goes nowhere on an iOS device: panics also go
/// through `error!` (NSLog) while the panicking frame is still on the stack.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
fn install_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "non-string panic payload".to_string());
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "<unknown>".to_string());
        let thread = std::thread::current();
        crate::error!("panic: thread={} location={location} payload={payload}\n{}", thread.name().unwrap_or("<unnamed>"), std::backtrace::Backtrace::force_capture());
        previous(info);
    }));
}

/// `MAKEPAD_DEV_LAUNCH_DIR`: a development runner launched this app; it moves into
/// `MAKEPAD_DEV_WORKING_DIR` and reports its pid there (the runner waits on that pid).
fn dev_launch_begin() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("MAKEPAD_DEV_LAUNCH_DIR")?);
    let report = || -> std::io::Result<()> {
        let working_dir = std::env::var_os("MAKEPAD_DEV_WORKING_DIR").ok_or_else(|| {
            std::io::Error::other("the development runner supplied no working directory")
        })?;
        std::env::set_current_dir(working_dir)?;
        std::fs::write(dir.join("pid"), format!("{}\n", std::process::id()))
    };
    if let Err(error) = report() {
        // the runner is waiting on that pid: fail here rather than leave it watching a
        // process it cannot see
        eprintln!("makepad: could not complete the development launch: {error}");
        let _ = std::fs::write(dir.join("status"), "1\n");
        std::process::exit(1);
    }
    Some(dir)
}

/// `MAKEPAD_FRAME_LOG=1`: the beat gaps over 24 ms, one line each (frame
/// pacing measurements; costs one clock read per beat when off).
mod frame_log {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static ON: OnceLock<bool> = OnceLock::new();
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    pub fn beat() {
        if !*ON.get_or_init(|| std::env::var_os("MAKEPAD_FRAME_LOG").is_some()) { return; }
        let now = Instant::now();
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = *last {
            let gap = now - prev;
            if gap > Duration::from_millis(24) { eprintln!("[frames] beat gap {:.1} ms", gap.as_secs_f64() * 1e3); }
        }
        *last = Some(now);
    }
}
