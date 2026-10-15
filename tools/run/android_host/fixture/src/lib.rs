//! The shape of what the host compiles on the phone, in miniature: `#[no_mangle] extern "C"`
//! functions the Java side calls (platform/src/os/linux/android/android_jni.rs), one of which tells
//! the program which JavaVM it is in (libs/android_state `jni_on_load`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};

static VM: AtomicI64 = AtomicI64::new(0);

#[no_mangle]
pub extern "C" fn jni_on_load(vm: *mut c_void) {
    VM.store(vm as i64, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn Java_dev_makepad_android_Fixture_add(a: i32, b: i32) -> i32 {
    // some work the compiler has to do on first call, on the calling thread
    let v: Vec<i32> = (0..a.max(0)).map(|x| x * b).collect();
    v.iter().sum::<i32>() + 1
}

#[no_mangle]
pub extern "C" fn Java_dev_makepad_android_Fixture_vm() -> i64 {
    VM.load(Ordering::SeqCst)
}
