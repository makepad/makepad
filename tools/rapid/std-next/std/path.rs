//! std::path for unix: Path (unsized newtype over OsStr, coord.md F-OS3) and PathBuf. The
//! component parser is real std's double-ended state machine without the Windows prefix
//! states (Rust std path.rs, MIT/Apache-2.0).

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

pub const MAIN_SEPARATOR: char = '/';
pub const MAIN_SEPARATOR_STR: &str = "/";

pub fn is_separator(c: char) -> bool {
    c == '/'
}

fn is_sep_byte(b: u8) -> bool {
    b == b'/'
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Component<'a> {
    RootDir,
    CurDir,
    ParentDir,
    Normal(&'a OsStr),
}

impl<'a> Component<'a> {
    pub fn as_os_str(self) -> &'a OsStr {
        match self {
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

// front/back cursor states (Prefix kept so the ordering matches real std)
const PREFIX: u8 = 0;
const START_DIR: u8 = 1;
const BODY: u8 = 2;
const DONE: u8 = 3;

#[derive(Clone)]
pub struct Components<'a> {
    path: &'a [u8],
    has_physical_root: bool,
    front: u8,
    back: u8,
}

impl<'a> Components<'a> {
    fn include_cur_dir(&self) -> bool {
        if self.has_physical_root {
            return false;
        }
        let p = self.path;
        if p.is_empty() || p[0] != b'.' {
            return false;
        }
        p.len() == 1 || is_sep_byte(p[1])
    }

    fn len_before_body(&self) -> usize {
        let root = if self.front <= START_DIR && self.has_physical_root { 1 } else { 0 };
        let cur_dir = if self.front <= START_DIR && self.include_cur_dir() { 1 } else { 0 };
        root + cur_dir
    }

    fn finished(&self) -> bool {
        self.front == DONE || self.back == DONE || self.front > self.back
    }

    fn parse_single_component(comp: &'a [u8]) -> Option<Component<'a>> {
        if comp.is_empty() || comp == b"." {
            None
        } else if comp == b".." {
            Some(Component::ParentDir)
        } else {
            Some(Component::Normal(OsStr::from_raw_bytes(comp)))
        }
    }

    fn parse_next_component(&self) -> (usize, Option<Component<'a>>) {
        let mut i = 0;
        while i < self.path.len() && !is_sep_byte(self.path[i]) {
            i += 1;
        }
        let (extra, comp) = if i == self.path.len() { (0, self.path) } else { (1, &self.path[..i]) };
        (comp.len() + extra, Components::parse_single_component(comp))
    }

    fn parse_next_component_back(&self) -> (usize, Option<Component<'a>>) {
        let start = self.len_before_body();
        let body = &self.path[start..];
        let mut i = body.len();
        let mut sep = None;
        while i > 0 {
            i -= 1;
            if is_sep_byte(body[i]) {
                sep = Some(i);
                break;
            }
        }
        let (extra, comp) = match sep {
            None => (0, body),
            Some(i) => (1, &body[i + 1..]),
        };
        (comp.len() + extra, Components::parse_single_component(comp))
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
            } else if self.front == START_DIR {
                self.front = BODY;
                if self.has_physical_root {
                    self.path = &self.path[1..];
                    return Some(Component::RootDir);
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
                } else if self.include_cur_dir() {
                    self.path = &self.path[..self.path.len() - 1];
                    return Some(Component::CurDir);
                }
            } else if self.back == PREFIX {
                self.back = DONE;
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
        if self.path.len() == other.path.len() && self.front == other.front && self.back == BODY && other.back == BODY && self.path == other.path {
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
    pub fn new<S: AsRef<OsStr> + ?Sized>(s: &S) -> &Path {
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
    pub fn is_absolute(&self) -> bool {
        self.has_root()
    }
    pub fn is_relative(&self) -> bool {
        !self.is_absolute()
    }
    pub fn has_root(&self) -> bool {
        let b = self.bytes();
        !b.is_empty() && is_sep_byte(b[0])
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
        Components { path: self.bytes(), has_physical_root: self.has_root(), front: PREFIX, back: BODY }
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
    pub fn new() -> PathBuf {
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
    fn push_path(&mut self, path: &Path) {
        let need_sep = match self.inner.as_encoded_bytes().last() {
            Some(c) => !is_sep_byte(*c),
            None => false,
        };
        if path.is_absolute() {
            self.inner.clear();
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
impl From<&str> for PathBuf {
    fn from(s: &str) -> PathBuf {
        PathBuf { inner: OsString::from(s) }
    }
}
impl From<&String> for PathBuf {
    fn from(s: &String) -> PathBuf {
        PathBuf { inner: OsString::from(s.as_str()) }
    }
}
impl From<OsString> for PathBuf {
    fn from(s: OsString) -> PathBuf {
        PathBuf { inner: s }
    }
}
impl From<&Path> for PathBuf {
    fn from(s: &Path) -> PathBuf {
        s.to_path_buf()
    }
}
impl From<&OsStr> for PathBuf {
    fn from(s: &OsStr) -> PathBuf {
        PathBuf { inner: s.to_os_string() }
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
pub fn absolute<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "cannot make an empty path absolute"));
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
