//! Which catalog assets carry a thumbnail, and which do not — asked of a
//! LIVE server over HTTP, so it never touches a root a server is using.
//!
//!   cargo run --release -p makepad-asset-client --example thumb_audit -- \
//!       http://127.0.0.1:50213 http://127.0.0.1:50214 <token> [namespace]

use makepad_asset_client::{ApiEndpoints, AssetClient, CatalogQuery, ClientConfig};

fn alias(hit: &makepad_asset_client::CatalogHit) -> String {
    hit.alias.as_ref().map(|a| a.to_string()).unwrap_or_else(|| "<no alias>".into())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let control: std::net::SocketAddr = args.next().expect("control addr").parse().expect("control addr");
    let data: std::net::SocketAddr = args.next().expect("data addr").parse().expect("data addr");
    let token = args.next().expect("token");
    let want_ns = args.next();

    let mut config = ClientConfig::new(std::env::temp_dir().join("thumb-audit-cache"));
    config.token = Some(token);
    let endpoints = ApiEndpoints { control, data };
    let mut client = AssetClient::connect(config, endpoints, None).expect("connect");

    let mut query = CatalogQuery::browse(100);
    query.live_only = true;
    if let Some(ns) = &want_ns {
        query.namespace = Some(ns.clone());
    }
    let page = client.catalog_search(&query, None).expect("search");
    println!("hits {} (total {})", page.hits.len(), page.total);

    let (mut with, mut without) = (0u32, 0u32);
    let mut blanks: Vec<String> = Vec::new();
    for hit in &page.hits {
        let detail = match client.asset_detail(&hit.asset_id) {
            Ok(d) => d,
            Err(e) => {
                println!("{}: detail failed: {e}", alias(hit));
                continue;
            }
        };
        let Some(head) = detail.latest_published() else {
            println!("{}: no published revision", alias(hit));
            continue;
        };
        let manifest = match client.fetch_asset_manifest(&head.revision) {
            Ok(m) => m,
            Err(e) => {
                println!("{}: manifest failed: {e}", alias(hit));
                continue;
            }
        };
        if std::env::var("DUMP_ONE").ok().as_deref() == Some(alias(hit).as_str()) {
            println!("=== {} ===\n{:#?}", alias(hit), manifest);
        }
        let roles: Vec<String> =
            manifest.files.iter().map(|f| format!("{:?}", f.role)).collect();
        match &manifest.thumbnail {
            Some(_) => with += 1,
            None => {
                without += 1;
                blanks.push(format!(
                    "{}  kind={:?}  files=[{}]",
                    alias(hit),
                    hit.kind,
                    roles.join(",")
                ));
            }
        }
    }
    println!("\nwith thumbnail: {with}   WITHOUT: {without}");
    for b in blanks.iter().take(60) {
        println!("  BLANK {b}");
    }
}
