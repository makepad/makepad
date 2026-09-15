use crate::local_time::LocalTime;
use makepad_widgets::*;
use makepad_widgets::makepad_micro_serde::*;
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AlarmSpec {
    pub id: u64,
    pub hour: u32,
    pub minute: u32,
    pub label: String,
    pub enabled: bool,
}
impl AlarmSpec {
    pub fn time(&self) -> String { format!("{:02}:{:02}", self.hour, self.minute) }
}

pub struct Alarm {
    pub spec: AlarmSpec,
    pub ringing: bool,
    last_day: Option<(i32, u32, u32)>,
    snooze: Option<Instant>,
}
impl Alarm {
    fn new(spec: AlarmSpec) -> Self { Self { spec, ringing: false, last_day: None, snooze: None } }
    fn tick_at(&mut self, now: LocalTime, monotonic: Instant) {
        if !self.spec.enabled { return; }
        if self.snooze.is_some_and(|t| monotonic >= t) { self.snooze = None; self.ringing = true; }
        let day = (now.year, now.month, now.day);
        if self.last_day != Some(day) && now.hour == self.spec.hour && now.minute == self.spec.minute {
            self.last_day = Some(day); self.ringing = true;
        }
    }
    fn stop(&mut self) { self.ringing = false; self.snooze = None; }
}

#[derive(Default)]
pub struct AlarmBook {
    pub alarms: Vec<Alarm>,
    next_id: u64,
}
impl AlarmBook {
    pub fn specs(&self) -> Vec<AlarmSpec> { self.alarms.iter().map(|a| a.spec.clone()).collect() }
    pub fn find(&self, id: u64) -> Option<&AlarmSpec> { self.alarms.iter().find(|a| a.spec.id == id).map(|a| &a.spec) }
    pub fn save(&mut self, id: Option<u64>, hour: u32, minute: u32, label: String) -> u64 {
        if let Some(alarm) = id.and_then(|id| self.alarms.iter_mut().find(|a| a.spec.id == id)) {
            alarm.spec.hour = hour % 24; alarm.spec.minute = minute % 60; alarm.spec.label = label;
            alarm.stop();
            return alarm.spec.id;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.alarms.push(Alarm::new(AlarmSpec { id, hour: hour % 24, minute: minute % 60, label, enabled: true }));
        id
    }
    pub fn toggle(&mut self, id: u64) {
        if let Some(a) = self.alarms.iter_mut().find(|a| a.spec.id == id) {
            a.spec.enabled = !a.spec.enabled;
            if !a.spec.enabled { a.stop(); }
        }
    }
    pub fn remove(&mut self, id: u64) { self.alarms.retain(|a| a.spec.id != id); }
    pub fn tick(&mut self, now: LocalTime) {
        let monotonic = Instant::now();
        for alarm in &mut self.alarms { alarm.tick_at(now, monotonic); }
    }
    pub fn ringing(&self) -> bool { self.alarms.iter().any(|a| a.ringing) }
    pub fn stop(&mut self) { for a in self.alarms.iter_mut().filter(|a| a.ringing) { a.stop(); } }
    pub fn snooze(&mut self) {
        for a in self.alarms.iter_mut().filter(|a| a.ringing) {
            a.ringing = false; a.snooze = Some(Instant::now() + Duration::from_secs(300));
        }
    }
    pub fn next(&self, now: LocalTime) -> Option<&AlarmSpec> {
        let minute = now.hour * 60 + now.minute;
        self.alarms.iter().filter(|a| a.spec.enabled)
            .min_by_key(|a| (a.spec.hour * 60 + a.spec.minute + 1440 - minute) % 1440)
            .map(|a| &a.spec)
    }
    pub fn encode(&self) -> Vec<u8> { self.specs().serialize_json().into_bytes() }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 256 * 1024 { return None; }
        let specs = Vec::<AlarmSpec>::deserialize_json(std::str::from_utf8(bytes).ok()?).ok()?;
        let mut book = Self::default();
        for spec in specs.into_iter().take(1000) {
            if spec.id == 0 || spec.id == u64::MAX || spec.hour >= 24 || spec.minute >= 60 || spec.label.len() > 512 || book.find(spec.id).is_some() { return None; }
            book.next_id = book.next_id.max(spec.id);
            book.alarms.push(Alarm::new(spec));
        }
        Some(book)
    }
}

/// One persistent audio callback. The UI changes only its atomic gate.
pub fn install_sound(cx: &mut Cx) -> Arc<AtomicBool> {
    let signal = Arc::new(AtomicBool::new(false));
    let gate = signal.clone();
    let mut time = 0.0f64;
    cx.audio_output(0, move |info, output| {
        output.zero();
        if !gate.load(Ordering::Relaxed) { time = 0.0; return; }
        let rate = info.sample_rate.max(1.0);
        for frame in 0..output.frame_count() {
            let beat = time % 1.6;
            let pulse = beat % 0.4;
            let envelope = if beat < 1.2 && pulse < 0.24 { (std::f64::consts::PI * pulse / 0.24).sin().powi(2) } else { 0.0 };
            let sample = ((time * std::f64::consts::TAU * 660.0).sin() * envelope * 0.12) as f32;
            for channel in 0..output.channel_count() { output.channel_mut(channel)[frame] = sample; }
            time += 1.0 / rate;
        }
    });
    signal
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alarms_ring_independently_once_per_day_and_remove_by_stable_id() {
        let mut book = AlarmBook::default();
        let a = book.save(None, 7, 0, "Morning".into());
        let b = book.save(None, 7, 1, "Tea".into());
        let mut now = LocalTime {year: 2026, month: 9, day: 6, hour: 7, minute: 0, ..Default::default()};
        book.tick(now); assert!(book.alarms[0].ringing && !book.alarms[1].ringing);
        book.stop(); book.tick(now); assert!(!book.ringing());
        now.minute = 1; book.tick(now); assert!(book.alarms[1].ringing);
        book.remove(a); assert_eq!(book.alarms[0].spec.id, b);
        book.remove(b); assert!(!book.ringing());
    }
    #[test]
    fn snoozing_one_alarm_does_not_change_another_and_disabling_cancels_it() {
        let mut book = AlarmBook::default();
        let a = book.save(None, 7, 0, String::new()); book.save(None, 8, 0, String::new());
        let now = LocalTime {year:2026, month:9, day:6, hour:7, minute:0, ..Default::default()};
        book.tick(now); book.snooze();
        assert!(book.alarms[0].snooze.is_some() && book.alarms[1].snooze.is_none());
        book.toggle(a); assert!(book.alarms[0].snooze.is_none());
        book.alarms[0].tick_at(now, Instant::now() + Duration::from_secs(400)); assert!(!book.ringing());
    }
    #[test]
    fn persisted_alarms_keep_ids_and_settings_without_resuming_a_ring() {
        let mut book = AlarmBook::default();
        let id = book.save(None, 6, 30, "Early train".into());
        let mut restored = AlarmBook::decode(&book.encode()).unwrap();
        assert_eq!(restored.find(id), book.find(id)); assert!(!restored.ringing());
        assert!(restored.save(None, 9, 0, String::new()) > id);
    }
}
