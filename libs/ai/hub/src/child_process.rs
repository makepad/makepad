//! Process-tree containment for every subprocess owned by the AI hub.
//!
//! Unix children start in their own process group, so cancellation and
//! deadline kills reach descendants as well as the direct child. Linux also
//! asks the kernel for `SIGKILL` when the service parent dies. macOS has no
//! `PR_SET_PDEATHSIG` equivalent: process-group kills cover supervised exits,
//! while the existing cancel/deadline polling remains its backstop.
//!
//! Linux caveat: `PR_SET_PDEATHSIG` binds to the spawning THREAD, not the
//! process — safe today because every long-lived worker (music3/world) is
//! spawned from the service's single long-lived job worker thread, and the
//! short-lived spawns (nvidia-smi, subproc jobs) block their spawning thread
//! until the child exits. A future spawn from a transient thread would kill
//! its child when that thread ends: keep spawns on owning threads.
//!
//! Windows children join one process-wide Job Object carrying
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; process teardown closes that handle
//! in the kernel and kills every process in the job.

use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

/// Spawns one contained child without changing its configured stdio or args.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    configure(command)?;
    let child = command.spawn()?;
    #[cfg(windows)]
    if let Err(error) = windows::assign(&child) {
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

/// Equivalent to [`Command::status`], through the contained spawn path.
pub(crate) fn status(command: &mut Command) -> io::Result<ExitStatus> {
    spawn(command)?.wait()
}

/// Equivalent to [`Command::output`], through the contained spawn path.
pub(crate) fn output(command: &mut Command) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn(command)?.wait_with_output()
}

/// Force-kills the whole Unix process group, with direct-child fallback when
/// the best-effort `setpgid` setup failed. Other platforms retain the direct
/// child kill; Windows descendants remain contained by the service Job.
pub(crate) fn kill_tree(child: &mut Child) -> io::Result<()> {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: negative pid selects the process group created in
            // `configure`; SIGKILL has no userspace handler.
            if unsafe { unix::kill(-pid, unix::SIGKILL) } == 0 {
                return Ok(());
            }
            // `setpgid` is deliberately best-effort, so preserve the old
            // direct-child kill as a fallback when no such group exists.
        }
    }
    child.kill()
}

#[cfg(unix)]
fn configure(command: &mut Command) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    #[cfg(target_os = "linux")]
    let parent_pid = std::process::id() as i32;

    // SAFETY: this closure runs after fork and invokes only small libc
    // syscalls plus errno inspection. Failures are intentionally ignored so
    // the existing polling/direct-kill behavior remains the fallback.
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            {
                let pdeath = unix::prctl(
                    unix::PR_SET_PDEATHSIG,
                    unix::SIGKILL as usize,
                    0,
                    0,
                    0,
                );
                if pdeath != 0 {
                    let _ = io::Error::last_os_error().raw_os_error();
                } else if unix::getppid() != parent_pid {
                    // Close the fork/prctl race if the service died between
                    // those two operations.
                    let _ = unix::kill(unix::getpid(), unix::SIGKILL);
                }
            }

            if unix::setpgid(0, 0) != 0 {
                let _ = io::Error::last_os_error().raw_os_error();
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(windows)]
fn configure(_command: &mut Command) -> io::Result<()> {
    // Create and configure the process-wide job before starting a child.
    windows::ensure_service_job()
}

#[cfg(not(any(unix, windows)))]
fn configure(_command: &mut Command) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
mod unix {
    pub(super) const SIGKILL: i32 = 9;

    extern "C" {
        pub(super) fn kill(pid: i32, signal: i32) -> i32;
        pub(super) fn setpgid(pid: i32, pgid: i32) -> i32;
    }

    #[cfg(target_os = "linux")]
    pub(super) const PR_SET_PDEATHSIG: i32 = 1;

