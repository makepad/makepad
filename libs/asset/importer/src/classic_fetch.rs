//! Official / shareware unpack for classic Import.
//!
//! Libre packs come from their GitHub releases. id / 3D Realms shareware
//! comes from labeled Archive.org items. Retail WADs/PAKs are never fetched.
//!
//! Asset UI downloads the URL through platform `cx.http_request`. This
//! module only names the URL and unpacks the bytes: a ZIP, a stub EXE with
//! a ZIP glued on (`PK\x03\x04`), a raw IWAD/PACK/GRP, or an ISO 9660
//! image (cooked 2048 or raw Mode-1 2352). Duke shareware is an ISOBuster
//! dump — do not KenSilverman-scan the raw 2352 bytes; walk the ISO and
//! cook `DUKE3D.GRP` via HTTP ranges.

use crate::classic_import::ClassicSource;
use makepad_zip_file::zip_read_central_directory;
use std::io::Cursor;
use std::path::Path;

const LOCAL_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const MAX_ARCHIVE_BYTES: usize = 200 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ClassicFetchSpec {
    pub urls: &'static [&'static str],
    pub label: &'static str,
}

impl ClassicFetchSpec {
    pub fn url(self) -> &'static str {
        self.urls[0]
    }

    pub fn url_at(self, index: usize) -> Option<&'static str> {
        self.urls.get(index).copied()
    }
}

pub fn fetch_spec(source: ClassicSource) -> Option<ClassicFetchSpec> {
    match source {
        ClassicSource::Freedoom => Some(ClassicFetchSpec {
            urls: &[
                "https://github.com/freedoom/freedoom/releases/download/v0.13.0/freedoom-0.13.0.zip",
            ],
            label: "Freedoom 0.13.0 (GitHub release)",
        }),
        ClassicSource::LibreQuake => Some(ClassicFetchSpec {
            urls: &[
                "https://github.com/lavenderdotpet/LibreQuake/releases/download/v0.09-beta/full.zip",
            ],
            label: "LibreQuake v0.09-beta full (GitHub release)",
        }),
        ClassicSource::Doom => Some(ClassicFetchSpec {
            urls: &[
                "https://archive.org/download/DoomsharewareEpisode/DoomV1.9sw1995idSoftwareInc.action.zip",
            ],
            label: "DOOM v1.9 shareware zip with loose DOOM1.WAD (Archive.org / id Software)",
        }),
        ClassicSource::Quake => Some(ClassicFetchSpec {
            urls: &[
                "https://archive.org/download/Quake_802/zQUAKE_SW-play.zip",
                "https://archive.org/download/Quake_802/zQUAKE_SW-play.zip/ID1%2FPAK0.PAK",
            ],
            label: "Quake shareware ID1/PAK0.PAK (Archive.org / Quake Shareware Episode)",
        }),
        ClassicSource::Duke3d => Some(ClassicFetchSpec {
            urls: &[
                "https://archive.org/download/cdrom-duke-nukem-3d-shareware/DukeNukem3dShareware.iso",
            ],
            label: "Duke Nukem 3D shareware episode ISO (Archive.org / 3D Realms)",
        }),
        ClassicSource::Quake2 => Some(ClassicFetchSpec {
            urls: &[
                "https://archive.org/download/QuakeII_1020/q2_test.zip",
                "https://archive.org/download/QuakeII_1020/q2_test.zip/baseq2%2Fpak0.pak",
            ],
            label: "Quake II Test demo baseq2/pak0.pak (Archive.org / id Software)",
        }),
        ClassicSource::Quake3 => Some(ClassicFetchSpec {
            urls: &[
                // Loki/id Linux demo: shell stub + gzip tar with demoq3/pak0.pk3.
                // The Windows Q3ADemo.exe is a custom installer, not a zip-sfx.
                "https://ftp.gwdg.de/pub/misc/ftp.idsoftware.com/idstuff/quake3/linux/linuxq3ademo-1.11-6.x86.gz.sh",
                "https://archive.org/download/ftp.idsoftware.com/idstuff/quake3/linux/linuxq3ademo-1.11-6.x86.gz.sh",
            ],
            label: "Quake III Arena Linux demo (id Software / ftp.idsoftware.com mirror)",
        }),
        ClassicSource::DarkMod => Some(ClassicFetchSpec {
            urls: &[crate::tdm_zipsync::TDM_INSTALLER_INI_URL],
            label: "The Dark Mod official zipsync (tdm_installer.ini + mirror PK4s)",
        }),
    }
}

pub fn has_classic_payload(dir: &Path) -> bool {
    payload_walk(dir, 0)
}

