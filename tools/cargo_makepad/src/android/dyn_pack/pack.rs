//! Pack the Android super-app APK: cargo-makepad's own APK of the host (its
//! cdylib + the engine .so + libstd) plus, as assets, the rustc toolchain, the
//! staged checkout and the cross-built target/ tree, plus the loader/driver
//! scripts as native libs. Runs after the stage, the cross-build and the
//! tile-build proofs; the result is `<out>.tmp.apk` beside the final APK,
//! which the command publishes by rename at its very end.
//!
//! Assets (`assets/wmdyn/`): `stamp`, `env.txt`, `proc-macros.txt`,
//! `proc-macro-svh.txt`, `manifest.txt`, and `<name>.tar.lz4.NNN` parts
//! (32 MB, stored) of src / target / tc: LZ4 frames the phone streams straight
//! into files/ (apps/wm/src/dylib_host.rs, makepad-tar + makepad-lz4; the gzip
//! archives it once inflated whole killed the process: OOM). jniLibs:
//! `libld-musl.so` (musl loader), `libbusybox.so`, `librustc-wrap.so`,
//! `libmk-ld.so`, `libmk-host-ld.so` — the three scripts run ON THE PHONE
//! under its shell (templates in `device/`).

use super::stage::read_pairs;
use super::{Dyn, Kind, RUST_HOST};
use crate::android::compile;
use makepad_lz4::FrameEncoder;
use makepad_tar::TarWriter;
use makepad_zip_file::{zip_read_central_directory, ZipMethod, ZipWriter, COMPRESS_METHOD_UNCOMPRESSED};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const PART: u64 = 32 * 1024 * 1024;
const RUSTC_WRAP: &str = include_str!("device/rustc-wrap.sh");
const MK_LD: &str = include_str!("device/mk-ld.sh");
const MK_HOST_LD: &str = include_str!("device/mk-host-ld.sh");

/// The archive parts on disk: `<name>.NNN`, 32 MB each.
struct PartSink {
    dir: PathBuf,
    name: String,
    file: Option<BufWriter<fs::File>>,
    parts: usize,
    in_part: u64,
    total: u64,
}

impl PartSink {
    fn new(dir: &Path, name: &str) -> Self {
        Self { dir: dir.to_path_buf(), name: name.to_string(), file: None, parts: 0, in_part: 0, total: 0 }
    }

    fn finish(mut self) -> io::Result<(usize, u64)> {
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        Ok((self.parts, self.total))
    }
}

impl Write for PartSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.file.is_none() || self.in_part == PART {
            if let Some(mut f) = self.file.take() {
                f.flush()?;
            }
            let path = self.dir.join(format!("{}.{:03}", self.name, self.parts));
            self.file = Some(BufWriter::with_capacity(1 << 20, fs::File::create(path)?));
            self.parts += 1;
            self.in_part = 0;
        }
        let n = buf.len().min((PART - self.in_part) as usize);
        self.file.as_mut().unwrap().write_all(&buf[..n])?;
        self.in_part += n as u64;
        self.total += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
        }
        Ok(())
    }
}

