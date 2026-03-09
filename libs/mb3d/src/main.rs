use makepad_mb3d_render::{
    exr_meta::{
        ViewerCameraMetadata, MB3D_CAMERA_ATTRIBUTE_NAME, MB3D_MIP_FILTER_ATTRIBUTE_NAME,
        MB3D_MIP_LEVEL_ATTRIBUTE_NAME, MB3D_MIP_TOTAL_LEVELS_ATTRIBUTE_NAME,
    },
    formulas, m3p, render,
};
use makepad_openexr::{
    self, Compression as ExrCompression, ExrChannel, ExrImage, ExrPart, RawAttribute,
};
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Png,
    Exr,
}

impl OutputFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "png" => Ok(Self::Png),
            "exr" => Ok(Self::Exr),
            _ => Err(format!(
                "invalid --format value '{value}' (expected 'png' or 'exr')"
            )),
        }
    }

    fn default_output_path(self) -> &'static str {
        match self {
            Self::Png => "cathedral_test.png",
            Self::Exr => "cathedral_test.exr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExrLayout {
    Multipart,
    Channels,
}

impl ExrLayout {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "multipart" => Ok(Self::Multipart),
            "channels" => Ok(Self::Channels),
            _ => Err(format!(
                "invalid --exr-layout value '{value}' (expected 'multipart' or 'channels')"
            )),
        }
    }
}

struct Options {
    m3p_path: String,
    output_path: String,
    scale: f64,
    adaptive_ao: bool,
    antialiasing: render::AntialiasingMode,
    output_format: OutputFormat,
    exr_layout: ExrLayout,
    exr_layers: Option<Vec<render::ExrLayerSpec>>,
    exr_mip_chain: bool,
}

fn usage(program: &str) -> String {
    format!(
        "Usage: {program} [--scale <factor>] [--no-adaptive-ao] [--aa <none|2x2>] [--format <png|exr>] [--exr-layout <multipart|channels>] [--layers <codes|all>] [--mip] [input.m3p] [output]\n\
         PNG: writes the beauty image as RGBA8.\n\
         EXR: writes float layers; default layer set is 'c'.\n\
         EXR layouts: multipart=one EXR part per layer, channels=single-part named channels for AE/ProEXR style workflows where lowercase utility layers are usually display-normalized while reconstruction-critical depth/normal data stays raw and camera metadata is stored in the header.\n\
         EXR mip: --mip stores repeated /2 levels as multipart parts named mip0..mipN, stopping before the next level would fall below 30x15. This requires --exr-layout=channels.\n\
         EXR layers: {}\n\
         EXR all: {}\n\
         EXR note: lowercase=Pxr24 lossy and uppercase=Zip lossless.",
        render::exr_layer_legend(),
        render::all_exr_layer_codes(),
    )
}

fn inline_flag_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    let prefix = format!("{flag}=");
    arg.strip_prefix(&prefix)
}

fn next_flag_value(args: &mut std::env::Args, flag: &str, program: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {flag}\n{}", usage(program)))
}

fn parse_scale(value: &str, program: &str) -> Result<f64, String> {
    let scale = value
        .parse::<f64>()
        .map_err(|_| format!("invalid --scale value '{value}'"))?;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(format!(
            "--scale must be a positive finite number\n{}",
            usage(program)
        ));
    }
    Ok(scale)
}

