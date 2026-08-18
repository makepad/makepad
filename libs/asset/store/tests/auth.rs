//! Auth behavior: hashed-only tokens, uniform authentication refusal,
//! explicit capability grants, and credentials-never-stored.

mod common;
use common::*;
use makepad_asset_store::{token_hash, Capability, Scope, ServerError};
use std::fs;

const SECRET: &[u8] = b"THE-DISTINCTIVE-TRANSPORT-SECRET-0123456789abcdef";

#[test]
fn token_roundtrip_and_uniform_refusals() {
    let (_root, core) = open_core("token");
    let auth = core.auth();
    let p = pid_n(1);
    auth.create_principal(&p, "uploader", NOW).unwrap();
    auth.register_token(&p, &token_hash(SECRET), NOW + 100_000, NOW).unwrap();

    assert_eq!(auth.authenticate(SECRET, NOW + 1).unwrap(), p);

    // Wrong secret, expired, revoked, disabled: all identical refusals.
    assert!(matches!(
        auth.authenticate(b"wrong secret", NOW + 1).unwrap_err(),
        ServerError::Unauthenticated
    ));
    assert!(matches!(
        auth.authenticate(SECRET, NOW + 100_000).unwrap_err(),
        ServerError::Unauthenticated
    ));
    auth.revoke_token(&token_hash(SECRET)).unwrap();
    assert!(matches!(
        auth.authenticate(SECRET, NOW + 1).unwrap_err(),
        ServerError::Unauthenticated
    ));

    let secret2: &[u8] = b"second-secret";
    auth.register_token(&p, &token_hash(secret2), NOW + 100_000, NOW).unwrap();
    auth.disable_principal(&p).unwrap();
    assert!(matches!(
        auth.authenticate(secret2, NOW + 1).unwrap_err(),
        ServerError::Unauthenticated
    ));
}

#[test]
fn token_admission_is_fail_closed() {
    let (_root, core) = open_core("token_admission");
    let auth = core.auth();
    // Token for an unknown principal refuses.
    assert!(matches!(
        auth.register_token(&pid_n(9), &token_hash(SECRET), NOW + 10, NOW).unwrap_err(),
        ServerError::NotFound { what: "principal" }
    ));
    let p = pid_n(1);
    auth.create_principal(&p, "u", NOW).unwrap();
    // Already-expired token refuses.
    assert!(matches!(
        auth.register_token(&p, &token_hash(SECRET), NOW, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "token already expired" }
    ));
    // Duplicate principal id refuses.
    assert!(matches!(
        auth.create_principal(&p, "other-name", NOW).unwrap_err(),
        ServerError::Conflict { what: "principal id" }
    ));
}

#[test]
fn secret_bytes_never_reach_the_catalog_files() {
    let (root, core) = open_core("no_secret");
    let auth = core.auth();
    let p = pid_n(1);
    auth.create_principal(&p, "uploader", NOW).unwrap();
    auth.register_token(&p, &token_hash(SECRET), NOW + 100_000, NOW).unwrap();
    auth.authenticate(SECRET, NOW + 1).unwrap();
    drop(core); // close the connection so WAL contents are final on disk

    // Scan every byte the server persisted: the secret must appear nowhere
    // (only its SHA-256 does).
    let mut scanned = 0;
    for name in ["catalog.sqlite3", "catalog.sqlite3-wal", "catalog.sqlite3-shm"] {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        scanned += 1;
        assert!(
            !bytes.windows(SECRET.len()).any(|w| w == SECRET),
            "secret bytes found in {name}"
        );
    }
    assert!(scanned > 0, "no catalog files scanned");
}

#[test]
fn grants_are_explicit_and_scoped() {
    let (_root, core) = open_core("grants");
    let auth = core.auth();
    let p = pid_n(1);
    auth.create_principal(&p, "worker", NOW).unwrap();

    // No grant at all: denied.
    assert!(matches!(
        auth.require(&p, Capability::BlobWrite, "rik2").unwrap_err(),
        ServerError::Denied { capability: "blob_write" }
    ));

    // Namespace-scoped grant works only in that namespace.
    auth.grant(&p, Capability::BlobWrite, Scope::Namespace("rik2"), NOW).unwrap();
    auth.require(&p, Capability::BlobWrite, "rik2").unwrap();
    assert!(matches!(
        auth.require(&p, Capability::BlobWrite, "other").unwrap_err(),
        ServerError::Denied { .. }
    ));
    // A different capability in the same namespace is still denied.
    assert!(matches!(
        auth.require(&p, Capability::AssetPublish, "rik2").unwrap_err(),
        ServerError::Denied { capability: "asset_publish" }
    ));

    // Wildcard grant spans namespaces.
    auth.grant(&p, Capability::JobWorker, Scope::All, NOW).unwrap();
    auth.require(&p, Capability::JobWorker, "rik2").unwrap();
    auth.require(&p, Capability::JobWorker, "other").unwrap();

    // Revocation returns to denied.
    auth.revoke_grant(&p, Capability::BlobWrite, Scope::Namespace("rik2")).unwrap();
    assert!(matches!(
        auth.require(&p, Capability::BlobWrite, "rik2").unwrap_err(),
        ServerError::Denied { .. }
    ));

    // Granting to an unknown principal refuses.
    assert!(matches!(
        auth.grant(&pid_n(9), Capability::BlobWrite, Scope::All, NOW).unwrap_err(),
        ServerError::NotFound { what: "principal" }
    ));
}
