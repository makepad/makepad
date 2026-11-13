//! The `makepad` command's help: the main page, one page per command, and the unknown-command
//! error. All help text lives here. Colour is plain ANSI: off when the stream is not a terminal
//! or NO_COLOR is set; CLICOLOR_FORCE=1 forces it on.

use std::io::IsTerminal;

/// The word a command line starts with, for every command `makepad` knows (help pages or not).
pub const COMMANDS: &[&str] = &[
    "run", "app", "fetch", "export", "pack-src", "inspect", "check", "selftest", "test", "live", "fix", "clean", "serve", "schema", "hover", "definition",
    "parse", "resolve", "file", "std-surface", "lex", "hub", "remote-relay", "remote", "help",
];

#[derive(Clone, Copy)]
pub enum Stream {
    Out,
    Err,
}

struct Style(bool);

impl Style {
    fn new(stream: Stream) -> Style {
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let tty = match stream {
            Stream::Out => std::io::stdout().is_terminal(),
            Stream::Err => std::io::stderr().is_terminal(),
        };
        let on = if env("NO_COLOR").is_some() {
            false
        } else if env("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
            true
        } else {
            tty
        };
        Style(on)
    }
    fn paint(&self, code: &str, s: &str) -> String {
        if self.0 { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    fn head(&self, s: &str) -> String {
        self.paint("1;32", s)
    }
    fn name(&self, s: &str) -> String {
        self.paint("1;36", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn red(&self, s: &str) -> String {
        self.paint("1;31", s)
    }
    /// Command words in bold cyan, `<placeholders>` and `[optional]` parts dim.
    fn spec(&self, s: &str) -> String {
        s.split(' ')
            .map(|w| if w.starts_with('<') || w.starts_with('[') { self.dim(w) } else { self.name(w) })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A page under construction: section headers and two-column rows, `width` columns for names.
struct Page<'a> {
    st: &'a Style,
    out: String,
    width: usize,
}

impl<'a> Page<'a> {
    fn new(st: &'a Style, width: usize) -> Page<'a> {
        Page { st, out: String::new(), width }
    }
    fn line(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }
    fn blank(&mut self) {
        self.out.push('\n');
    }
    fn section(&mut self, title: &str) {
        self.blank();
        let h = self.st.head(title);
        self.line(&h);
    }
    /// `  name   description`; continuation lines of a description align under it.
    fn row(&mut self, name: &str, desc: &str) {
        let pad = " ".repeat(self.width.saturating_sub(name.chars().count()).max(1));
        let mut lines = desc.split('\n');
        let first = format!("  {}{}{}", self.st.spec(name), pad, lines.next().unwrap_or(""));
        self.line(&first);
        for l in lines {
            let cont = format!("  {}{}", " ".repeat(self.width), l);
            self.line(&cont);
        }
    }
    fn usage(&mut self, text: &str) {
        let head = format!("{} {}", self.st.head("Usage:"), self.st.spec(text));
        self.line(&head);
    }
    fn text(&mut self, s: &str) {
        self.line(&format!("  {s}"));
    }
    fn footer(&mut self, s: &str) {
        self.blank();
        let d = self.st.dim(s);
        self.line(&d);
    }
}

/// Whether this binary is the bare runner, `relax` (its host is BARE, whose package is this crate),
/// rather than the `makepad` binary, which brings its own host.
pub fn bare() -> bool {
    crate::host().package == env!("CARGO_PKG_NAME")
}

/// The command this binary is run as, for titles, usage lines and hints.
pub fn command() -> &'static str {
    if bare() { "relax" } else { "makepad" }
}

fn title(p: &mut Page, what: &str) {
    let t = format!("{} {}", p.st.name(command()), p.st.dim(&format!("{} (experimental)", env!("CARGO_PKG_VERSION"))));
    p.line(&format!("{t} \u{2014} {what}"));
}

/// `relax`'s main page: the commands of the bare runner, which runs any Rust package.
fn relax_main_page(st: &Style) -> String {
    let mut p = Page::new(st, 29);
    title(&mut p, "run a Rust crate from source through the Relax JIT");
    p.blank();
    p.usage(&format!("{} <command> [options]", command()));
    p.section("Commands:");
    p.row("run <package>", "Run a package of the workspace in this folder");
    p.row("run <git-url> <package>", "Run a package straight from a git repository");
    p.row("test <package>", "Run a package's tests");
    p.row("test <file.rs> [filter]", "Run the tests of one file");
    p.row("check <package>", "Type- and borrow-check without running");
    p.row("live <file.rs> <edit.rs>...", "Run tests, then apply edits as live patches");
    p.row("selftest <package>", "Test Relax itself: compile every fn of a package, list failures");
    p.row("fix --to-strict <package>", "Rewrite relaxed code to plain rustc Rust");
    p.row("fetch <git-url>", "Fetch a repository into the local store");
    p.row("export <git-url> <folder>", "Write a repository's files into a folder");
    p.row("inspect <pid> <question>", "Ask a running program about itself (hangs, stacks, events)");
    p.row("clean [-p <package>]", "Remove the workspace's target/relax cache");
    host_rows(&mut p);
    p.row("serve", "Run the warm check server");
    p.row("help [command]", "Show this help, or a command's page");
    p.section("Options:");
    p.width = 16;
    p.row("--verbose", "Phase timings and info (also RELAX_VERBOSE=1)");
    p.row("--quiet", "Hide the compile-error stream");
    p.row("--no-asm", "Refuse inline assembly (asm!/global_asm!/naked_asm!),\nfor sandboxed uses (also RELAX_NO_ASM=1)");
    p.row("--allow-c", "Build C dependencies with cargo and the system C\ncompiler and link them in (also RELAX_ALLOW_C=1)");
    p.row("--version", "Print the version and exit");
    p.row("-h, --help", "Print this help");
    p.section("Environment:");
    p.row("RELAX_HOME", "Where git stores live (default ~/.relax)");
    p.row("NO_COLOR", "Turn colour off (CLICOLOR_FORCE=1 forces it on)");
    p.footer(&format!("Run `{} help <command>` for details. What does not work yet: libs/relax/KNOWN_GAPS.md", command()));
    p.out
}

fn main_page(st: &Style) -> String {
    if bare() {
        return relax_main_page(st);
    }
    let mut p = Page::new(st, 29);
    title(&mut p, "fast JIT loader for makepad Rust apps");
    p.blank();
    p.usage("makepad <command> [options]");
    p.section("Commands:");
    p.row("run <package>", "Run an app from this workspace");
    p.row("run <git-url> <package>", "Run an app straight from a git repository");
    p.row("run --src <store> <package>", "Run an app from a packed source archive (no checkout)");
    p.row("app [--home <dir>] <id>", "Run an app the runner set up (its launch record; macOS: from its own .app)");
    p.row("fetch <git-url>", "Fetch a repository into the local store");
    p.row("export <git-url> <folder>", "Write a repository's files (and the app's edits) into a folder");
    p.row("pack-src <out> <package>...", "Pack the sources a run of these packages reads into one store");
    p.row("inspect <app> <question>", "Ask a running app about itself (hangs, stacks, events)");
    p.row("hub | remote-relay | remote", "Reach apps on other machines (see makepad help inspect)");
    p.row("check <package>", "Type- and borrow-check without running");
    p.row("check --src <store> <package>", "Check an app from a packed source archive, reading only it");
    p.row("selftest <package>", "Test Relax itself: compile every fn of a package, list failures");
    p.row("test <file.rs> [filter]", "Run a crate's tests");
    p.row("live <file.rs> <edit.rs>...", "Run tests, then apply edits as live patches");
    p.row("fix --to-strict <package>", "Rewrite relaxed code to plain rustc Rust");
    p.row("clean [-p <package>]", "Remove the workspace's target/relax cache");
    host_rows(&mut p);
    p.row("serve", "Run the warm check server");
    p.row("schema|hover|definition", "Editor queries to the check server");
    p.row("help [command]", "Show this help, or a command's page");
    p.section("Options:");
    p.width = 16;
    p.row("--verbose", "Phase timings and info (also RELAX_VERBOSE=1)");
    p.row("--quiet", "Hide the compile-error stream");
    p.row("--no-asm", "Refuse inline assembly (asm!/global_asm!/naked_asm!),\nfor sandboxed uses (also RELAX_NO_ASM=1)");
    p.row("--allow-c", "Build C dependencies with cargo and the system C\ncompiler and link them in (also RELAX_ALLOW_C=1)");
    p.row("--version", "Print the version and exit");
    p.row("-h, --help", "Print this help");
    p.section("Environment:");
    p.row("RELAX_HOME", "Where stores and caches live (default ~/.relax)");
    p.row("NO_COLOR", "Turn colour off (CLICOLOR_FORCE=1 forces it on)");
    p.footer("Run `makepad help <command>` for details.");
    p.out
}

fn run_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "run an app");
    p.blank();
    p.usage(&format!("{tool} run [options] <package> [cargo flags] [-- args]", tool = command()));
    p.line(&format!("       {}", st.spec(&format!("{} run [options] <git-url>[#ref] [--mount <dir>=<git-url>[#ref]]...", command()))));
    p.line(&format!("           {}", st.spec("[--cached] <package> [cargo flags] [-- args]")));
    p.blank();
    p.text("Runs the package of the workspace in the current folder, or of a git");
    p.text("repository (smart HTTP). Nothing is checked out. Program arguments");
    p.text("go after `--`.");
    p.section("Options:");
    p.row("--verbose", "Phase timings and info (also RELAX_VERBOSE=1)");
    p.row("--quiet", "Hide the compile-error stream");
    p.row("--ahead=<level>", "Compile ahead of the calls: minimal (only what runs,\nfor editing), balanced (default: what the UI is\nabout to need), aot-max (a background pass over\neverything reachable, all workers) or aot-min (the\nsame pass slowly, in the OS background class); also\nRELAX_AHEAD");
    p.row("--check-first", "Borrow-check each function before it first runs (the\ndefault checks in the background and never waits)");
    p.row("--units <json>", "Use a saved unit graph instead of cargo");
    p.row("--agent", "Park panics for an agent; print events on stderr");
    p.row("--unattended", "Never park: a panic unwinds at once and a compile\nerror ends the run (scripts, CI); no live session,\nso a saved edit is not patched in");
    p.row("--heap[=<bytes>]", "Track the heap by stack from the start (inspect heap)");
    p.row("--cfg <spec>", "Set a cfg for the program (name or name=\"value\"), as RUSTFLAGS=--cfg");
    p.row("--no-asm", "Refuse the program's inline assembly (asm!,\nglobal_asm!, naked_asm!: an error naming the crate),\nfor sandboxed uses; also RELAX_NO_ASM=1");
    p.row("--allow-c", "Build dependencies that compile C with cargo and the\nsystem C compiler; also RELAX_ALLOW_C=1");
    p.row("--privileged", "Grant the app the HAL's domain control: it may start,\nstop and embed other apps as domains (a window\nmanager)");
    p.row("--relaxed[=<list>]", "Relaxed Rust for every crate (all, or a list)");
    p.row("--mount <dir>=<git-url>", "Mount another repository at a folder of the\nfirst (repeatable); an optional #ref pins it");
    p.row("--cached", "Run the newest local snapshot at once, and\nrefresh it in the background for next time");
    p.section("Build:");
    p.text("A run builds as cargo's release profile does (no debug assertions or");
    p.text("overflow checks), so build scripts see PROFILE=release, OPT_LEVEL=3 and");
    p.text("DEBUG=false; `cargo run` builds dev. env!(\"CARGO\"), proc macros and the");
    p.text("program read CARGO as the absolute path of the cargo on PATH (or of the one");
    p.text("that started the run), build scripts the loader (which answers `metadata`).");
    p.section("Cache:");
    p.text("A workspace run keeps its cache and build files in the workspace's");
    p.text("target/relax (like cargo). A git run has no checkout and keeps");
    p.text("them in $RELAX_HOME/target/<hash>. Repositories are one store");
    p.text("file each in $RELAX_HOME/cache (default ~/.relax).");
    p.out
}

fn pack_src_page(st: &Style) -> String {
    let mut p = Page::new(st, 36);
    title(&mut p, "pack the sources of apps into one store");
    p.blank();
    p.usage(&format!("{tool} pack-src [--target <triple>] [--fonts <manifest>] <out.store> <package> [flags] [<package> [flags]]...", tool = command()));
    p.blank();
    p.text(&format!("Runs `{tool} check` of each package cold, records every file the front end", tool = command()));
    p.text("reads (the crate graph and cfg of --target, build scripts included), adds the");
    p.text("resources/ folders of the crates it read sources of and Relax's std tree, and");
    p.text("writes ONE synthetic commit into a one-file git store: git blob ids, sorted");
    p.text("trees, a fixed author, date and message, objects in id order, a fixed deflate");
    p.text(&format!("level, so the same files give the same bytes. `{tool} run --src <out.store>", tool = command()));
    p.text("<package>` runs from it with no checkout; the store may also sit inside a");
    p.text("bigger file: --src <file>@<offset>+<len> (an APK asset).");
    p.section("Options:");
    p.row("--target <triple>", "Pack the crate graph of that target (RELAX_CHECK_TARGET)");
    p.row("<package> [flags]", "--features <list>, --no-default-features, as for check");
    p.out
}

fn export_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "export a working tree");
    p.blank();
    p.usage(&format!("{tool} export <git-url>[#ref] [--mount <dir>=<git-url>[#ref]]... [--cached] <folder>", tool = command()));
    p.usage(&format!("{tool} export --mounted <folder>", tool = command()));
    p.blank();
    p.text("Writes the repositories' files into <folder> (each mounted one at its");
    p.text("folder), then the files of $RELAX_EDITS over them: the tree a run of");
    p.text("the same spec reads, for a native build. Unchanged files are not rewritten;");
    p.text("files an earlier export wrote and this one does not are removed. --mounted");
    p.text("writes out what this run's own virtual workspace has mounted.");
    p.out
}

fn fetch_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "fetch a repository");
    p.blank();
    p.usage(&format!("{tool} fetch <git-url>[#ref] [--mount <dir>=<git-url>[#ref]]...", tool = command()));
    p.blank();
    p.text("Fetches the repository (and any mounted ones) into the local store and");
    p.text("prints one line per repository: folder, store file, commit.");
    p.section("Options:");
    p.row("--mount <dir>=<git-url>", "Also fetch a repository mounted at <dir>");
    p.section("Store:");
    p.text("Each repository is one file, $RELAX_HOME/cache/git-<name>-<hash>.store");
    p.text("(default ~/.relax); the two newest fetches per ref are kept, and");
    p.text("nothing is checked out. Offline, the newest kept commits are used.");
    p.out
}

