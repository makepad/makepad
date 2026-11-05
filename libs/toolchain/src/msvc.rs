use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use makepad_strict_json::{self as json, Value};

use crate::extract;
use crate::fetch;
use crate::jobs;
use crate::msi;
use crate::progress;

const CHANNEL: &str = "https://aka.ms/vs/17/release/channel";

/// Written once all of a component is unpacked: a stopped install leaves the
/// files it got to, and those alone say nothing.
const BUILD_TOOLS_READY: &str = ".build-tools-ready";
const SDK_READY: &str = ".sdk-ready";
pub(crate) const SDK_READY_RECORD: &str = "all payloads extracted\n";

/// The C++ compiler of the Build tools.
pub fn build_tools_ready(dest: &Path) -> bool {
    dest.join(BUILD_TOOLS_READY).is_file()
}
/// The Windows SDK's import and C runtime libraries.
pub fn sdk_ready(dest: &Path) -> bool {
    fs::read_to_string(dest.join(SDK_READY)).is_ok_and(|text| text == SDK_READY_RECORD)
}
pub fn ready(dest: &Path) -> bool {
    build_tools_ready(dest) && sdk_ready(dest)
}

/// A failed smoke check must not make the next attempt reuse its damaged components.
pub(crate) fn invalidate(dest: &Path) -> Result<(), String> {
    for marker in [BUILD_TOOLS_READY, SDK_READY] {
        crate::remove_marker(&dest.join(marker))?;
    }
    Ok(())
}

/// The Build tools and Windows SDK jobs that still have work: two
/// components installing side by side into one folder, sharing one
/// catalog download. `all` keeps a ready one (it only reports ready).
pub fn install_jobs<'a>(cache: &'a Path, dest: &'a Path, catalog: &'a Catalog, all: bool) -> Vec<jobs::Job<'a>> {
    let mut out = Vec::new();
    if all || !build_tools_ready(dest) {
        out.push(jobs::Job::new("Build tools", move || install_build_tools(cache, dest, catalog)));
    }
    if all || !sdk_ready(dest) {
        out.push(jobs::Job::new("Windows SDK", move || install_sdk(cache, dest, catalog)));
    }
    out
}

/// Visual Studio's package catalog, downloaded once for both jobs; the
/// second waits for the first's download. Read only after that.
#[derive(Default)]
pub struct Catalog(OnceLock<Result<Loaded, String>>);
pub struct Loaded {
    vsman: Value,
    msvc_ver: String,
    sdk_pid: String,
}
impl Catalog {
    fn get(&self) -> Result<&Loaded, String> {
        self.0.get_or_init(load_catalog).as_ref().map_err(Clone::clone)
    }
}
fn load_catalog() -> Result<Loaded, String> {
    let _timing = crate::timing::component("Catalog");
    crate::setup_note!("msvc: vs 17 release channel");
    let channel_bytes = fetch::bytes(CHANNEL)?;
    let channel = json::parse(&channel_bytes).map_err(|e| format!("channel json: {e}"))?;
    let (vsman_url, vsman_sha) = channel_item_payload(&channel, "Microsoft.VisualStudio.Manifests.VisualStudio")?;
    crate::setup_note!("msvc: catalog {vsman_url}");
    let vsman_bytes = fetch::bytes(&vsman_url)?;
    fetch::check_sha256("the Visual Studio catalog", &vsman_bytes, &vsman_sha)?;
    let vsman = json::parse(&vsman_bytes).map_err(|e| format!("vsman json: {e}"))?;
    let by_id = packages_by_id(&vsman)?;
    let msvc_pid = latest_msvc(&by_id).ok_or("no MSVC tools package")?;
    let msvc_ver = msvc_pid
        .strip_prefix("microsoft.vc.")
        .and_then(|s| s.strip_suffix(".tools.hostx64.targetx64.base"))
        .ok_or("msvc pid parse")?
        .to_string();
    let sdk_pid = latest_sdk(&by_id).ok_or("no Windows SDK component")?;
    crate::setup_note!("msvc: VC {msvc_ver}");
    crate::setup_note!("msvc: SDK {sdk_pid}");
    drop(by_id);
    Ok(Loaded { vsman, msvc_ver, sdk_pid })
}
fn packages_by_id(vsman: &Value) -> Result<HashMap<String, Vec<&Value>>, String> {
    let packages = vsman.get("packages").and_then(Value::as_arr).ok_or("vsman packages")?;
    let mut by_id: HashMap<String, Vec<&Value>> = HashMap::new();
    for p in packages {
        if let Some(id) = p.get("id").and_then(Value::as_str) {
            by_id.entry(id.to_ascii_lowercase()).or_default().push(p);
        }
    }
    Ok(by_id)
}
/// Which payload a sequential install would have written later wins when
/// two carry the same file: Build tools first, then the SDK's MSIs in order.
fn order(component: u64, index: usize) -> u64 {
    (component << 32) | index as u64
}

