//! NAT-PMP external-address and UDP mapping messages (RFC 6886, section 3).

use std::net::Ipv4Addr;

pub const EXTERNAL_ADDRESS: [u8; 2] = [0, 0];

/// Deletion uses a zero lifetime and a zero suggested external port.
pub fn map(internal: u16, external: u16, lifetime: u32) -> [u8; 12] {
    let mut bytes = [0; 12];
    bytes[1] = 1;
    bytes[4..6].copy_from_slice(&internal.to_be_bytes());
    if lifetime != 0 {
        bytes[6..8].copy_from_slice(&external.to_be_bytes());
    }
    bytes[8..].copy_from_slice(&lifetime.to_be_bytes());
    bytes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultCode {
    Success,
    UnsupportedVersion,
    NotAuthorized,
    NetworkFailure,
    OutOfResources,
    UnsupportedOpcode,
    Unknown(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    Address(Ipv4Addr),
    Mapping { internal: u16, external: u16, lifetime: u32 },
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub result: ResultCode,
    pub epoch: u32,
    pub payload: Payload,
}

pub fn reply(bytes: &[u8]) -> Option<Reply> {
    if bytes.first() != Some(&0) {
        return None;
    }
    let result = match u16::from_be_bytes(bytes.get(2..4)?.try_into().ok()?) {
        0 => ResultCode::Success,
        1 => ResultCode::UnsupportedVersion,
        2 => ResultCode::NotAuthorized,
        3 => ResultCode::NetworkFailure,
        4 => ResultCode::OutOfResources,
        5 => ResultCode::UnsupportedOpcode,
        code => ResultCode::Unknown(code),
    };
    let epoch = u32::from_be_bytes(bytes.get(4..8)?.try_into().ok()?);
    let payload = match (bytes[1], &bytes[8..]) {
        (128, &[a, b, c, d]) => Payload::Address(Ipv4Addr::new(a, b, c, d)),
        (129, &[i, j, e, f, a, b, c, d]) => Payload::Mapping {
            internal: u16::from_be_bytes([i, j]),
            external: u16::from_be_bytes([e, f]),
            lifetime: u32::from_be_bytes([a, b, c, d]),
        },
        (128 | 129, []) if result != ResultCode::Success => Payload::Error,
        _ => return None,
    };
    Some(Reply { result, epoch, payload })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_and_address_wire_format() {
        assert_eq!(EXTERNAL_ADDRESS, [0, 0], "external address query");
        assert_eq!(
            map(0x2345, 0x3456, 300),
            [0, 1, 0, 0, 0x23, 0x45, 0x34, 0x56, 0, 0, 1, 44],
            "mapping request"
        );
        assert_eq!(map(0x2345, 0x3456, 0), [0, 1, 0, 0, 0x23, 0x45, 0, 0, 0, 0, 0, 0], "delete request");
        let addr = [0, 128, 0, 0, 0, 0, 0, 42, 198, 51, 100, 8];
        let ports = [0, 129, 0, 0, 0, 0, 0, 42, 0x23, 0x45, 0x34, 0x57, 0, 0, 0, 60];
        for (bytes, expected) in [
            (&addr[..], Payload::Address(Ipv4Addr::new(198, 51, 100, 8))),
            (&ports[..], Payload::Mapping { internal: 0x2345, external: 0x3457, lifetime: 60 }),
        ] {
            assert_eq!(
                reply(bytes),
                Some(Reply { result: ResultCode::Success, epoch: 42, payload: expected })
            );
            for end in 0..bytes.len() {
                assert!(reply(&bytes[..end]).is_none(), "accepted prefix {end}");
            }
        }
        let mut denied = ports;
        denied[3] = 2;
        assert_eq!(reply(&denied).unwrap().result, ResultCode::NotAuthorized);
        denied[1] = 1;
        assert!(reply(&denied).is_none(), "request mistaken for reply");
    }
}
