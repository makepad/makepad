//! `shell/hotspot_linux.rs` — the Wi-Fi panel's hotspot mode, Linux only.
//!
//! The pure parts live here: the passphrase generator and its 0600 store,
//! the SSID, the Wi-Fi join payload for the QR code, the capability
//! decision, the client list, and the restore-the-old-network decision.
//! The D-Bus calls that act on them are the Wi-Fi worker's
//! (`wifi_linux.rs`, `Worker::hotspot_*`), because the worker owns the one
//! bus connection to iwd.
//!
//! # Why the hotspot REPLACES the Wi-Fi connection (strategy b)
//!
//! The target machine's radio can run an AP next to a station, but only on
//! the station's channel (`iw list`: `#{ managed } <= 2, #{ AP, ... } <= 1,
//! #channels <= 1`). iwd cannot use that: it keeps ONE netdev per phy
//! (`wlan0`), and a device is either a `Station` or an `AccessPoint`
//! (`net.connman.iwd.Device.Mode`, writable, `Adapter.SupportedModes`).
//! Switching the mode tears the station down. A second netdev for the same
//! phy has to be created with `iw dev ... interface add`, which needs root,
//! and iwd would then manage it as another station. So with iwd alone the
//! choice is (b): while the hotspot is on, the laptop has no Wi-Fi uplink
//! and the phone talks to the laptop directly (no internet for the phone).
//! Turning it off puts the device back in station mode and joins the
//! network that was connected before ([`restore_step`]).
//!
//! # Addressing
//!
//! iwd's `AccessPoint.Start(ssid, psk)` runs the AP but neither addresses
//! the interface nor serves DHCP; its own DHCP server needs a root-owned
//! profile in `/var/lib/iwd/ap/`. The machine already runs
//! systemd-networkd, which has a built-in DHCP server, so one root-owned
//! drop-in ([`networkd_dropin`]) matching `WLANInterfaceType=ap` gives the
//! hotspot interface [`ADDRESS`] and DHCP. No new daemon, no package. The
//! panel greys the switch out, with this reason, until the file exists.
//! The file is installed (and removed with `--remove`) by
//! `apps/wm/resources/hotspot/install-hotspot-network.sh`, run once as root;
//! its header has the first-test steps and the undo. A unit test keeps the
//! script's copy equal to [`networkd_dropin`].
//!
//! # The passphrase
//!
//! [`WORD_COUNT`] words of the in-tree list (`hotspot_words.txt`, 2048
//! common words of 4 to 8 letters), drawn with `getrandom(2)`: 4 x 11 bits
//! = 44 bits. Online guessing against WPA2 is rate-limited by the AP; 44
//! bits is far out of reach for that, and the list is easy to read out and
//! type on a phone. Generated once, stored `0600`, reused until the user
//! presses Regenerate.

use std::ffi::c_void;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The hotspot interface's address (the laptop's own on that network).
pub const ADDRESS: &str = "192.168.77.1";
/// Where the root-owned networkd drop-in goes.
pub const NETWORKD_FILE: &str = "/etc/systemd/network/20-makepad-hotspot.network";
/// Words in a passphrase.
pub const WORD_COUNT: usize = 4;
/// How long to wait for the old network to come back after the hotspot.
pub const RESTORE_SECS: f64 = 45.0;

// ======================================================================
// Passphrase
// ======================================================================

const WORDS_TEXT: &str = include_str!("hotspot_words.txt");

/// The wordlist, one entry per line of the text file.
pub fn words() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| WORDS_TEXT.lines().filter(|l| !l.is_empty()).collect())
}

/// A source of random bytes: the kernel's, or a fixed one in a test.
pub type Random<'a> = &'a mut dyn FnMut(&mut [u8]) -> Result<(), String>;

extern "C" {
    fn getrandom(buf: *mut c_void, len: usize, flags: u32) -> isize;
}

