//! "Fast start" for an MP4: the movie's index (`moov`) moved in front of its media (`mdat`), the
//! layout messaging apps and web players expect (they read the index first and can start on a
//! partial file). AVAssetWriter writes the index last; this moves it and shifts the chunk offsets
//! (`stco` / `co64`) of every track by the index's size.

use std::path::Path;

fn be32(b: &[u8], at: usize) -> u64 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as u64
}

/// The top-level boxes of `b`: (type, start, size).
fn boxes(b: &[u8], from: usize, to: usize) -> Result<Vec<([u8; 4], usize, usize)>, String> {
    let mut out = Vec::new();
    let mut at = from;
    while at + 8 <= to {
        let mut size = be32(b, at) as usize;
        let kind: [u8; 4] = b[at + 4..at + 8].try_into().unwrap();
        if size == 1 {
            if at + 16 > to {
                return Err("truncated box".into());
            }
            size = u64::from_be_bytes(b[at + 8..at + 16].try_into().unwrap()) as usize;
        } else if size == 0 {
            size = to - at;
        }
        if size < 8 || at + size > to {
            return Err(format!("bad box {} at {at}", String::from_utf8_lossy(&kind)));
        }
        out.push((kind, at, size));
        at += size;
    }
    Ok(out)
}

/// Add `shift` to every chunk offset under the box at `from..to` (a container's children).
fn shift_offsets(b: &mut [u8], from: usize, to: usize, shift: u64) -> Result<(), String> {
    for (kind, at, size) in boxes(b, from, to)? {
        match &kind {
            b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" => shift_offsets(b, at + 8, at + size, shift)?,
            b"stco" => {
                let n = be32(b, at + 12) as usize;
                for i in 0..n {
                    let p = at + 16 + i * 4;
                    let v = be32(b, p) + shift;
                    let v = u32::try_from(v).map_err(|_| "chunk offset past 4 GB".to_string())?;
                    b[p..p + 4].copy_from_slice(&v.to_be_bytes());
                }
            }
            b"co64" => {
                let n = be32(b, at + 12) as usize;
                for i in 0..n {
                    let p = at + 16 + i * 8;
                    let v = u64::from_be_bytes(b[p..p + 8].try_into().unwrap()) + shift;
                    b[p..p + 8].copy_from_slice(&v.to_be_bytes());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The file at `path` with its `moov` in front of its `mdat` (rewritten in place through a
/// temporary file); a file already laid out so is left alone.
pub fn faststart(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let top = boxes(&bytes, 0, bytes.len())?;
    let moov = top.iter().position(|b| &b.0 == b"moov").ok_or("no moov")?;
    let mdat = top.iter().position(|b| &b.0 == b"mdat").ok_or("no mdat")?;
    if moov < mdat {
        return Ok(());
    }
    let (_, m_at, m_size) = top[moov];
    let mut index = bytes[m_at..m_at + m_size].to_vec();
    shift_offsets(&mut index, 8, m_size, m_size as u64)?;
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &(kind, at, size)) in top.iter().enumerate() {
        if i == moov {
            continue;
        }
        if &kind == b"mdat" && i == mdat {
            out.extend_from_slice(&index);
        }
        out.extend_from_slice(&bytes[at..at + size]);
    }
    let tmp = path.with_extension("faststart");
    std::fs::write(&tmp, &out).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
}
