//! Building Rust on the phone, the device half: first-run provisioning of
//! what `cargo makepad android dyn-pack` / `proc-pack --proc-toolchain`
//! packed, and the cargo every on-device build runs.
//!
//! Assets (`wmdyn/…`): `stamp` (pack id), `env.txt` (KEY=VALUE: the linker
//! string and RUSTFLAGS of the Mac cross-build — cargo hashes both),
//! `proc-macro-svh.txt` (the SVH each rebuilt proc-macro must carry),
//! `manifest.txt` (`name parts bytes` per archive) and `<name>.NNN` parts
//! (32 MB, stored) of `src.tar.lz4`, `target.tar.lz4`, `tc.tar.lz4`: LZ4
//! frames streamed straight out of the APK into the files dir by
//! makepad-tar, a few megabytes of memory whatever the archive's size.
//!
//! nativeLibraryDir (`apk_data_file`, executable) carries the musl loader
//! `libld-musl.so` and the `#!/system/bin/sh` drivers `librustc-wrap.so` /
//! `libmk-ld.so` / `libmk-host-ld.so`. Everything under the files dir is only
//! ever mmap-exec'd (allowed), never execve'd (denied: W^X).
//!
//! Where the assets come from is the caller's ([`Assets`]): the WM process
//! reads them through the AssetManager; a hosted child (no JVM) reads the
//! APK file.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The pack the files dir holds whole: written last, after the bootstrap.
/// A match means nothing to do.
const STAMP: &str = ".wmdyn-stamp";
/// The pack whose archives are (being) unpacked. Another pack wipes the
/// tree first, so two packs never mix.
const PACK: &str = ".wmdyn-pack";
/// `.wmdyn-done-<dir>` holds the pack stamp once that archive is on disk,
/// so a launch killed mid-way (the OS, a crash, the person) resumes at the
/// next archive instead of redoing 600 MB.
const DONE_PREFIX: &str = ".wmdyn-done-";
/// A progress line every this many compressed bytes.
const PROGRESS_STEP: u64 = 4 << 20;
/// The generated crate whose dependencies are the host proc-macros the
/// phone rebuilds (tools/cargo_makepad/src/android/dyn_pack/stage.rs).
pub const BOOTSTRAP: &str = "wmdyn-bootstrap";
/// The phone's target triple.
pub const TARGET: &str = "aarch64-linux-android";
/// The proc-macro bootstrap of a first provisioning: a handful of host
/// crates; far longer means a wedged toolchain.
const BOOTSTRAP_DEADLINE: Duration = Duration::from_secs(600);

/// Where the packed assets are read from; names are relative to the APK's
/// `assets/` (`wmdyn/stamp`).
pub trait Assets {
    /// A small asset, whole.
    fn load(&mut self, name: &str) -> Option<Vec<u8>>;
    /// A large asset as a stream the caller owns: the archive parts are
    /// 32 MB each and are never held whole (the super-app beside a 400 MB
    /// GPU process was killed for less).
    fn open(&mut self, name: &str) -> Option<Box<dyn Read>>;
}

/// The files-dir layout and the environment of every on-device cargo.
pub struct Toolchain {
    /// The app's files dir: `src/`, `target/`, `tc/`, `cargo/`, `home/`, `tmp/`.
    pub data: PathBuf,
    /// nativeLibraryDir: the loader and the drivers.
    pub nlib: PathBuf,
    /// `env.txt`: the strings the Mac build hashed.
    env: Vec<(String, String)>,
}

impl Toolchain {
    /// `data` and `nlib` as given; the env from the APK's `wmdyn/env.txt`.
    pub fn new(data: PathBuf, nlib: PathBuf, assets: &mut dyn Assets) -> Result<Toolchain, String> {
        let env = text(assets, "wmdyn/env.txt")?
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        Ok(Toolchain { data, nlib, env })
    }

    /// The staged checkout (`MAKEPAD_WM_ROOT` of the super-app).
    pub fn src(&self) -> PathBuf {
        self.data.join("src")
    }

    /// The shared target dir every build uses.
    pub fn target(&self) -> PathBuf {
        self.data.join("target")
    }

    /// The release output dir of the phone's target triple.
    pub fn release_dir(&self) -> PathBuf {
        self.target().join(TARGET).join("release")
    }

