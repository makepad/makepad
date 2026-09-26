//! Run one hosted app in this process: `dlopen` its library (the APK's, or
//! one built on the phone) and call its `makepad_hosted_main`, the
//! `app_main!` export for a hosted child (platform/src/os/linux/android/
//! android_hosted.rs). An app's library may live where the phone forbids
//! exec but allows mapping code; this executable lives where exec is allowed.
//!
//! `makepad-launch --build <bin> <apk library> [args]`: build the app on the
//! phone first — the desktop WM's `cargo run --release -p <app>` — from the
//! toolchain, source tree and target/ the APK carries (`cargo makepad
//! android proc-pack --proc-toolchain`), provisioned into the app's files
//! dir on first use. cargo's lines go to stdout, which the WM puts on the
//! app's tile and in its log. The fresh `libapp_<bin>.so` runs; a failed
//! build runs the APK's library and says so.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

fn last_dl_error() -> String {
    let err = unsafe { dlerror() };
    if err.is_null() {
        "unknown error".to_string()
    } else {
        unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned()
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        eprintln!("usage: makepad-launch [--build <bin>] <app library> [args]");
        std::process::exit(2);
    };
    let library = if first == "--build" {
        let (Some(bin), Some(apk_library)) = (args.next(), args.next()) else {
            eprintln!("usage: makepad-launch --build <bin> <app library> [args]");
            std::process::exit(2);
        };
        match build::build(&bin) {
            Ok(built) => built,
            Err(e) => {
                println!("on-device build of {bin} failed: {e}");
                println!("running the APK's library instead");
                apk_library
            }
        }
    } else {
        first
    };
    // The app reads its mode from the environment: its library's own copy
    // of std never saw this process's arguments.
    std::env::set_var("MAKEPAD_STDIN_LOOP", "1");
    let path = CString::new(library.clone()).expect("library path");
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        eprintln!("makepad-launch: cannot load {library}: {}", last_dl_error());
        std::process::exit(1);
    }
    let symbol = unsafe { dlsym(handle, c"makepad_hosted_main".as_ptr()) };
    if symbol.is_null() {
        eprintln!("makepad-launch: {library} has no makepad_hosted_main: {}", last_dl_error());
        std::process::exit(1);
    }
    let entry: extern "C" fn() = unsafe { std::mem::transmute(symbol) };
    entry();
}

mod build {
    //! The on-device build (makepad-ondevice-build) with the APK file as the
    //! asset source.
    use makepad_ondevice_build::{provision, run_cargo, Assets, Toolchain, TARGET};
    use makepad_zip_file::{zip_read_central_directory, CentralDirectoryFileHeader, LocalFileHeader, COMPRESS_METHOD_UNCOMPRESSED};
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// Waiting for another launcher's provisioning or build (the home tiles
    /// start together): a first provisioning is well under a minute on the
    /// Pixel; beyond this something is wedged and the APK's library runs.
    const LOCK_DEADLINE: Duration = Duration::from_secs(300);
    /// One app build; an unchanged app is a few seconds, an edited one ~15.
    const BUILD_DEADLINE: Duration = Duration::from_secs(300);

    /// The APK's `assets/…` entries, read from the file (the hosted-child
    /// environment names it: android_hosted.rs `APK_ENV`).
    struct ApkFile {
        path: String,
        file: File,
        entries: HashMap<String, CentralDirectoryFileHeader>,
    }

    impl ApkFile {
        fn open(path: &str) -> Result<ApkFile, String> {
            let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
            let dir = zip_read_central_directory(&mut file).map_err(|e| format!("{path}: {e:?}"))?;
            let entries = dir.file_headers.into_iter().map(|h| (h.file_name.clone(), h)).collect();
            Ok(ApkFile { path: path.to_string(), file, entries })
        }
    }

