//! Device-faithful rehearsal of the phone's first tile open, from the PACKED
//! APK: unpack src/target exactly as the phone does (the LZ4 frame parts
//! streamed through makepad-tar, which restores the ustar whole-second mtime,
//! no nanoseconds), bootstrap the proc-macros, check their recorded SVHs,
//! bump target/ mtimes, then `cargo rustc` each app with the device's command
//! and env ([`super::prove`]). Pass = cargo ok, widgets Fresh, 0 Dirty, no
//! build script run (on the phone a build-script exec is EACCES: W^X), one
//! Compiling, the engine .so untouched, the app .so produced and NEEDING the
//! engine. Everything lands in `<stage>/rehearsal`, wiped first.

use super::stage::{set_mtime, BOOTSTRAP};
use super::{prove, Dyn};
use makepad_zip_file::{zip_read_central_directory, CentralDirectoryFileHeader};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// The parts of one archive, in order, as one byte stream — the phone's
/// asset reader (apps/wm/src/dylib_host.rs `PartReader`).
struct PartReader<'a> {
    zip: &'a mut fs::File,
    headers: &'a BTreeMap<String, &'a CentralDirectoryFileHeader>,
    name: String,
    parts: usize,
    next: usize,
    buf: Vec<u8>,
    pos: usize,
    fed: u64,
}

