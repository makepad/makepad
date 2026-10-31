//! Initialize Proton's existing OpenXR adapter without an OpenVR runtime, then
//! launch the original game. No CRT or third-party DLL is bundled with this PE.
#![cfg_attr(target_os = "windows", no_std)]
#![cfg_attr(target_os = "windows", no_main)]

#[cfg(not(target_os = "windows"))]
fn main() { eprintln!("Build makepad-xr-proton for x86_64-pc-windows-msvc; it runs inside Proton."); }

#[cfg(target_os = "windows")]
mod windows {
    use core::{ffi::c_void, ptr::{null, null_mut}};
    type Handle = *mut c_void;
    #[repr(C)]
    struct StartupInfo {
        size: u32, reserved: *mut u16, desktop: *mut u16, title: *mut u16,
        x: u32, y: u32, width: u32, height: u32, chars_x: u32, chars_y: u32,
        fill: u32, flags: u32, show: u16, reserved_size: u16, reserved_data: *mut u8,
        stdin: Handle, stdout: Handle, stderr: Handle,
    }
    #[repr(C)]
    struct ProcessInfo { process: Handle, thread: Handle, pid: u32, tid: u32 }
    #[link(name = "kernel32", kind = "raw-dylib")]
    extern "system" {
        fn LoadLibraryA(name: *const u8) -> Handle;
        fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: Handle) -> i32;
        fn GetCommandLineW() -> *const u16;
        fn GetStdHandle(which: u32) -> Handle;
        fn WriteFile(file: Handle, bytes: *const u8, len: u32, written: *mut u32, overlap: Handle) -> i32;
        fn GetProcessHeap() -> Handle;
        fn HeapAlloc(heap: Handle, flags: u32, size: usize) -> *mut c_void;
        fn HeapFree(heap: Handle, flags: u32, ptr: *mut c_void) -> i32;
        fn CreateProcessW(app: *const u16, command: *mut u16, process_security: Handle,
            thread_security: Handle, inherit: i32, flags: u32, env: Handle,
            directory: *const u16, startup: *const StartupInfo, process: *mut ProcessInfo) -> i32;
        fn WaitForSingleObject(handle: Handle, timeout: u32) -> u32;
        fn GetExitCodeProcess(handle: Handle, code: *mut u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn ExitProcess(code: u32) -> !;
    }
    #[link(name = "advapi32", kind = "raw-dylib")]
    extern "system" {
        fn RegCreateKeyExA(parent: Handle, name: *const u8, reserved: u32, class: *mut u8,
            options: u32, access: u32, security: Handle, key: *mut Handle, disposition: *mut u32) -> i32;
        fn RegSetValueExA(key: Handle, name: *const u8, reserved: u32, ty: u32, value: *const u8, len: u32) -> i32;
        fn RegQueryValueExA(key: Handle, name: *const u8, reserved: *mut u32, ty: *mut u32, value: *mut u8, len: *mut u32) -> i32;
        fn RegDeleteValueA(key: Handle, name: *const u8) -> i32;
        fn RegCloseKey(key: Handle) -> i32;
    }
    unsafe fn log(message: &str) {
        let mut written = 0;
        WriteFile(GetStdHandle(-12i32 as u32), message.as_ptr(), message.len() as u32, &mut written, null_mut());
    }
    unsafe fn state(key: Handle, value: u32) -> bool {
        RegSetValueExA(key, c"state".as_ptr().cast(), 0, 4, (&value as *const u32).cast(), 4) == 0
    }
    unsafe fn initialize() -> bool {
        let mut key = null_mut();
        // This is session state in the current game's Wine prefix, not the
        // system-wide OpenXR runtime or a persistent OpenVR registration.
        if RegCreateKeyExA(0x80000001u32 as i32 as isize as Handle, c"Software\\Wine\\VR".as_ptr().cast(),
            0, null_mut(), 1, 0x2001f, null_mut(), &mut key, null_mut()) != 0 { return false; }
        let names = [c"openxr_vulkan_instance_extensions", c"openxr_vulkan_device_extensions",
            c"openxr_vulkan_device_vid", c"openxr_vulkan_device_pid"];
        for name in names { RegDeleteValueA(key, name.as_ptr().cast()); }
        let mut ok = state(key, 0);
        let module = LoadLibraryA(c"wineopenxr.dll".as_ptr().cast());
        if !module.is_null() {
            let address = GetProcAddress(module, c"wineopenxr_init_registry".as_ptr().cast());
            if !address.is_null() {
                let init: unsafe extern "C" fn() -> i32 = core::mem::transmute(address);
                ok &= init() != 0;
                // The Proton entry point can return true when the headset probe
                // failed. Require its actual runtime-derived results as well.
                for (index, name) in names.iter().enumerate() {
                    let (mut ty, mut len) = (0, 0);
                    ok &= RegQueryValueExA(key, name.as_ptr().cast(), null_mut(), &mut ty, null_mut(), &mut len) == 0
                        && if index < 2 { ty == 1 && len > 1 } else { ty == 4 && len == 4 };
                }
            } else { ok = false; }
            FreeLibrary(module);
        } else { ok = false; }
        ok &= state(key, if ok { 1 } else { u32::MAX });
        RegCloseKey(key);
        ok
    }
    #[no_mangle]
    pub unsafe extern "system" fn mainCRTStartup() -> ! {
        let mut command = GetCommandLineW();
        let mut quoted = false;
        while *command != 0 {
            if *command == 34 { quoted = !quoted; }
            else if !quoted && (*command == 32 || *command == 9) { break; }
            command = command.add(1);
        }
        while *command == 32 || *command == 9 { command = command.add(1); }
        if *command == 0 { log("[makepad-openxr] No game command supplied.\n"); ExitProcess(2); }
        if !initialize() {
            log("[makepad-openxr] OpenXR initialization failed. Connect the Quest in XR Bridge before launching the game.\n");
            ExitProcess(1);
        }
        log("[makepad-openxr] Proton OpenXR initialized without OpenVR; starting game.\n");
        let mut len = 0;
        while *command.add(len) != 0 { len += 1; }
        let heap = GetProcessHeap();
        let copy = HeapAlloc(heap, 0, (len + 1) * 2).cast::<u16>();
        if copy.is_null() { ExitProcess(1); }
        for n in 0..=len { copy.add(n).write(command.add(n).read()); }
        let mut startup: StartupInfo = core::mem::zeroed();
        startup.size = core::mem::size_of::<StartupInfo>() as u32;
        let mut process: ProcessInfo = core::mem::zeroed();
        let launched = CreateProcessW(null(), copy, null_mut(), null_mut(), 1, 0, null_mut(), null(), &startup, &mut process);
        HeapFree(heap, 0, copy.cast());
        if launched == 0 { log("[makepad-openxr] Could not start the game process.\n"); ExitProcess(1); }
        CloseHandle(process.thread);
        WaitForSingleObject(process.process, u32::MAX);
        let mut code = 1;
        GetExitCodeProcess(process.process, &mut code);
        CloseHandle(process.process);
        ExitProcess(code)
    }
    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! { unsafe { ExitProcess(1) } }
}
