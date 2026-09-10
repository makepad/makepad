# Director

An environment for watching and directing AI work through live terminals,
code editors, agent lanes, the cached architecture plans (`arch/`) and the
disk map. The interactive code map is its sibling app, Scope (private, cloned
at `apps/scope`).

Use the four mode icons in the toolbar: **Structured** for project files and
docked panes, **Tasks** for the agent lanes with their terminals,
**Architecture** for the plan of the active file's crate (`arch/<crate>/crate.toml`,
else the platform's: lanes, subsystem cards and their relations; hover reads a
card, click pins it in the side report, drag pans, the wheel with a modifier
zooms, F fits) or **Disk** for the volume map. The active icon is highlighted.
Structured starts with a **Project** file tree on the left and the work Dock on
the right. Expand folders and click files to open their shared editor; the tree
keeps its expansion/selection when switching modes. Refresh and Reveal active
file controls sit above it. Directory reads run in the background and refresh
every two seconds. Dotfiles are visible, `.git` is excluded, symlink folders
remain leaves, and partial/unreadable listings are reported.

Drag a Dock tab to a pane edge to split left/right or top/bottom; drop it in the
center or tab strip to regroup/reorder. Drag project files into panes or onto
their edges to open them there. Visible splitters resize the panes and highlight while hovered or dragged; the Project pane
itself can be repositioned. Dock arrangements persist between launches and appearance changes.
Older saves with detached default panes recover their valid visible layout.
Moving a tab retains its terminal process, code buffer and unsaved edits.

All modes render the same live terminal and editor widgets, so switching preserves
their sessions, output and unsaved edits. **Tasks** shows the iteration lanes:
each lane has its own terminal on the persistent screen host, so an agent keeps
running across Director restarts; the caption tools start Fable or Codex lanes,
open archived lanes, fit all lanes and drive the local → work → dev flow. The
mode is saved between launches.

The compact top toolbar holds the three modes, the disk and workspace
inventory, and Settings. Save with Cmd/Ctrl+S in an editor, or Cmd/Ctrl+Shift+S
for every unsaved document; closing Director lists what is still unsaved. Settings,
disk, activity diagnostics and usage details open in a separate utility panel,
outside the workspace presentations. Close the panel with ×, Escape or a click
outside it; the active work item is retained. Older saved utility tabs are removed
from the workspace on restore.

Close an item with its Structured tab close button. Closing a terminal ends its live shell.

**Open code** accepts an absolute path through F10. Up to 32 UTF-8 files of at most 2 MiB
each can stay open. Clean editors apply observed disk changes incrementally;
external changes to a dirty buffer produce a conflict and retain local text.
Saves run in the background and check for changed disk content. Dirty buffers
remain unsaved until explicitly saved or discarded. Director keeps the window
open when a close would lose those edits; use Save or Discard edits first.

On macOS and Linux, a bounded worker samples Director's descendant processes and
Git source metadata every two seconds. New observed source changes can open
live code editors without taking focus. Discovery watches up to 512 source
paths; changes already present at startup form the baseline. The Activity view
lists the observed processes beneath live Director terminals, the source changes
and the observation limits; quota pollers, inventory helpers and their
children stay out of it.

Build from the repository workspace:

```sh
cargo build --release -p makepad-director
./target/release/director
```

The window manager's launcher builds this same package from `apps/director`.
For an owned inspection instance, add `--remote` and follow
[the app remote guide](../../docs/agents/app-remote.md).

`--cwd <absolute-directory>` sets the initial terminal/repository context.
`--size 640x600` sets a standalone window size; compact windows keep the same icon toolbar above the workspace.
`--state-dir <directory>` overrides the normal `~/.makepad/studio` settings and
layout location (the name is kept so existing lanes and sessions stay found). Standalone appearance is selected in Settings; hosted appearance
follows the window manager.

On macOS, **Follow host OS** follows both the style family and the system's
light/dark appearance, including changes while Director is open. The Dark checkbox
shows the detected appearance and is disabled in this mode. Choose an explicit
style to use the manual light/dark preference. Platforms without a native
appearance query retain the manual light/dark fallback.

The disk worker samples the volume and inventories registered worktrees,
conventional nested build directories and Cargo-configured targets. Discovery
and size scans have time/output budgets; incomplete coverage is reported.
Canonical targets and hard links count once. Allocated/apparent bytes are
estimates, not a guarantee of space reclaimed from filesystem clones/snapshots.
Cleanup currently provides inspection and exact-path previews only. Process
observation does not establish that deleting a workspace is safe.

The bottom status bar shows **Fable** session (`S`) and weekly (`W`) usage,
and **Astra** weekly usage, beside disk space. Percentages show capacity used.
Each provider has a separate compact island on one line: usage, reset time/date
(for example, `Sep 10`) and signed-in account email. The bottom bar stays one
row tall; its provider strip scrolls horizontally on small screens, while
Refresh and disk space stay visible. Hover or click for full account details.
Refresh requests a fresh reading; automatic polling runs every 2 minutes,
with 10-minute backoff after errors.
Click either provider for reset times, freshness and the signed-in account email
when available. `director.status` and `director.inspect_usage` expose each provider's
`account_email` (null when unavailable), refreshed with usage.
Quota windows include `reset_date`, `reset_time` and `reset_timezone` where
reported. Dates without a year keep the provider's wording; absolute resets use UTC.
Missing limits display `—`; old results remain marked stale after a failed query. Spark, reserve and credit
buckets are excluded. Fable's weekly indicator uses the tighter reported
account-wide or Fable-specific allowance.

One background worker owns Fable's persistent CLI terminal and sends `/usage`
only after the input prompt is ready. It dismisses the view between polls and
reuses the terminal; it creates no worktree or build directory. Startup hooks,
MCPs and model tools are disabled for this status-only session. Astra reads the
installed Codex CLI's `account/rateLimits/read` and `account/read` through a private stdio child,
without creating a model conversation. All queries have bounded time/output and
owned-process shutdown. Absolute reset times display in UTC; local or ambiguous
reset strings retain their original wording.
Fable's email comes from `claude auth status --json`; only its email is retained.
Account identity is queried afresh even when quota data is stale, and changing
accounts prevents carrying the previous account's quota sample forward.

F10 opens the standard Makepad assistant using the existing AI settings. Its
Director tools operate the same terminal, code, mode, appearance, usage and disk
state as the UI, including adding system design tabs with source links. The
`dock_tab`, `refresh_project_tree` and `reveal_project_file` tools operate this
same project browser and Dock. The tool console accepts `/help` and `/director.status {}`. Terminal commands execute
in the live shell; closing a terminal or discarding code edits is a destructive
tool action.

Activity coverage is observational: brief processes can fall between samples,
and an exited process has an unknown result. App/test processes currently show
status and terminal output; embedded app windows and structured test results
are future work. Provider-internal activity and edit authorship are not inferred.
Design tabs are supplied by the user or assistant.

Validation:

```sh
cargo test --release -p makepad-director
```

Tests cover typed actions, the persisted workspace mode, zoomable input
transforms, document revisions/conflicts and bounded saves,
process/source observations, provider quota parsing and CLI shutdown, disk
discovery and style registration. Runtime
acceptance additionally checks live interaction, mode switches, navigation,
external code changes, F10 and graceful shutdown.
