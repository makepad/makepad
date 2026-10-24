//! Machine activity safety gate. The monitor alone probes the OS. Readers use
//! atomics, including cancellation checks made by independent backend lanes.
//!
//! The window manager that hosts the hub is the second source: it sees games
//! (a fullscreen client, an active gamepad) where an OS probe cannot, and it
//! owns the node's compute mode (`POST /control/local-use`, [`ComputeMode`]).
//! Both feed the same published code, so closing admission, unloading and
//! rerouting run through the one path below.
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Duration, Instant};
use crate::protocol::ActivityJson;

#[cfg(any(target_os = "windows", test))]
mod windows;

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    pub supported: bool,
    pub idle_seconds: u64,
    pub quiet_seconds: u64,
    pub gpu_percent: f64,
    pub stale_ms: u64,
    pub invalid: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self { enabled: cfg!(target_os="windows"), supported: cfg!(target_os="windows"),
            idle_seconds: 300, quiet_seconds: 20, gpu_percent: 15.0, stale_ms: 5000, invalid: false }
    }
}
impl Config {
    pub fn from_env() -> Self { Self::parse(|key| match std::env::var(key) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => Some("invalid-non-unicode-setting".into()),
    }) }
    fn parse(mut env: impl FnMut(&str)->Option<String>) -> Self {
        let mut c = Self::default();
        // Only this exact operator environment setting overrides the gate.
        if let Some(v) = env("MAKEPAD_AI_ACTIVITY") {
            match v.as_str() { "enabled" => c.enabled = true, "operator-disabled" => c.enabled = false,
                _ => { c.enabled = true; c.invalid = true; } }
        }
        for (name, out, min, max) in [
            ("MAKEPAD_AI_ACTIVITY_IDLE_SECONDS", &mut c.idle_seconds, 300, 86400),
            ("MAKEPAD_AI_ACTIVITY_QUIET_SECONDS", &mut c.quiet_seconds, 15, 300),
            ("MAKEPAD_AI_ACTIVITY_STALE_MS", &mut c.stale_ms, 2000, 5000),
        ] {
            if let Some(v) = env(name) { match v.parse::<u64>() {
                Ok(v) if v >= min && v <= max => *out = v, _ => c.invalid = true,
            }}
        }
        if let Some(v) = env("MAKEPAD_AI_ACTIVITY_GPU_PERCENT") { match v.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 && v <= 100.0 => c.gpu_percent = v,
            _ => c.invalid = true,
        }}
        // Invalid configuration always fails closed, even beside an override.
        if c.invalid { c.enabled = true; }
        c
    }
}

/// The node's compute availability, set by the hosting WM (its `ai-hub.conf`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComputeMode {
    /// Local use (a game, or the OS probe where there is one) closes admission
    /// and releases the GPU; it reopens once local use has ended.
    #[default]
    Automatic,
    /// Always admits; the UI's GPU memory still comes first (`/control/yield`).
    UiFirst,
    /// Always admits and never yields GPU memory: the UI may degrade or fail.
    AiFirst,
    /// Admits nothing; models are unloaded.
    Unavailable,
}
impl ComputeMode {
    pub const ALL: [ComputeMode; 4] = [ComputeMode::Automatic, ComputeMode::UiFirst, ComputeMode::AiFirst, ComputeMode::Unavailable];
    pub fn name(self) -> &'static str {
        match self { ComputeMode::Automatic=>"auto", ComputeMode::UiFirst=>"ui-first", ComputeMode::AiFirst=>"ai-first", ComputeMode::Unavailable=>"unavailable" }
    }
    pub fn parse(name: &str) -> Option<Self> { Self::ALL.into_iter().find(|m| m.name()==name) }
    fn from_u64(v: u64) -> Self { Self::ALL.get(v as usize).copied().unwrap_or_default() }
    fn to_u64(self) -> u64 { Self::ALL.iter().position(|m| *m==self).unwrap_or(0) as u64 }
}