/// Fill `buf` from the kernel's CSPRNG (`getrandom(2)`, blocking until it
/// is seeded).
pub fn system_random(buf: &mut [u8]) -> Result<(), String> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = unsafe { getrandom(buf[filled..].as_mut_ptr() as *mut c_void, buf.len() - filled, 0) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("getrandom failed: {e}"));
        }
        filled += n as usize;
    }
    Ok(())
}

/// An unbiased index below `len` (rejection sampling on 16-bit draws).
fn pick_index(rng: Random, len: usize) -> Result<usize, String> {
    debug_assert!(len > 0 && len <= 1 << 16);
    let mask = len.next_power_of_two() - 1;
    loop {
        let mut two = [0u8; 2];
        rng(&mut two)?;
        let v = u16::from_le_bytes(two) as usize & mask;
        if v < len {
            return Ok(v);
        }
    }
}

/// `WORD_COUNT` random words joined with dashes.
pub fn generate_passphrase(rng: Random) -> Result<String, String> {
    let list = words();
    let mut out = String::new();
    for i in 0..WORD_COUNT {
        if i > 0 {
            out.push('-');
        }
        out.push_str(list[pick_index(rng, list.len())?]);
    }
    Ok(out)
}

/// A WPA passphrase iwd and phones accept: 8 to 63 printable ASCII.
pub fn passphrase_problem(p: &str) -> Option<&'static str> {
    if p.len() < 8 || p.len() > 63 {
        Some("a Wi-Fi passphrase has 8 to 63 characters")
    } else if !p.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        Some("a Wi-Fi passphrase uses printable ASCII only")
    } else {
        None
    }
}

// ======================================================================
// The 0600 store
// ======================================================================

const PASSPHRASE_FILE: &str = "passphrase";
const PREVIOUS_FILE: &str = "previous-network";

/// The hotspot's files: the passphrase, and the network the hotspot
/// replaced (so a later session can still put it back). Private to the
/// user: directory `0700`, files `0600`.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `$XDG_CONFIG_HOME/makepad/wm/hotspot` when `XDG_CONFIG_HOME` is
    /// absolute, else `$HOME/.config/makepad/wm/hotspot` (where the WM's
    /// other settings live).
    pub fn default_dir() -> Result<PathBuf, String> {
        let absolute = |key: &str| std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute());
        let root = match absolute("XDG_CONFIG_HOME") {
            Some(dir) => dir,
            None => absolute("HOME")
                .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is an absolute path".to_string())?
                .join(".config"),
        };
        Ok(root.join("makepad").join("wm").join("hotspot"))
    }

    fn write_private(&self, name: &str, contents: &str) -> Result<(), String> {
        let err = |what: &str, e: std::io::Error| format!("hotspot: {what} {}: {e}", self.dir.display());
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|e| err("cannot create", e))?;
        let tmp = self.dir.join(format!("{name}.new"));
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err("cannot clear", e)),
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| err("cannot write", e))?;
        file.write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| err("cannot write", e))?;
        drop(file);
        std::fs::rename(&tmp, self.dir.join(name)).map_err(|e| err("cannot replace the file in", e))
    }

    fn read_line(&self, name: &str) -> Result<Option<String>, String> {
        match std::fs::read_to_string(self.dir.join(name)) {
            Ok(text) => Ok(Some(text.trim_end_matches(['\n', '\r']).to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("hotspot: cannot read {}: {e}", self.dir.join(name).display())),
        }
    }

    /// The stored passphrase, creating one the first time.
    pub fn load_or_create(&self, rng: Random) -> Result<String, String> {
        match self.read_line(PASSPHRASE_FILE)? {
            Some(p) => match passphrase_problem(&p) {
                None => Ok(p),
                Some(why) => Err(format!(
                    "hotspot: the stored passphrase is not usable ({why}); press Regenerate"
                )),
            },
            None => {
                let p = generate_passphrase(rng)?;
                self.write_private(PASSPHRASE_FILE, &format!("{p}\n"))?;
                Ok(p)
            }
        }
    }

    /// A fresh passphrase, different from the stored one, replacing it.
    pub fn regenerate(&self, rng: Random) -> Result<String, String> {
        let old = self.read_line(PASSPHRASE_FILE).ok().flatten();
        for _ in 0..8 {
            let p = generate_passphrase(rng)?;
            if old.as_deref() != Some(p.as_str()) {
                self.write_private(PASSPHRASE_FILE, &format!("{p}\n"))?;
                return Ok(p);
            }
        }
        Err("hotspot: could not draw a different passphrase".into())
    }

    pub fn previous_network(&self) -> Option<String> {
        self.read_line(PREVIOUS_FILE).ok().flatten().filter(|s| !s.is_empty())
    }

    pub fn save_previous(&self, name: Option<&str>) -> Result<(), String> {
        match name {
            Some(n) if !n.contains(['\n', '\r']) => self.write_private(PREVIOUS_FILE, &format!("{n}\n")),
            _ => self.clear_previous(),
        }
    }

    pub fn clear_previous(&self) -> Result<(), String> {
        match std::fs::remove_file(self.dir.join(PREVIOUS_FILE)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("hotspot: cannot clear the previous network: {e}")),
        }
    }

    pub fn passphrase_path(&self) -> PathBuf {
        self.dir.join(PASSPHRASE_FILE)
    }
}

