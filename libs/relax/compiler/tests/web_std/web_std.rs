// The web layer of Relax's std (std/sys/web.rs, fs_web.rs, process_web.rs, env_web.rs, ...) against
// rustc's std for wasm32-unknown-unknown. `run()` prints one line per observation through the
// host's `relax_web_print`; the output must equal the one of the same program built by rustc for
// wasm32 and run in node (host.js). Panics do not unwind in the rustc build, so the observations of
// panicking calls are Relax-only (`relax_only`, lines prefixed `RELAX-ONLY`), checked against
// relax_only.expected. Driver: run.sh.
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::ffi::{CString, OsString};
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, DefaultHasher};
use std::io::{self, BufRead, BufReader, BufWriter, Cursor, IsTerminal, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, LazyLock, Mutex, Once, OnceLock, RwLock};
use std::time::Duration;

extern "C" {
    fn relax_web_print(ptr: *const u8, len: usize);
}

fn p(s: &str) {
    unsafe { relax_web_print(s.as_ptr(), s.len()) }
}

macro_rules! out {
    ($($a:tt)*) => {{
        let mut s = String::new();
        let _ = write!(s, $($a)*);
        s.push('\n');
        p(&s);
    }};
}

fn show<T: std::fmt::Debug>(name: &str, r: &io::Result<T>) {
    match r {
        Ok(v) => out!("{name}: Ok({v:?})"),
        Err(e) => out!("{name}: Err kind={:?} display={e} debug={e:?}", e.kind()),
    }
}

fn fmt_section() {
    out!("fmt: {:>8.3}|{:<5}|{:^7}|{:#x}|{:08.2}|{:e}|{:+}", 3.14159f64, "ab", "mid", 255, -2.5f32, 1234.5f64, 7);
    out!("fmt: {:?} {:?} {:?}", "tab\t\"q\"\u{1f600}\n", 'x', '\'');
    out!("fmt: {} {} {} {}", f32::MAX, f64::MIN_POSITIVE, 1.0e21f64, 0.1f32 + 0.2f32);
    out!("fmt: {:?} {:?} {:?}", f64::NAN, -f64::INFINITY, -0.0f64);
    out!("fmt: {:#?}", (1u8, "a", [Some(1i32), None]));
    out!("fmt: {:10.3?}|{:<10?}|{:#b}|{:#o}", 1.5f64, Some(2u8), 5u8, 64u8);
    out!("fmt: {} {}", i128::MIN, u128::MAX);
    let w = 12;
    out!("fmt: {:>w$}|{:.*}", "r", 3, 2.718281828f64, w = w);
    out!("fmt: {:?}", String::from("x").as_bytes());
    out!("fmt: {}", format_args!("{}-{}", 1, 2));
}

fn str_section() {
    let s = "Hello, Wörld! ß straße";
    out!("str: {} {} {}", s.to_uppercase(), s.to_lowercase(), s.len());
    out!("str: {:?}", s.split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).collect::<Vec<_>>());
    out!("str: {:?}", s.char_indices().filter(|(_, c)| !c.is_ascii()).collect::<Vec<_>>());
    out!("str: {:?} {:?} {:?}", "42".parse::<u8>(), "-1".parse::<u8>().is_err(), "3.5e2".parse::<f64>());
    out!("str: {:?} {:?}", "x".repeat(3), "a,b,,c".rsplitn(2, ',').collect::<Vec<_>>());
    out!("str: {:?}", String::from_utf8_lossy(&[72, 105, 0xff, 33]));
    let bad = std::hint::black_box([0xc3u8, 0x28]);
    out!("str: {:?}", std::str::from_utf8(&bad).map_err(|e| e.to_string()));
    out!("str: {:?} {}", "  trim\t".trim(), "ab".cmp("b") as i32);
    let c = CString::new("abc").unwrap();
    out!("str: {:?} {:?} {:?}", c, c.as_bytes_with_nul(), CString::new("a\0b").is_err());
    let o = OsString::from("os str");
    out!("str: {:?} {:?}", o, o.to_str());
}

