//! Working-tree ignore rules. No subprocesses: Git's index supplies tracked
//! membership; our matcher applies repository excludes and per-directory rules.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Why ignore rules could not be read: an I/O failure or an unusable Git
/// index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IgnoreError {
    Io(String),
    Git(String),
}

impl std::fmt::Display for IgnoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IgnoreError::Io(e) | IgnoreError::Git(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for IgnoreError {}

impl From<std::io::Error> for IgnoreError {
    fn from(e: std::io::Error) -> Self {
        IgnoreError::Io(e.to_string())
    }
}

impl From<crate::GitError> for IgnoreError {
    fn from(e: crate::GitError) -> Self {
        IgnoreError::Git(e.to_string())
    }
}

/// Copy-on-write shared state: clones share the rule tables until one of
/// them mutates (a parallel walk merges its own tables back).
#[derive(Clone, Debug)]
struct Shared<T>(Arc<T>);
impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self(Arc::new(T::default()))
    }
}
impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(value))
    }
}
impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}
impl<T: Clone> Shared<T> {
    fn into_inner(self) -> T {
        Arc::try_unwrap(self.0).unwrap_or_else(|v| (*v).clone())
    }
}
impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&*self.0).into_iter()
    }
}
impl<T: Clone + IntoIterator> IntoIterator for Shared<T> {
    type Item = T::Item;
    type IntoIter = T::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.into_inner().into_iter()
    }
}

#[derive(Clone, Debug)]
struct Rule {
    pattern: Vec<u8>,
    negative: bool,
    directory: bool,
    anchored: bool,
}

impl Rule {
    fn parse(line: &[u8]) -> Option<Self> {
        let mut line = line.strip_suffix(b"\r").unwrap_or(line);
        while line.last() == Some(&b' ') {
            let escapes = line[..line.len() - 1]
                .iter()
                .rev()
                .take_while(|&&b| b == b'\\')
                .count();
            if escapes % 2 == 1 {
                break;
            }
            line = &line[..line.len() - 1];
        }
        if line.is_empty() || line[0] == b'#' {
            return None;
        }
        let negative = line[0] == b'!';
        if negative {
            line = &line[1..];
        }
        let directory = line.ends_with(b"/");
        if directory {
            line = &line[..line.len() - 1];
        }
        let anchored = line.contains(&b'/');
        if line.starts_with(b"/") {
            line = &line[1..];
        }
        if line.is_empty() {
            return None;
        }
        Some(Self {
            pattern: line.to_vec(),
            negative,
            directory,
            anchored,
        })
    }
    fn matches(&self, path: &str, directory: bool) -> bool {
        if self.directory && !directory {
            return false;
        }
        let path = if self.anchored {
            path
        } else {
            path.rsplit('/').next().unwrap_or(path)
        };
        wildmatch(&self.pattern, path.as_bytes())
    }
}

/// Byte-oriented FNM_PATHNAME matching, including Git's component-boundary **.
/// Dynamic programming bounds repeated stars without exponential backtracking.
fn wildmatch(pattern: &[u8], text: &[u8]) -> bool {
    fn run(p: &[u8], t: &[u8], i: usize, j: usize, memo: &mut [Option<bool>]) -> bool {
        let key = i * (t.len() + 1) + j;
        if let Some(value) = memo[key] {
            return value;
        }
        let value = if i == p.len() {
            j == t.len()
        } else {
            match p[i] {
                b'*' => {
                    let mut end = i + 1;
                    while p.get(end) == Some(&b'*') {
                        end += 1;
                    }
                    let double = end - i >= 2
                        && (i == 0 || p[i - 1] == b'/')
                        && (end == p.len() || p[end] == b'/');
                    if double && p.get(end) == Some(&b'/') {
                        run(p, t, end + 1, j, memo)
                            || (j..t.len()).any(|k| t[k] == b'/' && run(p, t, end + 1, k + 1, memo))
                    } else {
                        let limit = if double {
                            t.len()
                        } else {
                            j + t[j..]
                                .iter()
                                .position(|&b| b == b'/')
                                .unwrap_or(t.len() - j)
                        };
                        (j..=limit).any(|k| run(p, t, end, k, memo))
                    }
                }
                b'?' => j < t.len() && t[j] != b'/' && run(p, t, i + 1, j + 1, memo),
                b'\\' => p
                    .get(i + 1)
                    .is_some_and(|&b| t.get(j) == Some(&b) && run(p, t, i + 2, j + 1, memo)),
                b'[' => {
                    j < t.len()
                        && t[j] != b'/'
                        && bracket(p, i + 1, t[j])
                            .is_some_and(|(end, matched)| matched && run(p, t, end, j + 1, memo))
                }
                b => t.get(j) == Some(&b) && run(p, t, i + 1, j + 1, memo),
            }
        };
        memo[key] = Some(value);
        value
    }
    run(
        pattern,
        text,
        0,
        0,
        &mut vec![None; (pattern.len() + 1) * (text.len() + 1)],
    )
}

