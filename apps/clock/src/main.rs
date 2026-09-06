//! clock — current local time, a stopwatch and a countdown timer. One app
//! process owns one model and two resident faces on the same window: the
//! full face and a compact home-widget tile. The host picks the face with
//! `HostedViewMode` over `StudioToApp::Custom`; state, timers and view
//! identities survive the switch because nothing is rebuilt.

pub use makepad_widgets;
use makepad_widgets::*;
use std::time::Instant;

mod face;
mod ui;
mod alarm;
use alarm::AlarmBook;
mod wheel;
mod alarm_list;
use wheel::TimeWheel;
use alarm_list::{AlarmList, AlarmListAction};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
mod local_time;
use face::ClockFace;
use local_time::LocalTime;

app_main!(App);

/// Elapsed time accumulated across start/pause cycles.
#[derive(Default)]
struct Stopwatch {
    base: f64,
    started: Option<Instant>,
}

impl Stopwatch {
    fn running(&self) -> bool {
        self.started.is_some()
    }
    fn elapsed(&self) -> f64 {
        self.base + self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }
    fn toggle(&mut self) {
        match self.started.take() {
            Some(s) => self.base += s.elapsed().as_secs_f64(),
            None => self.started = Some(Instant::now()),
        }
    }
    fn reset(&mut self) {
        self.base = 0.0;
        self.started = None;
    }
}

/// A countdown that counts wall time while running; it only ever reports
/// reaching zero on screen (no notification is delivered).
struct Countdown {
    /// The duration the next Reset returns to.
    preset: f64,
    /// Seconds left at the moment it was last started or paused.
    remaining_at: f64,
    started: Option<Instant>,
}

impl Default for Countdown {
    fn default() -> Self {
        Self { preset: 300.0, remaining_at: 300.0, started: None }
    }
}

impl Countdown {
    fn running(&self) -> bool {
        self.started.is_some()
    }
    fn remaining(&self) -> f64 {
        let run = self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        (self.remaining_at - run).max(0.0)
    }
    fn finished(&self) -> bool {
        self.remaining() <= 0.0 && self.preset > 0.0
    }
    /// Freeze the running clock into `remaining_at`; stops when it hit zero.
    fn settle(&mut self) {
        if let Some(s) = self.started {
            let left = (self.remaining_at - s.elapsed().as_secs_f64()).max(0.0);
            if left <= 0.0 {
                self.remaining_at = 0.0;
                self.started = None;
            }
        }
    }
    fn toggle(&mut self) {
        match self.started.take() {
            Some(s) => self.remaining_at = (self.remaining_at - s.elapsed().as_secs_f64()).max(0.0),
            None => {
                if self.remaining_at <= 0.0 {
                    self.remaining_at = self.preset;
                }
                if self.remaining_at > 0.0 {
                    self.started = Some(Instant::now());
                }
            }
        }
    }
    fn reset(&mut self) {
        self.started = None;
        self.remaining_at = self.preset;
    }
    fn adjust(&mut self, delta: f64) {
        let was_running = self.started.take().map(|s| s.elapsed().as_secs_f64());
        if let Some(run) = was_running {
            self.remaining_at = (self.remaining_at - run).max(0.0);
        }
        self.preset = (self.preset + delta).clamp(0.0, 24.0 * 3600.0);
        self.remaining_at = (self.remaining_at + delta).clamp(0.0, 24.0 * 3600.0);
        if was_running.is_some() && self.remaining_at > 0.0 {
            self.started = Some(Instant::now());
        }
    }
}

fn mm_ss_tenths(secs: f64) -> String {
    let tenths = (secs * 10.0).floor() as u64;
    let (m, s, t) = (tenths / 600, (tenths / 10) % 60, tenths % 10);
    if m >= 100 {
        format!("{}:{:02}:{:02}.{}", m / 60, m % 60, s, t)
    } else {
        format!("{:02}:{:02}.{}", m, s, t)
    }
}

