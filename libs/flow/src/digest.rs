//! SHA-256 for the engine core.
//!
//! The core hashes values, run identities and graph digests with the shared
//! zero-dependency implementation in `makepad-core-util`, so a
//! `document`/`runtime` build links neither the AI hub nor an asset crate.

pub(crate) struct Sha256(makepad_core_util::sha256::Sha256);

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self(makepad_core_util::sha256::Sha256::new())
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub(crate) fn finish(self) -> [u8; 32] {
        self.0.finalize()
    }
}

pub(crate) use makepad_core_util::sha256::to_hex;

/// A sha256 digest from lower-case hex, no `sha256:` prefix.
// Its one caller is the native run store.
#[cfg(all(feature = "runtime", not(target_arch = "wasm32")))]
pub(crate) fn from_hex_32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let mut out = [0; 32];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(out)
}

#[cfg(all(feature = "runtime", not(target_arch = "wasm32")))]
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
