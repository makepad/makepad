//! This machine's USB serial devices, and which of them may be opened: `serial.conf`.
//!
//! Listing reads the OS's registry and never opens a port: opening one asserts DTR, which resets
//! an Arduino. The person's file says which devices are off; software reads it at every listing
//! and before every open. Agents never write it; devhub's master writes only its `serve` line
//! ([`set_serve`], from a box's `serial=on` in nodes.conf), every other line as the person left
//! it. `$MAKEPAD_HOME/devhub/serial.conf`, else `~/.makepad/devhub/serial.conf`:
//!
//! ```text
//! serve = on                            # devhub serves this machine's devices; absent or off: none
//! off 1a86:7523 port=3-1.1  fan PWM     # <on|off> <vid>:<pid> <serial=S|port=P> [note]
//! ```
//!
//! A `port=` value is the USB topology name (`3-1.1` on Linux, the location id `0x14100000` on
//! macOS) or, on Linux, the device's path below `/sys/devices` (its controller and port chain,
//! which bus renumbering does not change). A line that names a device turns it on or off; a
//! device no line names is on. Devices without a serial number of their own are told apart by
//! port only, so the rules lean towards off for them (`verdict`).
//!
//! Off by default, on for our boxes: a box with no file, or without `serve = on`, offers nothing
//! over the network ([`Conf::serves`] is false). "All on" for an absent file only means the
//! person's own app listing and choosing the devices on that same machine, never devhub offering
//! them to the fleet.

use makepad_usb::DeviceInfo;
use std::io;
use std::path::PathBuf;

/// What tells a USB device apart for serial.conf: its kind, its serial number and its port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub vendor: u16,
    pub product: u16,
    pub serial: Option<String>,
    /// The topology name (`3-1.1`; a macOS location id).
    pub port: String,
    /// The port as a path bus renumbering does not change (`makepad_usb::DeviceInfo::location`).
    pub location: String,
}

impl Ident {
    pub fn of(usb: &DeviceInfo) -> Ident {
        Ident { vendor: usb.vendor, product: usb.product, serial: usb.serial.clone(), port: usb.port.clone(), location: usb.location.clone() }
    }
}

/// A USB serial device's port, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialDevice {
    /// The node to open (`/dev/cu.usbmodem1101`, `/dev/ttyACM0`).
    pub path: String,
    pub ident: Ident,
    /// The product and maker strings, when the device has them.
    pub name: Option<String>,
    pub manufacturer: Option<String>,
    /// Whether it may be opened, and why not.
    pub verdict: Verdict,
}

impl SerialDevice {
    /// The USB device, across listings and replugs into the same port: `<vid>:<pid>/serial=<S>`,
    /// or `<vid>:<pid>/port=<P>` for one without a serial number of its own (serial.conf's words).
    pub fn id(&self) -> String {
        let at = match &self.ident.serial {
            Some(serial) if !serial.is_empty() => format!("serial={serial}"),
            _ => format!("port={}", self.ident.port),
        };
        format!("{:04x}:{:04x}/{at}", self.ident.vendor, self.ident.product)
    }

