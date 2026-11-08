//! How a card holds the displays it drives. A driver that grants a display to the descriptor it
//! was acquired through, and will not grant it through that descriptor again, gets one DRM lease
//! per display (its connector and one CRTC): giving a display back revokes that lease alone, and
//! a reconnect acquires through a fresh one. A display WSI that takes one descriptor for the
//! whole GPU (`mesa`) is acquired once per card, through the card's master descriptor.
use makepad_vulkan::vk;
use std::collections::{HashMap, HashSet};

/// One CRTC (display head) of a card, as DRM shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Head {
    pub crtc: u32,
    /// The connector it lights (0: none), and whether that connector is connected.
    pub lights: u32,
    pub connected: bool,
}

/// A card's heads in DRM order, and each connector's possible heads (bit i: `heads[i]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct KmsState {
    pub heads: Vec<Head>,
    pub possible: HashMap<u32, u32>,
}

/// How a connector that found no head (or plane) to take fails: it is retried when a monitor
/// is unplugged or turned off.
pub(super) const NO_FREE_HEAD: &str = "no free display head";

/// The head a connector takes, and what has to give it up first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    /// The head already lighting this connector, or one lighting no connected connector.
    Free(u32),
    /// A head held for a turned-off display: that display is given back first.
    Parked { crtc: u32, display: vk::DisplayKHR },
    /// A head the console lights for a connector the WM does not drive: switched off first.
    Console { crtc: u32, connector: u32 },
}

/// The head `connector` takes, in this order: the head already lighting it, else the first
/// possible head lighting no connected connector (the way the driver's own grant picks one),
/// else a turned-off display's head, else a head the console lights for a connector the WM
/// does not drive. The last two happen only when no head is free: nothing is taken up front.
/// `ours`: the heads this card's leases hold; `driven`: the connectors the WM drives or holds;
/// `parked`: the heads of turned-off displays.
pub(super) fn pick_head(connector: u32, kms: &KmsState, ours: &HashSet<u32>, driven: &HashSet<u32>, parked: &[(u32, vk::DisplayKHR)]) -> Option<Pick> {
    let possible = kms.possible.get(&connector).copied().unwrap_or(0);
    let possible = |crtc: u32| kms.heads.iter().position(|h| h.crtc == crtc).is_some_and(|i| i < 32 && possible & (1 << i) != 0);
    let lit_by_other = |h: &Head| h.lights != 0 && h.lights != connector && h.connected;
    let open = |h: &Head| possible(h.crtc) && !ours.contains(&h.crtc) && !(lit_by_other(h) && driven.contains(&h.lights));
    if let Some(h) = kms.heads.iter().find(|h| h.lights == connector && !ours.contains(&h.crtc)) { return Some(Pick::Free(h.crtc)); }
    if let Some(h) = kms.heads.iter().find(|h| open(h) && !lit_by_other(h)) { return Some(Pick::Free(h.crtc)); }
    if let Some(&(crtc, display)) = parked.iter().find(|(crtc, _)| possible(*crtc)) { return Some(Pick::Parked { crtc, display }); }
    kms.heads.iter().find(|h| open(h) && lit_by_other(h)).map(|h| Pick::Console { crtc: h.crtc, connector: h.lights })
}

/// The DRM and Vulkan calls a card makes to hold its displays.
pub(super) trait CardDriver {
    type Lease;
    fn kms(&mut self) -> Result<KmsState, String>;
    /// Switch a CRTC off through the master (the console on it goes dark).
    fn switch_off(&mut self, crtc: u32) -> Result<(), String>;
    /// A lease of `connector` and `crtc` from the card's master descriptor.
    fn lease(&mut self, connector: u32, crtc: u32) -> Result<Self::Lease, String>;
    /// Revoke the lease and close its descriptor.
    fn revoke(&mut self, lease: Self::Lease);
    /// Acquire `display` through `lease`, or (`mesa`) through a duplicate of the master descriptor.
    fn acquire(&mut self, display: vk::DisplayKHR, lease: Option<&Self::Lease>) -> Result<(), String>;
    fn release(&mut self, display: vk::DisplayKHR);
}