/// Duke needs `DUKE3D.GRP`. A stray `.wad` left by a failed ISO scan is not
/// a Duke payload (we have seen `DOOM1.WAD` that is actually `DUKE.RTS`).
pub fn has_source_payload(source: ClassicSource, dir: &Path) -> bool {
    match source {
        ClassicSource::Duke3d => payload_files(dir)
            .iter()
            .any(|name| name.to_ascii_lowercase().ends_with(".grp")),
        _ => has_classic_payload(dir),
    }
}

pub fn payload_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_payload_names(dir, 0, &mut out);
    out.sort();
    out
}

fn collect_payload_names(dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_payload_names(&path, depth + 1, out);
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if is_keep_name(&name.to_ascii_lowercase()) {
            out.push(name);
        }
    }
}

fn payload_walk(dir: &Path, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if payload_walk(&path, depth + 1) {
                return true;
            }
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if is_keep_name(&name) {
            return true;
        }
    }
    false
}

/// Unpack a downloaded shareware archive (zip / exe+zip / raw wad/pak/grp /
/// ISO with an embedded KenSilverman GRP) into `dest`.
pub fn unpack_downloaded(bytes: &[u8], dest: &Path) -> Result<(), String> {
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "download is {} bytes; max is {MAX_ARCHIVE_BYTES}",
            bytes.len()
        ));
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    if let Some(name) = whole_file_payload_name(bytes) {
        write_rel(dest, name, bytes)?;
        return Ok(());
    }
    let _ = unpack_archive(bytes, dest, 0);
    if !has_classic_payload(dest) {
        let _ = unpack_exe_members(bytes, dest);
    }
    if !has_classic_payload(dest) && crate::iso9660::looks_like_iso(bytes) {
        let _ = crate::iso9660::extract_keep_from_image(
            bytes,
            dest,
            crate::iso9660::iso_keep_name,
        );
    }
    if !has_classic_payload(dest) {
        extract_magic_payloads(bytes, dest)?;
    }
    if !has_classic_payload(dest) {
        let _ = unpack_makeself_gzip(bytes, dest);
    }
    if !has_classic_payload(dest) {
        return Err(format!(
            "downloaded {} bytes but found no wad/pak/grp/pk3 under {}",
            bytes.len(),
            dest.display()
        ));
    }
    Ok(())
}

fn unpack_archive(bytes: &[u8], dest: &Path, depth: usize) -> Result<(), String> {
    if depth > 3 {
        return Ok(());
    }
    let zip = zip_payload(bytes).ok_or("no ZIP local-file header (PK\\x03\\x04) in archive/exe")?;
    let mut cursor = Cursor::new(zip);
    let dir = zip_read_central_directory(&mut cursor).map_err(|e| format!("zip directory: {e:?}"))?;
    for header in &dir.file_headers {
        let Some(rel) = safe_rel(&header.file_name) else {
            continue;
        };
        let lower = rel.to_ascii_lowercase();
        let data = match header.extract(&mut cursor) {
            Ok(data) => data,
            Err(_) => continue,
        };
        if is_keep_name(&lower) {
            let out = dest.join(&rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&out, &data).map_err(|e| format!("write {}: {e}", out.display()))?;
            continue;
        }
        // Nested zip of data files only. Skip installer EXEs unless we
        // still have no payload after this pass.
        if is_archive_name(&lower) && !lower.ends_with(".exe") {
            unpack_archive(&data, dest, depth + 1)?;
        }
    }
    Ok(())
}

fn unpack_exe_members(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let zip = match zip_payload(bytes) {
        Some(zip) => zip,
        None => return Ok(()),
    };
    let mut cursor = Cursor::new(zip);
    let Ok(dir) = zip_read_central_directory(&mut cursor) else {
        return Ok(());
    };
    for header in &dir.file_headers {
        let Some(rel) = safe_rel(&header.file_name) else {
            continue;
        };
        if !rel.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        if let Ok(data) = header.extract(&mut cursor) {
            let _ = unpack_archive(&data, dest, 1);
        }
    }
    Ok(())
}

fn zip_payload(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.starts_with(&LOCAL_SIG) {
        return Some(bytes);
    }
    let pos = bytes.windows(4).position(|w| w == LOCAL_SIG)?;
    Some(&bytes[pos..])
}

fn is_archive_name(name: &str) -> bool {
    name.ends_with(".zip") || name.ends_with(".exe") || name.ends_with(".shr")
}

