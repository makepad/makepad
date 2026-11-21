//! Linked viewers share one stream slot. The selected point survives a link leaving.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrDeviceKind {
    Headset,
    Handheld,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrDeviceLink {
    Offline,
    Standby,
    Streaming,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XrDevice {
    pub point_hex: String,
    pub name: String,
    pub kind: Option<XrDeviceKind>,
    pub on: bool,
    pub link: XrDeviceLink,
    /// Where the device is linked, e.g. "with an app on another PC (test run)". None unless it is with another host.
    pub elsewhere: Option<String>,
}

pub(super) type Point = [u8; 65];

pub(super) struct Link {
    pub id: u64,
    pub point: Point,
    pub name: String,
    pub kind: XrDeviceKind,
    pub standby: bool,
}

#[derive(Default)]
pub(super) struct Links {
    joined: HashMap<u64, Link>,
    stream: Option<u64>,
    // The selftest clears its reservation when its Core stops.
    selftest: Arc<AtomicBool>,
    pub on: Option<Point>,
    // The panel takes a snapshot with one Arc clone and copies its rows outside the lock.
    pub devices: Arc<Vec<(Point, XrDevice)>>,
}

impl Links {
    pub fn join(&mut self, link: Link) {
        let point = link.point;
        self.joined.insert(link.id, link);
        self.update_device(&point);
    }

    pub fn get(&self, link: u64) -> Option<&Link> {
        self.joined.get(&link)
    }

    pub fn linked(&self, point: &Point) -> bool {
        self.joined.values().any(|link| &link.point == point)
    }

    pub fn wants_stream(&self, link: u64, on: Option<&Point>) -> bool {
        self.get(link).is_some_and(|l| Some(&l.point) == on)
            && (self.stream.is_none() || self.stream == Some(link))
            && !self.testing()
    }

    pub fn take(&mut self, link: u64) -> bool {
        if !self.wants_stream(link, self.on.as_ref()) {
            return false;
        }
        self.stream = Some(link);
        let point = self.joined[&link].point;
        self.update_device(&point);
        true
    }

    pub fn release(&mut self, link: u64) {
        if self.stream == Some(link) {
            self.stream = None;
            if let Some(point) = self.get(link).map(|l| l.point) {
                self.update_device(&point);
            }
        }
    }

    pub fn leave(&mut self, link: u64) {
        if let Some(l) = self.joined.remove(&link) {
            self.update_device(&l.point);
        }
        self.release(link);
    }

    pub fn forget(&mut self, point: &Point) {
        // The old pump still owns the slot until it tears down. Even an immediate new
        // pairing of the same point cannot revive the forgotten link.
        self.joined.retain(|_, l| &l.point != point);
    }

    /// The device the view is on: the streaming one, else the switched-on one (offline, or
    /// linked before its pump takes the slot); empty while every switch is off.
    pub fn showing(&self) -> String {
        self.stream.and_then(|id| self.get(id)).map(|l| l.name.clone()).unwrap_or_else(|| self.on_name())
    }

    fn on_name(&self) -> String {
        self.on.and_then(|on| self.devices.iter().find(|(p, _)| *p == on)).map(|(_, d)| d.name.clone()).unwrap_or_default()
    }

    /// The Core has stopped, but the old link keeps the slot until it sends Standby.
    /// Name the linked viewer that will be eligible when that slot is released.
    pub fn handoff_name(&self, leaving: u64) -> String {
        self.joined.values().find(|l| l.id != leaving && Some(l.point) == self.on)
            .map(|l| l.name.clone()).unwrap_or_else(|| self.on_name())
    }

    pub fn occupied(&self) -> bool {
        self.stream.is_some()
    }

    pub fn testing(&self) -> bool {
        self.selftest.load(Ordering::Acquire)
    }

    pub fn take_selftest(&self) -> Option<Arc<AtomicBool>> {
        (self.stream.is_none() && !self.selftest.swap(true, Ordering::AcqRel)).then(|| self.selftest.clone())
    }

    fn status(&self, point: &Point) -> (Option<XrDeviceKind>, XrDeviceLink) {
        if let Some(stream) = self.stream.and_then(|id| self.get(id)).filter(|l| &l.point == point) {
            return (Some(stream.kind), XrDeviceLink::Streaming);
        }
        match self.joined.values().find(|l| &l.point == point) {
            Some(l) => (Some(l.kind), XrDeviceLink::Standby),
            None => (None, XrDeviceLink::Offline),
        }
    }

    fn update_device(&mut self, point: &Point) {
        let (kind, link) = self.status(point);
        if let Some((_, d)) = Arc::make_mut(&mut self.devices).iter_mut().find(|(p, _)| p == point) {
            (d.kind, d.link) = (kind, link);
        }
    }

    pub fn paired(&mut self, on: Option<Point>, viewers: Vec<(Point, String, String)>) {
        self.on = on;
        let mut online = HashMap::new();
        for link in self.joined.values() {
            online.insert(link.point, (Some(link.kind), XrDeviceLink::Standby));
        }
        if let Some(link) = self.stream.and_then(|id| self.get(id)) {
            online.insert(link.point, (Some(link.kind), XrDeviceLink::Streaming));
        }
        self.devices = Arc::new(viewers.into_iter().map(|(point, point_hex, name)| {
            let (kind, link) = online.get(&point).copied().unwrap_or((None, XrDeviceLink::Offline));
            (point, XrDevice { point_hex, name, kind, link, on: Some(point) == on, elsewhere: None })
        }).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_selected_viewer_can_take_the_released_slot() {
        let mut links = Links::default();
        let (a, b) = ([1; 65], [2; 65]);
        let rows = || vec![(a, "a".into(), "headset".into()), (b, "b".into(), "tablet".into())];
        links.paired(None, rows());
        links.join(Link { id: 1, point: a, name: "headset".into(), kind: XrDeviceKind::Headset, standby: true });
        links.join(Link { id: 2, point: b, name: "tablet".into(), kind: XrDeviceKind::Handheld, standby: true });
        assert!(!links.take(1) && !links.take(2), "all off leaves both linked viewers on standby");
        links.paired(Some(a), rows());
        assert!(links.take(1), "the selected headset takes the free slot");
        assert_eq!(links.showing(), "headset", "standby names the streaming headset");
        assert!(!links.take(2), "a second viewer waits");
        assert_eq!(links.devices[0].1.link, XrDeviceLink::Streaming, "the first paired row owns the stream");
        assert_eq!(links.devices[1].1.link, XrDeviceLink::Standby, "the second row stays linked");
        links.paired(Some(b), rows());
        assert!(!links.wants_stream(1, links.on.as_ref()), "the old pump must stop after a switch");
        assert!(!links.take(2), "the new pump must wait for teardown");
        links.release(1);
        assert!(links.take(2), "the tablet takes the released slot");
        links.leave(2);
        assert!(!links.occupied(), "leaving frees the slot");
        assert_eq!(links.showing(), "tablet", "standby still names the switched-on device while it is offline");
        let tablet = &links.devices[1].1;
        assert!(tablet.on && tablet.kind.is_none() && tablet.link == XrDeviceLink::Offline, "the offline tablet keeps its switch");
        assert!(!links.take(1), "an offline selection cannot be taken over");
        links.paired(None, rows());
        assert!(links.showing().is_empty(), "every switch off names no device");
    }
}
