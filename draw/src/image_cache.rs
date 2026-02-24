use crate::makepad_platform::*;
use makepad_webp::WebPDecoder;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::{post_process_image, PngDecoder};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

pub use makepad_webp::DecodingError as WebpDecodeErrors;
pub use makepad_zune_jpeg::errors::DecodeErrors as JpgDecodeErrors;
pub use makepad_zune_png::error::PngDecodeErrors;

#[derive(Debug, Default, Clone)]
pub struct ImageBuffer {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
    pub animation: Option<TextureAnimation>,
}

impl ImageBuffer {
    pub fn new(in_data: &[u8], width: usize, height: usize) -> Result<ImageBuffer, ImageError> {
        let mut out = Vec::new();
        let pixels = width * height;
        out.resize(pixels, 0u32);
        match in_data.len() / pixels {
            4 => {
                for i in 0..pixels {
                    let r = in_data[i * 4];
                    let g = in_data[i * 4 + 1];
                    let b = in_data[i * 4 + 2];
                    let a = in_data[i * 4 + 3];
                    out[i] = ((a as u32) << 24)
                        | ((r as u32) << 16)
                        | ((g as u32) << 8)
                        | ((b as u32) << 0);
                }
            }
            3 => {
                for i in 0..pixels {
                    let r = in_data[i * 3];
                    let g = in_data[i * 3 + 1];
                    let b = in_data[i * 3 + 2];
                    out[i] =
                        0xff000000 | ((r as u32) << 16) | ((g as u32) << 8) | ((b as u32) << 0);
                }
            }
            2 => {
                for i in 0..pixels {
                    let r = in_data[i * 2];
                    let a = in_data[i * 2 + 1];
                    out[i] = ((a as u32) << 24)
                        | ((r as u32) << 16)
                        | ((r as u32) << 8)
                        | ((r as u32) << 0);
                }
            }
            1 => {
                for i in 0..pixels {
                    let r = in_data[i];
                    out[i] = ((0xff as u32) << 24)
                        | ((r as u32) << 16)
                        | ((r as u32) << 8)
                        | ((r as u32) << 0);
                }
            }
            unsupported => return Err(ImageError::InvalidPixelAlignment(unsupported)),
        }
        Ok(ImageBuffer {
            width,
            height,
            data: out,
            animation: None,
        })
    }

