//! Product shaping for directly-generated artifacts (aicore §9): the
//! dressing every creator applies when IT drives the fleet and publishes
//! the result itself — the store's queue and claim loop are gone, but the
//! catalog row a generation lands as is unchanged.
//!
//! What lives here:
//!
//! * [`GenRequest::from_body`] — the typed job-body contract shared by
//!   every generation kind (prompt, model pin, seed, passthrough params).
//! * [`wire_request`] — the store-vocabulary body → typed fleet request
//!   translation, one field map for every domain.
//! * [`build_product`] — one verified artifact → the catalog row its kind
//!   declares: measured dims/duration/mesh stats and a REAL thumbnail,
//!   parsed here so an unloadable product fails the run instead of
//!   becoming a row no viewer can open.
//! * [`dress_generated_publish`] — the full publish document: title from
//!   the person's own words, cleaned prompts ([`annotation_text`]),
//!   provenance, typed seed provenance. Byte-parity with what the queue's
//!   worker used to publish, so rows from either era are identical.

use crate::gen_kinds::{GenKind, InputNeed};
use crate::glb::inspect_glb;
use crate::import::{placeholder_thumb, usable_image_thumb};
use crate::thumbs::{
    audio_thumbnail_jpeg, decode_audio, encode_jpeg_bgra, jpeg_dims,
    placeholder_bgra_512, png_dims, THUMB_DIM,
};
use crate::videothumb::probe_video;
use makepad_asset_client::json::Value;
use makepad_asset_client::{
    PublishFile, PublishProvenance, PublishRequest, PublishRights, PublishStats,
    PublishThumbnail,
};
use makepad_asset_data::{AssetAlias, MediaType, ThumbnailMedia};