fn install_build_tools(cache: &Path, dest: &Path, catalog: &Catalog) -> Result<(), String> {
    progress::package("Build tools", "Read package catalog", 0, 0);
    if build_tools_ready(dest) {
        progress::stage("Ready", "Microsoft Build tools already installed", 1.0);
        return Ok(());
    }
    let catalog = catalog.get()?;
    let by_id = packages_by_id(&catalog.vsman)?;
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let ver = &catalog.msvc_ver;
    let host = "x64";
    let target = "x64";
    let mut ids = vec![
        "microsoft.visualcpp.dia.sdk".to_string(),
        format!("microsoft.vc.{ver}.crt.headers.base"),
        format!("microsoft.vc.{ver}.crt.source.base"),
        format!("microsoft.vc.{ver}.tools.host{host}.target{target}.base"),
        format!("microsoft.vc.{ver}.tools.host{host}.target{target}.res.base"),
        format!("microsoft.vc.{ver}.crt.{target}.desktop.base"),
        format!("microsoft.vc.{ver}.crt.{target}.store.base"),
    ];
    if let Some(redist) = redist_pkg(&by_id, ver, target) {
        ids.push(redist);
    }
    let mut packages = Vec::new();
    for id in &ids {
        match pick_pkg(&by_id, id) {
            Some(pkg) => packages.push(pkg),
            None => crate::setup_note!("  msvc skip missing {id}"),
        }
    }
    let payloads: Vec<(&str, &Value)> = packages
        .iter()
        .flat_map(|pkg| {
            let id = pkg.get("id").and_then(Value::as_str).unwrap_or("a package with no id");
            pkg.get("payloads").and_then(Value::as_arr).into_iter().flatten().map(move |p| (id, p))
        })
        .filter(|(_, p)| payload_file(p).is_some_and(|file| file.ends_with(".vsix") || file.ends_with(".zip") || file.ends_with(".msi")))
        .collect();
    // One bar for the whole component: every payload's size is in the
    // manifest, so the work is known before the first download.
    let _total = progress::total("Build tools", payloads.iter().map(|(_, p)| payload_size(p)).sum());
    let mut pending = Vec::new();
    for (index, (package, p)) in payloads.iter().enumerate() {
        let (url, sha, file) = payload_url(package, p)?;
        let size = payload_size(p);
        let unzip = file.ends_with(".vsix") || file.ends_with(".zip");
        let dest = dest.to_path_buf();
        pending.push(fetch::file_then(cache, &url, &file, &sha, size, move |bytes| {
            jobs::check_cancelled()?;
            if unzip {
                extract::unzip_parallel(bytes, &dest, extract::Strip::Prefix("Contents".into()), order(0, index), size)?;
            } else {
                progress::advance(size);
            }
            Ok(())
        }));
    }
    jobs::wait_all(pending)?;
    write_nvcc_bats(dest)?;
    if find_cl(dest).is_none() {
        return Err("cl.exe missing after msvc extract".into());
    }
    fs::write(dest.join(BUILD_TOOLS_READY), "").map_err(|e| e.to_string())?;
    progress::stage("Ready", "Microsoft Build tools installed", 1.0);
    Ok(())
}

