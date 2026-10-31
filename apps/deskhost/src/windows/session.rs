//! Session checks and the privileged console task. All work runs on the host worker.
use super::windows;
use windows::core::{BSTR, HRESULT, PWSTR};
use windows::session::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, RPC_E_CHANGED_MODE};
use windows::session::Win32::System::RemoteDesktop::{WTSConnectState, WTSDisconnected, WTSGetActiveConsoleSessionId};
use windows::session::Win32::System::TaskScheduler::{IRegisteredTask, ITaskService, TaskScheduler};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSFreeMemory, WTSQuerySessionInformationW, WTS_CONNECTSTATE_CLASS};
use windows::Win32::System::Variant::VARIANT;

const CONSOLE_TASK: &str = "Makepad Desk Host console";
const MISSING_TASK: &str = "the console task is missing: run setup.ps1";

#[derive(Clone, Copy, Debug)]
struct Session {
    id: u32,
    console: u32,
    disconnected: bool,
}

#[derive(Debug, PartialEq)]
struct Decision {
    needs_move: bool,
    can_move: bool,
    note: &'static str,
}

/// Session identity decides whether to move. In particular, a disconnected user's
/// session differs from the empty console/login screen even though SM_REMOTESESSION is false.
fn decide(session: Session, task_present: bool) -> Decision {
    let needs_move = session.id != session.console;
    let note = if !task_present {
        MISSING_TASK
    } else if !needs_move {
        "the session is on the console"
    } else if session.disconnected {
        "the session is disconnected; moving it to the console"
    } else {
        "this desktop is in a Remote Desktop session; moving it to the console"
    };
    Decision { needs_move, can_move: needs_move && task_present, note }
}

fn current() -> Result<Session, String> {
    unsafe {
        let mut id = 0;
        ProcessIdToSessionId(std::process::id(), &mut id).map_err(|e| format!("cannot identify this desktop's session: {e:?}"))?;
        let mut buffer = PWSTR(std::ptr::null_mut());
        let mut bytes = 0;
        WTSQuerySessionInformationW(None, id, WTSConnectState, &mut buffer, &mut bytes).map_err(|e| format!("cannot read this desktop's session state: {e:?}"))?;
        let state = if !buffer.0.is_null() && bytes as usize >= std::mem::size_of::<WTS_CONNECTSTATE_CLASS>() {
            Some(std::ptr::read_unaligned(buffer.0.cast::<WTS_CONNECTSTATE_CLASS>()))
        } else {
            None
        };
        WTSFreeMemory(buffer.0.cast());
        let state = state.ok_or("Windows returned no desktop session state")?;
        Ok(Session { id, console: WTSGetActiveConsoleSessionId(), disconnected: state == WTSDisconnected })
    }
}

struct Apartment(bool);
impl Apartment {
    fn new() -> Result<Self, HRESULT> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            // This worker already has a different apartment; use it without uninitializing it.
            Ok(Self(false))
        } else {
            hr.ok()?;
            Ok(Self(true))
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 { unsafe { CoUninitialize() }; }
    }
}

struct ConsoleTask {
    // Release the COM interface before balancing this thread's COM initialization.
    task: IRegisteredTask,
    _apartment: Apartment,
}
impl ConsoleTask {
    fn open() -> Result<Option<Self>, String> {
        let open = || -> Result<Option<Self>, HRESULT> {
            let apartment = Apartment::new()?;
            unsafe {
                let service = ITaskService::from_raw(CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER, &ITaskService::IID)?)?;
                service.Connect(VARIANT::default(), VARIANT::default(), VARIANT::default(), VARIANT::default())?;
                let root = BSTR::from_str("\\")?;
                let folder = service.GetFolder(Some(root.0))?;
                let name = BSTR::from_str(CONSOLE_TASK)?;
                let task = match folder.GetTask(name.0) {
                    Ok(task) => task,
                    Err(e) if e == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) || e == HRESULT::from_win32(ERROR_PATH_NOT_FOUND.0) => return Ok(None),
                    Err(e) => return Err(e),
                };
                Ok(Some(Self { task, _apartment: apartment }))
            }
        };
        open().map_err(|e| format!("cannot check the console task: {e:?}; run setup.ps1"))
    }

    fn run(&self) -> Result<(), String> {
        unsafe {
            if self.task.get_Enabled().map_err(|e| format!("cannot read the console task: {e:?}"))?.0 == 0 {
                return Err("the console task is disabled: run setup.ps1".into());
            }
            // Run asynchronously as SYSTEM using the identity/ACL installed by setup.ps1.
            let _running = self.task.Run(VARIANT::default()).map_err(|e| format!("the console task could not run: {e:?}; run setup.ps1"))?;
        }
        Ok(())
    }
}

/// No side effects: used at startup, by --check, and before a viewer is welcomed.
/// Ordinary status must not claim that a move is already underway.
pub(super) fn status(moving: bool) -> String {
    let check = || -> Result<String, String> {
        let session = current()?;
        let task = ConsoleTask::open()?;
        let decision = decide(session, task.is_some());
        let mut note = decision.note.to_string();
        if !moving {
            note = note.replace("; moving it to the console", "; waiting for a viewer to move it to the console");
        }
        if task.is_none() && decision.needs_move {
            let state = if session.disconnected { "the session is disconnected" } else { "this desktop is in a Remote Desktop session" };
            note = format!("{state}; {note}");
        }
        Ok(note)
    };
    check().unwrap_or_else(|e| e)
}

/// Returns whether capture must be restarted after a session move. Never called at startup.
pub(super) fn prepare() -> Result<bool, String> {
    let session = current()?;
    let task = ConsoleTask::open()?;
    let decision = decide(session, task.is_some());
    if !decision.needs_move { return Ok(false); }
    if !decision.can_move { return Err(decision.note.into()); }
    super::log(decision.note);
    task.expect("can_move requires the task").run()?;
    let since = std::time::Instant::now();
    while current()?.id != unsafe { WTSGetActiveConsoleSessionId() } {
        // Retain the existing preparation deadline so a failed task does not block host controls.
        if since.elapsed() > std::time::Duration::from_secs(10) {
            return Err(format!("{}; the console task did not move the session within 10 s", decision.note));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    super::log("the session is on the console");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_move_decision() {
        // The reported incident: user 2 disconnected, console 4 has only a login screen.
        let disconnected = Session { id: 2, console: 4, disconnected: true };
        assert_eq!(decide(disconnected, true), Decision { needs_move: true, can_move: true, note: "the session is disconnected; moving it to the console" });
        assert_eq!(decide(disconnected, false), Decision { needs_move: true, can_move: false, note: MISSING_TASK });
        let remote = Session { disconnected: false, ..disconnected };
        assert!(decide(remote, true).can_move);
        assert!(decide(remote, true).note.contains("Remote Desktop"));
        assert!(!decide(remote, false).can_move);
        for disconnected in [false, true] {
            let console = Session { id: 2, console: 2, disconnected };
            for present in [false, true] {
                assert!(!decide(console, present).needs_move);
                assert!(!decide(console, present).can_move);
            }
            assert_eq!(decide(console, false).note, MISSING_TASK);
        }
        // Windows reports no console during attach/detach transitions.
        assert!(decide(Session { console: u32::MAX, ..disconnected }, true).can_move);
    }
}
