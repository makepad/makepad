//! Starting and stopping an app child: the environment that points it at
//! the hub, its own process group, and killing that group by PID.

use std::process::Child;
use std::process::Command;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use makepad_platform::thread::{CancellationToken, Lane, TaskHandle, TaskPool};
use makepad_platform::Cx;

use crate::hub::ClientId;

/// SIGTERM-to-SIGKILL escalation gap inside `kill_child_group`, once a
/// caller has already decided to hard-kill (past its close grace, or the
/// client never got that far -- still building when it was closed).
pub const GROUP_KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(300);
/// Time for the protocol's shutdown request to save state and exit.
pub const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Reap a child after its host has sent `StudioToApp::Kill`. The returned
/// task fences a replacement launch; a busy pool returns ownership to retry.
/// The owner's cleanup runs on the same worker, after the process has exited.
pub fn reap_closed_child(mut child: Child, pool: &TaskPool, after_exit: impl FnOnce() + Send + 'static) -> Result<TaskHandle<()>, Child> {
    let Ok(slot) = pool.reserve(Lane::Heavy) else { return Err(child) };
    Ok(slot.submit(move || {
        let wait = CancellationToken::new();
        let deadline = Cx::monotonic_now() + CLOSE_GRACE.as_secs_f64();
        loop {
            if matches!(child.try_wait(), Ok(Some(_))) {
                after_exit();
                return;
            }
            let now = Cx::monotonic_now();
            if now >= deadline { break; }
            let _ = wait.wait_until((now + 0.016).min(deadline));
        }
        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            signal::kill_group(pid, signal::SIGTERM);
            let _ = wait.wait_until(Cx::monotonic_now() + GROUP_KILL_GRACE.as_secs_f64());
            if signal::alive(-pid) { signal::kill_group(pid, signal::SIGKILL); }
        }
        #[cfg(not(unix))]
        let _ = child.kill();
        let _ = child.wait();
        after_exit();
    }))
}

/// What a child needs to find the hub as client `id`: `STUDIO_HOST`,
/// `STUDIO_BUILD` and `STUDIO_CRATE` (the binary name), exactly like
/// studio launches run targets. On unix the child also leads a brand-new
/// process group, so everything it forks can be reached by one
/// negative-pid signal (`kill_child_group`).
pub fn configure(cmd: &mut Command, hub_port: u16, id: ClientId, bin: &str) {
    cmd.env("STUDIO_HOST", format!("http://127.0.0.1:{}", hub_port))
        .env("STUDIO_BUILD", id.to_string())
        .env("STUDIO_CRATE", bin);
    #[cfg(unix)]
    own_process_group(cmd);
}

/// Put `cmd`'s child at the head of a brand-new process group (unix only):
/// `process_group(0)` is `setpgid(0, 0)` before exec, so the pgid becomes
/// the child's own pid. Every process it forks (a build's rustc) inherits
/// that same pgid, so the whole tree can be reached by one negative-pid
/// signal later. `configure` does this; a caller that spawns a process
/// without `configure` (a build) calls it directly.
#[cfg(unix)]
pub fn own_process_group(cmd: &mut Command) {
    cmd.process_group(0);
}

/// Linux: the child gets SIGTERM when the thread that spawns it ends (the
/// host's UI thread: the host quit, crashed or was killed), so a hosted child
/// never outlives its host, whatever the host's shutdown did or did not get
/// to. Checks the parent once more after arming it, for a host that died
/// between fork and exec. The child's own process group goes with it through
/// the child's handling (CEF's subprocesses end with their browser process).
#[cfg(target_os = "linux")]
pub fn exit_with_parent(cmd: &mut Command) {
    extern "C" {
        fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
        fn getppid() -> i32;
        fn getpid() -> i32;
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const PR_SET_PDEATHSIG: i32 = 1;
    let parent = std::process::id() as i32;
    // SAFETY: prctl, getppid, getpid and kill are async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            if prctl(PR_SET_PDEATHSIG, signal::SIGTERM as u64, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if getppid() != parent {
                kill(getpid(), signal::SIGTERM);
            }
            Ok(())
        });
    }
}

