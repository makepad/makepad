use super::*;

pub(crate) fn run_preview_splitter_is_collapsed(align: SplitterAlign) -> bool {
    matches!(align, SplitterAlign::Weighted(w) if w >= 0.999)
}

pub(crate) fn run_preview_splitter_restore_target(
    current: SplitterAlign,
    has_runs: bool,
    restore: Option<SplitterAlign>,
) -> Option<SplitterAlign> {
    if has_runs && run_preview_splitter_is_collapsed(current) {
        Some(restore.unwrap_or(SplitterAlign::Weighted(0.62)))
    } else {
        None
    }
}

impl App {
    pub(crate) fn sync_run_preview_splitter(&mut self, cx: &mut Cx, mount: &str) {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };
        let has_runs = self.data.run_tab_state.values().any(|s| s.mount == mount);
        let Some(items) = dock.clone_state() else {
            return;
        };
        let Some(current) = items.get(&id!(editor_split)).and_then(|item| {
            if let DockItem::Splitter { align, .. } = item {
                Some(*align)
            } else {
                None
            }
        }) else {
            return;
        };
        let collapsed = run_preview_splitter_is_collapsed(current);
        if !collapsed {
            self.data
                .run_panel_split_restore
                .insert(mount.to_string(), current);
        }
        if let Some(align) = run_preview_splitter_restore_target(
            current,
            has_runs,
            self.data.run_panel_split_restore.remove(mount),
        ) {
            dock.set_splitter_align(cx, id!(editor_split), align, false);
        }
    }

    pub(crate) fn ensure_run_tab_for_build(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        mount: &str,
        package: &str,
        select: bool,
    ) -> Option<LiveId> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                let window_id = self
                    .data
                    .run_tab_state
                    .get(&tab_id)
                    .and_then(|state| state.window_id);
                let addr = self.studio_addr();
                dock.item(tab_id)
                    .desktop_run_view(cx, ids!(run_view))
                    .set_run_target(cx, build_id, window_id, addr.as_deref());
                if select {
                    dock.select_tab(cx, tab_id);
                }
                self.sync_run_preview_splitter(cx, mount);
                return Some(tab_id);
            }
            self.data.run_tab_by_build.remove(&build_id);
            self.data.run_tab_state.remove(&tab_id);
        }

        let anchor = Self::find_anchor_tab_in(
            &dock,
            id!(run_first),
            self.data
                .run_tab_state
                .iter()
                .map(|(id, s)| (id, s.mount.as_str())),
            mount,
        )?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("run/{}/{}", mount, build_id.0)).0);
        Self::create_dock_tab(
            &dock,
            cx,
            anchor,
            tab_id,
            id!(RunningAppPane),
            package.to_string(),
            select,
        )?;

        self.data.run_tab_by_build.insert(build_id, tab_id);
        self.data.run_tab_state.insert(
            tab_id,
            RunTabState {
                mount: mount.to_string(),
                package: package.to_string(),
                build_id,
                status: "starting".to_string(),
                window_id: None,
            },
        );
        dock.set_tab_title(cx, tab_id, package.to_string());
        let addr = self.studio_addr();
        dock.item(tab_id)
            .desktop_run_view(cx, ids!(run_view))
            .set_run_target(cx, build_id, None, addr.as_deref());
        self.sync_run_preview_splitter(cx, mount);
        Some(tab_id)
    }

    pub(crate) fn refresh_run_view_targets(&mut self, cx: &mut Cx) {
        let targets: Vec<(LiveId, String, QueryId, Option<usize>)> = self
            .data
            .run_tab_state
            .iter()
            .filter_map(|(tab_id, state)| {
                let active_mount = self.data.build_to_mount.get(&state.build_id)?;
                if active_mount != &state.mount {
                    return None;
                }
                Some((
                    *tab_id,
                    state.mount.clone(),
                    state.build_id,
                    state.window_id,
                ))
            })
            .collect();

        let addr = self.studio_addr();
        for (tab_id, mount, build_id, window_id) in targets {
            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                dock.item(tab_id)
                    .desktop_run_view(cx, ids!(run_view))
                    .set_run_target(cx, build_id, window_id, addr.as_deref());
            }
        }
    }

    pub(crate) fn run_item(&mut self, cx: &mut Cx, mount: &str, name: &str) {
        if self.data.active_mount.as_deref() != Some(mount) {
            self.select_mount(cx, mount);
        }
        let Some(_query_id) = self.send_studio(ClientToHub::RunItem {
            mount: mount.to_string(),
            name: name.to_string(),
        }) else {
            self.set_status(cx, "backend not connected");
            return;
        };
        self.close_mount_run_and_log_tabs(cx, mount);
        self.set_status(cx, &format!("running {} on {}", name, mount));
    }

    pub(crate) fn handle_run_view_actions(&mut self, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(run_action) = widget_action.action.downcast_ref::<DesktopRunViewAction>()
            else {
                continue;
            };
            match run_action {
                DesktopRunViewAction::ForwardToApp { build_id, msg_bin } => {
                    let _ = self.send_studio(ClientToHub::ForwardToApp {
                        build_id: *build_id,
                        msg_bin: msg_bin.clone(),
                    });
                }
                DesktopRunViewAction::None => {}
            }
        }
    }

    pub(crate) fn close_run_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(state) = self.data.run_tab_state.remove(&tab_id) else {
            return;
        };
        self.data.run_tab_by_build.remove(&state.build_id);
        self.data.build_to_mount.remove(&state.build_id);
        let _ = self.send_studio(ClientToHub::StopBuild {
            build_id: state.build_id,
        });
        if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
            dock.close_tab(cx, tab_id);
        }
        self.sync_run_preview_splitter(cx, &state.mount);
    }

    pub(crate) fn close_mount_run_and_log_tabs(&mut self, cx: &mut Cx, mount: &str) {
        let run_tabs: Vec<LiveId> = self
            .data
            .run_tab_state
            .iter()
            .filter_map(|(tab_id, state)| (state.mount == mount).then_some(*tab_id))
            .collect();
        let log_tabs: Vec<LiveId> = self
            .data
            .log_tab_state
            .iter()
            .filter_map(|(tab_id, state)| (state.mount == mount).then_some(*tab_id))
            .collect();

        for tab_id in log_tabs {
            self.close_log_tab(cx, tab_id);
        }
        for tab_id in run_tabs {
            self.close_run_tab(cx, tab_id);
        }
    }

    pub(crate) fn clear_build_tabs(&mut self, cx: &mut Cx, build_id: QueryId) {
        let mount_for_sync = self.data.build_to_mount.get(&build_id).cloned();
        let run_tab_id = self.data.run_tab_by_build.remove(&build_id);
        let log_tab_id = self.data.log_tab_by_build.remove(&build_id);
        let profiler_tab_id = self.data.profiler_tab_by_build.remove(&build_id);

        if let Some(tab_id) = run_tab_id {
            if let Some(state) = self.data.run_tab_state.remove(&tab_id) {
                if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
                    dock.close_tab(cx, tab_id);
                }
            }
        }

        if let Some(tab_id) = log_tab_id {
            if let Some(state) = self.data.log_tab_state.remove(&tab_id) {
                if self.data.active_log_build_by_mount.get(&state.mount) == Some(&build_id) {
                    self.data.active_log_build_by_mount.remove(&state.mount);
                }
                if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
                    dock.close_tab(cx, tab_id);
                }
            }
        }

        if let Some(tab_id) = profiler_tab_id {
            if let Some(state) = self.data.profiler_tab_state.remove(&tab_id) {
                if self.data.active_log_build_by_mount.get(&state.mount) == Some(&build_id) {
                    self.data.active_log_build_by_mount.remove(&state.mount);
                }
                if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
                    dock.close_tab(cx, tab_id);
                }
            }
        }

        self.stop_profiler_query_for_build(build_id);
        self.data.profiler_running_by_build.remove(&build_id);
        self.data.profiler_time_start_by_build.remove(&build_id);
        self.data.profiler_samples_by_build.remove(&build_id);
        self.data.build_log_entries.remove(&build_id);
        self.data.build_to_mount.remove(&build_id);
        self.data.build_package.remove(&build_id);
        if let Some(mount) = mount_for_sync {
            self.sync_run_preview_splitter(cx, &mount);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_weighted(align: Option<SplitterAlign>, expected: f64) {
        match align {
            Some(SplitterAlign::Weighted(actual)) => {
                assert!(
                    (actual - expected).abs() < 0.0001,
                    "expected {expected}, got {actual}"
                );
            }
            Some(other) => panic!("expected weighted splitter align, got {:?}", other),
            None => panic!("expected weighted splitter align, got none"),
        }
    }

    #[test]
    fn no_active_runs_do_not_auto_collapse_preview() {
        assert!(run_preview_splitter_restore_target(
            SplitterAlign::Weighted(0.62),
            false,
            Some(SplitterAlign::Weighted(0.4)),
        )
        .is_none());
    }

    #[test]
    fn collapsed_preview_restores_saved_align_when_run_starts() {
        assert_weighted(
            run_preview_splitter_restore_target(
                SplitterAlign::Weighted(1.0),
                true,
                Some(SplitterAlign::Weighted(0.4)),
            ),
            0.4,
        );
    }

    #[test]
    fn collapsed_preview_uses_default_align_without_saved_state() {
        assert_weighted(
            run_preview_splitter_restore_target(SplitterAlign::Weighted(1.0), true, None),
            0.62,
        );
    }
}
