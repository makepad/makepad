//! Network byte order. The ACK is the next wanted segment. Credit reserves the segment
//! slots starting there. Echo identifies the actual datagram that prompted the ACK.
use crate::{tls::constant_time_eq, tunnel::hmac_sha256};

pub(super) const KIND: u8 = 0xe1;
pub(super) const OPEN: u8 = 1;
pub(super) const ACCEPT: u8 = 2;
pub(super) const DATA: u8 = 3;
pub(super) const ACK: u8 = 4;
pub(super) const FIN: u8 = 5;
pub(super) const PING: u8 = 6;
pub(super) const RESET: u8 = 7;
pub(super) const CHALLENGE: u8 = 8;
pub(super) const RESPONSE: u8 = 9;
const SIGNED: u8 = 0x80;
const HEADER: usize = 32;
pub(super) const SEGMENT: usize = 1200 - HEADER - 8;
pub(super) const MAX_PN: u64 = (1 << 48) - 1;

pub(super) fn number(bytes: &[u8]) -> u64 {
    let mut full = [0; 8];
    full[2..].copy_from_slice(bytes);
    u64::from_be_bytes(full)
}

/// Expand a serial number to its nearest representative around the local cursor.
pub(super) fn expand(low: u32, near: u64) -> Option<u64> {
    near.checked_add_signed(i64::from(low.wrapping_sub(near as u32) as i32))
}

pub(super) struct Packet<'a> {
    pub kind: u8,
    pub number: u64,
    pub seq: u32,
    pub ack: u32,
    pub credit: u16,
    pub echo: u64,
    pub body: &'a [u8],
}

impl<'a> Packet<'a> {
    pub fn encode(&self, id: &[u8; 8], key: Option<&[u8; 32]>) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER + self.body.len() + 8);
        bytes.push(KIND);
        bytes.push(self.kind | if key.is_some() { SIGNED } else { 0 });
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&self.number.to_be_bytes()[2..]);
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&self.ack.to_be_bytes());
        bytes.extend_from_slice(&self.credit.to_be_bytes());
        bytes.extend_from_slice(&self.echo.to_be_bytes()[2..]);
        bytes.extend_from_slice(self.body);
        if let Some(key) = key {
            let tag = hmac_sha256(key, &bytes);
            bytes.extend_from_slice(&tag[..8]);
        }
        bytes
    }

    /// Returns the packet and whether its tag was verified with the receive key.
    pub fn decode(bytes: &'a [u8], id: &[u8; 8], key: Option<&[u8; 32]>) -> Option<(Self, bool)> {
        if !(HEADER..=1200).contains(&bytes.len()) || bytes[0] != KIND || bytes[2..10] != *id {
            return None;
        }
        let signed = bytes[1] & SIGNED != 0;
        let end = bytes.len() - if signed { 8 } else { 0 };
        if end < HEADER {
            return None;
        }
        let verify_key = key.filter(|_| signed);
        if let Some(key) = verify_key {
            if !constant_time_eq(&hmac_sha256(key, &bytes[..end])[..8], &bytes[end..]) {
                return None;
            }
        }
        let kind = bytes[1] & !SIGNED;
        let body = &bytes[HEADER..end];
        let valid = match kind {
            DATA => (1..=SEGMENT).contains(&body.len()),
            CHALLENGE | RESPONSE => body.len() == 8 && verify_key.is_some(),
            FIN | RESET => body.is_empty() && verify_key.is_some(),
            OPEN | ACCEPT | PING | ACK => body.is_empty(),
            _ => false,
        };
        let pn = number(&bytes[10..16]);
        if !valid || pn == 0 {
            return None;
        }
        let packet = Self {
            kind,
            number: pn,
            seq: u32::from_be_bytes(bytes[16..20].try_into().ok()?),
            ack: u32::from_be_bytes(bytes[20..24].try_into().ok()?),
            credit: u16::from_be_bytes(bytes[24..26].try_into().ok()?),
            echo: number(&bytes[26..32]),
            body,
        };
        Some((packet, verify_key.is_some()))
    }
}
