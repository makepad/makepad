//! Rasterize the operating system color emoji font where supported.
use crate::content::PreviewImage;
#[cfg(feature = "emoji")]
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};
#[cfg(feature = "emoji")]
thread_local! { static CACHE: RefCell<BTreeMap<String, PreviewImage>> = const { RefCell::new(BTreeMap::new()) }; }

#[cfg(feature = "emoji")]
pub fn render(text: &str, size: f32) -> Result<PreviewImage, String> {
    if text.len() > 128 || !(8.0..=48.0).contains(&size) {
        return Err("Invalid emoji".into());
    }
    let key = format!("{text}:{size}");
    if let Some(image) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return Ok(image);
    }
    let (width, height, png) = raster(text, size)?;
    let image = PreviewImage {
        id: format!("emoji-{}.png", blake3::hash(&png).to_hex()),
        width,
        height,
        png: Arc::new(png),
    };
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(key, image.clone());
    });
    Ok(image)
}

#[cfg(not(feature = "emoji"))]
pub fn render(_: &str, _: f32) -> Result<PreviewImage, String> {
    Err("Color emoji support is disabled".into())
}

#[cfg(all(feature = "emoji", not(any(target_os = "macos", target_os = "ios"))))]
fn raster(_: &str, _: f32) -> Result<(u32, u32, Vec<u8>), String> {
    Err("Use platform emoji font".into())
}

#[cfg(all(feature = "emoji", any(target_os = "macos", target_os = "ios")))]
fn raster(text: &str, size: f32) -> Result<(u32, u32, Vec<u8>), String> {
    use core_foundation::{
        attributed_string::CFMutableAttributedString,
        base::{CFRange, CFType, TCFType},
        string::{CFString, CFStringRef},
    };
    #[cfg(feature = "emoji")]
    use std::{ffi::c_void, ptr};
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontCreateWithName(
            name: *const c_void,
            size: f64,
            matrix: *const c_void,
        ) -> *const c_void;
        static kCTFontAttributeName: CFStringRef;
        fn CTLineCreateWithAttributedString(string: *const c_void) -> *const c_void;
        fn CTLineGetTypographicBounds(
            line: *const c_void,
            ascent: *mut f64,
            descent: *mut f64,
            leading: *mut f64,
        ) -> f64;
        fn CTLineDraw(line: *const c_void, context: *mut c_void);
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGColorSpaceRelease(space: *mut c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits: usize,
            stride: usize,
            space: *mut c_void,
            info: u32,
        ) -> *mut c_void;
        fn CGContextSetTextPosition(context: *mut c_void, x: f64, y: f64);
        fn CGContextSetRGBFillColor(context: *mut c_void, r: f64, g: f64, b: f64, a: f64);
        fn CGContextRelease(context: *mut c_void);
    }
    unsafe {
        let name = CFString::new("AppleColorEmoji");
        let font = CTFontCreateWithName(name.as_CFTypeRef(), size as f64 * 2.0, ptr::null());
        if font.is_null() {
            return Err("Emoji font unavailable".into());
        }
        let font = CFType::wrap_under_create_rule(font);
        let string = CFString::new(text);
        let mut attributed = CFMutableAttributedString::new();
        attributed.replace_str(&string, CFRange::init(0, 0));
        attributed.set_attribute(
            CFRange::init(0, attributed.char_len()),
            kCTFontAttributeName,
            &font,
        );
        let line = CTLineCreateWithAttributedString(attributed.as_CFTypeRef());
        if line.is_null() {
            return Err("Emoji layout failed".into());
        }
        let line = CFType::wrap_under_create_rule(line);
        let (mut ascent, mut descent) = (0.0, 0.0);
        let advance = CTLineGetTypographicBounds(
            line.as_CFTypeRef(),
            &mut ascent,
            &mut descent,
            ptr::null_mut(),
        );
        let width = (advance.ceil() as u32 + 4).clamp(4, 512);
        let height = ((ascent + descent).ceil() as u32 + 4).clamp(4, 256);
        let mut rgba = vec![0; width as usize * height as usize * 4];
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            space,
            (4 << 12) | 1,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            return Err("Emoji bitmap failed".into());
        }
        CGContextSetRGBFillColor(context, 0.10, 0.10, 0.10, 1.0);
        CGContextSetTextPosition(context, 2.0, descent.ceil() + 2.0);
        CTLineDraw(line.as_CFTypeRef(), context);
        CGContextRelease(context);
        let pixmap =
            tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(width, height).unwrap())
                .ok_or("Emoji pixels missing")?;
        Ok((
            width.div_ceil(2),
            height.div_ceil(2),
            pixmap.encode_png().map_err(|e| e.to_string())?,
        ))
    }
}

#[cfg(all(test, feature = "emoji", target_os = "macos"))]
mod tests {
    #[test]
    fn emoji_contains_color_pixels() {
        let image = super::render("😃", 16.0).unwrap();
        let pixels = tiny_skia::Pixmap::decode_png(&image.png).unwrap();
        assert!(
            pixels
                .pixels()
                .iter()
                .filter(|p| p.red() > 150 && p.green() > 90 && p.blue() < 110 && p.alpha() > 200)
                .count()
                > 30
        );
    }
}
