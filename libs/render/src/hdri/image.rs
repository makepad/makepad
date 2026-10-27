//! Image files for the HDRI generator: the sRGB curves, the quarter-width column roll between
//! the engine's equirect convention and the file convention, and readers for OpenEXR, the 8-bit
//! formats the image cache decodes, and Radiance `.hdr` (through `ibl::load_hdr`). Writers live
//! in `export.rs`.
//!
//! Everything here is linear Rec.709 radiance in an `EnvMap`. Alpha is 1 but for the pictures of
//! image lights (1b): `decode_image` keeps an 8-bit file's alpha and an EXR's A channel, straight.
//! An environment (`decode_equirect`, `load_equirect`) is the colour its file stores, opaque: only
//! a light's picture uses alpha (the plan owner's D13). sRGB appears only where 8-bit files come in.
//! `load_image` and `load_equirect` read only a regular file of at most `MAX_IMAGE_FILE_BYTES`,
//! and a network path only when the person picked it, not when a preset names it (`PathOrigin`,
//! D5).
//! Nothing here samples, resizes or decodes RGBE itself: `EnvMap` and `ibl` already do that.
//!
//! Two conventions meet here. The engine (`ibl::dir_to_equirect_uv`) puts −Z at the image centre
//! and +X at u = 0.75; files (Blender, Unity, three.js, DrawPbr) put +X at the centre. They differ
//! by exactly a quarter turn, `u_file = u_engine − 0.25`, so a file is a column roll of the
//! engine map. `decode_image` and `read_exr` return the file's columns untouched (the file
//! convention); `load_equirect` rolls them into the engine convention, and the writers in
//! `export.rs` roll the other way.

use makepad_draw::*;
use makepad_draw::makepad_platform::resource_resolver::DecodeBudget;
use makepad_openexr::{read_from_slice, ExrPart, SampleBuffer};
use makepad_render_material::ibl::{self, EnvMap};
use std::path::Path;
use super::vec;

/// The exact sRGB decoding curve (IEC 61966-2-1), for 8-bit colours coming into the linear model.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The exact sRGB encoding curve, the inverse of `srgb_to_linear`. It does not clamp; callers
/// clamp to 0..1 first when they want a displayable value.
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Lossless quarter-width column roll between the engine convention (−Z at the centre, +X at
/// u = 0.75) and the file convention (+X at the centre). `to_file` rolls engine → file
/// (`u_file = u − 0.25`); otherwise file → engine. Exact when `width % 4 == 0` (every size the
/// app bakes, 512 up to 8192); other widths are resampled bilinearly at the texel centres, which
/// blurs by up to half a texel and is the only lossy path. Any height works: the roll only moves
/// columns, so it also serves files that are a pixel off 2:1.
pub fn roll_quarter(env: &EnvMap, to_file: bool) -> EnvMap {
    let (width, height) = (env.width, env.height);
    if width == 0 || height == 0 || env.data.len() < width * height {
        return env.clone();
    }
    let mut data = Vec::with_capacity(width * height);
    if width % 4 == 0 {
        // Engine column x holds u = (x + 0.5) / w. That content sits a quarter turn to the left
        // in the file, so file column x reads engine column x + w/4; the way back reads x − w/4
        // (written as x + 3w/4 to stay unsigned). Both wrap at the seam.
        let shift = if to_file { width / 4 } else { width - width / 4 };
        for y in 0..height {
            let row = &env.data[y * width..(y + 1) * width];
            for x in 0..width {
                data.push(row[(x + shift) % width]);
            }
        }
    } else {
        // A fractional shift: read each output texel centre a quarter turn away. `sample_uv`
        // wraps u, so the seam needs no special case.
        let du = if to_file { 0.25 } else { -0.25 };
        for y in 0..height {
            let v = (y as f32 + 0.5) / height as f32;
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32 + du;
                data.push(env.sample_uv([u, v]));
            }
        }
    }
    EnvMap { width, height, data }
}

/// Luminance of the map's solid-angle-weighted mean radiance: the auto-exposure meter. An
/// empty map meters 0 rather than NaN.
pub fn mean_luminance(env: &EnvMap) -> f32 {
    if env.width == 0 || env.height == 0 || env.data.len() < env.width * env.height {
        return 0.0;
    }
    let l = crate::sky::luminance(vec(env.mean()));
    if l.is_finite() {
        l
    } else {
        0.0
    }
}

/// Radiance read from or written to a float file. NaN and negatives become 0. +∞ becomes 65504,
/// half's largest value, which is where an infinity in a half file usually came from.
pub(crate) fn sanitize_radiance(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else if v == f32::INFINITY {
        65504.0
    } else {
        v.max(0.0)
    }
}

/// OpenEXR's magic number 20000630, little-endian.
const EXR_MAGIC: [u8; 4] = [0x76, 0x2f, 0x31, 0x01];
/// The most texels a file may decode to (16 B each, so 1 GiB): the same cap `ibl::load_hdr` has.
const MAX_PIXELS: usize = 64 * 1024 * 1024;

/// The texel count of a `width` x `height` image, checked against `MAX_PIXELS` before anything
/// sized by it is allocated. Every reader goes through here: the 8-bit path widens each 4-byte
/// texel to 16 bytes, so an unchecked 16384 x 16384 PNG (the image cache's own ceiling) would ask
/// for a 4 GiB `Vec` on top of its 1 GiB decode. The error has no prefix; callers add theirs.
fn checked_texels(width: usize, height: usize) -> Result<usize, String> {
    match width.checked_mul(height) {
        Some(0) => Err(format!("{width}x{height} has no texels")),
        Some(count) if count <= MAX_PIXELS => Ok(count),
        _ => Err(format!("{width}x{height} is over the 64 Mpx limit")),
    }
}

/// What a decode does with a file's alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Alpha {
    /// Kept, straight, in the fourth channel: a light's picture (1b).
    Straight,
    /// Left out: the colour the file stores, opaque. An environment (D13): an EXR texel stored
    /// premultiplied with A = 0 is light all the same (a renderer's camera-visible sky), and
    /// dividing a low alpha out would cap a bright texel at 65504.
    Ignored,
}

/// Reads an OpenEXR through makepad-openexr. It takes the first part with R, G and B channels or,
/// failing that, a luminance-only Y part read as grey; samples may be half, float or uint. An "A"
/// channel in that part is read as straight alpha (1b). The image is the part's data window,
/// whatever its origin; the display window is ignored. The columns are returned as the file
/// holds them (the file convention, +X at the centre).
/// The reader handles scanline files with no, ZIP, ZIPS or PXR24 compression. PIZ, DWA and tiled
/// files fail with a message that points at `.hdr`, which always imports.
pub fn read_exr(bytes: &[u8]) -> Result<EnvMap, String> {
    read_exr_with(bytes, Alpha::Straight)
}

