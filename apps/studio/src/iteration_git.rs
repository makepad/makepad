//! Synchronous Git operations for Studio's iteration worker, on `makepad_git`
//! only: no git binary is ever spawned. Never call from UI. Private
//! checkpoints are retained locally; public promotion creates one parent.

use makepad_git::{
    diff_blobs, format_unified_diff, hash_file_blob, index::serialize_index, merge3_text,
    merge_trees, refs, stage_file, unstage_file, Commit, DiffOp, FileStatus, GitError, Index,
    IndexEntry, ObjectId, RefTarget, Repository, Signature, TreeChange, TreeMergeEntry,
    WorktreeBranch,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const OWNER_FILE: &str = "studio-iteration-owner";
const RELEASED_DIR: &str = "studio-iteration-released";
const PRIVATE_REFS: &str = "refs/studio/local-checkpoints/";
const MAX_PATHS: usize = 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const HOOK_MARKER: &str = "# Studio local checkpoint pre-push policy v1";
const NEVER_PUSH_REMOTE: &str = "studio-local-never-push";

#[derive(Clone, Debug)]
pub struct RepositoryState {
    pub root: PathBuf,
    pub common_dir: PathBuf,
    pub branch: Option<String>,
    pub head: String,
    pub changes: Vec<ChangedPath>,
    pub worktrees: Vec<WorktreeState>,
}

#[derive(Clone, Debug)]
pub struct ChangedPath {
    pub path: String,
    /// Two-letter porcelain code: index column then worktree column
    /// (`M `, ` M`, `MM`, `A `, `D `, ` D`, `??`).
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct WorktreeState {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: String,
}

#[derive(Clone, Debug)]
pub struct OwnedWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub common_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub commit: String,
    pub tree: String,
    pub parent: String,
    pub branch: String,
}

#[derive(Clone, Debug)]
pub struct PromotionPreview {
    pub repository: PathBuf,
    pub common_dir: PathBuf,
    pub source_branch: String,
    pub target_branch: String,
    pub source_oid: String,
    pub target_oid: String,
    pub result_tree: String,
    pub changed_paths: Vec<String>,
    pub diff_stat: String,
}

#[derive(Clone, Debug)]
pub struct SyncPreview {
    pub repository: PathBuf,
    pub common_dir: PathBuf,
    pub source_ref: String,
    pub target_branch: String,
    pub source_oid: String,
    pub target_oid: String,
    pub result_tree: String,
    pub fast_forward: bool,
    pub unchanged: bool,
    pub diff_stat: String,
}

pub fn is_private_branch(branch: &str) -> bool {
    branch == "local"
        || branch.strip_prefix("local-").is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

pub fn inspect(repository: &Path) -> Result<RepositoryState, String> {
    let mut repo = open(repository)?;
    state_of(&mut repo)
}

/// Verify an existing bounded checkout without changing refs or its owner marker.
pub fn verify_flow_worktree(owned: &OwnedWorktree) -> Result<(), String> {
    if !is_private_branch(&owned.branch) { return Err("Source must remain on a private local branch".into()); }
    verify_owned(owned)
}

/// One directory per active flow, reused for every build. No shared files are staged.
pub fn ensure_flow_worktree(
    repository: &Path,
    destination: &Path,
    private_branch: &str,
    guard_executable: &Path,
) -> Result<OwnedWorktree, String> {
    if !is_private_branch(private_branch) { return Err("A flow branch must be local or local-<flow-id>".into()); }
    ensure_worktree(repository, destination, private_branch, Some(guard_executable))
}

pub fn ensure_local_worktree(
    repository: &Path,
    destination: &Path,
    guard_executable: &Path,
) -> Result<OwnedWorktree, String> {
    ensure_flow_worktree(repository, destination, "local", guard_executable)
}

/// Explicitly create an owned checkout for a public tier, if that branch is free.
/// An existing human checkout of work/dev is never adopted or moved implicitly.
pub fn ensure_promotion_worktree(
    repository: &Path,
    destination: &Path,
    branch: &str,
) -> Result<OwnedWorktree, String> {
    if !matches!(branch, "work" | "dev") { return Err("Promotion targets must be work or dev".into()); }
    ensure_worktree(repository, destination, branch, None)
}

fn ensure_worktree(
    repository: &Path,
    destination: &Path,
    branch: &str,
    guard_executable: Option<&Path>,
) -> Result<OwnedWorktree, String> {
    let mut repo = open(repository)?;
    let state = state_of(&mut repo)?;
    if !destination.is_absolute() { return Err("Worktree destination must be absolute".into()); }
    if destination.exists() {
        let path = fs::canonicalize(destination).map_err(io_error)?;
        let owned = OwnedWorktree { path, branch: branch.into(), common_dir: state.common_dir };
        verify_owned(&owned)?;
        if let Some(guard) = guard_executable { install_push_guard(&state.root, guard)?; }
        return Ok(owned);
    }
    if let Some(existing) = state.worktrees.iter().find(|tree| tree.branch.as_deref() == Some(branch)) {
        return Err(format!("Branch {branch} is already checked out at {}; reuse its owned flow or close it explicitly", existing.path.display()));
    }
    if let Some(guard) = guard_executable { install_push_guard(&state.root, guard)?; }
    let reference = format!("refs/heads/{branch}");
    let exists = ref_exists(&repo, &reference)?;
    let released = released_record(&state.common_dir, branch);
    if exists && is_private_branch(branch) && fs::read_to_string(&released).ok().as_deref() != Some(&released_text(branch, &state.common_dir)) {
        return Err(format!("Existing private branch {branch} has no owned checkout at this location; reconnect its original flow instead of adopting it implicitly"));
    }
    if !exists && !is_private_branch(branch) {
        return Err(format!("Create the {branch} branch explicitly before assigning its worktree"));
    }
    let start = if exists { None } else { Some(resolve(&repo, "refs/heads/work")?) };
    let spec = match &start {
        Some(start) => WorktreeBranch::NewFrom { name: branch, start: *start },
        None => WorktreeBranch::Existing(branch),
    };
    let linked = repo.worktree_add(destination, spec, None)
        .map_err(|error| format!("Cannot add the {branch} worktree at {}: {error}", destination.display()))?;
    let owned = OwnedWorktree {
        path: fs::canonicalize(destination).map_err(io_error)?,
        branch: branch.into(),
        common_dir: state.common_dir,
    };
    write_new(&linked.git_dir.join(OWNER_FILE), owner_text(&owned).as_bytes())?;
    if is_private_branch(branch) {
        // Default git push also fails closed if the hook is unavailable. Explicit
        // --no-verify or changing repository configuration is outside this guard.
        set_branch_push_remote(&owned.common_dir, branch)?;
    }
    if exists { let _ = fs::remove_file(&released); }
    verify_owned(&owned)?;
    Ok(owned)
}

/// Remove an owned flow checkout after its processes and lease are gone. The
/// private branch and every `refs/studio/local-checkpoints/*` ref stay, and a
/// release record lets the same flow reattach to its branch later. Dirty work
/// (including untracked files) is refused unless `force`.
pub fn remove_flow_worktree(owned: &OwnedWorktree, force: bool) -> Result<(), String> {
    if !is_private_branch(&owned.branch) { return Err("Only private flow checkouts can be removed here".into()); }
    verify_owned(owned)?;
    let mut repo = open(&owned.path)?;
    assert_no_operation(&repo)?;
    if !force {
        let changes = changed_paths(&mut repo)?;
        if let Some(first) = changes.first() {
            return Err(format!("Local workspace has {} uncommitted change(s) ({}); checkpoint them or remove with force", changes.len(), first.path));
        }
    }
    let lease = acquire_lease(&repo)?;
    let record = released_record(&owned.common_dir, &owned.branch);
    if let Some(parent) = record.parent() { fs::create_dir_all(parent).map_err(io_error)?; }
    fs::write(&record, released_text(&owned.branch, &owned.common_dir)).map_err(io_error)?;
    let path = owned.path.to_str().ok_or("Worktree path must be UTF-8")?.to_owned();
    drop(lease);
    repo.worktree_remove(&path, force).map_err(|error| format!("Cannot remove the local workspace: {error}"))
}

/// Exact allowlist; all changed files must be included or the build cannot use
/// this worktree. Existing tracked documentation/tests are retained unchanged.
pub fn checkpoint(
    local: &OwnedWorktree,
    expected_head: &str,
    paths: &[String],
    instruction_paths: &[String],
    message: &str,
) -> Result<Checkpoint, String> {
    verify_owned(local)?;
    if !is_private_branch(&local.branch) { return Err("Build checkpoints belong only on a local flow branch".into()); }
    validate_message(message)?;
    let mut repo = open(&local.path)?;
    let _lease = acquire_lease(&repo)?;
    assert_head(&repo, local, expected_head)?;
    assert_no_operation(&repo)?;
    if paths.len() > MAX_PATHS { return Err(format!("A checkpoint accepts at most {MAX_PATHS} explicit files")); }
    let changes = changed_paths(&mut repo)?;
    if changes.iter().any(|change| change.status != "??" && change.status.as_bytes()[0] != b' ') {
        return Err("Checkpoint requires an untouched index; staged changes must be resolved explicitly".into());
    }
    let selected: BTreeSet<_> = paths.iter().cloned().collect();
    if selected.len() != paths.len() { return Err("Checkpoint file list contains duplicates".into()); }
    for change in &changes {
        if !selected.contains(&change.path) {
            return Err(format!("Uncheckpointed change {} would make the build differ from its commit", change.path));
        }
    }
    let current_index = repo.read_index().map_err(git_error)?;
    for path in &selected { validate_checkpoint_path(&repo, &current_index, path, instruction_paths)?; }
    let index_path = repo.git_dir.join("index");
    let mut locked_index = FileLease::create(index_path.with_file_name("index.lock"))?;
    let head_oid = parse_oid(expected_head)?;
    let head_tree = repo.read_commit(&head_oid).map_err(git_error)?.tree;
    let mut index = index_from_tree(&mut repo, &head_tree, &current_index)?;
    for path in &selected {
        if repo.workdir.join(path).is_file() {
            stage_file(&repo.common_dir, &repo.workdir, &mut index, path).map_err(git_error)?;
        } else {
            unstage_file(&mut index, path);
        }
    }
    let tree = repo.index_to_tree(&index).map_err(git_error)?;
    validate_added_source(&mut repo, &head_tree, &tree)?;
    for path in &selected {
        let file = repo.workdir.join(path);
        let entry = index.entries.binary_search_by(|entry| entry.path.as_str().cmp(path.as_str()));
        let stable = match (entry, file.is_file()) {
            (Ok(at), true) => hash_file_blob(&file).map_err(git_error)? == index.entries[at].oid,
            (Err(_), false) => true,
            _ => false,
        };
        if !stable { return Err("Files changed while preparing the checkpoint; retry after the edit finishes".into()); }
    }
    for change in changed_paths(&mut repo)? {
        if !selected.contains(&change.path) {
            return Err(format!("A new change appeared while checkpointing: {}; retry after the edit finishes", change.path));
        }
    }
    assert_head(&repo, local, expected_head)?;
    let full_message = format!("{message}\n\nStudio-Local-Checkpoint: true\n");
    let commit = create_commit(&repo, &tree, &[head_oid], &full_message)?;
    locked_index.write(&serialize_index(&index).map_err(git_error)?)?;
    update_private_ref(&repo, local, &head_oid, &commit)?;
    locked_index.install(&index_path)?;
    Ok(Checkpoint { commit: commit.to_hex(), tree: tree.to_hex(), parent: expected_head.into(), branch: local.branch.clone() })
}

/// Call immediately before and after validation/build while the agent's edits
/// are paused. A hash never silently stands for a dirty or superseded checkout.
pub fn verify_checkpoint_for_build(local: &OwnedWorktree, expected_commit: &str) -> Result<(), String> {
    verify_owned(local)?;
    let mut repo = open(&local.path)?;
    assert_head(&repo, local, expected_commit)?;
    assert_clean(&mut repo)
}

pub fn preview_promotion(repository: &Path, source: &str, target: &str) -> Result<PromotionPreview, String> {
    if !(is_private_branch(source) && target == "work" || source == "work" && target == "dev") {
        return Err("Promotions are squash local->work or squash work->dev only".into());
    }
    let mut repo = open(repository)?;
    let state = identity_of(&repo)?;
    let source_oid = resolve(&repo, &format!("refs/heads/{source}"))?;
    let target_oid = resolve(&repo, &format!("refs/heads/{target}"))?;
    assert_public_commit(&mut repo, &target_oid)?;
    if source == "work" { assert_public_commit(&mut repo, &source_oid)?; }
    let result_tree = merged_tree(&mut repo, &target_oid, &source_oid)?;
    let target_tree = commit_tree_oid(&mut repo, &target_oid)?;
    let changed_paths = changed_names(&mut repo, &target_tree, &result_tree)?;
    let diff_stat = diff_stat(&mut repo, &target_tree, &result_tree)?;
    Ok(PromotionPreview {
        repository: state.0, common_dir: state.1,
        source_branch: source.into(), target_branch: target.into(),
        source_oid: source_oid.to_hex(), target_oid: target_oid.to_hex(), result_tree: result_tree.to_hex(), changed_paths, diff_stat,
    })
}

/// Caller supplies validation for these exact source/target/tree IDs before
/// applying. The source is not a parent of the resulting public commit.
pub fn apply_promotion(preview: &PromotionPreview, target: &OwnedWorktree, message: &str) -> Result<Checkpoint, String> {
    validate_message(message)?;
    verify_owned(target)?;
    let mut repo = open(&target.path)?;
    let _lease = acquire_lease(&repo)?;
    if target.common_dir != preview.common_dir || target.branch != preview.target_branch {
        return Err("Promotion target does not match the reviewed repository and branch".into());
    }
    assert_clean(&mut repo)?;
    let current = preview_promotion(&target.path, &preview.source_branch, &preview.target_branch)?;
    if current.source_oid != preview.source_oid || current.target_oid != preview.target_oid || current.result_tree != preview.result_tree {
        return Err("Promotion preview is stale; inspect and validate the updated diff".into());
    }
    if current.changed_paths.is_empty() { return Err("There are no changes to squash".into()); }
    let tree = parse_oid(&preview.result_tree)?;
    let parent = parse_oid(&preview.target_oid)?;
    let commit = create_commit(&repo, &tree, &[parent], message)?;
    assert_public_commit(&mut repo, &commit)?;
    install_commit(&mut repo, target, &parent, &commit, false)?;
    Ok(Checkpoint { commit: commit.to_hex(), tree: preview.result_tree.clone(), parent: preview.target_oid.clone(), branch: target.branch.clone() })
}

/// Fetch is deliberately separate. These previews use current local tracking
/// refs; only the two public remote branches may feed their matching local tier.
pub fn preview_sync(repository: &Path, source_ref: &str, target: &str) -> Result<SyncPreview, String> {
    let permitted = source_ref == "refs/heads/work" && is_private_branch(target)
        || source_ref == "refs/heads/dev" && target == "work"
        || source_ref == "refs/remotes/origin/work" && target == "work"
        || source_ref == "refs/remotes/origin/dev" && target == "dev";
    if !permitted { return Err("Sync supports origin/work->work, origin/dev->dev, dev->work, and work->local only".into()); }
    let mut repo = open(repository)?;
    let state = identity_of(&repo)?;
    let source_oid = resolve(&repo, source_ref)?;
    let target_oid = resolve(&repo, &format!("refs/heads/{target}"))?;
    assert_public_commit(&mut repo, &source_oid)?;
    if !is_private_branch(target) { assert_public_commit(&mut repo, &target_oid)?; }
    let unchanged = ancestor(&mut repo, &source_oid, &target_oid)?;
    let fast_forward = ancestor(&mut repo, &target_oid, &source_oid)?;
    let target_tree = commit_tree_oid(&mut repo, &target_oid)?;
    let result_tree = if unchanged { target_tree }
        else if fast_forward { commit_tree_oid(&mut repo, &source_oid)? }
        else { merged_tree(&mut repo, &target_oid, &source_oid)? };
    let diff_stat = diff_stat(&mut repo, &target_tree, &result_tree)?;
    Ok(SyncPreview { repository: state.0, common_dir: state.1, source_ref: source_ref.into(),
        target_branch: target.into(), source_oid: source_oid.to_hex(), target_oid: target_oid.to_hex(),
        result_tree: result_tree.to_hex(), fast_forward, unchanged, diff_stat })
}

/// Refresh only public intake branches. `makepad_git` has no network
/// transport for this repository's origin, so the refresh must happen outside
/// Studio; nothing is merged, moved, pruned or pushed either way.
pub fn fetch_public_refs(repository: &Path) -> Result<(), String> {
    let repo = open(repository)?;
    let origin = remote_url(&repo.common_dir, "origin").unwrap_or_else(|| "origin".into());
    Err(format!("Fetching {origin} needs a network transport that Studio's own Git library does not provide; refresh origin/work and origin/dev outside Studio, then run the sync preview again"))
}

pub fn apply_sync(preview: &SyncPreview, target: &OwnedWorktree, message: &str) -> Result<Checkpoint, String> {
    validate_message(message)?;
    verify_owned(target)?;
    let mut repo = open(&target.path)?;
    let _lease = acquire_lease(&repo)?;
    if target.common_dir != preview.common_dir || target.branch != preview.target_branch {
        return Err("Sync target does not match its preview".into());
    }
    assert_clean(&mut repo)?;
    let current = preview_sync(&target.path, &preview.source_ref, &preview.target_branch)?;
    if current.source_oid != preview.source_oid || current.target_oid != preview.target_oid || current.result_tree != preview.result_tree {
        return Err("Sync preview is stale; inspect and validate it again".into());
    }
    if current.unchanged { return Ok(Checkpoint { commit: current.target_oid.clone(), tree: current.result_tree,
        parent: current.target_oid, branch: target.branch.clone() }); }
    let private = is_private_branch(&target.branch);
    let target_oid = parse_oid(&current.target_oid)?;
    let source_oid = parse_oid(&current.source_oid)?;
    let commit = if current.fast_forward { source_oid } else {
        let message = if private { format!("{message}\n\nStudio-Local-Checkpoint: true\n") } else { message.into() };
        create_commit(&repo, &parse_oid(&current.result_tree)?, &[target_oid, source_oid], &message)?
    };
    if !private { assert_public_commit(&mut repo, &commit)?; }
    install_commit(&mut repo, target, &target_oid, &commit, private && !current.fast_forward)?;
    Ok(Checkpoint { commit: commit.to_hex(), tree: current.result_tree, parent: current.target_oid, branch: target.branch.clone() })
}

/// Used by the standalone guard. This also catches renamed branches and raw
/// object IDs, because policy checks the complete reachable commit ancestry.
pub fn validate_push_policy(repository: &Path, input: &str) -> Result<(), String> {
    if input.len() > MAX_OUTPUT { return Err("Push description exceeds policy limit".into()); }
    let mut repo = open(repository)?;
    let private = private_commits(&repo)?;
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 4 { return Err("Malformed pre-push input; refusing push".into()); }
        let (local_ref, local_oid, remote_ref, remote_oid) = (fields[0], fields[1], fields[2], fields[3]);
        validate_oid(local_oid)?;
        validate_oid(remote_oid)?;
        if private_ref(local_ref) || private_ref(remote_ref) {
            return Err(format!("Studio local history must never be pushed ({local_ref} -> {remote_ref}); squash it into work first"));
        }
        if local_oid.bytes().all(|b| b == b'0') { continue; }
        let commit = parse_oid(local_oid)?;
        repo.read_commit(&commit).map_err(|error| format!("{local_oid} is not a commit: {error}"))?;
        let reachable = reachable_commits(&mut repo, &commit)?;
        if let Some(checkpoint) = private.iter().find(|checkpoint| reachable.contains(checkpoint)) {
            return Err(format!("Push would publish local checkpoint {} through {remote_ref}; only squash promotions may reach work/dev", checkpoint.to_hex()));
        }
    }
    Ok(())
}

pub fn install_push_guard(repository: &Path, guard_executable: &Path) -> Result<(), String> {
    let executable = fs::canonicalize(guard_executable).map_err(|error| format!("Build studio-git-guard before preparing a flow: {error}"))?;
    if !executable.is_file() { return Err("Git policy executable must be a file".into()); }
    let repo = open(repository)?;
    let hook = repo.common_dir.join("hooks").join("pre-push");
    let parent = hook.parent().ok_or("Git hook has no parent directory")?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let _lease = FileLease::create(parent.join("studio-pre-push-install.lock"))?;
    let previous = parent.join("pre-push.studio-original");
    let mut chain = previous.exists();
    let mut preserve_existing = false;
    if hook.exists() {
        let old = fs::read(&hook).map_err(io_error)?;
        if !old.starts_with(format!("#!/bin/sh\n{HOOK_MARKER}\n").as_bytes()) {
            if previous.exists() { return Err("Existing pre-push backup needs inspection before installing Studio policy".into()); }
            preserve_existing = true;
            chain = true;
        }
    }
    let chain_line = if chain { format!("if [ -x {0} ]; then {0} \"$@\" < \"$studio_push_input\" || exit $?; fi\n", shell_quote(&previous)?) } else { String::new() };
    let script = format!("#!/bin/sh\n{HOOK_MARKER}\nset -eu\nstudio_push_input=$(mktemp \"${{TMPDIR:-/tmp}}/studio-pre-push.XXXXXX\")\ntrap 'rm -f \"$studio_push_input\"' EXIT HUP INT TERM\ncat > \"$studio_push_input\"\n{} \"$@\" < \"$studio_push_input\" || exit $?\n{chain_line}", shell_quote(&executable)?);
    let temporary = parent.join("pre-push.studio-new");
    write_new(&temporary, script.as_bytes())?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).map_err(io_error)?;
    }
    if preserve_existing {
        if let Err(error) = fs::rename(&hook, &previous) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
    }
    if let Err(error) = fs::rename(&temporary, &hook) {
        if preserve_existing {
            fs::rename(&previous, &hook).map_err(|restore| format!("Hook installation failed ({error}); restore {} manually: {restore}", previous.display()))?;
        }
        let _ = fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    Ok(())
}