/// True when `path` is a file readable only by its owner.
pub fn is_private(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o777 == 0o600)
}

// ======================================================================
// SSID and the join code
// ======================================================================

/// `makepad-<hostname>` (a hostname that already starts `makepad-` is used
/// as is), the hostname reduced to letters, digits and
/// dashes, the whole name at most 32 bytes (the SSID limit).
pub fn ssid_for_host(host: &str) -> String {
    let clean: String = host
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let clean = if clean.is_empty() { "linux".to_string() } else { clean };
    // A host already named `makepad-...` is not announced as `makepad-makepad-...`.
    let mut ssid = if clean.to_ascii_lowercase().starts_with("makepad-") {
        clean
    } else {
        format!("makepad-{clean}")
    };
    ssid.truncate(32);
    ssid
}

pub fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default()
}

/// A value inside a `WIFI:` payload: `\ ; , : "` get a backslash.
pub fn escape_wifi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// What the QR code carries: phones join the network from it.
pub fn wifi_qr_payload(ssid: &str, passphrase: &str) -> String {
    format!("WIFI:T:WPA;S:{};P:{};;", escape_wifi(ssid), escape_wifi(passphrase))
}

// ======================================================================
// Addressing: the networkd drop-in
// ======================================================================

/// The file to install (as root, once) at [`NETWORKD_FILE`].
pub fn networkd_dropin() -> String {
    format!(
        "[Match]\nWLANInterfaceType=ap\n\n[Network]\nAddress={ADDRESS}/24\nDHCPServer=yes\n\
         ConfigureWithoutCarrier=yes\n\n[DHCPServer]\nEmitDNS=no\n"
    )
}

/// Does this `.network` file give an access-point interface [`ADDRESS`]
/// and a DHCP server?
pub fn networkd_enables_ap(text: &str) -> bool {
    let mut section = String::new();
    let (mut matches_ap, mut dhcp_server, mut address) = (false, false, false);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match (section.as_str(), key) {
            ("Match", "WLANInterfaceType") => {
                matches_ap = !value.starts_with('!') && value.split_whitespace().any(|t| t == "ap");
            }
            ("Network", "DHCPServer") => {
                dhcp_server = matches!(value, "yes" | "true" | "1" | "on");
            }
            ("Network", "Address") => {
                if value.starts_with(&format!("{ADDRESS}/")) {
                    address = true;
                }
            }
            _ => {}
        }
    }
    matches_ap && dhcp_server && address
}