/// `read_exr`, with the alpha kept straight or left out.
fn read_exr_with(bytes: &[u8], alpha_use: Alpha) -> Result<EnvMap, String> {
    let exr = read_from_slice(bytes).map_err(|e| {
        format!("EXR: {e} (scanline EXRs with no, ZIP, ZIPS or PXR24 compression load; for PIZ, DWA or tiled files use .hdr)")
    })?;
    let mut parts = exr.parts;
    let rgb_part = parts
        .iter()
        .position(|p| has_channel(p, "R") && has_channel(p, "G") && has_channel(p, "B"));
    let index = match rgb_part.or_else(|| parts.iter().position(|p| has_channel(p, "Y"))) {
        Some(index) => index,
        None => {
            let names: Vec<&str> = parts
                .iter()
                .flat_map(|p| p.channels.iter().map(|c| c.name.as_str()))
                .collect();
            return Err(format!("EXR: no R, G, B (or Y) channels; found {}", names.join(", ")));
        }
    };
    let mut part = parts.swap_remove(index);
    let width = part.width().map_err(|e| format!("EXR: {e}"))?;
    let height = part.height().map_err(|e| format!("EXR: {e}"))?;
    let count = checked_texels(width, height).map_err(|e| format!("EXR: {e}"))?;
    let planes = if rgb_part.is_some() {
        [take_channel(&mut part, "R"), take_channel(&mut part, "G"), take_channel(&mut part, "B")]
    } else {
        let y = take_channel(&mut part, "Y");
        [y.clone(), y.clone(), y]
    };
    if planes.iter().any(|plane| plane.len() != count) {
        return Err(format!("EXR: channel sizes do not match the {width}x{height} data window"));
    }
    // Alpha (1b, B14): an "A" channel in the same part, for the pictures of
    // image lights. OpenEXR stores colour premultiplied by it; the pictures
    // want it straight, as the 8-bit decodes give it, so the colour is divided
    // back out where a texel is partly covered. An environment keeps the
    // stored colour and no alpha (D13). A file without A (or with an A of
    // another size) is opaque.
    let alpha = if alpha_use == Alpha::Straight { take_channel(&mut part, "A") } else { Vec::new() };
    let alpha = if alpha.len() == count { alpha } else { Vec::new() };
    let data = (0..count)
        .map(|i| {
            let a = alpha.get(i).map_or(1.0, |&a| if a.is_finite() { a.clamp(0.0, 1.0) } else { 1.0 });
            let straight = |c: f32| if a > 0.0 && a < 1.0 { (c / a).min(65504.0) } else { c };
            [
                straight(sanitize_radiance(planes[0][i])),
                straight(sanitize_radiance(planes[1][i])),
                straight(sanitize_radiance(planes[2][i])),
                a,
            ]
        })
        .collect();
    Ok(EnvMap { width, height, data })
}

fn has_channel(part: &ExrPart, name: &str) -> bool {
    part.channels.iter().any(|c| c.name == name)
}

/// Moves one channel's samples out of the part as f32 (empty when the channel is missing).
fn take_channel(part: &mut ExrPart, name: &str) -> Vec<f32> {
    let Some(channel) = part.channels.iter_mut().find(|c| c.name == name) else {
        return Vec::new();
    };
    match std::mem::replace(&mut channel.samples, SampleBuffer::Float(Vec::new())) {
        SampleBuffer::Float(values) => values,
        SampleBuffer::Half(values) => values.into_iter().map(|v| v.to_f32()).collect(),
        SampleBuffer::Uint(values) => values.into_iter().map(|v| v as f32).collect(),
    }
}

/// Decodes an image file from memory by its magic bytes, returning the FILE convention (the
/// columns as the file holds them):
/// - OpenEXR (76 2f 31 01) through `read_exr`;
/// - Radiance (`#?`) through `ibl::load_hdr`, with a fresh 1 GiB `DecodeBudget` (the app has no
///   document budget to charge; `load_hdr`'s own size caps still apply);
/// - anything else `decode_image_from_data` knows (PNG, JPEG, WebP, GIF, BMP, QOI, ICO). Its
///   8-bit sRGB values are linearised with the exact sRGB curve, and alpha is kept in the fourth
///   channel (opaque files read 1.0). Animated files are refused, and so is anything over the
///   64 Mpx cap (as for EXR).
pub fn decode_image(bytes: &[u8]) -> Result<EnvMap, String> {
    decode_with(bytes, Alpha::Straight)
}

/// `decode_image`, with the alpha kept straight or left out (an environment: the RGB the file
/// stores, opaque).
fn decode_with(bytes: &[u8], alpha: Alpha) -> Result<EnvMap, String> {
    if bytes.starts_with(&EXR_MAGIC) {
        return read_exr_with(bytes, alpha);
    }
    if bytes.starts_with(b"#?") {
        return ibl::load_hdr(bytes, &mut DecodeBudget::default()).map_err(|e| format!("HDR: {e}"));
    }
    let buffer = decode_image_from_data(bytes).map_err(|e| format!("image: {e}"))?;
    // An animated GIF, APNG or WebP comes back as one buffer holding every frame in a grid (the
    // atlas), so its width and height cover all the frames: it would load as a nonsense map.
    if buffer.animation.is_some() {
        return Err("image: animated images are not supported (use a still image)".to_string());
    }
    let count = checked_texels(buffer.width, buffer.height).map_err(|e| format!("image: {e}"))?;
    if buffer.data.len() < count {
        return Err(format!("image: decoded {}x{} but got {} texels", buffer.width, buffer.height, buffer.data.len()));
    }
    // 256 entries cover every 8-bit value, so each texel costs three table reads instead of three powf.
    let linear: [f32; 256] = std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0));
    // The image cache packs 0xAARRGGBB.
    let data = buffer.data[..count]
        .iter()
        .map(|&packed| {
            [
                linear[((packed >> 16) & 0xff) as usize],
                linear[((packed >> 8) & 0xff) as usize],
                linear[(packed & 0xff) as usize],
                // Straight (not premultiplied) alpha, as the decoder packs it:
                // an image light may use it to cut the light out (B14). An
                // environment is opaque in the colour it stores (D13).
                if alpha == Alpha::Straight { (packed >> 24) as f32 / 255.0 } else { 1.0 },
            ]
        })
        .collect();
    Ok(EnvMap { width: buffer.width, height: buffer.height, data })
}

