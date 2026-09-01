//! One-off audit + repair for cross-published game sources (2026-08-26).
//!
//! The bug: a thumbnail publish raced a Play-switch and republished one
//! game's source over another (Chicane Circuit's head became arena's).
//! The store keeps history, so repair = one more publish carrying the
//! game's own last-good source.
//!
//! Usage (asset-ui server on localhost):
//!   repair_games audit
//!   repair_games show ast_...
//!   repair_games repair <namespace> <slug> <arev_...>
//!
//! NOT a shipped tool: working-tree scaffolding for the repair session.

use makepad_asset_client::{ApiEndpoints, AssetClient, CatalogQuery, ClientConfig};
use makepad_asset_data::{AssetKind, FileRole};
use makepad_asset_importer::games_import;
use std::collections::HashMap;
use std::str::FromStr;

fn server_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("local/asset-ui/asset-server")
}

fn connect() -> AssetClient {
    let dir = server_dir();
    let listen = std::fs::read_to_string(dir.join("listen")).expect("listen file");
    let mut parts = listen.trim().split(':');
    let ip = parts.next().expect("ip");
    let control: u16 = parts.next().expect("control").parse().expect("control port");
    let data: u16 = parts.next().expect("data").parse().expect("data port");
    let token = std::fs::read_to_string(dir.join("admin-token")).expect("admin token");
    let id_hex = std::fs::read_to_string(dir.join("server-id")).expect("server id");
    let id_hex = id_hex.trim();
    let mut server_id = [0u8; 16];
    for i in 0..16 {
        server_id[i] = u8::from_str_radix(&id_hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    let cache = std::env::temp_dir().join("repair-games-cache");
    let mut cfg = ClientConfig::new(cache);
    cfg.token = Some(token.trim().to_string());
    let endpoints = ApiEndpoints {
        control: format!("{ip}:{control}").parse().expect("control addr"),
        data: format!("{ip}:{data}").parse().expect("data addr"),
    };
    AssetClient::connect(cfg, endpoints, Some(server_id)).expect("connect")
}

/// The head source of one game, with enough identity to compare.
#[allow(dead_code)] // identity carried for the comparison prints
struct Head {
    asset: makepad_asset_data::AssetId,
    title: String,
    alias: String,
    revision: makepad_asset_data::AssetRevisionId,
    blob: makepad_asset_data::BlobId,
    text: String,
}

fn head_source(client: &mut AssetClient, alias_str: &str) -> Result<Head, String> {
    let alias =
        makepad_asset_data::AssetAlias::from_str(alias_str).map_err(|e| format!("{e}"))?;
    let head = client.resolve_alias(&alias).map_err(|e| format!("resolve: {e}"))?;
    let manifest = client
        .fetch_asset_manifest(&head.head_revision)
        .map_err(|e| format!("manifest: {e}"))?;
    let source = manifest
        .files
        .iter()
        .find(|f| f.role == FileRole::Source)
        .ok_or("no source file")?;
    let bytes = client
        .fetch_blob_bytes(&source.blob, Some(source.byte_len))
        .map_err(|e| format!("blob: {e}"))?;
    Ok(Head {
        asset: head.asset_id,
        title: String::new(),
        alias: alias_str.to_string(),
        revision: head.head_revision,
        blob: source.blob,
        text: String::from_utf8_lossy(&bytes).to_string(),
    })
}

fn games(
    client: &mut AssetClient,
) -> Vec<(String, String, String, makepad_asset_data::AssetId)> {
    let query = CatalogQuery {
        text: String::new(),
        namespace: None,
        kind: Some(AssetKind::Game),
        category: None,
        tag: Some("game".into()),
        exclude_tag: None,
        creator: None,
        live_only: true,
        page_size: 50,
        facets: 0,
    };
    let page = client.catalog_search(&query, None).expect("search games");
    page.hits
        .into_iter()
        .filter_map(|hit| {
            let alias = hit.alias?;
            Some((hit.title, hit.snippet, alias.to_string(), hit.asset_id))
        })
        .collect()
}

/// A few lines that say what a source IS: the first non-empty,
/// non-boilerplate lines.
fn fingerprint(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .take(160)
        .collect()
}

fn audit(client: &mut AssetClient) {
    let list = games(client);
    println!("{} games listed", list.len());
    let mut by_blob: HashMap<makepad_asset_data::BlobId, Vec<String>> = HashMap::new();
    for (title, _snippet, alias, asset) in &list {
        match head_source(client, alias) {
            Ok(head) => {
                let blob_hex = format!("{:?}", head.blob);
                println!(
                    "\n== {title} ({alias})\n   asset {asset}\n   head  {}\n   blob  {}\n   text  {}",
                    head.revision,
                    &blob_hex[..blob_hex.len().min(24)],
                    fingerprint(&head.text)
                );
                by_blob.entry(head.blob).or_default().push(format!("{title} ({alias})"));
            }
            Err(error) => println!("\n== {title} ({alias})\n   UNREADABLE: {error}"),
        }
    }
    let mut clean = true;
    for (blob, owners) in &by_blob {
        if owners.len() > 1 {
            clean = false;
            println!("\nDUPLICATE HEAD SOURCE {blob:?}:\n   {}", owners.join("\n   "));
        }
    }
    if clean {
        println!("\nno two games share a head source blob");
    }
}

fn show(client: &mut AssetClient, asset: &str) {
    let asset = makepad_asset_data::AssetId::from_str(asset).expect("asset id");
    let detail = client.asset_detail(&asset).expect("detail");
    println!("asset {asset} retired={}", detail.retired);
    for candidate in &detail.candidates {
        let line = match client.fetch_asset_manifest(&candidate.revision) {
            Ok(manifest) => match manifest.files.iter().find(|f| f.role == FileRole::Source) {
                Some(file) => {
                    let bytes = client
                        .fetch_blob_bytes(&file.blob, Some(file.byte_len))
                        .unwrap_or_default();
                    format!(
                        "blob {:?} ({} bytes) — {}",
                        file.blob,
                        file.byte_len,
                        fingerprint(&String::from_utf8_lossy(&bytes))
                    )
                }
                None => "no source file".to_string(),
            },
            Err(error) => format!("manifest unreadable: {error}"),
        };
        println!(
            "  {:?} {} published_ms={:?}\n     {line}",
            candidate.state, candidate.revision, candidate.published_ms
        );
    }
}

fn repair(client: &mut AssetClient, namespace: &str, slug: &str, revision: &str) {
    let revision = makepad_asset_data::AssetRevisionId::from_str(revision).expect("revision id");
    let manifest = client.fetch_asset_manifest(&revision).expect("manifest of the good revision");
    let source = manifest
        .files
        .iter()
        .find(|f| f.role == FileRole::Source)
        .expect("good revision has a source");
    let bytes = client
        .fetch_blob_bytes(&source.blob, Some(source.byte_len))
        .expect("good source blob");
    println!(
        "restoring {namespace}/{slug} from {revision}\n  source blob {:?} ({} bytes)\n  {}",
        source.blob,
        bytes.len(),
        fingerprint(&String::from_utf8_lossy(&bytes))
    );
    // Title + description as currently listed; the picture carries over.
    let listed = games(client);
    let alias = games_import::game_alias(namespace, slug);
    let listed_row = listed.iter().find(|(_, _, a, _)| *a == alias);
    let title = listed_row.map(|(t, _, _, _)| t.clone()).unwrap_or_else(|| slug.to_string());
    let description = listed_row.map(|(_, d, _, _)| d.clone()).unwrap_or_default();
    let thumbnail = games_import::head_thumbnail(client, namespace, slug).expect("head thumbnail");
    let rights = makepad_asset_client::PublishRights::generated_cc0();
    games_import::publish_game_revision(
        client,
        namespace,
        slug,
        &title,
        &description,
        Some(bytes),
        thumbnail,
        &rights,
    )
    .expect("publish restored revision");
    println!("restored — new head published for {alias}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut client = connect();
    match args.first().map(String::as_str) {
        Some("audit") => audit(&mut client),
        Some("show") => show(&mut client, args.get(1).expect("show <asset_id>")),
        Some("repair") => repair(
            &mut client,
            args.get(1).expect("repair <namespace> <slug> <arev_...>"),
            args.get(2).expect("slug"),
            args.get(3).expect("revision"),
        ),
        _ => eprintln!("usage: repair_games audit | show <ast_...> | repair <ns> <slug> <arev_...>"),
    }
}