    /// `cargo` = the musl loader running the toolchain's cargo, with the
    /// environment of [`Toolchain::env`].
    pub fn cargo(&self) -> Command {
        let mut cmd = Command::new(self.nlib.join("libld-musl.so"));
        cmd.arg(self.data.join("tc/bin/cargo"));
        self.env(&mut cmd);
        cmd
    }

    /// Everything lives under the files dir, with the exact strings the Mac
    /// cross-build used (cargo hashes them).
    pub fn env(&self, cmd: &mut Command) {
        cmd.env_remove("MAKEPAD");
        cmd.env("RUSTC", self.nlib.join("librustc-wrap.so"));
        cmd.env("MAKEPAD_DYN_DIR", &self.data);
        cmd.env("MAKEPAD_DYN_NLIB", &self.nlib);
        cmd.env("CARGO_HOME", self.data.join("cargo"));
        cmd.env("CARGO_TARGET_DIR", self.target());
        cmd.env("HOME", self.data.join("home"));
        cmd.env("TMPDIR", self.data.join("tmp"));
        cmd.env("PATH", "/system/bin");
        cmd.env("CARGO_TERM_COLOR", "never");
        cmd.env("CARGO_BUILD_JOBS", "4");
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
    }
}

fn text(assets: &mut dyn Assets, name: &str) -> Result<String, String> {
    assets
        .load(name)
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| format!("asset {name} missing: APK not packed for on-device builds"))
}

/// Whether the files dir already holds this APK's pack whole.
pub fn provisioned(tc: &Toolchain, assets: &mut dyn Assets) -> bool {
    let Ok(stamp) = text(assets, "wmdyn/stamp") else { return false };
    std::fs::read_to_string(tc.data.join(STAMP)).ok().as_deref() == Some(stamp.as_str())
}

/// Unpack the APK's archives into the files dir (resuming a killed run),
/// rebuild the proc-macros, write their recorded SVHs, align target/'s
/// mtimes; once per pack. `say` gets every progress line.
pub fn provision(tc: &Toolchain, assets: &mut dyn Assets, say: &mut dyn FnMut(String)) -> Result<(), String> {
    let stamp = text(assets, "wmdyn/stamp")?;
    if std::fs::read_to_string(tc.data.join(STAMP)).ok().as_deref() == Some(stamp.as_str()) {
        say(format!("provision skipped: stamp hit ({})", stamp.trim()));
        return Ok(());
    }
    match provision_pack(tc, assets, &stamp, say) {
        Ok(()) => Ok(()),
        Err(e) => {
            say(format!("provision failed: {e}"));
            Err(e)
        }
    }
}

fn provision_pack(tc: &Toolchain, assets: &mut dyn Assets, stamp: &str, say: &mut dyn FnMut(String)) -> Result<(), String> {
    let started = std::time::Instant::now();
    let data = &tc.data;
    if std::fs::read_to_string(data.join(PACK)).ok().as_deref() == Some(stamp) {
        say(format!("provisioning {}: resuming", stamp.trim()));
    } else {
        say(format!("provisioning {} into {}", stamp.trim(), data.display()));
        for dir in ["src", "target", "tc", "cargo", "home", "tmp"] {
            let _ = std::fs::remove_dir_all(data.join(dir));
        }
        let _ = std::fs::remove_file(data.join(STAMP));
        if let Ok(rd) = std::fs::read_dir(data) {
            for entry in rd.flatten() {
                if entry.file_name().to_string_lossy().starts_with(DONE_PREFIX) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        std::fs::write(data.join(PACK), stamp).map_err(|e| format!("pack stamp: {e}"))?;
    }
    for dir in ["cargo", "home", "tmp"] {
        std::fs::create_dir_all(data.join(dir)).map_err(|e| format!("mkdir {dir}: {e}"))?;
    }
    let manifest = text(assets, "wmdyn/manifest.txt")?;
    for line in manifest.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(parts), Some(bytes)) = (it.next(), it.next(), it.next()) else { continue };
        let parts: usize = parts.parse().map_err(|_| format!("manifest: {line}"))?;
        let bytes: u64 = bytes.parse().map_err(|_| format!("manifest: {line}"))?;
        // `src.tar.lz4` unpacks to `src/`: the archive's one top directory.
        let dir = name.split('.').next().unwrap_or(name);
        let done = data.join(format!("{DONE_PREFIX}{dir}"));
        if std::fs::read_to_string(&done).ok().as_deref() == Some(stamp) {
            say(format!("extracting {dir}: already on disk"));
            continue;
        }
        // A half-written tree from a launch that died mid-archive.
        let _ = std::fs::remove_dir_all(data.join(dir));
        extract(tc, assets, name, dir, parts, bytes, say)?;
        std::fs::write(&done, stamp).map_err(|e| format!("{}: {e}", done.display()))?;
    }
    bootstrap(tc, say)?;
    patch_proc_macro_svh(tc, assets, say)?;
    say("aligning target/ mtimes".to_string());
    bump_mtimes(&tc.target())?;
    std::fs::write(data.join(STAMP), stamp).map_err(|e| format!("stamp: {e}"))?;
    say(format!("provisioned in {:.0} s", started.elapsed().as_secs_f64()));
    Ok(())
}