// --- Read-only helpers for the worker's other Git questions ---

/// The tree a commit records.
pub fn commit_tree(repository: &Path, commit: &str) -> Result<String, String> {
    let mut repo = open(repository)?;
    Ok(commit_tree_oid(&mut repo, &parse_oid(commit)?)?.to_hex())
}

/// The first parent of a commit.
pub fn parent_commit(repository: &Path, commit: &str) -> Result<String, String> {
    let mut repo = open(repository)?;
    let parent = repo.read_commit(&parse_oid(commit)?).map_err(git_error)?.parents.first().copied()
        .ok_or("The first artifact has no parent commit")?;
    Ok(parent.to_hex())
}

/// The blob id a file's bytes hash to, streamed.
pub fn content_hash(path: &Path) -> Result<String, String> {
    hash_file_blob(path).map(|oid| oid.to_hex()).map_err(git_error)
}

/// A unified diff between two commits, or between `from` (`HEAD` or a commit)
/// and the working tree when `to` is `None`. Renames appear as delete + add.
pub fn unified_diff(repository: &Path, from: &str, to: Option<&str>) -> Result<String, String> {
    let mut repo = open(repository)?;
    let from_oid = if from == "HEAD" { repo.head_oid().map_err(git_error)? } else { parse_oid(from)? };
    let from_tree = commit_tree_oid(&mut repo, &from_oid)?;
    let mut output = String::new();
    match to {
        Some(to) => {
            let to_tree = commit_tree_oid(&mut repo, &parse_oid(to)?)?;
            for change in repo.diff_trees(&from_tree, &to_tree).map_err(git_error)? {
                let (path, old, new) = match change {
                    TreeChange::Added { path, oid, .. } => (path, None, Some(repo.read_blob(&oid).map_err(git_error)?)),
                    TreeChange::Deleted { path, oid, .. } => (path, Some(repo.read_blob(&oid).map_err(git_error)?), None),
                    TreeChange::Modified { path, old_oid, new_oid, .. } => (path, Some(repo.read_blob(&old_oid).map_err(git_error)?), Some(repo.read_blob(&new_oid).map_err(git_error)?)),
                };
                append_file_diff(&mut output, &path, old.as_deref(), new.as_deref())?;
            }
        }
        None => {
            let files = repo.flatten_tree_with_mode(&from_tree).map_err(git_error)?;
            for change in changed_paths(&mut repo)? {
                let old = match files.get(&change.path) { Some((oid, _)) => Some(repo.read_blob(oid).map_err(git_error)?), None => None };
                let file = repo.workdir.join(&change.path);
                let new = if file.is_file() { Some(fs::read(&file).map_err(io_error)?) } else { None };
                append_file_diff(&mut output, &change.path, old.as_deref(), new.as_deref())?;
            }
        }
    }
    Ok(output.trim_end_matches(['\n', '\r']).to_owned())
}

