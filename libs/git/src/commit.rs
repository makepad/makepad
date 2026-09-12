use crate::error::GitError;
use crate::oid::ObjectId;

#[derive(Debug, Clone)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub timestamp: i64,
    pub tz_offset: String, // e.g. "+0100", "-0500"
}

impl Signature {
    pub fn serialize(&self) -> String {
        format!(
            "{} <{}> {} {}",
            self.name, self.email, self.timestamp, self.tz_offset
        )
    }
}

fn parse_signature(s: &str) -> Result<Signature, GitError> {
    // Format: "Name <email> timestamp tz"
    let lt = s
        .find('<')
        .ok_or_else(|| GitError::InvalidObject("signature: no '<'".into()))?;
    let gt = s
        .find('>')
        .ok_or_else(|| GitError::InvalidObject("signature: no '>'".into()))?;

    let name = s[..lt].trim().to_string();
    let email = s[lt + 1..gt].to_string();
    let rest = s[gt + 1..].trim();

    let mut parts = rest.splitn(2, ' ');
    let timestamp: i64 = parts
        .next()
        .ok_or_else(|| GitError::InvalidObject("signature: no timestamp".into()))?
        .parse()
        .map_err(|_| GitError::InvalidObject("signature: invalid timestamp".into()))?;
    let tz_offset = parts
        .next()
        .ok_or_else(|| GitError::InvalidObject("signature: no tz".into()))?
        .to_string();

    Ok(Signature {
        name,
        email,
        timestamp,
        tz_offset,
    })
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    pub message: String,
}

impl Commit {
    /// The actual first parent, independent of history display order.
    pub fn first_parent(&self) -> Option<ObjectId> { self.parents.first().copied() }

    /// Parse colon-separated trailers in the final paragraph, preserving key
    /// spelling, order and duplicates and unfolding continuations with spaces.
    /// A blank line must separate the block from the subject/body. Implements
    /// Git's unconfigured block rule (all trailers, or >=25% with a Git-generated
    /// signoff/cherry-pick line). Default '#' comment lines are ignored.
    /// Repository trailer configuration and patch suffixes are not consulted;
    /// this API deliberately requires the block to be the last paragraph.
    pub fn trailers(&self) -> Vec<(String, String)> {
        let message = self.message.trim_end_matches(|c: char| c.is_ascii_whitespace());
        let mut start = None;
        let mut offset = 0;
        for line in message.split_inclusive('\n') {
            offset += line.len();
            if line.trim().is_empty() { start = Some(offset); }
        }
        let Some(start) = start else { return Vec::new(); };
        let mut trailers: Vec<(String, String)> = Vec::new();
        let mut non_trailers = 0usize;
        let mut continuation = false;
        let mut generated = false;
        let mut trailer_lines = 0usize;
        for line in message[start..].lines() {
            if line.starts_with('#') { continuation = false; continue; }
            if line.starts_with(|c: char| c.is_ascii_whitespace()) && continuation {
                let value = &mut trailers.last_mut().unwrap().1;
                if !value.is_empty() { value.push(' '); }
                value.push_str(line.trim());
                continue;
            }
            let parsed = line.split_once(':').and_then(|(key, value)| {
                let key = key.trim_end_matches([' ', '\t']);
                if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') { return None; }
                Some((key, value.trim()))
            });
            if line.starts_with("(cherry picked from commit ") {
                generated = true;
                trailer_lines += 1;
                continuation = false;
                continue;
            }
            if let Some((key, value)) = parsed {
                generated |= line.starts_with("Signed-off-by: ");
                trailer_lines += 1;
                trailers.push((key.to_string(), value.to_string()));
                continuation = true;
            } else {
                non_trailers += 1;
                continuation = false;
            }
        }
        if non_trailers == 0 || (generated && trailer_lines.saturating_mul(3) >= non_trailers) { trailers }
        else { Vec::new() }
    }
}

/// Parse the text content of a commit object.
pub fn parse_commit(data: &[u8]) -> Result<Commit, GitError> {
    let text = std::str::from_utf8(data)
        .map_err(|_| GitError::InvalidObject("commit: invalid UTF-8".into()))?;

    // Split at the blank line separating headers from message
    let (headers, message) = match text.find("\n\n") {
        Some(pos) => (&text[..pos], text[pos + 2..].to_string()),
        None => (text, String::new()),
    };

    let mut tree = None;
    let mut parents = Vec::new();
    let mut author = None;
    let mut committer = None;

    for line in headers.lines() {
        if let Some(rest) = line.strip_prefix("tree ") {
            tree = Some(ObjectId::from_hex(rest.trim())?);
        } else if let Some(rest) = line.strip_prefix("parent ") {
            parents.push(ObjectId::from_hex(rest.trim())?);
        } else if let Some(rest) = line.strip_prefix("author ") {
            author = Some(parse_signature(rest)?);
        } else if let Some(rest) = line.strip_prefix("committer ") {
            committer = Some(parse_signature(rest)?);
        }
        // Skip gpgsig and other headers we don't need
    }

    let tree = tree.ok_or_else(|| GitError::InvalidObject("commit: no tree".into()))?;
    let author = author.ok_or_else(|| GitError::InvalidObject("commit: no author".into()))?;
    let committer =
        committer.ok_or_else(|| GitError::InvalidObject("commit: no committer".into()))?;

    Ok(Commit {
        tree,
        parents,
        author,
        committer,
        message,
    })
}