    pub fn into_new_texture(self, cx: &mut Cx) -> Texture {
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: self.width,
                height: self.height,
                data: Some(self.data),
                updated: TextureUpdated::Full,
            },
        );
        texture.set_animation(cx, self.animation);
        texture
    }

    pub fn from_png(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = PngDecoder::new(cursor);
        decoder.decode_headers()?;

        if decoder.is_animated() {
            return Self::decode_animated_png(&mut decoder);
        }

        let image = decoder.decode()?;
        let decoded_data =
            image
                .u8()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to decode PNG image data as a slice of u8 bytes",
                )))?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get PNG image dimensions",
                )))?;
        Self::new(&decoded_data, width, height)
    }

    fn decode_animated_png<T: makepad_zune_png::makepad_zune_core::bytestream::ZByteReaderTrait>(
        decoder: &mut PngDecoder<T>,
    ) -> Result<ImageBuffer, ImageError> {
        let colorspace =
            decoder
                .colorspace()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG colorspace",
                )))?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG image dimensions",
                )))?;
        let actl_info =
            decoder
                .actl_info()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG actl info",
                )))?;

        let num_components = colorspace.num_components();
        let mut output = vec![0; width * height * num_components];
        let fits_horizontal = Cx::max_texture_width() / width;
        let total_width = fits_horizontal * width;
        let total_height = ((actl_info.num_frames as usize / fits_horizontal) + 1) * height;
        let mut final_buffer = ImageBuffer::default();
        final_buffer.data.resize(total_width * total_height, 0);
        final_buffer.width = total_width;
        final_buffer.height = total_height;
        let mut cx = 0;
        let mut cy = 0;
        final_buffer.animation = Some(TextureAnimation {
            width,
            height,
            num_frames: actl_info.num_frames as usize,
        });
        let mut previous_frame = None;
        while decoder.more_frames() {
            decoder.decode_headers()?;
            let frame = decoder.frame_info().expect("to have already been decoded");
            let pix = decoder.decode_raw()?;
            let info =
                decoder
                    .info()
                    .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                        "Failed to get animated PNG image info",
                    )))?;
            post_process_image(
                &info,
                colorspace,
                &frame,
                &pix,
                previous_frame.as_deref(),
                &mut output,
                None,
            )?;
            previous_frame = Some(pix);
            match num_components {
                4 => {
                    for y in 0..height {
                        for x in 0..width {
                            let r = output[y * width * 4 + x * 4];
                            let g = output[y * width * 4 + x * 4 + 1];
                            let b = output[y * width * 4 + x * 4 + 2];
                            let a = output[y * width * 4 + x * 4 + 3];
                            final_buffer.data[(y + cy) * total_width + (x + cx)] = ((a as u32)
                                << 24)
                                | ((r as u32) << 16)
                                | ((g as u32) << 8)
                                | ((b as u32) << 0);
                        }
                    }
                }
                3 => {
                    for y in 0..height {
                        for x in 0..width {
                            let r = output[y * width * 3 + x * 3];
                            let g = output[y * width * 3 + x * 3 + 1];
                            let b = output[y * width * 3 + x * 3 + 2];
                            final_buffer.data[(y + cy) * total_width + (x + cx)] = 0xff000000
                                | ((r as u32) << 16)
                                | ((g as u32) << 8)
                                | ((b as u32) << 0);
                        }
                    }
                }
                _ => return Err(ImageError::InvalidPixelAlignment(num_components)),
            }
            cx += width;
            if cx >= total_width {
                cy += height;
                cx = 0;
            }
        }
        Ok(final_buffer)
    }

    pub fn from_webp(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = std::io::Cursor::new(data);
        let mut decoder =
            WebPDecoder::new(std::io::BufReader::new(cursor)).map_err(ImageError::WebpDecode)?;
        let (width, height) = decoder.dimensions();
        let buf_size = decoder
            .output_buffer_size()
            .ok_or(ImageError::WebpDecode(WebpDecodeErrors::ImageTooLarge))?;
        let mut buf = vec![0u8; buf_size];
        decoder
            .read_image(&mut buf)
            .map_err(ImageError::WebpDecode)?;
        Self::new(&buf, width as usize, height as usize)
    }

    pub fn from_jpg(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = JpegDecoder::new(cursor);
        match decoder.decode() {
            Ok(data) => {
                let info =
                    decoder
                        .info()
                        .ok_or(ImageError::JpgDecode(JpgDecodeErrors::FormatStatic(
                            "Failed to decode JPG image info",
                        )))?;
                ImageBuffer::new(&data, info.width as usize, info.height as usize)
            }
            Err(err) => Err(ImageError::JpgDecode(err)),
        }
    }
}

pub enum ImageCacheEntry {
    Loaded(Texture),
    Loading(usize, usize),
}

#[derive(Debug)]
pub struct AsyncImageLoad {
    pub image_path: PathBuf,
    pub result: RefCell<Option<Result<ImageBuffer, ImageError>>>,
}

pub struct ImageCache {
    pub map: HashMap<PathBuf, ImageCacheEntry>,
    pub thread_pool: Option<TagThreadPool<PathBuf>>,
    pub pending_http_requests: HashMap<LiveId, PathBuf>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            thread_pool: None,
            pending_http_requests: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub enum ImageError {
    EmptyData,
    InvalidPixelAlignment(usize),
    JpgDecode(JpgDecodeErrors),
    PathNotFound(PathBuf),
    PngDecode(PngDecodeErrors),
    WebpDecode(WebpDecodeErrors),
    UnsupportedFormat,
    Http(String),
}

pub enum AsyncLoadResult {
    Loading(usize, usize),
    Loaded,
}

impl Error for ImageError {}

impl From<PngDecodeErrors> for ImageError {
    fn from(value: PngDecodeErrors) -> Self {
        Self::PngDecode(value)
    }
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

fn image_decode_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("MAKEPAD_GLTF_TEX_DEBUG")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}

#[inline]
fn decode_timing_start() -> Option<Instant> {
    if !image_decode_debug_enabled() {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Instant::now())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn headless_mode_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MAKEPAD")
            .map(|value| value.eq_ignore_ascii_case("headless"))
            .unwrap_or(false)
    })
}

fn detect_image_format(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        Some("png")
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        Some("jpg")
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn detect_image_format_from_path_and_data(image_path: &Path, data: &[u8]) -> Option<&'static str> {
    // Prefer magic-byte detection over file extensions so in-memory/binary
    // resources decode correctly even when their synthetic path has no extension.
    if let Some(format) = detect_image_format(data) {
        return Some(format);
    }

    // Keep extension fallback for edge cases where headers are unavailable.
    let ext = image_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase());
    match ext.as_deref() {
        Some("jpg") | Some("jpeg") => Some("jpg"),
        Some("png") => Some("png"),
        Some("webp") => Some("webp"),
        _ => None,
    }
}