fn append_file_diff(output: &mut String, path: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> Result<(), String> {
    output.push_str(&format!("diff --git a/{path} b/{path}\n"));
    if old.is_some_and(is_binary) || new.is_some_and(is_binary) {
        output.push_str(&format!("Binary files a/{path} and b/{path} differ\n"));
    } else {
        let diff = diff_blobs(old.unwrap_or(&[]), new.unwrap_or(&[]),
            old.map(|_| path.to_owned()), new.map(|_| path.to_owned()), None, None);
        output.push_str(&format_unified_diff(&diff, 3));
    }
    if output.len() > MAX_OUTPUT { return Err("Git output exceeds 4 MiB; inspect a narrower file selection".into()); }
    Ok(())
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|byte| *byte == 0)
}

// --- Checkpoint policy ---

fn validate_checkpoint_path(repo: &Repository, index: &Index, value: &str, instructions: &[String]) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty() || value.len() > 4096 || value.contains(['\0', '\n', '\r', '\\'])
        || !path.components().all(|component| matches!(component, Component::Normal(_))) {
        return Err(format!("Checkpoint requires a literal repository-relative file path: {value:?}"));
    }
    let lower = value.to_ascii_lowercase();
    let parts: Vec<_> = lower.split('/').collect();
    if parts.iter().any(|part| matches!(*part, ".git" | "local" | "target" | "node_modules" | "logs" | "recordings" | "screenshots" | "scratch" | "tmp" | ".claude" | ".codex" | ".grok"))
        || [".mp4", ".mov", ".log", ".tmp", ".bak", ".swp", ".DS_Store"].iter().any(|extension| lower.ends_with(&extension.to_ascii_lowercase())) {
        return Err(format!("Generated state or media is excluded from checkpoints: {value}"));
    }
    let tracked = index.entries.iter().any(|entry| entry.stage() == 0 && entry.path == value);
    let instruction_name = path.file_name().and_then(OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("AGENTS.md") || name.eq_ignore_ascii_case("CLAUDE.md"));
    if lower.ends_with(".md") && !(tracked && instruction_name && instructions.iter().any(|allowed| allowed == value)) {
        return Err(format!("Markdown changes are excluded except explicitly allowed existing instruction files: {value}"));
    }
    if parts.iter().any(|part| matches!(*part, "test" | "tests" | "__tests__"))
        || path.file_stem().and_then(OsStr::to_str).is_some_and(|stem| {
            let stem = stem.to_ascii_lowercase();
            stem.starts_with("test_") || stem.ends_with("_test") || stem.ends_with("_tests") || stem.ends_with(".test") || stem.ends_with(".spec")
        }) {
        return Err(format!("Test changes are excluded from iteration commits: {value}"));
    }
    let absolute = repo.workdir.join(path);
    match fs::symlink_metadata(&absolute) {
        Ok(metadata) => {
            if !metadata.is_file() { return Err(format!("Checkpoint entries must be files, not directories or links: {value}")); }
            let actual = fs::canonicalize(&absolute).map_err(io_error)?;
            if !actual.starts_with(&repo.workdir) { return Err(format!("Checkpoint path escapes its worktree: {value}")); }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && tracked => {},
        Err(error) => return Err(format!("Cannot checkpoint {value}: {error}")),
    }
    Ok(())
}

