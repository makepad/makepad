use {
    crate::unix_str::{UnixString, UnixStr},
    makepad_micro_serde::{DeBin, DeBinErr, SerBin},
    std::{borrow::Borrow, iter::FromIterator, ops::Deref},
};

#[derive(Clone, DeBin, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, SerBin)]
pub struct UnixPathBuf {
    string: UnixString
}

impl UnixPathBuf {
    pub fn new() -> Self {
        Self {
            string: UnixString::new()
        }
    }

    pub fn into_unix_string(self) -> UnixString {
        self.string
    }

    pub fn as_unix_path(&self) -> &UnixPath {
        UnixPath::new(&self.string)
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<u8> {
        self.string.as_mut_vec()
    }

    pub fn push<P: AsRef<UnixPath>>(&mut self, path: P) {
        self._push(path.as_ref())
    }

    fn _push(&mut self, path: &UnixPath) {
        // in general, a separator is needed if the rightmost byte is not a separator
        let need_sep = self
            .as_mut_vec()
            .last()
            .map(|c| *c != b'/')
            .unwrap_or(false);

        // absolute `path` replaces `self`
        if path.is_absolute() || path.has_root() {
            self.as_mut_vec().truncate(0);
        } else if need_sep {
            self.string.push("/");
        }

        self.string.push(path.as_unix_str());
    }
}

impl<T: ?Sized + AsRef<UnixStr>> From<&T> for UnixPathBuf {
    fn from(s: &T) -> Self {
        Self::from(s.as_ref().to_unix_string())
    }
}

impl From<UnixString> for UnixPathBuf {
    fn from(string: UnixString) -> Self {
        Self { string }
    }
}

impl<P: AsRef<UnixPath>> FromIterator<P> for UnixPathBuf {
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> Self {
        let mut buf = Self::new();
        buf.extend(iter);
        buf
    }
}

impl Deref for UnixPathBuf {
    type Target = UnixPath;

    fn deref(&self) -> &UnixPath {
        self.as_unix_path()
    }
}

impl AsRef<UnixPath> for UnixPathBuf {
    fn as_ref(&self) -> &UnixPath {
        self.as_unix_path()
    }
}

impl Borrow<UnixPath> for UnixPathBuf {
    fn borrow(&self) -> &UnixPath {
        self.as_unix_path()
    }
}

impl<P: AsRef<UnixPath>> Extend<P> for UnixPathBuf {
    fn extend<I: IntoIterator<Item = P>>(&mut self, iter: I) {
        iter.into_iter().for_each(move |path| self.push(path.as_ref()));
    }
}

#[derive(Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct UnixPath {
    string: UnixStr
}

impl UnixPath {
    pub fn new<S: AsRef<UnixStr> + ?Sized>(string: &S) -> &Self {
        unsafe { std::mem::transmute(string.as_ref()) }
    }

    pub fn from_bytes(bytes: &[u8]) -> &Self {
        Self::new(UnixStr::from_bytes(bytes))
    }

    pub fn as_unix_str(&self) -> &UnixStr {
        &self.string
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.string.as_bytes()
    }

    /// Returns `true` if the `Path` is absolute, i.e., if it is independent of
    /// the current directory.
    ///
    /// A path is absolute if it starts with the root, so `is_absolute` and
    /// [`has_root`] are equivalent.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_collab_protocol::unix_path::UnixPath;
    ///
    /// assert!(!UnixPath::new("foo.txt").is_absolute());
    /// ```
    ///
    /// [`has_root`]: #method.has_root
    pub fn is_absolute(&self) -> bool {
        self.has_root()
    }

    /// Returns `true` if the `Path` has a root.
    ///
    /// A path has a root if it begins with `/`.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_collab_protocol::unix_path::UnixPath;
    ///
    /// assert!(UnixPath::new("/etc/passwd").has_root());
    /// ```
    pub fn has_root(&self) -> bool {
        self.components().has_root()
    }

    pub fn file_name(&self) -> Option<&UnixStr> {
        self.components().next_back().and_then(|p| match p {
            Component::Normal(p) => Some(p),
            _ => None,
        })
    }

