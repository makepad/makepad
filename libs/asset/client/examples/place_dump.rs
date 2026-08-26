//! Dump an asset's manifest file roles, and the text of any text-ish blob —
//! for reading a world's `.place` sidecar out of a live store.
//!
//!   cargo run --release -p makepad-asset-client --example place_dump -- \
//!       <control> <data> <token> <alias>

use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig};

fn main() {
    let mut a = std::env::args().skip(1);
    let control: std::net::SocketAddr = a.next().unwrap().parse().unwrap();
    let data: std::net::SocketAddr = a.next().unwrap().parse().unwrap();
    let token = a.next().unwrap();
    let alias = a.next().unwrap();

    let mut cfg = ClientConfig::new(std::env::temp_dir().join("place-dump-cache"));
    cfg.token = Some(token);
    let mut c = AssetClient::connect(cfg, ApiEndpoints { control, data }, None).expect("connect");

    let parsed: makepad_asset_data::AssetAlias = alias.parse().expect("alias");
    let id = c.resolve_alias(&parsed).expect("resolve").asset_id;
    let detail = c.asset_detail(&id).expect("detail");
    let head = detail.latest_published().expect("published");
    let m = c.fetch_asset_manifest(&head.revision).expect("manifest");
    println!("{} — {} file(s)", alias, m.files.len());
    for f in &m.files {
        println!("  role={:?} media={:?} bytes={}", f.role, f.media, f.byte_len);
        if matches!(f.media, makepad_asset_data::MediaType::Text) {
            match c.blob_path(&f.blob, Some(f.byte_len)) {
                Ok(p) => match std::fs::read_to_string(&p) {
                    Ok(t) => println!("---- text ----\n{t}\n---- end ----"),
                    Err(e) => println!("    read failed: {e}"),
                },
                Err(e) => println!("    blob failed: {e}"),
            }
        }
    }
}