fn redist_pkg(by_id: &HashMap<String, Vec<&Value>>, ver: &str, target: &str) -> Option<String> {
    let direct = format!("microsoft.vc.{ver}.crt.redist.{target}.base");
    if by_id.contains_key(&direct) {
        return Some(direct);
    }
    let parent = format!("microsoft.visualcpp.crt.redist.{target}");
    let pkg = pick_pkg(by_id, &parent)?;
    let deps = pkg.get("dependencies")?;
    match deps {
        Value::Arr(arr) => arr
            .iter()
            .filter_map(Value::as_str)
            .find(|s| s.to_ascii_lowercase().ends_with(".base"))
            .map(|s| s.to_ascii_lowercase()),
        Value::Obj(pairs) => pairs
            .iter()
            .map(|(k, _)| k.to_ascii_lowercase())
            .find(|k| k.ends_with(".base")),
        _ => None,
    }
}

/// Files wanted from the cabinets: member name → where each MSI puts it.
type Wanted = HashMap<String, Vec<(u64, PathBuf)>>;

fn install_sdk(cache: &Path, dest: &Path, catalog: &Catalog) -> Result<(), String> {
    progress::package("Windows SDK", "Read package list", 0, 0);
    if sdk_ready(dest) {
        progress::stage("Ready", "Windows SDK already installed", 1.0);
        return Ok(());
    }
    let catalog = catalog.get()?;
    let by_id = packages_by_id(&catalog.vsman)?;
    let sdk_comp = pick_pkg(&by_id, &catalog.sdk_pid).ok_or("sdk component")?;
    let dep = first_dep_id(sdk_comp).ok_or("sdk dependency")?;
    let sdk_pkg = pick_pkg(&by_id, &dep).ok_or_else(|| format!("sdk pkg {dep}"))?;
    let payloads = sdk_pkg
        .get("payloads")
        .and_then(Value::as_arr)
        .ok_or("sdk payloads")?;
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    let msi_names = [
        "Windows SDK Desktop Headers x64-x86_en-us.msi",
        "Windows SDK Desktop Headers x86-x86_en-us.msi",
        // winapifamily.h and the rest of `shared`, since SDK 26100
        "Windows SDK OnecoreUap Headers x64-x86_en-us.msi",
        "Windows SDK OnecoreUap Headers x86-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Headers OnecoreUap-x86_en-us.msi",
        "Windows SDK Desktop Libs x64-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Headers-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Libs-x86_en-us.msi",
        "Universal CRT Headers Libraries and Sources-x86_en-us.msi",
    ];
    // The MSIs are small and name the CABs they need; read them first so the
    // component's bar knows its whole work before the large downloads.
    let mut pending = Vec::new();
    let mut msi_bytes = 0;
    for name in msi_names {
        let Some(payload) = payload_named(payloads, name) else {
            crate::setup_note!("  sdk skip {name}");
            continue;
        };
        let (url, sha, file) = payload_url(&dep, payload)?;
        msi_bytes += payload_size(payload);
        pending.push(fetch::listing_then(cache, &url, &file, &sha, payload_size(payload), move |bytes| {
            let files = msi::msi_files(&bytes).map_err(|e| format!("{name}: {e}"))?;
            // The Media table names exactly the cabinets this MSI needs
            // (25 of the SDK's 149, 179 of 462 MB); the byte scan found none.
            let cabs = match msi::msi_cabinets(&bytes)? {
                cabs if !cabs.is_empty() => cabs,
                _ => scan_cab_names(&bytes),
            };
            Ok((name, files, cabs))
        }));
    }
    let msis = jobs::wait_all(pending)?;
    let mut cabs: Vec<&Value> = Vec::new();
    for (_, _, names) in &msis {
        // A cabinet the catalog lacks matters only if a file is in it: the check below counts them.
        for cab in names.iter().filter_map(|cab| payload_named(payloads, cab)) {
            if !cabs.iter().any(|c| std::ptr::eq(*c, cab)) {
                cabs.push(cab);
            }
        }
    }
    if cabs.is_empty() {
        crate::setup_note!("  no .cab names scanned from msi; downloading sdk cab payloads");
        cabs = payloads
            .iter()
            .filter(|p| payload_file(p).is_some_and(|file| {
                let lower = file.to_ascii_lowercase();
                lower.ends_with(".cab") && !lower.contains("arm")
            }))
            .collect();
    }
    let mut wanted = Wanted::new();
    let msi_count = msis.len();
    for (index, (_, files, _)) in msis.into_iter().enumerate() {
        for (id, rel) in files {
            wanted.entry(id).or_default().push((order(1, index), rel));
        }
    }
    let wanted = Arc::new(wanted);
    let cab_bytes: u64 = cabs.iter().map(|c| payload_size(c)).sum();
    let _total = progress::total("Windows SDK", msi_bytes + cab_bytes);
    // The MSIs are already downloaded; reading them counts as their unpacking.
    progress::advance(2 * msi_bytes);
    let mut pending = Vec::new();
    for cab in &cabs {
        let (url, sha, file) = payload_url(&dep, cab)?;
        let size = payload_size(cab);
        let (dest, wanted, name) = (dest.to_path_buf(), wanted.clone(), file.clone());
        pending.push(fetch::file_then(cache, &url, &file, &sha, size, move |bytes| {
            jobs::check_cancelled()?;
            unpack_cab(bytes, &dest, &wanted, &name, size)
        }));
    }
    // Distinct paths: a file that two MSIs install counts once.
    let written: std::collections::HashSet<String> = jobs::wait_all(pending)?.into_iter().flatten().collect();
    let wanted_paths: std::collections::HashSet<String> =
        wanted.values().flatten().map(|(_, rel)| rel.to_string_lossy().to_lowercase()).collect();
    crate::setup_note!("sdk: {} of the {} files its {msi_count} MSIs install, from {} cabinets", written.len(), wanted_paths.len(), cabs.len());
    if !wanted_paths.is_subset(&written) {
        return Err(format!("SDK extraction incomplete: {} files missing", wanted_paths.difference(&written).count()));
    }
    jobs::check_cancelled()?;
    if walk_find(dest, "kernel32.lib").is_none() {
        return Err("kernel32.lib missing after sdk extract".into());
    }
    if walk_find(dest, "libucrt.lib").is_none() {
        return Err("libucrt.lib missing after sdk extract".into());
    }
    if walk_find(dest, "winapifamily.h").is_none() {
        return Err("winapifamily.h missing after sdk extract".into());
    }
    fs::write(dest.join(SDK_READY), SDK_READY_RECORD).map_err(|e| e.to_string())?;
    progress::stage("Ready", "Windows SDK installed", 1.0);
    Ok(())
}

