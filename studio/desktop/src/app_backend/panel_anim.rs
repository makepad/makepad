use crate::{
    makepad_widgets::*,
    App, SidebarAnimation, BottomPanelAnimation, AgentPanelAnimation,
};

impl App {
    pub(crate) fn panel_animation_progress(time: f64, start_time: &mut Option<f64>) -> f64 {
        let start_time = start_time.get_or_insert(time);
        let elapsed = (time - *start_time).max(0.0);
        let duration = 0.16;
        let progress = (elapsed / duration).min(1.0);
        1.0 - (1.0 - progress).powi(3)
    }

    pub(crate) fn workspace_root_splitter_position(&mut self, cx: &mut Cx, mount: &str) -> Option<f64> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        dock.splitter_position(id!(root))
    }

    pub(crate) fn set_workspace_root_splitter_width(&mut self, cx: &mut Cx, mount: &str, width: f64) -> bool {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return false;
        };
        dock.set_splitter_align(cx, id!(root), SplitterAlign::FromA(width.max(0.0)), false)
    }

    pub(crate) fn start_sidebar_animation(&mut self, cx: &mut Cx, mount: &str, to_width: f64) {
        let from_width = self
            .workspace_root_splitter_position(cx, mount)
            .unwrap_or(to_width);
        self.sidebar_animation = Some(SidebarAnimation {
            mount: mount.to_string(),
            from_width,
            to_width: to_width.max(0.0),
            start_time: None,
        });
        self.sidebar_animation_next_frame = cx.new_next_frame();
    }

    pub(crate) fn step_sidebar_animation(&mut self, cx: &mut Cx, time: f64) {
        let Some(animation) = self.sidebar_animation.as_mut() else {
            return;
        };
        let eased = Self::panel_animation_progress(time, &mut animation.start_time);
        let progress = eased;
        let mount = animation.mount.clone();
        let target = animation.to_width;
        let width = animation.from_width + (target - animation.from_width) * eased;

        if !self.set_workspace_root_splitter_width(cx, &mount, width) {
            self.sidebar_animation = None;
            return;
        }

        if progress >= 1.0 {
            self.sidebar_animation = None;
            self.set_workspace_root_splitter_width(cx, &mount, target);
            self.save_state(cx, 0);
        } else {
            self.sidebar_animation_next_frame = cx.new_next_frame();
        }
    }

    pub(crate) fn workspace_main_splitter_height(&mut self, cx: &mut Cx, mount: &str) -> Option<f64> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        let dock_height = dock.area().rect(cx).size.y.max(0.0);
        let splitter_position = dock.splitter_position(id!(main_split))?;
        Some((dock_height - splitter_position).max(0.0))
    }

    pub(crate) fn set_workspace_main_splitter_height(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        height: f64,
    ) -> bool {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return false;
        };
        dock.set_splitter_align(
            cx,
            id!(main_split),
            SplitterAlign::FromB(height.max(0.0)),
            false,
        )
    }

    pub(crate) fn workspace_agent_splitter_width(&mut self, cx: &mut Cx, mount: &str) -> Option<f64> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        let dock_width = dock.area().rect(cx).size.x.max(0.0);
        if let Some(DockItem::Splitter { align, .. }) = dock.clone_state()?.get(&id!(agent_split)) {
            return match align {
                SplitterAlign::FromB(width) => Some(width.max(0.0)),
                SplitterAlign::FromA(width) => Some((dock_width - width).max(0.0)),
                SplitterAlign::Weighted(weight) => Some((dock_width * (1.0 - weight)).max(0.0)),
            };
        }
        let splitter_position = dock.splitter_position(id!(agent_split))?;
        Some((dock_width - splitter_position).max(0.0))
    }

    pub(crate) fn set_workspace_agent_splitter_width(&mut self, cx: &mut Cx, mount: &str, width: f64) -> bool {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return false;
        };
        dock.set_splitter_align(
            cx,
            id!(agent_split),
            SplitterAlign::FromB(width.max(0.0)),
            false,
        )
    }

    pub(crate) fn workspace_selected_sidebar_tab(&mut self, cx: &mut Cx, mount: &str) -> Option<LiveId> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        let dock_items = dock.clone_state()?;
        let Some(DockItem::Tabs { tabs, selected, .. }) = dock_items.get(&id!(tree_tabs)) else {
            return None;
        };
        tabs.get(*selected).copied()
    }

    pub(crate) fn workspace_sidebar_is_open(&mut self, cx: &mut Cx, mount: &str) -> bool {
        self.sidebar_animation
            .as_ref()
            .filter(|animation| animation.mount == mount)
            .map(|animation| animation.to_width > 1.0)
            .or_else(|| {
                self.workspace_root_splitter_position(cx, mount)
                    .map(|width| width > 1.0)
            })
            .unwrap_or(false)
    }

    pub(crate) fn apply_bottom_bar_button_style(
        &mut self,
        cx: &mut Cx,
        button_id: LiveId,
        active: bool,
        _active_bg: Vec4f,
        _active_bg_hover: Vec4f,
        active_icon: Vec4f,
    ) {
        let mut button = self.ui.widget(cx, &[button_id]);
        let bg = Vec4f::from_u32(0x00000000);
        let bg_hover = Vec4f::from_u32(0xffffff15);
        let bg_down = Vec4f::from_u32(0xffffff24);
        let icon = if active {
            active_icon
        } else {
            Vec4f::from_u32(0x8c8c8cff)
        };
        let icon_hover = if active {
            active_icon
        } else {
            Vec4f::from_u32(0xffffffff)
        };
        let icon_down = if active {
            active_icon
        } else {
            Vec4f::from_u32(0xffffffff)
        };
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(bg)
                color_hover: #(bg_hover)
                color_down: #(bg_down)
                color_focus: #(bg)
            }
            draw_icon +: {
                color: #(icon)
                color_hover: #(icon_hover)
                color_down: #(icon_down)
                color_focus: #(icon)
            }
        });
    }

    pub(crate) fn sync_bottom_bar_state(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };

        let selected_sidebar_tab = self.workspace_selected_sidebar_tab(cx, &active_mount);
        let sidebar_open = self.workspace_sidebar_is_open(cx, &active_mount);
        let file_active = sidebar_open && selected_sidebar_tab == Some(id!(tree_tab));
        let run_active = sidebar_open && selected_sidebar_tab == Some(id!(run_list_tab));
        let terminal_active = self
            .bottom_panel_animation
            .as_ref()
            .filter(|animation| animation.mount == active_mount)
            .map(|animation| animation.to_height > 1.0)
            .or_else(|| {
                self.workspace_main_splitter_height(cx, &active_mount)
                    .map(|height| height > 1.0)
            })
            .unwrap_or(false);
        let agent_active = self
            .agent_panel_animation
            .as_ref()
            .filter(|animation| animation.mount == active_mount)
            .map(|animation| animation.to_width > 1.0)
            .or_else(|| {
                self.workspace_agent_splitter_width(cx, &active_mount)
                    .map(|width| width > 1.0)
            })
            .unwrap_or(false);

        let active_color = Vec4f::from_u32(0x61afefff);
        self.apply_bottom_bar_button_style(
            cx,
            id!(bottom_file_tree_toggle),
            file_active,
            Vec4f::from_u32(0x0),
            Vec4f::from_u32(0x0),
            active_color,
        );
        self.apply_bottom_bar_button_style(
            cx,
            id!(bottom_run_list_toggle),
            run_active,
            Vec4f::from_u32(0x0),
            Vec4f::from_u32(0x0),
            active_color,
        );
        self.apply_bottom_bar_button_style(
            cx,
            id!(bottom_panel_toggle),
            terminal_active,
            Vec4f::from_u32(0x0),
            Vec4f::from_u32(0x0),
            active_color,
        );
        self.apply_bottom_bar_button_style(
            cx,
            id!(bottom_agent_toggle),
            agent_active,
            Vec4f::from_u32(0x0),
            Vec4f::from_u32(0x0),
            active_color,
        );
    }

    pub(crate) fn start_bottom_panel_animation(&mut self, cx: &mut Cx, mount: &str, to_height: f64) {
        let from_height = self
            .workspace_main_splitter_height(cx, mount)
            .unwrap_or(to_height);
        self.bottom_panel_animation = Some(BottomPanelAnimation {
            mount: mount.to_string(),
            from_height,
            to_height: to_height.max(0.0),
            start_time: None,
        });
        self.bottom_panel_animation_next_frame = cx.new_next_frame();
    }

    pub(crate) fn step_bottom_panel_animation(&mut self, cx: &mut Cx, time: f64) {
        let Some(animation) = self.bottom_panel_animation.as_mut() else {
            return;
        };
        let eased = Self::panel_animation_progress(time, &mut animation.start_time);
        let progress = eased;
        let mount = animation.mount.clone();
        let target = animation.to_height;
        let height = animation.from_height + (target - animation.from_height) * eased;

        if !self.set_workspace_main_splitter_height(cx, &mount, height) {
            self.bottom_panel_animation = None;
            return;
        }

        if progress >= 1.0 {
            self.bottom_panel_animation = None;
            self.set_workspace_main_splitter_height(cx, &mount, target);
            self.save_state(cx, 0);
        } else {
            self.bottom_panel_animation_next_frame = cx.new_next_frame();
        }
    }

    pub(crate) fn start_agent_panel_animation(&mut self, cx: &mut Cx, mount: &str, to_width: f64) {
        let from_width = self
            .workspace_agent_splitter_width(cx, mount)
            .unwrap_or(to_width);
        self.agent_panel_animation = Some(AgentPanelAnimation {
            mount: mount.to_string(),
            from_width,
            to_width: to_width.max(0.0),
            start_time: None,
        });
        self.agent_panel_animation_next_frame = cx.new_next_frame();
    }

    pub(crate) fn step_agent_panel_animation(&mut self, cx: &mut Cx, time: f64) {
        let Some(animation) = self.agent_panel_animation.as_mut() else {
            return;
        };
        let eased = Self::panel_animation_progress(time, &mut animation.start_time);
        let progress = eased;
        let mount = animation.mount.clone();
        let target = animation.to_width;
        let width = animation.from_width + (target - animation.from_width) * eased;

        if !self.set_workspace_agent_splitter_width(cx, &mount, width) {
            self.agent_panel_animation = None;
            return;
        }

        if progress >= 1.0 {
            self.agent_panel_animation = None;
            self.set_workspace_agent_splitter_width(cx, &mount, target);
            self.save_state(cx, 0);
        } else {
            self.agent_panel_animation_next_frame = cx.new_next_frame();
        }
    }

    pub(crate) fn sync_mount_tab_bar_visibility(&mut self, cx: &mut Cx) {
        let dock = self.ui.dock(cx, ids!(mount_dock));
        let Some(mut dock_items) = dock.clone_state() else {
            return;
        };

        let mut changed = false;
        for (item_id, item) in dock_items.iter_mut() {
            let DockItem::Tabs {
                tabs,
                selected,
                closable,
                hide_tab_bar,
            } = item
            else {
                continue;
            };
            let should_hide = *item_id == id!(tree_tabs) || tabs.len() <= 1;
            if *hide_tab_bar == should_hide {
                continue;
            }
            *item = DockItem::Tabs {
                tabs: tabs.clone(),
                selected: *selected,
                closable: *closable,
                hide_tab_bar: should_hide,
            };
            changed = true;
        }

        if changed {
            dock.load_state(cx, dock_items);
        }
    }

    pub(crate) fn select_sidebar_tab(&mut self, cx: &mut Cx, mount: &str, tab_id: LiveId) {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };
        dock.select_tab(cx, tab_id);

        let Some(current_width) = self.workspace_root_splitter_position(cx, mount) else {
            return;
        };
        if current_width <= 1.0 {
            let restore_width = self
                .mount_state(mount)
                .and_then(|state| state.sidebar_restore_width)
                .unwrap_or(310.0);
            self.start_sidebar_animation(cx, mount, restore_width);
        }
    }

    pub(crate) fn toggle_sidebar_tab(&mut self, cx: &mut Cx, mount: &str, tab_id: LiveId) {
        let is_selected = self.workspace_selected_sidebar_tab(cx, mount) == Some(tab_id);
        let is_open = self.workspace_sidebar_is_open(cx, mount);

        if is_selected && is_open {
            if let Some(current_width) = self.workspace_root_splitter_position(cx, mount) {
                self.mount_state_mut(mount).sidebar_restore_width = Some(current_width);
                self.start_sidebar_animation(cx, mount, 0.0);
            }
            return;
        }

        self.select_sidebar_tab(cx, mount, tab_id);
    }

    pub(crate) fn toggle_mount_sidebar(&mut self, cx: &mut Cx, mount: &str) {
        let Some(current_width) = self.workspace_root_splitter_position(cx, mount) else {
            return;
        };
        let restore_width = self
            .mount_state(mount)
            .and_then(|state| state.sidebar_restore_width)
            .unwrap_or(310.0);

        if current_width <= 1.0 {
            self.start_sidebar_animation(cx, mount, restore_width);
        } else {
            self.mount_state_mut(mount).sidebar_restore_width = Some(current_width);
            self.start_sidebar_animation(cx, mount, 0.0);
        }
    }

    pub(crate) fn toggle_agent_panel(&mut self, cx: &mut Cx, mount: &str) {
        let Some(current_width) = self.workspace_agent_splitter_width(cx, mount) else {
            return;
        };
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };
        dock.select_tab(cx, id!(ai_tab));

        let restore_width = self
            .mount_state(mount)
            .and_then(|state| state.agent_panel_restore_width)
            .unwrap_or(310.0);

        if current_width <= 1.0 {
            self.start_agent_panel_animation(cx, mount, restore_width);
        } else {
            self.mount_state_mut(mount).agent_panel_restore_width = Some(current_width);
            self.start_agent_panel_animation(cx, mount, 0.0);
        }
    }

    pub(crate) fn toggle_bottom_panel(&mut self, cx: &mut Cx, mount: &str) {
        let Some(current_height) = self.workspace_main_splitter_height(cx, mount) else {
            return;
        };
        let restore_height = self
            .mount_state(mount)
            .and_then(|state| state.bottom_panel_restore_height)
            .unwrap_or(220.0);

        if current_height <= 1.0 {
            self.start_bottom_panel_animation(cx, mount, restore_height);
        } else {
            self.mount_state_mut(mount).bottom_panel_restore_height = Some(current_height);
            self.start_bottom_panel_animation(cx, mount, 0.0);
        }
    }
}
