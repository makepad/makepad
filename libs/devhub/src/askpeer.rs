//! Who sent an answer to one of the user's questions (`devhub ask answer`, the aires tray): the
//! master judges the asking process from its socket's credentials (its pid, which no client can
//! claim), never from what the request says.
//!
//! Agents run as the same user as the person, so nothing here stops one that sets out to forge an
//! answer (that takes a separate user or code signing). What it does: no agent answers for the
//! person by habit, by mistake or from a script, and main can tell a tray answer from anything
//! else. An answer is
//! - refused when the process or any of its parents up to launchd / init is in an agent's
//!   environment (an agent CLI's variable: [`AGENT_VARS`]) or is an agent CLI ([`AGENT_NAMES`]);
//! - `tray` when the process is the installed aires app (its bundle's executable, started by
//!   launchd: its login item or `open`), which answers only on presses the window system
//!   delivered;
//! - `cli` otherwise: the person's `devhub ask answer` at a terminal.
//!
//! A process's environment is read as the OS keeps it from its start (macOS `KERN_PROCARGS2`, Linux
//! `/proc/<pid>/environ`); macOS hides it for its own platform binaries (a shell), whose parent
//! chain then decides.

/// The executable of the aires app: in its installed bundle ([`installed_tray`]) it is the tray.
pub const AIRES_EXE: &str = "makepad-app-aires";

/// The installed tray's executable (tools/aires/install-mac.sh), as launchd starts it: the one
/// place a tray answer comes from. Empty without a home folder: then nothing is the tray.
pub fn installed_tray() -> String {
    match std::env::var("HOME") {
        Ok(home) if home.starts_with('/') => format!("{}/Applications/aires.app/Contents/MacOS/{AIRES_EXE}", home.trim_end_matches('/')),
        _ => String::new(),
    }
}

/// What an agent CLI puts in the environment of what it runs.
pub const AGENT_VARS: [&str; 5] = ["DEVHUB_AGENT", "CLAUDECODE", "CLAUDE_CODE_", "CODEX_", "GROK_"];

/// Agent CLIs, by process name.
pub const AGENT_NAMES: [&str; 3] = ["claude", "codex", "grok"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Tray,
    Cli,
    Refused,
}

impl Source {
    pub fn word(self) -> &'static str {
        match self {
            Source::Tray => "tray",
            Source::Cli => "cli",
            Source::Refused => "refused",
        }
    }
}

/// The verdict on an asking process: its source, its chain as a log line names it
/// (`pid 4242 makepad-app-aires, from 1 launchd`) and, when refused, why.
#[derive(Clone, Debug, PartialEq)]
pub struct Judged {
    pub source: Source,
    pub chain: String,
    pub why: String,
}

/// One process of the chain, as the OS reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// its executable's path (empty: not known)
    pub exe: String,
    /// its environment's variable names (empty: none, or hidden)
    pub env: Vec<String>,
}

/// Whether a variable name is an agent CLI's.
fn agent_var(name: &str) -> bool {
    AGENT_VARS.iter().any(|v| if v.ends_with('_') { name.starts_with(v) } else { name == *v })
}

/// Whether a process is an agent CLI, by its name or its executable's file name.
fn agent_cli(p: &Seen) -> bool {
    let file = p.exe.rsplit('/').next().unwrap_or("");
    [p.name.as_str(), file].iter().any(|n| AGENT_NAMES.contains(&n.to_ascii_lowercase().as_str()))
}

/// The chain as a log line: the asker first, then each parent.
fn chain_line(chain: &[Seen]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, p) in chain.iter().enumerate() {
        let name = if p.name.is_empty() { "?" } else { p.name.as_str() };
        parts.push(if i == 0 { format!("pid {} {name}", p.pid) } else { format!("from {} {name}", p.pid) });
    }
    if parts.is_empty() {
        return "a process the OS does not name".into();
    }
    parts.join(", ")
}

/// The verdict on `chain` (the asker first, then its parents up to launchd / init); `tray` is the
/// installed tray's executable ([`installed_tray`]).
pub fn judge_chain(chain: &[Seen], tray: &str) -> Judged {
    let line = chain_line(chain);
    let Some(asker) = chain.first() else {
        return Judged { source: Source::Refused, chain: line, why: "the asking process is gone already".into() };
    };
    // aires asking: the person reads this under its card, so it starts with what they do about it
    let aires = asker.exe.rsplit('/').next() == Some(AIRES_EXE);
    let refused = |reason: String| {
        let why = if aires { format!("this copy of aires was started by an agent: quit it from its menu-bar icon and open aires from Spotlight ({reason})") } else { reason };
        Judged { source: Source::Refused, chain: line.clone(), why }
    };
    for p in chain {
        if let Some(var) = p.env.iter().find(|v| agent_var(v)) {
            return refused(format!("pid {} {} is in an agent's environment: {var} is set", p.pid, p.name));
        }
        if agent_cli(p) {
            return refused(format!("pid {} {} is an agent CLI", p.pid, p.name));
        }
    }
    // the installed bundle that launchd started: the path as the process was exec'd (KERN_PROCARGS2)
    let source = if !tray.is_empty() && asker.exe == tray && asker.ppid == 1 { Source::Tray } else { Source::Cli };
    Judged { source, chain: line, why: String::new() }
}