/// One cabinet: each folder (an independent compressed stream) that holds
/// wanted files decompresses as its own task, and its files are written by
/// further tasks. `weight` units of the bar: half for decompressing, half
/// for writing, shared by the folders' block counts.
/// Returns the paths written (relative, lower case).
fn unpack_cab(bytes: Arc<Vec<u8>>, dest: &Path, wanted: &Arc<Wanted>, name: &str, weight: u64) -> Result<Vec<String>, String> {
    let parsed = crate::cab::parse(&bytes).map_err(|e| format!("cab list {name}: {e}"))?;
    let mut per_folder: Vec<Vec<crate::cab::CabFile>> = vec![Vec::new(); parsed.folders.len()];
    for f in parsed.files {
        if wanted.contains_key(&f.name.to_ascii_lowercase()) {
            per_folder.get_mut(f.folder).ok_or_else(|| format!("cab folder {} missing", f.folder))?.push(f);
        }
    }
    let blocks: usize = parsed.folders.iter().zip(&per_folder).filter(|(_, files)| !files.is_empty()).map(|(f, _)| f.blocks().max(1)).sum();
    if blocks == 0 {
        progress::advance(weight);
        return Ok(Vec::new());
    }
    let mut pending = Vec::new();
    let mut given = 0;
    let used: Vec<_> = parsed.folders.into_iter().zip(per_folder).filter(|(_, files)| !files.is_empty()).collect();
    let last = used.len() - 1;
    for (i, (folder, files)) in used.into_iter().enumerate() {
        let share = if i == last { weight - given } else { (weight as u128 * folder.blocks().max(1) as u128 / blocks as u128) as u64 };
        given += share;
        let (bytes, dest, wanted, name) = (bytes.clone(), dest.to_path_buf(), wanted.clone(), name.to_string());
        pending.push(jobs::spawn(move || {
            let raw = {
                let _unpacking = progress::activity(progress::Activity::Unpack);
                progress::step(share / 2);
                Arc::new(crate::cab::decompress_folder(&bytes, &folder).map_err(|e| format!("cab extract {name}: {e}"))?)
            };
            progress::step_done();
            drop(bytes);
            write_cab_files(raw, files, &dest, &wanted, &name, share - share / 2)
        }));
    }
    Ok(jobs::wait_all(pending)?.into_iter().flatten().collect())
}
fn write_cab_files(raw: Arc<Vec<u8>>, files: Vec<crate::cab::CabFile>, dest: &Path, wanted: &Arc<Wanted>, name: &str, weight: u64) -> Result<Vec<String>, String> {
    let chunks = extract::chunk_by(files, |f| f.size + 4096);
    let total: usize = chunks.iter().map(|c| c.1).sum();
    let last = chunks.len().saturating_sub(1);
    let mut given = 0;
    let mut pending = Vec::new();
    for (i, (chunk, cost)) in chunks.into_iter().enumerate() {
        let share = if i == last { weight - given } else { (weight as u128 * cost as u128 / total.max(1) as u128) as u64 };
        given += share;
        let (raw, dest, wanted, name) = (raw.clone(), dest.to_path_buf(), wanted.clone(), name.to_string());
        pending.push(jobs::spawn(move || {
            let _writing = progress::activity(progress::Activity::Write);
            progress::step(share);
            let mut written = Vec::new();
            for f in &chunk {
                let end = f.offset.checked_add(f.size).ok_or("cab file overflow")?;
                let Some(data) = raw.get(f.offset..end) else {
                    return Err(format!("cab {name}: {} out of range ({}+{} > {})", f.name, f.offset, f.size, raw.len()));
                };
                for (order, rel) in wanted.get(&f.name.to_ascii_lowercase()).into_iter().flatten() {
                    if extract::write_ordered(&dest.join(rel), data, *order)? {
                        written.push(rel.to_string_lossy().to_lowercase());
                    }
                }
            }
            progress::step_done();
            Ok(written)
        }));
    }
    Ok(jobs::wait_all(pending)?.into_iter().flatten().collect())
}