fn collections_section() {
    let mut v: Vec<i32> = (0..20).map(|i| (i * 7919) % 31).collect();
    v.sort();
    v.dedup();
    out!("vec: {:?} cap_ge_len={}", v, v.capacity() >= v.len());
    let mut v2 = vec![5, 3, 8, 1];
    v2.sort_by(|a, b| b.cmp(a));
    v2.insert(1, 99);
    v2.retain(|x| x % 2 == 1);
    out!("vec: {:?} {:?}", v2, v2.binary_search(&99).is_ok());
    let mut bm = BTreeMap::new();
    for (i, w) in ["delta", "alpha", "charlie", "bravo"].iter().enumerate() {
        bm.insert(*w, i);
    }
    out!("btree: {:?} first={:?}", bm, bm.iter().next());
    let bs: BTreeSet<_> = [5, 1, 9, 3].into_iter().collect();
    out!("btreeset: {:?} {:?}", bs, bs.range(2..9).collect::<Vec<_>>());
    // a fixed hasher: the layout of the table is deterministic
    let mut hm: HashMap<u32, &str, BuildHasherDefault<DefaultHasher>> = HashMap::default();
    for (i, w) in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].iter().enumerate() {
        hm.insert(i as u32 * 37, w);
    }
    let mut keys: Vec<_> = hm.keys().copied().collect();
    keys.sort();
    out!("hashmap: len={} keys={:?} get={:?}", hm.len(), keys, hm.get(&37));
    let hs: HashSet<i32> = (0..100).collect();
    out!("hashset: {} {}", hs.len(), hs.iter().sum::<i32>());
    let mut dq: VecDeque<i32> = VecDeque::new();
    for i in 0..6 {
        if i % 2 == 0 { dq.push_back(i) } else { dq.push_front(i) }
    }
    out!("deque: {:?} {:?}", dq, dq.iter().rev().take(2).collect::<Vec<_>>());
    let mut heap = BinaryHeap::new();
    for x in [4, 9, 1, 7] { heap.push(x) }
    out!("heap: {:?} {:?}", heap.pop(), heap.into_sorted_vec());
}

fn rc_sync_section() {
    let a = Arc::new(5);
    let b = a.clone();
    let w = Arc::downgrade(&a);
    out!("arc: {} {} {} {:?}", Arc::strong_count(&a), Arc::weak_count(&a), *b + 1, w.upgrade());
    drop(a);
    drop(b);
    out!("arc: {:?}", w.upgrade());
    let r = Rc::new(RefCell::new(vec![1]));
    r.borrow_mut().push(2);
    out!("rc: {:?} {} {:?}", r, Rc::strong_count(&r), r.try_borrow_mut().is_ok());

    let m = Mutex::new(10);
    {
        let mut g = m.lock().unwrap();
        *g += 1;
        out!("mutex: try_lock while held = {}", m.try_lock().is_err());
    }
    out!("mutex: {:?} {:?} {}", m.lock().map(|g| *g).ok(), m, m.is_poisoned());
    let rw = RwLock::new(String::from("rw"));
    {
        let r1 = rw.read().unwrap();
        let r2 = rw.read().unwrap();
        out!("rwlock: {} {} write_blocked={}", *r1, r2.len(), rw.try_write().is_err());
    }
    rw.write().unwrap().push('!');
    out!("rwlock: {:?}", rw);
    let cv = Condvar::new();
    cv.notify_one();
    cv.notify_all();
    out!("condvar: {:?}", cv);
    static ONCE: Once = Once::new();
    let mut n = 0;
    ONCE.call_once(|| n += 1);
    ONCE.call_once(|| n += 1);
    out!("once: {} {}", n, ONCE.is_completed());
    let oc: OnceLock<String> = OnceLock::new();
    out!("oncelock: {:?} {:?} {:?}", oc.get(), oc.get_or_init(|| "v".into()), oc.set("w".into()));
    static LAZY: LazyLock<Vec<u8>> = LazyLock::new(|| vec![1, 2, 3]);
    out!("lazy: {:?}", *LAZY);
    let bar = Barrier::new(1);
    out!("barrier: {}", bar.wait().is_leader());
    thread_local! {
        static TL: Cell<u32> = const { Cell::new(7) };
        static TLR: RefCell<Vec<String>> = RefCell::new(vec!["init".to_string()]);
    }
    TL.with(|c| c.set(c.get() * 6));
    TLR.with(|v| v.borrow_mut().push("more".into()));
    out!("thread_local: {} {:?}", TL.get(), TLR.with(|v| v.borrow().clone()));
    let (tx, rx) = mpsc::channel();
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    let tx2 = tx.clone();
    drop(tx);
    out!("mpsc: {:?} {:?} {:?}", rx.recv(), rx.try_recv(), rx.try_recv());
    tx2.send(3).unwrap();
    drop(tx2);
    out!("mpsc: {:?} {:?}", rx.recv(), rx.recv());
    let (stx, srx) = mpsc::sync_channel::<u8>(1);
    out!("sync_channel: {:?} {:?} {:?}", stx.try_send(1), stx.try_send(2), srx.try_recv());
}