#[derive(Clone, Debug)]
pub(super) struct Observation {
    pub idle_seconds: Option<u64>,
    pub fullscreen: Result<bool, &'static str>,
    pub controller_active: Result<bool, &'static str>,
    pub foreign_gpu_percent: Result<f64, &'static str>,
    pub session_error: Option<&'static str>,
}

// Compact reason/status atomics: no private process/session information leaves
// the monitor. An odd sequence or inconsistent read fails closed immediately.
const START: u64=0; const IDLE:u64=1; const INPUT:u64=2; const FULLSCREEN:u64=3;
const CONTROLLER:u64=4; const GPU:u64=5; const QUIET:u64=6; const SESSION_ERROR:u64=7;
const FULLSCREEN_ERROR:u64=8; const CONTROLLER_ERROR:u64=9; const GPU_ERROR:u64=10;
const CONFIG_ERROR:u64=11; const DISABLED:u64=12; const UNSUPPORTED:u64=13; const STALE:u64=14; const RETIRING:u64=15;
const GAMING:u64=16; const UNAVAILABLE:u64=17; const UI_FIRST:u64=18; const AI_FIRST:u64=19; const RESTARTING:u64=20;
fn reason(code:u64)->(&'static str,&'static str) { match code {
    IDLE=>("idle","quiet"), INPUT=>("busy","recent-input"), FULLSCREEN=>("busy","fullscreen"),
    CONTROLLER=>("busy","controller-active"), GPU=>("busy","foreign-gpu-load"), QUIET=>("busy","quiet-hysteresis"),
    SESSION_ERROR=>("unknown","session-visibility"), FULLSCREEN_ERROR=>("unknown","foreground-unavailable"),
    CONTROLLER_ERROR=>("unknown","controller-probe-unavailable"), GPU_ERROR=>("unknown","gpu-counter-unavailable"),
    CONFIG_ERROR=>("unknown","invalid-operator-configuration"), DISABLED=>("disabled","operator-disabled"),
    UNSUPPORTED=>("unsupported","platform-unsupported"), STALE=>("unknown","monitor-stale"),
    RETIRING=>("busy","releasing-models"), GAMING=>("busy","gaming"),
    UNAVAILABLE=>("busy","operator-unavailable"), UI_FIRST=>("disabled","ui-first"), AI_FIRST=>("disabled","ai-first"),
    RESTARTING=>("busy","restarting"),
    _=>("unknown","monitor-starting"),
}}
fn permits(code:u64, config:&Config)->bool { code==IDLE || code==DISABLED || code==UI_FIRST || code==AI_FIRST || (code==UNSUPPORTED && !config.enabled) }
/// What the WM's mode and local-use report make of the OS probe's code.
fn combine(os:u64, mode:ComputeMode, gaming:bool, stopping:bool)->u64 {
    if stopping { return RESTARTING; }
    match mode {
        ComputeMode::Unavailable=>UNAVAILABLE, ComputeMode::AiFirst=>AI_FIRST, ComputeMode::UiFirst=>UI_FIRST,
        ComputeMode::Automatic=>if gaming {GAMING} else {os},
    }
}

const GPU_BUSY_MS: u64 = 3_000;
/// A failed GPU counter read reuses the last good reading for this long. One
/// unreadable sample (a GPU context created or destroyed between two PDH
/// collections, or the warmup after a query reset) happens every few minutes
/// on a working box; failing closed on it closed admission for the quiet
/// period and cancelled running jobs. Sustained load already needs
/// GPU_BUSY_MS to count, so a short blind spot hides nothing new.
const GPU_ERROR_GRACE_MS: u64 = 3_000;