pub fn find_cl(root: &Path) -> Option<std::path::PathBuf> {
    walk_find(root, "cl.exe")
}

pub fn msvc_bin_dir(root: &Path) -> Option<std::path::PathBuf> {
    find_cl(root).and_then(|p| p.parent().map(|p| p.to_path_buf()))
}

pub fn msvc_root_tools(root: &Path) -> Option<std::path::PathBuf> {
    // .../VC/Tools/MSVC/<ver>
    let bin = msvc_bin_dir(root)?;
    bin.parent()?.parent()?.parent().map(|p| p.to_path_buf())
}

fn walk_find(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    fn rec(dir: &Path, name: &str, depth: usize) -> Option<std::path::PathBuf> {
        if depth > 12 {
            return None;
        }
        let rd = fs::read_dir(dir).ok()?;
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && e.file_name().eq_ignore_ascii_case(name) {
                return Some(p);
            }
            if p.is_dir() {
                dirs.push(p);
            }
        }
        for d in dirs {
            if let Some(p) = rec(&d, name, depth + 1) {
                return Some(p);
            }
        }
        None
    }
    rec(root, name, 0)
}

fn scan_cab_names(msi: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = find_bytes(msi, b".cab", i) {
        let start = pos.saturating_sub(40);
        if let Some(name) = ascii_token(&msi[start..pos + 4]) {
            if name.to_ascii_lowercase().ends_with(".cab") && !out.contains(&name) {
                out.push(name);
            }
        }
        i = pos + 4;
    }
    // CFB string pools are UTF-16.
    let needle = b".\x00c\x00a\x00b\x00";
    i = 0;
    while let Some(pos) = find_bytes(msi, needle, i) {
        let start = pos.saturating_sub(80);
        if let Some(name) = utf16_token(&msi[start..pos + 8]) {
            if name.to_ascii_lowercase().ends_with(".cab") && !out.contains(&name) {
                out.push(name);
            }
        }
        i = pos + 8;
    }
    out
}

