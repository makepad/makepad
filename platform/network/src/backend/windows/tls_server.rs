//! TLS server on Schannel (SSPI `AcceptSecurityContext`).
//!
//! Keys: generated and used for signing with BCrypt (ECDSA P-256). For the
//! handshake, Schannel reads the private key from the CNG key store of the
//! account running the server, so on start the key is imported there under
//! a fixed name derived from the certificate (overwritten each start), and
//! the in-memory certificate context points at it. The key file remains
//! the source of truth, so the identity moves between accounts unchanged.
//!
//! Plain Win32 C APIs (SSPI, crypt32, CNG) declared here; no COM.

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::ptr::{null, null_mut};

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
    fn CertSetCertificateContextProperty(cert: *const c_void, prop: u32, flags: u32, data: *const c_void) -> i32;
}

#[link(name = "ncrypt")]
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
}

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(alg: *mut *mut c_void, id: *const u16, imp: *const u16, flags: u32) -> i32;
    fn BCryptCloseAlgorithmProvider(alg: *mut c_void, flags: u32) -> i32;
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
}

const SECPKG_CRED_INBOUND: u32 = 1;
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
const SEC_E_INCOMPLETE_MESSAGE: i32 = 0x8009_0318u32 as i32;
const SECPKG_ATTR_STREAM_SIZES: u32 = 4;

