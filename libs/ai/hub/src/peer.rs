//! Peer-assisted model-cache distribution: shared primitives.
//!
//! The fleet's coordinator (Asset Server) selects which box supplies a model
//! artifact and hands the receiver a short-lived ticket; the bytes then flow
//! DIRECTLY source box -> receiver box over the boxes' one existing service
//! port — never through the coordinator, and never through a second
//! daemon/sidecar (hard deployment invariant: one service process per PC).
//!
//! This module holds the pieces both sides share:
//! - [`TransferSecret`]: the HMAC key tickets are signed with. Loaded from
//!   `MAKEPAD_AI_PEER_SECRET` or the cache-dir `peer-secret` file; without a
//!   secret the box serves nothing (fail closed). The secret and every ticket
//!   signature are redacted from all Debug/log output.
//! - [`PeerTicket`]: `mtk1.<expiry>.<source>.<receiver>.<digest>.<hmac>` —
//!   auth data carried in headers (never URLs), scoped to one source node,
//!   one receiver node and one exact artifact digest, with a bounded expiry.
//! - Inventory: the serve allow-list. ONLY fetchable source artifacts whose
//!   SHA-256 and size are pinned AND whose on-disk verification receipt is
//!   currently valid are addressable — by digest, never by path. Structured
//!   conversion outputs stay private until the receiver has an install path
//!   for them; advertising bytes it cannot consume would produce bad plans.
//! - [`ServeLeases`]: refcounts on cache paths with an in-flight serve, so
//!   eviction/replacement paths refuse to delete or rename over a file that
//!   is being read by a peer transfer.
//! - [`PeerPlan`]: the receiver-side source list + ticket set for one job
//!   (request fields from the coordinator first, `MAKEPAD_AI_PEER_SOURCES`
//!   env injection for immediate deployments; selection stays central).

use crate::error::AssetAiError;
use crate::registry::Registry;
use crate::sha256::{to_hex, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default ticket lifetime for self-minted (fleet-shared-secret) tickets.
pub const TICKET_TTL_SECS: u64 = 300;
/// Verification-side cap: a ticket claiming to live longer than this is
/// refused outright, bounding the damage of an over-generous mint.
pub const TICKET_MAX_TTL_SECS: u64 = 24 * 60 * 60;
/// Minimum usable secret length; anything shorter is treated as absent.
pub const SECRET_MIN_BYTES: usize = 16;

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// HMAC-SHA256 (self-contained, same reasoning as src/sha256.rs)
// ---------------------------------------------------------------------------

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let mut hasher = Sha256::new();
        hasher.update(key);
        key_block[..32].copy_from_slice(&hasher.finish());
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    let mut pad = [0u8; 64];
    for (out, byte) in pad.iter_mut().zip(key_block.iter()) {
        *out = byte ^ 0x36;
    }
    inner.update(&pad);
    inner.update(message);
    let inner_hash = inner.finish();
    let mut outer = Sha256::new();
    for (out, byte) in pad.iter_mut().zip(key_block.iter()) {
        *out = byte ^ 0x5c;
    }
    outer.update(&pad);
    outer.update(&inner_hash);
    outer.finish()
}

/// Constant-time byte-string equality (length leak only).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Transfer secret
// ---------------------------------------------------------------------------

/// HMAC key for ticket signing/verification. Deliberately opaque: Debug and
/// Display never reveal the bytes, and there is no accessor that returns the
/// raw secret to callers outside this module's signing functions.
#[derive(Clone, PartialEq, Eq)]
pub struct TransferSecret(Vec<u8>);

