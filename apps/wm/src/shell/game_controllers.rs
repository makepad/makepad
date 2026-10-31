//! Controller inventory, peripheral batteries and Xbox receiver controls.
//! All filesystem access runs on the existing system worker, never the UI thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
    Headset,
    Other,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceBattery {
    pub name: String,
    pub kind: DeviceKind,
    pub detail: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Controller {
    pub name: String,
    pub battery: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Receiver {
    pub id: String,
    pub pairing: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub controllers: Vec<Controller>,
    pub receivers: Vec<Receiver>,
    pub batteries: Vec<DeviceBattery>,
}
#[cfg(target_os = "linux")]
fn read(path: impl AsRef<std::path::Path>) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}
#[cfg(target_os = "linux")]
pub fn sample() -> Inventory {
    use std::{fs, path::Path};
    let mut out = Inventory::default();
    // Read supplies once, then associate them with their owning input device.
    // A controller battery must not appear again as a generic peripheral.
    let supplies: Vec<_> = fs::read_dir("/sys/class/power_supply").into_iter().flatten()
        .flatten().map(|s| (s.path(), fs::canonicalize(s.path().join("device")).ok())).collect();
    let mut controller_supplies = std::collections::HashSet::new();
    let mut inputs = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/input") {
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with("event") { continue; }
            let device = entry.path().join("device");
            let Some(keys) = read(device.join("capabilities/key")) else { continue; };
            let actual = fs::canonicalize(&device).ok();
            let kind = if key_bit(&keys, 30) { DeviceKind::Keyboard } // KEY_A
                else if key_bit(&keys, 0x110) { DeviceKind::Mouse } // BTN_LEFT
                else { DeviceKind::Other };
            inputs.push((actual.clone(), kind));
            if !key_bit(&keys, 0x130) { continue; } // BTN_GAMEPAD
            let name = read(device.join("name")).unwrap_or_else(|| "Game controller".into());
            let mut battery = None;
            for (supply, owner) in &supplies {
                if !actual.as_ref().zip(owner.as_ref()).is_some_and(|(input, owner)| input.starts_with(owner)) { continue; }
                battery = Some(battery_detail(supply));
                controller_supplies.insert(supply.clone());
                break;
            }
            out.controllers.push(Controller {name, battery});
        }
    }
    for (supply, owner) in &supplies {
        if controller_supplies.contains(supply)
            || read(supply.join("type")).as_deref() != Some("Battery")
            || read(supply.join("scope")).as_deref() != Some("Device")
            || read(supply.join("present")).as_deref() == Some("0") { continue; }
        let name = read(supply.join("model_name")).filter(|s| !s.is_empty())
            .or_else(|| owner.as_ref().and_then(|p| read(p.join("name"))))
            .unwrap_or_else(|| "Wireless device".into());
        let lower = name.to_lowercase();
        // Gaming mice expose an extra keyboard interface for programmable buttons.
        let kind = if lower.contains("mouse") || lower.contains("trackball") { DeviceKind::Mouse }
            else { inputs.iter().filter(|(input, _)| input.as_ref().zip(owner.as_ref())
            .is_some_and(|(input, owner)| input.starts_with(owner)))
            .map(|(_, kind)| *kind).find(|k| *k == DeviceKind::Keyboard)
            .or_else(|| inputs.iter().find(|(input, kind)| *kind == DeviceKind::Mouse && input.as_ref().zip(owner.as_ref())
                .is_some_and(|(input, owner)| input.starts_with(owner))).map(|(_, kind)| *kind))
            .unwrap_or_else(|| if lower.contains("headset") || lower.contains("headphone") || lower.contains("blackshark") {
                DeviceKind::Headset
            } else { DeviceKind::Other }) };
        out.batteries.push(DeviceBattery { name, kind, detail: battery_detail(supply) });
    }
    // The BlackShark V3 Pro receiver (1532:0577) exposes no power_supply;
    // its battery comes from the headset's own reports (`blackshark`) and
    // the panel lists it from that snapshot, not from here.
    let root = Path::new("/sys/bus/usb/drivers/xone-dongle");
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if let Some(pairing) = read(entry.path().join("pairing")) {
                out.receivers.push(Receiver {id: entry.file_name().to_string_lossy().into_owned(), pairing: pairing == "1"});
            }
        }
    }
    out.controllers.sort_by(|a,b| a.name.cmp(&b.name));
    out.receivers.sort_by(|a,b| a.id.cmp(&b.id));
    out.batteries.sort_by(|a,b| a.name.cmp(&b.name));
    out
}
#[cfg(target_os = "linux")]
fn key_bit(keys: &str, bit: usize) -> bool {
    // Kernel bitmap words are printed high to low.
    let bits = usize::BITS as usize;
    keys.split_whitespace().rev().nth(bit / bits)
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .is_some_and(|word| word & (1 << (bit % bits)) != 0)
}
#[cfg(target_os = "linux")]
fn battery_detail(path: &std::path::Path) -> String {
    let level = read(path.join("capacity")).and_then(|s| s.parse::<u8>().ok())
        .filter(|n| *n <= 100).map(|n| format!("{n}%"))
        .or_else(|| read(path.join("capacity_level")).filter(|s| s != "Unknown"));
    let mut text = level.unwrap_or_else(|| "Battery unknown".into());
    if read(path.join("online")).as_deref() == Some("0") {
        text.push_str(" · Last reported (offline)");
    } else if let Some(status) = read(path.join("status")) {
        if matches!(status.as_str(), "Charging" | "Not charging" | "Full") {
            text.push_str(" · "); text.push_str(&status);
        }
    }
    text
}
#[cfg(target_os = "linux")]
pub fn pair(id: &str) -> Result<(), String> {
    // Only an interface the driver currently exposes may be selected.
    if !sample().receivers.iter().any(|r| r.id == id) {
        return Err("Xbox receiver disconnected".into());
    }
    let path = std::path::Path::new("/sys/bus/usb/drivers/xone-dongle").join(id).join("pairing");
    std::fs::write(path, b"1").map_err(|e| format!("Cannot start Xbox pairing: {e}"))
}
