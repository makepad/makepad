pub use makepad_widgets;
use makepad_widgets::*;
use std::{net::IpAddr, path::PathBuf, time::Instant};

mod controller;
mod kinematics;
mod recording;
mod scene;
mod session_track;
mod sessions;
mod ui;
mod workspace;
use controller::{Controller, Link, Sample};
use workspace::{Point, Pose, Workspace, Zone, STUDIO_POSE};

app_main!(App);

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    model: Study,
    #[rust]
    controller: Controller,
    #[rust]
    sessions: sessions::Sessions,
    #[rust]
    poll: Option<Timer>,
    #[rust]
    frame: NextFrame,
    #[rust]
    live_tool: Option<Pose>,
}

struct Study {
    workspace: Workspace,
    pose: Pose,
    page: usize,
    enclosure: bool,
    zone: usize,
    cursor: f64,
    trail: Vec<Point>,
    play: Option<Instant>,
    notice: String,
    // Last displayed live measurement is retained and labelled when reports stop.
    measured: bool,
}
impl Default for Study {
    fn default() -> Self {
        Self {
            workspace: Workspace::default(),
            pose: STUDIO_POSE,
            page: 0,
            enclosure: false,
            zone: 0,
            cursor: 0.,
            trail: Vec::new(),
            play: None,
            notice: "".into(),
            measured: false,
        }
    }
}

fn point(p: Point) -> String {
    format!("{:.1}, {:.1}, {:.1}", p[0], p[1], p[2])
}
fn study_path() -> Result<PathBuf, String> {
    makepad_widgets::makepad_platform::home::app_data_dir("robot")
        .map(|d| d.join("workspace.study"))
        .ok_or_else(|| "No application data directory is available.".into())
}
fn teach_ready(sample: &Sample) -> bool {
    sample.mode == 2 && matches!(sample.state, 0 | 2) && sample.error == 0 && sample.warning == 0
}
fn joint_paths() -> [&'static [LiveId]; 6] {
    [
        ids!(joint1),
        ids!(joint2),
        ids!(joint3),
        ids!(joint4),
        ids!(joint5),
        ids!(joint6),
    ]
}

