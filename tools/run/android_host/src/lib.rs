//! libmakepad.so owns Relax and the Android HAL. JNI is registered at load time;
//! the packed app compiles from source and runs its Cx HAL client on the boot thread.

pub mod boot;
#[cfg(all(target_os = "android", target_arch = "aarch64"))]
mod trampolines;
#[cfg(target_os = "android")]
mod jni;
#[cfg(target_os = "android")]
mod logcat;
#[cfg(target_os = "android")]
mod socket;

/// The APK's executable (libmakepad_exe.so, android_exe) calls this with its own arguments: Relax
/// running itself again, which is what every build script is.
#[no_mangle]
pub extern "C" fn makepad_cli_main(argc: std::ffi::c_int, argv: *const *const std::ffi::c_char) {
    let args: Vec<Vec<u8>> = (0..argc.max(0) as usize)
        .map(|i| unsafe { std::ffi::CStr::from_ptr(*argv.add(i)) }.to_bytes().to_vec())
        .collect();
    #[cfg(target_os = "android")]
    jni::name_host_library();
    makepad_run::run_with_args(args);
}
