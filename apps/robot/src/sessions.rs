//! Reviewing a take only chooses inputs to the 3D renderer. Controller commands
//! are issued separately by the live controls in the main application.
use crate::{
    controller::Link,
    recording::{ClearanceEvent, Recording},
    session_track::{SessionTrack, TrackAction},
    workspace::{Point, Workspace},
    App,
};
use makepad_widgets::{
    makepad_platform::thread::{Class, TaskHandle},
    *,
};
use std::{sync::Arc, time::Instant};

pub const SESSION_PAGE: usize = 2;

#[derive(Default)]
struct Analysis {
    events: Arc<Vec<ClearanceEvent>>,
    trail: Vec<Point>,
    latest_event_end: Vec<u64>,
}
enum FileJob {
    Save(TaskHandle<Result<(), String>>),
    Open(u64, TaskHandle<Result<Workspace, String>>),
}

#[derive(Default)]
pub struct Sessions {
    index: Option<usize>,
    position: u64,
    speed_index: usize,
    playing: Option<(Instant, u64)>,
    capture_pending: bool,
    result: Analysis,
    generation: u64,
    recheck: bool,
    checked: bool,
    analysis_error: Option<String>,
    checking: Option<(u64, TaskHandle<Analysis>)>,
    chosen_event: Option<usize>,
    study_version: u64,
    file_job: Option<FileJob>,
    timer: Option<Timer>,
}
const SPEEDS: [f64; 4] = [0.25, 0.5, 1., 2.];

