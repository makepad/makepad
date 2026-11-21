#![cfg(target_os = "linux")]
#![cfg(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "arm",
    target_arch = "aarch64",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64"
))]

use makepad_usb::{devices, open, Access, Class, Device};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

fn hex(path: &Path, attribute: &str) -> u16 {
    let text = fs::read_to_string(path.join(attribute)).expect("listed device's sysfs attribute remains readable");
    u16::from_str_radix(text.trim(), 16).expect("kernel USB attributes contain hexadecimal integers")
}

#[test]
fn open_configuration_agrees_with_cached_descriptors_when_accessible() {
    let listed = devices().expect("Linux USB enumeration succeeds");
    for info in listed.iter().filter(|info| info.access == Access::Ok) {
        let Ok(mut device) = open(info.id) else { continue };
        let interfaces = device.configuration().expect("an accessible device's configuration is readable");
        assert_eq!(interfaces, info.interfaces, "open descriptors agree with the active sysfs configuration");
        // Only read descriptors from one accessible node; never claim or detach anything.
        return;
    }
}

fn require_send<T: Send>() {}

#[test]
fn enumeration_agrees_with_sysfs_and_reports_hub_driver() {
    require_send::<Device>();
    let listed = devices().expect("Linux USB enumeration succeeds");
    let mut ids = HashSet::new();
    let mut saw_hub = false;
    let mut saw_hub_driver = false;
    for device in &listed {
        assert!(ids.insert(device.id), "each listed DeviceId is distinct: {}", device.id);
        let name = match device.port.strip_suffix("-0") {
            Some(bus) => format!("usb{bus}"),
            None => device.port.clone(),
        };
        let path = Path::new("/sys/bus/usb/devices").join(name);
        assert_eq!(device.vendor, hex(&path, "idVendor"), "{}: descriptor vendor matches sysfs", device.id);
        assert_eq!(device.product, hex(&path, "idProduct"), "{}: descriptor product matches sysfs", device.id);
        assert_eq!(device.class, Class {
            class: hex(&path, "bDeviceClass") as u8,
            subclass: hex(&path, "bDeviceSubClass") as u8,
            protocol: hex(&path, "bDeviceProtocol") as u8,
        }, "{}: descriptor class triple matches sysfs", device.id);
        let bus = fs::read_to_string(path.join("busnum")).expect("listed device has a sysfs bus number");
        let address = fs::read_to_string(path.join("devnum")).expect("listed device has a sysfs address");
        assert_eq!(device.id.to_string(), format!("usb {}-{}", bus.trim(), address.trim()),
            "DeviceId display names the current bus and address");
        assert_eq!(fs::canonicalize(&path).expect("a listed device's sysfs link resolves"),
            Path::new("/sys/devices").join(device.location.trim_start_matches('/')),
            "{}: location is the device's path below /sys/devices", device.id);
        for port in &device.serial_ports {
            let name = port.strip_prefix("/dev/").expect("serial port path must start with /dev/");
            assert!(Path::new("/sys/class/tty").join(name).is_dir(),
                "{} serial port {port} must have a tty class entry in sysfs", device.id);
        }
        for pair in device.serial_ports.windows(2) {
            assert!(pair[0] < pair[1], "{} serial ports must be sorted and unique, got {pair:?}", device.id);
        }
        if device.class.class == 9 {
            saw_hub = true;
            if let Access::Busy(reason) = &device.access {
                if reason.contains("bound to the hub driver") {
                    saw_hub_driver = true;
                }
            }
        }
    }
    assert!(saw_hub, "Linux enumeration includes at least one class-9 hub");
    assert!(saw_hub_driver, "at least one hub reports Busy naming the bound hub driver");
    // Enumeration and this comparison only read sysfs: no device is opened or detached.
}
