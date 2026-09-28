use std::path::{Path, PathBuf};

/// The app is asked to open files or URLs: [`Event::AppOpen`](crate::event::Event::AppOpen).
///
/// Every way an app can be asked arrives as this one event, after
/// `Event::Startup`:
/// - [`AppOpenSource::Launch`]: the positional arguments of the process's
///   own command line (desktop).
/// - [`AppOpenSource::SecondInstance`]: a later launch of a single-instance
///   app (see `AppMain::single_instance`) handed its arguments to this
///   process and exited. `items` can be empty: the user started the app
///   again. The platform raises the app's windows first.
/// - [`AppOpenSource::System`]: the OS routed an open request to the running
///   app: macOS `application:openURLs:` (Finder, `open`, URL schemes), iOS
///   `application:openURL:options:`, Android `ACTION_VIEW` / `ACTION_SEND`
///   intents.
#[derive(Clone, Debug, PartialEq)]
pub struct AppOpenEvent {
    pub items: Vec<AppOpenItem>,
    pub source: AppOpenSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppOpenSource {
    Launch,
    SecondInstance,
    System,
}

/// One thing to open: a local file (always absolute when it came from a
/// command line), or a URL the platform could not turn into a file
/// (`https:`, a custom scheme, an Android `content:` URI, shared text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppOpenItem {
    Path(PathBuf),
    Url(String),
}

impl AppOpenEvent {
    /// The local files among the items.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.items.iter().filter_map(AppOpenItem::path)
    }
}

impl AppOpenItem {
    /// Reads one command-line argument or platform string: a URL when it
    /// starts with a scheme (a `file:` URL becomes its path), otherwise a
    /// path, joined to `cwd` when it is relative. A one-letter "scheme" is a
    /// Windows drive (`C:\x`), so a scheme needs two characters.
    pub fn parse(value: &str, cwd: Option<&Path>) -> AppOpenItem {
        if let Some(scheme_len) = url_scheme_len(value) {
            if value[..scheme_len].eq_ignore_ascii_case("file") {
                if let Some(path) = file_url_path(&value[scheme_len + 1..]) {
                    return AppOpenItem::Path(path);
                }
            }
            return AppOpenItem::Url(value.to_string());
        }
        let path = PathBuf::from(value);
        match cwd {
            Some(cwd) if path.is_relative() => AppOpenItem::Path(cwd.join(path)),
            _ => AppOpenItem::Path(path),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            AppOpenItem::Path(path) => Some(path),
            AppOpenItem::Url(_) => None,
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            AppOpenItem::Path(_) => None,
            AppOpenItem::Url(url) => Some(url),
        }
    }

    /// The item as one string, the form `parse` reads back.
    pub fn to_arg(&self) -> String {
        match self {
            AppOpenItem::Path(path) => path.to_string_lossy().into_owned(),
            AppOpenItem::Url(url) => url.clone(),
        }
    }
}

/// The length of `value`'s URL scheme (RFC 3986: a letter, then letters,
/// digits, `+`, `-`, `.`), when it has one of at least two characters.
fn url_scheme_len(value: &str) -> Option<usize> {
    let colon = value.find(':')?;
    let scheme = &value.as_bytes()[..colon];
    let valid = scheme.len() >= 2
        && scheme[0].is_ascii_alphabetic()
        && scheme
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    valid.then_some(colon)
}

/// The path of a `file:` URL's remainder (`//host/path` or `/path`),
/// percent-decoded. Only local hosts are paths.
fn file_url_path(rest: &str) -> Option<PathBuf> {
    let path = match rest.strip_prefix("//") {
        Some(authority_and_path) => {
            let slash = authority_and_path.find('/')?;
            let host = &authority_and_path[..slash];
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return None;
            }
            &authority_and_path[slash..]
        }
        None => rest,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    // `file:///C:/x` on Windows: the drive follows the leading slash.
    #[cfg(windows)]
    let path = match path.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => path[1..].to_string(),
        _ => path,
    };
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tells_paths_from_urls() {
        let cwd = Path::new("/home/u/docs");
        assert_eq!(
            AppOpenItem::parse("notes.md", Some(cwd)),
            AppOpenItem::Path(PathBuf::from("/home/u/docs/notes.md"))
        );
        assert_eq!(
            AppOpenItem::parse("/abs/a.png", Some(cwd)),
            AppOpenItem::Path(PathBuf::from("/abs/a.png"))
        );
        assert_eq!(
            AppOpenItem::parse("https://example.com/x?y=1", Some(cwd)),
            AppOpenItem::Url("https://example.com/x?y=1".into())
        );
        assert_eq!(
            AppOpenItem::parse("myapp://open/42", None),
            AppOpenItem::Url("myapp://open/42".into())
        );
        assert_eq!(
            AppOpenItem::parse("content://media/external/images/1", None),
            AppOpenItem::Url("content://media/external/images/1".into())
        );
        assert_eq!(
            AppOpenItem::parse("file:///tmp/a%20b.txt", None),
            AppOpenItem::Path(PathBuf::from("/tmp/a b.txt"))
        );
        assert_eq!(
            AppOpenItem::parse("file://localhost/tmp/c.txt", None),
            AppOpenItem::Path(PathBuf::from("/tmp/c.txt"))
        );
        // A remote file URL is not a local path.
        assert_eq!(
            AppOpenItem::parse("file://server/share/a", None),
            AppOpenItem::Url("file://server/share/a".into())
        );
        // A drive letter is not a scheme.
        assert!(matches!(AppOpenItem::parse("C:\\x\\y.pdf", None), AppOpenItem::Path(_)));
    }

    #[test]
    fn to_arg_round_trips() {
        for item in [
            AppOpenItem::Path(PathBuf::from("/a/b c.txt")),
            AppOpenItem::Url("https://example.com/".into()),
        ] {
            assert_eq!(AppOpenItem::parse(&item.to_arg(), None), item);
        }
    }
}