fn atomics_section() {
    let a = AtomicUsize::new(5);
    out!("atomic: {} {} {:?} {:?}", a.fetch_add(3, Ordering::SeqCst), a.fetch_sub(1, Ordering::Relaxed), a.compare_exchange(7, 100, Ordering::AcqRel, Ordering::Acquire), a.compare_exchange(7, 1, Ordering::SeqCst, Ordering::SeqCst));
    let b = AtomicBool::new(false);
    out!("atomic: {} {} {}", b.swap(true, Ordering::SeqCst), b.fetch_and(false, Ordering::SeqCst), b.load(Ordering::SeqCst));
    let c = AtomicI64::new(-5);
    out!("atomic: {} {} {}", c.fetch_max(3, Ordering::SeqCst), c.fetch_xor(0xff, Ordering::SeqCst), c.load(Ordering::SeqCst));
    let d = AtomicU32::new(0xf0);
    out!("atomic: {:?} {}", d.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| Some(x >> 4)), d.load(Ordering::SeqCst));
    let mut x = 5u32;
    let ptr = AtomicPtr::new(&mut x as *mut u32);
    out!("atomic: {}", unsafe { *ptr.load(Ordering::SeqCst) });
    std::sync::atomic::fence(Ordering::SeqCst);
}

fn thread_section() {
    let r = std::thread::Builder::new().name("worker".into()).spawn(|| 1);
    match r {
        Ok(_) => out!("thread: spawned"),
        Err(e) => out!("thread: spawn Err kind={:?} display={e}", e.kind()),
    }
    match std::thread::available_parallelism() {
        Ok(n) => out!("thread: parallelism {n}"),
        Err(e) => out!("thread: parallelism Err kind={:?} display={e}", e.kind()),
    }
    std::thread::yield_now();
    out!("thread: panicking={}", std::thread::panicking());
}

fn io_section() {
    let mut so = io::stdout();
    show("stdout.write", &so.write(b"to the sink"));
    show("stdout.write_all", &so.write_all(b"all\n").map(|_| ()));
    show("stdout.flush", &so.flush());
    show("stderr.write", &io::stderr().write(b"err"));
    println!("a line to nowhere");
    eprintln!("an error line to nowhere");
    print!("no newline");
    out!("stdout.is_terminal={} stdin.is_terminal={} stderr.is_terminal={}", io::stdout().is_terminal(), io::stdin().is_terminal(), io::stderr().is_terminal());
    let mut line = String::new();
    show("stdin.read_line", &io::stdin().read_line(&mut line));
    out!("stdin line: {:?}", line);
    let mut buf = Vec::new();
    show("stdin.read_to_end", &io::stdin().read_to_end(&mut buf));
    show("stdin.read_exact", &io::stdin().read_exact(&mut [0u8; 1]));
    show("stdin.lines", &io::stdin().lock().lines().next().transpose());
    let mut c = Cursor::new(b"line one\nline two\nthree".to_vec());
    let mut s = String::new();
    c.read_line(&mut s).unwrap();
    out!("cursor: {:?} pos={}", s, c.position());
    let rest: Vec<String> = BufReader::new(c).lines().map(|l| l.unwrap()).collect();
    out!("bufreader: {:?}", rest);
    let mut bw = BufWriter::new(Vec::new());
    write!(bw, "{}-{}", 1, 2).unwrap();
    out!("bufwriter: {:?}", bw.into_inner().unwrap());
    let mut sink = Vec::new();
    out!("copy: {:?}", io::copy(&mut &b"12345"[..], &mut sink).unwrap());
    let e = io::Error::new(io::ErrorKind::Other, "custom");
    out!("error: {e} | {e:?} | {:?}", e.kind());
    let e = io::Error::from_raw_os_error(5);
    out!("error: {e} | {e:?} | {:?} | {:?}", e.kind(), e.raw_os_error());
    let e = io::Error::last_os_error();
    out!("error: {e} | {:?} | {:?}", e.kind(), e.raw_os_error());
    let e: io::Error = io::ErrorKind::NotFound.into();
    out!("error: {e} | {e:?}");
    let e = io::Error::new(io::ErrorKind::InvalidInput, io::Error::from(io::ErrorKind::TimedOut));
    out!("error: {e} | {e:?} | src={:?}", std::error::Error::source(&e).map(|s| s.to_string()));
    let (mut a, mut b) = ([1u8, 2, 3, 4], [0u8; 2]);
    out!("slice io: {:?} {:?}", (&a[..]).read(&mut b).unwrap(), b);
    out!("slice io: {:?} {:?}", (&mut a[..]).write(&[9, 9]).unwrap(), a);
    show("pipe", &io::pipe().map(|_| ()));
}

