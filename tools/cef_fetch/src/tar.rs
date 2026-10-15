//! Streaming CEF archive extraction: POSIX ustar/pax and GNU long names.
//! Only metadata extensions are buffered; file bodies flow directly to disk.
//! The destination must not be concurrently modified by another process.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn number(field: &[u8]) -> io::Result<u64> {
    if field[0] & 0x80 != 0 {
        if field[0] & 0x40 != 0 {
            return Err(invalid("negative tar number"));
        }
        return field[1..].iter().try_fold((field[0] & 0x7f) as u64, |n, b| {
            n.checked_mul(256).and_then(|n| n.checked_add(*b as u64))
                .ok_or_else(|| invalid("tar number overflow"))
        });
    }
    let mut digits = field.iter().copied().skip_while(|b| *b == b' ');
    let mut n = 0u64;
    while let Some(b) = digits.next() {
        if b == 0 || b == b' ' {
            if digits.any(|b| b != 0 && b != b' ') {
                return Err(invalid("invalid tar number terminator"));
            }
            break;
        }
        if !(b'0'..=b'7').contains(&b) {
            return Err(invalid("invalid octal tar number"));
        }
        n = n.checked_mul(8).and_then(|n| n.checked_add((b - b'0') as u64))
            .ok_or_else(|| invalid("tar number overflow"))?;
    }
    Ok(n)
}

fn cstring(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())]
}

fn path(bytes: &[u8]) -> io::Result<PathBuf> {
    if bytes.contains(&0) {
        return Err(invalid("NUL in tar path"));
    }
    #[cfg(unix)]
    let value = {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    };
    #[cfg(not(unix))]
    let value = PathBuf::from(std::str::from_utf8(bytes).map_err(|_| invalid("non-UTF8 tar path"))?);
    // Also reject Windows roots/drive paths on Unix, and backslash traversal.
    if bytes.starts_with(b"/") || bytes.starts_with(b"\\") || bytes.contains(&b':')
        || bytes.split(|b| *b == b'/' || *b == b'\\').any(|c| c == b"..")
        || value.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(invalid(format!("unsafe tar path: {}", value.display())));
    }
    Ok(value)
}

/// Reject all symlink components, including pre-existing ones and the leaf.
/// This deliberately never follows even an internal directory alias for writes.
fn checked(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut out = root.to_path_buf();
    for component in relative.components() {
        out.push(component);
        match fs::symlink_metadata(&out) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(invalid(format!("tar path traverses symlink: {}", out.display())));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn transfer(reader: &mut impl Read, size: u64, writer: &mut impl io::Write) -> io::Result<()> {
    if io::copy(&mut reader.take(size), writer)? != size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated tar entry"));
    }
    Ok(())
}

fn permissions(path: &Path, mode: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode((mode & 0o7777) as u32))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

#[derive(Default)]
struct Extended {
    path: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
    size: Option<u64>,
}

fn pax(mut data: &[u8], extended: &mut Extended) -> io::Result<()> {
    while !data.is_empty() {
        let space = data.iter().position(|b| *b == b' ').ok_or_else(|| invalid("invalid pax length"))?;
        let digits = &data[..space];
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(invalid("invalid pax length"));
        }
        let len: usize = std::str::from_utf8(digits).unwrap().parse().map_err(|_| invalid("pax length overflow"))?;
        if len <= space + 1 || len > data.len() || data[len - 1] != b'\n' {
            return Err(invalid("invalid pax record"));
        }
        let record = &data[space + 1..len - 1];
        let eq = record.iter().position(|b| *b == b'=').ok_or_else(|| invalid("invalid pax key"))?;
        let value = &record[eq + 1..];
        match &record[..eq] {
            b"path" => extended.path = (!value.is_empty()).then(|| value.to_vec()),
            b"linkpath" => extended.link = (!value.is_empty()).then(|| value.to_vec()),
            b"size" => {
                extended.size = if value.is_empty() { None } else {
                    if !value.iter().all(u8::is_ascii_digit) { return Err(invalid("invalid pax size")); }
                    Some(std::str::from_utf8(value).unwrap().parse().map_err(|_| invalid("pax size overflow"))?)
                };
            }
            _ => {}
        }
        data = &data[len..];
    }
    Ok(())
}

