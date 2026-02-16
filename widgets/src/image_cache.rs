use crate::makepad_draw::*;

#[derive(Clone, Copy, Debug, Default, Script, ScriptHook)]
pub enum ImageFit {
    #[default]
    Stretch,
    Horizontal,
    Vertical,
    Smallest,
    Biggest,
    Size,
}

pub use makepad_draw::{
    handle_image_cache_network_responses, load_image_file_by_path_async, load_image_from_cache,
    load_image_from_data_async, load_image_http_by_url_async, process_async_image_load,
    AsyncImageLoad, AsyncLoadResult, ImageBuffer, ImageCache, ImageCacheImpl, ImageError,
    JpgDecodeErrors, PngDecodeErrors,
};
