// The OS extension impls real std has on each platform and the JIT's std must have too: a
// thread's handle, the standard streams', handles and sockets behind Box / Rc / Arc, a child's
// pipes as owned descriptors. A Windows app that asked a JoinHandle for its handle
// (`thread.as_raw_handle()`, to cancel the thread's blocking read) failed to compile under the
// JIT with E0599 while rustc took it. Every test passes under rustc --test.

#[cfg(windows)]
mod windows {
    use std::error::Error;
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{self, IsTerminal};
    use std::iter::FusedIterator;
    use std::net::TcpListener;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{
        AsHandle, AsRawHandle, AsRawSocket, AsSocket, BorrowedHandle, FromRawHandle, HandleOrInvalid, HandleOrNull, IntoRawHandle,
        OwnedHandle, RawHandle, RawSocket,
    };
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::sync::Arc;

    fn handle_of<T: AsHandle>(t: &T) -> RawHandle {
        t.as_handle().as_raw_handle()
    }
    fn socket_of<T: AsSocket>(t: &T) -> RawSocket {
        t.as_socket().as_raw_socket()
    }
    fn shared<T: Send + Sync>() {}

    #[test]
    fn a01_join_handle() {
        let (go, wait) = mpsc::channel::<()>();
        let h = std::thread::spawn(move || {
            let _ = wait.recv();
            7
        });
        let raw = h.as_raw_handle();
        assert!(!raw.is_null());
        assert_eq!(h.as_handle().as_raw_handle(), raw);
        // a live thread's handle duplicates (a closed or made-up one does not)
        let copy = h.as_handle().try_clone_to_owned().unwrap();
        assert!(!copy.as_raw_handle().is_null());
        go.send(()).unwrap();
        assert_eq!(h.join().unwrap(), 7);

        // taken: the handle is the caller's to close
        let h = std::thread::spawn(|| ());
        let raw = h.into_raw_handle();
        let owned = unsafe { OwnedHandle::from_raw_handle(raw) };
        assert_eq!(owned.as_raw_handle(), raw);
        let owned: OwnedHandle = std::thread::spawn(|| ()).into();
        assert!(!owned.as_raw_handle().is_null());
    }

    #[test]
    fn a02_standard_streams() {
        assert_eq!(io::stdin().as_raw_handle(), io::stdin().lock().as_raw_handle());
        assert_eq!(io::stdout().as_raw_handle(), io::stdout().lock().as_raw_handle());
        assert_eq!(io::stderr().as_raw_handle(), io::stderr().lock().as_raw_handle());
        assert_eq!(handle_of(&io::stdout()), io::stdout().as_raw_handle());
        assert_eq!(io::stdout().as_handle().is_terminal(), io::stdout().is_terminal());
        assert_eq!(io::stderr().lock().as_handle().is_terminal(), io::stderr().is_terminal());
    }

    #[test]
    fn a03_behind_pointers() {
        let path = std::env::temp_dir().join(format!("os_ext_impls_{}.txt", std::process::id()));
        let f = File::create(&path).unwrap();
        let raw = f.as_raw_handle();
        let f = Box::new(f);
        assert_eq!(handle_of(&f), raw);
        let f = Rc::new(*f);
        assert_eq!(handle_of(&f), raw);
        let f = Arc::new(Rc::try_unwrap(f).unwrap());
        assert_eq!(handle_of(&f), raw);
        let owned = OwnedHandle::from(Arc::try_unwrap(f).unwrap());
        assert!(!owned.is_terminal());
        drop(owned);
        std::fs::remove_file(&path).unwrap();

        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let raw = l.as_raw_socket();
        assert_eq!(socket_of(&&l), raw);
        let mut l = Box::new(l);
        assert_eq!(socket_of(&&mut *l), raw);
        assert_eq!(socket_of(&l), raw);
        let l = Rc::new(*l);
        assert_eq!(socket_of(&l), raw);
        let l = Arc::new(Rc::try_unwrap(l).unwrap());
        assert_eq!(socket_of(&l), raw);
        assert!(format!("{:?}", l.as_socket()).starts_with("BorrowedSocket"));
    }

