use makepad_half::f16;
use makepad_openexr::{
    read_file, read_from_slice, read_headers_file, read_part_file, write_file, write_to_vec,
    Compression, ExrChannel, ExrImage, ExrPart,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn roundtrip_uncompressed_single_part_via_file() {
    let image = ExrImage::single(test_part(None, 5, 3, Compression::None, 0.0));
    let path = temp_path("single-none.exr");

    write_file(&path, &image).expect("write_file should succeed");
    let decoded = read_file(&path).expect("read_file should succeed");
    std::fs::remove_file(&path).ok();

    assert_images_match(&image, &decoded);
}

#[test]
fn roundtrip_zips_single_part_via_memory() {
    let image = ExrImage::single(test_part(None, 7, 4, Compression::Zips, 10.0));
    let encoded = write_to_vec(&image).expect("write_to_vec should succeed");
    let decoded = read_from_slice(&encoded).expect("read_from_slice should succeed");

    assert_images_match(&image, &decoded);
}

#[test]
fn roundtrip_zip_single_part_with_multiple_blocks() {
    let image = ExrImage::single(test_part(None, 6, 21, Compression::Zip, -3.25));
    let encoded = write_to_vec(&image).expect("write_to_vec should succeed");
    let decoded = read_from_slice(&encoded).expect("read_from_slice should succeed");

    assert_images_match(&image, &decoded);
}

#[test]
fn roundtrip_multipart_with_mixed_compressions() {
    let beauty = test_part(Some("beauty".to_string()), 4, 5, Compression::None, 1.5);
    let mut depth = test_part(Some("depth".to_string()), 4, 5, Compression::Zip, 64.0);
    depth.channels = vec![
        ExrChannel::float("Z", float_samples(4, 5, 64.0)),
        ExrChannel::half("Mask", half_samples(4, 5, 16.0)),
    ];

    let image = ExrImage {
        parts: vec![beauty, depth],
    };
    let encoded = write_to_vec(&image).expect("write_to_vec should succeed");
    let decoded = read_from_slice(&encoded).expect("read_from_slice should succeed");

    assert_images_match(&image, &decoded);
}

#[test]
fn roundtrip_pxr24_preserves_half_and_uint_and_quantizes_float() {
    let image = ExrImage::single(test_part(None, 32, 10, Compression::Pxr24, 7.0));
    let encoded = write_to_vec(&image).expect("write_to_vec should succeed");
    let decoded = read_from_slice(&encoded).expect("read_from_slice should succeed");

    let expected = quantized_for_pxr24(&image);
    assert_images_match(&expected, &decoded);
}

#[test]
fn read_headers_file_keeps_part_metadata_without_samples() {
    let image = ExrImage {
        parts: vec![
            test_part(Some("beauty".to_string()), 8, 4, Compression::Zip, 1.0),
            test_part(Some("mip1".to_string()), 4, 2, Compression::Zip, 2.0),
        ],
    };
    let path = temp_path("headers-only.exr");
    write_file(&path, &image).expect("write_file should succeed");

    let headers = read_headers_file(&path).expect("read_headers_file should succeed");
    std::fs::remove_file(&path).ok();

    assert_eq!(headers.parts.len(), 2);
    assert_eq!(headers.parts[0].name.as_deref(), Some("beauty"));
    assert_eq!(headers.parts[1].name.as_deref(), Some("mip1"));
    assert_eq!(headers.parts[0].width().unwrap(), 8);
    assert_eq!(headers.parts[1].height().unwrap(), 2);
    assert!(headers.parts[0]
        .channels
        .iter()
        .all(|channel| channel.samples.len() == 0));
}

#[test]
fn read_part_file_only_decodes_requested_part() {
    let beauty = test_part(Some("beauty".to_string()), 8, 4, Compression::None, 3.0);
    let mip = test_part(Some("mip1".to_string()), 4, 2, Compression::Zip, 12.0);
    let image = ExrImage {
        parts: vec![beauty.clone(), mip.clone()],
    };
    let path = temp_path("selected-part.exr");
    write_file(&path, &image).expect("write_file should succeed");

    let decoded = read_part_file(&path, 1).expect("read_part_file should succeed");
    std::fs::remove_file(&path).ok();

    assert_eq!(decoded.name.as_deref(), Some("mip1"));
    assert_eq!(decoded.width().unwrap(), 4);
    assert_eq!(decoded.height().unwrap(), 2);
    assert_eq!(decoded.channels, mip.channels);
}

fn test_part(
    name: Option<String>,
    width: usize,
    height: usize,
    compression: Compression,
    seed: f32,
) -> ExrPart {
    ExrPart::new(
        name,
        width,
        height,
        compression,
        vec![
            ExrChannel::half("A", half_samples(width, height, seed + 1.0)),
            ExrChannel::float("Depth", float_samples(width, height, seed + 2.0)),
            ExrChannel::uint("ObjectId", uint_samples(width, height, seed as u32 + 3)),
        ],
    )
}

fn half_samples(width: usize, height: usize, seed: f32) -> Vec<f16> {
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let value = seed + x as f32 * 0.25 + y as f32 * 0.5;
            out.push(f16::from_f32(value));
        }
    }
    out
}

