# Studio

An environment for watching and directing AI work through live terminals,
code editors, system designs and observed process activity.

Use the two mode icons in the toolbar: **Structured** for project files and
docked panes, or **Canvas** for the giant workspace. The active icon is highlighted.
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

Both modes render the same live terminal and editor widgets, so switching preserves
their sessions, output and unsaved edits. The canvas supports 0.001%–800% zoom,
an interactive minimap, relationship lines and compact cards at overview scale.
Nodes use Flow's card, shadow and grid renderer with tighter 4-point corners, colored type
icons and curved relationship lines. The navigator keeps a fixed-size map of
the whole workspace; its shaded window rectangle moves and resizes as you pan
and zoom. Map bounds change only when workspace contents or their layout change.

Choose **Auto layout** to place new cards near their parents without shuffling
existing cards. System children use separate design, terminal, code and run
columns. Drag the lower-right grip to resize terminals, editors and other cards
in either layout. Auto moves overlapping cards down without changing their lane;
shrinking a card keeps the remaining arrangement stable. Choose **Free layout**
to drag card headers. Auto and Free keep independent positions and sizes. Delegates of an agent/terminal stack
vertically beside it; owned code/design and run/app cards use adjacent columns. The mode, camera, arrangements, open
file paths and design cards are saved between launches.

Drag the background, Space-drag or middle-drag to pan. Scrolling over the canvas,
node frames or navigator zooms; live terminal/editor content scrolls normally.
Ctrl/Command-scroll zooms there too. The −/+ buttons also zoom;
**Fit** frames the workspace. Click or drag the
minimap to navigate. Double-click a card header to focus it; a design with a
linked source path opens that file. Terminals and code editors keep their live content down to 12% zoom; below
that they become compact summaries. Zoom into terminals and editors to interact
with their actual content.

The compact top toolbar provides terminal, code, activity, disk and settings
actions with tooltips. The code icon reveals a collapsible source-path row;
Enter opens an absolute or project-relative path. Save and Discard become
available when the active editor needs them. Settings, disk, activity diagnostics
and usage details open in a separate utility panel, outside both workspace
presentations. Close the panel with ×, Escape or a click outside it; the canvas
camera and active work item are retained. Older saved utility tabs are removed
from the workspace on restore.

Close a canvas item with its header ×, or its Structured tab close button.
Closing a terminal ends its live shell.

**Open code** accepts an absolute path through F10. Up to 32 UTF-8 files of at most 2 MiB
each can stay open. Clean editors apply observed disk changes incrementally;
external changes to a dirty buffer produce a conflict and retain local text.
Saves run in the background and check for changed disk content. Dirty buffers
remain unsaved until explicitly saved or discarded. Studio keeps the window
open when a close would lose those edits; use Save or Discard edits first.

On macOS and Linux, a bounded worker samples Studio's descendant processes and
Git source metadata every two seconds. New observed source changes can open
live code cards without moving the camera or taking focus. Discovery watches
up to 512 source paths; changes already present at startup form the baseline.
Only processes beneath live Studio terminals become workspace cards; quota
pollers, inventory helpers and their children stay out of the graph.
Process cards link through observed parent PIDs to their live terminals, where
command output remains available. The Activity view reports observation limits.

Build from the repository workspace:

```sh
cargo build --release -p makepad-studio
./target/release/studio
```

The window manager's Studio launcher builds this same package from `apps/studio`.
For an owned inspection instance, add `--remote` and follow
[the app remote guide](../../docs/agents/app-remote.md).

`--cwd <absolute-directory>` sets the initial terminal/repository context.
`--size 640x600` sets a standalone window size; compact windows keep the same icon toolbar above the workspace.
`--state-dir <directory>` overrides the normal `~/.makepad/studio` settings and
layout location. Standalone appearance is selected in Settings; hosted appearance
follows the window manager.

On macOS, **Follow host OS** follows both the style family and the system's
light/dark appearance, including changes while Studio is open. The Dark checkbox
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
The second line shows Fable's session reset time and both weekly reset dates.
Refresh requests a fresh reading; automatic polling runs every 2 minutes,
with 10-minute backoff after errors.
Click either provider for reset times, freshness and the signed-in account email
when available. `studio.status` and `studio.inspect_usage` expose each provider's
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
33 Studio tools operate the same terminal, code, canvas, appearance, usage and disk
state as the UI, including adding system design cards with source links. The
`dock_tab`, `refresh_project_tree` and `reveal_project_file` tools operate this
same project browser and Dock; `resize_card` resizes Canvas frames. The tool console accepts `/help` and `/studio.status {}`. Terminal commands execute
in the live shell; closing a terminal or discarding code edits is a destructive
tool action.

Activity coverage is observational: brief processes can fall between samples,
and an exited process has an unknown result. App/test processes currently show
status and terminal output; embedded app windows and structured test results
are future work. Provider-internal activity and edit authorship are not inferred.
Design cards are supplied by the user or assistant; automatic Rust architecture
discovery and pinch gestures are not implemented.

Validation:

```sh
cargo test --release -p makepad-studio
```

Tests cover typed actions, stable Auto/Free arrangements and restoration,
canvas input transforms, document revisions/conflicts and bounded saves,
process/source observations, provider quota parsing and CLI shutdown, disk
discovery and style registration. Runtime
acceptance additionally checks live interaction, mode switches, navigation,
external code changes, F10 and graceful shutdown.
