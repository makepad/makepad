//! Internet Archive content input.
//!
//! Three layers, each usable on its own:
//!
//! * **wire** — [`SearchQuery`] builds an `advancedsearch` URL and
//!   [`parse_search`] reads its JSON; [`parse_item`] reads `/metadata/<id>`
//!   into an [`Item`] whose [`ItemFile`]s know which of them are playable
//!   video, real images, or archive housekeeping. [`license_from_url`]
//!   turns the item's Creative Commons URL into something a rights record
//!   can carry.
//! * **transport** — [`fetch_bytes`] (small bodies: JSON, thumbnails),
//!   [`download_to_file`] (streaming to disk, with progress) and
//!   [`RangeSource`] (byte ranges on demand over one kept-alive connection —
//!   what a player that never downloads uses) over HTTPS only, with
//!   bounded redirect following pinned to `*.archive.org`.
//! * **worker** — [`ArchiveWorker`]: a control lane for searches and item
//!   lookups, a small thumbnail pool, and one bulk download lane, all
//!   reporting through a channel the host polls on its own tick. Files land
//!   in a cache directory keyed by item + file name, so a second look at the
//!   same clip costs nothing.
//!
//! Nothing here imports into any store: the host takes the finished file
//! path and does that with the client it already has.

pub mod cache;
pub mod http;
pub mod item;
pub mod license;
pub mod range;
pub mod search;
pub mod url;
pub mod worker;

pub use cache::{cache_file_for, head_file_for, part_file_for, thumb_file_for};
pub use http::{download_head_to_file, download_to_file, fetch_bytes, Error, Progress, MAX_REDIRECTS};
pub use item::{parse_item, FileKind, FileSource, Item, ItemFile};
pub use license::{license_from_url, Grant, LicenseInfo};
pub use makepad_network::blocking_http::CancelToken;
pub use range::{RangeSource, MAX_RANGE_BYTES};
pub use search::{
    parse_search, ItemMediaType, MediaFilter, SearchHit, SearchPage, SearchQuery, SortOrder,
    MAX_ROWS,
};
pub use url::{details_url, download_url, identifier_key, is_valid_identifier, metadata_url, thumb_url};
pub use worker::{ArchiveWorker, Cmd, Ev, Purpose};

/// Largest thumbnail body the worker will accept (archive tiles are tens of
/// kilobytes; a megabyte is already suspicious).
pub const MAX_THUMB_BYTES: usize = 4 * 1024 * 1024;
/// Largest search / metadata JSON body. A 100-row search page is ~50 KB;
/// a big item's metadata (thousands of files) can run to a few MB.
pub const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
/// How much of a media file a PREVIEW pulls: the swatch plays while the
/// bytes land, so it only ever needs the head of the file — an hour-long
/// tape is auditioned from its first minutes, never refused for its size.
/// (A file smaller than this is fetched whole, under its normal cache
/// name, so a following IMPORT of the same file is a cache hit.)
pub const PREVIEW_HEAD_BYTES: u64 = 48 * 1024 * 1024;
/// Largest media file an import download will pull. Hosts upload the bytes
/// to their store from memory, so this is also their transient RAM cost.
pub const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;