fn validate_added_source(repo: &mut Repository, before: &ObjectId, tree: &ObjectId) -> Result<(), String> {
    for change in repo.diff_trees(before, tree).map_err(git_error)? {
        let added: Vec<String> = match change {
            TreeChange::Added { oid, mode, .. } if mode != 0o160000 => {
                String::from_utf8_lossy(&repo.read_blob(&oid).map_err(git_error)?).lines().map(str::to_owned).collect()
            }
            TreeChange::Modified { old_oid, new_oid, new_mode, .. } if new_mode != 0o160000 => {
                let old = repo.read_blob(&old_oid).map_err(git_error)?;
                let new = repo.read_blob(&new_oid).map_err(git_error)?;
                let diff = diff_blobs(&old, &new, None, None, None, None);
                diff.ops.iter().flat_map(|op| match op {
                    DiffOp::Insert { new_index, len } => diff.new_lines[*new_index..(*new_index + *len).min(diff.new_lines.len())].to_vec(),
                    _ => Vec::new(),
                }).collect()
            }
            _ => Vec::new(),
        };
        for line in added {
            let line = line.trim();
            if line.starts_with("#[test]") || line.starts_with("#[cfg(test)]")
                || line.starts_with("#[tokio::test") || line.starts_with("#[test_case")
                || line.starts_with("mod tests") || line.starts_with("fn test_") {
                return Err("Added test code is excluded from iteration commits; run existing tests without adding test fixtures".into());
            }
        }
    }
    Ok(())
}

/// The index a tree describes, keeping the stat cache of entries the current
/// index already holds at the same blob so later status checks stay cheap.
fn index_from_tree(repo: &mut Repository, tree: &ObjectId, current: &Index) -> Result<Index, String> {
    let files = repo.flatten_tree_with_mode(tree).map_err(git_error)?;
    let known: HashMap<&str, &IndexEntry> = current.entries.iter().filter(|entry| entry.stage() == 0).map(|entry| (entry.path.as_str(), entry)).collect();
    let mut entries: Vec<IndexEntry> = files.into_iter().map(|(path, (oid, mode))| {
        match known.get(path.as_str()) {
            Some(existing) if existing.oid == oid && existing.mode == mode => (*existing).clone(),
            _ => IndexEntry { ctime_sec: 0, ctime_nsec: 0, mtime_sec: 0, mtime_nsec: 0, dev: 0, ino: 0, mode, uid: 0, gid: 0,
                file_size: 0, oid, flags: (path.len().min(0xFFF)) as u16, path },
        }
    }).collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Index { version: 2, entries })
}

// --- Commits, trees, refs ---

fn install_commit(repo: &mut Repository, target: &OwnedWorktree, old: &ObjectId, new: &ObjectId, private: bool) -> Result<(), String> {
    assert_head(repo, target, &old.to_hex())?;
    assert_clean(repo)?;
    let old_tree = commit_tree_oid(repo, old)?;
    let new_tree = commit_tree_oid(repo, new)?;
    // The two-tree update refuses to overwrite concurrently edited files.
    // A failed ref CAS leaves an inspectable updated checkout; never reset it.
    repo.update_worktree(&old_tree, &new_tree).map_err(|error| format!("Checkout update refused: {error}"))?;
    let result = if private { update_private_ref(repo, target, old, new) }
        else { write_ref_cas(repo, &format!("refs/heads/{}", target.branch), Some(old), new) };
    result.map_err(|error| format!("Prepared checkout but could not advance its branch; inspect staged changes before retrying: {error}"))
}

