//! `org.freedesktop.impl.portal.FileChooser` on the WM's portal backend (portal.rs): a
//! browser's upload or save dialog (Chromium asks xdg-desktop-portal, which asks us) opens the
//! WM's own picker (shell/file_picker.rs), the same one hosted makepad apps get.
//!
//! The call is decoded on the bus thread into a `RelayFileDialog` (the picker's request type),
//! the UI shows it, and the answer goes back as the method reply `(u response, a{sv} results)`:
//! 0 with `uris`, or 1 when cancelled. Linux only.
use std::ffi::CString;

use makepad_widgets::makepad_platform::studio::{RelayFileDialog, RelayFileDialogResult};

use crate::portal::{Interface, Write};
use crate::sdbus::{cstring, BusErr, Msg};

pub(crate) const NAME: &str = "org.freedesktop.impl.portal.FileChooser";

/// The interface to hand to `Portal::start`.
pub(crate) fn interface() -> Interface {
    Interface { name: NAME, xml: XML, decode }
}

/// The methods, as xdg-desktop-portal's impl FileChooser declares them.
const XML: &str = concat!(
    "<method name=\"OpenFile\"><arg type=\"o\" name=\"handle\" direction=\"in\"/><arg type=\"s\" name=\"app_id\" direction=\"in\"/>",
    "<arg type=\"s\" name=\"parent_window\" direction=\"in\"/><arg type=\"s\" name=\"title\" direction=\"in\"/><arg type=\"a{sv}\" name=\"options\" direction=\"in\"/>",
    "<arg type=\"u\" name=\"response\" direction=\"out\"/><arg type=\"a{sv}\" name=\"results\" direction=\"out\"/></method>",
    "<method name=\"SaveFile\"><arg type=\"o\" name=\"handle\" direction=\"in\"/><arg type=\"s\" name=\"app_id\" direction=\"in\"/>",
    "<arg type=\"s\" name=\"parent_window\" direction=\"in\"/><arg type=\"s\" name=\"title\" direction=\"in\"/><arg type=\"a{sv}\" name=\"options\" direction=\"in\"/>",
    "<arg type=\"u\" name=\"response\" direction=\"out\"/><arg type=\"a{sv}\" name=\"results\" direction=\"out\"/></method>",
);

/// `OpenFile` / `SaveFile` (handle o, app_id s, parent_window s, title s, options a{sv}) as the
/// picker's request. `SaveFiles` (several named files into one folder) is not served: the
/// caller gets UnknownMethod.
fn decode(member: &str, call: &Msg) -> Result<Box<dyn std::any::Any + Send>, BusErr> {
    let kind = match member {
        "OpenFile" => "select_file",
        "SaveFile" => "save_file",
        _ => return Err(BusErr::Dbus { name: "org.freedesktop.DBus.Error.UnknownMethod".into(), message: format!("FileChooser has no {member}") }),
    };
    let _handle = call.read_str(b'o')?;
    let app_id = call.read_str(b's')?;
    let _parent = call.read_str(b's')?;
    let title = call.read_str(b's')?;
    let options = read_options(call)?;
    let directory = options.directory && kind == "select_file";
    let request = RelayFileDialog {
        id: 0,
        kind: if directory { "select_folder".into() } else { kind.into() },
        title: Some(if title.is_empty() { app_id } else { title }),
        filename: options.current_name.or_else(|| options.current_file.as_ref().and_then(|f| f.rsplit('/').next().map(str::to_string))),
        location: options.current_folder.or_else(|| options.current_file.as_ref().and_then(|f| f.rsplit_once('/').map(|(dir, _)| dir.to_string()))),
        filters: options.filters,
        multiple: options.multiple,
    };
    Ok(Box::new(request))
}

#[derive(Default, Debug, PartialEq)]
struct Options {
    multiple: bool,
    directory: bool,
    current_name: Option<String>,
    current_folder: Option<String>,
    current_file: Option<String>,
    filters: Vec<(String, Vec<String>)>,
}

/// The `a{sv}` options the picker uses; the rest (accept_label, modal, choices,
/// current_filter) are read past.
fn read_options(m: &Msg) -> Result<Options, BusErr> {
    let mut o = Options::default();
    if !m.enter(b'a', c"{sv}")? {
        return Ok(o);
    }
    while m.enter(b'e', c"sv")? {
        let key = m.read_str(b's')?;
        let sig = m.peek()?.map(|(_, contents)| contents).unwrap_or_default();
        match (key.as_str(), sig.as_str()) {
            ("multiple", "b") | ("directory", "b") => {
                m.enter(b'v', c"b")?;
                let b = m.read_bool()?;
                m.exit()?;
                if key == "multiple" { o.multiple = b } else { o.directory = b }
            }
            ("current_name", "s") => {
                m.enter(b'v', c"s")?;
                o.current_name = Some(m.read_str(b's')?).filter(|s| !s.is_empty());
                m.exit()?;
            }
            ("current_folder", "ay") | ("current_file", "ay") => {
                m.enter(b'v', c"ay")?;
                let path = read_bytes_path(m)?;
                m.exit()?;
                if key == "current_folder" { o.current_folder = path } else { o.current_file = path }
            }
            ("filters", "a(sa(us))") => {
                m.enter(b'v', c"a(sa(us))")?;
                o.filters = read_filters(m)?;
                m.exit()?;
            }
            _ => m.skip()?,
        }
        m.exit()?;
    }
    m.exit()?;
    Ok(o)
}

