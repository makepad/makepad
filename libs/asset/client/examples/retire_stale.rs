//! Retire catalog assets by alias prefix — the stale rows an older import
//! left behind, which newer imports no longer produce.
//!
//!   cargo run --release -p makepad-asset-client --example retire_stale -- \
//!       <control-addr> <data-addr> <token> <namespace> <alias-substring> [--apply]
//!
//! Without `--apply` it only lists what it would retire.

use makepad_asset_client::{ApiEndpoints, AssetClient, CatalogQuery, ClientConfig};

fn alias(hit: &makepad_asset_client::CatalogHit) -> String {
    hit.alias.as_ref().map(|a| a.to_string()).unwrap_or_else(|| "<no alias>".into())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let control: std::net::SocketAddr = args.next().expect("control").parse().expect("control");
    let data: std::net::SocketAddr = args.next().expect("data").parse().expect("data");
    let token = args.next().expect("token");
    let ns = args.next().expect("namespace");
    let needle = args.next().expect("alias substring");
    let apply = args.next().as_deref() == Some("--apply");

    let mut config = ClientConfig::new(std::env::temp_dir().join("retire-stale-cache"));
    config.token = Some(token);
    let mut client =
        AssetClient::connect(config, ApiEndpoints { control, data }, None).expect("connect");

    let mut cursor = None;
    let mut hits = Vec::new();
    loop {
        let mut query = CatalogQuery::browse(100);
        query.live_only = true;
        query.namespace = Some(ns.clone());
        let page = client.catalog_search(&query, cursor.as_ref()).expect("search");
        for hit in &page.hits {
            if alias(hit).contains(&needle) {
                hits.push((hit.asset_id, alias(hit)));
            }
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    println!("{} matching asset(s){}", hits.len(), if apply { ", retiring" } else { " (dry run)" });
    for (id, name) in &hits {
        if apply {
            match client.retire_asset(id) {
                Ok(_) => println!("  retired {name}"),
                Err(e) => println!("  FAILED {name}: {e}"),
            }
        } else {
            println!("  would retire {name}");
        }
    }
}
