pub mod app_data;
pub mod app_ui;
pub mod desktop_code_editor;
pub mod desktop_file_tree;
pub mod desktop_log_view;
pub mod desktop_profiler_view;
pub mod desktop_run_list;
pub mod desktop_run_view;
pub mod desktop_terminal_view;

pub use makepad_code_editor;
pub use makepad_studio_hub;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_widgets::makepad_platform;
pub use makepad_widgets::makepad_platform::log;
pub use makepad_widgets::makepad_script;
pub use makepad_widgets::makepad_script::makepad_live_id;
pub use makepad_widgets::makepad_script::makepad_micro_serde;

use crate::{
    app_data::*,
    desktop_file_tree::*,
    desktop_log_view::*,
    desktop_profiler_view::*,
    desktop_run_list::*,
    desktop_run_view::*,
    desktop_terminal_view::*,
    makepad_code_editor::{
        code_editor::CodeEditorAction, decoration::DecorationSet, history::NewGroup,
        selection::Affinity, session::SelectionMode, text::Position, CodeDocument, CodeSession,
    },
    makepad_studio_hub::{HubConfig, MountConfig, StudioHub},
    makepad_widgets::*,
};
use makepad_studio_protocol::hub_protocol::{
    ClientToHub, FileNodeType, HubToClient, LogEntry, QueryId,
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, Path};

#[path = "app_backend.rs"]
mod app_backend;
#[path = "app_messages.rs"]
mod app_messages;
#[path = "app_state.rs"]
mod app_state;
#[path = "app_tabs.rs"]
mod app_tabs;

pub fn register_script_modules(vm: &mut ScriptVm) {
    crate::desktop_file_tree::script_mod(vm);
    crate::desktop_code_editor::script_mod(vm);
    crate::desktop_log_view::script_mod(vm);
    crate::desktop_profiler_view::script_mod(vm);
    crate::desktop_run_list::script_mod(vm);
    crate::desktop_run_view::script_mod(vm);
    crate::desktop_terminal_view::script_mod(vm);
    crate::app_ui::script_mod(vm);
}

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root {
            AppUI {}
        }
    }
}

fn push_capped_deque<T>(entries: &mut std::collections::VecDeque<T>, entry: T, max_len: usize) {
    entries.push_back(entry);
    while entries.len() > max_len {
        entries.pop_front();
    }
}

fn parse_path_line_column_token(token: &str) -> Option<(String, usize, usize)> {
    let cleaned = token.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'));
    let (path_and_line, column_str) = cleaned.rsplit_once(':')?;
    let (path, line_str) = path_and_line.rsplit_once(':')?;
    let line = line_str.parse::<usize>().ok()?.max(1);
    let column = column_str.parse::<usize>().ok()?.max(1);
    if path.is_empty() {
        return None;
    }
    Some((path.to_string(), line, column))
}

