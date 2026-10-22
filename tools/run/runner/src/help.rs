//! The `makepad` command's help: the main page, one page per command, and the unknown-command
//! error. All help text lives here. Colour is plain ANSI: off when the stream is not a terminal
//! or NO_COLOR is set; CLICOLOR_FORCE=1 forces it on.

use std::io::IsTerminal;

/// The word a command line starts with, for every command `makepad` knows (help pages or not).
pub const COMMANDS: &[&str] = &[
    "run", "app", "fetch", "pack-src", "inspect", "check", "selftest", "test", "live", "fix", "clean", "serve", "schema", "hover", "definition",
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

fn title(p: &mut Page, what: &str) {
    let t = format!("{} {}", p.st.name("makepad"), p.st.dim(&format!("{} (beta)", env!("CARGO_PKG_VERSION"))));
    p.line(&format!("{t} \u{2014} {what}"));
}

fn main_page(st: &Style) -> String {
    let mut p = Page::new(st, 29);
    title(&mut p, "fast JIT loader for makepad Rust apps");
    p.blank();
    p.usage("makepad <command> [options]");
    p.section("Commands:");
    p.row("run <package>", "Run an app from this workspace");
    p.row("run <git-url> <package>", "Run an app straight from a git repository");
    p.row("run --src <store> <package>", "Run an app from a packed source archive (no checkout)");
    p.row("app <id>", "Run an app the runner set up (its launch record; macOS: from its own .app)");
    p.row("fetch <git-url>", "Fetch a repository into the local store");
    p.row("pack-src <out> <package>...", "Pack the sources a run of these packages reads into one store");
    p.row("inspect <app> <question>", "Ask a running app about itself (hangs, stacks, events)");
    p.row("hub | remote-relay | remote", "Reach apps on other machines (see makepad help inspect)");
    p.row("check <package>", "Type- and borrow-check without running");
    p.row("check --src <store> <package>", "Check an app from a packed source archive, reading only it");
    p.row("selftest <package>", "Test makepad run itself: compile every fn of a package, list failures");
    p.row("test <file.rs> [filter]", "Run a crate's tests");
    p.row("live <file.rs> <edit.rs>...", "Run tests, then apply edits as live patches");
    p.row("fix --to-strict <package>", "Rewrite relaxed code to plain rustc Rust");
    p.row("clean [-p <package>]", "Remove the workspace's target/makepad-run cache");
    p.row("serve", "Run the warm check server");
    p.row("schema|hover|definition", "Editor queries to the check server");
    p.row("help [command]", "Show this help, or a command's page");
    p.section("Options:");
    p.width = 16;
    p.row("--verbose", "Phase timings and info (also MAKEPAD_RUN_VERBOSE=1)");
    p.row("--quiet", "Hide the compile-error stream");
    p.row("--no-asm", "Refuse inline assembly (asm!/global_asm!/naked_asm!),\nfor sandboxed uses (also MAKEPAD_RUN_NO_ASM=1)");
    p.row("--allow-c", "Build C dependencies with cargo and the system C\ncompiler and link them in (also MAKEPAD_RUN_ALLOW_C=1)");
    p.row("--version", "Print the version and exit");
    p.row("-h, --help", "Print this help");
    p.section("Environment:");
    p.row("MAKEPAD_RUN_HOME", "Where stores and caches live (default ~/.makepad-run)");
    p.row("NO_COLOR", "Turn colour off (CLICOLOR_FORCE=1 forces it on)");
    p.footer("Run `makepad help <command>` for details.");
    p.out
}

fn run_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "run an app");
    p.blank();
    p.usage("makepad run [options] <package> [cargo flags] [-- args]");
    p.line(&format!("       {}", st.spec("makepad run [options] <git-url>[#ref] [--mount <dir>=<git-url>[#ref]]...")));
    p.line(&format!("           {}", st.spec("[--cached] <package> [cargo flags] [-- args]")));
    p.blank();
    p.text("Runs the package of the workspace in the current folder, or of a git");
    p.text("repository (smart HTTP). Nothing is checked out. Program arguments");
    p.text("go after `--`.");
    p.section("Options:");
    p.row("--verbose", "Phase timings and info (also MAKEPAD_RUN_VERBOSE=1)");
    p.row("--quiet", "Hide the compile-error stream");
    p.row("--ahead=<level>", "Compile ahead of the calls: minimal (only what runs,\nfor editing), balanced (default: what the UI is\nabout to need), aot-max (a background pass over\neverything reachable, all workers) or aot-min (the\nsame pass slowly, in the OS background class); also\nMAKEPAD_RUN_AHEAD");
    p.row("--check-first", "Borrow-check each function before it first runs (the\ndefault checks in the background and never waits)");
    p.row("--units <json>", "Use a saved unit graph instead of cargo");
    p.row("--agent", "Park panics for an agent; print events on stderr");
    p.row("--heap[=<bytes>]", "Track the heap by stack from the start (inspect heap)");
    p.row("--cfg <spec>", "Set a cfg for the program (name or name=\"value\"), as RUSTFLAGS=--cfg");
    p.row("--no-asm", "Refuse the program's inline assembly (asm!,\nglobal_asm!, naked_asm!: an error naming the crate),\nfor sandboxed uses; also MAKEPAD_RUN_NO_ASM=1");
    p.row("--allow-c", "Build dependencies that compile C with cargo and the\nsystem C compiler; also MAKEPAD_RUN_ALLOW_C=1");
    p.row("--privileged", "Grant the app the HAL's domain control: it may start,\nstop and embed other apps as domains (a window\nmanager)");
    p.row("--relaxed[=<list>]", "Relaxed Rust for every crate (all, or a list)");
    p.row("--mount <dir>=<git-url>", "Mount another repository at a folder of the\nfirst (repeatable); an optional #ref pins it");
    p.row("--cached", "Run the newest local snapshot at once, and\nrefresh it in the background for next time");
    p.section("Cache:");
    p.text("A workspace run keeps its cache and build files in the workspace's");
    p.text("target/makepad-run (like cargo). A git run has no checkout and keeps");
    p.text("them in $MAKEPAD_RUN_HOME/target/<hash>. Repositories are one store");
    p.text("file each in $MAKEPAD_RUN_HOME/cache (default ~/.makepad-run).");
    p.out
}

