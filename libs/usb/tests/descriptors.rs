use makepad_usb::descriptor::{parse_device, ParsedConfiguration, ParsedDevice};
use makepad_usb::{Class, EndpointInfo, InterfaceInfo, Transfer};

// USB 2.0 chapter 9 layouts, written here: device, configuration, interface, endpoint.
const HUB: &[u8] = &[
    18, 1, 0x10, 0x01, 9, 0, 0, 8, 0x34, 0x12, 0x01, 0x00, 0x02, 0x01, 1, 2, 0, 1,
    9, 2, 25, 0, 1, 1, 0, 0xe0, 0,
    9, 4, 0, 0, 1, 9, 0, 0, 0,
    7, 5, 0x81, 3, 1, 0, 255,
];

const TMC: &[u8] = &[
    18, 1, 0x00, 0x02, 0, 0, 0, 64, 0xcd, 0xab, 0x34, 0x12, 0x10, 0x02, 1, 2, 3, 1,
    9, 2, 39, 0, 1, 1, 0, 0x80, 50,
    9, 4, 0, 0, 3, 0xfe, 3, 1, 4,
    7, 5, 0x01, 2, 0x00, 0x02, 0,
    7, 5, 0x82, 2, 0x00, 0x02, 0,
    7, 5, 0x83, 3, 0x00, 0x02, 4,
];

const COMPOSITE: &[u8] = &[
    18, 1, 0x00, 0x02, 0xef, 2, 1, 64, 0x34, 0x12, 0x78, 0x56, 0x01, 0x02, 1, 2, 3, 2,
    9, 2, 74, 0, 2, 1, 0, 0x80, 50,
    8, 11, 0, 2, 0xff, 1, 2, 0, // Interface association.
    9, 4, 0, 0, 1, 0xff, 1, 2, 0,
    5, 0x24, 0, 0x10, 0x01, // Class-specific interface descriptor.
    7, 5, 0x81, 3, 16, 0, 8,
    9, 4, 1, 0, 0, 1, 2, 3, 0,
    4, 0xff, 0xaa, 0x55, // Vendor-specific descriptor.
    9, 4, 1, 1, 2, 1, 2, 3, 0,
    7, 5, 0x82, 5, 0x00, 0x1a, 1, // Isochronous with high-bandwidth bits.
    7, 5, 0x03, 2, 64, 0, 0,
    9, 2, 25, 0, 1, 3, 0, 0x80, 25,
    9, 4, 2, 0, 1, 0xff, 4, 5, 0,
    7, 5, 0x02, 0, 64, 0, 0,
];

#[test]
fn full_speed_hub() {
    let parsed = parse_device(HUB).expect("a whole full-speed hub descriptor parses");
    assert_eq!(parsed, ParsedDevice {
        vendor: 0x1234,
        product: 1,
        release: 0x0102,
        usb: 0x0110,
        class: Class { class: 9, subclass: 0, protocol: 0 },
        configurations: vec![ParsedConfiguration {
            value: 1,
            interfaces: vec![InterfaceInfo {
                number: 0,
                alternate: 0,
                class: Class { class: 9, subclass: 0, protocol: 0 },
                name: None,
                driver: None,
                endpoints: vec![EndpointInfo { address: 0x81, transfer: Transfer::Interrupt, max_packet: 1, interval: 255 }],
            }],
        }],
    }, "hub fields, little-endian BCD values and the interrupt endpoint match chapter 9");
    assert!(parsed.configurations[0].interfaces[0].endpoints[0].is_in(), "hub status endpoint is IN");
    let device_only = parse_device(&HUB[..18]).expect("zero configuration blocks are allowed");
    assert!(device_only.configurations.is_empty(), "a device-only file has no invented configurations");
    assert_eq!(device_only.class, parsed.class, "device-only parsing retains the class triple");
}

#[test]
fn test_and_measurement_interface() {
    let parsed = parse_device(TMC).expect("the test-and-measurement fixture parses");
    assert_eq!(parsed, ParsedDevice {
        vendor: 0xabcd,
        product: 0x1234,
        release: 0x0210,
        usb: 0x0200,
        class: Class { class: 0, subclass: 0, protocol: 0 },
        configurations: vec![ParsedConfiguration {
            value: 1,
            interfaces: vec![InterfaceInfo {
                number: 0,
                alternate: 0,
                class: Class { class: 0xfe, subclass: 3, protocol: 1 },
                name: None,
                driver: None,
                endpoints: vec![
                    EndpointInfo { address: 0x01, transfer: Transfer::Bulk, max_packet: 512, interval: 0 },
                    EndpointInfo { address: 0x82, transfer: Transfer::Bulk, max_packet: 512, interval: 0 },
                    EndpointInfo { address: 0x83, transfer: Transfer::Interrupt, max_packet: 512, interval: 4 },
                ],
            }],
        }],
    }, "USBTMC class triple, three endpoints and 512-byte packet sizes are exact");
    let endpoints = &parsed.configurations[0].interfaces[0].endpoints;
    assert!(!endpoints[0].is_in(), "bulk command endpoint 0x01 is OUT");
    assert!(endpoints[1].is_in() && endpoints[2].is_in(), "bulk and interrupt reply endpoints are IN");
}

