//! The file-tree payload of a store push (devices: no checkout, no git on the receiving side).
//! The master computes it from git (the files that differ between the device's rev and the new
//! one, or the whole tree under the device's prefixes on a first push); the device reads it with
//! nothing but this module.
//!
//! Format: `MKSF1`, then entries: op (1 = write, 2 = delete), path length (u16 BE), the path
//! (UTF-8, relative, `/`-separated), data length (u32 BE), the data (empty for a delete).

pub const MAGIC: &[u8; 5] = b"MKSF1";
const MAX_FILE: usize = 64 << 20;

/// One change: the file's new bytes, or `None` for a delete.
pub type Change = (String, Option<Vec<u8>>);

/// A path a payload may carry: relative, no `..`, no backslashes, control characters or drive
/// letters, nothing a store could be steered outside its root with.
pub fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && !p.chars().any(|c| c.is_control())
        && p.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

pub fn encode(changes: &[Change]) -> Result<Vec<u8>, String> {
    let mut out = MAGIC.to_vec();
    for (path, data) in changes {
        if !valid_path(path) {
            return Err(format!("not a store path: {path}"));
        }
        let len = u16::try_from(path.len()).map_err(|_| "path too long")?;
        out.push(if data.is_some() { 1 } else { 2 });
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(path.as_bytes());
        let d = data.as_deref().unwrap_or(&[]);
        if d.len() > MAX_FILE {
            return Err(format!("{path}: larger than {} MB", MAX_FILE >> 20));
        }
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        out.extend_from_slice(d);
    }
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Vec<Change>, String> {
    let mut rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or("not a file-tree payload")?;
    let mut take = |n: usize| -> Result<&[u8], String> {
        if rest.len() < n {
            return Err("the payload is cut short".into());
        }
        let (a, b) = rest.split_at(n);
        rest = b;
        Ok(a)
    };
    let mut out = Vec::new();
    loop {
        let op = match take(1) {
            Ok(op) => op[0],
            Err(_) => break,
        };
        let plen = u16::from_be_bytes(take(2)?.try_into().unwrap()) as usize;
        let path = std::str::from_utf8(take(plen)?).map_err(|_| "a path is not UTF-8")?.to_string();
        if !valid_path(&path) {
            return Err(format!("not a store path: {}", crate::proto::clean(&path, 200)));
        }
        let dlen = u32::from_be_bytes(take(4)?.try_into().unwrap()) as usize;
        if dlen > MAX_FILE {
            return Err(format!("{path}: larger than {} MB", MAX_FILE >> 20));
        }
        let data = take(dlen)?.to_vec();
        out.push(match op {
            1 => (path, Some(data)),
            2 if dlen == 0 => (path, None),
            _ => return Err("a bad entry".into()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_hostile_paths() {
        let c: Vec<Change> = vec![("src/a.rs".into(), Some(b"fn a() {}".to_vec())), ("old.rs".into(), None), ("e.bin".into(), Some(Vec::new()))];
        assert_eq!(decode(&encode(&c).unwrap()).unwrap(), c);
        for bad in ["/etc/passwd", "../x", "a/../../x", "a//b", "a\\b", "C:/x", "a/./b", "", "a\nb"] {
            assert!(!valid_path(bad), "{bad:?}");
            assert!(encode(&[(bad.to_string(), None)]).is_err());
        }
        let mut evil = MAGIC.to_vec();
        evil.push(1);
        evil.extend_from_slice(&5u16.to_be_bytes());
        evil.extend_from_slice(b"../xx");
        evil.extend_from_slice(&0u32.to_be_bytes());
        assert!(decode(&evil).is_err());
        let full = encode(&c).unwrap();
        assert!(decode(&full[..full.len() - 3]).is_err());
    }
}