/// One artifact the fleet produced.
#[derive(Clone, Debug)]
pub struct GenArtifact {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// A source payload relayed from the catalog into the fleet request.
#[derive(Clone, Debug)]
pub struct GenInput {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// The typed job-body contract shared by every generation kind: a prompt, an
/// optional model pin, and the kind's own parameters passed through as the
/// client wrote them (the advertised profile's defaults, merged with
/// whatever the enqueuer added).
#[derive(Clone, Debug)]
pub struct GenRequest {
    pub kind: &'static GenKind,
    /// The prompt that goes to the model — the EXPANDED one once the
    /// expander has run.
    pub prompt: String,
    /// The human's own words, kept when an expansion replaced `prompt`.
    /// The published row is titled from this and its provenance names it,
    /// so a person can always find their run by what they typed.
    pub original_prompt: Option<String>,
    /// Empty = let domain affinity pick.
    pub model: String,
    pub seed: Option<u64>,
    /// The whole job body; the fleet adapter maps known keys onto the
    /// service's typed request and ignores the rest.
    pub body: Value,
    /// Resolved catalog input for transform kinds.
    pub input: Option<GenInput>,
}

impl GenRequest {
    /// Parse a claimed job body. A prompt is mandatory except for kinds that
    /// transform an input (an upscale has nothing to say about content).
    pub fn from_body(kind: &'static GenKind, body: &Value) -> Result<GenRequest, String> {
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        // A prompt is mandatory except for kinds that only TRANSFORM an
        // input (an upscale has nothing to say about content). A question
        // about an image is not a transform: without the question there is
        // nothing to answer.
        if prompt.is_empty() && (kind.input == InputNeed::None || kind.is_text()) {
            return Err("job body has no prompt".to_string());
        }
        if prompt.len() > 4_000 {
            return Err("prompt too long".to_string());
        }
        Ok(GenRequest {
            kind,
            prompt,
            original_prompt: None,
            model: body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
            seed: body.get("seed").and_then(Value::as_u64),
            body: body.clone(),
            input: None,
        })
    }
}

// ---------------------------------------------------------------------------
// product shaping
// ---------------------------------------------------------------------------

/// The full publish document for one directly-generated artifact — the exact
/// dressing the claim path applies (title from the person's words, cleaned
/// prompts, provenance, typed seed provenance), for creators that drive the
/// fleet themselves instead of through the store's queue (aicore §9).
/// `alias` is caller-owned: the queue used `<ns>/job-<hex>`; a direct run
/// names its own.
#[allow(clippy::too_many_arguments)]
pub fn dress_generated_publish(
    kind: &'static GenKind,
    namespace: &str,
    request: &GenRequest,
    product: GenArtifact,
    alias: Option<AssetAlias>,
    backend: String,
    model: String,
    version: String,
    rights: PublishRights,
) -> Result<PublishRequest, String> {
    let mut publish = build_product(kind, namespace, request, product)?;
    publish.alias = alias;
    publish.prompt = annotation_text(&request.prompt, MAX_PROMPT_BYTES);
    if let Some(original) = &request.original_prompt {
        publish.provenance = format!("expanded from: {}", annotation_text(original, 500));
    }
    publish.generator = "asset-worker".to_string();
    publish.backend = backend;
    publish.model = model.clone();
    publish.rights = rights;
    if let Some(seed) = request.seed.filter(|_| !version.is_empty()) {
        publish.manifest_provenance = Some(PublishProvenance {
            generator: "makepad-asset-ai".to_string(),
            model,
            version,
            seed,
            parents: vec![],
            params_digest: None,
        });
    }
    Ok(publish)
}

/// Turn one verified artifact into the catalog row its kind declares:
/// measured dimensions/duration/mesh stats and a real thumbnail, never an
/// assumed one. Payloads are PARSED here, so an unloadable product fails the
/// job instead of becoming a catalog entry no viewer can open.
pub fn build_product(
    kind: &'static GenKind,
    ns: &str,
    request: &GenRequest,
    product: GenArtifact,
) -> Result<PublishRequest, String> {
    // The row is titled by what the PERSON typed. An expanded prompt is a
    // paragraph of camera language; a person looking for their run scans for
    // their own words.
    let titled_from = request.original_prompt.as_deref().unwrap_or(&request.prompt);
    let mut title = annotation_text(titled_from, 120);
    if title.is_empty() {
        title = format!("Generated {}", kind.action);
    }
    let shape = kind.catalog().ok_or("kind publishes no catalog row")?;
    let bytes = product.bytes;
    let mut stats = PublishStats::default();
    let mut extra_tags: Vec<String> = Vec::new();
    let (media_millis, dims, thumbnail) = match shape.media {
        MediaType::Png | MediaType::Jpeg => {
            let dims = match shape.media {
                MediaType::Jpeg => jpeg_dims(&bytes),
                _ => png_dims(&bytes),
            }
            .ok_or("image: malformed header")?;
            let thumbnail = match usable_image_thumb(&bytes) {
                Some((thumb, media, w, h)) => {
                    PublishThumbnail { bytes: thumb, media, width: w, height: h, views: Vec::new() }
                }
                None => placeholder_thumb()?,
            };
            (0, Some(dims), thumbnail)
        }
        MediaType::Wav | MediaType::Mp3 | MediaType::Ogg => {
            let pcm = decode_audio(&bytes, shape.media)?;
            let millis = pcm.millis();
            let picture = audio_thumbnail_jpeg(&pcm)?;
            let thumbnail = PublishThumbnail {
                bytes: picture.bytes,
                media: ThumbnailMedia::Jpeg,
                width: picture.width,
                height: picture.height,
                views: picture.views,
            };
            (millis, None, thumbnail)
        }
        MediaType::Mp4 => {
            // The frame probe needs a file; the temp copy dies with this call.
            let tmp = std::env::temp_dir().join(format!(
                "asset-worker-{}-{}.mp4",
                std::process::id(),
                makepad_asset_client::util::to_hex(&[
                    (bytes.len() as u32).to_le_bytes()[0],
                    (bytes.len() as u32).to_le_bytes()[1],
                    (bytes.len() as u32).to_le_bytes()[2],
                    (bytes.len() as u32).to_le_bytes()[3],
                ])
            ));
            let probe = std::fs::write(&tmp, &bytes)
                .map_err(|e| e.to_string())
                .and_then(|_| probe_video(&tmp));
            let _ = std::fs::remove_file(&tmp);
            match probe {
                Ok(p) => (
                    p.duration_ms,
                    None,
                    PublishThumbnail {
                        bytes: p.thumbnail_jpeg,
                        media: ThumbnailMedia::Jpeg,
                        width: THUMB_DIM as u32,
                        height: THUMB_DIM as u32,
                        views: Vec::new(),
                    },
                ),
                Err(_) => {
                    // An honest placeholder plus the tag that says so; the
                    // clip itself is verified bytes and stays publishable.
                    extra_tags.push("no-preview-frame".to_string());
                    let jpeg =
                        encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM)?;
                    (
                        0,
                        None,
                        PublishThumbnail {
                            bytes: jpeg,
                            media: ThumbnailMedia::Jpeg,
                            width: THUMB_DIM as u32,
                            height: THUMB_DIM as u32,
                            views: Vec::new(),
                        },
                    )
                }
            }
        }
        MediaType::Glb => {
            let inspected = inspect_glb(&bytes)?;
            stats = PublishStats {
                triangles: inspected.triangles,
                vertices: inspected.vertices,
                joints: inspected.joints,
                clips: inspected.clips,
            };
            let thumbnail = match inspected.base_color.as_deref().and_then(usable_image_thumb) {
                Some((thumb, media, w, h)) => {
                    PublishThumbnail { bytes: thumb, media, width: w, height: h, views: Vec::new() }
                }
                None => placeholder_thumb()?,
            };
            (0, None, thumbnail)
        }
        MediaType::Ply => {
            let scene = makepad_splat::load_splat_from_bytes(&bytes, Some(std::path::Path::new(
                "product.ply",
            )))
            .map_err(|e| format!("ply: {e}"))?;
            if scene.splats.is_empty() {
                return Err("ply: no splats".to_string());
            }
            // Splat previews are a renderer's job (the Asset UI backfills
            // them offscreen); publishing an honest placeholder is better
            // than a fabricated image.
            (0, None, placeholder_thumb()?)
        }
        other => return Err(format!("unsupported product media {other:?}")),
    };

