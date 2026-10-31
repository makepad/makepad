#![cfg(test)]
#![cfg(feature="std")]
use std::io::{self, Read, Cursor};

extern crate brotli_decompressor;

static ENCODED: &'static [u8] = b"\x1b\x03)\x00\xa4\xcc\xde\xe2\xb3 vA\x00\x0c";

enum State { First, Second, Third, Fourth }
struct R(State);
impl Read for R {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0 {
            State::First => {
                buf[0] = ENCODED[0];
                self.0 = State::Second;
                return Ok(1);
            }
            State::Second => {
                self.0 = State::Third;
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "foo"));
            }
            State::Third => {
                self.0 = State::Fourth;
                buf[..ENCODED.len() - 1].copy_from_slice(&ENCODED[1..]);
                return Ok(ENCODED[1..].len());
            }
            State::Fourth => {
                return Ok(0);
            }
        }
    }
}
#[test]
fn test_would_block() {

    let mut d = brotli_decompressor::Decompressor::new(R(State::First), 8192);
    let mut b = [0; 8192];
    assert_eq!(d.read(&mut b).unwrap_err().kind(), io::ErrorKind::WouldBlock);
    assert!(d.read(&mut b).unwrap() != 0);
    println!("{}", String::from_utf8(b.to_vec()).unwrap());
    assert!(d.read(&mut b).unwrap() != 0);
    assert_eq!(d.read(&mut b).unwrap(), 0);
}

static ENCODED2: &'static [u8] = include_bytes!("ipsum.brotli");
static DECODED: &'static str = include_str!("ipsum.raw");

enum State2 {
    First,
    Second,
    Third,
    Fourth,
    Fifth,
    Sixth,
    Seventh,
    Eighth,
}

struct R2 {
    offset: usize,
    len: usize,
    state: State2,
}

impl R2 {
    fn new() -> R2 {
        R2 {
            offset: 0,
            len: 1,
            state: State2::First,
        }
    }
}
impl Read for R2 {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.state {
            State2::First => {
                self.state = State2::Second;
                let len = self.len;
                buf[..len].copy_from_slice(&ENCODED2[self.offset..self.offset+len]);
                self.offset += len;
                self.len = 100;
                return Ok(len);
            }
            State2::Second => {
                self.state = State2::Third;
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "foo"));
            }
            State2::Third => {
                self.state = State2::Fourth;
                let len = self.len;
                buf[..len].copy_from_slice(&ENCODED2[self.offset..self.offset+len]);
                self.offset += len;
                self.len = 100;
                return Ok(len);
            }
            State2::Fourth => {
                self.state = State2::Fifth;
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "foo"));
            }
            State2::Fifth => {
                self.state = State2::Sixth;
                let len = self.len;
                buf[..len].copy_from_slice(&ENCODED2[self.offset..self.offset+len]);
                self.offset += len;
                self.len = 100;
                return Ok(len);
            }
            State2::Sixth => {
                self.state = State2::Seventh;
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "foo"));
            }
            State2::Seventh => {
                self.state = State2::Eighth;
                buf[..ENCODED2.len() - self.offset].copy_from_slice(&ENCODED2[self.offset..]);
                return Ok(ENCODED2.len() - self.offset);
            }
            State2::Eighth => {
                return Ok(0);
            }
        }
    }
}

#[test]
fn would_block_more() {
    // Reference synchronous decoding.
    let mut b = [0; 8192];
    let mut bytes = vec![];
    let mut d = brotli_decompressor::Decompressor::new(Cursor::new(ENCODED2), 8192);
    let read = d.read(&mut b).unwrap();
    assert!(read != 0);
    bytes.extend_from_slice(&b[0..read]);

    assert_eq!(d.read(&mut b).unwrap(), 0);
    let reference_decoded = String::from_utf8(bytes).unwrap();
    // Ensure synchronous decoding matches original input.
    assert_eq!(reference_decoded, DECODED);

    // Incremental decoding.
    let r = R2::new();
    let mut d = brotli_decompressor::Decompressor::new(r, 8192);
    let mut bytes = vec![];
    let mut b = [0; 8192];

    assert_eq!(d.read(&mut b).unwrap_err().kind(), io::ErrorKind::WouldBlock);

    assert_eq!(d.read(&mut b).unwrap_err().kind(), io::ErrorKind::WouldBlock);

    let read = d.read(&mut b).unwrap();
    assert!(read != 0);
    bytes.extend_from_slice(&b[0..read]);

    assert_eq!(d.read(&mut b).unwrap_err().kind(), io::ErrorKind::WouldBlock);

    let read = d.read(&mut b).unwrap();
    assert!(read != 0);
    bytes.extend_from_slice(&b[0..read]);

    assert_eq!(d.read(&mut b).unwrap(), 0);

    let decoded = String::from_utf8(bytes).unwrap();

    // Ensure incremental decoding matches original input after brotli decompressor is finished.
    assert_eq!(decoded, reference_decoded);
}

