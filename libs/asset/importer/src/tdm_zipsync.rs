//! Official The Dark Mod fetch via the same HTTP zipsync the installer uses.
//!
//! 1. Read `tdm_installer.ini` (versions + mirrors).
//! 2. Walk `default` → `depends` down to the full `release200` package.
//! 3. Read each `manifest.iniz` (zip of `data.ini`).
//! 4. For every target `tdm_*.pk4` member, pick the newest provided copy
//!    (non-zero byterange) and download that remote zip.
//! 5. Rebuild local `tdm_*.pk4` files from the downloaded zips.
//!
//! Fan-mission trees (`fms/`) are not fetched. Member copies are pulled
//! with coalesced HTTP Range requests (same idea as the official installer)
//! so we do not download unused zip tails. Completed spans stay in
//! `.tdm-sync/` and are reused on the next import.

use crate::doom3_import::is_fan_mission_rel;
use makepad_zip_file::{
    zip_read_central_directory, LocalFileHeader, ZipMethod, ZipWriter,
    COMPRESS_METHOD_DEFLATED, COMPRESS_METHOD_UNCOMPRESSED,
};
use makepad_fast_inflate::inflate::decompress_to_vec;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

pub const TDM_INSTALLER_INI_URL: &str =
    "https://update.thedarkmod.com/zipsync/tdm_installer.ini";
pub const TDM_CENTRAL_MIRROR: &str = "https://update.thedarkmod.com/zipsync";

#[derive(Clone, Debug)]
pub struct TdmInstaller {
    pub mirrors: Vec<TdmMirror>,
    pub versions: BTreeMap<String, TdmVersion>,
}

