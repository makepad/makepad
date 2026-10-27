//! A tar archive read from memory (the dataset ships as a `.tar.gz`):
//! the regular files with their paths, ustar prefixes and GNU long names
//! included. Nothing is extracted to disk.

/// A `.tar.gz` decompressed whole (the gzip trailer gives its size; the
/// archive is 815 MB, past the decompressor's growing-buffer cap).
pub fn gunzip(gz: &[u8]) -> Result<Vec<u8>, String> {
    let size = gz.len().checked_sub(4).map(|at| u32::from_le_bytes([gz[at], gz[at + 1], gz[at + 2], gz[at + 3]]) as usize).ok_or("not a gzip file")?;
    let mut out = vec![0u8; size + 1];
    let (_, written) = makepad_fast_inflate::gzip_decompress(gz, &mut out).map_err(|e| format!("gzip: {e:?}"))?;
    out.truncate(written);
    Ok(out)
}

/// One regular file: its path in the archive and where its bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub offset: usize,
    pub len: usize,
}

fn octal(field: &[u8]) -> Option<usize> {
    let text: Vec<u8> = field.iter().copied().take_while(|b| *b != 0).filter(|b| *b != b' ').collect();
    if text.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(std::str::from_utf8(&text).ok()?, 8).ok()
}

fn name(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The archive's regular files, in archive order. A truncated or corrupt
/// archive gives the files before the damage.
pub fn entries(tar: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut long_name: Option<String> = None;
    while at + 512 <= tar.len() {
        let header = &tar[at..at + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let Some(size) = octal(&header[124..136]) else { break };
        let data = at + 512;
        if data + size > tar.len() {
            break;
        }
        let kind = header[156];
        match kind {
            // GNU long name: the next entry's path is this entry's data.
            b'L' => long_name = Some(name(&tar[data..data + size])),
            b'0' | 0 => {
                let path = long_name.take().unwrap_or_else(|| {
                    let prefix = if &header[257..262] == b"ustar" { name(&header[345..500]) } else { String::new() };
                    let base = name(&header[0..100]);
                    if prefix.is_empty() { base } else { format!("{prefix}/{base}") }
                });
                out.push(Entry { path, offset: data, len: size });
            }
            _ => long_name = None,
        }
        at = data + size.div_ceil(512) * 512;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(path: &str, size: usize, kind: u8) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..path.len()].copy_from_slice(path.as_bytes());
        let size = format!("{size:011o}\0");
        h[124..136].copy_from_slice(size.as_bytes());
        h[156] = kind;
        h[257..262].copy_from_slice(b"ustar");
        h
    }

    fn padded(data: &[u8]) -> Vec<u8> {
        let mut d = data.to_vec();
        d.resize(data.len().div_ceil(512) * 512, 0);
        d
    }

    #[test]
    fn files_long_names_and_directories() {
        let long = format!("clean_midi/{}/Song.mid", "A".repeat(120));
        let mut tar = header("clean_midi/", 0, b'5');
        tar.extend(header("clean_midi/Band/Hit.mid", 5, b'0'));
        tar.extend(padded(b"MThd!"));
        tar.extend(header("././@LongLink", long.len() + 1, b'L'));
        tar.extend(padded(format!("{long}\0").as_bytes()));
        tar.extend(header("cut", 3, b'0'));
        tar.extend(padded(b"abc"));
        tar.extend(vec![0u8; 1024]);
        let e = entries(&tar);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].path, "clean_midi/Band/Hit.mid");
        assert_eq!(&tar[e[0].offset..e[0].offset + e[0].len], b"MThd!");
        assert_eq!(e[1].path, long);
        assert_eq!(&tar[e[1].offset..e[1].offset + e[1].len], b"abc");
        // Truncated: the entries before the cut.
        assert_eq!(entries(&tar[..1300]).len(), 1);
    }
}
