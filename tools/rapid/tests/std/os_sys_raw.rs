// std-os lane: the raw libSystem/glibc calls std's sys layer makes, self-contained (no std
// types beyond println), so Rapid's C ABI for exactly these shapes is checked before the
// std modules themselves compile under Rapid. Differential: os_diff.sh --rapid.
// macOS arm64 only for now (Linux constants/layouts differ; see std/std/sys/linux.rs).

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
struct Stat {
    st_dev: i32,
    st_mode: u16,
    st_nlink: u16,
    st_ino: u64,
    st_uid: u32,
    st_gid: u32,
    st_rdev: i32,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
    st_birthtime: i64,
    st_birthtime_nsec: i64,
    st_size: i64,
    st_blocks: i64,
    st_blksize: i32,
    st_flags: u32,
    st_gen: u32,
    st_lspare: i32,
    st_qspare: [i64; 2],
}

#[repr(C)]
struct Dirent {
    d_ino: u64,
    d_seekoff: u64,
    d_reclen: u16,
    d_namlen: u16,
    d_type: u8,
    d_name: [u8; 1024],
}

extern "C" {
    fn clock_gettime(clock: u32, tp: *mut Timespec) -> i32;
    fn nanosleep(req: *const Timespec, rem: *mut Timespec) -> i32;
    fn open(path: *const u8, flags: i32, ...) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn unlink(path: *const u8) -> i32;
    fn stat(path: *const u8, buf: *mut Stat) -> i32;
    fn fstat(fd: i32, buf: *mut Stat) -> i32;
    fn opendir(path: *const u8) -> *mut u8;
    fn readdir(dir: *mut u8) -> *mut Dirent;
    fn closedir(dir: *mut u8) -> i32;
    fn mkdir(path: *const u8, mode: u32) -> i32;
    fn rmdir(path: *const u8) -> i32;
    fn __error() -> *mut i32;
    fn strerror_r(code: i32, buf: *mut u8, len: usize) -> i32;
    fn strlen(s: *const i8) -> usize;
    fn getenv(name: *const u8) -> *const u8;
    fn setenv(name: *const u8, value: *const u8, overwrite: i32) -> i32;
    fn os_sync_wait_on_address_with_timeout(addr: *mut u8, value: u64, size: usize, flags: u32, clockid: u32, timeout_ns: u64) -> i32;
    fn os_sync_wake_by_address_any(addr: *mut u8, size: usize, flags: u32) -> i32;
    fn pthread_key_create(key: *mut usize, dtor: extern "C" fn(*mut u8)) -> i32;
    fn pthread_setspecific(key: usize, value: *const u8) -> i32;
    fn pthread_getspecific(key: usize) -> *mut u8;
    fn pthread_self() -> usize;
    fn pthread_threadid_np(thread: usize, id: *mut u64) -> i32;
    fn sysconf(name: i32) -> i64;
    fn getpid() -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn pthread_create(t: *mut usize, attr: *const u8, f: extern "C" fn(*mut u8) -> *mut u8, arg: *mut u8) -> i32;
    fn pthread_join(t: usize, ret: *mut *mut u8) -> i32;
    fn os_sync_wait_on_address(addr: *mut u8, value: u64, size: usize, flags: u32) -> i32;
    fn os_sync_wake_by_address_all(addr: *mut u8, size: usize, flags: u32) -> i32;
}

static mut GATE: u32 = 0;

struct Job {
    n: u64,
    out: u64,
}

fn work(n: u64) -> u64 {
    let mut s = 0u64;
    let mut i = 0;
    while i < n {
        s = s.wrapping_mul(31).wrapping_add(i);
        i += 1;
    }
    s
}

/// Worker: blocks on the futex until the main thread opens GATE, then computes.
extern "C" fn worker(p: *mut u8) -> *mut u8 {
    unsafe {
        let gate = &mut GATE as *mut u32 as *mut u8;
        while GATE == 0 {
            os_sync_wait_on_address(gate, 0, 4, 0);
        }
        let j = p as *mut Job;
        (*j).out = work((*j).n);
    }
    0 as *mut u8
}

static mut FUTEX: u32 = 7;
static mut DTOR_RUNS: u32 = 0;

extern "C" fn key_dtor(_p: *mut u8) {
    unsafe {
        DTOR_RUNS += 1;
    }
}

fn errno() -> i32 {
    unsafe { *__error() }
}

fn c_text(p: *const u8) -> usize {
    unsafe { strlen(p as *const i8) }
}

#[test]
fn a01_clock_sleep() {
    let mut a = Timespec { tv_sec: 0, tv_nsec: 0 };
    let mut b = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe {
        clock_gettime(8, &mut a as *mut Timespec);
        let req = Timespec { tv_sec: 0, tv_nsec: 3_000_000 };
        let r = nanosleep(&req as *const Timespec, 0 as *mut Timespec);
        clock_gettime(8, &mut b as *mut Timespec);
        let d = (b.tv_sec - a.tv_sec) * 1_000_000_000 + (b.tv_nsec - a.tv_nsec);
        println!("| sleep {} {}", r, d >= 3_000_000 && d < 2_000_000_000);
        let mut rt = Timespec { tv_sec: 0, tv_nsec: 0 };
        clock_gettime(0, &mut rt as *mut Timespec);
        println!("| realtime {}", rt.tv_sec > 1_700_000_000);
    }
}

