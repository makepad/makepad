//! Windows has no exec, and whoever started a run (devhub, `cargo run`, a terminal) sees it end when
//! its process does. So a run that can restart itself (a live `makepad run`, a live test harness) is
//! the child of a thin parent, the first `makepad.exe`: that one starts the run, waits for it and
//! passes on its exit code. To restart, the run writes the next run's command to a file and exits
//! with [`RESTART_EXIT`]; the parent starts it. The old process is gone before the new one opens the
//! disk cache, the waiters see one process, and the run's pid changes with each restart. The parent's
//! pid is the one that stays, and the one whoever started the run holds: the run's discovery file
//! names it, so that pid finds the run (`makepad_remote::discovery::App::answers_to`).
use std::ffi::{OsStr, OsString};

/// The exit code with which a run asks its parent to start the next one.
#[cfg(windows)]
const RESTART_EXIT: u32 = 0x4D50_5253;

/// The environment variable naming the file the next run's command goes to: set by the parent for
/// the run it starts, which takes it out so the programs it starts get parents of their own.
#[cfg(windows)]
const FILE_ENV: &str = "RELAX_RESTART_FILE";

/// The parent's pid, set and taken out with [`FILE_ENV`].
#[cfg(windows)]
const PARENT_ENV: &str = "RELAX_RESTART_PARENT";

#[cfg(windows)]
static FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(windows)]
static PARENT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// The pid of the parent this run stands behind; None for a process without one.
#[cfg(windows)]
pub(crate) fn parent_pid() -> Option<u32> {
    PARENT.get().copied()
}

/// Whether the command line `args` (with the program's name first) is a run that can restart: the
/// same runs as `driver_options.live` in run.rs, as far as a command line tells.
fn can_restart(args: &[Vec<u8>]) -> bool {
    if args.get(1).map(Vec::as_slice) != Some(b"run".as_slice()) {
        return false;
    }
    let has = |names: &[&[u8]]| args[2..].iter().take_while(|a| a.as_slice() != b"--").any(|a| names.contains(&a.as_slice()));
    has(&[b"--live"]) || !has(&[b"--check", b"--harness", b"--plain-test", b"--no-live", b"--silent", b"--src"]) && std::env::var_os("RELAX_SCRIPT_DEPTH").is_none()
}

/// The next run as it is written to the file: what an exec would keep and change, the process's
/// working directory and whole environment included (the parent has neither as the run left them).
#[derive(Debug, PartialEq)]
struct Next {
    program: OsString,
    args: Vec<OsString>,
    dir: OsString,
    envs: Vec<(OsString, OsString)>,
}

fn put(out: &mut Vec<u8>, word: &OsStr) {
    let bytes = word.as_encoded_bytes();
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl Next {
    /// This process's directory and environment with `cmd`'s changes (its folder, when it names one).
    #[cfg(windows)]
    fn of(cmd: &std::process::Command) -> std::io::Result<Next> {
        let mut envs: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        for (k, v) in cmd.get_envs() {
            envs.retain(|(e, _)| !e.eq_ignore_ascii_case(k));
            if let Some(v) = v {
                envs.push((k.to_os_string(), v.to_os_string()));
            }
        }
        Ok(Next {
            program: cmd.get_program().to_os_string(),
            args: cmd.get_args().map(OsStr::to_os_string).collect(),
            dir: match cmd.get_current_dir() { Some(dir) => dir.as_os_str().to_os_string(), None => std::env::current_dir()?.into_os_string() },
            envs,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, &self.program);
        out.extend_from_slice(&(self.args.len() as u64).to_le_bytes());
        for a in &self.args {
            put(&mut out, a);
        }
        put(&mut out, &self.dir);
        out.extend_from_slice(&(self.envs.len() as u64).to_le_bytes());
        for (k, v) in &self.envs {
            put(&mut out, k);
            put(&mut out, v);
        }
        out
    }

    /// None: not what `encode` writes. The words come back as the run's own encoding wrote them:
    /// the file is this binary's, from the run this process started.
    fn decode(data: &[u8]) -> Option<Next> {
        struct Reader<'a>(&'a [u8]);
        impl<'a> Reader<'a> {
            fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
                if self.0.len() < n {
                    return None;
                }
                let (head, tail) = self.0.split_at(n);
                self.0 = tail;
                Some(head)
            }
            fn len(&mut self) -> Option<usize> {
                Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?) as usize)
            }
            fn word(&mut self) -> Option<OsString> {
                let n = self.len()?;
                // SAFETY: as_encoded_bytes wrote them (put), in this same binary
                Some(unsafe { OsStr::from_encoded_bytes_unchecked(self.bytes(n)?) }.to_os_string())
            }
        }
        let mut r = Reader(data);
        let program = r.word()?;
        let n = r.len()?;
        let args = (0..n).map(|_| r.word()).collect::<Option<Vec<_>>>()?;
        let dir = r.word()?;
        let n = r.len()?;
        let envs = (0..n).map(|_| Some((r.word()?, r.word()?))).collect::<Option<Vec<_>>>()?;
        r.0.is_empty().then_some(Next { program, args, dir, envs })
    }
}

