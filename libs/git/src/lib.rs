pub mod clone;
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
#[doc(hidden)]
pub mod test_support;
mod sha1;
pub mod tree;
pub mod worktree;

pub use clone::{local_clone_depth1, CloneTimings};
pub use commit::{Commit, Signature};
pub use diff::{
    diff_blobs, diff_lines, diff_trees, format_unified_diff, DiffOp, FileDiff, TreeChange,
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
pub use repo::Repository;
pub use tree::{Tree, TreeEntry};
pub use worktree::{
    checkout_tree, compute_status, compute_status_for_path_with_options,
    compute_status_for_path_worktree_only_with_options, compute_status_with_options,
    compute_status_worktree_only, compute_status_worktree_only_with_options, flatten_tree,
    stage_file, unstage_file, FileStatus, Status, StatusEntry, StatusOptions,
};
