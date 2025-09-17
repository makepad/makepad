use std::os::fd::{AsFd, AsRawFd};

use wayland_client::{Connection, EventQueue};

use crate::{cx_native::EventFlow, select_timer::SelectTimers, wayland::{wayland_event::WaylandEvent, wayland_state::WaylandState}, TimerEvent};

pub(crate) struct WaylandApp {
    connection: Connection,
    pub event_queue: EventQueue<WaylandState>,
    pub state: WaylandState,
    pub timers: SelectTimers,
    event_flow: EventFlow,
    event_loop_running: bool,
    event_callback: Option<Box<dyn FnMut(&mut WaylandApp, WaylandEvent)->EventFlow>>
}
impl WaylandApp {
    pub fn new(connection: Connection, event_queue: EventQueue<WaylandState>, state: WaylandState, event_callback: Box<dyn FnMut(&mut WaylandApp, WaylandEvent)->EventFlow>) -> Self {
        Self{
            connection,
            event_queue,
            state: state,
            timers: SelectTimers::new(),
            event_flow: EventFlow::Wait,
            event_loop_running: true,
            event_callback: Some(event_callback),
        }
    }
    pub fn event_loop(&mut self) {
        self.do_callback(WaylandEvent::Paint);
        let mut timer_ids = Vec::new();
        while self.event_loop_running {
            match self.event_flow {
                EventFlow::Exit => {
                    break;
                }
                EventFlow::Wait => {
                    let time = self.time_now();
                    self.timers.update_timers(&mut timer_ids);
                    for timer_id in &timer_ids{
                        self.do_callback(
                            WaylandEvent::Timer(TimerEvent {
                                timer_id:*timer_id,
                                time: Some(time)
                            })
                        );
                    }
                    if let Some(guard) = self.event_queue.prepare_read() {
                        self.timers.select(guard.connection_fd().as_raw_fd());
                    }
                    self.event_flow = EventFlow::Poll;
                }
                EventFlow::Poll => {
                    let time = self.time_now();
                    self.timers.update_timers(&mut timer_ids);
                    for timer_id in &timer_ids{
                        self.do_callback(
                            WaylandEvent::Timer(TimerEvent {
                                timer_id:*timer_id,
                                time: Some(time)
                            })
                        );
                    }
                    self.event_loop_poll();
                }
            }
        }
    }
    fn event_loop_poll(&mut self) {
        self.event_queue.flush().unwrap();
        if let Some(guard) = self.event_queue.prepare_read() {
            if guard.read().is_ok() {
                self.event_queue.dispatch_pending(&mut self.state).unwrap();
            }
        } else {
            self.event_queue.dispatch_pending(&mut self.state).unwrap();
        }

        self.do_callback(WaylandEvent::Paint);
    }
    fn do_callback(&mut self, event: WaylandEvent) {
        if let Some(mut callback) = self.event_callback.take() {
            self.event_flow = callback(self, event);
            if let EventFlow::Exit = self.event_flow {
                self.terminate_event_loop();
            }
            self.event_callback = Some(callback);
        }
    }
    pub fn terminate_event_loop(&mut self) {
        self.event_loop_running = false;
    }

    pub fn start_timer(&mut self, id: u64, timeout: f64, repeats: bool) {
        self.timers.start_timer(id, timeout, repeats);
    }

    pub fn stop_timer(&mut self, id: u64) {
        self.timers.stop_timer(id);
    }
    pub fn time_now(&self) -> f64 {
        self.timers.time_now()
    }
}
