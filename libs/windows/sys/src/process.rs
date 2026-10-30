//! Console-free child processes for windowed apps and background services.

use crate::Win32::System::Threading::CREATE_NO_WINDOW;
use std::os::windows::process::CommandExt;
use std::process::Command;

/// Sets the complete creation flags, adding CREATE_NO_WINDOW. Pass any other flags
/// explicitly: std's creation_flags setter replaces, rather than merges, them.
/// For a process that must outlive its parent's console, use DETACHED_PROCESS
/// instead (Windows ignores CREATE_NO_WINDOW in that combination).
pub fn no_window(command: &mut Command, flags: u32) -> &mut Command {
    command.creation_flags(flags | CREATE_NO_WINDOW.0)
}