fn path_to_virtual(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    pub ui: WidgetRef,
    #[rust]
    pub data: AppData,
    #[rust]
    pub file_filter_debounce_timer: Timer,
    #[rust]
    pub pending_file_filter: Option<(String, String)>,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.set_current_file_label(cx, None);
        self.start_backend(cx);
        self.load_state(cx, 0);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(active_mount) = self.data.active_mount.clone() {
            if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
                if let Some(node_id) = workspace
                    .desktop_file_tree(cx, ids!(file_tree))
                    .file_clicked(actions)
                {
                    self.open_node_in_editor(cx, node_id);
                }
                if let Some(path) = workspace
                    .desktop_file_tree(cx, ids!(file_tree))
                    .filtered_path_clicked(actions)
                {
                    self.open_path_in_editor(cx, &path);
                }
                if let Some(filter) = workspace
                    .text_input(cx, ids!(file_tree_filter))
                    .changed(actions)
                {
                    self.queue_mount_file_filter(cx, &active_mount, filter);
                }
                if let Some((mount, package, outside_studio)) = workspace
                    .desktop_run_list(cx, ids!(run_list))
                    .run_requested(actions)
                {
                    self.run_package(cx, &mount, &package, outside_studio);
                }
                if workspace.button(cx, ids!(run_stop_all)).clicked(actions) {
                    self.request_stop_all_builds_for_mount(cx, &active_mount);
                }
                if let Some((path, line, column)) = workspace
                    .desktop_log_view(cx, ids!(log_view))
                    .open_location_requested(actions)
                {
                    self.open_log_location(cx, &path, line, column);
                }
                if let Some(tail) = workspace
                    .check_box(cx, ids!(log_tail_toggle))
                    .changed(actions)
                {
                    self.set_mount_log_tail(cx, &active_mount, tail);
                }
                let log_view = workspace.desktop_log_view(cx, ids!(log_view));
                if log_view.scrolled(cx, actions) && !log_view.is_at_end(cx) {
                    let was_tailing = self
                        .mount_state(&active_mount)
                        .map(|state| state.log_tail)
                        .unwrap_or(true);
                    if was_tailing {
                        workspace
                            .check_box(cx, ids!(log_tail_toggle))
                            .set_active(cx, false);
                        self.set_mount_log_tail(cx, &active_mount, false);
                    }
                }
                if let Some(filter) = workspace.text_input(cx, ids!(log_filter)).changed(actions) {
                    self.set_mount_log_filter(&active_mount, filter);
                    self.restart_log_query_for_mount(cx, &active_mount);
                }
                if workspace
                    .button(cx, ids!(clear_log_filter))
                    .clicked(actions)
                {
                    self.set_mount_log_filter(&active_mount, String::new());
                    workspace.text_input(cx, ids!(log_filter)).set_text(cx, "");
                    self.restart_log_query_for_mount(cx, &active_mount);
                }
                if workspace
                    .button(cx, ids!(log_open_profiler))
                    .clicked(actions)
                {
                    self.open_profiler_for_mount(cx, &active_mount);
                }
            }
        }

        self.handle_run_view_actions(actions);
        self.handle_profiler_actions(cx, actions);
        self.handle_terminal_actions(actions);

        for action in actions {
            if let Some(action) = action.as_widget_action() {
                match action.cast() {
                    DockAction::TabWasPressed(tab_id) => {
                        if let Some(mount) = self.data.tab_to_mount.get(&tab_id).cloned() {
                            self.select_mount(cx, &mount);
                        } else if tab_id == id!(terminal_add) {
                            if let Some(mount) = self.data.active_mount.clone() {
                                self.create_new_terminal_tab(cx, &mount);
                            }
                        } else {
                            if let Some(state) = self.data.log_tab_state.get(&tab_id) {
                                self.data
                                    .active_log_build_by_mount
                                    .insert(state.mount.clone(), state.build_id);
                            } else if let Some(state) = self.data.profiler_tab_state.get(&tab_id) {
                                self.data
                                    .active_log_build_by_mount
                                    .insert(state.mount.clone(), state.build_id);
                            }
                            if let Some((_mount, path)) = self.terminal_tab_mount_path(tab_id) {
                                self.ensure_terminal_session_open(&path);
                            }
                            self.set_active_tab(cx, tab_id);
                        }
                    }
                    DockAction::TabCloseWasPressed(tab_id) => {
                        if self.data.tab_to_mount.contains_key(&tab_id) {
                            continue;
                        }
                        if self.data.run_tab_state.contains_key(&tab_id) {
                            self.close_run_tab(cx, tab_id);
                        } else if self.data.log_tab_state.contains_key(&tab_id) {
                            self.close_log_tab(cx, tab_id);
                        } else if self.data.profiler_tab_state.contains_key(&tab_id) {
                            self.close_profiler_tab(cx, tab_id);
                        } else if self.data.tab_to_path.contains_key(&tab_id) {
                            self.close_editor_tab(cx, tab_id);
                        } else if tab_id != id!(terminal_add) {
                            if let Some((mount, _path)) = self.terminal_tab_mount_path(tab_id) {
                                self.delete_terminal_tab_file(cx, &mount, tab_id);
                            } else if tab_id != id!(terminal_first) {
                                if let Some(mount) = self.data.active_mount.clone() {
                                    if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                                        dock.close_tab(cx, tab_id);
                                    }
                                }
                            }
                        }
                    }
                    DockAction::ShouldTabStartDrag(tab_id) => {
                        self.start_workspace_tab_drag(cx, tab_id);
                    }
                    DockAction::Drag(drag_event) => {
                        self.handle_workspace_tab_drag(cx, drag_event);
                    }
                    DockAction::Drop(drop_event) => {
                        self.handle_workspace_tab_drop(cx, drop_event);
                    }
                    DockAction::SplitPanelChanged { .. } | DockAction::None => {}
                }

                match action.cast() {
                    CodeEditorAction::TextDidChange => {
                        let tab_id = Self::tab_id_from_widget_uid(cx, action.widget_uid);
                        self.save_tab_file(cx, tab_id);
                    }
                    CodeEditorAction::UnhandledKeyDown(_) => {}
                    CodeEditorAction::None => {}
                }
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        crate::register_script_modules(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Timer(timer_event) = event {
            if self
                .file_filter_debounce_timer
                .is_timer(timer_event)
                .is_some()
            {
                self.flush_queued_mount_file_filter(cx);
            }
        }
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.data));
        if matches!(event, Event::Signal) {
            self.drain_studio_messages(cx);
        }
        self.refresh_run_view_targets(cx);
        self.save_state_if_needed(cx);
    }
}