/// Windows: a hosted child never outlives its host. Every child goes into one job object of
/// the host process with "kill on job close": the job's only handle is the host's, and the
/// system closes it when the host process ends (quit, crash or kill), which ends the children
/// and everything they started (CEF's subprocesses are in the job too: a child's children
/// inherit it). Assigned right after the spawn, before the child has started anything of its own.
#[cfg(windows)]
pub fn exit_with_parent(child: &Child) {
    use makepad_platform::windows::Win32::Foundation::HANDLE;
    use std::os::windows::io::AsRawHandle;
    if let Some(job) = host_job() {
        // SAFETY: the job handle lives as long as the process; the child's handle is valid
        if let Err(error) = unsafe { makepad_platform::windows::Win32::System::JobObjects::AssignProcessToJobObject(job, HANDLE(child.as_raw_handle() as _)) } {
            makepad_error_log::error!("app host: child {} not tied to the host's lifetime: {error:?}", child.id());
        }
    }
}

/// The host's job object (made once, never closed: it closes when the process ends).
#[cfg(windows)]
fn host_job() -> Option<makepad_platform::windows::Win32::Foundation::HANDLE> {
    use makepad_platform::windows::Win32::System::JobObjects::*;
    static JOB: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32).ok()?;
        Some(job.0 as usize)
    })
    .map(|h| makepad_platform::windows::Win32::Foundation::HANDLE(h as _))
}

/// `kill(2)` by hand -- this crate has no `libc` dependency, and a
/// two-liner beats pulling one in for a single syscall pair.
#[cfg(unix)]
mod signal {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;

    /// Signal the whole process group led by `pid` -- the POSIX convention
    /// of a negative pid.
    pub fn kill_group(pid: i32, sig: i32) {
        unsafe { kill(-pid, sig) };
    }

    /// `kill(pid, 0)` sends nothing; a zero return means the process (or
    /// group leader) still exists.
    pub fn alive(pid: i32) -> bool {
        unsafe { kill(pid, 0) == 0 }
    }
}

/// Whether process `pid` still exists.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    return signal::alive(pid as i32);
    #[cfg(windows)]
    unsafe {
        use makepad_platform::windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use makepad_platform::windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(process, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(process);
        alive
    }
}

/// Kill the whole process group a launched child heads -- the fix for
/// the leak `Child::kill()` had: through cargo, that call only ever reached
/// cargo itself, leaving the exec'd app (and any still-building rustc)
/// running as orphans. SIGTERM now, SIGKILL after `grace` for whatever is
/// still alive; the escalation runs off-thread so a UI-thread caller never
/// blocks on it. Windows keeps the plain `Child::kill()` this replaced.
#[cfg(unix)]
pub fn kill_child_group(child: &mut Child, grace: std::time::Duration, pool: &TaskPool) {
    let pid = child.id() as i32;
    signal::kill_group(pid, signal::SIGTERM);
    let wait = CancellationToken::new();
    let submitted = pool.submit(Lane::Heavy, move || {
        let _ = wait.wait_until(Cx::monotonic_now() + grace.as_secs_f64());
        if signal::alive(-pid) {
            signal::kill_group(pid, signal::SIGKILL);
        }
    });
    match submitted {
        Ok(task) => task.detach(),
        Err(_) if signal::alive(-pid) => signal::kill_group(pid, signal::SIGKILL),
        Err(_) => {}
    }
}

#[cfg(not(unix))]
pub fn kill_child_group(child: &mut Child, _grace: std::time::Duration, _pool: &TaskPool) {
    let _ = child.kill();
}