fn parse_aa(value: &str, program: &str) -> Result<render::AntialiasingMode, String> {
    match value {
        "none" => Ok(render::AntialiasingMode::None),
        "2x2" => Ok(render::AntialiasingMode::X2),
        _ => Err(format!(
            "invalid --aa value '{value}' (expected 'none' or '2x2')\n{}",
            usage(program)
        )),
    }
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut args = std::env::args();
    let program = args
        .next()
        .unwrap_or_else(|| "makepad-mb3d-render".to_string());
    let mut scale = 1.0f64;
    let mut adaptive_ao = true;
    let mut antialiasing = render::AntialiasingMode::None;
    let mut output_format = OutputFormat::Png;
    let mut exr_layout = ExrLayout::Multipart;
    let mut layer_codes: Option<String> = None;
    let mut exr_mip_chain = false;
    let mut positional = Vec::new();

    while let Some(arg) = args.next() {
        if let Some(value) = inline_flag_value(&arg, "--scale") {
            scale = parse_scale(value, &program)?;
            continue;
        }
        if let Some(value) = inline_flag_value(&arg, "--aa") {
            antialiasing = parse_aa(value, &program)?;
            continue;
        }
        if let Some(value) = inline_flag_value(&arg, "--format") {
            output_format = OutputFormat::parse(value)?;
            continue;
        }
        if let Some(value) = inline_flag_value(&arg, "--exr-layout") {
            exr_layout = ExrLayout::parse(value)?;
            continue;
        }
        if let Some(value) = inline_flag_value(&arg, "--layers") {
            layer_codes = Some(value.to_string());
            continue;
        }

        match arg.as_str() {
            "--scale" => {
                let value = next_flag_value(&mut args, "--scale", &program)?;
                scale = parse_scale(&value, &program)?;
            }
            "--aa" => {
                let value = next_flag_value(&mut args, "--aa", &program)?;
                antialiasing = parse_aa(&value, &program)?;
            }
            "--format" => {
                let value = next_flag_value(&mut args, "--format", &program)?;
                output_format = OutputFormat::parse(&value)?;
            }
            "--exr-layout" => {
                let value = next_flag_value(&mut args, "--exr-layout", &program)?;
                exr_layout = ExrLayout::parse(&value)?;
            }
            "--layers" => {
                let value = next_flag_value(&mut args, "--layers", &program)?;
                layer_codes = Some(value);
            }
            "--mip" => exr_mip_chain = true,
            "--no-adaptive-ao" => adaptive_ao = false,
            "-h" | "--help" => {
                println!("{}", usage(&program));
                return Ok(None);
            }
            _ => positional.push(arg),
        }
    }

    if positional.len() > 2 {
        return Err(usage(&program));
    }

    if output_format == OutputFormat::Png && layer_codes.is_some() {
        return Err(format!(
            "--layers is only valid with --format=exr\n{}",
            usage(&program)
        ));
    }
    if output_format == OutputFormat::Png && exr_mip_chain {
        return Err(format!("--mip is only valid with --format=exr\n{}", usage(&program)));
    }
    if exr_mip_chain && exr_layout != ExrLayout::Channels {
        return Err(format!(
            "--mip requires --exr-layout=channels so each mip level can be loaded independently\n{}",
            usage(&program)
        ));
    }
    let exr_layers = if output_format == OutputFormat::Exr {
        Some(render::parse_exr_layer_specs(
            layer_codes.as_deref().unwrap_or("c"),
        )?)
    } else {
        None
    };

    Ok(Some(Options {
        m3p_path: positional
            .first()
            .cloned()
            .unwrap_or_else(|| "local/mb3d/cathedral.m3p".to_string()),
        output_path: positional
            .get(1)
            .cloned()
            .unwrap_or_else(|| output_format.default_output_path().to_string()),
        scale,
        adaptive_ao,
        antialiasing,
        output_format,
        exr_layout,
        exr_layers,
        exr_mip_chain,
    }))
}

fn exr_part_compression(spec: render::ExrLayerSpec) -> ExrCompression {
    match spec.compression {
        render::ExrLayerCompression::Lossless => ExrCompression::Zip,
        render::ExrLayerCompression::Lossy => ExrCompression::Pxr24,
    }
}

fn encode_png(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let encoder_options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);

    let mut encoder = PngEncoder::new(pixels, encoder_options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| format!("PNG encode failed: {err:?}"))?;
    Ok(out)
}

fn flattened_channel_name(part_name: &str, channel_name: &str, preserve_rgb: bool) -> String {
    if preserve_rgb && matches!(channel_name, "R" | "G" | "B" | "A") {
        channel_name.to_string()
    } else {
        format!("{part_name}.{channel_name}")
    }
}

const MIP_MIN_WIDTH: usize = 30;
const MIP_MIN_HEIGHT: usize = 15;
const GAUSSIAN5_WEIGHTS: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
const MIP_FILTER_NAME: &str = "gaussian5";

