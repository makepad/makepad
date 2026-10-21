//! std::path: Path (unsized newtype over OsStr, coord.md F-OS3) and PathBuf. The component
//! parser is real std's double-ended state machine, written once for both platforms over the
//! same hooks as real std (`is_sep_byte`, `is_verbatim_sep`, `parse_prefix`, `HAS_PREFIXES`):
//! on unix there are no prefixes and only '/' separates; on Windows both '/' and '\' separate
//! (only '\' after a verbatim `\\?\` prefix) and `C:`, `\\server\share`, `\\?\..`, `\\.\..`
//! prefixes are parsed (real std path.rs + sys/path/windows_prefix.rs, MIT/Apache-2.0).

use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::error::Error;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::ops::Deref;

use crate::ffi::{OsStr, OsString};
use crate::fs;
use crate::io;

#[cfg(unix)]
pub const MAIN_SEPARATOR: char = '/';
#[cfg(unix)]
pub const MAIN_SEPARATOR_STR: &str = "/";
#[cfg(windows)]
pub const MAIN_SEPARATOR: char = '\\';
#[cfg(windows)]
pub const MAIN_SEPARATOR_STR: &str = "\\";

#[cfg(unix)]
const HAS_PREFIXES: bool = false;
#[cfg(windows)]
const HAS_PREFIXES: bool = true;

pub const fn is_separator(c: char) -> bool {
    c.is_ascii() && is_sep_byte(c as u8)
}

#[cfg(unix)]
const fn is_sep_byte(b: u8) -> bool {
    b == b'/'
}

#[cfg(unix)]
const fn is_verbatim_sep(b: u8) -> bool {
    b == b'/'
}

/// unix paths have no prefix.
#[cfg(unix)]
fn parse_prefix(_path: &OsStr) -> Option<Prefix<'_>> {
    None
}

#[cfg(windows)]
const fn is_sep_byte(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

#[cfg(windows)]
const fn is_verbatim_sep(b: u8) -> bool {
    b == b'\\'
}

/// Windows prefix parser (real std sys/path/windows_prefix.rs): the first 8 bytes are matched
/// with '/' read as '\', except that a verbatim `\\?\` must be spelled with backslashes.
#[cfg(windows)]
fn parse_prefix(path: &OsStr) -> Option<Prefix<'_>> {
    let b = path.as_encoded_bytes();
    let n = if b.len() < 8 { b.len() } else { 8 };
    let mut norm = [0u8; 8];
    let mut i = 0;
    while i < n {
        norm[i] = if b[i] == b'/' { b'\\' } else { b[i] };
        i += 1;
    }
    let p = &norm[..n];
    if p.starts_with(b"\\\\") {
        if p[2..].starts_with(b"?\\") && !b[..4].contains(&b'/') {
            if p[4..].starts_with(b"UNC\\") {
                // \\?\UNC\server\share
                let (server, rest) = parse_next_component(&b[8..], true);
                let (share, _) = parse_next_component(rest, true);
                Some(Prefix::VerbatimUNC(OsStr::from_raw_bytes(server), OsStr::from_raw_bytes(share)))
            } else {
                let rest = &b[4..];
                // in verbatim paths only an exact drive prefix is recognized
                match parse_drive_exact(rest) {
                    Some(drive) => Some(Prefix::VerbatimDisk(drive)),
                    None => {
                        let (prefix, _) = parse_next_component(rest, true);
                        Some(Prefix::Verbatim(OsStr::from_raw_bytes(prefix)))
                    }
                }
            }
        } else if p[2..].starts_with(b".\\") {
            // \\.\COM42
            let (prefix, _) = parse_next_component(&b[4..], false);
            Some(Prefix::DeviceNS(OsStr::from_raw_bytes(prefix)))
        } else {
            let (server, rest) = parse_next_component(&b[2..], false);
            let (share, _) = parse_next_component(rest, false);
            if !server.is_empty() && !share.is_empty() {
                // \\server\share
                Some(Prefix::UNC(OsStr::from_raw_bytes(server), OsStr::from_raw_bytes(share)))
            } else {
                None
            }
        }
    } else {
        match parse_drive(b) {
            Some(drive) => Some(Prefix::Disk(drive)),
            None => None,
        }
    }
}

/// `C:` (any ASCII letter, upper-cased) at the start of `b`.
#[cfg(windows)]
fn parse_drive(b: &[u8]) -> Option<u8> {
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some(b[0].to_ascii_uppercase())
    } else {
        None
    }
}

/// Exactly `C:` (end of path or a separator follows).
#[cfg(windows)]
fn parse_drive_exact(b: &[u8]) -> Option<u8> {
    if b.len() <= 2 || is_sep_byte(b[2]) {
        parse_drive(b)
    } else {
        None
    }
}

