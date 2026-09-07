// One replay clock drives every proposal, app preview and F10 explanation.
impl App {
    fn set_demo_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.iterations.visible = false;
        self.sync_flow_terminal_ownership(cx);
        self.ui.view(cx, ids!(flows_area)).set_visible(cx, false);
        self.ui.view(cx, ids!(caption_flow_tools)).set_visible(cx, false);
        self.ui.radio_button(cx, ids!(flows_tab)).set_active(cx, false, Animate::No);
        self.close_utility(cx);
        self.demo_visible = visible;
        self.ui.view(cx, ids!(work_area)).set_visible(cx, !visible);
        self.ui
            .view(cx, ids!(activity_lab))
            .set_visible(cx, visible);
        if !visible {
            self.demo_playing = false;
        }
        self.refresh_demo(cx);
        self.refresh_ai_context(cx);
    }

    fn demo_json(&self) -> Value {
        let snapshot = activity_demo::snapshot(self.demo_at);
        let selected_detail = self.demo_selected.as_deref().and_then(|id| {
            snapshot.agents.iter().find(|a| a.id == id).map(|a| format!("{}\n{}\n{}", a.task, a.terminal.join("\n"), a.diff.join("\n")))
                .or_else(|| snapshot.events.iter().find(|e| e.id == id).map(|e| format!("{}\n{}", e.title, e.detail)))
                .or_else(|| snapshot.apps.iter().find(|a| a.id == id).map(|a| a.caption.to_owned()))
        }).unwrap_or_else(|| "Synthetic sample. Select an agent, event or app for its detail. Every event listed occurred by this playhead.".into());
        json::obj(vec![
            ("synthetic", Value::Bool(true)),
            ("visible", Value::Bool(self.demo_visible)),
            ("layout", json::s(self.demo_layout.as_str())),
            ("at", Value::F64(self.demo_at)),
            ("duration", Value::F64(activity_demo::DURATION)),
            ("playing", Value::Bool(self.demo_playing)),
            ("speed", Value::F64(self.demo_speed)),
            (
                "selected",
                self.demo_selected
                    .as_ref()
                    .map(json::s)
                    .unwrap_or(Value::Null),
            ),
            (
                "recording",
                Value::Bool(self.demo_recording || self.demo_record_starting),
            ),
            ("finalizing", Value::Bool(self.demo_finalizing)),
            (
                "mp4",
                self.demo_record_path
                    .as_ref()
                    .map(|p| json::s(p.display().to_string()))
                    .unwrap_or(Value::Null),
            ),
            (
                "record_error",
                self.demo_record_error
                    .as_ref()
                    .map(json::s)
                    .unwrap_or(Value::Null),
            ),
            (
                "agents",
                Value::Arr(
                    snapshot
                        .agents
                        .iter()
                        .map(|a| {
                            json::obj(vec![
                                ("id", json::s(a.id)),
                                ("parent", a.parent.map(json::s).unwrap_or(Value::Null)),
                                ("name", json::s(a.name)),
                                ("state", json::s(format!("{:?}", a.state))),
                                ("task", json::s(a.task)),
                                ("file", a.file.map(json::s).unwrap_or(Value::Null)),
                                (
                                    "terminal_tail",
                                    json::s(
                                        a.terminal
                                            .iter()
                                            .rev()
                                            .take(2)
                                            .copied()
                                            .collect::<Vec<_>>()
                                            .join("\n"),
                                    ),
                                ),
                                ("diff", json::s(a.diff.join("\n"))),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "events",
                Value::Arr(
                    snapshot
                        .events
                        .iter()
                        .map(|e| {
                            json::obj(vec![
                                ("id", json::s(e.id)),
                                ("at", Value::F64(e.at)),
                                ("agent", json::s(e.agent)),
                                ("kind", json::s(format!("{:?}", e.kind))),
                                ("title", json::s(e.title)),
                                ("detail", json::s(e.detail)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "apps",
                Value::Arr(
                    snapshot
                        .apps
                        .iter()
                        .map(|a| {
                            json::obj(vec![
                                ("id", json::s(a.id)),
                                ("owner", json::s(a.owner)),
                                ("title", json::s(a.title)),
                                ("stage", json::s(format!("{:?}", a.stage))),
                                ("frame", Value::Int(a.frame as i64)),
                                ("caption", json::s(a.caption)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("description", json::s(selected_detail)),
        ])
    }

    fn refresh_demo(&self, cx: &mut Cx) {
        self.ui
            .radio_button(cx, ids!(demo_back))
            .set_active(cx, !self.demo_visible, Animate::No);
        self.ui.radio_button(cx, ids!(activity_lab_tab)).set_active(
            cx,
            self.demo_visible,
            Animate::No,
        );
        self.ui
            .view(cx, ids!(demo_view_controls))
            .set_visible(cx, self.demo_visible);
        self.ui
            .widget(cx, ids!(demo_scene_wrap))
            .set_visible(cx, !self.dashboard_visible);
        self.ui
            .view(cx, ids!(dashboard_panel))
            .set_visible(cx, self.dashboard_visible);
        self.ui
            .view(cx, ids!(demo_replay_controls))
            .set_visible(cx, !(self.dashboard_visible && self.dashboard_live));
        self.ui.radio_button(cx, ids!(dashboard_view)).set_active(
            cx,
            self.dashboard_visible,
            Animate::No,
        );
        self.ui.label(cx, ids!(demo_title)).set_text(
            cx,
            if self.dashboard_visible && self.dashboard_live {
                "Live project activity"
            } else {
                "Orbit Shop / checkout recovery"
            },
        );
        self.refresh_dashboard(cx);
        if let Some(mut scene) = self
            .ui
            .widget(cx, ids!(demo_scene))
            .borrow_mut::<StudioActivityViews>()
        {
            scene.set_view(cx, self.demo_layout, self.demo_at);
        }
        for (id, layout) in [
            (id!(demo_lanes), DemoLayout::AgentLanes),
            (id!(demo_timeline), DemoLayout::Timeline),
            (id!(demo_system), DemoLayout::SystemLanes),
        ] {
            self.ui.radio_button(cx, &[id]).set_active(
                cx,
                !self.dashboard_visible && layout == self.demo_layout,
                Animate::No,
            );
        }
        self.ui
            .slider(cx, ids!(demo_time))
            .set_value(cx, self.demo_at);
        let at = self.demo_at as u64;
        self.ui
            .label(cx, ids!(demo_clock))
            .set_text(cx, &format!("{:02}:{:02} / 03:00", at / 60, at % 60));
        self.ui.button(cx, ids!(demo_play)).set_text(
            cx,
            if self.demo_playing {
                "Pause"
            } else if self.demo_at >= activity_demo::DURATION {
                "Play again"
            } else {
                "Play"
            },
        );
        self.ui.button(cx, ids!(demo_record)).set_text(
            cx,
            if self.demo_recording || self.demo_record_starting {
                "Stop MP4"
            } else if self.demo_finalizing {
                "Finalizing…"
            } else {
                "Record MP4"
            },
        );
        self.ui
            .button(cx, ids!(demo_record))
            .set_disabled(cx, self.demo_finalizing);
        self.ui.button(cx, ids!(demo_watch)).set_disabled(
            cx,
            self.demo_record_path.is_none()
                || self.demo_recording
                || self.demo_record_starting
                || self.demo_finalizing,
        );
        let snapshot = activity_demo::snapshot(self.demo_at);
        self.ui.label(cx, ids!(demo_summary)).set_text(
            cx,
            &if self.dashboard_visible && self.dashboard_live {
                "Observed terminal output · processes · file changes · F10 briefing".to_owned()
            } else {
                format!(
                    "Sample replay · {} agents · {} app frames · F10 explains this time",
                    snapshot.agents.len(),
                    snapshot.apps.len()
                )
            },
        );
        let selected = self.demo_selected.as_deref();
        let detail = snapshot.agents.iter().find(|a| Some(a.id) == selected).map(|a| format!("{} · {:?} · {}\n{}", a.name, a.state, a.task, a.terminal.last().copied().unwrap_or("")))
            .or_else(|| snapshot.events.iter().find(|e| Some(e.id) == selected).map(|e| format!("{:02}:{:02} · {} · {}\n{}", e.at as u64 / 60, e.at as u64 % 60, e.agent, e.title, e.detail)))
            .or_else(|| snapshot.apps.iter().find(|a| Some(a.id) == selected).map(|a| format!("{} · owner {} · frame {}\n{}", a.title, a.owner, a.frame, a.caption)))
            .unwrap_or_else(|| match self.demo_layout {
                DemoLayout::AgentLanes => "Agent lanes · stable parent/subagent rows; work, edits and app evidence stay beside their owner. Select anything, then ask F10.".into(),
                DemoLayout::Timeline => "Timeline · Fable’s recommendation: agent rows, doing-now summaries and failure → fix → retest over one clock. Select a span, then ask F10.".into(),
                DemoLayout::SystemLanes => "System lanes · checkout, API and verification keep a fixed place; agent ownership follows each piece of work. Select anything, then ask F10.".into(),
            });
        self.ui
            .label(cx, ids!(demo_selection))
            .set_text(cx, if self.dashboard_visible { "Dashboard · scrub to inspect earlier evidence · Brief me asks F10 for an overview at this time" } else { &detail });
        let note = if let Some(error) = &self.demo_record_error {
            format!("Recording unavailable: {error}")
        } else if self.demo_recording {
            "Recording this Studio view, including visible sample app UIs · MP4 · maximum 30 seconds".into()
        } else if self.demo_finalizing {
            "Finalizing MP4…".into()
        } else if let Some(path) = &self.demo_record_path {
            format!("MP4 ready · {} · Play recording", path.display())
        } else {
            "Synthetic session and app frames · Record MP4 saves a 12x replay of this view · 30-second clip limit".into()
        };
        self.ui
            .label(cx, ids!(demo_record_note))
            .set_text(cx, &note);
    }

    fn ensure_demo_timer(&mut self, cx: &mut Cx) {
        if self.demo_timer.is_none() {
            self.demo_last_tick = cx.seconds_since_app_start();
            self.demo_timer = Some(cx.start_interval(1.0 / 30.0));
        }
    }

    fn seek_demo(&mut self, cx: &mut Cx, at: f64) {
        self.demo_at = at.clamp(0.0, activity_demo::DURATION);
        self.demo_playing = false;
        self.refresh_demo(cx);
        self.refresh_ai_context(cx);
    }

    fn play_demo(&mut self, cx: &mut Cx, playing: bool) {
        if playing && self.demo_at >= activity_demo::DURATION {
            self.demo_at = 0.0;
        }
        self.demo_playing = playing;
        self.demo_last_tick = cx.seconds_since_app_start();
        if playing {
            self.ensure_demo_timer(cx);
        }
        self.refresh_demo(cx);
    }

    fn tick_demo(&mut self, cx: &mut Cx, event: &Event) {
        if !self
            .demo_timer
            .as_ref()
            .is_some_and(|timer| timer.is_event(event).is_some())
        {
            return;
        }
        let now = cx.seconds_since_app_start();
        let delta = (now - self.demo_last_tick).clamp(0.0, 0.25);
        self.demo_last_tick = now;
        let old_second = self.demo_at as u64;
        if self.demo_playing {
            self.demo_at = (self.demo_at + delta * self.demo_speed).min(activity_demo::DURATION);
            if self.demo_at >= activity_demo::DURATION {
                self.demo_playing = false;
                self.demo_record_end = Some(now + 0.6);
            }
            self.refresh_demo(cx);
        }
        if self.demo_recording
            && (now - self.demo_record_started >= 30.0
                || self.demo_record_end.is_some_and(|end| now >= end))
        {
            self.stop_demo_recording(cx);
        }
        if old_second != self.demo_at as u64 {
            self.refresh_ai_context(cx);
        }
        if !self.demo_playing && !self.demo_recording && !self.demo_record_starting {
            if let Some(timer) = self.demo_timer.take() {
                cx.stop_timer(timer);
            }
        }
    }

    fn toggle_demo_recording(&mut self, cx: &mut Cx) {
        if self.demo_finalizing {
            return;
        }
        if self.demo_recording || self.demo_record_starting {
            self.stop_demo_recording(cx);
            return;
        }
        self.close_utility(cx);
        self.demo_record_error = None;
        self.demo_record_path = None;
        self.demo_record_starting = true;
        self.demo_record_stop_pending = false;
        self.demo_record_end = None;
        self.demo_at = 0.0;
        self.demo_speed = 12.0;
        self.ui
            .drop_down(cx, ids!(demo_speed))
            .set_selected_item(cx, 2);
        if let Some(mut window) = self.ui.window(cx, ids!(main_window)).borrow_mut() {
            window.toggle_recording(cx);
        }
        self.play_demo(cx, true);
    }

    fn stop_demo_recording(&mut self, cx: &mut Cx) {
        if self.demo_record_starting {
            self.demo_record_stop_pending = true;
        }
        if self.demo_recording {
            self.demo_recording = false;
            self.demo_finalizing = true;
            if let Some(mut window) = self.ui.window(cx, ids!(main_window)).borrow_mut() {
                window.toggle_recording(cx);
            }
            self.refresh_demo(cx);
        }
    }

    fn track_demo_recording(&mut self, cx: &mut Cx, actions: &Actions) {
        use makepad_widgets::screen_cap::ScreenCapAction;
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            match action.cast::<ScreenCapAction>() {
                ScreenCapAction::Started(_) => {
                    self.demo_record_starting = false;
                    self.demo_recording = true;
                    self.demo_finalizing = false;
                    self.demo_record_started = cx.seconds_since_app_start();
                    self.ensure_demo_timer(cx);
                    if self.demo_record_stop_pending {
                        self.demo_record_stop_pending = false;
                        self.stop_demo_recording(cx);
                    }
                }
                ScreenCapAction::Finished(path) => {
                    self.demo_recording = false;
                    self.demo_record_starting = false;
                    self.demo_record_stop_pending = false;
                    self.demo_finalizing = false;
                    self.demo_record_path = Some(path);
                }
                ScreenCapAction::Failed(error) => {
                    self.demo_recording = false;
                    self.demo_record_starting = false;
                    self.demo_record_stop_pending = false;
                    self.demo_finalizing = false;
                    self.demo_record_error = Some(error);
                }
                ScreenCapAction::None => continue,
            }
            self.refresh_demo(cx);
        }
    }

    fn play_demo_recording(&mut self, cx: &mut Cx) {
        let Some(path) = self.demo_record_path.clone() else {
            return;
        };
        self.demo_playing = false;
        self.show_utility(
            cx,
            id!(recording_tab),
            id!(RecordingTab),
            "Recorded Studio view · MP4",
        );
        self.demo_video_visible = true;
        self.demo_video_pending = Some(path);
        self.ui
            .video(cx, ids!(recording_player))
            .stop_and_cleanup_resources(cx);
        self.resume_demo_video(cx);
        self.refresh_demo(cx);
    }

    fn close_demo_video(&mut self, cx: &mut Cx) {
        self.demo_video_pending = None;
        self.demo_video_visible = false;
        self.ui
            .video(cx, ids!(recording_player))
            .stop_and_cleanup_resources(cx);
    }

    fn resume_demo_video(&mut self, cx: &mut Cx) {
        if self.demo_video_pending.is_none() {
            return;
        }
        let video = self.ui.video(cx, ids!(recording_player));
        if video.is_unprepared() {
            if let Some(path) = self.demo_video_pending.take() {
                video.set_source(makepad_widgets::video::VideoDataSource::Filesystem {
                    path: path.display().to_string(),
                });
                video.begin_playback(cx);
            }
        }
    }

    fn handle_demo_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        self.handle_dashboard_actions(cx, actions);
        if self.ui.radio_button(cx, ids!(demo_back)).clicked(actions) {
            self.set_demo_visible(cx, false);
        }
        if self
            .ui
            .radio_button(cx, ids!(activity_lab_tab))
            .clicked(actions)
        {
            self.ui_action(cx, Action::OpenDemo);
        }
        for (id, layout) in [
            (id!(demo_lanes), DemoLayout::AgentLanes),
            (id!(demo_timeline), DemoLayout::Timeline),
            (id!(demo_system), DemoLayout::SystemLanes),
        ] {
            if self.ui.radio_button(cx, &[id]).clicked(actions) {
                self.dashboard_visible = false;
                self.demo_layout = layout;
                self.refresh_demo(cx);
                self.refresh_ai_context(cx);
            }
        }
        if self.ui.button(cx, ids!(demo_fit)).clicked(actions) {
            if let Some(mut view) = self
                .ui
                .widget(cx, ids!(demo_scene))
                .borrow_mut::<StudioActivityViews>()
            {
                view.fit(cx);
            }
        }
        if let Some(at) = self.ui.slider(cx, ids!(demo_time)).slided(actions) {
            self.seek_demo(cx, at.round());
        }
        if self.ui.button(cx, ids!(demo_start)).clicked(actions) {
            self.seek_demo(cx, 0.0);
        }
        if self.ui.button(cx, ids!(demo_end)).clicked(actions) {
            self.seek_demo(cx, activity_demo::DURATION);
        }
        if self.ui.button(cx, ids!(demo_prev)).clicked(actions) {
            let at = activity_demo::events()
                .iter()
                .rev()
                .find(|e| e.at < self.demo_at - 0.01)
                .map(|e| e.at)
                .unwrap_or(0.0);
            self.seek_demo(cx, at);
        }
        if self.ui.button(cx, ids!(demo_next)).clicked(actions) {
            let at = activity_demo::events()
                .iter()
                .find(|e| e.at > self.demo_at + 0.01)
                .map(|e| e.at)
                .unwrap_or(activity_demo::DURATION);
            self.seek_demo(cx, at);
        }
        if self.ui.button(cx, ids!(demo_play)).clicked(actions) {
            self.play_demo(cx, !self.demo_playing);
        }
        if self.ui.button(cx, ids!(demo_record)).clicked(actions) {
            self.toggle_demo_recording(cx);
        }
        if self.ui.button(cx, ids!(demo_watch)).clicked(actions) {
            self.play_demo_recording(cx);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(demo_speed)).changed(actions) {
            self.demo_speed = [1.0, 4.0, 12.0][index.min(2)];
        }
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            if let ActivityViewAction::Selected(id) = action.cast::<ActivityViewAction>() {
                self.demo_selected = Some(id);
                self.refresh_demo(cx);
                self.refresh_ai_context(cx);
            }
        }
    }
}