    /// Produces an iterator over the [`Component`]s of the path.
    ///
    /// When parsing the path, there is a small amount of normalization:
    ///
    /// * Repeated separators are ignored, so `a/b` and `a//b` both have
    ///   `a` and `b` as components.
    ///
    /// * Occurrences of `.` are normalized away, except if they are at the
    ///   beginning of the path. For example, `a/./b`, `a/b/`, `a/b/.` and
    ///   `a/b` all have `a` and `b` as components, but `./a/b` starts with
    ///   an additional [`CurDir`] component.
    ///
    /// * A trailing slash is normalized away, `/a/b` and `/a/b/` are equivalent.
    ///
    /// Note that no other normalization takes place; in particular, `a/c`
    /// and `a/b/../c` are distinct, to account for the possibility that `b`
    /// is a symbolic link (so its parent isn't `a`).
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_collab_protocol::unix_path::{UnixPath, Component};
    /// use makepad_collab_protocol::unix_str::UnixStr;
    ///
    /// let mut components = UnixPath::new("/tmp/foo.txt").components();
    ///
    /// assert_eq!(components.next(), Some(Component::RootDir));
    /// assert_eq!(components.next(), Some(Component::Normal(UnixStr::new("tmp"))));
    /// assert_eq!(components.next(), Some(Component::Normal(UnixStr::new("foo.txt"))));
    /// assert_eq!(components.next(), None)
    /// ```
    ///
    /// [`Component`]: enum.Component.html
    /// [`CurDir`]: enum.Component.html#variant.CurDir
    pub fn components(&self) -> Components<'_> {
        Components {
            path: self.as_bytes(),
            has_physical_root: has_physical_root(self.as_bytes()),
            front: State::Prefix,
            back: State::Body,
        }
    }

    pub fn join<P: AsRef<Self>>(&self, path: P) -> UnixPathBuf {
        self._join(path.as_ref())
    }

    fn _join(&self, path: &Self) -> UnixPathBuf {
        let mut buf = self.to_unix_path_buf();
        buf.push(path);
        buf
    }

    pub fn to_unix_path_buf(&self) -> UnixPathBuf {
        UnixPathBuf::from(&self.string)
    }
}

impl AsRef<UnixPath> for UnixPath {
    fn as_ref(&self) -> &UnixPath {
        self
    }
}

#[derive(Clone)]
pub struct Components<'a> {
    // The path left to parse components from
    path: &'a [u8],

    // true if path *physically* has a root separator; for most Windows
    // prefixes, it may have a "logical" root separator for the purposes of
    // normalization, e.g.,  \\server\share == \\server\share\.
    has_physical_root: bool,

    // The iterator is double-ended, and these two states keep track of what has
    // been produced from either end
    front: State,
    back: State,
}

impl<'a> Components<'a> {
    // Given the iteration so far, how much of the pre-State::Body path is left?
    #[inline]
    fn len_before_body(&self) -> usize {
        let root = if self.front <= State::StartDir && self.has_physical_root {
            1
        } else {
            0
        };
        let cur_dir = if self.front <= State::StartDir && self.include_cur_dir() {
            1
        } else {
            0
        };
        root + cur_dir
    }

    // is the iteration complete?
    #[inline]
    fn finished(&self) -> bool {
        self.front == State::Done || self.back == State::Done || self.front > self.back
    }

    #[inline]
    fn is_sep_byte(&self, b: u8) -> bool {
        b == b'/'
    }

