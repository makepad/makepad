// rand 0.8's ReseedingRng: libc::pthread_atfork (glibc: in libc_nonshared.a, not exported by libc.so)
#[cfg(unix)]
extern "C" {
    fn pthread_atfork(prepare: Option<unsafe extern "C" fn()>, parent: Option<unsafe extern "C" fn()>, child: Option<unsafe extern "C" fn()>) -> i32;
}

#[cfg(unix)]
unsafe extern "C" fn mark() {}

#[test]
fn pthread_atfork_registers() {
    #[cfg(unix)]
    assert_eq!(unsafe { pthread_atfork(Some(mark), None, None) }, 0);
}