fn update_private_ref(repo: &Repository, target: &OwnedWorktree, old: &ObjectId, new: &ObjectId) -> Result<(), String> {
    let checkpoint_ref = format!("{PRIVATE_REFS}{}", new.to_hex());
    let created = match read_ref(repo, &checkpoint_ref)? {
        Some(RefTarget::Direct(existing)) if existing == *new => false,
        Some(_) => return Err(format!("{checkpoint_ref} already exists with another value")),
        None => { write_ref_cas(repo, &checkpoint_ref, None, new)?; true }
    };
    if let Err(error) = write_ref_cas(repo, &format!("refs/heads/{}", target.branch), Some(old), new) {
        if created { let _ = refs::delete_ref_in(&repo.git_dir, &repo.common_dir, &checkpoint_ref); }
        return Err(error);
    }
    Ok(())
}

/// Compare-and-swap one ref through git's own `<ref>.lock` protocol. With
/// `expected == None` the ref must not exist yet.
fn write_ref_cas(repo: &Repository, name: &str, expected: Option<&ObjectId>, new: &ObjectId) -> Result<(), String> {
    let dir = if refs::is_private_ref(name) { &repo.git_dir } else { &repo.common_dir };
    let ref_path = dir.join(name);
    if let Some(parent) = ref_path.parent() { fs::create_dir_all(parent).map_err(io_error)?; }
    let lock = FileLease::create(dir.join(format!("{name}.lock")))?;
    let current = match read_ref(repo, name)? {
        Some(RefTarget::Direct(oid)) => Some(oid),
        Some(RefTarget::Symbolic(other)) => return Err(format!("{name} is a symbolic ref to {other}")),
        None => None,
    };
    if current.as_ref() != expected {
        return Err(format!("{name} moved since this action was prepared; refresh before retrying"));
    }
    let mut lock = lock;
    lock.write(format!("{}\n", new.to_hex()).as_bytes())?;
    lock.install(&ref_path)
}

fn read_ref(repo: &Repository, name: &str) -> Result<Option<RefTarget>, String> {
    refs::read_ref_in(&repo.git_dir, &repo.common_dir, name).map_err(git_error)
}

fn ref_exists(repo: &Repository, name: &str) -> Result<bool, String> {
    Ok(read_ref(repo, name)?.is_some())
}

fn resolve(repo: &Repository, name: &str) -> Result<ObjectId, String> {
    if name == "HEAD" { return repo.head_oid().map_err(|error| format!("HEAD is not a commit: {error}")); }
    repo.resolve_ref(name).map_err(|error| format!("{name}: {error}"))
}

fn private_ref(reference: &str) -> bool {
    reference.starts_with("refs/studio/") || reference == "refs/heads/local"
        || reference.starts_with("refs/heads/local-") || reference.starts_with("refs/heads/local/")
        || reference == "refs/heads/steps" || reference.starts_with("refs/heads/steps-")
}

fn private_commits(repo: &Repository) -> Result<Vec<ObjectId>, String> {
    Ok(refs::list_refs_in(&repo.git_dir, &repo.common_dir, PRIVATE_REFS).map_err(git_error)?
        .into_iter().filter_map(|reference| match reference.target { RefTarget::Direct(oid) => Some(oid), RefTarget::Symbolic(_) => None }).collect())
}

fn assert_public_commit(repo: &mut Repository, commit: &ObjectId) -> Result<(), String> {
    let private = private_commits(repo)?;
    if private.is_empty() { return Ok(()); }
    let reachable = reachable_commits(repo, commit)?;
    if let Some(checkpoint) = private.iter().find(|checkpoint| reachable.contains(checkpoint)) {
        return Err(format!("Public history contains private checkpoint {}; repair by squash promotion before syncing or publishing", checkpoint.to_hex()));
    }
    Ok(())
}

/// Every commit reachable from `from`, itself included.
fn reachable_commits(repo: &mut Repository, from: &ObjectId) -> Result<HashSet<ObjectId>, String> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([*from]);
    while let Some(oid) = queue.pop_front() {
        if !seen.insert(oid) { continue; }
        for parent in repo.read_commit(&oid).map_err(|error| format!("{}: {error}", oid.to_hex()))?.parents {
            if !seen.contains(&parent) { queue.push_back(parent); }
        }
    }
    Ok(seen)
}

/// `merge-base --is-ancestor`: `ancestor` is `descendant` or reachable from it.
fn ancestor(repo: &mut Repository, ancestor: &ObjectId, descendant: &ObjectId) -> Result<bool, String> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([*descendant]);
    while let Some(oid) = queue.pop_front() {
        if oid == *ancestor { return Ok(true); }
        if !seen.insert(oid) { continue; }
        for parent in repo.read_commit(&oid).map_err(|error| format!("{}: {error}", oid.to_hex()))?.parents {
            if !seen.contains(&parent) { queue.push_back(parent); }
        }
    }
    Ok(false)
}

/// `merge-tree --write-tree target source`: a clean three-way merge written
/// as tree objects, or an error when any path needs a conflict decision.
fn merged_tree(repo: &mut Repository, target: &ObjectId, source: &ObjectId) -> Result<ObjectId, String> {
    let conflict = |detail: String| format!("Merge needs explicit conflict resolution; no checkout was changed: {detail}");
    let base = repo.merge_base(target, source).map_err(git_error)?
        .ok_or_else(|| conflict("the branches share no history".into()))?;
    let base_tree = commit_tree_oid(repo, &base)?;
    let ours_tree = commit_tree_oid(repo, target)?;
    let theirs_tree = commit_tree_oid(repo, source)?;
    let base_files = repo.flatten_tree_with_mode(&base_tree).map_err(git_error)?;
    let ours_files = repo.flatten_tree_with_mode(&ours_tree).map_err(git_error)?;
    let theirs_files = repo.flatten_tree_with_mode(&theirs_tree).map_err(git_error)?;
    let mut result: BTreeMap<String, (ObjectId, u32)> = BTreeMap::new();
    for entry in merge_trees(&base_files, &ours_files, &theirs_files) {
        match entry {
            TreeMergeEntry::Resolved { path, oid, mode } => { result.insert(path, (oid, mode)); }
            TreeMergeEntry::BothModified { path, base_oid, ours_oid, theirs_oid, mode } => {
                let base = repo.read_blob(&base_oid).map_err(git_error)?;
                let ours = repo.read_blob(&ours_oid).map_err(git_error)?;
                let theirs = repo.read_blob(&theirs_oid).map_err(git_error)?;
                let (Ok(base), Ok(ours), Ok(theirs)) = (std::str::from_utf8(&base), std::str::from_utf8(&ours), std::str::from_utf8(&theirs)) else {
                    return Err(conflict(format!("both sides changed binary file {path}")));
                };
                let merged = merge3_text(base, ours, theirs);
                if merged.has_conflict() { return Err(conflict(format!("both sides changed {path}"))); }
                let oid = repo.write_blob(merged.content().as_bytes()).map_err(git_error)?;
                result.insert(path, (oid, mode));
            }
            TreeMergeEntry::AddAdd { path, .. } => return Err(conflict(format!("both sides added {path} differently"))),
            TreeMergeEntry::DeleteModify { path, .. } => return Err(conflict(format!("{path} was deleted on one side and modified on the other"))),
        }
    }
    let entries: Vec<IndexEntry> = result.into_iter().map(|(path, (oid, mode))| IndexEntry {
        ctime_sec: 0, ctime_nsec: 0, mtime_sec: 0, mtime_nsec: 0, dev: 0, ino: 0, mode, uid: 0, gid: 0, file_size: 0, oid,
        flags: (path.len().min(0xFFF)) as u16, path,
    }).collect();
    repo.index_to_tree(&Index { version: 2, entries }).map_err(git_error)
}