fn pack_src_page(st: &Style) -> String {
    let mut p = Page::new(st, 36);
    title(&mut p, "pack the sources of apps into one store");
    p.blank();
    p.usage("makepad pack-src [--target <triple>] <out.store> <package> [flags] [<package> [flags]]...");
    p.blank();
    p.text("Runs `makepad check` of each package cold, records every file the front end");
    p.text("reads (the crate graph and cfg of --target, build scripts included), adds the");
    p.text("resources/ folders of the crates it read sources of and makepad run's std tree, and");
    p.text("writes ONE synthetic commit into a one-file git store: git blob ids, sorted");
    p.text("trees, a fixed author, date and message, objects in id order, a fixed deflate");
    p.text("level, so the same files give the same bytes. `makepad run --src <out.store>");
    p.text("<package>` runs from it with no checkout; the store may also sit inside a");
    p.text("bigger file: --src <file>@<offset>+<len> (an APK asset).");
    p.section("Options:");
    p.row("--target <triple>", "Pack the crate graph of that target (MAKEPAD_RUN_CHECK_TARGET)");
    p.row("<package> [flags]", "--features <list>, --no-default-features, as for check");
    p.out
}

fn fetch_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "fetch a repository");
    p.blank();
    p.usage("makepad fetch <git-url>[#ref] [--mount <dir>=<git-url>[#ref]]...");
    p.blank();
    p.text("Fetches the repository (and any mounted ones) into the local store and");
    p.text("prints one line per repository: folder, store file, commit.");
    p.section("Options:");
    p.row("--mount <dir>=<git-url>", "Also fetch a repository mounted at <dir>");
    p.section("Store:");
    p.text("Each repository is one file, $MAKEPAD_RUN_HOME/cache/git-<name>-<hash>.store");
    p.text("(default ~/.makepad-run); the two newest fetches per ref are kept, and");
    p.text("nothing is checked out. Offline, the newest kept commits are used.");
    p.out
}

fn check_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "type- and borrow-check");
    p.blank();
    p.usage("makepad check [options] <package>");
    p.blank();
    p.text("Type- and borrow-checks every function of the package's workspace crates,");
    p.text("like `cargo check`; compiles and runs nothing. Errors go to stderr.");
    p.section("Options:");
    p.row("--json", "One JSON object per diagnostic, then a summary");
    p.row("--workspace", "Check every workspace crate the package reaches (platform, widgets, ...)");
    p.row("--serve", "Start (or use) the warm check server");
    p.row("--no-serve", "Never use the check server");
    p.row("--watch", "Re-check what each edit invalidates");
    p.row("--deep", "Also compile (with --watch)");
    p.row("--cfg <spec>", "Set a cfg for the program (name or name=\"value\"), as RUSTFLAGS=--cfg");
    p.row("--src <store>", "Check from a packed source archive (pack-src), reading nothing else");
    p.text("The other `makepad run` options apply too.");
    p.out
}