/// A file name as bytes, NUL-terminated (`ay`).
fn read_bytes_path(m: &Msg) -> Result<Option<String>, BusErr> {
    let mut bytes = m.read_bytes()?;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    Ok((!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned()))
}

/// `a(sa(us))`: (name, [(0 glob | 1 mime type, pattern)]) -> (name, extensions). A glob
/// `*.png` is the extension `png`; `*` is everything; a mime type has no extension list here
/// and is left out (a filter of only mime types shows everything).
fn read_filters(m: &Msg) -> Result<Vec<(String, Vec<String>)>, BusErr> {
    let mut out = Vec::new();
    if !m.enter(b'a', c"(sa(us))")? {
        return Ok(out);
    }
    while m.enter(b'r', c"sa(us)")? {
        let name = m.read_str(b's')?;
        let mut patterns = Vec::new();
        if m.enter(b'a', c"(us)")? {
            while m.enter(b'r', c"us")? {
                let kind = m.read_num::<u32>(b'u')?;
                let pattern = m.read_str(b's')?;
                patterns.push((kind, pattern));
                m.exit()?;
            }
            m.exit()?;
        }
        m.exit()?;
        let exts = filter_extensions(&patterns);
        if !exts.is_empty() {
            out.push((name, exts));
        }
    }
    m.exit()?;
    Ok(out)
}

/// The extensions of a filter's glob patterns (`*.png` / `*.[pP][nN][gG]` -> `png`, `*` -> `*`).
fn filter_extensions(patterns: &[(u32, String)]) -> Vec<String> {
    let mut exts = Vec::new();
    for (kind, pattern) in patterns {
        if *kind != 0 {
            continue;
        }
        let ext = if pattern == "*" {
            "*".to_string()
        } else if let Some(rest) = pattern.strip_prefix("*.") {
            // GTK writes case-insensitive globs as [pP][nN][gG]: one letter per bracket.
            let mut ext = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                if c == '[' {
                    let inner: String = chars.by_ref().take_while(|c| *c != ']').collect();
                    ext.push(inner.chars().next().unwrap_or_default().to_ascii_lowercase());
                } else {
                    ext.push(c.to_ascii_lowercase());
                }
            }
            ext
        } else {
            continue;
        };
        if !ext.is_empty() && !exts.contains(&ext) {
            exts.push(ext);
        }
    }
    exts
}

/// `file://` URI of a path (RFC 3986: unreserved bytes and `/` as they are).
pub(crate) fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The method reply for the picker's answer: 0 + `uris`, or 1 (cancelled).
pub(crate) fn reply(result: &RelayFileDialogResult) -> Write {
    let uris: Vec<String> = result.paths.iter().map(|p| file_uri(p)).collect();
    Box::new(move |m: &Msg| {
        m.append_num(b'u', if uris.is_empty() { 1u32 } else { 0u32 })?;
        m.open(b'a', c"{sv}")?;
        if !uris.is_empty() {
            m.open(b'e', c"sv")?;
            m.append_str(b's', c"uris")?;
            m.open(b'v', c"as")?;
            m.open(b'a', c"s")?;
            for uri in &uris {
                let uri: CString = cstring(uri)?;
                m.append_str(b's', &uri)?;
            }
            m.close()?;
            m.close()?;
            m.close()?;
        }
        m.close()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_become_extensions() {
        let p = |v: &[(u32, &str)]| v.iter().map(|(k, s)| (*k, s.to_string())).collect::<Vec<_>>();
        assert_eq!(filter_extensions(&p(&[(0, "*.png"), (0, "*.[jJ][pP][gG]"), (1, "image/webp")])), ["png", "jpg"]);
        assert_eq!(filter_extensions(&p(&[(0, "*")])), ["*"]);
        assert!(filter_extensions(&p(&[(1, "image/*")])).is_empty());
    }

    #[test]
    fn file_uris_escape_what_a_uri_cannot_hold() {
        assert_eq!(file_uri("/home/a/My File #1.txt"), "file:///home/a/My%20File%20%231.txt");
        assert_eq!(file_uri("/tmp/ü"), "file:///tmp/%C3%BC");
    }
}
