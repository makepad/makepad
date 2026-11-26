//! TLS server on SecureTransport with an in-memory identity: the P-256 key
//! from its X9.63 bytes (SecKeyCreateWithData) and the certificate from its
//! DER, joined by SecIdentityCreate, so nothing touches a keychain.

use std::ffi::c_void;
use std::io;
use std::net::TcpStream;
use std::ptr;

use makepad_apple_sys::{
    kSecAttrKeyType, CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDictionaryCreate,
    CFErrorRef, CFRelease, CFStringRef, SecCertificateCreateWithData, SecIdentityCreate,
    SecIdentityRef, SecKeyCreateWithData, SecKeyRef,
};

use crate::socket_stream::apple_impl::NativeEngine;

// CommonCrypto is part of libSystem.
extern "C" {
    fn CCCryptorCreate(
        operation: u32, algorithm: u32, options: u32, key: *const u8, key_len: usize,
        iv: *const u8, cryptor: *mut *mut c_void,
    ) -> i32;
    fn CCCryptorUpdate(
        cryptor: *mut c_void, input: *const u8, input_len: usize, output: *mut u8,
        output_len: usize, written: *mut usize,
    ) -> i32;
    fn CCCryptorRelease(cryptor: *mut c_void) -> i32;
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecAttrKeyTypeECSECPrimeRandom: CFStringRef;
    static kSecAttrKeyClass: CFStringRef;
    static kSecAttrKeyClassPrivate: CFStringRef;
    static kSecAttrKeySizeInBits: CFStringRef;
    static kSecKeyAlgorithmECDSASignatureMessageX962SHA256: CFStringRef;
    static kSecKeyAlgorithmECDHKeyExchangeStandard: CFStringRef;
    fn SecKeyCopyKeyExchangeResult(
        key: SecKeyRef,
        algorithm: CFStringRef,
        peer: SecKeyRef,
        parameters: *const c_void,
        error: *mut CFErrorRef,
    ) -> *const c_void;
    fn SecKeyCreateRandomKey(parameters: *const c_void, error: *mut CFErrorRef) -> SecKeyRef;
    fn SecKeyCopyExternalRepresentation(key: SecKeyRef, error: *mut CFErrorRef) -> *const c_void;
    static kSecAttrKeyClassPublic: CFStringRef;
    fn SecKeyVerifySignature(
        key: SecKeyRef,
        algorithm: CFStringRef,
        data: *const c_void,
        signature: *const c_void,
        error: *mut CFErrorRef,
    ) -> u8;
    fn SecKeyCreateSignature(
        key: SecKeyRef,
        algorithm: CFStringRef,
        data: *const c_void,
        error: *mut CFErrorRef,
    ) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFNumberCreate(allocator: *const c_void, the_type: i32, value: *const c_void) -> *const c_void;
}

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

fn release_error(error: CFErrorRef) {
    if !error.is_null() {
        unsafe { CFRelease(error) };
    }
}

/// A CF dictionary {kSecAttrKeyType: EC, kSecAttrKeySizeInBits: 256}
/// (+ private class when `private`).
unsafe fn ec_attributes(private: bool) -> *const c_void {
    ec_attributes_class(if private { Some(kSecAttrKeyClassPrivate) } else { None })
}

unsafe fn ec_attributes_class(class: Option<CFStringRef>) -> *const c_void {
    let bits: i32 = 256;
    let number = CFNumberCreate(ptr::null(), 3, &bits as *const i32 as *const c_void);
    let mut keys: Vec<*const c_void> =
        vec![kSecAttrKeyType as *const c_void, kSecAttrKeySizeInBits as *const c_void];
    let mut values: Vec<*const c_void> =
        vec![kSecAttrKeyTypeECSECPrimeRandom as *const c_void, number];
    if let Some(class) = class {
        keys.push(kSecAttrKeyClass as *const c_void);
        values.push(class as *const c_void);
    }
    let dict = CFDictionaryCreate(
        ptr::null(),
        keys.as_ptr(),
        values.as_ptr(),
        keys.len() as isize,
        &kCFTypeDictionaryKeyCallBacks as *const c_void,
        &kCFTypeDictionaryValueCallBacks as *const c_void,
    );
    CFRelease(number);
    dict
}

