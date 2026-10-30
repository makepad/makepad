//! Code sync, remote launch and WM restart: lane DEVHUB2 owns everything under `src/sync/`
//! (DESIGN.md §11 has the file ownership). The backbone calls these hooks; nothing
//! else in the crate needs to change for sync to grow.
//!
//! - [`register_master`]: services on a node's MASTER endpoint (the master only): `sync`
//!   (`status`, `apply`, `wm-restart`, `tick`), the bundle channel `sync-push`, and `launch`.
//! - [`register_peer`]: services on a node's PEER endpoint (other hubs). Coordinated patches are
//!   driven by the master (it reads every session JIT's tick and names the tick to patch at), so
//!   hubs need nothing from each other yet.
//! - [`register_control`]: services on the master's control socket. The CLI reaches nodes through
//!   the hub's `route` (signing orders itself with the master's key), so nothing is added.
//! - [`cli`]: `makepad devhub sync|launch|stop|launched|launch-log|wm-restart …`.
//! - [`start_autosync`]: the master's watcher of work and commercial main, one node per round.
//!
//! How it fits together (replacing local/deploy.sh and fleetmp/*):
//! 1. `makepad devhub sync push <nodes>`: the master bundles `work` (+ commercial `main`, or any
//!    lane branch) against what each node's store already holds, signs a push order bound to the
//!    bundle's hash, streams it over the hub link; the node verifies and fetches it into
//!    `~/devhub/git/<repo>.git`.
//! 2. A signed apply moves the synced checkout (`~/makepad`, + apps/commercial) by fast-forward
//!    only (`read-tree -m -u`: only changed files are written; local edits make it refuse). Running
//!    JITs live-patch from the changed files; their outcome is reported per JIT.
//! 3. The prebuild runs with the WM's launch environment (build.rs), so launches compile nothing.
//! 4. A restart, when asked or when the WM binary changed (`restart=auto`): the WM's service,
//!    through the sudoers rule (the AI Hub restarts with the WM).
//! 5. Devices (no checkout, `paired` credential): the host installs a [`SourceStore`]; the master
//!    sends the files that changed (computed from git on the master) over `sync-files`, they go
//!    into the JIT's source store and its live patch ([`store`]).
//! 6. Coordinated patch: `--session <node>/<pid>,…` waits on every node until its JIT reaches the
//!    same session tick, then moves the files there.

#[cfg(feature = "checkout")]
mod autosync;
#[cfg(feature = "checkout")]
mod build;
#[cfg(feature = "checkout")]
mod cli;
#[cfg(feature = "checkout")]
mod conf;
pub mod files;
#[cfg(feature = "checkout")]
mod git;
#[cfg(feature = "checkout")]
mod jits;
#[cfg(feature = "checkout")]
mod launch;
#[cfg(feature = "checkout")]
mod serve;
mod sign;
pub mod store;

use makepad_remote::Endpoint;

#[cfg(feature = "checkout")]
pub use conf::SyncConf;
pub use store::{set_source_store, SourceStore};

/// What a node gives sync: its name and the records sink.
pub struct NodeCtx {
    pub machine: String,
    pub publish: crate::local::Publish,
}

/// The applier: a JIT host's source store when it installed one ([`set_source_store`]: a device
/// with no checkout), else the synced checkout.
pub fn register_master(ep: &Endpoint, ctx: &std::sync::Arc<NodeCtx>) {
    if let Some(s) = store::source_store() {
        return store::StoreSync::new(ctx.clone(), s, None).register(ep);
    }
    #[cfg(feature = "checkout")]
    serve::Sync::new(ctx.clone(), SyncConf::load()).register(ep);
}

/// Leases: have commit `sha` (pinned from `branch`) of `repo` (makepad | commercial, from the local repo at `dir`) in
/// `node`'s store; returns its commit (`control`: the master's control socket client).
#[cfg(feature = "checkout")]
pub fn ensure_rev(control: &makepad_remote::Client, node: &str, repo: &str, dir: &std::path::Path, branch: &str, sha: &str) -> Result<String, String> {
    cli::ensure_rev(control, node, repo, dir, branch, sha)
}

/// The node's bare store of `repo` (`~/devhub/git/<repo>.git`): where lease worktrees come from.
#[cfg(feature = "checkout")]
pub fn store_dir(repo: &str) -> std::path::PathBuf {
    git::stores_dir().join(format!("{repo}.git"))
}

pub fn register_peer(_ep: &Endpoint, _ctx: &std::sync::Arc<NodeCtx>) {}

pub fn register_control(_ep: &Endpoint, _hub: &makepad_remote::hub::Hub) {}

/// Starts the master's watcher when this hub has checkout support.
pub fn start_autosync(sock: std::path::PathBuf) {
    #[cfg(feature = "checkout")]
    autosync::start(sock);
    #[cfg(not(feature = "checkout"))]
    let _ = sock;
}

/// The log of a running launch of this node (None: not one of its launches).
pub fn launch_log(pid: u32) -> Option<std::path::PathBuf> {
    #[cfg(feature = "checkout")]
    return launch::ledger(pid).map(|(_, _, log)| log);
    #[cfg(not(feature = "checkout"))]
    {
        let _ = pid;
        None
    }
}

/// The sync words for the top-level help (empty without the checkout applier).
pub fn help() -> &'static str {
    #[cfg(feature = "checkout")]
    return cli::HELP;
    #[cfg(not(feature = "checkout"))]
    ""
}

pub fn cli(args: &[&str]) -> Option<i32> {
    #[cfg(feature = "checkout")]
    return cli::main(args);
    #[cfg(not(feature = "checkout"))]
    {
        let _ = args;
        None
    }
}