fn fs_section() {
    show("fs::read", &std::fs::read("/etc/passwd"));
    show("fs::read_to_string", &std::fs::read_to_string("a.txt"));
    show("fs::write", &std::fs::write("out.bin", b"x"));
    show("File::open", &std::fs::File::open("x").map(|_| ()));
    show("File::create", &std::fs::File::create("x").map(|_| ()));
    show("OpenOptions", &std::fs::OpenOptions::new().read(true).write(true).append(true).create(true).open("x").map(|_| ()));
    show("fs::metadata", &std::fs::metadata("/").map(|_| ()));
    show("fs::symlink_metadata", &std::fs::symlink_metadata("/").map(|_| ()));
    show("fs::read_dir", &std::fs::read_dir(".").map(|_| ()));
    show("fs::create_dir", &std::fs::create_dir("d"));
    show("fs::create_dir_all(\"\")", &std::fs::create_dir_all(""));
    show("fs::create_dir_all(\"/\")", &std::fs::create_dir_all("/"));
    show("fs::create_dir_all(a/b)", &std::fs::create_dir_all("a/b"));
    show("DirBuilder", &std::fs::DirBuilder::new().recursive(true).create("x/y"));
    show("fs::remove_file", &std::fs::remove_file("x"));
    show("fs::remove_dir", &std::fs::remove_dir("x"));
    show("fs::remove_dir_all", &std::fs::remove_dir_all("x"));
    show("fs::rename", &std::fs::rename("a", "b"));
    show("fs::copy", &std::fs::copy("a", "b"));
    show("fs::hard_link", &std::fs::hard_link("a", "b"));
    show("fs::canonicalize", &std::fs::canonicalize("."));
    show("fs::read_link", &std::fs::read_link("l"));
    show("fs::exists", &std::fs::exists("x"));
    out!("Path::exists={} is_file={} is_dir={}", Path::new("/").exists(), Path::new("x").is_file(), Path::new("/").is_dir());
    show("Path::try_exists", &Path::new("x").try_exists());
    show("Path::canonicalize", &Path::new("x").canonicalize());
    show("Path::read_dir", &Path::new("x").read_dir().map(|_| ()));
}

