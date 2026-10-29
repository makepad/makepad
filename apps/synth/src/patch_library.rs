//! The hybrid patch library's archives unpacked and checked: only the
//! data folders of `makepad_score_live::tandem_library::UNPACK_DIRS` are
//! taken (never skins, the third-party impulse responses or the excluded
//! wavetable folder), into the set's `data/`, and a manifest of every file
//! (path, size, SHA-256) is written from the pinned archive as it unpacks.
//! Verify re-hashes the unpacked files against that manifest.

use makepad_core_util::sha256::{to_hex, Sha256};
use makepad_model_install::{self as models, Installable};
use makepad_score_live::tandem_library::{self as lib, EXCLUDED, UNPACK_DIRS};
use makepad_zip_file::zip_read_central_directory;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The data-relative path an archive entry unpacks to, if it is taken:
/// `<top>/resources/data/<dir>/…` (source archive) or `<top>/<dir>/…`
/// (content archive).
fn take(entry: &str) -> Option<String> {
    let parts: Vec<&str> = entry.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let at = parts.iter().position(|p| UNPACK_DIRS.contains(p))?;
    // The data sits under resources/data (source archive) or the content
    // archive's data folder (`…/SurgeXTData/`), or at the top.
    let under_data = at >= 2 && parts[at - 2] == "resources" && parts[at - 1] == "data";
    let under_content = at >= 1 && parts[at - 1] == "SurgeXTData";
    if at >= 1 && !under_data && !under_content {
        return None;
    }
    if parts.iter().any(|p| *p == ".." || p.contains('\\')) {
        return None;
    }
    let rel = parts[at..].join("/");
    if EXCLUDED.iter().any(|x| rel == *x || rel.starts_with(&format!("{x}/"))) {
        return None;
    }
    Some(rel)
}

fn write_entry(data: &Path, rel: &str, bytes: &[u8], manifest: &mut Vec<String>) -> Result<(), String> {
    let dst = rel.split('/').fold(data.to_path_buf(), |p, part| p.join(part));
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&dst, bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut h = Sha256::new();
    h.update(bytes);
    manifest.push(format!("{rel}\t{}\t{}", bytes.len(), to_hex(&h.finalize())));
    Ok(())
}

/// Unpack an installed set (`progress(done, total)`); the patch count.
pub fn unpack(set: &Installable, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<usize, String> {
    let archive = models::dest_path(set);
    let data = lib::data_dir(set);
    if data.exists() {
        std::fs::remove_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    }
    std::fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let mut manifest = Vec::new();
    let name = archive.to_string_lossy().to_string();
    if name.ends_with(".zip") {
        let mut file = std::fs::File::open(&archive).map_err(|e| format!("{name}: {e}"))?;
        let dir = zip_read_central_directory(&mut file).map_err(|e| format!("{name}: {e:?}"))?;
        let total = dir.file_headers.len();
        for (i, h) in dir.file_headers.iter().enumerate() {
            if h.file_name.ends_with('/') {
                continue;
            }
            if let Some(rel) = take(&h.file_name) {
                let bytes = h.extract(&mut file).map_err(|e| format!("{}: {e:?}", h.file_name))?;
                write_entry(&data, &rel, &bytes, &mut manifest)?;
            }
            if i % 64 == 0 {
                progress(i, total);
            }
        }
    } else {
        let gz = std::fs::read(&archive).map_err(|e| format!("{name}: {e}"))?;
        let tar = makepad_fast_inflate::gzip_decompress_vec(&gz).map_err(|e| format!("{name}: {e}"))?;
        drop(gz);
        let entries = tar_entries(&tar)?;
        let total = entries.len();
        for (i, (path, at, len)) in entries.iter().enumerate() {
            if let Some(rel) = take(path) {
                write_entry(&data, &rel, &tar[*at..*at + *len], &mut manifest)?;
            }
            if i % 64 == 0 {
                progress(i, total);
            }
        }
    }
    manifest.sort();
    let mut f = std::fs::File::create(lib::manifest_path(set)).map_err(|e| e.to_string())?;
    for line in &manifest {
        writeln!(f, "{line}").map_err(|e| e.to_string())?;
    }
    Ok(manifest.iter().filter(|l| l.split('\t').next().is_some_and(|p| p.to_ascii_lowercase().ends_with(".fxp"))).count())
}

/// The regular files of a ustar / GNU tar: (path, data offset, length).
fn tar_entries(tar: &[u8]) -> Result<Vec<(String, usize, usize)>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut long_name: Option<String> = None;
    let field = |b: &[u8]| String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned();
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let size = usize::from_str_radix(field(&h[124..136]).trim(), 8).map_err(|_| "tar: bad size field".to_string())?;
        let kind = h[156];
        let mut name = field(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = field(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let data = at + 512;
        if data + size > tar.len() {
            return Err("tar: truncated".into());
        }
        match kind {
            b'L' => long_name = Some(field(&tar[data..data + size])),
            b'0' | 0 => out.push((long_name.take().unwrap_or(name), data, size)),
            _ => long_name = None,
        }
        at = data + size.div_ceil(512) * 512;
    }
    Ok(out)
}

/// Re-hash the unpacked files against the manifest.
pub fn verify(set: &Installable, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<usize, String> {
    let text = std::fs::read_to_string(lib::manifest_path(set)).map_err(|e| format!("no manifest: {e}"))?;
    let data = lib::data_dir(set);
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let mut parts = line.split('\t');
        let (Some(rel), Some(size), Some(sum)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let path: PathBuf = rel.split('/').fold(data.clone(), |p, part| p.join(part));
        let bytes = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
        if bytes.len().to_string() != size {
            return Err(format!("{rel}: size changed"));
        }
        let mut h = Sha256::new();
        h.update(&bytes);
        if to_hex(&h.finalize()) != sum {
            return Err(format!("{rel}: SHA-256 changed"));
        }
        if i % 64 == 0 {
            progress(i, lines.len());
        }
    }
    Ok(lines.len())
}

/// Remove a set: the archive (by the installer), its data and manifest.
pub fn remove_unpacked(set: &Installable) -> Result<(), String> {
    let dir = lib::dir(set);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::take;

    #[test]
    fn takes_only_the_data_folders() {
        assert_eq!(take("surge-abc/resources/data/patches_factory/Leads/A.fxp").as_deref(), Some("patches_factory/Leads/A.fxp"));
        assert_eq!(take("surge-abc/src/common/patches_factory/x.cpp"), None);
        assert_eq!(take("surge-abc/resources/data/skins/a.png"), None);
        assert_eq!(take("surge-abc/resources/data/wavetables/Waldorf/x.wt"), None);
        assert_eq!(take("surge-abc/resources/data/impulses_3rdparty/Voxengo/a.wav"), None);
        assert_eq!(take("SurgeXTData/patches_3rdparty/qb/B.fxp").as_deref(), Some("patches_3rdparty/qb/B.fxp"));
        assert_eq!(take("./Surge Synth Team/SurgeXTData/patches_factory/Leads/A.fxp").as_deref(), Some("patches_factory/Leads/A.fxp"));
    }
}