fn bracket(p: &[u8], mut i: usize, byte: u8) -> Option<(usize, bool)> {
    let negative = matches!(p.get(i), Some(b'!' | b'^'));
    if negative {
        i += 1;
    }
    let start = i;
    let mut matched = false;
    while i < p.len() {
        if p[i] == b']' && i > start {
            return Some((i + 1, matched != negative));
        }
        if p[i..].starts_with(b"[:") {
            let end = p[i + 2..].windows(2).position(|w| w == b":]")? + i + 2;
            matched |= match &p[i + 2..end] {
                b"alnum" => byte.is_ascii_alphanumeric(),
                b"alpha" => byte.is_ascii_alphabetic(),
                b"blank" => matches!(byte, b' ' | b'\t'),
                b"cntrl" => byte.is_ascii_control(),
                b"digit" => byte.is_ascii_digit(),
                b"graph" => byte.is_ascii_graphic(),
                b"lower" => byte.is_ascii_lowercase(),
                b"print" => byte.is_ascii_graphic() || byte == b' ',
                b"punct" => byte.is_ascii_punctuation(),
                b"space" => byte.is_ascii_whitespace(),
                b"upper" => byte.is_ascii_uppercase(),
                b"xdigit" => byte.is_ascii_hexdigit(),
                _ => return None,
            };
            i = end + 2;
            continue;
        }
        if p[i] == b'\\' {
            i += 1;
        }
        let first = *p.get(i)?;
        i += 1;
        if p.get(i) == Some(&b'-') && p.get(i + 1).is_some_and(|&b| b != b']') {
            i += 1;
            if p[i] == b'\\' {
                i += 1;
            }
            let last = *p.get(i)?;
            matched |= first <= byte && byte <= last;
            i += 1;
        } else {
            matched |= first == byte;
        }
    }
    None
}

/// Immutable rules retained by a frozen working-tree source for buffer overlays.
/// No filesystem access occurs when an overlay is applied.
#[derive(Clone, Debug)]
pub struct IgnoreRules {
    prefix: String,
    tracked: BTreeSet<String>,
    excludes: Vec<Rule>,
    rules: BTreeMap<String, Vec<Rule>>,
}
impl IgnoreRules {
    /// Retained allocation estimate, charged once per shared rule snapshot.
    pub fn byte_estimate(&self) -> usize {
        fn rules_bytes(rules: &Vec<Rule>) -> usize {
            rules.capacity() * std::mem::size_of::<Rule>()
                + rules
                    .iter()
                    .map(|rule| rule.pattern.capacity())
                    .sum::<usize>()
        }
        std::mem::size_of::<Self>()
            + self.prefix.capacity()
            + self
                .tracked
                .iter()
                .map(|path| path.capacity() + 64)
                .sum::<usize>()
            + rules_bytes(&self.excludes)
            + self
                .rules
                .iter()
                .map(|(path, rules)| path.capacity() + 96 + rules_bytes(rules))
                .sum::<usize>()
    }
    pub fn tracked(&self, path: &str) -> bool {
        self.tracked.contains(path)
    }
    pub fn ignored(&self, path: &str) -> bool {
        if self.tracked(path) {
            return false;
        }
        let path = if self.prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", self.prefix)
        };
        let ends: Vec<_> = path
            .match_indices('/')
            .map(|(i, _)| i)
            .chain([path.len()])
            .collect();
        for &end in &ends {
            let part = &path[..end];
            let directory = end != path.len();
            let mut ignored = false;
            for rule in &self.excludes {
                if rule.matches(part, directory) {
                    ignored = !rule.negative;
                }
            }
            for base in std::iter::once("").chain(part.match_indices('/').map(|(i, _)| &part[..i]))
            {
                let relative = if base.is_empty() {
                    part
                } else {
                    &part[base.len() + 1..]
                };
                if let Some(rules) = self.rules.get(base) {
                    for rule in rules {
                        if rule.matches(relative, directory) {
                            ignored = !rule.negative;
                        }
                    }
                }
            }
            if ignored {
                return true;
            }
        }
        false
    }
}