impl Read for PartReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.buf.len() {
            if self.next == self.parts {
                return Ok(0);
            }
            let part = format!("assets/wmdyn/{}.{:03}", self.name, self.next);
            let header = self
                .headers
                .get(&part)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{part} missing from the apk")))?;
            self.buf = header
                .extract(self.zip)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{part}: {e:?}")))?;
            self.pos = 0;
            self.next += 1;
            self.fed += self.buf.len() as u64;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn asset_text(zip: &mut fs::File, headers: &BTreeMap<String, &CentralDirectoryFileHeader>, name: &str) -> Result<String, String> {
    let path = format!("assets/wmdyn/{name}");
    let header = headers.get(&path).ok_or_else(|| format!("{path} missing from the apk"))?;
    let bytes = header.extract(zip).map_err(|e| format!("{path}: {e:?}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path}: not utf-8"))
}

/// Rehearse `apk` for `apps`; `tag` prefixes the logs. Ok(true) when every
/// app passes.
pub fn rehearse(d: &Dyn, apk: &Path, apps: &[String], tag: &str) -> Result<bool, String> {
    let x = d.stage.join("rehearsal");
    if x.is_dir() {
        fs::remove_dir_all(&x).map_err(|e| format!("{}: {e}", x.display()))?;
    }
    fs::create_dir_all(&x).map_err(|e| format!("{}: {e}", x.display()))?;
    fs::create_dir_all(&d.logs).map_err(|e| format!("{}: {e}", d.logs.display()))?;
    println!("== unpack src + target from {} (device mtime semantics)", apk.display());
    let mut zip = fs::File::open(apk).map_err(|e| format!("{}: {e}", apk.display()))?;
    let dir = zip_read_central_directory(&mut zip).map_err(|e| format!("{}: {e:?}", apk.display()))?;
    let headers: BTreeMap<String, &CentralDirectoryFileHeader> =
        dir.file_headers.iter().map(|h| (h.file_name.clone(), h)).collect();
    println!("{}", asset_text(&mut zip, &headers, "stamp")?);
    let manifest = asset_text(&mut zip, &headers, "manifest.txt")?;
    let mut unpacked = 0usize;
    for line in manifest.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(parts), Some(size)) = (it.next(), it.next(), it.next()) else { continue };
        if name != "src.tar.lz4" && name != "target.tar.lz4" {
            continue;
        }
        let parts: usize = parts.parse().map_err(|_| format!("manifest.txt: bad part count {parts}"))?;
        let size: u64 = size.parse().map_err(|_| format!("manifest.txt: bad size {size}"))?;
        // The phone's stream: the parts in order through the frame decoder
        // into the tar reader, never the whole archive in memory.
        let mut reader = PartReader { zip: &mut zip, headers: &headers, name: name.to_string(), parts, next: 0, buf: Vec::new(), pos: 0, fed: 0 };
        let report = makepad_tar::unpack_stream(&mut reader, &x, &mut |_| {}).map_err(|e| format!("{name}: {e}"))?;
        if reader.fed != size {
            return Err(format!("{name}: {} bytes fed, manifest says {size}", reader.fed));
        }
        println!("{name}: {} bytes, {} files", reader.fed, report.files);
        unpacked += report.files;
    }
    println!("{unpacked} files unpacked with whole-second mtimes");
    // The device's cargo env, verbatim from the APK (apps/wm/src/dylib_host.rs
    // android::Dyn::env), on top of the controlled inheritance; the wrapper
    // remaps like the device's librustc-wrap.so.
    let target = x.join("target");
    let cargo_home = x.join("cargo");
    fs::create_dir_all(&cargo_home).map_err(|e| format!("{}: {e}", cargo_home.display()))?;
    let mut env: Vec<(String, String)> = asset_text(&mut zip, &headers, "env.txt")?
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    env.push(("CARGO_TARGET_DIR".to_string(), target.to_string_lossy().to_string()));
    env.push(("CARGO_HOME".to_string(), cargo_home.to_string_lossy().to_string()));
    env.push(("CARGO_TERM_COLOR".to_string(), "never".to_string()));
    env.extend(d.wrapper_env());
    let src = x.join("src");

    println!("== bootstrap: cargo build -p {BOOTSTRAP}");
    let log = d.logs.join(format!("{tag}-bootstrap.log"));
    let mut cmd = super::controlled_command("rustup", &env);
    cmd.args(["run", "stable", "cargo", "build", "--release", "--offline", "--frozen", "--target"])
        .arg(d.triple())
        .args(["-p", BOOTSTRAP, "-v"])
        .current_dir(&src);
    if !super::run_logged(&mut cmd, &log)? {
        let text = fs::read_to_string(&log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for l in lines.iter().skip(lines.len().saturating_sub(20)) {
            println!("{l}");
        }
        return Err(format!("BOOTSTRAP FAILED ({})", log.display()));
    }
    let text = fs::read_to_string(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for l in text.lines() {
        let t = l.trim_start();
        for key in ["Compiling", "Fresh", "Dirty"] {
            if t.starts_with(key) && t[key.len()..].starts_with(' ') {
                *counts.entry(key).or_default() += 1;
            }
        }
    }
    println!("{}", counts.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(" "));

    println!("== proc-macro SVH: the APK's proc-macro-svh.txt vs the rebuilt units (same host: equal; the phone rewrites its .so headers to these)");
    let ext = std::env::consts::DLL_EXTENSION;
    let mut svh_fail = false;
    for line in asset_text(&mut zip, &headers, "proc-macro-svh.txt")?.lines() {
        let Some((file, svh)) = line.split_once(' ') else { continue };
        let dylib = target.join("release/deps").join(format!("{}.{ext}", file.trim_end_matches(".so")));
        let have = fs::read(&dylib).ok().and_then(|data| makepad_rmeta::read_header(&data).ok()).map(|h| makepad_rmeta::svh_hex(&h.svh));
        if have.as_deref() == Some(svh) {
            println!("   {file} {svh} ok");
        } else {
            println!("   {file}: recorded {svh}, rebuilt {} — FAIL", have.unwrap_or_else(|| "MISSING".to_string()));
            svh_fail = true;
        }
    }
    if svh_fail {
        return Err("SVH ASSET WRONG".to_string());
    }

    println!("== bump target/ mtimes (device: bump_mtimes)");
    bump_mtimes(&target)?;
    let mut all_pass = true;
    for app in apps {
        if d.is_proc() {
            // The phone's first build of a hosted app (the engine Fresh, the
            // wrapper relinked), then the build after a person edited the
            // app's source there: the app and the wrapper compile, nothing else.
            println!("== {app}: cargo build -p makepad-hosted-{app} (the device command)");
            let log = d.logs.join(format!("{tag}-{app}.log"));
            all_pass &= super::proc_ondevice(d, &src, &target, app, &log, false, &env)?;
            let edited = touch_app_source(d, &src, app)?;
            println!("== {app}: after an edit of {}", edited.display());
            let log = d.logs.join(format!("{tag}-{app}-edited.log"));
            all_pass &= super::proc_ondevice(d, &src, &target, app, &log, true, &env)?;
            continue;
        }
        println!("== {app}: cargo rustc --crate-type dylib (the device command)");
        let log = d.logs.join(format!("{tag}-{app}.log"));
        all_pass &= prove::prove(d, &src, &target, app, &log, false, &env)?;
    }
    Ok(all_pass)
}

/// Proc rehearsal: change the hosted app's library source the way an edit
/// on the phone does (content and a newer whole-second mtime); returns the
/// file. The app package's `src/lib.rs`.
fn touch_app_source(d: &Dyn, src: &Path, bin: &str) -> Result<PathBuf, String> {
    let super::Kind::Proc { wrappers } = &d.kind else {
        return Err("touch_app_source on a dyn tree".into());
    };
    let w = wrappers.iter().find(|w| w.bin == bin).ok_or_else(|| format!("{bin}: not a packed app"))?;
    let manifest = src.join(w.dir()).join("Cargo.toml");
    let text = fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    // `<app> = { path = "../../apps/x" }`: the app's directory in the tree.
    let key = format!("{} = {{ path = \"", w.app);
    let at = text.find(&key).ok_or_else(|| format!("{}: no dependency on {}", manifest.display(), w.app))?;
    let rest = &text[at + key.len()..];
    let rel = &rest[..rest.find('"').ok_or("unterminated path")?];
    let lib = src.join(w.dir()).join(rel).join("src/lib.rs");
    let mut body = fs::read_to_string(&lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    body.push_str("\n// rehearsal: an edit made on the phone\n");
    fs::write(&lib, body).map_err(|e| format!("{}: {e}", lib.display()))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) + 2;
    set_mtime(&lib, now)?;
    Ok(lib)
}

/// One whole-second instant for every file under `dir`
/// (apps/wm/src/dylib_host.rs `bump_mtimes`): cargo compares dependency
/// output mtimes with strict "newer than", nanoseconds included.
fn bump_mtimes(dir: &Path) -> Result<(), String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", d.display()))?;
            let p = entry.path();
            let ft = entry.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                set_mtime(&p, now)?;
            }
        }
    }
    Ok(())
}
