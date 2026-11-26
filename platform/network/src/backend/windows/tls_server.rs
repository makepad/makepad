//! TLS server on Schannel (SSPI `AcceptSecurityContext`).
//!
//! Keys: generated and used for signing with BCrypt (ECDSA P-256). For the
//! handshake, Schannel reopens a named key in the account's CNG store. Each
//! credential owns its key until its last session drops. The first use in a
//! process also cleans up containers whose owning processes have exited.
//!
//! Plain Win32 C APIs (SSPI, crypt32, CNG) declared here; no COM.

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::io;
use std::net::TcpStream;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Once};
use crate::tls::{engine::SocketPump, TlsEngine, TlsIdentity};

type SecurityStatus = i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SecHandle {
    lower: usize,
    upper: usize,
}

#[repr(C)]
struct SecBuffer {
    cb: u32,
    kind: u32,
    pv: *mut c_void,
}

#[repr(C)]
struct SecBufferDesc {
    version: u32,
    count: u32,
    buffers: *mut SecBuffer,
}

#[repr(C)]
#[derive(Default)]
struct StreamSizes {
    header: u32,
    trailer: u32,
    max_message: u32,
    buffers: u32,
    block_size: u32,
}

#[repr(C)]
struct TlsParameters {
    alpn_count: u32,
    alpn_ids: *mut c_void,
    disabled_protocols: u32,
    disabled_crypto_count: u32,
    disabled_crypto: *mut c_void,
    flags: u32,
}

#[repr(C)]
struct SchCredentials {
    version: u32,
    cred_format: u32,
    cert_count: u32,
    certs: *const *const c_void,
    root_store: *mut c_void,
    mapper_count: u32,
    mappers: *mut c_void,
    session_lifespan: u32,
    flags: u32,
    tls_parameter_count: u32,
    tls_parameters: *mut TlsParameters,
}

#[repr(C)]
struct SchannelCred {
    version: u32,
    cert_count: u32,
    certs: *const *const c_void,
    root_store: *mut c_void,
    mapper_count: u32,
    mappers: *mut c_void,
    alg_count: u32,
    algs: *mut u32,
    enabled_protocols: u32,
    min_cipher_strength: u32,
    max_cipher_strength: u32,
    session_lifespan: u32,
    flags: u32,
    cred_format: u32,
}

#[repr(C)]
struct CryptKeyProvInfo {
    container_name: *const u16,
    prov_name: *const u16,
    prov_type: u32,
    flags: u32,
    prov_param_count: u32,
    prov_params: *mut c_void,
    key_spec: u32,
}

#[repr(C)]
struct NCryptBuffer {
    cb: u32,
    kind: u32,
    pv: *mut c_void,
}

#[repr(C)]
struct NCryptBufferDesc {
    version: u32,
    count: u32,
    buffers: *mut NCryptBuffer,
}

#[link(name = "secur32")]
extern "system" {
    fn AcquireCredentialsHandleW(
        principal: *const u16,
        package: *const u16,
        usage: u32,
        logon_id: *mut c_void,
        auth_data: *mut c_void,
        get_key_fn: *mut c_void,
        get_key_arg: *mut c_void,
        credential: *mut SecHandle,
        expiry: *mut i64,
    ) -> SecurityStatus;
    fn FreeCredentialsHandle(credential: *mut SecHandle) -> SecurityStatus;
    fn AcceptSecurityContext(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        input: *mut SecBufferDesc,
        context_req: u32,
        data_rep: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        context_attr: *mut u32,
        expiry: *mut i64,
    ) -> SecurityStatus;
    fn InitializeSecurityContextW(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        target: *const u16,
        context_req: u32,
        reserved1: u32,
        data_rep: u32,
        input: *mut SecBufferDesc,
        reserved2: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        context_attr: *mut u32,
        expiry: *mut i64,
    ) -> SecurityStatus;
    fn DeleteSecurityContext(context: *mut SecHandle) -> SecurityStatus;
    fn FreeContextBuffer(buffer: *mut c_void) -> SecurityStatus;
    fn QueryContextAttributesW(context: *mut SecHandle, attribute: u32, buffer: *mut c_void) -> SecurityStatus;
    fn EncryptMessage(context: *mut SecHandle, qop: u32, message: *mut SecBufferDesc, seq: u32) -> SecurityStatus;
    fn DecryptMessage(context: *mut SecHandle, message: *mut SecBufferDesc, seq: u32, qop: *mut u32) -> SecurityStatus;
}

#[link(name = "crypt32")]
extern "system" {
    fn CertCreateCertificateContext(encoding: u32, der: *const u8, len: u32) -> *const c_void;
    fn CertFreeCertificateContext(cert: *const c_void) -> i32;
    fn CertEnumCertificatesInStore(store: *mut c_void, previous: *const c_void) -> *const c_void;
    fn CertSetCertificateContextProperty(cert: *const c_void, prop: u32, flags: u32, data: *const c_void) -> i32;
}