fn stamp(us: u64) -> String {
    let millis = us / 1000;
    format!(
        "{}:{:02}.{:03}",
        millis / 60000,
        (millis / 1000) % 60,
        millis % 1000
    )
}
impl App {
    fn selected_take(&self) -> Option<&Arc<Recording>> {
        self.sessions
            .index
            .and_then(|i| self.model.workspace.recordings.get(i))
    }
    pub fn replay_visible(&self) -> bool {
        self.model.page == SESSION_PAGE
            && !self.sessions.capture_pending
            && self.selected_take().is_some()
    }
    fn arm_work_timer(&mut self, cx: &mut Cx) {
        if self.sessions.timer.is_none() {
            self.sessions.timer = Some(cx.start_interval(0.08));
        }
    }
    fn rebuild_event_choices(&self, cx: &mut Cx) {
        let mut labels = vec!["Choose an event".to_owned()];
        labels.extend(
            self.sessions
                .result
                .events
                .iter()
                .map(|e| format!("{} · {}", stamp(e.time_us), e.message)),
        );
        let list = self.ui.drop_down(cx, ids!(event_list));
        list.set_labels(cx, labels);
        list.set_selected_item(cx, self.sessions.chosen_event.map_or(0, |i| i + 1));
    }
    pub fn study_changed(&mut self) {
        self.sessions.study_version = self.sessions.study_version.wrapping_add(1);
    }
    pub fn zones_changed(&mut self, cx: &mut Cx) {
        self.study_changed();
        self.request_analysis(cx);
    }
    fn request_analysis(&mut self, cx: &mut Cx) {
        self.sessions.generation = self.sessions.generation.wrapping_add(1);
        self.sessions.recheck = self.selected_take().is_some();
        self.sessions.checked = false;
        self.sessions.analysis_error = None;
        self.sessions.chosen_event = None;
        self.sessions.result = Analysis::default();
        self.rebuild_event_choices(cx);
        self.start_analysis(cx);
    }
    fn start_analysis(&mut self, cx: &mut Cx) {
        if !self.sessions.recheck || self.sessions.checking.is_some() {
            return;
        }
        self.sessions.recheck = false;
        let Some(take) = self.selected_take().cloned() else {
            return;
        };
        let workspace = Workspace {
            zones: self.model.workspace.zones.clone(),
            ..Default::default()
        };
        let job = cx.task_pool().submit(Class::Background, move || {
            let events = take.events(&workspace);
            let mut latest = 0;
            let latest_event_end = events
                .iter()
                .map(|e| {
                    latest = latest.max(e.end_us);
                    latest
                })
                .collect();
            Analysis {
                events: Arc::new(events),
                latest_event_end,
                // Visual geometry is bounded; the analyser receives every original sample.
                trail: take.tcp_trail(512),
            }
        });
        match job {
            Ok(handle) => {
                self.sessions.checking = Some((self.sessions.generation, handle));
                self.arm_work_timer(cx);
            }
            Err(e) => {
                self.sessions.analysis_error = Some(format!("Clearance review unavailable · {e}"))
            }
        }
    }
    pub fn session_timer(&self, event: &Event) -> bool {
        self.sessions
            .timer
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
    }
    pub fn poll_session_work(&mut self, cx: &mut Cx) {
        let mut changed = false;
        if let Some((version, result)) = self
            .sessions
            .checking
            .as_mut()
            .and_then(|(v, t)| t.try_take().map(|r| (*v, r)))
        {
            self.sessions.checking = None;
            if version == self.sessions.generation {
                match result {
                    Ok(data) => {
                        self.sessions.result = data;
                        self.sessions.checked = true;
                        self.rebuild_event_choices(cx);
                    }
                    Err(e) => {
                        self.sessions.analysis_error =
                            Some(format!("Clearance review unavailable · {e}"))
                    }
                }
                changed = true;
            }
            self.start_analysis(cx);
        }
        let mut opened = None;
        let file_result = match self.sessions.file_job.as_mut() {
            Some(FileJob::Save(job)) => job.try_take().map(|r| {
                r.map_err(|e| e.to_string())
                    .and_then(|r| r)
                    .map(|_| "Saved study on this computer.".to_owned())
            }),
            Some(FileJob::Open(version, job)) => job.try_take().map(|r| {
                let cell = r.map_err(|e| e.to_string())??;
                if *version != self.sessions.study_version {
                    return Err(
                        "Study changed while opening. Open it again to replace the current study."
                            .into(),
                    );
                }
                opened = Some(cell);
                Ok("Opened saved study.".into())
            }),
            None => None,
        };
        if let Some(result) = file_result {
            self.sessions.file_job = None;
            self.model.notice = result.unwrap_or_else(|e| e);
            changed = true;
        }
        if let Some(cell) = opened {
            self.model.play = None;
            self.model.cursor = 0.;
            self.model.trail = cell.flange_trail();
            self.model.workspace = cell;
            self.model.zone = 0;
            self.study_changed();
            self.corners_clear(cx);
            self.zone_list(cx);
            self.ui.slider(cx, ids!(scrub)).set_value(cx, 0.);
            self.select_take(cx, self.model.workspace.recordings.len().checked_sub(1));
            self.rebuild_take_choices(cx);
        }
        if self.sessions.capture_pending {
            self.collect_capture(cx);
            changed = true;
        }
        if self.sessions.file_job.is_none()
            && self.sessions.checking.is_none()
            && !self.sessions.capture_pending
        {
            if let Some(t) = self.sessions.timer.take() {
                cx.stop_timer(t);
            }
        }
        if changed {
            self.refresh(cx);
        }
    }
    pub fn study_file(&mut self, cx: &mut Cx, save: bool) {
        if self.sessions.file_job.is_some() || self.sessions.capture_pending {
            return;
        }
        let path = match crate::study_path() {
            Ok(p) => p,
            Err(e) => {
                self.model.notice = e;
                return;
            }
        };
        let result = if save {
            let snapshot = self.model.workspace.clone();
            cx.task_pool()
                .submit(Class::Background, move || snapshot.save(&path))
                .map(FileJob::Save)
        } else {
            let version = self.sessions.study_version;
            cx.task_pool()
                .submit(Class::Background, move || Workspace::load(&path))
                .map(|t| FileJob::Open(version, t))
        };
        match result {
            Ok(job) => {
                self.sessions.file_job = Some(job);
                self.model.notice = if save {
                    "Saving study…"
                } else {
                    "Opening study…"
                }
                .into();
                self.arm_work_timer(cx);
            }
            Err(e) => self.model.notice = e.to_string(),
        }
    }
    fn rebuild_take_choices(&self, cx: &mut Cx) {
        let mut names: Vec<_> = self
            .model
            .workspace
            .recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "{} {}  ·  {}",
                    if r.example { "Example" } else { "Session" },
                    i + 1,
                    stamp(r.duration_us())
                )
            })
            .collect();
        if names.is_empty() {
            names.push("No recordings".into());
        }
        let list = self.ui.drop_down(cx, ids!(take_list));
        list.set_labels(cx, names);
        list.set_selected_item(cx, self.sessions.index.unwrap_or(0));
    }
    fn select_take(&mut self, cx: &mut Cx, index: Option<usize>) {
        self.sessions.index = index;
        self.sessions.position = 0;
        self.sessions.playing = None;
        self.request_analysis(cx);
    }
    pub fn collect_capture(&mut self, cx: &mut Cx) {
        let Some(finished) = self.controller.take_completed_capture() else {
            return;
        };
        self.sessions.capture_pending = false;
        if finished.samples.is_empty() {
            self.model.notice = format!("No samples captured · {}", finished.reason);
            return;
        }
        let recording = Recording {
            samples: finished.samples,
            tool_offset: finished.tool_offset,
            reason: finished.reason,
            example: false,
        };
        self.model.notice = format!(
            "Recorded {} samples · {}",
            recording.samples.len(),
            recording.reason
        );
        self.model.workspace.recordings.push(Arc::new(recording));
        self.study_changed();
        self.select_take(cx, self.model.workspace.recordings.len().checked_sub(1));
        self.rebuild_take_choices(cx);
    }
    pub fn pause_session(&mut self) {
        self.sessions.playing = None;
    }
    pub fn session_playing(&self) -> bool {
        self.sessions.playing.is_some()
    }
    fn seek_take(&mut self, cx: &mut Cx, time: u64, chosen: Option<usize>) {
        let Some(duration) = self.selected_take().map(|r| r.duration_us()) else {
            return;
        };
        self.sessions.playing = None;
        self.sessions.position = time.min(duration);
        self.sessions.chosen_event = chosen;
        self.ui
            .drop_down(cx, ids!(event_list))
            .set_selected_item(cx, chosen.map_or(0, |i| i + 1));
    }
    fn jump_event(&mut self, cx: &mut Cx, index: usize) {
        if let Some(e) = self.sessions.result.events.get(index) {
            self.seek_take(cx, e.time_us, Some(index));
        }
    }
    pub fn advance_replay(&mut self, cx: &mut Cx) {
        if !self.replay_visible() {
            self.pause_session();
            return;
        }
        let Some((anchor, start)) = self.sessions.playing else {
            return;
        };
        let duration = self.selected_take().unwrap().duration_us();
        let dt = (anchor.elapsed().as_secs_f64() * 1e6 * SPEEDS[self.sessions.speed_index]) as u64;
        self.sessions.position = start.saturating_add(dt).min(duration);
        if self.sessions.position >= duration {
            self.pause_session();
            self.refresh(cx);
        } else {
            self.paint_replay(cx);
            self.frame = cx.new_next_frame();
        }
    }
    pub fn paint_replay(&mut self, cx: &mut Cx) {
        let Some(take) = self.selected_take() else {
            return;
        };
        let position = self.sessions.position;
        let frame = take.at(position);
        let duration = take.duration_us();
        let kind = if take.example {
            "Example TCP"
        } else {
            "Recorded TCP"
        };
        let events = &self.sessions.result.events;
        let active = self
            .sessions
            .chosen_event
            .and_then(|i| events.get(i))
            .filter(|e| e.time_us <= position && position <= e.end_us)
            .or_else(|| {
                let first = self
                    .sessions
                    .result
                    .latest_event_end
                    .partition_point(|&end| end < position);
                let last = events.partition_point(|e| e.time_us <= position);
                events[first..last]
                    .iter()
                    .rev()
                    .find(|e| position <= e.end_us)
            });
        self.draw_pose(
            cx,
            frame.joints,
            Some(frame.tcp_mm),
            kind,
            &self.sessions.result.trail,
            active,
        );
        self.text(
            cx,
            ids!(timecode),
            &format!("{}   /   {}", stamp(position), stamp(duration)),
        );
        if let Some(mut track) = self
            .ui
            .widget(cx, ids!(session_track))
            .borrow_mut::<SessionTrack>()
        {
            track.update(cx, duration, position, events);
        }
    }
    pub fn init_sessions(&mut self, cx: &mut Cx) {
        self.sessions.speed_index = self.ui.drop_down(cx, ids!(speed)).selected_item().min(3);
        self.rebuild_take_choices(cx);
        self.rebuild_event_choices(cx);
    }
    pub fn session_controls(&self, cx: &mut Cx, link: &Link) {
        let ready = matches!(link, Link::Live(s) if s.mode==2 && matches!(s.state,0..=2) && s.error==0 && s.warning==0);
        let pending = self.sessions.capture_pending;
        let capturing = self.controller.capture_status();
        self.ui.button(cx, ids!(capture_take)).set_enabled(
            cx,
            capturing.is_some() || (!pending && ready && self.sessions.file_job.is_none()),
        );
        self.text(
            cx,
            ids!(capture_take),
            if pending {
                "Finish recording"
            } else {
                "Record hand-guiding"
            },
        );
        let hint = match capturing {
            Some((n, t)) => format!("Recording   {}   ·   {n} samples", stamp(t)),
            None if pending => "Finishing the recording…".into(),
            None if ready => "Manual Mode · ready to record measured joints and tool tip.".into(),
            _ => "Connect in Controller and enter Manual Mode in UFACTORY Studio to record.".into(),
        };
        self.text(cx, ids!(capture_hint), &hint);
        let takes = &self.model.workspace.recordings;
        self.ui.button(cx, ids!(example)).set_visible(
            cx,
            takes.is_empty() && !pending && matches!(link, Link::Disconnected),
        );
        self.text(
            cx,
            ids!(example),
            if self.model.workspace.zones.is_empty() && self.model.workspace.poses.is_empty() {
                "Open example study"
            } else {
                "Add example session"
            },
        );
        self.ui
            .view(cx, ids!(transport))
            .set_visible(cx, self.replay_visible());
        self.text(
            cx,
            ids!(replay_button),
            if self.session_playing() {
                "Pause"
            } else {
                "Play"
            },
        );
        self.ui.button(cx, ids!(replay_button)).set_enabled(
            cx,
            self.selected_take().is_some_and(|r| r.duration_us() > 0),
        );
        let detail = self
            .selected_take()
            .map(|r| format!("{} samples · {}", r.samples.len(), r.reason))
            .unwrap_or_else(|| "No sessions recorded.".into());
        self.text(cx, ids!(session_info), &detail);
        let n = self.sessions.result.events.len();
        let summary = if takes.is_empty() {
            "Record or open a session to review it.".into()
        } else if let Some(msg) = &self.sessions.analysis_error {
            msg.clone()
        } else if !self.sessions.checked {
            "Checking recorded movement…".into()
        } else if self.model.workspace.zones.is_empty() {
            "Add a zone to check clearance.".into()
        } else if n == 0 {
            "No clearance events in the defined zones.".into()
        } else {
            format!(
                "{n} clearance {} · select an orange span to inspect.",
                if n == 1 { "event" } else { "events" }
            )
        };
        self.text(cx, ids!(event_summary), &summary);
        for id in [ids!(event_controls), ids!(event_steps)] {
            self.ui
                .view(cx, id)
                .set_visible(cx, n > 0 && self.replay_visible());
        }
        let can_file = !pending && self.sessions.file_job.is_none();
        for id in [ids!(save), ids!(load)] {
            self.ui.button(cx, id).set_enabled(cx, can_file);
        }
        if pending {
            self.text(cx, ids!(link_badge), &hint);
        } else if self.replay_visible() {
            let s = if matches!(link, Link::Disconnected) {
                "offline"
            } else if matches!(link, Link::Live(_)) {
                "controller connected"
            } else {
                "controller unavailable"
            };
            self.text(
                cx,
                ids!(link_badge),
                &format!(
                    "{} · {s}",
                    if self.selected_take().unwrap().example {
                        "Example replay"
                    } else {
                        "Recorded replay"
                    }
                ),
            );
        }
    }

    pub fn handle_session_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.model.page != SESSION_PAGE {
            return false;
        }
        let uid = self.ui.widget(cx, ids!(session_track)).widget_uid();
        let mut touched = false;
        for action in actions.filter_widget_actions_cast::<TrackAction>(uid) {
            if self.replay_visible() {
                match action {
                    TrackAction::Seek(t) => self.seek_take(cx, t, None),
                    TrackAction::Event(i) => self.jump_event(cx, i),
                    TrackAction::None => continue,
                }
                touched = true;
            }
        }
        if touched {
            return true;
        }
        if self.ui.button(cx, ids!(capture_take)).clicked(actions) {
            if self.sessions.capture_pending {
                self.controller.stop_capture();
                self.collect_capture(cx);
            } else if self.sessions.file_job.is_none() {
                match self.controller.start_capture() {
                    Ok(()) => {
                        self.sessions.capture_pending = true;
                        self.pause_session();
                        self.study_changed();
                        self.arm_work_timer(cx);
                        self.model.notice = "".into();
                    }
                    Err(e) => self.model.notice = e,
                }
            }
        } else if self.ui.button(cx, ids!(example)).clicked(actions)
            && self.model.workspace.recordings.is_empty()
            && matches!(self.controller.status(), Link::Disconnected)
        {
            if self.model.workspace.zones.is_empty() && self.model.workspace.poses.is_empty() {
                self.model.workspace.zones.push(Recording::example_zone());
                self.zone_list(cx);
            }
            self.model
                .workspace
                .recordings
                .push(Arc::new(Recording::example()));
            self.study_changed();
            self.select_take(cx, Some(0));
            self.rebuild_take_choices(cx);
            self.model.notice = "Opened a synthetic hand-guiding example.".into();
        } else if let Some(i) = self.ui.drop_down(cx, ids!(take_list)).changed(actions) {
            if i < self.model.workspace.recordings.len() {
                self.select_take(cx, Some(i));
            }
        } else if !self.replay_visible() {
            return false;
        } else if self.ui.button(cx, ids!(replay_button)).clicked(actions) {
            if self.sessions.playing.take().is_none() {
                let duration = self.selected_take().unwrap().duration_us();
                if duration > 0 {
                    if self.sessions.position >= duration {
                        self.seek_take(cx, 0, None);
                    }
                    self.sessions.playing = Some((Instant::now(), self.sessions.position));
                    self.frame = cx.new_next_frame();
                }
            }
        } else if let Some(i) = self.ui.drop_down(cx, ids!(speed)).changed(actions) {
            if self.session_playing() {
                self.advance_replay(cx);
            }
            self.sessions.speed_index = i.min(3);
            if self.session_playing() {
                self.sessions.playing = Some((Instant::now(), self.sessions.position));
            }
        } else if let Some(i) = self.ui.drop_down(cx, ids!(event_list)).changed(actions) {
            if let Some(index) = i.checked_sub(1) {
                self.jump_event(cx, index);
            }
        } else if self.ui.button(cx, ids!(previous_event)).clicked(actions)
            || self.ui.button(cx, ids!(next_event)).clicked(actions)
        {
            let events = &self.sessions.result.events;
            if !events.is_empty() {
                let next = self.ui.button(cx, ids!(next_event)).clicked(actions);
                let n = events.len();
                let i = match (self.sessions.chosen_event, next) {
                    (Some(i), true) => (i + 1) % n,
                    (Some(i), false) => (i + n - 1) % n,
                    (None, true) => {
                        events.partition_point(|e| e.time_us < self.sessions.position) % n
                    }
                    (None, false) => {
                        (events.partition_point(|e| e.time_us <= self.sessions.position) + n - 1)
                            % n
                    }
                };
                self.jump_event(cx, i);
            }
        } else {
            return false;
        }
        true
    }
}
