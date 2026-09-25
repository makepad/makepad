use std::collections::HashMap;
use std::fs;
use std::path::Path;

use makepad_strict_json::{self as json, Value};

use crate::extract;
use crate::http;
use crate::msi;

const CHANNEL: &str = "https://aka.ms/vs/17/release/channel";

/// The same VSIX/MSI/CAB extractor used by Windows setup also runs on Linux.
/// No Windows executables are run, and no global tools or registry are changed.
pub fn cli_install() -> Result<(), String> {
    let mut root = None;
    let mut accepted = false;
    let mut show_progress = false;
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(std::path::PathBuf::from(args.next().ok_or("--root needs a directory")?)),
            "--accept-ms-license" => accepted = true,
            "--progress" => show_progress = true,
            "--help" | "-h" => {
                crate::setup_note!("makepad-builder-cli windows-sdk --root DIR --accept-ms-license\n\
                    Downloads Visual Studio 2022 C++ tools and Windows SDK from Microsoft.\n\
                    Extracts VSIX/MSI/CAB into DIR/sdk without running Windows installers.\n\
                    https://visualstudio.microsoft.com/license-terms/vs2022-ga-diagnosticbuildtools/\n\
                    https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license");
                return Ok(());
            }
            _ => return Err(format!("unknown Windows SDK option {arg}")),
        }
    }
    let root = root.ok_or("windows-sdk requires --root DIR")?;
    let root = crate::validate_install_root(&root)?;
    if !accepted {
        return Err("Read Microsoft's Build Tools and Windows SDK terms (windows-sdk --help), then pass --accept-ms-license to download and extract them".into());
    }
    if show_progress { crate::tui::with_progress(|| install(&root.join("cache"), &root.join("sdk"))) }
    else { install(&root.join("cache"), &root.join("sdk")) }
}

pub fn ready(dest: &Path) -> bool {
    find_cl(dest).is_some() && walk_find(dest, "kernel32.lib").is_some() && walk_find(dest, "libucrt.lib").is_some()
}

pub fn install(cache: &Path, dest: &Path) -> Result<(), String> {
    crate::progress::package("Microsoft tools + Windows SDK", "Read package catalog", 0, 0);
    if find_cl(dest).is_some()
        && walk_find(dest, "kernel32.lib").is_some()
        && walk_find(dest, "libucrt.lib").is_some()
    {
        crate::progress::stage("Ready", "Microsoft tools and SDK already installed", 1.0);
        return Ok(());
    }
    crate::setup_note!("msvc: vs 17 release channel");
    let channel_bytes = http::fetch_bytes(CHANNEL)?;
    let channel = json::parse(&channel_bytes).map_err(|e| format!("channel json: {e}"))?;
    let vsman_url = channel_item_payload(&channel, "Microsoft.VisualStudio.Manifests.VisualStudio")?;
    crate::setup_note!("msvc: catalog {vsman_url}");
    let vsman_bytes = http::fetch_bytes(&vsman_url)?;
    let vsman = json::parse(&vsman_bytes).map_err(|e| format!("vsman json: {e}"))?;
    let packages = vsman
        .get("packages")
        .and_then(Value::as_arr)
        .ok_or("vsman packages")?;

    let mut by_id: HashMap<String, Vec<&Value>> = HashMap::new();
    for p in packages {
        if let Some(id) = p.get("id").and_then(Value::as_str) {
            by_id.entry(id.to_ascii_lowercase()).or_default().push(p);
        }
    }

    let msvc_pid = latest_msvc(&by_id).ok_or("no MSVC tools package")?;
    let msvc_ver = msvc_pid
        .strip_prefix("microsoft.vc.")
        .and_then(|s| s.strip_suffix(".tools.hostx64.targetx64.base"))
        .ok_or("msvc pid parse")?
        .to_string();
    let sdk_pid = latest_sdk(&by_id).ok_or("no Windows SDK component")?;
    crate::setup_note!("msvc: VC {msvc_ver}");
    crate::setup_note!("msvc: SDK {sdk_pid}");

    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    unpack_msvc_vsix(&by_id, cache, dest, &msvc_ver)?;
    unpack_sdk(&by_id, cache, dest, &sdk_pid)?;
    write_nvcc_bats(dest)?;
    if find_cl(dest).is_none() {
        return Err("cl.exe missing after msvc extract".into());
    }
    if walk_find(dest, "kernel32.lib").is_none() {
        return Err("kernel32.lib missing after sdk extract".into());
    }
    if walk_find(dest, "libucrt.lib").is_none() {
        return Err("libucrt.lib missing after sdk extract".into());
    }
    crate::progress::stage("Ready", "Microsoft tools and SDK installed", 1.0);
    Ok(())
}

