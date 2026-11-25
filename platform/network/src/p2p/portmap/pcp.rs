//! PCP MAP for UDP (RFC 6887, sections 7 and 11). The caller owns the nonce and socket.

use std::net::{Ipv6Addr, SocketAddrV4};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultCode {
    Success,
    UnsuppVersion,
    NotAuthorized,
    MalformedRequest,
    UnsuppOpcode,
    UnsuppOption,
    MalformedOption,
    NetworkFailure,
    NoResources,
    UnsuppProtocol,
    UserExQuota,
    CannotProvideExternal,
    AddressMismatch,
    ExcessiveRemotePeers,
    Unknown(u8),
}

impl From<u8> for ResultCode {
    fn from(code: u8) -> Self {
        use ResultCode::*;
        match code {
            0 => Success,
            1 => UnsuppVersion,
            2 => NotAuthorized,
            3 => MalformedRequest,
            4 => UnsuppOpcode,
            5 => UnsuppOption,
            6 => MalformedOption,
            7 => NetworkFailure,
            8 => NoResources,
            9 => UnsuppProtocol,
            10 => UserExQuota,
            11 => CannotProvideExternal,
            12 => AddressMismatch,
            13 => ExcessiveRemotePeers,
            other => Unknown(other),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Map {
    pub nonce: [u8; 12],
    pub internal: SocketAddrV4,
    pub suggested: SocketAddrV4,
    pub lifetime: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub result: ResultCode,
    pub lifetime: u32,
    pub epoch: u32,
    pub external_addr: Ipv6Addr,
    pub external_port: u16,
}

impl Map {
    pub fn encode(&self) -> [u8; 60] {
        let mut bytes = [0; 60];
        bytes[..2].copy_from_slice(&[2, 1]);
        bytes[4..8].copy_from_slice(&self.lifetime.to_be_bytes());
        bytes[8..24].copy_from_slice(&self.internal.ip().to_ipv6_mapped().octets());
        bytes[24..36].copy_from_slice(&self.nonce);
        bytes[36] = 17;
        bytes[40..42].copy_from_slice(&self.internal.port().to_be_bytes());
        bytes[42..44].copy_from_slice(&self.suggested.port().to_be_bytes());
        bytes[44..].copy_from_slice(&self.suggested.ip().to_ipv6_mapped().octets());
        bytes
    }

    /// Ignore replies to other transactions, malformed packets and incomplete options.
    pub fn reply(&self, bytes: &[u8]) -> Option<Reply> {
        if !(60..=1100).contains(&bytes.len())
            || bytes[..2] != [2, 0x81]
            || bytes[24..36] != self.nonce
            || bytes[36] != 17
            || bytes[40..42] != self.internal.port().to_be_bytes()
        {
            return None;
        }
        let mut options = &bytes[60..];
        while !options.is_empty() {
            let len = u16::from_be_bytes(options.get(2..4)?.try_into().ok()?) as usize;
            options = options.get(4 + ((len + 3) & !3)..)?;
        }
        Some(Reply {
            result: bytes[3].into(),
            lifetime: u32::from_be_bytes(bytes[4..8].try_into().ok()?),
            epoch: u32::from_be_bytes(bytes[8..12].try_into().ok()?),
            external_addr: Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[44..60]).ok()?),
            external_port: u16::from_be_bytes([bytes[42], bytes[43]]),
        })
    }
}

/// RFC 6886 section 3.5 specifies opcode zero for an unsupported version.
/// Some implementations instead echo the request opcode with its response bit.
pub fn nat_pmp_only(bytes: &[u8]) -> bool {
    bytes.len() == 8 && bytes[0] == 0 && matches!(bytes[1], 0 | 128 | 129) && bytes[2..4] == [0, 1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn udp_map_wire_format() {
        let map = Map {
            nonce: *b"abcdefghijkl",
            internal: SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 7), 0x1234),
            suggested: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0x2345),
            lifetime: 0x01020304,
        };
        let mut wire = vec![2, 1, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 192, 0, 2, 7];
        wire.extend_from_slice(b"abcdefghijkl");
        wire.extend_from_slice(&[
            17, 0, 0, 0, 0x12, 0x34, 0x23, 0x45, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0,
        ]);
        assert_eq!(&map.encode()[..], wire, "MAP request layout");
        wire[..24].fill(0);
        wire[..12].copy_from_slice(&[2, 129, 0, 0, 0, 0, 0, 30, 0, 0, 0, 42]);
        wire[56..60].copy_from_slice(&[203, 0, 113, 5]);
        assert_eq!(
            map.reply(&wire),
            Some(Reply {
                result: ResultCode::Success,
                lifetime: 30,
                epoch: 42,
                external_addr: Ipv4Addr::new(203, 0, 113, 5).to_ipv6_mapped(),
                external_port: 0x2345,
            }),
            "granted mapping"
        );
        for end in 0..wire.len() {
            assert!(map.reply(&wire[..end]).is_none(), "accepted prefix {end}");
        }
        for field in [0, 1, 24, 36, 41] {
            wire[field] ^= 1;
            assert!(map.reply(&wire).is_none(), "accepted mismatched field {field}");
            wire[field] ^= 1;
        }
        wire[3] = 11;
        assert_eq!(map.reply(&wire).unwrap().result, ResultCode::CannotProvideExternal);
        assert!(nat_pmp_only(&[0, 0, 0, 1, 0, 0, 0, 42]), "legacy version negotiation");
    }
}