    impl Assets for ApkFile {
        fn load(&mut self, name: &str) -> Option<Vec<u8>> {
            let h = self.entries.get(&format!("assets/{name}"))?;
            h.extract(&mut self.file).ok()
        }
        /// A STORED entry (the archive parts are packed stored) as a window
        /// of its own file handle: nothing is read ahead.
        fn open(&mut self, name: &str) -> Option<Box<dyn Read>> {
            let h = self.entries.get(&format!("assets/{name}"))?;
            let mut file = File::open(&self.path).ok()?;
            file.seek(SeekFrom::Start(h.relative_offset_of_local_header as u64)).ok()?;
            let local = LocalFileHeader::from_stream(&mut file).ok()?;
            if local.compression_method != COMPRESS_METHOD_UNCOMPRESSED {
                return None;
            }
            Some(Box::new(file.take(h.uncompressed_size as u64)))
        }
    }

    extern "C" {
        fn flock(fd: i32, op: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;

    /// Provision (once per pack), `cargo build -p makepad-hosted-<bin>`,
    /// then copy the library to a name of its own (`files/run/`) and return
    /// that path — all under one lock shared by every launcher: another
    /// launcher's build rewrites `target/.../libapp_<bin>.so` in place, and
    /// a copy renamed into place is never seen half-written by `dlopen`.
    pub fn build(bin: &str) -> Result<String, String> {
        let apk = std::env::var("MAKEPAD_ANDROID_APK").map_err(|_| "MAKEPAD_ANDROID_APK is not set".to_string())?;
        let data = std::env::var("MAKEPAD_ANDROID_DATA_PATH").map_err(|_| "MAKEPAD_ANDROID_DATA_PATH is not set".to_string())?;
        let nlib = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(PathBuf::from))
            .ok_or("the launcher's directory")?;
        let mut assets = ApkFile::open(&apk)?;
        let tc = Toolchain::new(PathBuf::from(&data), nlib, &mut assets)?;
        let say = &mut |line: String| println!("{line}");
        let lock_path = tc.data.join(".wmdyn-lock");
        let lock = File::create(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;
        let waited = Instant::now();
        {
            use std::os::fd::AsRawFd;
            let mut said = false;
            while unsafe { flock(lock.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
                if waited.elapsed() > LOCK_DEADLINE {
                    return Err(format!("another build held the lock for {} s", LOCK_DEADLINE.as_secs()));
                }
                if !said {
                    say("waiting for another app's build".to_string());
                    said = true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let t = Instant::now();
        provision(&tc, &mut assets, say)?;
        let package = format!("makepad-hosted-{bin}");
        let mut cmd = tc.cargo();
        cmd.current_dir(tc.src());
        cmd.args(["build", "--release", "--offline", "--frozen", "--target", TARGET, "-p", &package]);
        say(format!("cargo build -p {package}"));
        let started = Instant::now();
        run_cargo(cmd, &format!("cargo build -p {package}"), BUILD_DEADLINE, say)?;
        let stem = format!("libapp_{}", bin.replace('-', "_"));
        let lib = tc.release_dir().join(format!("{stem}.so"));
        if !lib.is_file() {
            return Err(format!("{} missing after the build", lib.display()));
        }
        let run = publish(&tc.data.join("run"), &stem, &lib)?;
        say(format!(
            "built {} in {:.1} s ({:.1} s with provisioning)",
            run.display(),
            started.elapsed().as_secs_f64(),
            t.elapsed().as_secs_f64()
        ));
        drop(lock);
        Ok(run.to_string_lossy().into_owned())
    }

    /// `<run>/<stem>-<nanos>.so`: written as `.tmp`, renamed into place;
    /// this app's older copies go (a process that mapped one keeps it).
    fn publish(run: &Path, stem: &str, lib: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(run).map_err(|e| format!("{}: {e}", run.display()))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let out = run.join(format!("{stem}-{nanos}.so"));
        let tmp = run.join(format!("{stem}-{nanos}.so.tmp"));
        std::fs::copy(lib, &tmp).map_err(|e| format!("{} -> {}: {e}", lib.display(), tmp.display()))?;
        std::fs::rename(&tmp, &out).map_err(|e| format!("{}: {e}", out.display()))?;
        if let Ok(rd) = std::fs::read_dir(run) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{stem}-")) && entry.path() != out {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Ok(out)
    }
}
