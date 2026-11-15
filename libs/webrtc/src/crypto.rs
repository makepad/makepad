//! The primitives the protocol needs, from the system's libcrypto (the same
//! OpenSSL the network layer's TLS links): HMAC-SHA1 and AES-128 in counter
//! mode. Nothing here keeps a secret longer than its owner does.

use std::ffi::{c_int, c_uchar, c_uint, c_void};
use std::ptr;

#[link(name = "crypto")]
extern "C" {
    fn EVP_sha1() -> *const c_void;
    fn HMAC(
        md: *const c_void,
        key: *const c_void,
        key_len: c_int,
        data: *const c_uchar,
        data_len: usize,
        out: *mut c_uchar,
        out_len: *mut c_uint,
    ) -> *mut c_uchar;
    fn EVP_aes_128_ctr() -> *const c_void;
    fn EVP_CIPHER_CTX_new() -> *mut c_void;
    fn EVP_CIPHER_CTX_free(ctx: *mut c_void);
    fn EVP_EncryptInit_ex(ctx: *mut c_void, cipher: *const c_void, engine: *mut c_void, key: *const u8, iv: *const u8) -> c_int;
    fn EVP_EncryptUpdate(ctx: *mut c_void, out: *mut u8, out_len: *mut c_int, input: *const u8, input_len: c_int) -> c_int;
}

/// HMAC-SHA1 of the concatenation of `parts` under `key`.
pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let data: Vec<u8> = parts.concat();
    let mut out = [0u8; 20];
    let mut len: c_uint = 0;
    // SAFETY: every pointer is to a live buffer of the length passed; `out`
    // holds the 20 bytes SHA-1 writes.
    let done = unsafe {
        HMAC(EVP_sha1(), key.as_ptr() as *const c_void, key.len() as c_int, data.as_ptr(), data.len(), out.as_mut_ptr(), &mut len)
    };
    assert!(!done.is_null() && len == 20, "libcrypto HMAC-SHA1 failed");
    out
}

/// AES-128 in counter mode under one key; each call starts at its own
/// 128-bit initial counter block.
pub struct AesCtr {
    ctx: *mut c_void,
}

// SAFETY: the context is owned by this value alone and libcrypto keeps no
// thread affinity for it.
unsafe impl Send for AesCtr {}

impl AesCtr {
    pub fn new(key: &[u8; 16]) -> AesCtr {
        // SAFETY: a fresh context, initialised with a 16-byte key; the IV
        // comes with each `apply`.
        unsafe {
            let ctx = EVP_CIPHER_CTX_new();
            assert!(!ctx.is_null(), "libcrypto: no cipher context");
            let ok = EVP_EncryptInit_ex(ctx, EVP_aes_128_ctr(), ptr::null_mut(), key.as_ptr(), ptr::null());
            assert!(ok == 1, "libcrypto: AES-128-CTR init failed");
            AesCtr { ctx }
        }
    }

    /// XORs `data` in place with the keystream that starts at `iv`.
    pub fn apply(&mut self, iv: &[u8; 16], data: &mut [u8]) {
        // SAFETY: the context holds the key; a null cipher and key keep
        // both and set only the counter. In and out may be the same buffer
        // for a stream mode.
        unsafe {
            let ok = EVP_EncryptInit_ex(self.ctx, ptr::null(), ptr::null_mut(), ptr::null(), iv.as_ptr());
            assert!(ok == 1, "libcrypto: AES-128-CTR counter failed");
            for chunk in data.chunks_mut(1 << 30) {
                let mut out_len: c_int = 0;
                let ok = EVP_EncryptUpdate(self.ctx, chunk.as_mut_ptr(), &mut out_len, chunk.as_ptr(), chunk.len() as c_int);
                assert!(ok == 1 && out_len as usize == chunk.len(), "libcrypto: AES-128-CTR failed");
            }
        }
    }

    /// `len` bytes of keystream from `iv`.
    pub fn keystream(&mut self, iv: &[u8; 16], len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        self.apply(iv, &mut out);
        out
    }
}

impl Drop for AesCtr {
    fn drop(&mut self) {
        // SAFETY: created by EVP_CIPHER_CTX_new and freed once.
        unsafe { EVP_CIPHER_CTX_free(self.ctx) }
    }
}

#[cfg(test)]
pub(crate) fn hex(text: &str) -> Vec<u8> {
    let text: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_and_counter_mode_match_their_standards() {
        // RFC 2202, HMAC-SHA1 test case 1.
        let mac = hmac_sha1(&[0x0b; 20], &[b"Hi ", b"There"]);
        assert_eq!(mac.to_vec(), hex("b617318655057264e28bc0b6fb378c8ef146be00"), "HMAC-SHA1 over parts");
        // NIST SP 800-38A F.5.1, CTR-AES128.Encrypt, first two blocks.
        let mut ctr = AesCtr::new(&hex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap());
        let iv: [u8; 16] = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff").try_into().unwrap();
        let mut data = hex("6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51");
        ctr.apply(&iv, &mut data);
        assert_eq!(data, hex("874d6191b620e3261bef6864990db6ce 9806f66b7970fdff8617187bb9fffdff"), "AES-128-CTR");
        // The counter restarts with each call: the same IV gives the same stream.
        assert_eq!(ctr.keystream(&iv, 16), ctr.keystream(&iv, 16));
    }
}
