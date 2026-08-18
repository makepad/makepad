//! ISO 9660 reader for cooked (2048) and raw Mode-1 (2352) images.
//!
//! Shareware CDs on Archive.org are often ISOBuster dumps: 2352-byte
//! sectors with sync/header/ECC around each 2048-byte user block. A
//! linear `KenSilverman` scan of those bytes is wrong. Walk the volume
//! descriptor + directories, then cook only the extents you need.
//!
//! Offsets are byte positions in the image. HTTP range fetchers use
//! [`IsoLayout::extent_raw_range`].

use std::path::Path;

pub const COOKED_SECTOR: u32 = 2048;
pub const RAW_SECTOR: u32 = 2352;
pub const RAW_DATA_OFF: u32 = 16;
pub const PVD_LBA: u32 = 16;
/// First 64 KiB covers cooked PVD (32 KiB) and raw PVD (~37 KiB).
pub const PROBE_BYTES: u64 = 65_536;

const SYNC: [u8; 12] = [
    0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsoLayout {
    pub sector: u32,
    pub data_off: u32,
}

impl IsoLayout {
    pub const COOKED: Self = Self {
        sector: COOKED_SECTOR,
        data_off: 0,
    };
    pub const RAW_MODE1: Self = Self {
        sector: RAW_SECTOR,
        data_off: RAW_DATA_OFF,
    };

    pub fn is_raw(self) -> bool {
        self.sector != COOKED_SECTOR
    }

    pub fn raw_offset(self, lba: u32) -> u64 {
        u64::from(lba) * u64::from(self.sector)
    }

    pub fn user_offset(self, lba: u32) -> u64 {
        self.raw_offset(lba) + u64::from(self.data_off)
    }

    /// Inclusive-start, exclusive-end byte range covering `size` user bytes
    /// at `lba` (includes sync/ECC on raw images).
    pub fn extent_raw_range(self, lba: u32, size: u32) -> (u64, u64) {
        let start = self.raw_offset(lba);
        let nsec = sectors_for(size);
        (start, start + nsec * u64::from(self.sector))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoVolume {
    pub layout: IsoLayout,
    pub volume_id: String,
    pub root_lba: u32,
    pub root_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoEntry {
    pub name: String,
    pub lba: u32,
    pub size: u32,
    pub is_dir: bool,
}

pub fn looks_like_iso(bytes: &[u8]) -> bool {
    detect_layout(bytes).is_ok()
}

pub fn detect_layout(probe: &[u8]) -> Result<IsoLayout, String> {
    if pvd_ok(probe, IsoLayout::RAW_MODE1) {
        return Ok(IsoLayout::RAW_MODE1);
    }
    if pvd_ok(probe, IsoLayout::COOKED) {
        return Ok(IsoLayout::COOKED);
    }
    if probe.len() >= 12 && probe[..12] == SYNC {
        return Err("raw CD image but no ISO 9660 PVD at LBA 16".into());
    }
    Err("not an ISO 9660 image (no CD001 PVD)".into())
}

pub fn parse_pvd(probe: &[u8], layout: IsoLayout) -> Result<IsoVolume, String> {
    let pvd = user_sector(probe, layout, PVD_LBA).ok_or("PVD not in probe")?;
    if pvd.len() < 190 || pvd[0] != 1 || &pvd[1..6] != b"CD001" {
        return Err("invalid primary volume descriptor".into());
    }
    let rec = &pvd[156..190];
    if rec[0] < 34 {
        return Err("PVD root directory record too short".into());
    }
    let root_lba = u32::from_le_bytes(rec[2..6].try_into().unwrap());
    let root_size = u32::from_le_bytes(rec[10..14].try_into().unwrap());
    if root_lba == 0 || root_size == 0 {
        return Err("PVD root directory is empty".into());
    }
    let volume_id = String::from_utf8_lossy(&pvd[40..72])
        .trim()
        .trim_end_matches('\0')
        .to_string();
    Ok(IsoVolume {
        layout,
        volume_id,
        root_lba,
        root_size,
    })
}

pub fn cook_extent(raw: &[u8], layout: IsoLayout, size: u32) -> Result<Vec<u8>, String> {
    let nsec = sectors_for(size) as usize;
    let need = nsec * layout.sector as usize;
    if raw.len() < need {
        return Err(format!(
            "ISO extent short: got {} want {need} for {size} user bytes",
            raw.len()
        ));
    }
    if !layout.is_raw() {
        return Ok(raw[..size as usize].to_vec());
    }
    let mut out = Vec::with_capacity(size as usize);
    let data_off = layout.data_off as usize;
    for i in 0..nsec {
        let sec = &raw[i * layout.sector as usize..];
        if sec.len() < data_off + 1 {
            return Err("raw sector truncated".into());
        }
        let take = (size as usize - out.len()).min(COOKED_SECTOR as usize);
        let data = sec.get(data_off..data_off + take).ok_or("raw sector data")?;
        out.extend_from_slice(data);
    }
    Ok(out)
}

pub fn parse_directory(cooked: &[u8]) -> Vec<IsoEntry> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < cooked.len() {
        let len = cooked[i] as usize;
        if len == 0 {
            i = (i / COOKED_SECTOR as usize + 1) * COOKED_SECTOR as usize;
            continue;
        }
        if i + len > cooked.len() || len < 34 {
            break;
        }
        let rec = &cooked[i..i + len];
        let lba = u32::from_le_bytes(rec[2..6].try_into().unwrap());
        let size = u32::from_le_bytes(rec[10..14].try_into().unwrap());
        let flags = rec[25];
        let nlen = rec[32] as usize;
        if 33 + nlen > rec.len() {
            break;
        }
        let raw_name = &rec[33..33 + nlen];
        if raw_name == [0] || raw_name == [1] {
            i += len;
            continue;
        }
        if let Some(name) = iso_file_name(raw_name) {
            out.push(IsoEntry {
                name,
                lba,
                size,
                is_dir: flags & 0x02 != 0,
            });
        }
        i += len;
    }
    out
}

pub fn iso_file_name(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    let stem = text.split(';').next().unwrap_or(text).trim();
    if stem.is_empty() || stem == "." || stem == ".." {
        return None;
    }
    // Record names must be single path components: anything with a
    // separator or drive/stream colon would escape `dest.join(name)`.
    if stem.contains('/') || stem.contains('\\') || stem.contains(':') || stem.contains('\0') {
        return None;
    }
    Some(stem.to_string())
}

pub fn iso_keep_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".grp")
}

/// Pull keep-files out of a whole image already in memory.
pub fn extract_keep_from_image(
    bytes: &[u8],
    dest: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>, String> {
    let layout = detect_layout(bytes)?;
    let vol = parse_pvd(bytes, layout)?;
    let mut wrote = Vec::new();
    let mut seen_dirs = std::collections::BTreeSet::new();
    let mut queue = vec![(vol.root_lba, vol.root_size, 0u32)];
    while let Some((lba, size, depth)) = queue.pop() {
        if depth > 6 || !seen_dirs.insert(lba) {
            continue;
        }
        let cooked = read_extent(bytes, layout, lba, size)?;
        for entry in parse_directory(&cooked) {
            if entry.is_dir {
                queue.push((entry.lba, entry.size, depth + 1));
                continue;
            }
            if !keep(&entry.name) {
                continue;
            }
            let file = read_extent(bytes, layout, entry.lba, entry.size)?;
            write_rel(dest, &entry.name, &file)?;
            wrote.push(entry.name);
        }
    }
    if wrote.is_empty() {
        return Err("ISO contained no keep files (no .grp)".into());
    }
    Ok(wrote)
}

fn read_extent(image: &[u8], layout: IsoLayout, lba: u32, size: u32) -> Result<Vec<u8>, String> {
    let (start, end) = layout.extent_raw_range(lba, size);
    let start = start as usize;
    let end = end as usize;
    let slice = image
        .get(start..end.min(image.len()))
        .ok_or_else(|| format!("ISO extent LBA {lba} is past end of image"))?;
    cook_extent(slice, layout, size)
}

fn pvd_ok(probe: &[u8], layout: IsoLayout) -> bool {
    user_sector(probe, layout, PVD_LBA)
        .map(|pvd| pvd.len() >= 6 && pvd[0] == 1 && &pvd[1..6] == b"CD001")
        .unwrap_or(false)
}

fn user_sector(probe: &[u8], layout: IsoLayout, lba: u32) -> Option<&[u8]> {
    let off = layout.user_offset(lba) as usize;
    probe.get(off..off + COOKED_SECTOR as usize)
}

fn sectors_for(size: u32) -> u64 {
    (u64::from(size) + u64::from(COOKED_SECTOR) - 1) / u64::from(COOKED_SECTOR)
}

fn write_rel(dest: &Path, rel: &str, data: &[u8]) -> Result<(), String> {
    let out = dest.join(rel);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&out, data).map_err(|e| format!("write {}: {e}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        rec[28] = 1;
        rec[30] = 0;
        rec[31] = 1;
        rec[32] = name_len as u8;
        rec[33..33 + name_len].copy_from_slice(name);
        rec
    }

    fn cooked_iso(file_name: &str, file_data: &[u8]) -> Vec<u8> {
        let file_sectors = sectors_for(file_data.len() as u32) as u32;
        let file_lba = 19u32;
        let total = file_lba + file_sectors.max(1);
        let mut img = vec![0u8; total as usize * COOKED_SECTOR as usize];

        let pvd_off = 16 * COOKED_SECTOR as usize;
        img[pvd_off] = 1;
        img[pvd_off + 1..pvd_off + 6].copy_from_slice(b"CD001");
        img[pvd_off + 6] = 1;
        img[pvd_off + 40..pvd_off + 47].copy_from_slice(b"TESTISO");
        img[pvd_off + 80..pvd_off + 84].copy_from_slice(&total.to_le_bytes());
        img[pvd_off + 84..pvd_off + 88].copy_from_slice(&total.to_be_bytes());
        img[pvd_off + 120..pvd_off + 122].copy_from_slice(&2048u16.to_le_bytes());
        img[pvd_off + 122..pvd_off + 124].copy_from_slice(&2048u16.to_be_bytes());
        let root = dir_record(18, COOKED_SECTOR, 2, &[0]);
        img[pvd_off + 156..pvd_off + 156 + root.len()].copy_from_slice(&root);

        let term = 17 * COOKED_SECTOR as usize;
        img[term] = 255;
        img[term + 1..term + 6].copy_from_slice(b"CD001");
        img[term + 6] = 1;

        let mut dir = Vec::new();
        dir.extend(dir_record(18, COOKED_SECTOR, 2, &[0]));
        dir.extend(dir_record(18, COOKED_SECTOR, 2, &[1]));
        let iso_name = format!("{file_name};1");
        dir.extend(dir_record(
            file_lba,
            file_data.len() as u32,
            0,
            iso_name.as_bytes(),
        ));
        img[18 * COOKED_SECTOR as usize..18 * COOKED_SECTOR as usize + dir.len()]
            .copy_from_slice(&dir);

        let dest = file_lba as usize * COOKED_SECTOR as usize;
        img[dest..dest + file_data.len()].copy_from_slice(file_data);
        img
    }

    fn raw_mode1(cooked: &[u8]) -> Vec<u8> {
        assert_eq!(cooked.len() % COOKED_SECTOR as usize, 0);
        let n = cooked.len() / COOKED_SECTOR as usize;
        let mut out = vec![0u8; n * RAW_SECTOR as usize];
        for i in 0..n {
            let sec = &mut out[i * RAW_SECTOR as usize..(i + 1) * RAW_SECTOR as usize];
            sec[..12].copy_from_slice(&SYNC);
            sec[15] = 1;
            sec[16..16 + COOKED_SECTOR as usize].copy_from_slice(
                &cooked[i * COOKED_SECTOR as usize..(i + 1) * COOKED_SECTOR as usize],
            );
        }
        out
    }

    fn grp_bytes() -> Vec<u8> {
        let mut g = b"KenSilverman".to_vec();
        g.extend_from_slice(&1u32.to_le_bytes());
        let mut entry = [0u8; 16];
        entry[..4].copy_from_slice(b"TEST");
        entry[12..16].copy_from_slice(&4u32.to_le_bytes());
        g.extend_from_slice(&entry);
        g.extend_from_slice(b"DATA");
        g
    }

    #[test]
    fn cooked_volume_lists_and_extracts_grp() {
        let grp = grp_bytes();
        let img = cooked_iso("DUKE3D.GRP", &grp);
        let layout = detect_layout(&img).unwrap();
        assert_eq!(layout, IsoLayout::COOKED);
        let vol = parse_pvd(&img, layout).unwrap();
        assert_eq!(vol.root_lba, 18);
        let dir = read_extent(&img, layout, vol.root_lba, vol.root_size).unwrap();
        let ents = parse_directory(&dir);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].name, "DUKE3D.GRP");
        assert!(!ents[0].is_dir);
        let got = read_extent(&img, layout, ents[0].lba, ents[0].size).unwrap();
        assert_eq!(got, grp);
    }

    #[test]
    fn raw_2352_pvd_is_not_at_cooked_offset() {
        let grp = grp_bytes();
        let raw = raw_mode1(&cooked_iso("DUKE3D.GRP", &grp));
        assert!(raw.len() > 32774);
        assert_ne!(&raw[32769..32774], b"CD001");
        assert_eq!(&raw[16 * 2352 + 17..16 * 2352 + 22], b"CD001");
        let layout = detect_layout(&raw).expect("raw pvd");
        assert_eq!(layout, IsoLayout::RAW_MODE1);
        let vol = parse_pvd(&raw, layout).unwrap();
        let dir = read_extent(&raw, layout, vol.root_lba, vol.root_size).unwrap();
        let ents = parse_directory(&dir);
        assert_eq!(ents[0].name, "DUKE3D.GRP");
        let got = read_extent(&raw, layout, ents[0].lba, ents[0].size).unwrap();
        assert_eq!(got, grp);
        assert!(got.starts_with(b"KenSilverman"));
    }

    #[test]
    fn extract_keep_writes_only_grp() {
        let grp = grp_bytes();
        let raw = raw_mode1(&cooked_iso("DUKE3D.GRP", &grp));
        let dir = std::env::temp_dir().join(format!("iso9660-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let names = extract_keep_from_image(&raw, &dir, iso_keep_name).unwrap();
        assert_eq!(names, ["DUKE3D.GRP"]);
        assert_eq!(std::fs::read(dir.join("DUKE3D.GRP")).unwrap(), grp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_name_strips_iso_version() {
        assert_eq!(iso_file_name(b"DUKE3D.GRP;1").as_deref(), Some("DUKE3D.GRP"));
        assert_eq!(iso_file_name(b"DUKE3D").as_deref(), Some("DUKE3D"));
    }

    #[test]
    fn file_name_rejects_path_escapes() {
        assert_eq!(iso_file_name(b"../../EVIL.GRP;1"), None);
        assert_eq!(iso_file_name(b"/ETC/PASSWD;1"), None);
        assert_eq!(iso_file_name(b"A/B.GRP"), None);
        assert_eq!(iso_file_name(b"A\\B.GRP"), None);
        assert_eq!(iso_file_name(b"C:EVIL.GRP"), None);
    }

    #[test]
    fn raw_range_covers_sync_and_data() {
        let (start, end) = IsoLayout::RAW_MODE1.extent_raw_range(757, 10_442_980);
        assert_eq!(start, 757 * 2352);
        assert_eq!((end - start) % 2352, 0);
        assert!(end - start > 10_442_980);
    }
}