#[link(name = "ncrypt", kind = "raw-dylib")]
extern "system" {
    fn NCryptOpenStorageProvider(provider: *mut usize, name: *const u16, flags: u32) -> SecurityStatus;
    fn NCryptImportKey(
        provider: usize,
        import_key: usize,
        blob_type: *const u16,
        parameters: *const NCryptBufferDesc,
        key: *mut usize,
        data: *const u8,
        len: u32,
        flags: u32,
    ) -> SecurityStatus;
    fn NCryptFreeObject(object: usize) -> SecurityStatus;
    fn NCryptOpenKey(provider: usize, key: *mut usize, name: *const u16, legacy_key_spec: u32, flags: u32) -> SecurityStatus;
    fn NCryptEnumKeys(provider: usize, scope: *const u16, name: *mut *mut KeyName, state: *mut *mut c_void, flags: u32) -> SecurityStatus;
    fn NCryptFreeBuffer(buffer: *mut c_void) -> SecurityStatus;
    fn NCryptDeleteKey(key: usize, flags: u32) -> SecurityStatus;
}

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(alg: *mut *mut c_void, id: *const u16, imp: *const u16, flags: u32) -> i32;
    fn BCryptCloseAlgorithmProvider(alg: *mut c_void, flags: u32) -> i32;
    fn BCryptSetProperty(handle: *mut c_void, name: *const u16, value: *const u8, len: u32, flags: u32) -> i32;
    fn BCryptGenerateSymmetricKey(
        alg: *mut c_void, key: *mut *mut c_void, object: *mut u8, object_len: u32,
        secret: *const u8, secret_len: u32, flags: u32,
    ) -> i32;
    fn BCryptEncrypt(
        key: *mut c_void, input: *const u8, input_len: u32, padding: *mut c_void,
        iv: *mut u8, iv_len: u32, output: *mut u8, output_len: u32, written: *mut u32, flags: u32,
    ) -> i32;
    fn BCryptGenerateKeyPair(alg: *mut c_void, key: *mut *mut c_void, bits: u32, flags: u32) -> i32;
    fn BCryptFinalizeKeyPair(key: *mut c_void, flags: u32) -> i32;
    fn BCryptExportKey(
        key: *mut c_void,
        export_key: *mut c_void,
        blob_type: *const u16,
        out: *mut u8,
        out_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
    fn BCryptImportKeyPair(
        alg: *mut c_void,
        import_key: *mut c_void,
        blob_type: *const u16,
        key: *mut *mut c_void,
        data: *const u8,
        len: u32,
        flags: u32,
    ) -> i32;
    fn BCryptSignHash(
        key: *mut c_void,
        padding: *mut c_void,
        hash: *const u8,
        hash_len: u32,
        sig: *mut u8,
        sig_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
    fn BCryptDestroyKey(key: *mut c_void) -> i32;
    fn BCryptSecretAgreement(private_key: *mut c_void, public_key: *mut c_void, secret: *mut *mut c_void, flags: u32) -> i32;
    fn BCryptDeriveKey(
        secret: *mut c_void,
        kdf: *const u16,
        parameters: *mut c_void,
        out: *mut u8,
        out_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
    fn BCryptDestroySecret(secret: *mut c_void) -> i32;
    fn BCryptVerifySignature(
        key: *mut c_void,
        padding: *mut c_void,
        hash: *const u8,
        hash_len: u32,
        sig: *const u8,
        sig_len: u32,
        flags: u32,
    ) -> i32;
}

const SECPKG_CRED_INBOUND: u32 = 1;
const SECPKG_CRED_OUTBOUND: u32 = 2;
const SCH_CRED_MANUAL_CRED_VALIDATION: u32 = 0x0000_0008;
const SCH_CRED_NO_DEFAULT_CREDS: u32 = 0x0000_0010;
const SP_PROT_TLS1_2_CLIENT: u32 = 0x0000_0800;
const ISC_FLAGS: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x4000 | 0x8000;
const SECPKG_ATTR_REMOTE_CERT_CONTEXT: u32 = 0x53;
const SCH_CREDENTIALS_VERSION: u32 = 5;
const SCHANNEL_CRED_VERSION: u32 = 4;
const SCH_USE_STRONG_CRYPTO: u32 = 0x0040_0000;
const SP_PROT_TLS1_2_SERVER: u32 = 0x0000_0400;
const SP_PROT_TLS1_2: u32 = 0x0000_0C00;
const SP_PROT_TLS1_3: u32 = 0x0000_3000;

const ASC_REQ_REPLAY_DETECT: u32 = 0x4;
const ASC_REQ_SEQUENCE_DETECT: u32 = 0x8;
const ASC_REQ_CONFIDENTIALITY: u32 = 0x10;
const ASC_REQ_ALLOCATE_MEMORY: u32 = 0x100;
const ASC_REQ_EXTENDED_ERROR: u32 = 0x8000;
const ASC_REQ_STREAM: u32 = 0x10000;
const ASC_FLAGS: u32 = ASC_REQ_REPLAY_DETECT
    | ASC_REQ_SEQUENCE_DETECT
    | ASC_REQ_CONFIDENTIALITY
    | ASC_REQ_ALLOCATE_MEMORY
    | ASC_REQ_EXTENDED_ERROR
    | ASC_REQ_STREAM;
const SECURITY_NATIVE_DREP: u32 = 0x10;

const SECBUFFER_EMPTY: u32 = 0;
const SECBUFFER_DATA: u32 = 1;
const SECBUFFER_TOKEN: u32 = 2;
const SECBUFFER_EXTRA: u32 = 5;
const SECBUFFER_STREAM_TRAILER: u32 = 6;
const SECBUFFER_STREAM_HEADER: u32 = 7;

const SEC_E_OK: i32 = 0;
const SEC_I_CONTINUE_NEEDED: i32 = 0x0009_0312;
const SEC_I_CONTEXT_EXPIRED: i32 = 0x0009_0317;
const SEC_I_RENEGOTIATE: i32 = 0x0009_0321;
/// The server asked for a client certificate (FTPS servers often do).
const SEC_I_INCOMPLETE_CREDENTIALS: i32 = 0x0009_0320;
const SEC_E_INCOMPLETE_MESSAGE: i32 = 0x8009_0318u32 as i32;
const SECPKG_ATTR_STREAM_SIZES: u32 = 4;
const SECPKG_ATTR_SESSION_INFO: u32 = 0x5d;
const SSL_SESSION_RECONNECT: u32 = 1;

#[repr(C)]
struct SessionInfo {
    flags: u32,
    session_id_len: u32,
    session_id: [u8; 32],
}

const X509_ASN_ENCODING: u32 = 0x1 | 0x10000;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const NCRYPTBUFFER_PKCS_KEY_NAME: u32 = 45;
const ECDSA_PRIVATE_P256_MAGIC: u32 = 0x3253_4345;
const ECDH_PRIVATE_P256_MAGIC: u32 = 0x324b_4345;
const ECDH_PUBLIC_P256_MAGIC: u32 = 0x314b_4345;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn status_error(what: &str, status: i32) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("{what} failed: 0x{:08x}", status as u32))
}