fn check_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "type- and borrow-check");
    p.blank();
    p.usage(&format!("{tool} check [options] <package>", tool = command()));
    p.blank();
    p.text("Type- and borrow-checks every function of the package's workspace crates,");
    p.text("like `cargo check`; compiles and runs nothing. Errors go to stderr.");
    p.section("Options:");
    p.row("--json", "One JSON object per diagnostic, then a summary");
    p.row("--workspace", "Check every workspace crate the package reaches, not only its own");
    p.row("--also=<pkg,..>", "Also check these packages' crates as this package's build compiles them (its features)");
    p.row("--serve", "Start (or use) the warm check server");
    p.row("--no-serve", "Never use the check server");
    p.row("--watch", "Re-check what each edit invalidates");
    p.row("--deep", "Also compile (with --watch)");
    p.row("--cfg <spec>", "Set a cfg for the program (name or name=\"value\"), as RUSTFLAGS=--cfg");
    p.row("--src <store>", "Check from a packed source archive (pack-src), reading nothing else");
    p.text(&format!("The other `{tool} run` options apply too.", tool = command()));
    p.out
}

fn selftest_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "test Relax on a package");
    p.blank();
    p.usage(&format!("{tool} selftest [--root-only] <package>", tool = command()));
    p.blank();
    p.text("A test of Relax itself: compiles every function of the package's crates");
    p.text("(and every instance, closure and drop glue they reach) through the whole");
    p.text("pipeline to machine code, runs nothing, and lists every failure, one line");
    p.text("each; `internal:` marks a Relax bug. Slow: minutes for a large app.");
    p.section("Options:");
    p.row("--root-only", "Start from the root crate's functions only");
    p.out
}

