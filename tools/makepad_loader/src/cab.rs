//! Microsoft Cabinet (MSZIP + LZX) unpacker.

use makepad_fast_inflate::{deflate_decompress_from, DecompressError};

use crate::lzx;

pub fn list(data: &[u8]) -> Result<Vec<String>, String> {
    Ok(parse(data)?.files.into_iter().map(|f| f.name).collect())
}

pub fn extract(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let parsed = parse(data)?;
    let mut folder_data = Vec::with_capacity(parsed.folders.len());
    for folder in &parsed.folders {
        folder_data.push(decompress_folder(data, folder)?);
    }
    let mut out = Vec::new();
    for f in parsed.files {
        let buf = folder_data
            .get(f.folder)
            .ok_or_else(|| format!("cab folder {} missing", f.folder))?;
        let end = f
            .offset
            .checked_add(f.size)
            .ok_or("cab file overflow")?;
        if end > buf.len() {
            return Err(format!(
                "cab file {} out of range ({}+{} > {})",
                f.name,
                f.offset,
                f.size,
                buf.len()
            ));
        }
        out.push((f.name, buf[f.offset..end].to_vec()));
    }
    Ok(out)
}

fn parse(data: &[u8]) -> Result<Parsed, String> {
    if data.len() < 36 || &data[0..4] != b"MSCF" {
        return Err("not a cabinet".into());
    }
    let coff_files = u32_le(data, 16)?;
    let nfolders = u16_le(data, 26)?;
    let nfiles = u16_le(data, 28)?;
    let flags = u16_le(data, 30)?;
    // 0x1 prev, 0x2 next, 0x4 reserve-present (NOT spanning).
    let mut off = 36usize;
    let mut cb_folder = 0usize;
    let mut cb_data = 0usize;
    if flags & 0x0004 != 0 {
        let cb_header = u16_le(data, off)? as usize;
        cb_folder = *data.get(off + 2).ok_or("cab truncated")? as usize;
        cb_data = *data.get(off + 3).ok_or("cab truncated")? as usize;
        off = off
            .checked_add(4)
            .and_then(|o| o.checked_add(cb_header))
            .ok_or("cab reserve overflow")?;
    }
    if flags & 0x0001 != 0 {
        skip_cstring(data, &mut off)?;
        skip_cstring(data, &mut off)?;
    }
    if flags & 0x0002 != 0 {
        skip_cstring(data, &mut off)?;
        skip_cstring(data, &mut off)?;
    }
    let mut folders = Vec::new();
    for _ in 0..nfolders {
        let coff = u32_le(data, off)?;
        let ndata = u16_le(data, off + 4)?;
        let tcomp = u16_le(data, off + 6)?;
        folders.push(Folder {
            data_off: coff as usize,
            blocks: ndata as usize,
            tcomp,
            cb_data,
        });
        off = off
            .checked_add(8 + cb_folder)
            .ok_or("cab folder overflow")?;
    }
    let mut files = Vec::new();
    let mut foff = coff_files as usize;
    for _ in 0..nfiles {
        let usize_ = u32_le(data, foff)?;
        let uoff = u32_le(data, foff + 4)?;
        let ifold = u16_le(data, foff + 8)?;
        let name = read_cstring(data, foff + 16)?;
        files.push(CabFile {
            name,
            size: usize_ as usize,
            offset: uoff as usize,
            folder: ifold as usize,
        });
        foff += 16 + files.last().unwrap().name.len() + 1;
    }
    Ok(Parsed { folders, files })
}

fn skip_cstring(data: &[u8], off: &mut usize) -> Result<(), String> {
    while *off < data.len() && data[*off] != 0 {
        *off += 1;
    }
    if *off >= data.len() {
        return Err("cab name unterminated".into());
    }
    *off += 1;
    Ok(())
}

struct Parsed {
    folders: Vec<Folder>,
    files: Vec<CabFile>,
}

struct Folder {
    data_off: usize,
    blocks: usize,
    tcomp: u16,
    cb_data: usize,
}

struct CabFile {
    name: String,
    size: usize,
    offset: usize,
    folder: usize,
}

fn decompress_folder(data: &[u8], folder: &Folder) -> Result<Vec<u8>, String> {
    let method = folder.tcomp & 0x000f;
    let mut off = folder.data_off;
    let mut raw = Vec::new();
    let mut lzx_state = if method == 3 {
        let window_bits = ((folder.tcomp >> 8) & 0x1f).max(15);
        Some(lzx::Decoder::new(window_bits as u32)?)
    } else {
        None
    };
    for _ in 0..folder.blocks {
        if off + 8 > data.len() {
            return Err("cab data truncated".into());
        }
        let csize = u16_le(data, off + 4)? as usize;
        let usize_ = u16_le(data, off + 6)? as usize;
        off += 8 + folder.cb_data;
        let chunk = data.get(off..off + csize).ok_or("cab chunk truncated")?;
        off += csize;
        match method {
            0 => raw.extend_from_slice(chunk),
            1 => {
                if chunk.len() < 2 || chunk[0] != b'C' || chunk[1] != b'K' {
                    return Err("mszip magic missing".into());
                }
                let hist = raw.len().min(32768);
                let cap = if usize_ == 0 { 32768 } else { usize_ };
                let mut buf = vec![0u8; hist + cap];
                if hist > 0 {
                    buf[..hist].copy_from_slice(&raw[raw.len() - hist..]);
                }
                let written = loop {
                    match deflate_decompress_from(&chunk[2..], &mut buf, hist) {
                        Ok((_, written)) => break written,
                        Err(DecompressError::InsufficientSpace) => {
                            let n = buf.len().saturating_mul(2).max(hist + 1);
                            buf.resize(n, 0);
                            if hist > 0 {
                                buf[..hist].copy_from_slice(&raw[raw.len() - hist..]);
                            }
                            if buf.len() > 64 * 1024 * 1024 {
                                return Err("mszip block too large".into());
                            }
                        }
                        Err(e) => return Err(format!("mszip deflate: {e}")),
                    }
                };
                if usize_ != 0 && written != usize_ {
                    return Err(format!("mszip size {written} != {usize_}"));
                }
                raw.extend_from_slice(&buf[hist..hist + written]);
            }
            3 => {
                let dec = lzx_state
                    .as_mut()
                    .unwrap()
                    .decompress(chunk, usize_)?;
                raw.extend_from_slice(&dec);
            }
            other => return Err(format!("cab compression {other} unsupported")),
        }
    }
    Ok(raw)
}

fn u16_le(data: &[u8], off: usize) -> Result<u16, String> {
    let b = data.get(off..off + 2).ok_or("cab truncated")?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_le(data: &[u8], off: usize) -> Result<u32, String> {
    let b = data.get(off..off + 4).ok_or("cab truncated")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_cstring(data: &[u8], off: usize) -> Result<String, String> {
    let mut end = off;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    if end >= data.len() {
        return Err("cab name unterminated".into());
    }
    Ok(String::from_utf8_lossy(&data[off..end]).into_owned())
}