    #[cfg(target_os = "linux")]
    extern "C" {
        pub(super) fn getpid() -> i32;
        pub(super) fn getppid() -> i32;
        pub(super) fn prctl(
            option: i32,
            arg2: usize,
            arg3: usize,
            arg4: usize,
            arg5: usize,
        ) -> i32;
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    type Handle = *mut c_void;
    type Bool = i32;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(size_of::<JobObjectBasicLimitInformation>() == 64);
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(size_of::<JobObjectExtendedLimitInformation>() == 144);
    #[cfg(target_pointer_width = "32")]
    const _: () = assert!(size_of::<JobObjectBasicLimitInformation>() == 48);
    #[cfg(target_pointer_width = "32")]
    const _: () = assert!(size_of::<JobObjectExtendedLimitInformation>() == 112);

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            information_class: i32,
            information: *const c_void,
            information_length: u32,
        ) -> Bool;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
        fn CloseHandle(object: Handle) -> Bool;
    }

    // Store the handle bits rather than a raw pointer so the process-wide
    // `OnceLock` is plainly Send + Sync.
    struct Job(usize);

    enum JobState {
        Ready(Job),
        Failed(i32),
    }

    static SERVICE_JOB: OnceLock<JobState> = OnceLock::new();

    pub(super) fn ensure_service_job() -> io::Result<()> {
        service_job().map(|_| ())
    }

    fn service_job() -> io::Result<&'static Job> {
        match SERVICE_JOB.get_or_init(create_job) {
            JobState::Ready(job) => Ok(job),
            JobState::Failed(code) => Err(io::Error::from_raw_os_error(*code)),
        }
    }

    fn create_job() -> JobState {
        // SAFETY: all pointers and structure sizes match the Win32 ABI and
        // are guarded by the compile-time layout assertions above.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return JobState::Failed(last_error_code());
            }
            let mut info: JobObjectExtendedLimitInformation = zeroed();
            info.basic_limit_information.limit_flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const _ as *const c_void,
                size_of::<JobObjectExtendedLimitInformation>() as u32,
            ) == 0
            {
                let code = last_error_code();
                let _ = CloseHandle(job);
                return JobState::Failed(code);
            }
            JobState::Ready(Job(job as usize))
        }
    }

    pub(super) fn assign(child: &Child) -> io::Result<()> {
        let job = service_job()?;
        // SAFETY: `Child` owns a live process handle until it is dropped.
        if unsafe {
            AssignProcessToJobObject(job.0 as Handle, child.as_raw_handle() as Handle)
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn last_error_code() -> i32 {
        io::Error::last_os_error().raw_os_error().unwrap_or(1)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::time::{Duration, Instant};

    const ESRCH: i32 = 3;

    #[test]
    fn group_kill_terminates_child_and_grandchild() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "(sleep 300 & echo $!); sleep 300"])
            .stdout(Stdio::piped());
        let mut child = spawn(&mut command).expect("spawn shell tree");
        let child_pid = child.id() as i32;
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("piped stdout"))
            .read_line(&mut line)
            .expect("read grandchild pid");
        let grandchild_pid = line.trim().parse::<i32>().expect("grandchild pid");

        assert!(process_exists(child_pid), "child {child_pid} never started");
        assert!(
            process_exists(grandchild_pid),
            "grandchild {grandchild_pid} never started"
        );
        kill_tree(&mut child).expect("kill process group");
        let _ = child.wait();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline
            && (process_exists(child_pid) || process_exists(grandchild_pid))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_pid_gone(child_pid);
        assert_pid_gone(grandchild_pid);
    }

    fn process_exists(pid: i32) -> bool {
        // SAFETY: signal zero only probes pid existence/permission.
        unsafe { unix::kill(pid, 0) == 0 }
    }

    fn assert_pid_gone(pid: i32) {
        // SAFETY: signal zero only probes pid existence/permission.
        let result = unsafe { unix::kill(pid, 0) };
        let errno = io::Error::last_os_error().raw_os_error();
        assert_eq!(result, -1, "pid {pid} still exists");
        assert_eq!(errno, Some(ESRCH), "pid {pid} probe failed unexpectedly");
    }
}