/// A run asks for the next one: `cmd` is written for the parent and this process ends. Returns only
/// on failure, with why: this run goes on.
#[cfg(windows)]
pub(crate) fn request(cmd: &std::process::Command) -> String {
    let Some(file) = FILE.get() else {
        return "this run has no parent process to start it again".to_string();
    };
    let next = match Next::of(cmd) {
        Ok(n) => n,
        Err(e) => return format!("this run's directory is unknown: {e}"),
    };
    if let Err(e) = std::fs::write(file, next.encode()) {
        return format!("cannot write {}: {e}", file.display());
    }
    // the next run's window may take the focus this one has
    const ASFW_ANY: u32 = u32::MAX;
    let _ = unsafe { makepad_windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(ASFW_ANY) };
    // an exec does not run the program's exit handlers either; the process's own exit work does run
    // (the discovery file of this pid goes: the hub reads one left behind as a crash)
    makepad_core_util::process_end::end_without_c_handlers(RESTART_EXIT as i32)
}

/// Windows, on entry of the loader with the resolved command line `args`: the run its parent started
/// remembers the parent's file and goes on; a run that can restart becomes that parent and never
/// returns; anything else goes on as a plain process.
#[cfg(windows)]
pub fn enter(args: &[Vec<u8>]) {
    if let Some(file) = std::env::var_os(FILE_ENV) {
        std::env::remove_var(FILE_ENV);
        let _ = FILE.set(std::path::PathBuf::from(file));
        if let Some(pid) = std::env::var_os(PARENT_ENV).and_then(|p| p.to_str()?.parse().ok()) {
            let _ = PARENT.set(pid);
        }
        std::env::remove_var(PARENT_ENV);
        return;
    }
    if can_restart(args) {
        parent(args);
    }
}

#[cfg(windows)]
fn parent(args: &[Vec<u8>]) -> ! {
    use std::os::windows::io::AsRawHandle;
    /// A console's Ctrl+C reaches the run too, which decides what it means; this process waits for it.
    extern "system" fn ignore_console_event(_: u32) -> i32 {
        1
    }
    unsafe { relax_session::win32::SetConsoleCtrlHandler(Some(ignore_console_event), 1) };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("relax: the {tool} binary's path is unknown: {e}", tool = crate::tool_name());
            std::process::exit(1);
        }
    };
    let job = kill_on_close_job();
    let file = std::env::temp_dir().join(format!("relax-restart-{}", std::process::id()));
    // SAFETY: args_bytes gives the encoded bytes of OsStrings
    let words = args[1..].iter().map(|a| unsafe { OsStr::from_encoded_bytes_unchecked(a) }.to_os_string()).collect();
    let dir = std::env::current_dir().map(std::path::PathBuf::into_os_string).unwrap_or_default();
    let mut next = Next { program: exe.into_os_string(), args: words, dir, envs: std::env::vars_os().collect() };
    loop {
        let mut cmd = std::process::Command::new(&next.program);
        cmd.args(&next.args).env_clear().envs(next.envs.iter().map(|(k, v)| (k, v))).env(FILE_ENV, &file).env(PARENT_ENV, std::process::id().to_string());
        if !next.dir.is_empty() {
            cmd.current_dir(&next.dir);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => next_failed(&next, &format!("cannot start the next run: {e}")),
        };
        if let Some(job) = &job {
            let handle = makepad_windows_sys::Win32::Foundation::HANDLE(child.as_raw_handle() as _);
            if unsafe { makepad_windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(*job, handle) }.is_err() {
                eprintln!("relax: this process's end will not end the run: it could not join the job");
            }
        }
        let code = match child.wait() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => next_failed(&next, &format!("cannot wait for the run: {e}")),
        };
        if code as u32 != RESTART_EXIT {
            let _ = std::fs::remove_file(&file);
            std::process::exit(code);
        }
        let read = std::fs::read(&file).map_err(|e| e.to_string()).and_then(|d| Next::decode(&d).ok_or("not a run's command".to_string()));
        let _ = std::fs::remove_file(&file);
        next = match read {
            Ok(n) => n,
            Err(e) => next_failed(&next, &format!("cannot read the next run's command in {}: {e}", file.display())),
        };
    }
}