unsafe fn cf_data_bytes(data: *const c_void) -> Vec<u8> {
    let len = CFDataGetLength(data) as usize;
    std::slice::from_raw_parts(CFDataGetBytePtr(data), len).to_vec()
}

struct Key(SecKeyRef);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

fn load_key(x963: &[u8; 97]) -> io::Result<Key> {
    unsafe {
        let data = CFDataCreate(ptr::null(), x963.as_ptr(), x963.len() as isize);
        let attrs = ec_attributes(true);
        let mut error: CFErrorRef = ptr::null();
        let key = SecKeyCreateWithData(data, attrs, &mut error);
        CFRelease(data);
        CFRelease(attrs);
        release_error(error);
        if key.is_null() {
            return Err(io_other("SecKeyCreateWithData rejected the TLS key"));
        }
        Ok(Key(key))
    }
}

pub fn generate_p256() -> io::Result<[u8; 97]> {
    unsafe {
        let attrs = ec_attributes(false);
        let mut error: CFErrorRef = ptr::null();
        let key = SecKeyCreateRandomKey(attrs, &mut error);
        CFRelease(attrs);
        release_error(error);
        if key.is_null() {
            return Err(io_other("SecKeyCreateRandomKey failed"));
        }
        let key = Key(key);
        let mut error: CFErrorRef = ptr::null();
        let data = SecKeyCopyExternalRepresentation(key.0, &mut error);
        release_error(error);
        if data.is_null() {
            return Err(io_other("SecKeyCopyExternalRepresentation failed"));
        }
        let bytes = cf_data_bytes(data);
        CFRelease(data);
        bytes
            .try_into()
            .map_err(|_| io_other("unexpected P-256 key representation"))
    }
}

pub fn ecdh_p256(key: &[u8; 97], peer_point: &[u8]) -> io::Result<[u8; 32]> {
    let key = load_key(key)?;
    unsafe {
        let data = CFDataCreate(ptr::null(), peer_point.as_ptr(), peer_point.len() as isize);
        let attrs = ec_attributes_class(Some(kSecAttrKeyClassPublic));
        let mut error: CFErrorRef = ptr::null();
        let peer = SecKeyCreateWithData(data, attrs, &mut error);
        CFRelease(data);
        CFRelease(attrs);
        release_error(error);
        if peer.is_null() {
            return Err(io_other("SecKeyCreateWithData rejected the P-256 peer point"));
        }
        let peer = Key(peer);
        let parameters = CFDictionaryCreate(ptr::null(), ptr::null(), ptr::null(), 0, ptr::null(), ptr::null());
        let mut error: CFErrorRef = ptr::null();
        let result = SecKeyCopyKeyExchangeResult(key.0, kSecKeyAlgorithmECDHKeyExchangeStandard, peer.0, parameters, &mut error);
        CFRelease(parameters);
        release_error(error);
        if result.is_null() {
            return Err(io_other("SecKeyCopyKeyExchangeResult failed"));
        }
        let bytes = cf_data_bytes(result);
        CFRelease(result);
        bytes.try_into().map_err(|_| io_other("unexpected P-256 shared secret length"))
    }
}

pub(super) struct NativeAes128Ecb(*mut c_void);

// SAFETY: CommonCrypto permits sequential use across threads. The handle is exclusively owned.
unsafe impl Send for NativeAes128Ecb {}

impl NativeAes128Ecb {
    pub(super) fn new(key: &[u8; 16]) -> io::Result<Self> {
        const CC_ENCRYPT: u32 = 0;
        const CC_ALGORITHM_AES: u32 = 0;
        const CC_OPTION_ECB_MODE: u32 = 2;
        let mut handle = ptr::null_mut();
        // SAFETY: CommonCrypto reads the 16-byte key and writes the new handle. ECB needs no IV.
        let status = unsafe {
            CCCryptorCreate(CC_ENCRYPT, CC_ALGORITHM_AES, CC_OPTION_ECB_MODE,
                key.as_ptr(), key.len(), ptr::null(), &mut handle)
        };
        if status != 0 {
            return Err(io_other(format!("CCCryptorCreate failed: {status}")));
        }
        Ok(Self(handle))
    }