/// The next component and the rest after its separator; in verbatim paths only '\' separates.
#[cfg(windows)]
fn parse_next_component(b: &[u8], verbatim: bool) -> (&[u8], &[u8]) {
    let mut i = 0;
    while i < b.len() {
        let sep = if verbatim { is_verbatim_sep(b[i]) } else { is_sep_byte(b[i]) };
        if sep {
            return (&b[..i], &b[i + 1..]);
        }
        i += 1;
    }
    (b, &b[b.len()..])
}

/// Windows path prefixes (real std path.rs); never produced on unix, where paths have none.
#[derive(Copy, Clone, Debug, Hash, PartialOrd, Ord, PartialEq, Eq)]
pub enum Prefix<'a> {
    Verbatim(&'a OsStr),
    VerbatimUNC(&'a OsStr, &'a OsStr),
    VerbatimDisk(u8),
    DeviceNS(&'a OsStr),
    UNC(&'a OsStr, &'a OsStr),
    Disk(u8),
}

impl<'a> Prefix<'a> {
    /// Byte length of the prefix as written.
    fn len(&self) -> usize {
        match *self {
            Prefix::Verbatim(x) => 4 + x.len(),
            Prefix::VerbatimUNC(x, y) => 8 + x.len() + if y.len() > 0 { 1 + y.len() } else { 0 },
            Prefix::VerbatimDisk(_) => 6,
            Prefix::UNC(x, y) => 2 + x.len() + if y.len() > 0 { 1 + y.len() } else { 0 },
            Prefix::DeviceNS(x) => 4 + x.len(),
            Prefix::Disk(_) => 2,
        }
    }
    pub fn is_verbatim(&self) -> bool {
        matches!(*self, Prefix::Verbatim(_) | Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(..))
    }
    fn is_drive(&self) -> bool {
        matches!(*self, Prefix::Disk(_))
    }
    fn has_implicit_root(&self) -> bool {
        !self.is_drive()
    }
}

/// A path's prefix component: its raw spelling plus the parsed kind. Equality, ordering and
/// hashing look at the parsed kind only (real std), so `C:` == `c:` and `\\a\b` == `//a/b`.
#[derive(Copy, Clone, Debug)]
pub struct PrefixComponent<'a> {
    raw: &'a OsStr,
    parsed: Prefix<'a>,
}

impl<'a> PrefixComponent<'a> {
    pub fn kind(&self) -> Prefix<'a> {
        self.parsed
    }
    pub fn as_os_str(&self) -> &'a OsStr {
        self.raw
    }
}

impl<'a> PartialEq for PrefixComponent<'a> {
    fn eq(&self, other: &PrefixComponent<'a>) -> bool {
        self.parsed == other.parsed
    }
}

impl<'a> Eq for PrefixComponent<'a> {}

impl<'a> PartialOrd for PrefixComponent<'a> {
    fn partial_cmp(&self, other: &PrefixComponent<'a>) -> Option<Ordering> {
        PartialOrd::partial_cmp(&self.parsed, &other.parsed)
    }
}

impl<'a> Ord for PrefixComponent<'a> {
    fn cmp(&self, other: &PrefixComponent<'a>) -> Ordering {
        Ord::cmp(&self.parsed, &other.parsed)
    }
}

impl<'a> Hash for PrefixComponent<'a> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.parsed.hash(h);
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Component<'a> {
    Prefix(PrefixComponent<'a>),
    RootDir,
    CurDir,
    ParentDir,
    Normal(&'a OsStr),
}

impl<'a> Component<'a> {
    pub fn as_os_str(self) -> &'a OsStr {
        match self {
            Component::Prefix(p) => p.as_os_str(),
            Component::RootDir => OsStr::new(MAIN_SEPARATOR_STR),
            Component::CurDir => OsStr::new("."),
            Component::ParentDir => OsStr::new(".."),
            Component::Normal(path) => path,
        }
    }
}

impl<'a> AsRef<OsStr> for Component<'a> {
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl<'a> AsRef<Path> for Component<'a> {
    fn as_ref(&self) -> &Path {
        Path::new(self.as_os_str())
    }
}

// front/back cursor states (Prefix first, as real std's State)
const PREFIX: u8 = 0;
const START_DIR: u8 = 1;
const BODY: u8 = 2;
const DONE: u8 = 3;

#[derive(Clone)]
pub struct Components<'a> {
    // the path left to parse components from
    path: &'a [u8],
    // the prefix as parsed from the original path (always None on unix)
    prefix: Option<Prefix<'a>>,
    // the path has a separator right after its prefix; most Windows prefixes also have an
    // implicit ("logical") root, e.g. \\server\share == \\server\share\
    has_physical_root: bool,
    front: u8,
    back: u8,
}

/// Whether the first byte after the prefix is a separator.
fn has_physical_root(s: &[u8], prefix: Option<Prefix<'_>>) -> bool {
    let path = match prefix {
        Some(p) => &s[p.len()..],
        None => s,
    };
    !path.is_empty() && is_sep_byte(path[0])
}

impl<'a> Components<'a> {
    fn prefix_len(&self) -> usize {
        if !HAS_PREFIXES {
            return 0;
        }
        match self.prefix {
            Some(p) => p.len(),
            None => 0,
        }
    }