fn decode_image_buffer(image_path: &Path, data: &[u8]) -> Result<ImageBuffer, ImageError> {
    let format = detect_image_format_from_path_and_data(image_path, data)
        .ok_or(ImageError::UnsupportedFormat)?;
    match format {
        "jpg" => ImageBuffer::from_jpg(data),
        "png" => ImageBuffer::from_png(data),
        "webp" => ImageBuffer::from_webp(data),
        _ => Err(ImageError::UnsupportedFormat),
    }
}

fn image_size_by_data(data: &[u8], image_path: &Path) -> Result<(usize, usize), ImageError> {
    let format = detect_image_format_from_path_and_data(image_path, data)
        .ok_or(ImageError::UnsupportedFormat)?;
    match format {
        "jpg" => {
            let cursor = ZCursor::new(data);
            let mut decoder = JpegDecoder::new(cursor);
            decoder.decode_headers().map_err(ImageError::JpgDecode)?;
            let image_info = decoder.info().ok_or_else(|| {
                ImageError::JpgDecode(JpgDecodeErrors::FormatStatic(
                    "Failed to get JPG image info after decoding headers",
                ))
            })?;
            Ok((image_info.width as usize, image_info.height as usize))
        }
        "png" => {
            let cursor = ZCursor::new(data);
            let mut decoder = PngDecoder::new(cursor);
            decoder.decode_headers()?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::PngDecode(
                PngDecodeErrors::GenericStatic("Failed to get PNG image dimensions"),
            ))?;
            Ok((width, height))
        }
        "webp" => {
            let cursor = std::io::Cursor::new(data);
            let decoder = WebPDecoder::new(std::io::BufReader::new(cursor))
                .map_err(ImageError::WebpDecode)?;
            let (width, height) = decoder.dimensions();
            Ok((width as usize, height as usize))
        }
        _ => Err(ImageError::UnsupportedFormat),
    }
}

fn ensure_image_cache_inner(cx: &mut Cx) {
    if !cx.has_global::<ImageCache>() {
        cx.set_global(ImageCache::new());
    }
}

fn ensure_thread_pool(cx: &mut Cx) {
    ensure_image_cache_inner(cx);
    if cx.get_global::<ImageCache>().thread_pool.is_none() {
        let threads = cx.cpu_cores().max(3) - 2;
        cx.get_global::<ImageCache>().thread_pool = Some(TagThreadPool::new(cx, threads));
    }
}

fn spawn_decode_job(cx: &mut Cx, image_path: PathBuf, data: Arc<Vec<u8>>) {
    ensure_thread_pool(cx);
    let image_size_bytes = data.len();
    cx.get_global::<ImageCache>()
        .thread_pool
        .as_mut()
        .unwrap()
        .execute_rev(image_path, move |image_path| {
            let start = decode_timing_start();
            if image_decode_debug_enabled() {
                log!(
                    "ImageCache: decode_start key={} bytes={}",
                    image_path.display(),
                    image_size_bytes
                );
            }
            let result = decode_image_buffer(&image_path, &data);
            if image_decode_debug_enabled() {
                let status = match &result {
                    Ok(buffer) => format!("ok {}x{}", buffer.width, buffer.height),
                    Err(err) => format!("err {err}"),
                };
                if let Some(start) = start {
                    log!(
                        "ImageCache: decode_done key={} elapsed_ms={:.1} {}",
                        image_path.display(),
                        start.elapsed().as_secs_f64() * 1000.0,
                        status
                    );
                } else {
                    log!(
                        "ImageCache: decode_done key={} {}",
                        image_path.display(),
                        status
                    );
                }
            }
            Cx::post_action(AsyncImageLoad {
                image_path,
                result: RefCell::new(Some(result)),
            });
        });
}

pub fn ensure_image_cache(cx: &mut Cx) {
    ensure_image_cache_inner(cx);
}