#[test]
fn composite_configurations_and_alternates() {
    let parsed = parse_device(COMPOSITE).expect("the composite fixture parses");
    assert_eq!(parsed, ParsedDevice {
        vendor: 0x1234,
        product: 0x5678,
        release: 0x0201,
        usb: 0x0200,
        class: Class { class: 0xef, subclass: 2, protocol: 1 },
        configurations: vec![
            ParsedConfiguration {
                value: 1,
                interfaces: vec![
                    InterfaceInfo {
                        number: 0,
                        alternate: 0,
                        class: Class { class: 0xff, subclass: 1, protocol: 2 },
                        name: None,
                        driver: None,
                        endpoints: vec![EndpointInfo { address: 0x81, transfer: Transfer::Interrupt, max_packet: 16, interval: 8 }],
                    },
                    InterfaceInfo {
                        number: 1,
                        alternate: 0,
                        class: Class { class: 1, subclass: 2, protocol: 3 },
                        name: None,
                        driver: None,
                        endpoints: vec![],
                    },
                    InterfaceInfo {
                        number: 1,
                        alternate: 1,
                        class: Class { class: 1, subclass: 2, protocol: 3 },
                        name: None,
                        driver: None,
                        endpoints: vec![
                            EndpointInfo { address: 0x82, transfer: Transfer::Isochronous, max_packet: 512, interval: 1 },
                            EndpointInfo { address: 0x03, transfer: Transfer::Bulk, max_packet: 64, interval: 0 },
                        ],
                    },
                ],
            },
            ParsedConfiguration {
                value: 3,
                interfaces: vec![InterfaceInfo {
                    number: 2,
                    alternate: 0,
                    class: Class { class: 0xff, subclass: 4, protocol: 5 },
                    name: None,
                    driver: None,
                    endpoints: vec![EndpointInfo { address: 0x02, transfer: Transfer::Control, max_packet: 64, interval: 0 }],
                }],
            },
        ],
    }, "configuration boundaries, alternate settings, skipped descriptors and masked packet bits are exact");
    assert_eq!(parsed.configurations[0].interfaces.len(), 3, "the IAD does not create an extra interface");
    assert_eq!(parsed.configurations[1].interfaces.len(), 1, "interfaces do not leak across configurations");
}

#[test]
fn malformed_lengths_report_the_descriptor_offset_without_panicking() {
    let mut zero_length = HUB.to_vec();
    zero_length[27] = 0;
    let mut one_length = HUB.to_vec();
    one_length[27] = 1;
    let mut past_end = HUB.to_vec();
    past_end[27] = 17;
    let mut total_past_end = HUB.to_vec();
    total_past_end[20] = 26;
    let mut total_short = HUB.to_vec();
    total_short[20] = 8;
    let mut endpoint_past_config = HUB.to_vec();
    endpoint_past_config[20] = 24;
    let mut trailing = HUB.to_vec();
    trailing.push(0xaa);
    let cases = [
        ("truncated device", &HUB[..17], 0, "bLength"),
        ("zero bLength", zero_length.as_slice(), 27, "bLength"),
        ("one-byte bLength", one_length.as_slice(), 27, "bLength"),
        ("bLength past end", past_end.as_slice(), 27, "bLength"),
        ("wTotalLength past end", total_past_end.as_slice(), 18, "wTotalLength"),
        ("wTotalLength below nine", total_short.as_slice(), 18, "wTotalLength"),
        ("endpoint past configuration", endpoint_past_config.as_slice(), 36, "bLength"),
        ("trailing bytes", trailing.as_slice(), 43, "truncated"),
    ];
    for (property, bytes, offset, reason) in cases {
        let result = std::panic::catch_unwind(|| parse_device(bytes));
        assert!(result.is_ok(), "{property}: malformed data must not panic");
        let err = result.unwrap().expect_err(property);
        assert_eq!(err.offset, offset, "{property}: error points to the offending descriptor");
        assert!(err.reason.contains(reason), "{property}: error explains the malformed length");
        assert!(err.to_string().contains(&format!("offset {offset}")), "{property}: displayed error names its offset");
    }
    for fixture in [HUB, TMC, COMPOSITE] {
        for length in 0..=fixture.len() {
            assert!(std::panic::catch_unwind(|| parse_device(&fixture[..length])).is_ok(),
                "every truncated fixture prefix is bounded and panic-free (length {length})");
        }
    }
}