    fn prefix_verbatim(&self) -> bool {
        if !HAS_PREFIXES {
            return false;
        }
        match self.prefix {
            Some(p) => p.is_verbatim(),
            None => false,
        }
    }

    /// How much of the prefix is left from the point of view of iteration.
    fn prefix_remaining(&self) -> usize {
        if !HAS_PREFIXES {
            return 0;
        }
        if self.front == PREFIX { self.prefix_len() } else { 0 }
    }

    fn len_before_body(&self) -> usize {
        let root = if self.front <= START_DIR && self.has_physical_root { 1 } else { 0 };
        let cur_dir = if self.front <= START_DIR && self.include_cur_dir() { 1 } else { 0 };
        self.prefix_remaining() + root + cur_dir
    }

    fn finished(&self) -> bool {
        self.front == DONE || self.back == DONE || self.front > self.back
    }

    fn is_sep_byte(&self, b: u8) -> bool {
        if self.prefix_verbatim() { is_verbatim_sep(b) } else { is_sep_byte(b) }
    }

    /// Is the *original* path rooted (physically, or implicitly by its prefix)?
    fn has_root(&self) -> bool {
        if self.has_physical_root {
            return true;
        }
        if HAS_PREFIXES {
            if let Some(p) = self.prefix {
                if p.has_implicit_root() {
                    return true;
                }
            }
        }
        false
    }

    /// Should the normalized path include a leading `.`?
    fn include_cur_dir(&self) -> bool {
        if self.has_root() {
            return false;
        }
        let p = &self.path[self.prefix_remaining()..];
        if p.is_empty() || p[0] != b'.' {
            return false;
        }
        p.len() == 1 || self.is_sep_byte(p[1])
    }

    fn parse_single_component<'b>(&self, comp: &'b [u8]) -> Option<Component<'b>> {
        if comp == b"." && HAS_PREFIXES && self.prefix_verbatim() {
            Some(Component::CurDir)
        } else if comp.is_empty() || comp == b"." {
            None
        } else if comp == b".." {
            Some(Component::ParentDir)
        } else {
            Some(Component::Normal(OsStr::from_raw_bytes(comp)))
        }
    }

    fn parse_next_component(&self) -> (usize, Option<Component<'a>>) {
        let mut i = 0;
        while i < self.path.len() && !self.is_sep_byte(self.path[i]) {
            i += 1;
        }
        let (extra, comp) = if i == self.path.len() { (0, self.path) } else { (1, &self.path[..i]) };
        (comp.len() + extra, self.parse_single_component(comp))
    }

    fn parse_next_component_back(&self) -> (usize, Option<Component<'a>>) {
        let start = self.len_before_body();
        let body = &self.path[start..];
        let mut i = body.len();
        let mut sep = None;
        while i > 0 {
            i -= 1;
            if self.is_sep_byte(body[i]) {
                sep = Some(i);
                break;
            }
        }
        let (extra, comp) = match sep {
            None => (0, body),
            Some(i) => (1, &body[i + 1..]),
        };
        (comp.len() + extra, self.parse_single_component(comp))
    }

    fn trim_left(&mut self) {
        while !self.path.is_empty() {
            let (size, comp) = self.parse_next_component();
            if comp.is_some() {
                return;
            }
            self.path = &self.path[size..];
        }
    }

    fn trim_right(&mut self) {
        while self.path.len() > self.len_before_body() {
            let (size, comp) = self.parse_next_component_back();
            if comp.is_some() {
                return;
            }
            self.path = &self.path[..self.path.len() - size];
        }
    }

    pub fn as_path(&self) -> &'a Path {
        let mut comps = self.clone();
        if comps.front == BODY {
            comps.trim_left();
        }
        if comps.back == BODY {
            comps.trim_right();
        }
        Path::from_bytes(comps.path)
    }
}

impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;
    fn next(&mut self) -> Option<Component<'a>> {
        while !self.finished() {
            if self.front == PREFIX {
                self.front = START_DIR;
                let n = self.prefix_len();
                if n > 0 {
                    let raw = &self.path[..n];
                    self.path = &self.path[n..];
                    let parsed = self.prefix.unwrap();
                    return Some(Component::Prefix(PrefixComponent { raw: OsStr::from_raw_bytes(raw), parsed }));
                }
            } else if self.front == START_DIR {
                self.front = BODY;
                if self.has_physical_root {
                    self.path = &self.path[1..];
                    return Some(Component::RootDir);
                } else if let Some(p) = self.prefix {
                    // a non-verbatim prefix with an implicit root (\\server\share, \\.\x) yields one
                    if p.has_implicit_root() && !p.is_verbatim() {
                        return Some(Component::RootDir);
                    }
                } else if self.include_cur_dir() {
                    self.path = &self.path[1..];
                    return Some(Component::CurDir);
                }
            } else if self.front == BODY {
                if self.path.is_empty() {
                    self.front = DONE;
                } else {
                    let (size, comp) = self.parse_next_component();
                    self.path = &self.path[size..];
                    if comp.is_some() {
                        return comp;
                    }
                }
            } else {
                panic!("internal error: entered unreachable code");
            }
        }
        None
    }
}

