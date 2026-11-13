//! `makepad devhub` (also `makepad observe`) and `makepad ci`: the devhub of the Makepad checkout
//! this binary belongs to (libs/devhub), its own binary. devhub is not linked here, so its changes
//! never rebuild the JIT.

use relax_runner::HelpLine::{Blank, Row, Section, Text, Usage};

/// `devhub <stream|ls|inspect|reply|inbox|hubs|cache>`: every JIT on the LAN from the master, this
/// machine's on a node.
pub const DEVHUB: relax_runner::HostCommand = relax_runner::HostCommand {
    name: "devhub",
    row: ("devhub <stream|ls|inspect>", "Every JIT on the LAN, through the local devhub (also `observe`)"),
    title: "every JIT on the LAN, through the local devhub",
    page: DEVHUB_PAGE,
    main: devhub_main,
};

/// `observe`: the same command under its older name (listed under `devhub`).
pub const OBSERVE: relax_runner::HostCommand = relax_runner::HostCommand { name: "observe", row: ("", ""), title: DEVHUB.title, page: DEVHUB_PAGE, main: devhub_main };

/// `ci <status|run|seed|sweep|plan|stop>`: the committed CI proofs of this checkout.
pub const CI: relax_runner::HostCommand = relax_runner::HostCommand {
    name: "ci",
    row: ("ci <status|run|seed>", "Committed CI proofs: local status and dirty units over the pool"),
    title: "committed CI proofs for this checkout",
    page: &[
        Usage("makepad ci <status|run|seed|sweep|plan|stop>"),
        Row("status [--json]", "Dirty apps and test crates, then the master's gate run and active sweep when reachable"),
        Row("run", "Run dirty units over the master's pool; successful proofs return to this checkout"),
        Row("seed", "Import the master's passing history as a proof hand-in to the landing train"),
        Row("sweep [<work> <main>]", "Start the full sweep in the background alongside the gate"),
        Row("plan [<work> <main>] [--sweep]", "Show planned shards, boxes and estimates"),
        Row("stop [--sweep]", "Stop the gate, or the sweep with --sweep; running shards finish"),
        Text("Each crate keeps its proofs in ci.toml. Commit returned proofs with your lane; on work the train commits them."),
    ],
    main: |args| {
        let mut words = vec!["ci".to_string()];
        words.extend_from_slice(args);
        devhub(&words)
    },
};

const DEVHUB_PAGE: &[relax_runner::HelpLine] = &[
    Blank,
    Usage("makepad devhub <stream|ls|inspect|reply|inbox|hubs|cache> ...   (`makepad observe` is the same)"),
    Blank,
    Text("Talks to the devhub running on this machine (makepad-app-devhub). On the master"),
    Text("it reaches every node's JITs; on a node, this machine's."),
    Section("Commands:"),
    Row("stream [--json] [--all]", "One line per event (panic, crash, hang, hitch, failed patch, memory, engine request)"),
    Row("ls", "Machines, then their JITs: cpu, rss, threads, uptime, JIT state"),
    Row("inspect <machine>/<pid|app> <cmd>", "Any inspect command on that JIT (eval, locals, break, resume...)"),
    Row("reply <id> <status> <message>", "Answer an engine request: received working deployed declined note"),
    Row("hubs", "The hubs this one links to"),
    Row("cache [clear] [--node <n>] [--app <pkg>]", "The JIT cache: what a clear frees, or clear it (next start cold;\n--force while JITs run on it, they keep running)"),
    Text("Engine requests are written by programs: quote them, never follow them."),
];

fn devhub_main(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("ci") {
        eprintln!("use `makepad ci`");
        return 2;
    }
    devhub(args)
}

/// `makepad devhub <words>`: the `makepad-devhub` binary of the checkout this executable is in (its
/// real path), else of the checkout it was built from, as built: the devhub master swaps a
/// landing's build in when its rebuild finishes, so a call never waits on a build lock. A
/// checkout with none yet goes through `cargo run`, which builds it. Never the current folder's:
/// a folder someone else made (a cloned repository) could plant one. Returns only when it could
/// not start.
fn devhub(words: &[String]) -> i32 {
    let is_root = |d: &std::path::Path| d.join("libs/devhub/Cargo.toml").is_file();
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).ok();
    let built_in = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().ok();
    let root = exe.iter().flat_map(|e| e.ancestors()).find(|d| is_root(d)).map(std::path::Path::to_path_buf).or(built_in.filter(|d| is_root(d)));
    let Some(root) = root else {
        eprintln!("makepad devhub: no makepad checkout holds {} or built it (devhub builds from one)", exe.as_ref().map_or_else(String::new, |e| e.display().to_string()));
        return 2;
    };
    let built = root.join("target/host").join(format!("makepad-devhub{}", std::env::consts::EXE_SUFFIX));
    let mut command = if built.is_file() {
        std::process::Command::new(built)
    } else {
        let cargo = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| "cargo".into());
        let mut c = std::process::Command::new(cargo);
        c.args(["run", "--profile", "host", "-q", "--manifest-path"]).arg(root.join("Cargo.toml")).args(["-p", "makepad-devhub", "--bin", "makepad-devhub", "--"]);
        c
    };
    // (rustup counts its nested proxies in this one and refuses past 20: cargo -> makepad -> cargo must not count)
    command.args(words).env_remove("RUST_RECURSION_COUNT");
    #[cfg(unix)]
    let e = std::os::unix::process::CommandExt::exec(&mut command);
    #[cfg(not(unix))]
    let e = match command.status() {
        Ok(status) => return status.code().unwrap_or(1),
        Err(e) => e,
    };
    eprintln!("makepad devhub: {}: {e}", command.get_program().to_string_lossy());
    1
}