fn test_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "run tests");
    p.blank();
    p.usage(&format!("{tool} test <file.rs> [filter]", tool = command()));
    p.usage(&format!("{tool} test <package> [--release | --profile <name>] [--lib|--bins|--tests] [-- test arguments]", tool = command()));
    p.blank();
    p.text("Runs the crate's tests in-process; the optional filter is passed to the");
    p.text("test harness.");
    p.text("A package's tests build with cargo test's profile: `test` (debug assertions and overflow");
    p.text("checks as the workspace's [profile.test] / [profile.dev] set them), --release the release one.");
    p.text("Build scripts see that profile's PROFILE, OPT_LEVEL and DEBUG, as under cargo.");
    p.text("A file's tests build as rustc -O --test.");
    p.row("-- --precompile", "Build the selected tests and all they call before the first one starts (no compile inside a test's deadlines)");
    p.usage(&format!("{tool} test --live <package> [--lib | --test <name>] [-- <test args>]", tool = command()));
    p.text("Keeps one target warm (default --lib); body edits rerun tests that entered the changed functions, including inlinees.");
    p.text("Non-body edits, unknown reach or changed unattributed code rerun all; the verdict says why.");
    p.text(&format!("Each run prints testverdict; {tool} inspect <pid> watch on=testverdict follows its JSON verdicts.", tool = command()));
    p.text(&format!("Each test's outcome, and whether it ran for that edit: {tool} inspect <pid> test-results group=<verdict group> --json.", tool = command()));
    p.text("Hot-load failures restart the test process; only the replacement tests give a verdict.");
    p.out
}