/// X9.63 (04||X||Y||d) to BCRYPT_ECCPRIVATE_BLOB.
fn ecc_private_blob(key: &[u8; 97]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(8 + 96);
    blob.extend_from_slice(&ECDSA_PRIVATE_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(&key[1..]);
    blob
}

struct Alg(*mut c_void);
impl Drop for Alg {
    fn drop(&mut self) {
        // SAFETY: Alg owns the provider handle and closes it once.
        unsafe { BCryptCloseAlgorithmProvider(self.0, 0) };
    }
}
struct BKey(*mut c_void);
impl Drop for BKey {
    fn drop(&mut self) {
        // SAFETY: BKey owns the key handle and destroys it once.
        unsafe { BCryptDestroyKey(self.0) };
    }
}

fn open_ecdsa() -> io::Result<Alg> {
    let mut alg = null_mut();
    let st = unsafe { BCryptOpenAlgorithmProvider(&mut alg, wide("ECDSA_P256").as_ptr(), null(), 0) };
    if st != 0 {
        return Err(status_error("BCryptOpenAlgorithmProvider", st));
    }
    Ok(Alg(alg))
}

pub fn generate_p256() -> io::Result<[u8; 97]> {
    let alg = open_ecdsa()?;
    let mut key = null_mut();
    let st = unsafe { BCryptGenerateKeyPair(alg.0, &mut key, 256, 0) };
    if st != 0 {
        return Err(status_error("BCryptGenerateKeyPair", st));
    }
    let key = BKey(key);
    let st = unsafe { BCryptFinalizeKeyPair(key.0, 0) };
    if st != 0 {
        return Err(status_error("BCryptFinalizeKeyPair", st));
    }
    let mut blob = [0u8; 8 + 96];
    let mut len = 0u32;
    let st = unsafe {
        BCryptExportKey(key.0, null_mut(), wide("ECCPRIVATEBLOB").as_ptr(), blob.as_mut_ptr(), blob.len() as u32, &mut len, 0)
    };
    if st != 0 || len as usize != blob.len() {
        return Err(status_error("BCryptExportKey", st));
    }
    let mut out = [0u8; 97];
    out[0] = 0x04;
    out[1..].copy_from_slice(&blob[8..]);
    Ok(out)
}

pub fn ecdh_p256(key: &[u8; 97], peer_point: &[u8]) -> io::Result<[u8; 32]> {
    let mut alg = null_mut();
    let st = unsafe { BCryptOpenAlgorithmProvider(&mut alg, wide("ECDH_P256").as_ptr(), null(), 0) };
    if st != 0 {
        return Err(status_error("BCryptOpenAlgorithmProvider(ECDH_P256)", st));
    }
    let alg = Alg(alg);
    let mut blob = ecc_private_blob(key);
    blob[..4].copy_from_slice(&ECDH_PRIVATE_P256_MAGIC.to_le_bytes());
    let mut private = null_mut();
    let st = unsafe {
        BCryptImportKeyPair(alg.0, null_mut(), wide("ECCPRIVATEBLOB").as_ptr(), &mut private, blob.as_ptr(), blob.len() as u32, 0)
    };
    if st != 0 {
        return Err(status_error("BCryptImportKeyPair(ECDH private)", st));
    }
    let private = BKey(private);
    let mut blob = Vec::with_capacity(8 + 64);
    blob.extend_from_slice(&ECDH_PUBLIC_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(&peer_point[1..65]);
    let mut peer = null_mut();
    // With flags = 0, CNG validates the imported public point.
    let st = unsafe {
        BCryptImportKeyPair(alg.0, null_mut(), wide("ECCPUBLICBLOB").as_ptr(), &mut peer, blob.as_ptr(), blob.len() as u32, 0)
    };
    if st != 0 {
        return Err(status_error("BCryptImportKeyPair(ECDH peer)", st));
    }
    let peer = BKey(peer);
    let mut secret = null_mut();
    let st = unsafe { BCryptSecretAgreement(private.0, peer.0, &mut secret, 0) };
    if st != 0 {
        return Err(status_error("BCryptSecretAgreement", st));
    }
    let mut out = [0u8; 32];
    let mut len = 0u32;
    let st = unsafe {
        // BCRYPT_KDF_RAW_SECRET (TRUNCATE) returns the raw secret little-endian.
        let st = BCryptDeriveKey(secret, wide("TRUNCATE").as_ptr(), null_mut(), out.as_mut_ptr(), 32, &mut len, 0);
        BCryptDestroySecret(secret);
        st
    };
    if st != 0 || len != 32 {
        return Err(status_error("BCryptDeriveKey(RAW_SECRET)", st));
    }
    out.reverse();
    Ok(out)
}

pub(super) struct NativeAes128Ecb {
    key: BKey,
    // Rust drops fields in declaration order, so the provider outlives its key.
    _provider: Alg,
}

// SAFETY: the CNG handles have no thread affinity and are exclusively owned.
unsafe impl Send for NativeAes128Ecb {}

impl NativeAes128Ecb {
    pub(super) fn new(key: &[u8; 16]) -> io::Result<Self> {
        let mut handle = null_mut();
        // SAFETY: the algorithm name is NUL-terminated and handle is a writable out parameter.
        let st = unsafe { BCryptOpenAlgorithmProvider(&mut handle, wide("AES").as_ptr(), null(), 0) };
        if st != 0 {
            return Err(status_error("BCryptOpenAlgorithmProvider(AES)", st));
        }
        let provider = Alg(handle);
        let mode = wide("ChainingModeECB");
        // SAFETY: the property name and value are live UTF-16 strings including their terminators.
        let st = unsafe {
            BCryptSetProperty(provider.0, wide("ChainingMode").as_ptr(), mode.as_ptr().cast(),
                (mode.len() * 2) as u32, 0)
        };
        if st != 0 {
            return Err(status_error("BCryptSetProperty(ChainingMode)", st));
        }
        let mut handle = null_mut();
        // SAFETY: CNG reads the 16-byte key and manages its own key-object storage with null/0.
        let st = unsafe {
            BCryptGenerateSymmetricKey(provider.0, &mut handle, null_mut(), 0, key.as_ptr(), 16, 0)
        };
        if st != 0 {
            return Err(status_error("BCryptGenerateSymmetricKey", st));
        }
        Ok(Self { key: BKey(handle), _provider: provider })
    }

    pub(super) fn encrypt_blocks(&mut self, blocks: &mut [u8]) -> io::Result<()> {
        let data = blocks.as_mut_ptr();
        let len = blocks.len() as u32;
        let mut written = 0;
        // SAFETY: the caller bounds len to whole blocks that fit u32. BCrypt permits identical
        // input and output buffers and ECB needs neither IV nor padding.
        let st = unsafe {
            BCryptEncrypt(self.key.0, data, len, null_mut(), null_mut(), 0, data, len, &mut written, 0)
        };
        if st != 0 {
            return Err(status_error("BCryptEncrypt", st));
        }
        if written != len {
            return Err(io::Error::other("BCryptEncrypt returned a partial block batch"));
        }
        Ok(())
    }
}

pub fn sign_p256_sha256(key: &[u8; 97], msg: &[u8]) -> io::Result<Vec<u8>> {
    let alg = open_ecdsa()?;
    let blob = ecc_private_blob(key);
    let mut handle = null_mut();
    let st = unsafe {
        BCryptImportKeyPair(alg.0, null_mut(), wide("ECCPRIVATEBLOB").as_ptr(), &mut handle, blob.as_ptr(), blob.len() as u32, 0)
    };
    if st != 0 {
        return Err(status_error("BCryptImportKeyPair", st));
    }
    let handle = BKey(handle);
    let digest = crate::digest::sha256_hash(msg);
    let mut raw = [0u8; 64];
    let mut len = 0u32;
    let st = unsafe { BCryptSignHash(handle.0, null_mut(), digest.as_ptr(), 32, raw.as_mut_ptr(), 64, &mut len, 0) };
    if st != 0 || len != 64 {
        return Err(status_error("BCryptSignHash", st));
    }
    Ok(crate::tls::x509::ecdsa_sig_from_raw(&raw))
}

pub fn verify_p256_sha256(point: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let Some(raw) = crate::tls::ecdsa_raw_from_der(sig) else { return false };
    let Ok(alg) = open_ecdsa() else { return false };
    let mut blob = Vec::with_capacity(8 + 64);
    blob.extend_from_slice(&0x3153_4345u32.to_le_bytes()); // ECDSA_PUBLIC_P256
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(&point[1..65]);
    let mut handle = null_mut();
    let st = unsafe {
        BCryptImportKeyPair(alg.0, null_mut(), wide("ECCPUBLICBLOB").as_ptr(), &mut handle, blob.as_ptr(), blob.len() as u32, 0)
    };
    if st != 0 {
        return false;
    }
    let handle = BKey(handle);
    let digest = crate::digest::sha256_hash(msg);
    unsafe { BCryptVerifySignature(handle.0, null_mut(), digest.as_ptr(), 32, raw.as_ptr(), 64, 0) == 0 }
}

const KEY_PROVIDER: &str = "Microsoft Software Key Storage Provider";

const CONTAINER_PREFIX: &str = "makepad-tls-session-";

#[repr(C)]
struct KeyName {
    name: *const u16,
    algorithm: *const u16,
    legacy_spec: u32,
    flags: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(object: *mut c_void, timeout: u32) -> u32;
    fn CloseHandle(object: *mut c_void) -> i32;
}

fn owner(name: &str) -> Option<u32> {
    let (pid, id) = name.strip_prefix(CONTAINER_PREFIX)?.split_once('-')?;
    let pid = pid.parse::<u32>().ok()?;
    (pid != 0 && id.len() == 32 && id.bytes().all(|v| v.is_ascii_hexdigit())).then_some(pid)
}

fn process_exited(pid: u32) -> bool {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            // ERROR_INVALID_PARAMETER means no such PID. Access denied and
            // other failures leave the owner's container alone.
            return io::Error::last_os_error().raw_os_error() == Some(87);
        }
        let exited = WaitForSingleObject(handle, 0) == 0;
        CloseHandle(handle);
        exited
    }
}