fn utf16_token(slice: &[u8]) -> Option<String> {
    let units: Vec<u16> = slice
        .chunks(2)
        .filter_map(|c| {
            if c.len() == 2 {
                Some(u16::from_le_bytes([c[0], c[1]]))
            } else {
                None
            }
        })
        .collect();
    let s = String::from_utf16_lossy(&units);
    let s = s
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    if s.to_ascii_lowercase().ends_with(".cab") && s.len() >= 8 {
        Some(s)
    } else {
        None
    }
}

fn ascii_token(slice: &[u8]) -> Option<String> {
    let mut end = slice.len();
    let mut start = 0;
    for (i, &b) in slice.iter().enumerate().rev() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' {
            end = i + 1;
            break;
        }
    }
    for (i, &b) in slice.iter().enumerate() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' {
            start = i;
            break;
        }
    }
    if start >= end {
        return None;
    }
    let s = std::str::from_utf8(&slice[start..end]).ok()?;
    if s.len() >= 8 {
        Some(s.to_string())
    } else {
        None
    }
}

fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// The payload's byte size from the manifest (0 when absent).
fn payload_size(p: &Value) -> u64 {
    p.get("size").and_then(Value::as_u64).unwrap_or(0)
}

/// The name a payload is saved under: the end of its `fileName`, else of its URL.
fn payload_file(p: &Value) -> Option<String> {
    let url = p.get("url").and_then(Value::as_str)?;
    Some(match p.get("fileName").and_then(Value::as_str) {
        Some(name) => name.rsplit(['/', '\\']).next().unwrap_or(name).to_string(),
        None => url.rsplit('/').next().unwrap_or("payload").to_string(),
    })
}

/// A payload's URL, SHA-256 and file name. One with no hash is refused: nothing unverified is
/// unpacked or run.
fn payload_url(package: &str, p: &Value) -> Result<(String, String, String), String> {
    let url = p.get("url").and_then(Value::as_str).ok_or_else(|| format!("{package}: a payload has no url"))?;
    let file = payload_file(p).ok_or_else(|| format!("{package}: a payload has no file name"))?;
    let sha = p.get("sha256").and_then(Value::as_str).ok_or_else(|| format!("{package}: {file} has no sha256"))?;
    Ok((url.to_string(), sha.to_string(), file))
}

fn payload_named<'a>(payloads: &'a [Value], name: &str) -> Option<&'a Value> {
    let want = name.replace('\\', "/").to_ascii_lowercase();
    payloads.iter().find(|p| {
        p.get("fileName")
            .and_then(Value::as_str)
            .map(|s| {
                s.replace('\\', "/")
                    .to_ascii_lowercase()
                    .ends_with(&want)
            })
            .unwrap_or(false)
    })
}

fn pick_pkg<'a>(by_id: &'a HashMap<String, Vec<&'a Value>>, id: &str) -> Option<&'a Value> {
    let list = by_id.get(&id.to_ascii_lowercase())?;
    list.iter()
        .copied()
        .find(|p| {
            p.get("language")
                .and_then(Value::as_str)
                .map(|l| l.eq_ignore_ascii_case("en-us"))
                .unwrap_or(true)
        })
        .or_else(|| list.first().copied())
}

fn first_dep_id(pkg: &Value) -> Option<String> {
    match pkg.get("dependencies")? {
        Value::Arr(arr) => arr.first().and_then(Value::as_str).map(|s| s.to_ascii_lowercase()),
        Value::Obj(pairs) => pairs.first().map(|(k, _)| k.to_ascii_lowercase()),
        _ => None,
    }
}