/// The largest picture or environment file `load_image` and `load_equirect` read (D5): 1 GiB,
/// what the decoded cap (`MAX_PIXELS` at 16 B a texel) holds. The size is checked from the open
/// file before anything is read, and the read stops one byte past it.
pub const MAX_IMAGE_FILE_BYTES: u64 = 1 << 30;

/// Where a path to read came from, which decides how far it is trusted (the plan owner's D5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOrigin {
    /// The person picked the file in a file dialog, or the host chose the path itself.
    Picked,
    /// A loaded preset or settings file names it, and someone else may have written that file.
    /// A network path (`\\host\share\...`) is not opened: opening one connects to that host as
    /// soon as the preset loads. The person can still pick such a file themselves.
    Preset,
}

/// What a path names, read from its spelling alone, before anything touches the file system.
/// The same on every system: a preset written on Windows names Windows paths wherever it loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathKind {
    Local,
    /// `\\host\share\...` (either separator), `\\?\UNC\host\share\...` or `\??\UNC\host\...`.
    Network,
    /// The Win32 device namespaces: `\\.\...` (`\\.\COM1`, `\\.\PhysicalDrive0`), and `\\?\`
    /// or `\??\` with anything but a drive or `UNC` (`\??\GLOBALROOT\Device\Mup\...`, the
    /// network's own redirector). Never a picture.
    Device,
}

fn path_kind(path: &str) -> PathKind {
    let bytes = path.as_bytes();
    let sep = |i: usize| bytes.get(i).is_some_and(|&c| c == b'/' || c == b'\\');
    // `\\?\` and `\??\` (the NT object manager's own prefix) both hand what follows to Windows
    // verbatim, so they are read alike: a drive is local, `UNC\` the network, anything else a
    // device. Either separator, as for `\\host`: refusing a spelling no picture has costs nothing.
    let verbatim = sep(0) && bytes.get(2) == Some(&b'?') && sep(3) && (sep(1) || bytes.get(1) == Some(&b'?'));
    if verbatim {
        let rest = &bytes[4..];
        return if rest.len() >= 2 && rest[0].is_ascii_alphabetic() && rest[1] == b':' {
            // \\?\C:\..., a disk path spelled verbatim.
            PathKind::Local
        } else if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"UNC") && sep(7) {
            PathKind::Network
        } else {
            PathKind::Device
        };
    }
    if !(sep(0) && sep(1)) {
        return PathKind::Local;
    }
    match bytes.get(2) {
        Some(b'.') if sep(3) => PathKind::Device,
        _ => PathKind::Network,
    }
}

/// Why `path` is not opened, decided from its spelling alone (nothing touches the file system,
/// so a refused network path never reaches its host): a device path, whoever names it, and a
/// network path that a preset names (D5). None when it may be opened.
pub fn refused_path(path: &Path, origin: PathOrigin) -> Option<String> {
    let text = path.to_string_lossy();
    match (path_kind(&text), origin) {
        (PathKind::Local, _) | (PathKind::Network, PathOrigin::Picked) => None,
        (PathKind::Network, PathOrigin::Preset) => Some(format!(
            "{}: a network path named by a preset is not opened; pick the file yourself to use it",
            path.display()
        )),
        (PathKind::Device, _) => Some(format!("{}: a device path, not a picture file", path.display())),
    }
}

/// Reads the whole of a regular file of at most `cap` bytes that `origin` lets be opened
/// (`refused_path`). A folder, a device (`/dev/zero`, `NUL`, `COM1`) or a pipe is refused before
/// it is opened, from its metadata (opening a pipe can block; reading `/dev/zero` never ends),
/// and again from the open handle. The length is checked before the read, and the read stops one
/// byte past the cap, so a file that grows while it is read is refused too. Errors name the path.
fn read_image_file(path: &Path, origin: PathOrigin, cap: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if let Some(refusal) = refused_path(path, origin) {
        return Err(refusal);
    }
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let not_a_file = || named(&"not a regular file");
    let regular = |meta: &std::fs::Metadata| -> Result<(), String> {
        if !meta.is_file() {
            return Err(not_a_file());
        }
        if meta.len() > cap {
            return Err(named(&format!("{} bytes is over the {cap} byte limit for a picture", meta.len())));
        }
        Ok(())
    };
    match std::fs::metadata(path) {
        Ok(meta) => regular(&meta)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(named(&e)),
        // A Windows device (NUL, CON, COM1) has no file metadata at all.
        Err(e) => return Err(named(&format!("not a regular file ({e})"))),
    }
    let file = std::fs::File::open(path).map_err(|e| named(&e))?;
    // The path may have been swapped for something else since: the handle is what is read.
    regular(&file.metadata().map_err(|e| named(&format!("not a regular file ({e})")))?)?;
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| named(&e))?;
    if bytes.len() as u64 > cap {
        return Err(named(&format!("grew past the {cap} byte limit for a picture while it was read")));
    }
    Ok(bytes)
}