#[test]
fn a02_files() {
    let path = b"/tmp/hr_os_sys_raw.txt\0" as *const [u8; 23] as *const u8;
    unsafe {
        // O_WRONLY|O_CREAT|O_TRUNC|O_CLOEXEC, mode passed as a variadic argument
        let fd = open(path, 0x1 | 0x200 | 0x400 | 0x1000000, 0o640u32);
        println!("| open {}", fd > 2);
        let msg = b"hello sys layer" as *const [u8; 15] as *const u8;
        println!("| write {}", write(fd, msg, 15));
        let mut st = Stat { st_dev: 0, st_mode: 0, st_nlink: 0, st_ino: 0, st_uid: 0, st_gid: 0, st_rdev: 0, st_atime: 0, st_atime_nsec: 0, st_mtime: 0, st_mtime_nsec: 0, st_ctime: 0, st_ctime_nsec: 0, st_birthtime: 0, st_birthtime_nsec: 0, st_size: 0, st_blocks: 0, st_blksize: 0, st_flags: 0, st_gen: 0, st_lspare: 0, st_qspare: [0; 2] };
        println!("| fstat {} size {} mode {:o} nlink {}", fstat(fd, &mut st as *mut Stat), st.st_size, st.st_mode & 0o777, st.st_nlink);
        println!("| fcntl getfd {}", fcntl(fd, 1));
        close(fd);
        let fd = open(path, 0);
        let mut buf = [0u8; 64];
        let n = read(fd, &mut buf as *mut [u8; 64] as *mut u8, 64);
        println!("| read {} {} {} {}", n, buf[0], buf[1], buf[4]);
        close(fd);
        println!("| stat {} {}", stat(path, &mut st as *mut Stat), st.st_size);
        unlink(path);
        println!("| stat missing {} errno {}", stat(path, &mut st as *mut Stat), errno());
        let mut eb = [0u8; 128];
        strerror_r(2, &mut eb as *mut [u8; 128] as *mut u8, 128);
        let n = c_text(&eb as *const [u8; 128] as *const u8);
        println!("| strerror {} {} {}", n, eb[0], eb[n - 1]);
    }
}

#[test]
fn a03_dirs() {
    let dir = b"/tmp/hr_os_sys_raw_dir\0" as *const [u8; 23] as *const u8;
    unsafe {
        mkdir(dir, 0o755);
        let f = b"/tmp/hr_os_sys_raw_dir/entry_name\0" as *const [u8; 34] as *const u8;
        let fd = open(f, 0x1 | 0x200, 0o644u32);
        close(fd);
        let d = opendir(dir);
        let mut names = 0;
        let mut saw = false;
        loop {
            let e = readdir(d);
            if e as usize == 0 {
                break;
            }
            names += 1;
            let len = (*e).d_namlen as usize;
            let ent = &*e;
            if len == 10 && ent.d_name[0] == b'e' && ent.d_name[6] == b'n' && ent.d_name[9] == b'e' {
                saw = (*e).d_type == 8;
            }
        }
        closedir(d);
        println!("| readdir {} {}", names, saw);
        unlink(f);
        println!("| rmdir {}", rmdir(dir));
    }
}

#[test]
fn a04_futex_tls_env() {
    unsafe {
        let addr = &mut FUTEX as *mut u32 as *mut u8;
        // value differs: returns at once
        let r1 = os_sync_wait_on_address_with_timeout(addr, 8, 4, 0, 32, 1_000_000);
        // value matches: times out after 2 ms
        let r2 = os_sync_wait_on_address_with_timeout(addr, 7, 4, 0, 32, 2_000_000);
        let e2 = errno();
        println!("| wait {} {} {}", r1 >= 0, r2, e2);
        let w = os_sync_wake_by_address_any(addr, 4, 0);
        println!("| wake no waiter {} {}", w, errno());
        let mut key = 0usize;
        println!("| key {}", pthread_key_create(&mut key as *mut usize, key_dtor));
        pthread_setspecific(key, 0x1234 as *const u8);
        println!("| tls {:x}", pthread_getspecific(key) as usize);
        let mut tid = 0u64;
        pthread_threadid_np(0, &mut tid as *mut u64);
        println!("| tid {} self {}", tid > 0, pthread_self() != 0);
        setenv(b"HR_SYS_RAW\0" as *const [u8; 11] as *const u8, b"yes\0" as *const [u8; 4] as *const u8, 1);
        let v = getenv(b"HR_SYS_RAW\0" as *const [u8; 11] as *const u8);
        println!("| env {}", c_text(v));
        println!("| cpus {} pid {}", sysconf(58) >= 1, getpid() > 0);
        println!("| dtor runs so far {}", DTOR_RUNS);
    }
}

#[test]
fn a05_threads_futex() {
    let mut jobs = [Job { n: 1000, out: 0 }, Job { n: 2000, out: 0 }, Job { n: 3000, out: 0 }, Job { n: 4000, out: 0 }];
    let mut ts = [0usize; 4];
    let mut i = 0;
    unsafe {
        while i < 4 {
            pthread_create(&mut ts[i] as *mut usize, 0 as *const u8, worker, &mut jobs[i] as *mut Job as *mut u8);
            i += 1;
        }
        let req = Timespec { tv_sec: 0, tv_nsec: 2_000_000 };
        nanosleep(&req as *const Timespec, 0 as *mut Timespec);
        GATE = 1;
        os_sync_wake_by_address_all(&mut GATE as *mut u32 as *mut u8, 4, 0);
        i = 0;
        while i < 4 {
            pthread_join(ts[i], 0 as *mut *mut u8);
            println!("| job {} {}", i, jobs[i].out);
            i += 1;
        }
    }
}