#[derive(Clone, Debug)]
pub struct TdmMirror {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct TdmVersion {
    pub name: String,
    pub manifest_url: String,
    pub depends: Option<String>,
    pub default: bool,
}

#[derive(Clone, Debug)]
pub struct TdmFile {
    pub zip_rel: String,
    pub filename: String,
    pub contents_hash: String,
    pub byterange: Option<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub struct TdmProvidedSet {
    pub version: String,
    pub package_root: String,
    pub files: Vec<TdmFile>,
}

/// One HTTP GET: a coalesced exclusive-end byte span on a remote zip.
#[derive(Clone, Debug)]
pub struct TdmFetchSpan {
    pub url: String,
    pub start: u64,
    pub end: u64,
    pub cache_name: String,
}

#[derive(Clone, Debug)]
pub struct TdmMemberPlan {
    pub dest_zip: String,
    pub filename: String,
    pub contents_hash: String,
    pub source_url: String,
    pub range: (u64, u64),
}

#[derive(Clone, Debug)]
pub struct TdmFetchPlan {
    pub version: String,
    pub spans: Vec<TdmFetchSpan>,
    pub members: Vec<TdmMemberPlan>,
}

/// Merge nearby member ranges on the same zip so one GET covers many files.
const RANGE_MERGE_GAP: u64 = 256 * 1024;

pub fn parse_installer_ini(text: &str) -> Result<TdmInstaller, String> {
    let mut mirrors = Vec::new();
    let mut versions = BTreeMap::new();
    let mut section = String::new();
    let mut mirror_name = String::new();
    let mut mirror_url = String::new();
    let mut ver_name = String::new();
    let mut ver_manifest = String::new();
    let mut ver_depends = None;
    let mut ver_default = false;

    let flush_mirror = |mirrors: &mut Vec<TdmMirror>, name: &str, url: &str| {
        if !name.is_empty() && !url.is_empty() {
            mirrors.push(TdmMirror {
                name: name.to_string(),
                url: url.trim_end_matches('/').to_string(),
            });
        }
    };
    let flush_version = |versions: &mut BTreeMap<String, TdmVersion>,
                         name: &str,
                         manifest: &str,
                         depends: &Option<String>,
                         default: bool| {
        if name.is_empty() || manifest.is_empty() {
            return;
        }
        versions.insert(
            name.to_string(),
            TdmVersion {
                name: name.to_string(),
                manifest_url: manifest.to_string(),
                depends: depends.clone(),
                default,
            },
        );
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = section_name(line) {
            flush_mirror(&mut mirrors, &mirror_name, &mirror_url);
            flush_version(
                &mut versions,
                &ver_name,
                &ver_manifest,
                &ver_depends,
                ver_default,
            );
            section = name;
            mirror_name.clear();
            mirror_url.clear();
            ver_name.clear();
            ver_manifest.clear();
            ver_depends = None;
            ver_default = false;
            if let Some(rest) = section.strip_prefix("Mirror ") {
                mirror_name = rest.to_string();
            } else if let Some(rest) = section.strip_prefix("Version ") {
                ver_name = rest.to_string();
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if section.starts_with("Mirror ") {
            if key == "url" {
                mirror_url = value.to_string();
            }
        } else if section.starts_with("Version ") {
            match key {
                "manifestUrl" => ver_manifest = value.to_string(),
                "depends" => {
                    if !value.is_empty() {
                        ver_depends = Some(value.to_string());
                    }
                }
                "default" => ver_default = value == "1" || value.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
    }
    flush_mirror(&mut mirrors, &mirror_name, &mirror_url);
    flush_version(
        &mut versions,
        &ver_name,
        &ver_manifest,
        &ver_depends,
        ver_default,
    );
    if versions.is_empty() {
        return Err("tdm_installer.ini has no Version sections".into());
    }
    Ok(TdmInstaller { mirrors, versions })
}

fn section_name(line: &str) -> Option<String> {
    let line = line.trim();
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim().to_string())
}

impl TdmInstaller {
    pub fn default_version(&self) -> Result<&TdmVersion, String> {
        self.versions
            .values()
            .find(|v| v.default)
            .or_else(|| self.versions.get("release214"))
            .ok_or_else(|| "tdm_installer.ini has no default version".into())
    }

    pub fn version_chain(&self) -> Result<Vec<&TdmVersion>, String> {
        let mut out = Vec::new();
        let mut name = self.default_version()?.name.clone();
        let mut guard = 0;
        while !name.is_empty() {
            guard += 1;
            if guard > 64 {
                return Err("tdm version depends loop".into());
            }
            let ver = self
                .versions
                .get(&name)
                .ok_or_else(|| format!("tdm version {name} missing from installer.ini"))?;
            out.push(ver);
            match &ver.depends {
                Some(next) => name = next.clone(),
                None => break,
            }
        }
        Ok(out)
    }

    pub fn preferred_mirror(&self) -> String {
        TDM_CENTRAL_MIRROR.to_string()
    }

    pub fn manifest_http_url(&self, version: &TdmVersion) -> String {
        resolve_manifest_url(&version.manifest_url, &self.preferred_mirror())
    }
}

pub fn resolve_manifest_url(template: &str, mirror: &str) -> String {
    let mirror = mirror.trim_end_matches('/');
    template
        .replace("${MIRROR}", mirror)
        .replace("http://update.thedarkmod.com/zipsync", TDM_CENTRAL_MIRROR)
}

pub fn package_root_from_manifest_url(url: &str) -> String {
    match url.rsplit_once('/') {
        Some((root, _)) => format!("{root}/"),
        None => url.to_string(),
    }
}

pub fn parse_manifest_iniz(bytes: &[u8]) -> Result<Vec<TdmFile>, String> {
    let zip = if bytes.starts_with(b"PK\x03\x04") {
        bytes
    } else {
        return Err("manifest.iniz is not a zip".into());
    };
    let mut cursor = Cursor::new(zip);
    let dir = zip_read_central_directory(&mut cursor).map_err(|e| format!("manifest zip: {e:?}"))?;
    let mut ini = None;
    for header in &dir.file_headers {
        if header.file_name.eq_ignore_ascii_case("data.ini") {
            ini = Some(
                header
                    .extract(&mut cursor)
                    .map_err(|e| format!("manifest data.ini: {e:?}"))?,
            );
            break;
        }
    }
    let ini = ini.ok_or("manifest.iniz has no data.ini")?;
    let text = String::from_utf8(ini).map_err(|e| format!("manifest data.ini utf8: {e}"))?;
    parse_manifest_ini(&text)
}

pub fn parse_manifest_ini(text: &str) -> Result<Vec<TdmFile>, String> {
    let mut files = Vec::new();
    let mut section = String::new();
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let flush = |section: &str, fields: &BTreeMap<String, String>, files: &mut Vec<TdmFile>| {
        let Some(rest) = section.strip_prefix("File ") else {
            return;
        };
        let (zip_rel, filename) = match rest.split_once("||") {
            Some((z, f)) => (z.to_string(), f.to_string()),
            None => return,
        };
        let br = fields.get("byterange").map(String::as_str).unwrap_or("0-0");
        let byterange = parse_range(br);
        files.push(TdmFile {
            zip_rel,
            filename,
            contents_hash: fields
                .get("contentsHash")
                .cloned()
                .unwrap_or_default(),
            byterange,
        });
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = section_name(line) {
            flush(&section, &fields, &mut files);
            section = name;
            fields.clear();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    flush(&section, &fields, &mut files);
    Ok(files)
}

fn parse_range(text: &str) -> Option<(u64, u64)> {
    let (a, b) = text.split_once('-')?;
    let start: u64 = a.parse().ok()?;
    let end: u64 = b.parse().ok()?;
    if start < end {
        Some((start, end))
    } else {
        None
    }
}

pub fn keep_tdm_core(file: &TdmFile) -> bool {
    if is_fan_mission_rel(&file.zip_rel) || is_fan_mission_rel(&file.filename) {
        return false;
    }
    let zip = file.zip_rel.replace('\\', "/").to_ascii_lowercase();
    zip.starts_with("tdm_") && zip.ends_with(".pk4") && !zip.contains('/')
}

pub fn plan_clean_install(sets: &[TdmProvidedSet]) -> Result<TdmFetchPlan, String> {
    let target = sets
        .first()
        .ok_or("no TDM manifests downloaded")?;
    let mut members = Vec::new();
    let mut per_url: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for file in target.files.iter().filter(|f| keep_tdm_core(f)) {
        let Some((source_url, range)) = find_source(sets, file) else {
            return Err(format!(
                "no provided copy of {}||{}",
                file.zip_rel, file.filename
            ));
        };
        per_url.entry(source_url.clone()).or_default().push(range);
        members.push(TdmMemberPlan {
            dest_zip: file.zip_rel.clone(),
            filename: file.filename.clone(),
            contents_hash: file.contents_hash.clone(),
            source_url,
            range,
        });
    }
    if members.is_empty() {
        return Err("TDM target manifest has no core tdm_*.pk4 files".into());
    }
    let mut spans = Vec::new();
    for (url, mut ranges) in per_url {
        ranges.sort_by_key(|r| r.0);
        let mut cur = ranges[0];
        for next in ranges.into_iter().skip(1) {
            if next.0 <= cur.1.saturating_add(RANGE_MERGE_GAP) {
                cur.1 = cur.1.max(next.1);
            } else {
                spans.push(span_for(url.clone(), cur.0, cur.1));
                cur = next;
            }
        }
        spans.push(span_for(url, cur.0, cur.1));
    }
    Ok(TdmFetchPlan {
        version: target.version.clone(),
        spans,
        members,
    })
}

fn span_for(url: String, start: u64, end: u64) -> TdmFetchSpan {
    TdmFetchSpan {
        cache_name: format!("{}_{start}_{end}", cache_name_for_url(&url)),
        url,
        start,
        end,
    }
}

pub fn span_cache_path(cache_dir: &Path, span: &TdmFetchSpan) -> PathBuf {
    cache_dir.join(&span.cache_name)
}

pub fn span_is_cached(cache_dir: &Path, span: &TdmFetchSpan) -> bool {
    let path = span_cache_path(cache_dir, span);
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    meta.is_file() && meta.len() == span.end.saturating_sub(span.start)
}

/// Write a finished span to a temp file, then rename so a crash cannot
/// leave a short file that looks valid.
pub fn write_span_atomic(cache_dir: &Path, span: &TdmFetchSpan, body: &[u8]) -> Result<(), String> {
    let expected = span.end.saturating_sub(span.start) as usize;
    if body.len() != expected {
        return Err(format!(
            "span {} {}-{} got {} bytes, expected {expected}",
            span.url,
            span.start,
            span.end,
            body.len()
        ));
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("create {}: {e}", cache_dir.display()))?;
    let dest = span_cache_path(cache_dir, span);
    let tmp = dest.with_extension("partial");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &dest).map_err(|e| format!("rename {}: {e}", dest.display()))?;
    Ok(())
}

pub fn plan_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("plan.v1")
}

pub fn save_plan(cache_dir: &Path, plan: &TdmFetchPlan) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("create {}: {e}", cache_dir.display()))?;
    let mut out = String::from("TDM-PLAN-1\n");
    out.push_str(&format!("version\t{}\n", plan.version));
    for span in &plan.spans {
        out.push_str(&format!(
            "SPAN\t{}\t{}\t{}\t{}\n",
            span.url, span.start, span.end, span.cache_name
        ));
    }
    for member in &plan.members {
        out.push_str(&format!(
            "MEMBER\t{}\t{}\t{}\t{}\t{}\t{}\n",
            member.dest_zip,
            member.filename,
            member.contents_hash,
            member.source_url,
            member.range.0,
            member.range.1
        ));
    }
    let dest = plan_path(cache_dir);
    let tmp = dest.with_extension("partial");
    std::fs::write(&tmp, out.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &dest).map_err(|e| format!("rename {}: {e}", dest.display()))?;
    Ok(())
}

pub fn load_plan(cache_dir: &Path) -> Result<TdmFetchPlan, String> {
    let text = std::fs::read_to_string(plan_path(cache_dir)).map_err(|e| format!("read plan: {e}"))?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    if header != "TDM-PLAN-1" {
        return Err(format!("unknown plan header {header:?}"));
    }
    let mut version = String::new();
    let mut spans = Vec::new();
    let mut members = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        match cols.next() {
            Some("version") => version = cols.next().unwrap_or("").to_string(),
            Some("SPAN") => {
                let url = cols.next().unwrap_or("").to_string();
                let start: u64 = cols.next().unwrap_or("0").parse().map_err(|e| format!("span start: {e}"))?;
                let end: u64 = cols.next().unwrap_or("0").parse().map_err(|e| format!("span end: {e}"))?;
                let cache_name = cols.next().unwrap_or("").to_string();
                if url.is_empty() || cache_name.is_empty() || start >= end {
                    return Err("corrupt SPAN line".into());
                }
                spans.push(TdmFetchSpan {
                    url,
                    start,
                    end,
                    cache_name,
                });
            }
            Some("MEMBER") => {
                let dest_zip = cols.next().unwrap_or("").to_string();
                let filename = cols.next().unwrap_or("").to_string();
                let contents_hash = cols.next().unwrap_or("").to_string();
                let source_url = cols.next().unwrap_or("").to_string();
                let r0: u64 = cols.next().unwrap_or("0").parse().map_err(|e| format!("member r0: {e}"))?;
                let r1: u64 = cols.next().unwrap_or("0").parse().map_err(|e| format!("member r1: {e}"))?;
                if dest_zip.is_empty() || filename.is_empty() || source_url.is_empty() {
                    return Err("corrupt MEMBER line".into());
                }
                members.push(TdmMemberPlan {
                    dest_zip,
                    filename,
                    contents_hash,
                    source_url,
                    range: (r0, r1),
                });
            }
            _ => return Err(format!("bad plan line: {line}")),
        }
    }
    if spans.is_empty() || members.is_empty() {
        return Err("saved plan has no spans/members".into());
    }
    Ok(TdmFetchPlan {
        version,
        spans,
        members,
    })
}

pub fn cached_span_progress(cache_dir: &Path, plan: &TdmFetchPlan) -> (usize, usize, u64) {
    let mut ready = 0usize;
    let mut ready_bytes = 0u64;
    for span in &plan.spans {
        if span_is_cached(cache_dir, span) {
            ready += 1;
            ready_bytes += span.end.saturating_sub(span.start);
        }
    }
    (ready, plan.spans.len(), ready_bytes)
}

fn find_source(sets: &[TdmProvidedSet], target: &TdmFile) -> Option<(String, (u64, u64))> {
    if !target.contents_hash.is_empty() {
        for set in sets {
            for file in &set.files {
                if let Some(range) = file.byterange {
                    if file.contents_hash == target.contents_hash {
                        return Some((join_url(&set.package_root, &file.zip_rel), range));
                    }
                }
            }
        }
    }
    for set in sets {
        for file in &set.files {
            if let Some(range) = file.byterange {
                if file.zip_rel == target.zip_rel && file.filename == target.filename {
                    return Some((join_url(&set.package_root, &file.zip_rel), range));
                }
            }
        }
    }
    None
}

pub fn join_url(root: &str, rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches('/');
    if root.ends_with('/') {
        format!("{root}{rel}")
    } else {
        format!("{root}/{rel}")
    }
}

fn cache_name_for_url(url: &str) -> String {
    let rest = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .replace('/', "_")
        .replace(':', "_");
    if rest.is_empty() {
        "tdm.bin".into()
    } else {
        rest
    }
}

pub fn reconstruct_pk4s(dest: &Path, plan: &TdmFetchPlan, cache_dir: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut by_zip: BTreeMap<&str, Vec<&TdmMemberPlan>> = BTreeMap::new();
    for member in &plan.members {
        by_zip.entry(member.dest_zip.as_str()).or_default().push(member);
    }
    let mut written = 0;
    for (zip_rel, members) in by_zip {
        let mut writer = ZipWriter::new();
        for member in members {
            let span = plan
                .spans
                .iter()
                .find(|span| {
                    span.url == member.source_url
                        && span.start <= member.range.0
                        && member.range.1 <= span.end
                })
                .ok_or_else(|| format!("no cached span for {}", member.filename))?;
            let cache = span_cache_path(cache_dir, span);
            let bytes = std::fs::read(&cache).map_err(|e| format!("read {}: {e}", cache.display()))?;
            let data = extract_from_span(&bytes, span.start, member.range).map_err(|e| {
                format!("{} from {}: {e}", member.filename, cache.display())
            })?;
            writer
                .add(&member.filename, &data, ZipMethod::Store)
                .map_err(|e| format!("zip {}: {e:?}", member.filename))?;
        }
        let archive = writer.finish().map_err(|e| format!("finish {zip_rel}: {e:?}"))?;
        let out = dest.join(zip_rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&out, archive).map_err(|e| format!("write {}: {e}", out.display()))?;
        written += 1;
    }
    Ok(written)
}

fn extract_from_span(
    span_bytes: &[u8],
    span_start: u64,
    range: (u64, u64),
) -> Result<Vec<u8>, String> {
    let offset = range.0.saturating_sub(span_start) as usize;
    let len = range.1.saturating_sub(range.0) as usize;
    let slice = span_bytes
        .get(offset..offset.saturating_add(len))
        .ok_or("member range is outside the cached span")?;
    extract_local_member(slice)
}

fn extract_local_member(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut cursor = Cursor::new(bytes);
    let header = LocalFileHeader::from_stream(&mut cursor).map_err(|e| format!("local header: {e:?}"))?;
    let mut compressed = vec![0u8; header.compressed_size as usize];
    use std::io::Read;
    cursor
        .read_exact(&mut compressed)
        .map_err(|e| format!("compressed payload: {e}"))?;
    match header.compression_method {
        COMPRESS_METHOD_UNCOMPRESSED => Ok(compressed),
        COMPRESS_METHOD_DEFLATED => {
            decompress_to_vec(&compressed).map_err(|_| "deflate failed".into())
        }
        other => Err(format!("unsupported zip method {other}")),
    }
}

fn extract_member(zip_bytes: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut cursor = Cursor::new(zip_bytes);
    let dir = zip_read_central_directory(&mut cursor).map_err(|e| format!("zip dir: {e:?}"))?;
    let want = name.replace('\\', "/");
    for header in &dir.file_headers {
        if header.file_name.replace('\\', "/") == want {
            return header
                .extract(&mut cursor)
                .map_err(|e| format!("extract {name}: {e:?}"));
        }
    }
    Err(format!("zip has no member {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_zip_file::ZipWriter;

    const INI: &str = r#"
[Mirror Central]
url=http://update.thedarkmod.com/zipsync
weight=0

[Version release214]
manifestUrl=${MIRROR}/release/release214_from_release213/manifest.iniz
depends=release213
folder=releases
default=1

[Version release213]
manifestUrl=${MIRROR}/release/release213_from_release212/manifest.iniz
depends=release200
folder=releases

[Version release200]
manifestUrl=${MIRROR}/release/release200/manifest.iniz
folder=releases
"#;

    #[test]
    fn installer_ini_walks_default_chain() {
        let parsed = parse_installer_ini(INI).expect("ini");
        let chain: Vec<&str> = parsed
            .version_chain()
            .unwrap()
            .into_iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(chain, ["release214", "release213", "release200"]);
        let url = parsed.manifest_http_url(parsed.default_version().unwrap());
        assert!(url.starts_with("https://update.thedarkmod.com/zipsync/release/"));
        assert!(url.ends_with("manifest.iniz"));
    }

    #[test]
    fn plan_picks_newest_provided_copy_and_skips_fms() {
        let target = vec![
            TdmFile {
                zip_rel: "tdm_base01.pk4".into(),
                filename: "materials/tdm.mtr".into(),
                contents_hash: "aaa".into(),
                byterange: None,
            },
            TdmFile {
                zip_rel: "fms/custom/custom.pk4".into(),
                filename: "maps/foo.map".into(),
                contents_hash: "bbb".into(),
                byterange: None,
            },
        ];
        let newest = TdmProvidedSet {
            version: "release214".into(),
            package_root: "https://update.thedarkmod.com/zipsync/release/r214/".into(),
            files: vec![TdmFile {
                zip_rel: "tdm_base01.pk4".into(),
                filename: "materials/tdm.mtr".into(),
                contents_hash: "aaa".into(),
                byterange: Some((0, 10)),
            }],
        };
        let older = TdmProvidedSet {
            version: "release200".into(),
            package_root: "https://update.thedarkmod.com/zipsync/release/r200/".into(),
            files: vec![TdmFile {
                zip_rel: "tdm_base01.pk4".into(),
                filename: "materials/tdm.mtr".into(),
                contents_hash: "aaa".into(),
                byterange: Some((0, 99)),
            }],
        };
        let mut sets = vec![
            TdmProvidedSet {
                version: "release214".into(),
                package_root: newest.package_root.clone(),
                files: target,
            },
            newest,
            older,
        ];
        // first set is target list; provided copies follow
        sets[0].files[0].byterange = None;
        let plan = plan_clean_install(&sets).expect("plan");
        assert_eq!(plan.spans.len(), 1);
        assert!(plan.spans[0].url.ends_with("/tdm_base01.pk4"));
        assert!(plan.spans[0].url.contains("/r214/"));
        assert_eq!(plan.spans[0].start, 0);
        assert_eq!(plan.spans[0].end, 10);
        assert_eq!(plan.members.len(), 1);
        assert_eq!(plan.members[0].filename, "materials/tdm.mtr");
        assert!(!span_is_cached(Path::new("/tmp/no-tdm-cache"), &plan.spans[0]));
    }

    #[test]
    fn nearby_ranges_coalesce_far_ranges_do_not() {
        let target = vec![
            TdmFile {
                zip_rel: "tdm_base01.pk4".into(),
                filename: "a".into(),
                contents_hash: "a".into(),
                byterange: None,
            },
            TdmFile {
                zip_rel: "tdm_base01.pk4".into(),
                filename: "b".into(),
                contents_hash: "b".into(),
                byterange: None,
            },
            TdmFile {
                zip_rel: "tdm_base01.pk4".into(),
                filename: "c".into(),
                contents_hash: "c".into(),
                byterange: None,
            },
        ];
        let provided = TdmProvidedSet {
            version: "release200".into(),
            package_root: "https://example.test/r200/".into(),
            files: vec![
                TdmFile {
                    zip_rel: "tdm_base01.pk4".into(),
                    filename: "a".into(),
                    contents_hash: "a".into(),
                    byterange: Some((0, 100)),
                },
                TdmFile {
                    zip_rel: "tdm_base01.pk4".into(),
                    filename: "b".into(),
                    contents_hash: "b".into(),
                    byterange: Some((120, 200)),
                },
                TdmFile {
                    zip_rel: "tdm_base01.pk4".into(),
                    filename: "c".into(),
                    contents_hash: "c".into(),
                    byterange: Some((2_000_000, 2_000_050)),
                },
            ],
        };
        let sets = vec![
            TdmProvidedSet {
                version: "release214".into(),
                package_root: provided.package_root.clone(),
                files: target,
            },
            provided,
        ];
        let plan = plan_clean_install(&sets).expect("plan");
        assert_eq!(plan.spans.len(), 2);
        assert_eq!(plan.spans[0].start, 0);
        assert_eq!(plan.spans[0].end, 200);
        assert_eq!(plan.spans[1].start, 2_000_000);
    }

    #[test]
    fn reconstruct_writes_tdm_pk4_from_cached_span() {
        let mut src = ZipWriter::new();
        src.add("materials/tdm.mtr", b"shader", ZipMethod::Store)
            .unwrap();
        let src_bytes = src.finish().unwrap();
        let dir = std::env::temp_dir().join(format!("tdm-sync-{}", std::process::id()));
        let cache = dir.join("cache");
        let dest = dir.join("out");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&cache).unwrap();
        let url = "https://example.test/tdm_base01.pk4";
        let span = TdmFetchSpan {
            url: url.into(),
            start: 0,
            end: src_bytes.len() as u64,
            cache_name: "span.bin".into(),
        };
        std::fs::write(cache.join(&span.cache_name), &src_bytes).unwrap();
        assert!(span_is_cached(&cache, &span));
        let plan = TdmFetchPlan {
            version: "release214".into(),
            spans: vec![span],
            members: vec![TdmMemberPlan {
                dest_zip: "tdm_base01.pk4".into(),
                filename: "materials/tdm.mtr".into(),
                contents_hash: "x".into(),
                source_url: url.into(),
                range: (0, src_bytes.len() as u64),
            }],
        };
        let n = reconstruct_pk4s(&dest, &plan, &cache).expect("rebuild");
        assert_eq!(n, 1);
        let built = std::fs::read(dest.join("tdm_base01.pk4")).unwrap();
        assert_eq!(extract_member(&built, "materials/tdm.mtr").unwrap(), b"shader");
        save_plan(&cache, &plan).expect("save");
        let loaded = load_plan(&cache).expect("load");
        assert_eq!(loaded.spans.len(), 1);
        assert_eq!(loaded.members[0].filename, "materials/tdm.mtr");
        assert_eq!(cached_span_progress(&cache, &loaded), (1, 1, src_bytes.len() as u64));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