/// Reads and decodes a file (file convention): a regular file of at most `MAX_IMAGE_FILE_BYTES`,
/// and no network path when a preset names it (`PathOrigin`, D5). Errors name the path.
pub fn load_image(path: &Path, origin: PathOrigin) -> Result<EnvMap, String> {
    let bytes = read_image_file(path, origin, MAX_IMAGE_FILE_BYTES)?;
    decode_image(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Like `decode_image`, but only accepts an equirect map, twice as wide as it is tall (one pixel
/// off is tolerated: cropped downloads are common), and rolls it from the file convention into
/// the engine convention, ready for `Env`, `ibl::ibl_texture` and `Renderer::register_environment`.
/// An environment is the colour its file stores, opaque: its alpha is not read (D13; only a
/// light's picture uses alpha). For a file already in memory (the web's file dialog, a host's
/// bounded read); `load_equirect` reads one from disk.
pub fn decode_equirect(bytes: &[u8]) -> Result<EnvMap, String> {
    let file = decode_with(bytes, Alpha::Ignored)?;
    if (file.width as i64 - 2 * file.height as i64).abs() > 1 {
        return Err(format!(
            "{}x{} is not an equirect map; it must be twice as wide as it is tall (2:1)",
            file.width, file.height
        ));
    }
    Ok(roll_quarter(&file, false))
}

/// `decode_equirect` of a file, read as `load_image` reads one (`MAX_IMAGE_FILE_BYTES`, a regular
/// file, no network path a preset names); errors name the path.
pub fn load_equirect(path: &Path, origin: PathOrigin) -> Result<EnvMap, String> {
    let bytes = read_image_file(path, origin, MAX_IMAGE_FILE_BYTES)?;
    decode_equirect(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_half::f16;
    use makepad_openexr::{write_to_vec, Box2i, Compression, ExrChannel, ExrImage, ExrPart};

    /// Texel (x, y) holds (x, y, 1, 1), so a texel shows where it came from.
    fn ramp(width: usize, height: usize) -> EnvMap {
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                data.push([x as f32, y as f32, 1.0, 1.0]);
            }
        }
        EnvMap { width, height, data }
    }

    fn texel(env: &EnvMap, x: usize, y: usize) -> [f32; 4] {
        env.data[y * env.width + x]
    }

    #[test]
    fn srgb_curves_are_exact_and_inverse() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1.0e-6);
        assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 1.0e-6);
        assert!((linear_to_srgb(0.18) - 0.461_356).abs() < 1.0e-4);
        // The linear and power pieces meet at the knee, in both directions.
        assert!((srgb_to_linear(0.04045) - srgb_to_linear(0.040_451)).abs() < 1.0e-6);
        assert!((linear_to_srgb(0.003_130_8) - linear_to_srgb(0.003_131)).abs() < 1.0e-5);
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            assert!((linear_to_srgb(srgb_to_linear(x)) - x).abs() < 1.0e-5, "{x}");
        }
    }

    #[test]
    fn roll_quarter_is_an_exact_column_roll_for_widths_divisible_by_four() {
        let engine = ramp(8, 4);
        let file = roll_quarter(&engine, true);
        assert_eq!((file.width, file.height), (8, 4));
        // The engine puts +X at u = 0.75: column 6 of 8. In the file it is the centre column, 4.
        let plus_x = ibl::dir_to_equirect_uv([1.0, 0.0, 0.0]);
        assert_eq!((plus_x[0] * 8.0).floor() as usize, 6);
        assert_eq!(texel(&file, 4, 1), texel(&engine, 6, 1));
        // North (−Z), the engine's centre column 4, lands at u = 0.25 in the file: column 2.
        assert_eq!(texel(&file, 2, 3), texel(&engine, 4, 3));
        // Every column moved by exactly a quarter, wrapping at the seam.
        for y in 0..4 {
            for x in 0..8 {
                assert_eq!(texel(&file, x, y), texel(&engine, (x + 2) % 8, y), "({x},{y})");
            }
        }
        // The way back is the inverse, bit for bit, in both orders.
        assert_eq!(roll_quarter(&file, false), engine);
        assert_eq!(roll_quarter(&roll_quarter(&engine, false), true), engine);
        // An empty map rolls to an empty map instead of panicking.
        let empty = EnvMap { width: 0, height: 0, data: Vec::new() };
        assert_eq!(roll_quarter(&empty, true), empty);
    }

    #[test]
    fn roll_quarter_resamples_other_widths_and_keeps_the_energy() {
        // 6 columns: a quarter turn is 1.5 texels, so the roll is a bilinear resample.
        let mut engine = ramp(6, 2);
        // A smooth ring pattern; ramp's x would jump from 5 to 0 at the seam.
        for (i, t) in engine.data.iter_mut().enumerate() {
            let x = (i % 6) as f32;
            *t = [(x * std::f32::consts::TAU / 6.0).sin() + 2.0, 1.0, 0.5, 1.0];
        }
        let file = roll_quarter(&engine, true);
        assert_eq!((file.width, file.height), (6, 2));
        // File column x reads the engine at u + 0.25: halfway between engine columns x+1 and x+2.
        for x in 0..6 {
            let want = engine.sample_uv([(x as f32 + 0.5) / 6.0 + 0.25, 0.25]);
            assert_eq!(texel(&file, x, 0), want, "column {x}");
            let mid = [texel(&engine, (x + 1) % 6, 0)[0], texel(&engine, (x + 2) % 6, 0)[0]];
            assert!((texel(&file, x, 0)[0] - (mid[0] + mid[1]) * 0.5).abs() < 1.0e-6);
        }
        // A wrap-around linear resample moves energy around; it neither creates nor loses any.
        let sum = |e: &EnvMap| e.data.iter().map(|t| t[0] as f64).sum::<f64>();
        assert!((sum(&file) - sum(&engine)).abs() < 1.0e-4);
        // Alpha stays 1 and the other channels stay flat.
        assert!(file.data.iter().all(|t| t[1] == 1.0 && t[2] == 0.5 && t[3] == 1.0));
        // The way back is also a resample; it lands within a texel's blur of the source.
        let back = roll_quarter(&file, false);
        for x in 0..6 {
            assert!((texel(&back, x, 0)[0] - texel(&engine, x, 0)[0]).abs() < 0.5, "column {x}");
        }
    }

    #[test]
    fn mean_luminance_meters_the_solid_angle_mean() {
        let grey = EnvMap::constant(16, [0.5, 0.5, 0.5]);
        assert!((mean_luminance(&grey) - 0.5).abs() < 1.0e-5);
        // Rec.709 weights: pure green weighs 0.7152.
        let green = EnvMap::constant(16, [0.0, 1.0, 0.0]);
        assert!((mean_luminance(&green) - 0.7152).abs() < 1.0e-5);
        // A bright top row counts less than a bright equator row: the meter is solid-angle weighted.
        let mut top = EnvMap::constant(16, [0.0, 0.0, 0.0]);
        let mut equator = top.clone();
        for x in 0..16 {
            top.data[x] = [10.0, 10.0, 10.0, 1.0];
            equator.data[4 * 16 + x] = [10.0, 10.0, 10.0, 1.0];
        }
        assert!(mean_luminance(&top) < mean_luminance(&equator) * 0.5);
        // An empty map meters 0 rather than NaN.
        assert_eq!(mean_luminance(&EnvMap { width: 0, height: 0, data: Vec::new() }), 0.0);
    }

    /// A fresh folder under the system temp dir, unique to this process and test.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("makepad_hdri_image_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Mid-grey (0.5) in RGBE: 128 · 2^(128 − 136).
    const GREY: [u8; 4] = [128, 128, 128, 128];

    /// A flat (not run-length coded) .hdr whose texel (x, y) is `texel(x, y)` in RGBE bytes. The
    /// first texel of a row must not start with (2, 2), or the reader takes the row for RLE.
    fn flat_hdr(width: usize, height: usize, texel: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut bytes = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&texel(x, y));
            }
        }
        bytes
    }

    #[test]
    fn read_exr_reads_rgb_from_any_data_window() {
        let r: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let g: Vec<f32> = (0..8).map(|i| i as f32 * 0.5).collect();
        let b: Vec<f32> = (0..8).map(|i| 100.0 + i as f32).collect();
        let mut part = ExrPart::new(None, 4, 2, Compression::None, vec![
            ExrChannel::float("B", b.clone()),
            ExrChannel::float("G", g.clone()),
            ExrChannel::float("R", r.clone()),
        ]);
        // Off-origin data windows are common (Blobbies.exr starts at -20); the image is the window.
        part.data_window = Box2i { min_x: -20, min_y: 5, max_x: -17, max_y: 6 };
        let env = read_exr(&write_to_vec(&ExrImage::single(part)).unwrap()).unwrap();
        assert_eq!((env.width, env.height), (4, 2));
        assert_eq!(texel(&env, 1, 1), [r[5], g[5], b[5], 1.0]);
        assert_eq!(texel(&env, 3, 0), [r[3], g[3], b[3], 1.0]);
        // The file's columns come back as they are: no roll here.
        assert_eq!(texel(&env, 0, 0), [r[0], g[0], b[0], 1.0]);
    }

    #[test]
    fn read_exr_reads_half_and_luminance_only_files() {
        let half = |values: &[f32]| values.iter().map(|&v| f16::from_f32(v)).collect::<Vec<f16>>();
        let part = ExrPart::new(None, 2, 1, Compression::Zip, vec![
            ExrChannel::half("R", half(&[1.0, 0.25])),
            ExrChannel::half("G", half(&[2.0, 0.5])),
            ExrChannel::half("B", half(&[4.0, 0.75])),
        ]);
        let env = read_exr(&write_to_vec(&ExrImage::single(part)).unwrap()).unwrap();
        assert_eq!(texel(&env, 0, 0), [1.0, 2.0, 4.0, 1.0]);
        assert_eq!(texel(&env, 1, 0), [0.25, 0.5, 0.75, 1.0]);
        // NaN and negatives are not radiance: they read as 0. +∞ reads as half's largest value.
        let odd = ExrPart::new(None, 3, 1, Compression::None, vec![
            ExrChannel::float("R", vec![f32::NAN, -1.0, f32::INFINITY]),
            ExrChannel::float("G", vec![0.0, 0.0, 0.0]),
            ExrChannel::float("B", vec![0.0, 0.0, 0.0]),
        ]);
        let env = read_exr(&write_to_vec(&ExrImage::single(odd)).unwrap()).unwrap();
        assert_eq!([texel(&env, 0, 0)[0], texel(&env, 1, 0)[0], texel(&env, 2, 0)[0]], [0.0, 0.0, 65504.0]);
        // A luminance-only file reads as grey.
        let grey = ExrPart::new(None, 1, 1, Compression::None, vec![ExrChannel::float("Y", vec![0.3])]);
        let env = read_exr(&write_to_vec(&ExrImage::single(grey)).unwrap()).unwrap();
        assert_eq!(texel(&env, 0, 0), [0.3, 0.3, 0.3, 1.0]);
        // A depth-only file has no colour to read.
        let depth = ExrPart::new(None, 1, 1, Compression::None, vec![ExrChannel::float("Z", vec![1.0])]);
        let error = read_exr(&write_to_vec(&ExrImage::single(depth)).unwrap()).unwrap_err();
        assert!(error.contains("no R, G, B"), "{error}");
        assert!(read_exr(b"not an exr").is_err());
    }

    #[test]
    fn decode_image_sniffs_exr_hdr_and_8_bit_files() {
        let part = ExrPart::new(None, 1, 1, Compression::None, vec![
            ExrChannel::float("R", vec![3.0]),
            ExrChannel::float("G", vec![2.0]),
            ExrChannel::float("B", vec![1.0]),
        ]);
        let exr = write_to_vec(&ExrImage::single(part)).unwrap();
        assert_eq!(decode_image(&exr).unwrap().data[0], [3.0, 2.0, 1.0, 1.0]);
        // Radiance goes through ibl::load_hdr: mid-grey texels read as 0.5.
        let hdr = decode_image(&flat_hdr(2, 1, |_, _| GREY)).unwrap();
        assert_eq!((hdr.width, hdr.height), (2, 1));
        assert_eq!(texel(&hdr, 1, 0), [0.5, 0.5, 0.5, 1.0]);
        // 8-bit files are sRGB: byte 128 is about 0.216 linear, not 0.5.
        let png = Cx::encode_rgba_as_png(2, 1, &[255, 128, 0, 255, 0, 0, 0, 255]).unwrap();
        let env = decode_image(&png).unwrap();
        assert_eq!((env.width, env.height), (2, 1));
        let t = texel(&env, 0, 0);
        assert!((t[0] - 1.0).abs() < 1.0e-6);
        assert!((t[1] - srgb_to_linear(128.0 / 255.0)).abs() < 1.0e-7);
        assert_eq!(t[2], 0.0);
        assert_eq!(t[3], 1.0);
        assert_eq!(texel(&env, 1, 0), [0.0, 0.0, 0.0, 1.0]);
        assert!(decode_image(b"not an image").is_err());
        // A broken .hdr reports as HDR, not as an unknown image.
        let error = decode_image(b"#?RADIANCE\nFORMAT=32-bit_rle_xyze\n\n-Y 1 +X 1\n\0\0\0\0").unwrap_err();
        assert!(error.starts_with("HDR:"), "{error}");
    }

    #[test]
    fn checked_texels_enforces_the_64_mpx_cap_on_every_path() {
        // The cap itself (8192 x 8192 = 64 Mpx) is allowed; one more texel or row is not. A
        // 64 Mpx image is too heavy to build here, so the shared check is what is pinned.
        assert_eq!(checked_texels(8192, 8192), Ok(MAX_PIXELS));
        assert_eq!(checked_texels(16384, 4096), Ok(MAX_PIXELS));
        assert_eq!(checked_texels(1, 1), Ok(1));
        let over = checked_texels(8192, 8193).unwrap_err();
        assert_eq!(over, "8192x8193 is over the 64 Mpx limit");
        // What a 16384 x 16384 PNG (the largest the image cache decodes) would ask for.
        assert!(checked_texels(16384, 16384).unwrap_err().contains("64 Mpx"));
        // A product that overflows usize is over the cap, not a wrap to something small.
        assert!(checked_texels(usize::MAX, 2).unwrap_err().contains("64 Mpx"));
        // Empty images are not images.
        assert!(checked_texels(0, 512).unwrap_err().contains("no texels"));
        assert!(checked_texels(512, 0).unwrap_err().contains("no texels"));
    }

    /// A 1x1 GIF of `frames` identical frames: palette entry 0 is orange (255, 128, 0).
    fn gif_1x1(frames: usize) -> Vec<u8> {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]); // 1x1, a 2-entry global palette
        gif.extend_from_slice(&[255, 128, 0, 0, 0, 0]);
        for _ in 0..frames {
            gif.extend_from_slice(&[0x21, 0xf9, 4, 0, 10, 0, 0, 0]); // graphic control: 100 ms
            gif.extend_from_slice(&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0]); // 1x1 image at the origin
            gif.extend_from_slice(&[2, 2, 0x44, 0x01, 0]); // LZW, minimum code size 2: clear, pixel 0, end
        }
        gif.push(0x3b);
        gif
    }

    #[test]
    fn decode_image_loads_a_still_gif_and_rejects_an_animated_one() {
        let env = decode_image(&gif_1x1(1)).unwrap();
        assert_eq!((env.width, env.height), (1, 1));
        assert_eq!(texel(&env, 0, 0), [1.0, srgb_to_linear(128.0 / 255.0), 0.0, 1.0]);
        // An animation's buffer is its whole frame atlas (here 4096x1: Cx::max_texture_width wide),
        // so it would otherwise load as one big map. It is refused by name, whatever its shape.
        let error = decode_image(&gif_1x1(2)).unwrap_err();
        assert!(error.contains("animated"), "{error}");
    }

    #[test]
    fn load_equirect_accepts_2_to_1_rolls_and_rejects_the_rest() {
        let dir = scratch_dir("equirect");
        let good = dir.join("good.hdr");
        let near = dir.join("near.hdr");
        let bad = dir.join("bad.hdr");
        // A bright texel at the file's centre column (4 of 8), row 1: that is +X in the file.
        let bright = [255u8, 255, 255, 129];
        std::fs::write(&good, flat_hdr(8, 4, |x, y| if (x, y) == (4, 1) { bright } else { GREY })).unwrap();
        std::fs::write(&near, flat_hdr(7, 4, |_, _| GREY)).unwrap();
        std::fs::write(&bad, flat_hdr(6, 4, |_, _| GREY)).unwrap();
        // load_image keeps the file's columns; load_equirect rolls +X to the engine's u = 0.75.
        let file = load_image(&good, PathOrigin::Picked).unwrap();
        assert_eq!((file.width, file.height), (8, 4));
        assert!(texel(&file, 4, 1)[0] > 1.5 && texel(&file, 6, 1)[0] < 0.6);
        let engine = load_equirect(&good, PathOrigin::Picked).unwrap();
        assert_eq!((engine.width, engine.height), (8, 4));
        assert!(texel(&engine, 6, 1)[0] > 1.5 && texel(&engine, 4, 1)[0] < 0.6);
        assert_eq!(engine, roll_quarter(&file, false));
        let x_plus = (ibl::dir_to_equirect_uv([1.0, 0.0, 0.0])[0] * 8.0).floor() as usize;
        assert_eq!(x_plus, 6);
        // One pixel off 2:1 is tolerated (a common crop); further off is not.
        assert_eq!(load_equirect(&near, PathOrigin::Picked).map(|e| (e.width, e.height)).unwrap(), (7, 4));
        let error = load_equirect(&bad, PathOrigin::Picked).unwrap_err();
        assert!(error.contains("2:1"), "{error}");
        let missing = load_equirect(&dir.join("missing.hdr"), PathOrigin::Picked).unwrap_err();
        assert!(missing.contains("missing.hdr"), "{missing}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A 16×8 baseline JPEG (4:4:4, quality 95, 291 bytes): the left 8×8 block is sRGB
    /// (200, 120, 40), the right one (30, 60, 90). makepad's decoder returns (201, 120, 41) and
    /// (30, 59, 89) for it.
    const TWO_BLOCK_JPG: [u8; 291] = [
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
        0xff, 0xdb, 0x00, 0x43, 0x00, 0x02, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x01, 0x01, 0x01, 0x02, 0x02, 0x02, 0x02, 0x02,
        0x04, 0x03, 0x02, 0x02, 0x02, 0x02, 0x05, 0x04, 0x04, 0x03, 0x04, 0x06, 0x05, 0x06, 0x06, 0x06, 0x05, 0x06, 0x06, 0x06,
        0x07, 0x09, 0x08, 0x06, 0x07, 0x09, 0x07, 0x06, 0x06, 0x08, 0x0b, 0x08, 0x09, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x06, 0x08,
        0x0b, 0x0c, 0x0b, 0x0a, 0x0c, 0x09, 0x0a, 0x0a, 0x0a, 0xff, 0xdb, 0x00, 0x43, 0x01, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02,
        0x05, 0x03, 0x03, 0x05, 0x0a, 0x07, 0x06, 0x07, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0xff, 0xc0,
        0x00, 0x11, 0x08, 0x00, 0x08, 0x00, 0x10, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xff, 0xc4, 0x00,
        0x15, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x09,
        0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xff, 0xc4, 0x00, 0x15, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x08, 0x09, 0xff, 0xc4, 0x00, 0x14, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f,
        0x00, 0x70, 0x2b, 0x2f, 0x12, 0xed, 0x49, 0x13, 0xfd, 0xff, 0xd9,
    ];

    #[test]
    fn decode_image_reads_a_jpg_as_srgb() {
        let env = decode_image(&TWO_BLOCK_JPG).unwrap();
        assert_eq!((env.width, env.height), (16, 8));
        // JPEG is lossy: allow two code values around the colours the file was made from.
        let near = |t: [f32; 4], rgb: [u8; 3]| {
            (0..3).all(|k| {
                let lo = srgb_to_linear((rgb[k] as f32 - 2.0) / 255.0);
                let hi = srgb_to_linear((rgb[k] as f32 + 2.0) / 255.0);
                t[k] >= lo && t[k] <= hi
            }) && t[3] == 1.0
        };
        for (x, y) in [(0, 0), (7, 7), (3, 4)] {
            assert!(near(texel(&env, x, y), [200, 120, 40]), "({x},{y}): {:?}", texel(&env, x, y));
        }
        for (x, y) in [(8, 0), (15, 7), (12, 3)] {
            assert!(near(texel(&env, x, y), [30, 60, 90]), "({x},{y}): {:?}", texel(&env, x, y));
        }
        // sRGB bytes are linearised: 200 reads as about 0.58, not 200/255 = 0.78.
        assert!(texel(&env, 0, 0)[0] < 0.62);
        // Through a file: a 2:1 JPG is an equirect and gets the same roll as every other format.
        let dir = scratch_dir("jpg");
        let path = dir.join("pano.jpg");
        std::fs::write(&path, TWO_BLOCK_JPG).unwrap();
        assert_eq!(load_equirect(&path, PathOrigin::Picked).unwrap(), roll_quarter(&env, false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn eight_bit_files_keep_their_alpha_and_equirects_are_opaque() {
        // A half-transparent white texel and a fully transparent one.
        let png = Cx::encode_rgba_as_png(2, 1, &[255, 255, 255, 128, 0, 0, 0, 0]).unwrap();
        let picture = decode_image(&png).unwrap();
        let white = texel(&picture, 0, 0);
        assert!(white[..3].iter().all(|c| (c - 1.0).abs() < 1.0e-6), "straight alpha: the colour is not darkened by it: {white:?}");
        assert!((white[3] - 128.0 / 255.0).abs() < 1.0e-6);
        assert_eq!(texel(&picture, 1, 0)[3], 0.0);
        // The same file through load_image keeps it; as an environment (2:1) it is opaque.
        let dir = scratch_dir("alpha");
        let path = dir.join("cutout.png");
        std::fs::write(&path, &png).unwrap();
        assert_eq!(load_image(&path, PathOrigin::Picked).unwrap(), picture);
        let environment = load_equirect(&path, PathOrigin::Picked).unwrap();
        assert!(environment.data.iter().all(|t| t[3] == 1.0), "{:?}", environment.data);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exr_alpha_is_read_straight_and_equirects_stay_premultiplied() {
        // OpenEXR stores colour premultiplied by A: a half-covered texel of
        // straight colour 2.0 is stored as 1.0 with A 0.5. Four wide, so the
        // equirect roll below is an exact column copy.
        let colour = vec![1.0f32, 0.0, 3.0, 3.0, 1.0, 0.0, 3.0, 3.0];
        let part = ExrPart::new(None, 4, 2, Compression::None, vec![
            ExrChannel::float("A", vec![0.5, 0.0, 1.0, 1.0, 0.5, 0.0, 1.0, 1.0]),
            ExrChannel::float("B", colour.clone()),
            ExrChannel::float("G", colour.clone()),
            ExrChannel::float("R", colour),
        ]);
        let bytes = write_to_vec(&ExrImage::single(part)).unwrap();
        let picture = read_exr(&bytes).unwrap();
        assert_eq!(texel(&picture, 0, 0), [2.0, 2.0, 2.0, 0.5], "straight, as the 8-bit decodes give it");
        assert_eq!(texel(&picture, 1, 0), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(texel(&picture, 2, 0), [3.0, 3.0, 3.0, 1.0]);
        // As an environment it is opaque in the colour the file stores (premultiplied): what it
        // showed before alpha was read (D13).
        let dir = scratch_dir("exr_alpha");
        let path = dir.join("cutout.exr");
        std::fs::write(&path, &bytes).unwrap();
        let environment = load_equirect(&path, PathOrigin::Picked).unwrap();
        assert!(environment.data.iter().all(|t| t[3] == 1.0), "{:?}", environment.data);
        let mut row: Vec<f32> = (0..4).map(|x| texel(&environment, x, 0)[0]).collect();
        row.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(row, [0.0, 1.0, 3.0, 3.0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// D13: an environment keeps the colour its file stores; only a light's
    /// picture uses alpha. An EXR texel stored as colour 5 with A = 0 is
    /// legal emission (a renderer's camera-visible sky written with alpha
    /// 0), and loaded black as an environment since B14 (L:124); a bright
    /// texel of low alpha lost colour to the straight read's 65504 cap. An
    /// 8-bit file's stored colour is its RGB, whatever its alpha.
    #[test]
    fn an_environment_keeps_the_colour_its_file_stores() {
        // Four wide, so the equirect roll is an exact column copy: A = 0 over
        // colour 5, A = 0.25 over 3, A = 1e-6 over 100 (straight that is 1e8,
        // over the cap), opaque 2.
        let colour = vec![5.0f32, 3.0, 100.0, 2.0, 5.0, 3.0, 100.0, 2.0];
        let part = ExrPart::new(None, 4, 2, Compression::None, vec![
            ExrChannel::float("A", vec![0.0, 0.25, 1.0e-6, 1.0, 0.0, 0.25, 1.0e-6, 1.0]),
            ExrChannel::float("B", colour.clone()),
            ExrChannel::float("G", colour.clone()),
            ExrChannel::float("R", colour),
        ]);
        let bytes = write_to_vec(&ExrImage::single(part)).unwrap();
        // As an environment: the stored colour, bit for bit, and opaque.
        let environment = decode_equirect(&bytes).unwrap();
        let stored = EnvMap { width: 4, height: 2, data: [5.0f32, 3.0, 100.0, 2.0, 5.0, 3.0, 100.0, 2.0].iter().map(|&c| [c, c, c, 1.0]).collect() };
        assert_eq!(environment, roll_quarter(&stored, false));
        // As a light's picture: straight colour and its alpha, as before.
        let picture = decode_image(&bytes).unwrap();
        assert_eq!(texel(&picture, 1, 0), [12.0, 12.0, 12.0, 0.25]);
        assert_eq!(texel(&picture, 0, 0)[3], 0.0);
        assert_eq!(texel(&picture, 3, 0), [2.0, 2.0, 2.0, 1.0]);
        // An 8-bit environment keeps its RGB too: transparent texels are not black.
        let png = Cx::encode_rgba_as_png(4, 2, &[[200u8, 100, 50, 0]; 8].concat()).unwrap();
        let environment = decode_equirect(&png).unwrap();
        let want = [srgb_to_linear(200.0 / 255.0), srgb_to_linear(100.0 / 255.0), srgb_to_linear(50.0 / 255.0), 1.0];
        assert!(environment.data.iter().all(|t| *t == want), "{:?}", environment.data);
        assert_eq!(decode_image(&png).unwrap().data[0][3], 0.0, "the picture keeps its alpha");
    }

    /// D5 (M4): a read stops at its byte cap, checked before the read. A
    /// 1 GiB file is too heavy to write here, so the shared reader is pinned
    /// with a small cap and the readers' own cap is pinned by its value.
    #[test]
    fn a_read_over_the_cap_is_refused() {
        assert_eq!(MAX_IMAGE_FILE_BYTES, 1 << 30, "1 GiB, as the plan owner ruled");
        let dir = scratch_dir("cap");
        let path = dir.join("eleven.bin");
        std::fs::write(&path, [7u8; 11]).unwrap();
        let error = read_image_file(&path, PathOrigin::Picked, 10).unwrap_err();
        assert!(error.contains("over the 10 byte limit") && error.contains("eleven.bin"), "{error}");
        assert_eq!(read_image_file(&path, PathOrigin::Picked, 11).unwrap(), vec![7u8; 11]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// D5 (M4): a folder or a device is refused before anything is read
    /// from it (`/dev/zero` would be read until memory runs out, a pipe can
    /// block the reader). The null device stands in for every device: it is
    /// the one a test can open without waiting or reading forever.
    #[test]
    fn a_device_or_a_folder_is_not_read() {
        let dir = scratch_dir("device");
        let device = if cfg!(windows) { std::path::PathBuf::from("NUL") } else { std::path::PathBuf::from("/dev/null") };
        for (what, path) in [("the null device", device), ("a folder", dir.clone())] {
            for origin in [PathOrigin::Picked, PathOrigin::Preset] {
                let error = load_image(&path, origin).unwrap_err();
                assert!(error.contains("not a regular file"), "{what}, {origin:?}: {error}");
                let error = load_equirect(&path, origin).unwrap_err();
                assert!(error.contains("not a regular file"), "{what}, {origin:?}: {error}");
            }
        }
        // The Win32 device namespaces are no picture whoever names them,
        // refused by their spelling alone.
        for device in [r"\\.\COM1", r"\\.\PhysicalDrive0", r"//./pipe/x", r"\\?\GLOBALROOT\Device\Null", r"\??\GLOBALROOT\Device\Null", r"\??\pipe\x"] {
            let error = load_image(std::path::Path::new(device), PathOrigin::Picked).unwrap_err();
            assert!(error.contains("a device path"), "{device}: {error}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// D5 (M4): a network path a preset names is refused by its spelling,
    /// before anything touches the file system (opening one connects to the
    /// host as the preset loads); the person's own pick of one is opened.
    /// Local paths load either way. Windows opens `\??\` (the NT object
    /// manager's own prefix) verbatim, as it does `\\?\`, so
    /// `\??\UNC\host\share` reaches the host too (an SMB open, seen on
    /// Windows in plan 1b's H5 review).
    #[test]
    fn a_network_path_from_a_preset_is_refused_and_a_picked_one_is_not() {
        let network = [
            r"\\nohost.invalid\share\lamp.png",
            "//nohost.invalid/share/lamp.png",
            r"\\?\UNC\nohost.invalid\share\lamp.png",
            r"/\nohost.invalid\share\lamp.png",
            r"\??\UNC\nohost.invalid\share\lamp.png",
            r"\??\unc\nohost.invalid\share\lamp.png",
        ];
        for key in network {
            let path = std::path::Path::new(key);
            let refusal = refused_path(path, PathOrigin::Preset).unwrap_or_else(|| panic!("{key} is not refused from a preset"));
            assert!(refusal.contains("network path") && refusal.contains("pick the file yourself"), "{key}: {refusal}");
            assert_eq!(refused_path(path, PathOrigin::Picked), None, "{key}: the person's own pick is opened");
            // The loaders refuse it the same way, without a look on the network.
            let error = load_image(path, PathOrigin::Preset).unwrap_err();
            assert!(error.contains("network path"), "{key}: {error}");
            let error = load_equirect(path, PathOrigin::Preset).unwrap_err();
            assert!(error.contains("network path"), "{key}: {error}");
        }
        // The network spelled through the object manager's device tree
        // (the redirector behind every UNC path) is a device path: refused
        // from a preset, and from the person too, whose dialog never gives one.
        for key in [r"\??\GLOBALROOT\Device\Mup\nohost.invalid\share\lamp.png", r"\\?\GLOBALROOT\Device\Mup\nohost.invalid\share\lamp.png"] {
            let path = std::path::Path::new(key);
            for origin in [PathOrigin::Preset, PathOrigin::Picked] {
                let refusal = refused_path(path, origin).unwrap_or_else(|| panic!("{key} is not refused, {origin:?}"));
                assert!(refusal.contains("a device path"), "{key}, {origin:?}: {refusal}");
            }
            let error = load_image(path, PathOrigin::Preset).unwrap_err();
            assert!(error.contains("a device path"), "{key}: {error}");
        }
        for local in [r"C:\pics\lamp.png", "C:/pics/lamp.png", r"\\?\C:\pics\lamp.png", r"\??\C:\pics\lamp.png", "/home/me/lamp.png", "lamp.png", r"pics\lamp.png"] {
            for origin in [PathOrigin::Picked, PathOrigin::Preset] {
                assert_eq!(refused_path(std::path::Path::new(local), origin), None, "{local}, {origin:?}");
            }
        }
        // A local file a preset names loads as a picked one does.
        let dir = scratch_dir("preset_local");
        let path = dir.join("pano.hdr");
        std::fs::write(&path, flat_hdr(8, 4, |_, _| GREY)).unwrap();
        assert_eq!(load_equirect(&path, PathOrigin::Preset).unwrap(), load_equirect(&path, PathOrigin::Picked).unwrap());
        assert_eq!(load_image(&path, PathOrigin::Preset).unwrap(), load_image(&path, PathOrigin::Picked).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_equirect_decodes_from_bytes_as_from_its_file() {
        let bytes = flat_hdr(8, 4, |x, _| if x == 4 { [255, 255, 255, 129] } else { GREY });
        let dir = scratch_dir("equirect_bytes");
        let path = dir.join("pano.hdr");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(decode_equirect(&bytes).unwrap(), load_equirect(&path, PathOrigin::Picked).unwrap());
        // The 2:1 rule holds for bytes too; with no path, the message names none.
        let error = decode_equirect(&flat_hdr(6, 4, |_, _| GREY)).unwrap_err();
        assert!(error.contains("2:1") && !error.contains(".hdr"), "{error}");
        // A file still names itself in its errors, as before.
        let error = load_equirect(&dir.join("missing.hdr"), PathOrigin::Picked).unwrap_err();
        assert!(error.contains("missing.hdr"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