const X509_ASN_ENCODING: u32 = 0x1 | 0x10000;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const NCRYPTBUFFER_PKCS_KEY_NAME: u32 = 45;
const NCRYPT_OVERWRITE_KEY_FLAG: u32 = 0x80;
const ECDSA_PRIVATE_P256_MAGIC: u32 = 0x3253_4345;

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
        unsafe { BCryptCloseAlgorithmProvider(self.0, 0) };
    }
}
struct BKey(*mut c_void);
impl Drop for BKey {
    fn drop(&mut self) {
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

pub struct ServerConfig {
    cred: SecHandle,
    cert: *const c_void,
}

unsafe impl Send for ServerConfig {}
unsafe impl Sync for ServerConfig {}

impl ServerConfig {
    pub fn new(cert_der: &[u8], key: &[u8; 97]) -> io::Result<Self> {
        let fp = crate::tls::fingerprint(cert_der);
        let container = wide(&format!("makepad-tls-{}", crate::tls::to_hex(&fp[..12])));
        let provider_name = wide("Microsoft Software Key Storage Provider");

        // The key, persisted in this account's CNG store where Schannel
        // looks it up through the certificate's key-provider property.
        unsafe {
            let mut provider = 0usize;
            let st = NCryptOpenStorageProvider(&mut provider, provider_name.as_ptr(), 0);
            if st != 0 {
                return Err(status_error("NCryptOpenStorageProvider", st));
            }
            let blob = ecc_private_blob(key);
            let mut name_buffer = NCryptBuffer {
                cb: (container.len() * 2) as u32,
                kind: NCRYPTBUFFER_PKCS_KEY_NAME,
                pv: container.as_ptr() as *mut c_void,
            };
            let params = NCryptBufferDesc { version: 0, count: 1, buffers: &mut name_buffer };
            let mut nkey = 0usize;
            let st = NCryptImportKey(
                provider,
                0,
                wide("ECCPRIVATEBLOB").as_ptr(),
                &params,
                &mut nkey,
                blob.as_ptr(),
                blob.len() as u32,
                NCRYPT_OVERWRITE_KEY_FLAG,
            );
            if nkey != 0 {
                NCryptFreeObject(nkey);
            }
            NCryptFreeObject(provider);
            if st != 0 {
                return Err(status_error("NCryptImportKey", st));
            }
        }

        let cert = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, cert_der.as_ptr(), cert_der.len() as u32) };
        if cert.is_null() {
            return Err(io::Error::last_os_error());
        }
        let info = CryptKeyProvInfo {
            container_name: container.as_ptr(),
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

        let certs = [cert];
        let package = wide("Microsoft Unified Security Protocol Provider");
        let mut cred = SecHandle::default();
        let mut expiry = 0i64;
        // TLS 1.2 and 1.3 (where this Windows has it) through SCH_CREDENTIALS.
        let mut tls = TlsParameters {
            alpn_count: 0,
            alpn_ids: null_mut(),
            disabled_protocols: !(SP_PROT_TLS1_2 | SP_PROT_TLS1_3),
            disabled_crypto_count: 0,
            disabled_crypto: null_mut(),
            flags: 0,
        };
        let mut creds = SchCredentials {
            version: SCH_CREDENTIALS_VERSION,
            cred_format: 0,
            cert_count: 1,
            certs: certs.as_ptr(),
            root_store: null_mut(),
            mapper_count: 0,
            mappers: null_mut(),
            session_lifespan: 0,
            flags: SCH_USE_STRONG_CRYPTO,
            tls_parameter_count: 1,
            tls_parameters: &mut tls,
        };
        let mut st = unsafe {
            AcquireCredentialsHandleW(
                null(),
                package.as_ptr(),
                SECPKG_CRED_INBOUND,
                null_mut(),
                &mut creds as *mut _ as *mut c_void,
                null_mut(),
                null_mut(),
                &mut cred,
                &mut expiry,
            )
        };
        if st != SEC_E_OK {
            // Windows before 10 1809 knows only SCHANNEL_CRED: TLS 1.2.
            let mut legacy = SchannelCred {
                version: SCHANNEL_CRED_VERSION,
                cert_count: 1,
                certs: certs.as_ptr(),
                root_store: null_mut(),
                mapper_count: 0,
                mappers: null_mut(),
                alg_count: 0,
                algs: null_mut(),
                enabled_protocols: SP_PROT_TLS1_2_SERVER,
                min_cipher_strength: 0,
                max_cipher_strength: 0,
                session_lifespan: 0,
                flags: SCH_USE_STRONG_CRYPTO,
                cred_format: 0,
            };
            st = unsafe {
                AcquireCredentialsHandleW(
                    null(),
                    package.as_ptr(),
                    SECPKG_CRED_INBOUND,
                    null_mut(),
                    &mut legacy as *mut _ as *mut c_void,
                    null_mut(),
                    null_mut(),
                    &mut cred,
                    &mut expiry,
                )
            };
        }
        if st != SEC_E_OK {
            unsafe { CertFreeCertificateContext(cert) };
            return Err(status_error("AcquireCredentialsHandle", st));
        }
        Ok(Self { cred, cert })
    }

    pub fn accept(&self, mut tcp: TcpStream) -> io::Result<ServerStream> {
        let mut cred = self.cred;
        let mut ctx = SecHandle::default();
        let mut have_ctx = false;
        let mut incoming: Vec<u8> = Vec::new();
        let mut need_read = true;
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if need_read {
                let n = tcp.read(&mut buf)?;
                if n == 0 {
                    if have_ctx {
                        unsafe { DeleteSecurityContext(&mut ctx) };
                    }
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed during TLS handshake"));
                }
                incoming.extend_from_slice(&buf[..n]);
            }
            let mut in_bufs = [
                SecBuffer { cb: incoming.len() as u32, kind: SECBUFFER_TOKEN, pv: incoming.as_mut_ptr() as *mut c_void },
                SecBuffer { cb: 0, kind: SECBUFFER_EMPTY, pv: null_mut() },
            ];
            let mut out_bufs = [SecBuffer { cb: 0, kind: SECBUFFER_TOKEN, pv: null_mut() }];
            let mut in_desc = SecBufferDesc { version: 0, count: 2, buffers: in_bufs.as_mut_ptr() };
            let mut out_desc = SecBufferDesc { version: 0, count: 1, buffers: out_bufs.as_mut_ptr() };
            let mut attrs = 0u32;
            let mut new_ctx = SecHandle::default();
            let st = unsafe {
                AcceptSecurityContext(
                    &mut cred,
                    if have_ctx { &mut ctx } else { null_mut() },
                    &mut in_desc,
                    ASC_FLAGS,
                    SECURITY_NATIVE_DREP,
                    if have_ctx { null_mut() } else { &mut new_ctx },
                    &mut out_desc,
                    &mut attrs,
                    null_mut(),
                )
            };
            if !have_ctx && (st == SEC_E_OK || st == SEC_I_CONTINUE_NEEDED) {
                ctx = new_ctx;
                have_ctx = true;
            }
            if out_bufs[0].cb > 0 && !out_bufs[0].pv.is_null() {
                let token = unsafe { std::slice::from_raw_parts(out_bufs[0].pv as *const u8, out_bufs[0].cb as usize) };
                let sent = tcp.write_all(token);
                unsafe { FreeContextBuffer(out_bufs[0].pv) };
                sent?;
            }
            match st {
                SEC_E_INCOMPLETE_MESSAGE => need_read = true,
                SEC_E_OK | SEC_I_CONTINUE_NEEDED => {
                    if in_bufs[1].kind == SECBUFFER_EXTRA && in_bufs[1].cb > 0 {
                        let keep = in_bufs[1].cb as usize;
                        incoming.drain(..incoming.len() - keep);
                    } else {
                        incoming.clear();
                    }
                    if st == SEC_E_OK {
                        break;
                    }
                    need_read = incoming.is_empty();
                }
                _ => {
                    if have_ctx {
                        unsafe { DeleteSecurityContext(&mut ctx) };
                    }
                    return Err(status_error("TLS handshake (AcceptSecurityContext)", st));
                }
            }
        }
        let mut sizes = StreamSizes::default();
        let st = unsafe { QueryContextAttributesW(&mut ctx, SECPKG_ATTR_STREAM_SIZES, &mut sizes as *mut _ as *mut c_void) };
        if st != SEC_E_OK {
            unsafe { DeleteSecurityContext(&mut ctx) };
            return Err(status_error("QueryContextAttributes(STREAM_SIZES)", st));
        }
        Ok(ServerStream { tcp, cred, ctx, sizes, incoming, plain: Vec::new(), plain_pos: 0, closed: false })
    }
}

