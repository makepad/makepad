//! serial.conf: what its lines say, and which devices may be opened, most protective rule first.

use makepad_serial::devices::{Conf, Ident, SerialDevice, Verdict};

fn usb(vendor: u16, product: u16, serial: Option<&str>, port: &str) -> Ident {
    Ident { vendor, product, serial: serial.map(str::to_owned), port: port.to_owned(),
        location: format!("pci0000:00/0000:00:14.0/usb3/{port}") }
}

fn off(verdict: Verdict) -> String {
    match verdict {
        Verdict::Off(why) => why,
        Verdict::On => panic!("expected off"),
    }
}

/// Two protected serial-less bridges of one kind by port, and other kinds that stay on.
const CONF: &str = "serve = on\n\
    # bridges on the fan controllers\n\
    off 1a86:7523 port=3-1.1  fan left\n\
    off 1a86:7523 port=3-1.4  fan right\n\
    off 0403:6001 serial=A1\n";

#[test]
fn serial_conf_turns_off_what_it_names_and_what_could_be_a_protected_device() {
    let conf = Conf::parse(CONF);
    assert!(conf.serves(), "serve = on lets devhub serve this machine's devices");
    let left = usb(0x1a86, 0x7523, None, "3-1.1");
    let right = usb(0x1a86, 0x7523, None, "3-1.4");
    let board = usb(0x303a, 0x1001, Some("48:27:E2:00:00:01"), "3-2");
    let both = [left.clone(), right.clone(), board.clone()];
    assert_eq!(off(conf.verdict(&left, &both)), "off in serial.conf (fan left)", "an off line by port turns its device off, with its note");
    assert_eq!(conf.verdict(&board, &both), Verdict::On, "a device no line names is on");

    let third = usb(0x1a86, 0x7523, None, "3-3");
    let all = [left.clone(), right.clone(), third.clone()];
    assert_eq!(off(conf.verdict(&third, &all)), "off: same kind as a protected device (fan left)",
        "a serial-less device of a protected kind is off until a line turns it on");
    let named = Conf::parse(&format!("{CONF}on 1a86:7523 port=3-3 the new bench board\n"));
    assert_eq!(named.verdict(&third, &all), Verdict::On, "an on line names a look-alike on while the protected ones are in place");

    let moved = [right.clone(), third.clone()];
    assert_eq!(off(named.verdict(&third, &moved)),
        "a protected device of this kind is not at its port; it may be this one (fan left)",
        "while a protected device is missing from its port, every look-alike is off, whatever on lines say");

    let renumbered = Ident { port: "4-1.1".into(), ..left.clone() };
    let by_path = Conf::parse("off 1a86:7523 port=pci0000:00/0000:00:14.0/usb3/3-1.1 fan left\n");
    assert!(matches!(by_path.verdict(&renumbered, &[renumbered.clone()]), Verdict::Off(_)),
        "a line by controller path still names the device after bus renumbering");

    let twin_a = usb(0x0403, 0x6001, Some("A1"), "1-2");
    let twin_b = usb(0x0403, 0x6001, Some("A1"), "1-3");
    let twins = [twin_a.clone(), twin_b.clone()];
    assert!(matches!(conf.verdict(&twin_b, &twins), Verdict::Off(_)), "an off line by serial covers every device with that serial");
    let guarded = Conf::parse("off 0403:6001 port=1-9 protected\n");
    let protected = usb(0x0403, 0x6001, None, "1-9");
    assert_eq!(guarded.verdict(&twin_a, &[protected.clone(), twin_a.clone()]), Verdict::On,
        "a device with a serial number of its own is off only by a line that names it");
    assert!(matches!(guarded.verdict(&twin_a, &[protected, twin_a.clone(), twin_b.clone()]), Verdict::Off(_)),
        "a serial number another attached device shares is no identity: the device is told apart by port only");

    assert_eq!(Conf::absent().verdict(&left, &both), Verdict::On, "with no file, this machine's own use opens its devices");
    assert!(!Conf::absent().serves(), "with no file, devhub serves nothing");
    assert!(!Conf::parse("").serves(), "a file without serve = on serves nothing");

    let bad = Conf::parse("serve = on\noff 1a86 port=3-1.1\n");
    assert!(!bad.serves(), "a file that does not read serves nothing");
    let why = off(bad.verdict(&board, &both));
    assert!(why.starts_with("serial.conf line 2: the device is <vid>:<pid> in hex"),
        "a file that does not read turns every device off and names its line: {why}");

    let device = SerialDevice { path: "/dev/cu.usbmodem1101".into(), ident: board, name: Some("USB JTAG/serial debug unit".into()),
        manufacturer: Some("Espressif".into()), verdict: Verdict::On };
    assert_eq!(device.label(), "USB JTAG/serial debug unit (Espressif) on cu.usbmodem1101", "a row names the product, its maker and the port");
}
