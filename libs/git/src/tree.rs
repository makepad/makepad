use crate::error::GitError;
use crate::oid::ObjectId;

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub mode: u32,
    pub name: String,
    pub oid: ObjectId,
}

impl TreeEntry {
    pub fn is_tree(&self) -> bool {
        self.mode == 0o040000
    }

    pub fn is_blob(&self) -> bool {
        (self.mode & 0o170000) == 0o100000
    }

    pub fn is_symlink(&self) -> bool {
        (self.mode & 0o170000) == 0o120000
    }

    pub fn is_gitlink(&self) -> bool {
        (self.mode & 0o170000) == 0o160000
    }
}

#[derive(Debug, Clone)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

/// Parse the binary content of a tree object.
///
/// Format: repeated entries of `<mode-ascii> <name>\0<20-byte-sha1>`
pub fn parse_tree(data: &[u8]) -> Result<Tree, GitError> {
    let mut entries = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Find the space separating mode from name
        let space_pos = data[pos..]
            .iter()
            .position(|&b| b == b' ')
            .ok_or_else(|| GitError::InvalidObject("tree entry: no space after mode".into()))?;
        let space_pos = pos + space_pos;

        // Parse mode (octal ASCII)
        let mode_str = std::str::from_utf8(&data[pos..space_pos])
            .map_err(|_| GitError::InvalidObject("tree entry: invalid mode".into()))?;
        let mode = u32::from_str_radix(mode_str, 8).map_err(|_| {
            GitError::InvalidObject(format!("tree entry: bad octal mode '{}'", mode_str))
        })?;

        // Find the null byte after the name
        let null_pos = data[space_pos + 1..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| GitError::InvalidObject("tree entry: no null after name".into()))?;
        let null_pos = space_pos + 1 + null_pos;

        let name = std::str::from_utf8(&data[space_pos + 1..null_pos])
            .map_err(|_| GitError::InvalidObject("tree entry: invalid UTF-8 name".into()))?
            .to_string();

        // Read 20-byte SHA-1 after the null
        let sha_start = null_pos + 1;
        let sha_end = sha_start + 20;
        if sha_end > data.len() {
            return Err(GitError::InvalidObject(
                "tree entry: truncated SHA-1".into(),
            ));
        }
        let oid = ObjectId::from_slice(&data[sha_start..sha_end])?;

        entries.push(TreeEntry { mode, name, oid });
        pos = sha_end;
    }

    Ok(Tree { entries })
}

/// Serialize a tree to the binary format git expects.
///
/// Entries must be sorted. Git sorts tree entries by name, with directories
/// having a trailing '/' appended for comparison purposes.
pub fn serialize_tree(tree: &Tree) -> Vec<u8> {
    let mut buf = Vec::new();
    // Entries should already be sorted, but let's sort to be safe
    let mut entries = tree.entries.clone();
    entries.sort_by(|a, b| {
        let a_name = if a.is_tree() {
            format!("{}/", a.name)
        } else {
            a.name.clone()
        };
        let b_name = if b.is_tree() {
            format!("{}/", b.name)
        } else {
            b.name.clone()
        };
        a_name.cmp(&b_name)
    });

    for entry in &entries {
        // Mode without leading zeros (git uses "40000" not "040000" for dirs)
        buf.extend_from_slice(format!("{:o}", entry.mode).as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(entry.name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(entry.oid.as_bytes());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_serialize_roundtrip() {
        let oid1 = ObjectId::from_hex("2015f8e40c38d86ca88808c6f031bb22544e92cf").unwrap();
        let oid2 = ObjectId::from_hex("d675fa44e50606caa705c3f48de02cf129c7f9a2").unwrap();
        let oid3 = ObjectId::from_hex("2a1384a888688f792564a8d2cc6a27f7ff1693a2").unwrap();

        let tree = Tree {
            entries: vec![
                TreeEntry {
                    mode: 0o100644,
                    name: "file1.txt".into(),
                    oid: oid1,
                },
                TreeEntry {
                    mode: 0o100644,
                    name: "file2.txt".into(),
                    oid: oid2,
                },
                TreeEntry {
                    mode: 0o040000,
                    name: "subdir".into(),
                    oid: oid3,
                },
            ],
        };

        let data = serialize_tree(&tree);
        let parsed = parse_tree(&data).unwrap();

        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.entries[0].name, "file1.txt");
        assert_eq!(parsed.entries[0].mode, 0o100644);
        assert_eq!(parsed.entries[0].oid, oid1);
        assert_eq!(parsed.entries[1].name, "file2.txt");
        assert_eq!(parsed.entries[2].name, "subdir");
        assert_eq!(parsed.entries[2].mode, 0o040000);
        assert!(parsed.entries[2].is_tree());
    }
}