fn live_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "live patching");
    p.blank();
    p.usage(&format!("{tool} live <v0.rs> <edit1.rs>...", tool = command()));
    p.blank();
    p.text("Runs the crate's tests, then applies each edit as a live patch (only");
    p.text("changed functions recompile) and reruns. A panic in patched code rolls");
    p.text("the patch back and reruns.");
    p.out
}

fn fix_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "relaxed to strict Rust");
    p.blank();
    p.usage(&format!("{tool} fix --to-strict [options] <package>", tool = command()));
    p.blank();
    p.text("Checks in relaxed mode (all features unless --relaxed=<list> follows) and");
    p.text("writes the plain rustc form of every accepted relaxation back into the");
    p.text("sources.");
    p.out
}

fn serve_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "the warm check server");
    p.blank();
    p.usage(&format!("{tool} serve [--workspace <dir>] [--idle-min 30]", tool = command()));
    p.blank();
    p.text(&format!("Keeps a warm session that `{tool} check` and the editor queries use:", tool = command()));
    p.text(&format!("{tool} schema [--serve] <package> <item>", tool = command()));
    p.text(&format!("{tool} hover|definition [--serve] <package> <file> <line> <col>", tool = command()));
    p.out
}

fn clean_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "remove cached build files");
    p.blank();
    p.usage(&format!("{tool} clean [-p <package>]", tool = command()));
    p.blank();
    p.text("Removes the workspace's <target>/relax folder: the code cache, unit");
    p.text("graphs and build script outputs of every app.");
    p.section("Options:");
    p.row("-p, --package <name>", "Only that app's files: its startup manifest\nand stores. Stores shared with other apps,\nregistry packages and build script outputs\nstay.");
    p.section("Notes:");
    p.text("A git run's virtual workspace has no target folder to clean. Its files");
    p.text("are in $RELAX_HOME/target/<hash>. RELAX_CACHE_DIR makes clean refuse.");
    p.out
}