/// The `.network` files networkd would read, in all its directories.
pub fn read_network_files() -> Vec<String> {
    let mut out = Vec::new();
    for dir in ["/etc/systemd/network", "/run/systemd/network", "/usr/lib/systemd/network"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "network") {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    out.push(t);
                }
            }
        }
    }
    out
}

// ======================================================================
// Capability
// ======================================================================

/// Whether the toggle is usable, and if not, why in a person's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    Available,
    Unsupported(String),
}

impl Default for Capability {
    fn default() -> Self {
        Capability::Unsupported("Checking the Wi-Fi adapter\u{2026}".into())
    }
}

/// What iwd (and networkd's files) say about this machine.
pub struct CapabilityInput<'a> {
    /// Why iwd cannot be used at all.
    pub unavailable: Option<&'a str>,
    pub device_present: bool,
    /// `Adapter.SupportedModes`, when iwd reported it.
    pub supported_modes: Option<&'a [String]>,
    /// A networkd file gives the AP interface its address and DHCP.
    pub addressing_ready: bool,
}

pub fn decide(input: &CapabilityInput) -> Capability {
    if let Some(why) = input.unavailable {
        return Capability::Unsupported(why.to_string());
    }
    if !input.device_present {
        return Capability::Unsupported("No Wi-Fi device".into());
    }
    match input.supported_modes {
        Some(modes) if modes.iter().any(|m| m == "ap") => {}
        Some(_) => return Capability::Unsupported("This Wi-Fi adapter cannot act as an access point".into()),
        None => return Capability::Unsupported("iwd did not report what the adapter supports".into()),
    }
    if !input.addressing_ready {
        return Capability::Unsupported(format!(
            "One-time setup needed: {NETWORKD_FILE} (address and DHCP for the hotspot)"
        ));
    }
    Capability::Available
}

// ======================================================================
// What the panel shows
// ======================================================================

/// A device that joined the hotspot, from the neighbour table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub ip: String,
    pub mac: String,
}

/// `/proc/net/arp` rows for `device` that completed resolution.
pub fn parse_arp(text: &str, device: &str) -> Vec<Client> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 || f[5] != device {
            continue;
        }
        let complete = u32::from_str_radix(f[2].trim_start_matches("0x"), 16).is_ok_and(|v| v & 0x2 != 0);
        if complete && f[3] != "00:00:00:00:00:00" {
            out.push(Client {
                ip: f[0].to_string(),
                mac: f[3].to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.ip.cmp(&b.ip));
    out
}

/// Everything the panel draws about the hotspot, published whole inside
/// the Wi-Fi snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotspotView {
    pub capability: Capability,
    /// The device is in access-point mode.
    pub ap_mode: bool,
    /// The access point is beaconing.
    pub started: bool,
    pub ssid: String,
    pub passphrase: Option<String>,
    pub ip: Option<String>,
    pub clients: Vec<Client>,
    /// The network the hotspot replaced, and is put back on turning it off.
    pub previous: Option<String>,
}

/// What iwd reported about the device, boiled down.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IwdFacts {
    pub device_present: bool,
    /// `Device.Mode`.
    pub mode: Option<String>,
    /// `AccessPoint.Started`, when the interface exists.
    pub ap_started: Option<bool>,
    /// `Adapter.SupportedModes`.
    pub supported_modes: Option<Vec<String>>,
}

impl IwdFacts {
    pub fn ap_mode(&self) -> bool {
        self.mode.as_deref() == Some("ap")
    }

    pub fn started(&self) -> bool {
        self.ap_mode() && self.ap_started == Some(true)
    }
}

/// Greedy word wrap by character count, at most `max_lines` lines (the
/// last one elided with `...` when the text does not fit).
pub fn wrap_text(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let extra = if cur.is_empty() { 0 } else { 1 };
        if !cur.is_empty() && cur.chars().count() + extra + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            last.push_str("...");
        }
    }
    lines
}

// ======================================================================
// Putting the old network back
// ======================================================================