fn delete_container(provider: usize, name: &[u16]) -> bool {
    unsafe {
        let mut key = 0;
        if NCryptOpenKey(provider, &mut key, name.as_ptr(), 0, 0) != 0 { return false; }
        if NCryptDeleteKey(key, 0) == 0 { return true; } // releases the handle on success
        NCryptFreeObject(key);
        false
    }
}

struct Provider(usize);
impl Provider {
    fn open() -> io::Result<Self> {
        let mut raw = 0;
        let st = unsafe { NCryptOpenStorageProvider(&mut raw, wide(KEY_PROVIDER).as_ptr(), 0) };
        if st != 0 { return Err(status_error("NCryptOpenStorageProvider", st)); }
        Ok(Self(raw))
    }
}
impl Drop for Provider {
    fn drop(&mut self) { unsafe { NCryptFreeObject(self.0) }; }
}

fn sweep_orphans() -> io::Result<()> {
    let provider = Provider::open()?;
    let mut state = null_mut();
    let mut orphans = Vec::new();
    let status = loop {
        let mut entry: *mut KeyName = null_mut();
        let status = unsafe { NCryptEnumKeys(provider.0, null(), &mut entry, &mut state, 0) };
        if status != 0 { break status; }
        unsafe {
            let name = (*entry).name;
            if !name.is_null() {
                let mut len = 0;
                while *name.add(len) != 0 { len += 1; }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(name, len));
                if owner(&text).is_some_and(process_exited) { orphans.push(text); }
            }
            NCryptFreeBuffer(entry.cast());
        }
    };
    if !state.is_null() { unsafe { NCryptFreeBuffer(state) }; }
    // Finish enumerating before modifying the store. Another sweeper can
    // win a deletion; the per-credential random name is never reused.
    for name in orphans {
        if delete_container(provider.0, &wide(&name)) {
            eprintln!("tls: removed abandoned CNG key {name}");
        }
    }
    const NTE_NO_MORE_ITEMS: i32 = 0x8009_002au32 as i32;
    if status != NTE_NO_MORE_ITEMS { return Err(status_error("NCryptEnumKeys", status)); }
    Ok(())
}

fn sweep_once() {
    static SWEEP: Once = Once::new();
    SWEEP.call_once(|| {
        if let Err(e) = sweep_orphans() { eprintln!("tls: CNG key cleanup: {e}"); }
    });
}

fn container_name(pid: u32) -> io::Result<Vec<u16>> {
    let mut random = [0; 16];
    crate::tls::os_random(&mut random)?;
    Ok(wide(&format!("{CONTAINER_PREFIX}{pid}-{}", crate::tls::to_hex(&random))))
}

// Schannel reopens the signing key across the LSASS process boundary.
// For a strict connection it is fresh: the in-band proof authenticates
// the device and signs the leaf hash. ECDHE protects prior traffic even
// if this TLS key is later read. The named import is owned by this
// credential and swept after a crash. Never store the device key here
// for a strict session.
fn store_key(container: &[u16], key: &[u8; 97]) -> io::Result<()> {
    let provider = Provider::open()?;
    unsafe {
        let blob = ecc_private_blob(key);
        let mut name_buffer = NCryptBuffer {
            cb: (container.len() * 2) as u32,
            kind: NCRYPTBUFFER_PKCS_KEY_NAME,
            pv: container.as_ptr() as *mut c_void,
        };
        let params = NCryptBufferDesc { version: 0, count: 1, buffers: &mut name_buffer };
        let mut nkey = 0usize;
        let st = NCryptImportKey(
            provider.0,
            0,
            wide("ECCPRIVATEBLOB").as_ptr(),
            &params,
            &mut nkey,
            blob.as_ptr(),
            blob.len() as u32,
            0,
        );
        if nkey != 0 {
            NCryptFreeObject(nkey);
        }
        if st != 0 {
            return Err(status_error("NCryptImportKey", st));
        }
    }
    Ok(())
}

struct StoredKey(Vec<u16>);
impl Drop for StoredKey {
    fn drop(&mut self) {
        if !Provider::open().is_ok_and(|provider| delete_container(provider.0, &self.0)) {
            eprintln!("tls: could not delete the server's CNG key");
        }
    }
}

struct ServerCredential {
    cred: SecHandle,
    cert: *const c_void,
    _key: StoredKey,
}

unsafe impl Send for ServerCredential {}
unsafe impl Sync for ServerCredential {}

#[derive(Clone)]
pub struct ServerConfig(Arc<ServerCredential>);

impl ServerConfig {
    pub fn new(cert_der: &[u8], key: &[u8; 97]) -> io::Result<Self> {
        Self::create(cert_der, key, false, SP_PROT_TLS1_2 | SP_PROT_TLS1_3)
    }