fn inspect_page(st: &Style) -> String {
    let mut p = Page::new(st, 34);
    title(&mut p, "ask a running app about itself");
    p.blank();
    p.usage(&format!("{tool} inspect <pid|list> <command> [k=v]...", tool = command()));
    p.blank();
    p.text(&format!("Every `{tool} run` listens on a local socket. Answers are JSON lines,", tool = command()));
    p.text("pre-analysed and bounded, each starting with what was measured.");
    p.section("Questions:");
    p.row("why-hung", "Verdict, wait chain or cycle, stacks");
    p.row("watch [on=panic,hang,..]", "Wait for the next event, print it, exit");
    p.row("events follow=1", "Stream events, one line each");
    p.row("threads | stacks | waits", "Threads, symbolised stacks, waits");
    p.row("panics | resume <id> panic", "Parked panics");
    p.row("help schema", "Tool definition for agents");
    p.text("At a terminal, or for the apps of your own lease, an app's name works in place of");
    p.text("its pid, and `all` covers every such app, also ones started later. Anything else is");
    p.text("refused and says which pid it would have reached: an agent names the pid.");
    p.section("Other machines:");
    p.row("<host>:<pid> <command>", "Ask an app on a host of hosts.toml (via the hub)");
    p.row("<host>:list", "That host's running apps");
    p.row("<host>:session launch <app>", "Start a hidden session there (kill <pid> ends it, also after the hub restarted)");
    p.row("<host>:session relaunch <pid>", "Restart its app (a fresh process; an app that restarts itself keeps its state)");
    p.row("hosts", "Registry, link state and round trip");
    p.row("all:list", "Apps here and on every host");
    p.text(&format!("Hosts are added with `{tool} remote add <name> --ssh user@host` or paired with", tool = command()));
    p.text(&format!("`{tool} remote pair <name> --hub <addr>` (the device dials `{tool} hub --listen`).", tool = command()));
    p.text("`--env NAME=value` on `remote add` (hosts.toml: env = { DISPLAY = \":0\" }) sets the environment of its sessions.");
    p.out
}

