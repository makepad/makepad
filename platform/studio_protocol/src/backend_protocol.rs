use makepad_live_id::LiveId;
use makepad_micro_serde::*;
use makepad_error_log::LogLevel;
use makepad_platform::os::shared_framebuf::PresentableDraw;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin, SerJson, DeJson)]
pub struct ClientId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin, SerJson, DeJson)]
pub struct QueryId(pub u64);

impl QueryId {
    pub fn new(client_id: ClientId, counter: u64) -> Self {
        QueryId((client_id.0 as u64) << 48 | (counter & 0x0000_FFFF_FFFF_FFFF))
    }

    pub fn client_id(self) -> ClientId {
        ClientId((self.0 >> 48) as u16)
    }

    pub fn counter(self) -> u64 {
        self.0 & 0x0000_FFFF_FFFF_FFFF
    }
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct UIToStudioEnvelope {
    pub query_id: QueryId,
    pub msg: UIToStudio,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum UIToStudio {
    // === File System ===
    LoadFileTree {
        mount: String,
    },
    OpenTextFile {
        path: String,
    },
    SaveTextFile {
        path: String,
        content: String,
    },
    DeleteFile {
        path: String,
    },
    ReadTextFile {
        path: String,
    },
    FindFiles {
        mount: Option<String>,
        pattern: String,
        is_regex: Option<bool>,
        max_results: Option<usize>,
    },

    // === Mount & Branch Management ===
    Mount {
        name: String,
        path: String,
    },
    Unmount {
        name: String,
    },
    CreateBranch {
        mount: String,
        name: String,
        from_ref: Option<String>,
    },
    DeleteBranch {
        mount: String,
        name: String,
    },
    GitLog {
        mount: String,
        max_count: Option<usize>,
    },

    // === Build Control ===
    ListBuilds,
    LoadRunnableBuilds {
        mount: String,
    },
    CargoRun {
        mount: String,
        args: Vec<String>,
        startup_query: Option<String>,
        env: Option<HashMap<String, String>>,
        buildbox: Option<String>,
    },
    StopBuild {
        build_id: QueryId,
    },

    // === App Interaction (opaque payload for now) ===
    ForwardToApp {
        build_id: QueryId,
        msg_bin: Vec<u8>,
    },
    TypeText {
        build_id: QueryId,
        text: String,
    },
    Return {
        build_id: QueryId,
        auto_dump: Option<bool>,
    },
    Click {
        build_id: QueryId,
        x: i64,
        y: i64,
    },
    Screenshot {
        build_id: QueryId,
        kind_id: Option<u32>,
    },
    WidgetTreeDump {
        build_id: QueryId,
    },
    WidgetQuery {
        build_id: QueryId,
        query: String,
    },
    RunViewInput {
        build_id: QueryId,
        window_id: usize,
        msg_bin: Vec<u8>,
    },
    RunViewResize {
        build_id: QueryId,
        window_id: usize,
        width: f64,
        height: f64,
        dpi: f64,
    },

    // === Terminal ===
    TerminalOpen {
        path: String,
        cols: u16,
        rows: u16,
        env: HashMap<String, String>,
    },
    TerminalInput {
        path: String,
        data: Vec<u8>,
    },
    TerminalResize {
        path: String,
        cols: u16,
        rows: u16,
    },
    TerminalClose {
        path: String,
    },

    // === Search & Query ===
    SearchFiles {
        mount: Option<String>,
        pattern: String,
        is_regex: Option<bool>,
        glob: Option<String>,
        max_results: Option<usize>,
    },
    QueryLogs {
        build_id: Option<QueryId>,
        level: Option<String>,
        source: Option<LogSource>,
        file: Option<String>,
        pattern: Option<String>,
        is_regex: Option<bool>,
        since_index: Option<usize>,
        live: Option<bool>,
    },
    QueryProfiler {
        build_id: Option<QueryId>,
        sample_type: Option<LiveId>,
        time_start: Option<f64>,
        time_end: Option<f64>,
        max_samples: Option<usize>,
        live: Option<bool>,
    },
    CancelQuery {
        query_id: QueryId,
    },

    // === BuildBox Management ===
    ListBuildBoxes,
    BuildBoxSyncNow {
        name: String,
    },

    // === Script CI ===
    RunScriptTask {
        script_path: String,
    },
    StopScriptTask {
        task_id: QueryId,
    },
    ListScriptTasks,

    // === Log ===
    LogClear,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum StudioToUI {
    // === Connection ===
    Hello {
        client_id: ClientId,
    },

    // === File System ===
    FileTree {
        mount: String,
        data: FileTreeData,
    },
    FileTreeDiff {
        mount: String,
        changes: Vec<FileTreeChange>,
    },
    TextFileOpened {
        path: String,
        content: String,
        git_status: GitStatus,
    },
    TextFileRead {
        path: String,
        content: String,
    },
    TextFileSaved {
        path: String,
        result: SaveResult,
    },
    FileChanged {
        path: String,
    },
    FindFileResults {
        query_id: QueryId,
        paths: Vec<String>,
        done: bool,
    },
    GitLog {
        mount: String,
        log: GitLog,
    },

    // === Build ===
    Builds {
        builds: Vec<BuildInfo>,
    },
    RunnableBuilds {
        mount: String,
        builds: Vec<RunnableBuild>,
    },
    BuildStarted {
        build_id: QueryId,
        mount: String,
        package: String,
    },
    BuildStopped {
        build_id: QueryId,
        exit_code: Option<i32>,
    },

    // === App Interaction ===
    Screenshot {
        query_id: QueryId,
        build_id: QueryId,
        kind_id: u32,
        path: String,
        width: u32,
        height: u32,
    },
    WidgetTreeDump {
        query_id: QueryId,
        build_id: QueryId,
        dump: String,
    },
    WidgetQuery {
        query_id: QueryId,
        build_id: QueryId,
        query: String,
        rects: Vec<String>,
    },

    // === RunView ===
    RunViewCreated {
        build_id: QueryId,
        window_id: usize,
    },
    RunViewSwapchain {
        build_id: QueryId,
        window_id: usize,
        swapchain_desc: String,
    },
    RunViewFrame {
        build_id: QueryId,
        window_id: usize,
        frame_id: u64,
        width: u32,
        height: u32,
        codec: FrameCodec,
        data: Vec<u8>,
    },
    RunViewDrawComplete {
        build_id: QueryId,
        window_id: usize,
        presentable_draw: PresentableDraw,
    },
    RunViewCursor {
        build_id: QueryId,
        cursor: String,
    },
    RunViewDestroyed {
        build_id: QueryId,
        window_id: usize,
    },

    // === Terminal ===
    TerminalOpened {
        path: String,
        history: Vec<u8>,
        grid: TerminalGrid,
    },
    TerminalOutput {
        path: String,
        data: Vec<u8>,
    },
    TerminalTitle {
        path: String,
        title: String,
    },
    TerminalExited {
        path: String,
        code: i32,
    },

    // === Search & Query ===
    SearchFileResults {
        query_id: QueryId,
        results: Vec<SearchResult>,
        done: bool,
    },
    QueryLogResults {
        query_id: QueryId,
        entries: Vec<(usize, LogEntry)>,
        done: bool,
    },
    QueryProfilerResults {
        query_id: QueryId,
        event_samples: Vec<EventSample>,
        gpu_samples: Vec<GPUSample>,
        gc_samples: Vec<GCSample>,
        total_in_window: usize,
        done: bool,
    },
    QueryCancelled {
        query_id: QueryId,
    },

    // === BuildBoxes ===
    BuildBoxes {
        boxes: Vec<BuildBoxInfo>,
    },
    BuildBoxConnected {
        info: BuildBoxInfo,
    },
    BuildBoxDisconnected {
        name: String,
    },

    // === Script CI ===
    ScriptTasks {
        tasks: Vec<ScriptTaskInfo>,
    },
    ScriptTaskStarted {
        task_id: QueryId,
        script_path: String,
    },
    ScriptTaskOutput {
        task_id: QueryId,
        build_id: Option<QueryId>,
        message: String,
        level: LogLevel,
    },
    ScriptTaskResult {
        task_id: QueryId,
        status: TaskStatus,
        attachments: Vec<Attachment>,
    },

    // === Log ===
    LogCleared,

    // === Error ===
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum FrameCodec {
    ZstdRgba,
    Jpeg,
    Png,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct FileTreeData {
    pub nodes: Vec<FileNode>,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct FileNode {
    pub path: String,
    pub name: String,
    pub node_type: FileNodeType,
    pub git_status: GitStatus,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum FileNodeType {
    File,
    Dir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerBin, DeBin, SerJson, DeJson, Default)]
pub enum GitStatus {
    Clean,
    Modified,
    Staged,
    Added,
    Untracked,
    Deleted,
    Conflict,
    Ignored,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum FileTreeChange {
    Added {
        path: String,
        node_type: FileNodeType,
        git_status: GitStatus,
    },
    Removed {
        path: String,
    },
    Modified {
        path: String,
        git_status: GitStatus,
    },
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum FileError {
    NotFound(String),
    InvalidPath(String),
    Io(String),
    Git(String),
    Other(String),
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum SaveResult {
    Ok,
    Err(FileError),
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct GitLog {
    pub commits: Vec<GitCommitInfo>,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct GitCommitInfo {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct BuildInfo {
    pub build_id: QueryId,
    pub mount: String,
    pub package: String,
    pub active: bool,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct RunnableBuild {
    pub package: String,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct BuildBoxInfo {
    pub name: String,
    pub platform: String,
    pub arch: String,
    pub status: BuildBoxStatus,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum BuildBoxStatus {
    Idle,
    Syncing,
    Building { build_id: QueryId },
    Offline,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct ScriptTaskInfo {
    pub task_id: QueryId,
    pub script_path: String,
    pub status: TaskStatus,
    pub started_at: f64,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum TaskStatus {
    Running,
    Passed,
    Failed { message: String },
    Warned { messages: Vec<String> },
    Cancelled,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct Attachment {
    pub name: String,
    pub data: Vec<u8>,
    pub mime: String,
    pub build_id: Option<QueryId>,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct SearchResult {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub line_text: String,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct LogEntry {
    pub index: usize,
    pub timestamp: f64,
    pub build_id: Option<QueryId>,
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub file_name: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerBin, DeBin, SerJson, DeJson)]
pub enum LogSource {
    Cargo,
    ChildApp,
    BuildBox,
    Studio,
    Terminal,
    ScriptCi,
    Other(LiveId),
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, Default)]
pub struct EventSample {
    pub at: f64,
    pub label: LiveId,
    pub event_u32: u32,
    pub event_meta: u64,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, Default)]
pub struct GPUSample {
    pub at: f64,
    pub label: LiveId,
    pub start: f64,
    pub end: f64,
    pub draw_calls: u64,
    pub instances: u64,
    pub vertices: u64,
    pub instance_bytes: u64,
    pub uniform_bytes: u64,
    pub vertex_buffer_bytes: u64,
    pub texture_bytes: u64,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, Default)]
pub struct GCSample {
    pub at: f64,
    pub label: LiveId,
    pub start: f64,
    pub end: f64,
    pub heap_live: u64,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, Default)]
pub struct TerminalGrid {
    pub cols: u16,
    pub rows: u16,
    pub text: String,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, Default)]
pub struct TerminalCellDiff {
    pub changed: Vec<TerminalCellUpdate>,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, Default)]
pub struct TerminalCellUpdate {
    pub x: u16,
    pub y: u16,
    pub ch: u32,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct FileDelta {
    pub path: String,
    pub kind: DeltaKind,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum DeltaKind {
    Write { content: Vec<u8> },
    Delete,
    MkDir,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct FileHash {
    pub path: String,
    pub size: u64,
    pub mtime_ns: u64,
    pub mode: u32,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub content_blake3: Vec<u8>,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum StudioToBuildBox {
    TreeHash {
        hash: String,
    },
    SyncFiles {
        files: Vec<FileDelta>,
    },
    RequestTreeHash,
    CargoBuild {
        build_id: QueryId,
        mount: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    StopBuild {
        build_id: QueryId,
    },
    Ping,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct StudioToBuildBoxVec(pub Vec<StudioToBuildBox>);

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum BuildBoxToStudio {
    Hello {
        name: String,
        platform: String,
        arch: String,
        tree_hash: String,
    },
    FileHashes {
        files: Vec<FileHash>,
    },
    SyncComplete {
        tree_hash: String,
    },
    SyncError {
        error: String,
    },
    BuildOutput {
        build_id: QueryId,
        line: String,
    },
    BuildStarted {
        build_id: QueryId,
    },
    BuildStopped {
        build_id: QueryId,
        exit_code: Option<i32>,
    },
    Pong,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct BuildBoxToStudioVec(pub Vec<BuildBoxToStudio>);
