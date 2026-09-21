#![cfg(not(target_arch = "wasm32"))]

use makepad_asset_client::{ApiEndpoints, PublishFile, PublishRequest, PublishThumbnail};
use makepad_asset_data::{AssetAlias, AssetKind, FileRole, MediaType, ThumbnailMedia};
use makepad_flow::client::{ClientError, Endpoints, FlowClient};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);
impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flow-assets-route-{}-{label}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempRoot { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn png() -> Vec<u8> {
    makepad_ai_hub::testpattern::encode_png_rgba(&vec![180; 256 * 256 * 4], 256, 256).unwrap()
}

#[test]
fn proxy_lists_and_serves_flow_asset_thumbnail() {
    let store_root = TempRoot::new("store");
    let flow_root = TempRoot::new("flow");
    let mut store_config = makepad_asset_store::ServerConfig::new(store_root.0.clone());
    store_config.control_addr = "127.0.0.1:0".parse().unwrap();
    store_config.data_addr = "127.0.0.1:0".parse().unwrap();
    store_config.bootstrap_admin = true;
    store_config.discovery = None;
    store_config.log = false;
    let mut store = makepad_asset_store::AssetServer::start(store_config).unwrap();
    let token = std::fs::read_to_string(store_root.0.join("admin-token")).unwrap().trim().to_string();
    let endpoints = ApiEndpoints { control: store.control_addr(), data: store.data_addr() };
    let mut client_config = makepad_asset_client::ClientConfig::new(store_root.0.join("seed-cache"));
    client_config.token = Some(token.clone());
    let mut asset_client = makepad_asset_client::AssetClient::connect(client_config, endpoints, Some(store.server_id())).unwrap();
    let image = png();
    let mut request = PublishRequest::new(
        "flows",
        AssetKind::Texture,
        "Route picture",
        PublishFile { bytes: image.clone(), media: MediaType::Png, role: FileRole::Texture, media_millis: 0, dims: Some((256, 256)) },
        PublishThumbnail::plain(image.clone(), ThumbnailMedia::Png, 256, 256),
    );
    request.alias = Some(AssetAlias::new("flows/route-picture").unwrap());
    request.tags = vec!["flow".into(), "route".into()];
    asset_client.publish_artifact(&request).unwrap();
    let large_image = makepad_ai_hub::testpattern::encode_png_rgba(
        &vec![180; 1024 * 512 * 4],
        1024,
        512,
    )
    .unwrap();
    let mut oversized = PublishRequest::new(
        "flows",
        AssetKind::Texture,
        "Oversized preview",
        PublishFile {
            bytes: large_image.clone(),
            media: MediaType::Png,
            role: FileRole::Texture,
            media_millis: 0,
            dims: Some((1024, 512)),
        },
        PublishThumbnail::plain(large_image, ThumbnailMedia::Png, 1024, 512),
    );
    oversized.alias = Some(AssetAlias::new("flows/oversized-preview").unwrap());
    let oversized_id = asset_client.publish_artifact(&oversized).unwrap().asset_id;
    let mut tagged = PublishRequest::new(
        "elsewhere",
        AssetKind::Texture,
        "Tagged picture",
        PublishFile {
            bytes: image.clone(),
            media: MediaType::Png,
            role: FileRole::Texture,
            media_millis: 0,
            dims: Some((256, 256)),
        },
        PublishThumbnail::plain(image.clone(), ThumbnailMedia::Png, 256, 256),
    );
    tagged.alias = Some(AssetAlias::new("elsewhere/tagged-picture").unwrap());
    tagged.tags = vec!["flow".into()];
    asset_client.publish_artifact(&tagged).unwrap();
    let mut unaliased = PublishRequest::new(
        "flows",
        AssetKind::Data,
        "Unaliased data",
        PublishFile {
            bytes: b"original asset bytes".to_vec(),
            media: MediaType::Bin,
            role: FileRole::Source,
            media_millis: 0,
            dims: None,
        },
        PublishThumbnail::plain(image.clone(), ThumbnailMedia::Png, 256, 256),
    );
    unaliased.alias = None;
    let unaliased_id = asset_client.publish_artifact(&unaliased).unwrap().asset_id;
    drop(asset_client);

    let mut flow_config = FlowServerConfig::new(flow_root.0.clone());
    flow_config.log = Box::new(|_| {});
    flow_config.asset.endpoints = Some(endpoints);
    flow_config.asset.server_id = Some(store.server_id());
    flow_config.asset.token = Some(token);
    let flow_server = FlowServer::start(flow_config).unwrap();
    let served = flow_server.endpoints();
    let flow_client = FlowClient::connect(
        Endpoints { control: served.control, data: served.data },
        served.token.clone(),
        Some(served.server_id),
    ).unwrap();
    let rows = flow_client.assets("Route", Some("flows"), 10).unwrap().assets;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].alias.as_deref(), Some("flows/route-picture"));
    assert_eq!(rows[0].tags, vec!["flow", "route"]);
    let default_rows = flow_client.assets("", Some("flows"), 10).unwrap().assets;
    assert_eq!(default_rows.len(), 4);
    assert!(default_rows.iter().any(|asset| asset.namespace == "elsewhere"));
    let mut page = flow_client.assets_page("", None, 1, None).unwrap();
    assert_eq!(page.assets.len(), 1);
    let first_cursor = page.cursor.clone().expect("more than one catalog result");
    let mut ids = std::collections::HashSet::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= 5, "pagination failed to advance");
        for row in &page.assets { assert!(ids.insert(row.id.clone()), "duplicate result on a later page"); }
        let Some(cursor) = page.cursor else { break };
        page = flow_client.assets_page("", None, 1, Some(&cursor)).unwrap();
    }
    assert_eq!(ids.len(), 4);
    assert!(flow_client.assets_page("different", None, 1, Some(&first_cursor)).is_err());
    let thumb = flow_client.asset_thumbnail("flows/route-picture").unwrap();
    assert_eq!(thumb.content_type, "image/png");
    assert_eq!(thumb.bytes.as_ref(), image.as_slice());
    let content = flow_client.asset_content(&unaliased_id.to_string()).unwrap();
    assert_eq!(content.content_type, "application/octet-stream");
    assert_eq!(content.bytes.as_ref(), b"original asset bytes");
    let preview = flow_client.asset_preview(&unaliased_id.to_string()).unwrap();
    assert_eq!(preview.content_type, "text/plain; charset=utf-8");
    assert_eq!(preview.bytes.as_ref(), "Binary data · 20 bytes".as_bytes());
    let oversized_preview = flow_client.asset_preview(&oversized_id.to_string()).unwrap();
    assert_eq!(oversized_preview.content_type, "image/png");
    assert!(oversized_preview.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(
        u32::from_be_bytes(oversized_preview.bytes[16..20].try_into().unwrap()),
        256
    );
    assert_eq!(
        u32::from_be_bytes(oversized_preview.bytes[20..24].try_into().unwrap()),
        128
    );
    let thumb_by_id = flow_client.asset_thumbnail(&unaliased_id.to_string()).unwrap();
    assert_eq!(thumb_by_id.content_type, "image/png");
    assert_eq!(thumb_by_id.bytes.as_ref(), image.as_slice());

    let mut wrong_token = served.token.clone();
    let last = wrong_token.pop().unwrap();
    wrong_token.push(if last == '0' { '1' } else { '0' });
    let unauthorized = FlowClient::connect(
        Endpoints { control: served.control, data: served.data },
        wrong_token,
        Some(served.server_id),
    );
    assert!(matches!(unauthorized, Err(ClientError::Unauthorized)));
    assert!(matches!(
        flow_client.asset_content("not/an-asset-id"),
        Err(ClientError::Protocol(_))
    ));
    assert!(matches!(
        flow_client.asset_thumbnail("../bad"),
        Err(ClientError::Protocol(_))
    ));
    flow_server.shutdown();
    store.shutdown();
}

#[test]
fn proxy_reports_no_discovered_store_and_retries_per_request() {
    let root = TempRoot::new("missing");
    let mut config = FlowServerConfig::new(root.0.clone());
    config.log = Box::new(|_| {});
    config.asset.discovery_wait_ms = 0;
    let server = FlowServer::start(config).unwrap();
    let served = server.endpoints();
    let client = FlowClient::connect(
        Endpoints { control: served.control, data: served.data },
        served.token,
        Some(served.server_id),
    ).unwrap();
    for _ in 0..2 {
        let error = client.assets("", Some("flows"), 10).unwrap_err();
        assert!(matches!(error, ClientError::Http { status: 503, .. }));
        assert!(error.to_string().contains("no asset server discovered on this LAN"));
    }
    server.shutdown();
}