    fn create(cert_der: &[u8], key: &[u8; 97], strict: bool, protocols: u32) -> io::Result<Self> {
        sweep_once();
        let container = container_name(std::process::id())?;
        store_key(&container, key)?;
        let stored = StoredKey(container);
        let provider_name = wide(KEY_PROVIDER);

        let cert = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, cert_der.as_ptr(), cert_der.len() as u32) };
        if cert.is_null() {
            return Err(io::Error::last_os_error());
        }
        let info = CryptKeyProvInfo {
            container_name: stored.0.as_ptr(),
            prov_name: provider_name.as_ptr(),
            prov_type: 0,
            flags: 0,
            prov_param_count: 0,
            prov_params: null_mut(),
            key_spec: 0,
        };
        if unsafe { CertSetCertificateContextProperty(cert, CERT_KEY_PROV_INFO_PROP_ID, 0, &info as *const _ as *const c_void) } == 0 {
            let err = io::Error::last_os_error();
            unsafe { CertFreeCertificateContext(cert) };
            return Err(err);
        }

        let cred = match credentials(Some(cert), strict, protocols) {
            Ok(cred) => cred,
            Err(e) => { unsafe { CertFreeCertificateContext(cert) }; return Err(e); }
        };
        Ok(Self(Arc::new(ServerCredential { cred, cert, _key: stored })))
    }

    pub fn accept(&self, tcp: TcpStream) -> io::Result<ServerStream> {
        let native = NativeEngine::new(Side::Server(self.clone()), false);
        SocketPump::handshake(tcp, TlsEngine::start(native, None, false, None)?, None)
    }
}

impl Drop for ServerCredential {
    fn drop(&mut self) {
        unsafe {
            FreeCredentialsHandle(&mut self.cred);
            CertFreeCertificateContext(self.cert);
        }
    }
}

pub(crate) type ServerStream = SocketPump;

/// A client credential handle. Schannel caches client sessions per
/// credential and target name, so connections that resume each other's
/// session share one.
struct ClientCred(SecHandle);

impl Drop for ClientCred {
    fn drop(&mut self) {
        unsafe {
            FreeCredentialsHandle(&mut self.0);
        }
    }
}

// The handle is only read after creation; SSPI handles are process-wide.
unsafe impl Send for ClientCred {}
unsafe impl Sync for ClientCred {}

struct ClientSide {
    target: Vec<u16>,
    /// Freed when the last connection on it is dropped (after its context).
    cred: Arc<ClientCred>,
}

#[repr(C)]
struct UnicodeString {
    len: u16,
    capacity: u16,
    text: *const u16,
}
impl UnicodeString {
    fn from_wide(text: &[u16]) -> Self {
        Self { len: ((text.len() - 1) * 2) as u16, capacity: (text.len() * 2) as u16, text: text.as_ptr() }
    }
}

#[repr(C)]
struct CryptoSettings {
    usage: u32,
    algorithm: UnicodeString,
    mode_count: u32,
    modes: *const UnicodeString,
    min_bits: u32,
    max_bits: u32,
}

fn credentials(cert: Option<*const c_void>, strict: bool, protocols: u32) -> io::Result<SecHandle> {
    sweep_once();
    let server = cert.is_some();
    let direction = if server { SECPKG_CRED_INBOUND } else { SECPKG_CRED_OUTBOUND };
    let cert_count = u32::from(server);
    let certs = cert.as_ref().map_or(null(), |p| p as *const _);
    let package = wide("Microsoft Unified Security Protocol Provider");
    let mut cred = SecHandle::default();
    let mut expiry = 0i64;
    let mut flags = SCH_USE_STRONG_CRYPTO;
    if !server { flags |= SCH_CRED_MANUAL_CRED_VALIDATION | SCH_CRED_NO_DEFAULT_CREDS; }
    if strict && server { flags |= 0x80; } // SCH_CRED_DISABLE_RECONNECTS

    // TLS_PARAMETERS describes exclusions. Remove finite-field DH and RSA
    // key exchange, and CBC AES; TLS 1.3 suites and the final negotiated
    // cipher are checked independently. No machine policy changes.
    let names = ["RSA", "DH", "AES", "3DES", "DES", "RC4", "NULL"].map(wide);
    let cbc = wide("ChainingModeCBC");
    let mode = UnicodeString::from_wide(&cbc);
    let disabled: Vec<_> = names.iter().enumerate().map(|(i, name)| CryptoSettings {
        usage: if i < 2 { 0 } else { 2 },
        algorithm: UnicodeString::from_wide(name),
        mode_count: u32::from(i == 2),
        modes: if i == 2 { &mode } else { null() },
        min_bits: 0, max_bits: 0,
    }).collect();
    let mut tls = TlsParameters {
        alpn_count: 0, alpn_ids: null_mut(), disabled_protocols: !protocols,
        disabled_crypto_count: if strict { disabled.len() as u32 } else { 0 },
        disabled_crypto: if strict { disabled.as_ptr() as *mut c_void } else { null_mut() },
        flags: 0,
    };
    let mut creds = SchCredentials {
        version: SCH_CREDENTIALS_VERSION, cred_format: 0, cert_count, certs, root_store: null_mut(),
        mapper_count: 0, mappers: null_mut(), session_lifespan: 0, flags, tls_parameter_count: 1, tls_parameters: &mut tls,
    };
    let mut st = unsafe {
        AcquireCredentialsHandleW(null(), package.as_ptr(), direction, null_mut(), &mut creds as *mut _ as *mut c_void,
            null_mut(), null_mut(), &mut cred, &mut expiry)
    };
    if st != SEC_E_OK && !strict {
        let mut legacy = SchannelCred {
            version: SCHANNEL_CRED_VERSION, cert_count, certs, root_store: null_mut(), mapper_count: 0,
            mappers: null_mut(), alg_count: 0, algs: null_mut(),
            enabled_protocols: if server { SP_PROT_TLS1_2_SERVER } else { SP_PROT_TLS1_2_CLIENT },
            min_cipher_strength: 0, max_cipher_strength: 0, session_lifespan: 0, flags, cred_format: 0,
        };
        st = unsafe {
            AcquireCredentialsHandleW(null(), package.as_ptr(), direction, null_mut(), &mut legacy as *mut _ as *mut c_void,
                null_mut(), null_mut(), &mut cred, &mut expiry)
        };
    }
    if st != SEC_E_OK { return Err(status_error("AcquireCredentialsHandle", st)); }
    Ok(cred)
}

#[repr(C)]
struct CertContext {
    encoding: u32,
    encoded: *const u8,
    encoded_len: u32,
    info: *mut c_void,
    store: *mut c_void,
}

