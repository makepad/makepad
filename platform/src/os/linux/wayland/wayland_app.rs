#![allow(dead_code, unused_imports)]
use std::{
    option,
    os::fd::{AsFd, AsRawFd},
};

use wayland_client::{Connection, EventQueue};

use crate::{
    cx_native::EventFlow,
    os::linux::wake_pipe::LinuxWakePipe,
    wayland::wayland_state::WaylandState,
    x11::xlib_event::XlibEvent,
    TimerEvent,
};

pub(crate) struct WaylandApp {
    connection: Connection,
    pub event_queue: EventQueue<WaylandState>,
    pub state: WaylandState,
    wake_pipe: LinuxWakePipe,
    main_fd_ready: bool,
    event_callback: Option<Box<dyn FnMut(&mut WaylandApp, XlibEvent) -> EventFlow>>,
}
impl WaylandApp {
    pub fn new(
        connection: Connection,
        event_queue: EventQueue<WaylandState>,
        state: WaylandState,
        event_callback: Box<dyn FnMut(&mut WaylandApp, XlibEvent) -> EventFlow>,
    ) -> Self {
        let mut wake_pipe = LinuxWakePipe::new().expect("failed to create Linux wake pipe");
        wake_pipe.install_signal_waker();
        Self {
            connection,
            event_queue,
            state: state,
            wake_pipe,
            main_fd_ready: false,
            event_callback: Some(event_callback),
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
                        self.do_callback(XlibEvent::Timer(TimerEvent {
                            timer_id: *timer_id,
                            time: Some(time),
                        }));
                    }
                    let select_result = if let Some(guard) = self.event_queue.prepare_read() {
                        let select_result = self.state.timers.select(
                            guard.connection_fd().as_raw_fd(),
                            Some(self.wake_pipe.read_fd()),
                        );
                        drop(guard);
                        select_result
                    } else {
                        self.state.event_flow = EventFlow::Poll;
                        continue;
                    };
                    if select_result.wake_fd_ready {
                        self.wake_pipe.drain();
                        self.do_callback(XlibEvent::Wake);
                    }
                    if let EventFlow::Exit = self.state.event_flow {
                        continue;
                    }
                    if select_result.main_fd_ready {
                        self.main_fd_ready = true;
                        self.state.event_flow = EventFlow::Poll;
                    } else if select_result.timed_out {
                        self.state.event_flow = EventFlow::Poll;
                    }
                }
                EventFlow::Poll => {
                    let time = self.time_now();
                    self.state.timers.update_timers(&mut timer_ids);
                    for timer_id in &timer_ids {
                        self.do_callback(XlibEvent::Timer(TimerEvent {
                            timer_id: *timer_id,
                            time: Some(time),
                        }));
                    }
                    self.event_loop_poll();
                }
            }
        }
    }
    fn event_loop_poll(&mut self) {
        if let Err(err) = self.event_queue.flush() {
            crate::warning!("Wayland flush failed: {}", err);
            self.terminate_event_loop();
            return;
        }

        if self.main_fd_ready {
            self.main_fd_ready = false;
            if let Some(guard) = self.event_queue.prepare_read() {
                if let Err(err) = guard.read() {
                    crate::warning!("Wayland read failed: {}", err);
                    self.terminate_event_loop();
                    return;
                }
            }
        }

        if let Err(err) = self.event_queue.dispatch_pending(&mut self.state) {
            crate::warning!("Wayland dispatch failed: {}", err);
            self.terminate_event_loop();
            return;
        }

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
        self.wake_pipe.uninstall_signal_waker();
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