impl<'a> DoubleEndedIterator for Components<'a> {
    fn next_back(&mut self) -> Option<Component<'a>> {
        while !self.finished() {
            if self.back == BODY {
                if self.path.len() > self.len_before_body() {
                    let (size, comp) = self.parse_next_component_back();
                    self.path = &self.path[..self.path.len() - size];
                    if comp.is_some() {
                        return comp;
                    }
                } else {
                    self.back = START_DIR;
                }
            } else if self.back == START_DIR {
                self.back = PREFIX;
                if self.has_physical_root {
                    self.path = &self.path[..self.path.len() - 1];
                    return Some(Component::RootDir);
                } else if let Some(p) = self.prefix {
                    if p.has_implicit_root() && !p.is_verbatim() {
                        return Some(Component::RootDir);
                    }
                } else if self.include_cur_dir() {
                    self.path = &self.path[..self.path.len() - 1];
                    return Some(Component::CurDir);
                }
            } else if self.back == PREFIX {
                self.back = DONE;
                if self.prefix_len() > 0 {
                    let parsed = self.prefix.unwrap();
                    return Some(Component::Prefix(PrefixComponent { raw: OsStr::from_raw_bytes(self.path), parsed }));
                }
                return None;
            } else {
                panic!("internal error: entered unreachable code");
            }
        }
        None
    }
}

impl<'a> PartialEq for Components<'a> {
    fn eq(&self, other: &Components<'a>) -> bool {
        // fast path: identical bytes in the same state
        if self.path.len() == other.path.len()
            && self.front == other.front
            && self.back == BODY
            && other.back == BODY
            && self.prefix_verbatim() == other.prefix_verbatim()
            && self.path == other.path
        {
            return true;
        }
        let mut a = self.clone();
        let mut b = other.clone();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if x != y {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
}

impl<'a> Eq for Components<'a> {}

impl<'a> PartialOrd for Components<'a> {
    fn partial_cmp(&self, other: &Components<'a>) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for Components<'a> {
    fn cmp(&self, other: &Components<'a>) -> Ordering {
        let mut a = self.clone();
        let mut b = other.clone();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(x), Some(y)) => {
                    let o = x.cmp(&y);
                    if o != Ordering::Equal {
                        return o;
                    }
                }
            }
        }
    }
}

/// The component list of a path, for Components' Debug.
struct ComponentsList<'a>(&'a Path);

impl<'a> fmt::Debug for ComponentsList<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut l = f.debug_list();
        for c in self.0.components() {
            l.entry(&c);
        }
        l.finish()
    }
}

impl<'a> fmt::Debug for Components<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Components").field(&ComponentsList(self.as_path())).finish()
    }
}

#[derive(Clone)]
pub struct Iter<'a> {
    inner: Components<'a>,
}

impl<'a> Iter<'a> {
    pub fn as_path(&self) -> &'a Path {
        self.inner.as_path()
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        match self.inner.next() {
            Some(c) => Some(c.as_os_str()),
            None => None,
        }
    }
}

impl<'a> DoubleEndedIterator for Iter<'a> {
    fn next_back(&mut self) -> Option<&'a OsStr> {
        match self.inner.next_back() {
            Some(c) => Some(c.as_os_str()),
            None => None,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Ancestors<'a> {
    next: Option<&'a Path>,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = &'a Path;
    fn next(&mut self) -> Option<&'a Path> {
        let next = self.next;
        self.next = match next {
            Some(p) => p.parent(),
            None => None,
        };
        next
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripPrefixError(());

impl fmt::Display for StripPrefixError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("prefix not found")
    }
}

impl Error for StripPrefixError {}

#[repr(transparent)]
pub struct Path {
    inner: OsStr,
}

#[derive(Clone, Default)]
pub struct PathBuf {
    inner: OsString,
}

/// (stem-or-name, extension) split of a file name at its last dot.
fn rsplit_file_at_dot(file: &OsStr) -> (Option<&OsStr>, Option<&OsStr>) {
    let b = file.as_encoded_bytes();
    if b == b".." {
        return (Some(file), None);
    }
    let mut i = b.len();
    while i > 0 {
        i -= 1;
        if b[i] == b'.' {
            if i == 0 {
                return (Some(file), None);
            }
            return (Some(OsStr::from_raw_bytes(&b[..i])), Some(OsStr::from_raw_bytes(&b[i + 1..])));
        }
    }
    (None, Some(file))
}

fn iter_after<'a, 'b>(mut iter: Components<'a>, mut prefix: Components<'b>) -> Option<Components<'a>> {
    loop {
        let mut iter_next = iter.clone();
        match (iter_next.next(), prefix.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return None;
                }
            }
            (Some(_), None) => return Some(iter),
            (None, None) => return Some(iter),
            (None, Some(_)) => return None,
        }
        iter = iter_next;
    }
}