    /// Extracts a slice corresponding to the portion of the path remaining for iteration.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_collab_protocol::unix_path::UnixPath;
    ///
    /// let mut components = UnixPath::new("/tmp/foo/bar.txt").components();
    /// components.next();
    /// components.next();
    ///
    /// assert_eq!(UnixPath::new("foo/bar.txt"), components.as_unix_path());
    /// ```
    pub fn as_unix_path(&self) -> &'a UnixPath {
        let mut comps = self.clone();
        if comps.front == State::Body {
            comps.trim_left();
        }
        if comps.back == State::Body {
            comps.trim_right();
        }
        UnixPath::from_bytes(comps.path)
    }

    /// Is the *original* path rooted?
    fn has_root(&self) -> bool {
        self.has_physical_root
    }

    /// Should the normalized path include a leading . ?
    fn include_cur_dir(&self) -> bool {
        if self.has_root() {
            return false;
        }
        let mut iter = self.path[..].iter();
        match (iter.next(), iter.next()) {
            (Some(&b'.'), None) => true,
            (Some(&b'.'), Some(&b)) => self.is_sep_byte(b),
            _ => false,
        }
    }

    // parse a given byte sequence into the corresponding path component
    fn parse_single_component<'b>(&self, comp: &'b [u8]) -> Option<Component<'b>> {
        match comp {
            b"." => None, // . components are normalized away, except at
            // the beginning of a path, which is treated
            // separately via `include_cur_dir`
            b".." => Some(Component::ParentDir),
            b"" => None,
            _ => Some(Component::Normal(UnixStr::from_bytes(comp))),
        }
    }

    // parse a component from the left, saying how many bytes to consume to
    // remove the component
    fn parse_next_component(&self) -> (usize, Option<Component<'a>>) {
        debug_assert!(self.front == State::Body);
        let (extra, comp) = match self.path.iter().position(|b| self.is_sep_byte(*b)) {
            None => (0, self.path),
            Some(i) => (1, &self.path[..i]),
        };
        (comp.len() + extra, self.parse_single_component(comp))
    }

    // parse a component from the right, saying how many bytes to consume to
    // remove the component
    fn parse_next_component_back(&self) -> (usize, Option<Component<'a>>) {
        debug_assert!(self.back == State::Body);
        let start = self.len_before_body();
        let (extra, comp) = match self.path[start..]
            .iter()
            .rposition(|b| self.is_sep_byte(*b))
        {
            None => (0, &self.path[start..]),
            Some(i) => (1, &self.path[start + i + 1..]),
        };
        (comp.len() + extra, self.parse_single_component(comp))
    }

    // trim away repeated separators (i.e., empty components) on the left
    fn trim_left(&mut self) {
        while !self.path.is_empty() {
            let (size, comp) = self.parse_next_component();
            if comp.is_some() {
                return;
            } else {
                self.path = &self.path[size..];
            }
        }
    }

    // trim away repeated separators (i.e., empty components) on the right
    fn trim_right(&mut self) {
        while self.path.len() > self.len_before_body() {
            let (size, comp) = self.parse_next_component_back();
            if comp.is_some() {
                return;
            } else {
                self.path = &self.path[..self.path.len() - size];
            }
        }
    }
}

impl AsRef<UnixPath> for Components<'_> {
    fn as_ref(&self) -> &UnixPath {
        self.as_unix_path()
    }
}

impl AsRef<UnixStr> for Components<'_> {
    fn as_ref(&self) -> &UnixStr {
        self.as_unix_path().as_unix_str()
    }
}

impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;

    fn next(&mut self) -> Option<Component<'a>> {
        while !self.finished() {
            match self.front {
                State::Prefix => {
                    self.front = State::StartDir;
                }
                State::StartDir => {
                    self.front = State::Body;
                    if self.has_physical_root {
                        debug_assert!(!self.path.is_empty());
                        self.path = &self.path[1..];
                        return Some(Component::RootDir);
                    } else if self.include_cur_dir() {
                        debug_assert!(!self.path.is_empty());
                        self.path = &self.path[1..];
                        return Some(Component::CurDir);
                    }
                }
                State::Body if !self.path.is_empty() => {
                    let (size, comp) = self.parse_next_component();
                    self.path = &self.path[size..];
                    if comp.is_some() {
                        return comp;
                    }
                }
                State::Body => {
                    self.front = State::Done;
                }
                State::Done => unreachable!(),
            }
        }
        None
    }
}