    /// A line for a person: the product (or VID:PID), its maker, and the port.
    pub fn label(&self) -> String {
        let name = self.name.clone().unwrap_or_else(|| format!("{:04x}:{:04x}", self.ident.vendor, self.ident.product));
        let node = self.path.rsplit('/').next().unwrap_or(&self.path);
        match &self.manufacturer {
            Some(maker) if !name.contains(maker.as_str()) => format!("{name} ({maker}) on {node}"),
            _ => format!("{name} on {node}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    On,
    /// Off, and why, in words for a person.
    Off(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    Serial(String),
    Port(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    on: bool,
    vendor: u16,
    product: u16,
    key: Key,
    note: String,
}

/// The settings as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conf(State);

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// No file: devhub serves nothing, and every device is on for this machine's own use.
    Absent,
    Read { serve: bool, rules: Vec<Rule> },
    /// A file that does not read protects hardware by turning every device off, naming the line.
    Bad(String),
}

impl Conf {
    /// The file's path (`MAKEPAD_HOME` moves it, as it moves the rest of devhub's state). `~` is
    /// `HOME`, on Windows `USERPROFILE` (where devhub keeps its own state there; a Windows box has
    /// no `HOME`).
    pub fn path() -> Option<PathBuf> {
        match std::env::var_os("MAKEPAD_HOME").filter(|home| !home.is_empty()) {
            Some(home) => Some(PathBuf::from(home).join("devhub/serial.conf")),
            None => std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".makepad/devhub/serial.conf")),
        }
    }

    /// This machine's settings, read now.
    pub fn read() -> Conf {
        let Some(path) = Conf::path() else { return Conf(State::Absent) };
        match std::fs::read_to_string(&path) {
            Ok(text) => Conf::parse(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Conf(State::Absent),
            Err(err) => Conf(State::Bad(format!("{} does not read: {err}", path.display()))),
        }
    }

    /// The settings in `text`; no text at all is a file that says nothing (serve off, all on).
    pub fn parse(text: &str) -> Conf {
        let mut serve = false;
        let mut rules = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = |why: &str| Conf(State::Bad(format!("serial.conf line {}: {why}: {raw}", n + 1)));
            if let Some(value) = line.strip_prefix("serve").map(str::trim).and_then(|rest| rest.strip_prefix('=')) {
                serve = match value.trim() {
                    "on" => true,
                    "off" => false,
                    _ => return bad("serve is on or off"),
                };
                continue;
            }
            let mut words = line.split_whitespace();
            let on = match words.next() {
                Some("on") => true,
                Some("off") => false,
                _ => return bad("a device line starts with on or off"),
            };
            let Some((vendor, product)) = words.next().and_then(|id| id.split_once(':'))
                .and_then(|(v, p)| Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?)))
            else {
                return bad("the device is <vid>:<pid> in hex");
            };
            let key = match words.next() {
                Some(word) if word.starts_with("serial=") && word.len() > 7 => Key::Serial(word[7..].to_owned()),
                Some(word) if word.starts_with("port=") && word.len() > 5 => Key::Port(word[5..].to_owned()),
                _ => return bad("the device is named by serial=<S> or port=<P>"),
            };
            rules.push(Rule { on, vendor, product, key, note: words.collect::<Vec<_>>().join(" ") });
        }
        Conf(State::Read { serve, rules })
    }

    /// No file at all.
    pub fn absent() -> Conf {
        Conf(State::Absent)
    }

    /// Whether devhub serves this machine's devices to the fleet.
    pub fn serves(&self) -> bool {
        matches!(self.0, State::Read { serve: true, .. })
    }

    /// Whether `device` may be opened, given every USB device attached now (`all`, the same
    /// listing). Rules, most protective first:
    /// - an `off` line that names the device turns it off;
    /// - a device without a serial number of its own (none, or one another attached device of
    ///   its kind shares) is off while an `off ... port=` device of its kind is missing from its
    ///   port, whatever `on` lines say: the protected one may be this one, moved;
    /// - such a device is also off while any `off` line names its kind, unless an `on` line names
    ///   it;
    /// - anything else is on.
    pub fn verdict(&self, device: &Ident, all: &[Ident]) -> Verdict {
        let rules = match &self.0 {
            State::Absent => return Verdict::On,
            State::Bad(why) => return Verdict::Off(why.clone()),
            State::Read { rules, .. } => rules,
        };
        let kind = |rule: &&Rule| rule.vendor == device.vendor && rule.product == device.product;
        if let Some(rule) = rules.iter().filter(kind).find(|rule| !rule.on && names(rule, device)) {
            return Verdict::Off(off_text(&rule.note, "off in serial.conf"));
        }
        if !own_serial(device, all) {
            let moved = rules.iter().filter(kind).find(|rule| !rule.on && matches!(rule.key, Key::Port(_))
                && !all.iter().any(|other| other.vendor == rule.vendor && other.product == rule.product && names(rule, other)));
            if let Some(rule) = moved {
                return Verdict::Off(off_text(&rule.note, "a protected device of this kind is not at its port; it may be this one"));
            }
            let named_on = rules.iter().filter(kind).any(|rule| rule.on && names(rule, device));
            if !named_on {
                if let Some(rule) = rules.iter().filter(kind).find(|rule| !rule.on) {
                    return Verdict::Off(off_text(&rule.note, "off: same kind as a protected device"));
                }
            }
        }
        Verdict::On
    }
}

fn off_text(note: &str, why: &str) -> String {
    if note.is_empty() { why.to_owned() } else { format!("{why} ({note})") }
}