pub struct Policy { config: Config, quiet_since: Option<u64>, controller_last: Option<u64>, last_sample: Option<u64>, gpu_busy_since: Option<u64>, gpu_last_good: Option<(u64,f64)> }
impl Policy {
    pub fn new(config:Config)->Self { Self {config, quiet_since:None,controller_last:None,last_sample:None,gpu_busy_since:None,gpu_last_good:None} }
    /// The foreign GPU load to judge by: this sample's, or within the grace
    /// period after a failed read, the last good one.
    fn gpu_load(&mut self, now:u64, sample:&Result<f64,&'static str>)->Result<f64,&'static str> {
        match sample {
            Ok(v) if v.is_finite() && *v>=0.0 => { self.gpu_last_good=Some((now,*v)); Ok(*v) }
            Ok(_) => Err("gpu_counter_value_invalid"),
            Err(e) => match self.gpu_last_good {
                Some((at,v)) if now>=at && now-at<=GPU_ERROR_GRACE_MS => Ok(v),
                _ => Err(e),
            },
        }
    }
    fn observe(&mut self, now:u64, o:&Observation)->(u64,Option<u64>) {
        if self.config.invalid {return (CONFIG_ERROR,o.idle_seconds)}
        if !self.config.supported {return (UNSUPPORTED,o.idle_seconds)}
        if !self.config.enabled {return (DISABLED,o.idle_seconds)}
        if self.last_sample.is_some_and(|last| now.saturating_sub(last)>self.config.stale_ms || now<last) { self.quiet_since=None; self.gpu_busy_since=None; self.gpu_last_good=None; }
        self.last_sample=Some(now);
        let gpu=self.gpu_load(now,&o.foreign_gpu_percent);
        let c=&self.config;
        if o.controller_active==Ok(true) {self.controller_last=Some(now);}
        let idle=o.idle_seconds.map(|idle| self.controller_last.map_or(idle, |last| idle.min(now.saturating_sub(last)/1000)));
        let mut code=if o.session_error.is_some() || idle.is_none() {SESSION_ERROR}
            else if o.fullscreen.is_err() {FULLSCREEN_ERROR}
            else if o.controller_active.is_err() {CONTROLLER_ERROR}
            else if gpu.is_err() {GPU_ERROR}
            else if o.fullscreen==Ok(true) {FULLSCREEN}
            else if o.controller_active==Ok(true) {CONTROLLER}
            else if idle.unwrap()<c.idle_seconds {INPUT}
            else if gpu.unwrap()>=c.gpu_percent {GPU}
            else {IDLE};
        if code==GPU {
            // Driver upload/copy bursts and background redraws are not proof
            // that a person has resumed using an otherwise idle computer.
            let since=*self.gpu_busy_since.get_or_insert(now);
            if now.saturating_sub(since)<GPU_BUSY_MS { code=IDLE; }
        } else { self.gpu_busy_since=None; }
        if code!=IDLE {self.quiet_since=None;return(code,idle)}
        let since=*self.quiet_since.get_or_insert(now);
        (if now.saturating_sub(since)>=c.quiet_seconds*1000 {IDLE} else {QUIET},idle)
    }
}

#[derive(Debug)]
pub struct ActivityGate {
    config: Config, clock: Instant, sequence:AtomicU64, code:AtomicU64,
    sampled_ms:AtomicU64, idle:AtomicU64, gpu:AtomicU64, epoch:AtomicU64, retiring:AtomicBool,
    probe_error:Mutex<Option<&'static str>>,
    /// The WM's report: [`ComputeMode`] and whether a game runs.
    mode:AtomicU64, gaming:AtomicBool,
    /// The process is stopping (SIGTERM): admission stays closed until it exits.
    stopping:AtomicBool,
    /// The OS probe's last sample (code, idle, gpu), republished with a new WM report.
    /// Held for the whole publication: the monitor and a WM report never interleave.
    os:Mutex<(u64,Option<u64>,Option<f64>)>,
}
impl ActivityGate {
    pub fn new(config:Config)->Arc<Self> {
        let code=if config.invalid {CONFIG_ERROR} else if !config.supported {UNSUPPORTED} else if !config.enabled {DISABLED} else {START};
        Arc::new(Self { config,clock:Instant::now(),sequence:AtomicU64::new(0),code:AtomicU64::new(code),
            sampled_ms:AtomicU64::new(0),idle:AtomicU64::new(u64::MAX),gpu:AtomicU64::new(f64::NAN.to_bits()),epoch:AtomicU64::new(0),retiring:AtomicBool::new(false),probe_error:Mutex::new(None),
            mode:AtomicU64::new(0),gaming:AtomicBool::new(false),stopping:AtomicBool::new(false),os:Mutex::new((code,None,None)) })
    }
    pub fn epoch(&self)->u64 {self.epoch.load(Ordering::SeqCst)}
    fn now(&self)->u64 {self.clock.elapsed().as_millis() as u64}
    fn read(&self)->(u64,u64,Option<u64>,Option<f64>) {
        let before=self.sequence.load(Ordering::SeqCst);
        let mut code=self.code.load(Ordering::SeqCst);
        let age=self.now().saturating_sub(self.sampled_ms.load(Ordering::SeqCst));
        let idle=self.idle.load(Ordering::SeqCst);
        let gpu=f64::from_bits(self.gpu.load(Ordering::SeqCst));
        if before%2!=0 || before!=self.sequence.load(Ordering::SeqCst) || (self.config.enabled && age>self.config.stale_ms) {code=STALE;}
        if permits(code,&self.config) && self.retiring.load(Ordering::SeqCst) {code=RETIRING;}
        (code,age,(idle!=u64::MAX).then_some(idle),gpu.is_finite().then_some(gpu))
    }
    pub fn allows_work(&self)->bool {
        // Do not use the metadata seqlock here: a healthy monitor refreshing
        // an idle sample must not spuriously cancel work during publication.
        let code=self.code.load(Ordering::SeqCst);
        permits(code,&self.config) && !self.retiring.load(Ordering::SeqCst)
            && (!self.config.enabled || self.now().saturating_sub(self.sampled_ms.load(Ordering::SeqCst))<=self.config.stale_ms)
    }
    pub(crate) fn retirement_finished(&self) { self.retiring.store(false,Ordering::SeqCst); }
    pub fn mode(&self)->ComputeMode { ComputeMode::from_u64(self.mode.load(Ordering::SeqCst)) }
    pub fn gaming(&self)->bool { self.gaming.load(Ordering::SeqCst) }
    /// The WM's report (`POST /control/local-use`), effective at once: closing
    /// cancels running jobs as `local-use: …` and unloads like any local use.
    pub fn set_local_use(&self, mode:ComputeMode, gaming:bool) {
        let os=self.os.lock().unwrap();
        self.mode.store(mode.to_u64(),Ordering::SeqCst);
        self.gaming.store(gaming,Ordering::SeqCst);
        self.publish(combine(os.0,mode,gaming,self.stopping.load(Ordering::SeqCst)),os.1,os.2);
    }
    /// The process is about to exit (a restart): admission closes for good, running jobs see
    /// `local-use: restarting` and their clients reroute; the fleet routes elsewhere meanwhile.
    pub fn begin_stop(&self) {
        let os=self.os.lock().unwrap();
        self.stopping.store(true,Ordering::SeqCst);
        self.publish(RESTARTING,os.1,os.2);
    }
    /// The OS probe's sample, under the WM's report.
    fn publish_os(&self,code:u64,idle:Option<u64>,gpu:Option<f64>) {
        let mut os=self.os.lock().unwrap();
        *os=(code,idle,gpu);
        self.publish(combine(code,self.mode(),self.gaming(),self.stopping.load(Ordering::SeqCst)),idle,gpu);
    }
    pub fn refusal(&self)->Option<String> {let code=self.read().0; (!permits(code,&self.config)).then(||format!("local-use: {}",reason(code).1))}
    pub fn snapshot(&self)->ActivityJson {
        let (code,age,idle,gpu)=self.read();let (state,why)=reason(code);
        ActivityJson {version:1, enabled:self.config.enabled, state:state.into(), reason:why.into(),idle_seconds:idle,
            mode:Some(self.mode().name().into()),
            foreign_gpu_percent:gpu,admission_open:permits(code,&self.config),idle_threshold_seconds:self.config.idle_seconds,
            quiet_seconds:self.config.quiet_seconds,gpu_threshold_percent:self.config.gpu_percent,sample_age_ms:age,
            probe_error:if state=="unknown" {self.probe_error.lock().ok().and_then(|e|*e).map(String::from)} else {None}}
    }
    fn publish(&self,code:u64,idle:Option<u64>,gpu:Option<f64>) {
        self.sequence.fetch_add(1,Ordering::SeqCst);
        // Advance on entry to busy: work that missed a short busy interval
        // can never resume under a newly quiet snapshot. Retirement must
        // complete even if a slow load outlasts the quiet interval.
        if !permits(code,&self.config) && permits(self.code.load(Ordering::SeqCst),&self.config) {
            self.retiring.store(true,Ordering::SeqCst);
            self.epoch.fetch_add(1,Ordering::SeqCst);
        }
        self.code.store(code,Ordering::SeqCst);self.idle.store(idle.unwrap_or(u64::MAX),Ordering::SeqCst);
        self.gpu.store(gpu.unwrap_or(f64::NAN).to_bits(),Ordering::SeqCst);
        self.sampled_ms.store(self.now(),Ordering::SeqCst);self.sequence.fetch_add(1,Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn test_publish(&self,open:bool) {self.publish_os(if open {IDLE}else{INPUT},Some(if open{600}else{0}),Some(0.0));}
}

pub struct Monitor { stop:Arc<AtomicBool>, thread:Option<std::thread::JoinHandle<()>> }
impl Monitor {
    pub fn start(gate:Arc<ActivityGate>,jobs:crate::jobs::SharedJobs)->std::io::Result<Self> {
        let stop=Arc::new(AtomicBool::new(false));let stopping=stop.clone();
        let thread=std::thread::Builder::new().name("ai-hub-activity".into()).spawn(move || {
            let mut sampler=Sampler::new(gate.config.clone());
            while !stopping.load(Ordering::Relaxed) {
                sampler.poll(&gate);
                jobs.with(|store|store.interrupt_for_activity());
                std::thread::park_timeout(Duration::from_secs(1));
            }
        })?;
        Ok(Self {stop,thread:Some(thread)})
    }
}
impl Drop for Monitor {fn drop(&mut self){self.stop.store(true,Ordering::Relaxed);if let Some(t)=self.thread.take(){t.thread().unpark();let _=t.join();}}}
struct Sampler {
    policy:Policy,
    #[cfg(target_os="windows")] probe:windows::Probe,
}
impl Sampler {
    fn new(config:Config)->Self {Self {policy:Policy::new(config),#[cfg(target_os="windows")] probe:windows::Probe::new()}}
    fn poll(&mut self,gate:&ActivityGate) {
        #[cfg(target_os="windows")]
        let o=self.probe.sample();
        #[cfg(not(target_os="windows"))]
        let o=Observation {idle_seconds:None,fullscreen:Err("unsupported"),controller_active:Err("unsupported"),foreign_gpu_percent:Err("unsupported"),session_error:Some("unsupported")};
        let (code,idle)=self.policy.observe(gate.now(),&o);
        let error=match code {SESSION_ERROR=>o.session_error, FULLSCREEN_ERROR=>o.fullscreen.err(),
            CONTROLLER_ERROR=>o.controller_active.err(), GPU_ERROR=>o.foreign_gpu_percent.err(), _=>None};
        if let Ok(mut slot)=gate.probe_error.lock() {*slot=error;}
        gate.publish_os(code,idle,o.foreign_gpu_percent.ok());
    }
}
/// Bounded headless evidence. No singleton, ports, cache paths or GPU models.
pub fn run_probe(seconds:u64)->Result<(),crate::AssetAiError> {
    use makepad_micro_serde::SerJson;
    if !(1..=3600).contains(&seconds) {return Err(crate::AssetAiError::Io("activity probe duration must be 1..3600 seconds".into()))}
    let config=Config::from_env();let gate=ActivityGate::new(config.clone());let mut sampler=Sampler::new(config);
    for n in 0..seconds {sampler.poll(&gate);gate.retirement_finished();println!("{}",gate.snapshot().serialize_json());if n+1<seconds {std::thread::sleep(Duration::from_secs(1));}}
    Ok(())
}
#[cfg(any(target_os="windows",test))]
fn tick_idle_seconds(now:u32,last:u32)->u64 {u64::from(now.wrapping_sub(last))/1000}

#[cfg(test)]
mod tests {
    use super::*;
    fn config()->Config {Config{enabled:true,supported:true,..Config::default()}}
    fn quiet()->Observation {Observation{idle_seconds:Some(600),fullscreen:Ok(false),controller_active:Ok(false),foreign_gpu_percent:Ok(0.0),session_error:None}}
    #[test] fn hysteresis_errors_and_controller_hold() {
        let mut p=Policy::new(config());let mut o=quiet();
        for t in 0..20 {assert_eq!(p.observe(t*1000,&o).0,QUIET);}
        assert_eq!(p.observe(20000,&o).0,IDLE);
        o.controller_active=Ok(true);assert_eq!(p.observe(21000,&o).0,CONTROLLER);
        o.controller_active=Ok(false);assert_eq!(p.observe(22000,&o).0,INPUT);
        o.session_error=Some("denied");assert_eq!(p.observe(23000,&o).0,SESSION_ERROR);
        o=quiet();assert_eq!(p.observe(400000,&o).0,QUIET);
    }
    #[test] fn gap_resets_quiet_and_busy_blocks_immediately() {
        let mut p=Policy::new(config());let mut o=quiet();p.observe(0,&o);
        assert_eq!(p.observe(21000,&o).0,QUIET);
        o.fullscreen=Ok(true);assert_eq!(p.observe(22000,&o).0,FULLSCREEN);
        o.fullscreen=Ok(false);
        // A failed read within the grace period reuses the last good value;
        // one that persists past it fails closed.
        o.foreign_gpu_percent=Err("warmup");assert_eq!(p.observe(23000,&o).0,QUIET);
        assert_eq!(p.observe(24000,&o).0,QUIET);
        assert_eq!(p.observe(25000,&o).0,QUIET);
        assert_eq!(p.observe(26000,&o).0,GPU_ERROR);
    }
    #[test] fn single_unreadable_gpu_sample_keeps_admission_open() {
        let mut p=Policy::new(config());let mut o=quiet();
        for t in 0..=20 {p.observe(t*1000,&o);}
        assert_eq!(p.observe(21000,&o).0,IDLE);
        o.foreign_gpu_percent=Err("gpu_counter_sample_unknown");
        assert_eq!(p.observe(22000,&o).0,IDLE);
        o.foreign_gpu_percent=Err("gpu_counter_warmup");
        assert_eq!(p.observe(23000,&o).0,IDLE);
        o.foreign_gpu_percent=Ok(0.0);
        assert_eq!(p.observe(24000,&o).0,IDLE);
        // Without a recent good reading there is nothing to fall back on.
        let mut p=Policy::new(config());
        assert_eq!(p.observe(0,&Observation{foreign_gpu_percent:Err("gpu_counter_warmup"),..quiet()}).0,GPU_ERROR);
    }
    #[test] fn invalid_settings_never_disable() {
        for v in ["NaN","0","101","-1","oops"] {
            let c=Config::parse(|key| match key {"MAKEPAD_AI_ACTIVITY"=>Some("operator-disabled".into()),"MAKEPAD_AI_ACTIVITY_GPU_PERCENT"=>Some(v.into()),_=>None});
            assert!(c.enabled && c.invalid);assert!(!ActivityGate::new(c).allows_work());
        }
    }
    #[test] fn gpu_spikes_do_not_cancel_but_sustained_load_does() {
        let mut p=Policy::new(config());let mut o=quiet();
        for t in 0..=20 {p.observe(t*1000,&o);}
        o.foreign_gpu_percent=Ok(8.0);
        assert_eq!(p.observe(21000,&o).0,IDLE);
        o.foreign_gpu_percent=Ok(90.0);
        for t in 22..25 {assert_eq!(p.observe(t*1000,&o).0,IDLE);}
        o.foreign_gpu_percent=Ok(0.0);
        assert_eq!(p.observe(25000,&o).0,IDLE);
        o.foreign_gpu_percent=Ok(20.0);
        for t in 26..29 {assert_eq!(p.observe(t*1000,&o).0,IDLE);}
        assert_eq!(p.observe(29000,&o).0,GPU);
        o.foreign_gpu_percent=Ok(0.0);
        assert_eq!(p.observe(30000,&o).0,QUIET);
        for t in 31..50 {assert_eq!(p.observe(t*1000,&o).0,QUIET);}
        assert_eq!(p.observe(50000,&o).0,IDLE);
    }
    #[test] fn human_activity_and_invalid_samples_interrupt_during_gpu_grace() {
        for (observation, expected) in [
            (Observation{idle_seconds:Some(0),..quiet()},INPUT),
            (Observation{fullscreen:Ok(true),..quiet()},FULLSCREEN),
            (Observation{controller_active:Ok(true),..quiet()},CONTROLLER),
        ] {
            let mut p=Policy::new(config());let mut o=quiet();
            for t in 0..=20 {p.observe(t*1000,&o);}
            o.foreign_gpu_percent=Ok(90.0);
            assert_eq!(p.observe(21000,&o).0,IDLE);
            assert_eq!(p.observe(22000,&observation).0,expected);
        }
    }
    #[test] fn wm_report_closes_and_reopens_where_the_os_probe_sees_nothing() {
        // A Linux node: no OS probe, admission open by default.
        let g=ActivityGate::new(Config{enabled:false,supported:false,..Config::default()});
        assert!(g.allows_work());
        let epoch=g.epoch();
        g.set_local_use(ComputeMode::Automatic,true);
        assert!(!g.allows_work());assert_eq!(g.refusal().as_deref(),Some("local-use: gaming"));
        assert_ne!(g.epoch(),epoch,"running jobs see the close");
        assert!(!g.snapshot().admission_open);
        // Ended: open again only once the models were released.
        g.set_local_use(ComputeMode::Automatic,false);
        assert!(!g.allows_work());assert_eq!(g.snapshot().reason,"releasing-models");
        g.retirement_finished();
        assert!(g.allows_work());assert!(g.snapshot().admission_open);
        // The forced modes win over a game; Unavailable over everything.
        g.set_local_use(ComputeMode::UiFirst,true);assert!(g.allows_work());assert_eq!(g.snapshot().reason,"ui-first");
        g.set_local_use(ComputeMode::AiFirst,true);assert!(g.allows_work());assert_eq!(g.snapshot().mode.as_deref(),Some("ai-first"));
        g.set_local_use(ComputeMode::Unavailable,false);assert!(!g.allows_work());
        assert_eq!(g.refusal().as_deref(),Some("local-use: operator-unavailable"));
    }
    #[test] fn a_stopping_hub_closes_admission_whatever_the_mode() {
        let g=ActivityGate::new(Config{enabled:false,supported:false,..Config::default()});
        g.set_local_use(ComputeMode::AiFirst,false);assert!(g.allows_work());
        g.begin_stop();
        assert!(!g.allows_work());assert_eq!(g.refusal().as_deref(),Some("local-use: restarting"));
        g.set_local_use(ComputeMode::UiFirst,false);g.retirement_finished();
        assert!(!g.allows_work(),"a later report does not reopen a stopping hub");
    }
    #[test] fn wm_report_survives_os_samples() {
        let g=ActivityGate::new(config());
        g.test_publish(true);g.set_local_use(ComputeMode::Automatic,true);
        g.test_publish(true);assert!(!g.allows_work(),"the next OS sample keeps the game's close");
        g.set_local_use(ComputeMode::UiFirst,false);g.retirement_finished();
        g.test_publish(false);assert!(g.allows_work(),"UI First admits through local input");
        for m in ComputeMode::ALL {assert_eq!(ComputeMode::parse(m.name()),Some(m));}
    }
    #[test] fn stale_and_epoch_cancel_are_latched() {
        let g=ActivityGate::new(Config{stale_ms:0,..config()});g.test_publish(true);
        std::thread::sleep(Duration::from_millis(2));assert!(!g.allows_work());assert_eq!(g.snapshot().reason,"monitor-stale");
        assert_eq!(tick_idle_seconds(500, u32::MAX-499),1);
    }
}