pub fn unpack(mut reader: impl Read, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    let root = destination.canonicalize()?;
    let mut extended = Extended::default();
    let mut directories = BTreeMap::new();
    loop {
        let mut header = [0u8; 512];
        reader.read_exact(&mut header)?;
        if header == [0; 512] {
            reader.read_exact(&mut header)?;
            if header != [0; 512] { return Err(invalid("tar requires two zero end blocks")); }
            break;
        }
        let sum: u64 = header.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u64 }).sum();
        if number(&header[148..156])? != sum { return Err(invalid("tar header checksum mismatch")); }
        let kind = header[156];
        let mut size = if !matches!(kind, b'L' | b'K' | b'x' | b'g') {
            match extended.size {
                Some(size) => size,
                None => number(&header[124..136])?,
            }
        } else {
            number(&header[124..136])?
        };
        if matches!(kind, b'L' | b'K' | b'x') {
            let mut data = Vec::new();
            transfer(&mut reader, size, &mut data)?;
            match kind {
                b'L' => extended.path = Some(cstring(&data).to_vec()),
                b'K' => extended.link = Some(cstring(&data).to_vec()),
                _ => pax(&data, &mut extended)?,
            }
        } else if kind == b'g' {
            transfer(&mut reader, size, &mut io::sink())?;
        } else {
            let meta = std::mem::take(&mut extended);
            size = meta.size.unwrap_or(size);
            let mut name = Vec::new();
            // Old GNU reuses the prefix region for timestamps and sparse data.
            if &header[257..263] == b"ustar\0" {
                let prefix = cstring(&header[345..500]);
                if !prefix.is_empty() { name.extend_from_slice(prefix); name.push(b'/'); }
            }
            name.extend_from_slice(cstring(&header[..100]));
            let relative = path(meta.path.as_deref().unwrap_or(&name))?;
            let out = checked(&root, &relative)?;
            let mode = number(&header[100..108])?;
            let result = (|| -> io::Result<()> {
                if matches!(kind, 0 | b'0' | b'1' | b'2' | b'5') {
                    if relative.as_os_str().is_empty() { return Err(invalid("empty tar entry name")); }
                    if let Some(parent) = out.parent() { fs::create_dir_all(parent)?; }
                }
                match kind {
                    0 | b'0' => {
                        // Unlink existing regular files so pre-existing hard links
                        // cannot cause writes to a file outside the destination.
                        if out.is_file() { fs::remove_file(&out)?; }
                        let mut file = File::options().write(true).create_new(true).open(&out)?;
                        transfer(&mut reader, size, &mut file)?;
                        permissions(&out, mode)?;
                        return Ok(());
                    }
                    b'5' => { fs::create_dir_all(&out)?; directories.insert(relative.clone(), mode); }
                    b'1' | b'2' => {
                        let target = path(meta.link.as_deref().unwrap_or(cstring(&header[157..257])))?;
                        if target.as_os_str().is_empty() { return Err(invalid("empty tar link target")); }
                        if kind == b'1' {
                            let source = checked(&root, &target)?;
                            if !source.is_file() { return Err(invalid("tar hard link target is not an existing file")); }
                            if source == out { return Err(invalid("tar hard link targets itself")); }
                            if out.is_file() { fs::remove_file(&out)?; }
                            let mut dest = File::options().write(true).create_new(true).open(&out)?;
                            io::copy(&mut File::open(source)?, &mut dest)?;
                            permissions(&out, mode)?;
                        } else {
                            #[cfg(unix)]
                            std::os::unix::fs::symlink(target, &out)?;
                            #[cfg(not(unix))]
                            return Err(invalid(format!("symlink unsupported on this platform: {}", relative.display())));
                        }
                    }
                    _ => {}
                }
                transfer(&mut reader, size, &mut io::sink())
            })();
            result.map_err(|e| io::Error::new(e.kind(), format!("tar entry {}: {e}", relative.display())))?;
        }
        transfer(&mut reader, (512 - size % 512) % 512, &mut io::sink())?;
    }
    // Children first: a read-only directory must not block extraction/chmod.
    for (relative, mode) in directories.iter().rev() {
        permissions(&checked(&root, relative)?, *mode)?;
    }
    Ok(())
}