#[derive(Clone, Debug)]
struct FlattenedChannelImage {
    width: usize,
    height: usize,
    compression: ExrCompression,
    channels: Vec<ExrChannel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MipLevelShape {
    level: usize,
    width: usize,
    height: usize,
}

fn mip_level_shapes(width: usize, height: usize) -> Vec<MipLevelShape> {
    let mut out = vec![MipLevelShape {
        level: 0,
        width,
        height,
    }];

    let mut current_width = width;
    let mut current_height = height;
    while current_width > 1 && current_height > 1 {
        let next_width = (current_width / 2).max(1);
        let next_height = (current_height / 2).max(1);
        if next_width < MIP_MIN_WIDTH || next_height < MIP_MIN_HEIGHT {
            break;
        }
        out.push(MipLevelShape {
            level: out.len(),
            width: next_width,
            height: next_height,
        });
        current_width = next_width;
        current_height = next_height;
    }
    out
}

fn clamp_index(index: isize, max: usize) -> usize {
    index.clamp(0, max.saturating_sub(1) as isize) as usize
}

fn gaussian5_sample(samples: &[f32]) -> f32 {
    let mut weighted = 0.0f32;
    let mut weight_sum = 0.0f32;
    for (&sample, &weight) in samples.iter().zip(GAUSSIAN5_WEIGHTS.iter()) {
        if sample.is_finite() {
            weighted += sample * weight;
            weight_sum += weight;
        }
    }
    if weight_sum > 0.0 {
        weighted / weight_sum
    } else {
        0.0
    }
}

fn downscale_channel_gaussian5(samples: &[f32], width: usize, height: usize) -> Vec<f32> {
    let dst_width = (width / 2).max(1);
    let dst_height = (height / 2).max(1);
    let mut horizontal = vec![0.0f32; dst_width * height];

    for y in 0..height {
        let row = &samples[y * width..(y + 1) * width];
        for dst_x in 0..dst_width {
            let center = (dst_x * 2) as isize;
            let taps = [
                row[clamp_index(center - 2, width)],
                row[clamp_index(center - 1, width)],
                row[clamp_index(center, width)],
                row[clamp_index(center + 1, width)],
                row[clamp_index(center + 2, width)],
            ];
            horizontal[y * dst_width + dst_x] = gaussian5_sample(&taps);
        }
    }

    let mut out = vec![0.0f32; dst_width * dst_height];
    for dst_y in 0..dst_height {
        let center = (dst_y * 2) as isize;
        for x in 0..dst_width {
            let taps = [
                horizontal[clamp_index(center - 2, height) * dst_width + x],
                horizontal[clamp_index(center - 1, height) * dst_width + x],
                horizontal[clamp_index(center, height) * dst_width + x],
                horizontal[clamp_index(center + 1, height) * dst_width + x],
                horizontal[clamp_index(center + 2, height) * dst_width + x],
            ];
            out[dst_y * dst_width + x] = gaussian5_sample(&taps);
        }
    }

    out
}

fn int_attribute(name: &str, value: i32) -> RawAttribute {
    RawAttribute {
        name: name.to_string(),
        type_name: "int".to_string(),
        value: value.to_le_bytes().to_vec(),
    }
}

fn string_attribute(name: &str, value: &str) -> RawAttribute {
    RawAttribute {
        name: name.to_string(),
        type_name: "string".to_string(),
        value: value.as_bytes().to_vec(),
    }
}

fn append_camera_metadata(part: &mut ExrPart, camera: &ViewerCameraMetadata) {
    part.other_attributes.push(string_attribute(
        MB3D_CAMERA_ATTRIBUTE_NAME,
        &camera.encode_string(),
    ));
}

#[cfg(test)]
mod tests {
    use super::{
        build_mip_chain_parts, downscale_channel_gaussian5, flattened_channel_name,
        flattened_compression, mip_level_shapes, normalize_channels_layout_image,
        FlattenedChannelImage, MipLevelShape, MB3D_MIP_LEVEL_ATTRIBUTE_NAME,
    };
    use makepad_mb3d_render::render::{
        ExrLayerChannel, ExrLayerCompression, ExrLayerImage, ExrLayerKind, ExrLayerPart,
        ExrLayerSpec,
    };
    use makepad_openexr::{Compression, ExrChannel};

    #[test]
    fn beauty_channels_stay_display_named() {
        assert_eq!(flattened_channel_name("beauty", "R", true), "R");
        assert_eq!(flattened_channel_name("normal", "X", false), "normal.X");
    }