impl Drop for ServerConfig {
    fn drop(&mut self) {
        unsafe {
            FreeCredentialsHandle(&mut self.cred);
            CertFreeCertificateContext(self.cert);
        }
    }
}

pub struct ServerStream {
    tcp: TcpStream,
    cred: SecHandle,
    ctx: SecHandle,
    sizes: StreamSizes,
    incoming: Vec<u8>,
    plain: Vec<u8>,
    plain_pos: usize,
    closed: bool,
}

unsafe impl Send for ServerStream {}

impl ServerStream {
    pub fn tcp(&self) -> &TcpStream {
        &self.tcp
    }

    pub fn shutdown(&mut self) {
        self.closed = true;
        let _ = self.tcp.shutdown(Shutdown::Both);
    }

    /// One decrypt attempt over `incoming`. Ok(true) when progress was made.
    fn decrypt_some(&mut self) -> io::Result<bool> {
        if self.incoming.is_empty() {
            return Ok(false);
        }
        let mut bufs = [
            SecBuffer { cb: self.incoming.len() as u32, kind: SECBUFFER_DATA, pv: self.incoming.as_mut_ptr() as *mut c_void },
            SecBuffer { cb: 0, kind: SECBUFFER_EMPTY, pv: null_mut() },
            SecBuffer { cb: 0, kind: SECBUFFER_EMPTY, pv: null_mut() },
            SecBuffer { cb: 0, kind: SECBUFFER_EMPTY, pv: null_mut() },
        ];
        let mut desc = SecBufferDesc { version: 0, count: 4, buffers: bufs.as_mut_ptr() };
        let st = unsafe { DecryptMessage(&mut self.ctx, &mut desc, 0, null_mut()) };
        match st {
            SEC_E_INCOMPLETE_MESSAGE => Ok(false),
            SEC_I_CONTEXT_EXPIRED => {
                self.closed = true;
                self.incoming.clear();
                Ok(true)
            }
            SEC_E_OK | SEC_I_RENEGOTIATE => {
                let mut extra: Option<Vec<u8>> = None;
                for b in &bufs {
                    if b.kind == SECBUFFER_DATA && b.cb > 0 {
                        let data = unsafe { std::slice::from_raw_parts(b.pv as *const u8, b.cb as usize) };
                        self.plain.extend_from_slice(data);
                    } else if b.kind == SECBUFFER_EXTRA && b.cb > 0 {
                        let start = self.incoming.len() - b.cb as usize;
                        extra = Some(self.incoming[start..].to_vec());
                    }
                }
                self.incoming = extra.unwrap_or_default();
                if st == SEC_I_RENEGOTIATE {
                    // TLS 1.3 post-handshake message (e.g. key update):
                    // hand it back to the handshake engine.
                    self.post_handshake()?;
                }
                Ok(true)
            }
            _ => Err(status_error("DecryptMessage", st)),
        }
    }