/// Same as iter_after from the back.
fn iter_after_back<'a, 'b>(mut iter: Components<'a>, mut suffix: Components<'b>) -> bool {
    loop {
        match (iter.next_back(), suffix.next_back()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return false;
                }
            }
            (_, None) => return true,
            (None, Some(_)) => return false,
        }
    }
}

impl Path {
    pub const fn new<S: AsRef<OsStr> + ?Sized>(s: &S) -> &Path {
        let os: &OsStr = s.as_ref();
        unsafe { &*(os as *const OsStr as *const Path) }
    }
    pub(crate) fn from_bytes(b: &[u8]) -> &Path {
        Path::new(OsStr::from_raw_bytes(b))
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        self.inner.as_encoded_bytes()
    }
    pub fn as_os_str(&self) -> &OsStr {
        &self.inner
    }
    pub fn as_mut_os_str(&mut self) -> &mut OsStr {
        &mut self.inner
    }
    pub fn to_str(&self) -> Option<&str> {
        self.inner.to_str()
    }
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        self.inner.to_string_lossy()
    }
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf { inner: self.inner.to_os_string() }
    }
    /// unix: rooted. Windows: rooted and prefixed (`C:\x`, `\\server\share`, `\\?\x`), so
    /// `\x` (root, no prefix) and `C:x` (prefix, no root) are relative.
    pub fn is_absolute(&self) -> bool {
        if HAS_PREFIXES {
            self.has_root() && self.prefix().is_some()
        } else {
            self.has_root()
        }
    }
    pub fn is_relative(&self) -> bool {
        !self.is_absolute()
    }
    pub fn has_root(&self) -> bool {
        self.components().has_root()
    }
    pub(crate) fn prefix(&self) -> Option<Prefix<'_>> {
        self.components().prefix
    }
    pub fn parent(&self) -> Option<&Path> {
        let mut comps = self.components();
        match comps.next_back() {
            Some(Component::Normal(_)) | Some(Component::CurDir) | Some(Component::ParentDir) => Some(comps.as_path()),
            _ => None,
        }
    }
    pub fn ancestors(&self) -> Ancestors<'_> {
        Ancestors { next: Some(self) }
    }
    pub fn file_name(&self) -> Option<&OsStr> {
        match self.components().next_back() {
            Some(Component::Normal(p)) => Some(p),
            _ => None,
        }
    }
    pub fn strip_prefix<P: AsRef<Path>>(&self, base: P) -> Result<&Path, StripPrefixError> {
        match iter_after(self.components(), base.as_ref().components()) {
            Some(c) => Ok(c.as_path()),
            None => Err(StripPrefixError(())),
        }
    }
    pub fn starts_with<P: AsRef<Path>>(&self, base: P) -> bool {
        iter_after(self.components(), base.as_ref().components()).is_some()
    }
    pub fn ends_with<P: AsRef<Path>>(&self, child: P) -> bool {
        iter_after_back(self.components(), child.as_ref().components())
    }
    pub fn file_stem(&self) -> Option<&OsStr> {
        match self.file_name() {
            Some(name) => {
                let (before, after) = rsplit_file_at_dot(name);
                match before {
                    Some(b) => Some(b),
                    None => after,
                }
            }
            None => None,
        }
    }
    pub fn extension(&self) -> Option<&OsStr> {
        match self.file_name() {
            Some(name) => {
                let (before, after) = rsplit_file_at_dot(name);
                match before {
                    Some(_) => after,
                    None => None,
                }
            }
            None => None,
        }
    }
    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.push(path);
        buf
    }
    pub fn with_file_name<S: AsRef<OsStr>>(&self, file_name: S) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.set_file_name(file_name);
        buf
    }
    pub fn with_extension<S: AsRef<OsStr>>(&self, extension: S) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.set_extension(extension);
        buf
    }
    pub fn components(&self) -> Components<'_> {
        let prefix = parse_prefix(self.as_os_str());
        let path = self.bytes();
        Components { path, prefix, has_physical_root: has_physical_root(path, prefix), front: PREFIX, back: BODY }
    }
    pub fn iter(&self) -> Iter<'_> {
        Iter { inner: self.components() }
    }
    pub fn display(&self) -> Display<'_> {
        Display { path: self }
    }
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        fs::metadata(self)
    }
    pub fn symlink_metadata(&self) -> io::Result<fs::Metadata> {
        fs::symlink_metadata(self)
    }
    pub fn canonicalize(&self) -> io::Result<PathBuf> {
        fs::canonicalize(self)
    }
    pub fn read_link(&self) -> io::Result<PathBuf> {
        fs::read_link(self)
    }
    pub fn read_dir(&self) -> io::Result<fs::ReadDir> {
        fs::read_dir(self)
    }
    pub fn exists(&self) -> bool {
        fs::metadata(self).is_ok()
    }
    pub fn try_exists(&self) -> io::Result<bool> {
        fs::exists(self)
    }
    pub fn is_file(&self) -> bool {
        match fs::metadata(self) {
            Ok(m) => m.is_file(),
            Err(_) => false,
        }
    }
    pub fn is_dir(&self) -> bool {
        match fs::metadata(self) {
            Ok(m) => m.is_dir(),
            Err(_) => false,
        }
    }
    pub fn is_symlink(&self) -> bool {
        match fs::symlink_metadata(self) {
            Ok(m) => m.is_symlink(),
            Err(_) => false,
        }
    }
    pub fn into_path_buf(self: Box<Path>) -> PathBuf {
        let raw = Box::into_raw(self) as *mut OsStr;
        PathBuf { inner: unsafe { Box::from_raw(raw) }.into_os_string() }
    }
}