fn path_section() {
    let pb = PathBuf::from("/usr/local/lib/libfoo.so.1");
    out!("path: {:?} {:?} {:?} {:?}", pb.parent(), pb.file_name(), pb.extension(), pb.file_stem());
    out!("path: abs={} rel={} has_root={}", pb.is_absolute(), pb.is_relative(), pb.has_root());
    out!("path: {:?}", pb.components().collect::<Vec<_>>());
    let mut j = PathBuf::from("base/dir");
    j.push("sub");
    j.push("/rooted");
    j.set_extension("txt");
    out!("path: {:?} {:?}", j, Path::new("a").join("/b").join("c/"));
    out!("path: {:?} {:?}", Path::new("a/./b/../c//d").components().collect::<Vec<_>>(), Path::new("a/b").strip_prefix("a"));
    out!("path: {} {}", Path::new("x/y.z").display(), std::path::MAIN_SEPARATOR);
    show("path::absolute", &std::path::absolute("rel/path"));
    show("path::absolute(/abs)", &std::path::absolute("/abs/../path"));
    out!("path: {:?} {:?}", Path::new("/a/b").starts_with("/a"), Path::new("/a/b").ends_with("b"));
}

fn env_section() {
    out!("env: args count={} {:?} {:?}", std::env::args().count(), std::env::args(), std::env::args_os());
    out!("env: len={} next={:?} rev={:?}", std::env::args().len(), std::env::args().next(), std::env::args().next_back());
    out!("env: {:?} {:?}", std::env::var("HOME"), std::env::var_os("PATH"));
    out!("env: {}", std::env::var("X").unwrap_err());
    show("env::current_dir", &std::env::current_dir());
    show("env::current_exe", &std::env::current_exe());
    show("env::set_current_dir", &std::env::set_current_dir("/"));
    #[allow(deprecated)]
    let home = std::env::home_dir();
    out!("env: home={:?}", home);
    out!("env: {:?} {:?} {:?} {:?} {:?}", std::env::consts::FAMILY, std::env::consts::OS, std::env::consts::EXE_SUFFIX, std::env::consts::DLL_EXTENSION, std::env::consts::DLL_PREFIX);
    match std::env::join_paths(["a", "b"]) {
        Ok(v) => out!("env: join_paths {:?}", v),
        Err(e) => out!("env: join_paths Err {e} | {e:?}"),
    }
}

fn process_section() {
    use std::process::{Command, ExitCode, Stdio};
    let mut c = Command::new("ls");
    c.arg("-l").args(["a", "b c"]).env("K", "v").env("Z", "w").env_remove("GONE").current_dir("/tmp");
    out!("command: {:?}", c);
    out!("command: {:#?}", c);
    out!("command: {:?} {:?} {:?} {:?}", c.get_program(), c.get_args().collect::<Vec<_>>(), c.get_envs().collect::<Vec<_>>(), c.get_current_dir());
    out!("command: {:?} {:?}", c.get_args(), c.get_envs());
    show("spawn", &c.spawn().map(|_| ()));
    show("output", &c.output().map(|_| ()));
    show("status", &c.status().map(|_| ()));
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    out!("command: {:#?}", c);
    let mut e = Command::new("env");
    e.env_clear().env("A", "1");
    out!("command: {:?}", e);
    out!("exitcode: {:?} {:?}", ExitCode::SUCCESS, ExitCode::from(3));
}