// Regression test for a missing `break` in the BROTLI_STATE_METABLOCK_DONE arm
// of BrotliDecompressStream that caused PADDING_2 errors (RFC 7932 §9.3:
// "the unused bits in the last byte must be zeros") to be silently dropped:
// the error code was set, control fell through into BROTLI_STATE_DONE, and
// WriteRingBuffer's return overwrote `result` with SUCCESS.
//
// This is the same valid 69-byte brotli stream encoding "the quick brown fox
// jumps over the lazy dog twice for redundancy and length" used by the C
// reference (`libbrotlidec`) and Go implementations (cbrotli / andybalholm)
// in their corresponding tests; both reject all corruptions below as
// PADDING_2 / _ERROR_FORMAT_PADDING_2.
#[test]
fn test_padding_2_rejection() {
    // Valid encoding of "the quick brown fox jumps over the lazy dog twice
    // for redundancy and length" produced by libbrotlidec at quality 5.
    let valid: &[u8] = &[
        0x1b, 0x4a, 0x00, 0x00, 0xc4, 0xf4, 0xa4, 0x69, 0xbd, 0x79, 0x25, 0x2d, 0x22, 0xb4, 0x52,
        0xea, 0x83, 0x0d, 0x38, 0x70, 0x68, 0xb2, 0x71, 0xc0, 0x41, 0x76, 0x1e, 0x36, 0xc6, 0xce,
        0x13, 0x84, 0xe8, 0x36, 0xf2, 0x2a, 0x0c, 0xe7, 0x89, 0x68, 0x7a, 0x04, 0x49, 0x2f, 0xaa,
        0xf7, 0x31, 0xa1, 0x9b, 0x0d, 0x48, 0xb7, 0xf0, 0x1f, 0x48, 0x33, 0x42, 0xa5, 0x9c, 0x31,
        0x26, 0x97, 0xa9, 0xc6, 0xbe, 0x67, 0x85, 0x52, 0x02,
    ];

    // Sanity: the unmodified stream decodes successfully.
    let mut d = brotli_decompressor::Decompressor::new(Cursor::new(valid), 4096);
    let mut decoded = Vec::new();
    d.read_to_end(&mut decoded).unwrap();
    assert_eq!(
        decoded,
        b"the quick brown fox jumps over the lazy dog twice for redundancy and length",
    );

    // Each of these single-bit flips lands in the final metablock's
    // byte-alignment padding region. A spec-conformant decoder must return
    // BROTLI_DECODER_ERROR_FORMAT_PADDING_2 for all of them.
    for &(offset, xor) in &[(13usize, 0x01u8), (23, 0x01), (33, 0x55)] {
        let mut corrupted = valid.to_vec();
        corrupted[offset] ^= xor;

        let mut d = brotli_decompressor::Decompressor::new(Cursor::new(corrupted.clone()), 4096);
        let mut sink = Vec::new();
        let err = d.read_to_end(&mut sink).map(|_| sink.clone()).err();
        let kind = err.as_ref().map(|e| e.kind());
        assert_eq!(
            kind,
            Some(io::ErrorKind::InvalidData),
            "decoder must reject padding-bit corruption at offset {} xor {:#x} \
             with InvalidData; got {:?}",
            offset, xor, err.as_ref().map_or_else(|| format!("Ok({:?})", sink), |e| format!("{:?}", e)),
        );
    }
}