impl PathBuf {
    pub const fn new() -> PathBuf {
        PathBuf { inner: OsString::new() }
    }
    pub fn with_capacity(capacity: usize) -> PathBuf {
        PathBuf { inner: OsString::with_capacity(capacity) }
    }
    pub fn as_path(&self) -> &Path {
        Path::new(&self.inner)
    }
    pub fn push<P: AsRef<Path>>(&mut self, path: P) {
        self.push_path(path.as_ref())
    }
    /// Real std's PathBuf::_push: an absolute or prefixed `path` replaces self; on a verbatim
    /// self the components are joined with `.`/`..` resolved; a rooted unprefixed `path`
    /// (`\x` on Windows) keeps self's prefix; otherwise a separator joins them (none after a
    /// bare `C:`).
    fn push_path(&mut self, path: &Path) {
        let mut need_sep = match self.inner.as_encoded_bytes().last() {
            Some(c) => !is_sep_byte(*c),
            None => false,
        };
        let comps = self.components();
        if comps.prefix_len() > 0 && comps.prefix_len() == comps.path.len() && comps.prefix.unwrap().is_drive() {
            need_sep = false;
        }
        if path.is_absolute() || path.prefix().is_some() {
            self.inner.clear();
        } else if comps.prefix_verbatim() && !path.as_os_str().is_empty() {
            let mut buf: Vec<Component<'_>> = comps.collect();
            for c in path.components() {
                match c {
                    Component::RootDir => {
                        buf.truncate(1);
                        buf.push(c);
                    }
                    Component::CurDir => {}
                    Component::ParentDir => {
                        if let Some(Component::Normal(_)) = buf.last() {
                            buf.pop();
                        }
                    }
                    _ => buf.push(c),
                }
            }
            let mut res = OsString::new();
            let mut need_sep = false;
            for c in buf {
                if need_sep && c != Component::RootDir {
                    res.push(MAIN_SEPARATOR_STR);
                }
                res.push(c.as_os_str());
                need_sep = match c {
                    Component::RootDir => false,
                    Component::Prefix(prefix) => !prefix.parsed.is_drive() && prefix.parsed.len() > 0,
                    _ => true,
                };
            }
            self.inner = res;
            return;
        } else if path.has_root() {
            let prefix_len = self.components().prefix_remaining();
            self.inner.vec_mut().truncate(prefix_len);
        } else if need_sep {
            self.inner.push(MAIN_SEPARATOR_STR);
        }
        self.inner.push(path.as_os_str());
    }
    pub fn pop(&mut self) -> bool {
        let len = match self.parent() {
            Some(p) => p.bytes().len(),
            None => return false,
        };
        self.inner.vec_mut().truncate(len);
        true
    }
    pub fn set_file_name<S: AsRef<OsStr>>(&mut self, file_name: S) {
        if self.file_name().is_some() {
            self.pop();
        }
        self.push(Path::new(file_name.as_ref()));
    }
    pub fn set_extension<S: AsRef<OsStr>>(&mut self, extension: S) -> bool {
        let extension = extension.as_ref();
        for b in extension.as_encoded_bytes() {
            if is_sep_byte(*b) {
                panic!("extension cannot contain path separators: {:?}", extension);
            }
        }
        let end = match self.file_stem() {
            None => return false,
            Some(stem) => {
                let base = self.inner.as_encoded_bytes().as_ptr() as usize;
                stem.as_encoded_bytes().as_ptr() as usize + stem.len() - base
            }
        };
        let v = self.inner.vec_mut();
        v.truncate(end);
        let new = extension.as_encoded_bytes();
        if !new.is_empty() {
            v.push(b'.');
            v.extend_from_slice(new);
        }
        true
    }
    pub fn as_mut_os_string(&mut self) -> &mut OsString {
        &mut self.inner
    }
    pub fn into_os_string(self) -> OsString {
        self.inner
    }
    pub fn into_boxed_path(self) -> Box<Path> {
        let raw = Box::into_raw(self.inner.into_boxed_os_str()) as *mut Path;
        unsafe { Box::from_raw(raw) }
    }
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
    pub fn clear(&mut self) {
        self.inner.clear()
    }
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional)
    }
}

impl Deref for PathBuf {
    type Target = Path;
    fn deref(&self) -> &Path {
        Path::new(&self.inner)
    }
}