fn latest_msvc(by_id: &HashMap<String, Vec<&Value>>) -> Option<String> {
    let mut best: Option<(String, String)> = None;
    for id in by_id.keys() {
        if id.starts_with("microsoft.vc.")
            && id.ends_with(".tools.hostx64.targetx64.base")
            && !id.contains("premium")
        {
            let ver = id
                .strip_prefix("microsoft.vc.")?
                .strip_suffix(".tools.hostx64.targetx64.base")?;
            if ver
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                if best.as_ref().map(|(v, _)| v.as_str()) < Some(ver) {
                    best = Some((ver.to_string(), id.clone()));
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

fn latest_sdk(by_id: &HashMap<String, Vec<&Value>>) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for id in by_id.keys() {
        if id.starts_with("microsoft.visualstudio.component.windows10sdk.")
            || id.starts_with("microsoft.visualstudio.component.windows11sdk.")
        {
            if let Some(num) = id.rsplit('.').next().and_then(|s| s.parse::<u32>().ok()) {
                if best.as_ref().map(|(n, _)| *n) < Some(num) {
                    best = Some((num, id.clone()));
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

/// The URL and SHA-256 of a channel item's first payload.
fn channel_item_payload(channel: &Value, id: &str) -> Result<(String, String), String> {
    let items = channel
        .get("channelItems")
        .and_then(Value::as_arr)
        .ok_or("channelItems")?;
    for it in items {
        if it.get("id").and_then(Value::as_str) == Some(id) {
            let payloads = it.get("payloads").and_then(Value::as_arr).ok_or("payloads")?;
            let payload = payloads.first().ok_or("payload url")?;
            let url = payload.get("url").and_then(Value::as_str).ok_or("payload url")?;
            let sha = payload.get("sha256").and_then(Value::as_str).ok_or_else(|| format!("channel item {id} has no sha256"))?;
            return Ok((url.to_string(), sha.to_string()));
        }
    }
    Err(format!("channel item {id} missing"))
}

fn write_nvcc_bats(dest: &Path) -> Result<(), String> {
    let build = dest.join("VC/Auxiliary/Build");
    fs::create_dir_all(&build).map_err(|e| e.to_string())?;
    extract::write_file(
        &build.join("vcvarsall.bat"),
        b"rem placeholder for nvcc -ccbin discovery\r\n",
    )?;
    extract::write_file(&build.join("vcvars64.bat"), b"rem\r\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_complete_sdk_is_ready_and_a_failed_check_clears_both_parts() {
        let dest = std::env::temp_dir().join(format!("makepad-toolchain-ready-{}", std::process::id()));
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join(BUILD_TOOLS_READY), "").unwrap();
        fs::write(dest.join(SDK_READY), "").unwrap();
        assert!(!sdk_ready(&dest), "a marker without the record of all payloads counted as ready");
        fs::write(dest.join(SDK_READY), SDK_READY_RECORD).unwrap();
        assert!(ready(&dest));
        invalidate(&dest).unwrap();
        assert!(!build_tools_ready(&dest) && !sdk_ready(&dest), "a retry would reuse what failed the check");
        invalidate(&dest).unwrap();
        fs::remove_dir_all(&dest).unwrap();
    }

    #[test]
    fn anything_without_a_hash_is_refused_and_a_wrong_hash_fails() {
        let bare = json::parse(br#"{"url":"https://example.invalid/a.vsix","fileName":"a.vsix"}"#).unwrap();
        let error = payload_url("the.package", &bare).unwrap_err();
        assert!(error.contains("the.package") && error.contains("sha256"), "{error}");
        // The install's filters look at the name, so such a payload reaches this refusal and is not skipped.
        assert_eq!(payload_file(&bare).as_deref(), Some("a.vsix"));
        let hashed = json::parse(br#"{"url":"https://example.invalid/a.vsix","fileName":"dir\\a.vsix","sha256":"ab"}"#).unwrap();
        assert_eq!(payload_url("the.package", &hashed).unwrap(), ("https://example.invalid/a.vsix".to_string(), "ab".to_string(), "a.vsix".to_string()));

        let channel = json::parse(br#"{"channelItems":[{"id":"m","payloads":[{"url":"https://example.invalid/m"}]}]}"#).unwrap();
        assert!(channel_item_payload(&channel, "m").unwrap_err().contains("sha256"));
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(fetch::check_sha256("m", b"abc", abc).is_ok());
        assert!(fetch::check_sha256("m", b"abd", abc).unwrap_err().contains("mismatch"));
    }

    #[test]
    fn malformed_sdk_payloads_fail() {
        let wanted = Arc::new(Wanted::new());
        assert!(unpack_cab(Arc::new(vec![0; 16]), Path::new("unused"), &wanted, "broken.cab", 0).is_err(), "malformed cabinet reported success");
        let files = vec![crate::cab::CabFile { name: "header".into(), folder: 0, offset: 1, size: 2 }];
        assert!(write_cab_files(Arc::new(vec![0]), files, Path::new("unused"), &wanted, "truncated.cab", 0).is_err(), "missing file bytes reported success");
    }
}
