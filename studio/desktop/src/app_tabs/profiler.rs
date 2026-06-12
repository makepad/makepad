use super::*;

impl App {
    pub(crate) fn find_profiler_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(id!(log_first)).is_some() {
            return Some(id!(log_first));
        }
        for (tab_id, state) in &self.data.profiler_tab_state {
            if state.mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        for (tab_id, state) in &self.data.log_tab_state {
            if state.mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }

    pub(crate) fn ensure_profiler_tab_for_build(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        mount: &str,
        title: &str,
        select: bool,
    ) -> Option<LiveId> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        if let Some(tab_id) = self.data.profiler_tab_by_build.get(&build_id).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                if select {
                    dock.select_tab(cx, tab_id);
                    self.data
                        .active_log_build_by_mount
                        .insert(mount.to_string(), build_id);
                }
                return Some(tab_id);
            }
            self.data.profiler_tab_by_build.remove(&build_id);
            self.data.profiler_tab_state.remove(&tab_id);
        }

        let anchor = self.find_profiler_anchor_tab(&dock, mount)?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("prof/{}/{}", mount, build_id.0)).0);
        Self::create_dock_tab(
            &dock,
            cx,
            anchor,
            tab_id,
            id!(ProfilerPane),
            title.to_string(),
            select,
        )?;

        self.data.profiler_tab_by_build.insert(build_id, tab_id);
        self.data.profiler_tab_state.insert(
            tab_id,
            ProfilerTabState {
                mount: mount.to_string(),
                build_id,
                title: title.to_string(),
            },
        );
        dock.set_tab_title(cx, tab_id, title.to_string());
        Some(tab_id)
    }

    pub(crate) fn start_profiler_query_for_build(&mut self, build_id: QueryId) {
        if let Some(prev_query_id) = self.data.live_profiler_query_by_build.remove(&build_id) {
            self.data
                .profiler_query_build_by_query
                .remove(&prev_query_id);
            let _ = self.send_studio(ClientToHub::CancelQuery {
                query_id: prev_query_id,
            });
        }
        let time_start = self
            .data
            .profiler_time_start_by_build
            .get(&build_id)
            .copied();
        let Some(query_id) = self.send_studio(ClientToHub::QueryProfiler {
            build_id: Some(build_id),
            sample_type: None,
            time_start,
            time_end: None,
            max_samples: Some(4096),
            live: Some(true),
        }) else {
            return;
        };
        self.data
            .live_profiler_query_by_build
            .insert(build_id, query_id);
        self.data
            .profiler_query_build_by_query
            .insert(query_id, build_id);
    }

    pub(crate) fn latest_profiler_sample_end(&self, build_id: QueryId) -> Option<f64> {
        let samples = self.data.profiler_samples_by_build.get(&build_id)?;
        [
            samples.event_samples.last().map(|sample| sample.end),
            samples.gpu_samples.last().map(|sample| sample.end),
            samples.gc_samples.last().map(|sample| sample.end),
        ]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.total_cmp(b))
    }

    pub(crate) fn stop_profiler_query_for_build(&mut self, build_id: QueryId) {
        if let Some(query_id) = self.data.live_profiler_query_by_build.remove(&build_id) {
            self.data.profiler_query_build_by_query.remove(&query_id);
            let _ = self.send_studio(ClientToHub::CancelQuery { query_id });
        }
    }

    pub(crate) fn profiler_target_for_mount(&self, mount: &str) -> Option<(QueryId, String)> {
        if let Some(build_id) = self.data.active_log_build_by_mount.get(mount).copied() {
            let title = self
                .data
                .build_package
                .get(&build_id)
                .cloned()
                .unwrap_or_else(|| format!("build {}", build_id.0));
            return Some((build_id, title));
        }
        let build_id = self
            .data
            .log_tab_state
            .values()
            .find(|state| state.mount == mount)
            .map(|state| state.build_id)
            .or_else(|| {
                self.data
                    .run_tab_state
                    .values()
                    .find(|state| state.mount == mount)
                    .map(|state| state.build_id)
            })?;
        let title = self
            .data
            .build_package
            .get(&build_id)
            .cloned()
            .unwrap_or_else(|| format!("build {}", build_id.0));
        Some((build_id, title))
    }

    pub(crate) fn open_profiler_for_mount(&mut self, cx: &mut Cx, mount: &str) {
        let Some((build_id, title)) = self.profiler_target_for_mount(mount) else {
            self.set_status(cx, "no build selected for profiler");
            return;
        };
        let tab_title = format!("{} profile", title);
        let Some(tab_id) =
            self.ensure_profiler_tab_for_build(cx, build_id, mount, &tab_title, true)
        else {
            self.set_status(cx, "failed to create profiler tab");
            return;
        };
        let running = *self
            .data
            .profiler_running_by_build
            .entry(build_id)
            .or_insert(true);
        if running {
            self.start_profiler_query_for_build(build_id);
        } else {
            self.stop_profiler_query_for_build(build_id);
        }
        if let Some(dock) = self.mount_workspace_dock(cx, mount) {
            dock.redraw_tab(cx, tab_id);
        }
        self.set_status(cx, &format!("opened profiler for {}", title));
    }

    pub(crate) fn handle_profiler_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(profiler_action) = widget_action
                .action
                .downcast_ref::<DesktopProfilerViewAction>()
            else {
                continue;
            };
            match profiler_action {
                DesktopProfilerViewAction::SetRunning { build_id, running } => {
                    self.data
                        .profiler_running_by_build
                        .insert(*build_id, *running);
                    if *running {
                        if let Some(last_end) = self.latest_profiler_sample_end(*build_id) {
                            self.data
                                .profiler_time_start_by_build
                                .insert(*build_id, last_end + 0.000_001);
                        }
                        self.data.profiler_samples_by_build.remove(build_id);
                        self.start_profiler_query_for_build(*build_id);
                    } else {
                        self.stop_profiler_query_for_build(*build_id);
                    }
                    if let Some(tab_id) = self.data.profiler_tab_by_build.get(build_id).copied() {
                        let mount = self
                            .data
                            .profiler_tab_state
                            .get(&tab_id)
                            .map(|state| state.mount.clone());
                        if let Some(mount) = mount {
                            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                                dock.redraw_tab(cx, tab_id);
                            }
                        }
                    }
                }
                DesktopProfilerViewAction::Clear { build_id } => {
                    if let Some(last_end) = self.latest_profiler_sample_end(*build_id) {
                        self.data
                            .profiler_time_start_by_build
                            .insert(*build_id, last_end + 0.000_001);
                    }
                    self.data.profiler_samples_by_build.remove(build_id);
                    if self
                        .data
                        .profiler_running_by_build
                        .get(build_id)
                        .copied()
                        .unwrap_or(true)
                    {
                        self.start_profiler_query_for_build(*build_id);
                    }
                    if let Some(tab_id) = self.data.profiler_tab_by_build.get(build_id).copied() {
                        let mount = self
                            .data
                            .profiler_tab_state
                            .get(&tab_id)
                            .map(|state| state.mount.clone());
                        if let Some(mount) = mount {
                            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                                dock.redraw_tab(cx, tab_id);
                            }
                        }
                    }
                }
                DesktopProfilerViewAction::None => {}
            }
        }
    }

    pub(crate) fn close_profiler_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(state) = self.data.profiler_tab_state.remove(&tab_id) else {
            return;
        };
        self.data.profiler_tab_by_build.remove(&state.build_id);
        self.data.profiler_running_by_build.remove(&state.build_id);
        self.data
            .profiler_time_start_by_build
            .remove(&state.build_id);
        if self.data.active_log_build_by_mount.get(&state.mount) == Some(&state.build_id) {
            self.data.active_log_build_by_mount.remove(&state.mount);
        }
        self.stop_profiler_query_for_build(state.build_id);
        if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
            dock.close_tab(cx, tab_id);
        }
    }
}