impl App {
    fn text(&self, cx: &mut Cx, id: &[LiveId], text: &str) {
        let w = self.ui.widget(cx, id);
        if w.text() != text {
            w.set_text(cx, text);
        }
    }
    fn corners_clear(&self, cx: &mut Cx) {
        self.text(cx, ids!(corner_a), "");
        self.text(cx, ids!(corner_b), "");
    }
    fn refresh(&mut self, cx: &mut Cx) {
        let link = self.controller.status();
        let offline = matches!(link, Link::Disconnected);
        let live = match &link {
            Link::Live(s) => Some(s),
            _ => None,
        };
        if self.live_tool != live.map(|s| s.tcp_offset) {
            self.corners_clear(cx);
            self.live_tool = live.map(|s| s.tcp_offset);
        }
        if let Some(s) = live {
            self.model.pose = s.joints;
            self.model.measured = true;
        }
        let q = self.model.pose;
        let tcp = live.map(|s| s.tcp_mm);
        let msg = match &link {
            Link::Disconnected => "Offline preview",
            Link::Connecting if self.model.measured => "Waiting for telemetry · last pose frozen",
            Link::Connecting => "Connecting · preview only",
            Link::Live(_) => "Live · measured pose",
            Link::Fault(_) if self.model.measured => "Link lost · last pose frozen",
            Link::Fault(_) => "Link unavailable · preview only",
        };
        self.text(cx, ids!(link_badge), msg);
        self.text(
            cx,
            ids!(connect),
            if offline {
                "Connect over Ethernet"
            } else {
                "Disconnect"
            },
        );
        let detail = match &link {
            Link::Live(s) => format!(
                "{} · state {} · error {} · warning {}{}",
                if s.mode == 2 {
                    "Manual mode".into()
                } else {
                    format!("Mode {}", s.mode)
                },
                s.state,
                s.error,
                s.warning,
                if s.notice.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", s.notice)
                }
            ),
            Link::Fault(e) => e.to_owned(),
            _ => msg.into(),
        };
        self.text(cx, ids!(details), &detail);
        let limits = live
            .and_then(|s| s.slow)
            .map(|l| {
                format!(
                    "Reduced mode {} · {:.1} mm/s · {:.2}°/s",
                    if l.enabled { "on" } else { "off" },
                    l.tcp_mm_s,
                    l.joint_deg_s
                )
            })
            .unwrap_or_else(|| "Reduced limits unavailable.".into());
        self.text(
            cx,
            ids!(limits),
            &format!(
                "{limits}\nApply caps of 10 mm/s and 2°/s. Lower existing limits are retained."
            ),
        );
        self.ui.button(cx, ids!(slow)).set_enabled(
            cx,
            live.is_some_and(|s| s.warning == 0 && s.error == 0 && matches!(s.state, 2 | 4)),
        );
        self.ui
            .button(cx, ids!(stop))
            .set_enabled(cx, live.is_some());
        let ready = live.is_some_and(teach_ready);
        for id in [ids!(capture_a), ids!(capture_b)] {
            self.ui.button(cx, id).set_enabled(cx, ready);
        }
        self.ui
            .button(cx, ids!(record_pose))
            .set_enabled(cx, offline || ready);
        self.ui
            .button(cx, ids!(remove_pose))
            .set_enabled(cx, offline && !self.model.workspace.poses.is_empty());
        self.ui
            .button(cx, ids!(preview_path))
            .set_enabled(cx, offline && self.model.workspace.poses.len() > 1);
        self.ui
            .button(cx, ids!(remove_zone))
            .set_enabled(cx, !self.model.workspace.zones.is_empty());
        for (i, id) in [
            ids!(pose_panel),
            ids!(path_panel),
            ids!(sessions_panel),
            ids!(zones_panel),
            ids!(control_panel),
        ]
        .into_iter()
        .enumerate()
        {
            self.ui.view(cx, id).set_visible(cx, i == self.model.page);
        }
        self.ui.view(cx, ids!(sliders)).set_visible(cx, offline);
        self.ui.widget(cx, ids!(readout)).set_visible(cx, !offline);
        self.ui
            .widget(cx, ids!(scrub))
            .set_visible(cx, offline && self.model.workspace.poses.len() > 1);
        self.text(cx,ids!(pose_hint),if offline {"Drag a joint to pose the preview."} else {"Hand guiding requires the correct mounting and payload in Studio. Capture poses in Manual Mode."});
        if offline {
            for (id, v) in joint_paths().into_iter().zip(q) {
                let slider = self.ui.slider(cx, id);
                if slider
                    .value()
                    .is_none_or(|a| (a - v.to_degrees()).abs() > 0.01)
                {
                    slider.set_value(cx, v.to_degrees());
                }
            }
        } else {
            let values = q
                .iter()
                .enumerate()
                .map(|(i, v)| format!("J{}   {:>6.1}°", i + 1, v.to_degrees()))
                .collect::<Vec<_>>()
                .join("\n");
            self.text(cx, ids!(readout), &values);
        }
        let part = if tcp.is_some() {
            "Measured TCP"
        } else if !offline && self.model.measured {
            "Last measured joints · nominal flange"
        } else {
            "Preview flange"
        };
        self.session_controls(cx, &link);
        if self.replay_visible() {
            self.paint_replay(cx);
        } else {
            self.draw_pose(
                cx,
                q,
                tcp,
                part,
                if self.model.page == 1 {
                    &self.model.trail
                } else {
                    &[]
                },
                None,
            );
        }
        let n = self.model.workspace.poses.len();
        self.text(
            cx,
            ids!(sequence),
            &match n {
                0 => "No poses recorded.".into(),
                1 => "1 pose recorded.".into(),
                _ => format!("{n} poses · preview path"),
            },
        );
        self.text(
            cx,
            ids!(preview_path),
            if self.model.play.is_some() {
                "Pause preview"
            } else {
                "Play preview"
            },
        );
        self.text(cx, ids!(notice), &self.model.notice);
    }
    fn draw_pose(
        &self,
        cx: &mut Cx,
        q: Pose,
        tcp: Option<Point>,
        part: &str,
        trail: &[Point],
        event: Option<&recording::ClearanceEvent>,
    ) {
        let xyz = tcp.unwrap_or_else(|| kinematics::joint_origins(q)[6]);
        self.text(
            cx,
            ids!(point_label),
            &format!(
                "{part}     X   {:.1}      Y   {:.1}      Z   {:.1}   mm",
                xyz[0], xyz[1], xyz[2]
            ),
        );
        let conflict = self.model.workspace.conflict(q, 0.);
        let summary = event
            .map(|e| e.message.as_str())
            .or_else(|| conflict.as_ref().map(|v| v.message.as_str()))
            .unwrap_or(if self.model.workspace.has_enclosure() {
                "Bare arm within preview zones · 12 mm margin"
            } else {
                "No enclosure defined · bare-arm preview"
            });
        self.text(cx, ids!(clearance), summary);
        let zones: Vec<_> = self
            .model
            .workspace
            .zones
            .iter()
            .map(|z| (z.enclosure, z.bounds.0, z.bounds.1))
            .collect();
        if let Some(mut scene) = self
            .ui
            .widget(cx, ids!(scene))
            .borrow_mut::<scene::RobotScene>()
        {
            scene.set_scene(
                cx,
                q,
                &zones,
                event
                    .and_then(|e| e.link)
                    .or_else(|| conflict.as_ref().and_then(|v| v.link)),
            );
            scene.set_tool_tip(cx, tcp);
            scene.set_trail(cx, trail);
        }
    }
    fn zone_list(&mut self, cx: &mut Cx) {
        let mut labels = self.model.workspace.zone_names();
        if labels.is_empty() {
            labels.push("No zones".into());
        }
        let id = self.model.zone.min(labels.len() - 1);
        self.model.zone = id;
        let list = self.ui.drop_down(cx, ids!(zone_list));
        list.set_labels(cx, labels);
        list.set_selected_item(cx, id);
        let desc = self
            .model
            .workspace
            .zones
            .get(id)
            .map(|z| format!("A   {}\nB   {}", point(z.bounds.0), point(z.bounds.1)))
            .unwrap_or_default();
        self.text(cx, ids!(zone_coords), &desc);
    }
    fn seek(&mut self, cx: &mut Cx, cursor: f64) {
        let poses = &self.model.workspace.poses;
        if poses.len() < 2 {
            return;
        }
        self.model.cursor = cursor.clamp(0., (poses.len() - 1) as f64);
        let first = (self.model.cursor.floor() as usize).min(poses.len() - 2);
        self.model.pose = workspace::interpolate(
            poses[first],
            poses[first + 1],
            self.model.cursor - first as f64,
        );
        self.ui
            .slider(cx, ids!(scrub))
            .set_value(cx, self.model.cursor * 100. / (poses.len() - 1) as f64);
        self.ui.drop_down(cx, ids!(preset)).set_selected_item(cx, 3);
    }
    fn tick(&mut self, cx: &mut Cx) {
        let Some(last) = self.model.play else { return };
        let n = self.model.workspace.poses.len();
        if n < 2 || !matches!(self.controller.status(), Link::Disconnected) {
            self.model.play = None;
            return;
        }
        let i = (self.model.cursor.floor() as usize).min(n - 2);
        let pair = &self.model.workspace.poses[i..=i + 1];
        let largest = pair[0]
            .iter()
            .zip(pair[1])
            .map(|(&a, b)| (a - b).abs().to_degrees())
            .fold(0., f64::max);
        let seconds = (largest / 2.).max(0.4);
        let now = Instant::now();
        self.seek(
            cx,
            (self.model.cursor + (now - last).as_secs_f64() / seconds).min((i + 1) as f64),
        );
        if self.model.cursor >= (n - 1) as f64 {
            self.model.play = None;
        } else {
            self.model.play = Some(now);
            self.frame = cx.new_next_frame();
        }
        self.refresh(cx);
    }
}
impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Handle only actual app actions. There is no background render loop for
        // an idle offline viewport.
        let offline = matches!(self.controller.status(), Link::Disconnected);
        macro_rules! click {
            ($id:ident) => {
                self.ui.button(cx, ids!($id)).clicked(actions)
            };
        }
        if self.handle_session_actions(cx, actions) {
            self.refresh(cx);
            return;
        }
        if let Some(v) = self
            .ui
            .segmented_control(cx, ids!(camera_tabs))
            .selected(actions)
        {
            if let Some(mut scene) = self
                .ui
                .widget(cx, ids!(scene))
                .borrow_mut::<scene::RobotScene>()
            {
                scene.set_camera(cx, v);
            }
        } else if let Some(v) = self.ui.segmented_control(cx, ids!(tabs)).selected(actions) {
            self.pause_session();
            if v == sessions::SESSION_PAGE {
                self.model.play = None;
            }
            self.model.page = v;
        } else if let Some(v) = self
            .ui
            .segmented_control(cx, ids!(zone_kind))
            .selected(actions)
        {
            self.model.enclosure = v == 1;
        } else if let Some(v) = self.ui.drop_down(cx, ids!(zone_list)).changed(actions) {
            self.model.zone = v;
            self.zone_list(cx);
        } else if let Some(v) = self
            .ui
            .drop_down(cx, ids!(preset))
            .changed(actions)
            .filter(|_| offline)
        {
            self.model.play = None;
            self.model.pose = match v {
                0 => STUDIO_POSE,
                1 => [0.1, 0.8, -1.82, 0.2, 0.55, 0.0],
                2 => [0.0, -0.25, -3.30, 0.15, 0.7, 0.3],
                _ => self.model.pose,
            };
        } else if click!(connect) {
            self.model.play = None;
            self.pause_session();
            if !offline {
                self.controller.disconnect();
                self.collect_capture(cx);
                if let Some(timer) = self.poll.take() {
                    cx.stop_timer(timer);
                }
                self.corners_clear(cx);
                self.model.measured = false;
                self.ui.drop_down(cx, ids!(preset)).set_selected_item(cx, 3);
                self.model.notice = "Disconnected. Joint editing affects the preview.".into();
            } else {
                let host = self.ui.widget(cx, ids!(host)).text();
                match host.trim().parse::<IpAddr>() {
                    Ok(ip) if !ip.is_unspecified() && !ip.is_multicast() => {
                        self.controller.connect(&cx.thread_spawner(), ip);
                        self.poll = Some(cx.start_interval(0.1));
                        self.corners_clear(cx);
                        self.model.notice = "Reading controller reports…".into();
                    }
                    _ => self.model.notice = "Enter the control box’s IP address.".into(),
                }
            }
        } else if click!(slow) {
            self.model.notice = match self.controller.status() {
                Link::Live(s) if matches!(s.state, 2 | 4) => self
                    .controller
                    .apply_slow_limits()
                    .map(|_| "Waiting for limit readback.".into())
                    .unwrap_or_else(|e| e),
                _ => "Stop the arm before applying limits.".into(),
            };
        } else if click!(stop) {
            self.model.notice = self
                .controller
                .request_stop()
                .map(|_| "Stop requested over Ethernet.".into())
                .unwrap_or_else(|e| e);
        } else if click!(capture_a) || click!(capture_b) {
            match self.controller.status() {
                Link::Live(s) if teach_ready(&s) && self.live_tool == Some(s.tcp_offset) => {
                    let id = if click!(capture_a) {
                        ids!(corner_a)
                    } else {
                        ids!(corner_b)
                    };
                    self.text(cx, id, &point(s.tcp_mm));
                    self.model.notice =
                        "Captured the measured tool tip in base coordinates.".into();
                }
                _ => {
                    self.model.notice =
                        "Capture needs fresh telemetry and local Manual Mode.".into()
                }
            }
        } else if click!(add_zone) {
            let a = self.ui.widget(cx, ids!(corner_a)).text();
            let b = self.ui.widget(cx, ids!(corner_b)).text();
            let result = (|| {
                let zone = Zone::from_corners(
                    self.model.enclosure,
                    workspace::parse_numbers(&a)?,
                    workspace::parse_numbers(&b)?,
                )?;
                self.model.workspace.add_zone(zone)?;
                self.zones_changed(cx);
                self.model.zone = self.model.workspace.zones.len() - 1;
                self.zone_list(cx);
                self.corners_clear(cx);
                Ok::<_, String>("Zone added to the study.".into())
            })();
            self.model.play = None;
            self.model.notice = result.unwrap_or_else(|e| e);
        } else if click!(remove_zone) {
            self.model.play = None;
            if self.model.zone < self.model.workspace.zones.len() {
                self.model.workspace.zones.remove(self.model.zone);
                self.zones_changed(cx);
                self.zone_list(cx);
                self.model.notice = "Zone removed from the study.".into();
            }
        } else if click!(record_pose) {
            self.model.play = None;
            let q = match self.controller.status() {
                Link::Disconnected => Some(self.model.pose),
                Link::Live(s) if teach_ready(&s) => Some(s.joints),
                _ => None,
            };
            if let Some(q) = q.filter(|q| workspace::valid_pose(*q)) {
                self.model.workspace.poses.push(q);
                self.study_changed();
                self.model.trail = self.model.workspace.flange_trail();
                self.model.notice = format!("Recorded pose {}.", self.model.workspace.poses.len());
                self.model.cursor = (self.model.workspace.poses.len() - 1) as f64;
                self.ui
                    .slider(cx, ids!(scrub))
                    .set_value(cx, if self.model.cursor > 0. { 100. } else { 0. });
            } else {
                self.model.notice =
                    "Capture needs a valid pose and fresh Manual Mode telemetry.".into();
            }
        } else if click!(remove_pose) && offline {
            self.model.play = None;
            self.model.workspace.poses.pop();
            self.study_changed();
            self.model.trail = self.model.workspace.flange_trail();
            self.model.cursor = self.model.workspace.poses.len().saturating_sub(1) as f64;
            if let Some(last) = self.model.workspace.poses.last() {
                self.model.pose = *last;
            }
            self.ui
                .slider(cx, ids!(scrub))
                .set_value(cx, if self.model.cursor > 0. { 100. } else { 0. });
            self.model.notice = "Last pose removed.".into();
        } else if click!(preview_path) && offline {
            if self.model.play.take().is_none() && self.model.workspace.poses.len() >= 2 {
                match self.model.workspace.review() {
                    Ok(()) => {
                        if self.model.cursor >= (self.model.workspace.poses.len() - 1) as f64 {
                            self.model.cursor = 0.;
                        }
                        self.seek(cx, self.model.cursor);
                        self.model.play = Some(Instant::now());
                        self.frame = cx.new_next_frame();
                        self.model.notice = "Playing locally · 2°/s maximum per joint.".into();
                    }
                    Err(e) => self.model.notice = format!("{e}. Scrub to inspect."),
                }
            }
        } else if let Some(v) = self
            .ui
            .slider(cx, ids!(scrub))
            .slided(actions)
            .filter(|_| offline)
        {
            self.model.play = None;
            self.seek(
                cx,
                v * 0.01 * self.model.workspace.poses.len().saturating_sub(1) as f64,
            );
        } else if click!(save) || click!(load) {
            self.study_file(cx, click!(save));
        } else {
            let Some((i, v)) = joint_paths()
                .into_iter()
                .enumerate()
                .find_map(|(i, p)| self.ui.slider(cx, p).slided(actions).map(|v| (i, v)))
                .filter(|_| offline)
            else {
                return;
            };
            if !v.is_finite() {
                return;
            }
            self.model.play = None;
            let (lo, hi) = kinematics::JOINT_LIMITS[i];
            self.model.pose[i] = v.to_radians().clamp(lo, hi);
            self.ui.drop_down(cx, ids!(preset)).set_selected_item(cx, 3);
        }
        self.refresh(cx);
    }
}
impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        scene::script_mod(vm);
        session_track::script_mod(vm);
        ui::script_mod(vm)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::Startup | Event::LiveEdit) {
            self.zone_list(cx);
            self.init_sessions(cx);
            self.refresh(cx);
        }
        if self.session_timer(event) {
            self.poll_session_work(cx);
        }
        if self
            .poll
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
        {
            self.refresh(cx);
        }
        if self.frame.is_event(event).is_some() {
            if self.session_playing() {
                self.advance_replay(cx);
            } else if self.model.play.is_some() {
                self.tick(cx);
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