pub fn process_async_image_load(
    cx: &mut Cx,
    image_path: &Path,
    result: Result<ImageBuffer, ImageError>,
) {
    ensure_image_cache_inner(cx);
    if let Ok(data) = result {
        let width = data.width;
        let height = data.height;
        let upload_start = decode_timing_start();
        let texture = data.into_new_texture(cx);
        if image_decode_debug_enabled() {
            if let Some(upload_start) = upload_start {
                log!(
                    "ImageCache: gpu_commit key={} elapsed_ms={:.1} size={}x{}",
                    image_path.display(),
                    upload_start.elapsed().as_secs_f64() * 1000.0,
                    width,
                    height
                );
            } else {
                log!(
                    "ImageCache: gpu_commit key={} size={}x{}",
                    image_path.display(),
                    width,
                    height
                );
            }
        }
        cx.get_global::<ImageCache>()
            .map
            .insert(image_path.into(), ImageCacheEntry::Loaded(texture));
    } else {
        if image_decode_debug_enabled() {
            log!(
                "ImageCache: gpu_commit key={} skipped (decode error)",
                image_path.display()
            );
        }
        cx.get_global::<ImageCache>().map.remove(image_path);
    }
}

pub fn load_image_from_cache(cx: &mut Cx, image_path: &Path) -> Option<Texture> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(texture)) => Some(texture.clone()),
        _ => None,
    }
}

pub fn load_image_from_data_async(
    cx: &mut Cx,
    image_path: &Path,
    data: Arc<Vec<u8>>,
) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(_)) => return Ok(AsyncLoadResult::Loaded),
        Some(ImageCacheEntry::Loading(w, h)) => return Ok(AsyncLoadResult::Loading(*w, *h)),
        None => {}
    }

    // On wasm, decode synchronously on the UI thread since thread pools
    // are not reliably available. Also decode synchronously for headless
    // single-frame runs so textured output is available in the first emitted PNG.
    #[cfg(target_arch = "wasm32")]
    let force_sync = true;
    #[cfg(not(target_arch = "wasm32"))]
    let force_sync = headless_mode_enabled();

    if force_sync {
        let image = decode_image_buffer(image_path, &data)?;
        let texture = image.into_new_texture(cx);
        cx.get_global::<ImageCache>()
            .map
            .insert(image_path.into(), ImageCacheEntry::Loaded(texture));
        return Ok(AsyncLoadResult::Loaded);
    }

    let (w, h) = image_size_by_data(&data, image_path)?;
    if image_decode_debug_enabled() {
        log!(
            "ImageCache: queue_decode key={} bytes={} size={}x{}",
            image_path.display(),
            data.len(),
            w,
            h
        );
    }
    cx.get_global::<ImageCache>()
        .map
        .insert(image_path.into(), ImageCacheEntry::Loading(w, h));
    spawn_decode_job(cx, image_path.to_path_buf(), data);
    Ok(AsyncLoadResult::Loading(w, h))
}

pub fn load_image_file_by_path_async(
    cx: &mut Cx,
    image_path: &Path,
) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(_)) => Ok(AsyncLoadResult::Loaded),
        Some(ImageCacheEntry::Loading(w, h)) => Ok(AsyncLoadResult::Loading(*w, *h)),
        None => match std::fs::read(image_path) {
            Ok(data) => load_image_from_data_async(cx, image_path, Arc::new(data)),
            Err(_) => Err(ImageError::PathNotFound(image_path.into())),
        },
    }
}

pub fn load_image_http_by_url_async(cx: &mut Cx, url: &str) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    let image_path = PathBuf::from(url);
    match cx.get_global::<ImageCache>().map.get(&image_path) {
        Some(ImageCacheEntry::Loaded(_)) => return Ok(AsyncLoadResult::Loaded),
        Some(ImageCacheEntry::Loading(w, h)) => return Ok(AsyncLoadResult::Loading(*w, *h)),
        None => {}
    }

    let request_id = LiveId::unique();
    cx.get_global::<ImageCache>()
        .map
        .insert(image_path.clone(), ImageCacheEntry::Loading(1, 1));
    cx.get_global::<ImageCache>()
        .pending_http_requests
        .insert(request_id, image_path);
    cx.http_request(
        request_id,
        HttpRequest::new(url.to_string(), HttpMethod::GET),
    );
    Ok(AsyncLoadResult::Loading(1, 1))
}

