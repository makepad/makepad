pub mod clone;
mod bounded_read;
pub mod commit;
pub mod diff;
pub mod error;
pub mod http_sync;
pub mod index;
pub mod merge;
pub mod object;
pub mod oid;
pub mod pack;
pub mod refs;
pub mod repo;
mod sha1;
#[doc(hidden)]
pub mod test_support;
pub mod tree;
pub mod worktree;
pub mod worktrees;

pub use clone::{local_clone_depth1, CloneTimings};
pub use commit::{Commit, Signature};
pub use diff::{
    diff_blobs, diff_lines, diff_lines_bounded, diff_lines_with_limits, diff_trees,
    format_unified_diff, BoundedDiff, DiffLimits, DiffOp, Exhaustion, FileDiff, LineEnding,
    LineIndex, LineRecord, TreeChange, UnavailableHunk, BoundedTreeDiff, TreeChangeKind,
    TreeChangeRecord, TreeDiffCounters, TreeDiffExhaustion, TreeDiffLimits,
};
pub use error::GitError;
pub use http_sync::{
    apply_pack_and_checkout, build_info_refs_request, build_ls_refs_head_request,
    build_upload_pack_request, extract_pack_from_response, parse_info_refs_response,
    parse_ls_refs_head_response, GitHttpMethod, GitHttpRequest, GitHttpResponse, HttpSyncHooks,
    HttpSyncReport, NoopHttpSyncHooks, RemoteHead,
};
pub use index::{Index, IndexEntry};
pub use merge::{find_merge_base, merge3_text, merge_trees, MergeResult, TreeMergeEntry};
pub use object::{Object, ObjectKind};
pub use oid::ObjectId;
pub use refs::{Ref, RefTarget};
pub use repo::{repository_paths, resolve_commondir, resolve_gitfile, Repository, RepositoryPaths};
pub use tree::{Tree, TreeEntry};
pub use worktree::{
    checkout_tree, compute_status, compute_status_for_path_with_options,
    compute_status_for_path_worktree_only_with_options, compute_status_with_options,
    compute_status_worktree_only, compute_status_worktree_only_with_options, flatten_tree,
    hash_file_blob, index_entry_from_metadata, stage_file, unstage_file, write_worktree_file,
    FileStatus, Status, StatusEntry, StatusOptions,
};
pub use worktrees::{LinkedWorktree, WorktreeBranch};

pub mod memory;
