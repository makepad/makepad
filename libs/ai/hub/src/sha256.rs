//! Streaming SHA-256 for download verification: model files are tens of GB,
//! so they are hashed in chunks straight off the download, never held in
//! memory. The implementation is the shared one in `makepad-core-util`; this
//! module keeps the `makepad_ai_hub::sha256::*` paths and the hub's own
//! vectors below.

pub use makepad_core_util::sha256::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn streaming_matches_oneshot() {
        let data: Vec<u8> = (0..100_000u32).map(|v| (v % 251) as u8).collect();
        let oneshot = sha256_hex(&data);
        // Feed in awkward chunk sizes to cross block boundaries.
        let mut hasher = Sha256::new();
        let mut pos = 0;
        let mut step = 1;
        while pos < data.len() {
            let end = (pos + step).min(data.len());
            hasher.update(&data[pos..end]);
            pos = end;
            step = step * 7 % 1023 + 1;
        }
        assert_eq!(to_hex(&hasher.finish()), oneshot);
    }
}
