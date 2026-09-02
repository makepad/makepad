#![cfg(all(feature = "web", not(target_arch = "wasm32")))]

mod common;

use common::{FixtureOptions, FixtureServer, FixtureStore};
use makepad_asset_client::{
    BaseUrl, OwnedRequest, PlatformHttpTransport, Transport, TransportCompletion, TransportError,
    TransportId, TransportMethod,
};
use makepad_asset_data::BlobId;
use std::time::{Duration, Instant};

fn wait_for(
    transport: &mut PlatformHttpTransport,
    id: TransportId,
) -> TransportCompletion {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut completions = Vec::new();
        transport.poll(&mut completions);
        if let Some(completion) = completions.into_iter().find(|item| item.id == id) {
            return completion;
        }
        assert!(Instant::now() < deadline, "platform request did not complete");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn platform_transport_get_range_not_found_and_cancel() {
    let mut store = FixtureStore::default();
    let revision = store.add_prop(7, "portable", None, "portable", b"0123456789".to_vec(), vec![]);
    let blob = store
        .assets
        .iter()
        .find(|asset| asset.revision == revision.revision)
        .unwrap()
        .manifest
        .files[0]
        .blob;
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let base = BaseUrl::parse(format!("http://127.0.0.1:{}", fixture.data.addr.port())).unwrap();
    let blob_target = makepad_asset_client::wire::path_blob(&blob);
    let blob_url = base.join(&blob_target).unwrap();
    let mut transport = PlatformHttpTransport::new();

    let get = transport.start(OwnedRequest::new(TransportMethod::Get, blob_url.clone()));
    let response = wait_for(&mut transport, get).result.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"0123456789");
    assert!(response.headers.iter().all(|(name, _)| *name == name.to_ascii_lowercase()));

    let range = transport.start(
        OwnedRequest::new(TransportMethod::Get, blob_url).header("Range", "bytes=4-"),
    );
    let response = wait_for(&mut transport, range).result.unwrap();
    assert_eq!(response.status, 206);
    assert_eq!(response.body, b"456789");
    assert_eq!(response.header("content-range"), Some("bytes 4-9/10"));

    let missing = transport.start(OwnedRequest::new(
        TransportMethod::Get,
        base.join(&makepad_asset_client::wire::path_blob(&BlobId::from_bytes([0; 32])))
            .unwrap(),
    ));
    assert_eq!(wait_for(&mut transport, missing).result.unwrap().status, 404);

    let cancelled = transport.start(OwnedRequest::new(
        TransportMethod::Get,
        base.join("/v1/missing").unwrap(),
    ));
    transport.cancel(cancelled);
    assert!(matches!(
        wait_for(&mut transport, cancelled).result,
        Err(TransportError::Cancelled)
    ));
}