fn changed_names(repo: &mut Repository, old_tree: &ObjectId, new_tree: &ObjectId) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = repo.diff_trees(old_tree, new_tree).map_err(git_error)?.into_iter().map(|change| match change {
        TreeChange::Added { path, .. } | TreeChange::Deleted { path, .. } | TreeChange::Modified { path, .. } => path,
    }).collect();
    names.sort();
    Ok(names)
}

/// `diff --stat` between two trees.
fn diff_stat(repo: &mut Repository, old_tree: &ObjectId, new_tree: &ObjectId) -> Result<String, String> {
    let mut rows: Vec<(String, usize, usize, bool)> = Vec::new();
    for change in repo.diff_trees(old_tree, new_tree).map_err(git_error)? {
        let (path, old, new) = match change {
            TreeChange::Added { path, oid, .. } => (path, None, Some(repo.read_blob(&oid).map_err(git_error)?)),
            TreeChange::Deleted { path, oid, .. } => (path, Some(repo.read_blob(&oid).map_err(git_error)?), None),
            TreeChange::Modified { path, old_oid, new_oid, .. } => (path, Some(repo.read_blob(&old_oid).map_err(git_error)?), Some(repo.read_blob(&new_oid).map_err(git_error)?)),
        };
        if old.as_deref().is_some_and(is_binary) || new.as_deref().is_some_and(is_binary) {
            rows.push((path, 0, 0, true));
            continue;
        }
        let diff = diff_blobs(old.as_deref().unwrap_or(&[]), new.as_deref().unwrap_or(&[]), None, None, None, None);
        let (mut insertions, mut deletions) = (0, 0);
        for op in &diff.ops {
            match op { DiffOp::Insert { len, .. } => insertions += len, DiffOp::Delete { len, .. } => deletions += len, DiffOp::Equal { .. } => {} }
        }
        rows.push((path, insertions, deletions, false));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    if rows.is_empty() { return Ok(String::new()); }
    let width = rows.iter().map(|row| row.0.len()).max().unwrap_or(0);
    let largest = rows.iter().map(|row| row.1 + row.2).max().unwrap_or(0).max(1);
    let mut text = String::new();
    let (mut files, mut insertions, mut deletions) = (0usize, 0usize, 0usize);
    for (path, added, removed, binary) in rows {
        files += 1;
        insertions += added;
        deletions += removed;
        if binary {
            text.push_str(&format!(" {path:<width$} | Bin\n"));
        } else {
            let total = added + removed;
            let scale = |count: usize| if largest > 40 { count * 40 / largest } else { count };
            text.push_str(&format!(" {path:<width$} | {total:>4} {}{}\n", "+".repeat(scale(added)), "-".repeat(scale(removed))));
        }
    }
    text.push_str(&format!(" {files} file{} changed, {insertions} insertion{}(+), {deletions} deletion{}(-)",
        if files == 1 { "" } else { "s" }, if insertions == 1 { "" } else { "s" }, if deletions == 1 { "" } else { "s" }));
    Ok(text)
}

fn commit_tree_oid(repo: &mut Repository, commit: &ObjectId) -> Result<ObjectId, String> {
    Ok(repo.read_commit(commit).map_err(|error| format!("{} is not a commit: {error}", commit.to_hex()))?.tree)
}

fn create_commit(repo: &Repository, tree: &ObjectId, parents: &[ObjectId], message: &str) -> Result<ObjectId, String> {
    let signature = identity(&repo.common_dir);
    repo.write_commit(&Commit { tree: *tree, parents: parents.to_vec(), author: signature.clone(), committer: signature, message: message.into() })
        .map_err(git_error)
}

/// `user.name`/`user.email` from the usual git config files (environment
/// overrides first), with a Studio fallback identity. Timestamps are UTC.
fn identity(common_dir: &Path) -> Signature {
    let mut name = std::env::var("GIT_AUTHOR_NAME").ok().filter(|value| !value.trim().is_empty());
    let mut email = std::env::var("GIT_AUTHOR_EMAIL").ok().filter(|value| !value.trim().is_empty());
    if name.is_none() || email.is_none() {
        let mut files = Vec::new();
        files.push(common_dir.join("config"));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            files.push(home.join(".gitconfig"));
            files.push(match std::env::var_os("XDG_CONFIG_HOME") { Some(xdg) => PathBuf::from(xdg).join("git/config"), None => home.join(".config/git/config") });
        }
        for file in files {
            let Ok(text) = fs::read_to_string(&file) else { continue };
            let mut section = String::new();
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') { section = line.to_ascii_lowercase(); continue; }
                if section != "[user]" { continue; }
                if let Some((key, value)) = line.split_once('=') {
                    let value = value.trim().trim_matches('"').to_owned();
                    match key.trim().to_ascii_lowercase().as_str() {
                        "name" if name.is_none() && !value.is_empty() => name = Some(value),
                        "email" if email.is_none() && !value.is_empty() => email = Some(value),
                        _ => {}
                    }
                }
            }
        }
    }
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs() as i64).unwrap_or(0);
    Signature {
        name: name.unwrap_or_else(|| "Studio Iteration".into()),
        email: email.unwrap_or_else(|| "studio@localhost".into()),
        timestamp,
        tz_offset: "+0000".into(),
    }
}

fn remote_url(common_dir: &Path, remote: &str) -> Option<String> {
    let text = fs::read_to_string(common_dir.join("config")).ok()?;
    let wanted = format!("[remote \"{remote}\"]");
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') { inside = line == wanted; continue; }
        if inside {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "url" { return Some(value.trim().to_owned()); }
            }
        }
    }
    None
}

/// `branch.<name>.pushRemote = studio-local-never-push`, appended to the
/// shared config through its lock file when the section is not present.
fn set_branch_push_remote(common_dir: &Path, branch: &str) -> Result<(), String> {
    let config = common_dir.join("config");
    let text = fs::read_to_string(&config).unwrap_or_default();
    let header = format!("[branch \"{branch}\"]");
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') { inside = line == header; continue; }
        if inside && line.split_once('=').is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("pushRemote")) { return Ok(()); }
    }
    let mut updated = text.clone();
    if !updated.is_empty() && !updated.ends_with('\n') { updated.push('\n'); }
    updated.push_str(&format!("{header}\n\tpushRemote = {NEVER_PUSH_REMOTE}\n"));
    let mut lock = FileLease::create(common_dir.join("config.lock"))?;
    lock.write(updated.as_bytes())?;
    lock.install(&config)
}

// --- Ownership, leases, state ---

fn verify_owned(owned: &OwnedWorktree) -> Result<(), String> {
    let repo = open(&owned.path)?;
    let (root, common_dir) = identity_of(&repo)?;
    let branch = repo.current_branch().map_err(git_error)?;
    if root != owned.path || common_dir != owned.common_dir || branch.as_deref() != Some(&owned.branch) {
        return Err("Worktree identity or branch changed; stop this operation and inspect it".into());
    }
    let marker = repo.git_dir.join(OWNER_FILE);
    if fs::read_to_string(&marker).map_err(|_| "This checkout is not owned by a Studio flow; shared user checkouts cannot be mutated")? != owner_text(owned) {
        return Err("Studio worktree ownership marker does not match this checkout".into());
    }
    Ok(())
}

fn owner_text(owned: &OwnedWorktree) -> String {
    format!("studio-iteration-worktree-v1\nbranch={}\npath={}\ncommon={}\n", owned.branch, owned.path.display(), owned.common_dir.display())
}

fn released_record(common_dir: &Path, branch: &str) -> PathBuf {
    common_dir.join(RELEASED_DIR).join(branch)
}

fn released_text(branch: &str, common_dir: &Path) -> String {
    format!("studio-iteration-released-v1\nbranch={branch}\ncommon={}\n", common_dir.display())
}

fn acquire_lease(repo: &Repository) -> Result<FileLease, String> {
    FileLease::create(repo.git_dir.join("studio-iteration.lock"))
}

fn assert_head(repo: &Repository, owned: &OwnedWorktree, expected: &str) -> Result<(), String> {
    validate_oid(expected)?;
    if resolve(repo, "HEAD")?.to_hex() != expected || repo.current_branch().map_err(git_error)?.as_deref() != Some(&owned.branch) {
        return Err("Branch changed since this action was prepared; refresh before retrying".into());
    }
    Ok(())
}