/// The archive's asset parts, one stream, each part read as it streams;
/// counts the compressed bytes.
struct PartsReader<'a> {
    assets: &'a mut dyn Assets,
    name: String,
    parts: usize,
    next: usize,
    current: Option<Box<dyn Read>>,
    read: u64,
}

impl Read for PartsReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.current.is_none() {
                if self.next == self.parts {
                    return Ok(0);
                }
                let part = format!("wmdyn/{}.{:03}", self.name, self.next);
                let stream = self
                    .assets
                    .open(&part)
                    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("asset {part} missing")))?;
                self.current = Some(stream);
                self.next += 1;
            }
            let n = self.current.as_mut().unwrap().read(buf)?;
            if n == 0 {
                self.current = None;
                continue;
            }
            self.read += n as u64;
            return Ok(n);
        }
    }
}

/// Stream the archive's asset parts through the LZ4 frame decoder and the
/// tar reader straight into the files dir. Nothing is concatenated on flash
/// and nothing is held whole: the gzip path read a 292 MB
/// archive into a Vec, inflated 800 MB more next to it, three archives in a
/// row beside a 400 MB GPU process — the OS killed it before a stamp was
/// written, and every launch started over.
fn extract(
    tc: &Toolchain,
    assets: &mut dyn Assets,
    name: &str,
    dir: &str,
    parts: usize,
    total: u64,
    say: &mut dyn FnMut(String),
) -> Result<(), String> {
    let mb = |b: u64| b / 1_000_000;
    let started = std::time::Instant::now();
    say(format!("inflating {dir} 0/{} MB", mb(total)));
    let mut source = PartsReader { assets, name: name.to_string(), parts, next: 0, current: None, read: 0 };
    let read = std::cell::Cell::new(0u64);
    let mut last_said = 0u64;
    let report = {
        let counted = CountedRead { inner: &mut source, read: &read };
        let mut progress = |p: &makepad_tar::Progress| {
            let got = read.get();
            if got - last_said >= PROGRESS_STEP {
                last_said = got;
                say(format!("inflating {dir} {}/{} MB · {} files", mb(got), mb(total), p.entries));
            }
        };
        makepad_tar::unpack_stream(counted, &tc.data, &mut progress).map_err(|e| format!("unpack {name}: {e}"))?
    };
    if source.read != total {
        return Err(format!("{name}: {} bytes in the APK, manifest says {total}", source.read));
    }
    say(format!(
        "unpacked {dir}: {} files, {} links in {:.0} s",
        report.files,
        report.symlinks + report.hardlinks,
        started.elapsed().as_secs_f64()
    ));
    Ok(())
}

/// A reader that publishes its byte count to a shared cell (the progress
/// line's numerator) while makepad-tar owns it.
struct CountedRead<'a, R: Read> {
    inner: R,
    read: &'a std::cell::Cell<u64>,
}

impl<R: Read> Read for CountedRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