fn is_keep_name(name: &str) -> bool {
    const EXTS: &[&str] = &[
        ".wad", ".pak", ".pk3", ".pk4", ".grp", ".map", ".bsp", ".mdl", ".md2", ".md3",
        ".md5mesh", ".spr", ".wav", ".tga", ".wal", ".lmp", ".proc",
    ];
    EXTS.iter().any(|ext| name.ends_with(ext))
}

fn safe_rel(name: &str) -> Option<String> {
    let rel = name.replace('\\', "/");
    if rel.is_empty() || rel.ends_with('/') {
        return None;
    }
    if rel.starts_with('/')
        || rel
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c.contains(':'))
    {
        return None;
    }
    Some(rel)
}

fn write_rel(dest: &Path, rel: &str, data: &[u8]) -> Result<(), String> {
    let out = dest.join(rel);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&out, data).map_err(|e| format!("write {}: {e}", out.display()))
}

fn whole_file_payload_name(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"IWAD") || bytes.starts_with(b"PWAD") {
        return Some("DOOM1.WAD");
    }
    if bytes.starts_with(b"PACK") {
        return Some("pak0.pak");
    }
    if bytes.starts_with(b"KenSilverman") {
        return Some("DUKE3D.GRP");
    }
    None
}

/// Loki/id `.sh` installers: a text stub, then a gzip stream (usually a tar
/// that contains `demoq3/pak0.pk3`).
fn unpack_makeself_gzip(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let Some(at) = find_gzip(bytes) else {
        return Ok(());
    };
    let tar = makepad_fast_inflate::gzip_decompress_vec(&bytes[at..])
        .map_err(|e| format!("gzip decompress: {e:?}"))?;
    if tar.starts_with(b"PK\x03\x04") {
        write_rel(dest, "demoq3/pak0.pk3", &tar)?;
        return Ok(());
    }
    if let Some((name, data)) = extract_tar_member(&tar, ".pk3") {
        write_rel(dest, &name, &data)?;
    }
    Ok(())
}

fn find_gzip(bytes: &[u8]) -> Option<usize> {
    bytes.windows(3).position(|w| w == [0x1f, 0x8b, 0x08])
}

/// Minimal ustar member walk. Enough for the Q3 Linux demo tarball.
fn extract_tar_member(tar: &[u8], suffix: &str) -> Option<(String, Vec<u8>)> {
    let suffix = suffix.to_ascii_lowercase();
    let mut i = 0usize;
    while i + 512 <= tar.len() {
        let hdr = &tar[i..i + 512];
        if hdr.iter().all(|&b| b == 0) {
            break;
        }
        let name_raw = &hdr[..100];
        let name_end = name_raw.iter().position(|&b| b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&name_raw[..name_end])
            .replace('\\', "/");
        let size = tar_octal(&hdr[124..136])?;
        i += 512;
        if i + size > tar.len() {
            break;
        }
        let data = tar[i..i + size].to_vec();
        i += size.div_ceil(512) * 512;
        if name.to_ascii_lowercase().ends_with(&suffix) && !name.ends_with('/') {
            let rel = name.trim_start_matches("./").to_string();
            if !rel.is_empty() {
                return Some((rel, data));
            }
        }
    }
    None
}

fn tar_octal(b: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(b).ok()?.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if s.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(s, 8).ok()
}

fn extract_magic_payloads(bytes: &[u8], dest: &Path) -> Result<(), String> {
    if let Some(wad) = slice_wad(bytes) {
        write_rel(dest, "DOOM1.WAD", wad)?;
    }
    if let Some(pak) = slice_pack(bytes) {
        write_rel(dest, "id1/pak0.pak", pak)?;
    }
    if let Some(grp) = slice_grp(bytes) {
        write_rel(dest, "DUKE3D.GRP", grp)?;
    }
    Ok(())
}

fn slice_wad(bytes: &[u8]) -> Option<&[u8]> {
    let pos = find_ascii(bytes, b"IWAD").or_else(|| find_ascii(bytes, b"PWAD"))?;
    let rest = bytes.get(pos..)?;
    if rest.len() < 12 {
        return None;
    }
    let numlumps = u32::from_le_bytes(rest[4..8].try_into().ok()?) as usize;
    let infotableofs = u32::from_le_bytes(rest[8..12].try_into().ok()?) as usize;
    let end = infotableofs.checked_add(numlumps.checked_mul(16)?)?;
    if end < 12 || end > rest.len() || numlumps == 0 || numlumps > 32_768 {
        return None;
    }
    Some(&rest[..end])
}