/// One step of restoring the Wi-Fi connection after the hotspot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreStep {
    /// Connected to the wanted network: done.
    Done,
    /// Join the network at this index of the list.
    Connect(usize),
    /// Not listed yet (scan pending) or iwd is busy: look again later.
    Wait,
}

/// `networks` is (name, connected); `joining` is true while the station
/// is connecting or iwd has already picked a network by itself.
pub fn restore_step(want: &str, networks: &[(&str, bool)], joining: bool) -> RestoreStep {
    if networks.iter().any(|(n, c)| *n == want && *c) {
        return RestoreStep::Done;
    }
    if joining {
        return RestoreStep::Wait;
    }
    match networks.iter().position(|(n, _)| *n == want) {
        Some(i) => RestoreStep::Connect(i),
        None => RestoreStep::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wm-hotspot-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A counter-driven stand-in for the kernel's generator.
    fn counting(start: u8) -> impl FnMut(&mut [u8]) -> Result<(), String> {
        let mut n = start;
        move |buf: &mut [u8]| {
            for b in buf {
                *b = n;
                n = n.wrapping_mul(37).wrapping_add(11);
            }
            Ok(())
        }
    }

    #[test]
    fn wordlist_is_2048_unique_lowercase_words_of_4_to_8_letters() {
        let w = words();
        assert_eq!(w.len(), 2048, "11 bits per word");
        let mut sorted: Vec<&str> = w.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), w.len(), "unique");
        for word in w {
            assert!((4..=8).contains(&word.len()), "{word}");
            assert!(word.bytes().all(|b| b.is_ascii_lowercase()), "{word}");
        }
        assert_eq!(WORDS_TEXT.lines().count(), 2048, "no blank lines");
    }

    #[test]
    fn passphrase_is_four_dashed_words_and_a_valid_wpa_key() {
        let mut rng = counting(7);
        for _ in 0..50 {
            let p = generate_passphrase(&mut rng).unwrap();
            let parts: Vec<&str> = p.split('-').collect();
            assert_eq!(parts.len(), WORD_COUNT, "{p}");
            assert!(parts.iter().all(|w| words().contains(w)), "{p}");
            assert_eq!(passphrase_problem(&p), None, "{p}");
        }
    }

    #[test]
    fn kernel_random_gives_different_passphrases() {
        let a = generate_passphrase(&mut system_random).unwrap();
        let b = generate_passphrase(&mut system_random).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn index_choice_is_unbiased_for_a_non_power_of_two() {
        // 3000 is not a power of two: draws at or above it are rejected.
        let mut calls = 0;
        let mut rng = |buf: &mut [u8]| {
            calls += 1;
            let v: u16 = if calls == 1 { 3500 } else { 2999 };
            buf.copy_from_slice(&v.to_le_bytes());
            Ok(())
        };
        assert_eq!(pick_index(&mut rng, 3000).unwrap(), 2999 & (4096 - 1));
    }

    #[test]
    fn store_round_trip_creates_once_and_reuses() {
        let dir = scratch("roundtrip");
        let store = Store::at(dir.clone());
        let first = store.load_or_create(&mut counting(1)).unwrap();
        assert!(is_private(&store.passphrase_path()), "file is 0600");
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        // A second load, even with a different generator, returns the same.
        assert_eq!(store.load_or_create(&mut counting(99)).unwrap(), first);
        assert_eq!(
            std::fs::read_to_string(store.passphrase_path()).unwrap(),
            format!("{first}\n")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn regenerate_replaces_the_stored_passphrase_privately() {
        let dir = scratch("regen");
        let store = Store::at(dir.clone());
        let old = store.load_or_create(&mut counting(1)).unwrap();
        // The generator would draw the old passphrase again first: the
        // regenerate path must skip it and still end up different.
        let mut again = counting(1);
        let new = store.regenerate(&mut again).unwrap();
        assert_ne!(new, old);
        assert!(is_private(&store.passphrase_path()));
        assert_eq!(store.load_or_create(&mut counting(5)).unwrap(), new, "round trip");
        // Loosened permissions are tightened by the next replacement.
        std::fs::set_permissions(store.passphrase_path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let third = store.regenerate(&mut system_random).unwrap();
        assert_ne!(third, new);
        assert!(is_private(&store.passphrase_path()));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_stored_passphrase_is_reported_not_replaced() {
        let dir = scratch("bad");
        let store = Store::at(dir.clone());
        store.write_private(PASSPHRASE_FILE, "short\n").unwrap();
        assert!(store.load_or_create(&mut counting(1)).is_err());
        assert_eq!(std::fs::read_to_string(store.passphrase_path()).unwrap(), "short\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn previous_network_round_trip() {
        let dir = scratch("prev");
        let store = Store::at(dir.clone());
        assert_eq!(store.previous_network(), None);
        store.save_previous(Some("Home 5G")).unwrap();
        assert_eq!(store.previous_network().as_deref(), Some("Home 5G"));
        assert!(is_private(&dir.join(PREVIOUS_FILE)));
        store.save_previous(None).unwrap();
        assert_eq!(store.previous_network(), None);
        store.clear_previous().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ssid_names_the_host_and_fits_32_bytes() {
        assert_eq!(ssid_for_host("arch149\n"), "makepad-arch149");
        assert_eq!(ssid_for_host("my laptop.local"), "makepad-my-laptop-local");
        assert_eq!(ssid_for_host(""), "makepad-linux");
        assert_eq!(ssid_for_host("makepad-arch\n"), "makepad-arch");
        assert_eq!(ssid_for_host(&"a".repeat(80)).len(), 32);
    }

    #[test]
    fn wifi_payload_escapes_the_special_characters() {
        assert_eq!(escape_wifi(r#"a\b;c,d:e"f"#), r#"a\\b\;c\,d\:e\"f"#);
        assert_eq!(
            wifi_qr_payload("makepad-box", "harbor-pencil-garden-maple"),
            "WIFI:T:WPA;S:makepad-box;P:harbor-pencil-garden-maple;;"
        );
        assert_eq!(wifi_qr_payload("a;b", "p:q"), r"WIFI:T:WPA;S:a\;b;P:p\:q;;");
    }

    #[test]
    fn the_payload_fits_a_qr_code() {
        let long = wifi_qr_payload(&ssid_for_host(&"h".repeat(40)), "amber-violin-orchard-planet");
        assert!(super::super::qr::Qr::encode(long.as_bytes()).is_ok());
    }

    #[test]
    fn the_install_script_writes_exactly_the_drop_in() {
        let script = include_str!("../../resources/hotspot/install-hotspot-network.sh");
        let body = script.split("<<'NETWORK'\n").nth(1).and_then(|r| r.split("NETWORK\n# END").next()).unwrap();
        assert_eq!(body, networkd_dropin());
    }

    #[test]
    fn networkd_dropin_is_recognised_and_other_files_are_not() {
        assert!(networkd_enables_ap(&networkd_dropin()));
        let station = "[Match]\nName=wl*\n\n[Network]\nDHCP=ipv4\n";
        assert!(!networkd_enables_ap(station));
        let no_server = "[Match]\nWLANInterfaceType=ap\n[Network]\nAddress=192.168.77.1/24\n";
        assert!(!networkd_enables_ap(no_server));
        let inverted = "[Match]\nWLANInterfaceType=!ap\n[Network]\nAddress=192.168.77.1/24\nDHCPServer=yes\n";
        assert!(!networkd_enables_ap(inverted));
        let other_net = "[Match]\nWLANInterfaceType=ap\n[Network]\nAddress=10.1.1.1/24\nDHCPServer=yes\n";
        assert!(!networkd_enables_ap(other_net));
        let commented = "[Match]\n# WLANInterfaceType=ap\n[Network]\nAddress=192.168.77.1/24\nDHCPServer=yes\n";
        assert!(!networkd_enables_ap(commented));
    }

    fn input<'a>(modes: Option<&'a [String]>, ready: bool) -> CapabilityInput<'a> {
        CapabilityInput {
            unavailable: None,
            device_present: true,
            supported_modes: modes,
            addressing_ready: ready,
        }
    }

    #[test]
    fn capability_decision() {
        let ap: Vec<String> = ["ad-hoc", "station", "ap"].map(String::from).to_vec();
        let no_ap: Vec<String> = ["station"].map(String::from).to_vec();
        assert_eq!(decide(&input(Some(&ap), true)), Capability::Available);
        let Capability::Unsupported(r) = decide(&input(Some(&no_ap), true)) else { panic!() };
        assert!(r.contains("cannot act as an access point"), "{r}");
        let Capability::Unsupported(r) = decide(&input(Some(&ap), false)) else { panic!() };
        assert!(r.contains(NETWORKD_FILE), "{r}");
        let Capability::Unsupported(r) = decide(&input(None, true)) else { panic!() };
        assert!(r.contains("did not report"), "{r}");
        let mut gone = input(Some(&ap), true);
        gone.device_present = false;
        assert_eq!(decide(&gone), Capability::Unsupported("No Wi-Fi device".into()));
        let mut dead = input(Some(&ap), true);
        dead.unavailable = Some("iwd is not running");
        assert_eq!(decide(&dead), Capability::Unsupported("iwd is not running".into()));
    }

    #[test]
    fn iwd_facts_read_ap_state() {
        let mut f = IwdFacts {
            device_present: true,
            mode: Some("station".into()),
            ap_started: None,
            supported_modes: None,
        };
        assert!(!f.ap_mode() && !f.started());
        f.mode = Some("ap".into());
        assert!(f.ap_mode() && !f.started(), "mode set, access point not yet started");
        f.ap_started = Some(true);
        assert!(f.started());
    }

    #[test]
    fn arp_rows_for_the_hotspot_device_only() {
        let table = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.77.12    0x1         0x2         aa:bb:cc:dd:ee:01     *        wlan0
192.168.77.9     0x1         0x2         aa:bb:cc:dd:ee:02     *        wlan0
192.168.77.40    0x1         0x0         00:00:00:00:00:00     *        wlan0
10.0.0.1         0x1         0x2         11:22:33:44:55:66     *        eth0
";
        let c = parse_arp(table, "wlan0");
        assert_eq!(
            c,
            vec![
                Client { ip: "192.168.77.12".into(), mac: "aa:bb:cc:dd:ee:01".into() },
                Client { ip: "192.168.77.9".into(), mac: "aa:bb:cc:dd:ee:02".into() },
            ]
        );
        assert!(parse_arp(table, "wlan1").is_empty());
    }

    #[test]
    fn wrap_text_breaks_at_words_and_caps_lines() {
        assert_eq!(wrap_text("one two three", 7, 5), ["one two", "three"]);
        assert_eq!(wrap_text("", 10, 3), Vec::<String>::new());
        assert_eq!(wrap_text("aaa bbb ccc ddd", 3, 2), ["aaa", "bbb..."]);
        assert_eq!(wrap_text("unbroken-long-word", 5, 3), ["unbroken-long-word"]);
    }

    #[test]
    fn restore_waits_for_the_scan_then_joins_the_old_network() {
        let nets = [("Cafe", false), ("Home 5G", false)];
        assert_eq!(restore_step("Home 5G", &nets, false), RestoreStep::Connect(1));
        assert_eq!(restore_step("Home 5G", &nets, true), RestoreStep::Wait, "iwd is already joining");
        assert_eq!(restore_step("Gone", &nets, false), RestoreStep::Wait, "not listed yet");
        assert_eq!(restore_step("Home 5G", &[("Home 5G", true)], false), RestoreStep::Done);
        assert_eq!(restore_step("Home 5G", &[], false), RestoreStep::Wait);
    }
}