    #[test]
    fn flattened_lossy_uses_pxr24_unless_lossless_is_requested() {
        let specs = [
            ExrLayerSpec {
                kind: ExrLayerKind::Beauty,
                compression: ExrLayerCompression::Lossy,
            },
            ExrLayerSpec {
                kind: ExrLayerKind::Depth,
                compression: ExrLayerCompression::Lossless,
            },
        ];
        assert_eq!(flattened_compression(specs), Compression::Zip);
        assert_eq!(
            flattened_compression([ExrLayerSpec {
                kind: ExrLayerKind::Depth,
                compression: ExrLayerCompression::Lossy,
            }]),
            Compression::Pxr24
        );
    }

    #[test]
    fn lossy_depth_channels_keep_raw_values() {
        let spec = ExrLayerSpec {
            kind: ExrLayerKind::Depth,
            compression: ExrLayerCompression::Lossy,
        };
        let mut image = ExrLayerImage {
            width: 3,
            height: 1,
            parts: vec![ExrLayerPart {
                spec,
                name: "depth",
                channels: vec![ExrLayerChannel {
                    name: "Depth",
                    samples: vec![10.0, 20.0, 30.0],
                }],
            }],
        };

        normalize_channels_layout_image(&mut image);

        assert_eq!(image.parts[0].channels[0].samples, vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn lossless_channels_keep_raw_values() {
        let spec = ExrLayerSpec {
            kind: ExrLayerKind::AmbientOcclusion,
            compression: ExrLayerCompression::Lossless,
        };
        let mut image = ExrLayerImage {
            width: 2,
            height: 1,
            parts: vec![ExrLayerPart {
                spec,
                name: "ambient_occlusion",
                channels: vec![ExrLayerChannel {
                    name: "AO",
                    samples: vec![1.0, 1.0],
                }],
            }],
        };

        normalize_channels_layout_image(&mut image);

        assert_eq!(image.parts.len(), 1);
        assert_eq!(image.parts[0].channels.len(), 1);
        assert_eq!(image.parts[0].channels[0].samples, vec![1.0, 1.0]);
    }

    #[test]
    fn lossy_channels_keep_fixed_schema() {
        let spec = ExrLayerSpec {
            kind: ExrLayerKind::AmbientOcclusion,
            compression: ExrLayerCompression::Lossy,
        };
        let mut image = ExrLayerImage {
            width: 2,
            height: 1,
            parts: vec![ExrLayerPart {
                spec,
                name: "ambient_occlusion",
                channels: vec![ExrLayerChannel {
                    name: "AO",
                    samples: vec![1.0, 1.0],
                }],
            }],
        };

        normalize_channels_layout_image(&mut image);

        assert_eq!(image.parts.len(), 1);
        assert_eq!(image.parts[0].channels.len(), 1);
        assert_eq!(image.parts[0].channels[0].samples, vec![1.0, 1.0]);
    }

    #[test]
    fn lossy_position_channels_keep_raw_values() {
        let spec = ExrLayerSpec {
            kind: ExrLayerKind::Position,
            compression: ExrLayerCompression::Lossy,
        };
        let samples_x = vec![-3.5, 0.25, 8.0];
        let samples_y = vec![1.5, -2.0, 4.0];
        let samples_z = vec![12.0, 6.0, -1.0];
        let mut image = ExrLayerImage {
            width: 3,
            height: 1,
            parts: vec![ExrLayerPart {
                spec,
                name: "position",
                channels: vec![
                    ExrLayerChannel {
                        name: "X",
                        samples: samples_x.clone(),
                    },
                    ExrLayerChannel {
                        name: "Y",
                        samples: samples_y.clone(),
                    },
                    ExrLayerChannel {
                        name: "Z",
                        samples: samples_z.clone(),
                    },
                ],
            }],
        };

        normalize_channels_layout_image(&mut image);

        assert_eq!(image.parts[0].channels[0].samples, samples_x);
        assert_eq!(image.parts[0].channels[1].samples, samples_y);
        assert_eq!(image.parts[0].channels[2].samples, samples_z);
    }

    #[test]
    fn mip_shapes_stop_before_sub_30x15_level() {
        assert_eq!(
            mip_level_shapes(1920, 1080),
            vec![
                MipLevelShape {
                    level: 0,
                    width: 1920,
                    height: 1080
                },
                MipLevelShape {
                    level: 1,
                    width: 960,
                    height: 540
                },
                MipLevelShape {
                    level: 2,
                    width: 480,
                    height: 270
                },
                MipLevelShape {
                    level: 3,
                    width: 240,
                    height: 135
                },
                MipLevelShape {
                    level: 4,
                    width: 120,
                    height: 67
                },
                MipLevelShape {
                    level: 5,
                    width: 60,
                    height: 33
                },
                MipLevelShape {
                    level: 6,
                    width: 30,
                    height: 16
                },
            ]
        );
    }

    #[test]
    fn gaussian_downscale_preserves_constant_fields() {
        let src = vec![2.5; 8 * 6];
        let dst = downscale_channel_gaussian5(&src, 8, 6);
        assert_eq!(dst.len(), 4 * 3);
        assert!(dst.iter().all(|value| (*value - 2.5).abs() < 1.0e-6));
    }

    #[test]
    fn mip_chain_parts_are_named_and_tagged_by_level() {
        let parts = build_mip_chain_parts(FlattenedChannelImage {
            width: 512,
            height: 256,
            compression: Compression::Zip,
            channels: vec![ExrChannel::float("R", vec![1.0; 512 * 256])],
        })
        .unwrap();

        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].name.as_deref(), Some("mip0"));
        assert_eq!(parts[1].name.as_deref(), Some("mip1"));
        assert_eq!(parts[4].name.as_deref(), Some("mip4"));
        assert_eq!(parts[0].width().unwrap(), 512);
        assert_eq!(parts[1].width().unwrap(), 256);
        assert_eq!(
            parts[1]
                .other_attributes
                .iter()
                .find(|attribute| attribute.name == MB3D_MIP_LEVEL_ATTRIBUTE_NAME)
                .map(|attribute| i32::from_le_bytes(attribute.value.as_slice().try_into().unwrap())),
            Some(1)
        );
    }
}

fn flattened_compression(specs: impl IntoIterator<Item = render::ExrLayerSpec>) -> ExrCompression {
    if specs
        .into_iter()
        .any(|spec| spec.compression == render::ExrLayerCompression::Lossless)
    {
        ExrCompression::Zip
    } else {
        ExrCompression::Pxr24
    }
}

fn remap_linear(samples: &mut [f32]) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut has_finite = false;
    for &value in samples.iter() {
        if value.is_finite() {
            min = min.min(value);
            max = max.max(value);
            has_finite = true;
        }
    }
    if !has_finite {
        samples.fill(0.0);
        return;
    }

