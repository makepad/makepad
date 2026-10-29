#![allow(dead_code, unused_imports)]
use std::{
    option,
    os::fd::{AsFd, AsRawFd},
};

use wayland_client::{Connection, EventQueue};

use crate::{
    cx_native::EventFlow, os::linux::windowing_backend::UI_TICK_INTERVAL,
    wayland::wayland_state::WaylandState, x11::xlib_event::XlibEvent, TimerEvent,
};

pub(crate) struct WaylandApp {
    // Field order is drop order. `state` owns the windows, whose teardown
    // still talks to the compositor: `eglDestroySurface` and
    // `wl_egl_window_destroy` marshal requests on the display. It must go
    // before `connection`, the last handle, whose drop disconnects the
    // display. The other way round every OpenGL exit segfaulted inside
    // NVIDIA's egl-wayland.
    pub state: WaylandState,
    pub event_queue: EventQueue<WaylandState>,
    connection: Connection,
    event_callback: Option<Box<dyn FnMut(&mut WaylandApp, XlibEvent) -> EventFlow>>,
    /// The longest the next select may block, in seconds; `None` waits for an event or a wakeup.
    pub ui_wait_limit: Option<f64>,
}
impl WaylandApp {
    pub fn new(
        connection: Connection,
        event_queue: EventQueue<WaylandState>,
        state: WaylandState,
        event_callback: Box<dyn FnMut(&mut WaylandApp, XlibEvent) -> EventFlow>,
    ) -> Self {
        Self {
            connection,
            event_queue,
            state: state,
            event_callback: Some(event_callback),
            ui_wait_limit: None,
        }
    }
    pub fn event_loop(&mut self) {
        self.do_callback(XlibEvent::Paint);
        let mut timer_ids = Vec::new();
        while self.state.event_loop_running {
            match self.state.event_flow {
                EventFlow::Exit => {
                    break;
                }
                EventFlow::Wait => {
                    let time = self.time_now();
                    self.state.timers.update_timers(&mut timer_ids);
                    for timer_id in &timer_ids {
                        if !self.state.handle_key_repeat_timer(*timer_id) {
                            self.do_callback(XlibEvent::Timer(TimerEvent {
                                timer_id: *timer_id,
                                time: Some(time),
                            }));
                        }
                    }
                    // Send any requests queued during event handling (cursor shapes,
                    // frame callback requests, etc.) before blocking, so the compositor
                    // can respond and wake the select below. A WouldBlock just means the
                    // socket buffer is full; the messages stay queued and the next loop
                    // iteration retries, so only a dead connection is fatal.
                    let mut wait_limit = self.ui_wait_limit;
                    if let Err(err) = self.event_queue.flush() {
                        let transient = matches!(
                            &err,
                            wayland_client::backend::WaylandError::Io(io)
                                if io.kind() == std::io::ErrorKind::WouldBlock
                        );
                        if !transient {
                            crate::warning!("Wayland flush failed: {}", err);
                            self.terminate_event_loop();
                            return;
                        }
                        // Nothing wakes us for that retry, so don't sleep long.
                        wait_limit = Some(wait_limit.map_or(UI_TICK_INTERVAL, |limit| limit.min(UI_TICK_INTERVAL)));
                    }
                    // A timer handler may have asked for a redraw, so paint right away.
                    if timer_ids.is_empty() {
                        if let Some(guard) = self.event_queue.prepare_read() {
                            self.state.timers.select_fds_capped(
                                std::iter::once(guard.connection_fd().as_raw_fd()),
                                wait_limit,
                            );
                        }
                    }
                    self.state.event_flow = EventFlow::Poll;
                }
                EventFlow::Poll => {
                    let time = self.time_now();
                    self.state.timers.update_timers(&mut timer_ids);
                    for timer_id in &timer_ids {
                        if !self.state.handle_key_repeat_timer(*timer_id) {
                            self.do_callback(XlibEvent::Timer(TimerEvent {
                                timer_id: *timer_id,
                                time: Some(time),
                            }));
                        }
                    }
                    self.event_loop_poll();
                }
            }
        }
    }
    fn event_loop_poll(&mut self) {
        // As in the Wait arm: a full socket buffer (WouldBlock) keeps the requests
        // queued for the next flush; only a dead connection ends the loop.
        if let Err(err) = self.event_queue.flush() {
            let transient = matches!(
                &err,
                wayland_client::backend::WaylandError::Io(io)
                    if io.kind() == std::io::ErrorKind::WouldBlock
            );
            if !transient {
                crate::warning!("Wayland flush failed: {}", err);
                self.terminate_event_loop();
                return;
            }
        }
        if let Some(guard) = self.event_queue.prepare_read() {
            if let Err(err) = guard.read() {
                crate::warning!("Wayland read failed: {}", err);
                self.terminate_event_loop();
                return;
            }
            if let Err(err) = self.event_queue.dispatch_pending(&mut self.state) {
                crate::warning!("Wayland dispatch failed: {}", err);
                self.terminate_event_loop();
                return;
            }
        } else if let Err(err) = self.event_queue.dispatch_pending(&mut self.state) {
            crate::warning!("Wayland dispatch failed: {}", err);
            self.terminate_event_loop();
            return;
        }

        // The whole pointer-event batch is drained; dispatch the single latest coalesced motion
        // (one hover hit-test instead of one per queued motion) before painting.
        self.state.flush_pending_motion();

        self.do_callback(XlibEvent::Paint);
    }
    fn do_callback(&mut self, event: XlibEvent) {
        if let Some(mut callback) = self.event_callback.take() {
            self.state.event_flow = callback(self, event);
            if let EventFlow::Exit = self.state.event_flow {
                self.terminate_event_loop();
            }
            self.event_callback = Some(callback);
        }
    }
    pub fn terminate_event_loop(&mut self) {
        self.state.event_loop_running = false;
    }

    pub fn start_timer(&mut self, id: u64, timeout: f64, repeats: bool) {
        self.state.start_timer(id, timeout, repeats);
    }

    pub fn stop_timer(&mut self, id: u64) {
        self.state.stop_timer(id);
    }
    pub fn time_now(&self) -> f64 {
        self.state.time_now()
    }
}
