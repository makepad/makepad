//! std::os::windows::process: Windows extensions to std::process (real std's
//! library/std/src/os/windows/process.rs): raw / owned handle conversions for Stdio, Child and
//! its pipes, CommandExt (creation_flags, show_window, force_quotes, raw_arg), ExitStatusExt,
//! ExitCodeExt, ChildExt. Not here: spawn_with_attributes / ProcThreadAttributeList and the
//! startupinfo_* / inherit_handles / async_pipes switches.

use crate::ffi::OsStr;
use crate::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use crate::process;

impl FromRawHandle for process::Stdio {
    unsafe fn from_raw_handle(handle: RawHandle) -> process::Stdio {
        process::Stdio::from(OwnedHandle::from_raw_handle(handle))
    }
}

impl AsRawHandle for process::Child {
    fn as_raw_handle(&self) -> RawHandle {
        self.process_handle().as_raw_handle()
    }
}
impl AsHandle for process::Child {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.process_handle().as_handle()
    }
}
impl IntoRawHandle for process::Child {
    fn into_raw_handle(self) -> RawHandle {
        self.into_process_handle().into_raw_handle()
    }
}
impl From<process::Child> for OwnedHandle {
    fn from(child: process::Child) -> OwnedHandle {
        child.into_process_handle()
    }
}

impl AsRawHandle for process::ChildStdin {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle().as_raw_handle()
    }
}
impl AsRawHandle for process::ChildStdout {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle().as_raw_handle()
    }
}
impl AsRawHandle for process::ChildStderr {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle().as_raw_handle()
    }
}
impl AsHandle for process::ChildStdin {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle().as_handle()
    }
}
impl AsHandle for process::ChildStdout {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle().as_handle()
    }
}
impl AsHandle for process::ChildStderr {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle().as_handle()
    }
}
impl IntoRawHandle for process::ChildStdin {
    fn into_raw_handle(self) -> RawHandle {
        self.into_handle().into_raw_handle()
    }
}
impl IntoRawHandle for process::ChildStdout {
    fn into_raw_handle(self) -> RawHandle {
        self.into_handle().into_raw_handle()
    }
}
impl IntoRawHandle for process::ChildStderr {
    fn into_raw_handle(self) -> RawHandle {
        self.into_handle().into_raw_handle()
    }
}
impl From<process::ChildStdin> for OwnedHandle {
    fn from(c: process::ChildStdin) -> OwnedHandle {
        c.into_handle()
    }
}
impl From<process::ChildStdout> for OwnedHandle {
    fn from(c: process::ChildStdout) -> OwnedHandle {
        c.into_handle()
    }
}
impl From<process::ChildStderr> for OwnedHandle {
    fn from(c: process::ChildStderr) -> OwnedHandle {
        c.into_handle()
    }
}
impl From<OwnedHandle> for process::ChildStdin {
    fn from(handle: OwnedHandle) -> process::ChildStdin {
        process::ChildStdin::from_handle(handle)
    }
}
impl From<OwnedHandle> for process::ChildStdout {
    fn from(handle: OwnedHandle) -> process::ChildStdout {
        process::ChildStdout::from_handle(handle)
    }
}
impl From<OwnedHandle> for process::ChildStderr {
    fn from(handle: OwnedHandle) -> process::ChildStderr {
        process::ChildStderr::from_handle(handle)
    }
}

pub trait ExitStatusExt {
    /// The status from a raw GetExitCodeProcess code.
    fn from_raw(raw: u32) -> Self;
}

impl ExitStatusExt for process::ExitStatus {
    fn from_raw(raw: u32) -> process::ExitStatus {
        process::ExitStatus::from_raw_code(raw)
    }
}

pub trait ExitCodeExt {
    /// An exit code from a raw u32 (the full range ExitProcess takes).
    fn from_raw(raw: u32) -> Self;
}

impl ExitCodeExt for process::ExitCode {
    fn from_raw(raw: u32) -> process::ExitCode {
        process::ExitCode::from_raw_code(raw)
    }
}

pub trait CommandExt {
    /// CreateProcessW's process creation flags (CREATE_NO_WINDOW etc.;
    /// CREATE_UNICODE_ENVIRONMENT is always added).
    fn creation_flags(&mut self, flags: u32) -> &mut process::Command;
    /// STARTUPINFO's wShowWindow (SW_HIDE etc.).
    fn show_window(&mut self, cmd_show: u16) -> &mut process::Command;
    /// Quote every regular argument, not only those with spaces or tabs.
    fn force_quotes(&mut self, enabled: bool) -> &mut process::Command;
    /// Appends `text` to the command line as is: no quoting, no escaping.
    fn raw_arg<S: AsRef<OsStr>>(&mut self, text_to_append_as_is: S) -> &mut process::Command;
}

impl CommandExt for process::Command {
    fn creation_flags(&mut self, flags: u32) -> &mut process::Command {
        self.set_creation_flags(flags);
        self
    }
    fn show_window(&mut self, cmd_show: u16) -> &mut process::Command {
        self.set_show_window(Some(cmd_show));
        self
    }
    fn force_quotes(&mut self, enabled: bool) -> &mut process::Command {
        self.set_force_quotes(enabled);
        self
    }
    fn raw_arg<S: AsRef<OsStr>>(&mut self, raw_text: S) -> &mut process::Command {
        self.push_raw_arg(raw_text.as_ref());
        self
    }
}

pub trait ChildExt {
    /// The handle of the child's main thread.
    fn main_thread_handle(&self) -> BorrowedHandle<'_>;
}

impl ChildExt for process::Child {
    fn main_thread_handle(&self) -> BorrowedHandle<'_> {
        self.main_thread().as_handle()
    }
}
