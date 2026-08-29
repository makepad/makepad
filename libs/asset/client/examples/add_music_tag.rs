//! Add the `music` tag to VJ-imported audio annotations that predate it.
//!
//! The DJ deck explorer narrows to the music tag, and the VJ's own IMPORT
//! flow only started writing that tag on audio in August 2026 — a library
//! imported before then is invisible to the deck browser until each track's
//! annotation carries it. `put_annotation` REPLACES the whole annotation, so
//! this rewrites the exact shape `apps/vj/src/media_scan.rs` gave those
//! assets — kind audio, categories `audio, imported`, tags `local, music` —
//! with the per-asset text supplied in a TSV, because no read API returns a
//! full annotation to modify.
//!
//! TSV columns, tab-separated, one asset per line:
//!   <asset-id hex> <title> <description> <provenance>
//!
//!   cargo run --release -p makepad-asset-client --example add_music_tag -- \
//!       <control-addr> <data-addr> <token> <tsv-path> [--apply]
//!
//! Without `--apply` it only lists what it would rewrite.

use makepad_asset_client::{AnnotationUpload, ApiEndpoints, AssetClient, ClientConfig};
use makepad_asset_data::{AssetId, AssetKind};

fn parse_id(hex: &str) -> AssetId {
    assert_eq!(hex.len(), 32, "asset id is 16 bytes of hex: {hex}");
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex");
    }
    AssetId::from_bytes(bytes)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let control: std::net::SocketAddr = args.next().expect("control").parse().expect("control");
    let data: std::net::SocketAddr = args.next().expect("data").parse().expect("data");
    let token = args.next().expect("token");
    let tsv = std::fs::read_to_string(args.next().expect("tsv path")).expect("read tsv");
    let apply = args.next().as_deref() == Some("--apply");

    let mut config = ClientConfig::new(std::env::temp_dir().join("add-music-tag-cache"));
    config.token = Some(token);
    let client =
        AssetClient::connect(config, ApiEndpoints { control, data }, None).expect("connect");

    for line in tsv.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.split('\t');
        let id = parse_id(fields.next().expect("id"));
        let title = fields.next().expect("title").to_string();
        let description = fields.next().expect("description").to_string();
        let provenance = fields.next().expect("provenance").to_string();
        if !apply {
            println!("would tag {title}");
            continue;
        }
        let ann = AnnotationUpload {
            title: title.clone(),
            description,
            kind: Some(AssetKind::Audio),
            categories: vec!["imported".to_string(), "audio".to_string()],
            tags: vec!["local".to_string(), "music".to_string()],
            creator: String::new(),
            generator: "makepad-vj import".to_string(),
            backend: String::new(),
            model: String::new(),
            prompt: String::new(),
            provenance,
            private: false,
        };
        match client.put_annotation(&id, &ann) {
            Ok(()) => println!("tagged {title}"),
            Err(error) => println!("FAILED {title}: {error}"),
        }
    }
}
