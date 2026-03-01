use crate::{
    makepad_code_editor::CodeSession,
    makepad_studio_backend::StudioConnection,
    makepad_widgets::{file_tree::GitStatusDotKind, *},
};
use makepad_studio_protocol::backend_protocol::{
    EventSample, FileNodeType, FileTreeData, GCSample, GPUSample, GitStatus, LogSource, QueryId,
    RunnableBuild,
};
use makepad_studio_protocol::LogLevel;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

#[derive(Clone)]
pub struct RunTabState {
    pub mount: String,
    pub package: String,
    pub build_id: QueryId,
    pub status: String,
    pub window_id: Option<usize>,
}

#[derive(Clone)]
pub struct LogTabState {
    pub mount: String,
    pub build_id: QueryId,
}

#[derive(Clone)]
pub struct ProfilerTabState {
    pub mount: String,
    pub build_id: QueryId,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct UiLogLocation {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

impl UiLogLocation {
    pub fn display_label(&self) -> String {
        format!("{}:{}:{}", self.path, self.line, self.column)
    }
}

#[derive(Clone, Debug)]
pub struct UiLogEntry {
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub location: Option<UiLogLocation>,
}

#[derive(Clone, Debug, Default)]
pub struct UiProfilerSamples {
    pub event_samples: Vec<EventSample>,
    pub gpu_samples: Vec<GPUSample>,
    pub gc_samples: Vec<GCSample>,
    pub total_in_window: usize,
}

pub struct MountState {
    pub root: PathBuf,
    pub tab_id: Option<LiveId>,
    pub file_tree_data: Option<FileTreeData>,
    pub runnable_builds: Vec<RunnableBuild>,
    pub log_entries: VecDeque<UiLogEntry>,
    pub terminal_files: Vec<String>,
    pub terminals_initialized: bool,
    pub select_last_terminal_once: bool,
    pub terminal_path_to_tab: HashMap<String, LiveId>,
    pub terminal_tab_to_path: HashMap<LiveId, String>,
    pub file_filter: String,
    pub file_filter_results: Vec<String>,
    pub file_filter_query: Option<QueryId>,
    pub file_filter_pending: bool,
    pub log_filter: String,
    pub log_tail: bool,
}

impl Default for MountState {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            tab_id: None,
            file_tree_data: None,
            runnable_builds: Vec::new(),
            log_entries: VecDeque::new(),
            terminal_files: Vec::new(),
            terminals_initialized: false,
            select_last_terminal_once: false,
            terminal_path_to_tab: HashMap::new(),
            terminal_tab_to_path: HashMap::new(),
            file_filter: String::new(),
            file_filter_results: Vec::new(),
            file_filter_query: None,
            file_filter_pending: false,
            log_filter: String::new(),
            log_tail: true,
        }
    }
}

#[derive(Default)]
pub struct AppData {
    pub studio: Option<StudioConnection>,
    pub mounts: HashMap<String, MountState>,
    pub tab_to_mount: HashMap<LiveId, String>,
    pub active_mount: Option<String>,
    pub file_tree: FlatFileTree,
    pub sessions: HashMap<LiveId, CodeSession>,
    pub path_to_tab: HashMap<String, LiveId>,
    pub tab_to_path: HashMap<LiveId, String>,
    pub pending_open_paths: HashSet<String>,
    pub pending_reload_paths: HashSet<String>,
    pub current_file_path: Option<String>,
    pub run_tab_state: HashMap<LiveId, RunTabState>,
    pub run_tab_by_build: HashMap<QueryId, LiveId>,
    pub log_tab_state: HashMap<LiveId, LogTabState>,
    pub log_tab_by_build: HashMap<QueryId, LiveId>,
    pub profiler_tab_state: HashMap<LiveId, ProfilerTabState>,
    pub profiler_tab_by_build: HashMap<QueryId, LiveId>,
    pub build_log_entries: HashMap<QueryId, VecDeque<UiLogEntry>>,
    pub profiler_samples_by_build: HashMap<QueryId, UiProfilerSamples>,
    pub profiler_running_by_build: HashMap<QueryId, bool>,
    pub profiler_time_start_by_build: HashMap<QueryId, f64>,
    pub build_to_mount: HashMap<QueryId, String>,
    pub build_package: HashMap<QueryId, String>,
    pub active_log_build_by_mount: HashMap<String, QueryId>,
    pub live_log_query: Option<QueryId>,
    pub live_profiler_query_by_build: HashMap<QueryId, QueryId>,
    pub profiler_query_build_by_query: HashMap<QueryId, QueryId>,
    pub terminal_stream_by_path: HashMap<String, Vec<u8>>,
    pub terminal_history_len_by_path: HashMap<String, usize>,
    pub terminal_desired_size_by_path: HashMap<String, (u16, u16)>,
    pub terminal_open_paths: HashSet<String>,
    pub file_filter_mount_by_query: HashMap<QueryId, String>,
    pub pending_stop_all_mount: Option<String>,
    pub pending_log_jumps: HashMap<String, (usize, usize)>,
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
    pub fn rebuild(&mut self, data: &FileTreeData) {
        self.nodes.clear();
        self.roots.clear();
        self.path_to_id.clear();

        for node in &data.nodes {
            let id = LiveId::from_str(&node.path);
            self.path_to_id.insert(node.path.clone(), id);
            self.nodes.insert(
                id,
                FlatNode {
                    id,
                    path: node.path.clone(),
                    name: node.name.clone(),
                    node_type: node.node_type.clone(),
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
            sort_meta.insert(
                *id,
                (
                    matches!(node.node_type, FileNodeType::Dir),
                    node.name.clone(),
                ),
            );
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

    pub fn git_status_dot_for_path(&self, path: &str) -> GitStatusDotKind {
        self.path_to_id
            .get(path)
            .and_then(|id| self.nodes.get(id))
            .map(|node| git_status_dot(node.git_status))
            .unwrap_or(GitStatusDotKind::None)
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