pub fn handle_image_cache_network_responses(cx: &mut Cx, e: &NetworkResponsesEvent) {
    if !cx.has_global::<ImageCache>() {
        return;
    }

    let mut decode_queue = Vec::<(PathBuf, Arc<Vec<u8>>)>::new();

    {
        let cache = cx.get_global::<ImageCache>();
        for response in e {
            match response {
                NetworkResponse::HttpError {
                    request_id,
                    error,
                } => {
                    let Some(image_path) = cache.pending_http_requests.remove(request_id) else {
                        continue;
                    };
                    error!(
                        "image http request failed for {:?}: {}",
                        image_path, error.message
                    );
                    cache.map.remove(&image_path);
                }
                NetworkResponse::HttpResponse {
                    request_id,
                    response,
                }
                | NetworkResponse::HttpStreamComplete {
                    request_id,
                    response,
                } => {
                    let Some(image_path) = cache.pending_http_requests.remove(request_id) else {
                        continue;
                    };
                    if !(200..300).contains(&response.status_code) {
                        cache.map.remove(&image_path);
                        continue;
                    }
                    if let Some(body) = &response.body {
                        cache.map.remove(&image_path);
                        decode_queue.push((image_path, Arc::new(body.clone())));
                    } else {
                        cache.map.remove(&image_path);
                    }
                }
                NetworkResponse::HttpProgress { .. }
                | NetworkResponse::HttpStreamChunk { .. }
                | NetworkResponse::WsOpened { .. }
                | NetworkResponse::WsMessage { .. }
                | NetworkResponse::WsClosed { .. }
                | NetworkResponse::WsError { .. } => {}
            }
        }
    }

    for (image_path, data) in decode_queue {
        let _ = load_image_from_data_async(cx, &image_path, data);
    }
}

pub trait ImageCacheImpl {
    fn get_texture(&self, id: usize) -> &Option<Texture>;
    fn set_texture(&mut self, texture: Option<Texture>, id: usize);

    fn lazy_create_image_cache(&mut self, cx: &mut Cx) {
        ensure_image_cache(cx);
    }

    fn load_png_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_png(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_jpg_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_jpg(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn process_async_image_load(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        result: Result<ImageBuffer, ImageError>,
    ) -> bool {
        process_async_image_load(cx, image_path, result);
        false
    }

    fn load_image_from_cache(&mut self, cx: &mut Cx, image_path: &Path, id: usize) -> bool {
        if let Some(texture) = load_image_from_cache(cx, image_path) {
            self.set_texture(Some(texture), id);
            true
        } else {
            false
        }
    }

    fn load_image_from_data_async_impl(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<Vec<u8>>,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        let result = load_image_from_data_async(cx, image_path, data)?;
        if matches!(result, AsyncLoadResult::Loaded) {
            let _ = self.load_image_from_cache(cx, image_path, id);
        }
        Ok(result)
    }

    fn load_image_file_by_path_async_impl(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        let result = load_image_file_by_path_async(cx, image_path)?;
        if matches!(result, AsyncLoadResult::Loaded) {
            let _ = self.load_image_from_cache(cx, image_path, id);
        }
        Ok(result)
    }

    fn load_image_http_by_url_async_impl(
        &mut self,
        cx: &mut Cx,
        url: &str,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        let result = load_image_http_by_url_async(cx, url)?;
        if matches!(result, AsyncLoadResult::Loaded) {
            let image_path = PathBuf::from(url);
            let _ = self.load_image_from_cache(cx, &image_path, id);
        }
        Ok(result)
    }

    fn load_image_file_by_path_and_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
        image_path: &Path,
    ) -> Result<(), ImageError> {
        let image = decode_image_buffer(image_path, data)?;
        let texture = image.into_new_texture(cx);
        ensure_image_cache(cx);
        cx.get_global::<ImageCache>()
            .map
            .insert(image_path.into(), ImageCacheEntry::Loaded(texture.clone()));
        self.set_texture(Some(texture), id);
        Ok(())
    }

    fn load_image_file_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<(), ImageError> {
        if let Some(texture) = load_image_from_cache(cx, image_path) {
            self.set_texture(Some(texture), id);
            return Ok(());
        }
        let data =
            std::fs::read(image_path).map_err(|_| ImageError::PathNotFound(image_path.into()))?;
        self.load_image_file_by_path_and_data(cx, &data, id, image_path)
    }

    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
        id: usize,
    ) -> Result<(), ImageError> {
        let p_image_path = Path::new(image_path);
        if let Some(texture) = load_image_from_cache(cx, p_image_path) {
            self.set_texture(Some(texture), id);
            return Ok(());
        }
        match cx.take_dependency(image_path) {
            Ok(data) => self.load_image_file_by_path_and_data(cx, &data, id, p_image_path),
            Err(_) => Err(ImageError::PathNotFound(image_path.into())),
        }
    }
}