fn hh_mm_ss(secs: f64) -> String {
    let whole = secs.ceil().max(0.0) as u64;
    let (h, m, s) = (whole / 3600, (whole / 60) % 60, whole % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// Once a second: wall time, countdown, and the paused stopwatch.
    #[rust]
    tick: Option<Timer>,
    /// Only while the stopwatch runs: tenths on screen.
    #[rust]
    fast: Option<Timer>,
    #[rust]
    stopwatch: Stopwatch,
    #[rust]
    countdown: Countdown,
    #[rust]
    last_time: Option<LocalTime>,
    #[rust]
    mode: LiveId,
    #[rust]
    alarm: AlarmBook,
    #[rust] alarm_edit_id: Option<u64>,
    #[rust] alarm_load: Option<StorageRequestId>,
    #[rust] alarm_write: Option<StorageRequestId>,
    #[rust] alarm_dirty: bool,
    #[rust] alarm_changed: bool,
    #[rust]
    alarm_signal: Arc<AtomicBool>,
    #[rust]
    countdown_alert: bool,
    #[rust]
    laps: Vec<f64>,
}

impl App {
    fn select_mode(&mut self,cx:&mut Cx,mode:LiveId) {
        self.mode=mode;
        self.ui.page_flip(cx,ids!(modes)).set_active_page(cx,mode);
        for (tab,page,title) in [(ids!(tab_clock),live_id!(clock_page),"Clock"),(ids!(tab_alarm),live_id!(alarm_page),"Alarm"),(ids!(tab_stopwatch),live_id!(stopwatch_page),"Stopwatch"),(ids!(tab_timer),live_id!(timer_page),"Timer")] {
            self.ui.radio_button(cx,tab).set_active(cx,mode==page,Animate::No);
            if mode==page {self.ui.label(cx,ids!(mode_title)).set_text(cx,title);}
        }
    }

    fn edit_alarm(&mut self, cx: &mut Cx, id: Option<u64>) {
        self.alarm_edit_id = id;
        let spec = id.and_then(|id| self.alarm.find(id)).cloned();
        let (hour, minute) = spec.as_ref().map(|a| (a.hour,a.minute)).unwrap_or((7,0));
        if let Some(mut wheel) = self.ui.widget(cx, ids!(alarm_wheel)).borrow_mut::<TimeWheel>() { wheel.set_time(cx,hour,minute); }
        self.ui.text_input(cx,ids!(alarm_label)).set_text(cx,spec.as_ref().map(|a| a.label.as_str()).unwrap_or(""));
        self.ui.label(cx,ids!(alarm_editor_title)).set_text(cx,if id.is_some() {"Edit alarm"} else {"New alarm"});
        self.ui.page_flip(cx,ids!(alarm_page)).set_active_page(cx,live_id!(alarm_edit_page));
    }
    fn alarms_modified(&mut self,cx:&mut Cx) {
        self.alarm_changed=true; self.alarm_dirty=true; self.persist_alarms(cx); self.refresh_alarm(cx);
    }
    fn persist_alarms(&mut self,cx:&mut Cx) {
        if !self.alarm_dirty || self.alarm_write.is_some() {return;}
        self.alarm_write=Some(cx.storage("clock").set(cx,"alarms",self.alarm.encode()));
        self.alarm_dirty=false;
    }
    fn alarm_storage(&mut self,cx:&mut Cx,responses:&[StorageResponse]) {
        for response in responses {
            if self.alarm_load==Some(response.request_id) {
                self.alarm_load=None;
                if !self.alarm_changed {
                    if let Ok(StorageResult::Value(Some(bytes)))=&response.result {
                        if let Some(book)=AlarmBook::decode(bytes) {self.alarm=book;self.refresh_alarm(cx);}
                    }
                }
            }
            if self.alarm_write==Some(response.request_id) {
                self.alarm_write=None;
                if let Err(error)=&response.result {
                    log!("clock: alarm save failed: {}",error);
                    self.ui.label(cx,ids!(alarm_note)).set_text(cx,"Could not save alarms. Changes remain available this session.");
                }
                self.persist_alarms(cx);
            }
        }
    }

    fn refresh_alarm(&mut self,cx:&mut Cx) {
        if let Some(mut list) = self.ui.widget(cx, ids!(alarm_list)).borrow_mut::<AlarmList>() {
            list.set_rows(cx, self.alarm.specs());
        }
        let next = self.alarm.next(LocalTime::now()).map(|a| format!("{} · Every day", a.time())).unwrap_or_else(|| "No alarm set".into());
        self.ui.label(cx,ids!(next_alarm_text)).set_text(cx,&next);
        let ringing=self.alarm.ringing()||self.countdown_alert;
        self.alarm_signal.store(ringing,Ordering::Relaxed);
        self.ui.widget(cx,ids!(alert_panel)).set_visible(cx,ringing);
        self.ui.widget(cx,ids!(alert_snooze)).set_visible(cx,self.alarm.ringing());
        self.ui.label(cx,ids!(alert_title)).set_text(cx,if self.alarm.ringing() {"Alarm"}else{"Time is up"});
        self.refresh_tile_extra(cx);
    }

    fn refresh_clock(&mut self, cx: &mut Cx, force: bool) {
        let now = LocalTime::now();
        if !force && self.last_time == Some(now) {
            return;
        }
        self.last_time = Some(now);
        for id in [ids!(face_full), ids!(face_tile)] {
            if let Some(mut face) = self.ui.widget(cx, id).borrow_mut::<ClockFace>() {
                face.set_time(cx, now.hour, now.minute, now.second);
            }
        }
        self.ui.label(cx, ids!(time_full)).set_text(cx, &now.hm());
        self.ui.label(cx, ids!(header_date)).set_text(cx, &now.date_text());
        self.ui.label(cx, ids!(date_full)).set_text(cx, &now.date_text());
        self.ui.label(cx, ids!(time_tile)).set_text(cx, &now.hm());
        self.ui.label(cx, ids!(date_tile)).set_text(cx, &now.date_text());
    }

    fn refresh_stopwatch(&mut self, cx: &mut Cx) {
        let text = mm_ss_tenths(self.stopwatch.elapsed());
        self.ui.label(cx, ids!(sw_readout)).set_text(cx, &text);
        let toggle = if self.stopwatch.running() { "Pause" } else { "Start" };
        self.ui.button(cx, ids!(sw_toggle)).set_text(cx, toggle);
        self.refresh_tile_extra(cx);
    }

    fn refresh_countdown(&mut self, cx: &mut Cx) {
        let was_running=self.countdown.running();
        self.countdown.settle();
        if was_running && self.countdown.finished() {self.countdown_alert=true;self.refresh_alarm(cx);}
        let text = hh_mm_ss(self.countdown.remaining());
        self.ui.label(cx, ids!(cd_readout)).set_text(cx, &text);
        let status = if self.countdown.finished() {
            "Time is up".to_string()
        } else if self.countdown.running() {
            format!("Running · set for {}", hh_mm_ss(self.countdown.preset))
        } else {
            format!("Paused · set for {}", hh_mm_ss(self.countdown.preset))
        };
        self.ui.label(cx, ids!(cd_status)).set_text(cx, &status);
        let toggle = if self.countdown.running() { "Pause" } else { "Start" };
        self.ui.button(cx, ids!(cd_toggle)).set_text(cx, toggle);
        self.refresh_tile_extra(cx);
    }

    /// The tile's third line: whichever of countdown/stopwatch is live.
    fn refresh_tile_extra(&mut self, cx: &mut Cx) {
        let extra = if self.alarm.ringing() {"Alarm · tap to open".into()} else if self.countdown.running() {
            format!("Timer {}", hh_mm_ss(self.countdown.remaining()))
        } else if self.countdown.finished() {
            "Timer done".to_string()
        } else if self.stopwatch.running() {
            format!("Stopwatch {}", hh_mm_ss(self.stopwatch.elapsed().floor()))
        } else if self.stopwatch.elapsed() > 0.0 {
            format!("Stopwatch paused {}", hh_mm_ss(self.stopwatch.elapsed().floor()))
        } else {
            String::new()
        };
        self.ui.label(cx, ids!(extra_tile)).set_text(cx, &extra);
    }

    fn sync_fast_timer(&mut self, cx: &mut Cx) {
        if self.stopwatch.running() {
            if self.fast.is_none() {
                self.fast = Some(cx.start_interval(0.1));
            }
        } else if let Some(t) = self.fast.take() {
            cx.stop_timer(t);
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        makepad_wm_api::set_title(cx, "Clock");
        self.alarm_signal=alarm::install_sound(cx);
        self.alarm_load = Some(cx.storage("clock").get(cx, "alarms"));
        self.select_mode(cx,live_id!(clock_page));
        self.refresh_alarm(cx);
        self.tick = Some(cx.start_interval(1.0));
        self.refresh_clock(cx, true);
        self.refresh_stopwatch(cx);
        self.refresh_countdown(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for (tab,page) in [(ids!(tab_clock),live_id!(clock_page)),(ids!(tab_alarm),live_id!(alarm_page)),(ids!(tab_stopwatch),live_id!(stopwatch_page)),(ids!(tab_timer),live_id!(timer_page))] {
            if self.ui.radio_button(cx,tab).clicked(actions) {self.select_mode(cx,page);}
        }
        if self.ui.button(cx,ids!(open_alarm)).clicked(actions) {self.select_mode(cx,live_id!(alarm_page));}
        if self.ui.button(cx, ids!(alarm_add)).clicked(actions) { self.edit_alarm(cx, None); }
        if self.ui.button(cx, ids!(alarm_cancel)).clicked(actions) {
            self.ui.page_flip(cx, ids!(alarm_page)).set_active_page(cx, live_id!(alarm_list_page));
        }
        if self.ui.button(cx, ids!(alarm_save)).clicked(actions) {
            let time = self.ui.widget(cx, ids!(alarm_wheel)).borrow::<TimeWheel>().map(|wheel| wheel.time()).unwrap_or((7,0));
            let label = self.ui.text_input(cx, ids!(alarm_label)).text().chars().take(80).collect();
            self.alarm.save(self.alarm_edit_id, time.0, time.1, label);
            self.alarm_changed = true; self.alarm_dirty = true; self.persist_alarms(cx); self.refresh_alarm(cx);
            self.ui.page_flip(cx, ids!(alarm_page)).set_active_page(cx, live_id!(alarm_list_page));
        }
        let uid = self.ui.widget(cx, ids!(alarm_list)).widget_uid();
        for action in actions.filter_widget_actions_cast::<AlarmListAction>(uid) {
            match action {
                AlarmListAction::Edit(id) => self.edit_alarm(cx, Some(id)),
                AlarmListAction::Toggle(id) => {self.alarm.toggle(id); self.alarms_modified(cx);}
                AlarmListAction::Remove(id) => {self.alarm.remove(id); self.alarms_modified(cx);}
                _ => {}
            }
        }
        if self.ui.button(cx,ids!(alert_stop)).clicked(actions) {self.alarm.stop();self.countdown_alert=false;self.refresh_alarm(cx);}
        if self.ui.button(cx,ids!(alert_snooze)).clicked(actions) {self.alarm.snooze();self.refresh_alarm(cx);}
        if self.ui.button(cx,ids!(sw_lap)).clicked(actions) && self.stopwatch.running() {
            let elapsed=self.stopwatch.elapsed();
            if self.laps.len()<32 {self.laps.push(elapsed);}
            let text=self.laps.iter().enumerate().rev().take(6).map(|(i,t)|format!("Lap {:02}    {}",i+1,mm_ss_tenths(*t))).collect::<Vec<_>>().join("\n");
            self.ui.label(cx,ids!(lap_readout)).set_text(cx,&text);
        }
        if self.ui.button(cx, ids!(sw_toggle)).clicked(actions) {
            self.stopwatch.toggle();
            self.sync_fast_timer(cx);
            self.refresh_stopwatch(cx);
        }
        if self.ui.button(cx, ids!(sw_reset)).clicked(actions) {
            self.laps.clear();
            self.ui.label(cx,ids!(lap_readout)).set_text(cx,"Your laps will appear here");
            self.stopwatch.reset();
            self.sync_fast_timer(cx);
            self.refresh_stopwatch(cx);
        }
        if self.ui.button(cx, ids!(cd_toggle)).clicked(actions) {
            self.countdown_alert=false;self.refresh_alarm(cx);
            self.countdown.toggle();
            self.refresh_countdown(cx);
        }
        if self.ui.button(cx, ids!(cd_reset)).clicked(actions) {
            self.countdown_alert=false;self.refresh_alarm(cx);
            self.countdown.reset();
            self.refresh_countdown(cx);
        }
        if self.ui.button(cx, ids!(cd_minus)).clicked(actions) {
            self.countdown.adjust(-60.0);
            self.refresh_countdown(cx);
        }
        if self.ui.button(cx, ids!(cd_plus)).clicked(actions) {
            self.countdown.adjust(60.0);
            self.refresh_countdown(cx);
        }
        if self.ui.button(cx, ids!(cd_plus5)).clicked(actions) {
            self.countdown.adjust(300.0);
            self.refresh_countdown(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        face::script_mod(vm);
        wheel::script_mod(vm);
        alarm_list::script_mod(vm);
        ui::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Storage(responses)=event {self.alarm_storage(cx,responses);}
        if let Event::AudioDevices(devices)=event {cx.use_audio_outputs(&devices.default_output());}
        if let Event::WindowGeomChange(e)=event {
            let face=if e.new_geom.inner_size.y<500.0 {172.0}else{268.0};
            let mut widget=self.ui.widget(cx,ids!(face_full));
            script_apply_eval!(cx,widget,{width:#(face) height:#(face)});
            let small = (e.new_geom.inner_size.y - 30.0).clamp(52.0,112.0);
            let mut tile = self.ui.widget(cx,ids!(face_tile));
            script_apply_eval!(cx,tile,{width:#(small) height:#(small)});
        }
        if matches!(event,Event::LiveEdit) {self.select_mode(cx,self.mode);self.refresh_clock(cx,true);self.refresh_alarm(cx);}
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        if self.tick.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            self.alarm.tick(LocalTime::now());
            self.refresh_alarm(cx);
            self.refresh_clock(cx, false);
            self.refresh_countdown(cx);
            if !self.stopwatch.running() {
                self.refresh_stopwatch(cx);
            }
        }
        if self.fast.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            self.refresh_stopwatch(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readouts_format() {
        assert_eq!(mm_ss_tenths(0.0), "00:00.0");
        assert_eq!(mm_ss_tenths(61.25), "01:01.2");
        assert_eq!(hh_mm_ss(300.0), "05:00");
        assert_eq!(hh_mm_ss(3661.0), "1:01:01");
    }

    #[test]
    fn countdown_adjusts_and_finishes() {
        let mut cd = Countdown::default();
        cd.adjust(-60.0);
        assert_eq!(cd.remaining(), 240.0);
        cd.adjust(-1000.0);
        assert_eq!(cd.remaining(), 0.0);
        assert!(!cd.finished(), "an empty preset never reports done");
        cd.adjust(60.0);
        cd.toggle();
        assert!(cd.running());
        cd.toggle();
        assert!(!cd.running() && cd.remaining() <= 60.0);
    }


}

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