fn assert_clean(repo: &mut Repository) -> Result<(), String> {
    assert_no_operation(repo)?;
    if !changed_paths(repo)?.is_empty() { return Err("Target worktree must be clean, including untracked files; no stash/reset is performed".into()); }
    Ok(())
}

fn assert_no_operation(repo: &Repository) -> Result<(), String> {
    for marker in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply", "BISECT_LOG", "sequencer"] {
        if repo.git_dir.join(marker).exists() { return Err(format!("Complete the existing Git operation ({marker}) before Studio changes this checkout")); }
    }
    Ok(())
}

/// `status --porcelain=v1 -z --untracked-files=all --no-renames`, including
/// git's racy-index rule: entries written in the same second as the index are
/// re-hashed instead of trusted by stat.
fn changed_paths(repo: &mut Repository) -> Result<Vec<ChangedPath>, String> {
    let index = repo.read_index().map_err(git_error)?;
    if let Some(entry) = index.entries.iter().find(|entry| entry.stage() != 0) {
        return Err(format!("Resolve conflict in {} before checkpointing", entry.path));
    }
    let status = repo.status().map_err(git_error)?;
    let mut codes: BTreeMap<String, [u8; 2]> = BTreeMap::new();
    for entry in status.entries {
        let code = codes.entry(entry.path).or_insert([b' ', b' ']);
        match entry.status {
            FileStatus::Staged => code[0] = b'M',
            FileStatus::StagedNew => code[0] = b'A',
            FileStatus::StagedDeleted => code[0] = b'D',
            FileStatus::Modified => code[1] = b'M',
            FileStatus::Deleted => code[1] = b'D',
            FileStatus::Untracked => *code = [b'?', b'?'],
        }
    }
    let index_mtime = fs::metadata(repo.git_dir.join("index")).ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs().min(u32::MAX as u64) as u32);
    if let Some(index_mtime) = index_mtime {
        for entry in index.entries.iter().filter(|entry| entry.mtime_sec >= index_mtime) {
            if codes.get(&entry.path).is_some_and(|code| code[1] != b' ') { continue; }
            let file = repo.workdir.join(&entry.path);
            if file.is_file() && hash_file_blob(&file).map_err(git_error)? != entry.oid {
                codes.entry(entry.path.clone()).or_insert([b' ', b' '])[1] = b'M';
            }
        }
    }
    Ok(codes.into_iter().map(|(path, code)| ChangedPath { path, status: String::from_utf8_lossy(&code).into_owned() }).collect())
}

fn state_of(repo: &mut Repository) -> Result<RepositoryState, String> {
    let (root, common_dir) = identity_of(repo)?;
    let head = resolve(repo, "HEAD")?.to_hex();
    let branch = repo.current_branch().map_err(git_error)?;
    let changes = changed_paths(repo)?;
    let worktrees = repo.worktree_list().map_err(git_error)?.into_iter().map(|tree| WorktreeState {
        path: tree.path,
        branch: tree.branch,
        head: tree.head.map(|oid| oid.to_hex()).unwrap_or_default(),
    }).collect();
    Ok(RepositoryState { root, common_dir, branch, head, changes, worktrees })
}

/// Canonical (toplevel, common dir) of an opened repository.
fn identity_of(repo: &Repository) -> Result<(PathBuf, PathBuf), String> {
    Ok((fs::canonicalize(&repo.workdir).map_err(io_error)?, fs::canonicalize(&repo.common_dir).map_err(io_error)?))
}

fn open(path: &Path) -> Result<Repository, String> {
    Repository::open(path).map_err(|error| format!("{} is not a Git worktree: {error}", path.display()))
}

fn parse_oid(value: &str) -> Result<ObjectId, String> {
    validate_oid(value)?;
    ObjectId::from_hex(value).map_err(git_error)
}

fn validate_oid(value: &str) -> Result<(), String> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Expected a full Git object hash".into());
    }
    Ok(())
}

fn validate_message(message: &str) -> Result<(), String> {
    if message.trim().is_empty() || message.len() > 8192 || message.contains('\0') { return Err("Commit message must be nonempty and at most 8 KiB".into()); }
    Ok(())
}