#[derive(Clone)]
pub struct GitIgnore {
    root: PathBuf,
    prefix: String,
    tracked: Shared<BTreeSet<String>>,
    excludes: Shared<Vec<Rule>>,
    rules: Shared<BTreeMap<String, Vec<Rule>>>,
    directories: Shared<BTreeMap<String, bool>>,
}

fn read_rules(path: &Path) -> Result<Vec<Rule>, IgnoreError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    // Git never follows a symlink when reading .gitignore.
    if !meta.is_file() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    Ok(bytes
        .split(|&b| b == b'\n')
        .filter_map(Rule::parse)
        .collect())
}

impl GitIgnore {
    pub fn new(root: &Path) -> Result<Self, IgnoreError> {
        let root = root.canonicalize()?;
        let mut at = root.clone();
        let repo = loop {
            if let Some(paths) = crate::repo::repository_paths(&at)? {
                break Some(paths);
            }
            if !at.pop() {
                break None;
            }
        };
        let mut tracked = BTreeSet::new();
        let mut excludes = Vec::new();
        let (root, prefix) = if let Some(repo) = repo {
            let prefix = root
                .strip_prefix(&repo.workdir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let index = repo.git_dir.join("index");
            match std::fs::read(&index) {
                Ok(bytes) => {
                    for path in index_paths(&bytes)? {
                        let relative = if prefix.is_empty() {
                            Some(path.as_str())
                        } else {
                            path.strip_prefix(&format!("{prefix}/"))
                        };
                        if let Some(path) = relative {
                            tracked.insert(path.to_owned());
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            excludes = read_rules(&repo.common_dir.join("info/exclude"))?;
            (repo.workdir, prefix)
        } else {
            (root, String::new())
        };
        Ok(Self {
            root,
            prefix,
            tracked: tracked.into(),
            excludes: excludes.into(),
            rules: Default::default(),
            directories: Default::default(),
        })
    }
    /// Every path the Git index lists (repository-relative, `/`-separated).
    pub fn tracked_paths(&self) -> &BTreeSet<String> {
        &self.tracked
    }
    /// Whether the Git index lists `path` (repository-relative, `/`-separated).
    pub fn is_tracked(&self, path: &str) -> bool {
        self.tracked.contains(path)
    }
    pub fn merge_walk(&mut self, other: Self) {
        self.rules.extend(other.rules);
        self.directories.extend(other.directories);
    }
    pub fn snapshot(&self) -> IgnoreRules {
        IgnoreRules {
            prefix: self.prefix.clone(),
            tracked: (*self.tracked).clone(),
            excludes: (*self.excludes).clone(),
            rules: (*self.rules).clone(),
        }
    }
    pub fn ignored(&mut self, path: &str, directory: bool) -> Result<bool, IgnoreError> {
        if !directory && self.tracked.contains(path) {
            return Ok(false);
        }
        let path = if self.prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", self.prefix)
        };
        self.ignored_repo(&path, directory)
    }
    fn ignored_repo(&mut self, path: &str, directory: bool) -> Result<bool, IgnoreError> {
        if directory {
            if let Some(&ignored) = self.directories.get(path) {
                return Ok(ignored);
            }
        }
        let parent = path.rsplit_once('/').map_or("", |(d, _)| d);
        let mut ignored = !parent.is_empty() && self.ignored_repo(parent, true)?;
        if !ignored {
            for rule in &self.excludes {
                if rule.matches(path, directory) {
                    ignored = !rule.negative;
                }
            }
            let mut bases = vec![""];
            bases.extend(path.match_indices('/').map(|(i, _)| &path[..i]));
            for base in bases {
                if !self.rules.contains_key(base) {
                    self.rules.insert(
                        base.to_owned(),
                        read_rules(&self.root.join(base).join(".gitignore"))?,
                    );
                }
                let relative = if base.is_empty() {
                    path
                } else {
                    &path[base.len() + 1..]
                };
                for rule in &self.rules[base] {
                    if rule.matches(relative, directory) {
                        ignored = !rule.negative;
                    }
                }
            }
        }
        if directory {
            self.directories.insert(path.to_owned(), ignored);
        }
        Ok(ignored)
    }
}

/// Read membership only, including unmerged stages, extended v3 flags and v4
/// prefix compression. The general Git reader currently only decodes v2/v3.
/// The tracked paths recorded in a Git index blob (`.git/index`).
pub fn index_paths(bytes: &[u8]) -> Result<BTreeSet<String>, IgnoreError> {
    let bad = || IgnoreError::Git("invalid or unsupported Git index for source inventory".into());
    if bytes.get(..4) != Some(b"DIRC") {
        return Err(bad());
    }
    let u32_at = |i: usize| -> Result<u32, IgnoreError> {
        Ok(u32::from_be_bytes(
            bytes.get(i..i + 4).ok_or_else(bad)?.try_into().unwrap(),
        ))
    };
    let version = u32_at(4)?;
    if !(2..=4).contains(&version) {
        return Err(bad());
    }
    let count = u32_at(8)?;
    let mut pos = 12;
    let mut previous = Vec::new();
    let mut paths = BTreeSet::new();
    for _ in 0..count {
        let start = pos;
        let flags = bytes.get(pos + 60..pos + 62).ok_or_else(bad)?;
        let extended = flags[0] & 0x40 != 0;
        pos += 62;
        if extended && version >= 3 {
            pos += 2;
        }
        let strip = if version == 4 {
            let mut byte = *bytes.get(pos).ok_or_else(bad)?;
            pos += 1;
            let mut value = (byte & 127) as usize;
            while byte & 128 != 0 {
                byte = *bytes.get(pos).ok_or_else(bad)?;
                pos += 1;
                value = value
                    .checked_add(1)
                    .and_then(|v| v.checked_mul(128))
                    .and_then(|v| v.checked_add((byte & 127) as usize))
                    .ok_or_else(bad)?;
            }
            value
        } else {
            previous.len()
        };
        let end = pos
            + bytes
                .get(pos..)
                .ok_or_else(bad)?
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(bad)?;
        previous.truncate(previous.len().checked_sub(strip).ok_or_else(bad)?);
        previous.extend_from_slice(&bytes[pos..end]);
        let path = std::str::from_utf8(&previous).map_err(|_| bad())?;
        if path.starts_with('/') || path.split('/').any(|s| matches!(s, "." | "..")) {
            return Err(bad());
        }
        paths.insert(path.to_owned());
        pos = if version == 4 {
            end + 1
        } else {
            start + ((end + 1 - start + 7) & !7)
        };
    }
    while pos + 8 <= bytes.len().saturating_sub(20) {
        let signature = &bytes[pos..pos + 4];
        // Never silently lose tracked membership for index extensions which
        // alter the entry set (split/sparse index). Report blocked coverage.
        if signature[0].is_ascii_lowercase() {
            return Err(IgnoreError::Git(format!(
                "unsupported Git index extension: {}",
                String::from_utf8_lossy(signature)
            )));
        }
        pos = pos
            .checked_add(8 + u32_at(pos + 4)? as usize)
            .ok_or_else(bad)?;
    }
    if pos != bytes.len().saturating_sub(20) {
        return Err(bad());
    }
    Ok(paths)
}