pub(super) struct Held<L> {
    pub display: vk::DisplayKHR,
    pub connector: u32,
    /// The head its lease holds (none on a one-descriptor card).
    pub crtc: Option<u32>,
    /// Turned off: held dark, its head given to another connector that finds none free.
    pub parked: bool,
    lease: Option<L>,
}

/// The displays one card holds.
pub(super) struct Holds<L> {
    /// The display WSI takes one descriptor for the whole GPU: one acquisition covers the card,
    /// and a release of any display ends it, so the card is released with its last display only.
    pub mesa: bool,
    pub displays: Vec<Held<L>>,
}

impl<L> Held<L> {
    pub fn lease(&self) -> Option<&L> { self.lease.as_ref() }
}

impl<L> Holds<L> {
    pub fn new(mesa: bool) -> Self { Self { mesa, displays: Vec::new() } }
    pub fn holds(&self, display: vk::DisplayKHR) -> bool { self.displays.iter().any(|h| h.display == display) }
    /// A turned-off display stays held, dark, with its head and plane, until another
    /// connector needs that head.
    pub fn park(&mut self, display: vk::DisplayKHR) {
        if let Some(held) = self.displays.iter_mut().find(|h| h.display == display) { held.parked = true; }
    }
    /// Hold `display` of `connector`. `driven`: the other connectors the WM drives on this card
    /// (a console on one of them is never switched off). Returns the turned-off displays given
    /// back for its head.
    pub fn acquire(&mut self, driver: &mut impl CardDriver<Lease = L>, display: vk::DisplayKHR, connector: u32, driven: &HashSet<u32>) -> Result<Vec<vk::DisplayKHR>, String> {
        if let Some(held) = self.displays.iter_mut().find(|h| h.display == display) {
            held.parked = false;
            return Ok(Vec::new());
        }
        let kms = driver.kms()?;
        let ours: HashSet<u32> = self.displays.iter().filter_map(|h| h.crtc).collect();
        let driven: HashSet<u32> = driven.iter().copied().chain(self.displays.iter().map(|h| h.connector)).collect();
        let parked: Vec<_> = self.displays.iter().filter(|h| h.parked).filter_map(|h| Some((h.crtc?, h.display))).collect();
        let mut given_back = Vec::new();
        let crtc = match pick_head(connector, &kms, &ours, &driven, &parked) {
            Some(Pick::Free(crtc)) => Some(crtc),
            Some(Pick::Parked { crtc, display: off }) => {
                eprintln!("[hal-direct] connector {connector}: no free display head; the turned-off display on CRTC {crtc} gives it up");
                self.release(driver, off);
                given_back.push(off);
                Some(crtc)
            }
            Some(Pick::Console { crtc, connector: lit }) => {
                eprintln!("[hal-direct] connector {connector}: no free display head; switching off CRTC {crtc}, the console on connector {lit} (not driven)");
                Some(crtc)
            }
            None => None,
        };
        // DRM still shows the head lighting another connector (a console, a turned-off or an
        // unplugged monitor): the master switches it off first, or the driver will not grant it.
        if let Some(crtc) = crtc.filter(|crtc| kms.heads.iter().any(|h| h.crtc == *crtc && h.lights != 0 && h.lights != connector)) {
            driver.switch_off(crtc)?;
        }
        if self.mesa {
            // The WSI picks the CRTC at its first modeset: the connector's own, or one showing nothing.
            if self.displays.is_empty() { driver.acquire(display, None)?; }
            self.displays.push(Held { display, connector, crtc: None, parked: false, lease: None });
            return Ok(given_back);
        }
        let crtc = crtc.ok_or_else(|| format!("{NO_FREE_HEAD} for connector {connector}: every head it can use drives another monitor"))?;
        let lease = driver.lease(connector, crtc)?;
        if let Err(e) = driver.acquire(display, Some(&lease)) {
            driver.revoke(lease);
            return Err(e);
        }
        self.displays.push(Held { display, connector, crtc: Some(crtc), parked: false, lease: Some(lease) });
        Ok(given_back)
    }
    /// Give one display back; every other display keeps its acquisition and its scanout.
    pub fn release(&mut self, driver: &mut impl CardDriver<Lease = L>, display: vk::DisplayKHR) {
        let Some(index) = self.displays.iter().position(|h| h.display == display) else { return };
        let held = self.displays.remove(index);
        if !self.mesa || self.displays.is_empty() { driver.release(display); }
        // The display goes first: its lease's descriptor stays open until it is released.
        if let Some(lease) = held.lease { driver.revoke(lease); }
    }
    pub fn release_all(&mut self, driver: &mut impl CardDriver<Lease = L>) {
        for (index, held) in std::mem::take(&mut self.displays).into_iter().enumerate() {
            if !self.mesa || index == 0 { driver.release(held.display); }
            if let Some(lease) = held.lease { driver.revoke(lease); }
        }
    }
}

