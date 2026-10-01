//! TLS server on SecureTransport with an in-memory identity: the P-256 key
//! from its X9.63 bytes (SecKeyCreateWithData) and the certificate from its
//! DER, joined by SecIdentityCreate, so nothing touches a keychain.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::ptr;

use makepad_apple_sys::{
    kSecAttrKeyType, CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDictionaryCreate,
    CFErrorRef, CFRelease, CFStringRef, SecCertificateCreateWithData, SecIdentityCreate,
    SecIdentityRef, SecKeyCreateWithData, SecKeyRef,
};

use crate::socket_stream::apple_impl::SecureTransportStream;

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecAttrKeyTypeECSECPrimeRandom: CFStringRef;
    static kSecAttrKeyClass: CFStringRef;
    static kSecAttrKeyClassPrivate: CFStringRef;
    static kSecAttrKeySizeInBits: CFStringRef;
    static kSecKeyAlgorithmECDSASignatureMessageX962SHA256: CFStringRef;
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

    pub fn accept(&self, tcp: TcpStream) -> io::Result<ServerStream> {
        Ok(ServerStream(SecureTransportStream::accept(tcp, self.identity)?))
    }
}

impl Drop for ServerConfig {
    fn drop(&mut self) {
        unsafe { CFRelease(self.identity) };
    }
}

pub struct ServerStream(SecureTransportStream);

impl ServerStream {
    pub fn tcp(&self) -> &TcpStream {
        self.0.tcp()
    }

    pub fn shutdown(&mut self) {
        self.0.shutdown();
    }
}

impl Read for ServerStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for ServerStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