/// Final slot teardown owns the child from here on. Signal and reap it wholly
/// on a heavy pool worker; dropping a slot on the UI thread never waits for a
/// process or decoder wrapper to exit.
pub fn reap_child_group(mut child: Child, grace: std::time::Duration, pool: &TaskPool) {
    #[cfg(unix)]
    let pid = {
        let pid = child.id() as i32;
        signal::kill_group(pid, signal::SIGTERM);
        pid
    };
    match pool.reserve(Lane::Heavy) {
        Ok(slot) => slot
            .submit(move || {
                #[cfg(unix)]
                {
                    let wait = CancellationToken::new();
                    let _ = wait.wait_until(Cx::monotonic_now() + grace.as_secs_f64());
                    if signal::alive(-pid) {
                        signal::kill_group(pid, signal::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = grace;
                    let _ = child.kill();
                }
                let _ = child.wait();
            })
            .detach(),
        Err(_) => {
            #[cfg(unix)]
            signal::kill_group(pid, signal::SIGKILL);
            #[cfg(not(unix))]
            let _ = child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::Stdio;

    /// Windows: a hosted child dies with its host. The host's job is what the system closes
    /// when the host process ends; this test plays that by closing a job of its own made the
    /// same way, and the child is gone without a kill from the test.
    #[cfg(windows)]
    #[test]
    fn a_child_ends_when_its_host_goes() {
        use makepad_platform::windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
        use makepad_platform::windows::Win32::System::JobObjects::*;
        use makepad_platform::windows::Win32::System::Threading::GetExitCodeProcess;
        use std::os::windows::io::AsRawHandle;
        let child = Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 > nul"]).spawn().expect("spawn cmd");
        unsafe {
            let job = CreateJobObjectW(None, None).expect("job");
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32).expect("limits");
            AssignProcessToJobObject(job, HANDLE(child.as_raw_handle() as _)).expect("assign");
            let _ = CloseHandle(job);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let alive = || {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(HANDLE(child.as_raw_handle() as _), &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32
        };
        while alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!alive(), "the child outlived its host's job");
    }

    /// The host's own job is made and takes a child.
    #[cfg(windows)]
    #[test]
    fn a_hosted_child_joins_the_hosts_job() {
        let mut child = Command::new("cmd").args(["/c", "exit 0"]).spawn().expect("spawn cmd");
        assert!(host_job().is_some());
        exit_with_parent(&child);
        let _ = child.wait();
    }

    /// `own_process_group` really does make the child its own group leader
    /// -- the precondition `kill_child_group`'s negative-pid signal relies
    /// on. `getpgid` is one more syscall this crate has no `libc` for.
    /// A hosted child dies with its host: the spawning thread ends (as the
    /// host's UI thread does when the host goes) and the child is gone,
    /// with no shutdown message and no kill from the host.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_ends_when_its_host_goes() {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let pid = std::thread::spawn(|| {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            exit_with_parent(&mut cmd);
            let child = cmd.spawn().expect("spawn sleep");
            let pid = child.id() as i32;
            // the host does not reap or kill it: it just goes away
            std::mem::forget(child);
            pid
        })
        .join()
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        // gone, or a zombie of this test process (reaped below)
        let gone = || unsafe { kill(pid, 0) } != 0 || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z "));
        while !gone() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(gone(), "the child outlived the thread that spawned it");
        extern "C" {
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        }
        let mut status = 0;
        unsafe { waitpid(pid, &mut status, 0) };
    }

    #[cfg(unix)]
    #[test]
    fn spawned_children_lead_their_own_process_group() {
        extern "C" {
            fn getpgid(pid: i32) -> i32;
        }
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 5");
        own_process_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn /bin/sh");
        let pid = child.id() as i32;
        let pgid = unsafe { getpgid(pid) };
        assert_eq!(pgid, pid, "the child should lead its own new group");
        let pool = Cx::new(Box::new(|_, _| {})).task_pool();
        kill_child_group(&mut child, std::time::Duration::from_millis(50), &pool);
        let _ = child.wait();
    }

    /// The bug this fixes: a child launched through a wrapper (`cargo run`
    /// stands in for it here as any process that forks a grandchild rather
    /// than exec-replacing itself) leaks that grandchild when only the
    /// wrapper is killed. Reproduce it with a shell that backgrounds a
    /// `sleep` and prints its pid, then confirm `kill_child_group` reaps
    /// BOTH -- the regression `Child::kill()` alone could not clear.
    #[cfg(unix)]
    #[test]
    fn killing_the_group_reaps_a_grandchild_the_wrapper_leaked() {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30 & echo $!; wait");
        own_process_group(&mut cmd);
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn /bin/sh");

        use std::io::BufRead;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read grandchild pid");
        let grandchild_pid: i32 = line.trim().parse().expect("a pid line");

        // The grandchild is alive and NOT the pid we hold -- it really is
        // one generation further down, like the app under `cargo run`.
        assert_ne!(grandchild_pid, child.id() as i32);
        assert_eq!(unsafe { kill(grandchild_pid, 0) }, 0, "grandchild not up yet");

        let pool = Cx::new(Box::new(|_, _| {})).task_pool();
        kill_child_group(&mut child, std::time::Duration::from_millis(50), &pool);
        // Past the SIGTERM->SIGKILL escalation: nothing in the group is
        // still standing, wrapper or grandchild.
        let _ = child.wait();
        // The wrapper can exit before the asynchronous escalation and before
        // launchd reaps the orphan. Wait only in this test, never on the UI.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while unsafe { kill(grandchild_pid, 0) } == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { kill(grandchild_pid, 0) },
            -1,
            "the grandchild the wrapper orphaned should be gone too"
        );
    }
}