/// The display planes this session holds, per GPU: plane -> the display we hold on it. A
/// display keeps its plane while it is held (shown, rebuilding or turned off) and leaves the
/// book when it is given back.
#[derive(Default)]
pub(super) struct PlaneBook { planes: HashMap<(vk::PhysicalDevice, u32), vk::DisplayKHR> }
impl PlaneBook {
    /// `display` is shown on `plane` (and on no other).
    pub fn show(&mut self, gpu: vk::PhysicalDevice, plane: u32, display: vk::DisplayKHR) {
        self.planes.retain(|_, d| *d != display);
        self.planes.insert((gpu, plane), display);
    }
    /// Only displays still held keep their planes.
    pub fn keep(&mut self, held: impl Fn(vk::DisplayKHR) -> bool) { self.planes.retain(|_, d| held(*d)); }
    pub fn clear(&mut self) { self.planes.clear(); }
    /// One GPU's planes: plane -> the display we hold on it.
    pub fn of(&self, gpu: vk::PhysicalDevice) -> HashMap<u32, vk::DisplayKHR> {
        self.planes.iter().filter(|((g, _), _)| *g == gpu).map(|((_, p), d)| (*p, *d)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::kms::{assign_planes, mode_ladder, plane_order, Demotions, DrmConnector, PlaneRow};

    fn display(connector: u32) -> vk::DisplayKHR { vk::DisplayKHR(u64::from(connector)) }
    fn none() -> HashSet<u32> { HashSet::new() }

    /// A card whose driver grants per descriptor: a connector granted through a descriptor is not
    /// granted again through it until that descriptor closes (a release leaves the grant), a
    /// lessee is granted only its leased head, and a head lighting another connected connector
    /// (the console) is never granted. Heads are CRTCs 100.., the console lighting the i-th
    /// connector on the i-th head; every connector can use every head.
    struct PerDescriptor {
        kms: KmsState,
        connected: HashSet<u32>,
        /// Open lease descriptors: fd -> (lessee's connector, CRTC).
        leases: HashMap<i32, (u32, u32)>,
        /// Per descriptor, the connectors ever granted through it while it was open.
        granted: HashMap<i32, HashSet<u32>>,
        /// The displays acquired now, and the descriptor each came through.
        acquired: HashMap<vk::DisplayKHR, i32>,
        switched_off: Vec<u32>,
        next_fd: i32,
    }
    impl PerDescriptor {
        fn new(heads: u32, lit: &[u32]) -> Self {
            let heads = (0..heads as usize).map(|i| Head { crtc: 100 + i as u32, lights: lit.get(i).copied().unwrap_or(0), connected: i < lit.len() }).collect();
            let mut card = Self { kms: KmsState { heads, possible: HashMap::new() }, connected: HashSet::new(),
                leases: HashMap::new(), granted: HashMap::new(), acquired: HashMap::new(), switched_off: Vec::new(), next_fd: 10 };
            for &c in lit { card.plug(c); }
            card
        }
        fn unplug(&mut self, connector: u32) {
            self.connected.remove(&connector);
            for head in &mut self.kms.heads { if head.lights == connector { head.connected = false; } }
        }
        fn plug(&mut self, connector: u32) {
            self.connected.insert(connector);
            self.kms.possible.insert(connector, (1u32 << self.kms.heads.len()) - 1);
            for head in &mut self.kms.heads { if head.lights == connector { head.connected = true; } }
        }
        fn crtc_of(&self, connector: u32) -> Option<u32> { self.leases.values().find(|(c, _)| *c == connector).map(|(_, h)| *h) }
    }
    impl CardDriver for PerDescriptor {
        type Lease = i32;
        fn kms(&mut self) -> Result<KmsState, String> { Ok(self.kms.clone()) }
        fn switch_off(&mut self, crtc: u32) -> Result<(), String> {
            let head = self.kms.heads.iter_mut().find(|h| h.crtc == crtc).unwrap();
            head.lights = 0; head.connected = false;
            self.switched_off.push(crtc);
            Ok(())
        }
        fn lease(&mut self, connector: u32, crtc: u32) -> Result<i32, String> {
            if self.leases.values().any(|&(c, h)| c == connector || h == crtc) { return Err("EBUSY".into()); }
            self.next_fd += 1;
            self.leases.insert(self.next_fd, (connector, crtc));
            Ok(self.next_fd)
        }
        fn revoke(&mut self, fd: i32) {
            assert!(!self.acquired.values().any(|&f| f == fd), "a lease closed under an acquired display");
            assert!(self.leases.remove(&fd).is_some(), "revoked twice");
            self.granted.remove(&fd);
        }
        fn acquire(&mut self, display: vk::DisplayKHR, lease: Option<&i32>) -> Result<(), String> {
            let fd = *lease.expect("granted per descriptor: every display comes through its lease");
            let (connector, crtc) = self.leases[&fd];
            assert_eq!(display, super::tests::display(connector));
            if !self.connected.contains(&connector) { return Err("not connected".into()); }
            if !self.granted.entry(fd).or_default().insert(connector) { return Err("ERROR_INITIALIZATION_FAILED: granted before through this descriptor".into()); }
            let head = self.kms.heads.iter().find(|h| h.crtc == crtc).unwrap();
            if head.lights != 0 && head.lights != connector && head.connected { return Err("ERROR_INITIALIZATION_FAILED: the head lights another connector".into()); }
            self.acquired.insert(display, fd);
            Ok(())
        }
        fn release(&mut self, display: vk::DisplayKHR) {
            assert!(self.acquired.remove(&display).is_some(), "released a display not held");
        }
    }

    /// A one-descriptor display WSI: a second acquisition through another descriptor is refused,
    /// and a release of any display ends the acquisition. Four heads, none lit.
    #[derive(Default)]
    struct OneDescriptor { acquired: bool, acquires: usize, releases: usize }
    impl CardDriver for OneDescriptor {
        type Lease = ();
        fn kms(&mut self) -> Result<KmsState, String> {
            Ok(KmsState { heads: (0..4).map(|i| Head { crtc: 100 + i, lights: 0, connected: false }).collect(), possible: (1..=4).map(|c| (c, 0b1111)).collect() })
        }
        fn switch_off(&mut self, _: u32) -> Result<(), String> { unreachable!("a free head is left") }
        fn lease(&mut self, _: u32, _: u32) -> Result<(), String> { unreachable!("no leases on a one-descriptor card") }
        fn revoke(&mut self, _: ()) { unreachable!() }
        fn acquire(&mut self, _: vk::DisplayKHR, lease: Option<&()>) -> Result<(), String> {
            assert!(lease.is_none());
            if self.acquired { return Err("no support for multiple leases".into()); }
            self.acquired = true; self.acquires += 1; Ok(())
        }
        fn release(&mut self, _: vk::DisplayKHR) { assert!(self.acquired); self.acquired = false; self.releases += 1; }
    }

    /// A DRM class directory with one card's connectors (status, id, EDID).
    struct Sysfs(std::path::PathBuf);
    impl Sysfs {
        fn new(test: &str, connectors: &[u32]) -> Self {
            let root = std::env::temp_dir().join(format!("makepad-hal-sysfs-{test}-{}", std::process::id()));
            for &c in connectors {
                let dir = root.join(format!("card1-DP-{c}"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("connector_id"), c.to_string()).unwrap();
                std::fs::write(dir.join("edid"), [c as u8; 128]).unwrap();
            }
            let sysfs = Self(root);
            for &c in connectors { sysfs.status(c, "connected"); }
            sysfs
        }
        fn status(&self, connector: u32, status: &str) {
            std::fs::write(self.0.join(format!("card1-DP-{connector}")).join("status"), format!("{status}\n")).unwrap();
        }
        fn scan(&self, probe: impl FnMut(&std::path::Path, u32)) -> Vec<u32> {
            super::super::kms::scan_sysfs_connectors(&self.0, &Default::default(), probe).unwrap().iter().map(|c| c.id).collect()
        }
    }
    impl Drop for Sysfs {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn a_reconnect_takes_a_fresh_lease_and_leaves_the_others_alone() {
        let monitors = [31, 32, 33, 34];
        let sysfs = Sysfs::new("reconnect", &monitors);
        let mut card = PerDescriptor::new(4, &monitors);
        let mut holds = Holds::new(false);
        assert_eq!(sysfs.scan(|_, _| {}), monitors);
        for &c in &monitors { holds.acquire(&mut card, display(c), c, &none()).unwrap(); }
        let heads: Vec<_> = holds.displays.iter().map(|h| h.crtc).collect();
        assert_eq!(heads, vec![Some(100), Some(101), Some(102), Some(103)], "each monitor keeps the head the console lit it on");
        let others = |card: &PerDescriptor| -> Vec<(vk::DisplayKHR, i32)> {
            let mut v: Vec<_> = card.acquired.iter().filter(|(d, _)| **d != display(32)).map(|(d, f)| (*d, *f)).collect();
            v.sort_by_key(|(d, _)| d.0); v
        };
        let before = others(&card);
        // unplugged: its display goes back, its lease is revoked
        card.unplug(32);
        sysfs.status(32, "disconnected");
        assert_eq!(sysfs.scan(|_, _| {}), [31, 33, 34]);
        holds.release(&mut card, display(32));
        assert_eq!(card.leases.len(), 3);
        assert!(holds.acquire(&mut card, display(32), 32, &none()).is_err(), "unplugged: nothing to acquire");
        // plugged back in; sysfs still reads "disconnected" until the master probes it
        card.plug(32);
        assert_eq!(sysfs.scan(|_, _| {}), [31, 33, 34], "the cached status is stale");
        assert_eq!(sysfs.scan(|_, id| if id == 32 { sysfs.status(32, "connected") }), monitors, "a probe refreshes it");
        // a fresh lease on its own head, granted through the new descriptor
        holds.acquire(&mut card, display(32), 32, &none()).expect("a reconnect acquires again");
        assert_eq!(card.crtc_of(32), Some(101));
        assert_eq!(others(&card), before, "the other three keep their leases and grants");
        // and again: every reconnect works, not just the first
        holds.release(&mut card, display(32));
        holds.acquire(&mut card, display(32), 32, &none()).expect("a second reconnect");
        // with all four heads held, a fifth monitor finds none
        card.plug(35);
        assert!(holds.acquire(&mut card, display(35), 35, &none()).unwrap_err().contains(NO_FREE_HEAD));
        holds.release_all(&mut card);
        assert!(card.acquired.is_empty() && card.leases.is_empty(), "everything released and revoked");
    }

    #[test]
    fn a_one_descriptor_card_is_held_once() {
        let mut wsi = OneDescriptor::default();
        let mut holds: Holds<()> = Holds::new(true);
        for c in [1, 2, 3] { holds.acquire(&mut wsi, display(c), c, &none()).unwrap(); }
        assert_eq!(wsi.acquires, 1, "one acquisition covers the card");
        holds.release(&mut wsi, display(2));
        holds.acquire(&mut wsi, display(2), 2, &none()).expect("a reconnect while others are held");
        assert_eq!((wsi.acquires, wsi.releases), (1, 0), "the card's acquisition never ends under its other displays");
        holds.release_all(&mut wsi);
        assert_eq!(wsi.releases, 1);
        holds.acquire(&mut wsi, display(1), 1, &none()).unwrap();
        holds.release(&mut wsi, display(1));
        assert_eq!((wsi.acquires, wsi.releases), (2, 2), "the last display ends it");
    }

    #[test]
    fn a_turned_off_monitor_keeps_its_head_until_a_monitor_finds_none_free() {
        let mut card = PerDescriptor::new(4, &[31, 32, 33]);
        let mut holds = Holds::new(false);
        for c in [31, 32, 33] { holds.acquire(&mut card, display(c), c, &none()).unwrap(); }
        holds.park(display(32));
        // a fourth monitor: a head is free, the turned-off one keeps its own
        card.plug(34);
        assert_eq!(holds.acquire(&mut card, display(34), 34, &none()), Ok(vec![]));
        assert_eq!((card.crtc_of(34), card.crtc_of(32)), (Some(103), Some(101)));
        // a fifth: no head free, so the turned-off display gives its head up
        card.plug(35);
        assert_eq!(holds.acquire(&mut card, display(35), 35, &none()), Ok(vec![display(32)]), "the turned-off display is given back");
        assert_eq!((card.crtc_of(35), card.crtc_of(32)), (Some(101), None));
        assert!(!holds.holds(display(32)) && !card.acquired.contains_key(&display(32)));
        // turned on again: every head drives a monitor now
        assert!(holds.acquire(&mut card, display(32), 32, &none()).unwrap_err().contains(NO_FREE_HEAD));
        // the fifth is unplugged: the head comes free and the monitor turned on takes it
        card.unplug(35);
        holds.release(&mut card, display(35));
        holds.acquire(&mut card, display(32), 32, &none()).expect("its head again");
        assert_eq!(card.crtc_of(32), Some(101));
        // turned off and on with heads to spare: the same display, lease and head
        holds.park(display(32));
        let fd = card.acquired[&display(32)];
        assert_eq!(holds.acquire(&mut card, display(32), 32, &none()), Ok(vec![]));
        assert_eq!(card.acquired[&display(32)], fd, "turning on again needs no new grant");
        assert_eq!(card.switched_off, vec![101], "only the head the turned-off display gave up was switched off");
    }

    #[test]
    fn a_console_on_a_connector_we_do_not_drive_gives_its_head_up_only_when_none_is_free() {
        // the console lights 41..44 on all four heads; the WM drives 41 and 42
        let mut card = PerDescriptor::new(4, &[41, 42, 43, 44]);
        let mut holds = Holds::new(false);
        for c in [41, 42] { holds.acquire(&mut card, display(c), c, &none()).unwrap(); }
        // 44 is unplugged: its head counts as free (cleared, nothing shows there); no console is touched
        card.unplug(44);
        card.plug(45);
        holds.acquire(&mut card, display(45), 45, &none()).unwrap();
        assert_eq!((card.crtc_of(45), card.switched_off.clone()), (Some(103), vec![103]));
        // 46 finds no free head: the console on 43 (not driven) is switched off for it
        card.plug(46);
        assert!(holds.acquire(&mut card, display(46), 46, &HashSet::from([43])).unwrap_err().contains(NO_FREE_HEAD),
            "a console on a connector the WM drives (pending) is never switched off");
        assert_eq!(card.switched_off, vec![103], "the console on 43 is untouched");
        holds.acquire(&mut card, display(46), 46, &none()).expect("the console's head");
        assert_eq!((card.crtc_of(46), card.switched_off.clone()), (Some(102), vec![103, 102]));
    }

    #[test]
    fn a_head_is_the_connectors_own_else_unlit_else_a_turned_off_ones_else_a_consoles() {
        let head = |crtc, lights, connected| Head { crtc, lights, connected };
        let kms = KmsState {
            heads: vec![head(1, 7, true), head(2, 0, false), head(3, 9, false), head(4, 8, true)],
            possible: HashMap::from([(8, 0b1111), (5, 0b1100), (6, 0b0001)]),
        };
        let pick = |c, ours: &[u32], driven: &[u32], parked: &[(u32, vk::DisplayKHR)]| {
            pick_head(c, &kms, &ours.iter().copied().collect(), &driven.iter().copied().collect(), parked)
        };
        assert_eq!(pick(8, &[], &[], &[]), Some(Pick::Free(4)), "its own head");
        assert_eq!(pick(8, &[4], &[], &[]), Some(Pick::Free(2)), "its own head is held: the first unlit one");
        assert_eq!(pick(5, &[], &[], &[]), Some(Pick::Free(3)), "a head lighting an unplugged connector is free");
        assert_eq!(pick(5, &[3], &[], &[(4, display(8))]), Some(Pick::Parked { crtc: 4, display: display(8) }), "a turned-off display's head");
        assert_eq!(pick(6, &[], &[], &[]), Some(Pick::Console { crtc: 1, connector: 7 }), "only a console's head is left");
        assert_eq!(pick(6, &[], &[7], &[]), None, "never a driven connector's");
        assert_eq!(pick(5, &[3, 4], &[], &[]), None);
    }

    #[test]
    fn plane_occupancy_follows_the_session_book_across_reconciles() {
        let gpu = vk::PhysicalDevice(1);
        let [d1, d2, d3, d4, d5, gone] = [1, 2, 3, 4, 5, 9].map(display);
        let row = |current: vk::DisplayKHR| PlaneRow { current, stack: 0, alpha: Some(vk::DisplayPlaneAlphaFlagsKHR::OPAQUE) };
        let order = |rows: &[PlaneRow], d, book: &PlaneBook, held: &[vk::DisplayKHR], connected: &[vk::DisplayKHR]| -> Vec<u32> {
            plane_order(rows, d, &book.of(gpu), &held.iter().copied().collect(), &connected.iter().copied().collect()).iter().map(|c| c.plane_index).collect()
        };
        let all = [d1, d2, d3, d4, d5];
        let mut book = PlaneBook::default();
        // start-up: the console shows each monitor on one plane
        let rows = [row(d1), row(d2), row(d3), row(d4)];
        let held = [d1, d2, d3, d4];
        let candidates: Vec<Vec<u32>> = held.iter().map(|&d| order(&rows, d, &book, &held, &all)).collect();
        assert_eq!(candidates, vec![vec![0], vec![1], vec![2], vec![3]], "a plane showing another held display is taken");
        assert_eq!(assign_planes(&candidates), vec![Some(0), Some(1), Some(2), Some(3)], "four monitors, four planes");
        for (plane, &d) in held.iter().enumerate() { book.show(gpu, plane as u32, d); }
        // d2 rebuilds (its presentation failed): the driver no longer shows it, the book keeps its plane
        let rows = [row(d1), row(vk::DisplayKHR::null()), row(d3), row(d4)];
        assert_eq!(order(&rows, d2, &book, &held, &all), vec![1]);
        // d3 is turned off and stays held: its plane is not another monitor's, whatever the driver shows
        let rows = [row(d1), row(d2), row(vk::DisplayKHR::null()), row(d4)];
        assert_eq!(order(&rows, d5, &book, &held, &all), Vec::<u32>::new(), "d5 finds no plane");
        assert_eq!(order(&rows, d3, &book, &held, &all), vec![2], "turned on again: its own plane");
        // d3 gives its head up for d5: out of the book, its plane is free (the console may show d3 there)
        let held = [d1, d2, d4, d5];
        book.keep(|d| held.contains(&d));
        let rows = [row(d1), row(d2), row(d3), row(d4)];
        assert_eq!(order(&rows, d5, &book, &held, &all), vec![2]);
        book.show(gpu, 2, d5);
        // a plane showing a display that is gone is free, and is taken before a console's
        let rows = [row(d1), row(d2), row(d5), row(d4), row(gone), row(d3)];
        assert_eq!(order(&rows, d3, &book, &[d1, d2, d4, d5], &all), vec![5, 4], "its own, then the gone one's");
        // a held display the driver shows on a second plane (a mode spanning two heads) keeps it
        let rows = [row(d1), row(d2), row(d5), row(d4), row(d4)];
        assert_eq!(order(&rows, d3, &book, &[d1, d2, d4, d5], &all), Vec::<u32>::new());
        // a pause gives everything back
        book.clear();
        assert!(book.of(gpu).is_empty());
    }

    #[test]
    fn four_monitors_on_four_planes_where_one_fast_mode_fails_to_present() {
        let gpu = vk::PhysicalDevice(1);
        let monitors = [51, 52, 53, 54];
        let mut card = PerDescriptor::new(4, &monitors);
        let mut holds = Holds::new(false);
        let mut book = PlaneBook::default();
        let mut demotions = Demotions::default();
        let connector = |id: u32| DrmConnector { card: "/dev/dri/card1".into(), name: format!("card1-DP-{id}"), id, fingerprint: u64::from(id), monitor: String::new() };
        let k4 = |hz: u32| (3840, 2160, hz * 1000);
        let modes = [k4(240), k4(144), k4(120), k4(60)];
        let row = |current| PlaneRow { current, stack: 0, alpha: Some(vk::DisplayPlaneAlphaFlagsKHR::OPAQUE) };
        let all: HashSet<_> = monitors.map(display).into();
        // start-up: four leases, four planes, each at the fastest mode
        for &c in &monitors { holds.acquire(&mut card, display(c), c, &none()).unwrap(); }
        let rows = monitors.map(|c| row(display(c)));
        let candidates: Vec<Vec<u32>> = monitors.iter().map(|&c| plane_order(&rows, display(c), &book.of(gpu), &all, &all).iter().map(|p| p.plane_index).collect()).collect();
        let planes = assign_planes(&candidates);
        assert_eq!(planes, vec![Some(0), Some(1), Some(2), Some(3)]);
        for (&c, plane) in monitors.iter().zip(&planes) { book.show(gpu, plane.unwrap(), display(c)); }
        let leases = card.acquired.clone();
        // 54 at 4K@240 never shows a frame: rebuilt below it, on its own plane, holding its lease
        assert!(demotions.failed(&connector(54), k4(240), false, "ERROR_UNKNOWN"));
        let rows = [row(display(51)), row(display(52)), row(display(53)), row(vk::DisplayKHR::null())];
        assert_eq!(holds.acquire(&mut card, display(54), 54, &none()), Ok(vec![]), "its display is still held");
        let own: Vec<u32> = plane_order(&rows, display(54), &book.of(gpu), &all, &all).iter().map(|p| p.plane_index).collect();
        assert_eq!(own, vec![3]);
        let ladder = mode_ladder(&modes, k4(240), &[k4(240)], demotions.below(&connector(54)).map(|b| b.0));
        assert_eq!(ladder[0], k4(144), "the next refresh at the same resolution");
        // 144 fails too: 120; the other three never noticed
        assert!(demotions.failed(&connector(54), k4(144), false, "ERROR_UNKNOWN"));
        assert_eq!(mode_ladder(&modes, k4(240), &[], demotions.below(&connector(54)).map(|b| b.0))[0], k4(120));
        assert_eq!(card.acquired, leases, "every monitor keeps its lease");
        assert_eq!(monitors.map(|c| card.crtc_of(c)), [Some(100), Some(101), Some(102), Some(103)]);
        assert!(demotions.below(&connector(51)).is_none());
    }
}