    pub(super) fn encrypt_blocks(&mut self, blocks: &mut [u8]) -> io::Result<()> {
        // CCCryptorUpdate requires disjoint input and output for ECB.
        let mut output = [0u8; 64 * 16];
        for chunk in blocks.chunks_mut(output.len()) {
            let mut written = 0;
            // SAFETY: the buffers are separate and live for the call. With no padding or
            // pending partial block, the encrypted whole blocks fit in output.
            let status = unsafe {
                CCCryptorUpdate(self.0, chunk.as_ptr(), chunk.len(), output.as_mut_ptr(),
                    output.len(), &mut written)
            };
            if status != 0 || written != chunk.len() {
                return Err(io_other(format!("CCCryptorUpdate failed: {status}, wrote {written} bytes")));
            }
            chunk.copy_from_slice(&output[..written]);
        }
        Ok(())
    }
}

impl Drop for NativeAes128Ecb {
    fn drop(&mut self) {
        // SAFETY: this value owns the handle from CCCryptorCreate and releases it once.
        unsafe { CCCryptorRelease(self.0) };
    }
}

pub fn sign_p256_sha256(key: &[u8; 97], msg: &[u8]) -> io::Result<Vec<u8>> {
    let key = load_key(key)?;
    unsafe {
        let data = CFDataCreate(ptr::null(), msg.as_ptr(), msg.len() as isize);
        let mut error: CFErrorRef = ptr::null();
        let sig = SecKeyCreateSignature(
            key.0,
            kSecKeyAlgorithmECDSASignatureMessageX962SHA256,
            data,
            &mut error,
        );
        CFRelease(data);
        release_error(error);
        if sig.is_null() {
            return Err(io_other("SecKeyCreateSignature failed"));
        }
        let bytes = cf_data_bytes(sig);
        CFRelease(sig);
        Ok(bytes)
    }
}

pub fn verify_p256_sha256(point: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    unsafe {
        let data = CFDataCreate(ptr::null(), point.as_ptr(), point.len() as isize);
        let attrs = ec_attributes_class(Some(kSecAttrKeyClassPublic));
        let mut error: CFErrorRef = ptr::null();
        let key = SecKeyCreateWithData(data, attrs, &mut error);
        CFRelease(data);
        CFRelease(attrs);
        release_error(error);
        if key.is_null() {
            return false;
        }
        let key = Key(key);
        let msg = CFDataCreate(ptr::null(), msg.as_ptr(), msg.len() as isize);
        let sig = CFDataCreate(ptr::null(), sig.as_ptr(), sig.len() as isize);
        let mut error: CFErrorRef = ptr::null();
        let ok = SecKeyVerifySignature(key.0, kSecKeyAlgorithmECDSASignatureMessageX962SHA256, msg, sig, &mut error);
        CFRelease(msg);
        CFRelease(sig);
        release_error(error);
        ok != 0
    }
}

pub struct ServerConfig {
    identity: SecIdentityRef,
}

unsafe impl Send for ServerConfig {}
unsafe impl Sync for ServerConfig {}

impl ServerConfig {
    pub fn new(cert_der: &[u8], key: &[u8; 97]) -> io::Result<Self> {
        let key = load_key(key)?;
        unsafe {
            let data = CFDataCreate(ptr::null(), cert_der.as_ptr(), cert_der.len() as isize);
            let cert = SecCertificateCreateWithData(ptr::null(), data);
            CFRelease(data);
            if cert.is_null() {
                return Err(io_other("SecCertificateCreateWithData rejected the certificate"));
            }
            let identity = SecIdentityCreate(ptr::null(), cert, key.0);
            CFRelease(cert);
            if identity.is_null() {
                return Err(io_other("SecIdentityCreate failed"));
            }
            Ok(Self { identity })
        }
    }

    pub(crate) fn engine(&self, strict: bool) -> io::Result<NativeEngine> {
        NativeEngine::server(self.identity, strict)
    }

    pub fn accept(&self, tcp: TcpStream) -> io::Result<ServerStream> {
        let engine = crate::tls::TlsEngine::start(self.engine(false)?, None, false, None)?;
        ServerStream::handshake(tcp, engine, None)
    }
}

impl Drop for ServerConfig {
    fn drop(&mut self) {
        unsafe { CFRelease(self.identity) };
    }
}

pub(crate) type ServerStream = crate::tls::engine::SocketPump;
