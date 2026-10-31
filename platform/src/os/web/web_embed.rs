//! The files a web export embeds in the wasm (`relax export-web`; format:
//! `makepad_web_manifest::embed`): a build with `--cfg makepad_embed` and
//! `MAKEPAD_EMBED=<file>` answers a GET or HEAD of an embedded URL itself
//! (`web_network`), and starts with the embed's query under the page's own
//! (`location.search`). Without the cfg nothing is embedded and every
//! request goes to the page.

use makepad_web_manifest::embed::EmbedRef;
use std::sync::OnceLock;

#[cfg(makepad_embed)]
static EMBED: &[u8] = include_bytes!(env!("MAKEPAD_EMBED"));
#[cfg(not(makepad_embed))]
static EMBED: &[u8] = &[];

/// The embed, read once (None: the build has none).
pub fn embedded() -> Option<&'static EmbedRef<'static>> {
    static READ: OnceLock<Option<EmbedRef<'static>>> = OnceLock::new();
    READ.get_or_init(|| {
        if EMBED.is_empty() {
            return None;
        }
        match EmbedRef::read(EMBED) {
            Ok(embed) => {
                crate::log!("web embed: {} files", embed.len());
                Some(embed)
            }
            Err(error) => {
                crate::error!("web embed: {error}");
                None
            }
        }
    })
    .as_ref()
}

/// The page's query with the embed's under it.
pub fn page_query(search: String) -> String {
    match embedded() {
        Some(embed) if !embed.query.is_empty() => makepad_web_manifest::embed::merge_query(&search, embed.query),
        _ => search,
    }
}