impl NativeEngine {
    pub(crate) fn peer_chain_der(&mut self) -> io::Result<Vec<Vec<u8>>> {
        let mut cert: *const c_void = null();
        let status = unsafe { QueryContextAttributesW(&mut self.ctx, SECPKG_ATTR_REMOTE_CERT_CONTEXT, &mut cert as *mut _ as *mut c_void) };
        if status != SEC_E_OK || cert.is_null() { return Err(status_error("server certificate", status)); }
        let chain = unsafe {
            let leaf = &*(cert as *const CertContext);
            let mut chain = vec![std::slice::from_raw_parts(leaf.encoded, leaf.encoded_len as usize).to_vec()];
            // The remote context's store holds the certificates sent by the
            // peer. Enumeration releases each previous certificate context.
            if !leaf.store.is_null() {
                let mut next = null();
                loop {
                    next = CertEnumCertificatesInStore(leaf.store, next);
                    if next.is_null() { break; }
                    let c = &*(next as *const CertContext);
                    let der = std::slice::from_raw_parts(c.encoded, c.encoded_len as usize).to_vec();
                    if !chain.contains(&der) { chain.push(der); }
                }
            }
            CertFreeCertificateContext(cert);
            chain
        };
        Ok(chain)
    }
}

enum Side {
    Client(ClientSide),
    Server(ServerConfig),
}

#[repr(C)]
struct CipherInfo {
    version: u32, protocol: u32, suite: u32, base_suite: u32,
    suite_name: [u16; 64], cipher: [u16; 64], cipher_len: u32, block_len: u32,
    hash: [u16; 64], hash_len: u32, exchange: [u16; 64], min_exchange: u32, max_exchange: u32,
    certificate: [u16; 64], key_type: u32,
}

pub(crate) struct NativeEngine {
    side: Side,
    strict: bool,
    ctx: SecHandle,
    have_ctx: bool,
    declined_client_cert: bool,
    post_handshake: bool,
    #[cfg(test)]
    post_handshake_messages: usize,
    tls13: bool,
    sizes: StreamSizes,
    incoming: Vec<u8>,
    pos: usize,
    output: Vec<u8>,
}

unsafe impl Send for NativeEngine {}

impl Drop for NativeEngine {
    fn drop(&mut self) {
        if self.have_ctx { unsafe { DeleteSecurityContext(&mut self.ctx) }; }
    }
}

fn buffer(kind: u32, bytes: &mut [u8]) -> SecBuffer {
    SecBuffer { cb: bytes.len() as u32, kind, pv: bytes.as_mut_ptr().cast() }
}
fn empty(kind: u32) -> SecBuffer { SecBuffer { cb: 0, kind, pv: null_mut() } }
fn descriptor(bufs: &mut [SecBuffer]) -> SecBufferDesc {
    SecBufferDesc { version: 0, count: bufs.len() as u32, buffers: bufs.as_mut_ptr() }
}

impl NativeEngine {
    fn new(side: Side, strict: bool) -> Self {
        Self { side, strict, ctx: SecHandle::default(), have_ctx: false, declined_client_cert: false,
            #[cfg(test)]
            post_handshake_messages: 0,
            post_handshake: false, tls13: false, sizes: StreamSizes::default(), incoming: Vec::new(), pos: 0, output: Vec::new() }
    }

    fn client(host: Option<&str>, resume: Option<&Self>, strict: bool) -> io::Result<Self> {
        let cred = match resume.map(|e| &e.side) {
            Some(Side::Client(side)) => side.cred.clone(),
            Some(Side::Server(_)) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected a client session")),
            None => Arc::new(ClientCred(credentials(None, strict, SP_PROT_TLS1_2 | SP_PROT_TLS1_3)?)),
        };
        Ok(Self::new(Side::Client(ClientSide { cred, target: host.map(wide).unwrap_or_default() }), strict))
    }

    pub(crate) fn strict_client() -> io::Result<Self> { Self::client(None, None, true) }

    pub(crate) fn strict_server(id: &TlsIdentity) -> io::Result<Self> {
        let config = ServerConfig::create(&id.cert_der, &id.key, true, SP_PROT_TLS1_2 | SP_PROT_TLS1_3)?;
        Ok(Self::new(Side::Server(config), true))
    }

    #[cfg(test)]
    pub(crate) fn tls12_server(id: &TlsIdentity) -> io::Result<Self> {
        let config = ServerConfig::create(&id.cert_der, &id.key, true, SP_PROT_TLS1_2)?;
        Ok(Self::new(Side::Server(config), true))
    }

    pub(crate) fn is_tls12(&self) -> io::Result<bool> {
        Ok(!self.tls13)
    }

    fn query<T>(&mut self, attribute: u32, output: &mut T) -> io::Result<()> {
        let st = unsafe { QueryContextAttributesW(&mut self.ctx, attribute, (output as *mut T).cast()) };
        if st == SEC_E_OK { Ok(()) } else { Err(status_error("QueryContextAttributes", st)) }
    }

    fn resumed(&mut self) -> io::Result<bool> {
        let mut session = SessionInfo { flags: 0, session_id_len: 0, session_id: [0; 32] };
        self.query(SECPKG_ATTR_SESSION_INFO, &mut session)?;
        Ok(session.flags & SSL_SESSION_RECONNECT != 0)
    }

    fn established(&mut self) -> io::Result<()> {
        let mut sizes = StreamSizes::default();
        self.query(SECPKG_ATTR_STREAM_SIZES, &mut sizes)?;
        self.sizes = sizes;
        // SecPkgContext_ConnectionInfo consists of seven DWORDs. Its first
        // field is the role-specific SP_PROT flag for the negotiated version.
        let mut connection_info = [0u32; 7];
        self.query(0x5a, &mut connection_info)?;
        self.tls13 = match connection_info[0] {
            SP_PROT_TLS1_2_CLIENT | SP_PROT_TLS1_2_SERVER => false,
            0x1000 | 0x2000 => true, // SP_PROT_TLS1_3_SERVER / CLIENT
            other => return Err(io::Error::other(format!("unexpected Schannel protocol {other:#x}"))),
        };
        if self.strict {
            if self.resumed()? { return Err(io::Error::other("unexpected TLS resumption")); }
            let mut cipher: CipherInfo = unsafe { std::mem::zeroed() };
            self.query(0x64, &mut cipher)?; // SECPKG_ATTR_CIPHER_INFO, version 1 (zero)
            let modern = if self.tls13 {
                matches!(cipher.suite, 0x1301..=0x1303)
            } else {
                matches!(cipher.suite, 0xc02b | 0xc02c | 0xc02f | 0xc030 | 0xcca8 | 0xcca9)
            };
            if !modern { return Err(io::Error::other(format!("unexpected TLS cipher suite {:04x}", cipher.suite))); }
        }
        Ok(())
    }