/// A rule names a device by its serial number, its topology name or (Linux) its controller path.
fn names(rule: &Rule, device: &Ident) -> bool {
    match &rule.key {
        Key::Serial(serial) => device.serial.as_deref() == Some(serial.as_str()),
        Key::Port(port) => *port == device.port || *port == device.location,
    }
}

/// The device has a serial number no other attached device of its kind shares.
/// (Two attached devices never share a port.)
fn own_serial(device: &Ident, all: &[Ident]) -> bool {
    let Some(serial) = device.serial.as_deref().filter(|serial| !serial.is_empty()) else { return false };
    !all.iter().any(|other| other.port != device.port && other.vendor == device.vendor && other.product == device.product
        && other.serial.as_deref() == Some(serial))
}

/// This machine's USB serial ports, each with its verdict under `conf`. Never opens a port.
pub fn list(conf: &Conf) -> io::Result<Vec<SerialDevice>> {
    let usb = makepad_usb::devices()?;
    let all: Vec<Ident> = usb.iter().map(Ident::of).collect();
    let mut ports: Vec<SerialDevice> = usb.iter().zip(&all).flat_map(|(usb, ident)| usb.serial_ports.iter().map(|path| SerialDevice {
        path: path.clone(), ident: ident.clone(), name: usb.name.clone(), manufacturer: usb.manufacturer.clone(),
        verdict: conf.verdict(ident, &all),
    })).collect();
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ports)
}

/// The node of the device at `path` now, checked against serial.conf as read now: what to open.
/// Off, gone or changed (another device at that node since it was listed) is an error.
pub fn resolve(listed: &SerialDevice) -> io::Result<String> {
    let conf = Conf::read();
    let now = list(&conf)?;
    let found = now.iter().find(|device| device.path == listed.path && same_device(&device.ident, &listed.ident))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} is gone or another device is at {}", listed.label(), listed.path)))?;
    match &found.verdict {
        Verdict::On => Ok(found.path.clone()),
        Verdict::Off(why) => Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{}: {why}", found.label()))),
    }
}

/// The same physical device across listings (the OS's ids change on replug; these do not).
fn same_device(a: &Ident, b: &Ident) -> bool {
    a.vendor == b.vendor && a.product == b.product && a.serial == b.serial && a.port == b.port
}

/// `text` with its `serve` line saying `on` or `off`: the first one changed in place (its comment
/// kept), or one added at the top when it has none. Every other line stays byte for byte.
pub fn set_serve(text: &str, on: bool) -> String {
    let want = if on { "serve = on" } else { "serve = off" };
    let mut out = String::with_capacity(text.len() + want.len() + 1);
    let mut done = false;
    for raw in text.split_inclusive('\n') {
        let body = raw.split('#').next().unwrap_or("").trim();
        let is_serve = body.strip_prefix("serve").map(str::trim_start).is_some_and(|rest| rest.starts_with('='));
        if is_serve && !done {
            done = true;
            let comment = raw.find('#').map(|i| raw[i..].trim_end_matches(['\n', '\r'])).unwrap_or("");
            let end = &raw[raw.trim_end_matches(['\n', '\r']).len()..];
            out.push_str(want);
            if !comment.is_empty() {
                out.push_str("  ");
                out.push_str(comment);
            }
            out.push_str(end);
        } else {
            out.push_str(raw);
        }
    }
    if !done {
        return format!("{want}\n{text}");
    }
    out
}

#[cfg(test)]
mod tests {
    /// devhub sets only the serve line: the person's device lines and comments stay as they were,
    /// and the result reads back as that serve.
    #[test]
    fn set_serve_changes_only_the_serve_line() {
        let person = "# the bench\noff 1a86:7523 port=3-1.1  fan PWM\nserve = off   # devhub\non 303a:1001 serial=AB\n";
        let on = super::set_serve(person, true);
        assert_eq!(on, "# the bench\noff 1a86:7523 port=3-1.1  fan PWM\nserve = on  # devhub\non 303a:1001 serial=AB\n");
        assert!(super::Conf::parse(&on).serves());
        assert_eq!(super::set_serve(&on, true), on, "already on: unchanged");
        let added = super::set_serve("off 1a86:7523 port=3-1.1\n", true);
        assert_eq!(added, "serve = on\noff 1a86:7523 port=3-1.1\n");
        assert!(!super::Conf::parse(&super::set_serve(&added, false)).serves());
        assert_eq!(super::set_serve("", true), "serve = on\n");
    }
}