/// Run a cargo child in a process group of its own and hand each stderr
/// line to `say`; the first lines of its errors go into the returned error.
/// Past `deadline` the whole group (cargo, rustc, the linker) is killed and
/// the run fails: a caller then falls back to what it shipped rather than
/// waiting on a wedged build forever.
pub fn run_cargo(mut cmd: Command, what: &str, deadline: Duration, say: &mut dyn FnMut(String)) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cargo ({what}): {e}"))?;
    let stderr = child.stderr.take();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }
    });
    let started = Instant::now();
    let mut err = String::new();
    let take = |line: String, err: &mut String, say: &mut dyn FnMut(String)| {
        let text = line.trim().to_string();
        if text.is_empty() {
            return;
        }
        if err.len() < 1200 {
            err.push_str(&text);
            err.push('\n');
        }
        say(text);
    };
    let killed = || format!("{what}: no result after {} s; killed", deadline.as_secs());
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => take(line, &mut err, say),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // stderr closed: the child is exiting.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match wait_until(&mut child, started + deadline) {
                Some(status) => break status,
                None => {
                    kill_group(&mut child);
                    return Err(killed());
                }
            },
        }
        if started.elapsed() > deadline {
            kill_group(&mut child);
            let _ = reader.join();
            return Err(killed());
        }
    };
    let _ = reader.join();
    if !status.success() {
        return Err(format!("{what} failed ({status}): {err}"));
    }
    Ok(())
}

fn wait_until(child: &mut Child, until: Instant) -> Option<std::process::ExitStatus> {
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() > until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// SIGKILL the child's process group (it leads one: `run_cargo`), then reap it.
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(-(child.id() as i32), 9) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The host-kind units the Mac could not ship (Mach-O): proc-macros and
/// their host rlibs, rebuilt here by the musl rustc as dependencies of the
/// generated `wmdyn-bootstrap` crate (build-override profile, the resolved
/// features: the same unit hashes the shipped fingerprints name). The
/// shipped units stay Fresh once the mtimes are aligned.
fn bootstrap(tc: &Toolchain, say: &mut dyn FnMut(String)) -> Result<(), String> {
    let mut cmd = tc.cargo();
    cmd.current_dir(tc.src());
    cmd.args(["build", "--release", "--offline", "--frozen", "--target", TARGET, "-p", BOOTSTRAP]);
    say("bootstrapping proc-macros with the on-device rustc".to_string());
    run_cargo(cmd, "proc-macro bootstrap", BOOTSTRAP_DEADLINE, say)
}

/// The SVH law (makepad_rmeta): the shipped metadata names every proc-macro
/// by the SVH of the Mac's Mach-O build, and a proc-macro the musl rustc
/// rebuilt carries another one (host triple, host std are in the hash) —
/// rustc then fails the first app build with E0463 "can't find crate for
/// makepad_micro_serde_derive". So each rebuilt `.so` gets the recorded SVH
/// written into its header (`wmdyn/proc-macro-svh.txt`: `lib<crate>-<hash>.so
/// <svh>`), before the mtime bump. A listed file the bootstrap did not
/// produce means the unit hashes differ from the Mac's: the shipped units
/// would not be Fresh either — stop here, say which.
fn patch_proc_macro_svh(tc: &Toolchain, assets: &mut dyn Assets, say: &mut dyn FnMut(String)) -> Result<(), String> {
    let list = text(assets, "wmdyn/proc-macro-svh.txt")?;
    let deps = tc.target().join("release/deps");
    for line in list.lines() {
        let mut it = line.split_whitespace();
        let (Some(file), Some(hex)) = (it.next(), it.next()) else { continue };
        let want = makepad_rmeta::parse_svh(hex).ok_or_else(|| format!("proc-macro-svh.txt: bad svh {hex}"))?;
        let path = deps.join(file);
        let mut data = std::fs::read(&path)
            .map_err(|e| format!("bootstrap produced no {file} (unit hash differs from the Mac's?): {e}"))?;
        let h = makepad_rmeta::read_header(&data).map_err(|e| format!("{file}: {e}"))?;
        if h.svh == want {
            say(format!("{file}: svh already {hex}"));
            continue;
        }
        makepad_rmeta::set_svh(&mut data, &h, want);
        std::fs::write(&path, &data).map_err(|e| format!("write {file}: {e}"))?;
        say(format!("{file}: svh {} -> {hex} ({} {})", makepad_rmeta::svh_hex(&h.svh), h.name, h.triple));
    }
    Ok(())
}

/// Every file under `dir` gets the same mtime, newer than any source:
/// cargo's "dependency output newer than mine" staleness rule cannot fire
/// between the Mac-built units and the device-built proc-macros. One
/// whole-second instant: cargo compares with strict "newer than",
/// nanoseconds included.
pub fn bump_mtimes(dir: &Path) -> Result<(), String> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("read_dir {}: {e}", d.display()))?;
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                if let Ok(f) = std::fs::File::options().write(true).open(&p) {
                    let _ = f.set_modified(now);
                }
            }
        }
    }
    Ok(())
}