    let mut request_out = PublishRequest::new(
        ns,
        shape.asset_kind,
        title,
        PublishFile {
            bytes,
            media: shape.media,
            role: shape.role,
            media_millis,
            dims,
        },
        thumbnail,
    );
    request_out.categories = vec![shape.category.to_string()];
    request_out.tags = shape.tags.iter().map(|t| t.to_string()).collect();
    request_out.tags.extend(extra_tags);
    // Client-proposed tags ride the job body (the VJ's loop pipe tags its
    // clips `loop`). Bounded and charset-checked here so a buggy client
    // cannot spray the catalog with junk rows.
    if let Some(tags) = request.body.get("tags").and_then(Value::as_arr) {
        for tag in tags.iter().filter_map(Value::as_str).take(4) {
            let tag = tag.trim().to_ascii_lowercase();
            let ok = (2..=24).contains(&tag.len())
                && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if ok && !request_out.tags.contains(&tag) {
                request_out.tags.push(tag);
            }
        }
    }
    request_out.stats = stats;
    Ok(request_out)
}

fn bounded(text: &str, max: usize) -> String {
    makepad_asset_client::util::sanitize_text(text, max)
}

/// Longest prompt recorded on a published row (the job body itself refuses
/// anything past 4 000, so this only ever bounds an expansion).
const MAX_PROMPT_BYTES: usize = 4_000;

/// Text on its way into a searchable annotation.
///
/// Every annotation field is refused by the store if it carries a control
/// character, and the expander's answer is a paragraph with newlines in it.
/// A control character becomes ONE SPACE rather than nothing: dropping the
/// newline between two lines would run their words together and make the
/// prompt unsearchable. Whitespace runs collapse, the ends trim, and the
/// cut happens on a word boundary so the last word is a word.
pub fn annotation_text(text: &str, max: usize) -> String {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = String::with_capacity(spaced.len().min(max));
    for word in spaced.split_whitespace() {
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + word.len() > max {
            break;
        }
        if sep == 1 {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        // One word longer than the whole budget: cut it mid-word rather
        // than publish an empty field.
        out = bounded(spaced.trim(), max);
    }
    out
}

/// Map the job body onto the service's typed request. Only keys the service
/// actually understands are forwarded; unknown ones are ignored rather than
/// smuggled through, so a client typo fails visibly at the model instead of
/// silently changing nothing.
pub fn wire_request(
    request: &GenRequest,
    model: String,
) -> makepad_ai_hub::protocol::GenerateRequestJson {
    use makepad_ai_hub::protocol::GenerateRequestJson;
    let body = &request.body;
    let u32_of = |key: &str| body.get(key).and_then(Value::as_u64).map(|v| v as u32);
    // JSON numbers reach us as either variant; a client writing `30` for a
    // seconds field must mean the same thing as `30.0`.
    let f64_of = |key: &str| {
        body.get(key).and_then(|v| match v {
            Value::F64(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        })
    };
    let str_of = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let mut wire = GenerateRequestJson {
        model,
        prompt: (!request.prompt.is_empty()).then(|| request.prompt.clone()),
        negative_prompt: str_of("negative_prompt"),
        width: u32_of("width"),
        height: u32_of("height"),
        seed: request.seed,
        steps: u32_of("steps"),
        guidance: f64_of("guidance"),
        // Video
        frames: u32_of("frames"),
        codec: str_of("codec").or_else(|| {
            (request.kind.catalog().map(|c| c.media) == Some(MediaType::Mp4))
                .then(|| "h264".to_string())
        }),
        // Vision: how much answer to allow. The nine-line record needs
        // ~200; a client asking its own question sets its own budget.
        max_tokens: u32_of("max_tokens"),
        // Text: what the expansion is FOR (a video brief is not a mesh
        // brief). Only the expander sets it.
        target_domain: str_of("target_domain"),
        audio: body.get("audio").and_then(Value::as_bool),
        interpolate: u32_of("interpolate"),
        // Enhance (video post-process)
        upscale: u32_of("upscale"),
        flow_map: body.get("flow_map").and_then(Value::as_bool),
        // Audio / music / speech
        seconds: f64_of("seconds"),
        lyrics: str_of("lyrics"),
        text: str_of("text"),
        voice: str_of("voice"),
        speed: f64_of("speed"),
        // Mesh / splat
        remesh_resolution: u32_of("remesh_resolution"),
        texture: body.get("texture").and_then(Value::as_bool),
        decimation_target: u32_of("decimation_target"),
        texture_size: u32_of("texture_size"),
        gaussians: u32_of("gaussians"),
        motion_mode: str_of("motion_mode"),
        // Image transforms
        strength: f64_of("strength").map(|v| v as f32),
        canny_low: f64_of("canny_low"),
        canny_high: f64_of("canny_high"),
        ..Default::default()
    };
    if let Some(input) = &request.input {
        wire.input_b64 = String::from_utf8(makepad_base64::base64_encode(
            &input.bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .ok();
        wire.input_content_type = Some(input.content_type.clone());
    }
    wire
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_kinds::kind_of;
    use makepad_asset_client::json::{obj, s};

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= *byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// A 1x1 PNG: the smallest byte string that is a REAL decodable image,
    /// so the publish path's header parse and thumbnail are exercised for
    /// real rather than mocked away.
    fn tiny_png() -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let chunk = |png: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            png.extend_from_slice(tag);
            png.extend_from_slice(data);
            let mut crc_input = tag.to_vec();
            crc_input.extend_from_slice(data);
            png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);
        // One zlib stream holding a single uncompressed deflate block with
        // the filter byte + one RGB pixel.
        let raw = [0u8, 0xFF, 0x40, 0x20];
        let mut z = vec![0x78, 0x01, 0x01, 4, 0, 0xFB, 0xFF];
        z.extend_from_slice(&raw);
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for byte in raw {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        chunk(&mut png, b"IDAT", &z);
        chunk(&mut png, b"IEND", &[]);
        png
    }

    /// The VJ's loop pipe tags its clips `loop` through the job body; the
    /// worker forwards bounded, charset-clean tags and drops the rest.
    #[test]
    fn client_tags_ride_the_body_bounded_and_sanitized() {
        let kind = kind_of("image.generate").unwrap();
        let body = obj(vec![
            ("prompt", s("a looping tunnel")),
            (
                "tags",
                Value::Arr(vec![
                    s("loop"),
                    s(" LOOP "),                    // dup after normalize
                    s("x"),                          // too short
                    s("has spaces"),                 // bad charset
                    s("this-tag-is-far-too-long-to-keep"), // too long
                    s("ok-2"),
                ]),
            ),
        ]);
        let request = GenRequest::from_body(kind, &body).unwrap();
        let product = GenArtifact {
            content_type: "image/png".to_string(),
            bytes: tiny_png(),
        };
        let publish = build_product(kind, "gen", &request, product).unwrap();
        assert!(publish.tags.contains(&"loop".to_string()), "{:?}", publish.tags);
        assert_eq!(
            publish.tags.iter().filter(|t| *t == "loop").count(),
            1,
            "{:?}",
            publish.tags
        );
        // Take-4 bound is applied BEFORE filtering, so ok-2 (position 6) is
        // dropped with the junk; the junk itself never lands.
        assert!(!publish.tags.iter().any(|t| t.contains(' ') || t.len() > 24), "{:?}", publish.tags);
    }

    #[test]
    fn the_wire_request_carries_each_domains_own_parameters() {
        let kind = kind_of("music.generate").unwrap();
        let body = obj(vec![
            ("prompt", s("a slow dub techno loop")),
            ("seconds", Value::F64(30.0)),
            ("lyrics", s("[Instrumental]")),
            ("seed", Value::Int(9)),
            ("nonsense", s("ignored")),
        ]);
        let request = GenRequest::from_body(kind, &body).unwrap();
        let wire = wire_request(&request, "minimax-music3".to_string());
        assert_eq!(wire.seconds, Some(30.0));
        assert_eq!(wire.lyrics.as_deref(), Some("[Instrumental]"));
        assert_eq!(wire.seed, Some(9));
        // A wav product never asks for a video codec.
        assert_eq!(wire.codec, None);

        // Video defaults the codec to the compatibility fallback exactly as
        // the old video-only coordinator did.
        let kind = kind_of("video.generate").unwrap();
        let body = obj(vec![("prompt", s("a clip")), ("frames", Value::Int(65))]);
        let request = GenRequest::from_body(kind, &body).unwrap();
        let wire = wire_request(&request, "minimax-h3".to_string());
        assert_eq!(wire.codec.as_deref(), Some("h264"));
        assert_eq!(wire.frames, Some(65));
    }

    #[test]
    fn a_transform_kind_needs_a_prompt_less_body_but_a_generator_does_not() {
        let upscale = kind_of("image.upscale").unwrap();
        let body = obj(vec![("source_alias", s("gen/pic"))]);
        assert!(GenRequest::from_body(upscale, &body).is_ok());
        let image = kind_of("image.generate").unwrap();
        assert!(GenRequest::from_body(image, &body).is_err());
        assert!(GenRequest::from_body(
            image,
            &obj(vec![("prompt", s(" ".repeat(4_001)))])
        )
        .is_err());
    }

    /// An expanded prompt is model output. The store refuses a control
    /// character in ANY annotation field, and refusing it after the GPU
    /// spend cost a finished clip.
    #[test]
    fn an_expanded_prompt_keeps_its_words_when_its_newlines_go() {
        assert_eq!(
            annotation_text("a neon city\nseen from above\r\n\tat dusk", 200),
            "a neon city seen from above at dusk"
        );
        assert!(!annotation_text("a\u{7}b\u{0}c", 200).chars().any(char::is_control));
        // The cut lands on a word boundary...
        assert_eq!(annotation_text("alpha beta gamma", 12), "alpha beta");
        // ...unless the first word is longer than the whole budget.
        assert_eq!(annotation_text("aaaaaaaaaaaaaaa", 4), "aaaa");
        assert_eq!(annotation_text("   \n\t  ", 40), "");
    }
}