/// Serialize a commit to the text format git expects.
pub fn serialize_commit(commit: &Commit) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!("tree {}\n", commit.tree.to_hex()));
    for parent in &commit.parents {
        s.push_str(&format!("parent {}\n", parent.to_hex()));
    }
    s.push_str(&format!("author {}\n", commit.author.serialize()));
    s.push_str(&format!("committer {}\n", commit.committer.serialize()));
    s.push('\n');
    s.push_str(&commit.message);
    s.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_trailer_block_and_first_parent() {
        let signature = Signature { name: "Test".into(), email: "test@example.com".into(), timestamp: 0, tz_offset: "+0000".into() };
        let mut commit = Commit { tree: ObjectId::ZERO, parents: vec![], author: signature.clone(), committer: signature, message: String::new() };
        assert_eq!(commit.first_parent(), None);
        commit.parents = vec![ObjectId::from_bytes([1; 20]), ObjectId::from_bytes([2; 20])];
        assert_eq!(commit.first_parent(), Some(ObjectId::from_bytes([1; 20])));
        commit.message = "Subject\r\n\r\nBody paragraph.\r\n\r\nCo-Authored-By: Claude Fable 5.1\r\n <noreply@anthropic.com>\r\nCo-Authored-By:\tExample <e@example.com>\r\nStudio-Local-Checkpoint: true\r\nDetails \t: first\r\n\tsecond\r\nEmpty:\r\n \t\r\n".into();
        assert_eq!(commit.trailers(), vec![
            ("Co-Authored-By".into(), "Claude Fable 5.1 <noreply@anthropic.com>".into()),
            ("Co-Authored-By".into(), "Example <e@example.com>".into()),
            ("Studio-Local-Checkpoint".into(), "true".into()),
            ("Details".into(), "first second".into()),
            ("Empty".into(), "".into()),
        ]);
        for message in [
            "", "Co-Authored-By: A", "Subject\nCo-Authored-By: A\n",
            "Subject\n\nCo-Authored-By: A\n\nLater prose\n",
            "Subject\n\nprose\nCo-Authored-By: A\n",
            "Subject\n\n Bad-Key: A\n", "Subject\n\nBad Key: A\n",
            "Subject\n\nKey=value\n", "Subject\n\n: value\n",
            "Subject\n\nCo-Authored-By: A\n---\npatch text\n",
        ] {
            commit.message = message.into();
            assert!(commit.trailers().is_empty(), "{message:?}");
        }
        commit.message = "Subject\n\none\ntwo\nthree\nSigned-off-by: A\n".into();
        assert_eq!(commit.trailers(), [("Signed-off-by".into(), "A".into())]);
        commit.message = "Subject\n\none\ntwo\nthree\nfour\nSigned-off-by: A\n".into();
        assert!(commit.trailers().is_empty());
        commit.message = "Subject\n\nprose\n(cherry picked from commit abc123)\n# comment\nCo-Authored-By: A\n".into();
        assert_eq!(commit.trailers(), [("Co-Authored-By".into(), "A".into())]);
        commit.message = "Subject\n\nprose\nsigned-off-by: A\n".into();
        assert!(commit.trailers().is_empty());
    }

    #[test]
    fn test_parse_commit() {
        let data = b"tree 3153326ac808d33c73f600ad1701501d7770355d\n\
parent 0f60d4fc088e59c246bd3fadff73454cf236b472\n\
author Admin <info@makepad.nl> 1770803379 +0100\n\
committer Admin <info@makepad.nl> 1770803379 +0100\n\
\n\
second commit\n";

        let commit = parse_commit(data).unwrap();
        assert_eq!(
            commit.tree.to_hex(),
            "3153326ac808d33c73f600ad1701501d7770355d"
        );
        assert_eq!(commit.parents.len(), 1);
        assert_eq!(
            commit.parents[0].to_hex(),
            "0f60d4fc088e59c246bd3fadff73454cf236b472"
        );
        assert_eq!(commit.author.name, "Admin");
        assert_eq!(commit.author.email, "info@makepad.nl");
        assert_eq!(commit.author.timestamp, 1770803379);
        assert_eq!(commit.author.tz_offset, "+0100");
        assert_eq!(commit.message, "second commit\n");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let commit = Commit {
            tree: ObjectId::from_hex("3153326ac808d33c73f600ad1701501d7770355d").unwrap(),
            parents: vec![ObjectId::from_hex("0f60d4fc088e59c246bd3fadff73454cf236b472").unwrap()],
            author: Signature {
                name: "Test User".into(),
                email: "test@example.com".into(),
                timestamp: 1700000000,
                tz_offset: "+0000".into(),
            },
            committer: Signature {
                name: "Test User".into(),
                email: "test@example.com".into(),
                timestamp: 1700000000,
                tz_offset: "+0000".into(),
            },
            message: "test commit\n".into(),
        };

        let data = serialize_commit(&commit);
        let parsed = parse_commit(&data).unwrap();
        assert_eq!(parsed.tree, commit.tree);
        assert_eq!(parsed.parents.len(), 1);
        assert_eq!(parsed.author.name, "Test User");
        assert_eq!(parsed.message, "test commit\n");
    }
}
