//! Code sync, remote launch and WM restart: lane DEVHUB2 owns everything under `src/sync/`
//! (DESIGN.md §11 has the file ownership). The backbone calls these hooks; nothing
//! else in the crate needs to change for sync to grow.
//!
//! - [`register_master`]: services on a node's MASTER endpoint (the master only): `sync`
//!   (`status`, `apply`, `wm-restart`, `tick`), the bundle channel `sync-push`, and `launch`.
//! - [`register_peer`]: services on a node's PEER endpoint (other hubs). Coordinated patches are
//!   driven by the master (it reads every session JIT's tick and names the tick to patch at), so
//!   hubs need nothing from each other yet.
//! - [`register_control`]: services on the master's control socket: `autosync` (`now <landed ms>`:
//!   a landing kicks the watcher). The CLI reaches nodes through the hub's `route` (signing orders
//!   itself with the master's key).
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
pub(crate) mod git;
#[cfg(feature = "checkout")]
mod jits;
#[cfg(feature = "checkout")]
pub mod lanes;
#[cfg(feature = "checkout")]
mod launch;
pub mod prewarm;
#[cfg(feature = "checkout")]
pub(crate) mod serve;
mod sign;
pub mod store;
#[cfg(feature = "checkout")]
mod toast;

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

/// Whether the master's auto-sync follows `node` (its `nodes` list, all when it has none).
#[cfg(feature = "checkout")]
pub fn autosync_covers(home: &std::path::Path, node: &str) -> bool {
    let text = std::fs::read_to_string(home.join("autosync.conf")).unwrap_or_default();
    let c = autosync::AutoConf::parse(&text, &makepad_app_registry::repo_root().unwrap_or_default());
    c.enabled && c.nodes.as_ref().is_none_or(|n| n.iter().any(|x| x == node))
}

/// Moves `node`'s tree to the tips of work and commercial main, with no build and no restart (a
/// node the auto-sync does not follow: the self-update brings its code itself). Returns the exit
/// code and the node's lines.
#[cfg(feature = "checkout")]
pub fn push_tree(control: &makepad_remote::Client, repo: &std::path::Path, node: &str) -> (i32, Vec<String>) {
    let mut lines = Vec::new();
    let code = cli::push_and_apply(control, repo, "work", Some("main"), &[node.to_string()], "off", "none", false, "", &mut |n, l| lines.push(format!("{n} {l}")));
    (code, lines)
}

/// The node's bare store of `repo` (`~/devhub/git/<repo>.git`): where lease worktrees come from.
#[cfg(feature = "checkout")]
pub fn store_dir(repo: &str) -> std::path::PathBuf {
    git::stores_dir().join(format!("{repo}.git"))
}

pub fn register_peer(_ep: &Endpoint, _ctx: &std::sync::Arc<NodeCtx>) {}

pub fn register_control(ep: &Endpoint, _hub: &makepad_remote::hub::Hub) {
    #[cfg(feature = "checkout")]
    ep.register(
        "autosync",
        1,
        makepad_remote::lines::service(
            "autosync",
            std::sync::Arc::new(|line, out| match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["now", ms] if ms.parse::<u64>().is_ok() => {
                    autosync::kick(ms.parse().unwrap_or(0));
                    out.push(format!("{{\"kicked\":true,\"landed_ms\":{ms}}}"));
                }
                _ => out.push(format!("{{\"err\":{}}}", makepad_remote::json_str("autosync now <landed wall ms>"))),
            }),
        ),
    );
    #[cfg(not(feature = "checkout"))]
    let _ = ep;
}

/// A landing happened (`landed_ms`): the master's auto-sync starts a round now.
#[cfg(feature = "checkout")]
pub fn kick_autosync(landed_ms: u64) {
    autosync::kick(landed_ms);
}

/// A control service as a call: (line, out).
pub type FlowServe = std::sync::Arc<dyn Fn(&str, &mut Vec<String>) + Send + Sync>;

/// The master's lane merge bot ([`lanes`]): its `lanes` control service and its loop. Both
/// `lanes …` and `prewarm` ([`prewarm::Board`]) as one call for the panel.
pub fn register_flow(ep: &Endpoint, machine: &str, board: std::sync::Arc<prewarm::Board>, publish: crate::local::Publish) -> Option<FlowServe> {
    #[cfg(feature = "checkout")]
    {
        let l = lanes::Lanes::new(machine, crate::proto::home(), publish);
        let l2 = l.clone();
        ep.register("lanes", 1, makepad_remote::lines::service("lanes", std::sync::Arc::new(move |line, out| l2.serve(line, out))));
        l.start();
        Some(std::sync::Arc::new(move |line: &str, out: &mut Vec<String>| {
            if line.trim() == "prewarm" {
                board.serve(out)
            } else {
                l.serve(line, out)
            }
        }))
    }
    #[cfg(not(feature = "checkout"))]
    {
        let _ = (ep, machine, publish);
        Some(std::sync::Arc::new(move |line: &str, out: &mut Vec<String>| {
            if line.trim() == "prewarm" {
                board.serve(out)
            }
        }))
    }
}

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