fn slice_pack(bytes: &[u8]) -> Option<&[u8]> {
    let pos = find_ascii(bytes, b"PACK")?;
    let rest = bytes.get(pos..)?;
    if rest.len() < 12 {
        return None;
    }
    let dirofs = u32::from_le_bytes(rest[4..8].try_into().ok()?) as usize;
    let dirlen = u32::from_le_bytes(rest[8..12].try_into().ok()?) as usize;
    let end = dirofs.checked_add(dirlen)?;
    if dirlen == 0 || dirlen % 64 != 0 || end > rest.len() || end < 12 {
        return None;
    }
    Some(&rest[..end])
}

fn slice_grp(bytes: &[u8]) -> Option<&[u8]> {
    let pos = find_ascii(bytes, b"KenSilverman")?;
    let rest = bytes.get(pos..)?;
    if rest.len() < 16 {
        return None;
    }
    let numfiles = u32::from_le_bytes(rest[12..16].try_into().ok()?) as usize;
    if numfiles == 0 || numfiles > 65_536 {
        return None;
    }
    let dir_end = 16usize.checked_add(numfiles.checked_mul(16)?)?;
    if dir_end > rest.len() {
        return None;
    }
    let mut data_end = dir_end;
    for i in 0..numfiles {
        let off = 16 + i * 16;
        let size = u32::from_le_bytes(rest[off + 12..off + 16].try_into().ok()?) as usize;
        data_end = data_end.checked_add(size)?;
    }
    if data_end > rest.len() {
        return None;
    }
    Some(&rest[..data_end])
}