impl<'a> DoubleEndedIterator for Components<'a> {
    fn next_back(&mut self) -> Option<Component<'a>> {
        while !self.finished() {
            match self.back {
                State::Body if self.path.len() > self.len_before_body() => {
                    let (size, comp) = self.parse_next_component_back();
                    self.path = &self.path[..self.path.len() - size];
                    if comp.is_some() {
                        return comp;
                    }
                }
                State::Body => {
                    self.back = State::StartDir;
                }
                State::StartDir => {
                    self.back = State::Prefix;
                    if self.has_physical_root {
                        self.path = &self.path[..self.path.len() - 1];
                        return Some(Component::RootDir);
                    } else if self.include_cur_dir() {
                        self.path = &self.path[..self.path.len() - 1];
                        return Some(Component::CurDir);
                    }
                }
                State::Prefix => {
                    self.back = State::Done;
                    return None;
                }
                State::Done => unreachable!(),
            }
        }
        None
    }
}

/// A single component of a path.
///
/// A `Component` roughly corresponds to a substring between path separators
/// (`/`).
///
/// This `enum` is created by iterating over [`Components`], which in turn is
/// created by the [`components`][`Path::components`] method on [`Path`].
///
/// # Examples
///
/// ```rust
/// use makepad_collab_protocol::unix_path::{Component, UnixPath};
///
/// let path = UnixPath::new("/tmp/foo/bar.txt");
/// let components = path.components().collect::<Vec<_>>();
/// assert_eq!(&components, &[
///     Component::RootDir,
///     Component::Normal("tmp".as_ref()),
///     Component::Normal("foo".as_ref()),
///     Component::Normal("bar.txt".as_ref()),
/// ]);
/// ```
///
/// [`Components`]: struct.Components.html
/// [`Path`]: struct.Path.html
/// [`Path::components`]: struct.Path.html#method.components
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Component<'a> {
    /// The root directory component, appears after any prefix and before anything else.
    ///
    /// It represents a separator that designates that a path starts from root.
    RootDir,

    /// A reference to the current directory, i.e., `.`.
    CurDir,

    /// A reference to the parent directory, i.e., `..`.
    ParentDir,

    /// A normal component, e.g., `a` and `b` in `a/b`.
    ///
    /// This variant is the most common one, it represents references to files
    /// or directories.
    Normal(&'a UnixStr),
}

impl<'a> Component<'a> {
    /// Extracts the underlying `UnixStr` slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_collab_protocol::unix_path::UnixPath;
    ///
    /// let path = UnixPath::new("./tmp/foo/bar.txt");
    /// let components: Vec<_> = path.components().map(|comp| comp.as_unix_str()).collect();
    // assert_eq!(&components, &[".", "tmp", "foo", "bar.txt"]);
    /// ```
    pub fn as_unix_str(self) -> &'a UnixStr {
        match self {
            Component::RootDir => UnixStr::new("/"),
            Component::CurDir => UnixStr::new("."),
            Component::ParentDir => UnixStr::new(".."),
            Component::Normal(path) => path,
        }
    }
}

impl AsRef<UnixStr> for Component<'_> {
    fn as_ref(&self) -> &UnixStr {
        self.as_unix_str()
    }
}

impl AsRef<UnixPath> for Component<'_> {
    fn as_ref(&self) -> &UnixPath {
        UnixPath::new(self.as_unix_str())
    }
}

/// Component parsing works by a double-ended state machine; the cursors at the
/// front and back of the path each keep track of what parts of the path have
/// been consumed so far.
///
/// Going front to back, a path is made up of a prefix, a starting
/// directory component, and a body (of normal components)
#[derive(Copy, Clone, PartialEq, PartialOrd, Debug)]
enum State {
    Prefix = 0,
    StartDir = 1, // / or . or nothing
    Body = 2,     // foo/bar/baz
    Done = 3,
}

/// Says whether the first byte after the prefix is a separator.
fn has_physical_root(path: &[u8]) -> bool {
    !path.is_empty() && path[0] == b'/'
}

impl AsRef<UnixPath> for UnixStr {
    fn as_ref(&self) -> &UnixPath {
        UnixPath::new(self)
    }
}

impl AsRef<UnixPath> for UnixString {
    fn as_ref(&self) -> &UnixPath {
        UnixPath::new(self.as_unix_str())
    }
}

impl AsRef<UnixPath> for String {
    fn as_ref(&self) -> &UnixPath {
        UnixPath::new(self.as_str())
    }
}

impl AsRef<UnixPath> for str {
    fn as_ref(&self) -> &UnixPath {
        UnixPath::new(self)
    }
}