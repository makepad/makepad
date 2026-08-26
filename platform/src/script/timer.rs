use crate::script::vm::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct CxScriptTimer {
    pub id: LiveId,
    pub repeat: bool,
    pub timer: Timer,
    pub callback: ScriptFnRef,
}

/// A hook that can claim dispatch of a firing script timer, e.g. to run the callback
/// in the isolate VM that owns it instead of the main app VM. Returns true if it
/// handled the timer; false falls through to the default main-VM dispatch.
pub type CxScriptTimerDispatchHook = fn(cx: &mut Cx, timer: &CxScriptTimer, time: ScriptValue) -> bool;

/// A hook consulted BEFORE a script timer is created, so an embedder that runs
/// untrusted script (see the widgets crate's Splash isolates) can cap how many
/// timers one heap holds and how fast they may tick.
///
/// `heap_key` identifies the script heap asking. Return the interval to use —
/// possibly clamped — or `None` to refuse the timer outright. The default,
/// with no hook registered, is the requested interval unchanged.
pub type CxScriptTimerAdmitHook = fn(cx: &mut Cx, heap_key: usize, requested_s: f64) -> Option<f64>;

/// The matching release hook: called when a timer this embedder admitted is
/// stopped, or fires for the last time, so its slot can be given back.
pub type CxScriptTimerReleaseHook = fn(cx: &mut Cx, heap_key: usize);

#[derive(Clone, Default)]
pub struct CxScriptTimers {
    pub timers: Vec<CxScriptTimer>,
    pub dispatch_hooks: Vec<CxScriptTimerDispatchHook>,
    pub admit_hooks: Vec<CxScriptTimerAdmitHook>,
    pub release_hooks: Vec<CxScriptTimerReleaseHook>,
}

impl Cx {
    /// Registers the admission/release pair that caps script timers per heap.
    pub fn set_script_timer_quota_hooks(
        &mut self,
        admit: CxScriptTimerAdmitHook,
        release: CxScriptTimerReleaseHook,
    ) {
        let timers = &mut self.script_data.timers;
        if !timers.admit_hooks.iter().any(|v| (*v as usize) == (admit as usize)) {
            timers.admit_hooks.push(admit);
        }
        if !timers.release_hooks.iter().any(|v| (*v as usize) == (release as usize)) {
            timers.release_hooks.push(release);
        }
    }

    /// The interval a new script timer for `heap_key` may actually use, after
    /// every registered quota hook has had its say. `None` = refused.
    ///
    /// The clamp is not only a policy matter: several platform backends hand
    /// the interval to `Duration::from_secs_f64`, which PANICS on a negative,
    /// NaN or infinite value, so an unfiltered `start_interval(-1.0)` from
    /// script takes the process down. The floor below is the last line of
    /// defence for embedders with no hook registered at all.
    fn admit_script_timer(&mut self, heap_key: usize, requested_s: f64) -> Option<f64> {
        let hooks = self.script_data.timers.admit_hooks.clone();
        let mut interval = requested_s;
        for hook in hooks {
            interval = hook(self, heap_key, interval)?;
        }
        if !interval.is_finite() || interval < 0.0 {
            return Some(0.0);
        }
        Some(interval)
    }

    fn release_script_timer(&mut self, heap_key: usize) {
        let hooks = self.script_data.timers.release_hooks.clone();
        for hook in hooks {
            hook(self, heap_key);
        }
    }

    pub fn add_script_timer_dispatch_hook(&mut self, hook: CxScriptTimerDispatchHook) {
        if !self
            .script_data
            .timers
            .dispatch_hooks
            .iter()
            .any(|v| (*v as usize) == (hook as usize))
        {
            self.script_data.timers.dispatch_hooks.push(hook);
        }
    }