/// The main page's rows of the commands the host adds (an empty row: another name of the command
/// listed beside it).
fn host_rows(p: &mut Page) {
    for c in crate::host().commands.iter().filter(|c| !c.row.0.is_empty()) {
        p.row(c.row.0, c.row.1);
    }
}

/// `help <name>` of a command the host adds.
fn host_page(st: &Style, c: &crate::HostCommand) -> String {
    let mut p = Page::new(st, 40);
    title(&mut p, c.title);
    for line in c.page {
        match *line {
            crate::HelpLine::Blank => p.blank(),
            crate::HelpLine::Usage(s) => p.usage(s),
            crate::HelpLine::Text(s) => p.text(s),
            crate::HelpLine::Section(s) => p.section(s),
            crate::HelpLine::Row(name, desc) => p.row(name, desc),
        }
    }
    p.out
}

/// The page for `makepad help [command]`; None for a word that is no command.
fn page(st: &Style, cmd: Option<&str>) -> Option<String> {
    Some(match cmd {
        None | Some("help") => main_page(st),
        Some("run") => run_page(st),
        Some("fetch") => fetch_page(st),
        Some("export") => export_page(st),
        Some("pack-src") => pack_src_page(st),
        Some("inspect") => inspect_page(st),
        Some("check") => check_page(st),
        Some("selftest") => selftest_page(st),
        Some("test") => test_page(st),
        Some("live") => live_page(st),
        Some("fix") => fix_page(st),
        Some("serve" | "schema" | "hover" | "definition") => serve_page(st),
        Some("clean") => clean_page(st),
        Some(c) => match crate::host().commands.iter().find(|h| h.name == c) {
            Some(h) => host_page(st, h),
            None if COMMANDS.contains(&c) => main_page(st),
            None => return None,
        },
    })
}

/// Prints the main help to `stream`.
pub fn print_main(stream: Stream) {
    emit(stream, &main_page(&Style::new(stream)));
}

/// `makepad help [command]`; exit code.
pub fn help(args: &[String]) -> i32 {
    let st = Style::new(Stream::Out);
    match page(&st, args.first().map(String::as_str)) {
        Some(text) => {
            emit(Stream::Out, &text);
            0
        }
        None => unknown(args[0].as_str()),
    }
}

/// One red error line plus a hint, on stderr; exit code.
pub fn unknown(word: &str) -> i32 {
    let st = Style::new(Stream::Err);
    eprintln!("{} unknown command `{}`", st.red("error:"), word);
    eprintln!();
    eprintln!("  {}", st.dim(&format!("Run `{} help` to list the commands.", command())));
    2
}

fn emit(stream: Stream, text: &str) {
    match stream {
        Stream::Out => print!("{text}"),
        Stream::Err => eprint!("{text}"),
    }
}