    fn post_handshake(&mut self) -> io::Result<()> {
        let mut in_bufs = [
            SecBuffer { cb: self.incoming.len() as u32, kind: SECBUFFER_TOKEN, pv: self.incoming.as_mut_ptr() as *mut c_void },
            SecBuffer { cb: 0, kind: SECBUFFER_EMPTY, pv: null_mut() },
        ];
        let mut out_bufs = [SecBuffer { cb: 0, kind: SECBUFFER_TOKEN, pv: null_mut() }];
        let mut in_desc = SecBufferDesc { version: 0, count: 2, buffers: in_bufs.as_mut_ptr() };
        let mut out_desc = SecBufferDesc { version: 0, count: 1, buffers: out_bufs.as_mut_ptr() };
        let mut attrs = 0u32;
        let st = unsafe {
            AcceptSecurityContext(
                &mut self.cred,
                &mut self.ctx,
                &mut in_desc,
                ASC_FLAGS,
                SECURITY_NATIVE_DREP,
                null_mut(),
                &mut out_desc,
                &mut attrs,
                null_mut(),
            )
        };
        if out_bufs[0].cb > 0 && !out_bufs[0].pv.is_null() {
            let token = unsafe { std::slice::from_raw_parts(out_bufs[0].pv as *const u8, out_bufs[0].cb as usize) };
            let sent = self.tcp.write_all(token);
            unsafe { FreeContextBuffer(out_bufs[0].pv) };
            sent?;
        }
        if st != SEC_E_OK && st != SEC_I_CONTINUE_NEEDED && st != SEC_E_INCOMPLETE_MESSAGE {
            return Err(status_error("TLS post-handshake", st));
        }
        if in_bufs[1].kind == SECBUFFER_EXTRA && in_bufs[1].cb > 0 {
            let keep = in_bufs[1].cb as usize;
            let len = self.incoming.len();
            self.incoming.drain(..len - keep);
        } else if st != SEC_E_INCOMPLETE_MESSAGE {
            self.incoming.clear();
        }
        Ok(())
    }
}

impl Drop for ServerStream {
    fn drop(&mut self) {
        unsafe { DeleteSecurityContext(&mut self.ctx) };
    }
}

impl Read for ServerStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.plain_pos < self.plain.len() {
                let n = out.len().min(self.plain.len() - self.plain_pos);
                out[..n].copy_from_slice(&self.plain[self.plain_pos..self.plain_pos + n]);
                self.plain_pos += n;
                if self.plain_pos == self.plain.len() {
                    self.plain.clear();
                    self.plain_pos = 0;
                }
                return Ok(n);
            }
            if self.closed {
                return Ok(0);
            }
            if self.decrypt_some()? {
                continue;
            }
            let mut buf = [0u8; 16 * 1024];
            let n = self.tcp.read(&mut buf)?;
            if n == 0 {
                self.closed = true;
                return Ok(0);
            }
            self.incoming.extend_from_slice(&buf[..n]);
        }
    }
}

impl Write for ServerStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
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
        let total = (bufs[0].cb + bufs[1].cb + bufs[2].cb) as usize;
        self.tcp.write_all(&msg[..total])?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}
