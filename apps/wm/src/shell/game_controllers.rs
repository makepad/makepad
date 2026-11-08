//! Controller inventory, the keyboards and mice the WM reads, peripheral batteries and Xbox
//! receiver controls.
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
/// A keyboard or mouse (a touchpad counts) the WM's own input reader has open, wired or
/// wireless.
#[derive(Clone, Debug, PartialEq)]
pub struct InputDevice {
    pub name: String,
    pub kind: DeviceKind,
    /// The kernel's reading of its battery, when it gives one.
    pub battery: Option<String>,
    /// It has sent a key, a button or a motion since it appeared; a wireless one whose receiver
    /// is in but that is off or out of range has not.
    pub reported: bool,
}
impl InputDevice {
    /// Its row's line: no signal yet, its battery, or connected.
    pub fn detail(&self) -> String {
        match (self.reported, &self.battery) {
            (false, Some(battery)) => format!("No signal yet · Battery: {battery}"),
            (false, None) => "No signal yet".into(),
            (true, Some(battery)) => format!("Battery: {battery}"),
            (true, None) => "Connected".into(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Receiver {
    pub id: String,
    pub pairing: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub controllers: Vec<Controller>,
    pub inputs: Vec<InputDevice>,
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
    // A controller's or a listed keyboard's or mouse's battery must not appear again as a
    // generic peripheral.
    let supplies: Vec<_> = fs::read_dir("/sys/class/power_supply").into_iter().flatten()
        .flatten().map(|s| (s.path(), fs::canonicalize(s.path().join("device")).ok())).collect();
    let mut listed_supplies = std::collections::HashSet::new();
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
                listed_supplies.insert(supply.clone());
                break;
            }
            out.controllers.push(Controller {name, battery});
        }
    }
    // The keyboards and mice the WM reads, by the nodes its input reader has open; a device's
    // battery is the supply its input device sits under, listed with it and not again below.
    let open = (0..).map_while(makepad_hal::user_input::open_device);
    for device in open {
        let node = Path::new("/sys/class/input").join(format!("event{}", device.event)).join("device");
        // the kernel's virtual bus: Desk Link's and other made-up devices, no hardware
        if read(node.join("id/bustype")).as_deref() == Some("0006") { continue; }
        let Some(keys) = read(node.join("capabilities/key")) else { continue; };
        let actual = fs::canonicalize(&node).ok();
        let supply = supplies.iter().find(|(_, owner)| actual.as_ref().zip(owner.as_ref())
            .is_some_and(|(input, owner)| input.starts_with(owner))).map(|(supply, _)| supply.clone());
        let name = read(node.join("name")).filter(|s| !s.is_empty());
        // A wireless receiver's node of a mouse often lists a keyboard's keys too (its
        // programmable buttons), and a keyboard may have a touchpad: such a node is a mouse
        // unless its name or its battery's says keyboard.
        let says_keyboard = name.iter().chain(supply.as_ref().and_then(|s| read(s.join("model_name"))).iter())
            .any(|n| n.to_lowercase().contains("keyboard"));
        let kind = match (key_bit(&keys, 30), key_bit(&keys, 0x110)) { // KEY_A, BTN_LEFT
            (true, true) if says_keyboard => DeviceKind::Keyboard,
            (true, false) => DeviceKind::Keyboard,
            (_, true) => DeviceKind::Mouse,
            (false, false) => continue,
        };
        let name = name.unwrap_or_else(|| if kind == DeviceKind::Keyboard { "Keyboard".into() } else { "Mouse".into() });
        let battery = supply.as_ref().map(|supply| battery_detail(supply));
        if let Some(supply) = supply { listed_supplies.insert(supply); }
        // one device's several nodes of a kind (a keyboard's media keys) are one row
        match out.inputs.iter_mut().find(|d| d.name == name && d.kind == kind) {
            Some(same) => {
                same.reported |= device.reported;
                if same.battery.is_none() { same.battery = battery; }
            }
            None => out.inputs.push(InputDevice { name, kind, battery, reported: device.reported }),
        }
    }
    for (supply, owner) in &supplies {
        if listed_supplies.contains(supply)
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
    out.inputs.sort_by(|a,b| (a.kind != DeviceKind::Keyboard, &a.name).cmp(&(b.kind != DeviceKind::Keyboard, &b.name)));
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyboard_or_mouse_says_no_signal_until_it_reports() {
        let device = |reported, battery: Option<&str>| InputDevice { name: "Keyboard".into(), kind: DeviceKind::Keyboard, battery: battery.map(Into::into), reported };
        assert_eq!(device(false, None).detail(), "No signal yet");
        assert_eq!(device(false, Some("80%")).detail(), "No signal yet · Battery: 80%");
        assert_eq!(device(true, Some("80%")).detail(), "Battery: 80%");
        assert_eq!(device(true, None).detail(), "Connected");
    }
}
