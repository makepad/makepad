//! The workspace around the code-chat: a dock with the file tree and
//! project search on the left, editor tabs in the middle, terminals under
//! them and the chat on the right. `shell` owns the dock and routes
//! `IdeAction`s between the panels; each panel is its own widget:
//!
//! - `editor`: one editable file per tab (save, dirty mark, find, go to
//!   line, reload from disk, the agent's edits lit up);
//! - `files`: the searchable tree (lazy, git-ignore aware, new / rename /
//!   delete, reveal) and the agent crate's `index`, the one list of the repo's files that
//!   quick-open (`quick_open`) and the chat's @-search rank;
//! - `search`: project-wide text search with `makepad-regex`;
//! - `terminal`: PTY terminals in the project folder;
//! - `run`: build and run the repo's apps (a sidebar section), with
//!   `build_output` the BUILD tab in the centre; errors open in the editor;
//! - `sessions`: the chat sessions list (a sidebar section); chats open
//!   as centre tabs beside the files.
//!
//! Panels never reach into each other: they emit an `IdeAction` and the
//! shell does the rest.

pub mod build_output;
pub mod editor;
pub mod files;
pub mod quick_open;
pub mod run;
pub mod search;
pub mod sessions;
pub mod shell;
pub mod terminal;

use makepad_code_agent::event::AgentConfig;
use makepad_code_agent::index;
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// What a panel asks of the workspace. Paths are relative to the project
/// root with `/` separators; lines and columns are 1-based.
#[derive(Clone, Debug, Default)]
pub enum IdeAction {
    /// Open (or select) the file's editor tab, at a line when given.
    OpenFile { path: String, line: Option<u32>, col: Option<u32> },
    /// Attach the file to the chat as a reference for the agent.
    AddReference(String),
    /// A file or folder was created, renamed or deleted from the tree:
    /// open editors follow (a rename retitles the tab, a delete closes a
    /// clean one) and the index refreshes.
    Created(String),
    Renamed { from: String, to: String },
    Deleted(String),
    /// The agent changed a file: an open editor reloads it and lights up
    /// these 1-based line ranges (inclusive) for a moment.
    AgentEdited { path: String, lines: Vec<(u32, u32)> },
    /// Open a chat tab: resume this saved session, or start a new one.
    OpenChat { session: Option<String> },
    /// A chat tab's title changed (its first prompt, or the resumed
    /// session's title); the shell retitles the tab it came from.
    ChatTitle(String),
    /// A chat's conversation was saved under this session id.
    ChatSession(String),
    /// A chat saved its session: the sessions list re-reads the store.
    SessionsChanged,
    /// A build started (from the RUN section or the agent): the shell shows
    /// the BUILD tab in the centre.
    ShowBuild,
    #[default]
    None,
}

/// The one shared state of the window, a `Cx` global set at startup before
/// the first draw: where the project is, how the agent runs, and the file
/// index every picker ranks.
pub struct Workspace {
    pub root: PathBuf,
    pub config: AgentConfig,
    pub index: index::FileIndex,
}

impl Workspace {
    pub fn install(cx: &mut Cx, config: AgentConfig) {
        let index = index::FileIndex::start(&config.root, std::sync::Arc::new(|| SignalToUI::set_ui_signal()));
        cx.set_global(Workspace { root: config.root.clone(), config, index });
    }

    /// The workspace, once `install` ran.
    pub fn of(cx: &Cx) -> Option<&Workspace> {
        cx.get_global_ref::<Workspace>()
    }

    pub fn absolute(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// `path` relative to the root with `/` separators, if it is inside it.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }
}

pub fn script_mod(vm: &mut ScriptVm) {
    editor::script_mod(vm);
    sessions::script_mod(vm);
    build_output::script_mod(vm);
    files::script_mod(vm);
    search::script_mod(vm);
    terminal::script_mod(vm);
    run::script_mod(vm);
    quick_open::script_mod(vm);
    shell::script_mod(vm);
}
