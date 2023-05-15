use {
    std::{
        collections::{VecDeque},
        time::Instant,
        mem,
        os::raw::{c_int},
        ptr,
    },
    self::super::{
        libc_sys,
    },
};


#[derive(Clone, Copy)]
pub struct SelectTimer {
    id: u64,
    timeout: f64,
    repeats: bool,
    delta_timeout: f64,
}

pub struct SelectTimers {
    //pub signal_fds: [c_int; 2],
    pub timers: VecDeque<SelectTimer>,
    pub time_start: Instant,
    pub select_time: f64,
}

impl SelectTimers {
    pub fn new() -> Self {
        Self {
            timers: Default::default(),
            time_start: Instant::now(),
            select_time: 0.0
        }
    }
    
    pub fn select(&mut self, fd: c_int) {
        let mut fds = mem::MaybeUninit::uninit();
        unsafe {
            libc_sys::FD_ZERO(fds.as_mut_ptr());
            libc_sys::FD_SET(0, fds.as_mut_ptr());
            libc_sys::FD_SET(fd, fds.as_mut_ptr()); 
        }
        //libc_sys::FD_SET(self.signal_fds[0], fds.as_mut_ptr());
        // If there are any timers, we set the timeout for select to the `delta_timeout`
        // of the first timer that should be fired. Otherwise, we set the timeout to
        // None, so that select will block indefinitely.
        let timeout = if let Some(timer) = self.timers.front() { 
            Some(libc_sys::timeval {
                // `tv_sec` is in seconds, so take the integer part of `delta_timeout`
                tv_sec: timer.delta_timeout.trunc() as libc_sys::time_t,
                // `tv_usec` is in microseconds, so take the fractional part of
                // `delta_timeout` 1000000.0.
                tv_usec: (timer.delta_timeout.fract() * 1000000.0) as libc_sys::time_t,
            })
        }  
        else { 
            None
        };
        let _nfds = unsafe {libc_sys::select(
            fd+1,
            fds.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            if let Some(mut timeout) = timeout {&mut timeout} else {ptr::null_mut()}
        )};  
       // println!("RETURNED!");
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    pub fn update_timers(&mut self, out: &mut Vec<u64>) {
        out.clear();
        let last_select_time = self.select_time;
        self.select_time = self.time_now();
        let mut select_time_used = self.select_time - last_select_time;
        //println!("{}", self.timers.len());
        while let Some(timer) = self.timers.front_mut() {
            // If the amount of time that elapsed is less than `delta_timeout` for the
            // next timer, then no more timers need to be fired.
            //  println!("TIMER COMPARE {} {}", select_time_used, timer.delta_timeout);
            if select_time_used < timer.delta_timeout {
                timer.delta_timeout -= select_time_used;
                break;
            }
            
            let timer = *self.timers.front().unwrap();
            select_time_used -= timer.delta_timeout;
            
            // Stop the timer to remove it from the list.
            self.stop_timer(timer.id);
            // If the timer is repeating, simply start it again.
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
        let index = self.timers.iter().position( | timer | {
            if delta_timeout < timer.delta_timeout {
                return true;
            }
            delta_timeout -= timer.delta_timeout;
            false
        }).unwrap_or(self.timers.len());
        
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
        //println!("STOPPING TIMER {:?}", id);
        
        // Since we are stopping an existing timer, our first step is to find where in the list this
        // timer should be removed.
        let index = if let Some(index) = self.timers.iter().position( | timer | timer.id == id) {
            index
        } else {
            return;
        };
        
        // Remove the timer from the list.
        let delta_timeout = self.timers.remove(index).unwrap().delta_timeout;
        
        // The timer succeeding the removed timer now has a different timer preceding it, so we need
        // to adjust its `delta timeout`.
        if index < self.timers.len() {
            self.timers[index].delta_timeout += delta_timeout;
        }
    }
    
}