fn net_section() {
    show("TcpStream::connect(ip)", &TcpStream::connect("127.0.0.1:80").map(|_| ()));
    show("TcpStream::connect(name)", &TcpStream::connect("example.com:80").map(|_| ()));
    show("TcpListener::bind", &std::net::TcpListener::bind("0.0.0.0:0").map(|_| ()));
    show("UdpSocket::bind", &UdpSocket::bind("0.0.0.0:0").map(|_| ()));
    show("to_socket_addrs(ip)", &"1.2.3.4:5".to_socket_addrs().map(|i| i.collect::<Vec<_>>()));
    show("to_socket_addrs(name)", &"localhost:80".to_socket_addrs().map(|i| i.collect::<Vec<_>>()));
    show("to_socket_addrs(bad)", &"nocolon".to_socket_addrs().map(|i| i.collect::<Vec<_>>()));
    show("to_socket_addrs(port)", &"host:99999".to_socket_addrs().map(|i| i.collect::<Vec<_>>()));
    show("to_socket_addrs((str,port))", &("localhost", 80).to_socket_addrs().map(|i| i.collect::<Vec<_>>()));
    out!("addr: {:?} {:?}", "[::1]:8080".parse::<SocketAddr>(), "1.2.3.4".parse::<IpAddr>());
    out!("addr: {} {} {}", Ipv4Addr::new(10, 0, 0, 1).is_private(), Ipv4Addr::LOCALHOST, Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
    out!("addr: {:?} {}", "256.0.0.1".parse::<Ipv4Addr>().map_err(|e| e.to_string()), Ipv6Addr::from([0u8; 16]));
}

fn time_section() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = Duration::new(5, 999_999_999) + Duration::from_millis(2);
    out!("duration: {:?} {:?} {} {}", d, d.checked_sub(Duration::from_secs(9)), d.as_secs_f64(), d.subsec_micros());
    out!("duration: {:?} {:?}", Duration::from_secs_f64(1.5).mul_f32(2.0), Duration::MAX.checked_add(Duration::from_nanos(1)));
    out!("systemtime: {:?} {:?}", UNIX_EPOCH, UNIX_EPOCH + Duration::new(3, 5));
    out!("systemtime: {:?} {:?}", UNIX_EPOCH.checked_sub(Duration::from_secs(1)), (UNIX_EPOCH + Duration::from_secs(10)).duration_since(UNIX_EPOCH));
    let later = UNIX_EPOCH + Duration::from_secs(2);
    out!("systemtime: {:?}", UNIX_EPOCH.duration_since(later).map_err(|e| (e.to_string(), e.duration())));
    out!("systemtime: {:?} {}", later.checked_add(Duration::from_secs(1)) > Some(later), later == later);
    let _ = SystemTime::UNIX_EPOCH;
}

fn misc_section() {
    out!("misc: {}", std::mem::size_of::<u128>());
    let b: Box<dyn std::any::Any> = Box::new(5i32);
    out!("misc: {:?} {:?}", b.downcast_ref::<i32>(), b.downcast_ref::<u8>());
    let r: Result<u8, std::num::ParseIntError> = "x".parse();
    out!("misc: {:?}", r);
    out!("misc: {:?} {:?}", 10u8.checked_mul(30), 7i32.rem_euclid(-3));
    out!("misc: {:?} {:?}", u32::from_str_radix("zz", 36), char::from_u32(0xd800));
    out!("misc: {:?}", (1..=4).map(|x| x * x).rev().collect::<Vec<_>>());
    let mut it = [1, 2, 3, 4, 5].chunks(2);
    out!("misc: {:?} {:?}", it.next(), it.last());
    let x = std::hint::black_box(41) + 1;
    out!("misc: {}", x);
    out!("misc: {:?}", std::panic::catch_unwind(|| ()).is_ok());
}

pub fn run() {
    out!("web_std start");
    fmt_section();
    str_section();
    collections_section();
    rc_sync_section();
    atomics_section();
    thread_section();
    io_section();
    fs_section();
    path_section();
    env_section();
    process_section();
    net_section();
    time_section();
    misc_section();
    #[cfg(relax)]
    relax_only();
    out!("web_std end");
}

// Observations of calls that panic on rustc's wasm32 (it aborts there: no unwinding), caught here.
#[cfg(relax)]
fn relax_only() {
    fn caught<F: FnOnce() + std::panic::UnwindSafe>(name: &str, f: F) {
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(f);
        let _ = std::panic::take_hook();
        match r {
            Ok(()) => out!("RELAX-ONLY {name}: returned"),
            Err(e) => {
                let m = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
                out!("RELAX-ONLY {name}: panicked {:?}", m);
            }
        }
    }
    caught("Instant::now", || drop(std::time::Instant::now()));
    caught("SystemTime::now", || drop(std::time::SystemTime::now()));
    caught("set_var", || std::env::set_var("K", "V"));
    caught("remove_var", || std::env::remove_var("K"));
    caught("vars", || drop(std::env::vars()));
    caught("vars_os", || drop(std::env::vars_os()));
    caught("temp_dir", || drop(std::env::temp_dir()));
    caught("split_paths", || drop(std::env::split_paths("a:b")));
    caught("process::id", || drop(std::process::id()));
    caught("File::open unwrap", || drop(std::fs::File::open("x").unwrap()));
}

#[cfg(relax)]
fn main() {
    run();
}

#[cfg(not(relax))]
#[no_mangle]
pub extern "C" fn run_export() {
    run();
}
