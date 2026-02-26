use crate::{
    desktop_file_tree::*,
    makepad_code_editor::{
        code_editor::CodeEditorAction,
        decoration::DecorationSet,
        CodeDocument,
        CodeSession,
    },
    makepad_micro_serde::*,
    makepad_studio_backend::{
        BackendConfig,
        FileNodeType,
        FileTreeData,
        GitStatus,
        MountConfig,
        QueryId,
        StudioBackend,
        StudioConnection,
        StudioToUI,
        UIToStudio,
    },
    makepad_widgets::{file_tree::GitStatusDotKind, *},
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::Write;

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

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        crate::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    fn start_backend(&mut self, cx: &mut Cx) {
        let root = match env::current_dir().and_then(|p| p.canonicalize()) {
            Ok(path) => path,
            Err(err) => {
                self.set_status(cx, &format!("failed to resolve current dir: {}", err));
                return;
            }
        };

        let config = BackendConfig {
            mounts: vec![MountConfig {
                name: "makepad".to_string(),
                path: root,
            }],
            ..Default::default()
        };

        match StudioBackend::start_in_process(config) {
            Ok(mut studio) => {
                let _ = studio.send(UIToStudio::LoadFileTree {
                    root: "makepad".to_string(),
                });
                self.data.studio = Some(studio);
                self.set_status(cx, "connected to backend");
            }
            Err(err) => {
                self.set_status(cx, &format!("backend startup failed: {}", err));
            }
        }
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn set_current_file_label(&self, cx: &mut Cx, path: Option<&str>) {
        let label = path.unwrap_or("No file");
        self.ui
            .label(cx, ids!(current_file_label))
            .set_text(cx, label);
    }

    fn send_studio(&mut self, msg: UIToStudio) -> Option<QueryId> {
        self.data.studio.as_mut().map(|studio| studio.send(msg))
    }

    fn drain_studio_messages(&mut self, cx: &mut Cx) {
        loop {
            let Some(msg) = self.data.studio.as_ref().and_then(|studio| studio.try_recv()) else {
                break;
            };
            self.handle_studio_message(cx, msg);
        }
    }

    fn handle_studio_message(&mut self, cx: &mut Cx, msg: StudioToUI) {
        match msg {
            StudioToUI::FileTree { root, data } => {
                if root == "makepad" {
                    self.data.file_tree.rebuild(data);
                    self.ui.widget(cx, ids!(file_tree)).redraw(cx);
                    self.ui
                        .desktop_file_tree(cx, ids!(file_tree))
                        .set_folder_is_open(cx, LiveId::from_str("makepad"), true, Animate::No);
                    self.set_status(cx, "file tree loaded");
                }
            }
            StudioToUI::TextFileOpened { path, content, .. } => {
                self.data.pending_open_paths.remove(&path);
                if let Some((tab_id, _)) = self.ensure_editor_tab_for_path(cx, &path, false) {
                    self.data.sessions.insert(
                        tab_id,
                        CodeSession::new(CodeDocument::new(content.into(), DecorationSet::new())),
                    );
                    self.ui.dock(cx, ids!(dock)).redraw_tab(cx, tab_id);
                }
                self.set_status(cx, "opened file");
            }
            StudioToUI::TextFileRead { path, content } => {
                self.data.pending_open_paths.remove(&path);
                if let Some((tab_id, _)) = self.ensure_editor_tab_for_path(cx, &path, false) {
                    self.data.sessions.insert(
                        tab_id,
                        CodeSession::new(CodeDocument::new(content.into(), DecorationSet::new())),
                    );
                    self.ui.dock(cx, ids!(dock)).redraw_tab(cx, tab_id);
                }
            }
            StudioToUI::TextFileSaved { path, result } => {
                self.set_status(cx, &format!("saved {} ({:?})", path, result));
            }
            StudioToUI::Error { message } => {
                self.set_status(cx, &format!("error: {}", message));
            }
            _ => {}
        }
    }

    fn tab_id_from_widget_uid(cx: &Cx, widget_uid: WidgetUid) -> LiveId {
        let path = cx.widget_tree().path_to(widget_uid);
        path.get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(editor_first))
    }

    fn set_active_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() {
            self.data.current_file_path = Some(path.clone());
            self.set_current_file_label(cx, Some(&path));
        } else {
            self.data.current_file_path = None;
            self.set_current_file_label(cx, None);
        }
    }

    fn ensure_editor_tab_for_path(
        &mut self,
        cx: &mut Cx,
        path: &str,
        select: bool,
    ) -> Option<(LiveId, bool)> {
        let dock = self.ui.dock(cx, ids!(dock));

        if let Some(tab_id) = self.data.path_to_tab.get(path).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                if select {
                    dock.select_tab(cx, tab_id);
                    self.set_active_tab(cx, tab_id);
                }
                return Some((tab_id, true));
            }
            self.data.path_to_tab.remove(path);
            self.data.tab_to_path.remove(&tab_id);
            self.data.sessions.remove(&tab_id);
        }

        let tab_id = if self.data.path_to_tab.is_empty() {
            let Some(anchor_tab_id) = self.find_editor_anchor_tab(cx, &dock) else {
                return None;
            };
            anchor_tab_id
        } else {
            let Some(anchor_tab_id) = self.find_editor_anchor_tab(cx, &dock) else {
                return None;
            };
            let (tab_bar, pos) = dock.find_tab_bar_of_tab(anchor_tab_id)?;
            let tab_id = dock.unique_id(LiveId::from_str(path).0);
            let created = if select {
                dock.create_and_select_tab(
                    cx,
                    tab_bar,
                    tab_id,
                    id!(CodeEditorPane),
                    String::new(),
                    id!(CloseableTab),
                    Some(pos),
                )
            } else {
                dock.create_tab(
                    cx,
                    tab_bar,
                    tab_id,
                    id!(CodeEditorPane),
                    String::new(),
                    id!(CloseableTab),
                    Some(pos),
                )
            };
            if created.is_none() {
                return None;
            }
            tab_id
        };

        self.data.path_to_tab.insert(path.to_string(), tab_id);
        self.data.tab_to_path.insert(tab_id, path.to_string());
        self.update_editor_tab_titles(cx);

        if select {
            dock.select_tab(cx, tab_id);
            self.set_active_tab(cx, tab_id);
        }

        Some((tab_id, false))
    }

    fn find_editor_anchor_tab(&self, cx: &Cx, dock: &DockRef) -> Option<LiveId> {
        if let Some(tab_id) = self.visible_editor_tab_id(cx) {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                return Some(tab_id);
            }
        }

        if dock.find_tab_bar_of_tab(id!(editor_first)).is_some() {
            return Some(id!(editor_first));
        }
        if dock.find_tab_bar_of_tab(id!(editor_tab)).is_some() {
            return Some(id!(editor_tab));
        }
        for tab_id in self.data.tab_to_path.keys() {
            if dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }

    fn visible_editor_tab_id(&self, cx: &Cx) -> Option<LiveId> {
        let uid = self.ui.widget(cx, ids!(code_editor)).widget_uid();
        let path = cx.widget_tree().path_to(uid);
        path.get(path.len().wrapping_sub(2)).copied()
    }

    fn close_editor_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if !self.data.tab_to_path.contains_key(&tab_id) {
            return;
        }
        if let Some(path) = self.data.tab_to_path.remove(&tab_id) {
            self.data.path_to_tab.remove(&path);
            self.data.sessions.remove(&tab_id);
            self.data.pending_open_paths.remove(&path);
            if self.data.current_file_path.as_deref() == Some(path.as_str()) {
                self.data.current_file_path = None;
                self.set_current_file_label(cx, None);
            }
        }
        self.ui.dock(cx, ids!(dock)).close_tab(cx, tab_id);
        self.update_editor_tab_titles(cx);
    }

    fn update_editor_tab_titles(&mut self, cx: &mut Cx) {
        let dock = self.ui.dock(cx, ids!(dock));
        if self.data.tab_to_path.is_empty() {
            dock.set_tab_title(cx, id!(editor_first), "Editor".to_string());
            dock.set_tab_title(cx, id!(editor_tab), "Editor".to_string());
            return;
        }

        let mut parts_by_tab: HashMap<LiveId, Vec<String>> = HashMap::new();
        let mut depth_by_tab: HashMap<LiveId, usize> = HashMap::new();
        for (tab_id, path) in &self.data.tab_to_path {
            let mut parts: Vec<String> = path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string())
                .collect();
            if parts.is_empty() {
                parts.push(path.clone());
            }
            parts_by_tab.insert(*tab_id, parts);
            depth_by_tab.insert(*tab_id, 1);
        }

        loop {
            let mut title_to_tabs: HashMap<String, Vec<LiveId>> = HashMap::new();
            for (tab_id, parts) in &parts_by_tab {
                let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
                title_to_tabs
                    .entry(Self::title_suffix(parts, depth))
                    .or_default()
                    .push(*tab_id);
            }

            let mut changed = false;
            for tabs in title_to_tabs.values() {
                if tabs.len() <= 1 {
                    continue;
                }
                let expandable = tabs.iter().any(|tab_id| {
                    let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
                    let part_count = parts_by_tab.get(tab_id).map_or(1, |parts| parts.len());
                    depth < part_count
                });
                if !expandable {
                    continue;
                }
                for tab_id in tabs {
                    let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
                    let part_count = parts_by_tab.get(tab_id).map_or(1, |parts| parts.len());
                    let next = (depth + 1).min(part_count);
                    if next != depth {
                        depth_by_tab.insert(*tab_id, next);
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }

        for (tab_id, parts) in &parts_by_tab {
            let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
            dock.set_tab_title(cx, *tab_id, Self::title_suffix(parts, depth));
        }
    }

    fn title_suffix(parts: &[String], depth: usize) -> String {
        let count = parts.len();
        let take = depth.min(count);
        parts[count - take..].join("/")
    }

    fn open_node_in_editor(&mut self, cx: &mut Cx, node_id: LiveId) {
        if !self.data.file_tree.is_file(node_id) {
            return;
        }
        let Some(path) = self.data.file_tree.path_for(node_id).map(ToOwned::to_owned) else {
            return;
        };
        let Some((tab_id, already_open)) = self.ensure_editor_tab_for_path(cx, &path, true) else {
            self.set_status(cx, "failed to create editor tab");
            return;
        };
        if already_open && self.data.sessions.contains_key(&tab_id) {
            self.set_status(cx, "focused open file");
            return;
        }
        if self.data.pending_open_paths.contains(&path) {
            self.set_status(cx, "opening...");
            return;
        }
        self.set_status(cx, &format!("opening {}", path));
        self.data.pending_open_paths.insert(path.clone());
        let _ = self.send_studio(UIToStudio::OpenTextFile { path });
    }

    fn save_tab_file(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() else {
            return;
        };
        let Some(session) = self.data.sessions.get(&tab_id) else {
            return;
        };
        let content = session.document().as_text().to_string();
        let _ = self.send_studio(UIToStudio::SaveTextFile { path, content });
        self.set_status(cx, "saving...");
    }

    fn load_state(&mut self, cx: &mut Cx, slot: usize) {
        let Ok(contents) = std::fs::read_to_string(format!("makepad_state{}.ron", slot)) else {
            return;
        };
        let (dock_items, tab_id_to_path) = match DesktopStateRon::deserialize_ron(&contents) {
            Ok(state) => (state.dock_items, state.tab_id_to_path),
            Err(_) => match DesktopStateRonLegacy::deserialize_ron(&contents) {
                Ok(state) => (state.dock_items, HashMap::new()),
                Err(_) => return,
            },
        };
        self.ui.dock(cx, ids!(dock)).load_state(cx, dock_items);

        let dock = self.ui.dock(cx, ids!(dock));

        self.data.path_to_tab.clear();
        self.data.tab_to_path = tab_id_to_path;
        self.data
            .tab_to_path
            .retain(|tab_id, _| dock.find_tab_bar_of_tab(*tab_id).is_some());
        for (tab_id, path) in &self.data.tab_to_path {
            self.data.path_to_tab.insert(path.clone(), *tab_id);
        }
        self.data.sessions.clear();
        self.data.pending_open_paths.clear();
        self.update_editor_tab_titles(cx);

        let paths: Vec<String> = self.data.tab_to_path.values().cloned().collect();
        for path in paths {
            self.data.pending_open_paths.insert(path.clone());
            let _ = self.send_studio(UIToStudio::OpenTextFile { path });
        }
    }

    fn save_state(&self, cx: &Cx, slot: usize) {
        let Some(dock_items) = self.ui.dock(cx, ids!(dock)).clone_state() else {
            return;
        };
        let state = DesktopStateRon {
            dock_items,
            tab_id_to_path: self.data.tab_to_path.clone(),
        };
        let saved = state.serialize_ron();
        if let Ok(mut file) = File::create(format!("makepad_state{}.ron", slot)) {
            let _ = file.write_all(saved.as_bytes());
        }
    }
}

#[derive(Default)]
pub struct AppData {
    pub studio: Option<StudioConnection>,
    pub file_tree: FlatFileTree,
    pub sessions: HashMap<LiveId, CodeSession>,
    pub path_to_tab: HashMap<String, LiveId>,
    pub tab_to_path: HashMap<LiveId, String>,
    pub pending_open_paths: HashSet<String>,
    pub current_file_path: Option<String>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    pub ui: WidgetRef,
    #[rust]
    pub data: AppData,
    #[rust]
    pub poll_timer: Timer,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.poll_timer = cx.start_interval(0.05);
        self.start_backend(cx);
        self.load_state(cx, 0);
        self.set_current_file_label(cx, None);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(node_id) = self
            .ui
            .desktop_file_tree(cx, ids!(file_tree))
            .file_clicked(actions)
        {
            self.open_node_in_editor(cx, node_id);
        }

        for action in actions {
            if let Some(action) = action.as_widget_action() {
                match action.cast() {
                    DockAction::TabWasPressed(tab_id) => self.set_active_tab(cx, tab_id),
                    DockAction::TabCloseWasPressed(tab_id) => self.close_editor_tab(cx, tab_id),
                    DockAction::SplitPanelChanged { .. }
                    | DockAction::ShouldTabStartDrag(_)
                    | DockAction::Drag(_)
                    | DockAction::Drop(_)
                    | DockAction::None => {}
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

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.poll_timer.is_timer(event).is_some() {
            self.drain_studio_messages(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.data));

        self.drain_studio_messages(cx);

        if self.ui.dock(cx, ids!(dock)).check_and_clear_need_save() {
            self.save_state(cx, 0);
        }
    }
}

#[derive(Clone)]
struct FlatNode {
    id: LiveId,
    path: String,
    name: String,
    node_type: FileNodeType,
    git_status: GitStatus,
    children: Vec<LiveId>,
}

#[derive(Default)]
pub struct FlatFileTree {
    nodes: HashMap<LiveId, FlatNode>,
    roots: Vec<LiveId>,
    path_to_id: HashMap<String, LiveId>,
}

impl FlatFileTree {
    pub fn rebuild(&mut self, data: FileTreeData) {
        self.nodes.clear();
        self.roots.clear();
        self.path_to_id.clear();

        for node in data.nodes {
            let id = LiveId::from_str(&node.path);
            self.path_to_id.insert(node.path.clone(), id);
            self.nodes.insert(
                id,
                FlatNode {
                    id,
                    path: node.path,
                    name: node.name,
                    node_type: node.node_type,
                    git_status: node.git_status,
                    children: Vec::new(),
                },
            );
        }

        let ids: Vec<LiveId> = self.nodes.keys().copied().collect();
        for id in &ids {
            let Some(path) = self.nodes.get(id).map(|node| node.path.clone()) else {
                continue;
            };
            let Some((parent_path, _)) = path.rsplit_once('/') else {
                self.roots.push(*id);
                continue;
            };
            if let Some(parent_id) = self.path_to_id.get(parent_path).copied() {
                if let Some(parent) = self.nodes.get_mut(&parent_id) {
                    parent.children.push(*id);
                }
            } else {
                self.roots.push(*id);
            }
        }

        let mut sort_meta = HashMap::<LiveId, (bool, String)>::new();
        for (id, node) in &self.nodes {
            sort_meta.insert(*id, (matches!(node.node_type, FileNodeType::Dir), node.name.clone()));
        }

        for node in self.nodes.values_mut() {
            node.children.sort_by(|a, b| {
                let Some((a_dir, a_name)) = sort_meta.get(a) else {
                    return std::cmp::Ordering::Equal;
                };
                let Some((b_dir, b_name)) = sort_meta.get(b) else {
                    return std::cmp::Ordering::Equal;
                };
                match b_dir.cmp(a_dir) {
                    std::cmp::Ordering::Equal => a_name.cmp(b_name),
                    other => other,
                }
            });
        }

        self.roots.sort_by(|a, b| {
            let Some((a_dir, a_name)) = sort_meta.get(a) else {
                return std::cmp::Ordering::Equal;
            };
            let Some((b_dir, b_name)) = sort_meta.get(b) else {
                return std::cmp::Ordering::Equal;
            };
            match b_dir.cmp(a_dir) {
                std::cmp::Ordering::Equal => a_name.cmp(b_name),
                other => other,
            }
        });
    }

    pub fn draw(&self, cx: &mut Cx2d, file_tree: &mut FileTree) {
        for root_id in &self.roots {
            self.draw_node(cx, file_tree, *root_id);
        }
    }

    pub fn is_file(&self, node_id: LiveId) -> bool {
        self.nodes
            .get(&node_id)
            .is_some_and(|node| matches!(node.node_type, FileNodeType::File))
    }

    pub fn path_for(&self, node_id: LiveId) -> Option<&str> {
        self.nodes.get(&node_id).map(|node| node.path.as_str())
    }

    fn draw_node(&self, cx: &mut Cx2d, file_tree: &mut FileTree, node_id: LiveId) {
        let Some(node) = self.nodes.get(&node_id) else {
            return;
        };
        let status = git_status_dot(node.git_status);

        if matches!(node.node_type, FileNodeType::Dir) {
            if file_tree
                .begin_folder_with_status(cx, node.id, &node.name, status)
                .is_ok()
            {
                for child_id in &node.children {
                    self.draw_node(cx, file_tree, *child_id);
                }
                file_tree.end_folder();
            }
        } else {
            file_tree.file_with_status(cx, node.id, &node.name, status);
        }
    }
}

fn git_status_dot(status: GitStatus) -> GitStatusDotKind {
    match status {
        GitStatus::Added | GitStatus::Untracked => GitStatusDotKind::New,
        GitStatus::Modified | GitStatus::Staged => GitStatusDotKind::Modified,
        GitStatus::Deleted => GitStatusDotKind::Deleted,
        GitStatus::Conflict => GitStatusDotKind::Mixed,
        GitStatus::Clean | GitStatus::Ignored | GitStatus::Unknown => GitStatusDotKind::None,
    }
}

#[derive(SerRon, DeRon)]
struct DesktopStateRon {
    dock_items: HashMap<LiveId, DockItem>,
    tab_id_to_path: HashMap<LiveId, String>,
}

#[derive(DeRon)]
struct DesktopStateRonLegacy {
    dock_items: HashMap<LiveId, DockItem>,
}