/// Pack `apk_in` (cargo-makepad's APK of the host) with the stage, the
/// target tree and the toolchain `tc` into `<out>.tmp.apk`, zipaligned and
/// debug-signed; returns that path. Scratch (`parts/`, the unaligned zip)
/// lives in the stage.
pub fn pack(d: &Dyn, apk_in: &Path, tc: &Path) -> Result<PathBuf, String> {
    for p in [apk_in, tc, &d.src, &d.target] {
        if !p.exists() {
            return Err(format!("missing {}", p.display()));
        }
    }
    let out_dir = d.out_apk.parent().ok_or("output apk has no directory")?;
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let parts_dir = d.stage.join("parts");
    if parts_dir.is_dir() {
        fs::remove_dir_all(&parts_dir).map_err(|e| format!("{}: {e}", parts_dir.display()))?;
    }
    fs::create_dir_all(&parts_dir).map_err(|e| format!("{}: {e}", parts_dir.display()))?;
    let env_text = d.env_text()?;
    let svh = proc_macro_svh(d)?;
    fs::write(d.stage.join("proc-macro-svh.txt"), &svh).map_err(|e| format!("proc-macro-svh.txt: {e}"))?;
    print!("proc-macro SVHs the engine records (the phone rewrites its rebuilt .so headers to these):\n{svh}");
    let head = git_head(&d.checkout);
    let stamp = format!("wmdyn {} head {head}", utc_now());
    let t0 = Instant::now();

    // 1. Archives (mtimes preserved: cargo compares them): a plain tar
    //    stream through an LZ4 frame of independent 4 MB blocks the phone
    //    decodes as it reads.
    let host_strip: Vec<String> = read_pairs(&d.stage.join("host-strip.txt"))?.into_iter().map(|(_, c)| c).collect();
    // Dyn: the tile apps' lib units stay behind (the phone compiles every
    // tile). Proc: every unit the Mac built ships — the apps' own too, Fresh
    // on the phone until their source changes — except the final hosted
    // libraries (`libapp_<bin>.so`): the APK carries those, and cargo relinks
    // one on its first device build.
    let (apps, outputs): (Vec<String>, Vec<String>) = match &d.kind {
        Kind::Dyn => (d.apps.iter().map(|a| a.replace('-', "_")).collect(), Vec::new()),
        Kind::Proc { wrappers } => (Vec::new(), wrappers.iter().map(|w| format!("lib{}.so", w.lib())).collect()),
    };
    let mut fp_lib_dirs = lib_fingerprint_dirs(&d.target.join("release"), &host_strip)?;
    fp_lib_dirs.extend(lib_fingerprint_dirs(&d.target.join(d.triple()).join("release"), &apps)?);
    let (_ndk_version, ndk_prebuilt_root) =
        compile::resolve_ndk_prebuilt_root(&d.sdk_dir, d.host_os, d.urls.ndk_version_full)?;
    let api = d.urls.sdk_version.to_string();
    let api_dir = ndk_prebuilt_root.join("sysroot/usr/lib").join(d.triple()).join(&api);
    let triple = d.triple().to_string();
    let mut tars: Vec<(&str, usize, u64)> = Vec::new();
    let sources: [(&str, &Path, Box<dyn Fn(&str) -> bool>); 3] = [
        ("src", &d.src, Box::new(|_| false)),
        ("target", &d.target, Box::new(|rel| target_skip(rel, &fp_lib_dirs, &host_strip, &apps, &outputs, &triple))),
        ("tc", tc, Box::new(tc_skip)),
    ];
    for (name, root, skip) in &sources {
        let archive = format!("{name}.tar.lz4");
        let mut tar = TarWriter::new(FrameEncoder::new(PartSink::new(&parts_dir, &archive)));
        let mut n = add_tree(&mut tar, root, name, skip)?;
        if *name == "tc" {
            for f in list_sorted(&api_dir)? {
                if f.is_dir() {
                    continue;
                }
                let file = name_of(&f)?;
                tar.add_path(&format!("tc/android/lib/{api}/{file}"), &f).map_err(|e| format!("{}: {e}", f.display()))?;
                n += 1;
            }
        }
        let encoder = tar.finish().map_err(|e| format!("{archive}: {e}"))?;
        let sink = encoder.finish().map_err(|e| format!("{archive}: lz4: {e}"))?;
        let (parts, size) = sink.finish().map_err(|e| format!("{archive}: {e}"))?;
        tars.push((name, parts, size));
        println!("{name}: {n} files -> {:.0} MB lz4 in {parts} parts ({:.0} s)", size as f64 / 1e6, t0.elapsed().as_secs_f64());
    }

    // 2. Small assets.
    let manifest: String = tars.iter().map(|(name, parts, size)| format!("{name}.tar.lz4 {parts} {size}\n")).collect();
    let proc_macros = fs::read(d.stage.join("proc-macros.txt")).map_err(|e| format!("proc-macros.txt: {e}"))?;
    let small: Vec<(&str, Vec<u8>)> = vec![
        ("stamp", stamp.clone().into_bytes()),
        ("env.txt", env_text.into_bytes()),
        ("proc-macros.txt", proc_macros),
        ("proc-macro-svh.txt", svh.clone().into_bytes()),
        ("manifest.txt", manifest.into_bytes()),
    ];
    let render = |template: &str| -> Vec<u8> {
        template.replace("@VV@", &d.rustc_vv).replace("@API@", &api).replace("@RUST_HOST@", RUST_HOST).into_bytes()
    };
    let jni: Vec<(&str, Vec<u8>)> = vec![
        ("libld-musl.so", read(&tc.join("ld.so"))?),
        ("libbusybox.so", read(&tc.join("bin/busybox"))?),
        ("librustc-wrap.so", render(RUSTC_WRAP)),
        ("libmk-ld.so", render(MK_LD)),
        ("libmk-host-ld.so", render(MK_HOST_LD)),
    ];

    // 3. Re-zip: cargo-makepad's entries minus its signature, plus ours.
    let stem = d.out_apk.file_stem().map(name_of_os).transpose()?.unwrap_or_else(|| "app".to_string());
    let unaligned = d.stage.join(format!("{stem}.unaligned.apk"));
    let tmp = out_dir.join(format!("{stem}.tmp.apk"));
    let mut zin = fs::File::open(apk_in).map_err(|e| format!("{}: {e}", apk_in.display()))?;
    let dir = zip_read_central_directory(&mut zin).map_err(|e| format!("{}: {e:?}", apk_in.display()))?;
    let out = fs::File::create(&unaligned).map_err(|e| format!("{}: {e}", unaligned.display()))?;
    let mut zout = ZipWriter::to(BufWriter::with_capacity(1 << 20, out));
    let abi = d.android_target.abi_identifier();
    for h in &dir.file_headers {
        if h.file_name.starts_with("META-INF/") {
            continue;
        }
        let data = h.extract(&mut zin).map_err(|e| format!("{}: {e:?}", h.file_name))?;
        let method = if h.compression_method == COMPRESS_METHOD_UNCOMPRESSED { ZipMethod::Store } else { ZipMethod::Deflate };
        zout.add(&h.file_name, &data, method).map_err(|e| format!("{}: {e}", h.file_name))?;
    }
    for (name, data) in &jni {
        zout.add(&format!("lib/{abi}/{name}"), data, ZipMethod::Deflate).map_err(|e| format!("{name}: {e}"))?;
    }
    for (name, data) in &small {
        zout.add(&format!("assets/wmdyn/{name}"), data, ZipMethod::Deflate).map_err(|e| format!("{name}: {e}"))?;
    }
    for part in list_sorted(&parts_dir)? {
        let name = name_of(&part)?;
        let data = read(&part)?;
        zout.add(&format!("assets/wmdyn/{name}"), &data, ZipMethod::Store).map_err(|e| format!("{name}: {e}"))?;
    }
    zout.finish().map_err(|e| format!("{}: {e}", unaligned.display()))?;
    fs::remove_dir_all(&parts_dir).map_err(|e| format!("{}: {e}", parts_dir.display()))?;
    let unaligned_len = fs::metadata(&unaligned).map_err(|e| format!("{}: {e}", unaligned.display()))?.len();
    if tmp.exists() {
        fs::remove_file(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    compile::zipalign_apk(&d.sdk_dir, &d.urls, &unaligned, &tmp)?;
    compile::sign_apk_debug(&d.sdk_dir, &d.urls, &tmp)?;
    // Aligning pads and signing appends: the finished file is never smaller
    // than what went in.
    let tmp_len = fs::metadata(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?.len();
    if tmp_len < unaligned_len {
        return Err(format!(
            "{}: {tmp_len} bytes after zipalign + apksigner, smaller than the {unaligned_len} byte zip that went in",
            tmp.display()
        ));
    }
    fs::remove_file(&unaligned).map_err(|e| format!("{}: {e}", unaligned.display()))?;
    println!("{}: {:.0} MB ({stamp}) in {:.0} s", tmp.display(), tmp_len as f64 / 1e6, t0.elapsed().as_secs_f64());
    Ok(tmp)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A file name as text; a name that is not UTF-8 is an error, never a
/// lossy stand-in (it would ship under another name or not at all).
fn name_of(path: &Path) -> Result<String, String> {
    let name = path.file_name().ok_or_else(|| format!("{}: no file name", path.display()))?;
    name_of_os(name).map_err(|_| format!("{}: file name is not UTF-8", path.display()))
}

fn name_of_os(name: &std::ffi::OsStr) -> Result<String, String> {
    name.to_str().map(str::to_string).ok_or_else(|| format!("{name:?}: not UTF-8"))
}

/// The entries of `dir`, sorted by name; a directory entry that cannot be
/// read is an error (an archive missing a file must never look complete).
fn list_sorted(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        out.push(entry.map_err(|e| format!("{}: {e}", dir.display()))?.path());
    }
    out.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(out)
}

/// Every file below `root` (sorted, files of a directory before its
/// subdirectories) as `<arc_root>/<rel>`, unless `skip(rel)`. A symlink to a
/// file is archived as a symlink; a symlink to a directory is not descended
/// and not archived.
fn add_tree<W: Write>(
    tar: &mut TarWriter<W>,
    root: &Path,
    arc_root: &str,
    skip: &dyn Fn(&str) -> bool,
) -> Result<usize, String> {
    fn walk<W: Write>(
        tar: &mut TarWriter<W>,
        dir: &Path,
        root: &Path,
        arc_root: &str,
        skip: &dyn Fn(&str) -> bool,
        n: &mut usize,
    ) -> Result<(), String> {
        let entries = list_sorted(dir)?;
        let mut subdirs = Vec::new();
        for p in entries {
            let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let is_dir = if meta.file_type().is_symlink() {
                // os.walk: a symlink to a directory is listed as a directory
                // (not followed, not archived); a dangling one is a file.
                fs::metadata(&p).map(|m| m.is_dir()).unwrap_or(false)
            } else {
                meta.is_dir()
            };
            if is_dir {
                if !meta.file_type().is_symlink() {
                    subdirs.push(p);
                }
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .map_err(|_| format!("{}: outside {}", p.display(), root.display()))?
                .to_str()
                .ok_or_else(|| format!("{}: path is not UTF-8", p.display()))?
                .replace('\\', "/");
            if skip(&rel) {
                continue;
            }
            tar.add_path(&format!("{arc_root}/{rel}"), &p).map_err(|e| format!("{}: {e}", p.display()))?;
            *n += 1;
        }
        for sub in subdirs {
            walk(tar, &sub, root, arc_root, skip, n)?;
        }
        Ok(())
    }
    let mut n = 0;
    walk(tar, root, root, arc_root, skip, &mut n)?;
    Ok(n)
}

/// The `.fingerprint/<pkg>-<hash>` dirs of the LIB units of `crates` under
/// `release` (a dir holding `lib-<crate>`); a package's build-script units
/// have their own dirs and are kept.
fn lib_fingerprint_dirs(release: &Path, crates: &[String]) -> Result<BTreeSet<String>, String> {
    let fp = release.join(".fingerprint");
    let mut out = BTreeSet::new();
    if !fp.is_dir() {
        return Ok(out);
    }
    for d in list_sorted(&fp)? {
        if !d.is_dir() {
            continue;
        }
        let names = list_sorted(&d)?.iter().map(|p| name_of(p)).collect::<Result<Vec<_>, _>>()?;
        if crates.iter().any(|c| names.iter().any(|n| *n == format!("lib-{c}"))) {
            out.insert(name_of(&d)?);
        }
    }
    Ok(out)
}

/// What of a cross-built target/ the phone does not get. Host-kind LIBRARY
/// units of the proc-macros and their deps (`host_strip`) are Mach-O: the
/// device rebuilds them with its musl rustc (same fingerprint hash: same
/// fields). An app's LIB unit goes (the phone compiles it); its build-script
/// units (`build/<app>-*/`, the `run-build-script-*` fingerprint, the host
/// binary under release/) stay, like every other build script's: the pack
/// proof built and ran them on the Mac, so on the phone they are Fresh and
/// their OUT_DIR output (apps/files: generated PNGs) ships as it is here.
/// `.rustc_info.json` is a per-machine cache; the env marker is ours.
fn target_skip(
    rel: &str,
    fp_lib_dirs: &BTreeSet<String>,
    host_strip: &[String],
    apps: &[String],
    outputs: &[String],
    triple: &str,
) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    if rel == ".rustc_info.json" || rel == ".makepad-dyn-env" || parts[0] == "makepad-android-apk" {
        return true;
    }
    if parts[0] == "release" {
        if parts.len() >= 2 && (parts[1] == "examples" || parts[1] == "incremental") {
            return true;
        }
        if parts.len() == 3
            && parts[1] == "deps"
            && host_strip.iter().any(|c| parts[2].starts_with(&format!("lib{c}-")) || parts[2].starts_with(&format!("{c}-")))
        {
            return true;
        }
        if parts.len() >= 3 && parts[1] == ".fingerprint" && fp_lib_dirs.contains(parts[2]) {
            return true;
        }
        return false;
    }
    if parts.len() >= 2 && parts[0] == triple && parts[1] == "release" {
        let rest = &parts[2..];
        if rest.is_empty() {
            return false;
        }
        if rest.len() == 1 && apps.iter().any(|a| rest[0].starts_with(&format!("lib{a}"))) {
            return true;
        }
        let file = rest[rest.len() - 1];
        if (rest.len() == 1 || (rest.len() == 2 && rest[0] == "deps")) && outputs.iter().any(|o| o == file) {
            return true;
        }
        if rest[0] == "deps"
            && rest.len() == 2
            && apps.iter().any(|a| rest[1].starts_with(&format!("lib{a}-")) || rest[1].starts_with(&format!("{a}-")))
        {
            return true;
        }
        if rest[0] == ".fingerprint" && rest.len() >= 2 && fp_lib_dirs.contains(rest[1]) {
            return true;
        }
    }
    false
}

/// What of the phone toolchain tree ships: rustc, cargo, rust-lld, the
/// libraries; not the docs, the other binaries, nor the API 34 objects
/// (the API the build targets comes from the NDK sysroot).
fn tc_skip(rel: &str) -> bool {
    if rel.starts_with("share/") || rel == "cargo-config.toml" || rel == "env" || rel == "ld.so" {
        return true;
    }
    if rel.starts_with("bin/") {
        return rel != "bin/rustc" && rel != "bin/cargo";
    }
    let lld_dir = format!("lib/rustlib/{RUST_HOST}/bin/");
    if rel.starts_with(&lld_dir) {
        return rel != format!("{lld_dir}rust-lld");
    }
    if rel.starts_with("android/lib/") && !rel.starts_with("android/lib/clang/") {
        return true;
    }
    false
}

/// `lib<crate>-<hash>.so <svh>` per proc-macro of the Android graph: the SVH
/// the engine's metadata records for it (the Mac's build), decoded from the
/// dylib header and required to occur in the engine .so's CrateDep table.
/// The phone rewrites its musl-built .so headers to these after the
/// bootstrap (apps/wm/src/dylib_host.rs `patch_proc_macro_svh`): a rebuilt
/// proc-macro can never carry the recorded SVH by itself (the SVH hashes the
/// host triple and std).
fn proc_macro_svh(d: &Dyn) -> Result<String, String> {
    // Where the recorded SVHs live: the engine dylib's CrateDep table (Dyn);
    // the target's rlibs (Proc: the engine is linked statically into each
    // app, so its crates' metadata is what the phone's rustc reads).
    let recorders: Vec<Vec<u8>> = match d.kind {
        Kind::Dyn => vec![read(&d.engine_dylib(&d.target))?],
        Kind::Proc { .. } => Vec::new(),
    };
    let rlibs = d.target.join(d.triple()).join("release/deps");
    let pairs = read_pairs(&d.stage.join("host-strip.txt"))?;
    let packages: Vec<String> = read_pairs(&d.stage.join("proc-macros.txt"))?.into_iter().map(|(p, _)| p).collect();
    let deps = d.target.join("release/deps");
    let ext = format!(".{}", std::env::consts::DLL_EXTENSION);
    let mut lines = Vec::new();
    for (pkg, crate_name) in &pairs {
        if !packages.contains(pkg) {
            continue;
        }
        let mut found = Vec::new();
        for p in list_sorted(&deps)? {
            let n = name_of(&p)?;
            if !(n.starts_with(&format!("lib{crate_name}-")) && n.ends_with(&ext)) {
                continue;
            }
            let h = makepad_rmeta::read_header(&read(&p)?).map_err(|e| format!("{n}: {e}"))?;
            if !h.proc_macro {
                continue;
            }
            let recorded = match d.kind {
                Kind::Dyn => recorders[0].windows(16).filter(|w| *w == h.svh).count() == 1,
                // One candidate is the one every rlib names; several (a
                // stale target) are told apart by the rlibs that record them.
                Kind::Proc { .. } => true,
            };
            if recorded {
                found.push((format!("lib{crate_name}{}.so {}", h.extra_filename, makepad_rmeta::svh_hex(&h.svh)), h.svh));
            }
        }
        if found.len() > 1 && d.is_proc() {
            let mut kept = Vec::new();
            for (line, svh) in found.drain(..) {
                if any_rlib_records(&rlibs, &svh)? {
                    kept.push((line, svh));
                }
            }
            found = kept;
        }
        let found: Vec<String> = found.into_iter().map(|(line, _)| line).collect();
        if found.len() != 1 {
            return Err(format!(
                "{pkg}: {} proc-macro dylibs whose SVH the engine records ({found:?}); stale {}/release/deps or a bad cross-build",
                found.len(),
                d.target.display()
            ));
        }
        lines.extend(found);
    }
    if lines.len() != packages.len() {
        return Err(format!("proc-macro-svh: {} of {} proc-macros resolved", lines.len(), packages.len()));
    }
    Ok(lines.iter().map(|l| format!("{l}\n")).collect())
}

/// Whether any rlib in `deps` carries `svh` (a crate that depends on the
/// proc-macro records its SVH in its metadata).
fn any_rlib_records(deps: &Path, svh: &[u8]) -> Result<bool, String> {
    for p in list_sorted(deps)? {
        if p.extension().map(|e| e == "rlib").unwrap_or(false) && read(&p)?.windows(svh.len()).any(|w| w == svh) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn git_head(cwd: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// `YYYY-MM-DDTHH:MM:SSZ`, now.
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = makepad_civil_time::to_ymd((secs / 86400) as i32);
    let rest = secs % 86400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}