/// The asker `pid` and its parents up to launchd / init, as the OS reports them now.
pub fn chain_of(pid: u32) -> Vec<Seen> {
    let mut chain: Vec<Seen> = Vec::new();
    let mut at = pid;
    // (a parent loop cannot happen; the pid check keeps a misread from walking forever)
    while at != 0 && !chain.iter().any(|p| p.pid == at) {
        let Some(p) = makepad_remote::procs::process(at) else { break };
        let (exe, env) = args_of(at).unwrap_or_default();
        let ppid = p.ppid;
        chain.push(Seen { pid: at, ppid, name: p.name, exe, env });
        if at <= 1 {
            break;
        }
        at = ppid;
    }
    chain
}

/// The verdict on the process `pid`.
pub fn judge(pid: u32) -> Judged {
    if pid == 0 {
        return Judged { source: Source::Refused, chain: "a process its connection does not name".into(), why: "the connection carries no process (not the local socket)".into() };
    }
    judge_chain(&chain_of(pid), &installed_tray())
}

/// A process's executable path (as it was exec'd) and environment variable names. None: not
/// readable (another user's process).
#[cfg(target_os = "macos")]
pub(crate) fn args_of(pid: u32) -> Option<(String, Vec<String>)> {
    extern "C" {
        fn sysctl(name: *const i32, namelen: u32, oldp: *mut u8, oldlenp: *mut usize, newp: *const u8, newlen: usize) -> i32;
    }
    // CTL_KERN 1, KERN_ARGMAX 8, KERN_PROCARGS2 49
    let mut argmax: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    if unsafe { sysctl([1, 8].as_ptr(), 2, (&mut argmax as *mut i32).cast(), &mut len, std::ptr::null(), 0) } != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut len = buf.len();
    let mib = [1, 49, i32::try_from(pid).ok()?];
    if unsafe { sysctl(mib.as_ptr(), 3, buf.as_mut_ptr(), &mut len, std::ptr::null(), 0) } != 0 {
        return None;
    }
    Some(parse_procargs2(&buf[..len.min(buf.len())]))
}

