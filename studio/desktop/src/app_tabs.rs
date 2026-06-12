use super::*;

#[path = "app_tabs/dock_helpers.rs"]
pub mod dock_helpers;
use dock_helpers::*;

#[path = "app_tabs/editor.rs"]
pub mod editor;
use editor::*;

#[path = "app_tabs/run.rs"]
pub mod run;
use run::*;

#[path = "app_tabs/log.rs"]
pub mod log;
use log::*;

#[path = "app_tabs/profiler.rs"]
pub mod profiler;
use profiler::*;

impl App {
    pub(super) fn send_terminal_input(&mut self, path: &str, data: Vec<u8>) {
        self.ensure_terminal_session_open(path);
        let _ = self.send_studio(ClientToHub::TerminalInput {
            path: path.to_string(),
            data,
        });
    }

    pub(super) fn request_terminal_viewport(
        &mut self,
        path: &str,
        cols: u16,
        rows: u16,
        pty_rows: u16,
        top_row: usize,
    ) {
        self.ensure_terminal_session_open(path);
        if !self.data.terminal_open_paths.contains(path) {
            return;
        }
        let _ = self.send_studio(ClientToHub::TerminalViewportRequest {
            path: path.to_string(),
            cols,
            rows,
            pty_rows,
            top_row,
        });
    }

    pub(super) fn handle_terminal_actions(&mut self, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(term_action) = widget_action
                .action
                .downcast_ref::<DesktopTerminalViewAction>()
            else {
                continue;
            };
            match term_action {
                DesktopTerminalViewAction::Input { path, data } => {
                    self.send_terminal_input(path, data.clone());
                }
                DesktopTerminalViewAction::RequestViewport {
                    path,
                    cols,
                    rows,
                    pty_rows,
                    top_row,
                } => {
                    self.request_terminal_viewport(path, *cols, *rows, *pty_rows, *top_row);
                }
                DesktopTerminalViewAction::None => {}
            }
        }
    }

    fn drag_source_tab_id(items: &[DragItem]) -> Option<LiveId> {
        if items.len() != 1 {
            return None;
        }
        match &items[0] {
            DragItem::FilePath { internal_id, .. } => *internal_id,
            DragItem::String { .. } => None,
        }
    }

    fn active_mount_dock_containing_tab(&mut self, cx: &mut Cx, tab_id: LiveId) -> Option<DockRef> {
        let active_mount = self.data.active_mount.clone()?;
        if let Some(dock) = self.mount_workspace_dock(cx, &active_mount) {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                return Some(dock);
            }
        }
        let dock = self.mount_terminal_dock(cx, &active_mount)?;
        if dock.find_tab_bar_of_tab(tab_id).is_some() {
            Some(dock)
        } else {
            None
        }
    }

    pub(super) fn start_workspace_tab_drag(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if self.data.tab_to_mount.contains_key(&tab_id) {
            return;
        }
        let Some(dock) = self.active_mount_dock_containing_tab(cx, tab_id) else {
            return;
        };

        dock.tab_start_drag(
            cx,
            tab_id,
            DragItem::FilePath {
                path: String::new(),
                internal_id: Some(tab_id),
            },
        );
    }

    pub(super) fn handle_workspace_tab_drag(&mut self, cx: &mut Cx, drag_event: DragHitEvent) {
        let Some(source_tab_id) = Self::drag_source_tab_id(drag_event.items.as_ref()) else {
            return;
        };
        if self.data.tab_to_mount.contains_key(&source_tab_id) {
            return;
        }
        let Some(dock) = self.active_mount_dock_containing_tab(cx, source_tab_id) else {
            return;
        };

        dock.accept_drag(cx, drag_event, DragResponse::Move);
    }

    pub(super) fn handle_workspace_tab_drop(&mut self, cx: &mut Cx, drop_event: DropHitEvent) {
        let Some(source_tab_id) = Self::drag_source_tab_id(drop_event.items.as_ref()) else {
            return;
        };
        if self.data.tab_to_mount.contains_key(&source_tab_id) {
            return;
        }
        let Some(dock) = self.active_mount_dock_containing_tab(cx, source_tab_id) else {
            return;
        };

        dock.drop_move(cx, drop_event.abs, source_tab_id);

        if self.data.tab_to_path.contains_key(&source_tab_id) {
            self.set_active_tab(cx, source_tab_id);
        } else if let Some((_mount, path)) = self.terminal_tab_mount_path(source_tab_id) {
            self.ensure_terminal_session_open(&path);
        }
    }
}