    let range = max - min;
    if range.abs() <= 1.0e-30 {
        let fill = if max.abs() > 1.0e-30 { 1.0 } else { 0.0 };
        for value in samples.iter_mut() {
            *value = if value.is_finite() { fill } else { 0.0 };
        }
        return;
    }

    for value in samples.iter_mut() {
        *value = if value.is_finite() {
            ((*value - min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

fn remap_positive_log(samples: &mut [f32]) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut has_positive = false;
    for &value in samples.iter() {
        if value.is_finite() && value > 0.0 {
            let logged = value.ln();
            min = min.min(logged);
            max = max.max(logged);
            has_positive = true;
        }
    }
    if !has_positive {
        samples.fill(0.0);
        return;
    }

    let range = max - min;
    if range.abs() <= 1.0e-30 {
        for value in samples.iter_mut() {
            *value = if value.is_finite() && *value > 0.0 {
                1.0
            } else {
                0.0
            };
        }
        return;
    }

    for value in samples.iter_mut() {
        *value = if value.is_finite() && *value > 0.0 {
            ((*value).ln() - min) / range
        } else {
            0.0
        };
    }
}

fn normalize_fold_part(part: &mut render::ExrLayerPart) {
    let (xyz, any_slice) = part.channels.split_at_mut(3);
    let any = &mut any_slice[0].samples;
    for axis in xyz.iter_mut() {
        for idx in 0..axis.samples.len() {
            axis.samples[idx] = if any[idx] > 1.0e-30 {
                axis.samples[idx] / any[idx]
            } else {
                0.0
            };
        }
    }
    remap_positive_log(any);
}

fn normalize_orbit_part(part: &mut render::ExrLayerPart) {
    remap_linear(&mut part.channels[0].samples);
    remap_linear(&mut part.channels[1].samples);
    remap_linear(&mut part.channels[2].samples);
    remap_positive_log(&mut part.channels[3].samples);
    remap_positive_log(&mut part.channels[4].samples);
}

fn normalize_uncertainty_part(part: &mut render::ExrLayerPart) {
    remap_linear(&mut part.channels[0].samples);
    remap_linear(&mut part.channels[1].samples);
    remap_linear(&mut part.channels[3].samples);
}

fn normalize_channels_layout_image(image: &mut render::ExrLayerImage) {
    for part in &mut image.parts {
        if part.spec.compression == render::ExrLayerCompression::Lossless {
            continue;
        }
        match part.spec.kind {
            render::ExrLayerKind::AmbientOcclusion
            | render::ExrLayerKind::Branches
            | render::ExrLayerKind::Beauty
            | render::ExrLayerKind::Gradient
            | render::ExrLayerKind::Normal
            | render::ExrLayerKind::Roughness
            | render::ExrLayerKind::Traps => {}
            render::ExrLayerKind::Depth => {}
            render::ExrLayerKind::Estimator => {
                remap_positive_log(&mut part.channels[0].samples);
            }
            render::ExrLayerKind::Folds => normalize_fold_part(part),
            render::ExrLayerKind::Iterations
            | render::ExrLayerKind::MarchSteps
            | render::ExrLayerKind::SignFlips => {
                for channel in &mut part.channels {
                    remap_linear(&mut channel.samples);
                }
            }
            render::ExrLayerKind::Orbit => normalize_orbit_part(part),
            render::ExrLayerKind::Position => {}
            render::ExrLayerKind::Uncertainty => normalize_uncertainty_part(part),
        }
    }
}

fn flatten_channels_layout_image(mut image: render::ExrLayerImage) -> FlattenedChannelImage {
    normalize_channels_layout_image(&mut image);
    let compression = flattened_compression(image.parts.iter().map(|part| part.spec));
    let channels = image
        .parts
        .into_iter()
        .flat_map(|part| {
            let preserve_rgb = part.spec.kind == render::ExrLayerKind::Beauty;
            part.channels.into_iter().map(move |channel| {
                ExrChannel::float(
                    flattened_channel_name(part.name, channel.name, preserve_rgb),
                    channel.samples,
                )
            })
        })
        .collect();
    FlattenedChannelImage {
        width: image.width,
        height: image.height,
        compression,
        channels,
    }
}

fn build_mip_chain_parts(base: FlattenedChannelImage) -> Result<Vec<ExrPart>, String> {
    let level_shapes = mip_level_shapes(base.width, base.height);
    let total_levels = i32::try_from(level_shapes.len())
        .map_err(|_| "mip level count overflow".to_string())?;
    let mut parts = Vec::with_capacity(level_shapes.len());

    let mut current_width = base.width;
    let mut current_height = base.height;
    let mut current_channels = base.channels;

    for level_shape in level_shapes {
        let level_index = i32::try_from(level_shape.level)
            .map_err(|_| "mip level index overflow".to_string())?;
        let mut part = ExrPart::new(
            Some(format!("mip{}", level_shape.level)),
            current_width,
            current_height,
            base.compression,
            current_channels.clone(),
        );
        part.other_attributes.push(int_attribute(
            MB3D_MIP_LEVEL_ATTRIBUTE_NAME,
            level_index,
        ));
        part.other_attributes.push(int_attribute(
            MB3D_MIP_TOTAL_LEVELS_ATTRIBUTE_NAME,
            total_levels,
        ));
        part.other_attributes.push(string_attribute(
            MB3D_MIP_FILTER_ATTRIBUTE_NAME,
            MIP_FILTER_NAME,
        ));
        parts.push(part);

        let next_width = (current_width / 2).max(1);
        let next_height = (current_height / 2).max(1);
        if next_width < MIP_MIN_WIDTH || next_height < MIP_MIN_HEIGHT {
            break;
        }

        current_channels = current_channels
            .into_iter()
            .map(|channel| {
                let samples = match channel.samples {
                    makepad_openexr::SampleBuffer::Float(values) => values,
                    makepad_openexr::SampleBuffer::Half(values) => {
                        values.into_iter().map(|value| value.to_f32()).collect()
                    }
                    makepad_openexr::SampleBuffer::Uint(values) => {
                        values.into_iter().map(|value| value as f32).collect()
                    }
                };
                ExrChannel::float(
                    channel.name,
                    downscale_channel_gaussian5(&samples, current_width, current_height),
                )
            })
            .collect();
        current_width = next_width;
        current_height = next_height;
    }

    Ok(parts)
}

fn encode_exr_with_layout(
    image: render::ExrLayerImage,
    layout: ExrLayout,
    mip_chain: bool,
    camera: &ViewerCameraMetadata,
) -> Result<Vec<u8>, String> {
    let mut parts = if mip_chain {
        build_mip_chain_parts(flatten_channels_layout_image(image))?
    } else {
        let render::ExrLayerImage {
            width,
            height,
            mut parts,
        } = image;
        match layout {
            ExrLayout::Multipart => parts
                .into_iter()
                .map(|part| {
                    let channels = part
                        .channels
                        .into_iter()
                        .map(|channel| ExrChannel::float(channel.name, channel.samples))
                        .collect();
                    ExrPart::new(
                        Some(part.name.to_string()),
                        width,
                        height,
                        exr_part_compression(part.spec),
                        channels,
                    )
                })
                .collect(),
            ExrLayout::Channels => {
                let flattened = flatten_channels_layout_image(render::ExrLayerImage {
                    width,
                    height,
                    parts: std::mem::take(&mut parts),
                });
                vec![ExrPart::new(
                    None,
                    flattened.width,
                    flattened.height,
                    flattened.compression,
                    flattened.channels,
                )]
            }
        }
    };
    for part in &mut parts {
        append_camera_metadata(part, camera);
    }
    makepad_openexr::write_to_vec(&ExrImage { parts })
        .map_err(|err| format!("EXR encode failed: {err}"))
}

fn selected_layer_codes(specs: &[render::ExrLayerSpec]) -> String {
    specs.iter().map(|spec| spec.code_char()).collect()
}

fn exr_layout_name(layout: ExrLayout) -> &'static str {
    match layout {
        ExrLayout::Multipart => "multipart",
        ExrLayout::Channels => "channels",
    }
}

fn main() {
    let Some(options) = (match parse_args() {
        Ok(options) => options,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }) else {
        return;
    };

    eprintln!("Parsing M3P: {}", options.m3p_path);
    let m3p_file = match m3p::parse(&options.m3p_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to parse M3P: {}", e);
            std::process::exit(1);
        }
    };

    let formula_slots = formulas::build_formulas(&m3p_file);
    if formula_slots.is_empty() {
        eprintln!("No formulas found, cannot render!");
        std::process::exit(1);
    }

    let mut params = render::RenderParams::from_m3p(&m3p_file);
    params.adaptive_ao_subsampling = options.adaptive_ao;
    params.antialiasing = options.antialiasing;
    params.apply_image_scale(options.scale);

    let w = params.camera.width as usize;
    let h = params.camera.height as usize;
    let out = match options.output_format {
        OutputFormat::Png => {
            let pixels =
                render::render(&formula_slots, &params, &m3p_file.lighting, &m3p_file.ssao);
            eprintln!("Encoding PNG {}x{} ...", w, h);
            match encode_png(&pixels, w, h) {
                Ok(out) => out,
                Err(err) => {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
        }
        OutputFormat::Exr => {
            let specs = options.exr_layers.as_ref().expect("EXR layer spec missing");
            eprintln!(
                "Encoding EXR {}x{} with layout {}{} and layers {} ...",
                w,
                h,
                exr_layout_name(options.exr_layout),
                if options.exr_mip_chain {
                    " + mip-chain"
                } else {
                    ""
                },
                selected_layer_codes(specs)
            );
            let image = match render::render_exr_layers(
                &formula_slots,
                &params,
                &m3p_file.lighting,
                &m3p_file.ssao,
                specs,
            ) {
                Ok(image) => image,
                Err(err) => {
                    eprintln!("EXR render failed: {err}");
                    std::process::exit(1);
                }
            };
            let camera_meta = ViewerCameraMetadata::from_camera(&params.camera);
            match encode_exr_with_layout(
                image,
                options.exr_layout,
                options.exr_mip_chain,
                &camera_meta,
            ) {
                Ok(out) => out,
                Err(err) => {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
        }
    };

    if let Err(err) = std::fs::write(&options.output_path, &out) {
        eprintln!("Failed to write {}: {}", options.output_path, err);
        std::process::exit(1);
    }
    eprintln!("Wrote {} ({} bytes)", options.output_path, out.len());
}
