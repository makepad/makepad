use crate::makepad_draw::*;

#[derive(Clone, Copy, Debug, Default, Script, ScriptHook)]
pub enum ImageFit {
    /// Draws the image into the widget's requested bounds without preserving
    /// the image's aspect ratio.
    #[default]
    Stretch,
    /// Preserves the image's aspect ratio by keeping the requested width and
    /// changing the widget height to match the image.
    Horizontal,
    /// Preserves the image's aspect ratio by keeping the available height and
    /// changing the widget width to match the image.
    Vertical,
    /// Preserves the image's aspect ratio and resizes the widget so the whole
    /// image fits inside the requested bounds.
    Smallest,
    /// Preserves the image's aspect ratio and resizes the widget so the image
    /// fully covers the requested bounds. The widget may overflow its parent;
    /// any cropping depends on the parent layout and clipping.
    Biggest,
    /// Keeps the requested widget bounds, preserves the image's aspect ratio,
    /// centers the image, and crops the long axis in the shader.
    CropToFill,
    /// Ignores the requested widget bounds and uses the image's natural size.
    Size,
}

pub use makepad_draw::{
    decode_image_from_data, handle_image_cache_network_responses, image_size_by_data,
    looks_like_svg, load_image_file_by_path_async, load_image_from_cache, load_image_from_data_async,
    load_image_http_by_url_async, process_async_image_load, AsyncImageLoad, AsyncLoadResult,
    ImageBuffer, ImageCache, ImageCacheImpl, ImageError, JpgDecodeErrors, PngDecodeErrors,
};