impl core::ops::DerefMut for PathBuf {
    fn deref_mut(&mut self) -> &mut Path {
        let os: &mut OsStr = &mut self.inner;
        unsafe { &mut *(os as *mut OsStr as *mut Path) }
    }
}

impl Borrow<Path> for PathBuf {
    fn borrow(&self) -> &Path {
        self.deref()
    }
}

impl ToOwned for Path {
    type Owned = PathBuf;
    fn to_owned(&self) -> PathBuf {
        self.to_path_buf()
    }
}

impl AsRef<Path> for Path {
    fn as_ref(&self) -> &Path {
        self
    }
}
impl AsRef<Path> for PathBuf {
    fn as_ref(&self) -> &Path {
        self
    }
}
impl AsRef<Path> for OsStr {
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}
impl AsRef<Path> for OsString {
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}
impl AsRef<Path> for str {
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}
impl AsRef<Path> for String {
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}
impl<'a> AsRef<Path> for Cow<'a, OsStr> {
    fn as_ref(&self) -> &Path {
        Path::new(&**self)
    }
}
impl AsRef<OsStr> for Path {
    fn as_ref(&self) -> &OsStr {
        &self.inner
    }
}
impl AsRef<OsStr> for PathBuf {
    fn as_ref(&self) -> &OsStr {
        &self.inner
    }
}

impl From<String> for PathBuf {
    fn from(s: String) -> PathBuf {
        PathBuf { inner: OsString::from(s) }
    }
}
/// Any `&T: AsRef<OsStr>` (str, String, Path, PathBuf, OsStr, OsString, ...), as real std.
impl<T: ?Sized + AsRef<OsStr>> From<&T> for PathBuf {
    fn from(s: &T) -> PathBuf {
        PathBuf::from(s.as_ref().to_os_string())
    }
}
impl From<OsString> for PathBuf {
    fn from(s: OsString) -> PathBuf {
        PathBuf { inner: s }
    }
}
impl From<PathBuf> for OsString {
    fn from(p: PathBuf) -> OsString {
        p.inner
    }
}
impl From<Box<Path>> for PathBuf {
    fn from(b: Box<Path>) -> PathBuf {
        b.into_path_buf()
    }
}
impl<'a> From<&'a Path> for Cow<'a, Path> {
    fn from(p: &'a Path) -> Cow<'a, Path> {
        Cow::Borrowed(p)
    }
}
impl<'a> From<&'a PathBuf> for Cow<'a, Path> {
    fn from(p: &'a PathBuf) -> Cow<'a, Path> {
        Cow::Borrowed(p.as_path())
    }
}
impl<'a> From<Cow<'a, Path>> for PathBuf {
    fn from(p: Cow<'a, Path>) -> PathBuf {
        p.into_owned()
    }
}
impl<'a> From<PathBuf> for Cow<'a, Path> {
    fn from(p: PathBuf) -> Cow<'a, Path> {
        Cow::Owned(p)
    }
}

impl<P: AsRef<Path>> core::iter::FromIterator<P> for PathBuf {
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> PathBuf {
        let mut buf = PathBuf::new();
        for p in iter {
            buf.push(p);
        }
        buf
    }
}

impl<P: AsRef<Path>> core::iter::Extend<P> for PathBuf {
    fn extend<I: IntoIterator<Item = P>>(&mut self, iter: I) {
        for p in iter {
            self.push(p);
        }
    }
}

impl<'a> IntoIterator for &'a Path {
    type Item = &'a OsStr;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a PathBuf {
    type Item = &'a OsStr;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl PartialEq for Path {
    fn eq(&self, other: &Path) -> bool {
        self.components() == other.components()
    }
}
impl Eq for Path {}
impl PartialOrd for Path {
    fn partial_cmp(&self, other: &Path) -> Option<Ordering> {
        Some(Ord::cmp(&self.components(), &other.components()))
    }
}
impl Ord for Path {
    fn cmp(&self, other: &Path) -> Ordering {
        Ord::cmp(&self.components(), &other.components())
    }
}
impl Hash for Path {
    fn hash<H: Hasher>(&self, h: &mut H) {
        for c in self.components() {
            c.hash(h);
        }
    }
}

impl PartialEq for PathBuf {
    fn eq(&self, other: &PathBuf) -> bool {
        self.components() == other.components()
    }
}
impl Eq for PathBuf {}
impl PartialOrd for PathBuf {
    fn partial_cmp(&self, other: &PathBuf) -> Option<Ordering> {
        Some(Ord::cmp(&self.components(), &other.components()))
    }
}
impl Ord for PathBuf {
    fn cmp(&self, other: &PathBuf) -> Ordering {
        Ord::cmp(&self.components(), &other.components())
    }
}
impl Hash for PathBuf {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_path().hash(h)
    }
}
impl PartialEq<Path> for PathBuf {
    fn eq(&self, other: &Path) -> bool {
        self.as_path() == other
    }
}
impl PartialEq<PathBuf> for Path {
    fn eq(&self, other: &PathBuf) -> bool {
        self == other.as_path()
    }
}
impl<'a> PartialEq<&'a Path> for PathBuf {
    fn eq(&self, other: &&'a Path) -> bool {
        self.as_path() == *other
    }
}
impl<'a> PartialEq<PathBuf> for &'a Path {
    fn eq(&self, other: &PathBuf) -> bool {
        *self == other.as_path()
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}
impl fmt::Debug for PathBuf {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.as_path(), f)
    }
}