/// The job this process's end ends the run with (a run outliving a parent that was killed would
/// hold its window and its files). What the run starts stays out of it, as without a parent: those
/// programs' ends are not this process's. None: it could not be made; the run goes on without it.
#[cfg(windows)]
fn kill_on_close_job() -> Option<makepad_windows_sys::Win32::Foundation::HANDLE> {
    use makepad_windows_sys::Win32::System::JobObjects::*;
    // winnt.h (the generated bindings do not carry it)
    const SILENT_BREAKAWAY_OK: u32 = 0x0000_1000;
    // SAFETY: the generated metadata supplies the Win32 structures and ABI
    let job = unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.0 | SILENT_BREAKAWAY_OK);
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const std::ffi::c_void, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32).ok()?;
        job
    };
    Some(job)
}

/// The run that asked for a restart has ended and the next did not start: only this process can say.
#[cfg(windows)]
fn next_failed(next: &Next, why: &str) -> ! {
    eprintln!("relax live: restart failed: {why}");
    if let Some((_, path)) = next.envs.iter().find(|(k, _)| k == "RELAX_JIT_RECOVERY") {
        crate::hot_failure::next_run_failed(std::path::Path::new(path), why);
    }
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<Vec<u8>> {
        args.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    #[test]
    fn only_a_run_that_can_restart_gets_a_parent() {
        for live in [&["makepad", "run", "tool-a"][..], &["makepad", "run", "--quiet", "tool-a"], &["makepad", "run", "--harness", "--live", "p"]] {
            assert!(can_restart(&words(live)), "{live:?}");
        }
        for other in [
            &["makepad", "inspect", "1", "stacks"][..],
            &["makepad", "devhub", "ls"],
            &["makepad", "test", "p"],
            &["makepad", "run", "--no-live", "p"],
            &["makepad", "run", "--check", "p"],
            &["makepad", "run", "--harness", "p"],
            &["makepad", "run", "--silent", "p"],
            &["makepad"],
        ] {
            assert!(!can_restart(&words(other)), "{other:?}");
        }
        assert!(can_restart(&words(&["makepad", "run", "p", "--", "--no-live"])), "the program's own arguments are not the run's");
    }

    #[test]
    fn the_next_run_comes_back_from_the_file_as_it_was_written() {
        let next = Next {
            program: "C:\\a b\\makepad.exe".into(),
            args: vec!["run".into(), "pkg".into(), "".into(), "with space".into()],
            dir: "C:\\work dir".into(),
            envs: vec![("RELAX_RESTART_T0".into(), "12".into()), ("EMPTY".into(), "".into())],
        };
        let data = next.encode();
        assert_eq!(Next::decode(&data), Some(next));
        assert_eq!(Next::decode(&data[..data.len() - 1]), None, "cut short");
        assert_eq!(Next::decode(&[data.as_slice(), &[0]].concat()), None, "something after it");
    }
}