    pub(crate) fn handle_script_timer(&mut self, event: &TimerEvent) {
        if let Some(i) = self
            .script_data
            .timers
            .timers
            .iter()
            .position(|v| v.timer.is_timer(event).is_some())
        {
            let timer = self.script_data.timers.timers[i].clone();
            if !timer.repeat {
                self.script_data.timers.timers.remove(i);
                // A one-shot that has fired is gone; give its slot back or an
                // app that uses timeouts normally would run itself out of them.
                self.release_script_timer(timer.callback.heap_key());
            }
            let time = if let Some(time) = event.time {
                time.into()
            } else {
                NIL
            };
            // A callback minted in an isolate VM must not run against the main VM's heap;
            // hooks (registered by the widgets crate) route those to their owning VM.
            let hooks = self.script_data.timers.dispatch_hooks.clone();
            for hook in hooks {
                if hook(self, &timer, time) {
                    return;
                }
            }
            self.with_vm_and_async(|vm| {
                vm.call(timer.callback.as_object().into(), &[time]);
            })
        }
    }
}

/// Local-time UTC offset in seconds, applied by `std.local_time`. The platform has no
/// timezone database, so hosts that know the local offset (e.g. via chrono) set it here.
static SCRIPT_LOCAL_UTC_OFFSET_SECS: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(0);

pub fn set_script_local_utc_offset_secs(offset_secs: i64) {
    SCRIPT_LOCAL_UTC_OFFSET_SECS.store(offset_secs, std::sync::atomic::Ordering::Relaxed);
}

