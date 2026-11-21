//! USB 2.0 chapter 9 descriptors, as cached in Linux's sysfs `descriptors` file.
//! Strings and driver bindings are supplied by the OS backend, never fetched here.
use crate::{Class, EndpointInfo, InterfaceInfo, Transfer};
use std::fmt;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedDevice {
    pub vendor: u16,
    pub product: u16,
    pub release: u16,
    pub usb: u16,
    pub class: Class,
    pub configurations: Vec<ParsedConfiguration>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedConfiguration {
    pub value: u8,
    /// Every alternate setting; names and drivers are None until the backend fills them.
    pub interfaces: Vec<InterfaceInfo>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DescriptorError {
    /// The start of the offending descriptor in the input slice.
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "descriptor at offset {}: {}", self.offset, self.reason)
    }
}

impl std::error::Error for DescriptorError {}

fn error(offset: usize, reason: &'static str) -> DescriptorError {
    DescriptorError { offset, reason }
}

fn descriptor(bytes: &[u8], offset: usize, end: usize) -> Result<&[u8], DescriptorError> {
    let remaining = bytes.get(offset..end).ok_or_else(|| error(offset, "descriptor past the end"))?;
    if remaining.len() < 2 {
        return Err(error(offset, "truncated descriptor header"));
    }
    let length = remaining[0] as usize;
    if length < 2 {
        return Err(error(offset, "bLength must be at least 2"));
    }
    remaining.get(..length).ok_or_else(|| error(offset, "bLength extends past the end"))
}

/// Parse a device descriptor followed by zero or more whole configuration blocks.
/// Unknown descriptors, including interface associations, are skipped by bLength.
/// wMaxPacketSize retains only bits 0..10, dropping the high-bandwidth bits 11..12.
pub fn parse_device(bytes: &[u8]) -> Result<ParsedDevice, DescriptorError> {
    let device = descriptor(bytes, 0, bytes.len())?;
    if device[1] != 1 || device.len() != 18 {
        return Err(error(0, "expected an 18-byte device descriptor (type 1)"));
    }
    let mut parsed = ParsedDevice {
        vendor: u16::from_le_bytes([device[8], device[9]]),
        product: u16::from_le_bytes([device[10], device[11]]),
        release: u16::from_le_bytes([device[12], device[13]]),
        usb: u16::from_le_bytes([device[2], device[3]]),
        class: Class { class: device[4], subclass: device[5], protocol: device[6] },
        configurations: Vec::new(),
    };
    let mut offset = device.len();
    while offset < bytes.len() {
        let config = descriptor(bytes, offset, bytes.len())?;
        if config[1] != 2 || config.len() < 9 {
            return Err(error(offset, "expected a configuration descriptor (type 2, at least 9 bytes)"));
        }
        let total = u16::from_le_bytes([config[2], config[3]]) as usize;
        if total < 9 || total < config.len() {
            return Err(error(offset, "wTotalLength is shorter than the configuration descriptor"));
        }
        if total > bytes.len() - offset {
            return Err(error(offset, "wTotalLength extends past the end"));
        }
        let end = offset + total;
        let mut configuration = ParsedConfiguration { value: config[5], interfaces: Vec::new() };
        offset += config.len();
        while offset < end {
            let item = descriptor(bytes, offset, end)?;
            match item[1] {
                4 => {
                    if item.len() < 9 {
                        return Err(error(offset, "interface descriptor is shorter than 9 bytes"));
                    }
                    configuration.interfaces.push(InterfaceInfo {
                        number: item[2],
                        alternate: item[3],
                        class: Class { class: item[5], subclass: item[6], protocol: item[7] },
                        name: None,
                        endpoints: Vec::new(),
                        driver: None,
                    });
                }
                5 => {
                    if item.len() < 7 {
                        return Err(error(offset, "endpoint descriptor is shorter than 7 bytes"));
                    }
                    let interface = configuration.interfaces.last_mut()
                        .ok_or_else(|| error(offset, "endpoint descriptor has no preceding interface"))?;
                    interface.endpoints.push(EndpointInfo {
                        address: item[2],
                        transfer: match item[3] & 3 {
                            0 => Transfer::Control,
                            1 => Transfer::Isochronous,
                            2 => Transfer::Bulk,
                            _ => Transfer::Interrupt,
                        },
                        max_packet: u16::from_le_bytes([item[4], item[5]]) & 0x07ff,
                        interval: item[6],
                    });
                }
                _ => {}
            }
            offset += item.len();
        }
        parsed.configurations.push(configuration);
    }
    Ok(parsed)
}