fn selftest_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "test makepad run on a package");
    p.blank();
    p.usage("makepad selftest [--root-only] <package>");
    p.blank();
    p.text("A test of makepad run itself: compiles every function of the package's crates");
    p.text("(and every instance, closure and drop glue they reach) through the whole");
    p.text("pipeline to machine code, runs nothing, and lists every failure, one line");
    p.text("each; `internal:` marks a makepad run bug. Slow: minutes for a large app.");
    p.section("Options:");
    p.row("--root-only", "Start from the root crate's functions only");
    p.out
}

fn test_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "run tests");
    p.blank();
    p.usage("makepad test <file.rs> [filter]");
    p.blank();
    p.text("Runs the crate's tests in-process; the optional filter is passed to the");
    p.text("test harness.");
    p.out
}

fn live_page(st: &Style) -> String {
    let mut p = Page::new(st, 16);
    title(&mut p, "live patching");
    p.blank();
    p.usage("makepad live <v0.rs> <edit1.rs>...");
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
    p.usage("makepad fix --to-strict [options] <package>");
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
    p.usage("makepad serve [--workspace <dir>] [--idle-min 30]");
    p.blank();
    p.text("Keeps a warm session that `makepad check` and the editor queries use:");
    p.text("makepad schema [--serve] <package> <item>");
    p.text("makepad hover|definition [--serve] <package> <file> <line> <col>");
    p.out
}

fn clean_page(st: &Style) -> String {
    let mut p = Page::new(st, 28);
    title(&mut p, "remove cached build files");
    p.blank();
    p.usage("makepad clean [-p <package>]");
    p.blank();
    p.text("Removes the workspace's <target>/makepad-run folder: the code cache, unit");
    p.text("graphs and build script outputs of every app.");
    p.section("Options:");
    p.row("-p, --package <name>", "Only that app's files: its startup manifest\nand stores. Stores shared with other apps,\nregistry packages and build script outputs\nstay.");
    p.section("Notes:");
    p.text("A git run's virtual workspace has no target folder to clean. Its files");
    p.text("are in $MAKEPAD_RUN_HOME/target/<hash>. MAKEPAD_RUN_CACHE_DIR makes clean refuse.");
    p.out
}

fn inspect_page(st: &Style) -> String {
    let mut p = Page::new(st, 34);
    title(&mut p, "ask a running app about itself");
    p.blank();
    p.usage("makepad inspect <pid|app|all|list> <command> [k=v]...");
    p.blank();
    p.text("Every `makepad run` listens on a local socket. Answers are JSON lines,");
    p.text("pre-analysed and bounded, each starting with what was measured.");
    p.section("Questions:");
    p.row("why-hung", "Verdict, wait chain or cycle, stacks");
    p.row("watch [on=panic,hang,..]", "Wait for the next event, print it, exit");
    p.row("events follow=1", "Stream events, one line each");
    p.row("threads | stacks | waits", "Threads, symbolised stacks, waits");
    p.row("panics | resume <id> panic", "Parked panics");
    p.row("help schema", "Tool definition for agents");
    p.text("`all` covers every running app, also ones started later.");
    p.section("Other machines:");
    p.row("<host>:<app|pid> <command>", "Ask an app on a host of hosts.toml (via the hub)");
    p.row("<host>:list", "That host's running apps");
    p.row("<host>:session launch <app>", "Start a hidden session there (kill <pid> ends it, also after the hub restarted)");
    p.row("<host>:session relaunch <pid>", "Restart its app domain, keeping the window and GPU device");
    p.row("hosts", "Registry, link state and round trip");
    p.row("all:list", "Apps here and on every host");
    p.text("Hosts are added with `makepad remote add <name> --ssh user@host` or paired with");
    p.text("`makepad remote pair <name> --hub <addr>` (the device dials `makepad hub --listen`).");
    p.text("`--env NAME=value` on `remote add` (hosts.toml: env = { DISPLAY = \":0\" }) sets the environment of its sessions.");
    p.out
}

/// The page for `makepad help [command]`; None for a word that is no command.
fn page(st: &Style, cmd: Option<&str>) -> Option<String> {
    Some(match cmd {
        None | Some("help") => main_page(st),
        Some("run") => run_page(st),
        Some("fetch") => fetch_page(st),
        Some("pack-src") => pack_src_page(st),
        Some("inspect") => inspect_page(st),
        Some("check") => check_page(st),
        Some("selftest") => selftest_page(st),
        Some("test") => test_page(st),
        Some("live") => live_page(st),
        Some("fix") => fix_page(st),
        Some("serve" | "schema" | "hover" | "definition") => serve_page(st),
        Some("clean") => clean_page(st),
        Some(c) if COMMANDS.contains(&c) => main_page(st),
        Some(_) => return None,
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
    eprintln!("  {}", st.dim("Run `makepad help` to list the commands."));
    2
}

fn emit(stream: Stream, text: &str) {
    match stream {
        Stream::Out => print!("{text}"),
        Stream::Err => eprint!("{text}"),
    }
}