    #[test]
    fn a04_handle_or_invalid() {
        shared::<HandleOrInvalid>();
        shared::<HandleOrNull>();
        let e = OwnedHandle::try_from(unsafe { HandleOrNull::from_raw_handle(std::ptr::null_mut()) }).unwrap_err();
        let e: Box<dyn Error> = Box::new(e);
        assert!(e.to_string().contains("null"));
        let e = OwnedHandle::try_from(unsafe { HandleOrInvalid::from_raw_handle(-1isize as RawHandle) }).unwrap_err();
        let e: &dyn Error = &e;
        assert!(e.source().is_none());

        // a valid one converts, and the handle stays open in the OwnedHandle
        let mine = io::stdout().as_handle().try_clone_to_owned();
        if let Ok(mine) = mine {
            let raw = mine.into_raw_handle();
            let owned = OwnedHandle::try_from(unsafe { HandleOrNull::from_raw_handle(raw) }).unwrap();
            assert_eq!(owned.as_raw_handle(), raw);
            let borrowed: BorrowedHandle<'_> = owned.as_handle();
            assert!(borrowed.try_clone_to_owned().is_ok());
            // one never taken is closed when it drops
            let raw = owned.into_raw_handle();
            drop(unsafe { HandleOrInvalid::from_raw_handle(raw) });
        }
    }

    #[test]
    fn a05_encode_wide() {
        fn fused<I: FusedIterator<Item = u16>>(i: I) -> Vec<u16> {
            i.collect()
        }
        let s = OsStr::new("a\u{10000}");
        assert_eq!(fused(s.encode_wide()), [0x61, 0xd800, 0xdc00]);
        assert_eq!(format!("{:?}", s.encode_wide()), "EncodeWide(['a', 0xD800, 0xDC00])");
    }
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
    use std::os::unix::thread::JoinHandleExt;
    use std::process::{ChildStdin, ChildStdout, Command, Stdio};
    use std::rc::Rc;
    use std::sync::Arc;

    fn fd_of<T: AsFd>(t: &T) -> RawFd {
        t.as_fd().as_raw_fd()
    }
    fn raw_of<T: AsRawFd>(t: &T) -> RawFd {
        t.as_raw_fd()
    }

    #[test]
    fn a01_join_handle() {
        let h = std::thread::spawn(|| 3);
        assert!(h.as_pthread_t() != 0);
        assert_eq!(h.join().unwrap(), 3);
    }

    #[test]
    fn a03_behind_pointers() {
        let f = File::open("/dev/null").unwrap();
        let raw = f.as_raw_fd();
        let f = Box::new(f);
        assert_eq!((fd_of(&f), raw_of(&f)), (raw, raw));
        let f = Rc::new(*f);
        assert_eq!((fd_of(&f), raw_of(&f)), (raw, raw));
        let f = Arc::new(Rc::try_unwrap(f).unwrap());
        assert_eq!((fd_of(&f), raw_of(&f)), (raw, raw));

        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let fd = OwnedFd::from(l);
        let l = TcpListener::from(fd);
        assert_eq!(l.local_addr().unwrap(), addr);
    }

    #[test]
    fn a06_child_pipes() {
        let mut child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        assert_eq!(fd_of(&stdin), stdin.as_raw_fd());
        assert_eq!(fd_of(&stdout), stdout.as_raw_fd());
        // through owned descriptors and back
        let mut stdin = ChildStdin::from(OwnedFd::from(stdin));
        let mut stdout = ChildStdout::from(OwnedFd::from(stdout));
        stdin.write_all(b"through the pipe\n").unwrap();
        drop(stdin);
        let mut got = String::new();
        stdout.read_to_string(&mut got).unwrap();
        assert_eq!(got, "through the pipe\n");
        assert!(child.wait().unwrap().success());

        let mut child = Command::new("sh").args(["-c", "echo err 1>&2"]).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let raw = child.stdin.take().unwrap().into_raw_fd();
        drop(unsafe { OwnedFd::from_raw_fd(raw) });
        let raw = child.stderr.take().unwrap().into_raw_fd();
        let mut err = String::new();
        unsafe { File::from_raw_fd(raw) }.read_to_string(&mut err).unwrap();
        assert_eq!(err, "err\n");
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn a07_stdio_from_raw_fd() {
        let (mut r, w) = std::io::pipe().unwrap();
        let out = unsafe { Stdio::from_raw_fd(w.into_raw_fd()) };
        let status = Command::new("echo").arg("from a raw fd").stdout(out).status().unwrap();
        assert!(status.success());
        let mut got = String::new();
        r.read_to_string(&mut got).unwrap();
        assert_eq!(got, "from a raw fd\n");
    }
}