fn find_ascii(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_zip_file::{ZipMethod, ZipWriter};

    #[test]
    fn zip_glued_on_exe_stub_is_found() {
        let mut zip = ZipWriter::new();
        zip.add("DOOM1.WAD", b"IWAD-test", ZipMethod::Store).unwrap();
        let archive = zip.finish().unwrap();
        let mut exe = b"MZ-stub-not-a-zip".to_vec();
        exe.extend_from_slice(&archive);
        let slice = zip_payload(&exe).expect("pk header");
        assert!(slice.starts_with(&LOCAL_SIG));
        let dir = std::env::temp_dir().join(format!("classic-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unpack_archive(&exe, &dir, 0).expect("unpack");
        let wad = std::fs::read(dir.join("DOOM1.WAD")).expect("wad");
        assert_eq!(wad, b"IWAD-test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_rel_rejects_escapes_and_drive_prefixes() {
        assert_eq!(safe_rel("id1/pak0.pak").as_deref(), Some("id1/pak0.pak"));
        assert_eq!(safe_rel("../evil.wad"), None);
        assert_eq!(safe_rel("/etc/passwd"), None);
        assert_eq!(safe_rel("c:/users/public/evil.exe"), None);
    }

    #[test]
    fn duke_payload_is_grp_not_a_stray_wad() {
        let dir = std::env::temp_dir().join(format!("duke-wad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DOOM1.WAD"), b"not-a-grp").unwrap();
        assert!(has_classic_payload(&dir));
        assert!(
            !has_source_payload(ClassicSource::Duke3d, &dir),
            "stray WAD must not skip Duke ISO fetch"
        );
        assert_eq!(payload_files(&dir), ["DOOM1.WAD"]);
        std::fs::write(dir.join("DUKE3D.GRP"), b"KenSilverman").unwrap();
        assert!(has_source_payload(ClassicSource::Duke3d, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shareware_has_archive_org_autofetch_darkmod_uses_official_zipsync() {
        let dark = fetch_spec(ClassicSource::DarkMod).expect("tdm");
        assert!(dark.url().contains("thedarkmod.com"));
        assert!(dark.url().ends_with("tdm_installer.ini"));
        for source in [
            ClassicSource::Freedoom,
            ClassicSource::LibreQuake,
            ClassicSource::Doom,
            ClassicSource::Quake,
            ClassicSource::Duke3d,
            ClassicSource::Quake2,
            ClassicSource::Quake3,
        ] {
            let spec = fetch_spec(source).unwrap_or_else(|| panic!("{source:?} needs a fetch URL"));
            for url in spec.urls {
                assert!(
                    url.starts_with("https://archive.org/")
                        || url.starts_with("https://github.com/")
                        || url.starts_with("https://ftp.gwdg.de/pub/misc/ftp.idsoftware.com/"),
                    "{source:?} url {url}"
                );
            }
        }
    }

    #[test]
    fn ken_silverman_grp_is_sliced_out_of_iso_padding() {
        let mut grp = b"KenSilverman".to_vec();
        grp.extend_from_slice(&1u32.to_le_bytes());
        let mut entry = [0u8; 16];
        entry[..4].copy_from_slice(b"TEST");
        entry[12..16].copy_from_slice(&4u32.to_le_bytes());
        grp.extend_from_slice(&entry);
        grp.extend_from_slice(b"DATA");
        let mut iso = vec![0u8; 64];
        iso.extend_from_slice(&grp);
        iso.extend_from_slice(&[0u8; 32]);
        let dir = std::env::temp_dir().join(format!("classic-grp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unpack_downloaded(&iso, &dir).expect("unpack");
        assert_eq!(std::fs::read(dir.join("DUKE3D.GRP")).unwrap(), grp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_2352_iso_is_cooked_to_grp_not_magic_scanned() {
        use crate::iso9660::{extract_keep_from_image, iso_keep_name};

        fn dir_record(lba: u32, size: u32, flags: u8, name: &[u8]) -> Vec<u8> {
            let name_len = name.len();
            let mut rec_len = 33 + name_len;
            if rec_len % 2 == 1 {
                rec_len += 1;
            }
            let mut rec = vec![0u8; rec_len];
            rec[0] = rec_len as u8;
            rec[2..6].copy_from_slice(&lba.to_le_bytes());
            rec[6..10].copy_from_slice(&lba.to_be_bytes());
            rec[10..14].copy_from_slice(&size.to_le_bytes());
            rec[14..18].copy_from_slice(&size.to_be_bytes());
            rec[25] = flags;
            rec[32] = name_len as u8;
            rec[33..33 + name_len].copy_from_slice(name);
            rec
        }

        let mut grp = b"KenSilverman".to_vec();
        grp.extend_from_slice(&0u32.to_le_bytes());
        let file_lba = 19u32;
        let total = 20u32;
        let mut cooked = vec![0u8; total as usize * 2048];
        let pvd = 16 * 2048;
        cooked[pvd] = 1;
        cooked[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        cooked[pvd + 6] = 1;
        let root = dir_record(18, 2048, 2, &[0]);
        cooked[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let mut dir = dir_record(18, 2048, 2, &[0]);
        dir.extend(dir_record(18, 2048, 2, &[1]));
        dir.extend(dir_record(file_lba, grp.len() as u32, 0, b"DUKE3D.GRP;1"));
        cooked[18 * 2048..18 * 2048 + dir.len()].copy_from_slice(&dir);
        cooked[19 * 2048..19 * 2048 + grp.len()].copy_from_slice(&grp);

        let mut raw = vec![0u8; 20 * 2352];
        for i in 0..20 {
            let sec = &mut raw[i * 2352..];
            sec[0] = 0;
            sec[1..11].fill(0xFF);
            sec[11] = 0;
            sec[15] = 1;
            sec[16..16 + 2048].copy_from_slice(&cooked[i * 2048..(i + 1) * 2048]);
        }

        let dir = std::env::temp_dir().join(format!("classic-iso-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unpack_downloaded(&raw, &dir).expect("unpack raw iso");
        assert_eq!(std::fs::read(dir.join("DUKE3D.GRP")).unwrap(), grp);
        let _ = extract_keep_from_image(&raw, &dir, iso_keep_name);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pack_magic_is_written_as_pak0() {
        let mut pak = b"PACK".to_vec();
        pak.extend_from_slice(&12u32.to_le_bytes());
        pak.extend_from_slice(&64u32.to_le_bytes());
        pak.extend_from_slice(&[0u8; 64]);
        let dir = std::env::temp_dir().join(format!("classic-pak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unpack_downloaded(&pak, &dir).expect("unpack");
        assert_eq!(std::fs::read(dir.join("pak0.pak")).unwrap(), pak);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn makeself_gzip_tar_yields_pk3() {
        let pk3 = {
            let mut z = ZipWriter::new();
            z.add("maps/q3dm1.bsp", b"IBSP-test", ZipMethod::Store).unwrap();
            z.finish().unwrap()
        };
        let name = b"demoq3/pak0.pk3";
        let mut hdr = vec![0u8; 512];
        hdr[..name.len()].copy_from_slice(name);
        let size = format!("{:o}", pk3.len());
        hdr[124..124 + size.len()].copy_from_slice(size.as_bytes());
        let mut tar = hdr;
        tar.extend_from_slice(&pk3);
        tar.resize(512 + pk3.len().div_ceil(512) * 512, 0);
        let mut sh = b"#!/bin/sh\n# stub\n".to_vec();
        sh.extend_from_slice(&makepad_fast_inflate::gzip_compress(&tar, 1));
        let dir = std::env::temp_dir().join(format!("classic-q3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unpack_downloaded(&sh, &dir).expect("unpack makeself");
        let got = std::fs::read(dir.join("demoq3/pak0.pk3")).expect("pk3");
        assert_eq!(got, pk3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