impl TransferSecret {
    /// `None` when the material is too short to be a usable key.
    pub fn new(bytes: &[u8]) -> Option<Self> {
        let start = bytes.iter().position(|b| !b.is_ascii_whitespace())?;
        let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace())? + 1;
        let trimmed = &bytes[start..end];
        if trimmed.len() < SECRET_MIN_BYTES {
            return None;
        }
        Some(Self(trimmed.to_vec()))
    }

    /// Resolution order: explicit override > `MAKEPAD_AI_PEER_SECRET` env >
    /// `<cache_dir>/peer-secret` file. Whitespace is trimmed. Anything under
    /// [`SECRET_MIN_BYTES`] is rejected with a warning (fail closed).
    pub fn resolve(explicit: Option<&str>, cache_dir: &Path) -> Option<Self> {
        if let Some(text) = explicit {
            return Self::checked(text.as_bytes(), "ServiceConfig peer secret");
        }
        if let Ok(text) = std::env::var("MAKEPAD_AI_PEER_SECRET") {
            return Self::checked(text.as_bytes(), "MAKEPAD_AI_PEER_SECRET");
        }
        let path = cache_dir.join("peer-secret");
        if let Ok(bytes) = std::fs::read(&path) {
            return Self::checked(&bytes, "peer-secret file");
        }
        None
    }

    fn checked(bytes: &[u8], origin: &str) -> Option<Self> {
        let secret = Self::new(bytes);
        if secret.is_none() {
            eprintln!(
                "peer: {origin} is shorter than {SECRET_MIN_BYTES} bytes — ignored (peer lane disabled)"
            );
        }
        secret
    }

    fn sign(&self, message: &[u8]) -> [u8; 32] {
        hmac_sha256(&self.0, message)
    }
}

impl std::fmt::Debug for TransferSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TransferSecret(len={}, redacted)", self.0.len())
    }
}

// ---------------------------------------------------------------------------
// Tickets
// ---------------------------------------------------------------------------

/// One parsed transfer ticket. The signature is never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct PeerTicket {
    pub expires_unix: u64,
    /// node_key of the box allowed to SERVE under this ticket.
    pub source_key: String,
    /// node_key of the box allowed to FETCH under this ticket.
    pub receiver_key: String,
    /// Exact artifact SHA-256 (lowercase hex) this ticket covers.
    pub digest: String,
    sig: String,
}