fn float_samples(width: usize, height: usize, seed: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            out.push(seed + (x as f32 * 1.75) - (y as f32 * 0.5));
        }
    }
    out
}

fn uint_samples(width: usize, height: usize, seed: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            out.push(seed + (y as u32 * 17) + x as u32);
        }
    }
    out
}

fn assert_images_match(expected: &ExrImage, actual: &ExrImage) {
    assert_eq!(
        expected.parts.len(),
        actual.parts.len(),
        "part count mismatch"
    );

    for (expected_part, actual_part) in expected.parts.iter().zip(actual.parts.iter()) {
        assert_eq!(expected_part.name, actual_part.name, "part name mismatch");
        assert_eq!(
            expected_part.compression, actual_part.compression,
            "compression mismatch"
        );
        assert_eq!(
            expected_part.display_window, actual_part.display_window,
            "display window mismatch"
        );
        assert_eq!(
            expected_part.data_window, actual_part.data_window,
            "data window mismatch"
        );
        assert_eq!(
            expected_part.line_order, actual_part.line_order,
            "line order mismatch"
        );
        assert_eq!(
            expected_part.pixel_aspect_ratio, actual_part.pixel_aspect_ratio,
            "pixel aspect mismatch"
        );
        assert_eq!(
            expected_part.screen_window_center, actual_part.screen_window_center,
            "screen window center mismatch"
        );
        assert_eq!(
            expected_part.screen_window_width, actual_part.screen_window_width,
            "screen window width mismatch"
        );
        assert_eq!(expected_part.view, actual_part.view, "view mismatch");
        assert_eq!(
            expected_part.multi_view, actual_part.multi_view,
            "multi_view mismatch"
        );
        let mut expected_channels: Vec<_> = expected_part.channels.iter().collect();
        expected_channels.sort_by(|a, b| a.name.cmp(&b.name));
        let mut actual_channels: Vec<_> = actual_part.channels.iter().collect();
        actual_channels.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            expected_channels.len(),
            actual_channels.len(),
            "channel count mismatch"
        );

        for (expected_channel, actual_channel) in expected_channels
            .into_iter()
            .zip(actual_channels.into_iter())
        {
            assert_eq!(
                expected_channel.name, actual_channel.name,
                "channel name mismatch"
            );
            assert_eq!(
                expected_channel.p_linear, actual_channel.p_linear,
                "p_linear mismatch"
            );
            assert_eq!(
                expected_channel.sampling, actual_channel.sampling,
                "sampling mismatch"
            );
            assert_eq!(
                expected_channel.samples, actual_channel.samples,
                "sample mismatch"
            );
        }
    }
}

fn quantized_for_pxr24(image: &ExrImage) -> ExrImage {
    let mut out = image.clone();
    for part in &mut out.parts {
        for channel in &mut part.channels {
            if let makepad_openexr::SampleBuffer::Float(values) = &mut channel.samples {
                for value in values {
                    *value = pxr24_quantize(*value);
                }
            }
        }
    }
    out
}

fn pxr24_quantize(value: f32) -> f32 {
    let bits = value.to_bits();
    let sign = bits & 0x8000_0000;
    let exponent = bits & 0x7f80_0000;
    let mantissa = bits & 0x007f_ffff;

    let f24 = if exponent == 0x7f80_0000 {
        if mantissa != 0 {
            let mantissa = mantissa >> 8;
            (sign >> 8) | (exponent >> 8) | mantissa | if mantissa == 0 { 1 } else { 0 }
        } else {
            (sign >> 8) | (exponent >> 8)
        }
    } else {
        let rounded = ((exponent | mantissa) + (mantissa & 0x80)) >> 8;
        let reduced = if rounded >= 0x007f_8000 {
            (exponent | mantissa) >> 8
        } else {
            rounded
        };
        (sign >> 8) | reduced
    };

    f32::from_bits(f24 << 8)
}

fn temp_path(file_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("makepad-openexr-{nanos}-{file_name}"))
}