pub fn script_local_utc_offset_secs() -> i64 {
    SCRIPT_LOCAL_UTC_OFFSET_SECS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Days-since-epoch to (year, month, day), Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn script_mod(vm: &mut ScriptVm) {
    let std = vm.module(id!(std));

    vm.new_handle_type(id_lut!(timer));

    pub fn next_hash(bytes: &[u8; 8]) -> u64 {
        let mut x: u64 = 0xd6e8_feb8_6659_fd93;
        let mut i = 0;
        while i < 8 {
            x = x.overflowing_add(bytes[i] as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        x
    }

    fn fresh_seed() -> u64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut seed = (nanos >> 64) as u64 ^ (nanos as u64);
        seed ^= std::process::id() as u64;
        if seed == 0 {
            seed = 0x9e37_79b9_7f4a_7c15;
        }
        seed
    }

    fn ensure_seeded(cx: &mut Cx) {
        if cx.script_data.random_seed == 0 {
            cx.script_data.random_seed = fresh_seed();
        }
    }

    vm.add_method(
        std,
        id_lut!(random_seed),
        script_args_def!(),
        |vm, _args| {
            let cx = vm.cx_mut();
            cx.script_data.random_seed = fresh_seed();
            NIL
        },
    );

    vm.add_method(std, id_lut!(random), script_args_def!(), |vm, _args| {
        let cx = vm.cx_mut();
        ensure_seeded(cx);
        let seed = cx.script_data.random_seed;
        let seed = next_hash(&seed.to_ne_bytes());
        cx.script_data.random_seed = seed;
        ((seed as f64) / u64::MAX as f64).into()
    });

    vm.add_method(std, id_lut!(random_u32), script_args_def!(), |vm, _args| {
        let cx = vm.cx_mut();
        ensure_seeded(cx);
        let seed = cx.script_data.random_seed;
        let seed = next_hash(&seed.to_ne_bytes());
        cx.script_data.random_seed = seed;
        (seed as u32 as f64).into()
    });

    vm.add_method(std, id_lut!(time_now), script_args_def!(), |_vm, _args| {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        secs.into()
    });

    // Returns {year month day hour minute second weekday} in local time (weekday: 0 = Sunday).
    // Takes an optional epoch-seconds arg, defaulting to now.
    vm.add_method(
        std,
        id_lut!(local_time),
        script_args_def!(epoch = NIL),
        |vm, args| {
            let epoch = script_value!(vm, args.epoch);
            let epoch_secs = epoch.as_number().unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0)
            });
            // Clamp the epoch to a sane range so an absurd script-supplied value
            // can't overflow the offset addition or the civil-date math.
            let epoch_i = (epoch_secs.clamp(-8.0e15, 8.0e15)) as i64;
            let local_secs = epoch_i.saturating_add(script_local_utc_offset_secs());
            let days = local_secs.div_euclid(86400);
            let secs_of_day = local_secs.rem_euclid(86400);
            let (year, month, day) = civil_from_days(days);
            // 1970-01-01 was a Thursday (weekday 4 with Sunday = 0).
            let weekday = (days + 4).rem_euclid(7);
            let obj = vm.bx.heap.new_object();
            let trap = vm.bx.threads.cur().trap.pass();
            vm.bx.heap.set_value(obj, id!(year).into(), (year as f64).into(), trap);
            vm.bx.heap.set_value(obj, id!(month).into(), (month as f64).into(), trap);
            vm.bx.heap.set_value(obj, id!(day).into(), (day as f64).into(), trap);
            vm.bx.heap.set_value(obj, id!(hour).into(), ((secs_of_day / 3600) as f64).into(), trap);
            vm.bx.heap.set_value(obj, id!(minute).into(), ((secs_of_day / 60 % 60) as f64).into(), trap);
            vm.bx.heap.set_value(obj, id!(second).into(), ((secs_of_day % 60) as f64).into(), trap);
            vm.bx.heap.set_value(obj, id!(weekday).into(), (weekday as f64).into(), trap);
            obj.into()
        },
    );

    vm.add_method(
        std,
        id_lut!(start_timeout),
        script_args_def!(delay = NIL, callback = NIL),
        |vm, args| {
            let delay = script_value!(vm, args.delay);
            let callback = script_value!(vm, args.callback);
            if !delay.is_number() || !vm.bx.heap.is_fn(callback.into()) {
                return script_err_type_mismatch!(vm.trap(), "invalid timer arg type");
            }
            let callback = ScriptFnRef::script_from_value(vm, callback);

            let heap_key = vm.bx.heap.heap_key();
            let cx = vm.cx_mut();
            // A refused timer answers NIL rather than trapping. The embedder
            // that set the cap already knows it was hit (it is what refused
            // it), and a script that asks for one timer too many should be
            // able to cope — `if t.is_nil()` — instead of dying mid-function.
            let Some(delay) = cx.admit_script_timer(heap_key, delay.as_number().unwrap_or(1.0))
            else {
                return NIL;
            };
            let cx = vm.cx_mut();
            let timer = cx.start_timeout(delay);

            let id = LiveId::unique();
            cx.script_data.timers.timers.push(CxScriptTimer {
                repeat: false,
                timer,
                id,
                callback,
            });
            id.escape()
        },
    );

    vm.add_method(
        std,
        id_lut!(start_interval),
        script_args_def!(delay = NIL, callback = NIL),
        |vm, args| {
            let delay = script_value!(vm, args.delay);
            let callback = script_value!(vm, args.callback);

            if !delay.is_number() || !ScriptFnRef::script_type_check(&vm.bx.heap, callback) {
                return script_err_type_mismatch!(vm.trap(), "invalid timer arg type");
            }
            let callback = ScriptFnRef::script_from_value(vm, callback);

            let heap_key = vm.bx.heap.heap_key();
            let cx = vm.cx_mut();
            // See start_timeout: over the cap, NIL rather than a trap.
            let Some(delay) = cx.admit_script_timer(heap_key, delay.as_number().unwrap_or(1.0))
            else {
                return NIL;
            };
            let cx = vm.cx_mut();

            let timer = cx.start_interval(delay);

            let id = LiveId::unique();
            cx.script_data.timers.timers.push(CxScriptTimer {
                repeat: true,
                timer,
                id,
                callback,
            });
            id.escape()
        },
    );

    vm.add_method(
        std,
        id_lut!(stop_timer),
        script_args_def!(timer = NIL),
        |vm, args| {
            let timer = script_value!(vm, args.timer);
            if !timer.is_id() {
                return script_err_type_mismatch!(vm.trap(), "invalid timer arg type");
            }
            let timer = timer.as_id().unwrap_or(id!());
            // Read the heap BEFORE borrowing cx out of vm.
            let heap_key = vm.bx.heap.heap_key();
            let cx = vm.cx_mut();
            // Only your own timers, and actually STOP them: dropping the
            // bookkeeping entry alone left the OS timer firing for the life of
            // the process, invisible to every teardown path (they all filter
            // this same Vec), and still costing a full event pass per fire.
            let found = cx
                .script_data
                .timers
                .timers
                .iter()
                .position(|v| v.id == timer && v.callback.heap_key() == heap_key);
            if let Some(i) = found {
                let entry = cx.script_data.timers.timers.remove(i);
                cx.stop_timer(entry.timer);
                cx.release_script_timer(heap_key);
            }
            NIL
        },
    );
}
