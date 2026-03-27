use std::{collections::VecDeque, mem, os::raw::c_int, ptr, time::Instant};

use self::super::libc_sys;

#[derive(Clone, Copy)]
pub struct SelectTimer {
    id: u64,
    timeout: f64,
    repeats: bool,
    delta_timeout: f64,
}

pub struct SelectTimers {
    pub timers: VecDeque<SelectTimer>,
    pub time_start: Instant,
    pub select_time: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SelectResult {
    pub main_fd_ready: bool,
    pub wake_fd_ready: bool,
    pub timed_out: bool,
}

impl SelectTimers {
    pub fn new() -> Self {
        Self {
            timers: Default::default(),
            time_start: Instant::now(),
            select_time: 0.0,
        }
    }

    pub fn has_timer(&self, id: u64) -> bool {
        self.timers.iter().any(|timer| timer.id == id)
    }

    pub fn select(&mut self, main_fd: c_int, wake_fd: Option<c_int>) -> SelectResult {
        let mut fds = mem::MaybeUninit::uninit();
        unsafe {
            libc_sys::FD_ZERO(fds.as_mut_ptr());
            libc_sys::FD_SET(main_fd, fds.as_mut_ptr());
            if let Some(wake_fd) = wake_fd {
                libc_sys::FD_SET(wake_fd, fds.as_mut_ptr());
            }
        }
        let mut timeout = self.timers.front().map(|timer| libc_sys::timeval {
            tv_sec: timer.delta_timeout.trunc() as libc_sys::time_t,
            tv_usec: (timer.delta_timeout.fract() * 1000_000.0) as libc_sys::time_t,
        });
        let nfds = wake_fd.map_or(main_fd, |wake_fd| main_fd.max(wake_fd)) + 1;
        let result = unsafe {
            libc_sys::select(
                nfds,
                fds.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                timeout
                    .as_mut()
                    .map(|t| t as *mut _)
                    .unwrap_or(ptr::null_mut()),
            )
        };

        if result <= 0 {
            return SelectResult {
                timed_out: result == 0,
                ..Default::default()
            };
        }

        let fds = unsafe { fds.assume_init() };
        SelectResult {
            main_fd_ready: unsafe { libc_sys::FD_ISSET(main_fd, &fds) },
            wake_fd_ready: wake_fd.is_some_and(|wake_fd| unsafe { libc_sys::FD_ISSET(wake_fd, &fds) }),
            timed_out: false,
        }
    }

    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_secs_f64()
    }

    pub fn update_timers(&mut self, out: &mut Vec<u64>) {
        out.clear();
        let last_select_time = self.select_time;
        self.select_time = self.time_now();
        let mut select_time_used = self.select_time - last_select_time;
        while let Some(timer) = self.timers.front_mut() {
            if select_time_used < timer.delta_timeout {
                timer.delta_timeout -= select_time_used;
                break;
            }

            let timer = *self.timers.front().unwrap();
            select_time_used -= timer.delta_timeout;

            self.remove_timer_at(0, false);
            if timer.repeats {
                self.start_timer(timer.id, timer.timeout, timer.repeats);
            }
            out.push(timer.id);
        }
    }

    pub fn start_timer(&mut self, id: u64, timeout: f64, repeats: bool) {
        //println!("STARTING TIMER {:?} {:?} {:?}", id, timeout, repeats);

        // Timers are stored in an ordered list. Each timer stores the amount of time between
        // when its predecessor in the list should fire and when the timer itself should fire
        // in `delta_timeout`.

        // Since we are starting a new timer, our first step is to find where in the list this
        // new timer should be inserted. `delta_timeout` is initially set to `timeout`. As we move
        // through the list, we subtract the `delta_timeout` of the timers preceding the new timer
        // in the list. Once this subtraction would cause an overflow, we have found the correct
        // position in the list. The timer should fire after the one preceding it in the list, and
        // before the one succeeding it in the list. Moreover `delta_timeout` is now set to the
        // correct value.
        let mut delta_timeout = timeout;
        let index = self
            .timers
            .iter()
            .position(|timer| {
                if delta_timeout < timer.delta_timeout {
                    return true;
                }
                delta_timeout -= timer.delta_timeout;
                false
            })
            .unwrap_or(self.timers.len());

        // Insert the timer in the list.
        //
        // We also store the original `timeout` with each timer. This is necessary if the timer is
        // repeatable and we want to restart it later on.
        self.timers.insert(
            index,
            SelectTimer {
                id,
                timeout,
                repeats,
                delta_timeout,
            },
        );

        // The timer succeeding the newly inserted timer now has a new timer preceding it, so we
        // need to adjust its `delta_timeout`.
        //
        // Note that by construction, `timer.delta_timeout < delta_timeout`. Otherwise, the newly
        // inserted timer would have been inserted *after* the timer succeeding it, not before it.
        if index < self.timers.len() - 1 {
            let timer = &mut self.timers[index + 1];
            // This computation should never underflow (see above)
            timer.delta_timeout -= delta_timeout;
        }
    }

    pub fn stop_timer(&mut self, id: u64) {
        let index = if let Some(index) = self.timers.iter().position(|timer| timer.id == id) {
            index
        } else {
            return;
        };
        self.remove_timer_at(index, true);
    }

    fn remove_timer_at(&mut self, index: usize, transfer_delta_to_successor: bool) {
        let removed = self.timers.remove(index).unwrap();
        if transfer_delta_to_successor {
            if let Some(next_timer) = self.timers.get_mut(index) {
                next_timer.delta_timeout += removed.delta_timeout;
            }
        }
    }
}
