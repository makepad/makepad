//! The compiler export probe on a desktop: `probe <package> <export prefix>...` in a checkout compiles the
//! package's exported functions (Relax `run --exports`), prints their addresses and calls the ones
//! named `call=<symbol>` from a thread Relax did not start (as the Activity's UI thread calls
//! the natives). This exercises foreign-thread JIT calls independently of Android boot.


fn main() {
    // Relax runs itself again for each build script (this executable, as the APK's libmakepad_exe.so)
    if std::env::args().nth(1).as_deref() == Some("run") {
        relax_runner::main(&makepad_run::HOST);
        return;
    }
    let mut words = std::env::args().skip(1);
    let package = words.next().expect("usage: probe <package> <export prefix>... [call=<symbol>]...");
    let mut exports = Vec::new();
    let mut calls = Vec::new();
    for w in words {
        match w.strip_prefix("call=") {
            Some(c) => calls.push(c.to_string()),
            None => exports.push(w),
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("run-boot".into())
        .stack_size(64 << 20)
        .spawn(move || {
            let _zone = relax_compiler::zone_alloc::compiler();
            relax::init_embedded(&std::env::current_exe().expect("current_exe"));
            let code = relax::run_checkout(
                vec!["--exports".into(), exports.join(",")],
                vec![package, "--no-default-features".into()],
            );
            let r = if code == 0 { Ok(relax_compiler::run::exports()) }
                else { Err(format!("compile failed: {code}")) };
            tx.send(r).unwrap();
            // the thread that booted the program stays, as in the host
            loop {
                std::thread::park();
            }
        })
        .unwrap();
    let exports = match rx.recv().unwrap() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("probe: boot failed: {e}");
            std::process::exit(1);
        }
    };
    for (symbol, addr) in &exports {
        println!("export {symbol} = {addr:#x}");
    }
    // call=<symbol>[:<a>,<b>]: from a thread Relax did not start; with arguments the function is
    // `extern "C" fn(i32, i32) -> i32`, else `fn(*mut c_void)`; `ret64=<symbol>` is `fn() -> i64`
    for call in &calls {
        let (name, args) = call.split_once(':').map_or((call.as_str(), None), |(n, a)| (n, Some(a)));
        let (name, wide) = match name.strip_prefix("ret64=") {
            Some(n) => (n, true),
            None => (name, false),
        };
        let Some((_, addr)) = exports.iter().find(|(s, _)| s == name) else {
            eprintln!("probe: no export {name}");
            std::process::exit(1);
        };
        let addr = *addr as usize;
        let args: Vec<i32> = args.map(|a| a.split(',').map(|x| x.parse().expect("integer argument")).collect()).unwrap_or_default();
        let out = std::thread::spawn(move || unsafe {
            if wide {
                let f: extern "C" fn() -> i64 = std::mem::transmute(addr);
                format!("{}", f())
            } else if args.len() == 2 {
                let f: extern "C" fn(i32, i32) -> i32 = std::mem::transmute(addr);
                format!("{}", f(args[0], args[1]))
            } else {
                let f: extern "C" fn(*mut std::ffi::c_void) = std::mem::transmute(addr);
                f(0x1234 as *mut std::ffi::c_void);
                "()".to_string()
            }
        })
        .join()
        .unwrap();
        println!("called {call} from a foreign thread: {out}");
    }
    std::process::exit(0);
}