/// `KERN_PROCARGS2`'s layout: argc (i32), the executable's path, NUL padding, argc arguments, then
/// the environment, each NUL-ended.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_procargs2(b: &[u8]) -> (String, Vec<String>) {
    if b.len() < 4 {
        return (String::new(), Vec::new());
    }
    let argc = i32::from_ne_bytes(b[..4].try_into().unwrap()).max(0) as usize;
    let mut strings = b[4..].split(|c| *c == 0);
    let exe = strings.next().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
    // the padding is empty strings; then the arguments, then the environment
    let mut rest = strings.skip_while(|s| s.is_empty());
    for _ in 0..argc {
        if rest.next().is_none() {
            break;
        }
    }
    let env = rest.take_while(|s| !s.is_empty()).filter_map(|s| env_name(s)).collect();
    (exe, env)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn args_of(pid: u32) -> Option<(String, Vec<String>)> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    Some((exe, env.split(|c| *c == 0).filter_map(env_name).collect()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn args_of(_pid: u32) -> Option<(String, Vec<String>)> {
    None
}

/// `NAME=value` -> `NAME`.
fn env_name(s: &[u8]) -> Option<String> {
    let eq = s.iter().position(|c| *c == b'=')?;
    (eq > 0).then(|| String::from_utf8_lossy(&s[..eq]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAY: &str = "/Users/x/Applications/aires.app/Contents/MacOS/makepad-app-aires";

    fn p(pid: u32, ppid: u32, name: &str, exe: &str, env: &[&str]) -> Seen {
        Seen { pid, ppid, name: name.into(), exe: exe.into(), env: env.iter().map(|e| e.to_string()).collect() }
    }

    /// The verdicts, each assertion naming the case: the installed tray, the person's terminal,
    /// and each way an agent shows (its variable on the asker or a parent, the agent CLI as a
    /// parent, a tray binary an agent's shell started).
    #[test]
    fn answers_are_judged_by_the_asking_process_and_its_parents() {
        let launchd = p(1, 0, "launchd", "/sbin/launchd", &[]);
        let tray = p(4242, 1, "makepad-app-aires", "/Users/x/Applications/aires.app/Contents/MacOS/makepad-app-aires", &["HOME", "PATH"]);
        let j = judge_chain(&[tray.clone(), launchd.clone()], TRAY);
        assert_eq!((j.source, j.chain.as_str()), (Source::Tray, "pid 4242 makepad-app-aires, from 1 launchd"), "the installed tray, started by launchd");

        let cli = p(900, 899, "makepad-devhub", "/repo/target/host/makepad-devhub", &["HOME", "TERM"]);
        let zsh = p(899, 800, "zsh", "/bin/zsh", &[]);
        let login = p(800, 700, "login", "/usr/bin/login", &[]);
        let terminal = p(700, 1, "Terminal", "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal", &[]);
        assert_eq!(judge_chain(&[cli.clone(), zsh.clone(), login.clone(), terminal.clone(), launchd.clone()], TRAY).source, Source::Cli, "the person's terminal");

        let in_agent = p(901, 899, "makepad-devhub", "/repo/target/host/makepad-devhub", &["HOME", "CLAUDECODE"]);
        let j = judge_chain(&[in_agent, zsh.clone(), launchd.clone()], TRAY);
        assert_eq!(j.source, Source::Refused, "an agent CLI's variable on the asker");
        assert!(j.why.contains("CLAUDECODE"), "the refusal names the variable: {}", j.why);

        let scrubbed_child = p(902, 903, "python3", "/usr/bin/python3", &["HOME"]);
        let agent_shell = p(903, 904, "bash", "/bin/bash", &["CODEX_SANDBOX"]);
        assert_eq!(judge_chain(&[scrubbed_child.clone(), agent_shell, launchd.clone()], TRAY).source, Source::Refused, "a scrubbed process whose parent is in an agent's environment");

        let hidden_shell = p(903, 1211, "zsh", "/bin/zsh", &[]);
        let claude = p(1211, 936, "claude", "/Users/x/.local/bin/claude", &["HOME"]);
        let j = judge_chain(&[scrubbed_child, hidden_shell, claude, launchd.clone()], TRAY);
        assert_eq!(j.source, Source::Refused, "an agent CLI among the parents (a shell whose environment the OS hides)");
        assert!(j.why.contains("agent CLI"), "{}", j.why);

        let agent_started = p(909, 1, "makepad-app-aires", TRAY, &["HOME", "CLAUDE_CODE_SESSION_ATTENDED"]);
        let j = judge_chain(&[agent_started, launchd.clone()], TRAY);
        assert_eq!(j.source, Source::Refused, "the installed tray started from an agent's environment");
        assert!(j.why.starts_with("this copy of aires was started by an agent: quit it") && j.why.contains("Spotlight") && j.why.contains("CLAUDE_CODE_SESSION_ATTENDED"), "aires's refusal says what the person does, then why: {}", j.why);

        let lane_tray = p(905, 903, "makepad-app-aires", "/lane/target/release/aires.app/Contents/MacOS/makepad-app-aires", &[]);
        let shell = p(903, 1, "zsh", "/bin/zsh", &[]);
        assert_eq!(judge_chain(&[lane_tray, shell, launchd.clone()], TRAY).source, Source::Cli, "a tray binary a shell started is no tray");

        let jit_tray = p(906, 1, "makepad", "/repo/target/host/makepad", &[]);
        assert_eq!(judge_chain(&[jit_tray, launchd.clone()], TRAY).source, Source::Cli, "only the installed bundle is the tray");
        let other_bundle = p(907, 1, "makepad-app-aires", "/tmp/x/aires.app/Contents/MacOS/makepad-app-aires", &[]);
        assert_eq!(judge_chain(&[other_bundle, launchd.clone()], TRAY).source, Source::Cli, "a bundle elsewhere that launchd started is no tray");
        let installed = p(908, 1, "makepad-app-aires", TRAY, &[]);
        assert_eq!(judge_chain(&[installed, launchd], "").source, Source::Cli, "without a home folder nothing is the tray");
        assert_eq!(judge(0).source, Source::Refused, "a connection that names no process");
    }

    #[test]
    fn procargs2_gives_the_executable_and_the_environment_names() {
        let mut b = 2i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/Apps/aires.app/Contents/MacOS/makepad-app-aires\0\0\0\0");
        b.extend_from_slice(b"makepad-app-aires\0--flag\0HOME=/u\0CLAUDECODE=1\0\0junk\0");
        let (exe, env) = parse_procargs2(&b);
        assert_eq!(exe, "/Apps/aires.app/Contents/MacOS/makepad-app-aires");
        assert_eq!(env, vec!["HOME".to_string(), "CLAUDECODE".to_string()], "arguments are skipped, the environment ends at its empty string");
        assert_eq!(parse_procargs2(&[1, 0]), (String::new(), Vec::new()), "a short buffer");
    }

    /// This test's own process: the OS's view of it, read the way the master reads an asker.
    #[test]
    fn this_process_reads_as_itself() {
        let chain = chain_of(std::process::id());
        assert_eq!(chain.first().map(|p| p.pid), Some(std::process::id()), "the asker is first");
        assert!(chain.last().is_some_and(|p| p.pid <= 1 || p.ppid == 0), "the chain ends at launchd / init: {chain:?}");
    }
}