pub struct Display<'a> {
    path: &'a Path,
}

impl<'a> fmt::Display for Display<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.path.inner.display(), f)
    }
}

impl<'a> fmt::Debug for Display<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.path, f)
    }
}

/// `std::path::absolute`: joins relative paths onto the current directory without
/// touching the filesystem (unix keeps `..`, drops `.` and repeated separators).
#[cfg(unix)]
pub fn absolute<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "cannot make an empty path absolute"));
    }
    let mut out: Vec<u8> = Vec::new();
    let b = path.bytes();
    if path.is_absolute() {
        // POSIX: exactly two leading slashes are kept
        if b.len() >= 2 && b[0] == b'/' && b[1] == b'/' && (b.len() == 2 || b[2] != b'/') {
            out.push(b'/');
        }
    } else {
        let cwd = crate::env::current_dir()?;
        out.extend_from_slice(cwd.bytes());
    }
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::CurDir => {}
            _ => {
                if out.last() != Some(&b'/') {
                    out.push(b'/');
                }
                out.extend_from_slice(c.as_os_str().as_encoded_bytes());
            }
        }
    }
    if out.is_empty() {
        out.push(b'/');
    }
    if b.last() == Some(&b'/') && out.last() != Some(&b'/') {
        out.push(b'/');
    }
    Ok(PathBuf { inner: OsString::from_bytes_vec(out) })
}

/// `std::path::absolute` on Windows (real std sys/path/windows.rs): verbatim paths are returned
/// unchanged, everything else goes through GetFullPathNameW (which resolves `.`/`..`, the
/// current directory and the per-drive current directory of `C:x`, without touching the disk).
#[cfg(windows)]
pub fn absolute<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    use crate::os::windows::ffi::OsStringExt;
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "cannot make an empty path absolute"));
    }
    let verbatim = match parse_prefix(path.as_os_str()) {
        Some(p) => p.is_verbatim(),
        None => false,
    };
    if verbatim {
        if path.bytes().contains(&0) {
            return Err(io::const_error!(io::ErrorKind::InvalidInput, "strings passed to WinAPI cannot contain NULs"));
        }
        return Ok(path.to_path_buf());
    }
    let wide = crate::sys::os::to_wide(path.as_os_str())?;
    let name = wide.as_ptr();
    let full = crate::sys::os::fill_utf16_buf(|buf, n| unsafe { GetFullPathNameW(name, n, buf, core::ptr::null_mut()) })?;
    Ok(PathBuf::from(OsString::from_wide(&full)))
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetFullPathNameW(name: *const u16, len: u32, buf: *mut u16, file_part: *mut *mut u16) -> u32;
}

/// Byte copy of the whole value (real std's CloneToUninit for Path), for Rc/Arc::make_mut.
#[cfg(not(relax_shim))]
unsafe impl core::clone::CloneToUninit for Path {
    unsafe fn clone_to_uninit(&self, dest: *mut u8) {
        let n = core::mem::size_of_val(self);
        core::ptr::copy_nonoverlapping(self as *const Path as *const u8, dest, n);
    }
}

impl From<&Path> for Box<Path> {
    fn from(p: &Path) -> Box<Path> {
        p.to_path_buf().into_boxed_path()
    }
}

impl From<PathBuf> for Box<Path> {
    fn from(p: PathBuf) -> Box<Path> {
        p.into_boxed_path()
    }
}

impl From<&Path> for alloc::sync::Arc<Path> {
    fn from(p: &Path) -> alloc::sync::Arc<Path> {
        let s: alloc::sync::Arc<OsStr> = alloc::sync::Arc::from(p.as_os_str());
        unsafe { alloc::sync::Arc::from_raw(alloc::sync::Arc::into_raw(s) as *const Path) }
    }
}

impl From<PathBuf> for alloc::sync::Arc<Path> {
    fn from(p: PathBuf) -> alloc::sync::Arc<Path> {
        alloc::sync::Arc::from(&*p)
    }
}

impl From<&Path> for alloc::rc::Rc<Path> {
    fn from(p: &Path) -> alloc::rc::Rc<Path> {
        let s: alloc::rc::Rc<OsStr> = alloc::rc::Rc::from(p.as_os_str());
        unsafe { alloc::rc::Rc::from_raw(alloc::rc::Rc::into_raw(s) as *const Path) }
    }
}

impl From<PathBuf> for alloc::rc::Rc<Path> {
    fn from(p: PathBuf) -> alloc::rc::Rc<Path> {
        alloc::rc::Rc::from(&*p)
    }
}