impl std::fmt::Debug for PeerTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PeerTicket{{exp:{}, source:{}, receiver:{}, digest:{}, sig:redacted}}",
            self.expires_unix, self.source_key, self.receiver_key, self.digest
        )
    }
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl PeerTicket {
    fn signed_prefix(expires_unix: u64, source: &str, receiver: &str, digest: &str) -> String {
        format!("mtk1.{expires_unix}.{source}.{receiver}.{digest}")
    }

    /// Mints a ticket string. The coordinator calls this with the source
    /// box's transfer secret; a box holding the fleet-shared secret can mint
    /// for itself as receiver (env-injected deployments).
    pub fn mint(
        secret: &TransferSecret,
        source_key: &str,
        receiver_key: &str,
        digest: &str,
        expires_unix: u64,
    ) -> String {
        let prefix = Self::signed_prefix(expires_unix, source_key, receiver_key, digest);
        let sig = to_hex(&secret.sign(prefix.as_bytes()));
        format!("{prefix}.{sig}")
    }

    /// Strict structural parse; no signature check yet.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > 512 {
            return None;
        }
        let mut parts = text.split('.');
        if parts.next()? != "mtk1" {
            return None;
        }
        let expires_unix: u64 = parts.next()?.parse().ok()?;
        let source_key = parts.next()?.to_string();
        let receiver_key = parts.next()?.to_string();
        let digest = parts.next()?.to_string();
        let sig = parts.next()?.to_string();
        if parts.next().is_some() {
            return None;
        }
        if !is_hex(&source_key, 32) || !is_hex(&receiver_key, 32) {
            return None;
        }
        if !is_hex(&digest, 64) || !is_hex(&sig, 64) {
            return None;
        }
        Some(Self {
            expires_unix,
            source_key,
            receiver_key,
            digest,
            sig,
        })
    }

    pub fn encode(&self) -> String {
        format!(
            "{}.{}",
            Self::signed_prefix(
                self.expires_unix,
                &self.source_key,
                &self.receiver_key,
                &self.digest
            ),
            self.sig
        )
    }

    /// Full serve-side check: signature, expiry window, and the exact
    /// source/receiver/digest scope. Error strings are safe to log (they
    /// never echo the ticket or signature).
    pub fn verify(
        &self,
        secret: &TransferSecret,
        source_key: &str,
        claimed_receiver: &str,
        digest: &str,
        now_unix: u64,
    ) -> Result<(), &'static str> {
        let prefix = Self::signed_prefix(
            self.expires_unix,
            &self.source_key,
            &self.receiver_key,
            &self.digest,
        );
        let expected = to_hex(&secret.sign(prefix.as_bytes()));
        if !constant_time_eq(expected.as_bytes(), self.sig.as_bytes()) {
            return Err("ticket signature invalid");
        }
        if now_unix > self.expires_unix {
            return Err("ticket expired");
        }
        if self.expires_unix - now_unix > TICKET_MAX_TTL_SECS {
            return Err("ticket lifetime exceeds the allowed maximum");
        }
        if self.source_key != source_key {
            return Err("ticket is scoped to a different source node");
        }
        if self.receiver_key != claimed_receiver {
            return Err("ticket is scoped to a different receiver node");
        }
        if self.digest != digest {
            return Err("ticket is scoped to a different artifact digest");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Inventory: the digest-addressed serve allow-list
// ---------------------------------------------------------------------------

/// One local, verified, digest-addressed artifact.
#[derive(Clone, Debug)]
pub struct BlobEntry {
    /// Lowercase-hex SHA-256 — the ONLY way this artifact is addressable.
    pub digest: String,
    pub size: u64,
    /// Canonical cache-relative path ('/'-separated). Peers install to the
    /// exact same path — no duplicate bytes, no renamed cache files.
    pub cache_as: String,
    /// Currently "source" (downloaded form). Reserved for future fetchable
    /// artifact kinds once receivers have an install path for them.
    pub kind: &'static str,
    /// Model ids that reference this artifact.
    pub models: Vec<String>,
    /// Absolute on-disk path.
    pub path: PathBuf,
}

/// Every fetchable registry source that is digest-pinned AND currently
/// verified on disk (receipt + identity), deduplicated by digest. This is both
/// the serve allow-list and what `/v1/model_inventory` reports to the
/// coordinator. Converted outputs are deliberately omitted (the current
/// receiver installs `FileSpec` sources only), but digest-pinned LOCAL
/// sources are included: the in-house quantized tiers exist nowhere but the
/// fleet, so the peer network is their only distribution channel — `local`
/// gates Hugging Face, not the LAN.
pub fn build_inventory(registry: &Registry, cache_dir: &Path) -> Vec<BlobEntry> {
    let mut by_digest: HashMap<String, BlobEntry> = HashMap::new();
    for model in &registry.models {
        for file in &model.files {
            if let (Some(digest), Some(size)) = (file.sha256.as_deref(), file.size) {
                if crate::download::source_file_is_verified(file, cache_dir) {
                    push_entry(
                        &mut by_digest,
                        digest,
                        size,
                        &file.cache_as,
                        "source",
                        &model.id,
                        file.dest_path(cache_dir),
                    );
                }
            }
        }
    }
    let mut out: Vec<BlobEntry> = by_digest.into_values().collect();
    out.sort_by(|a, b| a.digest.cmp(&b.digest));
    out
}

fn push_entry(
    by_digest: &mut HashMap<String, BlobEntry>,
    digest: &str,
    size: u64,
    cache_as: &str,
    kind: &'static str,
    model_id: &str,
    path: PathBuf,
) {
    let entry = by_digest
        .entry(digest.to_string())
        .or_insert_with(|| BlobEntry {
            digest: digest.to_string(),
            size,
            cache_as: cache_as.to_string(),
            kind,
            models: Vec::new(),
            path,
        });
    if !entry.models.iter().any(|m| m == model_id) {
        entry.models.push(model_id.to_string());
    }
}

/// Serve-time lookup: digest -> verified blob, re-checking the verification
/// receipt NOW (fail closed — a file that stopped verifying stops serving).
pub fn find_verified_blob(registry: &Registry, cache_dir: &Path, digest: &str) -> Option<BlobEntry> {
    if !is_hex(digest, 64) {
        return None;
    }
    for model in &registry.models {
        for file in &model.files {
            // `local` files with a pinned digest are servable like any other
            // source — see `build_inventory`.
            if file.sha256.as_deref() == Some(digest)
                && file.size.is_some()
                && crate::download::source_file_is_verified(file, cache_dir)
            {
                return Some(BlobEntry {
                    digest: digest.to_string(),
                    size: file.size.unwrap(),
                    cache_as: file.cache_as.clone(),
                    kind: "source",
                    models: vec![model.id.clone()],
                    path: file.dest_path(cache_dir),
                });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Serve leases
// ---------------------------------------------------------------------------

/// Refcounted read-leases on cache paths with an in-flight peer serve.
/// The blob endpoint holds one around every chunk read; the downloader's
/// delete/replace steps refuse to touch a leased path (bounded wait, then an
/// explicit error) so an in-flight source is never clobbered mid-read.
#[derive(Clone, Default)]
pub struct ServeLeases(Arc<Mutex<HashMap<PathBuf, usize>>>);

impl std::fmt::Debug for ServeLeases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ServeLeases({} active)", self.0.lock().unwrap().len())
    }
}

impl ServeLeases {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lease(&self, path: &Path) -> ServeLeaseGuard {
        let mut map = self.0.lock().unwrap();
        *map.entry(path.to_path_buf()).or_insert(0) += 1;
        ServeLeaseGuard {
            leases: self.clone(),
            path: path.to_path_buf(),
        }
    }

    pub fn is_leased(&self, path: &Path) -> bool {
        self.0
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .unwrap_or(0)
            > 0
    }

    /// Waits until `path` has no active serve lease. `Err` after `timeout`
    /// with the path still leased — callers surface that instead of deleting
    /// or renaming over an in-flight source.
    pub fn wait_unleased(
        &self,
        path: &Path,
        timeout: std::time::Duration,
    ) -> Result<(), AssetAiError> {
        let started = std::time::Instant::now();
        while self.is_leased(path) {
            if started.elapsed() > timeout {
                return Err(AssetAiError::Download(format!(
                    "{} is being served to a peer right now — refusing to replace it",
                    path.display()
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        Ok(())
    }
}

pub struct ServeLeaseGuard {
    leases: ServeLeases,
    path: PathBuf,
}

impl Drop for ServeLeaseGuard {
    fn drop(&mut self) {
        let mut map = self.leases.0.lock().unwrap();
        if let Some(count) = map.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                map.remove(&self.path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Receiver-side plan: which peers to try, with which tickets
// ---------------------------------------------------------------------------

/// Bounds on what a request may inject (hostile-input hygiene).
pub const MAX_PLAN_SOURCES: usize = 8;
pub const MAX_PLAN_TICKETS: usize = 1024;

/// The peer download plan for one job. Sources are tried in order, before
/// the canonical Hugging Face path. Selection stays centrally controlled:
/// the list comes from the coordinator (request fields) or from the operator
/// (`MAKEPAD_AI_PEER_SOURCES` env) — the service never discovers peers ad hoc.
#[derive(Clone)]
pub struct PeerPlan {
    /// This box's durable node_key (the ticket receiver scope).
    pub receiver_key: String,
    /// Service base URLs, e.g. "http://10.0.0.217:8765".
    pub sources: Vec<String>,
    /// Coordinator-minted tickets (self-describing scope).
    pub tickets: Vec<PeerTicket>,
    /// Fleet-shared secret for self-minting tickets when the coordinator
    /// did not supply them (env-injected deployments).
    pub secret: Option<TransferSecret>,
    /// Normalized operator-configured sources which may use the local shared
    /// secret. Request-provided URLs never enter this set: otherwise the
    /// public generate API would become a signing oracle for arbitrary hosts.
    self_mint_sources: Vec<String>,
}

impl std::fmt::Debug for PeerPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PeerPlan{{receiver:{}, sources:{:?}, tickets:{}, secret:{}}}",
            self.receiver_key,
            self.sources,
            self.tickets.len(),
            if self.secret.is_some() { "present" } else { "absent" }
        )
    }
}

impl PeerPlan {
    /// Builds the plan for one job: request-provided sources first (the
    /// coordinator's explicit selection), then env-injected sources, deduped,
    /// bounded to [`MAX_PLAN_SOURCES`]. Returns `None` when there is nothing
    /// to try (the job then goes straight to Hugging Face).
    pub fn for_job(
        request_sources: &[String],
        request_tickets: &[String],
        env_sources: &[String],
        receiver_key: &str,
        secret: Option<TransferSecret>,
    ) -> Option<Arc<PeerPlan>> {
        let mut sources: Vec<String> = Vec::new();
        for raw in request_sources {
            let Some(base) = normalize_source(raw) else {
                continue;
            };
            if !sources.contains(&base) {
                sources.push(base);
            }
            if sources.len() >= MAX_PLAN_SOURCES {
                break;
            }
        }
        let mut self_mint_sources = Vec::new();
        for raw in env_sources {
            let Some(base) = normalize_source(raw) else {
                continue;
            };
            if !self_mint_sources.contains(&base) {
                self_mint_sources.push(base.clone());
            }
            if !sources.contains(&base) && sources.len() < MAX_PLAN_SOURCES {
                sources.push(base);
            }
        }
        if sources.is_empty() {
            return None;
        }
        let mut tickets = Vec::new();
        for text in request_tickets.iter().take(MAX_PLAN_TICKETS) {
            match PeerTicket::parse(text.trim()) {
                Some(ticket) => tickets.push(ticket),
                None => eprintln!("peer: ignoring a malformed transfer ticket"),
            }
        }
        Some(Arc::new(PeerPlan {
            receiver_key: receiver_key.to_string(),
            sources,
            tickets,
            secret,
            self_mint_sources,
        }))
    }

    /// `MAKEPAD_AI_PEER_SOURCES`: comma/whitespace-separated service base
    /// URLs ("http://10.0.0.217:8765, http://10.0.0.123:8765").
    pub fn env_sources() -> Vec<String> {
        let Ok(text) = std::env::var("MAKEPAD_AI_PEER_SOURCES") else {
            return Vec::new();
        };
        text.split(|c: char| c == ',' || c.is_whitespace())
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// The ticket to present to `source_key` for `digest`: a coordinator
    /// ticket whose scope matches and is not expired, else a self-minted one
    /// when this box holds the fleet transfer secret.
    pub fn ticket_for(
        &self,
        source_base: &str,
        source_key: &str,
        digest: &str,
        now_unix: u64,
    ) -> Option<String> {
        if let Some(ticket) = self.tickets.iter().find(|t| {
            t.source_key == source_key
                && t.digest == digest
                && t.receiver_key == self.receiver_key
                && t.expires_unix >= now_unix
        }) {
            return Some(ticket.encode());
        }
        if !self.self_mint_sources.iter().any(|base| base == source_base) {
            return None;
        }
        let secret = self.secret.as_ref()?;
        Some(PeerTicket::mint(
            secret,
            source_key,
            &self.receiver_key,
            digest,
            now_unix + TICKET_TTL_SECS,
        ))
    }
}

/// Peer sources are origins, not general URLs. Besides making source/ticket
/// binding unambiguous, this keeps untrusted request fields out of HTTP request
/// lines and headers (no userinfo, path, query, fragment, whitespace or CTLs).
fn normalize_source(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty()
        || raw
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return None;
    }
    let parsed = crate::http_client::parse_url(raw).ok()?;
    if parsed.target != "/"
        || parsed.host.is_empty()
        || parsed.host.contains('@')
        || !parsed
            .host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return None;
    }
    let scheme = if parsed.https { "https" } else { "http" };
    let default_port = if parsed.https { 443 } else { 80 };
    if parsed.port == default_port {
        Some(format!("{scheme}://{}", parsed.host.to_ascii_lowercase()))
    } else {
        Some(format!(
            "{scheme}://{}:{}",
            parsed.host.to_ascii_lowercase(),
            parsed.port
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_rfc4231_vectors() {
        // Case 1: 20x0x0b key, "Hi There".
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            to_hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Case 2: key "Jefe", "what do ya want for nothing?".
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            to_hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Case 6: key longer than the block size gets hashed first.
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            to_hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    fn secret() -> TransferSecret {
        TransferSecret::new(b"unit-test-secret-0123456789").unwrap()
    }

    #[test]
    fn ticket_roundtrip_and_scope_denials() {
        let source = "a".repeat(32);
        let receiver = "b".repeat(32);
        let digest = "c".repeat(64);
        let now = 1_700_000_000u64;
        let text = PeerTicket::mint(&secret(), &source, &receiver, &digest, now + 60);
        let ticket = PeerTicket::parse(&text).expect("parses");
        assert_eq!(ticket.encode(), text);
        ticket
            .verify(&secret(), &source, &receiver, &digest, now)
            .expect("valid ticket verifies");

        // Expired.
        let expired = PeerTicket::parse(&PeerTicket::mint(
            &secret(),
            &source,
            &receiver,
            &digest,
            now - 1,
        ))
        .unwrap();
        assert_eq!(
            expired.verify(&secret(), &source, &receiver, &digest, now),
            Err("ticket expired")
        );
        // Far-future lifetime cap.
        let eternal = PeerTicket::parse(&PeerTicket::mint(
            &secret(),
            &source,
            &receiver,
            &digest,
            now + TICKET_MAX_TTL_SECS + 10,
        ))
        .unwrap();
        assert!(eternal
            .verify(&secret(), &source, &receiver, &digest, now)
            .is_err());
        // Wrong receiver claim.
        assert_eq!(
            ticket.verify(&secret(), &source, &"d".repeat(32), &digest, now),
            Err("ticket is scoped to a different receiver node")
        );
        // Wrong digest.
        assert_eq!(
            ticket.verify(&secret(), &source, &receiver, &"e".repeat(64), now),
            Err("ticket is scoped to a different artifact digest")
        );
        // Wrong source node.
        assert_eq!(
            ticket.verify(&secret(), &"f".repeat(32), &receiver, &digest, now),
            Err("ticket is scoped to a different source node")
        );
        // Tampered signature.
        let mut forged = ticket.clone();
        forged.sig = "0".repeat(64);
        assert_eq!(
            forged.verify(&secret(), &source, &receiver, &digest, now),
            Err("ticket signature invalid")
        );
        // Wrong secret.
        let other = TransferSecret::new(b"another-secret-9876543210").unwrap();
        assert!(ticket.verify(&other, &source, &receiver, &digest, now).is_err());
        // Malformed strings never parse.
        for bad in [
            "",
            "mtk1.1.2.3",
            "mtk2.1.aa.bb.cc.dd",
            &format!("mtk1.nope.{source}.{receiver}.{digest}.{}", "0".repeat(64)),
            &text[..text.len() - 1],
        ] {
            assert!(PeerTicket::parse(bad).is_none(), "parsed: {bad:?}");
        }
    }

    #[test]
    fn secrets_are_redacted_and_length_checked() {
        assert!(TransferSecret::new(b"short").is_none());
        let secret = secret();
        let debug = format!("{secret:?}");
        assert!(!debug.contains("unit-test"), "{debug}");
        let ticket = PeerTicket::parse(&PeerTicket::mint(
            &secret,
            &"a".repeat(32),
            &"b".repeat(32),
            &"c".repeat(64),
            9_999_999_999,
        ))
        .unwrap();
        let debug = format!("{ticket:?}");
        assert!(debug.contains("sig:redacted"), "{debug}");
        assert!(!debug.contains(&ticket.sig), "{debug}");
    }

    #[test]
    fn leases_refcount_and_block_replacement() {
        let leases = ServeLeases::new();
        let path = Path::new("/tmp/some/blob");
        assert!(!leases.is_leased(path));
        let a = leases.lease(path);
        let b = leases.lease(path);
        assert!(leases.is_leased(path));
        drop(a);
        assert!(leases.is_leased(path), "second lease still held");
        assert!(leases
            .wait_unleased(path, std::time::Duration::from_millis(60))
            .is_err());
        drop(b);
        assert!(!leases.is_leased(path));
        leases
            .wait_unleased(path, std::time::Duration::from_millis(60))
            .expect("unleased path passes");
    }

    #[test]
    fn plan_bounds_and_ticket_selection() {
        let receiver = "b".repeat(32);
        let source = "a".repeat(32);
        let digest = "c".repeat(64);
        let now = 1_700_000_000u64;
        let minted = PeerTicket::mint(&secret(), &source, &receiver, &digest, now + 60);
        let plan = PeerPlan::for_job(
            &["http://10.0.0.1:8765/".into(), "not-a-url".into()],
            &[minted.clone(), "garbage".into()],
            &["http://10.0.0.1:8765".into(), "http://10.0.0.2:8765".into()],
            &receiver,
            None,
        )
        .expect("plan");
        assert_eq!(
            plan.sources,
            vec![
                "http://10.0.0.1:8765".to_string(),
                "http://10.0.0.2:8765".to_string()
            ]
        );
        assert_eq!(plan.tickets.len(), 1);
        // Coordinator ticket matched by scope.
        assert_eq!(
            plan.ticket_for("http://10.0.0.1:8765", &source, &digest, now),
            Some(minted)
        );
        // No ticket for an unknown source without a secret.
        assert!(plan
            .ticket_for(
                "http://10.0.0.1:8765",
                &"f".repeat(32),
                &digest,
                now
            )
            .is_none());
        // A request-provided URL must never turn the receiver's shared secret
        // into a signing oracle.
        let plan = PeerPlan::for_job(
            &["http://10.0.0.1:8765".into()],
            &[],
            &[],
            &receiver,
            Some(secret()),
        )
        .unwrap();
        assert!(plan
            .ticket_for("http://10.0.0.1:8765", &source, &digest, now)
            .is_none());
        // Operator-configured sources may self-mint in shared-secret mode.
        let plan = PeerPlan::for_job(
            &[],
            &[],
            &["http://10.0.0.1:8765".into()],
            &receiver,
            Some(secret()),
        )
        .unwrap();
        let text = plan
            .ticket_for("http://10.0.0.1:8765", &source, &digest, now)
            .unwrap();
        let ticket = PeerTicket::parse(&text).unwrap();
        ticket
            .verify(&secret(), &source, &receiver, &digest, now)
            .expect("self-minted ticket verifies");
        // Empty inputs -> no plan.
        assert!(PeerPlan::for_job(&[], &[], &[], &receiver, None).is_none());
        // Debug output never contains ticket signatures.
        let debug = format!("{plan:?}");
        assert!(debug.contains("secret:present"));
        assert!(!debug.contains(&ticket.sig));
    }

    #[test]
    fn plan_accepts_origins_only() {
        let receiver = "b".repeat(32);
        let plan = PeerPlan::for_job(
            &[
                " HTTP://not-lowercase.invalid".into(),
                "http://user@example.com".into(),
                "http://example.com/path".into(),
                "http://example.com/?query".into(),
                "http://example.com\r\nX-Evil: yes".into(),
                "http://EXAMPLE.com:8765/".into(),
            ],
            &[],
            &[],
            &receiver,
            None,
        )
        .unwrap();
        assert_eq!(plan.sources, vec!["http://example.com:8765"]);
    }
}
