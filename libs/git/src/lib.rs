pub mod clone;
pub mod commit;
pub mod diff;
pub mod error;
pub mod index;
pub mod merge;
pub mod object;
pub mod oid;
pub mod pack;
pub mod refs;
pub mod repo;
mod sha1;
pub mod tree;
pub mod worktree;

pub use clone::{local_clone_depth1, CloneTimings};
pub use commit::{Commit, Signature};
pub use diff::{
    diff_blobs, diff_lines, diff_trees, format_unified_diff, DiffOp, FileDiff, TreeChange,
};
pub use error::GitError;
pub use index::{Index, IndexEntry};
pub use merge::{find_merge_base, merge3_text, merge_trees, MergeResult, TreeMergeEntry};
pub use object::{Object, ObjectKind};
pub use oid::ObjectId;
pub use refs::{Ref, RefTarget};
pub use repo::Repository;
pub use tree::{Tree, TreeEntry};
pub use worktree::{
    checkout_tree, compute_status, compute_status_with_options, compute_status_worktree_only,
    compute_status_worktree_only_with_options, flatten_tree, stage_file, unstage_file, FileStatus,
    Status, StatusEntry, StatusOptions,
};