    /// One SSPI transition. Input may be empty for the initial client token
    /// or when DecryptMessage asks SSPI to process a post-handshake message.
    fn transition(&mut self, bytes: &mut [u8], initial_client: bool) -> (i32, usize) {
        let mut input = [buffer(SECBUFFER_TOKEN, bytes), empty(SECBUFFER_EMPTY)];
        let mut output = [empty(SECBUFFER_TOKEN)];
        let mut in_desc = descriptor(&mut input);
        let mut out_desc = descriptor(&mut output);
        let mut attrs = 0;
        let mut new_ctx = SecHandle::default();
        let ctx = if self.have_ctx { &mut self.ctx } else { null_mut() };
        let next = if self.have_ctx { null_mut() } else { &mut new_ctx };
        let st = unsafe {
            match &self.side {
                Side::Client(side) => {
                    let mut cred = side.cred.0;
                    let target = if side.target.is_empty() { null() } else { side.target.as_ptr() };
                    InitializeSecurityContextW(&mut cred, ctx, target, ISC_FLAGS, 0, 0,
                        if initial_client { null_mut() } else { &mut in_desc }, 0, next, &mut out_desc, &mut attrs, null_mut())
                }
                Side::Server(config) => {
                    let mut cred = config.0.cred;
                    AcceptSecurityContext(&mut cred, ctx, &mut in_desc, ASC_FLAGS, SECURITY_NATIVE_DREP, next, &mut out_desc, &mut attrs, null_mut())
                }
            }
        };
        if !self.have_ctx && matches!(st, SEC_E_OK | SEC_I_CONTINUE_NEEDED | SEC_I_INCOMPLETE_CREDENTIALS) {
            self.ctx = new_ctx;
            self.have_ctx = true;
        }
        if !output[0].pv.is_null() {
            unsafe {
                self.output.extend_from_slice(std::slice::from_raw_parts(output[0].pv.cast::<u8>(), output[0].cb as usize));
                FreeContextBuffer(output[0].pv);
            }
        }
        let extra = if input[1].kind == SECBUFFER_EXTRA { input[1].cb as usize } else { 0 };
        (st, extra)
    }

    fn advance(&mut self, mut allow_empty: bool) -> io::Result<bool> {
        loop {
            let initial_client = matches!(self.side, Side::Client(_)) && !self.have_ctx;
            let len = (self.incoming.len() - self.pos).min(u32::MAX as usize);
            if len == 0 && !initial_client && !allow_empty { return Ok(false); }
            allow_empty = false;
            let mut incoming = std::mem::take(&mut self.incoming);
            let (st, extra) = self.transition(&mut incoming[self.pos..self.pos + len], initial_client);
            self.incoming = incoming;
            match st {
                SEC_E_INCOMPLETE_MESSAGE => return Ok(false),
                SEC_I_INCOMPLETE_CREDENTIALS if !self.declined_client_cert => {
                    self.declined_client_cert = true;
                    allow_empty = true;
                }
                SEC_E_OK | SEC_I_CONTINUE_NEEDED => {
                    self.pos += len.checked_sub(extra).ok_or_else(|| io::Error::other("invalid Schannel input count"))?;
                    if st == SEC_E_OK {
                        self.established()?;
                        self.post_handshake = false;
                        return Ok(true);
                    }
                    if extra == len && len != 0 { return Ok(false); }
                }
                _ => return Err(status_error("TLS handshake", st)),
            }
        }
    }

    pub(crate) fn handshake(&mut self) -> io::Result<bool> { self.advance(false) }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.pos >= self.incoming.len() - self.pos && self.pos != 0 {
            self.incoming.drain(..self.pos);
            self.pos = 0;
        }
        self.incoming.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn take(&mut self, output: &mut Vec<u8>) -> io::Result<()> {
        if output.is_empty() { std::mem::swap(output, &mut self.output); }
        else { output.append(&mut self.output); }
        Ok(())
    }

    pub(crate) fn peer_leaf_der(&mut self) -> io::Result<Vec<u8>> {
        let mut cert: *const CertContext = null();
        self.query(SECPKG_ATTR_REMOTE_CERT_CONTEXT, &mut cert)?;
        if cert.is_null() { return Err(io::Error::other("server sent no certificate")); }
        unsafe {
            let der = std::slice::from_raw_parts((*cert).encoded, (*cert).encoded_len as usize).to_vec();
            CertFreeCertificateContext(cert.cast());
            Ok(der)
        }
    }

    pub(crate) fn read_plain(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.post_handshake && !self.advance(false)? { return Err(io::ErrorKind::WouldBlock.into()); }
            let input = &mut self.incoming[self.pos..];
            if input.len() < 5 { return Err(io::ErrorKind::WouldBlock.into()); }
            let len = 5 + u16::from_be_bytes([input[3], input[4]]) as usize;
            if input.len() < len { return Err(io::ErrorKind::WouldBlock.into()); }
            // Submit exactly one complete TLS record. Keep any following
            // records separate from SSPI's modified post-handshake token.
            let mut bufs = [buffer(SECBUFFER_DATA, &mut input[..len]), empty(SECBUFFER_EMPTY), empty(SECBUFFER_EMPTY), empty(SECBUFFER_EMPTY)];
            let st = unsafe { DecryptMessage(&mut self.ctx, &mut descriptor(&mut bufs), 0, null_mut()) };
            match st {
                SEC_I_CONTEXT_EXPIRED => return Ok(0),
                SEC_I_RENEGOTIATE => {
                    #[cfg(test)]
                    { self.post_handshake_messages += 1; }
                    let extra = bufs.iter().find(|b| b.kind == SECBUFFER_EXTRA).map_or(0, |b| b.cb as usize);
                    self.pos += len.checked_sub(extra).ok_or_else(|| io::Error::other("invalid Schannel token"))?;
                    self.post_handshake = true;
                    if !self.advance(true)? { return Err(io::ErrorKind::WouldBlock.into()); }
                }
                SEC_E_OK => {
                    self.pos += len;
                    if let Some(b) = bufs.iter().find(|b| b.kind == SECBUFFER_DATA && b.cb != 0) {
                        let n = b.cb as usize;
                        if n > out.len() || b.pv.is_null() { return Err(io::Error::other("invalid Schannel plaintext size")); }
                        unsafe { std::ptr::copy_nonoverlapping(b.pv.cast::<u8>(), out.as_mut_ptr(), n); }
                        return Ok(n);
                    }
                }
                _ => return Err(status_error("DecryptMessage", st)),
            }
        }
    }

    pub(crate) fn write_plain(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.post_handshake { return Err(io::ErrorKind::WouldBlock.into()); }
        let n = data.len().min(self.sizes.max_message as usize);
        let header = self.sizes.header as usize;
        let trailer = self.sizes.trailer as usize;
        let mut msg = vec![0u8; header + n + trailer];
        msg[header..header + n].copy_from_slice(&data[..n]);
        let base = msg.as_mut_ptr();
        let mut bufs = [
            SecBuffer { cb: header as u32, kind: SECBUFFER_STREAM_HEADER, pv: base as *mut c_void },
            SecBuffer { cb: n as u32, kind: SECBUFFER_DATA, pv: unsafe { base.add(header) } as *mut c_void },
            SecBuffer { cb: trailer as u32, kind: SECBUFFER_STREAM_TRAILER, pv: unsafe { base.add(header + n) } as *mut c_void },
            SecBuffer { cb: 0, kind: SECBUFFER_EMPTY, pv: null_mut() },
        ];
        let mut desc = SecBufferDesc { version: 0, count: 4, buffers: bufs.as_mut_ptr() };
        let st = unsafe { EncryptMessage(&mut self.ctx, 0, &mut desc, 0) };
        if st != SEC_E_OK {
            return Err(status_error("EncryptMessage", st));
        }
        for b in &bufs[..3] {
            if b.cb != 0 {
                unsafe { self.output.extend_from_slice(std::slice::from_raw_parts(b.pv.cast::<u8>(), b.cb as usize)); }
            }
        }
        Ok(n)
    }

    pub(crate) fn close(&mut self) {
        // Tell SSPI to emit close_notify; a failed/expired context can only
        // be closed at the transport.
        extern "system" { fn ApplyControlToken(ctx: *mut SecHandle, input: *mut SecBufferDesc) -> i32; }
        let mut shutdown = 1u32.to_ne_bytes(); // SCHANNEL_SHUTDOWN
        let mut token = [buffer(SECBUFFER_TOKEN, &mut shutdown)];
        if self.have_ctx && unsafe { ApplyControlToken(&mut self.ctx, &mut descriptor(&mut token)) } == SEC_E_OK {
            self.transition(&mut [], false);
        }
    }
}