fn shell_quote(path: &Path) -> Result<String, String> {
    let value = path.to_str().ok_or("Git hook paths must be UTF-8")?;
    if value.contains(['\n', '\r', '\0']) { return Err("Unsupported Git hook path".into()); }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn write_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(io_error)?;
    file.write_all(contents).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn io_error(error: std::io::Error) -> String { error.to_string() }

fn git_error(error: GitError) -> String { error.to_string() }

struct FileLease { path: PathBuf, file: Option<fs::File>, installed: bool }
impl FileLease {
    fn create(path: PathBuf) -> Result<Self, String> {
        let file = OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|error| format!("Cannot acquire {} (another operation or recovery may be pending): {error}", path.display()))?;
        Ok(Self { path, file: Some(file), installed: false })
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let file = self.file.as_mut().ok_or("Git index lease is closed")?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)
    }
    fn install(&mut self, destination: &Path) -> Result<(), String> {
        self.file.take();
        // Once the ref has advanced this file is recovery state on failure.
        self.installed = true;
        fs::rename(&self.path, destination).map_err(|error| format!("Checkpoint commit exists, but its index needs recovery from {}: {error}", self.path.display()))?;
        Ok(())
    }
}
impl Drop for FileLease {
    fn drop(&mut self) {
        self.file.take();
        if !self.installed { let _ = fs::remove_file(&self.path); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_git::{test_support::tempdir, Tree, TreeEntry};

    fn sig() -> Signature {
        Signature { name: "Test".into(), email: "test@example.org".into(), timestamp: 0, tz_offset: "+0000".into() }
    }

    fn commit_files(repo: &mut Repository, files: &[(&str, &str)], parents: Vec<ObjectId>, message: &str) -> ObjectId {
        let mut dirs: BTreeMap<String, Vec<TreeEntry>> = BTreeMap::new();
        for (path, content) in files {
            let oid = repo.write_blob(content.as_bytes()).unwrap();
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
            dirs.entry(dir.into()).or_default().push(TreeEntry { mode: 0o100644, name: name.into(), oid });
        }
        let mut root = dirs.remove("").unwrap_or_default();
        for (dir, mut entries) in dirs {
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            let oid = repo.write_tree(&Tree { entries }).unwrap();
            root.push(TreeEntry { mode: 0o040000, name: dir, oid });
        }
        root.sort_by(|a, b| a.name.cmp(&b.name));
        let tree = repo.write_tree(&Tree { entries: root }).unwrap();
        repo.write_commit(&Commit { tree, parents, author: sig(), committer: sig(), message: message.into() }).unwrap()
    }

    #[test]
    fn flow_worktree_checkpoint_promotion_sync_and_release_round_trip() {
        let base = tempdir().unwrap();
        let root = fs::canonicalize(base.path()).unwrap();
        let repository = root.join("repo");
        let git = repository.join(".git");
        fs::create_dir_all(git.join("objects")).unwrap();
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let mut main = Repository::open(&repository).unwrap();
        let a = commit_files(&mut main, &[("src/lib.rs", "fn a() {}\n"), ("README.md", "readme\n")], vec![], "initial\n");
        for branch in ["main", "work", "dev"] { main.create_branch(branch, &a).unwrap(); }
        let guard = root.join("guard");
        fs::write(&guard, "#!/bin/sh\nexit 0\n").unwrap();

        // A private flow checkout on a new branch cut from work.
        let flow_dir = root.join("flows").join("f1");
        fs::create_dir_all(flow_dir.parent().unwrap()).unwrap();
        let owned = ensure_flow_worktree(&repository, &flow_dir, "local-f1", &guard).unwrap();
        assert_eq!(owned.branch, "local-f1");
        assert_eq!(owned.common_dir, git);
        let state = inspect(&flow_dir).unwrap();
        assert_eq!(state.branch.as_deref(), Some("local-f1"));
        assert_eq!(state.head, a.to_hex());
        assert!(state.changes.is_empty(), "{:?}", state.changes);
        assert_eq!(state.worktrees.len(), 2);
        assert!(fs::read_to_string(git.join("config")).unwrap().contains("[branch \"local-f1\"]"));
        assert!(git.join("hooks/pre-push").is_file());
        assert!(git.join("worktrees/f1").join(OWNER_FILE).is_file());
        verify_flow_worktree(&owned).unwrap();
        // Re-entry on the same destination verifies instead of re-adding.
        ensure_flow_worktree(&repository, &flow_dir, "local-f1", &guard).unwrap();
        // The same private branch cannot be adopted elsewhere.
        let error = ensure_flow_worktree(&repository, &root.join("flows").join("f2"), "local-f1", &guard).unwrap_err();
        assert!(error.contains("already checked out"), "{error}");

        // Edits show as porcelain codes and diff against HEAD.
        fs::write(flow_dir.join("src/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        fs::write(flow_dir.join("src/new.rs"), "pub fn new() {}\n").unwrap();
        let changes = inspect(&flow_dir).unwrap().changes;
        let codes: Vec<(String, String)> = changes.iter().map(|c| (c.path.clone(), c.status.clone())).collect();
        assert_eq!(codes, vec![("src/lib.rs".into(), " M".into()), ("src/new.rs".into(), "??".into())]);
        let diff = unified_diff(&flow_dir, "HEAD", None).unwrap();
        assert!(diff.contains("+++ b/src/lib.rs") && diff.contains("+fn b() {}") && diff.contains("+pub fn new() {}"), "{diff}");
        assert!(verify_checkpoint_for_build(&owned, &a.to_hex()).is_err(), "dirty checkout is not build-ready");

        // Checkpoints need the exact change list and refuse test code.
        let error = checkpoint(&owned, &a.to_hex(), &["src/lib.rs".into()], &[], "partial").unwrap_err();
        assert!(error.contains("Uncheckpointed change src/new.rs"), "{error}");
        fs::write(flow_dir.join("src/new.rs"), "#[test]\nfn t() {}\n").unwrap();
        let error = checkpoint(&owned, &a.to_hex(), &["src/lib.rs".into(), "src/new.rs".into()], &[], "tests").unwrap_err();
        assert!(error.contains("Added test code"), "{error}");
        fs::write(flow_dir.join("src/new.rs"), "pub fn new() {}\n").unwrap();
        let first = checkpoint(&owned, &a.to_hex(), &["src/lib.rs".into(), "src/new.rs".into()], &[], "Checkpoint one").unwrap();
        assert_eq!(first.parent, a.to_hex());
        assert_eq!(first.branch, "local-f1");
        let mut flow_repo = Repository::open(&flow_dir).unwrap();
        assert_eq!(flow_repo.head_oid().unwrap().to_hex(), first.commit);
        assert_eq!(flow_repo.resolve_ref(&format!("{PRIVATE_REFS}{}", first.commit)).unwrap().to_hex(), first.commit);
        assert!(flow_repo.read_commit(&parse_oid(&first.commit).unwrap()).unwrap().message.contains("Studio-Local-Checkpoint: true"));
        assert!(inspect(&flow_dir).unwrap().changes.is_empty(), "index and worktree agree after the checkpoint");
        verify_checkpoint_for_build(&owned, &first.commit).unwrap();
        assert_eq!(commit_tree(&flow_dir, &first.commit).unwrap(), first.tree);
        assert_eq!(parent_commit(&flow_dir, &first.commit).unwrap(), a.to_hex());
        let artifact_diff = unified_diff(&flow_dir, &a.to_hex(), Some(&first.commit)).unwrap();
        assert!(artifact_diff.contains("+fn b() {}"), "{artifact_diff}");

        // Squash promotion into an owned work checkout.
        let preview = preview_promotion(&flow_dir, "local-f1", "work").unwrap();
        assert_eq!(preview.source_oid, first.commit);
        assert_eq!(preview.target_oid, a.to_hex());
        assert_eq!(preview.changed_paths, vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]);
        assert!(preview.diff_stat.contains("2 files changed"), "{}", preview.diff_stat);
        let work_dir = root.join("integration").join("work");
        fs::create_dir_all(work_dir.parent().unwrap()).unwrap();
        let work = ensure_promotion_worktree(&flow_dir, &work_dir, "work").unwrap();
        let promoted = apply_promotion(&preview, &work, "Feature one").unwrap();
        let mut work_repo = Repository::open(&work_dir).unwrap();
        let promoted_commit = work_repo.read_commit(&parse_oid(&promoted.commit).unwrap()).unwrap();
        assert_eq!(promoted_commit.parents, vec![a]);
        assert_eq!(promoted_commit.tree.to_hex(), preview.result_tree);
        assert_eq!(work_repo.resolve_ref("refs/heads/work").unwrap().to_hex(), promoted.commit);
        assert_eq!(fs::read_to_string(work_dir.join("src/new.rs")).unwrap(), "pub fn new() {}\n");
        assert!(inspect(&work_dir).unwrap().changes.is_empty(), "promotion leaves the target clean");
        assert!(apply_promotion(&preview, &work, "again").unwrap_err().contains("stale"));

        // Sync the public result back into the private lane as a merge.
        let sync = preview_sync(&flow_dir, "refs/heads/work", "local-f1").unwrap();
        assert_eq!(sync.source_oid, promoted.commit);
        assert!(!sync.unchanged && !sync.fast_forward);
        let synced = apply_sync(&sync, &owned, "Sync work").unwrap();
        let merge = Repository::open(&flow_dir).unwrap().read_commit(&parse_oid(&synced.commit).unwrap()).unwrap();
        assert_eq!(merge.parents, vec![parse_oid(&first.commit).unwrap(), parse_oid(&promoted.commit).unwrap()]);
        assert!(inspect(&flow_dir).unwrap().changes.is_empty());
        assert!(preview_sync(&flow_dir, "refs/heads/work", "local-f1").unwrap().unchanged);

        // Push policy: private refs and private ancestry never leave.
        let zero = "0".repeat(40);
        assert!(validate_push_policy(&flow_dir, &format!("refs/heads/local-f1 {} refs/heads/local-f1 {zero}", first.commit)).unwrap_err().contains("never be pushed"));
        validate_push_policy(&flow_dir, &format!("refs/heads/work {} refs/heads/work {}", promoted.commit, a.to_hex())).unwrap();
        assert!(validate_push_policy(&flow_dir, &format!("refs/heads/feature {} refs/heads/feature {zero}", synced.commit)).unwrap_err().contains("local checkpoint"));
        assert!(preview_promotion(&flow_dir, "work", "dev").unwrap().changed_paths.len() == 2);

        // Release keeps dirty work, then removes the checkout but not the branch.
        fs::write(flow_dir.join("src/lib.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        let error = remove_flow_worktree(&owned, false).unwrap_err();
        assert!(error.contains("uncommitted change"), "{error}");
        assert!(flow_dir.join("src/lib.rs").is_file());
        fs::write(flow_dir.join("src/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        remove_flow_worktree(&owned, false).unwrap();
        assert!(!flow_dir.exists());
        assert!(!git.join("worktrees/f1").exists());
        assert_eq!(main.resolve_ref("refs/heads/local-f1").unwrap().to_hex(), synced.commit);
        assert_eq!(main.resolve_ref(&format!("{PRIVATE_REFS}{}", first.commit)).unwrap().to_hex(), first.commit);
        // Restoring the lane reattaches the released branch at its tip.
        let restored = ensure_flow_worktree(&repository, &flow_dir, "local-f1", &guard).unwrap();
        assert_eq!(inspect(&restored.path).unwrap().head, synced.commit);
        assert!(!git.join(RELEASED_DIR).join("local-f1").exists());
        assert_eq!(fs::read_to_string(flow_dir.join("src/new.rs")).unwrap(), "pub fn new() {}\n");
    }
}