pub fn find_cl(root: &Path) -> Option<std::path::PathBuf> {
    walk_find(root, "cl.exe")
}

#[allow(dead_code)]
pub fn find_link(root: &Path) -> Option<std::path::PathBuf> {
    walk_find(root, "link.exe")
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

fn unpack_msvc_vsix(
    by_id: &HashMap<String, Vec<&Value>>,
    cache: &Path,
    dest: &Path,
    ver: &str,
) -> Result<(), String> {
    let host = "x64";
    let target = "x64";
    let mut ids = vec![
        format!("microsoft.visualcpp.dia.sdk"),
        format!("microsoft.vc.{ver}.crt.headers.base"),
        format!("microsoft.vc.{ver}.crt.source.base"),
        format!("microsoft.vc.{ver}.tools.host{host}.target{target}.base"),
        format!("microsoft.vc.{ver}.tools.host{host}.target{target}.res.base"),
        format!("microsoft.vc.{ver}.crt.{target}.desktop.base"),
        format!("microsoft.vc.{ver}.crt.{target}.store.base"),
    ];
    if let Some(redist) = redist_pkg(by_id, ver, target) {
        ids.push(redist);
    }
    let count = ids.len();
    // One bar for the whole component: every payload's size is in the
    // manifest, so the work is known before the first download.
    let mut packages = Vec::new();
    for (index, id) in ids.into_iter().enumerate() {
        crate::progress::package("Build tools", &id, index + 1, count);
        match pick_pkg(by_id, &id) {
            Some(pkg) => packages.push(pkg),
            None => crate::setup_note!("  msvc skip missing {id}"),
        }
    }
    let payloads: Vec<&Value> = packages
        .iter()
        .filter_map(|pkg| pkg.get("payloads").and_then(Value::as_arr))
        .flatten()
        .filter(|p| payload_url(p).is_ok_and(|(_, _, file)| file.ends_with(".vsix") || file.ends_with(".zip") || file.ends_with(".msi")))
        .collect();
    crate::progress::total_begin("Build tools", payloads.iter().map(|p| payload_size(p)).sum());
    let result = payloads.iter().try_for_each(|p| download_and_unzip(p, cache, dest));
    crate::progress::total_end();
    result
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

fn unpack_sdk(
    by_id: &HashMap<String, Vec<&Value>>,
    cache: &Path,
    dest: &Path,
    sdk_pid: &str,
) -> Result<(), String> {
    let sdk_comp = pick_pkg(by_id, sdk_pid).ok_or("sdk component")?;
    let dep = first_dep_id(sdk_comp).ok_or("sdk dependency")?;
    let sdk_pkg = pick_pkg(by_id, &dep).ok_or_else(|| format!("sdk pkg {dep}"))?;
    let payloads = sdk_pkg
        .get("payloads")
        .and_then(Value::as_arr)
        .ok_or("sdk payloads")?;

    let msi_names = [
        "Windows SDK Desktop Headers x64-x86_en-us.msi",
        "Windows SDK Desktop Headers x86-x86_en-us.msi",
        "Windows SDK Desktop Libs x64-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Headers-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Libs-x86_en-us.msi",
        "Universal CRT Headers Libraries and Sources-x86_en-us.msi",
    ];

    // The MSIs are small and name the CABs they need; read them first so the
    // component's bar knows its whole work before the large downloads.
    crate::progress::package("Windows SDK", "Read package list", 0, 0);
    let mut msis: Vec<(String, Vec<u8>, u64, Vec<&Value>)> = Vec::new();
    for name in msi_names {
        let Some(payload) = payload_named(payloads, name) else {
            crate::setup_note!("  sdk skip {name}");
            continue;
        };
        let (url, sha, file) = payload_url(payload)?;
        let path = http::cached_file(cache, &url, &file, sha.as_deref())?;
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let cabs: Vec<&Value> = scan_cab_names(&bytes).iter().filter_map(|cab| payload_named(payloads, cab)).collect();
        msis.push((name.to_string(), bytes, payload_size(payload), cabs));
    }
    let mut cabs: Vec<&Value> = Vec::new();
    for (_, _, _, needed) in &msis {
        for cab in needed {
            if !cabs.iter().any(|c| std::ptr::eq(*c, *cab)) {
                cabs.push(cab);
            }
        }
    }
    let fallback = cabs.is_empty();
    if fallback {
        crate::setup_note!("  no .cab names scanned from msi; downloading sdk cab payloads");
        cabs = payloads
            .iter()
            .filter(|p| payload_url(p).is_ok_and(|(_, _, file)| {
                let lower = file.to_ascii_lowercase();
                lower.ends_with(".cab") && !lower.contains("arm")
            }))
            .collect();
    }
    let msi_bytes: u64 = msis.iter().map(|m| m.2).sum();
    let cab_bytes: u64 = cabs.iter().map(|c| payload_size(c)).sum();
    crate::progress::total_begin("Windows SDK", msi_bytes + cab_bytes);
    let result = (|| {
        // The MSIs are already downloaded.
        crate::progress::step(msi_bytes);
        crate::progress::step_done();
        let mut cab_map: HashMap<String, Vec<u8>> = HashMap::new();
        for cab in &cabs {
            let (url, sha, file) = payload_url(cab)?;
            crate::setup_note!("  cab {file}");
            crate::progress::step(payload_size(cab));
            let bytes = fs::read(http::cached_file(cache, &url, &file, sha.as_deref())?).map_err(|e| e.to_string())?;
            crate::progress::step_done();
            // unpack_msi searches every CAB it is given; one entry each.
            cab_map.insert(file.to_ascii_lowercase(), bytes);
        }
        let count = msis.len().max(1) as u64;
        for (name, bytes, size, needed) in &msis {
            // Each MSI unpacks its own CABs; without the scanned names the
            // CAB bytes are shared out evenly.
            let weight = size + if fallback { cab_bytes / count } else { needed.iter().map(|c| payload_size(c)).sum() };
            crate::progress::step(weight);
            crate::setup_note!("  msi unpack {name} (no msiexec)");
            match msi::unpack_msi(bytes, &cab_map, dest) {
                Ok(n) => crate::setup_note!("    {n} files"),
                Err(e) => crate::setup_note!("    WARN {name}: {e}"),
            }
            crate::progress::step_done();
        }
        Ok(())
    })();
    crate::progress::total_end();
    result
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

/// One payload: download (a cached copy counts as downloaded), then unpack
/// VSIX/ZIP contents; each is one step of the payload's size.
fn download_and_unzip(p: &Value, cache: &Path, dest: &Path) -> Result<(), String> {
    let (url, sha, file) = payload_url(p)?;
    let size = payload_size(p);
    crate::setup_note!("  vsix {file}");
    crate::progress::step(size);
    let path = http::cached_file(cache, &url, &file, sha.as_deref())?;
    crate::progress::step_done();
    crate::progress::step(size);
    if file.ends_with(".vsix") || file.ends_with(".zip") {
        extract::unzip_file(&path, dest, Some("Contents"))?;
    }
    crate::progress::step_done();
    Ok(())
}

/// The payload's byte size from the manifest (0 when absent).
fn payload_size(p: &Value) -> u64 {
    p.get("size").and_then(Value::as_u64).unwrap_or(0)
}

fn payload_url(p: &Value) -> Result<(String, Option<String>, String), String> {
    let url = p
        .get("url")
        .and_then(Value::as_str)
        .ok_or("payload url")?
        .to_string();
    let sha = p.get("sha256").and_then(Value::as_str).map(|s| s.to_string());
    let file = p
        .get("fileName")
        .and_then(Value::as_str)
        .map(|s| s.rsplit(['/', '\\']).next().unwrap_or(s).to_string())
        .unwrap_or_else(|| url.rsplit('/').next().unwrap_or("payload").to_string());
    Ok((url, sha, file))
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

fn channel_item_payload(channel: &Value, id: &str) -> Result<String, String> {
    let items = channel
        .get("channelItems")
        .and_then(Value::as_arr)
        .ok_or("channelItems")?;
    for it in items {
        if it.get("id").and_then(Value::as_str) == Some(id) {
            let payloads = it.get("payloads").and_then(Value::as_arr).ok_or("payloads")?;
            let url = payloads
                .first()
                .and_then(|p| p.get("url"))
                .and_then(Value::as_str)
                .ok_or("payload url")?;
            return Ok(url.to_string());
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