pub(crate) fn connect_capture(tcp: TcpStream, host: &str, resume: Option<&SocketPump>) -> io::Result<(SocketPump, Vec<u8>)> {
    let native = NativeEngine::client(Some(host), resume.map(|s| &s.engine.native), false)?;
    let mut stream = SocketPump::handshake(tcp, TlsEngine::start(native, None, true, None)?, None)?;
    if resume.is_some() && !stream.engine.native.resumed()? {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "the TLS session was not resumed"));
    }
    let leaf = stream.engine.server_certificate_der().unwrap().to_vec();
    Ok((stream, leaf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::io::{Read, Write};
    use std::time::Duration;

    /// Whether this account's key store holds a key by this (wide) name.
    fn stored(name: &[u16]) -> bool {
        unsafe {
            let mut provider = 0usize;
            assert_eq!(NCryptOpenStorageProvider(&mut provider, wide(KEY_PROVIDER).as_ptr(), 0), 0, "the software key store opens");
            let mut key = 0usize;
            let found = NCryptOpenKey(provider, &mut key, name.as_ptr(), 0, 0) == 0;
            if found {
                NCryptFreeObject(key);
            }
            NCryptFreeObject(provider);
            found
        }
    }

    #[test]
    fn tls_strict_server_sends_no_tickets_and_refuses_reconnect() {
        let id = crate::tls::engine::tests::identity();
        let config = ServerConfig::create(&id.cert_der, &id.key, true, SP_PROT_TLS1_2 | SP_PROT_TLS1_3).unwrap();
        // A legacy client can cache and offer sessions by credential and target.
        // Reuse both, and even the server credential, across the two handshakes.
        let target = std::net::Ipv4Addr::LOCALHOST.to_string();
        let mut previous: Option<TlsEngine> = None;
        for _ in 0..2 {
            let native = NativeEngine::client(Some(&target), previous.as_ref().map(|s| &s.native), false).unwrap();
            let mut client = TlsEngine::start(native, None, true, None).unwrap();
            let native = NativeEngine::new(Side::Server(config.clone()), true);
            let mut server = TlsEngine::start(native, None, false, None).unwrap();
            crate::tls::engine::tests::handshake(&mut client, &mut server);
            assert!(!client.native.resumed().unwrap(), "the client resumed a strict session");
            assert_eq!(client.native.post_handshake_messages, 0, "strict server issued a post-handshake ticket");
            previous = Some(client);
        }
    }

    /// Two servers of one identity each store a key of their own and delete only it: dropping one
    /// leaves the other serving, and once both are dropped the account's key store holds neither.
    #[test]
    fn each_server_config_deletes_the_key_it_stored() {
        let identity = crate::tls::TlsIdentity::in_memory(generate_p256().unwrap(), "makepad stored key test", None).unwrap();
        let first = ServerConfig::new(&identity.cert_der, &identity.key).unwrap();
        let second = ServerConfig::new(&identity.cert_der, &identity.key).unwrap();
        let (first_key, second_key) = (first.0._key.0.clone(), second.0._key.0.clone());
        assert_ne!(first_key, second_key, "one identity, a key per config");
        assert!(stored(&first_key) && stored(&second_key), "each config stored its key");
        let mut exited = std::process::Command::new("cmd.exe").args(["/d", "/c", "exit", "0"]).spawn().unwrap();
        assert!(exited.wait().unwrap().success());
        let orphan = container_name(exited.id()).unwrap();
        store_key(&orphan, &identity.key).unwrap();
        assert!(stored(&orphan), "the exited process's container exists before the sweep");
        sweep_orphans().unwrap();
        assert!(!stored(&orphan), "the sweep left an exited process's container");
        assert!(stored(&first_key), "the sweep removed a live process's container");
        drop(first);
        assert!(!stored(&first_key), "a dropped config's key is gone");
        assert!(stored(&second_key), "the other config's key stays");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let (mut stream, _) = crate::SocketStream::connect_capture("127.0.0.1", &port.to_string()).expect("a handshake");
            stream.write_all(b"k").and_then(|()| stream.flush()).unwrap();
            let mut back = [0u8; 1];
            stream.read_exact(&mut back).unwrap();
            back
        });
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut served = second.accept(tcp).expect("the remaining config still serves with its own key");
        let mut byte = [0u8; 1];
        served.read_exact(&mut byte).unwrap();
        served.write_all(&byte).and_then(|()| served.flush()).unwrap();
        assert_eq!(client.join().unwrap(), *b"k", "a byte each way through the handshake");
        drop(served);
        drop(second);
        assert!(!stored(&second_key), "the last config's key is gone too");
    }
}
